// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim mcp` — run a local STDIO Model Context Protocol server.
//!
//! Diverges into a long-running server loop rather than emitting a structured
//! report, so (per `subsystem-cli-api.md` "Commands That Exec a Child
//! Process") it is exempt from the `Printable` / `api/` path and returns an
//! [`ExitCode`] directly — the same exemption `tui` and `schema` use. The
//! server exposes Grimoire's catalog/status as MCP tools; mutating tools are
//! gated behind `--allow-writes` (read-only by default). The install scope is
//! a per-tool-call parameter (`global` / `config` / `workspace`), not a launch
//! flag — see `adr_mcp_percall_scope_fetch_render.md`.

use clap::Args;

use crate::cli::exit_code::ExitCode;
use crate::context::Context;

/// `grim mcp` arguments.
#[derive(Debug, Args)]
pub struct McpArgs {
    /// Enable mutating tools (currently `grim_render`). Off by default: the
    /// server is read-only unless this is set. Launch-pinned deliberately —
    /// enabling writes is a trust decision of whoever wires the server into
    /// a harness, never of the model calling the tools.
    #[arg(long)]
    pub allow_writes: bool,
}

/// Run `grim mcp`. Returns when the client closes stdin (EOF).
///
/// # Errors
///
/// A transport setup failure, or an error building the server. A clean client
/// disconnect exits `Success`.
pub async fn run(ctx: &Context, args: &McpArgs) -> anyhow::Result<ExitCode> {
    crate::mcp::server::serve(ctx, args).await
}
