# Meta-Plan: /swarm-execute max — apply review findings (feat/vendor-capability-unlock)

## Classification

- **Tier:** max (user-explicit). Deviation from tier-max's "requires plan
  artifact": the review report IS the design record — a fresh
  `/swarm-plan max` for an already-adversarially-reviewed fix queue would
  re-derive what 8 reviewers + Codex just produced. Contract-first TDD
  kept per finding (failing test → fix).
- **Target:** review verdict Request Changes — 1 Block, 7 Warn, 8 Suggest.
- **Branch:** feat/vendor-capability-unlock (continue on it).

## Overlays

| Axis | Value | Source |
|---|---|---|
| builder | opus | max mandatory |
| tester | opus | max mandatory |
| reviewer | opus | adversarial breadth escalation |
| loop-rounds | 3 | max default |
| review | adversarial (scaled to fix-diff) | max default |
| codex | on (one-shot, post-loop) | max mandatory |

## Commit plan (Conventional Commits, test-first per fix)

1. `fix(install)`: **B1** — pin-change reconciliation: prior-tracked client
   whose new `mcp_entry` is `None` gets its stale config entry
   splice-removed + warn. Failing unit test first (pin change http→oauth,
   assert entry removed from config + absent from record). installer.rs:1146/1247.
2. `fix(install)`: **W6** — reaper resolved-identity guard: skip reap when
   canonicalized old path == any canonicalized new output (symlink alias).
   Failing test with symlink fixture first. + **W1** two support-dir reap
   tests (deletes moved support dir / preserves edited one). installer.rs:919-951.
3. `fix(vendor-codex)`: **W7** — duplicate case-insensitive header names:
   fail-closed skip+warn (descriptor unrepresentable) — NOT validation
   tightening (would reject already-published artifacts; doctrine).
   + **S1** lowercase-`authorization` bearer test.
4. `fix(mcp)`: **S3** — chain `oauth.auth_server_metadata_url` into
   `string_values()` + rejection test (mcp.rs:346-361).
5. `refactor(vendor-opencode)`: **W2** — `_scope` → `scope`.
   `refactor(vendor-codex)`: **S7** — extract `classify_codex_headers`
   (behavior-preserving, tests unchanged).
6. `test(install)`: **S2** Copilot warn/no-warn assertions, **S5**
   `output_at_current_layout` direct unit test. `test(vendor-copilot)`:
   **S4** `must_document_copilot_registry` parity test (may need doc-row
   sync in same commit — parity discipline).
7. `docs`: **W3** agents.md:94 Copilot model "kept", **W4** artifacts.md:469
   `allowed-tools`, **S8** stability.md names deny-unknown-fields as
   deliberate departure from cargo/npm/helm ignore-unknown norm.
8. `chore`: **W5** watchlist rows — `.agent.md` row → refs issue #44
   (upstream settled, live-verify precondition); Copilot MCP env-sub row →
   correct product citation (CLI doc; note v0.0.406→407 regression
   copilot-cli#1403); Codex oauth row → note native `auth` enum surface.
   `installer.rs:877` doc-comment wording (**S6**) rides commit 1 or 2.

## Workers per phase

- Specify+implement pairs per commit group: worker-tester (opus) →
  worker-builder (opus); docs/watchlist via worker-doc-writer (sonnet);
  refactors via worker-builder (sonnet — mechanical, Two-Hats).
- Review-Fix Loop on the cumulative fix-diff (base = pre-fix HEAD
  338a986): Stage 1 spec-compliance + test-coverage (opus); Stage 2
  scaled adversarial — quality, security (B1/W6 touch deletion+config
  writes), architect (reconciliation seam design); ≤3 rounds.
- Codex one-shot gate on fix-diff; actionable → one opus builder pass →
  `task verify`; fail → revert pass, promote to deferred.

## Gates

`task rust:verify` per commit; commit-gate marker refreshed in separate
prior Bash call (hook timing); full `task verify` + `task catalog:verify`
final. Never push.

## Decision knobs baked in (flag at approval if wrong)

- **W7 = fail-closed skip, not validate-reject** (compat doctrine: no
  tightening against published artifacts).
- **Renderer-version mechanism NOT in this pass** — tracked as issue #44
  (own plan cycle; One-Way Door on state schema).
- D-c1 (partial multi-client write rollback) stays deferred — pre-existing,
  needs own design pass; B1 fix scoped to pin-change decline only.

## Not Doing

- No push, no PR. No FieldType::Json work. No `.agent.md` renderer change
  (issue #44). No installer-wide async-fs migration (D-q1).
