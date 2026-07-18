# ADR: Vendor render layout is outside the 1.0 stability contract; plugin rendering is a deferred per-vendor projection mode

## Metadata

**Status:** Accepted
**Date:** 2026-07-09
**Deciders:** Maintainer (via release-prepare Q&A), Architect
**Beads Issue:** N/A
**Related PRD:** N/A
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md`
**Domain Tags:** api | integration
**Supersedes:** N/A
**Superseded By:** N/A

## Context

Grimoire is heading to 1.0.0: CLI args, exit codes, `--format json` report
shapes, and file schemas become semver contracts. The open question this
ADR settles: **is the vendor on-disk render layout part of that contract,
and does grim need a plugin render path before freezing?**

Today grim projects canonical, agentskills-pure artifacts into plain
vendor-dir files at install time: skills to `~/.claude/skills/<name>/` or
`<workspace>/.claude/skills/<name>/`, rules/agents to their vendor
locations, MCP descriptors spliced as single managed members into shared
vendor config files (`ClientOutput.entry`, a two-level JSON pointer like
`/mcpServers/grim`). The install record (`ClientOutput` in
`src/install/install_state.rs`) anchors every output
(`AnchoredPath`/`AnchorRoots`) and hashes its footprint
(`footprint_hash`, or semantic `entry_value_hash` for entry outputs).

Meanwhile the harnesses grew *plugin* structures: Claude Code plugins
carry skills/agents/hooks/MCP/LSP under a colon-namespaced
`plugin:component` scheme, registered via `known_marketplaces.json` /
`extraKnownMarketplaces` + `enabledPlugins`; Copilot CLI (v1.0.69+) has a
comparable, weeks-old format. Two facts constrain any plugin move:

- Those registration surfaces are **undocumented plumbing** (Claude) or
  **actively churning** (Copilot).
- Claude's plugin format has **no rules/memories surface** — rules stay
  plain files no matter what; a plugin projection is inherently partial
  per vendor.

The release audit confirmed the namespace-collapse hazard that plugins
would eventually solve properly (OCI repo path collapses to its last
segment on disk) is already mitigated pre-1.0 by two shipped guards: the
declare-time `(kind, name)` uniqueness guard in `grim add` (a conflicting
re-declare refuses loudly instead of silently aliasing) and the
untracked-clobber guard in the installer (an unrecorded pre-existing
destination refuses instead of being overwritten).

## Decision Drivers

- 1.0 must freeze what users script against — without freezing surfaces
  grim does not own (vendor dirs are the *vendors'* namespace).
- Plugin rendering must remain reachable later **as a minor release**,
  not a 2.0.
- Grim's product identity is the agnostic canonical format
  (agentskills.io closest-to-spec, `product-context.md`); nothing
  vendor-specific may leak into the canonical layer.
- Do not build on Claude/Copilot registration surfaces while they are
  unfrozen/undocumented.

## Considered Options

### Option 1: Layout outside the contract; plugin rendering deferred as a render mode (chosen)

Declare vendor on-disk layout an implementation detail; record the
plugin direction (mode on `ClaudeVendor`, grim-owned source roots,
existing seams) without implementing it.

| Pros | Cons |
|------|------|
| 1.0 freezes only surfaces grim owns (CLI, JSON, schemas) | Users scripting raw vendor paths must migrate to `status --format json` |
| Plugin mode lands later as additive minor | Direction recorded on today's knowledge of vendor surfaces; may need refresh |
| No dependency on undocumented vendor plumbing | — |

### Option 2: Plugins as a new `ClientTarget`

`claude-plugin` beside `claude`.

| Pros | Cons |
|------|------|
| No render-mode concept needed | Wrong axis: same client, different *projection*; doubles every client-selection surface (`--client`, config `clients`, detection) for one vendor's packaging format |

### Option 3: Canonical namespace separator in the artifact format

Encode `bundle:skill` (or `bundle.skill`) into the canonical layer so all
vendors render namespaced names.

| Pros | Cons |
|------|------|
| One namespacing story everywhere | Breaks vendor-agnosticism: agentskills spec has no separator; forces one vendor's syntax onto all others; poisons the canonical layer grim's identity rests on |

### Option 4: Render plugins now, before 1.0

| Pros | Cons |
|------|------|
| Plugin capabilities (hooks/MCP/LSP) reachable at 1.0 | Freezes grim 1.0 on someone else's unfrozen contract (undocumented `known_marketplaces.json`, weeks-old Copilot format) — inverts the risk the 1.0 contract exists to remove |

### Option 5: Never render plugins

| Pros | Cons |
|------|------|
| Nothing to build | Hooks/MCP/LSP-carrying plugin capabilities stay permanently unreachable to grim users on Claude; concedes the harness-native packaging ground |

## Decision Outcome

**Chosen Option:** Option 1, in four parts.

### 1. Vendor render layout is OUTSIDE the 1.0 semver contract

The exact paths, directory shapes, and file names grim writes into
`~/.claude`, `<workspace>/.claude/`, `~/.copilot`, OpenCode config dirs,
and the placement of managed MCP config members are implementation
details. They may change in a **minor** release, provided:

- artifacts remain discoverable by the target client after upgrade;
- `status`/`update`/`uninstall` keep working across the change
  (automatic migration of install-state anchors);
- the supported discovery channel is `grim status --format json`, whose
  per-artifact `outputs: [{client, path}]` array (added in this release
  cycle, `src/api/status_report.rs`) reports where every output actually
  landed.

This is the move that makes plugin rendering an additive minor later
instead of a 2.0.

### 2. Plugin rendering direction (post-1.0, opt-in): a render mode on `ClaudeVendor`

- **Mode, not target.** Plugin rendering is a projection mode of the
  existing Claude vendor — not a new `ClientTarget` (Option 2 rejected).
- **Granularity: one plugin per declared top-level unit.** A bundle
  becomes a plugin named by its binding; a standalone package becomes a
  plugin named by its declared name. The declare-time `(kind, name)`
  uniqueness guard in `grim add` makes the declared name the
  collision-free plugin name.
- **Namespace is vendor syntax.** Users see `<declared-name>:<skill>` on
  Claude because *Claude* namespaces plugin components with a colon. The
  canonical layer stays agentskills-pure with no separator choice;
  namespacing is a per-vendor projection concern (dot/colon/whatever
  each harness uses). Option 3 rejected.

### 3. Mechanics recorded for the future implementer

- Plugin sources live in **grim-owned roots**:
  `$GRIM_HOME/claude/marketplace/…` (global scope),
  `<workspace>/.grimoire/claude/…` (project scope). Grim renders there,
  then registers — the vendor dir is never the source of truth.
- Registration goes through the existing reversible config-registration
  seam `Vendor::sync_config` (`src/install/vendor.rs:193`, the hooks-ADR
  pattern) + `src/install/json_splice.rs` (`upsert_member` /
  `remove_member` / `member_value`) against `known_marketplaces.json` /
  `extraKnownMarketplaces` + `enabledPlugins` members.
- Outputs are recorded **entry-typed** via the existing
  `ClientOutput.entry` pointer form with semantic hashing
  (`entry_value_hash`) — no state V3, no new `PathAnchor` variant.
- Uninstall inverts registration (the OpenCode rules-glob lifecycle
  precedent: registration added on install is removed on uninstall,
  never the user's file).
- Claude's own plugin cache directory is **out of integrity scope** —
  grim verifies its rendered sources and its registrations, not the
  harness's cache.
- **Rules stay plain files.** The Claude plugin format has no
  rules/memories surface; plugin mode is a partial projection per
  vendor, and that partiality is expected, not a bug.

### 4. Reserved config key: `[options] render = "files" | "plugin"`

Paper reservation only — the spelling is fixed here so docs and future
code agree, but no code parses it today. Adding it later is additive
(YAGNI now).

**Rationale:** grim's charm is the agnostic canonical format; the 1.0
contract must cover grim-owned surfaces only. Deferring implementation
avoids freezing on undocumented (`known_marketplaces.json`) or weeks-old
(Copilot) vendor surfaces while keeping the path open and cheap: every
seam the plugin mode needs (`sync_config`, `json_splice`, entry-typed
`ClientOutput`) already exists and ships in 1.0.

### Consequences

**Positive:**
- The 1.0 freeze covers exactly CLI args, exit codes, `--format json`
  shapes, `grimoire.toml`/`grimoire.lock`/state-V2 schemas — auditable
  and enforceable.
- Layout migrations (including the plugin mode's move of sources into
  grim-owned roots) are minors with automatic migration.
- Plugin mode lands as opt-in minor when vendor surfaces stabilize;
  Copilot parity follows when their format settles.

**Negative:**
- Users scripting against raw vendor paths (e.g. hardcoding
  `~/.claude/skills/<name>/`) must move to `grim status --format json`
  `outputs`. The stability docs page (plan item 10) must say this
  loudly.
- The recorded mechanics (Decision 3) describe today's Claude surfaces;
  the future implementer must re-verify them before building.
- The re-render trigger (`output_at_current_layout`) is a path-move
  proxy — blind to shape changes at a stable index path, to the
  `render="plugin"` config flip (Decision 3/4), and to entry-output
  relocations; mitigation recorded in
  `design_render_scheme_versioning.md` (render_scheme stamp, deferred
  YAGNI).

**Risks:**
- Vendor changes their plugin registration format before grim implements
  → mitigated: nothing shipped depends on it; only this ADR needs a
  refresh.
- A layout migration ships without its automatic state migration →
  mitigated: the promise in Decision 1 is testable (upgrade fixture:
  install with old layout, upgrade, `status`/`uninstall` must work).

## Technical Details

### Architecture

```
canonical artifact (agentskills-pure, no namespace separator)
        │
        ├─ render mode "files" (today, default forever)
        │    └─ vendor dirs: ~/.claude/skills/<name>/, rules, agents,
        │       MCP members via json_splice (ClientOutput.entry)
        │
        └─ render mode "plugin" (deferred, opt-in, Claude first)
             ├─ sources: $GRIM_HOME/claude/marketplace/… (global)
             │           <workspace>/.grimoire/claude/… (project)
             ├─ registration: Vendor::sync_config + json_splice
             │   (known_marketplaces.json / extraKnownMarketplaces,
             │    enabledPlugins) — entry-typed ClientOutputs
             ├─ namespace: <declared-name>:<skill>  (vendor syntax)
             └─ rules: still plain files (no plugin surface)
