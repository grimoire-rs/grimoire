## Code Review: main...hex/config-artifact-type (hex-review call 2 of 3, tier xhigh, loop mode)

### Summary
- Verdict: **Request Changes**
- Tier: xhigh (auto). Overlays: breadth=adversarial, rca=on, adversary=on. Loop mode, so the meta-plan gate was announced, not asked.
- Baseline: `main` (merge base 03393c51). Reviewed HEAD: b58d602ed0a6970fb91947068c6d176afaa927b3.
- Diff: 332 files, +57,840 / −3,561 (branch). The focus delta e73a58c..HEAD is 114 files, +11,656 / −2,584, in 19 commits. The largest is fa72cc8: 226 rows for 7 new harnesses plus the Appendix B keys.
- Counts, new at HEAD, after dedup and max-wins:
  - **4 Block**
  - **6 High**
  - **42 Warn**
  - **63 Suggest** (as tagged by the seats; merged duplicates removed)
- Call-1 findings: all 3 Blocks and all 4 Highs are **still open** at HEAD (table below). The plan routes them to the parent plan's fix pass (cycle 3) by design (plan § Header "Excluded").
- Integration gate (reported by the meta-orchestrator): `task --force verify` exit 0 on tree 4e15e8fd. Results:
  - nextest: 5600 of 5600 passed.
  - AI-config tests: 91 passed.
  - Acceptance: 2734 passed, 86 skipped.

  The gate is green, but it has no test that reaches the Kiro load drop (N-B1) or the comma-ending comment (N-H5).
- Cross-model: **ran, degraded.** codex 0.166.0, `-m gpt-6.1-sol`, from a contained worktree. Attempt 1 could not start its read-only sandbox (`bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted`), so it read no file. Attempt 2 embedded the e73a58c..HEAD diff of `src test docs/src catalog taskfiles` in the prompt and had no shell. Its "not found" property lines are therefore weak evidence. Details are in § Cross-Model Adversarial.
- Why Request Changes:
  - 4 new Blocks, plus 3 open call-1 Blocks.
  - Three systemic causes each affect at least 3 findings (RCA clusters R1, R3, R5).
  - One architect-flagged contract break (N-B4).
- Architect: the delta honours the ADR's three row categories, PD-17 (all 168 new-harness rows are global), the merge classes, and append-only anchors (`antigravity-cli-root`, `warp-config-root`). Two problems remain:
  - One one-way door must close before 0.16.0: `kind_read` (N-H4).
  - The detection markers lag the new settings roots (N-H1, N-W1).
- Researcher SOTA: the JSONC member cut and the stdio driver match the established approach. The `kind_read` bool is the right cache marker once N-H4 lands. OpenCode's undocumented `config.json` layer is a documentation gap only (N-W32).
- Security: no new vulnerability in the delta. All seven trust-boundary properties hold:
  - expansion refusals complete in both modes;
  - no row runs publisher code;
  - marketplace credential check;
  - fixed root segments;
  - splice differential;
  - consent coverage;
  - supply chain.

  Two gaps predate the range: the URL-userinfo spelling gap (Suggest) and relative env vendor roots (out of scope, to be filed).

### Call-1 Blocks and Highs at HEAD

| Call-1 finding | Status at HEAD | Evidence |
|---|---|---|
| Block: settings.md:203 (also :189, :204), `install` lists an adopted value in `abandoned_entries` | **open** | Moved by P-270 to settings.md:215-216 and :201. Still claims an install-time `abandoned_entries`; src/command/install.rs is unchanged in the range. |
| Block: json-interface.md:563, `required_by` sort omits settings | **open** | Now :564, text unchanged. See also the sibling N-B3 at :506. |
| Block: stability.md:692, registry-sourced kinds omit settings | **open** | Moved by P-277 to stability.md:698, text unchanged. |
| High: settings_phase.rs:1718, `--client`-narrowed run strands values | **open** | settings_phase.rs changed only at :324 and :726 (P-HOOK). Region unchanged. |
| High: guides/dependencies.md:29 (:169-170, :183-184), bundle claim false | **open** | File untouched in the range. |
| High: test/manual/README.md:953, part e writes real client dirs | **open** | Untouched in the range. |
| High: test_settings_real_harness.py:94-105, no platform guard (CWE-668) | **open** | Untouched in the range (s4, t2). |

Call-1 Warns changed by the delta:
- **Fixed:** json_splice.rs:247 (the scalar `/` stop) by 187c447 (s9).
- **Widened:**
  - installer.rs:1449, now :1513. Raised to Block as N-B4 because the published contract is contradicted.
  - status.rs:1483-1490, the cause without a remedy. The no-applicable-settings instance is N-W8.
  - The docs-parity test now covers 316 rows (Suggest).

Every other call-1 Warn and Suggest stands as recorded and is not re-raised.

### Queued-finding verdicts

| # | Verdict | Location | Severity | Seat |
|---|---|---|---|---|
| Q1 | **Partly confirmed.** The comment is stale, but it governs nothing: `kind_str` is exhaustive. The real exposure is external, because the released indexer refuses `kind: settings` (out of scope, item O6). | src/catalog/index_announce.rs:44 (also index_package.rs:27, catalog_service.rs:99) | Suggest | s12 |
| Q2 | **Confirmed** (N-B3) | docs/src/content/docs/json-interface.md:506 | Block (Accuracy) | t4 |
| Q3 | **Rejected.** grim pushes the OCI empty config with `artifact_type: None` for every kind (registry_client.rs:289-318, release.rs:615-620). artifact_kind.rs:192 serves only the tier-2 read path. The kind rides `com.grimoire.kind` (annotations.rs:806). | src/oci/artifact_kind.rs:192 | — | t5 |
| Q4 | **Confirmed, but predates the branch.** A resolve miss is labelled "skill" for every kind and shows the reserved `__grimoire` tag. It has been on main since eef13d6c (2026-07-11); settings is one more instance. | src/fetch.rs:90-98 | Warn, out of scope (O3) | t1 |
| Q5 | **Rejected, already fixed by P-270.** The row (now :167) names OpenCode, Kilo and Gemini. The emitted message is vendor-neutral and echoes no value, reproduced for Kilo and Gemini. | docs/src/content/docs/settings.md:167 (was :155) | — (key-vs-value nit: Suggest) | t4 |
| Q6 | **Confirmed.** Antigravity: a CLI-only home is detected as `gemini` only, reproduced (N-H1). Warp on Linux is detected only through `~/.warp` (N-W1). Decided: widen both (§ Deferred). | src/install/vendor_antigravity.rs:186; src/install/vendor_warp.rs:252 | High; Warn | s1, s2, t5, t6 |
| Q7 | **Partly confirmed.** The upstream layering (`config.json` < `opencode.json` < `.jsonc`) is real but documented only by grim's live check and one downstream source. grim's value outranks a user's `config.json` value without an ownership refusal. The settings kind's exposure is new on the branch. Docs-only fix (N-W32). | src/install/opencode_config.rs:123-133; docs settings.md:337 | Warn | t6 |
| Q8 | **Confirmed** (N-W29) | docs/src/content/docs/json-interface.md:657-663 | Warn | s12, t4 |
| Q9 | **Confirmed** (N-W28). Retitle and keep `{#catalog-cache-downgrade}`. | docs/src/content/docs/upgrading.md:284 | Warn | s8 |
| Q10a | **Confirmed** (N-W23) | plan:191 (S-003); src/install/uninstall.rs:461 | Warn | s9 |
| Q10b | **Confirmed** (N-H2) | src/install/install_state.rs:5903-5928 | High | s4 |

