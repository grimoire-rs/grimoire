// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The telemetry payload as plain data, and how it is assembled.
//!
//! No OpenTelemetry type appears here: `export` converts a [`Payload`] to
//! the wire form, and [`Payload::to_json_line`] renders the debug audit line
//! from the same value, so the two cannot disagree about what is sent. Every
//! attribute key lives in this file as a constant.

use std::time::{Duration, SystemTime};

use crate::cli::exit_code::ExitCode;
use crate::config::scope::ConfigScope;
use crate::install::client_target::ClientTarget;
use crate::telemetry::config::ResourceOverrides;
use crate::telemetry::record::ArtifactEvent;
use crate::telemetry::vcs::Vcs;
use crate::telemetry::{State, UNKNOWN_COMMAND};

pub const SERVICE_NAME: &str = "service.name";
pub const SERVICE_VERSION: &str = "service.version";
pub const OS_TYPE: &str = "os.type";
pub const HOST_ARCH: &str = "host.arch";
pub const INSTALLATION_ID: &str = "grim.installation.id";
pub const VCS_URL: &str = "vcs.repository.url.full";
pub const VCS_OWNER: &str = "vcs.owner.name";
pub const VCS_NAME: &str = "vcs.repository.name";
pub const VCS_PROVIDER: &str = "vcs.provider.name";

pub const EVENT_COMMAND: &str = "grim.command";
pub const EVENT_ARTIFACT: &str = "grim.artifact.changed";

pub const COMMAND_NAME: &str = "grim.command.name";
pub const COMMAND_SUBCOMMAND: &str = "grim.command.subcommand";
pub const COMMAND_DURATION: &str = "grim.command.duration";
pub const EXIT_CODE: &str = "process.exit.code";
pub const ERROR_TYPE: &str = "error.type";
pub const CI: &str = "grim.ci";
pub const SCOPE: &str = "grim.scope";
pub const CLIENTS: &str = "grim.clients";
pub const CLIENTS_DETECTED: &str = "grim.clients.detected";

pub const ARTIFACT_REF: &str = "grim.artifact.ref";
pub const ARTIFACT_KIND: &str = "grim.artifact.kind";
pub const ARTIFACT_ACTION: &str = "grim.artifact.action";

/// One attribute value. The wire form maps these one-to-one onto OTLP `AnyValue`.
#[derive(Debug, Clone, PartialEq)]
pub enum AttrValue {
    Str(String),
    Int(i64),
    Double(f64),
    Bool(bool),
    StrArray(Vec<String>),
}

/// One log-record event: a name plus ordered attributes.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub name: &'static str,
    pub attributes: Vec<(&'static str, AttrValue)>,
}

/// Everything one invocation sends: the resource and the events, in emit order.
#[derive(Debug, Clone, PartialEq)]
pub struct Payload {
    pub resource: Vec<(String, String)>,
    pub events: Vec<Event>,
    /// Stamped on every record at conversion time; never part of the JSON line.
    pub observed_at: SystemTime,
}

/// What `finish` knows once the command has returned.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub duration: Duration,
    pub exit_code: i64,
    pub error_type: Option<String>,
    pub ci: bool,
}

/// Semantic-convention `os.type` spelling of [`std::env::consts::OS`].
pub fn os_type(os: &str) -> &str {
    match os {
        "macos" => "darwin",
        "dragonfly" => "dragonflybsd",
        other => other,
    }
}

/// Semantic-convention `host.arch` spelling of [`std::env::consts::ARCH`].
pub fn host_arch(arch: &str) -> &str {
    match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "arm" => "arm32",
        "powerpc64" => "ppc64",
        other => other,
    }
}

/// Exit code and `error.type` for a finished command.
///
/// The exit comes from [`crate::error::outcome_exit`], the mapping `main.rs`
/// uses; `error.type` is the reason slug, else the exit slug, never on success.
pub fn exit_info(outcome: &anyhow::Result<ExitCode>) -> (i64, Option<String>) {
    let exit = crate::error::outcome_exit(outcome);
    let error_type = match (exit.exit, exit.reason) {
        (ExitCode::Success, _) => None,
        (_, Some(reason)) => Some(reason.to_string()),
        (code, None) => Some(code.slug().to_string()),
    };
    (i64::from(exit.exit as u8), error_type)
}

