# Research: upstream refresh — codex (deep)

## Metadata
**Date:** 2026-09-27 · **Domain:** vendors · **Depth:** `codex deep named harness`
**Triggered by:** /upstream-refresh codex (WP-D, plan_harness_capability_freshness)
**Expires:** 2027-03-27

Vendor version at check: Codex CLI **0.157.1** (GH release `rust-v0.157.1`,
2026-09-26; npm `@openai/codex` 0.157.1). Sources pinned to that tag
(commit `ac0e23e5`). Changelog scanned from 2026-07-17 (the newest
`verified` date among the Codex rows) to 2026-09-27. Feed cursor set to the
newest stable tag at pass start, `rust-v0.157.1`; prerelease tags
(`-alpha.N`, `prerelease: true`) are skipped per `domains.md`.

## Claims

Researcher (sonnet) table, one row per claim; numbered rows re-verify
ledger claims, `I` rows are deep-inventory differences. Quotes verbatim.
URLs under `…/` are `https://github.com/openai/codex/blob/rust-v0.157.1/codex-rs/`.

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | No path-glob rule scoping | watchlist Glob-scoped rules row | directory-granular `AGENTS.md` | unchanged; `AGENTS.override.md` precedes `AGENTS.md` per directory | https://learn.chatgpt.com/docs/agent-configuration/agents-md | "In each directory along the path, it checks for `AGENTS.override.md`, then `AGENTS.md`, then any fallback names in `project_doc_fallback_filenames`." | docs 2026-09-27 |
| 2 | `openai.yaml` sidecar | watchlist `openai.yaml` row | not stabilized | documented, parsed, optional, fail-open (`interface.*`, `policy.*`, `dependencies.tools[]`) | …/skills/src/assets/samples/skill-creator/references/openai_yaml.md | "`agents/openai.yaml` is an extended, product-specific config intended for the machine/harness to read, not the agent." | 0.157.1 |
| 3 | `nickname_candidates` needs an array type | watchlist row | as stated | unchanged | …/agent-roles/src/agent_role_config.rs | `pub nickname_candidates: Option<Vec<String>>,` | 0.157.1 |
| 4 | No `ws` MCP transport | watchlist `ws` row | as stated | unchanged: `Stdio` and `StreamableHttp` only | …/config/src/mcp_types.rs | `pub enum McpServerTransportConfig { Stdio { ... }, StreamableHttp { ... } }` | 0.157.1 |
| 5 | MCP `auth` enum | watchlist `oauth` row | `oauth` \| `chatgpt` | adds `ema_auth` (0.156+) | …/config/src/mcp_types.rs | "Exchange an enterprise IdP refresh token for resource-specific authorization." | 0.157.1 |
| 6 | Skills from `.agents/skills`, independent of `$CODEX_HOME` | `vendor_codex.rs` module doc | as stated | incomplete: also every ancestor dir up to the project root, project `.codex/skills`, deprecated `$CODEX_HOME/skills`, `/etc/codex/skills` | …/ext/skills/src/host_roots.rs | "Deprecated user skills location (`$CODEX_HOME/skills`), kept for backward compatibility." | 0.157.1 |
| 7 | Subagent TOML location and keys | `vendor_codex.rs` module doc | as stated | unchanged; discovery is recursive and merges `[agents.<name>]` in `config.toml` | …/agent-roles/src/loader.rs | `discover_agent_roles_in_dir(fs, &config_folder.join("agents"), &declared_role_files, startup_warnings)` | 0.157.1 |
| 8 | `model_reasoning_effort` literals | `CODEX_AGENT_FIELDS` | 8 literals | adds `persistent` | …/protocol/src/openai_models.rs | `"persistent" => Ok(Self::Persistent),` | 0.157.1 |
| 9 | No rule target; hooks `additionalContext` | module doc | as stated | unchanged; a hook handler may also be an `mcp_tool` | …/app-server-protocol/src/protocol/v2/config.rs | `McpTool { server: String, tool: String, input: ..., timeout_sec: ..., status_message: ... }` | 0.157.1 |
| 10 | MCP shape in `config.toml` | module doc, `mcp-servers.md` | as stated | unchanged | …/core/config.schema.json | "Raw MCP config shape used for deserialization and supported-field JSON Schema generation." | 0.157.1 |
| 11 | Refinements have no Codex target | `mcp_entry` comment | all four | `cwd`, `startup_timeout_sec`/`_ms`, `http_headers_helper` exist; `always_load` has none | …/core/config.schema.json | `"http_headers_helper": {"type": "string"}`, `"startup_timeout_sec": {"type": "number"}` | 0.157.1 |
| 12 | `sse` written as `url` | `mcp_entry` | as stated | unchanged: no `Sse` variant | …/config/src/mcp_types.rs | `StreamableHttp { url: String, ... }` | 0.157.1 |
| 13 | `CODEX_HOME` relocates agents and `config.toml`, not skills | AGENTS.md env table | as stated | unchanged for grim; its deprecated `skills/` is still scanned | https://learn.chatgpt.com/docs/agent-configuration/agents-md | "In your Codex home directory (defaults to `~/.codex`, unless you set `CODEX_HOME`), Codex reads `AGENTS.override.md` if it exists." | 0.157.1 |
| 14 | Telemetry opt-outs | `deep-pass.md` codex row | none documented | config keys `analytics.enabled`, `feedback.enabled`, `check_for_update_on_startup` | …/core/config.schema.json | "When `false`, disables analytics across Codex product surfaces in this machine. Defaults to `true`." | 0.157.1 |
| 15 | Matrix row Skill ✓ Rule ✗ Agent ✓ MCP ◐ | `clients.md` | as stated | unchanged | …/config/src/mcp_types.rs | (transport enum, row 4) | 0.157.1 |
| I1 | Agent role `config_file` key | `CODEX_AGENT_FIELDS` | — | a path to a per-role config layer; not modeled | …/agent-roles/src/agent_role_config.rs | (researcher) | 0.157.1 |
| I2 | Role files flatten the whole `ConfigToml` | `CODEX_AGENT_FIELDS` | 3 keys | any top-level config key is legal in a role file | …/agent-roles/src/agent_role_config.rs | (researcher) | 0.157.1 |
| I3 | `[agents]` session table | — | — | `enabled`, `max_concurrent_threads_per_session`, `default_subagent_model`, … — user config, no grim surface | …/core/config.schema.json | (researcher) | 0.157.1 |
| I4 | MCP keys grim cannot carry | `mcp-servers.md` | — | `enabled`, `enabled_tools`, `disabled_tools`, `default_tools_approval_mode`, `required`, `startup_readiness`, `tool_timeout_sec`, per-tool `tools.<name>`, … | …/core/config.schema.json | (researcher) | 0.157.1 |
| I5 | MCP `oauth` table keys | watchlist `oauth` row | — | `client_id`, `client_secret`, `callback_url`, `callback_port`, `authorization_server_issuer`; `scopes`, `oauth_resource` at server level | …/config/src/mcp_types.rs | (researcher) | 0.157.1 |
| I6 | Hook events | finding only (PR #98) | — | 12 under `[hooks]`: `Interrupt`, `PermissionRequest`, `PostCompact`, `PreCompact`, `PostToolUse`, `PreToolUse`, `SessionEnd`, `SessionStart`, `Stop`, `SubagentStart`, `SubagentStop`, `UserPromptSubmit` | …/app-server-protocol/src/protocol/v2/config.rs | (researcher) | 0.157.1 |
| I7 | `codex mcp-server` entry point removed | — | — | no grim reference | GH releases | (researcher) | ~0.153 |

Rows I1–I7 carry the researcher's source file but no quote of their own;
none of them drives a change, so they stand as findings only.

## Verifier sign-off

Opus verifier re-fetched every source at `rust-v0.157.1` (C-008).

| Claim | Verdict | Evidence URL |
|---|---|---|
| V1 `persistent` literal; all 8 existing literals and 3 `sandbox_mode` literals valid | CONFIRMED. Named since 0.151 ([#40799](https://github.com/openai/codex/pull/40799)); unknown strings pass through as `Custom` since 0.138 ([#26444](https://github.com/openai/codex/pull/26444)); before 0.138 a role file with `max`/`ultra`/`persistent` is skipped with a warning | https://github.com/openai/codex/blob/rust-v0.157.1/codex-rs/protocol/src/openai_models.rs |
| V2 startup timeout | CONFIRMED. `startup_timeout_ms` (u64 ms, ≤0.40) and `startup_timeout_sec` (f64 s); `_sec` wins when both set; grim's ms `timeout` maps faithfully to `_ms` | https://github.com/openai/codex/blob/rust-v0.157.1/codex-rs/config/src/mcp_types.rs |
| V3 `cwd` | REFUTED as `AbsolutePathBuf`: it is a plain string, stdio-only, resolved against the directory Codex was launched from; `cwd` on a `url` server fails the whole `config.toml` | https://github.com/openai/codex/blob/rust-v0.157.1/codex-rs/rmcp-client/src/stdio_server_launcher.rs |
| V4 `http_headers_helper` = Claude `headersHelper` | PARTIAL. Same JSON output, but a cleared env, no server name/URL vars, cached until 401/403, configured `Authorization` wins; 0.148+ | https://github.com/openai/codex/blob/rust-v0.157.1/codex-rs/rmcp-client/src/http_headers.rs |
| V5 unknown MCP keys refused | REFUTED. Only schemars denies them; serde warns and ignores (hard error only under `--strict-config`). Agent role files do deny unknown fields | https://github.com/openai/codex/blob/rust-v0.157.1/codex-rs/config/src/mcp_types.rs |
| V6 no `always_load` equivalent | CONFIRMED | same |
| V7 `auth` literals | CONFIRMED: `oauth` (default), `chatgpt`, `ema_auth` (0.156+); `auth` on stdio is a hard error | same |
| V8 skill roots | CONFIRMED, plus project `.codex/skills` | https://github.com/openai/codex/blob/rust-v0.157.1/codex-rs/ext/skills/src/host_roots.rs |
| V9 `openai.yaml` stable | CONFIRMED (optional, documented, parsed, fail-open) | https://github.com/openai/codex/blob/rust-v0.157.1/codex-rs/ext/skills/src/loader/metadata.rs |
| V10 telemetry opt-outs | PARTIAL. Config keys only, no env var; `otel.metrics_exporter` defaults to `statsig` unless `analytics.enabled = false`; `-c key=value` is global, so `codex -c … mcp list` works | https://github.com/openai/codex/blob/rust-v0.157.1/codex-rs/core/src/otel_init.rs |
| V11 subagent discovery | CONFIRMED: recursive `*.toml` under each layer's `agents/`; `name`, `description`, non-blank `developer_instructions` required; a malformed role is skipped, not fatal | https://github.com/openai/codex/blob/rust-v0.157.1/codex-rs/agent-roles/src/discovery.rs |

## Live CLI check

```text
codex mcp list (env -i, sandbox HOME, CODEX_HOME, DO_NOT_TRACK, -c analytics.enabled=false -c feedback.enabled=false -c check_for_update_on_startup=false; project marked trusted in the sandbox config.toml) — pass — both grim-rendered project entries listed enabled; the timeout fixture reads back as startup_timeout_sec 7.0
codex mcp list, global scope (grim install --global, cwd outside the project) — pass — grim's entry in $CODEX_HOME/config.toml listed; the hand-written [projects] table beside it survived the splice
codex mcp get grim — pass — stdio, command and args as rendered
codex debug prompt-input (offline, no login) — pass — grim-usage listed under Available skills, from <project>/.agents/skills (project) and $HOME/.agents/skills (global)
agents listing — skip — no headless verb; custom roles appear only in the spawn_agent tool definition, and a deliberately malformed role file raised no visible warning either (negative control)
rules — skip — Codex declines the kind; grim wrote no file (expected)
```

No login needed. Without the trust entry an untrusted project's
`.codex/config.toml` is not read at all (`No MCP servers configured`),
which confirms the trust gate the docs describe. `codex doctor` dials the
network (401 on the Responses WebSocket), so it is not a check verb. The
check ran twice: once before the renderer change, and once after it with
an MCP fixture carrying `timeout = 7000` and an agent carrying
`codex.reasoning-effort: persistent`. Sandbox and registry container removed.

## Routing

| Claim | Class | Landed as |
|---|---|---|
| 8 / V1 `persistent` literal | (b) | `CODEX_AGENT_FIELDS` + docs + unit and self-heal acceptance tests + watchlist row — [372e74d9](https://github.com/grimoire-rs/grimoire/commit/372e74d9) |
| 11 / V2 `timeout` → `startup_timeout_ms` | (b) | `mcp_entry` + `mcp-servers.md` + tests + watchlist row — [372e74d9](https://github.com/grimoire-rs/grimoire/commit/372e74d9) |
| 11 / V3 `cwd` | (a) | watchlist row: not mapped, relative base differs |
| 11 / V4 `http_headers_helper` | (c) | issue draft 2 |
| 1, 3, 4, 5, 12, I5 | (a) | watchlist rows re-dated; Codex split out of the shared `ws` and `oauth` rows so its date cannot freshen OpenCode's or Copilot's |
| 2 / V9 `openai.yaml` | (c) | issue draft 1 |
| 6, 13, V8 | (a) | `CODEX_HOME` row in the env-var table (C-018); no path change for grim |
| 14 / V10 | (a) | `deep-pass.md` codex row — skill commit |
| 7, 9, 10, 15 | (a) | confirmed unchanged; module doc dated (`verified 2026-09-27 against Codex CLI 0.157.1`) |
| I1–I4 | finding | no grim descriptor or registry field; recorded here |
| I6 hooks | finding | out of scope (PR #98); event list is the reference for it |
| I7 | none | no grim reference |
| catalog drift | (c) | issue draft 3 — `catalog/**` is a publish surface, not edited in a pass |

Security: none found.

**reviewer:spec (opus) on the renderer diff:** one Block, fixed: the
self-heal assertion read `outputs[].modified`, which status never emits,
so it could not fail; both the new Codex test and the Claude test from
2ecdfc03 now assert `state == "installed"`. Warns fixed: the upgrade
behavior below is stated in the watchlist row and the commit body; this
artifact lands right after the renderer commit that cites it. Suggests
taken: an integer-type assert on `startup_timeout_ms`, Qoder added to the
`timeout`/`cwd` rows of `mcp-servers.md` (existing drift), the Codex MCP
row of `subsystem-file-structure.md`, and the legacy-SSE risk in the
watchlist. Not taken: re-verifying Gemini's `timeout` meaning (reviewer
flagged it from memory; it belongs to the Gemini pass, WP-G).

**Decision — existing descriptors with `timeout` render one more key.**
Question: a descriptor that already carries `timeout` rendered no timeout
for Codex before; after this change `grim install` rewrites the entry with
`startup_timeout_ms`. Is that a Principle 9 break? Research: the entry is
grim-owned and judged semantically against its record, so the next
install re-materializes it like any renderer improvement; an older Codex
that predates a key only warns (V5), and `startup_timeout_ms` predates
0.40 anyway. `stability.md` freezes the descriptor schema and CLI, not the
exact rendered bytes. Decision: accept as an additive projection, the same
path Claude, OpenCode and Gemini `timeout` took; the acceptance test proves
the repeat install is byte-identical and `status` reads `installed`. The
integrity gate compares disk against the record, never against a fresh
render, so an entry installed before this change stays as it is, reported
`installed`, until its pin changes or `--force` runs — no drift, and no
`modified` state (the blind spot `adr_render_layout_stability.md` names).

## Issue drafts

Filed as https://github.com/grimoire-rs/grimoire/issues/143.

### 1. Codex: emit the `agents/openai.yaml` skill sidecar

**Labels:** `enhancement`, `vendor:codex`, `needs-design`

Codex 0.157.1 documents and parses an optional `agents/openai.yaml` beside
`SKILL.md`: `interface` (display name, short description, icons, brand
color, default prompt), `policy` (`allow_implicit_invocation`, `products`)
and `dependencies.tools[]` ([build skills](https://learn.chatgpt.com/docs/build-skills),
[metadata.rs](https://github.com/openai/codex/blob/rust-v0.157.1/codex-rs/ext/skills/src/loader/metadata.rs)).
The watchlist's precondition ("format not stabilized") no longer holds.
Emitting it is not a plain renderer row: Codex reads skills from the
shared `.agents/skills` pool, whose bytes must stay identical for every
pool client (`pool_vendors_render_byte_identical_skill_bytes`), and the
file would be a new per-skill output. Decide whether grim emits it (and
from which `codex.*` keys), or leaves it to authors who ship it inside the
skill tree.

### 2. Codex: project `headers_helper` onto `http_headers_helper`?

**Labels:** `enhancement`, `vendor:codex`, `needs-design`

Codex 0.148+ has `http_headers_helper`, a shell command that prints JSON
headers, like Claude's `headersHelper` ([http_headers.rs](https://github.com/openai/codex/blob/rust-v0.157.1/codex-rs/rmcp-client/src/http_headers.rs)).
The contracts differ: Codex clears the helper's environment down to a
fixed allow-list and passes no server name or URL, caches the result until
a 401/403, and lets a configured `Authorization` header win over the
helper's. A helper written for Claude can therefore misbehave under
Codex. Decide whether one descriptor field may drive both (and document
the differences), or whether Codex needs its own opt-in. grim drops the
field for Codex today.

### 3. Catalog `grim-authoring` drifted from the Codex docs

**Labels:** `docs`, `catalog`

This pass changed `docs/src/content/docs/vendor-metadata.md` and
`mcp-servers.md`. Two catalog references now lag:
`catalog/skills/grim-authoring/references/agent-spec.md` (around line 91)
inlines the `codex.reasoning-effort` literal list without `persistent`,
and `references/mcp-spec.md` (around line 64) lists `timeout` as
projecting for Claude and OpenCode only (Gemini, Qoder and now Codex also
project it). Tier 3 content should link the docs anchors instead of
inlining the lists (`catalog/README.md` › Content drift tiers). Fix both in
a catalog release.

## Friction

- **Deep-pass codex note was wrong:** the "PATH aliases under /tmp"
  warning fires for any `CODEX_HOME` below `/tmp`, and the session
  scratchpad is under `/tmp`, so nesting does not avoid it. It is benign.
  Fixed in `deep-pass.md`.
- **Deep-pass recipe missed the trust gate:** Codex ignores an untrusted
  project's `.codex/config.toml`, so the project `codex mcp list` showed
  nothing until the sandbox `config.toml` marked the project trusted.
  Fixed in `deep-pass.md`.
- **No skill or agent verb recorded for Codex:** `codex debug prompt-input`
  lists loaded skills offline; agents have no headless signal. Recorded
  in `deep-pass.md`.
- **Telemetry opt-outs were "confirm in the pass":** now the three `-c`
  keys. Fixed in `deep-pass.md`. (A shell that does not word-split an
  unquoted variable passed all three as one value; the recipe spells the
  flags out.)
- **First-pass feed note:** the researcher brief asks "Cursor found" even
  when the ledger cursor is `—`. Fixed: the brief says to report `n/a`
  then.
- Undated module doc in `vendor_codex.rs` (stale report blind to it) —
  dated in this pass.
- No researcher retries; no unsourced claims.

## Skill changes

`chore(skills)` commit after this record: `deep-pass.md` codex row
(telemetry opt-outs as `-c` flags, `codex debug prompt-input` for skills)
and codex note (benign `/tmp` warning, trusted-project entry, agents
`skip`, no `codex doctor`); `researcher-brief.md` (`Cursor found: n/a`
on a first pass).
