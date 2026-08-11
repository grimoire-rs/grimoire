# Design record — dual-candidate matching, clear flags, registry identity

**Companion to** `.agents/adr/adr_registry_filter_match_candidate.md`
(decision + rationale). This file carries the **contracts** and **scenarios**
only — every item below is written so a tester can produce a failing test from
this text without opening the source.

Three clusters:

- **A. Match candidate** (C-001 … C-012 **and C-030**, S-001 … S-011) — the
  ADR's subject.
- **B. `--clear-include` / `--clear-exclude`** (C-013 … C-021, S-012 … S-018)
  — owner decision 9.
- **C. Registry identity** (C-022 … C-029, S-019 … S-022b) — the typed
  `RowSource` key and the root-key collision.

**IDs are append-only.** C-030 belongs to cluster A but is numbered last so
every existing citation stays valid; S-022b is likewise a suffix rather than a
renumber.

Notation for a catalog row: `R` = `CatalogEntry.registry`, `P` =
`CatalogEntry.repository`, `bare` = `P`, `fq` = `"{R}/{P}"`.

---

## A. Match candidate

### C-001 — `qualified_candidate(registry, repository) -> String`

A new `pub(crate)` function in `src/config/registry_filter.rs`.

Exactly two clauses, in this order:

1. **`registry` non-empty** → returns `format!("{registry}/{repository}")`.
2. **`registry` empty** → returns `repository` unchanged. No candidate may
   ever begin with `/`; a leading-slash candidate would match no authored
   pattern and fail silently.

It replaces `browse_candidate`, which is removed rather than kept as a wrapper
— one seam, one rule statement.

**On `CatalogEntry::repo()`.** `repo()` (`src/catalog/registry_catalog.rs:200-203`)
is an unconditional `format!("{}/{}", self.registry, self.repository)`, so it
returns `"/acme/tools"` for an empty registry. `qualified_candidate` and
`repo()` therefore agree **byte-for-byte on every entry with a non-empty
registry — which is every entry a catalog build produces — and only there.**
Do not "fix" the divergence by giving `repo()` the carve-out: `repo()` feeds
`grim search` JSON and the index catalog key (`registry_catalog.rs:641`), both
frozen. Do not delete the carve-out either; clause 2 is the point.

**Tests:**
- `("ghcr.io", "acme/tools")` → `"ghcr.io/acme/tools"`;
- `("", "acme/tools")` → `"acme/tools"` (and asserts the result does not start
  with `/`);
- for a `CatalogEntry` with a **non-empty** registry,
  `qualified_candidate(&e.registry, &e.repository) == e.repo()`.

### C-002 — `RegistryFilter::matches(&self, registry: &str, repository: &str) -> bool`

The frozen formula, exactly:

```
bare        = repository
fq          = qualified_candidate(registry, repository)
include_hit = include_is_empty || include ~ bare || include ~ fq
exclude_hit = exclude ~ bare || exclude ~ fq
visible     = include_hit && !exclude_hit
```

Exclude-wins is applied **once**, to the combined per-list verdicts. It is
**not** two whole-filter verdicts OR-ed together.

Implementation note (not a contract): build one `globset::Candidate` per
string and call `is_match_candidate` four times — two evaluations of each of
the two already-compiled sets. No new `GlobSet` is built and no merged set is
introduced.

### C-003 — The discriminating test: exclude beats include across candidates

This test does not exist anywhere in the tree today and is the one that
separates the chosen semantics from the naive OR.

```
include = ["acme/tools"]
exclude = ["quay.io/acme/tools"]
row:      registry = "quay.io", repository = "acme/tools"
=> matches(...) == false          // HIDDEN
```

and the sibling that proves the exclude is host-scoped rather than global:

```
same filter
row:      registry = "ghcr.io", repository = "acme/tools"
=> matches(...) == true           // VISIBLE
```

Verified against `globset` 0.4.20 with grim's pinned constructor:
`include ~ bare = true`, `include ~ fq = false`, `exclude ~ bare = false`,
`exclude ~ fq = true`. A naive `matches(bare) || matches(fq)` returns `true`
for the first row.

### C-004 — Argument order is pinned

`matches(registry, repository)` takes two `&str` in that order — the same
order as `CatalogEntry`'s fields and as `repo()`'s format. A swapped call
compiles.

**Test:** a filter whose only pattern is `"ghcr.io/**"` must match
`matches("ghcr.io", "acme/tools")` and must **not** match
`matches("acme/tools", "ghcr.io")`.

> **Corrected 2026-08-11 (WP-A Specify, measured).** The sentence that
> followed — "this fails on a transposed call at the production site" — was
> **wrong**, and so was the post-stub review's framing that C-004 is the sole
> guard. There are **two distinct transpositions and neither test catches
> both**:
>
> | Mutation | Killed by |
> |---|---|
> | Arguments swapped **inside** `matches` (`qualified_candidate(repository, registry)`) | C-004's filter-level test — 11 tests fail |
> | The **production call site** transposed (`matches(&e.repository, &e.registry)`) | the three host-qualified **C-009 browse tests** — 3 fail |
>
> C-004 calls `matches` itself, so it structurally cannot observe how
> production calls it; it is an API-order contract. What discriminates a
> transposed *call site* is a **host-qualified pattern over a two-host
> fixture** — the claim that "no browse-level test can discriminate argument
> order" holds only for *wildcard-free* patterns, whose `{,/**}` expansion
> matches the transposed candidate anyway.
>
> The guard is two-part and neither part is redundant. **Before this pass
> both mutations survived the entire suite.** Do not let a later
> simplification collapse either half.

### C-005 — Precedence table, all four combinations

| `include` | `exclude` | Verdict |
|---|---|---|
| empty | empty | visible |
| non-empty | empty | visible iff some include pattern hits `bare` **or** `fq` |
| empty | non-empty | visible iff no exclude pattern hits `bare` **or** `fq` |
| non-empty | non-empty | visible iff (some include hits either) **and** (no exclude hits either) |

Each row needs a test. The `empty include` case must still be implemented as
*skipping the include check* (`include_is_empty`), never as a synthetic `**`
(ADR D2, unchanged).

### C-006 — Order independence survives

`registry_filter_matches_is_order_independent`
(`registry_filter.rs:941-971`) must still hold, and must now loop over
`(registry, repository)` pairs rather than single strings — at least one pair
per host so both candidates are exercised.

### C-007 — The rewritten host-invariant test

`browse_candidate_never_carries_the_registry_host`
(`registry_filter.rs:1018-1029`) asserts `!candidate.contains('.')` — the
property this change deletes for the qualified candidate. It must be
**rewritten, not recompiled**, into two assertions: the bare candidate still
carries no registry host, and the qualified candidate does.

### C-008 — No per-kind branch

There is exactly one candidate rule, and `matches` has no access to
`ResolvedRegistry.kind` and must not gain any.

Because `matches` cannot see `kind`, the assertion cannot live at the matcher
— it is a **browse-level** test through `load_catalog` with
`CatalogScope::Browse`. Seed the same fixture rows twice, once behind a source
whose `kind` is `SourceKind::Registry` and once behind one whose `kind` is
`SourceKind::IndexHttp` (`src/config/registry_resolve.rs:28-38` — the three
variants are `Registry`, `IndexHttp`, `IndexGit`; **there is no
`SourceKind::Oci`**), apply the identical filter, and assert the two browses
admit the identical row set.

### C-009 — The two-host fixture (land first)

`seed_catalog` (`src/catalog/catalog_service.rs:639-665`) keys its JSON
`entries` map on `(*repository).to_string()` at `:646` — the **bare**
repository. Two tuples sharing a `repository` but differing in `registry`
collide and the second silently overwrites the first, so **no fixture in the
tree can express a two-host index today**.

Required: a fixture whose `entries` map is keyed by `{registry}/{repository}`
(mirroring the index build at `registry_catalog.rs:641`), seeded with
`[("ghcr.io", "acme/tools"), ("quay.io", "acme/tools")]` under one locator.

**Tests over it:**
- bare pattern `include = ["acme/tools"]` → both rows visible;
- host-qualified `include = ["ghcr.io/acme/tools"]` → exactly the `ghcr.io`
  row;
- host-qualified `exclude = ["quay.io/**"]` with no include → exactly the
  `ghcr.io` row.

Keying `seed_catalog` unconditionally by the qualified form is safe —
`Catalog::entries()` (`registry_catalog.rs:318-320`) is `self.entries.values()`
and nothing downstream reads the key.

**It also cannot reorder an existing single-host fixture:** prepending the
*same* `{registry}/` prefix to every key preserves lexicographic order exactly.
No re-check pass over the existing fixtures is needed. The real ordering
consequence is the opposite one and belongs to the new fixture: two-host rows
interleave (`ghcr.io/acme/tools` before `quay.io/acme/tools`) where a
bare-keyed map held one entry, so the new tests must assert on sets or on the
qualified order, never on "the first row".

### C-010 — Compile-time budgets are unchanged in meaning

`MAX_PATTERN_BYTES`, `MAX_BRACE_DEPTH` and `MAX_PATTERN_LIST_BYTES` (64 KiB)
all bound the **compiled program size of one authored list** at config-load
time. Matching against two candidate strings instead of one is a query-time
cost. **No budget changes value or meaning.** A test asserting the 64 KiB
budget still rejects at the same threshold is a cheap regression guard.

### C-011 — The C-019 diagnostic is re-derived, not reworded

