# Plan: The experimental `settings` artifact kind

## Status

- **Plan:** plan_settings_artifact_kind
- **Active phase:** 9 — review-fix loop, cycle 4 of 4, the last: the G3 fix pass landed (landings A and B)
- **Step:** awaiting /finalize: every G3 gap row is merged; no review call remains (3 of 3 spent), so the meta-orchestrator decides the next act
- **Last update:** 2026-11-06 (G3 fix pass, landing B)
- State:   review
- Tier:    xhigh
- Tier-grammar: 5
- Updated: 2026-11-06
- Next:    /hex-finalize
- Reviewed: 9a951ee3f76085b0a72ef62e53ece033fbc28768

---

## Header

- **Goal contract (binding):** `.agents/goals/config-artifact-type.md`. Autonomy: never prompt; every doubt runs § Issue resolution and is recorded here. Granted acts: none, so pushes, PR acts and issues go to the meta-orchestrator.
- **Source (read-only):** [`config-artifact-type.md`](../discussions/config-artifact-type.md), owner-ratified 2026-11-03. Its Decisions are not reopened.
- **ADR (Accepted 2026-11-04, PD-15):** [`adr_settings_artifact_kind.md`](../adr/adr_settings_artifact_kind.md), Decisions 1–11, § Component contracts, § Migration and rollout, Appendices A and B. This plan reopens no ADR decision; it decomposes them. Signatures live in the ADR and are not restated.
- **Trust records amended by this run:** `adr_artifact_trust_model.md`, `adr_hook_workspace_consent.md`, `.claude/rules/arch-threat-model.md` (I1, I2, T3, T4).
- **Research:** `.agents/research/research_config_artifact_{codebase,archaeology,vendor_semantics,prior_art,community,field_list}.md`. Not repeated.
- **Branch:** `hex/config-artifact-type`, in `/home/mherwig/.herdr/worktrees/grimoire/duo`. It holds the goal, the discussion, the ADR, this plan and every implementation commit. Pipeline workers use worktrees under `.agents/worktrees/<pipeline>`, each after `git submodule update --init --recursive`.
- **Classification:** large scope · one-way door (high: OCI media type, lock and config tables, state field, record path, consent key prefix, flag pair, report keys) · tier xhigh (auto) · `architect=on` · `research=1` · `adversary=on`, unavailable: `Cross-model plan review skipped: no codex CLI, no second harness on PATH`.
- **Tracking:** no open GitHub issue or PR covers this work (`gh issue list --state open` read 2026-11-03; nearest #53, #55, #56, #264, #266, none related).
- **Gate command:** `ocx exec -- env -u OCX_GLOBAL -u OTEL_EXPORTER_OTLP_ENDPOINT -u OTEL_EXPORTER_OTLP_HEADERS task verify`. Plain `task verify` fails here. Use `task --force verify` for a final gate, because a cached run exits 0 with no tests. Docs: `ocx exec -- env -u OCX_GLOBAL task docs:check`. Read the real exit code, never one wrapped by `; echo`. Mid-step use the subsystem gate. `shellcheck` and `shfmt` are absent, so `task shell:verify` skips them: say so in any report that relies on it.
- **Discover inputs:** five reports at HEAD `25b722a`, not committed. Facts this plan relies on are cited as `file:line` here or in the ADR.

## Objective

A publisher ships harness configuration as a `settings` package. A consumer
adds it, and grim splices each declared value into the harness's own file,
for Claude, Codex, Copilot, Cursor, OpenCode and Qoder. A per-piece rule reads
the file first: a value grim wrote can move, and anything else refuses, listing
every piece across packages before any setting is written. Consent is the
existing workspace record. Every run that changes a setting lists the keys.

The feature ships behind `[options.experimental] settings`, in the next
unreleased minor, additive only. A project without the kind stays byte-identical.

## Scope

**In:** ADR Decisions 1–11 with Appendix A's 90 fields; three `fix:` commits
ahead of the feature (ADR premises 15 and 24, and the `reap_dropped_clients`
row); the drive-by dependency guide as its own `docs:` commit; the reference
page, the sharing guide and every named page update; the catalog drift review;
the `upstream-refresh` extension; upgrade fixture, old-grim checks, real Claude
and Codex binary checks; `/security-auditor`.

**Out (the discussion's Intent):** whole harness profiles, including env and
endpoints; hook blocks in settings; generic cross-harness keys; harnesses beyond
the six; re-asking consent on a version bump; exporting this kind into plugins.
Appendix B's unverified fields stay off.

**Follow-ups (acts outside this loop's grants):**

| # | Follow-up | Owner of the act |
|---|---|---|
| F-1 | File the other-harness issue, text in § Follow-up issue. A Requirement and a goal DoD box | meta-orchestrator |
| F-2 | Teach `grimoire-rs/indexer` the kind (`src/data/index.ts:14,17`, `src/validate/core/metadata.ts:13`, `src/enrich/index.ts:225`, `src/enrich/marketplace.ts:25`, `src/validate/core/marketplace.ts:17`, `search.ts:85`, `Catalog.tsx:44`, `Base.astro:2105`) | meta-orchestrator, other repo |
| F-3 | Teach `grimoire-rs/vscode` the kind and the new causes (`src/scopes.ts:90`, `tests/goldens`), with a paired release | meta-orchestrator, other repo |
| F-4 | Tag and release | owner |

## Decisions taken while decomposing

Each row is a question met while planning, the research that answered it, and
the decision. None reopens an ADR choice (goal § Issue resolution, step 2).

| # | Question | Research | Decision |
|---|---|---|---|
| PD-1 | ADR open question: Cursor's path order | ADR Decision 3; grim only edits an existing file | Closed on its recommendation: the existing-file order, with a watchlist row (P10) |
| PD-2 | ADR open question: detect a main-checkout scalar shadowing a worktree value | ADR Decision 9 | Closed on its recommendation: no detection; the reference page and a watchlist row cover it |
| PD-3 | ADR Decision 7 says a path source is `not-armed`, but can `[settings]` hold a path at all? | `project_config.rs:224,229`: `[mcp]` and `[hooks]` are `PathValues::Rejected` | `[settings]` is `PathValues::Rejected` too, so a path value exits at parse. Decision 7's `verdict` `None` stays the control for a hand-edited lock |
| PD-4 | Which version do docs name? | `Cargo.toml` 0.15.1, latest tag `v0.15.1`, dependencies unreleased and documented as 0.16.0 | Docs say 0.16.0, the same unreleased minor. The release run rechecks if another release intervenes |
| PD-5 | How does a real-binary run avoid the owner's credentials (`deep-pass.md` § 2)? | The Claude probe in the field list used one `claude -p` call, haiku, budget 0.30, in a throwaway repo | Credential-free only, with a broken-file positive control, under a scratch `HOME`. **Amended 2026-11-04:** the fallback to the recorded `claude -p` probe is struck by `.agents/goals/config-artifact-type.md` § Autonomy (no harness binary runs with the owner's login, credentials or real `HOME`). If Claude reports nothing without a session, `test_settings_real_harness.py` reports a skip with the reason, and § Verification record says so; it never reports passed. Codex runs in a sandboxed `CODEX_HOME` from `npm install --ignore-scripts --prefix <scratch>`, and reports not run if that install fails or needs a login. Both are opt-in by env, local, never CI |
| PD-6 | `--trust-hooks` as hidden or visible alias? | Principle 9; docs and pytest pin the name | `visible_alias`, so `--help` keeps naming it |
| PD-7 | Does the contract wave stub with `todo!()`? | Precedent plan § TDD; the template allows `unimplemented!()` | Reachable shims keep today's behaviour. `unimplemented!()` is allowed only where nothing reachable calls it, and its owner removes it |
| PD-8 | One test file or many? | Disjoint file sets are required per pipeline | One `test/tests/test_settings_*.py` per pipeline concern, listed in the Parallelization table |
| PD-9 | When does the watchlist glob land? | `test_ai_config.py` fails a dead `paths:` glob | P10, after P9 creates `settings.md` |
| PD-10 | Where does the audit sit? | Goal Verification; the audit needs the whole diff | Phase V step 6, after P10. Findings are recorded by location and class only |
| PD-11 | Where does `/upstream-refresh` prove itself? | `deep-pass.md` § 1 and § 2 | Phase V step 7, one named harness (Claude), reporting the settings-key inventory against Appendix A |
| PD-12 | Does `settings_fields()` land in the contract wave? | P5's validator needs a real list; CW is already the largest step | No. P2 lands it and P5 depends on P2. The wave count rises by one, and the wide wave stays wide |
| PD-13 | Does any new dependency land? | `toml_edit`, `serde_json`, `sha2` are direct; RFC 6901 is two escapes; `proptest` is not a dependency | None. `Cargo.lock` does not change. A property test is a hand-rolled table |
| PD-14 | How does a refusal keep the change list? | `finish` keeps the first error (`install.rs:506`); ADR Decision 8 | `finish` wraps only when `SettingsOutcome` is non-empty, so every other run takes today's path (C-054) |
| PD-15 | The ADR is Proposed; when does it flip? | Precedent: flipped after the plan panel | This run flips it to Accepted after the panel's fixes and the one re-validation, with the index row and the two trust boxes in the same commit |
| PD-16 | `decompose.md:32` puts contract tests in the wave; do these? | `plan_artifact_dependencies.md:127` wrote them per step, and its review passed | CW writes tests for what it delivers. Each later step writes its named tests first and records the red run. This plan's contract rows are the coverage map |
| PD-17 | The research seat proposes a pre-`--force` backup (Home Manager's lesson). Adopt it? | Discussion Decisions: "the old value appears only in the error"; Thread: "`--force` does not save the old value" | No. It would reverse a frozen Decision. C-062 states `--force` is unrecoverable and `current` is bounded to 200 characters |
| PD-18 | The read path needs `value_at`, so which pipeline owns it? | `install_state.rs:287,290`; C-009's units cannot run on a stub | `split_pointer_path` and `value_at` are real in CW (C-018). P1 keeps every write |
| PD-19 | P-G asserted behaviour from P3 and P4 | Plan review; `load_project` does not discard at CW | P-G stays wave 3 as characterization. The forged-state assertion moves to C-059, and the re-prompt to C-031 |
| PD-20 | "Copilot reads Claude's file" is a harness claim grim cannot observe | ADR Decision 9 | S-026 tests what grim writes. The Copilot behaviour stays documented, attributed to upstream |
| PD-21 | Harness reload claims rest on search summaries of a third-party mirror | Research seat R-1 could not fetch the pages | C-062 checks the real upstream issues before stating any reload behaviour; absent a primary source, the page says a restart is the safe assumption |
| PD-22 | The p9 differential allows a closed key list and the plan adds keys. Who owns the list? | `test/tools/p9_diff.py:53-63`, `diff_json` at `:339`; `3fa384a` added `pulled_in` the same way; deep only, so `task verify` never shows it | P-G owns it (C-072). Phase V step 11 runs `task p9:diff` locally, so a miss does not first appear at the goal's deep-verify gate |
| PD-23 | How is the write race proven? | The window is in-process and `src/` has no `GRIM_TEST_` seam | At the `settings_writer` seam: a unit passes `apply` a stale expected value, which is the exact condition the re-read detects. No production test seam is added for one acceptance test, and S-017 cites the unit |
| PD-24 | Where do `pieces` and `settings_changes` sit in the error document? | `test_json_interface.py:40` and `test_config_vendors.py:45` pin `set(doc) == {"error"}` | Inside `error`, beside `reason`. The ADR now says so |
| PD-25 | Execution found the dropped-client reap has neighbours C-002 does not cover: a same-update pin roll that changes the value re-registers then reaps an adopted entry; a client with two outputs loses its kept-modified output from the record; `reaped_clients` repeats a two-output client; `install_mcp`'s stale removal ignores `adopted` | `prune.rs`, `installer.rs:1505-1511,2866`, reproduced in step PF-2; `reap_dropped_clients` skips the settings kind (C-010), so no finding reaches this feature | Outside this goal's scope (goal § Issue resolution, item 6). A follow-up issue, requested from the meta-orchestrator. C-002 stays as written |
| PD-26 | (fix pass, cycle 3) "Every worker runs the full `task --force verify` in its own worktree before handing off", yet at most two gates may run at once and ten pipelines finish close together. Who presses the gate? | Follow-ups PD-31 met the same load on the same machine; worker-run gates would stack past two | The orchestrator runs each finished pipeline's `task --force verify` in that pipeline's own worktree, two at a time, and checks the staged tree is unchanged afterwards. A red pipeline gate goes back to a fixer in that worktree as an inner check, not a cycle. Before landing, a rehearsal gate runs on the exact landing tree in the docs worktree. A red landing gate stops the run |
| PD-27 | (fix pass, call 1 Block at settings.md:203) `install` never lists an adopted value in `abandoned_entries`, but the page says it does. Fix the code or the page? | `src/command/install.rs` has no `abandoned` field; the installer warns on stderr ("left in place, grim no longer tracks it"); `update` (`attach_abandoned`) and `uninstall` list it. ADR:400-401 already scopes the field to `update` and `uninstall` and says "`install` warns instead". A new install key would freeze a shape at 0.16.0 for a rare event whose value is never touched | Fix the page only. settings.md names the stderr warning on `install` and the `abandoned_entries` listing on `update` and `uninstall` |
| PD-28 | (fix pass, review O4 / S-5) Can settings writes refuse a relative or workspace-internal vendor-root env value without a layout move? | Read-only research seat. `stability.md:251-262` lets the experimental settings kind change in a minor; new cause literals are additive (`stability.md:265-269`, `json-interface.md:686`). `vendor_*.rs` `settings_config_path` reads the variables unvalidated (CWE-73), and global scope has no link confinement (CWE-59). `validate_grim_home` checks only relativity at global scope. The composed file path is judged in `settings_surface` and `Phase::refuses`, so no vendor file changes | Yes, for settings at global scope only. Refuse a non-absolute root (`settings-root-relative`) and a root inside the current directory's enclosing work tree unless that tree holds `$HOME` (`settings-root-in-workspace`). The row reads `not-armed`, the run exits 0, the file and the pin stay. Other kinds are unchanged. The second check is a heuristic: a run from a dotfiles repository whose root holds the config is refused (false positive), and an absolute path into another repository passes (the residual GRIM_HOME already carries, ADR T3 row) |
| PD-33 | (fix pass, closing review 3: C3-H2 / D-1, C3-W14 / D-2) The global root fence has two edges PD-28 does not name: a `HOME` the repository sets drops the fence, and a checkout with no `.git` is not fenced. Fix them in code, or record them? | `HOME`: the exemption reads `$HOME` from the environment, which the files the fence guards against can set (CWE-15, T3). A trusted home source needs `getpwuid_r`, which the crate cannot call (`unsafe_code = "forbid"`, `Cargo.toml:176`), and no golden-path crate offers one (`product-tech-strategy.md`). A narrower exemption, only a tree that is `$HOME` itself, reads the same value, so it closes nothing. With `GRIM_HOME` unset, that same `HOME` already moves grim's whole global config into the repository, the inherited gap `validate_grim_home` records (`hook_registrar.rs`). No `.git` (an archive, jj, hg, Sapling, or a `.git` file without `gitdir:`), CWE-15, T3: a fallback to the working directory refuses legitimate runs from inside a vendor directory, such as `~/.config` under an XDG root, and jj, hg and Sapling markers add surface for a rare case (YAGNI) | Both are recorded, not fixed: `arch-threat-model.md`'s fence paragraph, the ADR's T3 row and `subsystem-install-layout-settings.md` name them as edges beside PD-28's two. The `HOME` edge gets its legibility half in code: when a work tree around the command holds `$HOME`, a change line prints the link-resolved absolute path, not a `~` form. A code fix for `HOME` waits for a safe identity source |
| PD-29 | (fix pass, file-set scope) Call 1 flagged edits outside their pipeline's Expected files (47da12f `path_anchor.rs`; b48dc0b four `src/install` files under a `feat:` subject; bc273fc and 3081b1a), and call 2 flagged fa72cc8 (N-W42: `AGENTS.md`, `configuration.md`). Act? | Each edit was gated and reviewed in two closing calls; none is a defect by itself | Accept them as landed. No code change. `/hex-finalize` regroups the history, which is where the b48dc0b subject is corrected. Expected-file lists are a planning aid, not a gate |
| PD-30 | (fix pass, call 1 Warn at installer.rs:295 / settings_phase.rs:182) Layers are re-read every run, against ADR:845. Skip the read when records sit at their pins? | Engine pipeline: safe with one armed package (fixed, test fails when the skip goes). With two or more, a package recorded while another was gated was never compared with it, so skipping would let a cross-package conflict through | Skip only for one armed package. With two or more, each layer is read once per run (the double load is gone too). The ADR's Performance row is corrected to say so, so the Warn closes by fixing the claim |
| PD-31 | (fix pass, Warn deferrals) Call 1 settings_writer.rs:113-153 (no cross-process lock) and release.rs:574 / annotations.rs:791 (third copy of the JSON-layer body) | Lock: the scope `ConfigFileLock` already covers the whole run; a per-file sidecar lock would litter harness directories, inside the repository at project scope, and harnesses ignore it; the ADR scored that option 22 against 37. Copy: removing it needs an abstraction over two types in two files plus a byte-pinning test first, a refactor (Two Hats) | Both deferred. The lock stays as designed (ADR). The duplicate is a follow-up refactor, on the out-of-scope list for the meta-orchestrator |
| PD-32 | (fix pass) Smaller choices the pipelines made | Worker reports | An unreadable settings store reads `not-armed` with cause `not-registered` (the `SurfaceUnreadable` precedent), not a new token. `main.rs` gets no `shutdown_timeout`: the git probe is now a child killed at its timeout, and a runtime timeout would cut blocking writes. The key guard checks only the top-level key, its dotted pieces and dotted prefixes, because every MCP or hook container is a top-level key and a per-segment check refused Warp's own `/agents/mcp_servers/...` row. A settings file that stops parsing during a run exits 65 (was a retryable 75), allowed for the experimental kind. A non-interactive run without consent adds a `skipped` row per package, as the hook path does. The hook-kind consent remedy names `--trust-hooks` like its neighbours in `policy.rs`. Claude's project `/sandbox/enabled` is not reclassified; its limit goes into the settings Limits list |
| PD-34 | (G3 fix pass, closing review 3) Smaller choices the G3 pipelines made | Worker reports and the plan-marked review seat on G3-ENGINE (A4) | Fence: a root spelled with `..` is refused only while the fence holds a work tree, since an open fence fences nothing and lexical folding can disagree with how the OS resolves a link before `..`; an unreadable working directory reuses `settings-root-in-workspace`, since this pass adds no cause token (the set freezes at 0.16.0); the shown resolved path follows links. Evidence: each refused client is named once per client and cause on every run under `settings left unchanged:`, even when the package declares nothing for it; a fenced output counts as intact unread; an `install` row whose record holds a refused output reads `skipped`, an existing literal the review seat ruled within Principle 9, while `status` reads `not-armed` and `stale` still wins. One predicate (`recorded_refusal`) judges each recorded settings output by its recorded path before any open, for `status`, the TUI, the search and catalog badge and the engine's read paths; the badge has no `not-armed`, so it leaves a fenced output out of its check; `arming` names every client whose recorded output the fence refuses, selected or not, one verdict per client, sorted, and a gated note for that client gives way to the refusal, so a `not-armed` row never carries an empty `arming`; under a whole-artifact gate the row keeps the gate's verdicts and state. Narrowed runs: the pin is held only by a client whose recorded values differ from what the new version writes, any spelling mismatch counting as a difference; under autodetect no client counts as dropped. The other JSON spelling binds only OpenCode, Amp and Kilo; a marketplace URL is refused whenever `@` sits in its authority, cut at `/`, `?` and `#` only; an unknown vendor table reuses the unknown-key hint. Deferred out of this pass: a working directory reached through a link labels a link-placed root `Spelled`, which needs the unresolved tree path in the fence (both refuse; only the remedy differs); a `$HOME` spelled with `..` refuses home-placed clients (fails closed); the dropped-client remedy could name `grim update <package>`; the `.git` probe keeps a type-check-then-open window (CWE-367 class, needs a non-blocking open); a stray `.git` in a high ancestor widens the fence (fails closed) |

## Traceability to the source

### Requirements

| Discussion requirement | ADR | Contracts | Scenarios |
|---|---|---|---|
| Behaviour defaults, permission policy, marketplaces | D3, App. A | C-023, C-024, C-043 | S-003, S-026 |
| Public packages | D1, D7 | C-038, C-040 | S-002, S-025 |
| Six harnesses | D3, App. A | C-023…C-026 | S-003, S-023, S-024 |
| Follow-up issue | App. B | F-1, § Follow-up issue | Phase V step 8 |
| `upstream-refresh` check, five named files | Migration | C-066 | S-030 |
| Every changing run lists keys, including `install` after a bump | D8 | C-054 | S-010, S-012 |
| Reference page, sharing guide, page updates, catalog review | Migration | C-062…C-065, C-068 | S-026 |
| Dependency guide, own `docs:` commit, may land first | Migration | C-003 | S-001 |

### Open questions

| # | Question | Closed by | Contract |
|---|---|---|---|
| 1 | Behind `[options.experimental]` | D1 | C-007 |
| 2 | `settings` or `config` | D1 | C-004 |
| 3 | `--trust-hooks` answers too | D7 | C-033 |
| 4 | Which fields per harness | D3, App. A, App. B | C-023, F-1 |
| 5 | `enabledPlugins` | D3 | C-023 (denylist test) |
| 6 | Other marketplace keys | D3, App. A | C-023, S-026 |
| 7 | Linked worktree file | D9 | C-049 |
| 8 | Harness writes between read and write | D11 | C-022, C-051 |

### Verification

| Discussion bullet | Proof |
|---|---|
| `task verify`, `task docs:check`, `task catalog:verify` pass | Phase V steps 1, 2 |
| Every rule row for JSON and TOML and each merge class, replaying the archaeology | C-057, C-058; `test_settings_matrix.py`, `test_settings_file_fidelity.py` |
| The error lists every piece across packages, `overwrite`, text and JSON, nothing written | C-044, C-054; `test_settings_rule.py::test_refusal_lists_every_piece_across_packages`, `test_settings_reports.py::test_refusal_pieces_in_json_and_text` |
| Copied config and `git pull` end in that error; reordering flags nothing | C-059, C-052; `test_settings_teammate.py::test_copied_config_refuses`, `::test_pull_changed_value_refuses`, `test_settings_status.py::test_reordered_keys_read_installed` |
| Status indicators; refusal carries reason and `forceable` | C-052, C-054; `test_settings_status.py`, `test_settings_reports.py` |
| A matching value is adopted, no `--force`, survives uninstall | C-043, C-047; `test_settings_lifecycle.py::test_adopted_value_survives_uninstall` |
| A teammate's `--force` through the lock does not overwrite here | C-059; `test_settings_teammate.py::test_teammate_force_refuses_here` |
| Fresh clone, non-interactive: no write, exit 0, `workspace-not-consented` | C-046, C-052; `test_settings_gate.py::test_fresh_clone_writes_nothing`, `test_settings_status.py::test_fresh_clone_reports_workspace_not_consented` |
| Forged `state.json`, including through `git pull` | C-059, C-070; `test_settings_forged_state.py`, `test_state_load_rule.py` |
| A file without a record, a deleted record | C-043; `test_settings_rule.py::test_deleted_record_adopts_identical_and_refuses_different` |
| Older grim 78; a lock without it byte-identical | C-016, C-017; `test_golden_pre_settings.py`, `test_old_grim_settings.py` |
| Rendered settings load in real Claude and Codex | C-060; `test_settings_real_harness.py`, Phase V step 5 |
| `/security-auditor` passes | Phase V step 6 |
| Reference page and both guides pass, each use case tested | C-003, C-062, C-063, C-065; `task docs:check`, `doc_examples.py`, guide and reference tests |
| Follow-up issue exists; `/upstream-refresh` reports a named harness | F-1; Phase V steps 7, 8 |

## Design guarantees

### Backwards compatibility (Principle 9 gate)

Additive only. No Constitution Deviations: every surface below is byte-identical
for an existing user, or widened by an optional field, an appended literal, a
new table or an alias. Three behaviour changes on shipped paths are bug fixes,
each named below and called out in `upgrading.md`. The ADR's § Convention
deviations 1–13 are carried as stated, and each is a naming or scope note, not a
Principle 9 break.

| Surface | Change | Proof (test, owner) |
|---|---|---|
| OCI wire, every existing kind | None | Existing golden and release tests stay green (CW, P5) |
| New media type, annotation | New | `src/oci/settings.rs` units (P5) |
| `grimoire.lock`, no settings | Byte-identical | `test_golden_pre_settings.py` (P-G), `grimoire_lock.rs` snapshot (CW) |
| `grimoire.lock`, settings | `[[settings]]` when non-empty; `lock_version` stays 1 | byte-exact unit, `src/lock/testdata/lock.schema.json` golden (CW) |
| `grimoire.toml` | `[settings]`, `[options.experimental] settings`; hash unchanged when the table is empty | `hash.rs` unit, `test_golden_pre_hooks.py:219,245` stays green (CW) |
| `.grimoire/state.json` | Never holds the kind; a project without it byte-identical | P-G golden, `install_state.rs` units (P3) |
| `global.json` | `element` omitted when `None`; settings records additive | serde round-trip unit (CW), `test_old_grim_settings.py` 74 row (P-G) |
| Consent record | No format change; `settings:` keys inside `hooks` | `consent.rs` units (P4), P-G old-record fixture |
| Flags | `--trust-workspace` pair; `--trust-hooks` keeps working | every existing pytest pin stays green; `test_settings_consent.py` (P4) |
| Subcommands | 26 stays 26; `consent` is a visible alias | `command_name.rs` table row, `test_settings_consent.py::test_consent_alias_reports_hook_telemetry_name` (P4) |
| Reports | `settings_changes` on install, add, update, uninstall; `pieces` on update; `element` on `abandoned_entries` and status `outputs`, always present | `test_settings_reports.py` (P8), api units (CW), `test_status.py`, `test_uninstall.py`, `test_hooks_flag_off.py` pins extended (P7, CW) |
| Error document | `pieces`, `settings_changes` inside `error`, only when present | `main.rs` units: every existing `error_document_*` test unchanged (CW) |
| Flag-off differential run | Stdout differs by the closed additive keys only, and the tree by nothing | `test/tools/p9_diff.py`, `task p9:self-test` (P-G), `task p9:diff` (Phase V step 11) |
| `ErrorReason` | 9 slugs, unchanged | `c031_4_no_new_error_reason_slug` passes with no edit (CW) |
| Status | `gated` and `not-armed` widen; three cause literals appended | `artifact_status.rs` units, `test_settings_status.py` (P7) |
| `grim config list` | One row appended | `test_hooks_port_contracts.py` extended by appending (CW) |
| JSON schemas | Additive | schema goldens (CW, P5) |
| `.grimoire/state.json` load, any kind | **Fix:** an output its kind may not address is dropped | `test_state_load_rule.py`, ordinary-record positive control (PF) |
| `reap_dropped_clients` | **Fix:** an adopted MCP entry stays, named in `abandoned_entries` | `test_reap_adopted.py` (PF), `upgrading.md` (P9) |
| `git_info_dir` | **Fix:** the rule moves to the common `info/exclude` | `test_hooks_fixes_worktree_exclude.py` (PF), `upgrading.md` (P9) |

### TDD approach

Every pipeline step runs **Specify → Implement → Self-check**, as the precedent
plan does:

1. **Specify:** write the tests named in the step's contracts, from this plan
   and the ADR, not from code. Record the red run.
2. **Implement:** until the step's tests and the existing suite pass.
3. **Self-check:** the subsystem gate, targeted `pytest -k`, and one empirical
   mutation per contract: change one token, show a named test fails, revert.

Rules every step follows:

- Characterization first: P-G's golden test is green on the CW tree and stays
  green through every merge.
- A stale `test/bin/grim` passes negative tests vacuously, so every "nothing
  was written" assertion carries a positive control in the same test.
- Plan IDs stay out of source, test names and comments (code-docs LNK-01).
- Fixtures are pushed by the CW helper `make_settings`, never by the grim under
  test, so a broken writer cannot mask a broken reader.
- No test touches a real `HOME`. Harness roots are redirected by the vendor
  variables, and `test/conftest.py:261-290` keeps guarding managed Claude settings.

### JSON interface

Signature source: ADR § Component contracts. These are the surfaces consumers read.

```jsonc
// grim install --format json, a refusal. `pieces` and `settings_changes` sit
// inside `error`, each omitted when absent, because `set(doc) == {"error"}` is pinned
{"error": {"code": "data", "exit": 65, "message": "...",
           "reason": "modified", "forceable": true,
           "pieces": [{"package": "team-defaults", "client": "claude",
                       "file": "~/.claude/settings.json", "pointer": "/model", "element": null,
                       "marked": "overwrite", "state": "modified",
                       "wanted": "sonnet", "current": "opus"}]}}

// any other error: today's document, no new key
{"error": {"code": "not-found", "exit": 79, "message": "..."}}

// install, add, update, uninstall: always present, [] when empty
{"settings_changes": [{"package": "team-defaults", "client": "claude",
                       "file": ".claude/settings.local.json",
                       "key": "/permissions/deny", "element": "Bash(rm:*)",
                       "action": "added"}]}
```

`settings_changes` carries no values. `pieces[].current` is the only place an
old value appears: bounded to 200 characters, control characters escaped, URL
userinfo masked. A `grim status` settings row carries `arming` exactly as a hook
row does, and its `outputs` carry `element`. `outputs_pending` stays `[]` for
the kind.

### Exit codes

ADR § Component contracts holds the table. No new exit code and no new reason
slug. In one line: 65 `modified` or `untracked-destination` (forceable), 65 no
reason, not forceable (unparseable file, which wins over overwrite pieces;
invalid document), 65 `unsupported-artifact-feature` (unknown field at install),
78 (conflict across packages), 75 `locked` (harness wrote during the run), 81
(uncached layer, offline, plan needs it), 69 and 80 (registry and auth failure
loading a layer), 74 (store or state write), 0 with a status cause (flag off,
gated, guard refused).

## Component contracts

Each row adds placement, behaviour and the named test to the ADR's signature.

### P0: dependency guide

- **C-003 `docs/src/content/docs/guides/dependencies.md`.** Declared `how-to`, `everyday`. States when to use a bundle and when `requires`, and how to read `grim why`. Worked case 1: a lifecycle package over the playbook's six stages (plan, design, build, test, deploy, maintain), with shared templates as a required skill and review subagents as members. Worked case 2: a skill requiring an MCP server through `grim.requires.mcp.gdrive`. Sidebar entry under Guides in `docs/astro.config.mjs`. Every fence bound by `<!-- doc: guide-dependencies -->` to `test/tests/test_guide_dependencies.py`, header `# doc: guide-dependencies`, count pinned as `test_guide_agent_hooks.py:99` does. Published fixtures make `grim why` run offline.

### PF: fixes ahead of the feature

Each is a bug-fix workflow with a failing test first, and its own `fix:` commit.

- **C-001 `git_info_dir` (`hook_registrar.rs:2062`) resolves the common directory.** In a linked worktree the rule lands where git honours it. Test `test_hooks_fixes_worktree_exclude.py::test_linked_worktree_exclude_is_honoured`, asserting `git check-ignore`, with a primary-checkout control. Class recorded in ADR premise 15.
- **C-002 `reap_dropped_clients` (`prune.rs:438`) consults `adopted` for entry outputs.** Intact and adopted: left, and named in `abandoned_entries` (`prune.rs:406`). Intact and not adopted: removed. Drifted: `kept_modified_clients`. Units in `prune.rs` for each row; `test_reap_adopted.py::test_dropped_client_leaves_adopted_mcp_entry`.
- **C-070 State load rule (ADR Decision 6, premise 24).** Class and location only: trust-boundary bypass, CWE-345, in the project state load path. On load, an output its record's kind may not legally address is dropped with a warning, for every kind. Legality reads the kind's anchor table and the vendor's own surfaces, never the path alone. Units in `install_state.rs`; `test_state_load_rule.py::test_outputs_outside_their_kind_are_dropped`, parametrized over each other kind, and `::test_ordinary_mcp_record_keeps_its_outputs` as the positive control, covering a vendor whose MCP members share a settings file.

### Contract wave (CW)

- **C-004 Kind (D1).** `ArtifactKind::Settings` appended last, `ALL` length 7, token `settings`, `subdir()` `settings`, every compile-forced arm, and the silent sites: `fetch.rs:386-390`, `installer.rs:2425-2428`, `status.rs:669-677`, `client_supports_kind` (`installer.rs:1289`) answering from `settings_fields().is_empty()` with a pinned test, `kind_is_permanently_declined` (`installer.rs:1327`), `expected_outputs.rs:87,126` (no pending outputs), the resolver tables (`resolver.rs:121,352,388,678`), `grimoire_lock.rs:338,383,434` (`iter_artifacts` yields the kind), `command_error.rs:43` (the `--kind` list), and the message and doc at `src/oci/requires_decl.rs:31-32`, which list the tables. The literal joins the six `value_parser`s and the `schema.rs` requires enum. Units: `ALL` order, token round trip, `Ord` sorts last, `is_tar_kind` false, and a resolver unit that `[settings]` resolves.
- **C-005 Lock wire (D1).** `RawLock.settings`, `[[settings]]` emitted only when non-empty, element keys as every other kind. Byte-exact snapshot in `grimoire_lock.rs`; `lock_io.rs` fixtures unchanged; `src/lock/testdata/lock.schema.json` golden additive; `content_equal` covers the array (a change in settings alone is a change).
- **C-006 Config wire (D1, PD-3).** `[settings]` in `RawConfig`, `PathValues::Rejected`, appended to `DECLARATION_TABLES`, entering the hash only when non-empty. `collect_declared` in `status.rs:669` sees it. Units: empty table leaves the hash; a path value exits at parse.
- **C-007 Feature flag (D1).** `ExperimentalOptions.settings`, `settings_enabled()`, `ConfigKey::ExperimentalSettings` at `ALL[11]`, project-settable, no env form. `test_hooks_port_contracts.py:140-148` and the units at `config_keys.rs:1214`, `schema.rs:430`, `config_report.rs:1198` extend by appending; the env-inert test (`:185-210`) gains the key.
- **C-008 Vendor types (D3).** `MergeClass`, `SettingsScopes`, `SettingsValueType`, `SettingsField`, and the three defaulted `Vendor` methods. Unit: every vendor answers `&[]`, `None`, and today's behaviour is unchanged.
- **C-009 Piece record and read path (D4).** `ClientOutput.element`, omitted when `None`, because it is state under `deny_unknown_fields`. `AbandonedEntry.element` is always present, `null` when `None`, because it reaches JSON (`stability.md:71-74`). `is_present` gains `kind`. Private `entry_format(kind)` and `entry_vendor_owned_keys(kind)` replace the MCP calls on the read path. `current_hash`, `is_present` and `read_entry_value` dispatch on `kind` and `element` through `value_at`. `settings_value_hash` returns `Digest` and canonicalises an integer-valued float; `entry_value_hash` is untouched. The doc at `install_state.rs:352` widens. Units: a pre-existing state file round-trips byte-identical; `1` and `1.0` hash equal under settings and differ under MCP; a one-segment and a deep TOML pointer read correctly; a set element reads present, absent, sibling-only, and absent for a bare string equal to it.
- **C-010 Run types and seats (D5).** `InstallRun`, `SettingsOutcome`, `InstallOutcome::RefusedSettings`, `SettingsRun`. `install_all_with_progress` and `install_and_persist` take a required `SettingsRun` and return `InstallRun`. Every seat is stated: `Full` at `install.rs:119`, `update.rs:177`, `add.rs:718`, `tui/app.rs:2893,3030`; `Skip` at `install.rs:365`, `update.rs:600`, `tui/app.rs:3135`. `Settings` leaves the work list; `prune_orphans`, `reap_dropped_clients`, `reap_relocated_roots` and `retired_outputs` skip it, a closed list; `refuse_uninstallable_fallback` skips it. Units: `settings_records_skip_the_work_list`, `generic_removal_seams_skip_settings_records` in `prune.rs` and `installer.rs`. P6 proves the seats (C-047).
- **C-011 Errors (D5, D8).** `InstallErrorKind::{SettingsRefused, SettingsConflict, SettingsUnparseable}`, `UninstallError::SettingsDrifted`, `SettingsFailure`, `Error::Settings`, `classify_variant`. `refusal_error` (`install.rs:564`, read at `install.rs:526` and `update.rs:347`) gains the new outcome and loses its wildcard, because a missed arm exits 0 with no diagnostic. `error_document` gains optional `pieces` and `settings_changes` inside `error`, omitted when absent (PD-24). Units: mixed pieces read `modified`; an unparseable file wins, not forceable, with `pieces` attached; `refusal_error_maps_every_refusing_outcome`. `c031_4_no_new_error_reason_slug` and `c021_exit_codes_and_reasons_equal_main` pass with no edit.
- **C-012 API types (D8).** `SettingsChange`, `SettingsChangeAction`, `RefusedPiece`, `PieceMark`, `PieceState` in `src/api/settings_report.rs`, field order frozen; always-present `settings_changes` on `InstallReport`, `UpdateReport`, `AddReport`, `UninstallReport`; `pieces` on `UpdateReport`; `UninstallResult.settings_changes`. `test_uninstall.py:76-78` and the `test_hooks_flag_off.py:341-344,373` add-key pin extend by appending. Units pin the key order, and `[]` when empty.
- **C-013 Stubs.** `src/oci/settings.rs` types and constants; `src/install/{settings_store,settings_writer,settings_phase}.rs`, with `preflight` returning `Ok(())` and `apply` returning an empty result; the splice write signatures. All registered in `src/install.rs` and `src/oci.rs`, so no later pipeline edits a module root. PD-7 governs bodies. `test_settings_helpers.py::test_settings_row_passes_the_stubs` proves a declared package installs nothing and exits 0.
- **C-014 Consent shim (D7).** `consent_key_for`, `ConsentPolicies`, `ConsentNoun`, `declared_consent_keys`; `prompt_for_workspace` gains the noun, always `Hooks` for now. `InstallTarget` gains `settings_policy` and `with_settings_policy`. Callers in `install.rs`, `update.rs`, `add.rs` and `tui/app.rs:2871,3023,3131` attach both policies, `tui_hook_policy` returns `ConsentPolicies`, and `settings` reads flag-off. `grim uninstall` attaches none. Every hook test stays green. `target.rs` unit: a target built by `parse` carries no settings policy. P6 proves `None` fails closed (C-046).
- **C-015 Test infrastructure.** `make_settings(repo, tag, document, requires=None)` pushes through `push_manifest`, never through grim. `write_config` gains `settings=` and `experimental_settings=`. `settings_sandbox()` seeds redirected roots for all six clients. Self-test `test/tests/test_settings_helpers.py`.
- **C-018 Pointers and readers (D4, PD-18).** `split_pointer_path` is RFC 6901 with `~0`, `~1`; `split_pointer` stays two-segment. `PointerRead` and `value_at` are real for JSON, JSONC and TOML, any depth. A segment holding `/` or `~` round-trips. An unparseable file is `Err`, a non-container ancestor is `Blocked`. Units, plus a hand-rolled round-trip table.

### P-G: upgrade fixture and old-grim checks

- **C-016 Golden `test/data/golden/pre_settings_<sha>/`** from the binary at this branch's PF tip, with `test_golden_pre_settings.py`: a project of all six existing kinds keeps lock and state bytes modulo `generated_by`, online and offline; an old consent record with hook keys evaluates as before. Added to the `verify-deep.yml` list at `:271` and its junit check. Follows `.agents/golden_fixture_generation.md`.
- **C-017 `test_old_grim_settings.py`, opt-in `GRIM_OLD_BIN`.** Every reachable matrix row: lock 78, `[settings]` 78, experimental key 78, bundle member 65, `requires` edge 65, a hand-seeded `global.json` 74, `grim add` on the new layer reading no kind, and a v0.15.1 `grim hook allow` dropping `settings:` keys. Fixtures are hand-written or pushed by `make_settings`. One recorded run in § Verification record.
- **C-072 Differential gate (PD-22).** `ADDITIVE_STDOUT_KEYS` in `test/tools/p9_diff.py` gains `status.element` (`None` only), `settings_changes` (`[]` only) on `install`, `add`, `update` and `uninstall`, and `pieces` (`[]` only) on `update`. `self_test` gains a planted non-empty `settings_changes` and a planted non-null `element`, each reported. Proof: `task p9:self-test` here, and `task p9:diff` at Phase V step 11.

### P1: splice engines (mark `hard`)

- **C-019 JSON and JSONC members.** `upsert_member_at`, `remove_member_at`, any depth. A missing container is created. Comments, trailing commas, CRLF, tabs and sibling order survive. A key is escaped, so a package cannot close a string (commit 2043a62's class). Units in `json_splice.rs`: `upsert_at_depth_creates_containers`, `upsert_keeps_comments_and_sibling_order`, `upsert_keeps_crlf_and_tabs`, `remove_at_depth_leaves_siblings`, `key_with_quote_is_escaped`, `non_container_ancestor_is_invalid_data`.
- **C-020 JSON element ops.** `upsert_array_element_at`, `remove_array_element_at`, any depth: idempotent, identity is the element, an absent array is created, a comment inside the array survives, removing the last element leaves `[]` at op level. Units: `element_upsert_is_idempotent`, `element_upsert_creates_the_array`, `element_ops_keep_array_comments`, `element_remove_last_leaves_empty_array`.
- **C-021 TOML.** The same five operations. Inline tables edit in place, `ArrayOfTables` is a type mismatch, a scalar replacement keeps decor (`Table::insert` resets it, so the test pins an in-line comment), grim-created tables are implicit where allowed, a bare table header with a comment survives (2369e1e's class). Byte-diff units on the whole file: an untouched array-of-tables elsewhere stays identical after an unrelated upsert, including a header with inner whitespace and a comment above it; a dotted-key table's leading comment is not duplicated; a comment at the head of the document survives when the first table is created or pruned.
- **C-022 `settings_writer::apply` (D11).** One re-read, one re-judge, one `atomic_write_through_symlink`, per file, and one `debug` span per file. Mismatch without force: `SettingsWriteError`, mapped to exit 75 through `LockError::Locked`, nothing written to that file. With force: the comparison is skipped. `PruneEmpty` stops at a required pointer. A symlinked dotfile stays a symlink, including a link to a link across directories, and the final file keeps its mode. Units replay 7df05e4 and 31625da, and `write_race_maps_to_exit_75`.

### P2: typed list and vendors

- **C-023 Field tables (D3, App. A).** `settings_fields()` in six vendor files, exactly Appendix A. Units: 90 rows; a golden pinning `(pointer, class)` for every row, because a class change is a layout move; a denylist test that no first segment is `claudeMdExcludes`, `hooks`, `mcpServers`, `mcp`, `mcp_servers`, `instructions`, `enabledPlugins`; `project_values` on Claude `defaultMode`; `required_by_client` on Cursor's pointers; OpenCode's tool list in `map_keys`.
- **C-024 Paths and formats.** `settings_config_path` and `settings_config_format` per Appendix A's file line, honouring `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `COPILOT_HOME`, `QODER_CONFIG_DIR`, `OPENCODE_CONFIG`, `opencode_config::config_path_for_scope`. Non-Claude answers `None` at project scope; Claude answers `.claude/settings.local.json`. Unit per vendor and scope.
- **C-025 Cursor roots (D3, PD-1).** Two pure `VENDOR_ROOTS` rows, `cursor-cli` and `cursor-cli-xdg`, anchors `cursor-cli-root` and `cursor-cli-xdg-root`, appended to `SHIPPED_ANCHOR_TAGS` (`path_anchor.rs:1266`), on the `kilo-config` precedent, with `candidate_anchors` and `hermetic_vendor_roots`. `settings_config_path` makes the existence choice and records the anchor. Skills stay as `vendor_cursor.rs:23` says. Units: `cursor_settings_path_order`, `cursor_root_is_pure`, `cursor_skills_ignore_the_variable`.
- **C-026 Other vendors (13 clients).** No vendor file changes: `client_supports_kind` declines from `settings_fields().is_empty()`. `is_declined_global_pair` (`path_anchor.rs:1049`) answers to match. `kind_surface_is_true_everywhere_except_the_declared_scope_gaps` (`vendor.rs:1580`) records the kind as six-vendor-only. Unit: `settings_declines_every_other_client` over `ClientTarget::ALL`, asserting the warn-versus-debug split from `kind_is_permanently_declined`.

### P3: record store (mark `review`: the T3 answer)

- **C-027 `settings_store` (D6).** Path `$GRIM_HOME/settings/<workspace_key>.json`, envelope with `deny_unknown_fields`, directory 0700, file 0600. Unusable (parse failure, newer `v`, `workspace` differs) reads empty with a warning, and the first save keeps the old bytes as `<key>.json.unusable`, one slot. No records removes the file. Units for each.
- **C-028 Merge and split (D6).** `InstallState.settings_store`. `load_project` discards every `Settings` record from `state.json`, including the legacy fallback, with a warning, then runs `validate_grim_home`, then merges. Refused: no read, no write. `save` and mid-run persist split by kind at one chokepoint. Units: a project without the kind is byte-identical; a forged record never reaches any consumer; a Refused state persists nothing for the kind.
- **C-029 Global scope.** `SettingsStore::Inline`; records stay in `global.json`; `element` round-trips. Unit.

### P4: consent (mark `review`: trust gate)

- **C-030 Key (D7).** `settings:<binding>@<registry>/<repository>`; hook keys unchanged; a hook `x` never consents settings `x`, in either direction. Units in `consent.rs`.
- **C-031 Ladder.** `ConsentPolicies` from one record read, one flag, one interactivity answer. Each kind answers its own feature flag first. `declared_consent_keys` is the union of enabled kinds. The prompt fires if either kind reaches `ConsentRequired`; accepting records the union; a second flag turned on later reads `consent-drifted`; `verdict` stays per binding. Units over the truth table, and `test_settings_consent.py::test_record_without_settings_keys_re_prompts`, seeding what an older `grim hook allow` leaves.
- **C-032 Prompt.** One function, three nouns. A hooks-only workspace keeps the fragments pinned at `test_hook_arming.py:504` and `test_hooks_boundary.py:444`. "It grants nothing to any other checkout" stays. `test_settings_consent.py::test_prompt_names_settings`.
- **C-033 Flag pair (D7, PD-6).** `--trust-workspace` and `--no-trust-workspace` on `install`, `update`, `add`, with the old pair as `visible_alias`es. `overrides_with` retargets, so last wins across spellings. No env form: the `test_hooks_port_contracts.py` env-inert row gains it. `test_settings_consent.py::test_old_and_new_flags_are_one_answer`.
- **C-034 Verb.** `visible_alias = "consent"` on `Command::Hook` (`src/main.rs:149`). Count stays 26; telemetry stays `hook/allow`, with one row appended at `command_name.rs:144-147`. `allow` records both kinds regardless of flags, recomputing the union. `revoke` removes the record for both. The write-seam allowlist test (`test_hook_consent.py:216`) still holds. `test_settings_consent.py::test_consent_alias_reports_hook_telemetry_name`, `::test_allow_records_both_kinds`.
- **C-035 Gesture and disclosure.** A `settings:` key joins `consentable` in `resolve_for_add`, and `add.rs:624,683-692,776-782` widen. `disclose_bundle_hooks` becomes `disclose_transitive`, warn level, naming settings packages arriving through a bundle or `requires`. A path source answers `None`. Global scope reads consented. `test_settings_consent.py::test_add_is_the_gesture`, `::test_transitive_package_is_disclosed`.
- **C-069 Binding-name control (D2, D10).** Class and location only: unvalidated input at a boundary. The allowlists at `add.rs:773,1328` gain `Settings`, so a settings binding must be a `SkillName`, exit 65 at `add`. The phase refuses the same on a hand-edited config, because the name reaches a file path. `test_settings_consent.py::test_add_refuses_a_bad_binding`; P6 unit `settings_phase.rs::hand_edited_binding_is_refused`.

### P5: format, publish, public surface

- **C-036 Document (D2).** `from_toml_str_with_requires`, `to_layer_bytes` (pretty JSON, newline, sorted keys), `from_layer_bytes` (cap, parse, `Consume`). `requires` splits off; a non-table vendor is refused; datetimes are refused. Hand-written pass, since `flatten` defeats `deny_unknown_fields`. Round-trip units, byte-stable across key order.
- **C-037 Validation (D2).** Every document row of the ADR's table in both modes, as named units: unknown field, unknown vendor (warning at `Consume`), wrong type, enum, `null`, float, range, `{env:` and `{file:` on decoded values and keys, the Codex exclusion, URL userinfo, control characters in rules. `codex.rules` `pattern` is deliberately narrower than Codex's grammar. The 64 KiB cap reads the descriptor size, because the layer is plain JSON. The binding row is C-069.
- **C-038 Build and release.** `grim build --kind settings`, `release_settings`, `annotations_for_settings`, `conventional_source_path`, publish order after hooks and before bundles, `grim schema --kind settings`, `grim describe`, `grim fetch` cap. `test_settings_publish.py::test_build_release_publish`, `::test_invalid_document_exits_65`, `::test_schema_is_additive`.
- **C-039 Bundles, `requires`, export.** `RawBundleSource.settings` appended; members sort last; `[requires.settings]`; `grim export` declines through `admits`. `test_settings_publish.py::test_bundle_member_and_requires_edge`, `::test_export_declines`.
- **C-040 Discovery.** `kind_from_manifest`, `src/catalog/search_match.rs:398`, the stale doc at `registry_catalog.rs:245`, `mcp/server.rs:61`. `index_package.rs` holds `kind` as a free string, so no code changes there. `test_settings_publish.py::test_search_and_describe_show_the_kind`.

### P6: settings engine (mark `hard`)

File and exit assertions live here. Status assertions live in P7 and report assertions in P8.

- **C-041 Preflight (D5).** Three conflict shapes: one pointer two values, one pointer a strict prefix of another, the Codex exclusion across packages. Layers of armed packages only, through `fetch_verified_layer`. Past the flag, `add` treats the new package as armed; with the flag off, preflight is skipped. Documents are compared, not clients. Exit 78, no reason, no `--force`. This pipeline owns the function and the install slot at the first step of `install_all_with_progress`, returning one synthetic `Err`. `test_settings_rule.py::test_conflict_exits_78_on_install`, `::test_prefix_conflict`, `::test_codex_exclusion_across_packages`, `::test_required_feature_and_stale_edge_still_refuse`, `::test_unloadable_layer_fails_its_row_only`.
- **C-042 Offline and coverage (D5).** The five-row table. One test per offline row: `test_settings_lifecycle.py::test_offline_intact_is_a_no_op`, `::test_offline_orphan_removal_needs_no_layer`, `::test_offline_new_client_uncached_warns_and_exits_0`, `::test_offline_changed_plan_with_cached_layers_is_a_normal_run`, `::test_offline_uncached_layer_exits_81_for_settings_only`. Coverage: `::test_client_added_later_gets_its_pieces`, `::test_zero_output_record_converges_when_the_cursor_file_appears`.
- **C-043 The rule (D5).** First match wins, in the order 7, 3d, 3c, 3a, 3b, 2, 1, 5, 4. Rows 1–8, each with its `adopted` after state, as pure units over the cross product of `(F, L, h, adopted)`, asserting exactly one row each, then acceptance. `grim status`, the TUI and the gate share `current_hash`. `test_settings_rule.py::test_rule_row_<n>`, `::test_deleted_record_adopts_identical_and_refuses_different`, `::test_two_declarers_dropped_together_remove_the_piece`, for an update and for a pulled lock.
- **C-044 Whole-phase refusal.** All judging precedes any write. Any row 4, 5 or 7a refuses across all packages, and other artifacts still install. `test_settings_rule.py::test_refusal_lists_every_piece_across_packages`, asserting exit, files and the error's piece count, with a skill in the same run as the positive control.
- **C-045 `--force` (D5).** `All`, `None`, and `Only(nodes)`: a piece is forced if any declaring package is in the set; `add --force` cannot reach an orphan. A remaining `overwrite` refuses the whole phase. `refusals_outside_force` (`add.rs:538`, a `matches!`) matches the new outcome. `test_settings_rule.py::test_add_force_scope`.
- **C-046 Gate (D7).** The four-row run table, including orphans. `validate_grim_home` runs before any consent or record read: all three causes write nothing, exit 0. A path source in a hand-edited lock never applies. A missing `settings_policy` plans nothing. `test_settings_gate.py::test_fresh_clone_writes_nothing`, `::test_declined_changes_no_file`, `::test_grim_home_in_workspace_writes_nothing`, `::test_flag_off_leaves_values`, `::test_path_source_in_a_hand_edited_lock_never_applies`; `settings_phase.rs` unit `missing_policy_plans_nothing`, because every `Full` seat attaches a policy and no CLI path reaches the state.
- **C-047 Lifecycle and seats (D5).** Orphan removal on every `Full` path, no consent, listed. `grim uninstall settings x`: no consent, works with the flag off, through `settings_writer`, pre-flight lists drifted pieces through `SettingsDrifted`, adopted intact stays in `abandoned_entries`, a drifted adopted piece refuses, `--force` removes, a piece another surviving record declares stays. Client set grows through rows 1 and 2; shrinks through `settings_phase::reap_clients` at `update.rs:262`. A relocated root leaves the old value, named. `settings_phase` emits one stderr line per change at apply time and one closing restart line, and `uninstall.rs` fills `UninstallResult.settings_changes`. P8 reads both and proves them. `test_settings_lifecycle.py::test_adopted_value_survives_uninstall`, `::test_a_projected_install_leaves_other_packages_alone`, `::test_dev_refresh_never_plans_settings`, `::test_dropped_client_keeps_user_elements`, `::test_flag_off_client_shrink_touches_nothing`, `::test_uninstall_deep_pointer_and_element`, `::test_shared_element_survives_one_uninstall`.
- **C-048 Declines and shapes (D3, D8).** A project-scope `defaultMode` outside `project_values` declines that piece with a warning. Cursor's absent file declines that client. Required pointers are never removed. Zero applicable pieces write a zero-output record, the row is `skipped`, exit 0, including a settings-only lock with no detected client. An emptied array and object are pruned, and a missing object is created with only the package's keys. `test_settings_scope.py::test_project_default_mode_declines`, `::test_cursor_absent_file_declines`, `::test_cursor_required_pointers_stay`, `::test_zero_applicable_exits_0`, `::test_no_client_detected_exits_0`, `::test_add_then_uninstall_leaves_no_empty_containers`.
- **C-049 Claude project scope (D9).** `.claude/settings.local.json`, never `settings.json`, proven by a positive control in the tracked file. A linked worktree writes its own file. Every project write passes `confined_target`. `ensure_settings_local_excluded` gains this caller, and a tracked file warns `local-file-tracked`. `drop_settings_local_exclude` runs only when neither kind uses the file, and a linked worktree never drops it. `test_settings_scope.py::test_linked_worktree_writes_its_own_file`, `::test_hook_removal_keeps_the_exclude_rule_settings_need`, `::test_tracked_local_file_warns`, `::test_confinement_refusal_writes_nothing`.
- **C-050 Codex own file (D10).** `<codex-root>/rules/grim-<binding>.rules`, rendered by one closed function, `starlark_string()` escaping `\` and `"`, whole-file piece with a byte hash, the same rule table. `default.rules` is never touched. Units parse every rendered file back with a small string reader and compare it with the input, over a corpus: a lone backslash, a trailing backslash, `"""`, a non-BMP character, a long string. `test_settings_scope.py::test_codex_rules_file_lifecycle`.
- **C-051 Partial failure and lost values (D5, D11).** Each file written, then its record persisted. A mid-apply I/O error keeps earlier files recorded, and a retry converges. A crash between write and persist yields adoption. A value a harness dropped later returns through row 1 with no `--force`. `test_settings_lifecycle.py::test_retry_after_partial_failure_converges`, `::test_harness_dropped_value_converges_without_force`. The write race is proven at the seam (PD-23): `write_race_maps_to_exit_75` passes `apply` a stale expected value.

### P7: status and TUI

- **C-052 Status (D8).** `stale`, then a whole-artifact gate, then lifecycle. `settings_arming(...)` reuses `HookArming`; `hook_arming` is untouched. Three causes appended, with their rows in `json-interface.md`, because `test_every_hook_arming_cause_is_documented` reads every token. `gated` and `not-armed` docs widen (`artifact_status.rs:36,47`). `StatusOutput.element` is always present, and the pins at `test_status.py:110,208,588` extend. `local-file-tracked` is advisory (premise 3). `outputs_pending` stays `[]`. `test_settings_status.py`: one test per indicator, plus `::test_reordered_keys_read_installed`, `::test_zero_applicable_reads_gated`, `::test_tracked_local_file_note`, `::test_fresh_clone_reports_workspace_not_consented`, `::test_grim_home_refusal_reads_not_armed`, `::test_confinement_refusal_reads_not_armed`, `::test_flag_off_reads_gated`, `::test_cursor_absent_file_note`, `::test_revoke_keeps_values_and_reads_gated`, including the `grim uninstall` undo, and `::test_harness_dropped_value_reads_missing`.
- **C-053 TUI.** `derive_artifact_state` (`app.rs:1744`) reads the same check. Rows at `app.rs:1983,2166`, `2970`, `3096`. A settings refusal shows the piece list, and Overwrite forces the row's package. The result panel shows the change list, because the TUI sends tracing to a log file. Units in `app.rs`: `settings_row_state_matches_status`, `refusal_panel_lists_pieces`, `change_list_reaches_the_panel`.

### P8: command surface

- **C-054 Reports and refusal surface (D5, D8).** `settings_changes` on four reports, from `InstallRun`, `install_added`'s return, `UninstallResult`, and the `reap_clients` result at `update.rs:262`. `finish` emits `pieces` whichever refusal it kept, and `finish` and `update`'s hard `Err` path wrap in `Error::Settings` only when the outcome is non-empty (PD-14). `update` carries both keys in its report. Stderr lines for pieces come from `finish`. Change lines and the closing restart line are the phase's (C-047), and P8's tests prove them. `test_settings_reports.py::test_refusal_pieces_in_json_and_text`, `::test_changes_survive_a_later_refusal`, `::test_install_after_a_pull_lists_keys`, `::test_add_lists_changes`, `::test_uninstall_lists_changes`, `::test_force_lists_overwritten`, `::test_dropped_package_lists_removed`, `::test_old_value_only_in_the_error`, `::test_current_is_bounded_and_masked`, `::test_restart_note_follows_the_change_list`, `::test_update_refusal_exits_65_with_pieces`, `::test_update_hard_error_keeps_changes`, `::test_update_client_shrink_lists_removed`.
- **C-055 Preflight seats (D5).** `add` and `lock` before the lock is saved, and `update` between `roll_forward` and `lock_io::save`. `add` passes the full lock beside its projection. `test_settings_reports.py::test_add_conflict_leaves_the_lock`, `::test_lock_conflict_exits_78`, `::test_update_conflict_saves_nothing`, `::test_hand_edited_lock_conflict_exits_78`.
- **C-056 Telemetry.** `declared_identifiers` adds settings, kind literal only. No pointer, element or value is emitted. `record.rs` unit asserting a sentinel element never appears in the payload.

### P-X: cross-cutting acceptance

- **C-057 Matrix `test_settings_matrix.py`.** Parametrized: every rule row × JSON, JSONC, TOML × Scalar, Set, Map, plus Object recursion and the own-file class. A generated table keeps the row list equal to the ADR's.
- **C-058 `test_settings_file_fidelity.py`.** Replays the archaeology, with byte-diff assertions on the whole file where formatting is the claim: comments and key order survive for each format (c63bac6, 2369e1e); no competing file beside `.jsonc` (03e59b0); a symlinked dotfile stays a symlink, including a chain (7df05e4); uninstall leaves a modified or adopted value (41fac57, f0c565b); an adopted entry stays reachable once its record drops; a relocated root strands nothing unreported (f053de1); a partial multi-client run records what it wrote (31625da); a key is escaped (2043a62); a repointed `OPENCODE_CONFIG` edits the recorded file (8e913ac).
- **C-059 Trust scenarios.** `test_settings_forged_state.py`: a committed forged `state.json`, and one delivered by a real `git pull` over a real state file (bare repo, two clones), neither applies nor removes, with a user's `deny` rule as the probe, and a warning names the discard. `test_settings_teammate.py`: copied config, pulled value change, a teammate's `--force` through the lock, a teammate's bump, a dropped package.
- **C-060 Real binaries.** `test_settings_real_harness.py`, opt-in `GRIM_REAL_CLAUDE_BIN` and `GRIM_REAL_CODEX_BIN`, skipped otherwise. Rendered files for one package touching every merge class. A broken-file positive control proves the binary reports errors. Codex also runs `codex execpolicy check --rules <file>`, plus a recorded run with a second, deliberately bad rules file, so the reference page states the true behaviour. PD-5 governs credentials. One recorded run.
- **C-061 Manual rig.** `test/manual/README.md` gains `## 13`, with `test/manual/catalog/settings/` and `test/manual/project-settings/`.
- **C-071 Marketplace keys.** `test_settings_marketplace.py::test_claude_and_copilot_key`, `::test_codex_key`: grim writes `/extraKnownMarketplaces/<name>` and `/marketplaces/<name>` for one package, as Map pieces, and uninstall removes only that entry.

### P9: documentation

- **C-062 `settings.md` (reference).** Document format, merge classes, ownership, `--force`, the error, the per-harness field table, the `on-limited` limits (Cursor and Qoder shadowing, Claude worktree shadowing, Copilot reading `.claude/settings*.json` as documented upstream), *delivered, not enforced*, `--force` as unrecoverable with `current` bounded (PD-17), a harness's own reload behaviour per PD-21, and a dropped value returning at the next `grim install`. Declared `reference`. A Rust test `settings_docs_table_matches_registry` parses the table against `settings_fields()`, because `docs_reference_matches_<vendor>_registry` filters lowercase tokens (DOC-TYPE-18). Every runnable fence is bound to `test_docs_settings_reference.py`, `# doc: settings-reference`; any other carries a paired no-run marker. Sidebar beside `mcp-servers`.
- **C-063 `guides/share-settings.md` (how-to).** Three use cases: behaviour defaults, allow and deny rules, a pre-configured marketplace paired with `grim export marketplace`. Links each harness's own docs, with Qoder at `https://docs.qoder.com/cli/settings-reference`. Every fence bound to `test_guide_share_settings.py`, `# doc: guide-share-settings`, count pinned. Sidebar under Guides.
- **C-064 Page updates.** `commands.md`, `json-interface.md`, `artifacts.md` ("six kinds" → seven), `clients.md` (sixth column, every row, with `docs_matrix_row_set_matches_all_and_cells_track_kind_support` at `client_target.rs:1053` extended to it), `concepts.md`, `stability.md` (`:24` a third exception listing the ADR's Unstable surfaces, `:195-250`, `:204-205`, `:448-450` the 74 line, `:471-480`), `upgrading.md` (the three fixes, 0.16.0 per PD-4), plus `mcp-servers.md`, `vendor-metadata.md`, `configuration.md:86-109`, `publishing.md`, `package-index.md:288`, `hosting-an-index.md`, `hooks.md`, `guides/lifecycle.md:162`, `guides/registries.md:154`, `telemetry.md:186`.
- **C-065 Docs gate.** `doc_declaration.py`, `prose.py`, `page_type.py`, `doc_examples.py --changed-only`, `nav_depth.py`, `links_raw.py` and `docs:check` pass on every changed page. Written with the `docs` skill.

### P10: AI config, catalog, `upstream-refresh`

- **C-066 `upstream-refresh`.** `SKILL.md` (stays under 200 lines), `references/domains.md` (`vendors` bullets and sub-table), `references/researcher-brief.md` ("Claims to re-verify"), `references/deep-pass.md` (§ 1 "Settings keys" row, § 2 live check), and `vendor-capability-watchlist.md`: a new section with a `| Capability |` header and an `All rows \`verified YYYY-MM-DD\`` sentence, frontmatter `paths` gaining the settings source and `docs/src/content/docs/settings.md`, plus rows for PD-1, PD-2, each `on-limited` field and the Claude worktree read. `task upstream:stale` reads it.
- **C-067 AI config.** `AGENTS.md`, by in-place edits only, because it sits at its 280-line budget: `CURSOR_CONFIG_DIR` read for this file, the no-variable row names settings consent, the kind list. `.claude/rules.md`; `subsystem-file-structure.md` (`### Settings {#install-layout-settings}`, `$GRIM_HOME/settings/`, the two new anchors); `subsystem-cli-commands.md`; `subsystem-cli-api.md`; `product-context.md`'s kind list; `hex.md` pointers. `task claude:tests`.
- **C-068 Catalog drift review.** `catalog/README.md` trigger list; `grim-usage`, `grim-authoring`, `ai-config-authoring` kind and flag lists (`updating.md:57`, `bundle-spec.md:151`); `compatibility` bumps. `task catalog:verify`.

### G3-ENGINE: convergence gap, the fence and narrowed runs (closing review 3)

Findings are in `.agents/review/review_settings_artifact_kind_closing_3.md`. Every red names the mutant it kills, and the step pastes the red run.

1. **Red.** Write these failing tests first.
   - **Fence units beside `a_global_root_linked_into_the_tree_is_refused`:**
     - a root with `..` after a missing component (C3-H1);
     - a submodule cwd with the root in the superproject (C3-W13);
     - a FIFO `.git`, bounded by a timeout (C3-W15);
     - an unreadable cwd (C3-W16);
     - a repository under `$HOME`, which kills the operand swap at :844 (C3-W4).
   - **`test_settings_engine_root.py`:**
     - a second `install` still names the cause (C3-H4);
     - the link-case remedy says the file links into the repository (C3-H5);
     - `grim --global uninstall` under an in-repository root is refused (C3-W3);
     - `no-applicable-settings` is absent for a refused root (C3-W6).

     Strip every `GIT_*` key from the `git init` env and give that call a timeout (C3-W17).
   - **`test_settings_lifecycle.py`:**
     - an unchanged client does not hold the pin (C3-H3);
     - the `kept: None` branch (C3-W5);
     - the remedy carries `--global` at global scope.
   - **`test_settings_status.py`:**
     - the N-W5 case at global scope (C3-W24, C3-W28), with a skill-row positive control;
     - the `withheld_rows` refused-home return (C3-W7);
     - both `selection_remedy` fallbacks (C3-W8).
   - **Units:**
     - the N-W2 guard flags `/amp.mcpServers`, `/hooks` and `/mcp` (C3-W9);
     - `root_variable` names the variable each `settings_config_path` arm reads (C3-W18);
     - `claude settings.jsonc` and `kiro settings/cli.jsonc` are dropped on load (C3-W30);
     - `https://x\:tok@host/m` is refused (C3-W25).
2. **Fix.**
   - **Fence:**
     - Refuse any root with a `..` component.
     - `holds_git` reads only a regular file.
     - `work_tree` returns the outermost `.git` holder that does not hold `$HOME`.
     - An unreadable cwd refuses every global root.
   - **Decline evidence:**
     - Carry the matched spelling in `RootInWorkspace`.
     - Emit the decline from one place, whenever a selected surface is refused, and consult the fence before the survey hashes.
     - Print the resolved path in the change line when `$HOME` lies in the work tree (C3-H2, legibility half).
   - **Narrowed runs:** `left_behind` counts only clients whose recorded outputs differ from the new pin's. Its remedy is scope-aware and sends a dropped client to `grim update`.
   - **Messages and remedies** (C3-W2, W11, W21, W22, W23, W26):
     - a fence-withheld client is no longer reported as `kept_modified`;
     - the note on a row that applied elsewhere needs no selection change;
     - the global-only remedy names `grim --global add`;
     - the hooks tail says `grim hook allow`;
     - "unknown vendor table" gets a next step;
     - `ClientWithoutSettings` keeps "a newer grim may apply it".
   - **Load and parsing:**
     - The load filter compares the exact spelling except for OpenCode, Amp and Kilo.
     - The userinfo check cuts only at `/?#`.
   - **Structure:** move `prune_orphans`' doc back above its `fn` (C3-W10).
   - Gate: `task rust:verify` plus the named acceptance modules, then `task --force verify`.

### G3-RIG: convergence gap, manual rig and evidence (closing review 3)

1. **Fix.**
   - Give §13e its own `GRIM_HOME`, and make the `in_rig` guard accept only that path (C3-W40).
   - `in_rig` refuses when Claude managed settings exist (C3-W43).
   - Add `task p9:self-test` before `task p9:diff` in the verify-deep principle-9 job (C3-W41).
2. **Record.** Add the `GRIM_REQUIRE_GOLDEN=1` run of the four golden modules and an old-grim run against v0.15.1, with the tree SHA, to § Verification record (C3-W42).
3. Gate: `task shell:verify` and `task claude:tests`, plus the self-test by hand.

### G3-DOCS: convergence gap, pages and records (closing review 3)

Runs after G3-ENGINE and G3-274 land, because it documents their final messages.

1. **Red.** test_docs_kind_lists.py matches "sorts last" and "sorts first" phrasings, and fails on hook-spec.md:473-475 (C3-B2).
2. **Fix pages:**
   - clients.md:503-504 and vendor_antigravity.rs:9-10 say the client serves the Antigravity IDE (C3-B1).
   - hook-spec.md states the kind order.
   - settings.md: the rerun warning (:433-437), :387, :121-124, and the root remedies.
   - json-interface.md:96 (`clients_missing`) and :675, plus consume.md:283-289 (C3-W27).
   - Read `grim status` after a settings install (C3-W48), and a newly selected client is not reported (C3-W47).
   - upgrading.md: BOM acceptance (C3-W20) and the `--force` condition at :535-536 (C3-W33).
   - json-interface.md:97 and commands.md:717-718 (C3-W34).
   - stability.md: settings exit codes become Unstable (C3-W50).
3. **Fix records:**
   - **ADR:** Decision 8's cause table, the token count, Decision 11 and the exit row, the contracts block, and the changelog row for PD-28, PD-30 and PD-32 (C3-W19). Correct the Performance row's claim (C3-W1).
   - **arch-threat-model.md T3 fence paragraph and the settings layout rule:**
     - the repository-set `HOME` edge (C3-H2, record half);
     - the no-`.git` edge (C3-W14);
     - PD-46's Warp residual (C3-W31).
   - **Decision rows:** a new PD row beside PD-28 records the `HOME` edge.
   - subsystem-cli-commands.md:26 says five causes (C3-W12).
   - Correct the watchlist (:288-291) and domains.md:95 (C3-W44).
4. **Gate:**
   - `ocx exec -- env -u OCX_GLOBAL task docs:check`, with its exit recorded in the log (C3-W46);
   - `task claude:tests` and `task catalog:verify`;
   - then `task --force verify`.

## User-experience scenarios

| ID | Action | Expected outcome | Error cases | Tests |
|---|---|---|---|---|
| S-001 | A reader follows `guides/dependencies.md` | Both worked cases run; `grim why` prints the chain | n/a | `test_guide_dependencies.py` |
| S-002 | Publisher writes `settings/team-defaults.toml`, runs `grim build --kind settings`, then `grim release` | One JSON layer with its media type; `grim describe` shows the kind | invalid document → 65 naming the pointer | `test_settings_publish.py::test_build_release_publish`, `::test_invalid_document_exits_65` |
| S-003 | `grim add <pkg>` at project scope, flag on | Claude's local file gains the keys; the other clients decline as global-only with a warning; stderr and JSON list the changes | n/a | `test_settings_scope.py::test_project_add_writes_the_local_file`, `test_settings_reports.py::test_add_lists_changes` |
| S-004 | Fresh clone, non-interactive `grim install` | Nothing written, exit 0, `grim status` row `gated`, `workspace-not-consented` | n/a | `test_settings_gate.py::test_fresh_clone_writes_nothing`, `test_settings_status.py::test_fresh_clone_reports_workspace_not_consented` |
| S-005 | Interactive `grim install`, or `--trust-workspace` | One prompt naming settings; accepting records the union and writes | decline → no file changes, orphans included | `test_settings_gate.py::test_trust_flag_applies`, `::test_declined_changes_no_file`, `test_settings_consent.py::test_prompt_names_settings` |
| S-006 | User runs `/model` in Claude, then `grim install` | Exit 65 `modified`, every piece listed `overwrite`, no setting written, a skill in the same run installs | `--force` overwrites and lists `overwritten` | `test_settings_rule.py::test_refusal_lists_every_piece_across_packages`, `test_settings_reports.py::test_force_lists_overwritten` |
| S-007 | A copied config or a pull changes a value | Same error; `untracked-destination` when no piece has a record | n/a | `test_settings_teammate.py::test_copied_config_refuses`, `::test_pull_changed_value_refuses` |
| S-008 | `grim install --force`, then `grim uninstall settings x` | Grim owns the piece; uninstall removes it; the old value appeared only in the error | n/a | `test_settings_reports.py::test_old_value_only_in_the_error`, `test_settings_lifecycle.py::test_forced_piece_is_removed_on_uninstall` |
| S-009 | The file already holds the package's value | No write, no `--force`, `adopted`; uninstall leaves it | n/a | `test_settings_lifecycle.py::test_adopted_value_survives_uninstall` |
| S-010 | Teammate bumps the package; `git pull`; `grim install` | Intact values change, no prompt, the keys are listed | a hand-edited value → S-006 | `test_settings_reports.py::test_install_after_a_pull_lists_keys`, `test_settings_teammate.py::test_teammate_bump_needs_no_prompt` |
| S-011 | Teammate's `--force` reaches this machine through the lock | Same refusal here | n/a | `test_settings_teammate.py::test_teammate_force_refuses_here` |
| S-012 | Teammate removes the package; `grim install` here | Values grim wrote go, adopted ones stay, no consent, listed | n/a | `test_settings_reports.py::test_dropped_package_lists_removed`, `test_settings_teammate.py::test_dropped_package_keeps_adopted` |
| S-013 | Two packages set different scalars | Exit 78 at `add`, `lock`, `update` and `install`; the lock is not saved | hand-edited lock → the install slot | `test_settings_rule.py::test_conflict_exits_78_on_install`, `test_settings_reports.py::test_add_conflict_leaves_the_lock`, `::test_lock_conflict_exits_78`, `::test_update_conflict_saves_nothing`, `::test_hand_edited_lock_conflict_exits_78` |
| S-014 | A repository commits a forged `state.json` | Discarded with a warning; nothing applied or removed | delivered by `git pull` → the same | `test_settings_forged_state.py` |
| S-015 | The record file is deleted | Identical values adopted; different ones refuse; nothing removed | n/a | `test_settings_rule.py::test_deleted_record_adopts_identical_and_refuses_different` |
| S-016 | `grim uninstall settings x` with a drifted value | 65 `modified`, pieces listed, `--force` removes | flag off → still works | `test_settings_lifecycle.py::test_uninstall_drifted_refuses_and_force_removes`, `::test_uninstall_works_with_the_flag_off` |
| S-017 | A harness writes during the run | Exit 75, retryable, that file untouched | n/a | `settings_writer.rs` unit `write_race_maps_to_exit_75` (PD-23) |
| S-018 | `--offline` | Intact: no-op. Only orphans: removal runs. Changed with cached layers: normal. Uncached: 81 for settings rows, other artifacts install | a new client, uncached: warn, exit 0 | `test_settings_lifecycle.py::test_offline_intact_is_a_no_op`, `::test_offline_orphan_removal_needs_no_layer`, `::test_offline_new_client_uncached_warns_and_exits_0`, `::test_offline_changed_plan_with_cached_layers_is_a_normal_run`, `::test_offline_uncached_layer_exits_81_for_settings_only` |
| S-019 | An older grim meets the lock | 78; `global.json` 74; bundle member 65 | n/a | `test_old_grim_settings.py` |
| S-020 | `GRIM_HOME` inside the workspace | `not-armed`, the cause named, exit 0, no writes | n/a | `test_settings_gate.py::test_grim_home_in_workspace_writes_nothing`, `test_settings_status.py::test_grim_home_refusal_reads_not_armed` |
| S-021 | `grim consent revoke` | Values stay, the row reads `gated`, `grim uninstall` undoes them | n/a | `test_settings_status.py::test_revoke_keeps_values_and_reads_gated` |
| S-022 | Install in a linked worktree | The worktree's own local file is written; the main checkout is untouched | a shared scalar shadows, documented | `test_settings_scope.py::test_linked_worktree_writes_its_own_file` |
| S-023 | A package carries `codex.rules` | `rules/grim-<binding>.rules` rendered, listed, removed on uninstall | a control character → 65 | `test_settings_scope.py::test_codex_rules_file_lifecycle`, `test_settings_publish.py::test_invalid_document_exits_65` |
| S-024 | Cursor has no `cli-config.json` | That client declines with `client-config-absent`, other clients proceed | n/a | `test_settings_scope.py::test_cursor_absent_file_declines`, `test_settings_status.py::test_cursor_absent_file_note` |
| S-025 | A consumer finds a public package with `grim search`, `grim describe`, then adds it | The kind shows; consent applies as for any package | n/a | `test_settings_publish.py::test_search_and_describe_show_the_kind` |
| S-026 | A team pre-configures a marketplace | `/extraKnownMarketplaces/<name>` reaches Claude's and Copilot's files, `/marketplaces/<name>` reaches Codex's | n/a | `test_settings_marketplace.py`, `test_guide_share_settings.py` |
| S-027 | The user reorders keys in the file | Status stays `installed` | n/a | `test_settings_status.py::test_reordered_keys_read_installed` |
| S-028 | A package has only Codex keys at project scope | Zero-output record, row `skipped`, status `gated` with `no-applicable-settings`, exit 0 | n/a | `test_settings_scope.py::test_zero_applicable_exits_0`, `test_settings_status.py::test_zero_applicable_reads_gated` |
| S-029 | The experimental flag is off | `grim add` locks; nothing is fetched, written or removed; status `gated`; applied values stay | n/a | `test_settings_gate.py::test_flag_off_leaves_values`, `test_settings_status.py::test_flag_off_reads_gated` |
| S-030 | `/upstream-refresh` deep pass on Claude | Reports settings keys upstream against the list | n/a | Phase V step 7, recorded |
| S-031 | Codex is installed later, or Cursor's file appears | The next `grim install` writes the new client's pieces | offline and uncached: warn, exit 0 | `test_settings_lifecycle.py::test_client_added_later_gets_its_pieces`, `::test_zero_output_record_converges_when_the_cursor_file_appears` |
| S-032 | A running harness's own save drops grim's value | Status reads `missing`; `grim install` restores it with no `--force` | n/a | `test_settings_status.py::test_harness_dropped_value_reads_missing`, `test_settings_lifecycle.py::test_harness_dropped_value_converges_without_force` |

## Parallelization

P0 and PF land first, each commit alone, then the contract wave as one commit.
Pipelines then launch on dependency-ready. P5 follows P2 because it needs the
list. P6 runs alone: it needs five pipelines, and `installer.rs` allows no
sibling. P9 and P10 wait for the final CLI surface. Those are the only waves
narrower than file-disjointness alone would allow, and contract order justifies
each.

| Pipeline | Scope | Expected files | Wave | Depends on | Marks | Status |
|---|---|---|---|---|---|---|
| P0 | C-003; S-001 | `docs/src/content/docs/guides/dependencies.md`, `docs/astro.config.mjs`, `test/tests/test_guide_dependencies.py` | 1 | — | — | merged |
| PF | C-001, C-002, C-070 | `src/install/{hook_registrar,prune,install_state}.rs`, `test/tests/{test_hooks_fixes_worktree_exclude,test_reap_adopted,test_state_load_rule}.py` | 1 | — | — | merged |
| CW | C-004…C-015, C-018 | `src/oci.rs`, `src/oci/{artifact_kind,layer,requires_decl,settings}.rs`, `src/install.rs`, `src/install/{installer,install_state,install_error,settings_store,settings_writer,settings_phase,json_splice,toml_splice,vendor,client_target,expected_outputs,target,path_anchor,status_badge,prune,uninstall,hook_registrar,claude_config,opencode_config}.rs`, `src/hook/{consent,policy,trust}.rs`, `src/lock/{grimoire_lock,lock_io,effective_set}.rs`, `src/lock/testdata/lock.schema.json`, `src/config/{project_config,declaration,toml_merge,hash}.rs`, `src/command/{add,install,update,lock,build,release,publish,schema,status,uninstall,remove,why,config,config_keys,hook_consent,command_error}.rs`, `src/api/{install,update,add,uninstall,config,settings}_report.rs`, `src/api.rs`, `src/error.rs`, `src/main.rs`, `src/fetch.rs`, `src/mcp/render.rs`, `src/export/{resolve,stage,family,marketplace_export}.rs`, `src/resolve/resolver.rs`, `src/skill/local_pack.rs`, `src/tui/app.rs`, `test/src/{registry,helpers}.py`, `test/tests/{test_hooks_flag_off,test_hooks_port_contracts,test_settings_helpers,test_uninstall}.py` | 2 | PF, P0 | — | merged |
| P-G | C-016, C-017, C-072; S-019 | `test/data/golden/pre_settings_<sha>/**`, `test/tests/{test_golden_pre_settings,test_old_grim_settings}.py`, `test/tools/p9_diff.py`, `.github/workflows/verify-deep.yml` | 3 | CW | — | merged |
| P1 | C-019…C-022; S-017 | `src/install/{json_splice,toml_splice,settings_writer}.rs` | 3 | CW | hard | merged |
| P2 | C-023…C-026 | `src/install/vendor.rs`, `src/install/vendor_{claude,codex,copilot,cursor,opencode,qoder}.rs`, `src/install/path_anchor.rs`, `src/install/installer.rs` (test module) | 3 | CW | — | merged |
| P3 | C-027…C-029 | `src/install/{settings_store,install_state}.rs` | 3 | CW | review | merged |
| P4 | C-030…C-035, C-069; S-005 | `src/hook/{consent,policy,trust}.rs`, `src/command/{hook,hook_consent,add}.rs`, `src/command/hook/{allow,list,revoke}.rs`, `src/cli/options.rs`, `src/main.rs`, `src/telemetry/command_name.rs`, `test/tests/{test_settings_consent,test_hooks_port_contracts}.py`, `src/config/declaration.rs`, `src/command/{status,update,command_error}.rs`, `src/error.rs` (one arm) | 3 | CW | review | merged |
| P5 | C-036…C-040; S-002, S-023, S-025 | `src/oci/{settings,annotations,bundle,release}.rs`, `src/command/{build,release,publish,schema}.rs`, `src/command/testdata/**`, `src/config/project_config.rs`, `src/export/{resolve,stage}.rs`, `src/fetch.rs`, `src/mcp/server.rs`, `src/catalog/{search_match,registry_catalog}.rs`, `test/tests/test_settings_publish.py`, `src/main.rs`, `src/error.rs`, `src/command/search.rs`, `src/mcp/tool_args.rs`, `taskfiles/schema.taskfile.yml` | 4 | P2 | — | merged |
| P6 | C-041…C-051, C-069; S-003…S-006, S-008, S-009, S-013, S-015…S-018, S-020, S-022…S-024, S-028, S-029, S-031, S-032 | `src/install/{settings_phase,installer,install_error,install_state,uninstall,prune,hook_registrar,expected_outputs}.rs`, `src/command/{add,update}.rs`, `test/tests/test_settings_{rule,lifecycle,gate,scope}.py` | 5 | P1, P2, P3, P4, P5 | hard | merged |
| P7 | C-052, C-053; S-004, S-020, S-021, S-024, S-027…S-029, S-032 | `src/command/status.rs`, `src/api/{status_report,artifact_status}.rs`, `src/install/status_badge.rs`, `src/tui/{app,detail,render,state}.rs`, `docs/src/content/docs/json-interface.md`, `test/tests/{test_settings_status,test_status}.py` | 6 | P6 | — | merged |
| P8 | C-054…C-056; S-003, S-006, S-008, S-010, S-012, S-013 | `src/command/{install,update,add,lock,uninstall}.rs`, `src/telemetry/record.rs`, `test/tests/test_settings_reports.py` | 6 | P6 | — | merged |
| P-X | C-057…C-061, C-071; S-007, S-010…S-012, S-014, S-026 | `test/tests/test_settings_{matrix,file_fidelity,forged_state,teammate,marketplace,real_harness}.py`, `test/manual/**` | 7 | P7, P8 | — | merged |
| P9 | C-062…C-065; S-026 | `docs/src/content/docs/{settings,commands,json-interface,artifacts,clients,concepts,stability,upgrading,mcp-servers,vendor-metadata,configuration,publishing,package-index,hosting-an-index,hooks,telemetry}.md`, `docs/src/content/docs/guides/{share-settings,lifecycle,registries}.md`, `docs/astro.config.mjs`, `src/install/{vendor,client_target}.rs` (test modules), `test/tests/{test_guide_share_settings,test_docs_settings_reference}.py` | 8 | P-X, P0 | — | merged |
| P10 | C-066…C-068; S-030 | `.claude/skills/upstream-refresh/SKILL.md`, `.claude/skills/upstream-refresh/references/{domains,researcher-brief,deep-pass}.md`, `.claude/rules/{vendor-capability-watchlist,subsystem-file-structure,subsystem-cli-commands,subsystem-cli-api,product-context}.md`, `.claude/rules.md`, `AGENTS.md`, `.agents/memory/hex.md`, `.agents/upstream-checks.md`, `catalog/README.md`, `catalog/skills/**` | 9 | P9 | — | merged |
| G3-ENGINE | Convergence gap (closing review 3). PD-28 is partial: a `..` bypass (C3-H1), a repository-set `HOME` (C3-H2, by record), the cause silent after the first run (C3-H4), the wrong link-case remedy (C3-H5), a FIFO `.git` hang (C3-W15), the nested-repository gap (C3-W13) and an unreadable cwd failing open (C3-W16). C-047 is partial: `left_behind` names every unselected client (C3-H3). Engine Warns: C3-W2–W11 (W11 code half), W17, W18, W21–W26, W28, W30 | `src/install/settings_phase.rs`, `src/install/install_state.rs` (:1900-1902), `src/install/prune.rs` (doc comments :153-176), `src/install/vendor.rs` (test module), `src/api/artifact_status.rs`, `src/oci/settings.rs` (:441-445, :896, :942), `src/hook/trust.rs` (:381-391), `test/tests/test_settings_engine_root.py`, `test/tests/test_settings_lifecycle.py`, `test/tests/test_settings_status.py` | 10 | P6, P7 | hard, review | merged (G3 landing A) |
| G3-RIG | Convergence gap (closing review 3). C-061 is partial: §7 run after §13e writes the real client dirs (C3-W40), and `in_rig` ignores Claude managed settings (C3-W43). The p9 self-test runs in no gate (C3-W41). No golden or old-grim run record exists (C3-W42) | `test/manual/README.md`, `taskfiles/p9.taskfile.yml`, `.github/workflows/verify-deep.yml`, this plan's § Verification record | 10 | P-X, P-G | — | merged (G3 landing A) |
| G3-DOCS | Convergence gap (closing review 3). C-064 contradicts (C3-B1, Antigravity IDE). C-068 contradicts (C3-B2, hook-spec "sorts last") and is partial (C3-W27, consume.md). C-062 is partial (C3-H4's page claim at settings.md:433-437, C3-W11, C3-W45). C-065 is partial (C3-W46). Record Warns: C3-W1, W12, W14, W19, W20, W31, W44, W47, W48, W50; follow-up doc Warns C3-W33, W34 | `docs/src/content/docs/{clients,settings,json-interface,upgrading,stability,commands}.md`, `src/install/vendor_antigravity.rs` (doc comment :9-10 only), `catalog/skills/grim-authoring/references/hook-spec.md`, `catalog/skills/grim-usage/references/{consume,troubleshooting}.md`, `test/tests/test_docs_kind_lists.py`, `.claude/rules/{arch-threat-model,subsystem-cli-commands,subsystem-install-layout-settings}.md`, `.agents/adr/adr_settings_artifact_kind.md`, `.agents/upstream-watchlist.md`, `.claude/skills/upstream-refresh/references/domains.md`, this plan (§ Decisions, log) | 11 | G3-ENGINE, G3-274 (plan_settings_follow_ups) | — | merged (G3 landing B) |

**Hub and generated files** (no pipeline owns them): `Cargo.lock` (no change, PD-13), `test/uv.lock`, `docs/public/schemas/*.json` (gitignored, `task schema`).

**Contract-wave amendments.** `src/error.rs`, `src/api/*_report.rs` key shapes, `src/oci/settings.rs` types, the `Vendor` trait surface, `SettingsRun` and `InstallTarget` are CW-owned. A later step that must change one is a contract amendment: the orchestrator re-briefs every pipeline that reads it.

**Shared files, by edit order, each in a different wave.**

| File | Order |
|---|---|
| `src/install/installer.rs` | CW, P6 |
| `src/install/install_state.rs` | PF, CW, P3, P6 |
| `src/install/{prune,hook_registrar}.rs` | PF, CW, P6 |
| `src/install/{uninstall,expected_outputs}.rs` | CW, P6 |
| `src/install/{json_splice,toml_splice}.rs` | CW, P1 |
| `src/install/settings_writer.rs` | CW, P1 |
| `src/install/settings_store.rs` | CW, P3 |
| `src/install/{settings_phase,install_error}.rs` | CW, P6 |
| `src/oci/settings.rs` | CW, P5 |
| `src/install/path_anchor.rs` | CW, P2 |
| `src/install/status_badge.rs` | CW, P7 |
| `src/install/vendor.rs` | CW, P2, P9 (test module) |
| `src/install/client_target.rs` | CW, P9 (test module) |
| `src/command/add.rs` | CW, P4, P6, P8 |
| `src/command/update.rs` | CW, P6, P8 |
| `src/command/{install,lock,uninstall}.rs` | CW, P8 |
| `src/command/{build,release,publish,schema}.rs` | CW, P5 |
| `src/command/status.rs` | CW, P7 |
| `src/command/hook_consent.rs`, `src/hook/**`, `src/main.rs` | CW, P4 |
| `src/config/project_config.rs`, `src/export/{resolve,stage}.rs`, `src/fetch.rs` | CW, P5 |
| `src/tui/app.rs` | CW, P7 |
| `docs/astro.config.mjs` | P0, P9 |
| `docs/src/content/docs/json-interface.md` | P7, P9 |
| `test/tests/test_hooks_port_contracts.py` | CW, P4 |

Steps (each a fresh agent; Specify → Implement → Self-check):

- **P0** (own `docs:` commit, written with the `docs` skill)
  1. Guide, sidebar, bound test (C-003), against published fixtures. `task docs:check`. Commit alone.
- **PF** (each runs the `bugfix` skill's order)
  1. C-001: failing `git check-ignore` test, then the fix. `fix(install):` commit alone.
  2. C-002: failing units, then the fix. `fix(install):` commit alone.
  3. C-070: failing test, then the fix. `fix(install):` commit alone, its message naming the class only.
- **CW** (one commit at the end; steps share the worktree)
  1. Vendor types (C-008), then the kind and arms (C-004), because `client_supports_kind` reads `settings_fields()`. Then `cargo check --all-targets` until green.
  2. Lock, config, flag (C-005, C-006, C-007), with snapshots and the row-order pins.
  3. Pointers and readers (C-018), piece record and read path (C-009).
  4. Run types and seats, errors, API types (C-010, C-011, C-012), every caller adapted.
  5. Stubs and the consent shim (C-013, C-014). Module roots registered.
  6. Test infrastructure (C-015). Gate: full gate green, suite behaviour unchanged.
- **P-G**
  1. Generate the fixture from the PF tip binary; `test_golden_pre_settings.py` green on the CW tree (C-016); `verify-deep.yml`.
  2. Build v0.15.1, run `test_old_grim_settings.py` (C-017), and record the run in § Verification record.
  3. Extend the differential key table and its self-test (C-072).
- **P1** (mark `hard`: 19 prior fix commits in this area)
  1. JSON members (C-019).
  2. JSON element ops (C-020).
  3. TOML, all five operations (C-021).
  4. `settings_writer::apply` (C-022).
- **P2**
  1. Claude, Codex tables and paths (C-023, C-024).
  2. Copilot, OpenCode, Qoder tables and paths (C-023, C-024).
  3. Cursor table, paths, roots (C-023, C-025).
  4. Other vendors, surface test, row-count, class golden and denylist units (C-023, C-026).
- **P3** (mark `review`)
  1. `settings_store` (C-027).
  2. Merge, split, discard (C-028), every consumer read over a forged fixture.
  3. Global scope (C-029).
- **P4** (mark `review`)
  1. Key and ladder (C-030, C-031).
  2. Prompt and flag pair (C-032, C-033).
  3. Verb, gesture and the binding control's `add` half (C-034, C-035, C-069).
- **P5**
  1. Document and validation (C-036, C-037).
  2. Build, release, publish, schema (C-038).
  3. Bundles, `requires`, export, discovery (C-039, C-040).
- **P6** (mark `hard`: the rule is the feature)
  1. Pure rule function: C-043's cross-product units turn green, no I/O. Its acceptance tests wait for step 3.
  2. Preflight and install slot: C-041's four refusal tests turn green.
  3. `apply` wiring in `install_all_with_progress`, refusal, force: C-044, C-045, C-043's acceptance tests, C-041's `test_unloadable_layer_fails_its_row_only`, and C-042's five offline rows.
  4. Gate and guard: C-046.
  5. Lifecycle, seats, `reap_clients`, uninstall, partial failure: C-047, C-051, and C-042's `test_client_added_later_gets_its_pieces`.
  6. Declines and shapes, Claude project scope: C-048, C-049, and C-042's `test_zero_output_record_converges_when_the_cursor_file_appears`.
  7. Codex own file, the binding control's phase half: C-050, C-069.
- **P7**
  1. Status (C-052).
  2. TUI (C-053).
- **P8**
  1. Reports, `finish`, error document, text lines (C-054).
  2. Preflight seats (C-055), telemetry (C-056).
- **P-X**
  1. Matrix (C-057).
  2. File fidelity (C-058).
  3. Forged state, teammate scenarios, marketplace keys (C-059, C-071).
  4. Real binaries (C-060); Codex via `npm install --ignore-scripts --prefix <scratch>`; record the run.
  5. Manual rig (C-061).
- **P9** (written with the `docs` skill)
  1. `settings.md`, parity test, bound fences, sidebar (C-062).
  2. `guides/share-settings.md`, bound test, sidebar (C-063).
  3. Page updates, group 1: the seven named pages, with the `clients.md` parity test (C-064).
  4. Page updates, group 2: the rest, then the docs gate (C-064, C-065).
- **P10**
  1. `upstream-refresh` and watchlist (C-066).
  2. AI config (C-067).
  3. Catalog drift review (C-068).

```mermaid
graph TD
  subgraph W1
    P0[P0 dependency guide]
    PF[PF three fixes]
  end
  subgraph W2
    CW[CW contract wave]
  end
  subgraph W3
    PG[P-G golden and old grim]
    P1[P1 splice, hard]
    P2[P2 typed list, vendors]
    P3[P3 record store, review]
    P4[P4 consent, review]
  end
  subgraph W4
    P5[P5 format, publish]
  end
  subgraph W5
    P6[P6 engine, hard]
  end
  subgraph W6
    P7[P7 status, TUI]
    P8[P8 command surface]
  end
  subgraph W7
    PX[P-X acceptance, real binaries]
  end
  subgraph W8
    P9[P9 docs]
  end
  subgraph W9
    P10[P10 AI config, catalog, upstream-refresh]
  end
  PF --> CW
  P0 --> CW
  CW --> PG
  CW --> P1
  CW --> P2
  CW --> P3
  CW --> P4
  P2 --> P5
  P1 --> P6
  P2 --> P6
  P3 --> P6
  P4 --> P6
  P5 --> P6
  P6 --> P7
  P6 --> P8
  P7 --> PX
  P8 --> PX
  PX --> P9
  P0 --> P9
  P9 --> P10
```

**Critical path:** PF → CW → P2 → P5 → P6 → P8 → P-X → P9 → P10. P6 is the largest pipeline.

**Shippable after wave: 1** — the dependency guide and three `fix:` commits stand alone.
**Shippable after wave: 9** — the feature, behind its flag, with docs and catalog. No earlier wave ships the kind, because docs are a Requirement.

## Verification (Phase V, run by /hex-execute after P10)

Pytest commands run from `test/` with `GRIM_TEST_REGISTRY_HOST` set, as `verify-deep.yml:270-274` does.

1. Full gate: `ocx exec -- env -u OCX_GLOBAL -u OTEL_EXPORTER_OTLP_ENDPOINT -u OTEL_EXPORTER_OTLP_HEADERS task --force verify`, real exit code.
2. `ocx exec -- env -u OCX_GLOBAL task docs:check`, then `task catalog:verify`, `task claude:tests`.
3. Golden: `GRIM_REQUIRE_GOLDEN=1 uv run pytest -n 0 tests/test_golden_pre_settings.py tests/test_golden_pre_hooks.py tests/test_golden_pre_dependencies.py`. YAML check of `verify-deep.yml`.
4. Old grim: `GRIM_OLD_BIN=<v0.15.1 binary> uv run pytest -n 0 tests/test_old_grim_settings.py`.
5. Real binaries: `GRIM_REAL_CLAUDE_BIN=/home/mherwig/.local/bin/claude GRIM_REAL_CODEX_BIN=<sandbox> uv run pytest -n 0 tests/test_settings_real_harness.py`.
6. `/security-auditor` on the trust and consent changes (`src/install/settings_*`, `src/hook/**`, `src/oci/settings.rs`, the state load rule, the Starlark renderer, the `git pull` scenario). Blocks are fixed before review. Findings are recorded by location and class only.
7. `/upstream-refresh` deep pass on Claude. The report names settings keys upstream against `settings_fields()`.
8. Report the F-1 issue need to the meta-orchestrator; the DoD box ticks on its link.
9. `git log --format=%s main..HEAD` shows the three `fix:` commits and the `docs:` guide commit before the first `feat` of this feature.
10. Windows, if reachable: `/mnt/c/Users/ecom/grim-wintest/run.sh` against `origin/main`, known failures per grimoire-rs/grimoire#159.
11. Principle 9 differential: fetch `origin`, then `ocx exec -- env -u OCX_GLOBAL task p9:diff`, real exit code. Exit 1 lists a key or byte outside C-072; exit 2 is a vacuous or failed scenario.

## Verification record

- **Old grim (C-017), 2026-11-04:** v0.15.1 built from tag `v0.15.1`. `GRIM_OLD_BIN=<v0.15.1> uv run pytest -n 0 tests/test_old_grim_settings.py`, 9 passed, exit 0. Observed: lock 78, `[settings]` 78, experimental key 78, bundle member 65, `requires` edge 65, `grim add` of the layer 65 with no kind read, hand-seeded `global.json` 74, `hook allow` drops `settings:` keys and keeps hook keys. Every row matches the plan.
- **Real binaries (C-060), 2026-11-04:** Claude Code 2.1.318 (the installed binary) and codex-cli 0.166.0 (`npm install --ignore-scripts` into a scratch sandbox). Every child ran with an environment built from nothing: `PATH`, a scratch `HOME` under the test's tmp dir, `TMPDIR`, the vendor root and five telemetry and update opt-outs. No credential variable, no login, no model call; the only verbs were `claude doctor`, `codex mcp list`, `codex debug prompt-input` and `codex execpolicy check`. `tests/test_settings_real_harness.py` with both binaries set: 5 passed, 2 skipped, exit 0. Both unset: 1 passed, 6 skipped.
  - Claude: `doctor` reports no invalid setting in the rendered `settings.json`, and names both controls (a bad `permissions.defaultMode`, a truncated file). Passed.
  - Codex config: `mcp list` loads the rendered `config.toml` and refuses both controls (a bad `approval_policy`, malformed TOML). `debug prompt-input` reads back the rendered `sandbox_mode` and approval policy, and follows a flipped file. Passed.
  - Codex rules: `execpolicy check --rules rules/grim-team.rules` decides `forbidden` for `rm`, `allow` for `npm test`, and matches nothing for `ls`. Passed. With a deliberately bad rules file beside it, the whole call fails with "failed to parse policy" and prints no decision, so the good file's rule is not applied either. Passed.
  - Not run, each needs a session and so a login: which Claude value wins at run time, and how a Codex session treats a bad file under `rules/`. Both are skips naming that reason, not passes. Copilot: no binary run.
  - Mutation: `[` to `(` in the `.rules` renderer turned both `execpolicy` tests red; reverted, hash checked.
  - Finding withdrawn on 2026-11-04. The run above read Claude 2.1.318's error text for an invalid `defaultMode`, which lists six modes and leaves `manual` out. A direct `claude doctor` run with `manual` itself, credential-free (`env -i`, scratch HOME and config root), reported no invalid settings at user or project scope, while `Manual` and `bogus` controls were both reported. `settings_fields()` and the ADR table are right to take `manual`. At project scope grim takes only `default`, `acceptEdits` and `plan`, which is stricter than Claude.
- **Golden and old grim (C3-W42), 2026-11-06:** on the landing A tree `0484e1e5`, from its rehearsal worktree with its release `test/bin/grim`. Golden: a scratch zot on `127.0.0.1:5000`, then from `test/` `GRIM_REQUIRE_GOLDEN=1 GRIM_TEST_REGISTRY_HOST=localhost:5000 uv run pytest -n 0 tests/test_golden_pre_hooks.py tests/test_golden_pre_dependencies.py tests/test_golden_pre_settings.py tests/test_dependencies_contract.py`, 22 passed, exit 0. Old grim: the v0.15.1 release asset `grimoire-x86_64-unknown-linux-gnu.tar.gz`, its `.sha256` file checked OK, the binary reporting `grim 0.15.1`, as `GRIM_OLD_BIN` for `uv run pytest -n 0 tests/test_old_grim_settings.py tests/test_old_grim_dependencies.py tests/test_catalog_kindless_upgrade.py`, 20 passed, exit 0. Docs gate (C3-W46): `ocx exec -- env -u OCX_GLOBAL task docs:check` exit 0 on the pages step's tree, run twice, the second after its last edit; `task catalog:verify` exit 0; `task claude:tests` exit 0, 91 passed.

## Review record (plan-artifact scope, 2026-11-04)

Panel: `reviewer:spec` (Block 8, Warn 17, Suggest 10), `architect` (Block 5, Warn 12, Suggest 13; no matrix outcome flipped, record store ties), `researcher` (Warn 5, Suggest 2). Cross-model plan review skipped: no codex CLI, no second harness on PATH.

- **Fixed:** every Block and Warn from the spec and architect seats, and their Suggest rows, in the ADR and this plan. Research R-1 to R-5 are fixed in C-021, C-022, C-023, C-037, C-050, C-051, C-054, C-058, C-060, C-062.
- **Declined with a recorded reason:** R-6 (PD-17: a frozen Decision). R-7 needed no change.
- **Security, by reference only:** one pre-existing finding, ADR premise 24, location and class there, fixed by C-070. One missing boundary control, C-069. Full detail goes only in the DONE block (goal § Issue resolution, item 7).
- **Unverified, carried:** R-1's and R-2's sources are search summaries of a third-party mirror (PD-21).

Re-validation, one `reviewer:spec` pass: Request Changes, Block 1, Warn 8, Suggest 6. All 8 round 1 Blocks and 16 of 17 Warns stood fixed, and the rewrite introduced no Block. The cited code was re-read before any edit, and every finding held. All 15 are fixed here: N-B1 by C-072 and PD-22; N-W1 and N-S1 to N-S6 in place; N-W2 in C-011 and C-054; N-W3 by PD-24; N-W4 in C-047 and C-054; N-W5 in the ADR; N-W6 by PD-23; N-W7 in C-046, C-049 and C-052; N-W8 in the P6 steps. The hex scope allows one re-validation, so these fixes carry no further review pass. `task p9:diff` itself was not run, because the code it would diff does not exist yet: Phase V step 11 runs it.

## Goal DoD trace (ticks happen between hex-mode runs, with this evidence)

| Goal item | Evidence |
|---|---|
| Behaviour defaults, permission policy, marketplaces | C-023, C-043, C-071; S-003, S-026; `settings.md` table |
| Public packages | C-038, C-040; S-025 |
| Six harnesses | C-023…C-025; C-060 for two real binaries |
| Follow-up issue | **F-1**, the meta-orchestrator's link |
| `upstream-refresh` check | C-066; Phase V step 7 |
| Changed keys listed | C-054; S-010, S-012 |
| Documentation | C-062…C-065, C-068; Phase V step 2 |
| Dependency guide | C-003, its own `docs:` commit |
| PR, CI, deep verify (DONE block only) | The meta-orchestrator's acts; C-072 and Phase V step 11 run the Principle 9 differential locally first |

## Follow-up issue

For the meta-orchestrator to file (F-1).

**Title:** `Settings kind: support the remaining harnesses and the unverified keys`

**Body:**

```markdown
The experimental `settings` kind ships for Claude, Codex, Copilot, Cursor,
OpenCode and Qoder. See `.agents/adr/adr_settings_artifact_kind.md`, Appendix A
and Appendix B.

## Other harnesses

Add a typed key list, a settings file path and a format for each other client
grim supports: Agents, Amp, Antigravity, Cline, Droid, Gemini CLI, Goose, Junie,
Kilo, Kiro, OpenClaw, Warp, Zed. Verify every key against the harness's own docs
first. A key with unresolved values or an unverified merge stays off the list.

## Unverified keys on the first six

- Claude: `/theme`, `/editorMode`, `/tui`, `/attribution`, `/sandbox/filesystem/*`
- Codex: `/tui/notification_condition`, `/tui/notification_method`, `/file_opener`,
  the granular `/approval_policy` table, `/web_search`, rules `match` and `not_match`
- Copilot: tool rules, and the unread end of the config page
- Cursor: `/sandbox/mode`, `/sandbox/networkAccess`, `/model`
- OpenCode: `/compaction/reserved`, `tui.json` keys, `/share`, `/username`, the
  `config.json` file name
- Qoder: `/model/reasoningEffort`, and array merge between project and user files

## Done when

Each harness has verified rows or a recorded reason for none, and
`/upstream-refresh` covers it.
```

## Rollback

Everything sits behind `[options.experimental] settings`, off by default. A
revert of the feature commits leaves the three `fix:` commits and the guide
standing. No migration runs, so nothing on a user's disk needs undoing beyond
`grim uninstall settings <x>`.

## Risks

| Risk | Mitigation |
|---|---|
| Merge regressions, 19 fix commits of history | One writer, P1 `hard`, C-058 replay |
| CW is large and every pipeline sits on it | Six steps sharing one worktree; the gate is a green unchanged suite; compile probes fixed the file set |
| A vendor changes a key | Experimental flag, `Consume` refusal names a grim update, C-066 |
| Claude drops the "earlier version" read | Watchlist row; a move is a layout move |
| `grim install` exits 65 after a harmless `/model` | The discussion's accepted cost; the message names `--force` and the package |
| A real-binary run needs a login | PD-5: credential-free only; a check that needs a session reports not run, with the reason |

## Schedule log

- 2026-11-04 /hex-plan xhigh complete. Cross-model plan review skipped: no codex CLI, no second harness on PATH.
- 2026-11-04 /hex-execute: baseline `task --force verify` at `dbe35fb`, exit 0, 2264 passed, 60 skipped.
- 2026-11-04 /hex-execute commit shape. Question: every commit needs a verify marker under five minutes old and the goal brief forbids writing it, so a step cannot record work with `--no-verify` alone. Research: `.claude/hooks/pre_commit_verification.py`, retro ledger `project-commit-hook-blocks-no-verify`, a warm gate takes about 3 minutes. Decision: workers stage and never record. The orchestrator gates in the feature checkout, then records each pipeline by path straight away. Parallel waves use detached worktrees whose staged diffs apply onto the feature checkout, so there are no pipeline branches or merges. One gate per wave earns the marker; these per-wave gates are extra to the two gates of a run, and a wave gate's repair stays inside its wave.
- 2026-11-04 wave 1 gate: `task --force verify`, exit 0, 2291 passed, 60 skipped. PF step 3's worker stalled in a tool call and was pulled by the coordinator; `install_state.rs` matched its pre-mutation hash, and the gate covers the remaining build, clippy and modules.
- 2026-11-04 wave 2 gate: `task --force verify`, exit 0, 5061 units and 2296 acceptance passed, 60 skipped. CW step 5's worker stalled and was pulled; the gate supplies the final build and the hook modules it never ran. Workers now run plain single commands.
- 2026-11-04 P4's plan-marked review: 2 Block, 2 Warn, 3 Suggest, one seat (not a /hex-review call). Fixed 2 Block, 2 Warn and all but one Suggest part, declined with a reason in the fix report. Gate exit 0.
- 2026-11-04 P3's plan-marked review: 0 Block, 2 Warn, 1 Suggest, one seat. Both Warns fixed, the Suggest handed to P6. P5 landed with it. Gate: first run exit 201 on two clippy errors, fixed; second run `task --force verify`, exit 0, 5286 units and 2323 acceptance passed, 79 skipped.
- 2026-11-04 P1 landed at `55f7179`. Out-of-set: `src/error.rs` (`TargetIo` at 75 now carries reason `locked`) and one ADR line for the corrected signature. Gate: `task --force verify`, exit 0, 2323 acceptance passed, 79 skipped.
- 2026-11-04 P6 landed in seven commits, `6be776f` through the step 7 commit, one full `task --force verify` per step, all exit 0, the last at 2422 acceptance passed; golden modules 21 passed on a real port after steps 3 to 7. ADR row 5 gained its L-dropped cells: a forced removal under a blocking value drops the output and leaves the file. C-051's per-file persist has one surviving mutant: only a kill between two files could tell it from the error path, so that clause is met by reading. One contract-wave test changed one setup line, footprint to byte hash, as the ADR requires. `lock.rs`'s preflight slot stays with P8. Open questions from the steps are in the closing review's brief.
- 2026-11-04 P8 landed at `5d3d6fb`, gate exit 0, 2451 passed, 79 skipped. P-X steps 1 and 2 landed at `b48dc0b` with three defect fixes their tests found, gate exit 0, 2627 passed, 79 skipped. Those tests were red inside the step and no gate failed, so that fix is not a refinement cycle. P7 landed at `a3fd625`, gate exit 0, 2658 passed, 79 skipped; its patch needed one line resolved by hand in `src/command/uninstall.rs` (`pub(crate)` on a field P8 had retyped). Golden modules after P8 and at `b48dc0b`: 21 passed, exit 0.
- 2026-11-04 **Quiesced for a machine restart. Resume from "Resume hand-off" below.** Nothing is running. Branch `hex/config-artifact-type`, HEAD is the commit recording this entry. **Next step to run:** land P-X step 3 from worktree `px`, then build the TUI failure seam, then P-X step 4. **Open worktrees:** `.agents/worktrees/px`, detached at `b48dc0b`, with three new test modules staged and not recorded (9 tests, its own run exit 0, no full gate yet); no other agent worktree of this run remains. **Refinement cycles spent: 0 of 2** by the recorded rule that a wave gate's repair stays inside its wave. The goal file's wording counts "a red gate's fix pass", and one wave gate went red (wave 3, exit 201 on two clippy errors, rerun exit 0), so it reads 1 of 2 if the coordinator counts that repair. **`/hex-review` calls spent: 0 of 3.** The one-seat reviews of P3 and P4 were not `/hex-review` calls.
- 2026-11-04 Refinement budget, ruled by the meta-orchestrator: **1 of 2 cycles spent**. Goal § Loop shape counts "a red gate's fix pass", and the P3 and P5 wave gate exited 201 on two clippy errors before its fixed rerun exited 0. Any figure of 0 of 2 elsewhere in this file is superseded. `/hex-review` calls: 0 of 3.
- 2026-11-04 Resumed from the hand-off. P-X step 3 landed at `9711aaa`, gate `task --force verify` exit 0, 5513 units and 2667 acceptance passed, 79 skipped. P-X step 5 (manual rig) landed at `969d438`, gate exit 0, same counts; its Deferred: no version-bump walkthrough, no Codex rules own-file example, the README sequences were not run end to end, and shell lint is absent here. The coordinator opened the hand-off's follow-up issues as #274 to #277 (cited below). Refinement cycles: 1 of 2, unchanged. `/hex-review` calls: 0 of 3.
- 2026-11-04 P-X step 4 (real binaries) landed at `77a8052`. Its first gate went red: `task --force verify` exit 201, 1 failed, 2667 passed, 85 skipped. The failure was `test_mcp.py::test_grim_search_pages_an_oci_scope_like_the_cli`, a flake already recorded in `plan_artifact_dependencies.md` (red in a full run, green on rerun), outside this step's one new module. No fix pass was made: the rerun with no change exited 0, 5513 units and 2668 acceptance passed, 85 skipped. Counted as no refinement cycle, since no fix pass ran; the coordinator may rule otherwise. A retro entry records the flake. The run itself is in § Verification record.
- 2026-11-04 The TUI failure seam landed at `869a50a`. Its first gate went red: `task --force verify` exit 201, 1 unit test failed (`log_switch::tests::every_capture_helper_arms_the_interest_guard_t2`; 4002 of 5520 run). That test counts `set_default` capture helpers against `arm()` guards in each source file, and the new `panel_restart_note_matches_the_run_line` armed the guard but captured through `with_default`. The worker had run only filtered tests. **Repair, the second refinement cycle: 2 of 2 spent.** The orchestrator switched the test to the repo's `set_default` guard idiom (three lines, test code only), then ran clippy (exit 0) and the whole unit suite (5375 passed) before the rerun: exit 0, 5520 units and 2668 acceptance passed, 85 skipped. Worker briefs now require one unfiltered unit-suite run at the end of a Rust step; a retro entry records it. P-X is merged.
- 2026-11-04 Meta-orchestrator ruling: refinement cycles **2 of 2**, and no third exists. A further red landing gate that needs a fix, or a closing-review finding that needs a fix pass, ends the run before finalize (goal § Loop shape). Binding from now on: every worker ends its self-check with one full `task --force verify` in its own worktree, real exit 0, and a red there is that step's self-check, not a cycle. Docs steps also run doc_declaration, prose, page_type, links_raw and markdownlint by hand on each changed page, plus `task claude:tests` and `task catalog:verify` where they touch those trees. The landing gate must go green the first time, and the orchestrator lands only the tree a worker gated (index tree hashes compared). A rerun of an unchanged tree, with no fix, for a failure already on record as a flake is not a cycle; each one is logged here with its test name.
- 2026-11-04 P9 step 1, the settings reference page, landed at `d82228d`. The worker's full gate exited 0: 5521 units and 2671 acceptance passed, 85 skipped, 91 AI-config. The page had repeated the PX-4 `manual` finding, so the orchestrator ran `claude doctor` 2.1.318 credential-free with `manual` itself. It was accepted at user and project scope, while the `Manual` and `bogus` controls were reported. The finding is withdrawn (§ Verification record) and the typed list stands. A prose fix inside the same step corrected the page, and its full gate exited 0 with the same counts. The worker's index tree matched the staged tree (`7d268f2`). Landing gate `wave8-p9-1`: exit 0 on the first run, 5521 units and 2671 acceptance passed, 85 skipped, 91 AI-config. Carried into the later step briefs: the stale `configuration.md` experimental-flag text and the hooks-only `hooks.md` consent wording (step 4). Also carried: the `json-interface.md` error-document fields and the `grim add --kind` and `grim hook` help text, as out-of-set doc comments (step 3). markdownlint's MD054 bans the reference links docs-style mandates, a pre-existing conflict that `hooks.md` fails too. ruff was not runnable. A retro entry records the inference pitfall. Refinement cycles: 2 of 2, unchanged. `/hex-review` calls: 0 of 3.
- 2026-11-04 P9 step 2, the guide `guides/share-settings.md`, landed at `0e73a83`. The worker's full gate exited 0: 5521 units and 2673 acceptance passed, 85 skipped. 8 of 8 mutations were caught. The index trees matched (`dff9073`). Landing gate `wave8-p9-2`: exit 0 on the first run, 5521 units and 2673 acceptance passed, 85 skipped, 91 AI-config. markdownlint showed only the known classes (MD013 5, MD054 32). Not mutated: the host-managed `CLAUDE_CONFIG_DIR` skip and the interactive prompt path. Two coordinator rulings on its Deferred list. First, a `--global` settings add with only the project flag on writes nothing and prints nothing, and a flag-off project run is silent too. That is a defect, fixed by a new step FS-1 before P9-3. The ADR decides the effects (line 188: declare and lock, write nothing, status `gated`), so the run is not refused. It follows the hook rule (lines 189 and 487-490), and the hook kind warns per artifact on flag-off (`installer.rs:697`). FS-1 warns once per declared settings package and names the flag, with `--global` at global scope. Second, `guides/use-a-marketplace.md#team-pin-none` may be stale on Codex's project marketplaces: P9-4 checks it against the vendor doc, and P10-1 records any change in the watchlist with its date. Refinement cycles: 2 of 2, unchanged (FS-1 is forward work by ruling). `/hex-review` calls: 0 of 3.
- 2026-11-04 Step FS-1, the gated-settings warning, landed at `726097b`. Tests came first: 4 of 4 failed on the red run. Both mutations were caught and reverted, with hashes checked. The worker's full gate exited 0: 5522 units and 2677 acceptance passed, 85 skipped. The index trees matched (`c26189a`). Landing gate `wave8-fs-1`: exit 0 on the first run, 5522 units and 2677 acceptance passed, 85 skipped, 91 AI-config. Each locked settings package now gets one warning when the flag is off, naming the flag to set, with `--global` at global scope. Exit codes, JSON, files and status are unchanged. In a TUI session the line goes only to `tui.log`, like the other settings warnings. Out of set: two sentences in `settings.md`'s flag-off paragraph. Coordinator rulings on its Deferred list: hooks' flag-off remedy also lacks `--global`, which is shipped hook text, so the coordinator files it and it is not fixed here. `grim status --global` still shows the project-scope remedy for a gated settings package. That text is this branch's `settings_not_applied_message` (added in `005bec6`), reached through `refusal_reason` at `status.rs:1451`, so it is a feature defect. It is folded into P9-3 as an out-of-set `src` edit with a test. Refinement cycles: 2 of 2, unchanged. `/hex-review` calls: 0 of 3.
- 2026-11-04 P9 step 3, page updates group 1, landed at `3209293` (the status fix) and `bc273fc` (pages and help text). Pages: `commands`, `json-interface`, `artifacts`, `clients`, `concepts`, `stability`, `upgrading`. Also the `clients.md` matrix parity test and two help-text doc comments (`add.rs`, `main.rs`). The out-of-set `grim status --global` remedy fix (`policy.rs` with an acceptance test in `test_settings_status.py`) is part of it. Its first full gate inside the step exited 201: `test_telemetry_contract.py::test_stability_page_lists_telemetry_as_unstable` reads "telemetry switches" on one line, and the edit had split it. That was a step self-check, not a cycle; the rewrap gave exit 0, 5522 units and 2678 acceptance passed, 85 skipped. Before landing, the orchestrator compared every docs check on the seven pages with the base at `396bfd7`. prose fell from 1441 to 1434, with no new long sentence: each flagged sentence pairs with a base one of equal or lower word count. doc_examples was unchanged. page_type gives the same single `upgrading.md` DOC-TYPE-08 finding (a SHOULD rule the whole page departs from), now over 19 entries instead of 18, since the new 0.16.0 entry follows the page's own heading form. markdownlint grew only in MD054 (mandated reference links) and MD060 and MD013 on table rows and code. The exception was prose. The step had pushed 8 lines past 80 columns, and 5 more were already long in the base. A whitespace-only re-wrap inside the same step fixed all of them except the `grim add` usage synopsis, which is one code span and was already 107 characters in the base. No prose rule count rose on any page after that. MD013 went from 40 to 40 on artifacts, 7 to 7 on clients, 134 to 135 on commands, 5 to 4 on concepts, 68 to 65 on json-interface, 7 to 11 on stability and 20 to 20 on upgrading, every added one a table row, a fenced line or the synopsis. The re-wrap's full gate exited 0 with 5522 units and 2678 acceptance passed, 85 skipped. Hook sentences are byte-identical: `refusal_reason` now routes `FeatureOff` through `feature_off_message()`, which differs only for settings at global scope. `src` edits outside tests are doc comments only. Index trees matched (`3fe1a02`). Landing gate `wave8-p9-3`: exit 0 on the first run, 5522 units and 2678 acceptance passed, 85 skipped, 91 AI-config.
  P9-3's Deferred items, and where each lands. Kind doc comments in `uninstall.rs:39`, `remove.rs:28`, `why.rs:34`, `build.rs:37`, `mcp-servers.md:395`, `hooks.md`, `configuration.md`, and `concepts.md` heading ids go to P9-4. Catalog "six kinds" goes to P10-3. `subsystem-cli-commands.md` flag and schema rows and the `AGENTS.md` flag pair go to P10-2, because those are P10's files: AI config and catalog are not docs-site pages. Not taken, with reasons: the `clients.md:222` Codex Hook note is about the hooks matrix and holds no settings content, so it is outside this goal (for the coordinator's hook issue). The Claude `manual` claim is already settled by a run (§ Verification record). The lint classes MD040, MD033, MD014, DOC-EX-05, sentences over 25 words in existing text and `upgrading.md` DOC-TYPE-08 predate this branch on the pages it touched, and fixing them is page-wide work outside this goal. Refinement cycles: 2 of 2, unchanged. `/hex-review` calls: 0 of 3.
- 2026-11-05 P9 step 4, page updates group 2 and the docs gate, landed at `3081b1a`. Pages: `concepts` (six heading ids matching today's slugs), `configuration`, `guides/lifecycle`, `guides/registries`, `guides/use-a-marketplace`, `hooks`, `hosting-an-index`, `mcp-servers`, `package-index`, `publishing`, `telemetry` and `vendor-metadata`. Also `docs/check_urls.py` and its test, which gained the `grim-settings` schema, and help-text doc comments in `build.rs`, `remove.rs`, `uninstall.rs` and `why.rs`. `hooks-troubleshooting.md` was reviewed and left unchanged. The worker's full gate exited 0: 5522 units and 2678 acceptance passed, 85 skipped, 91 AI-config. The orchestrator compared the 12 changed pages with the base at `0ca5bac`. Prose fell from 681 to 677, page_type was clean and doc_examples unchanged, and no new prose line exceeds 80 columns. markdownlint grew only in MD054. Its two MD013 keys that read as grown are an edited fenced line and an edited table row, with per-page totals unchanged. `src` edits are doc comments only. Index trees matched (`508b003`). Landing gate `wave8-p9-4`: exit 0 on the first run, 5522 units and 2678 acceptance passed, 85 skipped, 91 AI-config. The Codex marketplace claim in `guides/use-a-marketplace.md#team-pin-none` was stale. Codex's configuration reference (fetched 2026-11-05) lets a trusted project's `.codex/config.toml` define marketplaces, so the section now says so. Its anchor is kept, and its heading still reads "Codex and Cursor" (a Suggest for the closing review). P10-1 copies the result into the watchlist. P9-4's Deferred items go to P10-3: the same stale Codex claim in the catalog's `marketplaces.md`, the catalog `grim schema --kind` lists that omit `settings`, and the `release.rs:46` help-text doc comment, carried there as an out-of-set edit because P9 has closed. Not taken: `docs/public/casts/registries.cast` shows an old kind-list error. It was already stale at base, its expect regex does not pin the list, and a re-recording needs the cast recorder. It is filed as #279. P9 is merged. Refinement cycles: 2 of 2, unchanged. `/hex-review` calls: 0 of 3.
- 2026-11-05 P10 step 1, the `upstream-refresh` settings-keys check and the watchlist settings section, landed at `7f5d8d1`. Changes: the `upstream-refresh` SKILL.md (184 lines, budget 200) and its `domains`, `researcher-brief` and `deep-pass` references. The `deep-pass` § 2 settings check runs credential-free verbs only and tests each value with its own run plus a bad-value control, never from error text. The watchlist gained a `## Settings keys watchlist` section with 9 rows. Five config-root rows were refreshed for the settings outputs; `CURSOR_CONFIG_DIR` changed behaviour, and the other rows keep their upstream dates. Out of set: two `.claude/rules.md` auto-load rows that follow the watchlist's new `paths`. Three rows go beyond the contract: Claude project `defaultMode` (the direct `manual` run), Copilot's shared Claude subset, and Codex with a bad `.rules` file. Their dates are copied from the research artifacts (2026-11-03) or the recorded runs (2026-11-04), and the Codex marketplace row from P9-4 (2026-11-05). `task upstream:stale` exited 0 and reads the new section: a run dated ahead lists all 9 rows. `task claude:tests`: 91 passed. The worker's full gate exited 0: 5522 units and 2678 acceptance passed, 85 skipped. Index trees matched (`2acce57`). Landing gate `wave9-p10-1`: exit 0 on the first run, 5522 units and 2678 acceptance passed, 85 skipped, 91 AI-config. Coordinator rulings on its Deferred list. The watchlist is 375 lines against its 200-line rule budget, but it was already 344 on main, so the overage predates this branch. The dated rows stay in that file because `task upstream:stale` reads them there, and splitting it would move them out of the script's reach. The coordinator opens an issue to split it under budget, and the item is on the closing-review list as decided. Second, `settings.md#codex-rules` does not name the bad-rules-file limit, so it is folded into P10-2 as an out-of-set docs edit. `CURSOR_CONFIG_DIR` in `subsystem-file-structure.md` and `AGENTS.md` stays with P10-2. Left as found: `deep-pass.md` § 1's Hooks row still calls PR #98 unmerged, which is stale but has nothing to do with settings. Copilot, Cursor, OpenCode and Qoder have no credential-free settings verb, so `deep-pass` § 2 records them as `skip`. Refinement cycles: 2 of 2, unchanged. `/hex-review` calls: 0 of 3.
- 2026-11-05 P10 step 2, AI config, landed at `88297a2` (the `settings.md` edit) and `e80e41e` (AI config). Edited: `AGENTS.md` (280 lines before and after, in place), `.claude/rules.md`, `subsystem-file-structure.md`, `subsystem-cli-commands.md`, `subsystem-cli-api.md` and `product-context.md`. `subsystem-file-structure.md` gained a `### Settings {#install-layout-settings}` section, and its `CURSOR_CONFIG_DIR` lines now say what is read. Out of set, one line each: `arch-threat-model.md` I1 and ADR Decision 6 record the two withheld settings cases, checked against `withhold_settings_outputs` and `is_in_another_runs_file` in `install_state.rs`. Also out of set: the ADR's Cursor anchors row (two tags), the `test/src/helpers.py` docstring, and the watchlist "Bad .rules file" row, now "names this limit". `settings.md#codex-rules` names the bad-rules-file limit as the recorded codex-cli 0.166.0 run shows it, with session behaviour unverified. Against the base at `7ed0e64` it gained no new finding beyond one MD054 reference link. `task claude:tests`: 91 passed. The worker's full gate exited 0: 5522 units and 2678 acceptance passed, 85 skipped. Index trees matched (`86aa7af`). Landing gate `wave9-p10-2`: exit 0 on the first run, 5522 units and 2678 acceptance passed, 85 skipped, 91 AI-config. Coordinator ruling on its Deferred list: `subsystem-file-structure.md` was already 914 lines on main against its 200-line budget, and the step takes it to 984. It is the same class as the watchlist, so it goes on the closing-review list as decided, and the coordinator opens an issue for it. Left as found, since each stays true for the kinds it describes: Cursor's variable is still "not honored" for agents and skills at `agents.md:164` and `vendor-metadata.md:488`, and in a skills comment at `test_global.py:921`. Refinement cycles: 2 of 2, unchanged. `/hex-review` calls: 0 of 3.
- 2026-11-05 P10 step 3, the catalog drift review, landed at `ff60bb1` (the `release.rs` help text), `6a71c34` (the bundle reference) and `4488787` (catalog). `catalog/README.md`'s trigger list gains the settings surfaces. The `grim-usage`, `grim-authoring` and `ai-config-authoring` skills and their references now name seven kinds, with settings experimental behind its flag. They name the `--trust-workspace` pair with the hook names as aliases, settings in the `--force`, status, schema and bundle lists, and the troubleshooting cause tokens as `json-interface.md` lists them. `marketplaces.md` matches `guides/use-a-marketplace.md#team-pin-none` on Codex. No version or `compatibility` bump: catalog packages publish at grim's release version, and both grim skills already require `grim>=0.16`. Out of set, carried from P9-4: the `src/command/release.rs` `path` doc comment names every shape `grim release` takes, as `build.rs` does, and `grim release --help` changes in that line only. A coordinator ruling moved two of the step's Deferred items back into it, done by a fix pass inside the step (not a cycle). `catalog/hooks/README.md` and the two hook descriptions name the `--trust-workspace` pair and `grim consent allow` / `revoke`, labelled "since grim 0.16.0" as `commands.md` and `hooks.md` word it, since `CHANGELOG.md` records no 0.16.0 yet. The `artifacts.md` bundle table and its "all member kinds" example gain `[hooks]` and `[settings]`, which `RawBundleSource` and `build.rs` accept. Against the base at `0eaa8a4` the four pages gained no new finding beyond two MD054 reference links, and prose fell from 169 to 163. `task catalog:verify` exited 0; `task claude:tests`: 91 passed. The worker's full gate exited 0: 5522 units and 2678 acceptance passed, 85 skipped. Index trees matched (`66c44b8`). Landing gate `wave9-p10-3`: exit 0 on the first run, 5522 units and 2678 acceptance passed, 85 skipped, 91 AI-config. Out of scope, filed by the coordinator as #282: the catalog `publish.toml` reads version 0.15.0 against Cargo's 0.15.1, and `grim-usage` describes an `[mcp]` path form the parser rejects. Three items left as found go on the closing-review list as Suggests. P10 is merged, and hand-off items 1 to 5 are done. Refinement cycles: 2 of 2, unchanged. `/hex-review` calls: 0 of 3.
- 2026-11-05 **Joint fix pass, landing 1 (settings cycle 3 of 4; follow-ups cycle 1 of 4).** It answers closing review call 1 for this plan; call 2's task-2 rows land in plan_settings_follow_ups. Ten pipelines from `d4a3b67`: engine (3 steps), status, messages (2 steps and a follow-up), detection (2 steps), tests, docs (2 steps), plus G-275, G-282 and G-270R for the follow-ups, and one read-only S-5 research seat (PD-28). Pipeline gates, each `task --force verify` in its own worktree, tree unchanged after: docs step 1 exit 201 (one Cline lock test failed under load at about 20 on 12 cores; the test's own release raced grim's lock, fixed in the tests pipeline with an atomic rename, red with a planted delay); G-282, detection, tests, G-275, G-270R, messages and status exit 0 (2734 to 2767 acceptance passed, 86 to 87 skipped; nextest 5600 to 5612); engine exit 201 (`test_docs.py` parsed the cause tokens from the `Display` impl the step had moved into `as_str`; the parser now reads `as_str`, and the two new tokens are documented by docs step 2, so the engine tree alone cannot pass that test and the rehearsal gate is its gate). None is a landing gate, so no cycle is spent. Deferred Warns: PD-30 (two or more packages, the claim corrected), PD-31. Not run: the real-harness binary tests (no codex binary here; claude would need the owner's login). Rehearsal gate on the exact landing tree `cfd1f554` in the docs worktree (the twelve patches replayed on `d4a3b67` give that tree): exit 0 in 645 s, nextest 5627 passed, acceptance 2769 passed, 87 skipped, AI config 91 passed. Landing: the twelve patches applied in commit order in the feature checkout, then this bookkeeping, then one landing gate; its result is recorded with landing 2.
- 2026-11-05 Landing 1 gate (feature checkout, 16:35:48–16:43:49Z): exit 0 on the first run; nextest 5627 passed, acceptance 2769 passed, 87 skipped, AI config 91 passed. Gated tree `21a925dd` equals the head after `087d55b`. Commits: `09e4f0d`, `ab6b711`, `ef14cd8`, `c5d9ced`, `f8da8d0`, `2bea53d`, `e94a99e`, `95268dd`, `dd47d33`, `475d442`, `b8eb8b8`, `0aede2c`, then the bookkeeping `087d55b`. Settings refinement count: 3 of 4.
- 2026-11-06 **G3 fix pass (settings cycle 4 of 4, the last; follow-ups cycle 2 of 4), landing A.** Closing review call 3 reviewed `9a951ee`. The squash onto `main` (`7a7fcaa` on `9c311ed`, goal record `adb790d`) no longer has it as an ancestor; its content is carried by the squash (local ref `archive/config-artifact-type-pre-squash`), so the review's line numbers refer to `9a951ee` and no new review was asked. Decisions D-1 to D-10 applied as decided: D-1 and D-2 recorded (PD-33), the rest in code or pages. Pipelines from `adb790d`, each with the orchestrator's full `task --force verify` in its own worktree (PD-26), every one green with the tree unchanged: G3-ENGINE-B, the engine Warns (nextest 5661, acceptance 2834 passed, 87 skipped); G3-RIG (nextest 5656, acceptance 2834 passed, 87 skipped); the follow-ups' G3-274 (nextest 5660, acceptance 2835 passed, 87 skipped); and G3-ENGINE-A in six serial steps: A1 the fence, A2 the decline evidence, A3 narrowed runs on G3-274's `configured_clients()` contract, A4 the plan-marked review seat (1 Block, 2 Warn, 7 Suggest), A5 the fixer (the Block, Suggests 1, 4, 6 and 7; Suggest 2 deferred, PD-34) and A6 a second fixer round (`arming` names every client the fence refuses). Its gates: after A3 (nextest 5675, acceptance 2852 passed, 87 skipped) and after A6, recorded with the landing. The review seat's two Warns are docs and go to G3-DOCS. Landing A (PD-3): the patches replayed on `adb790d` in a rehearsal worktree, its full gate, then the same patches applied with `git apply --index --3way` in the feature checkout and one landing gate; commits G3-274, the engine Warns, A1, A2, A3, A5 with A6, the rig (`test:` and `ci:`), then this bookkeeping. The landing gate's result is recorded in the next entry.
- 2026-11-06 Landing A gate (feature checkout, 00:50:08–00:55:46Z): exit 0 on the first run; nextest 5694 passed, acceptance 2853 passed, 87 skipped, AI config 91 passed. Gated tree `0484e1e5` equals the head after `e2620ab`, `git diff --exit-code` 0. Commits `b9a0ace`..`e2620ab`. The rehearsal of the same tree in its own worktree was green too (00:37:47–00:48:03Z, nextest 5694, acceptance 2853 passed, 87 skipped).
- 2026-11-06 G3-DOCS in two steps on the union of every G3 code step: the records (ADR, threat model, settings layout rule, `subsystem-cli-commands.md`, watchlist, `domains.md`; PD-33 landed with landing A) and the pages (settings, JSON, commands, clients, upgrading and stability pages, the `grim-usage` and `grim-authoring` references, the `grim-usage` description, the kind-order test). Pipeline gates green with the tree unchanged: records, before A3 (nextest 5673, acceptance 2845 passed, 87 skipped); pages, before A5 (nextest 5680, acceptance 2853 passed, 87 skipped). After A5 and A6, the orchestrator synced three record sentences (ADR Decision 7, the threat model's fence paragraph, the layout rule) to say no reader opens a fenced file and `arming` names every client it refuses; the pages already stated that behaviour. No catalog version bump: `task catalog:verify` is green. Landing B (PD-3): rehearsal of the exact tree in its own worktree, then `git apply --index --3way` in the feature checkout and one landing gate; commits the records, the pages, the catalog, then this bookkeeping. Its gate's result is recorded in the next entry.
- 2026-11-06 Landing B gate (feature checkout, 01:03:07–01:08:32Z): exit 0 on the first run; nextest 5694 passed, acceptance 2854 passed, 87 skipped, AI config 91 passed. Gated tree `34e4bba5` equals the head after `e344102`, `git diff --exit-code` 0. Commits `64b1a8e`..`e344102`. The rehearsal of that tree was green too (00:56:44–01:02:52Z, nextest 5694, acceptance 2854 passed, 87 skipped), and `task docs:check` and `task catalog:verify` exited 0 on it (C3-W46). The G3 fix pass ends here: every G3 gap row is merged, and its deferrals are in PD-34 and the follow-ups plan's PD-50.

### Resume hand-off

Written so a fresh `/hex-execute` needs nothing else: no per-run scratch path is read. A gitignored copy of the run's briefs, helper scripts, worker reports and gate results sits at `.agents/worktrees/resume-config-artifact-type/`. It is evidence only and nothing below depends on it. P-X's last recorded step is 2, but do not redo step 3: its work stands staged in `px`.

**Order of work**

Items 1 to 5 are done (2026-11-05, see the Schedule log). Resume at item 6.

1. Land P-X step 3. From `.agents/worktrees/px`: `git add -A`, write the staged diff with `git diff --cached --binary` to a file, and apply it in the feature checkout with `git apply --index --3way`. Gate, then record it as `test(settings): ...`. Then bring `px` to the new HEAD with a detached checkout, or remove it and make a fresh one.
2. The TUI failure seam, not yet built (one small `src/**` step). In `src/tui/app.rs` the failure arm of `run_batch_with_progress` and the `MemberAction` arm must read `settings_changes` from the error and absorb them into the settings ledger, so a TUI run that fails after changing values still lists them (goal: "Any run that changes applied settings lists the changed keys"). `change_lines` closes the panel with text that duplicates `warn_restart`: share one wording. `delete_targets` should need no edit once `Deletion.settings_changes` is filled before `failure` is matched; that was only read, so add a test.
3. P-X step 4, real binaries (C-060, PD-5). `test/tests/test_settings_real_harness.py`, opt-in `GRIM_REAL_CLAUDE_BIN` and `GRIM_REAL_CODEX_BIN`. Credential-free under a scratch `HOME`; the sandbox recipe is `.claude/skills/upstream-refresh/references/deep-pass.md` § 2. Codex comes from `npm install --ignore-scripts --prefix <scratch>`, and its run covers `codex execpolicy check --rules <file>` plus the recorded run with a deliberately bad rules file. Nothing has run against a real Codex so far: the read-back is a small string reader in the units. If Claude gives no result without a session, or Codex fails to install or needs a login, that binary reports not run with the reason, and § Verification record says so. Never report either as passed unless it ran. Record the run in § Verification record.
4. P-X step 5, manual rig (C-061). Set P-X `merged`.
5. P9 (four steps, the `docs` skill), then P10 (three steps), one at a time, each landing through a gate. Their hand-off lists are below. `AGENTS.md` stays at 280 lines, in-place edits only.
6. Integration gate with `--force`, concurrent with one `/hex-review`, seats per pipeline plus one for the seams. Pass it the review list below. No cross-model pass exists on this machine (no codex CLI, no second harness): disclose it, never claim one. At most 2 fix passes.
7. Phase V, steps 1 to 11, each with its real exit code, stating plainly any step skipped and why. Step 8 is done: F-1 is #270, F-2 is #271, F-3 is #272.
8. Set `State: review`, `Next: /hex-finalize`, run the upkeep step, update `hex.md` Memory, and remove every worktree this run made. `/hex-finalize` is the coordinator's, not this run's. Ticks in the goal file are the coordinator's too.

**Worker and orchestrator rules this run settled**

- One fresh agent per step: deep-reasoning class for `src/**`, standard class for tests, docs and config.
- Workers stage with `git add -A` and never record. The orchestrator runs the gate in the feature checkout, then records straight away, inside the marker's five minutes. The record command must not name a path under `.agents/worktrees/`: the commit hook then reads that worktree's marker and denies. Never write the marker by hand, whatever the hook's message suggests.
- Every worker runs each shell step as one plain command: no `bash -c`, no long `;` chains with nested `$(...)`. Two workers stalled on such calls and were pulled.
- A step that changed Rust ends with `cargo fmt --all` and `cargo clippy --locked --all-targets -- -D warnings`. One gate went red for lack of this.
- Self-check is one empirical mutation per contract, with the file's hash saved first and checked after the revert.
- Each worker's report ends with a Deferred list; the orchestrator copies those items forward. The next brief names them.

**Environment**

- A new worktree needs `git submodule update --init --recursive`, then `env -u OCX_GLOBAL ocx pull`. Copying `target/debug` from the feature checkout saves a cold build.
- Full gate: `ocx exec -- env -u OCX_GLOBAL -u OTEL_EXPORTER_OTLP_ENDPOINT -u OTEL_EXPORTER_OTLP_HEADERS task --force verify`. Read the real exit code, not a wrapper's. It takes five to six minutes, and a passing one writes the marker.
- Goldens need a registry on `localhost:5000`, or they skip. A scratch `zot serve` on `127.0.0.1:5000` with a throwaway root, then from `test/`: `GRIM_COMMAND=<root>/test/bin/grim GRIM_REQUIRE_GOLDEN=1 GRIM_TEST_REGISTRY_HOST=localhost:5000 uv run pytest -n 0 tests/test_golden_pre_settings.py tests/test_golden_pre_dependencies.py tests/test_golden_pre_hooks.py tests/test_dependencies_contract.py`. Run those four only: a full-suite run outlived the scratch registry's 600 s limit and gave 179 failures that were the dead registry, not the code.
- `GRIM_OLD_BIN` wants a grim 0.15.1 binary. Rebuild it from tag `v0.15.1`, or take `bin/grim-0.15.1` from the copy named above if it survived.
- `task docs:check` exits 201 here on `DOC-URL-01` for two LFS-pointer PNGs, because git-lfs is absent. This predates the branch. Run the later `docs:check` commands by hand and report that.
- shellcheck and shfmt are not installed, so shell lint is a known gap: say so. Ruff has no pin in the repo: `uv run ruff` fails to spawn. The github MCP server failed to connect this session; `gh` is the fallback for the GitHub context check.

**Hand-off list for P9**

- `upgrading.md`: the three fix notes (the worktree exclude moves to the common `info/exclude`; an adopted MCP entry stays and is named in `abandoned_entries`; a state load drops an output its kind may not address). `grim search settings` now filters by kind: a skill named `vscode-settings` no longer answers the bare word.
- `commands.md` update section and `json-interface.md:97`: `reaped_clients` says the output was deleted; neither mentions an adopted entry.
- `commands.md:1193` says an adopted entry is left "with or without --force"; the code and `test_force_uninstall_can_remove_an_adopted_mcp_entry` say `--force` removes it. Pre-existing drift.
- `AbandonedEntry`'s doc in `src/install/uninstall.rs` names only the escape arm.
- `doc_examples.py` has no `--changed-only` flag: run it on the changed pages.
- `json-interface.md` drift for `element`, the `locked` row (`TargetIo` at 75 carries reason `locked`), the error-document keys and the conflict line. It says "settings" with no page to link to until `settings.md` lands.
- Docs and catalog still name only `--trust-hooks`: add the `--trust-workspace` pair and the consent verb, with settings wording on revoke.
- Search and export docs; `docs/check_urls.py` schema lists for settings.
- The three decline causes, the project local file, the exclude rule and `local-file-tracked`. A dangling link is refused at project scope.
- `--force` on `grim uninstall` also means settings pieces, never an intact adopted one.
- The Codex own file: the `/rules` key in reports, the `grim-` prefix, `default.rules` never touched, `rules = []` places nothing, a restart after a change, and the `rules/` directory stays after the last file goes.
- `--force` at a rules file path replaces only an empty directory or a link to a directory. A non-empty directory, or a file at the `rules` ancestor, is a forceable refusal (65), and under `--force` it stops before any write with exit 74 naming the path.
- A shell `CLAUDE_CONFIG_DIR` move warns that a copy may remain at an old root grim cannot name. A settings `env` move names the old copy on install and in `abandoned_entries` on uninstall.
- A repointed `OPENCODE_CONFIG` holds the pin while a dropped value waits behind a set-aside output, so `status` reads outdated and each run re-fetches the layer.
- `grim lock` exits 78 on a conflict among armed packages. With the flag on and the kind in the lock, a damaged state file makes `grim lock` exit 74.
- `telemetry.md:186`: the `grim.artifact.kind` list lacks `settings`.
- `hooks-troubleshooting.md` and `catalog/skills/grim-usage/references/troubleshooting.md` mention cause tokens and need a drift review.
- `commands.md` `grim tui` and any TUI page have no settings states. The `?` overlay changed on settings-free screens too. A successful TUI run writes no restart line; its panel carries the note.

**Hand-off list for P10**

- Stale outside P2: the watchlist row, `subsystem-file-structure.md`, `AGENTS.md` (`CURSOR_CONFIG_DIR`), the `test/src/helpers.py` docstring, the ADR compatibility table.
- ADR Decision 6 and `arch-threat-model.md` I1 say an illegal output is dropped; neither records withheld outputs. One line each.
- `subsystem-cli-commands.md`: widen "hook-only `gated`/`not-armed`/`untrusted`", add `grim lock` exit 78, add the settings states to the `grim tui` row.
- Catalog drift review covers the `--force` text, cause tokens and `--trust-workspace`.

**Review list for the closing `/hex-review`**

- Principle 9: the shared JSON scanner now stops scalars at `/`, a narrow exception to byte-identical output when a comment touches a scalar. `TargetIo` at exit 75 now carries reason `locked`: does a shipped path reach it with no reason today? A settings-free `?` overlay in the TUI lost three blank rows. `config_report.rs` adds a list entry that `task p9:diff` may flag.
- `settings_phase.rs` carries a module-level `expect(clippy::result_large_err)`. `refused_bindings` adds a production `install -> command` edge. `tui/state.rs` imports a command-layer type.
- Layer loads: two per run (preflight, then apply), with conflicts rerun over what apply loaded. Offline and uncached, a package with no keys for a selected client warns every run; online it reloads every run. A zero-output record offline warns "not applied for claude" even for a Codex-only package.
- `refusals_outside_force` names only the first declarer of a shared piece, so the hint can be broader than needed.
- A gated package's recorded value can be changed by an armed one, because the conflict check compares armed packages only, per the ADR. No test.
- Refused `GRIM_HOME`: five gate tests were never seen red. `SettingsStore::Refused` under a valid home has no cause and reads `missing`. Whether `grim install` exits 78 through `scope_policies` under a refused home with a malformed global config is unverified. A set-aside own-file output there counts as undeclared and holds the pin.
- C-051's per-file persist has one surviving mutant, so that clause is met by reading. A record naming only unknown clients is dropped on uninstall with a warning and no test.
- `update`'s JSON row reads `updated` for a withheld package. A `--client` narrowed run moves `record.source` while unselected clients' outputs were not judged. `status` and `current_hash` with a directory at an own file are unchecked. The held pin's offline re-fetch is unverified.
- `install` does not list an adopted value in `abandoned_entries` when a package is dropped from the lock; `uninstall` and `update` do. ADR row 3c says report it.
- A hand-edited bad binding is refused at install, not at load: `grim lock` accepts it.
- `src/install/settings_phase.rs` `landing` and `Phase::write_file`: class, residual path-confinement behaviour around a link at project scope, accepted under N2; check it stays accepted.
- P6 step 7 changed one setup line of a contract-wave test, `install_state.rs::an_own_file_settings_output_hashes_the_file`, from a footprint hash to a byte hash, as the ADR requires. Check it is no weakening.
- `test_relocated_root_strands_nothing_unreported` was rewritten: a shell root move leaves nothing that can resolve the old file. A stored root on `ClientOutput` was rejected for now.
- P8: the restart line moved from the phase to each run-ending seat, and `install_and_persist` returns `InstallFailure` (ADR amended).
- Claude `manual` for `permissions.defaultMode`: the PX-4 finding is withdrawn (§ Verification record). Claude 2.1.318 accepts `manual`, so the typed list has no defect. Only the client's error text omits it. Check that no page or skill repeats the withdrawn claim.
- The TUI failure seam: after a refused delete, the panel still names `grim install --force` as the remedy, where `grim uninstall --force` applies. Its two failed-run tests are Unix-only (a read-only `.grimoire`), so Windows has no test of that path. `member_action` was extracted from `run`'s event loop to be testable.
- `guides/use-a-marketplace.md#team-pin-none`: the heading still reads "Codex and Cursor", though the section now says Codex reads a committed marketplace from a trusted project. The anchor is frozen; check whether the heading text should change (Suggest).
- `vendor-capability-watchlist.md` is 375 lines against its 200-line rule budget. Decided, not a finding: it was already 344 on main, `task upstream:stale` reads the dated rows from that file, and the coordinator filed the split under budget as #280.
- `subsystem-file-structure.md` is 984 lines against its 200-line rule budget. Decided, not a finding: it was already 914 on main, the same class as the watchlist, and the coordinator filed it as #281.
- `upgrading.md:490` still tells readers to re-run `grim install --trust-hooks`. The alias works; check whether the page should name `--trust-workspace` (Suggest).
- The `artifacts.md` `{#bundle-example}` example keeps a shortened digest (`sha256:8f4b…`), so it does not build as written. It predates this branch (Suggest).
- The same example, headed "all member kinds", has no `[bundles]` member, though nested bundles has its own example. It predates this branch (Suggest).

**Follow-up issues** (opened by the coordinator 2026-11-04; not re-listed as asks)

- MCP reap neighbours (PD-25): a same-update pin roll that reaps an adopted entry; a two-output client loses kept-modified from the record; `reaped_clients` repeats a two-output client; `install_mcp`'s stale removal ignores adopted. **#274.**
- Shipped `remove_member` and `remove_array_element` in `src/install/json_splice.rs` cut through `cut_range`: a comment before a comma can leave an unparseable JSONC file. **#275.**
- The hook-worded `record()` log, `ArmRefusal` text and "hook consent could not be recorded" log reused for settings runs. **#276.**
- Kindless cached rows. **#277.**
- `grim lock` and `grim add` moved a hand-repinned digest back to the tag's digest, seen twice. Not filed: branch `hex/lock-pin-stability` covers it.
- Two security items (old prefixed consent records; `validate_grim_home`) are recorded by reference in the goal file by the coordinator, not filed as issues.

**Disclosures for the final report**

- PF step 3 wrote no report file; its worker stalled and was pulled, and the gate covered what remained. CW step 5's worker stalled likewise.
- Before the closing review, per-step gates all exited 0 except wave 3's first run. Gate results: baseline 2264 passed, wave 1 2291, wave 2 2296, P-G 2296, P4 2311, P3 and P5 2323, P1 2323, P6 steps 2323, 2327, 2348, 2363, 2392, 2417, 2422, P8 2451, P-X 2627, P7 as above. Commits since the plan record of P5: `55f7179`, `6be776f`, `d94ffea`, `8834139`, `ad5f381`, `c04df7e`, `6902c07`, `e8ad3f7`, `5d3d6fb`, `b48dc0b`, `a3fd625`.
- P7 step 2's clippy ran before a comment-only edit; the full gate covers it since.
