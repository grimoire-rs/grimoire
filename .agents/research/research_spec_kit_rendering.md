# Research: spec-kit's Multi-Client Rendering Model (input for advanced grim rendering)

Analyzed 2026-07-19, `github/spec-kit` @ `main` (v0.13.0, commit `57cc518d`),
via 4-agent research fan-out (overview / init CLI / templates / multi-client).
Full facet transcripts: arcana session scratchpad. Written from the arcana
session that generalizes the OCX swarm skills into the public `hex` bundle;
persisted here because spec-kit's rendering pipeline is prior art for a
possible advanced-rendering feature in grim.

## What spec-kit is (context)

GitHub's official spec-driven-development toolkit: Python CLI (`specify init`)
scaffolds `.specify/` (templates, scripts, constitution, manifests) and
renders one canonical command set into per-client command/skill files.
122k stars, 251 contributors, ~daily releases, MIT, 34 client integrations.
Command flow: `constitution → specify → clarify → plan → tasks → analyze →
implement → converge`.

## The rendering architecture (the part that matters for grim)

```
templates/commands/*.md            ← ONE canonical source per command
  (YAML frontmatter + body; tokens: {SCRIPT} {ARGS}/$ARGUMENTS __AGENT__
   __SPECKIT_COMMAND_<NAME>__)
        │  hatchling force-include → wheel's specify_cli/core_pack/  (build once)
        ▼
integrations/<key>/  (34 subpackages, one per client)
  class ladder: MarkdownIntegration | TomlIntegration | YamlIntegration
                | SkillsIntegration | raw IntegrationBase (fully custom)
  declares: folder, commands_subdir, format, arg placeholder, extension,
            requires_cli, invoke separator
        ▼
specify init --integration <key>   ← RUNTIME rendering on the user's machine
  IntegrationBase.process_template(): script extraction → {SCRIPT} → arg
  placeholder → __AGENT__ → path rewrites → cross-command token resolution
```

Key architectural facts:

1. **Runtime rendering, not build-time packaging.** They ABANDONED per-agent
   release zips (old model: CI matrix built agent×shell variants as GitHub
   release assets; `specify init` downloaded them). Now: templates ship
   bundled in the wheel, rendering happens client-side at init. Zero network
   on init; `--offline`/`--github-token` are deprecated no-ops. This pivot
   validates grim's install-time materialization model.
2. **Single registry.** `INTEGRATION_REGISTRY` is the only place client
   metadata lives; everything else (agent config tables, extension system)
   derives from it at import time.
3. **Base-class ladder = cheap client onboarding.** ~20 clients are pure
   `MarkdownIntegration` with 3 class attributes and zero method overrides.
   Format families (TOML for Gemini, YAML recipes for Goose, skills dirs for
   Claude/Codex/Cursor/Zed) each get one base class; only genuinely weird
   clients (Copilot: dual-mode + `.vscode/settings.json` merge) go custom.

## Per-client render targets (selection)

| Client | Dir | Format | Notes |
|---|---|---|---|
| Claude Code | `.claude/skills/speckit-<n>/SKILL.md` | skills | injects `argument-hint`, `user-invocable`, `disable-model-invocation` |
| Copilot default | `.github/agents/speckit.<n>.agent.md` | md | + companion `.prompt.md` + `.vscode/settings.json` merge |
| Copilot `--skills` | `.github/skills/speckit-<n>/SKILL.md` | skills | omits Claude-only `mode:` field |
| Gemini CLI | `.gemini/commands/speckit.<n>.toml` | TOML | `description` + `prompt = """…"""` |
| Goose | `.goose/recipes/speckit.<n>.yaml` | YAML | full recipe render |
| Codex CLI | `.agents/skills/speckit-<n>/SKILL.md` | skills | invoked `$speckit-<n>` (dollar prefix) |
| Cursor | `.cursor/skills/speckit-<n>/SKILL.md` | skills | commands deprecated, skills default |
| generic | user-supplied `--commands-dir` | md | bring-your-own-agent fallback |

Cross-cutting conventions the renderer normalizes:

- **Arg placeholder**: `$ARGUMENTS` (md/skills) vs `{{args}}` (TOML/YAML) vs
  `{{parameters}}` (Forge) — one authoring token, per-family substitution.
- **Command separator**: `/speckit.<n>` for slash-command clients, but
  `speckit-<n>` for ALL skills-mode clients — the agentskills standard
  (`[a-z0-9-]` names) forbids dots. Cross-references between commands are
  authored as `__SPECKIT_COMMAND_PLAN__` tokens and resolved to the target
  client's separator at render time.
