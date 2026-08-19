# WP-H — Implement phase report

**Worktree:** `.agents/worktrees/impl-h` · **Branch:** `hex/hooks-artifact-kind--impl-h` · base `0387aa7`
**Phase:** Implement · **Status:** complete, uncommitted, not pushed
**Scope:** C-001 (build/add/publish), C-016(b), C-017 (status token + the eight `HookArmingCause`
variants, their distinguishing messages, and the row-state precedence)

> **Process note, mine to own.** This report was first written to the repo-root
> `.agents/wp-h-implement-report.md` — the main checkout's `.agents/`, not this branch's — so it was
> invisible to a reviewer looking at the worktree, and the lead had to ask for it twice. It now lives
> at `.agents/wp-h-implement-report.md` **inside the worktree**, which is the tracked, mergeable
> location beside `wp-h-stub-report.md`. The stray root copy was deleted; there is exactly one.

---

## ⚠ Read first — `grim status` panics today on a hand-written `[hooks]` table

**The lead's inference that neither stubbed seam is reachable is wrong, and I verified it by
execution rather than by reading.** No install seam is involved. `grim status` reads config, and a
`[hooks]` table parses straight into `DesiredSet::hooks` (`src/config/declaration.rs:484`), which is
all `declares_a_hook` needs:

```
$ cat grimoire.toml
[hooks]
guard = "ghcr.io/acme/guard:1.0.0"

$ grim status --format json
thread 'main' panicked at src/install/hook_registrar.rs:327:5:
not implemented: WP-I: re-derive C-017 causes 1-3 read-only (WP-H's status seam)
EXIT=101
```

