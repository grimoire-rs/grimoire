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
| `search` | `{kind, repo, summary, description, version, latest_tag, repository, revision, created, deprecated, status}` — see [grim search][commands-search] | `status`: install badge (`installed`, `not-installed`, …) |
| `config list` | `{key, value}` | — |
| `config registry list` | `{alias, oci, index, default}` — both locator keys present, exactly one non-null | — |
| `publish` | `{ref, kind, digest, tags, status}` + sibling envelope key `announce` (`{outcome, branch, url}` or null) — see [publish report][publishing-report] | `status`: `pushed`, `skipped`, `dry-run`, `failed` |

`kind` is always one of `skill`, `rule`, `agent`, `bundle`, `mcp`.

### Single-object reports {#shapes-single}

| Command | Shape | Enum values |
|---------|-------|-------------|
| `init` | `{path, scope, status}` | `status`: `created` |
| `add` | `{kind, name, pinned, status}` | `status`: `added` |
| `remove` | `{kind, name, status}` | `status`: `removed`, `absent` |
| `uninstall` | `{kind, name, status}` | `status`: `uninstalled`, `kept-by-bundle`, `not-installed` |
| `build` | `{kind, name, path, layer_digest, annotation_count, status}` | `status`: `built` |
| `release` | `{ref, manifest_digest, tags, pushed}` | `pushed`: bool (`false` = dry run) |
| `login` | `{registry, username}` | — |
| `logout` | `{registry}` | — |
| `config get` | `{key, value, set, scope}` — see the [config JSON table][commands-config-json] | `scope`: `project`, `global` |
| `config set` / `unset` / `registry add` / `rm` / `use` | `{action, key, value, scope}` | `action`: `set`, `unset`, `registry-added`, `registry-removed`, `registry-default` |
| `config registry show` | `{alias, oci, index, default}` — both locator keys present, exactly one non-null | — |
| `context` | `{version, scope, workspace, config_path, config_exists, lock_path, lock_exists, state_path, grim_home, offline, offline_source, clients, registries, default_registry}` — see [grim context][commands-context] | `offline_source`: `flag`, `env`, or null |
| `fetch` | `{ref, digest, kind, name, vendor, path?, content, truncated?, files?, pointer?, warnings?}` — see [grim fetch](#fetch) | — |

### The fetch exception {#fetch}

`grim fetch` shares its JSON payload with the MCP `grim_fetch` tool, and
that shape predates the [null policy](#null-policy): empty or default
fields (`path`, `truncated`, `files`, `pointer`, `warnings`) are
**omitted**, not null. Treat a missing key as its default. Its plain mode
is the raw `content` payload (pipe-able, no report at all) — the one
payload-plain command; see [grim fetch][commands-fetch].

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
| `no-permission` | 77 | Insufficient permission: registry 403, filesystem `EPERM` |
| `config` | 78 | Config file invalid or unparseable |
| `not-found` | 79 | Resource not found: missing package, absent explicit config path |
| `auth` | 80 | Authentication failure: registry 401, missing credential |
| `offline-blocked` | 81 | `--offline` (or `GRIM_OFFLINE`) blocked a network operation |

The numeric values follow BSD [`sysexits.h`][sysexits] (64–78) with
grim-specific codes above 78; the same table governs plain-mode exit
codes. Clap parse failures (the pre-contract boundary above) and `--help`
never produce the document.

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
`grim_search` and `grim_status` results are byte-identical to
`grim search --format json` / `grim status --format json` for the same
scope (envelope included), and `grim_fetch` returns the same shape as
`grim fetch --format json` — except the MCP tool truncates `content` at
256 KiB for tool-result budgets, while the CLI never truncates below its
8 MiB layer gate.

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
