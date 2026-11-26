# ADR: Repository split — `grim-mirror` and `grim-gui` leave grimoire, `grim-sdk` stays

## Metadata

**Status:** Accepted (owner decisions, 2026-11-25); orchestrator rulings PD-1 to PD-27 and ED-1 onward are open to the owner's veto
**Date:** 2026-11-26
**Deciders:** Michael Herwig (owner, rulings in `.agents/discussions/repository-split.md` § Decisions) + Claude (orchestrator, `/hex-plan` xhigh and `/hex-execute`, goal loop `.agents/goals/repository-split.md`)
**Beads Issue:** N/A
**Related PRD:** N/A (ratified discussion above; plan `.agents/plans/plan_repository_split.md`)
**Tech Strategy Alignment:**
- [x] Rust 2024, Tokio for `grim-sdk` and `grim-mirror`
- [x] Deviation: Tauri 2 and TypeScript for `grim-gui` only (owner decision, named below; `product-tech-strategy.md` lists Rust only); it binds no other crate
**Domain Tags:** integration | api | devops
**Supersedes:** N/A
**Extends:** `adr_artifact_mirroring.md` (the tool's repository, not its design)

## Context

`adr_artifact_mirroring.md` placed the mirror tool as a workspace crate of this repository, shipping in [PR 398](https://github.com/grimoire-rs/grimoire/pull/398). Before that PR merged the owner chose to give the tool and a new desktop GUI each their own repository, on the same scaffold as grimoire, with grimoire carrying one small client crate that both call.

## Decision Drivers

- grim has no library API and is freezing for 1.0 (Principle 9); a shared library of grim's internals would become a contract.
- A second compiled-in grim would write the same `$GRIM_HOME`, lock and install-state files as the terminal grim.
- The tool and the GUI release on their own cadence.

## Considered Options

- **Monorepo with a joint release.** Rejected: grim's release would wait on a desktop application, and it still leaves the second-grim problem.
- **A carved-out core library, or a grim submodule.** Rejected: exposes grim's internals; ocx's recorded friction comes from that link.
- **Split, scaffold copied, grim reached only through the binary by way of `grim-sdk`.** Chosen.

## Decision Outcome

### Owner decisions (Accepted)

- **Split, not monorepo.** `grimoire-rs/grim-mirror` and `grimoire-rs/grim-gui` are separate repositories.
- **Base means scaffold, not code.** Configuration, tasks, rules, test harness and CI shape are copied or installed from bundles; no shared code library.
- **The mirror crate is cut out of [PR 398](https://github.com/grimoire-rs/grimoire/pull/398) before merge,** with its ten acceptance suites and the harness they need.
- **`grim-gui` invokes the grim binary,** never links grim; it is a Tauri 2 skeleton (a named deviation from `product-tech-strategy.md`), binary `grim-gui`, started directly or as `grim gui`; the interface itself is another agent's work.
- **A minimal `grim-sdk` lands in grimoire,** a Rust crate that only invokes the binary: own `0.1.0` line, Unstable, unpublished, not a library API of grim.
- **The base is copied from grimoire; rules and skills come through grim bundles** first, and everything no bundle carries is copied or rewritten and listed in a manifest. No conventions repository for now.
- **Handoff is a goal loop;** the owner may create and push to the two new repositories.

### Orchestrator rulings (open to the owner's veto)

The full text of each, with the alternative not taken, is in the plan's § Decisions taken while planning (PD-1 to PD-27) and § Execution decisions (ED-1 onward). In short:

- PD-1, PD-2 (planning tier): tier xhigh; no architect seat, this ADR records the decisions.
- PD-3, PD-4 (one new function, ignored acceptance test): the one new function is `GrimDriver::require_version`; the SDK's acceptance check is an ignored Rust test run by a task against the grim built in the same run.
- PD-5, PD-6 (SDK reach): the SDK manifest has its own version and inherits the rest; during the run the new repositories reach it through a symlink outside both repositories, and after the push through a git `rev`.
- PD-7 (GUI bridge): the GUI bridge calls `GrimDriver::version()`. This follows the source ruling ("a Rust command asks `grim-sdk` for the version of the grim it finds"); the alternative of running `grim context --format json` was not taken.
- PD-8 to PD-11 (repository bootstrap): the mirror repository keeps the `crates/grim-mirror/` layout and a fresh `0.1.0` line; new repositories start with an empty root commit on `main` and work on `hex/initial-scaffold`; copies name only the three fixed source commits.
- PD-12 to PD-16, PD-26 (test and build setup): the new repositories test against the grim built in this checkout; Tauri builds run in a container; one build lock; `actionlint` pinned in each repository.
- PD-17 to PD-25, PD-27 (content and push order): the mirroring docs page moves with no stub; git-hardening snapshot, founding records, rule set and inventory row keys as the plan states; the repositories are pushed only after the pin step (PF).
- ED-1 to ED-12 (execution-time decisions): among them ED-6 (a fake grim exiting 78 shows exit 69 through C-004), ED-9 (`docs-essentials` in `grim-gui`), ED-11 (the licence-header check under hawkeye 7 is fixed in the two new repositories; grimoire's own copy is a follow-up) and ED-12 (the link-check exclusion list is derived from `grimoire.lock`).

### Evidence

R11 (the crate is removed only after its home passes): `grim-mirror` commit `1a476f8a357b8ad4cd61ac4ac49757047a8a8fc3` on `hex/initial-scaffold` holds the crate and its ten suites with a green `task --force verify`. **Local only until act 4** (the owner creating the GitHub repository); no CI run backs it yet.

### Finalize invariant

`/hex-finalize` reshapes this branch. Six branch commits carry mirror hunks (named in the plan § Rollout and migration by subject), and the later commits remove them again. The finalized tree must equal the target tip, the branch head after the last fix pass (the orchestrator records its SHA in the plan). Two checks must pass: the path check `git log -p main.. -- crates/grim-mirror test/src/git_fixture.py 'docs/**/mirroring.md'` prints nothing, and the content check `git log -S'grim-mirror' main.. -- . ':!.agents' ':!docs'` is reviewed by hand (docs keep links to the new repository).

### Consequences

**Positive:**
- grim's frozen surface stays the binary; the SDK is Unstable and unpublished.
- Each tool releases without grim.

**Negative:**
- Three repositories to keep in step; the manifests record drift but no tool enforces it.
- Both new repositories pin grim by commit until a release carries dispatch, the `mirror` table and `grim convert`.
- Machine-local path (PD-6, PD-27): both new repositories commit a `../.grimoire-split` path that resolves only on the owner's machine until the pin step, so CI cannot build before it (plan § Decisions).
- Tauri build (PD-13): Docker-only on this host; macOS and Windows are unproven until CI.
- GUI acceptance (PD-14): proves dispatch, `GRIM_BIN` and the bridge, not the rendered UI.
- Exit codes (ED-6): `grim-sdk` maps codes outside 69, 74, 75, 80, 81 to 69, so a caller loses the raw code.
- Reserved surface (PD-3): `require_version` has no consumer yet.

**Risks:**
- No CI has run in the two new repositories (macOS and Windows builds of `grim-gui`, the macOS `unsafe_code` check). Mitigation: act 4 of the plan, owner-run.

## Links

- `.agents/plans/plan_repository_split.md`, `.agents/discussions/repository-split.md`
- `.agents/adr/adr_artifact_mirroring.md`
