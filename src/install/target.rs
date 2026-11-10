// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The set of AI client targets an install/update writes to.
//!
//! A list of [`ClientTarget`]s rooted at a workspace. The installer
//! iterates the targets, materializing each artifact into every selected
//! client's layout, so one install can generate for several clients at
//! once (e.g. Claude and OpenCode).
//!
//! When neither `--client` nor the config `[options].clients` selects a
//! client, the set defaults to **all detected clients** — those whose
//! vendor directory / marker is present for the scope (see
//! [`detect_clients`]) — plus, at project scope, the clients the global
//! `[options].clients` lists. Neither finding anything falls back to the
//! single synthetic generic client [`ClientTarget::Agents`], which writes one copy
//! into the cross-vendor `.agents/skills` pool. It deliberately does **not**
//! fall back to every known client: that wrote eleven vendor directories the
//! user never asked for, and those directories are exactly what made the
//! *next* run "detect" every client — a fallback that manufactures its own
//! detection signal, unrecoverably.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::declaration::VendorOptions;
use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;

use super::client_target::ClientTarget;
use super::install_error::InstallError;

/// What put a client into an [`InstallTarget`]'s set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientSource {
    /// The `--client` flag, or a client list handed straight to
    /// [`InstallTarget::new`].
    Flag,
    /// The project's `[options].clients`.
    Project,
    /// A vendor marker present in the workspace.
    Detected,
    /// The global `[options].clients`, for a client with no marker here.
    Global,
    /// Nothing selected or detected anything: the generic `agents` client.
    Fallback,
}

impl std::fmt::Display for ClientSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Flag => "--client",
            Self::Project => "project",
            Self::Detected => "detected",
            Self::Global => "global",
            Self::Fallback => "fallback",
        })
    }
}

/// One or more AI client targets rooted at a workspace.
#[derive(Debug, Clone)]
pub struct InstallTarget {
    workspace: PathBuf,
    scope: ConfigScope,
    clients: Vec<ClientTarget>,
    /// What selected each entry of `clients`, index-aligned. A lone
    /// [`ClientSource::Fallback`] marks the generic client substituted when
    /// flag, config, detection and the global list were all empty; it gates
    /// the residual "nothing here is installable" error.
    sources: Vec<ClientSource>,
    /// Clients whose skills render into the shared `.agents/skills` pool
    /// instead of their native skills directory
    /// (`[options.vendors.<name>].shared_skills = true`).
    ///
    /// Derived in [`Self::parse`] from the resolved scope's `[options.vendors]`
    /// table — never merged across scopes, never overridable by a flag.
    shared_skills: Vec<ClientTarget>,
    /// Clients opted out of the pool (`shared_skills = false`): their skills
    /// stay native even beside a client that writes the pool.
    native_skills: Vec<ClientTarget>,
    /// The resolved scope's `[options].clients`, parsed before detection;
    /// `None` when empty (autodetect). Set only by [`Self::parse`].
    configured: Option<Vec<ClientTarget>>,
    /// The resolved hook arming policy, or `None` when this target was not
    /// built by a mutating command. Attached by [`Self::with_hook_policy`] at
    /// the mutating boundary, never by [`Self::parse`] (shared with the
    /// read-only `status`/`search`/`context`), so a read-only command can
    /// neither arm nor prompt.
    hook_policy: Option<crate::hook::policy::HookPolicy>,
    /// The resolved settings policy. `None` fails closed: the settings step
    /// plans nothing. Attached like [`Self::hook_policy`], and for the same
    /// reason never by [`Self::parse`].
    settings_policy: Option<crate::hook::policy::HookPolicy>,
    /// `$GRIM_HOME`, the one input a hook destination needs that a client
    /// layout does not (C-106): a hook payload is machine-local at both scopes
    /// (invariant I1), so [`Self::path_for`] routes `Hook` under it.
    ///
    /// Resolved at construction through [`crate::env::grim_home`], the same
    /// resolution [`crate::context::Context`] (and therefore
    /// [`AnchorRoots`](super::path_anchor::AnchorRoots)) uses, so the three-
    /// argument `path_for` keeps main's signature at every hook-free call site.
    grim_home: PathBuf,
}

impl InstallTarget {
    /// Build a target for the given clients rooted at `workspace` for
    /// `scope` (global scope resolves vendor-native user-level paths).
    ///
    /// An empty `clients` list defaults to [`detect_clients_or_all`], so this
    /// constructor never produces an empty (silent no-op) target.
    ///
    /// **The generic-client fallback is NOT here — it lives in
    /// [`Self::parse`].** Every production path resolves through `parse`;
    /// `new` is reached with an empty list only from unit tests, whose
    /// rules-only fixtures depend on the historic all-clients behaviour. A
    /// new production call site must go through `parse`, not this.
    ///
    /// Same reason `shared_skills` is empty here and only ever populated by
    /// `parse`: it comes from the resolved scope's config, which `new` does
    /// not take.
    pub fn new(workspace: &Path, scope: ConfigScope, clients: Vec<ClientTarget>) -> Self {
        let (clients, source) = if clients.is_empty() {
            (detect_clients_or_all(workspace, scope), ClientSource::Detected)
        } else {
            (clients, ClientSource::Flag)
        };
        Self {
            workspace: workspace.to_path_buf(),
            scope,
            sources: vec![source; clients.len()],
            clients,
            shared_skills: Vec::new(),
            native_skills: Vec::new(),
            configured: None,
            hook_policy: None,
            settings_policy: None,
            grim_home: crate::env::grim_home(),
        }
    }

    /// Parse a comma-separated / repeated `--client` list into an
    /// [`InstallTarget`]. An empty flag list falls back to the config
    /// `clients` default; when that is also empty, the detected clients for
    /// `scope` are used; when detection is *also* empty, the single generic
    /// [`ClientTarget::Agents`] client is substituted and the target is
    /// marked as the fallback (see [`Self::is_generic_fallback`]). Each value
    /// (flag or config) may itself be a comma list.
    ///
    /// This is the single seam every mutating command resolves through, so
    /// the fallback lives here rather than in [`detect_clients`] — that
    /// function's read-only consumers (`status`, `search`, the TUI badge
    /// sites) must keep working on a bare workspace.
    ///
    /// `vendors` is the resolved scope's raw `[options.vendors]` table (read
    /// off [`ConfigOptions`](crate::config::declaration::ConfigOptions), never
    /// `ResolvedOptions` — a missing entry already means "every field at its
    /// resting state"). It is unrelated to client *selection*: an opted-in
    /// client that the flag/config/detection chain did not select is simply
    /// not installed for.
    ///
    /// # Errors
    ///
    /// [`super::install_error::InstallErrorKind::UnsupportedClient`] for an
    /// unknown client name.
    pub fn parse(
        workspace: &Path,
        scope: ConfigScope,
        flag_values: &[String],
        config_default: &[String],
        vendors: &BTreeMap<String, VendorOptions>,
    ) -> Result<Self, InstallError> {
        Self::parse_with_global(workspace, scope, flag_values, config_default, &[], vendors)
    }

