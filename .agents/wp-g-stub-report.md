# WP-G — Stub report: registry trust resolution + audit trail

**Worktree:** `.agents/worktrees/wp-g` · **Branch:** `hex/hooks-artifact-kind--wp-g` (based on `9c82115`)
**Phase:** Stub · **Uncommitted, not pushed** · **Date:** 2026-08-17
**Contracts:** C-022 (B4, B5, W8), C-023 (W5), C-012 · *deferred: W7, W3-docs*
**Withdrawn and not built:** C-026, B6, W6 — both hook environment variables are deleted.

---

## Verdict first — what is wrong in the plan, the contracts, or merged code

Nine findings. **Adjudicated by the lead 2026-08-17; all plan corrections are applied on the
feature branch.** Current state:

| # | Severity | Status |
|---|---|---|
| **F1** | Block | **Accepted** — `src/hook.rs` created; the plan's file cell now names it |
| **F2** | Block | **Accepted, assigned to WP-G for the Implement phase only** — `src/config/registry_resolve.rs` joins the file set for the `fn` → `pub fn` diff. Not touched during Stub |
| **F3** | Warn | **Accepted** — the plan now points to `adr_hooks_support.md:1377-1391` as C-012's only definition, inline in the Specify clause |
| **F4** | Block | **SETTLED — tier-aware, and neither reading I proposed.** `audit.rs` adjusted; see below |
| **F5** | Warn | **Accepted** — `timestamp` stays, flagged beyond-contract in its own field doc |
| **F6** | Owed | **Conservative reading stands** as the decision; the `⚠ Owed` block stays, surfaced to the owner as a decision taken |
| **F7** | Block | **Accepted, all three sites fixed** — rule 3 closed, inertness test struck with its reason, W6's env-form direction removed |
| **F8** | Suggest | Noted, no action |
| **F9** | Suggest | Open — WP-M's file set |

### F1 — Block (resolved by creating the file). WP-G's file set omits the module root, and the repo has no `mod.rs`

The declared set is `src/hook/{trust,audit}.rs`. `arch-principles.md` › Code Style Conventions
mandates **"named module files, no `mod.rs` files"**, and `find src -name mod.rs` returns nothing
— every subsystem is `src/<name>.rs` + `src/<name>/`. So `src/hook/trust.rs` and
`src/hook/audit.rs` are **uninstantiable** as written: `mod hook;` in `src/main.rs` needs
`src/hook.rs`.

I created **`src/hook.rs`** (module root, 58 lines, doc only + two `pub mod` lines). Justification
for stepping one file past the declared set rather than reporting and stopping:

- `grep -n "src/hook" plan_hooks_artifact_kind.md` returns exactly two hits, both **WP-G's own**
  (lines 1213, 1888). No other WP names any file under `src/hook/`, so the root is WP-G's by
  elimination and carries **zero merge-conflict risk**.
- Reporting without creating it would have left the WP with nothing that compiles, which is the
  Stub gate.

**Plan correction owed:** WP-G's `Expected files` cell should read
`src/hook.rs` + `src/hook/{trust,audit}.rs` (new), `src/main.rs`.

### F2 — Block, NOT resolved. B5 mandates reusing `normalize_locator`, which is private and in no WP's file set

C-022's B5 amendment (plan line 851) requires trust matching to be
*"case-normalized on the host and trailing-slash-normalized (**reuse `normalize_locator`**)"*.

`src/config/registry_resolve.rs:70` is **`fn normalize_locator`** — private, no `pub`. So
`trust::grants` and `trust::is_bare_host` **cannot be implemented from WP-G's declared file set.**

Worse: `grep -n registry_resolve` over the plan returns **four hits, all evidence citations**
(lines 820, 833, 1422, 2184) and **not one file-set cell**. `src/config/registry_resolve.rs`
belongs to **no work package**. WP-E owns `src/config/{declaration,hash,project_config}.rs`, not
this one.

