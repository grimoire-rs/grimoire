# Handover — `feat/registry-set-verb` high-tier review findings

**Written 2026-08-11.** Merged record of two independent high-tier reviews of
this branch, plus the owner's decisions taken on 2026-08-11. Self-contained:
the raw per-perspective reports lived in session scratchpads that do not
survive, so every finding below carries its own evidence.

Reviews merged: an 8-perspective `/hex-review` panel (spec, test-coverage, UX,
security, performance, docs, architect, researcher) with a `codex:rescue`
cross-model gate, and a prior 6-perspective review with its own cross-model
leg. Where the two disagreed, the empirically-verified side won and the other
is recorded as withdrawn.

## What this branch is

`feat/registry-set-verb`, 5 commits on top of `main`, 15 files, +1612 / −580.

| Commit | What |
|---|---|
| `d9f3be4` | `feat(config): add 'registry set' to edit an entry in place` |
| `d2eb999` | `docs(config): state that a wildcard-free pattern expands downward only` |
| `f790273` | `feat(config)!: match browse filters against the repository path` |
| `3e58d8d` | `fix(config): browse every entry a file declares at one locator` |
| `1ed73aa` | `fix(tui): give every declared entry its own tree root` |

Browse filters shipped without an edit path — a registry's filter could only be
chosen at `registry add` time. This branch adds the edit verb, changes what a
pattern is matched against, and stops two kinds of silent entry loss.

Uncommitted in the tree: `Cargo.toml` / `Cargo.lock` at 0.12.1 → 0.13.0. Owner's
local release emulation, deliberately not committed. Out of scope.

---

## Owner decisions, 2026-08-11 — these gate the work below

Read these first. Several review findings are **deleted** by them; the work
packages already reflect that.

1. **There is no breaking change.** `src/config/registry_filter.rs` is absent
   from every release tag (verified `v0.9.0` … `v0.12.1`) — browse filters have
   never shipped. **Drop the `!` and the `BREAKING CHANGE:` footer from
   `f790273`** at `/finalize`. No Constitution Deviations row is required, the
   plan's "Additive-only throughout" row stands, and `docs/src/upgrading.md`
   needs no section. *This deletes the constitution gate entirely.*

2. **Candidate rule: dual-candidate match (option A).** Each pattern is matched
   against **both** `repository` and `{registry}/{repository}`; a hit on either
   admits (or excludes). No host-detection heuristic — a host cannot be
   identified by inspection, since OCI namespace segments may carry dots
   (`acme.corp/tools`) and hosts need not (`localhost:5000`). Consequences:
   - `acme/tools` matches via the repository candidate → every host.
   - `quay.io/acme/tools` matches via the fully-qualified candidate → that host
     only. This is the host precision the rule currently cannot express.
   - Identical for `oci` and `index` sources, with no per-kind branch —
     explicitly required by the owner.
   - Every pattern already written for the shipped rule keeps working.
   - The entry's own locator remains **not** an input, so everything
     `f790273` bought is preserved.

   *This replaces the architect's Block (adopt the fully-qualified candidate)
   rather than trading it for another.*

3. **Do not split the branch.** One feature branch carrying several changes is
   fine. A rename is optional.

4. **Do not dedup the flat / JSON search rows.** The JSON is the interface the
   VS Code extension consumes, and it *should* see that multiple registries are
   configured. Add `alias` to `SearchEntry` and a Registry column to the table
   so the rows are distinguishable. Duplicates become legible, not fewer.

5. **`grim init --registry` seeds an alias.**

6. **Keep both `registry use` and `registry set --default`.** Removing `use` is
   not available — it shipped in `v0.12.1` and removal is a breaking change
   under Principle 9. Two mechanisms for one write is acceptable. Align the
   `action` string only if free (`registry-set` is new and unreleased).

7. **No `MAX_REGISTRIES` cap, and the O(n²) locator scan is not a gate.** It is
   entries × distinct locators; artifacts never enter it and are capped at 500
   per registry, so it does not scale with catalog size. Not an attack vector.
   The 3-line `HashMap` fix is optional — take it because it is free, not
   because it blocks.

8. **The exclude-only fail-open: document and accept.** An exclude-only filter
   has no zero-match diagnostic in either direction, by design. Add one line
   under `#browse-filters`. *This downgrades the rewritten test
   `test_exclude_that_removes_nothing_stays_silent` from a High finding to a
   docstring fix — its assertion is now the ruled behaviour; only its rationale
   still argues from the superseded C-005 semantic.*

9. **Add `--clear-include` / `--clear-exclude` to `registry set`.** Requested by
   the VS Code extension. Patch semantics are untouched — the clear rides a
   *distinct* flag rather than overloading absence. Do **not** make
   `--include ''` mean clear: it is exit 65 today, and an unset shell variable
   (`--include "$VAR"`) would silently destroy a list. `config unset` stays
   exactly as it is. Purely additive.

## Verdict

**Request Changes.** 6 Block, 13 High, 12 Warn after the decisions above.

Verify with `task --force verify` — plain `task verify` prints "up to date" and
exits 0 from the Taskfile cache without running a test. Last measured green at
2555 unit + 975 acceptance, *before* any of the fixes below.

---

## Work packages — grouped by OWNING FILE, not by theme

This grouping is deliberate. The previous convergence round on this feature
first grouped review findings thematically and five of six rows collided on the
same files — it could not have run in parallel worktrees at all. Regrouped by
owning file, four packages ran concurrently with zero merge conflicts.

