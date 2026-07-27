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

/// Clients **verified** to scan the cross-vendor `$HOME/.agents/skills` pool,
/// by [`Vendor::name`]. The evidence roster behind
/// [`Vendor::pool_capable`] — absent evidence defaults to *not* capable.
///
/// - `codex`, `gemini`, `zed`, `amp`, `agents` already render there by
///   default (their [`Vendor::skills_root`] *is* the pool).
/// - `cursor`, `copilot`, `opencode` scan it **additively**, alongside their
///   own native skills dir — verified 2026-07-26. They are the members the
///   opt-in actually buys anything for.
/// - `goose` and `warp` scan it at **both** scopes, verified 2026-07-27
///   against each vendor's own docs. They differ in what grim writes by
///   default: Goose renders *into* the pool (its own `.goose/skills` is
///   labelled back-compat upstream, and `.agents/skills` the recommended
///   location), while Warp renders natively to `.warp/skills` and reaches the
///   pool only through the opt-in. Membership here is about what a client
///   **reads**, not where grim writes — those are separate questions.
/// - Absent, deliberately: `claude` (does not scan the pool), `kiro` and
///   `junie` (not evidenced either way), `cline` and `droid` (confirmed
///   *absent* from their own documented scan lists, not merely unevidenced),
///   `openclaw` (it does scan the pool at priority 3, but it is global-only
///   and the interaction between a scope-gapped client and `shared_skills` is
///   unproven — a deliberate deferral, since adding is additive and removing
///   is breaking), `kilo` (**partial**: project pool only, no global support)
///   and `antigravity` — which **does**
///   read the project pool but not the global one (its global skills live
///   under its own `~/.gemini/config/skills`). Membership here is scope-blind,
///   so adding it would make `shared_skills = true` write global skills where
///   Antigravity never scans, and nothing would fail: the anchor table
///   classifies the pooled destination happily. A partial pool member needs a
///   scope-aware predicate before it can join this roster.
///
/// A client may be **added** later — that is additive. Removing one is
/// breaking: a config that was accepted would start erroring (Principle 9).
///
/// # A vendor that declares `skill_fields` is NOT pool-capable
///
/// [`Vendor::pool_capable`] ANDs this list with an empty
/// [`Vendor::skill_fields`] registry, and that conjunct must not be weakened.
/// grim writes ONE physical pool tree that every pool member records an output
/// against, so a member emitting its own `<vendor>.*` fields breaks it two
/// ways:
///
/// - **across passes** — a client flipping into the pool lands on a directory
///   its siblings already record, and the untracked-clobber gate deliberately
///   lets it through: the gate asks whether any recorded output claims the
///   destination, and one does. That is correct — the directory is grim's —
///   but it means the materialize step rewrites it, silently invalidating
///   every sibling's stored `content_hash` the moment the bytes differ;
/// - **within one pass** — destinations are deduped *before* any render, so
///   the second pool vendor reuses the first's bytes **and** its hash. Its own
///   fields never render at all, silently, and which vendor wins depends on
///   `ClientTarget::ALL` order rather than on anything the user wrote.
///
/// Both have the same fix, and it is not a guard in the installer: keep a
/// fields-declaring vendor out of the pool.
const POOL_CAPABLE_VENDORS: &[&str] = &[
    "codex", "gemini", "zed", "amp", "agents", "cursor", "copilot", "opencode", "goose", "warp",
];

/// [`Vendor::pool_capable`] with both inputs injected.
///
/// Split out so the `skill_fields` conjunct is *decidable* by a test. No
/// shipped vendor is both on the roster and declaring skill fields — Claude is
/// the only vendor with a registry and it is deliberately off the roster — so
/// against real vendors alone the conjunct is invisible: delete it and every
/// assertion still passes. That is exactly the drift it exists to catch, so it
/// gets the injected-input seam `zed_root_from` established.
fn pool_capable_from(name: &str, declares_skill_fields: bool) -> bool {
    POOL_CAPABLE_VENDORS.contains(&name) && !declares_skill_fields
}

/// A supported AI client's materialization strategy.
pub trait Vendor {
    /// The vendor name — the `metadata` namespace prefix and the
    /// `--client` identifier (`claude`, `opencode`, `copilot`, `codex`,
    /// `cursor`, `kiro`, `junie`, `gemini`, `zed`, `amp`).
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

