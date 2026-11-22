// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Validate a local skill directory / rule file against the Agent Skills
//! standard and pack it into the exact uncompressed-tar layout the
//! [`crate::install::materializer::DefaultMaterializer`] expects.
//!
//! The pack ↔ install round-trip is a hard contract: `pack_skill_dir`
//! emits entries rooted at `<name>/`, while `pack_rule_file` and
//! `pack_agent_file` emit the single `<name>.md` index — byte-for-byte what the
//! materializer (and the acceptance harness `make_artifact`) extracts. The
//! tar entries are emitted in sorted path order so the layer digest is
//! deterministic.

use std::path::{Path, PathBuf};

use super::agent_frontmatter::AgentFrontmatter;
use super::rule_frontmatter::{ParsedRule, RuleFrontmatter};
use super::skill_error::{SkillError, SkillErrorKind};
use super::skill_frontmatter::SkillFrontmatter;
use super::skill_name::SkillName;
use crate::install::ignore_set::{IgnoreSet, IgnoreSetError};

/// Validate the skill directory at `dir`.
///
/// Checks: `SKILL.md` is present and readable; its frontmatter parses and
/// the required fields are well-formed; the frontmatter `name` equals the
/// directory name.
///
/// A symlinked `SKILL.md` is treated as absent (not read through): packing
/// (`collect_files`, which walks `dir` to build the tar) uses
/// `symlink_metadata` and silently skips a symlinked entry, so a symlinked
/// index would validate against content packing never includes — and, in
/// a cloned repo, that content may sit outside the skill's own tree
/// (CWE-59). Validation must agree with packing's skip, not read through it.
/// This guard is leaf-only: it rejects a symlinked `SKILL.md` file, but
/// does not detect a symlinked skill-directory root or an ancestor
/// directory in `dir`'s path — that vector is an accepted trust-model
/// limitation (a local path source is trusted like a build script).
///
/// # Errors
///
/// [`SkillErrorKind::MissingSkillMd`], [`SkillErrorKind::FrontmatterParse`],
/// [`SkillErrorKind::NameMismatch`], [`SkillErrorKind::NameInvalid`], or
/// [`SkillErrorKind::Io`].
pub fn validate_skill_dir(dir: &Path) -> Result<SkillFrontmatter, SkillError> {
    let skill_md = dir.join("SKILL.md");
    if !skill_md.is_file() || is_symlink(&skill_md) {
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
/// A symlinked `file` is rejected rather than read through, for the same
/// reason as [`validate_skill_dir`]'s index guard (CWE-59). This guard is
/// leaf-only: it rejects a symlinked index file, but does not detect a
/// symlinked ancestor directory in `file`'s path — that vector is an
/// accepted trust-model limitation (a local path source is trusted like a
/// build script).
///
/// # Errors
///
/// [`SkillErrorKind::Io`], [`SkillErrorKind::NameInvalid`], or
/// [`SkillErrorKind::FrontmatterParse`].
pub fn validate_rule_file(file: &Path) -> Result<RuleFrontmatter, SkillError> {
    let name = rule_name(file)?;
    SkillName::parse(&name).map_err(|e| SkillError::new(file, SkillErrorKind::NameInvalid(e)))?;
    reject_symlinked_index(file)?;
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
/// A symlinked `file` is rejected rather than read through, for the same
/// reason as [`validate_skill_dir`]'s index guard (CWE-59). This guard is
/// leaf-only: it rejects a symlinked index file, but does not detect a
/// symlinked ancestor directory in `file`'s path — that vector is an
/// accepted trust-model limitation (a local path source is trusted like a
/// build script).
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
    reject_symlinked_index(file)?;
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

/// Whether `path` is itself a symlink. Uses `symlink_metadata` (never
/// follows the link) — the same call [`collect_files`] uses to decide
/// whether to skip an entry while packing. A read/stat failure (including
/// the path simply not existing) is not a symlink.
fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// Reject a symlinked rule/agent index file (CWE-59): a rule or agent has
/// no directory wrapper to discover its index through, so unlike
/// [`validate_skill_dir`] there is no "absent" framing — a symlinked `file`
/// is refused outright as not-found, the same shape a genuinely-missing
/// path already produces via [`std::fs::read_to_string`].
///
/// # Errors
///
/// [`SkillErrorKind::Io`] (not-found) when `file` is a symlink.
fn reject_symlinked_index(file: &Path) -> Result<(), SkillError> {
    if is_symlink(file) {
        return Err(SkillError::new(
            file,
            SkillErrorKind::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "index file is a symlink; refusing to read through it outside the source tree",
            )),
        ));
    }
    Ok(())
}

/// Pack the skill directory at `dir` into an uncompressed tar whose
/// entries are rooted at `<name>/`, matching the materializer's expected
/// layout. The tree under `dir` is included minus its ignore set (defaults plus
/// `.grimignore`, see [`IgnoreSet`]); entries are emitted in
/// sorted path order for a deterministic digest.
///
/// # Errors
///
/// [`SkillErrorKind::Io`] for a walk/read failure.
pub fn pack_skill_dir(dir: &Path) -> Result<Vec<u8>, SkillError> {
    pack_tree_limited(dir, &PackLimits::DEFAULT, crate::oci::ArtifactKind::Skill)
}

/// [`pack_skill_dir`] for a **hook** payload tree: the same walk, sort order
/// and bounds, except `hook.toml` is never ignored (C-160) — a hook whose
/// `.grimignore` says `*.toml` still packs its manifest.
///
/// # Errors
///
/// As [`pack_skill_dir`].
pub fn pack_hook_payload_dir(dir: &Path) -> Result<Vec<u8>, SkillError> {
    pack_tree_limited(dir, &PackLimits::DEFAULT, crate::oci::ArtifactKind::Hook)
}

/// Pack a **mod** directory: the walk of [`pack_skill_dir`] under the mod caps,
/// `mod.toml` never ignored and a symlink refused (65).
///
/// `layout` receives the sorted `/`-joined paths below the root (`mod.toml`
/// and `.grimignore` excluded) and returns the packed `mod.toml` text, or the
/// reason the tree is refused. The author's `mod.toml` is replaced by that
/// text in the tar.
///
/// # Errors
///
/// [`SkillErrorKind::Io`] for a walk/read failure, [`SkillErrorKind::TooLarge`]
/// past a mod cap, [`SkillErrorKind::ValidationFailed`] for a symlink or a
/// refused layout.
pub fn pack_mod_dir(
    dir: &Path,
    layout: impl FnOnce(&[String]) -> Result<String, String>,
) -> Result<Vec<u8>, SkillError> {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| {
            SkillError::new(
                dir,
                SkillErrorKind::NameInvalid("mod path has no directory name".to_string()),
            )
        })?;
    let mut state = WalkState::default();
    let ignore = ignore_set(dir, crate::oci::ArtifactKind::Mod)?;
    collect_files(dir, dir, &name, &ignore, &mut state, 0, &PackLimits::MOD)?;
    let prefix = format!("{name}/");
    let manifest_entry = format!("{prefix}mod.toml");
    let ignore_entry = format!("{prefix}{}", crate::install::ignore_set::GRIMIGNORE);
    // `.grimignore` steers the walk; it is not payload.
    let mut files: Vec<(String, PathBuf)> = state.out.into_iter().filter(|(e, _)| *e != ignore_entry).collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let rels: Vec<String> = files
        .iter()
        .filter(|(e, _)| *e != manifest_entry)
        .map(|(e, _)| e[prefix.len()..].to_string())
        .collect();
    let packed_toml = layout(&rels).map_err(|reason| SkillError::new(dir, SkillErrorKind::ValidationFailed(reason)))?;

    let mut builder = tar::Builder::new(Vec::new());
    let mut read_bytes: u64 = 0;
    for (entry_path, abs) in &files {
        let bytes = if *entry_path == manifest_entry {
            packed_toml.clone().into_bytes()
        } else {
            read_capped(abs, PackLimits::MOD.byte_limit.saturating_sub(read_bytes))?
        };
        read_bytes = read_bytes.saturating_add(bytes.len() as u64);
        append_entry(&mut builder, entry_path, &bytes).map_err(|e| SkillError::new(abs, SkillErrorKind::Io(e)))?;
    }
    // The walk counted the author's `mod.toml`; the install validator counts the packed one.
    check_pack_bounds(dir, read_bytes, files.len(), &PackLimits::MOD)?;
    builder
        .into_inner()
        .map_err(|e| SkillError::new(dir, SkillErrorKind::Io(e)))
}