### WP-1 — `src/config/registry_filter.rs`

**BLOCK — implement the dual-candidate rule (decision 2).**

Today `browse_candidate(repository) -> String` is the identity function and
`catalog_service.rs:341` feeds it `e.repository` only, so the registry host is
discarded before the matcher ever runs. One index entry serving
`ghcr.io/acme/tools` and `quay.io/acme/tools` produces the identical candidate
for both rows: `exclude = ["acme/tools"]` hides both and **no pattern exists
that hides one and keeps the other**.

The rows themselves are fine — the index catalog keys by `e.repo()`, the
fully-qualified ref (`registry_catalog.rs:641` → `:201-203`), so both rows are
stored, listed and displayed with distinct identities. The collapse is
filter-local, which is why the fix is filter-local.

`adr_registry_browse_filters.md:615-618` had already rejected the shipped rule
on exactly this ground — *"for an index source `repository` is host-stripped,
so `acme/foo` on `ghcr.io` and on `docker.io` collide into one pattern space —
and the built-in fallback browse source is an index"* (`FALLBACK_INDEX =
https://index.grimoire.rs`, `src/command.rs:254`; index layout
`index/<host>/<ns>/<pkg>/`, array-valued `registryHosts`,
`docs/src/hosting-an-index.md:227,244`).

**Remediation:** give the matcher both candidates — either
`browse_candidate(registry, repository) -> (String, String)` or change
`RegistryFilter::matches` to take `(registry, repository)`. Admit on either
match; for `exclude`, hide on either match. Keep the named seam: it holds the
frozen rule statement and its tests.

**BLOCK (test) — no fixture in the tree can express a two-host index.**
`seed_catalog` takes one url, which is why nothing went red. Add a two-host
variant and assert that a host-qualified pattern selects one row and the bare
pattern selects both. This is the fixture whose absence hid the defect.

**Suggest —** the `-> &str` micro-optimisation for the identity function is
moot; the signature changes anyway under decision 2.

### WP-2 — `src/config/registry_resolve.rs`, `src/config/project_config.rs`

**BLOCK — root-key collision merges two registries under one mislabeled root.**

`source_key = alias.unwrap_or(url)` (`src/tui/app.rs:2988-2990`) became the sole
tree-root identity. Nothing stops one entry's *alias* equalling another entry's
*locator*; `validate_registries` checks alias uniqueness only among aliases.
Reproduced, exit 0, no warning:

```toml
[[registries]]
oci = "acme.example"          # no alias -> root key "acme.example"
default = true
[[registries]]
alias = "acme.example"        # collides with the entry above's root key
oci = "other.example"
[[registries]]
alias = "Local"               # collides with the synthetic local-artifacts root
oci = "third.example"
```
```
entry url=acme.example   alias=None         -> root key 'acme.example'
entry url=other.example  alias=acme.example -> root key 'acme.example'
entry url=third.example  alias=Local        -> root key 'Local'
```

Both entries' rows merge under one root, labelled with whichever entry last
wrote into the `BTreeMap` in `registry_labels`. Not reachable on `main`, where
every root-key helper keyed on `r.url`. `"Local"` is a live sentinel — 27 uses
in `app.rs`, 6 each in `tree.rs` and `render.rs`.

**Remediation:** reject at validation — an alias that equals any configured
locator, and the reserved `Local` sentinel. The alias-format check lives in
`src/command/config.rs:1102` (`validate_alias_format`, CLI path) *and*
`src/config/project_config.rs:264` (config load); both need it, and the
load-time one is authoritative. **This is the one place WP-2 touches
`config.rs` — announce it to WP-5 before editing rather than merging over
them.**

**Warn — the locator half of the dedup key is defended only by an unrelated
module.** Dropping the locator from the `(locator, alias)` key is caught by
exactly one test, in `command::release`. `config::registry_resolve`'s own 39
tests all pass. Add the symmetric case beside
`one_locator_under_two_aliases_is_two_sources_across_scopes_too`: one alias at
two different locators (project `acme → ghcr.io/acme`, global `acme →
quay.io/acme`) must yield two entries.

**Warn — `adr_registry_default_dedup.md:45` still reads "deduped by url".**
`v0.12.1` keyed `seen` on `normalize_locator(locator)` alone; HEAD keys on
`(normalize_locator(locator), alias)`. This is the one ADR covering *released*
behaviour and it is now wrong. Amend it (WP-7 carries the record work, but the
fact belongs here).

**Suggest —** `a_global_file_may_declare_the_same_locator_twice`'s comment
(`:660-661`) claims `seen` is written by project entries only. It is written for
every entry; the test passes because the alias is in the key. The same comment
defect makes `3e58d8d`'s commit message wrong: **two alias-less entries at one
locator in ONE file still collapse**, with the second's filter dropped. That is
deliberate — it is what keeps `grim init`'s #28 collapse working — but it is
unrecorded and mis-described. Pin it with a test.

### WP-3 — `src/catalog/catalog_service.rs`

**HIGH — the shared loader accepts a mixed-kind pairing and serves one view over
the wrong transport.** `loads` (`:264-277`) deduplicates by URL and records only
the **first** entry's `SourceKind`; `:279-293` performs one refresh for that
kind and `:318-342` reuses the result for every entry at that URL. The
cross-model leg confirmed the config is *accepted*:

```toml
[[registries]]
alias = "idx"
index = "http://127.0.0.1:18999"
default = true
[[registries]]
alias = "oci"
oci = "http://127.0.0.1:18999"
```

`grim config registry list --format json` prints `index` then `registry` at the
same URL. `validate_registries` constrains `index` locators
(`project_config.rs:253-263`) but never `oci`, so nothing rejects it. The
inline comment at `:264-266` — *"the kind is read off the locator
(`classify_index`), so they cannot disagree"* — is false: kind is read off which
**field** the entry set (`registry_resolve.rs:220` vs `:226-229`).
**Remediation:** key `loads` on `(url, kind)`, or reject the pairing at
validation. Either way, fix the comment.

**HIGH — the fan-out's distinctness half is unpinned.** Three mutations of the
`loads` / `load_of` mapping (`:267-277`) survive the full suite: every registry
reading `loads[0]`; entry N reading the first *different* locator; and
**reverting `3e58d8d` entirely** (spawn one load per entry). Every existing
multi-source fixture seeds both locators with the same `FIXTURE_REPOS`, so it
cannot see the difference. A verified test that goes red under the first two:

```rust
#[tokio::test]
async fn each_source_reads_its_own_locators_catalog() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = GrimPaths::new(tmp.path().to_path_buf());
    seed_catalog(&paths, "ghcr.io", &[("ghcr.io", "acme/only-on-ghcr")], false);
    seed_catalog(&paths, "quay.io", &[("quay.io", "acme/only-on-quay")], false);
    let (results, _) = browse_capturing(
        tmp.path(),
        &[source("ghcr.io", Some("gh"), &[], &[]), source("quay.io", Some("qy"), &[], &[])],
        "", CatalogScope::Browse,
    ).await;
    assert_eq!(group_repos(&results, 0), vec!["ghcr.io/acme/only-on-ghcr".to_string()]);
    assert_eq!(group_repos(&results, 1), vec!["quay.io/acme/only-on-quay".to_string()]);
}
```

**HIGH — the cold-cache race the fan-out exists to prevent is not tested, and
the test whose comment claims to test it cannot.**
`two_views_of_one_locator_each_get_the_whole_catalog` browses **offline against
a pre-seeded cache** (`browse_capturing` always passes `offline=true`, `:758`),
and `Catalog::coordinate`'s offline branch returns `Serve` before any lock is
touched (`registry_catalog.rs:542-544`) — so no lock is ever contended and
per-entry spawning is indistinguishable. A comment asserting a guarantee the
test does not provide is worse than no comment.

It *is* deterministically reproducible at the unit layer: swap `FailingAccess`
for a counting `OciAccess` delegating to
`crate::oci::access::memory_registry::MemoryRegistry`, run `load_catalog` with
`offline=false, force=true` and two same-locator entries, then assert
`rows_before_filter == 1` for **both** groups. The walk-count assertion alone is
not a discriminator (the lock serialises them either way); the
`rows_before_filter` assertion is what catches it.

The underlying race analysis is **correct and load-bearing** — verified against
`Catalog::coordinate` (`registry_catalog.rs:527-587`): two entries at one
locator produce one `catalog_file_for` path, `flock` is per open file
description so the second acquire in the same process fails `EWOULDBLOCK` →
`LockErrorKind::Locked` → `:579` serves `Catalog::empty` on a cold cache. One
task per distinct locator removes it.

**HIGH — this diff deleted the only test pinning `truncated: true`.**
`filter_never_rescues_a_build_truncated_group_c008` (plan C-008 / ADR D6) was
removed by `f790273`; nothing replaced it. `main` carries 3 `any_truncated`
assertions in this file, HEAD carries 2. Hardcoding `truncated: false` passes
all 2555 tests. Blast radius is user-visible and silent: `search.rs:135`
suppresses the truncation hint and `app.rs:1052` drops the TUI indicator. Pure
re-add — nothing in it depended on the old candidate rule.

