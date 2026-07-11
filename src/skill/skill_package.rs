// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Validate a local skill directory / rule file against the Agent Skills
//! standard and pack it into the exact uncompressed-tar layout the
//! [`crate::install::materializer::DefaultMaterializer`] expects.
//!
//! The pack ↔ install round-trip is a hard contract: `pack_skill_dir`
//! emits entries rooted at `<name>/`, while `pack_rule_file` and
//! `pack_agent_file` emit a single `<name>.md` (the rule variant plus an
//! optional support dir), byte-for-byte what the materializer (and the
//! acceptance harness `make_artifact`) extracts. The tar entries are
//! emitted in sorted path order so the layer digest is deterministic.

use std::path::{Path, PathBuf};

use super::agent_frontmatter::AgentFrontmatter;
use super::rule_frontmatter::{ParsedRule, RuleFrontmatter};
use super::skill_error::{SkillError, SkillErrorKind};
use super::skill_frontmatter::SkillFrontmatter;
use super::skill_name::SkillName;

/// Validate the skill directory at `dir`.
///
/// Checks: `SKILL.md` is present and readable; its frontmatter parses and
/// the required fields are well-formed; the frontmatter `name` equals the
/// directory name.
///
/// # Errors
///
/// [`SkillErrorKind::MissingSkillMd`], [`SkillErrorKind::FrontmatterParse`],
/// [`SkillErrorKind::NameMismatch`], [`SkillErrorKind::NameInvalid`], or
/// [`SkillErrorKind::Io`].
pub fn validate_skill_dir(dir: &Path) -> Result<SkillFrontmatter, SkillError> {
    let skill_md = dir.join("SKILL.md");
    if !skill_md.is_file() {
        return Err(SkillError::new(dir, SkillErrorKind::MissingSkillMd));
    }
    let doc = std::fs::read_to_string(&skill_md).map_err(|e| SkillError::new(&skill_md, SkillErrorKind::Io(e)))?;
    let fm = SkillFrontmatter::parse_doc(&doc, &skill_md)?;

    let dir_name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| {
            SkillError::new(
                dir,
                SkillErrorKind::NameInvalid("skill path has no directory name".to_string()),
            )
        })?;

    // The dir name must itself be a valid skill name and equal the
    // frontmatter name (the Agent Skills standard's directory-equality
    // rule).
    SkillName::parse(&dir_name).map_err(|e| SkillError::new(dir, SkillErrorKind::NameInvalid(e)))?;
    if fm.name.as_str() != dir_name {
        return Err(SkillError::new(
            dir,
            SkillErrorKind::NameMismatch {
                frontmatter: fm.name.to_string(),
                dir: dir_name,
            },
        ));
    }
    Ok(fm)
}

/// Validate the rule file at `file`.
///
/// A rule is any `.md` file; its optional `---paths:---` frontmatter must
/// parse when present. The file name (sans `.md`) must be a valid skill
/// name (rules share the name charset).
///
/// # Errors
///
/// [`SkillErrorKind::Io`], [`SkillErrorKind::NameInvalid`], or
/// [`SkillErrorKind::FrontmatterParse`].
pub fn validate_rule_file(file: &Path) -> Result<RuleFrontmatter, SkillError> {
    let name = rule_name(file)?;
    SkillName::parse(&name).map_err(|e| SkillError::new(file, SkillErrorKind::NameInvalid(e)))?;
    let doc = std::fs::read_to_string(file).map_err(|e| SkillError::new(file, SkillErrorKind::Io(e)))?;
    let ParsedRule { frontmatter, .. } = RuleFrontmatter::parse_doc(&doc, file)?;
    Ok(frontmatter)
}

/// Validate the agent file at `file`.
///
/// An agent is a single `.md` file whose frontmatter is **required** and
/// must carry `name` + `description`; the frontmatter `name` must equal
/// the file stem (the OpenCode filename-as-identity rule, enforced for
/// every client so the identity is consistent).
///
/// # Errors
///
/// [`SkillErrorKind::Io`], [`SkillErrorKind::NameInvalid`],
/// [`SkillErrorKind::NameMismatch`],
/// [`SkillErrorKind::MissingFrontmatter`], or
/// [`SkillErrorKind::FrontmatterParse`].
pub fn validate_agent_file(file: &Path) -> Result<AgentFrontmatter, SkillError> {
    let stem = rule_name(file)?;
    SkillName::parse(&stem).map_err(|e| SkillError::new(file, SkillErrorKind::NameInvalid(e)))?;
    let doc = std::fs::read_to_string(file).map_err(|e| SkillError::new(file, SkillErrorKind::Io(e)))?;
    let parsed = AgentFrontmatter::parse_doc(&doc, file)?;
    if parsed.frontmatter.name.as_str() != stem {
        return Err(SkillError::new(
            file,
            SkillErrorKind::NameMismatch {
                frontmatter: parsed.frontmatter.name.to_string(),
                dir: stem,
            },
        ));
    }
    Ok(parsed.frontmatter)
}

