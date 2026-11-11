# Discussion: grim shell activation

State: handed-off → architect · Updated: 2026-11-11
Ratified: 2026-11-11 → architect
Confidence: ratified by the owner on the second restate, after declining the first; decisions rest on three research files dated 2026-11-10 (`research_shell_activation_prior_art.md`, `research_shell_activation_recon.md`, `research_shell_activation_update_cache.md`).

## Intent

The owner's ask, 2026-11-10: "How could we implement a good grim shell activation that support auto-completion and maybe even a shell hook that auto-detect available updates and maybe even auto-update based on global/project config. IMHO we could steal a lot of proven impl from ocx here. Especially with gitignored artifacts auto-rematerializing on changed lock would also be interesting."

Later the same day the owner narrowed it: automatic updating makes sense at global scope and is doubtful in a project, because a checkout of an older branch carries older pins on purpose.

Out of scope:

- Installing or writing any file from the prompt hook.
- Automatic update of a project's `grimoire.lock`.
- Dynamic (value) completion. `clap_complete`'s `CompleteEnv` is still behind `unstable-dynamic`.
- Editing the user's shell rc file. The user adds the activation line by hand.
- A grim command that installs git hooks. The git-hook route is a documented recipe.
- File watchers, daemons and shims.
- A hook for elvish or nushell. Elvish keeps its existing completions.
- Updating the grim binary itself.

## Requirements

Provisional prose. Command and key names are placeholders until the ADR fixes them.

- **Activation.** A new command, working name `grim shell init <shell>`, prints a script for bash, zsh, fish and PowerShell (PowerShell 7 and Windows PowerShell 5.1). The user evaluates it from their rc file. It registers the existing static completions and installs a prompt hook. `grim completions` keeps its current behavior.
- **Prompt cost.** An idle prompt spawns no process: a shell-builtin guard compares a stamp file and the working directory first, the shape ocx uses. The prompt path never touches the network and never waits on one.
- **Lock-drift notice.** In a project whose `grimoire.lock` no longer matches the installed outputs, the hook prints one line telling the user to run `grim install`. It writes nothing. This needs a new stat-only staleness check, because `grim status` hashes every output and its exit code is frozen at 0.
- **Git-hook recipe.** The docs show how to call `grim install` from `post-checkout`, `post-merge` and `post-rewrite`, which covers agent-run pulls and new worktrees that a prompt hook never sees. A no-op `grim install` must be fast enough for that; it costs about 230 ms today.
- **Reference check cache.** grim gains a machine-wide cache under `$GRIM_HOME`: for each declared reference (`registry/repository:tag`), the digest a checked round last saw and when it was checked. Every `grim status --check` round writes it, whoever started it: the user, a git hook, the VS Code extension, the shell hook's background run. It is a new file set. The existing tag cache at `$GRIM_HOME/tags/<registry>/<repository>/tags.json` stays byte-for-byte as it is: it has no timestamp, older grims reject unknown fields in it, and it feeds offline resolution, so writing check results into it would change what an offline run resolves to.
- **Local verdict.** "Is this pin behind?" is the lock pin compared with the cached digest. The hook computes it whenever the lock changes, with no network, so a checkout of another branch or a manual edit is reflected at once.
- **Background check.** For references the cache has never seen or holds for longer than the TTL, the hook starts a detached `grim status --check` round. Global scope is checked at shell startup; a project is checked the first time the prompt hook runs inside the workspace and again when its lock gains an unseen reference. The TTL is configurable. The check is skipped offline and in CI.
- **Notice rate.** The hook speaks when a finding appears or changes, and once at shell start while the finding is outstanding. It is silent on every other prompt, and silent in a non-interactive terminal.
- **Advisory only.** The cache informs notices. `grim install`, `grim update` and `grim status --check` keep resolving live, so a stale cache entry can never move or hold a pin.
- **Project-scope check reach.** The project-scope check runs in any workspace that has a lock, a fresh clone included.
- **Update mode option.** One key in the existing `options.*` set, declared in `src/command/config_keys.rs`, so `grim config list --all` and the VS Code extension's settings view show it. Values `none`, `notify`, `prompt` and `auto`; the default is `notify` at both scopes. `prompt` asks yes or no in an interactive terminal only. `auto` applies global-scope updates without asking. The TTL is a second key in the same set.
- **Project narrowing.** `prompt` and `auto` take effect for global scope only and only from global config. A project file may set `none` or `notify`; a wider value there is ignored with one warning. Nothing ever applies an update to a project's `grimoire.lock`.
- **Consent.** No path in this feature grants hook or settings consent. An unattended apply leaves a hook or settings artifact unarmed, as a non-interactive run does today.
- **Notice wording.** Notices use fixed wording and counts, never strings taken from the repository. This is the assistant's addition: it keeps terminal escape sequences out of the prompt.