`BROWSE_FILTER_REMEDY` (`catalog_service.rs:412`) and its doc block
(`:405-482`) state the superseded rule. The recovery sketch at `:452-457`
("probe whether the exclude patterns match the fully-qualified form when they
matched nothing source-relative … needs an exclude-only matcher, which does
not exist today") has **lost its premise**: the fully-qualified form is now
matched unconditionally by ordinary matching, so it is no longer a
discriminator and the sketch must be deleted rather than updated.

The `zero_match_warning` **predicate** is unchanged (fires only on an
unqueried browse whose non-empty `include` admitted nothing from a group that
had rows). Only the remedy string changes.

**The current string** (`catalog_service.rs:412`):

```
; patterns match the repository path with no registry host, and anchor at its first segment — see https://grimoire.rs/configuration.html#browse-filters
```

**The replacement**, which states both candidates and keeps the shape, the
em-dash and the anchor URL:

```
; patterns match either the repository path or the fully-qualified reference, and anchor at the candidate's first segment — see https://grimoire.rs/configuration.html#browse-filters
```

**Six carriers, and two of them are already stale on the branch** — grep
`anchor at its first segment` and `patterns are relative to this entry's own
locator` before editing:

| Carrier | State today |
|---|---|
| `src/catalog/catalog_service.rs:412` (the producer) | current wording |
| `src/catalog/catalog_service.rs:1016` (`const ANCHOR`, test-local copy) | current wording |
| `src/catalog/catalog_service.rs:1107` (a hardcoded full expected line) | current wording |
| `docs/src/configuration.md:491` | current wording |
| `docs/src/commands.md:743` | current wording |
| `catalog/skills/grim-usage/references/registries.md:271` | **pre-`f790273`** — "patterns are relative to this entry's own locator" |
| `.claude/rules/subsystem-cli-commands.md:27` | **pre-`f790273`** — same stale sentence |

The last two are a *correction*, not a re-derivation; `task catalog:verify` did
not catch the catalog copy because it is a drift-review duty, not a
string-equality test.

**The only mechanical gate available** is a verbatim-parity assertion between
the producer constant and the two `docs/src` copies. The two agent-facing
copies and the two test-local copies are caught by the suite going red, not by
a parity test. Deleting the `:452-457` recovery sketch has no test-observable
consequence at all — it is a review obligation, not an assertion.

Per owner decision 8, the exclude-only fail-open keeps its behaviour: an
exclude-only filter has no zero-match diagnostic in either direction. One line
under `#browse-filters` states it.

### C-012 — The documentation obligation (the Renovate lesson, made binding)

Every surface that describes the rule must **name both candidate strings in
its first paragraph**, with a worked example of each:

> A pattern is tested against two strings: the repository path
> (`acme/tools`) and the fully-qualified reference
> (`ghcr.io/acme/tools`). A hit on either counts. A bare pattern therefore
> matches on every host; a host-qualified pattern matches on that host only.
> The entry's own `oci`/`index` locator is never part of either.

This is binding on: `src/command/config.rs:104-116` (the live `--help` text,
pinned verbatim by `registry_add_help_states_how_a_pattern_is_anchored` at
`:3731-3761`), `src/command/config_keys.rs` `INCLUDE`/`EXCLUDE`
`KeySpec.description`, the mirrored **first paragraph** of
`src/config/declaration.rs`'s `include`/`exclude` doc comments,
`docs/src/configuration.md`, and
`catalog/skills/grim-usage/references/registries.md`.

**The blockquote is illustrative, not verbatim** — except for two load-bearing
substrings, because both gates are substring/prefix tests rather than equality
tests:

- `registry_add_help_states_how_a_pattern_is_anchored` (`config.rs:3731-3761`)
  asserts *substrings* of the `--help` text. Its assertions must be re-pointed
  at the new rule; at minimum the help text must contain both candidate
  examples (`acme/tools` **and** `ghcr.io/acme/tools`) and must no longer
  contain "with the registry host removed".
- `assert_description_prefix` (`config_keys.rs:699-710`) is a
  whitespace-normalized `starts_with`: `config_keys.rs`'s `INCLUDE`/`EXCLUDE`
  `KeySpec.description` must be a **prefix** of `declaration.rs`'s matching doc
  comment. So the two-candidate sentence has to open both, character-for-
  character, or the gate cannot see it.

**Why the first paragraph specifically:** anything stated in a second or third
paragraph is invisible to that prefix gate — which is exactly how the
superseded rule survived into the published JSON Schema
(`declaration.rs:276-282`, emitted verbatim by `grim schema --kind config`).

### C-030 — The unchanged non-boundaries

*Numbered out of sequence deliberately: contract IDs are append-only so every
existing citation stays valid. This contract belongs to cluster A.*

Three properties of `adr_registry_browse_filters.md` D5/D6/D9 are **not**
changed by dual-candidate matching, and each is the regression guard for a
scenario that would otherwise be reachable from no contract. All three are
what Principle 3 in the new ADR's Constitution check leans on — the filter did
not become a resolution or security boundary.

| Assertion | ADR clause | Scenario |
|---|---|---|
| A forced `--registry` browse applies no filter — `resolve_registries`' forced branch constructs entries with `RegistryFilter::default()`, so every candidate is tested against two empty sets and admitted | D9 | S-007 |
| `CatalogScope::Complete` is never filtered — the match arm at `catalog_service.rs:339-341` is total over the enum and `Complete` returns `true` unconditionally, so `grim status --check` still sees every declared artifact's `deprecated`/`replaced_by` | D5, D6 | S-008 |
| An excluded reference **resolves** byte-identically to a visible one — filtering is read-time only and `src/resolve/` never sees a `ResolvedRegistry` or the catalog | D6 | S-009 |

Each needs a test. The `Complete` one must go red if the match arm's
`CatalogScope::Complete => true` is mutated to call `matches`.

---

### UX scenarios — match candidate

**S-001 — a bare pattern is host-agnostic (regression).**
`include = ["acme/tools"]` on an index serving `ghcr.io/acme/tools` and
`quay.io/acme/tools`. `grim search` shows both rows. Exit 0.

**S-002 — a host-qualified include selects one host.**
`include = ["ghcr.io/acme/tools"]` on the same index. `grim search` shows only
`ghcr.io/acme/tools`. Exit 0.

**S-003 — a host-qualified exclude carves one host out of a bare include.**
`include = ["acme/tools"]`, `exclude = ["quay.io/acme/tools"]`. `grim search`
shows `ghcr.io/acme/tools` and not `quay.io/acme/tools`. Exit 0.
*This scenario passes under the naive-OR implementation only by accident of a
different config; with this exact config the naive OR shows both rows.*

**S-004 — a whole host is excluded.**
`exclude = ["quay.io/**"]`, no include. Every `quay.io` row disappears; every
other host's rows remain. Exit 0.

**S-005 — the same pattern behaves identically on an `oci` and an `index`
entry.** `include = ["acme/tools"]` against `oci = "ghcr.io"` and against an
index serving `ghcr.io/acme/tools` select the same repository. Exit 0.

**S-006 — a locator edit does not re-aim a pattern.**
With `include = ["acme/platform/**"]`, changing `oci = "ghcr.io/acme"` to
`oci = "ghcr.io"` (or the reverse) leaves the admitted set unchanged.

**S-007 — `--registry` applies no filter.**
`grim search --registry ghcr.io` browses unfiltered even when the configured
entry for `ghcr.io` carries an `include`. Exit 0.

**S-008 — `grim status --check` is never filtered.**
A declared artifact matching no `include` pattern still receives its
`deprecated` / `replaced_by` values. Exit 0.

**S-009 — an excluded package still resolves and installs.**
`grim add <excluded-ref>` succeeds and produces a lock entry byte-identical to
the one a visible reference produces.

**S-010 — the false-positive caveat, and its remedy.**
A repository literally named `ghcr.io/foo` hosted on `quay.io` is admitted by
`include = ["ghcr.io/**"]` (via its bare candidate). Adding
`exclude = ["quay.io/ghcr.io/foo"]` removes exactly that row and no other —
the exclude hits via the qualified candidate, and its bare candidate
`ghcr.io/foo` does not match a pattern beginning `quay.io/`.

**S-011 — a port-qualified host cannot false-positive.**
`include = ["localhost:5000/**"]` never matches any row's bare candidate: the
OCI `<name>` grammar forbids `:`, so no repository path can spell a
port-bearing host.

**S-023 — a mixed-case registry host does not match a lowercase pattern
(documented caveat, accepted).** *Added 2026-08-11; found independently by
the researcher and quality perspectives in Review-Fix round 1, both with
binary repros.*

An entry declared `oci = "GHCR.io/acme"` keeps that casing all the way into
`CatalogEntry.registry` — `trim_locator`'s own doc is explicit that the
stored url is identity and is "never case- or slash-folded", and
`oci_host_case_variants_dedup` (`registry_resolve.rs:923-935`) pins it. The
qualified candidate is therefore `GHCR.io/acme/tools`, and
`include = ["ghcr.io/**"]` — the DNS-conventional spelling every doc example
uses — admits nothing.

**This is new surface.** Before this change no host ever entered a candidate,
so no case difference could affect a pattern at all. `compile_pattern` leaves
`case_insensitive` at `false`, justified in its own doc as "OCI repository
names are lowercase by spec" — true of the *path* half only. The
`distribution/reference` grammar makes `alpha-numeric := /[a-z0-9]+/` for
paths but lets `domain-component` carry uppercase, and grim mirrors that
asymmetry exactly (`identifier.rs:87` enforces lowercase for repositories;
there is no registry equivalent).

Both directions were reproduced against a release binary:

- **include**: warns and is recoverable — the C-019 diagnostic fires
  (`filter admitted 0 of 2 repositories`), exit 0.
- **exclude**: **silent** — `exclude = ["QUAY.IO/**"]` hides nothing, exit 0,
  no diagnostic. This is a new instance of the fail-open the owner accepted
  in decision 7, on a surface that did not previously exist.

**Accepted, not fixed, this round.** Cost is not the obstacle — under
C-001's exact-capacity construction, folding the host is a ~10-byte ASCII
scan with zero extra allocations. The obstacles are that folding the
*candidate* without the *pattern* leaves a half-fixed asymmetry (a
lowercase-config / uppercase-pattern config would still fail), that a glob
has no decidable host portion to fold (`*/tools` has none), and that folding
breaks C-001's stated `repo()` byte-equality. The validation route — reject a
mixed-case host at load — is closed by Principle 9: it narrows an input that
parses on `v0.12.1`.

**Obligations this round:** one sentence under `#browse-filters` naming the
caveat (WP-D, C-032), and a correction to `qualified_candidate`'s doc, which
currently claims "a case difference between the locator and the row's
registry can no longer make a strip quietly not fire" — true of the *deleted*
strip, and misleading beside the sensitivity it does not mention (WP-A).

**Routed to the owner** as a deferred question: fold the host segment in a
later round, or keep the caveat.

---

## B. `--clear-include` / `--clear-exclude`

Owner decision 9. Purely additive; `grim config unset registry.<alias>.include`
stays exactly as it is and remains a second path to the identical mutation.

**Design it as one change with the surviving-mutant Block.** The clear branch
and the mutant (`if !include.is_empty() {` → `{`, `src/command/config.rs:1468`)
sit on the same arm; the clear flags are what finally give that arm a witness
(list stays vs list empties), which the mutant cannot fake.

### C-013 — clap definition

Two new `bool` fields on `RegistryCommand::Set` (`config.rs:130-156`):

```rust
#[arg(long, conflicts_with = "include")]  clear_include: bool,
#[arg(long, conflicts_with = "exclude")]  clear_exclude: bool,
```

`conflicts_with` names the **struct field ident**, matching the existing
`index`/`oci` precedent at `:138`. `RegistryCommand::Add` is unaffected — an
unfiltered `add` is simply omitting the flags.

### C-014 — `run_registry_set` signature

Gains `clear_include: bool, clear_exclude: bool`, forwarded from the dispatch
arm (`config.rs:204-211`) the same way `*default` already travels.

### C-015 — The "nothing to change" guard widens

`config.rs:1410-1418`:

```rust
if locator.is_none() && !make_default && include.is_empty() && exclude.is_empty()
    && !clear_include && !clear_exclude { ... exit 64 ... }
```

The message's enumeration must gain `--clear-include` / `--clear-exclude`, or
it contradicts the guard it backs. Its existing sentence pointing at
`grim config unset registry.<alias>.include` stays — both routes are valid.

### C-016 — The clear branches

At `config.rs:1468-1473`:

```rust
if !include.is_empty() {
    set_registry_field(&mut registries, alias, |rc| rc.include = include.to_vec());
} else if clear_include {
    set_registry_field(&mut registries, alias, |rc| rc.include.clear());
}
// symmetric for exclude
```

`else if` is correct because `conflicts_with` makes
`!include.is_empty() && clear_include` unreachable at the clap layer.

### C-017 — A clear is silent

No warning, on any list length. **Rationale:** `config unset
registry.<alias>.include` (`config.rs:868-881`) clears silently, and a clear
is that operation, not `config set`'s single-pattern replacement — the case
`warn_on_discarded_patterns` (`:584-598`) exists for, where a *replacement*
silently destroyed a multi-pattern list under a report that read as an
addition. A clear says what it does in its own flag name.

*(Flagged as agreed rather than assumed: this is the recon's open question, and
the answer is silence.)*

### C-018 — A clear round-trips as an absent key

An emptied list is written as **no key at all** — byte-identical to an entry
that was never filtered.

**The mechanism is the hand-rolled writer, not serde.** Per the parent ADR's
D13, `write_config` (`src/command/add.rs:881`) emits `[[registries]]` with
`writeln!`, and its own emptiness guard is what produces this behaviour
(`add.rs:978-984`):

```rust
for (key, patterns) in [("include", &rc.include), ("exclude", &rc.exclude)] {
    if !patterns.is_empty() {
        let list = toml::Value::Array(patterns.iter().cloned().map(toml::Value::String).collect());
        let _ = writeln!(out, "{key} = {list}");
    }
}
```

That guard already exists and already has a tripwire:
`write_config_omits_filters_when_unset` (`add.rs:1313`) asserts an unfiltered
entry emits no `include` line. A clear therefore needs **no write-layer
change** — but the reason is this guard, and a builder must not remove it.

*Secondary note:* `RegistryConfig.include`/`.exclude` also carry
`#[serde(default, skip_serializing_if = "Vec::is_empty")]` (`declaration.rs:283`,
`:302`). That governs `grim schema` and any serde round-trip; it does **not**
govern the file grim writes.

**Test:** extend `write_config_omits_filters_when_unset` (or add its sibling)
with an entry whose `include` was populated and then emptied — the written
file must contain no `include` line and must re-parse to `include == []`.

### C-019 — Idempotence

`registry set <alias> --clear-include` on an entry whose `include` is already
empty exits 0, writes a valid file, and leaves the entry otherwise unchanged.

### C-020 — Mutation witnesses (the gate this work must pass)

Four single-token mutations, each of which **must** turn a test red:

| Mutation | Test that must fail |
|---|---|
| `if !include.is_empty() {` → `{` at `:1468` | a `--default`-only edit on an entry carrying `include = ["a/**"]` asserts `include == ["a/**"]` afterwards |
| delete the `else if clear_include` arm | `--clear-include` on a populated list asserts `include == []` afterwards |
| delete `&& !clear_include` from the guard | `registry set <alias> --clear-include` with no other flag exits 0, not 64 |
| delete `conflicts_with = "include"` | `--clear-include --include x` together exits 64 |

The first row is the handover's WP-5 **BLOCK** and is closed by the same
change. The proper form is the mirror test the handover asks for: seed with
both lists, edit each *other* field in turn, assert `include` is unchanged
each time.

> **Row 1's literal spelling stops compiling once the clear branch lands
> (2026-08-11, WP-B fix pass).** `if !include.is_empty() {` → `{` leaves a
> dangling `} else if clear_include {`, which is a **syntax error**. The
> semantic equivalent is collapsing the whole `if`/`else if` to the
> unconditional replacement arm, and that is what must be run:
>
> ```
> registry_set_preserves_the_filter_lists_when_editing_any_other_field
>   → FAILED. 0 passed; 1 failed
> ```
>
> Recorded because anyone re-deriving row 1 verbatim gets a compile error and
> may misread it as "the mutant is already dead". It **is** dead — but by
> type-checking, and only in that literal spelling. The assertion is what
> kills the semantic form.

**Two further mutation families this package must keep killing** (WP-B fix
pass, all measured):

- **The plain-cell grammar (E-12 §4).** Substring assertions let **4 of 5**
  grammar mutants survive, including the one that prints pattern *text*
  instead of a count. One `assert_eq!` on the whole cell kills all five.
- **The three `escape_debug` sites** (`key`, `value`, the summarised
  locator). Dropping any one is caught.

### C-021 — The write-report `fields` array

**Decided (owner, review round 2), no longer a recommendation.**
`ConfigWriteReport` (`src/api/config_report.rs:179-193`) gains **one**
additive field: `fields`, an **always-present** array. One row per write, never
`{"items": [...]}` — the four sibling write verbs (`config set`,
`registry add`, `rm`, `use`) shipped as flat objects and cannot grow an
envelope.

**Element shape — an explicit `action` discriminator, never a bare `null`.**

```json
{
  "action": "registry-set",
  "key": "registry.acme",
  "value": "ghcr.io/moved",
  "scope": "project",
  "dry_run": false,
  "fields": [
    { "field": "oci",     "action": "set",     "value": "ghcr.io/moved" },
    { "field": "include", "action": "set",     "value": ["a/**", "b/**"] },
    { "field": "exclude", "action": "cleared" },
    { "field": "default", "action": "set",     "value": true }
  ]
}
```

- `field` — the `RegistryField` name (`oci` | `index` | `default` | `include`
  | `exclude`).
- `action` — `"set"` or `"cleared"`. A `"cleared"` element carries **no**
  `value` key.
- `value` on a `"set"` element is the field's own JSON type: string for a
  locator, array of strings for a list, boolean for `default`.
- Element order follows `RegistryField::ALL`'s frozen positions, so two
  invocations that touch the same fields produce byte-identical arrays.

**Why not `value: null` for "cleared".** `null` already carries two distinct
meanings in this object family: `ConfigWriteReport.value`'s documented
per-verb-shape null (`:184-186`, "`None` for `unset` / `rm` / `use`") and
`docs/src/stability.md`'s additive-field "this field does not apply" null. A
third, nested meaning — and the only one encoding an *event* rather than a
*state* — is the same class of quiet overload the ADR's Renovate lesson warns
against, applied to a token instead of a field name. Terraform's `-json`
formats never overload a bare null; they discriminate with an adjacent
explicit key. One extra key, no schema redesign.

**Always-present, on every verb.** `fields` is additive-safe under
`docs/src/stability.md` § Additive fields **only if** it is present on every
`ConfigWriteReport`, including the released verbs — `[]` where nothing
structured applies, never an absent key. `subsystem-cli-api.md` independently
bans `skip_serializing_if` in `src/api/`, so this is already the house rule.

**`key` and `value` are untouched.** `key` stays `registry.{alias}`; `value`
keeps its existing per-verb meaning. Repurposing either breaks
`test_config_registry.py:1459` and its Rust twin.

**Plain renderer.** `print_plain` (`:195-215`) keeps its single
`print_table(["Action","Key","Value","Scope","Dry Run"])` call and its one row
— the Single-Table Rule. `fields` is JSON-only, except that for `registry set`
the `Value` cell renders a compact summary of `fields` (e.g.
`oci=ghcr.io/moved, include=2, exclude cleared, default=true`) instead of the
blank cell a filter-only edit prints today. Only `registry set`'s cell changes;
it is unreleased.

**The tripwire moves in the same change.**
`config_write_report_json_pins_frozen_shape` (`src/api/config_report.rs:778-799`)
asserts the top-level key set is *exactly* `{action, key, value, scope,
dry_run}`; its own comment already says a future field must widen this set
rather than replace it. Widen it to `{action, key, value, scope, dry_run,
fields}`. `config_write_report_json_carries_action_key_value_scope` (`:757-776`)
and `config_write_report_plain_emits_table_with_action_columns` (`:801-825`)
do not enumerate the key set and survive unchanged.

**Assertions this contract requires:**

1. The widened frozen-shape key set, including `fields`.
2. A released verb (`config set`) emits `"fields": []` — present, empty, never
   absent.
3. `registry set acme --clear-exclude` emits exactly
   `[{"field":"exclude","action":"cleared"}]`, with **no** `value` key on that
   element (assert key absence, not `value == null`).
4. A multi-field edit emits one element per touched field, in
   `RegistryField::ALL` order, and nothing for untouched fields.
5. `docs/src/commands.md:209` gains `registry set` in the JSON-shapes row
   **and** the `fields` key in the shape string, in one edit.

---

### UX scenarios — clear flags

**S-012 — clear one side, keep the other.**
Entry with `include = ["a/**","b/**"]`, `exclude = ["legacy/**"]`.
`grim config registry set acme --clear-include` → exit 0.
`grim config registry show acme --format json` → `include == []`,
`exclude == ["legacy/**"]`, `oci` and `default` unchanged.

**S-013 — the flag pair is refused.**
`grim config registry set acme --clear-include --include 'a/**'` → exit **64**
from clap, before any alias resolution (so a non-existent alias yields clap's
error, not `no registry '<alias>'`).

**S-014 — a clear alone is enough to act.**
`grim config registry set acme --clear-include` does **not** hit the
"nothing to change" guard. Exit 0.

**S-015 — no flags at all still refuses.**
`grim config registry set acme` → exit **64**, message enumerating
`--oci/--index`, `--include`, `--exclude`, `--clear-include`,
`--clear-exclude`, `--default`, and naming the `config unset` route.

**S-016 — idempotent.**
Two consecutive `--clear-include` calls both exit 0; the second changes
nothing.

**S-017 — the file shows an absent key.**
After a clear, `grimoire.toml` contains no `include = …` line for that entry —
byte-identical to an entry that never had one.

**S-018 — patch semantics hold in the other direction.**
`grim config registry set acme --default` on an entry carrying
`include = ["a/**"]` leaves `include == ["a/**"]`. *(This is the scenario the
surviving mutant currently destroys silently.)*

---

## C. Registry identity

### C-022 — `RowSource`, the typed root identity — **the encoding is decided here**

This contract owns the representation. C-028 cites it and must not redefine it.

**The type**, in `src/config/registry_resolve.rs` (beside `ResolvedRegistry`,
the thing it identifies):

> **Amended 2026-08-11, WP-A Verify-Architecture (Block).** The first form of
> this contract made `Alias` carry the alias *alone*, and C-028 argued the key
> was therefore injective over the resolved set. That inference was wrong —
> being a *function of* `(alias, locator)` does not make a value *injective
> on* the pair, and `Alias(alias)` is a function of the alias half only. Two
> entries sharing an alias at different locators both survive
> `resolve_registries` (its `seen` key is `(normalize_locator, alias)`,
> `registry_resolve.rs:300`) and both rendered `"alias:acme"` — one merged
> root, which is **exactly S-022b**, a scenario this spec requires to pass.
> `Alias` now carries both halves. The cross-variant collisions (S-022) were
> always closed and are unaffected.

```rust
/// Which browse source a row is attributed to — the injective root identity
/// two entries at one locator need.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum RowSource {
    /// The synthetic local / dev-record group. Not a configured entry.
    Local,
    /// A configured entry that declared an alias. Carries the locator too:
    /// one alias may be declared at two locators across scopes (S-022b), and
    /// both survive `resolve_registries`' `(normalize_locator, alias)` dedup.
    Alias { alias: String, locator: String },
    /// A configured entry that declared no alias, identified by its locator.
    Locator(String),
    /// No source attribution — the row re-attributes by longest configured prefix.
    Unattributed,
}

/// The single constructor both producers delegate to.
pub(crate) fn row_source_of(alias: Option<&str>, locator: &str) -> RowSource;
```

A typed enum rather than a discriminated string, per `arch-principles.md`
"Domain types over `String`". **Injective on `(alias, locator)` by
construction**, in both directions:

- *Across variants* — `Alias { .. }`, `Locator("acme.example")` and `Local`
  are distinct values, so no `PartialEq`, `Ord` or `Hash` comparison can
  merge an alias with another entry's locator or with the `Local` sentinel.
- *Within `Alias`* — it now carries the whole dedup pair, so two entries that
  survive `resolve_registries` differ in at least one component and therefore
  in the value. This is the half the first form got wrong.

Nothing validates either collision away.

**`PartialOrd`/`Ord` are derived for container convenience only.** No
consumer sorts a `RowSource` today (`registry_order` stays `Vec<String>`),
and the derived order follows variant declaration order — `Local < Alias <
Locator < Unattributed` — which is **arbitrary and must not be relied on**.
It is not `registry_order`'s precedence and a `BTreeMap<RowSource, _>` keyed
on it would silently group every aliased entry ahead of every unaliased one.

**What "its locator" is:** `ResolvedRegistry.url` exactly as stored —
`trim_locator`-trimmed at construction (`registry_resolve.rs:264-266`), never
case- or slash-folded. It is deliberately *not* `seen`'s
`normalize_locator(...)` form; see C-029.

**`root_key() -> String`**, the rendering into the tree's existing
`String`-keyed space (`Node` keys, `registry_labels`, `registry_order` all stay
`String`, so no tree refactor):

| Variant | Rendering |
|---|---|
| `Local` | `"Local"` — the literal, unchanged, so existing tree/render expectations hold |
| `Alias { alias, locator }` | `"alias:{alias}/{locator}"` |
| `Locator(l)` | `"locator:{l}"` |
| `Unattributed` | `""` — never reaches the tree; `display_split` returns before asking |

The tags make the rendering injective too, which is what the `String`-keyed
maps need. **`/` is a sound separator, not a guess:** `validate_registries`
rejects an alias containing `/` (`project_config.rs:288`, with
`registries_alias_with_slash_rejected` at `:1050` pinning it), so the first
`/` after the `alias:` tag is unambiguously the boundary — the alias half is
recoverable, and no `(alias, locator)` pair can spell another pair's
rendering. A control-character separator would encode the same thing less
legibly and would surface as mojibake in any accidental display.

**`src/config/` deliberately owns this tree-key format.** `root_key`'s only
callers live in `src/tui/` (C-023's four sites, C-024's three), so the
`"alias:"`/`"locator:"` tags are a format two modules share. It stays beside
the type it renders. This sentence is the ownership statement — do not leave
it implicit.

> **Rationale corrected 2026-08-11 (WP-A Review-Fix, architect).** The
> original justification — "the alternative puts an `impl RowSource` in the
> TUI and inverts the same coupling without removing it" — **is false about
> Rust**: an inherent `impl` must live in the defining *crate*, not the
> defining *module*, so `impl RowSource { fn root_key }` in `src/tui/` is
> perfectly legal and would genuinely remove `config`'s knowledge of the
> tree's key space.
>
> The conclusion stands on better ground: **`CatalogGroup` produces a
> `RowSource` too, and it lives in `catalog`, not `tui`** — so a `tui`-side
> rendering would be reachable from a non-TUI producer. That is the reason
> to keep it in `config`. (Moving the impl now would also collide with WP-C
> for zero behaviour change.)

**Tests:**
- `row_source_of(Some("acme.example"), "x").root_key() != row_source_of(None, "acme.example").root_key()` (alias vs locator);
- `row_source_of(Some("Local"), "x").root_key() != RowSource::Local.root_key()` (alias vs the sentinel);
- **S-022b at the type level** — `row_source_of(Some("acme"), "ghcr.io/acme") != row_source_of(Some("acme"), "quay.io/acme")`, and their `root_key()`s differ. This is the assertion the first form of the contract could not pass.

**Two consequences a builder must handle, both stated so they are not
discovered late:**

1. **A tagged key must never be displayed.** `TuiState::registry_label`
   (`state.rs:941-946`) falls back to the string it was passed, and
   `render.rs:834` calls it unguarded (unlike `render.rs:472-473`, which checks
   `contains_key` first). Fix it once in `registry_label` — on a miss, strip a
   leading `alias:` / `locator:` tag before returning — rather than guarding
   each caller. `registry_label_falls_back_to_url_when_no_alias`
   (`state.rs:3639`) is the test that must be extended, not deleted.

   **The `alias:` strip takes the alias half, not the remainder.** After the
   2026-08-11 amendment an `alias:` key renders `"alias:{alias}/{locator}"`,
   so a naive `strip_prefix("alias:")` would surface `acme/ghcr.io/acme` on a
   miss. Split at the **first** `/` and return the alias; `/` cannot occur in
   an alias, so the split is exact. `locator:` keys strip whole, unchanged.
   A test must cover the miss path for an `alias:` key specifically — the
   existing one covers only the no-alias case.
2. **`registry_order` may no longer be folded into the attribution set.**
   `TreeBuildOptions.registry_order`'s doc (`tree.rs:41-51`) says a root key
   *is* its locator when the entry declared no alias, so the order vector
   doubles as attribution input. Under tagging that is false. The fold is
   already redundant — `registry_locators` carries every locator — so remove
   the fold and correct the doc.

### C-023 — `key()` on both producers

```rust
impl ResolvedRegistry { pub(crate) fn key(&self) -> RowSource; }   // registry_resolve.rs
impl CatalogGroup     { pub(crate) fn key(&self) -> RowSource; }   // catalog_service.rs
```

Both are one-line delegations to `row_source_of` (C-022) — the free function is
the single source of truth, so the two methods cannot drift and the equality
test below is a regression guard rather than the only guard. Both return only
`Alias` or `Locator`; `Local` and `Unattributed` exist for `TuiRow.source` and
are unreachable from a configured entry.

Neither introduces an import or a dependency direction: `RowSource` lives in
`config`, and `catalog → config` (`catalog_service.rs:40`) and `tui → config`
both already exist.

**Test:** for an aliased entry and an unaliased one, the `ResolvedRegistry` and
the `CatalogGroup` it drove return equal `RowSource` values.

`app.rs`'s private `source_key` (`:2988-2990`) is deleted; its four call sites
(`project_group_rows:1220`, `registry_order:2998`, `registry_labels:3030`,
`elision_registry:3048`) switch to `.key()` / `.key().root_key()`.

### C-024 — Fix `aggregate_registry_health` (regression, user-visible)

`app.rs:1081/1084/1087` push `g.registry.clone()` — the raw locator — into
`RegistryHealth.{offline,truncated,filtered}`, while `registry_labels` is now
keyed by `source_key`. `render.rs:834` looks the label up by locator, misses,
and falls back to the raw url:

```
main:                    Grimoire            filtered: acme (localhost:5002/uxrev)
feat/registry-set-verb:  Grimoire                   filtered: localhost:5002/uxrev
```

Push `g.key().root_key()` at all three sites (`RegistryHealth`'s three fields
stay `Vec<String>`; `render.rs:834` looks them up through `registry_label`,
whose fallback C-022 fixes). The existing guard
`apply_catalog_results_propagates_registry_labels` (`app.rs:3674-3699`)
hand-builds a url-keyed map and structurally cannot catch this; it needs a
fixture built by `registry_labels()` + `aggregate_registry_health()`.

This matters beyond cosmetics: in the single-registry case the root is elided
(D-ELIDE), so this clause is the entire in-TUI signal that a filter is
mis-aimed, and tracing output goes to `$GRIM_HOME/tui.log` for the whole
alt-screen session.

### C-025 — Fix `c019_filter_emptied`

`app.rs:1146` resolves a group's filter with
`.find(|r| r.url == group.registry)` — locator only. With two views at one
locator (the configuration `3e58d8d` exists to enable) both groups read the
**first** entry's filter. Use `.find(|r| r.key() == group.key())`.

### C-026 — Pin `project_group_rows` at its producer

`app.rs:1220` sets each row's `source` to `source_key(alias, url)` — becoming
`group.key()` under C-023. That one line *is* `1ed73aa`'s behaviour change, and
replacing it with the group's locator alone leaves the whole suite green.
`two_views_of_one_locator_are_two_named_roots` constructs `TuiRow`s with
`source` already set, so it pins the consumer, never the producer. Needed: a
test over `project_group_rows` with two `CatalogGroup`s sharing a `registry`
and differing in `alias`, asserting two distinct `source` values.

### C-027 — Remove `CatalogGroup.alias`'s lint suppression

`catalog_service.rs:141-144` carries
`#[allow(dead_code, reason = "captured for a future alias display; no
TUI/search consumer yet")]`. The reason is false on this branch —
`app.rs:1220` consumes `group.alias` — and it becomes doubly false once
`CatalogGroup::key()` (C-023) reads it.

**Owner: WP-A**, which already owns `src/catalog/catalog_service.rs` and lands
`key()` on the same struct. **Assertion:** the attribute is deleted outright
(not reworded) and `cargo clippy -D warnings` stays green, which is only true
once `key()` gives the field a second live consumer.

### C-028 — The root-key collision: recommended fix, and a Principle 9 finding

**The reproduced defect** (handover WP-2): `source_key` returns the alias when
present and the locator otherwise, so nothing stops one entry's *alias*
equalling another entry's *locator*, or the reserved `"Local"` sentinel. Both
entries' rows merge under one root labelled with whichever entry last wrote
into the `BTreeMap`. Exit 0, no warning. Not reachable on `main`, where every
root-key helper keyed on `r.url`.

**Scope: fix the key, do not validate the collision away.**

The representation is C-022's — `RowSource`, injective by construction. This
contract does not redefine it; it states what adopting it costs and what it
closes.

The root key is TUI-internal and `docs/src/stability.md:128-131` explicitly
excludes TUI appearance from the freeze, so the change is free of
frozen-surface cost. Two field-level consequences:

- **`TuiRow.source: Option<String>` becomes `TuiRow.source: RowSource`**
  (`state.rs:192-199`). The mapping is `None → Unattributed`,
  `Some("Local") → Local`, `Some(key) → Alias(..) | Locator(..)`. Its doc
  comment is stale in the same edit — it still says "set **only** for
  package-index sources … `None` for OCI registry sources", which
  `project_group_rows` (`app.rs:1220`) falsified.
- **Every reader of the 3-way overload changes shape**, from `==
  Some("Local")` to `matches!(.., RowSource::Local)`: `app.rs:1499`, `:1855`,
  `:2176`, `:2394`; `render.rs:400`; `detail.rs:141` and its fixture at
  `:650`; `tree.rs`'s test constructors `row2` (`:1022-1040`), `index_row`
  (`:1057-1063`), `local_row` (`:1067-1073`) and the expectation at `:1216`.
  `event.rs` and `update_check.rs` only construct the `None`/`Unattributed`
  case and survive mechanically.

This closes the alias-vs-locator collision, the alias-vs-`Local` collision and
the cross-scope variant (below) in one change, with no validation rule.

**Principle 9 finding — the validation route is not free.**
`RegistryConfig.alias` shipped in `v0.12.1`, constrained to non-empty +
trimmed + no `/` + no control characters + no `"` or `\` + unique **among
aliases** (`project_config.rs:220-320` at that tag). Nothing there compares an
alias to the locator set, and nothing reserves a name. So `alias = "Local"`,
and an alias equal to another entry's locator, both parse today and both work
correctly on `main`. Rejecting either at config load (exit 78 from
`validate_registries`) **narrows an input that parses on a released build**.

The rule that binds today is **`AGENTS.md` Principle 9** ("Stabilization
freeze on the road to 1.0.0: breaking changes are prohibited").
`docs/src/stability.md`'s manifest-input clause states the same policy for the
1.x line but is future-tensed on a page that opens "Grimoire is pre-1.0" —
cite Principle 9 as the prohibition, the stability page as its forward
statement. The handover's remediation did not price either.

If the owner nonetheless wants a validation layer, the Principle-9-clean form
is a **warning, not a rejection**, emitted at `grim config registry add`
(exit 0) beside the existing duplicate-alias check at `config.rs:1318-1336` —
a warning narrows nothing. `run_registry_set` must **not** be the insertion
point: `config.rs:1440-1445` is the exact six-line block WP-5's "alias exists
only in the other scope" finding already rewrites, and a second edit there is
a same-function, same-block conflict rather than merely a same-file one.

**The cross-scope residue, stated so it can be taken or deferred knowingly.**
Neither validation layer ever sees the merged project+global set:
`validate_registries` is called once per file (`project_config.rs:192`,
`config.rs:1176`, `:1229` — always a single resolved scope), and
`validate_alias_format` (`config.rs:1102`) has no registries array in scope at
either of its two call sites, structurally. The **only** function that holds
both scopes' entries is `resolve_registries` (`registry_resolve.rs:165-322`,
`project.iter().chain(global.iter())` at `:218`), and it returns `Vec`, not
`Result`, fail-open by explicit design (`:239-244`) — every browse path,
TUI startup, `search` and MCP included, depends on that unconditional return.

So: a project alias equal to a **global-only** locator (or the reverse)
survives *any* same-scope validation, at either layer, and is only visible at
the one place that cannot reject it.

Under C-022's key that residue is not merely mitigated, it is **structurally
absent**: `resolve_registries`' dedup key is `(normalize_locator(locator),
alias)`, so any two entries surviving into the resolved set differ in at least
one of those components — and `RowSource`, **as amended**, is injective on
exactly that pair. The key is therefore injective over the resolved set by
construction, and *scope never enters the argument*. Under the validation route
the residue stays open and would need `resolve_registries` to grow a
`Result`-returning cousin or a fail-open `tracing::warn!` arm. That asymmetry
is the decisive argument for fixing the key.

> **Corrected 2026-08-11 (WP-A Verify-Architecture, Block).** The sentence
> above originally read "`RowSource` is a function of exactly
> `(alias, locator)`. The key is therefore injective over the resolved set by
> construction." **That was a non-sequitur** — a function *of* a pair need
> not be injective *on* it, and the first `Alias(String)` form discarded the
> locator outright, so S-022b (one alias at two locators) merged two roots.
> The conclusion survives only because C-022's `Alias` now carries both
> halves. Do not cite the original inference; the injectivity is a property
> of the amended representation, not of the dedup key's arity.

**The residue that does remain is human, not structural.** Two entries can
still be *labelled* confusably — `registry_labels` (`app.rs:3022-3033`) builds
`"{alias} ({url})"`, and two views of one locator differ only in the half the
label does not carry. That is ADR D7's withdrawn territory and stays deferred;
it is not a collision, and every case in S-022 produces a distinguishable
label.

### C-029 — Locator-half dedup coverage (Warn, cheap)

`resolve_registries`' `seen` key is `(normalize_locator(locator), alias)`, and
dropping the **locator** half is caught by exactly one test, in
`command::release` — `config::registry_resolve`'s own 39 tests all pass.
Add the symmetric case beside
`one_locator_under_two_aliases_is_two_sources_across_scopes_too`: one alias at
two different locators (project `acme → ghcr.io/acme`, global
`acme → quay.io/acme`) must yield two entries.

Note that this key is a **different concept** from `key()` and must not be
unified with it: `seen` decides whether two `RegistryConfig` entries are the
same declared entry (case/slash-normalized locator + raw alias); `key()`
answers what names this entry's root on screen (as-stored, `trim_locator`-only
url). The per-locator cache key (`store/paths.rs:100-103`, raw locator, no
alias) is a third concept and is correct as-is — two aliased views of one
locator are meant to share one cache file.

---

### UX scenarios — registry identity

**S-019 — the TUI health line names the alias again.**
One configured entry `alias = "acme"`, `oci = "localhost:5002/uxrev"`, with a
filter that empties it. The TUI status line reads
`filtered: acme (localhost:5002/uxrev)`, not the bare locator.

**S-020 — two views of one locator get their own filter verdict.**
Project config declares one locator twice under two aliases, one unfiltered
and one whose `include` admits nothing. The narrow view's root shows the
`filtered:` clause; the wide view's does not.

**S-021 — two views of one locator are two named roots.**
The same config produces two distinct tree roots in the TUI, and
`project_group_rows` assigns two distinct `source` values.

**S-022 — an alias colliding with another entry's locator does not merge
roots.** The handover's three-entry reproduction (`oci = "acme.example"` with
no alias; `alias = "acme.example"` on `other.example`; `alias = "Local"` on
`third.example`) yields three distinct roots with three correct labels — plus
the synthetic local root, four in total — and the config still loads (exit 0):
it parsed on `v0.12.1` and must keep parsing.

**S-022b — one alias at two different locators is two roots.** Project
`acme → ghcr.io/acme`, global `acme → quay.io/acme`. Both survive
`resolve_registries` (that is what C-029's new test asserts), and today's
`source_key(Some("acme"), url)` returns `"acme"` for both — one merged root,
the same failure mode as S-022 reached by the other component of the dedup key.
The TUI must show two roots. This is C-029's fixture given a TUI-level sibling.

---

## Cross-cluster notes for the decomposer

- **A owns `RowSource`.** C-022/C-023 live in `registry_resolve.rs` and
  `catalog_service.rs`, both already A's files. **C cannot compile before A
  merges** — C-024/C-025/C-026 all consume `key()` / `RowSource`. That
  compile-order edge, not a file overlap, is the real A→C dependency.
- **The `tree.rs` overlap is a red herring on its own.** A's two
  candidate-contrast tests (`tree.rs:1789`, `:1818`) and C's `display_split`
  (`:612-621`) are ~1170 lines apart and merge cleanly. What *does* couple them
  is C-028's `TuiRow.source` type change: it rewrites `tree.rs`'s test
  constructors `row2`/`index_row`/`local_row`, which A's rewritten tests call.
- **A's file set** (`registry_filter.rs`, `catalog_service.rs`, `search.rs`,
  `registry_resolve.rs`, `tui/tree.rs` tests) and **B's file set**
  (`command/config.rs`, `api/config_report.rs`) are disjoint **except** for one
  string: the live `--help` text at `src/command/config.rs:104-116` (C-012)
  lives in B's file but states A's rule, and its pinning test
  (`config.rs:3731-3761`) fails the moment A lands. The plan assigns that
  string to **WP-D**, the restatement package, so B stays genuinely
  wave-1-independent.
- **C touches `src/tui/**` only** — `app.rs`, `state.rs`, `render.rs`,
  `tree.rs`, `detail.rs`. `key()` and C-027 sit in A's files, so C touches
  **zero lines of `src/command/config.rs`** and **zero lines of
  `catalog_service.rs`**, which removes the WP-2 / WP-5 same-block conflict
  entirely.
- **Order:** C-009 (fixture) → C-001…C-008 (matcher) → call-site churn →
  C-011 (diagnostics) → C-030 (the unchanged non-boundaries) → C-012 (every
  restatement, written from the landed code). B is independent of A and C.
- **Gate:** `task --force verify`. Plain `task verify` prints "up to date" and
  exits 0 from the Taskfile cache without running a test.

---

## Execution-phase clarifications

Edge cases the WP-A Stub phase surfaced that the contracts above did not
name. **All four resolve to no code change** — recorded here so the Implement
builder and the review panel do not re-litigate them.

### E-1 — `row_source_of(Some(""), locator)` needs no guard (C-022)

`Some("")` is representable in the signature and unreachable from a parsed
config (`validate_registries` rejects an empty or untrimmed alias at load).
It needs no fall-through to `Locator` regardless, because **injectivity does
not depend on it**: `Alias("")` renders `"alias:"`, `Locator("")` renders
`"locator:"`, and neither equals the other, `"Local"`, or `""`. The tagging
is what makes the key injective, not any property of the alias string. Add
the precondition to the doc comment; add no branch.

### E-2 — `qualified_candidate(r, "")` returning `"{r}/"` is correct (C-001)

C-001's two clauses are pinned on `registry` only, so an empty `repository`
yields a trailing slash — the mirror of the leading-slash case clause 2
exists to prevent. This is **deliberately not** given a third clause:
`CatalogEntry::repo()` is an unconditional `format!("{}/{}", registry,
repository)` and produces the identical `"{r}/"`, so C-001's third test (the
`repo()` equality on a non-empty registry) holds for an empty repository too.
A carve-out here would *break* that agreement to guard a state no catalog
build produces.

### E-3 — `Unattributed.root_key() == ""` stays total (C-022, C-028)

`root_key` is total and hands back `""` for `Unattributed`. Two
`Unattributed` rows therefore share a key — which is correct, since they
share the absence of a root. The guarantee that the value never reaches the
tree is **WP-C's**, discharged by `display_split` returning before it asks
(C-022's own note). WP-A adds no debug assert and does not split the
rendering.

### E-5 — `run_registry_set` takes the `too_many_arguments` allow (C-014)

C-014's two new `bool` parameters put `run_registry_set` at 9, over clippy's
threshold of 7. The gate is `cargo clippy --locked --all-targets -- -D
warnings` (`taskfiles/rust.taskfile.yml:43`), so this **fails verification**;
it is not a nit.

Take the attribute, with a `reason`:

```rust
#[allow(
    clippy::too_many_arguments,
    reason = "design C-014: the two clear flags travel flat, exactly as `default` already does. Collapsing this signature into a params struct is deliberately deferred — mixing a refactor into a feature diff violates the Two Hats Rule (quality-core.md)"
)]
```

There is established house precedent — eleven sites carry it, and
`catalog_service.rs:237-240` is the same situation verbatim (`load_catalog`
at 8 parameters, struct collapse deferred for the same Two Hats reason,
recorded as an ADR follow-up). Do **not** collapse the signature inside this
feature diff.

### E-6 — `RegistryFieldChange.field` is `&'static str`, and the ordering
claim needs its own guard (C-021)

WP-B's stub types `field` as `&'static str` rather than `RegistryField`,
because `RegistryField` lives in `src/command/config_keys.rs`, which is
**WP-D's** file. That is the right call for the wave-1 boundary and it
matches the file's existing `RegistryFieldEntry.key` convention.

**The reason above is not the load-bearing one** (WP-B post-stub architect).
Merely *naming* the type `RegistryField` in a field declaration is a read of
`config_keys.rs`, not an edit — the same argument E-6 makes for reading
`RegistryField::ALL`. The real blocker is narrower and stronger:
**`RegistryField` derives `Debug, Clone, Copy, PartialEq, Eq` and not
`Serialize`** (`config_keys.rs:181`), so typing the field would force an edit
to WP-D's file. E-6 stands on that reason.

**What `&'static str` loses is mainly spelling, not ordering.** It admits
`"Include"` or `"registry.oci"` at compile time; `RegistryField` admits
neither. `subsystem-cli-api.md` § Typed Enums Over Strings and
`quality-core.md`'s Warn-tier "stringly-typed APIs" both bite. And if WP-D
ever appends a sixth `RegistryField::ALL` member, **nothing fails to
compile** — `fields` would silently omit it from every `registry set` report,
a quiet gap on a JSON surface that only fires if someone remembers to write
the sixth test case.

**Implement closes both inside WP-B's file set.** Populate by iterating
`RegistryField::ALL` and mapping each member through an **exhaustive `match`
on `RegistryField`** returning `Option<RegistryFieldChangeAction>`:

```rust
RegistryField::ALL.into_iter().filter_map(|f| { /* exhaustive match on f */ })
```

Order then comes from `ALL` structurally, spelling from `field_name()`
structurally, and a sixth member becomes a **compile error** rather than a
silent omission. This mirrors `config.rs:299-317`, whose own comment reads
"a field added to that array must become addressable without a second edit
here". **Require the exhaustive match, not merely "iterate `ALL`".**

**The trap that makes this non-optional:** `RegistryField::ALL`'s order is
`Oci, Index, Default, Include, Exclude` (`config_keys.rs:203-209`) and the
enum's own *declaration* order is `Oci, Index, Include, Exclude, Default`
(`:182-188`) — **they differ**. Anything iterating the enum instead of `ALL`
produces the wrong order. `ALL` is documented append-only precisely because
the VS Code extension indexes it positionally, so the stub's doc comment
("in `RegistryField::ALL` order") is a consumer-visible promise, not
decoration. C-021 assertion 4 pins it.

### E-7 — C-021's plain-renderer clause is WP-B's Implement, and Specify gets a sixth assertion

Raised by the WP-B post-stub spec review as an unowned obligation, and it is
right that nothing currently fails if it is skipped: C-021's five listed
assertions are all JSON-shaped, while the renderer clause ("for `registry
set` the `Value` cell renders a compact summary of `fields` … instead of the
blank cell a filter-only edit prints today") is the contract's only
*behavioural* requirement.

It is **not dropped**. `ConfigWriteReport::print_plain` lives in
`src/api/config_report.rs`, which is WP-B's file, so WP-B's Implement owns
it. Add a **sixth** assertion to C-021's list:

6. `registry set` with a multi-field edit renders a non-blank `Value` cell
   summarising `fields` (e.g. `oci=ghcr.io/moved, include=2, exclude
   cleared, default=true`); every other verb's `Value` cell is byte-identical
   to today's. The Single-Table Rule holds — still one `print_table` call,
   still one row.

Only `registry set`'s cell may change, and only because that verb is
unreleased.

**Derive the cell from `!self.fields.is_empty()`, not from `action ==
WriteAction::RegistrySet`.** `fields` is `[]` on the other five verbs, so one
expression covers all six with no verb-sniffing branch and the Single-Table
Rule untouched.

Note also `config_report.rs:24-26`: the module `//!` doc already asserts
`fields` is "`[]` on every write verb except `registry set`", which the stub
does not yet satisfy. Contract-first, fine for one phase — it must not
survive Implement unfulfilled.

