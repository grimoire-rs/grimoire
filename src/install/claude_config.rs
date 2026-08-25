// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Managed exclusion of a rule's support directory from Claude Code's
//! auto-loaded context.
//!
//! A rule may carry a sibling support directory, and grim installs it
//! beside the index at `<rules_dir>/<name>/` for every client so the
//! index's relative links resolve. Claude Code discovers `.claude/rules/`
//! **recursively** and loads any file without `paths:` frontmatter as
//! unconditional context — so the depth files the index is supposed to
//! *route to* land in every session instead of the one the index sends
//! the agent to. Measured on a real consumer: 145k tokens of always-on
//! context from two support trees whose index rules were correctly scoped
//! and correctly not loaded (grimoire-rs/grimoire#102).
//!
//! No authoring fixes this — no path glob expresses "only when the index
//! sends you", which is precisely what a support directory means. grim
//! wrote those files, so grim owns how they behave (class 1 repair,
//! `adr_vendor_support_tiers.md`). So grim manages one
//! [`claudeMdExcludes`][claude-memory] element per support-dir rule:
//! added while the rule's output is recorded, removed when that output is
//! retired.
//!
//! **grim only ever removes an element it computed from a record it wrote.**
//! Removal is driven by the retired outputs the operation hands the sync
//! ([`super::install_state::retired_outputs`]), never *to establish
//! ownership* by probing the filesystem for a directory that happens to be
//! absent: this file is the consumer's, it is routinely committed, and it
//! holds their own exclusions beside grim's. A directory-absence probe
//! cannot tell grim's element from an identical one the user typed — and
//! would delete the user's, in their git-tracked file, for a rule grim
//! never installed.
//!
//! The filesystem is consulted once more, in the **opposite direction**: a
//! retired name whose directory is still on disk keeps its element. Three
//! paths drop a record while deliberately leaving the tree in place — an
//! output that resolves outside its anchor root, on either the uninstall
//! ([`super::uninstall`]) or the dropped-client reap
//! ([`super::prune::reap_dropped_clients`], and `prune_orphans` through it),
//! and a shared footprint a surviving sibling still references. Stripping
//! the exclusion off a live support tree is the grimoire#102 symptom all
//! over again, silently. The probe can only ever **decline** a removal grim
//! has already proved it owns, never authorize one, so it cannot reach a
//! user's element — it is a suppressor, not the old ownership oracle, and
//! not a leftover of it.
//!
//! **Accepted, and deliberately not fixed:** where the consumer hand-wrote
//! the exact element grim would write, the upsert adopts it silently (grim
//! cannot distinguish "I wrote this last run" from "the user typed it") and
//! removes it when that rule is uninstalled. That is convergent and
//! correct — the line's only purpose was to exclude that rule's tree, and
//! the tree is going away with it.
//!
//! **Exclusion suppresses auto-load only.** The files stay on disk and
//! stay readable with the Read tool, so the index's routing keeps working
//! and its relative links keep resolving — which is the behaviour the
//! artifact was authored for.
//!
//! Per rule, never a blanket `rules/*/**`: a user's own
//! `.claude/rules/frontend/` organization is documented upstream and must
//! keep loading.
//!
//! One [`scope_root`] call yields both the settings file and the `rules/`
//! directory its elements name, so the exclusion and the files it names can
//! never disagree:
//! - **project** scope edits `<workspace>/.claude/settings.json` with the
//!   worktree-portable relative glob `**/.claude/rules/<name>/**` — one
//!   committed settings file serves every git worktree of the repo, so an
//!   absolute path baked for one would not match the others;
//! - **global** scope edits `<claude_root>/settings.json`
//!   (`$CLAUDE_CONFIG_DIR` else `~/.claude`) with an absolute glob rooted
//!   there. Claude matches patterns against absolute paths, and a
//!   per-machine file has no portability requirement.
//!
//! Editing discipline is [`super::managed_config`]'s: strict on add,
//! tolerant on remove, span-preserving throughout — `settings.json` also
//! holds the consumer's `permissions` and `hooks`, and every byte outside
//! the managed element survives untouched.
//!
//! [claude-memory]: https://code.claude.com/docs/en/memory

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::install::install_state::{ClientOutput, InstallState};
use crate::oci::ArtifactKind;

use super::client_target::ClientTarget;
use super::managed_config;
use super::vendor::{env_dir, home_dir};

/// The root config key holding Claude Code's auto-load exclusion globs.
const EXCLUDES_KEY: &str = "claudeMdExcludes";

/// Claude Code reads exactly this spelling — there is no documented
/// `.jsonc` sibling, so unlike OpenCode there is no spelling to probe.
const SETTINGS_FILE: &str = "settings.json";

/// The prefix a project-scope managed element carries. `**/` keeps one
/// committed `settings.json` valid across every git worktree of the repo.
const PROJECT_PREFIX: &str = "**/.claude/rules/";

/// Every managed element ends here — the directory and everything under it.
const ENTRY_SUFFIX: &str = "/**";

