# Command Reference

Every command follows the same shape: parse references into typed values, run
the operation, and report what actually happened. Structured output renders as
an aligned table by default or as JSON with `--format json`, so the same
command serves humans and scripts.

Run `grim <command> --help` for the authoritative, always-current flag list.

## Global options {#global-options}

These apply to every subcommand:

| Flag | Effect |
|------|--------|
| `--format <plain\|json>` | Output format for structured results (default `plain`). |
| `--global` | Operate on the global scope instead of the discovered project. |
| `--config <path>` | Use an explicit project config file. |
| `--registry <ref>` | Registry for short identifiers and the browse set. Repeatable / comma-separated (`--registry a,b`); the first value is the default. |
| `--offline` | Disable all network access; work from the cache only and fail rather than reach a registry. |
| `--progress <auto\|json\|none>` | Progress rendering for long-running passes (default `auto` = tty-gated stderr bar on `install`, silent elsewhere). `json` emits NDJSON events on **stderr** — `{"event":"start","total":N}`, `{"event":"advance","position":i,"total":N,"label":"…"}` (`label` is display-only), `{"event":"finish"}` — while stdout keeps the normal report. **Experimental pre-1.0**; see [Stability](./stability.md#unstable). |
| `--log-level <level>` | Override the tracing log level (`warn`, `info`, `debug`). |

## The lifecycle commands

| Command | Purpose |
|---------|---------|
| [`grim init`](#init) | Create a fresh `grimoire.toml`. |
| [`grim config`](#config) | Read and write `grimoire.toml` settings and registries. |
| [`grim add`](#add) | Declare a skill/rule/agent/mcp server, lock it, and install it by default. |
| [`grim lock`](#lock) | Resolve declared floating tags to pinned digests. |
| [`grim install`](#install) | Materialize the locked artifacts into your AI client(s). |
| [`grim update`](#update) | Re-resolve floating tags and re-materialize changes. |
| [`grim status`](#status) | Report the state of every declared artifact. |
| [`grim remove`](#remove) | Undeclare an artifact (config + lock only). |
| [`grim uninstall`](#uninstall) | Fully remove an artifact (files + record + config). |
| [`grim search`](#search) | Search the registry catalog. |
| [`grim fetch`](#fetch) | Print an artifact's content without installing. |
| [`grim describe`](#describe) | Show an artifact's manifest metadata without downloading it. |
| [`grim tui`](#tui) | Browse the catalog interactively. |
| [`grim build`](#build) | Validate and pack a local artifact. |
| [`grim release`](#release) | Validate, pack, and push an artifact. |
| [`grim publish`](#publish) | Validate and batch-release all packages from a manifest. |
| [`grim login`](#login) | Authenticate to a registry and store the credential. |
| [`grim logout`](#logout) | Remove a stored registry credential. |
| [`grim schema`](#schema) | Print the JSON Schema for `grimoire.toml` or `publish.toml`. |
| [`grim mcp`](#mcp) | Run a local STDIO MCP server for AI agent integration. |

## grim init {#init}

Writes a fresh `grimoire.toml` in the current directory. `--registry <ref>`
seeds the default browse source as a `[[registries]]` entry with
`default = true` — the locator's shape picks the key: an index-shaped value
(`http(s)://`, `git+…`, `ssh://`, `git@…`, `….git`) is written as
`index = …`, anything else as a plain OCI `oci = …`. Without the flag, a
set `GRIM_DEFAULT_REGISTRY` is snapshotted the same way (the built-in
defaults are never written — they keep floating with the binary).
`--global` creates the global config at `$GRIM_HOME/grimoire.toml`
instead of a project-local one.

```sh
grim init --registry ghcr.io/acme
```

## grim config {#config}

`grim config` reads and writes `grimoire.toml`, modeled on [`git config`][git-config]. Before it existed, querying a setting or scripting a config change required hand-editing TOML and relying on the next command run to catch typos.

The command covers two areas of the file: **settings** (the `[options]` and `[options.tui]` tables) and **named registries** (the `[[registries]]` array). Declarations — the `[skills]`, `[rules]`, `[agents]`, and `[bundles]` tables — remain under [`grim add`](#add) and [`grim remove`](#remove), which must re-resolve the lockfile on every change.

Scope follows the same rule as every config-aware command: without a flag, `grim config` discovers and edits the project `grimoire.toml` by walking up from the working directory; `--global` targets `$GRIM_HOME/grimoire.toml`; `--config <path>` selects an explicit project file.

Every write re-runs registry validation before touching the file, so the at-most-one-`default` constraint and alias rules always hold. The serializer is shared with [`grim add`](#add) and [`grim remove`](#remove) — **comments are not preserved on any write**, with one exception: a leading `#:schema` editor directive survives every rewrite.

### Settings {#config-settings}

Four verbs operate on dotted keys:

```sh
grim config get   options.clients
grim config set   options.clients claude,opencode
grim config set   options.clients claude,opencode --dry-run
grim config unset options.tui.default_view
grim config list
grim config list --all
```

`get` prints the bare value on a single line with no key name or table header, so `$(grim config get options.clients)` works directly in shell. A valid-but-unset key exits `1` with no stdout — the same contract as [`git config`][git-config]: `grim config get options.clients || echo default`. An unknown key (typo or unsupported leaf) exits `64` without reading the config.

`set` and `unset` print a one-row confirmation table with `Action`, `Key`, `Value`, `Scope`, and `Dry Run` columns.

`set` accepts `--dry-run`: it parses the key, resolves scope, and validates the value exactly as a real `set` would, then reports the confirmation table without acquiring the write lock or touching `grimoire.toml`. Error behavior is identical to a real `set` — an unknown key still exits `64`, an invalid value still exits `65`, running outside a project still exits `79`. `--global set --dry-run` against a `$GRIM_HOME` with no existing config succeeds and creates no file (a real `--global set` does create one, seeded from empty defaults). `unset` has no `--dry-run` flag.

`list` shows every explicitly-set key and value for the active scope — keys at their default or absent values are omitted. Each invocation reads from exactly one scope, so origin is implicit in the scope flag used. Scopes are never merged: `grim config --global list` shows only global values, project `list` shows only project values.

`--all` additionally lists every supported key that is unset or at its default (plain: empty `Value` cell; JSON: `value` null, `set` false). Keys whose false/empty value collapses to unset — `options.show_deprecated`, `options.tui.group_by_type`, an empty `options.clients`, an empty `options.tui.tree_separators` — appear unset under `--all` even after being explicitly set to that value. Registry rows appear only for existing aliased `[[registries]]` entries; `registry.<alias>.default` always shows its effective value (no unset state — it is never omitted, `--all` or not); an alias-less `[[registries]]` entry is not dotted-key addressable and is never listed, `--all` or not. In plain output, an empty `Value` cell is ambiguous between unset and an explicit empty-string value (e.g. `options.default_registry` set to `""`) — the JSON `value`/`set` pair disambiguates.

`config list --format json` emits the same per-key metadata (type, title, description, default, enum values) for every row, set or unset — tooling such as editor extensions can drive a settings UI from it without hardcoding the key list.

The supported dotted keys are:

| Key | Value type | Notes |
|-----|------------|-------|
| `options.clients` | comma-separated client names, closed set | An unordered set of unique values drawn from `claude`, `opencode`, `copilot` — e.g. `claude,opencode`. An unrecognized name or a repeated segment exits `65`; input order is otherwise preserved on store. Empty string clears the list. |
| `options.default_registry` | string | Legacy field — prefer `grim config registry use` for new configs. |
| `options.show_deprecated` | `true` or `false` | `false` is the default (deprecated artifacts are hidden from `grim search` and the TUI unless installed); setting it to `false` removes the key, so a subsequent `get` exits 1 (consistent with `list`, which omits default values). Seeds the initial state for both `grim search` and `grim tui`; the search `--show-deprecated` flag and the TUI `h` key override it per run. |
| `options.tui.default_view` | `flat` or `tree` | Other values exit `65`. |
| `options.tui.group_by_type` | `true` or `false` | `false` is the default; setting it to `false` removes the key, so a subsequent `get` exits 1 (consistent with `list`, which omits default values). |
| `options.tui.tree_separators` | comma-separated single-character strings | Each character must be non-control and non-whitespace; other values exit `65`. |
| `options.tui.expand_levels` | non-negative integer | How many tree levels open expanded: `1` (default when unset) shows only registry roots, `0` opens fully expanded. Non-integer or negative values exit `65`. Setting it stores the value; a subsequent `get` echoes it (unlike the default-valued keys above, an explicit value is always kept). |
| `registry.<alias>.oci` | string | The registry entry must already exist. Mutually exclusive with `index` (setting it on an index entry exits `65`); unsettable only when `index` is set — else use `grim config registry rm <alias>`. The pre-0.7.0 field name `url` is accepted as an alias. |
| `registry.<alias>.index` | string | A [package-index](./package-index.md) locator (`http(s)://` base or git repository). Mutually exclusive with `oci` (same rules mirrored); a locator matching neither transport exits `65`. |
| `registry.<alias>.default` | `true` or `false` | Setting to `true` clears all other entries' `default` flag, the same as `grim config registry use`. |

Registry dotted keys require the entry to already exist — only `grim config registry add` creates entries. Passing `registry.<alias>` without a trailing field to `unset` removes the whole entry, equivalent to `grim config registry rm <alias>`.

### Registry lifecycle {#config-registry}

`grim config registry` manages the `[[registries]]` array through dedicated lifecycle verbs:

```sh
grim config registry add  acme --oci ghcr.io/acme
grim config registry add  acme --oci ghcr.io/acme --default
grim config registry add  hub  --index https://index.grimoire.rs
grim config registry use  acme     # mark as default; clears the prior default
grim config registry show acme     # print one registry's fields
grim config registry rm   acme
grim config registry list
grim config registry fields        # per-registry field metadata; works with no config at all
```

`registry add` requires exactly one of `--oci` / `--index` — a registry
entry lists via the OCI `_catalog` endpoint, an index entry lists from a
[package index](./package-index.md). (`--url` remains a hidden alias for
`--oci` from before 0.7.0.) Adding an alias that already exists
exits `64` — update the locator with `grim config set
registry.<alias>.oci <new-ref>`, or remove and re-add.

`registry use` is the correct way to change the default registry. It sets the target entry's `default` flag and clears the flag on all others in one atomic write. Dotted `grim config set registry.<alias>.default true` routes through the same logic.

`registry list` shows all `[[registries]]` entries in the scope. Entries without an alias (locator-only entries hand-authored before aliases were introduced) appear with an empty `Alias` cell and are **not addressable by dotted key** — assign them an alias to manage them with `grim config`.

`registry fields` lists the 3 addressable per-registry field names (`oci`, `index`, `default`) with their type, title, and description — the same static metadata the `registry.<alias>.<field>` dotted-key table above documents, in machine-readable form. It reads no config and resolves no scope: unlike every other `config` subcommand it works in a directory with no `grimoire.toml`, and with no `--global`/`--config` flag needed.

### JSON output {#config-json}

Add `--format json` to any subcommand for machine-readable output (full
cross-command contract: [the JSON interface](./json-interface.md)). The
shapes are:

| Subcommand | JSON shape |
|-----------|------------|
| `get` (value set) | `{"key":"…","value":"…","set":true,"scope":"project"\|"global"}` |
| `get` (unset, exits 1) | `{"key":"…","value":null,"set":false,"scope":"project"\|"global"}` |
| `set` / `unset` / `registry add`, `rm`, `use` | `{"action":"…","key":"…","value":string or null,"scope":"…","dry_run":bool}` — `dry_run` is `true` only for `set --dry-run`; every other write verb always reports `false` |
| `list` | `{"items": [...]}` of `{"key":"…","value":string or null,"set":bool,"type":"…","title":"…","description":"…","default":string or null,"values":[…] or null,"constraints":{"item_pattern":"…","item_width":integer} or null}` |
| `registry list` | `{"items": [...]}` of `{"alias":string or null,"oci":string or null,"index":string or null,"default":bool}` |
| `registry show` | `{"alias":"…","oci":string or null,"index":string or null,"default":bool}` |
| `registry fields` | `{"items": [...]}` of `{"key":"…","type":"…","title":"…","description":"…"}` — `key` is the short field name (`oci`, `index`, `default`), not a dotted key; no `value`/`set`/`default` (meaningless for a field pattern) |

`list` rows carry all nine fields whether or not `--all` was passed — the flag only widens the row set, never the row shape. `value` is `null` only for an unset row (surfaced only under `--all`); `set` is `value != null`. `type` is one of `string`, `boolean`, `integer`, `enum`, `string-list`, `string-set`; `values` is non-null for `enum` and `string-set` keys (the allowed value set), `null` otherwise. `title` and `description` are fixed per-key metadata, not derived from the current value; `default` is the runtime default in CLI string form, or `null` when the key has no fixed default. `constraints` is non-null only for a list key whose items carry a shape rule beyond closed-set membership — today just `options.tui.tree_separators` — and is advisory: `item_pattern` is necessary but not sufficient (it cannot express the paired `item_width` rule), so `grim`'s own validation stays authoritative even when a value matches the pattern; see [the JSON interface](./json-interface.md#shapes-items) for the full honesty contract.

Registry rows always carry both locator keys — exactly one of `oci` /
`index` is non-null for a valid entry.

The `action` field in write confirmations takes one of: `set`, `unset`, `registry-added`, `registry-removed`, `registry-default`. The `scope` field is `project` or `global`.

### Exit codes {#config-exit-codes}

| Situation | Code |
|-----------|------|
| Success | `0` |
| `get` of a valid-but-unset key (no stdout) | `1` |
| Unknown key name / missing or duplicate alias / bad subcommand args | `64` |
| Invalid value (bad enum, non-boolean, bad separator character, unrecognized or duplicate string-set entry) | `65` |
| Write or lock I/O failure | `74` |
| Concurrent write that can't acquire the config lock | `75` |
| Config file parse failure | `78` |
| Explicit `--config <path>` not found, or required config absent | `79` |

## grim add {#add}

`grim add [--kind <skill|rule|agent|bundle|mcp>] [--name <name>] [--no-install] [--force] <reference>`
declares a skill, rule, [agent](./agents.md), [MCP server](./mcp-servers.md),
or bundle, pins it in the lock, and — by default — materializes it into your
detected AI clients in one step. `<reference>` is the only required argument —
`registry/repo:tag` or `registry/repo@sha256:…`.

When `--kind` is omitted, the kind is inferred from the artifact's
`com.grimoire.kind` manifest annotation set at release time (artifacts
published by older grim are still typed from their legacy `artifactType`). When
`--name` is omitted, the binding name defaults to the reference's last path
segment. If the kind cannot be inferred (for example, a non-Grimoire image),
`add` errors and asks you to supply `--kind` explicitly.

By default `add` installs what it declares, so a single command is enough to
start using an artifact. Only the freshly-added artifact (or, for a bundle, its
members) is materialized; the rest of the lock is left for
[`grim install`](#install). Pass `--no-install` to declare and lock only —
useful when adding several artifacts before one `grim install` pass, or when
choosing clients explicitly with [`grim install --client`](#install).

Install-on-add honours the same integrity gates as [`grim install`](#install):
a previously installed artifact that was modified locally, or a pre-existing
destination grim has no record of writing, refuses with exit 65 (under
`--format json` the error document carries the [`reason`
subtype](./json-interface.md#error-reason) `modified` or
`untracked-destination`). `--force` overwrites deliberately — identical
semantics to `grim install --force`, including the untracked-destination
clobber guard — so re-running the *same* `grim add` with `--force` is the
recovery path for a modified-state refusal. With `--no-install` nothing is
materialized, so `--force` is inert.

The declared name is a unique key per kind: re-running `add` with a
`(kind, name)` pair that is already declared under a *different* reference
refuses (exit 64) instead of silently replacing it, and names the existing
reference in the error. Pass `--name` to bind the new reference under a
different name. Re-declaring the exact same reference stays a no-op
overwrite.

A skill, rule, or agent binding name becomes the install directory or
file name, so it must satisfy the [artifact name
rules](./artifacts.md#names): 1–64 characters of lowercase letters,
digits, hyphens, and periods, with no leading, trailing, or adjacent
separators. A name outside that grammar — whether passed via `--name` or
derived from the reference — refuses with exit 64 (lowercase-only also
keeps bindings collision-free on case-insensitive filesystems). Bundle
and MCP binding names are unrestricted.

A renamed skill installs under the binding name, and the installed
`SKILL.md` frontmatter `name` is rewritten to match it (the Agent Skills
standard requires the two to agree), so a rename never leaves two
installed skills claiming the same name. A renamed multi-file rule keeps
its support directory under the binding name too — relative links inside
the index that point at the original name may not resolve; grim warns
when that applies.

Installing an artifact whose `(kind, name)` is already installed at the
**other** scope for one of the same clients prints a warning: both
copies are visible to that client, and the client's own precedence
decides which wins.

```sh
grim add ghcr.io/acme/code-review:1
grim add --kind rule --name rust-style ghcr.io/acme/rust-style:2
grim add --kind bundle ghcr.io/acme/python-stack:1
grim add ghcr.io/grimoire-rs/mcp/grim:1
grim add --no-install ghcr.io/acme/code-review:1   # declare + lock only
```

Adding a [bundle](./concepts.md#bundles) declares it in `[bundles]` and expands
its members into the lock. `grim remove bundle <name>` undeclares the bundle and
drops the members it contributed — a member another still-declared bundle also
contributes only loses this bundle's provenance entry and stays locked.

If the reference is [deprecated](./publishing.md#metadata-deprecated), `add`
prints the publisher's notice on stderr and still completes the add.

### Local path sources {#add-path}

`<reference>` may also be a [local path](./concepts.md#references-tags-and-digests)
— `./skills/x`, `../shared/rule.md`, or an absolute path — for a skill, rule,
or agent. `add` declares it verbatim, pins it by the SHA-256 of its canonical
packed layer instead of a registry digest, and installs it exactly like a
registry reference:

```sh
grim add ./skills/my-skill
grim add --kind agent ../shared/reviewer.md
```

The kind is inferred from the path's shape — the same rule
[`grim build`](#build) uses: a directory with `SKILL.md` is a skill, a bare
`.md` file is a rule; pass `--kind agent` for an agent. The binding name
defaults to the artifact's own name (a skill's frontmatter `name`, or a
rule/agent file's stem) rather than a reference path segment. A relative CLI
path is rewritten to be relative to the config file's directory before it is
declared, so the recorded value is portable when a co-worker clones the
repo; an absolute path is declared verbatim (on Windows, `\` separators in
the CLI argument are normalized to `/` — the declared value is always the
forward-slash form the config grammar accepts), and a project-scope config
carrying one gets a warning on every subsequent command — it is not portable
to another machine (global scope, being machine-local already, stays quiet).
A relative path source whose `../..` chain resolves outside the workspace
root — for example `../../shared/skill` from a monorepo — gets the same
warning even though it stays portable: it reads a file the workspace
boundary does not contain. See the [path-source trust
model][path-source-trust] for the full reasoning.

A bundle has no local-path form via `add`: `grim add --kind bundle
./bundles/x.toml` refuses (exit 64) and points you at declaring the entry in
`[bundles]` directly and running [`grim lock`](#lock) instead — see
[`[bundles]`](./configuration.md#bundles) for the local-bundle shape.

## grim lock {#lock}

Resolves the floating tags declared in `grimoire.toml` to concrete digests and
writes `grimoire.lock`. Run it after editing the config by hand; `grim add`
already locks what it declares.

## grim install {#install}

Materializes every locked artifact into your AI clients' configuration
directories. `--client <list>` selects AI clients (`claude`, `opencode`,
`copilot`, `codex`, comma-separated), overriding the config `clients` option. When
neither selects a client, the **detected** clients for the scope are
targeted — every client whose vendor directory or marker is present —
falling back to all clients when none are detected. `--force` overwrites a
locally modified artifact instead of refusing it.

Install never clobbers files it did not create: a destination that already
exists on disk **without an install record** — a hand-authored skill
directory, a rule file, or an MCP config member owned by you or another
tool — is refused (exit 65) with the conflicting path named. `--force`
overwrites and records it. One exception: when the existing content is
identical to what the install would write, it is **adopted** into the
install record and reported `unchanged` — so deleting the state file while
leaving rendered files intact repairs itself on the next install.

```sh
grim install
grim install --client claude,copilot
```

### Dev-install a local path {#install-dev}

`grim install <path>` — a [local path](./concepts.md#references-tags-and-digests)
positional argument (`./…`, `../…`, or absolute; Windows `\` separators are
accepted and normalized) — renders a skill, rule, or
agent straight from disk into your clients **without** touching
`grimoire.toml` or `grimoire.lock`. It is a throwaway test loop: edit the
source, re-run the same command, see the change land, with nothing declared.

```sh
grim install ./skills/my-skill
grim install ../shared/reviewer.md --kind agent
```

An install record is still written, marked `dev`, so the artifact stays
visible instead of turning into an untracked file: [`grim status`](#status)
lists it with `Source` reading `path: <path> (dev)`,
[`grim update`](#update) re-packs the source and re-materializes it when the
content drifts, and [`grim uninstall`](#uninstall) removes it. Unlike an
orphan dropped from the lock, a dev record is **never** pruned automatically
— removing it is always an explicit `grim uninstall`. `--kind` overrides the
path-shape inference the same way it does for [`grim add`](#add-path);
`--client` and `--force` behave exactly as for a normal install.

Reach for [`grim add <path>`](#add-path) instead when you want the source
declared — committed to `grimoire.toml` and shared with anyone who clones
the repo, rather than local to this checkout.

A dev-install is refused (exit 64) when the packed artifact's own
`(kind, name)` — its frontmatter `name` for a skill, its file stem for a
rule or agent — matches a binding already declared in `grimoire.toml`, with
guidance to either remove the declaration or dev-install the local artifact
under a different name or `--kind`. A dev record and a declared binding are
stored under the same install-record key; letting a colliding dev-install
through would leave a later [`grim uninstall`](#uninstall) free to drop the
declared binding it never actually owned, silently losing config the
dev-install never touched. The check runs before the local path is
installed or recorded, so the existing declaration is left untouched.

## grim update {#update}

`grim update [names…]` re-resolves floating tags, rolls the lock forward, and
re-materializes only what changed. With no names it updates everything; pass
binding names to scope it. Shares `--client` and `--force` with install.

```sh
grim update
grim update code-review rust-style
```

Because update reconciles the workspace to the freshly-resolved lock, it also
**prunes** artifacts that have dropped out of the lock — most often a
[bundle](./concepts.md#bundles) member that the bundle stopped including. A
clean, unmodified orphan is deleted (files and install record) and reported with
the `removed` action. An orphan you have edited locally is **kept** and reported
as `kept-modified`, so an accidental bundle change never silently discards your
work; re-run with `--force` to prune it anyway. This mirrors the install
integrity gate, where a locally modified artifact is refused rather than
overwritten without `--force`.

Pruning happens only on `update`. `grim install` materializes the current lock
but never deletes — like [`grim remove`](#remove), it leaves files on disk.

`update` also refreshes every [local path source](#add-path): a declared
path dependency and a [dev-installed](#install-dev) record alike are
re-packed, and a changed content hash re-materializes them — the local
equivalent of a floating registry tag rolling to a new digest. Before you
run it, [`grim status`](#status) surfaces that drift ahead of time as
`outdated`, the same state a moved registry tag produces.

## grim status {#status}

Reports each declared artifact's state — installed, outdated, locally modified,
integrity-missing, or not installed. The `Source` column shows each artifact's
[provenance](./concepts.md#bundles): `direct` for a registry declaration, the
bundle it came from, `path: <path>` for a declared [local path
source](#add-path), or `path: <path> (dev)` for a [dev-installed](#install-dev)
record. Pair with `--format json` to drive automation.

`--format json` output carries an `outputs` array per artifact: the
per-[client](./concepts.md#clients) locations the artifact was materialized
to, read back from install state. It is empty for a declared-but-not-installed
artifact. This is the supported way to script "where did grim put this file?"
— the on-disk layout under each client's directory is an implementation
detail and may change. Each item also carries `clients_missing` /
`clients_extra`: the project's configured client target diffed against the
artifact's recorded install-state clients, entirely from local state (no
network) — sorted arrays, `[]` when the two sets agree.

```json
{
  "items": [
    {
      "kind": "skill",
      "name": "code-review",
      "source": "direct",
      "pinned": "ghcr.io/acme/code-review@sha256:1f2e...",
      "state": "installed",
      "outputs": [
        { "client": "claude", "path": "/repo/.claude/skills/code-review" }
      ],
      "clients_missing": [],
      "clients_extra": [],
      "deprecated": null,
      "replaced_by": null,
      "update_available": null
    }
  ],
  "checked": false
}
```

### grim status --check {#status-check}

`--check` adds one live catalog lookup — the same
[`load_catalog`](#search) seam `grim search`/`tui`/`mcp` share, scoped to
the project's configured registries — and fills in `deprecated` /
`replaced_by` on every registry-sourced row (a directly-declared or
bundle-member artifact; a declared bundle, a dev-install, or a
[path-sourced](#add-path) artifact carries no registry pin, so it is never
matched). It costs one network round-trip regardless of how many artifacts
are declared.

`checked` (top-level, alongside `items`) reports whether the check actually
ran: `true` only when `--check` was passed and the invocation is online.
Combined with `--offline` (or `$GRIM_OFFLINE`), the check is skipped
entirely — one stderr warning explains why, `checked` stays `false`, every
`deprecated`/`replaced_by` stays `null`, and the command still exits `0`. A
single registry's catalog refresh failing (offline cache, transport error)
degrades only that registry's rows to `null`; `checked` still reports `true`
— the attempt was made online, it just came back partial for that source.

`update_available` is populated by a **fresh per-artifact re-resolution**,
independent of that one catalog lookup: for each directly-declared,
registry-locked row grim re-discovers the registry's current
representative tag and compares its digest to the lock pin — the same "is a
newer version available?" decision the [TUI](#tui)'s `↑ outdated` badge
uses (so a newer semver release surfaces even when the cached catalog tag
is stale). It is `true` when the registry's latest digest differs from the
lock pin, `false` when it matches (or the tag vanished), and `null` for a
row with no lock pin (declared-bundle, dev-install, [path source](#add-path)),
a bundle-member row (it updates via its bundle, not its own tag), or an
artifact whose re-resolution failed — a completed re-resolve never reports
`null`, and a failed one never lies as `false`. These per-artifact checks
run with bounded concurrency; `status` still always exits `0`.

```sh
grim status --check
grim status --check --format json
```

## grim context {#context}

`grim context` reports the **resolved invocation context** — read-only,
offline, no side effects. It answers "what would grim act on from here?"
without a consumer reimplementing the config walk-up, client detection, or
registry precedence rules: the resolved scope, the config/lock/state paths
(with existence flags for config and lock), the effective
[client](./concepts.md#clients) target set, the resolved
[registry browse set](./configuration.md#multiple-registries), the primary
registry, and whether the run is [offline](#global-options) (and why:
`flag` or `env`).

```sh
grim context --format json | jq .clients
```

The JSON document is a single object: `{version, scope, workspace,
config_path, config_exists, lock_path, lock_exists, state_path, grim_home,
offline, offline_source, clients, registries, default_registry}`.
`registries` entries are `{alias, url, kind, default, authenticated}` with
`kind` either `registry` or `index`. `authenticated` is a boolean: `true`
when a credential for this registry's **host** is present in the
docker-compatible credential store (`~/.docker/config.json`, or
`$DOCKER_CONFIG/config.json`) — an `auths` or `credHelpers` entry for the
host. It is a file-only probe that never invokes a credential helper, so a
global `credsStore` with no matching per-host entry is **not** detected as
authenticated, and a missing, unreadable, or malformed config reports
`false` for every registry rather than failing the command. The host is the
url with any scheme and namespace path stripped, matching the credential
grim uses when pulling from that registry. `clients` carries **names
only** — the vendor on-disk layout is not a contract; script installed
locations via [`grim status --format json`](#status) `outputs` instead.

Scope follows the usual rules: project walk-up by default, `--global` for
the global scope, `--config <path>` for an explicit file. Outside a
project without `--global` it exits `79` like every other scope command.

## grim remove {#remove}

`grim remove <kind> <name>` (`<kind>` is `skill`, `rule`, `agent`, `bundle`,
or `mcp`) undeclares an artifact from `grimoire.toml` and the lock. It
leaves already-installed files (or, for an [MCP server](./mcp-servers.md),
the registered config entry) in place — use [`grim uninstall`](#uninstall)
to remove those too.

Removal acts on the **effective** declaration, fully offline: the lock entry
is dropped only when no remaining declaration holds the artifact. Removing a
direct declaration while a declared bundle still names the artifact at the
*same* identifier keeps the entry — its provenance flips to the bundle. If
the bundle names it at a *different* identifier, the correct pin cannot be
derived offline: the entry is dropped, the lock is left stale, and grim tells
you to run [`grim lock`](#lock) — never a silently incomplete fresh lock.

A [dev-installed](#install-dev) record was never declared, so `remove` has
nothing to undeclare for it — use `grim uninstall` to drop one instead.

## grim uninstall {#uninstall}

`grim uninstall <kind> <name>` (`<kind>` is `skill`, `rule`, `agent`, or
`mcp`) is the full inverse of install: it deletes the materialized files,
drops the install record, and undeclares the artifact from the config and
lock. The interactive TUI's delete action reuses the same seam. For an
[MCP server](./mcp-servers.md#modification-detection), there is no
materialized file to delete — grim splices only the managed entry back out
of each client's config file, leaving the file itself and every other
entry untouched.

The lock follows the same effective-declaration rule as
[`grim remove`](#remove): when a declared bundle still names the artifact at
the same identifier, the files are deleted (that is what you asked for) but
the lock entry survives via the bundle — the next `grim install`
rematerializes it.

`uninstall <kind> <name>` is also the removal path for a
[dev-installed](#install-dev) record: since a dev install writes only an
install record and never a declaration, the file-deletion half behaves
exactly like a registry artifact's uninstall — same addressing, same
deleted files, same dropped record — and the undeclare half is a no-op
(there was nothing declared to drop).

## grim search {#search}

`grim search [query]` searches the registry catalog by case-insensitive
substring against repository, summary, description, and keywords; an empty
query lists the whole catalog. The query is whitespace-split and the terms
are ANDed — every term must match somewhere. A bare kind keyword (`skill`,
`rule`, `agent`, `mcp`, `bundle` — singular or plural) filters by kind
instead of matching as text, so `grim search skill review` finds skills
matching "review". When `[[registries]]` are configured, all
of them are browsed and the results are flattened into one table.
`--refresh` forces a catalog rebuild; `--registry <ref>` collapses the
browse to exactly the registries it names — repeatable and comma-separated
(`--registry a,b` or `--registry a --registry b`), first value is primary.
`GRIM_DEFAULT_REGISTRY` is only the
short-id resolution default — it does not restrict the browse set when
`[[registries]]` is configured.

The plain table shows each entry's short summary (`com.grimoire.summary`),
falling back to the description when no summary is set. On an interactive
terminal that column is truncated to fit the width; piped output and
`--format json` keep the full description. The JSON output also carries a
`repository` field — the artifact's authored
[repository URL](./publishing.md#metadata-repository), or `null` when the
artifact has none — and a `replaced_by` field — the authored
[successor reference](./publishing.md#metadata-replaced-by), or `null` when
none. Neither is shown in the plain table.

A [deprecated](./publishing.md#metadata-deprecated) entry is **hidden by
default** — unless it is installed in the active scope (directly or via a
bundle), in which case it stays listed so you can see what you have. Pass
`--show-deprecated` to include every deprecated entry, or set
[`options.show_deprecated`](#config) to `true` to change the default. A shown
deprecated entry is flagged in the `Status` cell with a comma-suffixed
`deprecated` (e.g. `installed,deprecated`), and JSON carries the notice in a
`deprecated` field (`null` when the artifact is not deprecated).

```sh
grim search review
grim search --show-deprecated review
grim search --refresh --registry ghcr.io/acme
```

## grim fetch {#fetch}

`grim fetch <ref>` resolves an artifact and prints its content — **use ≠
install**: nothing is materialized, no state is touched. It is the CLI
port of the MCP [`grim_fetch` tool](#mcp): canonical (as-authored) content
by default, a `--vendor <claude|opencode|copilot>` projection, or one
`--path <tree-path>` support file. Two more flags switch the report to a
different shape entirely instead of fetching the artifact:
[`--description`](#fetch-description) fetches the repository's description
companion, and [`--digest-only`](#fetch-digest-only) resolves a digest
without downloading anything.

A `--path` file is UTF-8 text by default. A **binary** support file (e.g. a
`logo.png`) that fits within the size limit is returned base64-encoded — the
JSON report gains an `encoding: "base64"` field, and plain output **decodes
it back to the raw bytes**, so a redirect round-trips byte-identical:

```sh
grim fetch skills/code-review > SKILL.md
grim fetch skills/code-review --path code-review/references/checklist.md
grim fetch skills/code-review --path code-review/logo.png > logo.png   # binary, byte-identical
grim fetch skills/code-review --format json | jq '.files[].path'
```

Plain output is the **raw content payload** — exact bytes, no table, no
added trailing newline — so it pipes.

`--format json` emits the full fetch report: `{ref, digest, kind, name,
vendor, path?, content, encoding?, truncated?, files?, pointer?, warnings?}`
(the MCP payload shape — empty/default fields omitted; `encoding` is present
only for a base64 binary `--path` file). Warnings print to stderr, keeping
stdout a pure payload.

Unlike the MCP tool — which truncates documents at 256 KiB for tool-result
budgets — the CLI **never truncates** a printed payload. Two ceilings guard
the download instead, with different failure modes: the manifest's
declared layer size is checked against the 8 MiB limit *before* download
(a cheap reject for an honestly-oversized layer, exit 65), and that same
declared size then bounds the *actual* streamed bytes — a registry that
serves more than it declared aborts mid-transfer into a data error (exit
65) rather than growing an unbounded body in memory. A missing repository
or tag — the artifact itself, or (with `--description`) its companion —
is a not-found failure (exit 79); an offline run against an uncached
reference exits 81 instead, since the reference may exist and only the
network to confirm it is unreachable.

### Description companion (--description) {#fetch-description}

`--description` retargets the reference to the repository's [description
companion](./publishing.md#description-companion) — the reserved
`__grimoire` tag is a grim-internal implementation detail; the flag is
the documented way to reach it, so nothing outside grim needs to type or
know the tag. `--format json` returns every packed file inline in one
call instead of the single artifact document:

```sh
grim fetch ghcr.io/acme/mcp/postgres --description --format json | jq '.files[].path'
grim fetch ghcr.io/acme/mcp/postgres --description --out ./docs   # unpack the tree to disk
```

The JSON shape is `{ref, digest, kind: "desc", files: [{path, size,
content, encoding?}], warnings?}` — every member inline (`encoding:
"base64"` only for a binary member), bounded by the same 8 MiB layer gate
as any fetch and never per-file truncated, so the whole companion —
README, logo, changelog, and any README-referenced assets — comes back in
one report. A multi-file bundle has no single payload to print, so plain
mode requires `--out <dir>` to unpack the tree to disk instead; without
`--out` (and no `--path`), plain mode is a usage error (exit 64).

### Cache probe (--digest-only) {#fetch-digest-only}

`--digest-only` resolves the reference to a digest and reports `{ref,
digest, warnings?}` **without downloading** the manifest or any blob — a
cheap HEAD-equivalent probe. It composes with `--description` to probe
the companion tag instead of the artifact; either way the reported digest
equals the corresponding full fetch's manifest digest, so a consumer
caches on it directly and skips an unchanged download:

```sh
grim fetch skills/code-review --digest-only --format json
grim fetch skills/code-review --description --digest-only --format json
```

`--digest-only` takes no `--vendor`, `--path`, or `--out` — a digest-only
probe never downloads content to shape, so combining any of them is a
usage error (exit 64). An offline run against an uncached reference exits
81 rather than a misleading not-found.

### Flag-combination errors {#fetch-usage-errors}

| Combination | Result |
|---|---|
| `--out` without `--description` | usage error (exit 64) |
| `--vendor` with `--description` | usage error (exit 64) |
| `--digest-only` with `--vendor`, `--path`, or `--out` | usage error (exit 64) |
| `--description` in plain mode, without `--out` and without `--path` | usage error (exit 64) |

## grim describe {#describe}

`grim describe <ref>` reports an artifact's **manifest-level metadata** —
its kind, curated annotations, and tags — **without downloading its
content**. Where [`grim fetch`](#fetch) pulls the layer to print the
document, `describe` only resolves the reference, lists the repository's
tags, and reads the manifest annotations, so it is a cheap way to inspect an
artifact (or discover its available versions) before fetching or installing.
It is the CLI port of the MCP [`grim_describe` tool](#mcp) and takes no flags
beyond the globals.

`--format json` emits a single object with every field always present
(explicit `null` when absent; `keywords`/`tags` are `[]` when none, and
`annotations` is the verbatim manifest annotation map):

```
{ref, digest, kind, name, title, description, has_description, summary,
 version, license, repository, revision, created, keywords, deprecated,
 replaced_by, tags, annotations}
```

`kind` is `null` for a foreign / non-Grimoire manifest (describe never
hard-errors on one). `has_description` is an always-present boolean —
whether the repository carries a [description
companion](./publishing.md#description-companion) — derived from the tag
listing describe already fetches, at zero extra network cost, so a
consumer can skip a blind probe before calling
[`grim fetch <ref> --description`](#fetch-description). `tags[]` lists
only user-facing tags — grim-internal companions such as `__grimoire` (see
the
[repository description companion](./publishing.md#description-companion)) are
hidden. The
curated fields follow [`grim search`](#search) semantics: `repository` is kept only when the source annotation is an
`https://` URL, `deprecated` is the deprecation notice or `null`, and
`replaced_by` is the [successor reference](./publishing.md#metadata-replaced-by)
or `null`. Plain output is a flat key/value table (like
[`grim context`](#context)) with `keywords` and `tags` comma-joined.

```sh
grim describe skills/code-review
grim describe ghcr.io/acme/skills/code-review:1.2.0 --format json | jq .tags
```

Errors follow the fetch taxonomy: a missing repository is a not-found
failure (parity with `grim fetch`), an auth failure exits 80, an offline run
that cannot reach the registry exits 81, and an unreachable registry exits
69.

## grim tui {#tui}

`grim tui` opens an interactive browser over your declared registries'
catalogs. It shows the catalog with live install state in colour, opening in
the collapsible tree view by default and toggling to a flat kind-grouped list
(press `t`; set [`options.tui.default_view`][options-tui] to `"flat"` to open
there instead). When
more than one registry is configured, the flat list adds a leading **Registry**
column showing the configured alias (or the raw URL when no alias was set), and
the Repo cell is shortened to the registry-relative path so names stay readable.
It supports multi-select with batch install, update, and delete. Press `?` in the TUI
for the full key map; highlights are `t` to toggle tree/flat view, `v` to
pick a version, `o` to open the selected entry's repository URL in the
browser, `g` to switch scope, `h` to show/hide deprecated artifacts, and
`space` to mark rows.

Like [`grim search`](#search), deprecated artifacts that are not installed are
**hidden on open** (installed-but-deprecated rows stay visible, still marked).
Press `h` to reveal or re-hide them live, pass `--show-deprecated` to open with
them shown, or set [`options.show_deprecated`](#config) to `true` for the
default.

**Tree view** — pressing `t` switches the catalog between flat list mode and
a collapsible tree grouped by browse source and repository path. Rows from an
OCI registry group under that registry (host plus configured namespace); rows
from a [package index](./package-index.md) group under the index source, with
the full OCI reference folded below it (an unbranched host/namespace chain
like `ghcr.io/grimoire-rs` renders as one joined node). In tree mode:

| Key | Action |
|-----|--------|
| `t` | Toggle between flat list and tree view. |
| `→` | Expand the selected group (reveal its children). Tree mode only. |
| `←` | Collapse the selected group. On an already-collapsed group or on a leaf entry, jump to the parent group instead (ARIA-style navigation). Tree mode only. |
| `z` | Fold the whole tree: if anything is collapsed, expand everything; otherwise collapse back to the configured [`expand_levels`][options-tui] depth. Tree mode only. |
| `Enter` on a group | Fold or unfold the group (same as `→`/`←` toggle); on a leaf entry, open the detail pane as usual. |
| `space` on a group | Mark every descendant leaf in the subtree. The group's mark glyph turns filled (`▣`) when all descendants are marked. |
| `i` / `u` / `d` on a group | Install, update, or uninstall every leaf in the subtree (when no other rows are individually marked). Batch behavior follows the same selection precedence as the flat view. |

Each group row shows a rollup glyph reflecting the worst install state of
its descendants — `↑` when any descendant is outdated, `✱` when any is
locally modified, and so on — so a collapsed tree still surfaces what needs
attention.

**Compact namespaces** — a run of namespace segments that never branches
collapses into one row whose label is the joined path, the same idea as [VS
Code's "compact folders"][vscode-compact] folding `a/b/c` when each level
holds a single child. The join merges namespace groups into each other only —
never a namespace into the package row directly below it — and stops where the
path branches, so a registry holding only `acme/team/skills/lint` and
`acme/team/skills/fmt` shows `acme/team/skills` as one group above the `lint`
and `fmt` leaves. A registry root always keeps its own row.

**Bundle member expansion** — when the selected row is a bundle leaf, pressing
`→` (or `Enter`) reveals its members as indented child rows badged
`(via bundle)`. Member rows are read-only: they reflect what a bundle
declares, derived from the registry (or the lock snapshot when offline).
Bundle members cannot be individually marked, installed, or uninstalled from
the tree — use the parent bundle row for batch operations.

**Local group** — [declared path sources](#add-path) and
[dev-installed](#install-dev) records have no registry to root under, so
they group under their own top-level **Local** root alongside the registry
roots, carrying the same `path: <path>` / `path: <path> (dev)` provenance
[`grim status`](#status) reports. `i` / `u` / `d` on a Local row route to
the local seams instead of the registry ones: install/update on a
declared-path row re-locks it in place (the `grimoire.toml` entry is
untouched) and materializes it, install/update on a dev row re-packs and
re-materializes it, and delete removes the materialized files either
way — additionally dropping the `[skills]` / `[rules]` / `[agents]` entry
for a declared-path row, or just the install record for a dev row (which
was never declared).

An active search (started with `/`) reveals matching entries even when their
parent group is collapsed — the tree stays navigable in search mode and does
not force a switch to flat view.

Four config fields under `[options.tui]` in `grimoire.toml` let you set
the opening view mode, how many tree levels open expanded, and how paths are
split into groups. See [`[options.tui]`][options-tui] for the full reference.

Like `grim search`, the TUI browses **every** registry declared in
`[[registries]]`, grouping entries under one collapsible root per registry.
When exactly one registry resolves, its root prefix is elided to keep names
short; with several, the roots are ordered by resolution precedence, and a
registry that is empty or offline still appears as an empty `0/0` root so the
full configured set stays visible. An explicit `--registry` flag collapses the
browse to exactly the registries it names — repeatable and comma-separated
for several at once. `GRIM_DEFAULT_REGISTRY` is only the
short-id resolution default — it does not collapse the browse set when
`[[registries]]` is configured; in that case both `grim search` and `grim tui`
browse all declared registries regardless of whether the env var is set.

When the active scope has no `grimoire.toml` yet, the TUI offers to create
one before starting, as popup dialogs: confirm the init, pick the browse
source type — **index** (a [package index](./package-index.md), the
default) or **oci** (a plain registry listing via `_catalog`) — then
accept or edit the type's pre-filled locator. The pre-selected type and
its prefill come from the effective browse primary — the `--registry`
flag, then the configured `[[registries]]` primary / legacy default
chain, then the built-in fallback **index** `https://index.grimoire.rs`;
the other type prefills its built-in fallback. The accepted value is
persisted as a `[[registries]]` entry with `default = true` in the new
config, keyed `index` or `oci` by the locator's shape — the type choice
picks the prefill, the shape keys the entry (clearing the input seeds
nothing). Cancelling closes the TUI.

`enter` opens the detail pane for the selected row: the centered artifact
reference, its `Summary:` and `Description:` sections, and a `Metadata:`
block with the keywords and the
[repository URL](./publishing.md#metadata-repository) (version and install
status stay on the catalog row). `↑`/`↓` always move the selection —
detail open or not — so navigation is never stranded; `esc` returns to
the list. The pane itself scrolls with `j`/`k` (line by line) and
`pgup`/`pgdn` (a page), from any mode — no need to open it first.
Scrolling is clamped at both ends: it saturates at the top and stops
when the content's last line reaches the pane's bottom edge.

A TUI install or update goes through the same seams as the commands: it
declares the entry in the active scope's `grimoire.toml` and relocks it (like
[`grim add`](#add)), then materializes just that artifact (like
[`grim install`](#install)). Delete is the full inverse via the
[`grim uninstall`](#uninstall) seam. Installing a version older than the
registry's latest flips the row to `outdated` right after the install
completes.

A bundle row works the same way at the bundle level. Install declares it
under `[bundles]`, expands it into its members (like
`grim add --kind bundle`), and materializes exactly those members; the row's
state aggregates the member states. Delete removes the member files and
records, evicts the members from the lock, and undeclares the bundle. A
member shared with another still-declared bundle is spared: its files stay
on disk and its lock entry only loses the deleted bundle's provenance.

```sh
grim tui --registry ghcr.io/acme
```

## grim build {#build}

`grim build <path>` validates and packs a local skill directory, rule `.md`
file, [agent](./agents.md) `.md` file, [MCP server](./mcp-servers.md)
`.toml` file, or bundle `.toml` file without pushing it — a dry run for
authors. `--kind <skill|rule|agent|bundle|mcp>` forces the artifact kind
instead of auto-detecting it from the path. An agent always needs `--kind
agent` — a bare `.md` packs as a rule; an MCP server always needs `--kind
mcp` — a bare `.toml` packs as a bundle. `--git` embeds
[git provenance](./publishing.md#git-provenance) (commit revision, commit
date, and the `origin` remote) so the preflight reflects what a release would
stamp.

## grim release {#release}

`grim release <path> <reference>` validates, packs, and pushes an artifact.
By default a full semver reference (e.g. `1.2.3`) applies cascade tags —
`1.2.3`, `1.2`, `1`, and `latest` are all moved — while a non-version tag
(e.g. `canary`, `edge`) publishes only that exact tag with no cascade.
`--cascade` asserts the cascade and requires a full semver: a non-semver tag
with `--cascade` exits 65 (a typo guard). `--no-cascade` suppresses the
floats and publishes only the exact tag, even for a full semver. A reference
with no tag at all is an error. A reference tag in grim's reserved namespace
(`__grimoire` or `__grimoire.<x>`) is a usage error (exit 64), refused before
any packing or push so a [description companion](./publishing.md#description-companion)
tag can never be overwritten. `--dry-run` prints the push plan without
pushing; `--force` moves an existing exact-version tag that points at a
different digest;
`--skip-existing` (conflicts with `--force`) turns a release whose
exact-version tag already exists into a success no-op that pushes nothing —
for manifest-driven publishers that re-run blanket releases and only want
bumped versions pushed. A `.toml` path publishes a
[bundle](./concepts.md#bundles) by default, or an
[MCP server](./mcp-servers.md) with `--kind mcp`; `--pin` (bundles only)
freezes floating members to digests. `--git` embeds
[git provenance](./publishing.md#git-provenance) (commit revision, date,
and `origin` remote) as OCI annotations; it is off by default so an
ordinary re-release stays idempotent. `--push-registry <host[/prefix]>`
pushes to a different endpoint while every baked and reported name — the
source-annotation fallback, pinned bundle member ids, the report `ref` —
keeps the reference's registry (the canonical pull name); a malformed
value exits 65 — see
[Push vs pull registries](./publishing.md#batch-publish-push-registry).
See [Publishing](./publishing.md) for the full workflow.

Pointing `grim release` at a `publish.toml` (a file with a top-level
`registry` key) produces a hint to use `grim publish` instead. The mirror
also holds: pointing `grim publish` at a bundle TOML (flat `name = "reference"`
entries) produces a hint to use `grim release --kind bundle`. A `.toml`
carrying a `[server]` table gets the equivalent nudge toward
`grim release --kind mcp` — see
[MCP Server Artifacts](./mcp-servers.md#publishing).

```sh
grim release ./code-review ghcr.io/acme/code-review:1.2.3 --dry-run
grim release ./python-stack.toml ghcr.io/acme/python-stack:1.0.0 --pin
```

## grim publish {#publish}

`grim publish` reads a `publish.toml` manifest and releases every declared
package in kind order (skills → rules → agents →
[mcp servers](./mcp-servers.md) → bundles, alphabetical within kind). It
validates the whole manifest before any push, then composes
[`grim release`](#release) per entry.

The default behavior skips entries whose exact-version tag already exists,
making the command idempotent: re-running after a partial failure pushes only
the remaining entries. Pass `--force` to move existing exact-version tags
instead. The two modes are mutually exclusive.

`--dry-run` validates the manifest and prints the full push plan without
touching the registry. `--only <name>` (repeatable) filters to a single
entry; a name absent from the manifest exits 65. `--version <version>` is
the single version source: a **semver** value overrides the manifest's
top-level `version` (the catalog-wide value entries inherit) and every entry
cascades; a **non-semver** value (e.g. `canary`) is a movable channel tag
applied to every entry uniformly, with no cascade. Not every non-semver
value is accepted as a channel: a prerelease or build-metadata version
(`1.2.3-rc.1`, `1.2.3+build`), a reserved cascade-float shape (`latest`, a
bare major such as `1`, or a `major.minor` such as `1.2`), or a value that
is not a legal OCI tag (e.g. a slash-bearing CI ref like `feature/foo`) all
exit 65 at validation, before any push. The manifest's `version_prefix`
(default `v`) is stripped first, so publishing from a CI git tag is
`--version "$GITHUB_REF_NAME"` — see
[One version for the whole catalog](./publishing.md#batch-publish-version).
`--cascade` / `--no-cascade` control the cascade for the whole run;
`--cascade` combined with a channel value exits 65. Like every publish, a
channel value skips-existing by default and needs `--force` to move.
`--manifest <path>` selects a manifest other than the default `./publish.toml`.
`--git` embeds [git provenance](./publishing.md#git-provenance) on every
published entry (forwarded to each `release`); a non-git path fails (65).
`--announce` announces the published packages to a
[package index](./package-index.md) git repository after the pushes —
`--announce-repo <url>` picks the index repository (default
`https://github.com/grimoire-rs/index`); the token comes from
`GRIM_ANNOUNCE_TOKEN` (see [Publishing from CI](./ci.md)).
The [global `--registry` flag][global-options] overrides the manifest's
`registry` value for staging runs or acceptance tests without editing the file.
`GRIM_DEFAULT_REGISTRY` and the config-file `default_registry` do **not**
override the manifest — the manifest's `registry` field is explicit input, and
only the flag tier wins.
`--push-registry <host[/prefix]>` (or the manifest's `push_registry`; the
flag wins) pushes every entry to a different endpoint while all baked and
reported names keep the manifest `registry` — the canonical pull name —
see [Push vs pull registries](./publishing.md#batch-publish-push-registry).

Exit codes from the release path propagate per entry. Validation failures
exit 65 (data error). The report renders for all completed entries plus
the first failed entry; re-run with `--only` for surgical recovery.

```sh
grim publish --dry-run
grim publish
grim publish --only grim-usage
grim publish --version canary
```

See [Batch publishing with a manifest](./publishing.md#batch-publish) for
the manifest schema, source layout conventions, and disambiguation from
bundle files.

## grim login {#login}

`grim login [registry]` authenticates to a registry and stores the credential
in the Docker-compatible credential store, so later pulls and pushes reuse it.
Pass the username with `-u`/`--username` (prompted on a terminal when omitted)
and the password via `--password-stdin` or a hidden terminal prompt — there is
no `--password <value>` flag, by design. `--allow-insecure-store` permits a
base64 plaintext entry when no credential helper is configured. By default
the credential is verified against the registry before it is stored: a
rejected credential exits 80 and stores nothing, an unreachable registry
exits 69; `--no-verify` skips the ping and stores optimistically — see
[Verification](./authentication.md#login-verify) for the full outcome
table and the offline interplay. With no
positional `registry`, it resolves `--registry`, then `GRIM_DEFAULT_REGISTRY`,
then the configured `[[registries]]` (aliases resolve; the default entry
wins) — but never the built-in fallback registry: with nothing configured
anywhere the command fails with exit 78 rather than storing a credential
for a registry you never named. See
[Authentication](./authentication.md) for storage details.

```sh
echo "$TOKEN" | grim login ghcr.io -u alice --password-stdin
```

## grim logout {#logout}

`grim logout [registry]` removes a stored credential. It is idempotent —
logging out when nothing is stored exits `0` — and resolves the registry the
same way [`grim login`](#login) does.

```sh
grim logout ghcr.io
```

## grim schema {#schema}

`grim schema --kind <config|publish|lock|mcp>` prints a [JSON
Schema](https://json-schema.org/) for one of grim's TOML files to stdout.
`--kind config` describes `grimoire.toml`; `--kind publish` describes
`publish.toml`; `--kind lock` describes `grimoire.lock` (generated by grim,
published so tooling can validate or introspect it); `--kind mcp` describes
the [MCP server descriptor](./mcp-servers.md) (`mcp/<name>.toml`). Each
schema is generated from grim's own parser, so it accepts exactly what grim
accepts.

```sh
grim schema --kind config > grimoire-config.schema.json
grim schema --kind publish | jq .title
grim schema --kind lock | jq .title
grim schema --kind mcp | jq .title
```

The same schemas are published to the docs site; see [Editor schema
support](./configuration.md#editor-schema) for the hosted URLs and the
`#:schema` directive that wires an editor up to them.

## grim mcp {#mcp}

`grim mcp` runs a local [Model Context Protocol][mcp-spec] server over
STDIO. An AI agent host — [Claude Code][claude-code], [OpenCode][opencode],
or any [MCP][mcp-spec]-compatible client — connects to it over stdin/stdout
and gains structured access to Grimoire's catalog and install state without
running shell commands.

The server is **read-only by default**. The one mutating tool
(`grim_render`) is gated behind `--allow-writes`: without the flag it is
neither advertised nor callable.

The install **scope is chosen per tool call**, not at launch: every
scope-sensitive tool takes optional `global` / `config` / `workspace`
arguments (precedence in that order; all omitted means the project
discovered from the server's working directory — exactly the CLI default).
One server instance can answer questions about any scope.

> **Breaking change (v2):** `grim mcp --global` and `grim mcp --config`
> were removed and now exit `64` with a migration hint. Move the scope
> selection into the tool-call arguments instead.

Because stdout carries the [JSON-RPC][json-rpc] channel, the server writes
no diagnostic output there — all tracing goes to stderr. The server shuts
down when the client closes stdin (EOF).

| Flag | Effect |
|------|--------|
| `--allow-writes` | Register the write tool `grim_render`. Launch-pinned deliberately: enabling writes is a decision of whoever wires the server into a harness, never of the model calling the tools. |

**Tools exposed:**

| Tool | Description | Gate |
|------|-------------|------|
| `grim_search` | Browse/search the resolved scope's registries (no registry override — the configured set is the boundary). Args: `query?`, `refresh?`, scope. Same shape as `grim search --format json` (not byte-identical — see [MCP parity][json-mcp-parity]). | always |
| `grim_status` | Install status of every declared artifact in the requested scope. Args: scope. Same shape as `grim status --format json` (not byte-identical — see [MCP parity][json-mcp-parity]). | always |
| `grim_fetch` | Return an artifact's content in the tool result — no install. Canonical bytes by default; `vendor` (`claude`/`opencode`/`copilot`) returns that client's projection; `path` fetches one support file (base64 with `encoding: "base64"` for a binary file); a `files` listing is always included. `description` fetches the repository's [description companion](./publishing.md#description-companion) instead (every member inline); `digest_only` resolves to `{ref, digest}` with no download and composes with `description` to probe the companion tag. Content caps at 256 KiB (truncated content carries a marker); layers over 8 MiB are refused before download, and a registry that streams more bytes than its declared layer size aborts mid-transfer into a data error rather than buffering an unbounded body. Args: `ref`, `vendor?`, `path?`, `description?`, `digest_only?`, scope. | always |
| `grim_describe` | Report an artifact's manifest-level metadata — kind, curated annotations, tags, `has_description`, and the verbatim annotation map — without downloading its content. Same shape as `grim describe --format json`. Args: `ref`, scope. | always |
| `grim_render` | Write an artifact's vendor-native files into an arbitrary `dest_dir` (created if absent) — no install state, no client-config edits. Skill → `<dest_dir>/<name>/`, rule/agent → `<dest_dir>/<name>.md`. Args: `ref`, `vendor`, `dest_dir`, scope. | `--allow-writes` |

The scope arguments on each tool are `global` (boolean), `config` (explicit
`grimoire.toml` path), and `workspace` (directory to start the config
walk-up from). `grim_fetch`, `grim_describe`, and `grim_render` use them
only to decide which registries resolve the reference — none touches install
state.

**Registering with Claude Code** — add to `.mcp.json` in the project root
(or register globally via `claude mcp add`):

```json
{
  "mcpServers": {
    "grimoire": {
      "command": "grim",
      "args": ["mcp"]
    }
  }
}
```

Add `--allow-writes` to the `args` array to enable `grim_render`:

```json
{
  "mcpServers": {
    "grimoire": {
      "command": "grim",
      "args": ["mcp", "--allow-writes"]
    }
  }
}
```

Hand-authoring either block above is optional: grim's own server is also
published as the [MCP server artifact](./mcp-servers.md) `mcp/grim`, so
`grim add ghcr.io/grimoire-rs/mcp/grim:1` followed by `grim install`
registers the same entry — in every detected client, not just Claude Code
— without hand-editing any config file.

<!-- internal -->
[global-options]: #global-options
[options-tui]: ./configuration.md#options-tui
[json-mcp-parity]: ./json-interface.md#mcp-parity
[path-source-trust]: ./stability.md#limitations-path-source-trust

<!-- external -->
[git-config]: https://git-scm.com/docs/git-config
[vscode-compact]: https://code.visualstudio.com/docs/getstarted/userinterface
[mcp-spec]: https://spec.modelcontextprotocol.io/
[claude-code]: https://docs.anthropic.com/en/docs/claude-code
[opencode]: https://opencode.ai/
[json-rpc]: https://www.jsonrpc.org/specification
