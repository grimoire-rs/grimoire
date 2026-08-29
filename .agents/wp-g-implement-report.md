# WP-G — Implement report: registry trust resolution + audit trail

**Worktree:** `.agents/worktrees/impl-g` · **Branch:** `hex/hooks-artifact-kind--impl-g` (base `0387aa7`)
**Phase:** Implement · **Uncommitted, not pushed, `main` untouched** · **Date:** 2026-08-17
**Contracts:** C-022 (B4, B5, B7-consumer, W8), C-023 (W5), C-012 · *deferred: W7's text, W3-docs*
**Files:** `src/hook/trust.rs`, `src/hook/audit.rs`, `src/config/registry_resolve.rs` (one word).
`src/hook.rs` and `src/main.rs` needed **no** change — the stub's module root and `mod hook;`
line were already correct, so this phase touched neither.

`task verify` — **PASS** (full gate: format, clippy `-D warnings`, build, 2689 unit tests,
1019 acceptance tests, `claude:tests`, `catalog:verify`, `shell:verify`, link lint).

---

## Verdict first — what is wrong in merged code or in the contracts

Six findings. None blocks the WP; two are handoffs that will bite a later WP if ignored.

### F10 — Warn, **fixed** (scope widened by the lead 2026-08-17)

`src/config/registry_resolve.rs:66-70` said the dedup key is *"Used only as the `seen` key"*,
which stopped being true the moment `hook::trust::grants` / `is_bare_host` / `is_loopback`
consumed it. The sentence now names both uses and why they must share one normalization. Still
nothing else in that file beyond this line and the `fn` → `pub fn`.

### F16 — Block on the docs, **fixed** (found by the WP-I worker, verified by the lead)

`prompt_for_registry`'s and `persist_grant`'s `expect` reasons named
*"the hook arming path in `sync_config`"* as the caller. That is the wrong architectural layer:
`Vendor::sync_config` is invoked from six sites (`install::installer`, `command::uninstall`,
`command::update`, three in `tui::app`) and **every one is inside a per-client loop**, so
prompting there would ask up to three times for one consent and persist up to three times —
breaking C-023's *once* and contradicting this module's own `Arming::ConsentRequired`, which
exists to keep the prompt in exactly one place.

