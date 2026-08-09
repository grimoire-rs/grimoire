# Meta-Plan: Wave-1 Vendor Expansion (#51)

Preview for `/swarm-plan` max-tier run. Approval gate — no workers launched yet.

## Classification

- **Tier:** max (auto, confident)
- **Signals:** cross-subsystem (install core, docs, acceptance tests), trait-hierarchy change (`KindSupport` tri-state replaces `supports_kind` bool across all vendors), One-Way Door strings (new `PathAnchor` serialized tags persist into every install-state file; six `<vendor>.*` namespace names become authoring format), scope Large (6 vendors, ~10+ files each wave).
- **Overlays (adapted — deviations from stock max, approved via this gate):**
  - `architect=reuse` — design phase satisfied by three user-reviewed ADRs from this session (`adr_vendor_wave_expansion.md`, `adr_client_compat_matrix.md`; `adr_managed_context_block.md` out of scope). No new opus design ADR; opus architect runs only in the review panel (trade-off honesty). ADR status flips Proposed→Accepted at plan approval.
  - `research=3`, refocused — researchers do the ADR's ⚠ live-verification checklist (exact skill dirs, MCP config paths/keys/entry shapes, rule frontmatter, detection signals, env-var overrides, version pins) instead of generic SOTA (already persisted: `research_spec_kit_rendering.md` + session recon).
  - `pr_faq`/`prd` skipped — internal platform expansion; issue #51 Value section + ADRs carry the narrative. (Stock max mandates both; explicit deviation.)
  - `codex=on` (mandatory at max) — plan-artifact review, model sol; graceful skip if companion unavailable.

## GitHub context

- Target: grimoire-rs/grimoire#51 (authored this session — content in ADR, no re-fetch).
- Related: #52/#53 excluded (managed context, separate plan later).

## Workers I Would Launch

| Phase | Workers | Model | Scope |
|---|---|---|---|
| 1 Discover | 1 `worker-architecture-explorer` | sonnet | installer/uninstall/install_state internals the tri-state + refcount guard touch; reusable test patterns |
| 1 Discover | 2 `worker-explorer` | haiku | (a) render/client_target/path_anchor extension sites; (b) docs/src conventions + existing parity tests + acceptance-test fixtures for vendors |
| 2 Research | 3 `worker-researcher` | sonnet | per-vendor live verification, 2 vendors each: Cursor+Kiro / Junie+Gemini / Zed+Amp → `research_vendor_verification_*.md` |
| 3–5 Author | orchestrator inline | — | `plan_vendor_wave_expansion.md` (Status block, contracts, TDD phases, per-vendor task graph) |
| 6 Review | `worker-reviewer` (spec-compliance) + `worker-architect` (opus, trade-off honesty) + `worker-researcher` (SOTA gap) | mixed | parallel panel, ≤2 rounds |
| 6 Gate | `codex-adversary` plan-artifact | sol | one-shot, triaged |

Max concurrent: 3 (phases sequential). Total ~10 worker launches.

## Artifacts I Would Produce

- `.agents/plans/plan_vendor_wave_expansion.md` — executable phases (Stub → Specify → Implement → Review) per vendor + shared groundwork (tri-state, namespace derivation, refcount guard, matrix page + parity tests); explicit sections: backwards-compat, TDD strategy, JSON-interface impact, exit codes (per plan quality bars).
- `.claude/state/current_plan.md` pointer.
- `.agents/research/research_vendor_verification_{cursor_kiro,junie_gemini,zed_amp}.md` — version-pinned path/format verification.
- ADR status flips (Proposed→Accepted) on the two in-scope ADRs.

## Estimated Cost

~10 workers (3 parallel peak), heaviest call = opus review architect + Codex sol pass. No implementation tokens.

## Not Doing

- No implementation, no commits, no PR.
- No managed-context work (#52/#53).
- No pr_faq/prd artifacts (deviation above).
- No re-verification of Goose/Windsurf/Cline (out of scope per ADR).
