# Research: OTEL prior art — metric, event and attribute catalogs of agent CLIs

<!--
Technology Landscape Research
Filename: .agents/research/research_otel_prior_art_signal_catalog.md
Owner: Researcher (hex research phase)
Handoff to: Architect (/hex-architect), /hex-plan
Related Skills: hex-architect, hex-plan

Neutral evidence only: the requester asked for findings, negatives and leads with no
recommendation. Sources older than ~18 months (before 2025-04-22) are marked [STALE].
Extends (does not redo) research_otel_cli_prior_art.md, research_otel_community_threads.md.
-->

## Metadata

**Date:** 2026-10-22
**Domain:** observability
**Triggered by:** Owner follow-up in `.agents/discussions/otel-usage-telemetry.md` ("Signals: events and metrics" and "Context collection", both open at lines 118-139): what do events carry beside spans, what is the rough metric set, how is context collected, and is a git repository auto-detected. This lane catalogs what five comparable tools emit and attach; grim-side design is out of scope here.
**Expires:** 2027-04-22 (metric and attribute names in all five tools change release to release; Claude Code docs cite attributes gated by version numbers such as v2.1.269 and v2.1.287)

## Direct Answer

No recommendation (out of scope for this lane). What the five tools do, in answer to the owner's three questions:

1. **Events beside spans.** Four of five tools emit OTLP log-record events as a primary signal: Claude Code 28 named events, Gemini CLI 41 listed, Codex 8 documented plus more in source, Copilot/VS Code 10. Spans exist in all four but are secondary or gated: Claude Code spans are Beta behind two extra flags, Gemini CLI traces are off by default, Codex has a separate `trace_exporter`, Copilot has four span kinds. Docker CLI, the one short-lived non-agent CLI, emits no events and no spans: a single metric. For "a package of config was installed or loaded", the nearest precedents are events: Claude Code `claude_code.plugin_installed` / `plugin_loaded` / `skill_activated`, Codex `codex.skill_invocation`, Gemini CLI `gemini_cli.extension_install`. No tool documents why it chose events over spans or metrics (finding 7).
2. **Roughly planned metrics.** Other tools' counts: Claude Code 8, Gemini CLI 27 + 2 `gen_ai.*`, Codex 66 name constants (10 in its docs table), Copilot 20, Docker CLI 1. The shared shape: a session counter, a duration histogram or counter in ms or s, token and tool-call counters. The process-lifetime analogues of grim are `claude_code.session.count`, `codex.process.start` and Docker's `command.time`. Full catalog in **Key Findings 1-5**.
3. **Context collection and git auto-detection.** Auto-detection exists, with opposite defaults: Claude Code derives `vcs.repository.url.full` / `vcs.owner.name` / `vcs.repository.name` / `vcs.provider.name` from the `origin` remote once per session, **off by default** (`OTEL_METRICS_INCLUDE_REPOSITORY=false`, v2.1.269+); Copilot attaches `github.copilot.git.repository`, `.branch`, `.commit_sha` to the `invoke_agent` span "when in a Git repo", with **no documented opt-in** (content and identity capture have switches, git data is not listed as gated). Codex, Gemini CLI and Docker CLI attach no git or cwd context in their OTel output. No tool attaches cwd as an attribute; Claude Code's only path-like field is `workspace.host_paths` (desktop app, events only, never metrics).

## Technology Landscape

### Trending (gaining momentum)

