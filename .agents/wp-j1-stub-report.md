# WP-J1 — Stub phase report

Worktree `.agents/worktrees/wp-j1`, branch `hex/hooks-artifact-kind--wp-j1`, base `9c82115`.
Files touched (declared set only): `src/install/path_anchor.rs`, `src/install/client_target.rs`.
**Uncommitted, not pushed.**

## Findings — defects in the plan / ADR / merged code

### F-1 — WITHDRAWN. C-013 needs no amendment; the ADR sentence is correct

I filed this as a Block: that scoping `✓` to tiers valid at the event still left the cell
unsatisfiable, because `context: None` at `claude·Stop` (`src/oci/hook.rs:693`), `codex·Stop`
(`:736`), `copilot·SessionStart` (`:773`) and `copilot·Stop` (`:784`) makes
`hook_tier_support` answer `Degraded`, so no client reached `Native` everywhere.

**Refuted by the orchestrator, and the refutation is right.** The projection facts were correct;
the error was one level up — I treated an absent `context` as a *fidelity loss for the observer
tier*, but **`additionalContext` has no tier owner**, so its absence costs no tier anything it was
entitled to. From the tier definitions (`src/oci/hook.rs:196-206`): `Observer` is defined as a
tier whose *"response cannot change what happens"*, and injecting context changes what the model
subsequently sees, so an observer emitting it contradicts its own definition; `Gatekeeper`'s power
is the verdict, `Mutator`'s is `updatedInput`. The channel's one owner is **mutator control 5,
visible-to-model** (S-016, ADR:1371), `mutator` is valid only at `PreToolUse`, and all three v1
clients carry `context: Some(_)` there — so the capability that needs the channel has it exactly
where it is needed, and `context: None` at `Stop`/`SessionStart` denies nothing. My objection to
(b) ("publishes `✓` for a client that silently drops `additionalContext` at `Stop`") does not
hold: nothing at `Stop` may emit it.

Corrected rule, now stubbed: **only a decline of a tier declarable at that event degrades a
cell.** `hook_tier_support`'s `Degraded` has no bearing on the cell, and `verdict: &[]` at
`SessionStart` is not a decline at all — no client admits a verdict there, so gatekeeper is not
declarable at that event. Shipped column: **claude `✓`, codex `✓`, copilot `◐`, 15 × `✗`**.

Reading (a) would have shipped 3 × `◐` + 15 × `✗` with zero `✓` — a docs matrix telling users no
client supports hooks. Recorded here rather than deleted, and recorded in the `hook_matrix_cell`
doc comment as the *reason* rather than as an alternative, so the next reader cannot re-derive (a).

**One consequential follow-on I found while re-deriving** — it changed the code, not just the
prose. Under (b), requiring **every** probe matcher to arm a pair would let ADR decision K alone
drag claude and codex back to `◐`: a match-all `mutator` is refused
(`MutatorOnShellCommandTool`), and that is a declarable pair at `PreToolUse`. So the quantifier
over `HOOK_CELL_PROBE_MATCHERS` had to become **any-arms, not all-arms** — Decision K refuses a
`(tool, matcher)`, never a client, so a client whose mutator arms for `Read` and declines for `*`
*has* the tier, and the refusal the author actually hits is reported per hook through the S-013
`Declined` path where a per-matcher fact belongs. A decline holding for **both** probes is
matcher-independent — a projection-table gap — and that is what degrades. Without this the
adopted reading would not produce the matrix it is supposed to produce.

**One stale citation in the refutation, conclusion unaffected.** The second driver given for
copilot's `◐` — "`mutator` at `PreToolUse` Declined at v1 on the unverified `updatedInput`
spelling (ADR:1182)" — was superseded by WP-B: merged WP-A carries
`mutation: Some("hookSpecificOutput.updatedInput")` for `copilot·PreToolUse`
(`src/oci/hook.rs:754`) with the explicit note *"The earlier `Declined` was the
documentation-only answer; do not restore it."* Copilot's `◐` therefore rests on the
`PostToolUse` `verdict: &[]` ground alone, which is sufficient. Verdict unchanged; flagged only
because ADR:1182's row is now stale prose that a later reader could restore.

### F-2 (Warn, merged code) — `client_supports_kind` already answers `true` for `Hook` on all 18 clients

`installer.rs:1118-1121`'s catch-all arm covers `Hook` today, so
`client_supports_kind(Warp, Hook, ws, Global)` is `kind_support(Hook) != Declined && kind_surface(…)`
= `true && true` = **true**. That is precisely decision D-1's failure mode, live in merged code
since WP-A added the variant. It is inert only because every install/remove/fetch seam refuses
`Hook` first (`command/install.rs:200,289`, `command/remove.rs:66`, `fetch.rs:488,930`,
`mcp/render.rs:103,130,158`) — i.e. protected by a refusal WP-J2 is going to delete. The plan
assigns the arm to WP-J2; it is worth stating that the window is already open, not opening.

