# Discussion: Lock pin stability

State: handed-off → loop · Updated: 2026-11-03
Participants: owner and hex-discuss; one session, 2026-11-03
Ratified: 2026-11-03 → loop
Confidence: ratified by the owner; research vintages 2026-11-03 (codebase, prior art, reconcile, community, archaeology; council of three seats). In-session reproduction of the shared-reference carry bug only; the other defects and the council's stories rest on code reading.

## Intent

In the owner's words: "`grim lock` was never meant to update." Only `grim update`
may move an existing lock pin. Today `grim lock`, `grim add` (via the stale-lock
fallback to a full resolve in `src/command/add.rs` `relock_entry`) and every
"run `grim lock`" recovery re-pin existing entries to the tag's current digest.
The owner classes fixing this as a bug fix, not a breaking change.

The owner wants grim to adopt the model from ocx: the lock records which
reference resolved each entry; if that reference did not change and no update
was issued, the entry is left untouched, and an entry whose reference is new is
known to be newly added.

Second strand, in the owner's words: "our implementation on what to
update/install lock and so forth got rather complicated — I always thought we
reduce complexity with simple set theory where we have a current state and a
target state and derive mutations from that, and the target state computation is
what makes the selected operation (`update [<name>...]`, `lock`, `add …`)."

Third strand, in the owner's words: "we have not enough acceptance tests for
bundles and dependencies, especially for edge cases … run a few (3-5) subagents
that try to find edge cases and fix errors on the fly … from user perspectives
doing weird things, or having old, new, clean, dirty states … put a lot of
effort in there but do not overdrive acceptance tests: try some out, and if they
are reasonable or detect something, add them (maybe even implement sharding like
in ocx with junit merge after, for faster CI)."

Why now: three defects on `main` (`03393c5`, after the artifact-dependencies
merge):

1. **Re-pin.** `grim lock` calls `roll_forward(.., &[], ..)` →
   `build_carry` returns before filling `digests` when nothing is selected
   (`src/resolve/resolver.rs:349`) → every node, transitive ones included, is
   resolved fresh. `grim add` always changes `declaration_hash`, so
   `resolve_lock_partial`'s stale guard fires and `relock_entry` falls back to a
   full `resolve_lock` (`src/command/add.rs:1391-1401`); bundle add always fully
   resolves (`add.rs:1364`). The TUI uses the same seam.
2. **Issue #269.** `grim update <names>` re-resolves only the names, then
   installs, integrity-checks, prunes and reaps the whole lock
   (`src/command/update.rs:176-277`): a deleted entry of an unnamed binding is
   rewritten and reported `unchanged`; a locally modified unnamed entry makes the
   run exit 65. https://github.com/grimoire-rs/grimoire/issues/269
3. **Shared-reference carry bug.** `Carry.digests` is keyed by requested
   `Identifier` (`src/resolve/graph.rs:479-484`) and seeds the resolve memo, so
   when unnamed `b` requests the same reference as named `a`, `build_carry`
   (`resolver.rs:378-388`) stores `b`'s old digest under that reference and
   `grim update a` makes **zero** registry calls and keeps `a`'s old digest.
   Reproduced in-session with a unit test: two skills `a`, `b` both declared
   `ghcr.io/acme/s:1`, previous lock pins both at digest `old`, lock hash
   current; `resolve_lock_partial(["a"])` with a `MockAccess` scripted to return
   `new` → `calls=0`, `a` stays at `old`. (Test was reverted; rewrite it in
   `src/resolve/resolver.rs` tests next to `partial_carries_forward_unselected_entries`.)

Out of scope: the dependency model itself (`.agents/adr/adr_artifact_dependencies.md`
is Accepted and stays, amended only as listed below), hook trust and consent
(`adr_hook_workspace_consent.md`), OCI transport and credentials, the
`grim update <names>`-on-a-stale-lock exit 65 (kept as is), any change to
`grim install` pruning (install never prunes; that contract stays), and release
work (version bump, changelog, tag, publish, including the 0.16.0 release gates).

