// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Recording what a command did: clients and changed artifacts.

use std::path::Path;

use crate::config::declaration::DesiredSet;
use crate::config::scope::ConfigScope;
use crate::install::client_target::ClientTarget;
use crate::install::target::{InstallTarget, detect_clients};
use crate::oci::Identifier;
use crate::oci::artifact_kind::ArtifactKind;
use crate::telemetry::Telemetry;

/// What happened to an artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactAction {
    Installed,
    Updated,
    Removed,
    Added,
}

impl ArtifactAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::Updated => "updated",
            Self::Removed => "removed",
            Self::Added => "added",
        }
    }
}

/// One changed artifact. `reference` is `registry/repository[:tag]`, `None`
/// for a path source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactEvent {
    pub kind: ArtifactKind,
    pub action: ArtifactAction,
    pub reference: Option<String>,
}

impl Telemetry {
    /// Records the selected and detected clients of an install-like command.
    /// Detection runs only when the record is kept: an unarmed or long-lived
    /// handle never touches the file system.
    pub fn record_clients_with(&self, selected: &[ClientTarget], detect: impl FnOnce() -> Vec<ClientTarget>) {
        self.record(|state| {
            state.clients = Some(selected.to_vec());
            state.detected = Some(detect());
        });
    }

    /// [`Self::record_clients_with`] for the three install-like commands:
    /// the target's clients, detection against the resolved scope.
    pub fn record_install_clients(&self, target: &InstallTarget, workspace: &Path, scope: ConfigScope) {
        self.record_clients_with(target.clients(), || detect_clients(workspace, scope));
    }

    /// Records the artifacts `build` yields, in order, building them only when
    /// the record is kept.
    pub fn record_artifacts_with(&self, build: impl FnOnce() -> Vec<ArtifactEvent>) {
        self.record(|state| state.artifacts.extend(build()));
    }
}

/// `registry/repository[:tag]` for `source`.
///
/// Built from the identifier's accessors, so a digest or anything else a raw
/// string could carry never reaches the wire. The tag is the one a `declared`
/// identifier for the same repository carries; a pin's own advisory tag is
/// not trusted, and a bundle member or digest-only declaration has none.
pub fn reference<'a>(source: &Identifier, declared: impl IntoIterator<Item = &'a Identifier>) -> String {
    let tag = declared
        .into_iter()
        .filter(|id| id.registry() == source.registry() && id.repository() == source.repository())
        .find_map(Identifier::tag);
    let location = source.registry_repository();
    match tag {
        Some(tag) => format!("{location}:{tag}"),
        None => location,
    }
}

