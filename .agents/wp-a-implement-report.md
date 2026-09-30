# WP-A — Implement phase report

Worktree `.agents/worktrees/impl-a`, branch `hex/hooks-artifact-kind--impl-a`, base `7873f20`.
Files touched: `src/oci/hook.rs`, `src/oci/artifact_kind.rs`. **Nothing committed.**

## 1. The urgency premise is wrong — and the live panic is NOT in my file set

The task framed `HookManifest::from_toml_str` (`src/oci/hook.rs`) as *"REACHABLE TODAY and
panics"* via `grim build <dir containing hook.toml>`. **Executed, that is not the panic site.**

```
$ grim build ./shell-guard        # dir carries hook.toml + guard.sh
thread 'main' panicked at src/command/build.rs:121:5:
not implemented: WP-H stub: hook.toml read + validate + pack + annotate (C-001)
EXIT=101
```

`detect_kind` (`build.rs:72`) correctly resolves the directory to `ArtifactKind::Hook`, then
`validate_and_pack` dispatches to `pack_hook_dir` (`build.rs:114-122`) — **a WP-H stub whose own
`unimplemented!()` fires before `hook.rs` is ever entered.** `from_toml_str` had **no caller
anywhere in the tree**: `rg 'from_toml_str' src/` returns hits in `mcp.rs` and
`project_config.rs` only. Its `unimplemented!()` was **unreachable**, not live.

Consequences the orchestrator needs:

- **Exit 101 on `grim build <hook dir>` is unchanged by this fold, and cannot be changed from
  WP-A's file set.** `build.rs` is WP-H's. The Block-tier reachable panic is WP-H's
  `pack_hook_dir` (plus `annotations_for_hook`, `src/oci/annotations.rs:536`, same wave).
- The sibling worker's `grim status` → `exit 101` is therefore also very unlikely to be
  `hook.rs`; whichever stub it hit should be re-attributed by reading the panic line rather than
  inferred from the kind involved.
- What *is* reachable in `hook.rs` today and now exercised end-to-end: `grim schema --kind hook`
  (`schema.rs:121` → `schemars::schema_for!(HookManifest)`) — **exit 0**, emits the manifest
  schema including `CanonicalEvent`, `HookTier`, `HookPayloadMode` and the flattened handler.

`from_toml_str` is implemented regardless — it is WP-H's immediate dependency and will be live the
moment `pack_hook_dir` lands, so the ordering is right; only the "panicking in a released command
today" characterisation was not.

## 2. Second finding — a value-level round-trip test HIDES the `policy` datetime corruption

The stub doc (and § 5.3 risk 2) says a TOML datetime under `policy` *"re-serializes as a nested
table leaking that private sentinel"*. Correct, but the obvious test for it passes:

`{"$__toml_private_datetime": "2026-08-14"}` is **itself stable** across
serialize → parse, so `HookManifest == HookManifest` holds and a value-equality round-trip test is
**green on a corrupted document**. The corruption is only visible in the emitted **text** —
`since = 2026-08-14` becomes a nested table. `a_toml_datetime_under_policy_does_not_round_trip`
asserts on the text for that reason, with the value-equality assertion kept beside it as the
explicit statement of why it is not the check that matters. **WP-M's format doc and whoever adds
the build-time refusal must not "verify" this with a value round-trip.**

## 3. Third finding — WP-F had already filed a rule-3 obligation nobody had routed to WP-A

`HookDecline::MatcherEmpty` (`src/install/vendor.rs:302-309`) reads: *"**Owed upstream:**
`grim build` should reject it as a manifest error (`HookManifest::validate` rule 3), which would
make this variant unreachable."* That obligation appears in no WP-A bullet in the plan.
Implemented: `validate_matcher("")` → **new `HookError::MatcherEmpty`** (additive variant on an
already-`#[non_exhaustive]` enum classified wholesale as `DataError`; no consumer edit, no exhaustive
match anywhere — `error.rs:222` is `Error::Hook(_)`). WP-F's variant survives as the seam's
backstop, now unreachable from a built artifact rather than from a hoped-for convention.

## 4. What was implemented (all 8 bodies)

