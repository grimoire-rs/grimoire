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
use crate::oci::hook::{
    CanonicalEvent, HookEntry, HookRegistration, HookSurface, HookTier, matcher_char_allowed, projection_for,
};
use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;

use super::hook_command::{self, CommandForm, HostOs, WindowsShell};
use super::hook_dispatch::RootToken;
use super::install_state::{ClientOutput, InstallState};
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
/// - [`Declined`](KindSupport::Declined): grim renders nothing for this
///   (vendor, kind) pair — warn + skip + zero outputs. Sometimes because no
///   ownable surface exists upstream at all (Codex rules, and the wave-1
///   declines); sometimes because a real surface exists but grim has not
///   built the render yet (Droid agents, Cline's CLI
///   agent surface) — a grim capability gap, not an
///   upstream absence. The module doc on each `Vendor` impl states which.
///
/// Behavior mapping onto the old bool: `Declined` is the old `false`;
/// `Native` and `Degraded` are both the old `true`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindSupport {
    /// Faithful native surface — the kind installs with full fidelity.
    Native,
    /// Ownable surface, reduced fidelity — installs with a warning.
    Degraded,
    /// Nothing rendered — warn + skip + zero outputs (no surface upstream,
    /// or a real one grim has not built yet; see `Vendor::kind_support`).
    Declined,
}

/// The member key marking an array element in a client-owned config as
/// **grim's**, paired with [`HOOK_MARKER_VALUE`].
///
/// One marker does two jobs for the nested splice: it is the `identity_keys`
/// member for the upsert, and it is the whole of the `owner` predicate for
/// enumerate-and-reap. Namespaced `com.grimoire.*` to match the OCI annotation
/// convention grim already owns (`com.grimoire.kind`, `com.grimoire.summary`);
/// a dotted JSON key is ordinary data (the shipped `amp.mcpServers` literal is
/// the in-repo precedent) and reads unmistakably as somebody's namespace rather
/// than a field the client should try to interpret.
///
/// # It goes on the HANDLER ELEMENT — the object the primitive tests
///
/// For claude the addressed element is a handler inside
/// `hooks.<Event>[].hooks[]`, so grim's handler object is
/// `{type, command, timeout?, com.grimoire.managed}`. **Corrected 2026-08-17
/// (WP-F post-stub review, F-1); an earlier draft of this doc put the marker on
/// the enclosing matcher *group* and that cannot be driven by the merged
/// primitive in either direction:**
///
/// - `upsert_nested_handler` raises `InvalidData` when *"`handler` itself lacks
///   one of `path.identity_keys`"*, and `handler` is the **element** — a
///   group-level marker makes every identity key unsatisfiable on the object
///   being tested, so every call refuses.
/// - `owned_nested_handlers` matches `owner` against **elements**. A group-level
///   marker is invisible to it, so the reap driver would own nothing — re-opening
///   the unreapable-registration hole (D-1) the constant marker exists to close,
///   one level up.
/// - There is no `upsert_nested_group` / `remove_nested_group` /
///   `owned_nested_groups`; `NestedGroupPath` was split out *only* so
///   `owned_nested_handlers` can address every group under one member. Adding a
///   group-level write would mean editing merged WP-D code.
///
/// The group-level placement was argued from "grim never interleaves a member
/// into the object Claude validates hardest" — **asserted, not evidenced**: both
/// levels are unverified on claude, while codex (the only client with executed
/// evidence) tolerates an unknown *handler* field and fails catastrophically only
/// at the *top* level. Escalate to the group only if the claude probe shows the
/// handler object rejects an unknown member.
///
/// grim still owns a whole group when it *creates* one — that is unaffected.
///
/// # Identity and ownership agree by construction
///
/// - **`identity_keys` = `[HOOK_MARKER_KEY]`, alone.** Not
///   `[HOOK_MARKER_KEY, "matcher"]`: `NestedHandlerPath` already carries
///   `group_value` — the value at `group_key`, i.e. the matcher — as a *separate*
///   field that selects the group **before** identity is consulted, so identity
///   is only ever resolved *within one already-selected group*, where Decision H
///   leaves at most one grim-owned element. WP-D states exactly this.
/// - **`owner` = `[(HOOK_MARKER_KEY, HOOK_MARKER_VALUE)]`**, the same one member.
///   One marker, two roles, no asymmetry to maintain — which is what WP-D says
///   the pair is *for*.
/// - **`["type", "command"]` is disqualified as an identity key** and is struck
///   from the plan: the command string embeds the launcher path and
///   `--root <abs workspace>`, so a workspace or `$GRIM_HOME` move changes the
///   value, identity stops matching the element on disk, and the next install
///   inserts a second element beside a husk no later run can name.
///
/// # Only splice surfaces need it
///
/// [`HookSurface::SpliceConfig`] only — claude in v1, and the five splice-shaped
/// phase-3 clients (cursor, gemini, droid, antigravity, junie). An
/// [`HookSurface::OwnFile`] client needs **no marker at all**: grim owns the
/// whole file, so ownership is the *path* and reaping is regenerating or
/// removing that file. That is why codex's and copilot's stricter parsers never
/// have to tolerate an unknown key — grim puts none in their files. (Codex would
/// in fact tolerate one inside a handler object, verified by execution; it
/// `deny_unknown_fields` at the **top level** and drops every hook in the file,
/// so a top-level marker there would be a catastrophic choice. Recorded because
/// the tempting symmetry — "stamp it everywhere" — is the wrong instinct.)
///
/// # Both strings are frozen forever (Principle 9)
///
/// They are written into a file grim does not own. Changing either one makes
/// every already-written registration invisible to a single-value `owner`
/// predicate — it stays armed, in a user-owned config, with nothing looking for
/// it.
///
/// A migration is *possible*: `owned_nested_handlers` takes an `owner`
/// **slice**, so a dual-value predicate spanning old and new spellings is the
/// same additive reap this repo already does elsewhere (the legacy
/// `open-code-*` tags, `reap_relocated_roots`, the reaped legacy
/// `catalog.json`). So the argument for freezing is **not** "no migration
/// exists" — it is that freezing avoids taking on that dual-predicate reaper
/// obligation, permanently, for a string that buys nothing by changing.
pub const HOOK_MARKER_KEY: &str = "com.grimoire.managed";

/// The marker's value: a **grim constant**, and that is the whole point.
///
/// # Why not the artifact name
///
/// Because the `owner` predicate has to match a registration whose artifact has
/// **already left install state** — an uninstalled artifact's name is precisely
/// the thing the registrar no longer has. A name-valued marker would satisfy
/// "stable and not path-derived" and still leave the registration armed forever
/// in a user-owned file (invariants I1/I5). The same argument disqualifies the
/// scope, the workspace path and `$GRIM_HOME`, each of which additionally makes
/// ownership *environment-derived*.
///
/// A string rather than `true`, so the predicate stays meaningful if grim ever
/// manages a second kind of element in the same config, and **unversioned**, so
/// it can never need to change: version or artifact identity, if ever required,
/// goes in a *different* member that is neither an identity key nor an `owner`
/// field. Under Decision L nothing in a registration is recorded and one
/// dispatcher serves every hook, so artifact identity has no business in there
/// at all — the dispatch table maps `(event, matcher)` to payloads.
pub const HOOK_MARKER_VALUE: &str = "hook-dispatcher";

/// The structural key names of a [`HookSurface::SpliceConfig`] client's hook
/// block — the part that cannot vary per registration.
///
/// Split out from [`SplicedHandler`] because the **reap** needs it with no
/// registration in hand: enumerate-and-reap runs precisely when the desired set
/// is empty, and it still has to address every group under every event member.
///
/// Claude's is `hooks.<Event>[].hooks[]` keyed on `matcher`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookSpliceShape {
    /// The top-level container object (claude: `hooks`).
    pub container: &'static str,
    /// The key whose value selects a group inside the member array
    /// (claude: `matcher`).
    pub group_key: &'static str,
    /// The key holding the group's element array (claude: `hooks`).
    pub elements_key: &'static str,
}

/// One grim-owned handler element on a [`HookSurface::SpliceConfig`] surface,
/// with the nested address that locates it.
///
/// Returned by [`Vendor::hook_spliced_handler`] so the convergence driver can
/// build a `json_splice::NestedHandlerPath` without knowing which client it is
/// talking to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplicedHandler {
    /// The client's structural key names.
    pub shape: HookSpliceShape,
    /// The container member for this event — the client's own event spelling.
    pub member: String,
    /// The value at `group_key` this element's group carries. Never absent:
    /// the primitive selects a group by a `&str`, so a match-all registration
    /// needs the client's own literal for it (claude: `*`).
    pub group_value: String,
    /// The element itself, marker included.
    pub element: serde_json::Value,
}

/// Why a `(hook, client)` pair is **not** registered.
///
/// A decline is a *reported outcome*, not a failure: the installer warns once,
/// records zero outputs, and `grim status` shows the pair `Declined` with this
/// reason (S-013) — the same direction every other unsupported `(client, kind)`
/// pair takes, and the fail-safe direction invariant **I3** requires. It is
/// deliberately **not** an `std::error::Error` and never becomes a
/// [`crate::error::Error`]: nothing here changes an exit code.
///
/// Carried in the `Err` arm of [`Vendor::hook_registration`] because that gives
/// the two-variant shape with a typed reason and no bespoke outcome enum —
/// `Option` (the [`Vendor::mcp_entry`] precedent) would lose the reason, and the
/// reason is exactly what makes a silent guardrail visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookDecline {
    /// The client has no hook surface at all ([`Vendor::hook_surface`] is
    /// `None`) — the default, and 15 of 18 clients today.
    NoSurface,
    /// The client's surface shape has no v1 writer
    /// ([`HookSurface::CodegenModule`]). Declined + one warning, **never a
    /// panic**: an unhandled surface must fail the way every other unsupported
    /// pair fails (I3).
    SurfaceUnimplemented,
    /// The client hosts no hook at this event — no
    /// [`RESPONSE_PROJECTION`](crate::oci::hook::RESPONSE_PROJECTION) row for
    /// the `(client, event)` pair.
    EventUnsupported,
    /// The declared tier cannot be honoured at this event on this client —
    /// [`Vendor::hook_tier_support`] returned [`KindSupport::Declined`]. Never
    /// silently degraded into a weaker tier: degrading a guardrail into a
    /// logger reports a security control as installed when it is not.
    TierUnsupported,
    /// **ADR decision K** — a `mutator` whose matcher could select a tool whose
    /// input *is a shell command string* ([`SHELL_COMMAND_TOOLS`]).
    ///
    /// Refused per `(tool, matcher)`, not per event, which is why it cannot live
    /// in [`Vendor::hook_tier_support`] — that signature is tool-blind. The ADR is
    /// explicit that the permitted-field table *"gates field names, not
    /// contents"*: a rewritten `{"command": "..."}` is a well-formed
    /// `updatedInput`, so nothing upstream of here catches it. This is
    /// CVE-2023-22809's shape and one of three controls the research would not
    /// ship the `mutator` tier without.
    MutatorOnShellCommandTool,
    /// The declared `matcher` has no lossless form in this client's matcher
    /// dialect (C-025) — today an interior `*`/`?`, or a regex metacharacter in
    /// an otherwise-exact name. Never approximated: WP-B executed both failure
    /// directions, and an approximated matcher is either inert or over-broad
    /// while still reporting as installed.
    MatcherNotLossless,
    /// The declared `matcher` is the empty string, which Copilot rejects
    /// outright (`matcher cannot be empty — hook will be skipped`) while Claude
    /// silently treats it as match-all. No translation is both faithful and
    /// non-skipped, so the pair declines rather than guessing which the author
    /// meant. **Owed upstream:** `grim build` should reject it as a manifest
    /// error (`HookManifest::validate` rule 3), which would make this variant
    /// unreachable — it stays as the seam's backstop.
    MatcherEmpty,
    /// The launcher path cannot be written into this client's command without
    /// ambiguity (control character, non-UTF-8, or unquotable under `cmd /C`).
    LauncherPathUnquotable,
}

impl HookDecline {
    /// The user-facing reason phrase, in library error style (lowercase, no
    /// trailing punctuation) so it composes into a warning or a `grim status`
    /// cell.
    pub fn reason(self) -> &'static str {
        match self {
            Self::NoSurface => "the client has no hook surface",
            Self::SurfaceUnimplemented => "this client's hook surface shape is not implemented yet",
            Self::EventUnsupported => "the client hosts no hook at this event",
            Self::TierUnsupported => "the client cannot honour this hook's tier at this event",
            Self::MutatorOnShellCommandTool => {
                "a 'mutator' may not rewrite the input of a tool whose input is a shell command string"
            }
            Self::MatcherNotLossless => "the matcher has no lossless form in this client's matcher dialect",
            Self::MatcherEmpty => "an empty matcher is rejected by at least one client and match-all on another",
            Self::LauncherPathUnquotable => "the launcher path cannot be quoted for this client's shell",
        }
    }
}

impl std::fmt::Display for HookDecline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

/// grim's own `matcher` dialect, classified for translation (C-025).
///
/// grim declares its matcher as "an exact tool name or a glob, never a regex".
/// No v1 client's matcher field is a glob — WP-B executed the matrix
/// (`research_hooks_launcher_verification.md` § 3): claude is a
/// **start-anchored case-sensitive regex** (`Ba*` means `B` + zero-or-more `a`,
/// so `Bash` matches by accident and `Bazz` matches too), codex is the same
/// regex over a Claude-style tool name, and copilot's PascalCase dialect is
/// literal-name matching that treats `Ba*` as neither glob nor match-all. So
/// only three authored forms survive translation, and the rest **decline**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatcherForm {
    /// No matcher, or the whole-string `*` — every tool.
    All,
    /// One exact tool name, or a `A|B` alternation of exact names. Translates
    /// as the identity function into all three v1 dialects.
    ExactOrAlternation,
    /// Nothing else translates losslessly — see [`HookDecline::MatcherNotLossless`].
    NotTranslatable,
    /// The empty string — see [`HookDecline::MatcherEmpty`].
    Empty,
}