## Decisions

- Lock drift at project scope: the prompt hook **notifies only**. It prints one line and never writes files (owner, 2026-11-10). This matches ocx, whose hook leaves materialization to an explicit command.
- Rematerialize after a checkout or pull runs through **opt-in git hooks** (`post-checkout`, `post-merge`, `post-rewrite`) that call `grim install`. The owner asked for documentation of that route. Reading it as a documented recipe, with no grim command that installs git hooks, is the assistant's interpretation.
- A no-op `grim install` must be fast enough to sit in a git hook (owner: "should have a fast noopt"). Today it costs about 230 ms against 9.1 ms for `grim status`.
- Update check: the hook starts a detached background check, throttled by a **configurable TTL**; the prompt never waits on the network (owner, 2026-11-10). This deliberately departs from C-041's "no background refresh" stance for this one path, and the ADR records that.
- Check scopes: shell startup checks global scope; the first prompt inside a workspace checks that project (owner, 2026-11-10).
- The project-scope check runs in **any workspace**, a fresh clone included (owner, 2026-11-10). The assistant recommended limiting it to workspaces grim had already installed in on this machine. The consequence is carried under Open questions.
- Project scope is **notify only**; global scope gets a mode setting `none | notify | prompt | auto` (owner, 2026-11-10). The owner's reason for the project half: a checkout of an older branch carries older pins on purpose.
- The mode defaults to `notify` at both scopes and is declared in the `options.*` set, so the VS Code extension displays it (owner, 2026-11-10).
- The notice is computed from a per-reference cache, so it needs no network call of its own (owner's direction, 2026-11-10: "per-reference cache seems fine"). The owner assumed grim already caches tag resolutions. It does, at `$GRIM_HOME/tags/`, but that cache holds only what the last lock, update or add on this machine saw, carries no time, and is read only offline, so it cannot say that something newer exists.
- The hook speaks on change and once at shell start (owner, 2026-11-10).
- `grim status --check` is the explicit check command; no new command (owner, 2026-11-10).
- Answering update checks from the index is deferred: the index records grim reads carry no digest per tag. Deferring it is the assistant's proposal, put to the owner at the restate.
- Shells in the first version: bash, zsh, fish and PowerShell, the last covering both PowerShell 7 and Windows PowerShell 5.1 (owner, 2026-11-10). Elvish keeps its existing completions and gets no hook.
- Drain target: first a goal loop whose first deliverable is an ADR (owner, 2026-11-10); changed to `/hex-architect` directly when the owner ratified the restate (owner, 2026-11-11). Planning and building start separately after the ADR.
- Deferring index-derived checks was ratified with the restate (owner, 2026-11-11).
- Carried from the assistant's opening position, not contested: the prompt path spawns nothing when idle (shell-builtin guard on a stamp file, the ocx shape) and never touches the network; completions stay static `clap_complete` output because dynamic completion is an unstable crate feature; project config may only narrow behavior, the lesson of mise CVE-2026-35533.

## Threads

- **Closed by research 2026-11-10 and ratified at the second restate 2026-11-11: update-check freshness (owner declined the first restate, 2026-11-10).** Outcome: the per-reference cache is new work beside the existing tag cache; the index cannot answer today because its records carry no digest per tag. The Requirements section now states the corrected model. History of the thread: The owner's objection: a time-only cache misses a checkout or a manual edit, and checking always is annoying; there must also be an explicit check command a git hook can call. Proposed correction, not yet agreed: cache the registry answer per declared reference (`registry/repo:tag` → digest and time) machine-wide; compute the verdict locally whenever the lock changes; go to the network only for references that are unseen or older than the TTL; print a notice only when the finding changes and the terminal is interactive; `grim status --check` is the explicit command and writes the cache. The cache informs notices only and never feeds install or update resolution. The "Update verdict record" and "Update check" requirements above predate this thread and are superseded once it closes.
  - Owner, 2026-11-10: a per-reference cache "seems fine", on condition that the existing search, sync and refresh paths are checked first, because with the new index structure such hits should be cheap to find. Research dispatched: `.agents/research/research_shell_activation_update_cache.md`.
  - Agreed, owner, 2026-11-10: the hook speaks when a finding appears or changes, and once at shell start while it is outstanding; it is silent on every other prompt.
  - Agreed, owner, 2026-11-10: `grim status --check` is the explicit check command. No new command; it gains the side effect of writing the cache, and its output and exit code stay as they are.

## Research

- `.agents/research/research_shell_activation_prior_art.md` (2026-11-10): prior-art web scan across direnv, mise, uv, pixi, fnm, oh-my-zsh, Homebrew, gh and `clap_complete`. Headlines:
  - A prompt hook or a git hook sees a lock changed by `git pull` in place; a directory-change hook does not.
  - Prompt hooks do not fire in the non-interactive shells AI coding agents spawn.
  - No surveyed tool installs from its activation hook on directory entry; uv and pixi sync on a real command instead.
  - mise CVE-2026-35533: a project config file could set trust keys and mark itself trusted; the fix ignores those keys outside global config.
  - oh-my-zsh ships the update modes `auto`, `reminder` and `disabled` and asks by default.
  - `clap_complete` dynamic completion (`CompleteEnv`) is still behind `unstable-dynamic` at 4.6.11.
- `.agents/research/research_shell_activation_recon.md` (2026-11-10): codebase recon over grimoire and `~/dev/ocx-sh/ocx`. Headlines:
  - grim today: static completions for 5 shells, written to a file only; no shell hook, no stat-only staleness check, no throttle store for update checks.
  - Measured: `grim status` 9.1 ms offline (hashes every output, never locks, always exits 0); no-op `grim install` about 230 ms, non-blocking flock, exit 75 for a second writer.
  - grim ignores only `.grimoire/` itself; rendered outputs are gitignored by the user's choice, not by grim.
  - ocx hook: a shell-builtin guard (`-nt` against a stamp file, `$PWD`) spawns nothing when idle; a trip costs 6.0 ms through a stat-fingerprint path.
  - ocx's hook never installs on a lock change: it re-exports environment only and leaves materialization to an explicit command.
  - ocx consent is a per-directory stamp; a fresh clone is inert.
  - ocx update checks run per command, throttled by file mtime; auto-apply covers the ocx binary only, and the update policy cannot be set from the project file.
  - The grim threat model has no ruling on unattended materialization of skills, rules or agents; only hooks and settings carry consent.

- `.agents/research/research_shell_activation_update_cache.md` (2026-11-10): existing sources for an update-check cache. Headlines:
  - A tag cache exists: `$GRIM_HOME/tags/<registry>/<repository>/tags.json` maps tag to digest (`src/oci/tag_cache.rs`). It is written only by a resolving command (lock, update, add, fetch, release), read only offline, has no timestamp, and rejects unknown fields.
  - `grim status --check` resolves with a read-only operation and writes no cache.
  - The index cannot answer "tag to digest": `src/catalog/index_package.rs` reads no tag or version data, and no per-tag digest exists in the index records or the indexer. Only a plain OCI catalog source keeps one representative tag with its digest.
  - Shipped index layout: the `c/root.json` probe, a gzip flat blob and the sync sidecar. The shard planes are indexer-private and the hashed record tree is deferred; grim has no shard lookup.
  - A catalog revalidation costs nothing inside the 3600 s TTL and one small request past it. Loading the cache is a full parse of one file per source.
  - The index lags the registry by one to two hours typically, without bound for an index with no schedule; an index-derived answer would say "up to date" right after a publish.
  - Measured: `--version` 2.7 ms, `--offline status` 9.5 ms, `--offline search` 13.2 ms on a 21-entry cache.

## Related

- ocx shell integration, `~/dev/ocx-sh/ocx` (a sibling checkout outside this repository): `crates/ocx_shell/src/shell/hook.rs`, `crates/ocx_setup/src/rc_block.rs`, and its `shell-integration.mdx` page. Source of the guard, stamp and update-throttle patterns.
- [grimoire-rs/grimoire#53](https://github.com/grimoire-rs/grimoire/issues/53): re-materialize managed context blocks on config change. Adjacent; it excludes file watching and daemon behavior.
- `.agents/plans/plan_catalog_freshness_revalidation.md` C-041: the CLI deliberately has no background refresh.
- `.agents/adr/adr_lock_pin_stability.md`: only `grim update` moves a lock pin.
- `.claude/rules/arch-threat-model.md` (T3 untrusted clone, I1) and `.agents/adr/adr_hook_workspace_consent.md`.
- `src/command/completions.rs`, `src/command/status.rs`, `src/command/install.rs`, `src/main.rs`: the surfaces this feature builds on.
- `src/catalog/update_availability.rs`: the "would `grim update` move this pin?" decision, live and uncached today.
- `src/command/config_keys.rs`: the `options.*` key declarations the new keys join.
- `~/dev/grimoire-rs/vscode/src/checkScheduler.ts` (sibling checkout): the extension already runs debounced `grim status --check` rounds behind its own `grimoire.checkArtifactUpdates` setting.
- `docs/src/content/docs/commands.md`, and the first-party skills under `catalog/`, which need a drift review for any new command.

## Open questions

- [NEEDS CLARIFICATION: Is the any-workspace project check acceptable under the threat model?] Recommended: the ADR's security review rules on it, with "known workspaces only" (a machine-local record under `$GRIM_HOME`) as the fallback — entering a fresh clone makes grim contact hosts the repository names, and whether a project-declared registry can also direct plain-HTTP requests has not been checked.
- [NEEDS CLARIFICATION: How does a PowerShell prompt hook work on 7 and on 5.1?] Recommended: one research pass before the ADR closes, with Windows proof from a CI deep run — the herdr host has no Windows, and the prior-art scan did not cover it.
- [NEEDS CLARIFICATION: Why does a no-op `grim install` cost about 230 ms, and what is the budget?] Recommended: profile first, then set the number from the profile — `grim status` does comparable reading in 9.1 ms.
- [NEEDS CLARIFICATION: Does the update mode get an environment variable?] Recommended: no form that widens it — a repository's own `.envrc` could otherwise switch a cloner's global scope to `auto`.
- [NEEDS CLARIFICATION: Is the new surface labelled Unstable until 1.0?] Recommended: yes — the feature is additive, and the label keeps the hook script and config keys changeable during the freeze.
- [NEEDS CLARIFICATION: How does a long-lived shell notice that the TTL ran out without spawning a process?] Recommended: a shell-variable deadline where the shell has a builtin clock, startup-only elsewhere — macOS still ships bash 3.2.
- [NEEDS CLARIFICATION: Where does `prompt` mode ask, and what does "yes" run?] Recommended: at the next prompt after the cached result arrives, running the same update and install an explicit `grim update` at global scope runs.
- [NEEDS CLARIFICATION: How does the background check coexist with a running `grim install`?] Recommended: the check takes no writer lock — a second writer exits 75 today.
- [NEEDS CLARIFICATION: Does the VS Code extension adopt the verdict record and the new option in this work?] Recommended: no, grim ships the record and the keys first and the extension follows in its own change — the extension's rounds already write the record through `grim status --check` without any extension edit.
- [NEEDS CLARIFICATION: Should the indexer publish a digest per tag so one index request answers every indexed reference?] Recommended: a follow-up issue, not this work — it is a change in the indexer repository, the answer lags the registry by an hour or more, and the per-reference cache is needed anyway for references no index covers.
- [NEEDS CLARIFICATION: What does a background round cost for N references, and does the hook's local verdict need a synchronous fast path?] Recommended: price it in the ADR — a checked round resolves at concurrency 8, and `crates/grim-hook` is the precedent for a std-only hot path.
- Accepted consequence, recorded: a fresh agent worktree stays stale until that clone has the git hooks wired.
- Names for the command family and the config keys are open.

## Verification

- `task verify` passes, and `task docs:check` and `task catalog:verify` pass for the new docs page and the skill drift review.
- Acceptance tests under `test/tests/` drive the emitted script in real bash, zsh and fish on Linux CI. PowerShell 7 and 5.1 are proven on a Windows CI deep run.
- An idle prompt spawns zero processes, shown by a shell trace in a test.
- A prompt inside a project with an unreachable registry returns within the hook budget, which shows the prompt path never waits on the network.
- A changed `grimoire.lock` produces the notice and no file write; a fresh clone produces the same notice.
- A no-op `grim install` meets the budget the profile sets.
- The mode matrix is tested: `none`, `notify`, `prompt`, `auto` at global scope; a project file that sets `prompt` or `auto` is ignored with one warning.
- A `grim status --check` run writes the reference check cache, and a following prompt prints the notice with the registry unreachable.
- Checking out a lock with older pins prints the update notice at the next prompt with the registry unreachable; a second prompt prints nothing.
- `$GRIM_HOME/tags/` is byte-identical before and after a `grim status --check` run, and an offline resolve gives the same digests as before.
- `grim config list --all` lists the new keys with title and description.
- A hook or settings artifact stays unarmed after an unattended apply.
- `grim completions` and `grim status` output is byte-identical before and after (Principle 9).
