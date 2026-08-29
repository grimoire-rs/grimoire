# WP-P0 propagation sweep — folding the format audit into the plan's contracts

**Date:** 2026-08-17 · **Author:** propagation sweep · **Source of truth:**
[`security_audit_hooks_formats.md`](./security_audit_hooks_formats.md) (8 Block · 9 Warn · 3 Suggest)
**File edited:** `.agents/plans/plan_hooks_artifact_kind.md` — **the only file touched.** Nothing
committed. No source code written.

**Why this sweep exists:** the plan has twice been damaged by a fix pass that amended one layer and
left the others standing. Under contract-first TDD the `**Specify:**` lines are **test source**, so
an amendment reaching contract prose but not the Specify line is **unarmed** — and a table asserting
a Specify item that does not exist is worse than silence. Every folded finding below was therefore
chased through all eight layers.

## The eight layers

| # | Layer |
|---|---|
| **L1** | Named contract text — the "Amendments to ADR contracts" table + the dedicated contract paragraphs (C-006, C-007, C-008, C-017, C-018b, C-022, C-023, C-026) |
| **L2** | § Launcher narrative + § The registration table |
| **L3** | Parallelization table rows — `Scope (C-/S- IDs)` **and** `Expected files` |
| **L4** | `### WP-G` / `### WP-I` — Stub/Implement bullets **and** `Specify:` lines |
| **L5** | `### WP-E` — marked **post-hoc** (WP-E is running now) |
| **L6** | Threat-model / risk register |
| **L7** | Open Questions |
| **L8** | Constitution Deviations |

Two further layers were swept even though they were not in the brief, because findings landed there:
**L9 = other WP sections** (WP-H, WP-K, WP-M, WP-O) and **L10 = UX scenarios** (S-011) — both are
Specify-generating surfaces, so leaving them stale would have been the same failure mode.

## Finding × layer matrix

Legend: **✔** touched · **–** not applicable · **(def)** recorded as a clearly-labelled deferred item
rather than folded-and-armed.

