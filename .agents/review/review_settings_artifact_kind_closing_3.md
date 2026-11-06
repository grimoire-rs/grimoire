## Code Review: d4a3b67..hex/config-artifact-type (hex-review call 3 of 3, tier xhigh, loop mode)

Commit links point at `grimoire-rs/grimoire`. They resolve once the owner pushes the branch.

### Summary
- Verdict: **Request Changes**
- Tier: xhigh (auto). Overlays: breadth=adversarial, rca=on, adversary=on. Loop mode, so the meta-plan gate was announced, not asked.
- Target: the joint fix pass [d4a3b67](https://github.com/grimoire-rs/grimoire/commit/d4a3b67)..HEAD, 22 commits. Landing 1 is [09e4f0d](https://github.com/grimoire-rs/grimoire/commit/09e4f0d)..[087d55b](https://github.com/grimoire-rs/grimoire/commit/087d55b), landing 2 is [0a4ab18](https://github.com/grimoire-rs/grimoire/commit/0a4ab18)..[b1bb58f](https://github.com/grimoire-rs/grimoire/commit/b1bb58f), and [9a951ee](https://github.com/grimoire-rs/grimoire/commit/9a951ee) only records the goal file. Reviewed HEAD: 9a951ee3f76085b0a72ef62e53ece033fbc28768.
- Counts, new at HEAD, after dedup and max-wins:
  - **2 Block**
  - **5 High**
  - **50 Warn** (every Deferred item was resolved here; see § Deferred Findings)
  - **30 Suggest**
- Convergence over calls 1 and 2: 14 of the 17 Blocks and Highs are **closed**, and 3 are **partial** (C1:17, C1:19, N-B2). N-B3 is closed, but the same defect class reappears in another file as a new Block (C3-B2). The table below gives the evidence.
- Integration gate, run by the meta-orchestrator; this call ran none. Landing gate 2 rerun: `task --force verify` exit 0 on tree 028dd41, the same code as HEAD, since 9a951ee changes only the goal file.
  - nextest: 5638 of 5638 passed.
  - AI-config tests: 91 passed.
  - Acceptance: 2826 passed, 87 skipped.

  The gate is green, but no test reaches either fence bypass (C3-H1, C3-H2) or a second install under a refused root (C3-H4).
- Cross-model: **ran, degraded.** codex-cli 0.166.0, `-m gpt-6.1-sol`. The read-only sandbox probe failed again (`bwrap: loopback: Failed RTM_NEWADDR`), so codex reviewed a pasted diff with no shell. It raised 2 Blocks and 3 Warns. After triage: 2 actionable (one held at High, one at Warn), 3 duplicates. Details are in § Cross-Model Adversarial.
- Why Request Changes:
  - 2 new Blocks, both doc Accuracy defects.
  - Two security vulnerabilities in the new fence (C3-H1, C3-H2; CWE-41 and CWE-15, attacker T3).
  - Five systemic causes each reach at least 3 findings (RCA R1–R5).
- Architect: **no boundary violation.** [e94a99e](https://github.com/grimoire-rs/grimoire/commit/e94a99e) removed the only production `install → command` edge. No commit adds an edge from `src/install`, `src/hook` or `src/api` into `src/command` or `src/tui`. ADR Decision 6's three withheld cases and Decision 2 each match the code and have a test that kills their mutant. The ADR text lags PD-28 and PD-32 (C3-W19).
- Security: every global settings write and removal is fenced on the path actually opened (`judge` settings_phase.rs:1737, `write_file` :1925, uninstall :479, reap :380). The fence still has two bypasses: `..` (C3-H1) and a repository-set `HOME` (C3-H2). Neither is a recorded edge in arch-threat-model.md:94-103.
- Performance: no real cost. The fence runs once per phase, at global scope only, in single-digit syscalls per file.

### Call-1 and call-2 Blocks and Highs at HEAD

| Finding | Verdict | Evidence |
|---|---|---|
| C1:12 Block, settings.md:203 (`install` lists the adopted value) | closed | settings.md:212, :214, :228-231 match installer.rs:503-513 (stderr warning) and update.rs:485-513 (`attach_abandoned`). The install report carries no `abandoned` field. |
| C1:13 Block, json-interface.md:563 (`required_by` order) | closed | json-interface.md:564 lists `ArtifactKind::ALL` (artifact_kind.rs:77-85). Removing `settings` fails test_docs_kind_lists.py:65. |
| C1:14 Block, stability.md:692 (registry-sourced kinds) | closed | stability.md:698-701 names hook and settings. The offline path is settings_phase.rs:1107-1146. |
| C1:17 High, narrowed run strands values at the old pin | **partial** | The pin hold works: dropping the `continue` at settings_phase.rs:2072 fails test_settings_lifecycle.py:776, and dropping `warn_left_behind` fails :777. But `left_behind` (:2092-2105) names every unselected client the record holds, whatever value it holds. Its remedy does not converge, and an autodetect client that drops out holds the pin back for good (C3-H3). |
| C1:18 High, dependencies.md bundle claim | closed | Each changed hunk was checked against the code: :35, :39-42, :177-179, :191-194, :22-26 (vendor_agents.rs:59-61, installer.rs:1724-1746). |
| C1:19 High, manual rig global steps | **partial** | Part e now fails closed: `in_rig` (test/manual/README.md:964-972) checks four roots and runs in a subshell with a rig `HOME`. It was simulated in six cases under bash and zsh. Two residuals remain at Warn. Running §7 after §13e writes the real client dirs (C3-W40), and `in_rig` ignores Claude's managed settings (C3-W43). |
| C1:20 High, real-harness platform guard | closed | test_settings_real_harness.py:93-94 skips off Linux, and :140-141 asserts before any spawn. The child env is built from nothing (:111-123) and checked by `_assert_no_credentials` (:126-136). |
| N-B1 Block, Kiro output dropped at each global load | closed | install_state.rs:1844-1847 compares the anchored relative path. Unit tests :5986 and :6010; acceptance test_settings_every_host.py:162, :165. |
| N-B2 Block, clients.md Antigravity | **partial** | The CLI half is fixed: clients.md:484-485, :500-503, and detection at vendor_antigravity.rs:440-442. The rewrite in [dd47d33](https://github.com/grimoire-rs/grimoire/commit/dd47d33) added a false IDE sentence (C3-B1). |
| N-B3 Block, json-interface.md:506 `describe` order | closed, with a sibling open | :506 names settings, and test_docs_kind_lists.py:34-36 reads the order from the binary. The same class survives in `hook-spec.md:473-475` ("a hook sorts last"), a phrasing that test does not match (C3-B2). |
| N-B4 Block, `client_supports_kind` ignores scope and platform | closed | installer.rs:1591 asks `settings_file_here`. Reverting the arm fails installer.rs:7663 ("codex at Project"). |
| N-H1 High, CLI-only Antigravity home undetected | closed | vendor_antigravity.rs:186, :440-442; unit test :642-657; test_detect_settings_roots.py:24-33; upgrading.md:43. A t1 probe detects `[gemini, antigravity, warp]`. |
| N-H2 High, load test listed six clients | closed | install_state.rs:5809-5813 derives `settings_hosts()` from `ClientTarget::ALL`. |
| N-H3 High, no end-to-end test for the new harnesses | closed | test_settings_every_host.py:111-168 covers all 13 hosts (build, install, reload, record, status, uninstall), and it runs in the gate. |
| N-H4 High, `kind_read` froze rows naming an unknown kind | closed | registry_catalog.rs:1469, :1497-1504 (three tiers); test :4314-4370. |
| N-H5 High, comma-ending comment | closed | json_splice.rs:1715-1733 takes the comma by token; test :3690 asserts at :3753. A sweep of about 670k cases found 0 failures, while the same sweep found 821 broken outputs before the change. |
| N-H6 High, upgrading.md downgrade claim | closed | upgrading.md:287-325, checked against tags v0.14.0, v0.14.3 and v0.15.0. |

**Follow-up contracts the fix pass claimed to close:**
- Closed: C-006, S-003, C-024, C-025, C-033, C-035, S-002 and S-010.
- C-005 is closed. One edge (an emptied container) is resolved under D-9.
- C-013 is closed for the pin-change path only; codex's counterexample makes it contradicts (C3-W37).
- C-016 is partial (C3-W32).
- C1:31 is closed (t5).

### Stage 1 — Correctness

#### Spec compliance (post-implementation traceability)
Nine seats ran: s1 to s8 on pipelines, s9 on seams.

- **C3-B1 [Block]** docs/src/content/docs/clients.md:503-504, also src/install/vendor_antigravity.rs:9-10 [s8 Block, s9 High; max-wins].
  - The page says "The Antigravity IDE (v2.1.x) reads `~/.gemini/antigravity/skills/` instead, so this client name does not serve it". That is false.
  - The upstream skills page, `.agents/upstream-watchlist.md:89`, `:96` and research V13 all say the IDE reads `~/.gemini/config/skills/`, and it still reads the legacy dir.
  - Cause: [dd47d33](https://github.com/grimoire-rs/grimoire/commit/dd47d33) and [a6183d9](https://github.com/grimoire-rs/grimoire/commit/a6183d9) were built in parallel at d4a3b67 (R1).
  - Remedy: state that the client serves the IDE too, and fix the vendor doc comment.
- **C3-B2 [Block]** catalog/skills/grim-authoring/references/hook-spec.md:473-475 [s8].
  - "a hook sorts last" is false: settings sorts after hook (artifact_kind.rs:77-85; bundle.rs:135).
  - test_docs_kind_lists.py:29-31 does not match this phrasing (R2).
  - Remedy: name the order, and widen the test's pattern to the "sorts last" or "sorts first" forms.
- **C3-H3 [High]** src/install/settings_phase.rs:2066-2073 with `left_behind` :2092-2105 and `warn_left_behind` :339-347 [s1 High; t1 Warn and codex P7a merged].
  - `left_behind` names every unselected client the record holds, whatever value it holds. Four consequences:
    - The printed `grim install --client <other>` names the first client back, so it never converges.
    - Under autodetect, a client that is no longer detected keeps the pin old for good: status reads `outdated`, and the warning and layer read repeat on every run.
    - On install or add, a client dropped from `[options].clients` is told `grim install --client <dropped>` instead of `grim update`.
    - At global scope the remedy omits `--global`, so typed as shown it exits 79 (t1 probe e8). The sentence also says "kiro still hold".
  - Remedy:
    - Count as left behind only a client whose recorded outputs differ from what the new pin would write.
    - Scope the remedy, and route a dropped client to `grim update`.
- Warns:
  - **C3-W1** [s1] adr_settings_artifact_kind.md:856 (PD-30). "each layer is read once per run" is false for `update` and `add`. The preflight (update.rs:139, add.rs:414) and installer.rs:387 each read every armed layer, so its warnings print twice. Remedy: pass the preloaded layers through, or correct the claim.
  - **C3-W2** [s1] settings_phase.rs:1458-1485. A dropped client withheld by the fence is reported in `kept_modified_clients`, which is documented as "locally modified".
  - **C3-W10** [s1, s2, s9] prune.rs:153-176. The doc block of `prune_orphans_off_runtime` was inserted under `prune_orphans`' doc, so the merged comment carries two `# Errors` sections and `pub fn prune_orphans` (:201) has no doc.
  - **C3-W11** [s1, s3; t4 Low] oci/settings.rs:896. `ClientWithoutSettings` dropped "a newer grim may apply it", although only a layer from a newer grim can carry such a table. settings.md:121-124 quotes only the unknown-client wording.
  - **C3-W12** [s1] .claude/rules/subsystem-cli-commands.md:26 says "three" settings causes; there are five.
  - **C3-W26** [s3] src/hook/trust.rs:381-391. The record-failure tail for the `Hooks` noun names `grim consent allow`, against the impl's own rule at :338-340.
  - **C3-W27** [s9 Warn, s3 Suggest, codex P6; max-wins] json-interface.md:96 and catalog consume.md:283-289 lack the settings-row `clients_missing: []` rule. The code has it (status.rs:1891), and so does commands.md:837-839. C-068 is partial.
  - **C3-W29** [s9] installer.rs:640-659. `configured_clients` ([57a7dab](https://github.com/grimoire-rs/grimoire/commit/57a7dab)) is a second `[options].clients` reader and a second config parse (browse-filter `compile_set`) on the async thread. That breaks the one-global-load rule (global_config.rs:58-63), and the W-S2 seam test scans only command.rs.
  - **C3-W30** [s4] install_state.rs:1900-1902. The load filter treats `.json` and `.jsonc` as one file for every client, but only OpenCode, Amp and Kilo choose by disk. A recorded `claude-root/settings.jsonc` survives load, and a probe showed an orphan prune editing `~/.claude/settings.jsonc`. The diff widened this to Kiro's nested path. No attacker applies (`global.json` is machine-local).
  - **C3-W31** [s4] arch-threat-model.md:92 and subsystem-install-layout-settings.md:63-70 omit PD-46's accepted Warp macOS/Linux residual, which the ADR records.
  - **C3-W32** [s5] installer.rs:3368 (toml_splice.rs:135). A stale member in an unparseable Codex `config.toml` is still dropped from the record with no warning and no `abandoned_entries` row. N-W16 is partial, and so is C-016.
  - **C3-W33** [s5] upgrading.md:535-536 says "leave that client's adopted entry in place" without a condition. With `--force` all three commands remove it (installer.rs:3365).
  - **C3-W34** [s5] json-interface.md:97 and commands.md:717-718. Without `--force`, an edited stale member of a selected client refuses the row; it is named only under `--force`.
  - **C3-W37** [codex P3 Block, held at Warn; s5 Deferred merged; resolved D-7] installer.rs:1767-1770 (MCP path :2981-2987). The dropped-adopter carry-forward applies only when `pin_changed`.
    - `install`, `add` and TUI carry a dropped client's outputs at the old pin, inside a record at the new pin.
    - A later unchanged-pin `grim update --client X` that has a reason to reconcile, such as one missing sibling output, skips the protection. It rewrites the adopted member to the new value: the member is `tracked` (:3108-3115), and `adopted` turns false once the value differs (:3127). `update` then reaps X (update.rs:305), and the user's entry is gone without `--force`.
    - s5's half: a carried client that is re-added reads `installed` while its registration holds the old definition.
    - Contradicts C-013.
    - Remedy: decide the dropped adopter on every reconciliation, and pin the mutant.
  - **C3-W38** [s5, Deferred, resolved D-8] plan_settings_follow_ups.md:101 (PD-45) and the 57a7dab body say "per output". The code decides per client (installer.rs:1782-1809), for the reason its comment gives. Remedy: correct the record, not the code.
  - **C3-W44** [s8] .agents/upstream-watchlist.md:288-291 and .claude/skills/upstream-refresh/references/domains.md:95 say settings.md does not name the Claude limits. It does (settings.md:865-881, [0aede2c](https://github.com/grimoire-rs/grimoire/commit/0aede2c)). Same R1 cause as C3-B1.
  - **C3-W45** [s8] settings.md:387. The Cursor decline example omits the absolute path that `ClientConfigAbsent` prints (settings_phase.rs:782).
  - **C3-W46** [s8] There is no committed `task docs:check` record after 57a7dab's doc hunks, so C1:61 is partial.

#### Test coverage (Specify-phase adequacy)
- **C3-W3** [s1, s2] settings_phase.rs:479. Uninstall's own `fence: RootFence::of(scope)` can become `RootFence::default()` and no test fails.
- **C3-W4** [s2] settings_phase.rs:844. The home exemption's main layout, a repository under `$HOME`, is not pinned, so an operand swap at :844 survives.
- **C3-W5** [s2] settings_phase.rs:2095-2097. The `kept: None` branch of `left_behind` is untested.
- **C3-W6** [s2] settings_phase.rs:2138-2143. No test asserts that `no-applicable-settings` is absent for a root-refused package.
- **C3-W7** [s2] settings_phase.rs:296-298. Deleting the refused-home early return in `withheld_rows` survives every test.
- **C3-W8** [s2] settings_phase.rs:2110-2111, :2119-2121. The two `selection_remedy` fallbacks are untested.
- **C3-W9** [s1 Warn, s2 Suggest] vendor.rs:2314-2319. The N-W2 guard has no positive control, so weakening the predicate passes while the table is clean.
- **C3-W24** [s3] test_settings_status.py:338-360. The N-W5 test cannot fail on the D12 revert, because OpenCode is not a project-scope settings host.
- **C3-W28** [s9] status.rs:1891. The settings-row guard has no failing test at global scope.
- **C3-W35** [s5] installer.rs:3377, :2466 (N-W17). Restoring the "or pass --force" remedy fails no assertion.
- **C3-W36** [s5] update.rs:509 (N-W11). The test passes `&mut []` for `pruned`, so dropping the pruned rows from the sort fails nothing.
- **C3-W40** [s7] test/manual/README.md:500-501, :973-975. After §13e the rig's GRIM_HOME declares the settings package, so §7's unguarded `grim --global install` writes the real `~/.claude`, `~/.codex` and `~/.copilot` files.
- **C3-W41** [s7] taskfiles/p9.taskfile.yml:47-51, verify-deep.yml:300-311. The p9 self-test is the only assertion that kills the new `element` predicate's mutant, and no gate runs it.
- **C3-W42** [s7] plan_settings_artifact_kind.md:622-631. No golden or old-grim run is recorded after [475d442](https://github.com/grimoire-rs/grimoire/commit/475d442). The seat ran old-grim: 9 passed.
- **C3-W43** [s7] README:964-972. `in_rig` ignores Claude managed settings, whose `env.CLAUDE_CONFIG_DIR` wins over the shell (vendor_claude.rs:660-683).

### Stage 2 — Adversarial panel

#### Quality (user-feedback slot)
t1 ran `target/release/grim` in scratch environments against a private local registry, and changed nothing.

- **C3-H4 [High]** settings_phase.rs:1107 with :2137-2160 [t1 High; s1 Warn on the layout rule :72-73 merged, max-wins].
  - The fence's cause is printed only from `plan`, which runs only when `survey` finds the record stale or a surface missing. `rows()` then suppresses its own warning, assuming `plan` already named the decline.
  - At an unchanged pin, nothing prints:
    - Probe e2 (`KIRO_HOME=kiro-rel` inside a repo): run 1 warns; runs 2 and 3 print `skipped` with empty stderr, and `update` prints `unchanged`.
    - Probe e3: an intact recorded in-repo file makes `install` report `unchanged` (s1: `already-installed`) while `status` reads `not-armed`.
  - This contradicts settings.md:433-437 and the docstring at test_settings_engine_root.py:7. The survey and status `footprint` also hash the refused file before the fence runs, against layout rule :72-73.
  - Remedy:
    - Emit the decline from one place (`rows()` or the survey) whenever a selected client's surface is refused.
    - Add a second `install` to test_settings_engine_root.py:72-115.
- **C3-H5 [High]** settings_phase.rs:769-772, :791-795, :801-822; artifact_status.rs:382-386; settings.md:397; json-interface.md:675; catalog/skills/grim-usage/references/troubleshooting.md:410-414 [t1].
  - The `settings-root-in-workspace` remedy always tells the user to fix a variable. That is wrong whenever the file lies inside only through a link: the dotfiles case that settings.md:440-442 and the threat model document.
    - Probe e3 (stow-style `~/.claude` link, no `CLAUDE_CONFIG_DIR`): "set CLAUDE_CONFIG_DIR … or unset it".
    - Probe e4: "set HOME … or unset it" for droid and antigravity. For a HOME-placed client this cause can only come from a link, so that advice is wrong every time it prints.
  - Remedy: carry the matched spelling in `RootInWorkspace`. For a link, say that it links into this repository and to run grim from outside it. Add "or run grim from outside this repository" everywhere.
- Warns:
  - **C3-W21** [t1] settings_phase.rs:758-762 (status.rs:3690-3703). A per-client note tells the user to change the client selection even when the row applied for other clients. Following it would drop Zed from skills, rules and MCP.
  - **C3-W22** [t1] oci/settings.rs:942. "unknown vendor table <name>" has no next step, unlike its sibling (`unknown_key_hint`, error.rs:388-401).
  - **C3-W23** [codex P7b, verified] settings_phase.rs:2126-2127. At project scope, the remedy for values that are global-only says "run `grim install --global`" and omits declaring the package globally. The `SettingsDecline::GlobalOnly` remedy includes that step.
  - **C3-W47** [t1, Deferred, resolved D-5] status.rs:1499-1545, commands.md:837-839. A newly selected capable client gets no signal on a settings row: `clients_missing: []`, `outputs_pending: []`. D12's premise ("a client the package declares no value for") does not cover that case.
  - **C3-W48** [t1, Deferred, resolved D-6] install_report.rs:60-63, json-interface.md:95, :690-693. A fence refusal exits 0 and leaves the install report with `skipped` or `unchanged` and `armed: null`. Scripts cannot see it without `grim status`.
- Suggest: see the compact list.

#### Security
t2 ran probes against a private zot on a free loopback port. No harness binary ran.

- **C3-H1 [High]** settings_phase.rs:863-866 with `resolved_prefix` :892-905 [s2; t2 confirmed; t6 named the mechanism].
  - A global root spelled with `..` after a missing component passes the fence. At `nope/..`, `file_name()` is `None`, so `resolved_prefix` returns the input unresolved (:899-901), and neither spelling `starts_with(tree)`.
  - t2 probe p1: `KIRO_HOME=$S/nope/../repo/kiro` made `grim --global install` exit 0. It created `nope` and wrote `repo/kiro/settings/cli.json`. For Codex, `config.toml` landed in the repository and the rules file was then refused mid-run, leaving a partial write.
  - CWE-41, attacker T3: `.envrc` `$PWD` or `.mise.toml` `{{config_root}}` with `..`. A link planted at the landing path makes grim write through it (`atomic_write_through_symlink`).
  - Not a recorded edge (arch-threat-model.md:99-103). settings.md's "a file counts as inside when its own path … lies there" is false for this spelling.
  - Held at High, not Block: the values come from the user's own global config, and the harness under the same environment already reads that place.
  - Remedy: refuse any root with a `..` component (or fold it lexically first), and add `..` rows to the fence units.
- **C3-H2 [High]** settings_phase.rs:841-848 with env.rs:123-126 [codex P1 Block; t2 High, Deferred; resolved actionable D-1].
  - `holds_home` reads `$HOME`. A repository sets that variable through the same files the fence exists for (arch-threat-model.md:94-95).
  - t2 probe p2: `GRIM_HOME` explicit outside, `HOME=$S/repo/fakehome`, cwd `$S/repo`. `grim --global install` wrote four files into the repository (Claude, Codex, Copilot under `fakehome`, plus Kiro). The change list printed `~/.claude/settings.json`, which hides where the write landed.
  - CWE-15, T3. One variable drops the fence for every client. The threat model lists two edges, and not this one.
  - Remedy (D-1):
    - Record it as the third edge: arch-threat-model.md T3 fence paragraph, a new PD row next to PD-28, and settings.md "Config roots inside a repository".
    - Print the resolved path, not a `~` form, in the change line when `$HOME` lies inside the command's work tree.
- **C3-W13** [t6 Warn; t5 Deferred; t2 Suggest; resolved D-3] settings_phase.rs:871-873. `work_tree` returns the nearest `.git` holder. From inside a submodule or nested clone, a root elsewhere in the superproject passes, though direnv applies the superproject's `.envrc` there (t2 probe p5). Remedy: fence the outermost ancestor holding `.git` that does not hold `$HOME`.
- **C3-W14** [t2, Deferred, resolved D-2] settings_phase.rs:871-873. A checkout with no `.git` (zip, tarball, jj, hg, Sapling, or a `.git` file without `gitdir:`) is unfenced, and the threat model does not record it. CWE-15, T3. Remedy: record it as an edge.
- **C3-W15** [t2 Warn, t6 Suggest] settings_phase.rs:875-889. `holds_git` opens `.git` whatever its type.
  - A FIFO, or a link to `/dev/tty`, named `.git` in any ancestor hangs every `grim --global` run that has settings on. Probe: killed by `timeout 8`.
  - Sources: a tarball (T3) or a `/tmp/.git` planted by another local user (T5). CWE-400.
  - Remedy: read only `Ok(meta) if meta.is_file()`.
- **C3-W16** [t6] settings_phase.rs:813-820. `current_dir().ok()` turns an unreadable cwd into "no fence" (fails open), while every other branch fails closed.
- **C3-W25** [s3; t2 partial] oci/settings.rs:441-445. `authority_has_userinfo` cuts the authority at `\` (WHATWG). git's `credential_from_url` and curl cut only at `/?#`, so `https://x\:tok@host/m` builds with exit 0 and git reads credentials from it. CWE-522, T1.

#### Performance
t3 found no cost. One Suggest: a row naming an unknown grimoire kind costs at most one manifest GET per TTL hour (by design, N-H4). It is merged with s4 S3.

#### Documentation
t4 found 0 Critical, 0 Medium and 0 Accuracy that are new, and 3 Low. Its convergence covers the fence pages, the exit codes, `clients_missing`, and 57a7dab's and [95268dd](https://github.com/grimoire-rs/grimoire/commit/95268dd)'s hunks. A scripted check of 976 links on 29 changed pages found none broken. The Accuracy defects found in Stage 1 stand (C3-B1, C3-B2). The settings.md:121-124 Low is merged into C3-W11.

#### Architecture
t5: **no boundary violation** (evidence in § Summary).
- **C3-W18** [t5] settings_phase.rs:801-821. `root_variable` is a third, hand-kept copy of the variable that places each settings file.
  - Tests pin only Kiro and Copilot: mutating `Claude => "HOME"` fails nothing.
  - Its "HOME" is wrong for Junie, OpenClaw, Goose, Zed and Cline, which matters once [#292](https://github.com/grimoire-rs/grimoire/issues/292) gives any of them rows.
- **C3-W19** [t5] adr_settings_artifact_kind.md:578, :657-658, :732, :789, :826, :977. The ADR was not amended for PD-28 or PD-32:
  - the causes table lists 3 of 5;
  - the token count says 21, not 23;
  - Decision 11 and the exit table still say 75 for a file that stops parsing;
  - `SettingsRaced`, `SettingsBecameUnparseable` and `InvalidBindingName` are absent;
  - the changelog row omits PD-28, PD-30 and PD-32.
- **C3-W20** [t5] json_config.rs:39-46, json_splice.rs:1035-1037, :1345-1366 ([0a4ab18](https://github.com/grimoire-rs/grimoire/commit/0a4ab18)). Accepting a byte-order mark changes shipped behaviour for every JSON vendor file grim splices (MCP, hooks, `claudeMdExcludes`): v0.15.1 refused such a file with exit 74. Nothing records it. upgrading.md:505-509 says no install fix "changes an exit code". Remedy: add an upgrading.md paragraph and an ADR matrix row, and give the change its own `fix(install):` subject at `/hex-finalize`.
- **C3-W50** [t5, Deferred, resolved D-4] stability.md:251-264, :30-31. PD-32 changed 75 to 65 as "allowed for the experimental kind". But the settings Unstable list does not cover exit codes, which the Frozen table freezes at 0.16.0.

#### SOTA / technical soundness
t6 confirms C3-H1 as the std `Path::file_name()` pitfall. dunce, `url` 2.5.8 and tokio process use is current. One open point is unverified: case handling of realpath on macOS (no macOS host). t2 reasons that case alone opens no hole.
- **C3-W17** [t6] test/tests/test_settings_engine_root.py:43. `git init` runs with `{**os.environ}`, so an inherited `GIT_DIR`, `GIT_WORK_TREE` or `GIT_INDEX_FILE` (pytest under a git hook) re-initialises the outer repository. The call also has no `timeout=`.

#### Suggest (30, compact; actionable unless marked)
1. [s1, t5] settings_phase.rs:831-839, :1078. `RootFence::of` reads cwd and stats ancestors inside `src/install`, and status rebuilds it per client. Build it once at the command seat, on `InstallTarget`.
2. [s1] settings_phase.rs:1500-1503, :1419-1430. A refused recorded value gets two warnings for one cause.
3. [s1] update.rs:261, :279. If `prune_orphans_off_runtime` fails after `kept` moved the pin, the settings reap never runs.
4. [s2] settings_phase.rs:5049. "invalid settings binding name: skill name '…'": the reason calls a settings binding a skill name.
5. [s2, t5] install_error.rs:209-215 vs command_error.rs:74-79. Two variants carry one contract, and the non-Settings arm (error.rs:643) is unreachable in production.
6. [s3] hook_registrar.rs:2204-2207. The comment says an un-killed git keeps grim alive; with null stdio it does not.
7. [s3, t1] tui/app.rs:3446. `grim uninstall --force` is not runnable as written (kind and name are required). The uninstall refusal says "change" where it should say "remove" (install_error.rs:170).
8. [s3] add.rs:880-886. "set or changed outside grim" is now also used for untracked skill, rule and agent destinations.
9. [s4] install_state.rs:5973-5975, :5995-5997. The derived-host tests pass vacuously when every path is `None`.
10. [s4] test/src/helpers.py:319-320. The Warp comment is stale since N-W1.
11. [s4, t3] registry_catalog.rs:1497-1504. An empty `com.grimoire.kind` is re-read on every refresh, forever.
12. [s5] installer.rs:1795-1799. `grim install --client X` outside `[options].clients` writes nothing for X, says nothing, and reports `updated`.
13. [s5] installer.rs:3376-3379. The stale "leaving … no longer records it" warning fires before the writes.
14. [s6] json_splice.rs:3766-3807. The PD-43 grid has no CRLF, tab, trailing-comma or BOM axis.
15. [s7] README:966-967. The `in_rig` guard is lexical: `…/.settings-roots/../../../../realhome/.codex` passes.
16. [s7] test_settings_real_harness.py:225. `pytest.raises` has no `match=`.
17. [s7] test_stdio_drivers.py:160-170. The pid-file race can flake on a loaded host (never a false pass).
18. [s7] test_golden_pre_settings.py:462-463. The offline case has no negative control.
19. [s8] clients.md:269-270. "every platform but macOS and Linux, where grim has no root" reads backwards.
20. [s8] upgrading.md:295-296. An em dash, plus "newer" and "older"; name the versions.
21. [s8] research_upstream_appendix_b_settings_20261105.md:426. A relative `(#modelsettings)` link.
22. [s8] consume.md:430-433. `abandoned_entries` is described too narrowly for `update`.
23. [s9] There is no acceptance case for a relative root with a prior record.
24. [t1] Inconsistent "re-run" and "rerun", and inconsistent backtick use, across the new messages.
25. [t1] install_error.rs:193-197. The mid-run unparseable message could name line and column.
26. [t4] settings.md:60-71. The consent-reason table omits the flag-off and insecure-transport reasons (policy.rs:405, :413).
27. [t4] Several new sentences exceed 25 words (settings.md:392-395, upgrading.md:531-537).
28. [t5] No source-level guard pins that `src/install`, `src/hook` and `src/api` never name `crate::command` or `crate::tui` (C1:31 has no regression test).
29. [t5] install_state.rs:1864-1867 → settings_phase.rs:911-930 closes an `install_state ↔ settings_phase` cycle. Move `settings_file_here` beside `settings_config_path` during the planned `InstallPass` refactor.
30. [t5] registry_catalog.rs:1497-1504. `manifest_names_a_grimoire_kind` restates `kind_from_manifest`'s tiers in another module.

### Convergence
Unmet requirement IDs. Gap rows are appended at the end of each plan's Parallelization table, with matching steps.

**plan_settings_artifact_kind.md**, rows G3-ENGINE, G3-RIG and G3-DOCS:
- PD-28 (the T3 fence; it carries no C-ID) — **partial**:
  - two bypasses (C3-H1 `..`, C3-H2 `HOME`);
  - the cause is silent after the first run (C3-H4);
  - the link-case remedy is wrong (C3-H5);
  - a FIFO hang (C3-W15), the nested-repo gap (C3-W13), no `.git` unrecorded (C3-W14), and an unreadable cwd fails open (C3-W16).
- C-047 — **partial**: the narrowed-run rule names every unselected client, so its remedy does not converge (C3-H3).
- C-061 — **partial**: §7 after §13e writes real dirs (C3-W40), and `in_rig` ignores managed settings (C3-W43).
- C-062 — **partial**: settings.md:433-437 claims the warning on every run (C3-H4), plus :387 (C3-W45) and :121-124 (C3-W11).
- C-064 — **contradicts**: clients.md:503-504 says the Antigravity IDE is not served (C3-B1).
- C-065 — **partial**: no docs gate record for the fix pass's doc hunks (C3-W46).
- C-068 — **contradicts**: hook-spec.md:473-475 (C3-B2). Also **partial**: consume.md lacks the settings-row `clients_missing` rule (C3-W27).

**plan_settings_follow_ups.md**, row G3-274:
- C-013 — **contradicts**: after `install` carries a dropped client's adopted member at the old pin, an unchanged-pin `grim update --client X` with one missing output rewrites the member to the new value and then reaps it, without `--force` (C3-W37).
- C-016 — **partial**: a stale member in an unparseable `config.toml` is dropped unnamed and unwarned (C3-W32).

### Fold-Back
Not performed. The verdict is not Approve, the check is unconverged, and neither plan carries a `## Spec Deltas` block.

### Cross-Model Adversarial
- **Leg:** ran, degraded. codex-cli 0.166.0, `-m gpt-6.1-sol`, `-s read-only --ephemeral --skip-git-repo-check`, reasoning effort high. It ran from the throwaway detached worktree `.agents/worktrees/review3-codex` at 9a951ee, with `AGENTS.md`, `CLAUDE.md`, `.claude/`, `.codex/`, every `.env`, hooks and credential files removed. The worktree was removed afterwards.
- **Sandbox probe (once):** `SANDBOX_FAIL`, `bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted`. codex therefore reviewed the pasted d4a3b67..HEAD diff (about 560 KB) with no shell. It ran under neither sandbox-off nor full access. Exit 0, no usage-limit text.
- **Returned:** P1, P3, P6 and P7 with counterexamples; P2, P4, P5 and P8 "not found". Those "not found" lines rest on hunks alone and are weak evidence. P5 in particular missed s1's C3-H3.
- **Triage:**
  - **[Block] settings_phase.rs:844, P1, repository-controlled `HOME`:** actionable, merged into C3-H2 with t2. **Escalation noted:** codex graded Block. The severity is held at High:
    - the impact equals C3-H1's;
    - the values written are the user's own;
    - the harness under that environment reads the same place;
    - the remedy is a recorded edge plus legibility, because a trusted home source needs `getpwuid_r`. `unsafe_code = "forbid"` (Cargo.toml:176) rules that out, and so does product-tech-strategy.md, which lists no crate for it.
  - **[Block] installer.rs:1768, P3, adopted protection lost at a carried pin:** actionable as C3-W37; it also makes s5's Deferred carry-staleness actionable.
    - I verified the path by reading. `preserved_recorded_clients` returns early unless `pin_changed` (installer.rs:1767-1770; MCP at :2981-2987). At an unchanged pin the member is `tracked` (:3108-3115). `adopted` turns false once the value differs (:3127). `update` then reaps the client (update.rs:305, kept = `[options].clients`).
    - **Escalation noted:** held at Warn, by call 2's N-W12 and N-W13 precedent. It needs an earlier install carry, a missing sibling output, and `--client` naming a dropped client.
  - **[Warn] json-interface.md:96, P6:** duplicate of s9 and s3 (C3-W27).
  - **[Warn] settings_phase.rs:344, P7a, remedy omits `--global`:** duplicate of t1, merged into C3-H3.
  - **[Warn] settings_phase.rs:2130, P7b, global-only remedy omits the declaration step:** actionable, new (C3-W23), verified at :2126-2127.
  - Stated-convention: 0. Trivia: 0.

### Root-Cause Analysis
Five Whys per cluster. Every finding above Suggest belongs to one cluster, or carries its own chain (C3-H4).

**R1 (systemic): parallel pipelines built from one base, landed without a cross-pipeline consistency read.**
- **Issue:** two pipelines state one fact differently, or one adds a second copy of another's fact.
- **Why 1:** clients.md says the IDE is not served, while the watchlist and research say it is. The watchlist still says settings.md lacks the Claude limits. `configured_clients` re-reads the config. The `clients_missing: []` rule reached commands.md but not json-interface.md or consume.md.
- **Why 2:** dd47d33, a6183d9, 57a7dab and [ef14cd8](https://github.com/grimoire-rs/grimoire/commit/ef14cd8) were each written at d4a3b67, without the others' changes.
- **Why 3:** landing replayed the patches with `git apply --3way`. That resolves text conflicts, not contradicting statements in different files.
- **Why 4:** each pipeline gate and the rehearsal gate test behaviour. No step reads the landed tree for a fact stated twice.
- **Root cause:** the landing protocol has no seams read before the landing gate.
- **Fix:** after the patches land and before the gate, one seams seat reads the union diff for facts stated in two places. The fix pass's G3-DOCS row does this for the known cases.
- **Related:** C3-B1, C3-W27, C3-W29, C3-W44.

**R2 (systemic, recurring from call 2): a fix is credited with a test that cannot fail on its mutant, or with none.**
- **Why 1:** the N-W5 test runs where the guard is a no-op. The N-W2 guard has no positive control. The uninstall fence, the global-scope guard, N-W17, the N-W11 pruned half and five `left_behind`/fence branches survive deletion. The p9 self-test runs in no gate. The drift test misses "sorts last".
- **Why 2:** each fix step states "Red" in the plan, but the reds were taken on the happy path, not by planting the fix's own breaking edit.
- **Why 3:** call 2's RCA named the same cause. The fix pass shipped mutation-killing tests for the findings it listed, but not for the branches it added while fixing them.
- **Root cause:** no gate requires a planted-mutant check per new branch. Coverage evidence is narrated in a report, not pasted into one.
- **Fix:** every gap step in this synthesis names the mutant its test must kill. `/hex-execute` should paste the red run.
- **Related:** C3-B2, C3-W3–W9, C3-W24, C3-W28, C3-W35, C3-W36, C3-W41.

**R3 (systemic, security): the fence compares environment-derived spellings lexically and trusts an environment-derived anchor.**
- **Why 1:** `..` after a missing component defeats `resolved_prefix`. `$HOME` from the environment switches the fence off. The nearest `.git` is taken for the repository, and no `.git` means no fence. On load, `.json` and `.jsonc` count as one file for every client.
- **Why 2:** PD-28 specified a "literal or link-resolved spelling" check and a `$HOME` exemption. It did not name lexical normalisation, nor where `$HOME` comes from.
- **Why 3:** the S-5 research seat weighed layout moves, not path-canonicalisation pitfalls. No researcher-pitfalls pass ran on the fence before landing.
- **Root cause:** a trust-boundary control was specified as a predicate over spellings rather than over a normalised path with a trusted anchor.
- **Fix:**
  - Refuse `..` components.
  - Fence the outermost work tree, and read `.git` only as a regular file.
  - Record the `HOME` and no-`.git` edges, which a safe-Rust grim cannot close today.
  - Compare the exact spelling on load for clients that do not choose by disk.
- **Related:** C3-H1, C3-H2, C3-W13, C3-W14, C3-W15, C3-W16, C3-W30.

**R4 (systemic, recurring from call 2's R8): remedies are computed from a partial view of why a decline happened.**
- **Why 1:**
  - `left_behind` does not know whether a client's value changed.
  - The root remedy does not know whether a link or the variable put the file inside.
  - The per-client note does not know whether the row applied for other clients.
  - The global-only remedy does not know whether the package is declared globally.
  - "unknown vendor table" and the hooks tail fall back to a generic text.
- **Why 2:** each remedy is a string chosen at the decline site, from the decline's enum alone.
- **Why 3:** the decline types carry the outcome, not the evidence. For example, `RootInWorkspace(file)` drops which spelling matched.
- **Root cause:** decline variants carry too little context for a correct next step.
- **Fix:** carry the deciding evidence in the variant (matched spelling; scope; applied elsewhere; declared globally), and assert each remedy's text in a test.
- **Related:** C3-H3, C3-H5, C3-W21, C3-W22, C3-W23, C3-W26, C3-W45.

**R5 (systemic, recurring from call 2's R3): decision records are not amended in the commit that changes the decided behaviour.**
- **Why 1:**
  - The ADR lags PD-28, PD-30 and PD-32.
  - PD-45's text contradicts its code.
  - The 09e4f0d body misstates the offline warning.
  - The cli-commands rule counts three causes, not five.
  - The threat model omits PD-46's residual.
  - The settings exit codes are not in stability.md's Unstable list.
  - The BOM change is unrecorded.
- **Why 2:** the fix pass recorded decisions in the plan's PD table, which is not the canonical record a reader consults.
- **Why 3:** the docs pipeline synced pages and rules from the PD table, but the ADR and the threat model sit outside every pipeline's expected files (PD-29).
- **Root cause:** no pipeline owns the canonical records when a PD amends them.
- **Fix:** G3-DOCS owns the ADR, the threat model and stability.md for this pass.
- **Related:** C3-W1, C3-W12, C3-W19, C3-W20, C3-W31, C3-W38, C3-W49, C3-W50.

**C3-H4, its own chain.**
- **Why 1:** a second run under a refused root is silent.
- **Why 2:** only `plan` emits the decline, and `rows()` defers to it.
- **Why 3:** `plan` runs only when the survey sees work. A refused root with an intact or absent record shows no work.
- **Why 4:** the survey hashes outputs before consulting the fence (layout rule :72-73).
- **Root cause:** the fence is applied at write time only, so read-time paths never learn of it.
- **Fix:** consult the fence in the survey and emit from one place.

**Unclustered Warns, each with a single direct cause stated at the finding:** C3-W2, C3-W10, C3-W11, C3-W17, C3-W18, C3-W25, C3-W32, C3-W33, C3-W34, C3-W37, C3-W39, C3-W40, C3-W42, C3-W43, C3-W46, C3-W47, C3-W48. C3-W37 has call 2's R5 cause again: adopted-entry protection keyed on one event (a command then, a pin change now), not on the entry.

### Deferred Findings
None remain open. Each doubt was resolved here, as the goal requires, recorded as question, research, decision.

- **D-1, C3-H2 (repository-set `HOME`).**
  - Question: fix in code, or record as an edge?
  - Research:
    - A trusted home needs `getpwuid_r`, which the crate cannot call: `unsafe_code = "forbid"` at Cargo.toml:176, and no safe crate is on the golden path (product-tech-strategy.md).
    - Narrowing the exemption to "the tree is `$HOME`" is defeated by `HOME=$PWD`.
    - With `GRIM_HOME` unset, a repository-set `HOME` already moves grim's whole global config into the repository. That is out of scope and self-recorded at hook_registrar.rs:1677-1700.
  - Decision: actionable High, resolved as a record in the threat model, PD-28 and settings.md, plus a resolved-path change line. A code fix waits for a safe identity source.
- **D-2, C3-W14 (no `.git`).**
  - Question: widen the markers, fall back to cwd, or record?
  - Research:
    - A cwd fallback refuses legitimate runs from inside a vendor dir, for example cwd `~/.config` with an XDG root.
    - jj, hg and Sapling markers add surface for a rare case (YAGNI).
  - Decision: record as an edge.
- **D-3, C3-W13 (nested repository).**
  - Question: fence the outermost tree, or record?
  - Research: the outermost `.git` holder below the home-holding tree is one more ancestor walk. It loses no legitimate case, since the superproject is also a repository.
  - Decision: fix in code.
- **D-4, C3-W50 (settings exit codes).**
  - Question: freeze them at 0.16.0, or list them as Unstable?
  - Research: PD-32's own basis is "experimental kind". stability.md:252-262 lists the kind's other surfaces.
  - Decision: add 65, 75, 78 and 81 for settings to the Unstable list before 0.16.0.
- **D-5, C3-W47 (a newly selected client is invisible).**
  - Question: a status signal, or a documented limit?
  - Research: status reads no document (ADR D8 / D12), and a signal would need the layer.
  - Decision: document in commands.md:837-839 and json-interface.md that a settings row never reports a newly selected client, and that `grim install` does.
- **D-6, C3-W48 (fence refusal invisible to scripts).**
  - Question: a new report key, or a documented read?
  - Research: PD-28 decided exit 0. A key would freeze at 0.16.0 for a rare event (PD-27 precedent).
  - Decision: json-interface.md tells scripts to read `grim status` arming after a settings install.
- **D-7, C3-W37 (codex P3 plus s5's carry-staleness).**
  - Question: actionable now?
  - Research: verified by reading (see § Cross-Model). C-013's "never" fails within one `update`.
  - Decision: actionable Warn. Decide the dropped adopter on every reconciliation, not only on a pin change, and pin the mutant.
- **D-8, C3-W38 (PD-45).**
  - Question: the code or the record?
  - Research: installer.rs:1782-1809 decides per client because registering rewrites every file the client owns. Its owner test is `test_dropped_client_keeps_its_edited_file_and_reaps_the_other`.
  - Decision: correct PD-45's text. Correct the 57a7dab body at `/hex-finalize`.
- **D-9, s6 Deferred (an emptied container drops its interior comments).**
  - Question: does this contradict C-005?
  - Research: json_splice.rs:237-242 and :338-341 predate the branch ([6f16d0b](https://github.com/grimoire-rs/grimoire/commit/6f16d0b)). 0a4ab18's comment at :3897 states the reading. The settings writer keeps a commented emptied container (`empty_container_has_comment`).
  - Decision: C-005 holds, since the removed span includes the emptied container member.
    - Record that reading as a follow-ups PD row (counted as C3-W39).
    - Keeping a commented emptied container in the flat removals would change released kinds, so it goes on the out-of-scope list.
- **D-10, C3-W49 (the 09e4f0d body is false about the offline warning).**
  - Decision: correct it when `/hex-finalize` regroups history. No code change.

Two Warns come only from these resolutions:
- **C3-W39** [s6 Deferred, resolved D-9]: record D-9's reading of C-005 as a PD row in plan_settings_follow_ups.md.
- **C3-W49** [s2 Deferred, resolved D-10]: the [09e4f0d](https://github.com/grimoire-rs/grimoire/commit/09e4f0d) body says "the offline warning names only declared clients", but settings_phase.rs:1133-1143 names `survey.missing`.

### Out of scope (for the meta-orchestrator to file)
Each predates d4a3b67 or lies outside the goal. None duplicates [#284](https://github.com/grimoire-rs/grimoire/issues/284)–[#304](https://github.com/grimoire-rs/grimoire/issues/304).

1. **[High, t2]** hook_registrar.rs:1661-1704, `validate_grim_home` at global scope.
   - A `GRIM_HOME` inside the cwd's repository passes, for example a repository-set `HOME` with `GRIM_HOME` unset. The repository then supplies grim's whole global config and settings records: probe p3 wrote `.claude/settings.json` and `global.json` into the repository.
   - T3, CWE-15. The code self-records it ("Tracked separately"). `RootFence` now computes the tree that check lacks.
2. [Warn, s6, D-9] json_splice.rs:237-242, :338-341. Removing the only item drops the container with its interior comments (rules and MCP uninstall).
3. [Warn, s6, s9] vendor_claude.rs:708-726. `settings_env_config_dir` reads Claude's settings without stripping a BOM, so a BOM-prefixed file loses its `env.CLAUDE_CONFIG_DIR` layer.
4. [Warn, s3] status.rs:1534-1537. The nothing-applies note ends "then run grim install" on a global row too.
5. [Warn, s5] commands.md:727-729. "`grim install` … never deletes", yet the stale removal splices a member out.
6. [Warn, s8] settings.md:353-354. The plain-output warning claim does not hold for `uninstall` or the update reap.
7. [Warn, s8] json-interface.md:633, :968. Example messages are not what grim prints.
8. [Warn, s7] test/manual/README.md:493-502 (§7). Its global add and install are unguarded.
9. [Warn, s4] install_state.rs:1105-1122. `merge_settings_records` joins after `drop_illegal_outputs` (attacker N2 only).
10. [Suggest, s1, s9] update.rs:305. `reap_dropped_clients` deletes files on the async thread.
11. [Suggest, s7] verify-deep.yml:6-9. Workflow-level `checks: write` and `pull-requests: write`.
12. [Suggest, s7] test_mcp_cline.py:311-315. The busy poll never asserts that grim waited.
13. [Suggest, s8] catalog/descriptions/grim-usage.md:4 omits settings packages.
14. [Suggest, t6] hook_registrar.rs:2189-2209. `git ls-files` honours a local `core.fsmonitor`.
15. [Suggest, t1] installer.rs:2967 with :3021. A vendor decline prints twice per run.
16. [Suggest, t1] install_error.rs:201. The pre-run unparseable error has no next step.
17. [Suggest, t1] add.rs:883. "run `grim install --force`" ignores global scope.
18. [Suggest, s5] installer.rs:3157 vs :3246. A non-adopted stale member is cut before the writes.
19. [Suggest, t5] The tracing-capture test writers are copied across about 10 modules.
20. [Suggest, t5] install_state → installer cycle (install_state.rs:46).
21. [Suggest, s6] A CR-only line comment reads as one comment (consistent refusal).

### Handoff
- **Verdict: Request Changes.** Blocks 2, High 5, Warn 50, Suggest 30.
- `/hex-review` calls: 3 of 3 spent. No further review call remains in this run.
- The fix pass's rows:
  - plan_settings_artifact_kind.md: G3-ENGINE (hard, review) and G3-RIG, wave 10; G3-DOCS, wave 11, after G3-ENGINE and the follow-ups row G3-274.
  - plan_settings_follow_ups.md: G3-274 (hard), wave 5.
- Highest-value order:
  1. C3-H1 and C3-H2 (security);
  2. C3-B1 and C3-B2 (doc Accuracy);
  3. C3-H3, C3-H4 and C3-H5;
  4. C3-W37 (C-013).
- Not a test-only fix: C3-H2 is resolved by record (D-1).
- Seats: s1–s9, t1–t6 and codex. Reports are in session scratch, not committed.
