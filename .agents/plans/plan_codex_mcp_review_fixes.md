# Plan: Codex MCP support + max-review fixes (feat-codex-vendor round 2)

## Status

- **Plan:** plan_codex_mcp_review_fixes
- **Active phase:** 6 — complete (loop converged, Codex gate resolved)
- **Step:** finalized
- **Last update:** 2026-07-17 (after 3328de5: feat(install): add Codex (OpenAI Codex CLI) as a client vendor)

## Classification

- **Tier:** high (explicit user arg)
- **Scope:** Medium — install subsystem + docs + tests
- **Reversibility:** One-Way Door Medium (new dep `toml_edit`, new anchor
  row `·codex·mcp`, MCP entry state surface)
- **Overlays:** builder=sonnet, loop-rounds=3, review=full, codex=on (terra)

## Goals (user directive)

1. Apply actionable findings from the max-tier review of feat-codex-vendor.
2. MCP artifacts supported for **all 4 clients** — add Codex MCP target.
3. MCP installs **idempotent**: artifacts rendering into a shared config
   file must never register an entry twice; re-install replaces in place.

## Component contracts

### C1 — Codex MCP registration (new)

- Target files: global `$CODEX_HOME/config.toml` (`CodexRoot` anchor,
  relative `config.toml`); project `<workspace>/.codex/config.toml`
  (`Workspace` anchor). Managed member: `mcp_servers.<name>` table.
- Mechanism: **span-preserving TOML splice** — new module
  `src/install/toml_splice.rs` mirroring `json_splice.rs` semantics
  (every byte outside the managed member survives: key order, formatting,
  comments). Use `toml_edit` crate (format-preserving; cargo-standard;
  add to Cargo.toml). The plain `toml` crate cannot preserve spans.
- Entry derivation: keep `Vendor::mcp_entry` JSON-typed (single source of
  truth); convert `serde_json::Value` → `toml_edit::Item` at splice time.
  Integrity stays the existing semantic canonical-JSON hash — unchanged
  and identical across clients.
- Field mapping (upstream schema, learn.chatgpt.com/docs/extend/mcp):
  stdio → `command`/`args`/`env`/`cwd`; HTTP → `url`. Env-ref
  descriptors: **DECIDED (orchestrator, Specify phase): skip with
  warning, Copilot precedent** — no verified evidence Codex substitutes
  `${VAR}` in config.toml; conservative default, Two-Way Door, revisit
  on upstream evidence. Implement adds the dedicated skip test.
- `CodexVendor::mcp_config_path` returns `Some(...)` for both scopes;
  `(Codex, Mcp)` anchor arm becomes real (`CodexRoot` / config.toml),
  removing that `unreachable!()`. Docs note: project `.codex/config.toml`
  only honored by Codex for trusted projects (upstream trust gate).
- Uninstall: remove only `mcp_servers.<name>`; never delete the file;
  leave unrelated `mcp_servers.*` and all other config untouched.
  **Arch-verify gap:** `uninstall.rs::remove_entry` is hardcoded to
  json_splice; a Codex TOML entry would hit the JSON scanner's
  InvalidData → tolerant no-op → entry orphaned silently. Implement must
  add client-aware format dispatch (`out.client` available in calling
  loop); Specify must pin a failing uninstall test for the TOML path.

**Implement-phase deviation (env-ref skip reconciled with the pinned
Specify test):** the DECIDED note above ("skip with warning" for any
env-ref descriptor) conflicts with the already-pinned test
`mcp_entry_stdio_maps_command_args_env_under_mcp_servers_pointer`
(`src/install/vendor_codex.rs`), which requires a literal `${GITHUB_TOKEN}`
value in `env` to register successfully — a stdio `env` entry is an OS
environment assignment for the launched subprocess, the same literal
passthrough Claude/OpenCode already give it, not something grim or Codex
substitutes. Reconciled: `mcp_entry` does **not** skip on env refs in
`command`/`args`/`env`. The skip-with-warning precedent instead applies to
HTTP/SSE **headers** — Codex's upstream schema maps only `url` for a
remote server, no headers field at all, so a descriptor needing headers
(almost always an auth token) is skipped rather than silently dropping
required auth. Implement added the dedicated test
`mcp_entry_http_with_headers_is_skipped_codex_has_no_headers_field` for
this case. All 27 Rust + 5 pytest specification tests pass; `task
--force verify` is green.

