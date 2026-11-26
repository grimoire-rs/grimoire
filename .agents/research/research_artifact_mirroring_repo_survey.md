# Research: How popular agent-config repositories structure content and whether it differs per harness

## Metadata

**Date:** 2026-11-23
**Domain:** packaging
**Triggered by:** artifact-mirroring discussion (importing content from public agent-config repositories into grim)
**Expires:** 2027-05-23 (star counts and layouts move weekly; re-verify)

Method: stars from the GitHub REST API on 2026-11-23 `[page]`; each repository shallow-cloned (`--depth 1 --filter=blob:none`) and read only, never executed `[tree]`. Clones deleted after reading. Diffs were run with `diff` on the checked-out files.

## Direct Answer

All three patterns exist in the sample, and the split is clear on one axis: **skills (SKILL.md folders) are almost always one shape; commands, agents, rules, hooks and manifests are where harness variants appear.** Where variants exist they are either rendered from one source (frontmatter/wrapper/format/path differences only) or hand-copied with a CI drift check. In the diffs read, body text differed in only a few places, mostly where a harness has a different tool set or invocation syntax.

## Repositories (stars as of 2026-11-23)

### 1. mattpocock/skills (331k, MIT) -- single-shape
- Harnesses: Claude Code, Codex, Copilot, Gemini CLI listed in README install blocks, all pointing at the same `skills/` tree `[tree]` README.md lines 36-80.
- One source, no renderer. `.claude-plugin/plugin.json` lists `./skills/engineering/<name>` dirs (same repo) `[tree]`. `AGENTS.md` is a symlink to `CLAUDE.md` `[tree]`.
- Sidecar: `skills/*/*/agents/openai.yaml` (38 files) holds only `interface.display_name/short_description` for Codex `[tree]`.
- Unit kinds: 39 SKILL.md; no rules/agents/hooks `[tree]`. Licence: one root `LICENSE` (MIT) `[tree]`.
- Diffs: none possible (no variants).

### 2. obra/superpowers (325k, MIT) -- single-shape content, per-harness wrappers
- Targets: Claude, Codex, Cursor, Gemini, OpenCode, Devin, Kimi, Pi, Hermes, Antigravity, Muse: one manifest dir each (`.claude-plugin`, `.codex-plugin`, `.cursor-plugin`, ...) plus `gemini-extension.json` `[tree]`.
- One `skills/` tree (15 SKILL.md); every manifest points at it (`"skills": "./skills/"` in `.cursor-plugin/plugin.json`; Claude marketplace `source: "./"`) `[tree]`. No generator; `.version-bump.json` keeps 12 manifest versions in sync `[tree]`.
- Variant content that is hand-maintained: `skills/using-superpowers/references/{claude-code,codex,gemini,hermes,pi,muse,antigravity}-tools.md` (tool-name mapping per harness; `gemini-extension.json` loads GEMINI.md which `@`-imports `gemini-tools.md`) `[tree]`.
- Diffs: `hooks/hooks.json` vs `hooks/hooks-cursor.json`: different event key case (`SessionStart` vs `sessionStart`), `version: 1`, no matcher/nesting in Cursor form `[tree]`. `codex-tools.md` vs `gemini-tools.md`: unrelated bodies (Codex config note vs a tool table) `[tree]`.
- Codex copy: `scripts/sync-to-codex-plugin.sh` rsyncs tracked content into a separate fork repo and opens a PR `[tree]`.
- Unit kinds: 15 skills, hooks, JS/TS plugin glue for OpenCode/Pi/Hermes. Licence: root MIT `[tree]`.

