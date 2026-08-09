# Plan: `grim publish` — manifest-driven batch release

## Status

- **Plan:** plan_grim_publish
- **Status:** Approved
- **Active phase:** 5 — Migration, Docs & Review (complete)
- **Step:** awaiting /swarm-review
- **Last update:** 2026-06-12 (after 3cefa1a: rebrand logo with flat spellbook-and-spark mark)

---

## Overview

**Status:** Draft
**Author:** /architect (Claude)
**Date:** 2026-06-12
**Related ADR:** [adr_grim_publish.md](../adr/adr_grim_publish.md)
**Research:** [research_publish_manifest.md](../research/research_publish_manifest.md)

## Objective

Built-in `grim publish` command: read a `publish.toml` manifest of packages
(`[skills.name]` / `[rules.name]` / `[agents.name]` / `[bundles.name]` tables
with `version`, optional `path`, bundle-only `pin`), release each via the
existing `release::run` seam in fixed kind order, default skip-existing.
Replaces `catalog/scripts/publish.py` entirely.

## Scope

### In Scope

- `grim publish` with `--manifest`, `--only`, `--tag`, `--dry-run`, `--force`,
  `--registry` (ADR D1–D5)
- `PublishReport`/`PublishEntry`/`PublishStatus` report type (ADR D6)
- Bundle-vs-manifest guard-rail errors both directions (ADR D7)
- Catalog migration: `publish.toml` new format, delete publish.py, taskfile +
  CI workflow updates
- Docs + AI-config drift: `docs/src/publishing.md`, `docs/src/commands.md`,
  `product-context.md` CLI glance, `subsystem-cli-commands.md` index,
  `grim-usage` skill drift review

### Out of Scope

- Parallel pushes (registry index writes deliberately serialized)
- Topological bundle-member ordering (fixed kind order suffices)
- Manifest-driven channel-tag lists (cascade tags + `--tag` cover it)
- Extraction of `push_artifact()` shared fn (wait for third caller)

## Research

**Research artifact:** [research_publish_manifest.md](../research/research_publish_manifest.md)

Skip-existing default = helm chart-releaser CI norm; keyed per-entry tables =
release-plz model; filename + structural schema disambiguation (no marker key);
fail-fast + `--only` for surgical recovery.

## Technical Approach

### Architecture Changes

```
grim publish (clap: PublishArgs)
  → read_capped(manifest) → PublishManifest (serde, deny_unknown_fields)
  → validate whole manifest (semver, paths exist, pin only on bundles,
    --only names known, --tag non-semver)   [fail before side effects]
  → for entry in skills→rules→agents→bundles (alpha within kind):
      build ReleaseArgs { path, "{registry}/{ns}/{name}:{tag|version}",
                          kind, dry_run, force, skip_existing: !force, pin }
      release::run(ctx, &args)  → push PublishEntry into PublishReport
      on Err: entry status=failed, stop; exit = classify_error(err)
  → (PublishReport, ExitCode)  → render via existing app.rs arm
```

### Key Decisions

| Decision | Rationale |
|----------|-----------|
| Compose `release::run` per entry | Same code path publish.py exercised via subprocess; zero drift (ADR D5/O2) |
| Skip-existing default, `--force` exclusive | Idempotent CI re-runs; semver tags never move silently (ADR D3/O1) |
| Filename + schema disambiguation, guard errors both parsers | Schemas already disjoint; marker key = ceremony (ADR D7/O3) |
| Whole-manifest validation before first push | publish.py parity; no partial side effects from a typo'd entry |
| Fail-fast with rendered partial report | Report shows pushed/skipped/failed rows even on error; JSON-consumable in CI |

## Implementation Steps

> Contract-First TDD: Stub → Verify → Specify → Implement → Review.

### Phase 1: Stubs

- [ ] **Step 1.1:** `src/command/publish.rs` — `PublishArgs` (clap),
      `PublishManifest`/`PublishEntrySpec` (serde), `pub async fn run(ctx, args)
      -> anyhow::Result<(PublishReport, ExitCode)>`, helper signatures
      (`plan_entries`, `validate_manifest`) with `unimplemented!()`
