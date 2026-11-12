---
title: "Command Reference"
description: "Every grim command follows the same shape: parse references into typed values, run the operation, then report the result."
---
<!-- doc_type: reference -->

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
| `--color <auto\|always\|never>` | When to colorize clap's help/error output and `--format json` (default `auto`): colorize only when stdout is a terminal, honoring `NO_COLOR`, `CLICOLOR`, `CLICOLOR_FORCE`, and `TERM=dumb` — see [environment variables](./configuration.md#environment-variables). `always` overrides every one of those signals, including `NO_COLOR`. |
| `-g`, `--global` | Operate on the global scope instead of the discovered project. |
| `--config <path>` | Use an explicit project config file. |
| `--registry <ref>` | Registry for short identifiers and the browse set. Repeatable / comma-separated (`--registry a,b`); the first value is the default. |
| `--offline` | Disable all network access; work from the cache only and fail rather than reach a registry. |
| `--progress <auto\|json\|none>` | Progress rendering for long-running passes (default `auto` = tty-gated stderr bar on `install`, `export plugin` and `export marketplace`, silent elsewhere; the two exports count every member fetched, across all plugins). `json` emits NDJSON events on **stderr** — `{"event":"start","total":N}`, `{"event":"advance","position":i,"total":N,"label":"…"}` (`label` is display-only), `{"event":"finish"}` — while stdout keeps the normal report. **Experimental pre-1.0**; see [Stability](./stability.md#unstable). |
| `--log-level <level>` | Set the tracing log level: `off`, `error`, `warn`, `info`, `debug` or `trace` (case-insensitive). Wins over [`GRIM_LOG`](./configuration.md#environment-variables). An invalid value prints one line on stderr naming the accepted levels, and the run continues at the `GRIM_LOG` or `warn` level. |

A downstream reader that closes the pipe early — `grim status --format
json | head` — makes grim exit `0` silently instead of erroring or
printing noise; see [Broken pipe][json-broken-pipe] for the full contract.

## The lifecycle commands

| Command | Purpose |
|---------|---------|
| [`grim init`](#init) | Create a fresh `grimoire.toml`. |
| [`grim config`](#config) | Read and write `grimoire.toml` settings and registries. |
| [`grim add`](#add) | Declare an artifact of any kind, lock it, and install it by default. |
| [`grim lock`](#lock) | Pin new declarations and keep existing pins; [`grim update`](#update) moves them. |
| [`grim install`](#install) | Materialize the locked artifacts into your AI client(s). |
| [`grim update`](#update) | Re-resolve floating tags and re-materialize changes. |
| [`grim status`](#status) | Report the state of every declared artifact. |
| [`grim why`](#why) | Show why an artifact is installed. |
| [`grim remove`](#remove) | Undeclare an artifact (config + lock only). |
| [`grim uninstall`](#uninstall) | Fully remove an artifact (files + record + config). |
| [`grim search`](#search) | Search the registry catalog. |
| [`grim rate`](#rate) | Vote on an artifact through the index's rating forge. |
| [`grim fetch`](#fetch) | Print an artifact's content without installing. |
| [`grim describe`](#describe) | Show an artifact's manifest metadata without downloading it. |
| [`grim export plugin`](#export-plugin) | Render locked artifacts as a plugin for a client without grim. |
| [`grim export marketplace`](#export-marketplace) | Regenerate a plugin marketplace repository from declared plugins. |
| [`grim tui`](#tui) | Browse the catalog interactively. |
| [`grim build`](#build) | Validate and pack a local artifact. |
| [`grim release`](#release) | Validate, pack, and push an artifact. |
| [`grim publish`](#publish) | Validate and batch-release all packages from a manifest. |
| [`grim login`](#login) | Authenticate to a registry and store the credential. |
| [`grim logout`](#logout) | Remove a stored registry credential. |
| [`grim schema`](#schema) | Print the JSON Schema for `grimoire.toml` or `publish.toml`. |
| [`grim completions`](#completions) | Print a shell completion script. |
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

`init` also seeds `[options].clients` with the
[clients](./concepts.md#clients) it detects for the scope, so the selection
is recorded once rather than re-derived on every run. Detecting nothing
writes no `[options]` table at all — the scope stays on autodetect and the
generic-client fallback remains a per-run decision.

An explicitly set `[options].clients` is what turns on two behaviours that
autodetect leaves off, so a seeded config gets both from the first run:
[`grim status`](#status) reports `clients_missing` / `clients_extra` against
it, and [`grim update`](#update) reaps the output of a client you later
remove from the list. Neither fires while the key is absent. Edit or unset
the key (`grim config unset options.clients`) to go back to autodetect.

```sh
grim init --registry ghcr.io/acme
```

## grim config {#config}

`grim config` reads and writes `grimoire.toml`, modeled on [`git config`][git-config]. Before it existed, querying a setting or scripting a config change required hand-editing TOML and relying on the next command run to catch typos.

The command covers two areas of the file: **settings** (the `[options]` and `[options.tui]` tables) and **named registries** (the `[[registries]]` array). Declarations remain under [`grim add`](#add) and [`grim remove`](#remove), which keep the lockfile in step with every change. Those are the `[skills]`, `[rules]`, `[agents]`, `[mcp]`, `[hooks]`, `[settings]` and `[bundles]` tables.

Scope follows the same rule as every config-aware command: without a flag, `grim config` discovers and edits the project `grimoire.toml` by walking up from the working directory; `--global` targets `$GRIM_HOME/grimoire.toml`; `--config <path>` selects an explicit project file.

Every write re-runs registry validation before touching the file, so the at-most-one-`default` constraint and alias rules always hold. The writer is shared with [`grim add`](#add) and [`grim remove`](#remove), and it **edits the file in place**: only the lines whose meaning changed move. Comments, blank lines and key order survive, and a new entry is appended at the end of its table.

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

`--all` additionally lists every supported key that is unset or at its default (plain: empty `Value` cell; JSON: `value` null, `set` false). Keys whose false/empty value collapses to unset — `options.show_deprecated`, `options.tui.group_by_type`, an empty `options.clients`, an empty `options.tui.tree_separators` — appear unset under `--all` even after being explicitly set to that value. Per-vendor rows follow the same rule as registry rows: `options.vendors.<name>.shared_skills` is listed only for a client the config's `[options.vendors]` actually names — `--all` widens a named entry to its unset row, it never enumerates every supported client. grim never writes an entry carrying only default values, but it leaves a hand-written one in place, so its `--all` row stays. Registry rows appear only for existing aliased `[[registries]]` entries; `registry.<alias>.default` always shows its effective value (no unset state — it is never omitted, `--all` or not); an alias-less `[[registries]]` entry is not dotted-key addressable and is never listed, `--all` or not. In plain output, an empty `Value` cell is ambiguous between unset and an explicit empty-string value (e.g. `options.default_registry` set to `""`) — the JSON `value`/`set` pair disambiguates.

`config list --format json` emits the same per-key metadata (type, title, description, default, enum values) for every row, set or unset — tooling such as editor extensions can drive a settings UI from it without hardcoding the key list.

The supported dotted keys are:

| Key | Value type | Notes |
|-----|------------|-------|
| `options.clients` | comma-separated client names, closed set | An unordered set of unique values drawn from the supported client names (see the [client compatibility matrix](./clients.md#matrix)) — e.g. `claude,opencode`. An unrecognized name or a repeated segment exits `65`; input order is otherwise preserved on store. Empty string clears the list. |
| `options.default_registry` | string | Legacy field — prefer `grim config registry use` for new configs. |
| `options.show_deprecated` | `true` or `false` | `false` is the default (deprecated artifacts are hidden from `grim search` and the TUI unless installed); setting it to `false` removes the key, so a subsequent `get` exits 1 (consistent with `list`, which omits default values). Seeds the initial state for both `grim search` and `grim tui`; the search `--show-deprecated` flag and the TUI `h` key override it per run. |
| `options.search_min_relevance` | integer `0`–`100` | How relevant a search result must be to be listed, as a percentage of the best hit's score, in [`grim search`](#search), the TUI search and the MCP `grim_search` tool. `50` is the default when unset; `0` lists every match. Values above `100` or non-integers exit `65`; an explicit value is always kept. |
| `options.tui.default_view` | `flat` or `tree` | Other values exit `65`. |
| `options.tui.group_by_type` | `true` or `false` | `false` is the default; setting it to `false` removes the key, so a subsequent `get` exits 1 (consistent with `list`, which omits default values). |
| `options.tui.tree_separators` | comma-separated single-character strings | Each character must be non-control and non-whitespace; other values exit `65`. |
| `options.tui.expand_levels` | non-negative integer | How many tree levels open expanded: `1` (default when unset) shows only registry roots, `0` opens fully expanded. Non-integer or negative values exit `65`. Setting it stores the value; a subsequent `get` echoes it (unlike the default-valued keys above, an explicit value is always kept). |
| `options.tui.sort` | `name`, `updated`, `rating` or `downloads` | The order [`grim tui`](#tui) opens in; other values exit `65`. Unset means the default kind-then-name grouping, which no listed value spells — so `list --all` reports `default: null` for this key, the one enum key with no fixed default. `--sort` overrides it per run. |
| `options.tui.sort_order` | `asc` or `desc` | The direction that order runs in; other values exit `65`. Unset means the order's own natural direction (which depends on `sort`), so `list --all` reports `default: null` here too. |
| `options.tui.theme` | a built-in theme name | The color theme of [`grim tui`](#tui); values outside the closed set (listed under [Choose a theme](./browse.md#themes)) exit `65`. `terminal` is the default, so `list --all` reports it as the default. `--theme` overrides it per run. |
| `options.vendors.<name>.shared_skills` | `true` or `false` | One key per client — `<name>` must be a supported client, else the key itself is unknown and exits `64`. `true` installs that client's skills into the cross-vendor `.agents/skills` pool instead of its own directory, and is accepted only for a client that reads that pool — enabling it elsewhere exits `65`. `false` keeps the client's own directory even beside a client that writes the pool; it is stored, so a subsequent `get` prints `false`. Unset means automatic: a pool-reading client joins the pool only beside a pool writer, so `list --all` reports `default: null` for this key, and `unset` returns the client to automatic. Changing the value moves the client's skills on the next `install`/`update`; a hand-edited old copy is kept and warned about, never deleted. See [`[options.vendors]`](./configuration.md#options-vendors). |
| `registry.<alias>.oci` | string | The registry entry must already exist. Mutually exclusive with `index` (setting it on an index entry exits `65`); unsettable only when `index` is set — else use `grim config registry rm <alias>`. The pre-0.7.0 field name `url` is accepted as an alias. |
| `registry.<alias>.index` | string | A [package-index](./package-index.md) locator (`http(s)://` base or git repository). Mutually exclusive with `oci` (same rules mirrored); a locator matching neither transport exits `65`. |
| `registry.<alias>.default` | `true` or `false` | Setting to `true` clears all other entries' `default` flag, the same as `grim config registry use`. |
| `registry.<alias>.include` | one glob pattern | A [browse filter](./configuration.md#browse-filters) narrowing what this registry shows in `grim search`, the TUI, and `grim_search`. `set` stores **exactly one** pattern, replacing the whole list — a comma is glob alternation syntax, never a separator, so nothing is split on one; a pattern carrying a comma outside a `{…}` group is stored verbatim and warned about. `set` on an entry that already carries more than one pattern **discards the rest** — exit `0`, with a warning naming the count. Write several patterns with repeated `--include` flags on `grim config registry add` (new entry) or `grim config registry set` (existing one), or by hand. A pattern that is empty, whitespace-only, carries a control character, exceeds 1024 bytes, nests `{` more than 32 levels deep, or fails to compile exits `65` — the same set a config-file read rejects with `78`, since both compile the pattern the way the browse filter itself is built. A sixth cap bounds the whole list rather than one pattern — 64 KiB summed as compiled — reachable only through `registry add`/`set`'s accumulated flags (exit `65`) or a hand-edited file (exit `78`); a single `set` call writes one pattern and can never trip it alone. `unset` clears the list — `grim config registry set <alias> --clear-include` is the equally supported second clearing route; `get` on an empty list exits `1`. |
| `registry.<alias>.exclude` | one glob pattern | Same shape and rules as `include`, hiding matching repositories instead. Combines with `include` on the same entry and wins wherever both match. |

Registry dotted keys require the entry to already exist — only `grim config registry add` creates entries. Passing `registry.<alias>` without a trailing field to `unset` removes the whole entry, equivalent to `grim config registry rm <alias>`.

### Registry lifecycle {#config-registry}

`grim config registry` manages the `[[registries]]` array through dedicated lifecycle verbs:

```sh
grim config registry add  acme --oci ghcr.io/acme
grim config registry add  acme --oci ghcr.io/acme --default
grim config registry add  hub  --index https://index.grimoire.rs
grim config registry add  acme --oci ghcr.io/acme \
  --include 'acme/platform/**' --include 'acme/tools/**' \
  --exclude 'acme/platform/legacy/**'
grim config registry set  acme --include 'acme/platform/**' \
  --include 'acme/tools/**'        # edit in place; unnamed fields keep their value
grim config registry set  acme --index https://index.acme.internal
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
exits `64` — edit it with `grim config registry set <alias>` instead.

`--include` / `--exclude` seed the entry's
[browse filter](./configuration.md#browse-filters). Both are repeatable and
accumulate; neither is ever comma-split, because a comma is glob alternation
syntax — `--include 'acme/{platform,tools}/**'` is one pattern, not two.
These flags — on `add` for a new entry, on `set` for an existing one — are
the only CLI path that writes a multi-pattern list: `grim config set
registry.<alias>.include` replaces the list with exactly one pattern. Both
flags are inert on an alias that already exists, since `add` rejects a
duplicate with exit `64`.

`registry set` edits an existing entry **in place**. It takes the same
`--oci` / `--index` / `--include` / `--exclude` / `--default` / `--insecure`
flags as `add`, and applies only the ones given: an omitted flag leaves that
field exactly as it was. A repeatable list flag replaces that whole list, so this is how a
browse filter grows past one pattern after the entry exists. `--oci` and
`--index` swap the entry's kind, clearing the other side. `--default` sets
the flag and clears every other entry's, like `registry use`; it cannot
*unset* one, since the default has to live somewhere — move it by naming
another entry. An alias that does not exist exits `64`, as does a `set`
naming no field at all.

`--insecure` contacts the registry over plain HTTP instead of HTTPS — for a
local or in-cluster registry without TLS. Unlike `--default` it is
revocable in place, with the `--no-insecure` half of the pair; `set` also
accepts `--insecure` alone, and omitting both leaves the current setting.
It applies to `--oci` entries only: pairing it with `--index` exits `65`,
since an index locator already carries its own scheme. Full semantics,
including how it combines with `GRIM_INSECURE_REGISTRIES` and what a
committed `grimoire.toml` implies for collaborators:
[Plain-HTTP registries](./configuration.md#plain-http-registries).

`--clear-include` and `--clear-exclude` are the two flags `add` does not
have. A list flag given zero times means "leave this field alone", so
emptying a list needs its own flag rather than a spelling of "no patterns":

```sh
grim config registry set acme --clear-include                 # unfilter the include side
grim config registry set acme --clear-exclude --oci ghcr.io/acme2   # combine with any other flag
```

Each conflicts with its own list flag — `--clear-include --include 'a/**'`
exits `64` — and neither conflicts with the other side, so a single call can
clear one list and rewrite the other. A clear is **silent** at every list
length, including an already-empty one: exit `0`, the entry otherwise
untouched, no warning. (Nothing else in the file moves either: every
`grim config` write edits it in place, as [above](#config).) It is not `config set`'s single-pattern replacement, which does warn
when it discards patterns; a clear says what it does in its own flag name.
A clear removes the list's key, so the entry reads exactly like one that was
never filtered. An already-empty `include = []` is left as written.

`grim config unset registry.<alias>.include` is the second, equally supported
clearing route, through the dotted-key surface. The two differ only in what
they report — see [JSON output](#config-json) below.

Unlike the `rm` + re-`add` round-trip it replaces, `set` keeps the entry's
position in `[[registries]]`. That matters: when no entry declares `default`,
the *first* one wins, so re-creating an entry could silently move the default.

`registry use` is the correct way to change the default registry. It sets the target entry's `default` flag and clears the flag on all others in one atomic write. Dotted `grim config set registry.<alias>.default true` routes through the same logic.

`registry list` shows all `[[registries]]` entries in the scope. Entries without an alias (locator-only entries hand-authored before aliases were introduced) appear with an empty `Alias` cell and are **not addressable by dotted key** — assign them an alias to manage them with `grim config`.

`registry fields` lists the 6 addressable per-registry field names (`oci`, `index`, `default`, `include`, `exclude`, `insecure`) with their type, title, and description — the same static metadata the `registry.<alias>.<field>` dotted-key table above documents, in machine-readable form. It reads no config and resolves no scope: unlike every other `config` subcommand it works in a directory with no `grimoire.toml`, and with no `--global`/`--config` flag needed. New fields are **appended** to that list, never inserted, so a consumer indexing the rows positionally keeps working.

### JSON output {#config-json}

Add `--format json` to any subcommand for machine-readable output (full
cross-command contract: [the JSON interface](./json-interface.md)). The
shapes are:

| Subcommand | JSON shape |
|-----------|------------|
| `get` (value set) | `{"key":"…","value":"…","set":true,"scope":"project"\|"global"}` |
| `get` (unset, exits 1) | `{"key":"…","value":null,"set":false,"scope":"project"\|"global"}` |
| `set` / `unset` / `registry add`, `rm`, `use` | `{"action":"…","key":"…","value":string or null,"scope":"…","dry_run":bool,"fields":[]}` — `dry_run` is `true` only for `set --dry-run`; every other write verb always reports `false`. `fields` is always present and always `[]` on these five verbs |
| `registry set` | the same object, with `fields` carrying one row per field the call wrote: `{"field":"oci"\|"index"\|"default"\|"include"\|"exclude"\|"insecure","action":"set","value":…}` or `{"field":"…","action":"cleared"}` (no `value` key on a cleared row). Rows follow `RegistryField::ALL` order — `oci, index, default, include, exclude, insecure` — so two calls touching the same fields emit byte-identical arrays. `--no-insecure` is a `set` to `false`, never a `cleared`. `value` is the locator the call **named** with `--oci`/`--index`, and `null` when neither flag was given — it echoes the flag, never a comparison against the stored entry |
| `list` | `{"items": [...]}` of `{"key":"…","value":string or null,"set":bool,"type":"…","title":"…","description":"…","default":string or null,"values":[…] or null,"constraints":{"item_pattern":"…","item_width":integer} or null,"section":"…","advanced":bool}` |
| `registry list` | `{"items": [...]}` of `{"alias":string or null,"oci":string or null,"index":string or null,"include":[…],"exclude":[…],"default":bool,"insecure":bool}` |
| `registry show` | `{"alias":"…","oci":string or null,"index":string or null,"include":[…],"exclude":[…],"default":bool,"insecure":bool}` |
| `registry fields` | `{"items": [...]}` of `{"key":"…","type":"…","title":"…","description":"…"}` — `key` is the short field name (`oci`, `index`, `default`, `include`, `exclude`, `insecure`), not a dotted key; no `value`/`set`/`default` (meaningless for a field pattern) |

`list` rows carry all eleven fields whether or not `--all` was passed — the flag only widens the row set, never the row shape. `value` is `null` only for an unset row (surfaced only under `--all`); `set` is `value != null`. `type` is one of `string`, `boolean`, `integer`, `enum`, `string-list`, `string-set`; `values` is non-null for `enum` and `string-set` keys (the allowed value set), `null` otherwise. `title` and `description` are fixed per-key metadata, not derived from the current value; `default` is the runtime default in CLI string form, or `null` when the key has no fixed default. `constraints` is non-null only for a list key whose items carry a shape rule beyond closed-set membership — today just `options.tui.tree_separators` — and is advisory: `item_pattern` is necessary but not sufficient (it cannot express the paired `item_width` rule), so `grim`'s own validation stays authoritative even when a value matches the pattern; see [the JSON interface](./json-interface.md#shapes-items) for the full honesty contract. `section` and `advanced` group keys for a settings UI, with the same page defining both.

Registry rows always carry both locator keys — exactly one of `oci` /
`index` is non-null for a valid entry — and both
[browse-filter](./configuration.md#browse-filters) keys: `include` and
`exclude` are always present arrays, `[]` on an unfiltered entry, never an
absent key. The plain `registry list` / `registry show` tables report the
filter as a `Filters` cell carrying **counts** (`2 include, 1 exclude`, or
`—` when unfiltered); the patterns themselves are read from `--format json`,
which has no width to lose. `insecure` is likewise always present, `false`
on an entry that never opted in, and has its own plain-table column.

The `action` field in write confirmations takes one of: `set`, `unset`, `registry-added`, `registry-removed`, `registry-default`, `registry-set`. The `scope` field is `project` or `global`.

`fields` reports **the write, not the invocation**. A field named with the
value it already held still emits its row — element presence means "this
field was written", never "this field changed" — and a `--oci`/`--index` kind
swap emits *two* rows, the named side `set` and the unnamed side `cleared`,
because the command really did perform both mutations. The unnamed locator
side is the one exception to "not a diff": it emits `cleared` only when it
actually held a value, since reporting a locator cleared where none existed
would be a phantom write. `fields` is also scoped to the entry named by
`key`: `--default` demotes whichever sibling entry held the flag, and that
sibling's change is **not** reported here — no row ever names another alias.

**Three CLI surfaces reach the same browse-filter field**, and their reports
differ. Two of them clear a list, and both clearing routes are supported —
neither is deprecated in favour of the other:

| Call | `action` | `fields` | Warns? |
|---|---|---|---|
| `registry set acme --include 'a/**'` | `registry-set` | `[{"field":"include","action":"set","value":["a/**"]}]` | no |
| `registry set acme --clear-include` | `registry-set` | `[{"field":"include","action":"cleared"}]` | no |
| `config set registry.acme.include 'a/**'` | `set` | `[]` | **yes**, when it discards patterns |
| `config unset registry.acme.include` | `unset` | `[]` | no |

`config set` is the narrow one: it writes exactly one pattern and replaces
the whole list, so on an entry already carrying more than one it warns naming
the discarded count (exit stays `0`). A clear through either of its two
routes is silent at every list length. The asymmetry is intended —
`fields` is scoped to `registry set` — but a consumer diffing the routes has
to be able to read it somewhere rather than discover it.

The plain one-row table renders `fields` into its `Value` cell, segments
joined with `", "` in the same order: `{field}={locator}` for a locator,
`default={true|false}`, `{field}={count}` for a list (the count, not the
patterns), and `{field} cleared` for a cleared row. On the five other write
verbs `fields` is `[]` and the cell keeps its previous meaning, the `value`
string.

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

`grim add [--kind <skill|rule|agent|bundle|mcp|hook|settings>] [--name <name>] [--no-install] [--force] [--trust-workspace | --no-trust-workspace] <reference>`
declares a skill, rule, [agent](./agents.md), [MCP server](./mcp-servers.md),
bundle, or (experimental) [hook](./hooks.md), pins it in the lock, and — by default — materializes it into your
detected AI clients in one step. `<reference>` is the only required argument —
`registry/repo:tag` or `registry/repo@sha256:…`.

A [settings package][settings] is the experimental `settings` kind, behind the
`[options.experimental] settings` flag since 0.16.0. Its contract is
[unstable][stability-unstable].

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

Install-on-add resolves its client targets exactly like
[`grim install`](#install), including the generic-client fallback — so
`grim add` of a rule, agent, or MCP server in a workspace where no client is
detected exits **78**: the generic `agents` client renders skills only, and
there is nowhere to put what you just asked for. The declaration and the lock
entry are still written, so selecting a client (`[options].clients`, or
`grim install --client <name>`) completes the install without re-adding.
`grim add` has no `--client` flag of its own; use `--no-install` when you mean
to declare now and choose clients later.

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

`--force` on `add` covers the artifact you typed, or a bundle's members
(nested bundles' included), and nothing else. A member you also declare
directly is yours, not the bundle's, so `--force` on the bundle leaves it alone. You consented to that artifact's files, not to the files of the
[dependencies](./concepts.md#dependencies) it brings.

If one of those would
overwrite a file grim did not create, `add` still refuses with exit 65 and the
reason `untracked-destination`. The message names every such dependency
destination, so one rerun can fix them all. Run
[`grim install --force`](#install) to overwrite them deliberately. A settings
value a dependency wants is named the same way, and the message then starts:

<!-- doc-norun: an error line grim prints, not a command -->
```text
dependencies set or changed outside grim were not overwritten: settings 'team' /model in .claude/settings.local.json; --force on add covers the added artifact only, run `grim install --force` to overwrite them
```
<!-- /doc-norun -->

`--force` **never bypasses path containment.** A refusal carrying the
[`anchor-escape`](./json-interface.md#error-reason) reason — a recorded path
that resolves outside the anchor root it was stored against, through a
symlinked final component — is refused with or without `--force`. Recover
with `grim uninstall <kind> <name>` followed by a fresh install; files may
remain on disk and must be removed manually. Under `--format json` the
forceable refusals are marked with
[`"forceable": true`](./json-interface.md#the-optional-forceable-field) and
`anchor-escape` is not, so a consumer must key on that field rather than on
the shared exit code 65.

Re-running `add` on a binding that already exists **re-pins** it when the
reference changes, as long as it names the same repository.
`grim add acme/code-review:2` after `grim add acme/code-review:1` switches the
declaration, the lock, and the installed files to version 2. Pinning a floating
tag to a digest works the same way, and so does going from a digest-only
reference back to a tag. Dropping the digest from `:1@sha256:…` back to `:1`
keeps that digest until [`grim update <name>`](#update).

A new dependency can ask for another version of a directly declared artifact.
The declaration's pin holds, and `add` warns once per held artifact. See
[a pin that holds back a request](#lock-held-back).

Re-declaring the exact same reference moves nothing, even after its tag moved.
The report still reads `added`, and one stderr line says the pin was kept. It
names `grim update -- <name>`, which moves the pin or says why it cannot.

The declared name is still a unique key per kind, so a *different repository*
— another registry host, another repository path, or a [local path
source](#add-path) where a registry reference was declared — refuses
(exit 64) instead of silently taking over the binding, and names the existing
reference in the error. Pass `--name` to bind that reference under a different
name.

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

Adding an artifact that declares [dependencies](./concepts.md#dependencies)
locks and installs the whole closure, and declares only the artifact you
typed. A dependency that is already locked but missing on disk is installed too.
Plain output adds one stderr line, `pulled in N dependencies: <kind>/<name>, …`,
naming the dependencies this add brought into the workspace, and
`--format json` carries the same list in
[`pulled_in`](./json-interface.md#shapes-single).

`add` keeps every other pin, even after a tag moved, and re-packs every
[local path source](#add-path). It can still move a pin it shares. When a new
dependency edge disagrees with a kept pin, grim releases that pin and resolves
it again, and [`moved`](./json-interface.md#shapes-single) lists it. Plain
output adds one stderr line naming each pin it moved. If the claims still
disagree, `add` exits `78`.

Some locks hold no member list for a declared bundle: grim 0.3 or earlier wrote
them, or a hand edit cut their `[[bundle]]` rows. There a new artifact that
requires a member can move that member's pin when its tag moved, and `moved`
lists it. Run `grim lock` before the add to keep the pin.

When the lock already holds a pin that answers the reference you typed, `add`
keeps it. That pin may be a dependency, a bundle member or a line you typed back.
One stderr note names `grim update -- <name>`, with the run's `--config` path, or
`grim --global update -- <name>` in global scope. It says that command moves the pin to the digest its tag names today, or says why it cannot. If that tag is gone, it offers to declare a reference that exists. Where only the manifest its tag names
refuses, the note says that manifest does not resolve and that the command says why.
When an artifact target's pin does move, `moved` lists it too. A
moved `[[bundle]]` pin has no row there, so it gets one stderr line instead:
`bundle '<name>' moved its pin from <old> to <new>`.

On a lock this grim wrote, typing the line and running [`grim lock`](#lock) keeps
the pin too. On a lock grim 0.15.1 wrote, no pin records a tag. A hand edit
there, a typed line or a deleted one, leaves the lock stale. The relock then
resolves every such pin once, so each follows a moved tag, and one warning names
them and says which moved. A `grim lock` or `grim add` run before the edit
records the tags and keeps those pins.

`grim add` still keeps the pin there. That holds at exit `0` even where the dependencies of a moved tag's manifest conflict. It also holds where they form a cycle, back at the pinned digest or at another one. The add also keeps the pin where a tag below that
manifest is gone, and `grim update -- <name>` then refuses and says why.
An unreachable registry, a refused credential, a timeout or a graph cap below the
manifest keeps its exit (`69`, `80`, `69`, `65`). The relock after a hand edit exits
`78`, `65` or `79` there and writes nothing.

An edge 0.15.1 never recorded can ask for a bundle member at another tag. The add
of that member then reads the tag you typed and what its manifest asks for. A gone
tag you typed refuses the add with `79`. A gone tag below that manifest does not,
and `grim lock` keeps the pin there too.

An add can also take a pin out of the lock, as when a re-declared reference no
longer requires a dependency. [`dropped`](./json-interface.md#shapes-single) lists
each one at the pin it held. Plain output adds one stderr line, and the files stay.
[`grim uninstall <kind> <name>`](#uninstall) removes them. A bare
[`grim update`](#update) prunes them too, but moves every pin.

Adding is also the consent gesture for a [hook](./hooks.md) you add. It consents
only to the hooks this add brings: the one you typed and any new hook it
requires. A hook that was already in the workspace stays as unconsented as it
was, including when a bundle you add also lists it.

The same holds for a settings package, with its flag on. An add records
[consent][settings-consent] by the rules below, and for no package already in
the workspace. `--no-trust-workspace` records nothing. With the flag on and
settings in the lock, `add` checks for a [conflict][settings-conflicts] before
it saves the lock. A conflict exits `78`, and the config and lock stay as they
were.

`add` records that consent ahead of its install, and only for hooks and
settings packages it read from a manifest this run resolved. One counts only
behind edges this run resolved, each settled in the repository it names. One
behind a kept pin, or behind a declaration that shadowed the edge, stays
unconsented.

Offline an edge counts only when the manifest your local store holds for its
parent lists it. An edge below a local path you added counts too, since this run
re-packed it. A hook behind an edge your store cannot confirm needs
[`grim hook allow`](#hook).

Unless you pass `--no-trust-workspace`, an add never withdraws consent from a
hook the record covers, so those hooks stay armed. A hook the add brings but
does not consent to stays gated. It is [drift](./hooks.md#consent) until
[`grim hook allow`](#hook) covers it.

An `add` whose install fails keeps its declaration, its lock and that consent.
Once you clear the failure, `grim install` or a retried `add` arms the hook with
no `grim hook allow`, and no hook the record covers disarms in between. A relock
that fails rolls the declaration back and records nothing. `--no-install`
records the same hook consent, since it declares and locks the same way. It
records no settings package, which the next `grim install` asks about.

Re-adding a hook or settings package whose pin the lock kept consents to it only
while that pin's recorded tag still names its digest. A pin with no tag
consents while its repository serves the digest.

That read needs the network and gets 10 seconds. If the tag moved, one warning
says the tag names another digest than the pin the add kept. It names
`grim update <name>`, which moves the pin or says why it cannot, and then
`grim hook allow`, or `grim consent allow` for a settings package, which takes the
lock as it stands. With no answer it stays
gated, one warning says so, and the exit stays `0`.

Adding a [bundle](./concepts.md#bundles) declares it in `[bundles]` and expands
its members into the lock. `grim remove bundle <name>` undeclares the bundle and
drops the members it contributed. A member another still-declared bundle also
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

### Exit codes {#add-exit-codes}

| Situation | Code |
|-----------|------|
| Success | `0` |
| A reference naming another repository for an existing binding, an invalid binding name, or a local-path bundle | `64` |
| A local modification or an untracked destination, or a dependency [cycle](./concepts.md#dependency-cycles) or [limit](./concepts.md#dependency-limits) | `65` |
| Concurrent write on the config file's lock | `75` |
| Sources claiming different versions of one artifact, or no client that can host it | `78` |
| A [lock file grim cannot read](#lock-unreadable) | `78`, `77` or `74` |
| A tag that does not exist, or a [kept pin whose manifest is gone](#lock-exit-codes) | `79` |

A resolve that fails restores `grimoire.toml`, and an unreadable lock refuses
before the config is written, so neither leaves a declaration behind. Every other
failure, such as an unreachable registry or a rejected credential, keeps the
classification [`grim export plugin`](#export-plugin-exit-codes) documents.

## grim lock {#lock}

Brings `grimoire.lock` in line with `grimoire.toml`. A new declaration, or a
reference you changed, resolves to a concrete digest, and one you removed leaves
the lock. Run it after editing the config by hand. `grim add` already locks what
it declares.

Every other pin stays, even after its tag moved on the registry. A one-line
config edit should not change what the rest of the team installs, so only
[`grim update`](#update) moves those pins. Each report row reads `locked` for a
new or moved pin, `unchanged` for a kept one, and `dropped` for one that left the
lock. A bundle has no row, so a stderr line names each bundle pin the run moved,
`bundle '<name>' moved its pin from <old> to <new>`.

The lock covers the whole [dependency](./concepts.md#dependencies) closure: each
declared artifact's dependencies, and theirs in turn, get their own entries.
With `--offline`, a dependency list missing from the local store is carried over
from the previous lock with a warning. Where the lock records none, what only
that artifact requires leaves the lock, as on 0.15.1. See
[offline and the store](./concepts.md#dependency-offline).

With the `settings` flag on and a [settings package][settings] in the lock,
`lock` also checks the packages it would apply before it writes the lock. A
[conflict][settings-conflicts] among them exits `78`, and a damaged install
state file exits `74`. Neither writes the lock.

### Which pins stay {#lock-pins}

A pin stays while the reference it records still answers the request. That means
the same registry and repository, the same tag, and the same digest when the
request names one. Dropping the digest from `:1@sha256:…` back to `:1` keeps that
digest until [`grim update <name>`](#update).

A dependency no declaration holds can have several requests and only one pin. The
lock's recorded edges hold the other requests. While the lock's hash matches the
declaration, every recorded edge answers, because the resolve that wrote the lock
settled them on one digest. So the pin stays after any of those tags moved, in
any repository, with no tag resolve.

Five cases resolve anyway:

- A [local path source](#add-path) has no tag to keep, so `lock` re-packs it and
  a changed file reaches the lock.
- A kept pin that disagrees with a new dependency edge is released and resolved
  again, so a moved tag cannot fake a conflict. A real disagreement still exits
  `78`. An edge the lock did not record answers only in the pin's own repository,
  so a pin is never carried into a new one.
- A pin with no recorded tag, from a lock grim 0.15.1 or earlier wrote, stays on
  a clean lock. The first write only stamps each tag onto its pin, with no tag
  resolve, and the rows read `unchanged`. That holds with no recorded edge too,
  and a bundle with no `[[bundle]]` row costs one tag read. On a stale lock, or
  when a moved parent or a new key asks for something else, the pin resolves
  once. A warning names it and the cause, as the
  [upgrade note](./upgrading.md#lock-keeps-pins) describes.
- Where the parent has an entry, an edge counts only if that entry records it,
  and 0.15.1 records no dependent's edges. A pin only such an unrecorded edge
  asks for settles as a fresh resolve does: it can move, or exits `78` on a
  conflict.
- On a stale lock a pin that names a recorded edge answers every recorded edge.
  It stays at exit `0` even where a fresh resolve would exit `78`, and
  [`grim update`](#update) still exits `78` on that conflict. A pin that names no
  edge vouches for none, so a surviving edge resolves once and can move it. One
  warning names each pin that moved this way.

On a lock whose hash matches `grimoire.toml`, a declared bundle or artifact with
no entry in the lock is read at its current tag. Only a lock grim 0.3 or earlier
wrote, or one cut by hand, lacks such an entry. If that tag moved, the stamp can
pair a member's new tag with its old digest.
[`grim status --check`](#status-check) reports that as an update, and
[`grim update`](#update) repairs it.

Deleting a direct line by hand leaves a stale hash and nothing else, so `grim lock`
reads the key as an honest diamond. It keeps the pin and its tag at exit `0`, even
where a fresh resolve exits `78`. A later declaration of the hidden tag resolves,
and `grim update` exits `78` naming the conflict, which is the signal.
[`grim status --check`](#status-check) re-resolves the pin's own tag while an edge
names it. A hidden edge at a tag the pin never held is not asked, so the row reads
`false` until the pin's own tag moves.

`grim lock` no longer re-checks an existing pin against its tag. It still reads
each kept pin's manifest by digest, local store first, to repair dependency
edges. A hand-edited digest under the same repository and tag therefore survives
it, and for a bundle that digest selects the member list. For a checkout you do
not trust, run [`grim update`](#update) or delete the lock.
[`grim status --check`](#status-check) reports a pin that differs from its tag.

### A pin that holds back a request {#lock-held-back}

A direct declaration's pin wins over what a dependency asks for. Suppose a
parent that is new to the lock, or moved in this run, requests a reference. A
local (path) parent counts when it is new or its content changed.

If that reference resolves to another digest than the held pin, the run still
exits `0` and writes the same lock. It prints one warning per held key on
stderr:

```text
skill/x is requested by skill/p as ghcr.io/acme/x:2, but your declaration holds it at <pin>; run `grim update -- x` to re-read the declared tag, or declare the requested reference
```

A key several new parents ask for gets one warning that names all of them.
`grim add`, `grim update` and `grim lock` print it. A declaration pinned by
digest gets a shorter text. It ends in `declare the requested reference to take
it`, because re-reading a tag cannot move a digest.

Four cases print nothing: a request that resolves to the held digest, one grim
cannot read, an offline run and a marketplace part run. None of them changes an
exit. The probe reads at most 64 distinct references and stops after 60 seconds
per run. The warnings it did not reach are dropped without a message. A standing
case from an earlier run is not repeated here:
[`grim status --check`](#status-check) lists it.

### Exit codes {#lock-exit-codes}

| Situation | Code |
|-----------|------|
| Success | `0` |
| A dependency [cycle](./concepts.md#dependency-cycles), a graph past the [limits](./concepts.md#dependency-limits), or an artifact that requires a feature this grim does not know (reason `unsupported-artifact-feature`) | `65` |
| Concurrent write on the config file's lock | `75` |
| Two sources want different versions of one artifact; the message lists them all and names the direct declaration that settles it, and `grim add --name <binding>` when the binding is not the repository's last segment | `78` |
| A [lock file grim cannot read](#lock-unreadable) | `78`, `77` or `74` |
| With the `settings` flag on, settings packages in the lock that [conflict][settings-conflicts] | `78` |
| With the `settings` flag on and settings in the lock, an install state file grim cannot read | `74` |
| A tag that does not exist, naming the artifact that required it | `79` |
| A kept pin whose manifest the registry dropped | `79` |

[`grim update`](#update) is the command that moves a pin whose request held, so
the last row refuses rather than resolve past it. The message names
`grim update -- <name>`, and bare `grim update` for when another pin is gone too.
If the tag is gone too, or a digest names the pin, it says to declare a
reference that exists and run `grim lock`. Each command carries the run's
`--global` or `--config`. A tag that no longer exists on a reference that names a digest
prints `manifest <digest> not found`.
Every other failure, such as an unreachable registry or a rejected credential,
keeps the classification
[`grim export plugin`](#export-plugin-exit-codes) documents.

### An unreadable lock {#lock-unreadable}

Without the previous lock there is nothing to keep, so a relock would move every
pin. `lock`, [`add`](#add) and [`update`](#update) refuse instead, and leave the
file as it was. Conflict markers, a too-large file or any other content grim
cannot parse exit `78`. So does a `grimoire.lock` that links to a file that is
not a lock, because grim reads through the link. A read that fails exits `77`
for permission denied and `74` otherwise.

Deleting the file gets the command running again, and it moves every pin. To keep
them, restore the file from git or resolve the conflict. A lock with a version
this grim does not know is the exception. The message asks you to upgrade grim,
because git only holds the same file.
[`grim remove`](#remove) and [`grim uninstall`](#uninstall) resolve nothing and
leave that lock as it is, so they warn once and exit `0`.

Every command that only reads the lock exits the same way as `lock`, as it always
did: `install`, `status`, `why` and a project `export`.

## grim install {#install}

Materializes every locked artifact into your AI clients' configuration
directories. `--client <list>` selects AI clients (comma-separated; any
supported client — see the [client compatibility matrix](./clients.md#matrix)),
overriding the config `clients` option. When
neither selects a client, the **detected** clients for the scope are
targeted — every client whose vendor directory or marker is present — plus,
in a project, the clients in the global `options.clients`. The run prints
`clients: <name> (<source>), …` on stderr, where the source is `--client`,
`project`, `detected`, `global` or `fallback`. A `--global` run labels the
clients of its own `options.clients` list `global`. The line is left out of
`--progress json` and `--progress none` runs.

When nothing is detected and no global list is set, the generic `agents` client is targeted instead: one
copy into the cross-vendor `.agents/skills` pool, rather than a copy into
every vendor directory grim knows about. That generic client renders skills
only, so an artifact set of nothing but rules, agents, and MCP servers has
nowhere to go — grim then exits **78** telling you to name a client with
`--client`. `--force` overwrites a locally modified artifact instead of
refusing it.

Install never clobbers files it did not create: a destination that already
exists on disk **without an install record** — a hand-authored skill
directory, a rule file, or an MCP config member owned by you or another
tool — is refused (exit 65) with the conflicting path named. `--force`
overwrites and records it. One exception: when the existing content is
identical to what the install would write, it is **adopted** into the
install record and reported `unchanged` — so deleting the state file while
leaving rendered files intact repairs itself on the next install.

A refused run exits `65` and names the first refused artifact in its error. One
stderr warning then names every other refused file artifact, uncapped, so one
rerun with `--force` covers them all. A refused settings package gets no second
mention, since its own warning already names each value.

A locked [hook](./hooks.md) is experimental. See
[Stability](./stability.md#unstable). It arms only past the
`[options.experimental] hooks` flag and the resolved workspace's consent
record. Everywhere else it is skipped, with a warning naming the cause, at
exit 0.

`--trust-workspace` / `--no-trust-workspace` (also on `add` and `update`)
answers the consent question for this one invocation. The flag writes no record,
though `add` still records [its own gesture](#add) unless `--no-trust-workspace`
is set. It outranks a stored consent record in both directions. See
[Control agent commands with hooks](./guides/agent-hooks.md) for a worked example.

A locked [settings package][settings] is experimental too, and applies only
past the `[options.experimental] settings` flag and [workspace
consent][settings-consent]. Since 0.16.0 the flag pair is named
`--trust-workspace` / `--no-trust-workspace`, and the hook names stay as
aliases. Either name answers for hooks and settings at once. Neither turns a
kind on, because its flag is checked first.

With the settings flag off, `install`, `update` and `add` warn once for each
locked settings package. A named `update` warns only for the packages in its
closure. The warning reads, for example:

<!-- doc-norun: a warning grim prints, not a command -->
```text
settings package 'team-defaults' not applied: settings are gated; enable them with `grim config set options.experimental.settings true`
```
<!-- /doc-norun -->

The command it names carries the run's `--global` or `--config`. `add --no-install`
installs nothing and so prints no warning. The exit code, JSON report, files
and `grim status` are the same as without the warning. A run that changes a
value reports it in the [change list][settings-changes], and
`install --force` overwrites a [refused value][settings-force].

Before any of that, install checks that the lock still describes the config
it was resolved from. `grimoire.lock` records a `declaration_hash` of the
declarations it was built from; a hand-edited `grimoire.toml`, or a merge
that brought in a declaration nobody locked, leaves that hash behind the
live config. Installing from a stale lock exits **65** having written
nothing at all — the refusal lands before the first artifact is touched:

```text
grimoire.lock is stale (declaration_hash sha256:4bbd… does not match current sha256:ce14…); run `grim lock` before installing
```

Under `--format json`
the run emits an [error document](./json-interface.md#error-document) and no
report.

This is the check to put in CI. [`grim status --check`](#status-check) sees
the same divergence and prints `stale` on every row, but it is a report,
not a gate: it exits `0` whatever it found. So the command that actually
fails a build when the lock and the config disagree is `grim install`, and
[`grim lock`](#lock) is what makes them agree again.

The neighbouring refusals differ in both code and cure: no lock at all is
exit **79**, ``no grimoire.lock found at <path>; run `grim lock` first``,
while a full [`grim update`](#update) re-resolves and rewrites the lock
rather than refusing it — only a partial `grim update <name>` refuses, at
exit `65` with its own message. A lock grim [cannot read](#lock-unreadable) exits
`78`, `77` or `74`.

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

The same refusal covers a name the lock installs through a bundle or a
dependency, in both directions. A dev-install of that name exits 64. So does
a `grim add` whose bundle members or dependencies include a dev-installed
name, with `grimoire.toml` left as it was. A `grim install` that would write
a locked artifact over a dev record also exits 64 and installs nothing.
Rerun it with `--force` to replace the local copy.

## grim update {#update}

`grim update [names…]` re-resolves floating tags, rolls the lock forward, and
re-materializes only what changed. It is the command that moves a pin whose
reference you did not change. With no names it updates everything, and binding
names [scope it](#update-names). Shares `--client` and `--force` with install.

```sh
grim update
grim update code-review rust-style
```

A moved parent can ask for another version of a directly declared
artifact. The declaration's pin holds, and `update` warns once per held
artifact, as [`lock`](#lock-held-back) does.

A row reads `updated` when its pin is new or moved, and `unchanged` when it held.
It reads `restored` when the pin held and this run wrote the files, with `old`
equal to `new`. That covers a deleted output coming back, a client's first copy
and a fresh checkout. It also covers a layout move, such as a pool reader joining
the shared pool.

An MCP registration grim adopts because the client's
config already holds the exact entry writes nothing, so it reads `unchanged`. A
hook whose client registration the run writes back reads `unchanged` too. A
bundle has no row, so a stderr line names each bundle pin the run moved, old to
new.

`update` runs the **same local-modification integrity gate as
[`grim install`](#install)**. A new pin overwrites machine-managed content
with no flag — that is the rolling-release contract and it is unchanged — but
an artifact whose on-disk bytes drifted from the recorded hash is **refused**
with exit `65` until you pass `--force`. One flag therefore governs every way
this command can destroy hand-edited work: the overwrite above, the prune
below, and the client reap below that.

> **Changed in 0.13.0.** `update` previously forced the overwrite
> unconditionally, so a hand-edited artifact was replaced silently at exit `0`
> while `grim install` refused the identical bytes. If you relied on that,
> add `--force`.

A [`.grimignore`](./artifacts.md#grimignore)d path is invisible to this
gate — it drifts without ever refusing the overwrite — because updating
still **replaces the whole installed directory** on a pin change. That
covers every path the publisher's `.grimignore` lists, not only the
defaults — a hand edit there is wiped without a refusal. A
hand-built `.venv/` or a script's `__pycache__/` is disposable: it survives
until the next `update`, then it is gone along with everything else that
was never part of the artifact's own tree.

Because update reconciles the workspace to the freshly-resolved lock, it also
**prunes** artifacts that have dropped out of the lock — most often a
[bundle](./concepts.md#bundles) member that the bundle stopped including. A
clean, unmodified orphan is deleted (files and install record) and reported with
the `removed` action. An orphan you have edited locally is **kept** and reported
as `kept-modified`, so an accidental bundle change never silently discards your
work. Re-run with `--force` to prune it anyway. This mirrors the install
integrity gate, where a locally modified artifact is refused rather than
overwritten without `--force`.

After a [named run](#update-names) the kept orphan has left the lock. The next
named run no longer sees it, and a stderr line says so.
`grim uninstall --force <kind> <name>` removes it. A bare `grim update --force`
does too, and moves every pin.

Narrowing the configured client set (`[options].clients`) is reconciled the
same way: when a client drops out of the set, `update` **reaps** the outputs it
no longer targets. A clean, unmodified output is deleted and the client is
listed under the row's `reaped_clients`. An output you have edited locally is
**kept** — the file stays, the client stays in the install record, and it is
reported under `kept_modified_clients` (with a stderr warning) rather than
silently discarded. `grim update --force` overrides that guard and deletes even
a locally-modified dropped-client output. Widening the set is symmetric: a
newly-configured client is materialized on the next `update`.

A dropped client's output is kept too when its config file cannot be read or
parsed, or sits behind a directory grim cannot search. It stays in the install
record. The file is never touched, `--force` included. A stderr warning
names the output.

An MCP entry grim [adopted](#install) rather than wrote is the user's own. A
reap leaves it in the config file, warns, and names it in the row's
`abandoned_entries`. The client is still listed under `reaped_clients`, and
the entry leaves the install record. `grim update --force` removes the entry
instead.

A new pin can also leave a client that stays selected unable to represent the
server. Its adopted entry stays in the same way, with `reaped_clients` empty.
An entry there that grim cannot read stays and is named too, with or without
`--force`. An entry you edited refuses the row, like any edited output, until
you pass `--force`. Then it stays in the file and is named as well.

This reap only fires against an **explicitly set** `[options].clients`. When
it is unset (the default, autodetect), `update` never reaps, and that holds
when the global `options.clients` joins the detected clients. The desired
client set would otherwise be re-derived from live client detection on every
run. A detection change (a client's marker directory removed, an environment
variable unexported) is not the same thing as the user narrowing their config.
Reaping on that basis would delete still-wanted output.

A [named run](#update-names) never reaps either: a dropped client's outputs wait
for a bare `grim update`.

Pruning and client-reaping happen only on `update`. `grim install` materializes
the current lock but never deletes — like [`grim remove`](#remove), it leaves
files on disk.

A bare `update` also refreshes every [local path source](#add-path): a declared
path dependency and a [dev-installed](#install-dev) record alike are
re-packed. A changed content hash re-materializes them, the local
equivalent of a floating registry tag rolling to a new digest. Before you
run it, [`grim status`](#status) surfaces that drift ahead of time as
`outdated`, the state of installed content that differs from its lock pin.

### Updating by name {#update-names}

Updating one artifact should leave the rest of the workspace alone. That includes
an unnamed artifact you edited or deleted, which a named run neither restores nor
refuses. A deleted or edited unnamed hook payload is not armed by a named run.
That hook alone loses its dispatch row, every other hook keeps arming, and one
[warning names it](./hooks-troubleshooting.md#payload-unarmable).
`grim install` restores a deleted payload, and `grim install --force` an edited
one. `grim update <names>` works on two sets: the pins it moves, and the
closure it installs.

A name moves that binding's own pin and nothing else it reaches. A dependency
keeps its pin even after its tag moved, unless the new version of its parent
changed the edge. A name may also be a bundle binding. The bundle moves, members
it added resolve, members it dropped leave, and a member whose reference it left
alone keeps its pin.

The closure is the names plus everything they pull in, read from the new lock:

| Pass | `grim update` | `grim update <names>` |
|------|---------------|-----------------------|
| Pins it moves | Every pin | The named bindings |
| Install and integrity gate, settings values included | The whole lock | The closure |
| `--force` overwrites, settings values included | Anywhere | Inside the closure and its prune |
| Prune | Whatever left the lock | What left the lock from the old closure |
| Client reap | Against an explicit `[options].clients` | Never |
| Path sources re-packed | All, dev-installed records included | The selected path entries, and dev-installed records inside the closure |
| Report rows | Every locked artifact and every pruned orphan | The closure, pruned orphans, and any other lock change |

So a modified dependency inside the closure still refuses with exit `65`, and an
edit outside it does not. An untouched binding gets no row at all.

Three things can still reach past the closure, and each gets a warning on stderr.
A pin nothing answers, such as one hand-edited to another tag, resolves once and
gets a row, but its files stay where they were. [`grim install`](#install)
applies that pin, and `grim uninstall <kind> <name>` removes the files of one
that left the lock. A bare `grim update` prunes those too, and moves every pin.
Hook consent covers the whole workspace, so a run that brings in a new hook also
gates hooks you did not name. One client hook registration serves every hook at
its event and matcher. Writing it for a hook in the closure also writes it for a
hook outside, and the warning names that hook.

Vendor config entries follow the closure instead. A named run writes a managed
entry, a `claudeMdExcludes` element or OpenCode's `instructions` glob, only for
a rule it names or reaches. It still removes the entry of an artifact it
removes. An entry you removed by hand for another rule stays removed until
`grim install`, `grim add`, a bare `grim update` or a TUI install writes it back.

Hook arming follows the closure too. A named run writes the client registration
and dispatch row of a hook it names or reaches, and leaves every other hook's as
it stands. A registration or row you removed by hand for another hook stays
removed, and a hook you only just allowed stays unarmed. Consent and payload
checks still cover every hook, so one outside the closure loses its arming when
either refuses it.

One registration is rewritten outside the closure anyway: a registration that
grim 0.15.0 wrote, which still runs the retired `bin/grim-hook` script.
That script cannot enforce a gatekeeper's verdict. A named run rewrites it when
the hook still has its dispatch row, and a warning names the hook. It leaves
the script itself for other workspaces until `grim install` or a bare
`grim update` deletes it.

`grim status` reads the dispatch table and not the client's file, so a hook whose
registration you removed still reads as installed. A named run warns about such
a gatekeeper outside its closure, and says nothing about an observer.

The warning names `grim install` to restore it. When a project run passed
`--trust-workspace` and the consent record alone does not arm that gatekeeper,
it names `grim install --trust-workspace`. A plain install would leave the
gatekeeper disarmed. The warning then also says that the flag arms every hook
and applies every settings package the workspace was not consented for.

The named install restores the registration even when it refuses another
artifact's local edit, so the restore needs no `--force`. When grim cannot parse
a file of that hook's own client, the warning names that file first. No run
registers there until you fix it.

`grim install`, `grim add`, a bare `grim update`, or a TUI install converges every
hook. `grim uninstall` and a TUI delete remove their own hook and leave every
other as it stands, except a registration that still runs the retired script.
Both warn about each gatekeeper they leave without a registration, with a named
run's remedy, and the TUI names it on the status line. Their remedy adds that a
stale lock needs `grim lock` first, since `grim install` refuses a stale lock
before it registers anything. Beside a note that the next `grim lock` may move a
pin, it sends a stale lock to that note's steps instead, because `grim install`
exits `65` until then. Beside a note with no steps, it says `grim lock` may resolve
the rows in that note afresh. Where the consent record misses a hook that the lock
or the config declares, the remedy names `grim hook allow` before the install. A plain install disarms
every hook. That command consents to every locked hook and settings package.

Settings values follow the closure as well. A named run writes, restores and
refuses only the values of a settings package it names or reaches. It moves only
that package's record to the new pin. A value of another package that you edited
or deleted leaves the run at exit `0`. `grim install` refuses the edit and
restores the deleted value.

Two settings checks still read the whole lock. A package in the closure that
[conflicts](./settings.md#conflicts) with one outside it exits `78` before the
lock is saved. A value that a package outside the closure also holds stays when
a package inside drops it.

So does a value that an outside package declares without holding it, if this run
could apply that package. The package inside then keeps the value and its
previous pin, and a warning names `grim install`, which hands the value over. Where
the declaring package's consent came from `--trust-workspace` alone, the warning
names `grim install --trust-workspace`.
An outside package the run may not apply, such as one the consent record does
not cover, keeps nothing. Both keeps hold for a package the run prunes too, such
as a member its bundle dropped.

When such a kept value was changed outside grim, a plain install refuses the
edit. The warning then says to delete that value or restore it to the value
grim wrote, then run `grim install`. That install still refuses every other edit
outside grim, and it writes no settings while it refuses any other settings value
at the scope.

The warning names `--force` as the other route, which overwrites every
edit outside grim at the scope. It also removes the edited values of packages
that left the lock. `grim install` takes no names that could narrow `--force`.

A pruned package's value that you edited refuses without `--force`, exit `65`.
The lock is saved by then, so the next named run no longer sees that package.
The refusal names `grim uninstall --force settings <name>`, which removes the value.

An install record the lock no longer holds, as after a run that crashed past its
lock write, is outside every closure. A warning names it.
`grim uninstall <kind> <name>` removes it. A bare `grim update` prunes it too,
moves every pin, and needs `--force` for one you edited. Every command a warning
names carries the run's `--global` or `--config <path>`.

A name that matches nothing exits `79`. So does a member that exists only under a
moved bundle tag, because the unselected bundle stays where it was. Run
`grim update <bundle>` instead. On a stale lock a named run exits `65`
(`stale-lock`) and asks for `grim lock` first. If `grim lock` then finds a pinned
manifest gone, bare `grim update` moves every pin.

With no lock at all, a named run warns, resolves and locks every declaration,
installs only the closure, and exits `0`. The warning names any name that matched
nothing.

### Exit codes {#update-exit-codes}

| Situation | Code |
|-----------|------|
| Success | `0` |
| A locally modified artifact, refused with `refused: true` on its row; the report still prints and the pin has already moved | `65` |
| A named run on a stale lock (`stale-lock`) | `65` |
| A dependency [cycle](./concepts.md#dependency-cycles), a [limit](./concepts.md#dependency-limits), or `unsupported-artifact-feature` | `65` |
| Concurrent write on the config file's lock | `75` |
| Sources claiming different versions of one artifact, which a moved tag [can cause](./concepts.md#bundle-conflicts) | `78` |
| A [lock file grim cannot read](#lock-unreadable) | `78`, `77` or `74` |
| A name that matches nothing, a tag that does not exist, or an unselected [pin whose manifest is gone](#lock-exit-codes) | `79` |

Every other failure, such as an unreachable registry or a rejected credential,
keeps the classification [`grim export plugin`](#export-plugin-exit-codes)
documents. `--marketplace` has [its own table](#update-marketplace-exit-codes).

### `--marketplace <PATH>` {#update-marketplace}

`grim update --marketplace marketplace.toml [selectors…]` rolls a
[declared plugin's](./configuration.md#marketplace-toml) pins forward, the
same way plain `update` rolls `grimoire.toml`'s. It re-resolves every
floating tag and writes a fresh `marketplace.lock`, but it **installs
nothing**. `--client` and `--force` both conflict with it — the flags
describe an installer this path never runs. It needs no `grimoire.toml` or
project scope either, so `--global`/`--config` conflict with it too.

```sh
grim update --marketplace marketplace.toml
grim update --marketplace marketplace.toml team
grim update --marketplace marketplace.toml team:code-review
```

With no selector, every declared plugin re-resolves. A bare `<plugin>`
selector scopes one plugin; `<plugin>:<member>` scopes one member inside
it, matched by lock name, mirroring plain `update <name>`'s partial
resolve. Every other pin in the plugin carries, and an include its lock entry
already holds reads no tag.

A bundle the plugin reaches carries too, at the digest its rows record in
`bundle_tag`. Only a run with no selector, a bare `<plugin>`, or a
`<plugin>:<bundle>` naming that bundle moves it. A pick of a bundle keeps a bundle
nested in it at the digest the plugin records and exits `0`. It also keeps the
bundle's members at their pins. The unreleased c5e3824 followed the moved tag of
the nested bundle or member and exited `78` on a conflict. Grim 0.15.1 exited `65`
on the nested bundle and `78` on the member. A pick that
names the nested bundle or the member still moves it. Where the plugin records no
digest for a nested bundle, a pick of the bundle that nests it moves it. Only such
a run records a digest. Offline, a recorded bundle the store lacks exits `81`. A
bundle whose manifest the registry dropped exits `79`.
On a plugin that records none, last resolved by grim 0.15.1, that run reads the
bundle tags it moves and records them. A `<plugin>:<bundle>` pick there records no
digest for a bundle it reaches through the one it names when a row carries that
bundle's stamp.

A `<plugin>:<bundle>` selector moves that bundle. Each row that carries the
bundle's stamp records its new digest, as a run on the whole plugin does. A member
whose reference the new list left alone keeps its pin where the plugin records its
tag. A row the plugin also includes directly keeps its pin. It records the digest
only where no other row carries that bundle's stamp. Grim 0.15.1 exited `79` on
that selector, and this grim still does on a plugin with no lock entry yet. The
same `79` holds for a bundle only a `requires` edge reaches, when 0.15.1 last
wrote the plugin and its `include` lists no bundle. A bare `<plugin>` moves that
bundle with the rest of the plugin.

A `<plugin>:<member>` run keeps every row outside its selection as the lock holds
it: pin, tag and `bundle_tag`. A member or bundle pick still reads every bundle
the plugin includes, at the digest the plugin records for it or else at its tag.
Where the plugin records no digest for an included bundle whose tag is gone, the
pick exits `79`. A version conflict among the entries they reach exits `78`, and
the message names the plugin include that settles it. That includes the moved
list of an unselected bundle that names an entry the plugin includes directly.

A pick also exits `78` and writes nothing where it would keep a dependent at its
pin beside another version of a row it requires. That happens when the
dependent's own tag moved onto that version. The run needs proof: the pin rule's
evidence, or a manifest or tag read that succeeded. A failed read refuses
nothing by itself and, with no other proof, keeps the earlier exit. For a
bundle, the part's own stamps can still prove the move: a row its tag brought
that the pick moved. A requirement the new lock lacks is held with the row that
needs it. An edge onto a key the plugin includes directly refuses nothing. The
row it requires can be a bundle too, judged on the bundle the run read for that
name, or else the one bundle of that repository the pick reaches. With none or
several, the earlier exit stays. So is each bundle that bundle's list nests at
any depth, read at the tag its edge names, under each binding the list names it
by. The rows such a list holds are judged too, unless the pick reaches a bundle
of that repository. A requirement tag republished onto the digest the pick moves
to still counts as held against what it named before.

The refusal names the narrow remedy first: add the dependent to the pick. It
then offers `grim update --marketplace <manifest> <plugin>`, and says that this
moves every pin of the plugin whose tag moved. The `79` text for a pinned
manifest the registry no longer holds says the same of its whole-plugin command.

A pick records no digest for a bundle it does not select, except one a bundle it
selects nests. That one records none while a row carries its stamp without a
digest. A part records one digest for each bundle pair, and a restored row's stamp
keeps no digest the run replaced.

A kept row with no tag in a plugin that records a digest gets a warning naming
the run that records its tag. The [upgrade note](./upgrading.md#lock-keeps-pins)
states this rule.

That partial resolve is refused at exit `65` (`stale-lock`) when
the plugin's lock entry is not already fresh. A stale part's `65` names the pick
`<plugin>:<name>` as its remedy, which relocks only that row and its dependencies. Repeated selectors merge
(`team:a team:b` picks both members). A selector naming an undeclared
plugin, or a member the plugin does not resolve to, exits `79`. On a plugin that
records the bundle's digest, that covers a member only a moved bundle tag would
bring, and a bare `<plugin>` brings that one. On a plugin that records none, it
covers a member the moved tag no longer lists, and the lock keeps that row.

A plugin declared with `project = "<dir>"` takes its pins from that
project's own `grimoire.lock`, so this command never rolls it forward. A
run without selectors leaves it alone, and a selector naming it exits `64`.
The message names `grim --config <dir>/grimoire.toml update`, which moves that
project's pins from anywhere. A `<dir>` outside the manifest's directory, or one
reached through a symbolic link, gets no command, only the reason.

### Exit codes {#update-marketplace-exit-codes}

| Situation | Code |
|-----------|------|
| Success | `0` |
| `--marketplace` combined with `--global`, `--config`, `--client`, or `--force`; a malformed selector (an empty plugin/member half, or more than one `:`); or a selector naming a `project` plugin | `64` |
| A missing or malformed marketplace manifest | `65` |
| Concurrent write on the manifest's lock file | `75` |
| A malformed `marketplace.lock` | `78` |
| A selector naming an undeclared plugin, or a member the plugin does not resolve to | `79` |
| A kept pin or a recorded bundle digest whose manifest neither the registry nor the local store holds, on a run that does not move it | `79` |
| A dependency graph that cannot be walked: a [cycle](./concepts.md#dependency-cycles), a [limit](./concepts.md#dependency-limits), or a feature this grim does not know (`unsupported-artifact-feature`) | `65` |
| An include's floating tag or a tag below an include needs the network under `--offline`, or a recorded bundle digest the local store lacks | `81` |

Every other failure propagates from the resolver with its existing
classification. That covers an unreachable registry, a rejected credential and
the `stale-lock` partial refusal named above. [`grim export
plugin`](#export-plugin-exit-codes) documents the same taxonomy in full.

Rerun it after editing `marketplace.toml`, or on a schedule, to keep a
plugin current between renders. A plugin you already shipped with
[`grim export plugin`](#export-plugin) picks up the rolled-forward pins
the next time someone re-exports it. This command is only the
roll-forward half of that workflow, never the render half.

The JSON report is [`update`'s own shape](./json-interface.md#shapes-items),
with one addition: every row gains an always-present `plugin` field. It is
`null` for a plain `update` run, and the plugin's name for every row this
flag produced. The plain table prefixes `Name` with `<plugin>:` instead of
adding a column.

## grim status {#status}

Reports each declared artifact's state in a `State` column, one of
`installed`, `outdated`, `modified`, `missing`, or `stale` — the five words
are defined under [artifact states](#artifact-states), alongside the
vocabularies the [TUI](#tui) and [`grim search`](#search) print, which
differ from this one. The `Source` column shows each artifact's
[provenance](./concepts.md#bundles): `direct` for a registry declaration, the
bundle it came from, `path: <path>` for a declared [local path
source](#add-path), `path: <path> (dev)` for a [dev-installed](#install-dev)
record, or `dependency: <kind>/<name>, …` for an artifact that only another
artifact's [dependencies](./concepts.md#dependencies) bring in. Pair with
`--format json` to drive automation.

Every row also carries `direct`, which says whether the artifact is declared in
this scope's `grimoire.toml`, and `required_by`, which lists the parents that
require it. A dependency of one artifact appears as its own row with
`direct: false`. `required_by` is `null`, and `source` reads plain `dependency`,
when the lock alone cannot work the parents out, for example before the first
`grim lock`. See [Why an artifact is installed](./json-interface.md#status-provenance)
for the exact contract, and [`grim why`](#why) for the chains behind a row.

Plain output also prints a `clients:` note on stderr, for example `clients:
claude (global), opencode (detected)`. It names the clients the report covers
and what selected each, in the words of [`grim install`](#install). Stdout and
`--format json` do not carry it.

A directly declared artifact may also be required by another artifact at the
same digest, with no artifact asking for another version of it. Plain output
then prints a note on stderr that the declaration can be removed. It stays safe to keep: a direct declaration wins over a dependency.

`--format json` output carries an `outputs` array per artifact: the
per-[client](./concepts.md#clients) locations the artifact was materialized
to, read back from install state. It is empty for a declared-but-not-installed
artifact. This is the supported way to script "where did grim put this file?"
— the on-disk layout under each client's directory is an implementation
detail and may change.

Each item also carries `clients_missing` /
`clients_extra`: the project's *explicitly configured* `[options].clients`
diffed against the artifact's recorded install-state clients, entirely from
local state (no network) — sorted arrays, `[]` when the two sets agree. When
`[options].clients` is unset (autodetect, with or without a global list), both
stay `[]` on every item instead: diffing against live client detection would report drift whenever
detection disagrees with what was recorded, which is not real config drift.
`clients_missing` also skips any configured client whose vendor cannot host
that artifact's kind at that scope — a Codex rule, say — since the install
was never going to record an output for it.

A settings row reports `clients_missing` as `[]`, as a bundle row does. An
install records nothing for a client the package declares no value for, so a
diff would never clear. For the same reason a settings row never reports a
newly selected client: `clients_missing` and `outputs_pending` both stay `[]`.
Run `grim install` after selecting one, and it applies what the package
declares for that client.

A separate array, `outputs_pending`, answers a different question: **what
would `grim install` write right now that the record does not already account
for?** Same `{client, path}` shape as `outputs`, `[]` when an install would
write nothing new. Two things put an entry in it, and both are real install
work:

- a **client that gained support since the last install** — you installed
  another AI client, or added one to `[options].clients`, and nothing was ever
  recorded for it;
- a **render-layout move**, reported at the new path.

A recorded output whose file was **deleted** out from under grim is not one of
them: `outputs_pending` reads the install record, not the filesystem, so that
entry reads `state: missing` instead.

This is *materialization drift*, and it is the one kind of drift no other
field could see: the artifact is byte-intact at the locked pin, so `state`
correctly reads `installed` while an install still has files to write. Unlike
`clients_missing` it is reported under autodetect too — it asks what an
install would do, not what you configured — and it is derived from the very
seam the installer uses to decide whether a pass is a no-op.

Remediation is **[`grim install`](#install)**. Not
[`grim update`](#update): that would also re-resolve floating tags and roll
your pins forward, which is a version change, not a repair. `outputs_pending`
never affects `state` and never affects the exit code.

A third array, `clients_unresolved`, names every active client whose recorded
output path could not be resolved at all — an anchor root absent on this
machine, or a recorded path refused by the containment guard. Those clients
are silently omitted from `outputs`, so without this key a wedged install
reports a bare `missing` with no explanation. The artifact's `state` stays
`missing` and the exit code stays `0`: `grim status` is a report, not a gate.

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
      "outputs_pending": [
        { "client": "cursor", "path": "/repo/.cursor/skills/code-review" }
      ],
      "clients_missing": [],
      "clients_extra": [],
      "clients_unresolved": [],
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

That lookup is deliberately **never** narrowed by a registry
[browse filter](./configuration.md#browse-filters). `--check` reports on
artifacts you already declared, so a filter hiding one of them from
`grim search` must not also hide its deprecation notice here — a filter is a
view over browsing, not over what you depend on.

`checked` (top-level, alongside `items`) reports whether the check actually
ran: `true` only when `--check` was passed and the invocation is online.
Combined with `--offline` (or `$GRIM_OFFLINE`), the check is skipped
entirely — one stderr warning explains why, `checked` stays `false`, every
`deprecated`/`replaced_by` stays `null`, and the command still exits `0`. A
single registry's catalog refresh failing (transport error) falls back to
that registry's cached catalog. With none cached, it degrades only that
registry's rows to `null`. `checked` still reports `true`, because the
attempt was made online and it just came back partial for that source.

`update_available` is populated by a **fresh per-artifact re-resolution**,
independent of that one catalog lookup: for each registry-locked row grim
re-resolves the reference that row declares — tag and all — and compares
its digest to the lock pin. It answers *would [`grim update`](#update) move
this pin?*, the same decision the [TUI](#tui)'s `↑ outdated` badge uses.
The resolution is always live, so a tag that moved surfaces even when the
cached catalog row is stale. That makes it the check for a pin
[`grim lock`](#lock) kept behind a moved tag, or one that was hand-edited. The
plain table keeps its columns, so stderr carries the answer: one note per row with
an update, naming `grim update -- <name>`. A `--config` run prints its absolute
config path in that command and a `--global` run prints `--global`, so the note
acts on the same config from any directory. On a stale lock it names `grim lock`
first instead. A dependency the lock records under more than one reference gets
no command, because a named update re-resolves every reference. `--format json` carries the field.

Bundles are covered on both halves. The bundle row re-resolves the declared
bundle reference against the pin recorded in the lock's `[[bundle]]`
snapshot. It is the only place a bundle that **added or dropped a member**
shows up, because every member already locked keeps its own pin. A
path-sourced bundle carries no registry pin and stays `null`.

Each bundle member row separately re-resolves the id its bundle listed. So a
member at a floating tag reports an update even while the bundle itself sits at
an unchanged version.

A dependency row answers too, by the tag its pin carries while an edge names that
tag. A tagless pin, or a kept pin whose tag no edge names any more, answers by the
smallest edge the lock records. An edge that names a digest holds the row to that
digest, because `grim update` cannot move past it. The answer covers
the recorded reference only. A moved parent that switches or drops the edge, or
a diamond that diverges, is not predicted.

The row answers only while each parent that records an edge onto it is in your
local store at its pin. That parent's stored manifest must also list the same
edges. Otherwise the row reads `null`, as on a fresh clone until `grim install`
or `grim lock` stores the parents.

A key the lock records under more than one reference re-resolves each of them.
When they name different digests, the row reads `null` in whatever order they
are listed. Its stderr note names the references and says `grim update`, with
the run's `--global` or `--config`, exits `78` until `grimoire.toml` declares
one.

Online, `--check` also prints one stderr note per directly declared row, or
declared bundle, that a recorded request is held back from. The note uses the words of the
[held-back warning](#lock-held-back). It reads the requests from the lock's
recorded edges. The note changes neither the exit code nor the JSON, and an
offline run or a lock with no recorded edges prints none. The note leaves once
`grim update -- <name>` moves the row to the requested digest.

A nested bundle has no row, and `update_available` stays each row's own answer.
A row that reaches a moved nested bundle lists it in
[`moved_nested_bundles`](./json-interface.md#status-check-nullability), and stderr
names it. `grim update -- <listed name>` moves it and can change its members.
The note carries the same scope, stale-lock and multi-reference qualifiers as a
row's note.
`grim update -- <row>` leaves the listing, because that row's own pin held.

Because the declared reference is what gets resolved, a release the
declaration does not point at is **not** an available update: a
`skill = "…:0.12.0"` pin, a `…:0.12` float that has not moved, and a
digest pin all report `false` while the repository carries a higher
`0.13.0` — `grim update` would leave every one of them exactly where it is.
Use [`grim search`](#search) or the TUI's version column to see what the
repository's newest release is.

The field is `true` when the declared reference now resolves to a digest
that differs from the lock pin. It is `false` when it matches (or the tag
vanished).

`null` means no answer. That covers a row with no lock pin
(dev-install, [path source](#add-path), path-sourced bundle) and a row with no
readable reference in the lock. It also covers an artifact whose re-resolution
failed. A completed re-resolve never reports `null`, and a failed one never lies
as `false`.

One re-resolve gets 30 seconds, and a tag outside the OCI grammar sends no request.
A run sends at most 2,048 re-resolves, rows first. Past that the rest read `null`,
with one warning.

A run also stops waiting on registries 300 seconds after the catalog lookup
starts. What got no answer by then reads `null`, and one warning counts those rows
and nested bundles. A catalog lookup still open then is dropped whole, so every
`deprecated` and `replaced_by` stays `null`, with one warning.

These per-artifact checks run with bounded concurrency, and no
state they find changes the exit code. Only a broken read fails `status`, such
as a corrupt lock, which exits `78`.

```sh
grim status --check
grim status --check --format json
```

## Artifact states {#artifact-states}

One artifact, three renderers, three word lists. [`grim status`](#status)
reports five states for every non-hook kind, the [TUI](#tui) seven, and
[`grim search`](#search)'s install badge five again. The badge's five are a
strict subset of the TUI's seven. `grim status` is the one that stands
apart: it shares only `installed`, `outdated` and `modified` with the
other two, and its `missing` and `stale` appear in neither. This section
is where every one of those words is defined.

A [hook](./hooks.md) (experimental) row adds three more tokens, none of
which any other kind can carry: `gated`, `not-armed`, and `untrusted`. See
[Hook arming][json-hook-arming] for what each one means and the full
`cause` table behind it.

A [settings][settings-status] row (experimental) can read `gated` or
`not-armed` too, and its `arming` array names each harness's cause.

`grim status` derives exactly one state per declared artifact, testing in
this order and taking the first that matches: `stale`, `missing`,
`modified`, `outdated`, `installed`.

| State | Meaning | Remedy |
|---|---|---|
| `installed` | Locked, an install record exists, every present output matches its recorded hash, and the recorded source content equals the lock's. | — |
| `outdated` | Recorded, outputs intact, but the installed content differs from the lock pin. | [`grim install`](#install) |
| `modified` | Recorded, outputs present, at least one drifted from its recorded hash — a hand edit. | Keep the edit, or [`grim install --force`](#install) to overwrite it |
| `missing` | No lock entry, **or** no install record, **or** a recorded output absent from disk. | [`grim install`](#install) — preceded by [`grim lock`](#lock) when the lock has no entry for it |
| `stale` | `grimoire.lock`'s `declaration_hash` no longer matches the live config. | [`grim lock`](#lock) |

A file excluded by [`.grimignore`](./artifacts.md#grimignore) never
contributes to the hash `modified` tests: a `__pycache__/` a bundled
script left behind, or any other file the artifact's default or authored
ignore list covers, cannot flip a row to `modified`. Only files that
*would* have been packed count.

Two of those rows say less than they appear to. `missing` collapses
never-installed and installed-then-deleted into a single word; the
`outputs` array under `--format json`, empty versus populated, is what
tells the two apart. And `stale` is not a fact about the artifact at all —
it is tested first, so it lands on **every** row at once and reports only
that the lock and the config have diverged, saying nothing about any
individual artifact's footprint. It is the same condition
[`grim install`](#install) refuses on with exit **65**, while `grim status`
still exits `0`.

The [TUI](#tui) carries no `stale` — that one describes the scope, not a
row — and adds four words of its own:

| State | What it adds |
|---|---|
| `not-installed` | Not declared, locked, or recorded in this scope. The TUI shows catalog rows `grim status` never lists, so it needs a word for them. |
| `via-bundle` | A [bundle](./concepts.md#bundles) member that is present and intact but not also declared standalone. `modified`, `outdated` and `integrity-missing` outrank it. |
| `pending` | Present and intact at the locked pin, but an install would still write an output the record never covered — a client that appeared since the install, or a render-layout move. [`grim install`](#install) clears it. |
| `integrity-missing` | An install record exists, but one or more client outputs are missing or unreadable, so the integrity record cannot be honoured. |

`installed`, `outdated` and `modified` keep their `grim status` meanings.
`pending` ranks below every problem state and only ever replaces
`installed`.

A settings row in the TUI adds three more, each with its own glyph and a
hint on the detail pane's `Settings:` line:

| State | Glyph | Meaning |
|---|---|---|
| `gated` | `○` | Held back by a gate you can answer: the `settings` flag is off, the workspace has not consented, or no value applies to the selected clients. |
| `not-armed` | `⊘` | grim refuses to write these settings here, whatever you answer. |
| `stale` | `≠` | `grimoire.toml` changed since the lock. [`grim lock`](#lock) brings it up to date. |

This `stale` is per row. It marks a settings package the lock has not caught
up with, and the scope-wide `stale` above still never appears. In a tree
group `gated` and `not-armed` count as `not-installed`, and `stale` counts
as `outdated`. [`grim status`](#status) names the cause behind `gated` and
`not-armed`.

Where several states roll up into one row the worst wins, and the two
places that happens rank them differently. A **tree group** shows

```text
integrity-missing > modified > outdated > not-installed > pending > installed
```

so one not-installed leaf beside a pending one renders `not-installed`,
and a group with no leaves at all renders `not-installed` too. A **bundle
row** drops `not-installed` from the ladder and folds an un-materialized
member into `pending` instead:

```text
integrity-missing > modified > outdated > pending > installed
```

Declaration is the gate there — a bundle absent from `[bundles]` reads
`not-installed` whatever its members look like — so no member can drag
the row back across the line declaration owns.

[`grim search`](#search)'s install badge is the TUI's list minus two —
`installed`, `not-installed`, `outdated`, `modified`, `pending` — carrying
neither `integrity-missing` nor `via-bundle` (and no `stale` either, for
the reason above), because a catalog row is
asked one question only: would installing this change anything here. That
leaves one crossing worth naming. An artifact whose output you deleted
reads `not-installed` on the search badge and `missing` under
[`grim status`](#status): the badge answers *is there work to do*, the
state answers *is the record whole*.

Every other page links this section rather than repeating the words —
including [the JSON `state` field](./json-interface.md#shapes-items), which
names the five `grim status` values and defines none of them. There is one
place to fix when the vocabulary moves.

## grim why {#why}

`grim why <kind> <name>` (`<kind>` is `skill`, `rule`, `agent`, `bundle`, `mcp`,
or the experimental [`hook`](./hooks.md) or [`settings`][settings]) shows why
an artifact is installed:
every chain from a declaration in `grimoire.toml` to it, root first. It reads the
config and the lock and never contacts a registry, so it works offline and costs
nothing. A [dependency](./concepts.md#dependencies) you never declared is the
usual reason to ask.

```sh
grim why skill gws-shared
```

```text
Path
skill/gws-drive -> skill/gws-shared
skill/review -> skill/gws-shared
```

Each row is one chain. The first node is something you declared, and the last is
the artifact you asked about. A [bundle](./concepts.md#bundles) is the root of
every chain through it. An artifact that you declared directly lists its own
one-node chain, beside any chain through an artifact that also requires it.

Two extra facts can follow the table. When a direct declaration shadows what an
artifact asked for, plain output adds an `overrides:` line naming the parent and
the identifier it wanted. The declaration won, as the
[conflict policy](./concepts.md#bundle-conflicts) says. A declared artifact may
also be required at the same digest, with no artifact asking for another version
of it. A stderr note then says the declaration can be removed.

Plain output shows the first ten chains and a line counting the rest. With
`--format json` the report lists up to 256 chains and always carries the exact
`path_count`, with `truncated` set when the list is shorter. The shape is in
[the `why` report](./json-interface.md#why-report).

`grim why` accepts the global options. `--global` asks about the global scope.

### Exit codes {#why-exit-codes}

| Situation | Exit |
|-----------|------|
| Chains printed | `0` |
| Unknown `<kind>` | `64` |
| No project, no lock (reason `no-lock`), or an artifact that is not in the effective set | `79` |
| The lock's dependency graph cannot be followed offline, or holds a cycle (reason `stale-lock`) | `65` |

Run [`grim lock`](#lock) to repair a `65`.

## grim context {#context}

`grim context` reports the **resolved invocation context** — read-only,
offline, no side effects. It answers "what would grim act on from here?"
without a consumer reimplementing the config walk-up, client detection, or
registry precedence rules: the resolved scope, the config/lock/state paths
(with existence flags for config and lock, plus the reason an existing lock
is unreadable), the effective
[client](./concepts.md#clients) target set, the resolved
[registry browse set](./configuration.md#multiple-registries), the primary
registry, and whether the run is [offline](#global-options) (and why:
`flag` or `env`).

```sh
grim context --format json | jq .clients
```

The JSON document is a single object: `{version, scope, workspace,
config_path, config_exists, lock_path, lock_exists, lock_error, state_path,
grim_home, offline, offline_source, clients, registries, default_registry}`.
`lock_error` carries the reason an **existing** lock could not be read
(oversized, corrupt, permission-denied) and is `null` when the lock reads
fine or is simply absent — `lock_exists` alone answers "is it there", not
"can grim use it". The exit code stays `0` either way; `context` reports
the state, it does not fail on it.
`registries` entries are `{alias, url, kind, default, authenticated, include,
exclude, insecure, catalog}` with `kind` either `registry` or `index`. `include` and `exclude`
are the source's authored
[browse-filter](./configuration.md#browse-filters) patterns in declaration
order — always-present arrays, `[]` on an unfiltered entry, and `[]` for
every entry under `--registry`, whose forced browse set genuinely carries no
filter. This is the seam that saves a consumer parsing `grimoire.toml`
itself. Because these are the patterns as **resolved** rather than as
authored, they are also how you catch a filter that failed to compile and was
[dropped whole](./configuration.md#browse-filters): the entry reports `[]`
for both lists while `grimoire.toml` still shows the patterns. The plain
table appends the same information to the registry row as
**counts** — `acme ghcr.io/acme (registry, default, 2 include, 1 exclude)` —
and omits both clauses entirely when the entry is unfiltered, so an
unfiltered row reads exactly as it always has. `insecure` is the entry's
authored [plain-HTTP](./configuration.md#plain-http-registries) opt-in,
always present and `false` by default; the plain row gains a `, insecure`
clause only when it is set. It reports the **authored field**, not the
effective transport: a host reached over HTTP through the implicit loopback
set or `GRIM_INSECURE_REGISTRIES` still reports `false`, so this key and
`grim config get registry.<alias>.insecure` always agree — except under
`--registry`, which replaces the browse set with synthesized entries that
carry no authored fields, so every row reports `false` there. That is a
statement about the browse set, not about transport: `--registry` never
narrows which hosts are reachable over plain HTTP.
`catalog` reports how that source's cached listing was last synced. It is
read from the [sync sidecar](./configuration.md#data-layout) and never
touches the network, so it costs nothing offline. Its keys are always
present:

- `built_at`: when the listing was last built or confirmed current.
- `entries`: the listing's size.
- `layout`: `flat` or `anchored`.
- `generation`: the adopted index root's generation. `null` on a flat sync.
- `fallbacks`: consecutive refreshes that fell back to `all.json`.
- `failures`: consecutive failed refreshes.
- `last_error`: the last failure, with any URL redacted.
- `retry_after`: the end of the [backoff](./package-index.md#consuming-backoff),
  as an RFC 3339 UTC timestamp, or `null` when no wait is recorded. It is
  grim's own deadline, not the server's `Retry-After` header: each failure
  stores the later of grim's schedule and the server's request, capped at one
  hour. Read it as the wait grim applies. grim ignores a deadline it cannot
  parse, and it cuts one more than an hour away to an hour, so the value can
  outlast the wait.

An OCI source reports `null` for `layout` and `generation`. The whole value
is `null` when the source has no readable sidecar from this version of grim,
as before its first sync. The plain table is unchanged.

`authenticated` is a boolean: `true`
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
`mcp`, or the experimental [`hook`](./hooks.md) or [`settings`][settings])
undeclares an artifact from `grimoire.toml` and the lock. It leaves
already-installed files (or, for an [MCP server](./mcp-servers.md), the
registered config entry) in place — use [`grim uninstall`](#uninstall) to
remove those too.

Removal acts on the **effective** declaration, fully offline: the lock entry
is dropped only when no remaining declaration holds the artifact. A declared
bundle may still name the artifact after you remove its direct declaration. At
the *same* identifier the entry stays, and its provenance flips to the bundle. At
a *different* identifier the pin may not answer it offline, and the table below
says whether the entry stays. When the pin answers the bundle's request, a fresh
lock is restamped. When it does not, the lock is left stale, and grim tells you
to run [`grim lock`](#lock), never a silently incomplete fresh lock. A bundle
that overrode a nested include leaves the lock stale the same way, because
nothing says offline what the hidden include holds.

Some locks hold no member list for a declared bundle: grim 0.3 or earlier wrote
them, or their `[[bundle]]` rows were cut by hand. Such a lock cannot say whether
that bundle provides the artifact, because grim never stamps a bundle on a
declared row. So `remove` reads the bundle's members from the local store. The
same goes for a bundle another artifact requires that has no row in the lock.

An artifact the stored members leave out drops, and the lock stays proven.

Take a lock whose hash matches its declaration and that records no dependencies,
as grim 0.15.1 writes. There an artifact stays only on evidence: the store shows
the bundle provides it, or its entry carries the bundle's stamp. The entry keeps
its pin under that bundle's stamp, as `grim lock` writes it. The lock stays
proven, so `grim install` exits `0` as on 0.15.1, and the next `grim lock` keeps
the pin.

With no such evidence, the entry drops as it did on 0.15.1. When the store lacks
the bundle, a note names it. If the bundle does provide the artifact, the next
`grim lock` resolves it afresh, and its pin can move.

On any other lock, a stale one or one that records dependencies, a store that
lacks the bundle rules nothing out. The entry stays with a note naming the
bundle, and the lock is left stale. [`grim lock`](#lock) then reads the bundle.
It keeps the pin of an artifact the bundle provides and drops one it does not.
`grim install` exits `65` until then.

A lock that was already stale stays stale, since restamping it would vouch for an
edit nothing has read. The exception is a lock that answers the edited declaration,
which restamps. Every declared artifact and bundle holds its row. Every other row
names an edge that reaches it and holds any digest an edge names. Nothing else is
left, and no key has two rows.

The other exception is an edit that takes the declaration back to where a stale
lock's hash came from. Removing a line you just typed does that. The lock stays as
it is, and its hash reads true again. An edit that changes the rows of a lock it
leaves stale writes a hash of its own. So a hash that matches proves the rows.

Any other stale lock keeps [`grim install`](#install) at exit `65` until
`grim lock`.

A [dev-installed](#install-dev) record was never declared, so `remove` has
nothing to undeclare for it — use `grim uninstall` to drop one instead.

`remove` follows [dependencies](./concepts.md#dependencies) the same way. It
drops the declaration, and from the lock every dependency that only the removed
artifact reached. A dependency another remaining artifact still requires stays.
Files stay on disk, as always. An artifact that is only a dependency was never
declared, so there is nothing to remove: `remove` reports `absent` for it, and
removing the artifact that requires it is what drops it.

`remove` resolves nothing, so a kept pin keeps its digest and its tag. After a
diamond's holder leaves, `x:1@sha256:…` can stay beside the surviving request
`x:2`. The pair is true, because `x:1` is what resolved that digest, and relocks
keep it. A bundle's lock row stays the same way, and
[`grim status --check`](#status-check) answers for the surviving request.

A key whose direct declaration left is the one case the lock cannot settle,
because its edges were hidden and never resolved. `remove` compares the requests
that survive with what the pin's own record names:

| The pin names | Lock row | `declaration_hash` |
|---|---|---|
| Every surviving request | Kept | Restamped |
| Some of them | Kept, with a note | Left stale |
| None of them | Dropped, with a note | Left stale |

In the last two rows `grim install` exits `65` until [`grim lock`](#lock). When
such an edit changes the lock's rows, it writes a hash no declaration produces.
Restoring `grimoire.toml` by hand then does not make the lock read fresh. An edit
that changes no row keeps the hash, which still proves those rows.
`grim lock` keeps a pin that names a surviving request, and
[`grim update`](#update) exits `78` if the requests truly disagree.

One write of a tag is the exception. A pin with no tag, from a lock grim 0.15.1 or
earlier wrote, relies on the lock's hash. An edit that ends the proof of a fresh
lock is its last holder, so it stamps kept tagless pins first.

The tag comes from the declaration, or from an edge the lock recorded in the
pin's repository. An edge the edit leaves in the lock comes first. A digest-only request stays
tagless, and no digest changes.

A lock already stale has no proof to stamp from, so an edit that leaves it stale
writes no tag. A tagless pin there resolves once, and a warning names it.

If the lock's dependency graph cannot be followed offline, `remove` acts on the
target alone. It drops that declaration and its lock entry, touches nothing
else, and prints a note naming the artifacts it kept:
``skill 'c' keeps its pin: this lock cannot show its dependencies; run `grim lock` ``.
The `grim lock` it names carries the run's `--global` or `--config`. The lock
is then stale until `grim lock` repairs it,
and `grim install` says so rather than installing from it. An edit back to the
lock's hash is the exception above: every row stays.

The target's entry stays where the rules above keep it. It also stays while an
artifact may require it whose requirements only `grim lock` reads. A note names
that artifact, and the lock stays stale.

On a lock that records dependencies, that is any artifact with no lock entry. It
may be a line typed since the last `grim lock`, or a dependency an earlier edit
dropped.

An entry that records none, as grim 0.15.1 writes every entry, cannot show what
it requires. On a lock that records none at all, the entry stays for a line typed
since the last `grim lock`. In either case `grim install` already exits `65` on
the lock.

Otherwise an entry that records none counts as requiring nothing, as on 0.15.1.
What only it requires leaves the lock, whatever the local store holds. The next
`grim lock` resolves it afresh if something still requires it, so its pin moves
if its tag moved.

Where the store holds such an entry's manifest, a note names it and each entry
that left which its dependency list may reach. The store can show that it
reaches none of them through entries the lock pins, and then no note names it.
An entry with no lock row is a link the store cannot follow, so past it the note
says the dependent may require them. The note gives the recovery: restore the
config the edit changed and `grimoire.lock`, run `grim lock` online once, then
repeat the edit. That config is `grimoire.toml`, or with `--global` the one in
`$GRIM_HOME`, beside its `grimoire.lock`. Where that pair is not in the working
directory, as under `--config` or from a subdirectory, the note names both by
absolute path. When the config changed since the last lock, the note says the recovery may not keep the rows, because its `grim lock` resolves every
untagged pin once.

Run one online `grim lock` before `grim remove`, `grim uninstall` or a TUI delete
on a lock 0.15.1 wrote. It records each dependent's dependencies, and the edit
then keeps the pins they reach. If it exits `78`, settle the conflict it names
first. 0.15.1 refuses a lock that records them, so its `status` and `install`
exit `78`. The first `grim install` on such a lock already exits `65` and names
`grim lock`.

An offline `grim lock` reads those requirements from the local store. An entry
counts as requiring what its lock entry records when the store lacks its
manifest, or the dependency list that manifest names. On a lock 0.15.1 wrote,
that is nothing.

What only that entry requires leaves the lock at exit `0`, whatever its kind, as
on 0.15.1. The next online `grim lock` resolves it afresh, so its pin moves if
its tag moved.

An offline export that relocks a marketplace plugin drops such entries too, so
an include you deleted leaves the plugin at exit `0`. 0.15.1 refuses that
offline export with `81` and writes nothing, and its next online export drops
the include. If an artifact still requires it,
`grim update --marketplace <manifest> <plugin>` brings it back as a dependency,
so its pin moves if its tag moved.

0.15.1's offline lock exits `0` only while no bundle is declared and its tag
cache answers every tag. Otherwise it exits `69` and leaves the lock as it was.

## grim uninstall {#uninstall}

`grim uninstall <kind> <name>` (`<kind>` is `skill`, `rule`, `agent`, `mcp`,
`hook`, or `settings`) is the full inverse of install: it deletes the
materialized files, drops the install record, and undeclares the artifact from
the config and lock. The interactive TUI's delete action reuses the same seam.
For an [MCP server](./mcp-servers.md#modification-detection), there is no
materialized file to delete — grim splices only the managed entry back out
of each client's config file, leaving the file itself and every other
entry untouched.

That splice is gated on the entry still matching what grim installed. If
the managed entry has drifted — you edited it in place after install —
`uninstall` refuses the whole operation with exit 65 (`reason:
"modified"`, `forceable: true`) and changes nothing, so nothing is
half-removed. `grim uninstall --force <kind> <name>` removes it anyway.
An entry that is already gone, or sits in a config file grim cannot
parse, is not gated: there is nothing left to preserve. Materialized
files are deleted either way — `--force` affects config entries only.

An entry grim *adopted* rather than wrote — it was already there and
already identical, so the install recorded it without touching a byte —
is left in the config file untouched, while the install record is dropped
as usual. Uninstall is the inverse of install, and the inverse of writing
nothing is removing nothing. A warning names the entry, and so does the
report's `abandoned_entries`. `grim uninstall --force` removes it instead.
Edit such an entry afterwards and it becomes ordinary drift, refused by the
gate above.

Uninstall writes no managed vendor config entry back. Removing one artifact
leaves another rule's `claudeMdExcludes` element or OpenCode glob as you left
it, and the TUI delete does the same.

A [settings package][settings-uninstall] follows the same gate, and
`--force` covers its values too. One case differs from an MCP entry. An
adopted value that still matches its record stays, even under `--force`, and
is named in `abandoned_entries`. An adopted value you changed refuses, and
`--force` removes it. Each value grim removes is listed in the report's
[`settings_changes`][settings-changes].

The lock follows the same effective-declaration rule as
[`grim remove`](#remove). Something that is still declared may keep the artifact
wanted after you undeclare it, and then `uninstall` does not delete its files:

| The artifact is | What `uninstall` does | `status` |
|---|---|---|
| Declared directly and also provided by a declared bundle, or required by another artifact | Drops the direct declaration only; files and lock entry stay | `uninstalled` |
| Not declared directly, provided by a declared bundle | Nothing changes | `kept-by-bundle` |
| Not declared directly, required by another artifact | Nothing changes | `kept-by-dependency` |
| Not held by anything | Deletes files, install record, declaration and lock entry | `uninstalled` |

On a lock with no member list for a declared bundle, the local store answers
for that bundle, as it does for [`remove`](#remove). So does that bundle's stamp
on the target's entry. With neither, the target's files go, and
`grim lock` then `grim install` bring them back.

For the two `kept-by-*` rows, remove the bundle or the requiring artifact to
remove the artifact. A `kept-by-bundle` run prints a line naming each bundle that
keeps it on its own, with the commands that remove it. So does a run that drops a
direct declaration a bundle also provides, though its status stays `uninstalled`. When no one bundle keeps
it on its own, the line names no bundle and gives only the `uninstall` command.

When nothing holds the target, `uninstall` also deletes the
files and install records of every [dependency](./concepts.md#dependencies) that
only the target reached. It keeps those another artifact still reaches. A kept
pin keeps its digest and its tag, and the lock's hash follows the table under
[`remove`](#remove).

If the dependency graph cannot be followed offline, `uninstall` removes the
target alone and prints the same note as `remove`, naming the artifacts it kept.

`uninstall <kind> <name>` is also the removal path for a
[dev-installed](#install-dev) record: since a dev install writes only an
install record and never a declaration, the file-deletion half behaves
exactly like a registry artifact's uninstall — same addressing, same
deleted files, same dropped record — and the undeclare half is a no-op
(there was nothing declared to drop).

## grim hook {#hook}

Three subcommands manage [hooks](./hooks.md). The kind is experimental. See
[Stability](./stability.md#unstable). Declaring one is still
`grim add --kind hook <reference>`, exactly like any other kind.

Since 0.16.0 `allow` and `revoke` also cover [settings
packages][settings-consent], and `grim hook` has the visible alias
`grim consent`. `grim hook --help` reads "Inspect and consent to lifecycle
hooks and settings".

- **`grim hook list`**. Declared hooks with their tier, events, and
  per-client arming state (`installed`, `gated`, `untrusted`, `not-armed`).
  Each carries an `arming` array of `{client, cause, message, transient}`
  for every client that did not arm. See [Hook arming][json-hook-arming]
  for the full cause table.

  A hook never materialized contributes no items, only one warning naming
  it. A hook reaped by flag-off keeps its payload and shows up `gated`
  like any other.
- **`grim hook allow [path]`**. Records this **workspace's** consent to
  the hooks and settings packages its lock pins, whichever flags are on.
  Its help reads "Consent to the hooks and settings this workspace
  declares". The record lives at
  `$GRIM_HOME/hooks/consent/<workspace-key>.json`, machine-local and keyed
  by the resolved workspace path.

  `--global` exits **64**: global scope is
  already consented and carries no record. With a settings package in the
  lock, the refusal names `grim consent allow`, and a recorded consent warns
  ``consent recorded; run `grim install` to apply what it covers``.
- **`grim hook revoke [path]`** (alias `deny`). Removes the workspace
  consent record, for hooks and settings alike. Settings values already
  written stay in place. Its warning still reads
  ``consent revoked; run `grim install` to disarm these hooks``.

  It is idempotent, so revoking a never-consented workspace exits `0` and
  reports `not-consented`. There is no fourth "explicitly
  denied" state, only absent, granted, or drifted. `--global` needs no
  refusal: global scope never writes a consent record, so there is nothing
  to remove. It exits `0` reporting `not-consented` like any other
  never-consented workspace.

```sh
grim hook allow
grim hook list --format json
grim hook revoke
```

Neither the feature flag nor a workspace's consent record has an
environment-variable form. That is deliberate, so nothing in a cloned
repository can flip either one. The only per-invocation override is the
`--trust-workspace` / `--no-trust-workspace` flag pair on `add`, `install`,
and `update`. Its aliases `--trust-hooks` / `--no-trust-hooks` still work.

## grim search {#search}

`grim search [query]` searches the registry catalog. An empty query lists
the whole catalog. The query is whitespace-split and the terms are ANDed.
Every term must match somewhere.

A bare kind keyword (`skill`, `rule`, `agent`, `mcp`, `bundle`, or the
experimental [`hook`](./hooks.md), singular or plural) filters by kind instead of matching as text. For example, `grim
search skill review` finds skills matching "review". Since 0.16.0 `setting`
and `settings` filter to the experimental [settings kind][settings] the same
way. They no longer match a name or keyword as text. When
`[[registries]]` are configured, all of them are browsed and the results
are flattened into one table.

The artifact's name (the repository's leaf segment) and its keywords match
by **fuzzy subsequence**, the same shape [fzf][fzf] and most editor command
palettes use. A term's letters must appear in order, but need not be
adjacent. So `grim search kubctl` finds `kube-control`, and a term may span
a hyphen or a path separator. Letters typed *wrongly* are not forgiven.
`kuberentes` does not find `kubernetes`, because the `n` and `e` are
transposed rather than merely missing.

A match only counts when it is *tight*. A term's letters strung out across a
long name are noise. A name or keyword that equals the term outright, or
carries it as a whole word, scores higher still. `grim search grim` ranks
the artifact literally named `grim` above `grim-usage`, and both above one
only tagged with the word.

The summary, the description, and the namespace (the path segments above the
artifact's name) match a different way. A term must be the **prefix of a
word** in that field. `rev` finds "code review".

`test` does not find "latest". A term that itself carries a separator
(`code-review`, `node.js`) instead has to appear contiguously. Fuzzy
subsequence matching is too loose for prose, where a short term's letters
sit inside almost any sentence.

The registry host is not searched by a plain word. Every artifact on
`ghcr.io` shares it, and a hit there would match every row from that host. A
term that itself names a host or a path (containing `/`, `.`, or `:`) is
matched against the full reference instead, host included. A `repo` value
copied out of `--format json` finds the artifact it named this way.

Because these matchers admit far more repositories than a plain substring
search would, results are **ranked by relevance**. Best match comes first,
across every browsed registry at once. A hit on the artifact's own name or a
keyword outranks the same word found only in a description. Results scoring
below [`options.search_min_relevance`](./configuration.md#grimoire-toml)
percent of the best hit are dropped. That is a percentage from `0` (list
every match) to `100`, defaulting to `50`.

When some artifact is *named* for the query, artifacts that merely mention
it in their description are left out. When nothing matches by name, the
cutoff is measured against whatever the best remaining hit is. A summary hit
can still drop a mere description prefix sitting beside it. Ranking
replaces registry-declaration order whenever there is a query. Hidden rows
are reported on stderr:

```text
3 weaker matches hidden; set options.search_min_relevance to 0 to list all
```

What attributes each row to the registry that served it is the `source`
object under `--format json`, described below. The unqueried browse is not
ranked and still lists registry by registry, in declaration order.

`--refresh` forces every browsed source to sync. It ignores the cache
lifetime and a failing source's [wait](./package-index.md#consuming-backoff).
On an index with a root it also drops the
[stats skip rules](./package-index.md#consuming-stats), so the stats are
fetched again.

`--registry <ref>` collapses the
browse to exactly the registries it names. It is repeatable and
comma-separated (`--registry a,b` or `--registry a --registry b`), and the
first value given is primary.

`GRIM_DEFAULT_REGISTRY` is only the
short-id resolution default. It does not restrict the browse set when
`[[registries]]` is configured.

`--limit <N>` prints at most `N` rows. It cuts the list after
ranking and `--sort`, so the rows kept are the first `N` in result order. `N` is a
positive integer. `--limit 0` is a usage error that exits `64`. Omitted,
every match is printed.

`--offset <N>` skips the first `N` rows before `--limit` applies, so
`--offset 100 --limit 100` prints rows 101 to 200. `N` is a non-negative integer
and defaults to `0`. A negative or non-numeric `N` is a usage error that exits
`64`. An offset at or past the last row prints only the table header, adds a
note on stderr and still exits `0`.

The JSON `total` field still counts every match, so a script can tell a cut
list from a complete one. See [the `total` and `truncated`
fields](./json-interface.md#search-total). To read every page, follow the
[paging recipe][json-search-paging].

In plain output, a partial page adds one line on stderr, after the table. It
names the row range, the total and, while rows remain, the next offset:

```text
showing 101-200 of 600; next page: --offset 200
```

The last page reads `showing 501-600 of 600` with no next offset. An offset
past the end reads `no rows at --offset 700; 600 match in total`. A complete
page prints no extra line, and neither does `--format json`.

A `[[registries]]` entry carrying an `include` / `exclude`
[browse filter](./configuration.md#browse-filters) contributes only the
repositories its patterns admit. The filter is applied at read time, after
the query — so `grim search` and the TUI narrow identically, and the shared
catalog cache stays whole. A mis-aimed-pattern warning fires on stderr,
naming the source and the counts, but **only on the unqueried browse** —
`grim search` with no query, or a TUI load:

```text
registry 'acme': filter admitted 0 of 148 repositories; patterns match either the repository path or the fully-qualified reference, and anchor at the candidate's first segment — see https://grimoire.rs/configuration.html#browse-filters
```

That line fires when a non-empty `include` list contributed **nothing** from
a source that had rows. Under `grim search <query>` the count is a
query-shaped subset — indistinguishable from a deliberate search for a
hidden term — so the warning stays silent there; it is decidable only on
the full, unqueried listing. A non-empty `exclude` that removes nothing does
**not** warn (tried and dropped: it is also the permanent, correct state of
an `exclude` with nothing yet to match). An **exclude-only** filter that
empties a source also stays silent, since that is what it was asked to do.
The exit code stays `0`. `--registry <ref>` browses **unfiltered** — a
forced browse set is exactly what the flag names, with no config filter
applied to it.

The filter runs at read time, after each source's listing is built. An
`index` source lists every entry it publishes, so a query, a filter,
[`grim rate`](#rate) and [`grim status --check`](#status) reach every row of
a large index. One past the [abuse bounds](./package-index.md#consuming)
fails its fetch instead.

An `oci` source lists every repository its registry's `_catalog` endpoint
returns, following the registry's own paging to the end. The first browse of a
large registry reads each repository's metadata and takes a while. While that
build runs, any other `grim` command serves the rows built so far, in the
terminal and with `--offline` alike, and reads `capped: true` for the source
until the build finishes. Rows are served only with their full metadata, so a
repository not yet read is absent, not shown bare. A build that is interrupted
resumes on the next command and does not re-read rows younger than the
catalog's one-hour lifetime. A later refresh asks the registry for each
repository's tag digest and downloads a manifest again when that digest
changed. It also reads a manifest again when a grim before 0.16.0 cached the
row without a kind, or when the manifest names a grimoire kind this grim does
not know.

An `oci` source reads `capped: true` while its listing is incomplete: the
build is still running or was interrupted, or the `_catalog` walk could not
finish (the registry repeated a paging cursor, failed part-way, or listed
more repository names than grim keeps). A queried search over such a source
prints one warning per source:

```text
catalog for source 'acme' is incomplete: its listing has not finished, so results and the total are a lower bound; the next online grim search retries it
```

An incomplete listing is never served as fresh: every online command retries
it, and a `_catalog` walk that cannot finish is walked again each time.

Reading `capped` in JSON is how a paging script tells "still loading" from
done. Run one `grim search --format json --limit 1` to drive the build and read
pages with `--offline` while it runs. That run builds only when no other grim
process holds the build lock; otherwise it serves the newest checkpoint and
exits at once with `capped: true`. Repeat the run until `capped` reads `false`.

A cached listing that an earlier grim capped at 500 rows is rebuilt on the
next command, not served until its catalog lifetime expires. An `oci` build
resumes from its rows, so only the repositories it lacks are read. If the
rebuild fails, grim serves the capped rows with the incomplete-listing
warning.

A registry that gates its `_catalog` browse endpoint (GitLab SaaS, GHCR,
Docker Hub) lists nothing at all, whatever the query. That looks like an
empty catalog, so `grim search` warns on stderr instead. The warning fires
when at least one browsed source is a plain OCI registry. It also requires
that none of the sources listed any row before the query, the filter, and
the cutoff ran:

```text
no catalog entries; some registries (…) gate the `_catalog` browse endpoint and an empty list is expected — install/add/release by explicit reference works regardless; see …
```

The hint fires under a query too, because a gated registry lists nothing
under any query. It stays quiet for a query that simply misses a populated
catalog. It also stays quiet for a source that failed to load outright,
which gets its own per-source warning instead.

The plain table shows each entry's short summary (`com.grimoire.summary`),
falling back to the description when no summary is set. On an interactive
terminal that column is truncated to fit the width; piped output and
`--format json` keep the full description. The JSON output also carries a
`repository` field — the artifact's authored
[repository URL](./publishing.md#metadata-repository), or `null` when the
artifact has none — and a `replaced_by` field — the authored
[successor reference](./publishing.md#metadata-replaced-by), or `null` when
none. Neither is shown in the plain table.

Every JSON item also carries a `source` object — `{alias, locator}` — naming
the configured [`[[registries]]`](./configuration.md#multiple-registries) entry
the row was browsed from. It is the same attribution the [TUI](#tui) roots its
registry tree by, and it is what lets a client group a flat result set by
alias. `alias` is `null` when the entry declares none, and under `--registry`
or the legacy single-registry fallback, which have no alias to report;
`locator` is byte-identical to the configured value. Neither half is derivable
from `repo`: that names the *artifact's* registry host, while an entry's
locator is a host plus an optional namespace for an `oci` source and the index
url for an `index` one — and one index serves rows from many hosts. Like
`repository`, `source` is JSON-only; the plain table keeps its five columns.

Beside `items`, the JSON document carries a `sources` array — one
`{alias, locator, ok, error, capped}` object per browsed source, in declaration
order. A source whose refresh fails serves its cached catalog when one
exists, and degrades to an empty group when none does. Either way the
reachable sources still answer, and the exit code stays `0` whether one
source failed or all of them did. `sources` is where that shows up. `ok` is
`false` and `error` names the cause for a source whose catalog failed to
refresh, so a short or empty `items` is never mistaken for a small or empty
catalog:

```json
{
  "items": [],
  "sources": [
    { "alias": "acme", "locator": "ghcr.io/acme", "ok": false, "error": "invalid catalog file", "capped": false }
  ],
  "total": 0,
  "truncated": false
}
```

The same failure is still reported on stderr for anyone reading the plain
output, which keeps its five columns. The line names the source by its
locator, escaped, never by its alias. With no cached catalog it reads
`catalog for source '<locator>' unavailable: …`. With one, grim serves the
cached rows and says so, and the source still reads `ok: false` in JSON:

```text
catalog for source 'https://index.example.org' unavailable: package index fetch failed for 'https://index.example.org'; serving the cached catalog built 2026-10-16T09:12:40Z
```

A failed refresh does not restamp the cache. Later commands make no request
to that source until its recorded wait ends, one hour at most, and the source
reads `ok: false` with a `refresh deferred until` error. `--refresh` ignores
the wait. A still-fresh cache is served with `ok: true`.
Full contract: [the `sources` array](./json-interface.md#search-sources).

Two more always-present siblings say whether `items` is the whole answer.
`total` counts every matching row before `--offset` and `--limit` cut the
list. `truncated` is `true` when `items` is not complete: `--offset` or
`--limit` cut it, or a source's listing is incomplete and `total` is a lower
bound. `items.length < total` means the page is partial, and an incomplete
listing may apply as well, which an incomplete-listing warning on stderr
reports. Full contract: [the `total` and `truncated`
fields](./json-interface.md#search-total).

A [deprecated](./publishing.md#metadata-deprecated) entry is **hidden by
default** — unless it is installed in the active scope (directly or via a
bundle), in which case it stays listed so you can see what you have. Pass
`--show-deprecated` to include every deprecated entry, or set
[`options.show_deprecated`](#config) to `true` to change the default. A shown
deprecated entry is flagged in the `Status` cell with a comma-suffixed
`deprecated` (e.g. `installed,deprecated`), and JSON carries the notice in a
`deprecated` field (`null` when the artifact is not deprecated).

When the browsed index publishes [artifact ratings](./ratings.md), JSON
items carry a `rating` object — `{up, url}`, or `null` for an unrated
artifact. It is JSON-only; the plain table keeps its five columns.

```sh
grim search review
grim search --show-deprecated review
grim search --refresh --registry ghcr.io/acme
grim search --sort rating --limit 10
grim search --sort rating --limit 10 --offset 10
```

### Ordering the results (--sort) {#search-sort}

`--sort <name|updated|rating|downloads>` replaces the default ordering
with an explicit one:

| Value | Order |
|---|---|
| `name` | Leaf name ascending, case-insensitive |
| `updated` | Publishing date descending; undated last |
| `rating` | Upvotes descending, then date descending; unrated last |
| `downloads` | Download total descending, then date descending; uncounted last |

`--sort rating` orders by upvotes descending, then by publishing date
descending, then by name — and an unrated artifact sorts into a bucket of
its own at the end rather than as zero votes, so a fresh index with no
ratings yet still reads in a stable, meaningful order. The same applies to
an uncounted artifact under `--sort downloads` and an undated one under
`--sort updated`: a missing value is never folded into `0` or epoch `0`,
which would order it against real data by accident.
Every mode breaks its last tie on the fully-qualified reference, so the
order is total — two runs over the same catalog render identically.

Given together with a query, `--sort` **replaces** relevance ranking:
`grim search review --sort rating` is "the best-rated of the review
matches", not "the most relevant, subsorted". The matches are the same set
an unsorted query returns, relevance cutoff included — set
[`options.search_min_relevance`](./configuration.md#grimoire-toml) to `0` to sort
every match. Omitted, results keep today's
order — relevance when queried, registry-declaration order otherwise —
which is what makes the flag purely additive.

[`grim tui --sort`](#tui) takes the same four values and applies the same
order.

```sh
grim search --sort rating
grim search review --sort updated
grim tui --sort rating
```

## grim rate {#rate}

`grim rate <ref>` casts a vote on an artifact. There is no Grimoire service
behind it: the vote is an ordinary upvote on the index operator's own
discussion thread (GitHub) or an emoji reaction on their work item
(GitLab), posted under **your own** forge account. How an index turns that
on, and what it guarantees, is [Artifact Ratings](./ratings.md); this
section is the command surface.

```
grim rate <ref> [--up | --remove] [--yes] [--dry-run]
                [--token-stdin] [--token-host <HOST>]
```

The reference resolves through the same catalog seam
[`grim search`](#search) uses, so a short identifier expands against the
resolved default registry and the global [`--registry`](#global-options)
flag selects the browse set. A source whose refresh failed resolves against
its stale-served cached rows, so `rate` still finds the row instead of
exiting `79`. The vote applies to the *repository*, so a tag
or digest suffix parses and is then ignored. A [browse
filter](./configuration.md#browse-filters) hiding a row from a listing does
not make it unvotable.

When the row reads as unrated, `rate` fetches that source's stats once more
before it exits `65`. A stats blob that was unavailable when the listing was
built then cannot hide a rating. The fetch stays within the row's own source
and is skipped under `--offline`.

Only an HTTP index publishes stats, so a git index or a registry source skips
the fetch and `rate` exits `65` as before. An index without a root serves its
stats in `stats.json` beside `all.json`, and that fetch downloads both files.

| Flag | Effect |
|---|---|
| `--up` | Register an upvote. The default when neither flag is given. |
| `--remove` | Retract **your own** upvote. Not a downvote — votes are up-only and binary, and both forges' primitives are toggles. Given with `--up`, exit 64. |
| `--yes` | Skip the confirmation prompt. **Required for every non-interactive run.** |
| `--dry-run` | Resolve the row, the provider and the host, print the report, and **mutate nothing**. On its own it needs no credential and makes no forge request, so it works under `--offline`. Combined with `--token-stdin` it also reads back your own vote state — see [reading your own vote state](#rate-viewer-state). |
| `--token-stdin` | Read the forge credential from stdin. There is intentionally **no** `--token <value>` flag — argv is world-readable. Implies a non-interactive run, so it requires `--yes` — **except under `--dry-run`**, which posts nothing and therefore has nothing to confirm. |
| `--token-host <HOST>` | Declare which host the injected credential (`--token-stdin` or `GRIM_RATE_TOKEN`) belongs to; grim refuses before sending anything if it does not match. Required when the host was declared by the index. |

**Voting confirms by default.** A vote posts publicly under your name, so
an interactive run prompts on stderr before it mutates anything:

```console
$ grim rate ghcr.io/acme/skills/code-review
This posts publicly to your github account as alice. Continue? [y/N] y
```

Only `y` or `yes` proceeds; anything else — including an empty line and
EOF — exits `0` with no mutation and no local vote record. A
**non-interactive** run without `--yes` exits `64` naming the flag rather
than hanging on a prompt nobody can answer, and `--token-stdin` without
`--yes` exits `64` too: stdin is carrying the credential, so it cannot also
carry a confirmation, and the prompt is deliberately never rerouted to
`/dev/tty`.

**On the voting path that rule is absolute**: a piped credential always
needs `--yes`. It is narrowed by `--dry-run`, and by nothing else — `grim
rate <ref> --dry-run --token-stdin` does not need `--yes`, because the
confirmation exists to gate a public post under your account and a dry run
posts nothing at all. Read separately the two look contradictory; the
exemption is one flag wide.

### Reading your own vote state {#rate-viewer-state}

`--dry-run` alone answers *where would this vote go*. `--dry-run` plus a
piped credential also answers *have I already voted* — it consumes the
credential, issues exactly **one read-only query**, and reports
`viewer_up`:

```console
$ printf '%s' "$FORGE_TOKEN" | grim rate acme/skills/code-review \
    --dry-run --token-stdin --format json | jq .viewer_up
true
```

It is the only way to get that answer: the count in `stats.json` is an
aggregate with no identity in it, and a client is not expected to query a
forge itself.

`viewer_up` is **tri-state**, and the third state is the point:

| Value | Meaning |
|---|---|
| `true` | The forge reports this credential's account as having upvoted |
| `false` | The forge reports it as not having upvoted |
| `null` | **Not asked, or not knowable** — no credential was piped, no host resolved, or the query failed or was unauthorised |

A failed query reports `null`, **never `false`**. Rendering a read you
could not complete as "you have not voted" is the exact claim
[invariant R-3](./ratings.md#workflow-guarantees) forbids, so a consumer
treats `null` as *unknown* and renders it neutral rather than as a
not-voted affordance.

Two interactions are easy to get backwards:

- **It needs the network.** Bare `--dry-run` works under `--offline`;
  `--dry-run --token-stdin` exits `81`, because it cannot degrade into an
  answer it never checked.
- **The credential is still validated as input.** Only the *query* failing
  yields `null` — a terminal on stdin or multi-line input is still `64`,
  and empty input is still `80`, exactly as on the voting path.
  `--token-host` is also still enforced here, before the token is read.

### Credentials {#rate-credentials}

`grim rate` has its **own, narrower** credential ladder — it does not reuse
the [publishing credential](./authentication.md) or the announce token,
because voting needs a far smaller grant than publishing does. With
`--token-stdin` the piped value is used and nothing else is consulted;
otherwise grim tries, in order:

1. `GRIM_RATE_TOKEN`
2. a host-matched CI token
3. the forge CLI's stored credential — `gh auth token --hostname <host>` or
   `glab auth token --hostname <host>`
4. refuse with exit `80`, naming the ladder and never a token

Under `--token-stdin`: a terminal on stdin is refused (`64` — the flag
exists for piping), more than one line is refused (`64`), and empty or
whitespace-only input is an auth error (`80`) that deliberately does
**not** fall through to the ladder above. A caller that said it was
supplying a credential must not silently vote as somebody else.

`github` votes against `api.github.com` and `gitlab` against `gitlab.com`
unless the index declares its own host in `providers.rating_host`, which
takes precedence. Comparison is exact: IDNA-normalised, ASCII-lowercased,
port included, **no suffix matching**. A loopback host — `localhost`,
`127.0.0.1` or `[::1]`, on any port — is contacted over plain HTTP; every
other host over HTTPS, and a *remote* index may not declare a loopback one.

Against an index-declared host, the two credentials grim does **not** look
up itself — `GRIM_RATE_TOKEN` and `--token-stdin` — must be accompanied by
`--token-host <host>` naming that exact host, or the run exits `80` before
the credential is read. The host-matched rungs (CI token, `gh`/`glab`
stored credential) are unaffected: they only ever resolve a credential for
the host being contacted. A bare `--dry-run` is never gated — it reads no
credential and is how a client learns the host in the first place.
See [voting against a private instance](./ratings.md#voting-host).

### JSON output {#rate-json}

`--format json` emits one object (not an array — `rate` concerns exactly
one reference) with every field always present, nullable ones as explicit
`null`:

```json
{
  "ref": "ghcr.io/acme/skills/code-review",
  "action": "up",
  "up": 43,
  "url": "https://github.com/acme/index/discussions/117",
  "provider": "github",
  "host": "api.github.com",
  "host_source": "default",
  "viewer_up": true
}
```

`up` is the count after the mutation, or the sidecar's count under
`--dry-run`; it is `null` when the forge's payload carries no total
(GitLab's emoji toggle reports state, not a count). `provider` is the value
the index published, verbatim, so an unrecognised one is still reported.
`host` is `null` when no host resolves — which under `--dry-run` is exactly
the "grim cannot vote here" answer a client needs *before* it picks an
authentication provider. `host_source` is `"default"` (the built-in
per-provider host) or `"index"` (`providers.rating_host`, declared by the
index and accepted), and `null` exactly when `host` is — see [the
`host_source` field](./json-interface.md#rate-host-source). `viewer_up` is
the tri-state described
[above](#rate-viewer-state) and is `null` on every path except
`--dry-run --token-stdin`.

Plain output is a single-row table:
`Ref | Action | Up | Voted | Provider | Host | Url`, with `-` for an absent
value. The `Voted` cell renders `yes` / `no` / `-` — three distinct cells,
so unknown never reads as "no". The `Host` cell reads `<host> (index)` when
the index declared it; there is no eighth column, because the
machine-readable answer is `host_source`.

### Exit codes {#rate-exit-codes}

| Code | Condition |
|---|---|
| `0` | Vote registered or retracted · confirmation declined (nothing mutated) · `--dry-run` resolved |
| `64` | `--up` with `--remove`; malformed reference; non-interactive without `--yes`; `--token-stdin` without `--yes`; `--token-stdin` with a terminal on stdin, or with multi-line input |
| `65` | The row carries no rating; the index declared no rating provider; the provider is one grim cannot vote through (the raw value is in the message); the forge answered with a populated GraphQL `errors` array |
| `69` | The forge is unreachable, answered 5xx, or applied a secondary rate limit |
| `79` | The reference resolves to no catalog row, or the vote target no longer exists |
| `80` | No credential resolved; the forge rejected it (401/403); `--token-stdin` received empty input; `--token-host` did not match the resolved host; an injected credential (`--token-stdin` or `GRIM_RATE_TOKEN`) was heading for an index-declared host without `--token-host` naming it — all refused **before** the token reached any header |
| `81` | `--offline` (or `GRIM_OFFLINE`) — a vote hard-refuses rather than degrading silently. Bare `--dry-run` is exempt; `--dry-run --token-stdin` is **not**, because it makes a query. |

Note what is *not* in that table: a failed or unauthorised
[viewer-state query](#rate-viewer-state) does not fail the command. It
reports `viewer_up: null` and exits `0` — the read is best-effort by
design, and the honest answer to a read you could not complete is
*unknown*.

```sh
grim rate ghcr.io/acme/skills/code-review
grim rate acme/skills/code-review --remove --yes
grim rate acme/skills/code-review --dry-run --format json
printf '%s' "$FORGE_TOKEN" | grim rate acme/skills/code-review \
  --dry-run --token-stdin --format json          # + viewer_up, no --yes needed
printf '%s' "$FORGE_TOKEN" | grim rate acme/skills/code-review \
  --token-stdin --token-host ghe.corp.example --yes
GRIM_RATE_TOKEN=… grim rate acme/skills/code-review \
  --token-host ghe.corp.example --yes            # required when the index declared the host
```

## grim fetch {#fetch}

`grim fetch <ref>` resolves an artifact and prints its content — **use ≠
install**: nothing is materialized, no state is touched. It is the CLI
port of the MCP [`grim_fetch` tool](#mcp): canonical (as-authored) content
by default, a `--vendor <client>` projection (any supported client — see
the [client compatibility matrix](./clients.md#matrix)), or one
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

An MCP server fetched with `--vendor copilot` at project scope previews one
entry only: the VS Code `.vscode/mcp.json` one. `grim install` also writes
the Copilot CLI's `.github/mcp.json` entry, in the global
`mcp-config.json` shape, which `--global --vendor copilot` shows.

`--format json` emits the full fetch report: `{ref, digest, kind, name,
vendor, path?, content, encoding?, truncated?, files?, pointer?, warnings?}`
(the MCP payload shape — empty/default fields omitted; `encoding` is present
only for a base64 binary `--path` file). Warnings print to stderr, keeping
stdout a pure payload.

Unlike the MCP tool — which truncates documents at 256 KiB for tool-result
budgets — the CLI **never truncates** a printed payload. Two ceilings guard
the download instead, with different failure modes: the manifest's
declared layer size is checked against the 32 MiB limit *before* download
(a cheap reject for an honestly-oversized layer, exit 65), and that same
declared size then bounds the *actual* streamed bytes — a registry that
serves more than it declared aborts mid-transfer into a data error (exit
65) rather than growing an unbounded body in memory. A missing repository
or tag — the artifact itself, or (with `--description`) its companion —
is a not-found failure (exit 79); an offline run against an uncached
reference exits 81 instead, since the reference may exist and only the
network to confirm it is unreachable.

A [dependent artifact](./artifacts.md#requires-layer) prints its content
layer only; the dependency list is not part of the payload, and
[`grim describe`](#describe) reports it. A manifest that declares a feature
this grim does not know exits 65 with reason `unsupported-artifact-feature`.

The 32 MiB limit bounds **decoded content**. A [gzip layer](./artifacts.md#layers)
is decompressed under the same limit, so a small compressed layer that expands
past 32 MiB exits 65 and is never truncated. The MCP `grim_fetch` tool and the
description companion follow the same rule.

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
"base64"` only for a binary member), bounded by the same 32 MiB layer gate
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
 version, license, repository, revision, created, authors, vendor, url,
 documentation, compatibility, support, keywords, deprecated, replaced_by,
 tags, annotations, compression, requires}
```

`kind` is `null` for a foreign / non-Grimoire manifest (describe never
hard-errors on one). `support` is an object `{issues, chat, contact,
security}` carrying the repository's
[support channels](./publishing.md#support-channels); it is read from the
description companion's manifest — one extra fetch, and only when
`has_description` is already true — so a repository without a companion costs
nothing and reports four `null`s. `compatibility` is a skill's authored
editor/runtime hint and is `null` for every other kind. `has_description` is an always-present boolean —
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
or `null`. `requires` lists the artifact's [declared dependencies](./artifacts.md#requires)
as `{kind, name, id}` entries in canonical order, `[]` when it declares none,
and `null` when the manifest is foreign. Reading it costs one small extra
blob fetch, and only for a dependent artifact.
`compression` is `"gzip"` for a [gzip-compressed
layer](./artifacts.md#layers), `"none"` for any other single layer grim can
read, and `null` for a layer it cannot read. See the
[field reference](./json-interface.md#describe-compression). Plain output is a
flat key/value table (like [`grim context`](#context)) with `keywords` and
`tags` comma-joined, plus one `compression` row, which shows `-` when
`compression` is `null`, and one `requires` row listing the dependencies.

```sh
grim describe skills/code-review
grim describe ghcr.io/acme/skills/code-review:1.2.0 --format json | jq .tags
```

Errors follow the fetch taxonomy: a missing repository is a not-found
failure (parity with `grim fetch`), an auth failure exits 80, an offline run
that cannot reach the registry exits 81, and an unreachable registry exits
69.

## grim export plugin {#export-plugin}

Every command above installs pinned artifacts *through* grim — a lockfile,
an install record, a rolling `grim update`. That reach stops at grim's own
door. A teammate who has not installed grim gets nothing from your
`grimoire.toml`, no matter how carefully it is pinned.

Two client families solve the same handoff problem their own way.
[Claude Code][claude-code] loads a self-contained directory carrying a
`.claude-plugin/plugin.json` manifest. It installs from a folder, a zip
upload, or a git marketplace — see [Plugins][claude-code-plugins] and
[Upload a plugin][claude-plugin-upload]. [GitHub Copilot][copilot],
[Codex][codex], and [Cursor][cursor] implement the same idea against the
vendor-neutral [Agent Plugins 1.0][agent-plugins] specification instead.

`grim export plugin` renders your already-locked artifacts into either
shape — a directory or a `--zip` archive. A teammate without grim installs
it exactly as their client documents, no OCI registry involved on their
end. Rendering reuses the same per-client materializer `grim install`
calls: a skill exported for Claude Code is byte-for-byte the `SKILL.md`
`grim install --client claude` would have written.

```sh
grim export plugin ghcr.io/acme/skills/code-review:1 --client claude --zip -o dist
grim export plugin --plugin team --client codex -o dist
grim export plugin --project --client claude -o dist
```

### Ad-hoc vs. declared {#export-plugin-modes}

| Refs | `--name` | `--plugin` | Behavior |
|---|---|---|---|
| 1 | absent | — | Ad-hoc plugin; name defaults to the reference's binding (its last path segment, or a local path's packed intrinsic name) |
| 1 | set | — | Ad-hoc plugin named `--name` |
| ≥2 | set | — | Ad-hoc plugin merging every reference |
| ≥2 | absent | — | Exit `64` — `--name` is required for more than one reference |
| 0 | set | any | Exit `64` — `--name` applies only to an ad-hoc or a `--project` export |
| 0 | — | `P…` | Declared plugin(s) named by `--plugin` (repeatable), read from `--marketplace` (default `./marketplace.toml`); an unknown name exits `79` — but a manifest declaring zero plugins exits `65` first, before `--plugin` is even checked against it |
| 0 | — | none | Every plugin the manifest declares; a manifest declaring none exits `65` with a hint toward `<ref>… --name` |
| ≥1 | — | set | Exit `64` (refs and `--plugin` conflict at the flag level) |

An **ad-hoc** export takes references directly on the command line.
Nothing is read from or written to a manifest or lock, and no
[`marketplace.lock`](./configuration.md#marketplace-toml) appears anywhere.
A **declared** export instead reads a
[`marketplace.toml`](./configuration.md#marketplace-toml)'s `[plugins.<name>]`
tables. It touches only the plugins whose declaration changed since the last
export, or that have no lock entry yet, then writes `marketplace.lock` back.

That reconcile follows [`grim lock`](#lock), not `grim update`. A new include
resolves, and an unchanged include keeps its pin even after its tag moved.

A bundle the plugin reaches keeps its pin too, by include, by `requires` or through
another bundle. `marketplace.lock` has no bundle row, so its digest sits in the
`bundle_tag` of the rows below it, and the reconcile reads the bundle at that digest.

A plugin that records no digest, written or resolved again by grim 0.15.1, follows
the tag on its next reconcile. One warning names the bundle and the rows that
changed, and that write records the digest. A member it still lists keeps its pin
while the tag that pin records matches the member. A member another request shares
at a different tag can resolve again, because the file has nowhere to record the
second request.

A plugin whose entries record no tag, written by grim 0.15.1 or earlier, resolves
an unchanged include once, and a warning names the plugin. An offline export warns
too, naming the plugin and each row a tag read moved, added or dropped.

Offline, a recorded bundle digest the store lacks exits `81` and leaves the lock as
it was. The tag cache never stands in for it. A plugin grim 0.15.1 wrote records no
digest, so an offline export reads its bundle tags from the local cache. With those
tags cached it exits `0`, where 0.15.1 exited `81`.

A kept pin or a recorded bundle digest whose manifest is gone exits `79`, naming the
[`grim update --marketplace`](#update-marketplace) command for that plugin.

### Flags {#export-plugin-flags}

| Flag | Effect |
|------|--------|
| `[REFS]…` | Positional references to export as one ad-hoc plugin |
| `--name <NAME>` | Plugin name for an ad-hoc or `--project` export; required with more than one reference |
| `--project [PATH]` | Export a project's `grimoire.lock` as one plugin, without resolving anything; `PATH` is its directory or `grimoire.toml`, else the project is discovered — see [export your project](#export-plugin-project); conflicts with `REFS`, `--plugin` and `--marketplace` |
| `--plugin <NAME>` | Export only this declared plugin (repeatable); conflicts with `REFS` |
| `--marketplace <PATH>` | The manifest declaring the plugins (default `./marketplace.toml`); conflicts with `REFS` |
| `--client <NAME>` | Client(s) to export for, comma-separated and repeatable (default: the config `clients` option, then `agents`); a list naming no client (`--client ""`) exits `64` |
| `--zip` | Write `<name>.<client>.zip` instead of a directory |
| `-o, --output <DIR>` | Directory the plugins are written into (default `.`, created if absent) |
| `--version <SEMVER>` | Plugin version base — defaults to the declared `version`, then the single ad-hoc reference's version annotation, then `0.0.0`; grim always appends a content-hash suffix |
| `--description <TEXT>` | Plugin description base — overrides the declared `description`, the project's `[plugin].description`, and the single ad-hoc reference's description annotation; longer than 500 characters exits `65` (see [the description cap](#export-plugin-description)) |
| `--logo <PATH>` | Plugin logo, `.png` or `.svg` up to 1 MiB — overrides the declared `logo`; see [logo](#export-plugin-logo) |
| `--force` | Replace existing outputs instead of refusing them |

`--client` accepts the same client names as
[`grim install`](#install) — see the [family table](#export-plugin-families)
below for which ones this command can render at all. Every other global flag
(`--offline`, `--registry`, `--config`, `--global`, …) behaves as documented
under [Global options](#global-options).

### Export your project {#export-plugin-project}

`--project` shares exactly what your project runs. It reads the
project's `grimoire.toml` and `grimoire.lock` and renders those pins as
one plugin. Without a value, it finds the project the way every project
command does. A value names the project directory or its `grimoire.toml`,
and cannot be combined with `--global` or `--config` (exit `64`). No
tag is re-resolved, so the plugin carries the digests your team installed,
even if a tag has moved since. Local path members are re-packed and must
still match their locked hash; a changed one exits `65` and asks for
`grim lock` in the project.

```sh
grim export plugin --project --client claude -o dist
grim export plugin --project ../team --client codex --zip -o dist
```

The lock must be fresh: declarations edited since the last `grim lock` exit
`65` (`grim lock` first), and a missing lock exits `79`. Nothing is
written besides the plugin output. Name and metadata come from flags,
then from the optional [`[plugin]` table](./configuration.md#plugin-table)
in `grimoire.toml`. With neither `--name` nor `[plugin].name` the export
exits `64`.

A [`marketplace.toml`](./configuration.md#marketplace-toml) can point one
of its plugins at a project with `project = "<dir>"`. The project's lock
supplies the pins, its `[plugin]` table any field the marketplace leaves
unset, and `marketplace.lock` records nothing for that plugin.

### Dependencies {#export-plugin-dependencies}

An artifact that [declares dependencies](./artifacts.md#requires) exports as
its whole closure. The dependencies are copied into the plugin next to the
artifact, so the plugin installs on its own: the manifest carries no
`dependencies` key and a client never resolves anything.

- A dependency published on another registry is copied the same way. Export
  needs read credentials for **every** registry in the closure, not only the
  one you named.
- A dependency shared by two exported plugins appears in each of them.
- A manifest that declares a feature this grim does not know exits `65` with
  reason `unsupported-artifact-feature` before anything is placed.

### Client families {#export-plugin-families}

Export renders into one of two on-disk shapes, never a client's own
[render layout](./stability.md#unstable):

| Family | Clients | Manifest | Members |
|---|---|---|---|
| Claude | `claude`, [`droid`][droid], [`junie`][junie], [`openclaw`][openclaw], [`qoder`][qoder] | `.claude-plugin/plugin.json`; Qoder: `.qoder-plugin/plugin.json` | `skills/<name>/`, `agents/<name>.md`, `.mcp.json` |
| Agent Plugins | [`copilot`][copilot], [`codex`][codex], [`cursor`][cursor], `agents` | `plugin.json` (with a `$schema`); Cursor also `.cursor-plugin/plugin.json` | `skills/<name>/`, `mcp.json` |

A client outside both families, or a client with no plugin surface at all
(explicitly named with `--client`), exits `78`. `--client` left unset falls
back to the config `clients` option and then to the generic `agents`
client, which always resolves to Agent Plugins.

The family fixes the manifest shape, but each member is still rendered
through the named client's own [`grim install`](#install) path. A plugin
for `claude` therefore gets the same file bytes an install for `claude`
would, including any vendor-specific rendering such as `claude.*` metadata
lifted to native keys. Name the client the plugin is for. Today `copilot`,
`codex`, `cursor`, and `agents` render the same member bytes. They differ in
the output name, the README's omitted-members line, and one file: a `cursor`
plugin also carries `.cursor-plugin/plugin.json`, a second manifest in the
Claude format, at the path Cursor's own plugin format looks for. A `qoder`
plugin is a Claude plugin whose manifest sits under `.qoder-plugin/`, and it
is built as a `claude` export is, MCP placeholder renames included.
Claude-family clients differ from each other in what they admit (table
below).

### What each client admits {#export-plugin-admission}

Not every member kind survives every client, the same faithfulness rule
[`grim install`](#install) applies at materialization time. A rule never
survives any client — neither plugin format has a place for one
(`no-format-surface`) — and a bundle never reaches this gate at all:
resolution expands it into its members first.

| Client | Family | Emits | Omits |
|---|---|---|---|
| `claude` | Claude | skill, agent, mcp | rule (`no-format-surface`) |
| [`droid`][droid] | Claude | skill, agent, mcp | rule (`no-format-surface`) |
| [`junie`][junie] | Claude | skill, agent, mcp | rule (`no-format-surface`) |
| [`openclaw`][openclaw] | Claude | skill | agent, mcp (`client-declined`); rule (`no-format-surface`) |
| [`qoder`][qoder] | Claude | skill, agent, mcp | rule (`no-format-surface`) |
| [`copilot`][copilot] | Agent Plugins | skill, mcp | agent (`no-format-surface`); rule (`no-format-surface`) |
| [`codex`][codex] | Agent Plugins | skill, mcp | agent (`no-format-surface`); rule (`no-format-surface`) |
| [`cursor`][cursor] | Agent Plugins | skill, mcp | agent (`no-format-surface`); rule (`no-format-surface`) |
| `agents` | Agent Plugins | skill, mcp | agent (`no-format-surface`); rule (`no-format-surface`) |

An agent is omitted `no-format-surface` for every Agent Plugins client —
that family's manifest has no agent key at all, so this is never
`client-declined` even where the client would install an agent through
`grim install`. MCP, by contrast, is admitted for every Agent Plugins
client regardless of that client's own install-time MCP support: the
`mcp.json` shape belongs to the family, not the client (`agents` declines
MCP on install yet still emits it here). Within the Claude family, MCP and
agent support instead follow the client's own [install-time kind
support][clients-matrix] — which is why `openclaw` drops both, while `junie`,
`qoder` and `droid` keep both (Droid translates a Claude-format plugin's `agents/`
and `.mcp.json` when it installs one). An agent whose name the client's
grammar rejects is omitted as `not-representable`. An omitted member is never
an error. It is named in the plugin's `README.md` and in the JSON
report's `omitted` array. An export whose every member is omitted for a
given client exits `65` (`EmptyPlugin`) rather than writing an empty
plugin.

A hook is never exported, because it needs grim at run time. It is omitted
`not-representable` for the Claude family and `no-format-surface` for Agent
Plugins, with one warning per hook per plugin. See [hooks](./hooks.md#export-declines).

A [settings package][settings] is never exported either, because no plugin
format carries harness settings. It is omitted `no-format-surface` for every
client and never fetched. One warning per member per plugin names it, the
README lists it under each client's omissions, and the JSON report lists it
in `omitted`. A plugin whose only members are settings packages exits `65`.

Junie still omits an agent `not-representable` when its name starts with a
digit or contains a `.`, the same names a Junie install skips.

An MCP descriptor using a transport or field the target shape cannot
express is omitted `not-representable` rather than silently dropped. For
an **Agent Plugins** export this covers `oauth`, the `ws` transport, or a
`${…}` reference inside `command`, `url`, an env key, or a header. The
spec performs no expansion there. In `args`, env values and `cwd` the
spec expands only `${PLUGIN_ROOT}` and `${PLUGIN_DATA}`. grim renames
`${CLAUDE_PLUGIN_ROOT}` and `${CLAUDE_PLUGIN_DATA}` to them. Any other
`${VAR}` there is exported with a warning, because a client may pass it
through literally. A `cursor` export writes `${CURSOR_PLUGIN_ROOT}` for the
root, because Cursor does not expand `${PLUGIN_ROOT}`. Cursor has no data
folder variable, so `${PLUGIN_DATA}` is exported with a warning.

A `claude` export does the reverse, and so does a `qoder` export. It renames `${PLUGIN_ROOT}` and
`${PLUGIN_DATA}` to `${CLAUDE_PLUGIN_ROOT}` and `${CLAUDE_PLUGIN_DATA}` in
every field, `url` and headers included. Claude expands its placeholders
there. `grim install` never renames.

Claude family MCP has its own, narrower decline. A `qoder` export declines
nothing here, because it builds each entry as `claude` does. Junie refuses a
descriptor carrying OAuth or an env reference. Droid refuses a `${VAR}`
reference in `command`, `args` or `url`, and an `oauth` block with
anything beyond a literal `client_id`. A reference in `env` or `headers`
is fine for Droid. The [team-plugin guide](./guides/team-plugin.md) covers the practical
fallout for stdio servers.

### Output naming and placement {#export-plugin-output}

Each `(plugin, client)` pair writes to `<DIR>/<plugin>.<client>` (a
directory) or `<DIR>/<plugin>.<client>.zip` (with `--zip`). `<DIR>` is
`--output`, the current directory by default. By default, `grim export
plugin` **refuses** to replace an existing output at exit `65`
(`untracked-destination`) and places nothing. `--force` replaces it
atomically instead. A zip is renamed over the old file; a directory's
previous contents are swapped out and removed. Neither route ever follows
a symlink at that path.

Two runs against the same manifest, lock, and grim build reproduce
byte-identical output: the same files at the same paths with the same
bytes. That holds independent of time zone, working directory, or
directory-iteration order.

Every output stages in a hidden `<DIR>/.grim-export-*` directory before
being placed. A run interrupted mid-flight (`SIGINT`, a crashed process)
can leave that staging directory behind; it is not automatically reaped
on the next run. Before deleting it, check for a `.replaced-<name>`
entry — the previous output an interrupted `--force` replace moved
aside. Move it back to `<name>` if that destination is missing, then
delete the staging directory.

The `version` grim assembles is `<base>+<12 hex>`. `base` is `--version`,
else the declared `version`. Failing both, an ad-hoc single reference
falls back to its own version annotation, else `0.0.0`.

The suffix is a content hash of the plugin tree as rendered for one client:
every file's path, its executable bit, and its SHA-256, taken with each
manifest written at `base`. It changes whenever any file of the tree does.
That covers a member's digest, a rename, a `description` or logo edit, and a
grim release that renders a skill differently. It can also differ between
clients for the same plugin, because each client's tree has its own manifest
path, README and omitted members. The same tree always gets the same
version, so a re-export that changes nothing changes no version.

Only the `<base>+<12 hex>` grammar is frozen. What feeds the hash is not: a
later release may change the inputs, and every plugin's suffix moves with it
(see [Stability](./stability.md#unstable)). The suffix moved once already,
when it stopped hashing member digests
([upgrading](./upgrading.md#plugin-version-move)).

### JSON report {#export-plugin-json}

`--format json` emits the [enveloped shape](./json-interface.md#shapes-items)
`{"items": [...]}`, one object per `(plugin, client)` pair actually written:

```json
{
  "items": [
    {
      "plugin": "team",
      "client": "claude",
      "family": "claude",
      "format": "zip",
      "path": "/abs/path/dist/team.claude.zip",
      "version": "0.0.0+c2cfaefc6f09",
      "members": [
        {"kind": "skill", "name": "team-plan", "lock_name": "team-plan", "pinned": "ghcr.io/acme/team-plan@sha256:…"}
      ],
      "omitted": [
        {"kind": "rule", "name": "team-reviewer", "reason": "no-format-surface"}
      ]
    }
  ]
}
```

`members`/`omitted` are sorted by kind then name; `family` is `claude` or
`agent-plugins`; `format` is `dir` or `zip`; `path` is the absolute final
path. See [the enveloped reports table][json-shapes-items] for the full
field reference, alongside every other command's `--format json` shape.

### Exit codes {#export-plugin-exit-codes}

| Situation | Code |
|-----------|------|
| Success | `0` |
| `≥2` refs without `--name`; refs combined with `--plugin`/`--marketplace`; `--project` combined with refs, `--plugin` or `--marketplace`, or without any plugin name; `--name` given with 0 refs and no `--project`; or an invalid `--name`, or an invalid plugin name derived from a single reference | `64` |
| A missing or malformed manifest, or a malformed `--version`; a `--description` or declared `description` too long for the cap; a stale `grimoire.lock` behind `--project` or a `project` plugin; a locked local member that changed on disk; a manifest declaring zero plugins, regardless of `--plugin` (it is checked first); a bad rename or a stale reference a rename left behind; an unsafe entry name; an admitted member set that is empty for a client; or an existing output without `--force` | `65` |
| Two references disagreeing on the same member's identity, or a `--client` (explicit or unrecognized) naming a client with no plugin format at all | `78` |
| A named plugin, a `<plugin>`/`<plugin>:<member>` selector, or an include's tag or manifest, not found; a project's `grimoire.toml` or `grimoire.lock` missing | `79` |
| Registry unreachable | `69` |
| Missing or rejected registry credential | `80` |
| An include's floating tag needs the network under `--offline` — this includes a pinned, already-cached registry member, since export has no manifest cache; only a local path source resolves offline (the same limitation `grim install --offline` has) | `81` |
| Concurrent write on the manifest's lock file | `75` |
| I/O failure (unwritable output, non-hardlink filesystem, a symlink under the staged root, …) | `74` / `77` |

Export-owned failures classify as above. Every other failure — a bad
reference, a bundle conflict, a stale lock — propagates from the resolver
or installer with its own existing exit code.

### Description and README {#export-plugin-description}

`plugin.json`'s `description` is the base text alone: `--description`, else
the declared `description`, else the single ad-hoc reference's description
annotation. With no base it is grim's pointer sentence, `Packaged by grim
(https://grimoire.rs); install grim for pinned, updatable installs.`

The description is at most 500 characters, counted in UTF-16 code units.
A base you wrote that is longer exits `65`: you can shorten it, so grim
refuses rather than cut it. A publisher's annotation text is cut to fit
instead, and grim warns on stderr. The cut ends at the last whole sentence
when that keeps at least half the text, otherwise at a word boundary
followed by `…`.

Each plugin also carries a generated `README.md`. It opens with the
declared `readme` file, or else the plugin name, the logo when there is
one, shown 96 pixels wide, and the full uncut base text. Then come the
members omitted for that client and the pointer sentence. A declared
`changelog` file is copied to `CHANGELOG.md`. Both files are Markdown, UTF-8
and at most 1 MiB, else the export exits `65`.

### Logo {#export-plugin-logo}

`--logo`, else the declared `logo`, names a `.png` or `.svg` file of at
most 1 MiB. An ad-hoc export of a single reference with neither falls back
to that repository's published logo, the `logo.svg` or `logo.png` of its
[description companion](./publishing.md#description-companion). That
companion tag is not pinned, so the logo is whatever it holds at export
time. A missing companion adds no logo, and an unreadable one only warns.
grim copies the logo to `assets/logo.png` or `assets/logo.svg` in
every exported plugin and shows it at the top of the README. A missing
file, another format, or a larger file exits `65` and places no
output.

The plugin formats differ in whether a client reads it. An Agent Plugins
`plugin.json` points at it under the Codex namespace,
`extensions."com.openai".interface.logo`, which Codex shows. Claude's
`plugin.json` has no logo field, so a Claude-family plugin carries the
file without a manifest reference to it.

A member's own logo is separate. A skill directory that ships `logo.png`
or `logo.svg` keeps it at `skills/<name>/` in every export, as its whole
tree is copied. Codex reads a per-skill icon from `agents/openai.yaml` in
the skill directory, but its plugin upload rejects a package whose skills
declare one there. grim therefore writes no per-skill icon; a skill's own
`agents/` directory is copied like the rest of its tree.

Rename, description, and version-annotation rules are covered in full in
the [`marketplace.toml` reference](./configuration.md#marketplace-toml);
a worked, end-to-end handoff is the [team-plugin guide](./guides/team-plugin.md).

`--marketplace` under `grim update` (above) rolls a declared plugin's pins
forward without exporting anything. It is the way to keep a shipped
plugin's *next* export current, without re-running `grim export plugin`
by hand first.

To serve the same plugins from a git repository that clients add once, see
[`grim export marketplace`](#export-marketplace).

## grim export marketplace {#export-marketplace}

[`grim export plugin`](#export-plugin) hands one plugin over as a folder or a
zip. A team or a public audience wants a standing address instead. They add
it once and receive updates. Every plugin-aware client reads such an address
as a git repository. The repository holds that client's marketplace file, at
a path it chose and in its own dialect.

`grim export marketplace` regenerates that repository from the plugins a
[`marketplace.toml`](./configuration.md#marketplace-toml) declares. It writes
one marketplace file per client, one plugin tree per plugin and client, and
a root `README.md` that tells visitors how to add the marketplace.
You commit the result. A consumer adds the repository with their client's own
commands and needs no grim. The [consumer guide](./guides/use-a-marketplace.md)
walks through each client.

```sh
grim export marketplace
grim export marketplace --marketplace tools/marketplace.toml -o ../acme-marketplace
```

Each run resolves the declared plugins the way [`grim export plugin`
--marketplace](#export-plugin-modes) does. Only the plugins whose declaration
changed are reconciled, and `marketplace.lock` is written beside the
manifest. Then every tree is rendered and the output is made to match. grim
keeps no record of earlier runs. It decides what it owns from the repository
itself ([ownership](#export-marketplace-ownership)).

The manifest must carry a [`[marketplace]`](./configuration.md#marketplace-table)
table naming the marketplace and its owner. Without one the command exits
`65` and names the table.

Each plugin tree follows the [`grim export plugin`](#export-plugin) admission
rules, so a [hook](./hooks.md#export-declines) member is omitted from every
tree. A plugin left with only hook members exits `65`. A [settings][settings]
member is omitted from every tree the same way, and a plugin left with only
settings members exits `65` too.

### Flags {#export-marketplace-flags}

| Flag | Effect |
|------|--------|
| `--marketplace <PATH>` | The manifest declaring `[marketplace]` and the plugins (default `./marketplace.toml`); its file name must not start with `.grim-export` |
| `-o, --output <DIR>` | The repository root written into (default: the manifest's directory, created if absent); the manifest's lock stays beside the manifest |
| `--force` | Adopt paths grim does not own yet — see [ownership](#export-marketplace-ownership) |

Everything else the plugin export takes is declared in the manifest, so clap
rejects it with exit `64`: positional references, `--name`, `--project`,
`--plugin`, `--zip`, `--version`, `--description`, `--logo` and `--client`.
The clients come from `[marketplace].clients`. Every other global flag
behaves as documented under [Global options](#global-options).

### Output layout {#export-marketplace-layout}

Each selected client gets one marketplace file, at the path its client reads,
and one tree per plugin at `./<client>/<plugin>`:

| Client | Marketplace file | Plugin tree | In the default set |
|---|---|---|---|
| `claude` | `.claude-plugin/marketplace.json` | `./claude/<plugin>` | yes |
| [`copilot`][copilot] | `.github/plugin/marketplace.json` | `./copilot/<plugin>` | yes |
| [`codex`][codex] | `.agents/plugins/marketplace.json` | `./codex/<plugin>` | yes |
| [`qoder`][qoder] | `.qoder-plugin/marketplace.json` | `./qoder/<plugin>` | yes |
| [`cursor`][cursor] | `.cursor-plugin/marketplace.json` | `./cursor/<plugin>` | no, opt-in only |

A table without `clients` selects the four defaults. Cursor is never in the
default set. [`droid`][droid], [`junie`][junie] and [`openclaw`][openclaw]
own no marketplace file, because they read Claude's. Naming one in `clients`
exits `65` and says to select `claude`.

Beside the client files, grim writes a root `README.md`
([described below](#export-marketplace-readme)).

A Droid user is served by the Claude file unless the repository also holds
`.factory-plugin/marketplace.json`, which Droid reads first. grim warns when
that file exists. Qoder likewise reads `.qoder-plugin/marketplace.json`
before Claude's. grim warns about a foreign copy of it in a repository that
does not select `qoder`.

Each tree is what [`grim export plugin`](#export-plugin) renders for that
client: the same [families](#export-plugin-families), the same
[admission](#export-plugin-admission) rules, the same
[version](#export-plugin-output). The marketplace file lists every plugin the
client can carry as `{name, source, version, description}` plus the
[profile fields][plugin-metadata] that harness's format defines. `source` is
`./<client>/<plugin>`, and `version` and `description` equal the tree's own
manifest. The file's own keys are `name`, `owner` (`name`, an optional
`email`, and for Claude and Qoder an optional `url`), `metadata.description`
when the table sets one, and `plugins`. It is pretty-printed JSON with a
trailing newline and no `$schema`.

A plugin with no member a client's format can carry is left out of that
client's file and reported as an `empty` row, with a warning. A plugin that no
selected client can carry exits `65`.

### Root README {#export-marketplace-readme}

A git host shows the repository's `README.md` as its front page, so grim
writes one at the output root. It holds the marketplace name, description
and owner. Each selected client gets a subsection with the commands that add
the marketplace and install a plugin. A table lists the plugins with their
descriptions and versions. The README ends with a note on private
repositories and a link to the
[consumer guide](./guides/use-a-marketplace.md). Its first line is
`<!-- generated by grim export marketplace; edit marketplace.toml instead -->`.
The README is deterministic, and the version column omits the tree hash.

The commands use [`[marketplace].repository`](./configuration.md#marketplace-table),
the clone URL consumers add. Without it the README shows the placeholder
`<clone URL of this repository>` and a note to set the key. The URL is never
written into a marketplace file or a plugin tree.

| Client | Add | Install |
|---|---|---|
| `claude` | `claude plugin marketplace add <repository>` | `claude plugin install <plugin>@<name>` |
| `copilot` | `copilot plugin marketplace add <repository>` | `copilot plugin install <plugin>@<name>` |
| `codex` | `codex plugin marketplace add <repository>` | `codex plugin add <plugin>@<name>` |
| `qoder` | `qoder plugins marketplace add <repository>` | `qoder plugins install <plugin>` |
| `cursor` | An administrator imports the URL as a Team Marketplace, or runs `cursor-agent plugin marketplace add <repository>` | none |

For Qoder, grim appends `.git` to the URL when its host is not `github.com`
and it does not end in `.git` already. The README sits outside `./<client>/`,
so dropping a client or renaming the marketplace never removes it.

### Ownership and `--force` {#export-marketplace-ownership}

The repository holds files grim does not write, so grim decides what it may
change by convention, not from a recorded state:

- A selected client owns `./<client>/` and its marketplace file. The file is
  grim's when it is absent or its `name` equals `[marketplace].name`.
- Inside an owned `./<client>/`, anything that is not a declared plugin is
  removed. A stray file is named on stderr. A declared plugin that becomes
  `empty` for a client loses its existing `./<client>/<plugin>` tree the
  same way, and the `empty` row is the only report of that deletion.
- A client that left `clients` is dropped when its file is grim's: the
  directory goes first, the file last. Its empty parent directories are
  pruned, except the top-level `.github/` and `.agents/`, which hold other
  tooling's files.
- Every other path in the repository is left alone, including the
  directory and file of an unselected client that grim does not own.
- The root `README.md` is grim's when it is absent or its first line is the
  generated-by comment. Any other `README.md` is left untouched with the
  warning `README.md is not grim's; leaving it`. That is never an error and
  `--force` does not change it. A hand-written README is therefore safe, and
  a left-alone README is not listed in the report.

Without `--force`, a run **refuses** at exit `65` (`untracked-destination`)
and writes nothing in two cases. One is a selected client's marketplace file
that belongs to another marketplace or is not a marketplace document. The
other is a non-empty `./<client>/` with no marketplace file.

`--force` adopts those paths. It overwrites the foreign file and deletes
everything else in `./<client>/`, so read the refusal's path list first.
Renaming `[marketplace].name` is the common way into this refusal: every
existing file now names the old marketplace. grim warns about this before it
refuses, and `--force` adopts the files under the new name. Consumers who
added the marketplace before the rename must then re-add it under the new
name.

grim writes marketplace files first, then trees, then removals, then the
lock. A crash in between leaves a layout the next run owns and repairs. A
re-run with unchanged inputs and the same grim build rewrites nothing. Every
tree and file it wrote before reports `unchanged`, and a plugin and client pair
that is `empty` stays `empty`.

A symlink, a non-directory, or on Windows a reparse point, anywhere on the way
to an owned path, exits `65` before anything is written. grim never follows
one while removing. A `logo`, a `path:` member or a `project` directory must
lie inside the manifest's directory and may not be a symlink (`65`). A
project's `grimoire.toml` and `grimoire.lock` may not be symlinks either
(`65`), and neither may the manifest or its lock.

Nothing the export reads may lie under a path it owns, or export would
replace its own input (`65`). That covers the manifest and its lock, a
plugin's `project` directory, a `path:` include, a logo, and the `path:`
members and logo a `project` plugin reads from its own project. The owned
paths are `./<client>/` and each client's marketplace file.
The match ignores ASCII case, so `Claude/` counts as `claude/`.

A plugin name becomes a directory name, so it must be portable. A Windows
reserved device name such as `con`, `aux`, `nul` or `com1` exits `65` as an
unsafe entry.

Two runs cannot overlap. grim locks the manifest's lock file and a sidecar
`.grim-export.lock` in the output root. A contended lock exits `75`. Keep the
sidecar out of git.

Every tree stages in a hidden `<root>/.grim-export-*` directory before it is
placed. One left behind by an interrupted run, or kept as the recovery
backup of a failed replace, is never swept. Each run warns and names it. Look
for a `.replaced-<plugin>.<client>` entry inside before you delete it.

### Plain output {#export-marketplace-plain}

`stdout` carries one table, `Plugin | Client | Version | Action | Omitted`,
with one row per plugin and client. `stderr` carries one line per marketplace
file and one for the root README, `wrote <path> (2 plugins)`,
`unchanged <path>` or `removed <path>`. The
wording of both is not a contract. The JSON report is.

### JSON report {#export-marketplace-json}

`--format json` emits `{"items": [...], "files": [...]}`. `items` has one row
per plugin and client. `files` has one row per marketplace file, then one for
the root `README.md` when grim wrote or checked it.

```json
{
  "items": [
    {
      "plugin": "team",
      "client": "claude",
      "family": "claude",
      "path": "/abs/repo/claude/team",
      "version": "1.4.0+3f9a0c12b7de",
      "action": "unchanged",
      "members": [
        {"kind": "skill", "name": "plan", "lock_name": "team-plan", "pinned": "ghcr.io/acme/team-plan@sha256:…"}
      ],
      "omitted": []
    }
  ],
  "files": [
    {
      "client": "claude",
      "path": "/abs/repo/.claude-plugin/marketplace.json",
      "action": "written",
      "plugins": ["team"]
    },
    {
      "client": null,
      "path": "/abs/repo/README.md",
      "action": "written",
      "plugins": ["team"]
    }
  ]
}
```

- `items[].action` is `written`, `unchanged`, `removed` or `empty`.
  `files[].action` is `written`, `unchanged` or `removed`. These literals are
  frozen, and every key is always present.
- `members` and `omitted` follow the
  [plugin export's](#export-plugin-json) shape and sort. `family` is `claude`
  or `agent-plugins`. Unlike a plugin export's item, there is no `format` key.
- A `removed` row is a tree grim deleted. Its `path` is that tree, `version`
  is `null`, and `members` and `omitted` are `[]`. A removed file's `plugins`
  is `[]`.
- An `empty` row is a plugin a client cannot carry. Its `path` is the tree
  that would have been written, `version` is `null`, `members` is `[]`, and
  `omitted` names every member with its reason.
- The README row has `client: null`, the same `path`, `action` and `plugins`
  keys, and lists every plugin. Its `action` is `written` or `unchanged`.
- Rows are ordered by plugin name, then client in the order `clients` lists
  them. `removed` rows follow, ordered by client and plugin. Files follow the
  client order, then the README row, with removed files last.

The report is emitted only when the run succeeds. See [the enveloped reports
table][json-shapes-items] for the field reference of the shared members.

### Exit codes {#export-marketplace-exit}

| Situation | Code |
|-----------|------|
| Success | `0` |
| A flag or positional the command does not take | `64` |
| A missing `[marketplace]` table, or an invalid name (reserved, malformed), owner, email, description, `repository` or client; a plugin name that is not portable (a Windows reserved device name such as `con`, `aux`, `nul` or `com1`, reported as an unsafe entry); a manifest or lock inside an owned path or itself a symlink; a logo, `path:` member or `project` directory outside the manifest directory, behind a symlink, or under a directory the export owns (matched ignoring ASCII case); a project's `grimoire.toml` or `grimoire.lock` that is a symlink | `65` |
| A plugin that no selected client can carry | `65` |
| A symlink, non-directory or reparse point on an owned path; a case collision or reserved name in a rendered tree | `65` |
| A foreign owned path without `--force` (`untracked-destination`), including a renamed marketplace | `65` |
| The manifest's lock or the output-root lock is held | `75` |
| Everything else, including the offline and network failures | as [`grim export plugin`](#export-plugin-exit-codes) |

`grim export plugin` keeps its own table above. From 0.15.0 it succeeds for
`--client qoder`, which exited `78` before.

## grim tui {#tui}

`grim tui` opens an interactive browser over your declared registries'
catalogs. It shows the catalog with live install state in colour, opening in
the collapsible tree view by default and toggling to a flat kind-grouped list
(press `t`; set [`options.tui.default_view`][options-tui] to `"flat"` to open
there instead). When
more than one registry is configured, the flat list adds a leading **Registry**
column showing `alias (url)` (or the raw URL when no alias was set), and
the Repo cell is shortened to the registry-relative path so names stay readable.
It supports multi-select with batch install, update, and delete. Press `?` in the TUI
for the full key map; highlights are `t` to toggle tree/flat view, `v` to
pick a version, `o` to open the selected entry's repository URL in the
browser, `g` to switch scope, `h` to show/hide deprecated artifacts, and
`space` to mark rows. A search that hides rows below the relevance cutoff
says so in the status line, as `N weaker matches hidden`.

The TUI opens in project scope when it finds a `grimoire.toml` by walking
up from the working directory. When it finds none, it opens in global
scope, which needs no config file, and the status line says so. Pressing
`g` then asks whether to create a project `grimoire.toml`, at the root of
the enclosing git repository or in the working directory outside one.
Initialize writes it and switches to project scope; Cancel stays in
global scope and writes nothing. The new file declares no registry, so
the default browse source applies until you add one. `grim tui --global`
with no global config opens too: the global config is optional.

Like [`grim search`](#search), a registry declaring an `include` / `exclude`
[browse filter](./configuration.md#browse-filters) shows only the
repositories its patterns admit — and a source whose filter admits nothing
keeps its tree root, rolled up as `0/0`, rather than vanishing from the
tree. The root label is unaffected: it stays the alias (or the raw
locator when no alias was set), exactly as without a filter. `--registry` collapses the browse and
applies no filter at all. Like the registry set itself, a filter is read
once at startup — editing `include`/`exclude` in `grimoire.toml` while the
TUI is open has no effect until you quit and reopen it.

The `filter admitted 0 of N repositories` warning is written to
`$GRIM_HOME/tui.log`, not to the screen — the alt-screen session redirects
all log output for its whole lifetime so nothing can scribble the frame. The
TUI's own channel for it is the status line, which names each affected source
in a `filtered:` clause alongside the existing `offline:` and `truncated:`
ones:

```text
offline: ghcr.io/down · truncated: ghcr.io/big · filtered: acme (localhost:5002/uxrev)
```

Each name in those clauses is the source's label — `alias (url)` for an
aliased entry, the bare locator for one without an alias. The tree root and
the flat list's Registry column show the same alias on its own (a locator
rarely fits a table cell); the group's detail pane spells the locator out.

The `truncated:` clause names the sources whose listing is incomplete: an
`oci` build that is still running or was interrupted, or a `_catalog` walk
that could not finish.

That clause is what explains a `0/0` root without leaving the TUI. It names
a source only when its own filter can be shown to have caused the empty
result — the source had rows before the filter ran, and none after, the
same proof the CLI's `admitted 0 of N` diagnostic requires. A source that
was already empty with nothing for a filter to remove is never blamed here
either, exactly as `grim search` stays quiet about it. A mis-aimed `exclude`
that removes nothing has no signal on either surface — the CLI itself
dropped that trigger (an `exclude` with nothing yet to match is
indistinguishable from a correct one waiting on a not-yet-published
repository), so a mis-aimed `exclude` renders a full tree with no warning
anywhere.

Like [`grim search`](#search), deprecated artifacts that are not installed are
**hidden on open** (installed-but-deprecated rows stay visible, still marked).
Press `h` to reveal or re-hide them live, pass `--show-deprecated` to open with
them shown, or set [`options.show_deprecated`](#config) to `true` for the
default.

`--sort <name|updated|rating|downloads>` opens the browse in one of the
[four orders `grim search --sort`](#search-sort) applies, with the same
semantics — including replacing the relevance ranking the `/` search
applies. It overrides [`options.tui.sort`](./configuration.md#options-tui)
for one run, and [`options.tui.sort_order`](./configuration.md#options-tui)
picks the direction the order opens in. Omitted and unconfigured, the
browse opens grouped by kind and then by name as it always has; the `s`
key cycles through the same orders live, `S` flips the direction, and the
Catalog title names the active one. When the browse source publishes
[artifact ratings](./ratings.md), the detail pane gains a `Rating:` row
below the provenance rows (`42 upvotes`); an unrated artifact simply has no
such row, never a `0`.

`--theme <name>` picks the color theme for one run and
overrides [`options.tui.theme`](./configuration.md#options-tui). The default,
`terminal`, draws with your terminal's own colors; every other theme is a
fixed palette with its own background. The value set is closed, and any other
name is a usage error. See [Choose a theme](./browse.md#themes) for all 14.

**Tree view** — pressing `t` switches the catalog between flat list mode and
a collapsible tree grouped by browse source and repository path. Rows from an
OCI registry group under that registry (host plus configured namespace).
Rows from a [package index](./package-index.md) group under the index
source, by where the index placed them: `<host>/<namespace>`, then the
package name. An unbranched chain like `github.com/grimoire-rs` renders as one
joined node. The OCI reference stays in the flat list and the detail pane.

Some pointers fall back to their OCI reference in the tree. That happens
when the index did not place the pointer, or placed it under a bad path.
A bad path has an empty, `.` or `..` segment, a character outside
`[A-Za-z0-9._:-]`, or too many or too long segments. In tree mode:

| Key | Action |
|-----|--------|
| `t` | Toggle between flat list and tree view. |
| `→` | Expand the selected group (reveal its children). Tree mode only. |
| `←` | Collapse the selected group. On an already-collapsed group or on a leaf entry, jump to the parent group instead (ARIA-style navigation). Tree mode only. |
| `z` | Fold the whole tree: if anything is collapsed, expand everything; otherwise collapse back to the configured [`expand_levels`][options-tui] depth. Tree mode only. |
| `Enter` on a group | Fold or unfold the group (same as `→`/`←` toggle). On a leaf entry it rewinds the detail pane and fetches its companion immediately rather than waiting for the selection to settle — and it is the only way to retry a companion whose fetch failed. |
| `tab` / `shift-tab` | Cycle the detail pane's [panels](#tui-detail-tabs) forward / backward. Works on every catalog row; a panel with nothing published behind it reads `not available`. |
| `space` on a group | Mark every descendant leaf in the subtree. The group's mark glyph turns filled (`▣`) when all descendants are marked. |
| `i` / `u` / `d` on a group | Install, update, or uninstall the subtree's descendant leaves whose current state permits the action (when no other rows are individually marked) — `i` acts on `not installed`, `pending` and `integrity-missing` leaves; `u`/`d` act on `installed`, `via bundle`, `outdated`, `modified`, `pending` and `integrity-missing` ones. `pending` is in both sets: the artifact is installed, so update and uninstall apply, *and* an install still has outputs to write for it. Leaves the action does not apply to are left untouched; if none qualify, the key press is a no-op with a status message. Batch behavior follows the same selection precedence as the flat view. |

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

An active search (started with `/`) switches the tree to the flat list once
the query holds a text term. Ranked results then read as one list instead of
scattering across groups. The list stays flat until the query is cleared, so
finishing a kind word like `bundle` does not flip the view back.

Clearing the query (`esc`) returns to the tree. Pressing `t` during a search is
honoured and sticks. Clearing the query then leaves the view as you chose
it.

Config fields under `[options.tui]` in `grimoire.toml` let you set
the opening view mode, how many tree levels open expanded, how paths are
split into groups, and the color theme. See [`[options.tui]`][options-tui] for the full reference.

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

The detail pane sits beside the catalog and is always live for the selected
row: the centered artifact reference, its `Summary:` and `Description:`
sections, and a `Metadata:` block with the keywords and the
[repository URL](./publishing.md#metadata-repository) (version and install
status stay on the catalog row).

There is **no detail focus to enter or leave.** `↑`/`↓` always move the
selection, the pane scrolls with `j`/`k` (line by line) and `pgup`/`pgdn` (a
page), `tab` switches its panel. Every one of them works from wherever you
are. Outside a search, `esc` quits on the first press. Scrolling is clamped
at both ends: it saturates at the top and stops when the content's last line reaches the pane's bottom
edge.

### Detail tabs and repository docs {#tui-detail-tabs}

Every catalog row's detail pane carries a three-panel strip, painted into the
pane's own top border and cycled with `tab` / `shift-tab`:

```
──Overview──Readme──Changelog─────────────────────
```

| Panel | Content |
|---|---|
| `Overview` | The metadata pane above, plus a `Support:` section when the repository publishes [support channels](./publishing.md#support-channels). |
| `Readme` | The companion's `README.md`. |
| `Changelog` | The companion's `CHANGELOG.md`. |

The strip is **fixed** — all three, on every row, always visible. A panel whose
document the repository did not publish is greyed and reads `not available`
when you land on it, rather than disappearing: a strip that changed as a fetch
landed would resize the pane under you, and `tab` would mean something
different on every row. Selection always starts on `Overview`.

grim asks the registry for the row's [description
companion](./publishing.md#description-companion) once per repository per
session, as soon as the selection holds still — arrowing through the catalog
never triggers it, and nothing needs pressing first. Markdown is rendered
lightly — headings, lists, fenced code, and rules; everything else shows as
written.

Support channels are the one thing in the pane that is **not** read from the
browse catalog. They live on the companion tag, which is mutable by design, so
a cached copy could name a contact that has already moved — which is why they
are absent from `grim search` and from a catalog row
([where each field surfaces](./publishing.md#metadata-surfaces)). Fetching them
once the selection settles — or immediately on `enter` — avoids that entirely.

Offline (or on a fetch that fails) the Overview keeps every catalog field and
its `Support:` section names the reason instead of the channels — an omitted
section would be indistinguishable from a repository that publishes none. A
failed fetch is **not** retried automatically: press `enter` for one more
attempt, or `r`, which clears the cache along with the catalog so a republished
README shows up without restarting the session.

An update, and an install at a version you picked, need only the binding that
holds the row. When the lock matches `grimoire.toml` they read it from the lock,
so a pin whose manifest its publisher deleted stops them no more than it stops
`grim update <name>`. Otherwise they ask [`grim lock`](#lock), as an install does.

A TUI install first asks what `grim lock` makes of the declaration as it stands.
Install on a row that relock reaches through the row's repository writes nothing
to `grimoire.toml` and saves that relock. That covers a
declared entry under any name, a bundle member and a dependency, also one an
edge asks for at that repository. It also covers a bundle that another bundle or
a dependency brings in. A bundle member's row also writes nothing when the declaration holds or
reaches its name at another repository or a path, so an override stays.

Where `grim lock` fails, Install fails the same way. The one exception is a `78`
on the row's own key when one of the conflicting requests names the row's
repository. Install then declares that key, as the message advises. A row
nothing reaches is declared and relocked like [`grim add`](#add), then
materialized like [`grim install`](#install).

A catalog row guesses its name from the repository. That name may already be
bound at another repository or a path. Install then refuses before writing
anything, as `grim add` does, and so does a version you picked.
`grim add --name` declares the row beside it.

Both also refuse when another artifact brings in that name from another
repository or a path. That includes a `78` whose conflicting requests all name
other repositories. Declaring the name would replace what those artifacts ask
for and move its pin.

Install keeps every other pin and re-packs every path source, as `grim add` does.
Update is [`grim update <name>`](#update-names) on the binding of the row's kind.
Another kind bound to the same name keeps its pin. It writes no line, keeps a
declared digest, and refuses a stale lock the same way.

Update installs that command's closure and refuses an edit inside it, which the
Overwrite retry then overwrites. A member that a direct line overrides inside a
nested bundle is outside that closure. Vendor config entries, hook arming and
settings values stop at it too. Update prunes what that command prunes. A
dropped artifact you edited stays, and the status line names the
`grim uninstall --force` command that removes it.

On a bundle row it selects the bundle binding alone, so a member keeps its pin
unless the bundle changed it. On a member row it selects the member's binding.
Only a version you picked writes a line. It re-declares the binding that holds
the row at that tag, never a second binding, or declares a row nothing holds.

Update on a row nothing holds exits `79` and writes nothing, as `grim update` of
an undeclared name does. That holds for a lock row no declaration reaches, and in
a project with no lock. There `grim update <name>` exits `0` instead, pruning
that row or locking every declared artifact. With no lock, an Update of a held
row also locks every declared artifact. Its status line then shows the warning
`grim update` prints.

An Install that declares a row is `grim add`'s [hook consent](#add) in TUI form.
It records the hooks that row newly brings before it installs them. The status
line reports `hook '<name>' consented`, also when the install then fails.

Every other Install, and every Update, writes no record. Each one arms on the
record as it stands, so a hook the record covers stays armed. For a hook the
record lacks, the line reports `hook '<name>' not armed` and the reason.

After an Install or Update, the status line names every pin the action moved or
dropped, the row's own included, as `(moved …; dropped …)`. It does so even if
the install then fails. A pin the lock did not hold before is neither. An
Overwrite retry keeps what its refused attempt named.

A delete shows the note its lock edit left. The line leads with
``run `grim lock` ``, which keeps the command visible on an 80-column terminal,
and a failure goes ahead of it. A note that warns the next `grim lock` may move
a pin gets no such lead, and neither does a batch that left one. A `--config` session's command is too long for
that terminal, so its last word shows only on a wider one.

A batch says how many distinct notes it left (`1 of 2 notes`). Where the notes
include one that warns the next `grim lock` may move a pin, the line shows that
one.

A delete that removes nothing says so. A row a bundle provides reads
`kept — provided by a bundle; remove the bundle to remove it`. A nested bundle's
row reads the same while the bundle that includes it stays declared. A row another
artifact requires reads
`kept — required by another artifact; remove that artifact to remove it`. A batch
counts such rows (`uninstalled 1/2, 1 kept`).

Delete is the full inverse via the
[`grim uninstall`](#uninstall) seam, so it also removes the dependencies only
that artifact needed. It acts on the binding that holds the row, so a row
declared under another name deletes that name. A bundle row, a Local one
included, also deletes the members only that bundle provided.

When nothing holds the row, Delete acts on its guessed name only where nothing
else uses it. Another terminal may have bound that name since the view loaded.
If `grimoire.toml`, the lock or an install record binds it at another
repository or a path, Delete refuses before changing anything. Press `r` to
refresh the view.

A Local row refuses the same way when `grimoire.toml` binds its name at a
registry or another path. With no line, its install record decides. A lock row
or record that an earlier registry install left does not count while the path
line stands. A bundle member's row refuses when its name is bound at another
repository or a path. If its bundle still provides it, the direct line decides.

Installing from a tag that has since
moved past the installed digest flips the row to `outdated` right after the
install completes.

**The Overwrite dialog** is what those seams' integrity gate looks like from
the TUI. `i` refuses an artifact whose bytes drifted from the hash grim
recorded, and so does `u` [since 0.13.0](./upgrading.md#update-integrity).
Rather than report a dead end, a refusal opens a modal offering the forced
retry. Answering it is what supplies `--force`, not the key you pressed.

Only a **single-artifact** action offers it. One answer cannot speak for
several artifacts, so a batch leaves its refusals in the aggregate status line
instead.

The dialog opens with one line that names, by kind and name, every artifact its
retry changes. Each refusal follows. A settings refusal names every package it
refused, then lists up to six of the refused values, and `tui.log` holds the
rest. An Update's dialog also lists each edited artifact its new lock dropped,
for the retry to remove.

A dialog taller than the screen scrolls. `↓`/`↑` or `j`/`k` move it a line,
and `pgdn`/`pgup` a page. Overwrite cannot be chosen until the last line has
been on screen. A resize sends the dialog back to its top, and the last line
must be on screen again before Overwrite can be chosen. A terminal 21 columns
wide or narrower shows the dialog no text, and so does a dialog whose text wraps
to more than 65,535 lines. Overwrite cannot be chosen there, and the key hint
reads `widen terminal` or `enlarge terminal`. A key pressed before the dialog
opened is dropped, so it never answers the dialog. The width count measures
the half-width sound marks U+FF9E and U+FF9F as one column each, as drawn. A variation
selector after one adds no column.

The retry forces only what the dialog names, so it changes no package or
artifact you were not shown. A refusal it did not list reopens the dialog with
that artifact added.
The list only grows, so there is no confirm loop to get stuck in.

An Install's dialog lists only what [`grim add --force`](#add) covers, never a
dependency. A refused dependency opens no dialog, and the status line names it
instead. An Update's lists what `grim update --force <name>` covers, its
dependencies included.

**The batch confirmation** stands between one keystroke and a whole catalog.
An `i`, `u` or `d` that would act on more than one artifact opens a prompt
before anything runs. The target is a marked set, or a group selected in
tree view. The prompt names the count and where the targets came from.
Examples: `Install all 3 marked artifacts`,
`Install 2 of 4 marked artifacts (2 already installed)`, and
`Uninstall every artifact in <group>` for a tree-view group.

It then lists the first few artifact names, followed by `and N more`.
Cancel is preselected and there is no `y` shortcut. `→`/`←` or `h`/`l`
switch between Cancel and the action button. `enter` confirms only on the
action button, and `q` quits the TUI. The scroll keys `↑`/`↓`, `j`/`k` and
`pgup`/`pgdn` move a prompt taller than the screen. A terminal narrower than the
buttons offers no action. Any other key, including `esc`, cancels and keeps the
marks.

A single artifact still acts on the first press.

**The `+ pending` badge** marks an artifact that is installed but does not yet
cover every client it should. It is materialization drift, the same thing
[`grim status`](#status) reports as `outputs_pending`. A client was added to
the set, or a release moved a destination. So `grim install` has a file to
write that the install record does not account for. The row is not broken and
nothing is out of date — pressing `i` writes the missing outputs and clears the
badge.

`u` and `d` work on a pending row too; it is the one state both action sets
share. Press `?` for the full status legend.

**A settings row** (experimental) can also read `gated`, `not-armed` or
`stale`, as [Artifact states](#artifact-states) defines. The detail pane adds
a `Settings:` line that says what to run next. `i` acts on all three, because
an install re-reads the gates and a lock catches up. The `?` overlay lists
the three words under "Status column", for example "settings held back: flag
off, no consent, or nothing applies here". The status bar does not list
them, and the overlay drops its blank separator rows so it fits an 80×24
terminal.

**The Settings panel** opens after an action that changed or refused a
[settings value][settings-changes]. Its only button is Close, `q` quits, the
scroll keys move a long list, and any other key closes it.

With nothing refused, the message reads "These settings changed." With a value
refused, it reads "Refused values were left as they are. `grim install --force`
overwrites the ones marked overwrite." After a delete it ends
"`grim uninstall --force settings <name>` removes them." instead.

`grim uninstall` keeps a package that a bundle or another declared artifact
still holds, so that command cannot remove such a value. A bundle member's
changed MCP entry is held the same way. A single-artifact delete then opens a
dialog instead of the panel, and the dialog lists each refused value as a
removal.

The delete checks everything it would remove before it removes any of it. So
the dialog opens with nothing removed yet and lists every refusal at once. Its
first line names each refused package and MCP server, then up to six settings
values follow. It scrolls like the Overwrite dialog, and Uninstall cannot be
chosen until its last line has been on screen. On a terminal 21 columns wide or
narrower it shows no text, and so does a dialog whose text wraps to more than
65,535 lines. Uninstall cannot be chosen there.

The dialog's **Uninstall** button repeats the delete and forces only the
packages and MCP servers the dialog names. A value of a package the dialog does
not name that changed after it opened refuses that retry. The retry then
removes nothing and opens the dialog again, naming that value too.

Cancel is preselected and removes nothing. The status line then says to delete
the row again and choose Uninstall. In a batch,
the panel ends "Delete the refused row on its own to remove them." instead.

The delete's command names the refused package, or reads `<kind> <name>` when
several packages refused. Both commands carry the session's `--global` or
`--config <path>`. The status line names the outcome
`refused (settings set or changed outside grim)`.
The lines below follow these forms:

<!-- doc-norun: lines the TUI draws, not a command -->
```text
changed claude .claude/settings.local.json /model (team-defaults)
overwrite claude .claude/settings.local.json /model (team-defaults): modified
  now: opus
remove claude .claude/settings.local.json /model (team-defaults): modified
remove mcp .mcp.json /mcpServers/srv (srv): modified
conflict claude /model (other-team)
```
<!-- /doc-norun -->

The panel shows at most 12 change lines and 6 refused lines. Past that one
line counts the rest, which are in `$GRIM_HOME/tui.log`. A change list ends
with the restart note. That log also holds every warning, so no restart line
reaches the terminal. When the Overwrite or delete dialog is already open, the
same lines join its detail instead of opening a second panel.

The `↑ outdated` badge tracks the reference the row is **declared** with, the
same one [`grim update`](#update) re-resolves. So pressing `u` on a badged row
always has something to do. A row declared at an exact version or a digest
never badges `↑`, however many higher tags the repository grows. The version
column is where a newer release shows up for those.

A bundle row works the same way at the bundle level. Install declares it
under `[bundles]`, expands it into its members (like
`grim add --kind bundle`), and materializes exactly those members. Delete
removes the member files and records, evicts the members from the lock, and
undeclares the bundle. A member shared with another still-declared bundle is
spared: its files stay on disk and its lock entry only loses the deleted
bundle's provenance.

The row's own state folds in the health of its members. Declaration is the
gate — a bundle absent from `[bundles]` reads `not-installed` whatever its
members look like — but past it the row shows the **worst** member state,
on the bundle-row ladder under [artifact states](#artifact-states). A
member that was never
materialized folds in as `pending`, not `not-installed`, so no member can drag
the row back across the line declaration owns. That is what makes the bundle
actionable as a unit: a flat `installed` would hide the damage *and* refuse
`i`, leaving nothing to press.

```sh
grim tui --registry ghcr.io/acme
```

## grim build {#build}

`grim build <path>` validates and packs a local skill directory, rule `.md`
file, [agent](./agents.md) `.md` file, [MCP server](./mcp-servers.md)
`.toml` file, or bundle `.toml` file without pushing it — a dry run for authors.
`--kind <skill|rule|agent|bundle|mcp|hook|settings>` forces the artifact kind
instead of auto-detecting it from the path. An agent always needs `--kind
agent` — a bare `.md` packs as a rule; an MCP server always needs `--kind
mcp` — a bare `.toml` packs as a bundle. A [settings document][settings] is
a `.toml` file too, so it always needs `--kind settings` and exits `78`
without it.

[Build provenance](./publishing.md#git-provenance) (commit revision and
date) is embedded by default so the preflight reflects what a release would
stamp; `--git` additionally requires it and
[discloses](./publishing.md#git-disclosure) the `origin` remote and commit
author, and `--no-git` suppresses all of it. The metadata flags
(`--license`, `--repository`, `--authors`, `--vendor`, `--url`,
`--documentation`) fill [annotations](./publishing.md#metadata-flags) the
artifact file does not author.

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
[MCP server](./mcp-servers.md) with `--kind mcp`; `--pin` freezes floating
bundle members to digests and, for any other kind, the floating references in
its [declared dependencies](./artifacts.md#requires). [Build provenance](./publishing.md#git-provenance) (commit revision and
date) is embedded by default and stays idempotent because neither value comes
from the clock; `--git` additionally requires it and
[discloses](./publishing.md#git-disclosure) the `origin` remote and commit
author (a non-git path then fails, 65), while `--no-git` suppresses every
derived annotation. The metadata flags (`--license`, `--repository`,
`--authors`, `--vendor`, `--url`, `--documentation`) fill
[annotations](./publishing.md#metadata-flags) the artifact file does not
author. `--compression <none|gzip>` (default `none`) packs a skill, rule,
agent or hook into a gzip layer, and a bundle or MCP server ignores it.
A bare `--compression` means `gzip`.
Consumers need grim 0.15.0 or later to install a gzip layer, and the flag
prints one stderr line saying so. Re-releasing an existing version with the
other setting adopts the published manifest when the content is otherwise
equal. `--force` converts it at the cost of a new digest. See
[Compressing layers](./publishing.md#compression). `--push-registry <host[/prefix]>`
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
[mcp servers](./mcp-servers.md) → hooks → settings → bundles, alphabetical within kind),
except that an entry always follows the same-manifest entries it
[requires](./publishing.md#requires-publish-order). It
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
`--compression <none|gzip>` overrides the manifest's top-level `compression`
key for the run, and `none` applies when neither is set. A bare
`--compression` means `gzip`. It affects skill,
rule, agent and hook entries only. The same consumer requirement and
[adopt semantics](./publishing.md#compression) as `grim release` apply.
`--git` requires [build provenance](./publishing.md#git-provenance) on every
published entry and discloses the `origin` remote and commit author (a non-git
path fails, 65); `--no-git` suppresses every derived annotation. The metadata
flags (`--license`, `--repository`, `--authors`, `--vendor`, `--url`,
`--documentation`) override the manifest's
[`[metadata]` tables](./publishing.md#batch-publish-metadata) for the run.
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
exit 65 (data error), and so do entries that require each other in a cycle,
before any push. The report renders for all completed entries plus
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

`grim schema --kind <config|publish|lock|mcp|hook|settings>` prints a [JSON
Schema](https://json-schema.org/) for one of grim's TOML files to stdout.
`--kind config` describes `grimoire.toml`. Its schema carries the
`[options.experimental]` table additively. `--kind publish` describes
`publish.toml`, additively gaining the `hooks` source table.

`--kind lock` describes `grimoire.lock`, generated by grim and published
so tooling can validate or introspect it, additively gaining a `hooks`
array. `--kind mcp` describes the [MCP server descriptor](./mcp-servers.md)
(`mcp/<name>.toml`). `--kind hook` describes [`hook.toml`](./hooks.md)
(experimental). `--kind settings` describes a [settings
document][settings] (`settings/<name>.toml`, experimental). Each schema is
generated from grim's own parser, so it accepts exactly what grim accepts.

```sh
grim schema --kind config > grimoire-config.schema.json
grim schema --kind publish | jq .title
grim schema --kind lock | jq .title
grim schema --kind mcp | jq .title
```

The same schemas are published to the docs site; see [Editor schema
support](./configuration.md#editor-schema) for the hosted URLs and the
`#:schema` directive that wires an editor up to them.

## grim completions {#completions}

`grim completions <shell>` prints a shell completion script to stdout for
one of five shells: `bash`, `elvish`, `fish`, `powershell`, `zsh`. The
script is generated from grim's real [clap][clap] command tree via
[`clap_complete`][clap-complete], so it can never describe a flag or
subcommand the parser does not actually accept, and it stays current as
commands are added — no separately maintained completion file to fall out
of sync.

There is no shell auto-detection and no eval-time hook: `<shell>` is a
required positional argument, and the command writes the plain script —
nothing else — to stdout, so it composes directly with redirection:

```sh
grim completions bash > ~/.local/share/bash-completion/completions/grim
grim completions zsh  > ~/.zfunc/_grim   # add ~/.zfunc to fpath before compinit
grim completions fish > ~/.config/fish/completions/grim.fish
```

A successful run exits `0`. Omitting `<shell>` or passing a value outside
the five supported shells is a usage error and exits `64`.

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
| `grim_search` | Browse/search the resolved scope's registries (no registry override — the configured set is the boundary). Each source's [browse filter](./configuration.md#browse-filters) narrows the results, exactly as for `grim search`; the tool's own description says so, and says that an absent package is still fetchable and installable by direct reference — a filter is a view, never a boundary. Args: `query?`, `refresh?`, `limit?` (positive integer, the same cap as `grim search --limit`), `offset?` (non-negative integer, the same skip as `grim search --offset`; a negative or non-integer value is an invalid-params error), scope. The payload still carries `total` and `truncated`; page with the [recipe][json-search-paging]. An unpaged call on a large index can return tens of megabytes, so pass `limit` and `offset`. `total`, `truncated` and `sources[].capped` report the cut. Same shape as `grim search --format json` (not byte-identical — see [MCP parity][json-mcp-parity]). | always |
| `grim_status` | Install status of every declared artifact in the requested scope. `check` (optional, default `false`) re-checks the live catalog for deprecation/replacement and re-resolves update availability — same semantics as CLI `grim status --check` (network read; the report's `checked` field says whether it ran). Args: `check?`, scope. Same shape as `grim status --format json` (not byte-identical — see [MCP parity][json-mcp-parity]). | always |
| `grim_fetch` | Return an artifact's content in the tool result — no install. Canonical bytes by default; `vendor` (any supported client name — see the [client compatibility matrix][clients-matrix]) returns that client's projection; `path` fetches one support file (base64 with `encoding: "base64"` for a binary file); a `files` listing is always included. `description` fetches the repository's [description companion](./publishing.md#description-companion) instead (every member inline); `digest_only` resolves to `{ref, digest}` with no download and composes with `description` to probe the companion tag. Content caps at 256 KiB (truncated content carries a marker); layers over 32 MiB are refused before download, and a registry that streams more bytes than its declared layer size aborts mid-transfer into a data error rather than buffering an unbounded body. Args: `ref`, `vendor?`, `path?`, `description?`, `digest_only?`, scope. | always |
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
[plugin-metadata]: ./configuration.md#plugin-metadata
[json-mcp-parity]: ./json-interface.md#mcp-parity
[json-search-paging]: ./json-interface.md#search-paging
[json-broken-pipe]: ./json-interface.md#broken-pipe
[json-shapes-items]: ./json-interface.md#shapes-items
[path-source-trust]: ./stability.md#limitations-path-source-trust
[clients-matrix]: ./clients.md#matrix
[json-hook-arming]: ./json-interface.md#hook-arming
[settings]: ./settings.md
[settings-changes]: ./settings.md#changes
[settings-conflicts]: ./settings.md#conflicts
[settings-consent]: ./settings.md#consent
[settings-force]: ./settings.md#force
[settings-status]: ./settings.md#status
[settings-uninstall]: ./settings.md#uninstall
[stability-unstable]: ./stability.md#unstable

<!-- external -->
[git-config]: https://git-scm.com/docs/git-config
[vscode-compact]: https://code.visualstudio.com/docs/getstarted/userinterface
[mcp-spec]: https://modelcontextprotocol.io/specification/latest
[claude-code]: https://docs.anthropic.com/en/docs/claude-code
[claude-code-plugins]: https://code.claude.com/docs/en/plugins
[claude-plugin-upload]: https://claude.com/docs/plugins/overview
[agent-plugins]: https://agent-plugins.org/
[copilot]: https://github.com/features/copilot
[codex]: https://developers.openai.com/codex
[cursor]: https://cursor.com
[junie]: https://www.jetbrains.com/junie/
[droid]: https://factory.ai
[qoder]: https://qoder.com
[openclaw]: https://github.com/openclaw/openclaw
[opencode]: https://opencode.ai/
[json-rpc]: https://www.jsonrpc.org/specification
[clap]: https://docs.rs/clap/latest/clap/
[clap-complete]: https://docs.rs/clap_complete/latest/clap_complete/
[fzf]: https://github.com/junegunn/fzf
