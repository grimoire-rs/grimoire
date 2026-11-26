# refactor(mirror): move grim-mirror to its own repository and add grim-sdk

## Summary

Replaces [#398](https://github.com/grimoire-rs/grimoire/pull/398). The mirror tool leaves this repository before it ever lands, and grimoire gains a minimal client crate that other repositories use to run the `grim` binary. Decisions: `.agents/adr/adr_repository_split.md`; plan: `.agents/plans/plan_repository_split.md`.

## What stays in grim

- PATH plugin dispatch for an unknown top-level name, with `GRIM_BIN`.
- The `mirror` table of `publish.toml` and the `com.grimoire.mirror.*` annotations.
- `grim convert` (Unstable).
- The grim side of the mirroring ADR (D2 to D5, D9, D10, D18, D19), docs for the above, and the catalog skills.

## What moved

- `crates/grim-mirror`, its ten acceptance suites, the harness they need and the mirroring docs page go to `grimoire-rs/grim-mirror`, with the full grimoire scaffold and agent configuration. `crates/grim-mirror` no longer exists here.
- A Tauri 2 skeleton `grimoire-rs/grim-gui` (binary `grim-gui`, started as `grim gui`) is a new repository; it shows the grim version it finds.
- Docs links to the mirroring page now point at `https://github.com/grimoire-rs/grim-mirror`.

## New crate: `grim-sdk`

`crates/grim-sdk` only invokes the binary. Unstable, own `0.1.0` line, unpublished, not a library API of grim. One acceptance check (`task rust:sdk:accept`) runs it against the grim built in the same run.

## Acts left to the owner and the meta-orchestrator

1. Push `hex/repository-split`, open this PR naming [#398](https://github.com/grimoire-rs/grimoire/pull/398) as replaced, dispatch `verify-deep.yml` on the new head.
2. Push the same tip to `hex/artifact-mirroring`, or close [#398](https://github.com/grimoire-rs/grimoire/pull/398).
3. Pin `grim-sdk` by commit in both new repositories (pipeline PF).
4. Create `grimoire-rs/grim-mirror` and `grimoire-rs/grim-gui`, push `main` and `hex/initial-scaffold`, open one PR each.
5. Transfer [#399](https://github.com/grimoire-rs/grimoire/issues/399), [#400](https://github.com/grimoire-rs/grimoire/issues/400), [#401](https://github.com/grimoire-rs/grimoire/issues/401) to `grim-mirror`; tick the split item on [#402](https://github.com/grimoire-rs/grimoire/issues/402).
6. Only if the git fetch of `grim-sdk` measures too costly: publish it to crates.io.
7. After this PR merges, move both pins (`grim-sdk` rev and `GRIM_COMMIT`) to the merged `main` commit.

## Evidence limits

No CI run backs the two new repositories yet. Their gates ran locally only: `grim-mirror` head `1a476f8a357b8ad4cd61ac4ac49757047a8a8fc3` (green `task --force verify`, local until act 4) and `grim-gui` (green `task --force verify` under Docker). The macOS and Windows builds of `grim-gui` and the macOS `unsafe_code` check are unproven until act 4.

## Link-check exclusion

None. `task docs:check` has no external link checker (no lychee; `docs/check_urls.py` and the docs-quality checks cover internal links, anchors and assets only), so it passed without excluding `https://github.com/grimoire-rs/grim-mirror`. Every grim-mirror URL in the docs is unchecked until the repository exists (act 4).

## AI assistance

AI assistance drafted this change; the owner reviews and decides every merge.