/// Classify a declared `matcher` for translation into a client's dialect.
///
/// # Identity is the only portable translation, and it is **not lossless**
///
/// **Corrected 2026-08-17 (WP-F post-stub review, F-3).** An earlier draft of
/// this doc claimed "the three lossless forms agree, so v1 needs no per-client
/// branch". The agreement is real — the same three authored forms are the only
/// ones that translate at all on every client — but WP-B § 3.2 records two ways
/// the *result* still differs, and **neither is detectable by a
/// client-independent classifier**, because grim holds no per-client tool roster
/// to compare against. They are residuals to disclose, not a branch to write:
///
/// 1. **claude and codex are start-anchored but tail-OPEN.** `Ba*` fires and
///    `as` does not, and `^Bash$` fires — so the end is not forced and an "exact
///    name" is a **prefix** match there, while copilot's PascalCase dialect is a
///    literal match. `Bash` is a real prefix of Claude Code's real `BashOutput`,
///    so one manifest selects different tool sets per client. Fail-**safe** for
///    `observer`/`gatekeeper` (they fire on more than named); **not** for
///    `mutator`, which would rewrite the input of a tool the author never named —
///    which is why
///    [`matcher_may_select_shell_command_tool`] is prefix-aware rather than exact.
/// 2. **copilot PascalCase matches literal names case-INsensitively; claude is
///    case-sensitive.** So `matcher = "bash"` installs, reports installed, fires
///    on copilot, and is **silently inert on claude and codex** — the
///    silent-guardrail class reached through casing instead of dialect. Canonical
///    PascalCase tool names are therefore an **authoring requirement**, owed to
///    the published format doc (WP-M), not something this seam can enforce.
///
/// **Do not "fix" this with `^(?:NAME)$` anchoring.** WP-B § 3.1 shows `^Bash$`
/// fires on claude and does **not** fire on codex or copilot-PascalCase, so
/// anchoring is *less* portable than identity. Specify pins the residuals; it
/// does not hunt a per-client translation step.
///
/// # The admitted set is narrower than C-018's
///
/// C-018's [`MATCHER_ALLOWED`](crate::oci::hook::MATCHER_ALLOWED) asks *what may
/// be published*; this asks *what survives translation*. `.` is the sharp case —
/// it passes C-018 and is a regex metacharacter, so an "exact name" containing
/// one would match more tools than it names on claude and codex.
pub fn classify_matcher(matcher: Option<&str>) -> MatcherForm {
    let Some(matcher) = matcher else {
        return MatcherForm::All;
    };
    if matcher.is_empty() {
        return MatcherForm::Empty;
    }
    // Only the WHOLE-string `*` is match-all. `Ba*` is not a prefix glob on any
    // v1 client — on claude/codex it is the regex `B` + zero-or-more `a`, which
    // matches `Bazz` as readily as `Bash` — so it falls through to
    // `NotTranslatable` with everything else.
    if matcher == "*" {
        return MatcherForm::All;
    }
    if matcher.split('|').all(is_exact_tool_name) {
        MatcherForm::ExactOrAlternation
    } else {
        MatcherForm::NotTranslatable
    }
}

/// Whether one alternative is an **exact tool name** — the only thing that
/// translates as the identity function into all three v1 dialects.
///
/// Three disqualifiers, each for its own reason:
///
/// - **empty** (`A|`, `|B`, `A||B`) — an empty regex alternative matches
///   *everything* on claude and codex, so an alternation with one is silently
///   over-broad rather than merely lossy;
/// - **`*` / `?`** — glob metacharacters in grim's dialect and regex
///   metacharacters in claude's/codex's, and neither reading is the other's;
/// - **`.`** — the sharp case named in [`classify_matcher`]'s doc: it passes
///   C-018's charset and is a regex any-character, so `Read.md` would match tools
///   the author never named on two of the three clients.
///
/// A character outside C-018's [`matcher_char_allowed`] set is disqualifying too.
/// The charset is enforced at `grim build`, but a `hook.toml` on disk is not
/// bound by that (the W2(c) argument for re-checking `MATCHER_MAX_BYTES` at read
/// time), and "untranslatable" is the correct verdict for a character grim never
/// admitted in the first place — it declines with a legible reason instead of
/// handing an unvetted string to a client's matcher engine.
fn is_exact_tool_name(alternative: &str) -> bool {
    !alternative.is_empty()
        && alternative
            .chars()
            .all(|c| matcher_char_allowed(c) && !matches!(c, '*' | '?' | '.'))
}

/// Per-client tool names whose input **is a shell command string**, by
/// [`Vendor::name`] — the roster ADR decision K's `mutator` refusal is resolved
/// against.
///
/// # Why this exists at all
///
/// Decision K refuses `mutator` for these tools, and the ADR is explicit that
/// the permitted-field table *"gates field names, not contents"* and is therefore
/// **insufficient** on its own: a rewrite of `{"command": "..."}` is a
/// well-formed `hookSpecificOutput.updatedInput` no field-name check can catch.
/// The shape is CVE-2023-22809 (`sudo` `EDITOR`) — a value that passes
/// validation and then reaches an interpreter. It is one of three controls the
/// research would not ship the tier without.
///
/// # Why per client, when all three entries are `Bash` today
///
/// Because the name the *matcher* is compared against is per client and is not
/// the wire tool name. Codex's shell tool is `exec_command` on the wire and is
/// **renamed to `Bash` in the hook payload** (WP-B § 3.2: `Bash` matches,
/// `^exec_command$` does not), so a shared constant would be right by
/// coincidence and wrong the moment a phase-3 client lands — cursor, kiro and
/// gemini each spell their shell tool differently. An evidence roster keyed on
/// `Vendor::name()`, following [`POOL_CAPABLE_VENDORS`]' shape rather than adding
/// a fifth trait method.
///
/// A client absent here contributes nothing, which is correct for the 15 that
/// decline hooks outright: they never reach the check.
// No `dead_code` attribute here, deliberately — and the reason changed with
// this WP's Implement phase. It used to be that the `expect` on
// `shell_command_tools` made that function a live ROOT for rustc's reachability
// walk, so the const it reads already counted as used. Now the roster is read
// through a live call chain instead — `Vendor::hook_registration` (whose own
// `allow` is the root) → `matcher_may_select_shell_command_tool` →
// `shell_command_tools` — so an attribute here would still be unfulfillable, for
// the ordinary reason rather than the subtle one.
const SHELL_COMMAND_TOOLS: &[(&str, &[&str])] = &[
    // Claude Code's `Bash` tool takes `{"command": "<string>"}`.
    ("claude", &["Bash"]),
    // Wire name `exec_command`; the hook payload — and therefore the matcher —
    // sees `Bash`.
    ("codex", &["Bash"]),
    // PascalCase dialect only, which is the one grim registers; its camelCase
    // dialect spells the same tool `bash` and grim never emits that key.
    ("copilot", &["Bash"]),
];

/// The shell-command-string tool names for `client` ([`SHELL_COMMAND_TOOLS`]),
/// empty when the client declares none.
fn shell_command_tools(client: &str) -> &'static [&'static str] {
    match SHELL_COMMAND_TOOLS.iter().find(|(name, _)| *name == client) {
        Some((_, tools)) => tools,
        None => &[],
    }
}

/// Whether `matcher` could select **any** of `client`'s shell-command-string
/// tools — the Decision K predicate.
///
/// Conservative by construction: it answers "could", not "will", and every
/// uncertainty resolves to `true`, because the failure directions are not
/// symmetric. A false `true` costs one declined mutator with a legible reason; a
/// false `false` ships the command-string rewrite the ADR refused.
///
/// The rule, which Specify generates its cases from:
///
/// | [`MatcherForm`] | Answer | Why |
/// |---|---|---|
/// | `All` (absent or `*`) | **true** | matches every tool, shell tool included |
/// | `Empty` | **true** | claude treats `""` as match-all |
/// | `NotTranslatable` | **true** | a glob/regex could match; the pair declines anyway, but the Decision K reason is the informative one |
/// | `ExactOrAlternation` | true iff some alternative could name a roster tool | see below |
///
/// An alternative `a` could name roster tool `t` when `t` **starts with** `a`,
/// compared **case-insensitively**. Both relaxations are forced by executed
/// evidence, not caution: claude and codex are start-anchored but **tail-open**
/// regexes, so the matcher `Ba` fires on tool `Bash`; and copilot's PascalCase
/// dialect matches literal names case-insensitively, so `bash` fires on `Bash`
/// there. See [`classify_matcher`] for both residuals.
/// The rule table is implemented **verbatim**, including its one asymmetry: a
/// client with no [`SHELL_COMMAND_TOOLS`] row still answers `true` for `All` /
/// `Empty` / `NotTranslatable` and `false` for `ExactOrAlternation`. That is not
/// an oversight to smooth over — the three unconditional arms are what keep an
/// un-updated roster from silently *admitting* a mutator on a client whose shell
/// tool nobody has written down yet. A hook-capable client added without its row
/// therefore declines every match-all mutator and admits every named one; adding
/// the row is part of adding the client.
fn matcher_may_select_shell_command_tool(client: &str, matcher: Option<&str>) -> bool {
    let tools = shell_command_tools(client);
    match classify_matcher(matcher) {
        MatcherForm::All | MatcherForm::Empty | MatcherForm::NotTranslatable => true,
        // `matcher` is `Some` and non-empty for this form by construction; the
        // `unwrap_or_default` is the panic-free spelling of that, not a case.
        MatcherForm::ExactOrAlternation => matcher.unwrap_or_default().split('|').any(|alternative| {
            tools
                .iter()
                .any(|tool| starts_with_ignore_ascii_case(tool, alternative))
        }),
    }
}

/// Whether `tool` begins with `alternative`, compared case-insensitively — the
/// "could name" relation of [`matcher_may_select_shell_command_tool`].
///
/// Both relaxations are forced by executed evidence, not by caution: claude and
/// codex match a start-anchored but **tail-open** regex (matcher `Ba` fires on
/// tool `Bash`), and copilot's PascalCase dialect matches literal names
/// **case-insensitively** (matcher `bash` fires on tool `Bash`).
///
/// Byte-wise rather than `to_lowercase()`: every roster entry and every
/// C-018-admissible matcher character is ASCII, and slicing `tool` by
/// `alternative.len()` on bytes cannot panic on a char boundary the way `&tool[..n]`
/// could.
fn starts_with_ignore_ascii_case(tool: &str, alternative: &str) -> bool {
    let (tool, alternative) = (tool.as_bytes(), alternative.as_bytes());
    tool.len() >= alternative.len() && tool[..alternative.len()].eq_ignore_ascii_case(alternative)
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

/// How a settings key merges into a harness file, and so what grim owns of it.
///
/// A row's class decides whether its records carry an element, so changing it
/// is a layout move under the stability contract: a state migration, or the
/// row re-added under a new pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeClass {
    /// A container reached by recursion: no piece of its own, created when
    /// missing and pruned when left empty.
    #[expect(dead_code, reason = "a container is reached by recursion, so no row names it")]
    Object,
    /// A string array owned element by element.
    Set,
    /// A named entry of an object, owned and compared whole.
    Map,
    /// One value owned whole, ordered lists and atomic objects included.
    Scalar,
    /// A whole file grim writes and deletes.
    OwnFile,
}

/// The scopes a settings key applies at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsScopes {
    /// Project and global.
    Both,
    /// Global only: the key has no effect in a project file.
    GlobalOnly,
}

/// The value a settings key accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsValueType {
    /// A boolean.
    Bool,
    /// Any string.
    String,
    /// One of a closed set of strings.
    Enum(&'static [&'static str]),
    /// An array of strings.
    StringList,
    /// Any one of the listed types.
    OneOf(&'static [SettingsValueType]),
    /// An object replaced whole. Its first level is closed to `keys`; inner
    /// values are checked for `null` and floats only.
    Atomic { keys: &'static [&'static str] },
    /// An object replaced whole. Its keys are free; every value is one of a
    /// closed set of strings.
    EnumMap(&'static [&'static str]),
}

/// One row of a vendor's settings key list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsField {
    /// The RFC 6901 pointer of the key; an angle-bracketed segment such as
    /// `<name>` marks a map key.
    pub pointer: &'static str,
    /// The accepted value.
    pub ty: SettingsValueType,
    /// How the key merges.
    pub merge: MergeClass,
    /// The scopes the key applies at.
    pub scopes: SettingsScopes,
    /// The map keys accepted, when the vendor closes that set.
    pub map_keys: Option<&'static [&'static str]>,
    /// The values accepted at project scope, when narrower than `ty`.
    pub project_values: Option<&'static [&'static str]>,
    /// The harness needs this pointer, so grim never removes it.
    pub required_by_client: bool,
}

