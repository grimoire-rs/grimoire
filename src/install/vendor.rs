// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The per-vendor materialization strategy seam.
//!
//! [`Vendor`] is the interface every supported AI client implements: it
//! owns the client's on-disk layout (project **and** global/native
//! user-level discovery paths), its known-field registries (the **only**
//! place vendor field knowledge lives), its index transforms, and its
//! config side-effects. [`super::client_target::ClientTarget`] stays the
//! closed identity enum (parse/display); behavior dispatches through the
//! vendor structs in `vendor_claude` / `vendor_opencode` /
//! `vendor_copilot`. Adding a client = one new struct + one enum arm.
//!
//! Design principle (owner decision): a capability **common to several
//! vendors** is authored once as a canonical top-level frontmatter field
//! and projected per vendor (e.g. a rule's `paths` → Claude `paths:`,
//! Copilot `applyTo:`); a capability **unique to one vendor** is authored
//! as a `<vendor>.<field>` string key inside the `metadata` map.
//!
//! Scope-aware layout: project-scope installs land under
//! `<workspace>/<root_dir>/…`; global-scope installs land in the vendor's
//! **native** user-level discovery directory (`~/.claude`,
//! `~/.config/opencode/skills`, `~/.copilot/skills`) so the tool actually
//! loads them — falling back to the workspace layout when the native
//! location cannot be resolved (no `$HOME`) or does not exist for the
//! artifact kind.

use std::io;
use std::path::{Path, PathBuf};

use crate::config::scope::ConfigScope;
use crate::oci::ArtifactKind;
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::install_state::InstallState;
use super::render::{RenderError, RenderedDoc};

/// The native YAML type a known namespaced field converts to.
#[derive(Debug, Clone, Copy)]
pub enum FieldType {
    /// `"true"` / `"false"` → native YAML bool; anything else errors.
    Bool,
    /// Passthrough string.
    String,
    /// Passthrough string validated against a closed set of literals.
    Enum(&'static [&'static str]),
    /// Base-10 integer literal → native YAML number; anything else errors.
    Integer,
    /// Finite float literal → native YAML number; anything else errors.
    Float,
    /// Comma-separated string → native YAML sequence (segments trimmed,
    /// empties dropped, input order kept). Never fails.
    CommaList,
}

/// How faithfully a vendor can host an [`ArtifactKind`].
///
/// Tri-state successor to the old `supports_kind` bool
/// (`adr_vendor_wave_expansion.md` §2 — the rule-classification principle):
///
/// - [`Native`](KindSupport::Native): a per-file surface that expresses the
///   kind faithfully (Claude/Copilot/Cursor/Kiro rules, agent frontmatter).
/// - [`Degraded`](KindSupport::Degraded): a grim-ownable per-file surface
///   exists but cannot express the kind's scoping — installed with the lossy
///   field dropped **and a warning** (OpenCode rules: `paths:` dropped).
/// - [`Declined`](KindSupport::Declined): no grim-ownable surface at all —
///   warn + skip + zero outputs (Codex rules, and the wave-1 declines).
///
/// Behavior mapping onto the old bool: `Declined` is the old `false`;
/// `Native` and `Degraded` are both the old `true`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindSupport {
    /// Faithful native surface — the kind installs with full fidelity.
    Native,
    /// Ownable surface, reduced fidelity — installs with a warning.
    Degraded,
    /// No ownable surface — warn + skip + zero outputs.
    Declined,
}

/// Which splice engine renders a vendor's [`Vendor::mcp_config_path`] file.
/// Every vendor but Codex writes a JSON/JSONC config, edited via
/// [`super::json_splice`]; Codex's `config.toml` is the first
/// TOML-formatted MCP config, edited via [`super::toml_splice`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum McpConfigFormat {
    /// JSON/JSONC — spliced via [`super::json_splice`].
    #[default]
    Json,
    /// TOML — spliced via [`super::toml_splice`].
    Toml,
}

/// One row of a vendor registry: the namespaced field name (the part
/// after `<vendor>.`), the native frontmatter key it lifts to, and its
/// native type.
pub struct KnownField {
    /// The metadata key suffix (`user-invocable` in `claude.user-invocable`).
    pub field: &'static str,
    /// The native frontmatter key the value is emitted under.
    pub native: &'static str,
    /// The native value type (drives conversion + validation).
    pub ty: FieldType,
}

/// A supported AI client's materialization strategy.
pub trait Vendor {
    /// The vendor name — the `metadata` namespace prefix and the
    /// `--client` identifier (`claude`, `opencode`, `copilot`, `codex`).
    fn name(&self) -> &'static str;

    /// The client root directory under a project workspace (`.claude`, …).
    fn root_dir(&self) -> &'static str;

