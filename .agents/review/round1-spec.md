# Round 1 — spec traceability (rv-spec)

Verdict: **1 Block, 8 Warn, 4 Suggest.** Executed: `cargo test` 2920 passed; release build clean;
two acceptance tests run; the Block reproduced end to end with a positive control.
Totals over 26 C-contracts + 16 S-scenarios: **36 delivered, 1 missing, 2 contradicts, 3 partial,
2 correctly withdrawn, 7 unrequested (2 worth acting on).**

## Block

**B-1 — an alternation matcher (`A|B`) arms on every client and fires on nothing. Reproduced.**
Three sites disagree about grim's matcher dialect:
- `src/oci/hook.rs:72` — `MATCHER_ALLOWED` admits `|`; C-018's frozen set does not list it.
- `src/install/vendor.rs:396` — `classify_matcher` splits on `|` → `ExactOrAlternation`; `:1143` passes
  the matcher verbatim into the client's structured field ⇒ **the hook registers and gets a row**.
- `src/command/hook/run.rs:452` — grim's authoritative pass is
  `if !matcher.contains(['*','?']) { return matcher == tool; }`. `"Bash|Read" == "Bash"` is false and
  `|` is not globset alternation ⇒ **nothing ever matches.**

`catalog/skills/grim-authoring/references/hook-spec.md:185-187` tells publishers `A|B` is one of three
forms "translating losslessly to every client" — a documented-safe authoring choice.

```
matcher "Bash|Read", tool Bash → (no output)                                      exit 0
matcher "Bash",      tool Bash → {"decision":"block",…"permissionDecision":"deny"} exit 0
```
Reported armed by `status`/`hook list`, never fires — the silent-guardrail class **C-025** exists to
prevent. Two tests currently **pin the defect as correct**: `run.rs:518-522` and `vendor.rs:1789`.
Both must be inverted; neither may survive.

Remediation: make `matches_tool` split on `|` and match each alternative (exact-or-glob per
alternative). If alternation is meant to be unsupported instead, `classify_matcher` must return
`NotTranslatable`, `MATCHER_ALLOWED` must drop `|`, and hook-spec.md must stop recommending it.

## Warn

- **W-1** C-015's byte-identity assertion **never executes**. `test/data/golden/pre_hooks_03e59b0/`
  ships fixtures, a generator, a replayer and `verify.py`; `README.md:229` states nothing is collected.
  No task, no CI job. Plus three missing `hash.rs` tests (key position; a `[hooks]` edit changes the
  hash; a hook-free project still hashes to the pre-hooks value).
- **W-2** Three published surfaces assert a defect the code no longer has:
  `.claude/rules/subsystem-cli-commands.md:25`, `docs/src/stability.md:359`, `docs/src/commands.md:638`
  all say a hook armed via `--allow-hooks` "still reports `gated`" and offer "read the dispatch table"
  as the untaken fix. `src/command/status.rs:705,855` **does** read it; `test_hook_arming.py:717`
  asserts `state != "gated"`, `arming == []` — **run, passes.** Delete the three paragraphs; keep the
  `trust_hooks`-invisibility gap, which is still accurate.
- **W-3** Two generators of the one string a client hashes; the second is dead, and
  `vendor.rs:1157` sets `command_windows: None`, so **codex/copilot hooks are absent on Windows** while
  a working PowerShell generator sits unused. One edit closes both.
- **W-4** `run.rs:456-467` compiles a glob (hence a regex, inside globset) **per invocation on the
  no-match hot path**. C-006 requires matchers "stored precompiled"; C-007 forbids per-invocation regex
  compilation. Never measured. Deferred pending a number, then cache or amend the contract.
- **W-5** C-016(b)'s TUI leg is not delivered: 6 `ArtifactKind::Hook` sites in `src/tui/app.rs` and
  **zero** assertions naming a hook.
- **W-6** `src/install/vendor.rs:120-127` — grim writes `com.grimoire.managed` into Claude's hook
  handler objects and its own doc says the tolerance is **unverified on claude**, the only `✓` client
  and the only project-scope surface. `vendor-capability-watchlist.md` has 12 new rows and none for this.
- **W-7** = U-1 below.
- **W-8** C-017's warning half is partial: per-hook declines name client and hook; the four environment
  refusals name client + cause only.

## Unrequested (the direction reviewers skip)

- **U-1 (Warn, deferred)** `pipeline.rs:397` `record_no_matches` writes an audit record on the
  **no-match** path — `create_dir_all` + rotation `statx` + open + append on every tool call with an
  armed-but-unmatched hook. C-012 specifies a record per *invocation*; S-004 specifies "nothing
  spawned". Cost measured: +0.04 ms ext4, **+14.1 ms per hook on 9P** (142 ms p50 at ten hooks). Gate it
  behind C-012's richer level or land it as an explicit C-012 amendment with the measured cost.
- **U-2** `MATCHER_ALLOWED` widened with `|` — folded into B-1.
- **U-3 (Warn)** `hook_launcher.rs:327,374` + `:206` — `registered_command`,
  `registered_command_powershell`, `powershell_single_quote` are dead in the shipped binary.
- **U-4 (accept)** `HOOKS_DIR_MODE`/`TABLE_MODE`/`MAX_TABLE_BYTES` — hardening no contract asked for,
  correct direction, partially pre-empts deferred C-017 cause 5.
- **U-5 (accept as measured)** the `RuntimeFlavor` split — but see S-4.
- **U-6 (accept)** `HookArmingCause` has 8+ variants where C-017 enumerates 5; each traces to a
  plan-stated behaviour and `state()` is a total match.
- **U-7 (suggest)** a `grim describe` row added to `subsystem-cli-commands.md` — unrelated to hooks.

## Tests weaker than the contract they claim to pin

| Contract | Test | Why weaker |
|---|---|---|
| C-025/C-018 | `run.rs:518-522` + `vendor.rs:1789` | **Pin the defect as correct** — jointly encode B-1 |
| C-015 | none | fixtures never collected |
| C-015 (hash) | `hash.rs` tests | no key-position, no `[hooks]`-changes-hash, no pre-hooks-value test |
| C-016(b) | acceptance only | 6 `tui/app.rs` sites unasserted |
| C-008/C-018b | `vendor.rs:1910` | strong, but pins only today's empty-verdict state and `VERDICT_EXIT_CODES` is private ⇒ no in-file fix path |
| C-017 | `log_sync` | environment refusals omit the hook |

## Suggest

- **S-1** the consent prompt omits the clause its own doc calls the control — that older grims will
  reject the file. One `writeln!`.
- **S-2** `grim schema`'s clap `about` omits `mcp` and `hook`.
- **S-3** `.agents/wp-o-report.md` still marks S-002b, S-003(arms) and S-015 as "defect (xfail)"; all
  three were fixed and no xfail survives. The map misleads the next reader.
- **S-4** C-007's "no tokio runtime" clause is unmet (`main.rs:279-301` builds a current-thread runtime)
  and the deviation is unrecorded. Amend the clause to "one current-thread runtime, no worker pool".

## Withdrawn decisions — no stale-text defects
C-026's withdrawal is delivered negatively and thoroughly; per-hook digest approval is gone; hooks are
bundle members with **disclosure, not a second prompt**. Stale ADR text is marked WITHDRAWN in place with
no code implementing it. One ordering choice correctly deferred to the owner: `policy.rs:184` puts
`trust_hooks = false` ahead of `--allow-hooks`.
