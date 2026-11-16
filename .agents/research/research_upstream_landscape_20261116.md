# Research: upstream refresh — landscape (sweep)

## Metadata
**Date:** 2026-11-16 · **Domain:** landscape · **Depth:** sweep — never checked
**Triggered by:** /upstream-refresh
**Expires:** 2027-05-16

## Claims

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | skills.sh / Vercel: 27.2k stars | .agents/research/research_promotion_positioning.md:98 | 27.2k★ (2026-07-26) | 39,673 stars (repo vercel-labs/skills, pushed 2026-11-16) | https://api.github.com/repos/vercel-labs/skills | stargazers_count 39673 | v1.9.0 (2026-11-16) |
| 2 | skills.sh documents no lockfile | research_promotion_positioning.md:98 | "no dependency resolution, cross-vendor rendering, hosted registry, or lockfile" | CHANGED: README documents `skills-lock.json` v2 with `computedHash` and `install --frozen-lockfile`; Git sources pin `commit`; skills.sh is a hosted directory/leaderboard | https://github.com/vercel-labs/skills/blob/main/README.md | "`skills-lock.json` (version 2) records a `computedHash` for each project skill." / "`skills install` (with or without `--frozen-lockfile`) fetches that commit, not the latest commit on `ref`" | v1.9.0 |
| 3 | skills.sh cross-vendor: no rendering (writes same SKILL.md to many agent dirs) | research_promotion_positioning.md:98 | no cross-vendor rendering | agent count grew: "Supports OpenCode, Claude Code, Codex, Cursor, and 75 more" (79 agents). Skills-only, installs into per-agent skill dirs; no rules/agents rendering stated | https://github.com/vercel-labs/skills/blob/main/README.md | "Supports **OpenCode**, **Claude Code**, **Codex**, **Cursor**, and [75 more](docs/agents.md)." | v1.9.0 |
| 4 | skills.sh exists, hosted directory | product-context.md:133 | Vercel `skills.sh` nearest comparable | exists (redirects to www.skills.sh); leaderboard 1,450,205 all-time entries; packs bundle public and private skills | https://www.skills.sh/docs | "Packs let you bundle public and private skills into one shareable install command." | n/a |
| 5 | Tessl exists; hosted registry, not OCI | research_promotion_positioning.md:99; product-context.md:133 | Podjarny-founded; $125M; Snyk score on every public skill | unchanged for registry + Snyk security: registry exists with quality/impact/security scores. Positioning drifted: homepage now "Software factory"; docs "What is Tessl?". Funding and founder not re-sourced (Unsourced) | https://tessl.io/registry ; https://docs.tessl.io/llms.txt | "Evaluated, secured, versioned, and discoverable." / "read their Snyk-powered security scores, and use install policies to block risky skills before they reach a developer." | n/a |
| 6 | ClawHub exists, hosted, 3,000+ skills, semantic search | research_promotion_positioning.md:100 | 3,000+ skills | exists at clawhub.ai (clawhub.com redirects); now lists Skills and Plugins; repo openclaw/clawhub 9,554 stars, pushed 2026-11-16. Skill total not shown on page fetched (count unverified) | https://clawhub.ai/skills ; https://api.github.com/repos/openclaw/clawhub | page text: "Discover skills and plugins from top creators" | n/a |
| 7 | skillctl: Sigstore/cosign signing, narrower scope | research_promotion_positioning.md:101; product-context.md:133 | "skillctl: Sigstore/cosign signing; narrower scope" | CHANGED scope: vrothberg/skillctl (created 2026-10-19, pushed 2026-11-14, Apache-2.0, 2 stars) is an OCI-registry skill manager: signed versioned artifacts, project dependencies with `skillctl.yaml`, Renovate updates. Directly overlaps grim's OCI claim. Sigstore/cosign confirmed. "Narrower scope" holds (skills only; plugins possible) | https://github.com/vrothberg/skillctl | "publish versioned artifacts through OCI registries and verify them against trusted publisher keys." / "Signing supports sigstore/cosign keys" | none tagged |
| 8 | jeffreytse/grimoire exists, 21 stars, Go, 1000+ skills across 26 domains | research_promotion_positioning.md:71,102; product-context.md:133 | 21★ | 39 stars; pushed 2026-07-24; latest v0.2.0 (2026-07-19); description unchanged | https://github.com/jeffreytse/grimoire | "Grimoire is a skills package manager for AI agents and best practices — 1000+ skills across 26 domains" | v0.2.0 |
| 9 | Vaishnav-Sabari-Girish/grimoire: Rust task runner, v0.4.0 2026-07-02, active | research_promotion_positioning.md:73 | v0.4.0, "active" | unchanged: v0.4.0 still latest, last push 2026-07-02 (no commit in 4.5 months; "active" now stale) | https://github.com/Vaishnav-Sabari-Girish/grimoire | "A modern grimoire for developers—define sigils, cast workflows, automate anything." | v0.4.0 |
| 10 | Vitale OCI spec: draft v0.1.0, Discussion #292, 2026-04-02 | research_promotion_positioning.md:128 | draft v0.1.0, 2026-04-02 | unchanged: discussion open, 28 comments, 16 +1; spec repo last push 2026-04-02, still "Draft" 0.1.0 | https://github.com/agentskills/agentskills/discussions/292 ; https://github.com/ThomasVitale/agents-skills-oci-artifacts-spec | "**Version:** 0.1.0 **Status:** Draft" | 0.1.0 |
| 11 | Arconia CLI (Java) and salaboy skills-oci (Go) both implement the spec, newer and thinner than grim | research_promotion_positioning.md:133 | exist | exist: arconia-io/arconia-cli 31 stars, pushed 2026-11-16, tag early-access; salaboy/skills-oci 16 stars, v0.0.12 2026-06-12 | https://github.com/arconia-io/arconia-cli ; https://github.com/salaboy/skills-oci | repo metadata via API | skills-oci v0.0.12 |
| 12 | openskills named among Show HN launches | research_promotion_positioning.md:103 | exists | numman-ali/openskills 10,832 stars, v1.5.0 2026-01-17, last push 2026-01-18 | https://github.com/numman-ali/openskills | repo metadata via API | v1.5.0 |
| 13 | hesreallyhim/awesome-claude-code 50,947 stars | research_promotion_positioning.md:58,408 | 50,947 | 58,149 stars, pushed 2026-11-16 | https://api.github.com/repos/hesreallyhim/awesome-claude-code | stargazers_count 58149 | n/a |
| 14 | Anthropic Agent Skills open standard Dec 2025, cross-vendor | research_promotion_positioning.md:105 | Dec 2025 | not re-fetched here; skills.sh README links https://agentskills.io as the spec | https://github.com/vercel-labs/skills/blob/main/README.md | "[Agent Skills Specification](https://agentskills.io)" | n/a |
| 15 | Comparable Tools structural prior art: ORAS, Helm OCI, lockfile UX, Claude Code plugin marketplaces | product-context.md:134-136 | listed, no re-verify date of their own | not checked (not tool-positioning claims; sweep scope) | - | - | - |

