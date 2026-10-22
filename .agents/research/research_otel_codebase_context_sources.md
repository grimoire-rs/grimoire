# Research: OTEL telemetry — where grim can source invocation context

<!--
Codebase recon lane (read-only, local, no web). Evidence only: no option is recommended.
Feeds the open "Context collection" and "Signals: events and metrics" threads in
.agents/discussions/otel-usage-telemetry.md. Builds on research_otel_codebase_recon.md
(tracing init, exit seam, OBS gaps) and does not repeat it.
-->

## Metadata

**Date:** 2026-10-22
**Domain:** observability, security
**Triggered by:** the owner's question "what with events? a list of roughly planned metrics? how is context collected, will we have auto-detection of a git repo?" and the discussion's open "Context collection" (resource / dispatch / late layers) and "Signals" threads. Tree state: HEAD `7b21abce`.
**Expires:** 2027-04-22 (re-verify file:line anchors after any change to `src/main.rs`, `src/app.rs`, `src/context.rs`, `src/oci/git_provenance.rs`)

## Direct Answer

For each candidate attribute: where the value is already computed, when it becomes known, and what it costs. "Dispatch" means after `parse_cli` in `main()`, before `app::run`. "Late" means inside a command, after `Context::new` and scope resolution.

| Attribute | Already computed at | Known when | Cost |
|---|---|---|---|
| command name | `Command` enum, `src/main.rs:91-146` (25 variants); `app::run` match `src/app.rs:70`. No `Command::name()` accessor exists (grep empty). Precedent for matching the parsed command at dispatch: `runtime_flavor(cli.command.as_ref())` `src/main.rs:184` | dispatch | pure (argv only) |
| subcommand | nested enums: `HookCommand` `src/command/hook.rs:83`, `ConfigCommand` `config.rs:48`, `RegistryCommand` `config.rs:93`, `ExportCommand` `export.rs:38` | dispatch | pure |
| output format, offline, color | `GlobalOptions` `src/cli/options.rs:60`; format captured `main.rs:179`; offline via `ctx.offline()` after `Context::new` (`context.rs:167`, reads `GRIM_OFFLINE`) | dispatch (flag); late (env) | pure |
| requested scope (global vs project) | `--global` flag in `GlobalOptions`; most commands call `scope_resolution::resolve(ctx, ctx.global(), ctx.config())` (install.rs:73, remove.rs:57, context.rs:38, status.rs:117, hook/list.rs:117, config.rs many) | dispatch (flag only); resolved scope late | flag: pure. Resolution: config file read + parse (`scope_resolution.rs`) |
| resolved scope | `ConfigScope` Display "global"/"project" `src/config/scope.rs:22-28` | late | config walk-up stat per ancestor + file read |
| CI present | only `CiEnv::from_env()` `src/catalog/forge.rs:75-92` (`GITHUB_ACTIONS`, `GITLAB_CI`, truthy = `1\|true\|yes\|on`). Called only at `rate.rs:754`, `publish.rs:1405` | dispatch possible (env) | pure env reads |
| CI org / server host | same `CiEnv`: `GITHUB_REPOSITORY_OWNER`, `GITHUB_SERVER_URL`, `GITHUB_API_URL`, `CI_SERVER_HOST`, `CI_API_V4_URL`, `CI_PROJECT_NAMESPACE` | dispatch possible (env) | pure env reads |
| git repo present | no general helper. Closest: `project_init_dir` `src/command/tui.rs:190-196` (ancestor `.git` exists check, stops before `$HOME`); `git_info_dir` `src/install/hook_registrar.rs:1859-1876` | late (needs cwd walk) | `stat` per ancestor; no spawn |
| git origin URL | only `derive` in `src/oci/git_provenance.rs:182` (private), reached via `resolve` `:238`; used by build/release/publish (`command/build.rs:372`, `release.rs:125`) | late, build/release/publish only today | `git config --get remote.origin.url` is one spawn (`:199`); `derive` spawns 5 |
| normalized origin | `normalize_remote_url` `git_provenance.rs:363` (pure, `pub`) | pure on any string | pure |
| workspace root | `walk_up_for_config` `src/config/project_config.rs:149-191`; `workspace_of` / `workspace_from` `scope_resolution.rs:~140-185` | late | stat per ancestor + config read |
| detected AI clients | `detect_clients` `src/install/target.rs:308` (raw, deterministic `ClientTarget::ALL` order, 19 clients `client_target.rs:265`); `detect_clients_or_all` `:327`; `InstallTarget::parse` `target.rs:~120` | late, inside install (`install.rs:94`), update (`update.rs:145`), add (`add.rs:495`), context (`context.rs:46`), status (`status.rs:159,180`), hook list, tui (`tui.rs:266`), init (`init.rs:80`) | `stat`s (`vendor.rs:1309-1312`: "Pure existence checks; no I/O beyond stat"); global-scope Claude detection also reads settings JSON once per process (`vendor_claude.rs:541`, OnceLock) |
| per-artifact outcome | `ArtifactInstall` `installer.rs:108` -> `finish()` `command/install.rs:495-542`; `UpdateEntry` `update.rs:612-659`; `AddReport` `add.rs:225-237,433-439`; remove / uninstall / lock / status reports | late | in memory already |
| exit code / error kind | `ExitCode::slug()` `src/cli/exit_code.rs:64-79`; `ErrorReason` closed 7-variant enum `src/error.rs:113-141`; `classify` `error.rs:200`; `error_document` `main.rs:363-385` | after `block_on` (`main.rs:199`, Err arm `:223-224`) | pure |
| interactive / TTY | four predicates (see Key Finding 12) | dispatch possible | `isatty` syscalls |
| OS / arch / version | version `env!("CARGO_PKG_VERSION")`; no `std::env::consts` use in `src/` (grep empty) | compile time | free |
| run id | none (OBS-23 unmet). `uuid 1.24.0` transitive via `rmcp 3.3.0` (features `v4`,`rng`,`std`); `getrandom 0.3` and `hex 0.4` are direct deps (`Cargo.toml:44-45`) | dispatch | one CSPRNG read |
| persistent install / machine id | none exists (Key Finding 10) | n/a | n/a |
| host agent (Claude Code, Codex, ...) | none; grep for `CLAUDECODE`, `TERM_PROGRAM` empty | n/a | n/a |

