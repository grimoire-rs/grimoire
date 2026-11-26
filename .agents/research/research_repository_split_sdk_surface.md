# Research: grim-sdk Surface (grim-calling code of grim-mirror)

## Metadata

**Date:** 2026-11-26
**Domain:** packaging
**Triggered by:** Owner decision: `crates/grim-mirror` moves to its own repository, grimoire gains a minimal `crates/grim-sdk`. Builds on `research_repository_split_recon.md` § 1.5 (not repeated).
**Expires:** 2027-02-26

## Direct Answer

All grim-calling code is already isolated in `crates/grim-mirror/src/publish.rs` (spawn core, lines 80-203) plus `exit_code.rs` (whole file) plus `DriverError` (`model.rs:89-103`). Pure move works. Four mirror types leak into it: `Kind` (convert), `Warning` (convert report), `Probe` (describe), and the mirror-specific `mirror`-table probe inside `capabilities`. Cut each with the smallest change below. **There is no version floor in the mirror today**: only `grim --version` text is stripped and stored (`publish.rs:225-234`, `run.rs:134`). A floor check is new code, not a move. grim-sdk is sync (std threads), needs only `serde_json` (+ `semver` if a floor is added), no tokio. Paths below are relative to `crates/grim-mirror/` unless noted.

## 1. Grim-calling items, current signatures

`src/publish.rs`
- consts `PUBLISH_DEADLINE: Duration = 600 s`, `CALL_DEADLINE: Duration = 120 s` (:19-20); `REVISION_KEY` (:21).
- `pub struct GrimDriver { pub bin: PathBuf }` (:67-70).
- `pub fn resolve_bin(grim_bin: Option<&OsStr>) -> PathBuf` (:73-78). Absolute `GRIM_BIN` else bare `grim` (spawn does the `PATH` lookup).
- `struct Output { code: Option<i32>, stdout: String, stderr: String }` (:80-84).
- `fn drain(mut pipe: impl Read + Send + 'static) -> thread::JoinHandle<String>` (:86).
- `fn run(bin: &Path, args: &[&OsStr], cwd: Option<&Path>, env: &[(&str, &OsStr)], deadline: Duration) -> Result<Output, DriverError>` (:98-165). Stdin null, piped out/err, ETXTBSY (errno 26) retry x20 at 25 ms (:119-135), try_wait poll 10 ms, kill at deadline returning `code: Some(75)` (:142-149). Carries the `shortcut:` comment (:96-97, descendants not killed).
- `fn run_retry(<same params>) -> Result<Output, DriverError>` (:168-185). One rerun on exit 75 or deadline.
- `fn unexpected(what: &str, o: &Output) -> DriverError` (:187), code 69.
- `fn exit_failure(o: &Output, what: &str) -> DriverError` (:195). Passes 69/74/75/80/81 through, else `unexpected`.
- `GrimDriver::capabilities(&self, needs_convert: bool) -> Result<Capabilities, DriverError>` (:220-261): runs `--version` (:225-234 parse, strip `grim `), `schema --kind publish` (:236-247), `convert --help` (:248-259). Mixed: version parse generic, rest mirror-specific.
- `pub struct Capabilities { pub grim_version: String }` (:33-37); `fn defines_mirror(schema: &Value) -> bool` (:206).
- `GrimDriver::convert(&self, src: &Path, dest: &Path, kind: Kind) -> Result<ConvertOutcome, DriverError>` (:269-315); `pub enum ConvertOutcome { Converted { warnings: Vec<Warning> }, Refused(String) }` (:39-46); `fn convert_warnings(report: &Value) -> Vec<Warning>` (:473).
- `GrimDriver::publish_group(&self, group_dir: &Path, dry_run: bool, source_date_epoch: i64, ceiling: &Path) -> Result<Vec<PublishItem>, DriverError>` (:324) and private `publish_group_within(.., deadline: Duration)` (:334). Env `SOURCE_DATE_EPOCH`, `GIT_CEILING_DIRECTORIES` (:347-350).
- `fn parse_publish(o: &Output) -> Option<Vec<PublishItem>>` (:443); `pub struct PublishItem {name, kind: String, version, status: PublishStatus, digest: Option<String>, message: Option<String>}` (:58); `pub enum PublishStatus {Pushed, DryRun, Skipped, Failed}` (:49).
- `impl Published for GrimDriver { fn probe(&self, reference: &str) -> Result<Option<Probe>, DriverError> }` (:366-383); `GrimDriver::describe_recorded(&self, reference: &str) -> Result<Option<Recorded>, DriverError>` (:399); `pub struct Recorded { pub probe: Probe, pub annotations: BTreeMap<String,String> }` (:387); `fn parse_describe(stdout: &str) -> Option<Probe>` (:427).
- Mirror-only, NOT grim-calling: trait `Published` (:24), `map_item` (:496), `output_drift`, `table_name`, `quote`, `key`, `GroupEntry`/`GroupKey`, `group_publish_toml` (:~586-615 area). Stay.

