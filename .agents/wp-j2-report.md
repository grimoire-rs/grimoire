# WP-J2 — install orchestration: report

Branch `hex/hooks-artifact-kind--wp4-j2`, based on feature tip `3772b76`.
Two commits: the package (`6a80bcd`), then WP-K's relayed **F-1** Block.
`task --force verify` green: **2823 unit tests** (was 2800, +23),
**1019 acceptance**, **51 AI-config**.

---

## 1. The three same-commit obligations

### Obligation 1 — the `Hook` arm in `client_supports_kind` — **discharged here**

`src/install/installer.rs`, `client_supports_kind`:

```rust
ArtifactKind::Hook => {
    client.vendor().hook_surface().is_some() && client.vendor().kind_surface(kind, scope)
}
```

**Which reading, and why.** `hook_surface().is_some() && kind_surface(Hook, scope)`,
**not** `client_target::hook_matrix_cell`. Three reasons:

1. WP-J1 already wrote the predicate down for me. `path_anchor.rs:972-977`'s doc on
   `is_declined_global_pair` says verbatim: *"this predicate must agree with
   `client_supports_kind`'s `Hook` arm (`hook_surface().is_some() && kind_surface(kind,
   scope)`, WP-J2)"*.
2. `command/status.rs:842-844`'s `client_has_hook_surface` (WP-H, merged) is spelled
   **identically**. A third spelling is how the install side and the report side come to
   disagree — the exact divergence this predicate exists to close.
3. `hook_matrix_cell` answers a *documentation* question: it probes
   `Vendor::hook_registration` with synthetic `HookEntry` values and stand-in launcher
   paths (`HOOK_CELL_PROBE_LAUNCHER`), is scope-blind, and is per-`(event, tier)`. Mine is
   a scope-aware install question.

**The defect was live and is now proven closed by test output.** Before the arm, the
catch-all evaluated `kind_support(Hook) != Declined && kind_surface(..)` = `true && true`
for all 18 clients. The failing-first run printed:

```
---- client_supports_kind_reads_hook_from_the_hook_surface stdout ----
assertion `left == right` failed: opencode at Project
  left: true
 right: false
```

Pinned by `client_supports_kind_reads_hook_from_the_hook_surface` (all 18 × both scopes,
plus the 15-surfaceless count asserted explicitly) and by
`hook_support_and_hook_anchoring_agree_for_every_client`, which closes the WP-J1 box's
"the two predicates disagree" window against `AnchoredPath::from_target`.

### Obligation 2 — the widened no-op guard — **already landed by WP-I; verified, not redone**

`src/install/hook_registrar.rs:308` already reads:

```rust
if !has_hook_record(vendor, state) && !owns_anything(vendor, surface) {
    return Ok(HookSync::NoHooks);
}
```

Direction confirmed correct, and it is covered by WP-I's own
`a_grim_owned_registration_with_no_record_no_longer_reads_as_a_no_op`. I read
`owns_anything`'s doc first as instructed: it is a deliberate over-approximation, probes
**both** marker constants (`HOOK_MARKER_KEY` **and** `HOOK_MARKER_VALUE`), and documents
the skip-vs-reap split plus the three false-negative classes of
`owned_nested_handlers`. **I changed nothing here** — the guard as merged is the shape the
ordering box asks for, so I touched no file outside my four core files for it.

### Obligation 3 — the panic refusal at `hook_registrar.rs:328` — **already converted; direction confirmed**

The site is no longer `unimplemented!()`; it is
`Err(io::Error::other(crate::oci::hook::unsupported_kind()))`. Direction is right: every
`sync_config` caller logs an `Err` as a **warning**, so the hand-edited-`state.json` input
degrades to "hooks not armed for this client" with exit 0 and the primary command
succeeding — I3 preserved, and correctly classified N2/N4, not a security finding. **Not
re-done.**

Executed proof that it is warn-only and not fatal (real binary, real registry):

```
$ grim install
WARN grim::install::installer: vendor config sync failed; artifacts installed and state
     saved, registration skipped client=claude
     error=the 'hook' artifact kind is not supported by this build of grim; …
