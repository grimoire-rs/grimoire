# Design: porting PR #98 (hooks artifact kind) onto current main

## Metadata

- **Status:** Draft — architect output of `/hex-plan xhigh`, input to the plan.
- **Date:** 2026-09-28
- **Branch:** `hex/hooks-pr-98-revival` (base `origin/main` @ `3194d0dd`).
  Supersedes PR [grimoire-rs/grimoire#98](https://github.com/grimoire-rs/grimoire/pull/98)
  as the vehicle. The PR diff is squashed once onto this branch.
- **Binding inputs:** `.agents/discussions/hooks-pr-98-revival.md` (ratified),
  `.agents/goals/hooks-pr-98-revival.md`, `research_hooks_pr98_drift.md`,
  `research_hooks_upstream_2026-09.md`, `research_hooks_qoder_copilot_schemas.md`.
  On the PR branch: `adr_hooks_support.md`, `adr_hook_workspace_consent.md`,
  `plan_hooks_artifact_kind.md`.

### Reversibility

| Decision | Door | Why |
|---|---|---|
| Installer composition (§ 5) | Two-way | Internal seams; the binary is the only consumer. |
| Qoder and Copilot projection rows | Two-way | Data rows in one table (C-021); hooks are experimental. |
| Experimental surfaces: `grim hook` verbs and their report JSON, `grim schema --kind hook`, the `options.experimental` table, `hook.toml`, `[hooks]` in `grimoire.toml`, the lock `hooks` table | Two-way, **provided** WP-05's stability bullet carves them out | `stability.md` › #unstable lets an explicitly experimental surface move ("may change or be removed in any minor while experimental"), as the NDJSON bullet already does. WP-05 authors that bullet (C-170) and must name every item in this row; without the carve-out they freeze on first release. |
| Additions inside already-frozen reports: `kind: "hook"`, `arming`, `gated`, `not-armed` in `status` JSON; `armed` on install/add/update rows; `omitted[].kind: "hook"` in the export report | **One-way** | Enum literals and field names in a frozen report cannot be removed afterwards. `arming: []` and `armed: null` on hook-free rows cannot be carved out (`subsystem-cli-api.md` bans `skip_serializing_if`). |
| OCI wire format: the hook artifact kind, its config media type, the `com.grimoire.*` hook annotations | **One-way** | Frozen row "OCI wire format: Artifact kinds" in `stability.md`. `grim build --kind hook` and `grim release` are not flag-gated, so a published hook artifact lives in registries for good. |
| Export decline of hook members | Two-way | Additive once a grim-free runtime exists (the deferred follow-up). |

### Numbering

The ADR owns C-001…C-014. The PR plan owns C-015…C-026 and S-001…S-016.
New contracts in this spec are numbered **C-101 and up**, and new scenarios
**S-101 and up**, so the Specify gate's join keys never collide.

## 1. Scope and relationship to the PR's design record

**In scope.** Everything the ratified discussion lists: port onto main's
seams, Qoder hooks, Copilot mutator bookkeeping and the conditional
SessionStart context, export decline, Starlight docs, Principle 9 proof, and
the `.grimignore` interaction.

**Out of scope, per the frozen decisions:** a grim-free hook runtime, hooks in
exported plugins, Qoder at project scope, the Copilot cloud agent (D7),
graduating hooks out of experimental.

### Contracts that stay binding unchanged

Tests generated from these on the PR branch carry over verbatim. This spec
does not restate them.

| Source | IDs |
|---|---|
| ADR `adr_hooks_support.md`, as amended in place by the plan | C-001 manifest, C-002 envelope, C-003 response, C-005 `Vendor` additions, C-006 dispatch table (WP-P0 amendment), C-007 `grim hook run` (WP-P0 amendment), C-008 launcher (both amendments), C-010 deltas, C-011 mutator pipeline, C-012 audit, C-014 `grim schema --kind hook` |
| Plan `plan_hooks_artifact_kind.md` | C-002 ownership note, C-009 lock-time integrity, C-015 golden byte-identity, C-016 exhaustiveness, C-017 visible convergence failure, C-018/C-018b matcher allowlist and no interpolation, C-019 exec bit, C-020 payload refcount, C-023 non-interactive never blocks (workspace amendment), C-025 matcher dialects |
| Consent ADR `adr_hook_workspace_consent.md` | The whole decision, including the arming composition order (feature flag → flag pair → transport → global → consent) |

**Withdrawn or superseded, do not implement:** C-022 (replaced by the consent
ADR), C-024, C-026.

### Contracts this spec extends as data, not as text

- **C-004 / C-021** (projection table): gains Qoder rows (C-120) and possibly
  a Copilot SessionStart context field (C-131). The table stays single-instance.
- **C-013** (`clients.md` hook column): gains the Qoder cell, derived by
  `hook_matrix_cell` as before.
- **C-015** (golden proof): extended by C-150 (CI wiring) and C-151
  (differential run against main).

### ADR corrections (text-only amendments, one changelog row each)

Gemini `trusted_hooks.json` → `trustedFolders.json` (ten sites, listed in the
vendor discovery). OpenCode: plugin-API-only, no declarative hook file. Copilot:
`modifiedArgs` documented (C-132), `SessionStart` context (C-131), cloud agent
supports hooks upstream but stays excluded (D7, recorded as a deliberate choice).
New amendments: **A7** export decline (C-140…C-142), **A8** Qoder surface
(C-120…C-124).

## 2. Port design — the redesign seams

Main's `92699721` extracted seams the PR never saw. Discovery found that the
PR's production edits to these seams are small; the hard part is placing them.
Each contract names the seam and states a testable behaviour.

### `src/install/installer.rs`

**C-101 — hook gates in `install_one`, fixed order.** For `kind == Hook`,
before `integrity_gate`, in this order:

1. `oci::hook::binding_name_refusal(&artifact.name)` is `Some` →
   `Ok(InstallOutcome::Skipped(reason))`, one `warn!`, **no record written**,
   no fetch.
2. `target.hook_policy()` is `None` → `Skipped`, one `warn!` naming
   `grim install`, **existing record left untouched**, no fetch. *New relative
   to the PR:* the PR let a `None` policy fall through and materialize a
   payload. `None` must mean "this invocation neither arms, reaps, nor
   materializes hooks".
3. `policy.refusal_reason(&source)` is `Some` → `Skipped`. A zero-output
   record is written only when none exists (PR S-001 semantics).
4. Everything after is main's unchanged path: `integrity_gate` →
   `effective_supporting_clients` → `stage_locked_artifact` → per-client
   materialize.

Once C-113 attaches a policy at every mutating seam, step 2 is reachable only
through a seam that forgot to. The mutating entry points
(`install_and_persist`, `grim update`'s install loop) therefore carry
`debug_assert!(target.hook_policy().is_some())` whenever the locked set holds
a `Hook`, so tests catch the next forked seam; release builds keep the step-2
skip as the belt. The assert sits in the seams, not in `install_one`, so
step 2 stays unit-testable.

*Test:* one unit test per step with a counting `OciAccess` asserting zero
`fetch_blob` calls on steps 1–3, plus the state delta. One
`#[should_panic]` test drives `install_and_persist` with a hook and no policy.

**C-102 — staging a hook.** `locate_canonical` treats `Hook` like `Skill`, both
on the exact path and on the rebinding fallback. `stage_locked_artifact`
returns `support_dir: None` for `Hook`. *Test:* the PR's
`locate_canonical_finds_the_hook_payload_directory` ports verbatim.
`stage_locked_artifact` on a hook tar yields `canonical == <root>/<name>/`.

**C-103 — `client_supports_kind` and permanence.** Adds the arm
`Hook => !vendor.declines_hooks_everywhere() && vendor.kind_surface(Hook, scope)`.
The skip-log split moves into
`kind_is_permanently_declined(client, kind) -> bool`, whose `Hook` arm reads
`declines_hooks_everywhere()`. `name_fits` needs no change (it checks agents
only). `client_hosts` therefore inherits the hook answer, and so do
`expected_clients`, `pending_outputs` and `status` drift, without edits.
*Test:* the PR's `client_supports_kind_reads_hook_from_the_hook_surface`,
widened to the 19-client `ClientTarget::ALL` on main. Qoder's expectation has
two sanctioned values: **WP-01** asserts Qoder `false` at both scopes (the PR
state, before C-120 exists). **WP-02** flips global to `true` only if C-121
passes; if C-121 declines Qoder, the expectation stays `false` at both scopes.
Project is `false` on either branch.

**C-104 — one convergence entry point.** New function
`hook_registrar::converge_for(policy: Option<&HookPolicy>, state: &InstallState, workspace: Option<&Path>, scope: Scope, roots: &AnchorRoots) -> Vec<(ClientTarget, HookSync)>`.
It returns `vec![]` when `policy` is `None`. Otherwise it calls
`converge_clients(state, workspace, scope, roots, policy)`. The signature
takes the policy, not an `InstallTarget`, because four of the six sites
(uninstall and the three TUI deletes) hold only `scope`/`ctx` plus
`involved_clients`, and must not build a target just to carry a policy.
It is called **immediately after every `sync_config` loop** on main:

| Call site on main | Policy attached by |
|---|---|
| `installer::install_and_persist` (after the loop at :470) | caller: `add`, `install` → `hook_consent::resolve[_for_add]` (may prompt); TUI installs per C-113 |
| `command/update.rs` (after the loop at :276) | `hook_consent::resolve` (may prompt) |
| `command/uninstall.rs` (after the loop at :142) | `hook_consent::resolve_without_consent` (never prompts) |
| `tui/app.rs` :2695, :2772, :3774 (delete paths) | `resolve_without_consent(.., None, &BTreeSet::new())` (never prompts) — *new relative to the PR, see C-113* |

`grim remove` is **not** a converging seam: `src/command/remove.rs` edits
config and lock only, loads no install state and reaches neither
`uninstall` nor a sync loop (kept so by the PR, `remove.rs:67-70`). A removed
hook therefore stays armed until the next `grim install` or `grim uninstall`
(deferred disarm, C-155). `grim hook revoke` and `grim config set
options.experimental.hooks false` share that deferral; each already warns
(PR `hook/revoke.rs:8-12`, `config.rs:704-722`).

*Tests:*

- A source-level test in the A-10 pattern: for every file under `src/`
  except `vendor*.rs`, `hook_registrar.rs` (defines `converge_for`) and the
  test's own file, the number of `.sync_config(` **call expressions** equals
  the number of `converge_for(` call expressions (occurrences preceded by
  `fn ` are not calls), counted with doc comments (`///`, `//!`) and line
  comments stripped. This catches the next fork of the loop without parsing
  function boundaries.
- Ordering is pinned behaviourally, one reaping test per non-install seam:
  `grim uninstall` of an armed hook, and each of the three TUI delete sites,
  leaves no grim-owned element in claude `settings.json` and no dispatch row
  for the hook. C-113's test is one of these four.

**C-105 — the hook-free short circuit is write-free.** `converge_clients`
returns without touching the filesystem when all three hold: no hook record
in state, no grim-owned element in any hook surface, and no
`$GRIM_HOME/hooks/dispatch.json`. That covers no launcher, no dispatch
table, no token key, and no empty hook file. *Test:* snapshot every file
under a fake `HOME`, the workspace and `GRIM_HOME` as (path, mtime, bytes).
Run `converge_for` with an arming policy over a hook-free state and assert
the snapshot is unchanged. Repeat with every hook client detected.

### `src/install/target.rs` and `src/install/expected_outputs.rs`

**C-106 — `InstallTarget` carries `grim_home`; `path_for` keeps main's
signature.** `InstallTarget` gains a `grim_home: PathBuf` field, set at
construction from the same `GRIM_HOME` resolution `AnchorRoots` uses.
`InstallTarget::path_for(client, kind, name)` keeps main's three-argument
form and intercepts `Hook` →
`hook_dispatch::payload_dir(&self.grim_home, root_scope_for(workspace, scope), name)`.
Every other kind returns exactly what main's form returns. This replaces the
PR's four-argument `path_for(.., roots)`: none of the ~15 hook-free call
sites (`installer`, `expected_outputs`, `path_anchor`, `client_target`,
`tui/app.rs`) changes, which shrinks the Principle 9 surface and the merge
conflicts. *Tests:* (a) a table test over every `ClientTarget::ALL` ×
{Skill, Rule, Agent, Mcp} × {Project, Global} asserting the output equals a
frozen copy of main's function body kept in the test module; (b) the PR's
`install_target_path_for_never_delegates_a_hook`; (c) `target.grim_home ==
roots.grim_home` for a target built under an overridden `GRIM_HOME`.

**C-107 — `expected_outputs` needs no hook-specific code.** Main's
`expected_clients(kind, name, target)` and `pending_outputs` reach hooks
through C-103 and C-106. The PR's `output_at_current_layout` edit is dropped:
main's `pending_for_client` / `output_at_current_layout` already call
`target.path_for(..)`, which C-106 makes hook-aware. *Tests:* the PR's
`only_hook_capable_clients_are_expected_hook_targets` and
`the_own_file_hook_clients_are_expected_at_global_scope_only`, re-signatured to
main's `expected_clients(kind, name, target)`. Qoder follows C-103's two
outcomes: **WP-01** asserts Qoder absent from the global-only set (PR state);
**WP-02** adds it to the global-only set only if C-121 passes, else it stays
absent. WP-02 owns these test modules for that flip.

### `src/install/client_target.rs`

**C-108 — `materialize` via `MaterializeRequest`.** Adds the arm
`ArtifactKind::Hook => Self::materialize_hook(req.artifact_root, req.dest)`.
`materialize_hook` keeps the PR body (`copy_tree`, sorted). It ignores
`scope`, `pinned` and `support_dir`, and never renders. `ClientTarget::path_for`
gets `Hook => unreachable!` **only because** C-106 intercepts first; the PR's
guard test pins that. *Test:* materializing the same hook for claude and
codex into one payload dir produces identical `MaterializedFile` lists, and
the installer dedups to one directory with one output per client (the PR's
`a_hook_materializes_one_shared_payload_dir_with_one_output_per_client`).

### `src/command/add.rs` — the shared declare seam

**C-109 — `declare_reference` and hooks.** The seam is shared with export, so
it only learns what is true for both callers:

- `declare_registry` infers `Hook` from the manifest with **no gate** (the
  PR's reasoning at the removed A-3 gate stands: the flag controls arming,
  not declaring). `DeclareError` gains **no** variant.
- `invalid_binding(kind, binding)` extends the `SkillName` charset guard to
  `Hook`. The binding becomes a directory under `$GRIM_HOME/hooks/`, which is
  a traversal surface. The error is `InvalidBindingName { kind: Hook, .. }` →
  `CommandError::InvalidBindingName` → exit **64**, as for the other kinds.
- `declare_path`: `--kind hook` → the existing
  `UnsupportedPathKind(Hook)`. A directory holding `hook.toml` without
  `--kind` → the existing `UninferablePathKind`. Both keep today's mapping.
  Hooks have no path-source form.
- **Reserved names** (`bin`, `payload`, `consent` and the rest of
  `RESERVED_ARTIFACT_NAMES`) are refused in `grim add` **after**
  `declare_reference` returns, as a scope-bound check like the existing
  dev-install collision. They are not refused inside the shared seam, so
  `grim export` never errors on a hook it will omit anyway. Exit **64**
  through `InvalidBindingName`. C-101 step 1 remains the belt for a
  hand-edited `grimoire.toml`.

*Tests:* the `declare_reference_spec` module gains a hook case per bullet.
`mapped(..)` asserts exit 64 for the charset and reserved cases.

**C-110 — `install_added` attaches the policy.** The PR's
`install_added(args: &AddArgs)` body ports as-is:
`resolve_for_add` → `target.with_hook_policy(..)`. `write_config` emits
`[options.experimental]` only when it is non-default, and emits `[hooks]`
only when non-empty.

### `src/command/update.rs`

**C-111 — `refresh_dev_installs` never packs a hook.** The `rec.kind` filter
moves **before** `pack_local_artifact_blocking`:
`Mcp | Bundle | Hook => continue`. `skill::local_pack::pack_local_artifact`'s
`Hook` arm returns `Err(SkillError{UnsupportedKind})` instead of the PR's
`unreachable!()`. A tampered `state.json` carrying `dev = true` on a hook
record must not panic (`quality-rust.md`: no panics on external input).
*Test:* a state with a dev hook record plus `grim update` → exit 0, one
`warn!`, no panic.

**C-112 — `grim update` converges after its own loop** (C-104 row). The
policy is resolved against the **new** lock, after `roll_forward`.
`roll_forward` itself stays hook-agnostic, apart from resolving the `hooks`
table like any kind (C-116).

### `src/tui/app.rs`

**C-113 — the TUI is a mutating seam, so it never holds a `None` policy.**
Every TUI action that installs, updates or deletes resolves its policy with
`resolve_without_consent`, which never prompts because the TUI owns the
terminal. The `declared` set it is handed is split **by direction**, because
`consent::evaluate` grants whenever `declared ⊆ record.hooks`:

- **Delete paths** (`tui/app.rs` :2695, :2772, :3774) pass
  `resolve_without_consent(.., None, &BTreeSet::new())`, the `grim uninstall`
  precedent. The empty set only answers *whether a record exists*; which hooks
  arm is decided per record — a hook arms on workspace consent only if its
  `consent_key` is in the loaded record (`HookPolicy::verdict`). *Corrected
  2026-09-28:* this line used to say "an empty set can only shrink the armed
  set", which was false — `Granted` was applied to every hook in install state,
  so a hook only `--trust-hooks` had armed re-armed on the next delete.
- **Install/update paths** (the `install_and_persist` sites at `tui/app.rs`
  :2920, :3043, :3140) pass
  `resolve_without_consent(.., None, &hook_consent::declared_hooks(&new_lock))`.
  A bundle install or roll-forward that adds a hook member is then
  `Drifted(new)`: drift gates (`gated` / `workspace-not-consented`, naming
  `grim hook allow`) and never grants. Passing an empty set here would
  evaluate `Granted` in any consented workspace and arm a hook the user never
  consented to.
- The PR's **WP-H refusal of direct hook-row installs in the TUI stays**. A
  hook reaches the TUI's install path only as a bundle member, and then
  gates as above.
- **Unreadable config on a TUI delete** (`load_scope_declaration` fails, the
  path that returns `Ok(())` at ≈:2762): the delete converges with a
  **flag-off policy** (the same `resolve_without_consent` builder with the
  feature flag forced `false`), which reaps every grim hook registration for that
  root. That is the safe default: nothing can be armed without a readable
  flag.

The PR left the TUI at `None`, so a TUI delete of a hook left its
registration and dispatch entry pointing at a deleted payload until the next
CLI install. *Tests:*

- Delete a hook row through each of the three TUI delete sites, then assert
  no grim-owned element remains in claude `settings.json` and the dispatch
  entry is gone (C-104's behavioural ordering tests).
- In a **consented** workspace, a TUI bundle install that adds a new hook
  member leaves no grim element in claude `settings.json`, and the hook row
  reports `gated` with cause `workspace-not-consented` (S-119).
- A TUI delete with an unparseable `grimoire.toml` reaps the root's grim
  hook registrations.

### `src/config/project_config.rs`

**C-114 — `parse_artifact_map(raw, path, values, kind)`.** The PR's `kind`
parameter is merged over main's `id.or_latest()` body. `[hooks]` parses with
`PathValues::Rejected`. The refusal message is
`format!("path sources are not supported for {kind} artifacts")`. *Test:*
for `mcp` that string is **byte-identical** to main's literal (Principle 9:
error text is unstable, but there is no reason to move it). `BundleSource`
gains `hooks`, which is additive.

### `src/lock/grimoire_lock.rs`, `src/install/prune.rs`

**C-115 — one artifact chain.** `GrimoireLock::iter_artifacts` chains
`hooks` last. `prune_orphans` replaces its hand-maintained
`skills.chain(rules)…` with `lock.iter_artifacts()`. The PR found this trap
firing for the third time (update deleted every installed hook). *Test:*
`iter_artifacts_chains_all_kinds_in_order` gains `Hook`. A prune over a lock
whose only artifact is a hook deletes nothing.

**C-116 — lock and declaration serialization.** `hooks` is serialized only
when non-empty, both in the lock and in the JCS declaration input, sitting
between `bundles` and `mcp` (C-015). `DECLARATION_HASH_VERSION` stays `1`.
This is C-015 itself, restated here only to pin that main's new
`serialize_artifact_views<S>()` helper carries the skip.

### `src/install/json_splice.rs`

**C-117 — one escaping helper.** Main's `json_string()` is kept at every
key-interpolation site. The PR's `json_key()` wrapper is deleted and its
nested-group primitive calls `json_string()`. *Test:* both branches' escaping
regression tests are kept and pass. `grep -c 'fn json_key' src/` is 0.

### `src/command/scope_resolution.rs`

**C-118 — relative `--config` resolves to an absolute workspace.** PR commit
`2a436191` is carried (main line 53 still has the bug). `--config
grimoire.toml` and `--config ./grimoire.toml` both give `cwd.join(..)`, with
no canonicalization. *Test:* the PR's regression test. The consent key and
payload dir differ for two repositories invoked the same way. C-151's
differential run includes a relative-`--config` invocation, so the change is
proven not to move any hook-free byte.

## 3. Feature contracts

### Qoder hook surface (ADR amendment A8)

**C-120 — surface.** `QoderVendor` overrides:

- `hook_surface() → Some(HookSurface::SpliceConfig)`.
- `hook_config_path(workspace, Global) → qoder_root(QODER_CONFIG_DIR, home)/settings.json`,
  which is the same file main's MCP writer resolves. `Project → None` (A1).
  Upstream confirms the relocation: `QODER_CONFIG_DIR` replaces `~/.qoder`
  wholesale and moves the user `settings.json`, which holds `hooks`
  (<https://docs.qoder.com/cli/settings>, fetched 2026-09-28). The watchlist
  row cites that page rather than the `AGENTS.md` table.
- `hook_splice_shape() → {container: "hooks", group_key: "matcher", elements_key: "hooks"}`.
- `hook_spliced_handler() →` the Claude element
  `{type: "command", command, timeout?, "com.grimoire.managed": true}`.
  Timeout is in seconds, matching Qoder's default of 30.

**C-121 — verification gate (blocks C-120 from merging).** The Qoder **CLI**
hooks page, <https://docs.qoder.com/cli/hooks-reference>, is fetched raw,
dated, and recorded in `research_hooks_qoder_copilot_schemas.md`. The IDE
page (`docs.qoder.com/qoder/hooks`) is not a substitute: grim's Qoder mapping
is verified against CLI 1.1.63. The gate must confirm three things:

- (a) the `hooks → event → [{matcher, hooks:[{type, command, timeout}]}]`
  registration shape. **Satisfied by the docs** (fetched 2026-09-28: Claude
  shape, plus documented `name`, `timeout`, `if`, `async` handler fields);
  the raw fetch is still recorded.
- (b) that an **unknown key inside a handler element is tolerated**, since
  Codex silently drops the whole file on one bad key. The CLI page is
  **silent** on unknown keys, so (b) needs either a live `qodercli` probe
  (a handler carrying `com.grimoire.managed`, invoked, exit code and output
  recorded with the CLI version) or an explicit upstream statement. No
  `qodercli` is installed locally; installing one for the probe is in scope.
- (c) the per-event response fields for C-122.

If (a) or (b) cannot be confirmed, Qoder ships **declined** (D-4):
`hook_surface() → None`, the `clients.md` cell is ✗, and a watchlist row
records the missing evidence. A marker that could wipe a user's own Qoder
hooks does not ship. The decision record is the goal's DoD evidence on the
decline branch.

**C-122 — projection rows, admitted per cell.** There are four `ProjectionRow`s
for `client: "qoder"`. A cell is populated only when C-121's CLI evidence
documents that exact field. Otherwise it is `None` or `&[]`, which the table
already renders as a decline.

| Event | verdict | reason | context | mutation | Default if undocumented on the CLI page |
|---|---|---|---|---|---|
| PreToolUse | `hookSpecificOutput.permissionDecision` (`allow`/`deny`/`ask`) | `hookSpecificOutput.permissionDecisionReason` | `hookSpecificOutput.additionalContext` | **`None`** | gatekeeper declined |
| PostToolUse | `&[]` | — | `hookSpecificOutput.additionalContext` | `None` | observer only |
| SessionStart | `&[]` | — | `hookSpecificOutput.additionalContext` | `None` | observer only |
| Stop | `&[]` | — | `None` | `None` | observer only |

The **mutator is declined for Qoder in this PR** whatever the docs say. A
mutator whose rewrite field is silently ignored runs the *unmodified*
command while grim reports it armed. That is the silent-guardrail class
C-025 exists to prevent, and it gets enabled only on live evidence, the
standard WP-B set for Copilot. When CLI and IDE shapes diverge for an event,
the CLI shape wins. When the CLI shape is unknown, that event's field is
declined (the table's `None`), and the event is never approximated from the
IDE page. *Test:* the C-021 agreement test covers the new rows
automatically. `hook_tier_support(Mutator, PreToolUse)` for qoder is
`Declined`.

**C-123 — coexistence with MCP in one file.** Splicing hooks into Qoder's
`settings.json` leaves every recorded Qoder MCP entry output's
`current_hash` unchanged, along with every byte outside the `hooks`
container. *Test:* seed `settings.json` with a user hook, a user key and a
grim MCP entry, then arm, re-arm and uninstall. After each step the MCP
record reads `installed`, and the user bytes are unchanged at every step
except the grim element.

**C-124 — self-heal.** A second `converge_for` with an unchanged state
writes zero bytes to every hook surface: Qoder, Claude, Codex and Copilot
(Principle 9, renderer self-heal). *Test:* mtime and bytes unchanged.

### Copilot

**C-130 — the copilot mutator field is chosen by a live gate.** The PR's
`hookSpecificOutput.updatedInput` row was live-verified once in the
PascalCase dialect grim registers, but
[github/copilot-cli#2013](https://github.com/github/copilot-cli/issues/2013)
contests that the CLI applies it. WP-02 therefore re-probes with the local
`copilot` CLI before the row ships:

1. Register a PascalCase `PreToolUse` `Bash` hook in
   `~/.copilot/hooks/grim.json` that returns
   `hookSpecificOutput.updatedInput` rewriting the command to write a nonce
   file. The rewrite **applies** if the nonce file appears and the original
   command's effect does not.
2. If it does not apply, probe top-level `modifiedArgs` in the **same**
   PascalCase dialect, same criterion.
3. The field that applies becomes the row's `mutation`. If neither applies,
   `hook_tier_support(Mutator, PreToolUse)` for copilot is `Declined` and
   the row's `mutation` is `None`.

The transcript, the applied field and `copilot --version` are recorded in
the research file, and the watchlist row cites them. *Test:* the row
assertion pins whichever outcome the probe recorded.

**C-131 — SessionStart context is conditional on a live probe.** The probe
registers a PascalCase `SessionStart` hook in `~/.copilot/hooks/grim.json`.
It returns a nonce once as
`{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":N}}`
and once as flat `{"additionalContext":N}`, then asks the session to repeat
the nonce. If exactly one form surfaces, the copilot `SessionStart` row's
`context` becomes that field. If none does, the row stays `None`, as the
PR shipped it. If both do, use the `hookSpecificOutput` form, matching
Claude. The probe transcript, dated with the CLI version, goes into the
research file, and the watchlist row cites it. No change without the probe.

**C-132 — `modifiedArgs` is emitted only if C-130 selects it.** The copilot
PreToolUse row's doc comment and ADR amendment document `modifiedArgs` as
the top-level spelling of the same capability: object, full replacement,
valid only with `allow`. Which field the projector emits follows C-130's
recorded outcome, never the docs alone. *Test:* the projector's output for a
copilot mutator verdict contains exactly the C-130-selected field and not
the other; on the decline outcome it contains neither, and the tier reads
`Declined`.

**C-133 — cloud agent stays excluded (D7).** Grim writes nothing under
`.github/hooks/`. *Test:* the existing S-010 absence assertion extends to
`.github/hooks/`.

### Export decline (ADR amendment A7)

**C-140 — admission.** `export::family::admits` gains a `Hook` arm ahead of
the vendor-support arm, with the reason chosen per family by what the format
can hold:

- **Claude plugin family** → `Err(OmitReason::NotRepresentable)`. The format
  *has* a hook surface (`hooks/hooks.json`), but a grim hook member cannot
  run there without grim's runtime, so its content cannot be expressed
  (`family.rs:50-51`).
- **Families with no hook file** → `Err(OmitReason::NoFormatSurface)`
  ("the plugin format has no place for the kind", `family.rs:48-49`). WP-03
  confirms per family which rule applies; any family that does define a hook
  file takes `NotRepresentable`, like Claude.

No new `OmitReason`. `stage_members` therefore marks a hook `Unfetched`, so a
hook member is **never fetched or staged** during export. Both literals sit
in a frozen report, so the choice is one-way (Reversibility). *Tests:* the
`family.rs` matrix gains a Hook column (`Err(NotRepresentable)` for the
Claude family, `Err(NoFormatSurface)` for the others, every client). A
`stage_members` test with a counting access asserts zero fetches for a hook
member. `adr_harness_plugin_export.md` gains a changelog row, since its
admission list enumerates kinds.

**C-141 — warning.** `stage_members` emits exactly **one** `tracing::warn!`
per hook member per plugin (not per client), to stderr, whatever `--format`
is set to:

```
hook '<emitted>' omitted from plugin '<plugin>': exported plugins run without grim, and hooks need grim's hook runtime
```

The text is not a contract. The acceptance test matches on
`hook '<name>' omitted from plugin '<plugin>'` only.

**C-142 — README and JSON reuse existing channels.** `plugin_readme` is
**unchanged**. The member appears in the existing
`Omitted for <client>: hook <emitted>.` line, so the README of a hook-free
plugin stays byte-identical. JSON: `items[].omitted[]` gains
`{"kind":"hook","name":"<emitted>","reason":"not-representable"}` for the
Claude family and `"reason":"no-format-surface"` for families without a hook
file (C-140), which is additive (a new `kind` literal; both reason literals
already exist). Exit codes are unchanged. A plugin with at
least one emitted member exits 0. A plugin whose **only** members are hooks
hits the existing `EmptyPlugin` (65), the same code an all-rules plugin gets
today.

### Principle 9 invariants

**C-150 — the golden fixture runs in CI, not only on a laptop.**
`test_golden_pre_hooks.py` stays pinned to `03e59b0` and is never
regenerated. The deep-verify workflow gains a step that starts `registry:2`
on `localhost:5000` and runs that module alone with
`GRIM_TEST_REGISTRY_HOST=localhost:5000` set **explicitly**: without it,
`test/conftest.py` selects a random-port zot and the module skips even while
port 5000 listens. The CI step also sets `GRIM_REQUIRE_GOLDEN=1`. In that
required mode any skip in the module is a **failure**, and the step asserts
that both golden tests executed and passed (for example by parsing the
pytest JUnit report for two `passed` cases and zero `skipped`).

**C-151 — differential run against main.** `task p9:diff` (deep-verify only,
non-gating in `task verify`) builds `origin/main`'s `grim` (cached by SHA,
same pinned toolchain for both builds) and this branch's `grim`. It runs one
hook-free scenario list with each binary. The list covers `init`, `add` of
skill/rule/agent/mcp/bundle, `install`, `update`, `uninstall`, `remove`,
both scopes, every detectable client present, relative `--config`, and a
**hook-free skill fixture containing an ignored `hook.toml`** (its
`.grimignore` lists `hook.toml`; C-160 must not change its packed bytes,
lock digest or installed content).

- **Checkpoint after every command.** The trees (workspace, fake `HOME`,
  `GRIM_HOME`) are compared after each command, not only at the end, so an
  `uninstall`/`remove` cannot erase an earlier rendering difference.
- **Each command must succeed** (exit status as the scenario expects, equal
  across binaries), and each checkpoint asserts its expected outputs exist
  and are non-empty, so two empty trees cannot pass.
- **Identical inputs.** Both binaries run sequentially under the identical
  absolute root (the same temp path, wiped between runs) with identical
  `HOME`, `GRIM_HOME` and environment.
- **Byte equality with a named mask.** Volatile fields are masked by an
  explicit, named allowlist in `test/tools/p9_diff.py` (for example
  `generated_at`, version strings). Anything not on the allowlist must match
  byte for byte. Directory walks are sorted before comparison.
- **Stdout JSON** may differ only by the additive keys listed in C-153,
  asserted as a key allowlist, not prose.

The tooling (`test/tools/p9_diff.py`, `taskfiles/p9.taskfile.yml`) depends
only on `origin/main` and a `grim` binary, so it ships in **wave 1 as
WP-00**, parallel to and file-disjoint from WP-01. An empty `task p9:diff`
**gates WP-01**, which is the port it protects.

**C-152 — flag off: no dispatcher from clean, deregistration on flip.** Two
separate claims, two tests:

- **(a) Clean state, flag off, writes nothing hook-related (the Principle 9
  claim).** From a state that never armed, with hooks declared and
  `options.experimental.hooks` unset or `false`, run `add`, `install` and
  `update` at both scopes. Afterwards there is no `$GRIM_HOME/hooks/`
  directory at all (no `bin/`, no `dispatch.json`, no payload dir), no
  consent record, and no grim-owned element in any hook surface of all four
  clients (claude settings at both scopes, codex `hooks.json`, copilot
  `hooks/grim.json`, qoder `settings.json`). `grim status` reports `gated`
  with cause `feature-off` and exits 0.
- **(b) On→off transition.** After a genuinely armed install (flag on,
  consented, hook armed for every detected client), set the flag off and run
  `grim install`. Every grim registration element is removed from every hook
  surface, and every dispatch row for that root is gone. The **payload
  directory, the launcher and the consent record are retained** (PR
  behaviour; other roots may still use the launcher, and re-enabling the
  flag must not re-prompt). The docs page (WP-05) states this retention.
  "Reaps all of it" is not the contract.

The C-152 assertions on qoder `settings.json` bind only in WP-08's full run;
in WP-04's own worktree, without WP-02, they are vacuous.

**C-153 — config key and additive surfaces.** `ConfigKey::ExperimentalHooks`
is **appended** at index 10 of `ConfigKey::ALL` (after `SearchMinRelevance`).
Its `KeySpec` is `key: "options.experimental.hooks"`,
`Bool { default: false }`, `title: "Experimental hooks"`, and the
`description` is rewritten to `subsystem-config-keys.md`:

> Controls whether `grim install` arms declared hooks in your AI clients. When unset, install arms nothing and removes grim's existing hook registrations; no environment variable overrides this.

The description promises only what install does. It does **not** claim hooks
"never run": an already-armed hook keeps firing until the next converging
command (C-155), and `grim hook run` does not re-check the flag at fire
time. The `declaration.rs` doc comment begins with that text, which
`config_key_metadata_matches_published_schema` checks.

The **closed list** of additive JSON and schema surfaces follows. Nothing
else may change, and C-151's stdout key allowlist is exactly this list.

- `grim config list`: one appended row.
- `grim schema --kind config`: an `experimental` property and a `hooks`
  property.
- `grim schema --kind lock`: a `hooks` property; the committed snapshot
  `src/lock/testdata/lock.schema.json` is updated to match.
- Bundle-source schema: `BundleSource` gains `hooks`.
- `grim schema --kind hook`: a new CLI literal (`SchemaKind` gains `hook`)
  and a new schema.
- `grim status --format json`: `kind: "hook"` rows, an always-present
  `arming: []` on every item, and state literals `gated` / `not-armed`.
- `grim install|add|update --format json`: an always-present `armed` field
  on every row, `null` for every non-hook kind (PR
  `src/api/install_report.rs`). Kept always-present, because
  `subsystem-cli-api.md` bans `skip_serializing_if`.
- `grim hook list --format json`: a new report.
- `grim export plugin --format json`: the C-142 `omitted[]` rows.

*Test:* C-151's stdout comparison asserts each hook-free JSON delta is a key
on this list; the `lock.schema.json` snapshot test and
`config_key_metadata_matches_published_schema` pass.

**C-154 — no new environment variable.** The PR's C-026 withdrawal stands.
`AGENTS.md`'s environment table gains no row. *Test:* the PR's inertness
test for `GRIM_EXPERIMENTAL_HOOKS` / `GRIM_ALLOW_HOOKS` ports.

**C-155 — deferred disarm is announced, not hidden.** Convergence derives
from install state, so a command that changes arming inputs without
converging leaves the hook armed until the next `grim install` or
`grim uninstall`. `grim remove` of a hook prints one `warn!` naming the hook
and stating it stays armed until `grim install` or `grim uninstall` runs;
exit code and JSON are unchanged. `grim hook revoke` and `grim config set
options.experimental.hooks false` keep the PR's equivalent warnings.
*Test:* `grim remove` of an armed hook → the warning on stderr, exit 0, and
the claude grim element still present until `grim install` removes it
(S-120).

*Follow-up, not a contract:* a fail-closed runtime gate in `grim hook run`
(read the root's flag and consent record before dispatch) would close the
deferral window. It is deferred to the hook-runtime follow-up and recorded
there.

### `.grimignore` and the installed payload

**C-160 — `hook.toml` is never ignored, for Hook artifacts only.**
`ignore_set::NEVER_IGNORED` stays main's three names
(`["SKILL.md", …, GRIMIGNORE]`) for skills, rules and agents. The ignore set
gains `hook.toml` as a never-ignored name **only when the artifact kind is
`Hook`**. A skill that ships an ignored `hook.toml` therefore keeps its
packed bytes, lock digest and installed content identical to main (C-151
carries that fixture). The frozen `stability.md` row "the three
never-ignored names" stays true for every non-hook kind; WP-05 must still
document the hook exception on that row. *Tests:*

- `grim build` of a hook whose `.grimignore` says `*.toml` still packs
  `hook.toml`.
- `grim build` of a skill whose `.grimignore` lists `hook.toml` omits it,
  byte-identical to main's pack.
- The artifact kind reaches the ignore set: the ignore-set builder takes the
  kind (`IgnoreSet` construction gains an `ArtifactKind` argument, defaulting
  every existing caller to its current kind), and `footprint_hash` for an
  installed hook payload (a directory output with `entry: None`) passes
  `ArtifactKind::Hook`. Tamper test: a hook whose `.grimignore` says
  `*.toml`, installed, then `hook.toml` edited → `grim status` shows
  `modified`.
- A Python handler that writes `__pycache__/` into its payload at run time
  leaves `grim status` `installed`.
- Editing `hook.toml`, `.grimignore` or the handler flips the row to
  `modified`.

*Accepted residual for `/security-auditor`:* the default ignore set also
hides a **planted** `__pycache__/*.pyc` in a sibling payload. That is the
C-009 tamper-evidence residual (T1 rewriting a sibling). It is evidence
lost, not prevention lost, and prevention was already out of scope (N2).

## 4. UX scenarios

Exit codes cite `quality-rust-exit_codes.md` via `src/error.rs` classification.
JSON deltas are additive only (C-153).

- **S-101 Upgrade, hook-free project.** Replace the binary, then run
  `grim install`. Outcome: `unchanged`, no file written (C-105, C-151), exit
  0. JSON: each `status` item gains `arming: []`, each install/add/update
  row gains `armed: null`, and nothing else (C-153 closed list).
- **S-102 Declare with the flag off.** `grim add ghcr.io/acme/shell-guard:1`
  → declared and locked. Install skips with one warning naming the flag.
  `status` shows `gated`, cause `feature-off`. Exit 0. No payload, launcher
  or dispatch table (C-152).
- **S-103 Global arming across four clients.** Flag on, `--global`, with
  claude, codex, copilot and qoder detected, then `grim install`. Result:
  payload at `$GRIM_HOME/hooks/<name>/`, one grim element per
  (client, event, matcher) in `~/.claude/settings.json`,
  `~/.codex/hooks.json`, `~/.copilot/hooks/grim.json` and
  `$QODER_CONFIG_DIR|~/.qoder/settings.json`. The report names the clients
  and tiers. Exit 0. Error case: a hook surface file is unparseable → that
  client is `not-armed` with a warning naming client and hook (C-017), and
  the exit stays 0.
- **S-104 Qoder at project scope.** A project install with qoder selected
  skips qoder with a `warn!`: hooks are global-only for qoder, the warning
  says so, and it points at `--global`. Claude project arms after consent.
  Exit 0.
- **S-105 A user's own Qoder hooks survive.** After arm, re-arm and
  uninstall, the user's hook entries, other keys and grim's MCP entry are
  byte-preserved, and the MCP row stays `installed` (C-123).
- **S-106 Qoder unverifiable.** C-121 fails, so qoder declines hooks. An
  explicit `--client qoder` hook install warns that qoder has no hook
  surface in this grim. The `clients.md` cell is ✗. Exit 0.
- **S-107 Copilot mutator.** A mutator on `PreToolUse` `Bash` rewrites the
  command through the field C-130's live probe selected
  (`hookSpecificOutput.updatedInput` or top-level `modifiedArgs`), and
  Copilot runs the rewritten command. If the probe found neither applies,
  the copilot mutator reports `Declined` and is not armed.
- **S-108 Copilot SessionStart context.** A context-returning SessionStart
  hook's text reaches the Copilot session only if C-131's probe admitted
  the field. Otherwise the hook still runs and the context is dropped by
  projection, which `grim hook list` shows as a declined cell.
- **S-109 Export with a hook member.** `grim export plugin --project` on a
  project with skill `a` and hook `g` → the warning (C-141) and the README
  line `Omitted for claude: hook g.`. JSON `omitted[]` carries
  `{kind:"hook",name:"g",reason:"not-representable"}` for the Claude family
  (`no-format-surface` for a family without a hook file, C-140). Exit 0. Error case:
  a project with only hooks → `EmptyPlugin`, 65 (existing).
- **S-110 Path source for a hook.** `grim add ./my-hook --kind hook` →
  `UnsupportedPathKind`, using the existing mapping. Without `--kind` →
  `UninferablePathKind` (existing). Nothing is written.
- **S-111 Reserved binding.** `grim add ghcr.io/acme/x:1 --name bin` (a hook)
  → exit 64, config untouched. A hand-edited `[hooks] bin = …` makes
  `install` skip it with a warning, and `status` shows `missing` (C-101
  step 1).
- **S-112 Older grim, hooks lock.** A pre-hooks grim reading a lock with
  `[[hooks]]` exits 78 (`EX_CONFIG`), as for every unknown lock field
  (stability › Forward compatibility, PR S-014).
- **S-113 TUI delete of a hook.** The registration and dispatch entry are
  gone in the same action, with no prompt (C-113). Installing a hook row
  directly from the TUI stays refused (the PR's WP-H decision); a hook
  reaches a TUI install only as a bundle member, and in an unconsented
  workspace that member shows `gated` / `workspace-not-consented` and names
  `grim hook allow`.
- **S-114 Relative `--config`.** `grim --config grimoire.toml hook allow`
  in repo A does not arm repo B invoked the same way (C-118).
- **S-115 Tampered dev record.** `state.json` holds `dev = true` on a hook,
  then `grim update` runs → one warning, no panic, exit 0 (C-111).
- **S-116 Runtime byproducts.** A Python handler creates `__pycache__/` in
  its payload, and `status` stays `installed`. A hand edit to `hook.toml`
  shows `modified`, and `install` refuses without `--force`, exit 0 with the
  row marked refused, as for skills (C-160).
- **S-117 Copilot cloud agent.** Nothing is written under `.github/hooks/`,
  and the docs' clients page states the exclusion (C-133).
- **S-118 Docs journey.** Defined in the plan (C-170): the hooks use-case
  page is reachable from the sidebar and its flag → add → install → allow →
  status fences run under the doc-example harness.
- **S-119 TUI bundle install adds a hook, consented workspace.** The
  workspace holds a consent record for its current hooks. A TUI install of
  a bundle whose new version adds hook `g` → `g` is `gated` with cause
  `workspace-not-consented` (drift, not grant), nothing is armed, no grim
  element for `g` appears in claude `settings.json`, and the TUI names
  `grim hook allow` (C-113).
- **S-120 `grim remove` of an armed hook.** Config and lock drop the hook,
  and stderr carries one warning that it stays armed until `grim install`
  or `grim uninstall`. Exit 0, JSON unchanged. The next `grim install`
  removes its registrations (C-155).

## 5. Trade-off: composing hooks with main's installer

| Option | Shape |
|---|---|
| **A** | Payload through main's `install_one` → `stage_locked_artifact` → `materialize` like a skill. Registrations and the dispatch table come from a derived `converge_for` post-pass after every `sync_config` loop (the PR's `hook_registrar`, re-seated per C-104). |
| **B** | Registration inside `Vendor::sync_config`. Widen its signature to carry the policy and config. On HEAD that is 3 impls (the trait default at `vendor.rs:470` plus overrides in `vendor_claude.rs:260` and `vendor_opencode.rs:341`), not 19. |
| **C** | A dedicated `install_hook` branch like `install_mcp`, recording each registration as a `ClientOutput` entry output. |
| **D** | Hang convergence on `roll_forward` in the resolve layer, since every lock change passes through it. |
| **E** | Converge inside `InstallState::persist`, so every state write re-derives arming. |
| **F** | A, plus one shared vendor-sync helper replacing the six `sync_config` loops. |
| **A+G** | A, plus a fail-closed runtime gate: `grim hook run` reads the root's flag and consent record before dispatch. |

Weighted criteria (weights sum to 1; scores 1–5):

| Criterion | W | A | B | C | D |
|---|---|---|---|---|---|
| Principle 9 blast radius on hook-free paths | 0.30 | 4 — one gated call, write-free short circuit (C-105); still touches the prune chain (C-115) | 3 — 3 `sync_config` impls re-signatured, plus every call site | 3 — new branch, but new record shape in `state.json` | 2 — resolve runs for `lock` too, which must never write client files |
| Correctness and security (reap `owned − desired`, ADR decision L, no stale arming) | 0.25 | 4 — derive-never-record, proven on the PR (WP-R); deferred disarm after `remove`/`revoke`/flag flip (C-155) | 2 — each loop iterates the **operation-scoped** client set (`sync_client_set(target.clients(), ..)`, TUI `involved_clients`), so a flag-off or revoke reap never reaches a hook client the operation did not touch; the per-root dispatch table cannot be written wholesale per client | 2 — main's `retired_outputs` reaping narrows the #54/#55 orphan-record argument, but flag and consent changes alter no record, so C degenerates into C + A | 1 — no install state at resolve time, so it cannot derive the desired set |
| Reuse of main's seams / DRY | 0.20 | 4 — reuses `stage_locked_artifact`, `integrity_gate` and dedup unchanged; one new helper | 3 | 2 — forks a third install path | 2 |
| Diff size and review load | 0.15 | 4 — the PR's `installer.rs` production additions are 133 lines by merge-base diff, comments included; the code lines are well under 100 | 3 | 2 | 3 |
| Testability | 0.10 | 4 — PR tests port; C-104's count test plus behavioural reaping tests guard the call sites | 3 | 3 | 2 |
| **Weighted** | | **4.00** | **2.75** | **2.40** | **1.90** |

Rows scored but not adopted (same weights):

| Option | Weighted | Verdict | Reason |
|---|---|---|---|
| E | 3.40 | Rejected | Layering: a storage-persist primitive would write client configs and need the hook policy inside the state layer. |
| F | 3.85 | Rejected | Two Hats: it refactors six hook-free loops inside a feature PR. A later refactor PR may take it. |
| A+G | 4.10 | Deferred | Scores above A, but the runtime gate belongs to the hook-runtime follow-up (C-155). A ships now; G lands there. |

**Steelman of B**, the best rejected option: `sync_config` already calls
itself "the reversible config-registration seam (hooks ADR pattern)"
(`vendor.rs:450-451`), and folding hooks in would honour that and drop the
second pass. B still loses because the seam's client set is
operation-scoped while arming is policy-scoped.

**`PlannedRegistration` considered** (`installer.rs:2297`). It does not
fit: it is an MCP two-level JSON-pointer member with plan-all-then-write
atomicity, while hook entries are nested `event → [{matcher, hooks:[…]}]`
arrays carrying a marker key, which is C-117's primitive.

**Recommendation: A.** The PR's "separate converge step" is the right
architecture. The only change is where it is called: main forked the
`sync_config` loop into six places (installer, update, uninstall, three TUI
sites), so convergence gets one entry point (`converge_for`), a count test
that pins it to every loop, behavioural reaping tests per seam, and a policy
attached at every mutating seam (C-113). The rejected options fail on
decision L (C, D), on client-set reach (B), on layering (E) or on scope (F).

## 6. Migration and rollout — Principle 9 checklist

| Question | Answer |
|---|---|
| Schema change to a released file? | **No.** Lock, state, config and declaration hash only gain content when a hook is declared. Empty collections are skipped (C-116), and `DECLARATION_HASH_VERSION` stays 1. |
| State migration? | **None.** Hooks were never released, so no shipped record exists. Install-state records for hooks use new anchor content only when a hook is present. |
| Old-path reaper? | **None for released paths.** The PR's pre-SEC-1 reaper (`<ws>/.grimoire/hooks/<name>` → `$GRIM_HOME`) serves only branch testers. Dropped (Open Question 2, resolved). |
| Upgrade fixture? | **Yes, existing:** `test/data/golden/pre_hooks_03e59b0`, pinned and never regenerated (C-015), now run in CI (C-150). **Plus** the differential run against current main (C-151), which covers client output files the golden does not. |
| Renderer self-heal? | Re-materializing a hook leaves `status` not-modified (PR test `re_materializing_a_hook_leaves_the_record_not_modified`). Re-converging writes zero bytes (C-124). |
| Default flip? | **Off** by default. Config-only, with no env override (C-153, C-154). |
| Downgrade? | A hook-bearing lock or state on an older grim exits 78 (S-112), documented under stability › Forward compatibility. A hook-free project is byte-identical, so it downgrades cleanly. |
| Rollout order in the one PR | 0. C-151 tooling (WP-00, wave 1, parallel to WP-01; an empty `task p9:diff` gates WP-01); 1. squash plus enum/type prerequisites; 2. seam ports C-101…C-118 (one serial unit for `installer.rs` + `target.rs`); 3. Qoder/Copilot data behind the C-121/C-131 evidence; 4. export C-140…C-142; 5. docs, catalog, watchlist, ADR amendments; 6. C-150/C-152 proof and the final C-151 run, Windows local run against `origin/main`, then `/hex-review` and `/finalize`. |

## 7. Constitution check

| # | Principle | Verdict | Note |
|---|---|---|---|
| 1 | Understand first | ✓ | Every seam contract cites main's current function and line. |
| 2 | Prove it works | ✓ | Each contract names its test. The Qoder and Copilot rows are gated on dated evidence (C-121, C-131). |
| 3 | Keep it safe | ✓ | Charset guard on hook bindings (C-109), no panic on a tampered state (C-111), marker-tolerance gate before touching a user's Qoder file (C-121), TUI reaping (C-113). Residual declared (C-160). |
| 4 | Keep it simple | ✓ | No new `DeclareError` variant, no new `OmitReason`, `plugin_readme` untouched, one converge entry point. |
| 5 | DRY | ✓ | Duplicate escaping helper removed (C-117). The prune chain collapses onto `iter_artifacts` (C-115). |
| 6 | Ship it | ✓ | One PR, no push by agents. |
| 7 | Leave a trail | ✓ | ADR amendments A7/A8, the watchlist rows, and this spec. |
| 8 | Learn and adapt | ✓ | The PR's third-firing prune trap is removed structurally rather than patched. |
| 9 | Preserve compatibility | ✓ with two rows | See below. |

| Deviation | Principle | Justification |
|---|---|---|
| `grim status --format json` output for a **hook-free** project changes: `arming: []` is added to every item | 9 | Additive field under stability › Additive fields. It is always present per `subsystem-cli-api.md`. On-disk files stay byte-identical (C-151). |
| `grim install`/`add`/`update --format json` output for a **hook-free** project changes: `armed: null` is added to every row | 9 | Additive field under stability › Additive fields. Kept always-present rather than `skip_serializing_if`, which `subsystem-cli-api.md` bans; a one-way literal (Reversibility). On-disk files stay byte-identical (C-151). |

## 8. Open questions

All three are **resolved**; recorded here for the trail.

1. **Should Qoder hooks ship if C-121 cannot confirm the CLI shape or
   unknown-key tolerance?** **Resolved (D-4): decline if unverifiable.**
   Qoder ships declined with a watchlist row when gate (a) or (b) fails. An
   untolerated `com.grimoire.managed` key could silently drop a user's own
   Qoder hooks (the Codex precedent). A decline costs zero lines and is
   additive to lift later. Gate (a) is satisfied by the CLI docs; (b) still
   needs a probe or an explicit upstream statement.
2. **Should the PR's pre-SEC-1 payload reaper be ported?** **Resolved: drop
   it.** The layout never shipped, so it only migrates branch testers.
   Dropping it removes a `candidate_anchors` classification from the port
   surface. Principle 9 does not bind unreleased layouts.
3. **Should the golden fixture run in deep verify only, or in every PR's
   acceptance job?** **Resolved: deep verify only (C-150).** It needs port
   5000, which conflicts with the session registry's dynamic port. Deep
   verify is already the Principle 9 gate the goal's DONE block cites.