### 3. affaan-m/ECC (309k, MIT) -- mixed (canonical source + committed generated copies + hand rules)
- Targets: Claude, Codex, Cursor, Gemini, Kiro, OpenCode, Pi, CodeBuddy, OpenClaw, Antigravity, Grok, Hermes, Kimi `[tree]` (dot-dirs at root).
- Canonical: `skills/` (328), `agents/` (72), `commands/` (99 legacy shims), `rules/` (18 language dirs + common) `[tree]`.
- Generators: `scripts/build-opencode-agents.js`, `scripts/agent-convert.js` (header says only `claude -> pi` implemented), `scripts/gemini-adapt-agents.js`; `manifests/install-modules.json` drives `install.sh` `[tree]`. Outputs are COMMITTED (`.kiro/agents/*.md+.json`, `.opencode/prompts/agents/*.txt`, `.cursor/skills`, `.agents/skills`, `.codex/agents/*.toml`) `[tree]`.
- Diffs: `skills/article-writing/SKILL.md` vs `.cursor/skills/...`: frontmatter only (`metadata.origin` flattened to `origin`) `[tree]`; vs `.agents/skills/...`: frontmatter only (`license: MIT` instead of `metadata.origin`) `[tree]`. `agents/architect.md` vs `.kiro/agents/architect.md`: frontmatter swapped to `allowedTools`, AND the "Prompt Defense Baseline" section is absent from the Kiro copy (body drift) `[tree]`. `rules/common/coding-style.md` vs `.cursor/rules/common-coding-style.md`: Cursor frontmatter added and the "Core Principles / KISS / DRY" sections removed (body differs) `[tree]`. `.kiro/agents/architect.json` embeds the prompt as one JSON string `[tree]`.
- Marketplace: `.claude-plugin/marketplace.json` `source: "./"` (same repo) `[tree]`. Licence: root MIT; 2 nested LICENSE files under `skills/` `[tree]`.

### 4. nextlevelbuilder/ui-ux-pro-max-skill (153k, MIT) -- generated at install time from one source
- Targets 25 platforms: `cli/assets/templates/platforms/*.json` (claude, cursor, copilot, gemini, codex, windsurf, kiro, opencode, ...) `[tree]`.
- Source: `cli/assets/templates/base/skill-content.md` (462 lines) + `quick-reference.md`; per-platform JSON gives root dir (`.claude`, `.cursor`, `.github`+`globalRoot .copilot`), frontmatter, title, `quickReference` on/off; `cli/src/utils/template.ts` renders `[tree]`. Outputs are produced by the installer; none committed for other harnesses `[tree]`.
- Diffs: `platforms/claude.json` vs `copilot.json`: root dir, platform name, title, quickReference flag only `[tree]`. Committed dev copy `.claude/skills/ui-ux-pro-max/SKILL.md` is 243 lines vs 462-line template (the committed copy is a different, shorter file) `[tree]`.
- Unit kinds: 7 skills (+ data CSVs). Marketplace: `.claude-plugin/marketplace.json`, `source: "./"` `[tree]`. Licence: root LICENSE; one extra under `ui-styling` `[tree]`.

### 5. github/spec-kit (151k, MIT) -- generated at install time from one source
- Source: `templates/commands/*.md` (9 commands: specify, plan, tasks, implement, ...) `[tree]`.
- Renderer: `src/specify_cli/integrations/<harness>/__init__.py`, about 40 harness classes (claude, copilot, cursor_agent, gemini, codex, opencode, kiro_cli, ...) declaring folder, subdir, format (md or toml), arg placeholder (`{{args}}`), and hook-event name map `[tree]`.
- Example: Gemini class sets `.gemini/commands`, `toml`, `args "{{args}}"`, maps `pre_tool_use -> BeforeTool`, timeouts in ms vs seconds elsewhere `[tree]` `src/specify_cli/integrations/gemini/__init__.py`. Per-harness differences are format/placeholder/event names, not instructions.
- Rendered output is written into the user's project by `specify init`; nothing committed (no `.claude/` in repo) `[tree]`. Extra kinds: extensions, presets, bundles with JSON catalogs `[tree]`. Licence: root MIT.