/// The rule's (or agent's) logical name: the file stem of a `.md` file.
fn rule_name(file: &Path) -> Result<String, SkillError> {
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .ok_or_else(|| {
            SkillError::new(
                file,
                SkillErrorKind::NameInvalid("rule path has no file name".to_string()),
            )
        })?;
    Ok(stem)
}

/// Pack the skill directory at `dir` into an uncompressed tar whose
/// entries are rooted at `<name>/`, matching the materializer's expected
/// layout. The whole tree under `dir` is included; entries are emitted in
/// sorted path order for a deterministic digest.
///
/// # Errors
///
/// [`SkillErrorKind::Io`] for a walk/read failure.
pub fn pack_skill_dir(dir: &Path) -> Result<Vec<u8>, SkillError> {
    pack_skill_dir_limited(dir, &PackLimits::DEFAULT)
}

/// [`pack_skill_dir`] with injectable packing bounds (see [`PackLimits`]),
/// so a test can drive the real walk with low caps.
fn pack_skill_dir_limited(dir: &Path, limits: &PackLimits) -> Result<Vec<u8>, SkillError> {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| {
            SkillError::new(
                dir,
                SkillErrorKind::NameInvalid("skill path has no directory name".to_string()),
            )
        })?;

    let mut state = WalkState::default();
    collect_files(dir, dir, &name, &mut state, 0, limits)?;
    let mut files = state.out;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut builder = tar::Builder::new(Vec::new());
    let mut read_bytes: u64 = 0;
    for (entry_path, abs) in &files {
        let bytes = read_capped(abs, limits.byte_limit.saturating_sub(read_bytes))?;
        read_bytes = read_bytes.saturating_add(bytes.len() as u64);
        append_entry(&mut builder, entry_path, &bytes).map_err(|e| SkillError::new(abs, SkillErrorKind::Io(e)))?;
    }
    builder
        .into_inner()
        .map_err(|e| SkillError::new(dir, SkillErrorKind::Io(e)))
}

/// Pack the rule file at `file` into an uncompressed tar.
///
/// Emits the index `<name>.md` entry, plus — when a sibling support
/// directory `<parent>/<name>/` exists beside the index — every file under
/// it rooted at `<name>/<rel>`. Entries are emitted in sorted path order so
/// the layer digest is deterministic; a rule with no support directory
/// packs byte-identically to a single `<name>.md` entry.
///
/// # Errors
///
/// [`SkillErrorKind::Io`] for a read/walk failure.
pub fn pack_rule_file(file: &Path) -> Result<Vec<u8>, SkillError> {
    let name = rule_name(file)?;

    let limits = &PackLimits::DEFAULT;

    // Seed the walk with the index file so the packing bounds account for
    // it alongside any support-dir files (CWE-400/770).
    let index_meta = std::fs::metadata(file).map_err(|e| SkillError::new(file, SkillErrorKind::Io(e)))?;
    let mut state = WalkState {
        out: vec![(format!("{name}.md"), file.to_path_buf())],
        total_bytes: index_meta.len(),
        nodes: 0,
    };
    check_pack_bounds(file, state.total_bytes, state.out.len(), limits)?;

    // The optional sibling support dir shares the index's stem: for
    // `rules/<name>.md` it is `rules/<name>/`. Include it only when it is a
    // real directory; any other sibling (or none) leaves the degenerate
    // single-file case untouched.
    let support = file.with_extension("");
    if support.is_dir() {
        collect_files(&support, &support, &name, &mut state, 0, limits)?;
    }
    let mut files = state.out;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut builder = tar::Builder::new(Vec::new());
    let mut read_bytes: u64 = 0;
    for (entry_path, abs) in &files {
        let bytes = read_capped(abs, limits.byte_limit.saturating_sub(read_bytes))?;
        read_bytes = read_bytes.saturating_add(bytes.len() as u64);
        append_entry(&mut builder, entry_path, &bytes).map_err(|e| SkillError::new(abs, SkillErrorKind::Io(e)))?;
    }
    builder
        .into_inner()
        .map_err(|e| SkillError::new(file, SkillErrorKind::Io(e)))
}

