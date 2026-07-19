# Updating This Guide

You loaded this file because you maintain the ai-config-authoring package
and need to refresh its claims against current vendor reality.

## Re-Research Procedure

1. **Re-fetch the primary sources** (Further Reading below) and diff
   against what this package claims — especially activation mechanics,
   scan paths, and frontmatter fields.
2. **Check the spec and client changelogs** for new fields, new artifact
   types, or changed defaults since the last revision.
3. **Re-survey flagship community packs** for norm drift: measured body
   lengths, frontmatter usage in the wild, description conventions.
4. **Re-verify every hard number before trusting it** — see below.

## Hard Numbers Drift Fastest

Limits, rates, and caps decay first: truncation thresholds, listing
budgets, character caps, activation-rate studies, adopter counts.
Everything marked "(as of 2026; re-verify)" is suspect after roughly six
months. The principles — progressive disclosure, the deletion test,
triggering-conditions-only descriptions, hooks for invariants — are
durable; update the numbers, keep the spine.

## Durable Search Terms

- `agentskills.io specification frontmatter`
- `skills-ref validate agentskills`
- `claude code memory rules paths frontmatter site:code.claude.com`
- `claude skill description triggering undertriggering overtriggering`
- `opencode rules instructions glob always loaded`
- `copilot custom instructions support matrix applyTo instructions.md`
- `codex skills subagents AGENTS.md site:developers.openai.com`
- `AGENTS.md standard adoption nearest file precedence`
- `skills not activating subagent headless`
- `anthropic effective context engineering attention budget`

## Further Reading

- [Agent Skills specification][spec] — re-check format constraints first.
- [Claude Code: memory][cc-mem] / [skills][cc-skills] /
  [sub-agents][cc-agents] / [hooks][cc-hooks] + the [changelog][cc-log].
- [OpenCode: rules][oc-rules] / [skills][oc-skills] / [agents][oc-agents].
- [Copilot: instructions support matrix][cop-matrix] /
  [agent skills][cop-skills] — surface support changes often.
- [Codex: skills][cx-skills] / [subagents][cx-agents] — AGENTS.md-only
  always-on surface, no rule mechanism; re-verify both claims here.
- [Skill authoring best practices][bp] and the
  [official example skills][repo] — re-measure body-length norms there.

[spec]: https://agentskills.io/specification
[cc-mem]: https://code.claude.com/docs/en/memory
[cc-skills]: https://code.claude.com/docs/en/skills
[cc-agents]: https://code.claude.com/docs/en/sub-agents
[cc-hooks]: https://code.claude.com/docs/en/hooks
[cc-log]: https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md
[oc-rules]: https://opencode.ai/docs/rules/
[oc-skills]: https://opencode.ai/docs/skills/
[oc-agents]: https://opencode.ai/docs/agents/
[cop-matrix]: https://docs.github.com/en/copilot/reference/custom-instructions-support
[cop-skills]: https://docs.github.com/en/copilot/concepts/agents/about-agent-skills
[cx-skills]: https://developers.openai.com/codex/skills
[cx-agents]: https://developers.openai.com/codex/subagents
[bp]: https://platform.claude.com/docs/en/agents-and-tools/agent-skills/best-practices
[repo]: https://github.com/anthropics/skills