/// The managed `claudeMdExcludes` element for the support directory of
/// the rule installed as `name` under `rules_dir`.
///
/// Project scope yields the workspace-independent `**/.claude/rules/<name>/**`;
/// global scope yields `<rules_dir>/<name>/**`. Claude Code is a Node tool
/// matching these as globs, where `\` is an escape character rather than a
/// separator, so a Windows `rules_dir` must still emit forward slashes.
fn managed_entry(name: &str, rules_dir: &Path, scope: ConfigScope) -> String {
    match scope {
        ConfigScope::Project => format!("{PROJECT_PREFIX}{name}{ENTRY_SUFFIX}"),
        ConfigScope::Global => format!("{}/{name}{ENTRY_SUFFIX}", managed_config::glob_path(rules_dir)),
    }
}

/// The support-directory names the Claude outputs among `outputs` name.
///
/// The recorded `support_dir` is the authority on the directory's name: it
/// is the path grim actually wrote, so it survives an install under a
/// binding name that differs from the artifact's.
///
/// The `SkillName::parse` filter is defence in depth against the one
/// declaration path that never validates this name: a directly declared
/// `[rule]` key is inserted verbatim by `parse_artifact_map` (only the value
/// is checked), and `support_dir.relative` is read here as a raw string
/// rather than through `AnchoredPath::resolve`, so the containment guard
/// never runs either. A name of `*` would interpolate into exactly the
/// blanket `**/.claude/rules/*/**` this module forbids, suppressing the
/// consumer's own `rules/` subdirectories. Also rejects empty.
fn support_dir_names<'a>(
    outputs: impl Iterator<Item = &'a ClientOutput> + 'a,
    claude: &'a str,
) -> impl Iterator<Item = String> + 'a {
    outputs
        .filter(move |o| o.client == claude)
        .filter_map(|o| o.support_dir.as_ref())
        .filter_map(|d| d.relative.rsplit('/').next())
        .filter(|n| match crate::skill::SkillName::parse(n) {
            Ok(_) => true,
            Err(reason) => {
                tracing::warn!(
                    name = %n,
                    %reason,
                    "support directory name is not a valid artifact name; its claudeMdExcludes element is skipped"
                );
                false
            }
        })
        .map(str::to_string)
}

/// Claude's layout root for `scope` — the directory holding both the
/// `rules/` tree and the settings file that excludes parts of it.
///
/// `None` at global scope when neither `$CLAUDE_CONFIG_DIR` nor `$HOME`
/// resolves. [`super::vendor_claude::scope_root`] falls back to
/// `<workspace>/.claude` there, which is right for *rendering* a rule but
/// wrong here: a rule installed globally while `$HOME` was set would, on
/// any later run without it, have its machine-absolute glob written into a
/// stray `settings.json` inside the user's repository. Same stance as
/// [`super::opencode_config`]'s global resolution — skip the sync rather
/// than invent a location.
///
/// Pure in its inputs, so both arms are reachable in a test without
/// mutating the process environment; [`scope_root`] is the wrapper that
/// reads it.
fn root_for(
    workspace: &Path,
    scope: ConfigScope,
    config_dir: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    match scope {
        // Delegated rather than spelled `.claude` a second time: the pin
        // test below resolves `rules/` through the vendor, so a one-sided
        // segment move would pass it while aiming this sync at a directory
        // nothing was written to.
        ConfigScope::Project => Some(super::vendor_claude::scope_root(workspace, scope)),
        ConfigScope::Global => super::vendor_claude::global_root(config_dir, home),
    }
}

/// [`root_for`] over the ambient environment — the one place this module
/// reads it.
fn scope_root(workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
    root_for(workspace, scope, env_dir("CLAUDE_CONFIG_DIR"), home_dir())
}

/// Converge `claudeMdExcludes` on the state's needs: one managed element
/// present for every recorded Claude rule that installed a support
/// directory, and none left over for one this operation retired. Call after
/// install/update/uninstall mutated `state`.
///
/// Removal cannot key on `state` — by the time the sync runs, an
/// uninstalled rule's record is gone and its name with it. It keys on
/// `retired` instead: the outputs the operation removed from the state
/// ([`super::install_state::retired_outputs`]), which name the very
/// directories grim itself installed. A retired name that the post-state
/// still wants (a re-install at a new pin retires the old output and
/// records a new one) keeps its element, and so does one whose directory
/// is still on disk (see the module doc's suppressor).
///
/// # Errors
///
/// An I/O failure reading/writing the settings file, or `InvalidData` when
/// an element must be **added** to a `settings.json` grim cannot parse
/// (grim refuses to clobber it). Removal stays tolerant.
pub fn sync_for_state(
    state: &InstallState,
    workspace: &Path,
    scope: ConfigScope,
    retired: &[ClientOutput],
) -> io::Result<()> {
    // Resolve the root once and derive both paths from it: resolved
    // separately they could disagree, and the element written would then name
    // a directory nothing was ever written to.
    sync_at_root(scope_root(workspace, scope), state, scope, retired)
}

