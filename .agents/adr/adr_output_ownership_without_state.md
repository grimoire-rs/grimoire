# ADR: Install records as a cache of the disk

## Metadata

**Status:** Proposed
**Date:** 2026-11-15 · **Deciders:** the owner (the model below, confirmed 2026-11-15; ratification pending)
**Beads Issue:** N/A · **Related PRD:** N/A (dossier: `.agents/discussions/stale-outputs-without-state.md`)
**Blast radius:** an external contract. No status literal, JSON field, state-schema field, lock change or rendered-byte change. Behaviour and exit changes are in open questions 1 and 2. Part B adds two internal, unstable files (B3, B4).
**Reversibility:** a medium decision and a one-way door of medium size, as classified, and the analysis agrees. The rule writes ordinary records, so the code is two-way. The documented behaviour and exit changes are one-way.
**Tech Strategy Alignment:** [x] Golden Path in `.claude/rules/product-tech-strategy.md` (no dependency, Tokio, Rust 2024)
**Domain Tags:** data | api | security · **Supersedes:** N/A · **Superseded By:** N/A

## Context

`grim` records what it rendered in `<workspace>/.grimoire/state.json`, gitignored and machine-local. A record names its pin: `InstallRecord.source` carries registry, repository and digest (`src/install/install_state.rs:565-578`, `src/lock/locked_source.rs:41-56`).

Two checkout models are supported. With **committed outputs**, the default and this repository's own, git moves the lock and the files together and the record lags. With **gitignored outputs**, the lock moves and the record still describes the disk. Neither the lock nor the record alone says what is on disk.

Two cases break today:
- **The record lags the files.** After a pull or a branch checkout, record at pin A, lock at B, files a correct render of B. The hash check runs before any pin comparison (`src/install/installer.rs:1996-2005`; the pin only at `:2035`), and status tests `modified` before `outdated` (`src/command/status.rs:2884-2890`). So install exits `65` and status reads `modified`, which the code cannot tell from a hand edit.
- **There is no record** (a clone or a second worktree). Removal walks records only (`src/install/prune.rs:237-243`). Worse than reported: `grim update` exits `65` (`untracked-destination`) on every artifact whose render changed (`src/install/installer.rs:1271-1296`, `src/command/update.rs:480-491`). `--force` overwrites and silently leaves every dropped member, and a named run reports `removed` for a member it never touched (`src/command/update.rs:1380-1395`).

Evidence is on disk. Whoever moves a committed lock commits the resulting tree, so the lock as found describes the disk; inside `grim update` the previous lock does (`src/command/update.rs:136`). Install already adopts a footprint identical to a render of the pin being installed (`src/install/installer.rs:1154-1161`).

## Decision: the record is a cache of the disk

**Rule:** a record is trusted while the files match it; when they do not and the lock names another digest, the files are compared with a render of that digest, and only a mismatch means a local edit.

`state.json` is the last state grim observed on disk, per artifact, keyed by the pinned identifier it rendered. It is validated against the files first, and against the lock only when the files disagree with it.

| Files vs record | Lock digest vs record digest | State | Today |
|---|---|---|---|
| unchanged | same | `installed` | works |
| unchanged | changed | `outdated`: out of sync; `grim install` re-renders to the lock's digest; the record names the old files to remove | works |
| changed | same | `modified`: a local edit; refuse | works |
| changed | changed | compare the files with a render of the lock's digest. Match: update the record. No match: `modified`, refuse | broken: always `modified`, exit 65, because the hash check runs before any pin comparison (`src/install/installer.rs:1988-2003`, `src/command/status.rs:2884-2890`) |
| no record, files exist | n/a | the same comparison against the lock's digest (in `grim update`: against the previous lock's digest, so files the new version dropped can be removed) | broken for `grim update` in a clone |

Rows 1 to 3 need no render: a digest comparison and a file hash (`src/install/content_hash.rs:88-131`). Rows 4 and 5 compare against a render, and `grim install` already holds that content. "The lock" is the **evidence lock**, the one that describes the disk: the lock being installed for the gate, the lock as found for status, `uninstall` and `remove`, and the previous lock in `grim update`.

The record stays authoritative where content cannot speak: bytes an older renderer or layout wrote (rows 1 and 2 keep them), dev installs, nodes no lock on disk names, and the MCP `adopted` flag, which only this machine's write clears.

**Part A** (rows 4 and 5 in the gate, status precedence and the mutating commands) ships on its own. **Part B** (status, the TUI and MCP apply rows 4 and 5 automatically, and fetch) is an owner decision on top; it carries the security findings and may ship after A.

### Owner decisions, 2026-11-15

1. `grim status`, TUI open and MCP `grim_status` apply rows 4 and 5 automatically, record only, at both scopes. The record is machine-local, so a reporting command may write it.
2. On a cold store, status and the TUI fetch when online; offline they skip and say so.
3. Visible, non-blocking, a very fast no-op.
4. Future commands must not silently skip the rule (B8).
5. Status keeps fetching after the owner was shown the security finding: an accepted risk, recorded in the owner's name on 2026-11-15.
6. The credential-transport defect class is fixed first, as a prerequisite of fetching status (B9).
7. The lagging record "should make no difference": row 4.
8. The model and the table above, confirmed in the owner's words.

