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
    let registries = super::registries_for_scope(ctx, &scope)
        .into_iter()
        .map(|r| ContextRegistry {
            authenticated: cred_store.as_ref().is_some_and(|s| s.has_credential(&r.url)),
            alias: r.alias,
            kind: if r.kind.is_index() {
                ContextRegistryKind::Index
            } else {
                ContextRegistryKind::Registry
            },
            default: r.is_default,
            url: r.url,
        })
        .collect();
    let default_registry = super::primary_registry_for_scope(ctx, &scope);

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

#[cfg(test)]
mod tests {
    use super::*;

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
        let primary = crate::command::primary_registry_for_scope(&ctx, &scope);
        assert!(!primary.is_empty(), "default registry always resolves");
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