#[cfg(test)]
fn pack_skill_dir_limited(dir: &Path, limits: &PackLimits) -> Result<Vec<u8>, SkillError> {
    pack_tree_limited(dir, limits, crate::oci::ArtifactKind::Skill)
}

/// [`pack_skill_dir`] with injectable packing bounds (see [`PackLimits`]),
/// so a test can drive the real walk with low caps.
fn pack_tree_limited(dir: &Path, limits: &PackLimits, kind: crate::oci::ArtifactKind) -> Result<Vec<u8>, SkillError> {
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
    let ignore = ignore_set(dir, kind)?;
    collect_files(dir, dir, &name, &ignore, &mut state, 0, limits)?;
    if kind == crate::oci::ArtifactKind::Hook {
        warn_left_behind_dependencies(&name, &state.pruned_dirs);
    }
    let mut files = state.out;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    if kind == crate::oci::ArtifactKind::Skill {
        refuse_packed_plugin_manifest(dir, &files)?;
    }

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

/// Whether `name` is a Claude plugin manifest entry name: cut at the first `:`
/// (an NTFS stream suffix), trailing ASCII dots and spaces dropped, then equal
/// to `.CLAUDE-PLUGIN` in Unicode uppercase. A case-insensitive or
/// Windows-normalising filesystem resolves every such spelling to the
/// directory a client reads, so an exact-name test would let them through.
#[must_use]
pub fn is_skill_plugin_manifest_name(name: &str) -> bool {
    let stem = name.split(':').next().unwrap_or("");
    stem.trim_end_matches(['.', ' ']).to_uppercase() == ".CLAUDE-PLUGIN"
}

/// The first manifest-named entry among a skill root's immediate child names,
/// in the spelling given. Callers pass names as the filesystem stored them.
pub fn skill_plugin_manifest_entry<'a>(root_names: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    root_names.into_iter().find(|n| is_skill_plugin_manifest_name(n))
}

