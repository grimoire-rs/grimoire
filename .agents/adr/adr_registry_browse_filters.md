# ADR: Per-registry browse filters (`include` / `exclude`)

## Metadata

**Status:** Accepted
**Date:** 2026-08-09
**Deciders:** Michael Herwig (maintainer)
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md`
      (Rust 2024; the one new dependency — `globset` — is the de-facto
      standard glob engine in the Rust ecosystem and costs exactly one
      net-new crate, `bstr`)
**Domain Tags:** api, integration, tui
**Supersedes:** N/A
**Research artifact:**
[`research_registry_browse_filters.md`](../research/research_registry_browse_filters.md)

## Context

A team points `grim` at one internal registry that hosts hundreds of
repositories, of which a handful are AI-agent config. Browsing
(`grim search`, `grim tui`, the MCP `grim_search` tool) shows everything
the registry lists. Today the only way to narrow the view is to split the
packages into a second registry or namespace — an infrastructure change to
solve a display problem.

The wanted capability is a **per-registry browse filter**: two optional
glob lists on each `[[registries]]` entry that narrow what browsing shows,
without fragmenting infrastructure and without ever affecting resolution or
install.

Four properties of the current code decide the shape of the change:

1. **One filter seam already exists.** `catalog_service::load_catalog`
   (`src/catalog/catalog_service.rs:173`) is where every browse front-end
   converges; its per-registry row loop (`:233-260`) already applies
   `.filter(|e| e.matches(&parsed))` for the `SearchQuery`. `grim search`
   (`src/command/search.rs:106`), the TUI (`src/tui/app.rs:1003`), and the
   MCP `grim_search` (which delegates to `command::search::run`) all
   inherit from it. A second filter there is inherited by all three.
2. **A fourth caller shares that seam and must NOT be filtered.**
   `grim status --check` (`src/command/status.rs:383`) calls `load_catalog`
   with an empty query purely to populate `deprecated` / `replaced_by` on
   *declared* artifacts. An unconditional filter would make `status --check`
   go blind on any declared artifact the filter excludes — a silent
   correctness bug, not a display change. (These four are the complete
   caller set; the only other catalog path,
   `UpdateChecker::spawn_catalog_refresh` at `src/tui/update_check.rs:268`,
   is not called from the event loop.)
3. **Resolution never sees the browse set.** `src/resolve/resolver.rs` has
   zero references to `ResolvedRegistry` or the `catalog` module;
   `resolve_lock()` works off already-baked `Identifier`s. The single
   intersection is `resolve_reference()`
   (`src/config/registry_resolve.rs:251`), which consults the alias→url
   table and never the catalog. `install.rs` and `lock.rs`: zero hits. The
   boundary this ADR needs is a boundary that already holds.
4. **The catalog cache is shared across scopes.** `catalog_file_for`
   (`src/store/paths.rs:100`) keys on the SHA-256 of the registry url
   alone, and the same file backs both browse and `status --check`. Any
   filter that ran at *build* time would poison that cache for the caller
   that must not be filtered.

## Decision Drivers

- **One seam, four callers** — the filter must be implemented once and be
  impossible for a future browse front-end to forget, while remaining
  impossible for a correctness-critical caller to inherit by accident.
- **Principle 9 (Preserve Compatibility)** — `docs/src/stability.md` freezes
  the `grimoire.toml` schema. An unset registry must serialize
  byte-identically to today and behave identically.
- **Not a security boundary** — a filter narrows a view. An excluded
  package must stay fully installable, resolvable, and describable by name.
- **Frozen-on-first-release semantics** — glob dialect, precedence, and the
  string a pattern matches against all freeze the moment this ships.
- **Deterministic display** — the tree-root label must be a function of
  config, not of the current result set.
- **Boring technology** — no bespoke glob engine, no new exit code, no new
  error kind, no new config surface shape.

## Decision

### D1. Two optional lists on `RegistryConfig`, additive by construction

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub include: Vec<String>,
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub exclude: Vec<String>,
```

An entry that sets neither parses, validates, resolves, and re-serializes
exactly as today. `include = []` and an absent `include` are the same
state — there is no third "explicitly empty" meaning, matching the
None-when-default collapse every other list-valued config key already uses
(`command::config::fixed_value`).

`RegistryConfig` carries `#[serde(deny_unknown_fields)]`, so an **older**
grim reading a **newer** config that sets a filter fails to parse rather
than ignoring the key. That is the accepted downgrade direction already
taken for `CatalogEntry::replaced_by`; Principle 9 governs new-grim-reads-
old-config, which holds.

### D2. Precedence: include-then-subtract, exclude wins, empty include skips the check

A row is shown iff:

- the include list is empty, **or** the candidate matches at least one
  include pattern; **and**
- the candidate matches no exclude pattern.

Empty include is implemented as *skipping the include check*, never as
compiling a synthetic `**` — so an empty list cannot be broken by a glob
subtlety. This is the Artifactory model. It is deliberately **not** the
model of the two nearest-looking precedents, and the docs must say so in
words: Cargo's `include`/`exclude` are *mutually exclusive*, and
"gitignore-style" means *ordered last-match-wins with a parent-directory
trap*. grim borrows gitignore's glob **tokens** (via globset) and neither
tool's **ordering model**.

