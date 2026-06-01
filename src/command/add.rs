// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim add <kind> <name> <ref>` — declare a skill/rule and lock it.
//!
//! Edits the discovered config's `[skills]`/`[rules]` table (re-serializing
//! the parsed config is acceptable — minimal formatting churn for a
//! provisional milestone), then re-resolves just that entry under the
//! config flock: a partial relock when a previous lock exists, a full
//! resolve otherwise. The new lock is saved with `generated_at`
//! preservation for the untouched entries.

use std::sync::Arc;

use clap::Args;

use crate::api::add_report::AddReport;
use crate::cli::exit_code::ExitCode;
use crate::context::Context;
use crate::lock::file_lock::ConfigFileLock;
use crate::lock::lock_io;
use crate::oci::access::OciAccess;
use crate::oci::{ArtifactKind, Identifier};
use crate::resolve::resolve_options::ResolveOptions;
use crate::resolve::resolver::{resolve_lock, resolve_lock_partial};

use super::scope_resolution;

/// `grim add` arguments.
#[derive(Debug, Args)]
pub struct AddArgs {
    /// `skill`, `rule`, or `bundle`.
    #[arg(value_parser = ["skill", "rule", "bundle"])]
    pub kind: String,

    /// The config binding name.
    pub name: String,

    /// The fully-qualified reference (`registry/repo:tag` or `@digest`).
    pub reference: String,

    /// Operate on the global scope instead of the discovered project.
    #[arg(long)]
    pub global: bool,

    /// Explicit project config path.
    #[arg(long)]
    pub config: Option<std::path::PathBuf>,
}

/// Run `grim add`.
///
/// # Errors
///
/// Config (78/79/74), invalid reference (65), or lock/resolve failures
/// propagate via the typed error chain.
pub async fn run(ctx: &Context, args: &AddArgs) -> anyhow::Result<(AddReport, ExitCode)> {
    let kind = match args.kind.as_str() {
        "skill" => ArtifactKind::Skill,
        "bundle" => ArtifactKind::Bundle,
        _ => ArtifactKind::Rule,
    };

    let scope = super::grim(scope_resolution::resolve(ctx, args.global, args.config.as_deref()))?;

    // Hold the config flock for the read-modify-write + relock window.
    let _guard = match scope_resolution::lockable_config_path(&scope) {
        Some(path) => Some(super::grim(ConfigFileLock::try_acquire(&path))?),
        None => None,
    };

    // Validate the reference (with the context default registry) and add
    // the entry to the in-memory declaration.
    let id = super::grim(parse_reference(ctx, &args.reference))?;
    let id = if id.tag().is_none() && id.digest().is_none() {
        id.clone_with_tag("latest")
    } else {
        id
    };

    let mut set = scope.set.clone();
    match kind {
        ArtifactKind::Skill => {
            set.skills.insert(args.name.clone(), id.clone());
        }
        ArtifactKind::Rule => {
            set.rules.insert(args.name.clone(), id.clone());
        }
        ArtifactKind::Bundle => {
            set.bundles.insert(args.name.clone(), id.clone());
        }
    }
    set.invalidate_declaration_hash_cache();

    // Persist the edited config (re-serialize the parsed declaration).
    super::grim(write_config(&scope.config_path, &scope.options, &set))?;

    // Relock: a partial relock of just this entry when a previous lock
    // exists and is not stale; a full resolve otherwise (or when the
    // partial stale guard fires — caught and retried as a full resolve so
    // `add` always leaves a consistent lock).
    let access: Arc<dyn OciAccess> = super::access_seam(ctx)?;
    let previous = lock_io::load(&scope.lock_path).ok();
    // A bundle declaration expands into members whose names differ from the
    // bundle's binding name, so a partial relock keyed on the bundle name
    // cannot work — always do a full resolve for bundles.
    let new_lock = if kind == ArtifactKind::Bundle {
        super::grim(resolve_lock(&set, &access, scope.scope, &ResolveOptions::default()).await)?
    } else {
        match &previous {
            Some(prev) => {
                match resolve_lock_partial(
                    &set,
                    prev,
                    &access,
                    std::slice::from_ref(&args.name),
                    scope.scope,
                    &ResolveOptions::default(),
                )
                .await
                {
                    Ok(lock) => lock,
                    Err(e)
                        if matches!(
                            e.kind,
                            crate::resolve::resolve_error::ResolveErrorKind::StaleLock { .. }
                        ) =>
                    {
                        // The added entry made the predecessor stale; a full
                        // resolve is the correct recovery (every entry is
                        // declared, so this is consistent).
                        super::grim(resolve_lock(&set, &access, scope.scope, &ResolveOptions::default()).await)?
                    }
                    Err(e) => return Err(crate::error::Error::from(e).into()),
                }
            }
            None => super::grim(resolve_lock(&set, &access, scope.scope, &ResolveOptions::default()).await)?,
        }
    };
    super::grim(lock_io::save(&scope.lock_path, &new_lock, previous.as_ref()))?;

    // A bundle has no single pinned member to report; surface the bundle
    // reference itself. A skill/rule reports the digest it resolved to.
    let pinned = if kind == ArtifactKind::Bundle {
        id.to_string()
    } else {
        new_lock
            .skills
            .iter()
            .chain(new_lock.rules.iter())
            .find(|a| a.kind == kind && a.name == args.name)
            .map(|a| a.pinned.strip_advisory().to_string())
            .unwrap_or_else(|| id.to_string())
    };

    Ok((AddReport::new(kind, args.name.clone(), pinned), ExitCode::Success))
}

