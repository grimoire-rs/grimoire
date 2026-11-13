// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Endpoint, headers, timeout and resource overrides from the `OTEL_*` environment.

use std::time::Duration;

use crate::telemetry::layer::TelemetryLayer;
use crate::telemetry::payload::set;

/// Why no request is made. Internal: ends in one debug line.
#[derive(Debug, thiserror::Error)]
pub enum Skip {
    #[error("no usable OTLP endpoint is configured")]
    Endpoint,
    #[error("unsupported OTLP protocol")]
    Protocol,
}

/// Header values that never reach a `Debug` or `Display` rendering.
#[derive(Clone, Default)]
pub struct Headers(Vec<(String, secrecy::SecretString)>);

impl Headers {
    pub fn new(pairs: Vec<(String, secrecy::SecretString)>) -> Self {
        Self(pairs)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Stores `name` lowercased; an existing entry equal ignoring ASCII case is replaced in place.
    pub fn set(&mut self, name: &str, value: secrecy::SecretString) {
        let name = name.to_ascii_lowercase();
        match self.0.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(&name)) {
            Some(slot) => *slot = (name, value),
            None => self.0.push((name, value)),
        }
    }

    /// `(name, value)` pairs; reading a value is an explicit act at the call site.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &secrecy::SecretString)> {
        self.0.iter().map(|(name, value)| (name.as_str(), value))
    }
}

impl std::fmt::Debug for Headers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Headers({} redacted)", self.0.len())
    }
}

/// Where and how to send.
#[derive(Debug)]
pub struct ExportConfig {
    /// The complete logs URL, used verbatim.
    pub endpoint: reqwest::Url,
    pub headers: Headers,
    pub timeout: Duration,
}

/// `OTEL_RESOURCE_ATTRIBUTES` pairs and `OTEL_SERVICE_NAME`, already parsed.
#[derive(Debug, Default, PartialEq)]
pub struct ResourceOverrides {
    pub attributes: Vec<(String, String)>,
    pub service_name: Option<String>,
}

/// Grim-owned header list, applied only with a `GRIM_CONFIG` endpoint.
pub(crate) const GRIM_TELEMETRY_OTLP_HEADERS: &str = "GRIM_TELEMETRY_OTLP_HEADERS";

const ENDPOINT: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
const LOGS_ENDPOINT: &str = "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT";
const HEADERS: &str = "OTEL_EXPORTER_OTLP_HEADERS";
const LOGS_HEADERS: &str = "OTEL_EXPORTER_OTLP_LOGS_HEADERS";
const TIMEOUT: &str = "OTEL_EXPORTER_OTLP_TIMEOUT";
const LOGS_TIMEOUT: &str = "OTEL_EXPORTER_OTLP_LOGS_TIMEOUT";
const PROTOCOL: &str = "OTEL_EXPORTER_OTLP_PROTOCOL";
const LOGS_PROTOCOL: &str = "OTEL_EXPORTER_OTLP_LOGS_PROTOCOL";
const RESOURCE_ATTRIBUTES: &str = "OTEL_RESOURCE_ATTRIBUTES";
const SERVICE_NAME: &str = "OTEL_SERVICE_NAME";

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(500);

/// Headers grim sets itself; an operator value for one would break the request.
const RESERVED_HEADERS: [&str; 4] = ["host", "content-length", "content-type", "transfer-encoding"];

/// An injected environment lookup, so tests never touch the process environment.
pub type Lookup<'a> = &'a dyn Fn(&str) -> Option<String>;

