// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Env-reference translation for vendor MCP config entries.
//!
//! The canonical descriptor syntax is `${VAR}` (Claude Code's native
//! form). Each vendor's [`super::vendor::Vendor::mcp_entry`] renders its
//! entry value and hands this walker a closure producing the vendor's
//! own reference syntax (`{env:VAR}` for OpenCode, `${env:VAR}` for the
//! VS Code / Copilot workspace config; Claude is the identity). Only
//! string leaves are rewritten; the ecosystem-specific entry *shape*
//! stays in the vendor impls.

/// Rewrite every canonical `${VAR}` reference inside `value`'s string
/// leaves via `render` (given the bare variable name). Malformed
/// references are left untouched — descriptor validation already
/// rejected them at publish time; install-side leniency keeps a
/// hand-tampered layer from panicking the walker.
pub fn translate_env_refs(value: &mut serde_json::Value, render: &dyn Fn(&str) -> String) {
    match value {
        serde_json::Value::String(s) => {
            let translated = translate_str(s, render);
            if translated != *s {
                *s = translated;
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                translate_env_refs(item, render);
            }
        }
        serde_json::Value::Object(map) => {
            for (_, item) in map.iter_mut() {
                translate_env_refs(item, render);
            }
        }
        _ => {}
    }
}

/// Rewrite `${VAR}` occurrences in one string.
fn translate_str(input: &str, render: &dyn Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) if is_env_name(&after[..end]) => {
                out.push_str(&render(&after[..end]));
                rest = &after[end + 1..];
            }
            _ => {
                // Not a canonical reference: emit the `${` literally and
                // continue after it.
                out.push_str("${");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Whether `name` matches `[A-Za-z_][A-Za-z0-9_]*` (the descriptor's
/// validated env-name charset).
fn is_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn translates_only_canonical_refs_in_string_leaves() {
        let mut v = json!({
            "url": "${BASE}/mcp",
            "headers": {"Authorization": "Bearer ${TOKEN}"},
            "args": ["--dsn", "${DB_DSN}", "plain", "$NOT_A_REF", "${1BAD}", "${UNCLOSED"],
            "count": 3,
            "flag": true,
        });
        translate_env_refs(&mut v, &|name| format!("{{env:{name}}}"));
        assert_eq!(v["url"], "{env:BASE}/mcp");
        assert_eq!(v["headers"]["Authorization"], "Bearer {env:TOKEN}");
        assert_eq!(v["args"][1], "{env:DB_DSN}");
        assert_eq!(v["args"][2], "plain");
        assert_eq!(v["args"][3], "$NOT_A_REF", "bare $VAR is a literal");
        assert_eq!(v["args"][4], "${1BAD}", "invalid name left untouched");
        assert_eq!(v["args"][5], "${UNCLOSED", "unclosed ref left untouched");
        assert_eq!(v["count"], 3);
    }

    #[test]
    fn vscode_style_translation() {
        let mut v = json!({"env": {"API_KEY": "${API_KEY}"}});
        translate_env_refs(&mut v, &|name| format!("${{env:{name}}}"));
        assert_eq!(v["env"]["API_KEY"], "${env:API_KEY}");
    }

    #[test]
    fn identity_translation_is_a_no_op() {
        let mut v = json!({"env": {"A": "${A}"}});
        translate_env_refs(&mut v, &|name| format!("${{{name}}}"));
        assert_eq!(v["env"]["A"], "${A}");
    }
}
