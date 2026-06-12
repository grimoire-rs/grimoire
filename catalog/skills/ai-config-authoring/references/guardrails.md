# AI Config Guardrails

> **Copy this file into your client's rules directory for always-on
> enforcement** — e.g. `.claude/rules/`, `.github/instructions/`, or
> reference it from `AGENTS.md`. It is self-contained by design and
> intentionally restates the budget table from its parent skill so it
> works with nothing else loaded.

When editing any AI agent configuration — skills, rules, agents,
instruction files — hold the change against this card.

## Context Budgets

| Artifact | Budget |
|---|---|
| Always-on instruction file | < 200 lines; every line passes the deletion test |
| Glob-scoped rule | < 200 lines each; glob matches ≥ 1 real file |
| Skill description | ≤ 1024 chars, single line, keywords front-loaded |
| Skill body (SKILL.md) | < 500 lines hard, ~250 healthy; depth in reference files |
| Bundled reference file | One topic; table of contents if > 100 lines |
| Hook | Zero context cost — prefer it for anything mechanical |

## Eight Anti-Patterns

1. **Always-on bloat** — lines that fail the deletion test ("would
   removing this cause mistakes?") tax every future task.
2. **Catch-all globs** — a rule scoped to `**/*` is an always-on file in
   disguise.
3. **Duplicated content across artifacts** — every fact lives in exactly
   one file; everything else points at it.
4. **Workflow summaries in descriptions** — a description must say *when*
   to use the artifact, never summarize its procedure; summaries become
   shortcuts that skip the body.
5. **Monolithic skill bodies** — reference material belongs in bundled
   files loaded on demand, not in the body.
6. **Dead globs** — after renames, scoped rules silently never fire;
   re-verify globs whenever paths move.
7. **Auto-invocable side effects** — workflows that deploy, publish, or
   delete must be manual-only, never description-triggered.
8. **Invariants entrusted to skills** — skill activation is
   probabilistic; anything that must happen every time belongs in a hook
   or permission setting.

## Single Source of Truth

One fact, one file. When the same rule appears in an instruction file, a
rule, and a skill, the copies *will* diverge and the model *will* pick
one arbitrarily. Designate the owner, link from everywhere else. The
only exception: an explicitly-marked standalone export — like this card.

## Further Reading

- [Effective context engineering for AI agents][ctx] — why every budget
  above exists.
- [Agent Skills specification][spec] — the normative limits for skills.
- [Skill authoring best practices][bp] — the anti-patterns in long form.

[ctx]: https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents
[spec]: https://agentskills.io/specification
[bp]: https://platform.claude.com/docs/en/agents-and-tools/agent-skills/best-practices