### Stage 1 — Correctness

#### Spec compliance (post-implementation traceability)
Seats s1–s3 (P-270 a/b/c), s5 (P-274), s7 (P-HOOK), s8 (P-277), s9 (P-275), s10 (P-283, P-279, f3c0fb7), s11 (P-282, P-CFG), and s12 for the seams. Every pipeline ID was located. The gaps are in § Convergence.

**Block**
- **N-B1** src/install/install_state.rs:1879-1883 (called from `is_settings_piece`, :1843-1845):
  - Defect: `is_settings_file` compares only file names. Kiro's output is anchored at `kiro-root` with relative path `settings/cli.json`, which never equals `cli.json`, so **every Kiro settings output is dropped at each global state load**. The next run sees a value grim did not record. Install then refuses with 65, and uninstall cannot remove the value.
  - Evidence: reproduced by s4 with the HEAD binary; the code path was confirmed by the orchestrator's own read.
  - Remedy: compare the anchored relative path against the answer's path below its anchor, with json/jsonc spelling, rather than the file name.
  - Order: land N-W4's withheld arm in the same change (see the order note there). [s4]
- **N-B2** docs/src/content/docs/clients.md:499-503 (also src/install/vendor_antigravity.rs:7-10 and :82; .agents/upstream-watchlist.md:74 and :96):
  - Defect: the page says the `antigravity` client targets the 2.0 desktop product and that `agy` CLI users "are not served by this client name". Both claims are false: the 25 new rows configure the CLI's `~/.gemini/antigravity-cli/settings.json`, and upstream (read 2026-11-05) says the CLI reads `~/.gemini/config/skills/`, shared with the App.
  - Remedy: rewrite the section once N-H1 lands. [s1; evidence t5]
- **N-B4** src/install/installer.rs:1513, against docs/src/content/docs/commands.md:821-823 (and stability.md:89-93):
  - Defect: the `Settings` arm of `client_supports_kind` answers from `settings_fields()` alone, ignoring scope and platform. The published `clients_missing` contract ("skips any configured client whose vendor cannot host that artifact's kind at that scope") is therefore false for settings rows:
    - at project scope for 12 of 13 hosts;
    - at global scope for Warp off macOS and Linux.
  - Provenance: this escalates call-1's Warn at installer.rs:1449, which fa72cc8 widened from 5 hosts to 12. Recorded once; the fix closes the call-1 Warn too.
  - Remedy: extract the file-free half of `settings_surface` (settings_phase.rs:631-635) as `settings_file_here(client, workspace, scope)` and call it from both sites. Leave out the `landing` and `exists()` I/O. Rewrite `client_supports_kind_reads_settings_from_the_key_list` per scope, with a Warp row whose answer is gated by `cfg!`. [s2 Warn, t5 Block; max-wins]

**High**
- **N-H1** src/install/vendor_antigravity.rs:186:
  - Defect: global detection keys only on `~/.gemini/config`, so a home holding only `~/.gemini/antigravity-cli` never selects `antigravity`. Reproduced: the settings package reads `gated`, and its note names `gemini`.
  - Decided (§ Deferred D2): OR `antigravity_cli_root(home).exists()` into the Global arm. Ship it with:
    - an upgrading.md#autodetect row and a unit test;
    - updates to subsystem-client-detection.md:31 and clients.md:484-485;
    - removal of the test/src/helpers.py:319-321 workaround. [s1, t5, t6]
- **N-H4** src/catalog/registry_catalog.rs:1466:
  - Defect: `kind_read: kind.is_none()` also marks a manifest that names a grimoire kind this grim does not know, so the row is frozen unread. A future kind repeats #277 behind a cache field whose name freezes at 0.16.0.
  - Decided (§ Deferred D1), option (a): `kind_read: kind.is_none() && !manifest_names_a_grimoire_kind(&m)`. The helper is true for a `com.grimoire.kind` annotation, or for an `application/vnd.grimoire.` artifact or config type.
  - Test: a `com.grimoire.kind=foo` row stays unmarked and is read again on the next refresh. [s8 Deferred, t5]
- **N-H5** src/install/json_splice.rs:1735 (also :1738; cause `cut_range`, :1163-1170):
  - Defect: a line comment that ends in `,` and sits before the last or only item makes the backward comma walk take the comment's own `,`. The cut deletes a byte of the user's comment, and when the closing bracket shares the removed item's line, **the file stops parsing**. This hits `grim uninstall mcp` on `opencode.jsonc` and every other caller.
  - Evidence: s9 checked 805,920 generated cases, and all 3,296 failures had this one shape. The orchestrator confirmed the walk by reading it.
  - Provenance: the behaviour predates 187c447, but #275 is closed and C-005/C-006/S-003 promise "the result parses".
  - Remedy: take only `cut_item`'s comment-aware comma (`trivia_end`) as the separator, never a byte-level search. Add regression rows for a same-line and an own-line closing bracket, and assert that the comment's text is unchanged. [s9]

**Warn**
- **N-W1** src/install/vendor_warp.rs:252 (rationale :44-49; subsystem-client-detection.md:35):
  - Defect: Warp on Linux is detected only by `~/.warp`, while its settings live in XDG `warp-terminal`.
  - Decided (D3): OR `warp_config_root(xdg)` into Global detection only when `settings_root_kind() == Xdg`. Rewrite the rationale and add an upgrading.md#autodetect row. [s2 Deferred, t5, t6]
