# Plan: review-fixes — description companion v2 max-tier findings

## Status

- **Plan:** plan_review_fixes_companion_v2
- **Active phase:** 1 — Remediation (single-phase plan)
- **Step:** finalized
- **Last update:** 2026-07-12 (after 42b96f4: chore(assets): refresh project logo — branch finalized 14→9)

## Handoff

- **Tier:** max
- **Scope:** Large (remediation batch from max-tier /swarm-review of PR #33)
- **Reversibility:** Two-Way Door fixes on an unmerged branch; B1 is a
  behavior tightening (previously-escaping paths now error) — CHANGELOG
  note, pre-1.0, no compat shim
- **Overlays:** builder=opus (mandatory), tester=opus (mandatory),
  reviewer=opus (adversarial breadth), loop-rounds=3, codex=on
  (sol, **base=bd6f1ee** — fix delta only; main..bd6f1ee already passed a
  Codex sol gate this session)
- **Subsystems Touched:** CLI shell/API (`src/command/{publish,fetch,release}.rs`),
  fetch core (`src/fetch.rs`), OCI (`src/oci/description.rs`), API reports
  (`src/api/publish_report.rs`), acceptance tests (`test/tests/**`), docs
  (`docs/src/**`), artifacts (ADR/handover one-liners)

Source of truth for findings: max-tier review verdict (this session).
Review baseline snapshot: `bd6f1ee` on `feat/vscode-extension-api`.

## Embedded policy decisions (defaults chosen; approval covers them)

1. **Containment = reject.** `[description]` paths and `include` glob hits
   must resolve inside the manifest directory. Out-of-tree (`..`,
   absolute, symlink escape) → publish-time data error (65). Monorepo
   parent-README becomes a possible future opt-in flag — **Not Doing** now.
   (Rationale: ADR says "paths relative to publish.toml"; install side
   already enforces containment; rev-security + Codex sol converged.)
2. **`--vendor` with `--description` → usage error (64).** Reconciles ADR
   "Risks" with the shipped gate set (cheapest consistent resolution;
   matches the three existing gates).

## Findings → work items

### P0 — Block (both mandatory)

| ID | Item | Anchors |
|---|---|---|
| B1 | Path containment for `[description]`: reject absolute + non-`Normal` components pre-traversal; canonicalize-and-contain every selected file (explicit paths AND glob hits, symlink targets included) under canonical manifest dir → 65 with clear message. Regression tests: `../` path, absolute path, symlink escape, `include=["../**/*.env"]`. | `src/command/publish.rs:1601-1817` (`resolve_description_spec`, `require_desc_path`, `expand_description_glob`) |
| B2 | Offline parity for fetch: digest resolution for `grim fetch` entry paths (content + `--description`) uses `Operation::Resolve` so offline-uncached → 81 (matches ADR:182, `fetch.rs:616` doc, commands.md:558). Scoped — do NOT flip other `fetch_artifact` callers blindly; audit each (install/render paths keep their semantics). Acceptance tests: `--offline` uncached `fetch`, `fetch --description`, `fetch --digest-only` → 81 envelope; cached variants unchanged. | `src/fetch.rs:324` (`Operation::Query`), `src/oci/access/cached_access.rs:70-73` |

### P1 — Warn batch

| ID | Item | Anchors |
|---|---|---|
| W1 | Reserved-tag write guard: single `validate_user_tag` seam rejecting `__grimoire` / `__grimoire.<x>` (64) on every user-supplied-tag write path — `grim release` target ref, publish cascade/channel values. Tests per surface. | `src/command/release.rs` (no guard today), `src/command/publish.rs:417-438` (`is_reserved_float_tag` precedent), `src/oci/description.rs:41` |
| W2 | Pre-pack companions before entry push loop: read + bounds-check + build tar for every planned companion BEFORE first registry mutation; keep bytes for post-loop push. Bad companion → abort, zero pushes. Test: invalid/oversized companion ⇒ no artifact pushed. | `src/command/publish.rs:811-847`, `push_one_description` |
| W3 | `EntryDescription` custom `Deserialize` on TOML value type (bool → `Enabled`, table → `Spec` with full error passthrough — precise `unknown field` messages). Malformed per-entry input tests. | `src/command/publish.rs:186-231` |
| W4 | Extract glob engine (`expand_description_glob`, `glob_segments`, `wildcard_match`, `read_dir_sorted`, ~110 lines) into its own module; add ceiling comment (`[...]`/`{...}` unsupported; `globset` upgrade path). Pure move + comment — no behavior change. | `src/command/publish.rs:1706-1817` |
| W5 | `--vendor` + `--description` → 64 gate (see policy 2). Test in flag-combination suite. | `src/command/fetch.rs:73-89` |
| W6 | Unit tests for `fetch_description` 65 branches: `__grimoire` tag → non-companion manifest ⇒ DataError; empty companion tar ⇒ DataError. | `src/fetch.rs:625-639` |
| W7 | Replication caveat: single-tag copy (skopeo/oras) drops the companion; full-repo sync required. One short subsection in publishing.md; mirror line in handover doc. | `docs/src/publishing.md` |

### P2 — Cheap Suggests (fold in)

| ID | Item | Anchors |
|---|---|---|
| S1 | commands.md overview table: add `describe` row (+ pre-existing missing `fetch`) | `docs/src/commands.md:24-42` |
| S2 | Stale "UTF-8 text only" → base64-for-binary in `--path` help text (CLI + MCP schema) | `src/command/fetch.rs:42-45`, `src/mcp/tool_args.rs:90-93` |
| S3 | Desc-row status strings: reuse `PublishStatus::{Pushed,DryRun}` Display (or coupling comment) | `src/api/publish_report.rs:~248` |
| S4 | Drop dead `media_type` assignment (push path discards it) or comment it ignored | `src/oci/description.rs:76` |
| S5 | `KEYWORDS_ANNOTATION` / `SUMMARY_ANNOTATION` consts replacing bare literals (new read sites minimum) | `src/oci/annotations.rs` + callers |
| S6 | Acceptance test: top-level `[description] publish = false` ⇒ `descriptions.items == []` | `test/tests/test_desc.py` |
| S7 | Index-source `summary` reaches `row["summary"]` assert | `test/tests/test_index_source.py` |
| S8 | `has_credential` doc wording (N reads for N registries) — fix comment or parse-once | `src/auth/store.rs`, `src/command/context.rs` |
| S9 | One-liners: ADR "Risks" deferred-idea note (additive Referrers emission); handover note recommending `--out` for asset-heavy companions; json-interface.md names inline shape "GitHub Contents API style" | `.agents/adr/adr_description_companion.md`, `handover_vscode_description_api.md`, `docs/src/json-interface.md` |

## Contracts (testable)

- `plan_descriptions` rejects any spec path or glob hit whose canonical
  form is not under the canonical manifest dir → `DataError` naming the
  offending source path.
- `grim fetch <uncached> --offline` / `+ --description` / `+ --digest-only`
  → exit 81, `{error:{code:"offline-blocked",exit:81}}`.
- `grim release ./x repo:__grimoire` → exit 64 before any network.
- Publish with unreadable/oversized companion → exit non-zero, **zero**
  registry mutations (assert via registry state).
- `[skills.x.description]` with typo'd key → error message contains
  `unknown field` and the field name (not "did not match any variant").
- Glob extraction: `task rust:verify` green with zero behavior diff
  (existing 12 publish glob/spec unit tests pass unchanged).

## Edge cases (Specify phase must cover)

- Containment: symlinked file inside tree → target outside (reject);
  symlinked dir mid-glob-walk escaping; `include = ["**/../*"]`;
  manifest dir itself a symlink (canonicalize base first).
- Offline: cached-tag-uncached-manifest fetch still errors offline
  (existing 81 via fetch_manifest — don't regress); `--digest-only`
  offline CACHED ref succeeds from cache.
- W2: two entries sharing one repo — companion packed once, not twice;
  dry-run unaffected (no pack needed? pack anyway for validation parity).
- W3: `description = 1` (neither bool nor table) → clear type error.

## Not Doing

- Monorepo parent-README opt-in flag (future demand-driven)
- sync-fs in async publish planning (pre-existing pattern — deferred)
- MCP access-seam amortization (broader refactor — deferred)
- fetch_outcome request-struct, whitespace-keyword asymmetry,
  `ArtifactKind::Skill` placeholder cosmetics (deferred)
- No push, no PR mutation