## Verifier sign-off

none required — no code landed in this run.

## Routing

- #1 skills.sh / Vercel: 27.2k stars → (a) ledger or doc text fix
- #2 skills.sh documents no lockfile → (c) issue draft
- #3 skills.sh cross-vendor: no rendering (writes same SKILL.md to many agent dirs) → (a) ledger or doc text fix
- #4 skills.sh exists, hosted directory → (c) issue draft
- #5 Tessl exists; hosted registry, not OCI → (a) ledger or doc text fix
- #6 ClawHub exists, hosted, 3,000+ skills, semantic search → (a) ledger or doc text fix
- #7 skillctl: Sigstore/cosign signing, narrower scope → (c) issue draft
- #8 jeffreytse/grimoire exists, 21 stars, Go, 1000+ skills across 26 domains → (a) ledger or doc text fix
- #9 Vaishnav-Sabari-Girish/grimoire: Rust task runner, v0.4.0 2026-07-02, active → (a) ledger or doc text fix
- #10 Vitale OCI spec: draft v0.1.0, Discussion #292, 2026-04-02 → (a) ledger or doc text fix
- #11 Arconia CLI (Java) and salaboy skills-oci (Go) both implement the spec, newer... → (a) ledger or doc text fix
- #12 openskills named among Show HN launches → (a) ledger or doc text fix
- #13 hesreallyhim/awesome-claude-code 50,947 stars → (a) ledger or doc text fix
- #14 Anthropic Agent Skills open standard Dec 2025, cross-vendor → (a) ledger or doc text fix
- #15 Comparable Tools structural prior art: ORAS, Helm OCI, lockfile UX, Claude Code... → (a) ledger or doc text fix

## Issue drafts

### Positioning: skills.sh lockfile and skillctl weaken two differentiators

The skills.sh README (https://github.com/vercel-labs/skills/blob/main/README.md) documents a `skills-lock.json` v2 with `computedHash` and `install --frozen-lockfile`, and skills.sh now supports 79 agents; vrothberg/skillctl (https://github.com/vrothberg/skillctl, created 2026-10-19) is an OCI-registry skill manager with signed versioned artifacts and project dependencies. Our positioning says skills.sh has no lockfile and skillctl is narrower (`.agents/research/research_promotion_positioning.md:98`, `:101`; `.claude/rules/product-context.md:133`). `.claude/rules/product-context.md` was not edited; the re-rank of differentiators is the owner's decision.

Labels: documentation

## Friction

- dropped (unsourced): Tessl "$125M raised", Podjarny founder, investors boldstart/GV/Index — no primary page fetched (tessl.io blog home returned 200 but no funding article located in budget).
- dropped (unsourced): Nesbitt skills-registry threat-model taxonomy (2026-06-03) — nesbitt.io returns 200, specific post not located.
- dropped (unsourced): Snyk audit "13.4% (534 of 3,984)" — Snyk primary report not located.
- dropped (unsourced): Paks, Askill, FastSkill, skill-get, Skilz (research_promotion_positioning.md:103) — repo searches returned only unrelated hits; no primary repo identified. skills named "Show HN" launches not re-verifiable.
- dropped (unsourced): ClawHub "3,000+ skills" total — listing page shows no total.
- dropped (unsourced): Dec 2025 cross-vendor adoption list (Microsoft, OpenAI, Atlassian, Figma, Cursor, GitHub) — not fetched.
- not reached: Claim 15 prior-art items (ORAS, Helm, lockfile UX, plugin marketplaces) — structural prior art, not checked.
- not reached: Tessl funding/founding; ClawHub OpenClaw ecosystem positioning text beyond homepage.
- feed: newest none (no feed), cursor found no
- note: Update Protocol items (product-context.md re-date, research_promotion_positioning.md rows 98, 101, 102 and differentiator re-rank) are left to the owner.

## Skill changes

Host caps concurrent subagents at 20; the researcher built its own claim list; replies are a short summary with the table in a file. Fixed in this run's chore(skills) commit.
