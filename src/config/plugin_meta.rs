// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Plugin metadata shared by `grimoire.toml`'s `[plugin]` table and
//! `marketplace.toml`'s `[plugins.<name>]` tables: the name, version and
//! description grammars, the author, link and keyword fields
//! ([`PluginProfile`]), and the rename rule.
//!
//! Lives in `config` so the project config can validate its `[plugin]`
//! table without depending on `export`; `export::marketplace` re-uses the
//! same rules. None of it is a resolution input: `[plugin]` sits outside
//! the declaration hash, so editing it never makes a lock stale.

use std::collections::BTreeMap;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::oci::annotations::{keywords_from_annotations, validate_repository_url};

/// Most characters a plugin description may carry, counted in UTF-16 code
/// units (the JavaScript `length` harness validators apply).
pub const MAX_DESCRIPTION_LEN: usize = 500;

/// Longest `license` and author name, in characters.
const MAX_TEXT_LEN: usize = 128;
/// Longest link, in characters.
const MAX_URL_LEN: usize = 2048;
/// Longest `category` slug.
const MAX_CATEGORY_LEN: usize = 32;
/// Longest keyword or tag.
const MAX_WORD_LEN: usize = 40;
/// Most keywords, and most tags.
pub const MAX_WORDS: usize = 20;

/// A plugin's author (`author` in `[plugin]` and `[plugins.<name>]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginAuthor {
    /// Display name; not empty, at most 128 characters.
    pub name: String,
    /// Contact address in the form `name@host.tld`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// An `https://` link to the author's page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// The author, link and keyword fields of a plugin, as every plugin and
/// marketplace file draws them. The declaring tables carry the fields
/// flat (`deny_unknown_fields` rules out flattening this struct into them)
/// and hand them over through `profile()`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginProfile {
    pub author: Option<PluginAuthor>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub documentation: Option<String>,
    pub support: Option<String>,
    pub license: Option<String>,
    pub keywords: Vec<String>,
    pub tags: Vec<String>,
    pub category: Option<String>,
}

impl PluginProfile {
    /// `self` with every field it leaves unset taken from `fallback`.
    pub fn or(self, fallback: PluginProfile) -> PluginProfile {
        let list = |own: Vec<String>, other: Vec<String>| if own.is_empty() { other } else { own };
        PluginProfile {
            author: self.author.or(fallback.author),
            homepage: self.homepage.or(fallback.homepage),
            repository: self.repository.or(fallback.repository),
            documentation: self.documentation.or(fallback.documentation),
            support: self.support.or(fallback.support),
            license: self.license.or(fallback.license),
            keywords: list(self.keywords, fallback.keywords),
            tags: list(self.tags, fallback.tags),
            category: self.category.or(fallback.category),
        }
    }

    /// Check every field; the reason names the first one that fails. Values
    /// are held as written: surrounding blanks and duplicates are errors,
    /// not normalised.
    pub fn validate(&self) -> Result<(), String> {
        self.problems()
            .into_iter()
            .next()
            .map_or(Ok(()), |(_, reason)| Err(reason))
    }