- **N-W12** src/install/installer.rs:1731 with src/command/update.rs:156-162:
  - Defect: `grim update --client copilot` with copilot dropped from `[options].clients` and a pin roll. Copilot is in `materialize_set`, so `reaped_adopters` is never consulted. The adopted member is rewritten, then reaped in the same run without `--force`, which contradicts C-013.
  - Decided (D4): decide the adopted carry-forward by `[options].clients` membership, before the materialize-set skip. [s5, s6 Deferred]
- **N-W13** src/install/installer.rs:283-298 (`install_all_with_progress` passes `configured: None`; test case `(None, true)` at :6799):
  - Defect: `grim install`/`add`/the TUI roll a dropped client's adopted member forward and clear `adopted`. The next `grim update` then reaps it without `--force`.
  - Decided (D5): pass the configured set on every pass. [s5 Deferred; codex High, triaged Warn, see § Cross-Model]
- **N-W14** src/install/installer.rs:1740-1747 (new comment :1712): the comment says per client, the drift check is per output. A two-output Copilot with one edited file refuses `RefusedUntracked` on the intact one. [s6]
- **N-W15** src/install/installer.rs:3102 (with :426-429, :3190-3192): `remove_stale_entries` adds its adopted entries to `abandoned` before the writes. A later failed write (I/O, the Cline lock) carries entries that were never left. [s6]
- **N-W16** src/install/installer.rs:3302-3304: a drifted or unreadable stale member reaches `continue` silently, with no warning and no `abandoned` item, yet its output leaves the record. The behaviour predates the commit, but the commit rewrote this function. [s5 Suggest, s6 Warn; max-wins]
- **N-W11** src/command/update.rs:482-503: `attach_abandoned` appends without sorting or dedup, which breaks json-interface.md:97 ("sorted and deduplicated") and `ReapedClients` (prune.rs:409-410). [s5 Warn, s6 Suggest at :489-492]
- **N-W26** docs/src/content/docs/json-interface.md:97 (also commands.md:700-704): `update`'s `abandoned_entries` now also names a stale adopted member of a still-selected client (`reaped_clients: []`). The text implies "abandoned implies reaped". [s5]
- **N-W42** AGENTS.md:111, docs/src/content/docs/configuration.md:1366 and :1372: fa72cc8 edits files outside P-270's Expected Files, and no decision extends the set. [s4]
- **N-W27** docs/src/content/docs/stability.md:615-617 and upgrading.md:310-311: "byte-identical to what 0.13 wrote" is false for an OCI row carrying `kind_read: true`. [s8]
- **N-W28** docs/src/content/docs/upgrading.md:284: the heading names only 0.13 and an index browse (Q9). [s8]
- **N-W30** docs/src/content/docs/settings.md:776-789: the Droid first-run-defaults limit is missing (Droid creates `~/.factory/settings.json` with defaults on first run). Add a watchlist re-check row; the contents of those defaults are unverified (t6). [s1, t6]
- **N-W31** docs/src/content/docs/settings.md:776-789 (ADR:1091): the Antigravity `/agentMode` limit is missing ("`auto` unavailable to business accounts; the CLI falls back"). [s1]
- **N-W35** .agents/research/research_upstream_kilo_settings_20261105.md:66-82: the Routing section has no variable-expansion verdict (watchlist :307 relies on it). [s1]
- **N-W36** .agents/research/research_upstream_appendix_b_settings_20261105.md:282 (also :236-237; ADR:1148): Copilot `trustedFolders` has no routing verdict. [s3]
- **N-W37** .claude/skills/upstream-refresh/references/domains.md:83-84 (also researcher-brief.md:28-40): PD-41 (Gemini folder trust off) is missing. [s3]
- **N-W38** .agents/upstream-watchlist.md:256-320: the OpenCode limits (expansion tokens; the `config.json` third file) have no dated rows. [s3]
- **N-W39** .agents/adr/adr_settings_artifact_kind.md:1125-1126: Cursor `autoAcceptWebSearch` is missing from Appendix B. [s3]
- **N-W33** catalog/README.md:63-65, catalog/publish.toml:12-13 (same claim in .github/workflows/publish-catalog.yml:11-13, whose :9 contradicts C-026): "a manual run re-pushes existing tags (a no-op, or a loud failure)" is false. Publish skips every existing exact-version tag (publish.rs:1188, release.rs:280-291). [s11]
- **N-W34** test/tests/test_catalog_drift.py:132-134 with catalog/skills/grim-usage/SKILL.md:155-157 (premise from .claude/rules/subsystem-cli-commands.md:25, stale on main): "`grim install` takes no positional" is false. Its dev-install path is refused for registry refs only (install.rs:33-40, :170-173). The test blocks restoring the true statement. [s11]
- **N-W40** .claude/rules.md:98-99 and vendor-capability-watchlist.md:96-101: "no two rules share a part's glob string" is false since 5c267f9. The `vendor_*.rs` pair is declared at rules.md:165. [s11]

#### Test coverage (Specify-phase adequacy)
Seats s4, s6 and s10, plus the coverage half of s9 and s11.

**High**
- **N-H2** src/install/install_state.rs:5903-5928: `every_listed_key_survives_load` lists the first six clients only (Q10b). Derive it from the registry, every client with a non-empty `settings_fields()` resolved through `from_target`. That test would have caught N-B1. [s4]
- **N-H3** test/src/helpers.py:283-324, no file under test/tests/: S-010 has **no end-to-end test for any of the seven new harnesses**: no install, reload, status and uninstall. Amp and Gemini share their file with grim's MCP entry. Add one test parametrised over every settings host. [s4 High and the merged Warn]