## Requirements

Provisional — the loop assigns IDs.

**Lock semantics**

- A lock pin moves only when its node is in the command's refresh set, or the
  reference now requested for it does not match the reference that resolved it.
  Everything else carries forward, even when its tag moved.
- Per-command refresh sets: `grim lock` → nothing; `grim add X` → nothing (X
  resolves because it is new or its reference changed); `grim remove` → no
  resolve; `grim update` → all; `grim update A…` → {A…}. The TUI install/update
  actions and marketplace locking follow the same rule.
- The resolving reference is recorded **in `pinned`** on every lock entry,
  top-level and transitive: stop stripping the tag on write
  (`strip_advisory` at `src/lock/grimoire_lock.rs:571`), so `pinned` reads
  `registry/repo:tag@sha256:…`. For a transitive node it is the winning edge
  request, resolved against the parent's current pin. No new lock field, no
  `lock_version` change: the published schema already allows an advisory `:tag`
  before the digest (`src/lock/testdata/lock.schema.json:290`), and v0.15.1's
  `Identifier::parse` reads `repo:tag@sha256:…` (its identifier tests). A
  separate `declared` field (OCX v4) was rejected because every grim ≤ 0.15.1
  parses with `deny_unknown_fields`, which would break
  `docs/src/content/docs/stability.md` › Forward compatibility ("a registry-only
  lock … stays byte-identical across the version boundary"; "a lock without
  dependencies … reads on both sides").
- Match rule, requested reference R vs recorded pin P:
  - R has a digest → match iff same registry/repository, same tag (or both
    none), same digest. Covers a digest-pinned `grimoire.toml` entry and a
    `--pin`-frozen bundle or requires member (`repo@sha256:…`).
  - R has no digest → match iff same registry/repository and same tag.
  - `or_latest` (`src/oci/identifier.rs:270`) adds `latest` only when there is
    neither tag nor digest, so a tagless P means a digest-only request or a
    pre-change lock.
  - Removing a digest constraint (`repo:1@sha256:D` → `repo:1`) keeps D until
    `grim update <name>` (P `repo:1@D` matches R `repo:1`), as Cargo keeps 1.2.3
    when `=1.2.3` loosens to `1`.
- Pre-change locks (every `pinned` tag-stripped): if `declaration_hash` matches,
  stamp the current requests onto the existing pins with no registry contact; if
  it drifted, resolve everything once and warn, naming the cause.
- Carry is keyed per node — (kind, name) plus the recorded reference — never by
  reference alone; a selected node never reads a carried pin; the fresh-resolve
  memo stays keyed by reference so one tag is resolved once per run.
- Declared bundles carry by pinned digest while repo and tag are unchanged, with
  members re-read from the manifest by that digest (store first). Edges always
  come from the digest-verified manifest, never from the lock's `requires`
  online (ADR Decision 3 invariant kept).
- Selected names match per kind where the caller knows the kind; `keep`
  (`resolver.rs:375-378`) must not skip carry for a same-named node of another kind.

**Command architecture** (council of three seats, 3/3; owner adopted)

- Lock half: one function computes the target lock. Replace
  `Option<&[String]>` / `names.is_empty()` with an explicit
  `Refresh { All, Names, Nothing }`; fold `resolve_lock_partial`,
  `relock_entry` and `relock_declared` into it. It returns per-entry
  dispositions (added / carried / moved / dropped) — a pure `plan_lock` —
  that feed the `lock`, `add` and `update` reports.
- Install half: one install-scope set handed to every pass — install, integrity
  gate, prune, reap, dev refresh, vendor sync, hook converge — and `--force`
  scoped to it via `ForceScope::Only` (`src/install/installer.rs:54`).
  `grim update <names>` uses the union of `reachable_lock(new_lock, node)`
  (`src/command/add.rs:807`, move it to a shared module) over the named nodes.
  No up-front target install-state planner: `install_one` keeps deciding per
  client from on-disk bytes, and the destructive passes keep their guards
  (shared MCP JSON refcount `shared_by_surviving_sibling`, dev-install
  exemption, raw-config client reap, hook consent, "install never prunes",
  "remove never deletes").
- On `grim update <names>`, prune deletes only nodes that were in the named
  bindings' closure in the previous lock and are gone from the new lock; reap of
  dropped clients is skipped.
- Extract persist → retired outputs → vendor sync → hook converge from
  `install_and_persist`; `update` stops hand-copying it.
- Reports keep their frozen `action` meanings (`LockAction` `locked`/`unchanged`,
  `UpdateAction` literals). Dispositions reach JSON only as additive fields or
  new enum literals; `UpdateAction` gains a value for "rewritten at the same pin"
  (a restored output), derived from the install outcome, not the lock diff.
- Hygiene carried in (operability seat): `lock_io::load(..).ok()` in
  `lock`/`update`/`add`/TUI must stop turning an unreadable lock into a silent
  full re-resolve; `grim update <names>` with no previous lock must not silently
  resolve everything.

**Marketplace** (owner: fully in scope)

- `marketplace.lock` follows the same pin rule. `src/export/resolve.rs:361-386`
  passes no previous lock for whole picks and for members picks with no
  existing part, so today every `grim update --marketplace` and export stage
  re-resolves them.

**Edge-case hunt and acceptance tests**

- After the core fixes land, one wave of 4 hex `simulator` workers (first-time,
  power-user, adversarial, automation) plus 1 state-matrix tester exercises
  bundles and dependencies against old / new / clean / dirty lock and install
  states (a 0.15.1-written lock, a pre-change lock, a drifted lock, deleted and
  hand-edited outputs, moved tags, digest-pinned and `--pin`-frozen members,
  diamonds, nested bundles, offline).
- Every finding becomes a failing test first, then a fix. A probe becomes a kept
  acceptance test only if it found something or covers a case nothing else does;
  no speculative suite growth.
- Last milestone: shard the acceptance suite in CI with a JUnit merge job, after
  ocx's `.github/workflows/acceptance.yml` (plan → binary → shards → one merge
  job judging the suite floor and xfail ceilings across all shards). Today grim
  runs one job with `pytest -n auto` (`test/taskfile.yml:107`) and uploads one
  `test/results/junit.xml` (`.github/workflows/verify-basic.yml:178-185`,
  `verify-deep.yml`).

