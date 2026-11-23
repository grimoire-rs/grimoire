# Research: Can a grim skill deliver a Claude Code mod today

## Metadata

**Date:** 2026-11-18
**Domain:** packaging
**Triggered by:** pre-plan discussion on a harness-mods artifact kind; disputed fact whether the existing `skill` kind already suffices
**Expires:** 2027-02-18 (Claude mods change every release; re-verify against the installed `claude` version)

## Direct Answer

Half 1 (grim packs and installs the whole skill directory): **confirmed**, by code reading and an experiment with `target/release/grim` 0.15.1.

Half 2 (Claude loads a plugin directory found under `.claude/skills/`, including its `hooks/hooks.json` `modules` code): **partly confirmed, not decided end to end**. The docs and `claude plugin list` confirm that a directory holding `.claude-plugin/plugin.json` under `.claude/skills/` loads as a plugin (the "skills-dir" origin) at user and project scope. That `modules` code actually runs from that origin was not observed: the runtime check needs a login, which the isolated config dir lacks. No doc page found says modules are excluded from that origin.

## Half 1, grim side

### Pack walk

- `grim build`, publish and local path sources share one walk: `collect_files` at `src/skill/skill_package.rs:632`. It recurses all directories and emits regular files only. Symlinks are skipped silently (`symlink_metadata`, lines 672 to 684; header comment lines 30 to 32).
- The only filter is `IgnoreSet` (`src/install/ignore_set.rs`): defaults `__pycache__/`, `*.py[co]`, `.venv/`, `venv/`, `.mypy_cache/`, `.pytest_cache/`, `.ruff_cache/`, `*.egg-info/`, `node_modules/`, `.git/`, `.svn/`, `.hg/`, `.DS_Store`, `Thumbs.db`, `desktop.ini`, `*.swp`, `*.swo`, `*~`, `.idea/`, `.vscode/`, plus the walk root's `.grimignore`. `NEVER_IGNORED` at line 48 is `SKILL.md` and `.grimignore`. `.claude-plugin/` and `hooks/` are not ignored; dot-directories as a class are not filtered.
- A skill requires `SKILL.md`: `validate_skill_dir` at `src/skill/skill_package.rs:46` rejects a missing or symlinked `SKILL.md`. Kind inference for a path is "directory with `SKILL.md` is a skill" (`docs/src/content/docs/commands.md` near line 635).
- Depth and node count are capped by `PackLimits` (`collect_files` lines 641 to 667). A `node_modules/` is pruned by the defaults, so a mod with npm dependencies needs a `!node_modules/` line in `.grimignore`.

### Materialize

- `src/install/materializer.rs:55` to `:110` copies every tar entry verbatim; only regular files and directories are accepted, anything else errors (lines 74 to 95). `SKILL.md` is the only file ever rewritten, and only when it carries tool-namespaced metadata keys (`.claude/rules/subsystem-install-layout-skills-rules.md` lines 15 to 24).
- Destination for the `claude` client is the client's `skills/` dir: project `<workspace>/.claude/skills/<name>/`, global `$CLAUDE_CONFIG_DIR/skills/<name>/` (observed below).

### Other kinds

- `rule`: one `<name>.md` under `rules/` (pre-0.16.0 packages also a sibling support dir under `rules/`); cannot place under `skills/`.
- `agent`: one file under `agents/`; no.
- `hook`: lands under grim's own hooks home (`$GRIM_HOME/hooks/`) and client hook config, not `skills/` (per `.claude/rules/subsystem-install-layout-hooks.md`; not re-read line by line here).
- `settings`, `mcp`, `bundle`: not skills-dir writers (not read in detail).
So only `skill` can place a directory under `.claude/skills/`.

### Experiment (scratch only)

A throwaway skill `modskill` with `SKILL.md`, `.claude-plugin/plugin.json`, `hooks/hooks.json` (`{"modules":["./register.js"]}`), `hooks/register.js` and a symlink `link-test` was added from a path source with `grim add ../src/modskill` in a scratch git workspace, `GRIM_HOME` and `HOME` inside the scratch folder.

