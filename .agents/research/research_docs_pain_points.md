# Research: pain points that decide which use cases the docs lead with

**Date:** 2026-09-06
**Question:** which reader tasks should front grimoire.rs, decided by what people
complain about online and what comparable tools lead with, rather than by the
owner's list alone.
**Method:** two web-research subagents (community voices; comparable tools),
evidence-only, dates verified on the source pages. Synthesis at the end maps
each pain to a discovery task in `.agents/discovery/use-cases.yaml`.
**Consumers:** `use-cases.yaml` (`pain_points`, `ia_plan.router`), the landing
router, the guide copy.

## A. Community voices, ranked by distinct sources

| # | Pain point (their words) | Sources | Evidence | Task |
|---|---|---|---|---|
| 1 | CLAUDE.md and AGENTS.md diverge across tools and repos; teams maintain both or hand-bridge | 8+ | [claude-code#6235](https://github.com/anthropics/claude-code/issues/6235) (2025-08-21, 3,020 upvotes on the parent), [claude-code#31005](https://github.com/anthropics/claude-code/issues/31005) (2026-03-05, 469 reactions: "7 months. 3,020 upvotes. Zero acknowledgment."), [HN 49367350](https://news.ycombinator.com/item?id=49367350) (2026-08-19: "set up your skills, rules, commands etc for claude in their own special place") | T04, T07 |
| 2 | Claude Code reads `.claude/skills` while the Agent Skills standard says `.agents/skills`; one skill cannot live in one place | 3 | [claude-code#31005](https://github.com/anthropics/claude-code/issues/31005), [drift-patterns gist](https://gist.github.com/yurukusa/d36197848911f025add142abefcde685) (2026-05-26, recommends symlinking), [vercel-labs/skills#395](https://github.com/vercel-labs/skills/issues/395) (2026-02-19) | T04 |
| 3 | Every MCP-capable tool has its own config schema, root key and file; the same server list is hand-maintained N times | 5 | [modelcontextprotocol#2218](https://github.com/modelcontextprotocol/modelcontextprotocol/discussions/2218) (2026-02-06: "each one invented its own schema, root key, field names, and file location"), [dev.to chezmoi](https://dev.to/dotwee/one-mcp-configuration-for-codex-claude-cursor-and-copilot-with-chezmoi-925), [platform.uno](https://platform.uno/blog/mcp-configuration-across-ai-agents/) | T19 |
| 4 | A CLI action silently rewrites settings files and drops hand-added fields or permissions | 3 | [claude-code#22659](https://github.com/anthropics/claude-code/issues/22659) (2026-02-03), [#9234](https://github.com/anthropics/claude-code/issues/9234), [#41259](https://github.com/anthropics/claude-code/issues/41259) | T08 |
| 5 | No built-in way to keep config, hooks and skills in sync across one person's machines, let alone a team; a repo-only skill leaks into global | 3 | [claude-code#36693](https://github.com/anthropics/claude-code/issues/36693) (2026-03-20, closed not-planned), [claude-sync](https://github.com/baptisterajaut/claude-sync) | T03 |
| 6 | The agent ignores the instruction file anyway, so it is "just another source of truth to sync" | 4 | [claude-code#18411](https://github.com/anthropics/claude-code/issues/18411), [HN 47481682](https://news.ycombinator.com/item?id=47481682) (2026-03-22), [HN 49367350](https://news.ycombinator.com/item?id=49367350) | gap (runtime fidelity) |
| 7 | Installing a third-party skill runs untrusted model-steering text with no way to see what it does first | 3 | [Datadog Security Labs](https://securitylabs.datadoghq.com/articles/malicious-skills-supply-chain-risks-in-coding-agents-with-dynamic-context/) (2026-05-11), [vercel-labs/skills#617](https://github.com/vercel-labs/skills/issues/617) (RFC: installs "without any verification"), [Red Hat Developer](https://developers.redhat.com/articles/2026/08/18/securing-claude-code-plug-ins-best-practices-repository-security) (2026-08-18) | T14 |
| 8 | A marketplace skill often makes output worse; you find out after paying the context tax | 3 | "installed all 47 skills… forty made the output worse" (ksred.com), [Medium, Jul 2026](https://medium.com/ai-all-in/a-bad-claude-skill-is-worse-than-no-skill-heres-the-rubric-3b19b6b80e00), [dev.to audit](https://dev.to/thestack_ai/i-audited-214-claude-code-skills-73-were-silently-broken-2m9a) ("73% were silently broken") | T14, T15 |
| 9 | Teams want a lockfile so CI can prove the shipped skills are the reviewed ones | 3 independent tools built for it | [pcomans/skills-lock](https://github.com/pcomans/skills-lock), [skills-lock/skil-lock](https://github.com/skills-lock/skil-lock), [agent.lock essay](https://2amsecurity.substack.com/p/where-is-my-agentlock-file) | T09, T28 |
| 10 | Public marketplaces cannot host private company skills; per-repo commits do not scale | 2 (+80 reactions) | [vercel-labs/skills#381](https://github.com/vercel-labs/skills/issues/381) (open, highest-reacted issue on skills.sh) | T10, T06, T11 |

**Wanted, unserved anywhere:** one config every client reads natively (MCP discussion #2218); settings-sync across own machines (#36693, closed); signature/provenance on install (#617); sanctioned private distribution (#381); a CI-enforceable lockfile for the whole capability surface (three competing hobby tools).

## B. Comparable tools: what they lead with, what their users complain about

| Tool | Leads with | Nav shape | Top complaints |
|---|---|---|---|
| Vercel skills.sh | "reusable capabilities… install with a single command" (feature) | Overview, CLI, Packs, API, FAQ, Customize | private skills [#381](https://github.com/vercel-labs/skills/issues/381) 80; restore from lockfile [#283](https://github.com/vercel-labs/skills/issues/283)/[#549](https://github.com/vercel-labs/skills/issues/549) 54+46; not linked into Claude's dir [#744](https://github.com/vercel-labs/skills/issues/744) 36; project installs untracked by lock [#155](https://github.com/vercel-labs/skills/issues/155) 35; versioning RFC [#11](https://github.com/vercel-labs/skills/issues/11) 27 |
| Tessl | persona routing (security leaders, platform teams) | Getting Started, Tutorials, Administration, Using, Creating, Distribution, Reference | closed SaaS; sells Snyk scoring and private-by-default publishing |
| ClawHub | by target app | — | no review path [#2272](https://github.com/openclaw/clawhub/issues/2272); command-injection takedown [#3533](https://github.com/openclaw/clawhub/issues/3533) |
| skillctl | quickstart, then "no npm install for skills" | — | tiny; explicit project/user split, per-agent symlinks |
| jeffreytse/grimoire | "Why", "Skills as packages", linting | — | 25★; scope priority session > project > global > system; two-tier trust |
| anthropics/skills | reference repo | — | namespace spoofing under anthropic/ [#492](https://github.com/anthropics/skills/issues/492) 43 comments; org-wide sharing [#228](https://github.com/anthropics/skills/issues/228); 0% trigger rate [#556](https://github.com/anthropics/skills/issues/556) |
| agentskills.io | what/why/how | — | no install folder standard [#15](https://github.com/agentskills/agentskills/issues/15), `.well-known` discovery [#255](https://github.com/agentskills/agentskills/issues/255) 20, monorepo discovery [#115](https://github.com/agentskills/agentskills/issues/115); OCI packaging [discussion #292](https://github.com/agentskills/agentskills/discussions/292) |
| Claude Code plugin marketplaces | team framing: "centralized discovery, version tracking, automatic updates" | Overview → Walkthrough | `marketplace.json` has a `renames` map (rename/deprecation answer) |
| Cursor rules | by tech stack (cursor.directory) | — | rules silently not applying ([forum](https://forum.cursor.com/t/cursor-rules-completely-broken/92554)); `.cursorrules` vs `.mdc` migration with no deprecation signal |
| rulesync | tool-support matrix | Installation, Getting Started, Supported Tools | every top issue is "vendor changed its format" |
| dotagents (709★) | "One canonical .agents folder that powers all your AI tools" | Quick Start, What it does, Where it links | hooks migration data loss [#9](https://github.com/iannuttall/dotagents/issues/9); "what about MCP configs?" [#11](https://github.com/iannuttall/dotagents/issues/11) |
| Helm (prior art) | registry mechanics; digest pinning late | — | — |
| ocx.sh (prior art) | task-first: Installation, Docker, Getting Started, User Guide, Authoring, In Depth, Reference, FAQ | — | versioning page: problem first, tags vs digests, then the two-part stability guarantee |

Synthesis verdicts (comparables worker): widely felt = lockfile restore, project vs global scope, private registry, cross-client sync (structurally unsolved, every tool chases vendor changes), rename/deprecation, where-skills-live, trust/provenance. Niche = inter-package dependencies. Doc-clarity model, not a pain = the versioning explanation shape.

## C. Synthesis: router decision

Three sources agree or disagree per use case. Owner = what colleagues said they do not understand; logs = the ten friction logs; web = sections A and B.

| Use case | Owner | Logs | Web | Router |
|---|---|---|---|---|
| Install and first skill | yes | rank 1 | skills.sh #744 (files not where the client reads) | 1 |
| One skill set across harnesses | yes | rank 6 | A1, A2 (strongest signal found: 3,020 upvotes), B cross-client sync | 2 |
| Scope and clients | yes | ranks 5, 9 | A5, B project-vs-global widely felt | 3 |
| Know what a skill does before it lands (vet; TUI as the means) | yes (as "TUI") | rank 8 | A7, A8, B trust widely felt and unaddressed | 4 |
| One MCP server definition for every agent | no | not run | A3 (5 sources), dotagents #11 | 5 (new) |
| Keep installs current without losing edits | yes | rank 7 | A4, B rename/deprecation | 6 |
| Floating tags, pinned digests, rolling releases | yes | not run | A9 adjacent; ocx as the shape | 7 |
| Pin versions for team and CI | yes | rank 3 | A9, B lockfile restore = top skills.sh cluster | 8 |
| Run your own index (private, nothing to host) | yes | rank 2 | A10, skills.sh #381 (80 reactions) | 9 |
| Catalog layout best practices | yes | T10 note | agentskills #115 (monorepo discovery), spec audience | sidebar + a step in the own-index tutorial |
| Company index beside the public one | no | rank 4 | A10 | sidebar |

Dropped from the router against the owner's pick: catalog layout (publisher-side, weaker external signal, and it belongs inside the own-index path as a step). Added against the owner's pick: MCP everywhere (five independent sources, a shipped grim capability, no page written for the job).

## Durable search terms

AGENTS.md CLAUDE.md sync multiple repositories drift · Claude Code skills .claude/skills vs .agents/skills · MCP server config duplicated claude codex cursor · prompt injection third-party skill marketplace trust · sync tool overwrote hand-edited settings.json · team lockfile AI agent skills CI enforce · vercel-labs/skills issues discovery trust · agent skills well-known URI · claude code marketplace.json renames · cursor .mdc rules alwaysApply migration · rulesync vendor format drift · dotagents symlink AI config · helm oci digest pinning · ocx.sh in-depth versioning