**Autonomy and end state** (owner: "there should never be a feedback ask to the
user until the full goal is reached")

- The run is fully unattended: no question to the owner at any milestone until
  every milestone is reached. `## Open questions` are decided by their
  `Recommended:` answers; a blocker becomes deferred residue in the final report,
  never a question.
- End state for grim: one PR on GitHub, recomposed by `/hex-finalize` into a
  linear Conventional-Commits series, CI green, **unmerged**. The loop watches CI
  after the push and fixes forward, re-finalizing, until it is green. Push and
  force-push are limited to the goal branch. The merge is the owner's.
- Release: **none**. The goal stops at merge-ready PRs: no version bump, no
  `task release:prepare`, no tag, no publish.
- Push is otherwise forbidden (`.claude/rules/workflow-git.md:22`), so the push,
  force-push and open-PR grants, for all three repositories, must go in the
  `/hex-loop` `{grants}` of the pasted prompt; pasting it is the owner's authority.

**Adoption in the other repositories** (owner: final milestones, one PR each,
after the grim PR is ready)

- `~/dev/grimoire-rs/vscode` (`grimoire-vscode`, v0.3.9, drives `grim` through
  its JSON interface, `src/grim.ts`): adopt grim's changed JSON — the restored
  update action, lock dispositions, dependency `required_by`/`direct` on status —
  and verify its Update button against the scoped `grim update -- <name>`.
- `~/dev/grimoire-rs/indexer` (`@grimoire-rs/indexer`, v0.6.3, `src/enrich/`):
  show declared dependencies (the `requires` layer) alongside bundle members.
- Each repository gets its own finalized PR, CI green, unmerged. Each follows
  that repository's own rules and gates; read its `AGENTS.md`/`CLAUDE.md` first.
- Each depends on grim's unreleased changes: pin its tests to a grim built from
  the goal branch, and state that dependency in the PR body so the merge order is
  grim first.

**Contracts that change with the fix** (docs are contracts; bug-fix classification)

- `docs/src/content/docs/concepts.md:245` — dependencies "re-resolved on each `grim lock` and `grim update` like any other".
- `docs/src/content/docs/commands.md` `#lock` ("Resolves the floating tags … to concrete digests"), `#update`, line ~95 ("must re-resolve the lockfile on every change"), line ~880 (bundle members "keep their own pin").
- `docs/src/content/docs/guides/lifecycle.md`, `guides/versioning.md`; `stability.md` gains a clause that `grim lock` keeps pins.
- `.agents/adr/adr_artifact_dependencies.md` Decision 3: "Declared bundles are re-fetched as today" → carried by pinned digest; partial-resolve carry generalizes to every lock write.
- `test/tests/test_dependencies.py:88-95` asserts `grim lock` rolls a moved transitive tag forward; it flips to asserting the pin is kept.
- Catalog skills `grim-usage`, `grim-authoring`: drift review (`catalog/README.md`).

## Decisions

- Outcome: a goal loop (drain → `/hex-loop`).
- Record the resolving reference in `pinned`, not a new field (owner first
  picked `declared`, then accepted `pinned` after the `stability.md` conflict
  surfaced; owner raised the digest-pinned edge case, resolved by the match rule).
- Drifted pre-change lock: resolve everything once, with a warning.
- Unpinning a digest keeps the pin until `grim update <name>`.
- Council shape adopted: set model for the lock (explicit refresh set, per-node
  carry, dispositions) plus one install-scope set for every pass; no install planner.
- Scoped prune = old closure minus new; no reap on a scoped update.
- Marketplace fully in scope.
- Edge-case hunters run once, after the core fixes; sharding is the last grim milestone.
- Fully unattended until every milestone is reached. End state: one finalized,
  linear, CI-green, unmerged PR per repository (grim, then vscode and indexer).
  No release work.

## Threads

- Where the resolving reference is recorded — **closed**: in `pinned`.
- Lock compatibility for a new field — **closed**: no new field, `lock_version` stays 1.
- Command architecture — **closed**: council shape.
- #269 scoped update — **closed**: closure scope, scoped force, scoped prune, no reap.

## Research

- `.agents/research/research_lock_pin_stability_codebase.md` — lock data model, `declaration_hash` inputs and comparators, every lock writer, `build_carry`/`Carry`, ADR-vs-code divergences, "run grim lock" messages, test gaps.
- `.agents/research/research_lock_pin_stability_prior_art.md` — package-manager prior art in five families; addendum on OCX lock v4 from the local checkout.
- `.agents/research/research_lock_pin_stability_reconcile.md` — desired-state reconcilers: lockfile tools restrict the target; graph tools filter a whole-state diff; prune scope is where accidents happen.
- `.agents/research/research_lock_pin_stability_community.md` — practitioner threads: helm#11308, Poetry 2.0 default flip, Bundler `--conservative`, Cargo branch-dep staleness, tag-repointing incidents.
- `.agents/research/research_lock_pin_stability_archaeology.md` — full re-resolve on `grim lock` is the OCX-port default (`936aed9`), never decided; partial stale guard is a doc comment only (`3d2ed57`); `update <names>` installing everything is the original shape; `adr_effective_set_mutations.md` already applies a before/after set model to remove/uninstall/TUI delete.
- Council (premortem, operability, simplicity) returned inline; synthesis is under Requirements › Command architecture.

## Related

- Issue #269 — https://github.com/grimoire-rs/grimoire/issues/269
- `.agents/adr/adr_artifact_dependencies.md` (Decisions 3, 4), `.agents/adr/adr_effective_set_mutations.md`
- `src/resolve/resolver.rs` (`build_carry`, `resolve_graph`, `resolve_lock_partial`, `roll_forward`), `src/resolve/graph.rs` (`Carry`, `resolve_digests`)
- `src/lock/grimoire_lock.rs` (`project`, `strip_advisory`), `src/oci/pinned_identifier.rs`, `src/oci/identifier.rs`
- `src/command/{lock,add,update,remove,install}.rs`, `src/tui/app.rs`, `src/install/installer.rs` (`ForceScope`, `install_all_with_progress`, `install_and_persist`), `src/install/prune.rs`, `src/export/resolve.rs`
- `src/api/artifact_status.rs` (`LockAction`, `UpdateAction`), `src/api/update_report.rs`
- OCX lock v4 (local checkout `~/dev/ocx-sh/ocx`, commit `7378fe0b`): `crates/ocx_project/src/lock.rs`, `crates/ocx_project/src/resolve.rs` (`resolve_lock_reconcile`), `.github/workflows/acceptance.yml` (sharding)
- Note: this worktree needed `git submodule update --init` (`external/*`) before cargo could build.

## Open questions

- [NEEDS CLARIFICATION: For a diamond whose agreeing edges request different tags (`B:1`, `B:1.2`, same digest), which tag does `pinned` record?] Recommended: the settled `winner_id` (`Settled::Agreed`, `src/resolve/graph.rs`) — it is already deterministic; a later change of either edge re-resolves B, which is conservative.
- [NEEDS CLARIFICATION: Name for the new `UpdateAction` literal for a restored output.] Recommended: `restored` — matches issue #269's suggestion.
- An unknown name passed to `grim update` — current behaviour unverified; check before changing it.

## Verification

- Regression tests first, each failing on `03393c5`:
  - unit: the shared-reference probe above; `grim lock` with no edit keeps every pin after the tag moved (scripted `MockAccess`); `add` of a new entry keeps all other pins; tag edit `:1` → `:2` re-resolves; repository change re-resolves; digest-declared and `--pin`-frozen members match by digest; unpin keeps D; pre-change clean lock stamps with zero access calls; pre-change drifted lock resolves all and warns.
  - acceptance (`test/tests/`): retag a floating tag between two lock writes, then `grim lock`, `grim add other`, a hand edit plus `grim lock`, `grim remove` plus `grim lock`, bundle add, TUI install → old pins kept; #269 case A (deleted unnamed entry untouched, reported nowhere) and case B (modified unnamed entry, exit 0), each also with `--global`; a modified dependency inside the named closure still exits 65; scoped prune removes a member the named bundle dropped and nothing else; marketplace `grim update --marketplace` and export stage keep unchanged picks.
  - flip `test/tests/test_dependencies.py:88-95`.
- 0.15.1 interop: a lock written after the change parses on a grim 0.15.1 binary (the `verify-deep.yml` old-binary run / `test/tools/p9_diff.py`).
- `task verify` green; `task docs:check` for the docs edits; `task catalog:verify` after the catalog drift review.
- Edge-case wave: each kept acceptance test names the finding that justified it.
- Sharding milestone: the merged JUnit report contains every test exactly once and the suite floor holds.
- End state: `gh pr view` per repository shows the PR open, unmerged, mergeable and all required checks green; `git log` on each branch is linear Conventional Commits on top of its base.
- vscode and indexer: each repository's own gate passes, and the vscode Update button is exercised against a grim built from the goal branch (case A and case B from #269).
