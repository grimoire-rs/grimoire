# Research: Artifact dependencies — community demand

## Metadata

**Date:** 2026-11-01
**Domain:** packaging
**Triggered by:** `.agents/discussions/artifact-dependencies.md` — community lane
**Expires:** 2027-05-01

## Findings

**Use cases**
- **A skill needs a sibling skill.** For example, gws-drive says "Read `../gws-shared/SKILL.md`", and a selective install leaves that path dangling ([vercel-labs/skills#860](https://github.com/vercel-labs/skills/issues/860), [#1438](https://github.com/vercel-labs/skills/issues/1438), [agentskills#46](https://github.com/agentskills/agentskills/issues/46)).
- **Shared reference content** ([vercel-labs/skills#515](https://github.com/vercel-labs/skills/issues/515)). One marketplace copies 32 agents across 7 plugins ([claude-code#9444](https://github.com/anthropics/claude-code/issues/9444)).
- **A skill needs tools or MCP servers** ([agentskills#485](https://github.com/agentskills/agentskills/issues/485), [#330](https://github.com/agentskills/agentskills/issues/330)). Codex declares `dependencies.tools` (type mcp) in `agents/openai.yaml` ([codex#57093](https://github.com/openai/codex/issues/57093)).
- **System dependencies** such as Chrome, poppler and pip packages ([agentskills#46](https://github.com/agentskills/agentskills/issues/46)).

**Spec status**
- **agentskills.** The requests are open ([#100](https://github.com/agentskills/agentskills/issues/100), [#110](https://github.com/agentskills/agentskills/issues/110), [#485](https://github.com/agentskills/agentskills/issues/485)) or closed without a field ([#90](https://github.com/agentskills/agentskills/issues/90), [#330](https://github.com/agentskills/agentskills/issues/330)).
- **Maintainer position.** Dependencies belong in a distribution-layer manifest, not in SKILL.md ([#110](https://github.com/agentskills/agentskills/issues/110)). A manifest-and-lock proposal is open as [discussion 210](https://github.com/agentskills/agentskills/discussions/210).
- **vercel-labs/skills.** Four open requests, with no maintainer reply seen.
- **Claude Code.** [#9444](https://github.com/anthropics/claude-code/issues/9444) was closed as shipped on 2026-08-17 (about 49 thumbs-up).

**Pain from shipped support**
- **Claude Code.**
  - `plugin update` skips newly declared dependencies ([#88663](https://github.com/anthropics/claude-code/issues/88663)).
  - Synced plugins can't satisfy dependencies ([#99959](https://github.com/anthropics/claude-code/issues/99959)).
  - Unhelpful version error ([#104128](https://github.com/anthropics/claude-code/issues/104128)).
  - Undocumented auto-install ([#48864](https://github.com/anthropics/claude-code/issues/48864)).
  - Teams objected when checked-in `enabledPlugins` stopped auto-installing ([#73522](https://github.com/anthropics/claude-code/issues/73522)).
- **APM.**
  - Orphaned transitive dependencies are kept ([#3401](https://github.com/microsoft/apm/issues/3401)).
  - A transitive dependency is silently dropped with exit 0 ([#2815](https://github.com/microsoft/apm/issues/2815)).
  - Same-name collisions are undefined ([#2413](https://github.com/microsoft/apm/issues/2413)).

**Versions vs presence.** Most requests ask only that a dependency be present. Version pinning comes from enterprise and team asks ([#110](https://github.com/agentskills/agentskills/issues/110), [claude-code#63986](https://github.com/anthropics/claude-code/issues/63986)).

negative:
- No HN thread found.
- No opencode request found.
- No vercel-labs maintainer response found.

leads:
- [agentskills discussion 210](https://github.com/agentskills/agentskills/discussions/210).
- [modelcontextprotocol PR 2640](https://github.com/modelcontextprotocol/modelcontextprotocol/pull/2640).
- [microsoft/apm#2270](https://github.com/microsoft/apm/issues/2270).