**Stub-phase notes (implementer decisions, not deviations):**

- `toml_splice.rs` re-exports `json_splice::{Splice, split_pointer}`
  rather than duplicating them — both are format-independent (pointer
  string parsing, "did the text change" enum), so the TOML module reuses
  them verbatim (DRY).
- Format dispatch seam: added `Vendor::mcp_config_format() -> McpConfigFormat`
  (default `Json`, `CodexVendor` overrides `Toml`) rather than sniffing the
  `mcp_config_path` file extension — a trait method fits the existing
  strategy-via-traits pattern every other vendor capability already uses.
  `install_mcp`'s json_splice/toml_splice branch reads this method at
  Implement phase.
- `(Codex, Rule)` `unreachable!()` in `path_anchor.rs::candidate_anchors`
  resolved by **reusing `AnchorError::UnknownAnchor`** (no new variant): the
  match arm returns an empty candidate set for that pair at global scope,
  so `from_target`'s existing "no root matched" fallthrough produces
  `UnknownAnchor` — already handled gracefully by `convert_v1_records`
  (warns + drops the record). Project scope is untouched (still
  unconditionally `[Workspace]`) since it was never at panic risk.
- `CodexVendor::mcp_entry` is **not yet overridden** (stays the trait
  default `None`): unlike `mcp_config_path`/`mcp_config_format` (pure path
  mapping, safe to implement now), the entry field-mapping is real business
  logic out of stub scope, and stubbing it as `unimplemented!()` would turn
  today's graceful skip (`grim fetch --vendor codex <mcp-ref>` already
  returns a clean "client cannot represent this descriptor" error via
  `src/fetch.rs`) into a panic on that existing call path. Left for
  Specify/Implement to add together with the real mapping.

### C2 — MCP idempotency (all 4 clients)

- Contract: install → entry present exactly once; second install (same
  or changed artifact) → replace in place, no duplicate key, no growth;
  byte-stable when entry value unchanged. Applies to shared configs
  (opencode.json also carries rules glob; .codex/config.toml carries
  arbitrary user config).
- Verify existing JSON-splice behavior with acceptance tests (may
  already hold — prove it); implement same guarantee in toml_splice.

**Implement-phase gap found (not in the original C1/C3 list):**
`InstallState`'s entry read-back path (`ClientOutput::current_hash`/
`is_present` → `read_entry_value` in `install_state.rs`) was hardcoded to
`json_config::parse_object`, so a repeat Codex install always read the
managed entry back as "absent" (JSON parser on a TOML file), never
short-circuited the integrity gate to `AlreadyInstalled`, and reported
`updated` on every reinstall — failing the pytest idempotency acceptance
test `test_global_codex_registers_entry_in_config_toml_idempotent`.
Fixed by threading `ClientOutput::client` → `Vendor::mcp_config_format`
through `read_entry_value`/`current_entry_hash`, dispatching to
`toml_splice::member_value` for Codex the same way `install_mcp` already
does. This is the same format-awareness fix as C1's uninstall gap, just
on the read side rather than the write side.

### C3 — Review-finding fixes (actionable set from max review)

1. docs/src/agents.md:99 emit-matrix Codex provenance cell → `yes (TOML #)` (Block).
2. V1-migration panic: `candidate_anchors` declined pairs return typed
   `AnchorError` (or `convert_v1_records` skips) — no `unreachable!()`
   reachable from persisted state. `(Codex,Mcp)` arm becomes real via C1;
   `(Codex,Rule)` arm needs the typed-error path.
3. Fetch-before-gate: compute supporting-client set before fetch/unpack;
   empty → record zero-output, outcome `Skipped`, no artifact access
   (installer.rs:403–491). **Arch-verify gap:** the gate must account
   for pin-change reattachment (installer.rs:467–483 re-adds
   previously-recorded clients outside the current `--client`
   selection): `effective_supporting_clients` takes the prior record as
   input; a narrowed selection at a new pin must NOT skip the fetch and
   strand recorded clients at the old pin. Regression test required.
4. Kind-aware `supporting` predicate (installer.rs:198) — use
   `mcp_config_path` for MCP (largely superseded by C1 for Codex, still
   fix the predicate so future surfaceless vendors report honestly).
5. Provenance newline guard: single-line invariant in provenance
   builders (`toml_provenance` + shared `provenance`), reject/escape
   newline in `pinned`.