| Finding | Attacker · invariant | L1 contract | L2 launcher/table | L3 parallel rows | L4 WP-G/WP-I | L5 WP-E | L6 threat/risk | L7 open Q | L8 deviations | L9 other WPs | L10 scenarios |
|---|---|---|---|---|---|---|---|---|---|---|---|
| **B1** table path is env-derived at runtime | T3 (esc. T4) · I1, I4 | ✔ C-006 (new ¶ + row), C-007 (new row), C-008 row, C-017 refusal table | ✔ box + Form cells | ✔ WP-I, WP-K, WP-H | ✔ WP-I stub+specify | – | ✔ new risk row | ✔ **Q4** (`--table` vs `--home`) | ✔ *no new deviation* | ✔ WP-K (import test), WP-H (token/UX), WP-M (file-structure rule), WP-O (case 1) | ✔ S-011 |
| **B2** double-quoted assignment expands | T3 · I1, I6 | ✔ C-008 row, C-018b widened, C-017 cause 3 | ✔ box + Form cells | ✔ WP-I, WP-H | ✔ WP-I stub+specify | – | ✔ new risk row | – | ✔ *no new deviation* | ✔ WP-H (UX) | – |
| **B3** root key guessable | T3 fire, T4 profit · I1, I4 | ✔ C-006 ¶, C-007 row, C-008 row | ✔ box + Form cells + the corrected "second attack" ¶ | ✔ WP-I, WP-K | ✔ WP-I stub+specify | – | ✔ **row 14 corrected** | ✔ **Q5** (random vs HMAC), Q1 reasoning corrected | ✔ *no new deviation* | ✔ WP-K (unknown token ⇒ exit 0), WP-O (case 2) | ✔ S-011 |
| **B4** which scope grants trust | T3 · I1, I4 | ✔ C-022 (precedence table verbatim) | – | ✔ WP-G, WP-M | ✔ WP-G stub+specify | – | ✔ **row 15 new** | – | ✔ *no new deviation* | ✔ WP-M (docs), WP-O (case 3) | ✔ S-011 |
| **B5** trust identity granularity | T1, T2 · I4, I2 | ✔ C-022 | – | ✔ WP-G, WP-M | ✔ WP-G stub+specify | – | ✔ **row 16 new** | – | ✔ *no new deviation* | ✔ WP-M (docs) | – |
| **B6** `GRIM_ALLOW_HOOKS` two ways | T3 · I4 | ✔ C-022, C-023 | – | ✔ WP-G, WP-M | ✔ WP-G stub+specify | ✔ inertness-test note | ✔ existing row **upgraded to a Block** | ✔ **Q6** (delete vs read-and-ignore) | ✔ *removes* a potential deviation | ✔ WP-M (**`AGENTS.md` env table obligation**) | – |
| **B7** `trust_hooks` bool drops `false` | T1 · I4, I5 | ✔ C-022 | – | ✔ **WP-E (+`src/command/add.rs`)**, WP-H, WP-M | ✔ WP-G consumes, does not define | ✔ **post-hoc block, 3 additions** | ✔ **row 17 new** | – | ✔ `RegistryField::ALL` append is additive, not a deviation | ✔ WP-M (field count 6→7), WP-O (case 5) | – |
| **B8** guard admits failing-exec states | none required; T4 can induce · I3 | ✔ C-008 row, C-006 ¶ (none), C-017 | ✔ box + Form cells | ✔ WP-I, WP-K (fork cost) | ✔ WP-I stub+specify; WP-B verdict item 1 marked **superseded** | – | ✔ new risk row | – | ✔ *no new deviation* | ✔ WP-K (latency), WP-O (case 4) | – |
| **W1** no lock on the dispatch write | none / **T5** shared `$GRIM_HOME` · I3 | ✔ C-006 ¶ (3), C-017 cause 4 | ✔ box (via C-006 pointer) | ✔ WP-I, WP-H | ✔ WP-I stub+specify | – | ✔ new risk row | – | ✔ *no new deviation* | ✔ WP-H (transient message) | – |
| **W2** `schema` + defensive parsing | T3 while B1 stands · I3 | ✔ C-006 ¶ (4) | – | ✔ WP-I, WP-K | ✔ WP-I specify | – | – | – | ✔ *no new deviation* | ✔ WP-K (runtime half) | – |
| **W4** `approved digest` gates nothing | none — a false claim · I5 | ✔ C-006 ¶ (5) + **ADR A6 owed** (C-011 control 7) | – | ✔ WP-I | ✔ WP-I stub | – | – | – | ✔ *no new deviation* | ✔ WP-K (C-011 Specify must not test it) | – |
| **W5** "no TTY" + prompt channel | none · I3 | ✔ C-023 | – | ✔ WP-G | ✔ WP-G specify | – | – | – | ✔ *no new deviation* | – | – |
| **W6** `GRIM_EXPERIMENTAL_HOOKS` repo-carried | T3 · I4 | ✔ C-026 | – | ✔ WP-G | ✔ WP-G specify | – | – | ✔ rides with **Q6** | ✔ *no new deviation* | – | – |
| **W8** `insecure = true` implicitly trusted | T2 · I2, I4 | ✔ C-022 | – | ✔ WP-G | ✔ WP-G stub+specify | – | – | – | ✔ *no new deviation* | – | – |
| **W3** shared `$GRIM_HOME` (def) | T5 · I1, I5 | ✔ C-017 cause 5 marked deferred | – | ✔ WP-I *deferred* | ✔ WP-I + WP-G deferred lists | – | – | – | – | ✔ WP-M (`0o700`/`0o600` if it lands) | – |
| **W7** prompt writes a key older grims reject (def) | none · Principle 9 | – | – | ✔ WP-G *deferred* | ✔ WP-G deferred list | – | – | – | ✔ **row 1 gains a second trigger** | ✔ WP-M (`stability.md`) | – |
| **W9** shim's `$PATH` fallback (def) | T3, conditional · I1 | ✔ C-008 row, marked deferred | ✔ box, marked deferred | ✔ WP-I *deferred* | ✔ WP-I deferred list | – | – | – | – | – | – |
| **S1** verify the launcher at install time (def) | – | ✔ C-017 (named) | – | ✔ WP-I *deferred* | ✔ WP-I deferred list + stub note | – | – | – | – | – | – |
| **S2** bound the guard's stderr noise (def) | – | – | – | ✔ WP-I *deferred* | ✔ WP-I deferred list | – | – | – | – | – | – |
| **S3** pin the command string byte-for-byte (def) | – | – | ✔ note under § registration table | ✔ WP-I *deferred* | ✔ WP-I deferred list | – | – | – | – | – | – |