These supersede the dossier's "`grim status` never writes" and "never fetches" (`.agents/discussions/stale-outputs-without-state.md:53-60`).

## Decision Drivers

- Grim removes only what it proves it wrote (`.agents/plans/bugfix_plan_reaper_ownership.md:42`, `:79`).
- Principle 9: additive only. `ClientOutput` is `deny_unknown_fields` (`src/install/install_state.rs:84-140`).
- A repository is untrusted (T3, `.claude/rules/arch-threat-model.md:46`): lock, outputs and `grimoire.toml` arrive with it.
- `remove` and `uninstall` stay network-free (`.agents/adr/adr_effective_set_mutations.md:42`); "install never prunes" and "remove never deletes" (`.agents/adr/adr_lock_pin_stability.md:183`).
- Both checkout models stay supported (`.agents/discussions/stale-outputs-without-state.md:46-48`). Prior art: `.agents/research/research_generated_files_ownership_prior_art.md`; no surveyed tool deletes from a previous lock alone.

## Considered Options

- **O1 Do nothing; document `grim install` before `grim update`.** `grim install` at the committed lock adopts identical files at the old pins. It relies on every human and agent knowing the step, and does nothing for row 4.
- **O2 The model (chosen).** Part A alone, or A plus B.
- **O3 Reuse the install gate against the previous pin.** Inside the pass, dropped nodes stay unreachable because the pass iterates the new lock. Run first at the previous lock, it rewrites identical bytes, restores deleted files and runs persist, vendor sync and hook convergence at a lock about to be replaced (`src/install/installer.rs:1268-1270`, `:1301-1504`).
- **O4 The mapping in the lock, or another travelling record.** Every lock table is `deny_unknown_fields` since v0.1.0 (`src/lock/grimoire_lock.rs:29,92,128`, `src/lock/locked_artifact.rs:16,80`, `src/lock/locked_bundle.rs:147`), so an older grim exits `78` on a new field or table (`docs/src/content/docs/stability.md:753-761`): a Principle 9 break. The client set is partly per machine (`src/install/target.rs:166-200`). With gitignored outputs the lock would describe files that are not on disk. A committed `state.json` is attacker-suppliable (`.claude/rules/arch-threat-model.md:72-89`).
- **O5 Remove the state file; derive ownership from lock and disk.** Weighed before as `adr_install_state_portability.md` Option 4 (`:170-181`). Decisive losses: no lock entry proves bytes an older renderer or layout wrote, which the Principle 9 reapers need (`docs/src/content/docs/stability.md` § The compatibility promise); a dev install has no lock entry (`src/install/prune.rs:242`); a value match would delete a user's MCP entry (`src/install/uninstall.rs:267-290`). Since that ADR, renderer-dependent bytes, layout moves, dev installs, the `adopted` flag and pool refcounts all added record-only facts.
- **O6 Delete by names, no content match.** It deletes hand-edited and user files at those names (`.agents/plans/bugfix_plan_reaper_ownership.md:79`).
- **O7 Wipe and regenerate owned directories.** Client roots hold user and third-party content.
- **Rejected variant: void the record when the pins differ** (raised and withdrawn in discussion). A pin mismatch is the normal `grim update` flow (`src/install/installer.rs:1089`, `:2098`) and the correct state for gitignored outputs after a checkout. Later code still reads the old record: the peer set (`src/install/expected_outputs.rs:272-295`), the pin-change carry-forward (`installer.rs:1624-1640`), the reapers (`:2098`, `:2475`), prune and dropped clients (`src/install/prune.rs:201`, `:480`) and the MCP `adopted` flag.

Scored 1 to 5, 5 best; weights add up to 20.

| Criterion | Weight | O1 | O2 A | O2 A+B | O3 | O4 | O5 | O6 | O7 |
|---|---|---|---|---|---|---|---|---|---|
| Removes only what grim proves it wrote | 5 | 4 | 4 | 4 | 3 | 2 | 2 | 1 | 1 |
| Fixes clone, pull and checkout without an extra step | 4 | 1 | 4 | 5 | 4 | 5 | 5 | 4 | 4 |
| Principle 9 | 4 | 5 | 4 | 4 | 4 | 1 | 1 | 4 | 1 |
| Keeps prior Accepted decisions | 3 | 5 | 4 | 4 | 2 | 1 | 1 | 1 | 2 |
| Offline and cold store degrade safely | 2 | 4 | 4 | 4 | 4 | 5 | 2 | 5 | 3 |
| Size and new surface | 2 | 5 | 3 | 2 | 4 | 2 | 1 | 4 | 2 |
| **Weighted total (max 100)** | | **77** | **78** | **80** | **69** | **51** | **43** | **58** | **41** |
| Gate: no repository-delivered record, no deletion without proof | | pass | pass | pass | pass | fail | fail | fail | fail |

The matrix does not separate O1 from O2 (77, 78, 80), and no honest weighting changes that. The owner's requirement decides: the lagging record "should make no difference", which O1 cannot deliver.