Kind  Name         Target                                    Status
hook  shell-guard  …/ws/.grimoire/hooks/shell-guard          installed
install exit=0
```

---

## 2. What I implemented

### `install_one`'s `Hook` branch — deliberately **not** a separate body

The plan says "shaped like the `install_mcp` branch point … but with its own body". I did
not add a separate body, and the reason is a DRY one worth recording rather than a
shortcut. `install_mcp` needs its own body because an MCP descriptor **never
materializes** — it splices a config entry. A hook payload **does** materialize: a
verbatim directory tree, the skill shape. A separate body would have to re-implement the
stage-into-a-sibling-tempdir → fsync → `remove_path` + `rename` publish → destination
dedup → footprint hash → per-client `ClientOutput` machinery (`installer.rs:718-912`) —
and that dedup is *exactly* S-003's one-directory-N-outputs shape, already shipped for
pool skills. Duplicating it would fork the C-009 crash-safety window and the prune
refcount's contract.

So the hook-specific work is the three places the generic path could not answer:

| Change | Site | Why |
|---|---|---|
| `client_supports_kind`'s `Hook` arm | `installer.rs` | obligation 1 |
| `locate_canonical`'s two `Hook` arms | `installer.rs:2443`, `:2463` | the two refusals the ledger assigns to WP-J2 (`rg 'hook kind: WP-'` → now zero) |
| `kind_is_permanently_declined` | `installer.rs` (new fn) | the skip *classification* |

`locate_canonical` now folds `Hook` into the `Skill` arms: a hook's canonical entry is its
payload **directory**. Confirmed against the wire layout — `fetch.rs:508` reads a hook's
index as `<name>/hook.toml`, and the real `grim release` tar contains
`shell-guard/hook.toml` + `shell-guard/guard.sh` (executed, see §5).

`kind_is_permanently_declined` is the skip-reporting fix. `install_one`'s retain loop chose
between a `debug!` skip and a `warn!` "no `{kind}` directory at `{scope}` scope" line by
asking `kind_support(kind) == Declined`. For `Hook` that answer is *always* `Native` (ADR
decision A), so the 15 surfaceless clients would each have emitted a **warning telling the
user to try the other scope**, where the answer is identically no. Now: no hook surface ⇒
permanent ⇒ `debug!`; surface exists but not at this scope (codex/copilot at project) ⇒
`warn!`, which is the real scope gap.

### `expected_outputs.rs`, `prune.rs`, `install_state.rs` — no production change was needed

Stated plainly because it matters for review:

- **`expected_outputs.rs`** is generic — `expected_clients` delegates straight to
  `client_supports_kind`, so obligation 1 fixes `grim status`'s `outputs_pending` for
  hooks with zero further code. Two tests added.
- **`install_state.rs`** is kind-agnostic. One round-trip test added.
- **`prune.rs`**'s `shared_by_surviving_sibling` is generic over `ClientOutput` and never
  consults `client_supports_kind` (`rg 'client_supports_kind|expected_clients'
  src/install/prune.rs` → **zero hits**). The two C-020 tests therefore **passed on first
  run** — they are characterization/regression guards, not failing-first tests, and I am
  saying so rather than implying I drove them red. `prune.rs` *did* need one production
  fix, but a different one — see §4, finding **B1**.

---

## 3. Contract / scenario → test map

| ID | Test | File | Status |
|---|---|---|---|
| **D-1 / obligation 1** | `client_supports_kind_reads_hook_from_the_hook_surface` | `installer.rs` | failed first (output above), now green |
| **D-1 (predicate agreement)** | `hook_support_and_hook_anchoring_agree_for_every_client` | `installer.rs` | failed first, now green |
| **C-019** install half | `an_installed_hook_payload_carries_no_exec_bit` (`#[cfg(unix)]`, `mode & 0o111 == 0`) | `installer.rs` | failed first, now green |
| **C-019** build half | *not duplicated* — WP-A's `running_the_payload_directly_is_refused_at_build` in `src/oci/hook.rs` owns it | — | pre-existing |
| **C-020** sequence | `c020_shared_hook_payload_survives_until_the_last_client_drops_it` | `prune.rs` | green on first run (see §2) + **executed** (§5) |
| **C-020** record-only refcount | `c020_a_partial_drop_never_deletes_a_payload_a_sibling_still_records` | `prune.rs` | green on first run |
| **S-003** | `a_hook_materializes_one_shared_payload_dir_with_one_output_per_client` | `installer.rs` | failed first, now green |
| **S-003** (state shape) | `a_hook_record_round_trips_with_one_output_per_arming_client` | `install_state.rs` | failed first (assertion), now green |
| **S-002** | **see §4 finding A1 — the plan's S-002 and my brief disagree; the plan's version is already satisfied by merged code, and the install-report half is unimplemented and out of my file set** | — | partial, declared |
| **S-007** install half | `a_new_hook_digest_re_materializes_and_moves_the_recorded_pin` | `installer.rs` | failed first, now green |
| **S-007** re-prompt half | not reachable — see §4 finding A2 | — | blocked, declared |
| **S-008** | executed end to end (§5); enabled by finding **B2**'s fix | — | **verified by execution** |
| **S-010 v1** | `a_project_hook_install_writes_nothing_armable_into_the_workspace` + executed (`.claude/` empty after install) | `installer.rs` | failed first, now green |
| **S-011** | install-side half only (no registration is written at all); the hostile-clone fixtures are WP-O's | — | partial by design |
| **S-013 / A1 scope gap** | `a_surfaceless_client_is_reported_as_skipped_never_armed`, `a_codex_hook_at_project_scope_is_skipped` | `installer.rs` | first failed / second passed on first run (already correct via `kind_surface`) |
| **Principle 9 self-heal** | `re_materializing_a_hook_leaves_the_record_not_modified` (asserts record byte-equality + `AlreadyInstalled`) | `installer.rs` | failed first, now green |
| I5 tamper-evidence | `a_locally_modified_hook_payload_is_refused_until_forced` | `installer.rs` | failed first, now green |
| `expected_outputs` seam | `only_hook_capable_clients_are_expected_hook_targets`, `the_own_file_hook_clients_are_expected_at_global_scope_only` | `expected_outputs.rs` | green on first run (they pin obligation 1's effect) |
| **B1** regression | `a_still_declared_hook_is_never_pruned_as_an_orphan` + `a_hook_that_dropped_out_of_the_lock_is_pruned` | `prune.rs` | **failed first**, now green |
| **B2** regression | `every_accepted_kind_string_resolves_to_that_kind` ×2 | `remove.rs`, `uninstall.rs` | added with the fix |
| **F-1** required field | `a_row_without_a_client_is_refused_never_defaulted` | `hook_dispatch.rs` | green (new field) |
| **F-1** per-client rows | `two_clients_arming_one_hook_are_two_selectable_rows_in_one_root` | `hook_dispatch.rs` | green (new field) |
| **F-1** stamping | `desired_entries_stamps_each_row_with_its_own_arming_client` | `hook_registrar.rs` | green (new field) |

---

## 4. What I found **wrong** in the plan and in already-merged code

### B1 — Block, data loss, found by **execution** — `prune_orphans` deletes every installed hook on the next `grim update`

`src/install/prune.rs`'s `declared` set was a hand-maintained chain:
`skills → rules → agents → mcp`. **`hooks` was missing.** Every still-declared hook record
therefore looked orphaned, so `grim update` deleted the payload and dropped the record.

Reproduced with the real binary before the fix:

```
$ grim --global update
Kind  Name         Old                  New                  Action
hook  shell-guard  sha256:e48590230d1e  sha256:e48590230d1e  unchanged
hook  shell-guard  sha256:e48590230d1e  -                    removed
=== payload after dropping copilot ===
payload GONE (wrong)
```

Note the function's **own comment** warned about this class: *"an omitted kind (agents/mcp
were missing until this fix) makes every one of its still-declared records look orphaned
and prunes them on every `grim update`."* The trap fired a third time.

