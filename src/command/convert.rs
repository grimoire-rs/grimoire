// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim convert` — rewrite a Claude-authored skill, rule or agent into the portable form.

use std::path::{Path, PathBuf};

use clap::Args;

use crate::api::convert_report::{BodyToken, ConvertReport};
use crate::cli::exit_code::ExitCode;
use crate::command::command_error::CommandError;
use crate::context::Context;
use crate::export::export_error::ExportError;
use crate::install::vendor_claude::CLAUDE_BODY_TOKENS;
use crate::install::vendor_inverse::{ConvertError, Inverted, invert};
use crate::oci::ArtifactKind;
use crate::skill::SkillName;
use crate::skill::skill_error::{SkillError, SkillErrorKind};

/// `grim convert` arguments.
#[derive(Debug, Args)]
pub struct ConvertArgs {
    /// A skill directory (holds `SKILL.md`) or a rule or agent `.md` file.
    pub path: PathBuf,

    /// The dialect the input is authored in.
    #[arg(long, value_parser = ["claude"])]
    pub from: String,

    /// Output directory: absent or empty.
    #[arg(long)]
    pub out: PathBuf,

    /// Force the artifact kind instead of auto-detecting it.
    #[arg(long, value_parser = ["skill", "rule", "agent"])]
    pub kind: Option<String>,
}

/// Run `grim convert`.
///
/// # Errors
///
/// Usage errors surface as 64, unparseable input or an occupied output as
/// 65, I/O as 74, an absent input as 79.
pub async fn run(_ctx: &Context, args: &ConvertArgs) -> anyhow::Result<(ConvertReport, ExitCode)> {
    convert(args).map(|r| (r, ExitCode::Success))
}

fn convert(args: &ConvertArgs) -> anyhow::Result<ConvertReport> {
    if !args.path.exists() {
        return Err(wrap(CommandError::ConvertInputNotFound {
            path: args.path.clone(),
        }));
    }
    let kind = resolve_kind(&args.path, args.kind.as_deref())?;

    let files = match kind {
        ArtifactKind::Skill => collect_skill_files(&args.path)?,
        _ => vec![PathBuf::from(file_name(&args.path)?)],
    };
    let index = if kind == ArtifactKind::Skill {
        args.path.join("SKILL.md")
    } else {
        args.path.clone()
    };
    let doc = read_text(&index)?;
    let (rewritten, inverted) = convert_doc(kind, &doc, &index)?;

    let name = match kind {
        ArtifactKind::Rule => stem(&args.path)?,
        _ => authored_name(&inverted.frontmatter, &index)?,
    };
    SkillName::parse(&name).map_err(|e| name_invalid(&index, e))?;

    let mut warnings = Vec::new();
    for rel in files.iter().filter(|r| r.extension().is_some_and(|e| e == "md")) {
        let src = if kind == ArtifactKind::Skill {
            args.path.join(rel)
        } else {
            args.path.clone()
        };
        let text = if src == index { doc.clone() } else { read_text(&src)? };
        warnings.extend(scan_body_tokens(&rel.to_string_lossy().replace('\\', "/"), &text));
    }

    ensure_out_usable(&args.out)?;
    let output = if kind == ArtifactKind::Skill {
        args.out.join(&name)
    } else {
        args.out.join(format!("{name}.md"))
    };
    write_output(kind, &args.path, &files, &output, &rewritten)?;

    let report = ConvertReport {
        kind: kind.to_string(),
        name,
        from: args.from.clone(),
        input: absolute(&args.path),
        output: absolute(&output),
        moved: inverted.moved,
        dropped: inverted.dropped,
        warnings,
    };
    Ok(report)
}

/// Route an error through the top-level type the exit-code classifier reads.
fn wrap(e: impl Into<crate::error::Error>) -> anyhow::Error {
    let e: crate::error::Error = e.into();
    anyhow::Error::new(e)
}

