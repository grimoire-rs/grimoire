// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The spine commands.
//!
//! Each command follows the pattern: parse → typed scope/refs →
//! operation → report built from operation results → render via
//! [`crate::cli::printer::Printable`]. `anyhow` is used here (the
//! application boundary); the lib subsystems stay on `thiserror`.

pub mod add;
pub mod build;
pub mod command_error;
pub mod completions;
pub mod config;
pub mod config_keys;
pub mod context;
pub mod describe;
pub mod fetch;
pub mod init;
pub mod install;
pub mod lock;
pub mod login;
pub mod logout;
pub mod mcp;
pub mod publish;
pub mod rate;
pub mod release;
pub mod remove;
pub mod schema;
pub mod scope_resolution;
pub mod search;
pub mod status;
pub mod tui;
pub mod uninstall;
pub mod update;

#[allow(unused_imports)]
pub use command_error::CommandError;

/// How `login` / `logout` treat a global config that fails to load.
///
/// The asymmetry is deliberate. `login` **writes** a credential, so a
/// registry set it could not fully assemble might send the secret to the
/// wrong host — fatal. `logout` **erases** one, and refusing to erase
/// because an unrelated file is unparseable strands the credential exactly
/// when the user most needs it gone (grim cannot repair that file either —
/// `config registry rm --global` exits 78 on it too), so it drops the
/// unreadable tier and carries on.
///
/// **What `Lenient` actually buys, and what it does not.** It rescues the
/// cases whose registry survives the dropped tier — a literal hostname, a
/// `--registry` flag, `$GRIM_DEFAULT_REGISTRY`, a project-scope alias. It
/// does **not** rescue an alias declared only in the broken global config:
/// that argument falls through to itself as a literal hostname
/// ([`resolve_login_registry`]), nothing is erased, and the command exits
/// **0** reporting a host the user never named. For that one case the degrade
/// strands the credential anyway *and* swaps a loud 78 for a silent success —
/// `Strict` at least said so. That is a reporting gap, not a wrong erase: the
/// dangerous direction is closed by construction, because aliases match
/// project-tier-first (`registry_resolve.rs`,
/// `project.iter().chain(global.iter())`), so dropping the global tier can
/// only remove a substitution, never redirect one to a different host.
///
/// Making that case exit 78 is **not** the fix: it changes a released exit
/// code (`main`'s `resolve_login_registry` has the identical fallthrough with
/// an infallible registry set, and exits 0 there today), which the 1.0
/// stabilization freeze prohibits. The report carries the fact instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalConfigPolicy {
    /// A malformed or invalid global config aborts the command (exit 78).
    Strict,
    /// A malformed or invalid global config warns and drops the global
    /// `[[registries]]` / `default_registry` tier; every tier that *is*
    /// readable (the `--registry` flag, `$GRIM_DEFAULT_REGISTRY`, the
    /// project `[[registries]]`) still applies. An alias that lived only in
    /// the dropped tier does not resolve — see the type's doc comment.
    Lenient,
}

/// Resolve the registry for `login` / `logout`: an explicit (non-empty)
/// argument wins — substituting a configured `[[registries]]` alias when the
/// argument matches one (mirroring the `alias/repo` substitution
/// [`crate::config::resolve_reference`] applies for `add`/`search`), else
/// taken as a literal hostname. Otherwise the resolved scope's registry set
/// supplies the primary: `--registry` flag, `$GRIM_DEFAULT_REGISTRY`, the
/// project/global `[[registries]]` default, or the legacy
/// `[options].default_registry` chain — the same seam
/// [`registries_for_scope`] / [`registries_global_fallback`] `add`/`release`
/// already use. A miss is a classifiable config error, not a panic.
///
/// Unlike `add`/`release`, `login`/`logout` never substitute the built-in
/// [`FALLBACK_REGISTRY`] when nothing is configured — silently storing (or
/// erasing) a credential for a registry the user never named would be a
/// silent surprise, so an unresolved browse set (only the built-in index
/// tier) still errors.
///
/// # Errors
///
/// [`CommandError::NoLoginRegistry`] when neither an argument nor a
/// resolvable default registry is available; under
/// [`GlobalConfigPolicy::Strict`] also a malformed or invalid global config
/// (exit 78) — see [`global_config_tiers`].
pub fn resolve_login_registry(
    ctx: &crate::context::Context,
    explicit: Option<&str>,
    policy: GlobalConfigPolicy,
) -> anyhow::Result<String> {
    resolve_login_registry_reporting(ctx, explicit, policy).map(|(registry, _)| registry)
}

/// [`resolve_login_registry`] under [`GlobalConfigPolicy::Lenient`], also
/// reporting whether the global tier was dropped so the caller can disclose
/// it (`logout` exits **0** either way — see [`GlobalConfigPolicy`]).
///
/// # Errors
///
/// As [`resolve_login_registry`], minus the strict-only config error: only
/// [`command_error::CommandError::NoLoginRegistry`] remains reachable.
pub fn resolve_login_registry_lenient(
    ctx: &crate::context::Context,
    explicit: Option<&str>,
) -> anyhow::Result<(String, bool)> {
    resolve_login_registry_reporting(ctx, explicit, GlobalConfigPolicy::Lenient)
}

/// The shared body of the two resolvers above; the flag is
/// [`login_registries`]' "global tier dropped", always `false` under
/// [`GlobalConfigPolicy::Strict`] (which errors instead of degrading).
fn resolve_login_registry_reporting(
    ctx: &crate::context::Context,
    explicit: Option<&str>,
    policy: GlobalConfigPolicy,
) -> anyhow::Result<(String, bool)> {
    let (registries, global_config_dropped) = login_registries(ctx, policy)?;
    if let Some(reg) = explicit.filter(|r| !r.is_empty()) {
        if let Some(matched) = registries.iter().find(|r| r.alias.as_deref() == Some(reg)) {
            return Ok((matched.url.clone(), global_config_dropped));
        }
        if global_config_dropped {
            if let Some(url) = salvaged_alias(ctx, reg) {
                tracing::warn!(
                    "'{}' resolved from the invalid global config's [[registries]]; only its alias map was used",
                    reg.escape_debug()
                );
                return Ok((url, global_config_dropped));
            }
            // The degrade WARN above says an alias declared in the global
            // config "will not substitute" — in the abstract. It never says
            // *this* invocation is that case, which is the one where exit 0
            // reads as "credential erased" and is not. Escaped: `reg` is
            // unvalidated argv on its way to stderr.
            tracing::warn!(
                "'{}' matched no configured alias; treating it as a literal hostname",
                reg.escape_debug()
            );
        }
        return Ok((reg.to_string(), global_config_dropped));
    }
    match crate::config::registry_resolve::primary_registry(&registries) {
        "" => Err(anyhow::Error::from(crate::error::Error::from(
            command_error::CommandError::NoLoginRegistry,
        ))),
        primary => Ok((primary.to_string(), global_config_dropped)),
    }
}

