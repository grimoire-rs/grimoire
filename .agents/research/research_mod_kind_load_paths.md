# Research: load paths for a generated `mod` artifact (Claude project scope, OpenCode V2)

## Metadata

**Date:** 2026-11-18
**Domain:** packaging
**Triggered by:** `/hex-architect` run on the harness-mods artifact kind; three load paths were assumed and unobserved
**Expires:** 2027-02-18 (both clients ship weekly; re-run `$L/run.sh` and `$L/ocrun.sh` against the installed versions)

Binaries observed: `claude 2.1.327`; `opencode v2.3.2` (V2). `$L` below is the scratch folder `.../scratchpad/loadtest/`. Every client ran with `HOME`, `XDG_*`, `CLAUDE_CONFIG_DIR` and `OPENCODE_CONFIG_DIR` inside `$L`. No login, no credential. Prior checks (`research_harness_mods_claude_wrapper_facts.md` sections 1 and 4, `research_harness_mods_opencode_plugin_facts.md`) are not repeated.

Method that made loading observable without a login: `claude -p hi --debug-file <f> < /dev/null` prints "Not logged in" but only after plugins and hooks modules load and `session.start` fires. A probe module hooks `session.start` and calls `$.fs.write("$L/marker-<name>.txt", ...)`. For OpenCode, `opencode run --standalone --print-logs --log-level debug -m none/none x` starts a private server, loads plugins, then fails with `Model unavailable: none/none` (no model request is made with an unknown model).

## Summary of observations

| # | Question | Result |
|---|---|---|
| 1 | Claude project-scope marketplace outside `.claude/skills/` | Observed. Works from `.claude/settings.json` or `.claude/settings.local.json`; needs a prior-recorded workspace trust; relative path works from the second session on, absolute path works in the first |
| 2 | Module outside `hooks/` | Observed loading and executing: in `hooks/`, at plugin root, in `src/` with a sibling helper |
| 3 | OpenCode V2 subfolder with generated `index.ts` | Observed. Marker written; helper beside the plugin loads only as part of the plugin |
| 4 | OpenCode global scope | Observed for both `OPENCODE_CONFIG_DIR` and the XDG default |
| 5 | Removal | Observed for both clients (details in section 5) |

Interactive-only behavior (trust dialog text, `/plugin` UI, `/reload-plugins`, tool and UI hooks firing) is not observed; section 6 lists it.

## 1. Claude, project scope, directory marketplace

### Files used (all under the scratch project `$L/projA/`)

```
.claude/settings.json                         (or settings.local.json)
.claude/mods/.claude-plugin/marketplace.json  (marketplace root = .claude/mods)
.claude/mods/probe/.claude-plugin/plugin.json
.claude/mods/probe/hooks/hooks.json
.claude/mods/probe/src/main.ts, src/helper.ts
```

`marketplace.json` (the `owner` object is required: without it `claude plugin validate .claude/mods` fails with `owner: Invalid input: expected object, received undefined`):

```json
{"name":"proj-mods","owner":{"name":"grim"},"plugins":[{"name":"probe","source":"./probe"}]}
```

Settings, working form (relative path, `./` prefix):

```json
{"extraKnownMarketplaces":{"proj-mods":{"source":{"source":"directory","path":"./.claude/mods"}}},"enabledPlugins":{"probe@proj-mods":true}}
```

### Matrix, each row `claude -p` in a fresh `CLAUDE_CONFIG_DIR` with trust pre-recorded, then a second run on the same config dir

Decisive signal: marker file plus debug line `hooks module probe@proj-mods loaded (worker, ...); events: session.start`.

| Case | Registration | 1st session | 2nd session |
|---|---|---|---|
| A2 | `settings.json`, absolute `path` | loaded, marker | loaded |
| B | `settings.json`, `./.claude/mods` | not loaded; debug `Skipping orphaned enabledPlugins entry probe@proj-mods: marketplace not registered`, then `[reconcile] ... Added marketplace source: proj-mods` | loaded, marker |
| C | `settings.local.json`, `./.claude/mods` | not loaded | loaded |
| E | `settings.local.json`, `.claude/mods` (no `./`) | not loaded | loaded |
| F | `settings.json`, `${CLAUDE_PROJECT_DIR}/.claude/mods` | not loaded | not loaded (variable is not expanded) |
| G | `extraKnownMarketplaces` only, no `enabledPlugins` | not loaded | not loaded |
| H | marketplace in `settings.json`, `enabledPlugins` in `settings.local.json` | loaded | loaded |
| I | case A2 settings, folder not trusted | not loaded | not loaded |