**Warn**
- **N-W3** src/oci/settings.rs:49-53 (tests: vendor_gemini.rs:868-893): three single-token mutations of `holds_dollar_reference` survive every test. [s4]
- **N-W2** src/install/vendor.rs:2285-2304: `no_settings_row_reaches_a_key_another_kind_owns` matches only the first `/` segment against `DENIED`. A flat dotted pointer such as Amp's `amp.mcpServers` (vendor_amp.rs:223-225), in the same file as grim's MCP entry, passes the guard. No shipped row crosses today. [s4, s12]
- **N-W18** src/install/installer.rs:1719: no test covers a non-adopted output of a client outside `configured`, so a mutant survives. [s6]
- **N-W19** src/command/update.rs:196: the `&[]` flag argument feeds both the configured set and the reap's desired set. No acceptance test runs `update --client X` with a non-empty `[options].clients`. [s6]
- **N-W20** src/command/update.rs:1061-1090: `an_abandoned_mcp_entry_lands_on_its_own_row` cannot tell kinds apart; `row_kind == kind` → `true` survives. [s6]
- **N-W21** src/install/prune.rs:1935 (S-002): the "`grim status` reports it modified" clause and the CLI-level `reaped_clients`/`kept_modified_clients` clause have no test. [s5 Suggest, s6 Warn]
- **N-W22** test/tests/test_settings_lifecycle.py:578: no test fails if the settings **uninstall** warning (settings_phase.rs:324) reverts to the hook wording (C-019). [s7]
- **N-W23** S-003 (plan:191, src/install/uninstall.rs:461): no acceptance test. Add one to test_fix_splice_status.py with the N-H5 shape. [s9]
- **N-W24** test/tests/test_stdio_drivers.py:53-67: the "close stdin once every id is answered" half of C-001 has no test. Two single-token mutants make every MCP call wait out the 60 s timeout. [s10]
- **N-W25** test/tests/test_catalog_drift.py:70-71: `test_documented_config_values_are_fully_qualified` checks a phrase and cannot fail on parser drift (C-025). [s11]

### Stage 2 — Adversarial panel

#### Quality (user-feedback slot)
**Warn**
- **N-W9** src/oci/settings.rs:889 (hint path src/error.rs:385-394, src/main.rs:224): a settings unknown-key refusal skips `unknown_key_hint`, so there is no `error.hint` and no "upgrade grim" line. That holds at consume time too, under `unsupported-artifact-feature`, against stability.md:554-555's promise. A bare Kiro dotted key prints `kiro has no setting at /chat` with no hint to quote it. [t1]
- **N-W10** src/install/hook_registrar.rs:240-263 (used at settings_phase.rs:324, :726, status.rs:1426): the three `GRIM_HOME` settings messages from #276 drop the remedy tail that the hook text carries for the same cause. [t1]
- **N-W8** src/install/settings_phase.rs:1756-1759, src/api/artifact_status.rs:356-358: `no-applicable-settings` names neither the uncovered harness nor how to select it. For an `agy`-only or XDG-only Warp user, the note names `gemini` or `agents`. This widens call-1's cause-without-remedy Warn (status.rs:1483-1490). [t1]
- **N-W17** src/install/installer.rs:3306-3309 (vocabulary drift with prune.rs:603-607): the #274 stale warning offers "`--force`" as a later remedy. The same run drops the output from the record, so a later `--force` does nothing. [t1]
- **N-W6** src/install/settings_phase.rs:106-119, :230-245:
  - Defect: a non-interactive project run with the flag on **silently** skips a package with no consent: no warning, no row, `{"items": [], "settings_changes": []}`. The hook path warns and records `skipped`. This is branch-wide; call 1 missed it.
  - Remedy: warn once per filtered package with `policy.refusal_reason(..)`. [t1]
- **N-W7** src/install/settings_phase.rs:1764-1767 (:1019): the first write after a `skipped` record reports `updated`, not `installed`. With the detection gaps, this is the normal first-write path for Warp and Antigravity. [t1]

#### Security
No Block, High or Warn in scope.
- All seven properties hold (t2): expansion refusals in both modes, every map, opaque key and array leaf; no row that runs publisher code; `MARKETPLACE_ROWS`; fixed root segments; the splice differential; consent coverage; supply chain.
- f3c0fb7 tools: tag-declared in `ocx.toml`, with a sha256 per platform in `ocx.lock`.
- Two items predate the range:
  - Suggest: URL userinfo in non-canonical spellings.
  - Out of scope O4: env-relative vendor roots.

#### Performance
No finding (t3). Measured or derived:
- `kind_read` refetch: once per legacy kindless row, in the existing 16-worker pool.
- Row tables: `&'static`.
- Per-piece lookup: O(rows of one vendor).
- `cut_item`: scans the local trivia span only.
- The stdio driver is event-driven.

#### Documentation
**Block**
- **N-B3** docs/src/content/docs/json-interface.md:506: the `describe` `requires` field reads "sorted by kind (skill, rule, agent, bundle, mcp, hook) then name". `requires_decl.rs:61-83` accepts `[requires.settings]` and sorts it after `hook` (`ArtifactKind::ALL`, artifact_kind.rs:77-85). Call 1 missed this sibling of its :563 Block (Q2). [t4]

**High**
- **N-H6** docs/src/content/docs/upgrading.md:301-303: "Only if you downgrade to 0.13 after browsing a package index" is false. A downgrade to 0.13 or 0.14 after a direct OCI browse now also meets the refusal. [s8]

**Warn**
- **N-W29** docs/src/content/docs/json-interface.md:657-663 (also hooks-troubleshooting.md:153-163): the three `grim-home-*` Meaning cells describe the dispatch table only, though settings.md:872-874 sends settings readers here and their `message` differs since 251524e (Q8). [s12, t4]
- **N-W32** docs/src/content/docs/settings.md:337 and the Limits list (:800-835): OpenCode's `config.json` layer, merged beneath `opencode.json[c]`, is not named. grim's write outranks a user's value there with no ownership refusal (Q7). [t6]

#### Architecture
**Warn**
- **N-W4** src/install/install_state.rs:1592-1606, :1636-1665:
  - Defect: a Warp settings output in a carried `global.json` is **dropped, not withheld**, on a host where Warp has no settings file. The ADR:460 rule exists to prevent exactly that (a third case).
  - Remedy:
    - generalise `is_in_another_runs_file` to settings anchors whose file this host does not resolve;
    - amend ADR:460 to three cases;
    - add a unit test.
  - Order: lands with N-B1, whose fix routes macOS to Linux carries into this path. [t5]
- **N-W5** src/command/status.rs:331-333 (`client_drift`, :1927-1945): a settings row keeps a capable host the package declares nothing for in `clients_missing` forever, because install never records a piece for it. Decided (D12): settings rows report `clients_missing: []` as bundle rows do, and commands.md:821-823 gains a sentence. [t5 Deferred]
- **N-W41** .claude/rules/subsystem-client-detection.md:1-5 with subsystem-vendor-paths.md:13: every `detect()` lives in `src/install/vendor_*.rs`, which never loads the detection part. That is how fa72cc8 added two settings roots without touching detection. Add one pointer line in subsystem-vendor-paths.md. [t5]