### E-9 — two file cells widen (orchestrator decision)

Both files are touched by this change, neither was in any work package's
cell, and each has exactly one natural owner.

**`src/api.rs` → WP-B.** The three new `config_report` types are absent from
the `pub use config_report::{…}` block at `src/api.rs:37-41`, while
`ConfigWriteReport` *is* re-exported and now carries a `pub` field of type
`RegistryFieldChange`. Every sibling type in that file is re-exported and
`subsystem-cli-api.md` § Adding a New Report Type step 2 makes it convention.
Without the widening, Implement must either path-qualify the import (breaking
the style at `config.rs:18`) or edit a file it does not own. WP-A, WP-C and
WP-D touch none of it, so there is no collision. Add `RegistryFieldChange`,
`RegistryFieldChangeAction` and `RegistryFieldValue` to that block.

**`docs/src/json-interface.md` → WP-D**, covered by the new contract below.

### C-032 — the three published surfaces this change invalidates (cluster B, owner WP-D)

None of these was claimed by any work package, and `subsystem-cli-api.md`
names `json-interface.md` as *the* consumer contract, so drift there is a
reportable defect rather than a nicety.

1. **`docs/src/json-interface.md:225`** pins the write-report shape as
   `{action, key, value, scope, dry_run}` for all six verbs. It is stale the
   moment C-021 lands. It must gain `fields`, and its `registry set` sentence
   ("reports `value` only when the call changed the locator; a filter-only
   edit reports `null`") must be reconciled with C-021's element array and
   E-7's `Value` cell.
2. **`docs/src/commands.md:150-176`** already documents `registry set` and
   its flags but nothing obliges anyone to document `--clear-include` /
   `--clear-exclude` there. Add them, alongside C-021 assertion 5's existing
   obligation on `:209`.
3. **`catalog/skills/grim-usage/references/registries.md`** does not mention
   `registry set` **at all** (`grep -rn "registry set" catalog/` → zero
   hits). That gap pre-dates this branch (`d9f3be4` added the verb without
   the catalog edit), but `catalog/README.md`'s drift duty is triggered by
   any `src/command/**` change and `task catalog:verify` gates it in CI. WP-D
   already owns the file. Document the verb including both clear flags.
4. **`test/tests/test_registries.py`'s `_ns_rel` docstring** — "the reference
   with the registry HOST removed and nothing else removed" — describes only
   the bare candidate and is now half the rule. Its tests still pass
   unchanged (the patterns are bare and hit the bare candidate), so this is
   doc drift, not a test defect. The file joins WP-D's cell for this one
   docstring; do not touch its assertions.
5. Two stale carriers of the *pre-`f790273`* wording remain unreachable from
   C-011's parity test, which only spans the producer constant and the two
   `docs/src` copies: `catalog/skills/grim-usage/references/registries.md:271`
   and `.claude/rules/subsystem-cli-commands.md:27` both still say "patterns
   are relative to this entry's own locator". Both are already in WP-D's
   cell; no mechanical gate exists for either, so they are a read-and-fix
   obligation.

### E-8 — `CatalogEntry.registry` is a bare host, and nothing says so

Raised by the WP-A post-stub architect. The whole dual-candidate rule rests
on `registry` being a bare host and `repository` carrying the entire
namespaced path — that is what makes S-005 (`oci` ≡ `index`), S-006 (a
locator edit cannot re-aim a pattern) and "a bare pattern is host-agnostic"
true. The invariant does hold — but **this contract originally named the
wrong guarantor**, and the correction matters because it changes what a
useful test must exercise.

> **Corrected 2026-08-11 (WP-A Review-Fix round 1, quality perspective,
> verified).** The claim was "`Catalog::build` passes
> `split_host_namespace(registry).0`, so the host is `/`-free". **False.**
> `split_host_namespace`'s fall-through arm is `_ => (registry, None)`
> (`registry_catalog.rs:855`), which returns the string **whole** when the
> namespace half is empty — and its own existing pin says so outright:
> `assert_eq!(split_host_namespace("ghcr.io/"), ("ghcr.io/", None));`
> (`registry_catalog.rs:1766`).
>
> The real guarantor is **`trim_locator`** (`registry_resolve.rs:104-106`),
> applied at all five `ResolvedRegistry` construction sites (`:299`, `:345`,
> `:382`, `:425`, `:433`), with `load_catalog` passing `reg.url` straight
> through (`catalog_service.rs:285`, `:294`). `IndexPackage::into_entry`
> guards its own path separately, rejecting an empty registry outright
> (`index_source.rs:89-91`).

> **Corrected again 2026-08-11 (WP-D Review-Fix round 1, doc perspective,
> verified at HEAD). The correction above over-swung, and this one is the
> record.** It is not `trim_locator` *instead of* `split_host_namespace`;
> for an `oci` source the two are **load-bearing in series**, and naming
> only one points a future maintainer at the wrong file.
>
> - **`split_host_namespace` is what removes the namespace.**
>   `Catalog::build` calls it (`registry_catalog.rs:690`) and spawns every
>   entry under the returned `host` (`:713`, `:722`), so `oci =
>   "ghcr.io/acme"` yields `registry = "ghcr.io"`, `repository =
>   "acme/foo"`. Nothing else does that. Refactor it and every authored
>   pattern silently re-aims.
> - **`trim_locator` is the upstream guard on its fall-through arm.** `_ =>
>   (registry, None)` (`registry_catalog.rs:855`) returns the string whole,
>   pinned by `assert_eq!(split_host_namespace("ghcr.io/"), ("ghcr.io/",
>   None))` (`:1766`). Only a **bare-host** locator reaches that arm — a
>   locator with a non-empty namespace half has its slash absorbed by the
>   split either way — so the trim is what stops `ghcr.io/` from landing a
>   `/` in every entry's `registry`.
>
> Two further clauses in the block above are wrong as written and are
> withdrawn: `load_catalog` does **not** pass `reg.url` to the matcher —
> the browse site is `reg.filter.matches(&e.registry, &e.repository)`
> (`catalog_service.rs:356`), and `reg.url` reaches only the catalog
> *build* and `CatalogGroup.registry`; and "It is *not*
> `split_host_namespace`" inverts the relationship outright. The `index`
> half is unaffected — `IndexPackage::into_entry` remains its own,
> independent guard (`index_source.rs:88-91`).

If either guard were ever dropped, every authored bare pattern would
silently re-aim with no diagnostic: precisely the failure class this ADR
exists to delete.

**New contract C-031, cluster A, owner WP-A.** `CatalogEntry.registry`
contains no `/`.

**A fixture-set assertion alone does not discharge it.** `seed_catalog`
writes the `registry` field as a JSON literal and `seeded_catalog` reads it
back through `Catalog::load`, so **no constructor runs** and
`!entry.registry.contains('/')` merely re-asserts the fixture's own literals
— delete `trim_locator` and it stays green. The test must exercise the real
path; the cheapest form that goes red under the regression is

```rust
assert!(!split_host_namespace(trim_locator("ghcr.io/")).0.contains('/'));
```

beside the fixture loop — it composes exactly the two guards, in the order
the production path applies them. Both the `qualified_candidate` doc and the
test comment must name **both**: `split_host_namespace` as what removes the
namespace, `trim_locator` as the upstream guard on its fall-through arm.

### E-10 — three consequences of the `Alias` amendment (WP-C's, decided here)

Raised by the WP-A re-stub. All three are choices the *old* encoding could
not offer.

**1. `registry_label`'s miss path reconstructs the full label, not the bare
alias.** C-022 consequence 1 predates the amendment and says "return the
alias half". Now that the key carries the locator too, the fallback can
rebuild `"{alias} ({locator})"` — **byte-identical to the hit path**, which
is exactly what `registry_labels` builds (`app.rs:3022-3033`) and what S-019
asserts. Do that. A miss then degrades to nothing at all rather than to a
shorter string, which is strictly better and costs one `format!`. A
`locator:` key still strips its tag and returns the locator unchanged.

**2. `RegistryHealth`'s three `Vec<String>` fields now hold longer keys** —
an aliased entry's element is `"alias:acme/localhost:5002/uxrev"`, not
`"alias:acme"`. C-024's mechanism is unchanged: the fields stay
`Vec<String>` of root keys and every display path goes through
`registry_label` (`render.rs:834`), which decision 1 makes total. **WP-C's
tests must assert on the rendered label, never on the raw key** — the key is
an internal identity, and asserting its spelling would pin an encoding the
spec deliberately owns at one seam.

**3. Split at the *first* `/`, never the last.** Both halves are recoverable
— alias is everything before the first `/` after the tag, locator is the
entire remainder verbatim — but only in that direction, because a locator
contains `/` and an alias cannot. A right-split silently returns garbage for
any multi-segment locator. Stated so nobody writes `rsplit_once`.

### E-11 — WP-A carries a temporary `dead_code` allow that WP-C must delete

Raised by WP-A's Specify pass, and it is a genuine merge-order problem, not a
nit. `RowSource`, its variants, `row_source_of`, `root_key` and both `key()`
methods have **no production consumer until WP-C rewires `app.rs`** —
test-only use does not count for dead-code analysis in the bin target. So
after WP-A's Implement lands, `cargo clippy --locked --all-targets -- -D
warnings` is still red on that cluster, and the plan's merge order requires
`task --force verify` green after **every** merge.

**Decision: WP-A carries the attribute; WP-C deletes it.** Running the gate
only on a merged A+C tree is the alternative, and it is worse — it makes
WP-A's merge unverifiable on its own and breaks the serialized merge
discipline the whole wave structure rests on.

The attribute goes on the smallest surface that silences the cluster, with a
`reason` that **names WP-C and this contract**:

```rust
#[allow(
    dead_code,
    reason = "E-11: RowSource's only production consumers land in WP-C (C-024/C-025/C-026/C-028, src/tui/app.rs). Test-only use does not satisfy dead-code analysis in the bin target. WP-C deletes this attribute — see its brief's hard gate"
)]
```

**This is a known failure mode on this project and it gets a hard gate.**
The 2026-08-09 round shipped a guard against a real process abort behind
`#[allow(dead_code)]` because its caller lived in another work package; had
the receiving package landed without wiring it, clippy would have stayed
green and the crash stayed live. So:

- **WP-C's brief carries "delete this attribute" as a hard gate**, not a
  suggestion, and WP-C's own clippy run is what proves the deletion is safe.
- **WP-C's post-merge verification is where it is checked.** If the attribute
  is still present after WP-C merges, that is a failed gate.

### E-12 — what `fields` reports: the write, not the invocation

Four gaps WP-B's Specify pass found in C-021 and refused to guess at. All four
resolve the same way, and the governing rule is `subsystem-cli.md`'s **"Report
actual results — a command reports what happened, not an echo of its
input."**

**1. A locator kind swap emits *both* elements.** `run_registry_set` sets
`rc.oci = Some(..)` **and** `rc.index = None` in one closure, so
`registry set acme --index …` on an OCI entry performs two mutations. It
reports two:

```json
[ { "field": "oci",   "action": "cleared" },
  { "field": "index", "action": "set", "value": "https://…" } ]
```

`oci` precedes `index` because `RegistryField::ALL` orders them so. Emitting
only the named side would hide a mutation the command actually performed —
exactly the class of quiet report the review round exists to close. C-021's
worked example shows one element only because its fixture's entry carried no
`index` to clear.

**The converse, stated (WP-B Implement raised it; the reading is confirmed):**
the unnamed locator side emits `cleared` **only when it actually held a
value**. `registry set acme --index …` on an entry that was already
index-only emits **no** `oci` element — reporting `oci cleared` where there
was no `oci` would be a phantom, and `fields` describes writes that happened.
This is also the only reading consistent with the pinned test
`registry_set_reports_one_element_per_touched_field_in_all_order`, which does
`--oci` on an entry carrying no `index` and asserts `index` is absent.

