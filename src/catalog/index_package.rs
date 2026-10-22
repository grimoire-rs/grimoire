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
    /// `skill` / `rule` / `agent` / `bundle`.
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
    #[serde(default)]
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
    #[serde(default)]
    replaced_by: Option<String>,
    /// SPDX license expression, mirroring
    /// `org.opencontainers.image.licenses`. Absent in pre-license index files.
    #[serde(default)]
    license: Option<String>,
    /// Publishing commit date (RFC3339), mirroring
    /// `org.opencontainers.image.created` — the browse-time recency signal.
    /// Absent in pre-provenance index files.
    #[serde(default)]
    created: Option<String>,
}

impl IndexPackage {
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
        Some(CatalogEntry {
            registry: registry.to_string(),
            repository: repository.to_string(),
            index_path,
            kind: Some(self.kind),
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
            replaced_by: self
                .replaced_by
                .as_deref()
                .and_then(crate::oci::annotations::normalize_deprecated),
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
            fetched_at: fetched_at.to_string(),
        })
    }
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