    /// The profile a pinned manifest's annotations describe, every invalid
    /// field dropped. The first author listed names the plugin author. No
    /// `support`: those channels sit on the companion manifest, which a
    /// pinned version manifest does not carry.
    pub fn from_annotations(annotations: &BTreeMap<String, String>) -> PluginProfile {
        let get = |key: &str| {
            annotations
                .get(key)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        PluginProfile {
            author: get("org.opencontainers.image.authors")
                .and_then(|v| v.split([',', '<']).next().map(|n| n.trim().to_string()))
                .filter(|name| !name.is_empty())
                .map(|name| PluginAuthor {
                    name,
                    email: None,
                    url: None,
                }),
            homepage: get("org.opencontainers.image.url"),
            repository: get("org.opencontainers.image.source"),
            documentation: get("org.opencontainers.image.documentation"),
            support: None,
            license: get("org.opencontainers.image.licenses"),
            keywords: keywords_from_annotations(annotations),
            tags: Vec::new(),
            category: None,
        }
        .sanitized()
    }

    /// `self` with every invalid field dropped, for values a publisher wrote
    /// and the exporter cannot correct.
    pub fn sanitized(mut self) -> PluginProfile {
        for (key, _) in self.problems() {
            match key {
                "author" => self.author = None,
                "homepage" => self.homepage = None,
                "repository" => self.repository = None,
                "documentation" => self.documentation = None,
                "support" => self.support = None,
                "license" => self.license = None,
                "keywords" => self.keywords.clear(),
                "tags" => self.tags.clear(),
                _ => self.category = None,
            }
        }
        self
    }

    /// `(field, reason)` of every invalid field.
    fn problems(&self) -> Vec<(&'static str, String)> {
        let mut found = Vec::new();
        let mut note = |key: &'static str, result: Result<(), String>| {
            if let Err(reason) = result {
                found.push((key, reason));
            }
        };
        if let Some(author) = &self.author {
            note("author", check_author(author));
        }
        for (key, value) in [
            ("homepage", &self.homepage),
            ("repository", &self.repository),
            ("documentation", &self.documentation),
            ("support", &self.support),
        ] {
            if let Some(url) = value {
                note(key, check_https_url(key, url));
            }
        }
        if let Some(license) = &self.license {
            note("license", check_text("license", license, MAX_TEXT_LEN));
        }
        for (key, words) in [("keywords", &self.keywords), ("tags", &self.tags)] {
            note(key, check_words(key, words));
        }
        if let Some(category) = &self.category {
            note("category", check_category(category));
        }
        found
    }
}

fn check_text(key: &str, value: &str, max: usize) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("`{key}` must not be empty"));
    }
    if value != value.trim() {
        return Err(format!("`{key}` must not start or end with whitespace"));
    }
    if value.chars().count() > max {
        return Err(format!("`{key}` is longer than {max} characters"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("`{key}` must not contain control characters"));
    }
    Ok(())
}

/// An `https://` link without credentials (the artifact `repository` rule);
/// `key` names the field in the reason.
pub fn check_https_url(key: &str, value: &str) -> Result<(), String> {
    check_text(key, value, MAX_URL_LEN)?;
    if value.chars().any(char::is_whitespace) || validate_repository_url(value).is_err() {
        return Err(format!("`{key}` must be an https:// URL without embedded credentials"));
    }
    Ok(())
}

fn check_author(author: &PluginAuthor) -> Result<(), String> {
    check_text("author.name", &author.name, MAX_TEXT_LEN)?;
    if let Some(email) = &author.email
        && !is_email(email)
    {
        return Err(format!(
            "author.email '{email}' is not an email address (expected name@host.tld)"
        ));
    }
    author
        .url
        .as_deref()
        .map_or(Ok(()), |url| check_https_url("author.url", url))
}

/// Keywords and tags: at most [`MAX_WORDS`] distinct short words.
fn check_words(key: &str, words: &[String]) -> Result<(), String> {
    if words.len() > MAX_WORDS {
        return Err(format!(
            "`{key}` holds {} entries; at most {MAX_WORDS} are allowed",
            words.len()
        ));
    }
    for (i, word) in words.iter().enumerate() {
        check_text(key, word, MAX_WORD_LEN)?;
        if word.contains(',') {
            return Err(format!("`{key}` entry '{word}' must not contain a comma"));
        }
        if words[..i].contains(word) {
            return Err(format!("`{key}` lists '{word}' twice"));
        }
    }
    Ok(())
}

/// A short lowercase slug: letters, digits and single hyphens.
fn check_category(value: &str) -> Result<(), String> {
    let slug = !value.is_empty()
        && value.len() <= MAX_CATEGORY_LEN
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if slug {
        Ok(())
    } else {
        Err(format!(
            "`category` '{value}' is invalid: use lowercase letters, digits and single hyphens, at most {MAX_CATEGORY_LEN} characters"
        ))
    }
}

/// `^[^@\s]+@[^@\s]+\.[^@\s]+$`: one `@`, no whitespace, and a dot with
/// something on both sides of it in the host part.
pub fn is_email(text: &str) -> bool {
    let Some((local, host)) = text.split_once('@') else {
        return false;
    };
    let plain = |part: &str| !part.is_empty() && !part.contains('@') && !part.chars().any(char::is_whitespace);
    plain(local)
        && plain(host)
        && host
            .char_indices()
            .any(|(i, c)| c == '.' && i > 0 && i + 1 < host.len())
}

