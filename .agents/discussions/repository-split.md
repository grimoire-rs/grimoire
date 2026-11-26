# Discussion: Repository split for the mirror tool and a GUI application

State: handed-off → loop · Updated: 2026-11-25
Ratified: 2026-11-25 → loop
Confidence: ratified by the owner in session with an explicit yes on the third restate. Decisions rest on six research files dated 2026-11-25 (`.agents/research/research_repository_split_*.md`). Entries under "Rulings of the orchestrator" were not confirmed one by one.

## Intent

The owner, in their own words (voice input, lightly corrected): "We have a good
state now, but we need to split the repositories now. That is very important.
And not only for this mirror application, I also want a GUI application, but
the base should be the same as this repository."

Why now: the mirror tool is finished and sits in the open pull request
`grimoire-rs/grimoire#398` as the workspace crate `crates/grim-mirror`. The
owner wants it in its own repository before it lands in grimoire, and wants a
second new repository for a GUI application built on the same base.

Out of scope:

- The GUI itself: screens, navigation, visual design, features. This work
  stops at a skeleton and another agent builds the interface.
- Packaging, signing, auto-update and a release pipeline for the GUI.
- A release of `grim-mirror` and the items listed in
  `grimoire-rs/grimoire#402` other than the repository split (curated list
  repository, sync job, `--announce`, command translation, indexer badge,
  `grim help <plugin>`).
- A library API of grim, and a split of grim's crate into library and binary.
- Growing the grim SDK beyond what the mirror calls today, its TypeScript
  half, and publishing it to crates.io.
- A shared conventions repository for the grimoire organisation, and bringing
  the grimoire repository onto `ocx-sh/conventions`.
- Bringing `grimoire-rs/vscode` and `grimoire-rs/indexer` onto the same base.
- A Windows build of `grim-mirror`, which it does not have today.

## Requirements

- The mirror tool lives in its own repository, `grimoire-rs/grim-mirror`, and
  never lands on grimoire's `main`.
- The pull request `grimoire-rs/grimoire#398` is rewritten without the crate
  and stays one pull request. It keeps plugin dispatch, the `mirror` table on
  a publish manifest entry with its three `com.grimoire.mirror.*` annotations,
  and `grim convert`, each with its tests and documentation.
- A GUI skeleton lives in its own repository, `grimoire-rs/grim-gui`: a Tauri 2
  application whose binary is named `grim-gui`, so that `grim gui` starts it
  through plugin dispatch.
- Both new repositories talk to grim only through the `grim` binary: its CLI,
  its `--format json` output, plugin dispatch and `GRIM_BIN`. Nothing links
  grim's code.
- A minimal grim SDK crate, `crates/grim-sdk`, lands in the grimoire
  repository inside `grimoire-rs/grimoire#398`. It is a Rust client that only
  invokes the binary: find grim (`GRIM_BIN`, then `PATH`), run it with
  `--format json`, check the version floor, map exit codes. Its content is the
  grim-calling code the mirror has today, moved, not newly designed. It has no
  dependency on the main crate, its own version line starting at `0.1.0`, is
  marked Unstable in `docs/src/content/docs/stability.md`, and is not
  published to crates.io in this run.
- `grim-mirror` and the `grim-gui` shell depend on `grim-sdk` as a git
  dependency pinned by commit.
- Both new repositories have the same scaffold as grimoire, taken by copy: the
  taskfile layout, the verify workflows, the Claude hooks and settings, the
  Rust configuration files, the licence, DCO and contribution files, and the
  test harness modules they need.
- Agent rules and skills are shared through grim itself: each new repository
  installs them from bundles through its own `grimoire.toml` and
  `grimoire.lock`. No rule or skill is copied by hand where a bundle carries
  it.
- Each new repository carries a manifest file that lists every copied file
  with the grimoire commit it was copied from, so drift can be measured later.
  The manifest also accounts for every row of the two inventories
  (`.agents/research/research_repository_split_agent_config.md` and
  `.agents/research/research_repository_split_testing_and_conventions.md`):
  taken as is, rewritten on the same pattern, or not applicable with a reason.
  A row with no entry is a defect. This is how "adopted in full" is checked.
