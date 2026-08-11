# ADR: The browse-filter match candidate — dual-candidate matching

## Metadata

**Status:** Accepted
**Date:** 2026-08-11
**Deciders:** Michael Herwig (maintainer)
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md`
      (Rust 2024; no new dependency — `globset` 0.4.20 is already pinned and
      the change is two extra `is_match_candidate` calls per row)
**Domain Tags:** api, integration, tui
**Supersedes:**
[`adr_registry_browse_filters.md`](./adr_registry_browse_filters.md) **§ D3's
Decision text** ("The match candidate is the row's source-relative path") **and
its first amendment block only** (2026-08-09, the `display_split` equality
claim), plus two of its *Alternatives Considered* entries — "Match against the
fully-qualified `registry/repository`" and "Match against
`CatalogRow::repository` alone".

**Explicitly carried forward, not superseded** — D3's two later amendment
blocks are not about the match candidate and remain live:

- *2026-08-10 (wave-1, S-6)*, `adr_registry_browse_filters.md:166-180` —
  `trim_locator` trims trailing slashes at every site constructing
  `ResolvedRegistry.url`. Still load-bearing: it is the precondition under
  which `key()` and every display path see one canonical locator form.
- *2026-08-10 (round-3, S-B)*, `:182-198` — `classify_index` has eight call
  sites and only the two inside `resolve_registries` receive a trimmed
  argument, so `index = "https://host/repo.git/"` classifies `IndexHttp` at
  validation and `IndexGit` at resolution. A recorded, unfixed
  transport-classification defect with no relationship to the candidate rule.
  This ADR is not its home and must not retire it.

**D1, D2, D4–D13 stand unchanged in mechanism.** One knock-on on D2's *text*:
it reads "the candidate matches at least one include pattern" — singular —
and after this decision there are two candidates. The migration's pointer step
lands beside D2 as well as D3 for exactly that reason.
**Research artifacts:**
[`research_registry_filter_candidate.md`](../research/research_registry_filter_candidate.md)
(technology + domain axes) and the design-patterns axis persisted with this
decision (Renovate, Gatekeeper, `containers/image`, OCI `<name>` grammar).

## Context

`grim`'s per-registry browse filter narrows what `grim search`, the TUI, and
the MCP `grim_search` show. It is applied at exactly one production site,
`src/catalog/catalog_service.rs:341`:

```rust
CatalogScope::Browse => reg.filter.matches(&browse_candidate(&e.repository)),
```

`browse_candidate` (`src/config/registry_filter.rs:496-498`) is today the
identity function, so a pattern is matched against the **bare repository
path** and nothing else. The registry host is discarded before the matcher
runs.

That is a deliberate choice, taken on this branch in `f790273` to fix a real
defect — a locator edit could silently re-aim every pattern written against
it — and it must not be undone. What it cost is host precision:

- A `CatalogEntry` carries `registry` and `repository` as separate fields
  (`registry_catalog.rs:135,137`), and the index catalog keys rows by the
  fully-qualified `repo()` (`registry_catalog.rs:641` → `:201-203`), so one
  index source genuinely serves `ghcr.io/acme/tools` **and**
  `quay.io/acme/tools` as two distinct rows.
- Both rows produce the identical candidate `acme/tools`. `exclude =
  ["acme/tools"]` hides both, and **no pattern exists that hides one and
  keeps the other**.
- `adr_registry_browse_filters.md:615-618` had already rejected this exact
  shape on this exact ground, and the built-in fallback browse source
  (`FALLBACK_INDEX = https://index.grimoire.rs`, `src/command.rs:254`) *is*
  an index. `product-context.md` names self-hosted multi-host indexes as a
  primary adoption path, not a corner case.

The owner has decided (handover decision 2, 2026-08-11) that each pattern is
matched against **both** `repository` and `{registry}/{repository}`, admitting
or excluding on a hit against either, identically for `oci` and `index`
sources with no per-kind branch.

That sentence is ambiguous between two implementations that return **different
answers on a real input**. This ADR resolves the ambiguity, records why the
surveyed alternatives were not taken, and freezes the resulting semantics.

## Decision Drivers

- **Host precision must become expressible** — the capability the current rule
  cannot express at all, on the source kind the product leads with.
- **A bare pattern stays host-agnostic** — owner-stated; every pattern written
  for the shipped rule keeps working, and `acme/tools` must keep meaning
  "on every host".
- **One rule for `oci` and `index`, no per-kind branch** — owner-stated,
  explicitly. It is what made the shipped rule an improvement over the
  locator-relative one it replaced.
- **The entry's own locator is never an input** — everything `f790273` bought
  is preserved; a locator edit must not re-aim a pattern.
- **Not a security boundary** — an excluded package stays fully resolvable,
  installable and describable by name (`adr_registry_browse_filters.md`
  Decision Drivers; re-verified this round: one enforcement site, `Complete`
  unfiltered, an excluded reference resolves byte-identically).
- **Minimum frozen surface** — the config schema, the CLI flag set, and the
  glob dialect all freeze at 1.0. A candidate rule that needs no new key, no
  new flag and no new dialect token is worth a lot.