/// Metadata a project declares for `grim export plugin --project`, and
/// that a `marketplace.toml` plugin pointing at the project falls back to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginMeta {
    /// Sets the name of the plugin `grim export plugin --project` builds
    /// from this project. Overridden by the `--name` flag when given.
    /// Lowercase letters, digits and single hyphens, at most 64 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Sets the description written to the exported plugin, at most 500
    /// characters. Overridden by the `--description` flag when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Sets the plugin version base, a semver version without build
    /// metadata; grim appends a content-hash suffix on export. Overridden by
    /// the `--version` flag when given. A leading `v` is dropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Sets the plugin logo, a `.png` or `.svg` file relative to the
    /// `grimoire.toml` directory, shipped as `assets/logo.<ext>` in every
    /// exported plugin. Overridden by the `--logo` flag when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo: Option<PathBuf>,
    /// Member rename rule applied on export.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename: Option<RenameRule>,
    /// Sets the plugin author, written to the exported plugin and its
    /// marketplace entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<PluginAuthor>,
    /// Sets the plugin's home page, an `https://` link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// Sets the plugin's source repository, an `https://` link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// Sets the plugin's documentation, an `https://` link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
    /// Sets where users get help, an `https://` link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support: Option<String>,
    /// Sets the plugin's license, preferably an SPDX identifier such as
    /// `Apache-2.0`; at most 128 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Lists the plugin's keywords, at most 20 distinct entries of at most
    /// 40 characters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    /// Lists the plugin's tags, at most 20 distinct entries of at most 40
    /// characters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Sets the plugin's category, a lowercase slug of at most 32
    /// characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

/// A plugin's member rename rule (`[plugin.rename]`, `[plugins.<name>.rename]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameRule {
    /// Prefix removed from member names that start with it; non-empty.
    pub strip_prefix: String,
}

impl PluginMeta {
    /// The `[plugin]` table of a `grimoire.toml` document exactly as
    /// authored — unvalidated, every other table ignored. `None` when the
    /// document declares none.
    pub fn from_config_toml(s: &str) -> Result<Option<PluginMeta>, toml::de::Error> {
        #[derive(Deserialize)]
        struct PluginOnly {
            #[serde(default)]
            plugin: Option<PluginMeta>,
        }
        toml::from_str::<PluginOnly>(s).map(|doc| doc.plugin)
    }

    /// Validate every field and normalize `version` in place (leading `v`
    /// stripped). Returns the reason on rejection; callers choose the exit
    /// code (78 at load, 65 for a `grim config set` value).
    pub fn validate(&mut self) -> Result<(), String> {
        if let Some(name) = &self.name {
            validate_plugin_name(name)?;
        }
        if let Some(description) = &self.description {
            validate_description(description)?;
        }
        if let Some(raw) = &self.version {
            self.version = Some(checked_version(raw)?);
        }
        if self.logo.as_ref().is_some_and(|l| l.as_os_str().is_empty()) {
            return Err("`logo` must not be empty".to_string());
        }
        if self.rename.as_ref().is_some_and(|r| r.strip_prefix.is_empty()) {
            return Err("`rename.strip_prefix` must not be empty".to_string());
        }
        self.profile().validate()
    }

    /// The author, link and keyword fields.
    pub fn profile(&self) -> PluginProfile {
        PluginProfile {
            author: self.author.clone(),
            homepage: self.homepage.clone(),
            repository: self.repository.clone(),
            documentation: self.documentation.clone(),
            support: self.support.clone(),
            license: self.license.clone(),
            keywords: self.keywords.clone(),
            tags: self.tags.clone(),
            category: self.category.clone(),
        }
    }
}

/// The plugin name rule (C-002): a plugin name is valid iff
/// [`crate::skill::SkillName::parse`] accepts it.
pub fn validate_plugin_name(name: &str) -> Result<(), String> {
    crate::skill::SkillName::parse(name)
        .map(drop)
        .map_err(|reason| format!("invalid plugin name '{name}': {reason}"))
}