`src/exit_code.rs` (whole file, 73 lines)
- `#[derive(Debug, Clone, Copy, PartialEq, Eq)] #[repr(u8)] pub enum ExitCode { Success=0, Failure=1, UsageError=64, DataError=65, Unavailable=69, IoError=74, TempFail=75, NoPermission=77, ConfigError=78, NotFound=79, AuthError=80, OfflineBlocked=81 }` (:7-22); `impl From<ExitCode> for std::process::ExitCode` (:24-28); test `values_match_grims_exit_codes` with `grim_variants()` (:30-72).

`src/model.rs:89-103`: `pub struct DriverError { pub code: Option<i32>, pub message: String }` + `Display` + `Error`. `model.rs:72-80`: `pub struct Probe { digest: String, revision: Option<String>, tags: Vec<String> }`.

`src/report.rs:209`: `pub fn infra_code(err: &DriverError) -> ExitCode` maps grim code to table else 69. Pure exit-code mapping, but it is how the mirror "maps exit codes". Tests at `report.rs:~430-438`.

`src/run.rs:51-59`: `Env { grim_bin: Option<OsString>, offline: Option<String> }`, `from_process()` reads `GRIM_BIN` (:56) and `GRIM_OFFLINE` (:57); `is_offline` truthy set 1/true/yes/on (:62-66). `run.rs:95-97` builds the driver. Offline refusal (:88) is mirror policy.

No other file spawns grim. `git` spawns (`git.rs`, `enumerate.rs`, `stage.rs` tests) are not grim-calling.

## 2. Cut line and proposed grim-sdk API

Moves to grim-sdk (pure move except where marked):
- `process.rs`: `Output` (make pub), `drain`, `run`, `run_retry`, deadline consts. Make `run`/`run_retry` pub (they are the generic "run grim" core).
- `bin.rs`: `resolve_bin`; add `pub fn from_env() -> PathBuf` reading `GRIM_BIN` (move of `run.rs:56` read; the one-line wrapper is the only new code).
- `error.rs`: `DriverError`, `unexpected`, `exit_failure`. Mirror keeps `pub use grim_sdk::DriverError;` in `model.rs` so `crate::model::DriverError` paths (run.rs:23, retire.rs:14, version.rs:14, report.rs:10) are untouched.
- `exit_code.rs`: whole file incl. the include_str test.
- `version.rs`: `pub fn version(bin: &Path) -> Result<String, DriverError>` = :225-234 extracted.
- `calls.rs`: `schema(bin, kind) -> Result<serde_json::Value, DriverError>` (:236-244 with parse), `has_command(bin, name) -> Result<bool, DriverError>` (the `convert --help` probe, :249-258), `convert`, `publish`, `describe`.

Stays in grim-mirror: `Published` trait and its `impl for GrimDriver` (a local trait on a foreign type is legal), `defines_mirror`, the `mirror`-table capability check and message (:243-247), `Capabilities`/`capabilities()` as a free fn composing `version`+`schema`+`has_command`, `map_item`, `output_drift`, `group_publish_toml`, `GroupEntry`, `GroupKey`, `table_name`, `convert_warnings`, `infra_code`, `Env::is_offline` and the offline refusal.

