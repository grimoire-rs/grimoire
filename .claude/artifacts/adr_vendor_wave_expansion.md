# ADR: Six new client vendors in two waves (Cursor, Kiro, Junie, Gemini, Zed, Amp)

## Metadata

**Status:** Proposed
**Date:** 2026-07-19
**Deciders:** maintainer (architect proposal)
**Beads Issue:** N/A (GitHub tracking: grimoire-rs/grimoire#51)
**Related PRD:** N/A
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md` (no new tech; existing Vendor trait, serde, json_splice)
**Domain Tags:** integration
**Supersedes:** N/A (extends [adr_codex_vendor.md](./adr_codex_vendor.md))

## Context

grim ships 4 vendors (Claude, OpenCode, Copilot, Codex) behind the `Vendor`
trait seam (`src/install/vendor.rs`: 8 required methods, 9 defaulted) and the
closed `ClientTarget` enum (`ALL` length 4, client_target.rs:119). The
mid-2026 landscape (session recon, 2026-07-19) changed the economics of
coverage:

- **Agent Skills standard**: ~40 adopting products; the universal skill render
  (empty field registry, like OpenCode/Copilot/Codex) works everywhere.
- **`.agents/skills` shared-dir convention**: grim already writes it for
  Codex — Zed, Amp, and every other scanner of that directory already receive
  grim-installed skills today, undocumented.
- **MCP config formats**: every surveyed client is JSON except Codex (TOML,
  covered) and Goose (YAML, not worth a third splice engine for one client).
  `json_splice` reuse covers the entire JSON tail.
- **AGENTS.md**: 30+ tools read it; it is the *only* rules surface for
  Amp/Zed/Codex-style clients — reachable only via managed-block injection
  ([adr_managed_context_block.md](./adr_managed_context_block.md)).

Per adr_codex_vendor.md's honest-cost correction: a new vendor is one
`vendor_<name>.rs` (~150–250 lines for a simple client, incl. tests) plus
~9 mostly compile-forced touch points (enum arms, anchors, namespaces,
docs-parity test). Stability contract (`docs/src/stability.md`,
adr_render_layout_stability.md): **new vendors are additive minors**; vendor
render layout is explicitly outside the 1.0 freeze.

Six targets selected (maintainer direction, 2026-07-19): Cursor, Kiro
(AWS), JetBrains Junie, Gemini CLI, Zed, Amp. Deferred: Goose (sole
YAML-config client — new splice engine), Windsurf/Cline/Kilo lineage
(mid-rebrand / archived-upstream volatility).

## Decision Drivers

- Adoption × implementation cost; skills+MCP are near-free everywhere, rules
  vary, agents are rare.
- Honest declines over silent lossy installs (Codex-rules precedent), but
  consistently applied — today OpenCode rules silently drop `paths:` scoping
  while Codex declines for the same reason, an unprincipled asymmetry.
- Decline→support later is additive; support→decline is breaking. Ship
  declines first, upgrade when the injection engine exists.
- Determinism contract: every generated file regenerates byte-identical.
- Upstream volatility: vendor paths/formats must be re-verified against
  pinned client versions at implementation time (watchlist procedure), not
  trusted from research alone.

## Industry Context & Research

spec-kit onboards 35 integrations via a registry + base-class ladder;
grim's equivalent is the `Vendor` trait's default methods (a simple vendor
overrides almost nothing). The 31 spec-kit integrations grim skips are
dominated by commands/workflows-format clients — a *kind*-shaped change
(grim has no command kind; `ArtifactKind` = Skill/Rule/Agent/Bundle/Mcp and
every exhaustive match would grow), not a vendor-shaped one. grim's
extension axis is vendor-shaped; commands stay out of scope while the
skills standard absorbs that space (Gemini gained native skills despite its
TOML commands).

**Research artifact:** [`research_spec_kit_rendering.md`](./research_spec_kit_rendering.md) + session recon (client landscape, rule-scoping survey, 2026-07-19).

## Decision Outcome

**Chosen Option:** Option B — two waves, decline-first.

### 1. Wave 1: six `ClientTarget` variants, skills + MCP everywhere, rules where a native scoped surface exists

`ClientTarget::ALL` grows 4 → 10. Per-vendor mapping — **live-verified
2026-07-19** against pinned primary sources (full fact tables:
`research_vendor_verification_{cursor_kiro,junie_gemini,zed_amp}.md`);
corrections from that pass are folded in below:

| Vendor | Skill | Rule | Agent | MCP |
|---|---|---|---|---|
| **Cursor** | native `.cursor/skills/<name>/`, global `~/.cursor/skills/` | **✓ scoped**: `.cursor/rules/<name>.mdc`; `paths` → `globs` (**comma-separated string**) + `alwaysApply: false`; unscoped → `alwaysApply: true` | **✓ native** (corrected — v2.4 shipped `.cursor/agents/<name>.md`, global `~/.cursor/agents/`; registry: `cursor.model`, `cursor.readonly`[bool], `cursor.is-background`[bool]) | `.cursor/mcp.json` / `~/.cursor/mcp.json`, `mcpServers`, stdio needs `type:"stdio"`; env refs `${env:NAME}`; oauth shape ≠ grim block → skip; json_splice |
| **Kiro** | native `.kiro/skills/`, global `~/.kiro/skills/` | **✓ scoped**: `.kiro/steering/<name>.md`; `paths` → `inclusion: fileMatch` + `fileMatchPattern` (array form); unscoped → `inclusion: always`. Global-scope scoped output written correctly but **inert until upstream #9176 fixed** — render-layer warning + Known-gaps row (self-heals on upstream fix) | ✗ declined — native IDE format exists BUT Kiro CLI expects incompatible JSON schema in the same `.kiro/agents/` dir (open bug #8040); watchlist, re-verify wave 2 | `.kiro/settings/mcp.json` project / `~/.kiro/settings/mcp.json` user, `mcpServers`; env refs `${VARIABLE_NAME}`; remote+oauth exists, shape ≠ grim block → skip; json_splice |
| **Junie** | native `.junie/skills/<name>/`, global `~/.junie/skills/<name>/` | ✗ declined (corrected — `.junie/rules/` **does not exist**; real surface is `.junie/AGENTS.md`, no per-file ownable dir) → wave-2 injection bucket | ✗ declined — `.junie/agents/*.md` exists but **EAP-only, not GA**; watchlist for GA | `.junie/mcp/mcp.json` / `~/.junie/mcp/mcp.json`, `mcpServers`; env refs undocumented → skip ref-bearing descriptors; json_splice |
| **Gemini CLI** | shared `.agents/skills` (review-corrected: Gemini's same-tier precedence favors `.agents/skills` over `.gemini/skills` — a native copy loses ties and doubles footprint; joins the Codex/Zed/Amp shared pool under the refcount guard) | ✗ declined — GEMINI.md hierarchy only, no ownable per-file surface; wave-2 injection candidate | **✓ native** (corrected — shipped `.gemini/agents/<name>.md`, global `~/.gemini/agents/`; `experimental.enableAgents` defaults **true**; registry: `gemini.model`, `gemini.temperature`[float], `gemini.max-turns`[int], `gemini.timeout-mins`[int], `gemini.kind`) | `.gemini/settings.json` project/user, `mcpServers`; transport mapping **sse→`url`, http→`httpUrl`**, stdio→`command`; env refs `${VAR}` native; oauth shape ≠ grim block → skip; json_splice |
| **Zed** | shared `.agents/skills` (already written for Codex; flat layout only) | ✗ declined — no scoping; **9-file first-match precedence** (`.rules` first, AGENTS.md 7th) — wave-2 injection must handle shadowing | ✗ declined — external agents via ACP, no file format | `settings.json` (`.zed/settings.json` project / `~/.config/zed/settings.json` global, JSONC), `context_servers`, **flat entry shape**; **no env-ref support upstream → skip ref-bearing descriptors**; json_splice |
| **Amp** | shared `.agents/skills` | ✗ declined — AGENTS.md (→AGENT.md→CLAUDE.md) only; wave-2: @-mention + `globs:` frontmatter enables **scoped** injection | ✗ declined — subagents runtime-spawned, no file format | project `.amp/settings.json` (workspace tier, merged over global) / global `~/.config/amp/settings.json`, key `"amp.mcpServers"` (literal dotted key); env refs `${VAR_NAME}`; `$AMP_SETTINGS_FILE` does **not** exist (CLI flag only); json_splice |

Detection: `.cursor/` `.kiro/` `.junie/` `.gemini/` `.zed/` `.amp/`
project markers; globals `~/.cursor` `~/.kiro` `~/.junie` `~/.gemini`
`~/.config/zed` `~/.config/amp`. **No vendor config-dir env override is
honored in wave 1** — `CURSOR_CONFIG_DIR` (possibly CLI-only), `KIRO_HOME`
(CLI-only, IDE ignores it — bug #9148), and Junie's per-kind
`JUNIE_*_LOCATIONS` family are all watchlisted instead.

New vendor **skill and rule** `KnownField` registries start **empty**
(universal renders). Cursor and Gemini ship **typed agent registries**
(`CURSOR_AGENT_FIELDS`, `GEMINI_AGENT_FIELDS`) — the per-vendor mapping table
above is authoritative; the remaining four (kiro/junie/zed/amp) start empty
across every kind. The `cursor.*`/`kiro.*`/`junie.*`/`gemini.*`/`zed.*`/`amp.*`
namespaces are reserved in `KNOWN_NAMESPACES` regardless, so foreign-key
stripping and typo-guard behavior stay uniform. Each vendor lands as its own conventional
commit (potentially its own minor release) with its compat-matrix row and
docs entries in the same commit
([adr_client_compat_matrix.md](./adr_client_compat_matrix.md) test enforces).

### 2. Rule-support classification principle + `KindSupport` tri-state

Adopt the Codex-ADR follow-up now, before the vendor count doubles:

```rust
enum KindSupport { Native, Degraded, Declined }
fn kind_support(&self, kind: ArtifactKind) -> KindSupport
```

replacing the bool `supports_kind` (internal trait, not public API — rename
is free; ~8 call sites, all mechanical). **No `scope` parameter in wave 1**
(review finding): no wave-1 gate cell is scope-dependent — Copilot's
inert-global case stays a residual resolution-aware branch, and Kiro's
global-fileMatch case is *content*-dependent (scoped vs unscoped rule),
which a kind-level gate cannot see either; it is handled at the render
layer instead (see mapping table). Widening the internal signature later
is cheap; adding `scope` now is speculative generality.
Classification principle for rules, applied uniformly:

- **Native**: client has a per-file rules surface that expresses `paths:`
  scoping (Claude, Copilot `applyTo`, Cursor `globs`, Kiro `fileMatchPattern`).
- **Degraded**: client has a per-file, grim-ownable rules surface but no
  scoping — installed, `paths:` dropped **with a warning** (OpenCode today
  de facto, formalized here). Degraded is honest where Declined would
  withhold real value.
- **Declined**: client has no grim-ownable rules surface at all — nothing to
  own, reap, or hash (Codex, and wave-1 Junie/Gemini/Zed/Amp — Junie
  corrected here after live verification killed the `.junie/rules/`
  claim). Warn + skip + zero outputs, per the Codex gate.

Kiro rules are **Native at both scopes**: grim writes correct `fileMatch`
steering everywhere; at global scope the output is inert until upstream
#9176 is fixed, surfaced as a render-layer warning (`RenderedDoc.warnings`
from `vendor_kiro::rule_index` — never a new installer special case) plus
a Known-gaps row. Correct output + honest warning self-heals when the
upstream bug closes, with no grim change (Copilot inert-global precedent).

This retroactively legitimizes the OpenCode behavior (reclassified Degraded,
gains the missing warning), keeps the Codex decline, and folds Copilot's
"supported-but-inert global rule" hard-coded branch into the same seam —
exactly the tri-state the Codex ADR review predicted.

### 3. Shared-anchor semantics (`.agents/skills`)

Zed + Amp + Codex all target the same `.agents/skills` directory. Renders
are identical (universal shape), so multi-client installs converge on
identical bytes — the adopt-if-identical gate already makes the second
writer a no-op. Two guards required:

- **Uninstall refcount**: removing one client's output must not delete a
  path another client's `ClientOutput` still records. Small installer guard:
  before file/dir removal, check no other client output references the same
  anchored path; if referenced, drop the record only.
- **Status attribution**: `grim status` outputs list the same path once per
  client — acceptable (truthful), no dedup needed.

### 4. Derive `KNOWN_NAMESPACES` from `ClientTarget::ALL`

The one non-compile-forced edit a new vendor needs today (render.rs:41
literal). Six chances to forget it = adopt the Codex-ADR follow-up: derive
from `ALL.map(|c| c.vendor().name())`. Ships with the first wave-1 vendor.

### 5. Wave 2 (dependent on adr_managed_context_block)

Rules for Junie/Gemini/Zed/Amp via managed-block injection (Amp scoped via
`globs:` frontmatter on mentioned files; Junie via `.junie/AGENTS.md`;
Gemini/Zed degraded-unscoped — Zed must target the first existing file in
its 9-name precedence list or the block is shadowed), and a possible
Codex-rules revisit under the same mechanism. Agent-cell re-checks ride
wave 2 too: Kiro (CLI/IDE collision #8040), Junie (EAP→GA). Decided there,
not here — wave 1 does not block on it.

## Considered Options

### Option A: Big bang — six vendors + injection machinery in one wave

| Pros | Cons |
|------|------|
| Full matrix in one release | Couples six vendor adds to an unproven markdown-splice engine |
| | Rules cells for Gemini/Zed/Amp gate everything behind the riskiest component |

### Option B: Two waves, decline-first (chosen)

| Pros | Cons |
|------|------|
| Skills+MCP value ships immediately; declines are additive to upgrade later | Three rules cells ship as ✗ initially |
| Injection engine gets its own ADR/design cycle | Two doc/matrix update rounds |
| Per-vendor commits stay independently revertible | |

### Option C: Cursor only

| Pros | Cons |
|------|------|
| Smallest change, highest single-vendor value | Leaves near-free wins (Kiro/Junie/Gemini skills+MCP; Zed/Amp already-served skills) unshipped and undocumented |

### Sub-option (Junie rules): Degraded install vs Declined

Originally Degraded, on the belief Junie had an ownable per-file surface
(`.junie/rules/`). **Live verification (2026-07-19) showed that surface
does not exist** — Junie's real mechanism is `.junie/AGENTS.md` (single
user-owned file, no per-file dir; legacy `guidelines/` folder semantics
undocumented). With no grim-ownable surface, the classification principle
resolves to **Declined** (same bucket as Codex/Gemini/Zed/Amp), wave-2
injection candidate. Option analysis retained for the record; outcome
corrected by evidence.

## Consequences

**Positive:** matrix grows 15/16 supported cells → 31/40 in wave 1
(Cursor 4, Kiro 3, Junie 2, Gemini 3, Zed 2, Amp 2; → up to 35/40 after
wave 2), every gap documented with a verified reason; `.agents/skills`
free coverage becomes an official, tested claim; tri-state formalizes the
OpenCode Degraded warning, powers the matrix `◐ ⇔ Degraded` check, and —
its main payoff — avoids re-migrating a bool across 10 vendors later.
(It removes no installer branch in wave 1: Copilot's inert-global branch
is explicitly retained, and Codex's decline was already a clean trait
method.)

**Negative:** 6 new `PathAnchor` variants + `AnchorRoots` fields; docs
surface (vendor-metadata.md, emit matrices, clients.md) grows per vendor;
watchlist gains rows (Kiro global fileMatch, per-vendor env-ref syntax).

**Risks:** upstream path/format drift between research and implementation —
mitigation: per-vendor live re-verification gate before landing (watchlist
procedure), ⚠ markers above are the checklist. Client rebrand volatility
(Windsurf precedent) — mitigated by deferring volatile vendors entirely.

**Compatibility:** additive minors throughout, with **one deliberate input
narrowing**: the typed `cursor.*`/`gemini.*` agent registries make a
malformed literal (e.g. `gemini.temperature: "warm"`) a hard publish-gate
failure (exit 65) where the pre-wave build passed it through as plain
metadata. Blast radius is the eight typed keys only — `kiro.*`/`junie.*`/
`zed.*`/`amp.*` keys still warn+drop. Reserved by design, not accidental
breakage. `--client` value set, detection, TUI, publish validation extend
generically (12 generic `ALL`/`VALUE_NAMES` consumers need zero edits). JSON
report shapes:
client names inside already-frozen shapes — additive per stability.md.
`kind_support` rename is internal-only. State schema: untouched (existing
`ClientOutput` fields suffice for wave 1).

## Related

- [adr_codex_vendor.md](./adr_codex_vendor.md) — decline gate, anchor pattern, follow-ups adopted here
- [adr_client_compat_matrix.md](./adr_client_compat_matrix.md) — enforcement of per-vendor docs
- [adr_managed_context_block.md](./adr_managed_context_block.md) — wave-2 dependency
- [adr_tool_namespaced_metadata_rendering.md](./adr_tool_namespaced_metadata_rendering.md) — namespace/projection engine
- [adr_install_state_portability.md](./adr_install_state_portability.md) — PathAnchor set extended

## Follow-ups

- Goose vendor iff a second YAML-config client appears (amortizes the third
  splice engine).
- Structured vendor metadata (`FieldType::Json`/array,
  [adr_structured_vendor_metadata.md](./adr_structured_vendor_metadata.md))
  unlocks OpenCode `permission`, Codex `nickname_candidates` — independent
  track, not gated by this ADR.
- Windsurf/Devin reassessment once the rebrand settles (one release cycle).