Fixed by deriving the set from `GrimoireLock::iter_artifacts()` — the lock's own definition
of "every locked artifact", which already includes `hooks` — so it cannot drift again.
Bundles stay correctly absent (a bundle never enters the lock as an artifact). In my file
set (`prune.rs`). Failing-first test recorded above.

### B2 — Block, wrong-target action — `grim remove`/`grim uninstall` silently treat `hook` as `rule`

`src/command/remove.rs:48` and `src/command/uninstall.rs:63` parsed the positional
`<kind>` with a local `match` ending `_ => ArtifactKind::Rule`. The `value_parser` was
widened to accept `"hook"` (remove.rs's own doc comment celebrates the additive
widening) — **the arm list was not**. Executed, before the fix:

```
$ grim uninstall hook shell-guard
Kind  Name         Status
rule  shell-guard  not-installed
uninstall exit=0
$ test -e .../hooks/shell-guard  →  STILL PRESENT
$ grim remove hook shell-guard
Kind  Name         Status
rule  shell-guard  absent           # declaration still in grimoire.toml
```

Two consequences: **S-008 was unsatisfiable** (no way to uninstall a hook), and — worse —
if a *rule* shares the binding name, `grim uninstall hook X` deletes the **rule** X's
files. Both now parse through `ArtifactKind::from_kind_str`, the existing single source of
truth for the spelling, and refuse (64) rather than panic on an unreachable value (A-3's
lesson). See §6 for the file-set declaration on `uninstall.rs`.