### 6. pbakaus/impeccable (116k, Apache-2.0) -- generated from one source, outputs committed
- Source `skill/SKILL.src.md` with `{{command_hint}}`, `{{scripts_path}}` placeholders and a `claude-code-description` key; `scripts/build.js` + `scripts/lib/transformers/{factory,providers,hooks}.js` `[tree]`.
- Committed outputs for ~17 harnesses: `.claude/skills`, `.cursor/skills`, `.gemini/skills`, `.kiro/skills`, `.opencode`, `.codex`, `.trae`, `.qoder`, `.pi`, ... plus `plugin/`, `cursor-plugin/` `[tree]`.
- Diffs: `.claude` vs `.gemini` vs `.cursor` vs `.kiro` SKILL.md: (a) Claude gets the shorter `claude-code-description`, others the long one; (b) `user-invocable`, `argument-hint`, `license` keys dropped on non-Claude; (c) body identical except the substituted script path (`.claude/skills/impeccable/scripts/...` becomes `.gemini/...`, `.kiro/...`) `[tree]`.
- Licence: root Apache-2.0 (191 lines), `NOTICE.md`.

### 7. anthropics/skills (199k, licence not detected by API) -- single-shape
- 19 skills under `skills/`; `spec/agent-skills-spec.md`; `template/SKILL.md` `[tree]`. `.claude-plugin/marketplace.json` has 2 plugin bundles (document-skills, example-skills) with `source: "./"` and explicit skill path lists `[tree]`.
- No harness variants; README says it is Anthropic's implementation and points to agentskills.io `[tree]`.
- Licence: no root LICENSE; per-skill `LICENSE.txt` and `license:` frontmatter (`Complete terms in LICENSE.txt`) `[tree]`.

### 8. addyosmani/agent-skills (120k, MIT) -- mixed (single-shape skills, hand-divergent commands with drift gate)
- 26 skills in `skills/`; `.claude/skills`, `.opencode/skills` are symlinks to it `[tree]`. 5 agent personas in `agents/*.md`; Codex and Claude plugin manifests (`.codex-plugin`, `.claude-plugin`, `plugin.json`) `[tree]`.
- Commands exist in three hand-maintained directories: `.claude/commands/*.md`, `.gemini/commands/*.toml`, `commands/*.toml` (Antigravity), 10 each `[tree]`.
- `scripts/validate-commands.js` header: checks each command exists in all three and `description` is identical; says "Prompt body differences are intentional -- each tool has its own syntax ($ARGUMENTS, agent-skills: prefixes, GEMINI.md vs CLAUDE.md)" `[tree]`.
- Diff: `.claude/commands/review.md` vs `.gemini/commands/review.toml`: Claude has `disable-model-invocation: true` frontmatter and "Invoke the agent-skills:code-review-and-quality skill"; Gemini has TOML `prompt = """..."""` and "Invoke the code-review-and-quality skill" (namespace prefix dropped); remaining lines the same `[tree]`.
- Licence: root MIT.

### 9. DietrichGebert/ponytail (197k, licence file at root) -- mixed (one source, hand-copied with a CI drift check)
- Compact rule text in `AGENTS.md`, copied to `.cursor/rules/ponytail.mdc`, `.windsurf/rules/`, `.clinerules/`, `.agents/rules/`, `.qoder/rules/`, `.github/copilot-instructions.md`, `.kiro/steering/` `[tree]`.
- `scripts/check-rule-copies.js`: compares each copy to AGENTS.md after stripping host frontmatter; fails with "drifted from AGENTS.md" `[tree]`. Bodies are therefore byte-identical, frontmatter differs (Cursor and Kiro keep their own). Not a renderer: copies are committed by hand.
- Skills (6) in `skills/`, commands in `commands/*.md` and `.opencode/command/*.toml` (parallel hand copies); manifests for Claude, Codex, Devin, Grok, Kimi, Qoder, Gemini `[tree]`. Licence: root `LICENSE`.