## Technology Landscape

### Established (already in the tree and reusable)

| Tool/Pattern | Status | Notes |
|---|---|---|
| `git` subprocess helper | Mature | `git_provenance.rs:319-341` (`Command::new("git").arg("-C")`), also `hook_registrar.rs:1901`, `catalog/index_announce.rs:637`, `catalog/index_source.rs:801` |
| `normalize_remote_url` | Mature, has gaps | strips userinfo, port, `.git`; keeps `?` and `#` (Key Finding 5) |
| `CiEnv::from_env` | Mature but narrow | two vendors, and it loads tokens into the struct (Key Finding 3) |
| `grim context` report | Mature | `command/context.rs`, `api/context_report.rs` already serializes version, scope, paths, grim_home, offline + source, clients, registries |
| `hook_dispatch` root key | Mature, feature-bound | `$GRIM_HOME/hooks/root-key`, 32 bytes CSPRNG, `hook_dispatch.rs:526-571` |

### Emerging (not in the tree)

| Tool/Pattern | Signal | Worth Watching Because |
|---|---|---|
| Direct `uuid` dependency | already compiled via `rmcp` | declaring it directly adds no new crate (inferred, not built) |
| No-spawn `.git/config` reader | no crate in tree | a spawn costs about 1.2 ms vs about 5 us for a read (WSL2 only) |

### Declining / absent

| Tool/Pattern | Signal | Note |
|---|---|---|
| Machine-id crates (`machine-uid`, `hostname`, `whoami`, `sysinfo`, `os_info`, `dirs`, `ulid`) | none in `Cargo.lock` | only `fastrand`, `getrandom` x3, `rand` x2, `uuid`, `which` are present |

## Design Patterns Worth Considering

Evidence for and against each option. No pick.

**Git origin: spawn `git` vs parse files vs CI env vs omit**

