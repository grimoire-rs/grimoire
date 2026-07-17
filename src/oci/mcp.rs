// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The MCP server descriptor artifact format.
//!
//! An `mcp` artifact is a vendor-agnostic description of one Model
//! Context Protocol server: transport plus launch/connection data. It is
//! authored as `mcp/<name>.toml`, published as a standard single-layer
//! OCI artifact whose layer blob is the canonical JSON serialization
//! (mirroring [`super::bundle`]), and installed by registering an entry
//! in each client's native MCP config file — an MCP descriptor never
//! materializes as a file of its own.
//!
//! Environment references inside string values use the canonical
//! `${VAR}` form (Claude Code's native syntax); each vendor writer
//! translates it at render time. `${VAR:-default}` is rejected in v1 —
//! only Claude supports defaults natively, and a silently dropped
//! default would behave differently per client.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::oci::annotations::validate_repository_url;

/// OCI layer media type for the MCP descriptor document.
pub const MCP_LAYER_MEDIA_TYPE: &str = "application/vnd.grimoire.mcp.v1+json";

/// Upper bound on the descriptor layer blob. A descriptor is a small
/// config document; the cap bounds memory against a hostile or corrupt
/// registry (CWE-770), mirroring the bundle layer cap.
pub const MCP_LAYER_SIZE_LIMIT: u64 = 64 * 1024;

/// An MCP server transport.
///
/// Closed internal enum — matches stay total, no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    /// A local process speaking JSON-RPC over stdio (`command` + `args`).
    Stdio,
    /// A remote server over streamable HTTP (`url` + `headers`).
    Http,
    /// A remote server over server-sent events (deprecated upstream but
    /// still accepted by every client).
    Sse,
    /// A remote server over a persistent WebSocket (`url` + `headers`,
    /// `ws://`/`wss://` scheme). Claude-native (`type: "ws"`); other
    /// vendors decline it.
    Ws,
}

impl std::fmt::Display for McpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
            Self::Sse => "sse",
            Self::Ws => "ws",
        })
    }
}

/// The `[server]` table: how clients launch or reach the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServer {
    /// Transport kind; drives which of the other fields are required.
    pub transport: McpTransport,
    /// Executable to launch (stdio only, required there).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Arguments passed to `command`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Environment for the launched process. Values may reference host
    /// environment variables as `${VAR}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Server URL (http/sse only, required there).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// HTTP headers sent to a remote server. Values may reference host
    /// environment variables as `${VAR}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// Server startup timeout in milliseconds (Claude `timeout`,
    /// OpenCode `timeout`). Valid for every transport.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    /// Load the server eagerly at client startup (Claude `alwaysLoad`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub always_load: Option<bool>,
    /// Executable that produces fresh auth headers (Claude
    /// `headersHelper`). Remote transports only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers_helper: Option<String>,
    /// Working directory for the launched process (OpenCode `cwd`).
    /// Stdio only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// The MCP descriptor document: authored as TOML, shipped as the
/// canonical JSON layer blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpDescriptor {
    /// Human-readable description (required — becomes the OCI
    /// `description` annotation).
    pub description: String,
    /// Optional short catalog summary (`com.grimoire.summary`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Optional comma-separated keywords (`com.grimoire.keywords`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<String>,
    /// Optional SPDX license expression
    /// (`org.opencontainers.image.licenses`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Optional HTTPS source-repository URL
    /// (`org.opencontainers.image.source`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// Optional deprecation notice (`com.grimoire.deprecated`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<String>,
    /// The server definition.
    pub server: McpServer,
}