`grim build ./<dir-with-hook.toml>` is reachable the same way — shape detection yields `Hook` with no
`--kind` flag at all, and it panics at `src/oci/hook.rs:411` (WP-A's `from_toml_str`).

**This is pre-existing at the stub state, not introduced by this diff.** On base `0387aa7`
`hook_arming` was itself `unimplemented!()` and was called per declared row, so the same config
panicked there. What my change does is **relocate** the panic from WP-H's stub into WP-I's and WP-A's.

| Seam | Reachable today? | Path |
|---|---|---|
| `hook_registrar::arming_refusal` (`status.rs:795`) | **yes — executed above** | any `[hooks]` entry in `grimoire.toml`, or a locked hook; no flag, no feature toggle, no install |
| `trust::decide` (`status.rs:674`) | **no, but only because the line above panics first** | `arming_refusal` runs during `HookArmingInputs::resolve`, before any row loop, so it pre-empts every later call site. It needs the feature flag on **and** a lock pin, and becomes reachable the moment WP-I's body lands |
| `HookManifest::{from_toml_str,validate}` (`build.rs`) | **yes** | `grim build <dir containing hook.toml>` |

**What this means for merge order:** WP-I's and WP-A's Implements must land **in or before the same
merge** as mine, or `grim status` is a panicking released command for anyone with a `[hooks]` table.
It does not change what my declared-set code does — it changes what is safe to ship without the other
two. Not calling those seams was not an option: the alternatives were re-deriving C-022's B4/B5
precedence inside `status.rs` (a second spelling of a security predicate) or reporting an unarmed hook
as armed. **WP-O should add the acceptance test that would have caught this** — `grim status` on a
declared-but-uninstalled hook, asserting exit 0.

---

## Gates

| Gate | Result |
|---|---|
| `cargo check --all-targets` | clean |
| `cargo clippy --locked --all-targets -- -D warnings` | clean |
| `cargo test --bin grim` | **2701 passed, 0 failed, 0 ignored** (2689 at stub + 12 new) |
| `cargo fmt` / `cargo fmt --check` | clean |
| **`task --force verify`** | **PASSED** — shell:verify, claude:tests, rust:format:check, rust:clippy:check, claude:lint:links, rust:build, catalog:verify, rust:test:unit (2701), acceptance **1019 passed in 19.22 s**; the `commit-verified` mark was written, which only happens when every task in the chain succeeds |

Ran twice with `--force` (cache bypassed). No test file was edited — in particular
`test/tests/test_status.py`'s 14-field tripwire is untouched and passes, because this diff adds no
new status-item field (`arming` landed with the stub).

---

## Every `unimplemented!()` in the file set is filled

| Location | What landed |
|---|---|
| `api/artifact_status.rs` `HookArmingCause::message` | eight distinct remedy strings; three unit tests assert distinctness, that every actionable cause names a command, and that Display/Serialize agree |
| `api/artifact_status.rs` `HookArmingCause::transient` | total match, `true` only for `DispatchLockHeld` |
| `command/status.rs` `hook_arming` | per-client verdicts over the registrar's and the trust gate's **own** seams |
| `command/status.rs` `hook_row_state` | `min_by_key` over one severity ladder |
| `command/status.rs` `warn_unarmed` | one line per verdict, `warn` for not-armed/untrusted, `debug` for gated |
| `command/build.rs` `pack_hook_dir` | `hook.toml` read → `from_toml_str` → `validate` → `pack_skill_dir` → `annotations_for_hook`; validate strictly before pack |
| `oci/annotations.rs` `annotations_for_hook` | title/description/version/kind + git-or-fallback source; no invented catalog keys |

Zero `unimplemented!()` left in the WP-H set (`grep` over all 18 files returns only three prose
comments). Nothing that reports a value echoes a CLI argument: `pack_hook_dir` reports
`manifest.name` (which `validate` has just proven equals the directory stem), and every `arming`
element is derived from a seam's return value.

---

## ⛔ Findings — read before the diff

### F-1 (Block, merged code) — `client_supports_kind` answers `true` for `Hook` on all 18 clients

`installer.rs:1106-1122`'s catch-all is `kind_support(kind) != Declined && kind_surface(kind, scope)`,
and no vendor overrides `kind_support` for `Hook`. So today it reports Warp and Zed — the two clients
with **no hook mechanism at all** — as hook-capable. Confirmed by reading the arm, and it is exactly
what `vendor.rs:706-716` warns WP-J2 about from the other direction.

Consequence for me: `grim status` could not use that function, or it would have reported every
surfaceless client as armed — the silent-guardrail shape C-017 exists to close. The predicate is
therefore spelled once in `HookArmingInputs::client_has_hook_surface` from the two authoritative
seams (`hook_surface().is_some() && kind_surface(Hook, scope)`), with an in-code **collapse
obligation**: delete it and call `client_supports_kind` in the same change that adds WP-J2's arm.
**I did not touch any install-path refusal.** Flagged rather than silently duplicated.

### F-2 (Block, file-set overrun — CONFIRMED as mechanically forced, ratification owed)

**Confirming the lead's reading, which is exactly right.** I edited three files outside my declared
set. Every edit is the deletion of a `dead_code` lint attribute, and each was **mechanically forced,
not optional**:

- the attributes are `#[expect(dead_code)]`, **not** `#[allow(dead_code)]`;
- wiring a caller makes the item live, which **unfulfills** the expectation;
- `unfulfilled_lint_expectations` is a warning, and the gate is `clippy --all-targets -- -D warnings`,
  so it is a **hard build failure**;
- each attribute's own text already named this as its trigger — e.g. "REMOVAL TRIGGER: delete this
  attribute when the first of those call sites lands". Mine is that call site.

There was no third option. Keeping the attribute fails the build; not consuming the seam means
re-deriving C-022's precedence table inside `status.rs` (F-3) or reporting an unarmed hook as armed.
I reproduced the failure before deleting anything, so this is observed, not assumed.

| File | Item | Edit |
|---|---|---|
| `src/config/declaration.rs` | `ExperimentalOptions::hooks_enabled` (WP-E's) | attribute deleted |
| `src/hook/trust.rs` | `LocatorKind` (WP-G's) | attribute deleted |
| `src/hook/trust.rs` | `decide` (WP-G's) | attribute deleted |
| `src/install/hook_registrar.rs` | `arming_refusal` (WP-I's) | attribute deleted |
| `src/install/hook_registrar.rs` | `ArmRefusal` (WP-I's) | attribute **kept**, reason text refined |

**Nothing else in those three files changed** — no signature, no body, no doc beyond the one reason
string. `git diff` on all three is 25 lines, 23 of them deletions.

**Concurrency, per the lead's context:** WP-G and WP-I are removing the same attributes as their
bodies land, so my versions are redundant at merge. **Not reverting them** — the lead resolves at
merge time by letting the owning package's version win, and my declared-set code compiles against
that result unchanged (it depends on the *items*, never on the attributes). Recorded here so the
merge record is honest and no reviewer treats these as scope creep.

**`ArmRefusal`'s attribute is retained** with a narrowed reason, and that one is *not* redundant with
WP-I's work in the same way: its two `GrimHome*` variants are constructed only by
`validate_grim_home`, whose body is still a stub, so they are dead in the bin profile. Deleting it
needs WP-I's body. That also forced one test compromise, recorded in-code: my refusal→cause test
asserts only `DispatchLocked`, because naming a `GrimHome*` variant makes it live in the test profile
and dead in the bin profile — a lint expectation satisfiable in neither state. **No
`#[allow(dead_code)]` was added anywhere**, so the stub report's "no `allow` in this tree" property
survives.

| File | Item | Note |
|---|---|---|
| `src/hook/trust.rs` | `LocatorKind` | WP-G's; its own removal trigger says "the first of those call sites lands" — this is that call site |
| `src/hook/trust.rs` | `decide` | same |
| `src/install/hook_registrar.rs` | `arming_refusal` | WP-I's; same stated trigger |
| `src/config/declaration.rs` | `ExperimentalOptions::hooks_enabled` | WP-E's; same stated trigger |
| `src/install/hook_registrar.rs` | `ArmRefusal` (enum) | **kept, reason rewritten** — see below |

Each deletion is exactly the lines the attribute occupies and each is one impl-g/impl-i will make
too, so identical deletions merge cleanly. **`ArmRefusal`'s attribute is retained** with a narrowed
reason: its two `GrimHome*` variants are constructed only by WP-I's still-stubbed
`validate_grim_home`, so they are dead in the bin profile. Deleting it needs WP-I's body. That also
forced one test compromise, recorded in-code: my refusal→cause test asserts only `DispatchLocked`,
because naming a `GrimHome*` variant would make it live in the test profile and dead in the bin
profile — a lint expectation satisfiable in neither state. **No `#[allow(dead_code)]` was added
anywhere.**

### F-3 (Warn, contract gap) — nobody owns a composed "is this registry trusted for hooks" seam

WP-G ships `decide(registry, repository, &[AuthoredRegistry])` — pure, and correctly refusing a
`&[ResolvedRegistry]`. But **nothing builds the `AuthoredRegistry` view**: WP-I's `desired_entries`
takes the predicate as an injected `&dyn Fn`, so the "read both config scopes and tag each entry
with the scope it was authored in" step is unowned. I built it in
`HookArmingInputs::{resolve,authored}`, routed through `super::global_config_tiers` so the
source-level one-loader pin (`command.rs`'s `the_global_config_is_loaded_from_exactly_one_seam_ws2`)
still holds.

**WP-J2 will need the same view for the install path.** It should reuse this one (or promote it),
not write a second — two spellings of B4's precedence table is precisely how a global `true` comes to
shadow a project `false`.

### F-4 (Warn) — the status path now reads the global config, so it is gated on a hook existing

`global_config_tiers` compiles every browse-filter glob (its own doc records 12.6 s → 21.5 s for a
second load on a 60-entry config). `HookArmingInputs` is therefore built **only** when
`declares_a_hook` is true, which checks the `[hooks]` table **and** the lock (a bundle-provided hook
appears in no config table; omitting the lock half would report every bundle hook as armed). A
malformed global config now surfaces as 78 from `grim status` on a hook-declaring project where it
previously would not — deliberate: swallowing it would drop every globally-authored `trust_hooks`
grant and report a trusted hook as untrusted.

### F-5 (Note) — no `Vendor` seam for "this client needs an out-of-band approval"

`ClientTrustPending` is Codex-only and grim cannot observe the `/hooks` approval, so `untrusted` is
the honest standing report for a codex hook row. There is no trait method behind that, and
`vendor.rs` is outside my set, so `requires_client_approval` is a **total match over all 18
`ClientTarget` variants** in `status.rs` — no wildcard, so promoting a client to a hook surface has
to answer this too. Owed: `Vendor::hook_approval()`, the same debt `sync_for_state` already records
for `hook_config_path`. A unit test asserts Codex is the only `true`.

---

## Decisions made during Implement

### D-A — cause 4 is constructed by a total map and filtered by a **rule**, not by a special case

The lead's instruction was to build no status path for `DispatchLocked`. Mapping it to `None` inside
`cause_from_refusal` would have done that — but it would also have left `HookArmingCause::DispatchLockHeld`
constructed nowhere (a `dead_code` warning, and an inert literal the plan explicitly rejects).

So: `cause_from_refusal` is **total and returns a plain cause** (a refusal cause added without
deciding its reported cause is a `cargo check` failure), and `hook_arming` filters with
`.filter(|c| !c.transient())` — *`grim status` never reports a transient cause, because a transient
answer is stale the moment it is printed.* That is a general rule with a general justification, it
filters exactly `DispatchLockHeld` today, it keeps `transient()` genuinely exercised, and a future
transient cause is filtered with no code change. **No status probe for a held lock exists and none
is possible**; `arming_refusal` structurally never returns it.

Consequence, documented in both the field doc and the module doc: `arming[].transient` reads `false`
on every status row. That is a true constant derived from a stated rule, not a permanently-null
field — it is kept so a consumer branching on `cause` never needs its own copy of the table.

### D-B — verdict order is decreasing **specificity**, not I4-first

Per client: `ClientHasNoHookSurface` → `FeatureFlagOff` → `RegistryNotTrusted` → refusal(1–3) →
`ClientTrustPending` → armed. The surface check is first even though I4's default-deny is answered
first when *arming*, because when *reporting* the useful cause is the one the user can act on:
pointing a Warp user at a feature flag names a knob that changes nothing for them. All three of
`ClientHasNoHookSurface`/`FeatureFlagOff`/`RegistryNotTrusted` are `gated`, so the row token is
unaffected either way — only the per-client detail moves.

### D-C — `OptedOut` and `NeedsConsent` collapse to one reported cause

Both mean "this registry has not been trusted for hooks" and both have the same remedy line. The
`--allow-hooks` / TTY composition (`trust::arming`) is deliberately **not** consulted: `grim status`
has no such flag and is not the invocation being judged, so modelling a per-invocation escape here
would report a state no status run can produce.

### D-D — a hook with no lock pin gets no trust verdict

C-022 keys on the **resolved** registry and repository (B5.4). An unlocked row has neither, so there
is no subject to answer about; its lifecycle state already reads `stale`/`missing`, which is the real
problem. `pinned` is now computed before `arming` in both loops for this reason.

### D-E — one severity ladder, not three total matches

Precedence, the warn/debug split, and "is this even an arming state" are one question asked three
ways. `ArmingSeverity` (`Refused < ClientWithheld < Gated < NotAnArmingState`, derived `Ord`) is the
single total match over `ArtifactStatus`. A lifecycle token sorts **last** and logs at `debug`, so a
future cause mis-mapped to one could never outrank a real refusal or masquerade as a warning.

### D-F — `HookArmingCause::ALL` moved to a `#[cfg(test)]` impl block

Nothing in production needs the whole roster (each call site derives one cause from real inputs), and
a `pub const` under `#[expect(dead_code)]` cannot be satisfied in both profiles once a test reads it.
The compiler-checked half of cause → token is `state()`'s total match; `ALL` only lets a test assert
the *text* is distinct, which no match can express.

### D-G — `pack_hook_dir` reuses `pack_skill_dir`

A hook genuinely is a payload tree in one uncompressed tar layer keyed on the directory name, and
`pack_skill_dir` requires no `SKILL.md` — it walks whatever is there in sorted order. A second
hook-specific walker would be a second set of packing bounds and a second sort order to keep
byte-identical.

---

## Cross-WP dependency — my paths panic until two other Implements merge

Full reachability analysis with executed evidence is in the **⚠ Read first** section at the top; this
is the summary table.

| Seam | Owner | Reached from | Reachable today |
|---|---|---|---|
| `HookManifest::from_toml_str`, `HookManifest::validate` | WP-A | `build::pack_hook_dir` | **yes** — `grim build <hook dir>` |
| `hook_registrar::arming_refusal` | WP-I | `HookArmingInputs::resolve` | **yes** — any `[hooks]` entry, `grim status` |
| `hook::trust::decide` | WP-G | `status::hook_arming` | no — the line above panics first |

Why `task verify` is nonetheless green: no acceptance test declares a hook
(`test/tests/test_hooks_*.py` is WP-O, pending), and every unit test I added exercises only pure code
I own. **That is a coverage gap, not a clean bill of health** — it is precisely why the panic above had
to be found by executing the binary by hand.

---

## Principle 9 audit of this diff

| Change | Direction |
|---|---|
| `HookArmingCause::{message,transient,state}` bodies | behaviour inside items that never shipped |
| `annotations_for_hook` | new builder; no existing annotation key moved, none invented |
| `pack_hook_dir` | new arm on `validate_and_pack`'s already-total match |
| `hook_arming` signature (`target,scope` → `pinned,inputs`) | private fn, no external surface |
| `HookArmingCause::ALL` → `#[cfg(test)]` | test-only item, never on the wire |
| status JSON | **no field added or removed** — `arming` and its four sub-fields landed with the stub; `test_status.py`'s 14-field tripwire untouched and green |
| plain table | still 6 columns (the `Note` column landed with the stub) |
| the five attribute deletions | lint attributes only; no signature, token or layout moves |

No `skip_serializing_if` in `src/api/`. `{"items": […]}` envelope untouched. Exit codes unchanged —
no arming verdict influences the exit code (`grim status` stays 0).

---

## Owed elsewhere (unchanged from the stub report unless marked)

- **Collapse `client_has_hook_surface` into `client_supports_kind`** — WP-J2, in the same change that
  adds the `Hook` arm. **New, and the sharpest item in this list.**
- **`Vendor::hook_approval()`** — delete `requires_client_approval` when `vendor.rs` is next open
  (WP-J2 or WP-F). **New.**
- **Reuse `HookArmingInputs::authored` for the install path** rather than writing a second authored-
  registry view — WP-J2. **New.**
- `ArmRefusal`'s `#[expect(dead_code)]` — WP-I deletes it with `validate_grim_home`'s body. **New.**
- **WP-O: an acceptance test for `grim status` on a declared-but-uninstalled hook**, asserting
  exit 0. **New, and it is the test whose absence let a reachable panic sit unnoticed** through a stub
  gate and an implement gate. A `[hooks]` table plus `grim status` is the whole fixture.
- **Merge order: WP-A's and WP-I's Implements must land in or before the same merge as mine.** **New.**
- **WP-M docs**, now also owed: `arming[].transient` is always `false` in a status report and why;
  `grim status` on a hook-declaring project can exit 78 on a malformed **global** config; the eight
  cause tokens as documented enum values (the stub report already listed the three state tokens).
- `RESERVED_ARTIFACT_NAMES` at the binding level — WP-I / WP-J2, unchanged.
- `row_kind_maps_every_catalog_kind` asserting `"hook"` — unchanged.
- `test/recordings/cast_recorder.py:110` column width — WP-O, unchanged.
