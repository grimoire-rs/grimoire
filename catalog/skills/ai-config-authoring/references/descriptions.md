# Description Craft

You loaded this file because a skill or agent fails to trigger (or
triggers too often), or because you are finalizing a description before
publishing.

## Why Descriptions Dominate

The name and description are the **only always-loaded content** of a
skill or agent — the model decides whether to load the body, or to
delegate, from the description alone. A weak description means the body
never runs: measured baselines show ~50% activation with vague
descriptions, and explicit trigger conditions improved activation ~20x
in a 650-trial study (as of 2026; re-verify).

## The Formula

**What it does + "Use when…" + the words users actually say.** Third
person, key use case first.

- Canonical shape: `Extract text and tables from PDF files, fill forms,
  merge documents. Use when working with PDF files or when the user
  mentions PDFs, forms, or document extraction.`
- Pack the "Use when" clause with user-utterance keywords: file
  extensions, command names, error strings, synonyms.
- Add negative scope when neighbors exist: "Do NOT use for spreadsheets —
  use the xlsx skill instead."
- Be a little pushy — models undertrigger by default.

## The CSO Rule

**Triggering conditions only — never a workflow summary.** A description
that summarizes the procedure becomes a shortcut: the model follows the
summary and skips the body. Documented field case: a description saying
"code review between tasks" caused exactly one review, even though the
skill body's flowchart required two — the summary replaced the skill.

The description answers *when should this load*; the body answers
*what to do*. Keep the two strictly separated.

## Forbidden Pattern: Workflow Verbs

Descriptions that say what the skill *does* step-by-step instead of
*when to use it* are the most common CSO violation. Treat these verbs as
red flags when they describe the skill's own procedure:

> dispatches · runs · iterates · orchestrates · performs · executes · handles

"Runs the linter, fixes findings, and re-runs until clean" reads as a
recipe — the model will follow those three steps from the description
and never load the body where the real procedure (and its edge cases)
lives.

## Truncation and Budgets

- Spec cap: 1–1024 characters, non-empty, no XML tags (open standard).
- Listing truncation: at least one client cuts the displayed description
  at 1,536 characters, and all descriptions share a combined listing
  budget — overflow silently drops the least-used entries (as of 2026;
  re-verify). **Front-load discriminating keywords**: truncation cuts
  from the end.
- Keep the description a single line — block scalars concatenate or
  truncate unpredictably across parsers.

## Good/Bad Pairs

| Bad | Why it fails | Good |
|---|---|---|
| "Helps with documents" | No keywords, no when-clause — coin-flip activation | "Extract text and tables from PDF files, fill forms, merge documents. Use when working with PDF files or when the user mentions PDFs, forms, or document extraction." |
| "Runs the release flow: bumps the version, builds, publishes, tags" | Workflow summary — the model follows the four verbs and skips the body | "Release workflow for this project. Use when cutting a release, publishing a new version, or when the user mentions tags or changelogs." |
| "I can help you process spreadsheets" | First/second person breaks discovery — descriptions are injected into the system prompt | "Processes Excel files and generates reports. Use when working with .xlsx files, pivot tables, or spreadsheet data." |

## Tuning Trigger Rate

- **Undertriggering** → add keywords and technical synonyms; make the
  when-clause pushier; put the key use case first.
- **Overtriggering** → narrow the scope; add negative triggers ("not for
  X — use the Y skill instead"); for side-effectful workflows, make the
  skill manual-only instead of tuning the description.
- Test both directions: ≥3 should-trigger phrasings *and* should-not-
  trigger neighbors (see [checklist.md](checklist.md)).

## Further Reading

- [Skill authoring best practices][bp] — the official what+when formula,
  third-person rule, named bad examples.
- [Agent Skills specification][spec] — the 1024-char constraint and
  description requirements.
- [A flagship community pack's writing-skills doctrine][cso] — the CSO
  rule and the one-review-instead-of-two field case.
- [Skills not triggering? The client might not see them][fsck] — silent
  listing-budget truncation as a failure mode.
- [Writing tools for agents][tools] — the same description craft applied
  to tool definitions.

[bp]: https://platform.claude.com/docs/en/agents-and-tools/agent-skills/best-practices
[spec]: https://agentskills.io/specification
[cso]: https://github.com/obra/superpowers
[fsck]: https://blog.fsck.com/2025/12/17/claude-code-skills-not-triggering-it-might-not-see-them/
[tools]: https://www.anthropic.com/engineering/writing-tools-for-agents