My `is_declined_global_pair` gate closes the *anchoring* half in the fail-safe direction, so the
two predicates now **disagree** for the 15 surfaceless clients (`client_supports_kind` says yes,
`candidate_anchors` returns the empty set). Until WP-J2 lands its arm that divergence would
surface as an `UnknownAnchor` warning rather than a reported skip. Unreachable today, and the
right way round: the classifier fails safe.

### F-3 (Warn, plan) — the plan's pinned-set instruction is not enforceable from my file set, but the test *can* live here

WP-F's note asks that the `SCOPE_GAPS` pinned-set test run through `client_supports_kind` at both
scopes. `client_supports_kind` is in `src/install/installer.rs` — **outside** my declared set — so
I could not add it. It is reachable from `client_target.rs`'s test module
(`crate::install::installer::client_supports_kind` is `pub`, and WP-L's row already declares
`client_target.rs (tests)`), so Specify can host it there without widening any file set. Recorded
as a Specify obligation below; nothing in the Stub phase can enforce it.

Note also that WP-F **already landed** the two hook rows in `SCOPE_GAPS`
(`vendor.rs:1319-1320`), so "if hooks add a scope gap, it belongs there" is done — my brief's
phrasing implied it was still open.

### F-4 (Suggest, plan) — `<workspace>/.grimoire/hooks/` collides with the default `$GRIM_HOME` basename

S-003's project payload root is `<workspace>/.grimoire/hooks/<name>`; `$GRIM_HOME` defaults to
`~/.grimoire`. A user who sets `GRIM_HOME=<workspace>/.grimoire` makes the project and global
payload paths **byte-identical**, so both scopes materialize into one directory and two records
(different anchors, same resolved path) point at it. WP-P0's C-017 refusal cause 2 —
`grim_home()` resolves inside the workspace — refuses *arming* in exactly that configuration, so
nothing executes; the **materialization** half is unguarded. Inert (a payload tree is data), and I
kept the plan's path verbatim rather than inventing a different segment. Flagged for WP-J2 /
WP-I: cause 2 is the whole mitigation, and it should be stated as covering this collision too.

## What landed

### `src/install/client_target.rs`

- **`path_for`'s `Hook` arm** replaces the `unreachable!()`. Scope-branching, **client-blind** (it
  never touches `self.vendor()`): project ⇒ `<workspace>/.grimoire/hooks/<name>`, global ⇒
  `<workspace>/hooks/<name>`. The global form is correct because `workspace` **is** `$GRIM_HOME`
  at global scope — verified at source, not assumed: `command/scope_resolution.rs:93`
  (`let workspace = paths.root()`), and the shipped test
  `path_for_global_scope_uses_vendor_native_roots` asserts the same convention for
  `global · opencode · rule`.
- **`materialize_hook`** (new, `fn` not method — takes no `self`, so a future per-client branch
  has to be added deliberately). Verbatim `copy_tree`, every file `generated: false`, no chmod
  (C-019: the exec bit is never load-bearing), idempotent across the N per-client calls that share
  one `dest` (C-020).
- **`hook_matrix_cell(client) -> KindSupport`** (new, `pub`, C-013). Fail-safe half implemented —
  `hook_surface().is_none() ⇒ Declined`, the answer for 15 of 18 clients — rest
  `unimplemented!()`. Same split and same rationale as WP-F's `hook_tier_support`.
  Two supporting consts: `HOOK_CELL_PROBE_LAUNCHER`, `HOOK_CELL_PROBE_MATCHERS`.

**The arming verdict is read off `hook_registration`, and off nothing else.** The doc comment
states the aggregation as three numbered rules a test can be generated from: probe set = the
`(event, tier)` pairs `HookTier::is_valid_at` admits; a pair arms when **at least one**
`HOOK_CELL_PROBE_MATCHERS` entry returns `Ok` and is `Declined` only when every entry returns
`Err`; the cell is `✓` when every declarable pair arms, `◐` when some but not all do, `✗` when
none do. `hook_tier_support` is not consulted directly — `hook_registration` already consults it
inside its own refusal order (`TierUnsupported`), so every projection-table gap reaches the cell
through one authority instead of two, and the `Degraded` value that F-1 turned on has no bearing.

The probe matcher set is **two** entries so the cell can separate a **matcher-specific** refusal
from a **client-wide** one: `Some("Read")` is a translatable non-shell name that exercises the
arming path; `None` (match-all) is what Decision K's conservative predicate refuses. See F-1 for
why the quantifier over them is any-arms.

`HOOK_CELL_PROBE_LAUNCHER` is a fixed absolute literal, safe because the entire refusal order of
`hook_registration` (surface → shape → event → tier → decision K → matcher) is launcher-blind —
the launcher reaches the command *string* and nothing else. Specify pins that assumption rather
than trusting it (see below).