/// A variable's value when set to something other than whitespace.
fn non_empty(lookup: Lookup<'_>, key: &str) -> Option<String> {
    lookup(key).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

/// Resolves the export configuration from an injected variable lookup.
///
/// # Errors
///
/// [`Skip`] when no request should be made.
pub fn from_env(lookup: Lookup<'_>) -> Result<ExportConfig, Skip> {
    let (endpoint, signal_endpoint) = if let Some(raw) = non_empty(lookup, LOGS_ENDPOINT) {
        (parse_endpoint(&raw, false)?, true)
    } else if let Some(raw) = non_empty(lookup, ENDPOINT) {
        (parse_endpoint(&raw, true)?, false)
    } else {
        return Err(Skip::Endpoint);
    };

    let protocol = non_empty(lookup, LOGS_PROTOCOL).or_else(|| non_empty(lookup, PROTOCOL));
    if protocol.is_some_and(|p| !p.eq_ignore_ascii_case("http/protobuf")) {
        return Err(Skip::Protocol);
    }

    // A signal endpoint never receives the generic headers: they are the
    // operator's credentials for the base endpoint, not for wherever a
    // repository's environment points the logs signal.
    let raw_headers = if signal_endpoint {
        lookup(LOGS_HEADERS)
    } else {
        lookup(LOGS_HEADERS)
            .filter(|v| !v.trim().is_empty())
            .or_else(|| lookup(HEADERS))
    };
    let headers = raw_headers.map(|raw| parse_headers(&raw)).unwrap_or_default();

    Ok(ExportConfig {
        endpoint,
        headers,
        timeout: timeout(lookup),
    })
}

/// A usable `http(s)` URL without userinfo; a base URL gets `/v1/logs` appended.
pub(crate) fn parse_endpoint(raw: &str, base: bool) -> Result<reqwest::Url, Skip> {
    let mut url = reqwest::Url::parse(raw).map_err(|_| Skip::Endpoint)?;
    let plain = matches!(url.scheme(), "http" | "https")
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none();
    if !plain {
        return Err(Skip::Endpoint);
    }
    if base {
        let path = format!("{}/v1/logs", url.path().trim_end_matches('/'));
        url.set_path(&path);
    }
    Ok(url)
}

/// First set timeout variable wins; anything but a positive integer is the default.
fn timeout(lookup: Lookup<'_>) -> Duration {
    non_empty(lookup, LOGS_TIMEOUT)
        .or_else(|| non_empty(lookup, TIMEOUT))
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map_or(DEFAULT_TIMEOUT, Duration::from_millis)
}

/// Folds every resource override into `attributes`, lowest precedence first:
/// `OTEL_RESOURCE_ATTRIBUTES`, `OTEL_SERVICE_NAME`, then the config files.
pub fn resource_overrides(lookup: Lookup<'_>, layer: &TelemetryLayer) -> ResourceOverrides {
    let mut attributes = Vec::new();
    if let Some(raw) = lookup(RESOURCE_ATTRIBUTES) {
        let (pairs, dropped) = parse_pairs(&raw, |a, b| a == b, |_, _| true);
        log_dropped("resource attribute", &dropped);
        attributes = pairs;
    }
    if let Some(name) = non_empty(lookup, SERVICE_NAME) {
        set(&mut attributes, "service.name", &name);
    }
    for (key, value) in &layer.resource_attributes {
        set(&mut attributes, key, value);
    }
    if let Some(name) = layer.service_name.as_deref().filter(|n| !n.is_empty()) {
        set(&mut attributes, "service.name", name);
    }
    // `payload::resource` applies `service_name` last; leaving the env value
    // there would beat a `service.name` attribute from a config file.
    ResourceOverrides {
        attributes,
        service_name: None,
    }
}

/// The export destination: a config-file endpoint with its file headers
/// (the grim-owned header variable overriding by name), else `from_env`.
///
/// # Errors
///
/// [`Skip`] when no request should be made.
pub fn select(layer: &TelemetryLayer, lookup: Lookup<'_>) -> Result<ExportConfig, Skip> {
    // Grim-owned credentials ride only with a grim-owned endpoint.
    let Some(endpoint) = &layer.endpoint else {
        return from_env(lookup);
    };
    // An ambient OTel value must not change a request bound for a grim-chosen endpoint.
    let mut headers = layer.headers.clone();
    if let Some(raw) = lookup(GRIM_TELEMETRY_OTLP_HEADERS) {
        for (name, value) in parse_headers(&raw).iter() {
            headers.set(name, value.clone());
        }
    }
    Ok(ExportConfig {
        endpoint: endpoint.clone(),
        headers,
        timeout: layer.timeout.unwrap_or(DEFAULT_TIMEOUT),
    })
}

/// A legal header name that is not one grim sets itself.
pub(crate) fn valid_header_name(name: &str) -> bool {
    reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_ok_and(|n| !RESERVED_HEADERS.contains(&n.as_str()))
}

fn parse_headers(raw: &str) -> Headers {
    let (pairs, dropped) = parse_pairs(raw, str::eq_ignore_ascii_case, |name, value| {
        valid_header_name(name) && reqwest::header::HeaderValue::from_str(value).is_ok()
    });
    log_dropped("header", &dropped);
    Headers::new(
        pairs
            .into_iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), secrecy::SecretString::from(value)))
            .collect(),
    )
}

