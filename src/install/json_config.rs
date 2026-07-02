// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Shared helpers for reading vendor-owned JSON/JSONC config files.
//!
//! grim edits several vendor config files it does not own (OpenCode's
//! `opencode.json`, client MCP configs). Every reader here is
//! conservative: content that does not parse as a JSON object — even
//! after the JSONC sanitization pass — is refused rather than clobbered.
//! The helpers were extracted from [`super::opencode_config`] so other
//! managed-config writers share one parse/error surface.

use std::io;
use std::path::{Path, PathBuf};

/// Parse `raw` as a JSON object, falling back to a JSONC sanitization pass
/// (comments, trailing commas). Returns the object and whether the
/// sanitization changed anything (⇒ rewriting loses comments).
///
/// # Errors
///
/// `InvalidData` when the content is not a JSON/JSONC object.
pub fn parse_object(raw: &str, path: &Path) -> io::Result<(serde_json::Map<String, serde_json::Value>, bool)> {
    let refused = || {
        invalid_data(format!(
            "'{}' is not a JSON object grim can edit; refusing to touch it",
            path.display()
        ))
    };
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(raw) {
        return Ok((map, false));
    }
    let sanitized = sanitize_jsonc(raw);
    match serde_json::from_str::<serde_json::Value>(&sanitized) {
        Ok(serde_json::Value::Object(map)) => Ok((map, true)),
        _ => Err(refused()),
    }
}

/// Strip `//` and `/* */` comments plus trailing commas — the JSONC
/// extensions vendor configs accept — while leaving string contents intact.
pub fn sanitize_jsonc(input: &str) -> String {
    // Pass 1: comments.
    let chars: Vec<char> = input.chars().collect();
    let mut no_comments = String::with_capacity(input.len());
    let mut i = 0;
    let mut in_string = false;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            no_comments.push(c);
            if c == '\\' && i + 1 < chars.len() {
                no_comments.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                no_comments.push(c);
                i += 1;
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i = (i + 2).min(chars.len());
            }
            _ => {
                no_comments.push(c);
                i += 1;
            }
        }
    }

    // Pass 2: trailing commas.
    let chars: Vec<char> = no_comments.chars().collect();
    let mut out = String::with_capacity(no_comments.len());
    let mut i = 0;
    let mut in_string = false;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if matches!(chars.get(j), Some('}') | Some(']')) {
                i += 1; // drop the trailing comma
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Attach `path` to a bare read/write error — `std::fs` errors do not
/// embed the path on stable Rust — preserving the source chain (never
/// stringify a structured error).
pub fn with_path(path: &Path, source: io::Error) -> io::Error {
    io::Error::new(
        source.kind(),
        PathIo {
            path: path.to_path_buf(),
            source,
        },
    )
}

/// A path-attributed I/O failure on a vendor config file.
#[derive(Debug, thiserror::Error)]
#[error("{path}")]
struct PathIo {
    path: PathBuf,
    #[source]
    source: io::Error,
}

/// An `InvalidData` I/O error carrying `msg`.
pub fn invalid_data(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_leaves_string_contents_alone() {
        let s = r#"{"a": "url://x", "b": "has // no comment", "c": "star /* kept */"}"#;
        assert_eq!(sanitize_jsonc(s), s);
    }

    #[test]
    fn parse_object_strict_then_jsonc_then_refuses() {
        let p = Path::new("cfg.json");
        let (map, extras) = parse_object(r#"{"a": 1}"#, p).unwrap();
        assert_eq!(map["a"], 1);
        assert!(!extras, "strict JSON parses without sanitization");

        let (map, extras) = parse_object("{\n  // c\n  \"a\": 1,\n}\n", p).unwrap();
        assert_eq!(map["a"], 1);
        assert!(extras, "JSONC parse reports that a rewrite loses comments");

        let err = parse_object("not json {{{", p).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let err = parse_object("[1, 2]", p).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData, "non-object JSON is refused");
    }
}
