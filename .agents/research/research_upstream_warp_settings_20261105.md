# Research: upstream refresh — warp settings (deep, #270)

## Metadata
**Date:** 2026-11-05 · **Domain:** vendor AI-config layouts · **Depth:** deep (settings row)
**Triggered by:** /hex-execute .agents/plans/plan_settings_follow_ups.md (R-A)
**Expires:** 2027-05-05

Pages fetched raw with `curl` as markdown (`<page>.md`), not through WebFetch. Page
date for W2 as recorded by the scoping artifact (2026-11-03); the others show none.
Vendor version: Warp stable `v0.2026.11.03.17.08.stable_00` (from
`releases.warp.dev/channel_versions.json`).

Sources: [settings file](https://docs.warp.dev/terminal/settings) (W1),
[all settings](https://docs.warp.dev/terminal/settings/all-settings) (W2, 2026-11-03),
[file locations](https://docs.warp.dev/terminal/settings/file-locations) (W3),
[agent profiles and permissions](https://docs.warp.dev/agents/capabilities/agent-profiles-permissions) (W4),
and the vendor's own JSON schema `resources/settings_schema.json` (W5, title "Warp
Settings", draft 2020-12), which W1 names: "The JSON schema for `settings.toml` is
bundled with the app (read-only)". W5 was extracted from the stable Linux package
`releases.warp.dev/stable/v0.2026.11.03.17.08.stable_00/warp-terminal-…-x86_64.pkg.tar.zst`
(sha256 `267c19d3…6a6561`; no checksum is published — `.sha256` and `SHA256SUMS`
return 404). Only that one JSON file was extracted; nothing in the package ran.

### PD-21 (Warp): is `agents.execution_profiles` hand-editable?

- W2: "`execution_profiles` — The collection of agent profiles and their permissions, shared by Agent Mode and the terminal agent. This is a structured object that Warp manages when you create and edit profiles in **Settings** > **Agents** > **Profiles**; edit it through the UI rather than by hand."
- W1, "Configure agent permissions", hand-edits the flat `[agents.profiles]` keys (`agent_mode_coding_permissions`, `agent_mode_command_execution_allowlist`, …).
- W4: permissions are per profile, and the command allowlist is "empty by default". W5's
  `execution_profiles` default holds a `default` profile with its own
  `command_allowlist: []` and `command_denylist`; W2 and W5 give `[agents.profiles]`
  a non-empty default allowlist (`cat`, `echo`, `find`, `grep`, `ls`, `which`).

**Decision: unresolved, so every key that depends on it stays off.** The pages do not
say whether the flat `[agents.profiles]` keys still govern the agent once
`execution_profiles` exists, or only seed it. No credential-free check is possible:
Warp is a GUI app with no settings or validation verb. Off: all of
`/agents/profiles/*` and `/agents/execution_profiles`. Global rows that do not touch
permissions are unaffected.

A second, independent reason for the profile lists: W2 says "Only include the
settings you want to change — Warp uses built-in defaults for everything else", and
the lists have non-empty built-in defaults. A grim set piece on
`agent_mode_command_execution_denylist` would create the key holding only grim's
entries and drop the built-in `rm`, `curl`, `ssh` … entries, weakening the policy.

## Claims

| # | Key (JSON pointer) | Value type and values | Merge (scalar / set / map; across which files) | Scope (global / project) | Category | Source URL | Verbatim quote | Page date / vendor version |
|---|---|---|---|---|---|---|---|---|
| 1 | (file) `settings.toml` | TOML v1.1 | one user file; no project file | global | — | W1, W3 | "Warp stores your preferences in a plain-text file called `settings.toml`." · "The file uses [TOML v1.1](https://toml.io/en/v1.1.0) syntax." · Linux: "`${XDG_CONFIG_HOME:-$HOME/.config}/warp-terminal/settings.toml`" | none / stable 2026.11.03 |
| 2 | `/agents/warp_agent/other/thinking_display_mode` | enum `show_and_collapse`, `always_show`, `never_show` | scalar | global | behaviour default | W2, W5 | "Controls how agent thinking traces are displayed after streaming." | 2026-11-03 / W5 |
| 3 | `/agents/warp_agent/other/open_conversation_layout_preference` | enum `new_tab`, `split_pane` | scalar | global | behaviour default | W2, W5 | "Whether to open agent conversations in a new tab or a split pane." | 2026-11-03 / W5 |
| 4 | `/agents/warp_agent/other/default_prompt_submission_mode` | W2: `interrupt`, `queue`; W5 adds `steer` | scalar | global | behaviour default | W2, W5 | "Default behavior when submitting a new prompt while the agent is still responding." | 2026-11-03 / W5 |
| 5 | `/agents/warp_agent/other/long_running_command_submission_mode` | enum `send_immediately`, `queue_until_command_completes` | scalar | global | behaviour default | W2, W5 | "What happens when you submit a prompt while the agent is running an agent-requested long-running command." | 2026-11-03 / W5 |
| 6 | `/agents/warp_agent/other/orchestration_message_display_mode` | enum `show_and_collapse`, `always_show`, `always_collapse` | scalar | global | behaviour default | W2, W5 | "How child-agent message bodies are displayed during multi-agent runs." | 2026-11-03 / W5 |
| 7 | `/agents/warp_agent/other/auto_approve_bypasses_command_denylist` | bool | scalar | global | permission policy | W2, W5 | "Whether auto-approve and fast forward run commands that match your command denylist without asking for confirmation." | 2026-11-03 / W5 |
| 8 | `/agents/warp_agent/other/cloud_agent_computer_use_enabled` | bool | scalar | global | permission policy | W2, W5 | "Whether computer use is enabled for cloud agent conversations." | 2026-11-03 / W5 |
| 9 | `/agents/warp_agent/other/agent_attribution_enabled` | bool | scalar | global | behaviour default | W2, W5 | "Whether the Warp Agent adds an attribution co-author line to commit messages and pull requests it creates." | 2026-11-03 / W5 |
| 10 | `/agents/warp_agent/other/show_agent_notifications`, `…/show_conversation_history`, `…/should_show_oz_updates_in_zero_state`, `…/should_render_use_agent_toolbar_for_user_commands` | bool | scalar | global | behaviour default | W2, W5 | "Whether agent notifications are shown." (one line per key) | 2026-11-03 / W5 |
| 11 | `/agents/warp_agent/is_any_ai_enabled` | bool | scalar | global | behaviour default | W2, W5 | "Controls whether all AI features are enabled." | 2026-11-03 / W5 |
| 12 | `/agents/warp_agent/active_ai/enabled`, `…/code_suggestions_enabled`, `…/intelligent_autosuggestions_enabled`, `…/agent_mode_query_suggestions_enabled`, `…/git_operations_autogen_enabled`, `…/rule_suggestions_enabled`, `…/shared_block_title_generation_enabled` | bool | scalar | global | behaviour default | W2, W5 | "Controls whether proactive AI features like suggestions are enabled." (one line per key) | 2026-11-03 / W5 |
| 13 | `/agents/warp_agent/input/ai_auto_detection_enabled`, `…/nld_in_terminal_enabled`, `…/show_model_selectors_in_prompt`, `…/show_agent_tips`, `…/include_agent_commands_in_history` | bool | scalar | global | behaviour default | W2, W5 | "Controls whether AI automatically detects natural language input." (W2 default `true`, W5 default `false`) | 2026-11-03 / W5 |
| 14 | `/agents/knowledge/rules_enabled`, `/agents/knowledge/warp_drive_context_enabled` | bool | scalar | global | behaviour default | W2, W5 | "Whether the agent uses your saved rules during requests." | 2026-11-03 / W5 |
| 15 | `/agents/mcp_servers/file_based_mcp_enabled` | bool | scalar | global | permission policy | W2, W5 | "Whether global file-based MCP servers from third-party agents are automatically spawned." | 2026-11-03 / W5 |
| 16 | `/general/default_session_mode` | enum `terminal`, `agent`, `cloud_agent`, `tab_config`, `docker_sandbox` | scalar | global | behaviour default | W2, W5 | "The default mode for new terminal sessions. … Options: `\"terminal\"`, `\"agent\"`, `\"cloud_agent\"`, `\"tab_config\"`, `\"docker_sandbox\"`." | 2026-11-03 / W5 |
| 17 | `/agents/profiles/agent_mode_coding_permissions` | enum `always_ask_before_reading`, `always_allow_reading`, `allow_reading_specific_files` | scalar | global | permission policy | W1, W2, W5 | "The file read permission level for the agent." | 2026-11-03 / W5 |
| 18 | `/agents/profiles/agent_mode_execute_readonly_commands` | bool | scalar | global | permission policy | W2, W5 | "Whether the agent can auto-execute read-only commands without asking." | 2026-11-03 / W5 |
| 19 | `/agents/profiles/agent_mode_command_execution_allowlist`, `…/agent_mode_command_execution_denylist`, `…/agent_mode_coding_file_read_allowlist` | string list (regex patterns; paths) | one file; writing the key replaces a non-empty built-in default | global | permission policy | W2, W5 | "Commands the agent must always ask before executing (regex patterns). Type: array of strings." (W2's default list runs from `bash` to `rm`) | 2026-11-03 / W5 |
| 20 | `/agents/execution_profiles` | object of profiles | UI-managed | global | permission policy | W2, W5 | "edit it through the UI rather than by hand." | 2026-11-03 / W5 |
| 21 | `/agents/custom_endpoints` | object | UI-managed | global | — | W2 | "Warp manages the collection from **Settings**; edit it through the UI rather than by hand." | 2026-11-03 |
| 22 | `/agents/cloud_conversation_storage_enabled`, `…/other/auto_handoff_on_sleep_enabled`, `…/other/should_force_disable_cloud_handoff`, `…/other/should_force_disable_ampersand_handoff` | bool | scalar | global | — | W2 | "Whether conversations are stored in the cloud." | 2026-11-03 |
| 23 | `/agents/usage_display_mode` | enum `credits`, `cost` | scalar | global | — | W2 | "Applies only to the CLI's settings file." | 2026-11-03 |
| 24 | `/agents/warp_agent/input/ai_command_denylist`, `…/agent_toolbar_chip_selection_setting` | string; string or object | scalar | global | — | W2 | "Commands to exclude from AI natural language autodetection. Type: string." | 2026-11-03 |
| 25 | `/agents/voice/*`, `/agents/third_party/*`, `/agents/statusline/*`, and all non-`agents` sections (`appearance`, `terminal`, `text_editing`, …) | various | — | global | — | W2, W5 | "Settings are grouped into sections that correspond to TOML table headers" | 2026-11-03 / W5 |

## Verifier sign-off
none required

## Live CLI check

not run — Warp is a GUI desktop app (the package holds one `warp` binary and a
`/usr/bin/warp-terminal` launcher) with no settings listing or validation verb; the
Warp Agent CLI installs only through an install script, which the brief forbids. The
vendor's own schema (W5) was read instead, without executing anything.

## Routing

- Harness: rows · re-check trigger: a Warp release whose docs reconcile `[agents.profiles]` with `execution_profiles`, or a credential-free settings validation verb.
- Settings file: Linux `${XDG_CONFIG_HOME:-$HOME/.config}/warp-terminal/settings.toml`; macOS `~/.warp/settings.toml`; Windows `%LOCALAPPDATA%\warp\Warp\config\settings.toml` (Stable; Preview uses `warp-terminal-preview` / `~/.warp-preview`) · TOML v1.1 (grim's `toml_edit 0.25.15+spec-1.1.0` parses 1.1) · relocated by `XDG_CONFIG_HOME` on Linux only (W3).
- Root: macOS is covered by the existing `warp-root` (`~/.warp`). Linux needs one new append-only `VendorRoot` row, proposed `warp-config` (anchor `warp-config-root`) = `$XDG_CONFIG_HOME|~/.config` + `/warp-terminal`, on the `kilo-config` / `cursor-cli-xdg` precedent. Windows has no grim root for `%LOCALAPPDATA%`; the row set is OS-dependent, as Cursor's is.
- Limits for every row: the Settings panel and the bundled `modify-settings` skill also write the file, and Warp hot-reloads it (W1); an unrecognized value shows a banner and "falls back to defaults for the affected settings" (W1).
- Project scope: none documented ("user only").
- Per key:
  - `/agents/warp_agent/other/thinking_display_mode`, `…/open_conversation_layout_preference`, `…/long_running_command_submission_mode`, `…/orchestration_message_display_mode` → row (enum, scalar, global)
  - `/agents/warp_agent/other/default_prompt_submission_mode` → row (enum `interrupt`, `queue`, scalar, global); `steer` is in W5 only, so it stays out
  - `/general/default_session_mode` → row (enum `terminal`, `agent`, `cloud_agent`, `docker_sandbox`, scalar, global); leave out `tab_config`, which needs a companion path key
  - `/agents/warp_agent/other/auto_approve_bypasses_command_denylist`, `…/cloud_agent_computer_use_enabled`, `/agents/mcp_servers/file_based_mcp_enabled` → row (bool, scalar, global; permission policy)
  - Bools in rows 9-14 → row (bool, scalar, global)
  - `/agents/profiles/*`, `/agents/execution_profiles` → off: PD-21 unresolved; the lists also replace non-empty built-in defaults
  - `/agents/custom_endpoints` → off: UI-managed, provider configuration
  - cloud storage and handoff keys → off: data-sharing controls, outside the three categories
  - `/agents/usage_display_mode` → off: CLI file only, billing display
  - `ai_command_denylist`, `agent_toolbar_chip_selection_setting` → off: string format unstated; union type
  - voice, third-party-CLI, status-line and terminal or appearance sections → off: input devices and terminal preferences, not AI-agent behaviour defaults

## Issue drafts
none (findings for the orchestrator go in your report)

## Friction

- `docs.warp.dev/getting-started/supported-platforms/linux.md` is a 404; the package URL was derived from `channel_versions.json` and the release host's naming.
- `app.warp.dev/download?package=…` returns an HTML page, not a redirect, so no direct link is documented.
- W2 and W5 disagree on two defaults (`ai_auto_detection_enabled`) and one value list (`default_prompt_submission_mode`).

## Skill changes
none
