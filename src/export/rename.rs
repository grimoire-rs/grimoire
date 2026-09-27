// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Member rename (`strip_prefix`) and the stale-reference scan over the
//! staged plugin tree (design record C-021, C-022).

use std::collections::HashMap;
use std::io;
use std::path::Path;

use crate::export::export_error::ExportError;
use crate::export::marketplace::RenameRule;
use crate::lock::LockedArtifact;
use crate::lock::grimoire_lock::is_contained_mcp_name;
use crate::oci::ArtifactKind;
use crate::skill::SkillName;

/// One occurrence of a renamed member's old name in a staged file (C-022).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct StaleRef {
    /// `/`-joined path relative to the plugin root.
    pub path: String,
    /// 1-based line number.
    pub line: usize,
    /// The old member name found.
    pub old: String,
}

/// Apply `rule` to `members` of `plugin` (C-021): each member paired with
/// its emitted name. Every emitted name is validated; collisions within a
/// kind are rejected.
pub fn apply(
    plugin: &str,
    members: &[LockedArtifact],
    rule: Option<&RenameRule>,
) -> Result<Vec<(LockedArtifact, String)>, ExportError> {
    let mut out = Vec::with_capacity(members.len());
    for member in members {
        let emitted = rule
            .and_then(|r| member.name.strip_prefix(r.strip_prefix.as_str()))
            .unwrap_or(&member.name);
        if !is_valid_name(member.kind, emitted) {
            return Err(ExportError::RenameInvalid {
                plugin: plugin.to_string(),
                from: member.name.clone(),
                to: emitted.to_string(),
            });
        }
        out.push((member.clone(), emitted.to_string()));
    }

    let mut seen: HashMap<(ArtifactKind, &str), &str> = HashMap::new();
    for (member, emitted) in &out {
        if let Some(first) = seen.insert((member.kind, emitted), &member.name) {
            return Err(ExportError::RenameCollision {
                plugin: plugin.to_string(),
                kind: member.kind,
                name: emitted.clone(),
                members: [first.to_string(), member.name.clone()],
            });
        }
    }
    Ok(out)
}

/// Find whole-token occurrences of each renamed `(old, new)` pair's old name
/// in the files under `root/skills/` and `root/agents/` (C-022).
pub fn scan(root: &Path, renamed: &[(String, String)]) -> std::io::Result<Vec<StaleRef>> {
    let mut hits = Vec::new();
    if renamed.is_empty() {
        return Ok(hits);
    }
    for top in ["skills", "agents"] {
        let dir = root.join(top);
        // An Agent Plugins tree has no `agents/` (C-016): absence is clean.
        match std::fs::symlink_metadata(&dir) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
            Ok(_) => scan_dir(root, &dir, renamed, &mut hits)?,
        }
    }
    hits.sort();
    Ok(hits)
}

