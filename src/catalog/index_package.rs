// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! One package pointer as the index publishes it, and its projection onto a
//! [`CatalogEntry`]. Both index transports parse into it: the HTTP `all.json`
//! reader and the git `metadata.json` walk.

use serde::Deserialize;

use crate::catalog::registry_catalog::CatalogEntry;

/// One package pointer as published in the index (`all.json` element or a
/// single `metadata.json`). Unknown fields are tolerated so index schema
/// additions never break older grim binaries.
#[derive(Debug, Deserialize)]
pub(crate) struct IndexPackage {
    /// Metadata schema version; only `1` is consumed today.
    schema: u32,
    /// Package name (equals the index directory name). With `namespace` it
    /// forms the entry's [`CatalogEntry::index_path`], the tree's grouping.
    pub(crate) name: String,
    /// `<host>/<namespace…>` the pointer sits under. Derived by the index
    /// compiler into `all.json`; absent from a raw `metadata.json`, where the
    /// git walk fills it from the directory path instead.
    #[serde(default)]
    pub(crate) namespace: Option<String>,
    /// Artifact kind, as `ArtifactKind` spells it.
    kind: String,
    /// OCI reference (`registry/repository`, no tag) grim resolves against.
    r#ref: String,
    /// One-line description shown in `grim search`.
    #[serde(default)]
    description: Option<String>,
    /// Source repository URL.
    #[serde(default)]
    repository: Option<String>,
    /// Publisher keywords, matched by `grim search` alongside the
    /// description. Absent in pre-keywords index files (and in the hosted
    /// `all.json` until packages re-announce) — defaults to `[]`.
    #[serde(default, deserialize_with = "null_as_default")]
    keywords: Vec<String>,
    /// Short single-line blurb, matched by `grim search`. Absent in
    /// pre-summary index files — defaults to `None`.
    #[serde(default)]
    summary: Option<String>,
    /// Publisher deprecation message, mirroring the artifact's
    /// `com.grimoire.deprecated` annotation. Non-empty ⇒ deprecated. Absent
    /// in pre-deprecation index files, where the row stays unmarked (the
    /// pointer is the only browse-time source — reading the annotation would
    /// cost one manifest fetch per index entry).
    #[serde(default)]
    deprecated: Option<String>,
    /// Successor reference, mirroring `com.grimoire.replaced-by`.
    // Both spellings are read and the record is never dropped; `replaced_by`
    // wins. No alias: one would make a record with both keys fail serde's
    // duplicate-field check and vanish.
    #[serde(default)]
    replaced_by: Option<String>,
    #[serde(default, rename = "replacedBy")]
    replaced_by_camel: Option<String>,
    /// SPDX license expression, mirroring
    /// `org.opencontainers.image.licenses`. Absent in pre-license index files.
    #[serde(default)]
    license: Option<String>,
    /// Publishing commit date (RFC3339), mirroring
    /// `org.opencontainers.image.created` — the browse-time recency signal.
    /// Absent in pre-provenance index files.
    #[serde(default)]
    created: Option<String>,
    /// Harnesses a `mod` serves, mirroring `com.grimoire.mod.clients`. Held
    /// raw so a wrong-shaped value reads as absent instead of failing the
    /// record; see [`Self::served_clients`].
    #[serde(default)]
    serves: Option<serde_json::Value>,
}