## Part A: the core fix

Part A is complete without Part B. It touches the install gate, status precedence, `update`, `uninstall`, `remove` and their TUI forms.

- **A1 One classifier.** For a node with a record, hash each present recorded output and compare with the record, then compare the evidence lock's digest with the record's. Rows 1 to 3 follow with no render. Row 4 goes to proof (A2) against the evidence entry. A node with no record whose destinations exist (A6) is row 5. Users: the gate, status, and the pre-passes of A5. Skill, rule, agent and mcp only; hooks, settings, bundles and dev records are never proven.
- **A2 Proof.** A file destination is proven only if all four hold:
  1. it exists with the preview's file type, and so does its support directory (a directory `foo.md/` holding `foo.md` hashes like the file, `src/install/content_hash.rs:50-67`);
  2. neither is a symbolic link;
  3. it resolves under its anchor with `Containment::Strict`;
  4. its `footprint_hash_for_kind` (`:100`) equals that of a preview rendered from the evidence entry with its pin string (`src/lock/locked_source.rs:111-116`).

  A rule whose index is gone is never proven. An MCP member is proven when its value, vendor-owned keys stripped, equals what grim would write (`src/install/installer.rs:3223-3272`). The preview reuses `stage_locked_artifact` (`:2712`) and the comparison at `:1271-1296`, extracted first as its own refactoring commit.
- **A3 Write.**
  - **Row 4 match:** the record takes the evidence entry's `source` and the matched hashes, only when every present recorded output proves against that entry. `support_dir`, `entry` and `adopted` keep their values. Otherwise nothing changes, and the row stays `modified`.
  - **Row 5 match:** one new record, `dev: false`, with outputs for the proven destinations only. A rule gets an anchored `support_dir` exactly when the evidence version ships one (`src/install/installer.rs:1368-1396`). Files get `adopted: false`, MCP outputs `entry` and `adopted: true`.
  - No other byte is written: no file under a client root, no vendor sync, no hook convergence.
- **A4 The gate.** Where the gate would refuse a drifted output (`src/install/installer.rs:1996-2005`) and the record's digest differs from the lock's, the refusal waits for the comparison after staging. Match: re-recorded, identical bytes re-materialize (`:1297`, recorded at `:1666`), and the row reads `unchanged`. No match: `65`, `reason: modified`. Same digest: refused at once, as today. Row 5 at the gate already works (`:1154-1161`). This covers `install`, `add`'s install, TUI Install and the gate inside `update`.
- **A5 Status precedence.** Row 4 is evaluated against the lock as found, store only, in memory, writing nothing. Match: `installed`, with `outputs` from the proven record. No match: `modified`. Content not in the store: `modified`, plus one stderr note naming `grim install`. Row 5 stays `missing` until Part B. A `stale` row still reads `stale` first (`src/command/status.rs:2875-2877`).
- **A6 Pre-passes for rows 4 and 5.** Candidates are found by presence: each client in `effective_clients` that hosts the kind (`src/install/expected_outputs.rs:272-318`, filters at `src/install/installer.rs:1110-1152`), at `destination(..)`, plus a rule's `<name>/`, each tested with `symlink_metadata`. Presence is never deletion authority.

  | Command | Evidence | Content | Nodes | Persist |
  |---|---|---|---|---|
  | `grim update` (bare) | previous lock | store, then registry online | every node of the previous lock | before `lock_io::save` (`src/command/update.rs:173`) |
  | `grim update <names>`, TUI Update | previous lock | as above | `NamedScope::force()` (`update.rs:709-711`) ∩ previous lock | before the save; TUI before `save_relock` (`src/tui/app.rs:4091-4100`) |
  | `grim uninstall`, TUI delete | lock as found | store only | the target plus the `plan_hold` orphans (`src/command/uninstall.rs:172-174`) | before the edit; failure exits `74` |
  | `grim remove` | lock as found | store only | nodes absent from `drop_from_lock`'s result (`src/command/remove.rs:100-117`) | before `save_lock`; failure warns, exit kept |

  With no evidence lock there is no pre-pass. When every node is rows 1 to 3, the pre-pass renders nothing, sends no request and writes nothing.
- **A7 `update` ordering and outcomes.** The install target (`src/command/update.rs:190`) and the state load (`:200`) move ahead of the lock save, because the save destroys the evidence and the run can return before the persist at `:281`. `state_before` (`:205`) then holds the proven records, so vendor sync retires their managed elements. An unreadable state (`74`), an unparsable client set (`78`) and a failed persist (`74`) now exit before the lock write. Rows stay exactly as D3 defines them (`.agents/adr/adr_lock_pin_stability.md:178`). Outcomes:

  | Node | Result |
  |---|---|
  | still shipped, moved pin, proven | `updated`, exit `0` (was `65`) |
  | carried, proven, nothing missing | `unchanged`, no write |
  | carried, one client proven, another's destination absent | materializes (`src/install/installer.rs:2033-2036`), `restored` |
  | dropped, proven | prune row `removed`, files deleted |
  | dropped, proven for one client only | only the recorded output is deleted (`src/install/prune.rs:295-300`); A9 names the rest |
  | still shipped, unproven | the existing `65`, `refused: true`; a link's message names the link and its target |
  | adopted MCP entry dropped | `removed`, entry kept, named in `abandoned_entries`; `--force` removes it |
