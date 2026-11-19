---
title: Mod reference
description: What a mod is, its layout and mod.toml, the clients that host it, workspace consent, the files grim writes, the lifecycle, and the status causes. Experimental pre-1.0.
---
<!-- doc_type: reference -->

**Experimental pre-1.0**. See [Stability][stability-unstable].

The `mod` kind sits behind the `[options.experimental] mods` flag, which is off
by default. The `[mods]` table, the consent record's path and format, the
installed layout and the generated file bytes may change in any minor release.
The kind token, the layer layout and the report literals are frozen.

A mod is publisher code that a client loads into its own process. A status
band in Claude Code and a plugin in OpenCode are examples. Grim writes the
code and the registration files that make the client load it.

This page covers the layout, `mod.toml`, the clients that host a mod, and
consent. It also covers the files grim writes, the lifecycle, the status causes
and the limits.

## Mod, hook or skill {#when}

Use a skill for instructions the agent reads. Use a [hook][hooks] for a program
the client runs around a tool call. Use a mod when the code must run inside the
client itself.

A skill whose root carries a Claude plugin manifest directory
(`.claude-plugin`) is refused at build, publish and install. Ship client-loaded
code as a mod. See [the promise][stability-skill-plugin-manifest].

Grim can gate what it writes. It cannot gate what a client loads. A mod is
unsandboxed publisher code with the reach of the client, so every mod needs
[workspace consent](#consent).

## Layout {#layout}

A mod is a directory named after the mod. It holds `mod.toml` and one
directory for each harness it serves:

<!-- doc-norun: shows a directory layout, not a command -->
```text
mods/status-band/
  mod.toml
  claude/        the Claude Code mods API
  opencode/      the OpenCode V2 plugin API
```
<!-- /doc-norun -->

A harness directory names one client API line. `opencode/` is the OpenCode V2
plugin API for good. A later incompatible line gets a new directory name.

A harness directory holds code files only: regular files with the extension
`.js`, `.mjs`, `.cjs`, `.jsx`, `.ts`, `.mts`, `.cts` or `.tsx`. A symlink is
refused at build. The layer is capped at 16 MiB decoded, 2,000 files and depth
16, and `mod.toml` at 64 KiB.

## The mod.toml file {#mod-toml}

`mod.toml` holds the descriptor. Save this file as `mods/status-band/mod.toml`:

<!-- doc: mods-reference -->
```toml
schema = 1
name = "status-band"
description = "Build status above the prompt"

[claude]
entry = "status-band.tsx"
```

`schema` must be `1`. `name` follows the skill-name grammar and equals the
directory name. `description` is required.

Each harness table is optional. A table sets `entry`, the code file the client
loads. With no table, or with no `entry`, the entry defaults. It is the one
top-level file in the harness directory whose name, without its code
extension, equals the mod name. Zero or several such files fail the build with exit `65`.

A mod can declare `[requires]`, as a `hook.toml` does. See
[Dependencies][artifacts-requires].

An OpenCode entry must be a `.ts` or `.js` file. No other code file in the entry's
directory may share its name without the extension. Helper files can use any
code extension.

Build and validate the mod. `grim build` infers the `mod` kind from `mod.toml`
after it finds no `SKILL.md` and no `hook.toml`. The command also writes the
entry file the descriptor names:

<!-- doc: mods-reference -->
```bash-run
mkdir -p mods/status-band/claude
printf 'export default {}\n' > mods/status-band/claude/status-band.tsx
grim build mods/status-band
```

The report names the kind `mod`, the name, the layer digest and the status
`built`. `grim build` writes the resolved entry of every served harness into
the packed `mod.toml`, so a second release of the same files gives the same
bytes. The file on disk stays as you wrote it.

`grim schema --kind mod` prints the JSON Schema for `mod.toml`.

### Parse rules {#parse-rules}

The build denies unknown keys and unknown tables. Install reads the packed file
more loosely. It ignores an unknown top-level table as a harness this grim does
not know, and it denies unknown keys inside a known table.

An unknown harness directory is checked for containment, segment names and the
caps. Grim never writes it, and the install warns once.

### What a mod cannot use {#excluded}

The generated Claude plugin manifest carries only a `name` and, when the
author tree holds `types/index.d.ts`, a `types` path. Four manifest features
are therefore unavailable to a mod:

- `userConfig`: the module's `register(on, options)` receives no options.
- `dependencies`.
- `defaultEnabled`.
- `engines`.

## Served clients {#served}

A mod serves the harnesses its layer carries. `grim build` records them, sorted
and comma-joined, in the manifest annotation `com.grimoire.mod.clients`. The
lock row copies them into its `clients` field.

Three conditions let a client host a mod. The client supports the kind at that
scope, the row lists the client, and, for OpenCode, the host declaration says
so. Claude Code
and OpenCode host mods. Kilo declines the kind. A client that cannot host a
mod gets one warning and no files.

`grim describe` and `grim search` report the served set as `serves`. The value
is `null` when the source published no annotation.

### The OpenCode host declaration {#host-declaration}

Grim cannot tell OpenCode V1 from V2 without running the client, which it never
does. OpenCode hosts a mod only when the scope's config says so:

<!-- doc-norun: changes the reader's own grim configuration -->
```bash
grim config set options.vendors.opencode.plugin_api v2
```
<!-- /doc-norun -->

Only `v2` is valid, and only under `opencode`. Any other client refuses the key
when the config loads. Without it, OpenCode declines with one warning that
names the key, and grim writes nothing for it. Grim reads the key from the
resolved scope's own config, with no merge across scopes.

In a project config, the key makes a grim without mod support exit `78`. In the global config, it makes
only `--global` commands of such a grim exit `78`. See [Older grims](#older-grims).

## Turn the kind on {#enable}

Turn the flag on for one project, or add `--global`:

<!-- doc-norun: changes the reader's own grim configuration -->
```bash
grim config set options.experimental.mods true
```
<!-- /doc-norun -->

Then declare the mod in the `[mods]` table, with the entry forms of
[`[hooks]`][artifacts-hooks]:

<!-- doc-norun: shows a grimoire.toml table, and the registry is the reader's own -->
```toml
[mods]
status-band = "ghcr.io/acme/mods/status-band:1.0.0"
```
<!-- /doc-norun -->

A path value is refused when the config parses, because a mod has no local-pack
route. A dev install of a local path writes no mod. To test a mod, release it to a local registry and add it from there.

Declaring and locking need no flag. With the flag off, `grim status` reads
`gated` with the cause `feature-flag-off`, and `grim install` writes and
removes nothing. Clearing the flag disarms every written mod at the next
converging command.

## Consent {#consent}

Grim writes a mod only after you consent to the workspace. Consent decides what
grim writes. It never decides what a client loads.

The record is `$GRIM_HOME/consent/<workspace-key>.json`, one record for every
gated kind. Each entry is `<binding>@<registry>/<repository>` with no kind
prefix. The same binding from the same repository under another gated kind is
not drift. A new binding, a rebinding, or a binding from a new repository is.

The three ways to answer match [hooks][hooks-consent]: `grim consent allow`,
`grim add`, and an accepted prompt. A run with no terminal and no answer leaves
the mod `gated`.

The per-run flags `--trust-workspace` and `--no-trust-workspace` apply to
`grim install`, `grim update` and `grim add`. No environment variable or config
file answers for you. With `--no-trust-workspace`, the mod is disarmed for that
run, and the next consented run writes it again.

Every changing run prints one stderr line for each mod first written under a
consent entry the run's own gesture did not add. The line names the kind and the
source.

A record under `$GRIM_HOME/hooks/consent/` is never honoured. Grim deletes it
at each write and revoke when its `workspace` field names this workspace. Each
linked worktree and each moved checkout consents on its own.

`grim consent list` still lists hooks only.

## Where grim writes {#files}

Each mod becomes one Claude plugin and one OpenCode plugin directory, named
after the binding. Author files land under a grim-owned `src/` directory.

| Scope | Client | Files | Registration |
|---|---|---|---|
| Project | Claude Code | `.claude/mods/<binding>/` | `.claude/settings.local.json` |
| Global | Claude Code | `<claude-root>/mods/<binding>/` | `<claude-root>/settings.json` |
| Project | OpenCode | `.opencode/plugins/<binding>/` | none, loads on presence |
| Global | OpenCode | `<opencode-root>/plugins/<binding>/` | none |

`<claude-root>` follows `CLAUDE_CONFIG_DIR`. `<opencode-root>` is
`OPENCODE_CONFIG_DIR`, else the XDG `opencode` directory.

Claude Code loads a mod through a directory marketplace. Grim writes one shared
`mods/.claude-plugin/marketplace.json` for the scope, and an
`extraKnownMarketplaces` entry plus one `enabledPlugins` entry for each mod. The
marketplace name is `grim-mods-` and twelve hex digits. Project scope derives
them from the workspace key, and global scope from a random id that grim stores
once. The `grim-mods` prefix is reserved: the
[settings kind][settings-reserved] refuses a marketplace name that starts with
it.

Each Claude plugin holds `.claude-plugin/plugin.json` and `hooks/hooks.json`,
which names one module, `../src/<entry>`. Claude loads one module for each
plugin. An OpenCode plugin holds `index.ts`, a one-line re-export of your
entry, and `src/**`.

Global roots pass the same fence as the settings kind. A relative root, or a
root inside the repository the command runs in, is refused with exit `0`, status
`not-armed` and the cause `settings-root-relative` or `settings-root-in-workspace`.

Grim records each written file with a hash in a store under `$GRIM_HOME/mods/`.
A mod record never goes into `.grimoire/state.json` or the global state file.
The shared marketplace file and entry are grim's only while they match the hash
grim wrote. Any other file or entry refuses with `untracked-destination`
(exit `65`, forceable). A name that matches `grim-mods-<12 hex>` but is not the
current marketplace is removed at the next converging command.

Grim writes no `.git/info/exclude` line for a mod path.

Grim never writes Claude's `known_marketplaces.json` or plugin cache. After the
last mod of a scope leaves, `grim uninstall` names
`claude plugin marketplace remove <name>`.

### Two recorded exceptions {#exceptions}

The Claude payload under `.claude/mods/` and the OpenCode project directory
live in the repository. A branch that tracks those paths replaces them on
checkout. The Claude registration then enables what the branch put there.
OpenCode loads `.opencode/plugins/` on start with no prompt.

The lock pins what grim writes, not what the client loads. A committed mod
output runs for every cloner.

## Lifecycle {#lifecycle}

`grim install`, `grim update`, `grim add`, `grim uninstall` and the TUI's
Install, Update and Delete converge mods. Each runs the disarm half over every
mod of the scope. `--client` narrows writes, never the disarm half. A client
that `[options].clients` no longer names is disarmed at the next converging
command. Disarm needs no network and runs when another row failed.

`status`, `lock`, `remove`, `consent allow|revoke|list`, `hook list`, `export`,
`fetch` and the MCP tools never converge mods.

| Event | Files | Claude registration | Status |
|---|---|---|---|
| Consent present, flag on, OpenCode declared V2 | written | written | `installed` |
| Not consented or drifted, no terminal | none for that binding | none | `gated` |
| `--no-trust-workspace` | disarmed for this run | removed | `gated` |
| `grim consent revoke` | unchanged; stderr names each written mod | unchanged | `installed`, then `gated` after the next converging command |
| Next converging command after revoke, drift or flag off | disarmed | removed | `gated`; kept files read `mod-files-present` |
| An edited output at disarm | edited file kept, the rest deleted | removed | `gated` |
| `grim remove mod <name>` | unchanged; stderr names each mod the edit dropped and its `grim uninstall mod` command | unchanged | pruned by the next converging command |
| `grim uninstall mod <name>` | removed now | removed now | absent |

Disarm is removal. No dispatcher stops a loaded mod, so between grim commands
the client decides. An edited OpenCode `index.ts` keeps loading after disarm,
and the warning says so. `grim uninstall --force mod <name>` removes edited
files.

A named `grim update` and `grim add` write only the mods in their closure. A
gated mod outside the closure is disarmed and named on stderr.

### Revoke {#revoke}

`grim consent revoke` deletes the whole consent record. The mod store records
no grant source, so a mod written under the revoked record reads `installed`
until the next converging command disarms it. It then reads `gated`. Revoke
prints each written mod and `grim install`.

A drifted record defers the same way. A mod already written reads `installed`
until the next converging command disarms it. It then reads `gated` with the
cause `consent-drifted`.

Revoke with the grim that granted. An older grim's `grim hook revoke` removes
only the old path, and a newer grim then still arms on the v2 grant.

## Status {#status}

`grim status` reads each mod with the shared tokens: `installed`, `outdated`,
`modified`, `missing`, `stale`, `gated` and `not-armed`. `installed` means
written and registered, not observed running. The row's `armed` field stays
`null`.

| Cause | Row | Meaning |
|---|---|---|
| `feature-flag-off` | `gated` | The `mods` flag is off. |
| `workspace-not-consented` | `gated` | No consent entry covers the binding. |
| `consent-drifted` | `gated` | The binding or its source changed since consent. |
| `mod-files-tracked` | unchanged | Git tracks a file grim wrote, so a branch can replace it. |
| `mod-files-present` | unchanged | Files sit at the expected paths and grim does not own them. |
| `mod-disabled-in-client` | unchanged | The client records the plugin as disabled. `--force` restores it. |

The last three causes are advisory and exit `0`. A written output defers the
two consent causes, so a mod already written reads `installed` until a
converging command decides.

A hand edit of the shared Claude marketplace file or its
`extraKnownMarketplaces` entry moves every installed Claude mod row of the
scope to `modified`. An unparseable settings file reads as drift. A user's
`false` in `enabledPlugins` is not drift.

### When `installed` is not running {#inert}

The client can skip a written mod with no error grim can see. Grim never reads
client state. The causes are:

- Claude managed settings: `allowManagedModsOnly`, a `strictKnownMarketplaces`
  or `blockedMarketplaces` entry naming the directory, or a managed
  `enabledPlugins` value.
- Claude's `disableAllHooks`, the `--safe-mode` flag, or an untrusted folder.
- OpenCode managed `plugin.use` deny, or a `-` entry in its `plugins` array.

`disableSideloadFlags` covers `--plugin-dir` only, not this route.

### Refusals {#refusals}

An install re-checks the fetched layer before any write. A layer that fails is
refused with exit `65` and reason `invalid-mod`, per row. The other rows still
converge.

The error names the rule that failed. The rules cover a path that escapes, a
segment outside `[A-Za-z0-9._-]+`, and a Windows device name. They also cover
a missing or disallowed entry, and a harness set that differs from the
annotation or the lock row. The error document carries neither `retryable` nor
`forceable`.

A binding that fails the skill-name grammar or a device-name check is refused
the same way. Grim declines the Claude client, with a warning, for a binding
Claude's own validator refuses.

## Export {#export}

`grim export plugin` admits a mod for Claude when the row serves `claude`. Other
Claude-family clients decline with `client-declined`, and Agent Plugins has no
format surface for it. A Claude plugin loads one module, so a plugin with two or
more admitted mods carries none of them. Each is omitted as `not-representable`,
and the one-mod-per-plugin fix is named on stderr.

An export that stages a mod refuses an output directory inside a client config
root with exit `64`. Every export of other kinds keeps its behaviour.

The MCP tool `grim_fetch` answers "mods cannot be fetched yet". The tool
`grim_render` answers "mods render no files" and names `grim install`.

## Older grims {#older-grims}

An older grim fails closed on a mod project. The
[forward compatibility table][stability-forward-compat] lists each case, and
[the upgrade notes][upgrading-mods] list what to do. A `plugin_api`
key in a project config makes a grim without mod support exit `78`. In a global config, only `--global`
commands of such a grim exit `78`.

## Load probe {#load-probe}

The CI job `mod-load-probe` in `verify-deep.yml` installs a built mod and
starts the latest Claude Code and OpenCode without a login. A red run signals a
change in a client's loading. The job runs on `workflow_dispatch` only and
gates no merge.

[stability-unstable]: ./stability.md#unstable
[stability-skill-plugin-manifest]: ./stability.md#promise-skill-plugin-manifest
[stability-forward-compat]: ./stability.md#limitations-forward-compat
[upgrading-mods]: ./upgrading.md#upgrade-mods
[hooks]: ./hooks.md
[hooks-consent]: ./hooks.md#consent
[artifacts-hooks]: ./artifacts.md#hooks
[artifacts-requires]: ./artifacts.md#requires
[settings-reserved]: ./settings.md#reserved-marketplace-prefix
