// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The `[telemetry]` table of `GRIM_CONFIG` files, validated and merged in list order.

use std::path::PathBuf;
use std::time::Duration;

use secrecy::SecretString;

use crate::config::env_layers::{Layer, LayerError};
use crate::telemetry::config::{Headers, parse_endpoint, valid_header_name};
use crate::telemetry::payload::set;

/// What the files say, merged; every field is already validated.
#[derive(Clone, Default)]
pub struct TelemetryLayer {
    pub enabled: Option<bool>,
    /// Complete logs URL (`<endpoint>/v1/logs`), already validated.
    pub endpoint: Option<reqwest::Url>,
    pub headers: Headers,
    pub timeout: Option<Duration>,
    pub service_name: Option<String>,
    /// Per exact key, later file wins; first-seen order kept.
    pub resource_attributes: Vec<(String, String)>,
}

// A file endpoint's query can carry a token, so it is redacted here.
impl std::fmt::Debug for TelemetryLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelemetryLayer")
            .field("enabled", &self.enabled)
            .field(
                "endpoint",
                &self
                    .endpoint
                    .as_ref()
                    .map(|u| crate::telemetry::redact::url(u.as_str())),
            )
            .field("headers", &self.headers)
            .field("timeout", &self.timeout)
            .field("service_name", &self.service_name)
            .field("resource_attributes", &self.resource_attributes)
            .finish()
    }
}

/// Keys grim does not read, per file that has any.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct IgnoredKeys(Vec<(PathBuf, Vec<String>)>);

impl IgnoredKeys {
    /// `ignored GRIM_CONFIG keys in '<path>': <k1>, <k2>`, files joined by `; `.
    pub fn warning(&self) -> Option<String> {
        if self.0.is_empty() {
            return None;
        }
        let parts: Vec<String> = self
            .0
            .iter()
            .map(|(path, keys)| format!("ignored GRIM_CONFIG keys in '{}': {}", path.display(), keys.join(", ")))
            .collect();
        Some(parts.join("; "))
    }
}

fn invalid(layer: &Layer, key: impl Into<String>) -> LayerError {
    LayerError::InvalidKey {
        path: layer.path.clone(),
        key: key.into(),
    }
}

impl TelemetryLayer {
    /// Merges the `[telemetry]` table of every layer, later files winning.
    ///
    /// # Errors
    ///
    /// [`LayerError::InvalidKey`] for the first unusable value; it names the
    /// key and never the value.
    pub fn merge(layers: &[Layer]) -> Result<(TelemetryLayer, IgnoredKeys), LayerError> {
        let mut out = TelemetryLayer::default();
        let mut ignored = Vec::new();
        for layer in layers {
            let mut skipped: Vec<String> = layer.table.keys().filter(|k| *k != "telemetry").cloned().collect();
            if let Some(value) = layer.table.get("telemetry") {
                let table = value.as_table().ok_or_else(|| invalid(layer, "telemetry"))?;
                out.apply(layer, table, &mut skipped)?;
            }
            if !skipped.is_empty() {
                ignored.push((layer.path.clone(), skipped));
            }
        }
        Ok((out, IgnoredKeys(ignored)))
    }