Result at project scope, `.claude/skills/modskill/` contained exactly: `.claude-plugin/plugin.json`, `hooks/hooks.json`, `hooks/register.js`, `SKILL.md`. The symlink was dropped. Result at global scope (`grim install -g ../src/modskill`, `CLAUDE_CONFIG_DIR` in scratch): the same four files under `<cfg>/skills/modskill/`.

## Half 2, Claude side

Version tested: `claude 2.1.327`. Pages fetched 2026-11-18.

### Docs, quoted

[loading.md](https://code.claude.com/docs/en/plugins/loading.md), origin table:
"`@skills-dir` | You saved a plugin directory that has a `.claude-plugin/plugin.json` under `~/.claude/skills/` or the project's `.claude/skills/` | The manifest's `defaultEnabled`, unless a settings file sets `"<name>@skills-dir"` to `true` or `false`".

- Requirement is the manifest; the page does not say `SKILL.md` is required for the plugin to load. For both files present, the managed-settings section says: "Unless skills are among the locked kinds, a `SKILL.md` at the folder's root still loads as a plain skill. In that case `/<name>` keeps working, and the `/plugin` Installed tab lists that skill under Skills." That sentence describes the blocked-plugin case; it implies a folder with both can act as a plain skill, and (when not blocked) as a plugin. Whether both register in the unblocked case is shown only by the experiment below (plugin listed, SKILL.md present).
- Scope: user (`~/.claude/skills/`, which is `$CLAUDE_CONFIG_DIR/skills/`) and project (`.claude/skills/` of the session's primary working directory only). "unlike plain skills and commands, [it] doesn't search parent directories for plugins." A plugin at a subdirectory session start does not load. Desktop app in a worktree loads from the main checkout.
- Trust: "A project-scope plugin ... loads only after the same trust check that applies to project allow rules in `.claude/settings.json`. Trusting a parent folder or running with `-p` isn't enough." "Neither the trust check nor these component limits apply to a plugin under your own `~/.claude/skills/`." Repository plugins: MCP servers get per-server approval; "Background monitors do not load".
- Loading is in place, never copied: "`--plugin-dir` and skills-directory plugins: the directory loads in place and is never copied." Node dependencies are not auto-installed for in-place plugins (the install step is for cache copies).
- Name conflicts: `--plugin-dir` beats installed marketplace beats skills-dir; between user and project skills-dir copies the user copy wins.
- Managed settings can skip the whole folder: `strictPluginOnlyCustomization`, or `strictKnownMarketplaces` allowlist without `{ "source": "skills-dir" }`, or `blockedMarketplaces` listing it. No error is shown. `/plugin` shows nothing for a skipped folder.

[mods/admin.md](https://code.claude.com/docs/en/plugins/mods/admin.md): "In an interactive session in a directory the user hasn't trusted yet, no mod loads until they answer the trust prompt." `allowManagedModsOnly` (option on `cc-plugin-sec-default@builtin`) refuses "a mod in a plugin the user installed, a mod loaded with `--plugin-dir`, and a mod Claude wrote during a session"; the page does not name skills-dir explicitly, but a skills-dir plugin is a user-origin plugin, so it counts as a user's mod by the page's own rule ("Every other mod counts as a user's"). A mod "Claude writes during a session loads only when the allowlist includes `skills-dir`" under a marketplace allowlist. `enabledPlugins` is only needed to override `defaultEnabled`.

[mods/overview.md](https://code.claude.com/docs/en/plugins/mods/overview.md): a mod is a plugin with a hooks module; no sentence there ties modules to or excludes them from the skills-dir origin. "Hooks run" in `claude -p`.

[mods/reference.md](https://code.claude.com/docs/en/plugins/mods/reference.md): file table requires `.claude-plugin/plugin.json`, `hooks/hooks.json` with `modules`, and the module. No `skills-dir` or `SKILL.md` text found by grep on the fetched page.

### Experiment (isolated, no login)

- `claude plugin list` with `CLAUDE_CONFIG_DIR` pointed at the grim-installed global tree printed: `modskill@skills-dir, Version: 0.0.1, Scope: user, Status: loaded`. So grim's global install is discovered as a plugin by Claude 2.1.327, with `SKILL.md` and the manifest both present.
- `claude plugin list` in the scratch project (project scope, untrusted workspace, empty config dir) printed: "1 project-scope directory under ./.claude/skills/ that may load as a plugin was skipped because this workspace was not trusted when plugins were scanned. After accepting the trust dialog, run /reload-plugins (or relaunch) to load what qualifies." This matches the docs: project scope needs the trust dialog.
- `claude plugin validate` on the directory refused my first module because it imported `node:fs`: "a hooks module imports its own files by relative path and "claude-code", nothing else". A mod must use the `$` API (`$.fs.write`), not Node modules. The marker test as written would have failed for that reason, so it was not a valid probe anyway.
- `claude -p "say ok"` in the scratch project with an isolated config dir answered "Not logged in · Please run /login". Per the brief the run stopped there. No marker test was performed. Whether the module's code runs from the skills-dir origin is therefore **not observed**.

## Verdict per half

1. grim packs and installs `.claude-plugin/` and `hooks/`: **confirmed** (code plus experiment, project and global scope). Limits: symlinks dropped, `node_modules/` and a few dev dirs ignored by default, `SKILL.md` mandatory for the kind.
2. Claude loads the plugin from `.claude/skills/` and runs its modules: **loading as a plugin confirmed** (docs, `claude plugin list` at user scope); **module execution not observed**; project scope is gated by the folder trust dialog (docs, and the skipped-directory notice); user scope is not.

## negative:

- No file filter in grim blocks `.claude-plugin/` or `hooks/`; nothing found that strips them.
- Not verified: that `modules` execute from the skills-dir origin. No page found excludes them, none states they run.
- The first probe module was invalid (`node:fs` import); a valid probe needs `$.fs.write`.
- Claude and grim detail not checked: `rule`, `agent`, `hook`, `mcp`, `settings` placement was read from rule files, not from the code.
- Pinning `SKILL.md` frontmatter rendering: not tested whether a `metadata` `claude.*` key on the same `SKILL.md` interacts with the plugin.

## leads:

- Finish the module test with login: use a valid module (`$.fs.write` to a scratch path, or `$.ui.log` and read the `--debug` log for `hooks module modskill@skills-dir loaded`) at user scope in the scratch config, copying only credentials as the owner chooses.
- Test the project-scope path after the workspace trust dialog, and `allowManagedModsOnly` with `skills-dir`, which is the enterprise-blocking behavior.
- A skill-kind delivery is silently skipped under managed `strictPluginOnlyCustomization` or a marketplace allowlist without `skills-dir`; a dedicated kind could surface that as a grim warning.
- A mod with npm dependencies needs `!node_modules/` in `.grimignore` and, since the origin loads in place, no automatic dependency install.
- `grim` hook-kind subsystem rule, `.claude/rules/subsystem-install-layout-hooks.md`, to confirm the one-line claim above.

## Sources

- https://code.claude.com/docs/en/plugins/loading.md : origins, trust, in-place loading, managed blocking (fetched 2026-11-18)
- https://code.claude.com/docs/en/plugins/mods/admin.md : trust prompt, `allowManagedModsOnly`
- https://code.claude.com/docs/en/plugins/mods/overview.md : mod definition, where mods run
- https://code.claude.com/docs/en/plugins/mods/reference.md : file layout
- Local: `src/skill/skill_package.rs`, `src/install/ignore_set.rs`, `src/install/materializer.rs`, `.claude/rules/subsystem-install-layout-skills-rules.md`

## Scratch artifacts

`/tmp/claude-1000/-home-mherwig--herdr-worktrees-grimoire-duo/6dfdf366-5cef-4de7-a681-0f9aeeb626a4/scratchpad/modcheck/` holds `src/modskill/`, `proj/` (git, `grimoire.toml`, `.claude/skills/modskill/`), `grimhome/`, `ghome/`, `cfg/` (global install), `cfg2/` (empty isolated config), `NOTE.txt`.
