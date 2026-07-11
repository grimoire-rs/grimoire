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

use std::path::PathBuf;

use clap::Args;

use crate::api::fetch_report::FetchCliReport;
use crate::cli::exit_code::ExitCode;
use crate::cli::options::OutputFormat;
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

    /// Fetch the repository description companion (README, logo, CHANGELOG)
    /// instead of the artifact. JSON returns every member inline; plain
    /// requires `--out`.
    #[arg(long)]
    pub description: bool,

    /// Resolve the reference to a digest and report `{ref, digest}` without
    /// downloading the manifest or any blob — a cheap cache probe. Composes
    /// with `--description` to probe the companion tag.
    #[arg(long)]
    pub digest_only: bool,

    /// Unpack the `--description` companion tree into this directory (created
    /// if absent). Only valid with `--description`.
    #[arg(long)]
    pub out: Option<PathBuf>,
}

/// Run `grim fetch`.
///
/// # Errors
///
/// A flag-combination usage error (64), reference/resolution/transport
/// failures (their own exit taxonomy), an unknown `--vendor`, a `--vendor`
/// on a bundle, a missing `--path` / companion, an oversize layer, or
/// non-UTF-8 content.
pub async fn run(ctx: &Context, args: &FetchArgs, format: OutputFormat) -> anyhow::Result<(FetchCliReport, ExitCode)> {
    // Flag-combination usage gates (exit 64) before any resolution.
    if args.out.is_some() && !args.description {
        return Err(crate::command::config_usage("--out is only valid with --description"));
    }
    if args.digest_only && (args.vendor.is_some() || args.path.is_some() || args.out.is_some()) {
        return Err(crate::command::config_usage(
            "--digest-only resolves a digest without downloading; it takes no --vendor, --path, or --out",
        ));
    }
    // The full companion bundle has no single plain payload. A `--path` member
    // does (it prints like any support file), so only the bundle is gated.
    if args.description && args.path.is_none() && args.out.is_none() && matches!(format, OutputFormat::Plain) {
        return Err(crate::command::config_usage(
            "--description prints a multi-file bundle with no single plain payload; pass --out <dir> or --format json",
        ));
    }

    let scope = crate::command::resolve_fetch_scope(ctx, ctx.global(), ctx.config(), None);
    let access = crate::command::access_seam(ctx)?;
    // The layer gate (8 MiB, pre-download) is the effective ceiling, so with
    // the same value as the doc cap, truncation is unreachable and the plain
    // payload pipes byte-complete.
    let outcome = crate::fetch::fetch_outcome(
        &scope,
        &access,
        &args.reference,
        args.vendor.as_deref(),
        args.path.as_deref(),
        args.description,
        args.digest_only,
        args.out.as_deref(),
        FETCH_BLOB_SIZE_LIMIT as usize,
    )
    .await?;

    // Warnings ride stderr so stdout stays a pure payload.
    for warning in outcome.warnings() {
        tracing::warn!("{warning}");
    }

    Ok((FetchCliReport(outcome), ExitCode::Success))
}