Plus one item **not** from the audit, cross-referenced because B8 changes the same string: WP-B § 4's
**`$SHELL` risk** (Codex runs hooks through `$SHELL -lc`, so a `fish`/`nushell` user cannot execute the
POSIX guard at all). Recorded in WP-I's deferred list and as a WP-M watchlist obligation, explicitly
labelled *not a WP-P0 finding and not in this fold*.

## Ownership corrections made (the audit's Appendix D vs this plan's file sets)

| Change | Appendix D says | This plan's file sets require | Resolution recorded in |
|---|---|---|---|
| `trust_hooks: Option<bool>` (`src/config/declaration.rs`) | WP-G | **WP-E** — it owns `src/config/**` | § WP-E post-hoc block + WP-E row |
| `RegistryField::ALL` 6→7 (`src/command/config_keys.rs:204`) | WP-G | **WP-E** — it owns `src/command/config*` | same |
| The `write_config` emitter + the `registry_config_round_trips_every_field` tripwire (`src/command/add.rs:999-1030`, `:1390-1414`) | WP-G | **WP-E**, and `src/command/add.rs` must be **added to WP-E's `Expected files`** — it is otherwise WP-H's file. Safe under the serialized `E → F → G → H` merge order, exactly as the WP-A marker-arm precedent | same + WP-E row + a ⚠ on WP-H's row |
| `subsystem-cli-commands.md` field count 6→7 | WP-G | **WP-M** — it owns that file | § WP-M obligation 3 + WP-M row |
| The `env::grim_home()` source-level import test | WP-I (B1) | **WP-K** — WP-I does not create `src/command/hook*`, and WP-K already owns the identical A-10-pattern test for C-007 | § WP-K block (1) + WP-K row + C-006 ¶ (2) |
| The runtime halves of B1/B3/W2 (`--table` absoluteness, unknown-token ⇒ exit 0, `schema` reader) | WP-I | **WP-K** — same reason | § WP-K block (2)(3)(4) |
| Deleting C-011 control (7) | WP-I (with W4) | **the orchestrator/owner** — that sentence lives in `adr_hooks_support.md`, outside the plan, so it is an ADR amendment (**A6**) | C-006 ¶ (5) |

## Owed choices recorded, not made

The audit says "pick one of two" in four places. All four are recorded with **both** options and an
explicit owner; none was decided by this sweep. They are Open Questions **4, 5, 6** plus a fourth
riding with 6.

| # | Question | Owed by | Finding |
|---|---|---|---|
| Q4 | `--table '<abs>'` vs `--home '<abs>'` in the launcher argv | **WP-I** stub gate | B1 |
| Q5 | root token = 128 bits of randomness vs an HMAC of the root under a machine-local key | **WP-I** stub gate | B3 |
| Q6 | `GRIM_ALLOW_HOOKS` deleted from the surface vs read-and-ignored-with-a-warning | **WP-G** stub gate | B6 |
| Q6b | `GRIM_EXPERIMENTAL_HOOKS` env form honoured *only to disable* vs *required from global config when it enables* | **WP-G** stub gate | W6 |

## Layer sweep verification — the exact greps run

All run against `/mnt/wsl/share/dev/grimoire/grimoire/.agents/plans/plan_hooks_artifact_kind.md`.

