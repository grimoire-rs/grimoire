# Research: Claude Code plugin wrapper facts for a generated mod

## Metadata

**Date:** 2026-11-18
**Domain:** packaging
**Triggered by:** pre-plan discussion on a harness-mods artifact kind; grim would generate `.claude-plugin/plugin.json` and `hooks/hooks.json` around author-supplied JS/TS code
**Expires:** 2027-02-18 (mods change every release; re-verify against the installed `claude` version)

Scope note: facts and evidence only, no recommendation. Doc pages were fetched on 2026-11-18 as raw markdown from `https://code.claude.com/docs/en/<page>.md` (pages: `plugins/loading`, `plugins/manifest-reference`, `plugins/marketplace-reference`, `plugins/mods/overview`, `plugins/mods/create`, `plugins/mods/reference`, `plugins/mods/admin`, plus `plugins/org`, `plugins/cli-reference`, `plugins/create`, `plugins/troubleshooting`, `settings-reference`). The changelog was fetched from `https://raw.githubusercontent.com/anthropics/claude-code/main/CHANGELOG.md` on 2026-11-18. Local binary: `claude 2.1.327`. Commands ran with an isolated `CLAUDE_CONFIG_DIR` under the scratch folder `.../scratchpad/modcheck2/`. No login was available, so nothing that needs a running session (module execution, interactive trust, runtime name resolution) was observed; those items are marked unverified.

## 1. Smallest valid generated wrapper

**Minimal manifest.** The manifest reference: "`name` is the only required key" (manifest-reference, fields table intro). The mods reference says the same for mods: "`.claude-plugin/plugin.json` | Yes | The plugin manifest. Mods add no required fields." (mods/reference, Files).

Observed, `claude plugin validate`:

- `{"name":"a-min"}` with `hooks/hooks.json` = `{"modules":["./register.js"]}` and `hooks/register.js`: exit 0, "Validation passed with warnings". The warnings are `version`, `description`, `author` missing (advice only). Output also prints a "Plugin directory checks" block (README_MISSING, LICENSE_MISSING, ICON_MISSING) that "do not change the result" unless `--profile directory` is passed.
- `{}` as manifest: exit 1, `name: Invalid input: expected string, received undefined`.

**Name rules.** Doc: "It must be non-empty, with no spaces, `@`, `:`, path separators, control characters, or bidirectional-formatting characters; use kebab-case." Reserved: names starting `claude-`, `anthropic-`, `anthropics-`, `cc-plugin-`, or equal to `claude`, `anthropic`, `anthropics`, `claude-code`, `claude-mods`, or putting `official` beside `claude`/`anthropic` are an Error; `claude`/`anthropic` as a whole word elsewhere is a Warning. "Claude Code still installs and loads a plugin whose name they refuse" (it is `plugin init`/`tag` that refuse). Observed: `"My Mod"` fails (`Plugin name cannot contain spaces`); `claude-code-marketplace` fails (reserved); `a.b_c` passes with a warning (not kebab-case, "Claude Code accepts it, but the Claude.ai marketplace sync requires kebab-case"). Origin ids are `<name>@<origin>`; the origin names `inline`, `skills-dir`, `synced` are reserved as marketplace names (loading.md).

**Version format.** Doc: "A version string, not checked against semver." Observed: `"version":"1.0"` and `1.0.0-beta.1+x` give no warning. Setting `version` pins a marketplace-installed plugin until the string changes; for skills-dir and `--plugin-dir` plugins the doc table says the manifest `version` is shown, or `unknown` when absent (cli-reference, `plugin list --json`).

**`hooks.json` for a mod.** Doc: "`modules`: an array with one path, relative to this file, to the hooks module, as in `"modules": ["./register.js"]`. Can also hold settings hooks under `hooks`." (mods/reference, Files). Tutorial: "List one path, relative to `hooks.json`." and "having [`modules`] is what makes the plugin a mod" (mods/create, step 3). Observed: `{"description":"x"}` alone fails (`hooks.json must have hooks ... or modules`); a bare `"modules":["register.js"]` (no `./`) passes; `"modules":"./register.js"` (string) fails (`expected array`).

**Path base and location.** The path is relative to `hooks/hooks.json`, so the file's directory is `hooks/`. Observed, all exit 0 with correct `modules:` listing:

| Test | `modules` value | Result |
|---|---|---|
| module in `hooks/` | `["./register.js"]` | passes, `Provides: modules: hooks/register.js` |
| module at plugin root | `["../register.js"]` | passes, `modules: register.js` |
| module in subfolder with relative sibling import | `["../src/main.ts"]`, `main.ts` imports `./helper.js` | passes; 5 TS warnings (TS7016 no declaration file for `./helper.js`, implicit-any parameters), result still passes with warnings |
| `.ts` in `hooks/` | `["./register.ts"]` | passes |
| escapes plugin dir | `["../../outside.js"]` | error `The module path leaves the plugin directory` |
| non-code extension | `["./register.py"]` | error, "is not named like code"; accepted extensions per docs: `.js .mjs .cjs .jsx .ts .mts .cts .tsx` |

So a module outside `hooks/` is accepted by the validator as long as it stays inside the plugin directory. Whether runtime loads it the same way is unverified (no login); no doc page states a "must live under `hooks/`" rule, and the docs' own example uses `hooks/register.js`.

**How many modules.** Doc says "an array with one path". Observed with two entries: error `hooks.json modules names one hooks module per plugin; a second entry is refused` and the directory check `"modules" that is not a list of at most one string (HOOKS_SCHEMA_INVALID)`. More than one file is possible only through imports from the single entry module.

## 2. Optional manifest fields relevant to a mod

From the manifest reference fields table (fetched 2026-11-18), with introduction versions from the changelog where the changelog states one:

| Field | Meaning | Introduced / requires |
|---|---|---|
| `displayName` | UI name instead of `name`, free-form | not found in changelog |
| `version` | version string, not semver-checked; pins a marketplace install | long-standing |
| `description`, `author{name,email,url}`, `homepage`, `repository`, `license`, `keywords` | metadata. `homepage` "Must parse as a URL, or the plugin fails to load"; since 2.1.326 a malformed display field such as `author` or `homepage` is ignored with a note instead of refusing the plugin | `description`/`author` validate warnings when missing |
| `metadata` | "Free-form object for your own data. Claude Code doesn't read it" | not found |
| `icon`, `documentationUrl`, `supportUrl`, `privacyPolicyUrl`, `termsOfServiceUrl` | directory listing only; "Claude Code ignores them at load time"; on versions before 2.1.281 `validate` prints `Unknown field` for them (so `--strict` fails) | 2.1.281 for the quiet validate |
| `defaultEnabled` | start enabled or not when no `enabledPlugins` entry | 2.1.154 (changelog) |
| `engines["claude-code"]` | semver range of Claude Code versions | "Requires Claude Code v2.1.323 or later" |
| `dependencies` | other plugins (not npm): `"name"`, `"name@marketplace"`, or `{name,marketplace,version}`. For mods it also names plugins whose mods API namespace the mod uses: "There's one directory for each plugin your `plugin.json` lists under `dependencies`" (mods/create, generated types) | enforcement 2.1.143 |
| `userConfig` | values prompted at enable time, delivered to `register(on, options)` ("`options` holds the values of the `userConfig` fields the manifest declares") | 2.1.83; `options` pickers 2.1.271 ("users on Claude Code versions before v2.1.271 can't load the plugin") |
| `types` | `.d.ts` for `$.state` values and `$` nouns of a mod. "To leave `types` out of the manifest, you need v2.1.299 or later" (default path `types/index.d.ts`, required "When the mod uses `$.state` or adds a namespace to the mods API") | validate default path 2.1.299 |
| `experimental.{themes,monitors,evals}` | "whose manifest shape may still change"; top-level `themes`/`monitors` still load with a warning | `experimental` container 2.1.129 |
| `skills`, `commands`, `agents`, `hooks`, `mcpServers`, `lspServers`, `outputStyles`, `workflows`, `channels` | component paths, all containment-checked | n/a |