/// Resource attributes in merge order: derived keys, then
/// `OTEL_RESOURCE_ATTRIBUTES`, then `OTEL_SERVICE_NAME` (later wins).
pub fn resource(
    installation_id: Option<&str>,
    vcs: Option<&Vcs>,
    overrides: ResourceOverrides,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    set(&mut out, SERVICE_NAME, "grim");
    set(&mut out, SERVICE_VERSION, env!("CARGO_PKG_VERSION"));
    set(&mut out, OS_TYPE, os_type(std::env::consts::OS));
    set(&mut out, HOST_ARCH, host_arch(std::env::consts::ARCH));
    if let Some(id) = installation_id {
        set(&mut out, INSTALLATION_ID, id);
    }
    if let Some(vcs) = vcs {
        set(&mut out, VCS_URL, &vcs.url);
        if let Some(owner) = &vcs.owner {
            set(&mut out, VCS_OWNER, owner);
        }
        set(&mut out, VCS_NAME, &vcs.name);
        if let Some(provider) = &vcs.provider {
            set(&mut out, VCS_PROVIDER, provider);
        }
    }
    for (key, value) in &overrides.attributes {
        set(&mut out, key, value);
    }
    if let Some(name) = overrides.service_name.as_deref().filter(|n| !n.is_empty()) {
        set(&mut out, SERVICE_NAME, name);
    }
    out
}

fn set(resource: &mut Vec<(String, String)>, key: &str, value: &str) {
    match resource.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = value.to_string(),
        None => resource.push((key.to_string(), value.to_string())),
    }
}

/// Client names in [`ClientTarget::ALL`] order, whatever order they were recorded in.
fn client_names(clients: &[ClientTarget]) -> Vec<String> {
    ClientTarget::ALL
        .iter()
        .filter(|candidate| clients.contains(candidate))
        .map(|candidate| candidate.as_str().to_string())
        .collect()
}

fn scope_name(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::Global => "global",
        ConfigScope::Project => "project",
    }
}

fn artifact_event(event: &ArtifactEvent) -> Event {
    let mut attributes = Vec::with_capacity(3);
    if let Some(reference) = &event.reference {
        attributes.push((ARTIFACT_REF, AttrValue::Str(reference.clone())));
    }
    attributes.push((ARTIFACT_KIND, AttrValue::Str(event.kind.kind_str().to_string())));
    attributes.push((ARTIFACT_ACTION, AttrValue::Str(event.action.as_str().to_string())));
    Event {
        name: EVENT_ARTIFACT,
        attributes,
    }
}

/// Assembles the payload: `grim.command` first, then artifact events in record order.
pub fn build(resource: Vec<(String, String)>, state: &State, outcome: &Outcome) -> Payload {
    let (name, subcommand) = state.command.unwrap_or((UNKNOWN_COMMAND, None));
    let mut attributes = vec![(COMMAND_NAME, AttrValue::Str(name.to_string()))];
    if let Some(subcommand) = subcommand {
        attributes.push((COMMAND_SUBCOMMAND, AttrValue::Str(subcommand.to_string())));
    }
    attributes.push((COMMAND_DURATION, AttrValue::Double(outcome.duration.as_secs_f64())));
    attributes.push((EXIT_CODE, AttrValue::Int(outcome.exit_code)));
    if let Some(error_type) = &outcome.error_type {
        attributes.push((ERROR_TYPE, AttrValue::Str(error_type.clone())));
    }
    attributes.push((CI, AttrValue::Bool(outcome.ci)));
    if let Some(scope) = state.scope {
        attributes.push((SCOPE, AttrValue::Str(scope_name(scope).to_string())));
    }
    if let Some(clients) = &state.clients {
        attributes.push((CLIENTS, AttrValue::StrArray(client_names(clients))));
    }
    if let Some(detected) = &state.detected {
        attributes.push((CLIENTS_DETECTED, AttrValue::StrArray(client_names(detected))));
    }

    let mut events = vec![Event {
        name: EVENT_COMMAND,
        attributes,
    }];
    events.extend(state.artifacts.iter().map(artifact_event));
    Payload {
        resource,
        events,
        observed_at: SystemTime::now(),
    }
}

impl AttrValue {
    fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Str(s) => serde_json::Value::String(s.clone()),
            Self::Int(i) => serde_json::Value::from(*i),
            Self::Double(d) => serde_json::Value::from(*d),
            Self::Bool(b) => serde_json::Value::Bool(*b),
            Self::StrArray(items) => serde_json::Value::from(items.clone()),
        }
    }
}