| Option | Evidence for | Evidence against |
|---|---|---|
| Spawn `git config --get remote.origin.url` | Matches existing helper style (`git_provenance.rs:199`); honours `include.path`, `includeIf`, `GIT_DIR`, worktrees, `url.<base>.insteadOf` handling is git's own (note `config --get` returns the RAW url before insteadOf; `git remote get-url` applies it). Measured p50 1.21 ms, p90 1.62 ms on WSL2 (spawn floor `/bin/true` 0.43 / 0.74 ms); Windows unmeasured | A process spawn on every invocation, including short `grim status`; fails if `git` is absent from PATH; extra attack surface (CWE-426 class already discussed for hooks) |
| Parse `.git/config` without spawning | ~5 us; feasible for plain checkout, linked worktree (follow `<gitdir>/commondir` to common `config`), submodule (`gitdir: ../.git/modules/<name>`, config at `<gitdir>/config`) | Misses `include.path`, `includeIf`, `extensions.worktreeConfig` / `config.worktree`, `GIT_DIR` / `GIT_CONFIG_*` env, global and system config. Experiment (git 2.54.0): section and key names are case-insensitive, values may be quoted and carry trailing `;`/`#` comments, and with two `url` lines `git config --get` returns the LAST (`b.example`), so a first-match parser disagrees. A new parser is new code with its own edge cases; no git-parsing crate in tree |
| CI env only | Zero cost; vendor-published variables; already a forge-identity read used for announce defaults | Only meaningful in CI; says nothing on a developer machine; org / namespace only, no repo path for GitLab beyond `CI_PROJECT_NAMESPACE` |
| Omit origin entirely | Consistent with ADR default (below); zero cost | The discussion's repo-grouping use case gets no signal |

**Disclosure stance already recorded for the same value**

- `git_provenance.rs:23-31` module doc: "`Auto` withholds the remote URL ... The `origin` remote does not [describe the content]: it names the **forge host and repository path** the artifact was built from. For anything published to a wider audience than the checkout it came from, that is an infrastructure disclosure the publisher did not ask for, so it stays behind the explicit `--git` opt-in."
- `scoped_to` `:265-271` clears `source_url` and `authors` unless `GitMode::Force`; `derive` is private on purpose (`:175-181`) so the rule is enforced by module boundary.
- `adr_default_provenance_and_support_channels.md` driver `:46`, section 2 `:71-87`; rejected alternatives `:209-212` (auto-deriving `source` from origin = "infrastructure disclosure on by default"). `adr_git_provenance_annotations.md:78` rejects automatic-when-in-git-repo.
- Scope difference: that rule governs bytes written into a published artifact (public audience). Telemetry egress goes to a collector. No ruling exists on whether the same reasoning transfers (Key Finding 14).

**Identity source (for a distinct user / machine count)**

| Option | Evidence for | Evidence against |
|---|---|---|
| None (count invocations only) | No identifier to protect; matches issue #83 payload scope (command, version, platform, error kind) | No distinct-user metric possible |
| Derive from `hooks/root-key` | Already a machine-local 32-byte CSPRNG value, 0o600 | Exists only after hook install (`root_token` creates; `existing_root_token` `:454` never creates); "rotating the key is a disarm-everything operation"; `hooks/` is a reserved namespace; absent on most machines |
| New random id file under `$GRIM_HOME` | Simple; `getrandom` + `hex` already direct deps | A shared `GRIM_HOME` (devcontainer / shared volume) makes one id span machines (`subsystem-file-structure.md:560-568`, `adr_install_state_portability.md:90-99`); new persistent file needs layout stability review (Principle 9); a write on first run |
| Hash of `workspace_key` | Function exists, `hook_dispatch.rs:369-376` | Unsalted SHA-256 hex of the lossy workspace path: machine-specific, dictionary-guessable, differs per machine for the same repo |
| Forge account id from `state/votes.json` | Immutable account id (`catalog/vote_store.rs:84`) | Exists only after `grim rate`; it is a real forge identity |

**Event source for `grim.artifact.changed` / `grim.artifact.changes`**

| Option | Evidence for | Evidence against |
|---|---|---|
| Report structs | Already in memory: `InstallEntry{kind,name,target,status,armed}` (`api/install_report.rs`), `UpdateEntry{kind,name,old,new,action,...}` (`update.rs:612-659`), `AddReport{kind,name,pinned,status}`, remove / uninstall / lock reports | Reports differ per command and are rendered per concrete type in the `app.rs` arm (`render(&r, format)`); `Printable` has generic methods (not object-safe), so there is no common trait object. `finish()` discards `reference.source` (`command/install.rs:495-542`). Install report has one row per locked artifact: unchanged rows dominate on re-install, so a changed-set filter is needed |
| Lock / declaration join | The declaration holds `DeclaredSource::Registry(Identifier)` | Lock wire projection strips the advisory tag (`lock/grimoire_lock.rs:539-540`; `LockedSource::provenance()` `locked_source.rs:111-116`); `to_declared` (`locked_source.rs:98-106`) rebuilds a digest-pinned id with no tag. Lock-derived refs are `registry/repo@sha256:...`, so the discussion's `registry/repo:tag` needs a declaration lookup |

