---
paths:
  - crates/grim-mirror/**
---

# Subsystem: grim-mirror

`crates/grim-mirror` is a separate binary (`grim-mirror`) that republishes skills, rules, agents and bundles from git repositories and Claude plugin marketplaces as grim artifacts, at a pinned upstream commit. Decision record: `.agents/adr/adr_artifact_mirroring.md` (D1, D6-D8, D11-D17); design: `.agents/specs/design_artifact_mirroring.md`; operator docs: `docs/src/content/docs/mirroring.md`. Hooks, MCP, settings and mods are skipped by product policy, not by a gate on the kind.

## Invariants

1. **No dependency on `grimoire`.** The crate must move to its own repository unchanged. `tests/boundary.rs` pins it: `cargo tree -p grim-mirror -e normal` names no `grimoire` package, `publish = false`, `[package.metadata.dist] dist = false`, and `dist-workspace.toml` never names the crate. The only shared source text is the exit-code table (item 6). A needed grim type means a CLI/JSON field in grim, never a path dependency.
2. **Source-only, Unstable.** No release archive, no install path. The CLI, the `mirror.toml` grammar, the run report and the exit codes are an Unstable sibling surface listed under `docs/src/content/docs/stability.md` until the split. Published tags are permanent even so (snapshot versions in `version.rs`).
3. **Never runs upstream code.** No checkout: entries come from `git ls-tree -r -z` and blobs from `git cat-file --batch` (`stage.rs`, `git.rs`), so no filter driver, hook or attribute runs. No build steps: a template source (`*.src.md`, `*.tmpl`, `*.j2` and kin) or a Git LFS pointer refuses the unit as `unsupported-build-step`; a symlink or submodule entry refuses it as `unsupported-entry`. Every upstream-derived path is contained before it is joined (`unsafe-path`).
4. **Git hardening is copied, not shared** from `src/catalog/git_cmd.rs` (no lib split). `git.rs` sets `GLOBAL_CONFIG` (`protocol.ext.allow=never`, `http.followRedirects=initial`, `core.hooksPath=/dev/null`, `submodule.recurse=false`) and `ENV` (`GIT_TERMINAL_PROMPT=0`, pinned `GIT_ALLOW_PROTOCOL`, empty `GIT_ASKPASS`, `SSH_ASKPASS` removed, low-speed limits, `GIT_LITERAL_PATHSPECS=1`, `GIT_LFS_SKIP_SMUDGE=1`) before every verb, with null stdin, a deadline per call and `kill_on_drop`. `builder_pins_arguments_and_environment` pins the set: dropping a line fails it. Change both copies together, never one.
5. **Size caps before content is read.** A `--batch-check` pass refuses a blob over `MAX_BLOB_BYTES` (8 MiB) or a unit over `MAX_UNIT_BYTES` (64 MiB) as `unsupported-entry`; marketplace and manifest files, plugin counts and string lengths have their own caps in `marketplace.rs`; the licence matcher skips text over `MAX_MATCH_BYTES`. A new upstream-read path needs a cap.
6. **Drives grim only through its CLI and JSON output** (`publish.rs`, the one place grim is spawned): a capability probe on the `publish.toml` schema (no version compare), `describe`, `convert --from claude`, `publish`. The binary is `GRIM_BIN` when set and absolute, else `grim` on `PATH`; grim sets `GRIM_BIN` for a dispatched plugin (`grim mirror` runs `grim-mirror`). Exit codes mirror `src/cli/exit_code.rs`: `exit_code.rs` carries the subset and `values_match_grims_exit_codes` reads grim's file with `include_str!`. A renumbered grim code fails that test.
7. **Licence gate** (`license.rs`): evidence is tri-state per level (absent, recognised, unrecognised); the first level with evidence decides (unit, plugin, repository); a declaration in `mirror.toml` applies only when every level is absent and is recorded as `declared`, a matched file as `detected`. Missing, unrecognised, disallowed or conflicting evidence refuses the unit (`license-*` reasons). GPL/LGPL texts never assert `-only` or `-or-later` from the text alone. The licence texts under `licenses/` are matcher fixtures: never edit one to make a match pass.
8. **Reconcile, never force.** A moved upstream tag is refused (`upstream-moved`), never republished; a version collision is refused (`version-collision`); drift is detected by dry-run digest. Retirement (`retire.rs`) republishes one patch up, deprecated, and never deletes.
9. **The report is a contract.** `report.rs`: `schema`, `mode`, `grim`, `grim_mirror`, `upstreams[]`, `items[]` and `summary`. Item `status` is one of `published|would-publish|unchanged|refused|skipped|failed`; `reason` is a closed kebab-case vocabulary (`Reason`). Add a value, never rename or remove one, and bump `schema` only for a shape change. Exit: 0, else the first infra code, else 69 for any `failed`, else 65 for `refused` under `--fail-on refused` (default for `check`; `sync` defaults to `infra`).

## Testing

- Unit tests live beside the code (`cargo test -p grim-mirror`); `cargo clippy -p grim-mirror --all-targets -- -D warnings`.
- Acceptance: `test/tests/test_mirror_*.py` (bundles, cli, dialect, kinds, license, marketplace, monorepo, retire, staging, versions) plus `test_publish_mirror_table.py`. They use the `grim_mirror_binary` fixture (`GRIM_MIRROR_COMMAND`, else `test/bin/grim-mirror`, built by `test/taskfile.yml`) and a local registry. All skip on Windows (no mirror build there), so `test/SUITE_FLOOR` and `SKIP_CEILING` move with the case count.
- A new refusal reason, report field or `mirror.toml` key needs a unit test, an acceptance case and a `docs/src/content/docs/mirroring.md` update in the same change.

## Cross-references

- `subsystem-cli-commands.md` — `grim convert`, plugin dispatch, `publish.toml` `mirror` table
- `arch-threat-model.md` — a mirrored artifact's publisher of record is the operator (T1); upstream authorship is out of grim's integrity claim
- `subsystem-tests.md` — acceptance conventions
