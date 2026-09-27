// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Export-owned failures (design record C-028).
//!
//! Flat enum rather than the context + kind shape of the other tiers: every
//! variant already carries the one piece of context it is about. Resolver,
//! access, lock and install failures never become an `ExportError` — they
//! propagate as [`crate::error::Error`] with their existing classification.

use std::io;
use std::path::PathBuf;

use crate::install::ClientTarget;
use crate::oci::ArtifactKind;

/// How many stale-reference hits a [`ExportError::RenameStaleReference`]
/// message lists before it summarizes the rest.
const STALE_HITS_SHOWN: usize = 50;

/// An export-owned failure. Exit codes per variant are the C-028 table,
/// assigned by `crate::error::classify` (not here).
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// A flag combination or input the command line cannot accept (64).
    #[error("{0}")]
    Usage(String),

    /// `marketplace.toml` could not be accepted: bad file name, unreadable,
    /// oversized, malformed, or a declaration that breaks a rule (65).
    #[error("{}: {message}", path.display())]
    Manifest { path: PathBuf, message: String },

    /// No plugin was selected and the manifest declares none (65).
    #[error(
        "{} declares no plugins; declare a [plugins.<name>] table, or export refs directly with <ref>… --name",
        path.display()
    )]
    NoneDeclared { path: PathBuf },

    /// `--version` is not a semver version without build metadata (65).
    #[error("invalid plugin version '{value}': expected a semver version without build metadata (e.g. 1.2.0)")]
    InvalidVersion { value: String },

    /// An author-written description (`--description` or a declared
    /// `description`) is longer than a plugin description can carry
    /// beside grim's on-ramp sentence (65).
    #[error(
        "plugin '{plugin}': description is {len} characters; at most {max} fit (plugin descriptions are capped at {cap}, the rest is grim's on-ramp sentence)",
        cap = super::family::MAX_DESCRIPTION_LEN
    )]
    DescriptionTooLong { plugin: String, len: usize, max: usize },

    /// A rename produced an empty or invalid member name (65).
    #[error("plugin '{plugin}': renaming '{from}' yields invalid name '{to}'")]
    RenameInvalid { plugin: String, from: String, to: String },

    /// Two members of one kind share an emitted name after rename (65).
    #[error("plugin '{plugin}': {kind} members '{}' and '{}' both rename to '{name}'", members[0], members[1])]
    RenameCollision {
        plugin: String,
        kind: ArtifactKind,
        name: String,
        members: [String; 2],
    },

    /// A renamed member is still referenced by its old name inside the
    /// staged plugin tree (65). `hits` are pre-formatted
    /// `<client>: <relpath>:<line>: '<old>'` lines, sorted.
    #[error(
        "plugin '{plugin}': renamed members are still referenced by their old names:\n{}",
        stale_hit_lines(hits)
    )]
    RenameStaleReference { plugin: String, hits: Vec<String> },

    /// Every member of the plugin was omitted for this client (65).
    #[error("plugin '{plugin}' has no member the '{client}' plugin format can carry")]
    EmptyPlugin { plugin: String, client: ClientTarget },

    /// A staged path or zip entry name would escape the plugin root (65).
    #[error("unsafe entry path '{}' in plugin output", path.display())]
    UnsafeEntry { path: PathBuf },

    /// An output path already exists and `--force` was not given (65,
    /// `untracked-destination`).
    #[error(
        "output already exists: {}; rerun with --force to replace",
        paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
    )]
    OutputExists { paths: Vec<PathBuf> },

    /// An explicitly requested client has no plugin format (78).
    #[error(
        "client '{client}' has no plugin format; clients with one: {}",
        super::family::plugin_client_names()
    )]
    NoPluginFormat { client: ClientTarget },

    /// Two includes of one plugin supply different artifacts under the same
    /// `(kind, name)` (78).
    #[error("plugin '{plugin}': {kind} '{name}' is supplied by both '{first}' and '{second}'")]
    MemberConflict {
        plugin: String,
        kind: ArtifactKind,
        name: String,
        first: String,
        second: String,
    },

    /// A `--plugin` name is not declared in the manifest (79).
    #[error("plugin '{name}' is not declared in the marketplace manifest")]
    PluginNotFound { name: String },

    /// An include resolved to no tag or manifest in the registry (79).
    /// `include` is the reference as written.
    #[error("plugin '{plugin}': include '{include}' was not found in the registry")]
    IncludeNotFound { plugin: String, include: String },

    /// An update selector names no plugin or member (79).
    #[error("selector '{selector}' matches nothing in the marketplace")]
    SelectorNotFound { selector: String },

    /// A filesystem operation of the export itself failed (74 / 77 via
    /// `classify_io`).
    #[error("I/O error for {}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// The first [`STALE_HITS_SHOWN`] hits, one per line, then a count of the
/// rest.
fn stale_hit_lines(hits: &[String]) -> String {
    let mut lines: Vec<&str> = hits.iter().take(STALE_HITS_SHOWN).map(String::as_str).collect();
    let rest = hits.len().saturating_sub(STALE_HITS_SHOWN);
    let more = format!("… and {rest} more");
    if rest > 0 {
        lines.push(&more);
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    //! C-028 message shapes.

    use super::*;

    fn hits(n: usize) -> Vec<String> {
        (0..n)
            .map(|i| format!("claude: skills/x/SKILL.md:{i}: 'old'"))
            .collect()
    }

    #[test]
    fn c028_output_exists_lists_every_path_and_force_hint() {
        let err = ExportError::OutputExists {
            paths: vec![PathBuf::from("/out/a"), PathBuf::from("/out/b.zip")],
        };
        assert_eq!(
            err.to_string(),
            "output already exists: /out/a, /out/b.zip; rerun with --force to replace"
        );
    }

    #[test]
    fn c028_stale_reference_lists_all_up_to_cap() {
        let msg = ExportError::RenameStaleReference {
            plugin: "team".into(),
            hits: hits(STALE_HITS_SHOWN),
        }
        .to_string();
        let mut lines = msg.lines();
        assert_eq!(
            lines.next(),
            Some("plugin 'team': renamed members are still referenced by their old names:")
        );
        let rest: Vec<&str> = lines.collect();
        assert_eq!(rest, hits(50));
        assert!(!msg.contains("more"), "{msg}");
    }

    #[test]
    fn c028_stale_reference_caps_at_50_then_counts_rest() {
        let msg = ExportError::RenameStaleReference {
            plugin: "team".into(),
            hits: hits(53),
        }
        .to_string();
        let lines: Vec<&str> = msg.lines().skip(1).collect();
        assert_eq!(lines.len(), 51);
        assert_eq!(lines[..50], hits(50));
        assert_eq!(lines[50], "… and 3 more");
        assert!(!msg.contains(&hits(53)[50]), "hit 51 is summarized, not shown");
    }

    #[test]
    fn c028_manifest_shows_path_once_then_message() {
        let err = ExportError::Manifest {
            path: PathBuf::from("/w/market.toml"),
            message: "empty include".into(),
        };
        assert_eq!(err.to_string(), "/w/market.toml: empty include");
    }
}