#### SOTA / technical soundness
Researcher (t6), on the techniques:
- **JSONC member cut** (`cut_item`): current. It covers the trailing-comma failure mode, and CRLF is safe.
- **Stdio driver**: sound. stderr goes to `DEVNULL`, a reader thread drains stdout, then the shutdown sequence is close, wait, then kill.
- **`kind_read` bool**: the right marker for an append-only cache once N-H4 lands.

Expansion syntaxes:
- **Gemini**: the refusal is a superset of upstream's `envVarResolver.ts` regex, which resolves values only.
- **Kilo**: the `{env:`/`{file:` refusal holds whatever the `~` answer is.
- **Amp, Droid, Kiro, Warp, Antigravity**: none documents interpolation in settings strings.

Findings in this section: Warns merged into N-H1, N-W1 and N-W32; Suggests below.

#### Suggest (63, compact; actionable unless marked)

**s1**
- research_upstream_other_harnesses_settings_20261105.md:125-128: Amp has no expansion verdict.

**s2**
- vendor_gemini.rs:868-892 and vendor_kilo.rs:620-633: the refusals are tested in author mode only. Use `assert_refused_alike` (s4 duplicate merged).
- settings.md:830-831: the Kiro shadowing line is wrong for global-only keys.
- settings.md:783-784: the Kiro example uses a key class grim never delivers.
- catalog/skills/grim-authoring/SKILL.md:123-128 and grim-usage troubleshooting.md "Integrity Gates": they lack the quoted dotted Kiro key and the Kilo and Gemini refusals.

**s3**
- vendor_claude.rs:267-269: `allowRead` wording.
- vendor_claude.rs:1151-1190: the PD-39 test is not derived from the rows and has no forced case.
- settings.md:531: `/taskbarPresence` is Windows only.

**s4**
- test_settings_publish.py:204-234: no Kilo or Gemini case.
- vendor.rs:2230-2270: the docs parity test compares only (vendor, pointer), now over 316 rows.

**s5**
- upgrading.md:521-522: the stale removal also runs on `install` and `add`.

**s6**
- installer.rs:3305, :2402, prune.rs:592, uninstall.rs:262: PD-28 lives at four sites. Extract a keep-adopted helper.
- installer.rs:313: `install_all_before_reap` naming.
- test_reap_adopted.py:147-163: three assertion gaps.
- prune.rs:582 vs :652: refcount keyed on client names, retain keyed on outputs.

**s7**
- hook_consent.rs:336: the call site is unpinned.
- hook_consent.rs:344-350: `noun_for` duplicates `prompt_noun` (:262-266).
- status.rs:891-893: the doc comment omits the global flag-off arm.
- status.rs:3572: the test helper builds the hook text.
- plan:214, :301: file-set bookkeeping, and the PD-8 premise.

**s8**
- commands.md:1499-1500: the "downloads a manifest again only when it changed" claim.
- registry_catalog.rs:3487-3489: comment, and the 0.14 refusal is unpinned.
- plan:119: the C-012 text.
- plan:212: P-277's Expected Files.

**s9**
- json_splice.rs:2930: the nested test never compares the remaining value.
- (Deferred, D8) json_splice.rs:1738: a comment on the removed member's line. Decided: keep C-005; the comment survives.

**s10**
- stdio_drivers.py:186-199, :219: early-exit and kill paths are untested.
- stdio_drivers.py:167-201: `Popen` is not used as a context manager (also t6).
- registries.yaml:79: the kind list is pinned as a prefix only.
- test/taskfile.yml:124-133: `CLI_ARGS` is undocumented.
- stdio_drivers.py:5-6: stale "moved verbatim" docstring.
- plan:210: P-283's Expected Files.
- (Deferred, D9) test_telemetry_contract.py:146-162. Decided: keep `drive` and correct C-002's premise text.

**s11**
- post_tool_use_tracker.py:31-44: `CONTEXT_REMINDERS` is untested, and the remaining parts have no rows.
- test_ai_config.py:641-656: no anchor-fragment or part-budget test.
- vendor-capability-watchlist.md:24-26: "load on every launch" wording.
- test_catalog_drift.py:20, :22, :45-50: consume.md is unpinned, and `_CONFIG_ERROR` is misnamed (65 is `DataError`).
- test_catalog_drift.py:99-107: alias expansion is unproven.
- release.taskfile.yml:61: the `sed` can clobber a per-entry `version`.

**s12**
- consume.md:116-118: implies an MCP dev-install.
- clients.md:593-598: Warp auto-spawn is now a settings row.
- subsystem-vendor-paths.md:102, :77-80: the `KIRO_HOME` and XDG rows are not updated.
- research_upstream_other_harnesses_settings_20261105.md:127: `$AMP_SETTINGS_FILE` is contested (watchlist :82).
- subsystem-install-layout-settings.md:2-3: needs a pointer from vendor-paths.
- plan:342-353: the overlap table lacks P-270 and P-277 rows.
- index_announce.rs:44 (Q1): stale kind comment.

**t1**
- trust.rs:380-385: the record failure names no path and no next step.
- status.rs:914-919 vs policy.rs:210-214: two flag-off remedy forms in one table.
- settings.rs:925: the expansion-token message says neither why nor what to do.
- settings.rs:896: "unknown vendor table zed" for a known client with no settings.
- build.rs:677-678: "invalid skill name" for a settings stem (rules inherit it from main).

**t2**
- settings.rs:426-435: `holds_userinfo` misses WHATWG-normalised spellings (CWE-522, CWE-20). Parse with `reqwest::Url` and refuse a non-empty username or password.

**t4**
- clients.md:267: Warp declines on every platform but macOS and Linux.
- settings.md:167: OpenCode and Kilo keys are refused too, not only values.
- troubleshooting.md:402-404, consume.md:707-709: Warp settings work on macOS and Linux only.

**t5**
- installer.rs:776: `#[allow(clippy::too_many_arguments)]` → `#[expect(.., reason)]`.
- ADR:874: the Principle 9 matrix lacks the two new anchors.
- subsystem-file-structure.md:58-80: § Catalog Cache lacks `kind_read`.
- path_anchor.rs:3322-3325 (also hook_dispatch.rs:131, hook_registrar.rs:143, :1834): pointers to the moved anchor table.
- subsystem-install-layout-agents-mcp.md:26-31: cross-pointers between parts.