Proposed API after the move (names kept, signatures kept unless flagged CHANGED):
```
grim_sdk::ExitCode                       // unchanged, + From<ExitCode> for std::process::ExitCode
grim_sdk::DriverError { code, message }  // unchanged
grim_sdk::Output { code, stdout, stderr }          // CHANGED: pub
grim_sdk::resolve_bin(Option<&OsStr>) -> PathBuf   // unchanged
grim_sdk::bin_from_env() -> PathBuf                // new 1-liner
grim_sdk::run / run_retry (bin, args, cwd, env, deadline) -> Result<Output, DriverError>  // pub
grim_sdk::CALL_DEADLINE, PUBLISH_DEADLINE
grim_sdk::GrimDriver { pub bin }                   // unchanged
  .version() -> Result<String, DriverError>
  .schema(kind: &str) -> Result<Value, DriverError>
  .has_command(name: &str) -> Result<bool, DriverError>
  .convert(src, dest, kind: &str) -> Result<ConvertOutcome, DriverError>              // CHANGED kind
  .publish_group(group_dir, dry_run, env: &[(&str,&OsStr)], deadline) -> Result<Vec<PublishItem>, DriverError>  // CHANGED env
  .describe(reference) -> Result<Option<Recorded>, DriverError>   // merges probe+describe_recorded
grim_sdk::{ConvertOutcome, PublishItem, PublishStatus, Probe, Recorded}
```
Leaks and smallest cuts:
1. `Kind` in `convert` (:269-280). Take `kind: &str` ("skill"/"rule"/"agent"); the 3-arm match and the "cannot convert a {other:?} unit" error (:270-280) stay in the mirror, in a thin `fn convert_kind(kind: Kind) -> Result<&'static str, DriverError>`. Caller: `retire.rs` (see § 5).
2. `Warning` in `ConvertOutcome::Converted` (:40-43) and `convert_warnings` (:473, builds `report::Warning` with codes `body-token`/`field-dropped`). Make sdk return `Converted { report: serde_json::Value }`; `convert_warnings(&Value)` stays in the mirror unchanged. Two lines change at the one match site `retire.rs:83`.
3. `Probe` is defined in `model.rs:74` and used by `Published`, `version.rs`, `run.rs`. Move `Probe` to the sdk and re-export from `model.rs` (`pub use grim_sdk::Probe;`). It only mirrors `grim describe` fields, so it is generic. `REVISION_KEY` moves with `parse_describe`.
4. `capabilities` mirror probe: split as above; sdk never mentions `mirror`. `Capabilities` stays in the mirror (`run.rs:134` reads `.grim_version`).
5. `publish_group` hard-codes `SOURCE_DATE_EPOCH`/`GIT_CEILING_DIRECTORIES` (:346-350). Both are generic grim/git env, but the call is mirror-shaped; cut by passing `env` and a `deadline` from the mirror wrapper (`publish_group_within` already has the deadline parameter, :334). Mirror keeps `fn publish_group(driver, group_dir, dry_run, source_date_epoch, ceiling)` as a free fn building the env array.
6. `--format json` is added by hand in each call (:290-291, :342, :372, :402), not by a shared helper. The brief's "run with `--format json`" is the per-call arg; a shared helper would be new design, not part of a pure move.
7. `exit_failure`'s pass-through list `69|74|75|80|81` (:197) and `infra_code`'s list (report.rs:211-223) are two copies of "codes grim may return". Pure move keeps both; optional later: `ExitCode::from_code(i32) -> Option<ExitCode>` in the sdk and `infra_code` becomes one line.
8. Version floor: none exists. To satisfy "check the version floor" add `GrimDriver::require(min: &semver::Version) -> Result<Version, DriverError>` (code 69, message "upgrade grim"). New code, needs `semver`. Flag to owner: not a move.

## 3. Dependencies the moved code needs

From `Cargo.toml:15-27` (none else of the 12 crates is touched by the moved code):
- `serde_json = "1"` (describe/publish/schema parse, `Value`).
- `semver = "1"` (default features; only if item 8 is added; `serde` feature not needed).
- dev: `tempfile = "3"` (`Fake` grim in tests).
- Sync: `std::thread`, `std::process`, `std::time`. `tokio` is used only by the mirror's `cli.rs:109` runtime and `#[tokio::test]` in run.rs/marketplace.rs. `serde` derive not needed (no derive on moved types except none: `PublishItem`, `Probe` have no serde derives). `clap`, `toml`, `regex`, `globset`, `sha2`, `time`, `spdx` stay mirror-only. `[lints] workspace = true` applies (`unsafe_code=forbid` fine).

## 4. Tests that move