impl IndexPackage {
    /// `serves` under the annotation's grammar; any invalid value is absent.
    fn served_clients(&self) -> Option<Vec<String>> {
        let names = self.serves.as_ref()?.as_array().and_then(|items| {
            let strs: Option<Vec<&str>> = items.iter().map(serde_json::Value::as_str).collect();
            crate::oci::annotations::normalize_mod_clients(strs?)
        });
        if names.is_none() {
            tracing::debug!("index entry '{}': invalid serves dropped", self.r#ref);
        }
        names
    }

    /// Project into a [`CatalogEntry`], or `None` when the `ref` carries no
    /// `registry/repository` split or the schema version is unknown.
    pub(crate) fn into_entry(self, fetched_at: &str) -> Option<CatalogEntry> {
        if self.schema != 1 {
            tracing::warn!(
                "skipping index entry '{}': unsupported schema {}",
                self.r#ref,
                self.schema
            );
            return None;
        }
        let (registry, repository) = self.r#ref.split_once('/')?;
        if registry.is_empty() || repository.is_empty() {
            return None;
        }
        let index_path = self.namespace.as_deref().and_then(|ns| {
            let path = index_path(ns, &self.name);
            if path.is_none() {
                tracing::debug!(
                    "index entry '{}': namespace placement rejected, grouping by OCI path",
                    self.r#ref
                );
            }
            path
        });
        let serves = if self.kind == "mod" {
            self.served_clients()
        } else {
            None
        };
        Some(CatalogEntry {
            registry: registry.to_string(),
            repository: repository.to_string(),
            index_path,
            kind: Some(self.kind),
            kind_read: false,
            description: self.description,
            summary: self.summary,
            keywords: self.keywords,
            // Same HTTPS prefix guard as the manifest read-back path.
            repository_url: self.repository.filter(|r| r.starts_with("https://")),
            revision: None,
            created: self.created,
            // Same trim/empty-⇒-`None` normalization the annotation seam
            // applies, so a hand-authored `"deprecated": " "` in the index
            // cannot mark a row deprecated with an empty notice.
            deprecated: self
                .deprecated
                .as_deref()
                .and_then(crate::oci::annotations::normalize_deprecated),
            // A snake value that normalizes to blank falls back to the camel one.
            replaced_by: [&self.replaced_by, &self.replaced_by_camel]
                .into_iter()
                .find_map(|v| v.as_deref().and_then(crate::oci::annotations::normalize_deprecated)),
            // The index phone book carries no OCI image annotations beyond the
            // license the pointer now mirrors.
            oci: crate::catalog::registry_catalog::OciMeta {
                licenses: self.license,
                ..Default::default()
            },
            // Phone-book contract: no version data in the index; tags are
            // resolved live from the registry at install time.
            latest_tag: None,
            version: None,
            // `all.json` carries no ratings and no pull counts — both live in
            // the `stats.json` sidecar and are joined onto the entry by ref
            // afterwards.
            rating: None,
            downloads: None,
            serves,
            fetched_at: fetched_at.to_string(),
        })
    }
}

/// A JSON `null` reads as the type's default. The current indexer clamps a
/// wrong-shaped list to `[]`, but an older or other indexer can publish `null`,
/// and rejecting it would fail the whole index.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// `<namespace>/<name>` when every segment is a plain identifier, else `None`.
///
/// The index is untrusted and these segments become tree labels verbatim, so
/// the check is an allowlist (`[A-Za-z0-9._:-]`, never `.`/`..`) rather than a
/// sanitizer: a pointer that fails it keeps its OCI-path grouping instead.
/// Depth and segment length are capped for the same reason.
pub(crate) fn index_path(namespace: &str, name: &str) -> Option<String> {
    const MAX_SEGMENTS: usize = 16;
    const MAX_SEGMENT_LEN: usize = 128;
    let valid = |seg: &str| {
        !seg.is_empty()
            && seg.len() <= MAX_SEGMENT_LEN
            && seg != "."
            && seg != ".."
            && seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
    };
    (namespace.split('/').count() < MAX_SEGMENTS && namespace.split('/').all(valid) && valid(name))
        .then(|| format!("{namespace}/{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(extra: &str) -> CatalogEntry {
        let json = format!(r#"{{"schema": 1, "name": "x", "kind": "mod", "ref": "ghcr.io/acme/x"{extra}}}"#);
        serde_json::from_str::<IndexPackage>(&json)
            .expect("record parses")
            .into_entry("t")
            .expect("record projects")
    }

    #[test]
    fn serves_is_sorted_and_deduplicated() {
        let e = entry(r#", "serves": ["opencode", "claude", "claude"]"#);
        assert_eq!(e.serves, Some(vec!["claude".to_string(), "opencode".to_string()]));
    }

    #[test]
    fn invalid_serves_reads_as_absent() {
        for bad in [
            r#"["Claude"]"#,
            r#"[""]"#,
            r#""claude""#,
            "[1]",
            "null",
            "[]",
            "1",
            r#"{"x": 1}"#,
            "true",
        ] {
            let e = entry(&format!(r#", "serves": {bad}"#));
            assert_eq!(e.serves, None, "serves {bad}");
        }
    }

    #[test]
    fn replaced_by_reads_both_spellings() {
        assert_eq!(
            entry(r#", "replacedBy": "ghcr.io/x/y""#).replaced_by.as_deref(),
            Some("ghcr.io/x/y")
        );
        assert_eq!(
            entry(r#", "replaced_by": "ghcr.io/x/y""#).replaced_by.as_deref(),
            Some("ghcr.io/x/y")
        );
    }

    #[test]
    fn replaced_by_wins_when_both_spellings_are_present() {
        let e = entry(r#", "replaced_by": "ghcr.io/x/snake", "replacedBy": "ghcr.io/x/camel""#);
        assert_eq!(e.replaced_by.as_deref(), Some("ghcr.io/x/snake"));
        let e = entry(r#", "replacedBy": "ghcr.io/x/camel", "replaced_by": "ghcr.io/x/snake""#);
        assert_eq!(e.replaced_by.as_deref(), Some("ghcr.io/x/snake"));
    }

    #[test]
    fn non_mod_kind_drops_serves() {
        let json = r#"{"schema": 1, "name": "x", "kind": "skill", "ref": "ghcr.io/acme/x", "serves": ["claude"]}"#;
        let e = serde_json::from_str::<IndexPackage>(json)
            .unwrap()
            .into_entry("t")
            .unwrap();
        assert_eq!(e.serves, None);
    }
}
