# Research: Keeping multi-vendor capability matrices fresh

## Metadata

**Date:** 2026-09-27
**Domain:** cli
**Triggered by:** `.agents/discussions/harness-capability-freshness.md` — entry recon (prior-art lane)
**Expires:** 2027-03-27

## Direct Answer

No one-config-to-many-agents tool (rulesync, ruler, ai-rules-sync, agentsync)
documents a process for tracking *new vendor features* — they propagate
authored config into known formats. The closest exemplar is MDN
browser-compat-data (BCD): automated collection, but reconciliation is still a
human-run script plus required peer review. No first-party account exists of
agent-driven re-verification of a capability matrix; the general LLM
fact-checking literature converges on quote-level primary-source citation and
separated generator/verifier roles.

## Findings

- **Process shape.** BCD wants release-triggered updates
  ([openwebdocs/project#168](https://github.com/openwebdocs/project/issues/168));
  the collector runs per browser release, but merging is `npm run update-bcd`
  run by a contributor
  ([update-bcd.md](https://github.com/openwebdocs/mdn-bcd-collector/blob/main/docs/update-bcd.md)).
  Generic pattern: scheduled scrape → PR only on diff.
- **Evidence records.** BCD schema: `version_added`, `flag[]`,
  `partial_implementation`, ranged versions resolved by hand
  ([compat-data-schema.md](https://github.com/mdn/browser-compat-data/blob/main/schemas/compat-data-schema.md));
  provenance lives in the PR, not a stamped field. General knowledge-base
  practice: explicit `last_verified` plus per-content-type staleness thresholds.
- **LLM re-verification.** Mitigations: quote-level citation checked against
  the source, drop unverifiable findings
  ([arXiv 2605.06635](https://arxiv.org/html/2605.06635v1)); generator /
  fact-checker / citation-checker role split; freshness-aware retrieval over
  training data. Failure modes: fabricated identifiers, real sources cited for
  claims they don't support, stale facts on evolving knowledge.
- **Output routing.** Scrapers open a PR only on diff; BCD requires peer
  review. No source separates doc-only from behavioral-break routing.
- **Adjacent matrices.**
  [agent-skills-compat-matrix](https://github.com/porex-bot/agent-skills-compat-matrix),
  [AI Harness Engineering Compatibility Matrix](https://codylindley.github.io/ai-harness-engineering-compatibility-matrix/)
  — update process not visible from search.

## Negative

BCD's release-triggered automation is partial; the multi-agent sync tools
have no freshness mechanism to copy.

## Leads

- Whether BCD closed #168 with a real release trigger.
- Dependabot's changelog-fetching internals as a cite-and-stamp precedent.
- Direct read of `porex-bot/agent-skills-compat-matrix`.