/// `k=v,k=v` with `v` percent-decoded, plus the keys of the pairs dropped as
/// malformed (a pair without `=` has the empty key). A later key for which
/// `same_key` holds replaces an earlier one: header names compare
/// case-insensitively, resource attribute keys exactly.
fn parse_pairs(
    raw: &str,
    same_key: fn(&str, &str) -> bool,
    accept: impl Fn(&str, &str) -> bool,
) -> (Vec<(String, String)>, Vec<String>) {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut dropped = Vec::new();
    for pair in raw.split(',').filter(|p| !p.trim().is_empty()) {
        let parsed = pair.split_once('=').and_then(|(key, value)| {
            let key = key.trim();
            let value = percent_decode(value.trim())?;
            (!key.is_empty() && accept(key, &value)).then(|| (key.to_owned(), value))
        });
        let Some((key, value)) = parsed else {
            dropped.push(pair.split_once('=').map_or("", |(key, _)| key.trim()).to_owned());
            continue;
        };
        match out.iter_mut().find(|(k, _)| same_key(k, &key)) {
            Some(slot) => *slot = (key, value),
            None => out.push((key, value)),
        }
    }
    (out, dropped)
}

/// One debug line per dropped pair; the key is all it ever names.
fn log_dropped(what: &str, dropped: &[String]) {
    for key in dropped {
        tracing::debug!(target: crate::telemetry::LOG_TARGET, "telemetry dropped a malformed {what} pair (key {key:?})");
    }
}

