---
paths:
  - "src/install/vendor_*.rs"
  - "src/oci/mcp.rs"
---

# Vendor Capability Watchlist

Auto-fires on vendor renderer / MCP descriptor edits. Purpose: **re-check
upstream before patching a decline**. Every skip/warn/decline in a renderer
encodes an upstream limitation verified at a point in time — vendors ship
features continuously, and a decline can silently rot into a grim regression
(it happened: `xhigh` reasoning-effort, Codex `additionalContext`).

## Re-verify procedure

1. Before changing any decline/skip/warn in `src/install/vendor_*.rs` or
   validation in `src/oci/mcp.rs`, check the watchlist row below and its
   upstream doc link. Row stale (> ~6 months since `verified` date) →
   re-verify upstream first.
2. Upstream shipped the capability → patch renderer + docs
   (`docs/src/vendor-metadata.md` / `docs/src/mcp-servers.md`) + tests in
   **one commit** (parity tests require doc row and registry change
   together), then move/update the row here in the same commit.
3. Compatibility doctrine applies (CLAUDE.md principle 9): additive-only,
   never remove accepted literals, layout moves ship migration + reaper.

## Watchlist

All rows `verified 2026-07-17` unless noted.

| Capability | Vendor | Current grim behavior | Upstream status | Action when shipped |
|---|---|---|---|---|
| Global MCP env substitution | Copilot | skip + warn on env refs in global MCP | not documented in the local-CLI doc (literal values only) ([copilot cli docs](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers)); `${VAR}` substitution shipped in v0.0.406, then regressed in v0.0.407 ([github/copilot-cli#1403](https://github.com/github/copilot-cli/issues/1403)) — re-verify against a fixed release before trusting either state | project env refs, drop warn |
| Glob-scoped rules | Codex | `supports_kind` = false for Rule (AGENTS.md directory-granular only) | no path-glob scoping ([codex docs](https://github.com/openai/codex/blob/main/docs/config.md)) | enable Rule kind + scoped render |
| Vendor-specific skill frontmatter | OpenCode, Copilot | empty skill field registries | no vendor skill keys documented ([opencode](https://opencode.ai/docs/skills/), [copilot](https://docs.github.com/en/copilot)) | populate registries + parity docs |
| `openai.yaml` skill sidecar | Codex | not emitted | sidecar format not stabilized ([codex repo](https://github.com/openai/codex)) | emit sidecar from skill metadata |
| Agent `permission` map | OpenCode | dropped (scalar-only metadata) | shipped upstream ([opencode agents](https://opencode.ai/docs/agents/)) | gated on `adr_structured_vendor_metadata.md` acceptance (FieldType::Json) |
| MCP `oauth: false` opt-out | OpenCode | not representable — the descriptor `oauth` field is the structured object-only `McpOAuth` block | shipped upstream ([opencode mcp](https://opencode.ai/docs/mcp-servers/)) | needs schema verify — no dual-typed field; consider `oauth_disabled` bool |
| `.agent.md` extension | Copilot | emits `.md` agents | settled upstream — spec requires `.agent.md` ([copilot cli docs](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/create-custom-agents-for-cli)); tracked in [grimoire#44](https://github.com/grimoire-rs/grimoire/issues/44) (renderer-version re-materialization mechanism, NOT implemented on this branch) | live-verify against a shipped, version-pinned CLI first (issue #44), then switch/dual-emit needs layout-move reaper |
| `excludeAgent` third enum value | Copilot | two-literal enum | proposed ([gh discussion #195217](https://github.com/orgs/community/discussions/195217)) | append literal (additive) |
| `nickname_candidates` | Codex | not representable | shipped upstream; needs array FieldType ([codex config](https://github.com/openai/codex/blob/main/docs/config.md)) | add array FieldType, then registry row |
| `ws` MCP transport projection | OpenCode, Copilot, Codex | decline + warn (Claude projects) | not documented for these vendors | fold into remote arm per vendor |
| MCP `oauth` block projection | OpenCode, Copilot, Codex | decline + warn (Claude projects) | OpenCode/Copilot: no native oauth config surface documented. Codex: has a native `auth` enum (`oauth` default \| `chatgpt`, triggers `codex mcp login`'s stored-credential flow) — not zero-surface, just not grim's structured `McpOAuth` block | project per vendor schema (Codex: map onto the `auth` enum, not the full block) |

## Fragility note

Overlap detection in `test_path_overlaps_declared_or_absent` compares
`paths:` patterns as **exact strings**. This rule's globs
(`src/install/vendor_*.rs`, `src/oci/mcp.rs`) are unique strings today, so
no declared-overlap group is required — but they *semantically* overlap
`src/**` and `**/*.rs`. If another rule ever adopts these exact strings, a
declared group in `.claude/rules.md` becomes mandatory.
