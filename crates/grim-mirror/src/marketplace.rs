// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Enumeration of mirrorable units from a Claude plugin marketplace.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use serde_json::{Map, Value};

use crate::enumerate::{
    BundleDecl, EnumError, Enumeration, ExternalRepo, Seed, Skipped, Tree, Unreadable, agent_names, declared_bundles,
    join, leaf, name_globs, normalise_rel, stem,
};
use crate::git::{self, GitError, Repo};
use crate::model::{Commit, Kind};
use crate::report::Reason;
use crate::spec::{Upstream, name_ok, repo_ok};

/// Largest marketplace document read; upstream controls its size.
const MAX_MARKETPLACE: usize = 4 * 1024 * 1024;
const MAX_MANIFEST: usize = 1024 * 1024;
const MAX_PLUGINS: usize = 2000;
/// Longest component list a manifest or entry may declare.
const MAX_LIST: usize = 256;
const MAX_STRING: usize = 512;

/// Where a plugin's files live, before anything is fetched.
#[derive(Debug, PartialEq, Eq)]
enum Source {
    Local {
        root: String,
        at_marketplace_root: bool,
    },
    Remote {
        url: String,
        root: String,
        pin: Pin,
    },
    /// Not mirrored: `npm`, `archive`, a non-https URL, a malformed object.
    Unsupported,
    Unsafe,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Pin {
    sha: Option<String>,
    git_ref: Option<String>,
}

/// `owner/repo` with plain characters, neither segment `.` or `..`.
fn shorthand(s: &str) -> bool {
    let seg = |p: &str| {
        !matches!(p, "" | "." | "..")
            && p.bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
    };
    s.split_once('/').is_some_and(|(o, r)| seg(o) && seg(r))
}

/// A URL read from marketplace content must be `https://`; `file://` only when the
/// operator's own entry is a `file://` repository, so a remote marketplace cannot
/// aim the mirror at the host's filesystem.
fn content_url_ok(s: &str, allow_file: bool) -> bool {
    (s.starts_with("https://") || (allow_file && s.starts_with("file://"))) && repo_ok(s).is_ok()
}

fn repo_url(s: &str, allow_file: bool) -> Option<String> {
    if content_url_ok(s, allow_file) {
        Some(s.to_owned())
    } else if shorthand(s) {
        Some(format!("https://github.com/{s}"))
    } else {
        None
    }
}

fn pin_of(o: &Map<String, Value>) -> Option<Pin> {
    let sha = match o.get("sha").and_then(Value::as_str) {
        Some(s) if Commit::new(s).is_ok() => Some(s.to_owned()),
        Some(_) => return None,
        None => None,
    };
    let git_ref = match o.get("ref").and_then(Value::as_str) {
        Some(r) if r.len() <= 255 && !r.chars().any(|c| c.is_control() || c.is_whitespace()) => Some(r.to_owned()),
        Some(_) => return None,
        None => None,
    };
    // A full sha names the commit, so a ref beside it adds nothing.
    Some(if sha.is_some() {
        Pin { sha, git_ref: None }
    } else {
        Pin { sha, git_ref }
    })
}

fn resolve_source(v: Option<&Value>, mroot: &str, plugin_root: Option<&str>, allow_file: bool) -> Source {
    match v {
        Some(Value::String(s)) => local_source(s, mroot, plugin_root),
        Some(Value::Object(o)) => remote_source(o, allow_file),
        _ => Source::Unsupported,
    }
}

fn local_source(s: &str, mroot: &str, plugin_root: Option<&str>) -> Source {
    let Some(rel) = normalise_rel(s) else {
        return Source::Unsafe;
    };
    let bare = !s.contains('/') && s != ".";
    if bare {
        let base = match plugin_root {
            Some(p) => match normalise_rel(p) {
                Some(n) => n,
                None => return Source::Unsafe,
            },
            None => String::new(),
        };
        return Source::Local {
            root: join(mroot, &join(&base, &rel)),
            at_marketplace_root: false,
        };
    }
    Source::Local {
        root: join(mroot, &rel),
        at_marketplace_root: rel.is_empty(),
    }
}

fn remote_source(o: &Map<String, Value>, allow_file: bool) -> Source {
    let text = |k: &str| o.get(k).and_then(Value::as_str);
    let (url, root) = match text("source") {
        Some("github") => (
            text("repo")
                .filter(|r| shorthand(r))
                .and_then(|r| repo_url(r, allow_file)),
            String::new(),
        ),
        Some("url") => (
            text("url").filter(|u| content_url_ok(u, allow_file)).map(str::to_owned),
            String::new(),
        ),
        Some("git-subdir") => {
            let Some(path) = text("path") else {
                return Source::Unsupported;
            };
            match normalise_rel(path) {
                Some(root) => (text("url").and_then(|u| repo_url(u, allow_file)), root),
                None => return Source::Unsafe,
            }
        }
        _ => return Source::Unsupported,
    };
    match (url, pin_of(o)) {
        (Some(url), Some(pin)) => Source::Remote { url, root, pin },
        _ => Source::Unsupported,
    }
}

/// A repository at one commit, with its tree listed.
struct Loaded {
    repo: Repo,
    commit: Commit,
    tree: Tree,
    external: Option<ExternalRepo>,
}

fn blob(l: &Loaded, path: &str, cap: usize) -> Result<Option<Vec<u8>>, EnumError> {
    let Some(entry) = l.tree.file(path) else {
        return Ok(None);
    };
    let mut blobs = l.repo.cat_file_batch(std::slice::from_ref(&entry.oid))?;
    let bytes = blobs.pop().unwrap_or_default();
    if bytes.len() > cap {
        return Err(EnumError::Invalid(format!("{path}: larger than {cap} bytes")));
    }
    Ok(Some(bytes))
}

/// Strings of a manifest key given as a string or an array of strings, each
/// bounded and normalised against the plugin root. `Err` for anything odd.
fn path_list(m: &Map<String, Value>, key: &str) -> Result<Vec<String>, ()> {
    let raw: Vec<&str> = match m.get(key) {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(a)) if a.len() <= MAX_LIST => a.iter().filter_map(Value::as_str).collect(),
        _ => return Err(()),
    };
    raw.into_iter()
        .map(|s| if s.len() > MAX_STRING { None } else { normalise_rel(s) })
        .collect::<Option<_>>()
        .ok_or(())
}

