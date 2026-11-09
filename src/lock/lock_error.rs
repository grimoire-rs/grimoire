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
    /// What keeps the pins, on a lock [`super::lock_io::load`] could not read.
    pub recovery: Option<LockRecovery>,
}

/// The way out of a lock no reader can use. Every one words it alike, since
/// deleting the file is the obvious move and the one that loses the pins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockRecovery {
    /// The file is damaged.
    RestoreOrRepair,
    /// The file is fine, and this build is too old for it.
    UpgradeGrim,
}

impl std::fmt::Display for LockRecovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::RestoreOrRepair => {
                "the lock cannot be read; restore it from git or repair it, because deleting the lock moves every pin"
            }
            Self::UpgradeGrim => {
                "this lock needs a newer grim; upgrade grim, because deleting the lock moves every pin"
            }
        })
    }
}

impl LockError {
    /// Attach `path` context to `kind`.
    pub fn new(path: impl Into<PathBuf>, kind: LockErrorKind) -> Self {
        Self {
            path: path.into(),
            kind,
            recovery: None,
        }
    }

    /// Whether this is the plain "no lock file yet" failure — the one
    /// outcome every reader treats as normal rather than as a fault.
    ///
    /// Everything else (oversize, corrupt, unreadable, version-gated) is a
    /// real failure that must reach the user; readers that degrade to
    /// "nothing is installed" have to tell the two apart.
    pub fn is_not_found(&self) -> bool {
        matches!(&self.kind, LockErrorKind::Io(io) if io.kind() == std::io::ErrorKind::NotFound)
    }
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(recovery) = self.recovery {
            write!(f, "{recovery}: ")?;
        }
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

    /// The lock file exceeds [`crate::config::FILE_SIZE_LIMIT_BYTES`] —
    /// on read, or on write before the file is replaced.
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

    /// The file is the other lock flavor: a `grimoire.lock` carrying
    /// plugin scope, or a `marketplace.lock` entry outside a declared
    /// plugin (or with a name unsafe to materialize; the `grimoire.lock`
    /// form of that refusal is [`Self::InvalidEntryName`]).
    #[error("{message}")]
    ScopeMismatch { message: String },

    /// A `grimoire.lock` entry name is unsafe as an install path segment.
    /// Apart from [`Self::ScopeMismatch`] because a relock reads a
    /// plugin-scoped file as absent, and reading this one as absent would
    /// move every pin.
    #[error("{message}")]
    InvalidEntryName { message: String },

    /// The lock parses but its dependency graph cannot be followed offline
    /// (a missing snapshot, or a cycle). `message` says which and names the
    /// fix.
    #[error("{message}")]
    UnfollowableGraph { message: String },
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
