# WP-H — Stub phase report

**Worktree:** `.agents/worktrees/wp-h` · **Branch:** `hex/hooks-artifact-kind--wp-h` · base `9c82115`
**Phase:** Stub · **Status:** complete, uncommitted, not pushed
**Scope:** C-001 (build/add/publish), C-016(b), C-017 (status token + the four `not-armed`
refusal causes and their distinguishing messages — B1, B2, W1)

---

## Gates

| Gate | Result |
|---|---|
| `cargo check --all-targets` | clean |
| `cargo clippy --locked --all-targets -- -D warnings` | clean |
| `cargo test --bin grim` | **2689 passed, 0 failed, 0 ignored** |
| `cargo fmt` / `cargo fmt --check` | clean |

No `#[allow(dead_code)]` anywhere: five `#[expect(dead_code, reason = …)]` markers, each of which
becomes an `unfulfilled_lint_expectations` failure under `-D warnings` the moment the item is
consumed. Three provisional `expect`s were **removed** during the pass because they were
unfulfilled — the `Display` impls already reference every new enum variant, so the wiring is
compiler-proven rather than marker-asserted for those.

---

## ⛔ Findings — read before the diff

### F-1 (Block, plan defect) — C-017's status token has no owning file

**`src/api/artifact_status.rs`, `src/api/status_report.rs` and `src/api.rs` appear in NO work
package's `Expected files` column.** Verified: `grep -n "src/api" .agents/plans/plan_hooks_artifact_kind.md`
returns zero hits in the parallelization table.

C-017 requires a **`not-armed` state token "additive to the frozen `grim status --format json`
schema and documented as an enum value"**, plus `gated`, plus WP-B's third state, plus a
*distinguishing message per cause*. Every one of those is a type in `src/api/` —
`ArtifactStatus` lives in `artifact_status.rs` and `StatusEntry` in `status_report.rs`, per
`subsystem-cli-api.md`'s rule that report status values are typed enums in that layer.
`src/command/status.rs` alone cannot satisfy the contract: it can compute a state, but it has
nowhere to put it.

This is the same shape of gap WP-A's stub review found for `annotations_for_hook` — an unowned
file that a contract silently depends on.

**What I did, and why:** claimed all three. They are unclaimed by every WP in the plan, so the
merge-contract purpose of the file set is not violated — no parallel worktree can conflict. The
alternative (define the token inside `src/command/status.rs` and leave it unreachable from the
JSON schema) would have shipped a stub the Specify phase cannot write a test against, which
defeats the point of the phase. **Ratification owed:** add these three files to WP-H's row.

### F-2 (Warn, latent correctness) — `ArtifactStatus` was `rename_all = "lowercase"`

`NotArmed` under `lowercase` serializes as `"notarmed"`, not `"not-armed"` — the JSON token would
have silently disagreed with `Display` and with C-017's spelling. Switched the enum to
`kebab-case`.

**This moves no shipped token.** All five pre-hook variants are single words, so `lowercase` and
`kebab-case` are byte-identical for `installed` / `stale` / `modified` / `missing` / `outdated`.
Same precedent as `UpdateAction`'s `kept-modified`, which is already kebab-case in that file. Had
this not been caught, the frozen-schema addition would have shipped misspelled.

### F-3 (Warn, plan under-specification) — "four causes, four messages" is not implementable as
stated without a fifth and a sixth