| Item | Shape |
|---|---|
| `CanonicalEvent::admits_verdict` | query over `RESPONSE_PROJECTION`: some row for the event has a non-empty `verdict` |
| `CanonicalEvent::admits_mutation` | same query on `mutation.is_some()`; **not** a `match` on the variant, so C-021's table stays the single source. A test pins that it equals `PreToolUse`-only, so a survey error that put `mutation` on a later event fails loudly instead of widening `mutator` |
| `HookTier::is_valid_at` | `observer` → true; `gatekeeper` → `admits_verdict`; `mutator` → `admits_mutation`. No third statement of the rule |
| `HookHandler::first_token` | `argv[0]`, or `split_whitespace().next()` for `command` — the shell that runs the string splits on blank *runs*, so a literal `split(' ')` would yield `""` for `"  sh x.sh"` and skip C-019 entirely |
| `HookManifest::from_toml_str` | strict parse, plus a shape-agnostic `toml::Value` probe for the two errors the strict parse cannot express (below) |
| `HookManifest::validate` | all 9 rules, ordered 8 → 7 → (per entry) 5 → 1 → 6 → 9/2 → 3 → 4 |
| `validate_matcher` | empty → cap → charset, in that order |
| `projection_for` | the one lookup into `RESPONSE_PROJECTION` |

**Parse strictness.** `deny_unknown_fields` on `HookManifest` is untouched and tested. Two errors
are re-mapped into the author's vocabulary, and both come from the probe:

- **`UnsupportedSchema` (S-014)** is decided **before** the strict parse, so a v2 manifest gets the
  version error rather than a field error about a key it authored correctly for v2. A `schema` that
  is not a `u32` (`"1"`, `-1`) deliberately falls through to the TOML error, which names the real
  value — that is a malformed value, not an unsupported version, and inventing
  `found: 4294967295` for it would be a worse diagnostic.
- **`MissingHandler`** is recovered **structurally** (an entry table carrying neither `argv` nor
  `command`), never by matching serde's `no variant of enum HookHandler found in flattened data`.
  That string is a dependency's internal message; a `serde` reword would silently regress to the
  bare parse failure S-014 forbids. An entry with no string `id` is skipped so the strict parse's
  own missing-field message speaks instead of a placeholder id.

**Nothing coerces.** Every rule returns a `HookError` (→ `DataError`, exit 65). No default is
substituted for a bad value anywhere.

**Untrusted-input discipline (T1).** No manifest value reaches a command, a path, or a shell from
this file. The single filesystem touch is C-019's `is_file()` probe under the artifact directory,
in `payload_relative_file`, which **refuses to probe at all** for anything absolute,
drive-prefixed, or carrying a `..` component — such a token is an interpreter path as far as this
rule is concerned, and grim has no business stat-ing it. `./x` normalises to `x`, so
`command = "./guard.sh"` is caught. `MatcherCharset` quotes the rejected matcher, so the cap is
checked first and the quoted value is always ≤ 256 bytes.

`MATCHER_ALLOWED` / `matcher_char_allowed` are used as merged: the predicate is the membership
test, the constant is only the diagnostic spelling, and a test asserts the tempting
`MATCHER_ALLOWED.contains(c)` is wrong in **both** directions so nobody "simplifies" into it.
`RESPONSE_PROJECTION` remains the only projection table; no field name is restated in prose.

**One error message reworded.** `ReservedClientKey`'s text said *"key '{0}' is reserved for a
per-client override table"*, which is false for the case it mostly fires on — a **typo'd**
namespace (`cursour.event`) is not reserved, it is unknown. Now: *"key '{0}' is not a per-client
override table: expected '<client>.<field>' naming a client grim supports"*, covering both that and
a client name holding a scalar. Nothing pins the old text (no test, doc, or catalog reference).

## 5. `#[expect]` / `#[allow]` discipline

No `#[expect(dead_code)]` was deleted, because none applies to the items filled: `hook.rs` carries
a **module-wide** `#![allow(dead_code, reason = …)]` whose stated REMOVAL TRIGGER is WP-K, the last
consumer. The items are implemented but still have no production caller, so the attribute stays and
must be deleted in WP-K exactly as its reason says. `artifact_kind.rs`'s per-item
`#[allow(dead_code)]` on `is_dir_artifact` is left alone deliberately: it is exercised only by
tests, so `#[expect]` there would fire `unfulfilled_lint_expectations` under
`--all-targets -D warnings`.

