# Research: upstream refresh — specs (sweep)

## Metadata
**Date:** 2026-11-16 · **Domain:** specs · **Depth:** sweep — checked 50d ago
**Triggered by:** /upstream-refresh
**Expires:** 2027-05-16

## Claims

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | HTTP+SSE transport deprecated since 2025-03-26; earliest removal "Three months after SEP-2596 reaches Final"; still part of spec | .agents/upstream-watchlist.md:33 | deprecated, not removed | unchanged | https://github.com/modelcontextprotocol/modelcontextprotocol/blob/2026-07-28/docs/specification/2026-07-28/deprecated.mdx | "A Deprecated feature remains part of the specification but is scheduled for removal" / row: "HTTP+SSE transport ... 2025-03-26 ... Streamable HTTP ... Three months after SEP-2596 reaches Final" | MCP 2026-07-28 |
| 2 | Standard transports are stdio and Streamable HTTP only, no WebSocket | .agents/upstream-watchlist.md:33 | stdio + Streamable HTTP | unchanged | https://github.com/modelcontextprotocol/modelcontextprotocol/blob/2026-07-28/docs/specification/2026-07-28/basic/transports/index.mdx | "The binding pages specify the standard transports: 1. stdio ... 2. Streamable HTTP" | MCP 2026-07-28 |
| 3 | mcp-servers.md "HTTP+SSE transport is deprecated in the MCP spec but not removed" | docs/src/content/docs/mcp-servers.md:116-117 | deprecated not removed | unchanged | same as #1 | "No features have been removed under this policy yet." (deprecated.mdx, Removed section) | MCP 2026-07-28 |
| 4 | `ws` is no MCP standard transport (custom only), grim declines it per client | docs/src/content/docs/mcp-servers.md:92,505 | n/a | unchanged | same as #2 | "Clients and servers MAY implement additional custom transport mechanisms" | MCP 2026-07-28 |
| 5 | Link liveness: modelcontextprotocol.io/specification/latest (artifacts.md:967, mcp-servers.md:560, commands.md:4770, guides/mcp-everywhere.md:12, catalog registries.md:1162) | docs/src/content/docs/*.md, catalog/skills/grim-usage/references/registries.md:1162 | resolves | unchanged: HEAD 200, redirects to https://modelcontextprotocol.io/specification/2026-07-28 | https://modelcontextprotocol.io/specification/latest | HTTP 200 final URL .../specification/2026-07-28 | 2026-07-28 |
| 6 | Link liveness: https://modelcontextprotocol.io (catalog/descriptions/grim-mcp.md:3) | catalog/descriptions/grim-mcp.md:3 | resolves | unchanged: HEAD 200, redirects to /docs/2026-07-28/getting-started/intro | https://modelcontextprotocol.io | HTTP 200 | 2026-07-28 |
| 7 | skill `name`: 1-64 chars, lowercase alnum + hyphen, no leading/trailing hyphen, no `--`, must match dir. grim enforces all, ASCII-only, plus `.` superset | src/skill/skill_name.rs:6,29,56-78; artifacts.md:135-155 | enforced | unchanged | https://github.com/agentskills/agentskills/blob/main/docs/specification.mdx | "Must be 1-64 characters / May only contain unicode lowercase alphanumeric characters (`a-z`, `0-9`) and hyphens (`-`) / Must not contain consecutive hyphens (`--`) / Must match the parent directory name" | agentskills 83021e2 (spec file last changed 217be54, 2026-08-04) |
| 8 | `description`: 1-1024 chars non-empty, enforced as hard error | src/skill/skill_description.rs:7,17; skill-spec.md:36,163 | enforced | unchanged | same as #7 | "Must be 1-1024 characters" | same |
| 9 | `compatibility`: max 500, warn-only (never fails) | src/skill/skill_frontmatter.rs:46-59 + validate_namespaced_metadata; artifacts.md:171; skill-spec.md:38; watchlist:11 | warns on empty/>500 | unchanged | same as #7 | "Max 500 characters." / "Must be 1-500 characters if provided" | same |
| 10 | `license`, `metadata` (string->string map), `allowed-tools` (space-separated string, experimental) field table | artifacts.md:170-173; skill-spec.md:39 | as documented | unchanged | same as #7 | "A map from string keys to string values" / "A space-separated string of tools that are pre-approved to run - Experimental" | same |
| 11 | `.agents/skills` cross-vendor pool convention is named by the client-implementation guide, not the spec | clients.md:82,416,587 | convention | unchanged | https://agentskills.io/client-implementation/adding-skills-support | "The `.agents/skills/` paths have emerged as a widely-adopted convention for cross-client skill sharing. While the Agent Skills specification does not mandate where skill directories live" | agentskills 83021e2 |
| 12 | Link liveness agentskills: /specification and /client-implementation/adding-skills-support | artifacts.md `[agentskills-spec]`; domains.md | resolves | unchanged: both HEAD 200, no redirect | https://agentskills.io/specification | HTTP 200 | - |

## Verifier sign-off

none required — no code landed in this run.

## Routing

- #5 Link liveness: modelcontextprotocol.io/specification/latest (artifacts.md:967,... → (a) ledger or doc text fix
- #6 Link liveness: https://modelcontextprotocol.io... → (a) ledger or doc text fix
- #12 Link liveness agentskills: /specification and... → (a) ledger or doc text fix

## Issue drafts

none

## Friction

- dropped (unsourced): artifacts.md:145 "Agent Skills standard ... allows only `[a-z0-9-]`" is slightly off: spec prose says "unicode lowercase alphanumeric characters (`a-z`, `0-9`)". Text unchanged since 217be54 (verified 2026-09-27), so not a new change. Possible wording fix, class (a); grim is stricter (ASCII), so no limit unenforced.
- dropped (unsourced): Per-client MCP statements in mcp-servers.md (Claude, Codex, Copilot, Antigravity, Qoder...) belong to the vendors target, not specs.
- not reached: src/oci/mcp.rs and src/install/client_target.rs (`agents`) code-level check against spec: not walked beyond the SSE/ws claims above.
- not reached: SEP-2596 Final status date (removal clock) not fetched; lifecycle page /community/feature-lifecycle unchecked.
- feed: newest MCP 2026-07-28; agentskills 83021e2be457 (2026-11-04), cursor found yes

## Skill changes

Host caps concurrent subagents at 20; the researcher built its own claim list; replies are a short summary with the table in a file. Fixed in this run's chore(skills) commit.