    /// [`Self::parse`] with the global `[options].clients` joined to the
    /// detected clients.
    ///
    /// The global list only counts when neither `--client` nor the project's
    /// `[options].clients` selects anything, and only at project scope — a
    /// global run's own list already is the global one. Then the set is every
    /// detected client plus every `global_default` client, in
    /// [`ClientTarget::ALL`] order; a client both detected and listed reads
    /// [`ClientSource::Detected`]. Nothing detected with a non-empty list
    /// selects the list alone, not the generic fallback. An empty
    /// `global_default` is exactly [`Self::parse`].
    ///
    /// `global_default` arrives parsed: the global config loader has already
    /// refused an unknown name, so the caller never has a half-known list to
    /// handle.
    /// [`Self::configured_clients`] stays the project list, so the global list
    /// never turns on a reap or the configured-vs-recorded drift fields.
    ///
    /// # Errors
    ///
    /// [`super::install_error::InstallErrorKind::UnsupportedClient`] for an
    /// unknown name in `flag_values` or `config_default`.
    pub fn parse_with_global(
        workspace: &Path,
        scope: ConfigScope,
        flag_values: &[String],
        config_default: &[String],
        global_default: &[ClientTarget],
        vendors: &BTreeMap<String, VendorOptions>,
    ) -> Result<Self, InstallError> {
        let (source, selected_by): (&[String], _) = if flag_values.is_empty() {
            // A global run's own list is the global one, so that is what it reads.
            let by = match scope {
                ConfigScope::Project => ClientSource::Project,
                ConfigScope::Global => ClientSource::Global,
            };
            (config_default, by)
        } else {
            (flag_values, ClientSource::Flag)
        };
        // Both flag and config empty ⇒ detection runs (do not inject the
        // literal "claude").
        let selected = parse_client_list(source)?;
        // A `--client` selection narrows one run; only the config list says
        // which clients the project keeps, so the flag never fills this.
        let configured = if flag_values.is_empty() {
            selected.clone()
        } else {
            parse_client_list(config_default).unwrap_or_else(|error| {
                tracing::warn!("ignoring [options].clients for the adopted-entry carry-forward: {error}");
                Vec::new()
            })
        };
        let configured = (!configured.is_empty()).then_some(configured);

        // An unknown client name, or one the capability gate would have
        // refused, is dropped rather than raised: both are already refused at
        // the config surface (exit 64 / 65 at `config set`, 78 at load), so a
        // survivor here can only come from a config this process did not
        // validate. Silently ignoring it keeps the resting (native) layout —
        // rendering into a pool the client never reads is the failure this
        // whole feature exists to prevent.
        let shared_skills: Vec<ClientTarget> = vendors
            .iter()
            .filter(|(_, options)| options.shared_skills == Some(true))
            .filter_map(|(name, _)| name.parse::<ClientTarget>().ok())
            .filter(|client| client.vendor().pool_capable())
            .collect();
        let native_skills: Vec<ClientTarget> = vendors
            .iter()
            .filter(|(_, options)| options.shared_skills == Some(false))
            .filter_map(|(name, _)| name.parse::<ClientTarget>().ok())
            .collect();

        let (clients, sources) = if selected.is_empty() {
            let global: &[ClientTarget] = match scope {
                ConfigScope::Project => global_default,
                ConfigScope::Global => &[],
            };
            union_with_detected(detect_clients(workspace, scope), global)
        } else {
            let sources = vec![selected_by; selected.len()];
            (selected, sources)
        };
        Ok(Self {
            workspace: workspace.to_path_buf(),
            scope,
            clients,
            sources,
            shared_skills,
            native_skills,
            configured,
            hook_policy: None,
            settings_policy: None,
            grim_home: crate::env::grim_home(),
        })
    }

    /// Attach the resolved hook arming policy — **mutating commands only**.
    #[must_use]
    pub fn with_hook_policy(mut self, policy: crate::hook::policy::HookPolicy) -> Self {
        self.hook_policy = Some(policy);
        self
    }

    /// The resolved hook arming policy, or `None` when no mutating boundary
    /// attached one: this invocation neither arms nor reaps.
    pub fn hook_policy(&self) -> Option<&crate::hook::policy::HookPolicy> {
        self.hook_policy.as_ref()
    }

    /// Attach the resolved settings policy. Only a mutating command does.
    #[must_use]
    pub fn with_settings_policy(mut self, policy: crate::hook::policy::HookPolicy) -> Self {
        self.settings_policy = Some(policy);
        self
    }

    /// The resolved settings policy, or `None` when no mutating boundary
    /// attached one.
    pub fn settings_policy(&self) -> Option<&crate::hook::policy::HookPolicy> {
        self.settings_policy.as_ref()
    }

    /// The `grim` a remedy names. Every command that installs attaches a
    /// policy; a target that holds none (the settings uninstall pass) names its
    /// scope's flag-only form, as an unnamed policy does.
    pub fn grim(&self) -> &crate::config::scope::Invocation {
        use crate::config::scope::Invocation;
        let by_scope = match self.scope {
            ConfigScope::Global => &Invocation::GLOBAL,
            ConfigScope::Project => &Invocation::WALKED,
        };
        self.hook_policy
            .as_ref()
            .or(self.settings_policy.as_ref())
            .map_or(by_scope, crate::hook::policy::HookPolicy::invocation)
    }

    /// The `$GRIM_HOME` a hook destination resolves under (C-106).
    pub fn grim_home(&self) -> &Path {
        &self.grim_home
    }

    /// Replace the resolved `$GRIM_HOME` — hermetic tests only, whose
    /// [`AnchorRoots`](super::path_anchor::AnchorRoots) point at a temp dir
    /// rather than the ambient environment.
    #[cfg(test)]
    #[must_use]
    pub fn with_grim_home(mut self, grim_home: &Path) -> Self {
        self.grim_home = grim_home.to_path_buf();
        self
    }

    /// The client targets, in declared order (deduplicated).
    pub fn clients(&self) -> &[ClientTarget] {
        &self.clients
    }

    /// Each selected client with what put it there, in [`Self::clients`] order.
    pub fn client_sources(&self) -> impl Iterator<Item = (ClientTarget, ClientSource)> + '_ {
        self.clients.iter().copied().zip(self.sources.iter().copied())
    }

    /// The resolved scope's `[options].clients`, parsed; `None` when it is
    /// empty or unset (autodetect). A `--client` selection never counts.
    pub fn configured_clients(&self) -> Option<&[ClientTarget]> {
        self.configured.as_deref()
    }

    /// True when nothing selected this target: no `--client`, no config
    /// `[options].clients`, and nothing detected, so the set is the single
    /// generic [`ClientTarget::Agents`] client. An explicit `--client agents`
    /// is **not** a fallback — the user chose it.
    ///
    /// Gates the residual "nothing here is installable" refusal
    /// (`installer::refuse_uninstallable_fallback`), which is the only
    /// consumer: the generic client renders skills only, so a fallback target
    /// whose whole artifact set is declined has no destination at all.
    pub fn is_generic_fallback(&self) -> bool {
        self.sources == [ClientSource::Fallback]
    }

    /// The workspace root the client roots sit under.
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// The scope this target installs for.
    pub fn scope(&self) -> ConfigScope {
        self.scope
    }

    /// The install path for `(kind, name)` under `client`.
    ///
    /// A client opted into `shared_skills` has its **skills** — and only its
    /// skills — routed to the cross-vendor pool, by borrowing the generic
    /// client's skills root (the pool's single definition, at both scopes)
    /// rather than restating the path here. Every other kind, and every
    /// client that did not opt in, keeps its native layout untouched.
    ///
    /// Every client that already renders into the pool resolves its skills
    /// root through that exact same helper, so opting one of them in is a
    /// genuine no-op rather than a coincidence. `render.rs`'s `POOL_ROSTER` is
    /// the authoritative list; deliberately not restated here, since a count
    /// goes stale the next time a client joins. `goose` is one of them and is
    /// newly pool-capable, so `[options.vendors.goose].shared_skills = true`
    /// is an accepted config today — and it is this no-op, not a second write
    /// path.
    ///
    /// A **hook** payload never lands in the workspace, at either scope, and
    /// never depends on the client (S-003, invariant I1): it resolves under
    /// [`Self::grim_home`] via
    /// [`hook_dispatch::payload_dir`](super::hook_dispatch::payload_dir).
    /// Every other kind returns exactly what main's form returns (C-106).
    ///
    /// For a skill this is the destination *before* the pool auto-join; a
    /// caller that writes or compares a skill output goes through
    /// [`expected_outputs::destination`](super::expected_outputs::destination).
    pub fn path_for(&self, client: ClientTarget, kind: ArtifactKind, name: &str) -> PathBuf {
        if kind == ArtifactKind::Hook {
            return super::hook_dispatch::payload_dir(
                &self.grim_home,
                super::hook_registrar::root_scope_for(&self.workspace, self.scope),
                name,
            );
        }
        if kind == ArtifactKind::Skill && self.shared_skills.contains(&client) {
            return ClientTarget::Agents.path_for(&self.workspace, self.scope, kind, name);
        }
        client.path_for(&self.workspace, self.scope, kind, name)
    }

    /// Whether `client`'s skills render into the shared `.agents/skills`
    /// pool rather than its native skills directory (see [`Self::path_for`]).
    pub fn is_shared_skills(&self, client: ClientTarget) -> bool {
        self.shared_skills.contains(&client)
    }

    /// Whether `client`'s skill `name` lands in the pool on its own: a
    /// default pool writer, or a client set to `shared_skills = true`.
    ///
    /// Never depends on another client, so two auto-joining readers cannot
    /// pull each other into the pool.
    fn lands_in_pool(&self, client: ClientTarget, name: &str) -> bool {
        self.path_for(client, ArtifactKind::Skill, name) == self.pool_path(name)
    }

    /// The shared pool path for skill `name` at this target's scope.
    pub fn pool_path(&self, name: &str) -> PathBuf {
        ClientTarget::Agents.path_for(&self.workspace, self.scope, ArtifactKind::Skill, name)
    }

    /// Whether `client` auto-joins the pool for skill `name` because another
    /// client among `peers` lands there, so the reader would otherwise see the
    /// skill twice (issue #262).
    ///
    /// Only an unset `shared_skills` auto-joins: `false` keeps the native
    /// layout, and a client that already lands in the pool has nothing to
    /// join. The reader must also scan the pool in this environment, or a
    /// joined skill would go unread.
    pub fn joins_pool(&self, client: ClientTarget, name: &str, peers: &[ClientTarget]) -> bool {
        let vendor = client.vendor();
        !self.native_skills.contains(&client)
            && vendor.pool_capable()
            && !self.lands_in_pool(client, name)
            && vendor.reads_pool(self.scope)
            && peers
                .iter()
                .any(|peer| *peer != client && self.lands_in_pool(*peer, name))
    }
}

