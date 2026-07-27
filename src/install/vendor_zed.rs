// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Zed's vendor strategy: shared-pool skills + MCP; rules and agents declined.
//!
//! Zed mapping (`adr_vendor_wave_expansion.md`; live-verified 2026-07-19,
//! `research_vendor_verification_zed_amp.md`):
//!
//! - **Skills**: the shared `.agents/skills` pool (project
//!   `<ws>/.agents/skills`, global `$HOME/.agents/skills`) — already written
//!   for Codex; flat layout only. No Zed-native skills dir.
//! - **Rules**: **declined**. No scoping anywhere; instruction files follow a
//!   9-name first-match precedence (`.rules` first, AGENTS.md 7th) — wave-2
//!   injection must handle shadowing.
//! - **Agents**: **declined**. External agents via ACP, no file format.
//! - **MCP**: `.zed/settings.json` (project) / `~/.config/zed/settings.json`
//!   (global, JSONC), key `context_servers`, **flat entry shape**; **no
//!   env-ref support upstream → skip ref-bearing descriptors**; `json_splice`.
//!
//! No config-dir env override upstream. Global settings honor
//! `$XDG_CONFIG_HOME` on **Linux/FreeBSD only** — upstream `config_dir()`
//! (`crates/paths/src/paths.rs`) reads it on that branch alone; macOS falls
//! through to a hardcoded `~/.config/zed` and Windows uses `%APPDATA%\Zed`.
//! See [`zed_root`].

use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::render::{self, RenderError, RenderedDoc};
use super::vendor::{KindSupport, Vendor, global_skills_root, home_dir, xdg_config_dir};

/// Zed.
pub struct ZedVendor;