**Which fields make an older Claude Code reject the manifest.** Doc: "To keep the plugin working on v2.0.15 through v2.1.75, omit `engines`. Those versions reject the manifest with an `Unrecognized key` error when it has the field" (manifest-reference, `engines` row). Same wording for the marketplace file: "Those versions reject the whole `marketplace.json` when any entry has the field." Versions 2.1.76 to 2.1.322 are not described: current docs say "An unrecognized top-level key is stripped, and ... the plugin loads" (manifest-reference, Unrecognized fields), which implies those versions strip `engines` and load. Unverified: no old binary was available. Strict objects (`userConfig` options, `channels`, `lspServers`, `monitors`) reject unknown keys and the plugin does not load. A `userConfig` `options` key is the other documented hard cutoff (2.1.271).

Observed on 2.1.327: `engines` accepted silently; an unknown top-level key `bogus` gives warning `Unknown field 'bogus'. Claude Code ignores it at load time.`; `dependencies:["lodash"]` accepted by validate.

Option evidence (no pick):
- (a) minimal manifest only: avoids every versioned-key hazard above; the validator itself rates `version`, `description`, `author` as warnings, and `claude plugin validate --strict` would fail on those.
- (b) fixed passthrough list: each field above has a stated minimum version; `engines`, `options`, `types`-less all gate older clients.
- (c) arbitrary JSON: unknown top-level keys are stripped with a warning on 2.1.327 (loads), but a strict-object key (`userConfig`, `channels`, `lspServers`, `monitors`) stops the load; `--strict` turns the warnings into failures.

## 3. Version compatibility of the wrapper, 2.1.287 to 2.1.327

Mods began in the first release of the range: "2.1.287: Added Claude Mods: plugins may now modify deeper behavior" (CHANGELOG). The admin page confirms "Mods are on by default in Claude Code v2.1.286 and later". Mods-API changes in the range are numerous (new hooks, `$` methods, `Box`/`Text` props); the lines below are only those touching manifest, `hooks.json`, module loading, validation or plugin locations.

| Version | Change (CHANGELOG, abbreviated) | Breaking for a generated wrapper? |
|---|---|---|
| 2.1.289 | `validate` now handles a folder holding both a marketplace manifest and a plugin | no |
| 2.1.290 | validate lists gating hooks with `.catch` status (`gatingHooks`); fixed module load refusal on a destructured option named like a top-level function | no |
| 2.1.292, 2.1.295 | load/validate fixes: `$.state` call slowness, rebound top-level `var` now refused with a clear error, deep nesting | modules that rebind a top-level `var` are refused (2.1.292 "such a module is now refused") |
| 2.1.295 | generated type files no longer written into a `--plugin-dir` plugin in `-p`/SDK sessions, "only where a mod is being developed" | no |
| 2.1.296 | note when a second same-name plugin's hooks module "does not load" | one module per plugin name |
| 2.1.299 | `validate` accepts `$.state` with no `types` key when `./types/index.d.ts` exists; note for hooks skipped under managed settings; `plugin test` changes | no |
| 2.1.304 | removed validate notes predicting where hooks run | output text changed |
| 2.1.309 | clearer error for too-deep module | no |
| 2.1.321 | headless sessions reload mods in `.claude/skills` on file change (`CLAUDE_CODE_PLUGIN_DIR_WATCH=0` disables); `.claude/skills` inside `~/.claude/plugins` no longer loaded as skills; managed `extraKnownMarketplaces` takeover rules | no |
| 2.1.323 | `engines["claude-code"]` added ("plugins outside their stated Claude Code version range are not installed, updated or loaded"); TypeScript type checking of a hooks module added to validate ("each finding is a warning, and `--strict` fails on it"); `validate` fails a mod whose `engine.create` hook is not in the one accepted form ("installed mods keep loading"); `agent` setting deprecated | `engines` breaks only pre-2.1.76 clients per doc; type check adds warnings |
| 2.1.325 | "plugin loading to skip, and `claude plugin validate` to report, some unsafe paths and links in a plugin's folder"; `.claude/skills` inside `--plugin-dir` plugins no longer loaded from a session started above | symlinks inside the plugin may be skipped |
| 2.1.326 | malformed display field now loads with a note instead of refusing | no |

Shape that works across the whole range as far as the sources show: `plugin.json` with `name` (+ optional `version`, `description`, `author`), `hooks/hooks.json` `{"modules":["./<file>"]}`, one module using only relative imports and `claude-code`. No version in the range is documented as removing or renaming `modules`, `hooks.json` or `plugin.json`. Unverified: the shape was run only on 2.1.327; no earlier binary was tested.