- **A8 `uninstall` and `remove`.** Proven outputs go through the record-driven delete. An adopted MCP entry stays unless `--force`. A drifted MCP entry that proves against the evidence entry is re-recorded and removed with no `65`. `--force` creates no ownership, so unproven files stay. `uninstall` reads `uninstalled` (`src/command/uninstall.rs:253-254`); a node a bundle holds is not proven. `remove` deletes nothing, and a later bare `grim update` prunes (`src/install/prune.rs:237-243`).
- **A9 Unproven files.**
  - **The warning:** one stderr line per node left on disk: `<kind> '<name>' left <path>[, <path>…] on disk: grim could not prove it wrote <it|them> (<reason>)`, then `delete it by hand if unwanted`. A link gets "remove the link only"; an ancestor link outside the root gets no removal advice. A node that already has a refusal error, or the kept-entry warning (`src/install/uninstall.rs:284`), gets no second line. `warn_left_behind` (`src/command/update.rs:440`) names only nodes with a record.
  - **Reasons:** the files differ from `<pin>`; a directory stands where a file belongs; the rule's index is gone; the content is not in the local store (offline); the fetch or verification failed; the host failed earlier; the local source no longer matches; unreadable; render failed; a symbolic link to `<resolved>`; under `<link>` outside its root; never attempted (no evidence entry, or outside the closure).
  - **Tests** assert kind, name, each path and the resolved link target, never whole sentences.

  | Old pin content | `update` | `uninstall`, `remove` |
  |---|---|---|
  | in the store | prove | prove |
  | not in the store, online | fetch through the run's seam (`src/command/update.rs:134-135`), then prove | unproven, named |
  | offline, or gone (404) | unproven, named | unproven, named |
  | path source | packed from the workspace (`src/install/installer.rs:2760-2789`); drifted means unproven | same |
- **A10 Exits and scope.** The rule sets no exit, except the persist failures in A6 and A7. In `update`, a still-shipped unproven node keeps the existing `65`. At global scope the evidence lock is `$GRIM_HOME`'s and the state is `global.json`, with no scope branch in the gate or the prune.

## Part B: status, TUI and MCP apply the rule automatically

Owner decisions 1 to 6. B needs A, and its fetching needs B9 first.

- **B1 Surfaces.** `grim status`, TUI open and scope toggle, and `grim_status` run A1 to A3 for rows 4 and 5 against the lock as found, at both scopes, and commit (B3). Status and the TUI fetch when online, under B4. `grim_status` reads the store only and commits only under `--allow-writes`.
- **B2 Fast no-op.** Rows 1 to 3 cost what status pays today: it already hashes every recorded output (`src/command/status.rs:2962`). A recordless node whose destinations are all absent costs stat calls only. With nothing proven there is no commit: `state.json` keeps its bytes and mtime, and no `.grimoire/` appears where there are no outputs.
- **B3 Commit, never the config lock.**
  - **The lock:** status, the TUI loop and `grim_status` never take the config lock, so a status commit cannot cause the `75` every mutating command gets on a held config lock (`src/command/install.rs:81`, `add.rs:151`, `remove.rs:61`, `uninstall.rs:95`, `lock.rs:58`, `update.rs:128`, `init.rs:103`, `config.rs:1391`, TUI `config_guard` at `src/tui/app.rs:3020-3024`). The commit takes the state-file lock, an `AdvisoryFileLock` sidecar (`.grimoire/state.json.lock`, `$GRIM_HOME/state/global.json.lock`), with `try_acquire`. It holds it only to reload, re-check and `persist` (`src/install/install_state.rs:1325-1349`), never across a fetch or render.
  - **Re-check:** a record is written only for a node still unchanged since the proof: the same record bytes, the same evidence pin, and files that still hash to the match. A record a concurrent command wrote or removed is never overwritten or resurrected.
  - **Persist:** `persist` takes the same lock, polling up to 2 s, then writes unlocked as today. A status commit can delay a persist, never fail it. A mutating command may drop records status added after its load; they are proven again.
  - **Degradation:** on contention, any I/O error, a read-only server, or a `.grimoire` or `state.json` that is a link or resolves outside the workspace, status skips the write, reports from memory, and keeps the exit. Today `ensure_project_state_dir` follows such a link (`src/install/install_state.rs:799-815`); extending the guard to other commands is open question 3.
- **B4 Fetch bounds and memo.** Today the registry client sets no deadline (`src/command/status.rs:142-144`); only `--check` adds one (`:1209-1216`). Every B fetch therefore runs under these limits, defaults the plan may tune, each tested:
  - **Deadlines:** 10 s per call; 30 s per run for the CLI and MCP, 60 s for the TUI task. A call past its deadline counts as a host failure.
  - **Caps:** at most 64 manifest and 64 layer fetches per run; a manifest body of at most 4 MiB, refused before its digest check; at most 512 MiB of decoded layers per run, besides each layer's cap.
  - **Memo:** `$GRIM_HOME/state/adoption-misses.json`, internal and unstable, under its own advisory lock. It skips a failed host for 10 minutes and a digest answered `404` for 24 hours, so a polling editor repeats no request.
  - **Repeat cost:** a row 4 or 5 node with local content and no match costs one render and hash per run.
  - **The seam:** plain status builds no access seam today (`src/command/status.rs:686`). It builds one only when a fetch is needed; a failure degrades to store-only with the exit kept.
