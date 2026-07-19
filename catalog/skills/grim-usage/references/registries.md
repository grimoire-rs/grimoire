# Registries, Scopes, and Targets

You loaded this file because you need to resolve which registry a short
reference hits, which scope a command edits, which AI clients an install
lands in, how offline mode behaves, or how to search a catalog.

Contents: [Registry Resolution](#registry-resolution) ·
[Multiple Registries](#multiple-registries) ·
[Managing Config](#managing-config) ·
[Qualified References](#qualified-references) ·
[Scopes](#scopes) · [Client Targets](#client-targets) ·
[Offline Mode](#offline-mode) · [Search, TUI, and MCP](#search-tui-and-mcp)

## Registry Resolution

A fully qualified reference (`ghcr.io/acme/code-review:1`) needs no
resolution. A short reference (`code-review:1`) is expanded against the
default registry, resolved with this precedence — first present value
wins:

1. `--registry` flag
2. `GRIM_DEFAULT_REGISTRY` environment variable
3. project config `[[registries]]` primary (or legacy `[options].default_registry` when no `[[registries]]` declared)
4. global config `[[registries]]` primary (or legacy `[options].default_registry`)
5. the built-in fallback registry `ghcr.io/grimoire-rs` (applies only
   when nothing above is set; first-party packages live there)

Whatever default applied, the expanded reference is persisted **fully
qualified** in `grimoire.toml` and the lock — so a config never depends
on the environment that wrote it.

`grim login` / `grim logout` resolve their registry from the positional
argument, then `--registry`, then `GRIM_DEFAULT_REGISTRY`, then the
configured `[[registries]]` (aliases resolve, the default entry wins) —
but unlike other commands they never fall back to the built-in registry:
with nothing configured anywhere they error (78) rather than silently
storing a credential for a registry you never named. Confirm with
`grim login --help`.

Environment variables that matter here (full table:
[Configuration][envvars]):

| Variable | Purpose |
|---|---|
| `GRIM_HOME` | Data root: cache, catalog, global config (default `~/.grimoire`) |
| `GRIM_DEFAULT_REGISTRY` | Default registry for short references |
| `GRIM_OFFLINE` | Same as `--offline` |
| `GRIM_INSECURE_REGISTRIES` | Comma-separated plain-HTTP registries (local/in-cluster) |
| `DOCKER_CONFIG` | Directory of the Docker-compatible credential `config.json` |

## Multiple Registries {#multiple-registries}

When a project draws from more than one registry, declare them in a
`[[registries]]` array in `grimoire.toml` (or the global config). When
the array is present it replaces the single-registry path: `grim search`,
`grim tui`, and the MCP server browse **all declared registries at once**
instead of one. In the TUI each registry becomes its own collapsible tree
root, with the registry prefix shown only when more than one registry
resolves.

Each entry declares **exactly one** of `oci` (a plain OCI registry) or
`index` (a package index) — never both (`url` is accepted as a pre-0.7.0
parse-time alias for `oci`):

| Field | Required | Purpose |
|-------|----------|---------|
| `oci` | one of `oci`/`index` | Registry host and optional namespace — same form as `[options].default_registry`; browsed via `_catalog` |
| `index` | one of `oci`/`index` | Package index locator (see [Index Sources](#index-sources)) |
| `alias` | no | Short name for qualified `alias/repo` references |
| `default` | no | Marks the primary registry for short-id expansion; first entry is primary when none set it |

```toml
[[registries]]
alias = "acme"
oci = "ghcr.io/acme"
default = true

[[registries]]
alias = "internal"
oci = "registry.corp.example/team"
```

Project entries take precedence over global entries; duplicate locators
are deduped, first occurrence wins.

Browse-set precedence (what `grim search`, `grim tui`, and `grim mcp`
browse):

1. `--registry` flag — collapses browse to exactly the registries it names.
   Repeatable and comma-separated (`--registry a,b` or `--registry a
   --registry b`); the first value is the primary (short-id default).
2. `[[registries]]` (project, then global) — authoritative when present;
   `GRIM_DEFAULT_REGISTRY` does **not** collapse or restrict this set.
3. Single-default fallback (no `[[registries]]` declared): `GRIM_DEFAULT_REGISTRY`
   → project `[options].default_registry` → global `[options].default_registry`
   → built-in browse fallback: the public package index at
   `https://index.grimoire.rs` (a bare registry fallback would browse
   empty — GHCR gates `_catalog`).

The same precedence applies outside a project — with no `grimoire.toml`
resolvable the project tiers are simply absent, so a search run from a
bare directory still browses the global `[[registries]]` and otherwise
falls through to the built-in public index.

A config with no `[[registries]]` behaves exactly as before — the
`[options].default_registry` / `GRIM_DEFAULT_REGISTRY` / `--registry` /
built-in fallback chain still applies (see [Registry Resolution](#registry-resolution)).
Confirm with `grim --help` and `grim search --help`.

### Index Sources {#index-sources}

A **package index** is a phone book, not a catalog: it stores pointers
(name, kind, OCI ref, description, ownership) for packages that live on
possibly many different registries, and it never stores versions — `grim`
still resolves tags live from each pointer's registry at install time, so
a stale index can never serve a stale version. Registries such as GHCR,
Docker Hub, and GitLab SaaS gate the `_catalog` endpoint `oci` entries
need; an `index` entry sidesteps that gap. The default public index is
`https://index.grimoire.rs` ([grimoire-rs/index][index-repo] on GitHub).

Two transports, chosen by the locator's shape:

| Locator shape | Transport |
|---|---|
| `http://…`, `https://…` | Static files — fetches `<base>/all.json` |
| `git+…`, `ssh://…`, `git@…`, or ending in `.git` | Git — shallow-clones and walks `index/**/metadata.json` |

```toml
[[registries]]
alias = "hub"
index = "https://index.grimoire.rs"      # static-file transport
default = true

[[registries]]
alias = "team"
index = "https://gitlab.com/acme/index.git"  # git transport
```

CLI equivalent:

```sh
grim config registry add hub --index https://index.grimoire.rs --default
```

`oci` and `index` set together on one entry is a config error (exit 78);
a locator that matches neither transport shape is a data error (exit 65).
Both transports share the regular catalog cache (`$GRIM_HOME/catalog/`,
1-hour TTL, `--refresh`, offline degradation) and browse exactly like an
`oci` entry — search, TUI, and MCP treat index and registry sources
alike. `grim publish --announce` is the write side: it publishes
pointers into an index repository rather than reading them — see
[references/publish.md](publish.md#announce).

## Managing Config {#managing-config}

`grim config` reads and writes `grimoire.toml`, modeled on `git
config`, so you rarely hand-edit the file. It covers **settings**
(`[options]`, `[options.tui]`) and **named registries** (`[[registries]]`) —
but **not declarations** (`[skills]` / `[rules]` / `[agents]` / `[bundles]`),
which stay under `grim add` / `grim remove` because those must re-resolve the
lock on every change.

- **Settings** use dotted keys — `grim config get|set|unset <key>` and
  `grim config list`:

  ```sh
  grim config set   options.clients claude,opencode
  grim config set   options.tui.default_view tree
  grim config set   options.show_deprecated true   # show deprecated artifacts by default
  grim config set   options.clients claude,opencode --dry-run  # validate + report, write nothing
  grim config get   options.clients          # bare value on one line; exit 1 if unset
  grim config list                           # every explicitly-set key in this scope
  grim config list --all                     # every supported key, incl. unset — JSON carries type/title/description/default metadata for tooling, plus a constraints object for list keys with a shape rule beyond closed-set membership
  ```

  `set` accepts `--dry-run`: it validates the key and value and reports
  the same confirmation shape a real `set` would, without acquiring the
  write lock or touching `grimoire.toml` (`unset` has no such flag).

- **Registries** use lifecycle verbs under `grim config registry`:

  ```sh
  grim config registry add acme --oci ghcr.io/acme        # registry entry (needs --oci XOR --index)
  grim config registry add hub --index https://index.grimoire.rs  # index entry — see Index Sources
  grim config registry use acme                       # set default, clearing all others atomically
  grim config registry list                           # all entries in this scope
  grim config registry rm  acme
  grim config registry fields                         # oci/index/default field metadata — works with no config at all
  ```

  `registry use` is the correct way to change the default registry.

Scope follows the usual rule — project by default, `--global` for the global
config, `--config <path>` for an explicit file; each invocation reads or
writes exactly one scope (never merged). `get` prints the bare value so it
scripts cleanly (`$(grim config get options.clients)`), exiting `1` when the
key is valid but unset. Add `--format json` to any subcommand for
machine-readable output.

**Every grim write is lossy**: comments and the `#:schema` directive are
stripped from `grimoire.toml` on any `grim config` / `grim add` / `grim
remove`. The full dotted-key list, JSON shapes, and exit codes live in the
[command reference][config-cmd] — never memorize them; confirm with `grim
config --help`.

## Qualified References {#qualified-references}

A `[[registries]]` alias enables the `alias/repo[:tag]` qualified form:

```sh
# with alias "acme" → "ghcr.io/acme"
grim add acme/code-review:1.2
# expands to: ghcr.io/acme/code-review:1.2

# with alias "internal" → "registry.corp.example/team"
grim add internal/lint-rules:stable
# expands to: registry.corp.example/team/lint-rules:stable
```

The separator is `/`, not `:` — the colon form (`alias:repo`) is not
treated as a qualified reference because it is indistinguishable from a
bare `repo:tag`. A leading segment that does not match any configured alias
is treated as a repository path component under the primary registry:
`acme/x:1` where `acme` is not an alias expands to
`<primary-registry>/acme/x:1`.

Short references (no `/`-prefix alias, no explicit registry) still expand
against the primary registry unchanged.

## Scopes

grim works in two scopes. The **project** scope is the `grimoire.toml`
discovered upward from the working directory — per-repository config
beside the code. The **global** scope is a single config at
`$GRIM_HOME/grimoire.toml` for artifacts you want everywhere.

Commands operate on the discovered project by default; `--global`
switches to the global scope (and `grim init --global` creates it).
Global-scope installs land in each client's *native* user-level
directory (for example `~/.claude/skills/`), so clients find them with
no extra configuration. The TUI flips scope at runtime with `g`.

## Client Targets

An installed artifact lands in a **client target**: `claude`,
`opencode`, `copilot`, or `codex`, each receiving the artifact in its
native layout. `grim install` and `grim update` choose targets by
precedence:

1. `--client <list>` flag (comma-separated: `--client claude,copilot`)
2. config `[options].clients` (TOML array of client names)
3. auto-detection — every client whose marker exists for the active
   scope (e.g. a `.claude/` directory in the project)
4. fallback to **all** clients when nothing is detected, so an install
   never silently targets zero clients or prefers one

The detected set is recomputed each run, never written back to config.
Pin `[options].clients` when you want deterministic targets in CI — set it
with `grim config set options.clients claude,opencode` (see [Managing
Config](#managing-config)).

## Offline Mode

grim is **online by default**: every floating-tag lookup resolves fresh
against the registry, and the result is cached write-through. A floating
tag therefore never serves a stale pin, and there is no "cache first"
mode to surprise you.

`--offline` (or `GRIM_OFFLINE`) flips to **cache-only**: all network
access is forbidden, and an operation that would need the registry fails
with exit 81 instead of silently degrading. Use it in sealed CI or
air-gapped networks. Warm the cache first with a normal online run:

```sh
grim lock              # online: resolve + cache everything declared
grim install --offline # later: cache-only, no network
```

The flag or env var are the only switches — there is no config-file
counterpart for offline.

## Search, TUI, and MCP {#search-tui-and-mcp}

`grim search [query]` splits the query on whitespace and ANDs the terms —
each term substring-matches (case-insensitive) any of an entry's kind,
repository, summary, description, or keywords. A bare kind keyword
(`skill`/`rule`/`bundle`, singular or plural) filters by kind instead of
matching as text; an empty query lists the whole catalog. Confirm the
match fields and kind-filter keywords with `grim search --help`. When
`[[registries]]` are configured, all
of them are browsed and flattened into one table. The catalog is cached
under `$GRIM_HOME` — pass `--refresh` to rebuild it from the registry,
`--registry` to collapse the browse to exactly the registries it names
(repeatable / comma-separated for several at once). Plain
output shows the one-line summary (truncated to the terminal); piped
output and `--format json` keep the full description, and JSON adds a
`repository` URL field for tooling.

```sh
grim search review
grim search --refresh --registry ghcr.io/acme --format json
```

A package the publisher has marked deprecated is **hidden by default** from
both `grim search` and the TUI — unless it is installed in the active scope
(directly or via a bundle), so a deprecated dependency you already rely on
stays visible. Reveal the rest with `grim search --show-deprecated`, the TUI
`h` key (toggles live), or by defaulting `options.show_deprecated = true`
(`grim config set options.show_deprecated true` — seeds both). A shown
deprecated entry is flagged in `grim search` output (a `deprecated` marker on
the entry, and a `deprecated` field under `--format json`, which the
`grim_search` MCP tool inherits) and the TUI (a yellow `⚠` on the entry, with
the notice in the detail pane), so you can avoid pinning it. The MCP
`grim_search` tool has no deprecated toggle of its own — it follows the
resolved scope's `options.show_deprecated`.

`grim tui` browses your declared registries' catalogs interactively: kind-grouped list,
live install state, multi-select with batch install/update/delete, and a
detail pane per entry. Press `t` to toggle between the flat list and a
grouped collapsible tree view; the tree's opening mode, opening depth, and
path-splitting characters are configurable via `[options.tui]` in
`grimoire.toml` (`default_view`, `group_by_type`, `tree_separators`,
`expand_levels` — how many tree levels open expanded; the `z` key folds
between that depth and fully-expanded at runtime. Set them with `grim
config set options.tui.<key>`, see [Managing Config](#managing-config)).
Declared local path sources and dev-installs have no registry to root
under — they group under a top-level **Local** tree root, where install/
update/delete route to the local seams (re-pack and re-materialize)
instead of the registry ones.
When `[[registries]]`
are configured, the TUI browses all of them, one collapsible root per
registry; with exactly one it elides that root. A `--registry` flag collapses
the browse to exactly the registries it names (repeatable /
comma-separated). `GRIM_DEFAULT_REGISTRY` does **not**
collapse the browse set — it is only the short-id resolution default and the
single-registry fallback when no `[[registries]]` is declared. When the active
scope has no `grimoire.toml` yet it offers to create one before starting via
popup dialogs (the registry input is pre-filled with the effective default
registry and the accepted value is persisted as a `[[registries]]` entry with
`default = true`; cancelling closes the TUI). Its install, update, and delete
actions go through the same seams as `grim add`/`install`/`uninstall`. Press
`?` inside for the full key map.

`grim mcp` runs a local [Model Context Protocol][mcp-spec] server over
STDIO. An AI agent host such as [Claude Code][claude-code] connects to it
and can call these tools:

| Tool | What it returns | Gate |
|------|-----------------|------|
| `grim_search` | Same JSON as `grim search --format json`, over the resolved scope's registries (no registry override). Args: `query?`, `refresh?`, scope | always |
| `grim_status` | Same JSON as `grim status --format json` for the requested scope. Optional `check` forwards to the same live catalog re-check as CLI `--check` (deprecated/replaced_by/update_available; the report's `checked` field says whether it ran). Args: `check?`, scope | always |
| `grim_fetch` | An artifact's content in the tool result — no install. Canonical bytes, or a `vendor` (claude/opencode/copilot) projection, or one support file via `path` (binary → base64 with `encoding: "base64"`); a `files` listing is always included. `description` fetches the repository's description companion instead (every member inline); `digest_only` resolves `{ref, digest}` with no download and composes with `description`. Content caps at 256 KiB (marked truncation); needs network. Args: `ref`, `vendor?`, `path?`, `description?`, `digest_only?`, scope | always |
| `grim_describe` | Same JSON as `grim describe --format json` — manifest-level metadata (kind, curated annotations, tags, `has_description`) without downloading content. Args: `ref`, scope | always |
| `grim_render` | Writes an artifact's vendor-native files into `dest_dir` (created if absent) — no install state. Args: `ref`, `vendor`, `dest_dir`, scope | `--allow-writes` |

The install scope is chosen **per tool call**: optional `global` /
`config` / `workspace` arguments on each tool (precedence in that order;
all omitted = project walk-up from the server's working directory).
`grim mcp` takes no scope flags — `--global`/`--config` at launch exit 64.
The server is read-only by default; `--allow-writes` (the only flag)
enables `grim_render` — leave it off for a browse-only server.
Diagnostics go to stderr; stdout is the JSON-RPC channel. Register it
in a project `.mcp.json`:

```json
{ "mcpServers": { "grimoire": { "command": "grim", "args": ["mcp"] } } }
```

Confirm current flags with `grim mcp --help`.

> **Registry note**: catalog browse (`grim search` / TUI) depends on
> the registry exposing the `_catalog` endpoint. Registries such as GHCR,
> Docker Hub, and the GitLab Container Registry (SaaS) gate this endpoint
> — an empty browse result there is expected, not an error. Explicit-ref
> operations (install, add, release, publish) work on all registries. An
> [`index` source](#index-sources) sidesteps the gap entirely — that is
> why the built-in browse fallback is the public index, not a bare
> registry. See [Registry compatibility][registry-compat] for the full
> table.

## Further Reading

- [Concepts: scopes][scopes], [clients][clients], and
  [online-by-default][online] — the semantics behind each section above.
- [Configuration][envvars] — environment variables, `[[registries]]`
  schema, precedence rules, data layout under `GRIM_HOME`.
- [The Package Index][package-index] — index spec, auto-merge rules,
  hosting your own.
- [Command reference: search][search], [tui][tui], and [mcp][mcp].

[scopes]: https://grimoire.rs/concepts.html#scopes
[clients]: https://grimoire.rs/concepts.html#clients
[online]: https://grimoire.rs/concepts.html#online-by-default-offline-on-demand
[envvars]: https://grimoire.rs/configuration.html#environment-variables
[registry-compat]: https://grimoire.rs/configuration.html#registry-compatibility
[package-index]: https://grimoire.rs/package-index.html
[index-repo]: https://github.com/grimoire-rs/index
[config-cmd]: https://grimoire.rs/commands.html#config
[search]: https://grimoire.rs/commands.html#search
[tui]: https://grimoire.rs/commands.html#tui
[mcp]: https://grimoire.rs/commands.html#mcp
[mcp-spec]: https://spec.modelcontextprotocol.io/
[claude-code]: https://docs.anthropic.com/en/docs/claude-code
