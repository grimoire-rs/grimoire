// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The source-side grammar for an artifact's `requires` declaration.
//!
//! One grammar everywhere: a table (`skills`, `rules`, `agents`, `mcp`,
//! `hooks`, `bundles`, `settings`), a binding name that passes [`SkillName::parse`], and a
//! value that passes [`MemberRef::parse`]. Skills, rules and agents carry it
//! as flattened frontmatter `metadata` keys (`grim.requires.<table>.<name>`),
//! mcp and hook sources as `[requires.<table>]` TOML tables, and a bundle
//! source as a `[bundles]` table. Design record:
//! `.agents/adr/adr_artifact_dependencies.md` Decision 6.

use std::collections::BTreeMap;

use super::ArtifactKind;
use super::bundle::BundleMember;
use super::member_ref::{MemberRef, MemberRefError};
use crate::skill::SkillName;

/// Prefix of a flattened `requires` metadata key.
pub const REQUIRES_KEY_PREFIX: &str = "grim.requires.";

/// Why a `requires` declaration does not parse.
#[derive(Debug, thiserror::Error)]
pub enum RequiresDeclError {
    /// A key is not `grim.requires.<table>.<name>`.
    #[error("requires key '{key}' must be grim.requires.<table>.<name>")]
    MalformedKey { key: String },

    /// The table is not one of the seven requires tables.
    #[error("requires table '{table}' in '{key}' is not one of skills, rules, agents, mcp, hooks, bundles, settings")]
    UnknownTable { key: String, table: String },

    /// The binding name fails [`SkillName::parse`].
    #[error("requires entry '{key}' has an invalid name: {reason}")]
    InvalidName { key: String, reason: String },

    /// The value is not a member reference.
    #[error("requires entry '{key}' has an invalid reference")]
    InvalidRef {
        key: String,
        #[source]
        source: MemberRefError,
    },