/// Every registry identifier the scope declares, for [`reference`]'s tag lookup.
pub fn declared_identifiers(set: &DesiredSet) -> impl Iterator<Item = &Identifier> {
    [&set.skills, &set.rules, &set.agents, &set.bundles, &set.mcp, &set.hooks]
        .into_iter()
        .flat_map(|section| section.values())
        .filter_map(|source| source.identifier())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::parsed_command;
    use std::path::PathBuf;

    fn record_one(t: &Telemetry, kind: ArtifactKind, action: ArtifactAction, reference: Option<String>) {
        t.record_artifacts_with(|| {
            vec![ArtifactEvent {
                kind,
                action,
                reference,
            }]
        });
    }

    fn armed() -> Telemetry {
        Telemetry::armed_for_test(PathBuf::from("/grim-home"))
    }

    #[test]
    fn action_names_are_the_documented_literals() {
        assert_eq!(ArtifactAction::Installed.as_str(), "installed");
        assert_eq!(ArtifactAction::Updated.as_str(), "updated");
        assert_eq!(ArtifactAction::Removed.as_str(), "removed");
        assert_eq!(ArtifactAction::Added.as_str(), "added");
    }

    #[test]
    fn clients_land_in_the_state_for_payload_build() {
        let t = armed();
        t.record_clients_with(&[ClientTarget::Claude], || {
            vec![ClientTarget::Claude, ClientTarget::Codex]
        });
        let s = t.snapshot().unwrap();
        assert_eq!(s.clients, Some(vec![ClientTarget::Claude]));
        assert_eq!(s.detected, Some(vec![ClientTarget::Claude, ClientTarget::Codex]));
    }

    #[test]
    fn artifacts_keep_emit_order_and_a_missing_reference() {
        let t = armed();
        record_one(
            &t,
            ArtifactKind::Skill,
            ArtifactAction::Installed,
            Some("r.io/a/b:1".into()),
        );
        record_one(&t, ArtifactKind::Rule, ArtifactAction::Added, None);
        let s = t.snapshot().unwrap();
        assert_eq!(
            s.artifacts,
            vec![
                ArtifactEvent {
                    kind: ArtifactKind::Skill,
                    action: ArtifactAction::Installed,
                    reference: Some("r.io/a/b:1".into()),
                },
                ArtifactEvent {
                    kind: ArtifactKind::Rule,
                    action: ArtifactAction::Added,
                    reference: None,
                },
            ]
        );
    }

    #[test]
    fn records_reach_the_payload_events() {
        let t = armed();
        t.start(&parsed_command(&["install"]));
        t.record_clients_with(&[ClientTarget::Claude], || vec![ClientTarget::Claude]);
        record_one(
            &t,
            ArtifactKind::Skill,
            ArtifactAction::Updated,
            Some("r.io/a/b".into()),
        );
        let s = t.snapshot().unwrap();
        let outcome = crate::telemetry::payload::Outcome {
            duration: std::time::Duration::ZERO,
            exit_code: 0,
            error_type: None,
            ci: false,
        };
        let line = crate::telemetry::payload::build(Vec::new(), &s, &outcome).to_json_line();
        assert!(line.contains(r#""grim.clients":["claude"]"#), "{line}");
        assert!(line.contains(r#""grim.artifact.action":"updated""#), "{line}");
        assert!(line.contains(r#""grim.artifact.ref":"r.io/a/b""#), "{line}");
    }

    #[test]
    fn a_long_lived_session_records_nothing() {
        for name in ["mcp", "tui"] {
            let t = armed();
            t.start(&parsed_command(&[name]));
            t.record_clients_with(&[ClientTarget::Claude], || vec![ClientTarget::Claude]);
            record_one(
                &t,
                ArtifactKind::Skill,
                ArtifactAction::Installed,
                Some("r.io/a/b".into()),
            );
            let detected = std::cell::Cell::new(false);
            t.record_clients_with(&[ClientTarget::Claude], || {
                detected.set(true);
                Vec::new()
            });
            t.record_artifacts_with(|| {
                detected.set(true);
                Vec::new()
            });
            let s = t.snapshot().unwrap();
            assert!(
                s.clients.is_none() && s.detected.is_none() && s.artifacts.is_empty(),
                "{name}"
            );
            assert!(!detected.get(), "{name}: lazy builders must not run");
        }
    }

    #[test]
    fn an_unarmed_handle_never_runs_the_lazy_builders() {
        let t = Telemetry::default();
        t.record_clients_with(&[ClientTarget::Claude], || panic!("detection ran while unarmed"));
        t.record_artifacts_with(|| panic!("event building ran while unarmed"));
        record_one(&t, ArtifactKind::Skill, ArtifactAction::Installed, None);
        assert!(t.snapshot().is_none());
    }

    #[test]
    fn records_after_finish_are_dropped() {
        let t = armed();
        t.record(|s| s.finished = true);
        record_one(&t, ArtifactKind::Skill, ArtifactAction::Installed, None);
        assert!(t.snapshot().unwrap().artifacts.is_empty());
    }

    fn id(s: &str) -> Identifier {
        Identifier::parse(s).unwrap()
    }

    #[test]
    fn reference_is_registry_repository_with_the_declared_tag_only() {
        let pinned = id(&format!("r.io/acme/review:9@sha256:{}", "a".repeat(64)));
        let declared = [id("r.io/acme/review:1"), id("r.io/acme/other:2")];
        assert_eq!(reference(&pinned, declared.iter()).as_str(), "r.io/acme/review:1");
    }

    #[test]
    fn reference_omits_the_tag_when_nothing_declared_carries_one() {
        let pinned = id(&format!("r.io/acme/review@sha256:{}", "a".repeat(64)));
        let declared = [id("r.io/acme/other:2"), id("r.io/elsewhere/review:3")];
        assert_eq!(reference(&pinned, declared.iter()), "r.io/acme/review");
        let digest_only = id(&format!("r.io/acme/review@sha256:{}", "a".repeat(64)));
        assert_eq!(reference(&pinned, [digest_only].iter()).as_str(), "r.io/acme/review");
    }

    #[test]
    fn reference_never_carries_a_digest() {
        let pinned = id(&format!("r.io/acme/review:1@sha256:{}", "a".repeat(64)));
        let r = reference(&pinned, [pinned.clone()].iter());
        assert_eq!(r, "r.io/acme/review:1");
        assert!(!r.contains("sha256") && !r.contains('@'));
    }
}
