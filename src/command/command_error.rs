// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Command-tier precondition errors that do not belong to a single
//! subsystem.
//!
//! `grim install` / `grim update` enforce "a fresh lock must exist"
//! before doing any work. That precondition failure is neither a config
//! nor a lock *parse* failure — it is a workflow-state error with its own
//! exit-code mapping (missing lock ⇒ NotFound 79, stale lock ⇒ DataError
//! 65). A small dedicated error keeps the classifier exhaustive without
//! overloading the lock taxonomy.

/// A command-level precondition was not met.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CommandError {
    /// `install`/`update` requires a `grimoire.lock`, but none exists.
    #[error("no grimoire.lock found at {path}; run `grim lock` first")]
    LockMissing { path: std::path::PathBuf },

    /// The lock's declaration hash no longer matches the live config.
    #[error(
        "grimoire.lock is stale (declaration_hash {locked} does not match current {current}); run `grim lock` before installing"
    )]
    LockStale { locked: String, current: String },

    /// `login` / `logout` need a registry but none was given and no
    /// default is configured.
    #[error("no registry given; pass a registry argument or set GRIM_DEFAULT_REGISTRY")]
    NoLoginRegistry,

    /// `login` could not obtain a required credential input — typically a
    /// non-interactive shell missing `--username` / `--password-stdin`.
    #[error("{0}")]
    LoginInput(&'static str),

    /// `add` could not infer the artifact kind: the reference did not
    /// resolve to a manifest carrying a Grimoire OCI `artifactType` (a
    /// non-Grimoire image, or an offline cache miss). The user must pass
    /// `--kind`.
    #[error("could not infer the kind of '{reference}'; pass --kind skill|rule|bundle")]
    KindInferenceFailed { reference: String },

    /// `add` declared a `(kind, name)` that already exists in the config
    /// bound to a *different* identifier. The declared name is a true
    /// per-scope-unique key, so a silent overwrite would clobber the
    /// existing binding without the caller's awareness. Re-declaring the
    /// *same* identifier never reaches this variant — that path stays the
    /// pre-existing idempotent overwrite (exit 0). Exit 64: the same
    /// "conflicting invocation, fix and retry" contract as
    /// [`crate::config::config_error::ConfigErrorKind::ConfigAlreadyExists`].
    #[error(
        "{kind} '{name}' is already declared as {existing}; pass --name to declare '{requested}' under a different name"
    )]
    DeclareConflict {
        kind: crate::oci::ArtifactKind,
        name: String,
        existing: String,
        requested: String,
    },

    /// `add` received a binding name outside the artifact-name charset
    /// for a file-materializing kind (skill/rule/agent). The binding
    /// becomes the install directory / file name, so it must satisfy the
    /// Agent Skills name rules — in particular lowercase-only, which
    /// keeps bindings collision-free on case-insensitive filesystems.
    /// Exit 64.
    #[error("invalid {kind} binding name: {reason} (allowed: lowercase letters, digits, hyphens)")]
    InvalidBindingName {
        kind: crate::oci::ArtifactKind,
        reason: String,
    },

    /// `config` received an unknown dotted key, a duplicate alias, or
    /// another input that violates the command contract (exit 64).
    #[error("{0}")]
    ConfigUsage(String),

    /// `config set` received a value that is syntactically valid but
    /// semantically rejected (exit 65).
    #[error("{0}")]
    ConfigValue(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_actionable_and_lowercase_start() {
        let m = CommandError::LockMissing {
            path: std::path::PathBuf::from("/w/grimoire.lock"),
        };
        assert!(m.to_string().starts_with("no grimoire.lock"));
        assert!(m.to_string().contains("grim lock"));

        let s = CommandError::LockStale {
            locked: "sha256:aaa".to_string(),
            current: "sha256:bbb".to_string(),
        };
        assert!(s.to_string().contains("stale"));
        assert!(s.to_string().contains("grim lock"));
    }
}
