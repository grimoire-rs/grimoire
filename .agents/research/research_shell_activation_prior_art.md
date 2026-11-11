# Research: Shell activation — prior art

## Metadata

**Date:** 2026-11-10
**Domain:** cli
**Triggered by:** /hex-discuss shell-activation
**Expires:** 2027-05-10

Scope: how comparable CLI tools wire a shell rc line for completion, a directory/file hook, update detection, auto-update and auto-reinstall. Neutral evidence FOR and AGAINST per approach; no recommendation. Evidence quality: claims come from web-search summaries and page fetches by a leaf worker; items marked **[verify]** were second-hand, version-dependent, or contradicted by another source. No source was older than May 2025 unless flagged **[old]**.

## 1. Activation pattern

| Approach | Used by | FOR | AGAINST |
|---|---|---|---|
| `eval "$(tool init <shell>)"` (generated at each shell start) | direnv, mise, fnm, starship, zoxide, atuin | Output always matches the installed binary; one rc line; shell auto-detected from `MISE_SHELL`/`SHELL` for mise. | Spawns the binary at every shell start; one write-up measured uncached init calls at roughly 40-100 ms each (direnv hook, fnm env, zoxide init, brew shellenv). Author figures, not controlled benchmarks. |
| Cached static file, sourced | community helpers (evalcache, zsh-smartcache, per-dotfiles helpers) | Sourcing a file claimed ~1 ms vs 50-200 ms. | Needs invalidation (binary mtime, 24h TTL, manual delete); "a cache that regenerates every time is worse than no cache". Third-party add-ons, not a tool feature. |
| Shims on `PATH` | mise (`--shims`), asdf, volta | Work for IDEs/GUI apps that never see an activated shell, and for non-interactive use. | mise docs: shims do not set `[env]`, do not run `cd`/`enter`/`leave`/`watch_files` hooks; cost is paid per command; `command -v` can report a tool as installed when it is not. |
| Per-shell import files (elvish `use direnv`, nushell `env_change.PWD` snippet) | direnv | Idiomatic per shell. | Per-shell maintenance; nushell needs >= 0.104 for direnv's snippet. |