fn declared(m: &Map<String, Value>, key: &str) -> bool {
    match m.get(key) {
        None | Some(Value::Null | Value::Bool(false)) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(_) => true,
    }
}

const ENTRY_COMPONENTS: [&str; 5] = ["skills", "agents", "commands", "hooks", "mcpServers"];
const UNSUPPORTED_KEYS: [(&str, &str); 3] = [
    ("outputStyles", "output-styles"),
    ("lspServers", "lsp-servers"),
    ("monitors", "monitors"),
];

/// The components the effective manifest declares.
#[derive(Default)]
struct Declared {
    skills: Vec<String>,
    agents: Vec<String>,
    commands: bool,
    hooks: bool,
    mcp: bool,
    other: BTreeSet<&'static str>,
}

impl Declared {
    fn absorb(&mut self, m: &Map<String, Value>) -> Result<(), ()> {
        self.skills.extend(path_list(m, "skills")?);
        self.agents.extend(path_list(m, "agents")?);
        self.commands |= declared(m, "commands");
        self.hooks |= declared(m, "hooks");
        self.mcp |= declared(m, "mcpServers");
        for (key, label) in UNSUPPORTED_KEYS {
            if declared(m, key) {
                self.other.insert(label);
            }
        }
        Ok(())
    }
}

enum Plan {
    /// Claude would refuse to load this plugin, or its manifest is unreadable.
    Conflict,
    Unsafe,
    Parts(Parts),
}

#[derive(Default)]
struct Parts {
    skills: Vec<(String, String)>,
    agents: Vec<(String, String)>,
    skipped: Vec<(Kind, String, Reason)>,
    warnings: Vec<(String, String)>,
}

fn components(
    l: &Loaded,
    root: &str,
    plugin: &str,
    entry: &Map<String, Value>,
    entry_at_marketplace_root: bool,
) -> Result<Plan, EnumError> {
    let strict = entry.get("strict").and_then(Value::as_bool).unwrap_or(true);
    let mut parts = Parts::default();
    let manifest = match blob(l, &join(root, ".claude-plugin/plugin.json"), MAX_MANIFEST)? {
        None => None,
        Some(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(m)) => Some(m),
            _ => {
                parts.warnings.push((
                    "plugin-manifest-invalid".to_owned(),
                    format!("plugin {plugin}: .claude-plugin/plugin.json is not a JSON object"),
                ));
                return Ok(Plan::Conflict);
            }
        },
    };

    let entry_declares = ENTRY_COMPONENTS.iter().any(|k| declared(entry, k));
    let mut eff = Declared::default();
    let absorbed = match (&manifest, strict) {
        (Some(p), true) => eff.absorb(p).and_then(|()| eff.absorb(entry)),
        (Some(_), false) if entry_declares => return Ok(Plan::Conflict),
        (Some(p), false) => eff.absorb(p),
        (None, _) => eff.absorb(entry),
    };
    if absorbed.is_err() {
        return Ok(Plan::Unsafe);
    }
    let Ok(entry_skills) = path_list(entry, "skills") else {
        return Ok(Plan::Unsafe);
    };

    let mut skills: BTreeMap<String, String> = BTreeMap::new();
    let mut add_skill = |dir: &str| {
        let (name, path) = if dir.is_empty() {
            (plugin, ".")
        } else {
            (leaf(dir), dir)
        };
        skills.insert(path.to_owned(), name.to_owned());
    };
    let has_skill_md = |dir: &str| l.tree.file(&join(dir, "SKILL.md")).is_some();
    if !entry_at_marketplace_root || entry_skills.is_empty() {
        for d in l.tree.child_dirs(&join(root, "skills")) {
            if has_skill_md(d) {
                add_skill(d);
            }
        }
    }
    for p in &eff.skills {
        let full = join(root, p);
        if has_skill_md(&full) {
            add_skill(&full);
        } else {
            for d in l.tree.child_dirs(&full) {
                if has_skill_md(d) {
                    add_skill(d);
                }
            }
        }
    }
    if has_skill_md(root) {
        add_skill(root);
    }
    parts.skills = skills.into_iter().map(|(path, name)| (name, path)).collect();

    let mut agent_files = Vec::new();
    if eff.agents.is_empty() {
        agent_files.extend(
            l.tree
                .child_files(&join(root, "agents"))
                .into_iter()
                .filter(|e| e.path.ends_with(".md") && e.mode != "160000"),
        );
    } else {
        for p in &eff.agents {
            let full = join(root, p);
            match l.tree.file(&full) {
                Some(e) if e.mode == "160000" && e.path.ends_with(".md") => {
                    parts
                        .skipped
                        .push((Kind::Agent, stem(&e.path), Reason::UnsupportedEntry));
                }
                Some(e) if e.path.ends_with(".md") => agent_files.push(e),
                Some(_) => {}
                None if l.tree.is_dir(&full) => parts.warnings.push((
                    "agents-dir-ignored".to_owned(),
                    format!("plugin {plugin}: agents path {p} is a directory; list .md files"),
                )),
                None => {}
            }
        }
    }
    agent_files.sort_by(|a, b| a.path.cmp(&b.path));
    agent_files.dedup_by(|a, b| a.path == b.path);
    let names = agent_names(&l.repo, &agent_files)?;
    parts.agents = names
        .into_iter()
        .zip(&agent_files)
        .map(|(name, e)| (name, e.path.clone()))
        .collect();

    let file = |p: &str| l.tree.file(&join(root, p)).is_some();
    let mut skip = |kind, name: &str, reason| parts.skipped.push((kind, name.to_owned(), reason));
    if eff.commands || l.tree.has_files_under(&join(root, "commands")) {
        skip(Kind::Command, "commands", Reason::KindUnsupported);
    }
    if eff.hooks || file("hooks/hooks.json") {
        skip(Kind::Hook, "hooks", Reason::KindUnsupported);
    }
    if eff.mcp || file(".mcp.json") {
        skip(Kind::Mcp, "mcp", Reason::KindUnsupported);
    }
    if file("settings.json") {
        skip(Kind::Settings, "settings", Reason::KindUnsupported);
    }
    for label in &eff.other {
        skip(Kind::Component, label, Reason::ComponentUnsupported);
    }
    if l.tree.has_files_under(&join(root, "bin")) {
        skip(Kind::Component, "bin", Reason::ComponentUnsupported);
    }
    Ok(Plan::Parts(parts))
}