/// Recurse into `dir`, recording hits in every regular UTF-8 file.
fn scan_dir(root: &Path, dir: &Path, renamed: &[(String, String)], hits: &mut Vec<StaleRef>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_dir() {
            scan_dir(root, &path, renamed, hits)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let Ok(text) = String::from_utf8(std::fs::read(&path)?) else {
            continue;
        };
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        for (index, line) in text.split('\n').enumerate() {
            for (old, _) in renamed {
                if contains_token(line, old) {
                    hits.push(StaleRef {
                        path: rel.clone(),
                        line: index + 1,
                        old: old.clone(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Whether `line` holds `token` with no name character (`[A-Za-z0-9_-]`)
/// directly before or after it.
fn contains_token(line: &str, token: &str) -> bool {
    let is_name_char = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    line.match_indices(token).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + token.len()..].chars().next();
        !before.is_some_and(is_name_char) && !after.is_some_and(is_name_char)
    })
}

/// C-021's emitted-name rule: skill, rule and agent names are `SkillName`s;
/// an mcp name only has to stay one path component (C-006's containment
/// check).
fn is_valid_name(kind: ArtifactKind, name: &str) -> bool {
    if kind == ArtifactKind::Mcp {
        return is_contained_mcp_name(name);
    }
    SkillName::parse(name).is_ok()
}

#[cfg(test)]
mod tests {
    //! Specification tests written from the design record (C-021, C-022;
    //! S-014, S-015, S-016 unit halves), not from the implementation.

    use std::path::PathBuf;

    use super::*;
    use crate::oci::{ArtifactKind, Digest, Identifier, PinnedIdentifier};

    fn member(name: &str, kind: ArtifactKind) -> LockedArtifact {
        let id = Identifier::new_registry("x", "localhost:5000")
            .clone_with_digest(Digest::Sha256(std::iter::repeat_n('a', 64).collect()));
        LockedArtifact::direct(name.to_string(), kind, PinnedIdentifier::try_from(id).unwrap())
    }

    fn strip(prefix: &str) -> RenameRule {
        RenameRule {
            strip_prefix: prefix.to_string(),
        }
    }

    /// `(lock name, emitted name)` pairs, members unchanged otherwise.
    fn emitted(members: &[LockedArtifact], rule: Option<&RenameRule>) -> Vec<(String, String)> {
        let out = apply("team", members, rule).unwrap();
        for ((m, _), orig) in out.iter().zip(members) {
            assert_eq!(m, orig, "the member itself is carried unchanged");
        }
        out.into_iter().map(|(m, e)| (m.name, e)).collect()
    }

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    // ── C-021 names ──

    #[test]
    fn c021_no_rule_emits_lock_names() {
        let ms = [
            member("team-plan", ArtifactKind::Skill),
            member("reviewer", ArtifactKind::Agent),
            member("srv", ArtifactKind::Mcp),
        ];
        assert_eq!(
            emitted(&ms, None),
            pairs(&[("team-plan", "team-plan"), ("reviewer", "reviewer"), ("srv", "srv")])
        );
    }

    #[test]
    fn c021_s013_strip_prefix_on_every_kind_non_matching_unchanged() {
        let ms = [
            member("team-plan", ArtifactKind::Skill),
            member("team-reviewer", ArtifactKind::Agent),
            member("team-srv", ArtifactKind::Mcp),
            member("team-style", ArtifactKind::Rule),
            member("plan-team", ArtifactKind::Skill),
            member("teams", ArtifactKind::Skill),
        ];
        assert_eq!(
            emitted(&ms, Some(&strip("team-"))),
            pairs(&[
                ("team-plan", "plan"),
                ("team-reviewer", "reviewer"),
                ("team-srv", "srv"),
                ("team-style", "style"),
                ("plan-team", "plan-team"),
                ("teams", "teams"),
            ])
        );
    }

    fn invalid(members: &[LockedArtifact], rule: Option<&RenameRule>) -> (String, String, String) {
        match apply("team", members, rule) {
            Err(ExportError::RenameInvalid { plugin, from, to }) => (plugin, from, to),
            other => panic!("expected RenameInvalid, got {other:?}"),
        }
    }

    fn triple(a: &str, b: &str, c: &str) -> (String, String, String) {
        (a.into(), b.into(), c.into())
    }

    #[test]
    fn c021_s014_empty_remainder_is_rename_invalid() {
        let ms = [member("team-plan", ArtifactKind::Skill)];
        assert_eq!(invalid(&ms, Some(&strip("team-plan"))), triple("team", "team-plan", ""));
    }

    #[test]
    fn c021_s014_leading_hyphen_remainder_is_rename_invalid() {
        let ms = [member("team-plan", ArtifactKind::Skill)];
        assert_eq!(invalid(&ms, Some(&strip("team"))), triple("team", "team-plan", "-plan"));
    }

    #[test]
    fn c021_unrenamed_names_are_checked_too() {
        // With no rule, and with a rule the name does not match.
        let ms = [member("Bad_Name", ArtifactKind::Agent)];
        assert_eq!(invalid(&ms, None), triple("team", "Bad_Name", "Bad_Name"));
        assert_eq!(
            invalid(&ms, Some(&strip("team-"))),
            triple("team", "Bad_Name", "Bad_Name")
        );
        let rule = [member("x..y", ArtifactKind::Rule)];
        assert_eq!(invalid(&rule, None), triple("team", "x..y", "x..y"));
    }

    #[test]
    fn c021_mcp_names_use_containment_not_skill_name() {
        // `My_Server` fails SkillName but passes C-006 containment.
        let ok = [member("team-My_Server", ArtifactKind::Mcp)];
        assert_eq!(
            emitted(&ok, Some(&strip("team-"))),
            pairs(&[("team-My_Server", "My_Server")])
        );
        for (name, prefix, to) in [
            ("team-", "team-", ""),
            ("team..x", "team", "..x"),
            ("teama/b", "team", "a/b"),
            ("teama\\b", "team", "a\\b"),
            ("teama\0b", "team", "a\0b"),
        ] {
            let ms = [member(name, ArtifactKind::Mcp)];
            assert_eq!(invalid(&ms, Some(&strip(prefix))), triple("team", name, to), "{name:?}");
        }
    }

    #[test]
    fn c021_s015_collision_names_both_members() {
        let ms = [
            member("team-plan", ArtifactKind::Skill),
            member("plan", ArtifactKind::Skill),
        ];
        match apply("team", &ms, Some(&strip("team-"))) {
            Err(ExportError::RenameCollision {
                plugin,
                kind,
                name,
                members,
            }) => {
                assert_eq!(
                    (plugin.as_str(), kind, name.as_str()),
                    ("team", ArtifactKind::Skill, "plan")
                );
                let mut both = members.to_vec();
                both.sort();
                assert_eq!(both, ["plan", "team-plan"]);
            }
            other => panic!("expected RenameCollision, got {other:?}"),
        }
    }

    #[test]
    fn c021_equal_emitted_names_across_kinds_do_not_collide() {
        let ms = [
            member("team-plan", ArtifactKind::Skill),
            member("plan", ArtifactKind::Agent),
        ];
        assert_eq!(
            emitted(&ms, Some(&strip("team-"))),
            pairs(&[("team-plan", "plan"), ("plan", "plan")])
        );
    }

    // ── C-022 stale-reference scan (S-016 unit half) ──

    fn put(root: &Path, rel: &str, bytes: &[u8]) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn renamed() -> Vec<(String, String)> {
        vec![("hex-plan".into(), "plan".into()), ("hex-core".into(), "core".into())]
    }

    fn hit(path: &str, line: usize, old: &str) -> StaleRef {
        StaleRef {
            path: path.into(),
            line,
            old: old.into(),
        }
    }

    fn sorted(mut v: Vec<StaleRef>) -> Vec<StaleRef> {
        v.sort();
        v
    }

    const SKILL: &str = "---\n\
name: plan\n\
description: d\n\
---\n\
See ../hex-core/references/x.md for more.\n\
run /hex-plan\n\
use `hex-plan` here\n\
see hex-plan.\n\
skills/hex-plan/SKILL.md\n\
grimoire-hex-plan\n\
hex-planner\n\
hex_plan\n\
hex-plan\n\
grimoire-hex-plan then hex-plan\n\
hex-plan_x\n\
x_hex-plan\n\
hex-plan2\n";

    #[test]
    fn c022_positive_and_negative_token_cases_with_line_numbers() {
        let tmp = tempfile::tempdir().unwrap();
        put(tmp.path(), "skills/plan/SKILL.md", SKILL.as_bytes());
        let got = sorted(scan(tmp.path(), &renamed()).unwrap());
        let f = "skills/plan/SKILL.md";
        assert_eq!(
            got,
            sorted(vec![
                hit(f, 5, "hex-core"),
                hit(f, 6, "hex-plan"),
                hit(f, 7, "hex-plan"),
                hit(f, 8, "hex-plan"),
                hit(f, 9, "hex-plan"),
                // 10–12 negative; 2 (rewritten frontmatter) negative.
                hit(f, 13, "hex-plan"),
                // A non-hit earlier on the line must not hide a later hit.
                hit(f, 14, "hex-plan"),
                // 15–17 negative: `_` and digits are name characters.
            ])
        );
    }

    #[test]
    fn c022_walks_only_skills_and_agents_including_dotfiles() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        let line = b"run /hex-plan\n";
        put(r, "agents/reviewer.md", line);
        put(r, "skills/plan/.notes.md", line);
        put(r, "skills/plan/.hidden/deep/x.md", line);
        // Outside skills/ and agents/: never scanned.
        put(r, ".mcp.json", b"{\"mcpServers\":{\"hex-plan\":{}}}\n");
        put(r, "mcp.json", b"{\"mcpServers\":{\"hex-plan\":{}}}\n");
        put(r, "plugin.json", b"{\"description\":\"hex-plan\"}\n");
        put(r, ".claude-plugin/plugin.json", b"{\"description\":\"hex-plan\"}\n");
        put(r, "other/hex-plan.md", line);
        assert_eq!(
            sorted(scan(r, &renamed()).unwrap()),
            sorted(vec![
                hit("agents/reviewer.md", 1, "hex-plan"),
                hit("skills/plan/.hidden/deep/x.md", 1, "hex-plan"),
                hit("skills/plan/.notes.md", 1, "hex-plan"),
            ])
        );
    }

    #[test]
    fn c022_non_utf8_files_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        put(tmp.path(), "skills/plan/blob.bin", b"\xff\xfe run /hex-plan\n");
        put(tmp.path(), "skills/plan/SKILL.md", b"ok\nrun /hex-plan\n");
        assert_eq!(
            scan(tmp.path(), &renamed()).unwrap(),
            vec![hit("skills/plan/SKILL.md", 2, "hex-plan")]
        );
    }

    #[test]
    fn c022_absent_agents_dir_and_empty_rename_are_clean() {
        // Agent Plugins trees carry no agents/ (C-016): absence is not an error.
        let tmp = tempfile::tempdir().unwrap();
        put(tmp.path(), "skills/plan/SKILL.md", b"clean\n");
        assert_eq!(scan(tmp.path(), &renamed()).unwrap(), vec![]);
        put(tmp.path(), "skills/plan/SKILL.md", b"run /hex-plan\n");
        assert_eq!(scan(tmp.path(), &[]).unwrap(), vec![]);
    }

    #[cfg(unix)]
    fn chmod(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// Whether permission bits are enforced here (not when running as root).
    #[cfg(unix)]
    fn denied(path: &Path) -> bool {
        std::fs::read_dir(path).is_err() && std::fs::read(path).is_err()
    }

    #[cfg(unix)]
    #[test]
    fn c022_unreadable_dir_propagates_io_error() {
        let tmp = tempfile::tempdir().unwrap();
        put(tmp.path(), "skills/plan/SKILL.md", b"clean\n");
        let dir: PathBuf = tmp.path().join("skills/plan");
        chmod(&dir, 0o000);
        let enforced = denied(&dir);
        let result = scan(tmp.path(), &renamed());
        chmod(&dir, 0o755);
        if enforced {
            assert!(result.is_err(), "walk failure must propagate: {result:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn c022_unreadable_file_propagates_io_error() {
        let tmp = tempfile::tempdir().unwrap();
        put(tmp.path(), "agents/reviewer.md", b"clean\n");
        let file: PathBuf = tmp.path().join("agents/reviewer.md");
        chmod(&file, 0o000);
        let enforced = std::fs::read(&file).is_err();
        let result = scan(tmp.path(), &renamed());
        chmod(&file, 0o644);
        if enforced {
            assert!(result.is_err(), "read failure is not a non-UTF-8 skip: {result:?}");
        }
    }
}
