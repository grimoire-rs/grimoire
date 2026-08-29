# WP-F — Implement report (vendor seam, `src/install/vendor.rs`)

Branch `hex/hooks-artifact-kind--impl-f`, fast-forwarded onto the feature tip **`6bec5f4`** (WP-I's
and WP-G's implemented modules) as instructed. One file changed, **not committed**:
`src/install/vendor.rs`. `task --force verify` **exits 0** (2735 unit tests, 1019 acceptance tests,
`cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `claude:tests`). All four
`unimplemented!()` are gone; no new `unimplemented!()`, `todo!()`, `unwrap()` or `expect()` outside
`#[cfg(test)]`.

---

## 1. The `RootToken` Block — fixed as directed

`Vendor::hook_registration` and the private `registration_command` now take
**`&super::hook_dispatch::RootToken`**, and `vendor::RootToken<'a>` is **deleted**. No
`new(&str)`/`from_wire(&str)` was added anywhere: the parameter is now mintable only by
`hook_dispatch::root_token`'s HMAC derivation, which is the property that makes it un-forgeable and
the reason `HookRoot` was deleted in the first place. Two independent confirmations of the defect
(the parameter was unconstructible outside `vendor.rs`) matched what I filed last round.

The load-bearing reasoning from the deleted type's doc is preserved where the parameter is
described, in `hook_registration`'s `--root` bullet: why a token and not a scope enum or a path, why
grim-chosen is necessary but insufficient, why two types for one concept with the weaker one
reachable is the regression, and that a scope/path are inputs to *deriving* a token
(`hook_dispatch::RootScope`) rather than something the registration ever sees.

`HOOK_MARKER_VALUE`'s `expect(dead_code)` arrived removed in the merge and was **not** re-added.

---

## 2. Remaining findings in merged code and contracts

### F-I1 (Warn, was Block) — two generators for one byte-hashed string, now **pinned equal by test**

`hook_launcher::registered_command` is implemented on the tip and emits the same five lines. I could
not delegate to it, and the obstacle is now mechanical rather than a judgement call:

- Its `expect(dead_code)` is `#[cfg_attr(not(test), …)]`. A **production** call from
  `hook_registration` makes the item used in the non-test profile, the expectation goes unfulfilled,
  and `-D warnings` hard-fails — which is exactly how `HOOK_MARKER_VALUE` bit WP-I. Deleting that
  attribute means editing `hook_launcher.rs`, which is outside my file set.
- `registered_command` returns `Result<_, CommandRefusal>`, and that refusal is the registrar's:
  `hook_registrar::arming_refusal` already checks both paths (`path_is_representable` over
  `launcher_path` and `dispatch_path`) in step 1 of `sync_for_state`, before any file is touched.
  Mapping it to a `HookDecline` would report an environment problem as a per-hook policy decline,
  and adding a variant would touch WP-J1's in-flight `client_target.rs`.

So the emission stays in `vendor.rs`, and I added the guard that makes the duplication safe instead
of latent — **`the_two_command_generators_agree_byte_for_byte`**: it calls
`hook_launcher::registered_command` (legal in the test profile, where the attribute is absent) and
asserts byte equality against `registration_command` for all three v1 clients × a benign path and a
hostile one (`$(touch …)`, backtick, space, `'`). It passes today, so **there is no live divergence**
— and if either generator drifts, the test fails instead of a client silently un-trusting an
approved hook.

The byte divergence I reported last round (`case "$s" in *) exit 0 ;; esac` vs the `0)` arm) is
**resolved, by WP-I**: commit `950b01b` corrected `hook_launcher`'s module doc to carry the `0)` arm,
and its `verdict_arms` renders the empty allowlist exactly as this file does. Same no-trailing-newline
decision on both sides, independently.

**Still owed at merge:** exactly one generator should survive. Preferred shape unchanged — make
`VERDICT_EXIT_CODES`/`verdict_exit_codes` reachable, delete `vendor::registration_command` and
`vendor::posix_single_quote`, and call `registered_command` from here; the equality test then becomes
the proof that the swap changes no byte, and can be deleted with the loser.

### F-I2 (Block, contract, fixed here) — the stub signature could not emit its own documented string

`--table '<abs dispatch.json>'` (B1) is in the documented string and the stub had no table
parameter. Added `table: &Path` after `launcher`, documented in the method doc. The caller
(`sync_for_state`, WP-I/WP-J2) holds `grim_home` and already computes both paths — and
`hook_launcher::CommandSpec` carries exactly these five values, which the new equality test now
depends on structurally. Deriving the table by parent-walking `launcher` was the alternative and was
rejected: it re-encodes the `$GRIM_HOME/hooks/{bin/grim-hook,dispatch.json}` layout in a third module
with an undecidable failure mode for a launcher path lacking a grandparent.

**Together with §1, the signature is now**
`hook_registration(&self, entry, event, launcher: &Path, table: &Path, root: &RootToken)` — this is
what the four blocked consumers (WP-J1 `hook_matrix_cell`, WP-I's registrar, WP-J2's install branch,
WP-L's parity test) should code against.

### F-I3 (Warn) — `command_windows` is still `None`

Codex `commandWindows` and copilot `powershell` need
`hook_launcher::registered_command_powershell`; calling it from production hits the same
unfulfilled-`expect` wall as F-I1, and `powershell_single_quote` is private to that module. A third
copy here would compound the duplication. `None` leaves both fields absent — the pre-hooks status quo
on Windows, not a wrong value — recorded inline at the field. Fill it in the change that reconciles
the generators; deleting one `cfg_attr` block unblocks both at once.

### F-I4 (Warn) — the C-018b *manifest-level* pinning test is still blocked on WP-A

Every success path through `hook_registration` consults `hook_tier_support` →
`crate::oci::hook::projection_for`, still a WP-A `unimplemented!()`. So "build a registration from a
metacharacter-laden manifest and assert a byte-identical command" cannot run yet; a comment in the
tests module says so, and it lands the moment WP-A's body does. C-018b is nonetheless provable now,
two ways: structurally (`registration_command`'s five parameters hold no `HookEntry`, so nothing
publisher-controlled is in scope to interpolate) and by the byte-exact + hostile-path tests.

### F-I5 (Suggest) — the `matcher` charset is re-checked at the seam

`is_exact_tool_name` also rejects any character outside C-018's `matcher_char_allowed`: the
build-time charset does not bind a `hook.toml` on disk (the W2(c) argument that re-checks
`MATCHER_MAX_BYTES` at read time), and "untranslatable" is the honest verdict for a character grim
never admitted. No shipped-shaped matcher is affected (`Bash`, `mcp__a__b`, `A|B` all pass).

---

## 3. What the four bodies do

### `classify_matcher` (C-025)

`None` → `All`; `""` → `Empty`; whole-string `"*"` → `All`; otherwise `ExactOrAlternation` iff every
`|`-separated alternative is an exact tool name, else `NotTranslatable`. An exact name is non-empty,
`matcher_char_allowed` throughout, and free of `*`, `?`, `.`. Each disqualifier is documented with
its reason: an **empty alternative** matches everything as a regex (over-broad, not lossy), `*`/`?`
mean different things in grim's dialect and claude's/codex's regex, and `.` is the sharp case
(`Read.md` would match tools the author never named). `Ba*` is `NotTranslatable`, not a prefix glob.

### `matcher_may_select_shell_command_tool` (Decision K)

The documented rule table verbatim: `All` / `Empty` / `NotTranslatable` → **true**;
`ExactOrAlternation` → true iff some alternative is a **case-insensitive prefix** of a roster tool
(`starts_with_ignore_ascii_case`, byte-wise so no char-boundary panic is possible). Both relaxations
are the F-3 residuals: claude/codex are start-anchored but tail-open (`Ba` fires on `Bash`), copilot
PascalCase matches literal names case-insensitively (`bash` fires on `Bash`). Direction asserted:
`Ba` selects `Bash`, `BashOutput` does **not**. The table's one asymmetry — a client with no roster
row answers `true` for the three unconditional forms and `false` for a named matcher — is implemented
as written and documented as deliberate: it is what keeps an un-updated roster from silently
*admitting* a match-all mutator.

### `hook_tier_support` (C-021)

`hook_surface().is_none()` → `Declined`; then **one** lookup through
`projection_for(self.name(), event)` — never a scan of `RESPONSE_PROJECTION`, whose own doc forbids a
second reader — `None` → `Declined`; required field absent (`gatekeeper` → non-empty `verdict`,
`mutator` → `mutation`, `observer` → nothing) → `Declined`, never a weakened tier; `context` absent →
`Degraded`; else `Native`.

### `hook_registration` (C-005, C-018b, C-025)

Refusal order exactly as documented, Decision K ahead of the two matcher refusals: `NoSurface` →
`SurfaceUnimplemented` → `EventUnsupported` → `TierUnsupported` → `MutatorOnShellCommandTool` →
`MatcherEmpty` / `MatcherNotLossless`. `EventUnsupported` covers both ways a client can host no hook
at an event (no `hook_event_name`, no projection row) — naming is not support, so both are checked.
Nothing is bound from the surface: `OwnFile` and `SpliceConfig` produce the *same* registration (the
surface says who owns the file, not what it says); only the writer branches. `matcher`: `All` →
`None`, `ExactOrAlternation` → the authored string verbatim into the client's **structured** field.
`command`: `HookCommand::Shell` always — `Argv` is never constructed in v1. `timeout`:
`entry.timeout` unchanged.

The emitted string, byte for byte (now also proven equal to `hook_launcher`'s):

```sh
L='/abs/…/hooks/bin/grim-hook'
[ -f "$L" ] && [ -x "$L" ] || exit 0
"$L" run --client claude --event PreToolUse --table '/abs/…/hooks/dispatch.json' --root <32 hex>
s=$?
case "$s" in 0) exit 0 ;; *) exit 0 ;; esac
```

`-f` ahead of `-x` (a directory carries the exec bit); **no `exec`** (the status must survive for
`s=$?` + `case`); both paths POSIX **single-quoted at the assignment/argument site** with `'` →
`'\''` (a double-quoted literal still expands `$(…)` and backticks — WP-P0 ran the payload);
`client`/`event`/token unquoted because each is a closed grim-chosen set; absolute, never
`${GRIM_HOME:-…}`; no `$PATH` fallback; no trailing newline. `to_string_lossy` on a non-UTF-8 path is
the fail-safe direction — the replacement character makes `[ -f "$L" ]` false, so the hook does not
fire rather than firing on an unresolved path (the neighbouring control-character case is
`arming_refusal`'s, upstream).

---

## 4. `expect(dead_code)` bookkeeping

Deleted, each because its item went live and an unfulfilled `expect` is an error under
`-D warnings`: `HookDecline` (all seven variants now constructed), `MatcherForm`, `classify_matcher`,
`shell_command_tools`, `matcher_may_select_shell_command_tool`, `posix_single_quote`. Kept:
`HOOK_MARKER_KEY` (WP-I's splice writers remain its only consumer) and the four `allow(dead_code)`
attributes on the `Vendor` hook methods (WP-J2 is the first production caller, per their stated
removal trigger). `HOOK_MARKER_VALUE`'s attribute arrived deleted in the merge and stays deleted.
`registration_command` and the three new private helpers carry **no** attribute.

The comment above `SHELL_COMMAND_TOOLS` was updated: the const used to be reachable only because the
`expect` on `shell_command_tools` made that function a live root; it is now read through a live chain
rooted at `hook_registration`'s `allow`, so an attribute there would still be unfulfillable — for
the ordinary reason instead of the subtle one.

---

## 5. Tests added (6, all passing)

| Test | Pins |
|---|---|
| `classify_matcher_admits_exactly_the_three_translatable_forms` | C-025's table incl. `Ba*`/`Bas?`/`Read.md`/`Bash\|` → `NotTranslatable`, `$(id)` → `NotTranslatable` |
| `decision_k_predicate_follows_the_documented_rule_table` | every rule-table row, prefix direction (`Ba` yes, `BashOutput` no), copilot case-insensitivity, the empty-roster asymmetry |
| `registration_command_is_the_documented_five_lines` | the byte-exact string, plus named asserts for no-`exec`, `-f` before `-x`, no `GRIM_HOME`, no `$PATH` probe, no trailing newline |
| `registration_command_single_quotes_a_hostile_grim_home` | `$(touch …)`, backtick, space and `'` in the path — assignment line asserted verbatim with `'\''` |
| **`the_two_command_generators_agree_byte_for_byte`** | **F-I1's guard** — byte equality with `hook_launcher::registered_command` for claude/codex/copilot × benign and hostile paths |
| `hook_registration_declines_every_client_without_a_hook_surface` | the fail-safe gate over `ClientTarget::ALL` (15 clients) for `hook_registration` **and** all 12 `(tier, event)` pairs of `hook_tier_support` |

A test-local `token(hex)` mints a `RootToken` through its `Deserialize` (the dispatch table's own read
path) so a byte-exact expectation is possible **without** adding a production constructor that would
weaken the type — the real mint is a random-keyed HMAC. Documented at the helper.

Owed to Specify: F-I4's manifest-level C-018b pinning test and the `Native`/`Degraded`/`Declined`
tier matrix over the twelve real projection rows (both blocked on WP-A `projection_for`).

---

## 6. Gates

| Gate | Result |
|---|---|
| `cargo check --all-targets` | clean |
| `cargo clippy --locked --all-targets -- -D warnings` | clean |
| `cargo test --bin grim` | 2735 passed, 0 failed |
| `cargo fmt` | applied; `cargo fmt --check` clean |
| `task --force verify` | **exit 0** (lint + build + 2735 unit + 1019 acceptance + `claude:tests`) |

Working tree holds `src/install/vendor.rs` only — the two `uv.lock` files the pytest runs rewrite to
the local Artifactory mirror were reverted after each run. Nothing committed, nothing pushed, `main`
untouched.