/// The url an alias names in a global config that **parsed** but failed
/// validation, or `None` when there is nothing to salvage.
///
/// The narrow half of the `Lenient` degrade. Dropping the whole global tier
/// made `grim logout <alias>` fall through to a host literally named
/// `<alias>`, erase nothing, and exit **0** — success reported for work not
/// done, on a credential path. The alias map does not depend on the rules
/// `validate_registries` enforces: at most one default, unique aliases and
/// one locator per entry say nothing about which url a given alias names.
///
/// **Only the alias map is recovered, never a primary registry.** Which
/// entry is primary *is* decided by the rule that failed — and
/// [`crate::config::resolve_registries`] falls back to "first entry wins"
/// when none sets `default`, so feeding these entries back into it would let
/// a broken file elect a primary. A bare `logout` against a broken global
/// config therefore still resolves nothing and exits 78, exactly as before.
fn salvaged_alias(ctx: &crate::context::Context, alias: &str) -> Option<String> {
    crate::config::global_config::GlobalConfig::registries_unvalidated(&ctx.paths().global_config())
        .into_iter()
        .find(|entry| entry.alias.as_deref() == Some(alias))
        .map(|entry| entry.locator().to_string())
        .filter(|url| !url.is_empty())
}

/// The registry set `login`/`logout` resolve aliases and the configured
/// default against: the project scope's browse set when one is
/// discoverable, else the global-`[[registries]]`-aware fallback — the same
/// seam `add`/`release`/`search` use ([`registries_for_scope`] /
/// [`registries_global_fallback`]), so a `[[registries]]` alias or default
/// declared at either scope round-trips through `login`/`logout` too.
///
/// The returned flag is `true` when the global tier was dropped — i.e. the
/// degrade below actually fired. It is disclosure, not control flow: every
/// caller resolves identically either way.
///
/// # Errors
///
/// Under [`GlobalConfigPolicy::Strict`], a malformed or invalid global
/// config (exit 78) — see [`global_config_tiers`];
/// [`GlobalConfigPolicy::Lenient`] warns and re-assembles the set without
/// that tier instead. A *project*-scope resolution failure stays non-fatal
/// under either policy: it degrades to the global fallback set, as before.
fn login_registries(
    ctx: &crate::context::Context,
    policy: GlobalConfigPolicy,
) -> anyhow::Result<(Vec<crate::config::ResolvedRegistry>, bool)> {
    let scope = scope_resolution::resolve(ctx, ctx.global(), ctx.config()).ok();
    let assembled = match &scope {
        Some(scope) => registries_for_scope(ctx, scope),
        None => registries_global_fallback(ctx),
    };
    match assembled {
        Ok(registries) => Ok((registries, false)),
        // Every `Err` here is a `GlobalConfig::load` failure — the only
        // fallible step in either branch — so dropping the global tier is
        // precisely the degrade, and the project tier survives it.
        Err(err) if policy == GlobalConfigPolicy::Lenient => {
            tracing::warn!(
                "global config unusable ({err:#}); continuing without its [[registries]] — an alias declared there will not substitute"
            );
            Ok((
                crate::config::resolve_registries(
                    ctx.registry_flags(),
                    scope.as_ref().map_or(&[], |s| s.registries.as_slice()),
                    scope.as_ref().and_then(|s| s.options.default_registry.as_deref()),
                    &[],
                    None,
                    FALLBACK_INDEX,
                    ctx.registry_env(),
                ),
                true,
            ))
        }
        Err(err) => Err(err),
    }
}

/// The built-in default registry for push-side and short-id expansion,
/// used only when no other tier configures one (no `--registry` flag, no
/// `$GRIM_DEFAULT_REGISTRY`, no config `default_registry`). First-party
/// packages live under the grimoire-rs org on GHCR.
pub const FALLBACK_REGISTRY: &str = "ghcr.io/grimoire-rs";

/// The built-in browse fallback: the public package index. Used as the
/// final tier of the browse-set resolution so an unconfigured `grim
/// search` / TUI / MCP lists the ecosystem through the index (GHCR gates
/// `_catalog`, so a bare registry fallback would browse empty).
pub const FALLBACK_INDEX: &str = "https://index.grimoire.rs";

/// The single registry-precedence helper: `--registry` flag, then
/// `$GRIM_DEFAULT_REGISTRY`, then the project config
/// `[options].default_registry`, then the global config
/// `[options].default_registry`, then the built-in
/// [`FALLBACK_REGISTRY`]. The first present value wins, so the fallback
/// applies only when nothing is configured anywhere.
///
/// The default registry is purely a CLI-input convenience — the expanded
/// [`crate::oci::Identifier`] is always fully-qualified, so the lock and
/// config persist the registry host explicitly regardless of which default
/// was applied. Every registry call site (`add` / `release` / `search` /
/// `tui`) routes through this so the precedence is single-sourced.
pub fn resolve_default_registry(
    ctx: &crate::context::Context,
    project_default: Option<&str>,
    global_default: Option<&str>,
) -> String {
    ctx.registry_flag()
        .or_else(|| ctx.registry_env())
        .or(project_default)
        .or(global_default)
        .unwrap_or(FALLBACK_REGISTRY)
        .to_string()
}

/// Both global-config registry tiers — `[[registries]]` and
/// `[options].default_registry` — from **one** load. Empty / `None` for a
/// global-scope run: the global config is already that run's active scope,
/// so neither tier may be folded in twice.
///
/// One load, not two: `GlobalConfig::load` re-runs `validate_registries`,
/// which compiles every browse-filter glob, so reading the two fields
/// separately compiled the whole filter set twice. The ADR's stated reason
/// for needing no pattern-count cap is that patterns compile "once per
/// registry at config-load" — that claim is what this keeps true.
///
/// # Errors
///
/// A malformed or invalid global config (exit 78). Silently dropping it here
/// would erase every globally-declared registry from a project-scope run and
/// still exit 0 — the config error must reach the user, exactly as it does
/// for a global-scope run. An **absent** global config stays
/// `Ok((vec![], None))`.
pub fn global_config_tiers(
    ctx: &crate::context::Context,
    scope: crate::config::scope::ConfigScope,
) -> anyhow::Result<(Vec<crate::config::declaration::RegistryConfig>, Option<String>)> {
    if scope == crate::config::scope::ConfigScope::Global {
        return Ok((Vec::new(), None));
    }
    let cfg = grim(crate::config::global_config::GlobalConfig::load(
        &ctx.paths().global_config(),
    ))?;
    Ok((cfg.registries, cfg.options.default_registry))
}

/// Assemble the ordered registry browse set for a resolved scope.
///
/// The single seam `search` / `tui` / `mcp` call to get the multi-registry
/// set: the `--registry` flag(s) (`ctx.registry_flags`) collapse to exactly
/// those registries; otherwise the scope's `[[registries]]` are authoritative; when
/// no `[[registries]]` exist the legacy single-default chain
/// (`$GRIM_DEFAULT_REGISTRY` > project `[options].default_registry` > global >
/// fallback) applies — all via [`crate::config::resolve_registries`] so the
/// precedence is single-sourced.
///
/// # Errors
///
/// A malformed or invalid global config (exit 78) — see
/// [`global_config_tiers`].
pub fn registries_for_scope(
    ctx: &crate::context::Context,
    scope: &scope_resolution::ResolvedScope,
) -> anyhow::Result<Vec<crate::config::ResolvedRegistry>> {
    Ok(registries_and_short_id_default(ctx, scope)?.0)
}