impl SettingsField {
    /// A row with no narrowed value and no required pointer, which is most of them.
    pub const fn new(pointer: &'static str, ty: SettingsValueType, merge: MergeClass, scopes: SettingsScopes) -> Self {
        Self {
            pointer,
            ty,
            merge,
            scopes,
            map_keys: None,
            project_values: None,
            required_by_client: false,
        }
    }
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
///   own native skills dir — verified 2026-07-26. They are the members
///   pool membership changes anything for: beside a pool writer their skills
///   join the pool (`expected_outputs::destination`), else they stay native.
/// - `goose` and `warp` scan it at **both** scopes, verified 2026-07-27
///   against each vendor's own docs. They differ in what grim writes by
///   default: Goose renders *into* the pool (its own `.goose/skills` is
///   labelled back-compat upstream, and `.agents/skills` the recommended
///   location), while Warp renders natively to `.warp/skills` and joins the
///   pool only beside a pool writer or through `shared_skills`. Membership here is about what a client
///   **reads**, not where grim writes — those are separate questions.
/// - `droid` and `kilo` scan it at **both** scopes, verified 2026-09-27
///   (Factory CLI v0.228.0 lists `.agents/skills` as a compatibility source at
///   project and personal scope; Kilo v7.8.1 scans project `.agents/skills`
///   and `~/.agents/skills` by default). Both render natively alone and join
///   the pool beside a pool writer or through `shared_skills`, the Warp shape.
/// - `cline` scans it at **both** scopes too, source-verified 2026-09-27
///   against extension v4.1.21 / CLI v3.0.65: the shared SDK's
///   `resolveSkillsConfigSearchPaths` and the VS Code extension's
///   `getSkillsDirectoriesForScan` both list project `.agents/skills` and
///   global `~/.agents/skills` alongside Cline's own `.cline/skills` —
///   reversing the earlier "confirmed absence" reading, which had checked
///   only the extension's docs page, not its source, and missed the CLI
///   entirely. Renders natively by default, the Warp shape.
/// - Absent, deliberately: `claude` (does not scan the pool), `kiro`,
///   `junie` and `qoder` (not evidenced either way),
///   `openclaw` (it does scan the pool at priority 3, but it is global-only
///   and the interaction between a scope-gapped client and `shared_skills` is
///   unproven — a deliberate deferral, since adding is additive and removing
///   is breaking), and `antigravity` — which **does**
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
    "codex", "gemini", "zed", "amp", "agents", "cursor", "copilot", "opencode", "goose", "warp", "droid", "kilo",
    "cline",
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

    /// Where this client keeps its hook registrations, or `None` when it hosts
    /// no hooks — **the one gate [`ArtifactKind::Hook`] resolves through**.
    ///
    /// # Why this is not `kind_support`
    ///
    /// ADR decision A, and it is the reason this seam exists at all.
    /// [`Self::kind_support`] defaults to [`KindSupport::Native`] and every
    /// vendor override closes its `match` with a wildcard arm, so adding a
    /// `Hook` variant there would make **all 18 vendors silently claim native
    /// hook support** — including Warp and Zed, which have no hook mechanism of
    /// any kind — with no compile error and nothing to grep for. A forgotten
    /// vendor must fail *safe*, so the answer is opt-in: `None` by default, and
    /// exactly the clients with a verified surface override it.
    ///
    /// # Scope-blind on purpose
    ///
    /// `Some(_)` does **not** mean "install here at any scope". ADR amendment
    /// A1 reduces project scope to **claude only**: codex's `.codex/hooks.json`
    /// and copilot's `.github/hooks/*.json` are *committed* repository files,
    /// and anything armable inside a repository violates invariant **I1**
    /// (attacker T3 — a repo you cloned but do not control). That gate is the
    /// shipped [`Self::kind_surface`] seam — the same mechanism as
    /// Junie-rules-at-global and OpenClaw-skills-at-project — never a scope
    /// field here and never a widening of `kind_support`.
    ///
    /// **That gate is a conjunct, not a replacement.** `client_supports_kind`'s
    /// `Hook` arm substitutes `hook_surface().is_some()` for the `kind_support`
    /// call **only**, and must keep `&& kind_surface(kind, scope)`. Decision A's
    /// wording says nothing about `kind_surface`, and the shipped function's
    /// `Mcp`/`Bundle` arms return *without* consulting it — so the literal reading
    /// ("`ArtifactKind::Hook => vendor.hook_surface().is_some()`", added beside
    /// those two) drops the scope gate and arms codex and copilot at **project**
    /// scope, into tracked repository files: invariant **I1**, attacker **T3**.
    /// WP-J2 owns that arm; a test that pins `kind_surface` directly — as this
    /// module's does — cannot catch the omission, so the pinned-set test must run
    /// through `client_supports_kind` at **both** scopes.
    ///
    /// # The v1 roster, and why each of the other 15 declines
    ///
    /// | Client | Surface | Why |
    /// |---|---|---|
    /// | claude | [`SpliceConfig`](HookSurface::SpliceConfig) | `settings.json` / `settings.local.json`, both scopes |
    /// | codex | [`OwnFile`](HookSurface::OwnFile) | `$CODEX_HOME/hooks.json`, **global only** (A1) |
    /// | copilot | [`OwnFile`](HookSurface::OwnFile) | `~/.copilot/hooks/grim.json`, **global only** (A1) |
    /// | qoder | [`SpliceConfig`](HookSurface::SpliceConfig) | `$QODER_CONFIG_DIR`/`~/.qoder` `settings.json`, **global only** (A8) |
    /// | warp, zed | `None` | **no hook mechanism exists** — the only two of 17 surveyed clients with none |
    /// | agents | `None` | the synthetic zero-detection fallback client; there is no product behind it to register into |
    /// | cursor, gemini, droid, antigravity, junie | `None` | splice-shaped surfaces, deferred to ADR phase 3 |
    /// | kiro, goose, cline | `None` | own-file surfaces, deferred to phase 3 |
    /// | opencode, kilo, amp, openclaw | `None` | **JS/TS plugin** surfaces — [`CodegenModule`](HookSurface::CodegenModule) shaped, and v1 ships no template (§ Out of Scope) |
    ///
    /// The 12 deferred clients decline through the default, with no per-vendor
    /// edit: adding one later is additive under Principle 9, and a decline that
    /// costs zero lines is a decline that cannot rot. Their evidence is
    /// `research_hooks_vendor_survey.md`'s master matrix (2026-08-14, expires
    /// 2026-11-14) — re-verify there before promoting one, per
    /// `vendor-capability-watchlist.md`.
    fn hook_surface(&self) -> Option<HookSurface> {
        None
    }

    /// Whether this client declines hooks **at every scope** — the permanence
    /// half of a per-client skip.
    ///
    /// Derived, never overridden: it is the negation of [`Self::hook_surface`],
    /// and it exists so the two consumers that ask it — the installer's
    /// `kind_is_permanently_declined` and `path_anchor`'s
    /// `is_declined_global_pair` — cannot spell it differently. Both carry docs
    /// requiring them to agree, which is a note where a shared definition is
    /// the fix; two spellings of one predicate is how the browse filter and the
    /// TUI tree came to disagree about a row.
    ///
    /// Deliberately **not** `kind_support(Hook)`: that defaults to
    /// [`KindSupport::Native`] and every vendor override closes its `match` with
    /// a wildcard, so it answers "supported" for hooks on all 18 vendors —
    /// including the two with no hook mechanism at all. `hook_surface` is opt-in,
    /// so a forgotten vendor fails safe.
    fn declines_hooks_everywhere(&self) -> bool {
        self.hook_surface().is_none()
    }

