// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Plugin formats: client → family map, the admission gate, version and
//! description, the two `plugin.json` emitters and the Agent Plugins
//! `mcp.json` entry (design record C-015, C-016, C-023, C-024, C-025,
//! C-036).

use serde::Serialize;

use crate::export::export_error::ExportError;
use crate::export::marketplace::normalize_version;
use crate::install::ClientTarget;
use crate::install::vendor::KindSupport;
use crate::lock::LockedArtifact;
use crate::oci::mcp::{McpDescriptor, McpTransport};
use crate::oci::{Algorithm, ArtifactKind};

/// Hex digits of the member digest hash appended to a plugin version.
const VERSION_SUFFIX_LEN: usize = 12;

/// The on-ramp sentence ending every plugin description (C-024).
pub const ONRAMP: &str = "Packaged by grim (https://grimoire.rs); install grim for pinned, updatable installs.";

/// The `$schema` value of an Agent Plugins `plugin.json` (C-025).
pub const AGENT_PLUGINS_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";

/// A plugin format family (C-015). Serializes as `claude` /
/// `agent-plugins`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Family {
    /// Claude Code plugin: `.claude-plugin/plugin.json`, `.mcp.json`.
    Claude,
    /// Agent Plugins 1.0: `plugin.json` with `$schema`, `mcp.json`.
    AgentPlugins,
}

/// Why a member was left out of one client's plugin (C-016, C-020, C-036).
/// Serializes as the kebab literals `client-declined`, `no-format-surface`,
/// `not-representable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OmitReason {
    /// The client's install path declines the kind.
    ClientDeclined,
    /// The plugin format has no place for the kind.
    NoFormatSurface,
    /// The member's content cannot be expressed in the format.
    NotRepresentable,
}

/// Claude-family `plugin.json`: exactly these keys, in this order.
#[derive(Serialize)]
struct ClaudeManifest<'a> {
    name: &'a str,
    version: &'a str,
    description: &'a str,
}

/// Agent Plugins `plugin.json`: exactly these keys, in this order.
#[derive(Serialize)]
struct AgentPluginsManifest<'a> {
    #[serde(rename = "$schema")]
    schema: &'static str,
    name: &'a str,
    version: &'a str,
    description: &'a str,
}

/// The plugin family of `client`, or `None` when it has no plugin format
/// (C-015).
pub fn family_of(client: ClientTarget) -> Option<Family> {
    use ClientTarget::{Agents, Claude, Codex, Copilot, Cursor, Droid, Junie, OpenClaw};
    match client {
        Claude | Droid | Junie | OpenClaw => Some(Family::Claude),
        Copilot | Codex | Cursor | Agents => Some(Family::AgentPlugins),
        _ => None,
    }
}

