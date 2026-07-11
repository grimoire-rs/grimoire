// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! A local filesystem source declared in `grimoire.toml`.
//!
//! A config value is a path source — not an OCI reference — when it starts
//! with `./` or `../` or is absolute (see [`is_path_value`]). The OCI
//! identifier grammar rejects all three forms (directory-traversal and
//! missing-registry guards), so the discriminant is unambiguous. Mirrors
//! the `MemberRef` absolute/relative split used for bundle members.
//!
//! The raw declared string is preserved verbatim (it is what the
//! declaration hash and the lock record), and resolved against an anchor —
//! the directory holding the config file — only when filesystem access is
//! needed.

use std::path::{Component, Path, PathBuf};

/// True when a config/CLI value denotes a local path rather than an OCI
/// reference: it starts with `./` or `../`, or is an absolute path.
///
/// The single discriminator shared by config parsing, `grim add`, and
/// `grim install <path>`.
pub fn is_path_value(value: &str) -> bool {
    value.starts_with("./") || value.starts_with("../") || Path::new(value).is_absolute()
}

/// A validated local path source: the raw declared string, guaranteed to
/// start with `./` or `../` or be absolute, non-empty, and free of
/// backslashes (forward slashes only, so a committed config is portable
/// across platforms).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PathSource(String);

impl PathSource {
    /// Parse a declared path value.
    ///
    /// # Errors
    ///
    /// [`PathSourceError::MissingPrefix`] when the value is neither
    /// `./`/`../`-prefixed nor absolute, [`PathSourceError::Backslash`]
    /// when it contains `\`.
    pub fn parse(value: &str) -> Result<Self, PathSourceError> {
        if value.contains('\\') {
            return Err(PathSourceError::Backslash {
                value: value.to_string(),
            });
        }
        if !is_path_value(value) {
            return Err(PathSourceError::MissingPrefix {
                value: value.to_string(),
            });
        }
        Ok(Self(value.to_string()))
    }

    /// The raw declared string (`./skills/x`, `../shared/r.md`, or an
    /// absolute path).
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True when the declared path is absolute (machine-local; a committed
    /// project config carrying one is not portable).
    pub fn is_absolute(&self) -> bool {
        Path::new(&self.0).is_absolute()
    }

    /// Resolve against `anchor` (the config file's directory). An absolute
    /// source ignores the anchor. No canonicalization — the caller decides
    /// whether the path must exist.
    pub fn resolve(&self, anchor: &Path) -> PathBuf {
        let path = Path::new(&self.0);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            anchor.join(path)
        }
    }
}

impl std::fmt::Display for PathSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl serde::Serialize for PathSource {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for PathSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl schemars::JsonSchema for PathSource {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "PathSource".into()
    }

    /// On the wire a path source is a single string; the prefix invariant
    /// is enforced by the parser (a JSON-Schema pattern cannot express the
    /// platform-dependent absolute form).
    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "A local path source: `./` or `../` relative to the config file's directory, or an absolute path (forward slashes only)"
        })
    }
}

