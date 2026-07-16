# Skill Design

You loaded this file because you are writing a new SKILL.md, restructuring
an existing one, or reviewing a skill's anatomy, size, or bundled files.

Contents: [Anatomy](#anatomy) · [Size](#size) ·
[Progressive Disclosure](#progressive-disclosure) ·
[Eval-Driven Authoring](#eval-driven-authoring) ·
[Anti-Patterns](#anti-patterns)

## Anatomy

A skill is a directory whose entrypoint is `SKILL.md`: YAML frontmatter
between `---` fences, then a Markdown body. The directory name is the
skill's identity — `name` must match it exactly.

Portable frontmatter (the Agent Skills open standard):

| Field | Required | Constraints |
|---|---|---|
| `name` | yes | 1–64 chars; lowercase letters, digits, hyphens; no leading/trailing/consecutive hyphens; must equal the directory name (grim also accepts periods as a superset — prefer hyphens for portability) |
| `description` | yes | 1–1024 chars; what the skill does + when to use it — see [descriptions.md](descriptions.md) |
| `license` | no | License name or pointer to a bundled license file |
| `compatibility` | no | ≤ 500 chars; environment needs — most skills do not need it |
| `metadata` | no | String-to-string map; the blessed extension point (author, version) |
| `allowed-tools` | no | Experimental; support varies by client — never depend on it |

Everything beyond these fields is vendor-specific. All major clients
ignore unknown frontmatter, so vendor fields degrade silently elsewhere —
never make correctness depend on a vendor field or on argument
substitution; in other clients placeholders render as literal text.

Bundled directories carry normative semantics:

- `scripts/` — executable code, run via the shell, never loaded into
  context (only output enters). Self-contained or with documented
  dependencies, non-interactive, with helpful error messages. Mark them
  black-box: "run with `--help` first; do not read the source."
- `references/` — documentation read on demand; one focused topic per
  file, named descriptively (`form_validation_rules.md`, not `doc2.md`).
- `assets/` — static resources used in *output*: templates, schemas,
  fonts, lookup tables.

No README — `SKILL.md` is the sole entrypoint.

## Size

| Measure | Value |
|---|---|
| Hard ceiling | < 500 lines / < 5k tokens of body |
| Healthy median | ~250 lines — the official reference pack measures min 32 / median 254 / max 590 across 17 skills (as of 2026) |
| Thin pointer skills | ≤ ~130 lines, wrapping scripts |
| Metadata cost | ~100 tokens per skill, paid every session |

When approaching the ceiling, split: add a layer of hierarchy with clear
pointers. Do not compress prose into denser prose — density is not the
same as economy.

## Progressive Disclosure

Three loading levels: metadata (always) → body (on trigger) → bundled
files (on demand, effectively unlimited). Mental model: a well-organized
manual — table of contents, then chapters, then a detailed appendix.

- **Root as index.** SKILL.md is an overview that points to detailed
  material; quick-start inline, depth one level down.
- **One level deep, no chains.** Nested reference chains cause partial
  reads (`head -100`-style previews) and incomplete information.
- **TOC in big files.** Any bundled file over 100 lines gets a table of
  contents at the top so a partial read still reveals scope. (Official
  guidance says 100; some community packs use a looser 300 — adopt the
  stricter figure.)
- **Annotate every link** with what the file contains and when to load it:
  "For complete API details, read `references/api.md`."
- **Make execution intent explicit.** "Run `analyze.py` to extract
  fields" (execute) vs "Read `analyze.py` for the algorithm" (load).
- **Write standing instructions, not one-time steps.** Once invoked, the
  body persists for the session and is not re-read. Front-load critical
  rules — on at least one client, post-compaction only the first ~5k
  tokens per skill are re-attached (vendor-specific, as of 2026;
  re-verify).
- Relative paths from the skill root, forward slashes only.

## Eval-Driven Authoring

Build evaluations *before* writing extensive documentation. The loop:

1. **Find the gap.** Run the agent on representative tasks without the
   skill; record concrete failures.
2. **Write ≥ 3 eval scenarios** (query + input files + expected
   behavior), including should-NOT-trigger neighbors.
3. **Baseline** without the skill — proves the skill earns its tokens.
4. **Write the minimal instructions** that make the evals pass.
5. **Iterate with an author/tester split**: one agent instance authors
   and refines; a fresh instance runs the evals; feed observed failures
   back ("the tester skipped the filter step — is the rule prominent
   enough?").
6. **Test every model tier you target** — smaller models need more
   guidance, larger ones less.

Observe navigation during testing: an unexpected read order means the
structure is unintuitive; a bundled file that is never read is unnecessary
or badly signaled; a file read on every run should be promoted into the
body.

## Anti-Patterns

| Anti-pattern | Fix |
|---|---|
| Explaining what the model already knows | Default assumption: the model is already smart. Challenge every paragraph's token cost |
| Vague names (`helper`, `utils`, `tools`) | Specific, gerund-first names: `processing-pdfs` |
| Option overload ("use A, or B, or C, or…") | One default + one escape hatch |
| Voodoo constants (magic values without justification) | If you don't know the right value, the model can't determine it either — justify or remove |
| Time-sensitive content ("before August, use the old API") | Current instructions only; a collapsible legacy section if history matters |
| Scripts that punt errors back to the model | Handle errors in the script; verbose validator messages enable self-correction |
| Assuming packages are installed | State the install commands; verify availability per target environment |
| Inconsistent terminology (mixing "field" / "box" / "element") | Pick one term, use it everywhere |
| Body content that surprises users relative to the description | The body must do what the description promises — nothing hidden |

## Further Reading

- [Agent Skills specification][spec] — normative frontmatter and
  directory semantics; the `skills-ref` validator.
- [Skill authoring best practices][bp] — disclosure patterns, degrees of
  freedom, eval-first workflow, the named anti-patterns.
- [Agent Skills overview][overview] — the three-level loading model with
  token budgets.
- [Equipping agents for the real world][eng] — the design rationale for
  progressive disclosure and bundled executable code.
- [Official example skills + skill-creator][repo] — measured shapes and a
  meta-skill that scaffolds new skills.
- [A flagship community pack][superpowers] — skills as *tested*
  artifacts: TDD-for-docs, trigger-assertion suites.
- [The Complete Guide to Building Skills][pdf] — long-form guide:
  trigger diagnostics, testing matrix, distribution.

[spec]: https://agentskills.io/specification
[bp]: https://platform.claude.com/docs/en/agents-and-tools/agent-skills/best-practices
[overview]: https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview
[eng]: https://www.anthropic.com/engineering/equipping-agents-for-the-real-world-with-agent-skills
[repo]: https://github.com/anthropics/skills
[superpowers]: https://github.com/obra/superpowers
[pdf]: https://resources.anthropic.com/hubfs/The-Complete-Guide-to-Building-Skill-for-Claude.pdf