impl Vendor for ZedVendor {
    fn name(&self) -> &'static str {
        "zed"
    }

    fn root_dir(&self) -> &'static str {
        ".zed"
    }

    fn kind_support(&self, kind: ArtifactKind) -> KindSupport {
        // Rules declined (no scoping); agents declined (ACP-only, no file format).
        match kind {
            ArtifactKind::Rule | ArtifactKind::Agent => KindSupport::Declined,
            _ => KindSupport::Native,
        }
    }

    fn detect(&self, workspace: &Path, scope: ConfigScope) -> bool {
        match scope {
            ConfigScope::Project => workspace.join(".zed").exists(),
            ConfigScope::Global => zed_root(xdg_config_dir()).is_some_and(|p| p.exists()),
        }
    }

    fn skills_root(&self, workspace: &Path, scope: ConfigScope) -> PathBuf {
        match scope {
            ConfigScope::Project => workspace.join(".agents").join("skills"),
            ConfigScope::Global => {
                global_skills_root(home_dir()).unwrap_or_else(|| workspace.join(".agents").join("skills"))
            }
        }
    }

    fn rule_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        // Dead path: `kind_support` declines `Rule`. Defensive location.
        zed_scope_root(workspace, scope)
            .join("rules")
            .join(format!("{name}.md"))
    }

    fn agent_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf {
        // Dead path: `kind_support` declines `Agent`. Defensive location.
        zed_scope_root(workspace, scope)
            .join("agents")
            .join(format!("{name}.md"))
    }

    fn mcp_config_path(&self, workspace: &Path, scope: ConfigScope) -> Option<PathBuf> {
        Some(zed_scope_root(workspace, scope).join("settings.json"))
    }

    fn mcp_entry(
        &self,
        scope: ConfigScope,
        name: &str,
        descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        use crate::oci::mcp::McpTransport;

        // Zed's `context_servers` schema has no OAuth surface — a structured
        // oauth block is auth-critical, so the whole descriptor is skipped
        // with a warning rather than written lossy.
        let s = &descriptor.server;
        if s.oauth.is_some() {
            tracing::warn!("mcp server '{name}' skipped for zed ({scope}): no oauth surface in context_servers");
            return None;
        }
        // Zed performs no env-var expansion in settings.json (open upstream
        // discussions #26043/#18630/#56881/#53780) — a descriptor that needs
        // `${VAR}` is skipped rather than writing a broken literal or a
        // secret value to disk.
        if descriptor.has_env_refs() {
            tracing::warn!(
                "mcp server '{name}' skipped for zed ({scope}): context_servers supports no ${{VAR}} substitution \
                 and grim never inlines secret values"
            );
            return None;
        }

        // Flat entry shape: stdio → top-level command/args/env; remote →
        // url/headers (the nested `command:{path,...}` shape is
        // stale-blog-only). Refinement fields have no documented
        // context_servers target — dropped.
        let mut entry = serde_json::Map::new();
        match s.transport {
            McpTransport::Stdio => {
                entry.insert("command".into(), serde_json::json!(s.command));
                if !s.args.is_empty() {
                    entry.insert("args".into(), serde_json::json!(s.args));
                }
                if !s.env.is_empty() {
                    entry.insert("env".into(), serde_json::json!(s.env));
                }
            }
            // WebSocket has no context_servers schema mapping — skip with a
            // warning.
            McpTransport::Ws => {
                tracing::warn!("mcp server '{name}' skipped for zed ({scope}): no ws transport in context_servers");
                return None;
            }
            McpTransport::Http | McpTransport::Sse => {
                entry.insert("url".into(), serde_json::json!(s.url));
                if !s.headers.is_empty() {
                    entry.insert("headers".into(), serde_json::json!(s.headers));
                }
            }
        }
        Some((format!("/context_servers/{name}"), serde_json::Value::Object(entry)))
    }

    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Vendor-aware renderer against an EMPTY registry: byte-identical to
        // the vendor-less universal render, so the shared `.agents/skills` file
        // stays vendor-independent (pinned by
        // `pool_vendors_render_byte_identical_skill_bytes`, which compares
        // `.document`). What the universal renderer cannot do is warn: it
        // returns `warnings: Vec::new()` unconditionally, so an unknown `zed.*`
        // key would vanish with no diagnostic. Warnings never reach disk.
        render::render_skill_doc(doc, self)
    }

    fn rule_index(
        &self,
        _parsed: &ParsedRule,
        _scope: ConfigScope,
        _pinned: &str,
    ) -> Result<Option<RenderedDoc>, RenderError> {
        // Never called: rules are skipped at the `kind_support` gate.
        Ok(None)
    }

    fn agent_index(&self, _parsed: &ParsedAgent, _pinned: &str) -> Result<Option<RenderedDoc>, RenderError> {
        // Never called: agents are skipped at the `kind_support` gate.
        Ok(None)
    }
}

/// Zed's config root for a scope (hosts `settings.json`): the project `.zed`
/// dir, or the platform-native global root from [`zed_root`] — which honors
/// `$XDG_CONFIG_HOME` on Linux/FreeBSD only, never on macOS. Falls back to the
/// workspace layout when that root does not resolve. Skills do NOT root here —
/// they follow the shared `.agents/skills`.
fn zed_scope_root(workspace: &Path, scope: ConfigScope) -> PathBuf {
    match scope {
        ConfigScope::Project => workspace.join(".zed"),
        ConfigScope::Global => zed_root(xdg_config_dir()).unwrap_or_else(|| workspace.join(".zed")),
    }
}

/// Which directory family Zed derives its user-level config root from — the
/// three cases upstream's `config_dir()` actually has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ZedRootKind {
    /// Windows: `%APPDATA%\Zed`.
    Appdata,
    /// macOS: `~/.config/zed`, hardcoded — `$XDG_CONFIG_HOME` is never read.
    HomeDotConfig,
    /// Linux / FreeBSD: `$XDG_CONFIG_HOME|~/.config` + `zed`.
    Xdg,
}

