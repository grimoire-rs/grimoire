# WP-T — SEC-1 closed: hook payloads moved out of the workspace

**Status:** implemented, verified, committed on `hex/hooks-artifact-kind--wp5-t` (base `31a9154`).
**Gates:** `cargo fmt` clean · `cargo clippy --locked --all-targets -- -D warnings` clean ·
`cargo test --bin grim` **2896 passed** · `task verify` green (**1074 acceptance passed, 0
xfailed**, 51 AI-config passed). The `commit-verified` stamp was written by `task verify` itself.
**Executed proof:** § 5 — the same attacker artifact **arms** under the pre-fix binary and **does
not arm** under the fixed one.

WP-R's strict xfail is now a normal, passing, **parametrized** test.

---

## 1. The new layout, and the root key

| Scope | Payload directory |
|---|---|
| global | `$GRIM_HOME/hooks/<name>/` — unchanged |
| project | `$GRIM_HOME/hooks/payload/<workspace-key>/<name>/` |

`<workspace-key>` is **SHA-256 of the absolute workspace path, hex** — the same formula
`InstallState::legacy_project_path` uses to hash a path into a `$GRIM_HOME` segment, so there is
one spelling of "hash a path into a segment" rather than two. `hook_dispatch::payload_relative`
returns the `$GRIM_HOME`-relative remainder and `payload_dir` joins it; both are pure.

### I did not reuse `root_token`, and the reason is a real weakening of B3

The brief recommended reusing `hook_dispatch::root_token` — "keeps one spelling of *which
workspace*". I kept one spelling of the **semantic** root (`RootScope`, via the existing
`root_scope_for`, which is now `pub` and is the single mapping both derivations key on) but gave
the path its own derivation. Two independent reasons, both load-bearing:

1. **The token would be published.** A recorded install target is ordinary user-visible data: it is
   written into `state.json`, printed in the `Target` column of `grim install`, and reported by
   `grim status --format json` (`outputs_pending[].path`). Putting the token in the path therefore
   discloses it — and B3's entire property is that a hostile repository **cannot compute the
   victim's token**, because a committed registration naming a known `--root` fires the victim's
   already-armed hooks from any workspace (T3 to fire, T4 to profit; `hook_dispatch.rs` § "The root
   key is an opaque token"). A path key must be safe to print; a wire key must not be printable at
   all. Those are different jobs. Pinned by
   `hook_dispatch::tests::the_payload_key_is_not_the_dispatch_root_token`.
2. **It is not side-effect-free.** `root_token` reads — and on first use **creates**, `0o600` inside
   a `0o700` directory — the machine-local HMAC key. Install destinations are computed by
   *read-only* commands too: `grim status`'s materialization-drift check goes through the very same
   `InstallTarget::path_for` seam. A read-only command minting arming key material undoes WP-R's
   structural guarantee that `status`/`search`/`context` cannot touch the arming path. It would also
   have made `path_for` fallible, which forces either a second infallible spelling of the payload
   location or a fallback — and a fallback that returns a workspace path is exactly the defect.

Guessability costs nothing for the payload path: `$GRIM_HOME` is not writable by a repository, so
knowing a payload path grants an attacker nothing.

**`payload` joins `RESERVED_ARTIFACT_NAMES`** (`bin`, `dispatch.json`, `payload`). A *global*
artifact named `payload` would otherwise materialize over the directory holding every workspace's
payloads. The const's own doc said reserving two names was "cheaper and more additive than nesting
payloads under a `payload/` segment, which would move a shipped layout" — the nesting became
necessary for a security fix, and the layout was never shipped, so the trade it weighed no longer
applies. Its existing test iterates the const, so the third name is covered for free.

## 2. Where convergence derives the payload directory — the actual fix

`hook_registrar::desired_entries` (`src/install/hook_registrar.rs:1024`). It used to call
`output.resolved_target(roots, Strict)` — the install record's **own stored path** — and read
`hook.toml` out of it. It now builds

```rust
AnchoredPath { anchor: PathAnchor::GrimHome, relative: hook_dispatch::payload_relative(root, &record.name) }
    .resolve(roots, Containment::Strict)?
```

where `root: RootScope` is derived **once** in `converge_clients`, above the per-client loop, and is
the same value `converge_root` keys the dispatch table on — so the table key and the payload
location cannot disagree about which root is being armed.

**Relocating the directory without moving this read would have left the hole fully open**, which is
why the unit regression test asserts the *derivation* and not the directory: it plants a complete,
valid `hook.toml` inside the workspace, points a record straight at it, and asserts the desired set
is empty (`a_record_that_names_its_own_payload_directory_arms_nothing`).

What the record is still allowed to say: **which clients armed** (that is what an install wrote) and
the pin behind `resolved_digest`. Neither can redirect a read. The `AnchoredPath` round-trip is not
decoration — `record.name` arrives from a state file and is untrusted, so Layer 1 refuses a `..` in
it without touching the filesystem and Layer 2 refuses a symlinked ancestor escaping `$GRIM_HOME`.

Two supporting changes:

- **`InstallTarget::path_for` gained `roots: &AnchorRoots`** and intercepts `ArtifactKind::Hook`
  before delegating. `$GRIM_HOME` reaches that seam only through the pre-resolved `AnchorRoots`, and
  all five production call sites (3 in `installer.rs`, 2 in `expected_outputs.rs`) already held it.
  `ClientTarget::path_for`'s `Hook` arm is now `unreachable!()`, alongside the existing `Bundle` arm
  — deleted rather than left as a plausible fallback, because a fallback returning a workspace
  payload path *is* SEC-1. Its deadness is pinned by
  `target::tests::a_hook_payload_is_machine_local_at_both_scopes`.
- **`candidate_anchors(Project, _, Hook)` returns `[GrimHome, Workspace]`** — the one project triple
  that does not anchor at the workspace. `Workspace` is retained, second, purely so a
  pre-relocation record still classifies for the reaper.

## 3. The reaper, and `grim status`

No new reaper: `installer::reap_moved_outputs` already handles "the record sits at a path the
current layout no longer produces", and it fires because `output_at_current_layout` now computes the
`$GRIM_HOME` pair and compares it against the record's `Workspace` pair. Executed on a donor armed
by the **pre-fix** binary, then upgraded (§ 5, phase D):

```
status (record still at the OLD path)   → state=gated  pending=[{claude, $GRIM_HOME/hooks/payload/1857a79…/shell-guard}]
grim install                            → updated
status                                  → state=gated  pending=[]
grim install (again)                    → unchanged
```

**`status` reads neither `modified` nor `missing`** at any point — the requirement in the brief. It
reports the move as materialization drift (`outputs_pending`), which is that field's documented
purpose, and the drift clears on the next install. (`state=gated` is WP-R's **F-2**, unrelated and
not mine: `status::hook_arming` derives from config alone and `--allow-hooks` is never persisted.)

Precisely what the reap leaves behind: every **file** under `<ws>/.grimoire/hooks/` is gone; the
now-empty `.grimoire/hooks/` **directory** survives, because grim removes files, not empty
directories (the same convention documented for the Antigravity/Gemini detection leak). Nothing
armable remains. No `--force` is needed, and a user-edited orphan is still preserved-and-warned
rather than deleted, per `docs/src/stability.md`'s kept-modified promise.

## 4. The pre-existing "writes nothing armable" test — what it was actually asserting

`installer.rs::a_project_hook_install_writes_nothing_armable_into_the_workspace` **passed for the
entire life of SEC-1, and could not have caught it.** It asserted three things:

1. the payload **is** at `<ws>/.grimoire/hooks/<name>`;
2. the record anchors at `PathAnchor::Workspace`;
3. `install_one` writes no `.claude/settings.local.json`.

Only (3) is about anything armable, and it scoped "armable" to **the registration alone**, on the
theory stated verbatim in `client_target.rs` — *"nothing here is armable; the registration is"*.
(1) and (2) **pinned the vulnerable layout as the contract**: the test would have failed had anyone
moved the payload out of the workspace.

**That is a finding about the test, not a gap in it.** It is not a test that missed a case; it is a
test written from a premise SEC-1 falsified. Its name promised a property ("nothing armable") that
its body never checked, and the gap between the two was invisible because the premise was written
down as a comment three files away instead of as an assertion. The rewritten test now enumerates
every file under the workspace and allows only grim's own bookkeeping (`state.json`, the
self-managed `.gitignore`) — a whitelist, so the next thing that lands in a repository fails it
rather than slipping past a named-path check.

