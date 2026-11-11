# Research: Shell activation — codebase recon (grimoire and ocx)

## Metadata

**Date:** 2026-11-10
**Domain:** cli
**Triggered by:** /hex-discuss shell-activation
**Expires:** 2027-05-10

Scope: read-only recon of two local trees. Grimoire = `/home/mherwig/.herdr/worktrees/grimoire/uno` (grim 0.15.1, branch `uno`). ocx = `/home/mherwig/dev/ocx-sh/ocx`. All `path:line` cites are relative to the repo named in the section header. No recommendation is made. Measurements used the already-built `target/release/grim` (0.15.1, built 2026-11-10 19:08) and the installed `ocx` 0.6.5; nothing was built.

Side effect to disclose: I ran `grim --offline install` and `grim install` (both no-op, exit 0, every row `unchanged`) in the grimoire worktree to time them. `git status` shows no tracked change.

---

# Part 1 — grimoire

## Q1. Completions

- `grim completions <shell>` is a **static** `clap_complete` script, generated from the live clap tree: `src/command/completions.rs:3-7`, `:38-44` (`clap_complete::generate(args.shell, &mut crate::Cli::command(), "grim", &mut buf)`). Dependency: `Cargo.toml:31` (`clap_complete = "4.6.2"`).
- Shells: `clap_complete::Shell` = bash, elvish, fish, powershell, zsh (`src/command/completions.rs:24-28`; docs `docs/src/content/docs/commands.md:4209-4212`).
- Wiring: `src/main.rs:144` (enum variant), `src/app.rs:163` (dispatched without a `Printable` report).
- Docs state it plainly: "There is no shell auto-detection and no eval-time hook: `<shell>` is a required positional argument" (`docs/src/content/docs/commands.md:4217-4220`). Documented recipes are redirect-to-file only (bash-completion dir, `~/.zfunc/_grim`, fish completions dir) (`commands.md:4222-4226`). Missing/unknown shell is exit 64 (`commands.md:4228-4229`).
- Shipped in 0.11 (`docs/src/content/docs/changelog.md:457-497`); tests `test/tests/test_completions.py`, `test/tests/test_broken_pipe.py`.
- Dynamic/value completion: **not present.** `grep` for `unstable-dynamic`, `CompleteEnv`, `ArgValueCompleter` in `src/` and `Cargo.toml` finds nothing.
- Cost: `grim completions zsh` median 6.4 ms (10 runs, release binary).

## Q2. Staleness detection (lock vs installed outputs)

Command: `grim status` (read-only; header comment `src/command/status.rs:4-8`: "No network and no flock"). Always exits 0 (`commands.md:1613-1618` region, "it is a report, not a gate"; also `status.rs:4-8`).

