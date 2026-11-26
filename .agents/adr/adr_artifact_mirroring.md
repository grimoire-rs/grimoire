# ADR: Artifact mirroring — `grim-mirror`, PATH plugin dispatch, mirror provenance and `grim convert`

## Metadata

**Status:** Proposed
**Date:** 2026-11-24
**Note (2026-11-26):** the `grim-mirror` crate moved to its own repository, `grimoire-rs/grim-mirror`, before this branch merged (`adr_repository_split.md`). D1, D5 to D8 and D11 to D17 now bind that repository. The grim side stays here: D2 to D4, D9, D10, D18, D19. Status is unchanged.
**Deciders:** Michael Herwig (owner, rulings in `.agents/discussions/artifact-mirroring.md` § Decisions) + Claude (architect, `/hex-plan` xhigh, goal loop `.agents/goals/artifact-mirroring.md`)
**Beads Issue:** N/A
**Related PRD:** N/A (ratified discussion above; design `.agents/specs/design_artifact_mirroring.md`)
**Blast radius:** external contract on the grim side: PATH plugin dispatch for unknown subcommand names, the `GRIM_BIN` variable grim sets for a plugin, a per-entry `mirror` table in `publish.toml`, three new `com.grimoire.mirror.*` annotations and the precedence of four existing ones, and a new `grim convert` subcommand (Unstable). Unstable sibling surface: the `grim-mirror` binary, its `mirror.toml` grammar, its report and exit codes. Internal: `src/main.rs` parse-error path, `src/command/publish.rs`, `src/oci/annotations.rs`, `src/install/vendor*.rs` (read only, for the inverse), a new workspace crate `crates/grim-mirror`.
**Reversibility:** per decision, collected in D19. One-way doors: the dispatch rule and the plugin name space it creates (every future built-in name shadows a plugin), `GRIM_BIN`, the `mirror` table keys and the annotation keys (frozen at release), and the snapshot version grammar (every published snapshot is an immutable tag that later releases are ordered against). Two-way: everything inside `grim-mirror`, `grim convert`'s flags and report while Unstable, the licence matcher, the default allowlist.
**Tech Strategy Alignment:**
- [x] Rust 2024, Tokio, `serde`, `toml`, `serde_json`, `clap`, `regex`, `globset`, `semver`, `tempfile` (all in `Cargo.lock` already)
- [x] One new dependency, `spdx`, in `grim-mirror` only, never in the `grim` binary (D11)
**Domain Tags:** integration | api | security
**Supersedes:** N/A
**Extends:** `adr_artifact_trust_model.md` (scope note on mirrored content, D18), `.claude/rules/arch-threat-model.md` (PATH dispatch and mirrored content, D18), `adr_default_provenance_and_support_channels.md` (one tier above every authored tier, for mirror-table fields only, D4), `adr_tool_namespaced_metadata_rendering.md` (the inverse of its lift, D9)

## Context

The owner chose to republish content authored elsewhere — git repositories of skills, rules and agents, and Claude Code plugin marketplaces — as grim OCI artifacts, the shape `ocx-mirror` has for upstream releases. The discussion fixed the operator model, the kinds, the licence rule, the version sources, the tool home and the translation into grim's general form. It left eleven questions open, each with a recommendation; the goal loop needs all four Done criteria (monorepo import, bundles, non-skill kinds, marketplace input) met inside one PR on this repository.

### Premise corrections

| # | Discussion or research statement | Fact at HEAD `896dcb6e` | Effect |
|---|---|---|---|
| 1 | `a174a953` (refuse a skill whose root holds `.claude-plugin`) is "not on this branch" | It is on this branch (`src/skill/skill_package.rs:320-351`) | D8 strips that entry before staging; grim's refusal stays the backstop |
| 2 | Kinds refused: hooks, MCP, settings | `mod` shipped since the research (`ArtifactKind::ALL` has 8) | `mod` refused on the same ruling (D8) |
| 3 | A `0.0.0-<date>.<n>` snapshot might work | `grim publish` refuses any non-strict entry version (`src/command/publish.rs:738-740`, `:1948-1953`), a prerelease gets no floats (`src/oci/release.rs:125-183`) and no range or Any line admits one (`src/oci/version_claim.rs:160-165`, `:249-264`) | D5 uses a strict `X.Y.Z` |
| 4 | Origin "carried in the existing `repository`, `homepage` and `license` fields" | `repository` must be HTTPS (`adr_repository_annotation.md`); frontmatter outranks `publish.toml` `[metadata]` (`adr_default_provenance_and_support_channels.md`), so an upstream `license: Complete terms in LICENSE.txt` would win over the mirror's value | D4 adds a `mirror` table that outranks every authored tier, for its own fields only |
| 5 | `revision` names the upstream commit when publishing from a clone | `revision` comes from the git tree the artifact path sits in (`src/oci/git_provenance.rs`); the mirror stages outside any clone | D4 sets `revision` from the mirror table |
| 6 | ocx's `OCX_BINARY_PIN` is "the path of the running ocx", and ocx execs the plugin | `plugin_binary_pin` derives the pin from ocx's home launcher with `current_exe` as input, and dispatch spawns and waits (`ocx/crates/ocx_cli/src/app/plugin_dispatch.rs:64-88`) | D3 takes `current_exe`; D2 execs on Unix |
| 7 | "One enum per workspace, shared by all binaries", in a library crate (`quality-rust-exit_codes.md`) | The enum lives in the bin crate (`src/cli/exit_code.rs`); the launcher already keeps its own table pinned by name against it (`the_launcher_exit_table_equals_the_grim_exit_codes_of_the_same_name`) | D14 follows the launcher precedent |

### Open question → decision