/// This target's [`ZedRootKind`]. `cfg!` rather than `#[cfg]` so every arm
/// type-checks on every host — the macOS branch cannot rot unnoticed on a
/// Linux dev box.
fn zed_root_kind() -> ZedRootKind {
    if cfg!(windows) {
        ZedRootKind::Appdata
    } else if cfg!(target_os = "macos") {
        ZedRootKind::HomeDotConfig
    } else {
        ZedRootKind::Xdg
    }
}

/// Zed's user-level config root — **platform-divergent, verified against
/// upstream source** (`zed-industries/zed`, `crates/paths/src/paths.rs`,
/// `config_dir()`):
///
/// - Windows → `%APPDATA%\Zed`;
/// - **macOS → `~/.config/zed`, hardcoded.** Upstream reads an XDG variable
///   only on the `cfg!(any(target_os = "linux", target_os = "freebsd"))`
///   branch; macOS falls through to a literal `home_dir().join(".config")`
///   join. A macOS user with `$XDG_CONFIG_HOME` set therefore gets Zed
///   settings written where Zed does not read them if we honor it;
/// - Linux / FreeBSD → `$XDG_CONFIG_HOME|~/.config` + `zed`.
///
/// This is why the root does **not** simply consume the shared
/// [`xdg_config_dir`](super::vendor::xdg_config_dir), which honors the
/// variable on every non-Windows target.
///
/// **The branch is Zed-local by ruling, not by discovery** (wave-0 V3,
/// 2026-07-26). Read that precisely: it does *not* mean Amp was verified to
/// behave differently. Whether Amp reads `$XDG_CONFIG_HOME` on any platform is
/// unresolved at every evidence tier, and source-tier verification is
/// *unachievable* — Amp ships as a compiled binary with no public repo. The
/// shared helper is left alone because changing it would move Amp's macOS
/// resolution on **zero** evidence; both readings of the Amp evidence agree
/// that is the wrong move. If Amp's behaviour is ever established, revisit
/// whether this belongs in the shared helper after all.
///
/// No config-dir env override upstream. The [`PathAnchor`](super::path_anchor)
/// `ZedRoot` anchor is rooted here. Skills follow the shared
/// `$HOME/.agents/skills`.
///
/// One disclosed resolvability change, macOS only: the root used to come from
/// [`xdg_config_dir`](super::vendor::xdg_config_dir), so it resolved when
/// *either* `$XDG_CONFIG_HOME` or `$HOME` did; it now resolves only when
/// `$HOME` does. With `$HOME` unset on macOS this returns `None` where it
/// previously returned `Some($XDG_CONFIG_HOME/zed)` — and `None` is the
/// correct answer, because that path is one Zed never reads. Callers degrade
/// as they already do for any unresolvable root (workspace-layout fallback, or
/// `AnchorRootAbsent`). Every platform other than macOS is unaffected.
///
/// `zed_root` is only ever called from env-touching entry points
/// (`AnchorRoots::resolve`, `detect`, `zed_scope_root`), so reading
/// `%APPDATA%` / `$HOME` here is consistent with resolving XDG for the Linux
/// arm — the pure `PathAnchor::root` lookup never calls this.
pub(crate) fn zed_root(xdg_config: Option<PathBuf>) -> Option<PathBuf> {
    zed_root_from(
        zed_root_kind(),
        xdg_config,
        home_dir(),
        super::vendor::env_dir("APPDATA"),
    )
}

/// Pure per-platform join with the platform *and* every directory input
/// injected, so all three arms are testable on any host without mutating
/// process env (`std::env::set_var` is `unsafe` under Rust 2024 and this crate
/// forbids `unsafe_code`). The env reads live only in [`zed_root`].
fn zed_root_from(
    kind: ZedRootKind,
    xdg_config: Option<PathBuf>,
    home: Option<PathBuf>,
    appdata: Option<PathBuf>,
) -> Option<PathBuf> {
    match kind {
        ZedRootKind::Appdata => appdata.map(|c| c.join("Zed")),
        ZedRootKind::HomeDotConfig => home.map(|h| h.join(".config").join("zed")),
        ZedRootKind::Xdg => xdg_config.map(|c| c.join("zed")),
    }
}

