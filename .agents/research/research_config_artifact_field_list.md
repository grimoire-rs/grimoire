# Research: Verified first-slice settings field list for six harnesses

## Metadata

**Date:** 2026-11-03
**Domain:** packaging
**Triggered by:** `.agents/discussions/config-artifact-type.md` Open question "Which fields can each harness carry in the first slice?", and the marketplace, worktree and write-time questions
**Expires:** 2027-05-03

Method. Vendor docs were fetched on 2026-11-03 through a summarising fetcher, so quoted phrases are the fetcher's extraction. VERIFIED means a docs page stating the key was read; UNVERIFIED means the key stays off the list and goes to the follow-up issue. The Claude worktree and Codex availability items were run against real tools (section 8, 9). No vendor page carries a date, so none can be shown older than 18 months by date; the only dated third-party item is a Cursor forum thread of May 2026 (about 6 months). Out of scope and never listed: hooks, env, endpoints and base URLs, credentials, helper commands (`notify`, `statusLine`, `shell`), telemetry, Claude `enabledPlugins` (and the same-named keys on Copilot, Qoder and Codex `plugins.*`). Prior work not repeated: `.agents/research/research_config_artifact_vendor_semantics.md`.

Merge class here is grim's comparison unit (closed set from the discussion), not the harness's own merge rule; the harness rule is cited as soundness evidence where it matters.

## Direct Answer

1. Typed key list, VERIFIED fields, per harness, are in sections 1.1 to 1.6. Row counts are in the tables; rows marked UNVERIFIED or n/a are off the list.
2. Copilot has no tool allow or deny key in `~/.copilot/settings.json`. The user table has URL lists (`allowedUrls`, `deniedUrls`) and `permissions.disableBypassPermissionsMode`; tool rules come from `--allow-tool` / `--deny-tool` flags only. The Copilot tool-rule grammar for settings is UNVERIFIED and off the list.
3. `.github/copilot/settings.local.json` is documented as personal but is not auto-ignored ("add this to `.gitignore`"). It is not a guaranteed-untracked file, so Copilot stays global-only.
4. Cursor: the docs never state the merge. A staff-confirmed forum report says a project `.cursor/cli.json` that has any `permissions` section overrides the global one. A global-only permissions write is therefore not sound in repos whose `cli.json` has a `permissions` block. Non-permission Cursor keys are global-only by design ("Only permissions can be configured at the project level"), so those writes are sound.
5. Qoder settings docs: `https://docs.qoder.com/cli/settings-reference`, `https://docs.qoder.com/cli/settings`, `https://docs.qoder.com/cli/permissions`. Permission keys and grammar are VERIFIED. Two Qoder pages disagree on array merge (section 4).
6. Marketplace settings key: Claude `extraKnownMarketplaces` (keyed map), Copilot `extraKnownMarketplaces` (keyed map, same name, `source` of `directory`/`git`/`github`), Codex `[marketplaces.<name>]` (keyed table). Cursor, OpenCode and Qoder: no key found.
7. Claude in a linked worktree (real 2.1.317): both local files are read; the main checkout's value wins on a shared key; a file only in the worktree is still read. Confirmed by probe.
8. Rewrites during a session: Claude, Codex (rules file), Cursor, OpenCode and Copilot all write their own files; whole-file rejection on one bad value is documented for OpenCode (watchlist) and for Claude (invalid JSON or schema-rejected value skips the whole file).
9. A real Codex binary is obtainable: `npm view @openai/codex version` returned 0.166.0 with a `linux-x64` platform package. Nothing installed.

## 1. Field tables

Columns: key path (native; nested as JSON pointer or TOML path), type, merge class, evidence. "Loosens" marks a value that widens what the agent may do; the discussion's installer can use it for the consent prompt.

### 1.1 Claude Code

Files: global `~/.claude/settings.json` (`$CLAUDE_CONFIG_DIR` relocates); project scope per-developer untracked `.claude/settings.local.json` at the main checkout root. Format: strict JSON, no comments (comment or trailing comma is a Settings Error). Source for all rows: <https://code.claude.com/docs/en/settings> and <https://code.claude.com/docs/en/settings-reference>, read 2026-11-03 (settings-reference: first 200k of 448k characters).

