# Design Record: Local-Path-Sources Review Fixes

## Status

- **Plan:** plan_review_fixes_local_path_sources
- **Active phase:** 1 — Execute (complete)
- **Step:** awaiting /swarm-review (F1–F8 implemented; 2-round Review-Fix loop + Codex gate converged; task verify green)
- **Last update:** 2026-07-11 (branch feat/local-path-sources)

## Source

Applies the actionable code cluster from the max-tier `/swarm-review` of
`feat/local-path-sources`. Scope: **code fixes only.** Explicitly NOT here
(deferred scope/docs decisions): local-bundle implementation, TUI Local
group, doc/catalog drift, DRY helper extraction, `spawn_blocking` sweep,
`LockEntry` String weakening, `add` triple-pack.

Tier: high. Builder: opus. Codex: on. Contract-first: **failing tests first.**

## Subsystems Touched

install (`installer.rs`, `install_state.rs`, `install_error.rs`), skill
(`skill_package.rs`, `local_pack.rs`), lock (`locked_artifact.rs`,
`locked_source.rs`), command (`install.rs`, `update.rs`, `status.rs`).
Rules: subsystem-file-structure, subsystem-cli, quality-rust,
quality-rust-errors, quality-rust-exit_codes, quality-security, subsystem-tests.

## Fixes & Contracts

### F1 — `InstallIntent` / `dev` threaded through the install pipeline (Cluster B, High)
**Problem:** `install_one`/`install_mcp` hardcode `dev: false`
(`installer.rs:600,979`); `install.rs:273` and `update.rs:316` re-read the
record, flip `dev: true`, and **persist a second time**. Duplicated in two
sites; any future synthetic-lock caller that forgets the re-stamp writes
`dev:false` → next `grim update` `prune_orphans` (`prune.rs:169`, `!r.dev`)
reaps it → deletes the user's rendered files.
**Contract:** thread install intent (smallest correct form: a `dev: bool`
param, or an `InstallIntent { Declared, Dev }` enum) into
`install_and_persist` → `install_one` / `install_mcp` so the record is
written with the correct `dev` value **once**. Remove both re-read/flip/
second-persist blocks. Behavior-preserving for the declared path
(`dev:false`); dev-install records still land `dev:true` and stay
prune-exempt.
**Test:** a dev-install writes exactly one persisted record with `dev:true`
(assert via parsed `state.json`, not substring); a normal install writes
`dev:false`; dev record survives `grim update` prune with a *real*
co-orphan present (declared-then-undeclared artifact reaped, dev spared).

### F2 — canonical-vs-raw anchor for dev records (Cluster B, Warn)
**Problem:** dev flow records the path relative to
`dunce::canonicalize(config_dir)` (`install.rs` else-branch) but `status.rs`
/`update.rs` re-resolve against the **raw** `scope.config_dir()`. Under a
symlinked project dir the round-trip diverges → spurious `Outdated`/pack
failure.
**Contract:** use the same anchor (canonicalized) on both write and read,
OR store raw on both. Pick one; make declared + dev consistent.
**Test:** dev-install under a symlinked project directory reports
`Installed` (not `Outdated`) with no source edit.

### F3 — bounded local packing (Cluster A, Warn/High-impact, CWE-400/770)
**Problem:** `collect_files`/`pack_skill_dir` (`skill_package.rs`) have no
depth/file-count/cumulative-byte bound and read whole files into an
in-memory tar `Vec`. Registry ingestion is gated by
`INSTALL_LAYER_SIZE_LIMIT` (512 MiB); the local path bypasses it. Reachable
from read-only `grim status` (re-packs per invocation) → OOM on a mis-
pointed or hostile path.
**Contract:** add a cumulative-byte cap (reuse/mirror
`INSTALL_LAYER_SIZE_LIMIT`) plus a file-count cap in `collect_files`,
surfacing a new `SkillErrorKind` → `DataError` (65) **before** the
allocation grows unbounded. Applies to skill/rule/agent packing.
**Test:** a path source exceeding the byte or file-count cap fails with 65,
not OOM (unit test on `collect_files`/`pack_skill_dir` with a small
test-only cap or a fixture just over a low bound — do not actually allocate
512 MiB in tests).