- Adapted, not blindly copied: the commit guard comes with the root task that
  writes its stamp; the structural tests under `.claude/tests/` and the
  reminder tables in `.claude/hooks/post_tool_use_tracker.py` are rewritten
  for each repository's own layout and pass there.
- The mirror repository holds the crate and its ten acceptance suites, with a
  passing gate, before the crate is removed from the pull request's branch.
- The GUI skeleton keeps its front end in plain TypeScript without a UI
  framework and passes grim's JSON to the front end unchanged, so the later
  agent can lift code from the VS Code extension.
- The agentic configuration is adopted in full in both new repositories
  (owner, 2026-11-25: "make definitely sure the agentic config is adopted -
  also what is not part of grim, i.e. the subsystem idea and files"). This
  covers what no bundle carries: `AGENTS.md` with its structure, the rule
  catalog `.claude/rules.md`, the first-party rules, the path-scoped
  `subsystem-*.md` idea with new subsystem rules written for each repository,
  the hooks, `.claude/settings.json`, the structural tests under
  `.claude/tests/`, the templates, the first-party skills and the `.agents/`
  layout. Generic items are taken as they are; items that are grimoire content
  are rewritten for the new repository on the same pattern.
- The entire testing idea comes along (owner, 2026-11-25: "we should take the
  entire testing idea with us"): the layers, the pytest acceptance harness
  with its registry and runner, the gate (`task verify` with its stamp), the
  skip ceiling and the basic and deep CI tiers. The GUI skeleton gets the same
  idea applied to a Tauri application, not a copy of grim's suites.
- Other conventions of the same kind come along too (owner, 2026-11-25:
  "search for other similar requirements"). The inventories under Research
  list them; each item is taken, rewritten on the same pattern, or named as
  not applicable with a reason.

## Decisions

- **Split, not monorepo** (owner, 2026-11-25, confirmed after weighing a
  monorepo with a joint release: "Okay, then we should split it").
- **Base means scaffold, not code** (owner, 2026-11-25). The alternatives were
  a shared Rust library crate, or scaffold plus library. Grim has no library
  API and is freezing for 1.0.
- **The mirror crate is cut out of `grimoire-rs/grimoire#398` before merge**
  (owner, 2026-11-25, against the orchestrator's recommendation to merge first
  and extract afterwards). Consequences:
  - The nine-commit series is rewritten: the crate commit goes, and the mirror
    parts of the test, workflow, documentation, catalog and agent-rule commits
    go with it.
  - The rewritten branch needs a fresh release gate, a fresh `verify-deep.yml`
    run and a new force-push grant. The earlier grant was scoped to the
    finished `/hex-finalize` run.
  - The pre-rewrite tip is `eaa7da0b`; the local ref
    `backup/hex/artifact-mirroring-828f4d61` holds the tip before that.
- **This discussion settles both new repositories** (owner, 2026-11-25) and
  hands them to one goal loop.
- **The GUI application is for everyday use** (owner, 2026-11-25): browse and
  search the index, install, update, see status and drift. This is the scope
  of grim's TUI, so the JSON output it needs exists and is frozen.
- **The GUI application is a desktop binary named `grim-gui`** (owner,
  2026-11-25), started directly or as `grim gui`.
- **The GUI application uses Tauri 2** (owner, 2026-11-25). This is a named
  deviation from `.claude/rules/product-tech-strategy.md`, which lists Rust
  only: the shell is Rust, the front end is web technology.
- **This work delivers a GUI skeleton, not the GUI** (owner, 2026-11-25: "if
  there is a skeleton, I will invoke another agent to create the graphical
  user interface").
- **`grim-gui` invokes the grim binary** (owner, 2026-11-25). Compiling grim in
  was weighed twice, the second time for a monorepo with a joint release. A
  monorepo removes the API freeze, because an internal library is no public
  contract. It does not remove the second-grim problem: publishing together is
  not installing together, so a terminal grim and a GUI with another
  compiled-in grim would write the same `$GRIM_HOME`, lock and install-state
  files. It also makes grim's release wait on a desktop application. Shipping
  grim inside the GUI package as a Tauri sidecar stays possible later.
- **The base is copied from grimoire; rules and skills come through grim
  bundles** (owner, 2026-11-25: "Copy from grimoire is fine, but we should
  make sure to use grimoire itself to share agent skills and so forth").
  `ocx-sh/conventions` was weighed and not chosen now: it is an ocx tool, task
  and CI baseline with no Rust configuration, hooks, test harness or release
  flow, it runs every task through `ocx`, and the grimoire repository itself
  is not adopted and would fail its check.
- **No conventions repository for the grimoire organisation now.** The owner
  raised it and called it possibly too much; the orchestrator agrees. The
  manifest of copied files is the cheap stand-in. Revisit when a third
  repository adopts the base or the first measured drift costs a fix in more
  than one repository.
- **A minimal grim SDK is built now, in the grimoire repository** (owner,
  2026-11-25, against the orchestrator's recommendation to extract it after
  this run). The owner had asked how code is shared: grimoire as a submodule,
  a carved-out core library ("Would not do that to be honest"), or "one shared
  library set just invoking grim via Rust, like some kind of grim SDK ... a
  layer outside of grim itself", and then whether that crate could be part of
  grimoire. It can, as a crate that only invokes the binary; exposing grim's
  internals stays rejected. What this buys: client and contract change in one
  commit, CI tests the client against the grim built in the same run, and the
  exit-code check keeps reading grim's source. What it costs: the API is
  shaped by one real caller, the mirror, and the pull request and this run
  grow.
- **Handoff is a goal loop** (owner, 2026-11-25).
- **The loop may create the two new repositories and push to them freely**
  (owner, 2026-11-25, verbatim: "important to say you are obviously free to
  ceate and push as you like into two new repos"). This covers
  `grimoire-rs/grim-mirror` and `grimoire-rs/grim-gui` only. It does not cover
  the force-push that rewrites the branch of `grimoire-rs/grimoire#398`, the
  transfer of issues, or any merge. A goal loop accepts only grants it can
  quote from the owner's own paste, so the paste that starts the loop must
  repeat this sentence and name the force-push.

Rulings of the orchestrator on details, each open to the owner's veto:

- **No history rewrite tool for the move.** After the recompose the crate is
  one commit (`f71eb216`) and its tests one more (`4e420be8`), so there is no
  history worth carrying, and `git filter-repo` would drop the signatures. The
  new repository starts with fresh, signed, signed-off commits that name
  `grimoire-rs/grimoire` at `eaa7da0b` as their source. The alternative,
  `git filter-repo --path crates/grim-mirror` as the ADR sketches, would keep
  two unsigned commits.
- **No grim submodule in the mirror repository.** ocx's mirror vendors ocx as
  a submodule and links its crates, and its recorded friction comes from that
  link. The exit-code guard (`crates/grim-mirror/src/exit_code.rs:34` reading
  `src/cli/exit_code.rs`) moves into `crates/grim-sdk` with the exit-code
  table and stays a source-level check. The second guard
  (`crates/grim-mirror/src/git.rs:1019` reading `src/catalog/git_cmd.rs`)
  concerns how the mirror calls git, not grim; it becomes a copied snapshot
  in the mirror repository with the source commit in the manifest.
  `crates/grim-mirror/tests/boundary.rs` is replaced by two checks: in
  grimoire, that `grim-sdk` does not depend on the main crate; in the mirror
  repository, that no grim crate other than `grim-sdk` is a dependency.
- **The SDK pin follows the pull request.** Until
  `grimoire-rs/grimoire#398` merges, both new repositories pin `grim-sdk` and
  the grim they build to the pull request's head commit. A rebase merge
  creates new commit ids on `main`, so both pins are moved to `main` in a
  small follow-up change in each repository after the owner merges. The
  alternative, waiting for the merge before the new repositories start, would
  stall the split on a review.
- **The mirror repository builds grim from a pinned commit in CI** until a
  release carries plugin dispatch, the `mirror` table and `grim convert`; then
  it pins a released version and checks a version floor at run time.
  `grimoire-rs/vscode` and `grimoire-rs/indexer` already work this way. Until
  the pull request merges, the pin is the pull request's head commit.
- **Mirroring documentation moves with the tool.** The user guide for
  mirroring goes to the mirror repository. Grimoire's documentation keeps
  plugin dispatch, the `mirror` table and `grim convert`, and links to the
  mirror repository. `.claude/rules/subsystem-mirror.md` moves; the other rule
  files, the catalog skills and `AGENTS.md` lose their crate lines.
- Withdrawn: the orchestrator had ruled that rules existing only in
  grimoire's `.claude/rules/` are not copied. The owner overruled it on
  2026-11-25; see the agentic-configuration requirement. Bundles stay the
  first source (`hex` and `nox` from `michael-herwig/arcana`, the language,
  documentation and comment bundles from `ocx-sh/lore`); everything no bundle
  carries is copied or rewritten and listed in the manifest.
- **The GUI skeleton proves the bridge with one call**: a Rust command asks `grim-sdk` for the version of the grim it finds
  (`GRIM_BIN`, then `PATH`), and
  one page shows the result or a "grim not found" message. The front end is
  plain TypeScript on Vite. CI builds it unsigned on Linux, macOS and Windows.

## Threads

- Closed: where the shared scaffold lives. Copy with a manifest, bundles for
  rules and skills. See Decisions.
- Closed: invoke or compile in. Invoke. See Decisions.
- Closed: the GUI's purpose, delivery, stack and the skeleton boundary.
- Closed: how the mirror repository pins grim, and what replaces the source
  level guards. See the rulings.
- Closed, as input for the later GUI agent: reuse of the VS Code extension.
  The owner asked whether parts of it can be reused in the Tauri application.
  It is reasonable. In `grimoire-rs/vscode` at `9230dcd`, the interface is
  already web code in webviews: `src/webview/model.ts`, `render.ts`,
  `protocol.ts`, `sidebar/main.ts` and `settings/main.ts` are about 8,300
  lines with their own tests, and `src/grim.ts` (1,412 lines) types grim's
  JSON output. 30 of 79 source files import the `vscode` API; that host side
  (`src/extension.ts`, `src/views/*.ts`) is not reusable. The process spawn is
  not reusable either, because Tauri spawns on the Rust side. How the code is
  shared (a package both consume, or a copy) is the GUI agent's first
  decision. Not checked: how far the webview code depends on VS Code's theme
  variables and on `acquireVsCodeApi`.
- Closed: how code is shared between the repositories. A minimal `grim-sdk`
  crate in the grimoire repository, built now. See Decisions. Left for later:
  the TypeScript half, because `grimoire-rs/vscode` (`src/grim.ts`,
  hand-written), `grimoire-rs/indexer` and the GUI front end all need typed
  grim JSON; growing the Rust API when the GUI has real calls; publishing.
  ocx plans generated SDKs under its interface-contract ADR of 2026-10-03.

## Research

- `.agents/research/research_repository_split_recon.md`: the crate's coupling
  to the workspace, the size and kind of the non-product base, how
  `grimoire-rs/vscode` and `grimoire-rs/indexer` carry it (by copy, drifted),
  and what the existing records say. Decisive facts: the crate inherits only
  `version`, `edition`, `license` and `[lints]`; ten suites (71 tests) use
  `test/src/git_fixture.py`, `helpers.py`, `registry.py` and the zot registry;
  the crate is named in three workflows, `test/taskfile.yml`,
  `test/conftest.py`, six documentation pages, two catalog skills, ten
  `.claude` rule files and `AGENTS.md`; the mirror has no Windows build.
- `.agents/research/research_repository_split_prior_art.md`: mechanisms for
  sharing a base across repositories, how a GUI consumes a CLI core, plugin
  repositories, extraction with history, Rust GUI options. Decisive facts:
  `git filter-repo` drops commit signatures and needs a fresh clone; template
  repositories do not propagate updates; cargo versions its JSON output and
  discourages linking its crate.
- `.agents/research/research_repository_split_ocx.md`: what ocx did. Decisive
  facts: the mirror moved out on 2026-06-12 under
  `adr_cli_repo_split_template.md`; it links ocx crates through a submodule,
  and a crate deletion broke 493 references in 138 files; rules and skills
  reach it through grim bundles; 200 of 1,426 shared paths still drifted,
  mostly files outside the template.
- `.agents/research/research_repository_split_conventions_template.md`: what
  `ocx-sh/conventions` renders, how updates reach adopters, and how it
  compares with grimoire's scaffold. Decisive facts: no Rust configuration,
  hooks, test harness or release flow; `ocx.toml` with exact pins is
  mandatory; a template-owned `cliff.toml` hard-codes `ocx-sh`; six grimoire
  rows are listed as not adopted with no recorded reason.
- `.agents/research/research_repository_split_agent_config.md`: inventory of
  the first-party agent configuration, each item labelled take as is (about
  54), rewrite on the same pattern (65) or grimoire-only (33). Decisive facts:
  the subsystem idea has five parts (a path-scoped rule per code area, the
  path map in `AGENTS.md`, the catalog tables kept in step by tests, declared
  overlaps, and the `CONTEXT_REMINDERS` table in
  `.claude/hooks/post_tool_use_tracker.py`); the commit guard needs the root
  task `.verify:mark` in `taskfile.yml`, so `.claude/` copied alone blocks
  every commit; `.claude/tests/test_ai_config.py` hard-codes grimoire facts
  (vendored lists, forbidden type names, the count of global rules, the
  `AGENTS.md` budget); the reminder tables and `.claude/taskfile.yml` assume
  `src/`, `docs/`, `catalog/`, which a Tauri layout (`src-tauri/` plus a
  front-end `src/`) matches wrongly; `grimoire-rs/vscode` and
  `grimoire-rs/indexer` already share two rules that track grim facts,
  `artifact-kinds.md` and `grim-compat-markers.md`, which `grim-gui` needs;
  `pre_push_main_blocker.py` is tested but not wired in
  `.claude/settings.json`.
- `.agents/research/research_repository_split_testing_and_conventions.md`:
  inventory of the testing approach (74 rows: 18 take as is, 33 rewrite on the
  same pattern, 23 grimoire-only) and of the other repository conventions (80
  rows: 24, 36, 20). Decisive facts:
  - The mirror suites need `test/conftest.py` (registry lifecycle, the binary
    and runner fixtures, `grim_home`, `unique_repo`), two fixtures of
    `test/tests/conftest.py`, `test/src/zot.py`, `registry.py`, `runner.py`,
    `git_fixture.py`, two functions of `helpers.py`, a built `grim`, and zot
    and uv supplied through `ocx.toml`. They need nothing under `test/data/`.
  - `test/tests/conftest.py` imports `src.otlp_fixture` at module top, so a
    copy must be trimmed. `test/src/git_fixture.py:18` and
    `test/tests/test_mirror_bundles.py:19` read
    `crates/grim-mirror/licenses/MIT.txt` by repository-relative path.
  - Conventions a copy forgets most easily: `test/SUITE_FLOOR`,
    `test/SKIP_CEILING` and `test/XFAIL_CEILING` are enforced only by
    `accept_floor.py` in the CI merge job; an unreachable registry aborts the
    session; CI builds once, runs sharded acceptance, merges and publishes in
    a job with no checkout; actions are pinned by commit with minimal
    permissions; dependabot has one entry per composite-action directory; DCO
    and `cog.toml`; `--locked` with a committed `Cargo.lock`; the
    `.gitattributes` rules; `test_text_io_hygiene`.
  - For the GUI: `.licenserc.toml` covers only `*.rs`, dependabot needs an
    `npm` entry, and whether Tauri's generated code needs an exception to the
    workspace lint `unsafe_code = "forbid"` is unverified.
  - `ocx.toml` pins grim 0.14.3; no released grim carries the three
    contracts, which confirms the pinned-commit build.

## Related

- `.agents/discussions/artifact-mirroring.md`, `.agents/adr/adr_artifact_mirroring.md`,
  `.agents/plans/plan_artifact_mirroring.md`: the mirroring work this follows.
  The ADR is still `Proposed` and says the crate ships as a workspace crate;
  it needs a note that the crate moved before merge.
- `grimoire-rs/grimoire#398`: the open pull request that holds the crate.
- `grimoire-rs/grimoire#399`, `#400`, `#401`, `#402`: follow-up issues about
  the mirror tool, filed on the grimoire repository. They belong to the mirror
  repository once it exists.
- Sibling repositories: `grimoire-rs/vscode`, `grimoire-rs/indexer`.
- The owner's other project: `ocx-sh/ocx`, `ocx-sh/ocx-mirror`,
  `ocx-sh/conventions`, `ocx-sh/lore`.

## Open questions

- [NEEDS CLARIFICATION: With which visibility and settings are the two
  repositories created?] Recommended: public, like the sibling repositories,
  with the same ruleset on `main` as `grimoire-rs/grimoire` (no deletion, no
  non-fast-forward, pull request required, rebase only, one approving review)
  switched on after the first green gate. The owner granted creation and
  pushes; see Decisions.
- [NEEDS CLARIFICATION: Does the loop get the force-push to the branch of
  `grimoire-rs/grimoire#398` and a `verify-deep.yml` dispatch?] Recommended:
  yes, named in the paste that starts the loop, with lease, to that one
  branch.
- [NEEDS CLARIFICATION: Should the four follow-up issues move to the mirror
  repository?] Recommended: transfer `#399`, `#400` and `#401`, and keep
  `#402` on grimoire with its split item ticked. A transfer is an act on two
  repositories and needs a grant.
- [NEEDS CLARIFICATION: Does grim report the progress of a long install as
  JSON events a GUI can show live?] Recommended: the GUI agent checks this
  first. If grim does not, it is an additive change in grim, not a reason to
  link.
- [NEEDS CLARIFICATION: Should the generic grimoire-only rules become a
  bundle?] Recommended: not in this run. Publish `quality-core.md` and the
  `workflow-*.md` set as a bundle when a second repository asks for them.

- [NEEDS CLARIFICATION: Does Tauri's generated code need an exception to the
  lint `unsafe_code = "forbid"`?] Recommended: check on the skeleton's first
  build; if it does, scope the exception to the generated module and record
  it in the manifest as a deviation.
- [NEEDS CLARIFICATION: Do `artifact-kinds.md` and `grim-compat-markers.md`,
  the two rules `grimoire-rs/vscode` and `grimoire-rs/indexer` share to track
  grim facts, become a bundle?] Recommended: copy them into `grim-gui` with a
  manifest entry in this run; a bundle is a later step once three
  repositories carry them.
- [NEEDS CLARIFICATION: Does a cargo git dependency on the grimoire
  repository cost too much, given its two git submodules under `external/`?]
  Recommended: measure on the first mirror build. If the fetch is slow or
  fails, the fallback is publishing `grim-sdk` to crates.io, which is a
  release act and the owner's.

## Verification

- `grimoire-rs/grim-mirror`: its own full gate passes locally and in CI
  against a grim built from the pinned commit; the ten suites (71 tests) pass
  there; `grim mirror --help` reaches the binary through plugin dispatch; the
  manifest names every copied file and its source commit; `grim install` in
  the repository leaves `grim status` clean, which proves the rules and skills
  come from bundles.
- `grimoire-rs/grimoire#398`: `git grep grim-mirror` finds the crate nowhere
  under `crates/`, `Cargo.toml`, `.github/` and `test/`; `task --force verify`
  and `task docs:check` pass; `verify-deep.yml` is green on the new head; the
  pull request is ready and mergeable by rebase.
- `grimoire-rs/grim-gui`: the gate passes locally and CI builds the
  application on Linux, macOS and Windows; started as `grim gui` it shows the
  version of the grim that started it; with no grim reachable it shows the
  "grim not found" message; the manifest and the bundle check hold as above.
- No file under `src/` of grimoire gains a library target. `cargo tree` shows
  that `grim-sdk` has no dependency on the main crate and that each new
  repository depends on no grim crate other than `grim-sdk`.
- `grim-sdk` has its own unit tests and one acceptance check against the grim
  built in the same CI run; its exit-code table fails the build when
  `src/cli/exit_code.rs` changes without it.
- In each new repository: the manifest has an entry for every row of the two
  inventories; the structural tests of the agent configuration pass
  (`task claude:tests`); a commit attempted without a fresh gate is blocked
  and one after a green gate goes through; the suite floor and the skip and
  xfail ceilings are enforced in CI; an unreachable registry aborts the mirror
  repository's test session.
