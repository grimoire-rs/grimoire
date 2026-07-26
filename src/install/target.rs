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

use std::path::{Path, PathBuf};

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
    /// # Errors
    ///
    /// [`super::install_error::InstallErrorKind::UnsupportedClient`] for an
    /// unknown client name.
    pub fn parse(
        workspace: &Path,
        scope: ConfigScope,
        flag_values: &[String],
        config_default: &[String],
    ) -> Result<Self, InstallError> {
        let source: &[String] = if flag_values.is_empty() {
            config_default
        } else {
            flag_values
        };
        // Both flag and config empty ⇒ reach `new` with an empty list so
        // detection runs (do not inject the literal "claude").
        let raw: Vec<String> = source
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

        if clients.is_empty() {
            let detected = detect_clients(workspace, scope);
            if detected.is_empty() {
                return Ok(Self {
                    workspace: workspace.to_path_buf(),
                    scope,
                    clients: vec![ClientTarget::Agents],
                    generic_fallback: true,
                });
            }
            clients = detected;
        }
        Ok(Self::new(workspace, scope, clients))
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
    pub fn path_for(&self, client: ClientTarget, kind: ArtifactKind, name: &str) -> PathBuf {
        client.path_for(&self.workspace, self.scope, kind, name)
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
        let p = InstallTarget::parse(tmp.path(), ConfigScope::Project, &[], &[]).unwrap();
        assert_eq!(p.clients(), &[ClientTarget::OpenCode, ClientTarget::Copilot]);
    }

    #[test]
    fn explicit_config_overrides_detection() {
        // Even with `.opencode` present, an explicit config `clients`
        // declaration wins over detection.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".opencode")).unwrap();
        let t = InstallTarget::parse(tmp.path(), ConfigScope::Project, &[], &["claude".to_string()]).unwrap();
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
        let t = InstallTarget::parse(tmp.path(), ConfigScope::Project, &[], &[]).unwrap();
        assert_eq!(t.clients(), &[ClientTarget::Agents]);
        assert!(t.is_generic_fallback());
    }

    #[test]
    fn the_fallback_does_not_manufacture_its_own_detection_signal() {
        // The regression test for the original defect: materializing into the
        // pool must not make the *next* run resolve differently. Two runs on a
        // bare workspace both resolve to `agents` alone.
        let tmp = tempfile::tempdir().unwrap();
        let first = InstallTarget::parse(tmp.path(), ConfigScope::Project, &[], &[]).unwrap();
        assert_eq!(first.clients(), &[ClientTarget::Agents]);

        // Simulate what the install writes.
        std::fs::create_dir_all(tmp.path().join(".agents").join("skills").join("demo")).unwrap();

        let second = InstallTarget::parse(tmp.path(), ConfigScope::Project, &[], &[]).unwrap();
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
            let t = InstallTarget::parse(tmp.path(), ConfigScope::Project, &flag, &cfg).unwrap();
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
        let t = InstallTarget::parse(tmp.path(), ConfigScope::Project, &[], &[]).unwrap();
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
        )
        .unwrap();
        assert_eq!(t.clients(), &[ClientTarget::Claude, ClientTarget::Copilot]);
        // Repeated flag values merge.
        let t2 = InstallTarget::parse(
            Path::new("/w"),
            ConfigScope::Project,
            &["copilot".to_string(), "copilot".to_string(), "claude".to_string()],
            &[],
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
        )
        .unwrap();
        assert_eq!(t.clients(), &[ClientTarget::OpenCode, ClientTarget::Claude]);
        // `/w` does not exist ⇒ nothing detected ⇒ the generic client.
        let t2 = InstallTarget::parse(Path::new("/w"), ConfigScope::Project, &[], &[]).unwrap();
        assert_eq!(t2.clients(), &[ClientTarget::Agents]);
        // A flag list overrides the config default entirely.
        let t3 = InstallTarget::parse(
            Path::new("/w"),
            ConfigScope::Project,
            &["copilot".to_string()],
            &["claude".to_string()],
        )
        .unwrap();
        assert_eq!(t3.clients(), &[ClientTarget::Copilot]);
    }

    #[test]
    fn parse_rejects_unknown_client() {
        assert!(InstallTarget::parse(Path::new("/w"), ConfigScope::Project, &["vscode".to_string()], &[]).is_err());
    }

    #[test]
    fn path_for_delegates_to_client() {
        let t = InstallTarget::new(Path::new("/w"), ConfigScope::Project, vec![ClientTarget::Copilot]);
        assert_eq!(
            t.path_for(ClientTarget::Copilot, ArtifactKind::Rule, "rust-style"),
            PathBuf::from("/w/.github/instructions/rust-style.instructions.md")
        );
    }
}