/// The browse set **and** the short-id default registry for a resolved
/// scope, from **one** global-config load.
///
/// A caller that needs both must call this, never
/// [`registries_for_scope`] plus a second global-config read: this branch made
/// `GlobalConfig::load` compile every browse-filter glob, so a second read
/// recompiles the whole attacker-controlled filter set. Measured on one legal
/// 60-entry global config, release build: 12.63 s with one load, 21.52 s with
/// two — 1.7× CPU for identical input, from a `grimoire.toml` found by silent
/// walk-up. The ADR's stated reason for needing no pattern-count cap is that
/// patterns compile "once per registry at config-load"; keeping the two
/// consumers on one load is what makes that true at the call sites, which is
/// [`global_config_tiers`]' whole purpose.
///
/// The two answers come off different tiers on purpose. The browse set is
/// `[[registries]]`-authoritative; the short-id default is the legacy
/// flag > env > project > global > built-in chain, because an index source
/// cannot expand a short id (its locator is not a registry host).
///
/// # Errors
///
/// A malformed or invalid global config (exit 78) — see
/// [`global_config_tiers`].
pub fn registries_and_short_id_default(
    ctx: &crate::context::Context,
    scope: &scope_resolution::ResolvedScope,
) -> anyhow::Result<(Vec<crate::config::ResolvedRegistry>, String)> {
    let (global_registries, global_default) = global_config_tiers(ctx, scope.scope)?;
    let registries = crate::config::resolve_registries(
        ctx.registry_flags(),
        &scope.registries,
        scope.options.default_registry.as_deref(),
        &global_registries,
        global_default.as_deref(),
        FALLBACK_INDEX,
        ctx.registry_env(),
    );
    let short_id_default = resolve_default_registry(
        ctx,
        scope.options.default_registry.as_deref(),
        global_default.as_deref(),
    );
    Ok((registries, short_id_default))
}

/// The primary registry for a resolved scope via the same seam
/// `add` / `search` / `mcp` use: `primary_registry(&registries_for_scope(…))`.
///
/// This is the unified consumer seam — `release` and `tui` route through it
/// so that a `[[registries]]`-only config (no `[options].default_registry`)
/// is honored by all commands, removing the inconsistency where PATH-A
/// commands (`release_default_registry`, `resolve_registry`) previously
/// resolved only through the legacy `default_registry` chain.
///
/// On scope-resolution failure the scope is absent; call
/// [`primary_registry_global_fallback`] instead, which folds the global
/// `[[registries]]` tier so a `[[registries]]`-only global config is still
/// honored.
///
/// # Errors
///
/// A malformed or invalid global config (exit 78) — see
/// [`global_config_tiers`].
pub fn primary_registry_for_scope(
    ctx: &crate::context::Context,
    scope: &scope_resolution::ResolvedScope,
) -> anyhow::Result<String> {
    Ok(or_fallback_registry(crate::config::registry_resolve::primary_registry(
        &registries_for_scope(ctx, scope)?,
    )))
}

/// Index sources never expand short ids, so a browse set holding only
/// index sources (notably the built-in [`FALLBACK_INDEX`] tier) yields an
/// empty primary — substitute the push-side [`FALLBACK_REGISTRY`] so
/// `add`/`release` short ids keep a concrete registry host.
fn or_fallback_registry(primary: &str) -> String {
    if primary.is_empty() {
        FALLBACK_REGISTRY.to_string()
    } else {
        primary.to_string()
    }
}

/// The primary registry when scope resolution fails (e.g. `release` or `tui`
/// run outside any project): folds the global `[[registries]]` and the legacy
/// `[options].default_registry` tiers so a `[[registries]]`-only global config
/// is honored — the same contract as [`registries_for_scope`]'s global-tier
/// folding, but without a resolved project scope.
///
/// Precedence (mirrors [`crate::config::resolve_registries`] with empty project
/// tiers):
/// 1. `--registry` flag(s) (`ctx.registry_flags`): collapse to exactly those
///    registries. Only the flag collapses; `$GRIM_DEFAULT_REGISTRY` is a tier-3 default.
/// 2. Global `[[registries]]` (first `default = true`, else first entry)
/// 3. `$GRIM_DEFAULT_REGISTRY` (`ctx.registry_env`) → global
///    `[options].default_registry` → built-in [`FALLBACK_REGISTRY`]
///    (legacy single-default chain, only when no `[[registries]]` present)
///
/// # Errors
///
/// A malformed or invalid global config (exit 78) — see
/// [`global_config_tiers`].
pub fn primary_registry_global_fallback(ctx: &crate::context::Context) -> anyhow::Result<String> {
    Ok(or_fallback_registry(crate::config::registry_resolve::primary_registry(
        &registries_global_fallback(ctx)?,
    )))
}

/// The ordered browse set when scope resolution fails (no project config):
/// the `--registry` flag(s), else the global `[[registries]]`, else the
/// legacy single-default chain ending in the built-in [`FALLBACK_INDEX`].
/// The set-building seam behind [`primary_registry_global_fallback`],
/// exposed so browse-side consumers (the TUI init dialog pre-fill) can read
/// the primary *browse* locator — which may be an index source that
/// [`primary_registry_global_fallback`] deliberately substitutes away for
/// push-side use.
///
/// # Errors
///
/// A malformed or invalid global config (exit 78) — see
/// [`global_config_tiers`].
pub fn registries_global_fallback(
    ctx: &crate::context::Context,
) -> anyhow::Result<Vec<crate::config::ResolvedRegistry>> {
    let (global_regs, global_default) = global_config_tiers(ctx, crate::config::scope::ConfigScope::Project)?;
    Ok(crate::config::resolve_registries(
        ctx.registry_flags(),
        &[],
        None,
        &global_regs,
        global_default.as_deref(),
        FALLBACK_INDEX,
        ctx.registry_env(),
    ))
}

/// Resolve the neutral [`crate::fetch::FetchScope`] for a fetch/render:
/// the ordered registry browse set, the short-id default, the resolved
/// scope kind, and any degraded-scope warning.
///
/// Scope resolution parity with `grim search`: a resolvable scope supplies
/// its configured registry set; failure degrades to the flag/env/global
/// fallback chain (with a warning) instead of failing the fetch. This is
/// the single command-layer seam the `fetch` / `render` front-ends call so
/// the moved resolution stays single-sourced and the fetch core never
/// imports `command`.
///
/// # Errors
///
/// A malformed or invalid global config (exit 78) — see
/// [`global_config_tiers`]. A *scope*-resolution failure stays
/// non-fatal: it degrades to the fallback chain with a warning, as before.
pub fn resolve_fetch_scope(
    ctx: &crate::context::Context,
    global: bool,
    config: Option<&std::path::Path>,
    workspace: Option<&std::path::Path>,
) -> anyhow::Result<crate::fetch::FetchScope> {
    let mut warnings = Vec::new();
    let (registries, short_id_default, scope) = match scope_resolution::resolve_in(ctx, global, config, workspace) {
        Ok(scope) => {
            // One global-config load for both, not two — see
            // [`registries_and_short_id_default`]. This path is also the MCP
            // `grim_fetch` / `grim_render` tools.
            let (registries, short_id_default) = registries_and_short_id_default(ctx, &scope)?;
            (registries, short_id_default, scope.scope)
        }
        Err(e) => {
            warnings.push(format!(
                "no scope resolved ({e:#}); using the flag/env/global fallback registry chain"
            ));
            (
                registries_global_fallback(ctx)?,
                primary_registry_global_fallback(ctx)?,
                crate::config::scope::ConfigScope::Project,
            )
        }
    };
    Ok(crate::fetch::FetchScope {
        registries,
        short_id_default,
        scope,
        warnings,
    })
}

/// Build a classifiable usage error (exit 64) for a missing `login`
/// credential input, routed through the top-level error so
/// [`crate::error::classify_error`] sees it.
pub fn login_usage(message: &'static str) -> anyhow::Error {
    anyhow::Error::from(crate::error::Error::from(command_error::CommandError::LoginInput(
        message,
    )))
}

