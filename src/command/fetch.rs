// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim fetch <ref>` — resolve + fetch + print artifact content without
//! installing.
//!
//! CLI port of the MCP `grim_fetch` tool, sharing the neutral fetch core
//! ([`crate::fetch::fetch_with_limit`]): canonical (as-authored)
//! content by default, a `--vendor` projection, or one `--path` support
//! file. Plain mode prints the raw content payload (pipe-able —
//! `grim fetch ref --path X > file`); `--format json` emits the full
//! fetch report.
//!
//! Unlike the MCP tool the CLI never truncates: the per-document cap is
//! the 8 MiB pre-download layer gate, so any layer that passes the gate
//! prints byte-complete. Non-fatal warnings go to stderr via `tracing` so
//! the stdout payload stays clean.

use clap::Args;

use crate::api::fetch_report::FetchCliReport;
use crate::cli::exit_code::ExitCode;
use crate::context::Context;
use crate::fetch::FETCH_BLOB_SIZE_LIMIT;

/// `grim fetch` arguments.
#[derive(Debug, Args)]
pub struct FetchArgs {
    /// The artifact reference: a short id (`skills/code-review`), an
    /// alias-qualified ref, or a fully qualified one. Defaults to `latest`
    /// when no tag/digest is given.
    pub reference: String,

    /// Print this client's projection (`claude` / `opencode` / `copilot`)
    /// instead of the canonical as-authored document.
    #[arg(long)]
    pub vendor: Option<String>,

    /// Print one support file by its tree path (see the JSON `files`
    /// listing) instead of the index document. UTF-8 text only.
    #[arg(long)]
    pub path: Option<String>,
}

/// Run `grim fetch`.
///
/// # Errors
///
/// Reference/resolution/transport failures (their own exit taxonomy), an
/// unknown `--vendor`, a `--vendor` on a bundle, a missing `--path`
/// entry, an oversize layer, or non-UTF-8 content.
pub async fn run(ctx: &Context, args: &FetchArgs) -> anyhow::Result<(FetchCliReport, ExitCode)> {
    let scope = crate::command::resolve_fetch_scope(ctx, ctx.global(), ctx.config(), None);
    let access = crate::command::access_seam(ctx)?;
    // The layer gate (8 MiB, pre-download) is the effective ceiling, so
    // with the same value as the doc cap truncation is unreachable and
    // the plain payload pipes byte-complete.
    let report = crate::fetch::fetch_with_limit(
        &scope,
        &access,
        &args.reference,
        args.vendor.as_deref(),
        args.path.as_deref(),
        FETCH_BLOB_SIZE_LIMIT as usize,
    )
    .await?;

    // Warnings ride stderr so stdout stays a pure payload.
    for warning in &report.warnings {
        tracing::warn!("{warning}");
    }

    Ok((FetchCliReport(report), ExitCode::Success))
}