    /// Whether this vendor has a grim-ownable directory for `kind` at `scope`.
    ///
    /// [`Self::kind_support`] takes no scope, so it cannot express a vendor
    /// that hosts a kind at one scope and not the other. This is the
    /// scope-aware half, and it exists for exactly the same reason
    /// [`Self::mcp_config_path`]'s `Option` does — the installer's
    /// `client_supports_kind` consults both.
    ///
    /// Default `true`: almost every vendor hosts each kind it supports at both
    /// scopes. The two shipped gaps run in opposite directions, which is why
    /// this is one predicate rather than a per-kind pair:
    ///
    /// - **Junie** has `.junie/rules/` but no global `~/.junie/rules/`;
    /// - **OpenClaw** has global `~/.openclaw/skills` but no per-repository
    ///   scope at all — its "workspace" is a fixed daemon home.
    ///
    /// Returning `false` makes the installer warn, skip, and record **zero
    /// outputs** for that client at that scope — never write to a directory
    /// nothing reads, and never anchor a record at a path the anchor's own
    /// meaning does not cover.
    ///
    /// Not consulted for [`ArtifactKind::Mcp`] (that is `mcp_config_path`'s
    /// job), nor when [`Self::kind_support`] already declines the kind.
    fn kind_surface(&self, _kind: ArtifactKind, _scope: ConfigScope) -> bool {
        true
    }

