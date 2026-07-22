# Choosing an Artifact Type

You loaded this file because you are deciding whether a piece of agent
configuration should be an always-on instruction, a glob-scoped rule, an
on-demand skill, a subagent, or a hook — or because existing content has
outgrown its current type and needs to move.

Contents: [The Decision Table](#the-decision-table) ·
[Decision Heuristics](#decision-heuristics) ·
[Where Vendors Disagree](#where-vendors-disagree) ·
[Migration Paths](#migration-paths)

The five types span two axes: **when content enters context** (always → on
file match → on task match → on delegation → never) and **how reliably it
applies** (deterministic → advisory). The one-line rule: invariants in
hooks, identity in the always-on file, file-local standards in scoped
rules, occasional know-how in skills, context-hungry or
privilege-separated work in subagents.

## The Decision Table

Mechanics — activation and cost:

| Type | Activation model | Context cost |
|---|---|---|
| Always-on instruction file (CLAUDE.md, AGENTS.md, copilot-instructions.md, unscoped rules) | Unconditional at session start; import syntax expands at launch, so splitting saves nothing | Full cost every session, every turn — a permanent attention tax |
| Glob-scoped rule | Fires on file match; semantics differ sharply per vendor (see below) | Zero until a matching file is touched, then the rule body for the session |
| On-demand skill | Two-stage: name + description preloaded for routing; body loads on description match or explicit invocation; bundled files only on access | ~100 tokens/skill metadata always; body (< 5k tokens) only on invocation; bundled files free until read |
| Subagent | Model-delegated on description match, or explicitly invoked by name | Main context pays only the returned summary; total spend rises — multi-agent runs ~15x the tokens of a single chat (as of 2026; re-verify) |
| Hook | Deterministic, event-fired (pre/post tool use, session start/stop); never model-invoked | Zero context for the script itself; only injected output costs tokens |

Fit — what each type is for:

| Type | Best for | Poor fit |
|---|---|---|
| Always-on instruction file | Project identity, build/test commands, conventions relevant to every task | Occasional workflows; long reference material; anything needing guaranteed application — it is advisory |
| Glob-scoped rule | Per-language / per-subsystem standards needed while editing matching files (e.g. a Rust style rule on `**/*.rs`) | Guidance needed during planning before any file opens; enforcement; subagent contexts (rules do not transfer) |
| On-demand skill | Occasional procedures and domain knowledge; workflows with bundled scripts; anything that must port across clients | Must-always-apply invariants — activation is probabilistic; headless automation modes |
| Subagent | Context-heavy research; parallel workstreams; least-privilege tool sets; per-role model selection | Tasks needing the parent conversation's nuance (summaries are lossy); quick single lookups (spawn overhead) |
| Hook | Invariants: format-on-save, lint gates, blocking writes to protected paths, audit logging | Anything needing judgment; a *hard* security boundary — use the client's permission system for that |

Reality — support and failure modes:

| Type | Vendor support | Failure modes |
|---|---|---|
| Always-on instruction file | Universal — every client has one, under a different name: CLAUDE.md (Claude Code, hierarchy + imports), AGENTS.md (OpenCode, Codex, Zed, Amp — Amp falls back to AGENT.md then CLAUDE.md), copilot-instructions.md plus AGENTS.md/CLAUDE.md (Copilot), GEMINI.md (Gemini CLI), always-on steering (Kiro), an unscoped always-apply rule (Cursor), `.junie/AGENTS.md` (Junie — a client-specific path, not the root file) — all as of 2026; re-verify | Adherence collapses with size: ~150–200-instruction consistency ceiling measured; oversized files get half-ignored. A controlled study found LLM-*generated* context files net-negative (−3% task success, +20% cost) while human-written gained ~4% (as of 2026) |
| Glob-scoped rule | A minority capability — see [the grouping below](#where-vendors-disagree). Real per-file scoping on four clients (Claude Code, Copilot, Cursor, Kiro); a per-file surface without scoping on OpenCode; five clients have no ownable per-file rule file at all (as of 2026; re-verify) | Dead globs silently never fire after renames; glob mismatch is the primary documented load-failure cause (Copilot); invisible during planning; porting to a client without scoping either drops the scope or converts it to always-on cost |
| On-demand skill | Universal — the only type every client hosts, via the Agent Skills open standard (~35 adopters, as of 2026; re-verify). Discovery directories differ, and four clients share one pool — see [skill-design.md](skill-design.md) | Silent non-activation: ~50% baseline trigger with weak descriptions; 73% of 214 audited community skills never fired; 0% auto-activation inside spawned subagents (all as of 2026; re-verify) |
| Subagent | Six clients ship an installable agent file, in six incompatible envelopes; four have no installable format at all — the per-client table is in [agent-design.md](agent-design.md) (as of 2026; re-verify) | Over-summarization loses cross-domain context; skills and rules do not auto-fire inside; cost multiplies linearly with parallelism; an agent file written for a client with no format is simply never read |
| Hook | Claude Code: shell commands + exit-code protocol. OpenCode: JS/TS plugins (can throw to cancel a tool call). Copilot: declarative JSON in `.github/hooks/` | Not a security boundary: condition filters fail open, blocked tools get routed around, and some headless/pipe modes skip hooks entirely (as of 2026) |

## Decision Heuristics

Ask in order; the first decisive answer picks the type.

1. **Must it happen every time, zero exceptions?** Mechanical → hook.
   Needs judgment → one terse always-on line (accept that it is advisory),
   optionally backed by a hook that checks the outcome.
2. **Is it deterministic?** Formatting, linting, blocking, logging → hook
   (zero context cost). Needs interpretation → a prose artifact.
3. **How often is it relevant?** Every task → always-on (then apply the
   deletion test per line). While editing certain files → scoped rule.
   Occasionally, by topic → skill. Only on explicit request, or has side
   effects → manual-only skill.
4. **Must it fire unprompted?** Always-on files and hooks fire
   unconditionally; scoped rules fire on file match; skills fire on a
   *probabilistic* description match — never rely on a skill for something
   that must not be missed.
5. **Does the work need isolation, parallelism, or different
   privileges/model?** → subagent. Merely occasional knowledge → skill,
   far cheaper.
6. **Knowledge or capability?** Prose someone must read → rule or skill.
   Logic a machine can run → hook, or a script bundled inside a skill
   (executable code never enters context). Live *tool access* (query a
   service, call an API mid-conversation) → an MCP server — not a config
   content type at all, but wiring for one; grim distributes the server
   *registration* as its own artifact kind (`mcp` descriptors) so a
   skill that assumes a server can ship next to it.
7. **Must it work across vendors?** → skills for workflows: the skill is
   the only universally installable type. AGENTS.md is the widest *always-on*
   baseline but not a universal one — Claude Code, Cursor, Kiro, Gemini CLI,
   and Junie each read their own file instead, so the baseline needs a
   per-client wrapper, import, or symlink. Vendor-native rules and hooks are
   lock-in surfaces.
8. **Would removing it cause mistakes?** The deletion test, applied to
   every always-on line. If not, cut.

## Where Vendors Disagree

The first question is not *how* a type behaves per client but *whether the
client can host it at all* — write a type into a client with no surface for
it and the config looks installed while doing nothing. Ten clients group
into three tiers (as of 2026; re-verify):

| Type | Every client | Some clients | No surface at all |
|---|---|---|---|
| Skill | [Claude Code][cc], [OpenCode][oc-home], [Copilot][cop-home], [Codex][cx-home], [Cursor][cur], [Kiro][kiro], [Junie][junie], [Gemini CLI][gem], [Zed][zed], [Amp][amp] | — | — |
| Always-on file | all ten, under different filenames | — | — |
| Glob-scoped rule | — | Claude Code, Copilot, Cursor, Kiro (real scoping); OpenCode (per-file, no scoping) | Codex, Junie, Gemini CLI, Zed, Amp — always-on file only |
| Subagent | — | Claude Code, OpenCode, Copilot, Codex, Cursor, Gemini CLI | Kiro, Junie, Zed, Amp — no installable format |
| Hook | — | Claude Code, OpenCode, Copilot | unsurveyed for the other seven |

The two outliers worth memorizing: **rules are the least portable prose
type** (half the clients cannot host one — route that content to the
always-on file instead), and **skills are the only universally installable
type** — which is why cross-client packaging defaults to skills.

- **Glob scoping has no shared semantics.** Claude Code injects a scoped
  rule when a matching file is *read* — not when one is created. Copilot
  matches `applyTo:` against files in context, plus semantic matching of
  the rule description. Cursor takes a `globs` string, and splits it on
  every comma — including one inside a `{a,b}` alternation, so
  `src/**/*.{rs,toml}` becomes two patterns; write one extension per glob.
  Kiro takes a `fileMatchPattern` list. OpenCode resolves `instructions`
  globs at startup and loads everything always-on — porting scoped rules
  there converts them into permanent cost.
- **Skill plumbing differs.** Claude Code injects the body as a
  conversation message that persists; OpenCode returns it as a native
  `skill` tool result; Copilot uses dual activation (semantic + slash).
  Same standard, different delivery — never depend on the delivery.
- **Four clients share one skills directory.** Codex, Gemini CLI, Zed, and
  Amp all scan the cross-vendor `.agents/skills/`, so one copy there is
  read by all four — and a name collision there collides for all four.
  See [skill-design.md](skill-design.md).
- **Hooks are three technologies.** Shell + exit codes (Claude Code),
  JS/TS plugins that can cancel tool calls (OpenCode), declarative JSON
  command/http/prompt hooks (Copilot). Hooks must be re-implemented per
  client.
- **Name-collision precedence is client-specific** and contradicts the
  spec guide's stated convention. Unique names are the only portable
  strategy.
- **Always-on precedence stacks differ**: Copilot layers personal > repo >
  org with nearest-AGENTS.md-wins; OpenCode merges multiple config
  sources; Claude Code loads the full ancestor directory hierarchy.

## Migration Paths

| Signal content has outgrown its type | Move | Why |
|---|---|---|
| Always-on file > ~200 lines, or lines fail the deletion test | Split into scoped rules (per-area standards) + skills (workflows) | Adherence degrades with size; rules and skills defer the cost |
| A scoped rule has grown procedural (step-by-step, > 200 lines) | Rule → skill; the rule keeps a one-line invariant + pointer | Multi-step procedures are skill-shaped; bundled files are free until read |
| A skill encodes something that must never be skipped | Skill → hook for the enforcement core; the skill keeps the how/why | Skill activation is probabilistic; hooks are the only guarantee |
| A manual command gets used repeatedly in predictable contexts | Manual-only skill → auto-invocable skill with a trigger-rich description | Removes the "remember to type it" failure |
| Recurring research keeps flooding the main context | Inline work → subagent; preload needed skills explicitly — auto-activation does not transfer | The main context receives only the distilled summary |
| The same conventions are duplicated across clients | Per-vendor files → AGENTS.md as source of truth + a wrapper or symlink for each client that reads its own file | AGENTS.md and Agent Skills are the two multi-vendor standards; only the skill installs everywhere |

Default authoring sequence: start with a lean always-on file → extract
recurring workflows into skills as patterns emerge → add hooks once a
policy proves worth enforcing → reach for subagents only when isolation or
parallelism is demonstrably needed.

## Further Reading

- [Claude Code: memory and rules][cc-mem] — activation and token mechanics
  for always-on files and `paths:`-scoped rules.
- [Claude Code: skills][cc-skills] / [sub-agents][cc-agents] /
  [hooks][cc-hooks] — per-type mechanics for one major client.
- [OpenCode: rules][oc-rules] / [skills][oc-skills] /
  [plugins][oc-plugins] — the divergent model: always-on globs, native
  skill tool, JS plugins.
- [Copilot: custom-instructions support matrix][cop-matrix] /
  [agent skills][cop-skills] / [hooks reference][cop-hooks] — which
  mechanism works on which surface.
- [Codex: skills][cx-skills] / [subagents][cx-agents] — the AGENTS.md-only
  always-on surface, `.agents/skills/` discovery, TOML subagents.
- [Cursor][cur] / [Kiro][kiro] / [Junie][junie] / [Gemini CLI][gem] /
  [Zed][zed] / [Amp][amp] — the clients whose rule and agent surfaces are
  partial or absent; check each product's own docs before assuming a type
  installs there.
- [Agent Skills specification][spec] — the open standard and its token
  tiers.
- [Effective context engineering for AI agents][ctx] — why deferred
  loading wins.
- [What hooks can and cannot enforce][boucle] — the hooks-limitations
  taxonomy behind the "not a security boundary" row.
- [Evaluating AGENTS.md (ETH Zurich)][eth] — the −3%/+20% evidence on
  generated context files.
- [Claude Skills are awesome][willison] — the token-economics argument
  that made skills the default knowledge vehicle.

[cc]: https://code.claude.com
[oc-home]: https://opencode.ai
[cop-home]: https://github.com/features/copilot
[cx-home]: https://developers.openai.com/codex
[cur]: https://cursor.com
[kiro]: https://kiro.dev
[junie]: https://www.jetbrains.com/junie/
[gem]: https://geminicli.com
[zed]: https://zed.dev
[amp]: https://ampcode.com
[cc-mem]: https://code.claude.com/docs/en/memory
[cc-skills]: https://code.claude.com/docs/en/skills
[cc-agents]: https://code.claude.com/docs/en/sub-agents
[cc-hooks]: https://code.claude.com/docs/en/hooks
[oc-rules]: https://opencode.ai/docs/rules/
[oc-skills]: https://opencode.ai/docs/skills/
[oc-plugins]: https://opencode.ai/docs/plugins/
[cop-matrix]: https://docs.github.com/en/copilot/reference/custom-instructions-support
[cop-skills]: https://docs.github.com/en/copilot/concepts/agents/about-agent-skills
[cop-hooks]: https://docs.github.com/en/copilot/reference/hooks-reference
[cx-skills]: https://developers.openai.com/codex/skills
[cx-agents]: https://developers.openai.com/codex/subagents
[spec]: https://agentskills.io/specification
[ctx]: https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents
[boucle]: https://blog.boucle.sh/posts/what-claude-code-hooks-can-and-cannot-enforce
[eth]: https://arxiv.org/abs/2602.11988
[willison]: https://simonwillison.net/2025/Oct/16/claude-skills/
