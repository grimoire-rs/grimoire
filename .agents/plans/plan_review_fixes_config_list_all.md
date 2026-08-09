# Plan: Apply max-tier review findings — feat/config-list-all

## Status

- **Plan:** plan_review_fixes_config_list_all
- **Active phase:** 5 — Commit (complete)
- **Step:** finalized
- **Last update:** 2026-07-12 (after 48a3878: docs: document config list --all, string-set metadata, and validation)

## Context

`/swarm-review max` on `feat/config-list-all` (19 files, +1762/−331) returned
**Needs Work**: 0 Block, 1 High, 3 Warn, 7 Suggest actionable. Cross-model
(Codex sol) corroborated the load-time validation gap. This plan applies the
11 actionable findings. Deferred findings (wire typing, security hardening
scope, catalog audit commit, ADR refresh) are explicitly OUT of scope —
human judgment items.

- **Tier:** max (user-forced)
- **Scope:** small — mechanical fixes + one behavioral gap
- **Reversibility:** Two-Way Door (all changes on feature branch, pre-1.0)
- **Subsystems Touched:** CLI (src/command), config (src/config), install
  (src/install — test only), api (src/api — test fixtures only), acceptance
  tests, docs

## Findings → Work Items

### W1 — rust-validation (worker-builder, opus)
Files: `src/command/config.rs`, `src/config/project_config.rs`, `test/tests/test_config.py`

1. **[Warn, Codex+arch]** Load-time `options.clients` validation: add
   `validate_clients` beside `validate_tree_separators` in the config
   load/validation path (`project_config.rs`) — reject unknown (not in
   `ClientTarget::VALUE_NAMES`), blank, and duplicate entries as the same
   typed config error class tree_separators uses. Reuse the SAME shared
   validation from `apply_set` (config.rs Clients arm) so set-time and
   load-time cannot drift. Keep set-time exit 65 semantics.
   Failing tests FIRST (bugfix discipline): acceptance tests hand-author
   project + global TOML with `clients = ["vscode"]` and
   `clients = ["claude","claude"]`, assert clean typed error (not a panic,
   not silent success) on any command that loads config (`grim config list`).
2. **[Suggest, quality]** Align unknown-client message to the
   parse_default_view template: `"invalid value for options.clients: '{c}'; valid values: claude, opencode, copilot"`.
   Update unit + acceptance assertions accordingly.
3. **[Suggest, quality]** Duplicate-client message gains remediation hint:
   `"...duplicate client '{c}'; each client may appear once"`.
4. **[Suggest, testcov]** `collect_entries` assertion: `expand_levels = 0`
   emits a set row (value "0", set true) — no false-is-unset collapse for u32.
5. **[Suggest, testcov]** Pin `parse_default_view` error text in
   `parse_default_view_valid_and_invalid`: assert contains
   `"valid values: flat, tree"`.

### W2 — rust-resolved (worker-builder, opus)
Files: `src/config/resolved.rs`, `src/tui/app.rs`, `src/command/tui.rs`

6. **[Warn, arch]** Slim `ResolvedOptions` to the 4 fields that receive real
   TUI display defaults (`default_view`, `group_by_type`, `tree_separators`,
   `expand_levels`). Drop dead `clients`/`default_registry`/`show_deprecated`
   fields — bind them `: _` in the (still exhaustive, still no-`..`)
   destructure so the compile tripwire survives. Reword module doc:
   "single place TUI display defaults are applied" (kill the false global
   claim). Fix ripples in `TuiContext` construction/tests.
7. **[Suggest, testcov]** `resolved_keeps_explicit_zero_expand_levels`:
   `Some(0)` → `resolved().expand_levels == 0`.

### W3 — rust-drift-tests (worker-tester, sonnet)
Files: `src/command/config_keys.rs`, `src/install/client_target.rs`, `src/api/config_report.rs`

8. **[Warn, testcov]** `registry_field_completeness_matches_registry_config`:
   serialize fully-populated `RegistryConfig`, drop the alias selector key,
   assert remaining field set == `RegistryField::ALL` spec keys (mirror
   `config_options_completeness_matches_config_key_all`).
9. **[Suggest, arch]** `client_target.rs`: test `VALUE_NAMES.len() == ALL.len()`
   and each `VALUE_NAMES` entry round-trips through `FromStr` to the matching
   `ALL` variant.
10. **[Suggest, quality]** Fix 2 `ConfigListReport` render-test fixtures typing
    clients as `StringList` → `ValueType::StringSet { values: ClientTarget::VALUE_NAMES, default: None }`.
11. **[Suggest, testcov]** Pin typed-default metadata:
    `TuiTreeSeparators.spec().value_type.default_str() == Some("/")` and
    `TuiExpandLevels == Some("1")` (in config_keys.rs tests).

### W4 — docs (worker-doc-writer, sonnet)
Files: `docs/src/commands.md`

12. **[High, docs]** `commands.md:93`: add `options.tui.tree_separators` to the
    false/empty-collapse key list.
13. Sync any doc wording quoting the old unknown-client message shape (grep
    docs/ for "unknown client"; align with W1's new template).

## Phases (adapted max: fix-pass, not greenfield)

1. **Fix waves** — W1–W4 parallel (file-disjoint). W1 writes failing tests
   before the load-time validation lands (test-first gate for the one
   behavioral change).
2. **Gate** — `task --force rust:verify`, targeted acceptance
   (`test_config.py`, `test_json_interface.py`), then full `task verify`.
3. **Review round** — Stage 1 only (spec-compliance vs THIS findings list +
   test-coverage) + targeted architect check of the ResolvedOptions slim-down.
   Full Stage 2 panel deliberately NOT repeated: it just ran on the base diff;
   fix delta is small. Deviation from tier-max default, surfaced at gate.
4. **Codex one-shot** — mandatory at max: `code-diff --base main --model sol`
   on the amended branch diff. Actionable → one opus fix pass; failure →
   revert + defer.
5. **Commit** — conventional commits, never push:
   - `fix(cli): validate options.clients at config load and align messages`
   - `refactor(config): slim ResolvedOptions to defaulted TUI fields`
   - `test(cli): drift guards for registry fields, client names, typed defaults`
   - `docs: note tree_separators false/empty collapse under config list --all`

## Not Doing

- Deferred review findings (stringified wire contract, Cf-char/terminal-escape
  hardening, catalog audit-trail commit, ADR refresh, pre-existing doc comments)
- No push, no PR
