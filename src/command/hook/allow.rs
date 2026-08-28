// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim hook allow` — consent to the hooks this checkout declares.
//!
//! ## The gesture that was missing
//!
//! Consent already existed as a *state*; what did not exist was a way to **ask
//! for it**. Under registry-scoped trust the only routes were an interactive
//! prompt and a hand-edited `trust_hooks` line, so consent either appeared from
//! a question the user had to be present for, or from a config key most users
//! never found. This is the explicit gesture, and
//! [`super::revoke`] is its undo.
//!
//! ## What it records, and how that differs from `grim add`
//!
//! This records the **whole declared hook set** for the resolved workspace,
//! replacing whatever the record held. That is correct precisely because it is
//! the reviewing gesture: the user is shown the workspace and answers for all of
//! it. `grim add` unions instead, recording only what the added reference
//! brought in — see [`crate::command::hook_consent`] for why that asymmetry is
//! the T3 control rather than an inconsistency.
//!
//! ## Global scope is refused, and that is not a failure to write
//!
//! `$GRIM_HOME/grimoire.toml` is the user's own file on the user's own machine.
//! There is no third party's checkout to gate, so it is permanently consented
//! and carries no record; [`crate::hook::consent::record`] refuses to write one.
//! Reporting that as success would claim a record that does not exist, so it is
//! a **usage error** (64) naming the reason — the same shape `ocx shell allow`
//! uses for its own always-consented tier.

use std::path::PathBuf;

use clap::Args;

use crate::api::hook_report::{HookConsentAction, HookConsentReport};
use crate::cli::exit_code::ExitCode;
use crate::command::hook_consent;
use crate::command::scope_resolution;
use crate::context::Context;
use crate::hook::consent::{self, Recorded};
use crate::install::hook_dispatch;
use crate::lock::lock_io;

/// `grim hook allow` arguments.
#[derive(Debug, Args)]
pub struct AllowArgs {
    /// The directory whose workspace to consent to (default: the current one)
    ///
    /// Resolved by the same walk-up every other command uses, so this consents
    /// to exactly the workspace an install would arm. `--global` and `--config`
    /// still take precedence, as everywhere.
    pub path: Option<PathBuf>,
}

/// Record consent for the resolved workspace over every hook it declares.
///
/// # Errors
///
/// A scope that cannot be resolved, a configuration or lock file that does not
/// parse (the ordinary report-command failures), the record's own write
/// failure, or **global scope** — which is a usage error (64), not a write
/// failure.
pub async fn run(ctx: &Context, args: &AllowArgs) -> anyhow::Result<(HookConsentReport, ExitCode)> {
    let scope = crate::command::grim(scope_resolution::resolve_in(
        ctx,
        ctx.global(),
        ctx.config(),
        args.path.as_deref(),
    ))?;

    // Same tolerance `grim status` and `grim hook list` have: an absent lock
    // means nothing is pinned yet, and consenting to an empty set is a legitimate
    // (if inert) answer. A corrupt lock is a load failure (78) and propagates —
    // consenting over a set grim could not read would record a lie.
    let lock = match lock_io::load(&scope.lock_path) {
        Ok(l) => Some(l),
        Err(e) if e.is_not_found() => None,
        Err(e) => return Err(crate::error::Error::from(e).into()),
    };
    let hooks = lock.as_ref().map(hook_consent::declared_hooks).unwrap_or_default();

    // Replace, not union: this is the reviewing gesture, so the set the user was
    // shown is the set that lands. A hook that dropped out of the declaration
    // should not linger in the record as a pre-approval for its own return.
    match consent::record(ctx.grim_home(), scope.scope, &scope.workspace, &hooks)? {
        Recorded::Stamped => Ok((
            HookConsentReport::new(
                scope.workspace.clone(),
                HookConsentAction::Consented,
                hooks.into_iter().collect(),
                Some(hook_dispatch::consent_path(ctx.grim_home(), &scope.workspace)),
            ),
            ExitCode::Success,
        )),
        // Not a write failure: the global toolchain is always consented, so
        // declining to record it is the correct outcome. Saying so beats
        // reporting a record that was never written.
        Recorded::GlobalNeedsNoRecord => Err(crate::command::hook_consent_usage(
            "the global scope is always consented for hooks and carries no consent record; \
             run `grim hook allow` inside a workspace instead",
        )),
    }
}
