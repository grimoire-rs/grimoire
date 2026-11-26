# Research: Repository Split, the grimoire-side cut and the `grim-sdk` landing list

## Metadata

**Date:** 2026-11-26
**Domain:** packaging
**Triggered by:** `/hex-plan` repository-split run. Extends `research_repository_split_recon.md` § 1.2 (not repeated). Read-only; nothing was built or run, so every count below is from `git grep`, `git show` and `wc`.
**Expires:** 2027-02-26

## Direct Answer

- The cut edits or deletes about 50 tracked paths outside `crates/grim-mirror/`: 13 deletes (`mirroring.md`, `subsystem-mirror.md`, `git_fixture.py`, the ten `test_mirror_*.py`), the rest are line edits in 31 files. Nothing under `src/` names the crate (`git grep` empty), so grim's Rust sources are untouched.
- `mirroring.md` never reached `origin/main` (`git cat-file -e origin/main:docs/src/content/docs/mirroring.md` fails). The URL contract is therefore not broken by deleting it: `docs/check_urls.py` has no `mirroring` entry (`CHAPTERS` lists 21 other slugs, no frozen URL list, no redirect table in `astro.config.mjs`). The only coupling is rule DOC-URL-06 (sidebar slug must resolve to a page): the sidebar line `astro.config.mjs:196` must go in the same commit. No stub page is needed; a stub would have to be added to the sidebar and would publish a URL the owner does not want.
- Test arithmetic: the ten suites are **71 cases** (no `parametrize` in any of them). `SUITE_FLOOR` went 4384 to 4494 (+110) in `4e420be8`; 71 of the 110 are mirror, 39 are dispatch/convert/mirror-table cases that stay. Subtracting gives **4423** before the SDK check adds its cases; the final value must be **measured** (`pytest --collect-only -q | tail -1` on the merged tree), because `test_accept_floor.py` compares the merged junit count to the file. `SKIP_CEILING` stays **296**: the mirror suites skip only on `win32` (all 71 through `grim_mirror_binary`, plus 4 `skipif`), and the Windows leg is outside the floor (`subsystem-ci.md` § 8). The +1 in `d7cf2730` (295 to 296) is a dispatch case and stays.
- `Cargo.lock` (`4e420be8`/`f71eb216` added 31 lines in three hunks): the `grim-mirror` package, the `spdx 0.13.6` package (`smallvec` already existed), and a `dependencies = [serde, serde_core]` block on `semver` (the mirror alone enables semver's `serde` feature; the root does not, `Cargo.toml:49`). Revert all three; `cargo metadata --locked` must then pass. The other mirror dependencies are root dependencies and stay.
- Riskiest items: (1) `AGENTS.md` sits at exactly 280 of 280 lines; (2) the floor value must be measured, not computed; (3) `arch-threat-model.md` and `arch-principles.md` point at `subsystem-mirror.md`, which moves, so those links dangle and `test_catalog_references_resolve`/the doc link checks trip unless rewritten in the same commit; (4) the commits `f71eb216`, `4e420be8`, `55ed3316`, `18f160dc`, `eaa7da0b` mix mirror and keep content in the same files (`conftest.py`, `test/taskfile.yml`, docs, rules), so a revert-style cut is wrong; the cut is a rewrite of those files by hand.

## Table 1: edit list, one row per file

Commit tags: C=`f71eb216` crate, T=`4e420be8` tests+CI, D=`55ed3316` docs, S=`18f160dc` skills, R=`eaa7da0b` rules.

| File | Mirror content now | Action | Replacement |
|---|---|---|---|
| `crates/grim-mirror/**` (127 files, 23,694 lines) | all (C) | delete dir | moves to `grimoire-rs/grim-mirror` |
| `Cargo.toml:5` | `members` ends `"crates/grim-mirror"` (C) | rewrite line | drop it; add `"crates/grim-sdk"` |
| `Cargo.lock` (+31 lines, 3 hunks) | `grim-mirror` package (`:1100`), `spdx` package, `semver` gains `serde`/`serde_core` deps | delete 3 hunks | add `grim-sdk` entry |
| `.gitattributes:19` | `crates/grim-mirror/tests/licenses/*.crlf.txt -text` (C) | delete line | none (line moves with the crate) |
| `.github/workflows/verify-basic.yml:130-131` | step `Build grim-mirror` (`cargo build --release --locked -p grim-mirror`) (T) | delete 2 lines | none |
| same `:134-140` | step `Upload grim-mirror Binary`, artifact `grim-mirror-binary`, 3-day retention (T) | delete 7 lines | none |
| same `:171-172` | `mirror-artifact: grim-mirror-binary`, `mirror-file: grim-mirror` (T) | delete 2 lines | none |
| `.github/workflows/verify-deep.yml:32-40` | step `Build grim-mirror` per target (cp to `grim-mirror-<target>`) and `Upload grim-mirror Binary` (T) | delete 9 lines | none |
| same `:200-201` | `mirror-artifact`/`mirror-file` for the acceptance matrix (T) | delete 2 lines | none |
| `.github/workflows/acceptance.yml:24-29` | inputs `mirror-artifact`, `mirror-file` (T) | delete 6 lines | none |
| same `:91-103` | steps `Download grim-mirror`, `Prepare grim-mirror` (T) | delete 13 lines | none; whole `4e420be8` workflow delta is +45 lines across the three files, so the three files return to trunk bytes |
| `test/taskfile.yml:8` | var `GRIM_MIRROR_BINARY` (T) | delete line | none |
| same `:18-19` | `cargo build --release --locked -p grim-mirror` + `cp` into `test/bin/` (T) | delete 2 lines | none |
| same `:35,76,99,147,161,229,264` | `GRIM_MIRROR_COMMAND: "{{.GRIM_MIRROR_BINARY}}"` in 7 `env:` blocks (T) | delete 7 lines | none; `4e420be8` added 10 lines total (8 above plus 2), so reverting the file to trunk is exact |
| `test/conftest.py:192-258` | fixture `grim_mirror_binary`, class `GrimMirrorRunner`, fixture `grim_mirror` (69 lines, T) | delete block | none; nothing else imports them (`git grep` over `test/` hits only the ten suites) |
| `test/src/git_fixture.py` (199 lines, whole file new in T) | `MIT_TEXT` read from `crates/grim-mirror/licenses/MIT.txt` at import (`:18`), `GitRepo`, `skill_md`, `agent_md`, `monorepo`, `marketplace`, `BASE_DATE` | delete file | mirror-only: its only importers are the ten `test_mirror_*.py`; `test_publish_mirror_table.py`, `test_convert.py`, `test_plugin_dispatch.py` do not import it. It moves with the suites |
| `test/tests/test_mirror_{bundles,cli,dialect,kinds,license,marketplace,monorepo,retire,staging,versions}.py` | 5+13+4+4+10+14+5+4+6+6 = 71 cases, 1,445 lines (T) | delete 10 files | move to mirror repo |
| `test/tests/test_publish_mirror_table.py`, `test_convert.py`, `test_plugin_dispatch.py`, `test_settings_consent.py:26->27` | none (grim-side; none uses the binary) | keep | -- |
| `test/SUITE_FLOOR` | 4494 (T) | rewrite | measured value (about 4423 plus SDK cases) |
| `test/SKIP_CEILING` | 296 (the +1 is dispatch) | keep | re-measure at gate; expected unchanged |
| `docs/src/content/docs/mirroring.md` (441 lines, never on main) | whole page: scope, install, `mirror.toml`, run, report, CI, takedown, trust | delete file | user guide moves to mirror repo |
| `docs/astro.config.mjs:196` | sidebar `{ slug: 'mirroring', ... }` (D) | delete line | none (DOC-URL-06) |
| `docs/src/content/docs/commands.md:4689` | sentence "[`grim-mirror`](./mirroring.md) runs it on every unit of a `dialect = "claude"` upstream" in the `convert` section | rewrite sentence | name the mirror repository link `https://github.com/grimoire-rs/grim-mirror` |
| same `:4890-4891` | Plugins section: example `grim mirror sync` and "first plugin is grim-mirror" link | rewrite link only | keep the example (dispatch content), link to the repo |
| same `:4910`, `:4938` | `grim mirror sync --file x` example; reservation of the name `mirror` | keep | dispatch content; no link |
| rest of the `convert` section and the dispatch section of `commands.md` (about 98 added lines) | none | keep | -- |
| `docs/src/content/docs/publishing.md:1480-1523` | `mirror` table, keys, annotation mapping: grim-side | keep | -- |
| same `:1485-1486` | "[`grim-mirror`](./mirroring.md) writes it for every package it republishes" | rewrite link | repo URL |
| `docs/src/content/docs/artifacts.md:955-957,966-973` | the three `com.grimoire.mirror.*` rows and the "Mirrored artifacts" subsection | keep | grim-side wire contract |
| same `:961`, `:975` | link to `./mirroring.md` and `./mirroring.md#trust` | rewrite both | repo URL; `#trust` anchor has no grim-side target, so link the repo's README |
| `docs/src/content/docs/json-interface.md:714-715` | "report of the grim-mirror plugin ... documented on the mirroring page" (the `convert` report text above it is grim-side) | rewrite 2 lines | "documented in the mirror repository" with link |
| `docs/src/content/docs/stability.md:62-68` | reservation of the name `mirror`, `[grim-mirror](./mirroring.md)` | keep, rewrite link | repo URL |
| same `:662-669` | whole bullet "The grim-mirror plugin": grammar, report, exit codes, version rules | delete bullet | one sentence: a plugin program is not grim's and carries its own stability; add the `grim-sdk` bullet (see Table 2) |
| `docs/src/content/docs/configuration.md` | `GRIM_BIN` row (D) | keep | dispatch content |
| `catalog/skills/grim-usage/SKILL.md:145` | plugin note "(e.g. the Unstable `grim-mirror`)" (S) | rewrite 1 clause | "e.g. `grim-mirror`, a separate repository"; the `convert` table row (`:~60`) stays |
| `catalog/skills/grim-authoring/references/release-checklist.md:210` | "the Unstable `grim-mirror` binary writes these" | rewrite clause | "a mirror tool writes these"; the `mirror` table text stays |
| `.claude/rules.md:40` | By concern row "Mirroring upstream repositories" (R) | delete row | none |
| same `:89` | By subsystem row "Mirror crate" | delete row | add the SDK row if a subsystem rule is created |
| same `:128` | auto-load row `crates/grim-mirror/**` | delete row | SDK row if rule created |
| `.claude/rules/subsystem-mirror.md` (32 lines, glob `crates/grim-mirror/**`) | whole rule | delete file (moves) | rewritten in the mirror repo |
| `.claude/rules/arch-principles.md:19-24` | layout paragraph names `crates/grim-mirror` and `subsystem-mirror.md` | rewrite | name `crates/grim-sdk` instead |
| same `:109` | ADR index row for `adr_artifact_mirroring.md` ("workspace crate `crates/grim-mirror`... until its own repository") | rewrite row | state: dispatch, `mirror` table and `convert` landed here; the tool lives in `grimoire-rs/grim-mirror` |
| `.claude/rules/arch-threat-model.md:40` | scope note on PATH dispatch | keep | dispatch content |
| same `:193-197` | "Mirrored artifacts" bullet names `grim-mirror`, ends "(`subsystem-mirror.md`)" | rewrite | "a mirror tool"; drop the dangling rule reference, cite the ADR |
| `.claude/rules/product-context.md:106-110` | layout bullet names `crates/grim-mirror` sibling binary | rewrite | `crates/grim-sdk` |
| `.claude/rules/subsystem-cli-commands.md:44` | "`grim-mirror` is its main caller" in the `convert` row; plugin row (`:45`) keeps `mirror` reservation | rewrite one clause | "the mirror tool is its main caller" |
| `.claude/rules/meta-ai-config.md` | no hit (`git grep` empty) | keep | none; the brief listed it, nothing to change |
| `AGENTS.md:75-76` | layout: "and `crates/grim-mirror` (separate binary, ... Unstable)" (R) | rewrite 2 lines | `crates/grim-sdk` (own version, Unstable, unpublished, no dep on `grimoire`) |
| same `:85` | table row `crates/grim-mirror/**` | delete row | SDK row only if a rule exists |
| same `:111` | `GRIM_BIN` row: "`grim-mirror` drives grim through it" | rewrite clause | "plugins such as `grim-mirror` and `grim-sdk` callers use it"; keep the row |
| `.claude/hooks/hook_utils.py:339,353` | learnings category `mirror` | keep | unrelated and predates the branch (not in `git diff 72d3fce1 HEAD`) |
| `.claude/hooks/post_tool_use_tracker.py` | no mirror row (grep empty) | keep | -- |
| `.agents/upstream-watchlist.md:358-365` | section "Artifact mirroring": 3 rows; intro and row 1 name `grim-mirror` | rewrite 2 lines | rows stay (they back `convert` and the dialect); say the mirror repo owns the marketplace rows 2-3 after the split, or move those two rows there |
| `.agents/memory/hex.md:111,131` | `:111` is the uncommitted split decision; `:131` "Repository scope ... the split is a follow-up" | keep, append | a dated line "split done; crate moved"; do not rewrite history entries |
| `.agents/adr/adr_artifact_mirroring.md` (17 hits; chosen R2 at `:78-86`) | decision R2 "workspace crate `crates/grim-mirror`"; "Unstable sibling surface until the split" (`:84`) | add note | a dated note under Status: the crate moved to `grimoire-rs/grim-mirror` before merge; D1, D6-D8, D11-D17 now bind that repository; the grim side is D2, D9, D10, D18, D19. Status stays Proposed until the owner flips it |
| `.agents/adr/adr_artifact_trust_model.md:149-155,219` | proposed scope note naming the mirror | keep | tool-agnostic wording |
| `.agents/plans`, `specs`, `goals`, `discussions`, `research` | records | keep | historical |

## Table 2: landing list for `crates/grim-sdk`

| File | Edit | Check that trips if skipped |
|---|---|---|
| `crates/grim-sdk/Cargo.toml` | `[package]` name `grim-sdk`, `version = "0.1.0"` (own, not `version.workspace`), `publish = false`, `[package.metadata.dist] dist = false`, `[lints] workspace = true`; no dependency on `grimoire`; deps only `serde`, `serde_json`, optional `thiserror` | `cargo-deny`, dist plan |
| `crates/grim-sdk/src/exit_code.rs` | exit-code table as an enum plus a unit test that `include_str!("../../../src/cli/exit_code.rs")` and asserts each variant value equals grim's (same pattern as the removed `grim-mirror/src/exit_code.rs:34`); a changed or renumbered code fails `cargo test` | unit test |
| `crates/grim-sdk/tests/boundary.rs` | optional: `cargo tree -p grim-sdk -e normal` names no `grimoire` package; `dist-workspace.toml` never names it (copy of the mirror's `tests/boundary.rs:11-56`) | -- |
| acceptance check | recommended: `crates/grim-sdk/tests/against_grim.rs`, `#[ignore]` unless `GRIM_COMMAND` is set, runs `grim --version` and one `--format json` call through the SDK; wired as one CI step after the `Build` step of the job that already builds `grim` in `verify-basic.yml` (same run, same binary), through a new task `rust:sdk:accept` in `taskfiles/rust.taskfile.yml` (subsystem-ci rule 1: CI calls tasks). Alternative: a pytest case under `test/tests/test_sdk_cli.py` that needs a second built artifact like the mirror binary; heavier, not recommended | floor moves only if pytest |
| `Cargo.toml:5` | add `"crates/grim-sdk"` to `members` | build |
| `Cargo.lock` | one new `[[package]]` entry | `--locked` builds |
| `docs/src/content/docs/stability.md` | add a bullet in the `unstable` list (`:545` onward, beside `:662`): `grim-sdk` is an Unstable sibling surface, no stable library API, unpublished. The "Five exceptions" paragraph (`:24-30`) counts exceptions to the frozen rows, not this list, so its count stays | docs gate, DOC-URL-07 |
| `AGENTS.md:75-76` | layout sentence (net 0 lines after removing the mirror lines) | `test_line_budget` (280, exact now) |
| `AGENTS.md:85` | add a subsystem row only if `subsystem-sdk.md` is created | `test_catalog_subsystem_coverage` |
| `.claude/rules/subsystem-sdk.md` (new, optional, under 30 lines) | `paths: crates/grim-sdk/**`; invariants: no `grimoire` dep, drives grim by CLI and JSON, exit table pinned by test | `test_all_rule_globs_match_files` needs the dir to exist in the same commit |
| `.claude/rules.md` | if the rule exists: one row in By concern, one in By subsystem, one in By auto-load. No declared overlap needed: `crates/grim-sdk/**` is a distinct pattern string | `test_catalog_covers_all_rules`, `test_catalog_references_resolve`, `test_path_overlaps_declared_or_absent` |
| `.claude/rules/arch-principles.md:19-24`, `product-context.md:106-110` | layout paragraphs (counted in Table 1) | doc drift review |
| `.claude/hooks/post_tool_use_tracker.py` (`CONTEXT_REMINDERS`, `:33-45`) | optional row `("crates/grim-sdk/**", "subsystem-sdk.md", "SDK")` before `src/**`; no `crates/` row exists today, so none is required | `claude:tests` only if the rule file is named wrongly |
| `.licenserc.toml:5` | none: `crates/**/*.rs` already covers it; new `.rs` files need the SPDX header | licence-header check |
| `deny.toml` | none: workspace-wide; new dependencies must fit the allow list | `cargo deny` |
| `dist-workspace.toml` | none (`members = ["cargo:."]`); `dist = false` in the SDK manifest keeps the plan clean | `cargo dist plan` |
| `taskfiles/release.taskfile.yml:59` | none expected: `cargo set-version <v>` without `--workspace` bumps the root package and `[workspace.package]` only, so the SDK keeps 0.1.0. Unverified (not run); if the owner wants a guard, add `--exclude grim-sdk` is not valid without `--workspace`, so test it once at release prepare | release dry run |
| `.agents/adr/` | no ADR needed for the SDK unless the owner wants one; the discussion record is `.agents/discussions/repository-split.md` | -- |

Tests of `.claude/tests/test_ai_config.py` that the edits will trip if mishandled: `test_all_rule_globs_match_files` (deleting `subsystem-mirror.md` removes its glob, fine; a new rule needs files), `test_catalog_covers_all_rules` and `test_catalog_references_resolve` (the three `rules.md` rows and every `subsystem-mirror.md` link must go together; `arch-principles.md:24` and `arch-threat-model.md:197` hold the other two dangling references), `test_catalog_subsystem_coverage` (AGENTS.md rows must be in the catalog), `test_line_budget` (280). No ADR-index test fails on an ADR note; `adr_artifact_mirroring.md` stays Proposed.

## Negative results

- No hit for the crate in `src/`, `catalog/publish.toml`, `README.md`, `CONTRIBUTING.md`, `.github/dependabot.yml`, `taskfile.yml`, `taskfiles/`, `dist-workspace.toml`, `deny.toml`, `.licenserc.toml`, `.config/`, `docs/docs.toml`, `docs/check_urls.py`, `docs/public/`, `.claude/tests/`.
- `meta-ai-config.md` and `.claude/hooks/hook_utils.py` (category `mirror`, predates the branch) need no edit; the brief expected them.
- `test_publish_mirror_table.py`, `test_plugin_dispatch.py`, `test_convert.py` never use `grim_mirror_binary`, so they run with the binary removed.
- No redirect mechanism exists in `astro.config.mjs`; none is needed because the page was never published.
- Not measured (read-only run): the final `SUITE_FLOOR`, whether `cargo update`/`cargo metadata --locked` drops `smallvec` or keeps it, the `cargo set-version` behaviour with an independent member version, whether `spdx` is reachable through another dependency (the diff shows `grim-mirror` as the only added dependent).
