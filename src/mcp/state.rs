// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Shared state for the `grim mcp` server.
//!
//! Built once at server start and shared (behind an `Arc`) across every
//! concurrent tool call. The install **scope** is *not* part of this state:
//! each tool call carries its own optional scope parameters (`global` /
//! `config` / `workspace`) and resolves a fresh scope per call, so one
//! server instance can answer questions about any scope. Only
//! `--allow-writes` stays launch-pinned — enabling mutation is a trust
//! decision of whoever wires the server, not of the model. See
//! `adr_mcp_percall_scope_fetch_render.md`.

use crate::context::Context;

/// Server-wide state shared by all tool handlers.
pub struct McpState {
    /// The per-invocation context (env-derived paths, registry flag/env,
    /// offline). Cheap to clone; the tools reuse it for every command.
    pub ctx: Context,
    /// Whether mutating tools are enabled. When `false` the write tools
    /// (`grim_render`) are neither advertised nor callable.
    ///
    /// The actual gate is [`crate::mcp::server::build_router`] (a
    /// `disable_route` baked into the router at construction); this field
    /// is the launch-pinned trust decision's canonical home per
    /// `adr_mcp_percall_scope_fetch_render.md`, kept for a future tool
    /// handler that needs to read it directly rather than re-deriving it.
    #[allow(
        dead_code,
        reason = "documents the ADR's launch-pinned trust boundary; no direct reader yet"
    )]
    pub allow_writes: bool,
}