Answers:

- Which JSON works: both keys, in `settings.json` or `settings.local.json`, or split across the two (H). `enabledPlugins` is required (G). Observed.
- Relative paths: resolve against the project root and work, but the marketplace is registered by a background reconcile that runs after plugin loading, so the first session in a fresh config dir does not load the mod and the next one does. The reconcile then writes the resolved absolute path to `$CLAUDE_CONFIG_DIR/plugins/known_marketplaces.json` (`"installLocation": "<abs>/.claude/mods"`). An absolute path loads in the first session (debug: `Marketplace "proj-mods" is declared in settings (dir:...) but not recorded in known_marketplaces.json yet: its plugins load in place from the declared path`). Observed.
- `claude plugin marketplace add ./.claude/mods --scope project|local` writes the path into `settings.json` / `settings.local.json` as an absolute path, and prints `Successfully added marketplace: proj-mods (declared in project settings)`. `claude plugin install probe@proj-mods --scope project|local` then adds `"enabledPlugins":{"probe@proj-mods":true}` to the same file. A hand-written settings file and the CLI produce the same keys. Observed.
- `claude plugin list` output (non-session command):
  - settings written by hand, no `install`: `No plugins installed. Use \`claude plugin install\` to install a plugin.` and `--json` prints `[]`, even though a session loads the mod. The CLI ignores repo-authored enablement. Observed.
  - after `claude plugin install probe@proj-mods --scope project`:
    ```
    probe@proj-mods
      Version: 0.1.0
      Read from: <abs>/.claude/mods/probe
      Scope: project
      Status: ✔ enabled
    ```
    `--json`: `{"id":"probe@proj-mods","version":"0.1.0","scope":"project","enabled":true,"installPath":"<config>/plugins/cache/proj-mods/probe/0.1.0","readFromFolder":"<abs>/.claude/mods/probe","projectPath":"<abs project>","projectEnabled":true}`. The mod loads in place (`readFromFolder`); the `installPath` cache entry is a record, not the code that runs. With `--scope local` the row prints `Scope: local`. Observed.