/// `%XX` decoding; a stray `%` stays literal. `None` when the result is not UTF-8.
fn percent_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| char::from(b).to_digit(16);
        match (
            bytes[i],
            bytes.get(i + 1).and_then(|b| hex(*b)),
            bytes.get(i + 2).and_then(|b| hex(*b)),
        ) {
            (b'%', Some(hi), Some(lo)) => {
                out.push(u8::try_from(hi * 16 + lo).ok()?);
                i += 3;
            }
            (b, ..) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    type Vars<'a> = &'a [(&'a str, &'a str)];

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| (*v).to_owned())
    }

    fn header_pairs(cfg: &ExportConfig) -> Vec<(String, String)> {
        cfg.headers
            .iter()
            .map(|(k, v)| (k.to_owned(), v.expose_secret().to_owned()))
            .collect()
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
    }

    #[test]
    fn headers_debug_never_prints_values() {
        let headers = Headers::new(vec![(
            "authorization".into(),
            secrecy::SecretString::from("Bearer s3cret"),
        )]);
        let rendered = format!("{headers:?}");
        assert!(
            !rendered.contains("s3cret") && !rendered.contains("authorization"),
            "{rendered}"
        );
    }

    #[test]
    fn headers_set_replaces_ignoring_case_in_place() {
        let mut headers = Headers::default();
        assert!(headers.is_empty());
        headers.set("A-One", secrecy::SecretString::from("1"));
        headers.set("b", secrecy::SecretString::from("2"));
        headers.set("a-ONE", secrecy::SecretString::from("3"));
        let got: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (k, v.expose_secret())).collect();
        assert_eq!(got, [("a-one", "3"), ("b", "2")]);
    }

    #[test]
    fn endpoint_selection_table() {
        // (env, expected URL or None for a skip)
        let cases: &[(Vars, Option<&str>)] = &[
            (&[], None),
            (&[(ENDPOINT, "")], None),
            (&[(ENDPOINT, "   ")], None),
            (&[(ENDPOINT, "https://h:4318")], Some("https://h:4318/v1/logs")),
            (&[(ENDPOINT, "https://h:4318/")], Some("https://h:4318/v1/logs")),
            (&[(ENDPOINT, "https://h/otlp/")], Some("https://h/otlp/v1/logs")),
            (&[(ENDPOINT, "https://h/otlp")], Some("https://h/otlp/v1/logs")),
            (&[(ENDPOINT, "https://h/otlp?x=1")], Some("https://h/otlp/v1/logs?x=1")),
            (
                &[(LOGS_ENDPOINT, "http://c:9/custom/path")],
                Some("http://c:9/custom/path"),
            ),
            (
                &[(LOGS_ENDPOINT, "http://c:9"), (ENDPOINT, "http://other")],
                Some("http://c:9/"),
            ),
            (
                &[(LOGS_ENDPOINT, ""), (ENDPOINT, "http://other")],
                Some("http://other/v1/logs"),
            ),
            (&[(ENDPOINT, "ftp://h")], None),
            (&[(ENDPOINT, "not a url")], None),
            (&[(ENDPOINT, "https://user:pw@h")], None),
            (&[(ENDPOINT, "https://user@h")], None),
            (
                &[(LOGS_ENDPOINT, "https://user:pw@h/v1/logs"), (ENDPOINT, "http://ok")],
                None,
            ),
        ];
        for (vars, expected) in cases {
            let got = from_env(&env(vars)).ok().map(|c| c.endpoint.to_string());
            assert_eq!(got.as_deref(), *expected, "{vars:?}");
        }
    }

    #[test]
    fn header_scoping_table() {
        // generic headers reach the base endpoint only; a signal endpoint gets only its own
        let generic = (HEADERS, "authorization=Bearer%20LEAK");
        let signal = (LOGS_HEADERS, "x-ok=1");
        let cases: &[(Vars, Vars)] = &[
            (&[(ENDPOINT, "http://h"), generic], &[("authorization", "Bearer LEAK")]),
            (&[(ENDPOINT, "http://h"), generic, signal], &[("x-ok", "1")]),
            (&[(ENDPOINT, "http://h"), signal], &[("x-ok", "1")]),
            (
                &[(ENDPOINT, "http://h"), generic, (LOGS_HEADERS, "")],
                &[("authorization", "Bearer LEAK")],
            ),
            (&[(LOGS_ENDPOINT, "http://h/v1/logs"), generic], &[]),
            (
                &[(LOGS_ENDPOINT, "http://h/v1/logs"), generic, signal],
                &[("x-ok", "1")],
            ),
            (&[(LOGS_ENDPOINT, "http://h/v1/logs"), signal], &[("x-ok", "1")]),
            // the signal endpoint wins even when a base endpoint is set as well
            (
                &[(LOGS_ENDPOINT, "http://h/v1/logs"), (ENDPOINT, "http://b"), generic],
                &[],
            ),
        ];
        for (vars, expected) in cases {
            let cfg = from_env(&env(vars)).expect("endpoint set");
            assert_eq!(header_pairs(&cfg), pairs(expected), "{vars:?}");
        }
    }

    #[test]
    fn header_grammar_table() {
        let cases: &[(&str, Vars)] = &[
            ("k=v", &[("k", "v")]),
            (" k = v , k2=v2 ", &[("k", "v"), ("k2", "v2")]),
            ("k=a=b", &[("k", "a=b")]),
            ("k=a%2Cb%3Dc", &[("k", "a,b=c")]),
            ("k=100%", &[("k", "100%")]),
            ("k=%zz", &[("k", "%zz")]),
            ("k=%FF", &[]),
            ("K=1,k=2", &[("k", "2")]),
            ("Authorization=Bearer%20t", &[("authorization", "Bearer t")]),
            ("noequals,k=v", &[("k", "v")]),
            ("=v,k=v", &[("k", "v")]),
            ("bad name=v,k=v", &[("k", "v")]),
            ("k=line%0Abreak,ok=1", &[("ok", "1")]),
            (
                "host=evil,content-type=x,content-length=1,transfer-encoding=x,k=v",
                &[("k", "v")],
            ),
            (",,k=v,,", &[("k", "v")]),
            ("k=", &[("k", "")]),
        ];
        for (raw, expected) in cases {
            let vars = [(ENDPOINT, "http://h"), (HEADERS, *raw)];
            let cfg = from_env(&env(&vars)).expect("endpoint set");
            assert_eq!(header_pairs(&cfg), pairs(expected), "{raw:?}");
        }
    }

    #[test]
    fn timeout_table() {
        let cases: &[(Vars, u64)] = &[
            (&[], 500),
            (&[(TIMEOUT, "3000")], 3000),
            (&[(TIMEOUT, "300"), (LOGS_TIMEOUT, "900")], 900),
            (&[(LOGS_TIMEOUT, "")], 500),
            (&[(TIMEOUT, "0")], 500),
            (&[(TIMEOUT, "-5")], 500),
            (&[(TIMEOUT, "abc")], 500),
            (&[(TIMEOUT, "1.5")], 500),
            // a set-but-invalid signal value does not fall through to the generic one
            (&[(LOGS_TIMEOUT, "abc"), (TIMEOUT, "300")], 500),
        ];
        for (vars, ms) in cases {
            let mut all = vec![(ENDPOINT, "http://h")];
            all.extend_from_slice(vars);
            let cfg = from_env(&env(&all)).expect("endpoint set");
            assert_eq!(cfg.timeout, Duration::from_millis(*ms), "{vars:?}");
        }
    }

    #[test]
    fn protocol_table() {
        let cases: &[(Vars, bool)] = &[
            (&[], true),
            (&[(PROTOCOL, "http/protobuf")], true),
            (&[(LOGS_PROTOCOL, "http/protobuf")], true),
            (&[(PROTOCOL, "grpc")], false),
            (&[(PROTOCOL, "http/json")], false),
            (&[(LOGS_PROTOCOL, "grpc"), (PROTOCOL, "http/protobuf")], false),
            (&[(LOGS_PROTOCOL, "http/protobuf"), (PROTOCOL, "grpc")], true),
            (&[(PROTOCOL, "")], true),
        ];
        for (vars, ok) in cases {
            let mut all = vec![(ENDPOINT, "http://h")];
            all.extend_from_slice(vars);
            let got = from_env(&env(&all));
            assert_eq!(got.is_ok(), *ok, "{vars:?}");
            if !ok {
                assert!(matches!(got, Err(Skip::Protocol)), "{vars:?}");
            }
        }
    }

    #[test]
    fn missing_endpoint_wins_over_everything_else() {
        let vars = [(PROTOCOL, "grpc"), (HEADERS, "a=b")];
        assert!(matches!(from_env(&env(&vars)), Err(Skip::Endpoint)));
    }

    #[test]
    fn resource_overrides_table() {
        let none = resource_overrides(&env(&[]), &TelemetryLayer::default());
        assert_eq!(none, ResourceOverrides::default());

        let got = resource_overrides(
            &env(&[
                (
                    RESOURCE_ATTRIBUTES,
                    "vcs.owner.name=acme, deployment.env = prod%20eu ,junk,=x,k=a=b",
                ),
                (SERVICE_NAME, " svc "),
            ]),
            &TelemetryLayer::default(),
        );
        assert_eq!(
            got.attributes,
            pairs(&[
                ("vcs.owner.name", "acme"),
                ("deployment.env", "prod eu"),
                ("k", "a=b"),
                ("service.name", "svc")
            ])
        );
        assert_eq!(got.service_name, None);

        let empty_name = resource_overrides(&env(&[(SERVICE_NAME, "  ")]), &TelemetryLayer::default());
        assert!(empty_name.attributes.is_empty());

        // header-only restrictions do not apply to resource attributes
        let host = resource_overrides(
            &env(&[(RESOURCE_ATTRIBUTES, "host=h,Dup=1,dup=2,dup=3")]),
            &TelemetryLayer::default(),
        );
        assert_eq!(host.attributes, pairs(&[("host", "h"), ("Dup", "1"), ("dup", "3")]));
    }

    fn layer_with(endpoint: Option<&str>, headers: &[(&str, &str)]) -> TelemetryLayer {
        let mut layer = TelemetryLayer {
            endpoint: endpoint.map(|e| parse_endpoint(e, true).expect("url")),
            ..TelemetryLayer::default()
        };
        for (k, v) in headers {
            layer.headers.set(k, secrecy::SecretString::from(*v));
        }
        layer
    }

    fn hdrs(cfg: &ExportConfig) -> Vec<(String, String)> {
        header_pairs(cfg)
    }

    #[test]
    fn destination_unit_table() {
        let asked = std::cell::RefCell::new(Vec::<String>::new());
        let ambient: Vars = &[
            (ENDPOINT, "http://b"),
            (LOGS_ENDPOINT, "http://c/v1/logs"),
            (HEADERS, "x-ambient=1"),
            (PROTOCOL, "grpc"),
            (TIMEOUT, "1"),
        ];
        let lookup = |key: &str| {
            asked.borrow_mut().push(key.to_owned());
            ambient.iter().find(|(k, _)| *k == key).map(|(_, v)| (*v).to_owned())
        };
        // (a) a file endpoint ignores every ambient OTel variable
        let mut layer = layer_with(Some("http://file"), &[("authorization", "Bearer f-fake")]);
        let cfg = select(&layer, &lookup).expect("endpoint");
        assert_eq!(cfg.endpoint.as_str(), "http://file/v1/logs");
        assert_eq!(hdrs(&cfg), pairs(&[("authorization", "Bearer f-fake")]));
        assert_eq!(cfg.timeout, Duration::from_millis(500));
        layer.timeout = Some(Duration::from_millis(900));
        assert_eq!(
            select(&layer, &lookup).expect("endpoint").timeout,
            Duration::from_millis(900)
        );
        assert!(
            asked.borrow().iter().all(|k| !k.starts_with("OTEL_")),
            "{:?}",
            asked.borrow()
        );

        // (b) the grim header variable overrides by name, decoded
        let layer = layer_with(
            Some("http://file"),
            &[("authorization", "Bearer f-fake"), ("x-keep", "k")],
        );
        let vars: Vars = &[(GRIM_TELEMETRY_OTLP_HEADERS, "Authorization=Bearer%20e-fake,x-team=a")];
        let cfg = select(&layer, &env(vars)).expect("endpoint");
        assert_eq!(
            hdrs(&cfg),
            pairs(&[("authorization", "Bearer e-fake"), ("x-keep", "k"), ("x-team", "a")])
        );

        // (c) no file endpoint: exactly from_env, no grim-owned headers
        let layer = layer_with(None, &[("authorization", "Bearer f-fake")]);
        let envs: &[Vars] = &[
            &[(ENDPOINT, "http://h"), (HEADERS, "x-o=1")],
            &[(LOGS_ENDPOINT, "http://c:9/p"), (HEADERS, "x-o=1"), (TIMEOUT, "700")],
            &[(ENDPOINT, "http://h"), (LOGS_HEADERS, "x-s=2")],
        ];
        for base in envs {
            let mut all = base.to_vec();
            all.push((GRIM_TELEMETRY_OTLP_HEADERS, "x-grim=3"));
            let got = select(&layer, &env(&all)).expect("endpoint");
            let want = from_env(&env(base)).expect("endpoint");
            assert_eq!(got.endpoint, want.endpoint, "{base:?}");
            assert_eq!(got.timeout, want.timeout, "{base:?}");
            assert_eq!(hdrs(&got), hdrs(&want), "{base:?}");
        }

        // (d) no endpoint anywhere
        assert!(matches!(
            select(&layer, &env(&[(GRIM_TELEMETRY_OTLP_HEADERS, "a=b")])),
            Err(Skip::Endpoint)
        ));
    }

    fn resolved(vars: Vars, layer: &TelemetryLayer) -> Vec<(String, String)> {
        crate::telemetry::payload::resource(None, None, resource_overrides(&env(vars), layer))
    }

    fn service_name(out: &[(String, String)]) -> &str {
        out.iter()
            .find(|(k, _)| k == "service.name")
            .map(|(_, v)| v.as_str())
            .expect("service.name")
    }

    #[test]
    fn resource_overrides_with_layer_table() {
        let mut layer = TelemetryLayer {
            resource_attributes: pairs(&[("service.name", "from-file")]),
            ..TelemetryLayer::default()
        };
        let out = resolved(&[(SERVICE_NAME, "ambient")], &layer);
        assert_eq!(service_name(&out), "from-file");

        layer.service_name = Some("file-svc".into());
        assert_eq!(
            service_name(&resolved(&[(SERVICE_NAME, "ambient")], &layer)),
            "file-svc"
        );

        // an empty file value is ignored
        let blank = TelemetryLayer {
            service_name: Some(String::new()),
            ..TelemetryLayer::default()
        };
        assert_eq!(service_name(&resolved(&[(SERVICE_NAME, "svc")], &blank)), "svc");

        // empty layer keeps today's rule: OTEL_SERVICE_NAME beats the attribute
        let empty = TelemetryLayer::default();
        let out = resolved(
            &[(RESOURCE_ATTRIBUTES, "service.name=attr"), (SERVICE_NAME, "svc")],
            &empty,
        );
        assert_eq!(service_name(&out), "svc");

        // empty layer: output equals the pre-layer behaviour (derived keys, then attributes, then name)
        let base = resolved(&[], &empty);
        assert_eq!(
            base,
            crate::telemetry::payload::resource(None, None, ResourceOverrides::default())
        );
        let with_attr = resolved(&[(RESOURCE_ATTRIBUTES, "a=1,b=2")], &empty);
        let mut want = base.clone();
        want.extend(pairs(&[("a", "1"), ("b", "2")]));
        assert_eq!(with_attr, want);
        let with_both = resolved(&[(RESOURCE_ATTRIBUTES, "a=1,b=2"), (SERVICE_NAME, "svc")], &empty);
        let mut want = want.clone();
        want.iter_mut().find(|(k, _)| k == "service.name").expect("sn").1 = "svc".into();
        assert_eq!(with_both, want);

        // a layer attribute beats an equal OTEL_RESOURCE_ATTRIBUTES key, position kept
        let layer = TelemetryLayer {
            resource_attributes: pairs(&[("a", "file")]),
            ..TelemetryLayer::default()
        };
        let out = resolved(&[(RESOURCE_ATTRIBUTES, "a=1,b=2")], &layer);
        let tail = &out[base.len()..];
        assert_eq!(tail, pairs(&[("a", "file"), ("b", "2")]));
    }

    #[test]
    fn malformed_pairs_are_reported_by_key_only() {
        let (kept, dropped) = parse_pairs(
            "good=ok,bad key=TOPSECRET,=TOPSECRET2,nokey",
            |a, b| a == b,
            |key, _| !key.contains(' '),
        );
        assert_eq!(kept, pairs(&[("good", "ok")]));
        assert_eq!(dropped, ["bad key", "", ""].map(String::from));
        assert!(dropped.iter().all(|key| !key.contains("TOPSECRET")));
    }
}