- **B5 Visibility.**
  - **The running line** uses the `--progress` sink (`src/cli/options.rs:38-53`, `src/cli/progress.rs:66-103`): `start` with the candidate count, `advance` labelled `adopt <kind> <name>`, `finish`. Under `auto` it shows on a terminal, under `json` as NDJSON, and under `none` not at all.
  - **Outcome lines** are `warn`-level on stderr in both formats, so `GRIM_LOG=warn` shows them and `--log-level off` silences them. Tokens: `adopted <a> artifacts into the install record; <u> left unproven`, `records not written: <cause>`, and `offline: <u> artifacts were not proven`.
  - **The TUI status line:** `adopting <n> artifacts in the background…`, then `adopted <a> · <u> unproven (tui.log)`.
- **B6 TUI.** After the first `load_into` (`src/tui/app.rs:320-323`) and on a scope toggle, the TUI calls `adoption::spawn`. The task uses the bounded `JoinSet`-plus-channel pattern of `CompanionFetcher` (`src/tui/companion_fetch.rs:130-171`). It never commits: the 200 ms loop (`src/tui/app.rs:346-356`) commits its result between actions, so an action and the task never contend. The badges are then re-derived.
- **B7 MCP.**
  - **Plumbing:** `status::run_with(ctx, args, StatusMode { content, commit })` is a non-CLI parameter; `grim_status` (`src/mcp/server.rs:96-109`) passes store-only content, the existing call at `src/command/status.rs:714-715`. The write gate is `build_router` (`src/mcp/server.rs:47-52`); `McpState.allow_writes` is dead code (`src/mcp/state.rs:25-34`), and the plan makes it live.
  - **Fetching:** "never fetches" covers the rule only, because `check: true` already contacts hosts.
  - **Repeat cost:** without writes, a per-process memo keyed by scope, node, pin, client and footprint hash avoids re-rendering each call.
- **B8 Future commands cannot skip the rule.** Every loader returns a typestate; `InstallState::load*` and `empty` become private to `install_state.rs`:

  ```rust
  pub(crate) struct Unadopted { state: InstallState }
  impl Unadopted { fn exempt(self, why: Exempt) -> InstallState; fn defer(self) -> (InstallState, Deferred); }
  pub(crate) async fn adopt(state: Unadopted, env: AdoptEnv<'_>) -> Adopted;   // then Adopted::commit(HeldLock | TryLock | InMemory)
  pub(crate) enum Exempt { InstallGate, Browse, HookRecords, SettingsPreflight, DevRecords, CrossScopeShadow }
  ```

  A new caller that does not choose does not compile. Gaps: a wrong `Exempt` compiles (review owns it), and a hand-written parse of `state.json` would bypass it (none exists). Part A's callers adopt through the same type.

  | Call sites | Class |
  |---|---|
  | `status` (`src/command/status.rs:246`), `update` (`update.rs:200`), `uninstall` (`uninstall.rs:129`), `remove` (new), TUI `perform_uninstall` (`src/tui/app.rs:3080`), `perform_member_uninstall` (`:6170`), TUI Update in `perform` (new, before `:3365`), `grim_status` | `adopt` |
  | TUI open and scope toggle (new, beside `:320-323`) | `defer` |
  | `install` (`src/command/install.rs:107`, `:408`), `add` (`add.rs:1286`), TUI `install_row` (`src/tui/app.rs:3812`), `perform_local` (`:4209`) | `InstallGate` (A4) |
  | `search` (`src/command/search.rs:693`, `:758`), TUI `:526`, `:1134`, `load_scope_for_badges` (`:2250`) | `Browse` |
  | `hook list` (`src/command/hook/list.rs:151`) | `HookRecords` |
  | `add` settings preflight (`src/command/add.rs:496`), TUI `:4143`, `SettingsGateRequest::read` (`:5211`) | `SettingsPreflight` |
  | `add` dev collision guard (`src/command/add.rs:595`), TUI `perform_local_uninstall` (`src/tui/app.rs:3194`), `perform_local_dev` (`:4375`) | `DevRecords` |
  | `other_scope_state` (`src/install/installer.rs:655`, `:659`) | `CrossScopeShadow` |

  The TUI's own loader (`src/tui/app.rs:5186-5191`) is routed through `scope_resolution::load_state` (`src/command/scope_resolution.rs:313`). The other 15 top-level commands load no install state. A new command's author gets `Unadopted` and must choose: `adopt` with the lock that describes the disk if it reports or changes install state, else an `Exempt` reason. Outside the code change: one line in `.claude/rules/subsystem-cli-commands.md` § Conventions.
