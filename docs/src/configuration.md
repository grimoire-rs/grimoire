# Configuration

Grimoire keeps configuration in two small files and a handful of environment
variables. Settings (`[options]`, `[options.tui]`) and named registries
(`[[registries]]`) are managed through [`grim config`][grim-config]; declarations
(`[skills]`, `[rules]`, `[agents]`, `[bundles]`) stay under [`grim add`][grim-add]
and [`grim remove`][grim-remove]. You can also hand-edit either file directly,
but note that **any `grim` write — `grim config`, `grim add`, `grim remove` — uses a
lossy serializer: comments are removed** on every write, and so is any key whose
default value collapses to unset (`show_deprecated = false`, an all-default
`[options.tui]`, an all-default `[options.vendors.<name>]`). The one exception
is a leading [`#:schema` editor directive](#editor-schema), which every
rewrite preserves at the top of the file.

## `grimoire.toml` {#grimoire-toml}

The declaration file. An `[options]` table holds defaults, and `[skills]` /
`[rules]` / `[agents]` map each binding name to a reference:

```toml
#:schema https://grimoire.rs/schemas/grimoire-config.schema.json
[[registries]]
oci = "ghcr.io/acme"
default = true

[options]
clients = ["claude", "opencode"]

[skills]
code-review = "ghcr.io/acme/code-review:1"
commit-helper = "ghcr.io/acme/commit-helper:1"

[rules]
rust-style = "ghcr.io/acme/rust-style:2"

[agents]
code-reviewer = "ghcr.io/acme/code-reviewer:1"
```

The `[[registries]]` entry with `default = true` sets the primary registry short references expand against; `clients` selects which
[AI clients](./concepts.md#clients) `grim install` and `grim update` materialize
into. It accepts a TOML array of client names — any supported
client (see the [client compatibility matrix](./clients.md#matrix)); when
absent, the **detected** clients for the scope are targeted — every client
whose vendor directory or marker is present — falling back to the generic
`agents` client when none are detected. [`grim init`](./commands.md#init)
seeds this key with what it detects, so a workspace configured through
`init` records its client set once instead of re-deriving it every run.
Unknown keys are rejected on parse, so a typo surfaces
immediately rather than silently doing nothing. A hand-authored entry
outside the closed set, or a repeated entry, is rejected at config
**load** — exit 78 (`EX_CONFIG`), the same class of error as an invalid
`tree_separators` entry below — not deferred until `grim install` runs.

The top-level `show_deprecated` boolean (default `false`) controls whether
[deprecated](./publishing.md#metadata-deprecated) artifacts appear in
[`grim search`][grim-search] and the [`grim tui`][grim-tui] catalog. When
`false`, a deprecated artifact is hidden unless it is installed in the scope
(directly or through a bundle), so a deprecated dependency you already rely on
stays visible; `true` shows them everywhere. It seeds the initial state only —
the search `--show-deprecated` flag and the TUI `h` key override it per run, and
the `h` toggle is never written back to the file.

### `[options.tui]` {#options-tui}

The optional `[options.tui]` sub-table tunes the interactive catalog browser
launched by [`grim tui`][grim-tui]. All four fields are opt-in —
an absent `[options.tui]` leaves the TUI at its built-in defaults.

```toml
[options.tui]
default_view = "tree"
group_by_type = true
tree_separators = ["/", "-"]
expand_levels = 2
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `default_view` | `"flat"` or `"tree"` | `"tree"` | The view mode the browser opens in. Absent, it opens in the collapsible grouped tree (which needs no Registry column and reads compactly); set `"flat"` to open in the plain list instead. An unrecognised value is a config parse error — the enum is strict. The runtime `t` key still toggles between modes ephemerally; the config is never auto-rewritten. |
| `group_by_type` | boolean | `false` | When `true`, inserts an extra type-level group — `skill`, `rule`, `agent`, or `bundle` — between the registry root and the repository path segments in tree view. Has no effect in flat mode. |
| `tree_separators` | array of single-character strings | (absent or `[]`) | The characters on which a repository path is split into nested tree groups. Omitting the field (or setting it to `[]`) leaves the array empty in the config file; at runtime, an empty array normalizes to `["/"]`. Add `"-"` to split on hyphens as well, so `code-review` becomes `code` → `review`. Each entry must be exactly one character; empty or multi-character entries are a parse error. `grim config list --all --format json` surfaces this rule as a machine-readable `constraints` object on the row (advisory `item_pattern` + `item_width`; grim's own validation is authoritative) — see [the JSON interface](./json-interface.md#shapes-items). |
| `expand_levels` | non-negative integer | `1` | How many levels of the grouped tree open expanded, so a large catalog does not flood the screen. `1` (the default when absent) shows only the registry roots; `2` also expands their direct children, and so on. `0` opens the tree fully expanded. Every group below the opening depth starts collapsed, so expanding one reveals its children still folded — you drill down one level at a time. The runtime `z` key folds between this depth and fully-expanded; `→`/`←` still expand or collapse a single group. Has no effect in flat mode. |

Configuration parse errors — including an unrecognised `default_view` value or an invalid `tree_separators` entry — exit 78 (`EX_CONFIG`).

The registry host is always the tree root. When the browsed registry matches
the configured default registry, the host node is elided from the display
so leaf names stay short.

### `[options.vendors]` {#options-vendors}

The optional `[options.vendors.<name>]` sub-tables carry per-client rendering
options. The table key is a client name — the same closed set `clients` accepts
— so a misspelled client is rejected at config **load** with exit 78
(`EX_CONFIG`) rather than sitting in the file doing nothing:

```toml
[options.vendors.cursor]
shared_skills = true
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `shared_skills` | boolean | `false` | When `true`, this client's skills install into the cross-vendor `.agents/skills` pool instead of the client's own skills directory. Absent or `false`, the client keeps its native layout. |

Only a client that actually reads the pool may be opted into it — grim never
writes where nothing reads. Those clients are `codex`, `gemini`, `zed`, `amp`,
`goose` and the generic `agents` client (which already render there), plus
`cursor`, `copilot`, `opencode` and `warp` (which scan it alongside their own
skills directory).
Setting `true` on any other client is refused: exit 65 (`EX_DATAERR`) from
[`grim config set`](./commands.md#config), exit 78 (`EX_CONFIG`) at load when
the value was hand-authored. `false` stays accepted for every client — it is
their resting state.

Flipping the value moves the skill: the next [`grim install`](./commands.md#install)
or [`grim update`](./commands.md#update) writes it at the new location and
removes the old copy. A copy you edited by hand is **never** deleted — it is
kept and grim warns, naming both paths, because the client would otherwise
see the same skill twice (skill scanning is additive everywhere).

If the new location is already occupied by a file grim did not write — a skill
you curated in `.agents/skills` by hand — the move is refused with exit 65
(`EX_DATAERR`) and nothing is touched, the same
[untracked-destination guard](./json-interface.md#error-reason) that protects any install.
Pass `--force` to overwrite it.

An entry set to `true` round-trips through every `grim` write. One left at the
default — `shared_skills = false`, or a bare `[options.vendors.<name>]` header —
is dropped on the next write, like every other default-valued key.

The field has no CLI flag and no `[options]`-level equivalent, so its
precedence chain is two links long: the resolved scope's
`[options.vendors.<name>].shared_skills`, else the built-in default `false`.
There is no cross-scope merge — like every other key, each command resolves
exactly one scope and reads that scope's table whole.

[`grim config`](./commands.md#config) addresses the field as one dotted key per
client:

```sh
grim config set options.vendors.cursor.shared_skills true
grim config get options.vendors.cursor.shared_skills
grim config unset options.vendors.cursor.shared_skills
```

Because the client name is part of the key, naming a client that does not
exist is an unknown key — exit 64 (`EX_USAGE`), not a value error.

### `[bundles]` {#bundles}

An optional `[bundles]` table declares [bundles](./concepts.md#bundles), each
mapping a binding name to a bundle reference. A bundle expands into its member
skills, rules, and [agents](./agents.md) at lock time:

```toml
[bundles]
python-stack = "ghcr.io/acme/python-stack:1"

[skills]
# A direct declaration overrides a bundle member of the same name.
code-review = "ghcr.io/acme/code-review:2"
```

Bundle references follow the same rules as skills and rules — a bare reference
defaults to `:latest`. Per `(kind, name)`, a direct declaration wins over any
bundle, agreeing bundles coalesce, and disagreeing bundles fail closed; see the
[conflict policy](./concepts.md#bundle-conflicts).

A `[bundles]` value may also be a [local path](./concepts.md#references-tags-and-digests)
(`./bundles/x.toml`, `../…`, or absolute) instead of a registry reference —
a **local bundle**, declared and resolved without a publish step:

```toml
[bundles]
docs-stack = "./bundles/docs-stack.toml"
```

A local bundle's members must be **registry** references. `grim lock` rejects
a relative (`./`/`../`) member id with exit 65, because — unlike a
registry-published bundle, whose [deployment-relative
members](./artifacts.md#bundle-relative-refs) resolve against the bundle's
own repository — a local bundle has no registry identity to resolve a
relative member against. It is pinned by the SHA-256 of its canonical JSON
members layer, not a manifest digest; see [`grimoire.lock`](#grimoire-lock)
for the wire shape.

## Multiple registries {#multiple-registries}

A project that pulls artifacts from more than one registry can declare all
of them in a `[[registries]]` array instead of juggling `--registry` flags.
When the array is present it becomes the authoritative browse set for
[`grim search`](./commands.md#search), the [MCP server](./commands.md#mcp), and
the [TUI](./commands.md#tui) — `grim tui` browses all declared registries, one
collapsible root per registry. An explicit `--registry` flag still collapses the
browse to exactly the registries it names — repeatable and comma-separated
(`--registry a,b`) for several at once. `GRIM_DEFAULT_REGISTRY` does **not**
collapse the browse set — it is the short-id resolution default and only
applies as the single-registry fallback when no `[[registries]]` array is
declared.

Each entry declares **exactly one** source locator (`oci` or `index`)
plus optional fields:

| Field | Required | Description |
|-------|----------|-------------|
| `oci` | one of `oci`/`index` | Plain OCI registry ref — host and optional namespace, e.g. `ghcr.io/acme`. Same form as `[options].default_registry`. Lists packages via the OCI `_catalog` endpoint. The pre-0.7.0 key `url` is still accepted as a parse-time alias, so existing configs keep working; new writes use `oci`. |
| `index` | one of `oci`/`index` | A [package index](./package-index.md) locator: an `http(s)://` static base or a git repository (`git+…`, `ssh://`, `git@…`, or ending in `.git`). Replaces the `_catalog` listing; index entries carry their own registry refs. Mutually exclusive with `oci` — setting both is a parse error (exit 78). |
| `alias` | no | Short name for use in [qualified references](#qualified-references). Must be unique across the array. The TUI uses the alias as the display label in the flat list's Registry column and as the tree registry-root row label; entries without an alias fall back to the raw locator. |
| `default` | no | Marks this entry as the primary registry short identifiers expand against. At most one entry may set it; when none do, the first entry is primary. |
| `include` | no | Array of glob patterns narrowing what this source **shows** when browsed. Unset (or `[]`) shows every repository. See [Browse filters](#browse-filters). |
| `exclude` | no | Array of glob patterns hiding matching repositories from this source's browse. Combines with `include` and wins wherever both match. Unset (or `[]`) hides nothing. See [Browse filters](#browse-filters). |
| `insecure` | no | Contact this registry over plain HTTP instead of HTTPS. Unset (the default) uses HTTPS. `oci` entries only — setting it on an `index` entry is a parse error (exit 78), since an index locator already carries its own scheme. See [Plain-HTTP registries](#plain-http-registries). |

```toml
#:schema https://grimoire.rs/schemas/grimoire-config.schema.json
[[registries]]
alias = "acme"
oci = "ghcr.io/acme"
default = true

[[registries]]
alias = "internal"
oci = "registry.corp.example/team"
```

**The same locator may be declared more than once.** Two entries over one
registry, each with its own alias and its own
[browse filter](#browse-filters), are two *views* of that source, and both are
browsed — one group per entry, and one `grim tui` root per entry, named by its
alias:

```toml
[[registries]]
alias = "grim"
index = "https://index.grimoire.rs"
exclude = ["michael-herwig"]
default = true

[[registries]]
alias = "mine"
index = "https://index.grimoire.rs"
include = ["michael-herwig"]
```

grim loads each *locator* once and hands the result to every entry that names
it, so a second view costs no extra network round trip. Only the entries'
filters differ in what they show.

An entry is identified by its locator **and its alias**, so this holds across
files too: the same `[[registries]]` array can appear in the global config
(`$GRIM_HOME/grimoire.toml`), and a project entry naming a globally-declared
locator under a different alias is a second view rather than a replacement.

What does collapse is a genuine repeat — the same alias at the same locator in
both files, which is the shape `grim init` writes when it snapshots an index
the global config already declares. Project entries take precedence, so the
project one wins and the global one is dropped, taking its browse filter with
it. grim warns when the dropped entry declared one:

```text
registry 'acme': repeats an earlier entry for 'ghcr.io/acme'; ignoring its include/exclude filter
```

A repeat that declares no filter is deliberately silent: layering a project
entry over an identical global one is a legitimate setup, and warning on it
would fire on every browse for no recoverable reason. Two locators can match
without looking alike — a trailing slash or a difference in host case is
normalized away before the comparison.

A global config that is **unreadable or invalid** fails the command at
project scope too — exit 78 (`EX_CONFIG`), the same code a global-scope
run gives — rather than quietly dropping every globally-declared registry
and browsing on. "Invalid" reaches further than a parse error: a config
that is well-formed TOML but fails registry validation (two entries
declaring `default = true`, for instance) is exactly as fatal, because
every load re-validates every `[[registries]]` entry — a file that survives
every editor, formatter, and TOML linter can still trip this.

This reaches every command that resolves a registry: [`grim
context`](./commands.md#context), [`grim add`](./commands.md#add), [`grim
login`](./commands.md#login), [`grim fetch`](./commands.md#fetch), [`grim
describe`](./commands.md#describe), [`grim search`](./commands.md#search)
(without `--registry`), and [`grim status
--check`](./commands.md#status-check) — plus the MCP `grim_fetch`,
`grim_describe`, and `grim_render` tools ([`grim mcp`](./commands.md#mcp)),
which share `fetch`/`describe`'s scope resolution. Three commands never
reach that read and still exit `0` on the same broken file: [`grim search
--registry <ref>`](./commands.md#search), which collapses the browse set
from the flag and returns before any config is consulted; [`grim
status`](./commands.md#status) without `--check`, which resolves no
registries at all; and [`grim logout`](./commands.md#logout) — the one
command here that erases a credential rather than writing one, so an
unreadable global config degrades to a warning instead of refusing (`grim
login` keeps the hard failure — storing a credential against a registry set
grim could not fully assemble is the more dangerous direction). An
**absent** global config is not an error anywhere.

**Backward compatibility**: a config that omits `[[registries]]` entirely
behaves exactly as before — `[options].default_registry`, the environment
variable `GRIM_DEFAULT_REGISTRY`, and the `--registry` flag still drive the
single-registry path. The two approaches do not mix: when any `[[registries]]`
entry is declared, `[options].default_registry` is ignored for browse purposes
(the `default = true` entry, or first entry, takes its role). The field is still
read for back-compat and never destroyed on re-serialize, but `grim init` now
writes the `[[registries]]` shape for new configs — `[options].default_registry`
is deprecated for new writes.

**`grim login` / `grim logout`**: a positional registry argument matching a
configured `[[registries]]` alias substitutes that entry's URL. With no
argument, the registry resolves through the same chain as `add`/`search`:
the `--registry` flag, `GRIM_DEFAULT_REGISTRY`, the project/global
`[[registries]]` default, then the legacy `[options].default_registry`
chain. Unlike `add`/`release`, `login`/`logout` never fall back to the
built-in default registry — with nothing configured anywhere, they fail
with exit 78 rather than silently storing (or erasing) a credential for a
registry you never named. See [`grim login`][grim-login].

**At-most-one `default = true`**: declaring two `[[registries]]` entries with
`default = true` is a parse error (exit 78). When none set it, the first entry
is the primary.

### Browse filters {#browse-filters}

Two optional glob lists on a `[[registries]]` entry — `include` and
`exclude` — narrow what that source shows when it is browsed.

One shared index is the whole point of an index: every team in the company
points at the same locator. But a platform team that only ever installs from
`acme/platform` still pages past marketing's and data's packages in
[`grim search`][grim-search] and the [TUI][grim-tui], and the usual fix is to
split the index in two — doubling the infrastructure to shorten one team's
list.

A filter narrows the view instead of the deployment. The index stays one
index; each consumer declares what it wants to see:

```toml
[[registries]]
alias = "acme"
index = "https://index.acme.internal"
include = ["acme/platform/**", "acme/tools/**"]
exclude = ["acme/platform/legacy/**"]
```

Every pattern is tested against two strings — the repository path
(`acme/tools`) and the fully-qualified reference (`ghcr.io/acme/tools`) — and
a hit on either counts. The patterns above carry no registry host, so they
match on every host; adding one narrows them to that host. "Patterns match
two candidates" further down explains it in full.

A repository is shown when the `include` list is empty **or** at least one
`include` pattern matches it, **and** no `exclude` pattern matches. The two
lists combine on the same entry — unlike [Cargo's `include`/`exclude`
fields][cargo-include], which are mutually exclusive — and `exclude` wins
wherever both match. An entry setting neither field is unfiltered and
behaves exactly as it did before the fields existed. The filter applies to
[`grim search`](./commands.md#search), the [TUI](./commands.md#tui), and the
MCP [`grim_search`](./commands.md#mcp) tool, and to nothing else — for a
filter that validates. An invalid pattern is a config error like any
other: it is rejected at exit `78` reading `grimoire.toml`, or exit `65`
writing it through `grim config`, which blocks the whole command, not just
browsing. "Nothing else" describes a compiled filter's runtime reach, never
a bad pattern's blast radius.

**A browse filter is not access control.** `include` and `exclude` govern
browse and search *rendering* — they are a view over a catalog listing, not
a boundary. A direct reference to an excluded package still resolves, locks,
and installs: `grim add ghcr.io/acme/internal/thing` succeeds against an
entry excluding `acme/internal/**`, and so do `grim lock`, `grim install`,
`grim fetch`, and `grim release`. [`grim status --check`](./commands.md#status-check)
likewise ignores every filter, so a deprecation notice on something you
already depend on can never be hidden by one. The only mechanism that
restricts what a user can actually pull is the registry's own pull
authorization.

The sharpest reason is structural, not a matter of coverage: **the source
being filtered controls the very string its own filter is matched against.**
A pattern is tested against the candidate derived from the row the source
served — for an [index](./package-index.md) entry, the `ref` the index
itself published. Re-publishing the same artifact under a pointer that spells
the repository differently produces a row the pattern no longer matches, and
the filter is not consulted about anything else. Nothing verifies that the
string a source hands over describes what it points at. Treat the filter as
what it is — the reader's own view setting — and never as a control over what
a source may show you.

**A filter that cannot be compiled is dropped whole.** A pattern is normally
rejected long before this — see "Writing the patterns" below — but if a
filter does reach the browse path uncompilable — a pattern *list*
that only fails once its patterns are compiled together — grim fails **open**:
it warns, discards **that entry's entire filter, `include` and `exclude`
both**, and browses the source unfiltered.

```text
registry 'acme': invalid include/exclude pattern (does not compile as part of a glob set: error building NFA); browsing without a filter
```

The exit code stays `0`, and [`grim context`](./commands.md#context) then
reports that entry with no filter at all — which is the signal to look for.
Note what this costs: an `exclude` you wrote stops hiding anything. A display
filter must never be the reason a catalog silently empties, and hiding is not
a guarantee the filter ever made (paragraph above), so degrading to *shows
more* is the correct direction — but if a row disappearing matters to you,
that is the point at which a filter was the wrong tool.

**Glob dialect.** `*` and `?` match within a single path segment and stop at
a `/`; only `**` crosses one — the same rule [gitignore][gitignore] and
[ripgrep][ripgrep] follow — so `acme/*` matches `acme/foo` but not
`acme/foo/bar`, while `acme/**` matches both. **Unlike gitignore's own
negation model**, though: `include` and `exclude` are two
independently-evaluated pattern sets, never one ordered list where a later
`!pattern` re-admits what an earlier line excluded. Declaration order never
matters, and `exclude` always wins over `include` regardless of which was
written first. Matching is case-sensitive. Brace alternation works:
`acme/{platform,tools}/**` is one pattern. A
pattern containing none of `* ? [ ] { } \` is **wildcard-free** and
auto-expands to also match everything beneath it, so `acme/platform` behaves
as `acme/platform{,/**}` — the common case needs no wildcards at all. Every
other pattern is used verbatim.

**The expansion grows downward only, never leftward.** `acme/platform`
becomes `acme/platform{,/**}` — a suffix — so it still has to match a
candidate from that candidate's very first segment. It is not a search for a
name appearing anywhere, and the second candidate does not rescue it: `hex`
matches neither `acme/arcana/hex` nor `ghcr.io/acme/arcana/hex`. This is the
shape that surprises people, because a bare name reads like one:

| Pattern | Candidate | Matches? |
|---|---|---|
| `hex` | `hex` | yes |
| `hex` | `hex/core` | yes — the `{,/**}` half |
| `hex` | `acme/arcana/hex` | **no** — nothing expands to the left |
| `**/hex` | `acme/arcana/hex` | yes |
| `**/hex*` | `acme/arcana/hex-core` | yes |
| `acme/**` | `acme/arcana/hex` | yes |

Write a leading `**/` when you mean "wherever it lives", and a namespace
prefix when you mean "everything under this owner". A bare name only ever
means "this exact path, and what is beneath it". grim says so when a filter
misses everything — `filter admitted 0 of 12 repositories` — so treat that
warning as this table.

A backslash escapes the metacharacter after it — `acme\*x` matches the
literal `acme*x` — and it does so **identically on every platform**,
including Windows. `grimoire.toml` is a file teams commit and share, so one
pattern has to mean one thing on every checkout; grim pins that rather than
inheriting the platform-dependent default its glob engine would otherwise
apply.

**Patterns match two candidates.** A pattern is tested against **two** strings
derived from the row — its `repository` path, and the fully-qualified
`registry/repository` reference — and a hit on either counts. The entry's own
`oci` / `index` locator is part of neither:

| This entry's locator | Catalog row | Bare candidate | Qualified candidate |
|---|---|---|---|
| `ghcr.io` | `ghcr.io/acme/tools` | `acme/tools` | `ghcr.io/acme/tools` |
| `ghcr.io/acme` | `ghcr.io/acme/tools` | `acme/tools` | `ghcr.io/acme/tools` |
| `https://index.grimoire.rs` | `quay.io/acme/tools` | `acme/tools` | `quay.io/acme/tools` |

One rule for both source kinds, and the same answer at every locator depth.
Two consequences follow directly, and they are the whole point:

- **A bare pattern is host-agnostic.** `include = ["acme/tools"]` admits that
  repository on every host the source serves.
- **A host-qualified pattern selects one host.**
  `include = ["ghcr.io/acme/tools"]` admits it only from `ghcr.io`, which is
  what lets an index spanning several registries be filtered per host.

The practical failures this deletes are all of the "it silently stopped
matching" class:

- **Editing an entry's own locator cannot re-aim its patterns.** Moving
  `oci = "ghcr.io/acme"` to `oci = "ghcr.io"` used to change what every
  pattern in that entry was matched against, so an `include` that had worked
  for months began matching nothing — valid config, exit `0`, empty catalog.
- **A case difference between locator and row no longer disables a filter.**
  It used to make the prefix strip quietly not fire. (Case still matters
  *inside* a pattern that spells a host — see the caveat below.)
- **A pattern is portable.** Copying `acme/platform/**` between two entries —
  at different depths, or from an `oci` entry to an `index` one — means the
  same thing in both.

**`exclude` wins once, over the combined verdict.** The two lists are not two
whole-filter answers OR-ed together: grim asks "did any `include` pattern hit
either candidate?" and "did any `exclude` pattern hit either candidate?", then
shows the row when the first is true and the second is false. So
`include = ["acme/tools"]` with `exclude = ["quay.io/acme/tools"]` hides
exactly the `quay.io` row and keeps every other host's — the host-qualified
`exclude` does not disarm the bare `include` everywhere.

**A mixed-case registry host does not match a lowercase pattern.** An entry
declared `oci = "GHCR.io/acme"` keeps that spelling into the qualified
candidate (`GHCR.io/acme/tools`), and matching is case-sensitive, so
`include = ["ghcr.io/**"]` admits nothing and `exclude = ["QUAY.IO/**"]`
hides nothing. The `include` direction warns (`filter admitted 0 of N`); the
`exclude` direction is **silent** — the same "an exclude you wrote stops
hiding anything" direction the fail-open above takes, on a surface that did
not exist before the host entered a candidate. This is a documented caveat,
not a fixed one: write a host in a pattern exactly as the entry's own locator
spells it. The repository half is unaffected — OCI repository paths are
lowercase by spec; only the host component may carry uppercase.

Neither candidate is the string the [TUI][grim-tui] tree prints beneath a
source's root. The tree attributes a row to the **longest** configured
locator, so with both `ghcr.io` and `ghcr.io/acme` declared the row
`ghcr.io/acme/tools/foo` *displays* as `tools/foo` while a filter matches it
as `acme/tools/foo` or `ghcr.io/acme/tools/foo`. The tree is a display and
reshapes itself as you add sources; a pattern must not. Write patterns from
the reference, not from the tree.

When a pattern does miss, the signal is a warning naming the source and the
counts:

```text
registry 'acme': filter admitted 0 of 148 repositories; patterns match either the repository path or the fully-qualified reference, and anchor at the candidate's first segment — see https://grimoire.rs/configuration.html#browse-filters
```

grim emits it once per affected source per browse, for **one shape only**: a
non-empty `include` list that admitted **nothing** from a group that had
rows. The count is the rows the filter was actually asked about — **only on
the unqueried browse** (`grim search` with no query, every TUI load): under
`grim search <query>` the count is what the query already matched, a
query-shaped subset indistinguishable from a deliberate search for a hidden
term, so the warning stays silent there and is decidable only on the full
listing.

A non-empty `exclude` that removes **nothing** does **not** warn. An earlier
revision briefly added that trigger — aimed at an `exclude` copied off a
visible row (`acme/internal/**` against an `oci = "ghcr.io/acme"` source is
a no-op) — and dropped it: `admitted N of N` is also the permanent, correct
state of an `exclude` with nothing to match yet
(`exclude = ["archive/**"]` before anything under `archive/` is published),
and the counts alone cannot tell the two apart, so the trigger warned on
every correct config forever. An **exclude-only** filter that empties a
source stays silent too: that is explicit intent, not a mis-aimed pattern.
Either way the exit code stays `0` — a filter matching nothing is legal —
and the source's tree root still renders, at a `0/0` rollup rather than
disappearing.

**A filter narrows the view, never the listing.** Each source's browse
window is built first and capped at **500 repositories**, and only then are
the patterns consulted, so a narrow filter cannot widen what grim looked at
— it can only show less of the same 500. This matters because it is the
opposite of the intuition a server-side filter creates: `include =
["acme/platform/**"]` does not make grim walk deeper into a large registry
looking for `acme/platform` matches, it discards non-matches from the window
it already had. On a registry big enough to hit the cap, the honest fix is a
narrower `oci` locator (`ghcr.io/acme/platform` rather than `ghcr.io`),
which moves the cut-line itself. `grim search <query>` also reports the cap
when a query's results may be incomplete:

```text
catalog listing capped at 500 repositories; results may be incomplete — narrow the query or use a more specific term
```

**Writing the patterns.** Hand-editing `grimoire.toml` is one way;
[`grim config`](./commands.md#config) is the other. The repeatable
`--include` / `--exclude` flags — on `grim config registry add <alias>` for a
new entry, on `grim config registry set <alias>` for one that already exists —
are the only CLI path that writes a multi-pattern list. `set` edits in place
and applies only the flags it is given, so it changes a filter without
disturbing the entry's locator, default flag, or position.
`grim config set registry.<alias>.include <glob>` replaces the whole list
with **exactly one** pattern — a comma is glob alternation syntax, never a
separator, so nothing is ever split on one.

**Clearing a list has two routes, and both are supported.** A list flag given
zero times means "leave this field alone", so emptying one needs its own
flag: `grim config registry set <alias> --clear-include` (and
`--clear-exclude`) empties that side while leaving the entry's locator,
default flag, position, and other list untouched. `grim config unset
registry.<alias>.include` does the same thing through the dotted-key surface.
Both are silent on any list length — including an already-empty one, which
exits `0` and leaves the rest of the entry untouched — and both write the
emptied list as **no key at all**, byte-identical to an entry that was never
filtered. The *file* is still rewritten by the lossy serializer described at
the top of this page; a clear changes nothing in the entry, not nothing on
disk.
`--clear-include` conflicts with `--include` on the same call (exit `64`);
naming no field at all is also exit `64`.

The two routes differ only in what they report. `registry set --clear-include`
reports `action: "registry-set"` with a `{"field":"include","action":"cleared"}`
row in its `fields` array; `config unset` reports `action: "unset"` with an
empty `fields`. See [the JSON interface](./json-interface.md#config-write-fields)
for the full write-report shape.

Calling `set` on an entry that already carries more than one pattern
**discards the rest**: exit `0`, with a warning naming how many were
dropped, because the surviving pattern makes the result read as an edit
rather than a partial wipe.

```text
registry.acme.include: `grim config set` writes ONE pattern and replaces the whole list — the 2 patterns already stored are discarded, not appended to. To write several, use `grim config registry set acme` with repeated --include/--exclude flags, which edits the entry in place; or edit `grimoire.toml` by hand.
```

That is the same reason the `filter admitted M of N` diagnostic goes quiet
here too: one surviving pattern leaves a partially-correct filter, and
otherwise the loss traces to nothing.

That last rule has a consequence worth stating, because the round trip looks
safe and is not. `grim config get` comma-joins a multi-pattern list for
display; feeding that string straight back to `set` stores it as **one**
literal pattern. It does not fail — a comma outside a `{…}` group is a valid
glob, so the value validates, is written, and the command exits `0` with a
warning:

```text
registry.acme.include: 'acme/platform/**,acme/tools/**' is stored as ONE pattern — a comma is glob alternation, never a separator. If these were meant as separate patterns, brace them into one glob (`{a,b}`) or write the list by hand in `grimoire.toml`.
```

Read the true array from `grim config get … --format json` instead, and write
a multi-pattern list with repeated `registry add --include` flags or by hand.

A pattern that is empty, whitespace-only, carries a control character,
exceeds **1024 bytes**, nests `{` more than **32** levels deep, or fails to
compile is rejected outright. A sixth cap bounds the **list**, not one
pattern: an entry's `include` (or `exclude`) list, summed as *compiled*
rather than as authored — a wildcard-free pattern's auto-expansion (the
`{,/**}` suffix above) counts too — must not exceed **64 KiB**, invisible
to the five per-pattern checks above since no single pattern can trip it
alone.

Every one of the six is exit **78** (`EX_CONFIG`) when grim reads it from a
config file — project or global, at either scope. At the CLI write
boundary the split matters: the five per-pattern caps reject at exit **65**
(`EX_DATAERR`) from `grim config set` or from `grim config registry
add`/`set`, which write nothing in that case; the list-byte budget is
reachable only through the repeated `--include`/`--exclude` flags on
`registry add`/`set` (also exit **65**) — a single `grim config set` call
writes exactly one
pattern, capped well under the list budget, so it can never trip the sixth
cap by itself. Every cap accepts the same set on both paths, because both
run the pattern through the same compilation the browse filter itself is
built by. An over-long pattern is echoed back truncated, with its true byte
count, rather than reprinting the whole thing at you.

### Qualified references {#qualified-references}

When registries have aliases, a reference can be qualified with
`alias/repo[:tag]` to expand the alias to its configured URL. For example,
with the config above:

```sh
grim add acme/code-review:1.2
# expands to: grim add ghcr.io/acme/code-review:1.2

grim add internal/lint-rules:stable
# expands to: grim add registry.corp.example/team/lint-rules:stable
```

The qualified form uses a slash separator (`alias/repo`), not a colon —
`alias:repo` would be ambiguous with `repo:tag`. A reference whose leading
`/`-segment does not match any alias is treated as a multi-segment
repository path under the primary registry, exactly as without aliases
configured.

Short references with no alias and no explicit registry still expand
against the primary (or only) registry, unchanged from the single-registry
behavior.

### Plain-HTTP registries {#plain-http-registries}

grim contacts every registry over HTTPS, with two exceptions: the loopback
forms `localhost` and `127.0.0.1` (bare and on port `5000`) are always
plain HTTP, and any host explicitly opted in.

Opt a declared registry in with `insecure`:

```toml
[[registries]]
alias = "local"
oci = "localhost:5050/grimoire"
insecure = true
```

or from the CLI:

```sh
grim config registry add local --oci localhost:5050/grimoire --insecure
grim config registry set local --insecure      # turn it on for an existing entry
grim config registry set local --no-insecure   # and back off
```

The host is matched **exactly, including its port** — `localhost:5050` and
`localhost` are different hosts. The opt-in applies to every reference to
that host for the rest of the invocation, not only to packages browsed
through this entry, and `grim login` pings it over plain HTTP too.

`GRIM_INSECURE_REGISTRIES` still works and is the way to reach a host no
`[[registries]]` entry declares — a `--registry` browse, a `grim login`
against an undeclared host, a one-off `grim fetch`. The two **add up**:
nothing takes a host back out of the plain-HTTP set, so there is no
config-versus-environment conflict to resolve.

> **A committed `grimoire.toml` downgrades transport for everyone who
> clones it.** The environment variable is per-user and per-shell;
> `insecure` in the config is not. Credentials and artifact bytes for that
> host travel in cleartext for every collaborator and every CI job that
> runs in the project. Use it for a registry that is genuinely local or
> in-cluster, and check the box knowingly.

`grim context --format json` reports each entry's `insecure` field, so a
UI can show which sources are HTTP. It echoes the authored value only —
a host reached over HTTP through the loopback default or the environment
variable still reports `false`.

`insecure` applies to `oci` entries only. An `index` locator is a URL that
already spells its own scheme, so setting the field there is a parse error
(exit 78 at load, 65 from `grim config set`).

### Registry compatibility {#registry-compatibility}

`grim search` and the TUI browse a registry's catalog through the
host-level OCI `_catalog` endpoint. Not all registries expose it —
multi-tenant SaaS registries such as [GitHub Container Registry][ghcr]
and the [GitLab Container Registry][gitlab-registry] gate the endpoint
for namespace-privacy reasons. When a registry does not support
`_catalog`, a browse comes back empty.

An empty browse result on these registries is **expected behavior, not
an error**. Install, add, release, and publish work through explicit
references and are unaffected — every registry in the table below
supports explicit-reference operations.

To browse packages hosted on a `_catalog`-gated registry, use a
[package index](./package-index.md) entry (`index = …`) instead of a
registry `url` — the index lists the packages; the registry only serves
them.

| Registry | `_catalog` browse (`grim search`, TUI) | Explicit-ref ops (install / add / release / publish) |
|---|---|---|
| `registry:2` (local) | yes | yes |
| [Zot][zot] | yes | yes |
| [Harbor][harbor] | yes | yes |
| [GitHub Container Registry (GHCR)][ghcr] | no | yes |
| [Docker Hub][dockerhub] | no | yes |
| [GitLab Container Registry (SaaS)][gitlab-registry] | no | yes |

When an online browse that includes a registry (`oci`) source comes back
empty, grim prints a hint pointing to this section so you can confirm
whether the registry supports `_catalog`. An index-only browse set is
exempt — an index never touches `_catalog`, and a failed index fetch
gets its own per-source warning instead.

## `grimoire.lock` {#grimoire-lock}

The lockfile pins every declared tag to an exact digest and records the
[scope's](./concepts.md#scopes) declaration hash so drift is detectable. It is
generated by [`grim lock`](./commands.md#lock), `grim add`, and the
[TUI's](./commands.md#tui) install action; treat it as machine-owned and
commit it alongside `grimoire.toml`:

```toml
[metadata]
lock_version = 1
generated_by = "grim 0.1.0"

[[skill]]
name = "code-review"
pinned = "ghcr.io/acme/code-review@sha256:…"

[[rule]]
name = "rust-style"
pinned = "ghcr.io/acme/rust-style@sha256:…"

[[agent]]
name = "code-reviewer"
pinned = "ghcr.io/acme/code-reviewer@sha256:…"
```

A member that came from a [bundle](./concepts.md#bundles) additionally carries
`bundle` and `bundle_tag` fields recording its origin; a directly-declared entry
omits them, so a bundle-free lock is byte-identical to one written before
bundles existed. A member that **several** declared bundles contributed (an
agreeing overlap) records every contributor in a `bundles` sub-table array
(`[[skill.bundles]]` rows with `repo` and `tag`) instead of the single pair —
removing one bundle then only strips its provenance entry, and the member
stays locked until the last contributing bundle is removed. The same
compatibility holds for agents: an agent-free lock carries no `[[agent]]`
array at all and is byte-identical to one written before agents existed.

A lock with declared bundles also caches each bundle's expansion result in a
`[[bundle]]` section — binding name, `repo`, `tag`, the resolved manifest
digest, and the member list as `[[bundle.member]]` rows:

```toml
[[bundle]]
name = "starter-pack"
repo = "ghcr.io/acme/bundles/starter-pack"
tag = "1"
pinned = "ghcr.io/acme/bundles/starter-pack@sha256:…"

[[bundle.member]]
kind = "skill"
name = "code-reviewer"
id = "ghcr.io/acme/code-reviewer:1"
```

This cache is what lets `grim remove` and `grim uninstall` work **offline**
on the *effective* declaration: before applying an edit they compute the set
of artifacts the declaration implies before and after, drop only what no
remaining declaration holds, and keep everything else. A bundle-free lock
carries no `[[bundle]]` section at all.

### Local path sources {#lock-path-sources}

A skill, rule, or agent [declared as a local path](./concepts.md#references-tags-and-digests)
pins by the SHA-256 of its canonical packed layer instead of a registry
digest: the entry carries `path` and `hash` and omits `pinned` entirely —
the two field sets are mutually exclusive on the wire, the same XOR shape
as the `bundle`/`bundle_tag` pair above.

```toml
[[skill]]
name = "my-skill"
path = "./skills/my-skill"
hash = "sha256:…"
```

A [local bundle](./concepts.md#bundles) follows the same shape in its
`[[bundle]]` cache entry: `path` and `hash` (of the canonical members
layer) replace `repo`/`tag`/`pinned` entirely — never a mix of the two.

```toml
[[bundle]]
name = "docs-stack"
path = "./bundles/docs-stack.toml"
hash = "sha256:…"

[[bundle.member]]
kind = "skill"
name = "code-reviewer"
id = "ghcr.io/acme/code-reviewer:1"
```

## Editor schema support {#editor-schema}

Both author-facing files ship a published [JSON Schema](https://json-schema.org/),
so an editor can autocomplete keys and flag a mistyped table name the moment
you save — instead of surfacing the error at the next `grim` run. The schemas
are generated from grim's own parser, so they accept exactly what grim accepts.

| File | Schema URL |
|------|------------|
| `grimoire.toml` | `https://grimoire.rs/schemas/grimoire-config.schema.json` |
| `publish.toml` | `https://grimoire.rs/schemas/grim-publish.schema.json` |
| `grimoire.lock` | `https://grimoire.rs/schemas/grimoire-lock.schema.json` |

[Taplo](https://taplo.tamasfe.dev/) and the
[Even Better TOML](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml)
VS Code extension bind a file to its schema through a first-line directive:

```toml
#:schema https://grimoire.rs/schemas/grimoire-config.schema.json
```

To regenerate or inspect a schema locally, use [`grim schema`](./commands.md#schema):
`grim schema --kind config` prints the `grimoire.toml` schema and
`grim schema --kind publish` prints the `publish.toml` one.

## Scopes on disk

A **project** config is the `grimoire.toml` discovered from the working
directory. The **global** config lives at `$GRIM_HOME/grimoire.toml` and is
selected with `--global`. See [Concepts](./concepts.md#scopes) for when each
applies.

## Environment variables

| Variable | Purpose | Default |
|----------|---------|---------|
| `GRIM_HOME` | Root data directory (cache, global config, global install state at `$GRIM_HOME/state/global.json`). Project install state lives at `<workspace>/.grimoire/state.json`, not here. | `~/.grimoire` |
| `GRIM_DEFAULT_REGISTRY` | Default registry for short references. | unset |
| `GRIM_OFFLINE` | Disable all network access (same as `--offline`). | `false` |
| `GRIM_INSECURE_REGISTRIES` | Comma-separated registries reachable over plain HTTP — for a host no `[[registries]]` entry declares. Adds to the [`insecure`](#plain-http-registries) field rather than overriding it. | unset |
| `GRIM_ANNOUNCE_TOKEN` | Forge API token for [`grim publish --announce`](./package-index.md#announcing) — always wins over CI-provided tokens. Sent as an API header only, never logged. | unset |
| `DOCKER_CONFIG` | Directory holding the Docker-compatible `config.json` that [`grim login`](./authentication.md) reads and writes. | `~/.docker` |
| `CLAUDE_CONFIG_DIR` | Claude Code config-dir override (vendor variable, honored read-only). Global-scope installs follow it: it replaces `~/.claude` for skills, rules, and agents, and relocates the global MCP registration file to `$CLAUDE_CONFIG_DIR/.claude.json`. Also drives global-scope client detection. | unset |
| `COPILOT_HOME` | GitHub Copilot home override (vendor variable). Replaces `~/.copilot` for global-scope skills and agents, and relocates `mcp-config.json`. Also drives detection. | unset |
| `OPENCODE_CONFIG_DIR` | OpenCode config-dir override (vendor variable). Preferred over the XDG default (`$XDG_CONFIG_HOME/opencode`) as the global-scope install target for skills and agents — additive, OpenCode scans both. Also drives detection. | unset |
| `OPENCODE_CONFIG` | OpenCode config **file** that grim edits for global-scope rule and MCP registration (read and written). Falls back to `$XDG_CONFIG_HOME/opencode/opencode.json`. No effect on skill/agent paths. | unset |
| `CODEX_HOME` | Codex home override (vendor variable). Replaces `~/.codex` for global-scope agents and the MCP `config.toml`. Does **not** relocate Codex skills — those follow the cross-vendor `$HOME/.agents/skills` standard. Also drives detection. | unset |
| `KIRO_HOME` | Kiro home override (vendor variable). Replaces `~/.kiro` **outright, with no `.kiro` segment appended**, for global-scope skills, `steering/` rules, and `settings/mcp.json`. Also drives detection. grim follows the Kiro **CLI**; the Kiro IDE ignores this variable upstream. | unset |
| `GEMINI_CLI_HOME` | Gemini CLI home override (vendor variable). It replaces the **home directory**, so Gemini's config root becomes `$GEMINI_CLI_HOME/.gemini` — the segment is still appended, the opposite shape to `CODEX_HOME`/`KIRO_HOME`. Relocates global-scope Gemini agents and its `settings.json` MCP registration, and drives detection. Does **not** relocate the shared `$HOME/.agents/skills` pool, which serves several clients under one refcount. | unset |
| `XDG_CONFIG_HOME` | Standard XDG base directory. Roots OpenCode's default config dir, Amp's settings dir, and — **on Linux and FreeBSD only** — Zed's. On macOS Zed uses a hardcoded `~/.config/zed`; on Windows, `%APPDATA%\Zed`. Also one of the candidate roots for Kilo and Goose *detection*. | `~/.config` |
| `SSL_CERT_FILE` | Path to a PEM bundle of extra CA roots for TLS. Merged with — never replacing — grim's built-in Mozilla roots (see [CA roots](#ca-roots)). | system default |
| `SSL_CERT_DIR` | Directory of PEM CA-root files for TLS, same merge semantics as `SSL_CERT_FILE`. | system default |
| `NO_COLOR` | Any non-empty value disables color under [`--color auto`](./commands.md#global-options) — the highest-priority `auto` signal, overriding even `CLICOLOR_FORCE`. Only `--color always` overrides it. | unset |
| `CLICOLOR_FORCE` | A non-empty value other than `0` forces color on under `--color auto`, even when stdout is not a terminal — beaten only by `NO_COLOR`. | unset |
| `CLICOLOR` | `0` disables color under `--color auto`. Any other value has no effect (color already follows the terminal check). | unset |
| `TERM` | `dumb` disables color under `--color auto`, the same terminal-capability convention most color-aware CLIs follow. | unset |

One further vendor variable is read for **detection only** and never changes
where grim writes: `$GOOSE_PATH_ROOT`. When it is set it **replaces** Goose's
candidate config roots rather than extending them.

`COPILOT_HOME` carries an upstream caveat worth stating separately from what
grim does with it. grim honors it for the **standalone** Copilot CLI, as the
table says. VS Code's *embedded* Copilot CLI ignores the variable
([microsoft/vscode#314806](https://github.com/microsoft/vscode/issues/314806),
open), so setting it moves grim's output for one and not the other. That is an
upstream split, not a limit on what grim honors.

Newly honoring a vendor variable relocates a render root for anyone who
already set it. That is a layout move, not a breaking change — grim reaps
the pre-override copy on the next install, update, or uninstall. See
[Upgrading](./upgrading.md#relocated-roots) for what to expect, and
[Stability](./stability.md#unstable) for what the promise covers.

Announce additionally reads the standard CI variables (`GITHUB_ACTIONS`,
`GITHUB_SERVER_URL`, `GITHUB_API_URL`, `GITHUB_REPOSITORY_OWNER`,
`GH_TOKEN`/`GITHUB_TOKEN`; `GITLAB_CI`, `CI_SERVER_HOST`, `CI_API_V4_URL`,
`CI_PROJECT_NAMESPACE`, `GITLAB_TOKEN`) — only when the CI server host
equals the announce target host. On GitLab CI, `CI_JOB_TOKEN` is checked
for **presence only**: when set and the index host matches
`CI_SERVER_HOST`, grim hands git a fallback transport credential for the
announce push — the value itself is never read into grim and never used
for the MR API. See
[Announcing Packages](./package-index.md#announcing).

By default Grimoire resolves floating tags fresh from the registry, then caches
the result, so a floating tag never serves a stale pin. Pass `--offline` (or set
`GRIM_OFFLINE`) to work from the cache alone and fail rather than reach the
network.

A command-line flag always wins. Registry resolution operates on two separate
precedences depending on context:

**Browse-set** (what `grim search`, the TUI, and `grim mcp` browse): `--registry`
flag → project `[[registries]]` → global `[[registries]]` → single default
(`GRIM_DEFAULT_REGISTRY` → project `[options].default_registry` → global
`[options].default_registry` → built-in `https://index.grimoire.rs`, the
public [package index](./package-index.md)). The single-default tier
applies only when no `[[registries]]` array is declared anywhere. The same
chain applies outside a project — with no `grimoire.toml` resolvable the
project tiers are simply absent, so a bare `grim search` still browses the
global `[[registries]]` and otherwise falls through to the built-in
index. Only the
`--registry` flag collapses browse — to exactly the registries it names
(repeatable / comma-separated); `GRIM_DEFAULT_REGISTRY` does
not restrict the browse set when `[[registries]]` is configured.

**Short-id resolution** (expanding a bare `name:tag` to a full registry URL):
`--registry` flag → `GRIM_DEFAULT_REGISTRY` → project `[options].default_registry`
(or the primary entry of project `[[registries]]`) → global config → built-in
`ghcr.io/grimoire-rs`. Index sources never expand short ids — with an
index-only browse set the push-side fallback applies.

The `--offline` toggle has no config-file counterpart — the flag or its `GRIM_OFFLINE` variable applies.

## CA roots {#ca-roots}

Every grim registry and package-index call is HTTPS, so it needs a set of
trusted CA roots. Tools that rely solely on the host trust store break the
moment there isn't one — a [distroless][distroless] or minimal CI image ships
without `ca-certificates`, and grim's rustls stack treats an empty system store
as a hard error rather than falling back to nothing.

grim avoids that by compiling the [Mozilla CA root set][webpki-root-certs] into
the binary. Those public roots are always available, so a registry pull works
on a bare container with no trust store installed.

The built-in roots are *merged* with the host trust store, never a replacement
for it. A private or corporate CA supplied through the standard OpenSSL
`SSL_CERT_FILE` (a PEM bundle) or `SSL_CERT_DIR` (a directory of PEM files)
overrides is trusted *alongside* the public roots — so an internal registry
behind a corporate CA and a public one both verify from the same process.

## Data layout

The resolved-artifact content store, the catalog cache that
[`grim search`](./commands.md#search) and the [TUI](./commands.md#tui) read, and
the **global** install state (`$GRIM_HOME/state/global.json`) all live under
`GRIM_HOME`. Keeping cache and global state under one directory means installs
can use atomic, same-filesystem operations.

**Project install state** is separate: it lives at
`<workspace>/.grimoire/state.json`, co-located with `grimoire.toml`. The
workspace directory is the key, so two projects sharing the same `GRIM_HOME`
volume cannot collide. Grim writes a self-managed `.grimoire/.gitignore`
(contents: `*`) the first time it creates the `.grimoire/` directory, so the
state file is kept out of version control without touching your root
`.gitignore`.

<!-- internal -->
[grim-tui]: ./commands.md#tui
[grim-search]: ./commands.md#search
[grim-config]: ./commands.md#config
[grim-add]: ./commands.md#add
[grim-remove]: ./commands.md#remove
[grim-login]: ./authentication.md#login

<!-- external -->
[ghcr]: https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry
[gitlab-registry]: https://docs.gitlab.com/ee/user/packages/container_registry/
[zot]: https://zotregistry.dev/
[harbor]: https://goharbor.io/
[distroless]: https://github.com/GoogleContainerTools/distroless
[webpki-root-certs]: https://crates.io/crates/webpki-root-certs
[dockerhub]: https://hub.docker.com/
[cargo-include]: https://doc.rust-lang.org/cargo/reference/manifest.html#the-exclude-and-include-fields
[gitignore]: https://git-scm.com/docs/gitignore#_pattern_format
[ripgrep]: https://github.com/BurntSushi/ripgrep