State derivation (`src/command/status.rs:2866-2891`, `derive_state`), first match wins: `stale` (lock `declaration_hash` != live config, `:2876`) > `missing` (no lock entry / no install record / recorded output absent, `:2879-2887`) > `modified` (an output's on-disk hash != recorded `content_hash`, `:2962`) > `outdated` (outputs intact but `record.source` != lock source, `:2890`) > `installed`. Documented table: `docs/src/content/docs/commands.md:1551-1600` ("Artifact states").

What it reads: `grimoire.toml` (+ global config for `declaration_hash`), `grimoire.lock`, `.grimoire/state.json` (project) or `$GRIM_HOME/state/global.json` (global), and **the bytes of every recorded output file** (`footprint`, `status.rs:~2930-2975`: `out.is_present()` + `out.current_hash()` per output). No network, no lock. Fully offline by construction.

`outputs_pending` (`src/api/status_report.rs:238`, built at `status.rs:485,525,603`, via `pending_outputs_for` `status.rs:2706`): "what would `grim install` write right now that the record does not already account for" — derived from the same seam the installer uses to decide a pass is a no-op (`src/install/expected_outputs.rs:9`; `commands.md:1357-1391`). It reads the record, not the filesystem: a deleted output shows as `state: missing`, not pending (`commands.md:1363-1366`). Pending causes: a client that gained support, or a render-layout move.

Cheapest existing check: `grim --offline status` (or `--format json`) — **9.1 ms median** (10 runs; this repo, 14 rows). There is no stat-only / mtime-only fast path: it hashes output files every run. `status` has no exit-code gate: an unattended "is anything stale?" consumer must parse `state != installed` or `outputs_pending != []` from JSON; there is no `--exit-code`/`--check` meaning "fail when stale" (`--check` means the network update lookup, Q3).

The command that *fails* on lock-vs-config drift is `grim install`: exit 65 with nothing written (`commands.md:807-826`, `src/command/install.rs:91` `require_fresh_lock`); no lock at all is exit 79 (`commands.md:830-833`).

Not found: no mtime/size fingerprint, no "lock changed since last install" stamp, no cache keyed on the lock digest. Install state records per-output hashes (`src/install/install_state.rs`), not a lock hash.

## Q3. Update detection

- `grim status --check` is the "would update move a pin?" probe. It does a catalog load (one round trip, `commands.md:1470-1475`) plus a **live per-artifact tag re-resolution** for every registry-locked row, concurrency 8 (`src/command/status.rs:72-74`), whole sweep deadline 300 s (`status.rs:~84-90`, `CHECK_RUN_DEADLINE`). Decision function: `src/catalog/update_availability.rs:25-55` (`outdated_from_resolve`, `resolve_declared_digest`; `Operation::Query` = read-only, writes nothing to cache, `:44-47`). Skipped entirely under `--offline`/`GRIM_OFFLINE`; `checked: false` + one stderr warning (`commands.md:1488-1494`).
- The re-resolution is "always live" by design, so it is **not cached on disk and has no timestamp** (`commands.md:1496-1500`). Network cost: one manifest resolve per distinct declared reference (rows x references).
- `grim update` = re-resolve floating tags, rewrite lock, re-materialize (`src/command/update.rs:4-21`; `commands.md:909-917`). Mutating; takes the config flock (Q6).
- Catalog cache (browse, not pin freshness): `CATALOG_TTL_SECONDS = 3600` (`src/catalog/registry_catalog.rs:41`); sidecar `$GRIM_HOME/catalog/<h16>.sync.json` (`docs/src/content/docs/configuration.md:1538-1550`); failure backoff 60 s doubling to 3600 s (`src/catalog/sync_state.rs:37-43`); git-index clone TTL one hour (`src/catalog/index_git.rs:25`). Refresh modes `No | Yes | StatsForced` (`sync_state.rs:23-33`).
- Prior decision that bears on a background updater: "`grim search` and `grim status` keep today's semantics exactly: one-shot, no overlay, no auto-update ... A background refresh has no owner in a process about to exit — the TUI gets that and the CLI deliberately does not" (`.agents/plans/plan_catalog_freshness_revalidation.md:226-230`, C-041). `grim search` revalidation is deadline-bounded at 1 s (`:231`, C-044).
- Not found: no "outdated" cache file, no last-checked timestamp for pins, no background/daemon checker, no self-update of the grim binary (grep for `self update`/`self_update` in `src/command` finds none).

## Q4. Gitignore handling and what happens after `git pull`

- grim writes **one** gitignore: `.grimoire/.gitignore` containing `*`, created the first time `.grimoire/` is created, never overwrites a user-edited one (`src/install/install_state.rs:789-796`; doc `configuration.md:1561-1565`, `quickstart.md:133-135`). This keeps `.grimoire/state.json` out of git.
- Rendered outputs (`.claude/`, `.agents/`, etc.): "Grim writes no `.gitignore` for them, so a teammate who clones the repo gets them back by running `grim install`" (`docs/src/content/docs/quickstart.md:137-139`). Whether they are committed or ignored is the user's choice. In this repo they are committed: `git check-ignore -v .claude/skills/hex-core` prints nothing; only `.grimoire/state.json` is ignored (`.grimoire/.gitignore:1`).
- One `.git/info/exclude` writer exists, for hooks only: appends `.claude/settings.local.json` (`src/install/hook_registrar.rs:115-123`, `:2578-2610`; inverse `:2728`). Not a pattern for general outputs. Reason it avoids `.gitignore`: it is a tracked file (`hook_registrar.rs:115`).
- After `git pull` changes `grimoire.lock`: **nothing happens automatically.** `grim status` reports `outdated` (pin differs from recorded source) or `stale` (if `grimoire.toml` also changed); `grim install` re-materializes (`commands.md:1566-1580`, `:766-780`). No post-merge/post-checkout git hook, no docs guidance for one: grep for `post-merge|post-checkout|switch branch` in `docs/src/content/docs` finds only an unrelated marketplace `git pull` line (`guides/use-a-marketplace.md:214`).
- Hazard recorded in the threat model: "`git pull` replacing a gitignored state file" was reproduced on git 2.43, so `.grimoire/state.json` is not trusted storage for arming records (`.claude/rules/arch-threat-model.md:88`). Also `state.json` "can arrive with a `git clone`" (`src/install/install_state.rs:953`).
- Related open issue: [grimoire-rs/grimoire#53](https://github.com/grimoire-rs/grimoire/issues/53) "Re-materialize managed context blocks on config change and update" (title only read).

## Q5. Config layering, non-env-settable options, threat model

Layering:
- Global (`$GRIM_HOME/grimoire.toml`) and project (found by walking up) are **never merged**: each owns its lock; a command runs on exactly one scope (`src/config/scope.rs:12-16`).
- Cross-scope reads are per-key and ad hoc: short-id registry precedence `--registry` > `GRIM_DEFAULT_REGISTRY` > project `default_registry` > global > built-in (`AGENTS.md` env table, `GRIM_DEFAULT_REGISTRY` row; `src/command.rs:263-311`); install client set = project `clients` + global `options.clients` (`commands.md:766-780`); keys without runtime defaults pass through untouched (`src/config/resolved.rs:1-30`).
- `[options]` keys are a closed registry: 12 fixed keys (`src/command/config_keys.rs:3-10,45-60`). Unknown keys are rejected at parse (`configuration.md:51-52`).
- `[options.experimental]` holds `hooks` and `settings` booleans, settable at project **or** global scope via `grim config set` (`configuration.md:89-122`; `src/config/declaration.rs:193-232`). So a repository-carried `grimoire.toml` can carry the feature flag; the machine-local consent record is what stops a clone from acting.

Deliberately NOT settable from env or project config:
- No env variable for the experimental flags or the workspace consent record or its path; `GRIM_ALLOW_HOOKS` / `GRIM_EXPERIMENTAL_HOOKS` removed by owner decision 2026-08-17 and "must not be re-added" (`AGENTS.md:113` row "(no variable)"; `src/config/declaration.rs:170-190`). Reason: env is repo-carriable (`.envrc`, `.mise.toml`, devcontainer `containerEnv`), CWE-426.
- Consent is per workspace, machine-local (`$GRIM_HOME/hooks/consent/<workspace-key>.json`), written only by explicit gesture (`grim hook allow`, `grim add`, an accepted prompt) (`src/hook/trust.rs:8-19`). The only per-invocation answer is the `--trust-workspace` / `--no-trust-workspace` flag pair (aliases `--trust-hooks`), which outranks the record in both directions because a file cannot type a flag (`trust.rs:33-39`; `commands.md:793-799`).
- Arming ladder (`src/hook/trust.rs:25-58`): feature off -> flag denied -> flag granted -> plain-HTTP gate -> **global scope is implicitly granted** ("your own file on your own machine") -> stored consent -> drifted/absent => prompt if interactive, else `NoTtyToAsk` (not armed, exit 0). Global-scope precedent: global config needs no consent (`trust.rs:48-52`).

Threat model on repo-triggered action:
- T3 "an untrusted repository the user clones or opens ... Anything armable that travels in a repository is exposed to this attacker" (`.claude/rules/arch-threat-model.md:46`). I1: nothing armable lives inside a repository; payloads, launcher, dispatch table, consent and settings records live under `$GRIM_HOME` (`arch-threat-model.md:~66-95`). I4: default-deny for anything that executes (`:164`). N1: an insider with commit access is out of scope (`:57`). N4: a user bypassing a gate they were shown is out of scope (`:60`).
- Trust ADR: the trust decision happens once, at the declaration gesture (`.agents/adr/adr_artifact_trust_model.md:45-49`); decision 2 (all kinds equally trusted) carries a scoped hook exception (`:~63-75`), plus a settings exception (`adr_settings_artifact_kind.md`). Direnv precedent is cited as load-bearing for per-directory (path-keyed) consent: `.agents/adr/adr_hook_workspace_consent.md:32-34,81-84,385`.
- Non-interactive contract (C-023): "no TTY never asks" — a non-interactive run declines an unconsented workspace (`src/hook/trust.rs:242`, `:285-296`).
- Nothing in the threat model or ADRs addresses *automatic file materialization* from a lock a `git pull` brought in; "materialize" is today always user-run `grim install`/`update`/`add`. Not found: any ADR text that classifies unattended re-materialization of skills/rules (as opposed to hooks/settings) as a T3 exposure or as acceptable.

## Q6. Unattended-run behaviour

- Exit codes (sysexits-style): 0 ok, 1 failure, 64 usage, 65 data (stale lock, refused modified/untracked output), 69 unavailable, 74 io, 75 tempfail, 77 no-permission, 78 config, 79 not-found (no lock), 80 auth, 81 offline-blocked (`src/cli/exit_code.rs:22-55`; install doc comment `src/command/install.rs:57-61`).
- Interactivity: "interactive is defined as stdin AND stderr both being terminals" (`src/hook/trust.rs:285-296`); prompt goes to stderr. Only hook/settings consent prompts; install of skills/rules never prompts. Progress bar only on an interactive stderr (`src/command/install.rs:~110-115`, `--progress auto`).
- What install refuses: stale lock (65, nothing written); destination exists without an install record (65, names path; `--force`); locally modified output (65, `--force`); no clients detected and only non-skill kinds (78) (`commands.md:782-826`). Refusal still emits a report for `update` (`update.rs:20-24`).
- Concurrency: **yes, a file lock.** `grim install` takes `ConfigFileLock::try_acquire(<grimoire.toml>)` — exclusive advisory flock on a `<grimoire.toml>.lock` sidecar, **non-blocking** (`src/command/install.rs:80-83`; `src/lock/file_lock.rs:27-43`; mechanism `src/lock/advisory_lock.rs:3-30`). A second writer gets `LockErrorKind::Locked` -> exit 75 (`TempFail`), reason `Locked`, the only retryable reason (`src/error.rs:504-511`, `:196-199`). Readers (`status`) never lock (`file_lock.rs`, test `reader_is_unaffected_by_held_lock`). Catalog refresh uses the same mechanism per cache file (`file_lock.rs:3-9`).
- Measured (release binary, this repo, 12 artifacts, warm state, 10 runs): `grim --version` 2.2 ms; `grim completions zsh` 6.4 ms; `grim --offline status` 9.1 ms; no-op `grim --offline install` **237 ms** (max 244); no-op online `grim install` 230 ms, exit 0, all `unchanged`; `GRIM_LOG=debug` on that run printed zero lines matching http/manifest/resolve/fetch (absence of log lines, not an strace proof). No-op install is ~25x `status`; the code reads/hashes outputs through the integrity gate and persists state (`install_and_persist`, `src/command/install.rs:120`). I did not profile why.
- Install does not resolve; it needs a fresh lock and fetches only blobs not already in the content store (`src/command/install.rs:3-9`). A cold store needs the network (exit 81 offline, 69 unreachable; `install.rs:56-61`).

## Q7. Startup cost

- `main`: argv pre-scan for `--color`, clap parse, `init_tracing`, then `tokio::runtime::Runtime::new()` (multi-thread, default workers) and `block_on(app::run)` for **every** subcommand including `completions` (`src/main.rs:158-205`; `Cargo.toml:35` tokio `full`). No current-thread fast path.
- `app::run`: `Context::new` is env reads only, plus `Telemetry::from_env` (`src/app.rs:44`, `src/context.rs:187-206`). Telemetry is opt-in (`GRIM_TELEMETRY`, `OTEL_*`); unarmed it returns `Self::default()` (no I/O) (`src/telemetry.rs:93-127`); `telemetry().start` is a no-op when unarmed (`:135-140`). Config load is lazy (`context.rs:62-72`, global-config memo behind `OnceLock`).
- Measured floor: `--version` 2.2 ms, a full `status` 9.1 ms.
- Existing fast-path precedent: `crates/grim-hook` is a synchronous `std` binary (no Tokio/clap/tracing) that reads a pre-resolved dispatch table at `$GRIM_HOME/hooks/`, exists because a client execs it on every tool call (`.claude/rules/product-tech-strategy.md:14-23`; `crates/grim-hook/src/main.rs:10-18,24`). Its design rationale is `.agents/research/research_hooks_hotpath_cost.md:22-30` (precompute at install time; no config walk, no lock/state parse on the hot path; `:144-200` on Tokio start, serde-parse cost, locks).
- Not found: no existing "cheap staleness" entry point of that kind for lock-vs-outputs; `grim hook run` serves only hook events.

## Q8. Prior records on shell activation / auto-update / completions

Searched `.agents/{adr,plans,specs,discussions,research,memory}` and `docs/`.
- `.agents/discussions/shell-activation.md` (this session's artifact, untracked; contents header only at read time).
- No ADR, plan, spec or research file on shell activation, a prompt hook, or an auto-update feature. Not found.
- Completions: only the shipped command (changelog above) and passing mentions (`.agents/research/research_hooks_trampoline.md:161` and `adr_hooks_support.md:1391` cite `grim schema`/`grim completions` as the "payload-plain, wired directly in app.rs" pattern).
- Prior art cited in ADRs: direnv path-keyed trust (`adr_hook_workspace_consent.md:32-34`); `lefthook, starship, direnv, mise` publish no hot-path latency number (`adr_hooks_support.md:1718`; `research_hooks_trampoline.md:193`). The word "auto-update" in `.agents/` refers to *vendor harness* settings (Copilot `autoUpdate`, opencode `autoupdate`, Claude marketplaces), not grim (`adr_settings_artifact_kind.md:1114,1134`; `adr_harness_marketplace_export.md:679`).
- GitHub (gh search, titles only): [grimoire-rs/grimoire#53](https://github.com/grimoire-rs/grimoire/issues/53) re-materialize managed context blocks on config change; [#168](https://github.com/grimoire-rs/grimoire/issues/168) grim-free hook runtime for exported plugins; [#252](https://github.com/grimoire-rs/grimoire/issues/252) exec-form hooks. No issue on shell hooks, completions wiring or auto-update found by the search `shell OR completions OR activation OR hook-env`.

---

# Part 2 — ocx

ocx has a full shell-integration stack. Summary: login-time activation line + per-prompt reconcile hook + consent stamps + throttled background update notices. Its hook manages **PATH/env**, not files.

## A. Commands, flags, shells, emitted code

- `ocx self setup`: writes thin per-shell shims into `$OCX_HOME` (`env.sh`, `env.fish`, `env.nu`, `env.elv`, `env.ps1`) and a block-marked source line into profiles (`~/.profile`, `~/.bashrc`, `~/.zprofile`, `~/.zshrc`, ...); also registers a session PATH; idempotent, diff-gated, `--dry-run`, `--no-modify-path`, `--profile/--no-profile`, `--hook/--no-hook`, `--completion/--no-completion` (persist to `config.toml`) (`website/src/content/docs/reference/command-line/self.md:52-81`; `website/src/content/docs/explanation/shell-integration.mdx:38-48`; code `crates/ocx_cli/src/command/self_group/setup.rs`, `crates/ocx_setup/src/shims.rs:4,14`, `crates/ocx_setup/src/rc_block.rs:10-12`). Failure to edit a hand-edited block: exit 81 unless `--force` (`self.md:68`).
- Profile block (POSIX), `crates/ocx_setup/src/rc_block.rs:409-411`:
  `if [ -f "${OCX_HOME:-$HOME/.ocx}/env.sh" ]; then . "${OCX_HOME:-$HOME/.ocx}/env.sh"; fi`
- `ocx self activate [--shell=...] [--hook|--no-hook] [--completion|--no-completion]`: prints up to three blocks — completion script, PATH prepends, global-env eval — and registers the hook; shells ash/ksh/dash(sh)/bash/elvish/fish/batch/powershell/zsh/nushell (`self.md:14-50`). Hidden `--reconcile` is the per-prompt entry point (`crates/ocx_cli/src/command/self_group/activate.rs:76-77`).
- `ocx shell completion [--shell S] [--if-enabled]`: static `clap_complete` for bash/elvish/fish/powershell/zsh; detects the parent shell when `--shell` omitted (`crates/ocx_cli/src/command/shell_completion.rs:10-95`; `shell.md` reference).
- `ocx shell allow|revoke|state`: grant/withdraw consent, report integration state/inert reason (`website/src/content/docs/reference/command-line/shell.md`).
- `ocx direnv export`/`init`: stateless export mode for direnv users (`crates/ocx_cli/src/command/direnv_export.rs`).

Verbatim hook emitted for bash (`OCX_HOME=/tmp/x ocx self activate --shell=bash --hook --no-completion`, ocx 0.6.5; abbreviated only where the guard's long watch-path list repeats; source: `crates/ocx_shell/src/shell/hook.rs:262-302`):

```sh
if ! typeset -f __ocx_prompt_hook >/dev/null 2>&1; then
__ocx_stamp="$(command mktemp -t ocx-env-stamp.XXXXXXXX 2>/dev/null)" || __ocx_stamp=''
if [ -n "${__ocx_stamp-}" ] && [ -z "$(trap -p EXIT 2>/dev/null)" ]; then trap 'command rm -f "${__ocx_stamp-}" 2>/dev/null' EXIT; fi
__ocx_prompt_hook() {
local __ocx_status=$?
if [ -x '<bin>/ocx' ] && { [ -z "${__OCX_ENV_STATE-}" ] || [ "${__ocx_pwd-}" != "$PWD" ] || [ "${__ocx_yield-}" != "${DIRENV_DIR-}|${MISE_SHELL-}|${__MISE_ORIG_PATH-}" ] || [ "${__ocx_grant-}" != "${OCX_CONSENT_PATHS-}|${OCX_CONSENT_NAMESPACES-}" ] || [ -z "${__ocx_stamp-}" ] || [ ! -f "${__ocx_stamp-}" ] || [ '<home>/ocx.toml' -nt "${__ocx_stamp-}" ] || [ '<home>/ocx.lock' -nt "${__ocx_stamp-}" ] || ...more '-nt' tests... ; }; then
eval "$('<bin>/ocx' --offline self activate --reconcile --shell=bash 2>/dev/null)" || true
fi
return $__ocx_status
}
fi
case "$(declare -p PROMPT_COMMAND 2>/dev/null)" in
*__ocx_prompt_hook*) : ;;
'declare -a'*) PROMPT_COMMAND+=(__ocx_prompt_hook) ;;
*)
...PROMPT_COMMAND="${__ocx_pc:+$__ocx_pc;}__ocx_prompt_hook" ...
esac
```

The `ocx` function wrapper also re-runs the hook after every `ocx` command (`hook.rs:304-320`).

## B. Trigger model and cost control

- Triggers: **prompt hook** (bash `PROMPT_COMMAND` append, zsh `precmd_functions` append, fish `--on-event fish_prompt`, PowerShell `prompt` wrapper, elvish `$edit:before-readline`); `cd` is covered because the guard compares `$PWD` to the last-reconciled `$__ocx_pwd` (`hook.rs:285`; table `shell-integration.mdx:61-73`). nushell uses `env_change.PWD` (global tier only; project reconcile added later, issue #682 closed 2026-11-08). ash/dash/ksh/Batch: **no hook point**, shell-start activation only (`shell-integration.mdx:61-73`; `hook.rs:93-97`).
- Cost layers, cheapest first:
  1. **Shell-builtin guard**, no exec: compares `$__OCX_ENV_STATE` empty, `$PWD`, direnv/mise yield sentinels, consent-grant env vars, and `[ file -nt "$stamp" ]` for each watched file (project `ocx.toml` + `ocx.lock`, global `ocx.toml`/`ocx.lock`, managed-config snapshot, the ocx binary, config tiers, the project's consent stamp) (`hook.rs:274-293`; watch set `crates/ocx_shell/src/shell/reconcile/fingerprint.rs:84-133`). An unchanged prompt execs nothing. Stamp is a `mktemp` file per shell, cleaned on `EXIT` (`hook.rs:14-23`, `POSIX_STAMP_CLEANUP`).
  2. If the guard trips: exec `ocx --offline self activate --reconcile`. `--offline` "spar[es] a `reqwest` client per index source (22 vs 58 ms)" (`hook.rs:29-33`). The reconcile entry runs **before** `Context::try_init`; it decodes the env-var ledger, walks to the project, folds a stat fingerprint (sha256 over path/size/mtime of the watch set + consent env + ocx version) and, if the cached verdict is Inert/NoProject and the fingerprint is equal, emits only a checkpoint and exits (`activate.rs:178-243`; `fingerprint.rs:7-45`; `crates/ocx_package_manager/src/activation.rs:387`). Only negative verdicts are cached: caching `Activate` would make the untrusted env var a consent input (`activation.rs:386`, `activate.rs:225`).
  3. Full compose with a `LocalOnly`/offline manager ("a prompt must never block on the network", `activation.rs:424-425`).
- State carrier: one private env var `__OCX_ENV_STATE`, a base64 ledger (applied entries per scope, fingerprint, verdict), capped at 16 KiB; repair gesture `unset __OCX_ENV_STATE` (`shell-integration.mdx:53-59`; `troubleshooting/shell.mdx:~196-205`).
- Measured here: `ocx --offline self activate --reconcile --shell=bash` median **6.0 ms** (10 runs, empty `OCX_HOME`, includes process spawn; `ocx version` 4.3 ms). Their own gate figure: reconcile over a 8-tool project is 2.1-2.9 ms above a bare-exec floor; `RECONCILE_BUDGET_MS` was raised 3.0 -> 6.1 after the global tier was included ([ocx-sh/ocx#360](https://github.com/ocx-sh/ocx/issues/360), [#362](https://github.com/ocx-sh/ocx/issues/362); gate `test/bench/shell_latency.py`, named in [#359](https://github.com/ocx-sh/ocx/issues/359)).
- Hook toggles are read **once at shell start, never per prompt** ("the reconciler's budget forbids reading configuration or environment toggles on every prompt", `website/src/content/docs/troubleshooting/shell.mdx:28`). Five-rung ladder: `--hook/--no-hook` flag > `OCX_NO_HOOK` > `[shell] hook` > interactivity (`troubleshooting/shell.mdx:~14-22`; `crates/ocx_cli/src/options/hook.rs:57`).
- Interactivity is decided once in the shim from `$-` (`*i*`) and passed as `--interactive/--no-interactive` (`~/.ocx/env.sh` shim body; source `crates/ocx_setup/src/shims.rs:14`), because the binary cannot tell `ssh -t host 'bash -lc ...'` from a real prompt.

## C. What the hook does on a changed project file or lock

- It **re-derives the environment and re-exports PATH/env** (add/remove/switch entries, typed per-element diff) and prints a one-line summary like `ocx: +PATH` (`shell-integration.mdx:25-33`). It does **not** install, fetch or prune.
- In `env` mode a package missing from the store is not installed by the prompt path; materialization "is `ocx pull`'s job" (`crates/ocx_package_manager/src/activation.rs:3403`, test "an unmaterialised package is not this path's problem"). In `bin` mode a missing/mismatched render yields the notice `ocx: <dir>: its toolchain has not been rendered for this lock; run \`ocx pull\` here` (`activation.rs:578`; `shell-integration.mdx:122-134`). The stale window "between a lock change and the next `ocx pull` is **designed** ... The prompt path never prunes, and none may be added ... a whole-directory delete inside a repository-writable tree, running before every command you type" (`shell-integration.mdx:134`).
- A `git checkout` that swaps `ocx.lock` is noticed at the next prompt in bash/zsh/fish/PowerShell via mtime guard; elvish (no mtime) waits for the next `cd` or `ocx` command (`troubleshooting/shell.mdx:~209-211`).
- A narrow cheap heal exists: a missing default-group link is created at prompt time, "the commonest post-`git pull` state" (`activation.rs:3308`, test `c062_...`).
- Auto-install/auto-update: not by the hook. The only automatic mutation anywhere is `[update] self = "apply"` (installs a newer **ocx binary**) after a user's command, see F. `[update] toolchain = "apply"` is read as `notify`: "only `ocx update` moves a pin" (`crates/ocx_config/src/update.rs:~55-60`; `guides/update-ocx.mdx:~100`). `ocx direnv export` and `ocx env`/`exec` **do** install a missing package unless `--no-pull`/offline (`guides/project-toolchain.mdx:373`; `guides/run-project-tools.mdx:121`), but those are explicit commands, not the prompt hook.

## D. Trust model

- Per-directory consent before a project's files act. A fresh clone is inert (`shell-integration.mdx:17-21`). Only `stat` calls and `ocx.lock`'s source list are read pre-consent; `ocx.toml` is not parsed until consent says yes (`shell-integration.mdx:19`; `activate.rs:~190`).
- Three independent grants (`shell-integration.mdx:136-146`): (1) **consent stamp**, written by `ocx shell allow`, by `ocx init`, and as a side effect of any mutating command run in the directory (`add/remove/lock/update/upgrade/pull/exec`); records the source set of the lock; growing it past that set re-confirms; (2) **path grant** `[shell.consent] paths` / `OCX_CONSENT_PATHS` (devcontainer/CI image case; `/*` suffix); (3) **namespace grant** `[shell.consent] namespaces` / `OCX_CONSENT_NAMESPACES`, decided against the store's own record of what was fetched, not the lock's text. The global toolchain is always consented (the user's own file).
- Storage: stamp file at `$OCX_HOME/projects/<key>/consent.json`-style path via `StateStore::consent_stamp_file` (`crates/ocx_store/src/file_structure/state_store.rs:243`, project root `:248`); the stamp is in the hook's watch set so a grant in another terminal invalidates the cached verdict (`fingerprint.rs:~125-130`). `OCX_NO_CONSENT` lets automation decline the side-effect stamp (`shell-integration.mdx:148-150`).
- `[shell]` is never read from `ocx.toml` (`troubleshooting/shell.mdx:~46`); `[update]` is refused in `ocx.toml` (exit 78) and dropped from the managed tier (`crates/ocx_config/src/update.rs:1-6`; `guides/update-ocx.mdx:115-119`) — "neither a fleet publisher nor a cloned repository can switch on binary replacement".
- Accepted residual: within a consented namespace whoever can publish gets code on PATH; mitigation is opt-in `[[trust.policy]]` signing (`shell-integration.mdx:154-165`). Rationale for not re-confirming on every `git pull`: it "would train users to click through prompts".
- Cautionary precedent cited by ocx: mise GHSA-436v-8fw5-4mj8 (`shell-integration.mdx:19`).

## E. Completions

- **Static** `clap_complete`, not dynamic. `render_completion_script` wraps zsh output with a self-loading `compinit` prefix so sourcing from `.zprofile` works (`crates/ocx_cli/src/command/shell_completion.rs:89-98`).
- Wiring: the shim calls `ocx self activate --no-completion` for PATH/env, then a **separate** `ocx shell completion --shell=<detected> --if-enabled <interactivity>` and re-injects it on **every interactive source**, because zsh `compinit` rebuilds its table and oh-my-zsh runs it after `.zprofile` (comment in the installed `~/.ocx/env.sh`; source `crates/ocx_setup/src/shims.rs:14`). Policy ladder: `--completion/--no-completion` > `OCX_NO_COMPLETION` > `[shell] completions` > interactivity (`troubleshooting/shell.mdx:~34-40`; `crates/ocx_cli/src/options/completion.rs:22`).
- Manual forms documented: `source <(ocx shell completion --shell bash)`; zsh to `${fpath[1]}/_ocx` (`troubleshooting/shell.mdx:~55-62`).
- Dynamic completion of identifiers is an **open, blocked** issue: [ocx-sh/ocx#262](https://github.com/ocx-sh/ocx/issues/262) — needs clap's `unstable-dynamic` (clap-rs/clap#3166); adopting it "means re-cutting `ocx completion` and `ocx self activate` output, which is CLI-surface contract".

## F. Update checks

- Yes, but **per command, not per prompt**, and not from the hook. Before a user's command, two background checks run (ocx binary; locked tool tags drifted) once per interval, only if stderr is a TTY, not CI, not offline, `OCX_NO_UPDATE_CHECK` unset, not `--frozen` (`crates/ocx_cli/src/app.rs:160-180`; gate `crates/ocx_cli/src/app/background_check.rs:40-55`; checks `app/update_check.rs:21-60`, `app/toolchain_drift_check.rs:13-60`; command skip-set `app.rs:347-365`: skipped for `version`, `about`, `shell`, `self`, `config`, `exec`, `env`, `direnv`, and (drift only) `update/upgrade/lock/add/remove/init`).
- The check "waits for one registry round trip" before the command (`guides/update-ocx.mdx:77`); toolchain drift capped by `DRIFT_DEADLINE` (5 s total, `update-ocx.mdx:~111`). Failures never fail the command.
- Throttle/cache: files under `$OCX_HOME/state/update-check/` — one zero-byte file for ocx, one marker per toolchain; **mtime is the data** (`update-ocx.mdx:123`). Default interval 1 d (`ocx_config` `DEFAULT_INTERVAL`, `crates/ocx_config/src/update.rs:12`); `0` = every eligible command.
- Config `[update]`: `self = notify|apply|manual`, `toolchain = notify|manual` (`apply` = notify), `interval`; env overrides `OCX_SELF_UPDATE`, `OCX_TOOLCHAIN_UPDATE`, `OCX_UPDATE_CHECK_INTERVAL`; read from local `config.toml` tiers only (`update-ocx.mdx:93-121`; `crates/ocx_config/src/update.rs:1-60`). The two update vars are not forwarded to child ocx processes and `ocx.toml` cannot set them (`update-ocx.mdx:125`).
- `self = apply` runs after the command, on success and failure, "without changing its exit code", via the same path (and trust policy) as `ocx self update` (`app.rs:183-190`; `update-ocx.mdx:105-108`).
- Notice format: one stderr block "ocx: updates available ..." (`update-ocx.mdx:~79-89`; `app/update_notice.rs`).
- Toolchain drift reads lock platform digests vs registry; probes a project only if consented; silent on no lock, stale lock, registry error or >5 s (`update-ocx.mdx:~111-113`).

## G. Known problems, regrets, limits (cited, not judged)

- Per-prompt latency is a recurring gate problem: "C-044's per-prompt budget is nominally met and effectively undecidable on a slower runner" — real 8-tool project costs 2.1-2.9 ms over bare exec ([#360](https://github.com/ocx-sh/ocx/issues/360)); "The global tier does a store write per tool on every prompt" (`manager.find` upserts refs; ~0.36 ms/tool, 7 tools = +2.5 ms; [#362](https://github.com/ocx-sh/ocx/issues/362)); the latency fault injector exists for the gate (`hook.rs:~130-140`). Changelog: "Size the latency injection against the runner, not the dev box", "Stop an abstaining latency run from reading as PASSED" (`CHANGELOG.md:239,254`).
- Alternative never built: hookless shims mode, [#359](https://github.com/ocx-sh/ocx/issues/359) (reopen if reconcile stops being sub-ms or a hook-less shell becomes a target).
- Shell coverage gaps: ash/dash/ksh/Batch no per-prompt hook; elvish has no mtime term; nushell had token-less hooks until [#682](https://github.com/ocx-sh/ocx/issues/682) (closed); elvish pre-upgrade closure reprinting the stand-down notice, [#683](https://github.com/ocx-sh/ocx/issues/683) (closed) (`shell-integration.mdx:61-73`; `troubleshooting/shell.mdx:209`).
- Windows: "The shell edge-case module can't run on Windows because of our own harness assumptions" — six EC rows manual-only on Windows ([#358](https://github.com/ocx-sh/ocx/issues/358)); `EC-HOOK-009` PowerShell 5.1 prompt-wrap semantics, `cmd.exe` PATH algebra, 32767-char env block are genuine Windows-only limits.
- Staleness ceilings: a project file appearing in an *ancestor* of `$PWD` waits for the next `cd` (`hook.rs:~24-28`); mtime-granularity races need `unset __OCX_ENV_STATE` (`troubleshooting/shell.mdx:195-205`).
- Ordering hazards: powerlevel10k instant-prompt — source the activation line after the preamble (`troubleshooting/shell.mdx:187-190`); co-existence with direnv/mise handled by "yield" sentinels (`hook.rs:36-45`, `crates/ocx_shell/src/shell/coexistence.rs`); bash `PROMPT_COMMAND` string-vs-array and trailing-`;` handling inside the emitted code (see Q-A quote); `rbash` no-ops (`hook.rs:~83-90`).
- Env size: 16 KiB carrier cap; OS argv+envp limit is not accounted (`troubleshooting/shell.mdx:142`).
- Stale launchers after upgrade: [#679](https://github.com/ocx-sh/ocx/issues/679) (open, priority high). `ocx self uninstall` does not exist; no CLI to remove the hook/profile block ([#413](https://github.com/ocx-sh/ocx/issues/413), open; manual removal documented).
- Group selection per prompt not possible: [#363](https://github.com/ocx-sh/ocx/issues/363) (open). Consent stamp not covering `[env]`: [#364](https://github.com/ocx-sh/ocx/issues/364) (open).
- Design guardrails recorded as skill/rule: `.opencode/skills/shell-integration/SKILL.md` (multi-shell emitters, append-safe hooks, rc editing, completions by read-back; "What agents get wrong") with a harness kit `.opencode/skills/shell-integration/references/hooks/hook-harness.py` and rule `.opencode/rules/shell-quality/{emit,zsh}.md`.
- No TODO/FIXME debt in the hook emitter files (one stale "TODO 'First' bug" regression reference, `activate.rs:1143`).

## H. Structural differences affecting transferability

- What is managed: ocx composes **PATH and env vars** from content-addressed packages in `$OCX_HOME`; the hook output is shell code that mutates the live shell environment (reversible via the ledger). grim **copies files into agent-config directories** (`.claude/skills/...`, `.agents/skills`, MCP/settings JSON merges) and records per-output hashes; there is no environment to reconcile and no shell-process state to reverse (`docs/src/content/docs/commands.md:766-780`; `src/install/install_state.rs`).
- Where the work lands: ocx's reconcile result is ephemeral, in-process shell state (rebuildable on any prompt); a grim re-materialize is persistent filesystem writes into a (possibly git-tracked) repository tree. ocx explicitly forbids the prompt path from writing/pruning inside repo-writable trees (`shell-integration.mdx:134`), the same class of write grim's `install` performs.
- Cost of the "is anything stale" test: ocx's is a shell-builtin `-nt` against ~9 files then a stat fingerprint; grim's cheapest test is 9 ms of output hashing (`status`) with no mtime fast path, and its lock-vs-state comparison needs `state.json`, which the threat model treats as untrusted-in-repo storage when it is gitignored-but-pullable (`arch-threat-model.md:88`).
- Consent surface: ocx gates a project's PATH contribution by per-directory stamp/grants; grim gates only hooks and settings (and treats `grim install` itself as the user's explicit act). Skills/rules/agents have no consent record today (`adr_artifact_trust_model.md` decision 2: all kinds equally trusted; hook/settings exceptions).
- Scope model: ocx has one project slot + global tier reconciled each prompt; grim scopes are never merged and each owns a lock (`scope.rs:12-16`), with global scope installing into `~/.claude` etc. and project into the workspace.
- Concurrency: grim `install`/`update` hold a non-blocking flock (second writer exits 75); ocx's hook is read-mostly. A prompt hook that spawned `grim install` would race with itself across terminals (a retryable 75, `src/error.rs:504-511`), not queue.
- Runtime: ocx runs Tokio too (`#[tokio::test]` in activation, `tokio::task::spawn_blocking` in `activate.rs:230`) but its reconcile exits **before** `Context::try_init` on the stat-only path (`activate.rs:~165`, doc comment `:84-85`); grim's `main` builds a multi-thread Tokio runtime before any dispatch (`src/main.rs:189`).
- Completions are equivalent today (static `clap_complete`, five shells). ocx adds interactivity-aware injection and zsh `compinit` autoload; grim has the generator only.
- Update checks: ocx's are throttled, per-command, TTY/CI-gated notices with an on-disk mtime throttle; grim has no equivalent throttle store and its only online probe (`status --check`) is un-cached by design (`commands.md:1496-1500`).

---

## What was looked for and not found

- grim: dynamic completions; an eval-time hook or auto-detecting `completions`; any shell-hook/prompt-hook code; any "last-installed lock digest" or mtime fingerprint; an exit-code mode of `status` meaning "stale"; a throttle/last-checked store for pin updates; grim self-update; a post-merge/post-checkout recommendation in docs; any ADR/plan/research on shell activation; a lock-file or `.gitignore` writer for rendered outputs.
- ocx: an auto-install in the prompt hook (explicitly forbidden); per-prompt network; per-prompt config reads; `ocx self uninstall`; dynamic completion; a Windows-tested edge-case module.