That branch had **no witness** — two mutations of the `had_oci`/`had_index`
guards survived all 2570 tests — so WP-B added
`registry_set_kind_swap_reports_both_locator_sides`, covering all four
states. Do not remove it.

**2. A named field whose value did not change still emits its element.**
`registry set acme --default` on an already-default entry, or `--oci` with
the identical locator, emits `{"field":"default","action":"set","value":true}`
all the same. `fields` describes **the assignment the write performed and the
resulting state**, not a before/after diff — the code assigns
unconditionally, and making the report diff-aware would require a pre-read
the command does not do and a consumer contract nobody asked for. Element
presence therefore means "this field was written", never "this field
changed".

**3. E-7's cell example is corrected to `ALL` order.** E-7 illustrated
`oci=ghcr.io/moved, include=2, exclude cleared, default=true` — *declaration*
order, which contradicts C-021 assertion 4's `RegistryField::ALL` order once
the cell is derived from `fields`. The correct rendering of that same edit is:

```
oci=ghcr.io/moved, default=true, include=2, exclude cleared
```

**4. The cell grammar is normative, not illustrative.** Segments joined with
`", "`, in `fields` order:

| Element | Segment |
|---|---|
| `set` on a locator (`oci`/`index`) | `{field}={value}` |
| `set` on `default` | `default={true\|false}` |
| `set` on a list (`include`/`exclude`) | `{field}={count}` — the count, not the patterns |
| `cleared` | `{field} cleared` |

