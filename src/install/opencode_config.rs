// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Managed registration of grim's rule glob in OpenCode's `instructions`
//! config.
//!
//! OpenCode has no per-file rule scoping and no `rules/` directory of its
//! own: instruction files load through `AGENTS.md` or the `instructions`
//! array (paths / globs / URLs) in `opencode.json`. grim therefore writes
//! rules to `.opencode/rules/<name>.md` **and** keeps exactly one managed
//! glob entry in the vendor config pointing at that directory — added
//! when the first OpenCode rule installs, removed when the last one
//! uninstalls (the reversible config-registration pattern from the hooks
//! ADR).
//!
//! Config resolution mirrors OpenCode's own:
//! - **project** scope edits `<workspace>/opencode.jsonc` when present,
//!   else `<workspace>/opencode.json`, with a workspace-relative glob;
//! - **global** scope edits `$OPENCODE_CONFIG` when set, else
//!   `$XDG_CONFIG_HOME/opencode/opencode.json` (default
//!   `~/.config/opencode/opencode.json`), with an absolute glob rooted at
//!   `$GRIM_HOME` (the global install workspace).
//!
//! `$OPENCODE_CONFIG` (a config **file** path) and `$OPENCODE_CONFIG_DIR`
//! (OpenCode's additive skills/agents scan **directory**, honored by
//! [`super::vendor_opencode`]'s `skills_root`) are orthogonal variables —
//! only the former matters here.
//!
//! Edits are conservative: a config that does not parse (even after
//! JSONC comment / trailing-comma stripping) is **never** rewritten —
//! the sync fails rather than clobbering user content. The edit itself
//! goes through the span-preserving [`super::json_splice`] engine — the
//! same one that writes MCP entries into this very file — so every byte
//! outside the managed `instructions` element (key order, formatting,
//! JSONC comments) survives untouched.

use std::io;
use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::install::install_state::InstallState;
use crate::oci::ArtifactKind;
use crate::store::atomic_write;

use super::client_target::ClientTarget;
use super::json_config::with_path;
use super::json_splice::{self, Splice};

/// The workspace-relative glob grim manages for project-scope installs.
pub const MANAGED_PROJECT_GLOB: &str = ".opencode/rules/*.md";

/// The root config key holding OpenCode's instruction paths / globs / URLs.
const INSTRUCTIONS_KEY: &str = "instructions";

/// What a sync did to the vendor config.
///
/// Closed internal enum — matches stay total, no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstructionsSync {
    /// The managed glob was appended to `instructions`.
    Added,
    /// The managed glob was removed (and an emptied `instructions` key
    /// dropped).
    Removed,
    /// The config already matched the desired state — no write.
    Unchanged,
}

/// The managed `instructions` entry for an install scope rooted at
/// `workspace`: workspace-relative for a project config (which sits at
/// the workspace root), absolute for the global config (which does not).
pub fn managed_entry(workspace: &Path, scope: ConfigScope) -> String {
    match scope {
        ConfigScope::Project => MANAGED_PROJECT_GLOB.to_string(),
        // OpenCode (a Node/JS tool) reads `instructions` entries as globs;
        // JS glob engines treat `\` as an escape character, not a
        // separator, so the entry must stay forward-slash even when
        // `workspace` and the native join produce backslashes on Windows.
        ConfigScope::Global => workspace
            .join(MANAGED_PROJECT_GLOB)
            .to_string_lossy()
            .replace('\\', "/"),
    }
}

/// Resolve the OpenCode config file grim manages for `scope`, or `None`
/// when the global location cannot be determined (no `$OPENCODE_CONFIG`,
/// `$XDG_CONFIG_HOME`, or `$HOME`) — mirroring the other vendors'
/// no-`$HOME` handling, the sync is skipped rather than writing to a
/// CWD-relative path.
pub fn config_path_for_scope(workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
    match scope {
        ConfigScope::Project => Some(project_config_path(workspace)),
        // `env_dir` treats an empty value as unset — same convention as
        // every other vendor env override.
        ConfigScope::Global => global_config_path(
            super::vendor::env_dir("OPENCODE_CONFIG"),
            super::vendor::env_dir("XDG_CONFIG_HOME"),
            super::vendor::home_dir(),
        ),
    }
}