6. `codex.reasoning-effort` enum → align to current upstream native set
   `ultra|max|high|medium|low|minimal|none` (drop `xhigh` — Claude-ism;
   pre-release, no compat burden). Update docs/src/vendor-metadata.md +
   catalog agent-spec.md; extend registry↔docs parity test to `codex.*`.
7. Vacuous test fix: install_report.rs:132 → `v["items"][0]["target"]`.
8. docs/src/json-interface.md: `target` nullable note + `codex` in
   clients example; mcp-servers.md Codex row; catalog
   grim-usage/references/registries.md 4 clients; agents.md:179 +
   agent-spec.md:90 "exactly one <name>.md" reword; CHANGELOG
   "warns and skips (install still succeeds)" reword + MCP entry.
9. Test gaps: TOML emitter escaping matrix (quotes/backslash/multiline/
   CRLF/empty body), mixed-selection stderr quietness, prior-output
   Skipped ordering, CODEX_HOME global detection, zero-output
   uninstall/prune, `.agents/skills` in ALL-fallback assert.

### Deferred (explicitly NOT this run)

- `KindSupport` tri-state trait redesign (ADR amendment follow-up).
- Binding-name charset validation (pre-existing, all vendors).
- Anchor-tag rename decision (`codex-root`) — human call, still open.
- Codex-only-MCP hard-fail vs skip policy beyond what C1 obsoletes.

## Subsystems Touched

src/install (vendors, installer, anchors, splice), src/api, docs/src,
catalog skills, test/tests, Cargo.toml.

## UX scenarios (acceptance)

- `grim install --client codex` with mcp artifact → entry in
  `$CODEX_HOME/config.toml`, JSON report target non-null, status clean.
- Repeat install ×2 (each client) → exactly one entry, config byte-stable.
- Config with pre-existing user comments/keys → untouched outside entry.
- Uninstall → entry gone, file + foreign keys remain.
- V1 state with codex rule record → lossy-migration warning, no panic.

## Phases

1. **Stub** — toml_splice module skeleton, CodexVendor mcp overrides,
   anchor arm, AnchorError variant, installer gate reorder signatures.
2. **Verify Arch** — spec-compliance vs this plan + ADR.
3. **Specify** — failing tests for C1/C2/C3 contracts + UX scenarios.
4. **Implement** — fill bodies; docs fixes (C3.1, C3.8) alongside.
5. **Review-Fix Loop** — ≤3 rounds, full breadth.
6. **Codex gate** — terra, one-shot, then commit.

## Round-2 review-fix notes

Round-1 review found 2 Block + several Warn; all fixed minimally, each
backed by a failing-without-fix test.

- **Block — provenance comment-breakout (CWE-116):** `vendor.rs::single_line`
  now also escapes `<`/`>` → `&lt;`/`&gt;` so a literal `-->` (or any tag) in
  `pinned` cannot close the HTML `<!-- ... -->` comment early. Uniform across
  the HTML and TOML provenance builders (harmless in the `#` TOML variant).
- **Block — report lies on narrowed-selection + pin change:**
  `installer.rs::install_all_with_progress` now derives the report target and
  the decline warning from `effective_supporting_clients` (the SAME set
  `install_one` uses), so a `--client codex` pass at a new pin that
  re-materializes a prior Claude output reports Claude's path, not `None`.
- **Warn 4 decision — non-finite TOML float:** chose **preserve-as-string**
  over fail-loud. `member_value`/`toml_item_to_json` are a *tolerant* read
  path (`None` == absent/unparseable); turning a hand-authored `nan`/`inf`
  into a hard error would make that path fail and ripple through 5 functions
  + 2 external call sites that consume `Option`. Preserving the value as its
  lexical string keeps the field visible to the clobber gate + integrity hash
  (the actual bug — silent drop) with a one-arm change. JSON has no NaN, so a
  string is the only faithful in-`serde_json::Value` representation.
- **Warn 5 keyword:** `ClientOutput::mcp_format` exposed as `pub` (not the
  literally-requested `pub(crate)`) to match the struct's sibling methods
  (`current_hash`/`is_present`/`resolved_target` are all `pub`) and
  `quality-rust.md`'s "prefer `pub` over `pub(crate)`" rule; identical
  reachability in this binary-only crate.