An empty `fields` renders the cell exactly as today (blank for a filter-only
edit is no longer reachable, since a filter-only edit now populates `fields`).

**5. `docs/src/json-interface.md` must document the two clearing routes'
asymmetry** (WP-D, under C-032). `grim config unset registry.acme.include`
reports `action: "unset"` with `fields: []`; `registry set acme
--clear-include` reports `action: "registry-set"` with a `cleared` element.
Both routes are kept by owner decision 9 and `fields` is scoped to `registry
set`, so the asymmetry is intended — but a consumer diffing the two paths
must be able to read it somewhere rather than discover it.

**6. C-018's `add.rs` guard is depended on, not edited.**
`write_config_omits_filters_when_unset` (`src/command/add.rs:1313`) is in no
work package's file set, and that is correct: C-018 states a clear needs **no
write-layer change**, precisely because `write_config`'s existing emptiness
guard already omits an empty list. WP-B covers the contract from
`run_registry_set`'s side plus an acceptance test, which satisfies C-018's
"or add its sibling". **No builder may remove or weaken that guard** — it is
the mechanism the contract rests on.

### E-13 — C-011's two `docs/src` copies move to WP-A, or wave 1 cannot merge green

WP-A's Implement pass ends at **2578 passed, 1 failed**, and the single
failure is C-011's own parity test: `BROWSE_FILTER_REMEDY` now carries the
new string while `docs/src/configuration.md:491` and
`docs/src/commands.md:743` still carry the old one. Those two lines sat in
WP-D's cell, in **wave 3**.