**Warn — doc comments falsified by their own hunk.** `:405-411`
(`BROWSE_FILTER_REMEDY`'s doc) still says the root cause is "the match candidate
is relative to the declaring entry's own locator, plan C-005" while the constant
on the next line says the opposite. Same at `:418-425` and `:452-457` (whose
recovery sketch needs re-derivation, not rewording — "matched nothing
source-relative but matches fully-qualified" is no longer a discriminator, and
under decision 2 the candidates change again). Test comments at `:838`,
`:1158`, `:1170`. Two comments cite tests this diff deleted: `:721` →
`each_source_strips_its_own_url_through_the_seam_w10`, and `tui/tree.rs:1837` →
`source_matching_nothing_keeps_its_group_s004`.

**Warn (optional, decision 7) — O(n²) locator scan.** `load_of` builds with a
linear `loads.iter().position(|(u, _)| *u == reg.url)` inside a `.map()` over
every registry. Measured offline, release binary: 20 000 distinct locators
0.57 s; 100 000 distinct 14.55 s / 290 MB; 100 000 *same* locator 0.22 s. A
second measurement put the isolated scan at 37.5 s on the largest legal 8 MiB
config vs 29 ms hashed. Owner ruled this is not a gate — it is entries ×
distinct locators and never scales with artifacts. Take the fix because it is
3 lines, not because it blocks:

```rust
let mut loads: Vec<(&str, SourceKind)> = Vec::new();
let mut load_index: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
let load_of: Vec<usize> = registries
    .iter()
    .map(|reg| {
        *load_index.entry(reg.url.as_str()).or_insert_with(|| {
            loads.push((&reg.url, reg.kind));
            loads.len() - 1
        })
    })
    .collect();
```

**Suggest — a whole `Catalog` is cloned per entry where `main` moved it**
(`:322`). Measured 174 µs at the 500-entry `MAX_CATALOG_REPOS` cap (1.25 % of a
13.9 ms `grim search`). Avoidable outright — the match arm reads the catalog
only through `&self` methods and `by_load` is never mutated in the loop:
`.and_then(Option::as_ref)` instead of `.cloned()`. Compile-verified.

**Suggest —** the dedup compares raw urls (`:272`), so two case-variant
locators (`ghcr.io/acme` / `GHCR.IO/acme`) produce two cache files and two full
registry walks. Pre-existing at the cache layer (`paths.rs:100-103` hashes the
raw string too), but the commit body should not claim "the redundant walk" is
gone. Keying on `normalize_locator(&reg.url)` closes it.

**Doc (decision 8) —** the exclude-only filter has no zero-match diagnostic in
either direction; `zero_match_warning` (`:483-504`) returns `None` at `:492`
whenever `include_patterns()` is empty. That is now ruled intended. State it in
`docs/src/configuration.md` under `#browse-filters`.

### WP-4 — `src/tui/app.rs`, `src/tui/state.rs`, `src/tui/render.rs`, `src/tui/tree.rs`

**HIGH — the TUI registry-health line lost its alias label (regression).**
`registry_labels` is now keyed by root key (`source_key`), but
`RegistryHealth.offline / truncated / filtered` still push the **locator**
(`g.registry`, `app.rs:1081-1087`). `render.rs:834` looks up by locator, misses,
and falls back to the raw url. A/B'd on one config, two binaries:

```
main:                    Grimoire            filtered: acme (localhost:5002/uxrev)
feat/registry-set-verb:  Grimoire                   filtered: localhost:5002/uxrev
```

This matters more than a cosmetic label: in the **single-registry** case the
root is elided (D-ELIDE), so this clause is the *entire* in-TUI signal that a
filter is mis-aimed — and tracing output goes to `$GRIM_HOME/tui.log` for the
whole alt-screen session. With two views of one locator it becomes ambiguous,
naming a string both entries share.

Fix: push `source_key(g.alias.as_deref(), &g.registry)` at all three call sites.
The existing guard `apply_catalog_results_propagates_registry_labels`
(`app.rs:3674-3699`) hand-builds a url-keyed `labels` map and so cannot catch
this — give it a fixture built by `registry_labels()` +
`aggregate_registry_health()`.

**HIGH — `c019_filter_emptied` resolves a group's filter by url alone**
(`app.rs:1146`, `.find(|r| r.url == group.registry)`). With two views at one
locator — the configuration `3e58d8d` exists to enable — both groups get the
**first** entry's filter. A wide unfiltered view beside a narrow one that admits
nothing yields a 0/0 root labelled `mine (ghcr.io)` with no `filtered:` clause,
which is exactly the "no GUI sees it" gap `3e58d8d`'s own message cites as its
motivation. Key the lookup on `(url, alias)`; `CatalogGroup` already carries
`alias` (`catalog_service.rs:141-144`), and `source_key` is the function that
already computes this identity.

**HIGH — `1ed73aa`'s behaviour change has no test at its production site.**
`project_group_rows` (`app.rs:1220`) sets each row's `source` to
`source_key(alias, url)`. That one line *is* "give every declared entry its own
tree root". Replacing it with the pre-fix `group.registry.clone()` leaves all
2555 tests green. `two_views_of_one_locator_are_two_named_roots` looks like the
guard but constructs `TuiRow`s with `source` already set — it pins the consumer,
never the producer. Add a test over `project_group_rows`: two `CatalogGroup`s,
same `registry`, different `alias`, asserting two distinct `source` values.

**HIGH — `TuiRow.source` doc is stale and the field is a 3-way overload.**
`state.rs:192-199` still says "set **only** for package-index sources … `None`
for OCI registry sources". It now carries: the producing entry's key for *every*
catalog row; the literal sentinel `"Local"` (9 call sites doing
`row.source.as_deref() == Some("Local")`); or `None`. Violates the project's own
"Domain types over `String`" convention. Minimum: fix the doc. Better:
`enum RowSource { Local, Entry(String), Unattributed }`, which also removes the
`"Local"` half of WP-2's Block by construction.

**Warn — `display_split` derives a sourced row's path from the longest locator
across the whole configured set** (`tree.rs:612-641`). With `oci = "ghcr.io"` and
`oci = "ghcr.io/acme"` both configured, `ghcr.io/acme/platform/foo` renders under
root `ghcr.io` as `platform/foo` — the `acme` segment removed because an
*unrelated* entry declares it — and collides on screen with a genuine
`ghcr.io/platform/foo`. Two repositories, one displayed identity.
`docs/src/stability.md:129` excludes TUI appearance from the freeze, so this is
fixable now. Attribute against the producing entry's own locator in the
`Some(source)` arm; the entry key is already in hand at the call site.

**Warn — the `registry_locators` plumbing is unpinned outside the tree.**
Dropping it from `render.rs:764`'s `configured`, or making `app.rs:3006` return
`Vec::new()`, both leave the suite green. Since `registry_order` now carries
*aliases*, either mutation leaves the flat list with no real locators, so
`display_split` finds no prefix and the Registry column shows a bare host with an
un-shortened Repo cell — the exact failure `render.rs:761-763`'s own comment
warns about. `tree.rs:610` claims the two renderers "attribute identically";
nothing asserts that for the new field.

**Warn — `registry_order` + `registry_locators` are two parallel vectors
documented as parallel with nothing enforcing it** (`tree.rs:41-51`,
`state.rs:322-325`). Collapse into a local `RegistrySource { key, locator }` in
`tree.rs` — **not** `ResolvedRegistry`, which would drag `config` and `globset`
into a module whose doc pins it as a pure transform (`tree.rs:11-12`).
`app.rs`'s own `RegistryDisplay` (`:2950-2985`) is the precedent, introduced in
this same commit.

**Suggest —** the flat list's Registry column is not width-budgeted for
`alias (locator)` labels and overruns by a per-row amount. Likely pre-existing,
but `3e58d8d` makes long labels far more common (two views produce two labels
both carrying the same long locator).

**Suggest —** the root label disambiguates on the half that is identical.
`registry_labels` builds `"{alias} ({url})"`, and the change exists precisely for
the case where two entries share one locator — where `({url})` carries zero
discriminating information. Two roots read `house (ghcr.io/acme)` and
`global-acme (ghcr.io/acme)`; scope is the only difference and it is not in the
label. ADR D7 (a derived root label) was withdrawn when one locator meant one
root — the owner may want to reopen it; not a decision to take against a frozen
ADR.

### WP-5 — `src/command/config.rs` (the verb itself)

**BLOCK — surviving mutant: the include side of patch semantics has no witness.**
`if !include.is_empty() {` → `{` at `:1468` passes the whole Rust unit suite, the
new 299-line acceptance file, and clippy, while silently destroying the entry's
`include` list on a `--default`-only edit. Proven against the real binary:

```
--- after add ---
{"alias":"acme","oci":"ghcr.io/acme","include":["acme/platform/**"],
 "exclude":["acme/legacy/**"],"default":false}
--- after 'registry set acme --default' (MUTANT) ---
{"alias":"acme","oci":"ghcr.io/acme","include":[],
 "exclude":["acme/legacy/**"],"default":true}
```

The asymmetry with the exclude side is the tell: every patch-semantics test
(`registry_set_leaves_every_field_it_was_not_given`,
`test_registry_set_leaves_fields_it_was_not_given`) names **only `--include`** as
the given flag and asserts the *other* fields survive.
`test_registry_set_reports_registry_set_action`
(`test/tests/test_config_registry.py:1443`) performs the exact corrupting call —
a `--oci`-only `set` on an entry carrying `include = ["a/**"]` — and asserts
nothing about the list. Cheapest sufficient fix: extend that test with
`assert shown["include"] == ["a/**"]`. Proper fix: the mirror test — seed with
both lists, edit each *other* field in turn, assert `include` is unchanged.

**NEW (decision 9) — add `--clear-include` / `--clear-exclude`.** Each
`conflicts_with` its positive twin (exit 64 if both given). Three implementation
notes:
- `:1413`'s "nothing to change" guard must count the clear flags, or
  `registry set acme --clear-include` wrongly exits 64.
- The clear must be visible in the write report — folds into the report finding
  below.
- Mutation gate: clear-flag absent leaves the list; present empties it; both
  flags together exit 64. All three must go red under a single-token change.

**BLOCK — `grim init --registry` writes an entry the new verb can never address,
and the error steers the user into duplicate rows.** `grim init --registry
ghcr.io/acme` writes an **alias-less** `[[registries]]` entry with no CLI handle:

| Route | Result |
|---|---|
| `config registry set --include …` | 64 — `<ALIAS>` is a required positional |
| `config set registry.include …` | 64 — "without a field" |
| `config set registry..include …` | 64 — unknown key |
| `config registry list` | renders a **blank** Alias cell, no hint |

`registry set`'s missing-alias message then says *"add it with `grim config
registry add`"*. Following it creates a **second** entry at the same locator, so
`grim search` shows 5 rows for 4 packages: the newly filtered view **appended
to** the still-unfiltered one. The filter the user just wrote hides nothing, and
C-019 stays silent because the filter did admit 1 of 4.

**Remediation (decision 5):** `init --registry` seeds an alias. Also change the
message at `:1441-1444` so it does not recommend `registry add` when an entry at
the same locator already exists — name the locator collision instead.

**HIGH — `registry set`'s locator guards are entirely untested.** Deleting both
`reject_control_chars` and the `classify_index` check (`:1420-1432`) leaves the
suite green. `add`'s equivalents are covered at both layers; `set`'s at neither.
Security-adjacent —
`test_load_registry_hostile_locator_exits_78_without_raw_escape` exists precisely
because a control character in a locator injects a terminal escape into every
later config read, and `set` is now a second write path to that field.

**HIGH — `registry set`'s output describes nothing.** `value` is populated only
for a locator change, so a filter edit, an exclude edit and a `--default` change
print three identical rows with an empty Value cell. Measured across every write
verb, `registry set` is the only one where one `action` covers four distinct
mutations, `key` never descends to the field, and `value` is *conditional*:

```
$ grim config registry set acme --oci ghcr.io/moved --include 'a/**' --include 'b/**' \
      --exclude 'c/**' --default --format json
{ "action":"registry-set", "key":"registry.acme", "value":"ghcr.io/moved", ... }
```

Four of five mutations invisible. `subsystem-cli-api.md:56-62` ("report what
happened, not echo input") is not met — `value` is literally the `--oci`
argument echoed back. The plain table is worse than the JSON: a blank `Value`
cell reads as "the value is now empty". **Remediation:** when exactly one field
changed, set `key` to the field written and `value` to the rendered list;
otherwise add an always-present `fields` array (additive, keeps
`ConfigWriteReport`'s shape frozen). Must cover the new clear flags.

**HIGH — `--include ''` errors without the next command the sibling path prints.**

```
$ grim config registry set acme --include ''
invalid value for registry.acme.include: '' must not be empty or whitespace-only   [65]

$ grim config set registry.acme.include ''
invalid value for registry.acme.include: must not be empty or whitespace-only;
clear the filter with `grim config unset registry.acme.include`                    [65]
```

`--include ''` is the most likely attempt at clearing a filter. The string
already exists at `:540-543`, and `registry set` with *no* flags at all
(`:1410-1418`) does print the unset route — only the empty-pattern path lacks
it. Reject empty/whitespace in `check_filter_flags` with the
`check_set_filter_pattern` wording. **Under decision 9, also name
`--clear-include` here** — it is the better answer for this user.

**HIGH — an alias that exists only in the other scope gets a message that
creates a duplicate.** `grim config registry set onlyglobal --include 'acme/**'`
→ `no registry 'onlyglobal'; add it with 'grim config registry add'` [64]. The
entry exists and is one `--global` away; the message never mentions scope.
Probe the other scope before erroring and say so. `registry show` prints the
identical string, so the fix belongs to both, but `set`'s site is the one this
diff adds.

**Warn — CWE-117: the aggregate-budget branch echoes the alias unescaped**
(`:571`), twelve lines below a sibling branch that escapes it and documents why
(`check_filter_pattern:494-498`: *"the KEY is escaped too … `parse_key`'s
control-char screen is false for U+202E"*). Reachable exactly when every
individual pattern passed, so the escaping branch never runs. Reproduced with an
alias carrying U+202E and 100 × 1024-byte patterns — raw bidi override on
stderr. One expression: `key.escape_debug()`.

**Warn — CWE-367: `run_registry_set` commits a pre-lock snapshot.** `:1434` reads
the file, `:1447` compiles the globs, `:1449` takes the lock, `:1451` clones the
**pre-lock** snapshot, `:1475` writes it whole. Measured: one `registry set` with
both lists at the 64 KiB budget spends **0.28 s / 60 MB RSS**, mostly pre-lock —
a syscall-width race becomes one a plain shell hits first try. Reproduced: a
concurrent `config set` was silently discarded, both processes exiting 0.
Pre-existing across `add`/`rm`/`set`, but patch semantics make the fix trivially
correct **here**: after `acquire_config_lock`, re-resolve and apply the patch to
the fresh snapshot. Keep `check_filter_flags` pre-lock — the S-016 ordering is
right; only the *snapshot* must be re-taken.

**Warn — `--oci ''` blames the config file; `--index ''` names the flag.**
`--oci ''` → exit **78** naming `grimoire.toml`; `--index ''` → exit **65**
naming the flag, from the same function. This is the exact class
`check_filter_flags`'s own comment (`:561-569`) says the aggregate budget check
was added to avoid. Add an empty/whitespace guard beside the `classify_index`
check, exit 65, name `--oci`.

**Warn — the `WriteSite` enum's own doc and the bare-comma remedy are falsified
by this branch.** `:446-452` documents `WriteSite::Add` as "reachable only while
it does not exist", but `run_registry_set` (`:1447`) calls it on an entry that
must exist. `:507-515`'s `WriteSite::Set` warning omits a repeated-flag remedy,
justified in-code by "naming it would close a loop with no exit" — this branch
created the exit.

**Suggest — `registry set` replaces a multi-pattern list silently** where
`config set` warns (`warn_on_discarded_patterns`, `:584-598`, called only from
`apply_set`). Judged defensible: replacement is the verb's documented contract,
so silence is not wrong the way `config set`'s was. The actionable residue is
that the report leaves no record of what was lost — covered by the report
finding above. If a warning is added anyway, gate it so a caller re-passing the
stored patterns does not warn, and give the message a remedy variant ("use
`registry set`" is not a remedy when you are already inside it).

**Suggest —** the locator checks (`:1420-1432`) run *before* the alias-existence
check (`:1440`), contradicting the inline comment's "mirroring `add`'s
duplicate-alias ordering": `registry set ghost --index 'not-a-locator'` exits 65,
not 64. `test_registry_set_missing_alias_exits_64` only exercises `--include`.
Move the alias check up, or drop the clause from the comment.

**Suggest —** the duplicate-alias error (`:1327-1334`) is a 389-byte run-on
carrying four instructions where the first sentence is the whole answer.
`registry set <missing>`'s remedy prints `grim config registry add`, which exits
64 as spelled — needs `add <alias> --oci <ref>`.

### WP-6 — `src/command/search.rs`

**BLOCK (decision 4) — `grim search` prints the same repository once per view
with nothing to tell them apart.** `3e58d8d` makes two entries at one locator two
browsing views. Every browse surface got a discriminator except the one users hit
first: the flat table has no registry column, so overlapping views emit
byte-identical rows. One global + one project alias — the *ordinary* two-scope
shape — yields every package twice; three aliases yield 12 JSON items for 4
distinct packages, byte-identical objects.

`--format json` is worse: `SearchEntry` (`:159-172`) has no `alias` / `source`
field, so two entries are identical in **every** key including `repo`. The
sibling `grimoire-vscode` extension keys catalog cards on `repo` and currently
de-duplicates them, which silently destroys the second view's contribution — the
exact thing `3e58d8d` exists to stop. **This is the fix that unblocks the
extension.**

`CatalogGroup` already carries `alias`; `into_flat_rows`
(`catalog_service.rs:205-214`) discards it. Add it to `SearchEntry` and render a
leading Registry column under the same gate the TUI flat list uses. **Do not
dedupe by `repo`** — owner decision 4: the JSON should show that multiple
registries are configured.

### WP-7 — records and published surfaces (do this LAST, from shipped behaviour)

Write these from the **source**, not from the plan — on the previous round, docs
written from a drifted plan is how two shipped documents came to assert a safety
property the code did not have. Note that decision 2 changes the rule again, so
this WP must be written after WP-1 lands.

**BLOCK — the published JSON Schema still states the deleted rule.**
`src/config/declaration.rs:276-282` (`include`'s third paragraph) and `:294-298`
(`exclude`'s remedy paragraph) are emitted verbatim as the field `description` by
`grim schema --kind config` (schemars lifts `///`). This is a shipped output
consumed by editors, not an internal record. It says patterns are "relative to
this entry's own `oci`/`index` locator, never the fully-qualified ref" — both
halves false — and still names the `rm` + re-`add` round-trip this branch
replaced. The parity gate `assert_description_prefix`
(`config_keys.rs:699-710`) only checks the **first** paragraph, which is why this
passed.

**BLOCK — `catalog/skills/grim-usage/references/registries.md`**, a published
catalog artifact gated by `task catalog:verify`, and agent-facing so it actively
teaches the deleted rule. Lines 99-100 (dedup rule), 188-194 (an example that now
matches nothing, with prose stating the exact inverse of the new rule), 214-217,
252-268 (all three sentences false), 271 (the superseded warning string), 306-317,
330-337, and **376-383, where the `config registry` verb list omits `set`
entirely**.

**Warn — `grim config registry fields` never states the match rule and names only
`grim config set`** (`config_keys.rs:246-271`). Different surface, and the schema
fix does not reach it *because* the gate is prefix-only. This is the surface a
GUI builds its field help from — the `grimoire-vscode` settings panel reads
exactly this.

**Both are fixed by one edit:** put the match rule and both write verbs in the
**first** paragraph of `config_keys.rs`'s `INCLUDE`/`EXCLUDE` descriptions and
mirror that opening paragraph into `declaration.rs`, so the prefix gate stays
green. That single change fixes `registry fields` (plain + JSON), the published
schema, and `config list --format json`'s `description` at once.

**HIGH — agent-facing rule drift.** Two files were never touched by this diff and
will teach the deleted rule to the next agent that reads them:
- `.claude/rules/arch-principles.md:77` — the ADR-index row for
  `adr_registry_browse_filters.md` states "the match candidate is the row's path
  relative to the *declaring entry's own* url (index sources match the
  fully-qualified ref)". Both halves wrong. **This file auto-loads on every
  `src/**` edit.**