fn marketplace_root(path: &str) -> String {
    let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
    if leaf(dir) == ".claude-plugin" {
        dir.rsplit_once('/').map_or("", |(p, _)| p).to_owned()
    } else {
        dir.to_owned()
    }
}

/// Whether a failed fetch shows the repository or its pin is gone: the remote
/// answered that it holds no such repository, commit or ref. Every other
/// failure (network, timeout, unreadable objects) is a fault on the way, and
/// reading it as "gone" would let a scheduled `sync --fail-on infra` exit 0.
fn repository_absent(e: &GitError) -> bool {
    match e {
        GitError::CommitUnreachable(_) | GitError::RefUnknown(_) => true,
        GitError::Failed { stderr, .. } => {
            let s = stderr.to_ascii_lowercase();
            s.contains("repository not found")
                || s.contains("does not appear to be a git repository")
                || (s.contains("fatal: repository '") && s.contains("' not found"))
        }
        GitError::Spawn(_) | GitError::Timeout(_) | GitError::TooLarge(_) => false,
    }
}

/// Fetches an external plugin repository at its pin.
async fn load_remote(url: &str, pin: &Pin, work: &Path) -> Result<Loaded, EnumError> {
    let repo = git::fetch(url, work).await?;
    let commit = repo.candidate(pin.sha.as_deref(), pin.git_ref.as_deref())?;
    let tree = Tree::load(&repo, &commit)?;
    let external = Some(ExternalRepo {
        url: url.to_owned(),
        dir: repo.dir.clone(),
    });
    Ok(Loaded {
        repo,
        commit,
        tree,
        external,
    })
}