- **B9 Prerequisite: credential transport.** The security re-check confirmed a shipped defect class on the pull path: transport security for a registry that only a project file declares insecure. The plan fixes it before fetching status ships. Fixed means **a file a repository delivers can never cause a stored credential to travel over an unencrypted connection**, for every credential kind and every hop (registry, token realm, redirect). The decision is made where the credential is attached. `grim login` already holds this line (`src/auth/verify.rs:101-105`, `:166-170`).

## Drift cases behind the model

| Case | Model row | Today | Under this design |
|---|---|---|---|
| Clone, or a second worktree (own `state.json`, `.claude/rules/subsystem-install-state.md` § Project state) | 5 | status `missing`; `update` `65`; dropped nodes never pruned | A6, B1 |
| Pull of a teammate's update; checkout of an older branch, committed outputs | 4 | `modified`, `65` | A4, A5, A6 |
| Checkout, gitignored outputs | 2 | `outdated`; install re-renders | unchanged |
| Record names files git removed | none | status `missing` (`src/command/status.rs:2950-2954`); install restores (`src/install/installer.rs:2009-2012`) | unchanged |
| A node left the lock with no record | 5 | never pruned (`src/install/prune.rs:237-243`) | proven from the previous lock or before the edit; after a `git pull` alone: stays, unnamed |
| `state.json` replaced by git | 1 to 3 | loaded as found; settings records and every output that is not a legal surface for its kind are dropped (`src/install/install_state.rs:1018-1040`, `:1629-1665`) | unchanged; row 4 now outweighs its hashes |
| Grim upgraded, renderer bytes change | 1, 2 | the old bytes stay at the same pin (`installer.rs:2035`) and are overwritten on the next pin move | unchanged: only the record proves them |

A teammate whose grim renders differently commits files this grim cannot reproduce: they stay `modified` or unproven.

## Threat model

**Attacker:** T3. The repository supplies `grimoire.toml` (including `insecure = true`), the lock, the outputs, links (`.grimoire` included) and environment files; T1 can publish what a hostile lock names.

**Part A.** No new trigger: `update` fetches the old pin as it fetches the new one, and `uninstall` and `remove` stay offline. Writes are records only, equal to what `grim install` writes on the same proof, so deletion is not widened. At project scope destinations anchor only at `Workspace` (`src/install/path_anchor.rs:814`); a committed link that leads out fails `Containment::Strict`. Links and empty directories inside an adopted directory are skipped by the hash (`src/install/content_hash.rs:223-228`) and unlinked, never followed. At global scope only files byte-identical to renders of the user's own global pins are proven. Hooks and settings are never proven, so I1 and I4 hold.

**Part B.** The new element is the trigger. Plain `grim status`, which editors, agents and scripts run with no gesture, now contacts registries a repository names, on a cold store online. `grimoire-vscode` runs plain `status` in every workspace (`vscode/src/grim.ts:1006-1012`) and gates only `--check` on trust (`vscode/src/extension.ts:541-542`). `grim install`, `status --check` and TUI open (`src/tui/app.rs:320-331`, `:757-772`) already make such requests, and these hold for them today:
- a credential helper spawn before each call (`src/oci/access/registry_client.rs:86-93`, called at `:411`, `:453`);
- the credential-transport class (B9);
- a blind GET through a redirect or the token realm, with loopback plain HTTP by default (`registry_client.rs:138-151`);
- an unbounded manifest read (B4 caps it for B).

The last two come from the security re-check; the fork was not re-read here.

**Accepted risk, owner, 2026-11-15:** status and the TUI fetch on a cold store online from registries a repository names. That discloses source address, timing, and that the checkout was looked at. B9 and B4 are fixed first, not accepted. An older `grimoire-vscode` stays exposed under this acceptance.

**Residual:**
- **Identical copies:** a hand-made byte-identical copy can be proven and deleted, including one another tool put in the shared `~/.agents/skills`.
- **Ignored files:** files the artifact's `.grimignore` excludes go with an adopted directory.
- **Support directories:** a recordless rule's support directory whose index is gone is replaced by `grim install` without refusal (`src/install/installer.rs:1268-1270`, `:1485-1488`).
- **Escaping parent links:** install writes through an escaping parent link (open question 3).

## Compatibility

Read-only CLI status is not a written promise: `docs/src/content/docs/commands.md:1321-1436` states none, and two rules only imply it (`.claude/rules/subsystem-install-state.md` § Reap window, `.claude/rules/subsystem-cli-commands.md`). The MCP server's "read-only by default" is written (`commands.md:4272-4274`) and B7 keeps it. Nothing outside exit codes and JSON is frozen (`docs/src/content/docs/stability.md:587-590`).

| Item | Effect | Classification |
|---|---|---|
| State schema, lock, rendered bytes | Unchanged; 0.15.1 reads the records | Unchanged |
| `status` JSON | No field; row 4 matches read `installed` (A), row 5 matches `installed` (B) | Same literals and meanings |
| `install`, `update` exits | `0` where they exited `65` on a proven node | Open question 1 |
| `update`, `uninstall` failure order | State, client-set and persist failures exit before the lock or config edit | Open question 1 |
| `update`, `uninstall`, `remove` JSON | Unchanged | Rows as D3 |
| Status, TUI behaviour | Write records; may fetch, bounded | Open question 2 |
| Credential transport | A credential no longer travels in cleartext to a host only a project file declares insecure; such a pull may exit `80` | Open question 2 |
| `grim_status` | Writes only under `--allow-writes`; the rule never fetches | Keeps the read-only default |
| Persist timing | May wait up to 2 s for a status commit | Timing only |
| `grimoire-vscode` | Coordinated change: pass `--offline` to plain `status` while the workspace is untrusted | No grim contract change |