I did **not** edit it. The fix is a one-word diff (`fn` → `pub fn`) and needs an owner. Cheapest
correct assignment: **add `src/config/registry_resolve.rs` to WP-G's set for the Implement
phase** — WP-G is its only new consumer, and the alternative (a second spelling of the same
normalization inside `src/hook/trust.rs`) is exactly how the browse filter and the TUI tree came
to disagree about one row.

### F3 — Warn. C-012 has **no definition anywhere in the plan** — only in the ADR

`grep -n "C-012" plan_hooks_artifact_kind.md` → six hits: WP-G's Stub line, WP-G's Specify line,
WP-K's Scope cell, and three narrative mentions. **Not one is a definition.** The record shape,
the redaction level, the sanitization rule, the size cap and the rotation obligation exist only
at `adr_hooks_support.md:1377-1391`.

Every other contract WP-G is scoped (C-022, C-023) is restated *and* amended in the plan with a
WP-P0 box. A Specify worker generating tests from the plan alone — which is the declared
contract-first flow — would produce **nothing** for C-012. The plan needs at least a pointer line
under § Contracts naming the ADR location.

### F4 — Block on the contract text. **SETTLED by the lead 2026-08-17: tier-aware, and neither of the two readings I proposed**

`adr_hooks_support.md:1390-1391`: *"a write failure fails **closed** for the audit (refuse to run
the hook) rather than silently proceeding unlogged."* I flagged two readings and argued for
"do not spawn, exit 0" over the literal "return a deny", which is an I3 violation with a DoS
consequence — **in scope**, since N5 covers only a slow hook the *user* installed, not grim
causing the denial.

The lead accepted the prohibition and **rejected the blanket fix**: "do not spawn, exit 0" for
every tier discards more than the invariant needs. The settled rule withholds the hook's
**effect, sized to the tier**, and never the agent's progress. In every row: warn on stderr,
exit 0.

| Tier | On an audit-write failure | `AuditOutcome` |
|---|---|---|
| `observer` | do not spawn | `NotSpawnedUnlogged` |
| `gatekeeper` | do not spawn; **no verdict**. Failing open is within contract — the tier is already declared not a security boundary | `NotSpawnedUnlogged` |
| `mutator` | **spawn, then discard the rewrite** — the tool call proceeds with its **original** input | `RewriteDiscardedUnlogged` |

The mutator row is what earns the split. An **unlogged rewrite** is the only genuinely dangerous
outcome in this failure mode — mutator control 5 exists so the agent's own transcript records
that its command was altered, and an unwritten trail defeats exactly that — so the *rewrite* is
dropped, not the invocation. Withholding a mutator's whole invocation would additionally withhold
side effects the user installed it for, and would buy no safety the discard does not.

**Encoded in `src/hook/audit.rs`:**

