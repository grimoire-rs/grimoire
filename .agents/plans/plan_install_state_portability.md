# Plan: Portable install state (shared GRIM_HOME / devcontainers)

## Status

- **Plan:** plan_install_state_portability
- **Active phase:** 6 — Review-fix loop (swarm-execute high) COMPLETE
- **Step:** finalized
- **Last update:** 2026-06-14 (FINALIZED — branch `feat/install-state-portability` rebased to 2 Conventional Commits: `f79a68d` feat(install) + `9082634` chore(artifacts trail); fast-forwardable onto `main` (fd53510); `task verify` green. Pre-rebase: commit ec45575: 28 files +5506/−362. All 2 Block + 5 Warn from swarm-review tier=max fixed: B1 single `persist` seam (4 sites, TUI lossy-guard restored), B2 lexical existence-independent anchor, W1 dead variant removed, W2 dangling-symlink guard, W3 newer-version error, W4 docs, W5 tests (+9 unit → 909). Confirmation review CONFIRMED-CLEAN; Codex final gate PASS (1 Warn TUI error-source fidelity → fixed one-shot). `task verify` green: 909 unit + 257 acceptance + claude:tests. Carry-forward deferred: PERF-01..05, Windows/macOS-CI test execution, TOCTOU/cap-std, global serial/Q3, installer write-before-containment (pre-existing same-uid), dup-key last-writer-wins on load.)

## Classification

| Axis | Value |
|---|---|
| Scope | Medium–Large (≈15 source files + tests + docs — consumer surface wider than first cut; see §6) |
| Reversibility | One-Way Door **Medium–High** (on-disk format + user-data migration) |
| Tier | `high` + overlays |
| Overlays | architect=opus, research=3, codex=on |
| Subsystems | file-structure (`src/install/**`), cli seam (`scope_resolution`, `command/**`), TUI (`src/tui/**`), security (path traversal) |
| CLI surface change | **none** (internal storage format only) |

**Source artifacts:**
- ADR (Accepted, Option 1): [`adr_install_state_portability.md`](../adr/adr_install_state_portability.md)
- Research round 1 (3 axes): [`research_state_portability.md`](../research/research_state_portability.md)
- Research round 2 (devcontainer-id / layout / shared-home / blast-radius): [`research_state_portability_v2.md`](../research/research_state_portability_v2.md)
- Meta-plan: [`meta-plan_install_state_portability.md`](./meta-plan_install_state_portability.md)

**Round-2 design refinements (maintainer decision, post-research):** project state stays at `<workspace>/.grimoire/state.json` (dir confirmed over a loose root file). grim **writes a self-managed `.grimoire/.gitignore` (= `*`)** when it creates the dir (uv/pixi pattern) — never edits the consumer root `.gitignore`. `GRIM_STATE_DIR` override **dropped** (read-only `/workspace` already fails at skill materialization, so redirecting state rescues nothing — YAGNI). Global `global.json` **stays shared** (matches the sharing goal); `devcontainerId`/`GRIM_MACHINE_ID` keying is reserved for the deferred global follow-up, **not** used for project scope (it hashes the host path → identical at a shared `/workspace`, and never auto-injected).

**Defect class fixed:** (1) guaranteed project-state collision under shared `GRIM_HOME` (`sha256(/workspace/grimoire.toml)` identical across containers); (2) non-portable absolute paths; (3) `GRIM_HOME` pollution; (4) the `.copilot`-twice denormalization (top-level `target`/`content_hash` mirror of `clients[0]`).

**Review (converged, 2-round cap):** Round 1 — Claude panel (spec-compliance + opus architect + SOTA-gap): 6 block + 11 warn actionable folded in. Round 2 — spec-compliance re-validation (all 6 round-1 blocks confirmed resolved) + **cross-model Codex plan-gate**: 1 new block (I/O case), 3 warns (`#[non_exhaustive]`, `UnknownAnchor` dual-use → added `AnchorRootAbsent`, prune behavior), and Codex caught a **real migration hole** (§5 `load()` now has a legacy-file fallback on absent new file) + an ADR-claim/OQ3 scope contradiction (reconciled in the ADR). All folded in. No actionable findings remain.