### D3. The match candidate is the row's source-relative path

For each row in a group, the candidate string is

```
repo = "{entry.registry}/{entry.repository}"
candidate = repo.strip_prefix("{source.url}/").unwrap_or(repo)
```

which yields, per source kind:

| Source `url` | Row | Candidate |
|---|---|---|
| `ghcr.io` | `ghcr.io/acme/platform/foo` | `acme/platform/foo` |
| `ghcr.io/acme` | `ghcr.io/acme/platform/foo` | `platform/foo` |
| `https://index.grimoire.rs` (index) | `ghcr.io/acme/foo` | `ghcr.io/acme/foo` |

One rule, no per-kind dispatch.

> **AMENDED 2026-08-09 (owner decision).** The paragraph below claimed this
> is "byte-for-byte the second element of `tree::display_split`". **That
> holds only when configured sources do not overlap.** `display_split`
> delegates to `attribute_registry` (`src/tui/tree.rs:601-612`), which
> attributes a row to the longest configured prefix across the *whole*
> registry set; the candidate above strips only the *declaring entry's* own
> url. With `ghcr.io` and `ghcr.io/acme` both configured,
> `ghcr.io/acme/tools/foo` displays as `tools/foo` but is matched as
> `acme/tools/foo` by the `ghcr.io` entry.
>
> The declaring entry's url wins. Deriving the candidate from the full set
> would make a pattern's meaning depend on the *other* entries, so adding an
> unrelated source would silently re-point every existing filter — an
> unacceptable non-locality for a config surface. The equality claim is
> narrowed to non-overlapping sources rather than the rule being changed.

It is not a new mental model: it is
the second element of `tree::display_split`
(`src/tui/tree.rs:592`) — the path the TUI already renders *underneath*
that source's root, and the string the flat list already shortens to. A
filter pattern therefore reads exactly like what the user sees.

An index source has no single registry root to be relative to, so its
candidate is the fully-qualified ref — which is again what its tree root
already shows, because index-sourced rows carry `source = Some(locator)`
and `display_split` returns the full ref beneath it.

> **Amended 2026-08-10 (wave-1, S-6).** The formula above assumes
> `source.url` carries no trailing slash — true today, but not true of what
> a user can author: `oci = "ghcr.io/acme/"` passes validation unchanged.
> Before wave-1, an unstripped trailing slash made the second
> `strip_prefix` above fail (there is no `//` in the candidate to match),
> so every candidate silently fell through to the fully-qualified-ref case
> — a mis-relative filter with no diagnostic, and an exclude-only entry
> failing *open* with nothing on screen to explain why. `trim_locator`
> (`src/config/registry_resolve.rs`) now trims trailing slashes off the
> locator once, at every site that constructs `ResolvedRegistry.url`
> (plus `classify_index`), so `source.url` is always the trimmed form by
> the time this formula runs — `oci = "ghcr.io/acme/"` and
> `oci = "ghcr.io/acme"` produce identical candidates. The formula's
> shape is unchanged; what changed is that its precondition on
> `source.url` now actually holds.

> **Amended 2026-08-10 (round-3, S-B).** The parenthetical above — "at
> every site that constructs `ResolvedRegistry.url` (plus
> `classify_index`)" — over-reads. `classify_index` has **eight** call
> sites in the tree, and only the **two** inside `resolve_registries`
> (`registry_resolve.rs:208`, `:309`) pass a `trim_locator`-trimmed
> argument. The other six — `app.rs:1213`, `tui.rs:238`, `init.rs:144`,
> `config.rs:648`, `config.rs:1218`, `project_config.rs:253` — classify
> the raw, untrimmed locator. This is **benign today**, because the trim
> only matters where `classify_index`'s own `.git`-suffix check sits on
> the boundary: `index = "https://host/repo.git/"` classifies `IndexHttp`
> at validation (`project_config.rs:253`, untrimmed — the trailing slash
> defeats `ends_with(".git")`) and `IndexGit` at resolution
> (`registry_resolve.rs:309`, trimmed — the same locator now ends in
> `.git`). Two different transports for one config line, at two different
> times. Recorded as the real scope of the fix — narrower than the
> sentence above states — not as a new defect; nothing here changes
> behaviour.

### D4. Auto-expansion via one pinned glob constructor