/// The directory holding the **global** OpenCode config file grim edits —
/// the anchor root for a global OpenCode MCP output.
///
/// Derived from the same resolution [`config_path_for_scope`] uses, so the
/// anchor and the write path can never disagree. They must not be derived
/// separately: `$OPENCODE_CONFIG` names an arbitrary config **file**, while
/// `$OPENCODE_CONFIG_DIR` moves the *skills* root (and with it
/// [`PathAnchor::OpenCodeRoot`](super::path_anchor::PathAnchor)) without
/// moving the config file, so either variable makes the two locations
/// diverge.
///
/// `None` whenever [`config_path_for_scope`] resolves nothing, or the
/// resolved path has no parent (a bare relative filename).
pub fn global_config_dir(
    env_override: Option<PathBuf>,
    xdg_config_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    let parent = global_config_path(env_override, xdg_config_home, home)?
        .parent()?
        .to_path_buf();
    // `Path::parent` of a bare `oc.json` is an empty path, which would anchor
    // every record at the process CWD.
    (!parent.as_os_str().is_empty()).then_some(parent)
}

/// The project-scope config: `opencode.jsonc` when present (OpenCode
/// supports both spellings), else `opencode.json`.
fn project_config_path(workspace: &Path) -> PathBuf {
    let jsonc = workspace.join("opencode.jsonc");
    if jsonc.is_file() {
        jsonc
    } else {
        workspace.join("opencode.json")
    }
}

/// The global-scope config: `$OPENCODE_CONFIG` wins (it is OpenCode's own
/// "custom config file path" override), else the XDG default. `None` when
/// no variable resolves a location — a relative fallback would silently
/// land the edit wherever the process happens to run.
fn global_config_path(
    env_override: Option<PathBuf>,
    xdg_config_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(path) = env_override {
        return Some(path);
    }
    let config_dir = xdg_config_home.or_else(|| home.map(|h| h.join(".config")))?;
    Some(config_dir.join("opencode").join("opencode.json"))
}

/// Converge the vendor config on the state's needs: ensure the managed
/// glob is present while any OpenCode rule is recorded for this scope,
/// absent otherwise. With no OpenCode rule left, the now-empty managed
/// `.opencode/rules/` directory is reaped too (best-effort — a non-empty
/// dir is never touched). Call after install/update/uninstall mutated
/// `state`.
///
/// # Errors
///
/// An I/O failure reading/writing the config, or `InvalidData` when the
/// existing config cannot be parsed (grim refuses to clobber it).
pub fn sync_for_state(state: &InstallState, workspace: &Path, scope: ConfigScope) -> io::Result<InstructionsSync> {
    let opencode = ClientTarget::OpenCode.to_string();
    let want = state
        .iter_records()
        .any(|r| r.kind == ArtifactKind::Rule && r.outputs.iter().any(|c| c.client == opencode));
    // The managed rules dir mirrors the managed glob: when the last
    // OpenCode rule for this scope is gone, reap the now-empty
    // `.opencode/rules/` directory (it exists only because a rule install
    // created it). `remove_dir` refuses a non-empty dir, so user files
    // are never touched; that refusal — and an already-absent dir — are
    // deliberately ignored (best-effort hygiene, never a sync failure).
    if !want {
        let _ = std::fs::remove_dir(workspace.join(".opencode").join("rules"));
    }
    // No resolvable config location (global scope without $OPENCODE_CONFIG,
    // $XDG_CONFIG_HOME, or $HOME): skip the sync rather than invent a
    // CWD-relative path — the same degradation as the install paths.
    let Some(config_path) = config_path_for_scope(workspace, scope) else {
        return Ok(InstructionsSync::Unchanged);
    };
    let entry = managed_entry(workspace, scope);
    sync_managed_instruction(&config_path, &entry, want)
}