`publish.rs` tests (`#[cfg(all(test, unix))] mod tests` at :618; `Fake` fake-grim script harness :~630-675):
- Move: `resolve_bin_takes_only_absolute_grim_bin` (:678); capability pieces of `capabilities_report_version_when_everything_is_present` (:685, version assertion only), `capabilities_with_grim_missing_have_no_code` (:~724); `describe_0_reads_digest_revision_and_tags`, `describe_79_is_not_published`, `describe_failure_rows_carry_their_codes`, `describe_malformed_json_is_unexpected_output`, `describe_75_then_success_retries_once`, `describe_75_twice_fails_with_75_after_exactly_one_retry` (:~731-796, plus helper `describe_doc`); `publish_passes_args_cwd_and_env_and_maps_items`, `publish_dry_run_status_is_kept`, `publish_65_with_report_returns_items_without_it_is_a_refusal`, `publish_infra_exits_keep_their_code_even_with_a_report`, `publish_other_exit_or_garbage_stdout_is_69`, `publish_75_then_success_retries_once`, `publish_deadline_kills_retries_once_then_reports_75` (:~803-885, helpers `publish_doc`, `publish`); `convert_0_yields_warnings_65_is_refused_other_is_error`, `convert_passes_the_unit_kind` (:~920-980, adapt to `report` Value / `&str` kind).
- Stay in mirror: `capabilities_without_mirror_table_exit_69_and_name_it`, `capabilities_reject_mirror_on_another_definition`, `capabilities_without_convert_fail_only_when_needed` (:~704-745), `status_mapping_and_skipped_reread`, `output_drift_compares_digests`, `group_publish_toml_*`, `group_key_orders_by_upstream_plugin_version_commit`, `pitem`, `entry`.
- `Fake` is needed by both sides. Smallest: copy it into the sdk tests and keep the mirror's own (`retire.rs:~505 FakeGrim`, `stage.rs:592 fake_convert_grim`, `run.rs` `FakeGrim`, already separate copies). No shared test crate.
- `exit_code.rs` test `values_match_grims_exit_codes` moves with the file. `report.rs` `infra_code` test (:~430) stays unless item 7 is taken.
- `include_str!` path from `crates/grim-sdk/src/exit_code.rs`: `concat!(env!("CARGO_MANIFEST_DIR"), "/../../src/cli/exit_code.rs")`, byte-identical to today (same depth, `CARGO_MANIFEST_DIR` = `crates/grim-sdk`). Relative from the source file itself: `../../../src/cli/exit_code.rs`. The test also covers only the 12 variants; it checks sdk values equal grim's, not that new grim codes appear (`grim.len() >= mirror.len()` guard :67).

## 5. Mirror side after the move

Switch to `grim_sdk::...` (file:line, current):
- `main.rs:13` `mod exit_code;` delete.
- `cli.rs:11`, `report.rs:9`, `retire.rs:12`, `run.rs:19`: `use crate::exit_code::ExitCode;` becomes `use grim_sdk::ExitCode;`.
- `run.rs:24-26`: `publish::{GrimDriver, resolve_bin, ..}` split: `GrimDriver`, `resolve_bin`, `PublishItem` from `grim_sdk`; `GroupEntry`, `GroupKey`, `Published`, `group_publish_toml`, `map_item`, `output_drift` stay `crate::publish`. Uses: `run.rs:56` (`Env`), `:95-96` (driver), `:179`, `:200`, `:1508`, `:1922` (test), `:2014`; `run.rs:64` call becomes the mirror's `capabilities(&driver, needs_convert)` free fn.
- `run.rs:1509`, `:2014`, `:2071`, `:2097`: `publish::PublishStatus` etc. via re-export or `grim_sdk::PublishStatus`.
- `retire.rs:15-16`: `ConvertOutcome`, `GrimDriver`, `Recorded` from `grim_sdk`; call sites `:59` (`prepare` takes `&GrimDriver`), `:83` (`Converted`, adapt to `report`), `:161`, `:183`, `:194`; test `:512-513`.
- `stage.rs:592,604`: `crate::publish::GrimDriver` becomes `grim_sdk::GrimDriver`.
- `version.rs:14-15`: `DriverError`, `Probe` via `crate::model` re-export (no change if re-exported); `Published` stays.
- `bundle.rs:12,190`: `PublishItem`, `PublishStatus` from `grim_sdk`.
- `model.rs:72-103`: delete `Probe`, `DriverError`, add `pub use grim_sdk::{DriverError, Probe};` (keeps `report.rs:10`, `run.rs:23`, `retire.rs:14` unchanged).
- `Cargo.toml`: `grim-sdk = { path = "../grim-sdk" }` while in the workspace; `git`/tag dependency after the split.