    /// How this vendor hosts `kind` — the tri-state gate that replaced the
    /// old `supports_kind` bool. Default [`KindSupport::Native`]; a vendor
    /// overrides to declare a [`KindSupport::Degraded`] surface (installs
    /// with a fidelity-loss warning) or a [`KindSupport::Declined`] one (the
    /// installer warns + skips, records no output). Codex declines
    /// [`ArtifactKind::Rule`] — it has no faithful path-scoped instruction
    /// mechanism; OpenCode degrades it — a per-file surface without scoping.
    fn kind_support(&self, _kind: ArtifactKind) -> KindSupport {
        KindSupport::Native
    }

    /// Known `<vendor>.*` skill metadata fields lifted into native
    /// `SKILL.md` frontmatter. Empty ⇒ the vendor reads only universal
    /// agentskills fields (any own-namespace key is a typo: warn + drop).
    fn skill_fields(&self) -> &'static [KnownField] {
        &[]
    }

    /// Known `<vendor>.*` rule metadata fields. Same semantics as
    /// [`Self::skill_fields`], for rule frontmatter `metadata`.
    fn rule_fields(&self) -> &'static [KnownField] {
        &[]
    }

    /// Known `<vendor>.*` agent metadata fields. Same semantics as
    /// [`Self::skill_fields`], for agent frontmatter `metadata`. A lifted
    /// key whose native name collides with a projected common field
    /// (`model`, `tools`) **overrides** it — the documented escape hatch.
    fn agent_fields(&self) -> &'static [KnownField] {
        &[]
    }

    /// Whether this client is *detected* for `scope` — its vendor
    /// directory / config marker is present — so a default install (no
    /// `--client`, no `[options].clients`) should target it. Pure existence
    /// checks; no I/O beyond `stat`.
    ///
    /// The default probes the project root dir (`<workspace>/<root_dir>`)
    /// for project scope and returns `false` for global scope. Each vendor
    /// overrides this to own its native user-level discovery knowledge for
    /// the global scope (and, for Copilot, a tighter project marker than
    /// the broadly-present `.github` dir).
    fn detect(&self, workspace: &Path, scope: ConfigScope) -> bool {
        match scope {
            ConfigScope::Project => workspace.join(self.root_dir()).exists(),
            ConfigScope::Global => false,
        }
    }

    /// The directory skill trees install under for `scope`.
    fn skills_root(&self, workspace: &Path, scope: ConfigScope) -> PathBuf;

    /// The install path of the rule index `<name>` for `scope`.
    fn rule_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf;

    /// The install path of the agent file `<name>` for `scope`. Every
    /// vendor has a native agents directory (project and user level), so
    /// there is no default — each vendor owns its layout.
    fn agent_path(&self, workspace: &Path, scope: ConfigScope, name: &str) -> PathBuf;

    /// The vendor's native MCP config file for `scope`, or `None` when the
    /// vendor has no writable MCP registration surface there (an MCP
    /// install then skips this vendor with a warning, mirroring the
    /// Copilot global-rule degradation). Default: no surface.
    fn mcp_config_path(&self, _workspace: &Path, _scope: ConfigScope) -> Option<PathBuf> {
        None
    }

    /// The config-file format [`Self::mcp_config_path`] writes, so the
    /// installer's MCP registration step picks the matching span-preserving
    /// splice engine ([`super::json_splice`] vs [`super::toml_splice`]).
    /// Default [`McpConfigFormat::Json`] — every vendor but Codex writes a
    /// JSON/JSONC config; Codex's `config.toml` is TOML.
    fn mcp_config_format(&self) -> McpConfigFormat {
        McpConfigFormat::Json
    }

    /// Render the vendor-native MCP config entry for `descriptor` as a
    /// `(pointer, value)` pair — the two-level JSON pointer of the managed
    /// member inside [`Self::mcp_config_path`]'s file (e.g.
    /// `/mcpServers/<name>`) plus the entry value in the vendor's own
    /// schema and env-reference syntax. `None` when the vendor cannot
    /// represent this descriptor at `scope` (the install skips the vendor
    /// with a warning). Default: no surface.
    fn mcp_entry(
        &self,
        _scope: ConfigScope,
        _name: &str,
        _descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        None
    }

    /// Render the `SKILL.md` index for this vendor, or `None` when the
    /// canonical bytes should install verbatim (no tool-namespaced
    /// metadata, or not parseable as a skill).
    ///
    /// # Errors
    ///
    /// [`RenderError`] when a known `<vendor>.<field>` metadata key
    /// carries an unconvertible literal.
    fn skill_index(&self, doc: &str) -> Result<Option<RenderedDoc>, RenderError>;

    /// Render the rule index document for this vendor, or `None` when the
    /// canonical bytes should install verbatim. A `Some` document is
    /// written `generated: true` (integrity-anchored on the rendered
    /// bytes) and must be deterministic.
    ///
    /// `scope` is threaded from the materialize call path so a vendor whose
    /// rule emission is *content-* rather than *kind-*dependent on the install
    /// scope can react to it — the only wave-1 reader is Kiro, whose global
    /// scoped steering is written correctly but is inert until upstream #9176
    /// closes, surfaced as a [`RenderedDoc`] warning. Every other vendor
    /// ignores it and stays byte-identical across scopes.
    ///
    /// # Errors
    ///
    /// [`RenderError`] when a known `<vendor>.<field>` metadata key
    /// carries an unconvertible literal.
    fn rule_index(
        &self,
        parsed: &ParsedRule,
        scope: ConfigScope,
        pinned: &str,
    ) -> Result<Option<RenderedDoc>, RenderError>;

    /// Render the agent document for this vendor, or `None` when the
    /// canonical bytes should install verbatim. Same `generated`/
    /// determinism contract as [`Self::rule_index`]. The projected common
    /// fields (`name`/`description`/`model`/`tools`) follow the per-vendor
    /// emit matrix; a lifted `<vendor>.*` key overrides its common field.
    ///
    /// # Errors
    ///
    /// [`RenderError`] when a known `<vendor>.<field>` metadata key
    /// carries an unconvertible literal.
    fn agent_index(&self, parsed: &ParsedAgent, pinned: &str) -> Result<Option<RenderedDoc>, RenderError>;

    /// Converge vendor-owned configuration on the current install state —
    /// the reversible config-registration seam (hooks ADR pattern).
    /// Called after install/update/uninstall mutated `state` for every
    /// involved vendor. Default: no-op.
    ///
    /// # Errors
    ///
    /// An I/O failure editing the vendor config (the operation that
    /// triggered the sync still completed; callers surface the error).
    fn sync_config(&self, _state: &InstallState, _workspace: &Path, _scope: ConfigScope) -> io::Result<()> {
        Ok(())
    }
}

