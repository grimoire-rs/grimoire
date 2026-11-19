# Mod Spec

You loaded this file because you are authoring or fixing a grim **mod**:
a directory holding a `mod.toml` descriptor plus the client-loaded code
(TypeScript or JavaScript) one or more clients run inside their own
process, for `grim build` or `grim release`.

Contents: [Ships Gated](#ships-gated) · [File Shape](#file-shape) ·
[mod.toml](#mod-toml) · [The Tree Rules](#the-tree-rules) ·
[Entries](#entries) · [Which Clients](#which-clients) ·
[What Install Writes](#what-install-writes) · [Publishing](#publishing) ·
[Testing Against a Local Registry](#testing-locally) ·
[Skills Cannot Carry Client Code](#skills-cannot-carry-client-code) ·
[Validation Pitfalls](#validation-pitfalls)

## Ships Gated

A mod is **experimental** and a published mod is not a published *running*
thing:

- It sits behind `options.experimental.mods`, **off by default** and
  config-only (no environment override).
- Enabling the flag is not consent to write your code. At project scope the
  workspace must also have consented, through the one consent record every
  gated kind shares (`grim consent allow`, `grim add`, an accepted prompt, or
  `--trust-workspace`). Global scope is always consented.
- Consent decides what grim *writes*. It never decides what a client then
  loads: a written mod loads under the client's own rules.
- A consumer who adds your mod with the flag off gets a declaration and a
  lock pin, and nothing written.

Write the `description` for someone reading a catalog listing, and never
tell consumers a mod runs on install.

## File Shape

A mod is a **directory** artifact:

```
mods/status-band/
├── mod.toml            # the descriptor, the file that makes it a mod
├── claude/             # code for Claude Code (optional)
│   └── status-band.ts
└── opencode/           # code for OpenCode (optional)
    └── status-band.ts
```

The directory name is the artifact name and `mod.toml`'s `name` must equal
it. Kind inference checks `SKILL.md` first, then `hook.toml`, then
`mod.toml`, so a directory that also carries a `SKILL.md` or `hook.toml`
keeps that kind. Pass `--kind mod` in scripted loops.

## mod.toml

A strict document: an unknown key or table fails with exit 65.

| Key | Required | Notes |
|---|---|---|
| `schema` | yes | `1` today |
| `name` | yes | Must equal the directory stem; the skill-name grammar |
| `description` | yes | Becomes the catalog description; must not be blank |
| `[claude]` | no | Optional `entry`, the entry file relative to `claude/` |
| `[opencode]` | no | Optional `entry`, relative to `opencode/` |
| `[requires.*]` | no | Dependencies, declared as in `hook.toml` |

The file is at most 64 KiB. `grim build` writes the resolved entry of every
served client into the packed copy, so a consumer's grim never guesses one.
There is no `api` or version key for the client.

## The Tree Rules

`grim build`, `grim release` and install apply the same rules:

- Only `claude/` and `opencode/` sit beside `mod.toml`; any other top-level
  path fails the build.
- Every file in a harness directory is a code file ending in `.js`, `.mjs`,
  `.cjs`, `.jsx`, `.ts`, `.mts`, `.cts` or `.tsx`. Data files, JSON and
  images are refused.
- Every path segment matches `[A-Za-z0-9._-]+`, neither starts nor ends with
  a dot, and is not a Windows device name. No dotfiles, so no hidden
  directories.
- No two paths that differ only in case; no symlinks.
- Caps: 16 MiB decoded, 2000 files, 16 path segments below the mod root (the
  harness directory counts as one).

## Entries

An entry is the one file a client loads. Name it in `[claude]` or
`[opencode]`, or omit it and name one top-level file `<name>.<ext>` in the
harness directory (`claude/status-band.ts`). Zero or several matches fail the
build with a message naming the fix.

An OpenCode entry must end in `.ts` or `.js`, and no other file in its
directory may share its stem. A `[claude]` or `[opencode]` table without the
matching directory fails the build.

## Which Clients

Which clients a mod serves is exactly which harness directories it holds;
the packer writes them into the `com.grimoire.mod.clients` annotation
(`claude`, `opencode`, or `claude,opencode`). A client the mod does not serve
gets no warning. Only Claude and OpenCode host a mod today.

Claude reserves some names for plugins (`claude`, `anthropic`, `anthropics`,
`claude-code`, `claude-mods`, and the prefixes `claude-`, `anthropic-`,
`anthropics-`, `cc-plugin-`); a consumer binding a mod to one of them gets no
Claude install and a warning, and every other client still hosts it. OpenCode
hosts a mod only when the consumer's scope sets
`options.vendors.opencode.plugin_api = "v2"`.

## What Install Writes

Tell consumers the paths, not an effect:

| Client | Project scope | Global scope |
|---|---|---|
| Claude | `.claude/mods/<binding>/` (`src/**`, `.claude-plugin/plugin.json`, `hooks/hooks.json`); a shared `.claude/mods/.claude-plugin/marketplace.json`; entries in `.claude/settings.local.json` | the same under the Claude config root's `mods/` |
| OpenCode | `.opencode/plugins/<binding>/` (`src/**` plus a generated `index.ts`) | `plugins/` under the OpenCode config root |

The shared Claude marketplace is named `grim-mods-` plus 12 hex characters,
and grim owns every `extraKnownMarketplaces` key of that prefix in the
settings file. A user turning a mod off by setting its `enabledPlugins` entry
to `false` is respected, and the mod reads `mod-disabled-in-client` in
`grim status`.

Records of what grim wrote live in `$GRIM_HOME/mods/`, never in the
repository. `grim install` converges, and revoking consent does not remove
written files by itself: the next converging command disarms them.

## Publishing

- **A mod has no local-path source.** The config parser rejects a path value
  under `[mods]`, and a dev install does not take mods. The loop is
  `grim build` -> `grim release` (a local registry is fine) -> `grim add`
  from that reference.
- **A mod can be a bundle member**: a `[mods]` table in the bundle `.toml`.
  Members sort kind by kind, skill, rule, agent, mcp, hook, settings, mod, so
  mods come last. Membership declares; it never arms.
- **`publish.toml` has a `[mods.<name>]` table**, its conventional source
  `mods/<name>/`. Batch publish releases mods after `settings` and before
  `bundles`.
- `--compression gzip` applies to the mod layer. Cascade tags, the
  immutability gate and `--git` provenance behave as for every other kind.
- `grim schema --kind mod` prints the JSON Schema of `mod.toml`.

## Testing Against a Local Registry {#testing-locally}

```sh
registry=localhost:5000                       # loopback is plain HTTP by default
grim build ./mods/status-band
grim release ./mods/status-band "$registry/mods/status-band:0.1.0"
cd "$(mktemp -d)" && git init -q
grim config set options.experimental.mods true
grim add "$registry/mods/status-band:0.1.0" --kind mod --trust-workspace
grim status            # expect: the mod reads installed, or gated with a cause
```

Run it in a scratch directory with its own `GRIM_HOME`, never in a checkout
you care about. Read `grim status --format json` and branch on `cause`.

## Skills Cannot Carry Client Code

A skill whose root carries a `.claude-plugin` directory is refused at
`grim build`, `grim publish` and install (exit 65, reason
`skill-plugin-manifest`). Client-loaded code ships as a `mod`. Move the code
into a `claude/` directory of a mod and publish that.

## Validation Pitfalls

- `claude/readme.md` or any non-code file: refused. Keep docs outside the
  mod directory.
- A hidden directory such as `claude/.cache/`: refused (segment starts with a
  dot). List it in `.grimignore` only if it is a runtime byproduct; it is
  never a valid shipped path.
- Two entry candidates in `claude/` (`status-band.ts` and `status-band.tsx`):
  name one in `mod.toml`. Under `opencode/`, rename one instead, since no
  other file may share an OpenCode entry's stem.
- `name` differing from the directory: exit 65.
- Expecting the mod to appear in a client right after `grim add` with the
  flag off: nothing is written until the flag and consent are both met.
