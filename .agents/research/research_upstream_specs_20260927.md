# Research: upstream refresh — specs (sweep)

## Metadata
**Date:** 2026-09-27 · **Domain:** specs · **Depth:** `specs sweep never checked`
**Triggered by:** /upstream-refresh --domain specs
**Expires:** 2027-03-27

Two sonnet researchers (MCP half, agentskills.io half) and one opus
verifier. Feeds at pass start: MCP spec releases, newest stable tag
`2026-07-28` (previous `2025-11-25`); `agentskills/agentskills` has no
releases, newest commit `69ef37e9424c0a7ea9dd2293b559e43ec8176379`
(2026-08-09).

## Claims

### MCP specification

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| M1 | SSE transport deprecation status | `docs/src/content/docs/mcp-servers.md:115-118`; `src/oci/mcp.rs:44-46` | "sse … transport is deprecated upstream in the MCP spec but still accepted by every client" | unchanged: HTTP+SSE is still Deprecated, not removed, now under the feature-lifecycle policy with an earliest-removal rule | [deprecated.mdx](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/2026-07-28/docs/specification/2026-07-28/deprecated.mdx) | "HTTP+SSE transport \| SEP-2596 \| Deprecated in 2025-03-26 \| Migrate to Streamable HTTP \| Earliest removal: Three months after SEP-2596 reaches Final." | 2026-07-28 |
| M2 | Standard transport list (no WebSocket) | `mcp-servers.md:91,120-122`; `src/oci/mcp.rs:47-50` | grim's `ws` is Claude-native; the spec defines only stdio and Streamable HTTP | unchanged | [transports/index.mdx](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/2026-07-28/docs/specification/2026-07-28/basic/transports/index.mdx) | "The binding pages specify the standard transports: 1. stdio: newline-delimited messages over the standard streams of a client-launched subprocess. 2. Streamable HTTP: each message is an HTTP POST to a single MCP endpoint; replies arrive as a JSON object or a request-scoped SSE stream." | 2026-07-28 |
| M3 | `[mcp-spec]` link target | `mcp-servers.md:473` (and `artifacts.md:555`, `commands.md:2169`, `guides/mcp-everywhere.md:12`, `catalog/skills/grim-usage/references/registries.md:936`) | `https://spec.modelcontextprotocol.io/` | **dead** (TLS EOF on HEAD); `https://modelcontextprotocol.io/specification/latest` redirects to the current revision | [versioning](https://modelcontextprotocol.io/specification/versioning) | "The current protocol version is 2026-07-28." | 2026-07-28 |
| M4 | Authorization-server discovery, HTTPS | `mcp-servers.md:146`; `src/oci/mcp.rs:85` | `auth_server_metadata_url` = RFC 8414 metadata URL, https-only | unchanged: RFC 9728 protected-resource metadata points at the AS, whose metadata is RFC 8414 / OIDC discovery; AS endpoints must be HTTPS | [authorization-server-discovery.mdx](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/2026-07-28/docs/specification/2026-07-28/basic/authorization/authorization-server-discovery.mdx), [security-considerations.mdx](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/2026-07-28/docs/specification/2026-07-28/basic/authorization/security-considerations.mdx) | "MCP servers **MUST** implement the OAuth 2.0 Protected Resource Metadata (RFC9728) specification to indicate the locations of authorization servers." / "All authorization server endpoints **MUST** be served over HTTPS." | 2026-07-28 |
| M5 | `client_id` pre-registration | `mcp-servers.md:143` | "Pre-registered OAuth client id" | unchanged: pre-registration is still first priority; then Client ID Metadata Documents, then DCR as a deprecated fallback | [client-registration.mdx](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/2026-07-28/docs/specification/2026-07-28/basic/authorization/client-registration.mdx) | "Clients supporting all options **SHOULD** use the following priority order: 1. Use pre-registered client information for the server if the client has it available" | 2026-07-28 |
| M6 | `scopes` | `mcp-servers.md:144` | requested scopes | unchanged: a static list stays valid; the spec prefers the `WWW-Authenticate` challenge scope, then `scopes_supported` | [authorization/index.mdx](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/2026-07-28/docs/specification/2026-07-28/basic/authorization/index.mdx) | "MCP clients **SHOULD** follow this priority order for scope selection: 1. **Use `scope` parameter** from the initial `WWW-Authenticate` header in the 401 response, if provided" | 2026-07-28 |
| M7 | `callback_port` | `mcp-servers.md:145` | fixed localhost callback port | unchanged: redirect URIs must be `localhost` or HTTPS; no port constraint | [security-considerations.mdx](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/2026-07-28/docs/specification/2026-07-28/basic/authorization/security-considerations.mdx) | "All redirect URIs **MUST** be either `localhost` or use HTTPS." | 2026-07-28 |
| M8 | No cross-vendor MCP client config location | `mcp-servers.md:212-213`; `src/install/vendor_agents.rs:24-26` | none exists | unchanged: registry `server.json` is a server-publishing format, not client config | [registry/about.mdx](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/docs/registry/about.mdx) | "The MCP Registry is not intended to be directly consumed by host applications." | 2026-07-28 |
| M9a | Dynamic Client Registration deprecated | no ledger row | — | new in 2026-07-28: DCR deprecated in favour of CIMD, earliest removal in the first revision on or after 2027-07-28. grim models no DCR field | [deprecated.mdx](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/2026-07-28/docs/specification/2026-07-28/deprecated.mdx) | "Dynamic Client Registration \| PR #2858 \| `2026-07-28` \| Client ID Metadata Documents \| First revision released on or after 2027-07-28" | 2026-07-28 |
| M9b | Client ID Metadata Documents | no ledger row | — | added in 2025-11-25 as the recommended registration mechanism | [2025-11-25 changelog](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/docs/specification/2025-11-25/changelog.mdx) | "Add support for OAuth Client ID Metadata Documents as a recommended client registration mechanism (SEP-991, PR #1296)" | 2025-11-25 |
| M9c | Stateless protocol | no ledger row | — | 2026-07-28 removes `initialize` and `Mcp-Session-Id`, adds mandatory `server/discover`. No descriptor field is affected. grim's own server (`grim mcp`) locks rmcp 3.3.0, and rmcp 3.0 added 2026-07-28 support | [2026-07-28 changelog](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/2026-07-28/docs/specification/2026-07-28/changelog.mdx), [rmcp-v3.0.0](https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v3.0.0) | "Make MCP stateless: remove the `initialize`/`notifications/initialized` handshake" / "RMCP 3.0 adds support for MCP 2026-07-28." | 2026-07-28 |

### agentskills.io

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| A1 | Canonical spec URL | `src/skill.rs:10`, `artifacts.md:552`, `vendor-metadata.md:632` | `https://agentskills.io/specification` | unchanged | [adding-skills-support.mdx](https://github.com/agentskills/agentskills/blob/69ef37e9424c0a7ea9dd2293b559e43ec8176379/docs/client-implementation/adding-skills-support.mdx) | "Familiarity with the [Agent Skills specification](/specification), which defines the `SKILL.md` file format, frontmatter fields, and directory conventions." | 69ef37e9424c |
| A2 | `name` rules | `artifacts.md:67-74`; `src/skill/skill_name.rs:6,29` | `[a-z0-9-]`, 1–64, no edge/adjacent hyphen, equals directory | unchanged (grim's `.` stays a documented superset) | [specification.mdx](https://github.com/agentskills/agentskills/blob/69ef37e9424c0a7ea9dd2293b559e43ec8176379/docs/specification.mdx) | "May only contain unicode lowercase alphanumeric characters (`a-z`, `0-9`) and hyphens (`-`). Must not start or end with a hyphen (`-`). Must not contain consecutive hyphens (`--`)." | 69ef37e9424c |
| A3 | `description` 1–1024 | `src/skill/skill_description.rs:7,12` | 1–1024 | unchanged | specification.mdx | "Must be 1-1024 characters. Should describe both what the skill does and when to use it." | 69ef37e9424c |
| A4 | `license` optional string | `artifacts.md:93` | optional string | unchanged | specification.mdx | "Specifies the license applied to the skill." | 69ef37e9424c |
| A5 | `compatibility` length | `artifacts.md:94`; `catalog/skills/grim-authoring/references/skill-spec.md:34` | free text, no cap | **the spec caps it at 500 characters**; grim does not enforce it | specification.mdx | "Must be 1-500 characters if provided. Should only be included if your skill has specific environment requirements." | 69ef37e9424c |
| A6 | `allowed-tools` delimiter | `artifacts.md:95,139`; `skill-spec.md:35` | "Comma-separated tool allowlist" | **space-separated string, experimental** | specification.mdx | "A space-separated string of tools that are pre-approved to run. Experimental. Support for this field may vary between agent implementations." | 69ef37e9424c |
| A7 | `metadata` string→string map | `vendor-metadata.md:70`; `src/skill/skill_frontmatter.rs:53-57` | string values | unchanged (wording clarified in 3f3bbec8, PR #479) | specification.mdx | "A map from string keys to string values. Clients can use this to store additional properties not defined by the Agent Skills spec." | 69ef37e9424c |
| A9 | Install locations | `.agents/adr/adr_vendor_config_and_selection.md` D1; `src/install/vendor_agents.rs:6-9` | the spec is silent on locations | the spec is still silent, but agentskills.io's client guide (PR #200, 2026-03-05) names `.agents/skills/` and `~/.agents/skills/` as the cross-client convention | [adding-skills-support.mdx](https://github.com/agentskills/agentskills/blob/69ef37e9424c0a7ea9dd2293b559e43ec8176379/docs/client-implementation/adding-skills-support.mdx) | "The `.agents/skills/` paths have emerged as a widely-adopted convention for cross-client skill sharing. While the Agent Skills specification does not mandate where skill directories live (it only defines what goes inside them), scanning `.agents/skills/` means skills installed by other compliant clients are automatically visible to yours, and vice versa." | 69ef37e9424c |
| A10a | `name` digit range | informational | — | the spec text omitted `0-9` until 6868401b (2026-05-16); it now matches grim | [6868401b](https://github.com/agentskills/agentskills/commit/6868401b64f7) | "+May only contain unicode lowercase alphanumeric characters (`a-z`, `0-9`) and hyphens (`-`)" | 6868401b64f7 |
| A10b | Optional directories non-exhaustive | informational | — | `scripts/`, `references/`, `assets/` are recommendations, not a closed set (PR #268) | [675602eb](https://github.com/agentskills/agentskills/commit/675602ebc261) | "the listed directories are recommendations, not requirements." | 675602ebc261 |
| A10c | Client-implementation guide | informational | — | new guide: discovery scopes, lenient validation (name mismatch and length over 64 warn rather than reject) | [adding-skills-support](https://agentskills.io/client-implementation/adding-skills-support) | "Most locally-running agents scan at least two scopes" | 69ef37e9424c |

## Verifier sign-off

Opus verifier re-fetched every claim that drives an edit or an issue draft.

| Claim | Verdict | Evidence URL |
|---|---|---|
| M3 — old spec host dead, `/specification/latest` → 2026-07-28 | confirmed (HEAD: `SSLEOFError`; `/specification/latest` 200 at `/specification/2026-07-28`). Note: `/specification/versioning` now redirects to `/docs/2026-07-28/learn/versioning` | https://modelcontextprotocol.io/specification/versioning |
| A6 — `allowed-tools` space-separated, experimental | confirmed | https://github.com/agentskills/agentskills/blob/69ef37e9424c0a7ea9dd2293b559e43ec8176379/docs/specification.mdx |
| A5 — `compatibility` 1–500 characters | confirmed | same |
| A9 — `.agents/skills` convention in the guide, not the spec | confirmed | https://github.com/agentskills/agentskills/blob/69ef37e9424c0a7ea9dd2293b559e43ec8176379/docs/client-implementation/adding-skills-support.mdx |
| M1/M2 — HTTP+SSE deprecated not removed; stdio + Streamable HTTP only | confirmed | https://github.com/modelcontextprotocol/modelcontextprotocol/blob/2026-07-28/docs/specification/2026-07-28/basic/transports/index.mdx |
| M5/M9a — DCR deprecated, pre-registration first | confirmed | https://github.com/modelcontextprotocol/modelcontextprotocol/blob/2026-07-28/docs/specification/2026-07-28/basic/authorization/client-registration.mdx |
| M9c — rmcp 3.0 supports 2026-07-28; grim locks 3.3.0 | confirmed | https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v3.0.0 |

## Live CLI check

n/a — `sweep`, domain row, no harness CLI.

## Routing

| Claim | Class | Landing |
|---|---|---|
| M1, M2 | (a) | new dated watchlist row "HTTP+SSE transport (spec)"; the `mcp-servers.md` SSE paragraph and the `McpTransport::Sse` doc comment in `src/oci/mcp.rs` claimed every client accepts `sse` — Codex has no SSE transport and gets a streamable-HTTP `url` entry (Codex watchlist row), so both were reworded (doc text only; it also feeds the generated MCP schema `description`) |
| M3 | (a) | five MCP spec links repointed to `/specification/latest` (docs + `catalog/skills/grim-usage/references/registries.md`) — docs commit |
| A6 | (a) | `artifacts.md` Skills table + example, `skill-spec.md` row: space-separated, experimental; `vendor-metadata.md` `claude.allowed-tools` row now says Claude accepts a space- or comma-separated string ("Accepts a space- or comma-separated string, or a YAML list.", [claude skills](https://code.claude.com/docs/en/skills)) — docs commit |
| A5 | (a) + (c) | cap stated in `artifacts.md` and `skill-spec.md` as not enforced; enforcement → issue draft 1 |
| A9 | (a) | recorded here and in domains.md; ADR D1's premise ("spec silent on locations") still holds for the spec itself, and its decision rests on the owner's vendor-dir preference, so no ADR edit |
| M4–M8, M9a–c, A1–A4, A7, A10 | (a) | no change — confirmed or informational |

The only `src/` change is one doc comment, so no renderer, parity or
self-heal proof applies. The diff touches `src/oci/**` and `catalog/**`, so
the security seat ran.

## Review

- reviewer:spec (opus): no Block. Warn 1: `mcp-servers.md:115-117` said
  every client accepts `sse`, contradicted by the Codex row — fixed (plus the
  same claim in the `src/oci/mcp.rs` doc comment). Warn 2:
  `vendor-metadata.md:192` called `claude.allowed-tools` comma-separated
  while `artifacts.md` now says space-separated — fixed against Claude's
  skills docs. Suggest (catalog Tier 3 prefers linking exact limits): not
  applied, the same table already inlines the 1024 cap. `upstream:stale`
  report and `prose.py` on the changed lines: clean.
- reviewer:security (opus): no Block or Warn. Suggest:
  `docs/src/content/docs/artifacts.md:139` · least-privilege example ·
  unscoped `Bash` in the example (pre-existing, not applied). New links all
  point at the official MCP project. Follow-up on the `src/oci/mcp.rs` doc
  comment: no findings (doc text only; serde, validation and behaviour unchanged).

## Issue drafts

Filed as https://github.com/grimoire-rs/grimoire/issues/154.

1. **Warn on a skill `compatibility` longer than the agentskills 500-character cap**
   - Body: The Agent Skills specification says `compatibility` "Must be
     1-500 characters if provided"
     ([specification.mdx @69ef37e](https://github.com/agentskills/agentskills/blob/69ef37e9424c0a7ea9dd2293b559e43ec8176379/docs/specification.mdx)).
     grim validates `name` (≤ 64) and `description` (≤ 1024) but not
     `compatibility`, so `grim build` accepts a value strict tooling may
     reject. Rejecting it would break previously published skills
     (Principle 9), so the proposal is a build-time **warning** only, the
     way grim treats a dotted name: publish still succeeds. Open question:
     whether an empty `compatibility:` should warn too (the spec's lower
     bound is 1).
   - Labels: `enhancement`, `skills`, `spec-compat`
   - Found by: `research_upstream_specs_20260927.md`

## Friction

- The researcher's `curl` was denied and WebFetch returned "Socket is
  closed" on the dead spec host, so the MCP researcher left link liveness
  unsourced. The driver and verifier settled it with a `python3` urllib
  HEAD request. Skill fix: a liveness fallback in researcher-brief.md.
- domains.md › specs listed only `mcp-servers.md`, `src/oci/mcp.rs` and
  the `agents` target. The agentskills field table (`artifacts.md`, the
  catalog `skill-spec.md`), the `src/skill/` limits and four more MCP spec
  links were found by grep during the pass. Skill fix: domains.md §3 lists
  them.
- The driver's brief pointed the agentskills researcher at PR #268 for
  install locations; #268 is about optional directories. The researcher
  found the real source (the client guide, PR #200). Driver error, no
  skill change beyond naming the guide in domains.md.

## Skill changes

`domains.md` §3 now lists the agentskills field table (`artifacts.md`, the
catalog `skill-spec.md`), the `src/skill/` limits, every MCP spec link, the
agentskills client-implementation guide and repo-at-tag spec reads;
`researcher-brief.md` gains a link-liveness fallback. Commit `9f483b27`
(`chore(skills): widen the specs domain after its first sweep`). Docs and
watchlist fixes: `b71b6bca`.
