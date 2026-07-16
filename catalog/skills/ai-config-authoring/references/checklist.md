# Pre-Publish Checklist

You loaded this file because you are reviewing a skill, rule, or agent
package before publishing it — or before installing someone else's.

## Budgets

- [ ] SKILL.md body < 500 lines (aim ~250); always-on files < 200 lines
- [ ] Description ≤ 1024 chars, single line, third person
- [ ] Reference files focused on one topic; TOC present in any file
      > 100 lines
- [ ] References one level deep from the root — no chains

## Description Trigger Test

- [ ] ≥ 3 should-trigger eval scenarios, phrased as real user utterances,
      activate the artifact
- [ ] ≥ 1 should-not-trigger neighbor scenario stays quiet
- [ ] A baseline run *without* the artifact shows it earns its tokens
- [ ] Description states *when* to use, never the workflow — no workflow
      verbs (dispatches/runs/orchestrates/performs/executes/handles)
      describing the artifact's own procedure

## Structure

- [ ] Frontmatter `name` equals the directory name (lowercase letters,
      digits, hyphens — periods work in grim but hurt portability;
      ≤ 64 chars; no edge or adjacent separators)
- [ ] Every relative link resolves to a file inside the package
- [ ] Every bundled file is referenced from the root with explicit
      when-to-read (or run-vs-read) guidance
- [ ] Scripts run non-interactively, document their dependencies, and
      emit helpful errors
- [ ] License declared

## No Duplication, No Leakage

- [ ] Each fact lives in exactly one file; the root routes, it does not
      restate (explicitly-marked standalone exports excepted)
- [ ] Zero project-internal leakage in shareable artifacts: no private
      repo paths, internal tool names, team workflow, or host names
- [ ] Vendor-specific facts are labeled as such; nothing depends on a
      vendor-only field to be *correct*

## Machine-Checkable Invariants

Structural properties worth automating as CI tests — test the contract,
not the content:

- **Name parity**: frontmatter name == directory name, format rules pass
- **Link resolution**: every markdown link in the package resolves
- **Line budgets**: per-artifact-type caps, with a justified-exception
  list rather than silent waivers
- **Dead-glob scan**: every scoped-rule glob matches ≥ 1 file on disk
- **Catalog parity**: every rule appears in the index file and every
  index row resolves to a rule — drift fails the build
- **Forbidden-verb regex**: descriptions rejected when workflow verbs
  describe the artifact's own procedure
- **Trigger uniqueness**: no two skills in a collection claim the same
  trigger phrases or overlapping niches
- **Forbidden-strings scan**: project-internal names never appear in
  artifacts marked shareable
- **Description-budget sum**: total description length across the
  collection stays under the client's listing budget

## Further Reading

- [Skill authoring best practices][bp] — the eval-first checklist this
  one descends from.
- [Agent Skills specification][spec] — the normative format rules; the
  `skills-ref` validator automates the format half.
- [Spec governance repo][gov] — home of the validator tooling.
- [A flagship community pack][superpowers] — trigger-assertion and
  behavioral test suites for skills, in CI.

[bp]: https://platform.claude.com/docs/en/agents-and-tools/agent-skills/best-practices
[spec]: https://agentskills.io/specification
[gov]: https://github.com/agentskills/agentskills
[superpowers]: https://github.com/obra/superpowers