**Sentences to change:**
- `commands.md`: the status and `install` sections (`:1352-1357`, the `modified` row at `:1585`), the MCP paragraph (`:4272-4274`, `:4299`), and one sentence each for `update`, `uninstall` and `remove`.
- `json-interface.md`: `:96` and `:150-151`.
- `stability.md`: once ratified.
- AI config, outside the code change: `subsystem-install-state.md` (the Reap window) and `subsystem-cli-commands.md` (the rows and the B8 line).

## Plan order

1. **Part A** (A1 to A10): it ships alone, needs no fetching status, and fixes the pull, the checkout and the clone's `update`.
2. **B9 and B4:** they also harden `install`, `status --check` and the TUI, and ship on their own.
3. **Part B** (B1 to B8), after 1 and 2. Records that status writes, read by a polling editor, turn every clone into row 4 after the next pull, which is why A1 comes first.
4. **Docs, catalog skills** (`catalog/skills/grim-usage/references/consume.md:427-431`, `updating.md`, `troubleshooting.md`), and the `grimoire-vscode` change, which is harmless today and may ship first.

## Validation

Acceptance tests in `test/tests/test_ownership_without_state.py`, each from an installed project, with `.grimoire/` deleted or rewritten. A cold store is a second `GrimRunner` with a fresh `GRIM_HOME`; a pull is a tree and lock rendered by another runner.

**Rows 1 to 3 (unchanged)**
1. With the store emptied and `--offline`, `status`, `install` and `update` give today's results for each row: `installed` and no write; `outdated`, re-render and reap; `modified` and `65`. No render happens (A1).

**Row 4**
2. `install` after a pull, failing first: `65` today, then `0` and `unchanged`; status then reads `installed` (A4).
3. `install` after a checkout of an older committed branch: `0`, `unchanged` (A4).
4. `status` after a pull: `installed` with the content in the store and nothing written; `modified` plus the `grim install` note without it (A5).
5. `update` after a pull: the pre-pass re-pins from the previous lock, the run rolls forward and exits `0` (A6, A7).
6. A hand edit after a pull, and a two-client record with one client edited: `modified`, `65`, record unchanged (A3, A4).
7. `uninstall` of an MCP entry drifted to grim's value at the lock: removed, no `65` (A8).

**Row 5**
8. `update` in a clone to a version that drops a file inside a skill, failing first: `65` today, then `0`, the file gone, the row `updated` (A7).
9. A bundle that drops a member, and a renamed rule: the old files go with `removed` rows (A7).
10. A carried pin reads `unchanged` and keeps its mtime; with a second client's destination absent it reads `restored` (A7).
11. Two clients, a dropped skill proven for one and edited for the other: `removed`, the match goes, the edit stays and is named (A7, A9).
12. `grim update <bundle>` over an edited dropped member: one warning, not `warn_left_behind`. An edited still-shipped artifact: `65`, no second line (A9).
13. An extra regular file blocks proof. An extra symlink or empty directory does not, and its target survives a drop. A `foo.md/` directory, a symlinked destination and a committed `.claude` that links out are not proven (A2).
14. A rule whose support directory survives its index: named on a dropping `update` (A9).
15. A cold store online: the old pin is fetched and the stale files go. Two dropped rows on an unreachable host make one attempt (A9).
16. Offline with the old pin absent, gone from the registry, or a drifted path source: no `81`, the files stay and are named (A9, A10).
17. An unreadable `state.json`: `update` exits `74` and the lock is unchanged (A7).
18. Every record a cache hit: `update` sends no proof request, even with pins naming an unreachable host (A6).
19. `uninstall` with a warm store deletes; with a cold store or `--force` it leaves, warns, reads `uninstalled`, exits `0` (A8).
20. `remove` with a warm store, then a bare `update`, prunes; with a cold store the files stay and are named (A8).
21. A dropped MCP entry stays, named in `abandoned_entries`; `--force` removes it (A7).
22. `grim install` at the same version adopts and reads `unchanged`; `test/tests/test_clobber_guard.py:90-111` stays green (A4).
23. `grim --global update` with `global.json` deleted removes a dropped skill (A10).