| # | Open question (discussion § Open questions) | Decided in |
|---|---|---|
| 1 | Snapshot version grammar | D5 |
| 2 | Which names dispatch, from where | D2, D3 |
| 3 | Explicit "this is a mirror" marker | D4 |
| 4 | Author publishes natively or asks for removal | D15 |
| 5 | Licence allowlist and where it is read | D11 |
| 6 | Upstream content changes under a mirrored version | D6 |
| 7 | One repository, many units and bundles, in the mirror file | D7 |
| 8 | Plugin trees and the `.claude-plugin` refusal | D8 |
| 9 | Running the curated list and keeping it fresh | D17 |
| 10 | Where the dialect translation lives | D9 |
| 11 | Bodies naming one harness's tools or plugin-root variable | D10 |
| — | Orchestrator: repository scope, stability, shipping | D1 |

## Decision Drivers

1. **Done criteria 1–4 inside one granted repository** (goal file), with no feature cut.
2. **Principle 9.** Every grim-side change additive; inputs that worked before give byte-identical output, unknown-subcommand behaviour included (`test/tests/test_smoke.py:69-90`).
3. **The owner's two load-bearing properties of the tool home:** a separate binary reached by `grim <name>`, and grim's frozen surface does not absorb the mirror's.
4. **Threat model scope:** T1 (the operator is now the publisher of record), T2 (a moved upstream tag), T3 (PATH from repository files). No invented gate (`arch-threat-model.md` § Applying it).
5. **The mirror runs no upstream code** (owner ruling, Build steps).
6. **Small frozen surface.** Anything a later split or a second consumer may need to change stays Unstable.

## Industry Context & Research

**Research artifacts:** `research_artifact_mirroring_recon.md`, `_archaeology.md`, `_prior_art.md`, `_vendor.md`, `_adjacent.md`, `_repo_survey.md`, `_version_signals.md` (all 2026-11-23, at `92ebcf23`; facts above override them where they differ).

**Key insights carried in:** ocx's plugin pattern (`adr_cli_plugin_pattern.md`, built-ins win, git-style argv, PATH only) and repo-split template (`adr_cli_repo_split_template.md`, satellites start as workspace crates); Claude marketplace entries are git pointers with `ref`/`sha` (`_vendor.md` Q2); distributions give every unit split from one source the same version and order snapshots below a later release (`_adjacent.md` Q1–Q2); Open VSX and Cursor gate on OSI or permissive licences, no system reads SPDX (`_vendor.md` Q7); the commit that last changed a unit's subtree is the only version signal every surveyed repository has (`_version_signals.md` Part 3).

## Decision Outcome

Scores below are 0–3 per criterion, multiplied by the weight; the highest total wins unless a ruling or a recorded reason overrides it.

### D1. Repository scope, stability and shipping (orchestrator decision R2, confirmed)

Criteria: **G** meets Done 1–4 in the granted repository (4), **O** keeps the owner's tool-home properties (4), **S** later split is mechanical (2), **F** frozen-surface growth (3).

| Option | G | O | S | F | Total |
|---|---|---|---|---|---|
| R1 grim-side only, mirror deferred | 0 | 2 | 3 | 3 | 23 |
| **R2 workspace crate `crates/grim-mirror`, separate binary, no path dependency on `grimoire`** | 3 | 3 | 3 | 3 | **39** |
| R3 built-in `grim mirror` | 3 | 0 | 0 | 0 | 12 |
| R4 create the new repository now | 3 | 3 | 3 | 3 | — outside the run's grants |

**Chosen R2.** It meets every criterion and matches ocx's own history (ocx-mirror was a workspace crate before `adr_cli_repo_split_template.md` split it). The crate talks to grim only through `grim` argv and `--format json` output; the one source file it shares textually is grim's exit-code table, read by a test (D14). The split is a follow-up issue for the owner: `git filter-repo --path crates/grim-mirror`, grim vendored as a submodule if the exit-code test is kept.

