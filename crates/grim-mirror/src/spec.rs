// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The `mirror.toml` grammar and its validation.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorFile {
    pub schema: u32,
    pub mirror: MirrorSection,
    pub upstream: Vec<Upstream>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorSection {
    pub registry: String,
    /// SPDX ids; `None` admits every OSI-approved id.
    pub licenses: Option<Vec<String>>,
    pub backfill: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Dialect {
    #[default]
    Agentskills,
    Claude,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VersionMode {
    #[default]
    Auto,
    Snapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Upstream {
    pub name: String,
    pub repo: String,
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
    pub commit: Option<String>,
    #[serde(default)]
    pub dialect: Dialect,
    #[serde(default)]
    pub version: VersionMode,
    pub tag_pattern: Option<String>,
    pub backfill: Option<u32>,
    pub license: Option<String>,
    #[serde(default)]
    pub unit_licenses: BTreeMap<String, String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    pub marketplace: Option<String>,
    #[serde(default)]
    pub plugins: Vec<String>,
    pub retire: Option<Retire>,
    #[serde(default)]
    pub bundle: Vec<BundleSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Retire {
    pub reason: String,
    pub replaced_by: Option<String>,
    #[serde(default)]
    pub units: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleSpec {
    pub name: String,
    pub members: Vec<String>,
}

#[derive(Debug)]
pub enum SpecError {
    /// The file does not exist (exit 79).
    Absent(PathBuf),
    /// Unreadable, unparsable or failing validation; the message reads
    /// `mirror.toml: <key path>: <problem>` (exit 78).
    Invalid(String),
}

impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absent(p) => write!(f, "mirror file not found: {}", p.display()),
            Self::Invalid(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for SpecError {}

impl Upstream {
    /// The backfill depth in force: the entry's, else `[mirror]`'s, else 1.
    pub fn effective_backfill(&self, mirror: &MirrorSection) -> u32 {
        self.backfill.or(mirror.backfill).unwrap_or(1)
    }
}

/// Reads and validates a mirror file.
///
/// # Errors
///
/// [`SpecError::Absent`] when the file is missing, [`SpecError::Invalid`]
/// for every parse or validation failure.
pub fn load(path: &Path) -> Result<MirrorFile, SpecError> {
    match fs::read_to_string(path) {
        Ok(text) => parse(&text).map_err(|e| match e {
            SpecError::Invalid(m) => SpecError::Invalid(rename_file(m, path)),
            other => other,
        }),
        Err(e) if e.kind() == ErrorKind::NotFound => Err(SpecError::Absent(path.to_owned())),
        Err(e) => Err(SpecError::Invalid(format!("{}: {e}", path.display()))),
    }
}

/// Swaps the `mirror.toml:` prefix for the path the operator gave; the
/// default `./mirror.toml` keeps its short name.
fn rename_file(message: String, path: &Path) -> String {
    if path == Path::new("./mirror.toml") {
        return message;
    }
    match message.strip_prefix("mirror.toml: ") {
        Some(rest) => format!("{}: {rest}", path.display()),
        None => message,
    }
}

fn bad(key: impl fmt::Display, problem: impl fmt::Display) -> SpecError {
    SpecError::Invalid(format!("mirror.toml: {key}: {problem}"))
}

fn parse(text: &str) -> Result<MirrorFile, SpecError> {
    let table: toml::Table = text.parse().map_err(|e| toml_error(text, &e))?;
    check_keys(&table)?;
    let file: MirrorFile = toml::from_str(text).map_err(|e| toml_error(text, &e))?;
    validate(&file)?;
    Ok(file)
}

fn toml_error(text: &str, e: &toml::de::Error) -> SpecError {
    let place = match e.span() {
        Some(span) => format!("line {}", text[..span.start.min(text.len())].matches('\n').count() + 1),
        None => "document".to_owned(),
    };
    bad(place, e.message())
}

const TOP_KEYS: &[&str] = &["schema", "mirror", "upstream"];
const MIRROR_KEYS: &[&str] = &["registry", "licenses", "backfill"];
const UPSTREAM_KEYS: &[&str] = &[
    "name",
    "repo",
    "ref",
    "commit",
    "dialect",
    "version",
    "tag-pattern",
    "backfill",
    "license",
    "unit-licenses",
    "skills",
    "agents",
    "rules",
    "exclude",
    "marketplace",
    "plugins",
    "retire",
    "bundle",
];
const RETIRE_KEYS: &[&str] = &["reason", "replaced-by", "units"];
const BUNDLE_KEYS: &[&str] = &["name", "members"];

/// Names the full key path of an unknown key, which serde's error does not.
fn check_keys(top: &toml::Table) -> Result<(), SpecError> {
    unknown(top, TOP_KEYS, "")?;
    if let Some(toml::Value::Table(m)) = top.get("mirror") {
        unknown(m, MIRROR_KEYS, "mirror")?;
    }
    if let Some(toml::Value::Array(entries)) = top.get("upstream") {
        for (i, entry) in entries.iter().enumerate() {
            let toml::Value::Table(t) = entry else { continue };
            let at = format!("upstream[{i}]");
            unknown(t, UPSTREAM_KEYS, &at)?;
            if let Some(toml::Value::Table(r)) = t.get("retire") {
                unknown(r, RETIRE_KEYS, &format!("{at}.retire"))?;
            }
            if let Some(toml::Value::Array(bundles)) = t.get("bundle") {
                for (j, b) in bundles.iter().enumerate() {
                    if let toml::Value::Table(b) = b {
                        unknown(b, BUNDLE_KEYS, &format!("{at}.bundle[{j}]"))?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn unknown(t: &toml::Table, allowed: &[&str], at: &str) -> Result<(), SpecError> {
    match t.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) if at.is_empty() => Err(bad(k, "unknown key")),
        Some(k) => Err(bad(format!("{at}.{k}"), "unknown key")),
        None => Ok(()),
    }
}

fn validate(f: &MirrorFile) -> Result<(), SpecError> {
    if f.schema != 1 {
        return Err(bad("schema", format!("must be 1, found {}", f.schema)));
    }
    let reg = &f.mirror.registry;
    if reg.is_empty() || reg.contains("://") || reg.chars().any(char::is_whitespace) {
        return Err(bad("mirror.registry", "must be host[:port][/path]"));
    }
    if let Some(ids) = &f.mirror.licenses {
        for (i, id) in ids.iter().enumerate() {
            let at = format!("mirror.licenses[{i}]");
            if id.starts_with("LicenseRef-") {
                return Err(bad(at, format!("{id:?} is not an SPDX list id")));
            }
            if spdx::license_id(id).is_none() {
                return Err(bad(at, format!("{id:?} is not in the SPDX license list")));
            }
        }
    }
    backfill_range("mirror.backfill", f.mirror.backfill)?;

    let mut seen = Vec::new();
    for (i, u) in f.upstream.iter().enumerate() {
        let at = format!("upstream[{i}]");
        if seen.contains(&u.name.as_str()) {
            return Err(bad(format!("{at}.name"), format!("duplicate name {:?}", u.name)));
        }
        seen.push(&u.name);
        validate_upstream(u, &at)?;
    }
    Ok(())
}

fn validate_upstream(u: &Upstream, at: &str) -> Result<(), SpecError> {
    name_ok(&u.name).map_err(|p| bad(format!("{at}.name"), p))?;
    repo_ok(&u.repo).map_err(|p| bad(format!("{at}.repo"), p))?;
    if u.git_ref.is_some() && u.commit.is_some() {
        return Err(bad(format!("{at}.ref"), "excludes `commit`; set one of them"));
    }
    if let Some(c) = &u.commit {
        crate::model::Commit::new(c).map_err(|p| bad(format!("{at}.commit"), p))?;
    }
    if let Some(p) = &u.tag_pattern {
        let re = regex::Regex::new(p).map_err(|e| bad(format!("{at}.tag-pattern"), e))?;
        if re.captures_len() < 2 {
            return Err(bad(
                format!("{at}.tag-pattern"),
                "needs a capture group for the version",
            ));
        }
    }
    backfill_range(&format!("{at}.backfill"), u.backfill)?;
    if let Some(l) = &u.license {
        expression(l).map_err(|p| bad(format!("{at}.license"), p))?;
    }
    for (path, l) in &u.unit_licenses {
        let key = format!("{at}.unit-licenses.{path}");
        safe_relative(path).map_err(|p| bad(&key, p))?;
        expression(l).map_err(|p| bad(&key, p))?;
    }
    for (list, globs) in [
        ("skills", &u.skills),
        ("agents", &u.agents),
        ("rules", &u.rules),
        ("exclude", &u.exclude),
    ] {
        globs_ok(globs, &format!("{at}.{list}"), true)?;
    }
    if let Some(m) = &u.marketplace {
        safe_relative(m).map_err(|p| bad(format!("{at}.marketplace"), p))?;
    }
    match (&u.marketplace, u.plugins.is_empty()) {
        (Some(_), true) => return Err(bad(format!("{at}.plugins"), "required with `marketplace`")),
        (None, false) => return Err(bad(format!("{at}.marketplace"), "required with `plugins`")),
        _ => {}
    }
    globs_ok(&u.plugins, &format!("{at}.plugins"), false)?;
    if u.skills.is_empty() && u.agents.is_empty() && u.rules.is_empty() && u.marketplace.is_none() {
        return Err(bad(at, "needs `skills`, `agents`, `rules` or `marketplace`"));
    }
    if let Some(r) = &u.retire {
        for (i, unit) in r.units.iter().enumerate() {
            safe_relative(unit).map_err(|p| bad(format!("{at}.retire.units[{i}]"), p))?;
        }
    }
    let mut names = Vec::new();
    for (j, b) in u.bundle.iter().enumerate() {
        let b_at = format!("{at}.bundle[{j}]");
        name_ok(&b.name).map_err(|p| bad(format!("{b_at}.name"), p))?;
        if names.contains(&b.name.as_str()) {
            return Err(bad(format!("{b_at}.name"), format!("duplicate name {:?}", b.name)));
        }
        names.push(&b.name);
        globs_ok(&b.members, &format!("{b_at}.members"), true)?;
    }
    Ok(())
}

fn backfill_range(key: &str, v: Option<u32>) -> Result<(), SpecError> {
    match v {
        Some(n) if !(1..=100).contains(&n) => Err(bad(key, format!("must be 1..=100, found {n}"))),
        _ => Ok(()),
    }
}

/// `plugins` globs match names, so only the path-shaped lists are contained.
fn globs_ok(globs: &[String], key: &str, paths: bool) -> Result<(), SpecError> {
    for (i, g) in globs.iter().enumerate() {
        let at = format!("{key}[{i}]");
        if paths {
            safe_relative(g).map_err(|p| bad(&at, p))?;
        }
        globset::Glob::new(g).map_err(|e| bad(&at, e))?;
    }
    Ok(())
}

pub(crate) fn name_ok(name: &str) -> Result<(), String> {
    let b = name.as_bytes();
    let first = b.first().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let rest = b
        .iter()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-');
    if first && rest && b.len() <= 63 {
        Ok(())
    } else {
        Err(format!("{name:?} does not match ^[a-z0-9][a-z0-9-]{{0,62}}$"))
    }
}

/// `https://` or `file://`, no userinfo, no fragment, no whitespace or control
/// characters; the same grammar as grim's `mirror.source`.
pub(crate) fn repo_ok(repo: &str) -> Result<(), String> {
    let rest = repo
        .strip_prefix("https://")
        .or_else(|| repo.strip_prefix("file://"))
        .ok_or_else(|| format!("{repo:?} must start with https:// or file://"))?;
    if rest.is_empty() || repo.len() > 2048 || repo.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("{repo:?} is not a valid repository URL"));
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return Err("must not carry userinfo".to_owned());
    }
    if repo.contains('#') {
        return Err("must not carry a fragment".to_owned());
    }
    Ok(())
}

fn safe_relative(p: &str) -> Result<(), &'static str> {
    let drive = matches!(p.as_bytes(), [c, b':', ..] if c.is_ascii_alphabetic());
    if p.is_empty() {
        Err("must not be empty")
    } else if p.starts_with('/') || drive {
        Err("must be relative, not absolute")
    } else if p.contains('\\') {
        Err("must not contain a backslash")
    } else if p.contains('\0') {
        Err("must not contain NUL")
    } else if p.split('/').any(|s| s == "..") {
        Err("must not contain a `..` segment")
    } else {
        Ok(())
    }
}

fn expression(e: &str) -> Result<(), String> {
    spdx::Expression::parse(e)
        .map(|_| ())
        .map_err(|err| format!("{e:?} is not an SPDX expression: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "schema = 1\n[mirror]\nregistry = \"ghcr.io/acme/mirror\"\n";

    /// A valid entry; `extra` lines land inside it.
    fn doc(extra: &str) -> String {
        format!(
            "{HEAD}[[upstream]]\nname = \"a\"\nrepo = \"https://example.com/a\"\nskills = [\"skills/*\"]\n{extra}\n"
        )
    }

    fn err(text: &str) -> String {
        match parse(text) {
            Err(SpecError::Invalid(m)) => m,
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    fn err_at(text: &str, key: &str) {
        let m = err(text);
        assert!(m.starts_with(&format!("mirror.toml: {key}: ")), "{m}");
    }

    #[test]
    fn full_example_parses() {
        let text = "schema = 1\n[mirror]\nregistry = \"ghcr.io/acme/mirror\"\nlicenses = [\"MIT\", \"Apache-2.0\", \"CC0-1.0\"]\nbackfill = 2\n\
             [[upstream]]\nname = \"anthropic-skills\"\nrepo = \"file:///srv/skills\"\nref = \"main\"\ndialect = \"claude\"\nversion = \"snapshot\"\n\
             tag-pattern = '^v?(\\d+\\.\\d+\\.\\d+)$'\nbackfill = 3\nlicense = \"MIT OR Apache-2.0\"\n\
             unit-licenses = { \"skills/pdf\" = \"Apache-2.0\" }\nskills = [\"skills/*\"]\nagents = [\"agents/*.md\"]\nrules = [\"rules/*.md\"]\n\
             exclude = [\"skills/internal-*\"]\nmarketplace = \".claude-plugin/marketplace.json\"\nplugins = [\"python-*\"]\n\
             retire = { reason = \"moved\", replaced-by = \"ghcr.io/acme/pdf:1\", units = [\"skills/old-tool\"] }\n\
             [[upstream.bundle]]\nname = \"all\"\nmembers = [\"skills/*\"]\n";
        let f = parse(text).unwrap();
        let u = &f.upstream[0];
        assert_eq!(f.mirror.licenses.as_deref().map(<[String]>::len), Some(3));
        assert_eq!(u.git_ref.as_deref(), Some("main"));
        assert_eq!(u.dialect, Dialect::Claude);
        assert_eq!(u.version, VersionMode::Snapshot);
        assert_eq!(
            u.retire.as_ref().map(|r| r.units.clone()),
            Some(vec!["skills/old-tool".to_owned()])
        );
        assert_eq!(u.bundle[0].members, ["skills/*"]);
        assert_eq!(u.effective_backfill(&f.mirror), 3);
    }

    #[test]
    fn defaults_apply() {
        let f = parse(&doc("")).unwrap();
        let u = &f.upstream[0];
        assert_eq!(u.dialect, Dialect::Agentskills);
        assert_eq!(u.version, VersionMode::Auto);
        assert_eq!(
            (f.mirror.licenses.as_ref(), f.mirror.backfill, u.backfill),
            (None, None, None)
        );
        assert_eq!(u.effective_backfill(&f.mirror), 1);
        assert!(u.agents.is_empty() && u.exclude.is_empty() && u.bundle.is_empty() && u.unit_licenses.is_empty());
        let with_mirror = parse(&doc("").replace("[mirror]", "[mirror]\nbackfill = 5")).unwrap();
        assert_eq!(with_mirror.upstream[0].effective_backfill(&with_mirror.mirror), 5);
    }

    #[test]
    fn absent_file_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            load(&dir.path().join("mirror.toml")),
            Err(SpecError::Absent(_))
        ));
    }

    #[test]
    fn load_errors_name_the_given_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("other.toml");
        fs::write(&p, "schema = 1\nbogus = 1\n").unwrap();
        let Err(SpecError::Invalid(m)) = load(&p) else { panic!() };
        assert!(m.starts_with(&format!("{}: bogus: ", p.display())), "{m}");
        let d = dir.path().join("mirror.toml");
        fs::write(&d, "schema = 1\nbogus = 1\n").unwrap();
        let Err(SpecError::Invalid(m)) = load(&d) else { panic!() };
        assert!(m.starts_with(&format!("{}: bogus: ", d.display())), "{m}");
    }

    #[test]
    fn load_reads_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("mirror.toml");
        fs::write(&p, doc("")).unwrap();
        assert_eq!(load(&p).unwrap().upstream[0].name, "a");
    }

    #[test]
    fn syntax_error_names_the_line() {
        let m = err("schema = 1\n[mirror\n");
        assert!(m.starts_with("mirror.toml: line 2: "), "{m}");
    }

    #[test]
    fn unknown_top_level_key() {
        err_at(&format!("bogus = 1\n{}", doc("")), "bogus");
    }

    #[test]
    fn unknown_key_in_mirror() {
        err_at(&doc("").replace("[mirror]", "[mirror]\nbogus = 1"), "mirror.bogus");
    }

    #[test]
    fn unknown_key_in_upstream() {
        err_at(&doc("bogus = 1"), "upstream[0].bogus");
    }

    #[test]
    fn unknown_key_in_retire_and_bundle() {
        err_at(
            &doc("retire = { reason = \"x\", bogus = 1 }"),
            "upstream[0].retire.bogus",
        );
        err_at(
            &doc("[[upstream.bundle]]\nname = \"b\"\nmembers = [\"s/*\"]\nbogus = 1"),
            "upstream[0].bundle[0].bogus",
        );
    }

    #[test]
    fn schema_must_be_one() {
        err_at(&doc("").replace("schema = 1", "schema = 2"), "schema");
    }

    #[test]
    fn duplicate_upstream_name() {
        let two = format!(
            "{}[[upstream]]\nname = \"a\"\nrepo = \"https://example.com/b\"\nskills = [\"s/*\"]\n",
            doc("")
        );
        err_at(&two, "upstream[1].name");
    }

    #[test]
    fn duplicate_bundle_name() {
        let b = "[[upstream.bundle]]\nname = \"b\"\nmembers = [\"s/*\"]\n";
        err_at(&doc(&format!("{b}{b}")), "upstream[0].bundle[1].name");
    }

    #[test]
    fn name_grammar() {
        for bad in ["", "-a", "A", "a_b", &"a".repeat(64)] {
            err_at(
                &doc("").replace("name = \"a\"", &format!("name = \"{bad}\"")),
                "upstream[0].name",
            );
        }
        parse(&doc("").replace("name = \"a\"", &format!("name = \"{}\"", "a".repeat(63)))).unwrap();
    }

    #[test]
    fn repo_grammar() {
        assert!(repo_ok("https://github.com/o/r").is_ok() && repo_ok("file:///tmp/r").is_ok());
        for bad in [
            "http://x/y",
            "git@github.com:o/r",
            "ssh://x/y",
            "https://u:p@x/y",
            "https://",
            "https://x y",
        ] {
            assert!(repo_ok(bad).is_err(), "{bad}");
        }
        assert!(repo_ok(&format!("https://x/{}", "a".repeat(2048))).is_err());
    }

    #[test]
    fn ref_excludes_commit() {
        err_at(
            &doc(&format!("ref = \"main\"\ncommit = \"{}\"", "a".repeat(40))),
            "upstream[0].ref",
        );
    }

    #[test]
    fn commit_pin_must_be_40_hex() {
        err_at(&doc("commit = \"abc\""), "upstream[0].commit");
        parse(&doc(&format!("commit = \"{}\"", "a".repeat(40)))).unwrap();
    }

    #[test]
    fn tag_pattern_must_compile() {
        err_at(&doc("tag-pattern = '('"), "upstream[0].tag-pattern");
    }

    #[test]
    fn tag_pattern_needs_a_group() {
        err_at(&doc("tag-pattern = '^v1$'"), "upstream[0].tag-pattern");
    }

    #[test]
    fn marketplace_needs_plugins() {
        err_at(&doc("marketplace = \"m.json\""), "upstream[0].plugins");
    }

    #[test]
    fn plugins_need_marketplace() {
        err_at(&doc("plugins = [\"p\"]"), "upstream[0].marketplace");
    }

    #[test]
    fn entry_needs_a_kind_list_or_marketplace() {
        err_at(&doc("").replace("skills = [\"skills/*\"]", ""), "upstream[0]");
        // a marketplace alone is enough
        parse(&doc("marketplace = \"m.json\"\nplugins = [\"p\"]").replace("skills = [\"skills/*\"]", "")).unwrap();
    }

    #[test]
    fn licenses_ids_must_be_on_the_spdx_list() {
        let l = |ids: &str| doc("").replace("[mirror]", &format!("[mirror]\nlicenses = [{ids}]"));
        err_at(&l("\"MIT\", \"Not-A-Licence\""), "mirror.licenses[1]");
        parse(&l("\"MIT\", \"Apache-2.0\"")).unwrap();
    }

    #[test]
    fn licenses_refuse_license_ref() {
        err_at(
            &doc("").replace("[mirror]", "[mirror]\nlicenses = [\"LicenseRef-x\"]"),
            "mirror.licenses[0]",
        );
    }

    #[test]
    fn license_expression_must_parse() {
        err_at(&doc("license = \"MIT OR\""), "upstream[0].license");
        err_at(
            &doc("unit-licenses = { \"skills/a\" = \"nope nope\" }"),
            "upstream[0].unit-licenses.skills/a",
        );
    }

    #[test]
    fn backfill_range() {
        for bad in ["0", "101"] {
            err_at(&doc(&format!("backfill = {bad}")), "upstream[0].backfill");
            err_at(
                &doc("").replace("[mirror]", &format!("[mirror]\nbackfill = {bad}")),
                "mirror.backfill",
            );
        }
        parse(&doc("backfill = 100")).unwrap();
        parse(&doc("backfill = 1")).unwrap();
    }

    #[test]
    fn unit_license_paths_are_contained() {
        for bad in ["/abs", "a/../b", "a\\\\b"] {
            let m = err(&doc(&format!("unit-licenses = {{ \"{bad}\" = \"MIT\" }}")));
            assert!(m.starts_with("mirror.toml: upstream[0].unit-licenses."), "{bad}: {m}");
        }
    }

    #[test]
    fn retire_units_are_contained() {
        for bad in ["/abs", "a/../b", "a\\\\b"] {
            err_at(
                &doc(&format!("retire = {{ reason = \"x\", units = [\"{bad}\"] }}")),
                "upstream[0].retire.units[0]",
            );
        }
    }

    #[test]
    fn globs_are_contained() {
        for list in ["skills", "agents", "rules", "exclude"] {
            for bad in ["/abs/*", "../x", "a\\\\b"] {
                let text = doc(&format!("{list} = [\"{bad}\"]")).replace("skills = [\"skills/*\"]\n", "");
                let text = if list == "skills" {
                    text
                } else {
                    text.replace("name = \"a\"", "name = \"a\"\nskills = [\"s/*\"]")
                };
                err_at(&text, &format!("upstream[0].{list}[0]"));
            }
        }
        err_at(
            &doc("[[upstream.bundle]]\nname = \"b\"\nmembers = [\"../x\"]"),
            "upstream[0].bundle[0].members[0]",
        );
    }

    #[test]
    fn glob_must_compile() {
        err_at(&doc("").replace("skills/*", "["), "upstream[0].skills[0]");
    }

    #[test]
    fn marketplace_path_is_contained() {
        for bad in ["/m.json", "../m.json", "a\\\\m.json"] {
            err_at(
                &doc(&format!("marketplace = \"{bad}\"\nplugins = [\"p\"]")),
                "upstream[0].marketplace",
            );
        }
    }

    #[test]
    fn repo_must_match_the_grammar() {
        for bad in [
            "http://example.com/a",
            "ssh://git@example.com/a",
            "git@example.com:a/b",
            "https://user:pw@example.com/a",
            "https://user@example.com/a",
            "https://",
            "https://example.com/a b",
            "",
        ] {
            err_at(&doc("").replace("https://example.com/a", bad), "upstream[0].repo");
        }
        parse(&doc("").replace("https://example.com/a", "file:///tmp/r")).unwrap();
        for bad in ["file://u@host/r", "file:///tmp/r#x", "https://example.com/a#x"] {
            assert!(repo_ok(bad).is_err(), "{bad}");
        }
        parse(&doc("").replace("https://example.com/a", "https://example.com/a@b")).unwrap();
    }

    #[test]
    fn wrong_type_is_invalid_not_a_panic() {
        let m = err(&doc("skills = \"x\""));
        assert!(m.starts_with("mirror.toml: line "), "{m}");
    }

    #[test]
    fn missing_required_key_is_invalid() {
        let m = err("schema = 1\n[[upstream]]\nname = \"a\"\n");
        assert!(m.starts_with("mirror.toml: "), "{m}");
    }
}