    fn apply(&mut self, layer: &Layer, table: &toml::Table, skipped: &mut Vec<String>) -> Result<(), LayerError> {
        for (key, value) in table {
            match key.as_str() {
                "enabled" => {
                    self.enabled = Some(value.as_bool().ok_or_else(|| invalid(layer, "telemetry.enabled"))?);
                }
                "endpoint" => {
                    let url = value
                        .as_str()
                        .and_then(|raw| parse_endpoint(raw, true).ok())
                        .ok_or_else(|| invalid(layer, "telemetry.endpoint"))?;
                    self.endpoint = Some(url);
                }
                "headers" => {
                    let headers = value.as_table().ok_or_else(|| invalid(layer, "telemetry.headers"))?;
                    for (name, raw) in headers {
                        // The name is never echoed: a pasted secret can land there.
                        if !valid_header_name(name) {
                            return Err(invalid(layer, "telemetry.headers"));
                        }
                        let text = raw
                            .as_str()
                            .filter(|v| reqwest::header::HeaderValue::from_str(v).is_ok())
                            .ok_or_else(|| {
                                invalid(layer, format!("telemetry.headers.{}", name.to_ascii_lowercase()))
                            })?;
                        self.headers.set(name, SecretString::from(text));
                    }
                }
                "timeout" => {
                    let ms = value
                        .as_integer()
                        .and_then(|ms| u64::try_from(ms).ok())
                        .filter(|ms| *ms > 0)
                        .ok_or_else(|| invalid(layer, "telemetry.timeout"))?;
                    self.timeout = Some(Duration::from_millis(ms));
                }
                "protocol" => {
                    if !value.as_str().is_some_and(|p| p.eq_ignore_ascii_case("http/protobuf")) {
                        return Err(invalid(layer, "telemetry.protocol"));
                    }
                }
                "service_name" => {
                    let name = value.as_str().ok_or_else(|| invalid(layer, "telemetry.service_name"))?;
                    self.service_name = Some(name.to_owned());
                }
                "resource_attributes" => {
                    let attrs = value
                        .as_table()
                        .ok_or_else(|| invalid(layer, "telemetry.resource_attributes"))?;
                    for (attr, raw) in attrs {
                        let text = raw
                            .as_str()
                            .ok_or_else(|| invalid(layer, format!("telemetry.resource_attributes.{attr}")))?;
                        set(&mut self.resource_attributes, attr, text);
                    }
                }
                other => skipped.push(format!("telemetry.{other}")),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    fn layer(path: &str, toml_src: &str) -> Layer {
        Layer {
            path: PathBuf::from(path),
            table: toml::from_str::<toml::Table>(toml_src).unwrap(),
        }
    }

    fn merge_one(src: &str) -> Result<(TelemetryLayer, IgnoredKeys), LayerError> {
        TelemetryLayer::merge(&[layer("/org.toml", src)])
    }

    fn err_key(src: &str) -> String {
        let err = merge_one(src).unwrap_err();
        let LayerError::InvalidKey { ref key, .. } = err else {
            panic!("not InvalidKey: {err:?}");
        };
        assert!(!err.to_string().contains("SENTINEL"), "{err}");
        assert!(!format!("{err:?}").contains("SENTINEL"), "{err:?}");
        key.clone()
    }

    fn header_pairs(l: &TelemetryLayer) -> Vec<(String, String)> {
        l.headers
            .iter()
            .map(|(k, v)| (k.to_owned(), v.expose_secret().to_owned()))
            .collect()
    }

    #[test]
    fn wrong_types_name_the_dotted_key_not_the_value() {
        let cases = [
            ("[telemetry]\nenabled = \"SENTINEL-x1\"", "telemetry.enabled"),
            ("[telemetry]\nendpoint = 5", "telemetry.endpoint"),
            ("[telemetry]\nheaders = \"SENTINEL-x1\"", "telemetry.headers"),
            ("[telemetry]\ntimeout = \"SENTINEL-x1\"", "telemetry.timeout"),
            ("[telemetry]\nprotocol = 5", "telemetry.protocol"),
            ("[telemetry]\nservice_name = 5", "telemetry.service_name"),
            (
                "[telemetry]\nresource_attributes = \"SENTINEL-x1\"",
                "telemetry.resource_attributes",
            ),
            (
                "[telemetry.resource_attributes]\nk = 5",
                "telemetry.resource_attributes.k",
            ),
            ("[telemetry.headers]\nx-a = 5", "telemetry.headers.x-a"),
            ("telemetry = 1", "telemetry"),
        ];
        for (src, key) in cases {
            assert_eq!(err_key(src), key, "{src}");
        }
    }

    #[test]
    fn endpoint_with_userinfo_is_refused_without_echo() {
        let err = merge_one("[telemetry]\nendpoint = \"https://user:SENTINEL@h\"").unwrap_err();
        let shown = format!("{err} {err:?}");
        assert!(shown.contains("telemetry.endpoint"), "{shown}");
        assert!(!shown.contains("SENTINEL") && !shown.contains("user"), "{shown}");
    }

    #[test]
    fn endpoint_gets_logs_path() {
        let (l, _) = merge_one("[telemetry]\nendpoint = \"https://h:4318/otlp/\"").unwrap();
        assert_eq!(l.endpoint.unwrap().as_str(), "https://h:4318/otlp/v1/logs");
    }

    #[test]
    fn protocol_and_timeout_tables() {
        assert!(merge_one("[telemetry]\nprotocol = \"HTTP/Protobuf\"").is_ok());
        assert_eq!(err_key("[telemetry]\nprotocol = \"grpc\""), "telemetry.protocol");
        for bad in ["0", "-1", "\"fast\""] {
            assert_eq!(err_key(&format!("[telemetry]\ntimeout = {bad}")), "telemetry.timeout");
        }
        let (l, _) = merge_one("[telemetry]\ntimeout = 250").unwrap();
        assert_eq!(l.timeout, Some(Duration::from_millis(250)));
    }

    #[test]
    fn bad_header_name_or_value_is_refused_without_echo() {
        assert_eq!(
            err_key("[telemetry.headers]\nhost = \"SENTINEL-x1\""),
            "telemetry.headers"
        );
        assert_eq!(err_key("[telemetry.headers.\"bad name\"]\n"), "telemetry.headers");
        assert_eq!(
            err_key("[telemetry.headers]\nx-a = \"SENTINEL-x1\\nmore\""),
            "telemetry.headers.x-a"
        );
    }

    #[test]
    fn headers_merge_per_name_ignoring_case() {
        let layers = [
            layer(
                "/a.toml",
                "[telemetry.headers]\nAuthorization = \"one\"\nx-keep = \"k\"",
            ),
            layer("/b.toml", "[telemetry.headers]\nauthorization = \"two\""),
        ];
        let (l, _) = TelemetryLayer::merge(&layers).unwrap();
        assert_eq!(
            header_pairs(&l),
            [("authorization".into(), "two".into()), ("x-keep".into(), "k".into())]
        );
    }

    #[test]
    fn scalars_and_resource_attributes_later_wins() {
        let layers = [
            layer(
                "/a.toml",
                "[telemetry]\nenabled = true\nservice_name = \"a\"\n[telemetry.resource_attributes]\nk1 = \"a\"\nk2 = \"a\"",
            ),
            layer(
                "/b.toml",
                "[telemetry]\nenabled = false\n[telemetry.resource_attributes]\nk1 = \"b\"\nK2 = \"b\"",
            ),
        ];
        let (l, ignored) = TelemetryLayer::merge(&layers).unwrap();
        assert_eq!(l.enabled, Some(false));
        assert_eq!(l.service_name.as_deref(), Some("a"));
        assert_eq!(
            l.resource_attributes,
            [("k1", "b"), ("k2", "a"), ("K2", "b")].map(|(k, v)| (k.to_owned(), v.to_owned()))
        );
        assert_eq!(ignored.warning(), None);
    }

    #[test]
    fn ignored_keys_warn_in_table_order() {
        // toml::Table is sorted (no preserve_order), so keys list alphabetically.
        let src = "[options]\nx = 1\n[registries]\ny = 1\n[telemetry]\ncompression = \"gzip\"";
        let (l, ignored) = merge_one(src).unwrap();
        assert!(l.enabled.is_none());
        assert_eq!(
            ignored.warning().as_deref(),
            Some("ignored GRIM_CONFIG keys in '/org.toml': options, registries, telemetry.compression")
        );
    }

    #[test]
    fn ignored_keys_of_two_files_join() {
        let layers = [layer("/a.toml", "x = 1"), layer("/b.toml", "[telemetry]\nfoo = 1")];
        let (_, ignored) = TelemetryLayer::merge(&layers).unwrap();
        assert_eq!(
            ignored.warning().as_deref(),
            Some("ignored GRIM_CONFIG keys in '/a.toml': x; ignored GRIM_CONFIG keys in '/b.toml': telemetry.foo")
        );
    }

    #[test]
    fn empty_table_is_default() {
        let (l, ignored) = merge_one("[telemetry]").unwrap();
        assert!(l.enabled.is_none() && l.endpoint.is_none() && l.timeout.is_none());
        assert!(l.headers.is_empty() && l.resource_attributes.is_empty());
        assert!(ignored.warning().is_none());
    }

    #[test]
    fn layer_debug_redacts_endpoint_query() {
        let (l, _) = merge_one("[telemetry]\nendpoint = \"https://h/otlp/v1/logs?token=SENTINEL-q\"").unwrap();
        assert!(l.endpoint.is_some());
        assert!(!format!("{l:?}").contains("SENTINEL"));
    }

    #[test]
    fn merged_layer_debug_hides_header_values() {
        let (l, _) = merge_one("[telemetry.headers]\nx-a = \"SENTINEL-dbg\"").unwrap();
        assert!(!format!("{l:?}").contains("SENTINEL"));
    }
}