C-017's fold gives four `not-armed` refusal causes. But `grim status` must also distinguish
**`gated`** (three distinct reasons: feature flag off, registry not trusted, client has no hook
surface) and **`untrusted`** (WP-B's Codex `/hooks` state). The plan assigns all of these to WP-H
as "the token and the text" but only enumerates four.

Modelled as **one `HookArmingCause` enum with eight variants and a total `state()` match** that
maps cause → token. This makes the cause→token relation compiler-checked, so the
"generic not-armed" defect cannot reappear *by omission* — adding a cause without deciding its
token is a `cargo check` failure. W3's deferred fifth refusal cause is deliberately **absent**;
an inert literal would be a documented control that enforces nothing.

### F-4 (Warn) — a hook's arming state is per-`(hook, client)`, but `StatusEntry` is per-artifact

A hook can be armed on `claude` and gated on `codex`. A single `state` token cannot say that, and
C-017's "naming the client and the hook" is unsatisfiable from a per-artifact row alone.

Added `arming: Vec<HookArming>` (always-present, `[]` for every non-hook kind), one element per
affected client, and a documented row-state precedence `not-armed > untrusted > gated > lifecycle`
in `hook_row_state`. `[]` therefore means "nothing to report", never "unknown".

### F-5 (Note) — plain-output shape change, permitted but worth a reviewer's eye

`grim status`'s plain table goes **5 → 6 columns** (`Note`). Permitted: `docs/src/stability.md`
§ Unstable freezes "only exit codes and structured JSON output", explicitly excluding
"human-readable log or error text". Without it, plain output shows `not-armed` with no way to tell
*which* refusal — the human half of C-017 would be unmet. The cell carries the short cause token;
the full remedy sentence goes to stderr via `tracing` (`warn` for not-armed/untrusted, `debug` for
gated), matching the repo's standing tables-on-stdout / guidance-through-tracing split.

`test/recordings/cast_recorder.py:110` carries a comment measuring the 5-column width (~147 cols);
cosmetic, but WP-O should re-measure.

### F-6 (Note) — `publish.toml` gains a `[hooks]` table

WP-H's Specify line requires `grim publish` for the kind, which requires the table.
`#[serde(default)]`, appended after `mcp`, published **before** `bundles` (a bundle pinning a hook
member needs it pushed first — same correctness assumption skills/rules/agents already rely on).
Additive under the manifest-widening rule. Side effect: `grim schema --kind publish` gains the key
for free (generated from the parse struct). Flagging because it touches a frozen input schema, not
because it is a defect.

---

## Decisions recorded (the plan asked for these explicitly)

### D-1 — A hook is **not** dev-installable from a path in v1 (`command/install.rs`)

Three independent reasons, any one sufficient:

1. **Consent is unexpressible.** Per-registry `trust_hooks` (C-022) is the only consent surface
   for arming; a path source has no registry to carry it. Dev-installing would arm code with
   consent expressed nowhere — **invariant I4** forbids it.
2. **It would put an armable payload inside a repository.** A dev install's source is a
   working-tree path, and the natural loop is edit-in-repo → re-install: exactly the repo-resident
   arming **invariant I1** exists to prevent.
3. Allowing it later is additive (a new accepted `--kind` value); withdrawing it never is.

Reachability today is nil (dev-install's `--kind` parser is `["skill","rule","agent"]`, shape
inference never yields `Hook`), so the arm **refuses** (DataError/65) rather than
`unreachable!()` — defence in depth against a future widening. Consequences propagated to
`update.rs`'s `refresh_dev_installs`, `skill/local_pack.rs`, the TUI dev action, and the dev row's
`arming: Vec::new()` in `status.rs`, each restating the decision rather than the placeholder
rationale.

Authoring loop for a hook author: `grim build` → `grim release` to a local registry → `grim add`.

### D-2 — The TUI does **not** install or update a hook row in v1; it **does** uninstall one

`perform` (install/update) refuses. Arming needs an expressed consent and both surfaces that
express it are files (`[options.experimental] hooks`, `trust_hooks`); neither is something the TUI
asks for, and adding a third consent surface on the keystroke path for the one kind that executes
code is not something any artifact in this plan designed. **I4**: new execution capability ships
off by default; widening later is additive.

`perform_uninstall` **allows** hooks — it only ever reduces capability, and refusing would leave a
TUI user staring at an armed hook with no way to disarm from the surface they are in.

**Explicitly not the reason:** the refusal is *not* what keeps the gates honest. A TUI install
would reach the same installer seam the CLI does and be gated identically. What it avoids is
offering an arming action through a surface that cannot show the user what they are consenting to.

### D-3 — `grim fetch` supports hooks; `grim_render` (MCP) refuses them

Fetch is use-not-install: it prints bytes, arms nothing, writes nothing. Reading a published
hook's `hook.toml` and handler scripts **before** consenting to arm them is the most valuable thing
a user can do with the kind, so refusing would remove a review surface for zero safety. No
feature-flag gate for the same reason — a `gated` hook is precisely the one you want to inspect.
Index path is the skill shape (`<name>/hook.toml`), and `--vendor` returns canonical bytes
(`project_index` → `None`) because a hook's vendor-specific artifact is a *registration*, not a
projected file.

`grim_render` refuses with a kind-specific message. It writes to a caller-chosen `dest_dir`, so
even if a renderable file existed, honouring it would let an MCP client name a destination and have
grim arm code there — repo-resident arming through grim's own write tool.

### D-4 — Hook binding names join the `SkillName::parse` guard in `grim add`

A hook's payload directory is named by its **binding**, not its manifest `name`.
`HookManifest::validate` constrains the manifest field at `grim build`, on the *publisher's*
machine, and can never reach a binding key the consumer writes into their own `grimoire.toml`. This
is the unvalidated-binding-name class the plan names as owed defence-in-depth (the same chain
behind #56's T3 finding), closed here for the one kind where the path is armable.

Reserved-name rejection (`bin`, `dispatch.json` — `RESERVED_ARTIFACT_NAMES`) is *not* closed here:
both pass `SkillName` cleanly, and a payload materialized over either arms or disarms the
dispatcher itself. Documented in-code as owed to the install seam — **WP-I / WP-J2, flagged.**

---

## C-016(b) — the two production silent sites

| Site | Before | After |
|---|---|---|
| `status.rs` `collect_declared` | 4-element hand-maintained array | `hooks` appended last (pre-hook row order byte-stable) + a doc note that this is one of C-016(b)'s two sites and that C-016(b) does **not** protect an array added later |
| `release.rs:208,211` | two independent `if kind == …` checks | **converted to a total `match`** — a kind needing its own release path is now a compile error, not a silent route down the shared path. `Hook` takes the shared path deliberately (it is a directory tree in one tar layer; `Bundle`/`Mcp` diverge because their layers are not) |

The `release.rs` conversion is the substantive one: C-016(b) asked for a *test* per site, and a
compiler-forced site is strictly stronger than a test at the same cost.

## D-4 (plan finding) — all 8 `tui/app.rs` sites visited

| # | Site | Disposition |
|---|---|---|
| 1 | `perform_uninstall` gate | gate **removed** — hooks allowed (D-2) |
| 2 | `perform_local_uninstall` gate | refuses — a hook has no path source; live refusal not `unreachable!()` because `row_kind` reads a registry-controlled string and I3 forbids a panic here |
| 3 | `perform` gate | refuses (D-2), with the full rationale in-code |
| 4 | `perform_local` gate | refuses — same chain as #2 |
| 5 | `declared_as_path` match | `&set.hooks` — the honest map probe (always `false` today) beats a panic guarding a question the data answers |
| 6 | `perform_local_dev` synth match | refuses (D-1) |
| 7 | `direct_declared_repos` array (silent) | `Hook` appended. `Bundle`'s absence documented as **pre-existing and correct** (a bundle is never a member) |
| 8 | `local_rows` path-declared array (silent) | visited and deliberately **not** extended — every `source.path()` in `set.hooks` is `None`, so a row would read like support and provide none. `Mcp`'s pre-existing absence **flagged, not fixed** (D-5: no test would catch a drive-by change in a kind-dispatch array) |

`row_kind` needed no edit — it resolves through `ArtifactKind::from_kind_str`, which WP-A made
total, so `"hook"` maps for free.

---

## `unimplemented!()` inventory (what Specify writes tests against, what Implement fills)

| Location | Owed |
|---|---|
| `api/artifact_status.rs` `HookArmingCause::message` | per-cause remedy text; Specify asserts **distinctness across `ALL`** |
| `api/artifact_status.rs` `HookArmingCause::transient` | true only for `DispatchLockHeld` |
| `command/status.rs` `hook_arming` | per-client verdicts, reading the same inputs the registrar decides on (never a cached copy of its verdict) |
| `command/status.rs` `hook_row_state` | the documented precedence |
| `command/status.rs` `warn_unarmed` | severity-split stderr diagnostics |
| `command/build.rs` `pack_hook_dir` | `hook.toml` read → `validate` → pack → annotate; validate **before** pack |
| `oci/annotations.rs` `annotations_for_hook` | the annotation map |

`annotations_for_hook` is deliberately narrower than its five siblings: `hook.toml` has four
fields and `deny_unknown_fields`, so there is no `summary` / `keywords` / `repository` /
`deprecated` to lift. That is a statement about the manifest, not an omission — inventing an
annotation with no authored source would publish a field no author can set. Nothing from a
`HookEntry` reaches it either, which keeps the published catalog row independent of how many
handlers a hook declares.

---

## Principle 9 audit of this diff

| Change | Direction |
|---|---|
| `ArtifactStatus` + `Gated`, `NotArmed`, `Untrusted` | additive enum literals; no literal removed or re-spelled |
| `ArtifactStatus` `lowercase` → `kebab-case` | **byte-identical** for all five shipped tokens (single words) |
| `StatusEntry` + `arming` | additive, **always-present** (`[]`), no `skip_serializing_if` |
| `PublishManifest` + `hooks` | additive optional table (`#[serde(default)]`); a pre-hook `publish.toml` parses unchanged |
| `--kind` value sets (`add`, `build`, `remove`, `uninstall`, `release`) | `"hook"` **appended**, never inserted; no accepted value removed |
| `publish` kind order | `hooks` inserted **between** `mcp` and `bundles`; no existing kind's relative position moves |
| `grim status` plain table 5 → 6 columns | permitted — plain output is explicitly Unstable |
| `single_entry_lock` 4-tuple → 5-tuple | private fn, no external surface |

No `skip_serializing_if` in `src/api/`. Multi-item `{"items": […]}` envelope untouched. Every
report value derives from an operation result, never from a CLI arg.

---

## Not done / owed elsewhere

- **`RESERVED_ARTIFACT_NAMES` at the binding level** — flagged in-code, owed to the install seam
  (WP-I / WP-J2). `bin` and `dispatch.json` pass `SkillName::parse`.
- **`row_kind_maps_every_catalog_kind`** does not yet assert `"hook"` — Specify's line.
- **`test/recordings/cast_recorder.py:110`** column-width comment — WP-O.
- **WP-M docs** owed by this diff: the `not-armed` / `gated` / `untrusted` enum values and the
  `arming` field in `json-interface.md`; `publish.toml`'s `[hooks]` table in `publishing.md`; the
  6th `status` column and the widened `--kind` value sets in `commands.md`; **and D-1/D-2/D-3 as
  user-visible limits** (hooks are registry-only, the TUI does not install them, `grim_render`
  refuses them) — none of these are in WP-M's current obligation list.
