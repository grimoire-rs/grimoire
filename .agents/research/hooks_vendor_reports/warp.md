# Warp — native hook / lifecycle-event mechanism

Client: **Warp** (Warp Terminal, "Agent Mode" / the built-in agent; also the standalone
**Warp Agent CLI**, the `warp` command). Vendor: Warp (warpdotdev). Docs root:
https://docs.warp.dev/. Research date: **2026-08-14**. All sources fetched same day unless
otherwise noted.

## VERDICT: none

**Confidence: high.**

Warp's own agent has **no mechanism for executing user-supplied code at lifecycle/tool
events** — no hook config file, no hook schema, no event-triggered subprocess invocation
under Warp's control. What exists instead, and is easily confused with hooks:

1. An **agent notification *consumption* protocol** (OSC 777 terminal escape sequences) by
   which *other* CLI agents (Claude Code, Codex, Gemini CLI, OpenCode) — using **their own**
   hook/plugin systems — push structured events *into* Warp so Warp's UI can show toasts /
   tab badges / desktop alerts. Warp is the passive listener here, not the hook executor.
   Direction of control is the opposite of what grim's `hook` artifact needs (grim needs
   Warp to *run* something; here, some other program runs something and *tells* Warp).
2. **Three open, unimplemented GitHub feature requests** asking Warp's maintainers to build
   exactly the thing the brief is scoping (a hooks file, JSON-on-stdin, an event catalogue
   modeled on Claude Code) — none closed, none linked to a merged PR, none carrying a
   maintainer commitment, oldest dated 2025-10-20.

What would raise confidence further: a statement from a Warp team member on one of the
open issues, or access to the JSON schema Warp "bundles with the app" for `settings.toml`
autocomplete (referenced in docs but not published standalone) to positively confirm no
undocumented/experimental `[hooks]` table exists there.

---

## 1. Existence & name

Does not exist as a shipped feature under any name. Searched the docs site, the changelog,
and settings reference for: `hooks`, `agent hooks`, `lifecycle events`, `triggers`,
`automations`, `notify` (as a configurable command), `plugins` (event-handler sense),
`guardrails`/`policy hooks`. None of these surface a mechanism matching the brief's
definition ("client-invoked, deterministic execution of user-supplied code at
lifecycle/tool events").

Adjacent, official, real, but explicitly **not hooks** (each disambiguated below):

- **Agent Notifications** — a fixed set of three built-in notification categories
  (Complete / Request / Error) that Warp raises automatically for its own agent, plus a
  *consumption* path for external agents' own notification signals. No user-configurable
  command execution. Source: https://docs.warp.dev/agent-platform/capabilities/agent-notifications/
  (fetched 2026-08-14).
- **Agent Profiles & Permissions** — autonomy levels and command allow/denylists that gate
  *whether the agent's own built-in tools run*, not a mechanism to run *external*
  user-supplied code at an event. Source:
  https://docs.warp.dev/agent-platform/capabilities/agent-profiles-permissions/ (fetched
  2026-08-14).
- **Rules** (`AGENTS.md` / legacy `WARP.md`, Global Rules in Warp Drive) — static
  instructions injected into the agent's context, not executable. Source:
  https://docs.warp.dev/agent-platform/capabilities/rules/ (fetched 2026-08-14).
- **Workflows** (Warp Drive) — saved/parameterized shell commands a *human* runs from a
  picker; never triggered by an agent lifecycle event. Mentioned in nav only; out of scope
  per the brief.
- **MCP servers** — tool-calling extension point invoked *by the model's own tool-use
  loop*, not by lifecycle events, and not "user code at an event" in the hooks sense.
  Source: https://docs.warp.dev/agent-platform/capabilities/mcp/ (fetched 2026-08-14).
- **Shell integration (`precmd`/`preexec`)** — Warp injects shell hooks (zsh `precmd`,
  `preexec`; fish equivalents; `bash-preexec` shim for bash) that emit a DCS
  (Device Control String) escape sequence carrying JSON session metadata (cwd, command,
  exit code, duration) back to the Warp UI, purely to power terminal features: Blocks,
  per-command duration/notifications, working-directory tracking. **Confirmed unrelated to
  the agent's lifecycle** — it fires for every shell prompt/command regardless of whether
  an agent is involved, and there is no documented path from this DCS stream into agent
  hook-style code execution. (Search-derived summary of Warp's own shell-integration
  behavior; no single doc page fetched verbatim for this paragraph — treat the mechanism's
  *existence* as confirmed by multiple independent search hits including a Warp GitHub
  issue thread, `#2429`, showing the `warp_precmd` function name directly, but treat the
  DCS field list as approximate.)