**Compatibility promise.** For the mods API: "The events and methods can change between releases, so trust these files over any page, this one included, when they disagree." (mods/create) and "The events and methods can change between releases, so your README is the place to say which Claude Code version you tested with." (mods/create, share section). For the manifest: the only stated promises are the per-field minimum versions and the `engines` cutoff wording above, and `experimental` carries "manifest shape may still change". No page found promises stability for `plugin.json` or `hooks.json`.

## 4. Where to install without a marketplace

Evidence from loading.md, create.md, cli-reference.md, settings-reference.md and experiments.

**(a) Skills-directory origin** (`<name>@skills-dir`).
- Introduced: "2.1.157: Plugins in `.claude/skills` directories are now automatically loaded, no marketplace required" (CHANGELOG).
- Doc: "`@skills-dir` | You saved a plugin directory that has a `.claude-plugin/plugin.json` under `~/.claude/skills/` or the project's `.claude/skills/` | The manifest's `defaultEnabled`, unless a settings file sets `"<name>@skills-dir"` to `true` or `false`" (loading.md).
- Scopes: `user` (`~/.claude/skills/`) and `project` (primary working directory's `.claude/skills/` only; "unlike plain skills and commands, [it] doesn't search parent directories"). `plugin list --json` `scope` is "`user` or `project` for skills-directory plugins". No `local` scope.
- Trust: project scope "loads only after the same trust check that applies to project allow rules ... Trusting a parent folder or running with `-p` isn't enough"; "Neither the trust check nor these component limits apply to a plugin under your own `~/.claude/skills/`." Repository plugins: no background monitors, per-server MCP approval. Observed in an untrusted scratch project: `1 project-scope directory under ./.claude/skills/ that may load as a plugin was skipped because this workspace was not trusted when plugins were scanned.` Interactive mods also wait: "In an interactive session in a directory the user hasn't trusted yet, no mod loads until they answer the trust prompt" (mods/admin).
- In place: "`--plugin-dir` and skills-directory plugins: the directory loads in place and is never copied." Node dependencies are not installed for in-place plugins.
- Updates/removal: files are read from the folder; headless sessions reload mods in `.claude/skills` when files change (2.1.321). Removal = delete the folder, or `claude plugin disable <name>@skills-dir`. Observed: `{"enabledPlugins":{"only-manifest@skills-dir":false}}` gives `Status: ✘ disabled`.
- `CLAUDE_CONFIG_DIR`: user root is `$CLAUDE_CONFIG_DIR/skills/` (observed: folders under `cfg4/skills/` listed with `Scope: user`).
- Experiments (user scope, `CLAUDE_CONFIG_DIR=cfg4`, `claude plugin list`):
  - directory with only `.claude-plugin/plugin.json` and `hooks/` (no `SKILL.md`): `only-manifest@skills-dir ... Status: ✔ loaded`. A `SKILL.md` is not required.
  - sibling plain skill dir (`SKILL.md`, no manifest): not listed by `plugin list` (it is a skill, not a plugin).
  - plain skill `dup` and plugin folder `dup2` whose manifest name is `dup`: only `dup@skills-dir` (path `.../dup2`) listed, loaded. Runtime skill-vs-plugin name resolution not observable without a session (unverified). Plugin components are namespaced `<plugin>:<name>` (manifest-reference, `name`).
  - same manifest name as a marketplace-installed plugin: `good@skills-dir: ✘ Not loaded — the name "good" is already taken by an installed plugin (good@local-mkt), which takes precedence. Give the plugin at <path> a different "name" (in plugin.json or SKILL.md frontmatter) to load this copy.`
- Name-conflict order (loading.md, Name conflicts): managed `enabledPlugins` > `--plugin-dir`/`--plugin-url`/`CLAUDE_CODE_PLUGIN_DIRS` > installed marketplace plugin > skills-dir ("Between two of these, the copy under `~/.claude/skills/` loads and the project's `.claude/skills/` copy is dropped") > claude.ai synced. Two enabled plugins with the same manifest name: only one registers a hooks module ("one plugin per name loads a hooks module"; note requires 2.1.296).

**(b) Local directory marketplace plus `enabledPlugins`.**
- Mechanism: `extraKnownMarketplaces` entry `{"source":{"source":"directory","path":"<abs>"}}` plus a `marketplace.json` with relative-path entries (`"source":"./plugins/good"`), and `enabledPlugins["good@local-mkt"]: true` (settings-reference; marketplace-reference, relative path source).
- Experiment: `claude plugin marketplace add <abs dir> --scope user` printed `Successfully added marketplace: local-mkt (declared in user settings)`; `claude plugin install good@local-mkt --scope user` succeeded; `settings.json` then held both `extraKnownMarketplaces` and `enabledPlugins`; `plugin list` printed `Read from: <path inside the marketplace dir>`, i.e. loaded in place, no cache copy.
- Scopes: user, project, local for `enabledPlugins`/`extraKnownMarketplaces` ("`.claude/settings.json`", "`.claude/settings.local.json`"). Trust: repository `extraKnownMarketplaces` entries are honored "only after you accept the workspace trust dialog ... in a folder you haven't trusted, including a `-p` run there, it ignores them without a message."
- Updates: "A plugin loaded in place from a marketplace you added from a local path loads its current source files at every session start, whatever its version string says"; `/reload-plugins` or next session. Marketplace refresh before install is skipped for local `file`/`directory` sources. Dependencies are not installed there ("Install them there yourself").
- Removal: `claude plugin uninstall` / remove entries. Name conflicts: an installed marketplace plugin beats a same-named skills-dir plugin (observed above).
- `CLAUDE_CONFIG_DIR`: `settings.json`, `plugins/installed_plugins.json`, `plugins/known_marketplaces.json` all landed under the config dir (observed in `cfg5/`). The plugins root can also be moved with `CLAUDE_CODE_PLUGIN_CACHE_DIR` (loading.md).
- Git-hosted marketplace test was not completed (local git source not accepted as a path in the CLI); remote-source `engines` enforcement is therefore unverified (see 6).

**(c) Plugin cache directly.** Doc: cache is `cache/<marketplace>/<plugin>/<version>/`, written by install; `installed_plugins.json` and `known_marketplaces.json` hold records. Experiment: a hand-made `plugins/cache/ghost-mkt/ghost/1.0.0/` plus `plugins/ghostdir/` with manifests, and no records: `claude plugin list` printed `No plugins installed.` Cache copies are replaced and orphaned dirs are removed after 14 days when a version changes. No documented supported path for writing there.

**(d) Other documented mechanisms.**
- `--plugin-dir`, `--plugin-url`, `CLAUDE_CODE_PLUGIN_DIRS` (2.1.280, env var; "Project and local settings can't set this variable"): session-only, origin `@inline`. Observed `CLAUDE_CODE_PLUGIN_DIRS=<dir> claude plugin list` printed `a-min@inline ... Status: ✔ loaded` under `Session-only plugins`. Not persistent, so a tool cannot install through it except by wrapping launch.
- Project `.claude/plugins/` is not scanned: "Claude Code doesn't scan a project's `.claude/plugins/` directory." Observed: `.claude/plugins/pp` gave `No plugins installed.`
- Seed directory (`CLAUDE_CODE_PLUGIN_SEED_DIR`) for containers and CI (plugins/org, "Seed containers and CI"; not tested).
- `claude plugin init <name>` scaffolds a skills-dir plugin: "It will auto-load next session as my-tool@skills-dir."
- A mod Claude writes during a session lives in `~/.claude/dev-mods/<session-id>/` and loads only that session (mods/create).

## 5. Managed settings that block loading

Doc evidence, all by location from question 4. Nothing below was run against real managed settings (needs root-owned `/etc` files; not attempted).

| Setting | Effect | Skills-dir | Local directory marketplace | `--plugin-dir` | Observable from outside |
|---|---|---|---|---|---|
| `strictPluginOnlyCustomization` (true or any array entry) | "If you lock any surface, Claude Code also stops loading skills-directory plugins" (settings-reference); whole plugin skipped if any one kind is locked | blocked | not affected ("Doesn't restrict which plugins users install") | not affected unless `disableSideloadFlags` | `claude plugin list` shows "no plugin and no error for a skipped folder" (loading.md); `claude plugin init`/`enable` stop with `Plugins from ~/.claude/skills/ are blocked by your organization's managed settings` (troubleshooting); in-session `/status` `Setting sources` line lists `Enterprise managed settings`. Exit code of that stop is `1` per the cli-reference shared codes (not run) |
| `strictKnownMarketplaces` allowlist without `{ "source": "skills-dir" }` | "any allowlist, even an empty one, stops Claude Code loading `@skills-dir` plugins" | blocked | blocked unless the directory source is allowed; install refused with `blocked by enterprise policy` naming the source | not covered ("Doesn't ... block `--plugin-dir`") | same silent skip for skills-dir; policy error text for marketplaces in `/plugin`; install exits 1 |
| `blockedMarketplaces` with `{ "source": "skills-dir" }` | stops skills-dir loading, "without restricting any marketplace" | blocked | not affected | not affected | as above |
| `disableSideloadFlags` | rejects `--plugin-dir`, `--plugin-url`, `CLAUDE_CODE_PLUGIN_DIRS`; "Claude Code exited with this message and `Plugins, custom agents, and MCP servers can only be loaded from sources your administrator has approved.`" | not affected | not affected | blocked, startup exit | visible at startup |
| managed `enabledPlugins` `false` | "blocks at every scope and hides the plugin" | blocks by id | blocks by id | `--plugin-dir copy of "<name>" ignored: plugin is locked by managed settings` | `/plugin` Errors tab |
| `allowManagedModsOnly` (option on `cc-plugin-sec-default@builtin`, `pluginConfigs`) | "No mod a user brings runs its hooks: a mod in a plugin the user installed, a mod loaded with `--plugin-dir`, and a mod Claude wrote during a session"; org-owned mods exempt ("Every other mod counts as a user's") | the page does not name skills-dir; by its own rule a skills-dir mod is a user's | a directory marketplace counts as the org's only when managed settings name it by absolute path (mods/admin, "Install your organization's mods") | blocked | the plugin still loads; hooks are refused. Evidence of refusal is "the guard's message" in the transcript and the debug log (`claude --debug`), plus a `/plugin` note naming skipped hooks (2.1.299). Whether `claude plugin list` or `validate` shows it was not observed |
| managed `extraKnownMarketplaces` | "a declared name can't be added from another source" (2.1.321) | n/a | a same-named user marketplace from another source is hidden | n/a | n/a |
| built-in guard `sec-default@builtin` | loads when managed settings or `managed-mcp.json` exist, or a Team/Enterprise login; blocks user mods on listed events (`prompt.*`, `classic.*`, `settings.read`, `session.compact`...) | applies | applies | applies | `/plugin` note "Its hooks on ... do not run here" |

Option evidence: (a) cannot observe: for a skills-dir plugin, `claude plugin list` and `/plugin` show nothing at all and no error (loading.md quote above), and `allowManagedModsOnly` leaves no `list` status documented. (b) can observe, by a documented command: `claude plugin init`/`enable` fail with a named message; `claude plugin install` for a marketplace plugin fails with the policy error; and absence from `claude plugin list --json` after a write is itself a signal (indirect). `--debug` log lines and `/status` need a session.

## 6. `engines` gate

Doc: "`engines` | The Claude Code versions that the plugin works on, as a semver range under the `claude-code` key, such as `"engines": { "claude-code": ">=2.1.400" }`. Requires Claude Code v2.1.323 or later." Changelog 2.1.323: "plugins outside their stated Claude Code version range are not installed, updated or loaded, and install and update pick the newest version that fits." Marketplace entries carry the same field (marketplace-reference line 94); a bad range gives `Must be a version range such as ">=2.1.0"; got "<value>". It will be ignored at load time.` (Error, `plugins[i].engines.claude-code`) and a non-object gives a warning.

Observed on 2.1.327, with `engines: {"claude-code": ">=9.0.0"}`:
- skills-dir plugin: `Status: ✔ loaded`, `Note: It needs Claude Code >=9.0.0, this is 2.1.327. It loads anyway, because it's read from a local folder`; JSON `noteDetails: [{"type":"engine-range-exempt"}]`.
- local-directory-marketplace plugin: `claude plugin install future@local-mkt` succeeded and printed `Note: "future" needs Claude Code >=9.0.0, this is 2.1.327, but loads anyway because it's read from a local folder`; `plugin list` shows `Status: ✔ enabled` with the same note.
- `claude plugin validate`: no output about `engines` for a satisfied or unsatisfied range.

So for the two no-marketplace locations tested, the gate is advisory only. The docs do not state this exemption anywhere; it was found only by running the binary. Enforcement for remote sources (git, github, npm, url) is documented by the changelog line but unverified here, as is the exact user-facing text for a rejected remote install. Pre-2.1.323 clients ignore or reject the key as described in section 2.

## 7. Dependencies

Quote and source: "Import only from files inside the plugin directory, by relative path. The one bare import allowed is `claude-code`, for types and a few helpers." (mods/create, "Check what Claude Code reads from your mod"). The validator's own error: `cannot import "lodash" (from hooks/register.js): a hooks module imports its own files by relative path and "claude-code", nothing else` (observed; same for `node:fs`). Dynamic `import()` fails: `a dynamic import(); a hooks module imports its own files with an import declaration`. `require` and CommonJS fail (docs: "Write every file as an ES module, with `import` and not `require`"). The tutorial adds: "You don't need Node.js, a bundler, or a build step, because Claude Code loads `.js` and `.ts` files directly."

Supported routes for outside code:
- Vendored files: observed that a relative import into a vendored folder (`import {a} from "../node_modules/foo/index.js"`) passes validate with no complaint. Such files are subject to the same import rules and the type check. Not run in a live session.
- Bundled single file: an esbuild-style single ES module (`var a=1;export function register(on){...}`) passes validate (`hooks: tool.call`). A bundle that retains `require` or dynamic `import()` would fail the rules above (inferred from the rules; only the dynamic and CJS cases were run).
- npm packages in `package.json`: loading.md installs "the plugin's Node.js package dependencies" only when copied into the cache and only "so the plugin's hooks and MCP servers can load them"; there is no statement that a hooks module may import them, and the import rule forbids bare specifiers. For in-place plugins (skills-dir, directory marketplace) nothing is installed. Whether a module could load a package by relative path to `node_modules` at runtime is unverified.
- Sharing between mods: plugin `dependencies` plus a namespace added to the mods API (mods/create types section); not a general code-sharing route (not tested).
- A mod may run programs through `$.process.run` (mods/admin: "reaches the network with the user's own access"), which is outside the import rule.

`claude plugin validate` says nothing special about a `node_modules` folder itself; it analyses only the module and its import graph.

## 8. Static review output

Run on the minimal mod (`/modcheck2/exp/a-min`, `claude plugin validate ./a-min`), the lines in order:

```
  ❯ ./register.js hooks: tool.call
  ❯ ./register.js gating hook without .catch: tool.call
  ❯ ./register.js calls: $.ui.invalidate

Provides:

  ❯ modules: hooks/register.js
```

With matchers the doc example reads `hooks: session.start, tool.call, command.run{command=tally}, ui.render{component=Spinner}` and `calls: $.command.register, $.ui.invalidate`; `env reads:`/`env writes:`/`state reads:`/`state writes:` lines appear when used (mods/create). A hook with no mods API call prints `calls: nothing on $` (observed). The path in the prefix is the `modules` string as written (`../src/main.ts` printed as `../src/main.ts`).

Stability: the text is not documented as stable; the changelog shows it changing (2.1.290 added gating-hook lines, 2.1.304 "Removed the notes ... that predicted where a mod's hooks would run", 2.1.323 added type-check warnings). Machine-readable form: "`--json` | Output the validation report as one JSON object with the same exit codes. Requires Claude Code v2.1.259 or later" (cli-reference, plugin validate). Observed JSON keys: `success`, `strict`, `target`, `manifest{errors,warnings,notes,gatingHooks,modules}`, `contents[]` (hooks entry with `modules[].uses{events,calls,env,state}`, `hooks[{pattern,matcher,isGate,hasCatch}]`, `nouns`), `provides{modules,hooks,...}`, `advice`, `directory{profile,findings}`. The docs name `gatingHooks` and say a mod's `e.uses.calls` use the same facts "without the `$.`" (mods/admin); no page documents the JSON schema or promises its stability. Exit codes: "`0` on success and `1` on failure. `validate` adds exit `2` for an unexpected error" (cli-reference).

## negative:

- No doc page states that a hooks module may live outside `hooks/`, nor forbids it; only the validator behavior (passes for in-plugin paths) is evidence. Runtime load of a non-`hooks/` module is unverified.
- No doc page states any stability promise for `plugin.json` or `hooks.json`; the only promises are per-field minimum versions and the `engines` cutoff.
- The `engines` local-folder exemption is not in the docs; observed only. A docs-only reading would predict "not loaded".
- No doc page says whether `allowManagedModsOnly` applies to skills-dir mods by name; the inference rests on the "every other mod counts as a user's" sentence.
- Skills-dir plugin load under managed `strictPluginOnlyCustomization` is silent in `plugin list` per the docs; there is no documented status to read.
- A `.claude/plugins/` project directory and a hand-written cache entry do not load (observed).
- Older binaries (2.1.287 to 2.1.322, and 2.0.15 to 2.1.75) were not available, so cross-version claims rest on docs and changelog only.
- Not verified at all: module execution (needs login), project-scope load after the trust dialog, any managed-settings behavior, remote-source `engines` rejection text, skill-vs-plugin same-name runtime resolution.

## leads:

- Live-session check: run a probe module from skills-dir (user scope) with a valid `$.fs.write` marker, including a module at plugin root and one in a subfolder, to settle where modules may live; needs a login.
- Managed-settings test: a throwaway root-owned `managed-settings.json` on a disposable machine or container with `strictPluginOnlyCustomization` and `strictKnownMarketplaces` to see what `claude plugin list --json`, `/status` and `--debug` print.
- Version matrix: install pinned older `claude` builds (for example 2.1.287, 2.1.299, 2.1.322) in containers and run `validate` plus a probe session on the generated wrapper, including `engines` rejection below 2.1.76.
- Competitive/vendor: how the Claude Code `plugin init --with` scaffolds and the built-in `plugin-authoring` skill generate the same wrapper (a first-party precedent for a generated manifest).
- Repo archaeology: the `mods/sec-default` directory in the Claude Code repository is public (mods/admin) and is a real multi-file mod example.
- Docs stability: the `mods/types/claude-code.d.ts` file in the Claude Code repository is named as the authoritative reference; its history would show API churn rate.

## Sources

- https://code.claude.com/docs/en/plugins/loading.md : origins, trust, in-place loading, name conflicts, dependency install (fetched 2026-11-18)
- https://code.claude.com/docs/en/plugins/manifest-reference.md : fields, `engines`, name rules, unrecognized fields (2026-11-18)
- https://code.claude.com/docs/en/plugins/marketplace-reference.md : `engines` on entries, relative-path sources (2026-11-18)
- https://code.claude.com/docs/en/plugins/mods/overview.md, `create.md`, `reference.md`, `admin.md` : mod layout, import rule, validate output, managed controls (2026-11-18)
- https://code.claude.com/docs/en/plugins/org.md, `cli-reference.md`, `create.md`, `troubleshooting.md` ; https://code.claude.com/docs/en/settings-reference.md : managed keys, `validate --json`, exit codes, `plugin init` (2026-11-18)
- https://raw.githubusercontent.com/anthropics/claude-code/main/CHANGELOG.md : version history 2.1.287 to 2.1.327 (2026-11-18)
- Local runs: `claude 2.1.327`, `claude plugin validate`, `claude plugin list [--json]`, `claude plugin marketplace add`, `claude plugin install`

## Scratch artifacts

Under `/tmp/claude-1000/-home-mherwig--herdr-worktrees-grimoire-duo/6dfdf366-5cef-4de7-a681-0f9aeeb626a4/scratchpad/modcheck2/`: helper scripts `mk.sh` and `v.sh`; `docs/` (fetched markdown and changelog slices); `exp/` (throwaway plugins a-min, b-root, c-sub, d-two, e-ts, f-noname, n-*, p-*); isolated config dirs `cfg`, `cfg4` to `cfg8`; marketplaces `mkt`, `mkt2`; projects `projp`, `projs`.
