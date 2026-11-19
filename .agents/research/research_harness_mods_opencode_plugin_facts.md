# Research: OpenCode and Kilo plugin facts for an installing tool

## Metadata

**Date:** 2026-11-18 (every page and repo below was fetched on this date)
**Domain:** packaging
**Triggered by:** Discussion on shipping harness extension code (OpenCode plugins) through grimoire. Open design options per question; this file records facts and evidence only.
**Expires:** 2027-02-18 (OpenCode V2 is under three months old and its plugin API is still moving)
**Lane:** primary-doc and source research. Findings, `negative:` and `leads:` only. No recommendation.

Marks: "verified" = read in this run from the cited page or source file. "Unverified" = not confirmed.

Sources cloned on 2026-11-18 (shallow):
- OpenCode `dev` (V2, version 2.3.0), commit `73215c55ebba6ce79c4e8597e047eb024ea64bb5`, https://github.com/anomalyco/opencode. Paths below prefixed `V2:`.
- OpenCode `v1` branch (version 1.19.6), commit `c618499abc63a8f3006ab640c698e45b1e77ccf8`, same repo. Paths prefixed `V1:`.
- Kilo `main` (`@kilocode/cli` 7.8.30), commit `956f3146050c68d82d11378442fef240703164ae`, https://github.com/Kilo-Org/kilocode. Paths prefixed `Kilo:`.
- Docs pages: https://opencode.ai/docs/plugins/ and https://kilo.ai/docs/automate/extending/plugins (WebFetch, 2026-11-18); V2 docs sources `services/site/src/docs/content/*.mdx` and V1 docs `packages/web/src/content/docs/plugins.mdx` read from the clones.
- npm registry JSON, 2026-11-18.
- No `opencode` or `kilo` binary on PATH; nothing was executed.

## Version context (needed for every answer below)

Two incompatible OpenCode lines are live on 2026-11-18.
- V1: npm `opencode-ai` dist-tag `latest` is 1.19.6 (published 2026-11-17), and `@opencode-ai/plugin` `latest` is 1.19.6 (npm registry).
- V2: `@opencode/plugin` `latest` is 2.3.2 (2026-11-18T01:23Z) (npm registry). V2 source is the `dev` branch, `packages/cli/package.json` version 2.3.0.
- V2 migration page: "OpenCode 1 and OpenCode 2 both use the `opencode` command and are no longer installed side by side by default." and "V1 plugin implementations do not run in V2." (`V2: services/site/src/docs/content/migrate-v1.mdx`, `.../build/plugins/migrate-v1.mdx`).
- Intentional breaks, per `migrate-v1.mdx`: "V2 has three intentional breaking changes: Plugins use a new plugin API; The server API and clients have new contracts; Terminal client configuration moves from layered `tui.json(c)` files to one global `cli.json` file".
- The public docs site https://opencode.ai/docs/plugins/ (fetched) already describes the V2 shape (`plugins` key, `.opencode/plugins/`, `./gui-experimental`).

So an installer in 2026-11 faces both a V1 target (single files, named-function exports) and a V2 target (directory entries, `Plugin.define` default export).

## Q1. Local plugin discovery

### V2 (verified, source)

Directory name: both singular and plural. `V2: packages/core/src/plugin/source-directory.ts:7`: `export const names = ["plugin", "plugins"] as const`.

Scan rule, `V2: packages/core/src/plugin/source-directory.ts:13-31`. For each of `plugin/` and `plugins/` it lists the direct children (one level), sorted by path. A child is a plugin target when it is: a file ending `.ts` or `.js` (line 23-24); any directory (line 25); or a symlink resolving to a `.ts`/`.js` file or to a directory (lines 26-28). Everything else (for example `.mjs`, `.tsx`, `.json`, `.md` files at the top level) is dropped. Subdirectories are not walked: a subdirectory is one plugin target, not a container of targets.

A directory target is then resolved to an entry, `V2: packages/core/src/config/plugin/source.ts:166-177`: `Host.resolve({ directory })`, `const entrypoint = entrypoints.server ?? entrypoints.gui; if (!entrypoint) return []`. A directory with no resolvable server or GUI entry yields nothing and is silently skipped (a TUI-only directory is also skipped here; "A TUI-only directory stays the TUI's own", line 170). It also requires the resolved entry to stay inside the directory (lines 173-176, `FSUtil.contains`).

