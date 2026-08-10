// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim context` — read-only introspection of the resolved invocation
//! context.
//!
//! Pure serialization of what scope resolution and the registry resolver
//! already computed: scope, paths (+ existence), effective client-target
//! set, registry browse set, and offline mode. No network, no writes, no
//! side effects — the command exists so an external consumer (editor
//! extension, script) can ask "what would grim act on from here?" without
//! reimplementing the walk-up/precedence rules.
//!
//! Outside a project without `--global` the config walk-up fails with
//! `NotDiscovered` exactly like every other scope command (exit 79).

use clap::Args;

use crate::api::context_report::{ContextRegistry, ContextRegistryKind, ContextReport, OfflineSource};
use crate::auth::store::{DockerCredentialStore, StoreOptions};
use crate::cli::exit_code::ExitCode;
use crate::context::Context;
use crate::install::target::InstallTarget;

use super::scope_resolution;

/// `grim context` arguments (none — scope comes from the global flags).
#[derive(Debug, Args)]
pub struct ContextArgs {}

/// Run `grim context`.
///
/// # Errors
///
/// Propagates scope-resolution failures (`NotDiscovered` ⇒ 79, parse ⇒
/// 78) and an invalid configured client name (65).
pub async fn run(ctx: &Context, _args: &ContextArgs) -> anyhow::Result<(ContextReport, ExitCode)> {
    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;

    // Effective client set: config `[options].clients` (no --client flag on
    // this command), else detection, else the generic `agents` client — the
    // same seam install uses, so this reports the RESOLVED set, never the
    // raw detection. `grim context` is the diagnostic surface for exactly
    // the no-client-detected situation, so it must answer `["agents"]` here
    // and never error.
    let target = super::grim(InstallTarget::parse(
        &scope.workspace,
        scope.scope,
        &[],
        &scope.options.clients,
        &scope.options.vendors,
    ))?;
    let clients = target.clients().iter().map(ToString::to_string).collect();

    // A file-only view of the docker-compatible credential store, resolved
    // once. `None` when no config location exists (no `$DOCKER_CONFIG`, no
    // home) — everything then reports `authenticated: false`. Never spawns a
    // credential helper.
    let cred_store = DockerCredentialStore::new(StoreOptions::default()).ok();
    let resolved = super::registries_for_scope(ctx, &scope)?;
    // The default is read off the set already resolved above rather than
    // through `primary_registry_for_scope`, which resolves a second time
    // (S-8): that recompiled every entry's `GlobSet` again and printed each
    // filter's diagnostics twice for one `grim context`.
    let default_registry = default_registry_of(&resolved);
    let registries = resolved
        .into_iter()
        .map(|r| {
            let authenticated = cred_store.as_ref().is_some_and(|s| s.has_credential(&r.url));
            context_registry(r, authenticated)
        })
        .collect();

    // `Context::offline` folds flag and env; the ambient env var is
    // reported as the source whenever it is set (it applies regardless of
    // the flag), else the flag.
    let offline_source = if !ctx.offline() {
        None
    } else if crate::env::offline() {
        Some(OfflineSource::Env)
    } else {
        Some(OfflineSource::Flag)
    };

    // The lock is read, not just probed. This is the command a user reaches
    // for when install state looks wrong, and reporting `exists` for a lock
    // no other command can read points the diagnosis away from the actual
    // fault. A missing lock stays silent — that is the normal pre-lock
    // state, not a failure. Read-only either way, so the exit code is
    // unchanged: `context` reports facts, it does not judge them.
    let lock_error = match crate::lock::lock_io::load(&scope.lock_path) {
        Ok(_) => None,
        Err(e) if e.is_not_found() => None,
        // The kind's chain, not the whole error: `LockError`'s own Display
        // leads with the path, already reported as `lock_path`.
        Err(e) => Some(format!("{:#}", anyhow::Error::new(e.kind))),
    };

    let report = ContextReport {
        version: env!("CARGO_PKG_VERSION").to_string(),
        scope: scope.scope.to_string(),
        config_exists: scope.config_path.exists(),
        lock_exists: scope.lock_path.exists(),
        lock_error,
        workspace: scope.workspace,
        config_path: scope.config_path,
        lock_path: scope.lock_path,
        state_path: scope.state_path,
        grim_home: ctx.grim_home().to_path_buf(),
        offline: ctx.offline(),
        offline_source,
        clients,
        registries,
        default_registry,
    };
    Ok((report, ExitCode::Success))
}

/// The reported default registry for an already-resolved browse set.
///
/// The same composition [`super::primary_registry_for_scope`] applies —
/// including the push-side substitution for an index-only set, where
/// `primary_registry` has no registry-kind entry to return.
fn default_registry_of(registries: &[crate::config::ResolvedRegistry]) -> String {
    super::or_fallback_registry(crate::config::registry_resolve::primary_registry(registries))
}