/// Refuse a skill whose packed entries (`<name>/<rel>`, post-ignore) include a
/// root-level manifest-named entry, file or directory. Deeper levels stay allowed.
fn refuse_packed_plugin_manifest(dir: &Path, files: &[(String, PathBuf)]) -> Result<(), SkillError> {
    let root_children = files.iter().filter_map(|(entry, _)| {
        let mut parts = entry.splitn(3, '/');
        parts.next();
        parts.next()
    });
    match skill_plugin_manifest_entry(root_children) {
        Some(entry) => Err(SkillError::new(
            dir,
            SkillErrorKind::SkillPluginManifest {
                dir: dir.to_path_buf(),
                entry: entry.to_string(),
            },
        )),
        None => Ok(()),
    }
}

/// `grim publish` pre-pass: refuse a skill that `pack_skill_dir` would refuse,
/// before any entry of the batch is pushed.
///
/// # Errors
///
/// [`SkillErrorKind::SkillPluginManifest`]; [`SkillErrorKind::Io`] and the
/// packing bounds for a walk failure.
pub fn refuse_skill_plugin_manifest(dir: &Path) -> Result<(), SkillError> {
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
    let ignore = ignore_set(dir, crate::oci::ArtifactKind::Skill)?;
    collect_files(dir, dir, &name, &ignore, &mut state, 0, &PackLimits::DEFAULT)?;
    refuse_packed_plugin_manifest(dir, &state.out)
}

/// A hook handler runs from its installed payload, so an ignored dependency
/// directory in the source tree (`node_modules/`, `.venv/`) is one the handler
/// may import from and will not find. Not provable from `hook.toml`, so a
/// warning rather than a refusal (a handler *naming* such a file is rule 11's 65).
fn warn_left_behind_dependencies(name: &str, pruned_dirs: &[PathBuf]) {
    const DEPENDENCY_DIRS: &[&str] = &["node_modules", ".venv", "venv"];
    for dir in pruned_dirs {
        if dir.file_name().is_some_and(|n| DEPENDENCY_DIRS.iter().any(|d| n == *d)) {
            let rel: Vec<String> = dir.iter().map(|c| c.to_string_lossy().into_owned()).collect();
            tracing::warn!(
                "hook '{name}': '{}/' is excluded by the ignore rules and will not be packed; a handler that \
                 loads dependencies from it will fail at run time — vendor them into a shipped path or \
                 re-include the directory with a '!' line in .grimignore",
                rel.join("/")
            );
        }
    }
}

/// Catalog files that may sit in the stem directory of a rule or agent
/// (`rules/<name>/`, `agents/<name>/`). They are read elsewhere and never packed.
const COMPANION_FILES: &[&str] = &["README.md", "logo.png", "logo.svg"];

/// Files in the stem directory of `file` other than [`COMPANION_FILES`]:
/// forward-slash paths relative to that directory, sorted. Empty when the
/// directory is absent. Only the built-in ignore defaults apply, so a
/// `.grimignore` there cannot hide a stray file.
fn stray_stem_files(file: &Path) -> Result<Vec<String>, SkillError> {
    let stem_dir = file.with_extension("");
    if !stem_dir.is_dir() {
        return Ok(Vec::new());
    }
    let limits = &PackLimits::DEFAULT;
    let ignore = IgnoreSet::defaults_only(crate::oci::ArtifactKind::Rule);
    let mut state = WalkState::default();
    collect_files(&stem_dir, &stem_dir, "", &ignore, &mut state, 0, limits)?;
    let mut files: Vec<String> = state
        .out
        .into_iter()
        .map(|(entry, _)| entry.trim_start_matches('/').to_string())
        .filter(|rel| !COMPANION_FILES.contains(&rel.as_str()))
        .collect();
    files.sort();
    Ok(files)
}

/// Refuse a rule or agent whose stem directory holds a file other than
/// [`COMPANION_FILES`]. Shared by the pack functions and `grim publish`, which
/// runs it for every entry before the first push.
///
/// # Errors
///
/// [`SkillErrorKind::RuleSupportDir`] for a rule, [`SkillErrorKind::AgentStrayFiles`]
/// for an agent; [`SkillErrorKind::Io`] for a walk failure.
pub fn refuse_stray_stem_files(file: &Path, kind: crate::oci::ArtifactKind) -> Result<(), SkillError> {
    let files = stray_stem_files(file)?;
    if files.is_empty() {
        return Ok(());
    }
    let dir = file.with_extension("");
    let kind = if kind == crate::oci::ArtifactKind::Agent {
        SkillErrorKind::AgentStrayFiles { dir, files }
    } else {
        SkillErrorKind::RuleSupportDir { dir, files }
    };
    Err(SkillError::new(file, kind))
}

/// Pack the single `<name>.md` index of a rule into an uncompressed tar.
///
/// The stem directory (`<parent>/<name>/`) may hold only [`COMPANION_FILES`],
/// which are not packed.
///
/// # Errors
///
/// [`SkillErrorKind::RuleSupportDir`] when the stem directory holds any other
/// file; [`SkillErrorKind::Io`] for a read/walk failure.
pub fn pack_rule_file(file: &Path) -> Result<Vec<u8>, SkillError> {
    let name = rule_name(file)?;
    refuse_stray_stem_files(file, crate::oci::ArtifactKind::Rule)?;
    pack_index_only(file, &name)
}