#[cfg(test)]
mod tests {
    //! Specification tests for Zed — skills + MCP only; rules and agents
    //! declined (`adr_vendor_wave_expansion.md` +
    //! `research_vendor_verification_zed_amp.md`).
    use super::*;
    use crate::oci::mcp::McpDescriptor;

    // ── kind_support: rules + agents declined ──

    #[test]
    fn kind_support_declines_rule_and_agent() {
        assert_eq!(ZedVendor.kind_support(ArtifactKind::Skill), KindSupport::Native);
        assert_eq!(ZedVendor.kind_support(ArtifactKind::Mcp), KindSupport::Native);
        assert_eq!(
            ZedVendor.kind_support(ArtifactKind::Rule),
            KindSupport::Declined,
            "no scoping anywhere"
        );
        assert_eq!(
            ZedVendor.kind_support(ArtifactKind::Agent),
            KindSupport::Declined,
            "ACP-only, no file format"
        );
    }

    // ── detect: project scope follows `.zed`; global scope follows zed_root ──

    #[test]
    fn detect_project_scope_follows_dot_zed_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path();
        assert!(!ZedVendor.detect(w, ConfigScope::Project), "absent .zed ⇒ not detected");
        std::fs::create_dir_all(w.join(".zed")).unwrap();
        assert!(ZedVendor.detect(w, ConfigScope::Project), "present .zed ⇒ detected");
    }

    #[test]
    fn detect_global_scope_existence_permutations_via_zed_root() {
        // `ZedVendor::detect`'s Global arm is exactly
        // `zed_root(xdg_config_dir()).is_some_and(|p| p.exists())`. Fabricating
        // `XDG_CONFIG_HOME`/`HOME` permutations in-process is not possible
        // here: Rust 2024 makes `std::env::set_var` `unsafe`, and this crate
        // `forbid`s `unsafe_code` crate-wide (see `vendor_codex.rs`'s
        // `detect_global_scope_existence_permutations_via_codex_root` for the
        // same precedent). This proves the piece `zed_root_is_xdg_zed_on_unix`
        // above does not: the trailing `.exists()` dir-presence check, across
        // both an XDG_CONFIG_HOME-unset (`~/.config` fallback value) and an
        // XDG_CONFIG_HOME-set (custom dir) resolved value — the most
        // error-prone root among the wave-1 vendors.
        //
        // Goes through `zed_root_from` with an explicit `Xdg` kind, not
        // `zed_root`: `zed_root` reads ambient `$HOME`/`%APPDATA%`, and on the
        // `HomeDotConfig` (macOS) arm it *discards* the injected argument, so
        // both fabricated permutations collapse onto the host's real
        // `~/.config/zed` — the second `!exists()` assertion then fails on the
        // directory the first one just created, and the test writes into the
        // real `$HOME`. Injecting the kind keeps the check hermetic and runs it
        // on every host, mirroring `vendor_amp.rs`/`vendor_codex.rs`, whose
        // roots take every input as an argument.
        let tmp = tempfile::tempdir().unwrap();
        let fallback = tmp.path().join("home").join(".config"); // unset ⇒ ~/.config fallback
        let custom = tmp.path().join("custom-xdg"); // set ⇒ a custom XDG_CONFIG_HOME
        let xdg_root = |xdg| zed_root_from(ZedRootKind::Xdg, xdg, None, None);

        // Unset: fallback root absent ⇒ not detected yet.
        let root = xdg_root(Some(fallback.clone())).unwrap();
        assert!(!root.exists(), "absent ~/.config/zed must not exist yet: {root:?}");
        std::fs::create_dir_all(&root).unwrap();
        assert!(root.exists(), "present ~/.config/zed now exists");

        // Set: a custom XDG_CONFIG_HOME root absent ⇒ not detected yet,
        // independent of the fallback resolved above.
        let overridden = xdg_root(Some(custom.clone())).unwrap();
        assert!(
            !overridden.exists(),
            "absent custom XDG_CONFIG_HOME/zed must not exist yet: {overridden:?}"
        );
        std::fs::create_dir_all(&overridden).unwrap();
        assert!(overridden.exists());

        // Neither XDG_CONFIG_HOME nor $HOME resolvable ⇒ no root at all.
        assert_eq!(xdg_root(None), None);
    }

    // ── mcp_entry: `context_servers` container, FLAT entry shape ──

    #[test]
    fn mcp_entry_stdio_is_flat_under_context_servers_pointer() {
        // Zed's key is `context_servers` (not `mcpServers`); the entry is a
        // FLAT `{command, args, env}` shape (the nested `command:{path,...}`
        // shape is stale-blog-only).
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nargs = [\"mcp\"]",
        )
        .unwrap();
        let (pointer, value) = ZedVendor
            .mcp_entry(ConfigScope::Project, "grim", &d)
            .expect("stdio registers");
        assert_eq!(pointer, "/context_servers/grim");
        assert_eq!(
            value["command"], "grim",
            "flat command, not nested `command.path`: {value}"
        );
        assert_eq!(value["args"][0], "mcp");
        assert!(
            value.get("mcpServers").is_none(),
            "Zed does not use the mcpServers key: {value}"
        );
    }

    #[test]
    fn mcp_entry_skips_env_ref_bearing_descriptor() {
        // Zed has no env-ref substitution upstream → skip ref-bearing
        // descriptors rather than write a broken literal.
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\nenv = { TOKEN = \"${GITHUB_TOKEN}\" }",
        )
        .unwrap();
        assert!(
            ZedVendor.mcp_entry(ConfigScope::Project, "grim", &d).is_none(),
            "an env-ref-bearing descriptor must be skipped for Zed"
        );
    }

    #[test]
    fn mcp_entry_declines_oauth_and_ws() {
        let oauth = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"http\"\nurl = \"https://x\"\n[server.oauth]\nclient_id = \"c\"",
        )
        .unwrap();
        assert!(
            ZedVendor.mcp_entry(ConfigScope::Project, "m", &oauth).is_none(),
            "oauth skipped"
        );
        let ws =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"ws\"\nurl = \"wss://x/socket\"")
                .unwrap();
        assert!(
            ZedVendor.mcp_entry(ConfigScope::Project, "m", &ws).is_none(),
            "ws skipped"
        );
    }

    #[test]
    fn mcp_entry_drops_refinement_fields() {
        // Refinement fields have no documented `context_servers` target —
        // dropped. Mirrors vendor_copilot.rs::mcp_entry_drops_refinement_fields.
        let d = McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"\ntimeout = 7000\ncwd = \"./srv\"\nalways_load = true\n",
        )
        .unwrap();
        let (_, value) = ZedVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        for key in ["timeout", "cwd", "always_load", "alwaysLoad", "headersHelper"] {
            assert!(value.get(key).is_none(), "no Zed target for '{key}': {value}");
        }
    }

    #[test]
    fn mcp_entry_is_deterministic() {
        let d =
            McpDescriptor::from_toml_str("description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"")
                .unwrap();
        let a = ZedVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        let b = ZedVendor.mcp_entry(ConfigScope::Project, "m", &d).unwrap();
        assert_eq!(a, b, "regeneration must be byte-identical");
    }

    // ── zed_root: native settings dir per platform ──
    //
    // Upstream `config_dir()` (zed-industries/zed, crates/paths/src/paths.rs)
    // has three branches: Windows `%APPDATA%\Zed`, Linux/FreeBSD XDG, and a
    // fallthrough `home_dir().join(".config")` that macOS lands on. Every arm
    // is exercised here on every host by injecting the platform, so the macOS
    // branch cannot rot unnoticed on a Linux or Windows dev box.

    #[test]
    fn zed_root_macos_ignores_xdg_config_home() {
        // Regression: grim resolved Zed's root through the shared
        // `xdg_config_dir()`, which honors `$XDG_CONFIG_HOME` on every
        // non-Windows target. Zed's macOS branch never reads it, so a macOS
        // user with the variable set had `settings.json` written where Zed
        // does not look.
        let xdg = Some(PathBuf::from("/custom/xdg"));
        let home = Some(PathBuf::from("/Users/u"));
        assert_eq!(
            zed_root_from(ZedRootKind::HomeDotConfig, xdg.clone(), home.clone(), None),
            Some(PathBuf::from("/Users/u/.config/zed")),
            "macOS hardcodes ~/.config/zed and must ignore $XDG_CONFIG_HOME"
        );
        // Linux/FreeBSD is the arm that DOES honor it — same inputs, different
        // answer, which is the whole point of the split.
        assert_eq!(
            zed_root_from(ZedRootKind::Xdg, xdg, home, None),
            Some(PathBuf::from("/custom/xdg/zed")),
            "Linux/FreeBSD honors $XDG_CONFIG_HOME"
        );
    }

    #[test]
    fn zed_root_from_covers_every_platform_arm_and_unresolvable_inputs() {
        let xdg = Some(PathBuf::from("/home/u/.config"));
        let home = Some(PathBuf::from("/home/u"));
        // Separator-neutral stand-in for `%APPDATA%`: a literal `C:\…` fixture
        // is one opaque component on Unix, so the join would not round-trip.
        let appdata = Some(PathBuf::from("/appdata"));

        assert_eq!(
            zed_root_from(ZedRootKind::Appdata, xdg.clone(), home.clone(), appdata.clone()),
            Some(PathBuf::from("/appdata").join("Zed")),
            "Windows uses %APPDATA%\\Zed (capital Z) and ignores both unix inputs"
        );
        assert_eq!(
            zed_root_from(ZedRootKind::Xdg, xdg, home.clone(), appdata.clone()),
            Some(PathBuf::from("/home/u/.config/zed"))
        );

        // Each arm yields None only when ITS own input is unresolvable.
        assert_eq!(zed_root_from(ZedRootKind::Appdata, None, home, None), None);
        assert_eq!(
            zed_root_from(ZedRootKind::HomeDotConfig, None, None, appdata.clone()),
            None
        );
        assert_eq!(zed_root_from(ZedRootKind::Xdg, None, None, appdata), None);
    }

    // The two tests above pin what each ARM computes. These pin the WIRING —
    // that `zed_root` hands this host's arm the right inputs. Without them,
    // reverting `zed_root` to feed `ZedRootKind::Xdg` on macOS (the original
    // bug) would leave every other test in this file green.

    #[cfg(target_os = "macos")]
    #[test]
    fn zed_root_ignores_the_xdg_argument_on_this_host() {
        let bogus = Some(PathBuf::from("/definitely/not/home/.config"));
        assert_eq!(
            zed_root(bogus),
            home_dir().map(|h| h.join(".config").join("zed")),
            "macOS must resolve from $HOME and discard the XDG argument entirely"
        );
    }

    #[cfg(all(not(windows), not(target_os = "macos")))]
    #[test]
    fn zed_root_honors_the_xdg_argument_on_this_host() {
        assert_eq!(
            zed_root(Some(PathBuf::from("/custom/xdg"))),
            Some(PathBuf::from("/custom/xdg/zed")),
            "Linux/FreeBSD must resolve from the XDG argument"
        );
        assert_eq!(zed_root(None), None, "no resolvable XDG dir ⇒ no root");
    }
}