    /// A TOML value is not the shape the grammar needs (a table, or a string reference).
    #[error("requires entry '{key}' must be {expected}")]
    WrongType { key: String, expected: &'static str },

    /// One `(kind, name)` is declared twice.
    #[error("requires entry '{key}' declares {kind} '{name}' more than once")]
    Duplicate {
        key: String,
        kind: ArtifactKind,
        name: String,
    },
}

/// The kind a requires table names, by its `subdir` spelling.
fn kind_of_table(table: &str) -> Option<ArtifactKind> {
    ArtifactKind::ALL.into_iter().find(|k| k.subdir() == table)
}

fn member(key: &str, kind: ArtifactKind, name: &str, value: &str) -> Result<BundleMember, RequiresDeclError> {
    SkillName::parse(name).map_err(|reason| RequiresDeclError::InvalidName {
        key: key.to_string(),
        reason,
    })?;
    MemberRef::parse(value).map_err(|source| RequiresDeclError::InvalidRef {
        key: key.to_string(),
        source,
    })?;
    Ok(BundleMember {
        kind,
        name: name.to_string(),
        id: value.to_string(),
    })
}

/// Sort by `(kind, name)` and refuse a repeated pair.
fn finish(mut members: Vec<(String, BundleMember)>) -> Result<Vec<BundleMember>, RequiresDeclError> {
    members.sort_by(|(_, a), (_, b)| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
    if let Some(pair) = members
        .windows(2)
        .find(|w| (w[0].1.kind, &w[0].1.name) == (w[1].1.kind, &w[1].1.name))
    {
        return Err(RequiresDeclError::Duplicate {
            key: pair[1].0.clone(),
            kind: pair[1].1.kind,
            name: pair[1].1.name.clone(),
        });
    }
    Ok(members.into_iter().map(|(_, m)| m).collect())
}

/// Read the `grim.requires.<table>.<name>` keys of a `metadata` map.
///
/// Other keys are ignored. The name is everything after the table, so a name
/// with a period (`socket.io`) is one name.
///
/// # Errors
///
/// [`RequiresDeclError`] for a `grim.requires.` key with a missing name, an
/// unknown table, an invalid name or an invalid reference.
pub fn from_metadata(metadata: &BTreeMap<String, String>) -> Result<Vec<BundleMember>, RequiresDeclError> {
    let mut out = Vec::new();
    for (key, value) in metadata {
        let Some(rest) = key.strip_prefix(REQUIRES_KEY_PREFIX) else {
            continue;
        };
        let (table, name) = rest
            .split_once('.')
            .ok_or_else(|| RequiresDeclError::MalformedKey { key: key.clone() })?;
        let kind = kind_of_table(table).ok_or_else(|| RequiresDeclError::UnknownTable {
            key: key.clone(),
            table: table.to_string(),
        })?;
        out.push((key.clone(), member(key, kind, name, value)?));
    }
    finish(out)
}

/// Read the `[requires.<table>]` tables and the `[bundles]` table of a parsed
/// TOML source (an mcp descriptor, a `hook.toml`, a bundle source).
///
/// # Errors
///
/// [`RequiresDeclError`] for a table or entry of the wrong type, an unknown
/// table, an invalid name or reference, or a pair declared in both
/// `[requires.bundles]` and `[bundles]`.
pub fn from_toml_tables(source: &toml::Table) -> Result<Vec<BundleMember>, RequiresDeclError> {
    let mut out = Vec::new();
    if let Some(requires) = source.get("requires") {
        let requires = requires.as_table().ok_or_else(|| RequiresDeclError::WrongType {
            key: "requires".to_string(),
            expected: "a table",
        })?;
        for (table, entries) in requires {
            let key = format!("requires.{table}");
            let kind = kind_of_table(table).ok_or_else(|| RequiresDeclError::UnknownTable {
                key: key.clone(),
                table: table.clone(),
            })?;
            collect_table(&mut out, &key, kind, entries)?;
        }
    }
    if let Some(bundles) = source.get("bundles") {
        collect_table(&mut out, "bundles", ArtifactKind::Bundle, bundles)?;
    }
    finish(out)
}

fn collect_table(
    out: &mut Vec<(String, BundleMember)>,
    key: &str,
    kind: ArtifactKind,
    entries: &toml::Value,
) -> Result<(), RequiresDeclError> {
    let entries = entries.as_table().ok_or_else(|| RequiresDeclError::WrongType {
        key: key.to_string(),
        expected: "a table",
    })?;
    for (name, value) in entries {
        let entry_key = format!("{key}.{name}");
        let value = value.as_str().ok_or_else(|| RequiresDeclError::WrongType {
            key: entry_key.clone(),
            expected: "a string reference",
        })?;
        out.push((entry_key.clone(), member(&entry_key, kind, name, value)?));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn toml_of(s: &str) -> toml::Table {
        s.parse().expect("test toml parses")
    }

    #[test]
    fn metadata_keys_map_to_members_sorted_by_kind_then_name() {
        let got = from_metadata(&meta(&[
            ("grim.requires.mcp.gdrive", "ghcr.io/acme/mcp/gdrive:2"),
            ("grim.requires.skills.zeta", "./zeta:1"),
            ("grim.requires.skills.alpha", "../shared/alpha:1"),
            ("grim.requires.rules.style", "ghcr.io/acme/style:1"),
            ("grim.requires.agents.helper", "./helper:1"),
            ("grim.requires.hooks.guard", "./guard:1"),
            ("grim.requires.bundles.set", "./set:1"),
            ("claude.user-invocable", "true"),
            ("repository", "https://example.com/r"),
        ]))
        .unwrap();
        let order: Vec<(ArtifactKind, &str)> = got.iter().map(|m| (m.kind, m.name.as_str())).collect();
        assert_eq!(
            order,
            [
                (ArtifactKind::Skill, "alpha"),
                (ArtifactKind::Skill, "zeta"),
                (ArtifactKind::Rule, "style"),
                (ArtifactKind::Agent, "helper"),
                (ArtifactKind::Bundle, "set"),
                (ArtifactKind::Mcp, "gdrive"),
                (ArtifactKind::Hook, "guard"),
            ]
        );
        assert_eq!(got[0].id, "../shared/alpha:1");
    }

    #[test]
    fn no_requires_keys_is_empty() {
        assert!(from_metadata(&meta(&[("repository", "https://x")])).unwrap().is_empty());
        assert!(from_metadata(&BTreeMap::new()).unwrap().is_empty());
    }

    #[test]
    fn a_period_in_the_name_is_part_of_the_name() {
        let got = from_metadata(&meta(&[("grim.requires.skills.socket.io", "./socket.io:1")])).unwrap();
        assert_eq!(got[0].name, "socket.io");
    }

    #[test]
    fn settings_is_a_requires_table_and_the_refusal_names_it() {
        let got = from_metadata(&meta(&[("grim.requires.settings.team", "./team:1")])).unwrap();
        assert_eq!(got[0].kind, ArtifactKind::Settings);
        let got = from_toml_tables(&toml_of("[requires.settings]\nteam = \"./team:1\"\n")).unwrap();
        assert_eq!(got[0].kind, ArtifactKind::Settings);

        let err = from_metadata(&meta(&[("grim.requires.widgets.a", "./a:1")])).unwrap_err();
        assert!(err.to_string().contains("settings"), "{err}");
    }

    #[test]
    fn metadata_refuses_a_bad_table_name_or_ref() {
        let cases = [
            ("grim.requires.widgets.a", "./a:1", "UnknownTable"),
            ("grim.requires.skills", "./a:1", "MalformedKey"),
            ("grim.requires.skills.", "./a:1", "InvalidName"),
            ("grim.requires.skills.Bad_Name", "./a:1", "InvalidName"),
            ("grim.requires.skills.a", "no-registry:1", "InvalidRef"),
            ("grim.requires.skills.a", "./a/../b:1", "InvalidRef"),
        ];
        for (key, value, variant) in cases {
            let err = from_metadata(&meta(&[(key, value)])).unwrap_err();
            assert!(format!("{err:?}").starts_with(variant), "{key} = {value}: {err:?}");
        }
    }

    #[test]
    fn toml_reads_requires_tables_and_the_bundles_table() {
        let got = from_toml_tables(&toml_of(
            r#"
            [requires.skills]
            helper = "./helper:1"
            [requires.mcp]
            gdrive = "ghcr.io/acme/mcp/gdrive:2"
            [bundles]
            set = "./set:1"
            "#,
        ))
        .unwrap();
        let order: Vec<(ArtifactKind, &str)> = got.iter().map(|m| (m.kind, m.name.as_str())).collect();
        assert_eq!(
            order,
            [
                (ArtifactKind::Skill, "helper"),
                (ArtifactKind::Bundle, "set"),
                (ArtifactKind::Mcp, "gdrive"),
            ]
        );
    }

    #[test]
    fn toml_without_requires_is_empty() {
        assert!(from_toml_tables(&toml_of("name = \"x\"")).unwrap().is_empty());
    }

    #[test]
    fn toml_refuses_wrong_shapes() {
        let cases = [
            "requires = 1",
            "[requires]\nwidgets = {}",
            "[requires]\nskills = \"./a:1\"",
            "[requires.skills]\na = 1",
            "[requires.skills]\nBad_Name = \"./a:1\"",
            "[requires.skills]\na = \"bare:1\"",
            "bundles = \"./a:1\"",
        ];
        for src in cases {
            assert!(from_toml_tables(&toml_of(src)).is_err(), "{src}");
        }
    }

    #[test]
    fn toml_refuses_a_pair_declared_twice() {
        let err = from_toml_tables(&toml_of(
            "[requires.bundles]\nset = \"./a:1\"\n[bundles]\nset = \"./b:1\"",
        ))
        .unwrap_err();
        assert!(matches!(err, RequiresDeclError::Duplicate { .. }), "{err:?}");
    }
}