/// Idempotently add (`want = true`) or remove (`want = false`) the managed
/// `entry` in the `instructions` array of the config at `config_path`.
///
/// - Adding creates the file (`{"instructions": [entry]}`) when absent.
/// - Removing an entry from an absent/never-registered config is a no-op.
/// - Other config keys and other `instructions` entries are preserved.
///
/// Removal (`want == false`) is tolerant: an absent, unparseable, or
/// wrong-typed (`instructions` not an array) config has nothing grim-managed
/// to remove, so it converges as [`InstructionsSync::Unchanged`] rather than
/// failing. Adding (`want == true`) stays strict — grim never rewrites a file
/// it cannot parse or whose `instructions` is an unexpected type.
///
/// # Errors
///
/// An I/O failure, or — **only when adding** (`want == true`) — `InvalidData`
/// when the existing content is not a JSON/JSONC object, or its `instructions`
/// key is not an array (grim never clobbers an unknown-schema file).
pub fn sync_managed_instruction(config_path: &Path, entry: &str, want: bool) -> io::Result<InstructionsSync> {
    // A missing file reads as empty text — the splice engine's own
    // "no document yet" case, which emits the minimal skeleton on add and
    // is a no-op on remove.
    let raw = match std::fs::read_to_string(config_path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(with_path(config_path, e)),
    };

    let spliced = if want {
        json_splice::upsert_array_element(&raw, INSTRUCTIONS_KEY, entry)
    } else {
        json_splice::remove_array_element(&raw, INSTRUCTIONS_KEY, entry)
    };
    let spliced = match spliced {
        Ok(splice) => splice,
        // Removal is tolerant (`want == false`): a config grim cannot parse —
        // or whose `instructions` is not an array — has nothing grim-managed
        // to remove, so converge as `Unchanged` rather than fail a command
        // whose primary action already ran. Adding stays strict (never
        // rewrite an unknown-schema file).
        Err(_) if !want => return Ok(InstructionsSync::Unchanged),
        Err(e) => return Err(with_path(config_path, e)),
    };

    match spliced {
        Splice::Unchanged => Ok(InstructionsSync::Unchanged),
        Splice::Changed(text) => {
            atomic_write(config_path, text.as_bytes()).map_err(|e| with_path(config_path, e))?;
            Ok(if want {
                InstructionsSync::Added
            } else {
                InstructionsSync::Removed
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_creates_file_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("opencode.json");

        let first = sync_managed_instruction(&cfg, ".opencode/rules/*.md", true).unwrap();
        assert_eq!(first, InstructionsSync::Added);
        let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(doc["instructions"][0], ".opencode/rules/*.md");

        let second = sync_managed_instruction(&cfg, ".opencode/rules/*.md", true).unwrap();
        assert_eq!(second, InstructionsSync::Unchanged);
    }

    /// Regression: the managed `instructions` entry is spliced in place, so
    /// every byte outside it — comments, key order, formatting — survives.
    /// The sync used to reserialize the whole document through serde, which
    /// dropped JSONC comments, reflowed the file, and alphabetized every key
    /// (`serde_json::Map` is a `BTreeMap` in this build), rewriting a
    /// user-owned config grim only meant to register one glob in.
    #[test]
    fn sync_preserves_comments_key_order_and_formatting() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("opencode.jsonc");
        let original = concat!(
            "{\n",
            "  // which model to use\n",
            "  \"model\": \"anthropic/claude\",\n",
            "  \"zulu\":   true,\n",
            "  \"instructions\": [\n",
            "    \"CONTRIBUTING.md\"\n",
            "  ],\n",
            "  \"alpha\": 1\n",
            "}\n",
        );
        std::fs::write(&cfg, original).unwrap();

        assert_eq!(
            sync_managed_instruction(&cfg, ".opencode/rules/*.md", true).unwrap(),
            InstructionsSync::Added
        );
        let added = std::fs::read_to_string(&cfg).unwrap();
        assert!(added.contains("// which model to use"), "comment preserved: {added}");
        assert!(
            added.find("\"zulu\"") < added.find("\"alpha\""),
            "authored key order preserved, not alphabetized: {added}"
        );
        assert!(added.contains("\"zulu\":   true"), "formatting preserved: {added}");
        assert!(
            added.contains("\".opencode/rules/*.md\""),
            "managed entry added: {added}"
        );
        assert!(
            added.contains("\"CONTRIBUTING.md\""),
            "sibling entry preserved: {added}"
        );

        // Removing what was just added restores the original bytes exactly —
        // the strongest span-preservation invariant.
        assert_eq!(
            sync_managed_instruction(&cfg, ".opencode/rules/*.md", false).unwrap(),
            InstructionsSync::Removed
        );
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            original,
            "remove undoes add byte-for-byte"
        );
    }

    #[test]
    fn remove_preserves_other_entries_and_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("opencode.json");
        std::fs::write(
            &cfg,
            r#"{"model": "anthropic/claude", "instructions": ["CONTRIBUTING.md", ".opencode/rules/*.md"]}"#,
        )
        .unwrap();

        let out = sync_managed_instruction(&cfg, ".opencode/rules/*.md", false).unwrap();
        assert_eq!(out, InstructionsSync::Removed);
        let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(doc["model"], "anthropic/claude", "unrelated keys preserved");
        assert_eq!(doc["instructions"], serde_json::json!(["CONTRIBUTING.md"]));
    }

    #[test]
    fn remove_last_entry_drops_the_key_and_absent_file_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("opencode.json");
        std::fs::write(&cfg, r#"{"instructions": [".opencode/rules/*.md"]}"#).unwrap();

        let out = sync_managed_instruction(&cfg, ".opencode/rules/*.md", false).unwrap();
        assert_eq!(out, InstructionsSync::Removed);
        let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert!(doc.get("instructions").is_none(), "emptied key dropped");

        // Remove against a config that never existed: converges, no file.
        let missing = tmp.path().join("never.json");
        let out = sync_managed_instruction(&missing, "x", false).unwrap();
        assert_eq!(out, InstructionsSync::Unchanged);
        assert!(!missing.exists());
    }

    #[test]
    fn jsonc_comments_and_trailing_commas_parse_but_unparseable_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("opencode.jsonc");
        std::fs::write(
            &cfg,
            "{\n  // the model\n  \"model\": \"a/b\", /* block */\n  \"instructions\": [\"x.md\",],\n}\n",
        )
        .unwrap();
        let out = sync_managed_instruction(&cfg, "g", true).unwrap();
        assert_eq!(out, InstructionsSync::Added);
        // The written file is still JSONC — its comments and trailing commas
        // survive the splice — so it is read back through the JSONC-tolerant
        // parser, not plain `serde_json::from_str`.
        let written = std::fs::read_to_string(&cfg).unwrap();
        assert!(written.contains("// the model"), "line comment survives: {written}");
        assert!(written.contains("/* block */"), "block comment survives: {written}");
        let doc: serde_json::Value =
            serde_json::from_str(&crate::install::json_config::sanitize_jsonc(&written)).unwrap();
        assert_eq!(doc["model"], "a/b");
        assert_eq!(doc["instructions"], serde_json::json!(["x.md", "g"]));

        let broken = tmp.path().join("broken.json");
        std::fs::write(&broken, "not json at all {{{").unwrap();
        let err = sync_managed_instruction(&broken, "g", true).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            std::fs::read_to_string(&broken).unwrap(),
            "not json at all {{{",
            "unparseable config must never be rewritten"
        );
    }

    // ── C6/C7: tolerant removal, strict add ─────────────────────────────────

    /// C6: removing the managed glob from an unparseable config converges as
    /// `Unchanged` (nothing grim-managed to remove) and never rewrites it.
    #[test]
    fn remove_tolerates_unparseable_opencode_config() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("opencode.json");
        let garbage = "not json at all {{{";
        std::fs::write(&cfg, garbage).unwrap();

        let out = sync_managed_instruction(&cfg, ".opencode/rules/*.md", false).unwrap();
        assert_eq!(out, InstructionsSync::Unchanged);
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            garbage,
            "an unparseable config must never be rewritten, even on removal"
        );
    }

    /// C7: removing the managed glob when `instructions` is not an array
    /// converges as `Unchanged` rather than hard-failing.
    #[test]
    fn remove_tolerates_non_array_instructions() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("opencode.json");
        let body = r#"{"instructions": "x"}"#;
        std::fs::write(&cfg, body).unwrap();

        let out = sync_managed_instruction(&cfg, ".opencode/rules/*.md", false).unwrap();
        assert_eq!(out, InstructionsSync::Unchanged);
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            body,
            "a non-array instructions value is left untouched on removal"
        );
    }

    /// C6/C7 guard: adding stays strict — an unparseable config is refused
    /// (never clobbered) so an unknown schema is preserved.
    #[test]
    fn add_rejects_unparseable_config() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("opencode.json");
        let garbage = "not json at all {{{";
        std::fs::write(&cfg, garbage).unwrap();

        let err = sync_managed_instruction(&cfg, ".opencode/rules/*.md", true).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            garbage,
            "adding must never clobber an unparseable config"
        );
    }

    #[test]
    fn managed_entry_is_relative_for_project_absolute_for_global() {
        let ws = Path::new("/data/grim-home");
        assert_eq!(managed_entry(ws, ConfigScope::Project), ".opencode/rules/*.md");
        assert_eq!(
            managed_entry(ws, ConfigScope::Global),
            "/data/grim-home/.opencode/rules/*.md"
        );
    }

    /// Regression: a Windows-style workspace path must not leak `\` into
    /// the OpenCode glob entry — JS glob engines treat `\` as an escape
    /// character, not a separator.
    #[test]
    fn managed_entry_global_has_no_backslashes() {
        let ws = Path::new("C:\\Users\\dev\\grim-home");
        let entry = managed_entry(ws, ConfigScope::Global);
        assert!(
            !entry.contains('\\'),
            "glob entry must be forward-slash only, got {entry:?}"
        );
        assert_eq!(entry, "C:/Users/dev/grim-home/.opencode/rules/*.md");
    }

    #[test]
    fn global_config_path_resolution_order() {
        assert_eq!(
            global_config_path(Some(PathBuf::from("/custom/oc.json")), None, None),
            Some(PathBuf::from("/custom/oc.json")),
            "OPENCODE_CONFIG wins"
        );
        assert_eq!(
            global_config_path(None, Some(PathBuf::from("/xdg")), Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/xdg/opencode/opencode.json"))
        );
        assert_eq!(
            global_config_path(None, None, Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/home/u/.config/opencode/opencode.json"))
        );
        assert_eq!(
            global_config_path(None, None, None),
            None,
            "no variable at all: skip the sync, never a CWD-relative path"
        );
    }

    #[test]
    fn project_config_prefers_existing_jsonc() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(project_config_path(tmp.path()), tmp.path().join("opencode.json"));
        std::fs::write(tmp.path().join("opencode.jsonc"), "{}\n").unwrap();
        assert_eq!(project_config_path(tmp.path()), tmp.path().join("opencode.jsonc"));
    }

    #[test]
    fn sync_for_state_adds_only_when_an_opencode_rule_is_recorded() {
        use crate::install::install_state::{ClientOutput, InstallRecord};
        use crate::install::path_anchor::{AnchoredPath, PathAnchor};
        use crate::oci::pinned_identifier::PinnedIdentifier;
        use crate::oci::{Digest, Identifier};

        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let pinned = PinnedIdentifier::try_from(
            Identifier::new_registry("acme/r", "localhost:5000").clone_with_digest(Digest::Sha256("a".repeat(64))),
        )
        .unwrap();

        let mut state = InstallState::empty(&ws.join("state.json"));
        // No opencode rule yet ⇒ no write.
        assert_eq!(
            sync_for_state(&state, ws, ConfigScope::Project).unwrap(),
            InstructionsSync::Unchanged
        );
        assert!(!ws.join("opencode.json").exists());

        // Record an opencode rule using `outputs` (the V2 field; no denorm fields).
        state.record(InstallRecord {
            kind: ArtifactKind::Rule,
            name: "r".to_string(),
            source: crate::lock::locked_source::LockedSource::Registry(pinned),
            dev: false,
            outputs: vec![ClientOutput {
                client: "opencode".to_string(),
                target: AnchoredPath {
                    anchor: PathAnchor::Workspace,
                    relative: ".opencode/rules/r.md".to_string(),
                },
                content_hash: Digest::Sha256("b".repeat(64)),
                support_dir: None,
                entry: None,
            }],
        });
        assert_eq!(
            sync_for_state(&state, ws, ConfigScope::Project).unwrap(),
            InstructionsSync::Added
        );

        state.remove(ArtifactKind::Rule, "r");
        assert_eq!(
            sync_for_state(&state, ws, ConfigScope::Project).unwrap(),
            InstructionsSync::Removed
        );
    }

    #[test]
    fn sync_for_state_reaps_empty_rules_dir_but_never_user_files() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let rules_dir = ws.join(".opencode").join("rules");

        // Empty managed dir + no opencode rule recorded ⇒ reaped.
        std::fs::create_dir_all(&rules_dir).unwrap();
        let state = InstallState::empty(&ws.join("state.json"));
        sync_for_state(&state, ws, ConfigScope::Project).unwrap();
        assert!(!rules_dir.exists(), "empty rules dir is reaped");
        assert!(ws.join(".opencode").exists(), "only the rules dir itself goes");

        // A dir holding user files is never touched.
        std::fs::create_dir_all(&rules_dir).unwrap();
        std::fs::write(rules_dir.join("mine.md"), "user content\n").unwrap();
        sync_for_state(&state, ws, ConfigScope::Project).unwrap();
        assert!(rules_dir.join("mine.md").is_file(), "non-empty dir is preserved");

        // An absent dir stays a silent no-op (idempotent).
        std::fs::remove_file(rules_dir.join("mine.md")).unwrap();
        std::fs::remove_dir(&rules_dir).unwrap();
        sync_for_state(&state, ws, ConfigScope::Project).unwrap();
        assert!(!rules_dir.exists());
    }

    #[test]
    fn written_config_stays_valid_json_and_preserves_foreign_keys() {
        // Contract pin: every write is a span-preserving splice that leaves
        // the file valid JSON with its foreign keys byte-intact. This used to
        // pin serde's pretty printer instead — the whole-file reserialize that
        // reordered and reflowed a user-owned config on every rule install.
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("opencode.json");
        let original = "{\"$schema\": \"https://opencode.ai/config.json\"}";
        std::fs::write(&cfg, original).unwrap();

        sync_managed_instruction(&cfg, ".opencode/rules/*.md", true).unwrap();
        let added = std::fs::read_to_string(&cfg).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&added).unwrap();
        assert_eq!(doc["$schema"], "https://opencode.ai/config.json");
        assert_eq!(doc["instructions"], serde_json::json!([".opencode/rules/*.md"]));
        assert!(
            added.contains("\"$schema\": \"https://opencode.ai/config.json\""),
            "the foreign key's own spelling survives verbatim: {added}"
        );

        // Removing the last managed entry drops the whole `instructions`
        // member (no `[]` husk) and restores the original bytes.
        sync_managed_instruction(&cfg, ".opencode/rules/*.md", false).unwrap();
        let removed = std::fs::read_to_string(&cfg).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&removed).unwrap();
        assert!(doc.get("instructions").is_none());
        assert_eq!(removed, original, "remove undoes add byte-for-byte");
    }
}
