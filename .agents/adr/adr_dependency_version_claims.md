# ADR: Dependency tags as Cargo-style version claims, newest in the claimed major, settled by a sticky fixpoint

## Metadata

**Status:** Accepted (2026-11-12, owner, at the hex-plan gate: Q1 option a, Q2 option a, Q3 F1)
**Date:** 2026-11-12 · **Deciders:** Michael Herwig (owner) + Claude (architect)
**Beads Issue:** N/A · **Related PRD:** N/A (codebase map: `.agents/research/research_dependency_version_claims_codebase.md`)
**Amends:**
- `adr_artifact_dependencies.md`: Decision 3 (diamond policy, its "different digests 78" exit row, "Direct wins" as narrowed by Decision 8, "the only tag resolve per node", which becomes one `T(line)` resolve plus a bounded listing, and the partial-resolve rule that a pin stays only while its winning requested id matches), and its error and limit lists (four new refusals, two new `GraphLimit` values).
- `adr_lock_pin_stability.md`: § The pin rule (a semantic-evidence row for `requires` Range claims and an any-tag row for bare direct lines, graph carry only), D1 (which tag a new or moved pin records), and the § Validation agreement clauses `exit-agrees` and `relock-error`.
- `adr_unified_publish_version_cascade.md`: "cascade floats still always move" (Decision 9: a float never moves to a lower version).

**Blast radius:** external contract. Resolve results of `add`, `lock` and `update` (and the TUI and export relocks), the tag a new or moved `pinned` records, `status --check` answers for Range-claimed rows, one CLI flag on three commands, one appended `ErrorReason` literal, four new refusals on existing exit codes, `grim release` and `grim publish` leaving a float on a higher version, and one always-present report field (`floats_kept`) on both. No lock, config or hash change.
**Reversibility, per decision:**

| Decision | Door |
|---|---|
| 1 Claim grammar on `requires` edges | Two-way today (no release carries `requires`); **one-way at the first release that does**. Claim semantics ship in the same release as `requires` |
| 2 Newest in the claimed major | Two-way (selection is internal) |
| 3 Held pin stays while it satisfies | Two-way, widening only |
| 4 `--allow-downgrade`, reason `major-downgrade` | **One-way** once shipped |
| 5 Two majors exit 78 | Two-way in the widening direction |
| 6 Sticky fixpoint | Two-way (internal) |
| 7 Dropped-claim refusal | Two-way in the widening direction |
| 8 Bare direct yields | Two-way until `requires` ships, then **one-way** |
| 8 Bare direct line answered by any tag of its repository (Q2) | Two-way, widening only |
| 9 Release never lowers a float | Two-way for the behaviour; **one-way** for `floats_kept` once shipped |
| Recorded tag of a moved pin (`x:2@…`) | **One-way**: committed locks keep it |
| The `org.opencontainers.image.version` annotation as evidence | **One-way**: published annotations start steering resolves |
| Stamps of a bundle reached by a Range claim (its `[[bundle]]` tag) | **One-way**: committed locks keep them |

**Tech Strategy Alignment:**
- [x] Golden Path (no new crate: `semver = "1"` exists; Tokio, Rust 2024)
**Domain Tags:** data | api · **Supersedes:** N/A · **Superseded By:** N/A

## Context

Decision 3 of the dependencies ADR merges two requests for one node only when they resolve to the same digest. Two dependents asking `B:2` and `B:2.1` therefore exit 78 as soon as any 2.x above 2.1 ships, because the floats `2` and `2.1` then name different manifests. The owner judged this "too brittle" and fixed eight decisions. This record says how to realize them, where a decision cannot be realized as worded, and what changes for users.

The resolver: `Graph` (`src/resolve/graph.rs`) walks pass after pass (`walk_until_settled`: expand, `settle_pass`, `restart`); `restart` keeps `digests`, `reads`, `released` and `attested`. `expand_counting_passes` runs a second, "kept" walk. `settle` (`src/lock/merge_policy.rs`) is the pure rule, with an offline caller in `src/lock/effective_set.rs`. `PinnedIdentifier::answers` (`src/oci/pinned_identifier.rs`) and `src/lock/pin_rule.rs` decide what a held pin answers; `answers_edge` accepts a recorded edge in any repository. `requires` is unreleased: 0.15.1 refuses a second layer. Bundle members are released.

## Decision Drivers

- The `B:2` beside `B:2.1` diamond must resolve, and stay resolved when 2.2 ships.
- Principle 9: 0.15.1 locks and configs read unchanged; a dependency-free lock stays byte-identical; a released field keeps its meaning.
- `grim lock` never moves a pin that still answers; every moved pin is explainable.
- Determinism: the same inputs give the same lock, whatever the claim order or the node names.
- Bounded registry cost and a defined offline answer for every case.

## Industry Context & Research

From `.agents/research/research_artifact_dependencies_prior_art.md` (expires 2027-05-01), not redone here. Claude Code plugins intersect several semver constraints to the highest satisfying tag and fail when none does: the closest analog. Cargo unifies semver-compatible requirements with lock priority but builds incompatible majors side by side; grim has one node per key, so two majors refuse. Go MVS shows a deterministic resolver without SAT, though it picks the minimum. uv/PubGrub backtracks with readable traces: the rejected option. Helm keeps semver constraints with a lock replayed by `build`.

## Considered Options

Two gates first: G1, the diamond resolves; G2, the resolve stops deterministically within a stated bound. Weights over the rest: determinism and explainability 0.25, Principle 9 0.25, registry cost and offline 0.20, implementation risk in `graph.rs` 0.15, error quality 0.15. Scores 1 to 5.

| Option | G1 | G2 | Det. | P9 | Cost | Risk | Errors | Weighted | Verdict |
|---|---|---|---|---|---|---|---|---|---|
| O1 Equal-digest only (status quo) | fail | pass | — | — | — | — | — | — | Rejected: fails G1 |
| O2 Full solver with backtracking (PubGrub-style) | pass | pass | 4 | 3 | 2 | 1 | 4 | 2.90 | Rejected: many manifest reads per node, a second resolver beside the pin rule, and Decision 5 forbids backtracking |
| O3 Caret claims, newest-in-major, sticky fixpoint | pass | pass (work budget) | 3 | 4 | 3 | 3 | 2 | **3.10** | **Chosen** |
| O4 Recompute lines every pass, no stickiness | pass | fail | — | — | — | — | — | — | Rejected: oscillates (example (ii)) |
| O5 Sticky lines that drop a source once its parent leaves | pass | fail | — | — | — | — | — | — | Rejected: oscillates the same way in example (ii) |

O3 scores low on determinism (its answer depends on pass history: example (ii) holds N2 at 1.0 where a solver finds 2.0) and on errors (refusals can name versions the user never chose). The margin over O2 is within scoring noise; O3 also stands on Decision 5.

**A. Trusting the cascade tag.** `move_tags` (`src/command/release.rs:822`) moves every cascade tag unconditionally, so a 2.3.7 backport after 2.9.0 points `2` and `latest` at 2.3.7. Decision 9 stops that for every release from this change on.

| Option | Verdict |
|---|---|
| A1 Trust `T(line)` blindly | Rejected: can violate a lower bound |
| A2 Always walk `tags/list` | Rejected (Q1 option c): one paginated walk per repository on every pick, and every offline pick fails |
| **A3 Resolve `T(line)`, check its evidence against the line, walk `tags/list` once per repository only on a miss; `grim release` never lowers a float** | **Chosen** (owner, Q1 option a) |
| A4 Follow the tag and leave release as it is | Rejected (Q1 option b): "newest" fails after any backport, and `grim update` moves pins backward within a major |

**B. Representing "no tag".**

| Option | Verdict |
|---|---|
| **B1 `latest` is the "any" claim; `or_latest` stays at parse** | **Chosen**: hash, config, lock and every `x:latest@…` pin unchanged |
| B2 A new any token | Rejected: a schema change for nothing B1 lacks |
| B3 Drop `or_latest` | Rejected: every bare line's declaration hash changes, so every upgraded lock reads stale (65) |

**C. Where a held pin's version comes from.**

| Option | Verdict |
|---|---|
| **C1 The manifest's `org.opencontainers.image.version`, else the reference tag's shape** | **Chosen**: stamped on every kind since v0.1.0, inside the digest, read by digest anyway (store first) |
| C2 A `version` field in the lock | Rejected: 0.15.1 refuses unknown fields (78), byte identity breaks |
| C3 Tag shape only | Rejected: `x:2@…` says nothing exact |
| C4 Reverse lookup through `tags/list` | Rejected: a network walk to keep a pin |

**F. Force scope of `grim add X` when X moves a dependency.** `--force` on `grim add` does not cover a dependency the add moves: its force scope is the target only (deps Decision 4, lock-pin D3).

| Option | Verdict |
|---|---|
| **F1 Keep force target-only. A moved dependency with edited files: the lock is saved, `add` exits 65 naming the dependency, and recovery is `grim install --force` or reverting the edit** | **Chosen** (owner, Q3) |
| F2 Widen `--force` to moved nodes | Rejected: a flag typed for X overwrites edits in files the user did not name |
| F3 The whole closure, as `update` | Rejected: same, wider |
| F4 Check moved dependencies before the save, refuse with nothing changed | Rejected (Q3): `add` already saves the lock and then refuses any dependency outside its force scope, so F4 gives one command two behaviours for refused dependencies. `refuse_added_conflict` (`src/command/add.rs:383`) checks a property of the lock; edited files are disk state the installer gate owns |