- [x] **Step 1.2:** `src/api/publish_report.rs` — `PublishReport`,
      `PublishEntry`, `PublishStatus` + `Printable` impl shell;
      `pub mod publish_report;` in `src/api.rs` (actual layout is
      `api/*_report.rs`, not `api/data/` — plan corrected post-stub)
- [ ] **Step 1.3:** Wiring — `pub mod publish;` in `src/command.rs`,
      `Publish(PublishArgs)` in `main.rs`, dispatch arm in `app.rs`
- [ ] **Step 1.4:** Visibility — make needed `release.rs` seams reachable
      (`run` already pub; check `parse_reference` need — reference is built
      as a full string, so likely none)

Gate: `cargo check` passes.

### Phase 2: Architecture Review

`worker-reviewer` (spec-compliance, post-stub) vs ADR D1–D7. Feature touches
>3 files — review NOT optional.

### Phase 3: Specification Tests

- [ ] **Step 3.1:** Unit tests in `publish.rs` (from ADR contracts):
      manifest parse happy path; bad semver; missing source; `pin` on
      non-bundle; bundle-shaped file → guard error; `--only` unknown name;
      `--tag` semver rejected; ordering (skills<rules<agents<bundles, alpha);
      reference construction with `--tag` override; end-to-end batch against
      `MemoryRegistry` (skip-existing + force + dry-run)
- [ ] **Step 3.2:** Unit test in `build.rs`: `read_bundle_members` on
      registry-keyed TOML → publish-manifest hint error
- [ ] **Step 3.3:** Acceptance tests `test/tests/test_publish.py` (fixtures:
      `grim_at`, `registry`, `unique_repo`): publish all kinds; re-run skips
      existing; `--dry-run` pushes nothing; `--force` moves tags; `--only`
      filters; `--tag canary` movable; semver `--tag` exits 65; missing
      manifest exits with data error; JSON report shape; bundle published
      after member skills resolvable

Gate: tests compile + fail `unimplemented`.

### Phase 4: Implementation

- [ ] **Step 4.1:** Manifest parsing + validation (`read_capped`, serde,
      whole-manifest checks, guard rails)
- [ ] **Step 4.2:** Planning + iteration loop calling `release::run`,
      report assembly, fail-fast classification
- [ ] **Step 4.3:** `PublishReport` rendering (single table, static headers;
      JSON bare array)

Gate: `task rust:verify` + acceptance tests pass.

### Phase 5: Migration, Docs & Review

- [ ] **Step 5.1:** Migrate `catalog/publish.toml` to per-entry tables;
      delete `catalog/scripts/publish.py`; update `catalog/taskfile.yml`
      `release` task → `grim publish`; update
      `.github/workflows/publish-catalog.yml`
- [ ] **Step 5.2:** Docs: `docs/src/publishing.md` (manifest section),
      `docs/src/commands.md` (publish entry)
- [ ] **Step 5.3:** AI-config drift (same commit discipline):
      `product-context.md` CLI glance (`release` vs `publish` split),
      `subsystem-cli-commands.md` index, `grim-usage` skill drift review per
      `catalog/README.md`, bump skill version in `publish.toml` if content
      changes
- [ ] **Step 5.4:** Review-Fix Loop on full diff; `task verify` final gate

## Files to Modify

| File | Action | Description |
|------|--------|-------------|
| `src/command/publish.rs` | Create | Command: args, manifest, loop |
| `src/api/publish_report.rs` | Create | Batch report type |
| `src/command.rs`, `src/main.rs`, `src/app.rs`, `src/api.rs` | Modify | Wiring |
| `src/command/build.rs` | Modify | Bundle-reader guard rail |
| `test/tests/test_publish.py` | Create | Acceptance suite |
| `catalog/publish.toml` | Modify | New per-entry format |
| `catalog/scripts/publish.py` | Delete | Replaced by built-in |
| `catalog/taskfile.yml` | Modify | `release` task calls grim publish |
| `.github/workflows/publish-catalog.yml` | Modify | Driver swap |
| `docs/src/publishing.md`, `docs/src/commands.md` | Modify | Document command |
| `.claude/rules/product-context.md`, `.claude/rules/subsystem-cli-commands.md` | Modify | Positioning + index drift |
| `catalog/skills/grim-usage/**` | Review | Drift duty (catalog/README.md) |

