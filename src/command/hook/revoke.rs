// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim hook revoke` — withdraw this checkout's hook consent.
//!
//! The undo half of [`super::allow`]. Immediately effective in the sense that
//! matters: the record is read on every arming decision, so the next
//! `grim install` (or any other converging command) disarms — unless
//! `--trust-hooks` is passed on that run, which is N4 and deliberate.
//!
//! **It does not disarm anything by itself.** The dispatch table is written by
//! convergence, not by this command, and re-deriving it here would put a
//! machine-wide write on a command whose whole job is to delete one small file.
//! The report says so rather than implying an immediate teardown.
//!
//! **Idempotent by contract.** Revoking a workspace that was never consented is
//! exit 0 with a line saying so — the requested state is the state that already
//! held, and refusing there would put a failure on the most ordinary outcome of
//! a command whose whole job is to make sure a record is gone.

use std::path::PathBuf;

use clap::Args;

use crate::api::hook_report::{HookConsentAction, HookConsentReport};
use crate::cli::exit_code::ExitCode;
use crate::command::scope_resolution;
use crate::context::Context;
use crate::hook::consent::{self, Revoked};

/// `grim hook revoke` arguments.
#[derive(Debug, Args)]
pub struct RevokeArgs {
    /// The directory whose workspace to revoke (default: the current one)
    ///
    /// Resolved by the same walk-up `grim hook allow` uses, so the two commands
    /// always name the same workspace.
    pub path: Option<PathBuf>,
}

/// Remove the resolved workspace's consent record, if it has one.
///
/// # Errors
///
/// A scope that cannot be resolved, or the removal's own I/O failure. An absent
/// record is **not** an error.
///
/// Global scope needs no refusal here, unlike [`super::allow`]:
/// [`crate::hook::consent::record`] never writes a global record, so there is
/// nothing to remove and the answer is [`Revoked::Absent`] by construction
/// rather than by a second copy of that predicate.
pub async fn run(ctx: &Context, args: &RevokeArgs) -> anyhow::Result<(HookConsentReport, ExitCode)> {
    let scope = crate::command::grim(scope_resolution::resolve_in(
        ctx,
        ctx.global(),
        ctx.config(),
        args.path.as_deref(),
    ))?;

    // No lock read: revoking is about the record, not about what is declared.
    // A workspace whose lock no longer parses must still be revocable — that is
    // exactly the state a user reaches for this command in.
    let action = match consent::revoke(ctx.grim_home(), &scope.workspace)? {
        Revoked::Removed => HookConsentAction::Revoked,
        Revoked::Absent => HookConsentAction::NotConsented,
    };
    Ok((
        HookConsentReport::new(scope.workspace.clone(), action, Vec::new(), None),
        ExitCode::Success,
    ))
}