## 6. Tests added

`src/oci/hook.rs` — 27 tests. Highlights beyond the obvious per-rule cases:

- `parses_the_documented_example` / `round_trips_through_toml_with_the_handler_flattened` —
  § 5.3 risk 1, **exercised not assumed**: with two `#[serde(flatten)]` fields, `argv` is asserted
  to land in `handler` and the vendor bag to be empty, and the emitted document is asserted to
  carry a bare `argv = [` rather than a `handler = { … }` wrapper no third-party reader expects.
- `both_handlers_parse_and_then_fail_validation` — B1 empirically: both keys **parse**, `argv`
  wins, `command` lands in `vendor`, `validate` → `AmbiguousHandler`.
- `native_only_moment_admits_observer_and_gatekeeper_but_never_mutator` — asserts the refusal is
  `MutatorRequiresCanonicalEvent`, not `TierNotValidAtEvent`.
- `vendor_keys_must_name_a_supported_client_and_hold_a_table` — loops **all 18**
  `ClientTarget::ALL` names as legal namespaces, and rejects a typo, a scalar-valued client key,
  and a misspelled typed key (`timeoot`). This is what closes the hole `deny_unknown_fields`
  cannot.
- `matcher_charset_length_and_empty_are_refused` — includes the **bidi** (`U+202E`) and
  **zero-width** (`U+200B`) cases, which are the allowlist's actual reason for existing, not just
  the shell metacharacters.
- `running_the_payload_directly_is_refused_at_build` +
  `a_traversing_first_token_is_never_probed_as_a_payload_file` — C-019 in both directions, on a
  real temp payload tree, asserting the message teaches the interpreter form.

`src/oci/artifact_kind.rs` — extended `Hook` into the three existing tests, replaced two
hand-maintained 5-kind arrays with `ArtifactKind::ALL` loops (that array shape **is** D-5), and
added `all_is_complete_and_injective`: the runtime half of C-016(a), asserting `subdir` /
`kind_str` / `artifact_type` / `config_media_type` are each injective across `ALL`. The `const`
block cannot catch a copy-pasted `all_index` arm returning a duplicate index, which is precisely
the escape its own doc comment describes.

## 7. Gates — all executed, all clean

| Gate | Result |
|---|---|
| `cargo check --all-targets` | clean |
| `cargo clippy --locked --all-targets -- -D warnings` | clean |
| `cargo test --bin grim` | **2717 passed, 0 failed** |
| `cargo fmt` | applied, no diff after |
| `task --force verify` | **exit 0** — 51 claude structural, 2717 Rust (nextest), 1019 pytest |
| `grim build ./shell-guard` (executed) | **exit 101**, `build.rs:121`, WP-H's stub — see § 1 |
| `grim schema --kind hook` (executed) | **exit 0** |

`git status` is exactly the two files in my set. The `uv.lock` files that `task verify` touched as
a side effect were reverted.

## 8. Handed forward

1. **WP-H** — `pack_hook_dir` (`build.rs:114`) and `annotations_for_hook`
   (`annotations.rs:536`) are the live Block-tier panics on the hook path. `from_toml_str` +
   `validate` are ready for both; `validate` wants the artifact **directory** (its `file_name()` is
   the stem rule 7 compares against).
2. **WP-H / WP-M** — `grim build` still owes a refusal for a TOML datetime / local-date /
   local-time under `policy` or a vendor key (§ 2). Verify it on the **emitted text**.
3. **WP-F** — `HookDecline::MatcherEmpty` is now unreachable from a built artifact (§ 3); keep it
   as the backstop, and note `validate_matcher` deliberately does **not** judge interior `*`/`?`
   losslessness — that is C-025's per-client question, not a build-time one.
4. **WP-K** — deleting `hook.rs`'s module-wide `#![allow(dead_code)]` is still owed, per its own
   REMOVAL TRIGGER.
