// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The `grim_fetch` tool: a thin MCP adapter over the neutral fetch core
//! ([`crate::fetch`]).
//!
//! It resolves the per-call scope + access seam from the MCP argument trio
//! (`global`/`config`/`workspace`) and delegates to
//! [`crate::fetch::fetch_with_limit`] with the 256 KiB tool-result document
//! cap. All content-shaping, size gating, and truncation live in the core;
//! this file only bridges the MCP argument type to it.

use crate::context::Context;

use super::tool_args::FetchToolArgs;

/// Run the `grim_fetch` tool: resolve scope + access, then fetch and shape
/// the artifact content. Documents cap at
/// [`crate::fetch::FETCH_DOC_SIZE_LIMIT`] (a truncated doc is still useful
/// in a tool result).
///
/// # Errors
///
/// See [`crate::fetch::fetch_with_limit`].
pub async fn fetch(ctx: &Context, args: &FetchToolArgs) -> anyhow::Result<crate::fetch::FetchReport> {
    let scope = crate::command::resolve_fetch_scope(
        ctx,
        args.scope.global(),
        args.scope.config.as_deref(),
        args.scope.workspace.as_deref(),
    );
    let access = crate::command::access_seam(ctx)?;
    crate::fetch::fetch_with_limit(
        &scope,
        &access,
        &args.reference,
        args.vendor.as_deref(),
        args.path.as_deref(),
        crate::fetch::FETCH_DOC_SIZE_LIMIT,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_args_ref_rename_and_unknown_key_tolerance() {
        let args: FetchToolArgs =
            serde_json::from_str(r#"{"ref": "skills/x", "vendor": "claude", "ignored": true}"#).unwrap();
        assert_eq!(args.reference, "skills/x");
        assert_eq!(args.vendor.as_deref(), Some("claude"));
        assert!(args.path.is_none());
    }
}
