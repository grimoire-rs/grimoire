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
| Rule scoping | Junie | `Degraded` — installs to `.junie/rules/<n>.md` with `paths` dropped + warning | *verified 2026-07-27.* `.junie/rules/*.md` is **current, not legacy** — step 4 of the discovery order, above the step-5 "Legacy guidelines file (still supported)". Verbatim: "All Markdown files in the rules directory, concatenated automatically" ([junie docs](https://junie.jetbrains.com/docs/environment-variables.html)) — flat, no per-file activation key. Supersedes the earlier "no grim-ownable per-file rules surface" verdict, which was wrong: the surface is ownable, the blocker is scoping | promote to `Native` when a per-file activation/glob key is documented |
| Global rules directory | Junie | `kind_surface(Rule, Global) == false` — warn, skip, zero outputs | no `~/.junie/rules/` documented; only the workspace `.junie/rules/` exists | drop the `kind_surface` override when a user-level rules dir ships |
| MCP env interpolation | Junie | ref-bearing descriptors skipped | env interpolation undocumented (JUNIE-2173) | drop the skip when documented |
| `JUNIE_*_LOCATIONS` overrides | Junie | not honored | per-kind override family untested | honor once verified |
| Legacy `guidelines/` folder | Junie | not written | folder semantics undocumented | watch — no action yet |
| Rules (Antigravity) | Gemini | declined (GEMINI.md hierarchy only) | individual-tier Gemini CLI sunset 2026-06-18 → Antigravity CLI | **done 2026-07-26** — Antigravity 2.0 shipped as its own client; see its rows below. The Antigravity CLI variant is still unserved |
| `experimental.enableAgents` | Gemini | emits agents (flag default true) | default `true` pinned via `settingsSchema.ts` + revert PR #23672 | re-check on release-pin bumps |
| MCP oauth block | Gemini | skipped | `{enabled}`/`authProviderType` shape ≠ grim's `McpOAuth` | project when the shapes align |
| Agent inline `mcpServers` | Gemini | not emitted | agent frontmatter now allows inline `mcpServers` | projection candidate |
| Rules | Zed | declined | 9-file instruction precedence (`.rules` first … AGENTS.md 7th), no scoping | wave-2 injection must handle shadowing |
| MCP env refs | Zed | ref-bearing descriptors skipped | env-ref / keychain support tracked (#56881) | drop the skip when shipped |
| `$AMP_SETTINGS_FILE` | Amp | not honored (no such var exists) | only `--settings-file` / `--mcp-config` CLI flags exist | none — no env surface to honor |
| Skills-scan precedence | Amp | installs to the shared `.agents/skills` pool | scan precedence list includes `.claude/skills` back-compat | watch — a precedence shift could reshadow |
| Rules | Antigravity | declined | *verified 2026-07-26.* Workspace `.agents/rules` IS a per-file folder ([antigravity docs](https://antigravity.google/docs/rules-workflows)), but (a) global rules are one shared `~/.gemini/GEMINI.md` that Gemini CLI also writes ([gemini-cli#16058](https://github.com/google-gemini/gemini-cli/issues/16058)) and `kind_support` cannot answer per scope, and (b) no rule-file frontmatter table is published, so `paths` has no on-disk target | enable Rule when a global per-file dir appears, or when a documented scoping key lets grim carry `paths` at workspace scope |
| Project-scope detection | Antigravity | never detected at project scope | *verified 2026-07-26.* `/docs/projects` documents no product-specific project marker; `.agents/` is a five-client shared marker and must not count | flip `detect` when a product-specific project dir is documented |
| Global root sharing (IDE variant **or Gemini CLI**) | Antigravity | detects on `~/.gemini/config` | *verified 2026-07-26.* `~/.gemini/config` is 2.0's documented user-config root. **Unresolved on two fronts:** whether the v2.1.x IDE also creates it (one summarizer pass suggested it, no independent confirmation), and whether plain **Gemini CLI** ever creates a `config` subdir under its own `~/.gemini` root — if it does, every Gemini CLI user auto-detects as Antigravity and gets agents + a spliced `mcp_config.json` under a root their client never reads | tighten the marker if 2.0 gains an exclusive dir |
| Reverse detection leak into Gemini | Antigravity | none — disclosed in `docs/src/clients.md` `{#gap-antigravity}` | *found in review 2026-07-26.* `~/.gemini/config` nests inside `~/.gemini`, so a global Antigravity install creates Gemini CLI's global marker and makes **gemini** detected on the next autodetected command. Narrowing `gemini`'s marker to a Gemini-CLI-exclusive file (`settings.json` / `oauth_creds.json`) would fix it but changes a shipped client's detection under the freeze — owner call | revisit with `vendor_gemini`'s owner |
| `ws` MCP transport | Antigravity | declined | *verified 2026-07-26, ambiguous.* `/docs/mcp` names websocket alongside sse/http under one `serverUrl` field, but only via a summarizing fetch — the raw page body could not be retrieved, and a merged transport list is a summarizer artifact shape. Declined because decline → support is additive and the reverse is breaking | enable `Ws` in `mcp_entry` once raw page text confirms it |
| Path-relocating env override | Antigravity | none honored | *verified 2026-07-26.* No override found across nine official pages; `ANTIGRAVITY_API_KEY` / `ANTIGRAVITY_TOKEN` are auth credentials and move nothing. Recorded as "not found in current docs", not "does not exist" | honor once one is documented |
| `antigravity.*` agent registry | Antigravity | empty — only `name`/`description`/`model`/`tools` projected | *verified 2026-07-26.* Upstream `/docs/subagents` also documents `mainAgent`, `subagent`, `commandExecutionPolicy`, `mcpServers`, `skills`/`plugins` | additive registry candidate — each key is a permanent contract, so add on demand |
| MCP oauth block | Antigravity | skipped | *verified 2026-07-26.* Upstream shape is `{clientId, clientSecret}` + `authProviderType`; grim's `McpOAuth` is `{client_id, scopes, callback_port, auth_server_metadata_url}` — no `clientSecret`, no target for three fields | project when the shapes align |
| MCP env refs | Antigravity | ref-bearing descriptors skipped | *verified 2026-07-26.* `/docs/mcp` documents no `${VAR}` substitution — silence, not a documented negative | drop the skip when documented |
| CLI / IDE variants | Antigravity | not served — the client name targets 2.0 only | *verified 2026-07-26.* The CLI (`agy`) reads `~/.gemini/antigravity-cli/skills/` and the IDE `~/.gemini/antigravity/skills/`. The bet is that both converge on the shared `~/.gemini/config/` root: `/hooks` moved there in CLI v1.0.8 and `/agents` in v1.1.0, but **skills are named in no changelog entry yet** | add as separate client names (additive) if convergence does not happen |
| Live MCP handshake validation | all | config shapes only, never validates the wire protocol | MCP spec breaking release 2026-07-28 (wire-protocol only; config shapes unaffected) | re-check only if grim ever validates live handshakes |

## Fragility note

Overlap detection in `test_path_overlaps_declared_or_absent` compares
`paths:` patterns as **exact strings**. This rule's globs
(`src/install/vendor_*.rs`, `src/oci/mcp.rs`) are unique strings today, so
no declared-overlap group is required — but they *semantically* overlap
`src/**` and `**/*.rs`. If another rule ever adopts these exact strings, a
declared group in `.claude/rules.md` becomes mandatory.
