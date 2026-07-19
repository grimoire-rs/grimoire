# ADR: User-facing client compatibility matrix with code-enforced freshness

## Metadata

**Status:** Proposed
**Date:** 2026-07-19
**Deciders:** maintainer (architect proposal)
**Beads Issue:** N/A
**Related PRD:** N/A
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md` (no new tech; mdBook page + Rust unit test)
**Domain Tags:** integration
**Supersedes:** N/A

## Context

grim supports 4 clients × 5 artifact kinds, with a mix of full support,
partial support (MCP ws/oauth/env-ref limits), and explicit declines
(Codex rules). The truth lives in code (`Vendor::supports_kind`, the
per-vendor `mcp_entry` capability checks) and is documented **fragmentarily**:

- `docs/src/artifacts.md#vendor-extensions` — vendor *key* registry only
- `docs/src/agents.md#emit-matrix`, `docs/src/mcp-servers.md#emit-matrix` —
  per-kind projection tables, **no structural test** (can drift silently)
- Rule rendering per vendor — full truth only in the internal AI-config rule
  `subsystem-file-structure.md`, not on the docs site
- `vendor-capability-watchlist.md` declines — 10 of 11 rows internal-only;
  a user hitting a decline sees a runtime warning with no documented rationale
- `compatibility:` frontmatter field — free-text hint sitting next to real
  enforced matrices, never disclaimed; readers can believe
  `compatibility: codex` makes an artifact Codex-compatible

No single page answers "which client gets which artifact kind, and why not."
[`adr_vendor_wave_expansion.md`](./adr_vendor_wave_expansion.md) grows the
matrix from 16 to 40 cells — hand-maintained fragments will not survive that.

An enforcement pattern already exists: `docs_reference_matches_<vendor>_registry`
tests (vendor_claude.rs:394, vendor_copilot.rs:370, vendex_codex.rs:938) read
`docs/src/vendor-metadata.md` at compile time via
`concat!(env!("CARGO_MANIFEST_DIR"), ...)` and assert bidirectional set
equality between documented backtick tokens and the `KnownField` registries.
Gap in the pattern itself: OpenCode has a non-empty agent registry and a
documented section but **no parity test**.

## Decision Drivers

- Principle 9: docs are contracts — a support matrix that can silently lie is
  worse than none.
- Vendor expansion multiplies drift surface 2.5× imminently.
- Existing compile-time doc-parity discipline is proven and cheap; a second
  mechanism (doc generation pipeline) would be new infrastructure.
- Declines must be *fair*: documented user-facing with rationale, not just a
  runtime warning.

## Industry Context & Research

spec-kit (34+ integrations) maintains
`docs/reference/integrations.md` as a hand-curated registry table derived
from a single `INTEGRATION_REGISTRY` in code — but has no test tying the
two together, and its docs have drifted before (release-asset pipeline
docs outliving the pipeline). grim's compile-time parity-test discipline is
the stronger mechanism; this ADR extends it rather than importing anything.

**Research artifact:** [`research_spec_kit_rendering.md`](./research_spec_kit_rendering.md); session recon 2026-07-19 (vendor map, docs sweep).

## Decision Outcome

**Chosen Option:** Option A — hand-written matrix page + table-parity Rust test.

### 1. New docs page `docs/src/clients.md`

One matrix table: rows = clients (`ClientTarget::ALL` order), columns =
`Skill | Rule | Agent | MCP`. Cell legend:

- `✓` supported (native or transform)
- `◐` supported with documented limitation (footnote link, e.g. MCP
  ws/oauth declines, Copilot global env-ref skip, unscoped-rule degradation)
- `✗` declined (link to a "Known gaps" entry with the rationale)

Bundle is a footnote, not a column (decomposes to member kinds).
Below the matrix:

- **Known gaps** section — the user-relevant `vendor-capability-watchlist.md`
  rows promoted user-facing: rationale + upstream tracking pointer each.
  The watchlist rule stays the internal working document; the docs section is
  its published projection.
- **`compatibility:` disclaimer** — explicit statement that the frontmatter
  field is a free-text editor/runtime hint with zero effect on grim's
  per-vendor rendering, and that this matrix is the enforced truth.
- `SUMMARY.md` entry; concrete path tables in `agents.md`/`mcp-servers.md`
  gain a one-line pointer to `stability.md#unstable` (paths are not contract).

### 2. Table-parity test

One Rust unit test (co-located in `src/install/client_target.rs` tests,
beside `ALL`): reads `docs/src/clients.md` at compile time, parses the first
markdown table (line-starts-with-`|` split; small bespoke parser — the
existing tests scan backticks, tables are new), and asserts for every
`(client, kind)` in `ClientTarget::ALL × [Skill, Rule, Agent, Mcp]`:

- documented `✗` ⇔ `!vendor.supports_kind(kind)` (for MCP:
  `mcp_config_path` returning `None`)
- documented `✓`/`◐` ⇔ supported in code
- row set == `ALL` exactly (new vendor without a matrix row fails the build,
  removed vendor with a stale row fails the build)

`◐`-vs-`✓` nuance is **not** machine-checked in v1 — partial-capability
detail is not introspectable from the `Vendor` trait today. Footnotes stay
manual. If [`adr_vendor_wave_expansion.md`](./adr_vendor_wave_expansion.md)'s
`KindSupport` tri-state lands, the test upgrades to check `◐` ⇔ `Degraded`
for the rule column.

### 3. Backfill the parity-pattern holes

Same effort, same convention:

- `docs_reference_matches_opencode_registry` in vendor_opencode.rs (the
  missing fourth test).
- Emit-matrix coverage tests for `agents.md` and `mcp-servers.md`: lighter
  invariant — every `ClientTarget` name appears in each emit-matrix table
  (row-presence, not cell semantics). Catches "added vendor, forgot the
  emit matrix" without over-constraining prose.

### 4. Agent awareness: scoped rule + hook reminder (not CLAUDE.md)

The parity test is the hard gate; agents should still learn the duty
*before* a red test. Per the meta-ai-config decision tree (session-global
knowledge → CLAUDE.md only when its absence causes mistakes everywhere;
file-scoped duty → scoped rule + hook), the awareness lands as:

- **`docs-style.md` rule addition** (auto-loads on `docs/**`): the client
  compatibility matrix in `clients.md` is code-mirrored — any support/decline
  change requires the matching `Vendor` change in the same commit, and vice
  versa. `worker-doc-reviewer` inherits this via its docs-consistency scope.
- **`post_tool_use_tracker.py` `config_reminder` entry**: edits to
  `src/install/vendor_*.rs` / `client_target.rs` fire a reminder listing
  `docs/src/clients.md`, the emit matrices, and
  `vendor-capability-watchlist.md`. Zero context cost, deterministic.
- **No CLAUDE.md line** — context budget; the duty is file-scoped, not
  every-session knowledge. (CLAUDE.md already points at the catalog that
  routes doc work to `docs-style.md`.)

## Considered Options

### Option A: Hand-written page + table-parity test (chosen)

| Pros | Cons |
|------|------|
| Reuses proven compile-time doc-read discipline | Cell semantics beyond ✓/✗ stay hand-maintained |
| Zero new infrastructure; test fails exactly when a vendor lands without docs | Small bespoke markdown-table parser in test code |
| Page stays human-authored — prose, footnotes, links read well | |

### Option B: Generated matrix (build step emits table into mdBook)

| Pros | Cons |
|------|------|
| Single source of truth, zero drift by construction | New build tooling (no xtask convention exists in the single-binary crate) |
| Scales to arbitrary capability detail | Generated prose reads worse; footnotes/rationale still hand-written anyway |
| | mdBook build gains a codegen dependency — CI + local docs flow complexity |

### Option C: Docs page only, no enforcement

| Pros | Cons |
|------|------|
| Cheapest | Rots exactly like the untested emit matrices already threaten to |
| | Fails the "docs are contracts" principle at the moment the surface grows 2.5× |

**Rationale:** A gives B's guarantee for the one dimension that matters
(support/decline correctness) at C's cost. Revisit B only if the matrix
grows a capability dimension the trait can express.

## Consequences

**Positive:** every future vendor addition is forced to ship its matrix row
and known-gaps entries in the same commit; declines become fair (documented,
justified, tracked).

**Negative:** docs edits to the matrix table now require matching code (and
vice versa) — intended friction. Partial-cell footnotes can still drift.

**Risks:** markdown-table parser brittleness against cosmetic table
reformatting — mitigate by parsing cell *content* tokens (`✓`/`◐`/`✗`) only,
ignoring alignment/whitespace.

**Compatibility:** pure docs + test addition. No CLI, JSON, schema, or
layout change. (A later `grim clients --format json` emitting the same
matrix would be an additive minor — explicitly out of scope here.)

## Related

- [adr_vendor_wave_expansion.md](./adr_vendor_wave_expansion.md) — consumer of this mechanism
- [adr_render_layout_stability.md](./adr_render_layout_stability.md) — why concrete paths link out to stability.md
- `.claude/rules/vendor-capability-watchlist.md` — internal source for Known gaps

## Follow-ups

- `grim clients` CLI surface (machine-readable matrix) — deferred, additive.
- Upgrade `◐` checking when `KindSupport` tri-state exists.
- arch-principles.md ADR index currently missing 7 existing ADRs — index
  housekeeping chore, independent of this decision.
