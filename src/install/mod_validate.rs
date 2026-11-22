//! Install-time validation of a mod layer, before any byte is written.
//!
//! The registry is untrusted: build-time checks bind honest tooling only, so
//! the fetched tar is re-checked here whole before anything is rendered.
//! Taking the decoded tar bytes (not an unpacked directory) is deliberate: a
//! symlink or hardlink entry is only visible in the archive.

#![allow(
    dead_code,
    reason = "the surface exists before its callers; the install pipeline fills it in"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use crate::oci::annotations::{MOD_CLIENTS_ANNOTATION, mod_clients_from_annotations};
use crate::oci::mod_manifest::{
    InstalledMod, KNOWN_HARNESSES, MAX_MOD_DEPTH, MAX_MOD_FILES, MAX_MOD_LAYER_BYTES, case_insensitive_collision,
    code_extension, code_stem, is_opencode_entry, segment_refusal,
};
use crate::path_safety::{contain, path_segment_refusal};

/// A layer that passed every install-time rule.
#[derive(Debug)]
pub struct ValidatedMod {
    /// The packed `mod.toml`, parsed.
    pub manifest: InstalledMod,
    /// Per known harness, every file as `/`-joined path relative to the
    /// harness directory, to its bytes.
    pub harnesses: BTreeMap<String, BTreeMap<String, Vec<u8>>>,
    /// Whether the Claude tree holds `types/index.d.ts`.
    pub claude_types: bool,
    /// Harness names this grim does not know (directories and tables), sorted;
    /// one warning each, never written.
    pub unknown_harnesses: Vec<String>,
}

impl ValidatedMod {
    /// The files of one known harness, or `None` when the mod does not serve it.
    pub fn files(&self, harness: &str) -> Option<&BTreeMap<String, Vec<u8>>> {
        self.harnesses.get(harness)
    }
}

/// A layer one install-time rule refused.
#[derive(Debug, thiserror::Error)]
#[error("invalid mod: {rule}: {detail}")]
pub struct ModInvalid {
    /// The rule that refused, spelled for the message.
    pub rule: &'static str,
    /// What the rule found.
    pub detail: String,
}

fn refuse<T>(rule: &'static str, detail: impl Into<String>) -> Result<T, ModInvalid> {
    Err(ModInvalid {
        rule,
        detail: detail.into(),
    })
}

/// What one tar entry is, after the type gate.
struct Raw {
    path: String,
    is_dir: bool,
    bytes: Vec<u8>,
}

/// Read every entry, refusing anything but regular files and directories, and
/// enforcing the entry-count and byte caps before a body is read.
fn read_entries(tar_bytes: &[u8]) -> Result<Vec<Raw>, ModInvalid> {
    let mut archive = tar::Archive::new(tar_bytes);
    let entries = archive.entries().map_err(|e| ModInvalid {
        rule: "archive",
        detail: format!("cannot read the tar: {e}"),
    })?;
    let (mut files, mut dirs, mut total) = (0usize, 0usize, 0u64);
    let mut out = Vec::new();
    for entry in entries {
        let mut entry = entry.map_err(|e| ModInvalid {
            rule: "archive",
            detail: format!("cannot read a tar entry: {e}"),
        })?;
        let path = match std::str::from_utf8(&entry.path_bytes()) {
            Ok(p) => p.to_string(),
            Err(_) => return refuse("path", "an entry path is not UTF-8"),
        };
        let kind = entry.header().entry_type();
        let is_dir = kind.is_dir();
        if !is_dir && !kind.is_file() {
            return refuse(
                "entry-type",
                format!("'{}' is neither a regular file nor a directory", shown(&path)),
            );
        }
        if is_dir {
            dirs += 1;
            if dirs > MAX_MOD_FILES {
                return refuse("caps", format!("more than {MAX_MOD_FILES} directory entries"));
            }
            out.push(Raw {
                path,
                is_dir,
                bytes: Vec::new(),
            });
            continue;
        }
        files += 1;
        if files > MAX_MOD_FILES {
            return refuse("caps", format!("more than {MAX_MOD_FILES} files"));
        }
        let size = entry.size();
        total = total.saturating_add(size);
        if total > MAX_MOD_LAYER_BYTES {
            return refuse("caps", format!("more than {MAX_MOD_LAYER_BYTES} bytes decoded"));
        }
        let mut bytes = Vec::with_capacity(size as usize);
        entry
            .by_ref()
            .take(size)
            .read_to_end(&mut bytes)
            .map_err(|e| ModInvalid {
                rule: "archive",
                detail: format!("cannot read '{}': {e}", shown(&path)),
            })?;
        out.push(Raw { path, is_dir, bytes });
    }
    Ok(out)
}