- The module doc's fail-closed section is rewritten as the three-row tier table, keeping the
  prohibition on the deny reading and the reason (Copilot `preToolUse` denies on any non-zero;
  Claude's `exit 2` *is* deny).
- `AuditOutcome::RefusedUnlogged` is **split** into `NotSpawnedUnlogged` and
  `RewriteDiscardedUnlogged`, because *the hook never ran* and *the hook ran and its rewrite was
  dropped* are different forensic facts and WP-K reads the distinction at the call site. Safe to
  rename rather than deprecate: the variant is unreleased stub surface with no consumer, so
  Principle 9's freeze does not reach it.
- `AuditRecord::verdict` documents that `RewriteDiscardedUnlogged` is deliberately **not** in the
  never-reached-a-verdict list: the payload did answer, so `verdict` stays
  `Some(AuditVerdict::Mutate)`. **`verdict` records what the hook said; `outcome` records what
  grim did with it.**
- `AuditLog::append`'s `# Errors` section carries all three rows plus "never a non-zero exit, and
  never a deny, in any row" — that is the doc a WP-K implementer reads at the call site.

The plan now also states the rule, so WP-K cannot land the deny reading against a permissive
contract.

### F5 — Warn. C-012's field list has no timestamp

The enumerated redacted view is *"hook id, event, client, tier, digest, which fields changed,
sizes, the decision verdict, a correlation id, outcome status."* No time. A forensic record with
no timestamp answers no forensic question, and the correlation id joins records **inside** the
trail to each other but joins nothing outside it — not a client's own transcript, not a CI job
log.

I added `AuditRecord::timestamp` (RFC 3339 UTC; `chrono` is already a direct dependency) and
flagged it **in the field's own doc comment** as an addition beyond the contract text rather than
folding it in silently.

### F6 — Warn, owed to the owner. No contract states the ordering between `--allow-hooks` and an explicit `trust_hooks = false`

B4's deny rule — *"any `trust_hooks = false` in any scope wins over every grant"* — is written
about **config scopes granting**. It says nothing about the per-invocation flag. N4 (*"a user
deliberately bypassing a gate they were shown"* is a non-goal) argues the flag should win.

I implemented the **conservative** reading: `TrustDecision::OptedOut` is answered **before**
`allow_hooks`, so an explicit per-registry opt-out is not overridden by a blanket flag. Rationale
recorded at `trust::arming`'s doc comment, with a `⚠ Owed` block naming the ambiguity: the
narrower explicit statement beats the blanket one, and this can be **loosened additively** later
under Principle 9 where the reverse could not. One line either way once the owner rules.

### F7 — Block. The plan's § WP-G section still specifies a test for the deleted environment variable, contradicting its own table

Three places in § WP-G still describe `GRIM_ALLOW_HOOKS` as live:

- line 1407-1408, inside **`Specify:`**: *"**a bare `GRIM_ALLOW_HOOKS=1` with no trusted registry
  and no `--allow-hooks` arms nothing** — the repo-carried-environment path"*
- line 1429-1434, rule 3: *"**WP-G owes the choice at its stub gate**"* … *"is the **inertness
  test** and survives under **both** options"*
- line 1443-1444: *"W6's env-form direction (whichever option is chosen, the *falsy disarms* half
  is tested either way)"*

The Parallelization table at line 1888 records the opposite and correct state:
`~~B6 GRIM_ALLOW_HOOKS disposition~~ **RESOLVED — deleted from the surface**`,
`~~C-026~~ **WITHDRAWN … no inertness test needed**`.

So the plan contradicts itself, and the surviving half is the one a Specify worker reads. Both
named tests are now **unwritable**: there is no variable to be inert and no env form to be falsy.

This is the third recurrence of the plan's own recorded lesson (line 2355: *"it amended contract
text and Scope cells and left the **`Specify:` lines** standing — under contract-first TDD the
Specify list is equally the text a test is generated from"*). § WP-G's Specify clause and rule 3
should be deleted, matching the table.

Per the settled decision and the brief, I built **no** environment-variable read and **no**
inertness test. `grep -rn "GRIM_ALLOW_HOOKS\|GRIM_EXPERIMENTAL_HOOKS" src/hook*` → zero hits.

### F8 — Suggest. B4's dangerous seam is still the obvious thing to reach for; the type system now says no

Not a plan defect — the plan warns about it explicitly. But `resolve_registries` remains the
natural call for "which registries are configured", and its output has already discarded both
distinctions the contract turns on. So the stub makes the wrong input **unrepresentable** rather
than merely discouraged:

`trust::decide` takes `&[AuthoredRegistry]`, which carries `scope: ConfigScope` and
`trust_hooks: Option<bool>`. `ResolvedRegistry` carries **neither**, so it cannot be passed. The
rationale is on `AuthoredRegistry`'s own doc comment ("*Feeding it to `decide` is the defect, not
an implementation shortcut*").

The A-10-style source-level import test that would *pin* this (no `resolve_registries` import in
`src/hook/trust.rs`) belongs with the trust resolution's Specify step.

### F9 — Suggest. `arch-threat-model.md`'s `paths:` does not cover `src/hook/**`

Its scope note anticipates `src/command/hook*` (WP-K's file, correctly absent until it exists),
but the consent gate lives in `src/hook/trust.rs` and nothing routes the threat model to it. Its
current globs are `src/oci/**`, `src/install/**`, `src/command/{login,logout,publish*,release*}`,
`catalog/**`.

A rule that does not fire on the file implementing the trust boundary is missing its most
security-sensitive path. `.claude/rules/**` is **WP-M's** file set, so this is flagged, not
edited — and it needs a matching row in `.claude/rules.md`'s "By auto-load path" table plus the
declared-overlap group in the same commit, or `test_catalog_covers_all_rules` fails.

---

## What was built

Three files. `src/hook.rs` is the one addition to the declared set (F1).

### `src/hook.rs` — module root

Doc only. States what is deliberately **absent** and unreachable from here, because each is
something a reader working from the ADR top-to-bottom would rebuild:

- **No approval store** — no per-hook record, no hash chain, no per-artifact key, no
  `hook_approvals.json` (owner decision reversing D5; the ADR's two `WITHDRAWN` banners exist for
  exactly this reason).
- **No environment variable** — neither hook variable exists; the three questions live in three
  places (config key, `trust_hooks`, `--allow-hooks`) with **no precedence rule left to get
  wrong**.
- **No `env::grim_home()`** — with the B1 reasoning (returns its value verbatim, no absoluteness
  check, relative `.grimoire` fallback when `HOME` is unset, and for a client-spawned
  `grim hook run` the CWD **is the workspace**).
- I3 and I5 stated once, at the top, in the form the submodules depend on.

### `src/hook/trust.rs` — C-022 + C-023

Module doc carries **B4's precedence table verbatim** (all five rows), the deny rule stated as
*not* a precedence rule, B5's identity question, and an explicit section stating that **the
gatekeeper tier is not a security boundary** — defence-in-depth a user may rely on for hygiene
and must not rely on for security.

| Item | Contract |
|---|---|
| `LocatorKind{Oci,Index}` | B5.3 — an `index` entry never grants for the hosts its pointers name |
| `AuthoredRegistry{scope,locator,kind,insecure,trust_hooks}` | B4 + B7 — the authored view, scope-tagged, tri-state `trust_hooks` consumed (WP-E defines it) |
| `TrustDecision{Trusted,OptedOut,NeedsConsent}` | C-022 |
| `Interactivity{Interactive,NonInteractive}` | C-023 / W5 |
| `NotArmedReason{FeatureOff,RegistryOptedOut,NoTtyToAsk,ConsentDeclined}` | every variant documented as an **exit-0** outcome (I3) |
| `GrantSource{GlobalConfigEntry,AllowHooksFlag,ConsentPrompt}` | so a durable config grant is distinguishable from a one-shot CI escape |
| `Arming{Armed,ConsentRequired,NotArmed}` + `ArmingQuery` | the composed verdict; every input is a **parameter**, nothing ambient |
| `decide()` | B4 scope precedence + B5 matching + W8, stated as six conjunctive conditions; documented as **not a first-match scan**, because the deny rule must see every entry |
| `arming()` | the ordering, with F6's `⚠ Owed` block |
| `grants()` | B5.1 path-segment-boundary prefix, with the five-row grant/deny table; carries F2's flag |
| `is_bare_host()` | B5.2 — a bare host is the whole shared multi-tenant registry |
| `is_loopback()` | W8's only exemption |
| `interactivity()` | **the only ambient read in the module**; stdin AND stderr both TTYs |
| `ConsentAnswer` + `prompt_for_registry()` | prompt on **stderr**, names the **registry** not the artifact; W7 recorded as a named deferral |
| `persist_grant()` | writes a **namespaced** `trust_hooks = true` into **global** config via `command::add::write_config` |

Design notes worth a reviewer's attention:

- **`ConfigScope` is reused** (`src/config/scope.rs`), not redefined. B4 turns on exactly the
  distinction that enum already carries.
- **`decide` is pure** — no I/O, no clock, no environment — so every row of B4's table and every
  case of B5's matching is a unit test with no terminal and no config file. `prompt_for_registry`
  and `persist_grant` are the only I/O, and `Arming::ConsentRequired` is what keeps the prompt out
  of the decision.
- **W5's stdout/stderr split follows `src/auth/prompt.rs`**, which made the same split for
  `grim login` for the same reason. Cited in the doc so the precedent is discoverable.
- `prompt_for_registry`'s `# Errors` says an I/O failure **must** be treated as `Declined` +
  exit 0 (I3), never as a hard failure and never as a deny.
- `persist_grant`'s `# Errors` says a grant that could not be recorded **must not arm** — it would
  arm again next run with no record of why.

### `src/hook/audit.rs` — C-012

Module doc: redaction rationale (Kubernetes audit levels, CloudTrail truncation, CWE-117 with
ANSI-escape spoofing and grim's own CVE-2025-58160, CWE-400), **full-body capture explicitly out
of scope with no level enum and no flag to flip**, F4's fail-closed disambiguation, and the
parameter-only path rule (B1).

- `AUDIT_SCHEMA_VERSION` / `MAX_RECORD_BYTES` (4 KiB, **truncate not drop**) / `MAX_LOG_BYTES`
  (8 MiB) / `ROTATED_SUFFIX` (one retained generation — bounded at `2 × MAX_LOG_BYTES` with no
  cleanup job to forget).
- `AuditVerdict{NoOpinion,Allow,Deny,Mutate}` — `Allow` documented as a **privilege statement**,
  not a no-op, because it suppresses the client's own tool-approval prompt.
- `AuditOutcome{Completed,NoMatch,TimedOut,SpawnFailed,ResponseRejected,NotSpawnedUnlogged,
  RewriteDiscardedUnlogged}` — the last two are F4's tier split.
- `AuditRecord` — JSONL, `Serialize`, no field carrying a payload body, a tool-input value, or a
  mutated command. `changed_fields` names what moved; nothing quotes it. `digest` documents C-009
  (the runtime hashes nothing) and the I5 tamper-**evidence** framing.
- **`AuditInput` / `AuditRecord::new(&AuditInput)`** — I replaced a ten-positional-argument
  constructor with a two-type split, so "sanitize on the way in" is a **type boundary** a
  reviewer sees rather than a comment a new call site can miss: `AuditInput` is where hostile
  bytes live (`hook_id` is publisher-authored, `changed_fields` is payload-derived),
  `AuditRecord` is sanitized by construction, and `new` is the only bridge. This also removed a
  `clippy::too_many_arguments` suppression.
- `sanitize()` — C0 + C1 + `DEL`, documented as **not** an allowlist (a hook id is legitimately
  Unicode; rejecting it would make the trail lossy about the artifact it describes).
- `AuditLog::at(PathBuf)` / `path()` / `append()` / `rotate_if_needed()` — holds a path, not a
  handle, because the trail is written once per short-lived `grim hook run` and a handle would
  hold a lock across the payload's whole lifetime.

### `src/main.rs` — the only pre-existing file touched

**One added line, nothing else.** No deletions, no modifications, no reordering. Verbatim
`git diff src/main.rs`:

```diff
@@ -24,6 +24,7 @@ mod env;
  mod error;
  mod fetch;
  mod glob;
+mod hook;
  mod install;
  mod lock;
  mod log_switch;
```

`mod hook;` in the crate root's alphabetical module list, between `glob` and `install`. Required
by F1: without it `src/hook/*.rs` is not part of the crate. Nothing else in the file — not the
`Command` enum, not `main`, not `init_tracing`, not the `use` block, not the test module — is
altered. `grim hook run`'s CLI wiring is **WP-K's** (`src/command/hook.rs` plus the `Command`
variant), and this WP added no subcommand.

---

## Scaffolding discipline

**`#[expect(dead_code, reason = "…")]`, never `#[allow]`** — 28 items (16 in `trust.rs`, 12 in
`audit.rs`), each reason naming a
**REMOVAL TRIGGER** and the WP whose call site fulfills it, following the merged
`src/install/vendor.rs` convention. `expect` fires `unfulfilled_lint_expectations` under
`-D warnings` the moment an item becomes used, so the wiring is compiler-proven rather than
comment-proven.

**One deliberate exception, and it is the interesting one.** `AuthoredRegistry` carries **no**
`expect`: it is already reachable from `decide`'s and `ArmingQuery`'s signatures, so no dead-code
diagnostic fires and an expectation there is itself unfulfilled — which fails
`-D warnings`. A comment on the struct records why the attribute is absent, so nobody "fixes" it
back in. (Found by the gate, not by reading — which is the argument for `expect` over `allow` in
one line.)

Bodies are `let _ = (params); unimplemented!("WP-G stub: … (C-0xx)")`, matching
`src/install/vendor.rs` and `src/oci/hook.rs`.

---

## Gates — all four clean

| Gate | Result |
|---|---|
| `cargo check --all-targets` | **clean — zero warnings** |
| `cargo clippy --locked --all-targets -- -D warnings` | **clean** |
| `cargo test --bin grim` | **2689 passed; 0 failed** |
| `cargo fmt --check` | **clean** |

Re-run in full after F4's tier-aware adjustment; the numbers above are the post-adjustment run.

Two extra checks, unasked but cheap:

- `cargo doc --no-deps --document-private-items` — the repo carries 36 pre-existing
  unresolved-link warnings; **none** is in `src/hook`. Every intra-doc link in the three new files
  resolves.
- `task claude:tests` — **51 passed**. No rule glob was changed, so this only confirms nothing
  broke; F9's fix will need it re-run.

**Not committed. Not pushed. `main` untouched.**

---

## Handoffs

| To | What |
|---|---|
| **Orchestrator** | F1, F3, F4, F7 — **all closed on the feature branch.** Remaining: F9 (routed to WP-M below) |
| **Owner** | F6 — **decided, not open.** An explicit per-registry `trust_hooks = false` is not overridden by a blanket `--allow-hooks`; the narrower explicit statement wins, it is fail-safe, and it is loosenable additively later where the reverse is not. Surfaced for the record |
| **WP-G (Implement)** | `src/config/registry_resolve.rs` is in the file set **for Implement only** — the one-word `fn` → `pub fn` on `normalize_locator:70`, nothing else in that file. Then `grants`/`is_bare_host` reuse it rather than re-spelling the normalization |
| **Specify (WP-G)** | B4's five rows incl. "a project entry alone arms nothing"; B5's four grant/deny cases; W8's `insecure` case + loopback exemption; W5's **stdout-piped-with-stdin-a-TTY** case (no prompt, `not-armed`, exit 0, well-formed JSON on stdout) and prompt-on-stderr; C-012 record shape, sanitization, cap, rotation. **No inertness test and no falsy-disarm test** — both surfaces are deleted (F7). Add the A-10 source-level import test from F8. **F4 adds three rows**: per tier, an audit-write failure yields the right `AuditOutcome`, exit 0, and — for `mutator` — the tool call carrying its **original** input with `verdict: Some(Mutate)` |
| **WP-I** | consumes `trust::arming` + `AuditLog::at`; the "found sound" list in the audit applies — do not "improve" `--root`'s derivation discipline, `"$L"` quoting, `atomic_write`'s crash safety, or `RESERVED_ARTIFACT_NAMES` |
| **WP-K** | owns C-012's fail-closed leg, now **tier-aware** (F4). **Read `src/hook/audit.rs`'s module-doc tier table and `AuditLog::append`'s `# Errors` first**: observer/gatekeeper → do not spawn, no verdict, `NotSpawnedUnlogged`; mutator → spawn then **discard the rewrite**, `RewriteDiscardedUnlogged`, verdict still `Some(Mutate)`. **Never a non-zero exit or a deny in any row.** `verdict` records what the hook said, `outcome` what grim did |
| **WP-M** | F9 (`arch-threat-model.md` `paths:` + `.claude/rules.md` tables, same commit); W7's prompt text and `docs/src/stability.md` note; the gatekeeper-is-not-a-boundary statement must reach a user-facing doc page, not only a module doc |