/// Parse `--client`-style values into client targets: each value may be a
/// comma list; names are trimmed, empties dropped, duplicates removed in
/// first-seen order. An empty result is returned as-is — the detection and
/// generic fallbacks belong to [`InstallTarget::parse`], not to parsing.
///
/// # Errors
///
/// [`super::install_error::InstallErrorKind::UnsupportedClient`] for an
/// unknown client name.
pub(crate) fn parse_client_list(values: &[String]) -> Result<Vec<ClientTarget>, InstallError> {
    let raw: Vec<String> = values
        .iter()
        .flat_map(|v| v.split(',').map(|s| s.trim().to_string()))
        .collect();

    let mut clients = Vec::new();
    for name in raw {
        if name.is_empty() {
            continue;
        }
        let client: ClientTarget = name.parse()?;
        if !clients.contains(&client) {
            clients.push(client);
        }
    }
    Ok(clients)
}

/// True when `values` names no client: empty, or only blank entries. The
/// question [`InstallTarget::parse_with_global`] asks before it lets the global
/// list in, so a reader of the active set can ask it too.
pub(crate) fn names_no_client(values: &[String]) -> bool {
    values
        .iter()
        .flat_map(|v| v.split(','))
        .all(|name| name.trim().is_empty())
}

/// The detected clients joined with the global list, in [`ClientTarget::ALL`]
/// order, each with its source. Both empty is the generic fallback.
fn union_with_detected(detected: Vec<ClientTarget>, global: &[ClientTarget]) -> (Vec<ClientTarget>, Vec<ClientSource>) {
    let (clients, sources): (Vec<_>, Vec<_>) = ClientTarget::ALL
        .into_iter()
        .filter_map(|client| {
            if detected.contains(&client) {
                Some((client, ClientSource::Detected))
            } else {
                global.contains(&client).then_some((client, ClientSource::Global))
            }
        })
        .unzip();
    if clients.is_empty() {
        (vec![ClientTarget::Agents], vec![ClientSource::Fallback])
    } else {
        (clients, sources)
    }
}

/// The detected AI clients for `workspace` at `scope`, in
/// [`ClientTarget::ALL`] order: every client whose vendor directory /
/// marker is present (see [`super::vendor::Vendor::detect`]).
///
/// **Raw** — an empty result means "nothing detected" and is returned as
/// such. Selecting what to do about that belongs to the caller:
/// [`InstallTarget::parse`] substitutes the generic client, while the
/// read-only consumers use [`detect_clients_or_all`].
pub fn detect_clients(workspace: &Path, scope: ConfigScope) -> Vec<ClientTarget> {
    ClientTarget::ALL
        .into_iter()
        .filter(|c| c.vendor().detect(workspace, scope))
        .collect()
}

/// [`detect_clients`], falling back to **all** clients when nothing is
/// detected.
///
/// The permissive reading, for read-only consumers that reconcile *recorded*
/// outputs against "which clients might be present" — `grim status`,
/// `grim search`, and the TUI badge derivation. Answering "none" there would
/// make an installed artifact report as having no outputs at all on a
/// workspace whose marker dirs were since removed.
///
/// Never use this to decide where to **write**: it is the fallback whose
/// side effects (eleven vendor directories) manufactured the detection
/// signal for the next run.
pub fn detect_clients_or_all(workspace: &Path, scope: ConfigScope) -> Vec<ClientTarget> {
    let detected = detect_clients(workspace, scope);
    if detected.is_empty() {
        ClientTarget::ALL.to_vec()
    } else {
        detected
    }
}

/// [`detect_clients_or_all`] joined with the global `[options].clients`, in
/// [`ClientTarget::ALL`] order: the clients a read-only command treats as
/// present. A global-list client has no marker in the workspace, so without
/// it a deleted output of that client would read as one the user dropped.
pub fn active_clients(workspace: &Path, scope: ConfigScope, global: &[ClientTarget]) -> Vec<ClientTarget> {
    let active = detect_clients_or_all(workspace, scope);
    ClientTarget::ALL
        .into_iter()
        .filter(|client| active.contains(client) || global.contains(client))
        .collect()
}