/// Length of `s` as [`MAX_DESCRIPTION_LEN`] counts it.
pub fn description_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// An author-written description must fit [`MAX_DESCRIPTION_LEN`] once
/// trimmed.
pub fn validate_description(description: &str) -> Result<(), String> {
    let len = description_len(description.trim());
    if len > MAX_DESCRIPTION_LEN {
        return Err(format!(
            "description is {len} characters; plugin descriptions allow at most {MAX_DESCRIPTION_LEN}"
        ));
    }
    Ok(())
}

/// The single home of the plugin version grammar (C-023): strip one
/// leading `v`, parse as semver, reject non-empty build metadata
/// (pre-release is allowed). `None` when invalid.
pub fn normalize_version(raw: &str) -> Option<String> {
    let bare = raw.strip_prefix('v').unwrap_or(raw);
    semver::Version::parse(bare)
        .ok()
        .filter(|v| v.build.is_empty())
        .map(|v| v.to_string())
}

/// [`normalize_version`] with the rejection message.
pub fn checked_version(raw: &str) -> Result<String, String> {
    normalize_version(raw).ok_or_else(|| {
        format!("invalid version '{raw}': expected a semver version without build metadata (e.g. 1.2.0)")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> PluginMeta {
        PluginMeta {
            name: Some("team".into()),
            description: Some("Team tools".into()),
            version: Some("v1.2.0".into()),
            logo: Some("assets/team.svg".into()),
            rename: Some(RenameRule {
                strip_prefix: "acme-".into(),
            }),
            ..PluginMeta::default()
        }
    }

    fn full_profile() -> PluginProfile {
        PluginProfile {
            author: Some(PluginAuthor {
                name: "Acme".into(),
                email: Some("dev@acme.example".into()),
                url: Some("https://acme.example".into()),
            }),
            homepage: Some("https://acme.example/team".into()),
            repository: Some("https://github.com/acme/team".into()),
            documentation: Some("https://acme.example/docs".into()),
            support: Some("https://github.com/acme/team/issues".into()),
            license: Some("Apache-2.0".into()),
            keywords: vec!["review".into(), "quality".into()],
            tags: vec!["team".into()],
            category: Some("developer-tools".into()),
        }
    }

    #[test]
    fn valid_meta_normalizes_the_version() {
        let mut m = meta();
        m.validate().unwrap();
        assert_eq!(m.version.as_deref(), Some("1.2.0"));
    }

    #[test]
    fn each_bad_field_is_rejected_with_its_reason() {
        type Breaker = fn(&mut PluginMeta);
        let cases: [(Breaker, &str); 5] = [
            (|m| m.name = Some("Team".into()), "invalid plugin name 'Team'"),
            (|m| m.description = Some("x".repeat(501)), "at most 500"),
            (|m| m.version = Some("1.0.0+b".into()), "invalid version '1.0.0+b'"),
            (|m| m.logo = Some(PathBuf::new()), "`logo` must not be empty"),
            (
                |m| {
                    m.rename = Some(RenameRule {
                        strip_prefix: String::new(),
                    })
                },
                "`rename.strip_prefix` must not be empty",
            ),
        ];
        for (break_it, reason) in cases {
            let mut m = meta();
            break_it(&mut m);
            let err = m.validate().unwrap_err();
            assert!(err.contains(reason), "{err}");
        }
    }

    #[test]
    fn description_cap_counts_utf16_after_trimming() {
        assert!(validate_description(&format!("  {}  ", "x".repeat(500))).is_ok());
        assert!(validate_description(&"🦀".repeat(251)).is_err(), "502 UTF-16 units");
    }

    #[test]
    fn version_grammar() {
        assert_eq!(normalize_version("v2.0.0").as_deref(), Some("2.0.0"));
        assert_eq!(normalize_version("1.0.0-rc.1").as_deref(), Some("1.0.0-rc.1"));
        for bad in ["latest", "1.0", "1.0.0+x"] {
            assert_eq!(normalize_version(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_full_profile_is_valid_and_an_empty_one_too() {
        assert_eq!(full_profile().validate(), Ok(()));
        assert_eq!(PluginProfile::default().validate(), Ok(()));
    }

    #[test]
    fn each_bad_profile_field_is_rejected_naming_the_field() {
        type Breaker = fn(&mut PluginProfile);
        let long = "x".repeat(2049);
        let cases: Vec<(Breaker, &str)> = vec![
            (|p| p.author.as_mut().unwrap().name = " ".into(), "author.name"),
            (
                |p| p.author.as_mut().unwrap().email = Some("nope".into()),
                "author.email",
            ),
            (
                |p| p.author.as_mut().unwrap().url = Some("http://acme.example".into()),
                "author.url",
            ),
            (|p| p.homepage = Some("ftp://acme.example".into()), "homepage"),
            (
                |p| p.repository = Some("https://user:pw@github.com/a/b".into()),
                "repository",
            ),
            (|p| p.documentation = Some("https://a b".into()), "documentation"),
            (|p| p.support = Some(String::new()), "support"),
            (|p| p.license = Some("x".repeat(129)), "license"),
            (|p| p.license = Some(" MIT".into()), "license"),
            (|p| p.keywords = vec!["a".into(), "a".into()], "keywords"),
            (|p| p.keywords = vec!["a,b".into()], "comma"),
            (|p| p.tags = (0..21).map(|i| format!("t{i}")).collect(), "tags"),
            (|p| p.tags = vec!["x".repeat(41)], "tags"),
            (|p| p.category = Some("Dev Tools".into()), "category"),
            (|p| p.category = Some("a--b".into()), "category"),
        ];
        for (break_it, needle) in cases {
            let mut p = full_profile();
            break_it(&mut p);
            let err = p.validate().unwrap_err();
            assert!(err.contains(needle), "{needle}: {err}");
        }
        let mut p = full_profile();
        p.homepage = Some(long);
        assert!(p.validate().unwrap_err().contains("homepage"));
    }

    #[test]
    fn a_url_error_never_echoes_the_value() {
        let mut p = full_profile();
        p.repository = Some("https://token@github.com/a/b".into());
        assert!(!p.validate().unwrap_err().contains("token"));
    }

    #[test]
    fn sanitized_drops_only_the_invalid_fields() {
        let mut p = full_profile();
        p.homepage = Some("http://insecure.example".into());
        p.keywords = vec!["ok".into(), "ok".into()];
        let clean = p.sanitized();
        assert_eq!(clean.homepage, None);
        assert!(clean.keywords.is_empty());
        assert_eq!(clean.license.as_deref(), Some("Apache-2.0"));
        assert_eq!(clean.author, full_profile().author);
        assert_eq!(full_profile().sanitized(), full_profile());
    }

    #[test]
    fn or_fills_only_what_is_unset() {
        let own = PluginProfile {
            license: Some("MIT".into()),
            keywords: vec!["mine".into()],
            ..PluginProfile::default()
        };
        let merged = own.or(full_profile());
        assert_eq!(merged.license.as_deref(), Some("MIT"));
        assert_eq!(merged.keywords, ["mine"]);
        assert_eq!(merged.tags, ["team"]);
        assert_eq!(merged.homepage.as_deref(), Some("https://acme.example/team"));
    }

    #[test]
    fn plugin_meta_validates_its_profile_fields() {
        let mut m = meta();
        m.homepage = Some("http://insecure.example".into());
        assert!(m.validate().unwrap_err().contains("homepage"));
        let mut ok = meta();
        ok.keywords = vec!["review".into()];
        ok.author = full_profile().author;
        ok.validate().unwrap();
        assert_eq!(ok.profile().keywords, ["review"]);
    }

    #[test]
    fn plugin_table_with_every_new_key_parses_and_an_unknown_key_is_refused() {
        let doc =
            "[plugin]\nhomepage = \"https://acme.example\"\nkeywords = [\"a\"]\n[plugin.author]\nname = \"Acme\"\n";
        let meta = PluginMeta::from_config_toml(doc).unwrap().unwrap();
        assert_eq!(meta.author.unwrap().name, "Acme");
        assert!(PluginMeta::from_config_toml("[plugin.author]\nname = \"A\"\nbogus = 1\n").is_err());
    }
}
