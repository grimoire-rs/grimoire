# ADR: Managed markdown block engine for client context injection

## Metadata

**Status:** Proposed
**Date:** 2026-07-19
**Deciders:** maintainer (architect proposal)
**Beads Issue:** N/A (GitHub tracking: grimoire-rs/grimoire#52; re-materialization deferral: #53)
**Related PRD:** N/A
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md` (no new tech; stdlib string handling, existing state machinery)
**Domain Tags:** integration
**Supersedes:** N/A

## Context

Two needs converge on one missing mechanism:

1. **Grimoire-managed context** (maintainer direction, 2026-07-19): grim
   should maintain a knowledge file (working name `.grimoire/context.md`)
   and make each client load it via that client's ambient-context mechanism
   (CLAUDE.md / AGENTS.md / GEMINI.md), so installed-artifact knowledge
   reaches clients without hand-wiring.
2. **Wave-2 rules coverage**
   ([adr_vendor_wave_expansion.md](./adr_vendor_wave_expansion.md)): Gemini,
   Zed, Amp — and potentially Codex — have **no grim-ownable rules surface**;
   their only ambient surface is a single user-owned file. Reaching them
   requires grim to own a *region inside* a user-owned markdown file.

grim already owns fragments inside user-owned files twice, with a proven
skeleton (session recon, file:line verified):

- **OpenCode `instructions` entry** (opencode_config.rs): fixed managed
  string in a JSON array; presence-derived, strict-add/tolerant-remove.
- **MCP config splice** (json_splice.rs / toml_splice.rs; ~740/770 lines):
  `ClientOutput.entry` locator (two-level JSON pointer, install_state.rs:84-94),
  semantic `entry_value_hash` drift detection, adopt-if-identical /
  refuse-if-different untracked-clobber gate (installer.rs:1260-1278),
  splice-out-never-delete-file uninstall.

No markdown equivalent exists (grep-verified). `ClientOutput.entry` is
documented MCP-specific; `McpConfigFormat` dispatches only Json|Toml at
exactly 3 sites. The `entry` field itself is the precedent that an
**optional `ClientOutput` field is additive — no state-version bump**
(V1→V2 discriminator untouched).

Naming constraint: `grim context` already exists — a **frozen, read-only**
introspection command (context.rs:4-15 "no network, no writes, no side
effects"; docs/src/commands.md:503; 7 acceptance tests). It cannot be
overloaded with a mutating meaning, even additively, without breaking its
documented mental model.

Injection capability per client (web recon, primary docs, 2026-07-19):

| Client | Ambient file | Import mechanism | Injection mode |
|---|---|---|---|
| Claude Code | CLAUDE.md | `@path` (approval dialog; declining disables imports permanently) | **None needed** — `.claude/rules/` is a native ownable surface grim already uses |
| Copilot (VS Code) | copilot-instructions.md | none (closed "not planned") | **None needed** — `.github/instructions/` native |
| OpenCode | opencode.json | `instructions` array (native file list) | **Already shipped** (managed glob entry) |
| Codex | AGENTS.md | none (spec has no import) | Inline block |
| Zed | AGENTS.md | none | Inline block |
| Junie | .junie/AGENTS.md / AGENTS.md | none | Inline block (rules dir exists — injection only for context feature) |
| Amp | AGENTS.md | `@path` mention + `globs:` frontmatter on mentioned files | Pointer block (enables **scoped** wave-2 rules) |
| Gemini CLI | GEMINI.md | `@file.md` memport (depth 5, `.md`-only, path-allowlisted) | Pointer block |

Key structural fact: Codex/Zed/Amp (and Junie) all read the **same
`AGENTS.md`** — one managed block serves several clients at once, which
makes shared-region ownership semantics (who records it, when is it removed)
the central design problem.

## Decision Drivers

- One engine, two features: managed context now, wave-2 rules cells next,
  Codex-rules revisit possible — amortizes the new machinery.
- Never clobber user content; user hand-edits inside the block must be
  detected as drift, content outside the block must be byte-preserved.
- Uninstall completeness — spec-kit's known gap is *no removal path at all*
  (disabled = inert block forever); grim's uninstall discipline requires
  `remove_block`.
- Writing into a user's CLAUDE.md/AGENTS.md is invasive — must be **opt-in**.
- Additive-only stability: new optional state field, no version bump, no
  frozen-shape changes.
- CLI vocabulary hygiene: "context" is taken by a frozen read-only command.

## Industry Context & Research

spec-kit's `agent-context` extension is the direct prior art: config-driven
target list (`context_file(s)`), `<!-- SPECKIT START/END -->` markers, and a
battle-tested 4-branch upsert (`_upsert_section`): both markers → splice
region, preserve everything outside; orphaned start or end marker →
regenerate wholesale from the surviving marker; neither → append at EOF
(create file if absent). It also repairs vendor activation quirks in
passing (injects `alwaysApply: true` for `.mdc` targets). Its gaps — no
removal command, no drift detection, re-appends after manual deletion —
are exactly what grim's existing ownership skeleton adds. Generic siblings:
Ansible `blockinfile`, Puppet concat fragments (same preserve-outside /
regenerate-inside contract).

**Research artifact:** [`research_spec_kit_rendering.md`](./research_spec_kit_rendering.md) §"Context-file registry" + session recon (injection mechanics, marker prior art, 2026-07-19).

## Decision Outcome

**Chosen Option:** Option 1 — new sibling engine + additive locator field;
install-side feature, no new artifact kind; opt-in.

### 1. Engine: `src/install/markdown_splice.rs`

Mirrors the two-function `Splice`-returning contract of the existing
splicers:

```rust
upsert_block(text, id, content) -> Result<Splice>   // Splice::{Changed(String), Unchanged}
remove_block(text, id)          -> Result<Splice>
```

Markers carry an id so one file can hold independent grim blocks:

```markdown
<!-- grimoire:begin <id> -->
…managed content…
<!-- grimoire:end <id> -->
```

Semantics: spec-kit's 4-branch algorithm (splice / orphaned-marker
regenerate / append-or-create), plus: preserve the file's dominant line
ending (don't normalize user files), idempotent by content comparison
(byte-equal block → `Unchanged`). Simple substring/line search — markdown
has no grammar to parse; no shared trait with json/toml splice (three
engines, one informal 3-function contract, matching the existing
convention).

### 2. State: additive `block` locator on `ClientOutput`

New optional field `block: Option<String>` (the marker id) beside `entry`
— **not** overloading `entry` (its doc contract is MCP-specific) and **not**
extending `McpConfigFormat` (its 3 dispatch sites are MCP plumbing).
`content_hash` for a block output = SHA-256 over the block's managed
content (trailing-whitespace-normalized). Integrity gate, adopt-or-refuse
untracked gate, and drift refusal (`--force` override) reuse the existing
skeleton: presence of `block` switches the generic file ops onto the
block-aware path, exactly as `entry` does for MCP.

**Shared-file semantics** (several clients, one AGENTS.md): one block id
per (file, feature), written once, recorded as a `ClientOutput` per
participating client — same refcount rule as
adr_vendor_wave_expansion.md §3: last participating client's uninstall
removes the block; earlier uninstalls only drop their record.

### 3. Content model: install-side feature, not a new `ArtifactKind`

A new kind would cut across every exhaustive `match kind` site
(client_target, path_anchor, materializer, render, installer, bundle
resolver) for zero v1 benefit. Instead the engine has two content
*producers*, phased:

- **v1 — managed context**: grim materializes `.grimoire/context.md`
  (project) / `$GRIM_HOME/context.md` (global) from the install state — a
  compact digest of installed artifacts (name, kind, description,
  invocation hints) so any client knows what grim installed. Injection per
  the mode table: pointer block (one import line) for Gemini/Amp; inline
  block (digest embedded) for AGENTS.md-only clients. Clients with native
  ownable surfaces (Claude, Copilot, OpenCode) get **nothing injected** —
  they already load grim's output natively; no redundant block.
- **v2 — wave-2 rules**: rule bodies rendered into the managed block
  (inline) or referenced (Amp pointer + `globs:` scoping). Detailed in the
  wave-2 design, riding this engine unchanged.
- **v3 — artifact-provided brief**: an artifact may *ship* brief content
  (optional companion member, `description`-companion precedent — layout
  detail deferred to its own design). When an installed artifact provides
  a brief and the feature is **disabled**, install/update emits a warning
  ("artifact X provides a brief; enable `[options].managed_brief` to
  inject it") — discoverability without side effects. The warning contract
  is decided now; the member format is not.

Publishable "context artifact" kind: explicitly deferred, revisit post-1.0
if catalog demand appears.

### 4. Activation: opt-in config, no new top-level command

No new CLI verb. The feature runs as install/uninstall-time sync (the
`Vendor::sync_config` pattern OpenCode already uses), gated by a config key,
default **off**:

```toml
[options]
managed_brief = true   # name provisional — see Open naming below
```

`grim install`/`update`/`uninstall` maintain the block only while the key
is on; turning it off + reinstalling removes managed blocks (tolerant-remove
discipline). Surfacing in `grim status` outputs (target file + block id) is
additive JSON.

**Deferred: automatic re-materialization on config change.** Flipping the
key (or `grim update` changing the digest inputs) *should* re-materialize
blocks without an explicit reinstall — but config edits happen outside any
grim invocation (no watch mechanism), and a config-flip-aware sync touches
the same re-render-trigger gap adr_render_layout_stability.md already
flagged (`output_at_current_layout` is blind to shape changes at a stable
path; mitigation deferred to `design_render_scheme_versioning.md`). v1
contract: blocks converge on the **next** install/update/uninstall run;
automatic convergence is a tracked follow-up issue, not silently promised.

**Open naming (user decision):** feature/config-key name must avoid the
"context" root. Candidates: `brief` (recommended — short, evokes "briefing
the agent"), `primer`, `handoff`. The materialized file itself may keep the
user-suggested `context.md` name — file naming is render layout (unstable
surface), CLI/config vocabulary is the frozen one.

### 5. Safety rules

- Opt-in only; never touches CLAUDE.md/AGENTS.md/GEMINI.md by default.
- Create-if-absent allowed *only* under the opt-in (consented).
- Adopt-if-identical / refuse-if-different on first contact with an
  untracked existing block; `--force` overrides.
- User edits inside the block → drift refusal on next sync (same UX as
  modified rendered files today).
- Content outside markers: byte-preserved, hard requirement, tested.
- Claude `@import` approval-dialog quirk is moot (no injection for Claude).

## Considered Options

### Option 1: New sibling engine + additive `block` locator (chosen)

| Pros | Cons |
|------|------|
| Reuses proven ownership/idempotence/drift/uninstall skeleton | Third splice module with an informal (unshared) contract |
| No state-version bump; no frozen-surface change | New locator field to plumb through integrity/uninstall paths |
| One engine serves context + wave-2 rules + Codex revisit | |

### Option 2: Extend `McpConfigFormat` with a Markdown variant

| Pros | Cons |
|------|------|
| Reuses the existing 3 dispatch sites | Overloads a documented MCP-specific contract (`entry` pointer semantics don't fit marker ids) |
| | Markdown blocks are not config *entries* — forced abstraction |

### Option 3: New `ArtifactKind::Context` (publishable kind)

| Pros | Cons |
|------|------|
| Context becomes distributable like any artifact | Kind-shaped change across every exhaustive match — highest-cost extension axis |
| | v1 content is *derived* from install state, not published content — wrong model today |

### Option 4: Per-skill hand-rolled injection (status quo)

| Pros | Cons |
|------|------|
| Zero grim work | Every skill reinvents client detection + marker handling (observed in the wild); no drift detection, no uninstall |

## Consequences

**Positive:** AGENTS.md-only clients become reachable (unblocks 3–4 wave-2
rules cells); installed-set discovery lands for every client; spec-kit's
removal/drift gaps are fixed by construction.

**Negative:** a third hand-rolled splice engine to maintain; block content
regeneration must stay deterministic (same contract as every renderer);
shared-file refcounting adds an installer guard.

**Risks:** clients changing ambient-file precedence (e.g. Zed's ordered
fallback list) can silently stop reading the injected file — watchlist
rows per injection target; digest content quality (too long = context
bloat in the client) — cap digest size, keep it a pointer where imports
exist.

**Compatibility:** state field additive (entry precedent, no version bump);
config key additive; status JSON additive; no CLI surface change in v1; no
existing render layout moves. Fully within the 1.0 additive-only policy.

## Related

- [adr_vendor_wave_expansion.md](./adr_vendor_wave_expansion.md) — wave-2 consumer + shared-anchor refcount rule
- [adr_codex_vendor.md](./adr_codex_vendor.md) — the rules decline this engine may later revisit
- [adr_install_state_portability.md](./adr_install_state_portability.md) — ClientOutput/anchor machinery extended
- [adr_render_layout_stability.md](./adr_render_layout_stability.md) — why the materialized file name is changeable, the config key is not

## Follow-ups

- Codex-rules revisit: inline rule injection is still scoping-lossy →
  would be `KindSupport::Degraded`, decide with wave-2 design.
- Digest format spec (what the brief contains, size cap) — design-spec
  detail for the plan phase, not ADR-level.
- Naming decision (`brief` vs alternatives) — user call before implementation.
- Re-materialization on config change / update — deferred, tracked in
  grimoire-rs/grimoire#53; v1 converges on next grim run.
- Artifact-provided brief member format (v3 producer) — own design cycle;
  only the warn-when-disabled contract is fixed here.
- Watchlist rows: per-client ambient-file precedence, Gemini memport
  allowlist behavior against `.grimoire/` paths (must verify memport can
  import from a non-default directory).