/// A path for a message: escaped so a hostile name cannot forge a line.
fn shown(path: &str) -> String {
    path.escape_debug().to_string()
}

/// The path rules that hold for every entry, known harness or not: portable
/// segments, containment, no backslash.
fn check_path(path: &str) -> Result<Vec<&str>, ModInvalid> {
    let trimmed = path.strip_suffix('/').unwrap_or(path);
    if trimmed.contains('\\') {
        return refuse("path", format!("'{}' holds a backslash", shown(path)));
    }
    if let Some(why) = path_segment_refusal(trimmed) {
        return refuse("path", format!("'{}': {why}", shown(path)));
    }
    if contain(Path::new("/grim-mod-layer"), Path::new(trimmed)).is_err() {
        return refuse("path", format!("'{}' escapes the layer", shown(path)));
    }
    Ok(trimmed.split('/').collect())
}

/// Validate the decoded layer tar against the manifest `annotation` (the
/// `com.grimoire.mod.clients` value) and the lock row's `lock_clients`.
///
/// The tar is rooted at `<name>/` as the packer writes it. Nothing is
/// returned unless every rule holds, so a refusal precedes every byte written.
///
/// # Errors
///
/// [`ModInvalid`] naming the first rule the layer breaks.
pub fn validate_layer(
    tar_bytes: &[u8],
    annotation: Option<&str>,
    lock_clients: &BTreeSet<String>,
) -> Result<ValidatedMod, ModInvalid> {
    let raw = read_entries(tar_bytes)?;
    let mut root: Option<&str> = None;
    // (segments below the root, is_dir, bytes) for every entry below the root.
    let mut below: Vec<(Vec<&str>, bool, &[u8])> = Vec::new();
    for r in &raw {
        let segments = check_path(&r.path)?;
        match root {
            None => root = Some(segments[0]),
            Some(first) if first != segments[0] => {
                return refuse("layout", "the layer has more than one top-level directory");
            }
            Some(_) => {}
        }
        if segments.len() == 1 {
            if !r.is_dir {
                return refuse("layout", "the layer root is a file, not a directory");
            }
            continue;
        }
        let rest = segments[1..].to_vec();
        if rest.len() > MAX_MOD_DEPTH {
            return refuse(
                "caps",
                format!("'{}' is deeper than {MAX_MOD_DEPTH} segments", shown(&r.path)),
            );
        }
        below.push((rest, r.is_dir, &r.bytes));
    }
    let Some(root) = root else {
        return refuse("layout", "the layer is empty");
    };

    let joined: Vec<String> = below.iter().map(|(s, _, _)| s.join("/")).collect();
    let mut file_paths = BTreeSet::new();
    for ((_, is_dir, _), path) in below.iter().zip(&joined) {
        if !is_dir && !file_paths.insert(path.as_str()) {
            return refuse("duplicate", format!("'{}' appears twice", shown(path)));
        }
    }
    if let Some((a, b)) = case_insensitive_collision(joined.iter().map(String::as_str)) {
        return refuse(
            "duplicate",
            format!("'{}' and '{}' differ only in case", shown(&a), shown(&b)),
        );
    }
    for path in &joined {
        let mut prefix = String::new();
        for part in path.split('/') {
            if file_paths.contains(prefix.as_str()) {
                return refuse(
                    "duplicate",
                    format!("'{}' is both a file and a directory", shown(&prefix)),
                );
            }
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
        }
    }

    // mod.toml: the one file at the top level.
    let mut manifest_bytes: Option<&[u8]> = None;
    let mut present: BTreeSet<&str> = BTreeSet::new();
    for (segs, is_dir, bytes) in &below {
        if segs.len() == 1 && !is_dir {
            if segs[0] != "mod.toml" {
                return refuse(
                    "layout",
                    format!("'{}' sits beside the harness directories", shown(segs[0])),
                );
            }
            manifest_bytes = Some(bytes);
        } else {
            present.insert(segs[0]);
        }
    }
    let Some(manifest_bytes) = manifest_bytes else {
        return refuse("manifest", "the layer has no mod.toml");
    };
    let manifest = std::str::from_utf8(manifest_bytes)
        .map_err(|_| ModInvalid {
            rule: "manifest",
            detail: "mod.toml is not UTF-8".to_string(),
        })
        .and_then(|s| {
            InstalledMod::from_installed_toml(s).map_err(|e| ModInvalid {
                rule: "manifest",
                detail: e.to_string(),
            })
        })?;
    if manifest.name != root {
        return refuse(
            "manifest",
            format!(
                "mod.toml name '{}' differs from the layer directory '{root}'",
                manifest.name
            ),
        );
    }

    // D3 rule 2: directories and annotation and lock agree.
    let map: BTreeMap<String, String> = annotation
        .map(|v| BTreeMap::from([(MOD_CLIENTS_ANNOTATION.to_string(), v.to_string())]))
        .unwrap_or_default();
    let annotated = mod_clients_from_annotations(&map).map_err(|e| ModInvalid {
        rule: "clients",
        detail: e.to_string(),
    })?;
    let annotated: BTreeSet<String> = annotated.into_iter().collect();
    if &annotated != lock_clients {
        return refuse(
            "clients",
            format!("the lock row's clients {lock_clients:?} differ from the annotation {annotated:?}"),
        );
    }
    let known_dirs: BTreeSet<String> = present
        .iter()
        .filter(|d| KNOWN_HARNESSES.contains(d))
        .map(|d| (*d).to_string())
        .collect();
    let known_annotated: BTreeSet<String> = annotated
        .iter()
        .filter(|n| KNOWN_HARNESSES.contains(&n.as_str()))
        .cloned()
        .collect();
    if known_dirs != known_annotated {
        return refuse(
            "clients",
            format!("the harness directories {known_dirs:?} differ from the annotation {known_annotated:?}"),
        );
    }

    // Segment rule in every harness directory (known or not); code extension
    // only in known ones.
    let mut harnesses: BTreeMap<String, BTreeMap<String, Vec<u8>>> = BTreeMap::new();
    let mut unknown: BTreeSet<String> = manifest.unknown_harnesses.iter().cloned().collect();
    for (segs, is_dir, bytes) in &below {
        if segs.len() == 1 && !is_dir && segs[0] == "mod.toml" {
            continue;
        }
        for seg in segs {
            if let Some(why) = segment_refusal(seg) {
                return refuse(
                    "segment",
                    format!("'{}' in '{}': {why}", shown(seg), shown(&segs.join("/"))),
                );
            }
        }
        let harness = segs[0];
        if !KNOWN_HARNESSES.contains(&harness) {
            unknown.insert(harness.to_string());
            continue;
        }
        if *is_dir {
            continue;
        }
        let rel = segs[1..].join("/");
        if code_extension(segs[segs.len() - 1]).is_none() {
            return refuse(
                "extension",
                format!("'{}' has no code extension", shown(&segs.join("/"))),
            );
        }
        harnesses
            .entry(harness.to_string())
            .or_default()
            .insert(rel, bytes.to_vec());
    }
    // A known directory holding no file still counts as served.
    for d in &known_dirs {
        harnesses.entry(d.clone()).or_default();
    }

    // D3 rule 3: the packed entries.
    let served: BTreeSet<&str> = manifest.served.keys().map(String::as_str).collect();
    let dirs: BTreeSet<&str> = known_dirs.iter().map(String::as_str).collect();
    if served != dirs {
        return refuse(
            "entry",
            format!("mod.toml serves {served:?} but the layer holds the harness directories {dirs:?}"),
        );
    }
    for (harness, entry) in &manifest.served {
        let files = &harnesses[harness];
        check_entry(harness, entry, files)?;
    }

    let claude_types = harnesses
        .get("claude")
        .is_some_and(|f| f.contains_key("types/index.d.ts"));
    Ok(ValidatedMod {
        manifest,
        harnesses,
        claude_types,
        unknown_harnesses: unknown.into_iter().collect(),
    })
}