fn name_invalid(path: &Path, why: String) -> anyhow::Error {
    wrap(SkillError::new(path, SkillErrorKind::NameInvalid(why)))
}

fn usage(msg: impl Into<String>) -> anyhow::Error {
    wrap(ExportError::Usage(msg.into()))
}

fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

fn io_err(path: &Path, e: std::io::Error) -> anyhow::Error {
    wrap(SkillError::new(path, SkillErrorKind::Io(e)))
}

fn invalid(path: &Path, msg: impl Into<String>) -> anyhow::Error {
    wrap(SkillError::new(path, SkillErrorKind::ValidationFailed(msg.into())))
}

fn file_name(p: &Path) -> anyhow::Result<String> {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| usage(format!("{} has no file name", p.display())))
}

fn stem(p: &Path) -> anyhow::Result<String> {
    p.file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| usage(format!("{} has no file name", p.display())))
}

fn read_text(p: &Path) -> anyhow::Result<String> {
    std::fs::read_to_string(p).map_err(|e| io_err(p, e))
}

/// Infer the kind as `grim build` does, then hold it to the three convertible kinds.
fn resolve_kind(path: &Path, forced: Option<&str>) -> anyhow::Result<ArtifactKind> {
    let is_md = path.is_file() && path.extension().is_some_and(|e| e == "md");
    let is_skill_dir = path.is_dir() && path.join("SKILL.md").is_file();
    let kind = match forced {
        Some(k) => ArtifactKind::from_kind_str(k).ok_or_else(|| usage(format!("unknown --kind '{k}'")))?,
        None => super::build::detect_kind(path, None).map_err(|_| {
            usage(format!(
                "{} is neither a skill directory (SKILL.md) nor a .md file",
                path.display()
            ))
        })?,
    };
    let fits = match kind {
        ArtifactKind::Skill => is_skill_dir,
        ArtifactKind::Rule | ArtifactKind::Agent => is_md,
        _ => false,
    };
    if fits {
        Ok(kind)
    } else {
        let shown = path.display();
        let why = if is_skill_dir {
            format!("{shown} is a skill directory (holds SKILL.md); drop --kind or pass --kind skill")
        } else if is_md {
            format!("{shown} is a .md file; pass --kind rule or --kind agent")
        } else {
            format!("{shown} cannot be converted as {kind}; it is neither a skill directory nor a .md file")
        };
        Err(usage(why))
    }
}

