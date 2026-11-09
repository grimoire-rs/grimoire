// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim remove <kind> <name>` — undeclare a skill/rule.
//!
//! Drops the entry from the discovered config's `[skills]`/`[rules]`
//! table and from the lock (re-saved). Materialized files are
//! **intentionally left on disk** this milestone — removing installed
//! client files is deferred (a future `grim clean`); `remove` only
//! affects the declaration and the lock so the change is reversible.

use clap::Args;

use crate::api::remove_report::{RemoveReport, RemoveStatus};
use crate::cli::exit_code::ExitCode;
use crate::config::declaration::{ConfigOptions, DesiredSet, RegistryConfig};
use crate::config::scope::Invocation;
use crate::context::Context;
use crate::lock::file_lock::ConfigFileLock;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::lock_io;
use crate::oci::ArtifactKind;

use super::add::{restore_config, snapshot_config, write_config};
use super::scope_resolution;

/// `grim remove` arguments.
#[derive(Debug, Args)]
pub struct RemoveArgs {
    /// `skill`, `rule`, `agent`, `bundle`, `mcp`, `hook`, or `settings`.
    //
    // `hook` appended, never inserted — an accepted value set is a frozen CLI
    // surface and widening it is the additive direction (Principle 9).
    #[arg(value_parser = ["skill", "rule", "agent", "bundle", "mcp", "hook", "settings"])]
    pub kind: String,

    /// The config binding name to remove.
    pub name: String,
}

/// Run `grim remove`.
///
/// # Errors
///
/// Config (78/79/74) or lock save (74) failures propagate via the typed
/// error chain. An absent entry is reported, not an error.
pub async fn run(ctx: &Context, args: &RemoveArgs) -> anyhow::Result<(RemoveReport, ExitCode)> {
    // Derived from the enum, never a local arm list with a `_ => Rule`
    // catch-all — see the same fix in `command::uninstall::run` for the defect
    // that catch-all caused once the `value_parser` gained `"hook"`.
    let kind = ArtifactKind::from_kind_str(&args.kind).ok_or_else(|| {
        crate::error::Error::from(super::command_error::CommandError::ConfigUsage(format!(
            "unknown artifact kind '{}'",
            args.kind
        )))
    })?;

    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;

    let _guard = match scope_resolution::lockable_config_path(&scope) {
        Some(path) => Some(super::grim(ConfigFileLock::try_acquire(&path))?),
        None => None,
    };

    let set_before = scope.set.clone();
    let mut set = scope.set.clone();
    let removed = match kind {
        // `remove` undeclares only (config + lock); files stay on disk, and for
        // a hook that includes the registration, which `grim uninstall` is what
        // removes. Refusing a hook here would leave the user no way to
        // undeclare one, and the gate that matters (arming) is not on this path.
        ArtifactKind::Hook => set.hooks.remove(&args.name).is_some(),
        ArtifactKind::Skill => set.skills.remove(&args.name).is_some(),
        ArtifactKind::Rule => set.rules.remove(&args.name).is_some(),
        ArtifactKind::Agent => set.agents.remove(&args.name).is_some(),
        ArtifactKind::Bundle => set.bundles.remove(&args.name).is_some(),
        ArtifactKind::Mcp => set.mcp.remove(&args.name).is_some(),
        ArtifactKind::Settings => set.settings.remove(&args.name).is_some(),
    };

    if !removed {
        return Ok((
            RemoveReport::new(kind, args.name.clone(), RemoveStatus::Absent),
            ExitCode::Success,
        ));
    }
    set.invalidate_declaration_hash_cache();

    // Persist the edited config, then bring the lock to the post-edit
    // effective state: drop what no other declaration holds, keep shared
    // entries, restamp the hash only if the lock states a fact.
    let edit = ConfigEdit::write(
        &scope.config_path,
        &scope.options,
        &scope.registries,
        &set,
        &scope.invocation,
    )?;

    if let Some(previous) = load_for_edit(&scope.lock_path) {
        let target = super::uninstall::EditTarget {
            config_path: &scope.config_path,
            lock_path: &scope.lock_path,
            grim: &scope.invocation,
            store: ctx.paths(),
        };
        let stored = target.stored_members(&set, &previous);
        let outcome = crate::lock::edit::drop_from_lock(
            &previous,
            kind,
            &args.name,
            &set_before,
            &set,
            &scope.invocation,
            &stored,
        );
        edit.save_lock(&scope.lock_path, &outcome.lock, &previous)?;
        for note in &outcome.notes {
            tracing::warn!("{note}");
        }
    }

    // C-155: `remove` is not a converging seam (it loads no install state), so
    // an armed hook keeps firing until the next converging command. Say so
    // rather than let the undeclare look like a disarm.
    if kind == ArtifactKind::Hook {
        tracing::warn!(
            "hook '{}' was undeclared but stays armed until `{grim} install` or `{grim} uninstall` runs",
            args.name,
            grim = scope.invocation
        );
    }

    Ok((
        RemoveReport::new(kind, args.name.clone(), RemoveStatus::Removed),
        ExitCode::Success,
    ))
}