    /// Known `<vendor>.*` skill metadata fields lifted into native
    /// `SKILL.md` frontmatter. Empty ⇒ the vendor reads only universal
    /// agentskills fields (any own-namespace key is a typo: warn + drop).
    fn skill_fields(&self) -> &'static [KnownField] {
        &[]
    }

    /// Whether this client actually **reads** the cross-vendor
    /// `$HOME/.agents/skills` pool, and may therefore be opted into rendering
    /// its skills there via `[options.vendors.<name>].shared_skills`.
    ///
    /// The default is derived, not overridden per vendor: membership of
    /// [`POOL_CAPABLE_VENDORS`] **and** an empty [`Self::skill_fields`]
    /// registry. The second conjunct is load-bearing, not decoration — see
    /// that constant's docs for the two failure modes it prevents. Deriving it
    /// here rather than asserting it in a test means a vendor that later
    /// declares a skill field cannot stay pool-capable by oversight.
    fn pool_capable(&self) -> bool {
        pool_capable_from(self.name(), !self.skill_fields().is_empty())
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
    fn pool_capable_roster_names_real_clients_and_matches_the_predicate() {
        use crate::install::client_target::ClientTarget;

        // Every roster entry must name a real client — a typo would silently
        // make that vendor un-opt-in-able with no failure anywhere.
        let known: Vec<&str> = ClientTarget::ALL.iter().map(|c| c.vendor().name()).collect();
        for name in POOL_CAPABLE_VENDORS {
            assert!(known.contains(name), "'{name}' is not a client name: {known:?}");
        }

        let capable: Vec<&str> = ClientTarget::ALL
            .iter()
            .filter(|c| c.vendor().pool_capable())
            .map(|c| c.vendor().name())
            .collect();
        assert_eq!(
            capable,
            vec![
                "opencode", "copilot", "codex", "cursor", "gemini", "zed", "amp", "agents", "goose", "warp"
            ],
            "the pool-capable set is an evidence roster; a client joining or leaving it is a deliberate change"
        );
        // Claude is the verified non-reader AND the only vendor declaring
        // skill fields — both reasons must independently exclude it.
        assert!(!ClientTarget::Claude.vendor().pool_capable());
        assert!(!ClientTarget::Kiro.vendor().pool_capable());
        assert!(!ClientTarget::Junie.vendor().pool_capable());
        // Confirmed absences from their own scan lists, not evidence gaps.
        assert!(!ClientTarget::Cline.vendor().pool_capable());
        assert!(!ClientTarget::Droid.vendor().pool_capable());
        // Partial members / deliberate deferrals — the shape that silently
        // writes global skills where the client never scans if let in.
        assert!(!ClientTarget::Kilo.vendor().pool_capable());
        assert!(!ClientTarget::OpenClaw.vendor().pool_capable());
    }

    #[test]
    fn a_vendor_declaring_skill_fields_is_never_pool_capable() {
        use crate::install::client_target::ClientTarget;

        // The load-bearing rule: one physical pool tree cannot host two
        // vendors that render different bytes into it.
        //
        // Asserting it over the real vendors alone would be VACUOUS — Claude
        // is the only vendor with a `skill_fields` registry and it is already
        // off the roster, so deleting the conjunct changes no answer. Inject
        // the input instead, so the conjunct is what decides.
        assert!(pool_capable_from("cursor", false), "a roster member with no fields");
        assert!(
            !pool_capable_from("cursor", true),
            "declaring skill_fields must remove a roster member from the pool"
        );
        assert!(!pool_capable_from("claude", false), "off the roster stays off it");

        // …and the invariant holds for every vendor that actually ships.
        for client in ClientTarget::ALL {
            let vendor = client.vendor();
            assert!(
                vendor.skill_fields().is_empty() || !vendor.pool_capable(),
                "'{}' declares skill_fields and must not be pool-capable",
                vendor.name()
            );
        }
        assert!(
            !ClientTarget::Claude.vendor().skill_fields().is_empty(),
            "Claude is the live example the rule is written for"
        );
    }

    /// Every `(client, kind, scope)` whose surface is absent — the complete
    /// exception set to `kind_surface`'s `true` default.
    const SCOPE_GAPS: &[(&str, ArtifactKind, ConfigScope)] = &[
        // `.junie/rules/` is ownable; no global `~/.junie/rules/` exists.
        ("junie", ArtifactKind::Rule, ConfigScope::Global),
        // OpenClaw has no per-repository scope: its "project" path is a fixed
        // daemon home that does not track the repo grim was invoked in.
        ("openclaw", ArtifactKind::Skill, ConfigScope::Project),
    ];

    #[test]
    fn kind_surface_is_true_everywhere_except_the_declared_scope_gaps() {
        use crate::install::client_target::ClientTarget;

        // `kind_surface` defaults to `true`, so it must not have narrowed any
        // shipped vendor. Asserted over the real roster at BOTH scopes and all
        // three file kinds rather than by reading the default — an accidental
        // override would silently stop a client installing a kind the compat
        // matrix says it supports, and nothing else would notice.
        for client in ClientTarget::ALL {
            for kind in [ArtifactKind::Skill, ArtifactKind::Rule, ArtifactKind::Agent] {
                for scope in [ConfigScope::Project, ConfigScope::Global] {
                    let expected = !SCOPE_GAPS
                        .iter()
                        .any(|(c, k, s)| *c == client.vendor().name() && *k == kind && *s == scope);
                    assert_eq!(
                        client.vendor().kind_surface(kind, scope),
                        expected,
                        "{client} kind_surface({kind:?}, {scope:?}) must be {expected}"
                    );
                }
            }
        }
    }

    #[test]
    fn reserving_a_namespace_never_drops_a_key_silently() {
        use crate::install::client_target::ClientTarget;

        // Adding a client reserves its `<name>.` metadata prefix, so a key that
        // was plain pass-through data under an earlier grim starts being
        // dropped. The drop is unavoidable and additive; doing it SILENTLY is
        // not — a user would read the vanished key as a bug.
        //
        // The trap is real, not theoretical: `render_universal_skill_doc`
        // returns `warnings: Vec::new()` unconditionally, so any vendor routing
        // its `skill_index` through it drops own-namespace keys with no
        // diagnostic at all. Goose was written that way first, since it renders
        // into the shared pool; `amp`, `antigravity`, `codex`, `gemini` and
        // `zed` shipped that way and were the defect this test first found.
        //
        // Derived from `ClientTarget::ALL` rather than a hand-kept list, so a
        // new vendor is covered the day it lands instead of the day someone
        // remembers to add it — a per-client message names the offender.
        // The exclusion is structural, not a backlog: a client that reserves no
        // namespace cannot drop an own-namespace key, because it has none. It is
        // read off `KNOWN_NAMESPACES` — the same list the renderer consults —
        // rather than hardcoding `ClientTarget::Agents`, so this test and the
        // reservation policy cannot drift apart by convention.
        let mut silent: Vec<&'static str> = Vec::new();
        let mut unrendered: Vec<&'static str> = Vec::new();
        for client in ClientTarget::ALL
            .iter()
            .filter(|c| crate::install::render::reserves_namespace(c.vendor().name()))
        {
            let vendor = client.vendor();
            let name = vendor.name();
            let doc = format!("---\nname: s\ndescription: d\nmetadata:\n  {name}.made-up-key: x\n---\n# body\n");
            let Ok(Some(out)) = vendor.skill_index(&doc) else {
                // Recorded, never skipped past. The key IS tool-namespaced for
                // every client here, so `None` (verbatim install) or `Err` means
                // this vendor answers the question some other way — which may be
                // defensible, but it is a deliberate exemption someone must
                // write down, not something the loop should quietly tolerate.
                // A bare `continue` here would reopen the exact hole deriving
                // this list from `ClientTarget::ALL` was meant to close.
                unrendered.push(name);
                continue;
            };
            assert!(
                !out.document.contains("made-up-key"),
                "'{name}' must drop an unknown own-namespace key: {}",
                out.document
            );
            // Collected rather than asserted in place so one run names every
            // SILENT-DROP offender: fixing this defect meant auditing five
            // vendors at once, and a fail-fast assert would have reported them
            // one per run. (The document-leak assert above stays fail-fast — a
            // key that survives is a different, louder bug.)
            if !out.warnings.iter().any(|w| w.contains("made-up-key")) {
                silent.push(name);
            }
        }
        assert!(
            silent.is_empty(),
            "these clients dropped an own-namespace key SILENTLY — the warning must name key and client: {silent:?}"
        );
        assert!(
            unrendered.is_empty(),
            "these clients neither rendered nor errored on an own-namespace key, so this test never judged them: {unrendered:?}"
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