/// Pack the agent file at `file` into an uncompressed tar with exactly one
/// `<name>.md` entry.
///
/// Unlike [`pack_rule_file`], a sibling directory sharing the stem is
/// **not** packed — agents have no support-directory contract (every
/// client reads a standalone agent file). The single stable-header entry
/// makes the layer digest deterministic.
///
/// # Errors
///
/// [`SkillErrorKind::Io`] for a read failure,
/// [`SkillErrorKind::NameInvalid`] for a stem-less path.
pub fn pack_agent_file(file: &Path) -> Result<Vec<u8>, SkillError> {
    let name = rule_name(file)?;
    // Bound the single-file read before pulling it into memory (CWE-400/770).
    let meta = std::fs::metadata(file).map_err(|e| SkillError::new(file, SkillErrorKind::Io(e)))?;
    check_pack_bounds(file, meta.len(), 1, &PackLimits::DEFAULT)?;
    let bytes = read_capped(file, PackLimits::DEFAULT.byte_limit)?;
    let mut builder = tar::Builder::new(Vec::new());
    append_entry(&mut builder, &format!("{name}.md"), &bytes)
        .map_err(|e| SkillError::new(file, SkillErrorKind::Io(e)))?;
    builder
        .into_inner()
        .map_err(|e| SkillError::new(file, SkillErrorKind::Io(e)))
}

/// Read `path` into memory, bounding the ACTUAL read at `remaining` bytes so
/// a file that grew between the pre-read metadata stat and this read — or
/// whose metadata under-reported its length — cannot allocate past the packing
/// byte cap (CWE-400/770; closes the stat-then-read TOCTOU that a metadata-only
/// check leaves open). `remaining` is the unused portion of the cumulative byte
/// budget. The metadata pre-check still fast-fails the common oversized-static
/// -file case without opening; this bound is the read-side backstop.
///
/// # Errors
///
/// [`SkillErrorKind::TooLarge`] when the actual content exceeds `remaining`,
/// or [`SkillErrorKind::Io`] on an open/read failure.
fn read_capped(path: &Path, remaining: u64) -> Result<Vec<u8>, SkillError> {
    use std::io::Read as _;

    let file = std::fs::File::open(path).map_err(|e| SkillError::new(path, SkillErrorKind::Io(e)))?;
    // Read at most one byte past the budget: enough to detect an over-budget
    // file, never enough to allocate unbounded whatever the metadata claimed.
    let mut bytes = Vec::new();
    file.take(remaining.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| SkillError::new(path, SkillErrorKind::Io(e)))?;
    if bytes.len() as u64 > remaining {
        return Err(SkillError::new(
            path,
            SkillErrorKind::TooLarge(format!(
                "file size exceeds the remaining packing budget of {remaining} bytes"
            )),
        ));
    }
    Ok(bytes)
}

/// Append one regular-file entry with a stable header (mode 0o644, no
/// mtime/uid/gid noise) so the produced tar bytes are deterministic.
fn append_entry(builder: &mut tar::Builder<Vec<u8>>, path: &str, bytes: &[u8]) -> std::io::Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    builder.append_data(&mut header, path, bytes)
}

/// Cumulative-byte cap for packing a local source tree. Mirrors
/// `crate::install::installer::INSTALL_LAYER_SIZE_LIMIT` (512 MiB) so a
/// local path source cannot bypass the ceiling that gates registry
/// ingestion (CWE-400/770). Kept as a local mirror rather than importing
/// the install-tier constant to avoid a `skill → install` dependency
/// (install already depends on skill).
const PACK_BYTE_LIMIT: u64 = 512 * 1024 * 1024;

/// Maximum number of files packed from one local source tree — a defence
/// against a pathological deep/wide tree exhausting memory before the byte
/// cap trips (CWE-400/770).
const PACK_FILE_LIMIT: usize = 10_000;

/// Maximum filesystem entries (files **and** directories) visited while
/// walking one local source tree. A directory-heavy tree — many nested or
/// sibling empty directories — packs zero files and zero bytes, so without
/// a node cap it slips past both other bounds; the walk itself is the DoS
/// vector (CWE-400/674). Counted incrementally as each entry is read.
const PACK_NODE_LIMIT: usize = 50_000;

/// Maximum directory-recursion depth. A deeply-nested tree would otherwise
/// recurse [`collect_files`] without bound and exhaust the stack (CWE-674).
const PACK_DEPTH_LIMIT: usize = 64;

/// Injectable packing safety bounds. Production entry points use
/// [`PackLimits::DEFAULT`]; tests drive the real `pack_*` → [`collect_files`]
/// walk with low caps so the byte / entry / depth accounting is exercised
/// end to end, without allocating gigabytes or millions of inodes.
#[derive(Clone, Copy)]
struct PackLimits {
    /// Cumulative byte cap across all packed files.
    byte_limit: u64,
    /// Maximum number of regular files packed.
    file_limit: usize,
    /// Maximum filesystem entries (files + directories) visited.
    node_limit: usize,
    /// Maximum directory-recursion depth.
    depth_limit: usize,
}

impl PackLimits {
    /// The production caps applied by every real packing entry point.
    const DEFAULT: PackLimits = PackLimits {
        byte_limit: PACK_BYTE_LIMIT,
        file_limit: PACK_FILE_LIMIT,
        node_limit: PACK_NODE_LIMIT,
        depth_limit: PACK_DEPTH_LIMIT,
    };
}

