# Plan: `.grimignore` — exclude runtime junk from packing and drift hashing

## Status

- **Plan:** plan_grimignore
- **Active phase:** 5 — Review & documentation
- **Step:** /hex-review → done (cycle 1 Request Changes: 2 Blocks, fixed in a11f43a4; cycle 2 delta review of e6c292cf+a11f43a4: Approve, 3 Suggest; next: /hex-finalize)
- **Last update:** 2026-09-27

---

## Overview

**Status:** Draft
**Date:** 2026-09-27
**Issue:** [grimoire-rs/grimoire#128](https://github.com/grimoire-rs/grimoire/issues/128)
**Related ADR:** N/A — decision recorded in *Key Decisions* below (small, additive)

## Objective

A skill (or a multi-file rule's support dir) whose bundled scripts run must
not read as `modified`. Today `content_hash.rs::collect_files` hashes every
regular file under the installed tree, so a `__pycache__/*.pyc` written by
running `scripts/foo.py` flips `grim status` to `modified` and makes
`grim update` refuse (exit 65) without `--force`.

Fix: a gitignore-syntax ignore set — built-in defaults, extended by an
optional `.grimignore` at the walked root — applied by **both** the pack
walk (`grim build`/`release`/`publish`, local path sources) and the drift
hash walk (every integrity reader).

## Scope

### In Scope

- Skills: `.grimignore` at the skill dir root.
- Multi-file rules: `.grimignore` at the support dir root (`rules/<name>/`).
- Built-in default ignore list (below).
- Pack walk (`src/skill/skill_package.rs::collect_files`) and hash walk
  (`src/install/content_hash.rs::collect_files`).
- Legacy-record fallback in `ClientOutput::current_hash`.
- Docs + catalog drift (see Phase 5).

### Out of Scope

- **Agents** — `pack_agent_file` packs only the fixed `AGENT_COMPANIONS`
  allowlist (`README.md`, `logo.png`, `logo.svg`); no scripts, no junk. Not
  given `.grimignore` — record this in docs so nobody extends it by accident.
- Nested `.grimignore` files (only the walk root's counts; a nested one is an
  ordinary file). Add when someone asks.
- Protecting user files from deletion on update: install/update already
  `remove_dir_all` the destination (`installer.rs:889`); unchanged.
- Single-file rules (no tree to walk).

## Research

Three researchers ran 2026-09-27 (in-conversation, not persisted):

- **Prior art** — npm's "presence of `.npmignore` replaces defaults" is the
  known footgun (empty file ships everything); gcloudignore and Cargo
  *extend* defaults. No AI-skill ecosystem (agentskills.io, Claude Code
  plugins, skills.sh, Codex) defines a packaging ignore file; `.aiignore`
  is read-exclusion, unrelated. `.grimignore` is green-field.
- **Parser** — `ignore::gitignore::GitignoreBuilder` (BurntSushi, ripgrep's
  matcher): full gitignore semantics (anchoring, `**`, dir-only `/`, `!`),
  pure in-memory matching, no FS access. Net-new crates: `ignore`,
  `crossbeam-deque`, `crossbeam-epoch`, `crossbeam-utils` (rest already in
  `Cargo.lock` via `globset`); licenses Unlicense/MIT and MIT/Apache-2.0 —
  inside `deny.toml` allow list. Globset-only would mean reimplementing the
  gitignore spec by hand.
- **Codebase** — only two walks matter. Every integrity reader (status,
  TUI, status badge, installer gates, update, uninstall, prune) funnels
  through `ClientOutput::current_hash` → `footprint_hash` → `collect_files`.
  Materializer and `copy_tree` copy the layer verbatim, so a pack-excluded
  file never reaches a client dir; export and MCP render follow for free.
  No related open/closed issues or PRs besides #128.

## Technical Approach

### Architecture Changes

New module `src/install/ignore_set.rs` (or `src/ignore_set.rs` if the
builder in `skill/` should not depend on `install/` — decide at stub time by
checking existing dependency direction) owning the one matcher:

```
IgnoreSet::for_root(root: &Path) -> Result<IgnoreSet, IgnoreSetError>
    defaults (add_line each)  →  then <root>/.grimignore (if a regular file)
IgnoreSet::is_ignored(&self, rel: &Path, is_dir: bool) -> bool
    never ignores the root's index entries (see decisions)
```

Both `collect_files` walks build one `IgnoreSet` at the walk root and check
each child **before** descending into a directory or counting a file.

### Key Decisions

| Decision | Rationale |
|---|---|
| Defaults always apply; `.grimignore` lines are added **after** them, so `!pattern` re-includes a default | Owner decision 2026-09-27. Avoids npm's replace-on-presence footgun; opt-out stays per-path and visible |
| Full gitignore syntax via the `ignore` crate | Real semantics (anchoring, `**`, `!`, dir-only) without a hand-rolled parser |
| Only the walk root's `.grimignore` counts | YAGNI; one file, one place to look |
| `.grimignore` is itself packed and installed, and hashed | Consumer-side hashing needs the publisher's rules; editing it post-install is itself drift, so it cannot silently widen the blind spot |
| `SKILL.md`, the rule index `<name>.md`, and `.grimignore` itself are never ignored | A pattern like `*.md` or `.*` must not strip the artifact's identity or its own rules |
| Case-sensitive matching on every OS | Same bytes → same digest across platforms (hash is cross-platform stable) |
| Ignored directories are pruned, not walked | A large `node_modules/` must not trip `PackLimits` node/byte caps, and `status` gets faster |
| Legacy fallback lives inside `ClientOutput::current_hash` | 11 callers compare `current_hash` with the recorded hash; fixing the one shared seam keeps every caller unchanged |

### Default ignore list (v1)

```
__pycache__/
*.py[co]
.venv/
venv/
.mypy_cache/
.pytest_cache/
.ruff_cache/
*.egg-info/
node_modules/
.git/
.svn/
.hg/
.DS_Store
Thumbs.db
desktop.ini
*.swp
*.swo
*~
.idea/
.vscode/
```

Not `target/` or `.gitignore`: both are plausible skill content names;
publishers add them to `.grimignore` themselves.

### Legacy fallback

Recorded hashes from before this change were computed unfiltered over a
fresh materialization. For any artifact whose layer contains **no**
default-ignored files, filtered == unfiltered, so nothing changes. Only an
artifact that *shipped* e.g. a `.pyc` differs. Therefore:

```
current_hash():
    filtered = footprint_hash(target, support)          // new scheme
    if filtered == self.content_hash → return filtered
    unfiltered = footprint_hash_unfiltered(...)         // only on mismatch
    if unfiltered == self.content_hash → return unfiltered
    return filtered                                      // real drift
```

Install/update records the filtered hash, so a legacy record migrates to the
new scheme on the next write. Cost: a second walk only on mismatch.

### Backwards compatibility (Principle 9)

- **State schema:** unchanged. No new field.
- **Layer format:** unchanged shape. Artifacts built by new grim simply omit
  ignored files; old grim installs them fine.
- **Old grim reading new artifacts:** `.grimignore` is an ordinary file to
  old grim — packed, installed, hashed unfiltered. Works, without the fix.
- **Existing installs:** covered by the legacy fallback above; proven by an
  upgrade test (record an unfiltered hash over a tree that ships a `.pyc`,
  then assert `current_hash` equals it).
- **Growing the default list later:** only affects artifacts whose layer
  ships a newly-defaulted file *and* whose record used the previous
  filtered set — the fallback does not cover that. Ceiling documented in
  Risks; `stability.md` states default-list growth is additive for new
  publishes only.

### JSON interface

No output shape changes. `grim build --json`, `grim status --json`, and the
MCP tools emit exactly the same fields; `status` values simply stop
reporting false `modified`.

### Exit codes

| Case | Exit |
|---|---|
| `grim build` with an unparsable `.grimignore` line | 65 (data error), message names file + line |
| Hash walk meets an unparsable installed `.grimignore` | not fatal: invalid lines skipped with a `warn!`, defaults + valid lines still apply (deterministic; the file is hashed anyway, so tampering still reads as drift) |
| Everything else | unchanged |

## Implementation Steps

> Contract-first TDD: Stub → Specify → Implement → Review.

### Phase 1: Stubs

- [x] **1.1** Add `ignore = "0.4"` to `Cargo.toml` with a why-comment in the
  style of the `globset` entry.
- [x] **1.2** Stub `IgnoreSet` (`for_root`, `is_ignored`, `DEFAULT_PATTERNS`),
  error type with file + line context.
- [x] **1.3** Stub `footprint_hash_unfiltered` (or an `IgnoreMode` param —
  pick the smaller diff) in `content_hash.rs`.

### Phase 2: Architecture review

Skipped unless stub review shows a dependency-direction problem between
`skill/` and `install/` (≤ 4 files).

### Phase 3: Specification tests

- [x] **3.1** Unit — `ignore_set.rs`: defaults match `__pycache__/x.pyc`,
  `node_modules/a/b.js`, `.DS_Store`; `!node_modules/` in `.grimignore`
  re-includes; `SKILL.md` / index / `.grimignore` never ignored even under
  `*`; dir-only pattern does not match a file of the same name; case
  sensitivity; Windows-style separators in rel paths.
- [x] **3.2** Unit — `content_hash.rs`: adding `scripts/__pycache__/a.pyc`
  leaves the digest unchanged; adding `scripts/new.py` changes it; editing
  `.grimignore` changes it; support-dir rule same pair of cases.
- [x] **3.3** Unit — `install_state.rs`: legacy record (unfiltered hash over
  a tree that ships a `.pyc`) → `current_hash` returns the recorded value;
  real drift on a legacy record still differs.
- [x] **3.4** Unit — `skill_package.rs`: pack skips ignored files; ignored
  dir with > `node_limit` entries does not trip `TooLarge`; `.grimignore`
  is packed; invalid `.grimignore` → error mapped to exit 65; rule support
  dir honours its own `.grimignore`.
- [x] **3.5** Acceptance — `test/tests/test_integrity.py`: install skill
  with `scripts/foo.py`, create `scripts/__pycache__/foo.cpython-313.pyc`
  in the installed dir → `grim status` reports installed, `grim update`
  does not refuse. Same for a multi-file rule's support dir. Real edit to
  `scripts/foo.py` still reports `modified` (guard against over-ignoring).
- [x] **3.6** Acceptance — `test/tests/test_build.py`: built layer omits
  `__pycache__/`, `.DS_Store`; `.grimignore` with `!.DS_Store` ships it;
  `.grimignore` with `secret.txt` excludes it.

Gate: tests compile and fail against stubs.

### Phase 4: Implementation

- [x] **4.1** `IgnoreSet` on `GitignoreBuilder`; defaults via `add_line`,
  then `.grimignore`; `case_insensitive(false)`; always query with paths
  under the builder root (the crate panics otherwise).
- [x] **4.2** Pack walk: build the set once per walk root; check before the
  `symlink_metadata` branch; prune ignored dirs before recursing. Ignored
  entries still count one node at the parent read (bounded); their children
  are never read.
- [x] **4.3** Hash walk: same check in `content_hash.rs::collect_files`;
  `footprint_hash` builds the support dir's set from the support dir root.
- [x] **4.4** Legacy fallback in `ClientOutput::current_hash`.

Gate: `task verify` green; Windows check via
`/mnt/c/Users/ecom/grim-wintest/run.sh` on the two new acceptance tests.

### Phase 5: Review & documentation

- [x] **5.1** Review: `opus` code review (L1 Approve, 2 doc Warns + BOM/expect Suggests fixed); security lens on the blind-spot
  trade-off (below).
- [x] **5.2** Docs:
  - `docs/src/content/docs/artifacts.md:81` — "installed verbatim" becomes
    "except ignored files"; new `.grimignore` section with the default list
    and the `!` opt-out; note agents are allowlist-only.
  - `docs/src/content/docs/publishing.md` — packing exclusions near the
    symlink note (~176).
  - `docs/src/content/docs/stability.md` — `.grimignore` semantics are a
    contract; default-list growth applies to new publishes.
  - Explain in the status/drift docs that ignored paths are not drift and
    that update still replaces the whole directory.
- [x] **5.3** Catalog drift (`catalog/README.md` procedure):
  `grim-authoring` (SKILL.md table ~25, `references/skill-spec.md`),
  `grim-usage` build/release/publish rows (~78-80). `task catalog:verify`.

## Files to Modify

| File | Action | Description |
|---|---|---|
| `Cargo.toml`, `Cargo.lock` | Modify | add `ignore` |
| `src/install/ignore_set.rs` (or `src/ignore_set.rs`) | Create | matcher + defaults |
| `src/install/content_hash.rs` | Modify | filtered walk, unfiltered variant |
| `src/install/install_state.rs` | Modify | legacy fallback in `current_hash` |
| `src/skill/skill_package.rs` | Modify | filtered, pruning pack walk |
| `test/tests/test_integrity.py`, `test/tests/test_build.py` | Modify | acceptance tests |
| docs + catalog pages listed in 5.2/5.3 | Modify | documentation |

## Dependencies

| Package | Version | Purpose |
|---|---|---|
| `ignore` | 0.4 | gitignore parser/matcher (only `ignore::gitignore`) |

## Risks

| Risk | Mitigation |
|---|---|
| Ignored paths are drift blind spots. Per `adr_artifact_trust_model.md` the hash is a clobber gate, not tamper detection, so the real cost is: `update` now replaces a dir whose only change is in ignored paths (e.g. a hand-built `.venv/`) without the `--force` refusal. A planted `.pyc` also goes unseen, but needs local write access | Accept; document in the status/drift docs that ignored paths are disposable and wiped on update; `.grimignore` itself is hashed |
| Publisher over-ignores (`*`) and ships an empty skill | Index + `.grimignore` never ignored; build still validates `SKILL.md` |
| Default-list growth flips records of artifacts that shipped a newly-defaulted file | Rare (publisher had to ship junk); on first growth add a recorded ignore-scheme version (additive optional state field) — deferred, noted in `stability.md` |
| `ignore` crate panics on a path outside its root | Walks only query paths under the root they built from; unit test pins it |
| Two walks drift apart | Both call the one `IgnoreSet`; shared unit tests |

## Rollback Plan

Revert the commits. State schema unchanged, so records written by the new
grim (filtered hashes) read as drift only for artifacts that shipped ignored
files — the same set the fallback covers today; `grim update --force`
re-records.

## Progress Log

| Date | Update |
|---|---|
| 2026-09-27 | Plan drafted from three research passes; owner chose defaults + `!` opt-out |
| 2026-09-27 | Unfiltered variant vs mode param → private `Filter` enum inside `content_hash.rs`; only `footprint_hash_for_record` is public (`footprint_hash_unfiltered` is `cfg(test)`) |
| 2026-09-27 | Fallback only in `current_hash`? → no: the installer's tracked-destination check (`installer.rs`, pair-match gate) compares `footprint_hash` with the record directly, so both call the shared `footprint_hash_for_record` |
| 2026-09-27 | Rule index `<name>.md` never ignored → holds structurally: the index sits beside the support dir, outside the walked root; `NEVER_IGNORED` is `SKILL.md` + `.grimignore` |
| 2026-09-27 | `content_hash` now production-dead (all reads go through `footprint_hash`) → `cfg(test)` rather than kept as dead pub API |
| 2026-09-27 | Build acceptance tests: `grim build` emits no layer → assert via `layer_digest` equality/inequality |
| 2026-09-27 | L1 review Approve; fixed `!` wording in grim-authoring catalog, stability.md growth paragraph, BOM strip + loud `expect` in `ignore_set.rs`. Windows: 7 new acceptance tests pass locally |
| 2026-09-27 | Block fix: matcher root was the walk root, and `Gitignore::matched` byte-strips it from relative queries (root `s` turned `scripts/x` into `cripts/x`, so `grim build s` packed ignored files) → matcher built with root `.`, which the crate never strips; only relative paths are queried |
| 2026-09-27 | Block fix: `builder.build().expect` panicked when a hostile `.grimignore` exceeded globset's regex size limit → build error returned (`IgnoreSetError::Build`); `.grimignore` capped at 64 KiB before parsing (`TooLarge`). Strict (pack) → exit 65 naming the file; lenient (hash) → `warn!` + defaults only. Backslash→slash rewrite dropped (Windows `Path` already splits on `\`; on Unix it is a filename byte) |
| 2026-09-27 | Warn (publisher patterns widen what update wipes): accepted per Key Decision "consumer-side hashing needs the publisher's rules"; documented in `artifacts.md` and `commands.md`. Alternatives (defaults-only hashing; refusing layers that ship self-ignored paths) not taken |
| 2026-09-27 | Deferred: pin/fixed-digest test for `ignore` crate matching semantics; `IgnoreSet` rebuilt per `footprint_hash` call; fallback (unfiltered) walk still reads ignored dirs |
| 2026-09-27 | hex-review (medium, spec + security): 2 Blocks fixed in a11f43a4; Windows: all 9 new acceptance tests pass locally |