/// Neutralize `pinned` — a registry ref / digest string threaded verbatim
/// into a single-line provenance comment — against two injection vectors so
/// no untrusted byte can escape the generated header:
///
/// - **control characters** (newlines included) collapse to a space, so an
///   embedded newline can never open a second line (HTML/TOML injection);
/// - **`<` / `>`** escape to `&lt;` / `&gt;`, so a literal `-->` cannot close
///   the HTML `<!-- ... -->` comment early and inject live content after it
///   (CWE-116). Harmless in the TOML `#` variant, which has no comment
///   terminator to break — the same neutralized value is used for both.
fn single_line(pinned: &str) -> std::borrow::Cow<'_, str> {
    if pinned.chars().any(|c| c.is_control() || c == '<' || c == '>') {
        let mut out = String::with_capacity(pinned.len());
        for c in pinned.chars() {
            match c {
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                c if c.is_control() => out.push(' '),
                c => out.push(c),
            }
        }
        std::borrow::Cow::Owned(out)
    } else {
        std::borrow::Cow::Borrowed(pinned)
    }
}

/// The shared provenance header generated rule transforms prepend.
pub fn provenance(pinned: &str) -> String {
    format!(
        "<!-- generated by grim from {}; edits will be overwritten -->\n",
        single_line(pinned)
    )
}

/// The provenance header generated TOML transforms prepend. TOML uses `#`
/// line comments — the HTML-comment [`provenance`] header is invalid in
/// TOML, so Codex agent files get this variant instead.
pub fn toml_provenance(pinned: &str) -> String {
    format!(
        "# generated by grim from {}; edits will be overwritten\n",
        single_line(pinned)
    )
}

/// The user's home directory: `$HOME` on Unix, `%USERPROFILE%` on Windows.
pub(crate) use crate::env::home_dir;

/// The value of `var` as a path, when set and non-empty. An empty value
/// is treated as unset, matching common env-override conventions.
pub fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// `$XDG_CONFIG_HOME`, else `$HOME/.config`, when resolvable.
pub fn xdg_config_dir() -> Option<PathBuf> {
    env_dir("XDG_CONFIG_HOME").or_else(|| home_dir().map(|h| h.join(".config")))
}