`git.rs:1019` test `hardening_covers_what_the_index_clone_copy_sets`: it `include_str!`s `src/catalog/git_cmd.rs` and extracts the `"-c", "` literals (:1020-1026) and compares to `GLOBAL_CONFIG`. In the mirror repo copy that file once to `crates/grim-mirror/tests/fixtures/git_cmd.rs.snapshot` (or `src/` neighbour), change the path to `concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/git_cmd.rs.snapshot")`, and add a header line stating the grimoire commit it was taken from. Consequence: the test no longer detects drift in grim; it pins the mirror against a frozen copy. Refresh is manual (stated in the snapshot header). The split ADR said a submodule would keep the check live; the owner chose the copy.

## 6. `tests/boundary.rs`

Three tests (56 lines):
- `dependency_tree_has_no_grimoire_package` (:16-40): runs `cargo tree -p grim-mirror -e normal --prefix none --offline` from `CARGO_MANIFEST_DIR/../..`, asserts the crate appears and no package named `grimoire`.
- `manifest_is_not_published_or_distributed` (:42-50): asserts `package.publish == false` and `package.metadata.dist.dist == false`.
- `dist_workspace_does_not_name_the_crate` (:52-56): reads root `dist-workspace.toml`, asserts it lacks `grim-mirror`.

Replacement in grimoire: delete `crates/grim-mirror/tests/boundary.rs` with the crate; create `crates/grim-sdk/tests/boundary.rs` with the same three tests, `-p grim-sdk`, assert no `grimoire` (and also no `grimoire-hook-core`/`grim-hook`, cheap and stricter), `publish=false`, `dist=false`, `dist-workspace.toml` lacks `grim-sdk`. It needs `toml` as dev-dependency (the mirror gets it from its normal deps); add `toml = "1.2"` dev-dep.
In the mirror repo: test 1 asserts the tree's package names include `grim-sdk` and contain no package among `grimoire`, `grim-hook`, `grimoire-hook-core` (`cargo tree` without `--offline` only if the git dependency is not vendored; keep `--offline` after a fetch). Test 2 stays. Test 3 is dropped (no `dist-workspace.toml` there unless the mirror later adopts dist, which would make it a shipping decision).

## 7. Workspace acceptance of grim-sdk

- Root `Cargo.toml:5` `members` add `"crates/grim-sdk"` (and later remove `"crates/grim-mirror"`). `resolver = "3"`, no `workspace.dependencies` table, so every dep is spelled in the member.
- `[workspace.package]` (:11-14) has version/edition/license. grim-sdk: `version = "0.1.0"` (own literal), `edition.workspace = true`, `license.workspace = true`, `publish = false`, `[package.metadata.dist] dist = false`, `[lints] workspace = true`, plus a `description`.
- `cargo set-version` at `taskfiles/release.taskfile.yml:59`: `cargo set-version "{{...}}"`, no `--workspace`, no `-p`. In cargo-edit the default targets the current (root) package, whose `version.workspace = true` makes it rewrite `[workspace.package].version`. A member with its own literal version is therefore not touched by this call. Not executed here (read-only brief); verify once with `cargo set-version --dry-run 0.18.0` and check `git diff` shows only the root `[workspace.package]`. Do not introduce `--workspace` into that line, it would rewrite grim-sdk to the grim version. `task release:prepare` also seds `catalog/publish.toml` only (:61). The lockfile gets a `grim-sdk 0.1.0` entry; the `Cargo.lock` change is part of the commit.
- `dist-workspace.toml:2` `members = ["cargo:."]` only dist-packages the root; `dist = false` plus the boundary test keep it out. `deny.toml` applies workspace-wide, no edit.
- Tooling picks it up through `--workspace` (`taskfiles/rust.taskfile.yml:47,72`, `taskfile.yml:~36`, `coverage.taskfile.yml:21`), so sdk tests, clippy and coverage need no taskfile change.

## Negative results

- No version-floor logic anywhere in `crates/grim-mirror/src` (only `publish.rs:225-234` parse and `Capabilities.grim_version`, consumed at `run.rs:134`). No `MINIMUM_GRIM_VERSION` equivalent.
- No other file in the mirror spawns grim. `GRIM_OFFLINE` is read only at `run.rs:57`; `GRIM_BIN` only at `run.rs:56`.
- Nothing in the moved code needs tokio, serde derives or clap.
- `cargo set-version` behavior is from cargo-edit semantics, not run here (read-only brief, no cargo).
- Line numbers inside `publish.rs` tests (:618 onward) are approximate where marked `~`, derived from a relative grep.