/// Every file under the skill directory, sorted; a symlink or special file refuses the run.
fn collect_skill_files(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
        let mut entries = std::fs::read_dir(dir)
            .and_then(|it| it.collect::<Result<Vec<_>, _>>())
            .map_err(|e| io_err(dir, e))?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let ty = entry.file_type().map_err(|e| io_err(&path, e))?;
            if ty.is_symlink() {
                return Err(invalid(&path, "symlinks are not followed; remove the link"));
            } else if ty.is_dir() {
                walk(root, &path, out)?;
            } else if ty.is_file() {
                out.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
            } else {
                return Err(invalid(&path, "not a regular file"));
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(root, root, &mut files)?;
    Ok(files)
}

/// `--out` must be absent or an empty directory.
fn ensure_out_usable(out: &Path) -> anyhow::Result<()> {
    if !out.exists() {
        return Ok(());
    }
    let empty = out.is_dir() && std::fs::read_dir(out).map_err(|e| io_err(out, e))?.next().is_none();
    if empty {
        Ok(())
    } else {
        Err(wrap(CommandError::ConvertOutputOccupied {
            path: out.to_path_buf(),
        }))
    }
}

/// Split `doc` into `(frontmatter, body)` without touching a byte of either.
///
/// `Ok(None)` is a document with no opening `---`; `Err` an opening with no close.
fn split_doc(doc: &str) -> Result<Option<(&str, &str)>, ()> {
    let mut offset = 0;
    let mut fm_start = None;
    for line in doc.split_inclusive('\n') {
        let end = offset + line.len();
        match fm_start {
            None if line.trim().is_empty() => {}
            None if line.trim() == "---" => fm_start = Some(end),
            None => return Ok(None),
            Some(start) if line.trim() == "---" => return Ok(Some((&doc[start..offset], &doc[end..]))),
            Some(_) => {}
        }
        offset = end;
    }
    if fm_start.is_some() { Err(()) } else { Ok(None) }
}

/// The converted document and the inversion's report.
fn convert_doc(kind: ArtifactKind, doc: &str, path: &Path) -> anyhow::Result<(String, Inverted)> {
    let unreadable = |e: ConvertError| invalid(path, e.to_string());
    match split_doc(doc) {
        Ok(Some((fm, body))) => {
            let inverted = invert(kind, fm).map_err(unreadable)?;
            let out = format!("---\n{}---\n{body}", inverted.frontmatter);
            Ok((out, inverted))
        }
        Ok(None) if kind == ArtifactKind::Rule => Ok((
            doc.to_string(),
            Inverted {
                frontmatter: String::new(),
                moved: vec![],
                dropped: vec![],
            },
        )),
        Ok(None) => Err(wrap(SkillError::new(path, SkillErrorKind::MissingFrontmatter))),
        Err(()) => Err(invalid(path, "frontmatter has no closing '---'")),
    }
}

fn authored_name(frontmatter: &str, path: &Path) -> anyhow::Result<String> {
    let doc: serde_yaml::Value = serde_yaml::from_str(frontmatter).unwrap_or(serde_yaml::Value::Null);
    match doc.get("name").and_then(serde_yaml::Value::as_str) {
        Some(n) => Ok(n.to_string()),
        None => Err(name_invalid(path, "frontmatter has no string `name`".into())),
    }
}

/// Every Claude-only token in `text`, in line then column order, 1-based lines.
fn scan_body_tokens(file: &str, text: &str) -> Vec<BodyToken> {
    let mut found = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let mut hits: Vec<(usize, &str)> = CLAUDE_BODY_TOKENS
            .iter()
            .flat_map(|t| line.match_indices(t).map(move |(col, _)| (col, *t)))
            .collect();
        hits.sort_by_key(|(col, _)| *col);
        found.extend(hits.into_iter().map(|(_, token)| BodyToken {
            file: file.to_string(),
            line: i + 1,
            token: token.to_string(),
        }));
    }
    found
}