/// Reject a local source tree whose cumulative byte size or file count
/// exceeds the packing bounds, before the in-memory tar `Vec` grows
/// unbounded (CWE-400/770). Applies to skill/rule/agent packing; called
/// cumulatively from [`collect_files`] and the `pack_*` entry points as the
/// tree is walked.
///
/// # Errors
///
/// [`SkillErrorKind::TooLarge`] when a bound is exceeded.
fn check_pack_bounds(root: &Path, total_bytes: u64, file_count: usize, limits: &PackLimits) -> Result<(), SkillError> {
    if total_bytes > limits.byte_limit {
        return Err(SkillError::new(
            root,
            SkillErrorKind::TooLarge(format!(
                "cumulative size {total_bytes} bytes exceeds the packing limit of {} bytes",
                limits.byte_limit
            )),
        ));
    }
    if file_count > limits.file_limit {
        return Err(SkillError::new(
            root,
            SkillErrorKind::TooLarge(format!(
                "file count {file_count} exceeds the packing limit of {}",
                limits.file_limit
            )),
        ));
    }
    Ok(())
}

/// Mutable accumulators carried through the recursive [`collect_files`]
/// walk: the collected `(tar_entry_path, absolute_path)` pairs, the running
/// cumulative byte total, and the count of filesystem entries visited (files
/// *and* directories). Grouped so the recursion stays within the argument
/// count clippy allows.
#[derive(Default)]
struct WalkState {
    /// Collected `(tar_entry_path, absolute_path)` pairs, in walk order.
    out: Vec<(String, PathBuf)>,
    /// Cumulative byte size of every collected file (from metadata).
    total_bytes: u64,
    /// Filesystem entries visited so far (files + directories).
    nodes: usize,
}

