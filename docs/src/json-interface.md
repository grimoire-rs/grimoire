# The JSON Interface

Every grim command that reports something offers `--format json`. Together
those payloads form one machine-readable surface — the thing a wrapper
script, CI job, or editor extension programs against instead of scraping
tables. This page is the reference for that surface: the envelope rules,
every report shape, the error document, and how exit codes and JSON
interact. The shapes below are [frozen at 1.0][stability-frozen]; changes
are additive only.

## One document per invocation {#one-document}

A `--format json` run writes **exactly one JSON document to stdout** —
a report on success (and on the [documented non-zero
reports](#exit-interplay)), or the [error document](#error-document) on
failure. Nothing else lands on stdout: progress, warnings, and log lines
ride stderr, so `grim … --format json | jq .` always parses.

One boundary sits **before** the contract: a command line that
[clap][clap] cannot parse (unknown flag, missing argument) fails before
grim knows the output format. Those failures print clap's plain-text
usage message and exit `64` (or `0` for `--help`/`--version`) — no JSON
document is emitted. Everything after a successful parse honors the
contract.

Two commands are exempt because stdout *is* their payload: `grim schema`
prints a JSON Schema document, and `grim mcp` speaks JSON-RPC. `grim tui`
owns the terminal and emits no report. In plain mode, `grim fetch` prints
raw artifact content ([payload-plain](#fetch)); its `--format json` is a
normal report.

## The items envelope {#items-envelope}

A bare JSON array can never grow: adding any cross-cutting field to a
top-level `[...]` is a breaking change, so under the [additive-field
policy][stability-additive] a bare-array report would be frozen forever.
Every multi-item report therefore wraps its rows in a uniform envelope:

```json
{
  "items": [ … ]
}
```

`items` is always present (an empty result is `{"items": []}`), and the
envelope object may gain sibling fields in a minor release — `grim
publish` already carries one (`announce`). Commands with enveloped
reports: `lock`, `install`, `status`, `update`, `search`,
`config list`, `config registry list`, and `publish`.

Everything else reports a **single flat object** — those commands concern
exactly one subject (one config file, one artifact, one credential), so
there is no row list to wrap.

## Report shapes {#report-shapes}

### Enveloped reports {#shapes-items}

One row object per item inside `{"items": [...]}`:

| Command | Item shape | Enum values |
|---------|-----------|-------------|
| `lock` | `{kind, name, pinned, action}` | `action`: `locked`, `unchanged` |
| `install` | `{kind, name, target, status}` | `status`: `installed`, `updated`, `unchanged`, `refused`, `skipped` |
| `status` | `{kind, name, source, pinned, state, outputs}` — `pinned` null until locked; `outputs` is `[{client, path}]` (see [grim status][commands-status]) | `state`: `installed`, `stale`, `modified`, `missing`, `outdated` |
| `update` | `{kind, name, old, new, action}` — `old` null for a first lock, `new` null for a pruned row | `action`: `updated`, `unchanged`, `removed`, `kept-modified` |
| `search` | `{kind, repo, summary, description, version, latest_tag, repository, revision, created, deprecated, replaced_by, status}` — `kind` is `null` when the catalog row's manifest declares none; `replaced_by` is the successor reference or `null`; see [grim search][commands-search] | `status`: install badge (`installed`, `not-installed`, …) |
| `config list` | `{key, value, set, type, title, description, default, values}` | — |
| `config registry list` | `{alias, oci, index, default}` — both locator keys present, exactly one non-null | — |
| `publish` | `{ref, kind, digest, tags, status, pushed_to}` (`ref` is the pull name; `pushed_to` is the push-side reference under a [push/pull registry split](./publishing.md#batch-publish-push-registry), `null` when inactive) + sibling envelope keys `descriptions` (`{"items": [...]}` of published/planned [description companion](./publishing.md#description-companion) pushes, `{ref, repository, digest, files}`, `digest` `null` under `--dry-run`; empty `items` when no companion was resolved) and `announce` (`{outcome, branch, url}` or null) — see [publish report][publishing-report] | `status`: `pushed`, `skipped`, `dry-run`, `failed` |

`kind` is one of `skill`, `rule`, `agent`, `bundle`, `mcp` for every
enveloped report except `search`: the other reports resolve a locked or
otherwise real artifact, so their `kind` is always one of those five
values, while `search` reports a catalog row whose manifest may declare
no kind at all, in which case `kind` is `null`.

`install`'s `target` is `null` when every selected client declines the
artifact's kind — e.g. a rule installed with only [Codex][codex-subagents-docs]
selected (Codex has no path-scoped rule mechanism), or an mcp descriptor
no selected client can register. Nothing was written to disk in that
case, so there is no path to report; `status` is `skipped`.

`config list`'s `type` field is one of `string`, `boolean`, `integer`,
`enum`, `string-list`, `string-set`. `string-list` is an ordered, open
list — any value is accepted (e.g. `options.tui.tree_separators`), and its
`values` stays `null`. `string-set` is an unordered collection of unique
values, each drawn from the closed `values` list — the same non-null
shape `enum` rows carry. `options.clients` is the one `string-set` key
today, so it is the one non-`enum` row whose `values` is a list
(`["claude","opencode","copilot","codex"]`) rather than `null`.

### Single-object reports {#shapes-single}

| Command | Shape | Enum values |
|---------|-------|-------------|
| `init` | `{path, scope, status}` | `status`: `created` |
| `add` | `{kind, name, pinned, status}` | `status`: `added` |
| `remove` | `{kind, name, status}` | `status`: `removed`, `absent` |
| `uninstall` | `{kind, name, status}` | `status`: `uninstalled`, `kept-by-bundle`, `not-installed` |
| `build` | `{kind, name, path, layer_digest, annotation_count, status}` | `status`: `built` |
| `release` | `{ref, manifest_digest, tags, pushed, pushed_to}` — `ref` is the pull name; `pushed_to` is the push-side reference under a [`--push-registry` split](./publishing.md#batch-publish-push-registry), `null` when inactive | `pushed`: bool (`false` = dry run) |
| `login` | `{registry, username, verification}` | `verification`: `verified`, `no-auth-required`, `skipped` |
| `logout` | `{registry}` | — |
| `config get` | `{key, value, set, scope}` — see the [config JSON table][commands-config-json] | `scope`: `project`, `global` |
| `config set` / `unset` / `registry add` / `rm` / `use` | `{action, key, value, scope, dry_run}` — `dry_run` is `true` only for `config set --dry-run`, `false` for every other write verb (`unset` has no `--dry-run` flag) | `action`: `set`, `unset`, `registry-added`, `registry-removed`, `registry-default` |
| `config registry show` | `{alias, oci, index, default}` — both locator keys present, exactly one non-null | — |
| `context` | `{version, scope, workspace, config_path, config_exists, lock_path, lock_exists, state_path, grim_home, offline, offline_source, clients, registries, default_registry}`; `registries[]` is `{alias, url, kind, default, authenticated}` — see [grim context][commands-context] | `offline_source`: `flag`, `env`, or null |
| `describe` | `{ref, digest, kind, name, title, description, has_description, summary, version, license, repository, revision, created, keywords, deprecated, replaced_by, tags, annotations}` — every field always present; `kind` is `null` for a foreign manifest; `has_description` is a boolean (whether the repository carries a [description companion](./publishing.md#description-companion), derived from the tag listing at zero extra network cost); `keywords`/`tags` are `[]` when none; `annotations` is the verbatim manifest map; see [grim describe][commands-describe] | — |
| `fetch` | Tri-shaped by flags — content, description bundle, or digest probe — see [the fetch exception](#fetch) | — |

### The fetch exception {#fetch}

`grim fetch` shares its JSON payload with the MCP `grim_fetch` tool. Both
route every call through the same neutral fetch core, so the report takes
one of three shapes depending on the `--description` / `--digest-only`
flags — each an untagged variant, its own flat JSON object. The shape
predates the [null policy](#null-policy): empty or default fields are
**omitted**, not null. Treat a missing key as its default.

**Content** (default, `--vendor`, or `--path`) — the resolved artifact
document: `{ref, digest, kind, name, vendor, path?, content, encoding?,
truncated?, files?, pointer?, warnings?}`.

```json
{
  "ref": "ghcr.io/acme/skills/code-review:1.2.0",
  "digest": "sha256:…",
  "kind": "skill",
  "name": "code-review",
  "vendor": "canonical",
  "content": "---\nname: code-review\n…"
}
```

`encoding` is present only as `"base64"`, when `content` is the base64 of
a non-UTF-8 `--path` support file (plain mode decodes it back to the raw
bytes). Its plain mode is the raw `content` payload (pipe-able, no report
at all) — the one payload-plain command; see [grim fetch][commands-fetch].

**Description bundle** (`--description`) — the repository's [description
companion](./publishing.md#description-companion), every member inline:
`{ref, digest, kind: "desc", files: [{path, size, content, encoding?}],
warnings?}`.

```json
{
  "ref": "ghcr.io/acme/mcp/postgres:__grimoire",
  "digest": "sha256:…",
  "kind": "desc",
  "files": [
    { "path": "README.md", "size": 812, "content": "…" },
    { "path": "logo.svg", "size": 4096, "content": "…", "encoding": "base64" }
  ]
}
```

Each `files[]` entry is the familiar GitHub Contents API style — a `path`
plus inline `content` (base64 for binary members) — so a consumer already
written against that shape maps onto it directly.

Bounded by the same 8 MiB layer gate as any fetch, with no per-file
truncation — the whole companion returns in one call. A multi-file bundle
has no single payload to print, so plain mode requires
[`--out <dir>`](./commands.md#fetch-description) to unpack the tree to
disk instead of printing JSON.

**Digest probe** (`--digest-only`, optionally combined with
`--description`) — a resolve-only cache key, no download: `{ref, digest,
warnings?}`.

```json
{ "ref": "ghcr.io/acme/skills/code-review:1.2.0", "digest": "sha256:…" }
```

The reported digest equals the corresponding full fetch's manifest digest
— the artifact's, or, combined with `--description`, the companion's — so
a consumer caches on it and skips an unchanged download entirely. Plain
mode prints the bare digest, no trailing newline.

## The error document {#error-document}

A failing run under `--format json` previously left stdout empty; a
consumer had to scrape stderr prose. Since the 1.0 contract, both
post-parse failure paths emit a structured document on **stdout**:

```json
{
  "error": {
    "code": "not-found",
    "exit": 79,
    "message": "/abs/path/grimoire.toml: I/O error: No such file or directory (os error 2)"
  }
}
```

The consumer rule: **parse stdout; a top-level `error` key marks the
error document.** No report shape has a top-level `error` key, so the
check is unambiguous. The document rides stdout — not stderr — because
stderr carries tracing output and the two streams would interleave; the
human-readable error chain still prints to stderr unchanged.

`message` is the rendered error chain — human-readable text, **not** a
contract (see [what is not frozen][stability-unstable]). Programmatic
dispatch uses `code` and `exit`:

| `code` | `exit` | Meaning |
|--------|--------|---------|
| `failure` | 1 | Generic failure — no specific class applies |
| `usage` | 64 | Bad invocation (post-parse): unknown config key, conflicting flags |
| `data` | 65 | Malformed input data: bad reference, invalid digest, integrity refusal |
| `unavailable` | 69 | Required resource unreachable: registry down, announce failure |
| `io` | 74 | Filesystem I/O failure |
| `temp-fail` | 75 | Transient failure — retry may succeed |
| `no-permission` | 77 | Insufficient permission: filesystem `EPERM` |
| `config` | 78 | Config file invalid or unparseable |
| `not-found` | 79 | Resource not found: missing package, absent explicit config path |
| `auth` | 80 | Authentication failure: registry 401 or 403, missing credential |
| `offline-blocked` | 81 | `--offline` (or `GRIM_OFFLINE`) blocked a network operation |

The numeric values follow BSD [`sysexits.h`][sysexits] (64–78) with
grim-specific codes above 78; the same table governs plain-mode exit
codes. Clap parse failures (the pre-contract boundary above) and `--help`
never produce the document.

### The optional `reason` field {#error-reason}

Some failures carry a machine-readable `reason` alongside `code`/`exit` —
a kebab-case subtype that lets a consumer detect a *specific* refusal
without scraping the non-frozen `message`:

```json
{
  "error": {
    "code": "data",
    "exit": 65,
    "reason": "stale-lock",
    "message": "skill 'code-review' (…): partial-resolve refused: lock declaration_hash … does not match current …; retry with a full resolve"
  }
}
```

`reason` is **additive and optional**: it appears only when grim has a
subtype for the failure and is **omitted** otherwise — an absent key, not
`null` (the [`fetch` omit-empty fields](#fetch) set the precedent; the
error document is likewise exempt from the [null policy](#null-policy)).
A consumer must tolerate both its absence and a value it does not
recognize. The reasons defined so far:

| `reason` | Paired with | Meaning |
|----------|-------------|---------|
| `stale-lock` | `data` / 65 | A partial `grim update <name>` was refused because `grimoire.lock` no longer matches the current declaration. Retry with a full `grim update` (no names). |
| `modified` | `data` / 65 | An install was refused because the installed artifact was modified locally (the same state `grim status` reports as `modified`). Retry the same `grim install` / `grim add` with `--force` to overwrite. |
| `untracked-destination` | `data` / 65 | An install was refused because the destination already exists on disk with no install record — grim does not clobber files it did not create. Retry with `--force` to overwrite and record it. |
| `no-config` | `not-found` / 79 | A project-scope command found no `grimoire.toml` by walking up from the working directory. Distinct from an explicit `--config <path>` that does not exist, which also exits 79 but carries no `reason` — that is a wrong path, not "no config anywhere". |
| `locked` | `temp-fail` / 75 | A config-file write was refused because another `grim` process holds the `<file>.lock` advisory sidecar. Transient — retry the same command. |

New reasons may appear in any minor release under the [additive-field
policy][stability-additive]; existing ones never change meaning.

### The optional `retryable` field

Alongside `reason`, the error object may carry `"retryable": true` — a
hint that the same command is worth retrying unchanged, no `--force` or
input fix required:

```json
{
  "error": {
    "code": "temp-fail",
    "exit": 75,
    "reason": "locked",
    "retryable": true,
    "message": "…: another process holds the advisory lock; try again"
  }
}
```

`retryable` is **additive and omit-when-absent**, same rule as `reason`
itself: present only when `reason` is present *and* that specific reason
is retryable, otherwise the key is absent entirely — never a bare
`false`. Today only `locked` sets it; every other documented `reason`
(including a bare `reason`-less failure) omits the key.

## Null and additive policy {#null-policy}

Optional report fields are **always present**: a field that does not
apply serializes as an explicit `null`, never as an absent key (the
`fetch` payload is the [one documented exception](#fetch)). A consumer
can therefore distinguish "not applicable" (`null`) from "older grim
that predates the field" (key missing) without version sniffing.

New fields may appear in any minor release; existing fields never change
type or meaning and are never removed. Readers must ignore unknown
fields. The full policy, including the install-state schema it also
covers, lives on the [stability page][stability-additive].

`config list`'s `value` field became nullable in a shape-compatible way:
`null` is emitted only for unset rows, which only the new `--all` flag
surfaces — a consumer that never passes `--all` keeps seeing non-null
values, so the additive-field policy holds.

## Exit codes and JSON together {#exit-interplay}

A non-zero exit does **not** imply the error document. Two commands ship
a full report alongside a non-zero code, because the outcome is data, not
a fault in producing it:

- `config get` of a valid-but-unset key exits `1` and still prints the
  full `{key, value: null, set: false, scope}` report ([config exit
  codes][commands-config-exit]).
- `publish` on a fail-fast stop (exit `65`) or an announce failure after
  a successful push (exit `69`) still prints the full report — completed
  entries, the failed entry, `announce: null` ([publish
  report][publishing-report]).

The error document appears only when the command could not produce a
report at all. A robust consumer therefore branches on the top-level
`error` key first, then on the exit code.

## MCP parity {#mcp-parity}

The [MCP server][commands-mcp] tools return the same payloads:
`grim_search` and `grim_status` results have the same shape, envelope,
and field values as `grim search --format json` / `grim status --format
json` for the same scope — parsed, the two documents compare equal. They
are **not byte-identical**: the MCP server serializes compact JSON
(`serde_json::to_string`) while the CLI pretty-prints
(`to_string_pretty`), so whitespace differs. `grim_fetch` returns the
same shape as `grim fetch --format json` — except the MCP tool truncates
`content` at 256 KiB for tool-result budgets, while the CLI never
truncates a printed payload. Both share the same two download ceilings:
the manifest's declared layer size is checked against the 8 MiB limit
before download, and that declared size then bounds the actual streamed
bytes — a registry serving more than it declared aborts mid-transfer into
a data error (exit 65) on either interface. The tool's `description` and
`digest_only` arguments select the same tri-shaped report the CLI flags
do, with identical composition rules (`digest_only` with `description`
probes the companion tag) — both interfaces call the same fetch core.
`grim_describe` returns the same shape as `grim describe --format json`,
including the `has_description` field.

## No self-identifying reports {#no-discriminator}

Reports carry **no type discriminator** ("this is a status report"). The
caller knows what it invoked — a wrapper that runs `grim status` does not
need the payload to repeat it, and every report would spend a reserved
key on redundancy. This is a deliberate 1.0 decision: if a future
multiplexing consumer genuinely needs one, adding a field is additive and
can ship in a minor release.

<!-- internal -->
[commands-status]: ./commands.md#status
[commands-search]: ./commands.md#search
[commands-context]: ./commands.md#context
[commands-describe]: ./commands.md#describe
[commands-fetch]: ./commands.md#fetch
[commands-mcp]: ./commands.md#mcp
[commands-config-json]: ./commands.md#config-json
[commands-config-exit]: ./commands.md#config-exit-codes
[publishing-report]: ./publishing.md#batch-publish-report
[stability-frozen]: ./stability.md#frozen
[stability-additive]: ./stability.md#frozen-additive-fields
[stability-unstable]: ./stability.md#unstable

<!-- external -->
[clap]: https://docs.rs/clap/latest/clap/
[sysexits]: https://man.freebsd.org/cgi/man.cgi?sysexits
[codex-subagents-docs]: https://developers.openai.com/codex/subagents
