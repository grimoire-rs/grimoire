# WP-D + WP-J1 — Implement phase report

Worktree `.agents/worktrees/impl-d`, branch `hex/hooks-artifact-kind--impl-d`, base `7873f20`.
Files touched (declared set only): `src/install/json_splice.rs`, `src/install/client_target.rs`.
**Uncommitted, not pushed.**

## Findings — defects in merged code / contracts

### B-1 (Block, merged code) — `Vendor::hook_registration` is uncallable from outside `vendor.rs`, so WP-J1's cell cannot be completed

`hook_registration`'s `root` parameter is `vendor::RootToken<'a>(&'a str)` (`src/install/vendor.rs:124`)
— a tuple struct with a **private field and no constructor of any kind** (there is no
`impl RootToken<'_>` block anywhere in the file). Proven by compiling the real call:

```
error[E0603]: tuple struct constructor `RootToken` is private
   --> src/install/client_target.rs:739:31
    |
739 |     let root = super::vendor::RootToken("grim-matrix-probe");
    |                               ^^^^^^^^^ private tuple struct constructor
   ::: src/install/vendor.rs:124:26
124 | pub struct RootToken<'a>(&'a str);
    |                          ------- a constructor is private if any of the fields is private
```

The type's doc says a token *"is constructed by the registrar's derivation"* — but the registrar
(`src/install/hook_registrar.rs`) is a **sibling** module and equally cannot construct one, so this
blocks **every** planned consumer, not only mine: WP-I's registrar, WP-J2's install branch, WP-L's
parity test. Not a WP-J1 defect and not fixable from WP-J1's file set.

### B-2 (Block, merged code) — two `RootToken` types for one concept

- `vendor::RootToken<'a>(&'a str)` — WP-F, placeholder, no constructor.
- `hook_dispatch::RootToken(String)` — WP-I, the real thing: `Serialize`/`Deserialize`, `as_str()`,
  `Display`, and the actual derivation `root_token(grim_home, RootScope) -> io::Result<RootToken>`.
  Re-exported as `install::RootToken` (`src/install.rs:60`).

`vendor::RootToken`'s own doc comment warns against precisely this — *"two types for one concept, one
of them the forbidden wire form, is how B3 gets quietly undone by a later reader reaching for the more
convenient one."* **Recommended fix (owner: whoever holds `vendor.rs`):** change
`hook_registration`'s signature to take `&super::hook_dispatch::RootToken` and delete
`vendor::RootToken`. Adding a `RootToken::from_wire(&str)` escape hatch instead would work
mechanically but re-opens the "anything may mint a token" hole the type exists to close — not
recommended.

### W-1 (Warn, WP-D's own shipped doc contract) — the no-husk cascade deleted foreign data, and my own test caught it

`remove_nested_handler`'s doc said, verbatim: *"An emptied element array drops its whole group
object"* — two paragraphs above *"That is deliberate — never delete foreign data."* Those contradict
for a real case: a user authors `{"matcher": "*"}` with **no** handler array, grim adds
`"hooks": [dispatcher]` to **their** object, and on removal the literal rule deletes the user's group
object (and, being the only group, cascades to the whole `hooks` container).

**Fixed, and the doc amended in place.** The cascade now drops the group object only when the group
carries **nothing but `group_key` and `elements_key`** — the two members grim writes when it creates a
group, so such a group is indistinguishable from one grim authored. A group carrying any further
member is provably not grim's alone, so only the `elements_key` member grim added is cut.

**One irreducible residue, asserted so it cannot change silently.** A group whose *only* authored
member is the matcher is byte-identical to one grim creates, so no rule can separate them, and the two
desired outcomes conflict (byte-for-byte reversibility for grim's own group vs. sparing an inert user
stub). Byte-for-byte wins, because grim creating its own matcher group is the shipped v1 path and the
loss is a matcher group with no handlers, which does nothing. Pinned by
`nested_upsert_creates_an_absent_element_array_in_an_authored_group` and
`nested_remove_never_deletes_a_group_carrying_authored_content`.

### W-2 (Warn, process) — the `expect(dead_code)` mechanic cannot survive its own tests, and `allow` is forced

Every stub-phase `#[expect(dead_code)]` in both files had to become `#[allow(dead_code, reason = …)]`.
This is not laxity, it is arithmetic: once this module's tests exercise an item, the **test** target
reports `unfulfilled_lint_expectations` (the item *is* used) while the **bin** target reports
`dead_code` (no production caller). Under `-D warnings` both are errors, and `allow` is the only
attribute satisfying both. Verified by compiling all four states.

Consequence worth recording, because the plan chose `expect` specifically to make wiring
"compiler-proven instead of reviewer-trusted": from the moment a stub gains tests, that proof is gone
and the REMOVAL TRIGGER is prose again. The plan already half-knew this — WP-I's `sync_for_state`
carries the note *"`#[expect(dead_code)]` proves a function is reachable, never that it is consumed."*
The still-open obligations are restated as comments at each site rather than as attributes.

Two attribute placements were rejected by the compiler exactly as the brief predicted: `expect` on
`NestedGroupPath` / `NestedHandlerPath` became unfulfilled the moment the implemented bodies read
their fields (before tests existed at all).

## What landed

### `src/install/json_splice.rs` — GitHub #56, all five sites

`json_key` implemented (delegating to `json_string`; a JSON object key *is* a string literal, and two
names keep "where are keys escaped" greppable). Every one of the five interpolation sites and all
seven interpolations now route through it — `upsert_member` lines 62/77/111 (`container` **and**
`member` at the first two) plus `upsert_array_element` 189/199, which #56 never mentions. `grep '\\"'`
over the module returns one hit, inside `json_key`'s own doc comment.

Also extracted the thrice-duplicated `'{container}' is not a JSON object` refusal into
`not_an_object`, behaviour-identical.

Tests (unit half of the Principle 9 obligation; the pre-escaping-era on-disk fixture stays owed to
Specify):
- `hostile_member_name_survives_every_upsert_site` — `"`, `\`, `U+0001` and a literal
  string-closing injection payload (`x", "injected": {...}, "y": "`) as the **member**, at all three
  `upsert_member` sites; asserts valid JSON, exactly one member, siblings intact.
- `hostile_member_name_is_found_again_and_removed` — the *other* half of the defect: a `\` used to
  emit valid JSON decoding to a different key, so the lookup never matched and grim re-inserted a
  duplicate every install. Asserts re-upsert is `Unchanged`, `member_value` finds it, remove drops it.
- `hostile_container_name_and_array_key_are_escaped_too` — the two currently-unreachable classes,
  fixed anyway because "currently unreachable" is the reasoning that made #56 latent.
- `escaping_is_the_identity_function_on_an_ordinary_name` — `/`, non-ASCII and ordinary names are
  byte-identical, which is what makes the fix self-healing.

### `src/install/json_splice.rs` — the object-in-nested-array primitive

All four bodies implemented: `upsert_nested_handler`, `remove_nested_handler`,
`nested_handler_value`, `owned_nested_handlers`.

- **Span-preserving by construction.** Every write is nested substitution —
  `splice_span` / `cut_out` at the innermost level, then substituting each new inner text back into
  its parent's span, up through elements array → group object → group array → `container.member`.
  No offset arithmetic across levels, so a composition bug cannot silently shift a span.
- **The three "nothing to locate" cases delegate to `upsert_member`** (no document, no container, no
  member) rather than hand-rolling a second skeleton/insert path one nesting level down. DRY, and it
  inherits the existing round-trip tests for those paths.
- **First-match at both levels**, matching `upsert_array_element`/`remove_array_element` rather than
  `last_member`'s last-wins, so "removal undoes insert byte-for-byte" is satisfiable.
- **Both degenerate identities refuse, in `upsert`, `remove` and (as `None`) the read** — empty
  `identity_keys` (vacuously true ⇒ would adopt and overwrite a *user's* element) and a handler
  lacking an identity key (never satisfiable ⇒ insert-every-run). `validate_identity` runs **before**
  the tolerant no-ops in `remove_nested_handler`, deliberately: a degenerate identity is a caller
  defect, not a state of the file, and refusing it only for some inputs would make the pair disagree
  about its own contract.
- **The marker's value is never touched by this module.** `identity_keys` and `owner` are both
  caller-supplied, so the constant-vs-artifact-name decision stays in `vendor.rs` where
  `HOOK_MARKER_VALUE` is frozen. `owned_nested_handlers` matches `owner` by **exact parsed value**, as
  its contract says.
- **Issue #56 discipline applies to the new code too** — the one key it writes
  (`elements_key`, when adding the array to an authored group) goes through `json_key`; no pointer or
  key path is ever built by string concatenation, and `NestedGroupPath`'s four names plus
  `group_value` are consumed as **data** (compared against parsed values, rendered through
  `serde_json`), never as syntax.
- **`owned_nested_handlers` skips a group whose `group_key` is absent or not a string**, documented
  as the conservative direction: the pair it returns feeds `NestedHandlerPath::group_value`, a `&str`,
  so a non-string group key yields a group the caller could not address for removal anyway. Grim only
  ever writes a string one, so reaching this needs a hand-edit that moved grim's marker somewhere grim
  never put it.

20 new tests, covering: creation from empty text / absent container / absent member / absent group /
absent element array; comment, key-order and formatting preservation against a realistic
`settings.json` with two authored matcher groups; `remove` undoing `upsert` byte-for-byte in six
distinct shapes; the D-2 relocation case (a changed `command` **updates**, exactly one element, no
fork); the full no-husk cascade and each level it must *not* fire on; JSONC comments and trailing
commas; eight refusal shapes; both degenerate identities; and the D-1 enumerate-and-reap loop —
install three matcher groups, enumerate what is owned with **no record at all**, remove
`owned − desired`, and land back on the authored file byte-for-byte.

### `src/install/client_target.rs` — `hook_matrix_cell` (C-013)

Implemented, minus the one blocked call:

- **Rule 1** — the probe set is `CanonicalEvent::ALL × HookTier::ALL` filtered by
  `HookTier::is_valid_at`, so the cell never asks about combinations `grim build` rejects and a tier
  not declarable at an event cannot degrade a cell (`verdict: &[]` at `SessionStart`).
- **Rule 2** — any-arms over `HOOK_CELL_PROBE_MATCHERS`, so Decision K's per-`(tool, matcher)`
  refusal cannot drag a client to `◐`.
- **Rule 3** — `✓` when every declarable pair arms, `◐` when some, `✗` when none. `declarable` is
  never zero (`observer` is valid at every event), so the `✓` arm cannot be reached vacuously.
- **The verdict is read off `hook_registration` and off nothing else** — `hook_tier_support` is never
  consulted here. It answers `Native` for `(Mutator, PreToolUse)` on claude and copilot while the
  registration declines, and a cell filled from it alone is the S-013 silent-guardrail report.
  `hook_tier_support`'s `Degraded` (from a `context: None` column) has no bearing: no tier is entitled
  to `additionalContext`, its one owner is mutator control 5 at `PreToolUse`, and all three v1 clients
  carry the channel there.
- `hook_cell_probe_entry` builds the throwaway `HookEntry`; every field outside `(tier, matcher)` is
  fixed filler, and `event: None` deliberately, since a `<vendor>.event` override is exactly the
  per-client special case a matrix cell must not encode.
- `hook_cell_probe_arms` is the **only** remaining `unimplemented!()`, and its body is the blocked
  `hook_registration` call. Its doc carries the compiler error, names the two candidate fixes, and
  states that when the signature lands the body becomes
  `vendor.hook_registration(&entry, event, launcher, root).is_ok()` and nothing else changes.
- `hook_matrix_cell_declines_every_client_with_no_hook_surface` pins the fail-safe half — quantified
  over `ClientTarget::ALL` (not a literal list, so a new vendor is covered the day it lands) and
  asserting the roster is 15 of 18.

## Still owed

1. **B-1/B-2's fix in `vendor.rs`**, then `hook_cell_probe_arms`' one-line body. Everything else for
   C-013 is in place. **One line, one file — route it back here rather than to a new worker.**
2. **The pinned per-client cell values** (`claude`/`codex` `Native`, `copilot` `Degraded`, 15
   `Declined`) and the **launcher-independence** test (compute the cell with two launcher paths,
   assert equality) — both need item 1 first, and both are unwritable until then because they would
   panic rather than fail.
3. **Principle 9 acceptance half for #56** — install against a **pre-escaping-era on-disk fixture**,
   upgrade, assert `status` not-modified and the file byte-unchanged. Every test here runs against
   files the new code wrote, so none of them proves self-heal. The unit half (escaping is the identity
   function on an ordinary name) is done.
4. **The #56 regression fed as a `grimoire.toml` binding key** end to end, per § G-1's re-aimed
   Specify note — the reachable hostile input is the binding key, not an artifact-internal name. Not
   reachable from this file set (it needs the config parse path).
5. **`toml_splice.rs`'s `container`-name test** and its module-doc audit invariant — in WP-D's planned
   file set but **outside the file set I was given**, so untouched.
6. **Binding-name charset validation as defence in depth** — still owed and still unowned (§ G-1
   names it explicitly as out of WP-D's scope but in scope to name). An unvalidated binding name flows
   to filesystem paths and state records, not only to this splice.
7. **`hook_registrar::sync_for_state` must actually call `owned_nested_handlers`** and remove
   `owned − desired`. The primitive exists and is tested; nothing enforces that the registrar uses it,
   and its absence is the D-1 hole (a registration armed forever in a file grim does not own).

## Judgement asked for by the orchestrator — WP-I's `owns_anything` substring probe

**Keep it as a cheap pre-filter. Do not replace it with `owned_nested_handlers`.** The two answer
different questions and the composition is strictly better than either alone:

- `owns_anything` (`hook_registrar.rs:346`) answers *"is it possible that something grim-owned is
  here?"* — the **skip** decision. It must never say `false` when a marked element exists, so an
  over-approximation is the correct shape.
- `owned_nested_handlers` answers *"what exactly do I own, so I can remove what I no longer want?"*
  — the **reap** decision, inside the convergence body. It must be exact.

**The enumeration has three blind spots, and replacing the pre-filter would make them load-bearing
for the skip decision — the one direction that strands an armed registration.**

1. **Unparsable text ⇒ owns nothing.** `owned_nested_handlers` returns `Vec::new()` when
   `parse_value` fails. The substring probe never parses, so a config a user has *broken* while
   grim's marked element is still in it stays visible: convergence runs and the add-strict splice
   refuses, which surfaces as a warning. Under a replacement it would read as `NoHooks` — silently.
   **This is the one thing that still depends on the weaker form**, and the probe's doc should say so
   rather than calling it imprecision.
2. **Per-event blindness.** The enumeration reads one `member` per canonical event, so an element
   written under an event key *this* binary does not project — a future grim adds an event, or a
   vendor's event-name projection changes — is invisible to it. WP-I's own module doc already records
   this as N-3; what it does not say is that the substring probe is what covers it.
3. **Non-string `group_key` ⇒ skipped** (documented in `owned_nested_handlers`, conservative because
   such a group is unaddressable via `NestedHandlerPath::group_value: &str`). The probe sees it.

Two further reasons, independent of correctness:

- **Cost.** The guard's own doc requires it to cost "almost no reads", and `sync_config` runs for
  every client on every install, update, uninstall and TUI action. A substring scan is memchr-class;
  the enumeration is a full serde parse plus a JSONC-sanitize fallback — over `~/.claude.json`, which
  `json_splice`'s module doc calls out as Claude's monolithic user-state file.
- **It is not a drop-in.** `owns_anything` takes `&dyn Vendor` + `&Path` and has no way to obtain
  `container` / `member` / `group_key` / `elements_key`. A replacement needs a new vendor seam — the
  same shape as the already-recorded *"promote it to `Vendor::hook_config_path`"* obligation.

**Correction owed to WP-I's doc (its file, not mine).** `owns_anything`'s doc says *"Replace the
probe with the enumeration when WP-D's body lands — the guard's contract does not change, only its
precision."* That is now wrong on both counts: for the skip decision precision would **decrease**
(three new false-negative classes), and the contract **would** change (unparsable-but-marked flips
from reported to silent). The instruction should be replaced with the split above.

**One cheap improvement worth taking, same direction.** Probe for **both** `HOOK_MARKER_KEY` and
`HOOK_MARKER_VALUE` being present, not just the value. Two independent strings must both appear in
the bytes for a marked element to exist, so it still can never say `false` when one does — while
killing the accidental positive where the word `hook-dispatcher` merely appears in a comment or an
unrelated value. If the false-positive cost ever measures, narrow the *substring* further; never
narrow it into a parse.

## Second judgement — does the splice path assume a bare-path handler command? **No**

Asked after WP-A landed the C-019 rule (a handler whose command invokes a payload file directly is
rejected at `grim build`, because a registry-delivered payload arrives `0o644`). Two answers, and the
first is the structural one:

**1. The authored handler never reaches this module.** `HookEntry.handler` — the `HookHandler` C-019
validates through `first_token()` (`src/oci/hook.rs:317`) — flows into `DispatchEntry.handler`
(`hook_dispatch.rs:359`), i.e. the **dispatch table**. What the splice writes is grim's own dispatcher
registration, whose command is `HookRegistration.command` (`hook.rs:807`) — the launcher guard string
grim assembles itself. The two objects are distinct and only the second crosses this boundary, so
C-019's rule and `json_splice` do not interact at all.

**2. And the splice would not care either way**, which is the property worth pinning rather than
reasoning about. It consults exactly the members `path.identity_keys` names (one, the constant marker)
and treats the rest of the element as an opaque `serde_json::Value`. Two new tests prove it:

- `nested_splice_reads_no_element_field_but_the_identity_keys` — the `{type, command}` form,
  copilot's argv/exec form `{exec, args: […]}`, and a bare `{marker}` element all upsert, re-detect as
  `Unchanged`, and round-trip byte-for-byte identically. It also locates an element written in one
  shape using a probe of another, which is the same property that makes a launcher or root-token move
  an update rather than a fork (D-2).
- `a_command_string_reaches_the_config_as_escaped_data` — five command strings, including the
  multi-line `L='…'` guard form, `$(id)`/backtick/quote metacharacters, `./guard.sh` and a Windows
  path with backslashes, all land as JSON string **values**, come back verbatim, are re-found on the
  next run, and remove cleanly. Nothing here parses or splits a command; C-018b is enforced where the
  string is assembled, not here.

So: no assumption to correct, and now no assumption that can silently appear later either.

## Unblocked — `hook_matrix_cell` complete, and the shipped column

`Vendor::hook_registration` now takes `&hook_dispatch::RootToken` and
`vendor::RootToken` is deleted, so the cell is implemented. **No `unimplemented!()`
remains in either of my files.**

| Client | Cell | Matches the stub's prediction? |
|---|---|---|
| claude | `Native` (`✓`) | yes |
| codex | `Native` (`✓`) | yes |
| copilot | `Degraded` (`◐`) | yes |
| the other 15 | `Declined` (`✗`) | yes |

**Nothing moved.** The fold was not adjusted to match an observation — the pinned
test was written from the stub's rationale and passed on the first run against real
bodies. Copilot's `◐` rests on `verdict: &[]` at `PostToolUse`, where gatekeeper *is*
declarable (claude and codex both block there), so the gap is matcher-independent.

**Decision K went live and did not move a cell, which is the two-probe design working.**
`hook_matrix_cell_is_not_hook_tier_support` states it executably: for
`(Mutator, PreToolUse)` on claude and copilot, `hook_tier_support` answers `Native`
while the **match-all** registration declines (`MutatorOnShellCommandTool`) — and the
same tier arms for `Some("Read")`. Any-arms is what keeps a per-`(tool, matcher)`
security refusal out of the doc column; all-arms would have dragged claude and codex
to `◐` and made the column say "partial hook support" about decision K.

### `root` is a parameter, not minted in the cell — and that is the honest option

`RootToken` has **no constructor**, deliberately. The only way to obtain one is
`hook_dispatch::root_token`, whose HMAC derivation reads or creates the machine key
under a **real** `$GRIM_HOME`. A documentation-matrix helper must not depend on a
resolved `$GRIM_HOME`, so there were three options: do filesystem I/O inside the cell
(wrong — a doc question doing I/O, with no error channel and `Declined` as the only
degradation, which would publish "no hook support" on an I/O blip); acquire a test-only
bypass (refused — that re-opens exactly what deleting the placeholder closed); or
**take the token from the caller**. The third is what shipped. It costs nothing: every
real consumer already has a `$GRIM_HOME` (WP-H's status cell runs inside a grim
process; WP-L's parity test derives one in a temp dir, as this module's tests do), and
`hook_cell_probe_independence` proves the value cannot change the answer — two
independently keyed tokens give an identical column for all 18 clients.

`HOOK_CELL_PROBE_TABLE` was added beside the launcher const for the new `table: &Path`
parameter, on the same argument: every refusal returns before launcher, table or root
is read — they reach only `registration_command` in the `Ok` arm.

### Finding (Warn, merged code) — `RootToken`'s "no constructor" property is not enforced

`vendor.rs:991-995` states a token is *"minted **only** by `hook_dispatch::root_token`'s
HMAC derivation, and that is the whole safety argument"*. That is the intent, but
`RootToken` derives **`Deserialize`** (`hook_dispatch.rs:192`), and a private-field
newtype with `Deserialize` has a transparent String deserializer — so
`serde_json::from_str::<RootToken>("\"anything\"")` mints one from an arbitrary string.
I did **not** use that route (it is the bypass the brief forbids); reporting it because
the doc asserts a property the type does not have.

It is not simply removable: `DispatchTable.roots` is a `BTreeMap<RootToken, _>` and has
to round-trip through JSON. Two honest fixes, both cheap: soften the doc to "no
*constructor API* exists; the serde route exists for the dispatch table alone and is
not a minting path", or keep the claim true by giving the table a
`#[serde(with = …)]`/wrapper so `RootToken` itself needs no `Deserialize`. Attacker is
**T3/T4** and the exposure is *inside* grim's own process — a forged token still has to
match a table entry — so this is a doc-accuracy Warn, not a Block.

## Gates

| gate | result |
|---|---|
| `cargo check --all-targets` | clean |
| `cargo clippy --locked --all-targets -- -D warnings` | clean |
| `cargo test --bin grim` | **2709 passed, 0 failed** (baseline 2689; +20 json_splice, +1 client_target) |
| `cargo fmt` | applied, tree formatted |
| **`task --force verify`** | **exit 0** — 51 AI-config tests, 2709 unit, **1019 acceptance**, all passed |

Re-run after merging the feature-branch tip `9a50255` (WP-I, WP-G, WP-H — none of which touches
either of my files, so the merge was conflict-free):

| gate | result |
|---|---|
| `cargo clippy --locked --all-targets -- -D warnings` | clean |
| **`task --force verify`** | **exit 0** — **2761 unit** (2709 mine + 52 from the merge), **1019 acceptance**, 0 failed |

Re-run after merging `7d5ad50` (the ownership-probe fix) and `f0e268a` (WP-A manifest parsing), plus
the two shape-blindness tests:

| gate | result |
|---|---|
| **`task --force verify`** | **exit 0** — 51 AI-config, **2791 unit**, **1019 acceptance**, 0 failed |

Final run, after merging the `RootToken` signature change and completing
`hook_matrix_cell` (+4 tests: pinned per-client values, root-independence,
tier-support-vs-registration, and the fail-safe roster):

| gate | result |
|---|---|
| `cargo clippy --locked --all-targets -- -D warnings` | clean |
| `grep unimplemented!` over both files | **no match** |
| **`task --force verify`** | **exit 0** — 51 AI-config, **2800 unit**, **1019 acceptance**, 0 failed |
| `git status --short` | only the two declared files modified; nothing committed, nothing pushed |

`task verify`'s `uv` step rewrites `test/uv.lock` and `.claude/tests/uv.lock` as a side effect; both
were reverted so the diff stays inside the declared file set.