/// Every client [`family_of`] maps, comma-joined in [`ClientTarget::ALL`]
/// order — the `NoPluginFormat` hint.
pub fn plugin_client_names() -> String {
    ClientTarget::ALL
        .into_iter()
        .filter(|c| family_of(*c).is_some())
        .map(|c| c.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether a member of `kind` goes into `client`'s plugin of `family`
/// (C-016). MCP members may still be omitted later as `not-representable`.
/// Bundles never reach this gate (resolution expands them into members);
/// [`ArtifactKind::Bundle`] returns [`OmitReason::NoFormatSurface`]
/// defensively.
pub fn admits(family: Family, client: ClientTarget, kind: ArtifactKind) -> Result<(), OmitReason> {
    use ArtifactKind::{Agent, Bundle, Mcp, Rule, Skill};
    match (family, kind) {
        (_, Rule | Bundle) | (Family::AgentPlugins, Agent) => Err(OmitReason::NoFormatSurface),
        // The Agent Plugins `mcp.json` shape is the family's, not the
        // client's: no install-time `kind_support` gate.
        (Family::AgentPlugins, Mcp) => Ok(()),
        (_, Skill) | (Family::Claude, Agent | Mcp) => match client.vendor().kind_support(kind) {
            KindSupport::Declined => Err(OmitReason::ClientDeclined),
            KindSupport::Native | KindSupport::Degraded => Ok(()),
        },
    }
}

/// The plugin version (C-023): `base` (`--version`, declared or annotation
/// version, else `0.0.0`) with one leading `v` stripped, plus `+` and 12 hex
/// of the member digest hash. `members` are `(locked member, emitted name)`.
pub fn plugin_version(base: Option<&str>, members: &[(LockedArtifact, String)]) -> Result<String, ExportError> {
    let base = match base {
        Some(raw) => normalize_version(raw).ok_or_else(|| ExportError::InvalidVersion { value: raw.to_string() })?,
        None => "0.0.0".to_string(),
    };
    let mut lines: Vec<String> = members
        .iter()
        .map(|(member, emitted)| format!("{}\t{emitted}\t{}\n", member.kind, member.source.content_digest()))
        .collect();
    lines.sort();
    let digest = Algorithm::Sha256.hash(lines.concat());
    let suffix = digest.hex().get(..VERSION_SUFFIX_LEN).unwrap_or(digest.hex());
    Ok(format!("{base}+{suffix}"))
}

/// Most characters a plugin `description` may carry, counted in UTF-16
/// code units (the JavaScript `length` harness validators apply, never
/// fewer than Unicode scalars).
pub const MAX_DESCRIPTION_LEN: usize = 500;

/// Length of `s` as [`MAX_DESCRIPTION_LEN`] counts it.
pub fn description_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Most characters an author-written description base may carry: whatever
/// [`ONRAMP`] and its separating space leave of [`MAX_DESCRIPTION_LEN`].
pub fn max_description_base_len() -> usize {
    MAX_DESCRIPTION_LEN - description_len(ONRAMP) - 1
}

/// `s` cut to at most `max` units, ending in `…` when cut; empty when not
/// even the ellipsis fits beside some text.
fn fit(s: &str, max: usize) -> String {
    if description_len(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 1; // the ellipsis
    for c in s.chars() {
        used += c.len_utf16();
        if used > max {
            break;
        }
        out.push(c);
    }
    let kept = out.trim_end();
    if kept.is_empty() {
        String::new()
    } else {
        format!("{kept}…")
    }
}

/// The plugin description (C-024): `base`, the omission sentence, then
/// [`ONRAMP`], space-joined, never longer than [`MAX_DESCRIPTION_LEN`].
/// The on-ramp is kept whole; the base is cut first, then the omission
/// sentence, each ending in `…` when cut. `omitted` is `(kind, emitted
/// name)`, already sorted by the caller in C-016 order. The flag is true
/// when anything was cut.
pub fn plugin_description(base: Option<&str>, omitted: &[(ArtifactKind, String)]) -> (String, bool) {
    let mut tail: Vec<String> = Vec::new();
    let mut cut = false;
    if !omitted.is_empty() {
        let list: Vec<String> = omitted.iter().map(|(kind, name)| format!("{kind} {name}")).collect();
        let sentence = format!("Omitted for this client: {}.", list.join(", "));
        let fitted = fit(&sentence, max_description_base_len());
        cut |= fitted != sentence;
        tail.push(fitted);
    }
    tail.push(ONRAMP.to_string());
    let tail = tail.join(" ");
    let mut parts: Vec<String> = Vec::new();
    if let Some(base) = base.map(str::trim).filter(|b| !b.is_empty()) {
        let room = MAX_DESCRIPTION_LEN.saturating_sub(description_len(&tail) + 1);
        let fitted = fit(base, room);
        cut |= fitted != base;
        if !fitted.is_empty() {
            parts.push(fitted);
        }
    }
    parts.push(tail);
    (parts.join(" "), cut)
}

/// Bytes of `.claude-plugin/plugin.json` (C-025): pretty JSON plus one `\n`.
///
/// Serializing a struct of `&str` cannot fail, so this returns bytes, not a
/// `Result`; the body uses a non-panicking form (no `unwrap`/`expect`, per
/// the crate lints).
pub fn claude_plugin_json(name: &str, version: &str, description: &str) -> Vec<u8> {
    pretty_json_line(&ClaudeManifest {
        name,
        version,
        description,
    })
}

/// Bytes of the Agent Plugins `plugin.json` (C-025): pretty JSON plus one
/// `\n`, `$schema` first.
///
/// Serializing a struct of `&str` cannot fail, so this returns bytes, not a
/// `Result`; the body uses a non-panicking form (no `unwrap`/`expect`, per
/// the crate lints).
pub fn agent_plugins_plugin_json(name: &str, version: &str, description: &str) -> Vec<u8> {
    pretty_json_line(&AgentPluginsManifest {
        schema: AGENT_PLUGINS_SCHEMA,
        name,
        version,
        description,
    })
}

/// `to_vec_pretty` plus one `\n`. Serializing a struct of `&str` cannot
/// fail; the empty fallback keeps the crate's no-panic discipline (an empty
/// manifest would fail loudly downstream, never ship silently valid).
fn pretty_json_line(value: &impl Serialize) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).unwrap_or_default();
    bytes.push(b'\n');
    bytes
}

/// The Agent Plugins `mcp.json` `mcpServers` value for one server (C-036);
/// `None` when the descriptor cannot be represented.
pub fn agent_plugins_mcp_entry(name: &str, d: &McpDescriptor) -> Option<serde_json::Value> {
    let s = &d.server;
    // The spec expands `${…}` only in args, env values and cwd; anywhere
    // else a reference would be sent literally, and it has no auth or
    // websocket surface — decline rather than emit a broken server.
    let unexpanded = |v: &str| v.contains("${");
    if s.oauth.is_some()
        || s.command.as_deref().is_some_and(unexpanded)
        || s.url.as_deref().is_some_and(unexpanded)
        || s.env.keys().any(|k| unexpanded(k))
        || s.headers.iter().any(|(k, v)| unexpanded(k) || unexpanded(v))
    {
        return None;
    }

    let mut entry = serde_json::Map::new();
    match s.transport {
        McpTransport::Stdio => {
            entry.insert("type".into(), serde_json::json!("stdio"));
            entry.insert("command".into(), serde_json::json!(s.command));
            if !s.args.is_empty() {
                entry.insert("args".into(), serde_json::json!(s.args));
            }
            if !s.env.is_empty() {
                entry.insert("env".into(), serde_json::json!(s.env));
            }
            if let Some(cwd) = &s.cwd {
                entry.insert("cwd".into(), serde_json::json!(cwd));
            }
        }
        McpTransport::Ws => return None,
        McpTransport::Http | McpTransport::Sse => {
            // Agent Plugins names streamable HTTP `streamable-http`.
            let kind = if s.transport == McpTransport::Http {
                "streamable-http"
            } else {
                "sse"
            };
            entry.insert("type".into(), serde_json::json!(kind));
            entry.insert("url".into(), serde_json::json!(s.url));
            if !s.headers.is_empty() {
                entry.insert("headers".into(), serde_json::json!(s.headers));
            }
        }
    }
    // Refinement fields with no Agent Plugins key — dropped, as every
    // vendor projection does; the server itself is still emitted.
    for (field, present) in [
        ("timeout", s.timeout.is_some()),
        ("always_load", s.always_load.is_some()),
        ("headers_helper", s.headers_helper.is_some()),
    ] {
        if present {
            tracing::warn!("mcp server '{name}': `{field}` has no Agent Plugins mcp.json key; dropped");
        }
    }
    Some(serde_json::Value::Object(entry))
}

#[cfg(test)]
mod tests {
    //! Specification tests written from the design record (C-015, C-016,
    //! C-023, C-024, C-025, C-036; S-031 unit half), not from the
    //! implementation.

    use serde_json::json;

    use super::*;
    use crate::config::PathSource;
    use crate::lock::LockedSource;
    use crate::oci::{Digest, Identifier, PinnedIdentifier};

    // ── C-015 family map ──

    #[test]
    fn c015_family_map_is_total_over_every_client() {
        use ClientTarget::*;
        for client in ClientTarget::ALL {
            let expected = match client {
                Claude | Droid | Junie | OpenClaw => Some(Family::Claude),
                Copilot | Codex | Cursor | Agents => Some(Family::AgentPlugins),
                _ => None,
            };
            assert_eq!(family_of(client), expected, "{client}");
        }
    }

    #[test]
    fn c015_c016_family_and_omit_reason_serialize_as_kebab_literals() {
        assert_eq!(serde_json::to_value(Family::Claude).unwrap(), json!("claude"));
        assert_eq!(
            serde_json::to_value(Family::AgentPlugins).unwrap(),
            json!("agent-plugins")
        );
        assert_eq!(
            serde_json::to_value(OmitReason::ClientDeclined).unwrap(),
            json!("client-declined")
        );
        assert_eq!(
            serde_json::to_value(OmitReason::NoFormatSurface).unwrap(),
            json!("no-format-surface")
        );
        assert_eq!(
            serde_json::to_value(OmitReason::NotRepresentable).unwrap(),
            json!("not-representable")
        );
    }

    // ── C-016 admission gate ──

    /// C-016 table as expected today (its last paragraph): every phase-1
    /// client × every kind, bundle included defensively.
    #[test]
    fn c016_admission_table_per_phase1_client() {
        use ArtifactKind::{Agent, Bundle, Mcp, Rule, Skill};
        use OmitReason::{ClientDeclined as Cd, NoFormatSurface as Nfs};
        let ok = Ok(());
        // (family, client, skill, agent, mcp, rule)
        let table = [
            (Family::Claude, ClientTarget::Claude, ok, ok, ok, Err(Nfs)),
            (Family::Claude, ClientTarget::Droid, ok, Err(Cd), Err(Cd), Err(Nfs)),
            (Family::Claude, ClientTarget::Junie, ok, Err(Cd), ok, Err(Nfs)),
            (Family::Claude, ClientTarget::OpenClaw, ok, Err(Cd), Err(Cd), Err(Nfs)),
            // Agent Plugins: agents have no format surface (not client-declined,
            // even where the client would install them); MCP is the family's
            // file, admitted even for `agents`, which declines MCP on install.
            (Family::AgentPlugins, ClientTarget::Copilot, ok, Err(Nfs), ok, Err(Nfs)),
            (Family::AgentPlugins, ClientTarget::Codex, ok, Err(Nfs), ok, Err(Nfs)),
            (Family::AgentPlugins, ClientTarget::Cursor, ok, Err(Nfs), ok, Err(Nfs)),
            (Family::AgentPlugins, ClientTarget::Agents, ok, Err(Nfs), ok, Err(Nfs)),
        ];
        for (family, client, skill, agent, mcp, rule) in table {
            assert_eq!(admits(family, client, Skill), skill, "{client} skill");
            assert_eq!(admits(family, client, Agent), agent, "{client} agent");
            assert_eq!(admits(family, client, Mcp), mcp, "{client} mcp");
            // Rules: no plugin surface in either family — even where the
            // client declines rules on install (codex), the reason is the format's.
            assert_eq!(admits(family, client, Rule), rule, "{client} rule");
            assert_eq!(admits(family, client, Bundle), Err(Nfs), "{client} bundle");
        }
    }

    // ── C-023 version ──

    fn sha(byte: char) -> Digest {
        Digest::Sha256(std::iter::repeat_n(byte, 64).collect())
    }

    fn registry_member(name: &str, kind: ArtifactKind, byte: char) -> LockedArtifact {
        let id = Identifier::new_registry(name, "localhost:5000").clone_with_digest(sha(byte));
        LockedArtifact::direct(name.to_string(), kind, PinnedIdentifier::try_from(id).unwrap())
    }

    /// Three members over both source kinds, deliberately not in hash-line
    /// order: skill `team-plan`→`plan` (a…), mcp `srv` (b…), path agent
    /// `reviewer` (c…).
    fn members(plan_emitted: &str, plan_byte: char) -> Vec<(LockedArtifact, String)> {
        let agent = LockedArtifact {
            name: "reviewer".into(),
            kind: ArtifactKind::Agent,
            source: LockedSource::Path {
                path: PathSource::parse("./agents/reviewer.md").unwrap(),
                hash: sha('c'),
            },
            bundles: Vec::new(),
        };
        vec![
            (
                registry_member("team-plan", ArtifactKind::Skill, plan_byte),
                plan_emitted.into(),
            ),
            (registry_member("srv", ArtifactKind::Mcp, 'b'), "srv".into()),
            (agent, "reviewer".into()),
        ]
    }

    /// Independently computed (`printf … | sha256sum`) over
    /// `agent\treviewer\tsha256:c…\nmcp\tsrv\tsha256:b…\nskill\tplan\tsha256:a…\n`.
    const SUFFIX: &str = "c5a5324c93a6";

    #[test]
    fn c023_suffix_is_12_hex_of_sha256_over_sorted_member_lines() {
        assert_eq!(
            plugin_version(Some("1.2.0"), &members("plan", 'a')).unwrap(),
            format!("1.2.0+{SUFFIX}")
        );
    }

    #[test]
    fn c023_same_pins_twice_same_version_regardless_of_member_order() {
        let a = plugin_version(Some("1.2.0"), &members("plan", 'a')).unwrap();
        let mut reversed = members("plan", 'a');
        reversed.reverse();
        assert_eq!(a, plugin_version(Some("1.2.0"), &members("plan", 'a')).unwrap());
        assert_eq!(a, plugin_version(Some("1.2.0"), &reversed).unwrap());
    }

    #[test]
    fn c023_digest_change_changes_suffix() {
        let a = plugin_version(None, &members("plan", 'a')).unwrap();
        let d = plugin_version(None, &members("plan", 'd')).unwrap();
        assert_ne!(a, d);
    }

    #[test]
    fn c023_rename_changes_suffix_and_hashes_emitted_not_lock_name() {
        // Unrenamed: the skill line carries `team-plan` (sha256sum-computed).
        assert_eq!(
            plugin_version(None, &members("team-plan", 'a')).unwrap(),
            "0.0.0+d4c3184adf04"
        );
        assert_eq!(
            plugin_version(None, &members("plan", 'a')).unwrap(),
            format!("0.0.0+{SUFFIX}")
        );
    }

    #[test]
    fn c023_base_normalization() {
        let m = members("plan", 'a');
        assert_eq!(plugin_version(None, &m).unwrap(), format!("0.0.0+{SUFFIX}"));
        assert_eq!(plugin_version(Some("v2.0.0"), &m).unwrap(), format!("2.0.0+{SUFFIX}"));
        // Pre-release is allowed; only build metadata is refused.
        assert_eq!(
            plugin_version(Some("1.0.0-rc.1"), &m).unwrap(),
            format!("1.0.0-rc.1+{SUFFIX}")
        );
    }

    #[test]
    fn c023_invalid_base_is_invalid_version_carrying_the_input() {
        let m = members("plan", 'a');
        // `vv1.0.0`: only one leading `v` is stripped.
        for bad in ["1.0.0+x", "latest", "vv1.0.0", "1.0", ""] {
            match plugin_version(Some(bad), &m) {
                Err(ExportError::InvalidVersion { value }) => assert_eq!(value, bad),
                other => panic!("{bad:?}: expected InvalidVersion, got {other:?}"),
            }
        }
    }

    // ── C-024 description ──

    #[test]
    fn c024_onramp_exact_bytes() {
        assert_eq!(
            ONRAMP,
            "Packaged by grim (https://grimoire.rs); install grim for pinned, updatable installs."
        );
    }

    #[test]
    fn c024_description_without_base_or_omissions_is_onramp_only() {
        assert_eq!(
            plugin_description(None, &[]).0,
            "Packaged by grim (https://grimoire.rs); install grim for pinned, updatable installs."
        );
    }

    #[test]
    fn c024_base_is_trimmed_and_blank_base_dropped() {
        assert_eq!(
            plugin_description(Some("  Team tools\n"), &[]).0,
            "Team tools Packaged by grim (https://grimoire.rs); install grim for pinned, updatable installs."
        );
        for blank in ["", "   ", "\n\t "] {
            assert_eq!(plugin_description(Some(blank), &[]).0, ONRAMP, "{blank:?}");
        }
    }

    #[test]
    fn c024_omissions_sentence_between_base_and_onramp() {
        let omitted = [
            (ArtifactKind::Agent, "reviewer".to_string()),
            (ArtifactKind::Mcp, "srv".to_string()),
        ];
        assert_eq!(
            plugin_description(Some("Team tools"), &omitted).0,
            "Team tools Omitted for this client: agent reviewer, mcp srv. \
             Packaged by grim (https://grimoire.rs); install grim for pinned, updatable installs."
        );
        assert_eq!(
            plugin_description(Some(" "), &omitted[..1]).0,
            "Omitted for this client: agent reviewer. \
             Packaged by grim (https://grimoire.rs); install grim for pinned, updatable installs."
        );
    }

    #[test]
    fn c024_description_fitting_the_cap_is_uncut() {
        let base = "b".repeat(max_description_base_len());
        let (desc, cut) = plugin_description(Some(&base), &[]);
        assert!(!cut);
        assert_eq!(description_len(&desc), MAX_DESCRIPTION_LEN);
        assert_eq!(desc, format!("{base} {ONRAMP}"));
    }

    #[test]
    fn c024_long_base_is_cut_with_ellipsis_onramp_kept_whole() {
        let omitted = [(ArtifactKind::Rule, "r".to_string())];
        let base = "word ".repeat(200);
        let (desc, cut) = plugin_description(Some(&base), &omitted);
        assert!(cut);
        assert!(description_len(&desc) <= MAX_DESCRIPTION_LEN);
        assert!(desc.starts_with("word word"), "{desc}");
        assert!(
            desc.ends_with(&format!("… Omitted for this client: rule r. {ONRAMP}")),
            "{desc}"
        );
        assert!(!desc.contains(" …"), "no blank before the ellipsis: {desc}");
    }

    #[test]
    fn c024_cut_counts_utf16_units_and_never_splits_a_char() {
        // 🦀 is two UTF-16 units: the budget is spent in units, not chars.
        let base = "🦀".repeat(400);
        let (desc, cut) = plugin_description(Some(&base), &[]);
        assert!(cut);
        assert!(description_len(&desc) <= MAX_DESCRIPTION_LEN);
        assert!(desc.starts_with('🦀') && desc.contains("🦀… Packaged"), "{desc}");
    }

    #[test]
    fn c024_huge_omission_list_drops_base_and_cuts_sentence() {
        let omitted: Vec<(ArtifactKind, String)> =
            (0..100).map(|i| (ArtifactKind::Rule, format!("rule-{i:03}"))).collect();
        let (desc, cut) = plugin_description(Some("Team tools"), &omitted);
        assert!(cut);
        assert!(description_len(&desc) <= MAX_DESCRIPTION_LEN);
        assert!(desc.starts_with("Omitted for this client: rule rule-000"), "{desc}");
        assert!(desc.ends_with(&format!("… {ONRAMP}")), "{desc}");
    }

    // ── C-025 plugin.json emitters ──

    #[test]
    fn c025_agent_plugins_schema_const() {
        assert_eq!(
            AGENT_PLUGINS_SCHEMA,
            "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json"
        );
    }

    #[test]
    fn c025_claude_plugin_json_exact_bytes() {
        let got = claude_plugin_json("team", "1.0.0+c5a5324c93a6", "Team tools");
        assert_eq!(
            String::from_utf8(got).unwrap(),
            "{\n  \"name\": \"team\",\n  \"version\": \"1.0.0+c5a5324c93a6\",\n  \"description\": \"Team tools\"\n}\n"
        );
    }

    #[test]
    fn c025_agent_plugins_plugin_json_exact_bytes_schema_first() {
        let got = agent_plugins_plugin_json("team", "1.0.0+c5a5324c93a6", "Team tools");
        assert_eq!(
            String::from_utf8(got).unwrap(),
            "{\n  \"$schema\": \"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json\",\n  \
             \"name\": \"team\",\n  \"version\": \"1.0.0+c5a5324c93a6\",\n  \"description\": \"Team tools\"\n}\n"
        );
    }

    /// Top-level keys in document order, read off the pretty output (a
    /// parsed `serde_json::Map` does not promise to keep document order).
    fn top_level_keys(bytes: &[u8]) -> Vec<String> {
        std::str::from_utf8(bytes)
            .unwrap()
            .lines()
            .filter_map(|l| l.strip_prefix("  \""))
            .filter_map(|l| l.split_once("\":").map(|(k, _)| k.to_string()))
            .collect()
    }

    #[test]
    fn c025_key_lists_order_and_single_trailing_newline() {
        let desc = "Say \"hi\" — é\nnext";
        let claude = claude_plugin_json("team", "0.0.0+c5a5324c93a6", desc);
        let agent = agent_plugins_plugin_json("team", "0.0.0+c5a5324c93a6", desc);
        assert_eq!(top_level_keys(&claude), ["name", "version", "description"]);
        assert_eq!(top_level_keys(&agent), ["$schema", "name", "version", "description"]);
        for bytes in [&claude, &agent] {
            assert!(bytes.ends_with(b"}\n") && !bytes.ends_with(b"\n\n"));
            let v: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            assert_eq!(v["description"], desc, "escaped and round-trips");
        }
        let v: serde_json::Value = serde_json::from_slice(&agent).unwrap();
        assert_eq!(v["$schema"], AGENT_PLUGINS_SCHEMA);
    }

    // ── C-036 Agent Plugins mcp.json entry (S-031 unit half) ──

    fn desc(server: &str) -> McpDescriptor {
        McpDescriptor::from_toml_str(&format!("description = \"d\"\n[server]\n{server}")).unwrap()
    }

    #[test]
    fn c036_stdio_minimal() {
        let d = desc("transport = \"stdio\"\ncommand = \"grim\"");
        assert_eq!(
            agent_plugins_mcp_entry("m", &d),
            Some(json!({"type": "stdio", "command": "grim"})),
            "empty args/env omitted, no cwd"
        );
    }

    #[test]
    fn c036_stdio_full_passes_expandable_refs_verbatim() {
        let d = desc(
            "transport = \"stdio\"\ncommand = \"grim\"\nargs = [\"mcp\", \"${ROOT}\"]\n\
             env = { TOKEN = \"${TOKEN}\" }\ncwd = \"${HOME}/srv\"",
        );
        assert_eq!(
            agent_plugins_mcp_entry("m", &d),
            Some(json!({
                "type": "stdio",
                "command": "grim",
                "args": ["mcp", "${ROOT}"],
                "env": {"TOKEN": "${TOKEN}"},
                "cwd": "${HOME}/srv",
            }))
        );
    }

    #[test]
    fn c036_http_is_streamable_http() {
        let d = desc("transport = \"http\"\nurl = \"https://x/mcp\"\nheaders = { \"X-Team\" = \"core\" }");
        assert_eq!(
            agent_plugins_mcp_entry("web", &d),
            Some(json!({"type": "streamable-http", "url": "https://x/mcp", "headers": {"X-Team": "core"}}))
        );
        let bare = desc("transport = \"http\"\nurl = \"https://x/mcp\"");
        assert_eq!(
            agent_plugins_mcp_entry("web", &bare),
            Some(json!({"type": "streamable-http", "url": "https://x/mcp"})),
            "empty headers omitted"
        );
    }

    #[test]
    fn c036_sse() {
        let d = desc("transport = \"sse\"\nurl = \"https://x/sse\"");
        assert_eq!(
            agent_plugins_mcp_entry("feed", &d),
            Some(json!({"type": "sse", "url": "https://x/sse"}))
        );
        let h = desc("transport = \"sse\"\nurl = \"https://x/sse\"\nheaders = { A = \"b\" }");
        assert_eq!(
            agent_plugins_mcp_entry("feed", &h),
            Some(json!({"type": "sse", "url": "https://x/sse", "headers": {"A": "b"}}))
        );
    }

    #[test]
    fn c036_declines_ws_and_oauth() {
        let ws = desc("transport = \"ws\"\nurl = \"wss://x/socket\"");
        assert_eq!(agent_plugins_mcp_entry("sock", &ws), None);
        let oauth = desc("transport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nclient_id = \"c\"");
        assert_eq!(agent_plugins_mcp_entry("authd", &oauth), None);
    }

    #[test]
    fn c036_declines_unexpandable_env_refs() {
        let cases = [
            ("command", desc("transport = \"stdio\"\ncommand = \"${BIN}/srv\"")),
            ("url", desc("transport = \"http\"\nurl = \"https://${HOST}/mcp\"")),
            (
                "header value",
                desc("transport = \"http\"\nurl = \"https://x\"\nheaders = { Authorization = \"Bearer ${TOKEN}\" }"),
            ),
            ("env key", {
                let mut d = desc("transport = \"stdio\"\ncommand = \"grim\"");
                d.server.env.insert("${KEY}".into(), "v".into());
                d
            }),
            ("header name", {
                let mut d = desc("transport = \"sse\"\nurl = \"https://x\"");
                d.server.headers.insert("X-${K}".into(), "v".into());
                d
            }),
        ];
        for (what, d) in cases {
            assert_eq!(agent_plugins_mcp_entry("m", &d), None, "`${{` in {what}");
        }
    }

    #[test]
    fn c036_drops_refinement_fields() {
        let stdio = desc("transport = \"stdio\"\ncommand = \"grim\"\ntimeout = 7000\nalways_load = true");
        assert_eq!(
            agent_plugins_mcp_entry("m", &stdio),
            Some(json!({"type": "stdio", "command": "grim"}))
        );
        let http = desc("transport = \"http\"\nurl = \"https://x/mcp\"\ntimeout = 7000\nheaders_helper = \"./h.sh\"");
        assert_eq!(
            agent_plugins_mcp_entry("web", &http),
            Some(json!({"type": "streamable-http", "url": "https://x/mcp"}))
        );
    }

    #[test]
    fn c036_byte_identical_across_calls() {
        let d =
            desc("transport = \"stdio\"\ncommand = \"grim\"\nargs = [\"a\", \"b\"]\nenv = { Z = \"1\", A = \"2\" }");
        let a = serde_json::to_vec(&agent_plugins_mcp_entry("m", &d).unwrap()).unwrap();
        let b = serde_json::to_vec(&agent_plugins_mcp_entry("m", &d).unwrap()).unwrap();
        assert_eq!(a, b);
    }
}
