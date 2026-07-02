// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The `grim mcp` STDIO server: an rmcp [`ServerHandler`] exposing
//! Grimoire's catalog and install state as MCP tools.
//!
//! Read tools (`grim_search`, `grim_status`) are always available; they wrap
//! the existing `command::*::run` seams and serialize the same report the CLI
//! emits under `--format json`, so the MCP payload and the CLI JSON are one
//! source of truth. The install scope is a per-tool-call parameter (flattened
//! `ScopeToolArgs`), resolved fresh on every call — see
//! `adr_mcp_percall_scope_fetch_render.md`. Mutating tools are gated behind
//! `--allow-writes`.
//!
//! The server runs over stdio: stdout is the JSON-RPC channel, so the handlers
//! never print to it — all diagnostics go through `tracing` (stderr). The
//! service shuts down cleanly when the client closes stdin (EOF).

use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{ErrorData, ServerHandler, ServiceExt, tool, tool_handler, tool_router};

use crate::cli::exit_code::ExitCode;
use crate::command::mcp::McpArgs;
use crate::context::Context;
use crate::mcp::state::McpState;
use crate::mcp::tool_args::{FetchToolArgs, SearchToolArgs, StatusToolArgs};

/// The MCP server handler. Cloned per request by rmcp (a cheap `Arc` bump).
#[derive(Clone)]
pub struct GrimMcpServer {
    inner: Arc<McpState>,
}

#[tool_router]
impl GrimMcpServer {
    /// Browse the configured registries' catalog, filtered by an optional
    /// query and annotated with each repository's install status. Returns the
    /// same JSON payload as `grim search --format json`.
    #[tool(
        description = "Search the configured Grimoire registries for installable skills, rules, agents, and bundles. Returns a JSON array of matches with kind, repo, summary, version, and install status."
    )]
    async fn grim_search(&self, Parameters(args): Parameters<SearchToolArgs>) -> Result<String, ErrorData> {
        let search_args = crate::command::search::SearchArgs {
            query: args.query,
            refresh: args.refresh.unwrap_or(false),
            // Locked to the resolved scope's configured registry set — the tool
            // exposes no registry override (SSRF / CWE-918; see `SearchToolArgs`).
            registry: Vec::new(),
            global: args.scope.global(),
            config: args.scope.config,
            workspace: args.scope.workspace,
        };
        match crate::command::search::run(&self.inner.ctx, &search_args).await {
            Ok((report, _)) => to_json(&report),
            Err(e) => Err(tool_error("search", &e)),
        }
    }

    /// Report the install status of every declared artifact in the requested
    /// scope. Returns the same JSON payload as `grim status --format json`.
    #[tool(
        description = "Show the install status of every artifact declared in a Grimoire scope (installed / outdated / modified / not-installed). Scope is per call: `global`, `config`, or `workspace` (default: project discovered from the server's working directory). Returns a JSON array."
    )]
    async fn grim_status(&self, Parameters(args): Parameters<StatusToolArgs>) -> Result<String, ErrorData> {
        let status_args = crate::command::status::StatusArgs {
            global: args.scope.global(),
            config: args.scope.config,
            workspace: args.scope.workspace,
        };
        match crate::command::status::run(&self.inner.ctx, &status_args).await {
            Ok((report, _)) => to_json(&report),
            Err(e) => Err(tool_error("status", &e)),
        }
    }

    /// Fetch an artifact's content into the tool result — no install, no
    /// state, no harness reload (use ≠ install, see
    /// `adr_mcp_percall_scope_fetch_render.md`).
    #[tool(
        description = "Fetch a Grimoire artifact's content directly into the tool result — no install needed. Returns the canonical as-authored document unless `vendor` (claude/opencode/copilot) selects a client projection; `path` fetches one support file; a `files` listing is always included. Requires network access; content is capped at 256 KiB (truncated content is marked). Scope params (`global`/`config`/`workspace`) only select which registries are consulted."
    )]
    async fn grim_fetch(&self, Parameters(args): Parameters<FetchToolArgs>) -> Result<String, ErrorData> {
        match crate::mcp::fetch::fetch(&self.inner.ctx, &args).await {
            Ok(report) => to_json(&report),
            Err(e) => Err(tool_error("fetch", &e)),
        }
    }
}

#[tool_handler]
impl ServerHandler for GrimMcpServer {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        // `ServerInfo` / `Implementation` are `#[non_exhaustive]` (cannot be
        // struct-literal'd from outside rmcp); start from the default and set
        // the fields we own.
        let mut info = rmcp::model::ServerInfo::default();
        info.server_info.name = "grim".to_string();
        info.server_info.version = env!("CARGO_PKG_VERSION").to_string();
        info.instructions = Some(
            "Grimoire MCP server: browse and inspect OCI-distributed AI-agent configuration \
             (skills, rules, agents, bundles). Scope is chosen per tool call via the optional \
             `global` / `config` / `workspace` parameters (default: the project discovered \
             from the server's working directory). Read-only unless started with --allow-writes."
                .to_string(),
        );
        info
    }
}

/// Serialize a report to a JSON string, mapping a serialization failure to an
/// MCP error rather than panicking (no `.unwrap()` on the protocol path).
fn to_json<T: serde::Serialize>(report: &T) -> Result<String, ErrorData> {
    serde_json::to_string(report).map_err(|e| ErrorData::internal_error(format!("serialize: {e}"), None))
}

/// Map a command error chain to an MCP tool error, preserving the full
/// `{:#}` chain in the message (stderr-style, lowercase library wording).
fn tool_error(op: &str, err: &anyhow::Error) -> ErrorData {
    ErrorData::internal_error(format!("{op} failed: {err:#}"), None)
}

/// Run the MCP server over stdio until the client disconnects (stdin EOF).
///
/// # Errors
///
/// A transport setup failure. A clean client disconnect returns
/// `Ok(ExitCode::Success)`.
pub async fn serve(ctx: &Context, args: &McpArgs) -> anyhow::Result<ExitCode> {
    let state = McpState {
        ctx: ctx.clone(),
        allow_writes: args.allow_writes,
    };
    let server = GrimMcpServer { inner: Arc::new(state) };
    tracing::info!(allow_writes = args.allow_writes, "grim mcp server starting on stdio");
    let service = server.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    tracing::info!("grim mcp server stopped (client disconnected)");
    Ok(ExitCode::Success)
}
