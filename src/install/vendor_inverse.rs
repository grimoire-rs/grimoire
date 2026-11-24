// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Inverse of the Claude renderer: authored Claude frontmatter back to the portable form.

use serde_yaml::{Mapping, Value};

use crate::api::convert_report::{DropReason, Dropped, Moved};
use crate::oci::ArtifactKind;

use super::vendor::{FieldType, KnownField};
use super::vendor_claude::{CLAUDE_AGENT_FIELDS, CLAUDE_SKILL_FIELDS};

/// The converted frontmatter and what the conversion did to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inverted {
    /// Re-serialised YAML frontmatter, keys in input order.
    pub frontmatter: String,
    pub moved: Vec<Moved>,
    pub dropped: Vec<Dropped>,
}

/// Why a frontmatter could not be inverted.
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("frontmatter is not valid YAML: {0}")]
    Unparseable(String),
    #[error("frontmatter is not a mapping")]
    NotAMapping,
    #[error("artifact kind '{0}' has no Claude inverse")]
    UnsupportedKind(ArtifactKind),
}

const SKILL_COMMON: &[&str] = &[
    "name",
    "description",
    "license",
    "compatibility",
    "allowed-tools",
    "metadata",
];
const AGENT_COMMON: &[&str] = &["name", "description", "model", "tools", "license", "metadata"];
const RULE_COMMON: &[&str] = &["paths", "license", "metadata"];

/// Invert `frontmatter` (the text between the `---` fences) for `kind`.
///
/// # Errors
///
/// [`ConvertError`] when the YAML does not parse or is not a mapping.
pub fn invert(kind: ArtifactKind, frontmatter: &str) -> Result<Inverted, ConvertError> {
    let (common, registry): (&[&str], &[KnownField]) = match kind {
        ArtifactKind::Skill => (SKILL_COMMON, CLAUDE_SKILL_FIELDS),
        ArtifactKind::Agent => (AGENT_COMMON, CLAUDE_AGENT_FIELDS),
        ArtifactKind::Rule => (RULE_COMMON, &[]),
        other => return Err(ConvertError::UnsupportedKind(other)),
    };
    let input = match serde_yaml::from_str::<Value>(frontmatter) {
        Ok(Value::Mapping(m)) => m,
        Ok(Value::Null) => Mapping::new(),
        Ok(_) => return Err(ConvertError::NotAMapping),
        Err(e) => return Err(ConvertError::Unparseable(e.to_string())),
    };

    let metadata_key = Value::String("metadata".into());
    let mut metadata = match input.get(&metadata_key) {
        Some(Value::Mapping(m)) => Some(m.clone()),
        Some(Value::Null) | None => Some(Mapping::new()),
        Some(_) => None,
    };
    let mut kept = Mapping::new();
    let mut moved = Vec::new();
    let mut dropped = Vec::new();
    let mut additions: Vec<(String, String)> = Vec::new();

    for (key, value) in &input {
        let Some(name) = key.as_str() else {
            dropped.push(Dropped {
                field: serde_yaml::to_string(key).unwrap_or_default().trim().to_string(),
                reason: DropReason::UnknownField,
            });
            continue;
        };
        if common.contains(&name) {
            kept.insert(key.clone(), value.clone());
            continue;
        }
        let drop = |reason| Dropped {
            field: name.to_string(),
            reason,
        };
        let Some(row) = registry.iter().find(|r| r.native == name) else {
            dropped.push(drop(DropReason::UnknownField));
            continue;
        };
        let slot = format!("claude.{}", row.field);
        let Some(meta) = metadata.as_ref() else {
            dropped.push(drop(DropReason::Unrepresentable));
            continue;
        };
        if meta.contains_key(Value::String(slot.clone())) {
            dropped.push(drop(DropReason::Shadowed));
            continue;
        }
        match stored(row.ty, value) {
            Some(s) => {
                moved.push(Moved {
                    field: name.to_string(),
                    to: slot.clone(),
                });
                additions.push((slot, s));
            }
            None => dropped.push(drop(DropReason::Unrepresentable)),
        }
    }

    if !additions.is_empty()
        && let Some(meta) = metadata.as_mut()
    {
        for (slot, s) in additions {
            meta.insert(Value::String(slot), Value::String(s));
        }
    }
    if let Some(meta) = metadata
        && !meta.is_empty()
    {
        // Replaces in place when authored, else appends after every kept key.
        kept.insert(metadata_key, Value::Mapping(meta));
    }

    let frontmatter = if kept.is_empty() {
        String::new()
    } else {
        serde_yaml::to_string(&kept).map_err(|e| ConvertError::Unparseable(e.to_string()))?
    };
    Ok(Inverted {
        frontmatter,
        moved,
        dropped,
    })
}