- **Frontmatter dialects**: plain `description:` vs Copilot chat-mode
  `mode:` vs TOML keys vs YAML recipe headers — handled per base class via
  `post_process_skill_content()` hooks.
- **Quirk absorption**: Forge strips unsupported `handoffs:` key; Kiro gets
  prose instead of `$ARGUMENTS` (client never substitutes it); Cursor `.mdc`
  rules get `alwaysApply: true` injected; Windows exec dispatch resolves via
  `shutil.which()` (PATHEXT).

## Re-entrancy / install lifecycle (relevant to grim update/add semantics)

- Every written file is SHA-256-recorded in a per-integration
  `manifest.json`.
- Shared infra (`.specify/scripts`, `templates/`): existing files are
  **skipped** by default; `--force` overwrites all; upgrade mode
  (`refresh_managed`) overwrites only files whose on-disk hash still matches
  the recorded hash — user-modified files are preserved with a warning.
  Stale files the new bundle no longer ships are cleaned up, but only
  managed (unmodified) ones. Symlink-escape writes are hard errors.
- Gotcha they shipped: agent command/skill files are regenerated
  UNCONDITIONALLY on re-init (hand-edits silently clobbered) while shared
  templates are preserved — asymmetric policy, source of user surprise.
  grim's "refuse modified output unless --force" is the stricter, better
  default.
- Constitution: materialized from template **only if absent**, then only
  ever edited in place.
- 17 integrations declared "multi-install safe" (disjoint roots +
  manifests) → one project can carry claude + gemini + cursor simultaneously.

## Context-file registry (relevant to any grim "discovery note" feature)

Context management is an OPT-IN extension (`agent-context`), not core. Its
defaults table maps integration → context file: claude→`CLAUDE.md`,
gemini→`GEMINI.md`, copilot→`.github/copilot-instructions.md`, and a long
tail (codex/zed/opencode/…) → shared `AGENTS.md`; cursor→
`.cursor/rules/specify-rules.mdc`, windsurf/kilocode/cline/… → per-client
rules files. The write is a minimal marker-fenced block
(`<!-- SPECKIT START/END -->`) that only POINTS at the live plan
(`specs/<feature>/plan.md`) — no content duplication. Supports multiple
simultaneous context files.

## Implications for grim "advanced rendering"

grim today: universal skill render + string-valued vendor-metadata
projection (`claude.*`/`codex.*` keys → typed frontmatter); rules/agents get
per-client transforms; refuses overwriting modified outputs. spec-kit goes
further in four dimensions grim could adopt:

1. **Format-family transforms** — render one canonical artifact into
   TOML/YAML/prompt-file shapes for clients whose command surface is not
   markdown (Gemini commands, Goose recipes, Copilot prompt files). grim's
   analog: new render backends per client kind, chosen by a base-class-ladder
   equivalent in the materializer.
2. **Token pipeline** — client-resolved placeholders inside artifact bodies:
   arg placeholder (`$ARGUMENTS` vs `{{args}}`), agent name, and
   cross-artifact references resolved to the installing client's invocation
   syntax. grim's analog: a small, documented token set substituted at
   materialize time (today bodies are verbatim).
3. **Context-file registry** — per-client knowledge of WHERE the client
   reads ambient context (CLAUDE.md/AGENTS.md/rules dirs), enabling a
   `grim`-level marker-fenced pointer block feature instead of every skill
   (e.g. arcana's hex-init) hand-rolling client detection.
4. **Manifest-hash lifecycle** — grim already hashes; spec-kit's
   refresh-managed nuance (upgrade overwrites only unmodified files,
   preserves customized ones with a hint, cleans up stale managed files) is
   a good model for `grim update` semantics on multi-file skill trees.

Counter-lessons (what NOT to copy): unconditional regeneration of
client-side command files (silent clobber); stale docs describing a
release-asset pipeline that no longer exists; dual-mode integrations
(Copilot md vs skills) doubling test surface.

## Sources

All claims trace to `github/spec-kit@main`: `README.md`, `AGENTS.md`,
`spec-driven.md`, `docs/reference/integrations.md`,
`src/specify_cli/{commands/init.py,integrations/base.py,agents.py,_assets.py,shared_infra.py}`,
`templates/**`, `extensions/agent-context/**`,
`.github/workflows/release*.yml`, `pyproject.toml`; GitHub API for repo
metrics (stars/releases/contributors, fetched 2026-07-19).