Entry resolution for a directory, `V2: packages/plugin/src/host.ts:25-64`, and docs `V2: services/site/src/docs/content/build/plugins/index.mdx` ("Load"): "A local plugin directory with a `package.json` `name` and `exports` loads its parts through them, like the published package. Without them, it uses file names at its root: `server` (else `index`), `tui`, and `rpc` with a `.ts`, `.tsx`, or `.js` extension, and `gui-experimental.cjs` (or `.js`)." Source: `server: entry(["server", ""])`, where the empty subpath resolves `path.resolve(directory, "index")`.

Helper files, consequence of the above (derived, not stated in docs): a helper imported by the entry is safe when it lives inside the plugin's own subdirectory under any name other than `server`/`index`, since only the directory is a target. A helper file placed directly in `plugin(s)/` as `.ts`/`.js` IS a target and is loaded as a plugin (and fails with "Plugin must export a default definition with an id..." if it lacks the default export; see Q2). A helper in a subdirectory with no `index`/`server` file and no `package.json` `exports` resolves to nothing and is skipped silently (source.ts:172). The V2 migration page says "Keep supporting modules beside the plugin when moving it." (`build/plugins/migrate-v1.mdx`).

Locations, `V2: packages/core/src/config/discovery.ts:60-160` and `packages/core/src/config.ts:270-276` (`loadDirectory` adds a `Directory` entry for each config directory):
- Global: `global.config`, which is `process.env.OPENCODE_CONFIG_DIR ?? Path.config` (`V2: packages/util/src/global.ts:79`). `Path.config` is the XDG config root plus `opencode` (docs: "`~/.config/opencode/plugins/`", https://opencode.ai/docs/plugins/). XDG_CONFIG_HOME handling inside `roots()` was not opened (unverified detail; V1 uses `xdg-basedir` `xdgConfig`, `V1: packages/core/src/global.ts:3,13`).
- System (managed) directory is also a config directory (`discovery.ts:67-69`); path per platform not opened.
- Project: every `.opencode` directory found walking up from the working directory (`fs.up({ targets: ["."], start: location.directory })`, then `.opencode` under each; `discovery.ts:78-110,140-146`). Docs: "OpenCode also loads direct `.ts` and `.js` files and immediate plugin package directories from every discovered `.opencode/plugins/` directory." and "A `plugins/` directory beside a project-root `opencode.json(c)` is not discovered automatically; configure its plugin directories explicitly or move it under `.opencode/`." (https://opencode.ai/docs/plugins/).
- Whether `.claude/` or `.agents/` directories are scanned for plugins: they are discovered for other kinds (`discovery.ts:114`) but whether `plugins/` under them loads was not verified.

### V1 (verified, source)

`V1: packages/opencode/src/config/plugin.ts:21`: `Glob.scan("{plugin,plugins}/*.{ts,js}", { cwd: dir, absolute: true, dot: true, symlink: true })`. Top-level `.ts`/`.js` files only, both directory names, no subdirectory entries. Config directories: `V1: packages/opencode/src/config/paths.ts:23-41`: `Global.Path.config`, `.opencode` dirs walked up from the project (to the worktree), `.opencode` in the home directory, and `OPENCODE_CONFIG_DIR`. `V1: packages/core/src/global.ts:64`: `config: Flag.OPENCODE_CONFIG_DIR ?? Path.config`. V1 docs (`V1: packages/web/src/content/docs/plugins.mdx:22-25`): "`.opencode/plugins/` - Project-level plugins", "`~/.config/opencode/plugins/` - Global plugins", "Files in these directories are automatically loaded at startup."

### Option evidence

(a) One file per extension directly in `plugin(s)/`:
- For: the only shape that both V1 (`*.{ts,js}` glob) and V2 (file target) load. Works with no config edit.
- Against: every top-level `.ts`/`.js` is a plugin target in both lines, so a multi-file extension cannot put helpers beside the entry in the same directory in V1 (the glob would load them as plugins) or V2 (line 23-24). V1 loads each top-level file and tries every export (Q2), so a helper with named exports fails or is mis-run.

(b) Subdirectory per extension with an entry file:
- For (V2): supported and documented as "immediate plugin package directories", entry `index` or `server` (or `package.json` `exports`). Helpers inside the directory are not loaded separately.
- Against (V1): not discovered; the glob is one level (`plugin/*.ts`), so `plugin/foo/index.ts` is not matched. V2 is required for this shape, or the config entry in (c).

(c) Directory elsewhere plus a config entry:
- V2: `plugins` array entries such as `"./plugins/local"`, `"/absolute/path/plugin"`, `"file:///..."`; "Relative paths resolve from the config file containing the entry." and "Configured local paths must be directories. A path to a single file is skipped." (https://opencode.ai/docs/plugins/; `V2: source.ts:140-160`, file check at 154-157 logs "configured plugin path must be a directory"). Plugin arrays "from applicable config files are applied from lowest to highest precedence instead of replacing one another".
- V1: key is `plugin` (singular), entries are strings or `[spec, options]` tuples (`V1: packages/opencode/src/config/plugin.ts:32-60`); path specs resolve relative to the declaring config file; a directory target needs a `package.json` or index file (`V1: packages/opencode/src/plugin/shared.ts:~185-190`: "Plugin directory ... is missing package.json or index file"). V2 also still reads legacy `plugin` and normalizes it into `plugins` (`V2: packages/core/src/config/normalize.ts:185-190`).
- Cost: requires editing `opencode.json(c)`, which is a shared user file (JSONC; the CLI itself edits it with `jsonc-parser` `modify`, `V2: packages/cli/src/commands/handlers/plugin/add.ts:62-68`).

## Q2. Entry contract and API versions

### V2 (verified, source)

Contract, `V2: packages/core/src/plugin/module.ts:80-93`: the module must have a `default` export that is an object with a string `id` and either an `effect` function or a `setup` function. Failure message: "Plugin must export a default definition with an id and an effect or setup function." (`module.ts:~316`). Authoring form, `V2: services/site/src/docs/content/build/plugins/index.mdx`:

```ts
import { Plugin } from "@opencode/plugin"
export default Plugin.define({ id: "example", async setup(ctx) { ... } })
```

Package for types and runtime: `@opencode/plugin` (npm, latest 2.3.2; `V2: packages/plugin/package.json` name `@opencode/plugin`). Subpaths: `/effect`, `/tui`, `/gui`, `/rpc`, `/host`. README `V2: packages/plugin/README.md`: when the CLI loads plugins, it "resolves imports of `effect`, exported `effect/*` subpaths, and `@opencode/plugin` entrypoints (including imports from dependencies inside a plugin's `node_modules`) to the host's runtime module instances". The docs migration page: "Every V2 plugin needs a stable `id`. Plugin storage is scoped by this ID". A duplicate id is fatal to activation: `V2: packages/core/src/plugin.ts` `activate` does `yield* Effect.die(new Error(\`Duplicate plugin ID: ${definition.id}\`))` (read at ~line 198-200 of the activate function; line number approximate).

Manifest and version gate in V2: none found for local plugins. The docs only say "Use versions compatible with the OpenCode release you target" and "Published plugins should depend on a version of `@opencode/plugin` compatible with the OpenCode release they target." (`build/plugins/index.mdx`, `build/plugins/migrate-v1.mdx`). No `engines` check appears in `V2: packages/core/src/plugin/module.ts` (searched for `engines`; none). The `package.json` fields read from a local directory are only `name` and `exports` (`V2: packages/plugin/src/host.ts:81-84`).

Policy gate that does exist: managed policy action `plugin.use`, resource "A plugin by unversioned package name or absolute path" (`V2: services/site/src/docs/content/policies.mdx:201`); `module.ts:44-47` returns `{ blocked: true }` on deny. See Q4.

### V1 (verified, source)

Two accepted shapes, `V1: packages/opencode/src/plugin/index.ts:92-112` and `shared.ts:272-305`:
- Legacy: "for (const entry of Object.values(mod))" each export (named or default) that is a function, or an object with a `server` function, is run as a plugin; a non-function export throws `TypeError("Plugin export is not a function")` (index.ts:103-111). Docs form: `export const ExamplePlugin: Plugin = async ({ directory, project, client }) => { return { ...hooks } }` with `import type { Plugin } from "@opencode-ai/plugin"` (`V2: migrate-v1.mdx` "V1" snippet).
- V1 object form: default export `{ id?, server() | tui() }` (`readV1Plugin`, shared.ts:272-305; error "must default export an object with server()").

Package: `@opencode-ai/plugin` (npm `latest` 1.19.6). Version gate in V1: `engines.opencode` semver range, enforced only for npm-sourced plugins; `V1: packages/opencode/src/plugin/shared.ts:195-206` throws "Plugin requires opencode ${range} but running ${version}", called from `loader.ts:125-127` inside `if (base.source === "npm")`; a comment at `loader.ts:126` treats local code as development code and skips the gate (comment text partly read: "as local development code and skip this compatibility gate"). So local file plugins have no manifest or gate in V1 either.

### V1 vs V2 break

- What changed (docs `build/plugins/migrate-v1.mdx`): V1 plugin function returns one object of hooks keyed by string; V2 default-exports `Plugin.define({ id, setup(ctx) })` and registers hooks per domain (`ctx.tool.hook("execute.before", ...)`, `ctx.shell.hook(...)`, `ctx.<domain>.transform(...)`). Config key `plugin` becomes `plugins`; tuple becomes `{ package, options }`. Context: V1 `{ directory, project, client }` becomes `ctx.location`, `ctx.options`, domain methods.
- When: V2 npm packages exist from 2026-09 (`@opencode/plugin` placeholder 2026-09-02, beta 2026-09-12, `latest` 2.3.2 on 2026-11-18, npm registry). Exact V2 GA date not found (unverified).
- Both loaded? Same binary does not load both: "V1 plugin implementations do not run in V2." In V2 source, a module without a default `{ id, setup|effect }` fails with the LoadError above (`module.ts:80-93,316`). V2 does still read the V1 config keys and the singular `plugin/` directory (`source-directory.ts:7`, `normalize.ts:185`) but not V1 code. Which of V1 or V2 a user runs depends on how they installed the `opencode` command; V1 remains npm `latest` for `opencode-ai`.
- Failure behaviour: a plugin that fails to load logs `failed to load plugin` and the rest continue (`V2: packages/core/src/plugin.ts:96-99`).

### Option evidence

Single-payload for both lines: impossible by contract. V1 accepts a module whose exports are functions; V2 requires a default object with `id` and `setup`. A module exporting only a default `{ id, setup }` object fails V1's legacy path (non-function default throws "Plugin export is not a function" unless it has a `server` function; `{ id, server() }` is accepted by V1 `readV1Plugin`), and V2 requires `setup` or `effect`. A single file exporting both shapes was not tested (unverified). Dual shape would need two payload files in two directory shapes (single file for V1, directory for V2), or one dir whose layout V1 never reads.

## Q3. Dependencies

### V1 (verified, source and docs)

Docs `V1: packages/web/src/content/docs/plugins.mdx:50,76,86`: "**Local plugins** are loaded directly from the plugin directory. To use external packages, you must create a `package.json` within your config directory"; "OpenCode runs `bun install` at startup to install these." Source `V1: packages/opencode/src/config/config.ts:~447-470`: for each config directory it forks `npmSvc.install(dir, { add: [{ name: "@opencode-ai/plugin", version }] })` (failure only logs "background dependency install failed"). `V1: packages/core/src/npm.ts:88-110,147-195`: install uses Arborist `reify` with `ignoreScripts: true`, `save: true`, `saveType: "prod"`; it runs when `node_modules` is missing, or when `package.json` declares a dependency not in `package-lock.json`. Consequence: V1 itself writes `@opencode-ai/plugin` into the config directory `package.json` and creates `node_modules`; `ensureGitignore(dir)` adds `node_modules`, `package.json`, `package-lock.json`, `bun.lock` to `.gitignore` (`config.ts:~317`). Note the docs say `bun install`; the source in the V1 branch uses Arborist (docs and source differ; source read).

### V2 (verified, source)

No install for local plugins. `V2: packages/core/src/plugin/module.ts:100-110`: `const installed = local ? undefined : ... npm.add(operation.target)`; `Npm` is used only for non-local (package) specs. The V2 docs for local plugins say nothing about `package.json` or `bun install` (grep of `plugins.mdx`, `build/plugins/*.mdx`, `troubleshooting.mdx` for `node_modules`/`dependenc` matched only the Reload note). Local imports resolve by normal module resolution from the plugin directory (`resolveModule(specifier, target.directory)`, `host.ts:37`). Docs note: "Changes to unwatched local dependencies may still require restarting OpenCode." (https://opencode.ai/docs/plugins/). The docs page https://opencode.ai/docs/plugins/ "doesn't describe" npm dependencies for local plugins (WebFetch). A directory plugin can carry its own `package.json` and `node_modules` (a local directory with `package.json` `name` + `exports` resolves through them, `host.ts:28-30`); whether anything installs that `node_modules` was not found (no installer for local directories).

Missing import: the import throws during `Host.load` / `sources.read`; V2 logs `failed to load plugin` with the cause and skips that plugin (`plugin.ts:96-99`). An import failure is cached per entrypoint until its files change (`packages/plugin/src/source.ts:35-37`, "Cache the attempt before evaluating it: unchanged failing modules must not repeat import-time effects").

Runtime-provided modules (V2): `effect`, `effect/*` public subpaths, and `@opencode/plugin` resolve to the host's copies even from a plugin's `node_modules` (README above), so a self-contained plugin needs no install for these.

### Option evidence

(a) Self-contained files only: works on V2 with no installer step; on V1 works only if the file has no third-party runtime imports (type-only imports of `@opencode-ai/plugin` are erased; a runtime import such as the `tool` helper needs the package present, which V1 installs into the config directory in the background).
(b) Tool writes or merges a `package.json`: on V1 the host already owns `package.json` in the config directory (writes `@opencode-ai/plugin`, `ignoreScripts`, `save: true`); a merge would collide with host edits and with user content. On V2 nothing consumes a config-directory `package.json` for local plugins (not found), so the file would be inert unless the plugin directory carries its own installed `node_modules`.

## Q4. Trust and enabling

Verified negatives:
- OpenCode V1 and V2: no trust prompt or allowlist for project-local plugins found. V2 docs page https://opencode.ai/docs/plugins/ "doesn't describe a trust model" (WebFetch result). `grep -rniE "trust"` over `V1: packages/opencode/src/plugin` and `src/config` matched only an unrelated xAI JWT comment (`xai.ts:56`). A project `.opencode/plugins/*` is loaded automatically on start (V1 docs: "Files in these directories are automatically loaded at startup."; V2 docs: "Plugins under `.opencode/plugins/` load automatically").
- `V1: SECURITY.md` "Out of Scope": "**Malicious config files** | Users control their own config; modifying it is not an attack vector". Also "OpenCode does **not** sandbox the agent."

Switches that exist:
- Per-plugin disable (V2): "Prefix an ID or wildcard with `-` to disable it, use `*` for every plugin" (`plugins` array, https://opencode.ai/docs/plugins/). Disabling by wildcard then re-enabling by ID is supported ("A later ID re-enables a plugin.").
- Managed policy (V2): `plugin.use` action with `deny` effect; "a denied plugin is not loaded" (`policies.mdx:195-213`); `"resource": "*"` denies all (`policies.mdx:238`); requires OpenCode 2.2.3 or newer (`policies.mdx:247`). Two built-in policy plugins cannot be removed by a repo (https://opencode.ai/docs/plugins/).
- V1 flags: `OPENCODE_DISABLE_PROJECT_CONFIG` (`V1: packages/core/src/flag/flag.ts:54`; project `.opencode` dirs are skipped in `paths.ts:25-33`) and `OPENCODE_PURE` (flag.ts:66; TUI runtime skips external plugin origins, `V1: packages/opencode/src/plugin/tui/runtime.ts:1089`; the server-side effect of PURE was not traced: unverified). Neither found in V2 `packages/core` or `packages/util` source (grep returned only `OPENCODE_CONFIG_DIR`); V2 equivalents unverified.

Kilo adds a trust concept for config substitution only: `Kilo: packages/opencode/src/config/config.ts:829-835` computes `dirTrusted = dir === Flag.KILO_CONFIG_DIR || dirScope === "global"`; project config directories are "untrusted" so `{file:}`/`{env:}` substitution is confined. This does not gate file-plugin loading in the code read (the plugin glob at `Kilo: packages/opencode/src/config/plugin.ts:21` runs for each directory; whether untrusted directories' plugins are filtered later was not traced, unverified).

## Q5. Removal and updates

V2 (verified): plugin directories are watched; they are not startup-only. Docs: "Changes under watched config directories reload automatically. Server startup loads cached package plugins immediately, installs missing packages in the background, and checks unpinned npm and Git plugins for updates without changing the installed package." and "Changes to unwatched local dependencies may still require restarting OpenCode." with a manual path `touch .opencode/plugins/concise/index.ts` and `opencode service restart` (https://opencode.ai/docs/plugins/). Source: `V2: packages/core/src/config/plugin/source.ts:50-60` comment "The config change feed already covers {plugin,plugins} directories", and `V2: packages/plugin/src/source.ts` (`createPluginSources`) keeps a per-entrypoint digest map of files and re-reads when any digest differs; `dispose` clears the map. Removal: a deleted entry is no longer in the operations list on the next scan, and `plugin.ts` closes the plugin scope with cleanup (`failed to clean up plugin` path, ~line 150). That a deleted file takes effect without restart in practice was not run (unverified; no binary).

Cache that can keep things alive: the import cache keyed by entrypoint holds `loaded` (including failed attempts) until a tracked file digest changes (`source.ts:15-17,35-38`). Package (npm/git) plugins have separate caches (`~/.cache/opencode/node_modules/` per V1 docs line 48, quoted: "Packages and their dependencies are cached in `~/.cache/opencode/node_modules/`."); not relevant to files the tool writes.

V1 (partially verified): docs say "Files in these directories are automatically loaded at startup." No directory watcher was found in `V1: packages/opencode/src/plugin/index.ts` (grep for `watch`/`Watcher` returned nothing), so V1 is startup-only on this evidence (absence of a match, not a read of every path). Removing a file takes effect at the next start. `opencode plugin remove` only edits the config for package plugins (docs: "Install, list, check, update, or remove global package plugins with the CLI."); it does not touch files under `plugins/`.

## Q6. Kilo

Facts (verified unless marked):
- Repository: https://github.com/Kilo-Org/kilocode, `main` commit `956f3146...`, packages `@kilocode/cli` and `@kilocode/plugin`, both 7.8.30 (`rc`) / 7.8.27 (`latest`) (npm registry 2026-11-18; `Kilo: packages/plugin/package.json`, `packages/opencode/package.json`). Kilo `main` is built on the OpenCode V1 code (same `plugin/loader.ts`, `shared.ts`, `getLegacyPlugins`).
- Directories: global `~/.config/kilo/plugin/` and project `.kilo/plugin/` or legacy `.kilocode/plugin/`; "a `plugin/` or `plugins/` folder inside any config directory"; "Every `.ts` or `.js` file in those directories" loads at startup (https://kilo.ai/docs/automate/extending/plugins). Source: `Kilo: packages/opencode/src/config/plugin.ts:21` same glob `{plugin,plugins}/*.{ts,js}` as OpenCode V1; `Kilo: packages/opencode/src/config/paths.ts:23-41` targets `[".kilocode", ".kilo"]`, plus `KILO_CONFIG_DIR`; `Kilo: packages/core/src/global.ts:12,24,80` `app = "kilo"`, config `xdgConfig/kilo`, overridden by `KILO_CONFIG_DIR`. Also: `KILO_DISABLE_PROJECT_CONFIG`, `KILO_PURE` flags (`Kilo: packages/core/src/flag/flag.ts:131-144`).
- Reads OpenCode's config directory too? `Kilo: packages/opencode/src/kilocode/config/config.ts:703` has a comment "Global opencode config dir (sibling of the kilo global config dir, e.g. ~/.config/opencode)" and config file lists include `opencode.json(c)`; whether plugin directories under `~/.config/opencode/plugins/` are scanned was not verified.
- Package: `@kilocode/plugin` for types and runtime (docs: "Kilo installs `@kilocode/plugin` so types resolve out of the box"). Source `Kilo: packages/opencode/src/kilocode/config/plugin-deps.ts:6-26`: installs `@kilocode/plugin` into a config directory only when a plugin spec starts with `file://` (`needsLocalPluginDependency`).
- Manifest keys: `engines: { "opencode": "<range>" }` kept, not renamed: docs "Use the `engines` key with `"opencode"`, a version range. If the CLI doesn't satisfy it, 'the plugin is skipped and a warning is surfaced.'"; source `Kilo: packages/opencode/src/plugin/shared.ts:195-204` (same function and message as OpenCode V1). Config key `plugin` (docs). Docs: "Upstream docs (behavior is identical to OpenCode)."
- Dependencies: docs "Add a `package.json` to your config directory" and Kilo "runs `bun install` at startup"; git plugins "must be self-contained"; "Install scripts are disabled for npm plugins." (kilo.ai page).
- Divergence found in loader code: Kilo skips non-function named exports instead of throwing (`Kilo: packages/opencode/src/plugin/index.ts:108-122`, `if (!plugin) continue // kilocode_change`), where OpenCode V1 throws `TypeError("Plugin export is not a function")` (`V1: index.ts:103-111`).
- Kilo V2 work: branches `v2` and `kilo-v2` exist; `v2` carries `@opencode/plugin` 2.2.6 (`raw.githubusercontent.com/Kilo-Org/kilocode/v2/packages/plugin/package.json`), `kilo-v2` carries `@opencode-ai/plugin` 1.18.15; both have `packages/core/src/plugin/source-directory.ts` with `names = ["plugin", "plugins"]`. `@kilocode/cli` has dist-tag `kilo-v2` = `0.0.0-v2-141` published 2026-11-17 (npm registry). Kilo `main` (the `latest` and `rc` tags) is the V1 lineage; the V2 lineage is a pre-release channel on 2026-11-18. Details of its directory names and API were not read (unverified).

Would an OpenCode-API file load unchanged in Kilo?
- Same lineage as OpenCode V1: a file shaped for V1 (`export const X: Plugin = async (...) => ({ ...hooks })`) goes through the same `getLegacyPlugins` (`Object.values(mod)`) path. Type-only imports from `@opencode-ai/plugin` are erased at runtime (Bun). A runtime import from `@opencode-ai/plugin` (for example the `tool` helper) was not found aliased to `@kilocode/plugin` in Kilo source: the only occurrence in `Kilo: packages/opencode/src` is a comment about bridging built-in auth plugin types (`plugin/index.ts:83`), and Kilo installs `@kilocode/plugin`, not `@opencode-ai/plugin`, into config dirs (`plugin-deps.ts:15`). So a runtime import would not resolve unless the user separately has `@opencode-ai/plugin` installed. Not executed (no binary): unverified in practice.
- A V2-shaped file (`@opencode/plugin`, `Plugin.define`) would not load on Kilo `main` (V1 lineage: default export object has no `server` function; `Plugin.define` import from `@opencode/plugin` unresolved).
- Third-party report (from the earlier prior-art file, search-only, secondary): an installer ports OpenCode plugins for Kilo by swapping the package name and paths, https://www.mintlify.com/PeonPing/peon-ping/integrations/kilo-cli (not re-fetched in this run).

Option evidence:
(a) Same payload: supported by identical glob, identical legacy export scanning, identical `engines.opencode` key, and the Kilo docs sentence "behavior is identical to OpenCode"; but directories differ (`.kilo/plugin`, `~/.config/kilo/plugin`), and runtime imports of the OpenCode package name are the observed break point. Only the V1-shape lineage; V2-shape is not read by Kilo `main`.
(b) Own payload: needed if the extension has a runtime import of the plugin package (`@kilocode/plugin` vs `@opencode-ai/plugin`), or targets V2 on OpenCode (Kilo `main` cannot load V2 shape).
(c) Not served: Kilo's own docs describe plugins as supported (kilo.ai page); no evidence it lacks the surface.

## Q7. TUI and UI surfaces

V2 (verified, docs and source):
- Parts of one plugin package: server (`.` / `./server`), TUI (`./tui`), GUI (`./gui-experimental`), RPC (`./rpc`). "Each part is an export of the package." (`build/plugins/index.mdx`). TUI plugin: `import { Plugin } from "@opencode/plugin/tui"; export default Plugin.define({ id, setup(context) { context.ui.toast.show(...) } })` (`build/plugins/cli.mdx`). "TUI plugins extend the terminal UI with commands, routes, slots, Markdown renderers, notifications, and local state."
- Same directory? A local directory plugin resolves `tui` by file name at its root (`tui.ts`/`tui.tsx`/`tui.js`) in the same `.opencode/plugins/<name>/` directory, next to `index.ts`: `cli/plugins.mdx` lists `<global-config>/plugins/status/index.ts`, `<global-config>/plugins/status/tui.ts`, `<project>/.opencode/plugins/status/index.ts`, `<project>/.opencode/plugins/status/tui.ts` (lines 55-58). "The TUI loads the TUI parts of the plugins its server loads." A TUI-only plugin is configured in `cli.json`: `"plugins": ["opencode-acme-cli"]` (https://opencode.ai/docs/plugins/; `cli/plugins.mdx`), and a TUI-only local directory is not listed by the server scanner (`source.ts:170`: "A TUI-only directory stays the TUI's own"). Whether a TUI-only directory under `.opencode/plugins/` is auto-discovered by the CLI without a `cli.json` entry was not verified.
- GUI (desktop) part: "A plugin that exports `./gui-experimental` also adds panels, commands, or settings to OpenCode Desktop."; the Desktop app "loads plugins from the server it runs, so configure the plugin in your global `opencode.json(c)`" (https://opencode.ai/docs/plugins/). Local form: `gui-experimental.cjs` or `.js`, prebuilt CommonJS only: "The app evaluates the bundle as CommonJS text, so a TypeScript source or an ES module is never a GUI entry." (`V2: packages/plugin/src/host.ts:60`).
- Themes: separate declarative JSON files, not plugins. "Themes belong to the terminal client." Global `~/.config/opencode/themes/`, project `.opencode/themes/`; "OpenCode reads `.json` files only. A theme closer to the current directory replaces a global or parent theme with the same name." The filename without `.json` is the theme name; selection is `theme.name` in `cli.json` (`V2: services/site/src/docs/content/themes.mdx`). V2 theme format is new ("Every V2 theme declares one complete `base` token tree and at least one `light` or `dark` hue palette").
- V1 TUI: TUI plugins exist behind a separate `tui.json` `plugin` list (`V1: packages/opencode/src/config/tui.ts:157-162`, `plugin_origins`), V1 object form `{ tui() }`, and npm packages can ship themes through an `oc-themes` field in `package.json` (`V1: packages/opencode/src/plugin/shared.ts:239-257`, relative paths only). Local `.opencode/themes/` for V1 was not read (V1 docs path unverified).

## negative:

- No trust dialog, allowlist or consent step for project-local plugins in OpenCode V1 or V2, or in Kilo plugin loading (none found); project plugin loads on start. Only a managed-policy `plugin.use` deny (V2), per-ID `-` disable (V2), and V1/Kilo `*_DISABLE_PROJECT_CONFIG` / `*_PURE` flags exist.
- No manifest or version gate for local plugins in V2 (no `engines` check) and none for local files in V1/Kilo (the `engines.opencode` check runs for npm-sourced plugins only).
- No automatic dependency install for local plugins in V2 source (Npm service used only for package specs).
- Not found: a single payload that satisfies both V1 and V2 contracts (not tested by execution).
- Not found: an alias from `@opencode-ai/plugin` to `@kilocode/plugin` at Kilo runtime.
- Not run: no `opencode` or `kilo` binary on PATH, so nothing about live behaviour (hot reload on delete, error output) was observed.
- Docs/source mismatch: V1 docs say `bun install`; V1 branch source uses Arborist `reify`.

## leads:

- Run V1 and V2 binaries in a scratch directory to confirm: top-level helper file loading, subdirectory handling, duplicate-id die, delete-while-running behaviour.
- Read V2 `packages/util/src/global.ts` `roots()` for exact XDG_CONFIG_HOME and platform paths, and the system (managed) directory per OS.
- Check whether V2 auto-discovers a TUI-only plugin directory without a `cli.json` entry (`V2: packages/cli`, TUI plugin loader) and the `OPENCODE_PURE` / project-config-disable equivalents in V2.
- Read the Kilo `v2` / `kilo-v2` branches for directory names, config key and package (`@opencode/plugin` 2.2.6 on `v2`); that lineage decides whether Kilo needs a V2 payload.
- Check whether a Kilo untrusted project directory filters file plugins after config load (`Kilo: packages/opencode/src/config/config.ts:829+`).
- Find the V2 GA announcement or changelog entry to date the V1/V2 break (https://github.com/anomalyco/opencode/blob/dev/CHANGELOG.md).
- Check whether Kilo scans `~/.config/opencode/plugins/` (kilocode `config.ts:703` comment).

## Sources

| URL | What | Fetched |
|---|---|---|
| https://opencode.ai/docs/plugins/ | V2-shape plugin docs: locations, control, reload | 2026-11-18 |
| https://github.com/anomalyco/opencode (`dev`, `v1`) | V2 and V1 loader source, docs sources | 2026-11-18 |
| https://kilo.ai/docs/automate/extending/plugins | Kilo plugin docs | 2026-11-18 |
| https://github.com/Kilo-Org/kilocode (`main`, `v2`, `kilo-v2`) | Kilo loader source | 2026-11-18 |
| https://registry.npmjs.org/ (`@opencode/plugin`, `@opencode-ai/plugin`, `opencode-ai`, `@kilocode/plugin`, `@kilocode/cli`) | dist-tags and publish times | 2026-11-18 |