- `.claude/rules/subsystem-cli-commands.md:27` — pins the C-019 warning string
  verbatim; the string changed. Its `config registry` row lists
  `add|rm|use|show|list|fields` with `set` missing, and its `--include` prose
  still describes locator-relative anchoring.

**HIGH — design records.** Both need a **new dated amendment**, not an in-place
rewrite — surrounding amendments cite the old text:
- `.agents/adr/adr_registry_browse_filters.md` — D3 (heading, formula, candidate
  table rows 2-3, the 2026-08-09 owner amendment, the `display_split` equality
  claim, the index-source paragraph, the orphaned 2026-08-10 amendment), the
  Alternatives-Considered entries at `:611-618` (both the FQ and the
  `repository`-alone entries are superseded by decision 2), plus L505-506,
  L533-535, L547-548, L568-574, L655-661, L763-765.
- `.agents/plans/plan_registry_browse_filters.md` — C-005, C-013, C-018, C-019
  and their worked examples; L326-328, L349-350, L364-365, L380-389, L391-400
  (a wave-2 correction that is itself now wrong — record it as *reverted*, not
  stale), L467-470, L508, L524-537, L555-564, L649-654, L718, L729-731, L744-745,
  L1103, L1216-1219. **L1184's "Additive-only throughout" row stands** under
  decision 1.