/// Write the output. Every destination is `output` joined with a name read
/// from a directory listing (one component, never `..`), so nothing leaves `--out`.
fn write_output(
    kind: ArtifactKind,
    src: &Path,
    files: &[PathBuf],
    output: &Path,
    rewritten: &str,
) -> anyhow::Result<()> {
    let parent = output.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
    if kind != ArtifactKind::Skill {
        return std::fs::write(output, rewritten).map_err(|e| io_err(output, e));
    }
    for rel in files {
        let dest = output.join(rel);
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir).map_err(|e| io_err(dir, e))?;
        }
        if rel == Path::new("SKILL.md") {
            std::fs::write(&dest, rewritten).map_err(|e| io_err(&dest, e))?;
        } else {
            std::fs::copy(src.join(rel), &dest).map_err(|e| io_err(&dest, e))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok(file: &str, line: usize, token: &str) -> BodyToken {
        BodyToken {
            file: file.into(),
            line,
            token: token.into(),
        }
    }

    #[test]
    fn scanner_reports_each_token_with_one_based_lines() {
        let text = "plain\nrun ${CLAUDE_PLUGIN_ROOT}/x and $CLAUDE_PROJECT_DIR\n\n$ARGUMENTS\n";
        assert_eq!(
            scan_body_tokens("SKILL.md", text),
            vec![
                tok("SKILL.md", 2, "${CLAUDE_PLUGIN_ROOT}"),
                tok("SKILL.md", 2, "$CLAUDE_PROJECT_DIR"),
                tok("SKILL.md", 4, "$ARGUMENTS"),
            ]
        );
    }

    #[test]
    fn braced_token_does_not_also_report_the_bare_form() {
        let found = scan_body_tokens("a.md", "${CLAUDE_PROJECT_DIR} ${CLAUDE_PLUGIN_ROOT}");
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|t| t.token.starts_with("${")));
    }

    #[test]
    fn split_keeps_body_bytes() {
        let doc = "\n---\nname: a\n---\r\nbody\r\nno newline";
        let (fm, body) = split_doc(doc).unwrap().unwrap();
        assert_eq!(fm, "name: a\n");
        assert_eq!(body, "body\r\nno newline");
        assert_eq!(split_doc("just text").unwrap(), None);
        assert!(split_doc("---\nname: a\n").is_err());
    }

    #[test]
    fn out_must_be_absent_or_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(ensure_out_usable(&tmp.path().join("absent")).is_ok());
        assert!(ensure_out_usable(tmp.path()).is_ok());
        std::fs::write(tmp.path().join("f"), "x").unwrap();
        let err = ensure_out_usable(tmp.path()).unwrap_err();
        let c = crate::error::classify(&err);
        assert_eq!(c.exit, ExitCode::DataError);
        // No reason: `convert` has no --force, so `forceable` must stay absent.
        assert_eq!(c.reason, None);
    }

    #[test]
    fn output_paths_follow_the_kind() {
        let tmp = tempfile::tempdir().unwrap();
        let skill = tmp.path().join("in/pdf");
        std::fs::create_dir_all(skill.join("ref")).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: pdf\ndescription: d\nversion: 1\n---\nhi $ARGUMENTS\n",
        )
        .unwrap();
        std::fs::write(skill.join("ref/data.bin"), [0u8, 255, 7]).unwrap();
        let rule = tmp.path().join("in/style.md");
        std::fs::write(&rule, "no frontmatter\n").unwrap();

        let run = |path: &Path, out: &Path, kind: Option<&str>| {
            let args = ConvertArgs {
                path: path.to_path_buf(),
                from: "claude".into(),
                out: out.to_path_buf(),
                kind: kind.map(str::to_string),
            };
            convert(&args)
        };

        let r = run(&skill, &tmp.path().join("o1"), None).unwrap();
        assert!(tmp.path().join("o1/pdf/SKILL.md").is_file());
        assert_eq!(
            std::fs::read(tmp.path().join("o1/pdf/ref/data.bin")).unwrap(),
            [0u8, 255, 7]
        );
        assert_eq!(r.dropped.len(), 1);
        assert_eq!(r.warnings.len(), 1);

        run(&rule, &tmp.path().join("o2"), None).unwrap();
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("o2/style.md")).unwrap(),
            "no frontmatter\n"
        );

        let agent = tmp.path().join("in/reviewer.md");
        std::fs::write(&agent, "---\nname: rev-agent\ndescription: d\n---\nbody").unwrap();
        run(&agent, &tmp.path().join("o3"), Some("agent")).unwrap();
        assert!(tmp.path().join("o3/rev-agent.md").is_file());

        // kind mismatch and absent input
        assert!(run(&rule, &tmp.path().join("o4"), Some("skill")).is_err());
        assert!(run(&tmp.path().join("nope"), &tmp.path().join("o5"), None).is_err());
    }

    #[test]
    fn a_symlink_in_the_skill_dir_is_refused() {
        #[cfg(unix)]
        {
            let tmp = tempfile::tempdir().unwrap();
            let skill = tmp.path().join("s");
            std::fs::create_dir_all(&skill).unwrap();
            std::fs::write(skill.join("SKILL.md"), "x").unwrap();
            std::os::unix::fs::symlink("/etc/passwd", skill.join("link")).unwrap();
            assert!(collect_skill_files(&skill).is_err());
        }
    }
}