- **One-sentence explainability** — a config surface a user cannot restate
  correctly is a config surface that gets used wrong.
- **Boring technology** — no bespoke matcher, no host-detection heuristic.

## Industry Context & Research

**Key insight, and it cuts both ways.** No surveyed tool — Kubernetes
selectors, Prometheus `relabel_config`, gitattributes, cosign, Trivy/Grype,
Kyverno, Gatekeeper, Docker/containerd, Homebrew, Go modules, Nix, Renovate —
implements "one pattern, OR-matched against two full alternate string forms of
one object, admit on either" as an intentional design. The two real ecosystem
answers to "one short identifier, several candidate hosts" are **eliminate the
ambiguous form** (Go module paths always carry the host; Kubernetes 1.34 /
CRI-O `short-name-mode: enforcing`) or **resolve it once via an explicit
mapping before matching** (Homebrew's `user/repo/formula` qualifier;
`containers/image`'s `short-name-aliases.conf`). Both are *resolve once, match
once*. Dual-candidate resolves nothing and matches twice.

Four findings decide this ADR:

1. **Host-detection-by-inspection has a live, filed bug in Docker's own
   implementation.** `containers/image` treats a first path segment as a host
   if it contains a dot or colon or is `localhost`;
   [containers/image#775](https://github.com/containers/image/issues/775) is
   that heuristic misfiring in production. This is primary evidence for the
   rejection decision 2 already asserts, not merely an assertion.
2. **The collision is spec-legal, not hypothetical.** The OCI distribution
   spec's `<name>` grammar
   (`[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*`,
   fetched from the raw spec 2026-08-11) permits dots inside a segment and
   forbids colons. `acme.corp/tools` is legal; so is a repository literally
   named `ghcr.io/foo` hosted on `quay.io`.
3. **An over-matching *allow* rule is the more dangerous direction — in
   access-control tools.** Gatekeeper's own `K8sAllowedRepos` docs warn that
   `docker.io` without a trailing `/` admits `docker.io-evil/malicious-image`;
   Red Hat's 2020 `rhel7/etcd` short-name squatting attack is the same class,
   exploited. Saltzer & Schroeder's fail-safe-defaults is the general ancestor.
   Neither is a browse filter.
4. **Renovate hit grim's shape of problem and fixed it by naming the
   candidate.** `matchPackageNames` had been matching `depName` all along —
   two candidate strings for one entity under one field name. The resolution
   was an explicit second field (`matchDepNames`), **not** OR-ing both
   ([renovatebot/renovate#20213](https://github.com/renovatebot/renovate/discussions/20213)).
   This is the strongest single precedent against the chosen option and is
   answered in full below.

**Deferred finding for the owner, stated once.** The research does not
undermine the decision, but it does narrow the claim that can honestly be made
for it: dual-candidate matching is an unprecedented fourth shape, and the two
precedented shapes both eliminate the ambiguity rather than absorbing it. The
decision is defensible because grim's filter is a *view narrowing over rows*,
not a *reference resolution* — nothing downstream consumes the match verdict,
so the class of harm that made ambiguity dangerous in Podman and Gatekeeper
does not transfer. What does transfer is Renovate's lesson about
documentation: a rule that matches two strings must say *which two* in the
first sentence a reader hits, or it becomes a rule whose name outlives an
accurate description of what it matches. That obligation is a contract in the
design record, not a footnote.

**Second deferred finding — one open question for the owner.** Option 6b below
(include matches `bare` only, exclude matches either) is fail-safe on the allow
side, breaks no authored pattern, adds no frozen surface, and keeps the
fq-exclude remedy intact. Its only cost is S-002: a host-qualified *include*
stops selecting a single host. The decision as taken implies host-qualified
include is a required capability. **If host-qualified *exclude* alone is
sufficient for the multi-host-index adoption path, 6b is strictly better than
Option 4 and the false-positive class shrinks to the exclude side — the benign
direction.** Answering "exclude alone is enough" is the one input that would
reopen this ADR; nothing else in the review round does.

## Considered Options

Shorthand used throughout, for a row with `registry = R`, `repository = P`:

```
bare = P                    // "acme/tools"
fq   = "{R}/{P}"            // "quay.io/acme/tools"
```

### Option 1: Status quo — bare candidate only

**Description:** keep `browse_candidate` the identity function. Ship the
branch as-is.

| Pros | Cons |
|------|------|
| Zero change, zero risk, zero test churn | Host precision remains inexpressible on the product's lead discovery surface |
| One candidate, trivially explainable | The ADR it implements already rejected this shape by name (`:615-618`) |
| | Two rows of a multi-host index are permanently indistinguishable to a filter |

### Option 2: Fully-qualified always (the Go / normalization shape)

**Description:** `candidate = fq`, one candidate, uniform across source kinds.
Every pattern carries the host.

| Pros | Cons |
|------|------|
| Unambiguous by construction — the collision class disappears entirely | Deletes the host-agnostic bare pattern the owner requires: `acme/tools` stops matching anything |
| Precedented (Go module paths; K8s 1.34 enforcing mode) | Every already-authored pattern must be rewritten |
| One candidate, one rule, no per-kind branch | Forces `ghcr.io/` into every pattern in an entry whose next line already reads `oci = "ghcr.io"` — the exact redundancy `f790273` removed |

### Option 3: Dual-candidate, naive top-level OR ("Option A")

**Description:** call the existing single-candidate `matches()` once per
candidate and OR the two verdicts. Implementable as a wrapper at the call
site, with **no change to `RegistryFilter`'s signature**.

```rust
matches(bare) || matches(fq)
```

| Pros | Cons |
|------|------|
| Smallest possible diff — one call site, no signature change, no test churn | **Does not implement the owner's own worked example** (proof below) |
| Reads like the decision's sentence, literally | A host-qualified exclude cannot carve one host out of a bare-pattern include — the single capability the change exists to add |
| | The failure is silent: the config is valid, exit 0, and the row the user excluded is on screen |

### Option 4: Dual-candidate, per-list OR with one exclude-wins ("Option B") — **chosen**

**Description:** each *list* is tested against each candidate; the
include-then-subtract precedence is applied **once** to the combined verdicts.
Requires `RegistryFilter::matches` to take both strings.

```rust
include_hit = include_is_empty || include ~ bare || include ~ fq
exclude_hit = exclude ~ bare || exclude ~ fq
visible     = include_hit && !exclude_hit
```

| Pros | Cons |
|------|------|
| Satisfies the owner's worked example; a host-qualified exclude carves exactly one host out | Signature change reaches 24 textual `.matches(…)` call sites across 4 files (23 of them tests) |
| No new config key, no new CLI flag, no new dialect token — zero added frozen surface | An FQ-shaped pattern can false-positive against a dotted repository segment (bounded below) |
| Every already-authored bare pattern keeps working, host-agnostic | Unprecedented shape; nothing in the ecosystem to point at |
| Identical for `oci` and `index`, no per-kind branch | A previously-dead FQ-shaped pattern comes alive (no user has one — unreleased) |
| Matching cost doubles from 2 to 4 `is_match_candidate` calls per row — bounded, no new allocation class | |

### Option 5: Explicit candidate by name (the Renovate shape)

**Description:** keep one candidate per list and let the author say which.
Two concrete spellings:

- **5a — distinct fields:** `include` / `exclude` match `bare`;
  `include_qualified` / `exclude_qualified` match `fq`.
- **5b — a qualifier in the pattern grammar:** e.g.
  `include = ["registry:quay.io/acme/tools"]`.

| Pros | Cons |
|------|------|
| Eliminates the false-positive class completely — the author states which string is meant | **5a doubles a frozen surface**: 2 → 4 config keys (`RegistryField::ALL` is append-only with frozen positions), 4 → 8 CLI flags once decision 9's clear flags are counted, 4 `KeySpec` descriptions, 4 schema fields — plus a new precedence rule for how two include lists combine |
| Directly precedented, by a tool that reached it from grim's exact problem | **5b is a one-way door on the glob dialect** (`adr_registry_browse_filters.md` D4 pins three non-default globset knobs already); a colon sigil collides with `localhost:5000` |
| Additive over Option 4 — can still be added later if the ambiguity ever bites | Renovate's problem was an *accidental, invisible* conflation under a lying field name; grim's is intentional and statable in one sentence. The lesson transfers as a docs obligation, not a schema one |

### Option 6: Asymmetric strictness — the include side does not gain the second candidate

**Description:** the exclude list matches either candidate; the include list
does not. Two variants, and they are not equally strong — the record must
carry both, because only the second survives contact with the evidence.

**6a — include matches `fq` only.** The allow rule is normalized to the
canonical form.

| Pros | Cons |
|------|------|
| Unambiguous allow side | **This is Option 2 applied to one list**: a bare `acme/tools` include stops admitting anything, and every already-authored pattern must be rewritten. Fails the owner's requirement outright |

**6b — include matches `bare` only (the status quo); exclude matches either.**
The allow rule gains no new match surface at all; the deny rule gains the
qualified candidate.

| Pros | Cons |
|------|------|
| **Genuinely fail-safe on the allow side** — Saltzer & Schroeder applied literally, and the one direction Gatekeeper's `docker.io` / `docker.io-evil` bypass shows is dangerous | **Cannot express S-002** — a host-qualified *include* stops selecting a single host, so "show me only what is on `ghcr.io`" becomes inexpressible |
| Zero migration: it *is* today's include rule, so no authored pattern changes meaning | The owner's decision is explicit that a pattern is matched against both strings, "admitting **or** excluding on a hit against either" |
| Zero new frozen surface | The false-positive class shrinks but does not vanish — an over-matching *exclude* still hides a row via the qualified candidate |
| Keeps the fq-exclude remedy intact | Two mental models for one config pair, both frozen at 1.0 |
| Satisfies the owner's *worked example* as literally quoted (`quay.io/acme/tools` carving one host out of a bare include — that example is an **exclude**) | |

6b is the strongest rejected option in this ADR and is rejected on exactly one
ground: **it cannot express S-002.** Everything else about it is better than
or equal to Option 4. See "Why symmetric, not asymmetric" below for why that
one ground is decisive, and the Deferred finding for the question it leaves
open for the owner.

### Option 7: Resolve once via an explicit mapping (the Homebrew / `short-name-aliases.conf` shape)

**Description:** the second real ecosystem answer. Resolve a bare pattern to
one host before matching — via a qualifier in the reference grammar
(Homebrew's `user/repo/formula`) or an admin-authored alias table
(`containers/image`'s `short-name-aliases.conf` + `short-name-mode`).

| Pros | Cons |
|------|------|
| Precedented twice, in this exact domain; resolve-once/match-once is the discipline OWASP's canonicalize-before-validate guidance recommends | **Does not transplant.** grim's patterns filter *rows*; they do not resolve *references*. There is no "the" host to default a bare pattern to when one index aggregates several — the premise the alias table needs is absent |
| Eliminates the ambiguity rather than absorbing it | A qualifier in the pattern grammar is Option 5b under another name, and carries 5b's dialect one-way door |

A third normalization shape the research surfaced — require host-qualification
**only** inside a multi-host index, leave single-source registries alone — is
killed in one line by the owner's constraint: it is a per-kind branch, which is
exactly what the decision forbids. No surveyed tool does it either.

### Option 8: Host-detection-by-inspection (the Docker / `containers/image` shape)

**Description:** treat a leading path segment as a registry host when it looks
host-shaped (contains a dot or a colon, or is `localhost`), and derive one
canonical candidate from that.

| Pros | Cons |
|------|------|
| One candidate; matches how `docker pull nginx` becomes `docker.io/library/nginx` | **Pre-rejected by decision 2, and the rejection now has primary evidence.** A host cannot be identified by inspection — OCI namespace segments may carry dots (`acme.corp/tools`) and hosts need not (`localhost:5000`) |
| Decades of production use | [containers/image#775](https://github.com/containers/image/issues/775) is that heuristic misfiring, unresolved, in Docker's own reference implementation |

Recorded as an option block rather than left implicit, so the pre-rejection is
legible to the next reader instead of being a clause inside Option 6.

## Decision Outcome

**Chosen Option: Option 4 — dual-candidate, per-list OR, one exclude-wins.**
**Symmetric**: both lists match either candidate, on both sides.

### The proof that separates Option 3 from Option 4

Config:

```toml
include = ["acme/tools"]              # bare, host-generic
exclude = ["quay.io/acme/tools"]      # host-specific — the new capability
```

Row: `registry = "quay.io"`, `repository = "acme/tools"` → `bare =
"acme/tools"`, `fq = "quay.io/acme/tools"`.

Verified empirically against `globset` 0.4.20 with grim's own pinned
constructor (`empty_alternates(true) · literal_separator(true) ·
backslash_escape(true)`) and grim's own `expand_pattern`:

| Probe | Result |
|---|---|
| `include ~ bare` | `true` |
| `include ~ fq` | `false` — `acme/tools{,/**}` is anchored; no wildcard crosses the leading `quay.io/` |
| `exclude ~ bare` | `false` |
| `exclude ~ fq` | `true` |
| **Option 3 verdict** | `(true && !false) \|\| (false && !true)` = **`true` — row VISIBLE** |
| **Option 4 verdict** | `(true \|\| false) && !(false \|\| true)` = **`false` — row HIDDEN** |

Option 3 loses the capability precisely in the combined include+exclude case,
because it ORs *whole-filter verdicts* per candidate instead of ORing *per-list
hits* before applying exclude-wins once. The owner's own worked example
("`quay.io/acme/tools` matches via the fully-qualified candidate → that host
only. This is the host precision the rule currently cannot express") is only
satisfied by Option 4.

**Consequence for implementation:** the wrapper-at-the-call-site route is not
available. `RegistryFilter::matches` itself must take both strings. The
handover's WP-1 offers the two remediations as equal alternatives; they are
not.

### Frozen semantics

The four precedence combinations, restated as they freeze:

| `include` | `exclude` | Verdict |
|---|---|---|
| empty | empty | every row visible (unchanged) |
| non-empty | empty | visible iff some include pattern hits `bare` **or** `fq` |
| empty | non-empty | visible iff no exclude pattern hits `bare` **or** `fq` |
| non-empty | non-empty | visible iff (some include hits either candidate) **and** (no exclude hits either candidate) — an exclude hit on *one* candidate beats an include hit on the *other* |

Also frozen by this decision:

- `bare` is `repository` verbatim; `fq` is `{registry}/{repository}`.
- **When `registry` is empty the two candidates coincide** — `fq` is
  `repository` unchanged, so no candidate ever begins with `/`. A
  leading-slash candidate would match no authored pattern and fail silently,
  which is the failure class this ADR exists to remove.
- For every entry with a **non-empty** registry — which is every entry a
  catalog build produces — `fq` is byte-identical to `CatalogEntry::repo()`.
  The empty-registry carve-out is the one input where the two differ, and it
  belongs to the matcher, not to `repo()`: `repo()` feeds `grim search` JSON
  and the index catalog key (`registry_catalog.rs:641`), both frozen.
- The entry's own `oci`/`index` locator is **not** an input to either
  candidate. `f790273`'s guarantee is preserved verbatim.
- One rule for both source kinds. No per-kind branch exists or may be added.
- `--registry` still collapses the browse set and applies no filter (D9,
  unchanged).
- Read-time only. `CatalogScope::Complete` is never filtered (D5/D6,
  unchanged).

### Why symmetric, not asymmetric

The fail-safe-defaults argument is real, and Option 6b is a genuinely strong
form of it — fail-safe on the allow side, zero migration, zero new frozen
surface, and it keeps the fq-exclude remedy. It is rejected on one ground and
supported by two more:

1. **6b cannot express S-002.** A host-qualified *include* stops selecting a
   single host, so "show me only what is on `ghcr.io`" has no spelling. The
   owner's decision is explicit that a pattern is matched against both
   strings, admitting **or** excluding on a hit against either; 6b implements
   half of that sentence. This is the decisive ground, and it is the only one
   that holds against 6b. (Option 6a — include matches `fq` only — fails
   harder and for a different reason: it is Option 2 applied to one list.)
2. **The confidentiality premise does not hold.** Gatekeeper's bypass is
   dangerous because an admitted image *runs*. grim's browse filter is
   explicitly not access control: an over-matching include reveals one extra
   row in a listing of packages that were already fully resolvable,
   installable and describable by name. Nothing was hidden from anyone; the
   harm is a longer list, not a disclosure.
3. **Symmetry is what makes the safety valve work.** Exclude-wins is the
   strict-direction lever, and it only reaches a row admitted via the "wrong"
   candidate because exclude also matches either candidate. Making exclude
   strict would remove the only surgical remedy for an include false positive
   (see below). Making *include* strict removes the feature.

A rule where the two lists read differently also doubles the mental model on a
surface that freezes at 1.0, against an owner constraint whose spirit is
uniformity ("identical for `oci` and `index`, with no per-kind branch").

### Why not Renovate's explicit candidate (Option 5)

Renovate's fix was right for Renovate's problem, which was different in kind:
its two candidates were conflated **by accident**, under a field name
(`matchPackageNames`) that named the wrong one, so no user could have known
which string their rule matched. The remedy had to be a name.

grim's dual matching is intentional and statable in one sentence: *a pattern
is tested against both `acme/tools` and `ghcr.io/acme/tools`; a hit on either
counts.* What Renovate teaches, therefore, is a **documentation** obligation,
not a schema one — and that obligation is carried as a contract (the first
paragraph of every surface that describes the rule must name both candidate
strings; the parity gate `assert_description_prefix` only reads first
paragraphs, so anything stated later is invisible to it).

The schema answer (5a) costs 4 config keys, 8 CLI flags, 4 schema fields and a
new combination rule, all frozen at 1.0, to buy a capability nobody has yet
needed. `quality-core.md`'s YAGNI is dispositive: extract when the second
genuinely different use case appears. Decisively, **5a is purely additive over
Option 4** — a `*_qualified` field pair can be added in any later minor if the
ambiguity ever bites, and dual matching forecloses nothing. Option 5b, by
contrast, is a dialect one-way door and *is* foreclosed by shipping; it is
rejected permanently, not deferred.

### The false-positive class, bounded

Under dual matching, a pattern authored to mean "host `H`" can also hit the
**bare** candidate of a row on a different host `H'`, when that row's
repository path begins with segments spelling `H`.

Empirically confirmed with the pinned constructor: pattern `ghcr.io/**`
matches candidate `ghcr.io/foo` — which is the bare candidate of a repository
named `ghcr.io/foo` hosted on `quay.io`. (Its `fq` candidate,
`quay.io/ghcr.io/foo`, does **not** match — the collision arrives exclusively
through the bare candidate.)

Bounded from the spec and from the code:

- **Colon-bearing hosts are immune.** `<name>` forbids `:`, so no repository
  path can ever spell `localhost:5000` or any explicit-port host. A
  port-qualified pattern cannot false-positive at all.
- **Only lowercase, ASCII, dot/dash/underscore-spellable hosts can collide** —
  `<name>` forbids uppercase and every other character.
- **The collision needs one source serving both.** A filter is only ever
  applied to rows from its own `[[registries]]` entry
  (`catalog_service.rs:341`), so two entries never contend. In practice that
  means a multi-host index, or an OCI registry hosting a repository path that
  spells a different registry's name.
- **Direction of harm.** An include false-positive *reveals* one extra row in
  a browse listing; an exclude false-positive *hides* one. Neither reaches
  resolution, lock, install, or `status --check`.
- **This class already exists on the branch as shipped.** With the bare
  candidate as the only candidate, `include = ["ghcr.io/**"]` today already
  matches that same `quay.io` row. Dual matching does not create the
  collision; it makes host-qualified patterns *worth writing*, which raises
  exposure from "unreachable in practice" to "reachable whenever host
  precision is used".

**Verdict: a documented caveat, no mechanism.** A mitigation exists and is
surgical, which is why no engineered guard is warranted: the offending row is
removed by its fully-qualified form,

```toml
exclude = ["quay.io/ghcr.io/foo"]
```

which hits via the `fq` candidate and matches that row and nothing else (its
bare candidate `ghcr.io/foo` does not match a pattern beginning `quay.io/`).
This remedy exists **only because the exclude side is symmetric** — one more
reason not to make it strict.

### Quantified impact

| Metric | Before | After | Notes |
|--------|--------|-------|-------|
| `GlobSet` evaluations per browsed row | 2 | 4 | one `Candidate` per string; each compiled set evaluated twice. No new allocation class |
| Allocations per browsed row | 1 (`browse_candidate`'s `to_string`) | 1 (`format!` for `fq`; `bare` borrows) | net zero |
| Compile-time cost | unchanged | unchanged | `MAX_PATTERN_LIST_BYTES` (64 KiB) bounds compiled program size, not query-time candidate count. **Its meaning is unchanged** |
| Config keys / CLI flags / schema fields | 2 / 2 / 2 | 2 / 2 / 2 | zero added frozen surface |
| `.matches(…)` call sites needing a second argument | — | 24 textual across 4 files (23 tests, 1 production) | verified by grep; the "~30" in the discovery counts loop iterations, not call sites |

### Consequences

**Positive**

- Host precision becomes expressible, on the source kind the product leads
  with, with no infrastructure change and no new config surface.
- Every pattern written for the shipped rule keeps working, host-agnostic.
- `oci` and `index` entries agree, still with no per-kind branch.
- `f790273`'s guarantee — a locator edit cannot re-aim a pattern — is
  preserved exactly; neither candidate reads the entry's locator.
- The C-019 recovery sketch at `catalog_service.rs:452-457` ("probe whether
  the exclude patterns match the fully-qualified form … needs an exclude-only
  matcher that does not exist today") loses its premise: ordinary matching now
  does this unconditionally.

**Negative / risks**

- **Visibility is not monotone.** `include_hit` and `exclude_hit` both grow;
  exclude wins. A row's visibility can move in either direction relative to
  the shipped rule. Mitigated only by the feature being unreleased.
- **A previously-dead pattern comes alive.** An FQ-shaped pattern
  (`ghcr.io/**`) matched nothing meaningful before and now matches a whole
  host — as an include it widens, as an exclude it can empty a source. No user
  can hold one (unreleased), but the class is real and belongs in the docs.
- **The false-positive class above.** Bounded, documented, surgically
  remediable, no mechanism.
- **Semantics freeze on release**, alongside the dialect knobs D4 already
  froze. This decision adds one more one-way door: *which strings are
  matched*. Reversibility is asymmetric and in the safe direction — a third
  candidate or an explicit-candidate field pair (5a) can be added additively;
  removing a candidate cannot.
- **A larger test blast radius than the handover implies** — 24 call sites
  across `registry_filter.rs`, `registry_resolve.rs`, `search.rs`,
  `catalog_service.rs`, plus two `tree.rs` tests that call the candidate seam.
  One existing test (`browse_candidate_never_carries_the_registry_host`,
  `registry_filter.rs:1018-1029`) asserts the property this decision deletes
  and must be rewritten, not merely recompiled.
- **Two same-typed `&str` parameters in argument order.** `matches(registry,
  repository)` is swappable at a call site with no compile error.
  `quality-core.md` classes stringly-typed APIs Warn-tier. Accepted rather
  than newtyped (the candidates are ephemeral matcher inputs, not domain
  concepts with round-tripping needs — `arch-principles.md`'s "Domain types
  over `String`" does not bind), with an argument-order pinning test as the
  guard. See contract C-004 in the design record.

## Migration / rollout plan

**There is no user migration.** `src/config/registry_filter.rs` is absent from
every release tag `v0.9.0` … `v0.12.1` (verified by `git cat-file -e
<tag>:src/config/registry_filter.rs`), `RegistryConfig` at `v0.12.1` carries
no `include`/`exclude` fields, and `docs/src/configuration.md` at `v0.12.1`
has no `#browse-filters` anchor. The entire surface — config keys, CLI flags,
matcher, docs — is unreleased. No `docs/src/upgrading.md` section is required.

The migration is internal, and its **order is load-bearing**: every record of
the rule must be written from the landed code, not from the plan. On the
previous round, docs written from a drifted plan is how two shipped documents
came to assert a safety property the code did not have.

1. **Fixture first.** `seed_catalog` (`catalog_service.rs:639-665`) keys its
   JSON `entries` map on the bare `repository` (`:646`), so two hosts sharing
   one repository name collide and the second silently overwrites the first.
   *No fixture in the tree can express a two-host index.* Land the two-host
   fixture and the failing tests before the matcher.
2. **Matcher.** `registry_filter.rs` — signature, both candidates, per-list OR,
   the rewritten rule doc. Then the one production call site
   (`catalog_service.rs:341`).
3. **Call-site churn.** The 24 `.matches(…)` sites and the two `tree.rs`
   candidate-seam tests. Mechanical, but
   `browse_candidate_never_carries_the_registry_host` needs a rewritten
   assertion, not a recompile.
4. **Diagnostics.** `BROWSE_FILTER_REMEDY` and its doc block
   (`catalog_service.rs:405-482`) — re-derive, do not reword. Both
   `docs/src/configuration.md:491` and `docs/src/commands.md:743` carry that
   producer string verbatim and must move together.
5. **Live `--help`.** `src/command/config.rs:104-116` states the rule in the
   `--include` clap doc comment and is pinned verbatim by
   `registry_add_help_states_how_a_pattern_is_anchored` (`config.rs:3731-3761`).
   It fails the moment the rule changes, whichever work package "owns" the
   file.
6. **Parity-gated metadata.** `config_keys.rs` `INCLUDE`/`EXCLUDE`
   descriptions and the mirrored **first paragraph** of `declaration.rs`'s
   `include`/`exclude` doc comments. `assert_description_prefix`
   (`config_keys.rs:699-710`) is a prefix test — anything stated in a later
   paragraph is invisible to it, which is exactly how the superseded rule
   survived into the published schema.
7. **Docs.** `docs/src/configuration.md` (`:346-348`, `:448-449`, plus the
   `#browse-filters` section) and `docs/src/commands.md`.
8. **Catalog artifact.** `catalog/skills/grim-usage/references/registries.md`
   — agent-facing, gated by `task catalog:verify`.
9. **Agent-facing rules.** `.claude/rules/arch-principles.md:77` (the ADR-index
   row, which auto-loads on every `src/**/*.rs` edit) and
   `.claude/rules/subsystem-cli-commands.md:27`. Add an index row for **this**
   ADR in the same edit.
10. **Records.** A dated pointer amendment in
    `adr_registry_browse_filters.md` at **three** places — § D3's Decision
    text, § D2 (whose "the candidate" is singular and now names two), and
    beside the two superseded *Alternatives Considered* entries (`:611-618`).
    A pointer to this file, not a restatement, and it must leave D3's S-6 and
    S-B amendment blocks standing (see § Supersedes).
    `.agents/plans/plan_registry_browse_filters.md`'s C-005 / C-013 / C-018 /
    C-019 entries likewise.

Verification gate throughout: `task --force verify`. Plain `task verify`
prints "up to date" and exits 0 from the Taskfile cache without running a test.

## New ADR, not a fifth amendment

This is a **new ADR superseding D3's Decision text**, not a fifth amendment
block on it.

- D3 already carries four amendment blocks. A reader must currently apply four
  sequential corrections to learn what the rule is. A fifth — on a decision
  whose *Decision text itself* is now wrong, not merely imprecise — makes the
  record unreadable, and an unreadable record is how the drift this branch is
  fixing happened in the first place.
- Two of those four blocks (S-6, S-B) are about locator trimming and
  `classify_index`, not the candidate. Superseding D3 wholesale would retire a
  live, unfixed transport-classification defect that has no other home. The
  supersession is therefore scoped to the Decision text and the first
  amendment; § Supersedes names what is carried forward.
- The change supersedes more than D3's text: two entries in *Alternatives
  Considered* (`:611-618`) are now superseded outcomes rather than rejected
  options. An alternatives list cannot be amended in place without falsifying
  reasoning that neighbouring decisions cite.
- It is a different mechanism, not a correction to D3's mechanism: two
  candidates, per-list OR, one exclude-wins — with its own precedence table,
  its own frozen surface, and its own test obligations.
- The project's own ADR template says *one decision per ADR*, and the ADR set
  has supersession precedent (`adr_oci_empty_config_compat.md` supersedes
  `adr_oci_artifact_type.md`; both files retained and cross-linked).
- `arch-principles.md`'s ADR index carries one summary line per ADR. A new
  file gets a row stating the *current* rule, instead of forcing the existing
  row's prose to encode "the rule, except see amendment 5".

D1, D2, D4–D13 of `adr_registry_browse_filters.md` are untouched and remain
authoritative. D3 gets a dated pointer block only.

## Constitution check (`AGENTS.md` § Core Principles)

| # | Principle | Verdict |
|---|---|---|
| 1 | Understand First | **Pass** — the one production call site, all 24 test call sites, both producer types and the seam's doc block read before deciding; the Option 3/4 divergence proven empirically rather than argued |
| 2 | Prove It Works | **Pass** — the fixture gap is named as the first migration step; the combined include+exclude case has no existing test and gets one; mutation witnesses specified in the design record |
| 3 | Keep It Safe | **Pass** — the filter is not an access boundary (re-verified: one enforcement site, `Complete` unfiltered, an excluded reference resolves byte-identically). The one new risk class is bounded from the OCI spec, documented, and surgically remediable |
| 4 | Keep It Simple | **Pass** — no new config key, flag, dialect token or type. Option 5a's four-key expansion is rejected on exactly this ground |
| 5 | Don't Repeat Yourself | **Pass** — one matcher, one seam, one production call site; the qualified candidate is derived in one named place and agrees byte-for-byte with `CatalogEntry::repo()` on every catalog-built entry, pinned by a test. The one deliberate divergence (empty registry) is stated in Frozen semantics with its reason |
| 6 | Ship It | **Pass** — branch work, no push; this ADR writes no source |
| 7 | Leave a Trail | **Pass** — this record, its supersession pointer, and the ADR-index row |
| 8 | Learn and Adapt | **Pass** — the Renovate finding is converted into a standing documentation contract rather than discarded |
| 9 | Preserve Compatibility | **Pass, verified not assumed** — `registry_filter.rs` absent from `v0.9.0` … `v0.12.1`; `RegistryConfig` at `v0.12.1` has no `include`/`exclude`; docs have no `#browse-filters` anchor. No released surface changes. `RegistryFilter::matches` is `pub(crate)` in a single binary crate with no stable library API. `ConfigWriteReport` is untouched by this ADR |

**No Constitution Deviations row is required, and none is manufactured.**

One Principle 9 concern *was* found in adjacent work and is recorded where it
belongs — in the design record's registry-identity section, not here: rejecting
an alias that equals a locator, or the reserved `Local` sentinel, at config
load would **narrow** an input that parses on `v0.12.1` (`RegistryConfig.alias`
shipped, constrained to non-empty + trimmed + no `/` + no control or quote
characters + unique among aliases — nothing about locators or reserved names).

The rule that binds today is **`AGENTS.md` Principle 9** ("Stabilization freeze
on the road to 1.0.0: breaking changes are prohibited"), not
`docs/src/stability.md`'s manifest-input clause, whose own text is future-tensed
("parses on every later 1.x") on a page that opens "Grimoire is pre-1.0". The
stability page states the same policy for the 1.x line; Principle 9 is what
prohibits the narrowing now. The concern is not caused by this ADR, and the
design record recommends a fix that avoids it entirely.

## Technical Details

### API contract

```rust
// src/config/registry_filter.rs

/// The fully-qualified match candidate for one catalog row: `{registry}/{repository}`,
/// byte-identical to `CatalogEntry::repo()` whenever `registry` is non-empty —
/// which is every entry a catalog build produces. When `registry` IS empty the
/// two candidates coincide and this returns `repository` unchanged, so no
/// candidate can ever begin with `/`.
pub(crate) fn qualified_candidate(registry: &str, repository: &str) -> String;

impl RegistryFilter {
    /// `true` iff (the include list was empty, or SOME include pattern hits
    /// EITHER candidate) AND NO exclude pattern hits EITHER candidate.
    /// Candidates: `repository` verbatim, and `{registry}/{repository}`.
    /// The entry's own locator is not an input.
    pub(crate) fn matches(&self, registry: &str, repository: &str) -> bool;
}
```

```rust
// src/catalog/catalog_service.rs — the one production call site
CatalogScope::Browse => reg.filter.matches(&e.registry, &e.repository),
```

`registry_filter.rs` keeps its current stance: `globset` is its only import,
every signature stays primitive `&str`/`String`, and no `CatalogEntry`,
`CatalogRow` or `ResolvedRegistry` type is pulled in. No new module dependency
direction is created — both fields are already plain `String`s in hand at the
call site (`registry_catalog.rs:135,137`).

## Validation

- [ ] Two-host index fixture exists and a host-qualified pattern selects one
      row where the bare pattern selects both
- [ ] The combined include+exclude case (bare include, FQ exclude, one row)
      is pinned and would go red under Option 3
- [ ] Argument order of `matches` is pinned by a test that fails on a swap
- [ ] `task --force verify` green
- [ ] Every surface listed in the migration plan restates the rule from the
      landed code, naming **both** candidate strings in its first paragraph

## Links

- [`adr_registry_browse_filters.md`](./adr_registry_browse_filters.md) — the
  parent decision; D3 superseded by this file, D1/D2/D4–D13 stand
- [`adr_multi_registry_mcp.md`](./adr_multi_registry_mcp.md) — origin of the
  shared `load_catalog` seam and `[[registries]]`
- [`adr_projection_over_index.md`](./adr_projection_over_index.md) — the TUI
  tree's index/key contract
- [`research_registry_filter_candidate.md`](../research/research_registry_filter_candidate.md)
- `docs/src/stability.md` — frozen surfaces and the manifest-input widening rule
- [containers/image#775](https://github.com/containers/image/issues/775) —
  host-detection-by-inspection misfiring in Docker's own implementation
- [renovatebot/renovate#20213](https://github.com/renovatebot/renovate/discussions/20213)
  — explicit-candidate-by-name, the closest precedent, answered above
- [Gatekeeper — Allowed Repositories](https://open-policy-agent.github.io/gatekeeper-library/website/validation/allowedrepos/)
  — the `docker.io` / `docker.io-evil` prefix bypass
- [OCI distribution-spec](https://github.com/opencontainers/distribution-spec/blob/main/spec.md)
  — the `<name>` grammar that bounds the false-positive class

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-08-11 | Michael Herwig (architect pass) | Initial record. Supersedes `adr_registry_browse_filters.md` § D3 and two of its rejected alternatives |
