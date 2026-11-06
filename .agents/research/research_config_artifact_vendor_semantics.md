# Research: Harness settings semantics and third-party installability

## Metadata

**Date:** 2026-11-03
**Domain:** packaging
**Triggered by:** `.agents/discussions/config-artifact-type.md`
**Expires:** 2027-05-03

Method: vendor docs fetched 2026-11-03 through a summarising fetcher, so quoted phrases are the fetcher's extraction, not verified byte-for-byte; a row marked "not found" means the fetched page text did not state it, not that the vendor lacks it. No live harness was run. Vendor pages are living documents and carry no page date; none of the cited pages was dated, so none could be shown older than 18 months by date. Items that predate the 18-month window by version number are flagged inline. Nothing here recommends a course of action.

## Direct Answer

No single answer holds across the seven harnesses. What the evidence supports per settings category:

| Category | Evidence for being distributable | Evidence against |
|---|---|---|
| Behaviour defaults (model, UI, output) | Every harness has a user-level file where scalars are plain last-writer-wins; Claude and Copilot document that unknown or malformed entries are skipped or reported, not fatal ([Claude settings](https://code.claude.com/docs/en/settings)). | A project-scope write of a scalar is overridden by any higher layer, and managed `model` is a starting default only on Claude. OpenCode rejects the whole config on one bad value (watchlist row, 1.18.32). Renames recur (below). |
| Permissions and approvals | Claude, Codex, Kiro, Cursor all state a machine-readable direction: deny beats allow. A tool can classify an `allow` rule as loosening and a `deny` as tightening on those four. | OpenCode is last-match-wins, so order is load-bearing and appending can loosen or tighten. Gemini resolves by numeric priority with no stated tie-break. Claude refuses `auto`/`bypassPermissions` defaultMode from project scope and applies project `allow` only after a trust dialog. Copilot CLI: allow/deny grammar not found in fetched docs. |
| Env and endpoints | Claude has an `env` key and Gemini substitutes `${VAR}`; both are plain data. | Claude ignores telemetry export variables in project/local `env`; Codex ignores `openai_base_url`, `chatgpt_base_url` and provider keys in project config. Endpoint keys are the ones vendors refuse from repo scope. |
| Hooks | Claude, Cursor, Copilot and Codex all have file-based hook config with documented locations. | Hooks execute code. Every harness that documents trust gates them behind it (Claude `allowManagedHooksOnly`, Cursor "Require the workspace to be trusted", Codex untrusted projects skip hooks). Codex hooks sit behind `features.hooks`. |

A drop-in or include mechanism that lets a tool avoid editing the user's main file exists on Claude (managed tier only), Codex (profile files, `rules/` dirs), OpenCode (`OPENCODE_CONFIG`, `.opencode` dirs) and Copilot (`.github/copilot/settings.local.json`, hooks dir). It does not exist for the user tier on Claude.

## Per-harness findings

### 1. Permission model

| Harness | Grammar | Evaluation | Direction | Ordered tighten/loosen? |
|---|---|---|---|---|
| Claude Code | `Tool(specifier)`, e.g. `Bash(npm run test *)`, `Read(./.env)`; gitignore-style path anchors `//`, `~/`, `/`, `./`; `!` negation within one source | "Rules are evaluated in order: deny, then ask, then allow. The first match in that order determines the outcome" | Deny always wins; "An allow rule can't carve an exception out of a deny rule" ([permissions](https://code.claude.com/docs/en/permissions)) | Rule-level yes (deny > ask > allow). Mode-level not stated as a ladder, except `crossSessionInbound` `accept < hold < refuse` ([settings](https://code.claude.com/docs/en/settings)) |
| Codex CLI | `approval_policy` (`on-request`, `never`, ...), `sandbox_mode` (`read-only`, `workspace-write`, `danger-full-access`), named `[permissions.<name>]` profiles with filesystem `read`/`write`/`deny` per path or glob, and Starlark `prefix_rule()` | Multiple rule matches: "the most restrictive decision wins", `forbidden` > `prompt` > `allow` ([rules](https://learn.chatgpt.com/docs/agent-configuration/rules)) | Restrictive wins for rules; network `domains`: "`deny` wins on conflicts" ([config reference](https://learn.chatgpt.com/docs/config-file/config-reference)) | Rule decisions yes. Sandbox modes are naturally ordered by capability, but the fetched docs do not state an order; that ordering is inference. `on-failure` is deprecated and `untrusted` is no longer supported ([config reference](https://learn.chatgpt.com/docs/config-file/config-reference)) |
| Gemini CLI | `tools.allowed` (bypass confirmation), `tools.exclude`, `tools.confirmationRequired`; policy TOML `[[rule]]` with `toolName`, `argsPattern`, `commandPrefix`, `decision` (`allow`/`deny`/`ask_user`), `priority` 0-999 ([policy engine](https://geminicli.com/docs/reference/policy-engine)) | Highest final priority wins; tier bases Default 1, Extension 2, Workspace 3, User 4, Admin 5 | Not deny-first. "Deny only wins when its final priority is higher." No tie-break stated at equal priority | No. Order depends on integer priority, so tightening is not mechanically derivable from a rule alone. The page itself is inconsistent (table lists Admin as 5, prose says "Tier 4") |
| Copilot CLI | `--allow-tool`, `--deny-tool`, `--available-tools`, `--excluded-tools`, `--allow-url`/`--deny-url`; `deniedUrls`: "Denial rules take precedence over allow rules" ([config dir reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference)) | Tool rule grammar and order not found in fetched pages | URL deny wins; tool rules unknown | `permissions-config.json` "doesn't support deny rules, 'ask' rules, default modes, URL rules" (saved approvals only). `permissions.disableBypassPermissionsMode` accepts `"disable"`; other values are treated as `"disable"` with a warning |
| Cursor | CLI: `Shell(cmdBase)`, `Read(glob)`, `Write(glob)`, `Mcp(server:tool)`, `WebFetch(domain)` in `~/.cursor/cli-config.json` and `<project>/.cursor/cli.json` ([CLI permissions](https://cursor.com/docs/cli/reference/permissions)) | "Deny rules take precedence over allow rules" | Deny wins. For hooks: "any `deny` wins over `ask`, and `ask` wins over `allow`, regardless of source" ([hooks](https://cursor.com/docs/hooks)) | CLI yes. IDE auto-run: described as "best-effort guardrails rather than a hard security boundary" ([security](https://cursor.com/docs/agent/security)); no file grammar found |
| OpenCode | `permission` map: per tool (`read`, `edit`, `bash`, `webfetch`) a scalar or a pattern-to-action object; `*` and `?` wildcards ([permissions](https://opencode.ai/docs/permissions/)) | "the **last matching rule winning**" | Neither side wins by default; position decides. Defaults: most `allow`; `*.env` and `*.env.*` reads default to `ask` | No. Appending a rule after an existing one can flip it either way; a tool must understand ordering, and across merged configs the order is the merge order |
| Kiro | YAML rules with `capability`, `match`, `exclude`, `effect`; `*` and `**` ([permissions](https://kiro.dev/docs/permissions/)) | "deny-overrides algorithm: deny > ask > allow > evaluate" | Deny in any scope wins | Yes. Sub-agents get the stricter of parent and own. Headless: every ask becomes deny |

Contradiction to carry: Claude's "Yes, and don't ask again" writes an `allow` to the local file, yet "an `allow` rule there doesn't outrank an `ask` rule from a project or managed file" ([settings](https://code.claude.com/docs/en/settings)). Deny-first holds, but allow and ask interact across scopes too.

Kiro note: its docs describe a YAML permission system, while the watchlist (verified 2026-09-27) and Kiro CLI agent JSON use `allowedTools`/`toolsSettings`. The fetched page did not mention those keys; whether the two surfaces coexist is unresolved. The Kiro page may be newer than the watchlist row.

### 2. Layering and merge

| Harness | Layers (low to high) | Drop-in or include | List merge |
|---|---|---|---|
| Claude | user, shared project, project local, `--settings`, managed ([settings](https://code.claude.com/docs/en/settings)) | `managed-settings.d/*.json`, merged alphabetically after `managed-settings.json` ([managed settings](https://code.claude.com/docs/en/managed-settings)). No drop-in for user or project tier. `--settings` accepts inline JSON or a file path, per session only | "Claude Code combines the lists instead of picking one." Exceptions: `fallbackModel`, `modelPicker` (whole value, ignored at project/local), `availableModels` (managed replaces), `modelSettings`. Nested blocks merge key by key; `claudeMdExcludes` merges (watchlist) |
| Codex | `$CODEX_HOME/config.toml`, project `.codex/config.toml` (trusted only), profiles | `$CODEX_HOME/<name>.config.toml` profile files via `--profile`; `permissions.<name>.extends`; `rules/` folder under each active layer. "The page does not document an `include` directive." | Not stated for config lists. Rules from all layers are all evaluated, most restrictive wins |
| Gemini | defaults, system defaults, user, workspace, system overrides, env, CLI ([configuration](https://geminicli.com/docs/reference/configuration)) | Policy directories `~/.gemini/policies/*.toml`; env var paths for system files. No settings include | Arrays "are concatenated in the order System Defaults, User, Workspace, System Overrides"; objects such as `mcpServers` merge by name ([enterprise](https://geminicli.com/docs/cli/enterprise/)). Single-value settings: last wins. No `mergeStrategy` field was found in docs |
| Copilot | built-in, MDM, user `~/.copilot/settings.json`, repo `.github/copilot/settings.json`, local `settings.local.json`, env, flags | Repo and local files; hooks in `~/.copilot/hooks/` and `.github/hooks/*.json`, "loaded alongside" ([config dir reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference)). Also reads hooks from `.claude/settings.json` | List merge not stated. `--repo`/`--local` "accept only the settings a repository can override" |
| Cursor | CLI: global `cli-config.json`, project `cli.json`; hooks: enterprise, team, project, user | `hooks.json` per scope; merge of CLI project over global not stated | Hooks: all sources run; permission fields take the strictest; `followup_message` and similar fields let a lower-priority source override a higher one |
| OpenCode | remote `.well-known/opencode`, global, `OPENCODE_CONFIG`, project `opencode.json`, `.opencode` dirs, `OPENCODE_CONFIG_CONTENT`, managed files, macOS managed prefs ([config](https://opencode.ai/docs/config/)) | `OPENCODE_CONFIG` custom file; `.opencode` directories. "Configuration files are merged together, not replaced" | Arrays: not stated |
| Kiro | user `~/.kiro/settings/permissions.yaml`, workspace (stored per-user, outside the repo), agent profile, enterprise `managed-settings.json` | Agent-profile `permissions` field | Deny anywhere wins; no merge detail |

### 3. Managed or enterprise policy

| Harness | Can override user-level writes? | Blocks or filters third-party writes or sources? |
|---|---|---|
| Claude | Yes. "No user, project, local, or `--settings` value overrides them", except stricter lower-level values for listed keys (`permissions.blockReadsOutsideWorkingDirectories`, `maxEffortLevel`, others) ([settings](https://code.claude.com/docs/en/settings)) | `allowManagedPermissionRulesOnly` (managed becomes the only permission-rule source), `allowManagedHooksOnly`, `allowManagedMcpServersOnly`, `strictKnownMarketplaces` ([managed settings](https://code.claude.com/docs/en/managed-settings)). So a user-level install of `permissions.allow` or hooks can be silently dropped. `managedSourcesBehavior: "merge"` opt-in combines sources |
| Codex | `requirements.toml`: "Requirements set limits; configuration values provide defaults" ([config reference](https://learn.chatgpt.com/docs/config-file/config-reference)); `allowed_approval_policies`, `allowed_sandbox_modes`, managed hooks, MCP allowlist, restrictive `prefix_rule` ([managed config summary via search](https://developers.openai.com/codex/enterprise/managed-configuration)) | Constrains values; a user-written value outside the allowlist is out of policy. Direct fetch of the managed-configuration page returned 404; details rest on a search summary |
| Gemini | System settings "taking precedence over all other settings files"; admin keys `admin.mcp.*`, `admin.extensions.enabled`, `admin.skills.enabled`; admin policy dir must be root-owned | Admin policy dir ignores supplemental paths if the standard dir has any `.toml` |
| Copilot | MDM is a "policy baseline that user settings can usually override"; but `permissions.disableBypassPermissionsMode` MDM value always wins and sandbox managed values "set a floor" | Enterprise can disable features such as `shellShortcut` |
| Cursor | Hooks: enterprise > team > project > user. Workspace trust "can enforce through MDM" | Not documented for CLI permissions |
| OpenCode | Managed files and macOS managed prefs are the highest tier; "cannot be overridden by user or project configuration" | Source filtering not documented |
| Kiro | Enterprise permission policy supports `deny` and `ask` effects only (cannot grant) | Kiro hardcodes denies on `~/.kiro/settings/`, `.kiro/settings/` and `~/.kiro/workspace-roots/` so the agent cannot edit its own permission files ([permissions](https://kiro.dev/docs/permissions/)) |

Contradicting note: Copilot's MDM is described as overridable by users in general, unlike Claude's, so "managed wins" is not universal.

### 4. Stability

| Harness | Published schema | Churn evidence | Unknown keys |
|---|---|---|---|
| Claude | Yes, `https://json.schemastore.org/claude-code-settings.json`; docs warn it "can lag behind the newest CLI releases" ([settings](https://code.claude.com/docs/en/settings)) | Deprecated or removed keys in the settings reference: `disableArtifact`, `includeCoAuthoredBy` (renamed to `attribution`), `keybindingFlavor`, `permissionExplainerEnabled` (removed v2.1.257), `taskOutputMaxChars` (v2.1.277), `teammateDefaultModel` (v2.1.234) ([settings reference](https://code.claude.com/docs/en/settings-reference)); `bypassPermissions` project-scope behaviour changed at v2.1.257 | Bad JSON or schema-rejected value: whole file skipped with Settings Error. Individual bad entries (malformed permission rule, unknown hook event): "Settings Warning", entry skipped. Managed: invalid restrictive key reads as its strict value. Unparseable managed file: Claude "refuses to start" |
| Codex | Yes, `config-schema.json` linked from the reference ([config reference](https://learn.chatgpt.com/docs/config-file/config-reference)); docs also say the schema "includes keys that may not work" in some Work Cloud setups | `on-failure` deprecated, `untrusted` removed; docs moved domains (`developers.openai.com` to `learn.chatgpt.com`); MCP key rename pattern in watchlist (`startup_timeout_ms` / `_sec`) | Fetched pages do not say; watchlist: unknown MCP keys only warn on older Codex, and `cwd` on a `url` server "fails the whole `config.toml`" (watchlist row, Codex 0.157.1) |
| Gemini | Yes, `schemas/settings.schema.json` in repo and hosted copy | Deprecated: `ui.accessibility.enableLoadingPhrases`, `experimental.topicUpdateNarration`, `excludeTools`; "no automatic migration process" given. Product change: page states Gemini CLI "was replaced by Antigravity CLI on June 18, 2026" ([configuration](https://geminicli.com/docs/reference/configuration)). Unverified whether settings paths changed | Not stated |
| Copilot | None found in fetched pages (JSONC accepted) | `sandbox.gitAuth`/`sandbox.ghAuth` renamed, "ignored wherever they still appear, and there is no migration"; `--config-dir` deprecated for `COPILOT_HOME`; `infer` retired for `disable-model-invocation` (watchlist) | Listed in `/settings` Problems tab; `$schema` tolerated |
| Cursor | CLI config: no schema found. Hooks: required `"version": 1`; plugin schemas strict (`additionalProperties: false`, watchlist) | Not assessed | A strict plugin manifest rejects extras (watchlist, 2026-09-29) |
| OpenCode | Yes, `https://opencode.ai/config.json` and `tui.json` | `tools` deprecated for `permission`; `maxSteps` for `steps` (watchlist, agents.mdx 1.18.32) | Docs do not describe; watchlist: one invalid `color` or `steps` "makes OpenCode reject its whole config" (live-checked, 1.18.32). This is the clearest case where a third-party write can break the harness |
| Kiro | Not found | Agent JSON vs IDE markdown collision (watchlist, kiro #8040) | Not found |

### 5. Trust behaviour and refused keys

- **Claude.** Project `permissions.allow` and `additionalDirectories`, `extraKnownMarketplaces` and most `env` values apply only after the trust dialog; "`deny` and `ask` rules apply right away" ([permissions](https://code.claude.com/docs/en/permissions)). Hooks, `env`, and helper commands in settings files are gated the same way. `permissions.defaultMode` `auto` and `bypassPermissions` "don't take effect from project or local settings". Telemetry export variables in project `env` are ignored. Scope column in the settings reference marks keys as User, User or managed, Managed, or Global config; those never apply from the shared file. Why: they grant capability or move data off-machine. A tracked `settings.local.json` also needs trust.
- **Codex.** "Codex loads project-scoped config files only when you trust the project." Untrusted: no `.codex/` layers, config, hooks or rules. Ignored in project config: `openai_base_url`, `chatgpt_base_url`, provider, notification, profile, telemetry keys.
- **Gemini.** Folder trust is off by default (`security.folderTrust.enabled`). An untrusted folder does not load project `.gemini/settings.json`, project `.env`, MCP servers, custom commands, and disables auto-approval ([trusted folders](https://geminicli.com/docs/cli/trusted-folders/)). The Workspace policy tier is "currently non-functional".
- **Copilot.** Repo files accept only repo-overridable keys; sandbox settings and `shellShortcut` are "not repository-overridable". The full list was in a truncated page section and was not read.
- **Cursor.** Project hooks require a trusted workspace; IDE workspace trust is disabled by default ("Restricted mode breaks AI features").
- **OpenCode.** No trust model described for project config, which "is also safe to be checked into Git" per docs, yet it outranks global and remote config.
- **Kiro.** The structural answer: workspace permission rules are stored outside the repo, so "A cloned repo cannot inject permission rules".

### 6. Cross-harness mapping

| Intent | Claude | Codex | Gemini | Copilot CLI | Cursor | OpenCode | Kiro |
|---|---|---|---|---|---|---|---|
| Never read `.env` | `permissions.deny: ["Read(./.env)", "Read(./.env.*)"]` | `permissions.<n>.filesystem` `"**/*.env" = "deny"` (docs example; `**/.env` unconfirmed) | No equivalent found in fetched pages (policy rule on a read tool via `argsPattern` is possible but unshown; untrusted folders skip project `.env`) | Not found | CLI: `Read(.env)` deny; IDE: `.cursorignore` | `permission.read: {"*.env": "deny"}` (default is `ask`) | `fs_read` deny rule documented for write (`fs_write` deny `*.env`); read form unconfirmed |
| Allow the test command | `permissions.allow: ["Bash(npm run test *)"]` (project: applies after trust) | `prefix_rule(pattern=["npm","test"], decision="allow")` in `.rules`; runs outside sandbox | policy `commandPrefix = "npm test"`, `decision = "allow"`, or `tools.allowed` | `--allow-tool` flag; saved approvals only in `permissions-config.json` | `Shell(npm)` allow (first token only) | `bash: {"npm test*": "allow"}` placed after any `*` rule | `shell` allow rule in YAML |
| Read-only sandbox | `sandbox.*` keys and `defaultMode: plan`; not one switch (sandbox details not fetched) | `sandbox_mode = "read-only"` or `default_permissions = ":read-only"`. Do not combine with `sandbox_mode` | `tools.sandbox` (container sandbox, different meaning) | Sandbox keys are user or managed only | Not found | No sandbox concept in fetched pages | Not found |

Differences in meaning: Claude `Bash(npm run test *)` matches a full command with wildcard; Cursor `Shell` matches only the first token; Codex matches a command prefix as an argv list and also splits compound `bash -lc` commands; OpenCode globs the whole string. A rule that "allows tests" is therefore not byte-portable. Claude's Read deny does not by itself stop a Bash `cat`; sandbox or a second Bash rule is needed (the permissions page ties file-tool and Bash enforcement to different mechanisms; exact text not re-quoted here).

## Key Findings

1. Deny-first is documented for Claude, Cursor, Kiro, and Codex (most restrictive), so a tool can classify those rules mechanically. OpenCode is last-match-wins and Gemini is priority-numbered, so it cannot: [OpenCode](https://opencode.ai/docs/permissions/), [Gemini policy](https://geminicli.com/docs/reference/policy-engine).
2. Project-scope grants are gated on trust on Claude and Codex; project-scope restrictions (`deny`, `ask`) apply immediately on Claude: [Claude permissions](https://code.claude.com/docs/en/permissions), [Codex config](https://learn.chatgpt.com/docs/config-file/config-reference).
3. Managed policy can erase user-level permission and hook installs on Claude (`allowManagedPermissionRulesOnly`, `allowManagedHooksOnly`) and bound them on Codex (`requirements.toml`): [Claude managed](https://code.claude.com/docs/en/managed-settings).
4. Only Claude has a drop-in directory (`managed-settings.d`), and only for the managed tier; user-tier installs on every harness mean editing or owning the user's file, except Codex/Copilot/OpenCode which have extra-file mechanisms (profile files, `settings.local.json`, `OPENCODE_CONFIG`).
5. The worst-case failure of a bad third-party write is OpenCode rejecting the whole config; Claude skips the bad entry or file and surfaces it (`claude doctor`): [Claude settings](https://code.claude.com/docs/en/settings).
6. Endpoint and telemetry keys are refused from repo scope on Claude and Codex by design.
7. Churn: Claude documents at least six deprecated or removed keys; Copilot renamed sandbox keys without migration; Gemini CLI itself was superseded by Antigravity CLI (2026-06-18) per its docs.

## Sources

| Source | Type | Date | Relevance |
|--------|------|------|-----------|
| https://code.claude.com/docs/en/settings | Docs | live 2026-11-03 | Scopes, precedence, list merge, exceptions, trust, errors |
| https://code.claude.com/docs/en/permissions | Docs | live | Rule grammar, deny > ask > allow, workspace trust |
| https://code.claude.com/docs/en/managed-settings | Docs | live | `managed-settings.d`, `allowManaged*Only`, merge behaviour |
| https://code.claude.com/docs/en/settings-reference | Docs (first 100k of 448k chars read) | live | Deprecated and removed keys, scope column |
| https://learn.chatgpt.com/docs/config-file/config-reference | Docs (166k chars, 2 passes) | live | Codex keys, trust, profiles, filesystem deny |
| https://learn.chatgpt.com/docs/agent-configuration/rules | Docs | live | `prefix_rule`, most restrictive wins |
| https://developers.openai.com/codex/enterprise/managed-configuration | Docs via search summary only (direct fetch 404) | live | `requirements.toml` |
| https://geminicli.com/docs/reference/configuration | Docs | live | Layers, trust env, deprecations, Antigravity note |
| https://geminicli.com/docs/reference/policy-engine | Docs | live | Priority tiers, workspace tier disabled |
| https://geminicli.com/docs/cli/trusted-folders/ | Docs | live | What untrusted folders disable |
| https://geminicli.com/docs/cli/enterprise/ | Docs | live | Array concatenation, system overrides |
| https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference | Docs (first 100k of 470k) | live | User/repo/local files |
| https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference | Docs (partial, truncated sections) | live | Merge order, MDM, hooks, unknown keys |
| https://cursor.com/docs/cli/reference/permissions | Docs | live | Cursor CLI grammar, deny wins |
| https://cursor.com/docs/hooks | Docs | live | Hook scopes, merge, version, trust |
| https://cursor.com/docs/agent/security | Docs | live | Workspace trust, `.cursorignore` |
| https://opencode.ai/docs/permissions/ | Docs | live | Last-match-wins |
| https://opencode.ai/docs/config/ | Docs | live | Merge order, managed tiers |
| https://kiro.dev/docs/permissions/ | Docs | live | Deny-overrides, repo cannot inject rules |
| `.claude/rules/vendor-capability-watchlist.md` | Repo | 2026-09-27/29 | Reused for OpenCode/Codex/Copilot rejects-whole-config and rename evidence; read to line 150 of 345 only |

## Negative findings (dead ends and contradicting evidence)

- Copilot CLI tool allow/deny grammar, evaluation order and trust prompts for repo settings: not found; the relevant page section was truncated.
- Kiro: fetched page documents YAML permissions, while the watchlist and earlier CLI describe agent JSON `allowedTools`; unreconciled. `kiro.dev/docs/cli/chat/permissions/` redirects to the new page.
- Cursor: no schema, no CLI project-over-global merge rule, no IDE permission file grammar, no enterprise statement beyond hooks and workspace trust.
- Codex: managed-configuration page 404 on guessed path; direct page not read; `**/.env` deny unconfirmed; unknown-key behaviour unstated.
- Gemini: array `mergeStrategy`, unknown-key behaviour, and the Workspace tier are undocumented or disabled; page numbering inconsistent; product replaced by Antigravity CLI so settings semantics may be frozen or moving.
- Claude's "Variables Claude Code ignores in `env`" list was not read (beyond the 100k fetch window).
- Summarising fetcher may have paraphrased; verify quoted phrases before citing them in an ADR.

## Leads

- Antigravity CLI: whether it inherits Gemini's `settings.json` shape or replaces it.
- Sandbox semantics: Claude `sandbox.*` and Codex profiles decide whether "read-only sandbox" is expressible portably.
- Claude env ignore-list: read the rest of the settings reference for the exact refused variables.
- Copilot repo-overridable key list: read the truncated config-dir section.
- Kiro permission surfaces: reconcile YAML permissions with agent JSON.