/// The lock a declaration edit brings along, `None` when there is none to
/// touch. An unreadable lock holds no pin the edit could move, so the edit
/// goes on and only says so: every command that reads the lock refuses it.
pub(crate) fn load_for_edit(path: &std::path::Path) -> Option<GrimoireLock> {
    lock_io::load_existing(path).unwrap_or_else(|e| {
        // Not `registry_text`: its cap would cut the path off the end of the cause.
        let cause = crate::error::printed_chain(&anyhow::Error::new(e));
        // One event per line: the log writer spells out a line break inside an event,
        // so a TOML error's quoted file would read as one line of literal `\n`.
        let mut lines = cause.lines();
        tracing::warn!("the lock was not updated: {}", lines.next().unwrap_or_default());
        for line in lines {
            tracing::warn!("{line}");
        }
        None
    })
}

/// A declaration edit on disk, undone if the lock it goes with cannot be saved:
/// a declaration without its lock reads as a hand edit, and the next relock
/// would move what the failed remove kept.
pub(crate) struct ConfigEdit<'a> {
    path: &'a std::path::Path,
    original: Option<Vec<u8>>,
    written: Option<Vec<u8>>,
    /// The `grim` a failed rollback's remedy names.
    grim: &'a Invocation,
}

impl<'a> ConfigEdit<'a> {
    /// Write `set` to `path`, keeping the bytes it replaces.
    ///
    /// # Errors
    ///
    /// The config cannot be read or written (74/78/79).
    pub(crate) fn write(
        path: &'a std::path::Path,
        options: &ConfigOptions,
        registries: &[RegistryConfig],
        set: &DesiredSet,
        grim: &'a Invocation,
    ) -> anyhow::Result<Self> {
        let original = snapshot_config(path)?;
        super::grim(write_config(path, options, registries, set))?;
        // Unreadable right after a write, nothing can show the file is still
        // ours, so the rollback declines.
        let written = snapshot_config(path).ok().flatten();
        Ok(Self {
            path,
            original,
            written,
            grim,
        })
    }

    /// Save the lock this edit goes with, or put the declaration back.
    ///
    /// # Errors
    ///
    /// The lock cannot be saved (74).
    pub(crate) fn save_lock(
        &self,
        lock_path: &std::path::Path,
        lock: &GrimoireLock,
        previous: &GrimoireLock,
    ) -> anyhow::Result<()> {
        let saved = super::grim(lock_io::save(lock_path, lock, Some(previous)));
        if saved.is_err() {
            self.roll_back();
        }
        saved
    }

