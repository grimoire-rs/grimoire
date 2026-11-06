# Goal: Harness configuration as a grim artifact type

Source: .agents/discussions/config-artifact-type.md · Written: 2026-11-03 by /hex-loop

## Definition of done

- [x] "The first slice covers behaviour defaults, permission policy, and pre-configured marketplaces" — evidence: 91cd4ea (typed key list: behaviour defaults, permission policy, marketplace keys), 9711aaa (marketplace key scenarios), d82228d (`settings.md` key table)
- [x] "Public packages are in scope, as well as own machines and teams." — evidence: 3352416 (document format, publishing and discovery), 9711aaa (teammate scenarios), 0e73a83 (guide to sharing settings through a registry)
- [x] "First-slice harnesses: Claude, Codex, Copilot, Cursor, OpenCode, Qoder." — evidence: 91cd4ea (key list for the six harnesses), 77a8052 (real Claude Code 2.1.318 and codex-cli 0.166.0 runs, credential-free)
- [x] "A follow-up GitHub issue tracks support for every other harness." — evidence: https://github.com/grimoire-rs/grimoire/issues/270
- [x] "The `upstream-refresh` skill gains a check of each vendor's settings keys" — evidence: 7f5d8d1 (settings-keys check in `upstream-refresh`, dated watchlist rows read by `task upstream:stale`)
- [x] "Any run that changes applied settings lists the changed keys" — evidence: 5d3d6fb (changed keys listed on every run), 869a50a (a failed TUI run lists them too)
- [x] "Documentation is a first-class deliverable: the reference page, the sharing guide and updated pages" — evidence: d82228d (reference page `settings.md`), 0e73a83 (sharing guide `guides/share-settings.md`), bc273fc and 3081b1a (updated pages), 6a71c34 and ff60bb1 (bundle and help text)
- [x] "A how-to guide for declared dependencies, `docs/src/content/docs/guides/dependencies.md`" — evidence: 8d4d35b (`docs/src/content/docs/guides/dependencies.md`)
- [ ] Follow-up issues folded into this PR (owner, 2026-11-05: "We should address: … in this session", and "Fold into settings PR"). Each box below ticks when its issue's own "Done when" holds on this branch; the PR body closes it — evidence: the commit per issue.
  - [x] [#270](https://github.com/grimoire-rs/grimoire/issues/270) remaining harnesses and unverified keys — evidence: fa72cc8 (316 rows across 13 harnesses, 226 new; every key left off has its reason in its research artifact or ADR Appendix B; one dated watchlist row per harness for `/upstream-refresh`)
  - [ ] [#271](https://github.com/grimoire-rs/grimoire/issues/271) indexer support (work in `grimoire-rs/indexer`, its own PR; see § Context)
  - [ ] [#272](https://github.com/grimoire-rs/grimoire/issues/272) vscode extension support (work in `grimoire-rs/vscode`, its own PR, no release; see § Context)
  - [x] [#274](https://github.com/grimoire-rs/grimoire/issues/274) MCP reap gaps — evidence: 37f509c
  - [x] [#275](https://github.com/grimoire-rs/grimoire/issues/275) JSON removal next to a comment — evidence: 187c447
  - [x] [#276](https://github.com/grimoire-rs/grimoire/issues/276) consent messages on settings runs — evidence: 251524e
  - [x] [#277](https://github.com/grimoire-rs/grimoire/issues/277) kindless cached catalog rows — evidence: 059e89a
  - [x] [#278](https://github.com/grimoire-rs/grimoire/issues/278) hook flag-off remedy at global scope — evidence: 701fe1c
  - [x] [#279](https://github.com/grimoire-rs/grimoire/issues/279) stale registries cast — evidence: 1eeee3d
  - [x] [#280](https://github.com/grimoire-rs/grimoire/issues/280) split the watchlist under budget — evidence: e942a3d
  - [x] [#281](https://github.com/grimoire-rs/grimoire/issues/281) split `subsystem-file-structure.md` under budget — evidence: 5c267f9
  - [x] [#282](https://github.com/grimoire-rs/grimoire/issues/282) catalog drift — evidence: a023238
  - [x] [#283](https://github.com/grimoire-rs/grimoire/issues/283) flaky search-paging test — evidence: d2be51d (root cause: the driver closed stdin and rmcp dropped request 2 after 5 s; bounded proof per § Context: regression test red before the fix, 50 of 50 targeted runs under `-n auto`, every gate of the run green)
- [ ] PR merge-ready (DONE block only) — evidence: the PR URL, and the
  PR is not a draft.
- [ ] CI green per job (DONE block only) — evidence: every check run on
  the PR head SHA with its conclusion; every skipped job states its
  skip reason (its `if:` or path filter).
  A skip with no reason is not green.
- [ ] Deep verify passed (DONE block only) — evidence: the full documented verification result.

## Autonomy

- Prompting: never — no question waits for a human; a doubt runs
  § Issue resolution.
- Granted acts (authority: the pasted prompt): none
- Forbidden or narrowed acts: no harness binary runs with the owner's login, credentials or real `HOME` (`.claude/skills/upstream-refresh/references/deep-pass.md` § 2). One exception, the owner's in-session answer of 2026-11-05: codex may run under the owner's login as /hex-review's cross-model reviewer, for reviews only, from a throwaway worktree with the repo's instruction, hook and credential files removed. The fallback in the plan's PD-5 to a capped `claude -p` probe is struck: real-binary checks run credential-free in a sandbox, and a check that needs a session reports not run, with the reason.

## Issue resolution

Every doubt — an ambiguous requirement, a design question, a failure
with an unclear cause — runs this protocol:

1. Delegate the research to a sub-orchestrator.
2. Record question → research → decision in this file or in the PR.
3. Defer to a GitHub issue only in hard cases — the research ends with no
   decision, or the decision needs an act outside the grants; the issue
   link is the recorded decision.
4. Every doubt not deferred ends in an action.
5. A pre-existing failure that blocks done is in scope and runs this
   protocol.
6. Findings outside this goal's scope become follow-up issues.
7. Secrets and credential-bearing logs are never written to any committed
   file, commit message, PR text, PR comment or review comment, or issue.
   Security findings are never filed as issues:
   this file records them by reference only — location and class, no
   secret value or exploit detail — and only the DONE block reports them
   in full.

## Loop shape

- Entry point: Run the /hex-plan skill on "Harness configuration as a grim artifact type, per .agents/discussions/config-artifact-type.md".
- Refinement rounds: 4 per budget (pasted prompt I8, 2026-11-05, raised from 2; the owner, in session: "The budget can be seperated by you."). Four budgets: the settings work; this repo's follow-up issues; `grimoire-rs/indexer`; `grimoire-rs/vscode`. A cycle that repairs work of two budgets charges both. Each budget counts outer cycles: each review ⇄ execute
  pass is one, and so is every failed repair or retry cycle — a red
  gate's fix pass, an execute or finalize retry, a post-finalize CI fix ⇄
  re-finalize pass. Inner review-fix rounds do not count, and their limit
  is untouched.
  Past a budget, the DONE block reports every remaining criterion `not met`
  and the run stops.
- Ticks: /hex-loop commits nothing. The session creates or switches to
  the branch the pasted prompt's I9 names and commits this file first on
  it. A box is ticked only between hex-mode runs, never during one, and
  each tick is committed at once; every tick lands before the final
  /hex-finalize, none after it. The `(DONE block only)` criteria are
  evidenced only in the closing DONE block, never ticked.

## Rules

- Run rules:
  - None.

## Emphasis

None.

## Context

- Discussion: .agents/discussions/config-artifact-type.md (Decisions, Threads, Open questions with recommendations, Verification)
- Research: .agents/research/research_config_artifact_*.md (5 artifacts, 2026-11-03)
- First deliverable per the discussion's Intent: "the loop's first deliverable is an ADR, here also amending the trust records"
- Core rule per the discussion's Decisions: "The file decides; a record never authorises an overwrite."
- Acceptance detail lives in the discussion's Verification section.
- At writing, the discussion, the five research artifacts and the Memory row in .agents/memory/hex.md are uncommitted in this checkout.
- Deferred to issues on this repo, because no grant covered the other repos: indexer support #271, vscode support #272. On 2026-11-05 the owner answered "Branches, push and PRs" for both repos in session, but the pasted prompt that followed says "One PR per repo touched, other repos only as granted" and "Also granted: none", and I9 omits any grant it does not quote. Then the owner, in session, directly: "I explcility instruct and allow you to touch the other repos as well, preferably with a suborchestrator running the same hex-plan, hex-execute hex-review loop into hex-finalize." Reading: a direct owner instruction in this session, not a relayed grant. One sub-orchestrator per repo branches (`hex/settings-kind`), plans, executes and reviews, and commits locally. That repo's /hex-finalize runs in this session (I9), with push, the one PR, and draft → ready. No release. A re-paste carrying these grants in I9 was offered to the owner.
- Paused at 7cc908b for an owner restart; resumed 2026-11-04 with /hex-execute on hand-off items 1-5, stopped at ee377cd before the integration review (I8 retro first).
- Follow-ups opened 2026-11-04/05 (session grant): #274 MCP reap gaps (PD-25), #275 json_splice comment cut, #276 hook wording on settings consent messages, #277 kindless cached catalog rows, #278 hook flag-off remedy omits --global, #279 re-record the stale registries cast, #280 split the watchlist under budget, #281 split subsystem-file-structure.md under budget, #282 catalog drift (grim-usage [mcp] syntax, publish.toml version), #283 flaky test_grim_search_pages_an_oci_scope_like_the_cli, #284 untrack retro inbox entries on main.
- Not filed: `grim lock` / `grim add` moving a hand-repinned digest. Covered by branch hex/lock-pin-stability (aa539e8 "fix: one rule for what a pin answers").
- Security findings, by reference only (§ Issue resolution 7; in full only in the DONE block):
  - S-1: `src/hook/consent.rs`, consent record read path. Class: consent-namespace confusion for records written before the settings prefix guard (P4 review B1 residual). Owner decision.
  - S-2: `validate_grim_home`. Class: armable state resident in a repository; the guard covers the workspace only. Inherited, recorded in the ADR.
- Decision (§ Issue resolution, meta-orchestrator, 2026-11-04). Q: does "the I8 cap is spent" stop the run at 2 of 2? Reading: § Loop shape refines I11 (I10) with "Past 2 ... the run stops". Decision: at 2 of 2, forward work continues with no further repair cycle; the first event that calls for a third cycle (a red landing gate needing a fix, a closing-review fix pass, a finalize retry, a CI fix pass) ends the run, and the DONE block marks the rest not met.
- Decision (§ Issue resolution, meta-orchestrator, 2026-11-05). Q: does the bounded proof for #283 (plan PD-5) stand in for "50 consecutive full-suite runs"? Research: one full run takes 5–6 min, so 50 take about 4.6 h of serial machine time competing with this run's gates, and parallel load could itself cause flakes. Decision: yes, as I4's recorded strong reason. The proof has three parts: a deterministic regression test, 50 consecutive targeted runs of the four affected modules under `-n auto`, and every full gate of the run. #283's box ticks only after the 50 targeted runs pass. The PR body states the substitution.
- Refinement counts: settings work 4 of 4 (the cap was 2 until the re-pasted prompt of 2026-11-05); follow-ups 2 of 4; indexer 1 of 4; vscode 2 of 4. Cycle 3 (settings) and cycle 1 (follow-ups) are the one joint fix pass dispatched 2026-11-05 after closing review call 2, covering both syntheses. Cycle 4 (settings) and cycle 2 (follow-ups) are the fix pass after closing review call 3. `/hex-review` calls: 3 of 3 here, 3 per other repo. Settings cycle 1: wave 3 clippy repair (exit 201, rerun 0). Cycle 2: TUI failure seam gate repair (cd69f80). Not counted: the P-X step 4 flake rerun (68d37be), no fix made, failure already on record as a flake. Red runs inside a worker's own step are its inner self-check.
- Closing review 2026-11-05 (call 1 of 3, at e73a58c beside the integration gate, exit 0): Request Changes, 3 Block, 4 High, 55 Warn, 106 Suggest. Synthesis: `.agents/review/review_settings_artifact_kind_closing.md`. A fix pass would be a third refinement cycle, so the run stopped there (§ Loop shape: "Past 2 ... the run stops"). Resume: Run the /hex-execute skill on .agents/plans/plan_settings_artifact_kind.md, review-fix loop against that synthesis, after the owner raises the refinement budget.
- Resumed 2026-11-05 on the re-pasted prompt (I8 cap 4) and the owner's in-session answers: follow-up issues #270–#283 folded into this PR, codex allowed as reviewer.
- Decision (§ Issue resolution, meta-orchestrator, 2026-11-05). Q: fix the closing review now, then build the follow-ups? Reading: each fix pass after a review is one cycle, and two cycles remain. A fix pass now, a second after the follow-ups' review, and any CI fix would need three. Decision: build the follow-ups first as forward work, then run one integration gate beside a closing review (call 2, with codex) over everything. One fix pass (cycle 3) then answers both syntheses, call 1's and call 2's. Phase V and /hex-finalize follow, and cycle 4 stays in reserve for CI.
- Tooling (2026-11-05): uv, shellcheck, shfmt, lychee, ruff, cargo-deny and hawkeye joined the project `ocx.toml` (uv had come from the owner's global toolchain). git-lfs is installed, so `docs:check` passes. Gate command, corrected: `env -u OCX_GLOBAL -u OTEL_EXPORTER_OTLP_ENDPOINT -u OTEL_EXPORTER_OTLP_HEADERS ocx exec -- task --force verify`. The old form, with `ocx exec` first, composed the owner's global toolchain under `OCX_GLOBAL=1`, so the gate's shell lint never saw the project tools.
- Follow-ups opened 2026-11-05 from the follow-up plan's deferred findings (session grant): #285 vendored rule depth files load every session, #286 no YAML/JSON5 splice for Goose and OpenClaw settings, #287 four first-party rules over the 200-line budget with no test.
- Follow-up execution done 2026-11-05 (`.agents/plans/plan_settings_follow_ups.md`, waves 1–3, tip 282c28d): every wave's landing gate green at its first run, follow-ups budget 0 of 4. #283 bounded proof: regression test red before the fix ("the slow grim_search call (id 2) got no response; ids [1]"), 50 of 50 targeted runs under `-n auto` (08:01–08:46Z), every gate of the run green.
- Follow-ups opened 2026-11-05 from the follow-up execution report (session grant): #289 OpenCode permission schema drift (PD-19), #290 cached rust:verify skips docs-vs-code tests, #291 settings kind has no integer type, #292 project scope and second settings files for the new harnesses, #293 no gate runs ruff, #294 broken links in docs/README.md, #295 install and add reports do not name an abandoned adopted MCP entry. YAML and JSON5 are already #286.
- Not filed, upstream and not this repo's (other repos not granted): Droid `doctor --checks config` crashes on a bad sandbox block; Antigravity 1.5.0 has an undocumented `--settings` flag; Kiro's V3 agent auto-upgrade rewrites agent files; Junie keeps `allowlist.json` and a `byok` block; Cursor's changelog names `autoAcceptWebSearch`; the vendored docs-quality rule's `links_raw.py` command fails (fix upstream in `ocx-sh/grimoire-lore`).
- Owner-reversible decisions, for the PR body: Copilot `sandbox/userPolicy/*` stays off; OpenCode `/share` ships as `manual`/`disabled` only; `--force` replaces a user's Claude `attribution: false` like any value of another type (PD-39); Droid marketplaces take all four source shapes, without Claude's github-only narrowing (PD-36).
- Queued for the closing fix pass (in scope, found during the follow-ups):
  - Detection: grim detects Antigravity only by `~/.gemini/config` and Warp only by `~/.warp`, but their settings live in `~/.gemini/antigravity-cli` and the XDG `warp-terminal` folder, so settings outputs can be missing from `grim status`.
  - OpenCode: `opencode_config.rs:123-133` never reads `config.json`, which OpenCode 1.19.5 merges beneath `opencode.json[c]` (#270 listed the file name as unverified).
  - Docs: `json-interface.md:657-663` describes the grim-home causes in hook terms only; the catalog-cache-downgrade heading in `upgrading.md` names only 0.13.
  - Tests: S-003 (#275) has no acceptance test; `install_state.rs` `every_listed_key_survives_load` omits the new clients.
- Indexer sub-orchestrator done 2026-11-05: branch hex/settings-kind in grimoire-rs/indexer (worktree ~/dev/grimoire-rs/indexer-wt-settings-kind), review call 1 Approve after one fix pass; indexer budget 1 of 4; local gate red only on load timeouts in unchanged files (restore.test.ts, detect_run.test.ts, dev.test.ts; solo passes at load 43-48), so clean-runner CI decides. Cross-model pass not run: nox refused codex 0.166.0 (containment not established, adapter verified against 0.153.4).
- Grim drift the indexer found, for the closing fix pass: src/catalog/index_announce.rs:44 lists five kinds without settings; docs json-interface.md:506 `requires` sort list omits settings; the ADR says settings uses the OCI empty config, against artifact_kind.rs:192; the indexer conformance seed is not regenerated (optional).
- Security finding by reference (S-3, indexer repo, pre-existing): PackageDetail.astro:113-139. Class: CWE-451 (UI misrepresentation of critical information).
- VS Code sub-orchestrator done 2026-11-05: branch hex/settings-kind in grimoire-rs/vscode (worktree ~/dev/grimoire-rs/vscode-wt-settings-kind, tip 6653d31, 21 commits), review call 3 of 3 Approve, gate `task --force verify` green in docker image grim-vscode-gate:local (1548 passing, 7 pending), vscode budget 2 of 4. Cross-model pass not run: nox refused codex 0.166.0 (C-1007, C-1020). Release pairing left out by plan.
- Grim wording drift the vscode run found, for the closing fix pass: `grim fetch --description` on a settings package with no companion fails with a message calling it a "skill".
- Security finding by reference (S-4, vscode repo): `confirmOverwrite` dialog text. Class: CWE-451 (UI misrepresentation: an OCI-derived artifact name reaches a dialog unsanitised).
- Indexer and vscode finalize wait on a re-pasted goal whose I9 quotes push, PR and draft → ready for those repos (C-805a); until then both remote acts are reported not met.
- Retro checkpoint 2026-11-05 (`/hex-retro --loop`): b58d602; 11 proposals, none applied (vendored targets or outside the loop allow-list). Six upstream drafts (P-1, P-2, P-3, P-5 and P-7 for `michael-herwig/arcana`; P-6 for `ocx-sh/grimoire-lore`) are not filed, because other repos are not granted. Local candidates filed (session grant): #296 commit hook (P-8), #297 OCX_GLOBAL in test/taskfile.yml (P-9), #298 plan contract probes (P-10). P-11 is #290.
- Integration gate 2026-11-05, beside closing review call 2: `task --force verify` exit 0 on tree 4e15e8f (nextest 5600 of 5600, AI-config tests 91, acceptance 2734 passed, 86 skipped).
- Closing review call 2 of 3 (xhigh, at b58d602): Request Changes, 4 Block, 6 High, 42 Warn, 63 Suggest new. Call-1's 3 Blocks and 4 Highs are still open. Synthesis: `.agents/review/review_settings_artifact_kind_closing_2.md`. The cross-model leg ran degraded: codex's sandbox would not start (bwrap loopback), so attempt 2 got the diff in its prompt with no shell. It raised one High, folded as a Warn (N-W13) plus a stated convention. Queued findings: Q2, Q6, Q8, Q9 and Q10 confirmed; Q3 and Q5 rejected; Q1 moved out of scope (#303); Q4 is on main (#301); Q7 is docs-only (N-W32).
- Follow-ups opened 2026-11-05 from closing review call 2's out-of-scope list (session grant): #299 install_mcp discards the reap results, #300 an unreadable dropped-client config reads as absent, #301 fetch calls every miss a skill, #302 InstallPass refactor, #303 indexer-before-grim release order.
- Security finding by reference (S-5, this repo, review O4): `src/install/vendor.rs:1452-1458` (`env_dir`, `xdg_config_dir`) and its twin `path_anchor.rs:188-190`. Class: CWE-59/CWE-73, a vendor-root environment value relative to or inside the workspace (T3). Predates the branch for MCP and agents; the branch moves the settings rows of Kiro, Gemini, Kilo and Warp onto it. The fix pass decides whether settings writes refuse such a root without a layout move, and records the decision.
- Joint fix pass done 2026-11-05 (one outer cycle: settings 3 of 4, follow-ups 1 of 4). Landing 1: 087d55b, 12 fix commits plus bookkeeping; gate green at its first run (nextest 5627, acceptance 2769 passed, 87 skipped). It closes call-1's 3 Blocks and 4 Highs, and S-5 (e94a99e: settings at global scope refuse a relative or in-repository vendor root, causes `settings-root-relative` / `settings-root-in-workspace`, PD-28). Landing 2 (b1bb58f; landing gate rerun green on tree 028dd41: nextest 5638, acceptance 2826 passed, 87 skipped): G-275, G-282, G-270R, G-270, G-274 close call-2's N-B1 to N-B4 and N-H1 to N-H6. Each pipeline gate was green, and the rehearsal on the identical code tree was green (nextest 5638, acceptance 2826 passed, 87 skipped). Deferred Warns, each with its reason in the plans' § Decisions: C1:36 (lock), C1:47 (release-body dedup, #304), call-1 line 26 (a security class, by reference: `install_state.rs` `is_hook_payload`, an authorization key checked by shape only; consent still gates arming), call-1 line 27 (would change ADR Decision 6), C1:32 (ADR claim corrected instead).
- Decision (§ Issue resolution, meta-orchestrator, 2026-11-05). Q: does landing gate 2's red first run count as a refinement cycle? Research: exit 201 after 10 s at `rust:clippy:check`, a rustc internal compiler error in the feature checkout's stale incremental cache (`verify_ich.rs:80`, `shallow_lint_levels_on(install_all_before_reap)`); the same code tree had passed the full rehearsal gate minutes before, and the run left the tree unchanged. Decision: not counted, as with the 68d37be flake rerun. No code changed: `cargo clean -p grimoire`, then the same gate on the same tree (plus a one-line correction of the plan's own landing record).
- Security finding by reference (S-6, this repo, call-1 line 26, deferred): `src/install/install_state.rs` `is_hook_payload`. Class: an authorization-relevant key checked by shape only; the workspace key is not canonicalised. Consent still gates arming.
- Closing review call 3 of 3 (xhigh, at 9a951ee): Request Changes, 2 Block, 5 High, 50 Warn, 30 Suggest. Of call 1's and call 2's 17 Blocks and Highs, 14 are closed and 3 partial (C1:17, C1:19, N-B2). Synthesis: `.agents/review/review_settings_artifact_kind_closing_3.md`; gap rows G3-ENGINE, G3-RIG, G3-DOCS (settings plan) and G3-274 (follow-ups plan). The cross-model leg ran degraded, as in call 2 (bwrap loopback; diff in the prompt, no shell). The reviewer resolved its doubts as D-1 to D-10 (synthesis § Deferred Findings). D-1 records a repository-set `HOME` as an accepted gap until a safe identity source exists; owner-reversible, for the PR body.
- Decision (§ Issue resolution, meta-orchestrator, 2026-11-05). Q: spend the last settings cycle on call 3's fix pass, leaving none for a CI fix? Research: two doc-accuracy Blocks and two security Highs in the new root fence are open, so a PR flipped to ready without them would not be merge-ready; no `/hex-review` call remains either way. Decision: yes, one fix pass (settings 4 of 4, follow-ups 2 of 4, since G3-274 charges both). Mitigation as before: each worker runs the full gate in its own worktree before hand-off, and landing gates must pass on the first run. A red CI run after finalize would need a fifth cycle, so it ends the run with CI reported not met. The fix pass also takes the branch-authored items 4, 6, 13, 16 and 18 of call 3's out-of-scope list (settings surfaces this branch wrote).
- Decision (§ Issue resolution, meta-orchestrator, 2026-11-05). Q: how does the branch meet `main`, which moved 5 commits (9c311ed, 149626f, 8110f2a, 52379ce, dddf811) with conflicts in 10 files, when /hex-finalize's rebase halts on any conflict? Research: merging `main` in does not help, because the rebase drops the merge and replays every branch commit (checked on git 2.43); a 106-commit rebase meets the same conflicts commit by commit; finalize's `reset --soft` discards the input history anyway. Decision: one squash of the branch diff onto the fetched `main`, conflicts resolved once, and the full gate green in the sub-orchestrator's own worktree before the branch ref moves. Forward work, not a cycle. The old history stays on the local ref `archive/config-artifact-type-pre-squash`, so every SHA in this file still resolves. Commits carry a DCO sign-off from then on (`main` 149626f).
- Security findings by reference (§ Issue resolution 7; closing review call 3):
  - S-7 (C3-H1, this repo, in the fix pass): `src/install/settings_phase.rs` root fence, `resolved_prefix`. Class: CWE-41, path equivalence; attacker T3.
  - S-8 (C3-H2, this repo, resolved by record D-1): `src/install/settings_phase.rs` root fence with `src/env.rs` home lookup. Class: CWE-15, external control of a configuration root; attacker T3.
  - S-9 (out-of-scope item 1, pre-existing, extends S-2): `src/hook/hook_registrar.rs` `validate_grim_home` at global scope. Class: CWE-15; attacker T3.
  - S-10 (out-of-scope item 9): `src/install/install_state.rs` `merge_settings_records` ordering against `drop_illegal_outputs`. Class: path containment of a loaded record; attacker N2 only.
  - S-11 (out-of-scope item 11): `.github/workflows/verify-deep.yml` permissions. Class: CI least privilege, write scopes at workflow level.
  - S-12 (out-of-scope item 14): `src/hook/hook_registrar.rs` `git ls-files` call. Class: CWE-426, local repository configuration reaching a git subprocess.
- Follow-ups opened 2026-11-05 from closing review call 3's out-of-scope list (session grant): #305 an emptied JSON container drops its interior comments, #306 a BOM-prefixed Claude settings file loses its `CLAUDE_CONFIG_DIR` layer, #307 docs drift ("never deletes", json-interface example messages), #308 manual rig §7 runs global add and install on the real client directories, #309 the update reap blocks the async runtime, #310 the Cline lock-wait test never asserts the wait, #311 a vendor decline prints twice, #312 the refused-dependency remedy omits `--global`, #313 one tracing-capture test helper and no install_state → installer import, #314 a CR-only line comment. Items 1, 9, 11 and 14 are security findings (S-9 to S-12), not filed; items 4, 6, 13, 16 and 18 go into the fix pass.
- Squash onto `main` done 2026-11-05: 7a7fcaa (tree 120bb45) on `main` 9c311ed, with a DCO sign-off; `hex/config-artifact-type` moved there from fc2adb7, and the old history is on `archive/config-artifact-type-pre-squash`. Ten conflicts were resolved once. Main's edits to the two split rules moved into the split files (`subsystem-vendor-paths.md`, `subsystem-path-anchors.md`, `.agents/upstream-watchlist.md`), and `ocx.lock` regenerated byte-identical. Two compile fixes were needed (`config_keys.rs` `Some(false)`, `expected_outputs.rs` `element: None`). Gate in the sub-orchestrator's worktree: `task --force verify` exit 0 at its first run (nextest 5656 of 5656, AI-config tests 91, acceptance 2834 passed, 87 skipped). Noted, not changed: `vendor-metadata.md` still says "pool-capable via shared_skills", which is stale on `main` too.
- Fix pass after closing review call 3 done 2026-11-06 (settings cycle 4 of 4, follow-ups cycle 2 of 4; all four budgets of this repo's work are now spent or closed): 14 commits b9a0ace..f9561d2 in two landings. Every pipeline gate, both rehearsals and both landing gates in the feature checkout were green on their first run (landing A: nextest 5694, AI-config tests 91, acceptance 2853 passed, 87 skipped, tree 0484e1e; landing B: nextest 5694, AI-config tests 91, acceptance 2854 passed, 87 skipped, tree 34e4bba). Closed: C3-B1, C3-B2, C3-H1, C3-H3, C3-H4, C3-H5, C3-W37 (code half) and the gap rows' Warns, plus out-of-scope items 4, 6, 13, 16 and 18. C3-H2 is settled by record (D-1, PD-33), with its readable-path half in code. An inner review round inside the pass found one Block (a status read of a fenced, no-longer-selected client's file), fixed in 32d221f. Recorded instead of fixed, with reasons in PD-34 and the follow-ups plan's PD-50: Suggests 2, 3 and 5, and an old-pin registration on a re-selected client (needs a schema addition). W42 golden and old-grim (v0.15.1) runs recorded in the plan. `task docs:check` and `task catalog:verify` exit 0. No review ran after this pass: all three `/hex-review` calls are spent. Degradation: one docs claim (v0.15.1 exits 74 on a JSON file that starts with a byte-order mark) rests on the review and the ADR, not on a run here.
- Follow-ups opened 2026-11-06 from the fix pass's report (session grant): #315 project MCP records accept `.jsonc` for clients that read only `.json`, #316 `read_entry_value` reads an unparseable TOML config as an absent member, #317 the global `--log-level` flag is parsed but never applied.
- Security findings by reference (§ Issue resolution 7; fix pass after call 3):
  - S-13 (this repo): `src/install/settings_phase.rs` `holds_git`. Class: CWE-367, a check-then-open window on the repository marker.
  - S-14 (this repo, fails closed): the same root fence's repository walk. Class: an over-wide fenced tree from a marker in a high ancestor; the cost is false refusals only.
