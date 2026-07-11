// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim describe <ref>` — report an artifact's manifest-level metadata
//! (kind, curated annotations, tags) without downloading its content.
//!
//! CLI port of the MCP `grim_describe` tool, sharing the neutral describe
//! core ([`crate::fetch::describe_artifact`]): it resolves the reference,
//! lists the repository's tags, and reads the manifest annotations — no blob
//! is ever pulled. Plain mode prints a flat key/value table (like
//! `grim context`); `--format json` emits the full describe report.
//!
//! Non-fatal warnings (a degraded scope falling back to the flag/env/global
//! registry chain) go to stderr via `tracing` so the stdout payload stays
//! clean.

use clap::Args;

use crate::api::describe_report::DescribeCliReport;
use crate::cli::exit_code::ExitCode;
use crate::context::Context;

/// `grim describe` arguments.
#[derive(Debug, Args)]
pub struct DescribeArgs {
    /// The artifact reference: a short id (`skills/code-review`), an
    /// alias-qualified ref, or a fully qualified one. Defaults to `latest`
    /// when no tag/digest is given.
    pub reference: String,
}

/// Run `grim describe`.
///
/// # Errors
///
/// Reference/resolution/transport failures (their own exit taxonomy: offline
/// 81, auth 80, unreachable 69, …) or a missing tag / manifest.
pub async fn run(ctx: &Context, args: &DescribeArgs) -> anyhow::Result<(DescribeCliReport, ExitCode)> {
    let scope = crate::command::resolve_fetch_scope(ctx, ctx.global(), ctx.config(), None);
    // Degraded-scope warnings ride stderr so stdout stays a pure report.
    for warning in &scope.warnings {
        tracing::warn!("{warning}");
    }
    let access = crate::command::access_seam(ctx)?;
    let report = crate::fetch::describe_artifact(&scope, &access, &args.reference).await?;
    Ok((DescribeCliReport(report), ExitCode::Success))
}
