 - bundle expansion "wrong registry": ROOT-CAUSED, not a grim defect — the four
   ghcr.io/grimoire-rs packages had never been published (last green
   publish-catalog run, June 12, still targeted grim.ocx.sh). RESOLVED by the
   v0.7.0 release: post-announce publish-catalog pushed all four to GHCR and
   announced them (index PR #1, auto-merged). REMAINING (human, UI-only —
   no API for package visibility): flip each package public via direct
   settings URLs (sub-namespaced packages may not list in the Packages tab):
   github.com/orgs/grimoire-rs/packages/container/skills%2Fgrim-usage/settings,
   …/skills%2Fai-config-authoring/settings, …/skills%2Fgrim-authoring/settings,
   …/bundles%2Fgrim-essentials/settings. Then re-test TUI bundle expansion
   (anonymous pulls 403 until public).
 - [x] registry longest-prefix / "ghcr.io/grimoire-rs splitted into ghcr.io and
   grimoire-rs": fixed in two commits — 7f7e609 (index-only sets corrupted short-id
   adds with a registry-less ref; now falls back through the documented default
   chain) and 8b12470 (TUI tree roots index-sourced rows at their source locator;
   host/namespace chains fold into one node).
 - [x] mcp launch-scope semantics: RESOLVED by grim mcp v2
   (adr_mcp_percall_scope_fetch_render.md) — scope is a per-tool-call
   parameter (global/config/workspace), `grim mcp --global/--config`
   removed (exit 64); grim_fetch returns artifact content in-context
   (canonical or vendor projection), grim_render (behind --allow-writes)
   writes vendor-native files to an arbitrary dest_dir.
 - mcp v2 follow-ups (deferred, see adr_mcp_percall_scope_fetch_render.md):
   MCP roots as scope defaults (client support varies), hosted/remote MCP
   facade on the index server, MCP resources, manifest cache for true
   offline fetch (GRIM_OFFLINE fetch fails at fetch_manifest even with
   warm blobs). Still open from the mcp kind v1: mcp bundle membership,
   `${VAR:-default}` support, per-vendor override keys in the descriptor,
   VS Code user-profile mcp.json surface.