**t6** (Deferred, decided)
- Droid's default contents (D13, covered by N-W30).
- Kilo project-scope refusal stays (no action).
- Gemini refusal wider than upstream (no action).

### Convergence
Appended as rows G-275, G-274, G-270, G-282 and G-270R at the end of the plan's Parallelization table, with matching Implementation Steps:
- C-005 — **contradicts**: a line comment ending in `,` before the last or only member gives an unparseable result (N-H5).
- C-006 — **contradicts**: same cut, for array elements and nested handlers (N-H5).
- S-003 — **contradicts**: the comma-ending comment variant breaks the file; no acceptance evidence exists (N-H5, N-W23).
- C-013 — **contradicts**: `update --client X` for a client `[options].clients` drops rewrites, then reaps, the adopted entry in one run (N-W12).
- S-002 — **partial**: no evidence for the "`grim status` reports it modified" clause or for the CLI-level lists (N-W21).
- C-035 — **partial**: the host lists were not carried to `every_listed_key_survives_load` (N-H2).
- S-010 — **contradicts** for Kiro (`grim status` cannot report it, N-B1), **partial** for the other six new harnesses (no end-to-end evidence, N-H3).
- C-024 — **partial**: `grim install <path>` (dev-install) is dropped from the path-accepting commands (N-W34).
- C-025 — **partial**: the short and qualified forms are never parsed in tables, and the argument-only reading has no PD row (N-W25).
- C-033 — **partial**: no recorded reasons for Copilot `trustedFolders` and Cursor `autoAcceptWebSearch` (N-W36, N-W39).

### Fold-Back
Not performed. The verdict is not Approve, the convergence check is unconverged, and the plan carries no `## Spec Deltas` block.

### Cross-Model Adversarial
- **Leg:** ran, degraded. codex 0.166.0, model `gpt-6.1-sol`, `-s read-only --ephemeral`, from a throwaway detached worktree at b58d602 with these removed: `AGENTS.md`, `CLAUDE.md`, `.claude/`, `.github/`, `.mcp.json`, `.agents/{skills,memory,retro,goals}`. nox-review refuses codex 0.166.0, so codex was driven directly.
- **Attempt 1:** no review. The command sandbox did not start (`bwrap: loopback: Failed RTM_NEWADDR`), and codex declared every property unverified.
- **Attempt 2:** the same prompt (properties P1–P8, call-1 and filed issues excluded) with the e73a58c..HEAD diff of `src test docs/src catalog taskfiles` embedded (448 KB, the schema golden excluded) and no shell. Exit 0.
- **Returned:** 1 finding, P5 "counterexample", and P1–P4 and P6–P8 "not found". Those "not found" lines rest on the delta hunks alone and are weak evidence.
- **Triage of [High] src/install/installer.rs:6799.** The claim: "a pin change rewrites an adopted MCP entry without `--force` when its client remains configured, or no reap follows".
  - The `(None, true)` arm, no reap follows: **duplicate** of s5's Deferred two-command loss, now N-W13 and actionable. **Escalation noted:** codex graded High. The severity is held at Warn because C-013 is scoped to one `update` and the entry content equalled grim's own value at adoption. D5 makes the fix actionable either way.
  - The `(Some(both), true)` arm, client still configured: **stated convention**. C-017 keeps today's roll-forward for a configured client; adoption hands the member to grim. Dropped (1).
  - Trivia: 0. New actionable: 0. New deferred: 0.

### Root-Cause Analysis
Every Block, High and Warn above sits in at least one cluster. The clusters are listed from widest to narrowest.

**R1. Per-harness invariants are checked against hand-kept lists.**
- **Issue:** fa72cc8 added 7 hosts. The places that enumerate hosts or kinds by hand did not follow: a file-name filter, a test host list, a key denylist, a doc sort list. Kiro's file one directory below its anchor broke the first silently.
- **Why 1:** Kiro outputs are dropped at load, because `is_settings_file` compares file names.
- **Why 2:** the filter was written when every host's settings file sat at its anchor root.
- **Why 3:** the test that would catch it, `every_listed_key_survives_load`, names the six hosts it was written for, and no end-to-end test installs and reloads a new harness.
- **Why 4:** C-035's checklist names the lists P-270 must extend, but not the lists that derive per-host behaviour in install_state.rs.
- **Why 5:** no forcing function ties a per-host or per-kind list to the registry (`ClientTarget::ALL` with non-empty `settings_fields()`, or `ArtifactKind::ALL`). A new host compiles and passes while the lists stay short.
- **Systemic fix:** derive every per-host test list and guard from the registry. Add one end-to-end test parametrised over every settings host: build, install, reload, status, uninstall. Add a docs parity test asserting that every kind list in json-interface.md names `ArtifactKind::ALL`. Check each segment of a flat dotted pointer against the host's own MCP and hook keys.
- **Related:** N-B1, N-B3, N-H2, N-H3, N-W2, N-W3.

**R2. Tests prove presence, not the contract.**
- **Issue:** contracts with a CLI-observable clause (S-002, S-003, S-010, C-001, C-019, C-025) were delivered with unit tests, or with tests that cannot fail on the defect they name.
- **Why 1:** single-token mutants survive (N-W3, N-W18–N-W20, N-W24), and clauses have no test (N-W21–N-W23, N-W25).
- **Why 2:** plan steps scheduled unit tests and named the existing acceptance modules as the gate.
- **Why 3:** a gate of existing modules passes by construction for new behaviour.
- **Why 4:** the plan template asks "which test owns this contract" but not "which edit makes it red".
- **Why 5:** no mutation spot-check runs before a step's box is ticked.
- **Systemic fix:** every S-ID with a CLI clause names its acceptance test in the plan step. A step's report shows one planted mutation going red per contract; s9's and s6's harnesses are the model.
- **Related:** N-H3, N-W3, N-W18, N-W19, N-W20, N-W21, N-W22, N-W23, N-W24, N-W25.