/// Build a classifiable usage error (exit 64) for `grim config`: unknown
/// key, duplicate alias, or other contract violation.
pub fn config_usage(msg: impl Into<String>) -> anyhow::Error {
    anyhow::Error::from(crate::error::Error::from(command_error::CommandError::ConfigUsage(
        msg.into(),
    )))
}

/// Build a classifiable data error (exit 65) for `grim config set`: a
/// syntactically valid but semantically rejected value.
pub fn config_value(msg: impl Into<String>) -> anyhow::Error {
    anyhow::Error::from(crate::error::Error::from(command_error::CommandError::ConfigValue(
        msg.into(),
    )))
}

/// Map a subsystem `Result` into an `anyhow::Result` whose error is wrapped
/// in the top-level [`crate::error::Error`].
///
/// The bare `?` operator converts a subsystem error straight into
/// `anyhow::Error` via the blanket `From` impl, which bypasses
/// [`crate::error::classify_error`] (it only downcasts the top
/// [`crate::error::Error`]). Routing through this helper keeps every
/// command's exit-code mapping correct.
pub fn grim<T, E>(result: Result<T, E>) -> anyhow::Result<T>
where
    crate::error::Error: From<E>,
{
    result.map_err(|e| anyhow::Error::from(crate::error::Error::from(e)))
}

/// Build the OCI-access seam from the context, mapping a `$GRIM_HOME`
/// layout I/O failure to a classifiable install-tier `TargetIo` error
/// (exit 74) rather than the generic fall-through.
///
/// The seam is always-fresh online unless the invocation is offline, so a
/// rolling release re-resolves the floating tag instead of serving a
/// cached pin — no separate "remote" routing mode is needed.
pub fn access_seam(ctx: &crate::context::Context) -> anyhow::Result<std::sync::Arc<dyn crate::oci::access::OciAccess>> {
    let plain_http = plain_http_hosts(ctx);
    map_access_io(ctx, ctx.access(plain_http))
}

/// [`access_seam`] for a caller that resolved its own scope rather than
/// inheriting the launch scope — the MCP tools, whose scope is a per-call
/// parameter trio (`adr_mcp_percall_scope_fetch_render.md`).
///
/// The arguments are the same three `resolve_fetch_scope` takes, and both
/// must be called with the same values: the browse set and the transport
/// exception list are two answers about **one** scope, and resolving them
/// from different scopes is how a project's `insecure` opt-in ends up
/// downgrading a call scoped to a different project.
///
/// # Errors
///
/// A `$GRIM_HOME` layout I/O failure (exit 74), as [`access_seam`].
pub fn access_seam_scoped(
    ctx: &crate::context::Context,
    global: bool,
    config: Option<&std::path::Path>,
    workspace: Option<&std::path::Path>,
) -> anyhow::Result<std::sync::Arc<dyn crate::oci::access::OciAccess>> {
    let plain_http = crate::oci::access::registry_client::plain_http_hosts_with(&declared_insecure_hosts(
        ctx, global, config, workspace,
    ));
    map_access_io(ctx, ctx.access(plain_http))
}

/// The complete plain-HTTP exception list for this invocation: the implicit
/// loopback forms, every `GRIM_INSECURE_REGISTRIES` entry, and the locator
/// host of every **declared** `[[registries]]` entry that set
/// `insecure = true` — see [`declared_insecure_hosts`] for why "declared"
/// and not "resolved" is the load-bearing word.
///
/// This is where config re-enters a decision `Context` cannot make on its
/// own — it holds env reads only, by design. The two consumers are
/// [`access_seam`] (the OCI client) and `grim login`'s verification ping;
/// both take the finished list rather than re-deriving it, so the scheme
/// decision stays single-sourced.
///
/// Memoized on the context for the launch scope: the answer cannot change
/// within one invocation, and computing it costs a `GlobalConfig::load`
/// that compiles every browse-filter glob. A caller with its **own** scope
/// (the MCP tools) must use [`access_seam_scoped`], which skips the memo.
///
/// **Infallible.** A command that browses outside a project, or against an
/// unreadable global config, must not fail *here* — it has its own error
/// path for that, reached moments later. An unresolvable scope contributes
/// nothing and leaves the env-var and loopback halves intact, which is
/// exactly the behaviour that shipped before the config field existed.
pub fn plain_http_hosts(ctx: &crate::context::Context) -> Vec<String> {
    ctx.plain_http_memo()
        .get_or_init(|| {
            crate::oci::access::registry_client::plain_http_hosts_with(&declared_insecure_hosts(
                ctx,
                ctx.global(),
                ctx.config(),
                None,
            ))
        })
        .clone()
}

/// The locator hosts of every `[[registries]]` entry that **declared**
/// `insecure = true`, across the project and global tiers of one scope.
///
/// Read straight off the declared [`crate::config::declaration::RegistryConfig`]
/// entries, deliberately **not** off [`crate::config::resolve_registries`].
/// That resolver answers a different question — *which registries do I
/// browse, in what order* — and to answer it correctly it collapses to the
/// `--registry` forced set, dedups a shadowed alias across scopes, and
/// elects a primary. Every one of those is right for a browse set and wrong
/// for a transport allowlist, which is a plain union that nothing narrows:
/// `--registry ghcr.io` must not silently re-arm TLS against an unrelated
/// host the config declared plain-HTTP, and an entry hidden by a
/// same-alias entry in the other scope is still a host the user opted in.
///
/// An `index` entry contributes nothing — an index locator carries its own
/// `http(s)://` scheme, so the flag is rejected on one at config load.
///
/// **Infallible.** A command that browses outside a project, or against an
/// unreadable global config, must not fail *here* — it has its own error
/// path for that, reached moments later. An unresolvable scope contributes
/// nothing and leaves the env-var and loopback halves intact, which is
/// exactly the behaviour that shipped before the config field existed.
fn declared_insecure_hosts(
    ctx: &crate::context::Context,
    global: bool,
    config: Option<&std::path::Path>,
    workspace: Option<&std::path::Path>,
) -> Vec<String> {
    let scope = scope_resolution::resolve_in(ctx, global, config, workspace).ok();
    let project = scope.as_ref().map(|s| s.registries.as_slice()).unwrap_or_default();
    let scope_kind = scope
        .as_ref()
        .map_or(crate::config::scope::ConfigScope::Project, |s| s.scope);
    let global_entries = global_config_tiers(ctx, scope_kind)
        .map(|(registries, _)| registries)
        .unwrap_or_default();

    project
        .iter()
        .chain(global_entries.iter())
        .filter(|rc| rc.insecure)
        .filter_map(|rc| rc.oci.as_deref())
        // The locator host without its namespace, and *with* its port —
        // `HttpsExcept` matches the exact `host[:port]` a request carries.
        .map(|locator| crate::oci::access::registry_client::registry_host(locator).to_string())
        .collect()
}

