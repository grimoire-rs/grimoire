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
//! [`detect_clients`]). Detection finding nothing falls back to the single
//! synthetic generic client [`ClientTarget::Agents`], which writes one copy
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

/// One or more AI client targets rooted at a workspace.
#[derive(Debug, Clone)]
pub struct InstallTarget {
    workspace: PathBuf,
    scope: ConfigScope,
    clients: Vec<ClientTarget>,
    /// Set only by [`InstallTarget::parse`] when flag, config, and detection
    /// were all empty and the generic client was substituted. Gates the
    /// residual "nothing here is installable" error.
    generic_fallback: bool,
    /// Clients whose skills render into the shared `.agents/skills` pool
    /// instead of their native skills directory
    /// (`[options.vendors.<name>].shared_skills = true`).
    ///
    /// Derived in [`Self::parse`] from the resolved scope's `[options.vendors]`
    /// table — never merged across scopes, never overridable by a flag.
    shared_skills: Vec<ClientTarget>,
    /// The resolved hook arming policy, or `None` when this target was not
    /// built by a mutating command. Attached by [`Self::with_hook_policy`] at
    /// the mutating boundary, never by [`Self::parse`] (shared with the
    /// read-only `status`/`search`/`context`), so a read-only command can
    /// neither arm nor prompt.
    hook_policy: Option<crate::hook::policy::HookPolicy>,
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
        let clients = if clients.is_empty() {
            detect_clients_or_all(workspace, scope)
        } else {
            clients
        };
        Self {
            workspace: workspace.to_path_buf(),
            scope,
            clients,
            generic_fallback: false,
            shared_skills: Vec::new(),
            hook_policy: None,
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
        let source: &[String] = if flag_values.is_empty() {
            config_default
        } else {
            flag_values
        };
        // Both flag and config empty ⇒ reach `new` with an empty list so
        // detection runs (do not inject the literal "claude").
        let mut clients = parse_client_list(source)?;

        // An unknown client name, or one the capability gate would have
        // refused, is dropped rather than raised: both are already refused at
        // the config surface (exit 64 / 65 at `config set`, 78 at load), so a
        // survivor here can only come from a config this process did not
        // validate. Silently ignoring it keeps the resting (native) layout —
        // rendering into a pool the client never reads is the failure this
        // whole feature exists to prevent.
        let shared_skills: Vec<ClientTarget> = vendors
            .iter()
            .filter(|(_, options)| options.shared_skills)
            .filter_map(|(name, _)| name.parse::<ClientTarget>().ok())
            .filter(|client| client.vendor().pool_capable())
            .collect();

        if clients.is_empty() {
            let detected = detect_clients(workspace, scope);
            if detected.is_empty() {
                return Ok(Self {
                    workspace: workspace.to_path_buf(),
                    scope,
                    clients: vec![ClientTarget::Agents],
                    generic_fallback: true,
                    shared_skills,
                    hook_policy: None,
                    grim_home: crate::env::grim_home(),
                });
            }
            clients = detected;
        }
        let mut target = Self::new(workspace, scope, clients);
        target.shared_skills = shared_skills;
        Ok(target)
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
        self.generic_fallback
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
            .map(|c| (c.as_str().to_string(), VendorOptions { shared_skills: true }))
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
            .map(|n| ((*n).to_string(), VendorOptions { shared_skills: true }))
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
            [("cursor".to_string(), VendorOptions { shared_skills: false })]
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
}