| Tool/Pattern | Adoption Signal | Key Benefit | Relevance to Grimoire |
|-------------|----------------|-------------|-------------------|
| Operator-scoped repo identity as opt-in resource/event attributes (`vcs.*`) | Claude Code v2.1.269+ ([docs](https://code.claude.com/docs/en/monitoring-usage)); open request to add branch to every event ([anthropics/claude-code#102277](https://github.com/anthropics/claude-code/issues/102277), opened 2026-10-15, no maintainer reply) | Per-repo and per-ticket attribution at a shared collector | The discussion's git question (`otel-usage-telemetry.md:136-139`) has a live analogue |
| Hash/placeholder redaction of config-package names in events | Claude Code `plugin_id_hash`, `"third-party"` and `"custom_skill"` placeholders unless `OTEL_LOG_TOOL_DETAILS=1`; Copilot `mcp_server_name_hash` (SHA-256) | Fleet inventory without recording names | Direct analogue for artifact refs in `grim.artifact.changed` |
| OTel GenAI semconv (`gen_ai.*`) | Gemini CLI and Copilot emit it ([Gemini docs](https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/telemetry.md), [VS Code guide](https://code.visualstudio.com/docs/copilot/guides/monitoring-agents)) | Backend dashboards work across agents | Not applicable to a package manager (no LLM calls); shows how others pick a semconv |

### Established (proven, widely accepted)

| Tool/Pattern | Status | Notes |
|-------------|--------|-------|
| `OTEL_RESOURCE_ATTRIBUTES` as the operator's way to add static attributes | Honored by Claude Code, Copilot, Docker CLI; not documented for Codex or Gemini CLI | Finding 6 |
| Random, locally persisted install/user ID | Claude Code `user.id` (`~/.claude.json`), Gemini CLI `installation.id` (`randomUUID()`) | Finding 3 |
| Delta temporality for short-lived metric emitters | Docker CLI (manual reader, delta), Codex (delta default), Claude Code (delta default per prior research) | [Docker source](https://raw.githubusercontent.com/docker/cli/master/cli/command/telemetry.go) |

### Emerging (early but promising)

| Tool/Pattern | Signal | Worth Watching Because |
|-------------|--------|----------------------|
| Hooks/plugins asking to enrich native spans with delivery context | [claude-code#97772](https://github.com/anthropics/claude-code/issues/97772) (2026-09-28), [github/copilot-cli#4967](https://github.com/github/copilot-cli/issues/4967) (2026-09-25), both open, no maintainer reply | Shows operators find static `OTEL_RESOURCE_ATTRIBUTES` insufficient |
| Log-only vs trace-safe event targets (privacy tiering by signal) | Codex `log_event!` vs `trace_event!` macros | Finding 3 |

### Declining (losing mindshare)

| Tool/Pattern | Signal | Avoid Because |
|-------------|--------|---------------|
| Legacy Copilot repo attributes `copilot_chat.repo.remote_url`, `.head_branch_name`, `.head_commit_hash` | Labeled "Legacy" next to `github.copilot.git.*` in the [VS Code guide](https://code.visualstudio.com/docs/copilot/guides/monitoring-agents) | Renamed once already |
| `vcs.repository.ref.*` semconv keys | Deprecated in favor of `vcs.ref.head.*` ([registry](https://opentelemetry.io/docs/specs/semconv/registry/attributes/vcs/)); all remaining `vcs.*` keys are Release Candidate | Names moved within 12 months |

## Design Patterns Worth Considering

- **Gate identity per signal target, not per attribute.** Codex tags log events with `user.account_id` and `user.email` and strips both from the trace-safe variant ([events/shared.rs](https://raw.githubusercontent.com/openai/codex/main/codex-rs/otel/src/events/shared.rs)).
- **One switch for a whole identity class.** Copilot `captureIdentity` / `COPILOT_OTEL_CAPTURE_IDENTITY` (default false) covers account name, OS username, hostname; separate from content capture ([VS Code guide](https://code.visualstudio.com/docs/copilot/guides/monitoring-agents)).
- **Per-attribute cardinality toggles that apply to metrics only.** Claude Code `OTEL_METRICS_INCLUDE_{SESSION_ID,VERSION,ACCOUNT_UUID,ENTRYPOINT,RESOURCE_ATTRIBUTES,REPOSITORY}`; events keep the attributes regardless ([docs](https://code.claude.com/docs/en/monitoring-usage)).
- **Name redaction to a placeholder plus a hash.** Claude Code replaces non-official plugin, skill, agent and workflow names with `"third-party"` / `"custom_skill"` / `"custom"` unless `OTEL_LOG_TOOL_DETAILS=1`, and ships `plugin_id_hash` "sent only to your configured exporter" so fleets can count distinct plugins ([docs](https://code.claude.com/docs/en/monitoring-usage)).
- **Operator override of auto-detection.** Declaring `vcs.repository.url.full` in `OTEL_RESOURCE_ATTRIBUTES` stops Claude Code reading the remote at all ([docs](https://code.claude.com/docs/en/monitoring-usage)).
- **Repo files cannot turn export on.** Claude Code ignores `CLAUDE_CODE_ENABLE_TELEMETRY` in project/local settings ([env-vars](https://code.claude.com/docs/en/env-vars)); Codex says project-local `.codex/config.toml` cannot set `otel` keys ([config-reference](https://learn.chatgpt.com/docs/config-file/config-reference)).
- **Bounded exit.** Docker CLI: manual reader, delta, flush in `Shutdown`, `exportTimeout = 50 * time.Millisecond` ([telemetry.go](https://raw.githubusercontent.com/docker/cli/master/cli/command/telemetry.go)). Codex: shutdown on a pre-started detached thread `"codex-otel-shutdown"` with `shutdown_with_timeout` ([provider.rs](https://raw.githubusercontent.com/openai/codex/main/codex-rs/otel/src/provider.rs)).

## Key Findings

### 1. Claude Code (docs: [monitoring-usage](https://code.claude.com/docs/en/monitoring-usage), undated, cites v2.1.287; read in full across three fetches)

Gate: `CLAUDE_CODE_ENABLE_TELEMETRY=1` then per-signal exporters; resource attributes `service.name` (`claude-code`, or `claude-code-desktop`), `service.version`, `os.type`, `os.version`, `host.arch`, `wsl.version`; meter `com.anthropic.claude_code`. The page does not name instrument types; descriptions say "count".

**Metrics (8)**

| Name | Unit | Own attributes |
|---|---|---|
| `claude_code.session.count` | none | `start_type` (`fresh`, `resume`, `continue`, `agents_view`) |
| `claude_code.lines_of_code.count` | none | `type` (`added`/`removed`), `model` |
| `claude_code.pull_request.count` | none | standard only |
| `claude_code.commit.count` | none | standard only |
| `claude_code.cost.usage` | USD | `model`, `query_source`, `speed`, `effort`, `agent.name`, `skill.name`, `plugin.name`, `marketplace.name`, `mcp_server.name`, `mcp_tool.name` |
| `claude_code.token.usage` | tokens | `type` (`input`/`output`/`cacheRead`/`cacheCreation`) + the `cost.usage` set |
| `claude_code.code_edit_tool.decision` | none | `tool_name`, `decision`, `source`, `language` |
| `claude_code.active_time.total` | s | `type` (`user`/`cli`) |

**Log-record events (28 named; each carries standard attributes plus `event.name`, `event.timestamp`, `event.sequence`).** `user_prompt`, `assistant_response`, `tool_result`, `tool_decision`, `api_request`, `api_error`, `api_refusal`, `api_retries_exhausted`, `api_request_body`, `api_response_body`, `permission_mode_changed`, `auth`, `mcp_server_connection`, `internal_error` (class name and errno only; "message and stack trace are never included"), `plugin_installed` (`marketplace.is_official`, `install.trigger` `cli|ui`, `plugin.name`, `plugin.version`, `marketplace.name`; last three only with `OTEL_LOG_TOOL_DETAILS=1` for third-party marketplaces), `plugin_loaded` (once per enabled plugin at session start: `plugin.scope` `official|community|org|user-local|default-bundle`, `enabled_via`, `plugin_id_hash`, `has_hooks`, `has_mcp`, `skill_path_count`, `command_path_count`, `agent_path_count`, `safe_mode`), `skill_activated` (`skill.name`, `invocation_trigger`, `skill.source`), `at_mention`, `hook_registered`, `hook_execution_start`, `hook_execution_complete`, `hook_plugin_metrics`, `compaction`, `subagent_completed`, `feedback_survey`, `retention_sweep`, `managed_settings_resolved`, `system_prompt`. All prefixed `claude_code.`.

**Spans (Beta; needs `CLAUDE_CODE_ENABLE_TELEMETRY=1`, `CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1`, `OTEL_TRACES_EXPORTER`).** `claude_code.interaction` > `claude_code.llm_request`, `claude_code.hook`, `claude_code.tool` > `claude_code.tool.blocked_on_user`, `claude_code.tool.execution`. A `tool.output` span event exists behind `OTEL_LOG_TOOL_CONTENT=1`.

### 2. Gemini CLI (docs: [telemetry.md](https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/telemetry.md) on `main`, undated; source files read via raw GitHub, partly summarized)

Gate: `telemetry.enabled` / `GEMINI_TELEMETRY_ENABLED` (default false); traces separately `GEMINI_TELEMETRY_TRACES_ENABLED` (default false). Destination `GEMINI_TELEMETRY_OTLP_ENDPOINT`, falling back to `OTEL_EXPORTER_OTLP_ENDPOINT` per prior research. `logPrompts` defaults **true** per the docs table.

- **Metrics (27 `gemini_cli.*` + 2 `gen_ai.client.*`).** `session.count`, `onboarding.start`, `onboarding.success` (`user_tier`), `tool.call.count`, `tool.call.latency` (ms), `api.request.count`, `api.request.latency` (ms), `token.usage` (`type`: input/output/thought/cache/tool), `file.operation.count`, `lines.changed`, `chat_compression`, `chat.invalid_chunk.count`, `chat.content_retry.count`, `chat.content_retry_failure.count`, `slash_command.model.call_count`, `model_routing.latency`, `model_routing.failure.count`, `agent.run.count`, `agent.duration`, `agent.turns`, `plan.execution.count`, `ui.flicker.count`, `startup.duration` (`phase`), `memory.usage`, `cpu.usage` (percentage), `tool.queue.depth`, `tool.execution.breakdown`, plus `gen_ai.client.token.usage`, `gen_ai.client.operation.duration` (s). Most types and units are not stated.
- **Log events (41 listed).** Session: `gemini_cli.config` (carries `extensions`, `mcp_servers`, `auth_type`, `worktree_active`, optional `github_*`), `gemini_cli.user_prompt`, `startup_stats` (`phases`, `os_platform`, `os_release`, `is_docker`). Tools: `tool_call`, `tool_output_truncated`. Extensions: `extension_install` (`extension_name`, `extension_version`, `extension_source`, `status`), `extension_uninstall`, `extension_enable`, `extension_disable`. Others: `api_request`, `api_response`, `api_error`, `slash_command`, `model_routing`, `agent.start`, `agent.finish`, `hook_call`, `ide_connection`, `rewind`, `conversation_finished`.
- **Traces.** Opt-in; the page lists `gen_ai.operation.name` values (`tool_call`, `llm_call`, `user_prompt`, `system_prompt`, `agent_call`, `schedule_tool_calls`) rather than span names.
- **Common attributes per source** ([telemetryAttributes.ts](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/telemetry/telemetryAttributes.ts)): `session.id`, `installation.id`, `interactive`, `user.email` (only if a Google account is cached), `auth_type` (if set), `experiments.ids` (if non-empty). **Docs and source disagree**: the docs say `active_approval_mode` is attached to all data; that key is not in `getCommonAttributes`. Not reconciled.
- **Resource per source** ([sdk.ts](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/telemetry/sdk.ts)): `service.name`, `service.version` set to `process.version` (the Node runtime version, not the CLI version), `session.id`.

### 3. Codex CLI (docs: [config-advanced](https://learn.chatgpt.com/docs/config-file/config-advanced), [config-reference](https://learn.chatgpt.com/docs/config-file/config-reference), undated; source: `codex-rs/otel` on `main`)

Gate: `[otel]` in `config.toml`; `exporter = none | otlp-http | otlp-grpc` (events/logs), `trace_exporter`, `metrics_exporter` (`none | statsig | otlp-http | otlp-grpc`, **default `statsig`**, an OTLP/HTTP-JSON shorthand to Statsig per the [README](https://github.com/openai/codex/blob/main/codex-rs/otel/README.md)); `analytics.enabled = false` also disables custom OTLP metrics exporters in Codex CLI. Project-local `.codex/config.toml` cannot set `otel`.

- **Events documented** (8): `codex.conversation_starts`, `codex.api_request`, `codex.sse_event`, `codex.websocket_request`, `codex.websocket_event`, `codex.user_prompt` (length; content only with `otel.log_user_prompt`), `codex.tool_decision`, `codex.tool_result`. README adds `codex.skill_invocation` (`skill.name`, `skill.invocation_type` `explicit|implicit`, `conversation.id`, `turn.id`; "excludes skill contents, prompts, and tool output"), `codex.agent_response` and `codex.guardian_assessment` (both opt-in). Source also logs `codex.startup_phase`, `codex.turn_ttft`, `codex.turn_cost`, `codex.websocket_connect`, `codex.auth_recovery`, `codex.sandbox_outcome`, `codex.plugin_install_*` ([session_telemetry.rs](https://raw.githubusercontent.com/openai/codex/main/codex-rs/otel/src/events/session_telemetry.rs)).
- **Metrics.** The docs table lists 10 (`codex.api_request` and `.duration_ms`, `codex.sse_event` and `.duration_ms`, `codex.websocket.request` / `.event` and `.duration_ms`, `codex.tool.call` and `.duration_ms`; counters and histograms; default tags `auth_mode`, `originator`, `session_source`, `model`, `app.version`). Source ([names.rs](https://raw.githubusercontent.com/openai/codex/main/codex-rs/otel/src/metrics/names.rs)) has 66 name constants, including process-lifetime ones: `codex.process.start`, `codex.startup.phase.duration_ms`, `codex.thread.started`, `codex.hooks.run`, `codex.hooks.run.duration_ms`, `codex.thread.skills.enabled_total`, `codex.plugins.startup_sync`, `codex.turn.cost_microusd`.
- **Common event fields** (log variant, from `log_event!`): `event.timestamp`, `conversation.id`, `app.version`, `auth_mode`, `originator`, `user.account_id`, `user.email`, `terminal.type`, `model`, `slug`. The trace-safe variant omits exactly `user.account_id` and `user.email`. Metric tags are narrower (`session_source` and the list above), so account and email never reach metrics.
- **Resource** ([provider.rs](https://raw.githubusercontent.com/openai/codex/main/codex-rs/otel/src/provider.rs)): `service.name`, `service.version`, `env` (from `otel.environment`, default `dev`), and `host.name` **for logs only**, when non-empty. No `OTEL_RESOURCE_ATTRIBUTES` read in the files examined. `OtelSettings` ([config.rs](https://raw.githubusercontent.com/openai/codex/main/codex-rs/otel/src/config.rs)) has no git, repo or cwd field; closest is `codex_home`.

### 4. GitHub Copilot: VS Code extension and CLI ([VS Code guide](https://code.visualstudio.com/docs/copilot/guides/monitoring-agents), dated 2026-10-21)

Activation per the guide: `github.copilot.chat.otel.enabled`, `COPILOT_OTEL_ENABLED=true`, **or** `OTEL_EXPORTER_OTLP_ENDPOINT` set. The CLI-side docs are thinner ([GitHub docs](https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/configure-copilot-cli): only "OpenTelemetry exports configured by you or your enterprise"); [Langfuse](https://langfuse.com/integrations/developer-tools/github-copilot) shows `COPILOT_OTEL_ENABLED=true` for the CLI and says it uses `http/json`, while the VS Code guide says default `http/protobuf`. Unreconciled.

- **Metrics (20).** GenAI: `gen_ai.client.operation.duration` (histogram, s), `gen_ai.client.token.usage` (histogram). Extension: `copilot_chat.tool.call.count` (counter), `copilot_chat.tool.call.duration` (histogram, ms), `copilot_chat.agent.invocation.duration` (histogram, s), `copilot_chat.agent.turn.count`, `copilot_chat.session.count` (counter), `copilot_chat.time_to_first_token`. Outcome: `copilot_chat.edit.acceptance.count`, `chat_edit.outcome.count`, `lines_of_code.count`, `edit.survival.four_gram`, `edit.survival.no_revert`, `user.action.count`, `user.feedback.count`, `agent.edit_response.count`, `agent.summarization.count`, `pull_request.count`, `cloud.session.count`, `cloud.pr_ready.count`.
- **Events (10).** `gen_ai.client.inference.operation.details`, `copilot_chat.session.start`, `copilot_chat.tool.call`, `copilot_chat.agent.turn`, `copilot_chat.edit.feedback`, `copilot_chat.edit.hunk.action`, `copilot_chat.inline.done`, `copilot_chat.edit.survival`, `copilot_chat.user.feedback`, `copilot_chat.cloud.session.invoke`.
- **Spans (4).** `invoke_agent`, `chat`, `execute_tool`, `execute_hook`. `execute_tool` carries `github.copilot.tool.parameters.skill_name`, `.edit_type`, `.mcp_tool_name`, `.mcp_server_name_hash` (SHA-256); raw `mcp_server_name`, `command`, `file_path` only under content capture. `execute_hook` carries `github.copilot.hook.decision`, `.duration`, `.tool_names`.
- **Resource.** `service.name` = `copilot-chat` (override `OTEL_SERVICE_NAME`), `service.version`, `session.id` (unique per VS Code window); `process.user.name` and `host.name` only with identity capture, Local harness sessions only. No OS attribute documented.

### 5. Docker CLI ([docs](https://docs.docker.com/engine/cli/otel/), Engine 26.1.0, 2024 [STALE]; source on `master` read directly)

- **One metric.** `command.time`, unit ms; the docs do not give a type. Source ([telemetry_utils.go](https://raw.githubusercontent.com/docker/cli/master/cli/command/telemetry_utils.go)) creates it as `meter.Float64Counter("command.time", ... WithUnit("ms"))`, i.e. a counter of milliseconds, not a histogram. **This corrects `research_otel_cli_prior_art.md:198`** ("histogram-style").
- **Attributes.** Documented: `command.name`, `command.status.code`, `command.stdin.isatty`, `command.stdout.isatty`, `command.stderr.isatty`. Source adds `command.error.type` (`generic` or `canceled`) when an error occurred; not in the docs page. `command.name` is built from `cmd.Name()` up the parent chain; argument values are not recorded.
- **Resource** ([telemetry.go](https://raw.githubusercontent.com/docker/cli/master/cli/command/telemetry.go)): `service.name` = `filepath.Base(os.Args[0])`, `service.instance.id` = a fresh `uuid.NewString()` per process, SDK attributes, plus `OTEL_RESOURCE_ATTRIBUTES` via `resource.WithFromEnv()`; entries with the `docker.cli.` prefix are filtered out of that variable.
- **Transport and timing.** Exporters are gRPC in source (`otlpmetricgrpc`, `otlptracegrpc`), manual reader with delta temporality, flush inside `Shutdown`, `exportTimeout = 50 ms` ([telemetry_docker.go](https://raw.githubusercontent.com/docker/cli/master/cli/command/telemetry_docker.go)). Destination is `DOCKER_CLI_OTEL_EXPORTER_OTLP_ENDPOINT` only; telemetry is off unless set. No git, cwd, user or host attribute.

### 6. Identity, host and context attributes: side by side

| Attribute class | Claude Code | Gemini CLI | Codex | Copilot (VS Code guide) | Docker CLI |
|---|---|---|---|---|---|
| Session | `session.id` (+ `ccr.session.id`); `OTEL_METRICS_INCLUDE_SESSION_ID`, default true, documented for metrics | `session.id` on resource and common set; `gen_ai.conversation.id` on traces | `conversation.id` on log and trace events, not a metric tag | `session.id` per VS Code window; `gen_ai.conversation.id` | `service.instance.id`, random per process |
| Anonymous install/user id | `user.id`, random, persisted in `~/.claude.json`, always included | `installation.id`, `randomUUID()`, always; fixed `'123456789'` if the file cannot be read/written | none found | none | none |
| Account / org ids | `user.account_uuid`, `user.account_id` (toggle `OTEL_METRICS_INCLUDE_ACCOUNT_UUID`, default true), `organization.id` always when available | `auth_type` only | `user.account_id`, log events only | `captureIdentity` (default false) | none |
| Email | `user.email`, "always included when available"; no off switch documented, the page advises the backend "filter or redact" | `user.email` if cached; no switch seen | `user.email`, log events only | inside `captureIdentity` | none |
| Host / OS | resource `os.type`, `os.version`, `host.arch`, `wsl.version`; `host.name` not standard, set by the operator via `OTEL_RESOURCE_ATTRIBUTES` (page sentences on `host.name` are ambiguous) | `startup_stats` event: `os_platform`, `os_release`, `is_docker` | `host.name` on log resource only | `host.name`, `process.user.name` only with `captureIdentity` | none |
| Terminal / surface | `terminal.type` always when detected; `app.entrypoint` toggle default false | `interactive`; `GEMINI_CLI_SURFACE` appended to User-Agent | `terminal.type`, `originator`, `session_source` | `gen_ai.agent.name` (`copilot`, `copilotcli`, `claude`) | `command.std{in,out,err}.isatty` |
| App version | `app.version` toggle default **false**; `service.version` always | `service.version` = Node version | `app.version` on events and metrics; `service.version` | `service.version` | none (instrumentation version only) |
| Model | `model` on API metrics/events | `model` | `model`, `slug` | `gen_ai.request.model` | n/a |
| cwd / path | none; `workspace.host_paths` on events only (desktop) | none | none | not stated | none |
| Git repo | `vcs.repository.url.full`, `vcs.owner.name`, `vcs.repository.name`, `vcs.provider.name`; **off by default**; from `origin`, once per session | none in OTel output; `gemini_cli.config` has optional `github_repository_hash`, `github_workflow_name`, `github_event_name`, `github_pr_number` (derivation not located) | none | `github.copilot.git.repository`, `github.copilot.github.org` on `invoke_agent`; no gate documented | none |
| Git branch / SHA | `vcs.ref.head.name|revision|type` only on `tool_result` after a successful `git commit`, with `OTEL_LOG_TOOL_DETAILS=1` | none | none | `github.copilot.git.branch`, `.commit_sha` on `invoke_agent` | none |

**How Claude Code derives repo identity** (docs, [Repository attributes](https://code.claude.com/docs/en/monitoring-usage#repository-attributes)): from the `origin` remote, once per session; values lowercased; "credentials, query strings, and fragments from the remote URL never appear"; omitted when there is no `origin`, the remote is not URL-shaped, or "the only enclosing repository is your home directory"; HTTPS and SSH remotes give identical values on GitHub, GitLab, Bitbucket Cloud; "Anthropic's telemetry drops every `vcs.*` key". The page does not say how the remote is read.

**Semconv status** ([vcs registry](https://opentelemetry.io/docs/specs/semconv/registry/attributes/vcs/)): `vcs.repository.url.full`, `vcs.repository.name`, `vcs.owner.name`, `vcs.provider.name` (`github|gitlab|gitea|bitbucket`), `vcs.ref.head.*` are all Release Candidate; the registry page lists no sensitivity note for them.

### 7. How operators add attributes, and why the signal mix

| Tool | Static attributes | Dynamic / headers | Admin pinning |
|---|---|---|---|
| Claude Code | `OTEL_RESOURCE_ATTRIBUTES` (US-ASCII, no spaces, percent-encode specials, no quoting; keys copied onto every metric datapoint and event unless `OTEL_METRICS_INCLUDE_RESOURCE_ATTRIBUTES=false`; built-in keys win except `vcs.*`) | `otelHeadersHelper` script emitting JSON headers, run at start and every 29 min (`CLAUDE_CODE_OTEL_HEADERS_HELPER_DEBOUNCE_MS`), http only | Managed settings `env` block; a managed `OTEL_EXPORTER_OTLP_ENDPOINT` removes developer-set per-signal endpoints, managed headers remove per-signal headers; exporter selectors are not locked |
| Gemini CLI | not documented | `GEMINI_TELEMETRY_OTLP_HEADERS` (JSON or `key=value`) | none documented |
| Codex | `otel.environment` string; `OtelSettings.span_attributes` and `tracestate` exist in source with no documented config key located | static `headers` map per exporter, with `${ENV}` interpolation shown | user-level config only; third-party guides cite `/etc/codex/managed_config.toml` (not verified) |
| Copilot | `OTEL_RESOURCE_ATTRIBUTES`; "Managed resource attributes override this variable" | `OTEL_EXPORTER_OTLP_HEADERS` static | managed settings override user settings, "though OTel environment variables can still override managed values in the Copilot Chat extension" |
| Docker CLI | `OTEL_RESOURCE_ATTRIBUTES` (minus `docker.cli.*`) | none | none |

**Rationale for the signal mix.** No tool states one. Implied only: Claude Code keeps `prompt.id`, `workflow.run_id` and `workspace.host_paths` off metrics "because of unbounded cardinality" (page text, read as a stated reason) and uses events to "tie all of those events back to the single prompt"; Codex splits log-only from trace-safe event targets without prose; Docker's page gives no reason for metrics-only. Searches for Docker, Gemini and Codex design rationale (PRs, issues, blogs) returned nothing (Negative results).

**Operator demand for more context** (all open, no maintainer reply at read time): [claude-code#102277](https://github.com/anthropics/claude-code/issues/102277) wants `vcs.ref.head.name` on every event so a ~100-developer fleet can attribute cost to tickets by branch, and rejects a launch-time `OTEL_RESOURCE_ATTRIBUTES` because it "goes stale after `git switch`"; it also asks for a repo-relative path "so home directories and usernames aren't exposed". [claude-code#97772](https://github.com/anthropics/claude-code/issues/97772) and [github/copilot-cli#4967](https://github.com/github/copilot-cli/issues/4967) ask for allow-listed dynamic attributes on spans with type, size and cardinality limits.

### 8. Evidence for and against, per open question

**Events beside the span (per-artifact records).**
- For: closest precedents are events (`plugin_installed`, `plugin_loaded`, `skill_activated`, `codex.skill_invocation`, `extension_install`); four of five tools run events as their primary signal; Claude Code keeps names redactable in events via placeholders and hashes.
- Against: every tool configures each signal's exporter separately (Claude `OTEL_{METRICS,LOGS,TRACES}_EXPORTER`; Codex `exporter` / `trace_exporter` / `metrics_exporter`; Copilot per-signal), so an armed event signal is a second pipeline and a second endpoint path (inference from those configs, not measured for grim); Docker, the one short-lived CLI, emits neither events nor spans; Codex also offers events as span-attached variants (`trace_event!`), so "event" does not force the logs signal.

**Metrics emitted by the process vs derived at the collector.**
- For emitting: Docker CLI emits one delta counter from a 50 ms bounded flush; Claude Code emits process-lifetime counters (`session.count`, `commit.count`, `pull_request.count`) with only standard attributes; Codex has `codex.process.start`.
- Against / limits: Docker's single metric is the only short-lived-CLI precedent found; the agents are long-lived with periodic readers; Codex/Gemini/Copilot catalogs are dominated by LLM and session metrics with no analogue in a package manager. Collector-side derivation precedent (spanmetrics) is in the prior-art file, finding 6.

**Identity attributes.**
- For including: Claude Code and Gemini CLI send an anonymous install id by default; Claude Code docs point to "unique value queries" backends for DAU/WAU/MAU; operators in the issues above ask for more attribution, not less.
- Against: email has no documented off switch in Claude Code and Gemini CLI; Copilot and Docker send no identity by default and Copilot puts it behind one explicit switch; Codex confines account and email to the log signal. Community-thread evidence of identifier pushback (unsalted hashes, device IDs) is in `research_otel_community_threads.md:77-78,117,162`.

**Git repo / branch / cwd auto-detection.**
- For: two of five tools do it; Claude Code makes it opt-in with credential stripping, an operator override and a home-directory exclusion; Copilot includes it by default on one span; the semconv `vcs.*` keys exist; fleets ask for it (#102277).
- Against: three of five do not; Claude Code defaults it off and strips it from Anthropic's own pipeline; Copilot's inclusion has no documented switch; the branch name is the same string operators want and the string a ticket id leaks through; no tool attaches cwd; all `vcs.*` keys are RC; Next.js and Astro, per the community-threads file (`research_otel_community_threads.md:80,122-124`), hash the git remote with a salt rather than send it.

## Recommendation

None. The requester asked for evidence only (findings, negatives, leads); decisions are for `/hex-architect` and the owner.

## Negative results

- Codex: no span names listed anywhere in docs or README; no documented `OTEL_RESOURCE_ATTRIBUTES` handling; no git/cwd field in `OtelSettings`; `span_attributes` / `tracestate` have no documented config key (searched config-reference fully, `otel.span_attributes` not found).
- Gemini CLI: no `OTEL_RESOURCE_ATTRIBUTES` mention in docs or `sdk.ts`; the `github_*` fields of `gemini_cli.config` could not be traced to source (`types.ts` fetch did not contain them); no span names listed; metric types/units mostly unstated; docs-vs-source mismatch on `active_approval_mode`.
- Copilot CLI: no official GitHub page lists CLI OTel attributes; no GitHub-staff comment in `copilot-cli#4967`; protocol and content-capture variable names conflict across third-party guides (`http/json` vs `http/protobuf`; `COPILOT_OTEL_CAPTURE_CONTENT` vs `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT`).
- Docker CLI: no design rationale, PR or issue located for the metrics-only choice; docs do not state metric type or protocol; page is 2024 [STALE] (source on `master` still matches, re-read 2026-10-22).
- Claude Code: docs do not say whether `DISABLE_TELEMETRY` / `DO_NOT_TRACK` suppress an operator-configured export (carried from the prior-art file); no off switch for `user.email` documented; how the `origin` remote is read is not stated; `claudeissues.com` mirror pages were unreachable (connection refused), issues read on github.com instead.
- No tool found that emits any signal for the act of installing or updating a config package other than Claude Code (`plugin_installed`), Gemini CLI (`extension_install`) and Copilot/Codex plugin-suggestion events; no package-manager CLI (cargo, uv, npm, pip) with OTel signals located (same absence as the prior-art file).
- No tool documents cwd as an attribute; no published cardinality numbers for any `vcs.*` use.

## Leads

- **git-context-gating:** decide which of Claude Code's three levers (default-off toggle, credential stripping plus lowercasing, operator override via resource attribute) apply to grim, against `src/oci/git_provenance.rs:199,358` and the release `Auto` git-mode precedent in the discussion (`otel-usage-telemetry.md:136-139`).
- **artifact-name-redaction:** read `plugin_id_hash` / `"third-party"` and Copilot `mcp_server_name_hash` as the model for `grim.artifact.changed` refs; needs the hash construction (Claude Code does not publish the algorithm or a salt statement) before it can be compared with the salted-hash precedent in the community-threads file.
- **identity-source-for-distinct-counts:** the discussion leaves "distinct user or machine count" open; Claude Code `user.id` and Gemini `installation.id` are random persisted files, so a lane on where grim could persist one (under `GRIM_HOME`) versus the .NET/Homebrew device-id pushback is the open piece.
- **signal-count-and-endpoints:** measure what arming a log-record event adds for a short-lived process (second endpoint path, second POST, shutdown cost) using the opentelemetry-rust probe from the prior-art file; every catalog here is long-lived except Docker.
- **verify-gemini-and-codex-sources:** the raw-file reads for `types.ts` (Gemini) and `shared.rs` (Codex) were summarized by the fetch model; re-read the two files line by line before any attribute is cited in an ADR.
- **mcp-and-hook-callers:** whether grim should attach the calling agent (`gen_ai.agent.name`-style, dotnet env-var detection precedent in the prior-art file) is a separate attribute lane; Copilot's `gen_ai.agent.name` values (`copilot`, `copilotcli`, `claude`) are the only catalog precedent found here.

## Method notes

- All pages read via WebFetch (a small model summarizes the page against the prompt), so quoted strings are model-extracted from the page; the Claude Code monitoring page (156,786 chars) was read in full over three fetches, with the last 56,786 chars read through a persisted tool result. Source files (Codex, Gemini CLI, Docker CLI) were fetched from `raw.githubusercontent.com` `main`/`master` on 2026-10-22 with no commit pinned; pin SHAs before citing in an ADR.
- Counts of metrics and events are my own tallies of the listed names (Codex 66 constants from `names.rs`; Claude Code 28 events incl. `system_prompt` and `managed_settings_resolved`, whose attribute lists were not read).
- Bash network access was denied in this session, so no repository was cloned and no code was grepped locally.

## Sources

| Source | Type | Date | Relevance |
|--------|------|------|-----------|
| https://code.claude.com/docs/en/monitoring-usage | Docs | undated (cites v2.1.287) | Claude Code metrics, events, spans, standard attributes, `vcs.*`, privacy |
| https://code.claude.com/docs/en/env-vars | Docs | undated | Flush/shutdown timeouts, project settings cannot enable telemetry |
| https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/telemetry.md | Docs | undated | Gemini CLI metrics, events, settings |
| https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/telemetry/telemetryAttributes.ts | Source | undated, `main` | Common attributes |
| https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/telemetry/sdk.ts | Source | undated, `main` | Resource, exporters, shutdown |
| https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/utils/installationManager.ts | Source | undated, `main` | Random `installation.id` |
| https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/telemetry/types.ts | Source | undated, `main` | Event names, `worktree_active` (excerpt only) |
| https://learn.chatgpt.com/docs/config-file/config-advanced | Docs | undated | Codex events, metrics table, `[otel]` |
| https://learn.chatgpt.com/docs/config-file/config-reference | Docs | undated | Codex `otel.*` keys, project-config restriction |
| https://github.com/openai/codex/blob/main/codex-rs/otel/README.md | Source | undated, `main` | Codex README events, temporality |
| https://raw.githubusercontent.com/openai/codex/main/codex-rs/otel/src/metrics/names.rs | Source | undated, `main` | 66 metric name constants |
| https://raw.githubusercontent.com/openai/codex/main/codex-rs/otel/src/events/shared.rs | Source | undated, `main` | Log vs trace-safe common fields |
| https://raw.githubusercontent.com/openai/codex/main/codex-rs/otel/src/events/session_telemetry.rs | Source | undated, `main` | Event names |
| https://raw.githubusercontent.com/openai/codex/main/codex-rs/otel/src/provider.rs | Source | undated, `main` | Resource, shutdown thread |
| https://raw.githubusercontent.com/openai/codex/main/codex-rs/otel/src/config.rs | Source | undated, `main` | `OtelSettings` fields |
| https://code.visualstudio.com/docs/copilot/guides/monitoring-agents | Docs | 2026-10-21 | Copilot metrics, events, spans, identity/git, env vars |
| https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/configure-copilot-cli | Docs | undated | Copilot CLI OTel scope sentence |
| https://langfuse.com/integrations/developer-tools/github-copilot | Docs (third party) | undated | Copilot CLI span names, protocol claim |
| https://github.com/github/copilot-cli/issues/4967 | Issue | 2026-09-25 | Dynamic enrichment request |
| https://docs.docker.com/engine/cli/otel/ | Docs | Engine 26.1.0 (2024) [STALE] | Docker `command.time` |
| https://raw.githubusercontent.com/docker/cli/master/cli/command/telemetry.go | Source | undated, `master` | Resource, 50 ms timeout, delta |
| https://raw.githubusercontent.com/docker/cli/master/cli/command/telemetry_utils.go | Source | undated, `master` | Counter type, attributes |
| https://raw.githubusercontent.com/docker/cli/master/cli/command/telemetry_docker.go | Source | undated, `master` | gRPC exporters, endpoint env |
| https://raw.githubusercontent.com/docker/cli/master/cli/command/telemetry_options.go | Source | undated, `master` | No resource options |
| https://github.com/anthropics/claude-code/issues/102277 | Issue | 2026-10-15 | Branch-on-every-event request |
| https://github.com/anthropics/claude-code/issues/97772 | Issue | 2026-09-28 | Dynamic span enrichment request |
| https://opentelemetry.io/docs/specs/semconv/registry/attributes/vcs/ | Spec | RC, undated | `vcs.*` stability and notes |
| `.agents/research/research_otel_cli_prior_art.md` | Local research | 2026-10-22 | Gate/latency/receiver evidence this file extends; Docker "histogram-style" line corrected here |
| `.agents/research/research_otel_community_threads.md` | Local research | 2026-10-22 | Git-remote hashing, identifier pushback |
| `.agents/discussions/otel-usage-telemetry.md` | Local discussion | 2026-10-22 | Open questions this lane informs (lines 118-139) |
