## Code Review: main..hex/config-artifact-type (hex-review call 1 of 3, tier xhigh, loop mode)

### Summary
Verdict: Request Changes (3 Block, 4 High, 55 Warn after dedup, 106 Suggest as tagged, not deduplicated).
Integration gate `task --force verify` at e73a58c: exit 0; 5522 unit passed, 0 skipped; 2678 acceptance passed, 85 skipped; 91 AI-config passed. Golden (10) and old-grim (9) tests were among the skips (no registry on localhost:5000).
Cross-model review skipped: no codex CLI, no second harness on this machine.
Deviation: Stage 2 launched before the last Stage 1 seat (P10) finished; no Stage 2 seat reads Stage 1 output.
Seats: 15 pipeline seats (spec + test coverage) + seams; Stage 2 user-feedback, security, performance, docs, architect, pitfalls.
Decided items on the closing list (#280, #281, four Suggests, withdrawn `manual`) were not re-raised.

### Block (doc Accuracy -> Block)
- docs/src/content/docs/settings.md:203 (also :189, :204): says `install` lists an adopted value in `abandoned_entries`; `install` never emits it (src/command/install.rs:644). Docs-vs-code; ADR row 3c says report it. [docs, P9, ux]
- docs/src/content/docs/json-interface.md:563: `required_by` sort order omits settings (ArtifactKind::ALL ends with settings). Branch-introduced by omission. [docs]
- docs/src/content/docs/stability.md:692: registry-sourced kinds list omits settings; the hook omission predates the branch (main:645). [docs]

### High
- src/install/settings_phase.rs:1718: `--client`-narrowed install/update after a pin change strands unselected clients' values at the old pin while status reads them intact. [P6]
- docs/src/content/docs/guides/dependencies.md:29 (:169-170, :183-184): "drop one member" / "own declarations" claim about bundles is false. [P0]
- test/manual/README.md:953: part e sources an unresolvable path, so the global steps write the operator's real client dirs. [P-X]
- test/tests/test_settings_real_harness.py:94-105: no platform guard; off Linux it can reach the real home (CWE-668, by reference). [P-X]

### Warn — code
- src/install/installer.rs:1449: settings kind support ignores scope, so `clients_missing` is wrong at project scope. [P2]
- src/install/install_state.rs:1094, src/command/status.rs:1320, :1347-1351: unreadable store reads `missing` with no cause. [P3, P7, seams]
- src/install/install_state.rs:1772-1778: fail-open arm of `is_file_surface` untested. [PF]
- src/install/install_state.rs:1892-1896: hook payload key checked by shape only (CWE-639, by reference; Deferred). [PF]
- src/install/install_state.rs:1713-1715: unknown client string dropped at load, lost on next save (Deferred). [PF]
- src/install/settings_phase.rs:336-343, :1376-1378: record/output naming an unknown client dropped silently. [P6]
- src/install/settings_phase.rs:896-903, :815: `missing` / offline warning names clients the package declares nothing for. [P6]
- src/install/settings_phase.rs:262-395: blocking file I/O inside async `run` (reap, uninstall). [perf]
- src/install/settings_phase.rs:28: production `install -> command` import edge. [arch]
- src/install/installer.rs:295, src/install/settings_phase.rs:182, :788-792: layer re-read every run, contradicting ADR:845. [arch]
- src/error.rs:613-619: shipped Cline lock path gains reason `locked` / retryable, undocumented and untested. [seams, CW, P1, P5, arch]
- src/install/json_splice.rs:247 (cause :1470): scalar `/` stop widens #275. [P1]
- src/install/json_splice.rs:96-135, :1042-1046: BOM-prefixed JSON/JSONC refused. [pitfalls]
- src/install/settings_writer.rs:113-153: no cross-process lock; concurrent writers can overwrite (Deferred). [pitfalls]
- src/install/settings_writer.rs:129-141: non-UTF-8 file reported as "changed during the run, rerun". [pitfalls]
- src/install/settings_writer.rs:93-103: harness write during the run reported as "locked by another process". [ux]
- src/install/install_error.rs:168: refusal says "not written by grim" even for reason `modified` (also TUI, add hint, settings.md:216-228). [ux]
- src/install/install_error.rs:173: conflict message gives no remedy. [ux]
- src/command/status.rs:1483-1490 (+ settings_phase.rs install warnings): three settings notes print cause without remedy. [ux]
- src/command/status.rs:1456-1462: probe timeout does not bound the thread; runtime dropped without shutdown_timeout (main.rs:186). [P7, perf]
- src/tui/app.rs:3443-3446: refused uninstall names `grim install --force`. [P7, seams, ux]
- src/command/install.rs:611, src/command/update.rs:417: refusal stderr names one package for pieces of several. [P8]
- src/oci/settings.rs:174-187: unknown-vendor warning discarded; ADR D2 and settings.md:110-111 unmet. [P5]
- src/fetch.rs:627-636: settings `--vendor` / `path` refusals untested. [P5]
- src/command/release.rs:574, src/oci/annotations.rs:791: third copy of JSON-layer release / annotation body. [P5]

### Warn — tests and evidence
- test/tests/test_hooks_port_contracts.py:232: settings env-inertness checked through the file key only. [CW]
- test/tools/p9_diff.py:71-74: no `element` predicate for `update`'s `abandoned_entries`. [P-G]
- test/tests/test_old_grim_settings.py:116-129: requires-edge test cannot fail on anything settings-specific. [P-G]
- test/tests/test_old_grim_settings.py:113, :138-139, :172-175: three assertions cannot fail. [P-G]
- test/tests/test_golden_pre_settings.py:370-397: no offline state-bytes comparison (C-016). [P-G]
- Evidence gap: golden and old-grim tests skipped at the integration gate (C-016, C-072). [P-G]
- test/tests/test_guide_dependencies.py:72: number of compared outputs unpinned. [P0]
- test/tests/test_settings_real_harness.py:108-112: two checks cannot fail. [P-X]
- test/manual/README.md:895-896: says prompt `y` writes no record; it records consent. [P-X]
- test/manual/README.md:914-915: promised Codex warning never fires (clients = ["claude"]). [P-X]
- C-065 `docs:check` evidence missing. [P9]

### Warn — file-set scope
- src/install/path_anchor.rs:771 (47da12f): outside PF's Expected files. [PF]
- b48dc0b: edits four src/install files outside P-X's file set under a `feat:` subject. [P-X]
- bc273fc, 3081b1a: outside P9's Expected files. [P9]

### Warn — docs and AI config
- docs/src/content/docs/guides/dependencies.md:246-248: `grim remove` does not remove the server's config entry. [P0]
- docs/src/content/docs/guides/dependencies.md:20: prerequisites omit client setup; generic `agents` client declines agents and MCP. [P0]
- docs/src/content/docs/guides/dependencies.md:147-148: "`grim why` answers from the lock alone" is imprecise. [P0]
- docs/src/content/docs/guides/dependencies.md:9, :188-189: Google Drive not hyperlinked. [P0]
- docs/src/content/docs/settings.md:324: Cursor XDG fallback is Linux-only. [P9]
- docs/src/content/docs/settings.md:349-351: not-registered cause omits links that resolve out of bounds (CWE-59, by reference). [P9]
- docs/src/content/docs/json-interface.md:646: "each cause maps to exactly one state" contradicts :677-681. [ux]
- docs/src/content/docs/configuration.md data layout and GRIM_HOME row (:1352): settings record path undocumented. [docs]
- docs/src/content/docs/configuration.md env table: no CURSOR_CONFIG_DIR row. [docs]
- docs/src/content/docs/configuration.md editor-schema table: settings schema missing. [docs]
- docs/src/content/docs/publishing.md:875: settings omitted from kinds that declare dependencies. [docs]
- docs/src/content/docs/hooks-troubleshooting.md:37, guides/agent-hooks.md:91, commands.md:550: still teach `--trust-hooks` as primary (same class as decided upgrading.md:490). [docs]
- docs/src/content/docs/commands.md:96: declaration-table list omits `[settings]` (and pre-existing `[hooks]`). [docs]
- .claude/rules/subsystem-cli-commands.md:41: publish order omits settings. [P10]
- catalog/skills/grim-usage/references/consume.md:681: no grim 0.16.0 floor. [P10]
- .agents/upstream-checks.md:17 (S-030): deferred upstream check. [P10]

### Suggest
106 as tagged across 22 seat reports (not deduplicated); see review/out-*.md.

### Convergence
Not appended (orchestrator's decision). Fold-Back not performed (verdict is not Approve).