    fn roll_back(&self) {
        // A file that changed since this write holds someone else's edit, and
        // restoring over it would lose that edit.
        match snapshot_config(self.path) {
            Ok(now) if now.is_some() && now == self.written => {
                restore_config(self.path, self.original.as_deref(), self.grim);
            }
            _ => tracing::warn!(
                path = %self.path.display(),
                "the declaration changed while the lock was written, so it stays edited; run `{} lock`",
                self.grim
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::declaration::DesiredSet;
    use crate::lock::grimoire_lock::LockMetadata;
    use crate::lock::lock_version::LockVersion;
    use crate::lock::locked_artifact::LockedArtifact;
    use crate::oci::pinned_identifier::PinnedIdentifier;
    use crate::oci::{Digest, Identifier};
    use std::collections::BTreeMap;

    fn locked(name: &str) -> LockedArtifact {
        let id = Identifier::new_registry(name, "localhost:5000").clone_with_digest(Digest::Sha256("a".repeat(64)));
        LockedArtifact::direct(
            name.to_string(),
            ArtifactKind::Skill,
            PinnedIdentifier::try_from(id).unwrap(),
        )
    }

    fn lock_of(skills: Vec<LockedArtifact>) -> GrimoireLock {
        GrimoireLock {
            hooks: vec![],
            settings: vec![],
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: 1,
                declaration_hash: format!("sha256:{}", "d".repeat(64)),
                generated_by: "grim 0.1.0".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills,
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        }
    }

    fn set_of(skills: &[(&str, &str)], bundles: &[(&str, &str)]) -> DesiredSet {
        let skills: BTreeMap<String, crate::config::declaration::DeclaredSource> = skills
            .iter()
            .map(|(n, i)| {
                (
                    (*n).to_string(),
                    crate::config::declaration::DeclaredSource::Registry(Identifier::parse(i).unwrap()),
                )
            })
            .collect();
        let mut set = DesiredSet::from_parts(skills, BTreeMap::new());
        for (n, i) in bundles {
            set.bundles.insert(
                (*n).to_string(),
                crate::config::declaration::DeclaredSource::Registry(Identifier::parse(i).unwrap()),
            );
        }
        set.invalidate_declaration_hash_cache();
        set
    }

    #[test]
    fn an_edit_nothing_else_touched_rolls_back_and_a_changed_file_stays() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grimoire.toml");
        std::fs::write(&path, "[skills]\nalpha = \"localhost:5000/alpha:1\"\n").unwrap();
        let original = std::fs::read(&path).unwrap();
        let options = ConfigOptions::default();

        let edit =
            ConfigEdit::write(&path, &options, &[], &set_of(&[], &[]), &Invocation::WALKED).expect("the edit writes");
        assert_ne!(std::fs::read(&path).unwrap(), original);
        edit.roll_back();
        assert_eq!(std::fs::read(&path).unwrap(), original, "rolled back");

        let edit =
            ConfigEdit::write(&path, &options, &[], &set_of(&[], &[]), &Invocation::WALKED).expect("the edit writes");
        std::fs::write(&path, "# someone else's\n").unwrap();
        edit.roll_back();
        assert_eq!(std::fs::read(&path).unwrap(), b"# someone else's\n", "theirs stays");
    }

    // ── An unreadable lock ───────────────────────────────────────────────

    /// `grim remove` on this thread, with everything it logged. The guard stays
    /// across the await because a `#[tokio::test]` polls on this one thread.
    async fn remove_logged(ctx: &Context, args: &RemoveArgs) -> (anyhow::Result<(RemoveReport, ExitCode)>, String) {
        let capture = crate::catalog::test_hosts::LogCapture::start();
        let out = run(ctx, args).await;
        (out, capture.text())
    }

    fn workspace_with(lock: Option<&str>) -> (tempfile::TempDir, std::path::PathBuf, Context) {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let config_path = root.join("grimoire.toml");
        std::fs::write(&config_path, "[skills]\nalpha = \"localhost:5000/alpha:1\"\n").unwrap();
        let lock_path = root.join("grimoire.lock");
        if let Some(body) = lock {
            std::fs::write(&lock_path, body).unwrap();
        }
        let ctx = Context::hermetic_scoped(root.join("grim-home"), false, Some(config_path));
        (dir, lock_path, ctx)
    }

    fn remove_alpha() -> RemoveArgs {
        RemoveArgs {
            kind: "skill".to_string(),
            name: "alpha".to_string(),
        }
    }

    /// The lock's link points into a directory that refuses the write, so only
    /// the lock save fails. Root writes through 0o555, so there the failure
    /// cannot be provoked and the test returns.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_lock_save_restores_the_config() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let lock = lock_of(vec![locked("alpha")]).to_toml_string().unwrap();
        let (_dir, lock_path, ctx) = workspace_with(Some(&lock));
        let root = lock_path.parent().unwrap().to_path_buf();
        let config = root.join("grimoire.toml");
        let sealed = root.join("sealed");
        std::fs::create_dir(&sealed).unwrap();
        std::fs::rename(&lock_path, sealed.join("grimoire.lock")).unwrap();
        symlink(sealed.join("grimoire.lock"), &lock_path).unwrap();
        let before = std::fs::read(&config).unwrap();

        let mode = |bits| std::fs::Permissions::from_mode(bits);
        std::fs::set_permissions(&sealed, mode(0o555)).unwrap();
        let writable = tempfile::tempfile_in(&sealed).is_ok();
        let out = run(&ctx, &remove_alpha()).await;
        std::fs::set_permissions(&sealed, mode(0o755)).unwrap();
        if writable {
            return;
        }

        out.expect_err("the lock cannot be saved");
        assert_eq!(std::fs::read(&config).unwrap(), before);
    }

    #[tokio::test]
    async fn an_unreadable_locks_cause_reaches_the_log_escaped() {
        let (_dir, _lock, ctx) = workspace_with(Some("\u{1b}[2J not a lock"));

        let (out, logs) = remove_logged(&ctx, &remove_alpha()).await;

        out.expect("the config edit stands");
        assert!(!logs.contains('\u{1b}'), "{logs}");
        assert_eq!(
            logs.matches("the lock was not updated").count(),
            1,
            "one warning: {logs}"
        );
    }

    /// A TOML error quotes the file over several lines, and the warning keeps them
    /// as `grim install` prints them, one event each, because the log writer spells
    /// out a line break inside an event. [`LogCapture`] writes through that writer.
    ///
    /// [`LogCapture`]: crate::catalog::test_hosts::LogCapture
    #[tokio::test]
    async fn an_unreadable_locks_toml_error_keeps_its_lines() {
        let (_dir, _lock, ctx) = workspace_with(Some("not = [valid toml"));

        let (out, logs) = remove_logged(&ctx, &remove_alpha()).await;

        out.expect("the config edit stands");
        assert!(!logs.contains("\\n"), "no line break spelled out: {logs}");
        let lines: Vec<&str> = logs.trim_end().lines().collect();
        assert!(lines.len() > 1, "the quoted file keeps its lines: {logs}");
        assert!(lines.iter().all(|line| line.contains("WARN")), "{logs}");
        assert!(
            lines.iter().any(|line| line.ends_with("1 | not = [valid toml")),
            "{logs}"
        );
    }

    /// serde quotes an unknown key verbatim, and a quoted TOML key can hold a line
    /// break, which must stay on its line inside a warning whose lines survive.
    #[tokio::test]
    async fn a_quoted_key_in_an_unreadable_lock_forges_no_log_line() {
        let (_dir, _lock, ctx) = workspace_with(Some("\"a\\nWARN forged\" = 1\n"));

        let (out, logs) = remove_logged(&ctx, &remove_alpha()).await;

        out.expect("the config edit stands");
        assert!(logs.contains("unknown field `a\\nWARN forged"), "{logs}");
        assert!(
            !logs.lines().any(|line| line.trim_start().starts_with("WARN forged")),
            "{logs}"
        );
    }

    #[tokio::test]
    async fn an_unreadable_lock_warns_once_and_the_remove_still_succeeds() {
        for (body, cause) in [
            ("not = [valid toml", "invalid TOML"),
            ("[metadata]\nlock_version = 99\n", "lock_version"),
        ] {
            let (_dir, lock_path, ctx) = workspace_with(Some(body));

            let (out, logs) = remove_logged(&ctx, &remove_alpha()).await;

            let (_report, exit) = out.expect("no pin moves, so the exit stays 0");
            assert_eq!(exit, ExitCode::Success);
            assert_eq!(logs.matches("the lock was not updated").count(), 1, "{logs}");
            assert!(logs.contains(lock_path.to_str().unwrap()), "names the lock: {logs}");
            assert!(logs.contains(cause), "names the error: {logs}");
            assert_eq!(std::fs::read_to_string(&lock_path).unwrap(), body, "left as it was");
        }
    }

    #[tokio::test]
    async fn a_missing_lock_is_not_worth_a_warning() {
        let (_dir, _lock_path, ctx) = workspace_with(None);

        let (out, logs) = remove_logged(&ctx, &remove_alpha()).await;

        assert_eq!(out.expect("no lock is not an error").1, ExitCode::Success);
        assert!(logs.is_empty(), "{logs}");
    }

    /// Every string the `<kind>` positional accepts must resolve to the kind it
    /// names. **Regression guard, and it caught a live defect:** the
    /// `value_parser` gained `"hook"` while `run`'s local arm list kept a
    /// `_ => ArtifactKind::Rule` catch-all, so `grim remove hook <name>` silently
    /// acted on a *rule*. Deriving the kind from `ArtifactKind::from_kind_str`
    /// makes the two sides one source of truth; this test pins the accepted set
    /// against it so appending a kind to the parser and forgetting the mapping
    /// is a test failure rather than a wrong-target action.
    #[test]
    fn every_accepted_kind_string_resolves_to_that_kind() {
        for accepted in ["skill", "rule", "agent", "bundle", "mcp", "hook", "settings"] {
            let kind = ArtifactKind::from_kind_str(accepted)
                .unwrap_or_else(|| panic!("the `remove` parser accepts '{accepted}' but no kind parses it"));
            assert_eq!(kind.kind_str(), accepted, "'{accepted}' must not resolve to {kind}");
        }
    }
}