/// The metadata string for `value` under `ty`, or `None` when the YAML type is wrong.
fn stored(ty: FieldType, value: &Value) -> Option<String> {
    match (ty, value) {
        (FieldType::Bool, Value::Bool(b)) => Some(b.to_string()),
        (FieldType::String, Value::String(s)) => Some(s.clone()),
        (FieldType::Enum(set), Value::String(s)) if set.contains(&s.as_str()) => Some(s.clone()),
        (FieldType::Integer, Value::Number(n)) => n.as_i64().map(|i| i.to_string()),
        (FieldType::Float, Value::Number(n)) => n.as_f64().filter(|f| f.is_finite()).map(|f| f.to_string()),
        (FieldType::CommaList, Value::String(s)) => Some(s.clone()),
        (FieldType::CommaList, Value::Sequence(items)) => {
            let parts: Option<Vec<&str>> = items
                .iter()
                .map(|i| i.as_str().filter(|s| !s.trim().is_empty() && !s.contains(',')))
                .collect();
            parts.map(|p| p.join(", "))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::vendor::Vendor;
    use crate::install::vendor_claude::ClaudeVendor;
    use crate::skill::agent_frontmatter::AgentFrontmatter;
    use std::path::Path;

    fn parse(s: &str) -> Mapping {
        match serde_yaml::from_str::<Value>(s).unwrap() {
            Value::Mapping(m) => m,
            _ => panic!("not a mapping"),
        }
    }

    fn get<'a>(m: &'a Mapping, k: &str) -> Option<&'a Value> {
        m.get(Value::String(k.into()))
    }

    fn keys(m: &Mapping) -> Vec<String> {
        m.keys().map(|k| k.as_str().unwrap().to_string()).collect()
    }

    /// An authored value of the row's type, plus the semantic check on the lifted value.
    fn samples(ty: FieldType) -> Vec<Value> {
        match ty {
            FieldType::Bool => vec![Value::Bool(true), Value::Bool(false)],
            FieldType::String => vec![Value::String("some text".into())],
            FieldType::Enum(set) => set.iter().map(|s| Value::String((*s).into())).collect(),
            FieldType::Integer => vec![Value::Number(7.into()), Value::Number((-3).into())],
            FieldType::Float => vec![Value::Number(serde_yaml::Number::from(1.5)), Value::Number(2.into())],
            FieldType::CommaList => vec![
                Value::String("a,  b ,c".into()),
                Value::Sequence(vec![Value::String("a".into()), Value::String("b c".into())]),
            ],
        }
    }

    fn items(v: &Value) -> Vec<String> {
        match v {
            Value::String(s) => s
                .split(',')
                .map(|i| i.trim().to_string())
                .filter(|i| !i.is_empty())
                .collect(),
            Value::Sequence(q) => q.iter().map(|i| i.as_str().unwrap().trim().to_string()).collect(),
            _ => panic!("not a list"),
        }
    }

    fn same(ty: FieldType, authored: &Value, lifted: &Value) {
        match ty {
            FieldType::CommaList => assert_eq!(items(authored), items(lifted)),
            FieldType::Float => assert_eq!(authored.as_f64(), lifted.as_f64()),
            _ => assert_eq!(authored, lifted),
        }
    }

    fn lift(kind: ArtifactKind, fm: &str) -> Mapping {
        let doc = format!("---\n{fm}---\nbody\n");
        let rendered = match kind {
            ArtifactKind::Skill => ClaudeVendor.skill_index(&doc).unwrap(),
            ArtifactKind::Agent => {
                let parsed = AgentFrontmatter::parse_doc(&doc, Path::new("a.md")).unwrap();
                ClaudeVendor.agent_index(&parsed, "").unwrap()
            }
            _ => unreachable!(),
        }
        .expect("converted file carries claude metadata, so the lift re-renders it");
        let body = rendered.document.strip_prefix("---\n").unwrap();
        parse(body.split("---\n").next().unwrap())
    }

    #[test]
    fn every_registry_row_round_trips_through_the_install_lift() {
        for (kind, registry, common) in [
            (ArtifactKind::Skill, CLAUDE_SKILL_FIELDS, SKILL_COMMON),
            (ArtifactKind::Agent, CLAUDE_AGENT_FIELDS, AGENT_COMMON),
        ] {
            for row in registry {
                for authored in samples(row.ty) {
                    let mut src = Mapping::new();
                    src.insert("name".into(), "x".into());
                    src.insert("description".into(), "d".into());
                    src.insert(row.native.into(), authored.clone());
                    let out = invert(kind, &serde_yaml::to_string(&src).unwrap()).unwrap();
                    let fm = parse(&out.frontmatter);
                    if common.contains(&row.native) {
                        assert_eq!(get(&fm, row.native), Some(&authored), "{} stays top-level", row.native);
                        assert!(out.moved.is_empty());
                        continue;
                    }
                    assert_eq!(out.moved.len(), 1, "{}: {:?}", row.native, out.dropped);
                    assert!(get(&fm, row.native).is_none());
                    let lifted = lift(kind, &out.frontmatter);
                    same(row.ty, &authored, get(&lifted, row.native).expect("lifted back"));
                }
            }
        }
    }

    fn one(kind: ArtifactKind, yaml: &str) -> Inverted {
        invert(kind, yaml).unwrap()
    }

    fn meta(inv: &Inverted, slot: &str) -> Option<String> {
        let fm = parse(&inv.frontmatter);
        get(&fm, "metadata")?
            .as_mapping()?
            .get(Value::String(slot.into()))?
            .as_str()
            .map(String::from)
    }

    #[test]
    fn coercions_follow_the_table() {
        let s = ArtifactKind::Skill;
        assert_eq!(
            meta(&one(s, "user-invocable: false\n"), "claude.user-invocable").unwrap(),
            "false"
        );
        assert_eq!(
            meta(&one(ArtifactKind::Agent, "maxTurns: 12\n"), "claude.max-turns").unwrap(),
            "12"
        );
        assert_eq!(
            meta(&one(ArtifactKind::Agent, "skills: [a, b]\n"), "claude.skills").unwrap(),
            "a, b"
        );
        assert_eq!(
            meta(&one(ArtifactKind::Agent, "skills: 'a,b'\n"), "claude.skills").unwrap(),
            "a,b"
        );
        assert_eq!(meta(&one(s, "effort: high\n"), "claude.effort").unwrap(), "high");
    }

    #[test]
    fn wrong_types_are_unrepresentable() {
        for (kind, yaml, field) in [
            (ArtifactKind::Skill, "user-invocable: yes-please\n", "user-invocable"),
            (ArtifactKind::Skill, "effort: extreme\n", "effort"),
            (ArtifactKind::Agent, "maxTurns: 1.5\n", "maxTurns"),
            (ArtifactKind::Agent, "maxTurns: '3'\n", "maxTurns"),
            (ArtifactKind::Agent, "skills: [a, 'b,c']\n", "skills"),
            (ArtifactKind::Agent, "skills: [a, '']\n", "skills"),
            (ArtifactKind::Agent, "skills: [a, 1]\n", "skills"),
            (ArtifactKind::Agent, "autoCompactWindow: .inf\n", "autoCompactWindow"),
        ] {
            let inv = one(kind, yaml);
            assert!(inv.moved.is_empty(), "{yaml}");
            assert_eq!(
                inv.dropped,
                vec![Dropped {
                    field: field.into(),
                    reason: DropReason::Unrepresentable
                }],
                "{yaml}"
            );
        }
    }

    #[test]
    fn integer_and_float_forms() {
        assert_eq!(
            meta(&one(ArtifactKind::Agent, "maxTurns: -4\n"), "claude.max-turns").unwrap(),
            "-4"
        );
        assert_eq!(
            stored(FieldType::Float, &Value::Number(serde_yaml::Number::from(0.1))).unwrap(),
            "0.1"
        );
        assert_eq!(stored(FieldType::Float, &Value::Number(3.into())).unwrap(), "3");
        assert_eq!(
            stored(FieldType::Float, &Value::Number(serde_yaml::Number::from(f64::NAN))),
            None
        );
    }

    #[test]
    fn shadowed_keeps_metadata_and_drops_top_level() {
        let inv = one(
            ArtifactKind::Skill,
            "name: x\nuser-invocable: false\nmetadata:\n  claude.user-invocable: 'true'\n",
        );
        assert!(inv.moved.is_empty());
        assert_eq!(
            inv.dropped,
            vec![Dropped {
                field: "user-invocable".into(),
                reason: DropReason::Shadowed
            }]
        );
        assert_eq!(meta(&inv, "claude.user-invocable").unwrap(), "true");
        assert!(get(&parse(&inv.frontmatter), "user-invocable").is_none());
    }

    #[test]
    fn unknown_fields_are_dropped_and_common_kept() {
        let inv = one(
            ArtifactKind::Skill,
            "name: x\nhooks: {}\nversion: 1\nlicense: MIT\ncompatibility: c\nallowed-tools: Read\n",
        );
        let dropped: Vec<_> = inv.dropped.iter().map(|d| (d.field.as_str(), d.reason)).collect();
        assert_eq!(
            dropped,
            vec![
                ("hooks", DropReason::UnknownField),
                ("version", DropReason::UnknownField)
            ]
        );
        assert_eq!(
            keys(&parse(&inv.frontmatter)),
            ["name", "license", "compatibility", "allowed-tools"]
        );
        let rule = one(ArtifactKind::Rule, "paths: ['a/**']\nlicense: MIT\nmodel: x\n");
        assert_eq!(keys(&parse(&rule.frontmatter)), ["paths", "license"]);
        assert_eq!(rule.dropped.len(), 1);
    }

    #[test]
    fn key_order_is_preserved_and_moves_append_to_metadata() {
        let inv = one(
            ArtifactKind::Skill,
            "name: x\nuser-invocable: false\ndescription: d\nmetadata:\n  k: v\nmodel: m\ndisable-model-invocation: true\n",
        );
        let fm = parse(&inv.frontmatter);
        assert_eq!(keys(&fm), ["name", "description", "metadata"]);
        let md = get(&fm, "metadata").unwrap().as_mapping().unwrap();
        assert_eq!(
            keys(md),
            [
                "k",
                "claude.user-invocable",
                "claude.model",
                "claude.disable-model-invocation"
            ]
        );
        let moved: Vec<_> = inv.moved.iter().map(|m| m.field.as_str()).collect();
        assert_eq!(moved, ["user-invocable", "model", "disable-model-invocation"]);
    }

    #[test]
    fn bad_input_errors() {
        assert!(matches!(
            invert(ArtifactKind::Skill, "- a\n"),
            Err(ConvertError::NotAMapping)
        ));
        assert!(matches!(
            invert(ArtifactKind::Skill, "a: [\n"),
            Err(ConvertError::Unparseable(_))
        ));
        assert!(matches!(
            invert(ArtifactKind::Mcp, "a: 1\n"),
            Err(ConvertError::UnsupportedKind(_))
        ));
    }
}