The acceptance-level sibling (`test_a_project_hook_arms_with_nothing_armable_in_the_workspace`) does
the same for a real armed install through the real binary, permitting only
`.claude/settings.local.json` — the one repo-resident registration I1 admits.

## 5. Executed evidence

Real binaries, real registry (`localhost:5000`), real client config. Script:
`<scratchpad>/sec1lab/repro.sh`, reconstructed from `.agents/wp-r-report.md` § SEC-1 and WP-R's own
`armlab/clone.sh`. Two binaries built from this worktree: `grim-prefix` (base `31a9154`,
`git stash push -- src/`) and `grim-fixed`.

**A — pre-fix binary: SEC-1 reproduced.** Donor armed; the repo carries the payload:

```
========== A2  what the hostile repo carries (pre-fix: the payload is IN the repo) ==========
  .grimoire/state.json
  .grimoire/hooks/shell-guard/hook.toml
  .grimoire/hooks/shell-guard/guard.sh
  .grimoire/.gitignore

========== A3  the clone, on a fresh machine with a global trust grant, OFFLINE ==========
hook  shell-guard  …/a/clone/.grimoire/hooks/shell-guard  unchanged
-- victim dispatch table:
1 dispatch row(s)
   shell-guard guard client=claude payload_dir=…/a/clone/.grimoire/hooks/shell-guard
-- clone registration:
  WRITTEN (armed)
```

**B — fixed binary, the SAME clone, a fresh victim home, still offline: does not arm.**

```
========== B1  fixed binary, the SAME clone, a fresh victim home, OFFLINE ==========
 WARN grim::install::hook_registrar: hook desired-set projection failed; registration skipped
      client="claude" error=No such file or directory (os error 2)
localhost:5000/acme/wpt/shell-guard@sha256:78cf59fa…: offline mode blocked a required network operation
-- victim dispatch table:
NO dispatch table (nothing armed)
-- clone registration:
  none
```

**C — fixed binary, and the attacker hand-authors the pre-fix layout.** The fixed binary's own armed
donor leaves nothing armable in the repository; the attacker then re-plants the payload inside the
repo *and rewrites the record to name it* — and it still does not arm:

```
========== C1  fixed binary: arm a donor, and show the workspace holds nothing armable ==========
-- files under the workspace:
  .claude/settings.local.json
  .grimoire/.gitignore
  .grimoire/state.json
  grimoire.lock
  grimoire.toml
-- the payload, machine-local:
  $GRIM_HOME/hooks/payload/a06b2aa7f777e614…/shell-guard/guard.sh
  $GRIM_HOME/hooks/payload/a06b2aa7f777e614…/shell-guard/hook.toml

========== C2  the attacker re-plants the payload in the repo and rewrites the record ==========
  rewrote the record to: {'anchor': 'workspace', 'relative': '.grimoire/hooks/shell-guard'}
  .grimoire/hooks/shell-guard/guard.sh
  .grimoire/hooks/shell-guard/hook.toml
  .grimoire/state.json

========== C3  … a fresh victim home with a global grant, OFFLINE ==========
-- victim dispatch table:
NO dispatch table (nothing armed)
-- clone registration:
  none
-- victim launcher:
  none
```

**D — the upgrade path** (executed output in § 3).

**What the fix does *not* change, and should not:** on an **online** victim, the clone's committed
`grimoire.toml`/`grimoire.lock` naming a registry the victim's global config trusts for hooks will
cause grim to fetch that artifact at that pinned digest and arm it. That is the consented flow, not
SEC-1: the attacker's leverage is "I declared a dependency from a publisher you trust", which is
true of every artifact kind. What SEC-1 was about is that **no fetch, no trust re-check against
bytes grim obtained itself, and no install history** were needed.