/// A descriptor rejected at parse/validation time.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum McpError {
    /// The TOML source did not parse or carried unknown fields.
    #[error("invalid MCP descriptor: {0}")]
    Toml(#[source] Box<toml::de::Error>),
    /// The JSON layer blob did not parse.
    #[error("invalid MCP layer: {0}")]
    Json(#[source] serde_json::Error),
    /// `description` is empty.
    #[error("MCP descriptor requires a non-empty 'description'")]
    MissingDescription,
    /// A stdio server without a `command`, or a remote field on stdio.
    #[error("stdio transport requires 'command' and forbids 'url'/'headers'")]
    StdioShape,
    /// A remote server without a `url`, or a stdio field on http/sse.
    #[error("{transport} transport requires 'url' and forbids 'command'/'args'/'env'")]
    RemoteShape {
        /// The declared transport.
        transport: McpTransport,
    },
    /// A `url` whose scheme does not fit the transport: http(s) for
    /// `http`/`sse`, ws(s) for `ws`.
    #[error("invalid server url '{0}': expected http:// or https:// (ws:// or wss:// for the ws transport)")]
    InvalidUrl(String),
    /// A refinement field authored on a transport that cannot use it.
    #[error("field '{field}' is not valid for {transport} transport")]
    RefinementShape {
        /// The offending field (descriptor spelling).
        field: &'static str,
        /// The declared transport.
        transport: McpTransport,
    },
    /// An `env` key outside `[A-Za-z_][A-Za-z0-9_]*`.
    #[error("invalid env key '{0}': expected [A-Za-z_][A-Za-z0-9_]*")]
    InvalidEnvKey(String),
    /// A malformed `${…}` reference (including `${VAR:-default}`).
    #[error(
        "invalid environment reference '{0}': expected ${{VAR}} with VAR matching [A-Za-z_][A-Za-z0-9_]* (defaults like ${{VAR:-x}} are not supported)"
    )]
    InvalidEnvRef(String),
    /// An invalid authored repository URL.
    #[error(transparent)]
    Repository(#[from] crate::oci::annotations::RepositoryUrlError),
    /// The layer blob exceeds [`MCP_LAYER_SIZE_LIMIT`].
    #[error("MCP layer blob of {actual} bytes exceeds the {limit}-byte limit")]
    TooLarge {
        /// Received size.
        actual: u64,
        /// The enforced cap.
        limit: u64,
    },
}

impl McpDescriptor {
    /// Parse and validate an authored TOML descriptor.
    ///
    /// # Errors
    ///
    /// [`McpError`] on TOML/shape/reference violations — see the variant
    /// docs.
    pub fn from_toml_str(raw: &str) -> Result<Self, McpError> {
        let descriptor: Self = toml::from_str(raw).map_err(|e| McpError::Toml(Box::new(e)))?;
        descriptor.validate()?;
        Ok(descriptor)
    }

