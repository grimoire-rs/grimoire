# Research: How multi-harness tools adopt new vendor capabilities

## Metadata

**Date:** 2026-09-27
**Domain:** cli
**Triggered by:** `.agents/discussions/harness-capability-freshness.md` — competitive lane
**Expires:** 2027-03-27

## Direct Answer

None of the five tools examined discovers vendor capabilities automatically:
no changelog scraping, no CI diff against upstream schemas. All are driven by
issues, PRs or manual sweeps. "Generated" matrices only check internal
consistency, never upstream freshness. Only porex-bot's matrix enforces
per-record evidence (`verified_at`, `verified_by`, source URL, caveats).

## Findings

| Tool | Discovery | Per-capability evidence | Cadence | Matrix |
|---|---|---|---|---|
| [rulesync](https://github.com/dyoshikawa/rulesync) | issues/users | none — README ✅ table | many releases per day in late Sep 2026 (v16.37→v21.0 in ~9 days), one vendor feature per release, sometimes same-day lag | hand-maintained |
| [ruler](https://github.com/intellectronica/ruler) | issues | none | bursty, week+ gaps | hand-maintained; `ruler-check.yml` checks output vs source only |
| [codylindley matrix](https://codylindley.github.io/ai-harness-engineering-compatibility-matrix/) | single curator sweep against primary docs | vendor doc links; one global date ("re-checked … August 26, 2026") | periodic manual sweeps | hand-maintained |
| [porex-bot/agent-skills-compat-matrix](https://github.com/porex-bot/agent-skills-compat-matrix) | manual contribution/testing | `verified_at`, `verified_by`, `repo`/`url`, `caveats` required above `unknown`; no vendor-version field | contribution-driven | table generated from `data/*.json` by `generate.yml`; data hand-verified |
| [vercel-labs/skills](https://github.com/vercel-labs/skills) | community issues/PRs (885 issues, 351 open PRs) | none | contribution-driven | hand-maintained source table |

## Negative

No tool runs scheduled checks against vendor repos or docs. Generation
(ruler, porex-bot) was assumed to imply freshness checking; it does not.

## Leads

- rulesync CONTRIBUTING / issue templates for a "new vendor feature" template.
- Whether `agentskills/agentskills` or `anthropics/skills` publish a
  machine-readable capability schema.
- harness-sync's harness discovery — presence only, or capability versions too.