### The three open feature requests (primary evidence of absence)

| # | Title | State | Opened | Labels | Maintainer response |
|---|---|---|---|---|---|
| [#7834](https://github.com/warpdotdev/warp/issues/7834) | "Feature Request: Agent Lifecycle Hooks for Observability" | **Open** | 2025-10-20 | `enhancement`, `ready-to-spec`, `triaged` | None visible; no linked PR |
| [#6857](https://github.com/warpdotdev/Warp/issues/6857) | "Features: custom slash commands and agent hooks" | **Open** | 2025-07-17 | `area:agent`, `area:editor-notebooks`, `enhancement`, `os:linux`, `triaged` | None visible; no linked PR |
| [#12868](https://github.com/warpdotdev/warp/issues/12868) | "Plugin/marketplace packaging: bundle skills + commands + hooks + MCP into installable units (Claude-Code/Codex-compatible)" | **Open** | 2026-06-20 | `area:agent`, `area:mcp`, `area:skills`, `enhancement`, `ready-to-spec`, `repro:high`, `triaged`; assigned `@peicodes` | None visible; no linked PR |

Issue #7834's body is the most detailed and is worth quoting for what it proposes (this is
a **community member's proposal, NOT an implemented or vendor-endorsed schema** — labelled
`[unofficial/proposed]`):

> A configuration file (`~/.warp/hooks.yaml`) that triggers external scripts on agent
> lifecycle events: agent prompt submission and response lifecycle; tool usage (pre/post
> execution); code changes and command execution; error conditions and session boundaries.
> Each hook receives JSON-formatted event data via stdin containing timestamps,
> session/agent IDs, model information, and contextual details.

The requester explicitly cites Claude Code's hooks and the community
`disler/claude-code-hooks` repo as the model to copy, and argues hooks are needed because
MCP-based logging costs tokens and requires explicit agent invocation. **None of this is
shipped.** Issue #12868 (opened ten months later, 2026-06-20) explicitly lists hooks as
still-missing and cross-references #7834 and #6857 as the unresolved prior art, plus a
third, narrower one:

| # | Title | State | Opened | Note |
|---|---|---|---|---|
| [#8741](https://github.com/warpdotdev/warp/issues/8741) | "CLI hooks for Warp's built-in tools (markdown viewer, editor, split panes)" | Open | 2026-02-22 | **Different scope** — asks for CLI commands (`warp open --viewer`, `warp split right`) to script Warp's own GUI features from a shell. Not an event/lifecycle mechanism at all; flagged here only because the title contains "hooks" and it could otherwise be miscounted as evidence of an event system. |

Also relevant to the direction-of-control question:

- [#9094](https://github.com/warpdotdev/Warp/issues/9094) "Interaction with Warp" — requests
  exposed notification **APIs** so that Claude Code/Codex/other agents' own hooks can signal
  Warp — i.e., asks for more of the *consumption* direction, not a Warp-side execution hook.
- [#12329](https://github.com/warpdotdev/warp/issues/12329) "Support reliable CLI agent
  notifications over SSH and tmux" — Open, opened 2026-06-08. Entirely about making the
  *existing* OSC 777 notify-Warp path more reliable over SSH/tmux (a bundled
  `warp-agent-notify` helper binary, socket delivery). Confirms (again) that the shipped
  mechanism is agents-notify-Warp, not Warp-executes-hook.

## 2. Config location(s)

No hook config file exists, so there is nothing to give a path for. For completeness (and
because the brief's own known-paths note references `~/.warp/` at global scope), the
**real, adjacent** config surfaces and their actual paths:

| Surface | Global/user scope | Project scope | Format | Notes |
|---|---|---|---|---|
| Warp settings | macOS: `~/.warp/settings.toml`; Linux: `~/.config/warp-terminal/settings.toml`; Windows: `%LOCALAPPDATA%\warp\Warp\config\settings.toml` | — (no project-level settings.toml documented) | TOML v1.1 | Watched and hot-reloaded — "Warp watches `settings.toml` for changes and applies them instantly when you save the file," no restart. Bidirectional sync with the GUI Settings panel. Bad/unknown keys → dismissible warning banner, falls back to defaults for the affected keys. A JSON Schema is "bundled with the app" for editor autocomplete (not found published standalone). Source: https://docs.warp.dev/terminal/settings (fetched 2026-08-14). |
| Rules | Global Rules live in **Warp Drive** (cloud-side "Personal > Rules" UI) — no plain dotfile path documented | `AGENTS.md` (preferred) or legacy `WARP.md`, at repo root and applied best-effort per subdirectory | Markdown | Source: https://docs.warp.dev/agent-platform/capabilities/rules/ (fetched 2026-08-14). |
| MCP servers | `~/.warp/.mcp.json` (Warp also reads other providers' native global files: `~/.claude.json`, `~/.codex/config.toml`, `~/.agents/.mcp.json`) | `.warp/.mcp.json` at project root (also reads `.mcp.json`, `.codex/config.toml`, `.agents/.mcp.json`) | JSON, `{"mcpServers": {"<name>": {...}}}` named map | Project-scoped servers "from any provider require explicit approval"; global servers from Warp auto-spawn by default, global servers from third-party providers need "Auto-spawn servers from third-party agents" opt-in. Source: https://docs.warp.dev/agent-platform/capabilities/mcp/ (fetched 2026-08-14). |
| Skills | `~/.agents/skills/` (marked "recommended" by Warp's own docs) **and** `~/.warp/skills/` (vendor-specific; also scans `~/.claude/skills/`, `~/.codex/skills/`, etc.) | `.agents/skills/` (recommended) and `.warp/skills/` at project root | Directory of `SKILL.md` (YAML frontmatter + markdown) | Priority: home-directory (global) skills first, then skills closer to repo root. **Note for the team**: Warp's docs do not describe an opt-in gate for the shared `~/.agents/skills/` pool — it appears to be scanned by default, alongside `~/.warp/skills/`, not behind a flag. Worth reconciling against grim's existing `vendor_warp.rs` assumption if that assumption is "shared pool only via opt-in." Source: https://docs.warp.dev/agent-platform/capabilities/skills/ (fetched 2026-08-14). |
| Settings Sync (cloud) | N/A — a sync layer, not a config location | N/A | — | "Settings Sync works by syncing the state of most of your Warp settings to our cloud servers." Synced: themes, most features, privacy settings, AI settings (cross-device, same-platform only). Explicitly **not** synced: custom keybindings, custom themes, device-specific settings (editor, startup shell). The page makes **no mention** of rules/MCP/skills/permissions being sync-covered. Practical reading: local file writes to the paths above are not blocked or overwritten by Settings Sync as documented — sync and file-based config appear to be independent layers, not competing ones. Source: https://docs.warp.dev/terminal/more-features/settings-sync/ (fetched 2026-08-14). |

No hooks directory-convention (e.g. `hooks/*.json`) exists anywhere in the above — there is
nothing to merge or have "one win," because there is no hook config at all.

## 3. Config schema — verbatim

**NOT APPLICABLE — no shipped schema.** The only schema in circulation is the *proposed,
unimplemented* one from issue #7834 (`[unofficial/proposed]`, a GitHub issue author's
design sketch, not vendor documentation):

- Proposed file: `~/.warp/hooks.yaml`
- Proposed shape: unspecified in the issue text beyond "a configuration file... that
  triggers external scripts on agent lifecycle events" — no verbatim key/value example is
  given in the issue body itself (per the fetched extract). Do **not** treat this as a real
  schema; it is a wishlist, not a contract.

Because nothing is shipped, the brief's specific question — named map keyed by event vs.
array of entries, matcher/filter syntax, stable id/name field for idempotent third-party
ownership — has **no real answer**. For comparison, the shipped MCP schema (`3.` above) is
a named map (`mcpServers` keyed by server name), which is the closest hint at how Warp
tends to shape its own JSON configs, but extrapolating a hooks shape from that would be
speculation.

## 4. Event catalogue

**NOT APPLICABLE for Warp's own agent** — no event fires a user-supplied command.

The closest real catalogue is the **inbound** notification protocol other agents use to
signal Warp (i.e., events *those* tools' own hook systems recognize, not Warp's). This
section is included only to prevent confusion with a real Warp event catalogue, and its
detailed field-level claims are **[unofficial]** (sourced from a third-party reverse-
engineering write-up, not vendor docs), with the parts that are officially corroborated
called out explicitly.

**Officially corroborated** (via the official `warpdotdev/claude-code-warp` reference repo,
fetched 2026-08-14 — this counts as the client's own public source per the brief's evidence
rules, since `warpdotdev` is Warp's own GitHub org): the Claude Code integration plugin
registers against **six Claude Code hooks** — `SessionStart`, `Stop`, `Notification`
(mapped to an idle-prompt signal), `PermissionRequest`, `UserPromptSubmit`, `PostToolUse` —
and forwards each as an OSC 777 escape sequence to Warp. **Claude Code owns and executes
the hook mechanism; the plugin code that runs inside it is what talks to Warp**, not the
reverse.

**[unofficial]** (yigitkonur.com, "reverse-engineering Warp's cli-agent notification
protocol," fetched 2026-08-14 — third-party blog, treat every specific below as a lead, not
a fact) claims a documented-looking wire protocol with:

- Transport: `\x1b]777;notify;warp://cli-agent;<JSON>\x07` written to `/dev/tty` (bypasses
  stdout/pipes, said to work over SSH).
- A required envelope: `{"v": 1, "agent": "<name>", "event": "<event>", "session_id": "...",
  "cwd": "...", "project": "..."}`.
- Feature-gate env vars: `WARP_CLI_AGENT_PROTOCOL_VERSION` (currently `1`),
  `WARP_CLIENT_VERSION`.
- Seven/eight named events: `session_start`, `prompt_submit`, `tool_complete`,
  `permission_request`, `idle_prompt`, `stop`, plus two OpenCode-specific extensions
  `question_asked` and `permission_replied`.
- Truncation rules (query/response to 200 chars, permission summaries to 120 chars) and a
  "protocol negotiates downward" rule.

None of this appears on docs.warp.dev. The one *officially* documented escape-sequence
format is generic and simpler — plain OSC 9 (`ESC ] 9 ; <body> BEL`) and OSC 777
(`ESC ] 777 ; notify ; <title> ; <body> BEL`) for arbitrary terminal notifications, with no
mention of the structured `warp://cli-agent` JSON-in-title convention. Source:
https://docs.warp.dev/terminal/more-features/notifications/ (fetched 2026-08-14). The
structured convention is real (corroborated by the three official reference-implementation
repos existing at all — `warpdotdev/claude-code-warp`, `warpdotdev/gemini-cli-warp`,
`warpdotdev/opencode-warp`) but its exact wire-format field list should be treated as
unofficial until Warp publishes it as a spec.

Even taking the fullest (unofficial) version at face value, this catalogue describes what
Warp is willing to **receive** from someone else's hook system — it is not an event
catalogue for hooking Warp's own agent.

## 5. Invocation

Not applicable — nothing to invoke. (For the inbound protocol: the *other* agent's own
hook runtime invokes its own scripts per its own contract — e.g. Claude Code hooks run as
shell commands with Claude Code's own working-directory/timeout/shell rules — and it is
those scripts, running under Claude Code, that shell out to write to `/dev/tty`. Warp
itself never spawns a process here; it only reads terminal output.)

## 6. Input payload — verbatim

Not applicable for a Warp-side hook. See §4 for the inbound envelope (labelled
`[unofficial]` in its field-level detail).

## 7. Output / response contract — verbatim

Not applicable. There is no exit-code contract, no stdout-as-JSON contract, no
deny/allow/ask response object, because Warp never executes a user hook to receive a
response from. The closest real "response contract" in the product is the **agent
permission model** (§ below), which governs whether Warp's own built-in tools are allowed
to run — a policy gate, not a hook response.

## 8. Reliability & limits

Not applicable (no hooks to time out, parallelize, or order). Tangential fact that could
matter to trampoline design: MCP servers are documented as persistent across restarts —
"[a stopped server] will remain so on next launch" — and community reports (e.g. Warp issue
#8400, "Built in MCP keeps returning transport closed - needs constant restart") suggest at
least some MCP config changes have historically needed a restart in practice, in some
tension with the CLI-side claim (search-derived, not directly fetched) that the standalone
Warp Agent CLI "watches the configuration file while it's running and reloads most values
as you save." Treat the desktop app's exact reload behavior for MCP as **unconfirmed
in either direction** beyond "settings.toml itself is confirmed hot-reloaded."

## 9. Security posture

No hook-specific warning exists (nothing to warn about). The nearest analogues, both
officially documented:

- **Command allow/denylist**: "The Warp Agent lets you define an allowlist of commands
  that run automatically without confirmation... Commands in the allowlist will always
  auto-execute, even if they are not read-only operations." "Command denylist rules take
  precedence over allowlist rules and agent autonomy settings." Default denylist patterns
  include `wget(\s.*)?`, `curl(\s.*)?`, `rm(\s.*)?`, `eval(\s.*)?`. "YOLO mode" (all four
  autonomy permissions set to Always allow) is named explicitly as a state, with denylist
  rules still overriding it. Source:
  https://docs.warp.dev/agent-platform/capabilities/agent-profiles-permissions/ (fetched
  2026-08-14).
- **MCP trust boundary**: "Project-scoped servers from any provider require explicit
  approval," while global servers from Warp itself auto-spawn by default and global
  servers from third-party providers require an opt-in toggle. This is the closest existing
  precedent for "repo-supplied config should not silently execute" — it is scoped to MCP
  servers, not to a hook mechanism that doesn't exist. Source: same MCP page as § 2.
- No statement anywhere in the fetched docs says "hooks are arbitrary code execution" —
  because Warp has no hooks to make that disclosure about. Compare: the community feature
  request (#7834) itself does not raise a security angle at all, which is itself notable
  given how central "hooks are arbitrary code execution, trust the repo before enabling"
  language is in Claude Code's own docs.

## 10. Third-party installability

Practically **yes, for the surfaces that exist** (settings.toml, MCP JSON, `AGENTS.md`/
`WARP.md`, skills directories) — all are plain files under paths a script can write without
the GUI, and settings.toml is explicitly confirmed hot-reloaded with no restart needed.
Global Rules are the one surface that appears to be **cloud/UI-native rather than a local
dotfile** ("via Warp Drive interface… no explicit file path mentioned in documentation") —
if that holds up, a third-party installer cannot land a *global* Rule by writing a file the
way it can for a *project* Rule (`AGENTS.md`). Settings Sync (cloud) is a separate,
opt-in-by-login layer that, per its own docs, does not claim to cover rules/MCP/skills/
permissions, so it should not block local file installation of any of those.

None of this matters for **hooks specifically**, because there is no hook file format to
install into. A third-party installer cannot "install a Warp agent hook" today by any
means — GUI, cloud, or file — because the feature does not exist in the product.

## 11. Trampoline viability

**Not viable today — there is no target to trampoline into.** A generic
`grim hook run --client warp --event <E>` command would have nothing to be invoked by: no
config key tells Warp's agent to shell out to anything at any event. If/when Warp ships
the feature requested in #7834, the trampoline pattern looks favorable on paper (the
proposal explicitly describes "JSON-formatted event data via stdin," a shell-command-style
hook, and a YAML config file) — i.e. the community's own ask is shaped like a subprocess
hook (compatible with a trampoline), not a JS-module or in-process plugin hook. But nothing
about the actual implementation is committed, so any grim design keyed to `~/.warp/hooks.
yaml` or that stdin shape would be pure speculation about an unshipped, unassigned,
maintainer-silent feature request.

The one thing grim *could* build today that overlaps this space: since Warp already
consumes the OSC 777 "agent notify" convention from other tools' hook systems, grim could
in principle install a **Claude-Code-style hook script** (for clients that do have real
hooks) whose job is to emit OSC 777 sequences — but that would be installing a hook for
*Claude Code* (or whichever client has real hooks) that happens to talk to Warp's display
layer, not installing anything into Warp itself. Out of scope for "Warp's own hook
contract," flagged here only because it's the one concrete, shippable integration this
research surfaced.

---

## Sources

| URL | What it establishes | Fetched |
|---|---|---|
| https://docs.warp.dev/ | Site nav/structure; no dedicated "Hooks" section anywhere in the IA | 2026-08-14 |
| https://docs.warp.dev/agent-platform/capabilities/agent-notifications/ | Notification categories (Complete/Request/Error); built-in vs third-party agent setup; no `notify` command or hook | 2026-08-14 |
| https://docs.warp.dev/agent-platform/capabilities/agent-profiles-permissions/ | Profiles, autonomy levels, command allow/denylist syntax (regex), MCP permission gating, "YOLO mode" | 2026-08-14 |
| https://docs.warp.dev/agents/cli/ | Warp Agent CLI overview; no hooks/lifecycle mechanism; shares harness/rules/skills with desktop app | 2026-08-14 |
| https://docs.warp.dev/terminal/more-features/notifications/ | Official OSC 9 / OSC 777 escape-sequence format for generic terminal notifications (not agent-specific, no `warp://cli-agent` JSON convention documented here) | 2026-08-14 |
| https://docs.warp.dev/agent-platform/capabilities/rules/ | Rules file paths: `AGENTS.md`/`WARP.md` (project), Warp Drive (global); no hooks mentioned | 2026-08-14 |
| https://docs.warp.dev/agent-platform/capabilities/mcp/ | MCP config paths (global `~/.warp/.mcp.json`, project `.warp/.mcp.json`), `mcpServers` named-map schema, project-scope approval requirement | 2026-08-14 |
| https://docs.warp.dev/terminal/settings | `settings.toml` paths per OS, TOML format, confirmed hot-reload/no-restart, bidirectional GUI sync, bundled JSON schema for autocomplete | 2026-08-14 |
| https://docs.warp.dev/terminal/more-features/settings-sync/ | Settings Sync is cloud-based, opt-in-by-login; does not claim to cover rules/MCP/skills/permissions | 2026-08-14 |
| https://docs.warp.dev/agent-platform/capabilities/skills/ | Skills directory paths (`~/.agents/skills/` recommended, `~/.warp/skills/` and other vendor dirs also scanned), priority order, `SKILL.md` format | 2026-08-14 |
| https://github.com/warpdotdev/claude-code-warp | **Official** reference plugin; confirms 6 Claude-Code-owned hook events forwarded to Warp via OSC 777; confirms direction of control (Claude Code executes, Warp receives) | 2026-08-14 |
| https://github.com/warpdotdev/warp/issues/7834 | Canonical open feature request for Warp-native agent lifecycle hooks; proposed (unshipped) `~/.warp/hooks.yaml`; no maintainer response; Open since 2025-10-20 | 2026-08-14 |
| https://github.com/warpdotdev/Warp/issues/6857 | Second open request bundling "agent hooks" with custom slash commands; Open since 2025-07-17; no maintainer response | 2026-08-14 |
| https://github.com/warpdotdev/warp/issues/12868 | Third open request (plugin/marketplace packaging incl. a "hooks engine"); explicitly cross-references #7834/#6857 as still unresolved; Open since 2026-06-20 | 2026-08-14 |
| https://github.com/warpdotdev/warp/issues/8741 | Different-scope "hooks" request (CLI access to Warp's own GUI tools, not an event system) — disambiguation only; Open since 2026-02-22 | 2026-08-14 |
| https://github.com/warpdotdev/Warp/issues/9094 | Requests exposed notification APIs for other agents' hooks to signal Warp — confirms consumption direction | 2026-08-14 |
| https://github.com/warpdotdev/warp/issues/12329 | SSH/tmux reliability work on the *existing* agents-notify-Warp path (OSC 777, planned `warp-agent-notify` helper); Open since 2026-06-08 | 2026-08-14 |
| https://yigitkonur.com/reverse-engineering-warp-cli-agent-protocol | **[unofficial]** third-party reverse-engineering of the OSC 777 envelope, event list, env-var gates, truncation rules — leads only, not vendor fact | 2026-08-14 |
| (search-derived) Warp shell integration (`precmd`/`preexec`/DCS) powering Blocks/session metadata | Confirms shell hooks exist but serve terminal UI, not agent lifecycle; corroborated by GitHub issue `warpdotdev/Warp#2429` showing the `warp_precmd` function name | 2026-08-14 |