/// Compute a `./`/`../`-prefixed relative path from `base` (a directory)
/// to `target`, component-wise. Both inputs must be absolute; the caller
/// canonicalizes beforehand so symlink-induced divergence is resolved.
///
/// # Errors
///
/// [`PathSourceError::NotRelocatable`] when the two paths share no common
/// prefix (e.g. different filesystem roots/drives) or a non-UTF-8
/// component is encountered.
pub fn relative_to(base: &Path, target: &Path) -> Result<PathSource, PathSourceError> {
    let not_relocatable = || PathSourceError::NotRelocatable {
        base: base.display().to_string(),
        target: target.display().to_string(),
    };

    if !base.is_absolute() || !target.is_absolute() {
        return Err(not_relocatable());
    }

    let base_parts: Vec<Component<'_>> = base.components().collect();
    let target_parts: Vec<Component<'_>> = target.components().collect();

    let common = base_parts
        .iter()
        .zip(target_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();
    // No shared root component (Windows: different drive prefixes).
    if common == 0 {
        return Err(not_relocatable());
    }

    let mut segments: Vec<String> = Vec::new();
    for _ in &base_parts[common..] {
        segments.push("..".to_string());
    }
    for part in &target_parts[common..] {
        match part {
            Component::Normal(s) => segments.push(s.to_str().ok_or_else(not_relocatable)?.to_string()),
            _ => return Err(not_relocatable()),
        }
    }

    let raw = if segments.first().is_some_and(|s| s == "..") {
        segments.join("/")
    } else {
        // Prefix invariant: an inside-base target gets the explicit `./`.
        format!("./{}", segments.join("/"))
    };
    PathSource::parse(&raw)
}

/// A declared path value failed validation.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PathSourceError {
    /// Neither `./`/`../`-prefixed nor absolute — not a path value.
    #[error("path source '{value}' must start with ./ or ../ or be absolute")]
    MissingPrefix { value: String },
    /// Backslashes are rejected so committed configs stay portable.
    #[error("path source '{value}' must use forward slashes")]
    Backslash { value: String },
    /// No relative form exists between the two locations.
    #[error("cannot express '{target}' relative to '{base}'")]
    NotRelocatable { base: String, target: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_path_value_table() {
        assert!(is_path_value("./skills/x"));
        assert!(is_path_value("../shared/r.md"));
        assert!(is_path_value("/abs/skills/x"));
        assert!(!is_path_value("ghcr.io/acme/x:1"));
        assert!(!is_path_value("skills/x"));
        assert!(!is_path_value("x"));
        assert!(!is_path_value(""));
    }

    #[test]
    fn parse_accepts_relative_and_absolute() {
        assert_eq!(PathSource::parse("./skills/x").unwrap().as_str(), "./skills/x");
        assert_eq!(PathSource::parse("../r.md").unwrap().as_str(), "../r.md");
        let abs = PathSource::parse("/abs/x").unwrap();
        assert!(abs.is_absolute());
        assert!(!PathSource::parse("./x").unwrap().is_absolute());
    }

    #[test]
    fn parse_rejects_bare_and_backslash() {
        assert!(matches!(
            PathSource::parse("skills/x"),
            Err(PathSourceError::MissingPrefix { .. })
        ));
        assert!(matches!(
            PathSource::parse(".\\skills\\x"),
            Err(PathSourceError::Backslash { .. })
        ));
        assert!(matches!(
            PathSource::parse(""),
            Err(PathSourceError::MissingPrefix { .. })
        ));
    }

    #[test]
    fn resolve_joins_relative_and_keeps_absolute() {
        let anchor = Path::new("/proj");
        assert_eq!(
            PathSource::parse("./skills/x").unwrap().resolve(anchor),
            PathBuf::from("/proj/./skills/x")
        );
        assert_eq!(
            PathSource::parse("../y").unwrap().resolve(anchor),
            PathBuf::from("/proj/../y")
        );
        assert_eq!(
            PathSource::parse("/abs/x").unwrap().resolve(anchor),
            PathBuf::from("/abs/x")
        );
    }

    #[test]
    fn serde_round_trip_and_reject() {
        let src = PathSource::parse("./skills/x").unwrap();
        let json = serde_json::to_string(&src).unwrap();
        assert_eq!(json, "\"./skills/x\"");
        let back: PathSource = serde_json::from_str(&json).unwrap();
        assert_eq!(src, back);
        assert!(serde_json::from_str::<PathSource>("\"bare/word\"").is_err());
    }

    #[test]
    fn relative_to_inside_base() {
        let got = relative_to(Path::new("/proj"), Path::new("/proj/skills/x")).unwrap();
        assert_eq!(got.as_str(), "./skills/x");
    }

    #[test]
    fn relative_to_sibling_uses_parent_dirs() {
        let got = relative_to(Path::new("/proj/sub"), Path::new("/proj/skills/x")).unwrap();
        assert_eq!(got.as_str(), "../skills/x");
        let got = relative_to(Path::new("/a/b/c"), Path::new("/a/x")).unwrap();
        assert_eq!(got.as_str(), "../../x");
    }

    #[test]
    fn relative_to_rejects_relative_inputs() {
        assert!(relative_to(Path::new("proj"), Path::new("/proj/x")).is_err());
        assert!(relative_to(Path::new("/proj"), Path::new("x")).is_err());
    }
}