/// [`sync_for_state`] against an already-resolved layout `root` — the seam
/// that makes the declining branch reachable without mutating the process
/// environment.
///
/// # Errors
///
/// As [`sync_for_state`].
fn sync_at_root(
    root: Option<PathBuf>,
    state: &InstallState,
    scope: ConfigScope,
    retired: &[ClientOutput],
) -> io::Result<()> {
    let Some(root) = root else {
        // Declining is the safe half of a disagreement, not a non-event:
        // `vendor_claude::scope_root` still falls back to
        // `<workspace>/.claude`, so the rule and its support tree land in
        // that environment while the exclusion does not. Warn-level matches
        // what `installer.rs` already gives this sync's failure path.
        tracing::warn!(
            %scope,
            "no Claude config root resolved (neither $CLAUDE_CONFIG_DIR nor $HOME); the claudeMdExcludes registration is skipped"
        );
        return Ok(());
    };
    let rules_dir = super::vendor_claude::rules_dir(&root);
    let config_path = root.join(SETTINGS_FILE);
    let claude = ClientTarget::Claude.to_string();

    let wanted: BTreeSet<String> = support_dir_names(
        state
            .iter_records()
            .filter(|r| r.kind == ArtifactKind::Rule)
            .flat_map(|r| &r.outputs),
        &claude,
    )
    .collect();

    for name in &wanted {
        let entry = managed_entry(name, &rules_dir, scope);
        managed_config::sync_managed_element(&config_path, EXCLUDES_KEY, &entry, true)?;
    }

    // A retired name the post-state still wants keeps its element: the same
    // rule re-recorded under another binding (or re-installed at a new pin)
    // retires its old output and records a new one in the same breath.
    //
    // The `exists` suppressor runs the opposite way to an ownership probe
    // (module doc): a record can be dropped while its tree stays on disk —
    // an output resolving outside its anchor root, or a shared footprint a
    // surviving sibling still references — and that tree still auto-loads,
    // so its exclusion must stay. Declining a removal can never delete a
    // user's element; do not "simplify" it away.
    let stale: BTreeSet<String> = support_dir_names(retired.iter(), &claude)
        .filter(|name| !wanted.contains(name))
        .filter(|name| !rules_dir.join(name).exists())
        .collect();

    for name in &stale {
        let entry = managed_entry(name, &rules_dir, scope);
        // A diagnostic trace, not a user-facing notice: the default
        // `EnvFilter` is `warn`, so this is only visible when `GRIM_LOG`
        // asks for it. Warn-level would fire on every ordinary uninstall,
        // and the removal only ever drops the exclusion of a support
        // directory grim itself installed and has just retired.
        tracing::info!(
            config = %config_path.display(),
            entry = %entry,
            "removing the grim-managed claudeMdExcludes element of a retired support directory"
        );
        managed_config::sync_managed_element(&config_path, EXCLUDES_KEY, &entry, false)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::install_state::InstallRecord;
    use crate::install::path_anchor::{AnchoredPath, PathAnchor};
    use crate::oci::pinned_identifier::PinnedIdentifier;
    use crate::oci::{Digest, Identifier};

    /// A recorded Claude rule output, with or without a support directory.
    fn rule_record(name: &str, client: &str, support: Option<&str>) -> InstallRecord {
        let pinned = PinnedIdentifier::try_from(
            Identifier::new_registry("acme/r", "localhost:5000").clone_with_digest(Digest::Sha256("a".repeat(64))),
        )
        .unwrap();
        InstallRecord {
            kind: ArtifactKind::Rule,
            name: name.to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned),
            dev: false,
            outputs: vec![ClientOutput {
                client: client.to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: format!(".claude/rules/{name}.md"),
                },
                content_hash: Digest::Sha256("b".repeat(64)),
                support_dir: support.map(|rel| AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: rel.to_string(),
                }),
                entry: None,
                adopted: false,
            }],
        }
    }

    /// The single output of [`rule_record`], as an operation would hand it to
    /// the sync after removing it from the state.
    fn retired(name: &str, client: &str, support: Option<&str>) -> ClientOutput {
        rule_record(name, client, support).outputs.remove(0)
    }

    /// A workspace with `.claude/rules/<name>/` on disk, as an install leaves it.
    fn workspace_with_support_dir(names: &[&str]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        for name in names {
            std::fs::create_dir_all(tmp.path().join(".claude").join("rules").join(name)).unwrap();
        }
        tmp
    }

    /// Delete a support directory, as the operation whose sync follows has
    /// already done on disk. The removal side declines to strip the exclusion
    /// of a tree that is still there, so a test asserting a removal has to be
    /// honest about the filesystem.
    fn delete_support_dir(ws: &Path, name: &str) {
        std::fs::remove_dir_all(ws.join(".claude").join("rules").join(name)).unwrap();
    }

    fn excludes(config: &Path) -> serde_json::Value {
        let raw = std::fs::read_to_string(config).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        parsed.get(EXCLUDES_KEY).cloned().unwrap_or(serde_json::Value::Null)
    }

    fn state_at(ws: &Path) -> InstallState {
        InstallState::empty(&ws.join("state.json"))
    }

    #[test]
    fn managed_entry_is_worktree_portable_for_project_absolute_for_global() {
        let rules = Path::new("/home/u/.claude/rules");
        assert_eq!(
            managed_entry("r", rules, ConfigScope::Project),
            "**/.claude/rules/r/**",
            "one committed settings.json serves every worktree, so the project glob carries no absolute root"
        );
        assert_eq!(
            managed_entry("r", rules, ConfigScope::Global),
            "/home/u/.claude/rules/r/**"
        );
    }

    #[test]
    fn managed_entry_global_has_no_backslashes() {
        let rules = Path::new("C:\\Users\\dev\\.claude\\rules");
        let entry = managed_entry("r", rules, ConfigScope::Global);
        assert!(
            !entry.contains('\\'),
            "a Windows root must not leak `\\` into a glob, where it is an escape: {entry:?}"
        );
        assert_eq!(entry, "C:/Users/dev/.claude/rules/r/**");
    }

    #[test]
    fn a_recorded_support_dir_rule_registers_its_exclusion() {
        let tmp = workspace_with_support_dir(&["r"]);
        let ws = tmp.path();
        let mut state = state_at(ws);
        state.record(rule_record("r", "claude", Some(".claude/rules/r")));

        sync_for_state(&state, ws, ConfigScope::Project, &[]).unwrap();

        let config = ws.join(".claude").join(SETTINGS_FILE);
        assert_eq!(excludes(&config), serde_json::json!(["**/.claude/rules/r/**"]));

        // Idempotent: a second converge writes nothing new.
        sync_for_state(&state, ws, ConfigScope::Project, &[]).unwrap();
        assert_eq!(excludes(&config), serde_json::json!(["**/.claude/rules/r/**"]));
    }

    #[test]
    fn a_rule_without_a_support_dir_is_never_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let mut state = state_at(ws);
        state.record(rule_record("r", "claude", None));

        sync_for_state(&state, ws, ConfigScope::Project, &[]).unwrap();

        assert!(
            !ws.join(".claude").join(SETTINGS_FILE).exists(),
            "a single-file rule is the index Claude is supposed to load — nothing to exclude, no settings file to create"
        );
    }

    #[test]
    fn another_clients_support_dir_rule_is_never_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let mut state = state_at(ws);
        state.record(rule_record("r", "opencode", Some(".opencode/rules/r")));

        sync_for_state(&state, ws, ConfigScope::Project, &[]).unwrap();

        assert!(!ws.join(".claude").join(SETTINGS_FILE).exists());
    }

    #[test]
    fn a_user_authored_element_for_a_rule_grim_never_recorded_survives() {
        // The regression this module exists to prevent (grimoire-rs/grimoire#102
        // follow-up). A consumer's `settings.json` is git-tracked and holds
        // their own exclusions; a project that gitignores its support trees has
        // the directories absent on a fresh clone. Keying removal on "the
        // directory is gone" deleted both lines — and the emptied key with them.
        let tmp = workspace_with_support_dir(&["grim-owned"]);
        let ws = tmp.path();
        let config = ws.join(".claude").join(SETTINGS_FILE);
        std::fs::create_dir_all(ws.join(".claude")).unwrap();
        let original = concat!(
            "{\n",
            "  \"claudeMdExcludes\": [\n",
            "    \"**/.claude/rules/rust-cargo/**\",\n",
            "    \"**/.claude/rules/rust-quality/**\"\n",
            "  ]\n",
            "}\n",
        );
        std::fs::write(&config, original).unwrap();

        // A live recorded rule AND a retired one, so the splice really writes
        // (one element added, one removed) with the two hand-written elements
        // sitting beside it — a no-op sync would prove nothing about them.
        let mut state = state_at(ws);
        state.record(rule_record("grim-owned", "claude", Some(".claude/rules/grim-owned")));
        sync_for_state(
            &state,
            ws,
            ConfigScope::Project,
            &[retired("gone", "claude", Some(".claude/rules/gone"))],
        )
        .unwrap();

        assert_eq!(
            excludes(&config),
            serde_json::json!([
                "**/.claude/rules/rust-cargo/**",
                "**/.claude/rules/rust-quality/**",
                "**/.claude/rules/grim-owned/**"
            ]),
            "grim removes only what it can prove it retired; the other two are the user's"
        );
    }

    #[test]
    fn a_real_consumer_settings_file_is_untouched_byte_for_byte() {
        // Mirrors a real consumer's committed `.claude/settings.json`: two
        // hand-written exclusions beside `permissions` (whose `deny` list
        // carries `"// === … ==="` divider STRINGS, ordinary JSON that must
        // never be tidied) and `hooks`.
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let config = ws.join(".claude").join(SETTINGS_FILE);
        std::fs::create_dir_all(ws.join(".claude")).unwrap();
        let original = concat!(
            "{\n",
            "  \"claudeMdExcludes\": [\n",
            "    \"**/.claude/rules/rust-cargo/**\",\n",
            "    \"**/.claude/rules/rust-quality/**\"\n",
            "  ],\n",
            "  \"permissions\": {\n",
            "    \"allow\": [\n",
            "      \"Read\",\n",
            "      \"Grep\"\n",
            "    ],\n",
            "    \"deny\": [\n",
            "      \"// === Destructive File Operations ===\",\n",
            "      \"Bash(rm -rf:*)\",\n",
            "\n",
            "      \"// === Network ===\",\n",
            "      \"Bash(curl:*)\"\n",
            "    ]\n",
            "  },\n",
            "  \"enableAllProjectMcpServers\": true,\n",
            "  \"hooks\": {\n",
            "    \"Stop\": [\n",
            "      {\n",
            "        \"hooks\": [\n",
            "          { \"type\": \"command\", \"command\": \"uv run stop.py\", \"timeout\": 10 }\n",
            "        ]\n",
            "      }\n",
            "    ]\n",
            "  }\n",
            "}\n",
        );
        std::fs::write(&config, original).unwrap();

        sync_for_state(&state_at(ws), ws, ConfigScope::Project, &[]).unwrap();

        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            original,
            "a sync with nothing wanted and nothing retired must not write a byte"
        );
    }

    #[test]
    fn uninstall_removes_the_exclusion_and_drops_the_emptied_key() {
        let tmp = workspace_with_support_dir(&["r"]);
        let ws = tmp.path();
        let mut state = state_at(ws);
        state.record(rule_record("r", "claude", Some(".claude/rules/r")));
        sync_for_state(&state, ws, ConfigScope::Project, &[]).unwrap();

        // Uninstall: the files go first, then the record — and its output is
        // what the operation hands the sync as retired.
        delete_support_dir(ws, "r");
        state.remove(ArtifactKind::Rule, "r");
        sync_for_state(
            &state,
            ws,
            ConfigScope::Project,
            &[retired("r", "claude", Some(".claude/rules/r"))],
        )
        .unwrap();

        let config = ws.join(".claude").join(SETTINGS_FILE);
        let parsed: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert!(
            parsed.get(EXCLUDES_KEY).is_none(),
            "the emptied key is dropped, not left as []: {parsed}"
        );
    }

    #[test]
    fn uninstalling_one_of_two_rules_leaves_the_survivors_element() {
        let tmp = workspace_with_support_dir(&["alpha", "beta"]);
        let ws = tmp.path();
        let mut state = state_at(ws);
        state.record(rule_record("alpha", "claude", Some(".claude/rules/alpha")));
        state.record(rule_record("beta", "claude", Some(".claude/rules/beta")));
        sync_for_state(&state, ws, ConfigScope::Project, &[]).unwrap();

        delete_support_dir(ws, "alpha");
        state.remove(ArtifactKind::Rule, "alpha");
        sync_for_state(
            &state,
            ws,
            ConfigScope::Project,
            &[retired("alpha", "claude", Some(".claude/rules/alpha"))],
        )
        .unwrap();

        assert_eq!(
            excludes(&ws.join(".claude").join(SETTINGS_FILE)),
            serde_json::json!(["**/.claude/rules/beta/**"]),
            "only the retired rule's element goes; the survivor keeps its own"
        );
    }

    #[test]
    fn a_retired_output_for_another_client_removes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let config = ws.join(".claude").join(SETTINGS_FILE);
        std::fs::create_dir_all(ws.join(".claude")).unwrap();
        let original = r#"{"claudeMdExcludes": ["**/.claude/rules/r/**"]}"#;
        std::fs::write(&config, original).unwrap();

        // An OpenCode rule's support dir is a different tree entirely; its
        // removal says nothing about Claude's exclusion.
        sync_for_state(
            &state_at(ws),
            ws,
            ConfigScope::Project,
            &[retired("r", "opencode", Some(".opencode/rules/r"))],
        )
        .unwrap();

        assert_eq!(std::fs::read_to_string(&config).unwrap(), original);
    }

    #[test]
    fn a_retired_output_whose_name_is_not_an_artifact_name_removes_nothing() {
        // The blanket `**/.claude/rules/*/**` this module forbids must be
        // unreachable from the removal side too — a `*` support-dir name is
        // rejected (with a warning) by the same `SkillName::parse` filter the
        // add side uses, so it can never be interpolated into a removal.
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let config = ws.join(".claude").join(SETTINGS_FILE);
        std::fs::create_dir_all(ws.join(".claude")).unwrap();
        let original = r#"{"claudeMdExcludes": ["**/.claude/rules/*/**", "**/.claude/rules/mine/**"]}"#;
        std::fs::write(&config, original).unwrap();

        sync_for_state(
            &state_at(ws),
            ws,
            ConfigScope::Project,
            &[retired("blanket", "claude", Some(".claude/rules/*"))],
        )
        .unwrap();

        assert_eq!(std::fs::read_to_string(&config).unwrap(), original);
    }

    #[test]
    fn a_name_that_is_both_retired_and_wanted_keeps_its_element() {
        // A re-install at a new pin retires the old output and records a new
        // one in the same breath; a rebinding does the same under a different
        // record name. The directory never went away — the element must not.
        let tmp = workspace_with_support_dir(&["r"]);
        let ws = tmp.path();
        let mut state = state_at(ws);
        state.record(rule_record("r", "claude", Some(".claude/rules/r")));

        sync_for_state(
            &state,
            ws,
            ConfigScope::Project,
            &[retired("r", "claude", Some(".claude/rules/r"))],
        )
        .unwrap();

        assert_eq!(
            excludes(&ws.join(".claude").join(SETTINGS_FILE)),
            serde_json::json!(["**/.claude/rules/r/**"]),
            "wanted wins over retired: the post-state still installs this directory"
        );
    }

    #[test]
    fn a_non_string_exclusion_element_is_skipped_not_dropped() {
        // Hand-edited or future-schema content grim does not recognize:
        // skipping must mean "leave alone", never "drop".
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let config = ws.join(".claude").join(SETTINGS_FILE);
        std::fs::create_dir_all(ws.join(".claude")).unwrap();
        std::fs::write(
            &config,
            r#"{"claudeMdExcludes": [123, {"deep": true}, "**/.claude/rules/gone/**"]}"#,
        )
        .unwrap();

        sync_for_state(
            &state_at(ws),
            ws,
            ConfigScope::Project,
            &[retired("gone", "claude", Some(".claude/rules/gone"))],
        )
        .unwrap();

        assert_eq!(
            excludes(&config),
            serde_json::json!([123, {"deep": true}]),
            "only the retired managed element goes; the unrecognized elements stay"
        );
    }

    #[test]
    fn a_hand_written_exclusion_is_adopted_not_duplicated() {
        // The reporting project hand-wrote exactly this spelling as an
        // interim workaround — grim must find it present and leave it alone.
        let tmp = workspace_with_support_dir(&["r"]);
        let ws = tmp.path();
        let config = ws.join(".claude").join(SETTINGS_FILE);
        std::fs::write(
            &config,
            "{\n  \"claudeMdExcludes\": [\n    \"**/.claude/rules/r/**\"\n  ]\n}\n",
        )
        .unwrap();
        let original = std::fs::read_to_string(&config).unwrap();

        let mut state = state_at(ws);
        state.record(rule_record("r", "claude", Some(".claude/rules/r")));
        sync_for_state(&state, ws, ConfigScope::Project, &[]).unwrap();

        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            original,
            "an element already present is not rewritten, let alone duplicated"
        );
    }

    #[test]
    fn foreign_settings_keys_and_formatting_survive_the_splice() {
        let tmp = workspace_with_support_dir(&["r"]);
        let ws = tmp.path();
        let config = ws.join(".claude").join(SETTINGS_FILE);
        let original = concat!(
            "{\n",
            "  \"permissions\": {\n",
            "    \"deny\": [\"// never touch prod\", \"Bash(rm:*)\"]\n",
            "  },\n",
            "  \"hooks\": { \"Stop\": [] },\n",
            "  \"model\": \"opus\"\n",
            "}\n",
        );
        std::fs::write(&config, original).unwrap();

        let mut state = state_at(ws);
        state.record(rule_record("r", "claude", Some(".claude/rules/r")));
        sync_for_state(&state, ws, ConfigScope::Project, &[]).unwrap();

        let written = std::fs::read_to_string(&config).unwrap();
        assert!(
            written.contains("\"// never touch prod\""),
            "a `//`-prefixed string entry is ordinary JSON, never tidied: {written}"
        );
        assert!(
            written.contains("\"hooks\": { \"Stop\": [] },"),
            "formatting preserved: {written}"
        );
        // A splice into an ordinary settings file leaves strict JSON.
        let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(parsed["model"], "opus");
        assert_eq!(parsed["permissions"]["deny"][1], "Bash(rm:*)");
        assert_eq!(parsed[EXCLUDES_KEY], serde_json::json!(["**/.claude/rules/r/**"]));
    }

    #[test]
    fn a_jsonc_comment_in_the_settings_file_survives_the_splice() {
        let tmp = workspace_with_support_dir(&["r"]);
        let ws = tmp.path();
        let config = ws.join(".claude").join(SETTINGS_FILE);
        std::fs::write(
            &config,
            concat!(
                "{\n",
                "  // team policy: do not reformat\n",
                "  \"model\": \"opus\"\n",
                "}\n"
            ),
        )
        .unwrap();

        let mut state = state_at(ws);
        state.record(rule_record("r", "claude", Some(".claude/rules/r")));
        sync_for_state(&state, ws, ConfigScope::Project, &[]).unwrap();

        let written = std::fs::read_to_string(&config).unwrap();
        assert!(
            written.contains("// team policy: do not reformat"),
            "Claude reads JSONC, so a real line comment must survive: {written}"
        );
        // Still JSONC, so read it back through the JSONC-tolerant parser
        // rather than plain `serde_json::from_str`.
        let parsed: serde_json::Value =
            serde_json::from_str(&crate::install::json_config::sanitize_jsonc(&written)).unwrap();
        assert_eq!(parsed["model"], "opus");
        assert_eq!(parsed[EXCLUDES_KEY], serde_json::json!(["**/.claude/rules/r/**"]));
    }

    #[test]
    fn an_unparseable_settings_file_is_refused_on_add_and_tolerated_on_remove() {
        let tmp = workspace_with_support_dir(&["r"]);
        let ws = tmp.path();
        let config = ws.join(".claude").join(SETTINGS_FILE);
        let garbage = "not json at all {{{";
        std::fs::write(&config, garbage).unwrap();

        let mut state = state_at(ws);
        state.record(rule_record("r", "claude", Some(".claude/rules/r")));
        let err = sync_for_state(&state, ws, ConfigScope::Project, &[]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            garbage,
            "grim never clobbers a settings file it cannot parse"
        );

        // Nothing to add, one thing to retire: the removal is tolerant.
        delete_support_dir(ws, "r");
        sync_for_state(
            &state_at(ws),
            ws,
            ConfigScope::Project,
            &[retired("r", "claude", Some(".claude/rules/r"))],
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(&config).unwrap(), garbage);
    }

    #[test]
    fn the_support_dir_record_names_the_directory_not_the_binding() {
        // `grim add --name` installs a rule under its binding name, so the
        // recorded support-dir path — not the record name — is the authority.
        let tmp = workspace_with_support_dir(&["on-disk"]);
        let ws = tmp.path();
        let mut state = state_at(ws);
        state.record(rule_record("binding", "claude", Some(".claude/rules/on-disk")));

        sync_for_state(&state, ws, ConfigScope::Project, &[]).unwrap();

        assert_eq!(
            excludes(&ws.join(".claude").join(SETTINGS_FILE)),
            serde_json::json!(["**/.claude/rules/on-disk/**"])
        );
    }

    #[test]
    fn every_support_dir_rule_gets_its_own_exclusion() {
        // The reporting consumer had two support trees. Each add is its own
        // read-modify-write cycle over the file the previous one wrote, so a
        // second rule is the case a single-rule suite can never exercise.
        let tmp = workspace_with_support_dir(&["alpha", "beta"]);
        let ws = tmp.path();
        let mut state = state_at(ws);
        state.record(rule_record("alpha", "claude", Some(".claude/rules/alpha")));
        state.record(rule_record("beta", "claude", Some(".claude/rules/beta")));

        sync_for_state(&state, ws, ConfigScope::Project, &[]).unwrap();

        // `wanted` is a `BTreeSet`, so the append order is the name order.
        assert_eq!(
            excludes(&ws.join(".claude").join(SETTINGS_FILE)),
            serde_json::json!(["**/.claude/rules/alpha/**", "**/.claude/rules/beta/**"]),
            "every support-dir rule keeps its own element; iteration 2 must not clobber iteration 1"
        );
    }

    #[test]
    fn a_support_dir_name_that_is_not_an_artifact_name_registers_nothing() {
        // A directly declared `[rule]` key is never name-validated, so a
        // repo-committed `grimoire.toml` can carry `"*"` — which would
        // interpolate into the blanket `**/.claude/rules/*/**` this module
        // forbids, suppressing the consumer's own rules subdirectories.
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let mut state = state_at(ws);
        state.record(rule_record("blanket", "claude", Some(".claude/rules/*")));

        sync_for_state(&state, ws, ConfigScope::Project, &[]).unwrap();

        assert!(
            !ws.join(".claude").join(SETTINGS_FILE).exists(),
            "a name grim would never install must never reach the glob"
        );
    }

    #[test]
    fn root_for_declines_a_global_scope_that_resolves_nowhere() {
        let ws = Path::new("/w");
        assert_eq!(
            root_for(ws, ConfigScope::Global, None, None),
            None,
            "no override and no home: decline, never inherit the render path's workspace fallback"
        );
        assert_eq!(
            root_for(ws, ConfigScope::Global, Some(PathBuf::from("/custom/cc")), None),
            Some(PathBuf::from("/custom/cc")),
            "CLAUDE_CONFIG_DIR replaces the whole tree"
        );
        assert_eq!(
            root_for(ws, ConfigScope::Global, None, Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/home/u/.claude"))
        );
        assert_eq!(
            root_for(ws, ConfigScope::Project, None, None),
            Some(PathBuf::from("/w/.claude")),
            "project scope resolves from the workspace alone, never from the environment"
        );
    }

    #[test]
    fn a_declined_root_writes_no_settings_file_into_the_workspace() {
        // The render path falls back to `<workspace>/.claude` where no global
        // root resolves. The sync must not follow it there: a
        // machine-absolute glob baked into a repository-local settings file
        // is exactly what the decline exists to prevent.
        let tmp = workspace_with_support_dir(&["r"]);
        let ws = tmp.path();
        let mut state = state_at(ws);
        state.record(rule_record("r", "claude", Some(".claude/rules/r")));

        sync_at_root(None, &state, ConfigScope::Global, &[]).unwrap();

        assert!(
            !ws.join(".claude").join(SETTINGS_FILE).exists(),
            "a declined root writes nothing at all, least of all into the workspace"
        );
    }

    #[test]
    fn a_settings_path_that_cannot_be_written_fails_the_removal() {
        // A directory where the settings file belongs is neither absent nor
        // parseable: a removal that cannot be carried out must surface, not
        // report success. (With nothing retired there is no read at all — the
        // failure is reachable only once grim has something to deregister.)
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        std::fs::create_dir_all(ws.join(".claude").join(SETTINGS_FILE)).unwrap();

        let err = sync_for_state(
            &state_at(ws),
            ws,
            ConfigScope::Project,
            &[retired("r", "claude", Some(".claude/rules/r"))],
        )
        .unwrap_err();
        assert_ne!(
            err.kind(),
            io::ErrorKind::NotFound,
            "an unreadable settings file propagates rather than converging: {err}"
        );
    }

    #[test]
    fn an_update_that_drops_the_support_dir_deregisters_it() {
        let tmp = workspace_with_support_dir(&["r"]);
        let ws = tmp.path();
        let mut state = state_at(ws);
        state.record(rule_record("r", "claude", Some(".claude/rules/r")));
        sync_for_state(&state, ws, ConfigScope::Project, &[]).unwrap();

        // v2 of the same rule ships no support directory: the record stays
        // live under the same name while its support-dir output is replaced,
        // so the retired output is what carries the directory's name — the
        // only case where a *surviving* record loses its element. The
        // installer's own `cleanup` already deleted the stale tree by the
        // time the sync runs, so delete it here too — otherwise the
        // suppressor keeps the element and this passes for the wrong reason.
        delete_support_dir(ws, "r");
        state.record(rule_record("r", "claude", None));
        sync_for_state(
            &state,
            ws,
            ConfigScope::Project,
            &[retired("r", "claude", Some(".claude/rules/r"))],
        )
        .unwrap();

        let config = ws.join(".claude").join(SETTINGS_FILE);
        let parsed: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert!(
            parsed.get(EXCLUDES_KEY).is_none(),
            "the element goes with the support directory, not with the record: {parsed}"
        );
    }

    #[test]
    fn a_retired_output_whose_directory_is_still_present_keeps_its_element() {
        // Three paths drop a record while deliberately LEAVING the tree on
        // disk: an output resolving outside its anchor root, in `uninstall`
        // and in `prune::reap_dropped_clients` (`prune_orphans` reaches the
        // second through the first), and a shared footprint a surviving
        // sibling still references. The tree still auto-loads there, so
        // removing its exclusion is grimoire#102 all over again — silently.
        let tmp = workspace_with_support_dir(&["r"]);
        let ws = tmp.path();
        let mut state = state_at(ws);
        state.record(rule_record("r", "claude", Some(".claude/rules/r")));
        sync_for_state(&state, ws, ConfigScope::Project, &[]).unwrap();

        // The record goes; the directory does not.
        state.remove(ArtifactKind::Rule, "r");
        sync_for_state(
            &state,
            ws,
            ConfigScope::Project,
            &[retired("r", "claude", Some(".claude/rules/r"))],
        )
        .unwrap();

        assert_eq!(
            excludes(&ws.join(".claude").join(SETTINGS_FILE)),
            serde_json::json!(["**/.claude/rules/r/**"]),
            "the support tree is still on disk and still auto-loads; its exclusion must stay"
        );
    }

    #[test]
    fn a_non_array_excludes_key_is_refused_on_add_and_never_rewritten() {
        let tmp = workspace_with_support_dir(&["r"]);
        let ws = tmp.path();
        let config = ws.join(".claude").join(SETTINGS_FILE);
        let original = r#"{"claudeMdExcludes": "**/x/**"}"#;
        std::fs::write(&config, original).unwrap();

        let mut state = state_at(ws);
        state.record(rule_record("r", "claude", Some(".claude/rules/r")));
        let err = sync_for_state(&state, ws, ConfigScope::Project, &[]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            original,
            "grim never clobbers an unknown-schema `claudeMdExcludes`"
        );

        // Removal against the same unknown schema converges instead of failing.
        delete_support_dir(ws, "r");
        sync_for_state(
            &state_at(ws),
            ws,
            ConfigScope::Project,
            &[retired("r", "claude", Some(".claude/rules/r"))],
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(&config).unwrap(), original);
    }

    #[test]
    fn the_managed_prefix_and_the_vendors_rule_path_share_one_rules_segment() {
        use super::super::vendor::Vendor;

        // `PROJECT_PREFIX` spells `rules/` a second time. If the vendor ever
        // moves that segment, the element grim writes stops naming the
        // directory grim installs — and every element written before the move
        // becomes unremovable, because removal matches the recomputed spelling
        // exactly. So pin them together.
        let ws = Path::new("/w");
        let rules =
            super::super::vendor_claude::rules_dir(&super::super::vendor_claude::scope_root(ws, ConfigScope::Project));
        let element = managed_entry("r", &rules, ConfigScope::Project);
        let tail = element
            .strip_prefix("**/")
            .and_then(|e| e.strip_suffix(ENTRY_SUFFIX))
            .expect("the project element is prefix + name + suffix");

        assert!(
            managed_config::glob_path(&rules.join("r")).ends_with(tail),
            "the element names the very directory `rule_path` writes beside: {element} vs {rules:?}"
        );
        assert!(
            managed_config::glob_path(&super::super::vendor_claude::ClaudeVendor.rule_path(
                ws,
                ConfigScope::Project,
                "r"
            ))
            .ends_with(".claude/rules/r.md"),
            "and that directory is the vendor's own rules dir"
        );
    }
}