/// One line naming each client of `target` and what selected it, for the
/// `clients:` note `install` and `status` print on stderr.
pub fn clients_note(target: &InstallTarget) -> String {
    let named: Vec<String> = target
        .client_sources()
        .map(|(client, source)| format!("{client} ({source})"))
        .collect();
    format!("clients: {}", named.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_with_empty_list_keeps_the_all_clients_fallback() {
        // `new` is the permissive constructor: an empty list still resolves
        // to every client when nothing is detected. Only `parse` — the seam
        // every mutating command uses — substitutes the generic client.
        let tmp = tempfile::tempdir().unwrap();
        let t = InstallTarget::new(tmp.path(), ConfigScope::Project, vec![]);
        assert_eq!(t.clients(), &ClientTarget::ALL);
    }

    #[test]
    fn empty_targets_detected_clients_in_all_order() {
        // `.opencode` + `.github/instructions` present, no `.claude` ⇒ the
        // detected set is [OpenCode, Copilot] in ClientTarget::ALL order.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".opencode")).unwrap();
        std::fs::create_dir_all(tmp.path().join(".github").join("instructions")).unwrap();
        let t = InstallTarget::new(tmp.path(), ConfigScope::Project, vec![]);
        assert_eq!(t.clients(), &[ClientTarget::OpenCode, ClientTarget::Copilot]);
        // The same set reaches detection through `parse` (empty flag+config).
        let p = InstallTarget::parse(tmp.path(), ConfigScope::Project, &[], &[], &BTreeMap::new()).unwrap();
        assert_eq!(p.clients(), &[ClientTarget::OpenCode, ClientTarget::Copilot]);
    }

    #[test]
    fn explicit_config_overrides_detection() {
        // Even with `.opencode` present, an explicit config `clients`
        // declaration wins over detection.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".opencode")).unwrap();
        let t = InstallTarget::parse(
            tmp.path(),
            ConfigScope::Project,
            &[],
            &["claude".to_string()],
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(t.clients(), &[ClientTarget::Claude]);
    }

    #[test]
    fn detect_clients_is_raw_and_or_all_is_the_permissive_wrapper() {
        // Project scope on a bare workspace is hermetic (global detection
        // reads the developer's real `~/.claude` etc.). The raw function
        // reports the truth — nothing; the `_or_all` wrapper keeps the
        // historic permissive answer for the read-only consumers.
        let tmp = tempfile::tempdir().unwrap();
        assert!(
            detect_clients(tmp.path(), ConfigScope::Project).is_empty(),
            "raw detection must report an empty set, not invent one"
        );
        assert_eq!(
            detect_clients_or_all(tmp.path(), ConfigScope::Project),
            ClientTarget::ALL.to_vec()
        );
    }

    #[test]
    fn parse_falls_back_to_the_generic_client_when_nothing_is_detected() {
        // Flag, config, and detection all empty ⇒ the single generic client,
        // NOT every known client (which scattered eleven vendor directories).
        let tmp = tempfile::tempdir().unwrap();
        let t = InstallTarget::parse(tmp.path(), ConfigScope::Project, &[], &[], &BTreeMap::new()).unwrap();
        assert_eq!(t.clients(), &[ClientTarget::Agents]);
        assert!(t.is_generic_fallback());
    }

    #[test]
    fn the_fallback_does_not_manufacture_its_own_detection_signal() {
        // The regression test for the original defect: materializing into the
        // pool must not make the *next* run resolve differently. Two runs on a
        // bare workspace both resolve to `agents` alone.
        let tmp = tempfile::tempdir().unwrap();
        let first = InstallTarget::parse(tmp.path(), ConfigScope::Project, &[], &[], &BTreeMap::new()).unwrap();
        assert_eq!(first.clients(), &[ClientTarget::Agents]);

        // Simulate what the install writes.
        std::fs::create_dir_all(tmp.path().join(".agents").join("skills").join("demo")).unwrap();

        let second = InstallTarget::parse(tmp.path(), ConfigScope::Project, &[], &[], &BTreeMap::new()).unwrap();
        assert_eq!(
            second.clients(),
            &[ClientTarget::Agents],
            "the pool directory must not become a detection signal for any client"
        );
    }

    #[test]
    fn explicit_agents_selection_is_not_a_fallback() {
        // `--client agents` is a choice, so the residual refusal must not fire
        // for it even on a rules-only artifact set. Same for a config default.
        let tmp = tempfile::tempdir().unwrap();
        for (flag, cfg) in [
            (vec!["agents".to_string()], vec![]),
            (vec![], vec!["agents".to_string()]),
        ] {
            let t = InstallTarget::parse(tmp.path(), ConfigScope::Project, &flag, &cfg, &BTreeMap::new()).unwrap();
            assert_eq!(t.clients(), &[ClientTarget::Agents]);
            assert!(
                !t.is_generic_fallback(),
                "an explicitly named generic client is a selection, not a fallback"
            );
        }
    }

    #[test]
    fn a_detected_client_never_reaches_the_generic_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        let t = InstallTarget::parse(tmp.path(), ConfigScope::Project, &[], &[], &BTreeMap::new()).unwrap();
        assert_eq!(t.clients(), &[ClientTarget::Claude]);
        assert!(!t.is_generic_fallback());
    }

    #[test]
    fn parse_comma_list_dedups_and_orders() {
        let t = InstallTarget::parse(
            Path::new("/w"),
            ConfigScope::Project,
            &["claude,copilot".to_string()],
            &[],
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(t.clients(), &[ClientTarget::Claude, ClientTarget::Copilot]);
        // Repeated flag values merge.
        let t2 = InstallTarget::parse(
            Path::new("/w"),
            ConfigScope::Project,
            &["copilot".to_string(), "copilot".to_string(), "claude".to_string()],
            &[],
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(t2.clients(), &[ClientTarget::Copilot, ClientTarget::Claude]);
    }

    #[test]
    fn parse_falls_back_to_config_default() {
        // A config `clients` list (here two entries) is used when no flag.
        let t = InstallTarget::parse(
            Path::new("/w"),
            ConfigScope::Project,
            &[],
            &["opencode".to_string(), "claude".to_string()],
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(t.clients(), &[ClientTarget::OpenCode, ClientTarget::Claude]);
        // `/w` does not exist ⇒ nothing detected ⇒ the generic client.
        let t2 = InstallTarget::parse(Path::new("/w"), ConfigScope::Project, &[], &[], &BTreeMap::new()).unwrap();
        assert_eq!(t2.clients(), &[ClientTarget::Agents]);
        // A flag list overrides the config default entirely.
        let t3 = InstallTarget::parse(
            Path::new("/w"),
            ConfigScope::Project,
            &["copilot".to_string()],
            &["claude".to_string()],
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(t3.clients(), &[ClientTarget::Copilot]);
    }

    #[test]
    fn parse_rejects_unknown_client() {
        assert!(
            InstallTarget::parse(
                Path::new("/w"),
                ConfigScope::Project,
                &["vscode".to_string()],
                &[],
                &BTreeMap::new()
            )
            .is_err()
        );
    }

    /// ⛔ The negative the policy/consent split exists to guarantee: `parse`
    /// is also the seam `grim status`, `grim search` and `grim context` use, so
    /// it must never derive a hook policy — a read-only command must be unable
    /// to arm, reap or prompt.
    #[test]
    fn parse_never_attaches_a_hook_policy_so_a_read_only_command_cannot_arm_or_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        for (flag, cfg) in [
            (vec![], vec![]),
            (vec!["claude".to_string()], vec![]),
            (vec![], vec!["claude,codex".to_string()]),
        ] {
            let t = InstallTarget::parse(tmp.path(), ConfigScope::Project, &flag, &cfg, &BTreeMap::new()).unwrap();
            assert!(
                t.hook_policy().is_none(),
                "InstallTarget::parse must never derive a hook policy"
            );
        }
        assert!(
            InstallTarget::new(tmp.path(), ConfigScope::Project, vec![ClientTarget::Claude])
                .hook_policy()
                .is_none()
        );
    }

    #[test]
    fn with_hook_policy_is_the_only_way_a_target_gains_one() {
        let tmp = tempfile::tempdir().unwrap();
        let policy = crate::hook::policy::HookPolicy::new(
            true,
            None,
            crate::hook::trust::Interactivity::NonInteractive,
            ConfigScope::Project,
            tmp.path(),
            &[],
            crate::hook::consent::Consent::Granted,
        );
        let t = InstallTarget::parse(tmp.path(), ConfigScope::Project, &[], &[], &BTreeMap::new())
            .unwrap()
            .with_hook_policy(policy);
        assert!(
            t.hook_policy()
                .is_some_and(crate::hook::policy::HookPolicy::feature_enabled)
        );
    }

    /// `parse` is the seam `grim status`, `grim search` and `grim context`
    /// share, so a target it builds cannot apply settings.
    #[test]
    fn a_target_built_by_parse_carries_no_settings_policy() {
        let tmp = tempfile::tempdir().unwrap();
        for (flag, cfg) in [
            (vec![], vec![]),
            (vec!["claude".to_string()], vec![]),
            (vec![], vec!["claude,codex".to_string()]),
        ] {
            let t = InstallTarget::parse(tmp.path(), ConfigScope::Project, &flag, &cfg, &BTreeMap::new()).unwrap();
            assert!(t.settings_policy().is_none());
        }
        assert!(
            InstallTarget::new(tmp.path(), ConfigScope::Project, vec![ClientTarget::Claude])
                .settings_policy()
                .is_none()
        );
    }

    /// The two policies travel apart: each builder sets its own field.
    #[test]
    fn each_policy_builder_sets_only_its_own_field() {
        let tmp = tempfile::tempdir().unwrap();
        let policy = |feature_enabled| {
            crate::hook::policy::HookPolicy::new(
                feature_enabled,
                None,
                crate::hook::trust::Interactivity::NonInteractive,
                ConfigScope::Project,
                tmp.path(),
                &[],
                crate::hook::consent::Consent::Granted,
            )
        };
        let parsed = || InstallTarget::parse(tmp.path(), ConfigScope::Project, &[], &[], &BTreeMap::new()).unwrap();

        let settings_only = parsed().with_settings_policy(policy(true));
        assert!(
            settings_only
                .settings_policy()
                .is_some_and(crate::hook::policy::HookPolicy::feature_enabled)
        );
        assert!(settings_only.hook_policy().is_none());

        let both = parsed()
            .with_hook_policy(policy(true))
            .with_settings_policy(policy(false));
        assert!(
            both.hook_policy()
                .is_some_and(crate::hook::policy::HookPolicy::feature_enabled)
        );
        assert!(
            !both
                .settings_policy()
                .is_some_and(crate::hook::policy::HookPolicy::feature_enabled)
        );
    }

    /// A failed install's remedy reads this, so a `--config` run keeps its path.
    #[test]
    fn a_target_names_the_grim_of_its_policy() {
        let tmp = tempfile::tempdir().unwrap();
        let parse = || InstallTarget::parse(tmp.path(), ConfigScope::Project, &[], &[], &BTreeMap::new()).unwrap();
        let grim = crate::config::scope::Invocation::config(Path::new("/srv/team/grimoire.toml"));
        let policy = crate::hook::policy::HookPolicy::new(
            true,
            None,
            crate::hook::trust::Interactivity::NonInteractive,
            ConfigScope::Project,
            tmp.path(),
            &[],
            crate::hook::consent::Consent::Granted,
        )
        .naming(grim.clone());

        assert_eq!(parse().grim(), &crate::config::scope::Invocation::WALKED);
        assert_eq!(parse().with_hook_policy(policy).grim(), &grim);
    }

    /// A global run names `grim --global` whether or not it attached a policy:
    /// a plain `grim` would act on the project the shell stands in.
    #[test]
    fn a_global_target_without_a_policy_names_the_global_grim() {
        let tmp = tempfile::tempdir().unwrap();
        let target = InstallTarget::new(tmp.path(), ConfigScope::Global, vec![ClientTarget::Kiro]);

        assert_eq!(target.grim(), &crate::config::scope::Invocation::GLOBAL);
    }

    /// C-106 (a): main's `path_for` body, frozen. Every hook-free
    /// `(client, kind, scope)` — with and without the shared-skills pool —
    /// must resolve exactly as main did, so the `grim_home` field moves no
    /// hook-free byte.
    #[test]
    fn path_for_matches_mains_body_for_every_hook_free_kind() {
        fn mains_path_for(t: &InstallTarget, client: ClientTarget, kind: ArtifactKind, name: &str) -> PathBuf {
            if kind == ArtifactKind::Skill && t.shared_skills.contains(&client) {
                return ClientTarget::Agents.path_for(&t.workspace, t.scope, kind, name);
            }
            client.path_for(&t.workspace, t.scope, kind, name)
        }
        let pooled: BTreeMap<String, VendorOptions> = ClientTarget::ALL
            .iter()
            .map(|c| {
                (
                    c.as_str().to_string(),
                    VendorOptions {
                        shared_skills: Some(true),
                    },
                )
            })
            .collect();
        for scope in [ConfigScope::Project, ConfigScope::Global] {
            for vendors in [BTreeMap::new(), pooled.clone()] {
                let names: Vec<String> = ClientTarget::ALL.iter().map(|c| c.as_str().to_string()).collect();
                let t = InstallTarget::parse(Path::new("/w"), scope, &names, &[], &vendors)
                    .unwrap()
                    .with_grim_home(Path::new("/grim"));
                for client in ClientTarget::ALL {
                    for kind in [
                        ArtifactKind::Skill,
                        ArtifactKind::Rule,
                        ArtifactKind::Agent,
                        ArtifactKind::Mcp,
                    ] {
                        assert_eq!(
                            t.path_for(client, kind, "x"),
                            mains_path_for(&t, client, kind, "x"),
                            "{client} {kind:?} {scope:?}"
                        );
                    }
                }
            }
        }
    }

    /// C-106 (b): a hook never reaches `ClientTarget::path_for` (whose `Hook`
    /// arm is `unreachable!`) — for every client, at both scopes.
    #[test]
    fn install_target_path_for_never_delegates_a_hook() {
        for scope in [ConfigScope::Project, ConfigScope::Global] {
            let t = InstallTarget::new(Path::new("/w"), scope, ClientTarget::ALL.to_vec())
                .with_grim_home(Path::new("/grim"));
            for client in ClientTarget::ALL {
                let dest = t.path_for(client, ArtifactKind::Hook, "shell-guard");
                assert!(
                    dest.starts_with("/grim/hooks"),
                    "{client} {scope:?}: {}",
                    dest.display()
                );
            }
        }
    }

    /// C-106 (c): the target's `$GRIM_HOME` is the one `Context` — and so
    /// `AnchorRoots` — resolves, because both read `env::grim_home`.
    #[test]
    fn a_target_resolves_grim_home_like_the_context_does() {
        let tmp = tempfile::tempdir().unwrap();
        // `Context::new` sets its `grim_home` from `env::grim_home()`, and
        // `AnchorRoots::resolve` copies `ctx.grim_home()`.
        let resolved = crate::env::grim_home();
        let t = InstallTarget::new(tmp.path(), ConfigScope::Project, vec![ClientTarget::Claude]);
        assert_eq!(t.grim_home(), resolved);
        let parsed = InstallTarget::parse(tmp.path(), ConfigScope::Project, &[], &[], &BTreeMap::new()).unwrap();
        assert_eq!(parsed.grim_home(), resolved);
    }

    /// ⛔ **SEC-1.** A hook payload never lands in the workspace, at either
    /// scope, and never depends on which client armed it.
    #[test]
    fn a_hook_payload_is_machine_local_at_both_scopes() {
        let project = InstallTarget::new(Path::new("/w"), ConfigScope::Project, vec![ClientTarget::Claude])
            .with_grim_home(Path::new("/grim"));
        let dest = project.path_for(ClientTarget::Claude, ArtifactKind::Hook, "shell-guard");
        assert!(dest.starts_with("/grim"), "{}", dest.display());
        assert!(
            !dest.starts_with("/w"),
            "a repo-resident payload is I1 / SEC-1 — got {}",
            dest.display()
        );
        assert_eq!(
            dest,
            PathBuf::from("/grim").join(crate::install::hook_dispatch::payload_relative(
                crate::install::hook_dispatch::RootScope::Workspace(Path::new("/w")),
                "shell-guard",
            ))
        );

        // Two workspaces, one `$GRIM_HOME`, two directories.
        let other = InstallTarget::new(Path::new("/other"), ConfigScope::Project, vec![ClientTarget::Claude])
            .with_grim_home(Path::new("/grim"));
        assert_ne!(
            dest,
            other.path_for(ClientTarget::Claude, ArtifactKind::Hook, "shell-guard")
        );

        // Client-independent (S-003).
        assert_eq!(
            dest,
            project.path_for(ClientTarget::Codex, ArtifactKind::Hook, "shell-guard")
        );

        // Global scope keeps the flat `$GRIM_HOME/hooks/<name>` layout.
        let global = InstallTarget::new(Path::new("/w"), ConfigScope::Global, vec![ClientTarget::Claude])
            .with_grim_home(Path::new("/grim"));
        assert_eq!(
            global.path_for(ClientTarget::Claude, ArtifactKind::Hook, "shell-guard"),
            PathBuf::from("/grim/hooks/shell-guard")
        );
    }

    #[test]
    fn path_for_delegates_to_client() {
        let t = InstallTarget::new(Path::new("/w"), ConfigScope::Project, vec![ClientTarget::Copilot]);
        assert_eq!(
            t.path_for(ClientTarget::Copilot, ArtifactKind::Rule, "rust-style"),
            PathBuf::from("/w/.github/instructions/rust-style.instructions.md")
        );
    }

    // ── `[options.vendors.<name>].shared_skills` ────────────────────────────

    /// A `[options.vendors]` table opting `names` into the shared pool.
    fn pooled(names: &[&str]) -> BTreeMap<String, VendorOptions> {
        names
            .iter()
            .map(|n| {
                (
                    (*n).to_string(),
                    VendorOptions {
                        shared_skills: Some(true),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn shared_skills_moves_only_that_clients_skills_into_the_pool() {
        // Cursor opted in: its SKILLS go to the cross-vendor pool, everything
        // else — its own other kinds, and every other client — is untouched.
        let t = InstallTarget::parse(
            Path::new("/w"),
            ConfigScope::Project,
            &["cursor,copilot".to_string()],
            &[],
            &pooled(&["cursor"]),
        )
        .unwrap();
        assert_eq!(
            t.path_for(ClientTarget::Cursor, ArtifactKind::Skill, "x"),
            PathBuf::from("/w/.agents/skills/x"),
            "an opted-in client's skills must render into the shared pool"
        );
        assert_eq!(
            t.path_for(ClientTarget::Cursor, ArtifactKind::Rule, "x"),
            PathBuf::from("/w/.cursor/rules/x.mdc"),
            "the opt-in governs skills ONLY — other kinds keep the native layout"
        );
        assert_eq!(
            t.path_for(ClientTarget::Copilot, ArtifactKind::Skill, "x"),
            PathBuf::from("/w/.github/skills/x"),
            "a client that did not opt in keeps its native skills dir"
        );
    }

    #[test]
    fn absent_or_false_shared_skills_keeps_the_native_layout() {
        // The resting state, and the reverse flip: no entry and an explicit
        // `false` must both produce the native path, or turning the option
        // back off would never move the skill home.
        let cursor_skill = PathBuf::from("/w/.cursor/skills/x");
        for vendors in [
            BTreeMap::new(),
            [(
                "cursor".to_string(),
                VendorOptions {
                    shared_skills: Some(false),
                },
            )]
            .into_iter()
            .collect(),
        ] {
            let t = InstallTarget::parse(
                Path::new("/w"),
                ConfigScope::Project,
                &["cursor".to_string()],
                &[],
                &vendors,
            )
            .unwrap();
            assert_eq!(t.path_for(ClientTarget::Cursor, ArtifactKind::Skill, "x"), cursor_skill);
        }
    }

    #[test]
    fn shared_skills_is_ignored_for_a_client_that_cannot_pool() {
        // Defence in depth: the config surface already refuses this (exit 65
        // at `config set`, 78 at load), so this only fires for a config this
        // process did not validate. Writing into a pool Claude never reads —
        // and, at global scope, into an anchor its candidate set does not even
        // contain — is strictly worse than ignoring the entry.
        let t = InstallTarget::parse(
            Path::new("/w"),
            ConfigScope::Project,
            &["claude".to_string()],
            &[],
            &pooled(&["claude", "nonsense-client"]),
        )
        .unwrap();
        assert_eq!(
            t.path_for(ClientTarget::Claude, ArtifactKind::Skill, "x"),
            PathBuf::from("/w/.claude/skills/x"),
            "a non-pool-capable client must keep its native layout regardless"
        );
    }

    #[test]
    fn opting_in_an_already_pooled_client_is_a_genuine_no_op() {
        // `codex`/`gemini`/`zed`/`amp`/`agents` are on the capability roster
        // and already render into the pool, so the key is accepted for them
        // and changes nothing. That holds because the opt-in borrows the
        // generic client's skills root — the same helper they resolve through
        // — rather than restating the path.
        for client in [
            ClientTarget::Codex,
            ClientTarget::Gemini,
            ClientTarget::Zed,
            ClientTarget::Amp,
            ClientTarget::Agents,
        ] {
            let name = client.as_str();
            let native = InstallTarget::parse(
                Path::new("/w"),
                ConfigScope::Project,
                &[name.to_string()],
                &[],
                &BTreeMap::new(),
            )
            .unwrap();
            let opted = InstallTarget::parse(
                Path::new("/w"),
                ConfigScope::Project,
                &[name.to_string()],
                &[],
                &pooled(&[name]),
            )
            .unwrap();
            assert_eq!(
                opted.path_for(client, ArtifactKind::Skill, "x"),
                native.path_for(client, ArtifactKind::Skill, "x"),
                "{name} already pools — opting it in must not move anything"
            );
        }
    }

    // ── parse_client_list (C-015) ──────────────────────────────────

    use crate::install::install_error::InstallErrorKind;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn c015_parse_client_list_splits_trims_drops_empties_and_dedupes_in_order() {
        let got = parse_client_list(&strings(&[" copilot , ,claude", "claude,copilot", "", "codex"])).unwrap();
        assert_eq!(
            got,
            vec![ClientTarget::Copilot, ClientTarget::Claude, ClientTarget::Codex]
        );
    }

    #[test]
    fn c015_parse_client_list_returns_empty_as_is_without_fallback() {
        // Detection and the generic fallback belong to `InstallTarget::parse`,
        // never to the list parser — even on a workspace with `.claude/`.
        assert!(parse_client_list(&[]).unwrap().is_empty());
        assert!(parse_client_list(&strings(&["", " , ,"])).unwrap().is_empty());
    }

    #[test]
    fn c015_parse_client_list_unknown_name_is_unsupported_client_78() {
        let err = parse_client_list(&strings(&["claude, nope ,copilot"])).expect_err("unknown client");
        assert!(
            matches!(&err.kind, InstallErrorKind::UnsupportedClient(n) if n == "nope"),
            "the trimmed unknown name is reported: {err:?}"
        );
        assert_eq!(
            crate::error::classify_error(&anyhow::Error::from(crate::error::Error::from(err))),
            crate::cli::exit_code::ExitCode::ConfigError
        );
        // Names are case-sensitive, as `ClientTarget::from_str` always was.
        assert!(parse_client_list(&strings(&["Claude"])).is_err());
    }

    #[test]
    fn c015_c031_install_target_parse_agrees_with_parse_client_list() {
        // C-031.5: `InstallTarget::parse` calls the extracted parser, so for
        // any non-empty selection the two answer identically, flag or config.
        let tmp = tempfile::tempdir().unwrap();
        for input in [
            strings(&["claude"]),
            strings(&["claude,copilot"]),
            strings(&[" codex ,, codex", "claude"]),
            strings(&["agents", "agents,claude"]),
        ] {
            let listed = parse_client_list(&input).unwrap();
            let via_flag =
                InstallTarget::parse(tmp.path(), ConfigScope::Project, &input, &[], &BTreeMap::new()).unwrap();
            let via_cfg =
                InstallTarget::parse(tmp.path(), ConfigScope::Project, &[], &input, &BTreeMap::new()).unwrap();
            assert_eq!(via_flag.clients(), listed.as_slice(), "flag {input:?}");
            assert_eq!(via_cfg.clients(), listed.as_slice(), "config {input:?}");
            assert!(!via_flag.is_generic_fallback());
        }
        // Unknown names still refuse through `parse`, with the same kind.
        let err = InstallTarget::parse(
            tmp.path(),
            ConfigScope::Project,
            &strings(&["claude,nope"]),
            &[],
            &BTreeMap::new(),
        )
        .expect_err("unknown client");
        assert!(matches!(&err.kind, InstallErrorKind::UnsupportedClient(n) if n == "nope"));
        // An all-empty flag list still falls through to config, then detection.
        let t = InstallTarget::parse(
            tmp.path(),
            ConfigScope::Project,
            &[],
            &strings(&[" , "]),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(t.clients(), &[ClientTarget::Agents]);
        assert!(t.is_generic_fallback());
        // A present-but-blank flag still shadows the config default (the flag
        // wins on presence, not on content) — pre-extraction behaviour.
        let t = InstallTarget::parse(
            tmp.path(),
            ConfigScope::Project,
            &strings(&[","]),
            &strings(&["claude"]),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(t.clients(), &[ClientTarget::Agents]);
    }

    /// The configured set is the config list alone: `--client` narrows one
    /// run, and an empty list is autodetect, not "no clients".
    #[test]
    fn configured_clients_come_from_the_config_list_never_the_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let parse = |flag: &[&str], config: &[&str]| {
            InstallTarget::parse(
                tmp.path(),
                ConfigScope::Project,
                &strings(flag),
                &strings(config),
                &BTreeMap::new(),
            )
            .unwrap()
        };
        let (copilot, claude) = (ClientTarget::Copilot, ClientTarget::Claude);
        assert_eq!(parse(&["copilot"], &[]).configured_clients(), None, "flag only");
        assert_eq!(parse(&[], &[]).configured_clients(), None, "autodetect");
        assert_eq!(parse(&[], &[" , "]).configured_clients(), None, "a blank list");
        assert_eq!(
            parse(&[], &["copilot,claude"]).configured_clients(),
            Some(&[copilot, claude][..])
        );
        assert_eq!(
            parse(&["cursor"], &["copilot"]).configured_clients(),
            Some(&[copilot][..]),
            "the flag selects this run's clients, not the configured set"
        );
        assert_eq!(
            parse(&["cursor"], &["nope"]).configured_clients(),
            None,
            "an unparseable config list under a flag configures nothing"
        );
        let built = InstallTarget::new(tmp.path(), ConfigScope::Project, vec![claude]);
        assert_eq!(built.configured_clients(), None);
    }

    // ── pool auto-join (issue #262) ──

    fn target_with(scope: ConfigScope, clients: &[ClientTarget], vendors: &[(&str, bool)]) -> InstallTarget {
        let names: Vec<String> = clients.iter().map(|c| c.as_str().to_string()).collect();
        let vendors: BTreeMap<String, VendorOptions> = vendors
            .iter()
            .map(|(n, v)| {
                (
                    (*n).to_string(),
                    VendorOptions {
                        shared_skills: Some(*v),
                    },
                )
            })
            .collect();
        InstallTarget::parse(Path::new("/w"), scope, &names, &[], &vendors).unwrap()
    }

    /// Readers that render natively at `scope`, i.e. the ones that can join.
    fn native_readers(scope: ConfigScope) -> Vec<ClientTarget> {
        let t = InstallTarget::new(Path::new("/w"), scope, vec![]);
        ClientTarget::ALL
            .into_iter()
            .filter(|c| c.vendor().pool_capable() && !t.lands_in_pool(*c, "x"))
            .collect()
    }

    #[test]
    fn every_native_pool_reader_joins_beside_a_pool_writer_at_both_scopes() {
        for scope in [ConfigScope::Project, ConfigScope::Global] {
            let readers = native_readers(scope);
            assert!(readers.contains(&ClientTarget::OpenCode), "{scope:?}: {readers:?}");
            for reader in readers {
                let peers = [reader, ClientTarget::Codex];
                let t = target_with(scope, &peers, &[]);
                // An environment-gated reader (a disable flag set in this
                // process) stays native; every other one joins.
                assert_eq!(
                    t.joins_pool(reader, "x", &peers),
                    reader.vendor().reads_pool(scope),
                    "{reader} {scope:?}"
                );
            }
        }
    }

    #[test]
    fn readers_alone_never_pull_each_other_into_the_pool() {
        for scope in [ConfigScope::Project, ConfigScope::Global] {
            let peers = [ClientTarget::OpenCode, ClientTarget::Cursor];
            let t = target_with(scope, &peers, &[]);
            assert!(!t.joins_pool(ClientTarget::OpenCode, "x", &peers), "{scope:?}");
            assert!(!t.joins_pool(ClientTarget::Cursor, "x", &peers), "{scope:?}");
        }
    }

    #[test]
    fn an_explicit_false_keeps_the_reader_native_beside_a_pool_writer() {
        let peers = [ClientTarget::OpenCode, ClientTarget::Codex];
        let t = target_with(ConfigScope::Global, &peers, &[("opencode", false)]);
        assert!(!t.joins_pool(ClientTarget::OpenCode, "x", &peers));
    }

    #[test]
    fn a_reader_set_to_true_counts_as_a_pool_writer_for_its_peers() {
        let peers = [ClientTarget::OpenCode, ClientTarget::Cursor];
        let t = target_with(ConfigScope::Project, &peers, &[("cursor", true)]);
        assert!(t.joins_pool(ClientTarget::OpenCode, "x", &peers));
        assert!(!t.joins_pool(ClientTarget::Cursor, "x", &peers), "already in the pool");
    }

    #[test]
    fn an_antigravity_peer_counts_as_a_pool_writer_at_project_scope_only() {
        let peers = [ClientTarget::OpenCode, ClientTarget::Antigravity];
        let project = target_with(ConfigScope::Project, &peers, &[]);
        assert!(project.joins_pool(ClientTarget::OpenCode, "x", &peers));
        let global = target_with(ConfigScope::Global, &peers, &[]);
        assert!(!global.joins_pool(ClientTarget::OpenCode, "x", &peers));
    }

    #[test]
    fn a_reader_is_not_its_own_peer() {
        let peers = [ClientTarget::OpenCode];
        let t = target_with(ConfigScope::Project, &peers, &[]);
        assert!(!t.joins_pool(ClientTarget::OpenCode, "x", &peers));
    }

    #[test]
    fn a_non_pool_capable_client_never_joins() {
        let peers = [ClientTarget::Claude, ClientTarget::Codex];
        let t = target_with(ConfigScope::Project, &peers, &[]);
        assert!(!t.joins_pool(ClientTarget::Claude, "x", &peers));
    }

    // ── global client list at project scope ──

    use ClientSource::{Detected, Fallback, Flag, Global, Project};

    /// A workspace whose detected set is exactly `markers` (project scope is
    /// hermetic: it never reads the developer's home directory).
    fn workspace_with(markers: &[&str]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        for marker in markers {
            std::fs::create_dir_all(tmp.path().join(marker)).unwrap();
        }
        tmp
    }

    fn parse_global(ws: &Path, flag: &[&str], project: &[&str], global: &[ClientTarget]) -> InstallTarget {
        InstallTarget::parse_with_global(
            ws,
            ConfigScope::Project,
            &strings(flag),
            &strings(project),
            global,
            &BTreeMap::new(),
        )
        .unwrap()
    }

    fn sourced(t: &InstallTarget) -> Vec<(ClientTarget, ClientSource)> {
        t.client_sources().collect()
    }

    #[test]
    fn global_row_flag_wins_over_everything() {
        let ws = workspace_with(&[".opencode"]);
        let t = parse_global(ws.path(), &["copilot"], &["codex"], &[ClientTarget::Claude]);
        assert_eq!(sourced(&t), vec![(ClientTarget::Copilot, Flag)]);
        assert!(!t.is_generic_fallback());
    }

    #[test]
    fn global_row_project_list_wins_and_ignores_global_and_detected() {
        let ws = workspace_with(&[".opencode"]);
        let t = parse_global(ws.path(), &[], &["codex"], &[ClientTarget::Claude]);
        assert_eq!(sourced(&t), vec![(ClientTarget::Codex, Project)]);
    }

    #[test]
    fn global_row_detected_with_an_empty_global_list_is_detected_alone() {
        let ws = workspace_with(&[".opencode", ".github/instructions"]);
        let t = parse_global(ws.path(), &[], &[], &[]);
        assert_eq!(
            sourced(&t),
            vec![(ClientTarget::OpenCode, Detected), (ClientTarget::Copilot, Detected)]
        );
    }

    #[test]
    fn global_row_detected_plus_global_is_the_union_in_all_order() {
        // Global spelled out of order, with one entry also detected: the result
        // follows `ClientTarget::ALL`, and a client both detected and global
        // reads `detected`.
        let ws = workspace_with(&[".opencode"]);
        let global = [ClientTarget::Codex, ClientTarget::OpenCode, ClientTarget::Claude];
        let t = parse_global(ws.path(), &[], &[], &global);
        assert_eq!(
            sourced(&t),
            vec![
                (ClientTarget::Claude, Global),
                (ClientTarget::OpenCode, Detected),
                (ClientTarget::Codex, Global),
            ]
        );
        assert!(!t.is_generic_fallback());
    }

    #[test]
    fn global_row_nothing_detected_selects_the_global_list_alone() {
        let ws = workspace_with(&[]);
        let t = parse_global(ws.path(), &[], &[], &[ClientTarget::Claude, ClientTarget::Cursor]);
        assert_eq!(
            sourced(&t),
            vec![(ClientTarget::Claude, Global), (ClientTarget::Cursor, Global)]
        );
        assert!(!t.clients().contains(&ClientTarget::Agents), "no pool client");
        assert!(!t.is_generic_fallback(), "a global list is a selection, not a fallback");
    }

    #[test]
    fn global_row_nothing_detected_and_no_global_list_is_the_agents_fallback() {
        let ws = workspace_with(&[]);
        let t = parse_global(ws.path(), &[], &[], &[]);
        assert_eq!(sourced(&t), vec![(ClientTarget::Agents, Fallback)]);
        assert!(t.is_generic_fallback());
    }

    #[test]
    fn a_global_client_without_a_marker_is_selected_and_writes_its_own_layout() {
        let ws = workspace_with(&[".opencode"]);
        let t = parse_global(ws.path(), &[], &[], &[ClientTarget::Claude]);
        assert!(t.clients().contains(&ClientTarget::Claude));
        assert_eq!(
            t.path_for(ClientTarget::Claude, ArtifactKind::Skill, "x"),
            ws.path().join(".claude").join("skills").join("x")
        );
    }

    #[test]
    fn a_global_run_does_not_take_the_union() {
        // Global scope detection reads the real home, so assert only what
        // must hold wherever it lands: the list never contributes a client.
        let ws = workspace_with(&[]);
        let t = InstallTarget::parse_with_global(
            ws.path(),
            ConfigScope::Global,
            &[],
            &[],
            &[ClientTarget::Kiro, ClientTarget::Junie],
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(
            t.client_sources().all(|(_, source)| source != Global),
            "{:?}",
            sourced(&t)
        );
    }

    #[test]
    fn configured_clients_stay_the_project_list_under_the_union() {
        let ws = workspace_with(&[".opencode"]);
        let global = [ClientTarget::Claude];
        assert_eq!(
            parse_global(ws.path(), &[], &[], &global).configured_clients(),
            None,
            "a global list is not a configured list: no reap, no drift"
        );
        assert_eq!(
            parse_global(ws.path(), &["cursor"], &[], &global).configured_clients(),
            None
        );
        assert_eq!(
            parse_global(ws.path(), &[], &["codex"], &global).configured_clients(),
            Some(&[ClientTarget::Codex][..])
        );
        assert_eq!(
            parse_global(ws.path(), &["cursor"], &["codex"], &global).configured_clients(),
            Some(&[ClientTarget::Codex][..])
        );
    }

    #[test]
    fn an_empty_global_list_behaves_as_parse_does_in_every_row() {
        let bare = workspace_with(&[]);
        let detected = workspace_with(&[".opencode", ".claude"]);
        for ws in [&bare, &detected] {
            for (flag, project) in [
                (vec![], vec![]),
                (vec!["copilot"], vec![]),
                (vec![], vec!["codex"]),
                (vec!["cursor"], vec!["codex"]),
                (vec![","], vec!["claude"]),
                (vec![], vec![" , "]),
            ] {
                let old = InstallTarget::parse(
                    ws.path(),
                    ConfigScope::Project,
                    &strings(&flag),
                    &strings(&project),
                    &BTreeMap::new(),
                )
                .unwrap();
                let new = parse_global(ws.path(), &flag, &project, &[]);
                let label = format!("{flag:?} {project:?} {}", ws.path().display());
                assert_eq!(old.clients(), new.clients(), "{label}");
                assert_eq!(old.configured_clients(), new.configured_clients(), "{label}");
                assert_eq!(old.is_generic_fallback(), new.is_generic_fallback(), "{label}");
                assert_eq!(sourced(&old), sourced(&new), "{label}");
            }
        }
    }

    #[test]
    fn the_source_of_each_client_reads_as_its_flag_spelling() {
        let spelled: Vec<String> = [Flag, Project, Detected, Global, Fallback]
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(spelled, ["--client", "project", "detected", "global", "fallback"]);
    }

    #[test]
    fn the_strict_parser_raises_on_an_unknown_name() {
        let err = parse_client_list(&strings(&["claude, nope"])).expect_err("unknown client");
        assert!(matches!(&err.kind, InstallErrorKind::UnsupportedClient(n) if n == "nope"));
    }

    #[test]
    fn names_no_client_reads_blank_entries_as_no_list() {
        assert!(names_no_client(&[]));
        assert!(names_no_client(&strings(&["", " , "])));
        assert!(!names_no_client(&strings(&["claude"])));
        assert!(!names_no_client(&strings(&["", "nope"])));
    }

    #[test]
    fn a_global_run_labels_its_own_list_global() {
        let ws = workspace_with(&[]);
        let t = InstallTarget::parse_with_global(
            ws.path(),
            ConfigScope::Global,
            &[],
            &strings(&["claude"]),
            &[],
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(sourced(&t), vec![(ClientTarget::Claude, Global)]);
        assert_eq!(clients_note(&t), "clients: claude (global)");
        // `--client` still reads as the flag, and a project run keeps `project`.
        let flagged = InstallTarget::parse_with_global(
            ws.path(),
            ConfigScope::Global,
            &strings(&["codex"]),
            &strings(&["claude"]),
            &[],
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(sourced(&flagged), vec![(ClientTarget::Codex, Flag)]);
        let project = parse_global(ws.path(), &[], &["claude"], &[]);
        assert_eq!(sourced(&project), vec![(ClientTarget::Claude, Project)]);
    }

    #[test]
    fn active_clients_add_the_global_list_to_what_detection_reports() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(ws.path().join(".opencode")).unwrap();
        let project = ConfigScope::Project;
        assert_eq!(active_clients(ws.path(), project, &[]), vec![ClientTarget::OpenCode]);
        assert_eq!(
            active_clients(ws.path(), project, &[ClientTarget::Claude]),
            vec![ClientTarget::Claude, ClientTarget::OpenCode]
        );
        // Nothing detected is the permissive all-clients answer, global list or not.
        let bare = tempfile::tempdir().unwrap();
        assert_eq!(
            active_clients(bare.path(), project, &[ClientTarget::Claude]),
            ClientTarget::ALL.to_vec()
        );
    }

    #[test]
    fn the_clients_note_names_each_client_with_its_source() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(ws.path().join(".opencode")).unwrap();
        let target = parse_global(ws.path(), &[], &[], &[ClientTarget::Claude]);
        assert_eq!(clients_note(&target), "clients: claude (global), opencode (detected)");
        let bare = tempfile::tempdir().unwrap();
        assert_eq!(
            clients_note(&parse_global(bare.path(), &[], &[], &[])),
            "clients: agents (fallback)"
        );
    }
}