/// Pack the single `<name>.md` index of an agent into an uncompressed tar.
///
/// The stem directory may hold only [`COMPANION_FILES`], which are not packed.
///
/// # Errors
///
/// [`SkillErrorKind::AgentStrayFiles`] when the stem directory holds any other
/// file, [`SkillErrorKind::Io`] for a read failure,
/// [`SkillErrorKind::NameInvalid`] for a stem-less path.
pub fn pack_agent_file(file: &Path) -> Result<Vec<u8>, SkillError> {
    let name = rule_name(file)?;
    refuse_stray_stem_files(file, crate::oci::ArtifactKind::Agent)?;
    pack_index_only(file, &name)
}

fn pack_index_only(file: &Path, name: &str) -> Result<Vec<u8>, SkillError> {
    let limits = &PackLimits::DEFAULT;
    let meta = std::fs::metadata(file).map_err(|e| SkillError::new(file, SkillErrorKind::Io(e)))?;
    check_pack_bounds(file, meta.len(), 1, limits)?;
    let bytes = read_capped(file, limits.byte_limit)?;
    let mut builder = tar::Builder::new(Vec::new());
    append_entry(&mut builder, &format!("{name}.md"), &bytes)
        .map_err(|e| SkillError::new(file, SkillErrorKind::Io(e)))?;
    builder
        .into_inner()
        .map_err(|e| SkillError::new(file, SkillErrorKind::Io(e)))
}