## Key Findings

1. **No dispatch-time command name exists.** Grep for a `Command::name()` accessor is empty. `runtime_flavor(cli.command.as_ref())` at `src/main.rs:184` is the precedent for matching the parsed command at dispatch. Known after `parse_cli` (`main.rs:158`), argv-only, pure. Clap usage errors (exit 64, `main.rs:168`), help / version, and runtime-build failure return before this point.

2. **Requested scope is a flag; resolved scope is a file read.** `resolve_in` (`scope_resolution.rs`): `--global` -> Global (config `$GRIM_HOME/grimoire.toml`, workspace `$GRIM_HOME`), else `ProjectConfig::discover_from` -> Project. Exceptions: `release.rs:639` hardcodes project, `config.rs:1400` project, `export.rs:330` global, `search.rs:592` uses `args.global` per MCP call; the TUI can swap scope live (`tui.rs:91,107`, `g` toggle); `grim mcp` takes scope per call. Resolution failure = `NotDiscovered`, exit 79. About 15 command files call it.

3. **CI detection is narrow and credential-bearing.** `CiEnv::from_env` (`forge.rs:75-92`) knows `GITHUB_ACTIONS` and `GITLAB_CI` only. No generic `CI` variable and no other vendor (Jenkins, CircleCI, Buildkite) anywhere (grep empty). The struct stores `GH_TOKEN` / `GITHUB_TOKEN`, `GITLAB_TOKEN`, `GRIM_ANNOUNCE_TOKEN` as `Option<String>` (not `SecretString`) under `#[derive(Debug)]`, so reusing it for two booleans loads credentials into a Debug-printable struct. Called only from `rate.rs:754` and `publish.rs:1405`.

4. **Git origin is only ever read by spawning git, in build / release / publish.** `derive` (`git_provenance.rs:182`, private) runs `rev-parse HEAD`, `status --porcelain --untracked-files=no`, `show -s --format=%cI HEAD`, `config --get remote.origin.url` (`:199`, best effort), `show -s --format=%an HEAD` (`:206`): five spawns. Author is `%an` only, never `%ae` (`:111-118`). Not used on install / update paths.

5. **`normalize_remote_url` has an untested gap.** Verified by running the verbatim functions (scratchpad `normexp/n.rs`): `https://host/o/r?x=token123` -> `Some("https://host/o/r?x=token123")`; `#frag` kept; host case not lowercased (`HOST.Example.com` kept); `https://host/o/r.git/` (trailing slash) keeps `.git`; non-default https port dropped (`forge.corp:8443` -> `forge.corp`); `ssh://git@host:2222/o/r.git` -> `https://host/o/r`; userinfo stripped (up to the LAST `@`, `:357-362`). No test covers `?` / `#` shapes (tests at `:427-737`, userinfo tests `:526-575`). OBS-22 (`.claude/rules/rust-quality/docs-and-tracing.md:114`) requires redaction of signed URL parameters.

6. **A no-spawn origin read is feasible for plain cases, with caveats.** `.git` directory -> `<.git>/config`; linked worktree -> `.git` file `gitdir: <abs>/.git/worktrees/<name>`, that dir holds `commondir` (`../..`) and NO `config`, so the parser must follow `commondir`; submodule -> `gitdir: ../.git/modules/<name>` (relative), config at `<gitdir>/config`, no commondir, has `core.worktree`. Timing on this WSL2 box (indicative; Windows unmeasured): `git config --get remote.origin.url` p50 1.21 ms / p90 1.62 ms; `git rev-parse --show-toplevel` 1.44 / 1.80 ms; `/bin/true` floor 0.43 / 0.74 ms; reading `.git/config` about 5 us. Unhandled by a literal parse: `include.path`, `includeIf`, `extensions.worktreeConfig`, `GIT_DIR` / `GIT_CONFIG_*`, global / system config. Latest experiment (git 2.54.0, isolated HOME): mixed-case `[Remote "origin"]` / `URL = "..." ; comment` parses in git; two `url` values -> `--get` returns the last, `--get-all` returns both; `GIT_DIR=<path>` from another cwd resolves that repo's config.