That is a plan defect, not a build problem. The merge discipline requires
`task --force verify` green **after every merge**, and WP-A merges first — so
as written, wave 1 could not merge without a red gate, and the redness would
persist through WP-B and WP-C's merges too.

**C-011's two `docs/src` lines move to WP-A.** The parity assertion spans
exactly three sites — the producer constant and the two copies — and a
constant cannot move without them. Splitting a verbatim-string parity across
two waves is what created the gap.

**This does not collide with WP-D.** WP-D still owns both files for **C-012**,
its first-paragraph obligation, at different lines. The two packages are
never concurrent (WP-D depends on WP-A, and the merge order is serialized
A → B → C → D), so the disjointness invariant — which governs *concurrently
running* work packages — is not touched. WP-D's brief must simply not
re-litigate the remedy string it will find already correct.

**WP-D keeps the two agent-facing carriers**
(`catalog/skills/grim-usage/references/registries.md:271`,
`.claude/rules/subsystem-cli-commands.md:27`), which are *corrections* of the
pre-`f790273` wording rather than re-derivations, and which no parity test
reaches.

### E-14 — the dispatch-site flag swap is guarded only at the acceptance layer

WP-B's Implement pass measured it and reported it rather than papering over
it: **swapping `*clear_include` / `*clear_exclude` in the `run` dispatch arm
(`config.rs:230-231`) leaves the entire 2570-test unit suite green.** Five
acceptance tests catch it, and nothing else does.

This is a **structural property of the layout, not a defect, and not
actionable.** Every unit test calls `run_registry_set` directly, so none can
observe how `run` forwards into it; the acceptance layer is the only place
the dispatch arm is exercised at all, and it does catch the swap. The guard
exists — it is simply not where a reader scanning the unit tests would look
for it.

Recorded so a later reviewer who re-derives the mutation does not file it as
an uncovered path, and so nobody "fixes" it by duplicating the dispatch arm
into a unit test that would only re-assert clap's own wiring. The same shape
applies to `*default`, which shipped with the identical exposure.