**R3. Docs and records restate code facts outside the pipeline that changes them.**
- **Issue:** user pages and decision records state facts (hosts served, kind lists, cache compatibility, publish behaviour, rule overlap) that the delta changed from a different pipeline's file set.
- **Why 1:** the claims are now false (N-B2, N-B3, N-H6, N-W26–N-W29, N-W33, N-W34, N-W40), or the limits were never carried over (N-W30–N-W32, N-W35–N-W39).
- **Why 2:** each pipeline's Expected Files list names the files it meant to edit, not the files stating the facts it changes.
- **Why 3:** research artifacts list per-row limits, and nothing copies them into settings.md's Limits list and ADR Appendix B.
- **Why 4:** the doc-drift checks compare (vendor, pointer) only, and the parity tests never parse prose claims.
- **Why 5:** a fact has no single source; docs restate it by hand.
- **Systemic fix:** when a pipeline changes a fact, grep the docs and records for its statements (kind lists, host names, cache claims) and add those files to the pipeline's set. Generate Limits rows from the research Routing lines, or add a parity check between the two.
- **Related:** N-B2, N-B3, N-B4 (doc half), N-H6, N-W26, N-W27, N-W28, N-W29, N-W30, N-W31, N-W32, N-W33, N-W34, N-W35, N-W36, N-W37, N-W38, N-W39, N-W40, N-W41, N-W42.

**R4. "Is the client present", "can it host settings here" and "which file" are answered by three functions.**
- **Issue:** fa72cc8 taught `settings_config_path` two new roots, `antigravity-cli` and `warp-config`, while `detect`, `client_supports_kind`, the load filter, `clients_missing` and the selection messages kept their old answers.
- **Why 1:** CLI-only Antigravity and XDG-only Warp homes are not detected (N-H1, N-W1). `clients_missing` is wrong (N-B4, N-W5). A carried Warp record is dropped (N-W4). The first write after `skipped` reads `updated` (N-W7). The skip note names another client (N-W8).
- **Why 2:** `detect`, `client_supports_kind` and `settings_config_path` each re-derive the client's settings surface.
- **Why 3:** the detection rule does not load when `vendor_*.rs` is edited (N-W41), so the recorded "deliberately not consulted" rationale went unchallenged.
- **Why 4:** the settings kind arrived after detection was designed for skills and MCP.
- **Why 5:** no single per-vendor "settings surface here" answer exists to consume.
- **Systemic fix:** one `settings_file_here(client, workspace, scope)`, consumed by `client_supports_kind`, the load filter, `status` and the selection messages. Widen detection by OR as decided. Add a pointer from the vendor-paths part to the detection part.
- **Related:** N-B4, N-H1, N-W1, N-W4, N-W5, N-W7, N-W8, N-W41.

**R5. #274's adopted-entry protection is per command, not per entry.**
- **Issue:** "never remove an adopted entry without `--force`" holds on the `update` reap and the stale removal of one pass. It does not hold across `--client`, across `install` followed by `update`, or in what the warnings and JSON report.
- **Why 1:** an adopted entry is rewritten and then reaped in one run (N-W12) or across two commands (N-W13). The `abandoned` lists are unsorted, or carry entries a failed write never left (N-W11, N-W15). Drifted members disappear silently (N-W16). The warnings promise a later `--force` that cannot act (N-W17). The docs describe one path only (N-W26).
- **Why 2:** the configured client set is threaded only into `update`'s pass, behind `--client`'s override of `materialize_set`.
- **Why 3:** C-013 was scoped to "the same update", so other passes stayed on the old roll-forward (C-017).
- **Why 4:** PD-28's keep-adopted rule is spelt at four sites with three vocabularies (s6 Suggest).
- **Why 5:** adoption is a flag on an output that a rewrite clears, not a property the record carries until `--force` on that run removes it.
- **Systemic fix:**
  - Decide carry-forward from `[options].clients` on every pass.
  - Use one keep-adopted helper with one warning text and one sorted, deduped `abandoned` sink, extended only after its write.
  - Scope `--force` remedies in text to "this run".
- **Related:** N-W11, N-W12, N-W13, N-W14, N-W15, N-W16, N-W17, N-W21, N-W26.

**R6. Comma handling by byte walk in a comment-aware splice.**
- **Issue:** #275 fixed the two cases where a comment hides the separator, but `cut_range`'s backward walk still reads raw bytes and takes a comment's trailing `,`.
- **Why 1:** the file stops parsing, or a byte of a comment is lost (N-H5).
- **Why 2:** the fix added branches in `cut_item` instead of moving every separator decision onto `Scanner` positions.
- **Why 3:** C-005's tests enumerate five separator shapes, and the comma-ending comment is not among them.
- **Why 4:** no property test sweeps comment shapes against positions.
- **Why 5:** S-003's acceptance test was not written (N-W23), so nothing outside the unit suite exercised the shape.
- **Systemic fix:** take every separator from the tokenizer. Fold s9's generated sweep (805,920 cases, three checks) into the unit suite as a bounded property test.
- **Related:** N-H5, N-W23.

**R7. A cache marker records "read" without the knowledge it was read with.**
- **Issue:** `kind_read` marks a row "read, no kind" even when the kind was present but unknown to this grim.
- **Why 1:** a future kind would be frozen unread (N-H4).
- **Why 2:** `kind.is_none()` merges "foreign manifest" with "grimoire kind I cannot parse".
- **Why 3:** PD-7 reasoned about kindless foreign rows only.
- **Why 4:** #277's own cause (a kind added after the cache was written) was not replayed against the fix.
- **Why 5:** append-only cache fields carry no reader-knowledge contract in subsystem-file-structure.md.
- **Systemic fix:** option (a). Document the marker's meaning in § Catalog Cache (t5 Suggest).
- **Related:** N-H4, N-W27.

**R8. Settings messages took the cause half of the hook text, not the remedy half.**
- **Issue:** #276 and the settings path built settings wording beside the shared hint, remedy and not-armed seams instead of through them.
- **Why 1:** no upgrade or quoting hint (N-W9), no `GRIM_HOME` remedy (N-W10), a silent consent skip (N-W6), and no selection remedy (N-W8).
- **Why 2:** `settings_reason` re-spells the first half of `HookArmingCause::message`, and `unknown_key_hint` matches serde's wording only.
- **Why 3:** C-019 pinned the cause sentence but not the remedy tail.
- **Why 4:** `not_armed_message(Settings, NoTtyToAsk)` exists but install has no caller for it.
- **Why 5:** no test asserts "every settings warning and status message names a next step".
- **Systemic fix:** derive settings text from the same cause → (reason, remedy) table as hooks. Add a test that every settings refusal, warning and status message ends in a remedy or names its hint.
- **Related:** N-W6, N-W8, N-W9, N-W10, N-W17, N-W29.