## Dependencies

No new crates: `toml` 0.8 + serde already in tree. No new services.

## Testing Strategy

### Unit Tests (component contracts)

| Component | Behavior | Expected | Edge Cases |
|-----------|----------|----------|------------|
| `PublishManifest` parse | Valid manifest → typed entries | All kinds, paths defaulted from convention | bad semver, unknown keys, bundle-shaped file, `pin` on skill |
| `plan_entries` | Order + reference building | skills→rules→agents→bundles, alpha; `{registry}/{ns}/{name}:{ver}` | `--tag` override, `--only` filter, unknown `--only` name |
| Batch loop (MemoryRegistry) | Per-entry release + report | pushed/skipped/dry-run statuses | second run all-skipped; `--force` moves; failure → fail-fast + classified exit |
| Amendment branches (review back-fill) | dry-run on already-published → `skipped`; `resolve_force_skip` 3-branch matrix; post-parse D7 fallback; `--registry` flag wins in reference; cross-kind `--only` | per max-tier review F1–F4/F6/F7 | added after ADR amendment landed post-Specify — amendment ⇒ same-commit test-table update (RCA Cluster A) |
| `read_bundle_members` guard | registry-keyed TOML | hint error → `grim publish` | — |

### Acceptance Tests (user experience)

| User Action | Expected Outcome | Error Cases |
|-------------|------------------|-------------|
| `grim publish` in catalog-shaped dir | All packages pushed, table report | missing manifest → data error 65 |
| Re-run `grim publish` | All rows `skipped`, exit 0 | — |
| `--dry-run` | No registry writes, `dry-run` rows | — |
| `--force` | Exact tags moved | — |
| `--only X` | Only X pushed | unknown name → 65 |
| `--tag canary` | Movable tag, manifest versions untouched | semver tag → 65 |
| `--format json` | Bare array, per-entry status | — |

### Manual Testing

- [ ] `task catalog:release -- --dry-run` against migrated manifest before landing

## Rollback Plan

1. Single branch `feat/grim-publish`; revert merge commit restores publish.py
2. Registry state append-only under default skip-existing — no remote cleanup
3. Re-run old publish.py from previous tag if an emergency catalog publish needed

## Risks

| Risk | Mitigation |
|------|------------|
| `ReleaseArgs` construction drifts from release CLI semantics | Reuse `release::run` directly; acceptance tests mirror test_release.py cases |
| Manifest format churn after public exposure | ADR records additive-evolution policy; provisional status accepted |
| CI workflow breaks catalog publishing | publish-catalog.yml is workflow_dispatch (human-triggered); dry-run job first; manual test before landing |
| `release::run` private helpers change later and break publish assumptions | Publish tests run full push path against MemoryRegistry — drift caught at unit level |

## Checklist

### Before Starting

- [x] ADR drafted (adr_grim_publish.md) — awaiting approval
- [x] Dependencies available (toml/serde in tree)
- [x] Branch created: `feat/grim-publish`

### Before PR

- [ ] All tests passing (`task verify`)
- [ ] Docs + drift review complete
- [ ] Self-review complete

### Before Merge

- [ ] Review-Fix Loop converged
- [ ] No merge conflicts with main

## Notes

- **Ambiguities resolved post-Specify** (oracle = publish.py semantics):
  - Reference namespace = plural kind (`skills/`, `rules/`, `agents/`,
    `bundles/`) — matches publish.py KINDS table and ADR D5 examples.
  - Version regex strict `^\d+\.\d+\.\d+$` (publish.py SEMVER) — build
    metadata (`1.0.0+x`) and prerelease both rejected.
  - `--only` matches entry names globally across all kind tables
    (publish.py behavior).
  - Manifest validation errors map to 65 via the existing
    `SkillError`/`CommandError` → `classify_error` path — variant choice
    is implementation detail.
- publish-catalog.yml deliberately serializes pushes — keep sequential loop.
- `subsystem-cli-commands.md` and `product-context.md` still show illustrative
  `grim publish <path> <ref>` single-push — stale since `release` landed;
  this feature is the natural moment to correct both.

---

## Progress Log

| Date | Update |
|------|--------|
| 2026-06-12 | Plan drafted by /architect (discovery + research + ADR complete) |