### 10. ruvnet/ruflo (82k, MIT; formerly claude-flow) -- mixed (hand-built Claude tree + generated Codex tree)
- `.claude/` (33 agent files, 39 skills, 23 commands, settings, hooks) hand-maintained `[tree]`. `.agents/` is Codex output: README says "agent configuration and skills for OpenAI Codex CLI", `config.toml` header "Generated by: @claude-flow/codex" `[tree]`.
- `.agents/skills` has 135 skills, 89 named `agent-*` that are the Claude agents re-emitted as skills; sample `agent-agent/SKILL.md` has a stub description ("Agent skill for agent - invoke with $agent-agent") and is 821 lines `[tree]`. Overlapping skill `agentdb-advanced`: Claude `name: agentdb-advanced`, Codex `name: "AgentDB Advanced Features"` (frontmatter differs; body not fully diffed) `[tree]`.
- 49 plugins under `plugins/`, marketplace entries `source: "./plugins/<name>"` (same repo) `[tree]`. Licence: root MIT.

### 11. wshobson/agents (43k, MIT) -- generated from one source, outputs NOT committed
- Source: `plugins/<name>/{agents,skills,commands}` in Claude plugin format; 91 plugins, 199 agents, 185 skills, 103 commands `[tree]`.
- Adapters `tools/adapters/{codex,copilot,cursor,opencode,antigravity,pi}.py`, entry `tools/generate.py`; `make validate` and CI regenerate and validate. ARCHITECTURE.md: "Generated harness-specific artifacts ... are produced by adapters and gitignored. ... small native-install registries ... are committed" `[tree]` ARCHITECTURE.md:7.
- Per-harness transforms: Codex Markdown agent to TOML with 8 KB body cap and `references/` overflow plus a sandbox_mode heuristic; model aliases mapped per harness; tool permissions translated `[tree]` ARCHITECTURE.md:61, README.md:8.
- Hand-written per-harness content exists only as curated Cursor project rules (`tools/adapters/cursor_rules`, `.cursor/rules/*.mdc`) `[tree]`.
- Marketplace `.claude-plugin/marketplace.json` entries `source: "./plugins/<name>"`; per-plugin `.codex-plugin/plugin.json` and `.claude-plugin/plugin.json`; `.cursor-plugin/plugins/*.json` `[tree]`. Licence: root LICENSE.

### 12. github/awesome-copilot (41k, MIT) -- single-shape (Copilot-native)
- Units: 464 skills, 229 `*.agent.md`, 195 `*.instructions.md` (frontmatter `applyTo`), 116 plugins, 8 hooks, 8 workflows, 43 extensions `[tree]`.
- Copilot-specific file conventions; only 15 instruction files mention claude/codex/cursor. No other-harness variants `[tree]`.
- `.github/plugin/marketplace.json`; plugins' `plugin.json` reference `./agents/...` inside the plugin, with `$schema` agent-plugins.org `[tree]`. Licence: root MIT; no per-unit LICENSE seen.

### 13. PatrickJS/awesome-cursorrules (41k, CC0-1.0) -- single-shape (Cursor-only)
- 257 `rules/*.mdc` with `description/globs/alwaysApply` frontmatter; no `.cursorrules` left; scripts are repo-hygiene checks only (`check-rule-hygiene.mjs`, `cleanup-legacy-artifacts.mjs`) `[tree]`. No manifest. Licence: root CC0.

### 14. vercel-labs/agent-skills (35k, licence by frontmatter) -- single-shape
- 9 skills; `skills.sh.json` grouping file for skills.sh; `CLAUDE.md` is a symlink to `AGENTS.md`; `packages/react-best-practices-build` compiles rule fragments into one `AGENTS.md` per skill (build step exists but is not per-harness) `[tree]`. Committed `.zip` per skill. No LICENSE file; `license: MIT` in each SKILL.md `[tree]`.

### 15. openai/skills (29k, no root licence detected) -- single-shape
- `skills/.curated` and `skills/.system`, 44 SKILL.md, each with `agents/openai.yaml` (Codex UI metadata: display_name, short_description, icon, default_prompt) `[tree]`. 38 per-skill `LICENSE.txt` (Apache-2.0 sampled). No marketplace file, no other-harness content `[tree]`.

