---
name: prepare-release
description: 'Use when the tree must be made ready for a grim release ("prepare the release", "get release ready", "release prep") before the release commit. Covers every change since the last tag, drift in the shipped `catalog/` packages and the docs pages, the version bump, and the site changelog section. Argument is the bump level: major, minor or patch. The release commit, tag and push stay with the human.'
user-invocable: true
disable-model-invocation: true
argument-hint: "<major|minor|patch>"
triggers:
  - "prepare release"
  - "prepare the release"
  - "release ready"
  - "release prep"
  - "ready for release"
---

# /prepare-release — Make the Tree Release-Ready

Wraps the AI half of the release ceremony in
[workflow-release.md](../../rules/workflow-release.md): everything up to, and
not including, the `release: vX.Y.Z` commit. The session that runs it is the
orchestrator. Subagents do the reading and the edits, and the orchestrator
checks their reports against the code before it relies on them.

Worker briefs, ready to fill in: [references/briefs.md](./references/briefs.md).

## Contract

- **Input**: `$ARGUMENTS` is `major`, `minor` or `patch`. Missing or anything
  else: stop and ask. Never fall back to `auto`.
- **Output**: fix commits for catalog and docs drift, then an uncommitted
  working tree holding the bumped `Cargo.toml`, `Cargo.lock`,
  `catalog/publish.toml`, the regenerated `CHANGELOG.md`, and the new section
  on `docs/src/content/docs/changelog.md`.
- **Never**: the `release:` commit, a tag, a push, or a dispatch of
  `release.yml` or `publish-catalog.yml`. A release is `release.yml`
  dispatched from `main`; the workflow creates the tag.

## Procedure

### 1. Preflight

1. `LAST=$(git describe --tags --abbrev=0)`,
   `NEXT=$(git-cliff --bumped-version --bump <level>)`.
2. `git-cliff --version` and `cargo set-version --version` must both run:
   step 5 needs them and neither is in `ocx.toml`. Install what is missing now
   (`cargo install --locked cargo-edit`, at a version the active rustc accepts).
3. Stop when the branch is `main`, the tree is dirty, or `Cargo.toml` already
   carries a version above `$LAST`. The last case means a prepare already ran:
   resume at step 6.
4. **Release gates.** `gh issue list --state open --search "release"`, and read
   every issue that names a condition to meet before a grim release. Each open
   one is a blocker to report. It is never a reason to skip the remaining steps.
5. **Breaking markers.** List the `<type>!:` subjects and `BREAKING CHANGE`
   footers in `$LAST..HEAD`. Under the stabilization freeze each one needs an
   owner decision on record and an entry in
   `docs/src/content/docs/upgrading.md`. Report each by commit.
   A `patch` run that finds one stops here.

### 2. Inventory and catalog drift (two workers, parallel)

- **Inventory** (read-only): every user-visible change in `$LAST..HEAD`,
  grouped by outcome, with its doc page and anchor, its upgrade-note anchor,
  and a `gap:` line. It writes one scratch file outside the repository. Steps 3
  and 6 read that file and do not re-read the commits.
- **Catalog drift** (edits `catalog/` only): the trigger paths in
  [catalog/README.md](../../../catalog/README.md) › "Keeping content honest",
  checked against live `grim <cmd> --help`. It ends on `task catalog:verify`.
  Earlier drift reviews inside the range do not clear the commits after them.

### 3. Docs pass (one worker, after the inventory)

It fixes every `gap:` and every `upgrading: MISSING` from the inventory. It
edits `docs/` only and never the changelog page, and follows the `docs` skill and
`docs-style.md`. Gates: the `docs-quality` checks on the changed pages and
`task docs:check` (Node 24). A page that states a version ("since grim x.y.z")
must name `$NEXT`.

### 4. Check and commit the fixes

Read both diffs. Check a sample of the changed claims against
`./target/debug/grim <cmd> --help` and the source line the worker cites. A
worker's "verified" is a claim until one is checked. Then commit by the `commit`
skill's rules (signed off): `docs(catalog): …` and `docs: …`. They must land
before step 5, so that `CHANGELOG.md` and the catalog publish carry them.

### 5. Bump

```sh
env -u OCX_GLOBAL task release:prepare BUMP=<level>
```

It sets the version in `Cargo.toml` and `catalog/publish.toml`, regenerates
`CHANGELOG.md` and runs `task verify`. A red verify stops the run: report the
failing line. Do not write the changelog page for a tree that does not verify.

Then run the old-release interop suite against the bumped build. `task verify`
skips it without `GRIM_OLD_BIN`, and CI runs it in the deep tier only:

```sh
GRIM_OLD_BIN=<release binary that task names> env -u OCX_GLOBAL task test:old-grim
```

`ocx package exec ocx.sh/grimoire/cli:<version> -- sh -c 'command -v grim'`
prints that binary's path. A case that compares bytes written by two grim
versions can pass only while the development build still prints the old
version, so it fails here first. Fix it in a `test:` commit below the release
commit.

### 6. Site changelog

Run the `changelog` skill for `$NEXT`, with the inventory file as the source
for its filter and link steps. `major` and `minor` add a `##` section with a
theme paragraph. `patch` adds a `###` under the current minor. For `major`,
also re-read `docs/src/content/docs/stability.md` and
`docs/src/content/docs/upgrading.md` against
the inventory's breaking items.

### 7. Hand off

Leave the step 5 and step 6 changes uncommitted: they are the human's
`release: v$NEXT` commit. Report, in this order:

1. the version, and each gate with its result;
2. open blockers from step 1, as linked issues and commits;
3. drift fixed (counts, plus the commits), and any gap left open;
4. the commands the human runs, as `task release:prepare` printed them.

The `CHANGELOG.md` date is the day step 5 ran. When the release dispatches on a later
day, rerun `task release:prepare BUMP=auto VERSION=$NEXT` and correct the date
in the page's `### $NEXT` heading.

## Worker rules

- At most three workers, all in this checkout. Their edit scopes do not overlap
  (`catalog/`, `docs/`, one scratch file), so no extra worktree is needed, and
  only the catalog worker builds.
- Set the model on every spawn. Each of these briefs is default-tier work.
- No worker commits, pushes, stashes, or edits `Cargo.toml`, `CHANGELOG.md` or
  the `version` in `catalog/publish.toml`.
- A worker changes only what its brief names. Anything else it finds wrong, it
  reports. Read each diff for edits outside the brief and revert them.
- A worker report is structured: fixed (`file:line`, evidence), verified clean,
  undecided, gate result. "Unverified" claims are marked as such.

$ARGUMENTS
