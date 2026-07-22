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

**Every client has an always-on file; per-file scoping is the minority
capability.** Of the ten clients surveyed (as of 2026; re-verify), four
support real glob scoping, one hosts per-file rules without scoping, and
five have no ownable per-file rule surface at all — for those five the
always-on file is the *only* place rule content can live.

| Per-file rule surface | Clients | What you get |
|---|---|---|
| Real glob scoping | [Claude Code][cc-mem] `.claude/rules/*.md` + `paths:`; [Copilot][cop-ci] `.github/instructions/*.instructions.md` + `applyTo:`; [Cursor][cur] `.cursor/rules/*.mdc` + a `globs` string; [Kiro][kiro] `.kiro/steering/*.md` + a `fileMatchPattern` list | Content loads only when a matching file is in play |
| Per-file, no scoping | [OpenCode][oc-rules] — rule files load through the always-on `instructions` array | The body loads, the scope does not: permanent cost |
| None | [Codex][cx-skills], [Junie][junie], [Gemini CLI][gem], [Zed][zed], [Amp][amp] | Nothing to install — route the content to the always-on file |

Per-client caveats worth knowing before you write a glob:

- **Cursor** splits its `globs` string on every comma, including one inside
  a `{a,b}` alternation — `src/**/*.{rs,toml}` is read as two patterns.
  Write one extension per glob (as of 2026; re-verify).
- **Kiro** honors `fileMatch` steering at project scope; the same file at
  user scope is written correctly but currently ignored ([kiro
  #9176][kiro-9176], as of 2026; re-verify).

Always-on hard limits: Copilot code review reads only the **first 4,000
characters** of copilot-instructions.md (as of 2026; re-verify), and
>~1,000 lines degrades all its surfaces; OpenCode has no size guard at all,
so an oversized AGENTS.md can force context compaction.

- AGENTS.md is the closest thing to a portable always-on baseline —
  OpenCode, Copilot, Codex, Zed, and Amp read the root file natively (Amp
  falls back to AGENT.md then CLAUDE.md). Claude Code does not read it
  natively (as of 2026); bridge with an import or a symlink. Gemini CLI uses
  its own GEMINI.md hierarchy, Kiro an always-on steering file, Cursor an
  unscoped always-apply rule, and Junie its own `.junie/AGENTS.md` — a
  client-specific path, not the portable root file.
- Porting scoped rules to OpenCode converts them into always-on cost;
  consider converting procedural rules into skills there instead.
- For the five clients with no rule surface, do not ship a rule file and
  hope — the content belongs in the always-on file, or in a skill if it is
  occasional.
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
- [Codex: skills][cx-skills] — one of the clients with no rule mechanism at
  all; AGENTS.md is its only always-on surface.
- [Cursor][cur] / [Kiro][kiro] — the two newer clients with real glob
  scoping; check both for the comma-split and user-scope caveats above.
- [Junie][junie] / [Gemini CLI][gem] / [Zed][zed] / [Amp][amp] — always-on
  instruction files only; re-check before assuming a rule installs.
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
[cur]: https://cursor.com
[kiro]: https://kiro.dev
[kiro-9176]: https://github.com/kirodotdev/Kiro/issues/9176
[junie]: https://www.jetbrains.com/junie/
[gem]: https://geminicli.com
[zed]: https://zed.dev
[amp]: https://ampcode.com
[vsc-ci]: https://code.visualstudio.com/docs/agent-customization/custom-instructions
[gh-blog]: https://github.blog/ai-and-ml/github-copilot/unlocking-the-full-power-of-copilot-code-review-master-your-instructions-files/
[humanlayer]: https://humanlayer.dev/blog/writing-a-good-claude-md
[agentsmd]: https://agents.md
[eth]: https://arxiv.org/abs/2602.11988