### 16. bmad-code-org/BMAD-METHOD (56k, MIT) -- single-shape, installer delegates
- v7 preview branch: 36 SKILL.md under `skills/{core-tools,method,founder,toolsmith}`; per-skill `bmod.toml`/`customize.toml`; README: install runs `npx skills ...` and asks "which coding tools to install to" `[tree]` README.md:34-56. No per-harness trees in repo; per-harness rendering is outside the repository (the `skills` CLI) `[tree]`.
- Licence: root MIT; trademark file. Marketplace/plugin manifest: none found `[tree]`.

### 17. anthropics/claude-plugins-official (48k, Apache-2.0) -- single-shape marketplace
- 317 marketplace entries in `.claude-plugin/marketplace.json`: 55 string sources (local, in this repo), 163 `url`, 98 `git-subdir`, 1 `command` -- i.e. most point to OTHER repositories `[tree]`. Local trees `plugins/` (43) and `external_plugins/` (61). Licence: root + 43 per-plugin LICENSE files `[tree]`.

### Not inspected
- hesreallyhim/awesome-claude-code (59k): link list, no installable content; not read `[search]` (stars from API only).
- ComposioHQ/awesome-claude-skills (80k), davila7/claude-code-templates (34k): not inspected.
- anthropics/claude-code (157k): plugin examples only; not inspected.

## Count (17 inspected)

| Class | N | Repositories |
|---|---|---|
| single-shape (one tree, one format, manifests only point at it) | 9 | mattpocock/skills, obra/superpowers, anthropics/skills, awesome-copilot, awesome-cursorrules, openai/skills, vercel-labs/agent-skills, BMAD-METHOD, claude-plugins-official |
| generated from one source (renderer in repo) | 4 | ui-ux-pro-max (install time), spec-kit (install time), impeccable (committed outputs), wshobson/agents (gitignored outputs) |
| hand-divergent only | 0 | none seen as the sole pattern |
| mixed | 4 | ECC (canonical + committed generated + hand rules), addyosmani (single skills + hand commands + drift gate), ponytail (hand copies + drift gate), ruflo (hand Claude tree + generated Codex tree) |

superpowers is counted single-shape because skill bodies are shared, but it hand-maintains per-harness hooks files and tool-mapping reference docs; it could also be read as mixed.

## negative:
- No repository ranks "hand-maintained different instructions per harness" as its main pattern; the closest are addyosmani commands (bodies differ by invocation syntax and namespace prefix) and ECC Cursor rules (bodies trimmed).
- Skill bodies were never found different across harnesses except by substituted paths and frontmatter (impeccable, ECC) -- one exception: ECC Kiro agent copy lacks a section present in canonical (drift, not intent).
- No repository other than ECC and ruflo committed a full second copy of every unit with a different body.
- Licence is rarely per unit: only anthropics/skills, openai/skills, plugins-official use per-unit licence files; vercel uses frontmatter only; the rest use one root file.
- Not determined: whether the ECC and ruflo committed copies are regenerated on every release (no CI job read); how `npx skills` (BMAD, vercel) renders per harness (external tool, not read); line-level body diffs for ruflo's overlapping skills beyond frontmatter; licence of anthropics/skills individual skills (only the pointer read).

## leads:
- the `skills` CLI (`npx skills`, skills.sh): installs one SKILL.md tree to many harnesses; BMAD and vercel rely on it, so it is the de-facto renderer for the single-shape majority.
- agentskills.io spec / `spec/agent-skills-spec.md`: defines the one shared skill shape; check which frontmatter keys harnesses add on top (impeccable, ECC show `user-invocable`, `argument-hint`, `allowed-tools`).
- wshobson `docs/harnesses.md` capability matrix: a ready-made per-harness feature table.
- spec-kit `integrations/*/__init__.py`: structured declaration of per-harness folder, format, placeholder and hook-event names, comparable to grim's vendor tables.
- Marketplace `source` kinds in claude-plugins-official (`url`, `git-subdir`): shows how a listing points at other repositories, relevant to importing without copying.
- Hooks and commands are where variants live; a follow-up survey of hooks-only repositories was not done.
