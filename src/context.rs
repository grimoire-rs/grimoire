// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Per-invocation context.
//!
//! Built once per `grim` run. Phase 1 only resolves environment-derived
//! configuration; later phases attach the OCI-access client and local
//! store. Parsed CLI options override the corresponding environment
//! variables (the CLI is authoritative).

use std::path::PathBuf;
use std::sync::Arc;

use crate::cli::options::GlobalOptions;
use crate::env;
use crate::oci::access::cached_access::CachedAccess;
use crate::oci::access::registry_client::RegistryClient;
use crate::oci::access::{AccessMode, OciAccess};
use crate::oci::tag_cache::TagCache;
use crate::store::{BlobStore, GrimPaths};

/// Memoized OCI access seams for one invocation, keyed by the pair that
/// defines what a client *is*: its routing mode and its plain-HTTP exception
/// list. See [`Context::clients`].
type AccessMemo = std::sync::Mutex<std::collections::HashMap<(AccessMode, Vec<String>), Arc<dyn OciAccess>>>;

/// Resolved configuration for a single `grim` invocation.
///
/// Fields are resolved eagerly but cheaply (env reads only). The OCI
/// client / local store seam is deferred to Phase 3.
///
/// Uses manual `Debug` + `Clone` impls to accommodate the test-only
/// `test_access` field (`dyn OciAccess` is not `Debug`).
//
// TODO(phase-3): add the resolved OCI-access client + local store here,
// constructed lazily so commands that don't touch the registry pay
// nothing. No stub trait in Phase 1 — the seam lands with the access
// subsystem so its shape is driven by real call sites.
pub struct Context {
    grim_home: PathBuf,
    /// The `--registry` flag values only (highest registry precedence), in
    /// the order given. Multiple values browse several registries at once;
    /// the first is the default short identifiers expand against.
    registry_flag: Vec<String>,
    /// `$GRIM_DEFAULT_REGISTRY`, captured once at construction.
    registry_env: Option<String>,
    offline: bool,
    /// The `--progress` mode for long-running passes.
    progress: crate::cli::options::ProgressMode,
    /// The `--global` flag: operate on the global scope rather than the
    /// discovered project. Consumed by scope-aware commands via
    /// [`Self::global`] instead of a per-command redeclaration.
    global: bool,
    /// The `--config` flag: an explicit project config path.
    config: Option<PathBuf>,
    /// Per-invocation memo for the launch-scope plain-HTTP exception list.
    ///
    /// Not config on the context — a cache of one already-computed answer.
    /// Resolving it costs a `GlobalConfig::load`, which compiles every
    /// browse-filter glob; ~20 command sites build the OCI seam, and
    /// `publish` builds one per manifest entry, so recomputing it per call
    /// re-pays that compile N times for an answer that cannot change within
    /// one invocation. Filled by `command::plain_http_hosts`, which is the
    /// only writer. A **scoped** resolve (the MCP per-call scope) bypasses
    /// this deliberately — its answer is not the launch scope's.
    plain_http: std::sync::OnceLock<Vec<String>>,
    /// Per-invocation memo of built access seams, keyed by the two inputs
    /// that decide what a client *is*: its routing mode and its plain-HTTP
    /// exception list.
    ///
    /// The point is the registry **bearer token**. `oci-client` caches tokens
    /// per `(registry, repository, operation)` on the `Client` instance, so
    /// rebuilding the client throws that cache away and the next request
    /// re-runs the whole handshake (`GET /v2/` → `401` + challenge → token
    /// realm → the actual request). For a one-shot CLI run that is invisible;
    /// for the long-lived `grim mcp` server it meant every tool call paid a
    /// fresh handshake, because each call built its own client.
    ///
    /// Keyed rather than single-slot because the MCP tools resolve their
    /// scope per call, so the exception list genuinely varies within one
    /// process — and a client built for one scope's `insecure` opt-in must
    /// never serve another's (`access_seam_scoped`'s whole point).
    ///
    /// Nothing is persisted: the tokens live and die with the process.
    clients: AccessMemo,
    /// Test-only injected `OciAccess` override.  When `Some`, `access()`
    /// returns this instead of constructing a real `CachedAccess`.  Only
    /// compiled in test builds (`#[cfg(test)]`).
    #[cfg(test)]
    test_access: Option<Arc<dyn OciAccess>>,
    /// Test-only record of every plain-HTTP exception list handed to
    /// [`Self::access`].
    ///
    /// Without it the wiring is unobservable: a mutation making
    /// `command::access_seam` compute the list and then discard it — the
    /// feature's central behaviour removed — left the entire unit suite
    /// green, because every test asserted on the *list builder* and none on
    /// what the client was actually built with.
    #[cfg(test)]
    plain_http_seen: std::sync::Mutex<Vec<Vec<String>>>,
}

