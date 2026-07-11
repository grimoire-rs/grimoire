// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The `grim_describe` tool: a thin MCP adapter over the neutral describe
//! core ([`crate::fetch::describe_artifact`]).
//!
//! It resolves the per-call scope + access seam from the MCP argument trio
//! (`global`/`config`/`workspace`) and delegates to
//! [`crate::fetch::describe_artifact`], which reports manifest-level metadata
//! (kind, curated annotations, tags) without downloading the content layer.
//! The payload equals `grim describe --format json` modulo whitespace.

use crate::context::Context;

use super::tool_args::DescribeToolArgs;

/// Run the `grim_describe` tool: resolve scope + access, then read the
/// artifact's manifest-level metadata.
///
/// # Errors
///
/// See [`crate::fetch::describe_artifact`].
pub async fn describe(ctx: &Context, args: &DescribeToolArgs) -> anyhow::Result<crate::fetch::DescribeReport> {
    let scope = crate::command::resolve_fetch_scope(
        ctx,
        args.scope.global(),
        args.scope.config.as_deref(),
        args.scope.workspace.as_deref(),
    );
    let access = crate::command::access_seam(ctx)?;
    crate::fetch::describe_artifact(&scope, &access, &args.reference).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_args_ref_rename_and_unknown_key_tolerance() {
        let args: DescribeToolArgs =
            serde_json::from_str(r#"{"ref": "skills/x", "global": true, "ignored": true}"#).unwrap();
        assert_eq!(args.reference, "skills/x");
        assert!(args.scope.global());
    }
}
