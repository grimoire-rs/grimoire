// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Lock-tier errors (parse, serialize, flock contention, version gate).
//!
//! Three-layer shape mirroring [`crate::config::config_error`]: top
//! [`crate::error::Error`] → context-bearing [`LockError`] → discriminant
//! [`LockErrorKind`].

use std::path::PathBuf;

/// A lock-tier operation failed on a specific file.
#[derive(Debug)]
pub struct LockError {
    /// The file the failure occurred on (empty for in-memory parses).
    pub path: PathBuf,
    /// The specific failure.
    pub kind: LockErrorKind,
}

impl LockError {
    /// Attach `path` context to `kind`.
    pub fn new(path: impl Into<PathBuf>, kind: LockErrorKind) -> Self {
        Self {
            path: path.into(),
            kind,
        }
    }
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.as_os_str().is_empty() {
            write!(f, "{}", self.kind)
        } else {
            write!(f, "{}: {}", self.path.display(), self.kind)
        }
    }
}

impl std::error::Error for LockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // `Display` already embeds the kind's message; expose the kind's own
        // cause so `{:#}` chains do not print the kind twice.
        self.kind.source()
    }
}

/// Inner discriminant for lock-tier failures.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LockErrorKind {
    /// Failed to read or write the lock file.
    #[error("I/O error")]
    Io(#[source] std::io::Error),

    /// TOML parse failure (also fires on `deny_unknown_fields` and an
    /// unknown `lock_version` discriminant via `serde_repr`).
    #[error("invalid TOML")]
    TomlParse(#[source] toml::de::Error),

    /// TOML serialization failure (write path).
    #[error("TOML serialization error")]
    TomlSerialize(#[source] toml::ser::Error),

    /// The lock file exceeds the 64 KiB size cap.
    #[error("file too large: {size} bytes exceeds limit of {limit} bytes")]
    FileTooLarge { size: u64, limit: u64 },

    /// Another writer holds the exclusive advisory flock on the config
    /// file. Distinct from [`Self::Io`] so callers can retry with backoff.
    #[error("config file is locked by another process")]
    Locked,

    /// The lock's `declaration_hash_version` is from a newer release;
    /// reading is refused rather than comparing against a differently
    /// computed hash.
    #[error("unsupported declaration_hash_version {version}; this build understands version 1")]
    UnsupportedVersion { version: u8 },

    /// Partial-resolve refused: the predecessor lock's declaration hash
    /// does not match the current declaration. Both are surfaced so an
    /// operator can diff the lock against the live config.
    #[error(
        "partial-resolve refused: lock declaration_hash {previous_hash} does not match current {current_hash}; retry with a full resolve"
    )]
    StaleLockOnPartial {
        previous_hash: String,
        current_hash: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_with_path_uses_prefix() {
        let err = LockError::new(PathBuf::from("/p/grimoire.lock"), LockErrorKind::Locked);
        assert!(err.to_string().starts_with("/p/grimoire.lock: "));
    }

    #[test]
    fn display_without_path_no_leading_separator() {
        let err = LockError::new(PathBuf::new(), LockErrorKind::Locked);
        assert!(!err.to_string().starts_with(':'));
        assert!(!err.to_string().starts_with(' '));
    }
}