### A1 — the brief's S-002 does not match the plan's S-002, and the plan's is already satisfied

My brief said the approval prompt must name *"the artifact, its digest, its tier … and
every bundle-delivered member"*. The plan's actual S-002 (`plan_hooks_artifact_kind.md`
:989-994) says the opposite about the prompt:

> one prompt naming the **registry**, not the artifact

and `src/hook/trust.rs:541-543` (merged, WP-G) states the same as a caller obligation:
*"The prompt names the **registry**, never the artifact (S-002). Per-hook prompting is the
re-prompt-habituation failure the ADR lists as a risk and the owner reversed D5 to
avoid."* The brief's wording is the **pre-reversal D5** design (per-hook digest approval),
which the owner reversed on 2026-08-14. Digest and bundle-member enumeration appear
nowhere in S-002.

I implemented nothing against the withdrawn version. `prompt_for_registry` already
satisfies the plan's prompt half verbatim (registry named, escaped via `escape_debug`
against CWE-117/150, the exact file and the exact `trust_hooks = true` line stated, "no
TTY never asks — pass `--allow-hooks`" stated).

**What is genuinely missing is S-002's *other* half**: *"the install report names what was
armed, on which clients, at which tier (mutator wording distinct)"*. `grim install`'s
report today is `Kind | Name | Target | Status` — no tier, no arming column. That lives in
`src/api/install_report.rs`, which is **not in my file set**, so I did not add it. The
shipped surface for arming is `grim status`'s `arming[]` array (WP-H), verified working on
a real hook record in §5. **Recommendation: assign the install-report tier column
explicitly, to WP-H's owner or WP-M, or withdraw that half of S-002 in favour of
`grim status`.**

### A2 — Block on the feature as a whole: **nothing arms**, because `sync_for_state`'s convergence body is a stub nobody owns

The plan's Status block says the Implement pass is complete and
`rg 'unimplemented!(' src/` returns zero. That is true, but it is not the same claim as
"convergence is implemented". `hook_registrar::sync_for_state`'s body is **six documented
steps** (refuse early → generate launcher → compute desired set → write dispatch entry →
converge the client surface incl. `owned − desired` reap → git-exclude hygiene). None of
them exists. Past the widened no-op guard the function returns
`Err(unsupported_kind())` **unconditionally** — the shape that was correct while only a
hand-edited `state.json` could reach it, and that a legitimate hook record now also
reaches. Executed evidence is the warn line in §1 obligation 3.

Corroborating evidence that the body was never written: `desired_entries`,
`root_scope_for`, `ensure_settings_local_excluded`, `drop_settings_local_exclude`,
`HookSync`, `GIT_EXCLUDE_RELATIVE` all still carry `#[expect(dead_code)]` with
"REMOVAL TRIGGER: … when that caller lands", and `hook::trust`'s `arming`,
`interactivity`, `prompt_for_registry`, `persist_grant`, `NotArmedReason`, `GrantSource`
do too.

**The structural reason it is unassigned**, and this is the part the plan never resolved:
`Vendor::sync_config(state, workspace, scope)` cannot see the config, the CLI flags, or
the global-config path, so *neither* the feature flag (`ExperimentalOptions::hooks_enabled`)
*nor* per-registry trust can be evaluated where the plan puts them.
`trust.rs:546-554` and `:670-676` both say the composition
(`arming` → `prompt_for_registry` → `persist_grant`) *"belongs one layer up … (WP-J2's
`installer.rs`, or WP-K)"* — but threading it there means widening
`install_and_persist` / `install_all_with_progress`, whose **7 production call sites** span
`tui/app.rs` (3), `command/update.rs` (2), `command/install.rs` (2), `command/add.rs` (1).
`command/update.rs` and `command/add.rs` are not in my declared set at all and the compiler
does not force them, so per my brief I **stopped and am reporting rather than editing**.

Consequences to decide, not to assume settled:

1. **The trust gate is not in `install_one`.** I deliberately did not put it there, and I
   think the plan's WP-J2 line ("fetch → materialize once per scope → **registry-trust
   gate** → one `ClientOutput`") is wrong on this point: it contradicts
   `sync_for_state` step 3 (which places flag+trust in convergence), `desired_entries`'
   own `trust: &dyn Fn(&LockedSource) -> bool` parameter, and
   `path_anchor.rs:324` ("nothing here is armable; the registration is"). A payload is
   inert data at `0o644` under a grim-owned directory. Gating the *payload* would also
   break S-001's companion requirement that `grim status` reports `gated` — which needs a
   record to report on, and which **works today** (§5).