- `.agents/adr/adr_registry_default_dedup.md:45` — "deduped by url" is wrong; the
  released key was locator-only and HEAD's is `(locator, alias)`.

**Warn — `docs/src/commands.md:209`** — the JSON-shapes table lists
`set`/`unset`/`registry add, rm, use` sharing one write-confirmation shape but
omits `registry set`, even though it emits the identical struct and the sibling
enumeration 17 lines below (`:226`, same diff) was updated to include
`registry-set`.

**Warn — the anchoring rule is the inverse of its nearest precedents.** grim's
wildcard-free pattern is anchored at the root (`hex` needs `**/hex` to match
anywhere); gitignore's and Cargo's bare-pattern default is the opposite
(`foo` ≡ `**/foo`, `/foo` anchors). `docs/src/configuration.md` already flags
"this is the shape that surprises people" but never says *why*, and cites Cargo
two sections earlier for a different (precedence) divergence — inviting the wrong
generalization. Add one sentence naming the inversion explicitly.

**Doc (decision 8) —** one line under `#browse-filters`: an exclude-only filter
has no zero-match diagnostic in either direction, by design.

**Suggest —** `src/api/config_report.rs:184-185`, `value`'s field doc not
extended for the new verb (`json-interface.md:222` states it correctly).
`grim context` renders a missing alias as `-` while `config registry list`
renders a blank cell — pick one.