### F4 — symlink-skip regression coverage (Cluster A, Warn, CWE-59)
**Problem:** the symlink-skip in `collect_files` (only `is_file()`/`is_dir()`
entries packed) is the sole barrier against exfiltrating a victim's secrets
via a symlink in a cloned repo; it has **zero** tests and is silent, so a
future "fix" could remove it unnoticed.
**Contract:** no code change (defense already correct) — add regression
tests pinning it.
**Test:** (unit) a symlinked file and a symlinked subdirectory under a path
source are absent from the packed tar; (acceptance) a path skill containing
a symlink to an out-of-tree secret installs without that secret appearing
in the client dir.

### F5 — install-time integrity + missing-at-install coverage (Cluster A, High)
**Problem:** `pack_verified_local` hash-mismatch (→65) and source-deleted-
at-install (→65) are both untested.
**Contract:** tests only.
**Test:** (acceptance) lock a path skill, edit the source, run bare `install`
(not `update`) → exit 65 + "content changed" on stderr; lock a path skill,
`rmtree` the source, run `install` → exit 65.

### F6 — `status` must not report `Installed` on a missing/unpackable dev source (Codex, Warn)
**Problem:** `status.rs:375` Err arm logs a warning and returns
`ArtifactStatus::Installed`, hiding source loss. The declared-path source
arm returns a problem-surfacing state for the same `Err`.
**Contract:** the dev source-pack `Err` arm returns a state that flags
attention — consistent with the declared-path source-drift arm (Missing or
Outdated; match the sibling). Keep the warning log.
**Test:** a dev-installed record whose source is deleted reports a non-
`Installed` state (matching the declared arm's behavior for the same case).

### F7 — path `hash` constrained to SHA-256 on the wire (Codex, High/Warn)
**Problem:** `hash: Option<Digest>` (`locked_artifact.rs`,
`install_state.rs`) accepts sha384/sha512, but packing only ever emits
SHA-256; a non-256 path hash deserializes, then fails-closed at install
with a misleading "content changed" message.
**Contract:** reject a non-SHA-256 path `hash` in `RawLockedArtifact::
try_from` and `RawInstallRecord::try_from` (join the existing XOR
validation), with a clear message → `ConfigError`(78)/`DataError`(65) per
the surrounding validation's convention.
**Test:** a lock/state record with `path` + a `sha512:` hash is rejected at
parse with the appropriate exit code; a `sha256:` path hash still parses.

### F8 — structured error source for local-pack failure (security→quality, block-tier rule)
**Problem:** `installer.rs:701`
`InstallErrorKind::LocalSource(format!("{e:#}"))` flattens the `SkillError`
source chain into a `String` (block-tier per quality-rust-errors: "String
wrapping structured error's Display output").
**Contract:** carry the packing `SkillError` structurally via `#[source]`
(new/adjusted `InstallErrorKind` variant). Keep the *content-changed* case
as a structured message variant (it wraps no error — fields: name, locked,
actual). Both → `DataError` (65); exit-code classification unchanged.
**Test:** the source chain is walkable (`source()` returns the inner
`SkillError`) for a pack failure; exit code stays 65.

## Non-negotiable gates

- All new/changed behavior has a failing test first.
- Registry-only lock/state byte-identity + frozen declaration-hash corpus
  stay green (compat).
- `task verify` green before commit. Never push.

## Deferred (out of this execute; surface at commit)

Local-bundle impl + TUI Local group (scope decisions → `/architect`), doc &
catalog drift (→ `/doc-writer`), DRY helper extraction, `spawn_blocking`
uniformity, `LockEntry` String→typed, `add` triple-pack, ignore-file
support, monorepo path-bases.