/// Units of every plugin selected by the entry's `plugins` globs, external
/// `git-subdir` repositories fetched under `work`.
///
/// Each plugin yields skills and agents from its manifest, one `Skipped` row
/// per component not mirrored, and one bundle of its units. A plugin the
/// mirror cannot read is a `Skipped` row of kind `bundle` with no bundle,
/// among them one whose external repository is unavailable or whose manifest
/// is unreadable or oversize; the other plugins still list.
///
/// # Errors
///
/// A git failure in the marketplace repository or an unreadable marketplace
/// document.
pub async fn plugins(repo: &Repo, upstream: &Upstream, commit: &Commit, work: &Path) -> Result<Enumeration, EnumError> {
    let mut out = Enumeration::default();
    let Some(path) = &upstream.marketplace else {
        return Ok(out);
    };
    let path = normalise_rel(path).ok_or_else(|| EnumError::Invalid(format!("marketplace path {path:?} is unsafe")))?;
    let local = Loaded {
        repo: repo.clone(),
        commit: commit.clone(),
        tree: Tree::load(repo, commit)?,
        external: None,
    };
    let bytes = blob(&local, &path, MAX_MARKETPLACE)?
        .ok_or_else(|| EnumError::Invalid(format!("marketplace file {path} not found at {commit}")))?;
    let doc: Value =
        serde_json::from_slice(&bytes).map_err(|e| EnumError::Invalid(format!("marketplace {path}: {e}")))?;
    let entries = doc
        .get("plugins")
        .and_then(Value::as_array)
        .filter(|a| a.len() <= MAX_PLUGINS)
        .ok_or_else(|| EnumError::Invalid(format!("marketplace {path}: `plugins` is missing or too long")))?;
    let plugin_root = doc.pointer("/metadata/pluginRoot").and_then(Value::as_str);
    let mroot = marketplace_root(&path);
    let selected = name_globs(&upstream.plugins)?;
    let allow_file = upstream.repo.starts_with("file://");

    let mut seen = BTreeSet::new();
    let mut remote: HashMap<String, Loaded> = HashMap::new();
    for e in entries {
        let Some(entry) = e.as_object() else { continue };
        let Some(name) = entry
            .get("name")
            .and_then(Value::as_str)
            .filter(|n| n.len() <= MAX_STRING)
        else {
            continue;
        };
        if !selected.is_match(name) {
            continue;
        }
        let skip_plugin = |out: &mut Enumeration, kind, reason| {
            out.skipped.push(Skipped {
                kind,
                name: name.to_owned(),
                plugin: Some(name.to_owned()),
                reason,
            });
        };
        if !seen.insert(name.to_owned()) {
            skip_plugin(&mut out, Kind::Bundle, Reason::NameCollision);
            continue;
        }
        if name_ok(name).is_err() {
            skip_plugin(&mut out, Kind::Bundle, Reason::InvalidName);
            continue;
        }
        let (loaded, root, at_root) = match resolve_source(entry.get("source"), &mroot, plugin_root, allow_file) {
            Source::Unsupported => {
                skip_plugin(&mut out, Kind::Component, Reason::SourceUnsupported);
                continue;
            }
            Source::Unsafe => {
                skip_plugin(&mut out, Kind::Bundle, Reason::UnsafePath);
                continue;
            }
            Source::Local {
                root,
                at_marketplace_root,
            } => (&local, root, at_marketplace_root),
            Source::Remote { url, root, pin } => {
                let key = format!("{url}\0{:?}\0{:?}", pin.sha, pin.git_ref);
                if !remote.contains_key(&key) {
                    match load_remote(&url, &pin, work).await {
                        Ok(l) => {
                            remote.insert(key.clone(), l);
                        }
                        Err(e) => {
                            out.warnings
                                .push(("plugin-unavailable".to_owned(), format!("plugin {name}: {e}")));
                            match e {
                                EnumError::Git(error) if !repository_absent(&error) => out.failed.push(Unreadable {
                                    plugin: name.to_owned(),
                                    error,
                                }),
                                _ => skip_plugin(&mut out, Kind::Bundle, Reason::UpstreamUnavailable),
                            }
                            continue;
                        }
                    }
                }
                (&remote[&key], root, false)
            }
        };
        let plan = match components(loaded, &root, name, entry, at_root) {
            Ok(plan) => plan,
            Err(e @ (EnumError::Invalid(_) | EnumError::Git(GitError::TooLarge(_)))) => {
                out.warnings
                    .push(("plugin-unreadable".to_owned(), format!("plugin {name}: {e}")));
                skip_plugin(&mut out, Kind::Bundle, Reason::UnsupportedEntry);
                continue;
            }
            Err(EnumError::Git(error)) => {
                out.warnings
                    .push(("plugin-unreadable".to_owned(), format!("plugin {name}: {error}")));
                out.failed.push(Unreadable {
                    plugin: name.to_owned(),
                    error,
                });
                continue;
            }
        };
        let parts = match plan {
            Plan::Conflict => {
                skip_plugin(&mut out, Kind::Bundle, Reason::PluginConflict);
                continue;
            }
            Plan::Unsafe => {
                skip_plugin(&mut out, Kind::Bundle, Reason::UnsafePath);
                continue;
            }
            Plan::Parts(p) => p,
        };
        let seed = Seed {
            upstream: upstream.name.clone(),
            plugin: Some(name.to_owned()),
            commit: loaded.commit.clone(),
            external: loaded.external.clone(),
            plugin_root: Some(root.trim_matches('/').to_owned()).filter(|r| !r.is_empty() && r != "."),
        };
        let mut members = Vec::new();
        for (kind, units) in [(Kind::Skill, &parts.skills), (Kind::Agent, &parts.agents)] {
            for (unit, path) in units {
                members.extend(seed.admit(&mut out, kind, unit, path));
            }
        }
        for (kind, label, reason) in parts.skipped {
            out.skipped.push(Skipped {
                kind,
                name: label,
                plugin: Some(name.to_owned()),
                reason,
            });
        }
        out.warnings.extend(parts.warnings);
        out.bundles.push(BundleDecl {
            name: name.to_owned(),
            plugin: Some(name.to_owned()),
            members,
        });
    }
    if upstream.skills.is_empty() && upstream.agents.is_empty() && upstream.rules.is_empty() {
        out.bundles.extend(declared_bundles(upstream, &out.units)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::enumerate::fixture::{Fx, strs, upstream};

    fn mk(plugins: &Value) -> Fx {
        let fx = Fx::new();
        fx.write(
            ".claude-plugin/marketplace.json",
            &json!({"metadata": {"pluginRoot": "./plugins"}, "plugins": plugins}).to_string(),
        );
        fx
    }

    async fn run(fx: &Fx, names: &[&str]) -> Enumeration {
        let repo = fx.url();
        let c = fx.save();
        run_at(fx, names, &repo, &c).await
    }

    async fn run_as_https(fx: &Fx, names: &[&str]) -> Enumeration {
        let c = fx.save();
        run_at(fx, names, "https://example.com/r", &c).await
    }

    async fn run_at(fx: &Fx, names: &[&str], repo: &str, c: &Commit) -> Enumeration {
        let mut up = upstream();
        up.repo = repo.to_owned();
        up.marketplace = Some(".claude-plugin/marketplace.json".to_owned());
        up.plugins = strs(names);
        let work = tempfile::tempdir().unwrap();
        plugins(&fx.repo(), &up, c, work.path()).await.unwrap()
    }

    fn unit_paths(e: &Enumeration) -> Vec<(Kind, String, String)> {
        e.units
            .iter()
            .map(|u| (u.kind, u.name.clone(), u.path.clone()))
            .collect()
    }

    fn skipped(e: &Enumeration) -> Vec<(Kind, String, Reason)> {
        e.skipped.iter().map(|s| (s.kind, s.name.clone(), s.reason)).collect()
    }

    #[test]
    fn sources_resolve_per_the_table() {
        let s = |v: Value, pr: Option<&str>| resolve_source(Some(&v), "m", pr, false);
        let local = |root: &str, at: bool| Source::Local {
            root: root.to_owned(),
            at_marketplace_root: at,
        };
        assert_eq!(s(json!("a"), None), local("m/a", false));
        assert_eq!(s(json!("a"), Some("./plugins")), local("m/plugins/a", false));
        assert_eq!(s(json!("./a"), Some("./plugins")), local("m/a", false));
        assert_eq!(s(json!("x/y"), Some("plugins")), local("m/x/y", false));
        assert_eq!(s(json!("."), Some("plugins")), local("m", true));
        assert_eq!(s(json!("./"), None), local("m", true));
        for bad in ["../x", "a/../b", "/abs", "a\\b", "", "https://h/x"] {
            assert_eq!(s(json!(bad), None), Source::Unsafe, "{bad}");
        }
        assert_eq!(s(json!("a"), Some("../up")), Source::Unsafe);
    }

    #[test]
    fn object_sources_follow_the_url_grammar() {
        let s = |v: Value| resolve_source(Some(&v), "", None, true);
        let remote = |url: &str, root: &str, sha: Option<&str>, git_ref: Option<&str>| Source::Remote {
            url: url.to_owned(),
            root: root.to_owned(),
            pin: Pin {
                sha: sha.map(str::to_owned),
                git_ref: git_ref.map(str::to_owned),
            },
        };
        assert_eq!(
            s(json!({"source": "github", "repo": "o/r"})),
            remote("https://github.com/o/r", "", None, None)
        );
        assert_eq!(
            s(json!({"source": "url", "url": "https://h/x.git", "ref": "v1"})),
            remote("https://h/x.git", "", None, Some("v1"))
        );
        assert_eq!(
            s(json!({"source": "url", "url": "file:///tmp/r"})),
            remote("file:///tmp/r", "", None, None)
        );
        let sha = "a".repeat(40);
        assert_eq!(
            s(json!({"source": "git-subdir", "url": "o/r", "path": "./sub/dir", "sha": sha, "ref": "main"})),
            remote("https://github.com/o/r", "sub/dir", Some(&sha), None)
        );
        for unsupported in [
            json!({"source": "url", "url": "git@github.com:o/r.git"}),
            json!({"source": "url", "url": "http://h/r"}),
            json!({"source": "url", "url": "ssh://h/r"}),
            json!({"source": "git-subdir", "url": "ssh://h/r", "path": "p"}),
            json!({"source": "git-subdir", "url": "https://h/r"}),
            json!({"source": "github", "repo": "../x"}),
            json!({"source": "github", "repo": "o/r", "sha": "short"}),
            json!({"source": "npm", "package": "x"}),
            json!({"source": "archive", "url": "https://h/a.tgz"}),
            json!({"source": "command"}),
            json!({}),
        ] {
            assert_eq!(s(unsupported.clone()), Source::Unsupported, "{unsupported}");
        }
        assert_eq!(
            s(json!({"source": "git-subdir", "url": "o/r", "path": "../x"})),
            Source::Unsafe
        );
    }

    #[tokio::test]
    async fn relative_plugin_yields_skills_agents_and_one_bundle() {
        let fx = mk(&json!([{"name": "a", "source": "./plugins/a"}, {"name": "other", "source": "./plugins/o"}]));
        fx.write("plugins/a/skills/x/SKILL.md", "x")
            .write("plugins/a/skills/loose.md", "x")
            .write("plugins/a/agents/rev.md", "---\nname: reviewer\n---\n")
            .write("plugins/a/agents/notes.txt", "x")
            .write("plugins/o/skills/y/SKILL.md", "x");
        let e = run(&fx, &["a"]).await;
        assert_eq!(
            unit_paths(&e),
            [
                (Kind::Skill, "x".into(), "plugins/a/skills/x".into()),
                (Kind::Agent, "reviewer".into(), "plugins/a/agents/rev.md".into())
            ]
        );
        assert!(
            e.units
                .iter()
                .all(|u| u.plugin.as_deref() == Some("a") && u.external.is_none())
        );
        assert_eq!(
            e.bundles,
            [BundleDecl {
                name: "a".into(),
                plugin: Some("a".into()),
                members: vec!["plugins/a/skills/x".into(), "plugins/a/agents/rev.md".into()]
            }]
        );
        assert!(e.skipped.is_empty());
    }

    #[tokio::test]
    async fn plugin_glob_selects_and_pluginroot_applies_to_bare_names_only() {
        let fx = mk(&json!([
            {"name": "py-a", "source": "py-a"},
            {"name": "py-b", "source": "./py-b"},
            {"name": "skip", "source": "skip"}
        ]));
        fx.write("plugins/py-a/skills/s/SKILL.md", "x")
            .write("py-b/skills/t/SKILL.md", "x")
            .write("plugins/skip/skills/u/SKILL.md", "x");
        let e = run(&fx, &["py-*"]).await;
        let paths: Vec<_> = e.units.iter().map(|u| u.path.as_str()).collect();
        assert_eq!(paths, ["plugins/py-a/skills/s", "py-b/skills/t"]);
        assert_eq!(e.bundles.len(), 2);
    }

    #[tokio::test]
    async fn marketplace_root_is_the_dir_holding_claude_plugin() {
        let fx = Fx::new();
        fx.write(
            "sub/.claude-plugin/marketplace.json",
            &json!({"plugins": [{"name": "a", "source": "./a"}]}).to_string(),
        )
        .write("sub/a/skills/x/SKILL.md", "x");
        let c = fx.save();
        let mut up = upstream();
        up.marketplace = Some("sub/.claude-plugin/marketplace.json".into());
        up.plugins = strs(&["*"]);
        let w = tempfile::tempdir().unwrap();
        let e = plugins(&fx.repo(), &up, &c, w.path()).await.unwrap();
        assert_eq!(e.units[0].path, "sub/a/skills/x");
    }

    #[tokio::test]
    async fn strict_true_unions_manifest_and_entry_components() {
        let fx = mk(&json!([{"name": "a", "source": "./a", "skills": ["./more"]}]));
        fx.write(
            "a/.claude-plugin/plugin.json",
            &json!({"skills": ["./extra"]}).to_string(),
        )
        .write("a/skills/d/SKILL.md", "x")
        .write("a/extra/e/SKILL.md", "x")
        .write("a/more/f/SKILL.md", "x");
        let e = run(&fx, &["a"]).await;
        let names: Vec<_> = e.units.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(names, ["e", "f", "d"], "default scan plus both lists, in path order");
    }

    #[tokio::test]
    async fn without_manifest_the_entry_is_the_manifest() {
        let fx = mk(&json!([{"name": "a", "source": "./a", "agents": ["./custom/z.md"]}]));
        fx.write("a/agents/ignored.md", "x").write("a/custom/z.md", "x");
        let e = run(&fx, &["a"]).await;
        assert_eq!(unit_paths(&e), [(Kind::Agent, "z".into(), "a/custom/z.md".into())]);
    }

    #[tokio::test]
    async fn strict_false_with_manifest_and_entry_components_conflicts() {
        let fx = mk(&json!([
            {"name": "bad", "source": "./bad", "strict": false, "skills": ["./s"]},
            {"name": "ok", "source": "./ok", "strict": false}
        ]));
        fx.write("bad/.claude-plugin/plugin.json", "{}")
            .write("bad/s/x/SKILL.md", "x")
            .write("ok/.claude-plugin/plugin.json", &json!({"skills": ["./s"]}).to_string())
            .write("ok/s/y/SKILL.md", "x");
        let e = run(&fx, &["*"]).await;
        assert_eq!(skipped(&e), [(Kind::Bundle, "bad".into(), Reason::PluginConflict)]);
        assert_eq!(e.units.len(), 1, "the non-conflicting plugin still lists");
        assert_eq!(e.bundles.len(), 1, "no bundle for the conflict");
        assert_eq!(e.bundles[0].name, "ok");
    }

    #[tokio::test]
    async fn skills_add_but_a_root_source_entry_list_replaces() {
        let fx = mk(&json!([
            {"name": "sub", "source": "./sub", "skills": ["./extra"]},
            {"name": "root", "source": ".", "skills": ["./extra"]}
        ]));
        fx.write("sub/skills/a/SKILL.md", "x")
            .write("sub/extra/b/SKILL.md", "x")
            .write("skills/c/SKILL.md", "x")
            .write("extra/d/SKILL.md", "x");
        let e = run(&fx, &["*"]).await;
        let by_plugin = |p: &str| -> Vec<String> {
            e.units
                .iter()
                .filter(|u| u.plugin.as_deref() == Some(p))
                .map(|u| u.name.clone())
                .collect()
        };
        assert_eq!(
            by_plugin("sub"),
            ["b", "a"],
            "listed path and default scan, in path order"
        );
        assert_eq!(by_plugin("root"), ["d"], "default skills/ is replaced");
    }

    #[tokio::test]
    async fn declared_agents_replace_the_default_and_directories_are_ignored() {
        let fx = mk(&json!([{"name": "a", "source": "./a"}]));
        fx.write(
            "a/.claude-plugin/plugin.json",
            &json!({"agents": ["./one.md", "./dir"]}).to_string(),
        )
        .write("a/agents/default.md", "x")
        .write("a/one.md", "x")
        .write("a/dir/two.md", "x");
        let e = run(&fx, &["a"]).await;
        assert_eq!(unit_paths(&e), [(Kind::Agent, "one".into(), "a/one.md".into())]);
        assert_eq!(e.warnings.len(), 1);
        assert_eq!(e.warnings[0].0, "agents-dir-ignored");
    }

    #[tokio::test]
    async fn a_plugin_root_holding_skill_md_is_one_skill() {
        let fx = mk(&json!([{"name": "solo", "source": "./solo"}, {"name": "whole", "source": "."}]));
        fx.write("solo/SKILL.md", "x").write("SKILL.md", "x");
        let e = run(&fx, &["*"]).await;
        assert_eq!(
            unit_paths(&e),
            [
                (Kind::Skill, "solo".into(), "solo".into()),
                (Kind::Skill, "whole".into(), ".".into())
            ]
        );
    }

    #[tokio::test]
    async fn non_mirrored_components_are_one_skipped_row_each() {
        let fx = mk(&json!([{"name": "a", "source": "./a"}]));
        fx.write("a/skills/s/SKILL.md", "x")
            .write("a/commands/c.md", "x")
            .write("a/hooks/hooks.json", "{}")
            .write("a/.mcp.json", "{}")
            .write("a/settings.json", "{}")
            .write("a/bin/tool", "x")
            .write(
                "a/.claude-plugin/plugin.json",
                &json!({"outputStyles": "./o", "lspServers": {"x": {}}, "monitors": ["m"]}).to_string(),
            );
        let e = run(&fx, &["a"]).await;
        let mut got = skipped(&e);
        got.sort_by(|x, y| x.1.cmp(&y.1));
        let row = |k, n: &str, r| (k, n.to_owned(), r);
        let kind = Reason::KindUnsupported;
        let comp = Reason::ComponentUnsupported;
        assert_eq!(
            got,
            [
                row(Kind::Component, "bin", comp),
                row(Kind::Command, "commands", kind),
                row(Kind::Hook, "hooks", kind),
                row(Kind::Component, "lsp-servers", comp),
                row(Kind::Mcp, "mcp", kind),
                row(Kind::Component, "monitors", comp),
                row(Kind::Component, "output-styles", comp),
                row(Kind::Settings, "settings", kind),
            ]
        );
        assert_eq!(
            e.bundles[0].members,
            ["a/skills/s"],
            "skipped parts stay out of the bundle"
        );
    }

    #[tokio::test]
    async fn declared_hooks_and_mcp_in_the_entry_are_skipped_too() {
        let fx = mk(&json!([{"name": "a", "source": "./a", "hooks": {"PreToolUse": []}, "mcpServers": {"s": {}}}]));
        fx.write("a/skills/s/SKILL.md", "x");
        let e = run(&fx, &["a"]).await;
        let got: Vec<_> = skipped(&e).into_iter().map(|s| s.0).collect();
        assert_eq!(got, [Kind::Hook, Kind::Mcp]);
    }

    #[tokio::test]
    async fn unsupported_sources_skip_without_a_bundle() {
        let fx = mk(&json!([
            {"name": "n", "source": {"source": "npm", "package": "x"}},
            {"name": "g", "source": {"source": "url", "url": "git@github.com:o/r.git"}}
        ]));
        let e = run(&fx, &["*"]).await;
        assert_eq!(
            skipped(&e),
            [
                (Kind::Component, "n".into(), Reason::SourceUnsupported),
                (Kind::Component, "g".into(), Reason::SourceUnsupported)
            ]
        );
        assert!(e.bundles.is_empty() && e.units.is_empty());
    }

    #[tokio::test]
    async fn escaping_paths_and_bad_names_are_refused() {
        let fx = mk(&json!([
            {"name": "esc", "source": "../x"},
            {"name": "comp", "source": "./comp", "skills": ["../up"]},
            {"name": "Bad_Name", "source": "./b"},
            {"name": "dup", "source": "./d"},
            {"name": "dup", "source": "./d"}
        ]));
        fx.write("d/skills/s/SKILL.md", "x");
        let e = run(&fx, &["*"]).await;
        assert_eq!(
            skipped(&e),
            [
                (Kind::Bundle, "esc".into(), Reason::UnsafePath),
                (Kind::Bundle, "comp".into(), Reason::UnsafePath),
                (Kind::Bundle, "Bad_Name".into(), Reason::InvalidName),
                (Kind::Bundle, "dup".into(), Reason::NameCollision)
            ]
        );
        assert_eq!(e.bundles.len(), 1);
    }

    #[tokio::test]
    async fn git_subdir_plugin_is_fetched_and_inherits_the_entry() {
        let ext = Fx::new();
        ext.write("pkg/skills/rem/SKILL.md", "x").write("pkg/agents/ag.md", "x");
        let ext_commit = ext.save();
        let fx = mk(&json!([{
            "name": "far",
            "source": {"source": "git-subdir", "url": ext.url(), "path": "pkg", "sha": ext_commit.as_str()}
        }]));
        let e = run(&fx, &["far"]).await;
        let paths: Vec<_> = e.units.iter().map(|u| u.path.as_str()).collect();
        assert_eq!(paths, ["pkg/skills/rem", "pkg/agents/ag.md"]);
        for u in &e.units {
            assert_eq!(u.content_commit, ext_commit);
            assert_eq!(u.external.as_ref().map(|x| x.url.clone()), Some(ext.url()));
        }
        assert_eq!(e.bundles[0].members.len(), 2);
    }

    #[test]
    fn file_urls_in_marketplace_content_need_a_file_upstream() {
        let src = json!({"source": "url", "url": "file:///tmp/r"});
        assert_eq!(
            resolve_source(Some(&src), "", None, true),
            Source::Remote {
                url: "file:///tmp/r".to_owned(),
                root: String::new(),
                pin: Pin::default()
            }
        );
        assert_eq!(resolve_source(Some(&src), "", None, false), Source::Unsupported);
        let sub = json!({"source": "git-subdir", "url": "file:///tmp/r", "path": "p"});
        assert_eq!(resolve_source(Some(&sub), "", None, false), Source::Unsupported);
    }

    #[tokio::test]
    async fn an_https_marketplace_naming_a_file_url_is_source_unsupported() {
        let fx = mk(&json!([{"name": "x", "source": {"source": "url", "url": "file:///tmp/r"}}]));
        let e = run_as_https(&fx, &["x"]).await;
        assert_eq!(skipped(&e), [(Kind::Component, "x".into(), Reason::SourceUnsupported)]);
    }

    #[tokio::test]
    async fn a_dead_external_repository_is_one_refused_row_and_the_neighbour_lists() {
        let gone = Fx::new().url() + "-missing";
        let fx = mk(&json!([
            {"name": "gone", "source": {"source": "git-subdir", "url": gone, "path": "p"}},
            {"name": "fine", "source": "./fine"}
        ]));
        fx.write("fine/skills/f/SKILL.md", "x");
        let e = run(&fx, &["*"]).await;
        assert_eq!(
            skipped(&e),
            [(Kind::Bundle, "gone".into(), Reason::UpstreamUnavailable)]
        );
        assert_eq!(unit_paths(&e), [(Kind::Skill, "f".into(), "fine/skills/f".into())]);
        assert_eq!(e.bundles.len(), 1);
        assert_eq!(e.bundles[0].name, "fine");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_unreadable_external_repository_is_one_failed_plugin_and_the_neighbour_lists() {
        use std::os::unix::fs::PermissionsExt;
        let broken = Fx::new();
        broken.write("p/skills/b/SKILL.md", "x");
        broken.save();
        for obj in walk(&broken.dir.path().join(".git/objects")) {
            std::fs::set_permissions(&obj, std::fs::Permissions::from_mode(0o644)).unwrap();
            std::fs::write(obj, b"").unwrap();
        }
        let fx = mk(&json!([
            {"name": "broken", "source": {"source": "git-subdir", "url": broken.url(), "path": "p"}},
            {"name": "fine", "source": "./fine"}
        ]));
        fx.write("fine/skills/f/SKILL.md", "x");
        let e = run(&fx, &["*"]).await;
        assert!(skipped(&e).is_empty(), "{:?}", skipped(&e));
        assert_eq!(e.failed.len(), 1);
        assert_eq!(e.failed[0].plugin, "broken");
        assert!(!repository_absent(&e.failed[0].error), "{}", e.failed[0].error);
        assert_eq!(e.bundles.len(), 1);
        assert_eq!(e.bundles[0].name, "fine");
    }

    #[cfg(unix)]
    fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
        out
    }

    #[test]
    fn only_a_remote_that_answered_absent_reads_as_gone() {
        let failed = |stderr: &str| GitError::Failed {
            args: "clone".into(),
            code: Some(128),
            stderr: stderr.into(),
        };
        for gone in [
            failed("remote: Repository not found.\nfatal: repository 'https://github.com/o/r/' not found"),
            failed("fatal: repository 'https://gitlab.example/o/r.git/' not found"),
            failed("fatal: '/srv/r' does not appear to be a git repository"),
            GitError::CommitUnreachable("abc".into()),
            GitError::RefUnknown("v1".into()),
        ] {
            assert!(repository_absent(&gone), "{gone}");
        }
        for transient in [
            failed("fatal: unable to access 'https://github.com/o/r/': Could not resolve host: github.com"),
            failed("fatal: unable to access 'https://h/r/': The requested URL returned error: 503"),
            failed("error: RPC failed; curl 28 Operation too slow"),
            failed("fatal: could not read Username for 'https://github.com': terminal prompts disabled"),
            failed("fatal: git upload-pack: not our ref abc"),
            failed("git: 'remote-https' is not a git command; git-remote-https: not found"),
            GitError::Timeout("git clone: deadline passed".into()),
            GitError::Spawn("git: not found".into()),
        ] {
            assert!(!repository_absent(&transient), "{transient}");
        }
    }

    #[tokio::test]
    async fn a_declared_gitlink_agent_is_skipped_alone() {
        let fx = mk(&json!([{"name": "a", "source": "./a"}]));
        fx.write(
            "a/.claude-plugin/plugin.json",
            &json!({"agents": ["./g.md", "./ok.md"]}).to_string(),
        )
        .write("a/ok.md", "x");
        fx.git(&["add", "-A"]);
        let sub = "a".repeat(40);
        fx.git(&["update-index", "--add", "--cacheinfo", &format!("160000,{sub},a/g.md")]);
        fx.git(&["-c", "core.hooksPath=/dev/null", "commit", "-q", "-m", "x"]);
        let c = Commit::new(&fx.git(&["rev-parse", "HEAD"])).unwrap();
        let e = run_at(&fx, &["a"], &fx.url(), &c).await;
        assert_eq!(unit_paths(&e), [(Kind::Agent, "ok".into(), "a/ok.md".into())]);
        assert_eq!(skipped(&e), [(Kind::Agent, "g".into(), Reason::UnsupportedEntry)]);
        assert_eq!(e.bundles.len(), 1);
    }

    #[tokio::test]
    async fn an_oversize_manifest_is_one_refused_plugin_row() {
        let fx = mk(&json!([
            {"name": "big", "source": "./big"},
            {"name": "fine", "source": "./fine"}
        ]));
        fx.write("big/.claude-plugin/plugin.json", &" ".repeat(MAX_MANIFEST + 1))
            .write("fine/skills/f/SKILL.md", "x");
        let e = run(&fx, &["*"]).await;
        assert_eq!(skipped(&e), [(Kind::Bundle, "big".into(), Reason::UnsupportedEntry)]);
        assert_eq!(e.bundles.len(), 1);
        assert_eq!(e.bundles[0].name, "fine");
    }

    #[tokio::test]
    async fn marketplace_problems_are_invalid_errors() {
        let fx = Fx::new();
        fx.write("x", "x");
        let c = fx.save();
        let mut up = upstream();
        up.plugins = strs(&["*"]);
        let w = tempfile::tempdir().unwrap();
        up.marketplace = Some("missing.json".into());
        assert!(matches!(
            plugins(&fx.repo(), &up, &c, w.path()).await,
            Err(EnumError::Invalid(_))
        ));
        fx.write("bad.json", "not json");
        let c = fx.save();
        up.marketplace = Some("bad.json".into());
        assert!(matches!(
            plugins(&fx.repo(), &up, &c, w.path()).await,
            Err(EnumError::Invalid(_))
        ));
        up.marketplace = Some("../escape.json".into());
        assert!(matches!(
            plugins(&fx.repo(), &up, &c, w.path()).await,
            Err(EnumError::Invalid(_))
        ));
    }
}
