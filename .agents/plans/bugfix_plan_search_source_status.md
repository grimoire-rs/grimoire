# Bug Fix Plan: `grim search` drops a failed source silently

## Status

- **Plan:** bugfix_search_source_status
- **Active phase:** 7 — Commit & Document
- **Step:** /builder → implemented
- **Last update:** 2026-08-31 (implementation + docs complete, gate pending)

---

## Overview

**Status:** In Progress
**Author:** Michael Herwig
**Date:** 2026-08-31
**GitHub Issue:** [#108](https://github.com/grimoire-rs/grimoire/issues/108)
**Severity:** High — silently wrong data reaches a GUI consumer

## Bug Report

### Observed Behavior

`grim search --format json` drops a source it cannot read with only a
`tracing::warn!` on stderr. Exit stays `0`, `items` is silently truncated
(often to `[]`), and nothing in the envelope names the failed source. A
consumer reading stdout — the whole point of `--format json` — cannot tell
"no results" from "most of your registries did not load".

### Expected Behavior

The document names every browsed source and whether its catalog loaded, so a
consumer can say "2 of 3 registries unavailable" and name them.

### Reproduction Steps

1. Configure two `[[registries]]`, one pointing at a dead port.
2. `grim search --format json --refresh`
3. Observe: exit `0`, a plausible `items` list missing the dead source's rows,
   and no field naming it. Only stderr carries
   `catalog for source '<x>' unavailable: …`.

Reported downstream via
[grimoire-vscode](https://github.com/grimoire-rs/grimoire-vscode): a user got
an empty browse list. The extension can only guess — it marks results
"incomplete" when a successful run wrote anything to stderr, and presence of
stderr text is not a contract.

### Environment

| Factor | Value |
|--------|-------|
| Platform | any |
| Grimoire version | 0.14.0 (present since multi-registry browse) |
| Registry | any unreachable source, or an unreadable catalog cache |
| Configuration | two or more `[[registries]]` |

### Frequency

Always, whenever a configured source fails to load.

## Root Cause Analysis

### Investigation Log

1. **Symptom**: `items` short or empty, exit `0`, no field explains why.
2. **Proximate cause**: `src/command/search.rs` builds `SearchReport` from
   `CatalogResults.groups`, which carry no failure information to report.
3. **Root cause**: `src/catalog/catalog_service.rs` `load_catalog`'s fan-out
   matched each per-source load result, **logged the `Err(e)` and discarded
   it**, degrading the group to `CatalogGroup { rows: [], served_offline: true }`.
   `served_offline` is `true` both for a `--offline` browse and for a failed
   source, so nothing downstream could recover the difference.
4. **Introduced by**: original implementation of the shared multi-registry
   browse seam (`adr_multi_registry_mcp.md`); the per-source degrade is
   deliberate and correct — dropping the *cause* is the defect.

### Root Cause Statement

> A source's rows go missing because `load_catalog` discards the load error
> after logging it, so every consumer of the shared browse seam sees a failed
> source and an empty one as the same thing.

### Related Code

| File | Lines | Role |
|------|-------|------|
| `src/catalog/catalog_service.rs` | fan-out `match result` | Root cause: `Err(e)` logged, then dropped |
| `src/catalog/catalog_service.rs` | `CatalogGroup` | Had no field to carry the cause |
| `src/command/search.rs` | report construction | Where the symptom surfaces |
| `src/api/search_report.rs` | `SearchReport` | The envelope with nowhere to put it |

### Pattern Check

- [x] Same root cause reaches every consumer of the seam — `search`, `tui`,
      `status --check`, `rate`. Fixed once, at the seam.
- [x] Not a regression from a recent change; present since the seam landed.
- [x] The join-failure arm had the same shape — an absent map entry must not
      read as a healthy group. Covered by `TASK_FAILED`.

## Regression Test Specification

> Written BEFORE the fix. All five acceptance tests failed on the pre-fix
> binary with `KeyError: 'sources'`.

### Unit Tests

| Test | File | Asserts |
|------|------|---------|
| `per_registry_failure_degrades_to_empty_group_in_input_order` | `src/catalog/catalog_service.rs` | extended: a failed group carries `error.is_some()` |
| `a_source_that_loaded_carries_no_error` | `src/catalog/catalog_service.rs` | a healthy (empty) source carries `error.is_none()` — so `is_some()` is a sound discriminator |
| `json_carries_source_status_plain_table_does_not` | `src/api/search_report.rs` | `sources` shape, always-present-null `error`, item shape still 15 fields, plain table still 5 columns |
| `empty_results_serialize_as_empty_items` | `src/api/search_report.rs` | `{"items": [], "sources": []}` |

### Acceptance Tests

| Scenario | File | Steps |
|----------|------|-------|
| partial failure names the failed source | `test/tests/test_registries.py` | two registries, one dead port ⇒ `good` `ok:true`, `bad` `ok:false` + error, exit 0 |
| total failure ≠ empty catalog | `test/tests/test_registries.py` | every source dead ⇒ `items: []`, every source `ok:false`, exit 0 |
| healthy source always reported | `test/tests/test_search.py` | one reachable registry ⇒ one `ok:true` / `error:null` entry |
| unreadable cache is named | `test/tests/test_search.py` | corrupt `$GRIM_HOME/catalog/*.json`, browse `--offline` |
| failed index source is named | `test/tests/test_index_source.py` | unreachable `index =` locator |
| MCP parity holds | `test/tests/test_mcp.py` | `test_mcp_search_tool_matches_cli_json` unchanged and passing |

## Fix Approach

### Proposed Change

Carry the error at the seam; render it in `search`'s envelope.

### Files to Modify

| File | Change |
|------|--------|
| `src/catalog/catalog_service.rs` | `CatalogGroup.error: Option<String>`; fan-out task returns `Result<Catalog, String>`; join failure resolves to `TASK_FAILED`; both group arms set `error` |
| `src/api/search_report.rs` | `SearchSourceStatus {alias, locator, ok, error}`; `SearchReport.sources`; `new(items, sources)` |
| `src/api.rs` | re-export `SearchSourceStatus` |
| `src/command/search.rs` | project groups → `sources` before the flatten consumes them |
| `src/tui/app.rs` | two test fixtures gain `error: None` (production never builds a `CatalogGroup`) |
| `docs/src/{json-interface,commands,stability}.md` | contract text, incl. rewriting the "no field is planned on `search`" paragraph |
| `catalog/skills/grim-usage/references/registries.md` | first-party skill drift review |

### Alternatives Considered

| Approach | Rejected Because |
|----------|-----------------|
| Non-zero exit when every source failed | `search`'s exit-0-on-browse is a documented, frozen contract (Principle 9), and an exit code cannot name *which* source failed. Owner decision: envelope only. |
| Split `served_offline` into a state enum | Three shipped consumers read that bool; a refactor mixed into a fix violates the Two Hats Rule. `error` is additive beside it. |
| Parse the stderr warning downstream | What the VS Code extension does today; tracing text is not a contract. |
| Also emit `rows` / `truncated` / `rows_before_filter` | Same class of stderr-only signal, but not what #108 reports. Owner picked the four-field shape. Follow-up if wanted. |

### Risk Assessment

| Risk | Mitigation |
|------|------------|
| Frozen JSON contract | Additive sibling key on an existing envelope — the sanctioned shape (`publish.announce`, `status.checked`). Item shape guarded by the untouched 15-field assertions. |
| A join-failed group reading as healthy | `TASK_FAILED` const; absent map entry resolves to an error, never `None`. |
| Consumers relying on the stderr line | Left byte-identical, so the extension's current heuristic keeps working during migration. |
| MCP drift | `grim_search` delegates to `search::run`, so it inherits the key; parity test passes unchanged. |

## Verification Checklist

- [x] Regression tests failed on the pre-fix binary (`KeyError: 'sources'` ×5)
- [x] Fix applied — all five acceptance tests and four unit tests pass
- [x] Search-adjacent acceptance suites pass (143 tests)
- [x] `task catalog:verify` passes
- [ ] `task verify` (full gate)
- [x] No scope creep — TUI / `status --check` / `rate` gain the field but render nothing new