- Untrusted project: `-p` prints nothing, `plugin list` prints `No plugins installed`, marker absent; the only trace is the debug line `Skipping orphaned enabledPlugins entry ...: marketplace not registered`. `known_marketplaces.json` is not written. Observed. Matches the docs: "in a folder you haven't trusted, including a `-p` run there, it ignores them without a message" (https://code.claude.com/docs/en/settings-reference.md, `extraKnownMarketplaces`).
- Does a project-scope marketplace prompt on first open: docs name only the workspace trust dialog; trust of the exact folder is required (https://code.claude.com/docs/en/permissions.md#what-runs-before-you-trust-a-folder: `extraKnownMarketplaces` from the repository are "Not used, and no dialog is offered" when only a parent folder is trusted). No marketplace-specific consent prompt is documented. The dialog itself is not observed (interactive only).
- Pre-recording trust: documented key `projects["<path>"].hasTrustDialogAccepted = true` in `~/.claude.json`, which is `$CLAUDE_CONFIG_DIR/.claude.json` with the variable set. Used in every trusted row:
  ```json
  {"projects":{"<abs project path>":{"hasTrustDialogAccepted":true}}}
  ```
  The project here is a git repository root; the key is the repository root. Observed to take effect (trusted rows load, case I without it does not).
- Can a file grim owns alone carry the keys: not by any documented file. `extraKnownMarketplaces` and `enabledPlugins` are settings keys with "Any file" scope, and for a project the only repository-level files are `.claude/settings.json` (shared) and `.claude/settings.local.json` (per-clone, gitignored by Claude Code convention, but also a user file). Other carriers: `--settings <file>` (session only), user `$CLAUDE_CONFIG_DIR/settings.json` (user scope, not project), managed settings (admin). A grep of the settings reference finds no drop-in directory for project settings. So grim must merge keys into one of the two shared JSON files. Not observed beyond that grep; the claim is "none documented".
- Managed `strictKnownMarketplaces` and similar are covered in `research_harness_mods_claude_wrapper_facts.md` section 5; not retested.

## 2. Claude, module location

Three plugins in one marketplace, each `hooks.json` pointing at a different place; all passed `claude plugin validate` and all loaded and ran in one `claude -p` session (marker per plugin; debug `hooks module probe@proj-mods loaded (worker, environment 1 ...)`, `probe-root@proj-mods ... environment 2`, `probe-hooks@proj-mods ... environment 3`, then `$.fs.write (probe): <marker> (31 bytes)` per plugin). Observed.

| Plugin | `hooks/hooks.json` | Module file |
|---|---|---|
| `probe` | `{"modules":["../src/main.ts"]}` | `src/main.ts`, imports `./helper.js` (file `src/helper.ts`) |
| `probe-root` | `{"modules":["../register.ts"]}` | `register.ts` at plugin root |
| `probe-hooks` | `{"modules":["./register.ts"]}` | `hooks/register.ts` |

Validate output for `probe`: `../src/main.ts hooks: session.start`, `../src/main.ts calls: $.fs.write (via mark)`, `Provides: modules: src/main.ts`; the validator follows the helper import (`calls ... (via mark)`).

How far `claude -p` goes without a login: plugins load, hooks modules load in a worker, `engine.create` and `plugin.register: probe (user, probe@proj-mods) ... admitted` run, `session.start: raised (surface none, not interactive)` fires and the probe's hook runs (`probe@proj-mods session.start settled in 18.2ms (worker hop, next() included)`), then the run prints `Not logged in · Please run /login` and exits 0. Mod loading is logged before that. Not observed without a login: `prompt.*`, `tool.call`, `ui.render`, `command.run` hooks, anything needing a model turn, and the interactive path.

## 3. OpenCode V2, project scope, subfolder with generated index

Binary: the V2 CLI is the npm package `@opencode/cli` (the unscoped `opencode` does not exist on the registry; `opencode-ai` is V1, `latest` 1.19.6). `@opencode/cli@2.3.2` (maintainer `thdxr`, published 2026-11-18T01:30Z by GitHub Actions trusted publisher, repository `anomalyco/opencode`) has `bin: opencode -> bin/opencode.exe` and optional platform packages. Installed nothing globally: `npm pack @opencode/cli@2.3.2 @opencode/cli-linux-x64@2.3.2` in `$L/ocbin/dl/` with a scratch npm cache, unpacked the tarballs, ran `$L/ocbin/plat/package/bin/opencode`. The platform tarball sha512 matched the registry `dist.integrity` (`sha512-E9txUc4f...fJgw==`). The postinstall script (read, not run) only copies the platform binary into place. `opencode --version` prints `opencode v2.3.2`. Registry pages: https://registry.npmjs.org/@opencode%2Fcli/2.3.2 and `.../@opencode%2Fcli-linux-x64/2.3.2`.

Plugin files (`$L/mkpl.sh`):

```ts
// .opencode/plugins/probe/index.ts
export { default } from "./probe"
```
```ts
// .opencode/plugins/probe/probe.ts
import { Plugin } from "@opencode/plugin"
import { writeFileSync } from "node:fs"
import { helperValue } from "./helper"
export default Plugin.define({
  id: "probe-id",
  async setup(ctx) {
    writeFileSync("<scratch>/marker-oc-probe.txt", "loaded " + helperValue + " " + new Date().toISOString())
  },
})
```
```ts
// .opencode/plugins/probe/helper.ts
export const helperValue = "helper-ok"
```

Command: `cd $L/projOC && opencode run --standalone --print-logs --log-level debug -m none/none x < /dev/null`. Result: marker `loaded helper-ok 2026-11-18T18:26:19.850Z` (the helper import resolved), log line `msg="loading plugin" id=<abs>/.opencode/plugins/probe entrypoint=file://<abs>/.opencode/plugins/probe/index.ts`, no `@opencode/plugin` install was needed (the host supplies it). Observed.

Variants, each its own project and run:

| Case | Layout | Marker | Log (visible only with `--print-logs`) |
|---|---|---|---|
| T1 | plus top-level `.opencode/plugins/util.ts` with only a named export | `probe` ok | `loading plugin id=.../util.ts`, then `level=WARN message="failed to load plugin" target=.../util.ts ... PluginModule.LoadError: Plugin must export a default definition with an id and an effect or setup function. (cause: SchemaError(Missing key at ["default"]))`. A top-level helper file is loaded as its own plugin and rejected; siblings still load |
| T2 | plus subfolder `plugins/lib/a.ts`, no `index`/`server`, no `package.json` | `probe` ok | nothing: a folder with no resolvable entry is skipped silently |
| T3 | only `.opencode/plugin/probe/` (singular) | ok | loaded |
| T4 | two subfolders, same `id: "probe-id"` | first by scan order only (`probe`; `probeb` absent) | both `loading plugin` lines, no warning, no error, no `Duplicate` text anywhere in stderr or `$XDG_DATA_HOME/opencode/log/opencode.log` |
| T4b | same id, one in `plugin/`, one in `plugins/` | the `plugin/` one only (`probeb`; `probe` absent) | both directories are scanned, `plugin/` first; the second plugin is silently dropped |
| T5a | `index.ts` imports `./nope` | other plugin ok | `level=WARN message="failed to load plugin" target=.../bad ... Cannot find module './nope' imported from .../bad/index.ts` |
| T5b | `setup` throws | other plugin ok | `level=WARN message="failed to load plugin" plugin.id=thr-id cause="Cause([Die(Error: boom)])"` |
| T6 | nested `plugins/group/probe/index.ts` | none | nothing: only direct children of `plugin(s)/` are targets |
| T7 | folder with `probe.ts` and `helper.ts` but no `index.ts` | none | nothing: skipped silently |

Without `--print-logs` a failed plugin prints nothing on stdout or stderr (T5a run: only the unrelated model error); the `failed to load plugin` line goes to `$XDG_DATA_HOME/opencode/log/opencode.log`. So a broken generated plugin is invisible to the user. Observed.

Stray-process hazard: `opencode debug config` and `opencode plugin list` started a background `opencode serve --service` that stayed resident, watched the project and config directories and re-ran plugin `setup` on file changes (it wrote a stale marker during later tests). Stopped with `opencode service stop`. Use `run --standalone` for tests; a user's long-lived service reloading on file change is a hot-reload, not a defect. Observed.

Model-call note: the very first probe run omitted `-m none/none` and `opencode run` answered "hi" through the keyless default model `big-pickle` (one anonymous request, prompt text `hi`). All later runs used `-m none/none`.

## 4. OpenCode V2, global scope

| Case | Setup | Result |
|---|---|---|
| G1 | `OPENCODE_CONFIG_DIR=$L/oc`, plugin at `$L/oc/plugins/gprobe/{index,gprobe,helper}.ts`, empty project | `loading plugin id=$L/oc/plugins/gprobe`, marker written. Observed |
| G2 | `OPENCODE_CONFIG_DIR` unset, `XDG_CONFIG_HOME=$L/xdg/config`, plugin at `$L/xdg/config/opencode/plugins/xprobe/` | `loading plugin id=$L/xdg/config/opencode/plugins/xprobe`, marker written. Observed |

`opencode debug paths` confirmed `config` resolves to `OPENCODE_CONFIG_DIR` when set. The real global path is the XDG config dir plus `opencode` (here `$XDG_CONFIG_HOME/opencode`). The home-directory `~/.config/opencode` form with `XDG_CONFIG_HOME` unset was not tested (not needed to prove the mechanism).

## 5. Removal

**Claude.** After `claude plugin install ... --scope project` the machine holds `installed_plugins.json` and a cache entry besides the settings keys; after settings-only registration it holds only `known_marketplaces.json`.

- R1, mod folders deleted, `marketplace.json` and settings kept (project installed via CLI): `claude -p` still ends with `Not logged in`, no stderr; `claude plugin list` shows `probe@proj-mods ... Scope: project, Status: ✘ failed to load, Error: Plugin directory not found at path: <abs>/.claude/mods/probe. Check that the marketplace entry has the correct path.`; `--json` carries the same text in `errors`. Debug: `Plugin loading errors: Plugin directory not found ...`. A warning, visible in `plugin list` and `/plugin` (docs), not in a plain session start. Observed.
- R2, then settings entries and `.claude/mods` deleted, with the CLI-installed record still on the machine: `plugin list` shows `probe@proj-mods ... Status: ✘ disabled`, `--json` `enabled:false`; the cache directory stays (`Keeping .../cache/proj-mods/probe: it still holds ...`); debug WARN `Cache corrupted or missing for marketplace proj-mods, re-fetching from source: ENOENT` and `Failed to check for delisted plugins in "proj-mods": ... Marketplace file not found`. `plugin marketplace list` still lists `proj-mods` with the dead folder path. A stale record. Observed.
- R3, settings-only registration (no `plugin install`), then mod, marketplace and settings entries deleted: session start is clean (no stderr), `plugin list` prints `No plugins installed`, `--json` `[]`; the same two debug WARN lines appear and `known_marketplaces.json` keeps `proj-mods` (`claude plugin marketplace list` still prints it). A stale cache entry only. Observed.
- Clean removal on the machine needs `claude plugin uninstall probe@proj-mods --scope project` and `claude plugin marketplace remove proj-mods`; I used the latter between cases (it succeeded silently). Not run as a removal test beyond that.

**OpenCode.** Deleting `.opencode/plugins/probe/` and `$OPENCODE_CONFIG_DIR/plugins/gprobe/` (Q5a before: both markers written; Q5b after): no `loading plugin` line, no warning, no marker, nothing mentioning the old plugin in the run's stderr (0 matches). Only the unrelated model error remains. `$XDG_CACHE_HOME/opencode` holds only `bin/`; `$XDG_DATA_HOME/opencode` holds `log/`, `opencode.db`, `repos`, `shell`, `snapshot` and no per-plugin cache for local plugins. Clean. Observed.

## 6. Not observed, and why

- Interactive trust dialog and any consent text for a project marketplace: needs a TTY session and a trust answer; the task forbade answering trust prompts. Docs only.
- Whether the Claude first-session miss with a relative path also happens in an interactive session (the reconcile may trigger a reload there): `-p` only.
- Claude hooks that need a model turn or UI (`tool.call`, `ui.render`, commands): need a login.
- Older Claude versions (2.1.287 to 2.1.322): only 2.1.327 available.
- OpenCode V1 and Kilo: not in scope of this run.
- OpenCode managed policy `plugin.use` deny: not tested.
- Windows path forms and symlinked `.claude` directories: not tested.

## Recommendation

1. Claude project location: `.claude/mods/<name>/` is a working home outside the Claude skills directory. Generate a per-project marketplace at `.claude/mods/.claude-plugin/marketplace.json` listing each mod by `./<name>`, and register it in `.claude/settings.json` (shared) or `.claude/settings.local.json` (per clone) with both `extraKnownMarketplaces` and `enabledPlugins`. This is the only documented route because the `@skills-dir` route puts the mod under `.claude/skills/` and the scanned-project-plugins route does not exist.
2. Prefer the relative `path` `./.claude/mods` in a committed file: it is clone-portable and, observed, loads from the second session; or let grim write the absolute path into `settings.local.json` for a first-session load. The CLI route (`plugin marketplace add`, `plugin install --scope`) always writes absolute paths, so it suits the local file, not a committed one.
3. Module placement is unconstrained inside the plugin folder; keep the author's entry where the author put it and point `hooks.json` `modules` at it with `../<path>`. Observed for `hooks/`, plugin root and a subfolder with sibling imports.
4. OpenCode project location: `.opencode/plugins/<name>/index.ts` re-exporting the author's file, helpers inside `<name>/`. Never put loose `.ts` files directly in `plugins/` (each is loaded as a plugin and warns). Make the plugin `id` unique per mod: a duplicate id is silently dropped, first by scan order. Surface load failures in `grim status` by reading `$XDG_DATA_HOME/opencode/log/opencode.log`, because OpenCode prints nothing to the user.
5. Do not rely on the clients to report removal: Claude keeps `known_marketplaces.json` (and, after a CLI install, `installed_plugins.json` plus a cache copy) and warns in `plugin list` when a folder vanishes; OpenCode is clean.

## Working generated-file shapes (verbatim from what loaded)

Claude, `.claude/mods/.claude-plugin/marketplace.json`:

```json
{"name":"proj-mods","owner":{"name":"grim"},"plugins":[{"name":"probe","source":"./probe"}]}
```

Claude, `.claude/mods/probe/.claude-plugin/plugin.json`:

```json
{"name":"probe","version":"0.1.0","description":"probe"}
```

Claude, `.claude/mods/probe/hooks/hooks.json` (author module at `src/main.ts`; use `./register.ts` for a module in `hooks/`, `../register.ts` for plugin root):

```json
{"modules":["../src/main.ts"]}
```

Claude, `.claude/settings.json` or `.claude/settings.local.json` (relative form loads from the second session; swap in an absolute path for a first-session load):

```json
{"extraKnownMarketplaces":{"proj-mods":{"source":{"source":"directory","path":"./.claude/mods"}}},"enabledPlugins":{"probe@proj-mods":true}}
```

Claude, author module that loaded (`src/main.ts`, helper `src/helper.ts` imported as `./helper.js`):

```ts
import { mark } from "./helper.js"
export function register(on: any) {
  on("session.start", async ($: any, e: any, next: any) => {
    await mark($, "claude-subfolder")
    return next(e)
  })
}
```

OpenCode, `.opencode/plugins/probe/index.ts` (generated, one line):

```ts
export { default } from "./probe"
```

The author file `probe.ts` and helper `helper.ts` are the ones shown in section 3. Global scope uses the same folder under `$OPENCODE_CONFIG_DIR/plugins/<name>/` or `$XDG_CONFIG_HOME/opencode/plugins/<name>/`.

## negative:

- No settings file that grim could own alone for the Claude keys: only the two shared project files (documented absence, not a tested absence).
- No duplicate-id warning anywhere in OpenCode 2.3.2, at debug log level.
- No `@opencode/plugin` install needed for a local plugin; `opencode` unscoped is not an npm package.
- `${CLAUDE_PROJECT_DIR}` is not expanded in a marketplace `path`.

## leads:

- Interactive pty run in a throwaway directory to capture the trust dialog text and whether `/reload-plugins` picks up a first-session relative registration.
- A Claude login (throwaway account, isolated config dir) to observe `tool.call` and `ui.render` hooks from a project-scope mod.
- Older `claude` builds (2.1.287, 2.1.299, 2.1.322) for the same matrix.
- OpenCode managed policy and `OPENCODE_PURE`-style switches in V2.

## Sources

- https://code.claude.com/docs/en/settings-reference.md (`extraKnownMarketplaces`, `enabledPlugins`, marketplace source types; fetched 2026-11-18)
- https://code.claude.com/docs/en/plugins/loading.md (project-scope trust, "Enabled in project settings but not installed"; 2026-11-18)
- https://code.claude.com/docs/en/plugins/marketplace-reference.md (relative path sources; 2026-11-18)
- https://code.claude.com/docs/en/permissions.md (`hasTrustDialogAccepted`, what runs before trust; 2026-11-18)
- https://registry.npmjs.org/@opencode%2Fcli, `.../@opencode%2Fcli-linux-x64` (version, integrity, publisher; 2026-11-18)
- Local runs: `claude 2.1.327` (`plugin validate|list|install|marketplace add|remove`, `-p --debug-file`), `opencode v2.3.2` (`run --standalone`, `debug paths`, `service stop`)

## Scratch artifacts

Under `/tmp/claude-1000/-home-mherwig--herdr-worktrees-grimoire-duo/6dfdf366-5cef-4de7-a681-0f9aeeb626a4/scratchpad/loadtest/`: `env.sh`, `run.sh`, `ocrun.sh`, `mkpl.sh`; projects `projA` (Claude), `projOC`, `pT1` to `pT7`, `pT4b`, `pG` (OpenCode); config dirs `cfg-*`, `oc`, `xdg`, `home`; logs `dbg-*.log`, `ocrun-*.log`; `docs/` (fetched markdown); `ocbin/` (478 MB: `dl/` two npm tarballs, `pkg/`, `plat/`, `npmcache/`). `/tmp/opencode` (empty directory created by OpenCode, outside scratch).