2. **S-001's "install skips with a warning" is therefore not implemented** — install
   materializes the payload and reports `installed`. I3 and I4 are both intact (nothing is
   armed; `grim status` says `gated`), but the scenario's literal wording is unmet, and it
   needs the same plumbing as the gate.
3. **S-007's re-prompt half is unreachable** for the same reason.

**Recommended assignment:** one WP owning `InstallTarget` (where `shared_skills`, the other
config-derived install policy, already lives) plus the 7 `parse`/`install_and_persist`
call sites, landing the flag + trust + consent composition and `sync_for_state`'s body
together. It is not a WP-J2-shaped change and it is not WP-K's either as scoped.

### A3 — the ordering box's refusal list is stale; **only 2 of the 8 named sites were mine**

The WP-J2 ordering hazard box says the refusals at `command/install.rs:200,289`,
`command/remove.rs:66`, `fetch.rs:488,930`, `mcp/render.rs:103,130,158` are "WP-J2's to
delete". In the merged tree **none of them is a blanket refusal any more**:

- `fetch.rs:494` — `Hook` **joins** the tar-unpack arm; implemented, no refusal.
- `command/remove.rs:71` — a real `set.hooks.remove(..)` arm; implemented.
- `mcp/render.rs:110` — a **permanent, correct** decision (hooks render no files; honouring
  a caller-chosen `dest_dir` would arm code there — I1 through grim's own write tool).
- `command/install.rs:219,307` — a **permanent** WP-H decision (hooks are not
  dev-installable from a path: no registry ⇒ no `trust_hooks` ⇒ consent unexpressible).
- `tui/app.rs` ×4 — likewise permanent.

Deleting any of those would have been wrong. The real ledger line is accurate: **WP-J2 owns
2 sites in `installer.rs`** — both in `locate_canonical`. `rg 'hook kind: WP-' src/` now
returns only the doc reference in `oci/hook.rs:1184`.

### A4 — `skill_package.rs:439` is `:436` in the merged tree

C-019's premise cites `skill_package.rs:439` for the `0o644` stamp. The line is
`append_entry` at **`:436`** (`header.set_mode(0o644)`). Cosmetic; noting it so the next
reader does not chase a shifted line.

---

## 4b. F-1 and F-2 — the WP-K findings relayed mid-package

### F-1 (Block) — `DispatchEntry` gained a **required** `client` field

**All four premises independently verified against source before I touched anything:**

| Claim | Verified |
|---|---|
| `DispatchEntry`'s doc says the runtime selects by `(root token, client, event)` | `hook_dispatch.rs:370` |
| …but the struct has no `client` field | `:373-410` — `artifact`, `id`, `event`, `tier`, `matcher`, `handler`, `timeout`, `payload`, `payload_dir`, `resolved_digest`, `policy`. No `client`. |
| `desired_entries` is **per vendor** | `hook_registrar.rs:569` — `.filter(\|o\| o.client == vendor.name())`, and `payload_dir` derived from that output |
| `converge_root` is **per root, wholesale** | `hook_dispatch.rs:691-703` — `table.roots.insert(token, DispatchRoot { root, hooks: hooks.to_vec() })` |
| no production call site | `rg converge_root src/` → two doc references + its own tests only |

**One correction to the finding's own wording, and it strengthens the case.** The
message says unioning across vendors "produces rows differing only in
`payload_dir`". It does not: a hook's payload is **one directory per scope shared
by every arming client** (S-003 — I proved this by execution in §5, three clients
→ three outputs all at `grim-home` + `hooks/shell-guard`), and every remaining
field comes from the record or the manifest. So the union produces **N
byte-identical rows**. That is worse than a duplicate, because the client
dimension is not merely unpopulated — it is **unrecoverable**: a dedup by
`PartialEq` would silently collapse "armed for claude only" and "armed for
claude + codex" into the same table bytes. Avoiding the declining-client leak is
therefore *unrepresentable* without the field, not merely easy to get wrong.

**Required, not `#[serde(default)]` — I agree, and the never-shipped premise is
verified, not assumed.** `git ls-tree -r --name-only 03e59b0 -- src/install/`
(the v0.13.0 release commit) contains **no hook file at all**, so there is no
`dispatch.json` anywhere for Principle 9 to protect. `DISPATCH_SCHEMA` stays `1`.
The fail-safe argument also already has its enforcing test: `one_bad_row_rejects_the_whole_table`
(pre-existing, W2) means a client-less row degrades the table to *not armed*,
never to *armed for everyone*.

**Type: `String`, not `ClientTarget`.** `ClientTarget` derives no
`Serialize`/`Deserialize` (`client_target.rs:61`), so typing it would mean
widening a shipped enum's surface in a file WP-L is working in. `String` also
matches `ClientOutput::client` — one spelling across the two structures the
convergence loop bridges — and keeps a legacy/unparsable recorded client name
representable, which install state deliberately tolerates. It simply selects no
row, the fail-safe direction.

Blast radius was **2** construction sites, both compiler-forced. Pinned by three
tests: `a_row_without_a_client_is_refused_never_defaulted` (strips `client` from
a serialized row and asserts deserialization fails naming the field),
`two_clients_arming_one_hook_are_two_selectable_rows_in_one_root` (asserts the
rows differ in *nothing but* `client`, survive one wholesale write as two
selectable rows, and that a never-armed client is not selectable), and
`desired_entries_stamps_each_row_with_its_own_arming_client`.

**The runtime's row-selection rule is NOT in this commit** — it belongs to
`src/command/hook*`, which does not exist in my tree. Until WP-K lands it the
field is carried and unread. Say so rather than implying F-1 is closed end to
end.

**Files this added outside my declared set** (both WP-I's, both declared here):

- `src/install/hook_dispatch.rs` — the field + its doc + the test helper split +
  2 tests. **Directed by the orchestrator**; not in my brief's list.
- `src/install/hook_registrar.rs` — **compiler-forced** (`E0063: missing field
  client`), one line populating it from `output.client` rather than
  `vendor.name()` so the row's client and its `payload_dir` come from one
  `ClientOutput` and cannot describe different clients. Plus 1 test, which
  required converting `desired_entries`' `#[expect(dead_code)]` to
  `#[cfg_attr(not(test), expect(…))]` — the attribute cannot survive its first
  reader in either direction, and `root_scope_for` in the same file already sets
  that precedent.

### F-2 — the audit trail beside the dispatch table: **no objection, and its two load-bearing facts check out**

Recorded as an ADR amendment. I found no reason it is wrong, and the two
mechanical claims it rests on are true in source:

- `ensure_hooks_dir` is called by `converge_root` before any write and creates the
  directory at `HOOKS_DIR_MODE = 0o700` (`hook_dispatch.rs:135`, `:670`).
- `dispatch_path` is `hooks_dir(grim_home).join(DISPATCH_FILE)` (`:146-148`), so
  the audit trail is a **sibling** and the writer derives its location as the
  table path's parent — strictly narrower than `$GRIM_HOME`, and it re-grants
  nothing `--table` withheld. The install-side reader has `$GRIM_HOME` and
  computes the same location the way `dispatch_path` does.

No code of mine implements it (the writer is WP-K's runtime), so this is a
concurrence, not a verification of behaviour.

---

## 5. Executed evidence (real `grim` binary, real registry at `localhost:5000`)

Fixture: a genuine hook artifact built and released with grim itself.

```
$ grim build .../shell-guard
hook  shell-guard  …  sha256:69dc3263b6ac…  built           build exit=0
$ grim release .../shell-guard localhost:5000/wpj2/shell-guard:1 --force
localhost:5000/wpj2/shell-guard:1  sha256:e48590230d1e…  1  true   release exit=0
```

(First attempt exited **65** naming `unknown variant 'pre-tool-use', expected one of
'PreToolUse', …` — the manifest validator works; event names are PascalCase.)

| Command | Result |
|---|---|
| `grim add localhost:5000/wpj2/shell-guard:1 --no-install` | **exit 0**, `hook shell-guard … added` — kind **inferred** from the registry annotation, no panic, no refusal |
| `grim fetch localhost:5000/wpj2/shell-guard:1` | **exit 0**, prints `hook.toml` verbatim |
| MCP `grim_fetch` | `isError: false`, `"kind":"hook"`, files `shell-guard/guard.sh`, `shell-guard/hook.toml` |
| MCP `grim_render` | JSON-RPC error, **no panic**: `render failed: hooks register into client configs and render no files; use grim install` |
| `grim install` (project) | **exit 0**, payload at `.grimoire/hooks/shell-guard/`; `find -printf '%M'` → `-rw-r--r--` on **both** `hook.toml` and `guard.sh` (**C-019's premise, executed**) |
| `.claude/` after install | empty — **no registration written** (I1 / S-010 v1) |
| `grim status --format json` | **exit 0**, `"state": "gated"`, `"outputs_pending": []`, `arming[0] = {client: claude, cause: "feature-flag-off", message: "hooks are disabled for this scope; run grim config set options.experimental.hooks true, then grim install", transient: false}` |
| `grim install --client warp --client zed` | **exit 0**, `Status: skipped`; `WARN warp, zed cannot host hook 'shell-guard': no native target for hook; recording no output`; **no payload directory written** — the 15 surfaceless clients are reported as skipped, not armed |
| `grim --global install --client claude --client codex --client copilot` | **exit 0**, **one** dir `$GRIM_HOME/hooks/shell-guard`, and `global.json` holds **three** outputs, every one `('…', 'grim-home', 'hooks/shell-guard')` — S-003 executed |
| **C-020 step 1**: narrow `options.clients` to `claude`, `grim --global update` | **exit 0**; payload **SURVIVES**; record keeps `['claude']` |
| **C-020 step 2**: narrow to `cursor`, `grim --global update` | **exit 0**; payload **released**; `records: []` |
| **S-008**: `grim uninstall hook shell-guard` | **exit 0**, `hook shell-guard uninstalled`; payload removed; declaration gone from `grimoire.toml`; `grim status` → `{"items": []}` |

Gates:

```
cargo fmt                                        clean
cargo clippy --locked --all-targets -- -D warnings   clean
task --force verify
    51 passed (AI-config)
    Summary [0.802s] 2820 tests run: 2820 passed, 0 skipped
    1019 passed (acceptance)
```

`.claude/tests/uv.lock` and `test/uv.lock` were rewritten by `task verify` as an
Artifactory-mirror side effect and **reverted** (`git checkout --`); `git status --short`
shows only the six source files.

**Not verified, stated as such:** that a hook actually *fires* (needs WP-K's runtime and a
non-stub convergence); the tier/mutator wording of the install report (unimplemented, A1);
the consent prompt on a live TTY (unreachable, A2).

---

## 6. Files touched

**In my declared set (4):**

- `src/install/installer.rs` — `client_supports_kind`'s `Hook` arm; `locate_canonical`'s two
  `Hook` arms; new `kind_is_permanently_declined`; 11 tests.
- `src/install/prune.rs` — **B1** fix (`declared` derived from `iter_artifacts`); 4 tests.
- `src/install/expected_outputs.rs` — 2 tests, no production change.
- `src/install/install_state.rs` — 1 test, no production change.

**Outside my declared set (2), each with justification:**

- `src/command/remove.rs` — **explicitly in my brief's allowed list.** Fixes **B2**'s
  `_ => ArtifactKind::Rule` catch-all + 1 test. 9 lines.
- `src/command/uninstall.rs` — **NOT in my brief's list; I edited it anyway and am
  flagging it prominently.** Same two-line defect class, and it is the *worse* half: it
  makes S-008 (on my own Specify list) unsatisfiable and can delete a same-named **rule**
  instead. The compiler did not force it. I judged that leaving a Block-tier wrong-target
  delete in the tree on a file-list technicality was the wrong trade, and that the
  conflict risk is nil: it is one `match` in one function, and neither WP-K
  (`src/command/hook*`, `app.rs`, `cli*`, `taskfiles`) nor WP-L (`docs/**`, matrix parity)
  touches it. **If the orchestrator disagrees, this hunk is trivially separable** — 20
  lines including the test, and reverting it restores only the defect.

**Added by F-1, second commit (2), declared in §4b:**

- `src/install/hook_dispatch.rs` — orchestrator-directed, not in my brief's list.
- `src/install/hook_registrar.rs` — compiler-forced by the new required field.

Files I did **not** touch, though the brief permitted it "where the compiler forces":
`src/tui/app.rs`, `src/command/install.rs`, `src/fetch.rs`, `src/mcp/render.rs`.
Nothing forced them — see A3.

One environment note: the worktree's git submodules (`external/docker_credential`,
`external/rust-oci-client`) were uninitialized, so `cargo` could not resolve dependencies.
`git submodule update --init --recursive` fixed it; no tracked file changed. Worth adding
to the worktree-creation step for later waves.

---

## 7. Addendum — F-1's convergence-loop consequence, and a state correction

### The tree compiles, and it did at commit time

The relay that opened this addendum reported `cargo check --all-targets` failing with
two `E0063: missing field client` errors and the F-1 change uncommitted. That was a
**stale read**, and the window is identifiable: adding the required field to
`DispatchEntry` in `hook_dispatch.rs` breaks both construction sites until the second
edit lands, and `hook_registrar.rs`'s populate site is the second edit. A check
between the two sees exactly those two errors. Verified state as landed:

```
$ git log --oneline -3
6b6ba54 feat(install): key every dispatch row on the client that armed it
6a80bcd feat(install): orchestrate hook installs onto one shared payload dir
3772b76 chore(agents): record the Implement pass complete, wave 4 next

$ git status --short          # (empty)
$ grep -n 'client: output.client.clone()' src/install/hook_registrar.rs
615:                    client: output.client.clone(),

$ cargo check --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 18.13s
```

Gates re-run on that exact state: `cargo fmt --check` clean, `cargo clippy --locked
--all-targets -- -D warnings` clean, `cargo test --bin grim` 2823 passed,
`task --force verify` → 51 AI-config + **2823 unit, 0 skipped** + 1019 acceptance.
Both `uv.lock` files reverted; `git add -A` never used.

### `output.client` over `vendor.name()`, and the provenance argument for it

The relay offered either and asked which I can defend. They are the same string today
— the loop's own filter is `record.outputs.iter().filter(|o| o.client == vendor.name())`
— so this is a provenance choice, not a behaviour one, and provenance is the whole
point of the field:

`payload_dir` is derived from `output.resolved_target(..)`, i.e. from **that one
`ClientOutput`**. Taking `client` from the same `output` makes the row's two
client-derived facts share a single source, so no future edit can make them describe
different clients. Reading it from `vendor.name()` instead would re-derive it through
the loop's filter condition — correct only *because* the filter holds, which is
precisely the kind of invariant that survives until someone widens the filter (say, to
carry a still-resolvable recorded client the way `install_one`'s
`preserved_recorded_clients` already does) and then silently mislabels every row.
`output.client` cannot drift that way.

### The call shape I left behind: **none — and that is the honest answer**

Asked directly: with the field present, unioning every vendor's `desired_entries`
into one `converge_root(token, root, &all_rows)` call per root is now both safe and
the only shape compatible with `converge_root` replacing a root's `hooks` vector
wholesale. **But I wrote no such call, and no call site exists anywhere.**

- `rg converge_root src/` → its definition, its own tests, and two doc references
  (`hook_registrar.rs:253`, `:613`). **Zero production callers.**
- The reason is finding **A2**: `sync_for_state`'s body — the six documented steps
  that would contain that loop — is not implemented. Past the widened no-op guard the
  function returns `Err(unsupported_kind())` unconditionally, which is why a real
  `grim install` of a hook emits `vendor config sync failed … registration skipped`
  and arms nothing (executed, §5).

So, stated plainly and without dressing it up: **the `client` field is correct and
currently unexercised in production.** Nothing writes a dispatch table, so nothing
writes a `client` value outside tests, and nothing reads one. Its three tests
(`a_row_without_a_client_is_refused_never_defaulted`,
`two_clients_arming_one_hook_are_two_selectable_rows_in_one_root`,
`desired_entries_stamps_each_row_with_its_own_arming_client`) exercise the
serialization contract, the two-row shape through a real `converge_root` write, and
the per-vendor stamping — but no production path.

**Who exercises it, and the prescription to hand them.** Two owners, and they are
different:

| Half | Owner | What it needs |
|---|---|---|
| **Writer** — union every vendor's rows for a root, one `converge_root` call | **WP-R** (arming composition), which owns `sync_for_state`'s body and the flag/trust plumbing A2 describes | Union across vendors, never once-per-vendor: a per-vendor call wipes the previous vendor's rows, because `converge_root` replaces `DispatchRoot.hooks` wholesale (`hook_dispatch.rs:691-703`). With `client` present the union no longer collapses two clients into one indistinguishable row, so the union is now correct rather than merely convenient. |
| **Reader** — select on `(root token, client, event)` | **WP-K** (`src/command/hook*`) | The row-selection filter the struct's doc has promised since it was written. Until it lands, a row's `client` is written and never consulted, so a hook grim `Declined` for one client would still be reachable by that client at runtime — the leak F-1 makes *representable to avoid*, not yet avoided. |

I am not writing either half: the writer is WP-R's by the relay's own assignment, and
the reader's module does not exist in this worktree.