The xfail is gone and the test is now **parametrized over both attacker shapes**
(`as-grim-wrote-it`, `payload-replanted`), because the verbatim-commit variant alone would pass
against a build that merely moved the directory without moving the read.

## 6. Findings

### F-1 (Warn, not fixed — routed) `subsystem-file-structure.md`: the docs package left me a hole in a file version I do not have

My base (`31a9154`) has **no** hook content in `.claude/rules/subsystem-file-structure.md` at all.
The main worktree's copy — written by the concurrent docs package — has a `### Hooks
{#install-layout-hooks}` section containing:

> **The payload directory's location is being relocated right now** (the SEC-1 fix moving hook
> payloads out of the workspace and under `$GRIM_HOME`). It is deliberately **not documented here**
> — a path recorded mid-move is worse than an absent one. Add it when that lands.

So the file the brief assigned me "documents `<scope>/hooks/`" only in a revision that does not
exist on my branch. Writing my own `### Hooks` section into my copy would collide with theirs at the
same insertion point — a guaranteed conflict on 400 lines of prose, for text they explicitly
reserved. **I did not touch the file.** Ready-to-paste replacement for their placeholder:

> A hook payload is **machine-local at both scopes** (invariant I1): `$GRIM_HOME/hooks/<name>/`
> globally, `$GRIM_HOME/hooks/payload/<workspace-key>/<name>/` for a workspace, where
> `<workspace-key>` is the SHA-256 of the workspace path (hex) so two workspaces under one
> `$GRIM_HOME` cannot collide. It is deliberately **not** the dispatch `root_token`: a recorded
> install target is printed by `grim status`, and a disclosed root token lets a hostile repository's
> own registration fire the victim's armed hooks (B3). `payload` is therefore a third
> `RESERVED_ARTIFACT_NAMES` entry. Convergence derives this directory from `$GRIM_HOME` and the
> resolved scope, **never** from the install record — that derivation, not the relocation, is what
> closes SEC-1.

…and two rows for the **anchor root/remainder table** (which has no hook rows in either revision):

| Scope · client · kind | Anchor | Stored `relative` |
|---|---|---|
| project · any · hook | `grim-home` | `hooks/payload/<sha256-of-workspace>/<name>` — **the one project triple that does not anchor at `workspace`** (I1). `workspace` stays in the candidate set, second, so pre-relocation records still classify for the layout reaper |
| global · any · hook | `grim-home` | `hooks/<name>` (client-independent — S-003) |

The `project · any · any → workspace` row above them needs an "except `hook`" qualifier.

### F-2 (Suggest, not fixed — not my file) `src/command/install.rs`'s dev-install refusal reason 2 is now inaccurate

`install.rs:227` refuses a path-sourced hook partly because *"it would put an armable payload inside
a repository"*. After this change a dev-installed payload would be **copied out** of the repository
into `$GRIM_HOME`, so that reason no longer holds as stated (the *source* stays in the repo, and the
edit-then-reinstall loop is still repo-resident, but the installed payload is not). The refusal is
unaffected — its own doc says "any one sufficient", and reason 1 (consent is unexpressible for a
path source, I4) stands untouched. Left alone because `src/command/install.rs` is outside my
declared set and a concurrent package has been editing it; it is a comment-only correction.

### F-3 (Suggest) the projection-failure warning is accurate and unactionable

Both B and C above print `hook desired-set projection failed; registration skipped
error=No such file or directory (os error 2)`. It is the correct degrade (I3 — a per-client warn,
never a failed command) but it names an errno rather than the situation. A message naming the
artifact and the missing payload directory would be followable; today the user has to know that
"projection" means "grim looked for the payload under `$GRIM_HOME` and did not find it". The
message lives in `converge_clients` and is one line, but it is WP-R's text and changing it would
have been an undeclared edit to a merged contract string.

### F-4 (Suggest) `$GRIM_HOME/hooks/` may be created at umask, not `0o700`

