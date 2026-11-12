# Research: Scoping grim's OTLP destination and credentials to grim alone

<!--
Filename: .agents/research/research_otel_scoped_destination.md
Handoff to: /hex-architect or /hex-plan for grimoire-rs/grimoire#334
-->

## Metadata

**Date:** 2026-11-09
**Domain:** observability
**Triggered by:** [grimoire-rs/grimoire#334](https://github.com/grimoire-rs/grimoire/issues/334) — configure the collector without process-wide `OTEL_*` variables
**Expires:** 2027-05-09
**Builds on:** `research_otel_cli_prior_art.md` (activation switches, timeouts, SDK behaviour — not repeated here)

## Direct Answer

No comparable tool solves the #334 collision by reading only the standard `OTEL_*` family. The field splits three ways:

1. **Tool-owned destination keys or variables**: Gemini CLI (`GEMINI_TELEMETRY_OTLP_*`), Docker CLI (`DOCKER_CLI_OTEL_EXPORTER_OTLP_ENDPOINT`), Codex (`[otel]` in `config.toml`), Copilot (managed `telemetry` block).
2. **An admin-owned managed layer**: a fixed system path, MDM or server settings that pin destination and auth above user and env. Used by Claude Code, Codex and Copilot. Claude Code is the one tool that reads the standard variables. It compensates by stripping `OTEL_*` from every subprocess, ignoring exporter variables in repo settings, and offering a headers helper command.
3. **Deployer-side per-binary env**: wrappers that set variables for one executable. Used by Nix `makeWrapper`, proto `[tools.<id>.env]` and mise `[wrappers]`. ocx does the same with `private` patch variables on entrypoint launchers.

No tool lets an env var name a config file *below* the user layer and also guards against repo-injected env. Tools that care about repo injection gate by **location** (managed path, protected scope, trust prompt), never by pointer.

For grim, the answer is one change: `GRIM_TELEMETRY_OTLP_ENDPOINT` / `GRIM_TELEMETRY_OTLP_HEADERS`, in a **sealed** form. When the grim endpoint is set, grim reads no `OTEL_*` variable for the export. The ocx entrypoint (option A) is not worth its cost, and the org config-file layer is a separate ADR. See Recommendation.

## Technology Landscape

### Tool comparison

| Tool | Destination and auth source | Tool-prefixed? | Admin layer | `OTEL_*` to subprocesses | Repo-config guard |
|---|---|---|---|---|---|
| Claude Code | `OTEL_EXPORTER_OTLP_*` from the shell or a settings `env` block. `otelHeadersHelper` command (http only, refreshed every 29 min) | Gate only (`CLAUDE_CODE_ENABLE_TELEMETRY`) | `managed-settings.json` at a fixed OS path, plus `managed-settings.d/`, MDM and server-managed. Highest precedence. A managed OTLP value strips developer-set per-signal endpoint, protocol and header vars | Not passed to the Bash tool, hooks, MCP servers or LSPs. Only `TRACEPARENT` is passed | Exporter vars in `.claude/settings*.json` ignored, except off values |
| Codex CLI | `[otel]` in `config.toml`: `exporter`, `endpoint`, `protocol`, `headers` (`${VAR}` references) | Config keys. `OTEL_*` not documented | `/etc/codex/managed_config.toml`, `requirements.toml` (pins exporters, CLI ≥ 0.166.0), macOS MDM | `shell_environment_policy`. Whether it covers OTLP headers is undocumented | Project `.codex/config.toml` ignores `otel` |
| Gemini CLI | `telemetry.otlpEndpoint` / `otlpProtocol` / `otlpHeaders` in settings. `GEMINI_TELEMETRY_OTLP_ENDPOINT` / `_PROTOCOL` / `_HEADERS` | Yes | System settings file at a fixed path. `GEMINI_CLI_SYSTEM_SETTINGS_PATH` relocates it. Env and CLI still beat it | Not documented | Not documented |
| Copilot CLI | Enterprise-managed `telemetry` block (endpoint, protocol, headers). Env vars only from third-party pages | `COPILOT_OTEL_*` (unverified) | MDM, server-managed, `managed-settings.json`. Beats env and user | Managed headers are "never passed through environment variables" | Repo settings share a key subset that excludes telemetry |
| Cursor | Server-side Team Settings, headers stored encrypted | n/a | Enterprise admin console | n/a (no local export config) | n/a |
| Docker CLI | `DOCKER_CLI_OTEL_EXPORTER_OTLP_ENDPOINT`, else context metadata `otel.OTEL_EXPORTER_OTLP_ENDPOINT`. No headers | Yes. Bare `OTEL_*` not read | Docker context | — | — |
| OpenTofu | Standard `OTEL_*` only | No: exposed to the #334 collision | — | — | — |
| Java agent | `otel.*` system properties, then `OTEL_*` env, then config file | Per-JVM properties scope without a prefix | `OTEL_JAVAAGENT_CONFIGURATION_FILE` | — | — |
| **grim today** | Standard `OTEL_*` only. `GRIM_TELEMETRY` arms | Gate only | None | git env partly scrubbed. `OTEL_*` inherited by `git` and `gh`/`glab` | n/a (env only) |

### OpenTelemetry standard

- **Declarative config.** The variable is `OTEL_CONFIG_FILE`; the deprecated `OTEL_EXPERIMENTAL_CONFIG_FILE` is replaced. The data model is Stable and supports `${VAR}` / `${VAR:-default}` substitution. When the file is set, other `OTEL_*` vars are ignored unless the file references them. Implemented for cpp, go and python (1.0.0) and for java, js and php (rc). No Rust evidence found. It is itself one process-wide variable, so it reproduces the #334 collision one level up.
- **Prefixed variables.** The spec gives no guidance on app-prefixed OTel variables.
- **File-based secrets.** There is no `*_FILE` secret convention; only the TLS certificate and key variables take paths. `OTEL_EXPORTER_OTLP_HEADERS` is inline only.
- **Platform contribution to config.** Spec PR #3948 (merged 2024-04-05) discussed the shared host and worker environment. Follow-ups [#3966](https://github.com/open-telemetry/opentelemetry-specification/issues/3966) and [#3967](https://github.com/open-telemetry/opentelemetry-specification/issues/3967) were filed; their current state is unverified.

### Env-pointed config layers (issue direction 1)

| Tool | Variable | Effect | Trust handling |
|---|---|---|---|
| git | `GIT_CONFIG_SYSTEM` / `GIT_CONFIG_GLOBAL`; `GIT_CONFIG_COUNT/KEY_n/VALUE_n` | Replaces a layer / adds a top layer | Env is treated as the user's. Some keys (`safe.directory`, `safe.bareRepository`) are honoured only from protected scopes |
| pip | `PIP_CONFIG_FILE` | Loaded last, wins | None |
| uv | `--config-file` (`UV_CONFIG_FILE` unverified) | Replaces all discovery | None |
| cargo | None (`CARGO_HOME` relocates; `--config` adds) | — | None; repo config is normal |
| mise | `MISE_GLOBAL_CONFIG_FILE` / `MISE_SYSTEM_CONFIG_FILE` | Replaces the global or system layer | The pointer gets global trust. Project config needs `mise trust`, which gates `[env]`, `[hooks]` and `[settings]`. Advisory GHSA-436V-8FW5-4MJ8 (April 2026) |
| Gemini CLI | `GEMINI_CLI_SYSTEM_SETTINGS_PATH` / `_DEFAULTS_PATH` | Relocates the top or bottom layer | None. Its enterprise guide concedes users can repoint it and recommends a wrapper |
| Claude Code | None for the managed path | — | The managed layer is location-gated by design |
| kubectl | `KUBECONFIG` | Merged list | Docs warn a crafted kubeconfig can run code |

Pattern: a tool either trusts env completely or gates security-relevant keys by location. None trusts a file because an env var pointed at it while also defending against repo-injected env.

### Deployer-side per-binary scoping

- **Other managers.** Nix `makeWrapper` / `wrapProgram --set` (well known, page not read). proto `[tools.<id>.env]` (verified; requires a trusted config). mise `[wrappers.<name>]` (verified). asdf per-plugin `exec-env` (unverified). Activation-style managers are shell-wide: `mise activate`, `devbox shell`, conda, Scoop `env_set`.
- **ocx** (read locally, ocx website `explanation/patches.mdx`, `reference/env-composition.md`, `reference/metadata/entry-points.md`):
  - Patch companion variables carry a visibility. `private`, the default, reaches only the target's own generated launchers, and only when the target is a composition root.
  - Launchers inherit the ambient env; only `--clean` drops it. Patches apply at stage 3, above package env.
  - grim's package (`packaging/grim/metadata.json`) declares `binaries: ["grim"]` and a public `PATH` entry but **no `entrypoints`**. There is no launcher for a private variable to reach, which is exactly why #334 says ocx can't scope it.
  - The reference allows the same name in `binaries` and `entrypoints`: `entrypoints/` precedes `bin/` on the composed `PATH`.
  - Caveat: a launcher run by absolute path, or from a `PATH` no ocx command composed, matches only `*` patch rules.

## Key Findings

1. **The collision is real and acknowledged by the field.** Every AI-agent CLI except Claude Code moved the destination to tool-owned keys or variables. Claude Code reads the standard variables but compensates with a subprocess strip, a repo-settings guard, a managed pin and a headers helper.
2. **Managed layers don't fit #334's constraints.** In Claude Code, Codex and Copilot they are location-gated, sit above the user, and cannot be relocated by env. #334 needs the opposite on two counts: the deployer can't write system paths, and the user must stay in control.
3. **An env-named layer is a layering decision, not a security one.** Gemini, pip, git and ocx (`OCX_CONFIG`) have one. The security cost for grim is small: a project `grimoire.toml` can already set the experimental flags, consent never lives in a config file, and telemetry and registry defaults are already reachable from env. The real cost is that grim has no config layering today (`global_config.rs`: the two scopes are "independent and never merged"). ocx has a full chain (system, user, home, managed, `OCX_CONFIG`, `--config`) with env above every file. Bringing that to grim is its own ADR.
6. **The collision runs both ways, and the reverse is live today.** An armed grim reads ambient `OTEL_*` meant for other programs. `OTEL_SERVICE_NAME` and `OTEL_RESOURCE_ATTRIBUTES` relabel grim's telemetry (`src/telemetry/payload.rs::resource`). `OTEL_EXPORTER_OTLP_PROTOCOL=grpc` silently disables it, and an ambient timeout or header set rides along (`src/telemetry/config.rs::from_env`). A per-variable precedence rule leaves these holes; ignoring the whole family closes them.
4. **`OTEL_CONFIG_FILE` is unusable here.** It is the standard's answer, but it is process-wide and has no Rust implementation.
5. **Per-binary env is the deployer's job, done with wrappers.** ocx already has the mechanism; grim's package does not opt in.

## Options for grim

| Option | Reach | Stops redirection | Hides token from other tools | grim code | Contract risk |
|---|---|---|---|---|---|
| A. `"entrypoints": {"grim": {}}` in grim's ocx metadata, plus an org patch with `private` `OTEL_*` vars | ocx only | Yes | Yes, except grim's own children | None (packaging) | Package-shape change; launcher re-entry on every call |
| B. `GRIM_TELEMETRY_OTLP_ENDPOINT` / `_HEADERS`, winning over `OTEL_*`. A grim endpoint never receives `OTEL_*` headers | Any env deployer | Yes | No: visible in the env, but only grim sends it | Small (`src/telemetry/config.rs::from_env`) | None; telemetry is Unstable until 1.0 |
| C. Headers helper command (Claude Code style) | Any | — | Yes | Medium; new exec surface reachable from env | New trust question |
| D. Env-named org config layer (#334 direction 1) | Any | Yes | Yes, with correct file permissions | Large: a third config scope, `grim config` reporting | Frozen config contract; the CWE-426 decision unless key-gated |
| E. Fixed-path managed layer (Claude Code / Codex / Copilot style) | Admins with root or MDM | Yes | Yes | Large | Breaks "user stays in control"; deployer can't write `/etc` |
| F. `OTEL_CONFIG_FILE` | — | No (process-wide) | No | n/a | No Rust support |

## Recommendation

Do **B alone, sealed**. (Revised the same day; the first draft recommended A and B.)

- **Rule.** `GRIM_TELEMETRY_OTLP_ENDPOINT` set means grim reads no `OTEL_*` variable for the export: not headers, protocol, timeout, service name or resource attributes. Headers come only from `GRIM_TELEMETRY_OTLP_HEADERS`. Unset, today's `OTEL_*` behaviour is unchanged.
- **Opt-outs.** `armed()` does not change, so `GRIM_TELEMETRY` is still required and `DO_NOT_TRACK`, `OTEL_SDK_DISABLED` and offline still win.
- **Endpoint value.** A base URL with `/v1/logs` appended, the same value an operator would put in `OTEL_EXPORTER_OTLP_ENDPOINT`. That leaves room for more signals later.
- **Precedent.** Docker CLI reads no bare `OTEL_*`. `OTEL_CONFIG_FILE` makes an SDK ignore the other `OTEL_*` variables. Gemini CLI uses the same variable shape.
- **No regret.** In any later layered config, env stays the top rung (ocx row 8, grim's own `GRIM_DEFAULT_REGISTRY`). A `[telemetry]` table can join underneath without changing these variables.

**Not A.** Its only gain over B is hiding the token from other processes of the same user. That is not a security boundary, and under ocx the token sits in the patch package either way. Its cost is a launcher re-entry on every grim call for every ocx user, and absolute-path invocations skip name-anchored patches. Launcher latency was not measured.

**D and E are a separate ADR.** Write it when a non-telemetry org default needs delivery. Start from ocx's chain and decide which keys an env-named file may carry; the declared artifact set and consent must stay out.

Not included: a grim-prefixed resource-attributes variable for fleet labels. Add it when someone asks.

## Sources

| Source | Type | Date | Relevance |
|---|---|---|---|
| https://code.claude.com/docs/en/monitoring-usage | Docs | read 2026-11-09 | Claude Code OTel vars, headers helper, subprocess strip, repo guard |
| https://code.claude.com/docs/en/managed-settings | Docs | read 2026-11-09 | Managed layer paths, precedence, OTLP pinning |
| https://learn.chatgpt.com/docs/config-file/config-advanced | Docs | read 2026-11-09 | Codex `[otel]` keys |
| https://learn.chatgpt.com/docs/enterprise/managed-configuration | Docs | read 2026-11-09 | Codex managed config, `requirements.toml` |
| https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/telemetry.md | Docs | main, 2026-11-09 | Gemini telemetry keys and `GEMINI_TELEMETRY_OTLP_*` |
| https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md | Docs | main, 2026-11-09 | Gemini system settings layer and path override |
| https://github.blog/changelog/2026-07-08-enterprise-managed-opentelemetry-export-for-vs-code-and-cli/ | Changelog | 2026-07-08 | Copilot managed telemetry, headers kept out of env |
| https://cursor.com/docs/enterprise/opentelemetry-export | Docs | read 2026-11-09 | Cursor server-side export |
| https://docs.docker.com/engine/cli/otel/ | Docs | read 2026-11-09 | `DOCKER_CLI_OTEL_EXPORTER_OTLP_ENDPOINT` |
| https://raw.githubusercontent.com/docker/cli/master/cli/command/telemetry_docker.go | Source | master, 2026-11-09 | Docker precedence: env var, then context metadata; bare `OTEL_*` unread |
| https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/ | Spec | read 2026-11-09 | `OTEL_CONFIG_FILE`, precedence over other `OTEL_*` |
| https://opentelemetry.io/docs/specs/otel/configuration/data-model/ | Spec | read 2026-11-09 | Declarative config Stable, substitution syntax |
| https://github.com/open-telemetry/opentelemetry-configuration/blob/main/language-support-status.md | Repo | read 2026-11-09 | Language support (no Rust found) |
| https://opentelemetry.io/docs/specs/otel/protocol/exporter/ | Spec | read 2026-11-09 | No `*_FILE` secrets; TLS path vars |
| https://opentelemetry.io/docs/zero-code/java/agent/configuration/ | Docs | read 2026-11-09 | Per-JVM `otel.*` properties |
| https://opentofu.org/docs/internals/tracing/ | Docs | read 2026-11-09 | Standard-only OTel tool |
| https://git-scm.com/docs/git-config | Docs | read 2026-11-09 | `GIT_CONFIG_*`, protected scopes |
| https://pip.pypa.io/en/stable/topics/configuration/ | Docs | read 2026-11-09 | `PIP_CONFIG_FILE` |
| https://mise.jdx.dev/configuration.html | Docs | read 2026-11-09 | Global/system pointers, trust gating, wrappers |
| https://kubernetes.io/docs/concepts/configuration/organize-cluster-access-kubeconfig/ | Docs | read 2026-11-09 | `KUBECONFIG` merge and trust warning |
| ocx `website/src/content/docs/explanation/patches.mdx`, `reference/env-composition.md`, `reference/metadata/entry-points.md` | Local docs | ocx-sh/ocx checkout, 2026-11-09 | Private patch vars, launcher env stages, `binaries` + `entrypoints` |
| `packaging/grim/metadata.json` | Repo | 2026-11-09 | grim's ocx package declares no entrypoints |
