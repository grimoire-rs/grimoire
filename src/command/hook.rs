// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim hook` — the user-facing inventory (`list`) and the workspace consent
//! commands (`allow`, `revoke`).
//!
//! The dispatcher runtime is not a `grim` verb: it is the separate `grim-hook`
//! launcher binary (`crates/grim-hook`), built on `grimoire-hook-core`, which a
//! client's registration runs directly. This module keeps only what is
//! user-driven and scope-resolving.
//!
//! The source-level tests below hold every file of the two runtime crates
//! (`grimoire-hook-core`, `grim-hook`) to two bans, because a seam can be called
//! and an absent import cannot:
//!
//! - **C-007**: no config, scope resolution, `Context`, telemetry handle or
//!   environment data-root accessor. The runtime's only input is the table beside
//!   the launcher, and a client-spawned process runs with the workspace as its
//!   CWD, so an attacker-choosable data root would make the table repo-plantable
//!   (B1, T3, CWE-426).
//! - **C-009**: no digest primitive. `resolved_digest` is provenance carried into
//!   the audit record, never recomputed on the hot path (N2 is a non-goal).
//!
//! `list`, `allow` and `revoke` are exempt from the first: describing or
//! recording against the resolved scope is their whole job. The tests read code
//! lines only, so doc comments can name the forbidden symbols to explain them.

pub mod allow;
pub mod list;
pub mod projector;
pub mod revoke;
#[cfg(test)]
mod std_parity;

use clap::{Args, Subcommand};

/// `grim hook` arguments.
#[derive(Debug, Args)]
pub struct HookArgs {
    /// Which hook subcommand to run.
    #[command(subcommand)]
    pub command: HookCommand,
}

/// The `hook` subcommand tree.
#[derive(Debug, Subcommand)]
pub enum HookCommand {
    /// List declared hooks with their tiers, events, and per-client arming
    /// state.
    List(list::ListArgs),
    /// Consent to arming the hooks this workspace declares.
    Allow(allow::AllowArgs),
    /// Withdraw this workspace's hook consent.
    ///
    /// Aliased `deny`, because that is the name `direnv` gives the identical
    /// operation — there, `revoke` is itself an alias of `deny`, and both mean
    /// "remove the authorization". A user arriving from the tool this design
    /// cites should not have to learn which of the two words grim picked.
    ///
    /// The alias does **not** introduce a durable "no". There are three consent
    /// states, not four: absent, granted, drifted. Denying and never having
    /// answered are the same state on purpose — the default is already deny, so
    /// a recorded refusal would buy silence rather than safety, and silently
    /// withholding with no reported cause is the one direction the arming
    /// report never takes.
    #[command(alias = "deny")]
    Revoke(revoke::RevokeArgs),
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// The runtime crates' source roots, relative to the repository root.
    const RUNTIME_DIRS: &[&str] = &["crates/grimoire-hook-core/src", "crates/grim-hook/src"];

    fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    /// Every `.rs` file of the runtime crates, read at test time so a new file is
    /// scanned without anyone listing it.
    fn runtime_sources() -> Vec<(String, String)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        for dir in RUNTIME_DIRS {
            collect(&root.join(dir), &mut files);
        }
        files
            .into_iter()
            .map(|path| {
                let source = std::fs::read_to_string(&path).unwrap();
                (path.strip_prefix(root).unwrap().display().to_string(), source)
            })
            .collect()
    }

    /// Symbols the runtime may not name, and the reason each one is a
    /// runtime input the design forbids.
    ///
    /// The data-root accessor is spelled as a **call** (`grim_home()`) rather
    /// than as a bare name so the assertion cannot be satisfied by renaming a
    /// local variable, and cannot be tripped by a module path in a `use` that
    /// does not call it.
    const FORBIDDEN: &[(&str, &str)] = &[
        ("crate::config", "resolving configuration is scope resolution (C-007)"),
        ("scope_resolution", "scope resolution by name (C-007)"),
        (
            "crate::context",
            "the per-invocation context is the carrier of config, scope and the data root (C-007). \
             Spelled as the module path rather than the bare type name, so a vendor field literal \
             such as `additionalContext` cannot trip it",
        ),
        (
            "crate::telemetry",
            "the telemetry handle rides the per-invocation context, which the runtime never builds, \
             and the hook runtime must never export (hook path untouched)",
        ),
        (
            "Telemetry",
            "the telemetry handle type — naming it on the dispatch path means a record or an export \
             can happen per tool call",
        ),
        (
            "grim_home(",
            "the env data-root accessor returns its environment value verbatim, with a relative \
             fallback, and the CWD of a client-spawned hook process is the workspace \
             (B1, T3, CWE-426)",
        ),
    ];

    /// Code lines only: comment lines stripped, and everything from the first
    /// `#[cfg(test)]` on dropped as test code.
    ///
    /// Both halves matter. The test split keeps a test's own copy of a needle
    /// out of the scan; the comment strip is what lets the runtime's doc
    /// comments *explain* the forbidden symbols, which is the difference
    /// between a documented invariant and an unexplained one.
    fn code_lines(source: &str) -> String {
        let production = source.split_once("#[cfg(test)]").map_or(source, |(before, _)| before);
        production
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// **C-007 + WP-P0 B1, as a structural fact.**
    ///
    /// The runtime must not import or name config, scope resolution,
    /// `Context`, or the environment data-root accessor. This is a
    /// source-level test on purpose (see the module doc): a seam can be
    /// called, an absent import cannot, and there is nothing to inject on
    /// the hot path.
    #[test]
    fn the_runtime_imports_no_scope_no_config_no_data_root_c007() {
        for (file, source) in runtime_sources() {
            let body = code_lines(&source);
            for (needle, why) in FORBIDDEN {
                assert!(
                    !body.contains(needle),
                    "{file} names `{needle}`, which the hook runtime may not: {why}. \
                     If this is genuinely needed, it belongs in `hook/list.rs` (a report \
                     command) or at install time — never on the dispatch path"
                );
            }
        }
    }

    /// Symbols that would mean the runtime is hashing something, and the reason
    /// each one is forbidden on the dispatch path.
    ///
    /// **C-009 — the runtime hashes NOTHING.** Integrity is pinned at
    /// resolution; `DispatchEntry::resolved_digest` is provenance carried into
    /// the audit record, never a gate. Owner decision A3 deleted the exec-time
    /// re-check, so re-adding one would put a hash on the hot path of every tool
    /// call in order to defend against **N2** — a machine already compromised at
    /// grim's own privilege, which is an explicit non-goal.
    ///
    /// This is the same source-level idiom as the C-007 test above, and for the
    /// same reason: the regression direction is *someone adds a digest check*,
    /// and an absent symbol is the only form of that assertion a behavioural
    /// test cannot weaken. `hook/list.rs` is **not** exempt here — a report
    /// command has no business hashing on this path either.
    const HASHING: &[(&str, &str)] = &[
        (
            "crate::store::hash",
            "the content-hash module — reaching for it on the dispatch path is an exec-time \
             integrity re-check, which decision A3 deleted (C-009)",
        ),
        ("Algorithm::", "the hash-algorithm selector (C-009)"),
        ("Sha256", "a digest primitive (C-009)"),
        (
            ".hash(",
            "a hashing call. `resolved_digest` is provenance for the audit record; computing one \
             here re-adds the hot-path cost A3 removed (C-009 · N2)",
        ),
    ];

    /// **C-009, as a structural fact.** No runtime file computes a digest.
    #[test]
    fn the_runtime_computes_no_digest_c009() {
        for (file, source) in runtime_sources() {
            let body = code_lines(&source);
            for (needle, why) in HASHING {
                assert!(
                    !body.contains(needle),
                    "{file} names `{needle}`: {why}. The pinned digest travels from the dispatch \
                     table into the audit record verbatim and is never recomputed"
                );
            }
        }
        // The three exempt report commands are checked here explicitly: they
        // are outside `RUNTIME_SOURCES` because they resolve a scope, not
        // because they may hash. `allow`/`revoke` reach `workspace_key`
        // through `hook::consent`, which owns the one hashing seam.
        for (file, source) in [
            ("hook/list.rs", include_str!("hook/list.rs")),
            ("hook/allow.rs", include_str!("hook/allow.rs")),
            ("hook/revoke.rs", include_str!("hook/revoke.rs")),
        ] {
            let body = code_lines(source);
            for (needle, why) in HASHING {
                assert!(!body.contains(needle), "{file} names `{needle}`: {why}");
            }
        }
    }

    /// The scan reads the runtime crates themselves, so it must have found them:
    /// a moved directory would otherwise leave both bans checking nothing.
    #[test]
    fn the_scan_covers_the_runtime_crates() {
        let files: Vec<String> = runtime_sources().into_iter().map(|(file, _)| file).collect();
        for expected in [
            "crates/grim-hook/src/main.rs",
            "crates/grimoire-hook-core/src/run.rs",
            "crates/grimoire-hook-core/src/table.rs",
            "crates/grimoire-hook-core/src/pipeline.rs",
        ] {
            assert!(
                files.iter().any(|f| f.replace('\\', "/") == expected),
                "{expected} not scanned: {files:?}"
            );
        }
    }
}
