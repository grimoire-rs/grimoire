// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Plugin formats: client → family map, the admission gate, version and
//! description, the two `plugin.json` emitters and the Agent Plugins
//! `mcp.json` entry (design record C-015, C-016, C-023, C-024, C-025,
//! C-036).

use serde::Serialize;

use crate::export::archive::InventoryEntry;
use crate::install::ClientTarget;
use crate::install::vendor::KindSupport;
use crate::oci::mcp::{McpDescriptor, McpTransport};
use crate::oci::{Algorithm, ArtifactKind};

/// Hex digits of the tree hash appended to a plugin version.
const VERSION_SUFFIX_LEN: usize = 12;

/// The on-ramp sentence closing every plugin `README.md`, and the
/// description of a plugin with no base text (C-024).
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

/// Agent Plugins `plugin.json`: exactly these keys, in this order;
/// `extensions` only when the plugin has a logo.
#[derive(Serialize)]
struct AgentPluginsManifest<'a> {
    #[serde(rename = "$schema")]
    schema: &'static str,
    name: &'a str,
    version: &'a str,
    description: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    extensions: Option<serde_json::Value>,
}

/// Where a plugin logo lands in every exported tree, by file extension.
pub fn logo_path(ext: &str) -> String {
    format!("assets/logo.{ext}")
}

/// The plugin family of `client`, or `None` when it has no plugin format
/// (C-015).
pub fn family_of(client: ClientTarget) -> Option<Family> {
    use ClientTarget::{Agents, Claude, Codex, Copilot, Cursor, Droid, Junie, OpenClaw, Qoder};
    match client {
        Claude | Droid | Junie | OpenClaw | Qoder => Some(Family::Claude),
        Copilot | Codex | Cursor | Agents => Some(Family::AgentPlugins),
        _ => None,
    }
}