impl std::fmt::Debug for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("Context");
        d.field("grim_home", &self.grim_home)
            .field("registry_flag", &self.registry_flag)
            .field("registry_env", &self.registry_env)
            .field("offline", &self.offline)
            .field("global", &self.global)
            .field("config", &self.config);
        #[cfg(test)]
        d.field("test_access", &self.test_access.as_ref().map(|_| "<injected>"));
        d.finish()
    }
}

impl Clone for Context {
    fn clone(&self) -> Self {
        Self {
            grim_home: self.grim_home.clone(),
            registry_flag: self.registry_flag.clone(),
            registry_env: self.registry_env.clone(),
            offline: self.offline,
            progress: self.progress,
            global: self.global,
            config: self.config.clone(),
            // A fresh memo: a clone may be re-scoped by its new owner, and a
            // stale plain-HTTP list is the one thing that must never ride along.
            plain_http: std::sync::OnceLock::new(),
            // Likewise fresh: the client memo is keyed on the exception list,
            // so carrying it would be sound but pointless — a clone is made to
            // be re-scoped.
            clients: AccessMemo::default(),
            #[cfg(test)]
            test_access: self.test_access.clone(),
            #[cfg(test)]
            plain_http_seen: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl Context {
    /// Builds the context from parsed global options and the environment.
    ///
    /// Resolution-affecting CLI flags take precedence over their
    /// environment-variable counterparts. The `--registry` flag and the
    /// `$GRIM_DEFAULT_REGISTRY` env var are stored separately so the
    /// registry-precedence helper can order the flag above the env above
    /// any config default (see `command::resolve_default_registry`).
    pub fn new(options: &GlobalOptions) -> Self {
        Self {
            grim_home: env::grim_home(),
            registry_flag: options.registry.clone(),
            registry_env: env::default_registry(),
            offline: options.offline || env::offline(),
            progress: options.progress,
            global: options.global,
            config: options.config.clone(),
            plain_http: std::sync::OnceLock::new(),
            clients: AccessMemo::default(),
            #[cfg(test)]
            test_access: None,
            #[cfg(test)]
            plain_http_seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// The resolved Grimoire data root.
    pub fn grim_home(&self) -> &std::path::Path {
        &self.grim_home
    }

    /// The first `--registry` flag value, if any. Highest registry
    /// precedence — the precedence helper orders this above env and config.
    /// This is the single-registry view used for short-id expansion and the
    /// single-target commands (`login` / `init` / `publish`); the full set is
    /// [`Self::registry_flags`].
    pub fn registry_flag(&self) -> Option<&str> {
        self.registry_flag.first().map(String::as_str)
    }

    /// All `--registry` flag values, in order. The browse set (`search` /
    /// `tui` / `mcp`) collapses to exactly these when non-empty.
    pub fn registry_flags(&self) -> &[String] {
        &self.registry_flag
    }

    /// `$GRIM_DEFAULT_REGISTRY`, if set.
    pub fn registry_env(&self) -> Option<&str> {
        self.registry_env.as_deref()
    }

    /// The default registry for short identifiers: the `--registry` flag,
    /// else `$GRIM_DEFAULT_REGISTRY`. Config defaults are layered in by
    /// `command::resolve_default_registry`, not here.
    #[allow(
        dead_code,
        reason = "superseded by command::resolve_default_registry's fuller precedence chain; exercised directly by this module's tests"
    )]
    pub fn default_registry(&self) -> Option<&str> {
        self.registry_flag().or(self.registry_env.as_deref())
    }

    /// Whether all network access is disabled for this invocation.
    pub fn offline(&self) -> bool {
        self.offline
    }

    /// The `--progress` mode for long-running passes.
    pub fn progress(&self) -> crate::cli::options::ProgressMode {
        self.progress
    }

    /// The `--global` flag: operate on the global scope rather than the
    /// discovered project.
    pub fn global(&self) -> bool {
        self.global
    }

    /// The `--config` flag: an explicit project config path, if given.
    pub fn config(&self) -> Option<&std::path::Path> {
        self.config.as_deref()
    }

    /// The per-invocation memo backing [`crate::command::plain_http_hosts`].
    ///
    /// Exposed rather than filled here because computing the value needs the
    /// config layer, which `Context` deliberately does not reach into (env
    /// reads only — see the type docs). `command` owns the computation and
    /// this owns the lifetime.
    pub fn plain_http_memo(&self) -> &std::sync::OnceLock<Vec<String>> {
        &self.plain_http
    }

    /// The resolved cache-routing mode for this invocation: `Offline` when
    /// the invocation is offline, otherwise the always-fresh `Online`
    /// default. See [`AccessMode`].
    pub fn access_mode(&self) -> AccessMode {
        if self.offline {
            AccessMode::Offline
        } else {
            AccessMode::Online
        }
    }

    /// Typed view of the `$GRIM_HOME` layout for this invocation.
    pub fn paths(&self) -> GrimPaths {
        GrimPaths::new(self.grim_home.clone())
    }

    /// Build the OCI-access seam: a real registry client behind the
    /// persistent tag + blob cache, routed by [`Self::access_mode`].
    ///
    /// `ensure_layout` is called here so the cache directories exist (and
    /// the single-volume invariant is asserted) before the first lookup.
    ///
    /// In test builds (`#[cfg(test)]`), when a `test_access` override was
    /// injected via [`Self::with_access`], that instance is returned
    /// directly — no filesystem layout or real registry client is created.
    ///
    /// `plain_http` is the complete plain-HTTP exception list for this
    /// invocation. `Context` holds no config (env reads only, see the type
    /// docs), so the config-derived half is resolved by the caller —
    /// `command::plain_http_hosts`, reached through `command::access_seam`.
    ///
    /// # Errors
    ///
    /// Returns an [`std::io::Error`] if the `$GRIM_HOME` layout cannot be
    /// created. Callers route it through the install-tier `TargetIo` error
    /// so it classifies as an I/O exit code, not the generic fall-through.
    pub fn access(&self, plain_http: Vec<String>) -> std::io::Result<Arc<dyn OciAccess>> {
        #[cfg(test)]
        self.plain_http_seen
            .lock()
            .expect("plain-HTTP record mutex poisoned by a panicking test")
            .push(plain_http.clone());
        #[cfg(test)]
        if let Some(ref injected) = self.test_access {
            return Ok(Arc::clone(injected));
        }
        self.access_with_mode(self.access_mode(), plain_http)
    }

    /// Build the OCI-access seam with an explicit routing `mode`.
    ///
    /// # Errors
    ///
    /// Returns an [`std::io::Error`] if the `$GRIM_HOME` layout cannot be
    /// created.
    pub fn access_with_mode(&self, mode: AccessMode, plain_http: Vec<String>) -> std::io::Result<Arc<dyn OciAccess>> {
        // Reuse an identically-configured seam if this invocation already
        // built one. The saving is the registry bearer token: `oci-client`
        // caches it on the `Client`, so a fresh client re-runs the full
        // handshake on its next request. Immaterial for a one-shot CLI run,
        // decisive for `grim mcp`, where every tool call used to build its own
        // client and so paid a fresh handshake per call.
        //
        // The key is the full `(mode, plain_http)` pair, never just the mode:
        // handing a client built for one scope's `insecure` opt-in to another
        // scope would silently downgrade that scope's transport.
        let key = (mode, plain_http.clone());
        // Poison recovery rather than a panic: this is a cache, and the worst
        // a poisoned memo can cost is a rebuilt client (one extra handshake).
        // Taking the process down over it would turn a cache into a liability.
        if let Some(hit) = self
            .clients
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
        {
            return Ok(Arc::clone(hit));
        }

        let paths = self.paths();
        paths.ensure_layout()?;
        let cached = CachedAccess::new(
            RegistryClient::with_plain_http_hosts(plain_http),
            TagCache::new(paths.tags_dir()),
            BlobStore::new(paths.blobs_dir()),
            mode,
        );
        let access: Arc<dyn OciAccess> = Arc::new(cached);
        self.clients
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, Arc::clone(&access));
        Ok(access)
    }
}

#[cfg(test)]
impl Context {
    /// Hermetic test constructor: no ambient env reads. Tests asserting
    /// registry precedence or "no registry resolvable" must not inherit the
    /// developer's `$GRIM_DEFAULT_REGISTRY` / `$GRIM_HOME` (mutating the
    /// process env in tests is `unsafe` and forbidden — inject instead).
    pub fn hermetic(grim_home: std::path::PathBuf) -> Self {
        Self {
            grim_home,
            registry_flag: Vec::new(),
            registry_env: None,
            offline: false,
            progress: crate::cli::options::ProgressMode::Auto,
            global: false,
            config: None,
            plain_http: std::sync::OnceLock::new(),
            clients: AccessMemo::default(),
            test_access: None,
            plain_http_seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// [`Self::hermetic`] with an explicit `--global` / `--config` scope,
    /// for tests that need a hermetic `$GRIM_HOME` (no ambient env reads)
    /// alongside a specific scope resolution.
    pub fn hermetic_scoped(grim_home: std::path::PathBuf, global: bool, config: Option<std::path::PathBuf>) -> Self {
        Self {
            global,
            config,
            ..Self::hermetic(grim_home)
        }
    }

    /// Set the `--registry` flag values on a hermetic context, so a test can
    /// assert what the flag does (and, for the transport exception list,
    /// what it must *not* do) without mutating the process environment.
    pub fn with_registry_flags(mut self, flags: Vec<String>) -> Self {
        self.registry_flag = flags;
        self
    }

    /// Every plain-HTTP exception list [`Self::access`] was handed, in call
    /// order. The observable that makes the config→transport wiring
    /// testable — see the field docs.
    pub fn plain_http_seen(&self) -> Vec<Vec<String>> {
        self.plain_http_seen
            .lock()
            .expect("plain-HTTP record mutex poisoned by a panicking test")
            .clone()
    }

    /// Test-only constructor that injects a custom [`OciAccess`] override.
    /// Commands that call `access_seam(ctx)` will receive this instance
    /// instead of a real `CachedAccess`, enabling unit tests that exercise
    /// full `run()` paths against an in-memory registry double.
    ///
    /// Example:
    /// ```ignore
    /// let reg = MemoryRegistry::new();
    /// let ctx = Context::with_access(tmp.path().to_path_buf(), reg);
    /// ```
    pub fn with_access(grim_home: std::path::PathBuf, access: impl OciAccess + 'static) -> Self {
        Self {
            grim_home,
            registry_flag: Vec::new(),
            registry_env: None,
            offline: false,
            progress: crate::cli::options::ProgressMode::Auto,
            global: false,
            config: None,
            plain_http: std::sync::OnceLock::new(),
            clients: AccessMemo::default(),
            test_access: Some(Arc::new(access)),
            plain_http_seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Test-only constructor that injects both a custom [`OciAccess`]
    /// override and a `--registry` flag value. Used to test that the flag
    /// registry wins over the manifest registry (ADR D1).
    pub fn with_access_and_registry(
        grim_home: std::path::PathBuf,
        access: impl OciAccess + 'static,
        registry_flag: String,
    ) -> Self {
        Self {
            grim_home,
            registry_flag: vec![registry_flag],
            registry_env: None,
            offline: false,
            progress: crate::cli::options::ProgressMode::Auto,
            global: false,
            config: None,
            plain_http: std::sync::OnceLock::new(),
            clients: AccessMemo::default(),
            test_access: Some(Arc::new(access)),
            plain_http_seen: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::options::OutputFormat;

    fn opts() -> GlobalOptions {
        GlobalOptions {
            format: OutputFormat::Plain,
            color: crate::cli::color::ColorMode::Auto,
            progress: crate::cli::options::ProgressMode::Auto,
            offline: false,
            log_level: None,
            config: None,
            global: false,
            registry: Vec::new(),
        }
    }

    #[test]
    fn cli_offline_flag_forces_offline_regardless_of_env() {
        let mut o = opts();
        o.offline = true;
        let ctx = Context::new(&o);
        assert!(ctx.offline());
        assert_eq!(ctx.access_mode(), AccessMode::Offline);
    }

    #[test]
    fn default_invocation_is_online() {
        let ctx = Context::new(&opts());
        assert!(!ctx.offline());
        assert_eq!(ctx.access_mode(), AccessMode::Online);
    }

    /// Two identically-configured requests share one seam, so the registry
    /// bearer token `oci-client` caches on the `Client` survives across them.
    /// Without this, `grim mcp` paid a fresh token handshake on every tool
    /// call — the whole point of the memo, and unobservable from the outside
    /// (`Arc::ptr_eq` is the only witness).
    #[test]
    fn an_identically_configured_access_seam_is_reused() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        let hosts = vec!["registry.internal:5050".to_string()];

        let first = ctx.access_with_mode(AccessMode::Online, hosts.clone()).unwrap();
        let second = ctx.access_with_mode(AccessMode::Online, hosts.clone()).unwrap();
        assert!(Arc::ptr_eq(&first, &second), "the same seam must be handed back");
    }

    /// The memo keys on the FULL configuration, never the mode alone. Handing
    /// a client built for one scope's `insecure` opt-in to a different scope
    /// would silently downgrade that scope's transport to plain HTTP — the
    /// exact cross-scope leak `access_seam_scoped` exists to prevent.
    #[test]
    fn a_differently_configured_access_seam_is_not_reused() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());

        let insecure = ctx
            .access_with_mode(AccessMode::Online, vec!["registry.internal:5050".to_string()])
            .unwrap();
        let other = ctx.access_with_mode(AccessMode::Online, Vec::new()).unwrap();
        assert!(
            !Arc::ptr_eq(&insecure, &other),
            "a different plain-HTTP exception list must build its own client"
        );

        let offline = ctx
            .access_with_mode(AccessMode::Offline, vec!["registry.internal:5050".to_string()])
            .unwrap();
        assert!(!Arc::ptr_eq(&insecure, &offline), "a different mode must not be reused");
    }

    #[test]
    fn cli_registry_overrides_and_grim_home_resolves() {
        let mut o = opts();
        o.registry = vec!["ghcr.io/acme".to_string()];
        let ctx = Context::new(&o);
        assert_eq!(ctx.default_registry(), Some("ghcr.io/acme"));
        assert!(ctx.grim_home().is_absolute() || ctx.grim_home().ends_with(".grimoire"));
    }

    #[test]
    fn registry_flag_is_split_from_env_and_surfaced_separately() {
        // The `--registry` flag populates `registry_flag()`; `default_registry()`
        // folds flag-or-env for login back-compat. (The env var is not mutated
        // here — that is `unsafe` and the crate forbids it; the env accessor is
        // exercised structurally.)
        let mut o = opts();
        o.registry = vec!["ghcr.io/acme".to_string()];
        let ctx = Context::new(&o);
        assert_eq!(ctx.registry_flag(), Some("ghcr.io/acme"));
        assert_eq!(ctx.default_registry(), Some("ghcr.io/acme"));
    }

    #[test]
    fn multiple_registry_flags_surface_all_with_first_as_default() {
        let mut o = opts();
        o.registry = vec!["a.example".to_string(), "b.example".to_string()];
        let ctx = Context::new(&o);
        // Full browse set preserved in order; first value is the single default.
        assert_eq!(ctx.registry_flags(), &["a.example", "b.example"]);
        assert_eq!(ctx.registry_flag(), Some("a.example"));
        assert_eq!(ctx.default_registry(), Some("a.example"));
    }
}
