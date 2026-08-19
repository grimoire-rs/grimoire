// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim hook` — the dispatcher runtime (`run`) and the user-facing
//! inventory (`list`).
//!
//! Two subcommands with almost nothing in common, and the split is the
//! contract rather than tidiness:
//!
//! - [`run`] is **not intended for direct invocation**. Its caller is the
//!   generated launcher (`src/install/hook_launcher.rs`), it sits on the hot
//!   path of every tool call on every armed client, and it is the only
//!   command in grim that **can never fail**: its return type is
//!   [`ExitCode`](crate::cli::exit_code::ExitCode), not `Result`, because
//!   invariant **I3** makes "the feature is off" the single permitted degrade
//!   direction. Documented in the shape of the `grim mcp` row — present in
//!   the surface, driven by grim's own generated caller.
//! - [`list`] is an ordinary report command (S-015): `--format json`,
//!   `Printable`, scope resolution, the lot.
//!
//! ## Why the runtime lives in its own module and imports almost nothing
//!
//! **C-007 — the dispatch table is the runtime's sole input, and that is a
//! *structural* property, not a runtime check.** The runtime resolves no
//! scope, reads no config, and never asks the environment where anything
//! is: every path it touches was resolved at install time and baked into the
//! launcher argv. There is deliberately **no injection seam** for this. A
//! production `ScopeResolver` trait with one real implementor would add
//! indirection to the hot path to prove a compile-time truth and would
//! *weaken* the guarantee — a seam can be called, an absent import cannot.
//!
//! So the property is pinned the way this repo already pins call-site rules:
//! a source-level test (`the_runtime_imports_no_scope_no_config_no_data_root_c007`
//! below), `include_str!` over each runtime file, in the same idiom as
//! `command.rs`'s `the_global_config_is_loaded_from_exactly_one_seam_ws2`
//! and `tui.rs`'s `tui_run_propagates_every_registry_resolution_t4`.
//! Four forbidden symbols, and the fourth is the one WP-P0 added:
//!
//! | Forbidden in the runtime | Why |
//! |---|---|
//! | `crate::config` | resolving config *is* scope resolution (C-007) |
//! | `scope_resolution` | ditto, by its own name |
//! | `crate::context` | the per-invocation context — the carrier of both, and of the data root |
//! | the `env` data-root accessor | it returns its environment value **verbatim** — no absoluteness check, and a *relative* `.grimoire` when `HOME` is unset. For a client-spawned `grim hook run` the process CWD **is the workspace**, so a repo that ships `.envrc` / `.mise.toml` / devcontainer `containerEnv` would choose the dispatch table (audit finding **B1** · T3, escalating T4 · I1, I4 · CWE-426) |
//!
//! [`list`] is exempt and imports all of them: it is a report command whose
//! whole job is to describe the resolved scope. That is why the two live in
//! separate files. The test reads **code lines only** — comment lines are
//! stripped first, so this file and the runtime's own doc comments can name
//! the forbidden symbols in order to explain them.
//!
//! ## The entire launcher argv is untrusted (B3)
//!
//! Any local file can invoke `grim hook run` — a hostile repository can
//! commit its **own** client registration naming the victim's real launcher.
//! So no argv value is ever used as a path, as a trust input, or as anything
//! but a **lookup key**, and the one argv value that *is* a path
//! (`--table`) is refused unless it is already absolute. The refusal code is
//! **0**, never an error: some clients fail *closed* on a non-zero hook
//! exit, so an error here would let a malformed invocation deny the user's
//! own tool call. See [`argv`].

pub mod argv;
pub mod envelope;
pub mod list;
pub mod pipeline;
pub mod projector;
pub mod run;

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
    /// Dispatch armed hooks for one client event (invoked by the generated
    /// launcher, not by hand).
    ///
    /// Reads only the dispatch table named by `--table`; resolves no scope
    /// and no configuration. Exits **0** in every case grim controls — a
    /// client that fails closed on a non-zero hook exit must never be
    /// denied a tool call by grim's own internals.
    Run(run::RunArgs),
    /// List declared hooks with their tiers, events, and per-client arming
    /// state.
    List(list::ListArgs),
}

#[cfg(test)]
mod tests {
    /// Every source file that is part of the runtime, relative to this file.
    ///
    /// An explicit list rather than "everything under `hook/` except
    /// `list.rs`": adding a runtime module and forgetting to widen an
    /// exclusion is silent, whereas adding one and forgetting to add it here
    /// is caught by `every_declared_runtime_module_is_checked` below.
    const RUNTIME_SOURCES: &[(&str, &str)] = &[
        ("hook/argv.rs", include_str!("hook/argv.rs")),
        ("hook/envelope.rs", include_str!("hook/envelope.rs")),
        ("hook/pipeline.rs", include_str!("hook/pipeline.rs")),
        ("hook/projector.rs", include_str!("hook/projector.rs")),
        ("hook/run.rs", include_str!("hook/run.rs")),
    ];