### `src/install/path_anchor.rs`

- **`candidate_anchors`' `(_, ArtifactKind::Hook)` arm** replaces the `unreachable!()` with
  `Some(PathAnchor::GrimHome)`, which the existing tail collapses to `vec![GrimHome]`. Kept as a
  total arm rather than delegated to the guard, so a future `hook_surface` flip cannot reintroduce
  a panic (the A-3 lesson). No new `PathAnchor` variant, no `VENDOR_ROOTS` row, no
  `SHIPPED_ANCHOR_TAGS` entry, zero on-disk vocabulary change — Principle 9 clean.
- **`is_declined_global_pair` gains a `Hook` arm** reading `hook_surface().is_none()` instead of
  `kind_support`. This is the seam Decision A names: unmodified, the delegation answers
  "supported" for `Hook` on all 18 vendors (`kind_support` defaults to `Native` and every override
  ends in a wildcard), which would anchor a global hook payload under `$GRIM_HOME` for Warp and
  Zed — a recorded output nothing reads, which `prune`'s refcount would then keep alive for the
  clients that do arm it.
  `HookSurface::CodegenModule` is deliberately **not** excluded, so this predicate agrees with
  `client_supports_kind`'s planned `Hook` arm; the doc comment says so and names the agreement as
  the thing to test, not the variant list.
- **`expected_anchor_and_relative`** (test helper) — its `Hook` arm filled with the two table
  rows (`Workspace` + `.grimoire/hooks/<name>`, `GrimHome` + `hooks/<name>`) instead of left
  `unreachable!()`. This helper is the executable form of the §1.1 anchor-remainder table, so the
  row belongs with the table change; Specify only has to add `Hook` to the hand-maintained kind
  arrays and the round-trip is asserted for all 18 clients at both scopes.

### Scaffolding discipline

`#[expect(dead_code, reason = …)]` with a REMOVAL TRIGGER on `hook_matrix_cell`, per the brief —
**not** `allow`. (WP-F used `allow` on the `Vendor` hook *trait methods*; I did not touch those.)
The WP-F mechanic held empirically: the `expect` makes `hook_matrix_cell` a live root, so
`HOOK_CELL_PROBE_LAUNCHER` and `HOOK_CELL_PROBE_MATCHERS` already count as used and carry **no**
attribute of their own — confirmed by a clean `-D warnings` run, which would have reported either
an unfulfilled expectation or an unused const.

## Owed to Specify (this WP)

1. **The pinned-set test through `client_supports_kind` at both scopes** (F-3) — host it in
   `client_target.rs`'s test module. Consequence of omission is I1/T3.
2. **`is_declined_global_pair(c, Hook) == !client_supports_kind(c, Hook, ws, Global)`** for all 18
   clients — the agreement F-2's divergence makes testable.
3. **`hook_matrix_cell` is launcher-independent** — compute it with two different launcher paths,
   assert equality. Fails loudly if a future refusal starts inspecting the launcher.
4. **`hook_matrix_cell` values, pinned per client** — `claude` and `codex` `Native`, `copilot`
   `Degraded`, the other 15 `Declined`, quantified over `ClientTarget::ALL` rather than a literal
   list so a new vendor is covered the day it lands.
5. **The F-1 regression guard** — the two facts that would silently re-derive reading (a) if a
   later edit reintroduced them: an event whose `context` column is `None` does **not** move a
   cell, and a tier not declarable at an event (gatekeeper at `SessionStart`) does **not** move a
   cell. Both are decidable against the shipped table and both are one refactor away from being
   lost.
6. **Decision K does not move a cell** — `hook_matrix_cell(Claude)` is `Native` even though the
   match-all `mutator` probe declines: the any-arms quantifier is what makes that true, and an
   all-arms rewrite must fail a test rather than a review.
7. **S-003 paths** — `path_for(_, Project, Hook, n)` equal for all 18 clients and
   `= ws/.grimoire/hooks/n`; `path_for(_, Global, Hook, n) = grim_home/hooks/n`; plus the
   `from_target` round-trip once `Hook` joins the kind arrays (item in
   `expected_anchor_and_relative` is already filled).
8. **`materialize_hook`** — verbatim tree, every `MaterializedFile.generated == false`, mode
   preserved and never widened, and a second call over the same `dest` leaves identical content
   (the shared-dest idempotence C-020 rests on).

## Gates

| gate | result |
|---|---|
| `cargo check --all-targets` | clean |
| `cargo clippy --locked --all-targets -- -D warnings` | clean |
| `cargo test --bin grim` | **2689 passed, 0 failed** |
| `cargo fmt` | applied, tree formatted |
| `git status --short` | only the two declared files modified; nothing committed |
