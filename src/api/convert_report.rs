// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim convert` output.
//!
//! JSON format: a single object `{kind, name, from, input, output, moved,
//! dropped, warnings}`; every key is always present.

use std::io::{self, Write};
use std::path::PathBuf;

use serde::Serialize;

use crate::cli::printer::Printable;

/// A frontmatter key moved under `metadata."claude.<field>"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Moved {
    pub field: String,
    pub to: String,
}

/// Why a frontmatter key was removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DropReason {
    /// The `metadata."claude.<field>"` slot was already authored.
    Shadowed,
    /// The value has the wrong YAML type for its field.
    Unrepresentable,
    /// No registry row names the key.
    UnknownField,
}

/// A frontmatter key removed from the output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Dropped {
    pub field: String,
    pub reason: DropReason,
}

/// A Claude-only body token found in a unit file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BodyToken {
    pub file: String,
    pub line: usize,
    pub token: String,
}

/// The result of converting one artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConvertReport {
    pub kind: String,
    pub name: String,
    pub from: String,
    pub input: PathBuf,
    pub output: PathBuf,
    pub moved: Vec<Moved>,
    pub dropped: Vec<Dropped>,
    pub warnings: Vec<BodyToken>,
}

impl Printable for ConvertReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        writeln!(w, "converted {} {} -> {}", self.kind, self.name, self.output.display())?;
        let mut rows: Vec<Vec<String>> = Vec::new();
        for m in &self.moved {
            rows.push(vec![m.field.clone(), "moved".into(), format!("metadata.\"{}\"", m.to)]);
        }
        for d in &self.dropped {
            let reason = serde_json::to_value(d.reason)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            rows.push(vec![d.field.clone(), "dropped".into(), reason]);
        }
        for t in &self.warnings {
            rows.push(vec![
                t.token.clone(),
                "warning".into(),
                format!("{}:{}", t.file, t.line),
            ]);
        }
        crate::cli::printer::print_table(w, &["Field", "Action", "Detail"], &rows)
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialises_to_the_design_json_exactly() {
        let report = ConvertReport {
            kind: "skill".into(),
            name: "pdf".into(),
            from: "claude".into(),
            input: PathBuf::from("/abs/skills/pdf"),
            output: PathBuf::from("/abs/out/pdf"),
            moved: vec![Moved {
                field: "user-invocable".into(),
                to: "claude.user-invocable".into(),
            }],
            dropped: vec![Dropped {
                field: "hooks".into(),
                reason: DropReason::UnknownField,
            }],
            warnings: vec![BodyToken {
                file: "SKILL.md".into(),
                line: 12,
                token: "${CLAUDE_PLUGIN_ROOT}".into(),
            }],
        };
        let expected = serde_json::json!({
            "kind": "skill", "name": "pdf", "from": "claude",
            "input": "/abs/skills/pdf", "output": "/abs/out/pdf",
            "moved": [{"field": "user-invocable", "to": "claude.user-invocable"}],
            "dropped": [{"field": "hooks", "reason": "unknown-field"}],
            "warnings": [{"file": "SKILL.md", "line": 12, "token": "${CLAUDE_PLUGIN_ROOT}"}]
        });
        assert_eq!(serde_json::to_value(&report).unwrap(), expected);
    }

    #[test]
    fn every_drop_reason_has_its_design_spelling() {
        let spelled: Vec<_> = [
            DropReason::Shadowed,
            DropReason::Unrepresentable,
            DropReason::UnknownField,
        ]
        .iter()
        .map(|r| serde_json::to_value(r).unwrap())
        .collect();
        assert_eq!(spelled, ["shadowed", "unrepresentable", "unknown-field"]);
    }

    #[test]
    fn plain_table_lists_moved_dropped_and_warnings() {
        let report = ConvertReport {
            kind: "skill".into(),
            name: "s".into(),
            from: "claude".into(),
            input: "a".into(),
            output: "b".into(),
            moved: vec![Moved {
                field: "when_to_use".into(),
                to: "claude.when-to-use".into(),
            }],
            dropped: vec![Dropped {
                field: "hooks".into(),
                reason: DropReason::UnknownField,
            }],
            warnings: vec![BodyToken {
                file: "SKILL.md".into(),
                line: 3,
                token: "$ARGUMENTS".into(),
            }],
        };
        let mut buf = Vec::new();
        report.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.lines().next().unwrap().starts_with("converted skill"));
        assert!(out.lines().nth(1).unwrap().starts_with("Field"));
        assert!(out.contains("moved") && out.contains("unknown-field") && out.contains("SKILL.md:3"));
    }

    #[test]
    fn empty_lists_stay_present() {
        let report = ConvertReport {
            kind: "rule".into(),
            name: "r".into(),
            from: "claude".into(),
            input: "a".into(),
            output: "b".into(),
            moved: vec![],
            dropped: vec![],
            warnings: vec![],
        };
        let v = serde_json::to_value(&report).unwrap();
        assert_eq!(v["moved"], serde_json::json!([]));
        assert_eq!(v["dropped"], serde_json::json!([]));
        assert_eq!(v["warnings"], serde_json::json!([]));
    }
}