impl Payload {
    /// The single-line JSON the debug audit line carries.
    pub fn to_json_line(&self) -> String {
        let resource: serde_json::Map<_, _> = self
            .resource
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();
        let events: Vec<_> = self
            .events
            .iter()
            .map(|event| {
                let attributes: serde_json::Map<_, _> = event
                    .attributes
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), v.to_json()))
                    .collect();
                serde_json::json!({ "name": event.name, "attributes": attributes })
            })
            .collect();
        serde_json::json!({ "resource": resource, "events": events }).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::artifact_kind::ArtifactKind;
    use crate::telemetry::record::ArtifactAction;

    fn outcome() -> Outcome {
        Outcome {
            duration: Duration::from_millis(412),
            exit_code: 0,
            error_type: None,
            ci: false,
        }
    }

    fn state() -> State {
        State::new(std::path::PathBuf::from("/grim-home"), false)
    }

    fn attr<'a>(event: &'a Event, key: &str) -> Option<&'a AttrValue> {
        event.attributes.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
    }

    fn keys(event: &Event) -> Vec<&'static str> {
        event.attributes.iter().map(|(k, _)| *k).collect()
    }

    #[test]
    fn os_type_maps_the_two_divergent_names() {
        assert_eq!(os_type("macos"), "darwin");
        assert_eq!(os_type("dragonfly"), "dragonflybsd");
        for same in ["linux", "windows", "freebsd", "netbsd", "openbsd"] {
            assert_eq!(os_type(same), same);
        }
    }

    #[test]
    fn host_arch_maps_the_four_divergent_names() {
        assert_eq!(host_arch("x86_64"), "amd64");
        assert_eq!(host_arch("aarch64"), "arm64");
        assert_eq!(host_arch("arm"), "arm32");
        assert_eq!(host_arch("powerpc64"), "ppc64");
        for same in ["x86", "riscv64", "s390x", "loongarch64"] {
            assert_eq!(host_arch(same), same);
        }
    }

    #[test]
    fn exit_info_success_carries_no_error_type() {
        assert_eq!(exit_info(&Ok(ExitCode::Success)), (0, None));
    }

    #[test]
    fn exit_info_nonzero_ok_uses_the_exit_slug() {
        assert_eq!(exit_info(&Ok(ExitCode::NotFound)), (79, Some("not-found".to_string())));
        assert_eq!(exit_info(&Ok(ExitCode::Failure)), (1, Some("failure".to_string())));
    }

    #[test]
    fn exit_info_unclassified_error_is_failure() {
        let err = anyhow::anyhow!("boom");
        assert_eq!(exit_info(&Err(err)), (1, Some("failure".to_string())));
    }

    #[test]
    fn exit_info_prefers_the_reason_slug_over_the_exit_slug() {
        let err = anyhow::Error::new(crate::error::Error::Config(
            crate::config::config_error::ConfigError::new(
                std::path::PathBuf::new(),
                crate::config::config_error::ConfigErrorKind::NotDiscovered,
            ),
        ));
        assert_eq!(exit_info(&Err(err)), (79, Some("no-config".to_string())));
    }

    #[test]
    fn exit_info_maps_a_closed_stdout_pipe_to_zero() {
        let err = anyhow::Error::new(crate::cli::printer::StdoutPipeClosed).context("while printing");
        assert_eq!(exit_info(&Err(err)), (0, None));
    }

    #[test]
    fn resource_has_exactly_the_documented_base_keys() {
        let r = resource(None, None, ResourceOverrides::default());
        let keys: Vec<_> = r.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, [SERVICE_NAME, SERVICE_VERSION, OS_TYPE, HOST_ARCH]);
        assert_eq!(r[0].1, "grim");
        assert_eq!(r[1].1, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn resource_adds_installation_id_and_vcs_keys_when_present() {
        let vcs = Vcs {
            url: "https://github.com/o/r".into(),
            owner: Some("o".into()),
            name: "r".into(),
            provider: Some("github".into()),
        };
        let r = resource(Some("id-1"), Some(&vcs), ResourceOverrides::default());
        let get = |k: &str| r.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get(INSTALLATION_ID), Some("id-1"));
        assert_eq!(get(VCS_URL), Some("https://github.com/o/r"));
        assert_eq!(get(VCS_OWNER), Some("o"));
        assert_eq!(get(VCS_NAME), Some("r"));
        assert_eq!(get(VCS_PROVIDER), Some("github"));
    }

    #[test]
    fn resource_omits_the_owner_key_for_a_one_segment_remote() {
        let vcs = Vcs {
            url: "https://srv/repo".into(),
            owner: None,
            name: "repo".into(),
            provider: None,
        };
        let r = resource(None, Some(&vcs), ResourceOverrides::default());
        let get = |k: &str| r.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get(VCS_OWNER), None);
        assert_eq!(get(VCS_URL), Some("https://srv/repo"));
        assert_eq!(get(VCS_NAME), Some("repo"));
    }

    #[test]
    fn resource_omits_the_provider_key_when_there_is_none() {
        let vcs = Vcs {
            url: "https://host/o/r".into(),
            owner: Some("o".into()),
            name: "r".into(),
            provider: None,
        };
        let r = resource(None, Some(&vcs), ResourceOverrides::default());
        assert!(r.iter().all(|(k, _)| k != VCS_PROVIDER));
    }

    #[test]
    fn resource_merge_order_is_derived_then_attributes_then_service_name() {
        let vcs = Vcs {
            url: "https://github.com/o/r".into(),
            owner: Some("o".into()),
            name: "r".into(),
            provider: None,
        };
        let overrides = ResourceOverrides {
            attributes: vec![
                (VCS_OWNER.into(), "operator".into()),
                (SERVICE_NAME.into(), "from-attributes".into()),
                ("deployment.environment".into(), "ci".into()),
            ],
            service_name: Some("from-service-name".into()),
        };
        let r = resource(None, Some(&vcs), overrides);
        let get = |k: &str| r.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get(VCS_OWNER), Some("operator"), "operator vcs.* beats the derived one");
        assert_eq!(
            get(SERVICE_NAME),
            Some("from-service-name"),
            "OTEL_SERVICE_NAME wins over both"
        );
        assert_eq!(get("deployment.environment"), Some("ci"));
        assert_eq!(
            r.iter().filter(|(k, _)| k == SERVICE_NAME).count(),
            1,
            "no duplicate keys"
        );
    }

    #[test]
    fn resource_ignores_an_empty_service_name() {
        let overrides = ResourceOverrides {
            attributes: Vec::new(),
            service_name: Some(String::new()),
        };
        let r = resource(None, None, overrides);
        assert_eq!(r[0], (SERVICE_NAME.to_string(), "grim".to_string()));
    }

    #[test]
    fn build_emits_the_command_event_first_with_required_attributes() {
        let mut s = state();
        s.command = Some(("status", None));
        let p = build(Vec::new(), &s, &outcome());
        assert_eq!(p.events.len(), 1);
        let e = &p.events[0];
        assert_eq!(e.name, EVENT_COMMAND);
        assert_eq!(keys(e), [COMMAND_NAME, COMMAND_DURATION, EXIT_CODE, CI]);
        assert_eq!(attr(e, COMMAND_NAME), Some(&AttrValue::Str("status".into())));
        assert_eq!(attr(e, COMMAND_DURATION), Some(&AttrValue::Double(0.412)));
        assert_eq!(attr(e, EXIT_CODE), Some(&AttrValue::Int(0)));
        assert_eq!(attr(e, CI), Some(&AttrValue::Bool(false)));
    }

    #[test]
    fn build_names_an_unstarted_command_unknown() {
        let p = build(Vec::new(), &state(), &outcome());
        assert_eq!(
            attr(&p.events[0], COMMAND_NAME),
            Some(&AttrValue::Str("unknown".into()))
        );
    }

    #[test]
    fn build_adds_subcommand_error_type_and_scope_only_when_known() {
        let mut s = state();
        s.command = Some(("config", Some("registry add")));
        s.scope = Some(ConfigScope::Project);
        let mut o = outcome();
        o.exit_code = 79;
        o.error_type = Some("no-config".into());
        o.ci = true;
        let e = &build(Vec::new(), &s, &o).events[0];
        assert_eq!(
            attr(e, COMMAND_SUBCOMMAND),
            Some(&AttrValue::Str("registry add".into()))
        );
        assert_eq!(attr(e, ERROR_TYPE), Some(&AttrValue::Str("no-config".into())));
        assert_eq!(attr(e, SCOPE), Some(&AttrValue::Str("project".into())));
        assert_eq!(attr(e, EXIT_CODE), Some(&AttrValue::Int(79)));
        assert_eq!(attr(e, CI), Some(&AttrValue::Bool(true)));
        s.scope = Some(ConfigScope::Global);
        let e = &build(Vec::new(), &s, &outcome()).events[0];
        assert_eq!(attr(e, SCOPE), Some(&AttrValue::Str("global".into())));
        assert!(attr(e, ERROR_TYPE).is_none());
    }

    #[test]
    fn build_sorts_clients_in_target_order_and_marks_absence_by_missing_key() {
        let mut s = state();
        s.clients = Some(vec![ClientTarget::Codex, ClientTarget::Claude, ClientTarget::Codex]);
        s.detected = Some(vec![ClientTarget::Copilot, ClientTarget::Codex, ClientTarget::Claude]);
        let e = &build(Vec::new(), &s, &outcome()).events[0];
        let strs = |names: &[&str]| AttrValue::StrArray(names.iter().map(|n| (*n).to_string()).collect());
        assert_eq!(attr(e, CLIENTS), Some(&strs(&["claude", "codex"])));
        assert_eq!(attr(e, CLIENTS_DETECTED), Some(&strs(&["claude", "copilot", "codex"])));
        let bare = &build(Vec::new(), &state(), &outcome()).events[0];
        assert!(attr(bare, CLIENTS).is_none() && attr(bare, CLIENTS_DETECTED).is_none());
    }

    #[test]
    fn build_emits_empty_client_arrays_when_recorded_empty() {
        let mut s = state();
        s.clients = Some(Vec::new());
        s.detected = Some(Vec::new());
        let e = &build(Vec::new(), &s, &outcome()).events[0];
        assert_eq!(attr(e, CLIENTS), Some(&AttrValue::StrArray(Vec::new())));
        assert_eq!(attr(e, CLIENTS_DETECTED), Some(&AttrValue::StrArray(Vec::new())));
    }

    #[test]
    fn build_emits_one_artifact_event_per_record_in_order_after_the_command() {
        let mut s = state();
        s.artifacts = vec![
            ArtifactEvent {
                kind: ArtifactKind::Skill,
                action: ArtifactAction::Installed,
                reference: Some("ghcr.io/acme/review:1".into()),
            },
            ArtifactEvent {
                kind: ArtifactKind::Bundle,
                action: ArtifactAction::Added,
                reference: None,
            },
        ];
        let p = build(Vec::new(), &s, &outcome());
        assert_eq!(p.events.len(), 3);
        assert_eq!(p.events[0].name, EVENT_COMMAND);
        let (first, second) = (&p.events[1], &p.events[2]);
        assert_eq!(first.name, EVENT_ARTIFACT);
        assert_eq!(keys(first), [ARTIFACT_REF, ARTIFACT_KIND, ARTIFACT_ACTION]);
        assert_eq!(
            attr(first, ARTIFACT_REF),
            Some(&AttrValue::Str("ghcr.io/acme/review:1".into()))
        );
        assert_eq!(attr(first, ARTIFACT_KIND), Some(&AttrValue::Str("skill".into())));
        assert_eq!(attr(first, ARTIFACT_ACTION), Some(&AttrValue::Str("installed".into())));
        assert_eq!(
            keys(second),
            [ARTIFACT_KIND, ARTIFACT_ACTION],
            "no ref key for a path source"
        );
        assert_eq!(attr(second, ARTIFACT_KIND), Some(&AttrValue::Str("bundle".into())));
        assert_eq!(attr(second, ARTIFACT_ACTION), Some(&AttrValue::Str("added".into())));
    }

    #[test]
    fn json_line_is_one_line_and_types_numbers_arrays_and_bools() {
        let mut s = state();
        s.command = Some(("install", None));
        s.scope = Some(ConfigScope::Project);
        s.clients = Some(vec![ClientTarget::Claude]);
        let resource = vec![(SERVICE_NAME.to_string(), "grim".to_string())];
        let line = build(resource, &s, &outcome()).to_json_line();
        assert!(!line.contains('\n'));
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["resource"]["service.name"], "grim");
        let attrs = &v["events"][0]["attributes"];
        assert_eq!(v["events"][0]["name"], "grim.command");
        assert_eq!(attrs["process.exit.code"], serde_json::json!(0));
        assert!(attrs["process.exit.code"].is_i64());
        assert_eq!(attrs["grim.command.duration"], serde_json::json!(0.412));
        assert_eq!(attrs["grim.ci"], serde_json::json!(false));
        assert_eq!(attrs["grim.clients"], serde_json::json!(["claude"]));
        assert_eq!(attrs["grim.scope"], "project");
    }
}