    /// Serialize to the canonical pretty-JSON layer bytes.
    ///
    /// # Errors
    ///
    /// [`McpError::Json`] on a serializer failure (unreachable for this
    /// shape, but surfaced rather than panicking).
    pub fn to_layer_bytes(&self) -> Result<Vec<u8>, McpError> {
        let mut bytes = serde_json::to_vec_pretty(self).map_err(McpError::Json)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// Parse a descriptor layer blob (registry data: size-capped and
    /// re-validated — the wire is untrusted).
    ///
    /// # Errors
    ///
    /// [`McpError`] when the blob exceeds the cap, does not parse, or
    /// fails validation.
    pub fn from_layer_bytes(bytes: &[u8]) -> Result<Self, McpError> {
        if bytes.len() as u64 > MCP_LAYER_SIZE_LIMIT {
            return Err(McpError::TooLarge {
                actual: bytes.len() as u64,
                limit: MCP_LAYER_SIZE_LIMIT,
            });
        }
        let descriptor: Self = serde_json::from_slice(bytes).map_err(McpError::Json)?;
        descriptor.validate()?;
        Ok(descriptor)
    }

    /// Validate descriptor invariants (transport shape, env keys and
    /// `${VAR}` references, repository URL).
    fn validate(&self) -> Result<(), McpError> {
        if self.description.trim().is_empty() {
            return Err(McpError::MissingDescription);
        }
        if let Some(repo) = &self.repository {
            validate_repository_url(repo)?;
        }
        let s = &self.server;
        match s.transport {
            McpTransport::Stdio => {
                if s.command.as_deref().is_none_or(|c| c.trim().is_empty()) || s.url.is_some() || !s.headers.is_empty()
                {
                    return Err(McpError::StdioShape);
                }
                if s.headers_helper.is_some() {
                    return Err(McpError::RefinementShape {
                        field: "headers_helper",
                        transport: s.transport,
                    });
                }
            }
            McpTransport::Http | McpTransport::Sse | McpTransport::Ws => {
                let Some(url) = s.url.as_deref() else {
                    return Err(McpError::RemoteShape { transport: s.transport });
                };
                if s.command.is_some() || !s.args.is_empty() || !s.env.is_empty() {
                    return Err(McpError::RemoteShape { transport: s.transport });
                }
                // Scheme is transport-fitted: wss:// is canonical for ws
                // (code.claude.com/docs/en/mcp, "type": "ws" example).
                let scheme_ok = match s.transport {
                    McpTransport::Ws => url.starts_with("wss://") || url.starts_with("ws://"),
                    _ => url.starts_with("https://") || url.starts_with("http://"),
                };
                if !scheme_ok {
                    return Err(McpError::InvalidUrl(url.to_string()));
                }
                if s.cwd.is_some() {
                    return Err(McpError::RefinementShape {
                        field: "cwd",
                        transport: s.transport,
                    });
                }
            }
        }
        for key in s.env.keys() {
            if !is_env_name(key) {
                return Err(McpError::InvalidEnvKey(key.clone()));
            }
        }
        for value in self.string_values() {
            validate_env_refs(value)?;
        }
        Ok(())
    }

    /// Whether any string value carries a canonical `${VAR}` reference.
    /// Copilot CLI's global config supports no variable substitution, so
    /// its writer skips descriptors that need one (never inlines secrets).
    pub fn has_env_refs(&self) -> bool {
        self.string_values().any(|v| env_ref_names(v).next().is_some())
    }

    /// Every string value that may carry `${VAR}` references.
    fn string_values(&self) -> impl Iterator<Item = &str> {
        let s = &self.server;
        s.command
            .as_deref()
            .into_iter()
            .chain(s.args.iter().map(String::as_str))
            .chain(s.env.values().map(String::as_str))
            .chain(s.url.as_deref())
            .chain(s.headers.values().map(String::as_str))
    }
}

/// Whether `name` matches `[A-Za-z_][A-Za-z0-9_]*`.
fn is_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Validate every `${…}` occurrence in `value` as a canonical `${VAR}`
/// reference.
fn validate_env_refs(value: &str) -> Result<(), McpError> {
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(McpError::InvalidEnvRef(rest[start..].to_string()));
        };
        let name = &after[..end];
        if !is_env_name(name) {
            return Err(McpError::InvalidEnvRef(format!("${{{name}}}")));
        }
        rest = &after[end + 1..];
    }
    Ok(())
}