**Suggest —** `test_exclude_that_removes_nothing_stays_silent`
(`test/tests/test_registries.py:1355`) is now **correct** under decision 8; only
its docstring still argues from the superseded C-005 semantic. Docstring fix
only.

### WP-8 — `/finalize` (do this last, after every WP has merged)

- Drop the `!` and the `BREAKING CHANGE:` footer from `f790273` (decision 1).
- Correct `3e58d8d`'s body: the mechanism it describes ("dedup now runs between
  the two files") is not what shipped — `1ed73aa` replaced it 21 minutes later
  with `(locator, alias)` on a single chained loop. Its "redundant walk is gone"
  claim also overstates (case-variant locators still walk twice).
- Do **not** split the branch (decision 3). A rename is optional.

---

## Deferred to the owner — still open

1. **ADR D7 (a derived root label).** Withdrawn when one locator meant one root;
   two roots per locator may reopen it, at least enough to put scope or
   filter-state in the label. Pinned by
   `registry_labels_are_unchanged_by_a_configured_filter_s018`.
2. **`registry set --format json`: one row with a `fields` array, or N rows
   matching `config set`?** The first is additive-safe; the second is the more
   consistent surface and is still cheap while the branch is unpushed.
3. **`registry use` vs `registry set --default`** — same write, two `action`
   strings. Decision 6 keeps both verbs; aligning the string is free only
   because `registry-set` is unreleased.