| # | Grep | Purpose | Result |
|---|---|---|---|
| G1 | `for id in B1..S3; grep -c "\*\*$id\*\*\|$id ·"` | every finding id is present and attributed | all 20 present; B1 ×14, B3 ×11, S1/S2/S3 ×1 each |
| G2 | `grep -n "GRIM_ALLOW_HOOKS"` | **B6 consistency across every occurrence** — the brief's named risk | 20 hits; the two that still asserted an effect (C-022 body, WP-E Specify) were annotated in place; WP-M's "add `GRIM_ALLOW_HOOKS`" struck through and superseded |
| G3 | `grep -n -- "--root global\|--root <abs"` | no un-annotated pre-token root claim survives | 15 hits, each either struck through, inside a corrected box, or a deliberate description of the **attacker's** literal |
| G4 | `grep -n "machine-local\|whole machine\|machine-global"` | the C-006 "whole machine" claim is corrected everywhere | 12 hits; **WP-I's Stub line was still asserting it and was corrected in place** (found only by this grep) |
| G5 | `grep -n 'exec "\$L"'` | no un-annotated `exec` form survives | 3 hits: the superseded code fence (now carrying a DO-NOT-IMPLEMENT comment), the strike-through in the box, and WP-B's verdict item 1 (now marked **superseded**, also found only by this grep) |
| G6 | `grep -n "control (7)\|re-verified at execution"` | W4's companion deletion is owned | 1 hit, now naming the ADR and the owner |
| G7 | `grep -n "approved digest\|resolved_digest"` | W4's rename reaches the Stub line, not only the contract | 6 hits, incl. WP-I's Stub |
| G8 | `grep -n "trust_hooks: bool\|trust_hooks: Option"` | B7's tri-state is stated as the required type | 3 hits, all `Option<bool>` or the diagnosis of `bool` |
| G9 | `grep -n "env::grim_home\|grim_home()"` | B1's forbidden call is named in contract, WP, risk and test-owner layers | 18 hits across L1, L4, L6, L9 |
| G10 | `grep -n "\`Specify:\` additions"` | **the arming check** — every WP whose contract changed has a Specify line, not just prose | 4 explicit blocks (WP-G, WP-H, WP-I, WP-K); WP-E's is phrased "`Specify:` gains two tests"; WP-O's bullet list *is* its test list |
| G11 | per-WP `awk` slice + `grep -c "WP-P0"` | no WP section that owes a change was left untouched | A 0, B 1, C 0, D 0, **E 2, G 1, H 1, I 5, K 1, M 3, O 1**, F/J1/J2/L/N 0 (correctly — no finding lands on them) |
| G12 | `wc -l` | sanity | 1408 → 2209 lines, additive only |

**A process note worth recording:** my Bash cwd silently drifted into `.agents/worktrees/wp-f`, so
three verification greps ran against WP-F's older copy of the plan and appeared to show my edits
missing. Caught by comparing file sizes at absolute paths. Every `Edit` used absolute paths and was
unaffected — but a sweep that had *trusted* those greps would have re-applied edits into the wrong
file. Absolute paths in verification greps too, not just in edits.

## Contradictions found

**One, and it is an ownership contradiction rather than a factual one:** the audit's Appendix D routes
B7's three sites "all three in WP-G's commit, or the tripwire test fails", which is unachievable
against WP-G's declared file set (`src/hook/{trust,audit}.rs`, `src/main.rs`). The audit is right that
they must land **together**; it is wrong about which WP. Resolved as the table above, with
`src/command/add.rs` added to WP-E's `Expected files` and flagged on WP-H's row.

**No factual contradiction with WP-B's executed evidence.** The two places they could have collided
both turn out to be strict extensions:
- **The guard matrix.** WP-B § 4 measured the *absent* and *mode-0644* launcher states (both exit 0);
  WP-P0 measured *directory* (126), *bad interpreter* (127), *ENOEXEC* (126) and *mode 0100* (2). WP-B
  never tested those states, so B8 extends § 4 rather than contradicting it. Recorded that way on
  WP-B's verdict item 1.
- **The env-derived path.** WP-B § 5 proved env vars in the *registered command string* expand on all
  three clients; B1 is grim's own `env::grim_home()` call *inside the runtime*, one layer below. The
  audit states this explicitly and it holds.

Two Blocks were independently re-verified against source before folding, at the lead's instruction,
and **both hold**: **B1** (`src/env.rs:26-34` returns the env value verbatim, no absoluteness check,
relative `.grimoire` when `HOME` is unset) and **B7** (`src/command/add.rs:999-1030` is
emit-only-when-true, so a `trust_hooks: bool` silently drops an authored `false`).
`RegistryField::ALL` was confirmed at `src/command/config_keys.rs:204` as `[RegistryField; 6]`.

## One thing left alone, deliberately

The Status block still reads **`Active phase: 1 — Wave 1`** while wave 2 is running. That is the
orchestrator's bookkeeping field and editing it risks colliding with the lead's own state tracking, so
only **`Last update`** was bumped. Flagging it rather than fixing it.
