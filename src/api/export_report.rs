// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim export plugin` output (design record C-029).
//!
//! JSON format: `{"items": [...]}` where each item carries `plugin`,
//! `client`, `family`, `format`, `path`, `version`, `members` and
//! `omitted` — every key always present, `members`/`omitted` sorted
//! `(kind, name)` and `[]` when empty. Emitted only on success.
//!
//! Plain format: one table `Plugin | Client | Version | Path | Omitted`,
//! `Omitted` being the count of omitted members.

use std::io::{self, Write};
use std::path::PathBuf;

use serde::Serialize;

use crate::cli::printer::{Printable, print_table};
use crate::export::family::{Family, OmitReason};
use crate::oci::ArtifactKind;

/// One emitted-or-declared member of an exported plugin.
#[derive(Debug, Serialize)]
pub struct ExportMember {
    pub kind: ArtifactKind,
    pub name: String,
    pub lock_name: String,
    pub pinned: String,
}

/// One member left out of an exported plugin, and why.
#[derive(Debug, Serialize)]
pub struct ExportOmission {
    pub kind: ArtifactKind,
    pub name: String,
    pub reason: OmitReason,
}

/// The output format of one exported plugin (C-029).
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormatKind {
    Dir,
    Zip,
}

/// One `(plugin, client)` pair exported by `grim export plugin`.
#[derive(Debug, Serialize)]
pub struct ExportItem {
    pub plugin: String,
    pub client: String,
    pub family: Family,
    pub format: OutputFormatKind,
    /// Absolute final path of the exported directory or zip.
    pub path: PathBuf,
    pub version: String,
    pub members: Vec<ExportMember>,
    pub omitted: Vec<ExportOmission>,
}

/// The result of an export pass: one row per `(plugin, client)` pair.
#[derive(Debug, Serialize)]
pub struct ExportReport {
    items: Vec<ExportItem>,
}

impl ExportReport {
    /// Build from operation results. Item order is the caller's; each item's
    /// `members` and `omitted` are sorted `(kind, name)` with `kind` in
    /// `ArtifactKind` declaration order and `name` bytewise (C-029).
    pub fn new(mut items: Vec<ExportItem>) -> Self {
        for item in &mut items {
            item.members.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
            item.omitted.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
        }
        Self { items }
    }
}