## Frozen — report divergence as information, do not relitigate

- The candidate is matched **dual** (decision 2): both `repository` and
  `{registry}/{repository}`, hit on either. Locator-relative matching stays
  rejected; the entry's own locator is never an input.
- A wildcard-free pattern auto-expands **downward only** (`hex` → `hex{,/**}`).
- Two entries at one locator under two aliases are two views and **both browse**.
  "Make it a config error" and "gate it on the filters differing" were both
  rejected.
- `registry set` uses **patch semantics**. Clearing rides the new
  `--clear-include` / `--clear-exclude` flags (decision 9); `config unset`
  remains and is unchanged.
- Flag values pass as `--flag=value`, one flag per pattern, never comma-joined.
- Read-time-only filtering: the filter never touches resolution, locking, or
  install. Independently re-verified this round — one enforcement site
  (`catalog_service.rs:341`), `Complete` unfiltered by a *total* match, and an
  excluded reference resolves byte-identically to a visible one.
- Include/exclude precedence: include-then-subtract, exclude wins, empty include
  skips the check.
- The C-019 predicate (fires only on an unqueried browse whose non-empty
  `include` admitted nothing) is unchanged — decision 8 documents the
  exclude-only gap rather than closing it.

## Withdrawn on evidence — do not re-file

- **The alias/path-attribution collision in `tree.rs`** (an alias out-ranking a
  real locator in `attributed()`'s longest-prefix match). Its worked example
  needs an alias containing `/`; that is rejected at config load
  (`project_config.rs:288`, exit 78) and at the CLI (`config.rs:1117`, exit 64).
  An alias without `/` can only match by equalling the whole host, which yields
  the identical split. The `TreeBuildOptions` doc states the right conclusion for
  a slightly wrong reason and could be reworded. *(Note: the root-key collision
  in WP-2 is a different, real defect — `source_key`'s `BTreeMap`, not
  `attributed`'s prefix match.)*
- **Comment loss on config round-trip.** Hand-verified: entry position, both
  locators, `[options].clients`, `default_registry` and both declaration tables
  all survive a `registry set`; only the leading comment and `[options]` key
  order move, which is `write_config`'s documented lossy re-serialize and
  identical to every other `config set`. Pre-existing, out of diff scope.
- **The constitution violation.** Deleted by decision 1 — no released surface.

## Method notes for whoever runs the fix loop

- `task --force verify` is the only trustworthy gate. Plain `task verify` prints
  "up to date" and exits 0 from the Taskfile cache without running a test.
- The shell wraps `git`/`grep` through `rtk`, which **filters** output. Prefix
  with `rtk proxy` whenever you need the real thing (`rtk proxy git diff
  main...HEAD`). A plain `git diff` gave a 527-line summary of a 3191-line diff.
- Ask every builder the mutation question — *"what single-token mutation would
  make this wrong, and does a test fail on it?"* Across the two reviews, 37
  mutations were run and **8 survived**, including one that reverts a whole
  commit with the suite green. Reasoned mutation checks are not a substitute;
  apply them to real source, run them, revert them.
- Run merges and gates from the main checkout with `git -C`, never from inside a
  worktree you are about to remove.
- Each worktree needs `git submodule update --init --recursive` after
  `git worktree add`, or the build cannot resolve `external/*`.
- The commit hook wants its marker in the **worktree**, not the lead:
  `<worktree>/.claude/hooks/.state/commit-verified`; `mkdir -p` first.