## Decision Outcome

Chosen: O3. Decisions 1 to 8 are the owner's; Decision 9 is the owner's answer to Q1. Each states how it is realized.

1. **Claims are Cargo-style on `requires` edges.** `2.1` is [2.1.0, 3.0.0), `0.2` is [0.2.0, 0.3.0); no tag, or `latest`, is any version. A direct declaration's typed tag stays a literal override. Bundle-member tags stay literal: a released field cannot change meaning.
2. **The chosen version is the newest in the claimed major**, through the line's cascade tag (`2`, or `0.2` for 0.x), checked against the line, with a `tags/list` fallback. Non-semver tags keep the equal-digest rule. Decision 9 keeps the cascade tag on the newest release for every release this grim makes. A float an older grim or another tool moved back still names an older release, and the ladder takes it while it satisfies the line.
3. **A held pin stays while carry answers it**: for Range claims, while its evidence satisfies every claim. Below a new lower bound it moves to the line's target with no flag; reports already show it as `moved`.
4. **A major downgrade of a held pin needs `--allow-downgrade`.** "Major" is the caret compatibility group. The guard fires only when Range claims on `requires` edges chose the target; Literal, Any, bundle-member and direct-typed moves never trigger it.
5. **Two compatibility groups claimed on one node exit 78** (`BundleConflict`), flag or not. No backtracking. The fix stays a direct declaration with a tag.
6. **Resolution is a fixpoint with sticky learned lines.** A line only narrows and survives `restart`. A line node (all claims Range or Any) takes its candidate from its learned line only, and learns only from parents reachable from a root through in-line nodes, judged once per pass. Literal and Digest references and direct declarations resolve in the pass that reads them, as today.
7. **A move or a conflict that rests on a dropped version's claim exits 78** (`UnchosenRequirement`), naming both nodes and a fix. Every pin that moves is then inside the named closure.
8. **A bare direct declaration yields to a same-repository `requires` Range claim; a typed one wins over every edge.** An explicit `:latest` yields like a bare line, because the two are identical after parse. A bare direct line is answered by any tagged pin in its repository (owner, Q2), so after an edit from `x:2` to `x` only `grim update` moves `x:2@…`.
9. **`grim release` never moves a float to a lower version.** A cascade float (`X.Y`, `X`, `latest`) on a normal release whose target carries an exact version above the release, inside the float's span and compared by semver precedence, stays where it is while that version's own exact tag still names the float's manifest, and the report names it in the always-present `floats_kept`. No new flag. Floats already published and tags other tools moved stay as they are, so the ladder keeps its verify-then-list fallback.

### Consequences

- Good: diamonds inside one major resolve and stay resolved; `grim lock` keeps more pins (tag differs, version satisfies); the held-back warning fires only on a real version mismatch; a backport no longer drags `X` and `latest` back.
- Bad: a fresh resolve reads `latest` first for every claimed node, so a node claimed below its newest major costs an extra pass and one extra manifest read; Decision 7 refuses some graphs a solver would finish (example D7); a publisher can no longer roll `latest` back with `grim release`.
- Neutral: one stderr line names each yielding bare line, each narrowing by a dropped version, and each float a release kept.

**Dropped versions without a pin.** A fresh resolve has no held pins, so the pin form of Decision 7 never fires there. A dropped version still narrows or refuses in exactly one shape: version V of node P teaches in some pass, its Range claim narrows child C's learned line, and in a later pass a claim from a node that first taught after that pass moves P off V. In a fresh resolve P leaves V only when V was chosen under a wider line than its final one: an Any line taken at `latest`, a wider 0.x group, or a cascade tag an older grim or another tool moved back. C keeps V's narrowing. If C's line still holds a release, C resolves inside it, exit 0 with one stderr line naming V (example (ii)). If the line is empty or holds no release, the run exits 78 `UnchosenRequirement` in its conflict form.

**Convention calls.** Decision 8 narrows concepts.md's "a direct declaration always wins" for bare lines against `requires` edges; docs change with the code. The downgrade switch is an enum, not a `bool` parameter.

## Technical Details

```
Context: user ──> grim (add | lock | update | release | publish) ──> OCI registry (manifests, tags/list); lock, store, tag cache on disk
Resolve: claim_of ─> learned lines ─> ladder (T(line), listing) ─> settle ─> pin rule (carry) ─> downgrade and dropped-claim checks
Release: cascade set ─> evidence of each float's target ─> move or keep (floats_kept)
```

Contract index, in build order:

1. Claim grammar: [§ Claim grammar](#claim-grammar)
2. Evidence function: [§ Version evidence](#version-evidence-one-function)
3. Settle, selection and validation: [§ Settle predicates](#settle-predicates)
4. Target ladder: [§ Target ladder](#target-ladder)
5. Pin answers and its seam: [§ Pin answers](#pin-answers)
6. Per-node state and the work budget: [§ Passes](#passes)
7. Dropped-claim check: [§ Checks at the fixpoint](#checks-at-the-fixpoint), Decision 7 rows
8. Downgrade guard and flag: [§ Checks at the fixpoint](#checks-at-the-fixpoint), Decision 4 row, and [§ The flag](#the-flag)
9. Errors, exit code and message: [§ Errors](#errors)
10. `status --check`: [§ Reports and `status --check`](#reports-and-status---check)
11. Release float rule: [§ Release float rule](#release-float-rule)
12. Docs and catalog drift: [§ Principle 9](#principle-9), last row

### Claim grammar

A `requires` edge tag, after `_` becomes `+` and build metadata is dropped, is a **Range** claim only when it matches `^(0|[1-9][0-9]*)(\.(0|[1-9][0-9]*)){0,2}$`, or that pattern with three parts and a `-<prerelease>` suffix. It then becomes the caret `semver::VersionReq` `^tag`. The regex runs first because semver 1.0.28 also parses `2.x` and `x`, which are legal OCI tags.

| Edge reference | Claim |
|---|---|
| no tag, or `latest` | Any |
| `2` / `2.1` / `2.1.3` | Range [2.0.0, 3.0.0) / [2.1.0, 3.0.0) / [2.1.3, 3.0.0) |
| `0` / `0.2` / `0.2.5` / `0.0` / `0.0.3` | Range [0, 1) / [0.2, 0.3) / [0.2.5, 0.3) / [0.0, 0.1) / exactly 0.0.3 |
| `2.1.0-rc.1` | Range; prereleases of 2.1.0 only, by `VersionReq` |
| `2.x`, `x`, `v2`, `stable`, `02`, `2.1.0.1` | Literal (equal-digest rule). A version-shaped tag whose caret upper bound cannot be written (the component the caret raises is 18446744073709551615, judged on the lower bound without its prerelease) is also a Literal claim |
| carries `@sha256:…` | Digest |
| any bundle-member edge | Literal or Digest (unchanged) |

A node keeps its Range claims as a set of requirements; prerelease admission is `VersionReq::matches` over the whole set, in `satisfies`, `highest_in_line` and `merge_policy::validate`. Lower bound lo and upper bound hi are derived only to name the target tag and to detect an empty line (lo ≥ hi). `T(line)`: `M` for hi (M+1).0.0, `0` for 1.0.0, `0.m` for 0.(m+1).0, `0.0.p` for 0.0.(p+1), `latest` for an Any line. This is a new function: `cascade_set` (`src/oci/release.rs:197`) and `is_reserved_float_tag` (`src/command/publish.rs:873`) are private and map a version to tags.

### Version evidence: one function

`evidence(digest, reference)`, first rule that applies:

| Source | Evidence |
|---|---|
| The manifest at `digest` has `org.opencontainers.image.version` that parses as semver (after `_`→`+`) | exact version (memoized by digest) |
| `reference` carries a full semver tag | exact version |
| `reference` carries a tag `X.Y` or `X` | span [X.Y.0, X.(Y+1).0), or the caret span of `X` |
| `reference` shows nothing, and the pin is a held pin | the evidence of the previous lock's recorded edges onto it in its registry and repository, at its digest: each recorded `requires` edge no bundle lists as its caret range, since it settled through its line (`x:2.1` shows [2.1.0, 3.0.0)), and each edge a bundle lists as its tag's shape; the narrowest where they nest, unknown where two do not |
| otherwise | unknown |

A semver annotation wins over the tag. `satisfies(evidence, claims)`: an empty requirement set is satisfied by any evidence, unknown included. Otherwise an exact version matches every requirement, a span lies inside every requirement's range, and unknown satisfies nothing. This one predicate serves the pin answers row, "inside the line", carry, settle, restart and the release float rule. Tag-shape evidence is computed per reference, never memoized by digest, so the memo does not depend on which tag read a digest first. A recorded `x:2` is the span [2, 3), not unknown; a recorded `x:2.1` that settled through its line is [2.1, 3.0), not the tag's shape [2.1, 2.2), so two recorded claims `x:2.1` and `x:2.3` nest at [2.3, 3.0). A held pin whose annotation and own tag show nothing takes its version from the edges the previous lock recorded onto it, since each of their references read that digest. It never stands in for evidence that is known and fails, and a claim the lock did not record supplies none. A marketplace member pick (`grim update --marketplace M plugin:member`) reads a held dependent's recorded `requires` range claim that no bundle lists as its caret line too. A pick that moves the claimed row to a version inside that line keeps the dependent at exit 0. The version comes from the moved manifest's version annotation, else its tag's shape. Outside the line, or with no version shown, the held-requirement refusal (78) stands.

### Settle predicates

`settle` splits into selection (the ladder, graph only, with registry access) and validation (pure: claims plus one candidate and its evidence). The first predicate that matches decides. Any adds no constraint once another claim exists; its `latest` digest need not match.

| # | Predicate | Verdict |
|---|---|---|
| D1 | Direct is a path | Direct; every edge shadowed (today) |
| D2 | Direct has a tag other than `latest`, or a digest | Direct; shadowed: Range edges its evidence fails, Literal or Digest edges at another digest; the held-back warning lists shadowed `requires` edges. The held-back warning (`add`, `update`, `lock`) and the `status --check` held-back note judge a range request beside the range claims the pin's version meets alone, so a request that admits the pin only beside those lenders is answered, not held back |
| D3 | Direct is `latest` and a `requires` Range claim names the direct's own registry and repository | Yields: the node settles by E5 over those Range claims. Edges from elsewhere stay shadowed with today's publisher-mismatch warning; Literal and Digest `requires` edges and bundle-member edges stay shadowed. One stderr line names the yield |
| D4 | Direct is `latest` otherwise | Direct (today) |
| E1 | No direct; claims name more than one registry or repository | Equal-digest rule over all (today) |
| E2 | Any Digest claim | Digest claims equal; Literals resolve to that digest; its evidence satisfies every Range; else 78. On a node of one repository that a Literal or Digest claim fixes, Range and Any edges are judged, not resolved: a Range edge needs no tag of its own and agrees when the fixed manifest's evidence satisfies it; an Any edge adds no constraint. A failed read of such an edge's own tag refuses nothing. A Literal claim with no version evidence beside a Range claim does not satisfy it and exits 78. `status --check` on a single-repository key with a Literal or Digest edge judges a split on the fixing references alone, reads its Range edges by the fixing digest's evidence and ignores its Any edges |
| E3 | Literal and Range | Literal digests equal and their evidence satisfies every Range; unknown evidence must equal the ladder's target digest; else 78 |
| E4 | Literal only | Equal-digest rule (today) |
| E5 | Range | The ladder over the line. An empty line exits 78 `BundleConflict` when it is learned, before any ladder step can report 79; the dropped-source form (`UnchosenRequirement`) takes precedence |
| E6 | Any only | Resolve `latest` |

The offline caller `effective_set` passes the locked digest with evidence "vouched by the lock", which satisfies every claim; with no lock row, differing references read `Conflicted` as today. So `effective_set` behaves as today and reads no manifest.

### Passes

A *line node* has only Range or Any claims, or is a bare direct line that yields (D3). Only line nodes use learned lines. Literal and Digest references (bundle members included) and every other direct declaration resolve in the pass that reads them, as today. Each pass runs three steps:

1. **Expand.** A line node's candidate comes from its learned line only: the held pin when it is not selected or released and carry answers the learned line, else the ladder. An empty line is Any and takes `latest`. Claims read in this pass never steer a line node's choice in this pass. A miss on `latest` for a node whose learned line is still empty (the tag is absent, or offline it is not in the tag cache) is not final: the node has no candidate in that pass, starts no listing, and teaches nothing. If the judge narrows its line, the next pass resolves that line's own `T`. If the line is still empty when the pass ends, the next pass continues the ladder at step 3, which gives a candidate, 79, or the offline miss (69). A read that fails below a line node's candidate (a tag resolve, a manifest read, or a deeper line node's selection) does not end the pass: it refuses only at a stop that keeps that candidate, and a pass that moves the candidate drops it. A failure on a node the roots reach without passing a candidate refuses at once. A line node with no release in its line, or offline no listing, waits for the pass's end: a literal or digest claim that fixed it by then reads it at that manifest, and otherwise its refusal comes after the judge, so a conflict (78) wins over 79 or 69.
2. **Judge, once, over the whole pass.** A node is *in line* when its candidate passes the settle predicates against its learned line and every claim from every parent present in this pass. The *teaching set* is the roots that are in line, plus every in-line node with a teaching parent: reachability from the roots through in-line nodes. It does not depend on fold order or names. Every node with a teaching parent narrows its learned line by its teaching parents' same-repository Range claims and records each source (parent key, parent digest, claim tag). A held pin joins `released` when carry no longer answers its learned line and every claim present; `release_conflicts` and attestation run as today: a conflict releases a held pin only over a claim the previous lock never recorded, and a recorded claim the pin no longer answers is a conflict (78), because releasing it would move the pin on a `grim lock` with unchanged config. Nothing chosen under a non-teaching claim can teach, because line nodes read learned lines only.
3. **Restart** when a candidate lies outside its learned line, a node has no candidate yet (a deferred `latest` miss), or `released` or `attested` grew. Otherwise stop. A node still out of line at a stop goes to `finish`: conflicts (78), then cycles (65). A node waiting on a refusal does not count as a node with no candidate yet. A node whose read turns from its line to a literal or digest claim's manifest a second time gets one more pass, in which it selects only after the late round's waves end; a third turn stays a conflict (78), so passes stay finite.

**One test for a held pin.** Step 1 and the judge keep a held pin exactly when carry answers it (§ Pin answers). For Range claims carry answers by `satisfies` alone, with no recorded-edge escape when the evidence fails; an empty requirement set is satisfied by any evidence, unknown included.

| State | Survives `restart` | Kept walk |
|---|---|---|
| `learned: BTreeMap<NodeKey, Learned { reqs, sources }>` | yes | starts empty |
| `evidence: BTreeMap<Digest, Option<Version>>` (annotations) | yes | copied |
| `listings: BTreeMap<(registry, repository), Vec<(lower bound, tag)>>`, full-semver and `X.Y` tags only | yes | copied |
| `work`: node admissions summed over every pass | yes | shared: one count per `expand_counting_passes` call |
| `digests`, `reads`, `released`, `attested` | yes (today) | as today |

The kept walk starts with an empty `learned` because it must not import claims from a manifest it never reads.

**Work budget, not a convergence proof.** No pass bound is claimed: a chain of k nodes can need k passes. One budget covers both walks of an `expand_counting_passes` call: node admissions summed over every pass at most `16 × MAX_TRANSITIVE_NODES` (32,768). A 2,048-node graph gets 16 passes; a 100-node graph gets 327. Passing it exits 65 (`GraphTooLarge { limit: Work }`), and the message says this is a work limit, not proof the graph has no answer.

### Target ladder

| Step | Candidate | Accepted when | `pinned` records |
|---|---|---|---|
| 1 | Held pin, not selected, not released | carry answers the line (§ Pin answers) | its tag, unchanged |
| 2 | `T(line)`, one resolve per tag per run | `evidence(digest, T)` satisfies the line | `T`, e.g. `x:2@…` |
| 3 | Tags from `tags/list` whose own shape satisfies the line: a full-semver tag whose version matches every requirement, or an `X.Y` tag whose span lies inside every requirement. An Any line admits no prerelease tag, as Cargo's `*` does not. Highest lower bound first, a full version before a span on a tie, at most 8 resolved | `evidence(digest, tag)` satisfies the line. The annotation wins, so a release candidate retagged `2.5.0` fails and is passed over | that tag, e.g. `x:2.4.1@…` or `x:1.2@…` |
| — | none | 79 `NoMatchingVersion`; offline at step 3, an offline miss | — |

The former fourth step (the most demanding claim's own tag) is cut: it added churn on every lower-bound rise, and no case needs it. Step 3 admits `X.Y` tags so that a publisher who pushes only partial tags (`1`, `1.2`) with no semver annotation still answers a minor claim: `x:1.2` lands on the tag `1.2`. An `X` tag needs no entry there, because it is `T(line)` whenever its span fits. A publisher with neither a fitting tag nor a semver annotation answers with 79; declare it directly. Step 3 uses a new `highest_in_line`; `pick_highest_version` stays unchanged, because it admits prereleases and feeds the catalog. The two share only the `_`→`+` normalization. A Literal node records today's winner (smallest request); an Any-only node records `latest`.

**Listing bounds.** Each listing goes through `retry_chain`. Every `tags/list` page has its own 30 s deadline in the registry client, and no deadline spans the whole walk, so the byte budget bounds how many pages one walk reads. One run reads at most 32 MiB of listings in total (`GraphTooLarge { limit: ListingBytes }`, 65), counted in one unit wherever it is counted: what the walk charges, each tag's length plus the size of a `String`, plus 256 bytes and the cursor per page. Each walk stops at the smaller of that remainder and today's 128 MiB per-repository cap. A walk is complete only when it reaches the registry's last page. One cut short (a repeated cursor, an empty page with a next link, a next link grim cannot follow, or the budget) refuses with that 65 and never chooses a release from the prefix. The memo keeps only full-semver and `X.Y` tags.

### Pin answers

Every existing evidence path stays untouched: `answers_direct`, `answers_edge` (a recorded edge in any repository, or `names`), `answers_listed_edge`, `answers_stamped_edge` and `PinnedIdentifier::answers`. Two rows are added, in graph carry only:

| Claim | Pin answers when |
|---|---|
| A `requires` Range claim, same registry and repository | `evidence(pin digest, pin reference)` satisfies it, the reference being, for a held pin whose annotation and tag show nothing, its recorded edges' evidence (§ Version evidence). Nothing else answers: when that evidence fails, neither a recorded edge nor an equal tag answers in its place |
| A bare direct line (Any) | the pin carries any tag in the line's repository (owner, Q2). After an edit from `x:2` to `x`, `grim lock` keeps `x:2@…` and only `grim update` moves it; `grim add repo/x` over a held member pin keeps 2.x |

The restamp gate (`src/lock/edit.rs`, `restamp_answers_*`), the kept-pin note in `src/command/add.rs:823` and `src/export/resolve.rs:226` stay on tag equality and read no manifest. So the kept-pin note does not fire for a pin only the Any row keeps, and an offline edit can leave a lock stale where a claim-answering pin's tag differs, until the next `grim lock`. A key whose declaration left keeps its row, and leaves the lock stale, when its pin's record names no surviving request but every surviving request is a recorded `requires` range claim that no bundle's list carries and that the pin's tag does not rule out: the tag is an exact version inside the line, an `X.Y` or `X` span meeting it, or a tag that shows no version. `grim remove` and `grim uninstall` relock in the same run (owner decision 2026-11-13), so the kept row is settled there: the relock reads the pin's version and keeps it when it answers, else moves it and says so, because dropping the row would move the pin onto the newest release of the line. Under `--no-relock` the in-place edit keeps the row and leaves the lock stale until the next `grim lock`, and the note says `grim lock` keeps the pin if its version answers the claim. A tagless pin and a request a bundle lists keep the old rule: the row drops. This amends `adr_lock_pin_stability.md` § The pin rule, the clause that drops the row of a key whose declaration left. The restamp of a kept tagless pin takes the recorded edge whose caret range shows the pin's version, else the smallest edge's tag. Answers never narrow for any released shape (direct lines, bundle members, stamps, recorded edges); a bare direct line's answer widens. Only an unreleased `requires` Range claim loses tag equality and the recorded-edge answer when its evidence fails. The hand-edit residual widens: a hand-edited digest in the same repository whose annotation fits the claim survives `grim lock` (CWE-345, still the N1 non-goal).

A tagless pin that a digest-only request won stays tagless on every relock; an edge that names a tag at the same digest does not stamp it.

### Checks at the fixpoint

The dependencies ADR's error order applies: wave errors, then conflicts (78), then cycles (65). The downgrade guard runs last, on an otherwise valid graph.

- **Decision 7, pin form.** It covers a held, unselected pin whose final candidate differs. It fires when the pin's evidence satisfies every claim of the final graph and fails only learned Range sources whose parent digest is not in the final graph. A Literal or Digest conflict's release never triggers it for the released key (exit 0 as today); a version dropped further below that release still counts as dropped, so a deeper pin that would move only for its claim exits 78, and a narrowing it leaves raises `DroppedNarrowing`.
- **Decision 7, conflict form.** A learned line that turns empty or holds no release, while one of its binding sources has a parent digest absent from the current pass, exits 78 here instead of `BundleConflict` or 79. This is decided when the line is learned.
- **Decision 4.** It fires when the node's line came from `requires` Range claims, no typed direct declaration holds the key, and the held pin's evidence is known and lies in a higher compatibility group than the choice. Evidence is unknown for a third-party artifact with no semver annotation and a non-semver tag (`stable`); such a move proceeds and reports `moved`. A claim from a version the resolve dropped counts: when no claim present in the final graph fails the previous pin, the requesters are the line's learned sources that fail it, each dropped one named `<kind>/<name> <version> (dropped by this resolve)`.

### The flag

`--allow-downgrade` on `grim add`, `grim update` (including `--marketplace`) and `grim lock`, from one flattened `DowngradeOpts` in `src/cli/options.rs` (the `HookTrustOpts` pattern); `LockArgs` gains it. `ResolveOptions` gains `downgrade: Downgrade { Refuse, Allow }`, passed into `target_lock`. Not on `grim install`, which never resolves. The TUI and the export stage pass `Refuse`; their refusal names the CLI command.

### Errors

| Error | Exit | `reason` | Message template |
|---|---|---|---|
| `NoMatchingVersion` (new, wave) | 79 | null | `{kind} '{name}': no release of {repository} satisfies {claims}; the newest is {newest}`. `{newest}` is the highest release tag the run's listing holds. When this run read that tag's manifest and its version annotation names another version, the tail reads `; the newest is {tag}, whose manifest names version {annotated}`. A listing with no release but with prereleases reads `; it lists only prereleases, the newest is {tag}`, naming the highest prerelease. A listing with no version tag reads `; it lists no release`. The message reads nothing from the registry. |
| `GraphTooLarge { Work }` / `{ ListingBytes }` (new limits, wave) | 65 | null | `the resolve stopped after {n} node admissions in {p} passes; this is a work limit, not proof the dependencies have no answer` / `tag listings passed {n} bytes` |
| Offline miss on `T(line)` or a needed listing | 69 | null | Unchanged: both go through `retry_chain`, which wraps the miss as `RegistryUnreachable` |
| `BundleConflict` (conflict) | 78 | null | Unchanged; also fires on two groups and on any other empty learned line; its hint says "declare it with a tag" |
| `UnchosenRequirement` (new, conflict) | 78 | null | Pin form: `{kind} '{name}' would move from {from} to {to} only because {parent} {dropped}, which this resolve dropped, required {claim}; {fix}`. Conflict form: `{kind} '{name}': {claims} cannot all hold, and {parent} {dropped}, which claimed {claim}, is no longer in the graph; {fix}` |
| `MajorDowngrade` (new, last) | 78 | `major-downgrade` (appended) | `{kind} '{name}' would move from {from} to {to}, a lower major version, because {requesters} require {claim}; {fix}` |

`{fix}` is per command. Keeping a version means declaring the exact tag, `{repository}:{from}` (the digest when no such tag exists): a typed `X` would take the newest X, not `{from}`. A pin refusal (`MajorDowngrade`, `UnchosenRequirement`) ends with the command that moves the pin, naming `--allow-downgrade` where it applies. A conflict refusal (`BundleConflict`) ends with "declare <kind> <name> directly in grimoire.toml with a tag to choose one". The tag it names is one that exists: the surviving claim's own tag when one claim survives; else the newest tag this run listed that lies in the surviving line; else the line's cascade tag when this run resolved it; else the narrowest surviving claim's tag. A cascade tag alone may name no manifest, because no release creates one for a prerelease.

- `grim lock`, `UnchosenRequirement`: "declare it to keep {from}", or bare `grim update`, because a named update refuses a stale lock with 65, running `grim update --allow-downgrade` instead if it refuses a major downgrade, since a whole update may meet the downgrade guard.
- `grim add X`: every fix ends "then rerun `grim add X {flags}`", where {flags} repeats the typed `--kind`, `--name`, `--no-install`, `--force`, `--trust-workspace` or `--no-trust-workspace`, and `--allow-downgrade`, each shell-quoted. The `UnchosenRequirement` alternative reads "…, or run `grim update`, then rerun `grim add X {flags}`", running `grim update --allow-downgrade` instead if the update refuses a major downgrade. The TUI's install remedy names `grim add <ref> --name <binding>`.
- `grim update <names>`: the pin form of `UnchosenRequirement` adds "name {name} too: `grim update -- {names} {name}`", running `grim update --allow-downgrade -- {names} {name}` instead if it refuses a major downgrade. The conflict form (`claims` cannot all hold) gives the declare clause alone: naming the node still reads the held version that claimed the other side, so that run refuses again.
- `grim update --marketplace` adds ", which moves every pin of the plugin whose tag moved" after its command, and for `UnchosenRequirement` the same `--allow-downgrade` alternative.
- `MajorDowngrade` names only the declare clause and the flagged command, as before; no unflagged "name {name} too", because that update exits 78 again. A bare `grim update` keeps its flag-only text.

For `UnchosenRequirement` under `grim add` the alternative was shown to work by `test_declaring_the_surviving_claim_clears_a_move_only_a_dropped_version_asked_for[add]`. Known doubt: under a named update, following the declare clause first meets exit 65 "run `grim lock`, then this update" (the stale-lock gate), and its own remedy then works. The `grim add` rerun does not repeat the global `--registry` flag. `forceable` and `retryable` are false.

The slug serves a consumer: `grimoire-vscode` drives `add` and `update` through `--format json` and must offer `--allow-downgrade` without parsing text. `forceable` names `--force` and cannot say it. The other new errors carry no slug, like `BundleConflict`, because their remedy is a config edit the message names.

### Reports and `status --check`

JSON shapes stay unchanged, except `floats_kept` on `release` and `publish` (§ Release float rule). Moves report through the existing `disposition: moved` and `moved` arrays. Plain output adds one stderr line per pin a raised lower bound moved, one per yielding bare line, and one per narrowing by a dropped version.

`status --check` changes for a dependency row whose recorded edges are Range claims of one repository. It resolves `T(line)` of those claims and compares the digest with the pin, so such rows no longer read as a split (`split_note`); an empty line still does. A pin recorded from a listing (`x:2.4.1@…`) sees a newer release only when the cascade tag names one, not after a backport an older grim or another tool published. A direct row declared with no tag (or `latest`) and no digest, whose recorded edges are range claims of its own registry and repository, answers like a dependency row, by `T(line)`. Tagged and digest-pinned direct rows and bundle-member rows stay unchanged.

A `T(line)` digest that differs from the pin answers `update_available: true` only when its version evidence satisfies the line, as the target ladder accepts it. Evidence outside the line, or a gone manifest, answers `false` with no note. A failed manifest read answers `null`. So does a line whose cascade tag is gone, names a release outside the line, or names a manifest without version evidence: `grim update` then reads the tag listing, which status does not, so status cannot answer. `true` only when the read manifest's evidence lies in the line. A bare direct row (no tag or `latest`, no digest) builds its line from the same-repository Range edges only, so Literal, bundle-member and other-repository edges beside them do not turn the row into a `latest` check. Every manifest read of one `--check` run shares the 2048-read cap and the 8-wide bound and stops at the 300 s run deadline. A claim is a culprit (merge validate, typed-direct shadowing) only when the pin fails it beside the other present claims the pin satisfies alone.

### Release float rule

Decision 9 applies to `grim release` and to every `grim publish` entry, which runs the same path, whenever the cascade fires. `--no-cascade`, a prerelease and a non-semver tag move no float, as today. R is the release's exact version.

| Rule | Contract |
|---|---|
| Read | For each float F of the cascade set (`X.Y`, `X`, `latest`; never the exact tag), resolve F in the push repository. F absent: F is created. F names the release's target digest already: F moves (a no-op, no read). Otherwise C = `evidence(F's digest, F)`, store first, then the registry |
| Compare | Only an exact C counts: F's own tag gives a span (`X.Y`, `X`) or unknown (`latest`), never a version. F moves when C is not exact, when C is a prerelease (a float on a prerelease always moves to the release, whatever the precedence: `latest` on 2.0.0-rc.1 moves to 1.9.0, because `grim release` never cascades a prerelease and so one there was pushed by hand; owner decision 2026-11-13), when C lies outside F's own span (C's major and minor equal F's for `X.Y`, C's major equals F's for `X`; `latest` spans every version), or when R ≥ C by semver precedence, build metadata ignored. So a float on 2.4.1 is kept against 2.4.0. Otherwise F stays only while C's own exact tag (`X.Y.Z`, or `X.Y.Z-pre`) still names F's digest, one more tag resolve per float that would be kept; a gone or retagged exact tag moves F with one stderr line naming F, C and that tag |
| Order | Every float is read and compared after the overwrite guard and before any tag moves. Then the exact tag moves first and the moving floats after it, as `move_tags` does today |
| Failure | A failed tag or manifest read in a real release aborts before any tag moves, with the exit code of a failed overwrite-guard lookup (fail closed); offline, a real release whose cascade has a float exits 81. A float on a manifest grim cannot parse (an image index or another tool's image), or one the registry serves under another digest, is not a failed read: its evidence is unknown, so it moves, as it did before |
| `--dry-run` | Runs the same reads, so its plan names what a real run would move and keep. When a read fails, or offline, it keeps 0.15.1's exit code: one stderr warning (`could not check the floats of <repo>: <cause>; planning every float to move`), every float planned as moving, `floats_kept` `[]` |
| `--force` | Unchanged: it moves the exact tag only, never a kept float. No flag lowers a float: `--no-cascade` already covers "leave the floats alone", and a deliberate rollback of `latest` is a tag edit with another OCI tool |
| Report | `tags` keeps its documented meaning, the tags pointed at the manifest, so a kept float leaves it, as `--skip-existing` already reports `[]`. New always-present `floats_kept: [{tag, version}]` (`version` is C), `[]` when none, on `ReleaseReport` and on every `PublishEntry` (`[]` on a skipped or failed entry). Plain output adds one stderr line per kept float: `kept '{tag}' on {C}, above this release {R}` |
| Cost | Per release at most three tag resolves and three manifest reads, plus one tag resolve per float that would be kept, store first; none without the cascade |
| Residual | Floats already published and tags other tools moved stay as they are, so the ladder's verify-then-list fallback stays. Read-then-move is not atomic: two concurrent releases into one repository can still leave a float on the lower one, as with the overwrite guard. The version annotation is publisher-controlled; a float is kept only while the exact tag of the version it shows names the float's manifest, so a yanked release whose exact tag is gone, or a retagged one, no longer holds a float. A publisher who keeps both the annotation and that exact tag (a co-publisher's push, a date-style version such as `2026.11.1` also tagged) still keeps the float, visible only as `floats_kept` and the stderr line; the remedy is a tag edit with another OCI tool |

### Offline

A held pin's evidence comes from the stored manifest, else from its recorded tag's shape, else from the edges the previous lock recorded onto it (§ Version evidence). A store miss never releases a held pin: when the tag gives no evidence either, the pin is kept with one warning. A tag shape that only widens the claim is not evidence: a held pin whose tag's span meets the claimed line without lying inside it (a pin recorded under the cascade float `x:1` while a claim asks `x:1.2`) is judged by the previous lock's edges onto it, read as § Version evidence reads them, and kept with that warning when they show nothing either. A span wholly outside the line, or an exact tag outside it, still releases the pin. The warning says neither the tag nor the lock's recorded edges show whether the pin meets the claim. `T(line)` reads the tag cache; a miss exits 69, except the deferred `latest` miss on a still-empty line (§ Passes, step 1). A needed listing is always an offline miss (69), never a pick from cached tags, which hold only tags this machine resolved.

### Edge shapes

- 0.x follows Cargo: `0.2` and `0.3` conflict (78); `0.2` beside `0` narrows to 0.2.x.
- The cascade never targets a prerelease, so a prerelease claim lands on step 3.
- A claim above every release (`2.9` while 2.4 is newest) exits 79, as a missing tag does today.
- A repository with no `latest` tag (a third-party push, a `--no-cascade` or prerelease-only publisher) claimed as `x:1` resolves through `1` as today. It costs one failed `latest` lookup and one extra pass.
- Tags `2` and `3` naming one manifest merged before and now exit 78: a named narrowing.

### Data model

No schema change: `grimoire.toml`, `grimoire.lock` (`lock_version` 1), `marketplace.lock`, the declaration hash and published schemas stay. `pinned` keeps `repo:tag@sha256:…`; only which tag a new or moved pin carries changes. The release and publish reports gain `floats_kept`.

### Residuals

Known limits of the final code, none of them a defect the owner must rule on.

- A publisher who keeps both the version annotation and its exact tag still holds a float (§ Release float rule, Residual row).
- The listing budget counts the bytes the walk charges per tag, not raw response bytes, because the registry client does not expose body lengths.
- With no whole-walk deadline, only the per-page deadline and the byte budget bound one listing's wall time. A registry that answers each small page just under the deadline can hold one walk for a long time.
- The `grim add` rerun remedy does not repeat the global `--registry` flag, so a short-id reference typed under it reruns against the default registry.
- A pin that answers some remaining requests of a left declaration keeps the unqualified note "which keeps the pin", though the next lock judges the other requests by version and may still move it.
- A store-only bundle (members from the store, no `[[bundle]]` row) is not excluded from the recorded range edges a lock edit reads, so a member edge there could read as a claim. No such lock carries range edges from a bundle in practice.
- A lock edit that drops a key only because its target left writes no note for the dropped row, where a kept row gets the note.
- The held-back lender rule lives twice, in `merge_policy` and in the resolver's held-request check, because sharing it needs `merge_policy` to expose its helper.
- An unannotated Fixed row in `status --check` reads evidence from the smallest fixing reference's tag shape, where the graph tries the failing shape first; they differ only with no annotation and several semver-shaped fixing tags. The skipped-checks warning still says "re-resolves" though it counts manifest reads too, and the held-back notes keep their own 2048 count beside the check's.
- Suggest-tier review items left unfixed: the version-claim listing memo is scanned from a copy where an iterator over it would do (it changes a signature the graph calls); `grim add`'s remedy loops when the refused node is the add's own target, since the remedy builder does not know the target's key; `resolve_marketplace` takes loose arguments where one `ResolveOptions` would do; the three agreement oracles judge claims one at a time where one set would do, which waits on a prerelease case that would add a second known failure.
- Two refactors the closing review named are deferred to after merge (ruling 11); the main session files a follow-up issue.

## Principle 9

| Surface | Change |
|---|---|
| JSON reports, exit codes | No new exit code; four new refusals on 78, 79 and 65; `release` and `publish` gain the always-present `floats_kept` |
| `ErrorReason` | `major-downgrade` appended |
| CLI flags | `--allow-downgrade` on `add`, `update`, `lock`; none on `release` or `publish` |
| Lock, config and published schemas | None; 0.15.1 files read unchanged |
| Dependency-free lock | Byte-identical: no edge means no claim, and rows D1, D2 and D4 are today's |
| `status --check` | Range-claimed dependency rows answer by `T(line)` and stop reading as a split |
| Agreement harness (`src/resolve/pin_agreement_tests.rs`) | `exit-agrees` gains two named exceptions: `MajorDowngrade` and the pin form of `UnchosenRequirement`, which a relock refuses where a fresh resolve has no pin. `relock-error` admits 79 `NoMatchingVersion` and the two 65 budgets beside 78. The fake registry gains annotations and tag listings, and `--allow-downgrade` joins the writers |
| Release mechanics (stability.md, "release/push mechanics") | A cascade float no longer moves to a lower version (Decision 9). The owner accepted this behaviour change with Q1 on 2026-11-12. `tags` keeps its meaning. A real cascading `release`, and every cascading `publish` entry, read the floats before any tag moves and fail closed (69, 80 or 81) where the registry cannot be read. A dry run keeps 0.15.1's exit code: when a float read fails, or offline, it warns once on stderr that it could not check the floats and plans every float moving, as 0.15.1 planned, with `floats_kept` `[]`. `--no-cascade` and `grim build` read nothing. Ruled by the autonomous run's main session; not put to the owner |
| Pin answers (stability.md, "keeps every pin whose request is unchanged"; commands.md § Which pins stay, "the same tag") | A bare direct line is answered by any tag of its repository (Q2), and a `requires` version claim by version. stability.md reads "keeps every pin that still answers its request"; commands.md adds both answers beside "the same tag" |
| Behaviour (upgrading.md) | A `requires` diamond within one major resolves (unreleased feature). A bare direct line yields to a `requires` major claim. A moved pin may record a cascade tag where a requester named `2.1`. An edit from `x:2` to `x` keeps the pin until `grim update`. `grim release` and `grim publish` leave a float on a higher version and name it in `floats_kept` |
| Docs and catalog drift | concepts.md (Decision 8), commands.md (§ Which pins stay, `--allow-downgrade`, release and publish), json-interface.md (`floats_kept`, `major-downgrade`), publishing.md § Cascade tags, upgrading.md, stability.md, artifacts.md; a drift review of the `grim-usage` and `grim-authoring` catalog skills (`catalog/README.md`) |

## Implementation Plan

1. Pure model: claim regex and `claim_of`, requirement sets, `T(line)`, `evidence`, `satisfies`, `highest_in_line`. Table tests.
2. `settle` split into selection and validation, predicates D1 to E6, permutation and rename invariance tests.
3. `Graph`: line nodes, learned lines, memos, the judge step, restart rule, kept-walk reset, work and listing budgets.
4. Pin answers rows (Range, Any) in graph carry; Decision 7 checks; downgrade guard; `DowngradeOpts`, `ResolveOptions`, TUI and export call sites.
5. Release float rule: read and compare before `move_tags` at its four callers (`src/command/release.rs` lines 361, 472, 563, 649); `floats_kept` on `ReleaseReport` and `PublishEntry`.
6. Errors, slug, `status --check`, agreement harness, docs and catalog drift (§ Principle 9), acceptance tests.

## Validation

Unit, by behaviour: `a_two_beside_a_two_one_resolves_to_the_newest_two`; `a_line_survives_a_new_minor_release`; `claims_settle_the_same_in_every_order_and_under_every_name`; `two_majors_on_one_node_exit_78_with_or_without_the_flag`; `two_majors_exit_78_before_the_ladder_can_report_79`; `a_tag_like_two_x_is_a_literal_claim`; `a_held_pin_inside_the_line_is_kept_by_lock`; `a_held_pin_below_a_new_lower_bound_moves_up_and_reports_moved`; `a_pin_whose_annotation_contradicts_its_tag_does_not_answer_a_requires_claim`; `a_recorded_edge_does_not_keep_a_pin_whose_evidence_fails_the_claim`; `an_unannotated_latest_pin_on_an_any_line_is_kept`; `a_retagged_release_candidate_is_skipped_by_the_listing`; `a_literal_or_digest_node_resolves_in_the_pass_that_reads_it`; `a_bare_direct_line_yields_only_to_a_same_repository_range_claim`; `a_bare_line_keeps_a_pin_recorded_under_another_tag`; `a_typed_direct_line_wins_over_every_claim`; `a_bundle_member_tag_keeps_its_literal_meaning`; `a_bundle_only_lock_answers_as_before`; `a_major_downgrade_refuses_without_the_flag_and_moves_with_it`; `a_literal_or_any_move_never_asks_for_the_flag`; `a_pin_moved_only_by_a_dropped_version_exits_78` (backported cascade); `a_line_emptied_by_a_dropped_version_exits_78_in_conflict_form`; `a_literal_conflict_release_never_triggers_the_dropped_claim_check`; `the_kept_walk_learns_nothing_from_the_line_walk`; `a_wide_chain_of_k_nodes_settles_in_k_passes_within_the_budget`; `a_work_budget_breach_exits_65_and_says_it_is_a_limit`; `an_offline_listing_is_a_miss_not_a_smaller_pick`; `an_offline_store_miss_keeps_the_held_pin_and_warns`; `a_claim_above_every_release_exits_79`; `a_repository_without_latest_resolves_through_its_claimed_major`; `a_bare_claim_without_latest_falls_to_the_listing`.

Release float rule: `a_backport_keeps_x_and_latest_on_the_newer_release_and_moves_x_y`; `a_float_without_an_exact_version_moves_as_before`; `a_float_outside_its_own_span_moves`; `a_missing_float_is_created`; `a_rerelease_of_the_same_version_moves_its_floats`; `force_moves_the_exact_tag_but_never_lowers_a_float`; `a_dry_run_reports_the_floats_it_would_keep`; `a_failed_float_read_moves_no_tag`; `floats_kept_is_always_present_and_empty_without_the_cascade`; `a_prerelease_current_is_compared_by_precedence`; `offline_a_cascade_dry_run_plans_every_float_moving_and_a_release_exits_81`; `a_dry_run_that_cannot_read_a_float_plans_every_float_moving`; `a_float_whose_version_tag_is_gone_or_elsewhere_moves`; `a_float_on_an_unparseable_manifest_moves`.

Acceptance: a 0.15.1 lock and config read and install unchanged; a dependency-free lock stays byte-identical after `grim lock`; **lock moved, disk refused**: `grim add X` moves dependency R whose files were edited and exits 65 after saving the lock, a repeated `grim add X --force` exits 65 again (force covers X only), `grim install --force` succeeds, and every pin is identical across the three runs; releasing 2.9.0 then 2.3.7 leaves `2` and `latest` on 2.9.0, moves `2.3`, and names both kept floats in `floats_kept`; the worked examples below, run end to end.

### Worked examples

**(i) Fresh resolve.** Root A requires `P` (no tag), `M:1`, `K:1`. P 3.0.0 requires `C:0.2`, P 2.0.0 nothing; M 1.0.0 requires `P:2`; K 1.0.0 requires `C:0`; C 0.2.0 requires `G:0.1`, C 0.5.0 requires `G:0.3`. Newest: P 3.0.0, C 0.5.0, G 0.3.0. A is a direct declaration; every other node is a line node.

| Pass | Candidates (line nodes from learned lines) | Judgement | Learned |
|---|---|---|---|
| 1 | P 3.0.0, M 1.0.0, K 1.0.0, C 0.5.0, G 0.3.0 (all `latest`) | P out (M claims `2`). C out (P's `0.2`). Neither teaches, so G is reached only through a non-teaching node | P [2, 3) from M; C [0, 1) from K; M, K [1, 2) |
| 2 | P 2.0.0 (`2`), C 0.5.0 (`0`), G 0.3.0 | All in line; P claims nothing now | G [0.3, 0.4) from C |
| Result | P 2.0.0, C 0.5.0, G 0.3.0, exit 0, two passes | | |

Renaming M to Z changes nothing, because teaching is reachability, not order. P 3.0.0's `C:0.2` never teaches, so C is not held at 0.2.0, and G sees no `0.1` claim.

**(ii) The oscillator.** Root A requires `N1` and `N2` with no tag, so both are Any line nodes. N1 3.0 requires `N2:1`, N1 2.0 nothing; N2 1.0 requires `N1:2`, N2 2.0 nothing.

| Pass | Candidates | Judgement | Learned |
|---|---|---|---|
| 1 | N1 3.0, N2 2.0 | N2 out (N1 claims `1`) | N2 [1, 2) from N1 3.0 |
| 2 | N1 3.0, N2 1.0 | N1 out (N2 claims `2`); N2 in line and teaches | N1 [2, 3) from N2 1.0 |
| 3 | N1 2.0, N2 1.0 (sticky) | All in line | none |
| Result | N1 2.0, N2 1.0, exit 0, with one stderr line: N2 held to 1.x by N1 3.0, which the resolve dropped | | |

A solver finds N2 2.0. Without stickiness (O4) the run cycles (3.0, 2.0), (3.0, 1.0), (2.0, 1.0), (2.0, 2.0), then back to (3.0, 2.0). O5 cycles the same way, because each line drops once its source leaves.

**D7. A dropped version refuses `grim add`.** Held: A requires `Q:1`, `U:1`, `P:1`. Q 1.6.0 (requires `R:1.5`), R 1.5.3, U 1.2.0 (requires `P:1`), P 1.0.0, all annotated. X requires `U:1.5`. U 1.5.0 requires `P:1.1`; P 1.1.0 requires `Q:1.8`; Q 1.8.0 requires `R:2`. Every learned line starts empty, so pass 1 keeps every held pin at step 1.

| Pass | Event | Learned |
|---|---|---|
| 1 | U 1.2.0 out (X claims `1.5`), released; Q 1.6.0 in line and teaches | U [1.5, 2); R [1.5, 2) from Q 1.6.0 |
| 2 | U 1.5.0; P 1.0.0 out, released | P [1.1, 2) from U 1.5.0 |
| 3 | P 1.1.0; Q 1.6.0 out, released | Q [1.8, 2) from P 1.1.0 |
| 4 | Q 1.8.0 claims `R:2`; R's line [1.5, 2) and [2, 3) is empty, and Q 1.6.0 is gone | — |
| Result | 78 `UnchosenRequirement`, conflict form, conflict form: R's claims `^1.5, ^2` cannot all hold, and Q 1.6.0, which claimed `1.5`, is no longer in the graph; fix: declare R as `R:2`, then rerun `grim add X`, or run `grim update` | |

A solver gives Q 1.8.0 and R 2.0.0. The refusal is the intended cost of Decision 5. A bare `grim update` starts every node at its newest and settles in one pass.

**`B:2` beside `B:2.1`** (2.1.4 and 2.3.0 released; `2` names 2.3.0). Today: different digests, 78. Now: line [2.1, 3.0), `T` = `2` gives 2.3.0, `pinned = B:2@…`. When 2.4.0 ships, `grim lock` keeps the pin (its annotation 2.3.0 satisfies the line) and `grim update` moves to 2.4.0.

**Bare direct `x` held at 3.1.0, a new dependent requires `x:2`.** `grim lock` exits 78 `major-downgrade`. `grim lock --allow-downgrade` moves x to 2.6.0, `pinned = x:2@…`, reported `moved`. Declaring `x` as `repo/x:3.1.0` keeps it, and the held-back warning names the dependent. If the dependent later leaves, the bare line keeps `x:2@…` (Q2) until `grim update` moves it to `latest`.

**Backport** (2.9.0, then 2.3.7). A release by this grim keeps `2` and `latest` on 2.9.0 and moves `2.3` to 2.3.7 (Decision 9), so a claim `2` takes 2.9.0. Where an older grim or another tool moved `2` to 2.3.7, a claim `2` takes 2.3.7, because it satisfies the line, and a claim `2.5` puts 2.3.7 below lo, so the listing gives 2.9.0.

### NFR coverage

- Scalability: claims add O(edges) per pass; one `T` resolve per tag and at most one listing per repository per run; graph caps unchanged; the work budget scales inversely with graph size.
- Availability: pins that answer need no network; offline reads the store and tag cache, and a needed listing is a clean offline miss.
- Latency: a `grim lock` whose pins answer makes zero extra calls; a fresh resolve adds passes where a `latest` falls outside its claims, bounded only by the work budget; a listing is one bounded walk; a cascade release adds at most three tag resolves and three manifest reads.
- Security: T1 (an annotation lie) is the publisher's statement inside the digest; T2 (a cascade tag moved) never moves a held pin on `lock`, and on `update` moves within the line only; wire-driven work stops at the budget with 65. Consent reach is unchanged: a moved hook or settings version is the recorded I2 version residual.
- Cost: no infrastructure; listings capped at 32 MiB per run.
- Operability: refusals name the node, versions, claim and a per-command fix; moves show as `moved` plus a stderr line; the budget message says it is a limit; a kept float is named in the report and on stderr.

## Open questions

None. The owner answered all three on 2026-11-12: Q1 option (a) is Decision 9 and the A table, Q2 option (a) is the Any row in § Pin answers and Decision 8, and Q3 F1 is the F table.

## Review record

One review round (spec, adversarial quality, cross-model), one fix pass and one re-validation, all on 2026-11-12. A final text pass applied the re-validation's seven findings; no review ran after it. Not measured: the work budget (16 × the node cap) and the 32 MiB listing cap are picks. The worked examples were run by hand only; no code ran.

## Links

`adr_artifact_dependencies.md`, `adr_lock_pin_stability.md`, `adr_unified_publish_version_cascade.md`, `.agents/research/research_artifact_dependencies_prior_art.md`, `.agents/research/research_dependency_version_claims_codebase.md`, `docs/src/content/docs/stability.md`, `docs/src/content/docs/concepts.md`.

## Changelog

| Date | Change |
|---|---|
| 2026-11-12 | Proposed: realizes the owner's eight decisions on dependency version claims; amends deps Decision 3 and the lock-pin pin rule, D1 and agreement clauses |
| 2026-11-12 | Revised from merged review: one evidence function; ordered settle predicates; a once-per-pass teaching judgement; a work budget instead of a pass bound; pin rule rows confined to graph carry; listing bounds; `status --check`; three new open questions |
| 2026-11-12 | Ladder step 3 also admits `X.Y` tags by span (found while planning: fixtures and third-party publishers that push only partial tags fell to 79). A widening; not reviewed |
| 2026-11-12 | A `latest` miss on a still-empty learned line is deferred, not final (found by the plan's spec review: a claim `x:1` on a repository tagged only `1` fell to 79 in pass 1). Restores today's result; not reviewed |
| 2026-11-12 | An Any line admits no prerelease tag from the listing (found while building the claim model: an unclaimed node on a repository with no `latest` landed on `3.0.0-rc.1` ahead of `2.5.0`). A narrowing of step 3 only; a `latest` tag that names a prerelease still answers. Not reviewed |
| 2026-11-12 | Two prerelease rulings made during execution. A float found on a prerelease always moves to the release (Compare row). A held prerelease pin counts in its release's compatibility group for the downgrade guard (`3.0.0-rc.1` is a 3.x pin). Not reviewed |
| 2026-11-12 | Release float rule, from its pipeline review: an unparseable manifest under a float is unknown evidence and moves (Failure row); an offline float read exits 81; the annotation residual and the dry-run registry dependency are recorded (Residual row, § Principle 9). Not reviewed |
| 2026-11-12 | Accepted by the owner at the hex-plan gate (Q1 a, Q2 a, Q3 F1). Final text pass: seven re-validation findings applied (line nodes, one held-pin test, empty-line 78, finish order, pass cost, one listing test, work counter); Decision 9, the release float rule; open questions closed; contract index added |
| 2026-11-13 | Held pin evidence (pipeline review, Block): a held pin whose annotation and tag show nothing takes its version from the edges the previous lock recorded onto it (evidence table, § Pin answers, offline), so a plain `grim lock` with unchanged config no longer moves a tagless pin when only the registry moved. These are rulings of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | `release_conflicts` wording and the D7 result text (review rulings): a recorded claim the pin no longer answers stays a 78 conflict, because releasing would move a pin on an unchanged config, and the worked example now matches the conflict-form template with one dropped parent. These are rulings of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | Decision 7 wording (review ruling): a Literal or Digest conflict's release exempts only the released key, so a deeper pin that would move only for a dropped version's claim exits 78 and the remedy names `grim update`. These are rulings of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | The graph pipeline's review held reading 1 (a node mixed a second time is a 78), 2 (evidence read per reference), 5 (a path parent narrows the line but records no source), 7 (the node cap counts admissions), 8 (the Work error reports passes over both walks), 9 (a learned line keeps a yielding bare direct), 12 (the downgrade guard's version and requesters), 13 (conflict-form `declare`) and 18 (a held tagless pin keeps its `kept_reference` stamp). Readings 6 and 15 held after a fix. These are rulings of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | Residuals and deferred items: a configuration change that drops the narrowest recorded edge can leave a pin stamped with a wider claim's tag, and an offline vouch records an edge that a later relock reads as evidence. Deferred: `line_reference` and `adopt_yielding_reads` reading `carry.pins` directly (O7), and the `resolve_targets` serial `T` resolve for a held pin that fails its line (performance note). These are rulings of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | Behaviour change: an export pick with numeric `req:2` on the same digest as `req:1` now exits 78, because bundle-member tags give evidence on a mixed node (reading 3) and one digest tagged in two majors is two majors on one node; the 2026-11-08 ruling predates the ADR, and the non-semver `req:v2` control still passes. Ruled by the autonomous run's main session; approved by the owner on 2026-11-13. |
| 2026-11-13 | Errors, `{fix}` (user-level bug hunt with the built binary): a `MajorDowngrade` fix named a `grim update` that refuses the same way, so it now names only the declare clause and the flagged command; `UnchosenRequirement` under `grim add` offers `grim update`, then rerun. Amends C-018. These are rulings of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | Errors, `NoMatchingVersion` text (bug hunt): the 79 tail contradicted itself when the newest tag's manifest names another version, and said "no release" for a repository holding only prereleases; the tail now says so. Amends C-017. These are rulings of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | Reports and `status --check` (bug hunt): a yielded bare direct row now answers by `T(line)` like a dependency row, and a `T(line)` digest answers `update_available: true` only when its evidence satisfies the line, so the advertised update matches what `grim update` does. These are rulings of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | Claim grammar (bug hunt): a version-shaped tag whose caret upper bound cannot be written is a Literal claim, because one claim can never conflict; prerelease admission over the whole set now holds in `satisfies`, `highest_in_line` and `merge_policy::validate`. These are rulings of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | Settle predicates E2 and Pin answers (bug hunt): on a node fixed by a Literal or Digest claim, Range and Any edges are judged, not resolved, so a second `grim lock` no longer rewrites the lock and no range edge exits 79 for a missing tag; a tagless digest-only pin stays tagless on relock. These are rulings of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | Read-order fix (bug hunt): a pin released only by a claim of a parent version the run then dropped now exits 78 whether that version was read in line or out of line. No rule changed; the code now matches the pin form. These are rulings of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | Residuals (bug hunt): concurrent `grim release` runs into one repository can leave a cascade float on a lower version, because registries have no compare-and-set for tags (documented; follow-up issue filed). Seven checks still test one range claim at a time, so a prerelease pin beside a plain claim can be misjudged there. On a node fixed by a digest or literal claim, a range edge's failed tag read refuses nothing. These are rulings of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | A cascading `release --dry-run` and `publish --dry-run` keep the exit code 0.15.1 gave (ruling 1). When a float read fails, or offline, the dry run warns once that it could not check the floats and plans every float moving with `floats_kept` `[]`; a real release still fails closed (69, 80, 81). Breaking changes are prohibited, so the earlier "exits 69, 80 or 81" reading is withdrawn. A ruling of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | The owner reversed the ruling that compared a float on a prerelease by semver precedence: a float on a prerelease always moves to the release, so the 2026-11-12 ruling stands (Compare row) ("latest is a tag and rc would never cascade to be latest for good reason"). |
| 2026-11-13 | A float is kept only while the exact tag of the version it shows still exists and names the float's manifest (ruling 3). A yanked or retagged release, or a float another tool moved, no longer holds a float: it moves with one stderr line. The cost is one tag resolve per float that would be kept. A ruling of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | The docs state that a float trusts the publisher's own version annotation, limited by the exact-tag check of the row above; a publisher who keeps both the annotation and its exact tag keeps the float (ruling 10). A ruling of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | `status --check` answers `null` (unknown), not `false`, where the line's cascade tag is gone, names a release outside the line, or names a manifest without version evidence (ruling 4): `grim update` would read the tag listing, which status does not. Ruled by the autonomous run's main session; approved by the owner on 2026-11-13. |
| 2026-11-13 | Behaviour change accepted (ruling 5, TEST-10): a `grim remove` of a direct key that held two numeric-tag claims of different majors apart (`p` asks `x:1`, `q` asks `x:2`) leaves `grim lock` at exit 78, lock untouched. The owner's earlier exit-0 case holds for literal tags only, since numeric tags are claims and two majors on one node refuse. Pinned by `test_remove_of_a_direct_key_over_a_numeric_two_major_conflict_exits_78`. Ruled by the autonomous run's main session; approved by the owner on 2026-11-13. |
| 2026-11-13 | A recorded `requires` range edge that no bundle lists reads as its full caret range, not as the shape of its tag (ruling 6): `x:2.1` shows [2.1, 3.0). Every caller reads it so: the lock carry, the part carry, the lock edit's stamp, marketplace member picks and export. A member pick that moves a claimed row inside the line keeps its dependent at exit 0, where 0.15.1 also exited 0. A ruling of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | A suspected false 78 on a solvable graph was investigated with a test (ruling 7). It reproduced and was fixed; the rule is the next row. A ruling of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | Reading of the autonomous run: a node whose read turns from its line to a literal or digest claim's manifest a second time gets one more pass, in which it selects after the late round's waves end; a third turn stays a conflict (78). It revises reading 1 of the 2026-11-13 row above. A read that fails below a line node's candidate waits for the judge and refuses only at a stop that keeps that candidate, and a line with no release waits for the pass's end, so a conflict (78) wins over 79 or 69. A pass count can grow by one for such graphs. A ruling of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | The 32 MiB listing cap stands as the record states and is counted in one unit, the bytes the walk charges (ruling 8). A ruling of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | A listing cut short by a repeated cursor, an empty page with a next link, an unfollowable link or the budget refuses with the listing-limit error (65) and never picks a release from the prefix (ruling 12). A ruling of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | Every `tags/list` page has its own 30 s deadline and no deadline spans the whole walk (ruling 9); the byte budget bounds the pages one walk reads. A ruling of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | The two refactors the closing review named are deferred to after merge, and the main session files a follow-up issue (ruling 11). Suggest-tier items left unfixed are listed under Technical Details, Residuals. A ruling of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | Writer rule: a lock edit that removes a once-declared key keeps its row, and leaves the lock stale, when the pin's tag does not rule out a remaining recorded `requires` range claim (§ Pin answers). The next `grim lock` keeps the pin if its version answers the claim, else moves it. A tagless pin and a request a bundle lists keep the old rule: the row drops. Amends `adr_lock_pin_stability.md` § The pin rule, the clause that drops the row of a key whose declaration left. The restamp of a kept tagless pin takes the recorded edge whose caret range shows its version. A ruling of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | Errors, `{fix}` (found by review): a conflict refusal names a tag that exists (the surviving claim's own tag, else the newest listed tag in the surviving line, else the cascade tag this run resolved, else the narrowest claim's tag). A major downgrade that only a dropped version's claim forces still needs `--allow-downgrade`, and its requesters name the dropped versions. Remedies carry the flagged alternative where an update may meet a downgrade, repeat the typed `grim add` flags, name the TUI row's binding, and a conflict-form named update gives the declare clause alone. A ruling of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | Offline, a held pin recorded under a float that only overlaps the claimed line (`x:1` under `^1.2`) is kept by its recorded edges or the lock's word instead of being released into a listing miss (69); the warning no longer says its tag names no version. A span wholly outside the line still releases it. (Found by review.) A ruling of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | `status --check` and the held-back warning (found by review): a bare direct row builds its line from same-repository Range edges only; a key fixed by a Literal or Digest edge judges a split on the fixing references alone; every manifest read of one run shares the 2048 cap, the 8-wide bound and the 300 s deadline; a range request is judged beside the range claims the pin meets alone, so a request only those lenders admit is answered, not held back. A ruling of the autonomous run's main session, not reviewed by the owner. |
| 2026-11-13 | Pin answers: `grim remove` and `grim uninstall` relock in the same run, so a row kept "until the next `grim lock`" (including `Answered::MayHold`) is settled there; `--no-relock` keeps the in-place edit and the stale lock. Decided by the owner on 2026-11-13. |
| 2026-11-13 | Release float rule: a float whose current version is a prerelease always moves to the new release; a float on a normal release never moves lower. Decided by the owner on 2026-11-13. |
| 2026-11-13 | The flag name `--no-relock` and the TUI delete (it relocks the same way, changes nothing on a failed relock and shows the CLI remedy). Details chosen by the main session, not reviewed by the owner. |
| 2026-11-14 | `grim update --marketplace M <plugin>` (or `<plugin>:<member>`) relocks, in the same run, every unselected part whose declaration changed since it was locked, keeping each pin its includes still answer and refreshing nothing; a part in line is still carried byte for byte. Decided by the owner on 2026-11-14 (an operation meant to hold toml and lock in sync must not yield a stale lock). Exit change against 0.15.1: a run that exited `0` there now exits with the relock's code when that part cannot be relocked. Details chosen by the main session, not reviewed by the owner: that failure writes nothing and uses no new exit code; one stderr warning names the plugin first; the relock happens whenever the run changes the lock, so `grim export plugin --plugin` that saves `marketplace.lock` relocks stale parts too, and a run that changes nothing reads none; a declared plugin with no lock part yet stays unlocked by a run that did not pick it. |
| 2026-11-15 | A marketplace run that changes the lock (`grim update --marketplace M <plugin>`, a declared `grim export plugin --plugin` or `grim export marketplace` that saves `marketplace.lock`) also locks a declared plugin that has no part yet, resolving fresh, after one stderr line. A run that changes nothing still locks nothing; a failure exits with that plugin's code and writes nothing. Supersedes the "stays unlocked" clause of the 2026-11-14 row. Decided by the owner on 2026-11-15. |
| 2026-11-14 | A plain `grim remove` or `grim uninstall` relocks even where that writes `[[<kind>.requires]]` rows that grim 0.15.1 cannot read (0.15.1 then exits `78` on `status` and `install`). Accepted by the owner on 2026-11-14: `grim add` and `grim lock` already write those rows, `--no-relock` keeps the in-place edit for a team that stays on 0.15.1, and the rows appear only in a project whose artifacts declare dependencies. In the same discussion the owner kept the lock format as it is: `lock_version` stays `1`, no `[[edge]]` section, and every lock struct keeps `deny_unknown_fields` (`adr_artifact_dependencies.md`, option L3 stays rejected). The TUI delete has no way to skip the relock; left as is. |
| 2026-11-15 | The global flag `--locked` (env `GRIM_LOCKED`): a command that writes `grimoire.lock` or `marketplace.lock` refuses an out-of-sync lock at the start of the run and changes nothing; the command's own edit still relocks. The flag is decided by the owner. Approved by the owner with the plan on 2026-11-15: its meaning differs from Cargo's (the lock must be in sync before the run, not unchanged after it); the env pair `GRIM_LOCKED`; a missing project lock refuses with `79`; `grim lock --locked` follows the same rule; a marketplace part that is missing counts as stale (`65`). Chosen in this pass, not reviewed by the owner: the check reads the lock itself under the flag, so an unreadable lock refuses with its own code; it runs per command even when the run writes nothing, so its error comes first; the remedies say "without --locked or GRIM_LOCKED", so they work with the variable exported; a missing lock under the flag adds that context line; a marketplace part for a plugin the manifest no longer declares refuses too; every declared plugin is checked, not only the selected ones, and the first in manifest order is named; the TUI covers Install, Update, Delete and the local and member deletes. |
| 2026-11-15 | A lock write warns on stderr the first time the lock carries dependency rows (`requires`), because grim 0.15.1 and older cannot read such a lock. The exit code and the JSON output do not change. The warning is decided by the owner on 2026-11-15; its trigger (the transition, including a lock that did not exist before), its text and its seam (`lock_io::save`, `lock_io::save_marketplace`) are chosen in this pass, not reviewed by the owner. |