- **Stability:** the `grim-mirror` CLI, `mirror.toml` grammar, report and exit codes are an **Unstable sibling surface** until the split, listed by name under `stability.md` § Unstable. Reason: the split changes how it is installed and released, and the first operator runs will move the grammar.
- **Shipping:** **source-only** in the first version: `publish = false`, `[package.metadata.dist] dist = false`, not added to `dist-workspace.toml` (members stay `["cargo:."]`). An operator's CI runs `cargo install --locked --git <grimoire> grim-mirror` or builds the workspace. Reason: adding it to grim's 8-target release ties its cadence to grim's and creates an install path the split would break; a curated-list job builds once per run at negligible cost. Options not taken: ship in grim's archives (cost above); a separate `cargo-dist` app in this workspace (a second release pipeline to tear down at the split).
- **Name reservation:** grim adds no built-in named `mirror`, so `grim mirror` stays the plugin (D2's built-ins-win rule would otherwise shadow it).

### D2. Plugin dispatch: which names, from where

Criteria: **B** byte-identical behaviour for every input that works or fails today without a plugin (4), **T** no capability beyond what PATH control already grants (4), **U** ocx and git familiarity (2), **K** code size (1).

| Option | B | T | U | K | Total |
|---|---|---|---|---|---|
| clap `external_subcommand` on `Command` | 1 (typo suggestions and help change) | 3 | 3 | 2 | 21 |
| **Dispatch only from clap's `InvalidSubcommand` error at top level** | 3 | 3 | 2 | 2 | **30** |
| Plugin manifest or registry of allowed plugins | 3 | 3 | 0 | 0 | 24 |

**Chosen: dispatch from the parse error.** After `parse_cli` fails with `ErrorKind::InvalidSubcommand`, grim scans argv once (spec § 3.1) and dispatches only when all hold: the unknown token is the **first** positional after top-level global flags (so `grim config nosuch` never dispatches), it matches `^[a-z0-9][a-z0-9-]{0,62}$`, and a binary `grim-<name>` (`grim-<name>.exe` on Windows) is found. Otherwise clap's original error prints unchanged and grim exits 64, so the not-found path is byte-identical to today, `--format json` included (no error document, as before parse). Built-ins and their aliases always win by construction: clap matched them.

- **Search:** `PATH` only, entries in order; an empty or non-absolute entry is skipped (on POSIX an empty entry means the current directory, CWE-426); a candidate must be a regular file, and on Unix have an execute bit. Not the directory of the running grim: it adds a second rule for no case that PATH misses (the installers put both on PATH). Windows: `.exe` only, no `PATHEXT`, so no `.bat`/`.cmd` (CVE-2024-24576 class).
- **Execution:** the program is the absolute path the lookup found, argv stays `OsString` end to end. Unix replaces the process (`CommandExt::exec`), so exit status and signals are the child's own (a shell sees 128+n). Windows spawns, ignores Ctrl-C while waiting (the console delivers it to the child), and exits with the child's full `u32` code, NTSTATUS values included. Dispatch runs before tracing, the Tokio runtime and `Context`, so it is never a telemetry event (`Telemetry::start` takes a built-in `&Command`).
- **argv:** git style, the name dropped: `grim mirror sync -f x` runs `grim-mirror sync -f x`. Everything after the name is the plugin's.
- **Not added:** `grim help <plugin>` rewriting (ocx has it; it would change today's error for `grim help <unknown>`), plugin listing in `--help` and completions (help lists built-ins only), an install hint in the not-found message (it would change today's bytes).

**Threat model.** Whoever controls `PATH` — the user, or a repository through `.envrc`, `.mise.toml` or devcontainer env (`arch-threat-model.md:119-121`) — already chooses which `grim` runs, so a `grim-<name>` lookup on the same `PATH` gives that attacker nothing new (T3 unchanged). The skip of relative entries removes the one case where dispatch would be weaker than running a bare `grim`. Dispatch runs only on an explicit invocation of an unknown name, never from install, a hook, the TUI or MCP, so I4 (default-deny for new execution) holds: no input that ran nothing before runs something without the user typing it. A T4 agent that types `grim foo` could equally type `grim-foo`.

### D3. Global flags before the name, and the child's environment

Criteria: **R** honours `subsystem-cli.md` § Configuration Forwarding (4), **I** I6 (3), **M** the mirror's real needs (3).

| Option | R | I | M | Total |
|---|---|---|---|---|
| Forward every global flag as a private state variable (ocx) | 1 | 3 | 2 | 19 |
| **Forward `--offline`/`--locked` as their documented variables, refuse every other global flag before the name (64)** | 3 | 3 | 3 | **30** |
| Ignore global flags before the name silently | 0 | 3 | 1 | 12 |

**Chosen.** `--offline` becomes `GRIM_OFFLINE=1` and `--locked` becomes `GRIM_LOCKED=1` on the child, the two resolution flags with a documented variable. All `GRIM_*` keys a child receives come from one function, `crate::env::plugin_child_env`, as `subsystem-cli.md` § Configuration Forwarding requires (no such function exists today; grim spawns no grim-aware child yet). `--format`, `--color`, `--progress`, `--log-level` (presentation, must not propagate), and `--config`, `--global`, `--registry` (no variable carries their exact meaning; `GRIM_DEFAULT_REGISTRY` does not collapse browse, `GRIM_CONFIG` is telemetry layers) placed before a found plugin's name exit 64 (the forwarding rule says config and index flags propagate; with no variable of the same meaning, forwarding would invent new frozen variables or change meaning, so refusing is the honest form, recorded as this deviation) with `grim: '<flag>' before a plugin name is not supported; write it after the name: grim <name> <flag>`. Placed after the name they are the plugin's arguments.

- **`GRIM_BIN`:** grim sets it to `std::env::current_exe()` made absolute (canonical on Unix, never `\\?\` verbatim on Windows, via `dunce`), so the plugin calls the same grim (ocx `OCX_BINARY_PIN`). grim never reads it. It joins the env-var table as "set by grim for a plugin".
- **No variable is stripped.** I6 bars grim from *placing* secrets in argv or env; dispatch places none. The child inherits the user's environment, which is the user's own process tree. Stripping `GRIM_ANNOUNCE_TOKEN` would break the mirror's own `grim publish` child, and the plugin is code the user put on `PATH` that can read the same credential files grim reads. ocx strips signing variables because its mirror resolves signing refs from env (a confused deputy); grim's plugins have no such ladder.

### D4. Mirror marker and provenance: a `publish.toml` `mirror` table

Criteria: **A** additive (4), **P** survives authored frontmatter (3), **C** consumers can label without parsing `repository` (2), **F** frozen keys (2).

| Option | A | P | C | F | Total |
|---|---|---|---|---|---|
| Existing fields only (`repository`, `license`, `homepage`) | 3 | 0 | 0 | 3 | 18 |
| **Per-entry `[<kind>.<name>.mirror]` table, three new `com.grimoire.mirror.*` keys** | 3 | 3 | 3 | 2 | **31** |
| New `grim publish` flags (`--mirror-source`, …) | 3 | 3 | 3 | 0 | 27 (per run, not per entry; a run holds many units) |

**Chosen.** `PublishEntrySpec` gains an optional `mirror` table (every kind table, since the struct is shared; `deny_unknown_fields` inside it). Keys: `source`, `commit`, `path`, `license`, `license-source` required; `deprecated`, `replaced-by` optional. It emits:

| Annotation | Value | Precedence |
|---|---|---|
| `com.grimoire.mirror.source` (new; its presence is **the marker**) | upstream repository URL, `https://` or `file://`, no userinfo | — |
| `com.grimoire.mirror.path` (new) | unit path inside the upstream repository, `.` for the root | — |
| `com.grimoire.mirror.license-source` (new) | `detected` or `declared` | — |
| `org.opencontainers.image.revision` | `commit`, 40 lowercase hex | wins over the derived value |
| `org.opencontainers.image.licenses` | `license` | wins over every authored tier |
| `org.opencontainers.image.source` | `source` when `https://` | wins over every authored tier; a `file://` source leaves the normal chain |
| `com.grimoire.deprecated` / `com.grimoire.replaced-by` | when set | wins over every authored tier |

The outranking applies only to these fields and only with the table present, so no manifest without it changes. The mirror needs it because the bytes stay verbatim (an upstream frontmatter value must not be rewritten to make provenance true). `grim describe` gains **no** curated field: `annotations` is already the verbatim map (`json-interface.md`, describe row), which the indexer can read; a curated `mirror` field waits for a consumer to ask, so consumer release order (`subsystem-artifact-kinds.md`) is not engaged. `package-index.md` stays unchanged, as the discussion expected. An older grim installs a mirrored artifact unchanged (unknown annotation keys mean "does not say", `stability.md`); an older grim given a manifest with a `mirror` table refuses it at parse, which the mirror pre-empts with a version check (D13).

### D5. Snapshot version grammar

Criteria: **V** publishable by `grim publish` and visible to floats and range claims (4), **O** orders above the stale tag a forced snapshot replaces and below a real next release where possible (3), **D** deterministic from upstream data (3), **K** simplicity (1).

| Option | V | O | D | K | Total |
|---|---|---|---|---|---|
| Prerelease `0.0.0-<date>.<n>` with an additive publish loosening | 0 (no float, no range, no Any line) | 2 | 3 | 1 | 16 |
| `0.0.<YYYYMMDDHHMMSS>` always | 3 | 1 (a forced snapshot on a tagged repo sits below the old tag forever) | 3 | 3 | 27 |
| **`<M>.<m>.<YYYYMMDDHHMMSS>`, `M.m` from the highest pattern-matching tag reachable from the commit, else `0.0`** | 3 | 3 | 3 | 2 | **32** |
| Count-based (`r<count>`, Arch) | 3 | 1 | 1 (history rewrites renumber) | 2 | 20 |

**Chosen base-relative.** The patch is the UTC committer date (`%ct`, not author date, which rebases keep) of the commit that last changed the unit's subtree, as 14 digits; it fits `u64` and semver's numeric rule. Reason: the owner made forced snapshot mode the cure for "a repository that rarely tags stays stale" (2132 commits after ECC's last tag, `_version_signals.md` §a), and only a version above that tag reaches users following `latest` or the major float. With no tags it is `0.0.<ts>`: below every release `≥ 0.1.0`, but above any `0.0.Z` release (unlike Debian's `~` or conda-forge's `0.0.0.` prefix, which sort below everything). Units of one repository can sit on different bases, each from the tags merged into its own revision. Rules:

- **Same-second collision:** a different commit with the same timestamp yields an existing version with a different `revision`; the unit is `refused` `version-collision` until a later commit (D6).
- **Clock skew or rewritten history** giving an older timestamp publishes a lower version; the cascade never moves a float off the higher one (`src/oci/release.rs`), so nothing regresses.
- **Mode switch back to tags, or a first tag after a snapshot phase:** the tag-mode watermark ignores *snapshot-shaped* versions (patch of exactly 14 digits, D6), so the real release still publishes; but a later `M.m.(Z+1)` (or a first `0.0.Z`) sorts below a published `M.m.<ts>`, so the `M.m` float stays on the snapshot until a higher release. Documented in the mirror reference (release above the snapshot or skip to the next minor); no machinery.

### D6. Tag mode, auto mode, and upstream bytes changing under a version

Version mode per upstream entry: `auto` (default: tag mode when any tag matches `tag-pattern`, else snapshot), or `snapshot` (forced). `tag-pattern` is a regex (`regex` crate, already in `Cargo.lock`) whose first capture group must yield strict `X.Y.Z`; default `^v?(\d+\.\d+\.\d+)$`. A tag that matches but does not capture strict semver, and every prerelease, is skipped with a warning (unpublishable, fact 3). Options not taken for the pattern: a literal prefix string (cannot express a version embedded mid-name such as `release/1.2.3` or `copilot-plugin-v7.1.0-<sha>`); a version script (ruled out by the owner).

- **Eligible tags** are those merged into the candidate commit (`git tag --merged`), so `ref` bounds tag mode as it bounds snapshot mode. Two eligible tags capturing one version with different commits → that version `refused` `version-collision`.
- **What the target holds:** `grim describe <repo>:latest` gives the tag listing; exit 79 means no listing, and each version the mirror considers is probed exactly (79 = not published). Cached per run; at most `2 + backfill` describes per unit (spec § 5.7).
- **Selection** (what is new): eligible versions above the watermark `W` = the highest published non-snapshot-shaped strict-semver version, newest `backfill` of them (default 1). Reason: K-Dense has 109 tags × 177 skills; an unbounded first run is 19k pushes. Every unit present at the tag's commit gets the tag's version (the owner's consequence), with `revision` = the tag's peeled commit.
- **Reconciliation** (what is published), separate from selection so an idle run still verifies: `W` and every published version among the newest `backfill` eligible versions are compared with their tag's commit. An existing exact tag whose `org.opencontainers.image.revision` differs is `refused` (`upstream-moved`; `version-collision` in snapshot mode), exit 65, never forced; equal is `unchanged`. Residual, proportionate to T2: a moved tag below `W` and outside the window is not re-checked; `backfill` sets that cost. Options not taken: republish with `--force` (moves an exact tag, breaks every pin: T2 by our own hand); silently keep (hides a moved tag the operator should see); re-check every historic tag each run (one describe per tag per unit per run). `ocx-mirror` recorded no decision here (`_archaeology.md` §1.4).
- **Equal revision is not equal output.** A converter, mirror-file or grim packing change can alter the bytes of a version whose revision matches. The mirror compares the existing digest with a `grim publish --dry-run` digest (the dry-run preview equals a real push's digest, `publishing.md` § compression; grim writes no grim-version annotation) and reports `unchanged` with warning `output-drift`. No new frozen key; the operator gets a new version from the next upstream commit or tag, or by `retire`. For a bundle the same mismatch means its member set changed under one version: `refused` `bundle-drift`.
- **Idempotency:** a run with nothing new publishes nothing and reports `unchanged`; it does not rely on `grim publish` skip-existing alone, because skip-existing cannot tell an equal commit from a moved one (a `Skipped` item is re-read).

### D7. Mirror file shape: one entry per upstream repository

Criteria: **M** many units per repository (Done 1) (4), **B** bundles of mirrored members (Done 2) (3), **R** reviewable diffs in a list repository (2).

| Option | M | B | R | Total |
|---|---|---|---|---|
| One file per package (ocx) | 0 | 1 | 3 | 9 |
| **One `[[upstream]]` per repository: kind glob lists, `exclude`, `[[upstream.bundle]]`, optional marketplace enumerator** | 3 | 3 | 2 | **25** |
| One `[[unit]]` per artifact, repository repeated | 3 | 2 | 1 | 20 |

**Chosen.** Grammar in the spec § 4: `[mirror]` (registry, allowlist, backfill) and `[[upstream]]` (`name`, `repo`, `ref` or `commit`, `dialect`, `version`, `tag-pattern`, `license`, `unit-licenses`, `skills`/`agents`/`rules` globs, `exclude`, `marketplace` + `plugins`, `[[upstream.bundle]]`, `retire`). One file may hold many upstreams; `grim-mirror` takes `--upstream <name>` repeatable to run a subset.

### D8. Kind mapping, partial bundles, plugin trees

Mirrored: skill (a directory with `SKILL.md`), agent (`.md` under an `agents` glob), rule (`.md` under a `rules` glob), bundle (a declared `[[upstream.bundle]]` or one per enumerated plugin). Refused in the first version, by the owner's ruling (no signing, so nothing tells a user that executable content reached them unchanged): hook, MCP server, settings, and **mod** (new since the research, same reason). This is the mirror's product policy, not a grim install gate on a shipped kind, so `adr_artifact_trust_model.md` decision 2 is untouched.

- **Claude `commands/*.md`: skipped and reported** (`kind-unsupported`, `command`) — a **policy, deferred**, not a necessity. Claude Code documents commands as merged into skills, so a translation is possible and the skip loses real content. Reason for deferring: the owner's kind ruling lists skills, rules, agents and bundles, and the discussion's closed thread records that "real variants concentrate in slash commands and hooks, which the mirror does not republish". The semantic gap a translation must close: a command has no required `description` and is user-invoked by default, while a skill is model-invocable unless it says otherwise. A follow-up issue proposes translation.
- **Other plugin components** (`hooks/`, `.mcp.json`, `mcpServers`, `settings.json`, `outputStyles`, `lspServers`, `monitors`, `bin/`): skipped, reason named per component.
- **Plugin trees:** a plugin is read only as a table of contents (owner ruling, Translation). Each plugin becomes its admitted member artifacts plus one bundle; the plugin tree and its manifest are never published. A `.claude-plugin` entry at a unit's root is dropped from staging with a warning (fact 1); grim's `skill-plugin-manifest` refusal stays the backstop and surfaces as `refused` if anything slips.
- **Exact rules** live in the design: marketplace selection, `strict`, add-or-replace per component key and `metadata.pluginRoot` in § 5.3; bundle membership (only `published` or verified `unchanged` members; a `failed` member fails the bundle), bundle version from all member commits, `bundle-drift` and check mode in § 5.9.
- **Partial bundle:** a bundle publishes with its admitted members; every skipped or refused member is a report row with `bundle` set. A bundle with no admitted member is `refused` `empty-bundle`.

### D9. Dialect translation: `grim convert` (Unstable)

Criteria: **1** one source of truth with the lift registry (4), **H** a human converting their own skill gets the same result (2), **R** structured per-unit report for the mirror (3), **F** frozen surface (2).

| Option | 1 | H | R | F | Total |
|---|---|---|---|---|---|
| Copy of the registry in the mirror | 0 | 0 | 3 | 3 | 15 |
| `publish.toml` per-entry `dialect` applied at pack time | 3 | 1 (no file on disk; dev installs untranslated) | 1 (publish report grows translation rows) | 2 | 23 |
| `grim build --dialect` | 3 | 1 | 1 | 1 | 20 |
| **`grim convert <path> --from claude --out <dir>`, Unstable** | 3 | 3 | 3 | 2 | **31** |

**Chosen `grim convert`**, the 27th subcommand, listed under `stability.md` § Unstable until a later ADR stabilises it. The inverse is computed from the same `KnownField` tables (`src/install/vendor.rs:685`, `CLAUDE_SKILL_FIELDS`, `CLAUDE_AGENT_FIELDS`, the empty Claude rule registry), so a field added to the lift is inverted with no second edit. Rules (exact in spec § 5):

1. Common fields stay top level: skill `name`, `description`, `license`, `compatibility`, `allowed-tools`, `metadata`; agent `name`, `description`, `model`, `tools`, `license`, `metadata`; rule `paths`, `license`, `metadata` (agent and rule frontmatter keep unknown keys verbatim, and a licence statement is never dropped). A registry row whose native key is common (`allowed-tools`, agent `model`/`tools`) is not inverted.
2. A top-level key equal to a row's `native` moves to `metadata."claude.<field>"`, stringified by the row's `FieldType` so that the install-time lift yields a semantically equal value (a `CommaList` sequence becomes a comma string and lifts back to the same items); a value of the wrong YAML type is dropped (`unrepresentable`).
3. An existing `metadata."claude.<field>"` wins over the top-level key (`shadowed`), as the override escape hatch does at install.
4. Any other top-level key is dropped (`unknown-field`); no un-namespaced `metadata` key is invented, since grim reads several (`repository`, `summary`, `keywords`) and would change meaning.
5. Bodies are copied byte for byte; support files of a skill are copied as they are.

`--from` accepts `claude` only; `agentskills` input needs no conversion and the mirror skips the call. Further dialects are appended.

### D10. Bodies that name one harness (question 11)

**Publish and report**, as recommended: refusing would drop most Claude-authored skills. `grim convert` reports each occurrence of a closed token list (`${CLAUDE_PLUGIN_ROOT}`, `$CLAUDE_PLUGIN_ROOT`, `${CLAUDE_PROJECT_DIR}`, `$CLAUDE_PROJECT_DIR`, `$ARGUMENTS`) with file and line; the list lives beside the Claude registry and is re-verified upstream by the implementer (`.agents/upstream-watchlist.md` row). Prose naming a harness's tools is not detected, a stated limit. The mirror copies each warning to the unit's report row; warnings never change the exit code.

### D11. Licence: detection, allowlist, precedence

Criteria: **C** no false "allowed" (4), **D** dependency cost (2), **E** SPDX expressions (2), **O** OSI flag without a hand-kept list (2).

| Option | C | D | E | O | Total |
|---|---|---|---|---|---|
| `askalono` or `spdx` `detection` (full text DB) | 3 | 0 | 3 | 3 | 24 |
| Hand parser, hand allowlist, built-in matcher | 2 | 3 | 1 | 0 | 16 |
| **`spdx` crate (expressions, `is_osi_approved`) + built-in normalised-text matcher for 15 common licence texts** | 3 | 2 | 3 | 3 | **28** |

**Chosen.** `spdx` (EmbarkStudios, the parser `cargo-deny` uses) in `grim-mirror` only, default features, no `text` or `detection`. Reason for the one new dependency: it supplies both the expression grammar (`OR`, `AND`, `WITH`, `+`, parentheses) and the OSI flag the default allowlist needs, which a hand list would let drift.

- **Allowlist:** default = every SPDX licence id with `is_osi_approved()`. `[mirror] licenses = [...]` replaces it with a list of SPDX ids, any id the SPDX list holds (non-OSI ones warn once), so an operator can admit `CC0-1.0` (awesome-cursorrules, `_repo_survey.md` #13). `LicenseRef-*` is never allowed. An expression is allowed when it evaluates true with allowed ids true (`OR` any, `AND` all, `WITH` and `+` judged on the base id).
- **Evidence per level is tri-state:** absent, recognised, unrecognised. Fields (`SKILL.md` or agent/rule frontmatter `license`; `plugin.json` at plugin level; `package.json` at repository level) count when they parse as an SPDX expression in `spdx`'s lax mode (real frontmatter carries `GPL-3.0`, `GPL-2.0+`); an unparseable value (`Complete terms in LICENSE.txt`) is no evidence. A licence file (`LICENSE*`, `LICENCE*`, `COPYING*`) is recognised by an `SPDX-License-Identifier:` line or by a unique template match, else **unrecognised** — and an unrecognised file makes its level unrecognised.
- **Matcher:** header-only stripping of copyright and title lines, then a template must cover ≥ 0.97 of its own word bigrams with at most `max(4, 2%)` foreign bigrams, and exactly one template may match (spec § 5.6). So MIT plus a non-commercial clause, BSD-3 against BSD-2 and MIT against MIT-0 do not match. GPL-family texts never decide `-only` vs `-or-later`: a text match alone is unrecognised unless a field or SPDX line at the same level names the id. A golden corpus with must-not-match variants pins this.
- **Precedence:** unit level (a skill's directory; a single-file unit's frontmatter only) → plugin root → repository root; the first non-absent level decides. Recognised: all its sources must agree as canonical expressions, else `refused` `license-conflict`. Unrecognised: `refused` `license-unrecognised` unless `unit-licenses` declares that unit, recorded `declared` — the owner's "a repository can state its licence if it cannot be derived", made per unit because the operator has then read that file. All absent: `unit-licenses`, then the entry `license`, recorded `declared`; else `refused` `license-missing`. A declaration beside recognised evidence is ignored with a warning; a recognised licence not allowed is `refused` `license-not-allowed` whatever is declared.
- **Notice carriage:** MIT- and BSD-style licences require the notice to travel. When the deciding file sits at plugin or repository level, a skill without its own licence file gets a copy as `LICENSE`; a single-file agent or rule, which cannot carry a second file, carries it in the per-package README companion `publish.toml` already supports (`<name>.README.md`). Residual: the installed agent or rule file holds no licence text; the notice is in its repository's companion. Appending the text to the file was not taken: it changes the prompt.
- **Recording:** the canonical expression goes to `mirror.license`, the origin to `mirror.license-source` (`detected`/`declared`). A bundle has no upstream content of its own; its `license` is each member's expression parenthesised, deduplicated and joined with ` AND `, `declared` if any member is.

### D12. Fetch and staging: no upstream code runs

`grim-mirror` shells out to `git`, with the hardening of `src/catalog/git_cmd.rs:58-88` **copied** (no lib split: `-c protocol.ext.allow=never`, `-c http.followRedirects=initial`, pinned `GIT_ALLOW_PROTOCOL=file:http:https:ssh:git`, `GIT_TERMINAL_PROMPT=0`, empty `GIT_ASKPASS`, low-speed limits, null stdin, `kill_on_drop`, a deadline per call), with a unit test pinning the argument and env set so the copy cannot silently lose a line. Clone: `--filter=blob:none --no-checkout --no-local`, full history (subtree commits and reachable tags need it). Staging never checks out: entries come from `git ls-tree -r -z` and blobs from `git cat-file`, so no filter driver, hook or attribute runs; `100755` stays executable in staging (grim's packer then writes `0o644`, as for a native publish). `ref` resolves only through `refs/remotes/origin/<ref>` or `refs/tags/<ref>`, remote HEAD is refreshed on every fetch, and every upstream-derived path is contained before it is joined (`refused` `unsafe-path`). A symlink or submodule entry in a unit refuses it (`unsupported-entry`); a Git LFS pointer, or a unit whose directory has no kind manifest but a template source (`*.src.md`, `*.tmpl`, `*.template`, `*.j2`, `*.hbs`, `*.njk`), refuses it as `unsupported-build-step` (the owner's "installable as committed" rule). Options not taken: `gix` or `git2` (a new dependency where grim already relies on `git`); a checkout (runs filter drivers configured on the CI host).

### D13. Publishing through grim

The mirror resolves grim as `GRIM_BIN` when set and absolute, else `grim` on `PATH`; it probes grim's capabilities before any work instead of comparing versions (plan decision, 2026-11-24: a branch build and the last release both report 0.16.1, and the version bump is a release act): the `grim schema --kind publish` document must define a `mirror` property on the package entry, and, when any entry uses `dialect = "claude"`, `grim convert --help` must exit 0; else exit 69 naming the missing capability. It writes one `publish.toml` per group — (upstream, plugin or none, version, content commit), where the content commit is the tag's commit in tag mode and the unit's own revision in snapshot mode, the commit its files and licence are read at — in a fresh staging tree and runs `grim publish --format json` (`--dry-run` in check mode and for the output-drift check) with `SOURCE_DATE_EPOCH` = that content commit's time and `GIT_CEILING_DIRECTORIES` = the staging root's parent, so `created` is deterministic and no enclosing repository's HEAD is stamped. Bundles run after their members, with members as absolute exact-version references. It retries a `grim publish` or `grim describe` exit 75 once (ocx retries only 75); every other outcome follows the spec's failure matrix (§ 7.1), in which every failure maps to a non-zero exit. One writer per target prefix is assumed (the docs example sets a CI concurrency group); a `Skipped` item is re-read and its revision compared before it reports `unchanged`, which is how a second writer shows. Bundles reference only members that are `published` or verified `unchanged` this run; in check mode a bundle whose members would only be published is reported from the plan without running grim.

### D14. Reports, statuses, exit codes, one exit-code table

Per unit: `published`, `would-publish` (check), `unchanged`, `refused` (with a closed `reason`), `skipped` (a member kind or component the mirror does not republish), `failed` (infrastructure). Process exit: 64 usage, 78 invalid mirror file, 79 mirror file not found, then — after every upstream ran — the first infrastructure code met (69, 74, 75, 80, 81) if any; then, under `--fail-on refused`, 65 if any unit is `refused`; else 0. `--fail-on` defaults to `refused` for `check` and `infra` for `sync`. Reason: a standing refusal is a list defect that `check` on the list repository's pull requests must catch, while a scheduled `sync` that turned permanently red on one refused unit would hide the next infrastructure failure. `skipped` never changes the exit.

**One enum per workspace** without a dependency on the bin crate: `grim-mirror` defines its own `ExitCode` with the subset it uses, and a unit test reads `src/cli/exit_code.rs` through `include_str!` and asserts each variant equals grim's of the same name — the launcher precedent (fact 7), with no path dependency to undo at the split.

### D15. Takedown and native-publish handover

An entry gains `retire = { reason = "...", replaced-by = "<ref>", units = [...] }` (both optional). `units` lists target-repository suffixes (`skills/old-tool`) for units no longer enumerable upstream, which the mirror cannot otherwise find. On the next run, for each unit the mirror re-stages its highest published version from that version's recorded `revision` and `path` **with the current converter** — a new version, so payload drift from the old one is acceptable and stated — and publishes `X.Y.(Z+1)` with `mirror.deprecated` and `mirror.replaced-by` set: floats move onto the deprecation notice, every exact tag and every lock keeps resolving, `grim status --check` shows `deprecated`/`replaced_by`. After that the entry is removed from the list. Options not taken: republish the old version with `--force` (breaks pins); a stub tombstone (changes content under users' floats); delete repositories (the only answer to a takedown that demands removal — the operator deletes the registry repositories with registry tooling, documented as a runbook; grim has no delete command and the mirror adds none). When the upstream is gone the retire run is `refused` `upstream-unavailable` and the runbook applies.

### D16. Namespace and target repository template

Artifacts land under the operator's registry and prefix, never a name imitating the upstream author (discussion ruling). Fixed template, not configurable: unit `{registry}/{upstream}/{kind-subdir}/{name}`, marketplace member `{registry}/{upstream}/{plugin}/{kind-subdir}/{name}`, bundle `{registry}/{upstream}/bundles/{bundle}`; `{registry}` is `[mirror] registry` (`host[/prefix]`), `{upstream}` the entry name. Two units of one kind with one name in one entry are both `refused` `name-collision`. Unit names follow grim's name grammar; a name that fails it is `refused` `invalid-name`. Announcing to an index is out of the first version (follow-up issue); the operator can announce mirrored repositories by hand with their own `publish.toml`.

### D17. Running the curated list

As recommended: the curated `mirror.toml` and its CI job live in a list repository the owner runs (follow-up issue, outside this run's grants). This repository ships the tool, `crates/grim-mirror/examples/mirror.toml`, and a docs page with a GitHub Actions example: `grim-mirror check` on pull requests (resolve, licence and kind gate, convert, `grim publish --dry-run`, publish nothing) and `grim-mirror sync` on a schedule on the default branch. A re-run with nothing new publishes nothing (D6). No workflow generator (ocx has one; a pasted example is enough for one list). The example installs a pinned commit or tag (`cargo install --locked --git … --rev …`) and sets a `concurrency` group per target; the report names both the `grim` and `grim-mirror` versions, so a run is reproducible.

### D18. Trust ADR and threat-model amendments

Applied on acceptance (§ Amendments): the operator is the **publisher of record** for a mirrored artifact, so to a consumer the operator is T1; grim's integrity guarantee covers the bytes the mirror pushed, pinned by digest, and says nothing about upstream authorship. The mirror's own controls are scoped to T1/T2 on the upstream side: pinned commit per publish, `revision` recorded, a moved tag refused (D6), no upstream code run (D12), executable kinds not republished (D8). PATH dispatch adds no attacker capability (D2). No new gate in grim.

### D19. Stability, reversibility and rollout

| Surface | Status at release | Reversal cost |
|---|---|---|
| Dispatch rule (D2), flag refusal and forwarding (D3) | Frozen, CLI surface row | Narrowing it breaks installed plugins |
| `GRIM_BIN` | Frozen, environment row | Removal breaks plugins that read it |
| `mirror` table keys, the three annotations, their precedence (D4) | Frozen, `publish.toml` and OCI wire rows | Additive only from here |
| Snapshot grammar (D5) | Unstable as mirror behaviour, but published tags are permanent | A new grammar must order above every published one |
| `grim convert` (D9, D10) | Unstable | Free until stabilised |
| The plugin name space | Frozen by D2's built-ins-win rule | Every new built-in name shadows a plugin of that name: `mirror` is a permanent reservation, and any future built-in name needs a plugin-collision review before it ships |
| `grim-mirror` everything (D6–D8, D11–D17) | Unstable sibling surface | Free until the split |

## Consequences

**Positive:** Done 1–4 met by one tool over one source primitive (a git path at a pinned commit); grim gains a general plugin seam, a reusable Claude→general converter, and machine-readable mirror provenance; nothing changes for any existing input.

**Negative:** three frozen annotation keys and one frozen table; a 27th subcommand; one new crate dependency (`spdx`); a copied git hardening helper to keep in step until the split; tag mode re-versions every unit per tag; a mode switch back to tags can leave an `M.m` float on a snapshot.

**Risks:** a licence text matcher misreads a modified licence as standard (mitigation: coverage and foreign-bigram bounds, a unique match, unrecognised refuses, a golden corpus of must-not-match variants, the operator is publisher of record); a future grim built-in shadows a plugin name (mitigation: `mirror` reserved, git-style precedence documented); operators widen the allowlist to non-redistributable ids (their responsibility as publisher of record; a warning names each non-OSI id).

## Amendments to existing records (applied on acceptance, not by this ADR)

1. `adr_artifact_trust_model.md`, after decision 4: "**Mirrored artifacts.** An artifact carrying `com.grimoire.mirror.source` was republished by the operator who pushed it, who is its publisher of record; decisions 1–4 apply to it unchanged. Integrity covers the mirror's bytes, not upstream authorship. The mirror's refusal of hook, MCP, settings and mod content is the mirror's product policy, not a gate on a kind (`adr_artifact_mirroring.md` D8)."
2. `.claude/rules/arch-threat-model.md` § What grim is, append: "Since `adr_artifact_mirroring.md`, an unknown subcommand may run `grim-<name>` from absolute `PATH` entries; whoever controls `PATH` already chose which `grim` runs, so this adds no capability (T3 unchanged)." § Applying it gains: "A mirrored artifact's T1 is the operator; upstream T2 is answered by the mirror's recorded `revision` and its moved-tag refusal."
3. `docs/src/content/docs/stability.md`: CLI surface row gains plugin dispatch (D2, D3); environment row gains `GRIM_BIN`; `publish.toml` row gains the `mirror` table; OCI wire row gains the three `com.grimoire.mirror.*` keys; § Unstable gains `grim convert` and the `grim-mirror` sibling surface by name; "Four exceptions" becomes "Five exceptions".
4. `AGENTS.md`: "26 subcommands" → 27; env table gains `GRIM_BIN`; Architecture names `crates/grim-mirror`. Same edits in `.claude/rules/product-context.md`, `.claude/rules/arch-principles.md` § Crate Layout, `.claude/rules/subsystem-cli-commands.md` (rows for `grim convert` and plugin dispatch).
5. Docs pages: `commands.md` (dispatch, `convert`), `json-interface.md` (convert report), `publishing.md` (`mirror` table, precedence), `artifacts.md` (annotation list), environment reference (`GRIM_BIN`), new `mirroring.md` (how-to, with the example job and the takedown runbook), `changelog.md`. Catalog drift review of `grim-usage` and `grim-authoring` (`grim convert`).

## Revisit triggers

- The owner creates the mirror repository: run the split (follow-up issue), revisit D1 shipping.
- Signatures arrive (`adr_artifact_trust_model.md` § When signatures arrive): revisit D8's refused kinds and D18.
- A consumer asks for a curated `mirror` field on `describe` or `search`: add it under consumer release order.
- Claude changes command or plugin component semantics, or a second dialect is requested: revisit D8 commands and D9.

## Deferred (follow-up issues)

Repository split; curated list repository and its job; `--announce` support in the mirror; command → skill translation; indexer "mirrored" badge from the verbatim annotation; `grim help <plugin>` rewriting; npm, archive and command marketplace sources (declined by ruling).

## Links

- `.agents/discussions/artifact-mirroring.md`, `.agents/goals/artifact-mirroring.md`, `.agents/specs/design_artifact_mirroring.md`
- `adr_artifact_trust_model.md`, `adr_repository_annotation.md`, `adr_default_provenance_and_support_channels.md`, `adr_tool_namespaced_metadata_rendering.md`, `adr_unified_publish_version_cascade.md`, `adr_dependency_version_claims.md`, `adr_mod_artifact_kind.md` (D15 refusal)
- ocx: `ocx/.claude/artifacts/adr_cli_plugin_pattern.md`, `adr_cli_repo_split_template.md`; `ocx-mirror/docs/reference/mirror-yml.md`

---

## Changelog

| Date | Author | Change |
|---|---|---|
| 2026-11-24 | Claude (architect, `/hex-plan` xhigh) | Initial draft, **Proposed**. Eleven open questions and the orchestrator's repository-scope decision settled as D1–D19. |
| 2026-11-24 | Claude (`/hex-plan` orchestrator) | D13: grim capability probe replaces the `MIN_GRIM` version compare (plan § Decisions, PD-3). |
| 2026-11-24 | Claude (architect, review round 1) | B1–B10, W1–W14 applied: reconciliation split from selection, probes without `latest`, tags bounded by `ref`, snapshot-shaped watermark, output drift by dry-run digest, bundle drift and member rules, verified Claude marketplace rules, tri-state licence evidence and stricter matcher, `retire.units`, one writer, content-commit groups, notice carriage, git hardening details, convert invariant and `license` kept, failure matrix, forwarding function, `--fail-on`, Windows exit and Ctrl-C, plugin name reservation, pinned install. |