Shell coverage (documented):
- direnv: bash, zsh, fish, tcsh, elvish, nushell, PowerShell, murex ([man page](https://direnv.net/man/direnv.1.html), [hook docs](https://github.com/direnv/direnv/blob/master/docs/hook.md)).
- mise activate: bash, elvish, fish, nu, xonsh, zsh, pwsh, powershell ([activate](https://mise.jdx.dev/cli/activate.html)). It errors on cmd and unsupported shells when the shell cannot be detected.
- clap_complete dynamic: bash, elvish, fish, PowerShell, zsh; nushell not listed ([env docs](https://docs.rs/clap_complete/latest/clap_complete/env/index.html)).

Known breakage:
- Non-interactive shells: prompt hooks do not fire. A Claude Code issue reports the Bash tool runs a non-interactive shell that skips `~/.bashrc`, so direnv/devbox hooks never run; reporter proposes a SessionStart hook running `direnv export` ([claudeissues #42229](https://claudeissues.com/issue/42229-docs-guide-direnv-devbox-environment-loading-via-hooks)). Community report, unofficial workaround **[verify]**.
- Claude Code snapshots the shell environment once at session start and sources that static file for every tool call, so later directory changes do not re-run hooks (same source, plus [#3330](https://claudeissues.com/issue/3330-bug-shell-snapshot-cause-incorrect-environment-loaded)). Windows Git Bash snapshots produced from a login shell stripped activator PATH entries ([#57435](https://claudeissues.com/issue/57435-bash-tool-shell-snapshot-is-generated-by-a-login-shell-stripping-inherited-path), reported resolved).
- mise recommends `mise exec -- cmd` or `mise run` for scripts/CI rather than relying on the hook ([shims doc](https://mise.jdx.dev/dev-tools/shims.html)).
- Windows: mise says GUI programs ignore `$PROFILE`, so shims dir must go on the user `Path`; PowerShell 5.x and nushell update at the next prompt only (same doc). cmd is not a supported activation target for mise.
- Fish: direnv offers `direnv_fish_mode` (`eval_on_arrow` default) because fish has no pre-prompt hook equivalent (hook docs).
- Nested shells / IDE terminals: no authoritative source found; treat as unresearched.

## 2. Trigger model

| Trigger | Catches lock changed by `git pull` while staying in the same dir? | Notes |
|---|---|---|
| Prompt hook (`precmd`, `PROMPT_COMMAND`) | Yes, at the next prompt, if the hook checks file state. | direnv and mise both run before each prompt. Cost is per prompt, so must early-exit. |
| `chpwd` / `cd` hook | No (no directory change). | fnm `--use-on-cd` is this model ([fnm config](https://github.com/Schniz/fnm/blob/master/docs/configuration.md)). mise ships `hook_env.chpwd_only` which skips the prompt hook when the directory is unchanged ([PR 7312 summary](https://github.com/jdx/mise/pull/7312), merge not confirmed **[verify]**). |
| Git hooks `post-checkout`, `post-merge`, `post-rewrite` | Yes for the pull that changed it, independent of shell. | `post-checkout` fires on checkout/switch/clone/worktree add (args: prev, new, branch flag). `post-merge` fires on `git merge` incl. the merge step of `pull`, not on conflicts, and cannot affect the outcome. `post-rewrite` covers `rebase` and `amend`, so `pull --rebase` needs it ([githooks](https://git-scm.com/docs/githooks)). The page does not state fast-forward or `--no-verify` behaviour **[verify by test]**. `reference-transaction` fires on any ref update. |
| File watcher | Yes, but needs a resident process. | No comparable CLI found that does this inside the shell hook; mise `[[watch_files]]` hooks run inside the prompt hook, not a daemon ([hooks doc](https://mise.jdx.dev/hooks.html), partly experimental in older versions **[verify]**). |

What direnv does: runs before each prompt, looks for `.envrc` in current and parent dirs; reload on mtime change of watched files; `direnv reload` forces ([man page](https://direnv.net/man/direnv.1.html)). nix-direnv watches `flake.nix` and `flake.lock` by default, extra files need `watch_file` before `use flake`; manual mode only prints "cache is out of date" ([nix-direnv](https://www.github.com/nix-community/nix-direnv)). Inference only (not confirmed): a lock changed by `git pull` triggers reload at the next prompt via mtime.

What mise does: hook runs at each prompt and (most shells) on `cd` ([shims doc](https://mise.jdx.dev/dev-tools/shims.html)). `hook-env` source comment: "Shell activation must stay fast and non-networked"; it builds the toolset `offline: true` and merges watch files from the previous session ([hook_env.rs](https://docs.rs/crate/mise/latest/source/src/cli/hook_env.rs)). Fast path added in [PR 7073](https://github.com/jdx/mise/pull/7073): early exit before tool loading and config parsing by checking `MISE_` env, config mtimes, data dir; comment claims no-change case drops from ~40-60 ms to ~8-10 ms (self-reported estimate). Newer source adds directory check, in-memory env comparison and TTL skip (`hook_env.cache_ttl`, `MISE_HOOK_ENV_CACHE_TTL`).

Latency budgets stated/measured:
- mise: 8-10 ms no-change (self-reported); docs give no numbers ([activate](https://mise.jdx.dev/cli/activate.html), [shims](https://mise.jdx.dev/dev-tools/shims.html)).
- direnv: no stated budget; one shell-startup breakdown measured the hook at ~0.27 s at startup ([HN reader mirror](https://hn.jakechampion.name/reader/https://tech.northisland.ventures/posts/shell-startup-ai/)) **[verify]**.
- Real regression: after mise v2026.10.3, `redact = true` values were not stored in `env_cache`, so each prompt re-ran 1Password `op read`, ~3 s per Enter; workaround `hook_env.chpwd_only = true` ([classmethod blog](https://dev.classmethod.jp/articles/mise-secret-cache-fnox-daemon/)). Shows per-prompt work scales with whatever the config does.

Low-cost techniques seen: mtime/fingerprint comparison before parsing (mise); a `__MISE_ZSH_PRECMD_RUN`-style guard env var; TTL skip; lazy stub that removes itself on first use (zoxide wrapper pattern in dotfiles guides). Early exit in shell code before spawning the binary: not found documented for any of the compared tools; they all spawn the binary per prompt. Gap.

## 3. Auto-install on lock/manifest change

| Tool | Behaviour |
|---|---|
| mise | `auto_install` default `true`, but covers `mise x`, `mise run` and the not-found handler, not `cd`; the activation hook updates env/PATH and does not install ([settings](https://mise.jdx.dev/configuration/settings.html), [getting started](https://mise.jdx.dev/getting-started)). Third-party guide claims "install on cd" **[verify; conflicts with source reading]**. v2026.9.15 now warns when auto-install is off and it falls back to a same-named PATH binary ([release](https://newreleases.io/project/github/jdx/mise/release/v2026.9.15)). |
| uv | `uv run` locks and syncs before running by default; `--locked` errors on stale lock, `--frozen` uses lock as-is, `--no-sync` skips; env forms `UV_LOCKED`, `UV_FROZEN`, `UV_NO_SYNC` ([sync](https://docs.astral.sh/uv/concepts/projects/sync/), [env](https://docs.astral.sh/uv/reference/environment/)). Sync is on a real command, not a shell hook. |
| pixi | `install`, `run`, `shell`, `shell-hook` auto-update lock; `--frozen` / `--locked` / `--as-is` opt out ([pixi shell](https://pixi.prefix.dev/latest/reference/cli/pixi/shell/)). |
| direnv / nix-direnv | Reloads environment on watched-file change (incl. `flake.lock`); manual mode only warns. |
| devbox | `devbox generate direnv` creates `.envrc`; `devbox.json` changes trigger direnv reset. `devbox.lock` reload not confirmed ([PR 1748](https://github.com/jetify-com/devbox/pull/1748/files)). |
| fnm | `--use-on-cd` switches version on directory change; install only if `fnm install`/prompt ([config](https://github.com/Schniz/fnm/blob/master/docs/configuration.md)). |
| Git-hook route (husky, lefthook, `package-changed`, `git-pull-run`) | Scripts diff lockfile between old/new HEAD in `post-merge`/`post-checkout`/`post-rewrite`, then run `npm ci`/`pnpm install` ([dev.to](https://dev.to/zirkelc/automatically-install-npm-dependencies-on-git-pull-bg0), [lefthook course](https://www.newline.co/courses/bundling-and-automation-in-monorepos/automated-git-hooks-with-lefthook), [package-changed](https://unpkg.com/package-changed@3.0.0/README.md)). |

Git-hook route trade-offs:
- FOR: fires exactly when the lock changes, independent of the user's shell, IDE or agent; works in non-interactive use; no per-prompt cost.
- AGAINST: hooks are not cloned with the repo (each clone must install them, e.g. husky `prepare`); `post-checkout` also fires on rebase internals, so naive scripts run repeatedly; three hooks needed to cover merge, rebase, checkout; monorepo scripts must `cd` to the lock dir; Windows quoting issues reported (`git-pull-run` README); post hooks cannot block or report into the git operation; install runs inside a git command and a failure only exits the hook. Husky v7 changed config format, breaking older recipes.

Shell-hook route trade-offs: FOR: no per-repo install, also catches edits by other tools; AGAINST: never fires in non-interactive/agent shells; per-prompt cost; no event for "what changed".

User complaints / reverted defaults on auto-install: none found by search. Gap. The closest are mise's new fallback warning (above) and trust-bypass advisories (axis 6).

## 4. Update notification

| Tool | Mechanism | TTL / frequency | Opt-out |
|---|---|---|---|
| npm `update-notifier` | Detached unref'd child process checks, result cached; shown on next run ([repo](https://github.com/yeoman/update-notifier)) | 1 day default; first run waits one interval before notifying | `NO_UPDATE_NOTIFIER`, `--no-update-notifier`, config `optOut`; auto-skipped in CI and `NODE_ENV=test` |
| GitHub CLI | Check on any command, notice on stderr ([env doc](https://cli.github.com/manual/gh_help_environment)) | every 24 h | `GH_NO_UPDATE_NOTIFIER` (any value); separate `GH_NO_EXTENSION_UPDATE_NOTIFIER` |
| mise | Version notice in `mise version`/`--version`/`doctor`; `self-update` command ([settings](https://mise.jdx.dev/configuration/settings.html), [self-update](https://mise.jdx.dev/cli/self-update.html)) | `auto_update_check_duration` default 7d (deprecated alias of `self_update.check_duration`) | `disable_update_warning`; self-update default min release age 24h; packagers can disable the command |
| oh-my-zsh | Shell-start check with prompt/reminder ([README](https://github.com/ohmyzsh/ohmyzsh#getting-updates)) | default prompt every 2 weeks; `frequency 0` = every terminal start (not recommended) | `zstyle ':omz:update' mode disabled` |
| Homebrew | Update before `install`/`upgrade`/`tap` | `HOMEBREW_AUTO_UPDATE_SECS` (default 86400 per a mirror of `env_config.rb` **[verify]**), API check default 450 s | `HOMEBREW_NO_AUTO_UPDATE=1` ([manpage](https://docs.brew.sh/Manpage), [FAQ](https://docs.brew.sh/FAQ)) |
| deno, rustup, volta, chezmoi | Not confirmed. Deno upgrade page documents no update-check variable; rustup has `RUSTUP_AUTO_SELF_UPDATE` and an open request for a `NO_SELF_UPDATE` env var ([issue 3821](https://redirect.github.com/rust-lang/rustup/issues/3821)). | n/a | n/a |

Pattern summary: background child or on-real-command check plus on-disk cache; 1 d to 7 d TTL typical; CI auto-skip only documented for update-notifier. Complaints about noise: only Homebrew CI slowness threads found ([CircleCI](https://discuss.circleci.com/t/brew-update-is-painfully-slow/32871)). Not found: a tool that does the update check inside a prompt hook.

## 5. Automatic updates

- Homebrew: auto-update of its own definitions before certain commands is default-on; manpage recommends raising the interval over disabling, warns disabling "may result in a broken configuration" when tapping ([manpage](https://docs.brew.sh/Manpage), via search summary). CI incidents: auto-update broke Travis/CircleCI builds; Bitrise says a Homebrew release dropped URL-based pinning and recommends `HOMEBREW_NO_AUTO_UPDATE=1` ([Travis thread](https://travis-ci.community/t/syntax-error-unexpected-keyword-rescue-expecting-keyword-end-in-homebrew/5623/2), [Bitrise](https://discuss.bitrise.io/t/homebrew-auto-update-for-version-2-5-0-incident-september-8th/14214)). Homebrew updates its own package index, not user project files.
- oh-my-zsh: three modes (`auto`, `reminder`, `disabled`), default is a prompt every 2 weeks; supports `cooldown N` to apply only updates at least N days old; `omz update --unattended` was removed "because it had side effects" ([README](https://github.com/ohmyzsh/ohmyzsh#getting-updates)). Older `DISABLE_AUTO_UPDATE` variables still honoured per a secondary source **[verify]**.
- mise: `self_update.auto` (off by default per deprecated alias default `false`); `minimum_release_age` default 24h for self-update ([self-update](https://mise.jdx.dev/cli/self-update.html)).
- Package-manager vs self-updater conflict: a Claude Code issue reports a Homebrew-installed copy updated in place by the app's own updater, conflicting with Homebrew's management ([claude-code #41194](https://github.com/anthropics/claude-code/issues/41194)). Self-update on package-manager-installed binaries is a known conflict class; mise lets packagers disable `self-update`.
- Auto-update modifying a git-committed lock file: no evidence found for any tool doing this unattended. uv/pixi only change the lock on an explicit project command and offer `--locked`/`--frozen`; uv's `--upgrade` is the only path that moves pins ([uv sync](https://docs.astral.sh/uv/concepts/projects/sync/)). Gap: no incident write-ups located; absence of evidence, not evidence of safety.
- Backlash found for auto-update defaults specifically: CI slowness/breakage (Homebrew) only; no user-revolt write-up found for oh-my-zsh.

## 6. Trust and security

| Tool | Model | Notes |
|---|---|---|
| direnv | `.envrc` blocked until `direnv allow`; allow list under `$XDG_DATA_HOME/direnv/allow`; a changed file is re-blocked (hash differs); `deny`, `prune` ([man page](https://direnv.net/man/direnv.1.html)) | Man page: purpose is that a cloned repo or unpacked archive cannot run code on `cd`. No CVE found for the core trust model (search: only an unexplained distro-package advisory GHSA-cxq7-xw9v-rcv3, [Snyk](https://security.snyk.io/vuln/SNYK-CHAINGUARDLATEST-DIRENV-13811111)). Whitelist directives widen trust to directory trees (SO answer, secondary). |
| mise | `mise trust` per config; untrusted config prompts, is skipped, or errors; configs with only plain versions and tasks without secrets need no trust; `run`/`install`/`exec` auto-trust their active config; paranoid mode hashes content ([trust](https://mise.jdx.dev/cli/trust.html)) | Global-only settings: `trusted_config_paths`, `paranoid` named as examples; `auto_update*` options marked global-only ([settings](https://mise.jdx.dev/configuration/settings.html)). |
| VS Code Workspace Trust | Restricted Mode disables/limits tasks, debugging, workspace settings (those tagged `requireTrustedWorkspace`), terminal prompts, extensions; no startup prompt by default (`startupPrompt: never`) ([docs](https://code.visualstudio.com/docs/editing/workspaces/workspace-trust)) | |

Incidents:
- mise [CVE-2026-35533 / GHSA-436v-8fw5-4mj8](https://github.com/jdx/mise/security/advisories/GHSA-436v-8fw5-4mj8) (CVSS 7.7, CWE-284; affected from 2026.2.18; advisory lists patched 2026.6.4, another source says 2026.4.6 **[conflict]**): a project `.mise.toml` could set `trusted_config_paths`, `yes`, `ci` or `paranoid` and thereby mark itself trusted, enabling `_.source` execution during `hook-env`. Fix: ignore those four trust-control keys from non-global config. A later Debian tracker entry (CVE-2026-103929, "prior to 2026.7.14", untrusted `.mise.toml` `[settings]` loaded before trust approval) suggests the class recurred ([Debian](https://security-tracker.debian.org/tracker/CVE-2026-103929), secondary **[verify]**).
- Cursor: Workspace Trust off by default let `.vscode/tasks.json` with `runOn: folderOpen` execute on opening a repo ([Oasis, Sept 2025](https://www.oasis.security/blog/cursor-security-flaw), [Help Net Security](https://www.helpnetsecurity.com/2025/09/11/cursor-ai-editor-vulnerability/)); Cursor reportedly treats the default as intentional. No CVE found.
- VS Code task-based campaigns: North Korean actors reported abusing auto-run tasks; trust is sticky, so a later `git pull` can add a malicious task with no new prompt ([vpncentral](https://vpncentral.com/?p=247239), [NHI Mgmt](https://nhimg.org/articles/vs-code-tasks-files-let-attackers-trigger-developer-infection-chains/)); secondary sources **[verify]**.
- Direct relevance: trust that is tied to a path rather than content (mise non-paranoid, VS Code) survives a `git pull` that rewrites the file; direnv re-prompts on content change.

## 7. Completions (`clap_complete`)

- `CompleteEnv` is still gated behind the `unstable-dynamic` crate feature; docs state the interface between generated shell code and the program is unstable and recommend regenerating at shell startup instead of saving to a file ([env docs](https://docs.rs/clap_complete/latest/clap_complete/env/index.html)). Changelog never mentions stabilisation; latest release seen 4.6.11 (2026-09-15) ([CHANGELOG](https://github.com/clap-rs/clap/blob/master/clap_complete/CHANGELOG.md)).
- Shells: bash, elvish, fish, PowerShell, zsh per docs.rs. Conflict: a March 2026 task description says Elvish native completion and dynamic fish completion were gaps ([harbor task](https://hub.harborframework.com/tasks/abundant/clap-rs__clap-5621)) **[verify against current code]**. Nushell absent.
- Registration: app calls `CompleteEnv::with_factory(cli).complete()`; user adds `source <(COMPLETE=bash prog)` / `source <(COMPLETE=zsh prog)` / `COMPLETE=fish prog | source`; `COMPLETE=` or `0` disables. Setting `COMPLETE` makes the binary emit script or completions instead of normal run.
- Static generation (`clap_complete::generate`) is the stable alternative: file generated at build/release or via a subcommand; no runtime dependency on the binary being on `PATH`, but values (e.g., installed skill names) cannot be completed dynamically. Static-API stability not separately fetched; the changelog shows continued fixes (4.6.9 bash function names, 4.6.11 zsh escaping).
- Cost: the dynamic scheme spawns the binary on every TAB for candidates, plus one spawn at shell start if `source <(...)`. Cached-init write-ups put the spawn at 40-100 ms uncached ([dev.to](https://dev.to/martin_oehlert/from-14s-to-53ms-optimizing-zsh-startup-on-macos-5f09), [evalcache](https://github.com/mroth/evalcache) via search). Bundling completion registration into the activation line is the pattern of direnv (none), mise (`mise completion` is separate) and starship/zoxide (`init` includes completions for some shells); not verified per tool.

## Per-tool comparison

| Tool | Activation | Shells | Trigger | Auto-install | Update check | Trust model |
|---|---|---|---|---|---|---|
| direnv | `eval "$(direnv hook sh)"` | bash zsh fish tcsh elvish nu pwsh murex | prompt; mtime watch | n/a (reload env) | none | `direnv allow` hash-based |
| mise | `eval "$(mise activate sh)"`, or shims | bash zsh fish nu elvish xonsh pwsh | prompt + cd; fast path | on `x`/`run`/not-found, not on cd | 7d, version notice | `mise trust`; trust keys global-only after CVE |
| fnm | `eval "$(fnm env --use-on-cd)"` | several | cd only | via command | n/a | none |
| nix-direnv | `.envrc` function | via direnv | prompt via direnv | manual-reload option | none | direnv |
| devbox | `generate direnv` | via direnv | via direnv | on devbox commands | not checked | direnv |
| pixi | `shell-hook` output | several | none (explicit) | on `run`/`shell`/`shell-hook` | not checked | none |
| uv | none | n/a | none | on `uv run` (sync) | not checked | n/a |
| starship/zoxide/atuin | `eval "$(x init sh)"` | many | prompt/cd/keybinding | n/a | n/a | n/a |
| Homebrew | none (`brew shellenv`) | n/a | on command | auto-update on install/upgrade/tap | 24h | n/a |
| oh-my-zsh | sourced in `.zshrc` | zsh | shell start | n/a | 2 weeks, 3 modes | n/a |
| gh / update-notifier | n/a | n/a | on command | n/a | 24h/1d, CI skipped | n/a |
| husky / lefthook | git hooks | any | git events | script-defined | n/a | hooks not versioned by default |
| VS Code / Cursor | n/a | n/a | folder open | tasks | n/a | Workspace Trust (Cursor off by default) |

## Sources

| URL | Type | Date | Covers |
|---|---|---|---|
| https://direnv.net/man/direnv.1.html | official doc | current | hook lines, allow/deny, shells |
| https://github.com/direnv/direnv/blob/master/docs/hook.md | official doc | current | per-shell setup, fish mode, nushell |
| https://mise.jdx.dev/cli/activate.html | official doc | current | activate, shells, hook-env |
| https://mise.jdx.dev/dev-tools/shims.html | official doc | current | shims vs activate trade-offs |
| https://mise.jdx.dev/cli/trust.html | official doc | current | trust semantics |
| https://mise.jdx.dev/configuration/settings.html | official doc | current | auto_install, update settings (first 100k chars read) |
| https://mise.jdx.dev/cli/self-update.html | official doc | current | self-update, min release age |
| https://docs.rs/crate/mise/latest/source/src/cli/hook_env.rs | source | 2026.5.5 | hook-env offline, early exit |
| https://github.com/jdx/mise/pull/7073 | PR (search summary) | 2025-26 | early-exit fast path, 40-60 to 8-10 ms claim |
| https://github.com/jdx/mise/pull/7312 | PR (search summary) | 2025-26 | chpwd_only, cache_ttl; merge unconfirmed |
| https://dev.classmethod.jp/articles/mise-secret-cache-fnox-daemon/ | blog | 2026 | per-prompt 3 s regression |
| https://github.com/jdx/mise/security/advisories/GHSA-436v-8fw5-4mj8 | advisory | 2026-04-03 | trust-control settings bypass |
| https://security-tracker.debian.org/tracker/CVE-2026-103929 | tracker | 2026 | recurrence, secondary |
| https://newreleases.io/project/github/jdx/mise/release/v2026.9.15 | release notes | 2026-09 | auto-install PATH fallback warning |
| https://mise.jdx.dev/hooks.html | official doc (search hit only) | current | `[[watch_files]]` hook |
| https://www.github.com/nix-community/nix-direnv | repo | current | default watched files |
| https://github.com/jetify-com/devbox/pull/1748/files | PR | 2025 | devbox.json reset of direnv |
| https://pixi.prefix.dev/latest/reference/cli/pixi/shell/ | official doc | current | lock auto-update flags |
| https://docs.astral.sh/uv/concepts/projects/sync/ | official doc | current | uv run auto lock/sync |
| https://docs.astral.sh/uv/reference/environment/ | official doc | current | UV_LOCKED/FROZEN/NO_SYNC |
| https://github.com/Schniz/fnm/blob/master/docs/configuration.md | official doc | current | --use-on-cd |
| https://git-scm.com/docs/githooks | official doc | current | post-checkout/merge/rewrite |
| https://dev.to/zirkelc/automatically-install-npm-dependencies-on-git-pull-bg0 | blog | unknown **[verify]** | git-hook npm install recipe |
| https://www.newline.co/courses/bundling-and-automation-in-monorepos/automated-git-hooks-with-lefthook | course | unknown **[verify]** | lefthook three-hook setup |
| https://unpkg.com/package-changed@3.0.0/README.md | package readme | unknown **[verify]** | lockfile-change install |
| https://github.com/yeoman/update-notifier | repo | current | background check, 1 d, CI skip |
| https://cli.github.com/manual/gh_help_environment | official doc | current | gh 24 h notifier |
| https://github.com/ohmyzsh/ohmyzsh#getting-updates | official doc | current | omz update modes |
| https://docs.brew.sh/Manpage , https://docs.brew.sh/FAQ | official doc | current (manpage flagged ~old by search) **[verify]** | auto-update variables |
| https://travis-ci.community/t/syntax-error-unexpected-keyword-rescue-expecting-keyword-end-in-homebrew/5623/2 | forum | **[old]** | auto-update broke CI |
| https://discuss.bitrise.io/t/homebrew-auto-update-for-version-2-5-0-incident-september-8th/14214 | forum | **[old]** | pin removal incident |
| https://discuss.circleci.com/t/brew-update-is-painfully-slow/32871 | forum | **[old]** | auto-update slowness |
| https://github.com/anthropics/claude-code/issues/41194 | issue | 2026 | self-updater vs Homebrew |
| https://redirect.github.com/rust-lang/rustup/issues/3821 | issue | unknown **[verify]** | rustup self-update env |
| https://code.visualstudio.com/docs/editing/workspaces/workspace-trust | official doc | current | Restricted Mode |
| https://www.oasis.security/blog/cursor-security-flaw | security blog | 2025-09 | Cursor folderOpen autorun |
| https://www.helpnetsecurity.com/2025/09/11/cursor-ai-editor-vulnerability/ | news | 2025-09 | same |
| https://vpncentral.com/?p=247239 , https://nhimg.org/articles/vs-code-tasks-files-let-attackers-trigger-developer-infection-chains/ | news | 2026-01 | VS Code tasks campaigns, sticky trust |
| https://security.snyk.io/vuln/SNYK-CHAINGUARDLATEST-DIRENV-13811111 | advisory | 2025-11 | direnv distro advisory, unexplained |
| https://claudeissues.com/issue/42229-docs-guide-direnv-devbox-environment-loading-via-hooks | issue mirror | 2026 | agent shells skip rc hooks |
| https://claudeissues.com/issue/3330-bug-shell-snapshot-cause-incorrect-environment-loaded | issue mirror | 2025 | snapshot staleness |
| https://claudeissues.com/issue/57435-bash-tool-shell-snapshot-is-generated-by-a-login-shell-stripping-inherited-path | issue mirror | 2026 | Windows login-shell PATH loss |
| https://docs.rs/clap_complete/latest/clap_complete/env/index.html | official doc | current | CompleteEnv, unstable-dynamic, registration |
| https://github.com/clap-rs/clap/blob/master/clap_complete/CHANGELOG.md | changelog | to 2026-09-15 | versions, shell history |
| https://hub.harborframework.com/tasks/abundant/clap-rs__clap-5621 | task mirror | 2026-03 | claimed Elvish/fish dynamic gaps **[verify]** |
| https://dev.to/martin_oehlert/from-14s-to-53ms-optimizing-zsh-startup-on-macos-5f09 , https://github.com/mroth/evalcache | blogs | unknown | init caching costs |
| https://hn.jakechampion.name/reader/https://tech.northisland.ventures/posts/shell-startup-ai/ | blog mirror | 2026 | direnv hook ~0.27 s at startup |