**Part B**
24. Status in a clone writes the record and reads `installed`. A second run leaves `state.json`'s bytes and mtime unchanged and prints no line (B1, B2).
25. A cold store online: one fetch, then `adopted 1`; `--format json` stdout has no new key (B1, B5).
26. A cold store offline: no request, an `offline:` line, the row reads `missing` (B1, B5).
27. A read-only `.grimoire/`, where the sidecar cannot be created; and a `.grimoire` linked outside the workspace: `installed` from memory, `records not written:`, exit `0`, nothing written outside (B3).
28. Hold the state-file lock: status skips its write. Run `install` during a delayed status commit, and beside 20 status runs: no `75`, and the final state equals the install's (B3).
29. No outputs: no `.grimoire/` created (B2).
30. A status commit reaps the legacy project state file and leaves the settings records byte-identical (`src/install/install_state.rs:1272-1277`, `:1345-1347`).
31. `--progress json`, `--progress none` and `--log-level off` behave as B5 says.
32. A silent registry: the run ends inside its deadline with exit `0`, and a second run within 10 minutes sends nothing (B4).
33. Credential transport, failing first: a stored credential, and a registry only the project file declares insecure. A capturing registry sees no credential from `status` or `install` (B9).
34. `grim_status`: without `--allow-writes` it reports `installed` and writes nothing; with it, it writes. On a cold store it sends no request, and a repeated call renders nothing (B7).
35. TUI at global scope with `global.json` deleted: a scope toggle starts the task and re-derives the badges (B6).
36. A 0.15.1 interop module reads a `state.json` written by B (`task test:old-grim`).

**Unit tests:**
- A1's classifier for every row.
- A3's partial-proof rule.
- A failing lock writer leaves the proven records (A7).
- `build_report` rows unchanged, with `a_dropped_member_with_no_record_keeps_its_removed_row` green.
- B3's re-check.
- B4's timeouts and caps.
- In the TUI `TestBackend` pattern (`src/tui/app.rs:10610`): live input during the task, the loop committing between actions, and the pre-passes before `save_relock` and `delete_nodes`.

**Gates:** `task verify`, `task catalog:verify`, `task docs:check`.

## Boundary and convention violations

- **The record loses authority** where content can prove ownership. The gate and status were record-first.
- **Status writes** (B), against the implied read path. `grim_status` writes only under `--allow-writes`.
- **A filesystem probe** finds row 5 candidates. It never feeds a deletion.
- **`remove` reads and writes state** (`src/command/remove.rs:123`), and `uninstall` and `remove` render into a temporary directory. Both stay offline.
- **`update` writes state before the lock** and fails on state errors earlier (A7).
- **`removed` rows can leave files** (a kept MCP entry, a partial proof). `abandoned_entries` and A9 carry the truth.
- **A second lock on the state file** (B3). The TUI's own state loader is retired (B8).

## Open questions

[NEEDS CLARIFICATION: Ratify Part A's changes with no additive form: `install` and `update` exit `0` where they exited `65` on a node rows 4 and 5 prove; status reads `installed` where it read `modified`; `update` and `uninstall` fail on state, client-set or persist errors before the lock or config edit.]
Recommended: ratify. These are owner decision 7 and the model; the refusals pointed users at a destructive `--force`. No JSON shape changes.

[NEEDS CLARIFICATION: Ratify Part B's changes: status and the TUI write machine-local records and fetch, bounded, under the accepted risk; `grim_status` writes only under `--allow-writes`; after B9 a pull that sent a credential in cleartext to a host only a project file declares insecure may exit `80`.]
Recommended: ratify, as owner decisions 1, 2, 5 and 6, with the MCP server's written read-only promise kept.

[NEEDS CLARIFICATION: Should every command refuse to write through a link that leaves the workspace: the `.grimoire` directory (`src/install/install_state.rs:799-815`) and an output's parent (`src/install/installer.rs:1191-1193`, `:1436-1440`, `:1477`, where `--force` replaces a target)? B3 refuses it for status surfaces only.]
Recommended: yes, as its own bugfix plan, refusing with `65` like other containment failures. It changes shipped `install` behaviour.

## Unresolved review findings

**This revision is unreviewed.** It restructures the third revision around the owner's model; earlier revisions had spec, quality and security reviews, a cross-model pass and two re-checks.

Open:
- The redirect, token-realm and manifest-read claims about the `oci-client` fork come from the security re-check, and were not re-read here.
- The TUI task wiring beyond the cited lines is unverified.
- Open question 3.

## Links

- Dossier: `.agents/discussions/stale-outputs-without-state.md`
- Research: `.agents/research/research_output_ownership_claim_diff.md` (§ Record pin, lock pin and the files), `.agents/research/research_output_ownership_without_state.md`, `.agents/research/research_generated_files_ownership_prior_art.md`
- ADRs and plans: `adr_install_state_portability.md`, `adr_effective_set_mutations.md`, `adr_lock_pin_stability.md`, `adr_render_layout_stability.md`, `adr_vendor_config_and_selection.md` (all `.agents/adr/`), `.agents/plans/bugfix_plan_reaper_ownership.md`
- Rules and docs: `.claude/rules/arch-threat-model.md`, `subsystem-install-state.md`, `subsystem-cli-commands.md`, `docs/src/content/docs/stability.md`, `json-interface.md`
- `grimoire-vscode` (sibling repository): `vscode/src/grim.ts:1006-1012`, `vscode/src/extension.ts:541-542`
- Issue, out of scope with its own plan: [grimoire-rs/grimoire#257](https://github.com/grimoire-rs/grimoire/issues/257)
