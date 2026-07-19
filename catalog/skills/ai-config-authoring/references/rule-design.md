# Rule Design

You loaded this file because you are writing an always-on instruction file
(CLAUDE.md, AGENTS.md, copilot-instructions.md) or a glob-scoped rule, or
because an existing one has grown past its budget.

Contents: [Two Activation Modes](#two-activation-modes) ·
[The 200-Line Budget](#the-200-line-budget) ·
[Rules Are Advisory](#rules-are-advisory) ·
[Path-Scoping Discipline](#path-scoping-discipline) ·
[Tree Hierarchy + Catalog](#tree-hierarchy--catalog) ·
[Vendor Differences](#vendor-differences) ·
[The Eager-Import Trap](#the-eager-import-trap)

## Two Activation Modes

| Mode | Loads | Use for |
|---|---|---|
| Always-on | At session start, every session | Identity, commands, conventions relevant to every task |
| Glob-scoped | When a matching file enters the session | Standards needed while editing those files |

Minimize always-on content; prefer scoping. A scoped rule costs nothing
until a matching file is touched — it is the cheapest way to carry
per-area standards.

## The 200-Line Budget

Target under 200 lines per always-on file (vendor guidance, as of 2026;
re-verify). The evidence that adherence decays with length:

- Official guidance: longer files "consume more context and reduce
  adherence"; an oversized file gets half-ignored.
- Practitioner measurement: frontier models follow roughly 150–200
  instructions consistently (as of 2026; re-verify) — and the client's own
  system prompt already spends a sizable share of that.
- Controlled study: LLM-*generated* repo context files reduced task
  success ~3% versus no file at all, while raising cost over 20%;
  human-written files gained ~4%. Quality and brevity beat volume.

Apply the deletion test per line: would removing it cause mistakes? If
not, cut it. Prefer specificity that is verifiable ("use 2-space
indentation") over vibes ("format code properly"). Two contradicting
rules → the model picks one arbitrarily.

## Rules Are Advisory

Instruction files are context, not enforcement. They are requests the
model usually honors — one community measurement found ~70% compliance
for a prohibition line versus 100% for the equivalent hook (as of 2026).
For anything that must happen every single time, use a hook or the
client's permission system; keep the rule as the explanation of *why*.
See [choosing-types.md](choosing-types.md) for the full boundary.

## Path-Scoping Discipline

- One topic per rule file; name the file for the concern it covers.
- Scope narrowly. A catch-all glob (`**/*` or a whole source tree) is
  just another always-on file wearing a costume.
- **Dead-glob hazard**: after a directory rename, scoped rules silently
  never fire again — there is no error, just absence. Verify every glob
  still matches at least one file, and automate the check (see
  [checklist.md](checklist.md)).
- Vendor gap (Claude Code, as of 2026): scoped rules fire when a matching
  file is *read*, not when one is *created* — do not rely on them for
  new-file conventions.
- Scoped rules do not transfer into spawned subagents on any client.

## Tree Hierarchy + Catalog

Scoped rules have a structural blind spot: planning. During architecture,
research, or estimation no file is open, so nothing fires. Close the gap
with a tree: always-on root → catalog/index file → scoped leaf rules.

- The **catalog file** lists every rule by concern — "if you care about X,
  read rule Y" — so rules are discoverable *before* any file opens. The
  always-on root references the catalog instead of inlining the rules.
- Add an **activation nudge** to the root: "Before starting a task,
  identify which rules below are relevant and read them first." Index
  files without a nudge get skipped.
- **Parity protocol**: any rule added, removed, or renamed updates the
  catalog in the same commit. Enforce this with a structural test, or the
  catalog silently rots into a liability.
- **Declare overlaps**: when two rules intentionally share a glob,
  document the pair. An undeclared overlap is usually a scoping mistake.

## Vendor Differences

| Concern | Claude Code | OpenCode | Copilot |
|---|---|---|---|
| Always-on file | CLAUDE.md (directory hierarchy + imports) | AGENTS.md; CLAUDE.md as fallback; `instructions` array | copilot-instructions.md + AGENTS.md/CLAUDE.md |
| Glob scoping | `.claude/rules/*.md` with `paths:` — lazy, fires on read | None — `instructions` globs resolve at startup, always-on | `.github/instructions/*.instructions.md` with `applyTo:` |
| Hard limits | Listing/compaction budgets (as of 2026) | No size guard — an oversized AGENTS.md can force context compaction | Code review reads only the **first 4,000 characters** of copilot-instructions.md (as of 2026; re-verify); >~1,000 lines degrades all surfaces |

- Claude Code does not read AGENTS.md natively (as of 2026); bridge with
  an import or a symlink. OpenCode and Copilot read it natively —
  AGENTS.md is the closest thing to a portable always-on baseline.
- Porting scoped rules to OpenCode converts them into always-on cost;
  consider converting procedural rules into skills there instead.
- Codex has no rule mechanism at all, scoped or otherwise — a rule
  installed with `--client codex` is declined with a warning and writes
  no file. Codex's only always-on surface is its directory-granular
  AGENTS.md; route content there instead.
- Write for the worst consumer: most-critical content first, short
  imperative bullets, and never rely on the client fetching external URLs
  as normative content.

## The Eager-Import Trap

Import syntax (`@path/to/file` in Claude Code) organizes content but does
not defer it: imported files expand at launch and cost full context, every
session. The trap generalizes — any force-loading link is always-on cost
in disguise. For optional depth, write a plain pointer instead ("read
`docs/style.md` when touching templates"); the model loads it only when
relevant. Imports are for content that genuinely belongs in every
session and merely lives in another file.

## Further Reading

- [Claude Code: memory and rules][cc-mem] — the under-200-lines guidance,
  `paths:` scoping mechanics, the advisory-vs-enforced framing, import
  semantics.
- [Claude Code: best practices][cc-bp] — the deletion test and the
  include/exclude table for instruction files.
- [OpenCode: rules][oc-rules] — AGENTS.md discovery, the always-on
  `instructions` array.
- [Copilot: custom instructions][cop-ci] — file types and precedence
  across surfaces.
- [Codex: skills][cx-skills] — the client with no rule mechanism at all;
  AGENTS.md is its only always-on surface.
- [VS Code: custom instructions][vsc-ci] — `applyTo:` mechanics and the
  documented mismatch failure mode.
- [Copilot code review instructions deep-dive][gh-blog] — the 4,000-char
  limit and length-degradation guidance.
- [Writing a good always-on file][humanlayer] — the instruction-count
  ceiling argument and pruning discipline.
- [AGENTS.md][agentsmd] — the cross-vendor instruction-file standard.
- [Evaluating AGENTS.md (ETH Zurich)][eth] — empirical evidence that
  generated context files can be net-negative.

[cc-mem]: https://code.claude.com/docs/en/memory
[cc-bp]: https://code.claude.com/docs/en/best-practices
[oc-rules]: https://opencode.ai/docs/rules/
[cop-ci]: https://docs.github.com/en/copilot/customizing-copilot/adding-custom-instructions-for-github-copilot
[cx-skills]: https://developers.openai.com/codex/skills
[vsc-ci]: https://code.visualstudio.com/docs/agent-customization/custom-instructions
[gh-blog]: https://github.blog/ai-and-ml/github-copilot/unlocking-the-full-power-of-copilot-code-review-master-your-instructions-files/
[humanlayer]: https://humanlayer.dev/blog/writing-a-good-claude-md
[agentsmd]: https://agents.md
[eth]: https://arxiv.org/abs/2602.11988