    /// Every runtime module this file declares, and whether it is part of the
    /// runtime. `list` is the one deliberate exemption.
    const DECLARED_MODULES: &[&str] = &["argv", "envelope", "list", "pipeline", "projector", "run"];

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
            "grim_home(",
            "the env data-root accessor returns its environment value verbatim, with a relative \
             fallback, and the CWD of a client-spawned `grim hook run` is the workspace \
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
        for (file, source) in RUNTIME_SOURCES {
            let body = code_lines(source);
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
        for (file, source) in RUNTIME_SOURCES {
            let body = code_lines(source);
            for (needle, why) in HASHING {
                assert!(
                    !body.contains(needle),
                    "{file} names `{needle}`: {why}. The pinned digest travels from the dispatch \
                     table into the audit record verbatim and is never recomputed"
                );
            }
        }
        let list = code_lines(include_str!("hook/list.rs"));
        for (needle, why) in HASHING {
            assert!(!list.contains(needle), "hook/list.rs names `{needle}`: {why}");
        }
    }

    /// The runtime list must cover every module this file declares.
    ///
    /// Without this, adding `pub mod spawn;` and forgetting to list it in
    /// `RUNTIME_SOURCES` would leave the new file unchecked and the test
    /// still green — which is how a structural guard quietly stops guarding.
    #[test]
    fn every_declared_runtime_module_is_checked() {
        let root = code_lines(include_str!("hook.rs"));
        for module in DECLARED_MODULES {
            assert!(
                root.contains(&format!("pub mod {module};")),
                "`{module}` is listed here but no longer declared in hook.rs"
            );
        }
        assert_eq!(
            root.matches("pub mod ").count(),
            DECLARED_MODULES.len(),
            "a module was added to `hook.rs` without deciding whether it is part of the \
             runtime; add it to DECLARED_MODULES and to RUNTIME_SOURCES, or state here why \
             it is exempt as `list` is"
        );
        assert_eq!(
            RUNTIME_SOURCES.len() + 1,
            DECLARED_MODULES.len(),
            "`list` is the only intended non-runtime module; a second exemption needs its \
             own recorded reason"
        );
    }

    /// **The runtime that runs the dispatch path is chosen from the parsed
    /// command, and `main.rs` has exactly one multi-thread constructor.**
    ///
    /// The behavioural pin lives in `main.rs` (`the_hook_runtime_runs_on_a_single_worker_f1`
    /// asserts `num_workers() == 1`); this is the structural half, in the same
    /// source-level idiom as the C-007 and C-009 tests above, and it guards the
    /// regression those two cannot see: someone adds a second
    /// `tokio::runtime::Runtime::new()` — or moves the construction back above
    /// the parse — and `grim hook run` silently starts one worker thread per
    /// logical CPU again on every tool call (F-1: 24 `clone3`, ≈1.9 ms of a
    /// 3.20 ms floor, worse on a bigger machine).
    #[test]
    fn the_hook_runtime_is_not_built_on_the_multi_thread_scheduler_f1() {
        let main = code_lines(include_str!("../main.rs"));
        assert!(
            main.contains("build_runtime(runtime_flavor("),
            "main.rs must build its runtime from `runtime_flavor(<the parsed command>)`, so the \
             scheduler `grim hook run` gets is decided by what was parsed rather than fixed"
        );
        assert!(
            main.contains("new_current_thread"),
            "the current-thread scheduler is what makes the guard path cheap; without this \
             constructor main.rs cannot be giving the hook runtime one"
        );
        assert_eq!(
            main.matches("tokio::runtime::Runtime::new()").count(),
            1,
            "exactly one multi-thread constructor is expected, inside `build_runtime`'s \
             MultiThread arm — a second one is a path that bypasses the flavor decision"
        );
    }

    /// **The runtime arm must be dispatched before `Context` is built.**
    ///
    /// `app::run` builds one per-invocation context for every command, and
    /// its constructor reads the environment data root unconditionally
    /// (`src/context.rs:169`). Without an early arm, "`grim hook run` never
    /// reads the environment data root" would be **false for the process**
    /// even while true for this module — the value would simply never be
    /// *used*. The plan states the stronger claim, so `app.rs` dispatches the
    /// runtime before the context exists and this pins the ordering.
    #[test]
    fn app_dispatches_the_runtime_before_it_builds_a_context_b1() {
        let app = code_lines(include_str!("../app.rs"));
        let run_arm = app
            .find("hook::run::run(")
            .expect("app.rs must dispatch the hook runtime");
        let context = app
            .find("Context::new(")
            .expect("app.rs must build a per-invocation context");
        assert!(
            run_arm < context,
            "the `grim hook run` arm must be dispatched BEFORE the context is built, because \
             its constructor reads the environment data root unconditionally; otherwise the \
             process reads an attacker-choosable value on every tool call (B1)"
        );
    }
}