impl Printable for ExportReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        let rows: Vec<Vec<String>> = self
            .items
            .iter()
            .map(|i| {
                vec![
                    i.plugin.clone(),
                    i.client.clone(),
                    i.version.clone(),
                    i.path.display().to_string(),
                    i.omitted.len().to_string(),
                ]
            })
            .collect();
        print_table(w, &["Plugin", "Client", "Version", "Path", "Omitted"], &rows)
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed absolute path, free of spaces so plain rows split cleanly.
    fn abs(name: &str) -> PathBuf {
        PathBuf::from(if cfg!(windows) { r"C:\out" } else { "/out" }).join(name)
    }

    fn member(kind: ArtifactKind, name: &str) -> ExportMember {
        ExportMember {
            kind,
            name: name.to_string(),
            lock_name: format!("{name}-locked"),
            pinned: format!("ghcr.io/acme/{name}@sha256:{}", "a".repeat(64)),
        }
    }

    fn omission(kind: ArtifactKind, name: &str, reason: OmitReason) -> ExportOmission {
        ExportOmission {
            kind,
            name: name.to_string(),
            reason,
        }
    }

    fn item(plugin: &str, client: &str, members: Vec<ExportMember>, omitted: Vec<ExportOmission>) -> ExportItem {
        ExportItem {
            plugin: plugin.to_string(),
            client: client.to_string(),
            family: Family::Claude,
            format: OutputFormatKind::Zip,
            path: abs(&format!("{plugin}.{client}.zip")),
            version: "1.2.0+0123456789ab".to_string(),
            members,
            omitted,
        }
    }

    fn json(r: &ExportReport) -> (String, serde_json::Value) {
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        let v = serde_json::from_str(&text).unwrap();
        (text, v)
    }

    fn plain(r: &ExportReport) -> String {
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    /// Object keys at exactly `indent` spaces, in emitted order. serde_json
    /// has no `preserve_order` here, so key order is only visible in the
    /// pretty text (2-space indent: items at 6, member/omission objects at 10).
    fn keys_at(text: &str, indent: usize) -> Vec<String> {
        text.lines()
            .filter_map(|l| {
                let rest = l.strip_prefix(&" ".repeat(indent))?;
                let key = rest.strip_prefix('"')?.split_once("\":")?.0;
                Some(key.to_string())
            })
            .collect()
    }

    #[test]
    fn c029_json_is_items_envelope() {
        let r = ExportReport::new(vec![item("team", "claude", vec![], vec![])]);
        let (_, v) = json(&r);
        let obj = v.as_object().unwrap();
        assert_eq!(obj.keys().collect::<Vec<_>>(), ["items"]);
        assert_eq!(v["items"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn c029_item_key_list_and_order() {
        let r = ExportReport::new(vec![item(
            "team",
            "claude",
            vec![member(ArtifactKind::Skill, "s")],
            vec![omission(ArtifactKind::Rule, "r", OmitReason::NoFormatSurface)],
        )]);
        let (text, _) = json(&r);
        assert_eq!(
            keys_at(&text, 6),
            [
                "plugin", "client", "family", "format", "path", "version", "members", "omitted"
            ]
        );
        // One member then one omission, each with its exact key list.
        assert_eq!(
            keys_at(&text, 10),
            ["kind", "name", "lock_name", "pinned", "kind", "name", "reason"]
        );
    }

    #[test]
    fn c029_values_and_literals() {
        let mut agent = item(
            "team",
            "codex",
            vec![member(ArtifactKind::Mcp, "srv")],
            vec![
                omission(ArtifactKind::Agent, "a", OmitReason::NoFormatSurface),
                omission(ArtifactKind::Mcp, "m", OmitReason::NotRepresentable),
                omission(ArtifactKind::Skill, "s", OmitReason::ClientDeclined),
            ],
        );
        agent.family = Family::AgentPlugins;
        agent.format = OutputFormatKind::Dir;
        agent.path = abs("team.codex");
        let r = ExportReport::new(vec![item("team", "claude", vec![], vec![]), agent]);
        let (_, v) = json(&r);
        let (c, a) = (&v["items"][0], &v["items"][1]);
        assert_eq!(c["family"], "claude");
        assert_eq!(c["format"], "zip");
        assert_eq!(a["family"], "agent-plugins");
        assert_eq!(a["format"], "dir");
        assert_eq!(a["plugin"], "team");
        assert_eq!(a["client"], "codex");
        assert_eq!(a["version"], "1.2.0+0123456789ab");
        assert_eq!(a["members"][0]["kind"], "mcp");
        assert_eq!(a["members"][0]["name"], "srv");
        assert_eq!(a["members"][0]["lock_name"], "srv-locked");
        assert_eq!(
            a["members"][0]["pinned"],
            format!("ghcr.io/acme/srv@sha256:{}", "a".repeat(64))
        );
        let reasons: Vec<_> = a["omitted"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["reason"].clone())
            .collect();
        assert!(reasons.contains(&"client-declined".into()));
        assert!(reasons.contains(&"no-format-surface".into()));
        assert!(reasons.contains(&"not-representable".into()));
    }

    #[test]
    fn c029_path_is_absolute_string() {
        let r = ExportReport::new(vec![item("team", "claude", vec![], vec![])]);
        let (_, v) = json(&r);
        let p = v["items"][0]["path"].as_str().unwrap();
        assert_eq!(p, abs("team.claude.zip").to_str().unwrap());
        assert!(std::path::Path::new(p).is_absolute());
    }

    #[test]
    fn c029_empty_arrays_are_present() {
        let r = ExportReport::new(vec![item("team", "claude", vec![], vec![])]);
        let (text, v) = json(&r);
        assert_eq!(v["items"][0]["members"], serde_json::json!([]));
        assert_eq!(v["items"][0]["omitted"], serde_json::json!([]));
        assert!(text.contains("\"members\": []") && text.contains("\"omitted\": []"));
    }

    #[test]
    fn c029_empty_report_is_empty_items() {
        let (_, v) = json(&ExportReport::new(vec![]));
        assert_eq!(v, serde_json::json!({"items": []}));
    }

    /// Orchestrator decision: item order is the caller's (plugin, then client
    /// selection order) and `new` preserves it — neither re-sorted by plugin
    /// nor by client.
    #[test]
    fn c029_item_order_is_preserved() {
        let r = ExportReport::new(vec![
            item("zeta", "codex", vec![], vec![]),
            item("zeta", "claude", vec![], vec![]),
            item("alpha", "cursor", vec![], vec![]),
        ]);
        let (_, v) = json(&r);
        let order: Vec<_> = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| format!("{}/{}", i["plugin"].as_str().unwrap(), i["client"].as_str().unwrap()))
            .collect();
        assert_eq!(order, ["zeta/codex", "zeta/claude", "alpha/cursor"]);
    }

    fn pairs(arr: &serde_json::Value) -> Vec<String> {
        arr.as_array()
            .unwrap()
            .iter()
            .map(|m| format!("{} {}", m["kind"].as_str().unwrap(), m["name"].as_str().unwrap()))
            .collect()
    }

    /// C-029: `new` sorts `members` / `omitted` by name within one kind
    /// (bytewise: `B` < `a` < `b`).
    #[test]
    fn c029_new_sorts_by_name_within_kind() {
        let r = ExportReport::new(vec![item(
            "team",
            "claude",
            vec![
                member(ArtifactKind::Skill, "b"),
                member(ArtifactKind::Skill, "a"),
                member(ArtifactKind::Skill, "B"),
            ],
            vec![
                omission(ArtifactKind::Rule, "z", OmitReason::NoFormatSurface),
                omission(ArtifactKind::Rule, "y", OmitReason::NoFormatSurface),
            ],
        )]);
        let (_, v) = json(&r);
        assert_eq!(pairs(&v["items"][0]["members"]), ["skill B", "skill a", "skill b"]);
        assert_eq!(pairs(&v["items"][0]["omitted"]), ["rule y", "rule z"]);
    }

    /// C-029: `(kind, name)` with `kind` as the typed `ArtifactKind` — its
    /// `Ord` is declaration order (skill < rule < agent < bundle < mcp), not
    /// the lowercase string. DESIGN GAP recorded: the record says only
    /// "(kind, name)".
    #[test]
    fn c029_new_sorts_kind_first_by_artifact_kind_order() {
        let r = ExportReport::new(vec![item(
            "team",
            "claude",
            vec![
                member(ArtifactKind::Mcp, "a"),
                member(ArtifactKind::Agent, "a"),
                member(ArtifactKind::Skill, "z"),
            ],
            vec![
                omission(ArtifactKind::Mcp, "a", OmitReason::NotRepresentable),
                omission(ArtifactKind::Agent, "b", OmitReason::ClientDeclined),
                omission(ArtifactKind::Rule, "c", OmitReason::NoFormatSurface),
            ],
        )]);
        let (_, v) = json(&r);
        assert_eq!(pairs(&v["items"][0]["members"]), ["skill z", "agent a", "mcp a"]);
        assert_eq!(pairs(&v["items"][0]["omitted"]), ["rule c", "agent b", "mcp a"]);
    }

    /// C-029 plain: one table `Plugin | Client | Version | Path | Omitted`,
    /// one row per item in item order, `Omitted` = count.
    #[test]
    fn c029_plain_table() {
        let r = ExportReport::new(vec![
            item(
                "team",
                "codex",
                vec![member(ArtifactKind::Skill, "s")],
                vec![
                    omission(ArtifactKind::Agent, "a", OmitReason::NoFormatSurface),
                    omission(ArtifactKind::Rule, "r", OmitReason::NoFormatSurface),
                ],
            ),
            item("team", "claude", vec![member(ArtifactKind::Skill, "s")], vec![]),
        ]);
        let out = plain(&r);
        let rows: Vec<Vec<&str>> = out.lines().map(|l| l.split_whitespace().collect()).collect();
        let codex = abs("team.codex.zip");
        let claude = abs("team.claude.zip");
        assert_eq!(
            rows,
            [
                vec!["Plugin", "Client", "Version", "Path", "Omitted"],
                vec!["team", "codex", "1.2.0+0123456789ab", codex.to_str().unwrap(), "2"],
                vec!["team", "claude", "1.2.0+0123456789ab", claude.to_str().unwrap(), "0"],
            ]
        );
    }

    #[test]
    fn c029_plain_empty_report_is_header_only() {
        assert_eq!(
            plain(&ExportReport::new(vec![])).lines().collect::<Vec<_>>(),
            ["Plugin  Client  Version  Path  Omitted"]
        );
    }
}