fn map_access_io(
    ctx: &crate::context::Context,
    result: std::io::Result<std::sync::Arc<dyn crate::oci::access::OciAccess>>,
) -> anyhow::Result<std::sync::Arc<dyn crate::oci::access::OciAccess>> {
    result.map_err(|e| {
        anyhow::Error::from(crate::error::Error::from(
            crate::install::install_error::InstallError::without_reference(
                crate::install::install_error::InstallErrorKind::TargetIo {
                    path: ctx.paths().root().to_path_buf(),
                    source: e,
                },
            ),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::options::{GlobalOptions, OutputFormat};
    use crate::config::declaration::RegistryConfig;
    use crate::context::Context;

    fn opts(registry: Option<&str>) -> GlobalOptions {
        GlobalOptions {
            format: OutputFormat::Plain,
            color: crate::cli::color::ColorMode::Auto,
            progress: crate::cli::options::ProgressMode::Auto,
            offline: false,
            log_level: None,
            config: None,
            global: false,
            registry: registry.into_iter().map(str::to_string).collect(),
        }
    }

    #[test]
    fn precedence_flag_beats_config() {
        // The `--registry` flag wins over every config default. (The env is
        // not set in the test environment; the flag is the highest tier.)
        let ctx = Context::new(&opts(Some("flag.example")));
        assert_eq!(
            resolve_default_registry(&ctx, Some("proj.example"), Some("glob.example")),
            "flag.example"
        );
    }

    #[test]
    fn precedence_project_config_beats_global_config() {
        // No flag, no env ⇒ project config wins over the global fallback.
        // Hermetic: a developer's $GRIM_DEFAULT_REGISTRY must not interpose.
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        assert_eq!(
            resolve_default_registry(&ctx, Some("proj.example"), Some("glob.example")),
            "proj.example"
        );
    }

    #[test]
    fn precedence_global_config_beats_builtin_fallback() {
        // Hermetic: a developer's $GRIM_DEFAULT_REGISTRY must not interpose.
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        assert_eq!(
            resolve_default_registry(&ctx, None, Some("glob.example")),
            "glob.example"
        );
    }

    #[test]
    fn no_registry_anywhere_falls_back_to_builtin() {
        // Hermetic: a developer's $GRIM_DEFAULT_REGISTRY must not leak in.
        // Nothing configured anywhere ⇒ the built-in default applies.
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        assert_eq!(resolve_default_registry(&ctx, None, None), FALLBACK_REGISTRY);
    }

    // ── Contract (e)/(f) — primary_registry_for_scope regression guard ────

    /// Build a minimal `ResolvedScope` from an in-memory registries slice so
    /// tests can exercise `primary_registry_for_scope` without writing disk files.
    fn make_scope(tmp: &tempfile::TempDir, registries: Vec<RegistryConfig>) -> scope_resolution::ResolvedScope {
        use crate::config::declaration::DesiredSet;
        use crate::install::install_state::InstallState;
        use crate::install::path_anchor::AnchorRoots;
        scope_resolution::ResolvedScope {
            scope: crate::config::scope::ConfigScope::Project,
            set: DesiredSet::default(),
            options: crate::config::declaration::ConfigOptions::default(),
            registries,
            config_path: tmp.path().join("grimoire.toml"),
            lock_path: tmp.path().join("grimoire.lock"),
            state_path: InstallState::project_state_path(tmp.path()),
            workspace: tmp.path().to_path_buf(),
            roots: AnchorRoots {
                workspace: tmp.path().to_path_buf(),
                grim_home: tmp.path().to_path_buf(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn primary_registry_for_scope_returns_registries_primary() {
        // Contract (e): primary_registry_for_scope returns the [[registries]]
        // primary when present — NOT the fallback, NOT default_registry.
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        let regs = vec![RegistryConfig {
            alias: None,
            oci: Some("array.example".to_string()),
            index: None,
            default: true,
            ..Default::default()
        }];
        let scope = make_scope(&tmp, regs);
        assert_eq!(
            primary_registry_for_scope(&ctx, &scope).expect("no global config to fail on"),
            "array.example"
        );
    }

    #[test]
    fn primary_registry_for_scope_falls_back_when_no_registries() {
        // Contract (e) boundary: no [[registries]] → legacy chain → fallback.
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        let scope = make_scope(&tmp, vec![]);
        // No registries, no default_registry in options, hermetic ctx (no env) →
        // must fall back to FALLBACK_REGISTRY.
        assert_eq!(
            primary_registry_for_scope(&ctx, &scope).expect("no global config to fail on"),
            FALLBACK_REGISTRY
        );
    }

    // ── Regression guard: a broken global config must not be swallowed ──────
    //
    // `GlobalConfig::load` already maps an ABSENT file to an empty
    // declaration, so every `Err` it returns is a genuinely malformed or
    // invalid global config. Discarding it here made a project-scope run
    // drop every globally-declared registry and still exit 0.

    /// A global config whose `[[registries]]` entry carries an uncompilable
    /// `include` glob — rejected by `validate_registries` (exit 78).
    const MALFORMED_GLOBAL_CONFIG: &str =
        "[[registries]]\nalias = \"acme\"\noci = \"ghcr.io/acme\"\ninclude = [\"acme{unclosed\"]\n";

    #[test]
    fn global_config_tiers_errors_on_a_malformed_global_config() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("grimoire.toml"), MALFORMED_GLOBAL_CONFIG).unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        let err = global_config_tiers(&ctx, crate::config::scope::ConfigScope::Project)
            .expect_err("a malformed global config must surface, not vanish");
        assert_eq!(
            crate::error::classify_error(&err),
            crate::cli::exit_code::ExitCode::ConfigError,
            "a malformed global config is a config error (78): {err:#}"
        );
    }

    #[test]
    fn global_config_tiers_are_empty_when_absent() {
        // The boundary of the guard above: an absent global config is an
        // empty declaration, never an error.
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        assert_eq!(
            global_config_tiers(&ctx, crate::config::scope::ConfigScope::Project).expect("absent ⇒ Ok((vec![], None))"),
            (Vec::new(), None)
        );
    }

    #[test]
    fn global_config_tiers_ignore_a_malformed_global_config_at_global_scope() {
        // A global-scope run resolves the global config as its ACTIVE scope
        // (and errors there); this fallback tier must stay a no-op so the
        // same file is never read — or reported — twice.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("grimoire.toml"), MALFORMED_GLOBAL_CONFIG).unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        assert_eq!(
            global_config_tiers(&ctx, crate::config::scope::ConfigScope::Global)
                .expect("global scope short-circuits before the read"),
            (Vec::new(), None)
        );
    }

    #[test]
    fn global_config_tiers_reads_both_fields_from_one_load() {
        // S-8: the two tiers come from a single `GlobalConfig::load`, so both
        // must be populated by one call — a regression that split them again
        // would recompile every browse-filter glob twice.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("grimoire.toml"),
            "[options]\ndefault_registry = \"legacy.example\"\n\n[[registries]]\nalias = \"acme\"\noci = \"ghcr.io/acme\"\n",
        )
        .unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        let (registries, default) =
            global_config_tiers(&ctx, crate::config::scope::ConfigScope::Project).expect("valid global config");
        assert_eq!(registries.len(), 1, "the [[registries]] tier must survive");
        assert_eq!(default.as_deref(), Some("legacy.example"), "the default tier must too");
    }

    /// W-S2: `global_config_tiers`' "one load, not two" is call-site
    /// discipline, and a second load is invisible to every behavioural test —
    /// it changes no output, only CPU. Measured on one legal 60-entry global
    /// config (release build): `grim context` 12.63 s at one load, `grim fetch`
    /// 21.52 s at two — 1.7× for identical input, because a load on this branch
    /// compiles every browse-filter glob. Two callers had re-split it by
    /// pairing `registries_for_scope` with a second global-config read.
    ///
    /// So pin it at the source level, the deterministic idiom this branch
    /// already uses for a call-site pairing rule (`app.rs`'s
    /// `tui_spells_a_catalog_scope_exactly_once_outside_the_tests_h4`).
    ///
    /// **What this does and does not catch.** It catches a second *loader* —
    /// a helper that reads the global config outside `global_config_tiers`,
    /// which is how the split came back last time. It cannot catch one command
    /// calling the single seam twice; nothing deterministic can, short of
    /// timing.
    #[test]
    fn the_global_config_is_loaded_from_exactly_one_seam_ws2() {
        let source = include_str!("command.rs");
        // Everything from the first `#[cfg(test)]` on is test code, including
        // this assertion's own copy of the needle.
        let production = source.split_once("#[cfg(test)]").map_or(source, |(before, _)| before);
        assert_eq!(
            production.matches("global_config::GlobalConfig::load(").count(),
            1,
            "the global config must be loaded from exactly one place — `global_config_tiers`. \
             A second loader is how the two tiers get read separately again, which recompiles \
             the whole attacker-controlled browse-filter set for no behavioural difference"
        );
    }

    // ── Regression guard: primary_registry_global_fallback ──────────────────
    //
    // These tests lock the contract of the shared Err-branch helper used by
    // `release_default_registry` and `resolve_registry` when scope resolution
    // fails (no project `grimoire.toml`). Before the fix both branches read
    // the default registry from the global `[options]` table alone, ignoring
    // the global `[[registries]]` tier — a user with a `[[registries]]`-only
    // global config always got the built-in fallback instead of their registry.

    #[test]
    fn global_fallback_honors_registries_array_in_global_config() {
        // Regression: [[registries]]-only global config (no [options].default_registry)
        // must resolve to the declared registry, not the built-in fallback.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("grimoire.toml"),
            "[[registries]]\nurl = \"global.example\"\ndefault = true\n",
        )
        .unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        assert_eq!(
            primary_registry_global_fallback(&ctx).expect("valid global config"),
            "global.example"
        );
    }

    #[test]
    fn global_fallback_honors_legacy_default_registry_in_global_config() {
        // A global config with only [options].default_registry (no [[registries]])
        // must still return that value.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("grimoire.toml"),
            "[options]\ndefault_registry = \"legacy.example\"\n",
        )
        .unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        assert_eq!(
            primary_registry_global_fallback(&ctx).expect("valid global config"),
            "legacy.example"
        );
    }

    #[test]
    fn global_fallback_uses_builtin_when_no_global_config() {
        // No global config on disk ⇒ built-in fallback.
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Context::hermetic(tmp.path().to_path_buf());
        assert_eq!(
            primary_registry_global_fallback(&ctx).expect("absent global config"),
            FALLBACK_REGISTRY
        );
    }

    // ── Regression: a broken global config must not strand a credential ────
    //
    // `logout` is what you run when a token leaks, and grim cannot repair the
    // file that blocks it (`config registry rm --global` exits 78 too). A
    // global config that fails to PARSE, or that parses and fails
    // `validate_registries`, must not stop `logout <concrete-host>` — that
    // host needs no registry set to name. `login` WRITES a credential, so it
    // stays strict: an unassembled registry set could send the secret to the
    // wrong host.

    /// Syntactically broken TOML — `GlobalConfig::load` fails at the parser.
    const UNPARSEABLE_GLOBAL_CONFIG: &str = "[[registries]\nalias = \"acme\"\n";

    /// Valid TOML that `validate_registries` rejects (two `default = true`).
    /// The shape no doc mentions: it survives every editor, formatter and
    /// TOML linter, so the user has no local signal that it is broken.
    const INVALID_GLOBAL_CONFIG: &str = "[[registries]]\noci = \"ghcr.io/a\"\ndefault = true\n\n\
         [[registries]]\noci = \"ghcr.io/b\"\ndefault = true\n";

    /// Valid TOML that `validate_registries` rejects (two `default = true`)
    /// which *also* declares the alias a user would log out of. The
    /// distinction from [`INVALID_GLOBAL_CONFIG`] is the whole point of the
    /// salvage: the alias map does not depend on the rule that was broken.
    const INVALID_GLOBAL_CONFIG_WITH_ALIAS: &str = "[[registries]]\nalias = \"acme\"\noci = \"ghcr.io/acme\"\ndefault = true\n\n\
         [[registries]]\noci = \"ghcr.io/b\"\ndefault = true\n";

    /// A hermetic context whose `$GRIM_HOME` holds the given global config.
    fn ctx_with_global_config(tmp: &tempfile::TempDir, body: &str) -> Context {
        std::fs::write(tmp.path().join("grimoire.toml"), body).unwrap();
        Context::hermetic(tmp.path().to_path_buf())
    }

    #[test]
    fn logout_resolves_an_explicit_host_over_a_broken_global_config() {
        for (shape, body) in [
            ("unparseable", UNPARSEABLE_GLOBAL_CONFIG),
            ("invalid", INVALID_GLOBAL_CONFIG),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let ctx = ctx_with_global_config(&tmp, body);
            assert_eq!(
                resolve_login_registry(&ctx, Some("ghcr.io"), GlobalConfigPolicy::Lenient).unwrap_or_else(|e| panic!(
                    "logout must not be blocked by an unrelated {shape} global config: {e:#}"
                )),
                "ghcr.io"
            );
        }
    }

    /// W-S4: the degrade is invisible in the exit code (0) and in the
    /// resolved value (a plausible hostname), so the *only* way a consumer
    /// can tell that an alias declared solely in the global config did not
    /// substitute is this flag. A healthy config must not raise it, or the
    /// disclosure becomes noise and gets ignored.
    #[test]
    fn lenient_resolution_reports_whether_the_global_tier_was_dropped_ws4() {
        for (shape, body) in [
            ("unparseable", UNPARSEABLE_GLOBAL_CONFIG),
            ("invalid", INVALID_GLOBAL_CONFIG),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let ctx = ctx_with_global_config(&tmp, body);
            let (registry, dropped) = resolve_login_registry_lenient(&ctx, Some("acme"))
                .unwrap_or_else(|e| panic!("a {shape} global config must not block logout: {e:#}"));
            assert_eq!(registry, "acme", "an unsubstituted alias stays the literal argument");
            assert!(dropped, "a {shape} global config drops the global tier and must say so");
        }

        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_global_config(&tmp, "[[registries]]\noci = \"ghcr.io/acme\"\nalias = \"acme\"\n");
        let (registry, dropped) =
            resolve_login_registry_lenient(&ctx, Some("acme")).expect("a healthy config resolves");
        assert_eq!(
            registry, "ghcr.io/acme",
            "a readable global tier still substitutes the alias"
        );
        assert!(!dropped, "a healthy global config must not raise the disclosure flag");
    }

    /// W-S4: the degrade WARN fires on *every* broken global config, so it
    /// cannot tell the user whether their invocation was affected. This one
    /// names the argument that fell through — and must not fire when the
    /// alias resolved, or it would cry wolf on every healthy substitution.
    #[test]
    fn fallthrough_warning_fires_only_when_the_degrade_stranded_this_alias_ws4() {
        // Self-contained capture (DAMP): a thread-local subscriber over the
        // one call, mirroring the sibling harnesses in `command/config.rs`
        // and `config/registry_resolve.rs`.
        struct SharedBuf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for SharedBuf {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .expect("log buffer is never poisoned")
                    .extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        fn capture_logs(f: impl FnOnce()) -> String {
            crate::log_switch::tracing_capture::arm();
            let logs = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
            let sink = std::sync::Arc::clone(&logs);
            let guard = tracing::subscriber::set_default(
                tracing_subscriber::fmt()
                    .with_writer(move || SharedBuf(std::sync::Arc::clone(&sink)))
                    .with_ansi(false)
                    .without_time()
                    .finish(),
            );
            f();
            drop(guard);
            String::from_utf8(logs.lock().expect("log buffer is never poisoned").clone()).expect("tracing writes UTF-8")
        }

        // Broken global config + an alias that lived only there: the exact
        // case where exit 0 does not mean "credential erased".
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_global_config(&tmp, UNPARSEABLE_GLOBAL_CONFIG);
        let stranded = capture_logs(|| {
            resolve_login_registry_lenient(&ctx, Some("ac\u{202e}me")).expect("logout is never blocked");
        });
        assert!(
            stranded.contains("matched no configured alias"),
            "the stranded alias must be named for this invocation; got: {stranded:?}"
        );
        // Argv reaching stderr: `is_control` is false for U+202E, so nothing
        // upstream screens it out of the surrounding prose.
        assert!(
            !stranded.contains('\u{202e}') && stranded.contains("ac\\u{202e}me"),
            "the argument must be escaped in the warning; got: {stranded:?}"
        );

        // Two healthy-config cases, and the SECOND is the load-bearing one:
        // `grim logout ghcr.io` matches no alias either, so a warning gated
        // on the fallthrough alone — rather than on the degrade — would fire
        // on the ordinary path every time. Only the first case is covered by
        // `matched.is_some()`, which is why both are here.
        let healthy = "[[registries]]\noci = \"ghcr.io/acme\"\nalias = \"acme\"\n";
        for (case, arg) in [("a substituted alias", "acme"), ("a plain hostname", "ghcr.io")] {
            let tmp = tempfile::tempdir().unwrap();
            let ctx = ctx_with_global_config(&tmp, healthy);
            let quiet = capture_logs(|| {
                resolve_login_registry_lenient(&ctx, Some(arg)).expect("a healthy config resolves");
            });
            assert!(
                !quiet.contains("matched no configured alias"),
                "{case} under a readable global config must not warn; got: {quiet:?}"
            );
        }
    }

    #[test]
    fn login_still_refuses_a_broken_global_config() {
        for (shape, body) in [
            ("unparseable", UNPARSEABLE_GLOBAL_CONFIG),
            ("invalid", INVALID_GLOBAL_CONFIG),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let ctx = ctx_with_global_config(&tmp, body);
            let err = resolve_login_registry(&ctx, Some("ghcr.io"), GlobalConfigPolicy::Strict)
                .expect_err("login writes a credential, so a broken global config stays fatal");
            assert_eq!(
                crate::error::classify_error(&err),
                crate::cli::exit_code::ExitCode::ConfigError,
                "a {shape} global config is a config error (78) for login: {err:#}"
            );
        }
    }

    /// The `logout` half of W-S4, narrowed. A global config that *parsed*
    /// and only failed validation still carries a usable alias map — the
    /// at-most-one-default rule it broke has nothing to do with which url an
    /// alias names. Dropping the whole tier there made `grim logout acme`
    /// resolve to a host literally named `acme`, erase nothing, and exit 0:
    /// success reported for work not done, on a credential path.
    ///
    /// An *unparseable* config is the boundary and stays literal — there is
    /// no alias map to salvage, only bytes that are not TOML.
    ///
    /// The disclosure flag stays `true` in both shapes: the validated tier
    /// really was dropped (`default_registry` with it), and only alias
    /// substitution is recovered.
    #[test]
    fn a_validation_failure_still_substitutes_an_alias_the_config_declares() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_global_config(&tmp, INVALID_GLOBAL_CONFIG_WITH_ALIAS);
        let (registry, dropped) = resolve_login_registry_lenient(&ctx, Some("acme")).expect("logout is never blocked");
        assert_eq!(
            registry, "ghcr.io/acme",
            "a config that parsed still names this alias; erasing the credential for a host \
             literally called 'acme' is a silent no-op reported as success"
        );
        assert!(
            dropped,
            "the validated tier was still dropped — only the alias map is salvaged"
        );

        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_global_config(&tmp, UNPARSEABLE_GLOBAL_CONFIG);
        let (registry, dropped) = resolve_login_registry_lenient(&ctx, Some("acme")).expect("logout is never blocked");
        assert_eq!(
            registry, "acme",
            "bytes that are not TOML carry no alias map; the argument stays literal"
        );
        assert!(dropped, "an unparseable global config still drops the tier");

        // The line between salvage and trusting the file: *which* entry is
        // primary is decided by the very rule that failed, so a bare
        // `logout` recovers nothing and still errors.
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_global_config(&tmp, INVALID_GLOBAL_CONFIG_WITH_ALIAS);
        let err = resolve_login_registry_lenient(&ctx, None)
            .expect_err("a salvaged alias map must not also elect a primary registry");
        assert_eq!(
            crate::error::classify_error(&err),
            crate::cli::exit_code::ExitCode::ConfigError,
            "unchanged from before the salvage: no registry resolves (78): {err:#}"
        );
    }

    /// `login`/`logout` resolved their scope with `resolve(ctx, false, None)`
    /// — `--global` and `--config` hardcoded away, while every other command
    /// passes `ctx.global()` / `ctx.config()`. So `grim --config <path>
    /// logout` silently resolved against a walk-up from the current
    /// directory instead of the file the user named.
    #[test]
    fn bare_logout_honours_the_config_flag_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let config = project.join("grimoire.toml");
        std::fs::write(
            &config,
            "[[registries]]\nalias = \"acme\"\noci = \"ghcr.io/acme\"\ndefault = true\n",
        )
        .unwrap();
        let ctx = Context::hermetic_scoped(tmp.path().to_path_buf(), false, Some(config));
        assert_eq!(
            resolve_login_registry(&ctx, None, GlobalConfigPolicy::Strict)
                .expect("the named config declares a default registry"),
            "ghcr.io/acme",
            "`--config` must select the config `logout` resolves against, as it does for every \
             other command"
        );
    }

    #[test]
    fn global_fallback_flag_registry_overrides_global_config() {
        // Only the --registry flag collapses the browse set — it must win even
        // when a [[registries]] entry is declared in the global config. Note:
        // $GRIM_DEFAULT_REGISTRY is NOT a collapse trigger; it only heads the
        // tier-3 single-default chain when no [[registries]] are declared.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("grimoire.toml"),
            "[[registries]]\nurl = \"global.example\"\ndefault = true\n",
        )
        .unwrap();
        // Inject the flag tier via opts (the flag is in ctx directly via
        // `registry_flag`; no hermetic override needed for this tier).
        let ctx = Context::new(&opts(Some("flag.example")));
        assert_eq!(
            primary_registry_global_fallback(&ctx).expect("valid global config"),
            "flag.example"
        );
    }

    // ── `plain_http_hosts` — config-declared plain-HTTP opt-in ───────────

    /// A project scope on disk whose `grimoire.toml` is `body`, addressed
    /// through `--config` so no walk-up escapes the temp dir.
    fn ctx_with_project_config(tmp: &tempfile::TempDir, body: &str) -> Context {
        let config_path = tmp.path().join("grimoire.toml");
        std::fs::write(&config_path, body).unwrap();
        let grim_home = tmp.path().join("grim-home");
        std::fs::create_dir_all(&grim_home).unwrap();
        Context::hermetic_scoped(grim_home, false, Some(config_path))
    }

    #[test]
    fn plain_http_hosts_adds_the_declared_locator_host_without_its_namespace() {
        // The wiring `access_seam` and `grim login` both depend on: the
        // entry's host reaches the exception list with its PORT and without
        // its namespace, since `HttpsExcept` matches the exact `host[:port]`.
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_project_config(
            &tmp,
            "[skills]\n\n[rules]\n\n[[registries]]\nalias = \"local\"\n\
             oci = \"registry.internal:5050/grimoire\"\ninsecure = true\n\n\
             [[registries]]\nalias = \"corp\"\noci = \"registry.corp/team\"\n",
        );
        let hosts = plain_http_hosts(&ctx);
        assert!(
            hosts.contains(&"registry.internal:5050".to_string()),
            "the namespace must be stripped and the port kept; got: {hosts:?}"
        );
        assert!(
            !hosts.iter().any(|h| h.starts_with("registry.corp")),
            "an entry that did not opt in must not join the set; got: {hosts:?}"
        );
        assert!(
            hosts.contains(&"localhost".to_string()),
            "the implicit loopback set survives; got: {hosts:?}"
        );
    }

    #[test]
    fn plain_http_hosts_degrades_to_the_implicit_set_when_no_scope_resolves() {
        // Browsing outside a project must not fail HERE — the command has its
        // own error path for that, moments later.
        let tmp = tempfile::tempdir().unwrap();
        let grim_home = tmp.path().join("grim-home");
        std::fs::create_dir_all(&grim_home).unwrap();
        let missing = tmp.path().join("nowhere").join("grimoire.toml");
        let ctx = Context::hermetic_scoped(grim_home, false, Some(missing));
        assert_eq!(
            plain_http_hosts(&ctx),
            crate::oci::access::registry_client::plain_http_hosts()
        );
    }

    #[test]
    fn plain_http_hosts_ignores_an_index_entry_that_set_insecure() {
        // `validate_registries` rejects the pairing, so reaching this needs a
        // config that never loaded — but the host must not leak into the
        // exception list on any route.
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_project_config(
            &tmp,
            "[skills]\n\n[rules]\n\n[[registries]]\n\
             index = \"https://index.example\"\n",
        );
        let hosts = plain_http_hosts(&ctx);
        assert!(!hosts.iter().any(|h| h.contains("index.example")), "got: {hosts:?}");
    }

    /// The regression this whole seam was rebuilt for.
    ///
    /// `--registry` collapses the *browse set* to synthesized entries that
    /// carry no config fields — correct for browsing, catastrophic for
    /// transport. While the exception list was derived from
    /// `resolve_registries`, any `--registry` value (even one naming an
    /// unrelated registry) emptied the config half of the list for the whole
    /// invocation, so a declared plain-HTTP host had TLS attempted against
    /// it and every network command failed at exit 69.
    ///
    /// The list is a union that nothing narrows. Both cases below returned
    /// the bare implicit set before the fix.
    #[test]
    fn registry_flag_never_shrinks_the_plain_http_set() {
        let body = "[skills]\n\n[rules]\n\n[[registries]]\nalias = \"local\"\n\
                    oci = \"registry.internal:5050/grimoire\"\ninsecure = true\n";
        for flag in [
            // Pinning the very host that opted in.
            "registry.internal:5050/grimoire",
            // Pinning something else entirely — still must not disarm it.
            "ghcr.io/acme",
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let config_path = tmp.path().join("grimoire.toml");
            std::fs::write(&config_path, body).unwrap();
            let grim_home = tmp.path().join("grim-home");
            std::fs::create_dir_all(&grim_home).unwrap();
            let ctx = Context::hermetic_scoped(grim_home, false, Some(config_path))
                .with_registry_flags(vec![flag.to_string()]);
            assert!(
                plain_http_hosts(&ctx).contains(&"registry.internal:5050".to_string()),
                "--registry {flag} must not disarm a config-declared plain-HTTP host"
            );
        }
    }

    /// `access_seam` must hand the resolved list to the client, not compute
    /// and discard it.
    ///
    /// Mutation testing found this unobservable: reverting `access_seam` to
    /// the pre-feature global list left all 2648 unit tests and 1003 running
    /// acceptance tests green, because every assertion targeted the list
    /// *builder*.
    #[test]
    fn access_seam_hands_the_declared_hosts_to_the_client() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with_project_config(
            &tmp,
            "[skills]\n\n[rules]\n\n[[registries]]\nalias = \"local\"\n\
             oci = \"registry.internal:5050/grimoire\"\ninsecure = true\n",
        );
        access_seam(&ctx).expect("the seam builds against a temp $GRIM_HOME");
        let seen = ctx.plain_http_seen();
        assert_eq!(seen.len(), 1, "exactly one client build; got {seen:?}");
        assert!(
            seen[0].contains(&"registry.internal:5050".to_string()),
            "the declared host must reach the OCI client, not just the builder; got: {seen:?}"
        );
    }

    /// The MCP tools resolve their scope per tool call, so their transport
    /// must follow *that* scope — not the scope the server was launched in.
    ///
    /// Before the fix the browse set came from the call's `workspace`/
    /// `config` arguments while the exception list came from the server's
    /// own cwd, so one project's `insecure` downgraded a call scoped to
    /// another project, and that other project's own opt-in was inert.
    #[test]
    fn a_scoped_resolve_reads_the_scope_it_was_given_not_the_launch_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let entry = |host: &str| {
            format!("[skills]\n\n[rules]\n\n[[registries]]\nalias = \"local\"\noci = \"{host}\"\ninsecure = true\n")
        };
        let launch = tmp.path().join("launch");
        let other = tmp.path().join("other");
        for (dir, host) in [(&launch, "launch.internal:5050"), (&other, "other.internal:5051")] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("grimoire.toml"), entry(host)).unwrap();
        }
        let grim_home = tmp.path().join("grim-home");
        std::fs::create_dir_all(&grim_home).unwrap();
        let ctx = Context::hermetic_scoped(grim_home, false, Some(launch.join("grimoire.toml")));

        // The launch scope answers for the unscoped seam…
        assert!(plain_http_hosts(&ctx).contains(&"launch.internal:5050".to_string()));

        // …and a call naming another project answers for that one, only.
        let scoped = declared_insecure_hosts(&ctx, false, Some(&other.join("grimoire.toml")), None);
        assert_eq!(
            scoped,
            vec!["other.internal:5051".to_string()],
            "a per-call scope must not inherit the launch scope's opt-in, nor lose its own"
        );
    }

    /// The infallibility guarantee, on the branch that can actually fail.
    ///
    /// The absent-config case does not reach it — an absent global config is
    /// `Ok`. A *malformed* one is the error branch, and a `?` or `.expect()`
    /// here would turn it into a hard failure of every command that touches
    /// the network, including ones pointed elsewhere with `--config`.
    #[test]
    fn plain_http_hosts_degrades_when_the_global_config_is_unreadable() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("grimoire.toml"), MALFORMED_GLOBAL_CONFIG).unwrap();
        let missing = tmp.path().join("nowhere").join("grimoire.toml");
        let ctx = Context::hermetic_scoped(tmp.path().to_path_buf(), false, Some(missing));
        assert_eq!(
            plain_http_hosts(&ctx),
            crate::oci::access::registry_client::plain_http_hosts(),
            "an unreadable global config must contribute nothing, not fail here"
        );
    }
}