`ensure_hooks_dir` narrows `$GRIM_HOME/hooks` to `0o700` (W3) **only when it creates it**, and
`install_one`'s `create_dir_all(dest.parent())` now reaches that directory first on a machine whose
first hook operation is an install. This is **pre-existing** — the global-scope payload
(`$GRIM_HOME/hooks/<name>`) already had the same ordering — and it is not a regression: the
`root-key` file is created `0o600` via `O_EXCL` regardless of its parent's mode, and the table is
explicitly narrowed to `0o600` after every write. Recorded rather than fixed because W3's cause 5 is
already deferred and this widens no new surface.

### F-5 (Warn, fixed here) `a_refusal_writes_nothing_and_reports_every_client` was asserting the wrong thing

It asserted `!nested.join("hooks").exists()` — i.e. that a C-017 refusal leaves *no `hooks/`
directory at all*. That was only true because the fixture wrote its payload into the workspace; once
the payload is machine-local, the directory legitimately exists before convergence runs (a real
install materializes it first, and a refusal must not delete it). Retargeted at the three files that
actually arm: the launcher, the dispatch table, and the machine key. Strictly stronger — the old
assertion would have passed on a build that wrote a launcher into a *different* directory.

## 7. Files touched

**In my declared set:** `src/install/{hook_dispatch,hook_registrar,path_anchor,expected_outputs,installer,target,client_target}.rs`,
`src/oci/hook.rs` (`RESERVED_ARTIFACT_NAMES` + rule 8's doc), `.claude/rules/arch-threat-model.md`
(I1 now names the payload, with the omission-is-not-a-licence note),
`.agents/plans/plan_hooks_artifact_kind.md` (S-003 + WP-J1's Specify bullet),
`test/tests/test_hook_arming.py`.

**`src/install/prune.rs`: not touched** — it keys on resolved recorded outputs, so the shared-payload
refcount (`c020_shared_hook_payload_survives_until_the_last_client_drops_it`) is layout-agnostic and
passes unchanged.

**Overrun, declared:** `src/tui/app.rs` — **one line**, a test call site of
`InstallTarget::path_for` gaining the `roots` argument (`&roots` was already in scope). Mechanical;
`src/tui/app.rs` is not in my set and no other WP claims it, so it is declared rather than silent.

**`.claude/rules/subsystem-file-structure.md`: deliberately not touched** — see F-1.

`test/uv.lock` and `.claude/tests/uv.lock` reverted; nothing staged with `git add -A`.

## 8. New tests

| Test | Pins |
|---|---|
| `hook_registrar::a_record_that_names_its_own_payload_directory_arms_nothing` | ⛔ SEC-1 at unit level: a valid planted manifest the record points at arms nothing; positive control in the same function |
| `hook_dispatch::a_payload_is_under_grim_home_at_both_scopes_and_keyed_per_workspace` | the layout, per-workspace uniqueness, `Normal`-only remainder, stability across calls |
| `hook_dispatch::the_payload_key_is_not_the_dispatch_root_token` | B3: the path never carries the token, and deriving a path mints no key material and writes nothing |
| `target::a_hook_payload_is_machine_local_at_both_scopes` | both scopes under `$GRIM_HOME`, never the workspace; client-independent; two workspaces → two directories; also proves `ClientTarget::path_for`'s `unreachable!()` is dead |
| `installer::a_project_hook_install_writes_nothing_armable_into_the_workspace` | rewritten — whitelist of every file allowed under the workspace (see § 4) |
| `installer::an_old_layout_project_hook_re_materializes_and_reaps_the_workspace_payload` | Principle 9's layout-move duty: falls through the gate, re-materializes, reaps the old path, third pass is `AlreadyInstalled` |
| `test_hook_arming.py::test_a_cloned_workspaces_own_committed_hook_state_must_not_arm[as-grim-wrote-it\|payload-replanted]` | WP-R's xfail, now a passing parametrized test over both attacker shapes |
| `test_hook_arming.py::test_a_project_hook_arms_with_nothing_armable_in_the_workspace` | the layout formula through the real binary + the workspace whitelist + the dispatch row's `payload_dir` |