/// The manifest path of `client`'s plugin tree (C-005): `.qoder-plugin/plugin.json`
/// for Qoder, `.claude-plugin/plugin.json` for every other Claude-family
/// client, `plugin.json` for Agent Plugins. A client with no plugin format
/// gets the Claude path; export never renders one.
pub fn manifest_rel(client: ClientTarget) -> &'static str {
    match (client, family_of(client)) {
        (ClientTarget::Qoder, _) => ".qoder-plugin/plugin.json",
        (_, Some(Family::AgentPlugins)) => "plugin.json",
        _ => ".claude-plugin/plugin.json",
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

/// The plugin version (C-002): `base` (already normalized) plus `+` and 12
/// hex of the SHA-256 over the compact JSON array of `[name, exec, sha256]`
/// triples of the rendered tree. JSON, not a joined text, so no file name can
/// forge a neighbouring entry.
pub fn plugin_version(base: &str, inventory: &[InventoryEntry]) -> String {
    let triples: Vec<(&str, bool, &str)> = inventory
        .iter()
        .map(|e| (e.name.as_str(), e.exec, e.sha256.as_str()))
        .collect();
    let json = serde_json::to_vec(&triples).unwrap_or_default();
    let digest = Algorithm::Sha256.hash(json);
    let suffix = digest.hex().get(..VERSION_SUFFIX_LEN).unwrap_or(digest.hex());
    format!("{base}+{suffix}")
}

pub use crate::config::plugin_meta::{MAX_DESCRIPTION_LEN, description_len};

/// `s` cut to at most `max` units: at the last whole sentence when that
/// keeps half the budget, else at a word boundary ending in `…`. Trailing
/// separators (`,;:(-`) go with the cut word; a single word longer
/// than `max` is cut mid-word, the only way it fits.
fn cut_at_word(s: &str, max: usize) -> String {
    let mut end = 0;
    let mut used = 1; // the ellipsis
    for (i, c) in s.char_indices() {
        used += c.len_utf16();
        if used > max {
            break;
        }
        end = i + c.len_utf8();
    }
    let head = &s[..end];
    // A whole sentence reads better than a cut one: end at the last one when
    // that keeps at least half the budget.
    let sentence_end = head
        .char_indices()
        .filter(|&(i, c)| matches!(c, '.' | '!' | '?') && s[i + c.len_utf8()..].starts_with(char::is_whitespace))
        .map(|(i, c)| i + c.len_utf8())
        .next_back();
    if let Some(e) = sentence_end.filter(|&e| description_len(&s[..e]) * 2 >= max) {
        return s[..e].to_string();
    }
    let at_boundary = s[end..].starts_with(char::is_whitespace);
    let head = match head.rfind(char::is_whitespace) {
        Some(space) if !at_boundary => &head[..space],
        _ => head,
    };
    let head = head.trim_end_matches(|c: char| c.is_whitespace() || ",;:(-".contains(c));
    format!("{head}…")
}

/// The plugin description (C-024): the trimmed base alone, cut to
/// [`MAX_DESCRIPTION_LEN`] (see [`cut_at_word`]); [`ONRAMP`] when there is no base.
/// Omissions and the on-ramp live in the plugin's `README.md`
/// ([`plugin_readme`]), never here. The flag is true when the base was cut.
pub fn plugin_description(base: Option<&str>) -> (String, bool) {
    match base.map(str::trim).filter(|b| !b.is_empty()) {
        None => (ONRAMP.to_string(), false),
        Some(b) if description_len(b) <= MAX_DESCRIPTION_LEN => (b.to_string(), false),
        Some(b) => (cut_at_word(b, MAX_DESCRIPTION_LEN), true),
    }
}

/// The plugin root's `README.md` (C-024): the name, the logo image when
/// there is one, the full uncut base, the members omitted for `client`, then
/// [`ONRAMP`]. `omitted` is `(kind, emitted name)` in C-016 order.
pub fn plugin_readme(
    name: &str,
    client: ClientTarget,
    base: Option<&str>,
    omitted: &[(ArtifactKind, String)],
    logo: Option<&str>,
) -> Vec<u8> {
    let mut parts = vec![format!("# {name}")];
    if let Some(logo) = logo {
        parts.push(format!("![{name}]({logo})"));
    }
    if let Some(base) = base.map(str::trim).filter(|b| !b.is_empty()) {
        parts.push(base.to_string());
    }
    if !omitted.is_empty() {
        let list: Vec<String> = omitted.iter().map(|(kind, name)| format!("{kind} {name}")).collect();
        parts.push(format!("Omitted for {client}: {}.", list.join(", ")));
    }
    parts.push(ONRAMP.to_string());
    let mut text = parts.join("\n\n");
    text.push('\n');
    text.into_bytes()
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
///
/// `logo` is the plugin-root-relative logo path ([`logo_path`]); it is
/// declared under the Codex namespace (`extensions."com.openai".interface.logo`,
/// `./`-prefixed), the one Agent Plugins client that reads a logo.
pub fn agent_plugins_plugin_json(name: &str, version: &str, description: &str, logo: Option<&str>) -> Vec<u8> {
    pretty_json_line(&AgentPluginsManifest {
        schema: AGENT_PLUGINS_SCHEMA,
        name,
        version,
        description,
        extensions: logo.map(|l| serde_json::json!({"com.openai": {"interface": {"logo": format!("./{l}")}}})),
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
pub fn agent_plugins_mcp_entry(d: &McpDescriptor) -> Option<serde_json::Value> {
    let s = &d.server;
    // The spec expands placeholders only in args, env values and cwd;
    // anywhere else a reference would be sent literally, and it has no auth
    // or websocket surface — decline rather than emit a broken server.
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
    // Claude's plugin placeholders have spec equivalents (§9.2): rename them,
    // so one descriptor works in both plugin families.
    let mut entry = serde_json::Value::Object(entry);
    crate::install::mcp_config::translate_env_refs(&mut entry, &|var| match var {
        "CLAUDE_PLUGIN_ROOT" => "${PLUGIN_ROOT}".to_string(),
        "CLAUDE_PLUGIN_DATA" => "${PLUGIN_DATA}".to_string(),
        other => format!("${{{other}}}"),
    });
    // Refinement fields (timeout, always_load, headers_helper) have no Agent
    // Plugins key and are dropped, as every vendor projection does; the
    // per-client warning lives in `stage::mcp_value`.
    Some(entry)
}

/// The environment variables an Agent Plugins entry for `d` would still
/// reference after the placeholder rename — every `${VAR}` in args, env
/// values and cwd that is not a plugin placeholder. Sorted, deduplicated.
pub(crate) fn unexpanded_env_refs(d: &McpDescriptor) -> Vec<&str> {
    const PLACEHOLDERS: [&str; 4] = ["PLUGIN_ROOT", "PLUGIN_DATA", "CLAUDE_PLUGIN_ROOT", "CLAUDE_PLUGIN_DATA"];
    let s = &d.server;
    let names: std::collections::BTreeSet<&str> = s
        .args
        .iter()
        .chain(s.env.values())
        .chain(s.cwd.iter())
        .flat_map(|v| crate::oci::mcp::env_ref_names(v))
        .filter(|n| !PLACEHOLDERS.contains(n))
        .collect();
    names.into_iter().collect()
}

#[cfg(test)]
mod tests {
    //! Specification tests written from the design record (C-015, C-016,
    //! C-023, C-024, C-025, C-036; S-031 unit half), not from the
    //! implementation.

    use serde_json::json;

    use super::*;

    // ── C-015 family map ──

    #[test]
    fn c015_family_map_is_total_over_every_client() {
        use ClientTarget::*;
        for client in ClientTarget::ALL {
            let expected = match client {
                Claude | Droid | Junie | OpenClaw | Qoder => Some(Family::Claude),
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
            (Family::Claude, ClientTarget::Droid, ok, ok, ok, Err(Nfs)),
            (Family::Claude, ClientTarget::Junie, ok, ok, ok, Err(Nfs)),
            (Family::Claude, ClientTarget::OpenClaw, ok, Err(Cd), Err(Cd), Err(Nfs)),
            (Family::Claude, ClientTarget::Qoder, ok, ok, ok, Err(Nfs)),
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

    // ── C-002 version ──

    fn entry(name: &str, exec: bool, body: &str) -> InventoryEntry {
        InventoryEntry {
            name: name.to_string(),
            exec,
            sha256: Algorithm::Sha256.hash(body).hex().to_string(),
        }
    }

    /// The golden inventory: `alpha\n` / `beta\n` file contents.
    fn golden() -> Vec<InventoryEntry> {
        vec![entry("a.md", false, "alpha\n"), entry("scripts/run.sh", true, "beta\n")]
    }

    /// Independently computed (Python `hashlib` over
    /// `json.dumps(…, separators=(",", ":"))` of the same two triples); the
    /// pytest `_suffix` self-test asserts the same literal.
    const GOLDEN_SUFFIX: &str = "338bda8ec321";

    #[test]
    fn c002_golden_vector() {
        assert_eq!(plugin_version("1.2.0", &golden()), format!("1.2.0+{GOLDEN_SUFFIX}"));
    }

    #[test]
    fn c002_grammar_is_base_plus_12_lowercase_hex() {
        for base in ["0.0.0", "2.0.0-rc.1"] {
            let v = plugin_version(base, &golden());
            let (b, suffix) = v.split_once('+').unwrap();
            assert_eq!(b, base);
            assert_eq!(suffix.len(), 12);
            assert!(suffix.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')), "{v}");
        }
        assert!(plugin_version("0.0.0", &[]).starts_with("0.0.0+"));
    }

    #[test]
    fn c002_same_inventory_same_version_and_any_change_differs() {
        let v = plugin_version("1.0.0", &golden());
        assert_eq!(v, plugin_version("1.0.0", &golden()));
        let mut byte = golden();
        byte[0] = entry("a.md", false, "alpha!\n");
        let mut name = golden();
        name[0] = entry("b.md", false, "alpha\n");
        let mut exec = golden();
        exec[0].exec = true;
        for changed in [byte, name, exec] {
            assert_ne!(v, plugin_version("1.0.0", &changed));
        }
    }

    /// Codex-B2: a text framing would let a file named after another entry's
    /// line forge its neighbour; the JSON framing cannot.
    #[test]
    fn c002_counterexample_two_files_differ_from_one_forged_name() {
        let a = entry("a", false, "x");
        let b = entry("b", false, "y");
        let forged = entry(&format!("a\n{}  b", a.sha256), false, "y");
        assert_ne!(plugin_version("0.0.0", &[a, b]), plugin_version("0.0.0", &[forged]),);
    }

    // ── C-005 Qoder ──

    #[test]
    fn c005_manifest_rel_per_client() {
        for client in ClientTarget::ALL {
            let expected = match (client, family_of(client)) {
                (ClientTarget::Qoder, _) => ".qoder-plugin/plugin.json",
                (_, Some(Family::Claude)) => ".claude-plugin/plugin.json",
                (_, Some(Family::AgentPlugins)) => "plugin.json",
                (_, None) => continue,
            };
            assert_eq!(manifest_rel(client), expected, "{client}");
        }
    }

    #[test]
    fn c005_qoder_admits_skills_agents_and_mcp_but_no_rules() {
        use ArtifactKind::{Agent, Mcp, Rule, Skill};
        for kind in [Skill, Agent, Mcp] {
            assert_eq!(admits(Family::Claude, ClientTarget::Qoder, kind), Ok(()), "{kind}");
        }
        assert_eq!(
            admits(Family::Claude, ClientTarget::Qoder, Rule),
            Err(OmitReason::NoFormatSurface)
        );
    }

    // ── C-024 description and README ──

    #[test]
    fn c024_onramp_exact_bytes() {
        assert_eq!(
            ONRAMP,
            "Packaged by grim (https://grimoire.rs); install grim for pinned, updatable installs."
        );
    }

    #[test]
    fn c024_description_is_the_trimmed_base_alone() {
        assert_eq!(
            plugin_description(Some("  Team tools\n")),
            ("Team tools".to_string(), false)
        );
    }

    #[test]
    fn c024_no_or_blank_base_falls_back_to_onramp() {
        for base in [None, Some(""), Some("   "), Some("\n\t ")] {
            assert_eq!(plugin_description(base), (ONRAMP.to_string(), false), "{base:?}");
        }
    }

    #[test]
    fn c024_base_at_the_cap_is_uncut() {
        let base = "b".repeat(MAX_DESCRIPTION_LEN);
        assert_eq!(plugin_description(Some(&base)), (base, false));
    }

    #[test]
    fn c024_long_base_is_cut_at_a_word_boundary() {
        let base = format!("{}per-language migration order.", "word ".repeat(96));
        let (desc, cut) = plugin_description(Some(&base));
        assert!(cut);
        assert!(description_len(&desc) <= MAX_DESCRIPTION_LEN);
        assert!(desc.ends_with("word per-language…"), "{desc}");
        let kept = desc.trim_end_matches('…');
        assert!(base.starts_with(kept), "a prefix of the base: {desc}");
        assert!(
            base[kept.len()..].starts_with(char::is_whitespace),
            "cut at whitespace: {desc}"
        );
    }

    #[test]
    fn c024_cut_drops_trailing_separators() {
        let base = format!("{}, and more", "x".repeat(MAX_DESCRIPTION_LEN - 3));
        let (desc, _) = plugin_description(Some(&base));
        assert_eq!(desc, format!("{}…", "x".repeat(MAX_DESCRIPTION_LEN - 3)));
    }

    #[test]
    fn c024_single_overlong_word_is_cut_mid_word() {
        let (desc, cut) = plugin_description(Some(&"y".repeat(600)));
        assert!(cut);
        assert_eq!(desc, format!("{}…", "y".repeat(MAX_DESCRIPTION_LEN - 1)));
    }

    #[test]
    fn c024_cut_counts_utf16_units_and_never_splits_a_char() {
        // 🦀 is two UTF-16 units: the budget is spent in units, not chars.
        let (desc, cut) = plugin_description(Some(&"🦀 ".repeat(200)));
        assert!(cut);
        assert!(description_len(&desc) <= MAX_DESCRIPTION_LEN);
        assert!(desc.ends_with("🦀…"), "{desc}");
    }

    #[test]
    fn c024_cut_prefers_the_last_whole_sentence() {
        let base = "Short one. ".repeat(60);
        let (desc, cut) = plugin_description(Some(&base));
        assert!(cut);
        assert!(desc.ends_with("Short one."), "{desc}");
        assert!(description_len(&desc) <= MAX_DESCRIPTION_LEN);
        assert!(description_len(&desc) * 2 >= MAX_DESCRIPTION_LEN);
    }

    #[test]
    fn c024_early_sentence_end_is_not_worth_the_loss() {
        let base = format!("Intro. {}", "word ".repeat(150));
        let (desc, _) = plugin_description(Some(&base));
        assert!(desc.ends_with("word…"), "{desc}");
    }

    #[test]
    fn c024_readme_shows_the_logo_under_the_title() {
        let readme = plugin_readme(
            "team",
            ClientTarget::Claude,
            Some("Tools"),
            &[],
            Some("assets/logo.svg"),
        );
        assert_eq!(
            String::from_utf8(readme).unwrap(),
            format!("# team\n\n![team](assets/logo.svg)\n\nTools\n\n{ONRAMP}\n")
        );
    }

    #[test]
    fn c025_agent_plugins_logo_goes_under_the_codex_namespace() {
        let got = agent_plugins_plugin_json("team", "1.0.0+c5a5324c93a6", "Team tools", Some("assets/logo.png"));
        let v: serde_json::Value = serde_json::from_slice(&got).unwrap();
        assert_eq!(
            top_level_keys(&got),
            ["$schema", "name", "version", "description", "extensions"]
        );
        assert_eq!(v["extensions"]["com.openai"]["interface"]["logo"], "./assets/logo.png");
    }

    #[test]
    fn c024_readme_carries_base_omissions_and_onramp() {
        let omitted = [
            (ArtifactKind::Rule, "style".to_string()),
            (ArtifactKind::Agent, "reviewer".to_string()),
        ];
        let readme = plugin_readme("team", ClientTarget::Codex, Some(" Team tools "), &omitted, None);
        assert_eq!(
            String::from_utf8(readme).unwrap(),
            format!("# team\n\nTeam tools\n\nOmitted for codex: rule style, agent reviewer.\n\n{ONRAMP}\n")
        );
        let bare = plugin_readme("team", ClientTarget::Claude, None, &[], None);
        assert_eq!(String::from_utf8(bare).unwrap(), format!("# team\n\n{ONRAMP}\n"));
    }

    /// C-004: literal bytes, so a wording change is a deliberate edit here.
    #[test]
    fn c004_readme_and_onramp_golden_bytes() {
        let onramp = "Packaged by grim (https://grimoire.rs); install grim for pinned, updatable installs.";
        assert_eq!(ONRAMP, onramp);
        let omitted = [(ArtifactKind::Rule, "style".to_string())];
        let cases: [(Vec<u8>, String); 4] = [
            (
                plugin_readme("team", ClientTarget::Claude, None, &[], None),
                format!("# team\n\n{onramp}\n"),
            ),
            (
                plugin_readme(
                    "team",
                    ClientTarget::Claude,
                    Some("Tools"),
                    &[],
                    Some("assets/logo.png"),
                ),
                format!("# team\n\n![team](assets/logo.png)\n\nTools\n\n{onramp}\n"),
            ),
            (
                plugin_readme("team", ClientTarget::Codex, Some("Tools"), &omitted, None),
                format!("# team\n\nTools\n\nOmitted for codex: rule style.\n\n{onramp}\n"),
            ),
            (
                plugin_readme("team", ClientTarget::Qoder, None, &omitted, Some("assets/logo.svg")),
                format!("# team\n\n![team](assets/logo.svg)\n\nOmitted for qoder: rule style.\n\n{onramp}\n"),
            ),
        ];
        for (got, want) in cases {
            assert_eq!(String::from_utf8(got).unwrap(), want);
        }
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
        let got = agent_plugins_plugin_json("team", "1.0.0+c5a5324c93a6", "Team tools", None);
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
        let agent = agent_plugins_plugin_json("team", "0.0.0+c5a5324c93a6", desc, None);
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
            agent_plugins_mcp_entry(&d),
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
            agent_plugins_mcp_entry(&d),
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
    fn claude_plugin_placeholders_become_the_spec_placeholders() {
        let d = desc(
            "transport = \"stdio\"\ncommand = \"node\"\nargs = [\"${CLAUDE_PLUGIN_ROOT}/srv.js\"]\n\
             env = { CACHE = \"${CLAUDE_PLUGIN_DATA}/c\" }\ncwd = \"${CLAUDE_PLUGIN_ROOT}\"",
        );
        assert_eq!(
            agent_plugins_mcp_entry(&d),
            Some(json!({
                "type": "stdio",
                "command": "node",
                "args": ["${PLUGIN_ROOT}/srv.js"],
                "env": {"CACHE": "${PLUGIN_DATA}/c"},
                "cwd": "${PLUGIN_ROOT}",
            }))
        );
    }

    #[test]
    fn environment_refs_are_reported_but_still_shipped() {
        let d = desc(
            "transport = \"stdio\"\ncommand = \"grim\"\nargs = [\"${CLAUDE_PLUGIN_ROOT}\", \"--dsn\", \"${DB_DSN}\"]\n\
             env = { TOKEN = \"${TOKEN}\" }\ncwd = \"${PLUGIN_DATA}\"",
        );
        assert_eq!(unexpanded_env_refs(&d), vec!["DB_DSN", "TOKEN"]);
        assert!(agent_plugins_mcp_entry(&d).is_some(), "still shipped");
        let plain = desc("transport = \"stdio\"\ncommand = \"grim\"\nargs = [\"${PLUGIN_ROOT}\"]");
        assert!(unexpanded_env_refs(&plain).is_empty());
    }

    #[test]
    fn c036_http_is_streamable_http() {
        let d = desc("transport = \"http\"\nurl = \"https://x/mcp\"\nheaders = { \"X-Team\" = \"core\" }");
        assert_eq!(
            agent_plugins_mcp_entry(&d),
            Some(json!({"type": "streamable-http", "url": "https://x/mcp", "headers": {"X-Team": "core"}}))
        );
        let bare = desc("transport = \"http\"\nurl = \"https://x/mcp\"");
        assert_eq!(
            agent_plugins_mcp_entry(&bare),
            Some(json!({"type": "streamable-http", "url": "https://x/mcp"})),
            "empty headers omitted"
        );
    }

    #[test]
    fn c036_sse() {
        let d = desc("transport = \"sse\"\nurl = \"https://x/sse\"");
        assert_eq!(
            agent_plugins_mcp_entry(&d),
            Some(json!({"type": "sse", "url": "https://x/sse"}))
        );
        let h = desc("transport = \"sse\"\nurl = \"https://x/sse\"\nheaders = { A = \"b\" }");
        assert_eq!(
            agent_plugins_mcp_entry(&h),
            Some(json!({"type": "sse", "url": "https://x/sse", "headers": {"A": "b"}}))
        );
    }

    #[test]
    fn c036_declines_ws_and_oauth() {
        let ws = desc("transport = \"ws\"\nurl = \"wss://x/socket\"");
        assert_eq!(agent_plugins_mcp_entry(&ws), None);
        let oauth = desc("transport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nclient_id = \"c\"");
        assert_eq!(agent_plugins_mcp_entry(&oauth), None);
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
            assert_eq!(agent_plugins_mcp_entry(&d), None, "`${{` in {what}");
        }
    }

    #[test]
    fn c036_drops_refinement_fields() {
        let stdio = desc("transport = \"stdio\"\ncommand = \"grim\"\ntimeout = 7000\nalways_load = true");
        assert_eq!(
            agent_plugins_mcp_entry(&stdio),
            Some(json!({"type": "stdio", "command": "grim"}))
        );
        let http = desc("transport = \"http\"\nurl = \"https://x/mcp\"\ntimeout = 7000\nheaders_helper = \"./h.sh\"");
        assert_eq!(
            agent_plugins_mcp_entry(&http),
            Some(json!({"type": "streamable-http", "url": "https://x/mcp"}))
        );
    }

    #[test]
    fn c036_byte_identical_across_calls() {
        let d =
            desc("transport = \"stdio\"\ncommand = \"grim\"\nargs = [\"a\", \"b\"]\nenv = { Z = \"1\", A = \"2\" }");
        let a = serde_json::to_vec(&agent_plugins_mcp_entry(&d).unwrap()).unwrap();
        let b = serde_json::to_vec(&agent_plugins_mcp_entry(&d).unwrap()).unwrap();
        assert_eq!(a, b);
    }
}