/// Iterate all `${VAR}` reference names in `value` (assumes prior
/// validation; malformed references are skipped).
pub fn env_ref_names(value: &str) -> impl Iterator<Item = &str> {
    let mut rest = value;
    std::iter::from_fn(move || {
        loop {
            let start = rest.find("${")?;
            let after = &rest[start + 2..];
            let Some(end) = after.find('}') else {
                rest = "";
                return None;
            };
            let name = &after[..end];
            rest = &after[end + 1..];
            if is_env_name(name) {
                return Some(name);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const STDIO: &str = r#"
description = "Grimoire catalog over MCP."

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
env = { GRIM_HOME = "${GRIM_HOME}" }
"#;

    const HTTP: &str = r#"
description = "Remote server."
summary = "remote"
keywords = "a,b"
repository = "https://github.com/acme/x"

[server]
transport = "http"
url = "https://api.example.com/mcp"
headers = { Authorization = "Bearer ${TOKEN}" }
"#;

    #[test]
    fn parses_and_round_trips_layer_bytes() {
        let d = McpDescriptor::from_toml_str(STDIO).unwrap();
        assert_eq!(d.server.transport, McpTransport::Stdio);
        assert_eq!(d.server.command.as_deref(), Some("grim"));
        let bytes = d.to_layer_bytes().unwrap();
        let parsed = McpDescriptor::from_layer_bytes(&bytes).unwrap();
        assert_eq!(d, parsed);
        // Byte stability: same descriptor ⇒ same layer bytes.
        assert_eq!(bytes, parsed.to_layer_bytes().unwrap());
    }

    #[test]
    fn parses_remote_with_catalog_metadata() {
        let d = McpDescriptor::from_toml_str(HTTP).unwrap();
        assert_eq!(d.server.transport, McpTransport::Http);
        assert_eq!(d.summary.as_deref(), Some("remote"));
        assert_eq!(d.repository.as_deref(), Some("https://github.com/acme/x"));
    }

    #[test]
    fn validation_matrix_rejects_bad_shapes() {
        // Empty description.
        let err = McpDescriptor::from_toml_str("description = \" \"\n[server]\ntransport = \"stdio\"\ncommand = \"x\"")
            .unwrap_err();
        assert!(matches!(err, McpError::MissingDescription));
        // stdio without command.
        let err = McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"stdio\"").unwrap_err();
        assert!(matches!(err, McpError::StdioShape));
        // stdio with url.
        let err = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"x\"\nurl = \"https://x\"",
        )
        .unwrap_err();
        assert!(matches!(err, McpError::StdioShape));
        // http without url.
        let err = McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"http\"").unwrap_err();
        assert!(matches!(err, McpError::RemoteShape { .. }));
        // http with command.
        let err = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"\ncommand = \"x\"",
        )
        .unwrap_err();
        assert!(matches!(err, McpError::RemoteShape { .. }));
        // Non-http url scheme.
        let err =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"ftp://x\"")
                .unwrap_err();
        assert!(matches!(err, McpError::InvalidUrl(_)));
        // Unknown field.
        let err = McpDescriptor::from_toml_str(
            "description = \"d\"\nsurprise = 1\n[server]\ntransport = \"stdio\"\ncommand = \"x\"",
        )
        .unwrap_err();
        assert!(matches!(err, McpError::Toml(_)));
        // Bad env key.
        let err = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"x\"\nenv = { \"1BAD\" = \"v\" }",
        )
        .unwrap_err();
        assert!(matches!(err, McpError::InvalidEnvKey(_)));
        // Non-https repository.
        let err = McpDescriptor::from_toml_str(
            "description = \"d\"\nrepository = \"http://x\"\n[server]\ntransport = \"stdio\"\ncommand = \"x\"",
        )
        .unwrap_err();
        assert!(matches!(err, McpError::Repository(_)));
    }

    #[test]
    fn env_ref_defaults_and_malformed_refs_are_rejected() {
        for bad in ["${VAR:-fallback}", "${1BAD}", "${UNCLOSED", "${}"] {
            let toml = format!(
                "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"x\"\nenv = {{ A = \"{bad}\" }}",
            );
            let err = McpDescriptor::from_toml_str(&toml).unwrap_err();
            assert!(matches!(err, McpError::InvalidEnvRef(_)), "input: {bad}");
        }
        // A plain `$VAR` (no braces) is a literal, not a reference — allowed.
        let toml = "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"x\"\nenv = { A = \"$HOME\" }";
        McpDescriptor::from_toml_str(toml).unwrap();
    }

    #[test]
    fn ws_transport_parses_and_validates() {
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"ws\"\nurl = \"wss://mcp.example.com/socket\"\nheaders = { Authorization = \"Bearer ${TOKEN}\" }",
        )
        .unwrap();
        assert_eq!(d.server.transport, McpTransport::Ws);
        let bytes = d.to_layer_bytes().unwrap();
        assert_eq!(d, McpDescriptor::from_layer_bytes(&bytes).unwrap());

        // Remote shape enforced (url required, stdio fields forbidden).
        let err = McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"ws\"").unwrap_err();
        assert!(matches!(err, McpError::RemoteShape { .. }));

        // Scheme is transport-fitted both ways.
        let err =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"ws\"\nurl = \"https://x\"")
                .unwrap_err();
        assert!(matches!(err, McpError::InvalidUrl(_)));
        let err =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"wss://x\"")
                .unwrap_err();
        assert!(matches!(err, McpError::InvalidUrl(_)));
    }

    #[test]
    fn descriptor_round_trips_refinement_fields() {
        let toml = r#"
description = "d"

[server]
transport = "stdio"
command = "grim"
timeout = 30000
always_load = true
cwd = "./srv"
"#;
        let d = McpDescriptor::from_toml_str(toml).unwrap();
        assert_eq!(d.server.timeout, Some(30000));
        assert_eq!(d.server.always_load, Some(true));
        assert_eq!(d.server.cwd.as_deref(), Some("./srv"));
        let bytes = d.to_layer_bytes().unwrap();
        assert_eq!(d, McpDescriptor::from_layer_bytes(&bytes).unwrap());

        let remote = r#"
description = "d"

[server]
transport = "http"
url = "https://api.example.com/mcp"
timeout = 5000
headers_helper = "/usr/local/bin/fresh-token"
"#;
        let d = McpDescriptor::from_toml_str(remote).unwrap();
        assert_eq!(d.server.headers_helper.as_deref(), Some("/usr/local/bin/fresh-token"));
        let bytes = d.to_layer_bytes().unwrap();
        assert_eq!(d, McpDescriptor::from_layer_bytes(&bytes).unwrap());
    }

    #[test]
    fn refinement_field_on_wrong_transport_is_rejected() {
        // headers_helper is remote-only.
        let err = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"x\"\nheaders_helper = \"h\"",
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                McpError::RefinementShape {
                    field: "headers_helper",
                    ..
                }
            ),
            "{err}"
        );
        // cwd is stdio-only.
        let err = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"\ncwd = \".\"",
        )
        .unwrap_err();
        assert!(matches!(err, McpError::RefinementShape { field: "cwd", .. }), "{err}");
    }

    #[test]
    fn old_descriptor_without_refinement_fields_parses() {
        // Backward-compat lock: a layer blob published by an older grim
        // (no refinement fields) parses unchanged, and a descriptor not
        // using the new fields serializes byte-identically to the old
        // canonical layer shape (no new keys appear).
        let d = McpDescriptor::from_toml_str(STDIO).unwrap();
        let bytes = d.to_layer_bytes().unwrap();
        let json = String::from_utf8(bytes.clone()).unwrap();
        for absent in ["timeout", "always_load", "headers_helper", "cwd"] {
            assert!(
                !json.contains(absent),
                "unused refinement field '{absent}' must not serialize"
            );
        }
        assert_eq!(d, McpDescriptor::from_layer_bytes(&bytes).unwrap());
    }

    #[test]
    fn env_ref_names_iterates_references() {
        let names: Vec<&str> = env_ref_names("--dsn ${DB_DSN} --token ${API_KEY}").collect();
        assert_eq!(names, ["DB_DSN", "API_KEY"]);
        assert_eq!(env_ref_names("no refs $PLAIN").count(), 0);
    }

    #[test]
    fn layer_cap_is_enforced() {
        let big = vec![b' '; (MCP_LAYER_SIZE_LIMIT + 1) as usize];
        let err = McpDescriptor::from_layer_bytes(&big).unwrap_err();
        assert!(matches!(err, McpError::TooLarge { .. }));
    }
}