Systemic causes affecting at least 3 findings: R1, R2, R3, R4, R5 and R8. That alone makes the verdict Request Changes at this tier.

### Deferred Findings
The orchestrator resolved each one; no human was prompted. Each entry gives the question, the research, and the decision.

- **D1, `kind_read` and an unknown kind** (s8, registry_catalog.rs:1466).
  - Options: (a) skip the marker for a manifest that names a grimoire kind; (b) a kind-set generation; (c) accept and document.
  - Research (t5): every grim push stamps `com.grimoire.kind` (annotations.rs:424…870), a frozen wire field. (b) changes the field type after 0.15 wrote a bool and needs a constant someone must remember to bump.
  - **Decision: (a), Actionable High (N-H4)**, before 0.16.0 freezes the name.
- **D2, Antigravity detection** (s1 Q6a).
  - Research: rule :43-44 says detection writes nothing, so OR is safe. Upstream says the CLI reads `~/.gemini/config/skills/`, so the other kinds land where a CLI user reads them. The Gemini detect is unaffected.
  - **Decision: widen, Actionable High (N-H1)**, with an upgrading.md#autodetect row (additive, Principle 9).
- **D3, Warp detection on Linux** (s2 Q6b).
  - Research: the rule's rationale ("app-data and log directories") is false on Linux (docs.warp.dev, file-locations). Upstream does not say whether `~/.warp` is created on first run.
  - **Decision: widen on Linux only (`settings_root_kind() == Xdg`), Actionable Warn (N-W1).**
- **D4, `--client` and a client `[options].clients` drops** (s5, s6).
  - **Decision: Actionable Warn (N-W12).** The adopted carry-forward follows `[options].clients` membership even when `--client` names the client. That restores C-013's "never".
- **D5, the two-command loss via `install`** (s5; codex).
  - **Decision: Actionable Warn (N-W13).** `install`, `add` and the TUI read the same `[options].clients` and pass it as `configured`. upgrading.md:521-522 says so.
- **D6, an unparseable dropped-client file treated as absent** (s6, install_state.rs:203-206, prune.rs:492).
  - Predates the branch. **Decision: out of scope (O2).**
- **D7, install-pass parameter threading** (s6, 12 parameters).
  - **Decision (t5):** a separate `refactor(install):` branch introducing an `InstallPass` context after this feature lands (Two Hats rule; the fix pass owns these lines). Filed as O5. Now: `#[allow]` → `#[expect]` (Suggest).
- **D8, a comment on the removed member's line** (s9).
  - **Decision: keep C-005.** Comments outside the member's span survive. No change.
- **D9, the telemetry test keeps close-at-once `drive`** (s10).
  - **Decision:** keep it, since C-002 stands and `grim_status` without `check` is a local read. Correct C-002's premise text ("acts per request, race accepted") in the fix pass.
- **D10, settings announce versus the released indexer** (s12).
  - Belongs to another repository (#271/F-4). **Decision: out of scope (O6)**, for the meta-orchestrator.
- **D11, env-relative global vendor roots** (t2, CWE-59/CWE-73, T3 through repository env plus `grim --global`).
  - Research: the same helper backs MCP and agents on main, and ignoring a set value is a layout move that needs a `relocated_vendor_roots` decision.
  - **Decision: out of scope (O4)**, file a security issue. Not a decided trade under arch-threat-model.md; it needs its own decision.
- **D12, `clients_missing` for an undeclared but capable host** (t5).
  - **Decision: report `[]` for settings rows, Actionable Warn (N-W5)**, matching ADR:569's `outputs_pending` decision.
- **D13, the contents of Droid's first-run defaults** (t6).
  - A harness binary may not be run. **Decision:** document the limit and add a watchlist re-check row (N-W30). No code change.

### Out of scope (for the meta-orchestrator to file)
None of these is introduced or widened by the branch, except where the entry says so. Do not file anything already in #284–#295.
- **O1** src/install/installer.rs:3220-3227: `install_mcp` discards `reap_relocated_roots`'s `(preserved, abandoned)`, against its own doc (:2275-2285). Predates the branch. (s6)
- **O2** src/install/install_state.rs:203-206 → src/install/prune.rs:492, :566-570: a dropped client's config file that is unreadable or unparseable reads as absent, so the output is dropped from the record. Predates the branch. (s6)
- **O3** src/fetch.rs:90-98 (callers :332, :802, :1037): a resolve miss is labelled "skill" for every kind, and the message shows the reserved `__grimoire` tag (Q4). On main since eef13d6c. Message-only fix:
  - a kind-neutral `ResolveError` constructor;
  - "no description is published for <repo>" on a companion miss. (t1)
- **O4** src/install/vendor.rs:1452-1458 (`env_dir`, `xdg_config_dir`; anchor twin path_anchor.rs:188-190):
  - Defect: a relative or workspace-internal `KIRO_HOME`, `GEMINI_CLI_HOME` or `XDG_CONFIG_HOME` makes a global write relative to the current directory. A repository's own environment can then aim the write through a link it planted (CWE-59, CWE-73, CWE-436; attacker T3).
  - Provenance: predates the branch for MCP and agents; the branch moves the settings rows of Kiro, Gemini, Kilo and Warp onto it.
  - Needs a decision on the `expand_home` rule and a layout move. (t2)
- **O5** Install-pass parameter threading (installer.rs:235, :271, :609, :776): an `InstallPass` context struct, on a separate `refactor(install):` branch after this feature. (s6, t5)
- **O6** External: grimoire-rs/indexer 0.6.3 (main) `KINDS` lacks `settings`. `grim publish --announce` of a settings entry opens an index PR that fails validation while grim reports it done. The fix is on indexer branch `hex/settings-kind` ([#271](https://github.com/grimoire-rs/grimoire/issues/271), F-4). Release order: the indexer first, or package-index.md:288 names the indexer version. (s12)

### Handoff
- Scope: large (one-way door, high)
- Tier: xhigh
- Baseline: main (03393c51)
- Overlays: breadth=adversarial, rca=on, adversary=on
- Next: `/hex-execute .agents/plans/plan_settings_follow_ups.md "apply xhigh-tier review findings"`. The call-1 Blocks and Highs stay with the parent plan's fix pass (cycle 3).
- Upkeep: `hex.md › Pointers` was not touched; the brief forbids edits to `.agents/memory/hex.md`.
- Seat reports, the codex prompts, both codex outputs and the scratch harnesses live under the session scratchpad, `review2/`.