fn check_entry(harness: &str, entry: &str, files: &BTreeMap<String, Vec<u8>>) -> Result<(), ModInvalid> {
    if entry.contains('\\') {
        return refuse(
            "entry",
            format!("the {harness} entry '{}' holds a backslash", shown(entry)),
        );
    }
    if let Some(why) = path_segment_refusal(entry) {
        return refuse("entry", format!("the {harness} entry '{}': {why}", shown(entry)));
    }
    if let Some(bad) = entry.split('/').find_map(|s| segment_refusal(s).map(|w| (s, w))) {
        return refuse(
            "entry",
            format!("the {harness} entry '{}': '{}' {}", shown(entry), shown(bad.0), bad.1),
        );
    }
    if contain(Path::new("/grim-mod-layer"), Path::new(entry)).is_err() {
        return refuse(
            "entry",
            format!("the {harness} entry '{}' leaves its harness directory", shown(entry)),
        );
    }
    if !files.contains_key(entry) {
        return refuse(
            "entry",
            format!("the {harness} entry '{}' names no file in the layer", shown(entry)),
        );
    }
    let name = entry.rsplit('/').next().unwrap_or(entry);
    if harness == "opencode" {
        if !is_opencode_entry(name) {
            return refuse(
                "entry",
                format!("the opencode entry '{}' must be a .ts or .js file", shown(entry)),
            );
        }
        let (dir, _) = entry.rsplit_once('/').unwrap_or(("", entry));
        let stem = code_stem(name);
        let sharing = files
            .keys()
            .filter(|p| {
                let (d, n) = p.rsplit_once('/').unwrap_or(("", p));
                d == dir && code_stem(n) == stem
            })
            .count();
        if sharing > 1 {
            return refuse(
                "entry",
                format!(
                    "more than one code file in the opencode directory shares the stem of '{}'",
                    shown(entry)
                ),
            );
        }
    } else if code_extension(name).is_none() {
        return refuse(
            "entry",
            format!("the {harness} entry '{}' is not a code file", shown(entry)),
        );
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const TOML_BOTH: &str = "schema = 1\nname = \"status-band\"\ndescription = \"d\"\n[claude]\nentry = \"register.tsx\"\n[opencode]\nentry = \"status-band.ts\"\n";

    /// One raw tar entry: `kind` is the tar type byte.
    pub struct E<'a>(pub &'a str, pub u8, pub &'a [u8]);

    pub fn file<'a>(path: &'a str, body: &'a [u8]) -> E<'a> {
        E(path, b'0', body)
    }

    /// A tar with entry names written as given, bypassing the builder's path checks.
    pub fn tar_of(entries: &[E<'_>]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for E(path, kind, body) in entries {
            let mut h = tar::Header::new_old();
            let raw = path.as_bytes();
            h.as_old_mut().name[..raw.len()].copy_from_slice(raw);
            h.set_entry_type(tar::EntryType::new(*kind));
            h.set_mode(0o644);
            h.set_size(body.len() as u64);
            if matches!(*kind, b'1' | b'2') {
                h.set_link_name("target").unwrap();
            }
            h.set_cksum();
            b.append(&h, *body).unwrap();
        }
        b.into_inner().unwrap()
    }

    fn clients(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| (*s).to_string()).collect()
    }

    /// The valid baseline layer; tests extend or replace parts of it.
    fn base<'a>() -> Vec<E<'a>> {
        vec![
            file("status-band/mod.toml", TOML_BOTH.as_bytes()),
            file("status-band/claude/register.tsx", b"export default 1"),
            file("status-band/claude/lib/format.ts", b"export {}"),
            file("status-band/claude/types/index.d.ts", b"export {}"),
            file("status-band/opencode/status-band.ts", b"export default 1"),
            file("status-band/opencode/helper.ts", b"export {}"),
        ]
    }

    fn run(entries: &[E<'_>]) -> Result<ValidatedMod, ModInvalid> {
        validate_layer(
            &tar_of(entries),
            Some("claude,opencode"),
            &clients(&["claude", "opencode"]),
        )
    }

    fn rule_of(entries: &[E<'_>]) -> &'static str {
        run(entries).expect_err("layer must be refused").rule
    }

    /// The baseline with one extra entry.
    fn plus<'a>(extra: E<'a>) -> Vec<E<'a>> {
        let mut v = base();
        v.push(extra);
        v
    }

    #[test]
    fn a_valid_layer_passes_and_carries_its_files() {
        let v = run(&base()).expect("positive control");
        assert_eq!(v.manifest.name, "status-band");
        assert!(v.claude_types);
        assert_eq!(v.files("claude").unwrap().len(), 3);
        assert_eq!(v.files("opencode").unwrap().len(), 2);
        assert!(v.unknown_harnesses.is_empty());
    }

    #[test]
    fn a_layer_with_directory_entries_passes() {
        let v = plus(E("status-band/claude/lib/", b'5', b""));
        assert!(run(&v).is_ok());
    }

    #[test]
    fn hostile_paths_are_refused() {
        for (path, label) in [
            ("status-band/claude/../../x.ts", "dotdot"),
            ("/status-band/claude/x.ts", "absolute"),
            ("C:/status-band/claude/x.ts", "drive prefix"),
            ("status-band/claude/a\\b.ts", "backslash"),
        ] {
            let layer = plus(file(path, b"x"));
            assert_eq!(rule_of(&layer), "path", "{label}");
        }
    }

    #[test]
    fn segment_rule_refuses_leading_and_trailing_dot_and_foreign_characters() {
        for (path, label) in [
            ("status-band/claude/.hidden.ts", "leading dot"),
            ("status-band/claude/trail.ts.", "trailing dot"),
            ("status-band/claude/sp ace.ts", "space"),
            ("status-band/claude/caf\u{e9}.ts", "non-ascii"),
        ] {
            let layer = plus(file(path, b"x"));
            assert_eq!(rule_of(&layer), "segment", "{label}");
        }
    }

    #[test]
    fn every_device_name_form_is_refused() {
        for name in [
            "con.ts", "CON.ts", "nul", "Aux.ts", "com1.ts", "COM9.ts", "lpt1.tsx", "prn.x.ts", "LPT9",
        ] {
            let path = format!("status-band/claude/{name}");
            let layer = plus(file(&path, b"x"));
            assert_eq!(rule_of(&layer), "segment", "{name}");
        }
        // A device name as a directory segment too.
        let layer = plus(file("status-band/claude/con/x.ts", b"x"));
        assert_eq!(rule_of(&layer), "segment");
    }

    #[test]
    fn a_case_insensitive_duplicate_is_refused() {
        let layer = plus(file("status-band/claude/LIB/format.ts", b"x"));
        assert_eq!(rule_of(&layer), "duplicate");
        let layer = plus(file("status-band/opencode/Helper.ts", b"x"));
        assert_eq!(rule_of(&layer), "duplicate");
    }

    #[test]
    fn the_same_path_twice_is_refused() {
        let layer = plus(file("status-band/claude/lib/format.ts", b"y"));
        assert_eq!(rule_of(&layer), "duplicate");
    }

    #[test]
    fn symlink_and_hardlink_entries_are_refused() {
        assert_eq!(rule_of(&plus(E("status-band/claude/l.ts", b'2', b""))), "entry-type");
        assert_eq!(rule_of(&plus(E("status-band/claude/h.ts", b'1', b""))), "entry-type");
    }

    #[test]
    fn a_non_code_file_in_a_known_harness_is_refused() {
        assert_eq!(rule_of(&plus(file("status-band/claude/notes.md", b"x"))), "extension");
        assert_eq!(
            rule_of(&plus(file("status-band/opencode/data.json", b"x"))),
            "extension"
        );
        assert_eq!(rule_of(&plus(file("status-band/claude/noext", b"x"))), "extension");
    }

    #[test]
    fn annotation_and_directories_must_agree() {
        let bytes = tar_of(&base());
        let both = clients(&["claude", "opencode"]);
        let e = validate_layer(&bytes, Some("claude"), &clients(&["claude"])).unwrap_err();
        assert_eq!(e.rule, "clients");
        let e = validate_layer(
            &bytes,
            Some("claude,opencode,zed"),
            &clients(&["claude", "opencode", "zed"]),
        )
        .map(|v| v.unknown_harnesses)
        .unwrap();
        assert!(e.is_empty());
        assert_eq!(validate_layer(&bytes, None, &both).unwrap_err().rule, "clients");
        assert_eq!(
            validate_layer(&bytes, Some("Claude"), &both).unwrap_err().rule,
            "clients"
        );
    }

    #[test]
    fn lock_clients_must_equal_the_annotation() {
        let bytes = tar_of(&base());
        for lock in [&["claude"][..], &["claude", "opencode", "extra"], &[]] {
            let e = validate_layer(&bytes, Some("claude,opencode"), &clients(lock)).unwrap_err();
            assert_eq!(e.rule, "clients", "{lock:?}");
        }
    }

    #[test]
    fn a_missing_or_stray_entry_is_refused() {
        let toml = TOML_BOTH.replace("register.tsx", "gone.tsx");
        let mut layer = base();
        layer[0] = file("status-band/mod.toml", toml.as_bytes());
        assert_eq!(rule_of(&layer), "entry");
    }

    #[test]
    fn an_entry_outside_its_harness_directory_is_refused() {
        for bad in ["../opencode/status-band.ts", "/etc/passwd.ts", "C:x.ts"] {
            let toml = TOML_BOTH.replace("register.tsx", bad);
            let mut layer = base();
            layer[0] = file("status-band/mod.toml", toml.as_bytes());
            assert_eq!(rule_of(&layer), "entry", "{bad}");
        }
        // The descriptor parser already refuses a backslash entry before the layer check runs.
        let toml = TOML_BOTH.replace("register.tsx", "..\\x.ts");
        let mut layer = base();
        layer[0] = file("status-band/mod.toml", toml.as_bytes());
        assert_eq!(rule_of(&layer), "manifest");
    }

    #[test]
    fn an_entry_without_an_allowed_extension_is_refused() {
        let toml = TOML_BOTH.replace("status-band.ts", "status-band.tsx");
        let mut layer = base();
        layer[0] = file("status-band/mod.toml", toml.as_bytes());
        layer[4] = file("status-band/opencode/status-band.tsx", b"x");
        assert_eq!(rule_of(&layer), "entry");
    }

    #[test]
    fn an_opencode_stem_shared_by_two_code_files_is_refused() {
        assert_eq!(
            rule_of(&plus(file("status-band/opencode/status-band.js", b"x"))),
            "entry"
        );
    }

    #[test]
    fn a_served_harness_without_a_directory_is_refused() {
        let mut layer = base();
        layer.retain(|e| !e.0.contains("/opencode/"));
        let e = validate_layer(&tar_of(&layer), Some("claude"), &clients(&["claude"])).unwrap_err();
        assert_eq!(e.rule, "entry");
    }

    #[test]
    fn the_layer_root_must_match_the_manifest_name() {
        let layer: Vec<E> = vec![file("other/mod.toml", TOML_BOTH.as_bytes())];
        assert_eq!(rule_of(&layer), "manifest");
    }

    #[test]
    fn a_stray_top_level_file_is_refused() {
        assert_eq!(rule_of(&plus(file("status-band/README.md", b"x"))), "layout");
    }

    #[test]
    fn unknown_harness_table_and_directory_install_known_clients_with_one_warning() {
        let toml = format!("{TOML_BOTH}[zed]\nentry = \"a.ts\"\n");
        let mut layer = base();
        layer[0] = file("status-band/mod.toml", toml.as_bytes());
        layer.push(file("status-band/zed/anything.bin", b"x"));
        let bytes = tar_of(&layer);
        let v = validate_layer(
            &bytes,
            Some("claude,opencode,zed"),
            &clients(&["claude", "opencode", "zed"]),
        )
        .unwrap();
        assert_eq!(v.unknown_harnesses, vec!["zed".to_string()]);
        assert!(v.files("zed").is_none());
        assert_eq!(v.files("claude").unwrap().len(), 3);
    }

    #[test]
    fn an_unknown_harness_directory_still_gets_the_path_and_segment_rules() {
        let mut layer = base();
        layer.push(file("status-band/zed/con.ts", b"x"));
        let ann = Some("claude,opencode,zed");
        let lock = clients(&["claude", "opencode", "zed"]);
        assert_eq!(validate_layer(&tar_of(&layer), ann, &lock).unwrap_err().rule, "segment");
        let mut layer = base();
        layer.push(file("status-band/zed/../x", b"x"));
        assert_eq!(validate_layer(&tar_of(&layer), ann, &lock).unwrap_err().rule, "path");
    }

    #[test]
    fn the_file_count_cap_holds_at_the_limit() {
        let names: Vec<String> = (0..MAX_MOD_FILES - 6)
            .map(|i| format!("status-band/claude/f{i}.ts"))
            .collect();
        let mut layer = base();
        layer.extend(names.iter().map(|n| file(n, b"")));
        assert_eq!(layer.len(), MAX_MOD_FILES);
        assert!(run(&layer).is_ok(), "exactly at the cap");
        let over = "status-band/claude/over.ts".to_string();
        layer.push(file(&over, b""));
        assert_eq!(rule_of(&layer), "caps");
    }

    #[test]
    fn the_depth_cap_holds_at_the_limit() {
        let ok = format!("status-band/claude/{}x.ts", "d/".repeat(MAX_MOD_DEPTH - 2));
        assert!(
            run(&plus(file(&ok, b"x"))).is_ok(),
            "{MAX_MOD_DEPTH} segments below the root"
        );
        let deep = format!("status-band/claude/{}x.ts", "d/".repeat(MAX_MOD_DEPTH - 1));
        assert_eq!(rule_of(&plus(file(&deep, b"x"))), "caps");
    }

    #[test]
    fn the_byte_cap_is_refused_from_the_header_before_the_body_is_read() {
        // A header that claims one byte over the cap and carries no body.
        let mut b = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_path("status-band/mod.toml").unwrap();
        h.set_size(MAX_MOD_LAYER_BYTES + 1);
        h.set_cksum();
        b.append(&h, std::io::empty()).ok();
        let mut bytes = b.into_inner().unwrap_or_default();
        bytes.truncate(512);
        let e = validate_layer(&bytes, Some("claude,opencode"), &clients(&["claude", "opencode"])).unwrap_err();
        assert_eq!(e.rule, "caps");
    }
}
