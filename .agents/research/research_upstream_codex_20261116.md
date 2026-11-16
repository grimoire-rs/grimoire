# Research: upstream refresh — codex (sweep)

## Metadata
**Date:** 2026-11-16 · **Domain:** vendors · **Depth:** sweep — checked 50d ago, Tier 1, deep due
**Triggered by:** /upstream-refresh
**Expires:** 2027-05-16

## Claims

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | Glob-scoped rules: no path-glob scoping (AGENTS.md per directory) | .agents/upstream-watchlist.md:15 | kind_support false for Rule; verified 2026-09-27 0.157.1 | unchanged | https://learn.chatgpt.com/docs/agent-configuration/agents-md | In each directory along the path, it checks for AGENTS.override.md, then AGENTS.md, then any fallback names in project_doc_fallback_filenames. Codex includes at most one file per directory. | docs live 2026-11-16; schema/source rust-v0.169.0 |
| 2 | openai.yaml skill sidecar documented, optional, fail-open | .agents/upstream-watchlist.md:19 | interface.*, policy.{allow_implicit_invocation,products}, dependencies.tools[] | unchanged | https://learn.chatgpt.com/docs/build-skills | allow_implicit_invocation (default: true ): When false | docs live 2026-11-16 |
| 3 | nickname_candidates: non-empty, unique, ASCII letters/digits/space/-/_ | .agents/upstream-watchlist.md:27 | as stated | unchanged | https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/agent-roles/src/agent_role_config.rs | may only contain ASCII letters, digits, spaces, hyphens, and underscores | rust-v0.169.0 |
| 4 | MCP transports: only Stdio and StreamableHttp (no ws/sse) | .agents/upstream-watchlist.md:36 | as stated | unchanged | https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/config/src/mcp_types.rs | pub enum McpServerTransportConfig {  Stdio {  ...  StreamableHttp { | rust-v0.169.0 |
| 5 | MCP auth enum oauth\|chatgpt\|ema_auth; oauth table client_id, client_secret, callback_url, callback_port, authorization_server_issuer; scopes, oauth_resource | .agents/upstream-watchlist.md:40 | as stated | unchanged (oauth table also gained token_endpoint_auth_method: client_secret_basic\|client_secret_post; additive, grim declines the whole oauth block) | https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/core/config.schema.json | McpServerAuth: oneOf enum oauth, chatgpt, ema_auth; McpServerOAuthConfig properties: authorization_server_issuer, callback_port, callback_url, client_id, client_secret, token_endpoint_auth_method | rust-v0.169.0 |
| 6 | MCP startup_timeout_ms (u64) and startup_timeout_sec (f64) both accepted | .agents/upstream-watchlist.md:41 | _sec wins when both set | unchanged (schema: startup_timeout_ms u64, startup_timeout_sec double; which wins not re-read) | https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/core/config.schema.json | "startup_timeout_ms": {"default": null, "format": "uint64" | rust-v0.169.0 |
| 7 | MCP cwd is stdio-only, fails the config on a url server | .agents/upstream-watchlist.md:42 | as stated (relative-resolution half not re-verified) | unchanged (stdio-only half) | https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/config/src/mcp_types.rs | throw_if_set("streamable_http", "cwd", cwd.as_ref())?; | rust-v0.169.0 |
| 8 | MCP http_headers_helper exists (0.148+), cached helper | .agents/upstream-watchlist.md:43 | as stated (cleared env, 401/403 refresh, Authorization-wins halves not re-read) | unchanged (existence + cache) | https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/rmcp-client/src/http_headers.rs | const HELPER_TIMEOUT: Duration = Duration::from_secs(10);  ...  cache: Mutex<HeadersCache>, | rust-v0.169.0 |
| 9 | codex.reasoning-effort literals incl. persistent, ultra, max | .agents/upstream-watchlist.md:44; docs vendor-metadata.md:318 | as stated | unchanged | https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/protocol/src/openai_models.rs | Self::Max => "max",  Self::Ultra => "ultra",  Self::Persistent => "persistent", | rust-v0.169.0 |
| 10 | CODEX_HOME replaces ~/.codex (AGENTS.md, config.toml); skills scanned from $HOME/.agents/skills + deprecated $CODEX_HOME/skills | .agents/upstream-watchlist.md:166; clients.md; vendor-metadata.md:491,521-526 | as stated | unchanged | https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/ext/skills/src/host_roots.rs | Deprecated user skills location (`$CODEX_HOME/skills`), kept for backward compatibility. | rust-v0.169.0 |
| 11 | Marketplace registration: system, cloud-managed, user or trusted-project config.toml; project config only when trusted | .agents/upstream-watchlist.md:287 | as stated | unchanged | https://learn.chatgpt.com/docs/config-file/config-reference | Marketplaces can be defined in system, cloud-managed, user, or trusted-project config.toml. | docs live 2026-11-16 |
| 12 | Hooks run through $SHELL -lc (shell form only, no exec form) | .agents/upstream-watchlist.md:242 | as stated | unchanged | https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/hooks/src/engine/command_runner.rs | command.arg("-lc");  ...  ("SHELL", "/bin/sh") | rust-v0.169.0 |
| 13 | Client-side hook approval: non-managed hooks need review/trust before running | .agents/upstream-watchlist.md:243 | as stated (the 'silently skipped' half not re-verified; docs say Codex prints a startup warning pointing to /hooks) | unchanged (approval); note startup warning | https://learn.chatgpt.com/docs/hooks | Before a non-managed hook can run, Codex requires you to review and trust the exact hook definition. | docs live 2026-11-16 |
| 14 | Project-scope hooks: project hooks load only when .codex layer trusted (grim keeps global scope only) | .agents/upstream-watchlist.md:244; src/install/vendor_codex.rs:251,683 | global scope only | unchanged | https://learn.chatgpt.com/docs/hooks | Project-local hooks load only when the project .codex/ layer is trusted. | docs live 2026-11-16 |
| 15 | Rules file: Codex scans rules/ under every active config layer at startup; restart needed; decision allow\|prompt\|forbidden | .agents/upstream-watchlist.md:286; settings.md:561,172,1025 | as stated (bad-file session behaviour still unverified) | unchanged | https://learn.chatgpt.com/docs/agent-configuration/rules | Restart Codex. Codex scans rules/ under every active config layer at startup, including Team Config locations and the user layer at ~/.codex/rules/ . | docs live 2026-11-16 |
| 16 | Rules decision values: allow, prompt, forbidden | settings.md:172 | as stated | unchanged | https://learn.chatgpt.com/docs/agent-configuration/rules | forbidden > prompt > allow | docs live 2026-11-16 |
| 17 | Settings key /model (string) | src/install/vendor_codex.rs:115;settings.md:604 | string scalar | unchanged | https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/core/config.schema.json | "model": {"description": "Optional override of model selection.", "type": "string"} | rust-v0.169.0 |
| 18 | Settings key /model_reasoning_effort (string) | src/install/vendor_codex.rs:116;settings.md:605 | string scalar | unchanged | https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/core/config.schema.json | ReasoningEffort: {"description": "A non-empty reasoning effort value advertised by the model.", "minLength": 1, "type": "string"} | rust-v0.169.0 |
| 19 | Settings key /approval_policy enum on-request\|never | src/install/vendor_codex.rs:117;settings.md:606 | enum on-request, never | unchanged (granular object form exists; grim does not write it) | https://learn.chatgpt.com/docs/config-file/config-reference | untrusted is unsupported, and on-failure is deprecated; use on-request for interactive runs or never for non-interactive runs. | docs live 2026-11-16 |
| 20 | Settings key /sandbox_mode enum read-only\|workspace-write\|danger-full-access | src/install/vendor_codex.rs:122;settings.md:607 | as stated | unchanged | https://learn.chatgpt.com/docs/config-file/config-reference | read-only \| workspace-write \| danger-full-access Sandbox policy for filesystem and network access during command execution. | docs live 2026-11-16 |
| 21 | Settings keys /sandbox_workspace_write/{network_access bool, writable_roots set, exclude_slash_tmp bool, exclude_tmpdir_env_var bool} | src/install/vendor_codex.rs:127-147;settings.md:608-611 | as stated | unchanged | https://learn.chatgpt.com/docs/config-file/config-reference | sandbox_workspace_write.writable_roots array<string> Additional writable roots when sandbox_mode = "workspace-write" . | docs live 2026-11-16 |
| 22 | Settings key /default_permissions (string) | src/install/vendor_codex.rs:148;settings.md:612 | string scalar | unchanged | https://learn.chatgpt.com/docs/config-file/config-reference | default_permissions string Name of the default permissions profile to apply to sandboxed tool calls. | docs live 2026-11-16 |
| 23 | Settings key /permissions/<name> map with description, extends, filesystem, network, workspace_roots | src/install/vendor_codex.rs:149;settings.md:613 | as stated | unchanged | https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/core/config.schema.json | PermissionProfileToml: properties description, extends, filesystem, network, workspace_roots (additionalProperties false) | rust-v0.169.0 |
| 24 | Settings key /tui/alternate_screen enum auto\|always\|never | src/install/vendor_codex.rs:157;settings.md:614 | as stated | unchanged | https://learn.chatgpt.com/docs/config-file/config-reference | tui.alternate_screen auto \| always \| never Control alternate screen usage for the TUI | docs live 2026-11-16 |
| 25 | Settings keys /tui/animations, /tui/auto_recap, /tui/raw_output_mode (bool) | src/install/vendor_codex.rs:163-165;settings.md:615-617 | as stated | unchanged | https://learn.chatgpt.com/docs/config-file/config-reference | tui.raw_output_mode boolean Start the TUI in raw scrollback mode for copy-friendly terminal selection (default: false). | docs live 2026-11-16 |
| 26 | Settings key /history/persistence enum save-all\|none | src/install/vendor_codex.rs:166;settings.md:618 | as stated | unchanged | https://learn.chatgpt.com/docs/config-file/config-reference | history.persistence save-all \| none Control whether Codex saves session transcripts to history.jsonl. | docs live 2026-11-16 |
| 27 | Settings key /hide_agent_reasoning (bool) | src/install/vendor_codex.rs:172;settings.md:619 | as stated | unchanged | https://learn.chatgpt.com/docs/config-file/config-reference | hide_agent_reasoning boolean Suppress reasoning events in both the TUI and codex exec output. | docs live 2026-11-16 |
| 28 | Settings key /marketplaces/<name> map with source, source_type, ref, sparse_paths | src/install/vendor_codex.rs:173;settings.md:620 | as stated (schema also has Codex-written last_revision, last_updated) | unchanged | https://learn.chatgpt.com/docs/config-file/config-reference | marketplaces.<name>.source_type git \| local Source kind for a configured plugin marketplace. | docs live 2026-11-16 |
| 29 | Settings key rules (own file, pattern/decision/justification) | src/install/vendor_codex.rs:181;settings.md:621 | as stated | unchanged | https://learn.chatgpt.com/docs/agent-configuration/rules | decision (defaults to "allow" ) : The action to take when the rule matches. | docs live 2026-11-16 |
| 30 | Settings key /file_opener enum vscode\|vscode-insiders\|windsurf\|cursor\|none | src/install/vendor_codex.rs:191;settings.md:622;.agents/upstream-watchlist.md:294 | as stated | unchanged | https://learn.chatgpt.com/docs/config-file/config-reference | file_opener vscode \| vscode-insiders \| windsurf \| cursor \| none URI scheme used to open citations from Codex output (default: vscode ). | docs live 2026-11-16 |
| 31 | Settings key /tui/notification_method enum auto\|osc9\|bel | src/install/vendor_codex.rs:196;settings.md:623;.agents/upstream-watchlist.md:294 | auto, osc9, bel | auto, osc9, osc777, bel (osc777 added 0.168.0; the docs reference page still lists auto \| osc9 \| bel) | https://github.com/openai/codex/releases/tag/rust-v0.168.0 | Set `tui.notification_method = "osc777"` for terminal notifications. | rust-v0.168.0 (schema rust-v0.169.0: NotificationMethod enum ["auto","osc9","osc777","bel"]) |
| 32 | Settings key /tui/notification_condition enum unfocused\|always | src/install/vendor_codex.rs:200;settings.md:624 | as stated | unchanged | https://learn.chatgpt.com/docs/config-file/config-reference | tui.notification_condition unfocused \| always Control whether TUI notifications fire only when the terminal is unfocused or regardless of focus. | docs live 2026-11-16 |
| 33 | Settings key /web_search enum disabled\|cached\|indexed\|live | src/install/vendor_codex.rs:204;settings.md:625 | as stated | unchanged | https://learn.chatgpt.com/docs/config-file/config-reference | web_search disabled \| cached \| indexed \| live Web search mode | docs live 2026-11-16 |
| 34 | Vendor mapping stamp: 'Mapping verified 2026-09-27 against Codex CLI 0.157.1' | src/install/vendor_codex.rs:5 | 0.157.1 | unchanged content; stamp stays 0.157.1 until the osc777 row lands (feed shows 0.169.0 now). 0.158-0.169 notes touch no skill/agent/MCP/rules key grim renders | https://github.com/openai/codex/releases/tag/rust-v0.169.0 | `codex exec` keeps project configuration and MCP servers disabled in untrusted directories, even with `--sandbox workspace-write`. | rust-v0.169.0 |
| 35 | Codex plugin marketplace shape: name, interface.displayName, plugins; entry category | .agents/upstream-watchlist.md:46,61 | as stated | unchanged | https://developers.openai.com/plugins/build/plugins | { "name" : "local-example-plugins" , "interface" : { "displayName" : "Local Example Plugins" }, "plugins" : [ | docs live 2026-11-16 |

## Verifier sign-off

Signed off 2026-11-16 by three read-only sonnet verifiers (upstream re-fetched, grim code read) and by the implementing workers; one line per code-driving claim.

- #31 `/tui/notification_method` `osc777`: confirmed (rust-v0.168.0 release note, rust-v0.169.0 schema enum); landed in `58754de8`.

## Routing

- #5 MCP auth enum oauth/chatgpt/ema_auth; oauth table client_id, client_secret,... → (a) ledger or doc text fix
- #6 MCP startup_timeout_ms (u64) and startup_timeout_sec (f64) both accepted → (a) ledger or doc text fix
- #7 MCP cwd is stdio-only, fails the config on a url server → (a) ledger or doc text fix
- #8 MCP http_headers_helper exists (0.148+), cached helper → (a) ledger or doc text fix
- #13 Client-side hook approval: non-managed hooks need review/trust before running → (a) ledger or doc text fix
- #19 Settings key /approval_policy enum on-request/never → (a) ledger or doc text fix
- #31 Settings key /tui/notification_method enum auto/osc9/bel → (b) landed in `58754de8`
- #34 Vendor mapping stamp: 'Mapping verified 2026-09-27 against Codex CLI 0.157.1' → (a) ledger or doc text fix

## Issue drafts

none

## Friction

- dropped (unsourced): Config-string expansion (does Codex expand `${VAR}`/file refs in config.toml strings?) — config reference and schema searched for substitution wording; none found, but absence not proven. No claim row.
- dropped (unsourced): Trust-gate keys — config reference line 1038 shows `trust_level = "untrusted"` under `projects.<path>` (a folder pre-trust key); grim proposes no row for it. No new gate-disabling key found, but not exhaustively enumerated.
- dropped (unsourced): `.agents/upstream-watchlist.md:286` session behaviour of a malformed `.rules` file — verified only by run on 0.166.0; no primary doc states it.
- dropped (unsourced): `.agents/upstream-watchlist.md:42` relative `cwd` resolution and `:43` cleared-env/401-403/Authorization-wins halves of `http_headers_helper` — source files fetched, only the stdio-only and cache halves confirmed; rest not read.
- dropped (unsourced): `.agents/upstream-watchlist.md:243` "skipped silently" half — docs only say a startup warning points to /hooks.
- not reached: `.agents/upstream-watchlist.md:12` Plugin mcp.json env expansion (Codex: PLUGIN_ROOT/PLUGIN_DATA; hooks docs mention PLUGIN_ROOT/PLUGIN_DATA and CLAUDE_PLUGIN_* compatibility, but §9.2 of the agent-plugins spec not re-read)
- not reached: `.agents/upstream-watchlist.md:17` shared-pool auto-join (Codex side rests on row 10)
- not reached: `.agents/upstream-watchlist.md:45,60,62,64` plugin logo, marketplace metadata, Codex plugin `interface` (recorded 2026-10-17/29, not re-fetched; row 32 covers the marketplace shape only)
- not reached: `.agents/upstream-watchlist.md:217` private clone (marked Unverified)
- not reached: `.agents/upstream-watchlist.md:235,239` Codex mentions in hook-kind rows (Codex PostToolUse blocking verdict not re-checked)
- not reached: docs statements in clients.md:228-237, 318-335, mcp-servers.md:220-221,269-281,315,528-534 (all rest on rows above; header-surface claim `http_headers`/`env_http_headers`/`bearer_token_env_var` confirmed in RawMcpServerConfig schema properties but not given its own row)
- feed: `gh` releases API flaky (HTTP 504 on some pages), retried; patch tags not read; docs quotes are HTML stripped to text.
- feed: newest rust-v0.169.0 (2026-11-13), cursor found yes

## Skill changes

Host caps concurrent subagents at 20; the researcher built its own claim list; replies are a short summary with the table in a file. Fixed in this run's chore(skills) commit.
