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
| Glob-scoped rules | Codex | `kind_support` = false for Rule (AGENTS.md directory-granular only) | no path-glob scoping ([codex docs](https://github.com/openai/codex/blob/main/docs/config.md)) | enable Rule kind + scoped render |
| Vendor-specific skill frontmatter | OpenCode, Copilot | empty skill field registries | no vendor skill keys documented ([opencode](https://opencode.ai/docs/skills/), [copilot](https://docs.github.com/en/copilot)) | populate registries + parity docs |
| `openai.yaml` skill sidecar | Codex | not emitted | sidecar format not stabilized ([codex repo](https://github.com/openai/codex)) | emit sidecar from skill metadata |
| Agent `permission` map | OpenCode | dropped (scalar-only metadata) | shipped upstream ([opencode agents](https://opencode.ai/docs/agents/)) | gated on `adr_structured_vendor_metadata.md` acceptance (FieldType::Json) |
| MCP `oauth: false` opt-out | OpenCode | not representable — the descriptor `oauth` field is the structured object-only `McpOAuth` block | shipped upstream ([opencode mcp](https://opencode.ai/docs/mcp-servers/)) | needs schema verify — no dual-typed field; consider `oauth_disabled` bool |
| `.agent.md` extension | Copilot | emits `.md` agents | settled upstream — spec requires `.agent.md` ([copilot cli docs](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/create-custom-agents-for-cli)); tracked in [grimoire#44](https://github.com/grimoire-rs/grimoire/issues/44) (renderer-version re-materialization mechanism, NOT implemented on this branch) | live-verify against a shipped, version-pinned CLI first (issue #44), then switch/dual-emit needs layout-move reaper |
| `excludeAgent` third enum value | Copilot | two-literal enum | proposed ([gh discussion #195217](https://github.com/orgs/community/discussions/195217)) | append literal (additive) |
| `nickname_candidates` | Codex | not representable | shipped upstream; needs array FieldType ([codex config](https://github.com/openai/codex/blob/main/docs/config.md)) | add array FieldType, then registry row |
| `ws` MCP transport projection | OpenCode, Copilot, Codex | decline + warn (Claude projects) | not documented for these vendors | fold into remote arm per vendor |
| MCP `oauth` block projection | OpenCode, Copilot, Codex | decline + warn (Claude projects) | OpenCode/Copilot: no native oauth config surface documented. Codex: has a native `auth` enum (`oauth` default \| `chatgpt`, triggers `codex mcp login`'s stored-credential flow) — not zero-surface, just not grim's structured `McpOAuth` block | project per vendor schema (Codex: map onto the `auth` enum, not the full block) |

## Wave-1 vendor watchlist

All rows `verified 2026-07-19/20` (Cursor, Kiro, Junie, Gemini, Zed, Amp
landed in the vendor-wave expansion). Sources: `research_vendor_verification_*.md`.

| Capability | Vendor | Current grim behavior | Upstream status | Action when shipped |
|---|---|---|---|---|
| `CURSOR_CONFIG_DIR` override | Cursor | not honored (hardcodes `~/.cursor`) | possibly CLI-only, unverified against the IDE; SpaceX-acquisition watch (config surface may reshape); `/migrate-to-skills` leaves grim's `.mdc` rule shapes untouched | honor once IDE-honored is confirmed |
| Agent kind | Kiro | declined | CLI/IDE agent-format collision (#8040) — same `.kiro/agents/` dir, incompatible JSON schemas | enable Agent when the schema is unified |
| Global rule `fileMatch` scoping | Kiro | writes correct `fileMatch` steering + warns it is upstream-inert (#9176) | per-file `fileMatch` scoping open (#9176) | drop the warning when #9176 closes (self-heal, no render change) |
| `KIRO_HOME` override | Kiro | not honored | CLI-only; the IDE ignores it (#9148) — #9148 closed by bot mis-triage as dup of #6401 (unrelated/symlinks); gap confirmed open via changelog absence, not issue state | honor once IDE-honored |
| MCP `disabledTools` / remote oauth | Kiro | not emitted | docs added `disabledTools` + remote `oauth`/`oauthScopes` | projection candidates |
| Agent kind | Junie | declined | `.junie/agents/*.md` is EAP-only, not GA | enable Agent at GA |
| MCP env interpolation | Junie | ref-bearing descriptors skipped | env interpolation undocumented (JUNIE-2173) | drop the skip when documented |
| `JUNIE_*_LOCATIONS` overrides | Junie | not honored | per-kind override family untested | honor once verified |
| Legacy `guidelines/` folder | Junie | not written | folder semantics undocumented | watch — no action yet |
| Rules (Antigravity) | Gemini | declined (GEMINI.md hierarchy only) | individual-tier Gemini CLI sunset 2026-06-18 → Antigravity CLI | follow-up vendor candidate once the Antigravity surface is verified |
| `experimental.enableAgents` | Gemini | emits agents (flag default true) | default `true` pinned via `settingsSchema.ts` + revert PR #23672 | re-check on release-pin bumps |
| MCP oauth block | Gemini | skipped | `{enabled}`/`authProviderType` shape ≠ grim's `McpOAuth` | project when the shapes align |
| Agent inline `mcpServers` | Gemini | not emitted | agent frontmatter now allows inline `mcpServers` | projection candidate |
| Rules | Zed | declined | 9-file instruction precedence (`.rules` first … AGENTS.md 7th), no scoping | wave-2 injection must handle shadowing |
| MCP env refs | Zed | ref-bearing descriptors skipped | env-ref / keychain support tracked (#56881) | drop the skip when shipped |
| `$AMP_SETTINGS_FILE` | Amp | not honored (no such var exists) | only `--settings-file` / `--mcp-config` CLI flags exist | none — no env surface to honor |
| Skills-scan precedence | Amp | installs to the shared `.agents/skills` pool | scan precedence list includes `.claude/skills` back-compat | watch — a precedence shift could reshadow |
| Live MCP handshake validation | all | config shapes only, never validates the wire protocol | MCP spec breaking release 2026-07-28 (wire-protocol only; config shapes unaffected) | re-check only if grim ever validates live handshakes |

## Fragility note

Overlap detection in `test_path_overlaps_declared_or_absent` compares
`paths:` patterns as **exact strings**. This rule's globs
(`src/install/vendor_*.rs`, `src/oci/mcp.rs`) are unique strings today, so
no declared-overlap group is required — but they *semantically* overlap
`src/**` and `**/*.rs`. If another rule ever adopts these exact strings, a
declared group in `.claude/rules.md` becomes mandatory.