/// Project one resolved browse source into its report row.
///
/// The browse filter's patterns are read straight off the resolved entry
/// (plan C-020) rather than re-read from `grimoire.toml`: resolution is
/// what decides whether a filter applies at all, so a second config read
/// here would report configured patterns under `--registry`, where the
/// forced browse set is genuinely unfiltered (plan C-009, S-014/S-019).
fn context_registry(r: crate::config::ResolvedRegistry, authenticated: bool) -> ContextRegistry {
    ContextRegistry {
        authenticated,
        include: r.filter.include_patterns().to_vec(),
        exclude: r.filter.exclude_patterns().to_vec(),
        alias: r.alias,
        kind: if r.kind.is_index() {
            ContextRegistryKind::Index
        } else {
            ContextRegistryKind::Registry
        },
        default: r.is_default,
        url: r.url,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::declaration::RegistryConfig;
    use crate::config::resolve_registries;

    /// One project entry filtered on both sides.
    fn filtered_entry() -> RegistryConfig {
        RegistryConfig {
            alias: Some("acme".to_string()),
            oci: Some("ghcr.io/acme".to_string()),
            index: None,
            include: vec!["acme/platform/**".to_string(), "acme/tools/**".to_string()],
            exclude: vec!["acme/platform/legacy/**".to_string()],
            default: true,
        }
    }

    #[test]
    fn context_registry_reports_each_side_from_its_own_list() {
        // Plan C-020 / S-019: the authored patterns, in declaration order,
        // read back off the resolved entry's compiled filter. Swapping the
        // two fields here inverts an allowlist into a denylist in the one
        // report a JSON consumer trusts, and fails this assertion.
        let resolved = resolve_registries(&[], &[filtered_entry()], None, &[], None, "fallback.example", None);
        let row = context_registry(resolved.into_iter().next().expect("one entry"), false);
        assert_eq!(
            row.include,
            vec!["acme/platform/**".to_string(), "acme/tools/**".to_string()]
        );
        assert_eq!(row.exclude, vec!["acme/platform/legacy/**".to_string()]);
    }

    #[test]
    fn context_registry_under_forced_registry_reports_empty_lists() {
        // Plan C-020 / S-019: `--registry <ref>` collapses the browse set to
        // entries the resolver builds with no filter at all (C-009), so both
        // lists are `[]` even though the config declares patterns. Reporting
        // the configured patterns there would misdescribe the run.
        let resolved = resolve_registries(
            &["ghcr.io/other".to_string()],
            &[filtered_entry()],
            None,
            &[],
            None,
            "fallback.example",
            None,
        );
        let row = context_registry(resolved.into_iter().next().expect("one forced entry"), false);
        assert_eq!(row.url, "ghcr.io/other");
        assert!(
            row.include.is_empty() && row.exclude.is_empty(),
            "the forced browse set is unfiltered; got: {row:?}"
        );
    }

    #[test]
    fn context_registry_unfiltered_entry_reports_empty_lists() {
        let entry = RegistryConfig {
            alias: Some("acme".to_string()),
            oci: Some("ghcr.io/acme".to_string()),
            ..Default::default()
        };
        let resolved = resolve_registries(&[], &[entry], None, &[], None, "fallback.example", None);
        let row = context_registry(resolved.into_iter().next().expect("one entry"), false);
        assert!(row.include.is_empty() && row.exclude.is_empty());
    }

    #[tokio::test]
    async fn hermetic_global_scope_reports_paths_and_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        // Global scope: an absent global config is an empty declaration,
        // so the command succeeds even in an empty GRIM_HOME... but the
        // hermetic context has `global: false`; resolve explicitly here.
        let scope = scope_resolution::resolve(&ctx, true, None).expect("global scope resolves");
        assert_eq!(scope.scope, crate::config::scope::ConfigScope::Global);

        // Build the report through the same seams `run` uses, minus the
        // ctx-global flag (hermetic contexts are project-scoped).
        let target = InstallTarget::parse(
            &scope.workspace,
            scope.scope,
            &[],
            &scope.options.clients,
            &std::collections::BTreeMap::new(),
        )
        .unwrap();
        assert!(
            !target.clients().is_empty(),
            "context reports the resolved set — the generic client at minimum, never []"
        );
        let primary = crate::command::primary_registry_for_scope(&ctx, &scope).expect("no global config to fail on");
        assert!(!primary.is_empty(), "default registry always resolves");
    }

    #[tokio::test]
    async fn default_registry_of_matches_the_resolver_seam() {
        // S-8: `run` now reads the default off the browse set it already
        // resolved instead of re-resolving through
        // `primary_registry_for_scope`. An unconfigured scope is exactly
        // where the two can diverge — the set is index-only, so
        // `primary_registry` returns "" and only the push-side substitution
        // keeps the report's `default_registry` non-empty.
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        let scope = scope_resolution::resolve(&ctx, true, None).expect("global scope resolves");
        let resolved = crate::command::registries_for_scope(&ctx, &scope).expect("browse set resolves");
        assert!(
            resolved.iter().all(|r| r.kind.is_index()),
            "the divergence case needs an index-only set: {resolved:?}"
        );
        assert_eq!(
            default_registry_of(&resolved),
            crate::command::primary_registry_for_scope(&ctx, &scope).expect("seam resolves"),
        );
    }

    #[test]
    fn bare_workspace_reports_the_generic_client() {
        // The diagnostic contract: on a workspace where nothing is detected,
        // `clients` is `["agents"]` — the set an install would actually write
        // to. Not `[]` (which would read as "grim will do nothing") and not
        // all eleven (the old fallback's lie).
        let tmp = tempfile::tempdir().unwrap();
        let target = InstallTarget::parse(
            tmp.path(),
            crate::config::scope::ConfigScope::Project,
            &[],
            &[],
            &std::collections::BTreeMap::new(),
        )
        .unwrap();
        let clients: Vec<String> = target.clients().iter().map(ToString::to_string).collect();
        assert_eq!(clients, vec!["agents".to_string()]);
    }
}