**Pre-execution gate:** branch is `main` — `/swarm-execute` MUST create a feature branch first (principle #6, never commit on main).

**Round-3 fix decisions (swarm-review tier=max → review-fix loop):** independent max-tier review (report `.agents/review_install_state_portability.md`) returned Request Changes — 2 Block + 5 Warn actionable. Decisions:

- **B2 — store-time anchoring is existence-independent + lexical (revert the macOS canonicalize broadening).** `strip_prefix_relative` MUST NOT call `dunce::canonicalize` (canonicalize requires the path to exist → drops legacy records whose target file is gone during V1→V2 migration, silent data loss on macOS). Replace with a **lexical component-wise prefix match**: compare `root`'s components against `abs`'s; `Component::Normal` segments compare **case-insensitively on Windows/macOS** (per-component Unicode `to_lowercase`, never whole-path) and **byte-exact on Linux**; non-Normal/Prefix/RootDir components compare structurally. The stored remainder preserves `abs`'s **original case**. This restores the design's stated "lexical at store time, no canonicalize" invariant (§1.5) AND adds the macOS/Windows case-insensitivity the design under-specified. Read-time `resolve()` keeps its existence-gated canonicalize for containment. Test: a `Workspace` record whose target file does **not** exist still classifies on every platform; macOS-gated case-variant test.

- **B1 — single persist seam.** Extract `InstallState::persist(&self, scope: ConfigScope, workspace: &Path, grim_home: &Path, config_path: &Path) -> Result<(), PersistError>` doing: project-scope `ensure_project_state_dir` → `save` → lossy-gated `reap_legacy_project_state` (skip reap when `legacy_migration_lossy()`); global-scope `save` only. New `#[derive(thiserror::Error)] #[non_exhaustive] pub enum PersistError { EnsureDir{path,#[source]source}, Save{path,#[source]source} }` (lowercase, no period; `#[source]` preserved). All **4** call sites — `command/{install,update,uninstall}.rs` + `tui/app.rs` (×2) — call `persist`; the lossy guard can no longer diverge (root cause: the guard was added to 3 CLI seams, TUI's duplicate copy missed). Test: TUI-path lossy migration keeps the legacy file.

- **W1** remove dead `AnchorError::MigrationFailure` (defined/classified/tested, never constructed — converter is infallible) from `path_anchor.rs` + `error.rs` classify arm + the fabricating test.
- **W2** Layer-2 containment also fires for **dangling** symlinks: `if candidate.exists() || candidate.is_symlink()`.
- **W3** future-version on-disk file (V3 read by V2 binary): on version-probe failure, peek the raw `version` integer; if `> InstallStateVersion::V2 as u8`, return a user-facing "written by a newer version of grim (version N); upgrade grim to read it" instead of opaque `InvalidData`.
- **W4** docs: `CLAUDE.md:91-92` add **agents** to vendor-override rows (`CLAUDE_CONFIG_DIR`/`COPILOT_HOME`/`OPENCODE_CONFIG_DIR`) + `OPENCODE_CONFIG` "skill paths" → "skill/agent paths"; `CHANGELOG.md` reap-trigger list include the TUI path.
- **W5** tests: macOS case-insensitive store-time match, non-UTF8 component → `UnknownAnchor`, `.copilot`-twice denorm absence assertion (N clients → N outputs, no top-level mirror), store-time empty-remainder (`abs == root`) → `UnknownAnchor`.

Deferred (not in this loop): installer write-before-containment (pre-existing, same-uid), dup-key last-writer-wins on load, PERF-01..05, prune `#[non_exhaustive]` reap-default, legacy-sha canonicalize dup, TOCTOU/cap-std, global serial/Q3, `ClientOutput.client: String`, pre-write backup, e2e migration acceptance.

---

# Design: Portable install state (anchor-relativized, relocated) — Option 1

Executable design realizing `adr_install_state_portability.md` Option 1. Grounded in current code; every contract names the real file:line it replaces or calls. New code under `src/install/` (file-structure subsystem) and `src/command/scope_resolution.rs` (CLI seam). No CLI surface changes, no new output format.

## 1. Component Contracts

### 1.1 `PathAnchor` + `AnchorRoots` — new file `src/install/path_anchor.rs`

Closed internal enum, serialized as a kebab-case string tag (human-readable, forward-additive JSON).

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PathAnchor {
    Workspace,      // project scope: <workspace>/...               (project state only)
    ClaudeRoot,     // $CLAUDE_CONFIG_DIR else ~/.claude            (global Claude skills + rules + agents)
    CopilotRoot,    // $COPILOT_HOME else ~/.copilot                (global Copilot skills + agents)
    OpenCodeSkills, // $OPENCODE_CONFIG_DIR/skills else $XDG_CONFIG_HOME|~/.config/opencode/skills
    OpenCodeRoot,   // parent of OpenCodeSkills root                (global OpenCode agents)
    GrimHome,       // $GRIM_HOME (global OpenCode rules dir; global Copilot inert rules path)
}
```

**`AnchorRoots` — all six roots resolved ONCE at scope-resolution time (review sota-F1, spec-F5).** This is the key to `PathAnchor::root` being a pure table-lookup (no ambient env at resolve time → unit-testable without env):

```rust
pub struct AnchorRoots {
    pub workspace: PathBuf,
    pub grim_home: PathBuf,
    pub claude_root: Option<PathBuf>,     // None when neither $CLAUDE_CONFIG_DIR nor $HOME
    pub copilot_root: Option<PathBuf>,
    pub opencode_skills: Option<PathBuf>,
    // No opencode_root field: OpenCodeRoot is derived at lookup time as opencode_skills.parent()
}

impl AnchorRoots {
    /// Resolve every anchor root once, calling the vendor helpers with the
    /// SAME env inputs the materializer uses (single source of truth).
    pub fn resolve(workspace: PathBuf, ctx: &Context) -> Self;   // ctx supplies grim_home + env accessors
}

impl PathAnchor {
    /// Pure lookup into the pre-resolved AnchorRoots — no env reads, no I/O.
    pub fn root(self, roots: &AnchorRoots) -> Option<PathBuf>;
}
```

`AnchorRoots::resolve` delegates (NOT re-implements) to the existing vendor helpers, each of which already takes env values as parameters (so injection is trivial):
- `claude_root` ← `vendor_claude::global_root(env_dir("CLAUDE_CONFIG_DIR"), home_dir())` (vendor_claude.rs:249)
- `copilot_root` ← `vendor_copilot::global_skills_root(...)`'s parent, i.e. the Copilot native root `$COPILOT_HOME` else `~/.copilot` (vendor_copilot.rs:186) — promote the native-root helper to `pub(crate)`
- `opencode_skills` ← `vendor_opencode::global_skills_root(env_dir("OPENCODE_CONFIG_DIR"), xdg_config_dir())` (vendor_opencode.rs:206)
- `workspace`/`grim_home` ← passed in.

**Anchor root vs stored remainder — exact contract per (scope, client, kind)** (review arch-F8, arch-F2, sota-F5). Authoritative source is `ClientTarget::path_for(workspace, scope, kind, name)` (client_target.rs:**103**, the real production fn — NOT the test fns at :357/:401) composed with the per-vendor scope helpers. Note the deliberate asymmetry (each anchor's root already includes any fixed sub-segment, so the remainder is minimal):

| Scope · client · kind | Resolved target | Anchor | Stored `relative` |
|---|---|---|---|
| project · any · any | `<ws>/.claude/...`, `<ws>/.opencode/...`, `<ws>/.github/...` | `Workspace` | `.claude/...` etc. (full sub-path) |
| global · claude · skill | `<claude_root>/skills/<name>` | `ClaudeRoot` | `skills/<name>` |
| global · claude · rule | `<claude_root>/rules/<name>.md` | `ClaudeRoot` | `rules/<name>.md` |
| global · claude · agent | `<claude_root>/agents/<name>.md` | `ClaudeRoot` | `agents/<name>.md` |
| global · copilot · skill | `<copilot_root>/skills/<name>` | `CopilotRoot` | `skills/<name>` |
| global · copilot · agent | `<copilot_root>/agents/<name>.md` | `CopilotRoot` | `agents/<name>.md` |
| global · opencode · skill | `<opencode_skills>/<name>` | `OpenCodeSkills` | `<name>` (root already ends `/skills`) |
| global · opencode · agent | `<opencode_root>/agents/<name>.md` | `OpenCodeRoot` | `agents/<name>.md` |
| global · opencode · rule | `<grim_home>/.opencode/rules/<name>.md` | `GrimHome` | `.opencode/rules/<name>.md` |
| global · copilot · rule | `<grim_home>/.github/instructions/<name>...` (inert) | `GrimHome` | `.github/instructions/...` |

### 1.2 `AnchoredPath` — in `src/install/path_anchor.rs`

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchoredPath {
    pub anchor: PathAnchor,
    /// Forward-slash UTF-8. Invariant: components are EXCLUSIVELY `Normal` —
    /// no CurDir (`.`), ParentDir (`..`), RootDir, or Prefix. NEVER absolute.
    pub relative: String,
}
```

**Validation timing (review spec-F9, sota-F4):** the Normal-only invariant is enforced in **two** places — `from_target` asserts it before storing (well-formed records are always valid), and `resolve()` Layer 1 re-checks it at first use (a corrupt/tampered `relative` that passes JSON parsing is caught here). Deserialization does **not** re-validate — bare `String` + `deny_unknown_fields` is deliberate (see closed OQ: storage type). `CurDir` is **rejected**, not tolerated (simpler than the normalization question; `.` never appears in a well-formed remainder).

**resolve (containment-guaranteed)** — research Axis A two-layer pattern:

```rust
impl AnchoredPath {
    /// Re-join anchor + relative → absolute on-disk path, guaranteed contained
    /// under the anchor root.
    /// Layer 1 (always): reject any component that is not Normal (ParentDir,
    ///   RootDir, Prefix, CurDir) -> TraversalAttempt. Works for absent paths.
    /// Layer 2 (only when candidate exists): dunce::canonicalize both sides,
    ///   assert Path::starts_with (component-granular, never str) -> EscapedAnchor.
    /// AnchorRootAbsent if self.anchor.root(roots) is None (resolve-time).
    pub fn resolve(&self, roots: &AnchorRoots) -> Result<PathBuf, AnchorError>;
}
```

- No FS op (read/hash/delete) ever runs on the raw `relative` join without going through `resolve`.
- Windows: `dunce::canonicalize` both sides (no `\\?\` UNC false-negatives). `dunce = "1"` is the only new dependency (chosen over cap-std=overkill and soft-canonicalize=Layer-1 already handles absent paths).

### 1.3 Reworked `ClientOutput` (was `ClientRecord`, install_state.rs:41-60)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientOutput {
    pub client: String,
    pub target: AnchoredPath,                  // was PathBuf
    pub content_hash: Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support_dir: Option<AnchoredPath>,     // was Option<PathBuf>
}

impl ClientOutput {
    /// Resolve+validate target (and support_dir) then footprint_hash.
    pub fn current_hash(&self, roots: &AnchorRoots) -> Result<Digest, AnchorError>;
    pub fn resolved_target(&self, roots: &AnchorRoots) -> Result<PathBuf, AnchorError>;
    pub fn resolved_support_dir(&self, roots: &AnchorRoots) -> Result<Option<PathBuf>, AnchorError>;
}
```

`current_hash` calls `footprint_hash(resolved_target, resolved_support_dir.as_deref())` (content_hash.rs:64) — same hashing, portable inputs.

### 1.4 Reworked `InstallRecord` (install_state.rs:84-116)

Drop denormalized top-level `target` + `content_hash`; `outputs` is the single source of truth (fixes `.copilot`-twice).

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallRecord {
    pub kind: ArtifactKind,
    pub name: String,
    pub pinned: PinnedIdentifier,
    pub outputs: Vec<ClientOutput>,   // renamed from `clients`; no longer skip_if_empty
}
```

**Byte-stable serialization invariant (review spec-F3):** `save` sorts the record list by **`(name, kind)` ascending** before serialization; within a record, `outputs` is sorted by `client` ascending. This guarantees identical JSON regardless of insertion order. Stated here so T7 spec tests are writable without reading T8.

`client_outputs()` shim (install_state.rs:101-115) is **removed from the in-memory type**; its legacy synth (one `claude` output from denorm fields) moves into the V1→V2 converter (§5). **`load()` always converts in memory (§5)**, so in-memory `outputs` is never empty for any consumer; consumers iterate `record.outputs` directly.

### 1.5 Store-time relativization — `AnchoredPath::from_target`

Classifies an absolute install target into `(anchor, relative)`. Called where `ClientRecord` is built (installer.rs:296-301).

```rust
impl AnchoredPath {
    /// Classify `abs` against the scope/client/kind candidate anchors, returning
    /// the FIRST anchor whose resolved root is a Path-level prefix of `abs`
    /// (longest-root-first), remainder stored forward-slash, CurDir stripped,
    /// asserted Normal-only.
    /// CALLER INVARIANT: `abs` MUST be the non-canonicalized (pre-symlink) form,
    /// built as `root.join(relative)` with NO intervening canonicalize. Passing a
    /// canonicalized abs may yield UnknownAnchor.
    pub fn from_target(abs: &Path, scope: ConfigScope, client: ClientTarget, kind: ArtifactKind, roots: &AnchorRoots)
        -> Result<AnchoredPath, AnchorError>;   // Err(UnknownAnchor) if no root matches
}
```

Candidate sets (closed, from the §1.1 root/remainder table; built from `ClientTarget::path_for` per (scope, client, kind), not a phantom matrix fn):
- **Project scope** → `[Workspace]` only. Non-match ⇒ `UnknownAnchor` (never silently absolute).
- **Global scope** → the single anchor for that (client, kind) row in §1.1; with `GrimHome` also a candidate where vendors fall back to the workspace layout (no `$HOME`/XDG). Longest-root-first so a more specific root wins if `GrimHome` ever prefixes a vendor root (hermetic test layouts).

Relativization mechanics:
- Lexical prefix subtraction (`abs.strip_prefix(root)`); no canonicalize at store time **except** the Windows case-insensitivity guard below.
- Strip any `CurDir` components from the remainder; forward-slash join; assert Normal-only before storing.
- **Windows case-insensitive FS (review sota-F5):** `Path::strip_prefix` is byte-case-sensitive on all platforms, but Windows paths are case-insensitive. On `#[cfg(windows)]`, `dunce::canonicalize` BOTH `abs` and each candidate root before the prefix match, then derive the remainder. This is the only store-time canonicalize, Windows-only.

### 1.6 Project-state location and scope plumbing

- New project state path: `<workspace>/.grimoire/state.json`. Location is the key → `sha256(config_path)` filename (install_state.rs:142-147) deleted for the project case (retained only as `legacy_project_path` migration helper, §5).
- `InstallState::project_state_path(workspace) -> PathBuf` = `workspace.join(".grimoire").join("state.json")`. Replaces `project_path(state_dir, canonical)`.
- **Self-managed gitignore (round-2 Q1):** whenever the project `.grimoire/` dir is created (the `create_dir_all` that precedes the first `save`/migration write), grim also writes `.grimoire/.gitignore` with contents `*\n` **if absent** (idempotent; never overwrites a user-edited one; never touches the consumer's root `.gitignore`). Mirrors uv `.venv/.gitignore` / pixi `.pixi/.gitignore`. Global scope (`$GRIM_HOME/state/`) does **not** get this file. Helper lives beside `project_state_path`; called from the save/migration dir-create seam (T11/T12), not from read-only consumers.
- `InstallState::global_path(state_dir)` (install_state.rs:135-138) **unchanged location** (`$GRIM_HOME/state/global.json`); records now anchored. (Residual risk under shared GRIM_HOME — see §2(c) + Risks.)
- `scope_resolution::resolve` (scope_resolution.rs:75-80) project arm: drop `canonicalize(config)` + `project_path`, set `state_path = project_state_path(&workspace)`. Global arm (:61) unchanged. **Also construct `AnchorRoots::resolve(workspace, ctx)` here** so all consumers receive it from one place.
- Thread `AnchorRoots` into installer/uninstall/status/prune/badge/TUI (they have `scope.workspace` + `ctx.grim_home()` or a TuiContext that carries workspace).

### 1.7 OpenCode `managed_entry` under anchoring

`managed_entry` (opencode_config.rs:65-69) **unchanged**: global glob = `workspace.join(MANAGED_PROJECT_GLOB)` where global `workspace == $GRIM_HOME` (scope_resolution.rs:64). OpenCode rule files anchor to `GrimHome` (§1.1) — same dir the glob points at, coherent by construction. Test `managed_entry_is_relative_for_project_absolute_for_global` (:442-449) passes verbatim. The glob is recomputed each run (not stored), so a differing `$GRIM_HOME` resolves freshly.

## 2. UX / CLI Scenarios (action → outcome → error)

**(a) Host then container (workspace bind-mounted /workspace):**
- Host `grim install` → `<workspace>/.grimoire/state.json` with `Workspace`-anchored project outputs; `~/.claude/...` global outputs `ClaudeRoot`. ⇒ installed.
- Container `grim status` (different `$HOME`) → state rides bind mount; project outputs resolve under `/workspace`; global under container `~/.claude`. ⇒ correct status.
- Error: container lacks `~/.claude` → root resolves to a non-existent path → Layer 2 skipped → file reported `Missing` by `derive_state` (status.rs:194). Exit 0.

**(b) Two projects sharing one GRIM_HOME volume:**
- Each writes its own `<workspace>/.grimoire/state.json` ⇒ **no project collision** (location is the key; old `sha256(/workspace/...)` collision gone).
- **Global state is a different story:** `$GRIM_HOME/state/global.json` remains single and shared. Anchoring makes its PATHS resolve per-machine, but the FILE is shared — concurrent/serial `grim install --global` from two machines is last-writer-wins on the *record set* (same class as the project defect). See Risks + Deferred Q3. Not "benign" — a documented residual risk for `--global` under a shared volume.

**(c) Differing $HOME across machines, shared team GRIM_HOME (global scope):**
- Both machines read the same `global.json`; each resolves `ClaudeRoot`/`CopilotRoot`/`OpenCodeSkills` against ITS OWN `$HOME`/`$CLAUDE_CONFIG_DIR`. The only cross-machine shared data (`content_hash`, `pinned`) is machine-independent → path portability is correct (Cargo `~/.cargo/registry` model).
- Error: corrupt `relative` with `../` → `TraversalAttempt` (exit 65) on a mutating op; read-only `status`/`search`/TUI degrade that record to `Missing`/not-installed (never `?`-propagate — §3, §6).

## 3. Error Taxonomy — `AnchorError` (new, `src/install/path_anchor.rs`)

`thiserror`, lowercase no-period messages (`quality-rust-errors.md`), `#[source]`/`transparent` on I/O.

| Variant | message | Cause | Exit class | Remediation |
|---|---|---|---|---|
| `TraversalAttempt { relative }` | `path traversal rejected in stored relative path '{relative}'` | Layer-1 non-Normal component | DataError 65 | "install-state file corrupt; re-run `grim install`" |
| `EscapedAnchor { anchor, resolved }` | `resolved path '{resolved}' escapes its anchor root` | Layer-2 symlink escape | DataError 65 | same; investigate symlink tampering |
| `UnknownAnchor { path }` | `cannot classify install target '{path}' under any known anchor` | store-time (`from_target`) no root matches | Failure 1 | file issue; target outside vendor layout |
| `AnchorRootAbsent { anchor }` | `anchor root '{anchor}' is unresolvable (no env / home)` | resolve-time `anchor.root()` is None | Failure 1 (mutating); read-only → Missing | set `$HOME` / `$CLAUDE_CONFIG_DIR` / `$COPILOT_HOME` |
| `MigrationFailure { reason }` | `install-state migration failed: {reason}` | converter atomic step failed | IoError 74 | retry; if persistent, delete `.grimoire/state.json` + re-install |
| `Io { path, #[source] source }` | `I/O error at '{path}'` | read/canonicalize failure | IoError 74 | check permissions on `{path}` |

`AnchorError` derives `thiserror::Error` and carries `#[non_exhaustive]` (error-enum convention — `arch-principles.md` exempts error enums from the no-`#[non_exhaustive]` rule). `UnknownAnchor` is **store-time only** (`from_target`); resolve-time root-None is the distinct `AnchorRootAbsent` (review round-2 NEW-F3, avoids dual-use).

**Exit-code wiring (review spec-F8) — has dedicated task scope:**
- The top-level `Error` enum (or its install-tier wrapper) gains `Anchor(#[from] AnchorError)` so `?` propagation works from install/uninstall/update paths (T1).
- `classify_error` (`quality-rust-exit_codes.md`) extended to downcast `AnchorError`: `TraversalAttempt|EscapedAnchor → DataError(65)`, `MigrationFailure|Io → IoError(74)`, `UnknownAnchor|AnchorRootAbsent → Failure(1)` (T3 unit test + T11 wiring).
- Read-only `status`/`search`/TUI badge **never `?`-propagate** `AnchorError` — they `match` and map to `Missing`/not-installed (§6, T10).

## 4. Edge Cases

1. **Multi-file rule support_dir** — anchored same root as target; resolve both before `footprint_hash`; uninstall reaps index then support via resolved paths. Deleted support dir hashes index-only (drift, not error).
2. **Symlinked workspace** — resolve-time Layer-2 `dunce::canonicalize` both sides; containment holds. Store-time: `from_target` receives the non-canonicalized `abs` (caller invariant §1.5), so `strip_prefix` against the (also non-canonicalized) `roots.workspace` succeeds.
3. **Missing/partial state** — absent file → empty state (NotFound arm preserved); per-record resolution; missing files → `Missing`.
4. **Record matching no anchor (migration)** — `UnknownAnchor` ⇒ skipped with `tracing::warn!`, never silently dropped, never fabricated.
5. **Concurrent install** — both runs take the existing `ConfigFileLock` on the scope config; migration is serialized (winner converts, loser loads V2 + skips). **Caveat (review arch-F5):** the lock exists only when `lockable_config_path` is `Some` (config file present). Project scope: config sits at `<workspace>/grimoire.toml`, co-located with `.grimoire/state.json`, so the lock correctly serializes same-workspace installs. Config-less global scope: no advisory lock → rely on `atomic_write` for partial-file safety; accept last-writer-wins (ties to §2(b)/Risks).
6. **Empty `clients[]` legacy record** — converter synthesizes one `claude` `ClientOutput`, `content_hash` unchanged (drift baseline survives).
7. **Read-only `global.json` never migrated** — `load()` converts in memory every read (§5) so consumers see populated `outputs`; only the persist step (gated to mutating commands) is skipped.

## 5. Migration State Machine (V1 → V2)

`InstallStateVersion::V2 = 2` added to the `serde_repr` enum. Two-struct (V1 wire / V2 in-memory) explicit converter + version-peek probe; never `#[serde(untagged)]`.

**`load()` ALWAYS converts in memory (review arch-F6)** — read-only consumers (status, search, badge, TUI, prune) must never observe empty `outputs`. The persist/relocate/reap side is the only part gated to mutating commands.

```
load(scope, roots): read bytes from scope.state_path
  FOUND at new location:
    peek VersionProbe { version }
     ├── V2 -> deserialize InstallStateFileV2 -> in-memory (DONE)
     └── V1 -> CONVERT IN MEMORY (below)
  NOT FOUND at new location  (Codex round-2 fix: first post-upgrade read MUST still see migrated state):
     ├── project scope: legacy := legacy_project_path(grim_home/state, canonical_config_path)
     │      legacy exists -> read + CONVERT IN MEMORY (no disk write) ; else -> empty V2 state
     └── global scope: empty V2 state
  CONVERT IN MEMORY (V1 wire -> outputs; NO disk write):
        for each InstallRecordV1:
          outputs := clients.is_empty() ? [synth claude from denorm target/content_hash] : clients
          for each output:
            from_target(abs, scope, client, kind, roots)
              ok  -> ClientOutput { anchor+rel, content_hash UNCHANGED }
              Err -> warn "skipping unanchorable {path}"; drop output
          zero anchorable outputs -> warn; drop record
        -> V2 in-memory state
```

**Persistence (atomic, locked; only on mutating command — install/update/uninstall — never on read-only status):**
```
1. acquire ConfigFileLock on the scope config when lockable_config_path is Some
   (project: <workspace>/grimoire.toml, co-located with .grimoire/state.json)
2. detect legacy: old $GRIM_HOME/state/projects/<sha>.json exists (recompute legacy sha), OR loaded file was V1
3. convert (above), content_hash unchanged (preserves drift baseline)
4. atomic_write V2 to <workspace>/.grimoire/state.json (records sorted (name,kind); outputs sorted by client)
5. best-effort remove old projects/<sha>.json (idempotent; V2 discriminant prevents re-convert)
6. release lock (Drop)
```

**Old-file discovery:** converter recomputes `sha256(canonicalize(config_path))` (legacy formula) — the ONLY surviving use of the old hash, in marked helper `legacy_project_path(state_dir, canonical_config_path)`. **`load()` itself (read path, project scope) uses this helper to fall back to the legacy file when the new state file is absent**, converting in memory without writing — so first post-upgrade `status`/`search`/TUI see migrated state; the next mutating command persists + reaps. `load()` therefore receives the scope (config_path + workspace + state_path) and `grim_home`.

**Fallback (lazy rebuild):** only when conversion yields zero anchorable records from a non-empty legacy file; re-hash current bytes + one-shot stderr warning `"drift baseline reset: prior content hashes replaced with current on-disk hashes"`. Never the default.

**Forward-compat / rollback trap:** keep `deny_unknown_fields` on V2 structs. An old binary reading a V2 file fails cleanly — on the new `V2` discriminant at the `serde_repr` layer, and on the renamed `outputs`/new `anchor` fields via `deny_unknown_fields` — InvalidData, never silent truncation, no rollback by design (install.rs gates on lock freshness).

## 6. Consumer Edits (call-site contracts)

**Field-level surface is wider than the first cut (review arch-F1):** every reader of `record.client_outputs()` / `out.target` / `out.current_hash()` / the removed denorm fields must thread `AnchorRoots` and switch to `record.outputs` + resolved accessors. Read-only consumers MUST map `AnchorError` to missing via `match`, never `?` (review arch-F3).

| Consumer (file:line) | Today | After |
|---|---|---|
| `installer.rs:296-301` (build output) | `target: dest` (abs) | `target: AnchoredPath::from_target(&dest, scope, *client, kind, roots)?` (dest is NOT canonicalized — §1.5 invariant) |
| `installer.rs:127-148` (integrity gate) | `out.target.exists()`, `out.current_hash()` | `out.resolved_target(roots)?.exists()`, `out.current_hash(roots)?` |
| `installer.rs:311-318` (record) | denorm + `clients` | `outputs: client_records` only |
| `uninstall.rs:67-73` | `remove_output(&out.target)` | `remove_output(&out.resolved_target(roots)?)` (+ support) |
| `status.rs:192-211` | `out.target.exists()`, `current_hash()` | resolved variants; **`match` Err(AnchorError) => Missing**, never `?` |
| `prune.rs:119-189` (`prune_orphans`, `is_modified`) | `record.target`, `r.client_outputs()`, `out.target.exists()`, `out.current_hash()`, `out.content_hash` | thread `roots`; `r.outputs`; resolved accessors; invoked by `update.rs:131`. **Security-class distinction (review round-3 ARCH-4): prune reaps a genuinely-unresolvable orphan (`AnchorRootAbsent`, plain io) + `tracing::warn!`, consistent with status Missing — but PROPAGATES a security-class `AnchorError` (`TraversalAttempt`/`EscapedAnchor`) as `PruneError::Anchor` → `DataError(65)`, never reaping it** (supersedes round-2 NEW-F4) |
| `status_badge.rs:68-78` (`derive_badge`) | `outputs`, `out.target`, `out.current_hash()` | add `roots` param; **absorb AnchorError → NotInstalled/IntegrityMissing**, never `?`. Shared by search + TUI |
| `command/search.rs:120,156-162` | threads `(lock, state)` into `derive_badge` | build `AnchorRoots` from the scope it resolves (`load_scope_best_effort`); pass to `derive_badge` |
| `command/tui.rs:104-125` | builds TuiContext | construct `AnchorRoots` from TuiContext (carries workspace + state_path) |
| `src/tui/app.rs:670-680,840-858,956-977` (`derive_artifact_state`, `perform_uninstall`, install path) | `outputs`, `out.target`, `out.current_hash()`, uninstall seam | thread `roots`; resolved accessors; **derive_artifact_state absorbs AnchorError → not-installed**, never `?` |
| `opencode_config.rs:128-150` (`sync_for_state`) | `r.client_outputs()` | `r.outputs` (rename) |
| `scope_resolution.rs:75-80` | `canonicalize` + `project_path(sha)` | `project_state_path(&workspace)` + build `AnchorRoots::resolve(workspace, ctx)` |
| `command/install.rs`, `command/uninstall.rs`, `command/update.rs` | pass `&mut state` | also thread `AnchorRoots` from `scope.workspace` + `ctx.grim_home()` |
| top-level `error.rs` (or install-tier wrapper) | — | add `Anchor(#[from] AnchorError)`; extend `classify_error` |

`outputs` does NOT keep `#[serde(rename = "clients")]` — V2 is a new schema; old keys live only in the V1 wire struct. `command/lock.rs` needs **no** change (does not call install ops — confirm in review).

---

## Executable Phases (for /swarm-execute)

Contract-first TDD (Stub → Specify → Implement → Review). Dependency-ordered; parallelizable groups noted. Acceptance criteria are testable without reading implementation.

### Dependency graph

```
T1 (stub anchors + AnchorRoots + Error::Anchor) ─┬─ T2 (spec resolve) ── T3 (impl resolve + classify_error) ─┐
                                                 ├─ T4 (spec from_target) ── T5 (impl from_target) ──────────┤ (T5 deps: T4 AND T3)
                                                 └─ T6 (stub state V2) ── T7 (spec state/migrate) ── T8 (impl state+converter)
T6 ── T9 (stub consumers incl. prune/badge/search/tui/update) ── T10 (spec consumers+badge+tui+prune) ── T11 (impl consumers + classify_error wiring) ── T12 (impl migration persist)
T11 ── T13 (acceptance: collision/portability/drift/traversal)
T11,T12 ── T14 (docs + gitignore + global-residual-risk + catalog drift)
```
Parallelizable: {T2,T4} after T1; T6 alongside T2–T5; T13 alongside T14 after T11/T12.

### Tasks

**T1 — Stub: dunce dep + PathAnchor/AnchorRoots/AnchoredPath/AnchorError + Error::Anchor** · `stub` · deps: — · files: `Cargo.toml`, `src/install/path_anchor.rs`, `src/install.rs`, `src/error.rs` (or install-tier error)
- `dunce = "1"` in Cargo.toml (`// chosen over cap-std=overkill, soft-canonicalize=Layer-1 handles absent paths`); `cargo check` resolves it.
- `PathAnchor { Workspace, ClaudeRoot, CopilotRoot, OpenCodeSkills, OpenCodeRoot, GrimHome }` Serialize/Deserialize kebab-case.
- `pub struct AnchorRoots { pub workspace: PathBuf, pub grim_home: PathBuf, pub claude_root: Option<PathBuf>, pub copilot_root: Option<PathBuf>, pub opencode_skills: Option<PathBuf> }` (5 fields; no `opencode_root` — `OpenCodeRoot` is derived at lookup time as `opencode_skills.parent()`, not a stored field) + `pub fn resolve(workspace: PathBuf, ctx: &Context) -> Self` (struct-literal construction allowed; fields pub).
- `AnchoredPath { anchor, relative }` with `deny_unknown_fields`.
- `AnchorError` (thiserror, `#[non_exhaustive]`) variants TraversalAttempt, EscapedAnchor, UnknownAnchor (store-time), AnchorRootAbsent { anchor: PathAnchor } (resolve-time root None), MigrationFailure, `Io { path: PathBuf, #[source] source: io::Error }` (message `I/O error at '{path}'` — canonical acronym case per quality-rust-errors.md).
- Top-level `Error` (or install-tier wrapper) gains `Anchor(#[from] AnchorError)`.
- Signatures `PathAnchor::root`, `AnchoredPath::resolve/from_target`, `AnchorRoots::resolve`, `ClientOutput::current_hash/resolved_target/resolved_support_dir` with `unimplemented!()`.
- Module wired into `src/install.rs`; `cargo check` passes whole crate.

**T2 — Specify: resolve containment** · `specify` · deps: T1 · files: `src/install/path_anchor.rs`
- `Workspace.root(&roots)` == workspace; `GrimHome.root` == grim_home; `ClaudeRoot.root` == the value set in `AnchorRoots.claude_root` (pure lookup — test sets all five fields, NO env dependency).
- `resolve` of `skills/foo` → `root.join("skills/foo")`.
- `resolve` of relative with `..` → `Err(TraversalAttempt)` WITHOUT filesystem touch (path need not exist).
- `resolve` of leading-`/` relative → rejected (RootDir component).
- `resolve` of CurDir-containing relative `./skills/foo` → `Err(TraversalAttempt)` (CurDir rejected — §1.2 decision).
- `resolve` where the candidate path does NOT exist → Layer 2 NOT exercised → `Ok(candidate)` (proves Layer 1 standalone).
- `resolve` where `anchor.root` is None → `Err(AnchorRootAbsent)`.
- forward-slash relative resolves identically across platforms.
- Tests compile + fail against stubs.

**T3 — Implement: resolve (two-layer guard) + classify_error** · `implement` · deps: T2 · files: `path_anchor.rs`, `vendor_claude.rs`, `vendor_opencode.rs`, `vendor_copilot.rs`, `error.rs`
- `AnchorRoots::resolve` calls vendor helpers once; `PathAnchor::root` is pure lookup (no env).
- vendor `global_root`/`global_skills_root`/Copilot native-root promoted `pub(crate)` (no env re-impl).
- Layer 1 rejects every non-Normal component before any FS access.
- Layer 2 only when candidate exists; `dunce::canonicalize` both sides + `Path::starts_with`.
- **symlink-escape acceptance (moved from T2, review spec-F1):** tempdir anchor; place a symlink inside pointing outside; `resolve` returns `Err(EscapedAnchor)` (fixture: `std::os::unix::fs::symlink`, `#[cfg(unix)]`).
- `classify_error` extended + unit tests: `TraversalAttempt|EscapedAnchor → DataError(65)`, `MigrationFailure|Io → IoError(74)`, `UnknownAnchor → Failure(1)`.
- T2 passes; `task rust:verify` passes (no unwrap in non-test).

**T4 — Specify: from_target classification** · `specify` · deps: T1 · files: `src/install/path_anchor.rs`
- project Claude `<ws>/.claude/rules/x.md` → `(Workspace, ".claude/rules/x.md")`.
- exact remainder per global (client, kind) pair (§1.1 table): global Claude skill → `(ClaudeRoot, "skills/<name>")`; global OpenCode skill → `(OpenCodeSkills, "<name>")`; global OpenCode rule → `(GrimHome, ".opencode/rules/<name>.md")`; global Copilot skill → `(CopilotRoot, "skills/<name>")`.
- project target NOT under workspace → `Err(UnknownAnchor)`.
- stored `relative` is forward-slash, Normal-only — explicitly assert NO leading slash, NO `..`, NO `.` segments.
- strip_prefix remainder containing a `CurDir` component → stored relative has no `.` segments.
- non-canonicalized `abs` (built via `root.join`) → strip_prefix succeeds; an accidentally-canonicalized `abs` through a symlinked workspace → documents `UnknownAnchor` hazard.
- `#[cfg(windows)]`: case-mismatched root vs abs → still classifies (case-insensitive prefix per §1.5).
- longest-root-first when `GrimHome` is a prefix/suffix of a vendor root in a hermetic layout.
- Tests compile + fail.

**T5 — Implement: from_target (prefix subtraction)** · `implement` · deps: T4, T3 · files: `path_anchor.rs`
- tries scope/client/kind candidate anchors longest-root-first; first Path-level prefix match.
- lexical prefix subtraction; CurDir stripped; Normal-only asserted.
- `#[cfg(windows)]` case-insensitive prefix via `dunce::canonicalize` both sides (only store-time canonicalize).
- non-match → `Err(UnknownAnchor)`.
- All T4 tests pass; `task rust:verify` passes.

**T6 — Stub: reworked ClientOutput + InstallRecord** · `stub` · deps: T1 · files: `install_state.rs`
- `ClientRecord` → `ClientOutput` with `target: AnchoredPath`, `support_dir: Option<AnchoredPath>`.
- `InstallRecord.outputs: Vec<ClientOutput>`; NO top-level `target`/`content_hash`.
- `current_hash/resolved_target/resolved_support_dir(&self, roots)` signatures (unimplemented).
- `InstallStateVersion` gains `V2 = 2`.
- `client_outputs()` shim removed from in-memory type.
- install_state module compiles in isolation (consumers may be red pre-T9).

**T7 — Specify: V2 round-trip + V1→V2 migration** · `specify` · deps: T6, T3, T5 · files: `install_state.rs`
- V2 state with anchored outputs round-trips through disk identically.
- **byte-stable save:** records inserted in reversed order serialize identically to forward order (sort key `(name, kind)`; outputs by `client`).
- legacy V1 (denorm target/content_hash, empty clients) → load+migrate synthesizes one `claude` ClientOutput, `content_hash` UNCHANGED.
- V1 with populated `clients` (abs paths) → each becomes anchored ClientOutput, content_hash preserved.
- **read-only V1 load yields populated outputs (review arch-F6):** loading a V1 file does NOT write to disk and returns records with non-empty `outputs` (claude synthesized), content_hash unchanged.
- **legacy fallback on absent new file (review round-2 Codex):** new `<workspace>/.grimoire/state.json` ABSENT + legacy `projects/<sha>.json` PRESENT → `load()` discovers the legacy file via `legacy_project_path`, returns migrated `outputs` IN MEMORY, writes nothing; a second `load()` returns the same (idempotent, still no write).
- unknown version (99) rejects with InvalidData (existing invariant, :291).
- V1 record whose abs path matches no anchor → record dropped (no panic, no fabricated hash).
- **rollback trap (review sota-F9):** a V2 JSON deserialized into the V1 wire struct fails with InvalidData (not panic, not silent partial read).
- Tests compile + fail.

**T8 — Implement: ClientOutput methods + load/save converter** · `implement` · deps: T7 · files: `install_state.rs`, `path_anchor.rs`
- `current_hash` resolves target+support via AnchorRoots then `footprint_hash`.
- `load` peeks VersionProbe: V2 direct; V1 wire struct → convert IN MEMORY via `from_target`, content_hash unchanged, NO disk write.
- `load` for project scope, when the new state file is absent, discovers + reads the legacy `projects/<sha>.json` via `legacy_project_path` and converts in memory (no write) — closes the first-post-upgrade-read gap.
- `save` always writes V2; records sorted `(name, kind)`, outputs sorted `client`.
- converter `tracing::warn` per unanchorable output/record skipped; never fabricates a hash.
- T7 passes; `task rust:verify` passes.

**T9 — Stub: scope_resolution + all consumer signatures (AnchorRoots threading)** · `stub` · deps: T6 · files: `scope_resolution.rs`, `installer.rs`, `uninstall.rs`, `status.rs`, `prune.rs`, `status_badge.rs`, `opencode_config.rs`, `command/install.rs`, `command/uninstall.rs`, `command/update.rs`, `command/search.rs`, `command/tui.rs`, `src/tui/app.rs`
- `project_state_path(workspace)` → `<workspace>/.grimoire/state.json`; project arm uses it + builds `AnchorRoots::resolve`.
- stub `ensure_project_state_dir(workspace) -> io::Result<()>` (create_dir_all `.grimoire/` + write `.gitignore` = `*` if absent) — `unimplemented!()`; called from mutating seams only (T11/T12), never read-only consumers.
- global arm still `global_path(state_dir)`.
- ALL consumers (incl. prune/status_badge/search/tui/update) accept or construct `AnchorRoots`, reference `record.outputs` not `client_outputs()`.
- whole-crate `cargo check` passes (partial bodies, must compile).

**T10 — Specify: consumer behavior portability (incl. badge/TUI/prune)** · `specify` · deps: T9 · files: `installer.rs`, `uninstall.rs`, `status.rs`, `prune.rs`, `status_badge.rs`, `opencode_config.rs`, `src/tui/app.rs`
- installer: fresh install records `Workspace`-anchored targets (no absolute PathBuf in saved JSON).
- installer: integrity gate refuses tampered file via AnchoredPath (Refused), `--force` updates (port of `modified_file_refused_then_forced`).
- uninstall: removes skill dir + rule file via resolved paths + drops record.
- status: derive_state Installed/Modified/Missing via resolved targets; **AnchorError degrades to Missing via match, not a command failure** (unit: unresolvable AnchoredPath → Missing, no error).
- `derive_badge` (search/TUI): unresolvable AnchoredPath → NotInstalled/IntegrityMissing badge, never errors.
- `derive_artifact_state` (TUI): unresolvable AnchoredPath → not-installed, never `?`-propagates.
- prune: `is_modified`/`prune_orphans` resolve via roots; an unresolvable record (AnchorError) → treated as absent/orphaned (safe to reap) with a `tracing::warn!`, consistent with status Missing — never silently retained.
- opencode_config: `sync_for_state` reads `r.outputs`, still adds/removes managed glob; managed_entry assertions unchanged.
- Tests compile + fail.

**T11 — Implement: consumer call sites + classify_error wiring** · `implement` · deps: T10, T8 · files: `installer.rs`, `uninstall.rs`, `status.rs`, `prune.rs`, `status_badge.rs`, `opencode_config.rs`, `command/install.rs`, `command/uninstall.rs`, `command/update.rs`, `command/search.rs`, `command/tui.rs`, `src/tui/app.rs`, `error.rs`
- installer builds ClientOutput via `from_target` (non-canonicalized `dest`) and records `outputs` only.
- uninstall/status/prune/badge/TUI integrity all via `resolved_target`/`current_hash`; no consumer touches `.relative` or joins manually.
- read-only sites (status/derive_badge/derive_artifact_state) `match` AnchorError → missing; mutating sites propagate via `?` → classify_error → exit 65/74.
- command layer threads `AnchorRoots` from `scope.workspace` + `ctx.grim_home()`; search from its resolved scope; TUI from TuiContext.
- **mutating project-scope seams call `ensure_project_state_dir(workspace)` before `save`** (install/update/uninstall + TUI install/uninstall): create_dir_all `.grimoire/` then write `.grimoire/.gitignore` = `*\n` if absent (idempotent; never overwrites; global scope skips it). Unit test: helper creates the file once, leaves a user-edited one untouched.
- All T10 tests pass; pre-existing installer/uninstall/status/prune/badge/TUI unit tests pass (ported where signatures changed); `task rust:verify` passes.

**T12 — Implement: migration persistence (relocate + reap legacy)** · `implement` · deps: T11 · files: `install_state.rs`, `command/install.rs`, `command/uninstall.rs`, `command/update.rs`
- on a mutating command, legacy `$GRIM_HOME/state/projects/<sha>.json` is read, converted, atomic-written to `<workspace>/.grimoire/state.json`, then best-effort removed.
- legacy sha via marked `legacy_project_path` helper (only surviving use of old formula).
- migration under existing `ConfigFileLock` when `lockable_config_path` is Some; **None-lock path** (config-less global) documented to rely on atomic_write + accept last-writer-wins.
- test: seeded legacy `projects/<sha>.json` with abs paths → after `install`, new state.json exists, resolves clean, old file gone.
- **loser-skips test (review spec-F7):** pre-seeded V2 state.json → a second migration-path call reads V2, skips conversion, leaves the file byte-identical (zero writes). No real concurrency needed.
- `task rust:verify` passes.

**T13 — Specify: acceptance tests (collision/portability/drift/traversal)** · `specify` · deps: T11 · files: `test/tests/test_state_portability.py`, `test/tests/test_status.py`, `test/tests/test_integrity.py`
- two projects, same `/workspace`-style config path, one shared GRIM_HOME → NO collision (each own `.grimoire/state.json`).
- install, move project dir, `grim status` still resolves + reports installed (portability).
- install then edit support-dir file → `grim status` reports modified (drift preserved through anchoring).
- hand-corrupted `relative` with `../` → touching mutating command exits 65 (DataError); `grim status` degrades that row to missing, exits 0.
- fresh clone (no `.grimoire/state.json`) → `grim install` creates it (rebuild-on-missing), exit 0.
- **self-managed gitignore:** after first project `grim install`, `<workspace>/.grimoire/.gitignore` exists with contents `*`; the consumer's root `.gitignore` is unchanged; a second install does not overwrite a hand-edited `.grimoire/.gitignore`.
- fail before T11/T12, pass after.

**T14 — Docs: file-structure rule, self-managed gitignore, global residual risk, catalog drift** · `docs` · deps: T11, T12 · files: `.claude/rules/subsystem-file-structure.md`, `CLAUDE.md`, `catalog/README.md`, `docs/src`
- document that grim **self-manages `.grimoire/.gitignore`** (= `*`) on first project install — the consumer never edits their root `.gitignore`; note the dot-dir matches `.git/`/`.terraform/`/`.pixi/` convention. Warn against a devcontainer **named volume** at `/workspace/.grimoire` (shadows the bind-mounted state — Axis C gotcha).
- subsystem-file-structure.md documents new project state location, unchanged global, the PathAnchor set + §1.1 root/remainder table, resolve+validate containment rule (pointer to quality-security.md).
- **document the global.json residual** (review arch-F4 / round-2 Q3): under shared GRIM_HOME, `global.json` **stays shared** (the desired behavior — global skills + their state shared across containers); concurrent multi-machine `--global` installs are last-writer-wins on the record set. State the stance as **single-writer-at-a-time for v1**; per-host segmentation (`devcontainerId`/`GRIM_MACHINE_ID` key) is a tracked follow-up, **not** in this change.
- **No `GRIM_STATE_DIR`** (round-2 Q4 closed): read-only `/workspace` already fails at skill materialization, so a state-dir override rescues nothing — do not add an env var; no env-table change for this.
- first-party catalog skills (grim-usage / ai-config-authoring) drift-reviewed per catalog/README.md.
- `task claude:tests` passes (catalog parity).

---

## Risks

| Risk | Mitigation |
|---|---|
| **Path-traversal / symlink escape** — stored `relative` is untrusted at read; `../`/symlink could escape anchor and read/delete an arbitrary file. | Mandatory two-layer guard in `resolve` (Layer 1 lexical Component filter, CurDir rejected; Layer 2 `dunce::canonicalize`+`Path::starts_with` when path exists). No consumer joins anchor+relative manually. Traversal unit tests (T2) + symlink-escape (T3) + corrupted-state acceptance (T13). Security review of resolve boundary before merge. |
| **Migration silently resets drift baseline** — naive rebuild re-hashes user-edited bytes as clean. | Explicit two-struct converter carries `content_hash` UNCHANGED (T7/T8). Lazy rebuild only when zero records anchorable, with one-shot stderr warning. Migration test asserts content_hash preservation. |
| **Read-only consumers fail on corrupt state** — status/search/TUI `?`-propagating AnchorError would break read-only exit-0 contract. | status/derive_badge/derive_artifact_state `match` AnchorError → missing, never `?` (§3, §6, T10). Only mutating commands propagate → exit 65/74. |
| **Consumer surface undercount** — prune/status_badge/search/TUI also read reworked fields; missing them = late compile break or stubbed-check masking. | §6 + T9/T11 now list all of them; whole-crate `cargo check` gate at T9. |
| **Windows UNC + case-insensitivity** — `canonicalize` UNC false-negatives; `strip_prefix` case-sensitive. | `dunce::canonicalize` both sides at resolve (Layer 2) and at store-time on `#[cfg(windows)]` for the prefix match. `dunce = "1"` single new dep. |
| **`global.json` last-writer-wins under shared GRIM_HOME** — anchoring fixes PATH portability but the FILE is shared; concurrent multi-machine `--global` installs clobber record sets (same class as the project defect). | **Residual risk, documented (T14) + Deferred Q3.** Project state fully de-collided; global state is a conscious product-semantics decision (single-machine-per-volume, or follow-up to segment per machine identity). atomic_write prevents partial files. |
| **Anchor-set drift** — vendor root model changes ⇒ stale anchors. | `AnchorRoots::resolve`/`PathAnchor::root` delegate to existing vendor helpers (single source of truth); serde_repr V2 envelope gates schema; UnknownAnchor fails loud (warn+skip). Candidate sets from §1.1 table / tested `path_for`. |
| **Concurrent install / migration** races on state.json. | Migration/writes under existing `ConfigFileLock` on the new location's owning config (project: co-located grimoire.toml). Config-less global → no lock, atomic_write + last-writer-wins (ties to global residual risk). |

## Deferred / Open Questions (require human judgment)

1. *(Closed — round-2 research v2)* **Auto-edit `.gitignore`?** Resolved: grim writes a **self-managed `.grimoire/.gitignore` (= `*`)** inside the dir it owns (uv/pixi pattern), never touching the consumer's root `.gitignore`. Idempotent; never overwrites a hand-edited one. Implemented at T11/T12 via `ensure_project_state_dir`; tested T13.
2. *(Closed — review)* State stays gitignored / never committed. Research Axis B confirms (machine-local; committing churns across teammates). Decision recorded, not open.
3. *(Resolved — round-2 Q3)* **Global state under shared GRIM_HOME.** Path portability is **solved** by anchoring (per-machine resolution; content_hash/pinned machine-independent). `global.json` **stays a single shared file** — this is the *desired* behavior (share global skills + their state across containers, the maintainer's stated goal). Residual: concurrent multi-machine `--global` installs are last-writer-wins on the record set. **Stance: single-writer-at-a-time for v1, documented (T14).** Per-host segmentation (`devcontainerId`/`GRIM_MACHINE_ID`-keyed `state/global/<id>.json`) is the right tool for a **tracked follow-up**, not this change. (Research v2: `devcontainerId` hashes the host path → identical at shared `/workspace`, and is never auto-injected, so it is unsuitable for project scope anyway.)
4. *(Closed — round-2 Q4)* **`GRIM_STATE_DIR` override — not needed, dropped.** The only named failure mode was a read-only `/workspace`, but project-scope install materializes skills into `<workspace>/.claude/…` etc., so a read-only workspace already fails at materialization, long before the state write — redirecting state rescues nothing. No env var added (YAGNI; additive + non-breaking later if a real need emerges). No `scope_resolution`/data-model change for this; the Codex round-2 concern is moot.
5. *(Closed — review sota-F6)* `relative` storage type = bare `String`. Normal-only invariant enforced at store time (`from_target`) + tested (T4/T7); `relative_path::RelativePathBuf` not introduced (dep for documentation value only; product-tech-strategy "no new dep without functional need"). Recorded in §1.2.
6. **Orphaned `projects/*.json` GC** — legacy files for workspaces this machine can no longer locate (renamed/deleted): reap opportunistically, defer to a future `grim gc`, or ignore?

## Next Step

```
/swarm-execute .agents/plans/plan_install_state_portability.md
```
**Pre-flight:** branch off `main` first (currently on `main`; principle #6). Suggested feature branch e.g. `feat/install-state-portability`. Decide Deferred Q3 (global residual risk) and Q4 (GRIM_STATE_DIR / read-only) before or during execution — both affect T14 and possibly T9.
