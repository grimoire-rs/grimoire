// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Catalog-tier errors.
//!
//! Three-layer shape mirroring the other subsystems: top
//! [`crate::error::Error`] → context-bearing [`CatalogError`] (carries the
//! catalog file path) → discriminant [`CatalogErrorKind`]. An OCI-access
//! failure is wrapped (`Access`) so it classifies through the existing
//! access taxonomy; catalog building never *errors* on offline — it
//! degrades — so `Access` only ever surfaces a genuine transport/auth
//! fault, never a benign miss.

use std::io;
use std::path::{Path, PathBuf};

use crate::catalog::redact::redact_url;
use crate::oci::access::error::AccessError;

/// A catalog operation failed, with the catalog file path for context.
#[derive(Debug)]
pub struct CatalogError {
    /// The catalog file the failure relates to.
    pub path: PathBuf,
    /// The specific failure.
    pub kind: CatalogErrorKind,
}

impl CatalogError {
    /// Attach `path` context to `kind`.
    pub fn new(path: impl Into<PathBuf>, kind: CatalogErrorKind) -> Self {
        Self {
            path: path.into(),
            kind,
        }
    }
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.kind)
    }
}

impl std::error::Error for CatalogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // `Display` already embeds the kind's message; expose the kind's own
        // cause so `{:#}` chains do not print the kind twice.
        self.kind.source()
    }
}

/// Inner discriminant for catalog-tier failures.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CatalogErrorKind {
    /// A local I/O failure reading or writing the catalog file.
    #[error("I/O error")]
    Io(#[source] io::Error),

    /// The catalog file could not be parsed as the expected JSON envelope.
    #[error("invalid catalog file")]
    Parse(#[source] serde_json::Error),

    /// The catalog file declares an on-disk version this build cannot read.
    ///
    /// Currently unreachable via [`CatalogError::parse`] (the sole catalog
    /// version, `V1`, is enforced by `serde_repr` and surfaces as
    /// [`Self::Parse`] instead); kept classified and directly tested
    /// (`display_prefixes_with_path`) as the landing spot for a future
    /// version-2 envelope that needs a softer-than-`Parse` rejection.
    #[allow(
        dead_code,
        reason = "classified in error.rs; awaiting a second on-disk catalog version"
    )]
    #[error("unsupported catalog version: {version}")]
    UnsupportedVersion { version: u8 },

    /// An OCI-access failure while (re)building the catalog.
    ///
    /// Boxed: `AccessError` embeds an `Identifier` and would otherwise
    /// inflate every `Result<_, CatalogError>` (clippy `result_large_err`).
    #[error("registry access failed")]
    Access(#[source] Box<AccessError>),

    /// A package-index fetch failure while (re)building an index-backed
    /// catalog (HTTP transport or index-content parse; git subprocess
    /// failures surface as `Io`).
    #[error("package index fetch failed for '{locator}'")]
    IndexFetch {
        /// The index locator the fetch ran against, without credentials.
        locator: String,
        /// The transport / parse cause, with every URL in it redacted.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl CatalogError {
    /// Wrap a parse failure (rejects an unknown envelope version too:
    /// `serde_repr` fails the version field at parse time).
    pub fn parse(path: &Path, source: serde_json::Error) -> Self {
        Self::new(path, CatalogErrorKind::Parse(source))
    }

    /// Wrap a local I/O failure.
    pub fn io(path: &Path, source: io::Error) -> Self {
        Self::new(path, CatalogErrorKind::Io(source))
    }

    /// Wrap an OCI-access failure.
    pub fn access(path: &Path, source: AccessError) -> Self {
        Self::new(path, CatalogErrorKind::Access(Box::new(source)))
    }

    /// Wrap a package-index fetch failure.
    ///
    /// The locator and the cause are redacted here because the error reaches
    /// logs and JSON `error` fields; a token in either would leak with it.
    pub fn index_fetch(
        path: &Path,
        locator: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        let cause: Box<dyn std::error::Error + Send + Sync> = source.into();
        Self::new(
            path,
            CatalogErrorKind::IndexFetch {
                locator: redact_url(&locator.into()),
                source: Box::new(RedactedCause::of(cause.as_ref())),
            },
        )
    }
}

/// A cause flattened to text so every URL in its chain can be redacted.
///
/// reqwest drops userinfo from its own messages but keeps the query string,
/// so the chain cannot be passed through as it came.
#[derive(Debug)]
struct RedactedCause(String);

impl RedactedCause {
    fn of(cause: &(dyn std::error::Error + 'static)) -> Self {
        let mut text = cause.to_string();
        let mut next = cause.source();
        while let Some(inner) = next {
            text.push_str(": ");
            text.push_str(&inner.to_string());
            next = inner.source();
        }
        Self(redact_url(&text))
    }
}

impl std::fmt::Display for RedactedCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RedactedCause {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_prefixes_with_path() {
        let err = CatalogError::new(
            "/grim/catalog.json",
            CatalogErrorKind::UnsupportedVersion { version: 9 },
        );
        assert!(err.to_string().starts_with("/grim/catalog.json: "));
        assert!(err.to_string().contains("unsupported catalog version: 9"));
    }

    /// Display and every `source()` of an error, as an operator would see it.
    fn full_text(err: &dyn std::error::Error) -> String {
        let mut text = err.to_string();
        let mut next = err.source();
        while let Some(cause) = next {
            text.push_str(": ");
            text.push_str(&cause.to_string());
            next = cause.source();
        }
        text
    }

    #[test]
    fn an_index_fetch_names_the_locator_without_its_credentials() {
        let err = CatalogError::index_fetch(
            Path::new("/c"),
            "https://user:s3cret@idx.example/i?token=abc123#frag",
            io::Error::other("boom"),
        );

        let CatalogErrorKind::IndexFetch { locator, .. } = &err.kind else {
            panic!("expected an index fetch failure");
        };
        assert_eq!(locator, "https://idx.example/i");
        for text in [err.to_string(), err.kind.to_string()] {
            assert!(text.contains("https://idx.example/i"), "{text}");
            for secret in ["s3cret", "abc123", "frag"] {
                assert!(!text.contains(secret), "{secret} leaked into {text}");
            }
        }
    }

    #[test]
    fn an_index_fetch_cause_is_redacted_and_keeps_its_wording() {
        // reqwest drops userinfo from its error text but keeps the query.
        let cause = io::Error::other("error sending request for url (https://idx.example/all.json?token=abc123)");
        let err = CatalogError::index_fetch(Path::new("/c"), "https://idx.example/i", cause);

        let text = full_text(&err);

        assert!(
            text.contains("error sending request for url (https://idx.example/all.json)"),
            "{text}"
        );
        assert!(!text.contains("abc123"), "{text}");
    }

    #[test]
    fn source_chain_skips_kind_layer() {
        use std::error::Error;
        // Display embeds the kind, so the chain must not re-expose it: the
        // kind's own cause (the I/O error) is surfaced directly instead.
        let err = CatalogError::io(Path::new("/x"), io::Error::other("boom"));
        assert!(err.source().expect("chain reaches the I/O cause").is::<io::Error>());
    }
}