| Field path | Type | Merge class | Status and evidence |
|---|---|---|---|
| `/model` | string | scalar | VERIFIED, scope "Any file" |
| `/effortLevel` | `low`/`medium`/`high`/`xhigh` | scalar | VERIFIED |
| `/outputStyle` | string | scalar | VERIFIED |
| `/language` | string, not validated | scalar | VERIFIED |
| `/theme` | string | scalar | VERIFIED (value format not shown) |
| `/editorMode` | string | scalar | VERIFIED (value format not shown; enumerate before use) |
| `/spinnerTipsEnabled` | boolean | scalar | VERIFIED |
| `/alwaysThinkingEnabled` | boolean | scalar | VERIFIED |
| `/includeGitInstructions` | boolean | scalar | VERIFIED |
| `/attribution` (`commit`, `pr`, `sessionUrl`) | object | object | VERIFIED, sub-key shapes not shown |
| `/permissions/allow` | string[] rules | unordered-list | VERIFIED; lists "combine instead of picking one" ([settings](https://code.claude.com/docs/en/settings), "Lists merge"). Loosens. Project copy waits for workspace trust; the untracked local file does not while untracked |
| `/permissions/deny` | string[] | unordered-list | VERIFIED; applies immediately |
| `/permissions/ask` | string[] | unordered-list | VERIFIED |
| `/permissions/defaultMode` | enum `default`, `acceptEdits`, `plan`, `auto`, `dontAsk`, `bypassPermissions`, `manual` | scalar | VERIFIED. `auto` and `bypassPermissions` "don't take effect from project or local settings"; list only the three safe values for project scope. Loosens |
| `/sandbox/enabled` | boolean | scalar | VERIFIED |
| `/sandbox/filesystem/denyRead`, `/allowWrite`, `/denyWrite`, `/allowRead` | path[] | unordered-list | VERIFIED (types read; merge across files not separately stated, assumed same list rule: UNVERIFIED for the merge, so keep off until read) |
| `/extraKnownMarketplaces` | object name to `{source:{source:"github",repo,ref?}}` | keyed-map | VERIFIED. Shape from the existing research `research_marketplace_native_consumption.md` line 62 plus the settings page ("Any file"); applies after workspace trust from a repo file |
| `/availableModels`, `/fallbackModel`, `/modelPicker` | arrays | ordered-list-as-scalar | `fallbackModel`, `modelPicker` take the whole value from the highest file ([settings](https://code.claude.com/docs/en/settings)); `availableModels` managed-replaces. VERIFIED as whole-value, but keep off the first slice: restricts model choice, not a user default |
| `/tui` | string | scalar | VERIFIED key; value names not shown: UNVERIFIED, off |

The `sandbox/filesystem/*` row is UNVERIFIED for merge and off the list; the unlisted count above therefore excludes it. Claude keys that would be listed are the first 16 rows through `extraKnownMarketplaces`, minus `tui`; `availableModels` family and `tui` are off.

### 1.2 Codex CLI

File: global `~/.codex/config.toml` (`$CODEX_HOME`). Project `.codex/config.toml` is a tracked file and loads only when trusted, so global-only. Format TOML. Sources: <https://learn.chatgpt.com/docs/config-file/config-reference> (first 100k of 169k read, plus offset 100k pass), read 2026-11-03.

| Field path | Type | Merge class | Status and evidence |
|---|---|---|---|
| `model` | string | scalar | VERIFIED |
| `model_reasoning_effort` | string (`low`..`ultra`, "levels depend on the model") | scalar | VERIFIED; do not enumerate |
| `approval_policy` | `on-request`, `never`, or inline table `{ granular = {...} }` | scalar (compare the whole value) | VERIFIED; `untrusted` unsupported, `on-failure` deprecated. `never` loosens |
| `sandbox_mode` | `read-only`, `workspace-write`, `danger-full-access` | scalar | VERIFIED. Conflicts with `default_permissions`: the docs say not to combine them. `danger-full-access` loosens |
| `sandbox_workspace_write` | table: `network_access` bool, `writable_roots` string[], `exclude_slash_tmp`, `exclude_tmpdir_env_var` | object | VERIFIED; `writable_roots` is an unordered-list inside it |
| `default_permissions` | string (`:read-only`, `:workspace`, `:danger-full-access`, or custom name) | scalar | VERIFIED; cross-key exclusion with `sandbox_mode` |
| `permissions.<name>` | table with `description`, `extends`, `filesystem`, `network`, `workspace_roots` | keyed-map (one piece per profile name) | VERIFIED; `filesystem` values `read`/`write`/`deny` or nested table |
| `tui.alternate_screen` | `auto`/`always`/`never` | scalar | VERIFIED |
| `tui.animations`, `tui.auto_recap`, `tui.raw_output_mode` | boolean | scalar | VERIFIED |
| `tui.notification_condition` / `notification_method` | enum | scalar | VERIFIED |
| `history.persistence` | `save-all`/`none` | scalar | VERIFIED |
| `file_opener` | enum | scalar | VERIFIED |
| `hide_agent_reasoning` | boolean | scalar | VERIFIED |
| `marketplaces.<name>` | `source` string, `source_type` `git`/`local`, `ref?`, `sparse_paths?` | keyed-map | VERIFIED; "Marketplaces can be defined in a trusted-project `config.toml`" |
| `web_search` (top-level) | unclear | n/a | UNVERIFIED: the page says "prefer the top-level `web_search`" but documents only `tools.web_search` |
| `personality` | enum | n/a | off: deprecated |
| `notify`, `openai_base_url`, `chatgpt_base_url`, `model_provider(s)`, `profile(s)`, `otel` | | n/a | out of scope (and ignored in project config per the page) |
| `projects.<path>.trust_level` | | n/a | out of scope: trust decisions |
| `plugins.<name@marketplace>.enabled` | | n/a | out of scope: enables publisher code |

Codex `rules/` (section 6): grim can own `~/.codex/rules/<name>.rules` outright; Codex itself appends to `default.rules`, so grim must not touch that name.

### 1.3 GitHub Copilot CLI

Files: global `~/.copilot/settings.json` (`$COPILOT_HOME`). Repo `.github/copilot/settings.json` is committed; `.github/copilot/settings.local.json` is "personal overrides (add this to `.gitignore`)": not auto-ignored, so global-only. Format JSONC. `config.json` in the same dir is Copilot-managed and separate. Sources: <https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference> (all but the last 26k characters read in three windows), read 2026-11-03.

| Field path | Type | Merge class | Status and evidence |
|---|---|---|---|
| `/model` | string (`"auto"` allowed) | scalar | VERIFIED; repo-overridable, applies only in a trusted directory |
| `/effortLevel` | `low`/`medium`/`high`/`xhigh` | scalar | VERIFIED; repo-overridable |
| `/theme` | `default`/`github`/`dim`/`high-contrast`/`colorblind` | scalar | VERIFIED (`colorMode` deprecated alias) |
| `/defaultMode` | `interactive`/`plan`/`autopilot` | scalar | VERIFIED |
| `/defaultPermissionMode` | `manual`/`assisted`/`allow-all` | scalar | VERIFIED; `allow-all` loosens; managed `permissions.defaultMode` outranks |
| `/permissions/disableBypassPermissionsMode` | string, `"disable"` | scalar | VERIFIED (tightens; other values treated as `"disable"` with a warning) |
| `/allowedUrls` | string[] | unordered-list | VERIFIED; loosens; not repo-overridable |
| `/deniedUrls` | string[] | unordered-list | VERIFIED; "Denial rules take precedence"; repo layer is a union ("can add entries, never remove") |
| `/disabledMcpServers`, `/disabledSkills` | string[] | unordered-list | VERIFIED; union across layers |
| `/askUser`, `/includeCoAuthoredBy`, `/respectGitignore`, `/renderMarkdown`, `/stream`, `/transcriptView` (`default`/`concise`) | boolean or enum | scalar | VERIFIED; `respectGitignore` is tighten-only in a repo layer |
| `/mergeStrategy` | `rebase`/`merge` | scalar | VERIFIED |
| `/sandbox/enabled` | boolean | scalar | VERIFIED; user or managed only; managed value sets a floor |
| `/extraKnownMarketplaces` | object name to `{source:{"directory"\|"git"\|"github", ...}, autoUpdate?: true}` | keyed-map | VERIFIED; "merge with the repository overriding the user for the same key" |
| Tool allow/deny rules | | n/a | UNVERIFIED, off: no user or repo key in the table; only `--allow-tool`/`--deny-tool` flags documented. The managed `permissions` arrays (`deny`, `ask`, `allow`, `limitTo`; families `Shell(...)`, `Read(...)`, `Edit(...)`, `Write(...)`, `Domain(...)`) appear in the page's managed section; whether a user file accepts them was not stated, and the last 26k characters were not read |
| `/sandbox/userPolicy/*`, `/sandbox/auth/*` | | n/a | VERIFIED keys, off: network hosts, credential injection |
| `/proxyUrl`, `/hooks`, `/statusLine`, `/enabledPlugins`, `/bashEnv` | | n/a | out of scope |

Grammar of tool rules that is documented (CLI-reference context): shell matching "a trailing `:*` matches the text before it. A plain `*` has no special meaning, so `git*` doesn't match `git status`." `permissions-config.json` holds saved approvals only and "doesn't support deny rules" ([same page](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference)). Behaviour on bad values: "ignores the invalid values and shows a startup warning"; unknown keys listed in the `/settings` Problems tab; `$schema` tolerated.

### 1.4 Cursor (CLI)

Files: global `~/.cursor/cli-config.json` (`$CURSOR_CONFIG_DIR`, or `$XDG_CONFIG_HOME/cursor/cli-config.json` on Linux). Project `<project>/.cursor/cli.json` is committed; "Only permissions can be configured at the project level. All other CLI settings must be set globally." Format JSON. The IDE does not read these files (no IDE permission file found). Sources: <https://cursor.com/docs/cli/reference/configuration>, <https://cursor.com/docs/cli/reference/permissions>, read 2026-11-03.

| Field path | Type | Merge class | Status and evidence |
|---|---|---|---|
| `/permissions/allow` | string[] `Type(pattern)` | unordered-list | VERIFIED. Grammar: `Shell(cmdBase)`, `Shell(curl:*)`, `Read(glob)`, `Write(glob)`, `WebFetch(domain)`, `Mcp(server:tool)`. Loosens |
| `/permissions/deny` | string[] | unordered-list | VERIFIED; "Deny rules take precedence over allow rules" |
| `/approvalMode` | `allowlist`/`auto-review`/`unrestricted` | scalar | VERIFIED; `unrestricted` loosens |
| `/sandbox/mode` | string | scalar | VERIFIED key; values not enumerated: UNVERIFIED values |
| `/sandbox/networkAccess` | string | scalar | VERIFIED key; values not enumerated: UNVERIFIED values |
| `/sandbox/readBoundary` | `system`/`workspace` | scalar | VERIFIED |
| `/editor/vimMode` | boolean | scalar | VERIFIED (required field) |
| `/notifications`, `/hints`, `/rewind`, `/suggestNextPrompt`, `/maxMode` | boolean | scalar | VERIFIED |
| `/display/showLineNumbers`, `/showThinkingBlocks`, `/showStatusIndicators`, `/showStatusLineRunningTime` | boolean | scalar (inside object `/display`) | VERIFIED |
| `/attribution/attributeCommitsToAgent`, `/attributePRsToAgent` | boolean | scalar | VERIFIED |
| `/model` | object | n/a | UNVERIFIED shape; off. CLI-managed (`hasChangedDefaultModel`) |
| `/channel`, `/network/useHttp1ForAgent` | | n/a | off: update channel and transport, not behaviour defaults |

Merge rule (item 3): not in the official docs ("The page doesn't describe how global and project settings are merged"). Forum thread, CLI 2026.05.09, staff reply 2026-05-16: "an empty or partially filled permissions block in the project-level `.cursor/cli.json` overrides the global one during merge"; with project `{}` the global is "deep merged" ([forum](https://forum.cursor.com/t/cli-permission-allowlist-written-to-global-config-instead-of-project-level-cursor-cli-json/160343), read 2026-11-03). Workaround `--disable-project-configs`. Conclusion: a global `permissions` write is sound only when no project `permissions` section exists, which grim cannot know across repos. Recommendation in section "Recommendation". Required fields `version`, `editor.vimMode`, `permissions.allow|deny`: grim edits an existing file; creating a partial file relies on "It repairs missing fields", unverified for first creation.

### 1.5 OpenCode

File: global `~/.config/opencode/opencode.json` or `opencode.jsonc`. Source check at tag v1.18.32 (`packages/opencode/src/config/config.ts`, read 2026-11-03): `globalConfigFile()` picks the first existing of `opencode.jsonc`, `opencode.json`, `config.json`. Project `opencode.json` is committed, so global-only. Sources: <https://opencode.ai/docs/config/>, <https://opencode.ai/docs/permissions/>, <https://github.com/anomalyco/opencode/blob/v1.18.32/packages/opencode/src/config/config.ts>.

| Field path | Type | Merge class | Status and evidence |
|---|---|---|---|
| `/model` | string `provider/model` | scalar | VERIFIED |
| `/small_model` | string | scalar | VERIFIED |
| `/default_agent` | string | scalar | VERIFIED |
| `/autoupdate` | boolean or `"notify"` | scalar | VERIFIED |
| `/snapshot` | boolean | scalar | VERIFIED |
| `/compaction/auto`, `/compaction/prune` | boolean | scalar (object `/compaction`) | VERIFIED keys; `reserved` type not shown |
| `/permission` as a string (`"allow"`/`"ask"`/`"deny"`) | string | scalar | VERIFIED. A type change between string and object at this path is an `overwrite` |
| `/permission/<tool>` where value is a string | string | scalar | VERIFIED; tools: `read`, `edit`, `glob`, `grep`, `bash`, `task`, `todowrite`, `skill`, `lsp`, `question`, `webfetch`, `websearch`, `external_directory`, `doom_loop` |
| `/permission/<tool>` where value is a pattern object (e.g. `bash`) | object pattern to action | ordered-list-as-scalar | VERIFIED: "the last matching rule winning", so order is meaning. Treat the whole object as one value |
| `instructions`, `plugin` arrays | | n/a | `instructions` is deduped-concatenated across files in source; both off: file paths and npm packages load arbitrary content |
| `shell`, `server`, `provider`, `mcp`, `lsp`, `formatter` | | n/a | out of scope or UNVERIFIED |
| `theme`, `keybinds` | | n/a | UNVERIFIED: the config page says they belong in `tui.json`; the tui page was not read |
| `share`, `username` | | n/a | UNVERIFIED: not on the config page |

Count VERIFIED, listable: 9 (`model`, `small_model`, `default_agent`, `autoupdate`, `snapshot`, `compaction.*` as one object row, `permission` string, `permission.<tool>` scalar, `permission.<tool>` pattern object).

### 1.6 Qoder CLI

Files: global `~/.qoder/settings.json` (`$QODER_CONFIG_DIR`), project `.qoder/settings.json` (committed), local `.qoder/settings.local.json` (docs: "add it to `.gitignore`"; `/allow` and `/deny` persist there). Format JSON; comments supported ("Comments are ignored during parsing"). Project and local files are ignored in an untrusted folder. Sources: <https://docs.qoder.com/cli/settings-reference>, <https://docs.qoder.com/cli/settings>, <https://docs.qoder.com/cli/permissions>, read 2026-11-03.

| Field path | Type | Merge class | Status and evidence |
|---|---|---|---|
| `/model/name` | string | scalar | VERIFIED |
| `/model/reasoningEffort` | enum (values not listed) | scalar | VERIFIED key; values not enumerated |
| `/general/defaultPermissionMode` | `default`, `accept_edits`, `plan`, `auto`, `bypass_permissions`, `dont_ask` (aliases accepted, case-insensitive) | scalar | VERIFIED. `bypass_permissions` loosens |
| `/general/vimMode`, `/general/enableAutoUpdate`, `/general/enableNotifications` | boolean | scalar | VERIFIED |
| `/output/format` | `text`/`json` | scalar | VERIFIED |
| `/ui/theme` | string | scalar | VERIFIED |
| `/ui/hideTips`, `/ui/hideBanner`, `/ui/showLineNumbers`, `/ui/compactToolOutput` | boolean | scalar | VERIFIED |
| `/permissions/allow` | string[] | unordered-list | VERIFIED. Grammar: `Tool` or `Tool(content)`; names `Read`, `Edit`, `Write`, `Bash`, `Grep`, `Glob`, `WebFetch`, `WebSearch`, `Agent`, `mcp__<server>__<tool>`; Bash `exact`, `prefix:*`, `glob`; gitignore-style paths. Loosens |
| `/permissions/ask` | string[] | unordered-list | VERIFIED |
| `/permissions/deny` | string[] | unordered-list | VERIFIED; evaluation: deny first, then tool safety checks, then ask, then allow |
| `/permissions/additionalDirectories` | string[] | unordered-list | VERIFIED; widens file access; off the first slice on the same ground as Claude's trust-gated key |
| `/security/disableYoloMode` | boolean | scalar | VERIFIED (tightens) |
| `/autoMode`, `/allowManagedPermissionRulesOnly` | | n/a | VERIFIED keys, off: classifier policy and enterprise switch |
| `/permissions/trustDirectories`, `/enabledPlugins`, `/hooks` | | n/a | out of scope |

Evaluation order, doc page for the permission keys says "Rules merge across sources. Higher-priority sources override lower ones"; the settings page says "disable and exclude lists are union-merged ... Other arrays are replaced by the higher-precedence value". These pages disagree on whether `permissions.*` arrays from project or local replace or union with the user file. For a global-only write grim's comparison is unaffected, but a project or local `permissions` array may shadow the user list: record as a limitation, not a blocker.

## 2. Copilot answer (item 2)

- No tool allow/deny key in the user table (all rows from `allowedUrls` through `worktreePathTemplate` read); the only persisted permission-shaped keys are `allowedUrls`, `deniedUrls`, `defaultPermissionMode`, `permissions.disableBypassPermissionsMode`.
- Repo-overridable keys (exactly 14): `companyAnnouncements`, `contextTier`, `deniedUrls`, `disableAllHooks`, `disabledMcpServers`, `disabledSkills`, `effortLevel`, `enabledPlugins`, `extraKnownMarketplaces`, `hooks`, `includeCoAuthoredBy`, `mergeStrategy`, `model`, `respectGitignore`. "Any other keys ... are silently ignored." So tool rules cannot live in a repo file either.
- List merge: `deniedUrls`, `disabledMcpServers`, `disabledSkills` union; `enabledPlugins`, `extraKnownMarketplaces` keyed merge with repo winning per key; "Most others are replaced". Managed `permissions`: `deny` and `ask` union, `allow` and `limitTo` intersect.
- `.github/copilot/settings.local.json`: "Personal overrides (add this to `.gitignore`)", same schema as the repo file, "takes precedence". Not auto-ignored, unlike Claude's local file.

## 3. Cursor merge and global-only soundness (item 3)

Official docs are silent; the forum thread shows replace-on-presence. Global-only is sound for `approvalMode`, `sandbox.*`, `editor.*`, `display.*` (docs: these exist only globally). For `permissions.allow|deny` it is unsound when any project `cli.json` has a `permissions` block, and a project `deny` list shadows a global `deny`, so a tightening can be silently lost. Keep the permission rows on the list only with a documented limitation, or defer them to the follow-up issue (see Recommendation).

## 4. Qoder docs location (item 4)

Found via `https://docs.qoder.com/` (sidebar names `/cli/settings-reference` and `/cli/settings`) and a web search that returned `https://docs.qoder.com/cli/permissions` and Chinese/Japanese variants. The root page itself exposes no settings link (only `/cli/overview`, `/cli/sdk/overview`, `/release-notes/qoder-cli`). All three English pages fetched successfully, so no Qoder field is blocked on a missing page. Only the array-merge contradiction (section 1.6) remains.

## 5. Marketplace keys (item 5)

| Harness | Key | Shape | Evidence |
|---|---|---|---|
| Claude | `extraKnownMarketplaces` | name to `{source:{source,repo,ref?,...}}` | settings page, trust note |
| Codex | `[marketplaces.<name>]` | `source`, `source_type` (`git`/`local`), `ref`, `sparse_paths` | [config reference](https://learn.chatgpt.com/docs/config-file/config-reference); trusted-project config allowed |
| Copilot | `extraKnownMarketplaces` | name to `{source:{directory\|git\|github}, autoUpdate?}`; repo-overridable | [config dir reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference). Copilot also reads `.claude/settings.json` and `.claude/settings.local.json` for this shared subset, so one Claude write reaches both |
| Cursor | no key found | marketplaces are made on the web dashboard (Dashboard, Plugins & MCPs, Add Marketplace) | [plugins page](https://cursor.com/docs/plugins), configuration page lists no marketplace key |
| OpenCode | no key found | `plugin` array of npm names only | [config page](https://opencode.ai/docs/config/) |
| Qoder | no key found | settings reference has only `enabledPlugins`; marketplace is `.qoder-plugin/marketplace.json` per the watchlist | [settings reference](https://docs.qoder.com/cli/settings-reference) |

## 6. OpenCode permission and Codex rules (item 6)

OpenCode `permission` ([page](https://opencode.ai/docs/permissions/), read 2026-11-03): value is a string (applies to everything) or an object `tool -> action | {pattern -> action}`; actions `allow`/`ask`/`deny`; wildcards `*` zero or more, `?` one; "last matching rule wins", catch-all `"*"` first. Pattern object order is meaning, so each `permission.<tool>` pattern object is one ordered-list-as-scalar value; per-tool string values are scalars. Source (v1.18.32): configs are combined with remeda `mergeDeep` (objects merge key by key), and only `instructions` gets array union (`mergeConfigConcatArrays`); other arrays are replaced. Cross-file merge of pattern objects is therefore key-wise, so a project file adds or overwrites patterns inside a grim-written global object; an unlisted caveat for the overwrite check, not for the global write.

`.jsonc`: both formats supported; source reads with a JSONC parser (`ConfigParse.jsonc`), and its own global update path edits `.jsonc` in place with `jsonc-parser` `modify`/`applyEdits` (`patchJsonc`) and rewrites `.json` through `JSON.stringify` (comments lost). Grim's existing span-preserving splice must therefore target the file that exists: `opencode.jsonc` first. Values go through `{env:VAR}` and `{file:path}` expansion before parsing (variable `expanded` in `loadFile`), so a literal `{env:` in a rendered value is rewritten at load.

Codex `config.toml` keys sound in a user file: `model`, `model_reasoning_effort`, `approval_policy`, `sandbox_mode`, `sandbox_workspace_write`, `default_permissions`, `permissions.<name>`, `tui.*`, `history.*`, `file_opener`, `hide_agent_reasoning`, `marketplaces.<name>` (section 1.2). Never together: `default_permissions` and `sandbox_mode`/`[sandbox_workspace_write]`.

Codex `rules/` ([rules page](https://learn.chatgpt.com/docs/agent-configuration/rules), read 2026-11-03): `.rules` files in a `rules/` folder next to each active config layer (example `~/.codex/rules/default.rules`); Starlark; `prefix_rule(pattern=[...], decision="allow"|"prompt"|"forbidden", justification=..., match=[...], not_match=[...])`, `pattern` required and non-empty, `match`/`not_match` examples validated at load; all loaded files apply together, most restrictive wins (`forbidden` > `prompt` > `allow`); project-local rules load only when trusted. Codex writes to `~/.codex/rules/default.rules` when the user allow-lists a command in the TUI. Grim can own `~/.codex/rules/grim-<package>.rules` outright (whole-file ownership, no merge), and `codex execpolicy check --rules <file>` validates them. Whether an invalid `match` example rejects only that file or all rules is not stated: UNVERIFIED.

## 7. Write-time behaviour (item 7)

| Harness | Rewrites its own settings during a session | Rejects whole file on one bad value |
|---|---|---|
| Claude | Yes: `.claude/settings.local.json` on "Yes, and don't ask again"; `~/.claude/settings.json` on `/config` and `/model`; adds `**/.claude/settings.local.json` to the global git excludes ([settings](https://code.claude.com/docs/en/settings)). Watches files and reloads without restart | Invalid JSON or a schema-rejected value: whole file skipped ("Settings Error"); a malformed single entry is skipped with a warning |
| Codex | `rules/default.rules` on TUI allow-list; trust decisions live under `projects.<path>.trust_level` (docs list the key; that the CLI writes it is inferred, not read). Whether `/model` persists to `config.toml`: not found | Not stated in the pages read; watchlist: a `cwd` on a `url` MCP server "fails the whole `config.toml`" (Codex 0.157.1) |
| Copilot | Yes: `/settings`, `/model`, `/statusline` ("Managed by `/statusline`"); writes follow symlinks to the target | No: "ignores the invalid values and shows a startup warning" |
| Cursor | Yes: "Always allow" writes to the global file; CLI "repairs missing fields", backs up a corrupted file as `.bad` and recreates it ([configuration](https://cursor.com/docs/cli/reference/configuration)) | A corrupted file is replaced after a `.bad` backup (data not lost, but the user's file is reset) |
| OpenCode | Yes: on load it inserts `"$schema"` into a config file lacking it (source, `loadFile`), and `updateGlobal` writes after `/` setting changes | Yes per the watchlist (one invalid `color` or `steps` rejects the whole config, 1.18.32); the source decodes the whole file through one schema (`decodeConfig`) |
| Qoder | `/allow`, `/deny` persist to `settings.local.json`; `/settings` panel edits user files (implied, not confirmed) | Not covered by the page |

Validation choices that follow: re-read the file immediately before the write on Claude, Cursor and OpenCode; validate every enum value grim writes against the documented value set so one bad value can never reach OpenCode or Claude; keep `$schema` untouched on OpenCode and Copilot; preserve JSONC comments on OpenCode, Copilot and Qoder; Claude is strict JSON.

## 8. Claude worktree experiment (item 8)

Setup script (outside the grimoire repo): `/tmp/claude-1000/-home-mherwig--herdr-worktrees-grimoire-duo/92c3194d-4893-44e0-a3b3-6024ffd74614/scratchpad/setup-probe.sh` created a throwaway repo `claude-worktree-probe/main` (branch `main`) with a linked worktree `claude-worktree-probe/wt` (branch `wtb`). Files:

- `main/.claude/settings.local.json`: `{"env":{"PROBE_SHARED":"from-root","PROBE_ONLY_ROOT":"yes-root"}}`
- `wt/.claude/settings.local.json`: `{"env":{"PROBE_SHARED":"from-wt","PROBE_ONLY_WT":"yes-wt"}}`

Environment: `claude 2.1.317`; `env | grep -c '^OTEL'` printed 0 and every `OTEL_*` was unset anyway. Command, run from `wt/`:

`claude -p 'Run exactly this shell command and print its output verbatim, nothing else: printenv | grep ^PROBE_ | sort' --allowedTools 'Bash' --model haiku --max-budget-usd 0.30 --output-format text`

| Probe | Local files present | Output |
|---|---|---|
| 1 (model default, `--max-budget-usd 0.05`) | both | `Error: Exceeded USD budget (0.05)`: budget too tight for one tool-using turn; no output |
| 2 (`--model haiku`, budget 0.30) | both | `PROBE_ONLY_ROOT=yes-root`, `PROBE_ONLY_WT=yes-wt`, `PROBE_SHARED=from-root` |
| 3 (same, root file deleted) | worktree only | `PROBE_ONLY_WT=yes-wt`, `PROBE_SHARED=from-wt` |

Result, 2.1.317: in a linked worktree Claude reads both files; on a shared key the main checkout's value wins; a file present only in the worktree is still read. This matches the docs ("It still reads a file an earlier version left there alongside the root file; where both set the same key, the root's value applies"). Outputs saved at `.../claude-worktree-probe/probe2.out` and `probe3.out`. Not tested: which file Claude writes on "don't ask again" (docs: the main checkout root); that needs an interactive approval and was not attempted. Consequence for grim: a project-scope write must target the main checkout's `.claude/settings.local.json` (`git rev-parse --git-common-dir`'s parent), and a value grim wrote into a worktree's own file is shadowed on shared scalar keys while lists from both files union.

## 9. Codex availability (item 9)

`codex` is not on PATH (`which codex`: not found). `npm` exists via the repo toolchain (`/home/mherwig/.herdr/worktrees/grimoire/duo/.ocx/toolchain/links/default/node/content/bin/npm`). `npm view @openai/codex version` returned `0.166.0`; `npm view @openai/codex optionalDependencies --json` listed `@openai/codex-linux-x64` as `npm:@openai/codex@0.166.0-linux-x64` (also darwin and win32, x64 and arm64). A real Codex binary is therefore obtainable on this machine through npm without root; nothing was installed. Registry network access worked from this sandbox, 2026-11-03.

## Trends

- Claude's settings shape is being cloned: Qoder (settings and permissions near-identical keys), Copilot (reads `.claude/settings*.json` for a shared subset including `extraKnownMarketplaces`). A single Claude-shaped renderer covers Claude, Qoder and part of Copilot.
- Marketplace pre-configuration has converged on a named-map key in three of six harnesses (Claude, Copilot, Codex).
- Churn signals: Codex `approval_policy` values `untrusted` and `on-failure`, `personality` and `features.personality`; Copilot `colorMode` alias and renamed sandbox keys; OpenCode `theme`/`keybinds` moved to `tui.json`, `tools` to `permission`.

## Key Findings

1. Copilot has no persisted tool rule key; the discussion's "tool grammar not found" is resolved as "no such user or repo key": <https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference>.
2. Cursor's project `permissions` section replaces, not merges, the global one (forum, staff-confirmed): <https://forum.cursor.com/t/cli-permission-allowlist-written-to-global-config-instead-of-project-level-cursor-cli-json/160343>.
3. Qoder permissions are Claude-shaped and fully documented; its two pages disagree on array merge: <https://docs.qoder.com/cli/permissions>, <https://docs.qoder.com/cli/settings>.
4. Three harnesses document a marketplace settings key; three do not (section 5).
5. Claude worktree behaviour matches its docs on 2.1.317 (section 8).
6. OpenCode merges objects key-wise and unions only `instructions`; a `.jsonc` global file takes priority and must be edited in place (source v1.18.32).
7. Codex `rules/*.rules` can be owned outright: <https://learn.chatgpt.com/docs/agent-configuration/rules>.
8. Doc ages: no vendor page is dated; Codex docs moved domains (`developers.openai.com` to `learn.chatgpt.com`), so URLs may rot.

## Recommendation

Put on the first-slice list, global scope, these keys (each VERIFIED above): Claude 16 keys through `extraKnownMarketplaces` (project scope to the main-checkout `settings.local.json`, `defaultMode` restricted to `default`/`acceptEdits`/`plan`); Codex the 13 rows in 1.2 marked VERIFIED with `approval_policy`, `sandbox_mode` and `default_permissions` flagged as mutually constrained; Copilot the rows in 1.3 (no tool rules); Cursor everything in 1.4 except the two `sandbox.mode`/`sandbox.networkAccess` value sets and `permissions.*`, which ship global-only with the project-override limitation documented, or move to the follow-up issue if the owner does not accept that limitation; OpenCode the nine rows in 1.5; Qoder the rows in 1.6 except `permissions.additionalDirectories` and `autoMode`. Own `~/.codex/rules/grim-*.rules` outright. Follow-up issue (UNVERIFIED): Copilot tool rules and the final 26k characters of the config-dir page; Cursor `sandbox.*` values and `model`; Codex top-level `web_search` and invalid-rules behaviour; Claude `sandbox.filesystem.*` merge, `tui`, `editorMode` values; OpenCode `tui.json` keys, `share`, `username`; Qoder array-merge contradiction and `reasoningEffort` values.

## Sources

| Source | Type | Date read | Relevance |
|--------|------|-----------|-----------|
| https://code.claude.com/docs/en/settings | Docs | 2026-11-03 | Files, merge, worktree file, writes, errors |
| https://code.claude.com/docs/en/settings-reference | Docs (first 200k of 448k chars) | 2026-11-03 | Key list, `defaultMode`, `sandbox.*` |
| https://code.claude.com/docs/en/plugin-marketplaces | Docs | 2026-11-03 | Marketplace sources (key detail from earlier research) |
| https://learn.chatgpt.com/docs/config-file/config-reference | Docs (169k chars, two passes) | 2026-11-03 | Codex keys, marketplaces |
| https://learn.chatgpt.com/docs/agent-configuration/rules | Docs | 2026-11-03 | `rules/` format and ownership |
| https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference | Docs (all but final 26k of 240k chars) | 2026-11-03 | Copilot settings table, repo keys, merge |
| https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/configure-copilot-cli | Docs | 2026-11-03 | `config.json`, flags |
| https://cursor.com/docs/cli/reference/configuration | Docs | 2026-11-03 | Cursor key table, writes |
| https://cursor.com/docs/cli/reference/permissions | Docs | 2026-11-03 | Cursor grammar |
| https://cursor.com/docs/plugins | Docs | 2026-11-03 | No marketplace key |
| https://forum.cursor.com/t/cli-permission-allowlist-written-to-global-config-instead-of-project-level-cursor-cli-json/160343 | Forum, staff reply 2026-05-16 | 2026-11-03 | Project-over-global permissions merge |
| https://opencode.ai/docs/config/ | Docs | 2026-11-03 | Keys, formats, merge |
| https://opencode.ai/docs/permissions/ | Docs | 2026-11-03 | `permission` semantics |
| https://github.com/anomalyco/opencode/blob/v1.18.32/packages/opencode/src/config/config.ts | Source | 2026-11-03 | Merge, jsonc, `$schema` write |
| https://docs.qoder.com/cli/settings-reference | Docs | 2026-11-03 | Qoder key table |
| https://docs.qoder.com/cli/settings | Docs | 2026-11-03 | Layers, merge, trust |
| https://docs.qoder.com/cli/permissions | Docs | 2026-11-03 | Qoder grammar and order |
| https://docs.qoder.com/ | Docs index | 2026-11-03 | No settings link on the root |
| `.claude/rules/vendor-capability-watchlist.md` | Repo | rows 2026-09-27/29 | OpenCode and Codex whole-file rejection |
| `.agents/research/research_marketplace_native_consumption.md` | Repo | 2026-11-03 | Claude marketplace entry shape |

## Negative findings

- Copilot: no tool allow/deny key in the user or repo table; managed `permissions` arrays exist but whether a user file accepts them is unread (final 26k chars of the page unfetched). `trustedFolders` is not in the settings table (it lives in `config.json`).
- Cursor: the official docs state no merge rule; no IDE permission file; no marketplace key; `sandbox.mode` and `sandbox.networkAccess` values not enumerated.
- Qoder: no `extraKnownMarketplaces` in the settings reference; invalid-value behaviour not covered; the 8-layer heading lists seven layers; array-merge contradiction between two pages.
- OpenCode: docs state neither array merge nor invalid-config handling; `tui.json` page unread; no marketplace key.
- Codex: `https://learn.chatgpt.com/docs/config-file/basic` returned 404; unknown-key behaviour not found; whether Codex writes `projects.<path>.trust_level` and persists `/model` not confirmed; top-level `web_search` undefined in the page.
- Claude: first probe failed on `--max-budget-usd 0.05` ("Exceeded USD budget"); write location of "don't ask again" in a worktree not tested; `sandbox.filesystem.*` list merge not separately stated; the settings-reference fetcher omitted the `extraKnownMarketplaces` entry in two windows, so its shape rests on the earlier research file.
- The fetcher summarises; verify quoted phrases before citing them in an ADR.