A pattern containing none of `* ? [ ] { } \` is **wildcard-free** and
compiles as `"{p}{,/**}"` — it matches the bare name and everything under
it. Every pattern, expanded or not, compiles through exactly one function:

```rust
// AMENDED 2026-08-09 — see the D4 amendment below. The constructor as
// shipped also sets `.literal_separator(true)`.
GlobBuilder::new(pattern).empty_alternates(true).build()
```

`empty_alternates` defaults to `false`, under which `acme{,/**}` compiles
without error and **silently fails to match bare `acme`** (verified against
globset 0.4.20 in the research artifact). The failure is a silent
non-match, not a parse error, and this semantics freezes on release — so
the constructor is a single pinned function with a unit test asserting the
bare-name match, never a bare `Glob::new` at any call site.

> **SUPERSEDED, 2026-08-09 — see the D4 amendment below.** The paragraph
> that followed here said `literal_separator` stays at its `false` default
> so `*` crosses `/`. That is no longer what grim does: it is set to
> **`true`**, so `*` and `?` stop at a `/` and only `**` crosses one. The
> original reasoning contradicted C-003's own worked example and made `**`
> decorative. Left visible rather than rewritten so the decision's history
> reads honestly; the amendment is authoritative.

`case_insensitive` stays at its `false` default:
OCI repository names are lowercase by spec, and case-sensitivity is the
**reversible** choice — a future case-insensitivity opt-in widens what is
accepted, which the additive-only policy permits, whereas retro-fitting
case-sensitivity onto insensitive matching would narrow it.

Exactly-one-package is expressed by combining the lists, with no
trailing-slash rule, anchor character, or third knob:

```toml
include = ["acme/platform/foo"]   # ≡ acme/platform/foo{,/**}
exclude = ["acme/platform/foo/**"]
```

### D5. `load_catalog` gains a typed scope, not a bool

```rust
/// Whether this catalog load is a user-facing browse (honours each
/// source's `include`/`exclude`) or a completeness-critical lookup
/// (never hides a row — a missing row would be a correctness bug).
pub enum CatalogScope { Browse, Complete }
```

`search` / `tui` pass `Browse`; `status --check` passes `Complete`. The
filter stays implemented once, inside `load_catalog`; what each caller
chooses is whether it *is* a browse. A new front-end cannot compile without
answering the question, and cannot answer it by accident — which a `bool`
parameter would allow, and which `quality-core.md` classes Warn-tier.

`load_catalog` goes from 7 to 8 parameters. Collapsing it into a params
struct is the right eventual fix and is deliberately **not** done here:
`quality-core.md`'s Two Hats Rule forbids shipping a refactor of a shipped
seam inside a feature diff. Recorded as a follow-up.

> **Recorded 2026-08-10 (round-3, A1) — deferred alongside the params-struct
> collapse above.** The plan's C-019 diagnostic (`filter admitted M of N
> repositories`) is a `tracing::warn!` side effect inside this shared seam,
> while every sibling verdict on `CatalogGroup` (`truncated`,
> `served_offline`, `rows_before_filter`) is returned as data. The asymmetry
> means the warning reaches only one of three browse front-ends — the TUI
> re-derives its own predicate (see the A8 note under D7, below) and MCP
> ships a static sentence — and the two predicates can already disagree
> (the TUI's three gates vs. the CLI's four, no query gate on the TUI side).
> Returning the verdict as data on `CatalogGroup` is the right shape and is
> deferred, not fixed, for the same Two Hats reason as the collapse above:
> it changes a shipped seam's return type inside a feature diff.
> Crate-internal, so Principle 9 does not bind; wants to land together with
> the collapse above, post-landing.

### D6. Filtering is read-time only — never a build-time prefilter

The filter runs on `catalog.entries()` in `load_catalog`'s existing per-row
loop. It never narrows the catalog *build*. Two independent reasons, either
sufficient: the on-disk catalog is keyed by registry url alone and is
shared with the `Complete` caller, so a filtered build would poison
`status --check` across processes; and `catalog_service.rs`'s existing doc
already records that build-time prefiltering is avoided so
summary/description/keyword-only matches survive.

Consequently `CatalogGroup::truncated` keeps reporting **build-time**
truncation verbatim, unchanged by the filter. A narrow filter cannot rescue
a browse from `MAX_CATALOG_REPOS`, because the cap is applied while listing,
before the filter is ever consulted. Reporting `truncated: false` merely
because the surviving set is small would be a lie. This is a real
limitation of filters as a substitute for a narrower registry and belongs in
the docs.

> **Landed 2026-08-09 (WP-R7).** The docs obligation in the sentence above
> went unfulfilled through the original implementation — the cap was named
> nowhere a user configuring a filter would meet it. It is now stated with
> its number (500) and its ordering (cap first, filter second) in
> `docs/src/configuration.md` › Browse filters, `docs/src/commands.md` ›
> `grim search`, and the `grim-usage` skill's registries reference, each
> carrying the actionable consequence: on a source big enough to hit the
> cap, narrow the `oci` locator — a filter cannot widen the window. The
> wording had to be originated; no comparable tool documents this, because
> they all filter server-side and have no client-side window to cap.

### D7. The TUI tree-root label is derived from the include list's literal prefix

The label is a pure function of config:

1. For each include pattern, take its leading `/`-separated segments up to
   the first segment containing any of `* ? [ ] { } \`.
2. Take the segment-wise common prefix across all patterns.
3. Empty result → the label is unchanged from today.
4. Non-empty result `P` → the source's label gains `/{P}`:
   `"{alias} ({url}/{P})"`, or `"{url}/{P}"` when no alias is configured.

This injects at `src/tui/app.rs:1051-1061`, where `registry_labels` is
already built as a free-form display string keyed by url. `render.rs:472`
already looks the label up by group key, and `label` is already distinct
from `key` — so **no `tree.rs` structural change**, no key change, no
change to `collapsed`, and no change to path compression below the root.

Exclude patterns never contribute: excludes subtract, they do not describe
what the root *is*.

### D8. Names are never rewritten

The filter narrows; the label prefix is display-only. Full
`registry/repository` refs stay verbatim in every JSON payload, every
`grim search` row, every lock entry, and every install record. The
per-row prefix strip (`attribute_registry`, `src/tui/tree.rs:601`) keeps
stripping the **configured source url** and is not re-pointed at the
derived prefix — doing so would move tree group keys, which
[`adr_projection_over_index.md`](./adr_projection_over_index.md) makes the
tree's data contract.

### D9. `--registry` collapses the browse set and applies no filter

`resolve_registries`' forced branch already constructs entries with
`alias: None`, dropping config-derived metadata; filters follow the same
rule. `--registry` names a source explicitly, possibly one not declared in
`[[registries]]` at all, so it is an override of the browse set rather than
a selection within it.

### D10. No filter parameter is exposed to MCP

`grim_search` inherits the filter through `command::search::run` and gains
no argument. This mirrors the registry-allowlist stance already recorded in
[`adr_multi_registry_mcp.md`](./adr_multi_registry_mcp.md) §5: the
configured set is the boundary, and an agent-supplied override of what grim
browses is exactly the surface that stance closed.

### D11. Malformed patterns fail at config validation, reusing shipped error classes

Validation (empty pattern, control characters, glob compile failure) lands
in `validate_registries` (`src/config/project_config.rs:220`) under the
existing `ConfigErrorKind::RegistryInvalid` — no new error kind, no new
`ExitCode` variant. The exit-code split is the one the `index` locator
already establishes: **78** (`ConfigError`) at load, **65** (`DataError`)
at `grim config set` / `grim config registry add`. Messages render the
offending pattern via `escape_debug`, as every other message quoting
authored TOML in that function does.

A compile failure must not panic: it logs a warning and degrades to the
unfiltered view. Fail-open, not fail-closed — the filter is a view narrowing,
so degrading to "shows more" is recoverable and degrading to "shows nothing"
would look like a broken registry.

> **Amended 2026-08-09 (WP-R7), from shipped code.** Two details of the
> paragraph above were written from the design and never matched what
> landed. The **code is right and this record was stale** — nothing here
> asks for a code change.
>
> - **Location: `resolve_registries`, not `load_catalog`.** The filter is
>   compiled once at *resolve* time (`src/config/registry_resolve.rs`), so
>   `globset` stays inside `src/config/` and every consumer — `search`, the
>   TUI, MCP, `grim context` — receives an already-compiled filter rather
>   than each re-deriving one. `load_catalog` never compiles anything.
> - **Both lists are dropped, not "that list".** The fail-open arm
>   substitutes `RegistryFilter::default()`, so the entry loses its
>   `include` **and** its `exclude` and is browsed unfiltered. That is the
>   most fail-open outcome available and therefore the one this decision's
>   own stance implies; keeping a half-filter would be an intermediate state
>   nobody chose. It is also visible: `grim context` reports the entry with
>   no patterns.
>
> Reachability likewise moved. Validation (C-006) now runs each pattern
> through `compile_set` — the browse filter's own constructor — so a
> *single* bad pattern really is gated at 78/65. What survives is a pattern
> **list** that only fails once its members are compiled into one glob set,
> which per-pattern validation cannot see:
>
> ```text
> registry 'acme': invalid include/exclude pattern (does not compile as part of a glob set: error building NFA); browsing without a filter
> ```
>
> Reproduced against the shipped binary with ~6 000 individually-valid
> patterns in one `include` list; exit stayed `0`.

### D12. `include`/`exclude` are addressable config keys, reusing `StringList`

Two new `RegistryField` arms with
`ValueType::StringList { default: None }` — the same type
`options.tui.tree_separators` already ships, with the same comma-split-on-set
and `join(",")`-on-get. `constraints: None`: `ValueConstraints`
(`src/command/config_keys.rs:44`) requires **both** `item_pattern` and
`item_width`, and a glob has no width rule. Making `item_width` optional is
refused — `ValueConstraints` is published in `grim config list --format
json`, and changing a field's type from integer to integer-or-null is
exactly what the additive-field policy forbids. Glob validity is proven by
attempting compilation, which is a stronger check than a regex could be.

Consequence, documented not fixed: a comma is glob alternation syntax, so a
pattern containing `{a,b}` cannot be written through `grim config set` and
must be authored directly in `grimoire.toml`. Auto-expansion (D4) means the
one form a user would otherwise need — `acme{,/**}` — is never typed.
`grim config registry add --include`/`--exclude` are therefore **repeatable
only, never comma-split**, deliberately diverging from `--registry`'s
repeatable-or-comma-separated house style. Adding an escape spelling later
widens what is accepted and stays additive.

### D13. The hand-rolled TOML writer must learn the fields, with a round-trip tripwire

`command::add::write_config` (`src/command/add.rs:881`) **hand-writes**
`[[registries]]` with `writeln!` — it does not go through `Serialize`. It
is the single write path for `grim add`, `grim config set/unset`, and every
`grim config registry` verb. Adding the fields to `RegistryConfig` without
teaching that emitter would make any of those commands **silently delete a
hand-authored filter**.

The emitter gains both fields, and a round-trip test
(fully-populated `RegistryConfig` → `write_config` → re-parse → assert
equality) becomes the tripwire, so the next field added to
`RegistryConfig` cannot reintroduce the same data loss. The existing
`registry_field_completeness_matches_registry_config` drift test does not
cover the emitter — it compares `RegistryField::ALL` against serde output.

## Amendments (owner decisions, 2026-08-09, after plan review)

### D4 amended — `literal_separator` is `true`, not `false`

D4 pinned the constructor as
`GlobBuilder::new(p).empty_alternates(true).build()`, leaving
`literal_separator` at globset's `false` default so `*` crosses `/`.
**That was wrong and self-contradictory**, caught during WP-A's Specify
phase: C-003's own worked example ("`acme/*` → matches `acme/foo`, not
`acme/foo/bar`") describes `literal_separator = true`, and the
auto-expansion rule's use of `**` only makes sense if `*` is not already
recursive.

Verified empirically against globset 0.4.20:

| `literal_separator` | `acme/*` ~ `acme/foo` | `acme/*` ~ `acme/foo/bar` |
|---|---|---|
| `false` | match | **match** — `*` ≡ `**` |
| `true` | match | no match |

The pinned constructor is now
`GlobBuilder::new(&expand(p)).empty_alternates(true).literal_separator(true).build()`.
`acme{,/**}` behaves identically under both, so the auto-expansion rule is
unaffected.

**Rationale for `true`:** it matches gitignore, rsync and ripgrep, so a
user's existing intuition transfers; it keeps `**` meaningful rather than
decorative; and when a user guesses wrong it fails **narrow**, which is the
right direction for a feature whose whole purpose is narrowing. Under
`false` a pattern written as `acme/*` silently admits the entire subtree.

This is a dialect decision and the dialect freezes on release
(Consequences, below) — which is why it went to the owner rather than being
resolved as a typo.

### D4 amended a second time — `backslash_escape` is pinned `true` too (wave-1, W-8)

The paragraph above amended one non-default `GlobBuilder` setting and
missed a second: **`backslash_escape`** is also pinned `true`, and the
shipped constructor
(`GlobBuilder::new(&expand(p)).empty_alternates(true).literal_separator(true).backslash_escape(true).build()`,
`src/config/registry_filter.rs`) already reflects it — this record did not.

globset defaults `backslash_escape` to `!is_separator('\\')`, which is
**platform-conditional**: `true` (escape) everywhere `\` is not a path
separator, `false` (literal path separator) on Windows. Left at that
default, one committed `grimoire.toml` would mean two different things
across a team's checkouts — `\*` a literal `*` on Linux/macOS, a path
separator followed by a wildcard on a teammate's Windows machine. Pinning
it `true` unconditionally is the same choice the `ignore` crate (BurntSushi's
gitignore-semantics reference implementation, and this ADR's own cited
precedent) makes, for the same reason.

It is also a dialect decision, not a defaults sweep: `\` is one of the
seven [`GLOB_METACHARACTERS`] (module doc, `registry_filter.rs`), so a
pattern containing a backslash is classified as already-authored glob
syntax and passed through [`expand_pattern`] verbatim rather than
auto-expanded — leaving the platform default in place would have compiled
that pattern to a literal path separator on Windows and an escape
everywhere else, contradicting the module's own dialect. Recorded here,
not just in the module doc, because it is a third one-way door alongside
`empty_alternates` and `literal_separator` (Consequences, below) and this
ADR is where a reader checks which knobs are frozen.

**D7 is withdrawn.** The tree-root label is *not* derived from the include
list's literal prefix; it stays exactly as it is today
(`"{alias} ({url})"`). `alias` already lets a user name the root, so the
derivation was a second labelling mechanism competing with the first, it
coupled display to filter semantics (a source-url edit would silently move
the label as well as break the patterns — see D3 and the plan's C-019), and
it could never shorten the *rows*, which was the original request; D8's
refusal to re-point the per-row strip stands. Prefix display is deferred to
the VS Code tree-view design, where the visualization question is actually
being decided, so that any mechanism covers CLI, TUI, and extension
together instead of being retrofitted from one side. **This removes a frozen
surface; nothing replaces it.**

> **Recorded 2026-08-10 (round-3, A8).** "Nothing replaces it" is true of
> the *label*; it is not true of every disclosure this decision touches, and
> a reader of this withdrawal would otherwise assume it is. The TUI has its
> own channel for the plan's C-019 diagnostic — `aggregate_registry_health`
> / `c019_filter_emptied` (`src/tui/app.rs`, landed WP-R5/WP-R10) — reusing
> this contract's three gates (non-empty include, zero post-filter rows,
> non-zero pre-filter rows) to render `RegistryHealth.filtered` beside the
> registry-health line. It exists precisely because tracing output cannot
> reach the alt-screen (`SwitchableWriter` redirects it to `tui.log` for the
> session), so C-019's CLI warning needed a TUI-native answer. Full design
> and its one-sided limitation (only the "admitted 0 of N" shape has a
> channel; a non-empty exclude removing nothing does not) recorded at the
> plan's C-019, not duplicated here.

**D12 is amended: no comma splitting anywhere.** The original text kept
`StringList`'s house comma-split on `grim config set` while making
`--include` / `--exclude` repeatable-only, which left `registry add` able to
express `acme/{platform,tools}/**` and `config set` unable to — one field,
two capabilities. The resolution is to drop splitting entirely: `config set`
takes exactly **one** pattern and replaces the list with it, and a
multi-pattern list is written with repeated `registry add --include` flags
or by editing `grimoire.toml`. `get` remains display-only and is not
round-trippable for a multi-element list (`--format json` is the
authoritative shape). The `KeySpec` still declares `ValueType::StringList` —
the schema and JSON genuinely are a list of strings; only the CLI write path
diverges from house behaviour.

> **Recorded 2026-08-10 (round-3, W-A).** Replacing the whole list (above)
> has a sharp edge this decision did not record: `config set` on an alias
> that already carries 2+ patterns silently discards the rest, and without
> a diagnostic that reads as a routine update, not data loss. `src/command/
> config.rs`'s `warn_on_discarded_patterns` closes the silence — when the
> entry being replaced already had 2+ patterns, `set` emits a
> `tracing::warn!` naming the field, the discarded count, and the remedy
> (`registry rm` + re-`add` with repeated flags, or a hand edit). It shares
> a `WriteSite` enum with two siblings introduced the same round:
> `check_filter_pattern`'s bare-comma warning and `check_set_filter_pattern`'s
> empty-value-to-unset remedy — three distinct authoring mistakes, three
> distinct warnings, one enum that only ever changes which remedy is named.

## Rationale

**Why the seam and not the callers.** Three browse front-ends and one
completeness-critical caller share `load_catalog`. Filtering in the browse
callers would duplicate the predicate in two places today and silently omit
it from the fourth front-end someone adds later. Filtering in the seam with
a typed scope keeps one implementation and turns "is this a browse?" into a
compile-time question.

**Why `Complete` is a named scope rather than an absence.** `status --check`
does not merely *not want* the filter — hiding a declared artifact's
deprecation notice from it is a correctness bug. Naming the scope puts that
reasoning in the type, where the next reader finds it.

**Why source-relative and not fully-qualified.** The filter is declared
inside the `[[registries]]` entry that already names the source, so "within
this source" is the reading the config's own structure suggests. It is also
the string the TUI already shows beneath that source's root, so patterns and
display agree without a translation step. Fully-qualified patterns would
force `ghcr.io/acme/platform/**` into an entry whose next line already reads
`oci = "ghcr.io/acme"`.

**Why globset.** The empirical probe in the research artifact settles the
one load-bearing question (brace-alternation empty branches), the dependency
cost is one net-new crate (`bstr`), and every alternative either lacks brace
alternation (`glob`, itself the subject of a deprecation discussion), is two
orders of magnitude less exercised (`wax`), or is the wrong layer
(`ignore`, which is built *on* globset and adds directory walking this
feature never performs).

**Why fail-open on a compile error.** A browse filter is a UX narrowing, not
an allowlist. The recoverable direction is showing rows the user wanted
hidden; the unrecoverable one is a registry that looks empty for a reason
nothing on screen explains.

## Alternatives Considered

- **Filter in the browse callers only, leave `load_catalog` untouched.**
  Zero signature change and zero risk to `status --check`. Rejected: it
  duplicates the predicate across `search` and the TUI and discards the
  one-seam property that makes the design work at all; a fourth browse
  front-end would inherit nothing.
- **A `bool` `apply_filters` parameter.** Rejected: `quality-core.md` classes
  a two-state boolean parameter as Warn-tier where an enum is clearer, and
  here the two states are "display preference" and "correctness
  requirement" — the exact case a name is worth having.
- **Collapse `load_catalog` into a params struct as part of this change.**
  The right end state, rejected *for this diff*: mixing a refactor of a
  shipped seam into a feature violates the Two Hats Rule and would put the
  feature's review and the refactor's review in one diff. Deferred.
- **Apply the filter as a build-time prefilter (narrowing the `_catalog`
  walk).** Would let more matching repositories fit under
  `MAX_CATALOG_REPOS`. Rejected on two independent grounds: the on-disk
  catalog is keyed by registry url alone and shared with the `Complete`
  caller, so a filtered build poisons `status --check` across processes;
  and it re-opens the summary/keyword-match loss `catalog_service.rs`
  already documents avoiding.
- **Match against the fully-qualified `registry/repository`.** Uniform
  across source kinds and unambiguous. Rejected: it forces the source's own
  url into every pattern declared inside that source's entry, and diverges
  from what the TUI shows beneath the root.
- **Match against `CatalogRow::repository` alone.** Rejected: for an index
  source `repository` is host-stripped, so `acme/foo` on `ghcr.io` and on
  `docker.io` collide into one pattern space — and the built-in fallback
  browse source *is* an index.
- **A single pattern list plus a mode toggle (the Harbor shape), or
  ordered first-match-wins rules (the rsync shape).** Rejected: the first
  cannot express include-then-subtract at all; the second makes rule order
  load-bearing, which is precisely the property `.gitignore`'s
  parent-directory trap makes hard to reason about.
- **Widen `ValueConstraints` so `item_width` becomes optional.** Rejected
  under Principle 9: `ValueConstraints` is published in `grim config list
  --format json`, and the additive policy permits new optional fields but
  never a change to an existing field's type.
- **A third knob (trailing slash, anchor character, or an
  `exact`/`recursive` mode) for exactly-one-package.** Rejected: the two
  lists already express it, and every knob is a permanently frozen surface.

## Consequences

**Positive**

- One filter implementation, inherited by `search`, the TUI, and the MCP
  `grim_search`; `status --check` provably unaffected.
- An unset registry is byte-identical on disk and in behaviour; the whole
  feature is invisible to every existing config.
- The tree-root label becomes a function of config rather than of results,
  so it stops moving as the result set changes.
- Infrastructure stays whole: narrowing a view no longer argues for
  splitting a registry.
- Reuses shipped machinery end to end — `StringList`, the
  load-78/set-65 error split, `ConfigErrorKind::RegistryInvalid`,
  `registry_labels`, and the existing per-row filter loop.

**Negative / risks**

- **Data-loss hazard if D13 is missed.** The `[[registries]]` writer is
  hand-rolled; a filter added to the struct but not to the emitter is
  silently deleted by the next `grim add` or `grim config set`. Mitigated
  by the round-trip tripwire, which is the actual deliverable — the
  emitter arm alone is not.
- **Semantics freeze on release.** Precedence, the match candidate, the
  auto-expansion rule, case-sensitivity, `literal_separator`, and
  `backslash_escape` are all one-way doors. Each is either the
  ecosystem-precedented choice (Artifactory precedence; globset's dialect
  with `empty_alternates`, `literal_separator`, and `backslash_escape` all
  three pinned non-default per the D4 amendments) or the
  reversible-by-widening one (case-sensitivity).
- **A comma cannot be written through `grim config set`,** so brace
  alternation must be authored in the file. Documented; auto-expansion
  removes the case a user would actually hit.
- **An older grim rejects a config that sets a filter** rather than
  ignoring it (`deny_unknown_fields`). Accepted downgrade direction, with
  precedent.
- **"Browse filter ≠ access control" has no ecosystem prior art to copy.**
  No registry tool documents the distinction explicitly, and Verdaccio's
  superficially similar glob rules genuinely *are* its access-control
  mechanism. grim must state it plainly in the config reference rather
  than gesture at convention.
- **One new dependency tree**, priced at a single net-new crate (`bstr`)
  plus a `regex-automata` patch bump in the lockfile.
- **An invalid filter is a config error like any other — it can fail
  commands the key descriptions call "browse only."** Recorded 2026-08-10
  (round-3, A2). `validate_registries` runs on every config load, for every
  command, so a pattern that fails to *compile* (as opposed to one that
  merely matches nothing) exits 78/65 the same as any other malformed
  config value — `grim add`, `grim status`, `grim login` included, not only
  `search`/`tui`/`grim_search`. The `include`/`exclude` `KeySpec`
  descriptions promise the fields affect browsing only, which is true of a
  filter that *compiles*; it is not true of one that does not, because a
  config file is validated as a whole before any command runs. The wording
  fix belongs to the `KeySpec` descriptions themselves, which is an owner
  call deferred alongside S-5's structural trim — not changed here.

## Matching-engine risk notes

Recorded so the omissions read as decided rather than accidental. Sourced
from the plan-review gap check; each is a stated position, not new design.

- **Amended 2026-08-10 (wave-1, W-3), from shipped code.** The paragraph
  below originally read "No cap on pattern count or total pattern length …
  No cap." That was already half-wrong when written — two per-pattern caps
  (`MAX_PATTERN_BYTES` = 1024 bytes, `MAX_BRACE_DEPTH` = 32 levels,
  `src/config/registry_filter.rs`) predate this feature and bound one
  pattern — and wave-1 closed the other half: `MAX_PATTERN_LIST_BYTES`
  (64 KiB, summed per `include`/`exclude` list) is enforced in `compile_set`,
  the one function both the browse filter and config-load validation route
  through, so the failure a huge pattern *list* — not any single
  out-of-bounds pattern — used to reach (7 000 wildcard-dense patterns, each
  individually inside both per-pattern caps, drove `grim context` to 2.8 GB
  peak RSS) is now capped at the source. What is **not** capped is the
  cross-entry aggregate — many `[[registries]]` entries, each at its own
  64 KiB list budget — which is bounded only by
  `config::FILE_SIZE_LIMIT_BYTES` (8 MiB): measured worst case, 126 entries
  each at budget peaks at 933 MB, a 4.1× cut from the pre-cap number and
  back under the 2 GB line most CI runners default to, and linear in total
  pattern bytes rather than the single-giant-glob-set blowup the per-list
  cap already removed. Tightening that residual further is the file-size
  limit's job, not this feature's.
- **Recorded 2026-08-10 (round-3, A7/P-3).** "Linear in total pattern
  bytes" (above) describes memory scaling under the aggregate byte budget —
  it is not a claim that compiling a pattern list is cost-bounded in
  general, and round-3 measurement shows it is not: at an *identical* byte
  budget, a wildcard-dense list costs ~6× the wall-time and ~2.3× the RSS
  of a literal-heavy one (0.16–0.20 s / 62 MB vs. 0.03 s / 27 MB). The byte
  and depth caps bound RSS; they do not bound CPU, and neither bounds
  wildcard/pattern *count*. `validate_registries` runs on every config load
  for every command, so this is a legal, unwarned cost on
  `git clone && grim <anything>` against a hostile `grimoire.toml`. No
  count-cap precedent exists in any comparable tool (per the research
  artifact), and linear-time matching after compilation does not make the
  compilation step itself bounded — the two are independent properties.
  Folds into the same owner-deferred bucket as this section's 933 MB
  residual above; recorded so the deferral is informed, not fixed here.
- **Recorded 2026-08-10 (round-3, adversary P4) — flagged for verification
  in the WP-3 report; do not treat as confirmed without checking the code
  it describes.** `MAX_PATTERN_LIST_BYTES` budgets each pattern's
  **expanded** length (`expand_pattern(pattern).len()`), not its authored
  length — a wildcard-free pattern auto-expands to `"{p}{,/**}"` (D4/C-003),
  a fixed +6-byte, up-to-7× inflation on a short pattern, and charging the
  authored length let the budget be built past: ~65 500 one-byte patterns
  cleared 64 KiB authored and then built the oversized combined program
  anyway (`Regex("error building NFA")`, 277 MB peak RSS). The budget is
  enforced against what `compile_set` actually builds, closing a DoS
  control that previously fired after the allocation it exists to prevent.
- Artifactory ships a comparable cap (replication saves blocked past ~1848
  characters of combined include/exclude text), but its constraint is a
  database column width hit at runtime. grim compiles each `GlobSet` **once
  per registry at config-load**, never per row per request, so the cost
  shape does not transfer directly — the caps above are sized from grim's
  own measured blowup, not ported from Artifactory's number.
- **Future `regex` / `regex-automata` bumps are a risk surface this feature
  newly creates.** globset compiles through regex-automata, which has had a
  memory cliff before — 56 globset-compiled patterns went from 12.5 MB to
  601.6 MB retained across regex 1.8.4 → 1.9.0
  ([rust-lang/regex#1059](https://github.com/rust-lang/regex/issues/1059),
  fixed in #1062). A memory-DoS shape, distinct from backtracking. Worth a
  glance on any future bump of that pair.
- **The live glob-ReDoS CVE class does not reach globset.**
  [CVE-2026-27904](https://github.com/advisories/GHSA-23c5-xmqv-rm74)
  (minimatch, 2026-02) depends on nested extglob operators `*()` / `+()`
  producing nested unbounded quantifiers. globset's dialect has no extglob
  operator at all — only `* ? [ ] { }` — so the mechanism is structurally
  absent, not merely unexercised. Stated affirmatively because
  `include`/`exclude` are authored by anyone who can edit `grimoire.toml`
  and a reviewer will ask.
- **Unicode NFC/NFD mismatch cannot arise.** An NFC-authored pattern silently
  failing to match an NFD-stored name is a real, recent failure
  ([setuptools, 2026-06](https://github.com/pypa/setuptools/commit/dd9f436a36486b4cb8a4c70a2321548b0be09b8f)),
  but it needs arbitrary filenames. The D3 candidate is built from OCI
  registry/repository names, which the distribution spec's grammar restricts
  to ASCII `[a-z0-9._-]`. Closed by the input domain.

**On D6's framing.** Read-time-only filtering is not merely the safer of two
options — it is the only shape compatible with the product thesis. Every
adjacent tool with a real narrowed-view need either implements it server-side
as an access/routing layer (Nexus, Harbor, Artifactory virtual repos) or has
no filtering concept at all (Helm, ORAS, Nix, the OCI distribution spec
itself). A server-side curated view is foreclosed by "there is no Grimoire
service to sign up for", not deferred by it.

## References

- [`research_registry_browse_filters.md`](../research/research_registry_browse_filters.md)
  — globset semantics probed against 0.4.20, dependency accounting,
  precedence models across Artifactory / rsync / Cargo / gitignore /
  Verdaccio / Harbor
- [`adr_multi_registry_mcp.md`](./adr_multi_registry_mcp.md) — origin of the
  shared `load_catalog` seam, `[[registries]]`, and the MCP registry-
  allowlist stance this decision extends
- [`adr_projection_over_index.md`](./adr_projection_over_index.md) — the
  TUI tree's index/key contract, which D7 and D8 stay inside
- [`adr_grim_config_command.md`](./adr_grim_config_command.md) — the
  `grim config` surface contract and its exit-code envelopes
- `docs/src/stability.md` — frozen `grimoire.toml` schema, additive-field
  policy, and the explicit exclusion of TUI appearance from the guarantee
- `.claude/rules/subsystem-config-keys.md` — description authoring rules and
  the `config_key_metadata_matches_published_schema` tripwire
- `.claude/rules/quality-rust-exit_codes.md` — the exit-code taxonomy D11
  reuses without extending

<!-- external -->
[globset]: https://docs.rs/globset/latest/globset/