**Docs corrected, no code change** (every function in the chain is still uncalled, so there was
nothing to re-wire): `prompt_for_registry` gains a third caller obligation stating the layer —
the command boundary, above the per-client loop (WP-J2's `installer.rs`, or WP-K) — with the
cardinality argument spelled out, because that is what stops the next reader putting it back;
`persist_grant`'s doc points at it; both `expect` reasons now say *"…above the per-client loop …
NOT from `Vendor::sync_config`"*.

**Still saying `sync_config`, and left alone deliberately** — outside the two functions the lead
named: `arming`'s reason (`trust.rs:383`), `interactivity`'s (`:513`), and the shared enum reason
on `LocatorKind` / `NotArmedReason` / `GrantSource` (`:100`, `:207`, `:234`). `interactivity`'s is
the same defect in miniature — its own doc already says *"call it once at the command
boundary"*, which its `expect` reason then contradicts. One sweep of the same phrase closes all
five; say the word and it is one edit.

The lock obligation on `persist_grant` is unchanged and stands (F12).

### F11 — Warn. The brief's zero-grep invariant is unachievable as literally written

`grep -rn "GRIM_ALLOW_HOOKS\|GRIM_EXPERIMENTAL_HOOKS" src/` returns **5**, not 0 — and all five
are pre-existing prose in **merged** code stating that the variables do not exist:
`src/hook.rs:23-24`, `src/hook/trust.rs:282`, `src/config/declaration.rs:161-162`. This phase
added none.

The invariant that actually holds, and the one worth testing, is **zero reads**:
`grep -rn "env::var\|std::env" src/hook.rs src/hook/` returns only the three doc-comment
references to `crate::env::grim_home` that say *nothing here may call it*. No hook environment
variable is read anywhere, and no path in these modules consults the environment at all — the
only ambient read in the whole WP is `interactivity`'s two `is_terminal()` calls.

### F12 — Block **for WP-I**, resolved here as a documented caller obligation. `persist_grant` cannot take its own lock

`persist_grant` is a read-modify-write of the **global** `grimoire.toml` (load → mutate the
`[[registries]]` array → `write_config` re-serializes the **whole file**). Its stub signature
returns `ConfigError`, and `lock::file_lock::ConfigFileLock::try_acquire` returns a `LockError`,
which is not convertible — so acquiring the lock inside would mean widening the return type of an
already-reviewed signature.

Implemented the way `command::config::commit_config` already works: the lock is the **caller's**
obligation, stated in the function's doc.
**WP-I must wrap the call in `scope_resolution::lockable_path` + `ConfigFileLock::try_acquire`.**
Without it, two grim processes granting trust concurrently are last-writer-wins on the *entire*
global config — which silently deletes the loser's declarations, not just its grant. Note the
project-scope lock a `grim install` already holds is a **different file's** lock and does not
cover this.

### F13 — Warn, handoff to **the WP-G Specify worker**. `#[expect(dead_code)]` and a test-only caller are mutually exclusive under `--all-targets`

No unit tests were added in this phase, deliberately, and the reason is structural rather than a
preference: the gate is `cargo clippy --locked --all-targets -- -D warnings`, so both the
production and the `cfg(test)` builds are linted. An item that is dead in production and used by
a test **unfulfills** its `#[expect(dead_code)]` in the test build (hard error), while deleting
the attribute makes `dead_code` fire in the production build (hard error). There is no attribute
placement that satisfies both.

So WP-G's Specify tests must land **either** together with the deletion of the remaining
attributes and their real call sites (WP-I's `sync_config`, WP-K's runtime), **or** as `test/`
acceptance tests that drive the binary instead of calling the functions. The nine attributes this
phase removed were exactly the ones the compiler declared unfulfilled once the bodies landed —
found by the gate, not by reading, which is the argument for `expect` over `allow` restated.

### F14 — Suggest. An accepted prompt writes an **alias-less** entry, so `grim config registry` cannot address it

`persist_grant` appends `[[registries]]` with `oci` + `trust_hooks = true` and no `alias`
(inventing one risks colliding with a user's). C-022's promise — *"revocable by editing a
file"* — holds, and `grim context` still lists it. But `grim config registry set|show|rm <alias>`
and `config unset registry.<alias>.trust_hooks` all key on an alias, so the *CLI* revoke path
does not reach a grant grim itself wrote. Worth a docs sentence (WP-M) or an alias-derivation
decision (owner); not a defect in the contract as written.

### F15 — Suggest, and a near-miss worth recording for WP-P. W8's loopback exemption must not reuse the transport helper

The obvious implementation of `is_loopback` is
`oci::access::registry_client::plain_http_hosts()`, which is grim's existing "which hosts do we
reach over plain HTTP" list. It **unions `GRIM_INSECURE_REGISTRIES`** into that set. Reusing it
would have made an environment variable — routinely repo-carried, the exact CWE-426/B6 class this
feature deleted two variables to close — turn an `insecure = true` entry into an implicit hook
grant. `is_loopback` is therefore a pure, ambient-free match on `localhost` / `127.0.0.1` (bare
or with a port), with the reasoning in its doc comment so nobody "de-duplicates" it later. IPv6
`[::1]` is deliberately unmatched: an unmatched host simply needs an explicit
`trust_hooks = true`, which is the fail-safe direction.

---

## What landed

### `src/hook/trust.rs`

| Item | Behaviour |
|---|---|
| `decide` | Pure, **not** a first-match scan. Per entry: `grants` must match first (an entry that does not name the artifact neither grants nor denies), then a matching `trust_hooks = Some(false)` returns `OptedOut` **immediately** — so a deny beats a grant at any position, in either scope, at either locator kind. Otherwise the six conjunctive conditions fold into one `granted` flag: global scope **and** `Oci` kind **and** matched **and** (not bare-host or explicit `true`) **and** (not `insecure`, or explicit `true`, or loopback). Ends `Trusted` / `NeedsConsent`. |
| `arming` | `feature_enabled` first (`FeatureOff`), then `OptedOut` → `RegistryOptedOut` **ahead of** `allow_hooks` (F6's conservative reading, confirmed by the owner; the `⚠ Owed` block is kept verbatim), then `allow_hooks` → `Armed(AllowHooksFlag)`, then `Trusted` → `Armed(GlobalConfigEntry)`, then `NeedsConsent` → `ConsentRequired` when interactive else `NotArmed(NoTtyToAsk)`. |
| `grants` | Path-segment-boundary prefix over **`registry_resolve::normalize_locator`** — one spelling of the normalization, reused, not re-implemented. `candidate == pattern`, or `strip_prefix(pattern)` whose remainder `starts_with('/')`. So `ghcr.io/acme` grants `ghcr.io/acme` and `ghcr.io/acme/shell-guard`, and denies `ghcr.io/acme-evil/*` and `ghcr.io/other/*`. An empty locator, registry, or repository never grants (an empty pattern satisfies a prefix rule; the lock pin never is one). |
| `is_bare_host` | Normalized, scheme stripped, no `/` in the remainder. `localhost:5000` is a bare host — a port does not make a namespace. |
| `is_loopback` | Pure `localhost` / `127.0.0.1` (bare or ported) match. See F15. |
| `interactivity` | `stdin().is_terminal() && stderr().is_terminal()` — W5. The only ambient read in the module. |
| `prompt_for_registry` | Three lines + the question on **stderr**, `[y/N]`, naming the registry (never the artifact), what a hook is, the exact key it will add, the exact file, that declining changes nothing, and `--allow-hooks` for non-interactive runs. `y`/`yes` case-insensitive accept; blank, anything else, and EOF all decline. The registry is `escape_debug`-ed on the way to the terminal, as `validate_registries` does for an authored locator — a raw ESC or U+202E in a value that arrived from a lock pin repaints the line the user is answering. |
| `persist_grant` | `GlobalConfig::load` → find an entry whose **normalized `oci` equals** the namespaced target → set `trust_hooks = Some(true)` in place, else append `RegistryConfig { oci, trust_hooks: Some(true), .. }` → `validate_registries` → `command::add::write_config`. Matching is locator **equality**, not `grants`' prefix rule, because the bare-host case reaches here precisely *because* a prefixing entry did not grant: B5.2 requires the recorded answer to be the namespaced one the user was asked about, so a bare `ghcr.io` entry gains a namespaced sibling rather than a `trust_hooks = true` that would widen it to every publisher on the host. |
| `namespaced_locator` (new, private) | `<registry>/<first repository segment>`; a slash-less repository records the whole name — still narrower than the host, which is the property that matters. |
| `strip_scheme` (new, private) | Shared by `is_bare_host` / `is_loopback` so the scheme is dropped in exactly one place. |

**Not reached for, by construction:** `resolve_registries` is not imported (F8's
unrepresentable-wrong-input stands — `decide` takes `&[AuthoredRegistry]`), and nothing in the
module reads the environment or `env::grim_home`.

### `src/hook/audit.rs`

| Item | Behaviour |
|---|---|
| `sanitize` | Every `char::is_control()` (Unicode Cc = C0 + `DEL` + C1, exactly the documented closed set) → the visible literal `\u{XXXX}`. Not an allowlist: a Unicode hook id survives intact. |
| `AuditRecord::new` | Sanitizes `hook_id`, `client`, `digest` and every `changed_fields` entry, then **sorts** `changed_fields` (post-sanitization, so the order is the one a reader sees, and two records for the same rewrite are byte-equal). Stamps `AUDIT_SCHEMA_VERSION`, the timestamp via the existing **`lock_io::now_rfc3339`** (one instant format across every file grim writes, rather than a second `chrono` format string), and a 12-hex correlation id = SHA-256 over `timestamp ␟ pid ␟ hook_id ␟ event`, unit-separator-delimited so no field's content can impersonate a boundary. |
| `AuditLog::append` | `create_dir_all(parent)` → `rotate_if_needed` → `capped_line` → one `write_all` of line + `\n` through `OpenOptions::append(true)`, so two concurrent `grim hook run` processes interleave whole records instead of tearing one, with no lock. |
| `AuditLog::rotate_if_needed` | Absent ⇒ `Ok(())`; `len() < MAX_LOG_BYTES` ⇒ `Ok(())`; else `rename` to the `ROTATED_SUFFIX` sibling, which replaces any previous generation on every platform grim targets. Trail bounded at `2 × MAX_LOG_BYTES` with no cleanup job. |
| `capped_line` (new, private) | **Truncate, not drop.** Encode; over `MAX_RECORD_BYTES` ⇒ `changed_fields` → `["<elided>"]`, re-encode; still over ⇒ `hook_id` → `<elided>`; still over ⇒ `digest` and `client` too, and that encode returns unconditionally — every remaining field is a fixed-size enum, integer, timestamp or short digest, so the result is bounded by construction. The cap is measured **including** the newline (one byte conservative). |
| `rotated_path` / `ELIDED` / `CORRELATION_ID_HEX_LEN` (new) | Path suffix appended via `OsString` so `hooks.jsonl` rotates to `hooks.jsonl.1` (never `hooks.1`) and a non-UTF-8 path round-trips. `ELIDED` is a literal, not a truncation: a partial `hook_id` reads like a *different* hook id. |

**The tier-aware fail-closed contract is unchanged and unimplemented here, by design** — it is
WP-K's runtime leg. `append`'s `# Errors` section carries all three rows verbatim
(observer/gatekeeper → do not spawn, no verdict, `NotSpawnedUnlogged`; mutator → spawn then
discard the rewrite, `RewriteDiscardedUnlogged`, verdict still `Some(Mutate)`) plus **never a
non-zero exit and never a deny in any row**. The `timestamp` field keeps its
beyond-the-contract-text flag.

---

## Scaffolding discipline

Nine `#[expect(dead_code, …)]` attributes deleted — the exact set the compiler reported as
**unfulfilled** once the bodies landed: `TrustDecision`, `Interactivity`, `Arming`, `ArmingQuery`,
`decide`, `ConsentAnswer`, and the four `audit` constants (`AUDIT_SCHEMA_VERSION`,
`MAX_RECORD_BYTES`, `MAX_LOG_BYTES`, `ROTATED_SUFFIX`). `grants`, `is_bare_host` and `is_loopback`
lost theirs with their bodies.

Still attributed, correctly, because their only call sites are WP-I's and WP-K's:
`LocatorKind`, `NotArmedReason`, `GrantSource`, `arming`, `interactivity`,
`prompt_for_registry`, `persist_grant`, `AuditVerdict`, `AuditOutcome`, `AuditRecord::new`,
`AuditInput`(reachable, no attribute), `AuditLog::at`, `AuditLog::path`, `AuditLog::append`.
Each reason still names its REMOVAL TRIGGER.

New private items carry **no** attribute and need none: they are reachable from the public
functions above, so no diagnostic fires and an expectation would itself be unfulfilled — the same
reasoning the stub recorded for `AuthoredRegistry`.

---

## Gates

| Gate | Result |
|---|---|
| `cargo check --all-targets` | clean, zero warnings |
| `cargo clippy --locked --all-targets -- -D warnings` | clean |
| `cargo test --bin grim` | **2689 passed; 0 failed** (identical to the stub baseline) |
| `cargo fmt` | applied; `--check` clean |
| **`task verify`** | **PASS** — incl. 1019 acceptance tests, `claude:tests`, `catalog:verify` |

`test/uv.lock` and `.claude/tests/uv.lock` were rewritten by `uv` during the acceptance run (this
machine resolves PyPI through an internal Artifactory mirror, so every URL changed). **Reverted** —
the working tree holds exactly the three intended files.

---

## Handoffs

| To | What |
|---|---|
| **WP-J2 / WP-K** (the consent layer, **not** WP-I's `sync_config` — F16) | Compose at the command boundary, above the per-client loop: `interactivity()` once → `arming` → on `Arming::ConsentRequired` → `prompt_for_registry` → on `Accepted` → `persist_grant`. **F12 — hold the global config lock across `persist_grant`** (`scope_resolution::lockable_path` + `ConfigFileLock::try_acquire`; a `grim install`'s project-scope lock guards a **different file**). An `Err` from either degrades to *not armed, exit 0* — never a hard failure, never a deny — and a grant that could not be recorded **must not arm**. |
| **WP-K** | `AuditRecord::new` is the only construction path (a struct literal bypasses sanitization). Read `append`'s `# Errors` before implementing the fail-closed leg: three tier rows, exit 0 in every one. `AuditLog::append`/`rotate_if_needed` do **blocking** `std::fs` I/O in sync functions — the same shape as `write_config`/`atomic_write`, which async commands already call inline; if the runtime reaches them from an async task, wrap in `spawn_blocking` at the call site. |
| **Specify (WP-G)** | **F13 first** — the `expect(dead_code)`/`--all-targets` collision decides *where* these tests can live. Then: B4's five rows incl. "a project entry alone arms nothing"; B5's four cases; W8's `insecure` case + the loopback exemption; W5's stdout-piped-with-stdin-a-TTY case and prompt-on-stderr; C-012's record shape, sanitization, cap ladder and rotation. Two asymmetries worth their own tests, both deliberate: a **bare-host** entry cannot grant but its `trust_hooks = false` **does** deny every publisher on that host (fail-safe), and an **`index`** entry's `trust_hooks = false` is inert because its locator can never match an `oci` candidate. |
| **WP-M** | F10 (the stale `normalize_locator` doc), F14 (an alias-less granted entry is not CLI-addressable), and W7's remaining half — that a grim older than this release exits 78 on a config carrying `trust_hooks`, plus the `docs/src/stability.md` note. The prompt deliberately does **not** invent that sentence: it names a release version this module cannot know. The gatekeeper-is-not-a-boundary statement still owes a user-facing doc page. |
| **WP-P (audit)** | F15 — why the trust path must not reuse `plain_http_hosts`. Accepted residuals restated: no exec-time re-hash (C-009), tamper-**evidence** not prevention (I5), and the gatekeeper tier is not a security boundary. |