/// The cross-vendor shared skills pool `$HOME/.agents/skills` — the open
/// standard scanned by Codex, Gemini, Zed, and Amp (keyed on `$HOME` only,
/// **not** relocated by any vendor's config-dir override). The
/// [`PathAnchor`](super::path_anchor) `AgentsSkills` anchor is rooted here.
pub(crate) fn global_skills_root(home: Option<PathBuf>) -> Option<PathBuf> {
    home.map(|h| h.join(".agents").join("skills"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── C3.5: provenance single-line invariant ─────────────────────────────
    //
    // `pinned` is untrusted-ish authored content (a registry ref / digest
    // string) threaded verbatim into a provenance header. Neither
    // `provenance` nor `toml_provenance` currently guards against an
    // embedded newline, so a `pinned` value carrying one would let injected
    // text escape the single comment line — an HTML/TOML comment injection
    // into the generated file. Both builders must keep the header to
    // exactly one line (reject or escape the newline) regardless of how
    // `pinned` got that byte in it.

    #[test]
    fn provenance_pinned_with_embedded_newline_stays_single_line() {
        let pinned = "acme/x@sha256:deadbeef\nmalicious: injected";
        let out = provenance(pinned);
        assert_eq!(
            out.matches('\n').count(),
            1,
            "provenance header must stay a single line (one trailing newline only): {out:?}"
        );
        assert!(out.ends_with('\n'));
        assert!(out.starts_with("<!-- generated by grim from "));
    }

    #[test]
    fn toml_provenance_pinned_with_embedded_newline_stays_single_line() {
        let pinned = "acme/x@sha256:deadbeef\n[injected]\nkey = \"evil\"";
        let out = toml_provenance(pinned);
        assert_eq!(
            out.matches('\n').count(),
            1,
            "toml provenance header must stay a single line (one trailing newline only): {out:?}"
        );
        assert!(out.ends_with('\n'));
        assert!(out.starts_with("# generated by grim from "));
    }

    #[test]
    fn provenance_and_toml_provenance_replace_carriage_return_and_tab() {
        // `\r` and `\t` are both `char::is_control`, the same guard that
        // catches `\n` — cheap coverage for the other two ASCII control
        // characters most likely to show up in a copy-pasted ref string.
        let pinned = "acme/x@sha256:deadbeef\r\tinjected";
        let html = provenance(pinned);
        let toml = toml_provenance(pinned);
        for out in [&html, &toml] {
            assert_eq!(out.matches('\n').count(), 1, "must stay single-line: {out:?}");
            assert!(!out.contains('\r'), "carriage return must not survive: {out:?}");
            assert!(!out.contains('\t'), "tab must not survive: {out:?}");
        }
        assert!(
            html.contains("acme/x@sha256:deadbeef  injected"),
            "each control char becomes a space: {html:?}"
        );
        assert!(
            toml.contains("acme/x@sha256:deadbeef  injected"),
            "each control char becomes a space: {toml:?}"
        );
    }

    #[test]
    fn single_line_escapes_html_comment_breakout() {
        // A literal `-->` in `pinned` would close the HTML `<!-- ... -->`
        // provenance comment early, injecting live content into the generated
        // OpenCode/Copilot rule/agent file (CWE-116). Escaping `<`/`>`
        // neutralizes both the comment terminator (`-->`) and any injected tag.
        let pinned = "acme/x@sha256:d --> <script>alert(1)</script>";

        let escaped = single_line(pinned);
        assert!(!escaped.contains('<'), "raw '<' must be escaped: {escaped}");
        assert!(!escaped.contains('>'), "raw '>' must be escaped: {escaped}");
        assert!(!escaped.contains("-->"), "comment terminator neutralized: {escaped}");
        assert!(escaped.contains("&lt;script&gt;"), "escaped tag present: {escaped}");

        // In the full HTML header the only `<`/`>` left are the fixed
        // `<!--`/`-->` delimiters grim adds itself — the injected `-->` and
        // `<script>` can no longer break out of the comment.
        let out = provenance(pinned);
        assert_eq!(out.matches('\n').count(), 1, "single line: {out:?}");
        assert_eq!(out.matches('<').count(), 1, "only the opening <!-- delimiter: {out:?}");
        assert_eq!(out.matches('>').count(), 1, "only the closing --> delimiter: {out:?}");
    }

    #[test]
    fn provenance_without_embedded_newline_is_unaffected() {
        let pinned = "acme/x@sha256:deadbeef";
        assert_eq!(
            provenance(pinned),
            "<!-- generated by grim from acme/x@sha256:deadbeef; edits will be overwritten -->\n"
        );
        assert_eq!(
            toml_provenance(pinned),
            "# generated by grim from acme/x@sha256:deadbeef; edits will be overwritten\n"
        );
    }

    #[test]
    fn global_skills_root_is_home_agents_skills() {
        assert_eq!(
            global_skills_root(Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/home/u/.agents/skills"))
        );
        assert_eq!(global_skills_root(None), None);
    }
}