> **Amended twice, 2026-08-11. Read the second amendment — the first was
> wrong.**
>
> *First (WP-B Review-Fix, quality perspective):* "not actionable" was too
> strong; a third remedy exists that is neither a test nor a refactor —
> separate the three `bool`s with the slices:
>
> ```
> … make_default: bool, include: &[String], clear_include: bool,
>   exclude: &[String], clear_exclude: bool
> ```
>
> It was justified with the claim that **no two `bool`s adjacent ⇒ every
> pairwise transposition becomes a type error**, closing the exposure at
> compile time.
>
> *Second (WP-B fix pass, measured):* **that justification is false.**
> Adjacency is not what makes a transposition a type error — the argument
> *types at the two positions* are, and `clear_include`/`clear_exclude` are
> both `bool` at positions 7 and 9 either way. The builder ran E-14's exact
> mutation against the reordered signature:
>
> ```
> swap *clear_include / *clear_exclude in the run dispatch arm
>   → COMPILES (exposure still open)
>   → still caught only by the 5 acceptance tests, exactly as E-14 recorded
> ```
>
> **What the reorder genuinely buys**, also measured:
>
> | Order | Adjacent pairs | Adjacent transposition |
> |---|---|---|
> | old (`include, exclude, clear_include, clear_exclude`) | slice·slice, bool·bool | **compiles silently** |
> | new (`include, clear_include, exclude, clear_exclude`) | slice·bool, slice·bool | **type error** — rustc emits `help: swap these arguments` |
>
> So the accurate claim is "**every *adjacent* transposition becomes a type
> error**" — strictly more caught than before, at zero cost. **Keep the
> reorder for that weaker reason.** E-14's original "guarded only at the
> acceptance layer" verdict for the same-typed non-adjacent pair was **never
> superseded and remains true**. `*default`'s identical pre-existing exposure
> stays out of scope (Two Hats — released signature).

### E-15 — three rulings from WP-C's Stub phase

**1. C-022 consequence 2 is WP-C's Implement, and the fold is live in *two*
places.** The contract requires removing the `registry_order` fold from the
attribution set and correcting `TreeBuildOptions.registry_order`'s doc, which
still says "a root key *is* its locator whenever the entry declared no alias"
(`tree.rs:41-51`) — false under tagging. WP-C's stub found the fold at
**`tree.rs:~489` and `render.rs:766-769`**
(`state.registry_locators.iter().chain(state.registry_order.iter())`), not
one site.

Under tagging the fold is **inert rather than wrong** — no reference begins
with `alias:`/`locator:`, so nothing fails. That is precisely why it would be
missed, and why it gets done now: an inert fold plus a doc asserting the
superseded rule is how the next reader concludes the order vector is still
attribution input. Both sites are in WP-C's files.

**2. E-10.2 does not reach `registry_order` / `elision_registry`, and the
stub's substitute is accepted.** E-10.2 said WP-C's tests must assert on the
rendered label, never the raw key — which presupposes a label path.
`RegistryHealth` has one (`registry_label`); `registry_order` and
`elision_registry` do **not**, because the tree's node key *is* the root key
and there is nothing else to assert.

For those, deriving the expected value from `root_key()` (rather than
hardcoding `"locator:ghcr.io/acme"`) satisfies the intent: it pins
**behaviour** and leaves the encoding free to move at its one seam, which is
what E-10.2 exists to protect. Hardcoding the tagged spelling in a consumer
test is what stays forbidden.

**3. `event.rs` and `update_check.rs` join WP-C's cell.** C-028 and the stub
brief both said they "survive mechanically" — **wrong**. A literal
`source: None` cannot survive `TuiRow.source` becoming a non-`Option` enum;
they need a mechanical *edit*, not zero edits. The builder was right to make
the substitution and right to flag it.

Both are `src/tui/**`, no other package touches them, and the plan's Scope
cell already reads "Registry identity in the TUI". The cell now says
`src/tui/**` outright. Every change there is `None → RowSource::Unattributed`
plus a test-module import — no semantics.

### E-16 — two rulings from WP-C's Specify phase

**1. E-15.2 extends to the four pre-existing `aggregate_registry_health`
tests. Ratified.** `..._names_offline_truncated_and_filtered_sources_h5`,
`..._names_a_source_an_exclude_only_filter_emptied_ha`,
`..._never_blames_a_filter_it_cannot_prove_h5` and
`..._names_a_filter_emptied_source_served_from_cache_w2` all assert the raw
locator spelling of `RegistryHealth`, so C-024 turns them red. E-10.2's
"assert the rendered label" is the wrong instrument here for a measurable
reason: **their fixtures are unaliased**, so the rendered label is the bare
locator under *both* keyings, and a label assertion would pass whether or not
C-024 landed — silently deleting the guard at four of its six sites. Deriving
from `key().root_key()` keeps them discriminating; mutations 3a/3b/3c confirm
it, killing 4, 2 and 6 tests respectively.

The rule stands where it bites: **hardcoding a tagged spelling in a consumer
test stays forbidden.** Derivation is the substitute wherever no label path
exists *or* where the fixture cannot distinguish the two keyings.

**2. E-15.1's fold removal is an Implement obligation with no
Specify-reachable assertion.** Deleting the fold at `tree.rs:~489` and
`render.rs:766-769` breaks four existing tests —
`two_namespaced_registries_no_duplicate_roots`,
`bare_host_row_attributes_to_configured_namespaced_registry`,
`overlapping_same_host_registries_attribute_to_most_specific`,
`index_and_registry_rows_attribute_independently` — which pass bare locators
through `registry_order` with `registry_locators: Vec::new()`. Implement must
move those fixtures onto `registry_locators` in the same change.

No test pins this, and none can: the fold's **presence** is unobservable (no
reference can begin `alias:`/`locator:`), only its removal is. E-15.1 already
called it inert-rather-than-wrong; this records that the inertness is exactly
what puts it beyond a red-first gate. It is verified by the four tests going
green again on corrected fixtures, not by a new assertion.

### E-17 — four rulings from WP-C's Review-Fix round 1

**1. E-10.2 gains a rider: a derived expectation must not be the
function-under-test's own body.** `app.rs`'s
`elision_registry_returns_some_for_single_registry` read
`assert_eq!(elision_registry(&ctx), Some(bare.key().root_key()))` while
`elision_registry`'s entire body is `Some(only.key().root_key())` —
`assert_eq!(f(x), f(x))`.

**Precision, corrected from the round-1 ledger:** this did **not** let a
mutation survive. Mutating the body breaks the self-agreement, so mutation 9
dies and the Specify phase's "zero survivors" was honest. What the tautology
was blind to is a **consumer-side encoding mismatch** — producer and expectation
moved together while `strip_default_registry`, a third party, did not. That is
the hole the Block walked through. Derivation buys encoding freedom; the rider
is what keeps it from also buying blindness. Fixed by asserting against the root
a **real row** lands on (`project_group_rows` + `display_split`), an
independently produced second value.

**2. E-16.2 undercounted the fold-removal breakage by one — it is five, not
four.** The fifth is
`render::spec_multi_registry_render_tests::spec_flat_multi_registry_bare_host_row_attributes_to_configured`
— the **`render.rs` fold site's own fixture**. E-15.1 correctly found the fold
at two sites; E-16.2 then enumerated only the `tree.rs`-side fixtures. The fix
is a fixture correction, not a weakened test: it adds `set_registry_locators`
beside the retained `set_registry_order` (which also drives `is_multi_registry`,
so dropping it would silently disable the branch under test), and the assertion
is byte-unchanged. Confirmed by four perspectives, one of them by deleting the
added line and reading the failure.

**3. The `default_registry` overload, and the Block it produced.**
`elision_registry` returning a tagged root key is **correct** and must stay:
`tree.rs`'s `segments` compares `default_registry` against a root key. The
defect was that the same value was *also* chained into the locator-prefix
attribution set, and fed to `strip_default_registry`'s literal
`repo.strip_prefix`, which a tagged key can never satisfy.

Resolved by making the flat single-registry branch apply D-ELIDE as the same
root-key equality the tree uses, via `display_split`. `strip_default_registry`
is deleted and `tree.rs`'s `.chain(default_registry)` removed, so both
`configured` sets are `registry_locators` alone. This also makes
`display_split`'s "attribute identically to the tree" doc true — it was false,
because the single-registry branch never called it.

**Two reviewer prescriptions were verified wrong and rejected**, recorded so
they are not re-proposed: reverting `elision_registry` to a bare locator (breaks
tree elision at the `segments` comparison), and making `render.rs`'s tree-root
label lookup unconditional (at depth > 0 the node key is the cumulative path
while the label is the short segment, so it would render the full path for every
non-registry group — the `contains_key` guard is load-bearing).

**4. SP-1's accepted gap has a second site.** Re-adding the inert
`.chain(default_registry)` to the tree's attribution set leaves the suite green,
exactly as re-adding the `registry_order` fold does. Both are removal-proven and
reintroduction-unguarded, for the same reason: an inert entry has no observable
effect to assert on. Recorded, not guarded — a guard would assert that dead code
stays absent.

### E-18 — two surfaces WP-C's review added to WP-D's cell

**1. `docs/src/commands.md`'s TUI health-line worked example** reads
`filtered: acme`. `registry_labels()` has always built `"{alias} ({url})"` for a
hit — never a bare alias — so the example has been wrong at every point in this
branch's history. WP-C is the first point at which the correct format becomes a
**tested** contract (S-019 pins
`"filtered: acme (localhost:5002/uxrev)"`), which is what makes it checkable
rather than best-effort. Not one of C-032's three surfaces — a new item.

**2. `adr_multi_registry_mcp.md` needs an amendment, four clauses.** Its §1 note
is the record of the deferred catalog-refresh seam, and WP-C invalidates the
promise it carries:

1. `TuiRow.source` is a typed `RowSource`, and the tree/flat root key is
   `RowSource::root_key()` — a **tagged, display-only** rendering
   (`alias:{alias}/{locator}` / `locator:{locator}` / `Local` / `""`), never a
   lock or install key.
2. **Re-arming `spawn_catalog_refresh` is no longer a one-line change.** Its
   `Catalog`-shaped consumer produces `Unattributed` rows that root in *locator*
   space while `registry_order` is in *root-key* space, and `merge_catalog_rows`
   replaces the row set without touching the three vectors — so the first
   refresh after a naive re-arm renders every registry twice, once as an empty
   `0/0` root, with rows sorted to `usize::MAX` and aliases lost. Migrating onto
   `catalog_service::load_catalog` is a **precondition**, not an optimisation.
   (The in-code note at that seam was corrected in WP-C's fix pass; this is the
   durable record.)
3. The encoding has exactly one seam, and its parser (`label_from_root_key`) is
   part of it.
4. The `registry_order` / `registry_locators` contract: two projections of one
   ordered set, index-aligned by construction, with `usize::MAX` the deliberate
   — and silent — fallback for a key outside the set. `Local` is its one
   legitimate resident.

### E-4 — `config` exports no constructor for `Local` / `Unattributed`

By design. `row_source_of` structurally cannot return either, and both
`key()` methods return only `Alias`/`Locator`. Both variants are constructed
exclusively at WP-C's `TuiRow.source` mapping sites (C-028: `None →
Unattributed`, `Some("Local") → Local`). Stated so WP-C does not read the
absence as a gap.