7. **Existing worktree helper looks inert for its purpose.** `git_info_dir` (`hook_registrar.rs:1859-1876`) returns `<gitdir>/info` for a `gitdir:` file. Experiment: in a linked worktree `git rev-parse --git-path info/exclude` resolves to the COMMON dir's `info/exclude`; a rule written only to `<gitdir>/info/exclude` is not honoured (`git check-ignore` -> not ignored; same rule in the common dir -> ignored). The test `a_linked_worktree_writes_to_its_own_git_dir` (`hook_registrar.rs:3766`) asserts the current behavior. Hygiene only, not security (I1: ignore rules are not part of the control). Consequence: do not reuse `git_info_dir` for config lookup. Separately, `project_init_dir` (`tui.rs:190-196`) uses `.exists()`, so it accepts a `.git` dir or file, and stops BEFORE `$HOME` (exclusive) whereas the config walk-up includes `$HOME` (inclusive, `project_config.rs:172-191`). Its tests create `.git` only as a directory (`tui.rs:349,370`).

8. **Workspace root, git root, and cwd are three different things.** Workspace = parent dir of the discovered `grimoire.toml`, always absolute, resolved against cwd without canonicalize. `grim init` writes in cwd (`init.rs:59`); TUI init uses the git root. So workspace can equal, sit below (monorepo subdir), or have no relation to the git root; with `--config` it can be anywhere; global-scope workspace is `$GRIM_HOME`, not the cwd repo. A "git repo" attribute at global scope therefore needs cwd-based discovery, not the workspace.

9. **Client detection is cheap but late, and there are three client sets.** Detected (`detect_clients`), selected (`InstallTarget::parse`: flag > config `[options].clients` > detected > generic `agents` fallback with `is_generic_fallback()`), recorded (status `clients_missing` / `clients_extra`). `uninstall` / `remove` resolve no client set. `ClientTarget` is a closed enum (19 values): low cardinality. `adr_vendor_support_tiers.md:289-290`: "Re-check the Tier 1 set when usage telemetry lands (grimoire-rs/grimoire#83) — the first evidence-based promotion/demotion input".

10. **No persistent machine or install identifier exists.** No `install_id` / `machine_id` file; none of the id crates in `Cargo.lock`. Machine-local random exists only as `hooks/root-key` (tied to the hook feature). `workspace_key` is an unsalted path hash. `state/votes.json` is keyed by forge account id after `grim rate`. `$GRIM_HOME/state/global.json` and project `.grimoire/state.json` hold no id. A shared `GRIM_HOME` collapses distinct machines to one per-`GRIM_HOME` id.

11. **Per-artifact outcome data exists but loses the registry and tag.** `finish()` maps `InstallOutcome` (`installer.rs:77`: Installed / Updated / AlreadyInstalled / Skipped(String) / Refused / RefusedUntracked) to `InstallStatus` and drops `reference.source`. `ArtifactKind` is closed: Skill, Rule, Agent, Bundle, Mcp, Hook. Bundles expand to members, flattened in install rows. `update` rows carry content digests, not refs. `add` `pinned` is `registry/repo@sha256:...` (`add.rs:225-237,433-439`) or a bundle id.

12. **"Interactive" has four meanings.** Color uses stdout (`cli/color.rs:158`, plus NO_COLOR / CLICOLOR / CLICOLOR_FORCE / TERM=dumb `:60-80`); progress uses stderr (`cli/progress.rs:122-127`); `printer.rs:150` uses stdout width; `rate.rs:256,735` use stdin; the named `hook::trust::interactivity()` (`src/hook/trust.rs:289`) is stdin AND stderr both terminals (rationale: grimoire-vscode pipes `--format json` yet has a stdin TTY). All are `isatty` calls.