    /// This client's own name for `event`, or `None` when it hosts no hooks.
    ///
    /// Derived from [`Self::hook_surface`] rather than overridden per vendor,
    /// because every v1 client reads the **canonical PascalCase** spelling and
    /// one of them has a trap:
    ///
    /// **Copilot accepts two dialects, selected by the casing of the event key
    /// in its config file, and they are not equivalent.** WP-B executed both
    /// (`research_hooks_launcher_verification.md` § 3.1, § 6.3). Under
    /// camelCase `preToolUse` the matcher becomes an anchored full-match regex
    /// against a *lowercased* tool name, so grim's `matcher = "Bash"` **never
    /// fires** and `matcher = "*"` is **rejected as an invalid regex and the
    /// hook is skipped** — a guardrail that reports installed and does nothing.
    /// Under PascalCase `PreToolUse` grim's Claude-style dialect translates 1:1
    /// and the payload arrives in the same snake_case shape as claude and codex.
    /// Deriving the name here means the camelCase form is unreachable without a
    /// deliberate override, which is the correct amount of friction for it.
    ///
    /// Naming is not support: a `Some` here says only how the event is spelled.
    /// Whether the client hosts a hook at that event is
    /// [`RESPONSE_PROJECTION`](crate::oci::hook::RESPONSE_PROJECTION)'s answer,
    /// read through [`Self::hook_tier_support`].
    fn hook_event_name(&self, event: CanonicalEvent) -> Option<&'static str> {
        self.hook_surface().map(|_| event.as_str())
    }

    /// How faithfully this client can honour `tier` at `event`.
    ///
    /// **A query over [`RESPONSE_PROJECTION`](crate::oci::hook::RESPONSE_PROJECTION),
    /// never a second copy of it (C-021).** The projection table is the one
    /// instance of the `(vendor, event)` matrix; a per-vendor verdict table
    /// would drift, and the drift direction is "runtime emits a field
    /// render-time forbade" — the Codex fail-closed bug the table exists to
    /// prevent. The whole rule is three lines, and Specify generates its tests
    /// from them:
    ///
    /// 1. No surface, or no row for `(self.name(), event)` ⇒
    ///    [`KindSupport::Declined`].
    /// 2. The tier's **required** field absent ⇒ `Declined`. Required:
    ///    `gatekeeper` → a non-empty
    ///    [`verdict`](crate::oci::hook::ProjectionRow::verdict);
    ///    `mutator` → [`mutation`](crate::oci::hook::ProjectionRow::mutation);
    ///    `observer` → nothing, so an observer is never declined on a hosted
    ///    event.
    /// 3. Otherwise a field the tier *may* use absent ⇒
    ///    [`KindSupport::Degraded`] (dropped with one warning); else
    ///    [`KindSupport::Native`]. May-use: every tier may emit context, so an
    ///    absent `context` degrades all three.
    ///
    /// `Degraded` never weakens the tier itself — a tier the client cannot
    /// honour is `Declined`, never silently downgraded into a weaker one.
    ///
    /// # Not the arming authority for `mutator` — do not report from this alone
    ///
    /// This signature is **tool-blind and matcher-blind**, so ADR decision K
    /// (`mutator` refused for tools whose input is a shell command string) is
    /// unexpressible here and lives in [`Self::hook_registration`], which has the
    /// entry. Consequence: `hook_tier_support(Mutator, PreToolUse)` answers
    /// `Native` on claude and copilot while the registration for
    /// `matcher = "Bash"` **declines**
    /// ([`HookDecline::MutatorOnShellCommandTool`]). Both answers are correct at
    /// their own granularity — "the client can express an input rewrite at this
    /// event" is not "this hook is armed".
    ///
    /// A consumer that reports arming, or fills a compat-matrix cell, from this
    /// method alone would show a `mutator` as available where nothing is
    /// registered — an S-013 silent-guardrail report. The registration is the
    /// authority; this is the capability question it asks first.
    fn hook_tier_support(&self, tier: HookTier, event: CanonicalEvent) -> KindSupport {
        // Rule 1's first half, and it is deliberately ahead of the lookup: it is
        // the fail-safe half, it is what makes the 15 declining clients answer
        // correctly, and it must not depend on a table row existing.
        if self.hook_surface().is_none() {
            return KindSupport::Declined;
        }
        // Rule 1's second half. `projection_for` is the ONE lookup into the table
        // (C-021) — scanning `RESPONSE_PROJECTION` here would be the second
        // reader the table's own doc forbids, and the drift it warns about runs
        // in the fail-open direction.
        let Some(row) = projection_for(self.name(), event) else {
            return KindSupport::Declined;
        };
        // Rule 2 — the tier's REQUIRED field. Never degraded into a weaker tier:
        // reporting a guardrail as installed when it cannot block is the
        // silent-guardrail class this whole seam exists to prevent.
        let required_present = match tier {
            HookTier::Gatekeeper => !row.verdict.is_empty(),
            HookTier::Mutator => row.mutation.is_some(),
            HookTier::Observer => true,
        };
        if !required_present {
            return KindSupport::Declined;
        }
        // Rule 3 — a MAY-use field absent. Every tier may emit context, so an
        // absent `context` degrades all three (installed, one warning, the
        // canonical field dropped).
        if row.context.is_none() {
            return KindSupport::Degraded;
        }
        KindSupport::Native
    }

    /// How this client takes its hook command: exec form (Copilot) or a shell
    /// string. The default is the shell form Claude and Qoder use, with
    /// PowerShell as their Windows shell.
    fn hook_command_form(&self) -> CommandForm {
        CommandForm::Shell(WindowsShell::PowerShell)
    }

    /// Assemble the registration this client writes for
    /// `(entry, event, launcher, root, host_os)`: the single
    /// [`HookEntry`] to [`HookRegistration`] assembly site.
    ///
    /// The command runs the native launcher as `<client> <event> <root>`
    /// ([`hook_command`]). Nothing from `entry` reaches it: `matcher` lands in the
    /// client's structured matcher field and `timeout` in `timeout`, so a
    /// manifest stuffed with shell metacharacters yields a byte-identical
    /// command (pinned by `hook_command_ignores_every_entry_field`). The launcher
    /// path is the only operand that can need quoting, and the shell form's
    /// bytes are stable because Codex hashes the raw command text.
    ///
    /// # Refusal order
    ///
    /// [`HookDecline::NoSurface`], [`SurfaceUnimplemented`](HookDecline::SurfaceUnimplemented),
    /// [`EventUnsupported`](HookDecline::EventUnsupported),
    /// [`TierUnsupported`](HookDecline::TierUnsupported),
    /// [`MutatorOnShellCommandTool`](HookDecline::MutatorOnShellCommandTool),
    /// [`MatcherEmpty`](HookDecline::MatcherEmpty) or
    /// [`MatcherNotLossless`](HookDecline::MatcherNotLossless), then
    /// [`LauncherPathUnquotable`](HookDecline::LauncherPathUnquotable). Decision K
    /// stays ahead of the matcher refusals because it is the one reason that
    /// explains the design.
    ///
    /// # Errors
    ///
    /// One [`HookDecline`] per refusal above; a decline is warn-and-skip.
    fn hook_registration(
        &self,
        entry: &HookEntry,
        event: CanonicalEvent,
        launcher: &Path,
        root: &RootToken,
        host_os: HostOs,
    ) -> Result<HookRegistration, HookDecline> {
        // The fail-safe gate first. `CodegenModule` resolves to a decline **plus
        // one warning at the caller** and never to a panic (I3) — a `match` arm
        // reaching for `unimplemented!()` here would re-create, inside new code,
        // exactly the reachable-panic defect WP-A's hook marker arms were
        // corrected for.
        //
        // Nothing is bound from the surface: `OwnFile` and `SpliceConfig` produce
        // the *same* registration, because the surface says who owns the file the
        // registration is written into, not what it says. Only the writer (WP-I)
        // branches on it.
        match self.hook_surface() {
            None => return Err(HookDecline::NoSurface),
            Some(HookSurface::CodegenModule) => return Err(HookDecline::SurfaceUnimplemented),
            Some(HookSurface::SpliceConfig | HookSurface::OwnFile) => {}
        }
        // Two independent ways a client can host no hook at this event, one
        // reason: it cannot spell the event (a vendor that overrode
        // `hook_event_name` to `None` for it), or the projection table has no row
        // for the pair. Naming is not support, so both must be checked.
        let Some(event_name) = self.hook_event_name(event) else {
            return Err(HookDecline::EventUnsupported);
        };
        if projection_for(self.name(), event).is_none() {
            return Err(HookDecline::EventUnsupported);
        }
        if self.hook_tier_support(entry.tier, event) == KindSupport::Declined {
            return Err(HookDecline::TierUnsupported);
        }
        // ADR decision K, deliberately AHEAD of the two matcher refusals: an
        // empty or untranslatable matcher on a `mutator` declines either way, so
        // the order changes no verdict — only which reason the author is told, and
        // this is the one that explains the design.
        if entry.tier == HookTier::Mutator
            && matcher_may_select_shell_command_tool(self.name(), entry.matcher.as_deref())
        {
            return Err(HookDecline::MutatorOnShellCommandTool);
        }
        // C-025. Identity is the only translation any v1 client shares, so a
        // translatable matcher passes through verbatim into the client's own
        // STRUCTURED matcher field — never into the command text.
        let matcher = match classify_matcher(entry.matcher.as_deref()) {
            MatcherForm::All => None,
            MatcherForm::ExactOrAlternation => entry.matcher.clone(),
            MatcherForm::Empty => return Err(HookDecline::MatcherEmpty),
            MatcherForm::NotTranslatable => return Err(HookDecline::MatcherNotLossless),
        };
        let rendered = hook_command::render(
            self.hook_command_form(),
            launcher,
            self.name(),
            event_name,
            root.as_str(),
            host_os,
        )
        .map_err(|_| HookDecline::LauncherPathUnquotable)?;
        Ok(HookRegistration {
            event: event_name.to_string(),
            matcher,
            command: rendered.command,
            shell: rendered.shell,
            timeout: entry.timeout,
        })
    }

    /// Where this client's hook registrations live for `scope`, or `None` when
    /// it hosts none there.
    ///
    /// The promotion `hook_registrar::sync_for_state`'s doc owed: the
    /// *location* beside [`Self::hook_surface`]'s *shape*, so a generic
    /// consumer — the convergence driver, `grim status`'s `not-armed` probe,
    /// `expected_outputs` — can ask a client where its registration lives
    /// without knowing which client it is. Previously each vendor kept a
    /// private free function and passed the result into the registrar, which
    /// made the driver impossible to write generically.
    ///
    /// `None` is the honest answer at a scope the client hosts no hooks at
    /// (codex and copilot at **project** scope, amendment A1) — belt and braces
    /// with `kind_surface(Hook, scope)`, which stays the authoritative gate.
    fn hook_config_path(&self, _workspace: &Path, _scope: ConfigScope) -> Option<PathBuf> {
        None
    }

    /// The complete document grim writes into a
    /// [`HookSurface::OwnFile`] client's own hook file for `registrations`.
    ///
    /// `None` for every other surface, so the convergence driver can dispatch on
    /// the returned `Option` rather than on `self.name()` — a
    /// `match vendor.name()` in a shared module is the silent-drift shape D-1 is
    /// about.
    ///
    /// **No grim marker goes in either file.** Ownership on an `OwnFile`
    /// surface is the *path*, and codex `deny_unknown_fields` at the top level
    /// and drops **every** hook in the file on one unknown key.
    ///
    /// The two v1 `OwnFile` clients do **not** share a shape — codex nests
    /// handlers inside per-event matcher *groups* while copilot takes a flat
    /// per-event array with the matcher on each entry, and match-all is spelled
    /// by omission on both because copilot rejects `*` as an invalid regex — so
    /// this cannot be defaulted into one shared body.
    fn hook_file_document(&self, _registrations: &[HookRegistration]) -> Option<serde_json::Value> {
        None
    }

    /// This client's hook-block key names, or `None` when its surface is not
    /// [`HookSurface::SpliceConfig`].
    ///
    /// Separate from [`Self::hook_spliced_handler`] because the reap needs it
    /// with no registration to derive it from.
    fn hook_splice_shape(&self) -> Option<HookSpliceShape> {
        None
    }

    /// The addressed handler element grim splices into a
    /// [`HookSurface::SpliceConfig`] client's user-owned config for one
    /// registration, and the group address that locates it.
    ///
    /// `None` for every other surface. The element carries
    /// [`HOOK_MARKER_KEY`]/[`HOOK_MARKER_VALUE`], which is both the upsert's
    /// identity key and the whole of the enumerate-and-reap `owner` predicate.
    fn hook_spliced_handler(&self, _registration: &HookRegistration) -> Option<SplicedHandler> {
        None
    }

    /// Known `<vendor>.*` skill metadata fields lifted into native
    /// `SKILL.md` frontmatter. Empty ⇒ the vendor reads only universal
    /// agentskills fields (any own-namespace key is a typo: warn + drop).
    fn skill_fields(&self) -> &'static [KnownField] {
        &[]
    }

    /// Whether this client actually **reads** the cross-vendor
    /// `$HOME/.agents/skills` pool, so its skills may render there: beside a
    /// pool writer automatically, or as `[options.vendors.<name>].shared_skills`
    /// pins it.
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

    /// Whether this client scans the shared `.agents/skills` pool in the
    /// current environment at `scope`.
    ///
    /// Gates the pool auto-join only: a reader that has switched its pool scan
    /// off keeps its skills native, since joining would hide them from it.
    /// An explicit `shared_skills = true` is not gated. Implies
    /// [`Self::pool_capable`]; an override may only narrow it.
    fn reads_pool(&self, _scope: ConfigScope) -> bool {
        self.pool_capable()
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

    /// The agent-name grammar this client enforces when it is narrower than
    /// grim's own, or `None` when every grim name is valid here. An agent
    /// whose name falls outside it is skipped for this client with a warning
    /// — never renamed, which would break every reference to it — and
    /// [`super::installer::client_hosts`] leaves it out of every support
    /// decision, so the skip is not reported as pending drift.
    fn agent_name_grammar(&self) -> Option<super::render::NameGrammar> {
        None
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

    /// Every MCP config file the vendor reads at `scope` — the surface the
    /// installer writes and every support decision consults. Empty ⇔ no
    /// writable surface. Default: [`Self::mcp_config_path`] alone; a vendor
    /// whose clients read more than one file (Copilot: VS Code Chat's
    /// `.vscode/mcp.json` and the CLI's `.github/mcp.json`) overrides this,
    /// and each file gets its own recorded output.
    fn mcp_config_paths(&self, workspace: &Path, scope: ConfigScope) -> Vec<PathBuf> {
        self.mcp_config_path(workspace, scope).into_iter().collect()
    }

    /// Top-level keys of a grim-managed MCP entry that the vendor itself
    /// writes into it — approvals, toggles, OAuth tokens. They are left out
    /// of the integrity hash and the adopt/refuse comparison, and a grim
    /// rewrite carries them over from the entry on disk, so the client's own
    /// state neither reads as a user edit nor gets wiped. Uninstall still
    /// removes the whole entry. Default: none, which keeps every other
    /// vendor byte-identical.
    fn mcp_entry_vendor_owned_keys(&self) -> &'static [&'static str] {
        &[]
    }

    /// The config-file format [`Self::mcp_config_path`] writes, so the
    /// installer's MCP registration step picks the matching span-preserving
    /// splice engine ([`super::json_splice`] vs [`super::toml_splice`]).
    /// Default [`McpConfigFormat::Json`] — every vendor but Codex writes a
    /// JSON/JSONC config; Codex's `config.toml` is TOML.
    fn mcp_config_format(&self) -> McpConfigFormat {
        McpConfigFormat::Json
    }

    /// Every settings key this vendor accepts, as data. Empty means the client
    /// hosts no settings, which `client_supports_kind` answers from here rather
    /// than from [`Self::kind_support`], whose `Native` default says yes to all.
    fn settings_fields(&self) -> &'static [SettingsField] {
        &[]
    }

    /// The vendor's native settings file for `scope`, or `None` when it has no
    /// file there. Default: no file.
    fn settings_config_path(&self, _workspace: &Path, _scope: ConfigScope) -> Option<PathBuf> {
        None
    }

    /// The format of the [`Self::settings_config_path`] file, which picks the
    /// splice engine. Independent of [`Self::mcp_config_format`].
    fn settings_config_format(&self) -> McpConfigFormat {
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

    /// [`Self::mcp_entry`] for one file of [`Self::mcp_config_paths`] — what
    /// the installer calls, once per file. Default: every file takes the same
    /// entry; a vendor whose files follow different schemas overrides this
    /// (Copilot's `.github/mcp.json` takes the CLI shape, `.vscode/mcp.json`
    /// VS Code's).
    fn mcp_entry_for(
        &self,
        scope: ConfigScope,
        _config_path: &Path,
        name: &str,
        descriptor: &crate::oci::mcp::McpDescriptor,
    ) -> Option<(String, serde_json::Value)> {
        self.mcp_entry(scope, name, descriptor)
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

    /// [`Self::agent_index`] with the installed binding name (the file stem)
    /// known, for a vendor that must write a name its grammar accepts where
    /// the artifact's own frontmatter `name` would be rejected. Default:
    /// ignores `binding` and delegates.
    ///
    /// # Errors
    ///
    /// As [`Self::agent_index`].
    fn agent_index_named(
        &self,
        parsed: &ParsedAgent,
        binding: &str,
        pinned: &str,
    ) -> Result<Option<RenderedDoc>, RenderError> {
        let _ = binding;
        self.agent_index(parsed, pinned)
    }

    /// Converge vendor-owned configuration on the current install state —
    /// the reversible config-registration seam (hooks ADR pattern).
    /// Called after install/update/uninstall mutated `state` for every
    /// involved vendor. Default: no-op.
    ///
    /// `retired` carries the [`ClientOutput`]s the triggering operation
    /// removed from the state — every client's, not just this vendor's;
    /// [`super::install_state::retired_outputs`] computes it once per
    /// command from a pre-mutation snapshot. `state` alone cannot answer
    /// "what went away": an uninstalled artifact's record, and its name with
    /// it, is already gone by the time the sync runs, so a vendor that must
    /// **deregister** something needs the removal evidence handed to it
    /// rather than guessed from the filesystem. Empty on a pure install; a
    /// vendor whose managed config is a pure function of `state` (OpenCode's
    /// single `instructions` glob) ignores it.
    ///
    /// # Errors
    ///
    /// An I/O failure editing the vendor config (the operation that
    /// triggered the sync still completed; callers surface the error).
    fn sync_config(
        &self,
        _state: &InstallState,
        _workspace: &Path,
        _scope: ConfigScope,
        _retired: &[ClientOutput],
    ) -> io::Result<()> {
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

/// Whether a vendor's boolean disable flag reads as set.
///
/// Only an unset or empty variable, or an explicit false spelling, counts as
/// off. A value the vendor might parse differently counts as set, so the
/// skill stays native and shows twice instead of not loading at all.
pub(crate) fn disable_flag_from(value: Option<&str>) -> bool {
    value.is_some_and(|v| {
        let v = v.trim();
        !v.is_empty()
            && !["0", "false", "no", "off", "n"]
                .iter()
                .any(|f| v.eq_ignore_ascii_case(f))
    })
}

/// [`disable_flag_from`] over the process environment.
pub(crate) fn env_disable_flag(var: &str) -> bool {
    disable_flag_from(std::env::var(var).ok().as_deref())
}

/// `$XDG_CONFIG_HOME`, else `$HOME/.config`, when resolvable.
pub fn xdg_config_dir() -> Option<PathBuf> {
    env_dir("XDG_CONFIG_HOME").or_else(|| home_dir().map(|h| h.join(".config")))
}

/// The cross-vendor shared skills pool `$HOME/.agents/skills` — the open
/// standard scanned by every pool member (keyed on `$HOME` only,
/// **not** relocated by any vendor's config-dir override). The
/// [`PathAnchor`](super::path_anchor) `AgentsSkills` anchor is rooted here.
pub(crate) fn global_skills_root(home: Option<PathBuf>) -> Option<PathBuf> {
    home.map(|h| h.join(".agents").join("skills"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::hook::{HookCommand, HookShell};

    /// Decision K's roster is keyed by client name, outside the trait, so
    /// nothing else forces a row for a new mutator-capable client — and without
    /// one, a named `matcher = "Bash"` mutator would be admitted on it.
    #[test]
    fn every_mutator_capable_client_names_its_shell_command_tools() {
        let mut checked = Vec::new();
        for client in crate::install::client_target::ClientTarget::ALL {
            let vendor = client.vendor();
            if vendor.hook_tier_support(HookTier::Mutator, CanonicalEvent::PreToolUse) == KindSupport::Declined {
                continue;
            }
            assert!(
                !shell_command_tools(vendor.name()).is_empty(),
                "{client} hosts PreToolUse mutators but has no SHELL_COMMAND_TOOLS row"
            );
            checked.push(vendor.name());
        }
        assert!(checked.contains(&"claude"), "POSITIVE CONTROL: {checked:?}");
    }

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
                "opencode", "copilot", "codex", "cursor", "gemini", "zed", "amp", "agents", "cline", "droid", "goose",
                "warp", "kilo"
            ],
            "the pool-capable set is an evidence roster; a client joining or leaving it is a deliberate change"
        );
        // Claude is the verified non-reader AND the only vendor declaring
        // skill fields — both reasons must independently exclude it.
        assert!(!ClientTarget::Claude.vendor().pool_capable());
        assert!(!ClientTarget::Kiro.vendor().pool_capable());
        assert!(!ClientTarget::Junie.vendor().pool_capable());
        assert!(!ClientTarget::Qoder.vendor().pool_capable());
        // Cline scans both `.agents/skills` (project) and `~/.agents/skills`
        // (global) in its shared SDK and its VS Code extension alike —
        // verified 2026-09-27 against source, reversing the earlier
        // confirmed-absence reading.
        assert!(ClientTarget::Cline.vendor().pool_capable());
        // Partial members / deliberate deferrals — the shape that silently
        // writes global skills where the client never scans if let in.
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

    #[test]
    fn a_disable_flag_reads_set_unless_unset_empty_or_spelled_false() {
        for off in [
            None,
            Some(""),
            Some("  "),
            Some("0"),
            Some("false"),
            Some("FALSE"),
            Some("no"),
            Some("off"),
            Some("n"),
        ] {
            assert!(!disable_flag_from(off), "{off:?} must read as unset");
        }
        for on in [
            Some("1"),
            Some("true"),
            Some("yes"),
            Some("on"),
            Some("2"),
            Some("anything"),
        ] {
            assert!(disable_flag_from(on), "{on:?} must read as set");
        }
    }

    /// `reads_pool` may only narrow `pool_capable`: a vendor that reads the
    /// pool without being pool-capable would auto-join a pool it cannot load.
    #[test]
    fn reads_pool_implies_pool_capable_for_every_vendor() {
        for client in crate::install::ClientTarget::ALL {
            let vendor = client.vendor();
            for scope in [ConfigScope::Project, ConfigScope::Global] {
                assert!(
                    !vendor.reads_pool(scope) || vendor.pool_capable(),
                    "'{}' reads the pool at {scope:?} but is not pool-capable",
                    vendor.name()
                );
            }
        }
    }

    /// Every `(client, kind, scope)` whose surface is absent — the complete
    /// exception set to `kind_surface`'s `true` default.
    const SCOPE_GAPS: &[(&str, ArtifactKind, ConfigScope)] = &[
        // `.junie/rules/` is ownable; no global `~/.junie/rules/` exists.
        ("junie", ArtifactKind::Rule, ConfigScope::Global),
        // OpenClaw has no per-repository scope: its "project" path is a fixed
        // daemon home that does not track the repo grim was invoked in.
        ("openclaw", ArtifactKind::Skill, ConfigScope::Project),
        // Hooks at project scope, ADR amendment A1. Both clients HAVE a working
        // project hook surface; both are TRACKED repository files, so a
        // registration there would put something armable inside a repository
        // (I1, attacker T3) and would name an environment-derived launcher path.
        // The gap is the security decision, not a missing directory — which is
        // why it is spelled here rather than left to `hook_surface`, whose
        // answer is deliberately scope-blind.
        ("codex", ArtifactKind::Hook, ConfigScope::Project),
        ("copilot", ArtifactKind::Hook, ConfigScope::Project),
        // Qoder (ADR amendment A8, D-2): `.qoder/settings.json` is tracked too.
        ("qoder", ArtifactKind::Hook, ConfigScope::Project),
    ];

    #[test]
    fn kind_surface_is_true_everywhere_except_the_declared_scope_gaps() {
        use crate::install::client_target::ClientTarget;

        // `kind_surface` defaults to `true`, so it must not have narrowed any
        // shipped vendor. Asserted over the real roster at BOTH scopes and all
        // four file kinds rather than by reading the default — an accidental
        // override would silently stop a client installing a kind the compat
        // matrix says it supports, and nothing else would notice.
        //
        // `Hook` is in the loop because the two hook rows in `SCOPE_GAPS` are a
        // security decision (A1), and a flip in either direction is silent: made
        // `true`, grim writes an armable registration into a tracked file; made
        // `false` at global too, hooks stop installing anywhere on that client.
        // A `true` answer here says nothing about hook *capability* — that is
        // `hook_surface`'s question, and it is `None` for 15 of these clients.
        for client in ClientTarget::ALL {
            for kind in [
                ArtifactKind::Skill,
                ArtifactKind::Rule,
                ArtifactKind::Agent,
                ArtifactKind::Hook,
            ] {
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

    /// The clients that list settings keys, sorted; every other client
    /// declines the kind outright.
    const SETTINGS_HOSTS: [&str; 13] = [
        "amp",
        "antigravity",
        "claude",
        "codex",
        "copilot",
        "cursor",
        "droid",
        "gemini",
        "kilo",
        "kiro",
        "opencode",
        "qoder",
        "warp",
    ];

    /// `Settings` is limited to [`SETTINGS_HOSTS`] and carries no scope gap:
    /// `kind_surface` stays `true` at both scopes for every client, and the
    /// narrowing is the key list. The project-scope answer is the file path's.
    #[test]
    fn settings_is_a_listed_vendor_kind_with_no_scope_gap() {
        use crate::install::client_target::ClientTarget;

        let mut hosts: Vec<&str> = Vec::new();
        for client in ClientTarget::ALL {
            let vendor = client.vendor();
            if !vendor.settings_fields().is_empty() {
                hosts.push(vendor.name());
            }
            for scope in [ConfigScope::Project, ConfigScope::Global] {
                assert!(
                    vendor.kind_surface(ArtifactKind::Settings, scope),
                    "{client} kind_surface(Settings, {scope:?})"
                );
            }
            assert!(
                !SCOPE_GAPS
                    .iter()
                    .any(|(c, k, _)| *c == vendor.name() && *k == ArtifactKind::Settings),
                "{client}: a settings scope gap belongs in the key list or the path, not SCOPE_GAPS"
            );
        }
        let mut sorted = hosts.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, SETTINGS_HOSTS);
    }

    /// Appendix A in full: `(vendor, pointer, class)`. A class change is a
    /// layout move under the stability contract, so a row cannot change class
    /// without this table changing, and the table is written from the spec.
    const SETTINGS_GOLDEN: &[(&str, &str, MergeClass)] = &[
        ("claude", "/model", MergeClass::Scalar),
        ("claude", "/effortLevel", MergeClass::Scalar),
        ("claude", "/outputStyle", MergeClass::Scalar),
        ("claude", "/language", MergeClass::Scalar),
        ("claude", "/spinnerTipsEnabled", MergeClass::Scalar),
        ("claude", "/alwaysThinkingEnabled", MergeClass::Scalar),
        ("claude", "/includeGitInstructions", MergeClass::Scalar),
        ("claude", "/permissions/allow", MergeClass::Set),
        ("claude", "/permissions/deny", MergeClass::Set),
        ("claude", "/permissions/ask", MergeClass::Set),
        ("claude", "/permissions/defaultMode", MergeClass::Scalar),
        ("claude", "/sandbox/enabled", MergeClass::Scalar),
        ("claude", "/extraKnownMarketplaces/<name>", MergeClass::Map),
        ("claude", "/theme", MergeClass::Scalar),
        ("claude", "/editorMode", MergeClass::Scalar),
        ("claude", "/tui", MergeClass::Scalar),
        ("claude", "/attribution/commit", MergeClass::Scalar),
        ("claude", "/attribution/pr", MergeClass::Scalar),
        ("claude", "/attribution/sessionUrl", MergeClass::Scalar),
        ("claude", "/sandbox/filesystem/allowWrite", MergeClass::Set),
        ("claude", "/sandbox/filesystem/denyWrite", MergeClass::Set),
        ("claude", "/sandbox/filesystem/denyRead", MergeClass::Set),
        ("claude", "/sandbox/filesystem/allowRead", MergeClass::Set),
        ("codex", "/model", MergeClass::Scalar),
        ("codex", "/model_reasoning_effort", MergeClass::Scalar),
        ("codex", "/approval_policy", MergeClass::Scalar),
        ("codex", "/sandbox_mode", MergeClass::Scalar),
        ("codex", "/sandbox_workspace_write/network_access", MergeClass::Scalar),
        ("codex", "/sandbox_workspace_write/writable_roots", MergeClass::Set),
        (
            "codex",
            "/sandbox_workspace_write/exclude_slash_tmp",
            MergeClass::Scalar,
        ),
        (
            "codex",
            "/sandbox_workspace_write/exclude_tmpdir_env_var",
            MergeClass::Scalar,
        ),
        ("codex", "/default_permissions", MergeClass::Scalar),
        ("codex", "/permissions/<name>", MergeClass::Map),
        ("codex", "/tui/alternate_screen", MergeClass::Scalar),
        ("codex", "/tui/animations", MergeClass::Scalar),
        ("codex", "/tui/auto_recap", MergeClass::Scalar),
        ("codex", "/tui/raw_output_mode", MergeClass::Scalar),
        ("codex", "/history/persistence", MergeClass::Scalar),
        ("codex", "/hide_agent_reasoning", MergeClass::Scalar),
        ("codex", "/marketplaces/<name>", MergeClass::Map),
        ("codex", "rules", MergeClass::OwnFile),
        ("codex", "/file_opener", MergeClass::Scalar),
        ("codex", "/tui/notification_method", MergeClass::Scalar),
        ("codex", "/tui/notification_condition", MergeClass::Scalar),
        ("codex", "/web_search", MergeClass::Scalar),
        ("copilot", "/model", MergeClass::Scalar),
        ("copilot", "/effortLevel", MergeClass::Scalar),
        ("copilot", "/theme", MergeClass::Scalar),
        ("copilot", "/defaultMode", MergeClass::Scalar),
        ("copilot", "/defaultPermissionMode", MergeClass::Scalar),
        (
            "copilot",
            "/permissions/disableBypassPermissionsMode",
            MergeClass::Scalar,
        ),
        ("copilot", "/allowedUrls", MergeClass::Set),
        ("copilot", "/deniedUrls", MergeClass::Set),
        ("copilot", "/disabledMcpServers", MergeClass::Set),
        ("copilot", "/disabledSkills", MergeClass::Set),
        ("copilot", "/askUser", MergeClass::Scalar),
        ("copilot", "/includeCoAuthoredBy", MergeClass::Scalar),
        ("copilot", "/respectGitignore", MergeClass::Scalar),
        ("copilot", "/renderMarkdown", MergeClass::Scalar),
        ("copilot", "/stream", MergeClass::Scalar),
        ("copilot", "/transcriptView", MergeClass::Scalar),
        ("copilot", "/mergeStrategy", MergeClass::Scalar),
        ("copilot", "/sandbox/enabled", MergeClass::Scalar),
        ("copilot", "/extraKnownMarketplaces/<name>", MergeClass::Map),
        ("copilot", "/editorMode", MergeClass::Scalar),
        ("copilot", "/autoTier", MergeClass::Scalar),
        ("copilot", "/banner", MergeClass::Scalar),
        ("copilot", "/keepAlive", MergeClass::Scalar),
        ("copilot", "/worktreeBaseRef", MergeClass::Scalar),
        ("copilot", "/autoUpdatesChannel", MergeClass::Scalar),
        ("copilot", "/logLevel", MergeClass::Scalar),
        ("copilot", "/stayInAutopilot", MergeClass::Scalar),
        ("copilot", "/continueOnAutoMode", MergeClass::Scalar),
        ("copilot", "/showReasoning", MergeClass::Scalar),
        ("copilot", "/experimental", MergeClass::Scalar),
        ("copilot", "/toolSearch", MergeClass::Scalar),
        ("copilot", "/connectors", MergeClass::Scalar),
        ("copilot", "/customAgents/defaultLocalOnly", MergeClass::Scalar),
        ("copilot", "/autoUpdate", MergeClass::Scalar),
        ("copilot", "/beep", MergeClass::Scalar),
        ("copilot", "/beepOnSchedule", MergeClass::Scalar),
        ("copilot", "/compactPaste", MergeClass::Scalar),
        ("copilot", "/copyOnSelect", MergeClass::Scalar),
        ("copilot", "/inlineImages", MergeClass::Scalar),
        ("copilot", "/mouse", MergeClass::Scalar),
        ("copilot", "/pinnedPrompts", MergeClass::Scalar),
        ("copilot", "/renderHexColors", MergeClass::Scalar),
        ("copilot", "/screenReader", MergeClass::Scalar),
        ("copilot", "/scrollbar", MergeClass::Scalar),
        ("copilot", "/shellShortcut", MergeClass::Scalar),
        ("copilot", "/showTimestamps", MergeClass::Scalar),
        ("copilot", "/showTipsOnStartup", MergeClass::Scalar),
        ("copilot", "/sidebar", MergeClass::Scalar),
        ("copilot", "/streamerMode", MergeClass::Scalar),
        ("copilot", "/terminalNotifications", MergeClass::Scalar),
        ("copilot", "/terminalProgress", MergeClass::Scalar),
        ("copilot", "/updateTerminalTitle", MergeClass::Scalar),
        ("copilot", "/taskbarPresence", MergeClass::Scalar),
        ("copilot", "/ide/autoConnect", MergeClass::Scalar),
        ("copilot", "/ide/openDiffOnEdit", MergeClass::Scalar),
        ("copilot", "/dynamicRetrieval/skills", MergeClass::Scalar),
        ("copilot", "/tabs/enabled", MergeClass::Scalar),
        (
            "copilot",
            "/subagents/agents/rubber-duck/autoInvoke",
            MergeClass::Scalar,
        ),
        ("copilot", "/subagents/disabledSubagents", MergeClass::Set),
        ("copilot", "/enabledMcpServers", MergeClass::Set),
        ("copilot", "/sandbox/allowBypass", MergeClass::Scalar),
        ("cursor", "/permissions/allow", MergeClass::Set),
        ("cursor", "/permissions/deny", MergeClass::Set),
        ("cursor", "/approvalMode", MergeClass::Scalar),
        ("cursor", "/sandbox/readBoundary", MergeClass::Scalar),
        ("cursor", "/editor/vimMode", MergeClass::Scalar),
        ("cursor", "/notifications", MergeClass::Scalar),
        ("cursor", "/hints", MergeClass::Scalar),
        ("cursor", "/rewind", MergeClass::Scalar),
        ("cursor", "/suggestNextPrompt", MergeClass::Scalar),
        ("cursor", "/maxMode", MergeClass::Scalar),
        ("cursor", "/display/showLineNumbers", MergeClass::Scalar),
        ("cursor", "/display/showThinkingBlocks", MergeClass::Scalar),
        ("cursor", "/display/showStatusIndicators", MergeClass::Scalar),
        ("cursor", "/display/showStatusLineRunningTime", MergeClass::Scalar),
        ("cursor", "/attribution/attributeCommitsToAgent", MergeClass::Scalar),
        ("cursor", "/attribution/attributePRsToAgent", MergeClass::Scalar),
        ("opencode", "/model", MergeClass::Scalar),
        ("opencode", "/small_model", MergeClass::Scalar),
        ("opencode", "/default_agent", MergeClass::Scalar),
        ("opencode", "/autoupdate", MergeClass::Scalar),
        ("opencode", "/snapshot", MergeClass::Scalar),
        ("opencode", "/compaction/auto", MergeClass::Scalar),
        ("opencode", "/compaction/prune", MergeClass::Scalar),
        ("opencode", "/permission", MergeClass::Scalar),
        ("opencode", "/permission/<tool>", MergeClass::Map),
        ("opencode", "/share", MergeClass::Scalar),
        ("qoder", "/model/name", MergeClass::Scalar),
        ("qoder", "/general/defaultPermissionMode", MergeClass::Scalar),
        ("qoder", "/general/vimMode", MergeClass::Scalar),
        ("qoder", "/general/enableAutoUpdate", MergeClass::Scalar),
        ("qoder", "/general/enableNotifications", MergeClass::Scalar),
        ("qoder", "/output/format", MergeClass::Scalar),
        ("qoder", "/ui/theme", MergeClass::Scalar),
        ("qoder", "/ui/hideTips", MergeClass::Scalar),
        ("qoder", "/ui/hideBanner", MergeClass::Scalar),
        ("qoder", "/ui/showLineNumbers", MergeClass::Scalar),
        ("qoder", "/ui/compactToolOutput", MergeClass::Scalar),
        ("qoder", "/permissions/allow", MergeClass::Set),
        ("qoder", "/permissions/ask", MergeClass::Set),
        ("qoder", "/permissions/deny", MergeClass::Set),
        ("qoder", "/security/disableYoloMode", MergeClass::Scalar),
        ("qoder", "/model/reasoningEffort", MergeClass::Scalar),
        ("antigravity", "/agentMode", MergeClass::Scalar),
        ("antigravity", "/toolPermission", MergeClass::Scalar),
        ("antigravity", "/artifactReviewPolicy", MergeClass::Scalar),
        ("antigravity", "/allowNonWorkspaceAccess", MergeClass::Scalar),
        ("antigravity", "/enableTerminalSandbox", MergeClass::Scalar),
        ("antigravity", "/permissions/allow", MergeClass::Set),
        ("antigravity", "/permissions/ask", MergeClass::Set),
        ("antigravity", "/permissions/deny", MergeClass::Set),
        ("antigravity", "/model", MergeClass::Scalar),
        ("antigravity", "/verbosity", MergeClass::Scalar),
        ("antigravity", "/queuedMessages", MergeClass::Scalar),
        ("antigravity", "/notifications", MergeClass::Scalar),
        ("antigravity", "/colorScheme", MergeClass::Scalar),
        ("antigravity", "/altScreenMode", MergeClass::Scalar),
        ("antigravity", "/editorMode", MergeClass::Scalar),
        ("antigravity", "/vimInsertFirst", MergeClass::Scalar),
        ("antigravity", "/runningLightSpeed", MergeClass::Scalar),
        ("antigravity", "/pickerGrouping", MergeClass::Scalar),
        ("antigravity", "/showTips", MergeClass::Scalar),
        ("antigravity", "/showFeedbackSurvey", MergeClass::Scalar),
        ("antigravity", "/hideActiveItems", MergeClass::Scalar),
        ("antigravity", "/copyOnSelect", MergeClass::Scalar),
        ("antigravity", "/clearScrollbackOnResize", MergeClass::Scalar),
        ("antigravity", "/useTerminalCursor", MergeClass::Scalar),
        ("antigravity", "/disableSlashCommands", MergeClass::Scalar),
        ("amp", "/amp.showCosts", MergeClass::Scalar),
        ("amp", "/amp.notifications.enabled", MergeClass::Scalar),
        ("amp", "/amp.terminal.copyOnSelect", MergeClass::Scalar),
        ("amp", "/amp.terminal.detailsExpandedByDefault", MergeClass::Scalar),
        ("amp", "/amp.thread.autoArchiveOnQuit", MergeClass::Scalar),
        ("amp", "/amp.updates.mode", MergeClass::Scalar),
        ("amp", "/amp.git.commit.ampThread.enabled", MergeClass::Scalar),
        ("amp", "/amp.git.commit.coauthor.enabled", MergeClass::Scalar),
        ("droid", "/sessionDefaultSettings/autonomyLevel", MergeClass::Scalar),
        ("droid", "/sessionDefaultSettings/interactionMode", MergeClass::Scalar),
        ("droid", "/sessionDefaultSettings/model", MergeClass::Scalar),
        ("droid", "/sessionDefaultSettings/reasoningEffort", MergeClass::Scalar),
        ("droid", "/outputStyle", MergeClass::Scalar),
        ("droid", "/diffMode", MergeClass::Scalar),
        ("droid", "/soundFocusMode", MergeClass::Scalar),
        ("droid", "/completionSoundFocusMode", MergeClass::Scalar),
        ("droid", "/awaitingInputSoundFocusMode", MergeClass::Scalar),
        ("droid", "/toolResultDisplay", MergeClass::Scalar),
        ("droid", "/logoAnimation", MergeClass::Scalar),
        ("droid", "/subagentAutonomyLevel", MergeClass::Scalar),
        ("droid", "/includeCoAuthoredByDroid", MergeClass::Scalar),
        ("droid", "/enableDroidShield", MergeClass::Scalar),
        ("droid", "/ideAutoConnect", MergeClass::Scalar),
        ("droid", "/showThinkingInMainView", MergeClass::Scalar),
        ("droid", "/streamAssistantResponses", MergeClass::Scalar),
        ("droid", "/vimMode", MergeClass::Scalar),
        ("droid", "/blockOnMcpLoad", MergeClass::Scalar),
        ("droid", "/showTokenUsageIndicator", MergeClass::Scalar),
        ("droid", "/overrideTerminalColors", MergeClass::Scalar),
        ("droid", "/nerdFont", MergeClass::Scalar),
        ("droid", "/disabledSkills", MergeClass::Set),
        ("droid", "/sandbox/enabled", MergeClass::Scalar),
        ("droid", "/sandbox/mode", MergeClass::Scalar),
        ("droid", "/sandbox/filesystem/denyRead", MergeClass::Set),
        ("droid", "/sandbox/filesystem/denyWrite", MergeClass::Set),
        ("droid", "/sandbox/filesystem/allowWrite", MergeClass::Set),
        ("droid", "/sandbox/network/allowedDomains", MergeClass::Set),
        ("droid", "/extraKnownMarketplaces/<name>", MergeClass::Map),
        ("kilo", "/model", MergeClass::Scalar),
        ("kilo", "/small_model", MergeClass::Scalar),
        ("kilo", "/default_agent", MergeClass::Scalar),
        ("kilo", "/reasoning_display", MergeClass::Scalar),
        ("kilo", "/terminal_command_display", MergeClass::Scalar),
        ("kilo", "/code_edit_display", MergeClass::Scalar),
        ("kilo", "/web_search", MergeClass::Scalar),
        ("kilo", "/hide_prompt_training_models", MergeClass::Scalar),
        ("kilo", "/privacy_mode", MergeClass::Scalar),
        ("kiro", "/chat.defaultModel", MergeClass::Scalar),
        ("kiro", "/chat.defaultAgent", MergeClass::Scalar),
        ("kiro", "/chat.outputStyle", MergeClass::Scalar),
        ("kiro", "/chat.agentEngine", MergeClass::Scalar),
        ("kiro", "/acp.agentEngine", MergeClass::Scalar),
        ("kiro", "/chat.notificationMethod", MergeClass::Scalar),
        ("kiro", "/chat.historyMode", MergeClass::Scalar),
        ("kiro", "/chat.screenReader", MergeClass::Scalar),
        ("kiro", "/chat.workflowLaunch", MergeClass::Scalar),
        ("kiro", "/chat.sessionDashboard.scope", MergeClass::Scalar),
        ("kiro", "/chat.disableGranularTrust", MergeClass::Scalar),
        ("kiro", "/chat.enableNotifications", MergeClass::Scalar),
        ("kiro", "/chat.greeting.enabled", MergeClass::Scalar),
        ("kiro", "/chat.disableWrap", MergeClass::Scalar),
        ("kiro", "/chat.disableAutoCompaction", MergeClass::Scalar),
        ("kiro", "/chat.enablePromptHints", MergeClass::Scalar),
        ("kiro", "/chat.autoExpandToolOutput", MergeClass::Scalar),
        ("kiro", "/chat.sessionDashboard.indexResponses", MergeClass::Scalar),
        ("kiro", "/chat.allowAnimations", MergeClass::Scalar),
        ("kiro", "/chat.allowAsciiArt", MergeClass::Scalar),
        ("kiro", "/chat.allowIcons", MergeClass::Scalar),
        ("kiro", "/chat.preserveScrollback", MergeClass::Scalar),
        ("kiro", "/chat.terminalTitle", MergeClass::Scalar),
        ("kiro", "/chat.showThinking", MergeClass::Scalar),
        ("kiro", "/chat.enableKnowledge", MergeClass::Scalar),
        ("kiro", "/chat.enableCodeIntelligence", MergeClass::Scalar),
        ("kiro", "/chat.enableThinking", MergeClass::Scalar),
        ("kiro", "/chat.enableWorkflows", MergeClass::Scalar),
        ("kiro", "/toolSearch.enabled", MergeClass::Scalar),
        ("kiro", "/chat.enableAutoAgentUpgrade", MergeClass::Scalar),
        ("kiro", "/app.disableAutoupdates", MergeClass::Scalar),
        ("kiro", "/app.warnOnElevatedPrivileges", MergeClass::Scalar),
        ("kiro", "/knowledge.defaultIncludePatterns", MergeClass::Set),
        ("kiro", "/knowledge.defaultExcludePatterns", MergeClass::Set),
        ("gemini", "/general/defaultApprovalMode", MergeClass::Scalar),
        ("gemini", "/general/notificationMethod", MergeClass::Scalar),
        ("gemini", "/general/preferredEditor", MergeClass::Scalar),
        ("gemini", "/general/vimMode", MergeClass::Scalar),
        ("gemini", "/general/enableNotifications", MergeClass::Scalar),
        ("gemini", "/general/checkpointing/enabled", MergeClass::Scalar),
        ("gemini", "/general/plan/enabled", MergeClass::Scalar),
        ("gemini", "/general/enableAutoUpdate", MergeClass::Scalar),
        ("gemini", "/general/enableAutoUpdateNotification", MergeClass::Scalar),
        ("gemini", "/model/name", MergeClass::Scalar),
        ("gemini", "/output/format", MergeClass::Scalar),
        ("gemini", "/ui/inlineThinkingMode", MergeClass::Scalar),
        ("gemini", "/ui/loadingPhrases", MergeClass::Scalar),
        ("gemini", "/ui/errorVerbosity", MergeClass::Scalar),
        ("gemini", "/ui/hideTips", MergeClass::Scalar),
        ("gemini", "/ui/hideBanner", MergeClass::Scalar),
        ("gemini", "/ui/showLineNumbers", MergeClass::Scalar),
        ("gemini", "/ui/compactToolOutput", MergeClass::Scalar),
        ("gemini", "/tools/allowed", MergeClass::Set),
        ("gemini", "/tools/confirmationRequired", MergeClass::Set),
        ("gemini", "/tools/sandboxAllowedPaths", MergeClass::Set),
        ("gemini", "/tools/exclude", MergeClass::Set),
        ("gemini", "/tools/sandboxNetworkAccess", MergeClass::Scalar),
        (
            "gemini",
            "/tools/shell/backgroundCompletionBehavior",
            MergeClass::Scalar,
        ),
        ("gemini", "/security/toolSandboxing", MergeClass::Scalar),
        ("gemini", "/security/disableYoloMode", MergeClass::Scalar),
        ("gemini", "/security/disableAlwaysAllow", MergeClass::Scalar),
        ("gemini", "/security/enablePermanentToolApproval", MergeClass::Scalar),
        ("gemini", "/security/autoAddToPolicyByDefault", MergeClass::Scalar),
        ("gemini", "/security/blockGitExtensions", MergeClass::Scalar),
        ("gemini", "/skills/disabled", MergeClass::Set),
        ("gemini", "/context/fileFiltering/respectGitIgnore", MergeClass::Scalar),
        (
            "gemini",
            "/context/fileFiltering/respectGeminiIgnore",
            MergeClass::Scalar,
        ),
        (
            "warp",
            "/agents/warp_agent/other/thinking_display_mode",
            MergeClass::Scalar,
        ),
        (
            "warp",
            "/agents/warp_agent/other/open_conversation_layout_preference",
            MergeClass::Scalar,
        ),
        (
            "warp",
            "/agents/warp_agent/other/long_running_command_submission_mode",
            MergeClass::Scalar,
        ),
        (
            "warp",
            "/agents/warp_agent/other/orchestration_message_display_mode",
            MergeClass::Scalar,
        ),
        (
            "warp",
            "/agents/warp_agent/other/default_prompt_submission_mode",
            MergeClass::Scalar,
        ),
        ("warp", "/general/default_session_mode", MergeClass::Scalar),
        (
            "warp",
            "/agents/warp_agent/other/auto_approve_bypasses_command_denylist",
            MergeClass::Scalar,
        ),
        (
            "warp",
            "/agents/warp_agent/other/cloud_agent_computer_use_enabled",
            MergeClass::Scalar,
        ),
        ("warp", "/agents/mcp_servers/file_based_mcp_enabled", MergeClass::Scalar),
        (
            "warp",
            "/agents/warp_agent/other/agent_attribution_enabled",
            MergeClass::Scalar,
        ),
        (
            "warp",
            "/agents/warp_agent/other/show_agent_notifications",
            MergeClass::Scalar,
        ),
        (
            "warp",
            "/agents/warp_agent/other/show_conversation_history",
            MergeClass::Scalar,
        ),
        (
            "warp",
            "/agents/warp_agent/other/should_show_oz_updates_in_zero_state",
            MergeClass::Scalar,
        ),
        (
            "warp",
            "/agents/warp_agent/other/should_render_use_agent_toolbar_for_user_commands",
            MergeClass::Scalar,
        ),
        ("warp", "/agents/warp_agent/is_any_ai_enabled", MergeClass::Scalar),
        ("warp", "/agents/warp_agent/active_ai/enabled", MergeClass::Scalar),
        (
            "warp",
            "/agents/warp_agent/active_ai/code_suggestions_enabled",
            MergeClass::Scalar,
        ),
        (
            "warp",
            "/agents/warp_agent/active_ai/intelligent_autosuggestions_enabled",
            MergeClass::Scalar,
        ),
        (
            "warp",
            "/agents/warp_agent/active_ai/agent_mode_query_suggestions_enabled",
            MergeClass::Scalar,
        ),
        (
            "warp",
            "/agents/warp_agent/active_ai/git_operations_autogen_enabled",
            MergeClass::Scalar,
        ),
        (
            "warp",
            "/agents/warp_agent/active_ai/rule_suggestions_enabled",
            MergeClass::Scalar,
        ),
        (
            "warp",
            "/agents/warp_agent/active_ai/shared_block_title_generation_enabled",
            MergeClass::Scalar,
        ),
        (
            "warp",
            "/agents/warp_agent/input/ai_auto_detection_enabled",
            MergeClass::Scalar,
        ),
        (
            "warp",
            "/agents/warp_agent/input/nld_in_terminal_enabled",
            MergeClass::Scalar,
        ),
        (
            "warp",
            "/agents/warp_agent/input/show_model_selectors_in_prompt",
            MergeClass::Scalar,
        ),
        ("warp", "/agents/warp_agent/input/show_agent_tips", MergeClass::Scalar),
        (
            "warp",
            "/agents/warp_agent/input/include_agent_commands_in_history",
            MergeClass::Scalar,
        ),
        ("warp", "/agents/knowledge/rules_enabled", MergeClass::Scalar),
        (
            "warp",
            "/agents/knowledge/warp_drive_context_enabled",
            MergeClass::Scalar,
        ),
    ];

    fn settings_rows_of(name: &str) -> &'static [SettingsField] {
        use crate::install::client_target::ClientTarget;
        ClientTarget::ALL
            .iter()
            .map(|c| c.vendor())
            .find(|v| v.name() == name)
            .map(|v| v.settings_fields())
            .unwrap_or_else(|| panic!("no vendor {name}"))
    }

    #[test]
    fn the_thirteen_tables_hold_316_rows() {
        let total: usize = SETTINGS_HOSTS.iter().map(|n| settings_rows_of(n).len()).sum();
        assert_eq!(total, 316);
        assert_eq!(SETTINGS_GOLDEN.len(), 316);
    }

    /// The per-harness field table on `settings.md` lists exactly the
    /// `(vendor, pointer)` pairs `settings_fields()` declares, both ways. It
    /// reads the table rows, since a lowercase-token filter misses a key like
    /// `/permissions/allow`.
    #[test]
    fn settings_docs_table_matches_registry() {
        use std::collections::BTreeSet;
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/src/content/docs/settings.md");
        let doc =
            std::fs::read_to_string(path).expect("docs/src/content/docs/settings.md exists (doc/registry parity)");
        let mut documented = BTreeSet::new();
        for line in doc.lines().map(str::trim).filter(|l| l.starts_with('|')) {
            let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
            let [vendor, pointer, ..] = cells.as_slice() else {
                continue;
            };
            let vendor = vendor.trim_matches('`').to_ascii_lowercase();
            if !SETTINGS_HOSTS.contains(&vendor.as_str()) {
                continue;
            }
            if let Some(p) = pointer.strip_prefix('`').and_then(|p| p.strip_suffix('`'))
                && !p.contains('`')
            {
                documented.insert((vendor, p.to_string()));
            }
        }
        let registry: BTreeSet<(String, String)> = SETTINGS_HOSTS
            .iter()
            .flat_map(|n| {
                settings_rows_of(n)
                    .iter()
                    .map(move |f| (n.to_string(), f.pointer.to_string()))
            })
            .collect();
        let missing: Vec<_> = registry.difference(&documented).collect();
        let extra: Vec<_> = documented.difference(&registry).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "settings.md field table drifted: missing from the page {missing:?}, not in the registry {extra:?}"
        );
        assert_eq!(
            documented.len(),
            316,
            "settings.md field table holds one row per registry key"
        );
    }

    #[test]
    fn every_settings_row_keeps_its_pointer_and_class() {
        for name in SETTINGS_HOSTS {
            let actual: Vec<(&str, MergeClass)> = settings_rows_of(name).iter().map(|f| (f.pointer, f.merge)).collect();
            let golden: Vec<(&str, MergeClass)> = SETTINGS_GOLDEN
                .iter()
                .filter(|(v, _, _)| *v == name)
                .map(|(_, p, c)| (*p, *c))
                .collect();
            assert_eq!(actual, golden, "{name}");
        }
    }

    /// Top-level keys another artifact kind owns (hooks, MCP, rules, plugins)
    /// on every host.
    const OWNED_BY_ANOTHER_KIND: [&str; 7] = [
        "claudeMdExcludes",
        "hooks",
        "mcpServers",
        "mcp",
        "mcp_servers",
        "instructions",
        "enabledPlugins",
    ];

    /// The keys another kind owns in `vendor`'s files: the shared list, plus the
    /// top-level key its MCP entry and its hook splice sit under.
    fn keys_another_kind_owns(vendor: &dyn Vendor) -> Vec<String> {
        let probe = crate::oci::mcp::McpDescriptor::from_toml_str(
            "description = \"d\"\n[server]\ntransport = \"stdio\"\ncommand = \"grim\"",
        )
        .unwrap();
        let mut owned: Vec<String> = OWNED_BY_ANOTHER_KIND.iter().map(|key| key.to_string()).collect();
        for scope in [ConfigScope::Project, ConfigScope::Global] {
            if let Some((pointer, _)) = vendor.mcp_entry(scope, "probe", &probe) {
                owned.extend(pointer.trim_start_matches('/').split('/').next().map(str::to_string));
            }
        }
        owned.extend(vendor.hook_splice_shape().map(|shape| shape.container.to_string()));
        owned
    }

    /// Whether settings `pointer` reaches a key in `owned`. A flat dotted key
    /// (Amp's `amp.mcpServers`) is one segment, so each dotted piece of the
    /// top-level key is checked against the shared list and each dotted prefix
    /// against `owned`.
    fn reaches_owned_key(pointer: &str, owned: &[String]) -> bool {
        let top = pointer.trim_start_matches('/').split('/').next().unwrap_or("");
        let pieces: Vec<&str> = top.split('.').collect();
        pieces.iter().any(|piece| OWNED_BY_ANOTHER_KIND.contains(piece))
            || (1..=pieces.len()).any(|n| owned.contains(&pieces[..n].join(".")))
    }

    /// Keys another artifact kind owns are never settings rows, whatever the
    /// vendor spells them as.
    #[test]
    fn no_settings_row_reaches_a_key_another_kind_owns() {
        use crate::install::client_target::ClientTarget;
        for vendor in ClientTarget::ALL.iter().map(|c| c.vendor()) {
            let owned = keys_another_kind_owns(vendor);
            for field in vendor.settings_fields() {
                assert!(
                    !reaches_owned_key(field.pointer, &owned),
                    "{} {}",
                    vendor.name(),
                    field.pointer
                );
            }
        }
    }

    /// The guard above passes while the tables are clean, so it is only proof
    /// while it flags the keys it exists for, by each of its two halves.
    #[test]
    fn the_owned_key_guard_flags_hook_and_mcp_containers() {
        use crate::install::client_target::ClientTarget;
        let owned = |client: ClientTarget| keys_another_kind_owns(client.vendor());
        assert!(reaches_owned_key("/amp.mcpServers", &owned(ClientTarget::Amp)));
        assert!(reaches_owned_key("/amp.mcpServers/x", &owned(ClientTarget::Amp)));
        assert!(reaches_owned_key("/hooks", &owned(ClientTarget::Claude)));
        assert!(reaches_owned_key("/hooks/PreToolUse", &owned(ClientTarget::Claude)));
        assert!(reaches_owned_key("/mcp", &owned(ClientTarget::OpenCode)));
        // A dotted piece naming a shared key, with no host container to match.
        assert!(reaches_owned_key("/amp.mcpServers", &[]));
        // A host's own dotted container whose pieces the shared list lacks.
        assert!(reaches_owned_key("/team.tools.x", &["team.tools".to_string()]));
        // Positive controls hold only beside a key that passes.
        assert!(!reaches_owned_key("/amp.mcpServersX", &owned(ClientTarget::Amp)));
        assert!(!reaches_owned_key("/model", &owned(ClientTarget::Claude)));
        assert!(!reaches_owned_key("/team.toolsx", &["team.tools".to_string()]));
    }

    /// The trait arm names the vendor variable itself, which no unit can set:
    /// the environment is process-wide. Read the arm's source instead, so a
    /// rename or a swapped variable fails here.
    #[test]
    fn each_settings_path_arm_reads_its_own_vendor_variable() {
        fn arm(source: &'static str) -> &'static str {
            let start = source.find("fn settings_config_path(&self").expect("trait arm");
            let rest = &source[start..];
            &rest[..rest.find("\n    }\n").expect("arm end")]
        }
        fn hook_arm(source: &'static str) -> &'static str {
            let start = source.find("fn hook_config_path(&self").expect("hook arm");
            let rest = &source[start..];
            &rest[..rest.find("\n    }\n").expect("arm end")]
        }
        assert!(arm(include_str!("vendor_codex.rs")).contains("env_dir(\"CODEX_HOME\")"));
        assert!(arm(include_str!("vendor_copilot.rs")).contains("env_dir(\"COPILOT_HOME\")"));
        assert!(arm(include_str!("vendor_cursor.rs")).contains("env_dir(\"CURSOR_CONFIG_DIR\")"));
        assert!(arm(include_str!("vendor_claude.rs")).contains("global_root(config_dir_override(), home_dir())"));
        // Qoder's settings arm hands off to the hook arm, which names the variable.
        let qoder = include_str!("vendor_qoder.rs");
        assert!(arm(qoder).contains("self.hook_config_path("));
        assert!(hook_arm(qoder).contains("env_dir(\"QODER_CONFIG_DIR\")"));
        // OpenCode resolves through the shared config path, which owns the variable.
        assert!(arm(include_str!("vendor_opencode.rs")).contains("opencode_config::config_path_for_scope("));
        assert!(include_str!("opencode_config.rs").contains("env_dir(\"OPENCODE_CONFIG\")"));
        // Amp and Antigravity name no variable: XDG and the home directory.
        assert!(arm(include_str!("vendor_amp.rs")).contains("amp_root(xdg_config_dir())"));
        assert!(arm(include_str!("vendor_antigravity.rs")).contains("antigravity_cli_root(home_dir())"));
        // Droid names no variable and Kilo only XDG's.
        assert!(arm(include_str!("vendor_droid.rs")).contains("droid_root(home_dir())"));
        assert!(arm(include_str!("vendor_kilo.rs")).contains("kilo_config_root(xdg_config_dir())"));
        // Kiro's variable replaces its root, Gemini's the home its root sits in.
        assert!(arm(include_str!("vendor_kiro.rs")).contains("kiro_root(env_dir(\"KIRO_HOME\"), home_dir())"));
        assert!(
            arm(include_str!("vendor_gemini.rs")).contains("gemini_root(env_dir(\"GEMINI_CLI_HOME\"), home_dir())")
        );
        // Warp names no variable: the home on macOS, XDG on Linux.
        assert!(
            arm(include_str!("vendor_warp.rs"))
                .contains("settings_root(settings_root_kind(), home_dir(), xdg_config_dir())")
        );
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

    // ── The hook seam ──────────────────────────────────────────────────────
    //

    /// A [`RootToken`] with a known value, for a byte-exact assertion.
    ///
    /// The real mint is `hook_dispatch::root_token`'s HMAC under the machine-local
    /// key, which is random per `$GRIM_HOME` and therefore unusable in a
    /// byte-exact expectation — and the type has **no** `new(&str)`, deliberately:
    /// a token anything could build from a string would be as forgeable as the
    /// path it replaced.
    ///
    /// This used to go through `serde_json::from_value`, on the reasoning that
    /// `Deserialize` was "the dispatch table's own read path". That was the hole:
    /// serde gives a private-field newtype a transparent deserializer, so the
    /// route was open to *any* caller, not just the table — which falsified the
    /// safety claim on [`Vendor::hook_registration`]. `RootToken` no longer
    /// derives `Deserialize` (the table reads its keys through a field-scoped
    /// serde module instead), and tests use the `#[cfg(test)]` constructor, which
    /// does not exist in the shipped binary.
    fn token(hex: &str) -> RootToken {
        RootToken::for_test(hex)
    }

    #[test]
    fn classify_matcher_admits_exactly_the_three_translatable_forms() {
        // C-025's table. Only the whole-string `*` is match-all; an exact name is
        // one with no glob/regex metacharacter; an alternation is exact names
        // joined by `|`; everything else declines rather than approximating.
        assert_eq!(classify_matcher(None), MatcherForm::All);
        assert_eq!(classify_matcher(Some("*")), MatcherForm::All);
        assert_eq!(classify_matcher(Some("")), MatcherForm::Empty);
        assert_eq!(classify_matcher(Some("Bash")), MatcherForm::ExactOrAlternation);
        assert_eq!(classify_matcher(Some("Bash|Edit")), MatcherForm::ExactOrAlternation);
        assert_eq!(
            classify_matcher(Some("mcp__server__tool")),
            MatcherForm::ExactOrAlternation
        );
        // A prefix glob is NOT a prefix glob on any v1 client — `Ba*` is the regex
        // `B` + zero-or-more `a` on claude and codex, and neither a glob nor
        // match-all on copilot.
        assert_eq!(classify_matcher(Some("Ba*")), MatcherForm::NotTranslatable);
        assert_eq!(classify_matcher(Some("Bas?")), MatcherForm::NotTranslatable);
        // `.` passes C-018's charset and is a regex any-character — the sharp case.
        assert_eq!(classify_matcher(Some("Read.md")), MatcherForm::NotTranslatable);
        // An empty alternative matches EVERYTHING as a regex, so an alternation
        // carrying one is over-broad, not merely lossy.
        for over_broad in ["Bash|", "|Bash", "Bash||Edit"] {
            assert_eq!(
                classify_matcher(Some(over_broad)),
                MatcherForm::NotTranslatable,
                "{over_broad} has an empty alternative"
            );
        }
        // A character C-018 never admitted (a `hook.toml` on disk is not bound by
        // the build-time charset) declines instead of reaching a matcher engine.
        assert_eq!(classify_matcher(Some("$(id)")), MatcherForm::NotTranslatable);
    }

    #[test]
    fn decision_k_predicate_follows_the_documented_rule_table() {
        // Every uncertainty resolves to `true`: a false `true` costs one declined
        // mutator with a legible reason, a false `false` ships the command-string
        // rewrite the ADR refused.
        assert!(matcher_may_select_shell_command_tool("claude", None), "All");
        assert!(matcher_may_select_shell_command_tool("claude", Some("*")), "All");
        assert!(matcher_may_select_shell_command_tool("claude", Some("")), "Empty");
        assert!(
            matcher_may_select_shell_command_tool("claude", Some("Ba*")),
            "NotTranslatable"
        );

        // ExactOrAlternation: true iff an alternative could NAME a roster tool.
        assert!(matcher_may_select_shell_command_tool("claude", Some("Bash")));
        assert!(matcher_may_select_shell_command_tool("claude", Some("Edit|Bash")));
        assert!(!matcher_may_select_shell_command_tool("claude", Some("Edit|Write")));

        // Prefix-aware, because claude/codex are start-anchored but TAIL-OPEN:
        // the matcher `Ba` fires on the tool `Bash`.
        assert!(matcher_may_select_shell_command_tool("claude", Some("Ba")));
        assert!(matcher_may_select_shell_command_tool("codex", Some("B")));
        // …and NOT the other direction: a longer name is not selected by naming
        // more than the tool is called. `BashOutput` names claude's real
        // `BashOutput` tool, which is not a shell-command-string tool.
        assert!(!matcher_may_select_shell_command_tool("claude", Some("BashOutput")));

        // Case-insensitive, because copilot's PascalCase dialect matches literal
        // names that way — `bash` fires on `Bash` there.
        assert!(matcher_may_select_shell_command_tool("copilot", Some("bash")));
        assert!(matcher_may_select_shell_command_tool("copilot", Some("BASH")));

        // A client with no roster row contributes nothing for a NAMED matcher —
        // and still answers `true` for the three unconditional forms, which is
        // what keeps an un-updated roster from silently admitting a match-all
        // mutator.
        assert!(!matcher_may_select_shell_command_tool("cursor", Some("Bash")));
        assert!(matcher_may_select_shell_command_tool("cursor", None));
    }

    fn observer_entry(matcher: Option<&str>) -> HookEntry {
        HookEntry {
            id: "h".to_owned(),
            event: Some(CanonicalEvent::PreToolUse),
            tier: HookTier::Observer,
            matcher: matcher.map(str::to_owned),
            handler: crate::oci::hook::HookHandler::Command("./guard.sh".to_owned()),
            timeout: Some(5),
            payload: None,
            policy: None,
            vendor: std::collections::BTreeMap::new(),
        }
    }

    const ROOT: &str = "0123456789abcdef0123456789abcdef";

    fn registered(client: &str, launcher: &str, host_os: HostOs) -> Result<HookRegistration, HookDecline> {
        let target = crate::install::client_target::ClientTarget::ALL
            .into_iter()
            .find(|c| c.vendor().name() == client)
            .expect("a known client");
        target.vendor().hook_registration(
            &observer_entry(None),
            CanonicalEvent::PreToolUse,
            Path::new(launcher),
            &token(ROOT),
            host_os,
        )
    }

    fn shell(text: &str) -> HookCommand {
        HookCommand::Shell(text.to_owned())
    }

    /// Byte-for-byte registrations per client, host OS and path shape. Codex
    /// hashes the raw command text for its trust record, so a changed byte
    /// un-trusts an approved hook.
    #[test]
    fn hook_registration_goldens() {
        let unix = HostOs::Unix;
        let win = HostOs::Windows;
        let bare = "/home/u/.grimoire/hooks/grim-hook";
        let quoted = "/home/u/it's a dir/hooks/grim-hook";
        let win_bare = r"C:\Users\u\.grimoire\hooks\grim-hook.exe";
        let win_quoted = r"C:\Users\Jo Doe\.grimoire\hooks\grim-hook.exe";
        let ops = |client: &str| format!("{client} PreToolUse {ROOT}");

        // (client, launcher, host, command, shell)
        let cases: Vec<(&str, &str, HostOs, HookCommand, Option<HookShell>)> = vec![
            ("claude", bare, unix, shell(&format!("{bare} {}", ops("claude"))), None),
            ("qoder", bare, unix, shell(&format!("{bare} {}", ops("qoder"))), None),
            ("codex", bare, unix, shell(&format!("{bare} {}", ops("codex"))), None),
            (
                "copilot",
                bare,
                unix,
                HookCommand::Argv(vec![bare.into(), "copilot".into(), "PreToolUse".into(), ROOT.into()]),
                None,
            ),
            (
                "claude",
                quoted,
                unix,
                shell(&format!("'/home/u/it'\\''s a dir/hooks/grim-hook' {}", ops("claude"))),
                None,
            ),
            (
                "codex",
                quoted,
                unix,
                shell(&format!("'/home/u/it'\\''s a dir/hooks/grim-hook' {}", ops("codex"))),
                None,
            ),
            (
                "copilot",
                quoted,
                unix,
                HookCommand::Argv(vec![quoted.into(), "copilot".into(), "PreToolUse".into(), ROOT.into()]),
                None,
            ),
            (
                "claude",
                win_bare,
                win,
                shell(&format!("C:/Users/u/.grimoire/hooks/grim-hook.exe {}", ops("claude"))),
                None,
            ),
            (
                "codex",
                win_bare,
                win,
                shell(&format!("{win_bare} {}", ops("codex"))),
                None,
            ),
            (
                "copilot",
                win_bare,
                win,
                HookCommand::Argv(vec![
                    win_bare.into(),
                    "copilot".into(),
                    "PreToolUse".into(),
                    ROOT.into(),
                ]),
                None,
            ),
            (
                "claude",
                win_quoted,
                win,
                shell(&format!("& '{win_quoted}' {}", ops("claude"))),
                Some(HookShell::PowerShell),
            ),
            (
                "copilot",
                win_quoted,
                win,
                HookCommand::Argv(vec![
                    win_quoted.into(),
                    "copilot".into(),
                    "PreToolUse".into(),
                    ROOT.into(),
                ]),
                None,
            ),
        ];
        for (client, launcher, host_os, command, shell) in cases {
            let got = registered(client, launcher, host_os).unwrap_or_else(|d| panic!("{client} {launcher}: {d}"));
            assert_eq!(got.command, command, "{client} {launcher} {host_os:?}");
            assert_eq!(got.shell, shell, "{client} {launcher} {host_os:?}");
        }
        // Codex's `cmd /C` has no escape for a quote or a space, so a launcher
        // under such a path is refused instead of registered ambiguously.
        assert_eq!(
            registered("codex", win_quoted, win),
            Err(HookDecline::LauncherPathUnquotable)
        );
    }

    /// Nothing from the manifest entry reaches the command: a hostile id,
    /// handler and matcher yield the bytes a plain one does.
    #[test]
    fn hook_registration_command_ignores_every_entry_field() {
        let plain = observer_entry(Some("Bash"));
        let mut hostile = observer_entry(Some("Bash"));
        hostile.id = "$(touch /tmp/pwned)".to_owned();
        hostile.handler = crate::oci::hook::HookHandler::Command("'; rm -rf / #".to_owned());
        for client in ["claude", "codex", "copilot"] {
            let vendor = crate::install::client_target::ClientTarget::ALL
                .into_iter()
                .find(|c| c.vendor().name() == client)
                .expect("a known client")
                .vendor();
            let build = |entry: &HookEntry| {
                vendor
                    .hook_registration(
                        entry,
                        CanonicalEvent::PreToolUse,
                        Path::new("/home/u/.grimoire/hooks/grim-hook"),
                        &token(ROOT),
                        HostOs::Unix,
                    )
                    .expect("registers")
                    .command
            };
            assert_eq!(build(&plain), build(&hostile), "{client}");
        }
    }

    #[test]
    fn hook_registration_declines_every_client_without_a_hook_surface() {
        use crate::install::client_target::ClientTarget;

        // 15 of 18 clients, through the fail-safe gate that must answer before
        // anything else is consulted — including `hook_tier_support`, whose table
        // lookup is a WP-A stub. A forgotten vendor fails SAFE, which is the whole
        // reason `hook_surface` is opt-in rather than a `kind_support` arm.
        let entry = HookEntry {
            id: "h".to_owned(),
            event: Some(CanonicalEvent::PreToolUse),
            tier: HookTier::Observer,
            matcher: Some("Bash".to_owned()),
            handler: crate::oci::hook::HookHandler::Command("./guard.sh".to_owned()),
            timeout: Some(5),
            payload: None,
            policy: None,
            vendor: std::collections::BTreeMap::new(),
        };
        for client in ClientTarget::ALL {
            let vendor = client.vendor();
            if vendor.hook_surface().is_some() {
                continue;
            }
            assert_eq!(
                vendor.hook_registration(
                    &entry,
                    CanonicalEvent::PreToolUse,
                    Path::new("/home/u/.grimoire/hooks/grim-hook"),
                    &token("0123456789abcdef0123456789abcdef"),
                    HostOs::Unix,
                ),
                Err(HookDecline::NoSurface),
                "{client} has no hook surface and must decline"
            );
            for tier in HookTier::ALL {
                for event in CanonicalEvent::ALL {
                    assert_eq!(
                        vendor.hook_tier_support(tier, event),
                        KindSupport::Declined,
                        "{client} hosts no hook, so no tier is honourable at {event}"
                    );
                }
            }
        }
    }

    /// A vendor opts into settings by listing keys, so one that lists none
    /// answers the default file and format and gains no surface.
    #[test]
    fn a_vendor_listing_no_settings_keys_answers_the_defaults() {
        use crate::install::client_target::ClientTarget;

        let workspace = Path::new("/nonexistent/ws");
        let unlisted: Vec<_> = ClientTarget::ALL
            .into_iter()
            .filter(|client| client.vendor().settings_fields().is_empty())
            .collect();
        assert!(!unlisted.is_empty(), "the check needs a vendor to judge");
        for client in unlisted {
            let vendor = client.vendor();
            assert_eq!(vendor.settings_config_format(), McpConfigFormat::Json, "{client}");
            for scope in [ConfigScope::Project, ConfigScope::Global] {
                assert_eq!(
                    vendor.settings_config_path(workspace, scope),
                    None,
                    "{client} at {scope:?}"
                );
            }
        }
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