/// The pre-0.16 rule layout: the index plus every file under the stem
/// directory as `<name>/<rel>`, for tests that need an old-format package.
#[cfg(test)]
pub(crate) fn pack_rule_file_legacy(file: &Path) -> Result<Vec<u8>, SkillError> {
    let name = rule_name(file)?;
    let limits = &PackLimits::DEFAULT;
    let mut state = WalkState {
        out: vec![(format!("{name}.md"), file.to_path_buf())],
        ..WalkState::default()
    };
    let support = file.with_extension("");
    if support.is_dir() {
        let ignore = ignore_set(&support, crate::oci::ArtifactKind::Rule)?;
        collect_files(&support, &support, &name, &ignore, &mut state, 0, limits)?;
    }
    let mut files = state.out;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut builder = tar::Builder::new(Vec::new());
    for (entry_path, abs) in &files {
        let bytes = read_capped(abs, limits.byte_limit)?;
        append_entry(&mut builder, entry_path, &bytes).map_err(|e| SkillError::new(abs, SkillErrorKind::Io(e)))?;
    }
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

/// Pack an explicit `(packed_name, source_path)` mapping into the
/// uncompressed tar layer for the reserved `__grimoire` description
/// companion. Each source file is read and appended under its packed name —
/// well-known files (`README.md`, `logo.png`|`logo.svg`, `CHANGELOG.md`) map
/// their source path onto the wire name; `include` glob hits keep their
/// relative path. Entries are emitted in sorted packed-name order with stable
/// headers (no mtime/uid/gid), so republishing byte-identical content produces
/// an identical layer digest — the CAS no-op the caller relies on.
///
/// The mapping must be **non-empty**: an empty companion carries nothing, so
/// the caller resolves that to a data error (there is no README-required gate
/// anymore — every member is optional as long as at least one is present).
///
/// # Errors
///
/// [`SkillErrorKind::ValidationFailed`] when the mapping is empty;
/// [`SkillErrorKind::Io`] for a read failure;
/// [`SkillErrorKind::TooLarge`] when the files exceed the packing bounds.
pub fn pack_description_files(files: &[(String, PathBuf)]) -> Result<Vec<u8>, SkillError> {
    let limits = &PackLimits::DEFAULT;

    let Some((_, first)) = files.first() else {
        return Err(SkillError::new(
            Path::new("description"),
            SkillErrorKind::ValidationFailed("description companion resolves to no files".to_string()),
        ));
    };
    let root = first.clone();

    // Deterministic order: sort by the on-wire packed name.
    let mut files: Vec<(String, PathBuf)> = files.to_vec();
    files.sort_by(|a, b| a.0.cmp(&b.0));

    // Bound the cumulative size (from metadata) and file count before reading
    // any file into memory (CWE-400/770); the read-side cap below is the
    // TOCTOU backstop.
    let mut total_bytes: u64 = 0;
    for (_, abs) in &files {
        let meta = std::fs::metadata(abs).map_err(|e| SkillError::new(abs, SkillErrorKind::Io(e)))?;
        total_bytes = total_bytes.saturating_add(meta.len());
    }
    check_pack_bounds(&root, total_bytes, files.len(), limits)?;

    let mut builder = tar::Builder::new(Vec::new());
    let mut read_bytes: u64 = 0;
    for (entry_path, abs) in &files {
        let bytes = read_capped(abs, limits.byte_limit.saturating_sub(read_bytes))?;
        read_bytes = read_bytes.saturating_add(bytes.len() as u64);
        append_entry(&mut builder, entry_path, &bytes).map_err(|e| SkillError::new(abs, SkillErrorKind::Io(e)))?;
    }
    builder
        .into_inner()
        .map_err(|e| SkillError::new(&root, SkillErrorKind::Io(e)))
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
    /// Refuse a symlink instead of skipping it: a dropped file would break a
    /// mod's module graph at load time.
    refuse_symlinks: bool,
}

impl PackLimits {
    /// The production caps applied by every real packing entry point.
    const DEFAULT: PackLimits = PackLimits {
        byte_limit: PACK_BYTE_LIMIT,
        file_limit: PACK_FILE_LIMIT,
        node_limit: PACK_NODE_LIMIT,
        depth_limit: PACK_DEPTH_LIMIT,
        refuse_symlinks: false,
    };

    /// The mod caps: 16 MiB, 2,000 files, and `MAX_MOD_DEPTH` path segments
    /// below the root, so a directory sits at most `MAX_MOD_DEPTH - 1` deep.
    const MOD: PackLimits = PackLimits {
        byte_limit: crate::oci::mod_manifest::MAX_MOD_LAYER_BYTES,
        file_limit: crate::oci::mod_manifest::MAX_MOD_FILES,
        node_limit: PACK_NODE_LIMIT,
        depth_limit: crate::oci::mod_manifest::MAX_MOD_DEPTH - 1,
        refuse_symlinks: true,
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

/// Strict [`IgnoreSet`] for a pack walk root: an invalid `.grimignore` line
/// is a data error (exit 65) naming the file and line.
fn ignore_set(root: &Path, kind: crate::oci::ArtifactKind) -> Result<IgnoreSet, SkillError> {
    IgnoreSet::for_root(root, kind).map_err(|err| match err {
        IgnoreSetError::Io { path, source } => SkillError::new(path, SkillErrorKind::Io(source)),
        IgnoreSetError::InvalidLine { path, line, message } => SkillError::new(
            path,
            SkillErrorKind::ValidationFailed(format!("line {line}: invalid pattern: {message}")),
        ),
        IgnoreSetError::TooLarge { path } => SkillError::new(
            path,
            SkillErrorKind::ValidationFailed(format!(
                "larger than {} bytes",
                crate::install::ignore_set::MAX_GRIMIGNORE_BYTES
            )),
        ),
        IgnoreSetError::Build { path, message } => SkillError::new(
            path,
            SkillErrorKind::ValidationFailed(format!("cannot compile ignore patterns: {message}")),
        ),
    })
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
    /// Root-relative directories the ignore set pruned, in walk order.
    pruned_dirs: Vec<PathBuf>,
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
    ignore: &IgnoreSet,
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
        let rel = path.strip_prefix(root).unwrap_or(&path);
        // Ignored directories are pruned here: their children are never read,
        // so a large `node_modules/` cannot trip the node/byte caps.
        if ignore.is_ignored(rel, meta.is_dir()) {
            if meta.is_dir() {
                state.pruned_dirs.push(rel.to_path_buf());
            }
            continue;
        }
        if limits.refuse_symlinks && meta.file_type().is_symlink() {
            return Err(SkillError::new(
                &path,
                SkillErrorKind::ValidationFailed("a symlink cannot be packed into a mod".to_string()),
            ));
        }
        if meta.is_dir() {
            collect_files(root, &path, root_name, ignore, state, depth + 1, limits)?;
        } else if meta.is_file() {
            let rel_str: Vec<String> = rel
                .components()
                .filter_map(|c| match c {
                    std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect();
            // Root the entry at `<root_name>/<rel>` (skill / rule / agent —
            // every caller passes a non-empty name).
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

    fn rule_with(tmp: &Path, files: &[(&str, &str)]) -> PathBuf {
        let f = tmp.join("r.md");
        write(&f, "# index\n");
        for (rel, body) in files {
            write(&tmp.join("r").join(rel), body);
        }
        f
    }

    #[test]
    fn pack_rule_refuses_support_files_and_names_them() {
        let tmp = tempfile::tempdir().unwrap();
        let f = rule_with(tmp.path(), &[("notes.md", "n"), ("sub/deep.md", "d")]);
        let err = pack_rule_file(&f).expect_err("support dir");
        let SkillErrorKind::RuleSupportDir { files, .. } = &err.kind else {
            panic!("wrong kind: {err}");
        };
        assert_eq!(files, &["notes.md", "sub/deep.md"]);
        let text = err.to_string();
        assert!(text.contains("found in"));
        assert!(text.contains("upgrading.html#rule-support-dir"));
    }

    #[test]
    fn pack_rule_lists_at_most_five_files() {
        let tmp = tempfile::tempdir().unwrap();
        let names: Vec<String> = (0..7).map(|i| format!("f{i}.md")).collect();
        let entries: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), "x")).collect();
        let f = rule_with(tmp.path(), &entries);
        let text = pack_rule_file(&f).expect_err("support dir").to_string();
        assert!(text.contains("(+2 more)"), "{text}");
        assert!(!text.contains("f5.md"), "{text}");
    }

    #[test]
    fn pack_rule_with_companions_only_is_single_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let with = rule_with(
            &tmp.path().join("a"),
            &[("README.md", "r"), ("logo.png", "p"), ("logo.svg", "s")],
        );
        let without = rule_with(&tmp.path().join("b"), &[]);
        assert_eq!(pack_rule_file(&with).unwrap(), pack_rule_file(&without).unwrap());
    }

    #[test]
    fn pack_rule_ignores_os_junk_in_stem_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let f = rule_with(tmp.path(), &[(".DS_Store", "x")]);
        assert!(pack_rule_file(&f).is_ok());
    }

    #[test]
    fn pack_rule_does_not_read_grimignore_in_stem_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let f = rule_with(tmp.path(), &[(".grimignore", "notes.md\n"), ("notes.md", "n")]);
        let err = pack_rule_file(&f).expect_err("support dir");
        let SkillErrorKind::RuleSupportDir { files, .. } = &err.kind else {
            panic!("wrong kind: {err}");
        };
        assert_eq!(files, &[".grimignore", "notes.md"]);
    }

    #[test]
    fn pack_agent_is_index_only_with_companions() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("a.md");
        write(&f, "---\nname: a\ndescription: d\n---\nbody\n");
        write(&tmp.path().join("a/README.md"), "r");
        write(&tmp.path().join("a/logo.png"), "p");
        assert_eq!(tar_names(&pack_agent_file(&f).unwrap()), vec!["a.md"]);
    }

    #[test]
    fn pack_agent_refuses_stray_files() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("a.md");
        write(&f, "---\nname: a\ndescription: d\n---\nbody\n");
        write(&tmp.path().join("a/extra.txt"), "x");
        let err = pack_agent_file(&f).expect_err("stray");
        assert!(matches!(err.kind, SkillErrorKind::AgentStrayFiles { .. }));
        assert!(err.to_string().contains("upgrading.html#agent-companions"));
    }

    #[test]
    fn pack_rule_file_legacy_emits_name_rel_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let f = rule_with(tmp.path(), &[("notes.md", "n")]);
        assert_eq!(
            tar_names(&pack_rule_file_legacy(&f).unwrap()),
            vec!["r.md", "r/notes.md"]
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
    fn pack_agent_without_companion_dir_is_single_entry() {
        // The degenerate case (no sibling dir at all) is exactly one entry,
        // byte-identical to the historical single-file agent pack.
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("solo.md");
        write(&f, "---\nname: solo\ndescription: d\n---\nbody\n");
        let tar = pack_agent_file(&f).expect("pack");
        let dest = tmp.path().join("out");
        let written = DefaultMaterializer
            .materialize(ArtifactKind::Agent, "solo", &tar, &dest)
            .expect("materialize");
        assert_eq!(written, vec![PathBuf::from("solo.md")]);
    }

    #[test]
    fn pack_description_files_maps_source_paths_onto_wire_names_and_round_trips() {
        // The mapping decouples repo layout from wire layout: `assets/brand.png`
        // packs as the well-known `logo.png`, `docs/CHANGES.md` as `CHANGELOG.md`.
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        write(&src.join("README.md"), "# Repo\n\nWhat this repo ships.\n");
        write(&src.join("docs/CHANGES.md"), "# Changelog\n");
        write(&src.join("docs/img/diagram.svg"), "<svg/>\n");
        std::fs::create_dir_all(src.join("assets")).unwrap();
        std::fs::write(src.join("assets/brand.png"), [0x89u8, 0x50, 0x4e, 0x47, 0x00, 0xff]).unwrap();

        let files = vec![
            ("README.md".to_string(), src.join("README.md")),
            ("CHANGELOG.md".to_string(), src.join("docs/CHANGES.md")),
            ("logo.png".to_string(), src.join("assets/brand.png")),
            // An `include` glob hit keeps its relative path.
            ("docs/img/diagram.svg".to_string(), src.join("docs/img/diagram.svg")),
        ];
        let tar = pack_description_files(&files).expect("pack");
        let dest = tmp.path().join("out");
        // The materializer is generic over the tar; Skill kind is just a label.
        let written = DefaultMaterializer
            .materialize(ArtifactKind::Skill, "description", &tar, &dest)
            .expect("materialize");
        assert_eq!(
            written,
            vec![
                PathBuf::from("CHANGELOG.md"),
                PathBuf::from("README.md"),
                PathBuf::from("docs/img/diagram.svg"),
                PathBuf::from("logo.png"),
            ],
            "packed under wire names, sorted by packed name"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("README.md")).unwrap(),
            "# Repo\n\nWhat this repo ships.\n"
        );
        assert_eq!(
            std::fs::read(dest.join("logo.png")).unwrap(),
            [0x89u8, 0x50, 0x4e, 0x47, 0x00, 0xff]
        );

        // CAS: byte-identical content re-packs to identical bytes (same digest).
        assert_eq!(
            tar,
            pack_description_files(&files).unwrap(),
            "republish must be byte-stable"
        );
    }

    #[test]
    fn pack_description_files_readme_optional_but_not_empty() {
        // No README-required gate: a logo-only companion packs fine.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("logo.png"), [0x89u8, 0x50, 0x4e, 0x47]).unwrap();
        let files = vec![("logo.png".to_string(), tmp.path().join("logo.png"))];
        assert!(pack_description_files(&files).is_ok(), "README is optional");

        // But an empty mapping is a data error (nothing to publish).
        let err = pack_description_files(&[]).expect_err("empty companion is rejected");
        assert!(matches!(err.kind, SkillErrorKind::ValidationFailed(_)));
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
    /// `collect_files` calls `check_pack_bounds` incrementally as each file
    /// is collected (see its own doc comment), so the cap trips mid-walk
    /// rather than after the whole oversized tree is read into memory.
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

    // ── .grimignore ─────────────────────────────────────────────────────

    fn plain_skill(root: &Path, name: &str) -> PathBuf {
        let dir = root.join(name);
        write(
            &dir.join("SKILL.md"),
            &format!("---\nname: {name}\ndescription: d\n---\nbody\n"),
        );
        dir
    }

    #[test]
    fn skill_plugin_manifest_name_rule() {
        for name in [
            ".claude-plugin",
            ".Claude-Plugin",
            ".CLAUDE-PLUGIN",
            ".claude-plugin.",
            ".claude-plugin ",
            ".claude-plugin::$INDEX_ALLOCATION",
            ".claude-plugin\u{131}".trim_end_matches('\u{131}'),
            ".claude-plug\u{131}n",
        ] {
            assert!(is_skill_plugin_manifest_name(name), "{name:?}");
        }
        for name in [
            "claude-plugin",
            ".claude-plugins",
            ".claude-plugin-x",
            ".claude",
            "",
            ":.claude-plugin",
        ] {
            assert!(!is_skill_plugin_manifest_name(name), "{name:?}");
        }
        assert_eq!(
            skill_plugin_manifest_entry(["SKILL.md", ".Claude-Plugin"]),
            Some(".Claude-Plugin")
        );
        // A regular file of that name at the root is refused like a directory.
        let tmp = tempfile::tempdir().unwrap();
        let dir = plain_skill(tmp.path(), "s");
        write(&dir.join(".claude-plugin"), "x");
        let err = pack_skill_dir(&dir).expect_err("file entry");
        assert!(matches!(err.kind, SkillErrorKind::SkillPluginManifest { .. }), "{err}");
    }

    #[test]
    fn pack_refuses_root_manifest_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = plain_skill(tmp.path(), "s");
        write(&dir.join(".Claude-Plugin/plugin.json"), "{}");
        let err = pack_skill_dir(&dir).expect_err("manifest dir");
        let SkillErrorKind::SkillPluginManifest { entry, .. } = &err.kind else {
            panic!("wrong kind: {err}");
        };
        assert_eq!(entry, ".Claude-Plugin", "the entry keeps its spelling");
        let text = err.to_string();
        assert!(text.contains("since grim 0.17.0"), "{text}");
        assert!(text.contains("upgrading.html#skill-plugin-manifest"), "{text}");
        assert!(refuse_skill_plugin_manifest(&dir).is_err());
    }

    #[test]
    fn pack_keeps_nested_ignored_and_symlinked_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = plain_skill(tmp.path(), "nested");
        write(&nested.join("templates/.claude-plugin/plugin.json"), "{}");
        assert!(pack_skill_dir(&nested).is_ok());

        let ignored = plain_skill(tmp.path(), "ignored");
        write(&ignored.join(".claude-plugin/plugin.json"), "{}");
        write(&ignored.join(".grimignore"), ".claude-plugin\n");
        let packed = pack_skill_dir(&ignored).expect("ignored entry is not shipped");
        assert!(tar_names(&packed).iter().all(|n| !n.contains("claude-plugin")));

        let empty = plain_skill(tmp.path(), "empty");
        std::fs::create_dir_all(empty.join(".claude-plugin")).unwrap();
        assert!(pack_skill_dir(&empty).is_ok(), "an empty directory packs no entry");

        #[cfg(unix)]
        {
            let linked = plain_skill(tmp.path(), "linked");
            std::fs::create_dir_all(tmp.path().join("elsewhere")).unwrap();
            std::os::unix::fs::symlink(tmp.path().join("elsewhere"), linked.join(".claude-plugin")).unwrap();
            assert!(pack_skill_dir(&linked).is_ok(), "the walk drops a symlink");
        }
    }

    fn tar_names(tar: &[u8]) -> Vec<String> {
        tar::Archive::new(tar)
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    /// C-160: a hook whose `.grimignore` says `*.toml` still packs its
    /// manifest; a skill whose `.grimignore` lists `hook.toml` omits it,
    /// byte-identical to packing the tree without that file.
    #[test]
    fn hook_toml_is_never_ignored_for_hooks_only() {
        let tmp = tempfile::tempdir().unwrap();
        let hook = tmp.path().join("guard");
        write(&hook.join("hook.toml"), "schema = 1\n");
        write(&hook.join("guard.sh"), "exit 0\n");
        write(&hook.join("extra.toml"), "x = 1\n");
        write(&hook.join(".grimignore"), "*.toml\n");
        let names = tar_names(&pack_hook_payload_dir(&hook).unwrap());
        assert!(names.contains(&"guard/hook.toml".to_string()), "{names:?}");
        assert!(!names.contains(&"guard/extra.toml".to_string()), "{names:?}");

        let skill = tmp.path().join("s");
        write(&skill.join("SKILL.md"), "---\nname: s\ndescription: d\n---\n");
        write(&skill.join(".grimignore"), "hook.toml\n");
        let without = pack_skill_dir(&skill).unwrap();
        write(&skill.join("hook.toml"), "schema = 1\n");
        let with = pack_skill_dir(&skill).unwrap();
        assert_eq!(
            with, without,
            "an ignored hook.toml must not move a skill's packed bytes"
        );
    }

    #[test]
    fn pack_skips_ignored_files_and_packs_grimignore() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("s");
        write(&dir.join("SKILL.md"), "---\nname: s\ndescription: d\n---\n");
        write(&dir.join("scripts/foo.py"), "print(1)\n");
        write(&dir.join("scripts/__pycache__/foo.cpython-313.pyc"), "x");
        write(&dir.join(".DS_Store"), "x");
        write(&dir.join("secret.txt"), "x");
        write(&dir.join(".grimignore"), "secret.txt\n");
        let names = tar_names(&pack_skill_dir(&dir).unwrap());
        assert_eq!(names, vec!["s/.grimignore", "s/SKILL.md", "s/scripts/foo.py"]);
    }

    #[test]
    fn ignored_dir_is_pruned_before_the_node_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("s");
        write(&dir.join("SKILL.md"), "---\nname: s\ndescription: d\n---\n");
        for i in 0..10 {
            write(&dir.join(format!("node_modules/f{i}.js")), "x");
        }
        let limits = PackLimits {
            node_limit: 5,
            ..PackLimits::DEFAULT
        };
        let names = tar_names(&pack_skill_dir_limited(&dir, &limits).expect("pruned dir is never read"));
        assert_eq!(names, vec!["s/SKILL.md"]);
    }

    #[test]
    fn invalid_grimignore_is_a_data_error_naming_the_line() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("s");
        write(&dir.join("SKILL.md"), "---\nname: s\ndescription: d\n---\n");
        write(&dir.join(".grimignore"), "# comment\n{unclosed\n");
        let err = pack_skill_dir(&dir).expect_err("invalid pattern must fail the pack");
        assert!(
            matches!(err.kind, SkillErrorKind::ValidationFailed(_)),
            "{:?}",
            err.kind
        );
        assert!(err.path.ends_with(".grimignore"));
        assert!(err.to_string().contains("line 2"), "{err}");
        let top = anyhow::Error::from(crate::error::Error::from(err));
        assert_eq!(
            crate::error::classify_error(&top),
            crate::cli::exit_code::ExitCode::DataError
        );
    }

    #[test]
    fn legacy_rule_support_dir_honours_its_own_grimignore() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("rules/my-rule.md");
        write(&f, "# index\n");
        write(&tmp.path().join("rules/my-rule/run.py"), "x\n");
        write(&tmp.path().join("rules/my-rule/__pycache__/run.cpython-313.pyc"), "x");
        write(&tmp.path().join("rules/my-rule/draft.md"), "x\n");
        write(&tmp.path().join("rules/my-rule/.grimignore"), "draft.md\n");
        let names = tar_names(&pack_rule_file_legacy(&f).unwrap());
        assert_eq!(names, vec!["my-rule.md", "my-rule/.grimignore", "my-rule/run.py"]);
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

    // ── W5: validate_* agrees with collect_files' symlink-skip policy ────

    /// W5: a symlinked `SKILL.md` must be rejected by `validate_skill_dir`
    /// the same way a genuinely-missing one is (`MissingSkillMd`) — never
    /// read through. Packing already silently skips a symlinked entry
    /// (`pack_skill_dir_skips_symlinked_file_and_dir`); validation must
    /// agree instead of reading a target that may sit outside the tree.
    #[test]
    #[cfg(unix)]
    fn validate_skill_dir_rejects_symlinked_skill_md() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside-secret.md");
        write(&outside, "---\nname: code-review\ndescription: TOP SECRET\n---\n");

        let dir = tmp.path().join("code-review");
        std::fs::create_dir_all(&dir).unwrap();
        symlink(&outside, dir.join("SKILL.md")).unwrap();

        let err = validate_skill_dir(&dir).expect_err("a symlinked SKILL.md must be rejected, not read through");
        assert!(
            matches!(err.kind, SkillErrorKind::MissingSkillMd),
            "expected MissingSkillMd, got {:?}",
            err.kind
        );
    }

    /// W5: a symlinked rule file must be rejected outright (no directory
    /// wrapper to discover it through, unlike the skill-dir case).
    #[test]
    #[cfg(unix)]
    fn validate_rule_file_rejects_symlinked_file() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("secret.md");
        write(&outside, "---\npaths: [\"**/*.rs\"]\n---\nTOP SECRET\n");
        let link = tmp.path().join("rust-style.md");
        symlink(&outside, &link).unwrap();

        let err = validate_rule_file(&link).expect_err("a symlinked rule file must be rejected, not read through");
        assert!(
            matches!(err.kind, SkillErrorKind::Io(ref e) if e.kind() == std::io::ErrorKind::NotFound),
            "expected Io(NotFound), got {:?}",
            err.kind
        );
    }

    /// W5: a symlinked agent file must be rejected outright, mirroring the
    /// rule-file guard.
    #[test]
    #[cfg(unix)]
    fn validate_agent_file_rejects_symlinked_file() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("secret.md");
        write(&outside, "---\nname: reviewer\ndescription: TOP SECRET\n---\nbody\n");
        let link = tmp.path().join("reviewer.md");
        symlink(&outside, &link).unwrap();

        let err = validate_agent_file(&link).expect_err("a symlinked agent file must be rejected, not read through");
        assert!(
            matches!(err.kind, SkillErrorKind::Io(ref e) if e.kind() == std::io::ErrorKind::NotFound),
            "expected Io(NotFound), got {:?}",
            err.kind
        );
    }
}