13. **Hook-run import bans constrain where telemetry code may live.** `RUNTIME_SOURCES` = `hook/{argv,envelope,pipeline,projector,run}.rs` (`src/command/hook.rs:123-129`); `FORBIDDEN` needles in code lines only (comments and everything after the first `#[cfg(test)]` stripped): `crate::config`, `scope_resolution`, `crate::context`, `grim_home(` (test `the_runtime_imports_no_scope_no_config_no_data_root_c007`). C-009 `the_runtime_computes_no_digest_c009` bans `crate::store::hash`, `Algorithm::`, `Sha256`, `.hash(` in those five files. `every_declared_runtime_module_is_checked` forces a new `pub mod` in `hook.rs` to be classified. `app_dispatches_the_runtime_before_it_builds_a_context_b1` pins `hook::run::run(` before `Context::new(` in `app.rs` (comment `app.rs:44-57`: keeps "the dispatch path never reads an attacker-choosable data root" true of the whole process). `the_hook_runtime_is_not_built_on_the_multi_thread_scheduler_f1` pins `main.rs` strings. Consequences: a new module (for example `src/telemetry.rs`) or code in `main.rs` is not scanned and would not trip these tests; but telemetry init reachable before the `Hook(Run)` early return in `main()` that reads `grim_home()`, config, or Context would falsify the whole-process claim with no test catching it. Precedent for plain env reads before dispatch: `GRIM_LOG` (`main.rs:153`) and color env (`color::init`, `:176`) run for every command including `hook run`.

14. **No egress or disclosure ruling exists for telemetry.** Verified by grep: `arch-threat-model.md`, `adr_artifact_trust_model.md`, `quality-security.md`, `rust-quality/security.md` contain no text on disclosure, remote URL, origin, egress, exfiltration, telemetry or PII. `GRIM_TELEMETRY` and `DO_NOT_TRACK` are absent from `src`, `docs`, `catalog`; no mention of telemetry in `docs/src/content/docs/*.md`, `AGENTS.md` or `catalog/`. Existing related rules: OBS-12 (bans `opentelemetry*`; `docs-and-tracing.md:99`), OBS-13 (remote crash reporting opt-in, `:100`), OBS-17 (spans, `:109`), OBS-22 (URL redaction, `:114`), OBS-23 (run_id UUID/ULID, `:115`). `adr_artifact_ratings.md:1097` is the nearest precedent for disclosure by confirmation prompt (public votes).

15. **Version / platform data is free but unused.** No `std::env::consts::{OS,ARCH}` use in `src/`; only `cfg!(windows)` / `cfg!(target_os="macos")` branches and `HOOKS_SUPPORTED = cfg!(unix)` (`hook_registrar.rs:399`). No `build.rs`, no git SHA or build date (OBS-15 unmet).

16. **Error classification is safe to export only as slug.** `ErrorReason` Display = slug; `retryable()` only `Locked`. `error_document` `message` is the full chain (hosts, paths) and must not be exported. `is_stdout_pipe_closed` early return (`main.rs:207-209`) converts to exit 0.

## Recommendation

None by design. This lane is a recon; it records evidence for and against each option and leaves the pick to the discussion and a later ADR.

## Sources

| Source | Type | Date | Relevance |
|---|---|---|---|
| `src/main.rs`, `src/app.rs`, `src/context.rs` | Code | 2026-10-22 (HEAD `7b21abce`) | dispatch order, command enum, Context construction |
| `src/oci/git_provenance.rs` | Code | 2026-10-22 | only origin reader, disclosure doc, `normalize_remote_url` |
| `src/install/hook_registrar.rs`, `src/command/tui.rs`, `src/config/project_config.rs`, `src/command/scope_resolution.rs` | Code | 2026-10-22 | git / workspace discovery |
| `src/catalog/forge.rs` | Code | 2026-10-22 | CI env detection |
| `src/install/target.rs`, `src/install/vendor*.rs` | Code | 2026-10-22 | client detection |
| `src/command/hook.rs`, `src/hook/trust.rs`, `src/hook_dispatch.rs` | Code | 2026-10-22 | import-ban tests, interactivity, root key |
| `.agents/adr/adr_default_provenance_and_support_channels.md`, `adr_git_provenance_annotations.md`, `adr_vendor_support_tiers.md`, `adr_artifact_ratings.md` | ADR | 2026-10-22 | disclosure precedent, telemetry dependency |
| `.claude/rules/rust-quality/docs-and-tracing.md` | Rule | 2026-10-22 | OBS-12/13/17/22/23 |
| Scratchpad experiments `gitexp/`, `normexp/` (git 2.54.0, WSL2) | Local run | 2026-10-22 | worktree layout, config semantics, normalizer behavior, timings (WSL2 only; Windows unmeasured) |