```

### API Contract

Stable at 1.0: `grim status --format json` `outputs: [{client, path}]`
as the discovery channel for materialized locations. Explicitly unstable:
every path under any vendor root.

## Implementation Plan

1. [x] Record decision (this ADR).
2. [ ] Stability docs page lists vendor layout under "explicitly
       unstable" and points scripts at `status --format json` (plan
       item 10).
3. [ ] Post-1.0, when Claude registration surfaces are documented or
       demonstrably stable: implement render mode per Decision 3
       (global Claude scope first, auto-migration on mode switch,
       Copilot parity when their format settles).

## Validation

- [x] Decision grounded against real seams: `Vendor::sync_config`
      (`src/install/vendor.rs:193`), `json_splice::{upsert_member,
      remove_member, member_value}`, `ClientOutput{entry,
      content_hash}` + `entry_value_hash`
      (`src/install/install_state.rs`).
- [ ] Upgrade fixture (layout-migration promise) added when the first
      layout change ships.

## Links

- `.claude/rules/product-context.md` — canonical identity this ADR
  aligns with
- `.claude/artifacts/adr_registry_default_dedup.md` — sibling 1.0
  contract ADR (registry resolution semantics)
- `.claude/artifacts/adr_tool_namespaced_metadata_rendering.md` —
  vendor-rendering precedent
- `docs/src/` stability page (plan item 10) — user-facing companion

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-07-09 | Architect (release-prepare) | Initial accepted version |
| 2026-07-18 | Claude (release-compat audit) | Documented trigger blind spots; linked design_render_scheme_versioning.md |