/// Parse `<ref>` with the context default registry.
pub(crate) fn parse_reference(
    ctx: &Context,
    reference: &str,
) -> Result<Identifier, crate::oci::identifier::error::IdentifierError> {
    match ctx.default_registry() {
        Some(def) => Identifier::parse_with_default_registry(reference, def),
        None => Identifier::parse(reference),
    }
}

/// Re-serialize the declaration to `path` as the shared
/// `[options]`/`[bundles]`/`[skills]`/`[rules]` schema. Atomic via the
/// store primitive so a crash never truncates the config. The `[bundles]`
/// table is emitted only when at least one bundle is declared, so a
/// bundle-free config is byte-identical to one written before bundles
/// existed.
pub(crate) fn write_config(
    path: &std::path::Path,
    options: &crate::config::declaration::ConfigOptions,
    set: &crate::config::declaration::DesiredSet,
) -> Result<(), crate::config::config_error::ConfigError> {
    use std::fmt::Write as _;

    let mut out = String::new();
    if options.default_registry.is_some() || options.editor.is_some() {
        out.push_str("[options]\n");
        if let Some(r) = &options.default_registry {
            let _ = writeln!(out, "default_registry = \"{r}\"");
        }
        if let Some(e) = &options.editor {
            let _ = writeln!(out, "editor = \"{e}\"");
        }
        out.push('\n');
    }
    if !set.bundles.is_empty() {
        out.push_str("[bundles]\n");
        for (name, id) in &set.bundles {
            let _ = writeln!(out, "{name} = \"{id}\"");
        }
        out.push('\n');
    }
    out.push_str("[skills]\n");
    for (name, id) in &set.skills {
        let _ = writeln!(out, "{name} = \"{id}\"");
    }
    out.push_str("\n[rules]\n");
    for (name, id) in &set.rules {
        let _ = writeln!(out, "{name} = \"{id}\"");
    }

    crate::store::atomic_write::atomic_write(path, out.as_bytes()).map_err(|e| {
        crate::config::config_error::ConfigError::new(path, crate::config::config_error::ConfigErrorKind::Io(e))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::declaration::{ConfigOptions, DesiredSet};
    use crate::config::project_config::ProjectConfig;
    use std::collections::BTreeMap;

    #[test]
    fn write_config_round_trips_through_parser() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let mut skills = BTreeMap::new();
        skills.insert(
            "code-review".to_string(),
            Identifier::parse("ghcr.io/acme/code-review:stable").unwrap(),
        );
        let mut rules = BTreeMap::new();
        rules.insert(
            "rust-style".to_string(),
            Identifier::parse("ghcr.io/acme/rust-style:v3").unwrap(),
        );
        let set = DesiredSet::from_parts(skills, rules);
        let opts = ConfigOptions {
            default_registry: Some("ghcr.io/acme".to_string()),
            editor: Some("claude".to_string()),
        };
        write_config(&path, &opts, &set).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        let cfg = ProjectConfig::from_toml_str(&body).expect("re-serialized config must parse");
        assert_eq!(cfg.set.skills.len(), 1);
        assert_eq!(cfg.set.rules.len(), 1);
        assert_eq!(cfg.options.default_registry.as_deref(), Some("ghcr.io/acme"));
    }

    #[test]
    fn write_config_omits_options_when_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("grimoire.toml");
        let set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        write_config(&path, &ConfigOptions::default(), &set).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(!body.contains("[options]"));
        assert!(ProjectConfig::from_toml_str(&body).is_ok());
    }
}