/// Recursively collect `(tar_entry_path, absolute_path)` for every regular
/// file under `dir`, rooting the entry path at `<root_name>/<rel>`.
///
/// Every bound is enforced **during** the walk, before the in-memory tar
/// `Vec` is built (CWE-400/674/770):
///
/// - `depth` is checked on entry so a deeply-nested tree cannot recurse
///   without bound and exhaust the stack;
/// - `state.nodes` counts each filesystem entry (file *or* directory) as it
///   is read, incrementally — so a pathologically-wide directory cannot
///   materialize its whole entry list, and a directory-only tree that packs
///   no files and no bytes still trips a cap;
/// - `state.total_bytes` accumulates each collected file's size (from
///   metadata, not by reading contents) and `state.out.len()` its count,
///   both fed to [`check_pack_bounds`].
///
/// # Errors
///
/// [`SkillErrorKind::Io`] for a walk failure, or [`SkillErrorKind::TooLarge`]
/// once a depth / node / byte / file-count bound is exceeded.
fn collect_files(
    root: &Path,
    dir: &Path,
    root_name: &str,
    state: &mut WalkState,
    depth: usize,
    limits: &PackLimits,
) -> Result<(), SkillError> {
    if depth > limits.depth_limit {
        return Err(SkillError::new(
            root,
            SkillErrorKind::TooLarge(format!(
                "directory depth {depth} exceeds the packing limit of {}",
                limits.depth_limit
            )),
        ));
    }
    // Read entries lazily, bounding the running node count as each one is
    // read so a single pathologically-wide directory cannot materialize its
    // whole entry list before any check trips. The bounded list is then
    // sorted for a deterministic digest.
    let mut children: Vec<PathBuf> = Vec::new();
    let read_dir = std::fs::read_dir(dir).map_err(|e| SkillError::new(dir, SkillErrorKind::Io(e)))?;
    for entry in read_dir {
        let entry = entry.map_err(|e| SkillError::new(dir, SkillErrorKind::Io(e)))?;
        state.nodes += 1;
        if state.nodes > limits.node_limit {
            return Err(SkillError::new(
                root,
                SkillErrorKind::TooLarge(format!(
                    "entry count {} exceeds the packing limit of {}",
                    state.nodes, limits.node_limit
                )),
            ));
        }
        children.push(entry.path());
    }
    children.sort();
    for path in children {
        let meta = std::fs::symlink_metadata(&path).map_err(|e| SkillError::new(&path, SkillErrorKind::Io(e)))?;
        if meta.is_dir() {
            collect_files(root, &path, root_name, state, depth + 1, limits)?;
        } else if meta.is_file() {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let rel_str: Vec<String> = rel
                .components()
                .filter_map(|c| match c {
                    std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect();
            let entry = format!("{root_name}/{}", rel_str.join("/"));
            state.out.push((entry, path));
            state.total_bytes = state.total_bytes.saturating_add(meta.len());
            check_pack_bounds(root, state.total_bytes, state.out.len(), limits)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::materializer::{ArtifactMaterializer, DefaultMaterializer};
    use crate::oci::ArtifactKind;

    fn write(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn validate_skill_dir_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("code-review");
        write(
            &dir.join("SKILL.md"),
            "---\nname: code-review\ndescription: Review code.\n---\n# Body\n",
        );
        let fm = validate_skill_dir(&dir).expect("valid skill");
        assert_eq!(fm.name.as_str(), "code-review");
    }

    #[test]
    fn validate_skill_dir_missing_skill_md() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("code-review");
        std::fs::create_dir_all(&dir).unwrap();
        let err = validate_skill_dir(&dir).expect_err("no SKILL.md");
        assert!(matches!(err.kind, SkillErrorKind::MissingSkillMd));
    }

    #[test]
    fn validate_skill_dir_name_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("code-review");
        write(&dir.join("SKILL.md"), "---\nname: other-name\ndescription: d\n---\n");
        let err = validate_skill_dir(&dir).expect_err("name mismatch");
        assert!(matches!(err.kind, SkillErrorKind::NameMismatch { .. }));
    }

    #[test]
    fn validate_skill_dir_missing_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("s");
        write(&dir.join("SKILL.md"), "no frontmatter at all\n");
        let err = validate_skill_dir(&dir).expect_err("no frontmatter");
        assert!(matches!(err.kind, SkillErrorKind::MissingFrontmatter));
    }

    #[test]
    fn validate_rule_file_ok_and_bad() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("rust-style.md");
        write(&f, "---\npaths: [\"**/*.rs\"]\n---\n# Rust\n");
        let fm = validate_rule_file(&f).expect("valid rule");
        assert_eq!(fm.paths, vec!["**/*.rs"]);

        let bad = tmp.path().join("Bad_Name.md");
        write(&bad, "# x\n");
        assert!(matches!(
            validate_rule_file(&bad).expect_err("bad name").kind,
            SkillErrorKind::NameInvalid(_)
        ));
    }

    #[test]
    fn pack_skill_round_trips_through_materializer() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("code-review");
        write(
            &dir.join("SKILL.md"),
            "---\nname: code-review\ndescription: d\n---\n# Body\n",
        );
        write(&dir.join("scripts/run.sh"), "echo hi\n");

        let tar = pack_skill_dir(&dir).expect("pack");
        let dest = tmp.path().join("out");
        let written = DefaultMaterializer
            .materialize(ArtifactKind::Skill, "code-review", &tar, &dest)
            .expect("materialize");
        assert_eq!(
            written,
            vec![
                PathBuf::from("code-review/SKILL.md"),
                PathBuf::from("code-review/scripts/run.sh"),
            ]
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("code-review/SKILL.md")).unwrap(),
            "---\nname: code-review\ndescription: d\n---\n# Body\n"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("code-review/scripts/run.sh")).unwrap(),
            "echo hi\n"
        );
    }

    #[test]
    fn pack_rule_round_trips_through_materializer() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("rust-style.md");
        write(&f, "---\npaths: [\"**/*.rs\"]\n---\n# Rust Style\n");
        let tar = pack_rule_file(&f).expect("pack");
        let dest = tmp.path().join("out");
        let written = DefaultMaterializer
            .materialize(ArtifactKind::Rule, "rust-style", &tar, &dest)
            .expect("materialize");
        assert_eq!(written, vec![PathBuf::from("rust-style.md")]);
        assert_eq!(
            std::fs::read_to_string(dest.join("rust-style.md")).unwrap(),
            "---\npaths: [\"**/*.rs\"]\n---\n# Rust Style\n"
        );
    }

    #[test]
    fn pack_rule_with_support_dir_round_trips_index_and_files() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("rules/my-rule.md");
        write(
            &f,
            "---\npaths: [\"**/*.rs\"]\n---\n# index\nsee ./my-rule/examples.md\n",
        );
        // Sibling support dir sharing the index stem.
        write(&tmp.path().join("rules/my-rule/examples.md"), "# examples\n");
        write(&tmp.path().join("rules/my-rule/schema.json"), "{}\n");

        let tar = pack_rule_file(&f).expect("pack");
        let dest = tmp.path().join("out");
        let written = DefaultMaterializer
            .materialize(ArtifactKind::Rule, "my-rule", &tar, &dest)
            .expect("materialize");
        // The materializer returns `written` sorted as `PathBuf`
        // (component-wise), so support files precede the index file.
        assert_eq!(
            written,
            vec![
                PathBuf::from("my-rule/examples.md"),
                PathBuf::from("my-rule/schema.json"),
                PathBuf::from("my-rule.md"),
            ]
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("my-rule/examples.md")).unwrap(),
            "# examples\n"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("my-rule/schema.json")).unwrap(),
            "{}\n"
        );
    }

    #[test]
    fn pack_rule_without_support_dir_is_single_entry() {
        // The degenerate case must still pack to exactly one `<name>.md`
        // entry — no behavior change for plain single-file rules.
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("rust-style.md");
        write(&f, "---\npaths: [\"**/*.rs\"]\n---\n# Rust Style\n");
        let tar = pack_rule_file(&f).expect("pack");
        let dest = tmp.path().join("out");
        let written = DefaultMaterializer
            .materialize(ArtifactKind::Rule, "rust-style", &tar, &dest)
            .expect("materialize");
        assert_eq!(written, vec![PathBuf::from("rust-style.md")]);
    }

    #[test]
    fn pack_rule_with_support_dir_is_deterministic() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("my-rule.md");
        write(&f, "# index\n");
        write(&tmp.path().join("my-rule/a.md"), "a\n");
        write(&tmp.path().join("my-rule/nested/b.json"), "{}\n");
        let first = pack_rule_file(&f).unwrap();
        let second = pack_rule_file(&f).unwrap();
        assert_eq!(first, second, "multi-file rule pack must be byte-stable");
    }

    #[test]
    fn validate_agent_file_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("code-reviewer.md");
        write(
            &f,
            "---\nname: code-reviewer\ndescription: Reviews diffs.\n---\nYou review code.\n",
        );
        let fm = validate_agent_file(&f).expect("valid agent");
        assert_eq!(fm.name.as_str(), "code-reviewer");
        assert_eq!(fm.description.as_str(), "Reviews diffs.");
    }

    #[test]
    fn validate_agent_file_name_stem_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("code-reviewer.md");
        write(&f, "---\nname: other-name\ndescription: d\n---\nbody\n");
        let err = validate_agent_file(&f).expect_err("stem mismatch");
        assert!(matches!(err.kind, SkillErrorKind::NameMismatch { .. }));
    }

    #[test]
    fn validate_agent_file_requires_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("agent.md");
        write(&f, "no frontmatter at all\n");
        let err = validate_agent_file(&f).expect_err("agent frontmatter is required");
        assert!(matches!(err.kind, SkillErrorKind::MissingFrontmatter));
    }

    #[test]
    fn validate_agent_file_bad_stem() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("Bad_Name.md");
        write(&f, "---\nname: Bad_Name\ndescription: d\n---\n");
        assert!(matches!(
            validate_agent_file(&f).expect_err("bad name").kind,
            SkillErrorKind::NameInvalid(_)
        ));
    }

    #[test]
    fn pack_agent_round_trips_through_materializer() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("code-reviewer.md");
        let doc = "---\nname: code-reviewer\ndescription: d\n---\nYou review code.\n";
        write(&f, doc);
        let tar = pack_agent_file(&f).expect("pack");
        let dest = tmp.path().join("out");
        let written = DefaultMaterializer
            .materialize(ArtifactKind::Agent, "code-reviewer", &tar, &dest)
            .expect("materialize");
        assert_eq!(written, vec![PathBuf::from("code-reviewer.md")]);
        assert_eq!(std::fs::read_to_string(dest.join("code-reviewer.md")).unwrap(), doc);
    }

    #[test]
    fn pack_agent_ignores_sibling_dir_and_is_deterministic() {
        // Agents have no support-directory contract: a sibling dir sharing
        // the stem must NOT be packed (unlike rules).
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("my-agent.md");
        write(&f, "---\nname: my-agent\ndescription: d\n---\nbody\n");
        write(&tmp.path().join("my-agent/extra.md"), "# ignored\n");

        let first = pack_agent_file(&f).unwrap();
        let second = pack_agent_file(&f).unwrap();
        assert_eq!(first, second, "agent pack must be byte-stable");

        let dest = tmp.path().join("out");
        let written = DefaultMaterializer
            .materialize(ArtifactKind::Agent, "my-agent", &first, &dest)
            .expect("materialize");
        assert_eq!(written, vec![PathBuf::from("my-agent.md")], "single entry only");
    }

    #[test]
    fn pack_is_deterministic() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("s");
        write(&dir.join("SKILL.md"), "---\nname: s\ndescription: d\n---\n");
        write(&dir.join("a/one.txt"), "1");
        write(&dir.join("b/two.txt"), "2");
        let first = pack_skill_dir(&dir).unwrap();
        let second = pack_skill_dir(&dir).unwrap();
        assert_eq!(first, second, "pack must be byte-stable");
    }

    // ── F3: bounded packing ─────────────────────────────────────────────

    /// Contract test (design record F3): a local source tree whose file
    /// count exceeds `PACK_FILE_LIMIT` must fail packing with
    /// `SkillErrorKind::TooLarge`, before the in-memory tar grows
    /// unbounded (CWE-400/770). Uses only tiny (1-byte) files so the test
    /// stays fast — the file-COUNT cap, not the byte cap, is what trips.
    ///
    /// STUB: currently FAILS — `collect_files`/`pack_skill_dir` never call
    /// `check_pack_bounds`, so packing this oversized tree still succeeds.
    #[test]
    fn pack_skill_dir_rejects_file_count_over_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("big-skill");
        write(&dir.join("SKILL.md"), "---\nname: big-skill\ndescription: d\n---\n");
        let many = dir.join("many");
        std::fs::create_dir_all(&many).unwrap();
        // SKILL.md (1) + PACK_FILE_LIMIT more files pushes the count to
        // PACK_FILE_LIMIT + 1, one over the cap.
        for i in 0..PACK_FILE_LIMIT {
            std::fs::write(many.join(format!("f{i}.txt")), b"x").unwrap();
        }

        let err = pack_skill_dir(&dir).expect_err("file count over the packing cap must be rejected");
        assert!(
            matches!(err.kind, SkillErrorKind::TooLarge(_)),
            "expected TooLarge, got {:?}",
            err.kind
        );
    }

    /// Direct-call regression lock: `check_pack_bounds`'s own comparison
    /// logic is already correct (only its call site is missing) — it
    /// rejects a byte count over `PACK_BYTE_LIMIT` without needing to
    /// allocate anywhere near 512 MiB on disk, since the function takes
    /// the cumulative size as a plain `u64` rather than re-deriving it
    /// from the filesystem.
    #[test]
    fn check_pack_bounds_rejects_over_byte_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let err = check_pack_bounds(tmp.path(), PACK_BYTE_LIMIT + 1, 1, &PackLimits::DEFAULT)
            .expect_err("byte cap must be rejected");
        assert!(matches!(err.kind, SkillErrorKind::TooLarge(_)));
    }

    /// Direct-call regression lock: the file-count arm of `check_pack_bounds`.
    #[test]
    fn check_pack_bounds_rejects_over_file_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let err = check_pack_bounds(tmp.path(), 1, PACK_FILE_LIMIT + 1, &PackLimits::DEFAULT)
            .expect_err("file cap must be rejected");
        assert!(matches!(err.kind, SkillErrorKind::TooLarge(_)));
    }

    /// Direct-call regression lock: exactly-at-cap is within bounds.
    #[test]
    fn check_pack_bounds_allows_within_caps() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(check_pack_bounds(tmp.path(), PACK_BYTE_LIMIT, PACK_FILE_LIMIT, &PackLimits::DEFAULT).is_ok());
    }

    /// F3 (item 3): the cumulative-byte accounting must be exercised through
    /// the real `pack_skill_dir` → `collect_files` walk, not only via a
    /// direct `check_pack_bounds` call. A handful of small files whose
    /// combined size steps past a low injected byte cap must fail with
    /// `TooLarge` — the cap trips only on the THIRD file, so reverting the
    /// `total_bytes` accumulation in `collect_files` (leaving it at 0)
    /// breaks this test.
    #[test]
    fn pack_skill_dir_rejects_byte_total_over_low_cap_through_walk() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("s");
        // Three 10-byte files → cumulative 30 bytes; no single file exceeds
        // the 25-byte cap, so only the accumulated total can trip it.
        write(&dir.join("SKILL.md"), "0123456789");
        write(&dir.join("a.txt"), "0123456789");
        write(&dir.join("b.txt"), "0123456789");
        let limits = PackLimits {
            byte_limit: 25,
            ..PackLimits::DEFAULT
        };
        let err = pack_skill_dir_limited(&dir, &limits).expect_err("cumulative bytes over the cap must be rejected");
        assert!(
            matches!(err.kind, SkillErrorKind::TooLarge(_)),
            "expected TooLarge, got {:?}",
            err.kind
        );
    }

    /// F3 (item 2): a deeply-nested directory-only tree — no files, no bytes
    /// — must trip the recursion-depth guard with `TooLarge`, not recurse
    /// unbounded and exhaust the stack (CWE-674). Uses empty dirs so nothing
    /// is allocated; drives the production `PACK_DEPTH_LIMIT`.
    #[test]
    fn pack_skill_dir_rejects_dir_tree_over_depth_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("deep");
        let mut nested = dir.clone();
        // Nest past the depth cap so the walk recurses beyond it.
        for i in 0..(PACK_DEPTH_LIMIT + 2) {
            nested = nested.join(format!("d{i}"));
        }
        std::fs::create_dir_all(&nested).unwrap();

        let err = pack_skill_dir(&dir).expect_err("a dir tree past the depth cap must be rejected");
        assert!(
            matches!(err.kind, SkillErrorKind::TooLarge(_)),
            "expected TooLarge, got {:?}",
            err.kind
        );
    }

    /// F3 (item 2): a directory-heavy tree that stays under the byte and
    /// file caps (empty dirs pack nothing) must still trip the node/entry
    /// cap — the walk itself is the DoS vector (CWE-400/674). Driven with a
    /// low injected node cap so the test needs only a handful of dirs, not
    /// tens of thousands.
    #[test]
    fn pack_skill_dir_rejects_wide_dir_tree_over_node_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("wide");
        std::fs::create_dir_all(&dir).unwrap();
        // Ten sibling EMPTY dirs — zero files, zero bytes — exceed a node
        // cap of 5, proving the entry counter (not just file count / bytes)
        // bounds the walk.
        for i in 0..10 {
            std::fs::create_dir_all(dir.join(format!("d{i}"))).unwrap();
        }
        let limits = PackLimits {
            node_limit: 5,
            ..PackLimits::DEFAULT
        };
        let err =
            pack_skill_dir_limited(&dir, &limits).expect_err("a wide dir tree over the node cap must be rejected");
        assert!(
            matches!(err.kind, SkillErrorKind::TooLarge(_)),
            "expected TooLarge, got {:?}",
            err.kind
        );
    }

    /// F3 (read-side TOCTOU): `read_capped` bounds the ACTUAL read by the
    /// remaining byte budget, so a file whose content exceeds the budget is
    /// rejected with `TooLarge` regardless of what its metadata reported —
    /// closing the stat-then-read allocation gap (CWE-400/770) that a
    /// metadata-only check leaves open when a file grows or under-reports.
    /// At-budget content still reads back byte-for-byte, so packed output is
    /// unchanged for within-budget files.
    #[test]
    fn read_capped_bounds_actual_read_by_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("blob.bin");
        std::fs::write(&f, vec![b'x'; 100]).unwrap();

        // Budget below the file size: rejected. The read stops one byte past
        // the budget, so the whole file is never allocated.
        let err = read_capped(&f, 99).expect_err("content over the budget must be rejected");
        assert!(
            matches!(err.kind, SkillErrorKind::TooLarge(_)),
            "expected TooLarge, got {:?}",
            err.kind
        );

        // Exactly-at-budget: within bounds, exact bytes returned.
        let bytes = read_capped(&f, 100).expect("at-budget read");
        assert_eq!(bytes, vec![b'x'; 100]);
    }

    /// F3 (read-side backstop, item 3): a single file whose ACTUAL content
    /// exceeds a low injected byte cap must be rejected with `TooLarge`
    /// through the real `pack_skill_dir` → read path, not read into an
    /// unbounded `Vec`. The metadata pre-check fast-fails first here (accurate
    /// metadata), and `read_capped` is the read-side guard proven in isolation
    /// by `read_capped_bounds_actual_read_by_budget` — together they cover the
    /// stat-then-read TOCTOU (CWE-400/770).
    #[test]
    fn pack_skill_dir_rejects_oversized_file_through_walk() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("s");
        // One 40-byte file, one 30-byte SKILL.md → the single data file alone
        // exceeds the 20-byte cap.
        write(&dir.join("SKILL.md"), "---\nname: s\ndescription: d\n---\n");
        std::fs::write(dir.join("big.bin"), vec![b'x'; 40]).unwrap();
        let limits = PackLimits {
            byte_limit: 20,
            ..PackLimits::DEFAULT
        };
        let err = pack_skill_dir_limited(&dir, &limits).expect_err("an oversized file must be rejected");
        assert!(
            matches!(err.kind, SkillErrorKind::TooLarge(_)),
            "expected TooLarge, got {:?}",
            err.kind
        );
    }

    // ── F4: symlink-skip regression coverage ────────────────────────────

    /// Contract test (design record F4): a symlinked file AND a symlinked
    /// subdirectory under a skill dir must be absent from the packed tar —
    /// the sole barrier against exfiltrating a victim's secrets via a
    /// symlink in a cloned repo (CWE-59). Pins the existing (correct)
    /// `collect_files` behavior so a future "fix" cannot silently remove it.
    #[test]
    #[cfg(unix)]
    fn pack_skill_dir_skips_symlinked_file_and_dir() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        write(&outside.join("secret.txt"), "TOP SECRET");

        let dir = tmp.path().join("code-review");
        write(
            &dir.join("SKILL.md"),
            "---\nname: code-review\ndescription: d\n---\n# Body\n",
        );
        // A symlinked FILE pointing outside the tree.
        symlink(outside.join("secret.txt"), dir.join("leak.txt")).unwrap();
        // A symlinked SUBDIRECTORY pointing outside the tree.
        symlink(&outside, dir.join("linked-dir")).unwrap();

        let tar = pack_skill_dir(&dir).expect("pack succeeds, silently skipping the symlinks");
        let mut archive = tar::Archive::new(tar.as_slice());
        let names: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect();

        assert_eq!(names, vec!["code-review/SKILL.md".to_string()]);
        assert!(!names.iter().any(|n| n.contains("leak")));
        assert!(!names.iter().any(|n| n.contains("linked-dir")));
    }
}
