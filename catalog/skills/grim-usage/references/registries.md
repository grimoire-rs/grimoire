# Registries, Scopes, and Targets

You loaded this file because you need to resolve which registry a short
reference hits, which scope a command edits, which AI clients an install
lands in, how offline mode behaves, or how to search a catalog.

Contents: [Registry Resolution](#registry-resolution) ·
[Multiple Registries](#multiple-registries) ·
[Browse Filters](#browse-filters) ·
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
| `GRIM_INSECURE_REGISTRIES` | Comma-separated plain-HTTP registries, for a host no `[[registries]]` entry declares — adds to the `insecure` field, never overrides it |
| `DOCKER_CONFIG` | Directory of the Docker-compatible credential `config.json` |

Separately, grim honors each **client's own** directory-override variable
(`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `KIRO_HOME`, `GEMINI_CLI_HOME`,
`COPILOT_HOME`, `OPENCODE_CONFIG_DIR`, …) so a global-scope install lands
where that client actually reads. Their shapes are **not** uniform — some
replace the client's config dir outright, others replace the home
directory with the vendor segment still appended — so read the exact one
you set in [Configuration][envvars] rather than reasoning by analogy.
Two things follow. Setting one relocates that client's render root, and
grim reaps the copy stranded at the old root on the next `install`,
`update`, or `uninstall` (a copy you hand-edited is kept and warned about,
never deleted). And they drive global-scope client *detection*: a client
counts as present when its overridden root exists.

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
| `include` | no | Glob patterns narrowing what this source **shows** when browsed; unset or `[]` shows everything — see [Browse Filters](#browse-filters) |
| `exclude` | no | Glob patterns hiding matching repositories from this source's browse; combines with `include` and wins where both match |
| `insecure` | no | Contact this registry over plain HTTP instead of HTTPS — see [Plain HTTP](#plain-http); `oci` entries only |

```toml
[[registries]]
alias = "acme"
oci = "ghcr.io/acme"
default = true

[[registries]]
alias = "internal"
oci = "registry.corp.example/team"
```

Project entries take precedence over global entries. Deduplication keys on
the **locator *and* the alias together** (trailing slashes and host case
ignored), first occurrence wins — so only a genuine repeat collapses, which
is how a project entry shadows its global twin. One locator declared twice
under two different aliases is **two** browse sources on purpose: they are
two filtered views of one registry, and collapsing them would silently
discard one entry's `include`/`exclude`. An entry with no alias keys on the
locator alone.

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

Running your own index is one command: `npx @grimoire-rs/indexer init`
scaffolds an index repository — pointer tree, site config, and CI that
builds the catalog site and gates contributions — served from GitHub or
GitLab Pages. A plain git repository holding `index/**/metadata.json`
also works as-is, with no build step. Full walkthrough:
[Host Your Own Index][hosting].

`oci` and `index` set together on one entry is a config error (exit 78);
a locator that matches neither transport shape is a data error (exit 65).
Both transports share the regular catalog cache (`$GRIM_HOME/catalog/`,
1-hour TTL, `--refresh`, offline degradation) and browse exactly like an
`oci` entry — search, TUI, and MCP treat index and registry sources
alike. `grim publish --announce` is the write side: it publishes
pointers into an index repository rather than reading them — see
[references/publish.md](publish.md#announce).

### Plain HTTP {#plain-http}

grim uses HTTPS for every registry except the loopback forms `localhost` and
`127.0.0.1` (bare and on port `5000`), which are always plain HTTP. Opt any
other host in per entry:

```toml
[[registries]]
alias = "local"
oci = "localhost:5050/grimoire"
insecure = true
```

The host is matched **exactly, including its port** — `localhost:5050` and
`localhost` are different hosts — and the opt-in covers every reference to
that host for the invocation, `grim login`'s verification ping included.
`--registry` narrows what is *browsed*, never what may be reached over
plain HTTP, so it never disarms the opt-in.
`GRIM_INSECURE_REGISTRIES` still reaches a host no entry declares (a
`grim login` against an undeclared host, a one-off `grim fetch`); the two
**add up**, so there is no config-versus-environment conflict.

It does not widen where a credential may go: a registry reached over HTTPS
must keep the credential on HTTPS, so a `Bearer realm="http://…"` challenge
from it is refused regardless of what any entry declared.

Two things to say out loud when recommending it. `grimoire.toml` is
normally committed, so the downgrade applies to every collaborator and CI
job — unlike the environment variable, which is per-shell. And it is `oci`
entries only: pairing it with `index` is an error (`65` from `grim config`,
`78` at load), because an index locator already spells its own scheme.

`grim context --format json` reports each entry's `insecure`, so a UI can
show which sources are HTTP. It echoes the authored field, not the effective
transport — a host reached over HTTP through the loopback default or the
environment variable still reports `false`.

## Browse Filters {#browse-filters}

A `[[registries]]` entry may carry two glob lists, `include` and `exclude`,
that narrow what that source shows in `grim search`, the TUI, and the MCP
`grim_search` tool. They let one shared index serve several teams without
splitting it into several indices.

```toml
[[registries]]
alias = "acme"
index = "https://index.acme.internal"
include = ["acme/platform/**", "acme/tools/**"]
exclude = ["ghcr.io/acme/platform/legacy/**"]
```

Both spellings work, and they mean different things. Every pattern is tested
against **two** strings — the repository path (`acme/tools`) and the
fully-qualified reference (`ghcr.io/acme/tools`) — and a hit on either
counts. So the bare `include` patterns above admit those namespaces on
whatever host the index serves them from, while the host-qualified `exclude`
hides the legacy subtree on `ghcr.io` only. Same rule for an `oci` entry and
an `index` entry; see [Pattern rules](#pattern-rules).

A repository is shown when the `include` list is empty **or** an `include`
pattern matches it, **and** no `exclude` pattern matches. The two lists
combine on one entry (unlike Cargo's mutually exclusive pair) and `exclude`
wins where both match. An entry setting neither is unfiltered.

Exclude-wins is applied **once**, to the combined verdicts — not as two
whole-filter answers OR-ed together. `include = ["acme/tools"]` with
`exclude = ["quay.io/acme/tools"]` therefore hides exactly the `quay.io` row
and keeps every other host's; the host-qualified `exclude` does not disarm
the bare `include` everywhere.

### Not access control {#not-access-control}

**A browse filter is not access control. Never present it to a user as
one.** `include`/`exclude` govern browse and search *rendering* — nothing
else:

- A direct reference to an excluded package still **resolves, locks, and
  installs**. `grim add ghcr.io/acme/internal/thing` succeeds against an
  entry excluding `acme/internal/**`, and so do `grim lock`, `grim
  install`, `grim fetch`, and `grim release`. If you are asked to keep
  someone from *installing* a package, a filter does not do it.
- `grim status --check` ignores every filter, so a deprecation notice on an
  artifact the project already declares can never be hidden by one.
- **The filtered source controls the string its own filter is matched
  against.** Patterns are tested against the candidate derived from the row
  the source served — for an index entry, the `ref` the index itself
  published. The same artifact re-published under a differently-spelled
  pointer yields a row the pattern no longer matches, and nothing checks
  that a source's string describes what it points at. This is the sharpest
  reason a filter can never be a privacy boundary: it is the reader's own
  view setting, not a control over what a source may show.
- A filter that reaches the browse path uncompilable **fails open**: grim
  warns and drops **that entry's whole filter — `include` and `exclude`
  both** — browsing the source unfiltered, at exit `0`. It does not skip
  one list and keep the other. An `exclude` you wrote therefore stops
  hiding anything; `grim context` then shows that entry with no filter,
  which is the signal to look for. A filter never empties a catalog by
  failing — but never rely on it to keep something hidden either.
- The only mechanism that restricts what a user can actually pull is the
  **registry's own pull authorization**. Say so when a filter is proposed
  as a privacy or permission boundary.
- **An invalid pattern is a config error like any other, not a browse-only
  failure.** A malformed pattern is rejected at write time (`grim config
  set` / `registry add`, exit `65`, nothing written) or at load time (a
  hand-edited `grimoire.toml`, exit `78`) — before any command runs, browse
  or otherwise. "Nothing else" above describes a *compiled* filter's reach
  at runtime, never a bad pattern's blast radius, which blocks the whole
  invocation.

### Pattern rules {#pattern-rules}

- `*` and `?` stop at a `/`; only `**` crosses one (the gitignore dialect).
  `acme/*` matches `acme/foo`, not `acme/foo/bar`. Matching is
  case-sensitive. A backslash escapes the next metacharacter (`acme\*x`
  matches the literal `acme*x`), and does so identically on every platform
  including Windows — a committed `grimoire.toml` means one thing on every
  checkout.
- A pattern containing none of `* ? [ ] { } \` auto-expands to also match
  everything beneath it: `acme/platform` behaves as `acme/platform{,/**}`.
  Every other pattern is used verbatim. Brace alternation is one pattern:
  `acme/{platform,tools}/**`. The expansion is a **suffix**, so every
  pattern still anchors at the first segment of whichever candidate it is
  tested against — `hex` matches neither `acme/arcana/hex` nor
  `ghcr.io/acme/arcana/hex`; write `**/hex` for "wherever it sits".
- **Two candidates, a hit on either counts.** Every pattern is tested
  against the row's repository path **and** its fully-qualified
  `registry/repository` reference. For the row `ghcr.io/acme/tools` those
  are `acme/tools` and `ghcr.io/acme/tools`. One rule for an `oci` entry
  and an `index` entry alike — there is no per-kind branch and no
  host-detection heuristic.
- Consequently a **bare** pattern is host-agnostic (`acme/tools` admits
  that repository from every host a source serves) and a **host-qualified**
  pattern selects one host (`ghcr.io/acme/tools` admits it only from
  `ghcr.io`). That is how an index spanning several registries is filtered
  per host.
- **The entry's own `oci`/`index` locator is part of neither candidate.**
  Editing the locator cannot re-aim a pattern written against it, and a
  pattern copied between two entries — at different depths, or from an
  `oci` entry to an `index` one — means the same thing in both. Never tell
  a user to write a pattern relative to their locator; that rule was
  removed.
- Neither candidate is what the TUI tree shows beneath the source's root.
  The tree strips the *longest* locator across **all** entries; a pattern
  strips nothing. With both `ghcr.io` and `ghcr.io/acme` declared,
  `ghcr.io/acme/tools/foo` displays as `tools/foo` yet a filter matches it
  as `acme/tools/foo` or `ghcr.io/acme/tools/foo`. Write patterns from the
  reference, never from the tree.
- **A mixed-case registry host does not match a lowercase pattern.** An
  entry declared `oci = "GHCR.io/acme"` keeps that spelling into the
  qualified candidate, and matching is case-sensitive, so
  `include = ["ghcr.io/**"]` admits nothing (warns) and
  `exclude = ["QUAY.IO/**"]` hides nothing (**silently**). Documented
  caveat, not a fixed one: spell a host in a pattern exactly as the entry's
  locator spells it. Only the host half is affected — OCI repository paths
  are lowercase by spec.
- A pattern that matches nothing is legal. The one signal is a stderr
  warning; the exit code stays `0` and the source's tree root still renders
  at a `0/0` rollup:

  ```text
  registry 'acme': filter admitted 0 of 148 repositories; patterns match either the repository path or the fully-qualified reference, and anchor at the candidate's first segment — see https://grimoire.rs/configuration.html#browse-filters
  ```

  Emitted once per affected source per browse, for one shape only: a
  non-empty `include` list that admitted **nothing** from a group that had
  rows. The count is the rows the filter was asked about — under `grim
  search <query>`, what the query already matched, so the warning is silent
  under a non-empty query (a search for a deliberately-hidden term looks
  identical to a mis-aimed filter, and a warning that fires on the ordinary
  path stops being read on the rare one). A non-empty `exclude` that removes
  **nothing** has no warning at all — a correct `exclude` waiting for a
  repository that does not exist yet (`exclude = ["archive/**"]` before
  anything under `archive/` is published) looks identical to one copied off
  a displayed row, and the counts alone cannot tell them apart, so that
  trigger would cry wolf on every correct config forever. An
  **exclude-only** filter that empties a source stays silent too: that is
  intent, not a mistake.
- **The filter narrows the view, never the listing.** Each source's browse
  window is built and capped at **500 repositories** first; the patterns run
  afterwards. A narrow filter can never widen what grim looked at — on a
  registry big enough to hit the cap, narrow the `oci` locator instead
  (`ghcr.io/acme/platform`, not `ghcr.io`). Never tell a user a filter will
  surface packages a plain browse missed.

### Writing filters {#writing-filters}

```sh
grim config registry add acme --oci ghcr.io/acme \
  --include 'acme/platform/**' --include 'acme/tools/**' \
  --exclude 'acme/platform/legacy/**'

grim config registry set acme \
  --include 'acme/platform/**' --include 'acme/tools/**'   # edit in place
grim config registry set acme --clear-exclude              # empty one list
grim config set registry.acme.include 'acme/{platform,tools}/**'
grim config get registry.acme.include --format json
```

`--include`/`--exclude` are repeatable and accumulate; **neither is ever
comma-split**, because a comma is glob alternation syntax. `add` (new entry)
and `set` (existing entry) are the only CLI paths that write a multi-pattern
list — `grim config set registry.<alias>.include` replaces the whole list
with **exactly one** pattern, and warns naming the discarded count when the
entry carried more than one. Confirm the current flags with `grim config
registry set --help`.

Growing a filter on an existing entry is `grim config registry set`, **not**
a second `add` and no longer a re-create: `grim config registry add acme …`
on an alias that already exists is a usage error (`64`). `set` applies only
the flags it is given, leaving the entry's locator, default flag, and
position untouched — position matters, because when no entry declares
`default` the *first* one wins, so the old `rm` + re-`add` round trip could
silently move the default.

Emptying a list has **two** supported routes, and a list flag given zero
times is neither of them (that means "leave this field alone"):

```sh
grim config registry set acme --clear-include      # flag route
grim config unset registry.acme.include            # dotted-key route
```

Both are silent at every list length — including an already-empty list, which
exits `0` and leaves the rest of the entry untouched — and both write the
emptied list as **no key at all**, indistinguishable from an entry that was
never filtered. The file itself is still rewritten by the lossy serializer
(below); "unchanged" is about the entry, not the bytes. `--clear-include` conflicts with `--include` on the same call
(`64`); a `set` naming no field at all is also `64`. The two differ only in
their JSON report: the flag route is `action: "registry-set"` with a
`{"field":"include","action":"cleared"}` element in `fields`, the dotted-key
route is `action: "unset"` with `fields: []`.

`registry set`'s write report carries an always-present `fields` array — one
element per field the call wrote, in the frozen `oci, index, default,
include, exclude, insecure` order, each `{"field":…,"action":"set","value":…}` or
`{"field":…,"action":"cleared"}` (a cleared element has no `value` key).
Every other write verb reports `fields: []`. It describes the write, not a
diff: a field named with the value it already held still emits its element,
and a `--oci`/`--index` kind swap emits two (the named side `set`, the
emptied side `cleared`).

`grim config get` comma-joins a multi-pattern list for display and is
**not round-trippable**: feeding that string back to `set` stores it as one
literal glob. It does **not** fail — a comma outside `{…}` is a valid glob,
so the value validates, is written, and the command exits `0` with a warning
that it was stored as one pattern. Never treat that round trip as safe
because it "would error"; it does not. Use `--format json` for the true
array (`get` on an empty list exits `1`).

A pattern that is empty, whitespace-only, carries a control character,
exceeds 1024 bytes, nests `{` more than 32 levels deep, or fails to compile
is rejected: exit `78` when read from a config file — project or global, at
either scope — and `65` through `grim config set` / `registry add`, which
then write nothing. A sixth cap bounds the **list** rather than one
pattern — an entry's `include`/`exclude` list, summed as compiled, must not
exceed 64 KiB — invisible to the five caps above since no single pattern
can trip it alone. It is exit `78` from a config file too, but at the CLI
write boundary it is reachable only through `registry add`'s accumulated
flags (also exit `65`): `grim config set` writes exactly one pattern per
call, capped well under the list budget, and can never trip it by itself.
Every cap accepts exactly the same set on both paths, because both compile
the pattern the way the browse filter itself is built. Exact validation
messages live in the [command reference][config-cmd]; never quote them from
memory.

Full reference, including the `grim context` reporting: [Browse
filters][browse-filters].

## Managing Config {#managing-config}

`grim config` reads and writes `grimoire.toml`, modeled on `git
config`, so you rarely hand-edit the file. It covers **settings**
(`[options]`, `[options.tui]`, `[options.vendors.<name>]`) and **named
registries** (`[[registries]]`) —
but **not declarations** (`[skills]` / `[rules]` / `[agents]` / `[mcp]` /
`[bundles]`),
which stay under `grim add` / `grim remove` because those must re-resolve the
lock on every change.

- **Settings** use dotted keys — `grim config get|set|unset <key>` and
  `grim config list`:

  ```sh
  grim config set   options.clients claude,opencode
  grim config set   options.tui.default_view tree
  grim config set   options.show_deprecated true   # show deprecated artifacts by default
  grim config set   options.vendors.cursor.shared_skills true  # per-client: install this client's skills into the shared .agents/skills pool
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
  grim config registry set acme --oci ghcr.io/acme2   # edit in place; unnamed fields keep their value
  grim config registry set acme --clear-include       # empty a browse-filter list
  grim config registry use acme                       # set default, clearing all others atomically
  grim config registry show acme                      # one entry's fields
  grim config registry list                           # all entries in this scope
  grim config registry rm  acme
  grim config registry set local --insecure            # plain HTTP for this entry; --no-insecure turns it back off
  grim config registry fields                         # per-field metadata (oci/index/default/include/exclude/insecure) — works with no config at all
  ```

  `registry set` edits an **existing** entry, applying only the flags it is
  given and keeping the entry's position; `add` refuses an alias that
  already exists (`64`). It takes `add`'s `--oci` / `--index` /
  `--include` / `--exclude` / `--default` / `--insecure` plus
  `--clear-include` / `--clear-exclude` / `--no-insecure`, which `add` does
  not have. `--oci` and `--index` swap the entry's kind, clearing the other
  side. `--default` sets the flag and clears every other entry's; it cannot
  *unset* one — move the default by naming another entry. `--insecure`
  **can** be turned back off, with `--no-insecure`.

  `registry use` is the correct way to change the default registry.

Scope follows the usual rule — project by default, `--global` for the global
config, `--config <path>` for an explicit file; each invocation reads or
writes exactly one scope (never merged). `get` prints the bare value so it
scripts cleanly (`$(grim config get options.clients)`), exiting `1` when the
key is valid but unset. Add `--format json` to any subcommand for
machine-readable output.

**Every grim write is lossy**: comments are stripped from `grimoire.toml` on
any `grim config` / `grim add` / `grim remove`, and so is any key whose value
collapses to the default. The one exception is a **leading `#:schema` editor
directive, which every rewrite preserves** at the top of the file. The full dotted-key list, JSON shapes, and exit codes live in the
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

An installed artifact lands in a **client target** — `claude`, `opencode`,
`copilot`, `codex`, `cursor`, `kiro`, `junie`, `gemini`, `zed`, `amp`,
`antigravity`, `cline`, `droid`, `goose`, `warp`, `openclaw`, `kilo`, or
the vendor-neutral `agents` — each receiving the artifact in its native
layout. The set grows; `grim context` reports the names that resolve for
your scope, and the [Client Compatibility matrix][clients-matrix] is
authoritative.
`grim install` and `grim update` choose targets by precedence:

1. `--client <list>` flag (comma-separated: `--client claude,copilot`)
2. config `[options].clients` (TOML array of client names)
3. auto-detection — every client whose marker exists for the active
   scope (e.g. a `.claude/` directory in the project)
4. the generic `agents` client when nothing is detected — one copy into
   the cross-vendor `.agents/skills` pool that several clients already
   read, instead of a copy into every vendor directory grim knows about

`agents` renders **skills only**; it has no vendor-neutral surface for
rules, agents, or MCP servers. So if nothing is detected and your lock
holds *only* those kinds, the install has nothing it can write: `grim
install` — and `grim add` on such an artifact — exits **78** and tells you
to name a client with `--client` or `[options].clients`. `grim add` still
writes the declaration and the lock entry first, so a follow-up `grim
install --client <name>` completes without re-adding.

The detected set is recomputed each run and never written back to config —
except by `grim init`, which seeds `[options].clients` with what it detects
at the moment you create the file. Pin `[options].clients` when you want
deterministic targets in CI — set it with `grim config set options.clients
claude,opencode` (see [Managing Config](#managing-config)).

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
each term fuzzy-matches (case-insensitive) any of an entry's kind,
repository, summary, description, or keywords. Fuzzy means subsequence, as
in fzf: the letters must appear in order but need not be adjacent, so
`kubctl` finds `kube-control` (a mistyped letter is not forgiven — only a
missing one). Results are ranked by relevance, best first, across all
browsed registries; the unqueried browse is unranked and lists registry by
registry. A bare kind keyword (`skill`/`rule`/`bundle`, singular or plural)
filters by kind instead of matching as text; an empty query lists the whole
catalog. Confirm the match fields and kind-filter keywords with
`grim search --help`. When
`[[registries]]` are configured, all
of them are browsed and flattened into one table. The catalog is cached
under `$GRIM_HOME` — pass `--refresh` to rebuild it from the registry,
`--registry` to collapse the browse to exactly the registries it names
(repeatable / comma-separated for several at once). Plain
output shows the one-line summary (truncated to the terminal); piped
output and `--format json` keep the full description, and JSON adds a
`repository` URL field for tooling.

Each JSON item also carries a `source` object — `{alias, locator}` — naming
the `[[registries]]` entry it was browsed from, the same attribution the TUI
roots its tree by. Group a flat result set by its source with
`jq '.items | group_by(.source.alias // .source.locator)'`. `alias` is `null` when the
entry declares none (and under `--registry`); `locator` is the configured
value verbatim, which `repo` does not carry — `repo` names the artifact's own
registry host, and one index source serves rows from many hosts.

```sh
grim search review
grim search --refresh --registry ghcr.io/acme --format json
grim search --format json | jq '.items | group_by(.source.alias // .source.locator)'
```

A registry declaring a [browse filter](#browse-filters) contributes only the
repositories its patterns admit — to `grim search`, the TUI, and
`grim_search` alike. A filtered source that contributes nothing logs
`registry '<name>': filter admitted 0 of <N> repositories` on stderr and
still exits `0`; in the TUI its root stays visible at a `0/0` rollup.
`--registry` browses **unfiltered**: a forced browse set is exactly what the
flag names. `grim status --check` is never filtered either. If a search
comes back thinner than expected, check the entry's `include`/`exclude`
before blaming the registry — `grim context --format json` reports the
resolved patterns per source.

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
detail pane per entry. That pane is always live for the selection — there is no
focus to enter, so `esc` quits on the first press. It carries a fixed
three-panel strip in its top border: `tab` / `shift-tab` cycle
`Overview` / `Readme` / `Changelog` on every row, and a panel the repository
did not publish is greyed and reads `not available` rather than disappearing.
grim fetches the entry's
[description companion](publish.md#description-companion) once per repository
per session as soon as the selection holds still, filling the document panels
and adding a `Support:` section to Overview with the repository's issue
tracker, chat, contact, and security links. Those channels are deliberately
absent from `grim search` and from the cached catalog row — they are
repository-level and mutable, so only a live read is trustworthy. Press `t` to
toggle between the flat list and a
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

Three row states are worth knowing before you press anything, because each
one changes what the action keys do:

- **`+ pending`** marks an artifact that is installed but does not yet cover
  every client it should — the same materialization drift `grim status`
  reports as `outputs_pending`. Nothing is broken and nothing is out of
  date: `i` writes the missing outputs and clears the badge. `u` and `d`
  work on a pending row too, making it the one state both action sets share.
- **The Overwrite dialog** is the integrity gate seen from the TUI. `i` and
  `u` both refuse an artifact whose bytes drifted from the hash grim
  recorded — `u` included, since 0.13.0 — and the refusal opens a modal
  offering the forced retry. Answering that modal is what supplies
  `--force`; the key you pressed does not. Only a single-artifact action
  offers it, since one answer cannot speak for several artifacts, so a batch
  leaves its refusals in the status line instead. A retry that refuses again
  does not re-open the dialog, so there is no confirm loop to get stuck in.
- **A bundle row folds in its members' health.** Declaration is the gate — a
  bundle absent from `[bundles]` reads `not installed` whatever its members
  look like — but past it the row shows the *worst* member state, at the
  precedence `integrity-missing > modified > outdated > pending >
  installed`. A member that was never materialized folds in as `pending`,
  not `not installed`, so no member can drag the row back across the line
  declaration owns, and the bundle stays actionable as a unit.

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

## Ratings and Voting {#ratings}

Some indexes publish community ratings — a `stats.json` sidecar served
beside `all.json`, tallied from upvotes on the index operator's own forge
threads. There is no Grimoire service holding them; the forge *is* the
database. When the browsed index publishes them, `grim search --format
json` carries a `rating` object per row (`{up, url}`, or `null` when
unrated), the TUI detail pane gains a `Rating:` row, and `--sort rating`
orders the browse by upvotes.

Two facts decide whether you see any rating at all, and neither is an
error condition:

- **Ratings ride the HTTP index transport only.** A git-transport index
  and an OCI registry browsed by `_catalog` read unrated, permanently.
- **Absence is never an error.** No sidecar, no entry for a ref, or a
  document from a newer schema all mean *unrated*, logged at `debug`. A
  browse never fails over ratings, and `null` never means `0`.

`--sort <name|updated|rating>` applies to `grim search` and `grim tui`
alike. Unrated and undated artifacts sort into a bucket of their own at
the *end* rather than as zero votes or epoch 0, and every mode is total —
two runs over the same catalog render identically. Given together with a
query, `--sort` **replaces** relevance ranking rather than composing with
it; omitted, ordering is exactly what it was before the flag existed.
Confirm with `grim search --help`.

`grim rate <ref>` casts a vote. It posts publicly under **your own** forge
account, so an interactive run confirms first and a non-interactive one
must pass `--yes` (it exits 64 rather than hanging or voting unconfirmed).
`--remove` retracts your own upvote — it is not a downvote. It uses its
own narrow credential ladder, deliberately **not** the publishing or
announce token: `--token-stdin`, else `GRIM_RATE_TOKEN`, else a
host-matched CI token, else `gh`/`glab auth token`, else a refusal. There
is no `--token <value>` flag, because argv is world-readable.

Voting against a GitHub Enterprise Server or self-managed GitLab needs
`GRIM_RATING_HOST`. It is read from your own environment only — a fetched
`stats.json` carries no host at all — and compared exactly, with no suffix
matching. `grim rate <ref> --dry-run --format json` reports the resolved
host without a credential, a forge request, or a mutation, which is how a
client learns where a vote would go before it authenticates.

Piping a credential into that same dry run (`--dry-run --token-stdin`,
which needs no `--yes` because it posts nothing) adds one read-only query
and reports `viewer_up` — whether *this* account has already voted. It is
tri-state: `true`, `false`, or `null` for **not asked, or not knowable**.
Never read `null` as "not voted"; a query that failed observed nothing,
and saying otherwise is the one claim the design refuses to make. Full
surface, every flag, and all seven exit codes: [command reference:
rate][rate] and [Artifact Ratings][ratings-doc].

## Further Reading

- [Concepts: scopes][scopes], [clients][clients], and
  [online-by-default][online] — the semantics behind each section above.
- [Configuration][envvars] — environment variables, `[[registries]]`
  schema, precedence rules, data layout under `GRIM_HOME`.
- [The Package Index][package-index] — index spec, auto-merge rules.
- [Host Your Own Index][hosting] — scaffold, deploy, and gate your own.
- [Artifact Ratings][ratings-doc] — how an index collects them, per forge.
- [Command reference: search][search], [tui][tui], [rate][rate], and
  [mcp][mcp].

[scopes]: https://grimoire.rs/concepts.html#scopes
[clients]: https://grimoire.rs/concepts.html#clients
[clients-matrix]: https://grimoire.rs/clients.html#matrix
[online]: https://grimoire.rs/concepts.html#online-by-default-offline-on-demand
[envvars]: https://grimoire.rs/configuration.html#environment-variables
[registry-compat]: https://grimoire.rs/configuration.html#registry-compatibility
[browse-filters]: https://grimoire.rs/configuration.html#browse-filters
[package-index]: https://grimoire.rs/package-index.html
[hosting]: https://grimoire.rs/hosting-an-index.html
[index-repo]: https://github.com/grimoire-rs/index
[config-cmd]: https://grimoire.rs/commands.html#config
[search]: https://grimoire.rs/commands.html#search
[rate]: https://grimoire.rs/commands.html#rate
[ratings-doc]: https://grimoire.rs/ratings.html
[tui]: https://grimoire.rs/commands.html#tui
[mcp]: https://grimoire.rs/commands.html#mcp
[mcp-spec]: https://spec.modelcontextprotocol.io/
[claude-code]: https://docs.anthropic.com/en/docs/claude-code
