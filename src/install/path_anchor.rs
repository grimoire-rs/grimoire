// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Anchor-relativized install paths: store each materialized target as a
//! `(anchor, relative)` pair instead of an absolute path so the install
//! state is portable across machines (shared `$GRIM_HOME`, devcontainers
//! with a different `$HOME`).
//!
//! A [`PathAnchor`] names a logical root (the workspace, a vendor's native
//! config dir, `$GRIM_HOME`). [`AnchorRoots`] resolves every anchor's
//! concrete on-disk root **once** at scope-resolution time, so
//! [`PathAnchor::root`] is a pure table lookup (no ambient env at resolve
//! time → unit-testable without env). An [`AnchoredPath`] re-joins the two
//! through a two-layer containment guard ([`AnchoredPath::resolve`]) that
//! never lets a tampered `relative` escape its anchor root.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::config::scope::ConfigScope;
use crate::context::Context;
use crate::install::client_target::ClientTarget;
use crate::install::vendor::{KindSupport, env_dir, global_skills_root, home_dir};
use crate::install::{
    opencode_config, vendor_amp, vendor_antigravity, vendor_claude, vendor_cline, vendor_codex, vendor_copilot,
    vendor_cursor, vendor_droid, vendor_gemini, vendor_junie, vendor_kilo, vendor_kiro, vendor_openclaw,
    vendor_opencode, vendor_warp, vendor_zed,
};
use crate::oci::ArtifactKind;

/// How a row reads the environment. Injected rather than called ambiently so
/// [`AnchorRoots::resolve_from`] is a pure function of its arguments and the
/// table's wiring can be asserted without mutating process env —
/// `std::env::set_var` is `unsafe` under Rust 2024 and this crate forbids
/// `unsafe_code` (the `vendor_codex` / `vendor_zed` precedent).
type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<PathBuf>;

/// One [`VENDOR_ROOTS`] row: the vendor's name — which is also its
/// `<name>-root` anchor tag — and the resolver for its root, as a pure
/// function of the injected environment and home directory.
type VendorRootRow = (&'static str, fn(EnvLookup<'_>, Option<PathBuf>) -> Option<PathBuf>);

/// Every vendor whose global anchor root is a plain, self-contained
/// `Option<PathBuf>`, keyed by the vendor's
/// [`name`](crate::install::vendor::Vendor::name).
///
/// **A row's name is an on-disk contract**: the serialized anchor tag is
/// `<name>-root`, so `("cursor", …)` *is* the `cursor-root` that shipped
/// `state.json` files already carry. Rows may be appended; a name may never be
/// changed or removed (Principle 9).
///
/// Each resolver runs exactly once, inside [`AnchorRoots::resolve`] — the
/// single place ambient env is read — so [`PathAnchor::root`] stays a pure
/// lookup with no I/O and no env.
///
/// **Adding a vendor** costs one row here plus its `(client, kind)` arms in
/// [`candidate_anchors`]: no enum variant, no `AnchorRoots` field, no fixture
/// churn anywhere in the tree.
///
/// **Never add a row whose `<name>-root` tag collides with a fixed one.** The
/// fixed arms of [`PathAnchor::from_serde_tag`] are matched first, so a
/// colliding row would be written under its own name and silently read back as
/// the *other* anchor, resolving against a different root — e.g. a row named
/// `open-code`. `vendor_root_rows_and_reachable_vendor_roots_agree` is the
/// guard; it fails on any row that does not read back as itself.
///
/// A row named `opencode` is excluded for a second, independent reason:
/// OpenCode's skills-derived config root is already addressed by
/// [`PathAnchor::OpenCodeRoot`], so such a row would be a second anchor for
/// one root — two spellings of the same location in `state.json`, which the
/// reaper and the prune refcount treat as distinct outputs. (It would
/// additionally render identically to `OpenCodeRoot` under `Display`.)
/// The [`OPENCODE_CONFIG_ROW`] row is *not* that: it resolves the directory
/// holding OpenCode's config **file**, which `$OPENCODE_CONFIG` /
/// `$OPENCODE_CONFIG_DIR` can move independently of the skills root.
const VENDOR_ROOTS: &[VendorRootRow] = &[
    ("claude", |env, home| {
        vendor_claude::global_root(env("CLAUDE_CONFIG_DIR"), home)
    }),
    ("copilot", |env, home| {
        vendor_copilot::global_native_root(env("COPILOT_HOME"), home)
    }),
    ("codex", |env, home| vendor_codex::codex_root(env("CODEX_HOME"), home)),
    ("cursor", |_, home| vendor_cursor::cursor_root(home)),
    ("kiro", |env, home| vendor_kiro::kiro_root(env("KIRO_HOME"), home)),
    ("junie", |_, home| vendor_junie::junie_root(home)),
    ("gemini", |env, home| {
        vendor_gemini::gemini_root(env("GEMINI_CLI_HOME"), home)
    }),
    // Zed and Amp root under an XDG config dir, not `$HOME` directly. Note
    // `vendor_zed::zed_root` reads `$HOME` / `%APPDATA%` itself on its macOS
    // and Windows arms, so only the Linux/FreeBSD arm is a pure function of
    // what is injected here — see `every_vendor_root_row_resolves_to_its_own_root`.
    ("zed", |env, home| vendor_zed::zed_root(xdg_config_from(env, home))),
    ("amp", |env, home| vendor_amp::amp_root(xdg_config_from(env, home))),
    // `~/.gemini/config` — nested under, and distinct from, the `gemini`
    // row's `~/.gemini`. Each client's candidate set holds only its own
    // root, so the nesting never cross-classifies.
    ("antigravity", |_, home| vendor_antigravity::antigravity_root(home)),
    // Wave-2 batch. `goose` deliberately has NO row: it renders into the
    // shared pool at both scopes, so every path it writes anchors at
    // `AgentsSkills` and a vendor root would never be reachable.
    ("cline", |_, home| vendor_cline::cline_root(home)),
    // `droid-root` resolves to `~/.factory` — the tag follows the CLIENT name,
    // the directory follows the vendor's. Both are frozen; they differ on
    // purpose.
    ("droid", |_, home| vendor_droid::droid_root(home)),
    ("warp", |_, home| vendor_warp::warp_root(home)),
    ("openclaw", |_, home| vendor_openclaw::openclaw_root(home)),
    ("kilo", |_, home| vendor_kilo::kilo_root(home)),
    // OpenCode's OTHER root: the directory holding the config **file** grim
    // splices its global MCP entry into. Resolved through the very function
    // the write path uses (`opencode_config::config_path_for_scope`), so the
    // anchor cannot drift from where the bytes land — the divergence this row
    // exists to close. `OpenCodeRoot` follows `$OPENCODE_CONFIG_DIR` via the
    // skills root; the config file follows `$OPENCODE_CONFIG` / XDG. They
    // coincide only in the default layout, and `candidate_anchors` keeps
    // `open-code-root` first so that tie never re-anchors a shipped record.
    (OPENCODE_CONFIG_ROW, |env, home| {
        opencode_config::global_config_dir(env("OPENCODE_CONFIG"), env("XDG_CONFIG_HOME"), home)
    }),
];

/// The [`VENDOR_ROOTS`] row naming OpenCode's config-file directory. Named
/// once so the row, the `candidate_anchors` arm, and the tests cannot drift;
/// its on-disk tag is `opencode-config-root` (Principle 9: append-only).
const OPENCODE_CONFIG_ROW: &str = "opencode-config";

/// `$XDG_CONFIG_HOME`, else `<home>/.config` — the injected-input twin of
/// [`xdg_config_dir`], so a [`VENDOR_ROOTS`] row that needs an XDG dir stays a
/// pure function of `(env, home)`. Identical to `xdg_config_dir()` when fed
/// [`env_dir`] and [`home_dir`], which is what [`AnchorRoots::resolve`] does.
fn xdg_config_from(env: EnvLookup<'_>, home: Option<PathBuf>) -> Option<PathBuf> {
    env("XDG_CONFIG_HOME").or_else(|| home.map(|h| h.join(".config")))
}

/// The interned [`VENDOR_ROOTS`] key equal to `name`, or `None` when no vendor
/// owns it.
///
/// An unrecognized name is **refused, never interned** — that is what stops a
/// hand-edited `state.json` from minting an anchor grim knows nothing about,
/// and it is why [`PathAnchor::VendorRoot`] can hold a `&'static str` at all.
fn vendor_root_name(name: &str) -> Option<&'static str> {
    VENDOR_ROOTS.iter().find(|(n, _)| *n == name).map(|(n, _)| *n)
}

/// A logical root an install target is stored relative to.
///
/// Serialized as a kebab-case string tag (human-readable, forward-additive
/// JSON) — see [`PathAnchor::serde_tag`], which is the `state.json` contract.
/// Closed internal enum — matches stay total, no `#[non_exhaustive]`.
///
/// Most anchors are just "a vendor's own config root" and live in the
/// [`VENDOR_ROOTS`] table as [`Self::VendorRoot`]. The variants spelled out
/// below are the ones that genuinely resist that shape — each documents why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathAnchor {
    /// Project scope: `<workspace>/…` (project state only).
    ///
    /// Not a vendor root: it is every client's project-scope anchor at once,
    /// and it is rooted at the workspace, not at anything vendor-derived.
    Workspace,
    /// Global OpenCode skills: `$OPENCODE_CONFIG_DIR/skills` else
    /// `$XDG_CONFIG_HOME`|`~/.config`/opencode/skills.
    ///
    /// Not a vendor root: this is a *skills* dir one level below the config
    /// root, and [`Self::OpenCodeRoot`] is derived back out of it.
    OpenCodeSkills,
    /// Global OpenCode config dir (the parent of [`Self::OpenCodeSkills`]):
    /// hosts the sibling `agents/` dir, so a global OpenCode agent lands at
    /// `<opencode-root>/agents/<name>.md`. Derived as the parent of the
    /// resolved skills root — no separate `AnchorRoots` field, and so no
    /// [`VENDOR_ROOTS`] row either.
    OpenCodeRoot,
    /// `$GRIM_HOME`: the global OpenCode rules dir; also the pre-move
    /// fallback anchor for global Copilot rules (the layout-migration
    /// reaper collects old workspace-layout outputs there).
    ///
    /// Not a vendor root: it is grim's own directory and the **universal**
    /// fallback appended to every candidate list in [`candidate_anchors`].
    GrimHome,
    /// The directory holding Claude Code's user config file `.claude.json`
    /// (global-scope MCP registrations): `$CLAUDE_CONFIG_DIR` else `$HOME`.
    /// NOT derivable from Claude's own root — with the override set the
    /// file lives *inside* that dir, without it the file is a *sibling* of
    /// `~/.claude`. A second, differently-shaped root for one vendor, which
    /// the one-row-per-vendor table cannot express.
    ClaudeUserDir,
    /// The cross-vendor open standard `$HOME/.agents/skills`, shared by
    /// every pool member, including the generic `agents` client.
    ///
    /// Not a vendor root on three counts: it belongs to no single vendor, the
    /// root already ends in `/skills` (so `relative` is the bare skill name),
    /// and it is keyed on the real `$HOME` — deliberately NOT relocated by
    /// `$CODEX_HOME` or `$GEMINI_CLI_HOME`, because grim writes one physical
    /// pool tree whose prune refcount guard depends on that one-path shape.
    AgentsSkills,
    /// A vendor's own global config root, named by its [`VENDOR_ROOTS`] key —
    /// e.g. `VendorRoot("cursor")`, serialized `cursor-root`.
    ///
    /// The payload is always an interned `&'static str` from that table:
    /// [`Self::from_serde_tag`] refuses any other name, so this cannot carry
    /// an arbitrary string read off disk.
    VendorRoot(&'static str),
}

impl PathAnchor {
    /// The serialized tag — **the `state.json` on-disk contract**.
    ///
    /// Borrowed for every fixed anchor; owned only for a composed
    /// `<name>-root`.
    fn serde_tag(self) -> Cow<'static, str> {
        match self {
            Self::Workspace => Cow::Borrowed("workspace"),
            // `open-code-*`, not `opencode-*`: these two tags were minted by
            // `#[serde(rename_all = "kebab-case")]`, which split the variant
            // name on the internal capital. Shipped state files carry that
            // spelling, so it is frozen — `Display` renders it verbatim.
            Self::OpenCodeSkills => Cow::Borrowed("open-code-skills"),
            Self::OpenCodeRoot => Cow::Borrowed("open-code-root"),
            Self::GrimHome => Cow::Borrowed("grim-home"),
            Self::ClaudeUserDir => Cow::Borrowed("claude-user-dir"),
            Self::AgentsSkills => Cow::Borrowed("agents-skills"),
            Self::VendorRoot(name) => Cow::Owned(format!("{name}-root")),
        }
    }

    /// Every anchor that is not a [`Self::VendorRoot`], i.e. the fixed arms of
    /// [`Self::from_serde_tag`]. Their tag strings are derived through
    /// [`Self::serde_tag`] rather than restated, so the two cannot drift;
    /// `vendor_root_rows_and_reachable_vendor_roots_agree` proves each one
    /// still parses back to itself.
    const FIXED_ANCHORS: &'static [Self] = &[
        Self::Workspace,
        Self::OpenCodeSkills,
        Self::OpenCodeRoot,
        Self::GrimHome,
        Self::ClaudeUserDir,
        Self::AgentsSkills,
    ];

    /// Parse a serialized tag. `None` for any tag grim does not know — the
    /// fail-closed read path.
    fn from_serde_tag(tag: &str) -> Option<Self> {
        Some(match tag {
            // Fixed tags first: a `VENDOR_ROOTS` row must never shadow one.
            "workspace" => Self::Workspace,
            "open-code-skills" => Self::OpenCodeSkills,
            "open-code-root" => Self::OpenCodeRoot,
            "grim-home" => Self::GrimHome,
            "claude-user-dir" => Self::ClaudeUserDir,
            "agents-skills" => Self::AgentsSkills,
            _ => Self::VendorRoot(vendor_root_name(tag.strip_suffix("-root")?)?),
        })
    }
}

impl Serialize for PathAnchor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.serde_tag())
    }
}

impl<'de> Deserialize<'de> for PathAnchor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Owned `String`, not a borrowed `&str`: the latter would fail on any
        // non-borrowing deserializer (`from_reader`, `from_value`).
        let tag = String::deserialize(deserializer)?;
        Self::from_serde_tag(&tag).ok_or_else(|| {
            // Name the vocabulary, as the derived impl's "unknown variant,
            // expected one of …" did: this surfaces on a corrupt or
            // hand-edited state.json, where the valid set is the whole answer.
            let known = Self::FIXED_ANCHORS
                .iter()
                .map(|a| a.serde_tag().into_owned())
                .chain(VENDOR_ROOTS.iter().map(|(name, _)| format!("{name}-root")))
                .collect::<Vec<_>>()
                .join(", ");
            // Bounded *and* escaped, because the two hazards are independent.
            // The tag comes off a `state.json` a `git clone` can deliver (grim's
            // own `.gitignore` exists only after grim has run) and lands on a
            // terminal via `tracing::error!("{err:#}")`: `escape_debug` renders
            // the control byte as `\u{…}`, and covers what
            // `char::is_control` does not — U+202E and the other bidi /
            // zero-width format characters are not control chars. `take(64)`
            // is the separate problem: without it a 1 MiB tag echoes whole.
            // A truncated prefix still greps in the offending file.
            let shown = tag.chars().take(64).collect::<String>();
            serde::de::Error::custom(format!(
                "unknown path anchor '{}', expected one of {known}",
                shown.escape_debug()
            ))
        })
    }
}

impl std::fmt::Display for PathAnchor {
    /// Renders the serde tag verbatim, so the anchor an error names is the
    /// literal string a user can grep for in their `state.json`.
    ///
    /// This used to hand-write `opencode-skills` / `opencode-root` for the two
    /// OpenCode anchors while serde wrote `open-code-*` — an error pointed at a
    /// tag that appears in no state file. The fix could only go this direction:
    /// the serde tag is on-disk state and can never move, whereas `Display` is
    /// human-readable error text, which `docs/src/stability.md` puts outside
    /// the compatibility promise. Pinned by
    /// `display_agrees_with_the_serde_tag_for_every_anchor`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.serde_tag())
    }
}

/// Every anchor's concrete on-disk root, resolved once at scope-resolution
/// time so [`PathAnchor::root`] is a pure lookup (no ambient env reads, no
/// I/O at resolve time).
///
/// A field is `None` (or a `vendor_roots` key absent) when that root is
/// unresolvable — neither the vendor env override nor `$HOME` /
/// `$XDG_CONFIG_HOME` yielded a path.
///
/// **`OpenCodeRoot` has NO entry here.** It is derived at lookup time as
/// `opencode_skills.as_ref().and_then(|s| s.parent())` — a structurally
/// derivable relationship that does not need its own stored root.
/// New anchors whose root is derivable from an existing field (e.g. a
/// parent or sibling of a stored path) should follow this pattern rather
/// than adding a new `Option<PathBuf>` field to this struct.
///
/// A plain vendor config root needs neither: it belongs in the
/// [`VENDOR_ROOTS`] table and lands in [`Self::vendor_roots`], so adding a
/// vendor never edits this struct or any fixture that builds it.
#[derive(Default)]
pub struct AnchorRoots {
    /// The workspace root project-scope targets are rooted at.
    pub workspace: PathBuf,
    /// `$GRIM_HOME`.
    pub grim_home: PathBuf,
    /// The global OpenCode skills root, when resolvable. The OpenCode config
    /// root ([`PathAnchor::OpenCodeRoot`]) is derived as the parent of this
    /// path — no separate field is needed.
    pub opencode_skills: Option<PathBuf>,
    /// The dir holding Claude Code's user config file (`.claude.json`),
    /// when resolvable: `$CLAUDE_CONFIG_DIR` else `$HOME`. Not derivable
    /// from Claude's own root (see [`PathAnchor::ClaudeUserDir`]).
    pub claude_user_dir: Option<PathBuf>,
    /// The shared cross-vendor skills pool (`$HOME/.agents/skills`), when
    /// resolvable.
    pub agents_skills: Option<PathBuf>,
    /// Every [`PathAnchor::VendorRoot`] root, keyed by its [`VENDOR_ROOTS`]
    /// name. A missing key means that vendor's root is unresolvable — exactly
    /// what a `None` field meant before.
    pub vendor_roots: BTreeMap<&'static str, PathBuf>,
}

impl AnchorRoots {
    /// Resolve every anchor root once, calling the vendor helpers with the
    /// same env inputs the materializer uses (single source of truth). The
    /// resolved set is then a pure lookup table for [`PathAnchor::root`].
    ///
    /// This is the **only** place ambient environment is read; the work is
    /// delegated to [`Self::resolve_from`] with the real lookups so the
    /// mapping itself stays assertable.
    pub fn resolve(workspace: PathBuf, ctx: &Context) -> Self {
        Self::resolve_from(workspace, ctx.grim_home().to_path_buf(), &env_dir, home_dir())
    }

    /// [`Self::resolve`] with the environment injected — a pure function of
    /// its arguments.
    ///
    /// The seam exists because the wiring is otherwise untestable and a
    /// one-line mistake: swapping two [`VENDOR_ROOTS`] resolvers anchors one
    /// vendor's global installs under another's root, and every tag-vocabulary
    /// test still passes (they police tag *names*, not what a name resolves
    /// to). `std::env::set_var` cannot stand in — Rust 2024 makes it `unsafe`
    /// and this crate forbids `unsafe_code` — so the established pattern here
    /// is to inject the values instead (`vendor_codex::codex_root`,
    /// `vendor_zed::zed_root_from`).
    fn resolve_from(workspace: PathBuf, grim_home: PathBuf, env: EnvLookup<'_>, home: Option<PathBuf>) -> Self {
        Self {
            workspace,
            grim_home,
            opencode_skills: vendor_opencode::global_skills_root(
                env("OPENCODE_CONFIG_DIR"),
                xdg_config_from(env, home.clone()),
            ),
            claude_user_dir: vendor_claude::user_config_dir(env("CLAUDE_CONFIG_DIR"), home.clone()),
            agents_skills: global_skills_root(home.clone()),
            vendor_roots: VENDOR_ROOTS
                .iter()
                .filter_map(|(name, resolve_root)| resolve_root(env, home.clone()).map(|root| (*name, root)))
                .collect(),
        }
    }
}

impl PathAnchor {
    /// The concrete on-disk root for this anchor — a pure lookup into the
    /// pre-resolved [`AnchorRoots`] (no env reads, no I/O). `None` when the
    /// anchor's vendor root is unresolvable.
    pub fn root(self, roots: &AnchorRoots) -> Option<PathBuf> {
        match self {
            Self::Workspace => Some(roots.workspace.clone()),
            Self::GrimHome => Some(roots.grim_home.clone()),
            Self::OpenCodeSkills => roots.opencode_skills.clone(),
            // The OpenCode config dir is the parent of the skills root; the
            // sibling `agents/` dir lives directly under it.
            Self::OpenCodeRoot => roots
                .opencode_skills
                .as_ref()
                .and_then(|s| s.parent())
                .map(std::path::Path::to_path_buf),
            Self::ClaudeUserDir => roots.claude_user_dir.clone(),
            Self::AgentsSkills => roots.agents_skills.clone(),
            Self::VendorRoot(name) => roots.vendor_roots.get(name).cloned(),
        }
    }
}

/// How strictly [`AnchoredPath::resolve`]'s Layer 2 containment guard treats
/// an escape out of the anchor root (`adr_anchor_escape_recovery.md` §D2).
///
/// The parameter is explicit — never defaulted — so every call site is
/// visited and classified, and a future caller cannot silently inherit the
/// permissive mode. Same fail-closed discipline as the no-wildcard match in
/// [`super::prune`]'s `is_security_class`.
///
/// Closed internal enum — matches stay total, no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Containment {
    /// Permit an escape whose leaf is not itself a symlink: Layer 1 already
    /// guarantees a Normal-only remainder, so the escape can only originate
    /// from a symlinked **ancestor** inside the root — the user's own layout
    /// (GNU stow, yadm, an iCloud/Dropbox-synced config dir). For read-only
    /// probes; a symlinked leaf (the CWE-59 shape) stays refused.
    ///
    /// **`#[cfg(unix)]` only.** On Windows this variant is a synonym for
    /// [`Self::Strict`] — every escape is refused — because `is_symlink()`
    /// does not cover every reparse tag (`LX_SYMLINK`, `APPEXECLINK`, WCI).
    /// Classify a new call site by intent regardless of platform; do not
    /// assume the escape is permitted.
    AllowRelocatedAncestor,
    /// Refuse every escape. For any caller that deletes or rewrites — a
    /// blanket relax would hand `remove_dir_all` a path outside the root.
    Strict,
}

/// An install target stored as `(anchor, relative)` for portability.
///
/// The `relative` remainder is forward-slash UTF-8 and Normal-only —
/// no `CurDir` (`.`), `ParentDir` (`..`), `RootDir`, or `Prefix`
/// component, never absolute, never empty. The invariant is asserted at
/// store time ([`Self::from_target`]) and re-checked at first use
/// ([`Self::resolve`]). Deserialization does **not** re-validate (bare
/// `String` + `deny_unknown_fields`); the resolve-time guard catches a
/// tampered remainder that passes JSON parsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchoredPath {
    /// The logical root this path is relative to.
    pub anchor: PathAnchor,
    /// Forward-slash UTF-8 remainder. Invariant: every component is
    /// `Normal` — see the type doc.
    pub relative: String,
}

impl AnchoredPath {
    /// Classify an absolute install target into `(anchor, relative)`.
    ///
    /// Tries the scope/client/kind candidate anchors longest-root-first and
    /// returns the first whose resolved root is a `Path`-level prefix of
    /// `abs`; the remainder is stored forward-slash with any `CurDir`
    /// component stripped and asserted Normal-only and non-empty.
    ///
    /// CALLER INVARIANT: `abs` MUST be the non-canonicalized (pre-symlink)
    /// form, built as `root.join(relative)` with no intervening
    /// canonicalize. Passing a canonicalized `abs` may yield
    /// [`AnchorError::UnknownAnchor`].
    ///
    /// # Errors
    ///
    /// [`AnchorError::UnknownAnchor`] when no candidate root prefixes `abs`.
    pub fn from_target(
        abs: &Path,
        scope: ConfigScope,
        client: ClientTarget,
        kind: ArtifactKind,
        roots: &AnchorRoots,
    ) -> Result<AnchoredPath, AnchorError> {
        // Build the closed candidate set for this (scope, client, kind) from
        // the §1.1 root/remainder table, then try them longest-root-first so
        // a more specific root (e.g. a vendor root nested under GrimHome in a
        // hermetic layout) wins over a shorter prefix.
        let mut candidates: Vec<(PathAnchor, PathBuf)> = candidate_anchors(scope, client, kind)
            .into_iter()
            .filter_map(|anchor| anchor.root(roots).map(|root| (anchor, root)))
            .collect();
        candidates.sort_by_key(|(_, root)| std::cmp::Reverse(root.components().count()));

        for (anchor, root) in candidates {
            if let Some(relative) = strip_prefix_relative(abs, &root) {
                return Ok(AnchoredPath { anchor, relative });
            }
        }

        Err(AnchorError::UnknownAnchor {
            path: abs.to_path_buf(),
        })
    }

    /// Re-join anchor + relative into an absolute on-disk path, guaranteed
    /// contained under the anchor root.
    ///
    /// Layer 1 (always): reject any component that is not `Normal`
    /// (`ParentDir`, `RootDir`, `Prefix`, `CurDir`) or an empty `relative`
    /// (the anchor root itself) → [`AnchorError::TraversalAttempt`]. Works
    /// for absent paths.
    /// Layer 2 (when the candidate exists OR is a symlink, dangling
    /// included): `dunce::canonicalize` both sides and assert
    /// `Path::starts_with` (component-granular, never str) →
    /// [`AnchorError::EscapedAnchor`]. When Layer 2 fires, the
    /// **canonicalized** path is returned (not the raw join), closing the
    /// TOCTOU window so callers always act on the validated, symlink-resolved
    /// path. A dangling symlink (target absent) fails canonicalize and
    /// surfaces as [`AnchorError::Io`] — never a silent `Ok` of the raw join.
    /// [`AnchorError::AnchorRootAbsent`] when `self.anchor.root(roots)` is
    /// `None`.
    ///
    /// `containment` states the caller's intent (see [`Containment`]) — a
    /// read-only probe may tolerate an escape through a relocated ancestor,
    /// a destructive caller never does.
    ///
    /// # Residual TOCTOU
    ///
    /// Containment is validated at check time and the returned path is the
    /// canonicalized, check-time-contained form — callers always operate on
    /// that path, never the raw `relative`. A *fully* TOCTOU-proof guarantee
    /// against an intermediate-directory symlink swapped between this call
    /// and the caller's filesystem op would require handle-based resolution
    /// (`openat` / `O_NOFOLLOW` walking each component). That is out of scope
    /// for v1 given the threat model: grim manages the user's own config
    /// dirs, so an attacker who can swap a directory under those roots
    /// already has the privileges the guard protects. The two-layer guard
    /// addresses the realistic case (a tampered stored `relative` or a
    /// symlink already present at check time), not a same-uid racing
    /// adversary.
    ///
    /// [`Containment::AllowRelocatedAncestor`] **narrows** that guard on read
    /// paths (`adr_anchor_escape_recovery.md` §D2). This is a deliberate
    /// trade-off, not something the paragraph above already sanctioned: a
    /// *pre-planted* ancestor symlink is exactly what Layer 2 exists for, and
    /// the exclusion above covers only a *racing* adversary. What survives the
    /// narrowing is the invariant worth having — a tampered or stale state
    /// record can never direct a delete or a rewrite outside the anchor root,
    /// because every destructive caller passes [`Containment::Strict`].
    ///
    /// Two permanent non-goals follow from that:
    ///
    /// - **Never cache a validated root or path prefix across `resolve()`
    ///   calls.** Containment holds only because every call re-canonicalizes
    ///   fresh. Reusing a prefix is the shape of gitoxide
    ///   GHSA-f89h-2fjh-2r9q / CVE-2026-44471 (symlink-prefix-reuse worktree
    ///   escape); the per-artifact loop at `installer.rs:602-699` is the
    ///   obvious place someone would later "optimize".
    /// - **grim must not be run elevated.** The threat model above rests on
    ///   grim holding no more privilege than the owner of the config dirs it
    ///   manages.
    ///
    /// # Errors
    ///
    /// See the variant list above.
    pub fn resolve(&self, roots: &AnchorRoots, containment: Containment) -> Result<PathBuf, AnchorError> {
        let root = self
            .anchor
            .root(roots)
            .ok_or(AnchorError::AnchorRootAbsent { anchor: self.anchor })?;

        // Layer 1 (always, even for absent paths): the stored remainder is
        // untrusted at read time. Reject every component that is not
        // `Normal` — `ParentDir`/`RootDir`/`Prefix`/`CurDir` — so a tampered
        // `..`, leading `/`, `.`, or drive prefix can never escape the root.
        // Also reject an empty remainder (the root itself) — rooting at the
        // anchor itself is dangerous.
        let relative = Path::new(&self.relative);
        if self.relative.is_empty() {
            return Err(AnchorError::TraversalAttempt {
                relative: self.relative.clone(),
            });
        }
        for component in relative.components() {
            if !matches!(component, Component::Normal(_)) {
                return Err(AnchorError::TraversalAttempt {
                    relative: self.relative.clone(),
                });
            }
        }

        let candidate = root.join(relative);

        // Layer 2 (when the candidate exists OR is a symlink): a symlink in
        // the tree could still route a Normal-only path outside the root.
        // `exists()` is `false` for a DANGLING symlink (target absent), so we
        // also test `is_symlink()` — otherwise the guard would be skipped and
        // `Ok(root.join(symlink))` returned unvalidated. For a dangling
        // symlink the canonicalize below fails, yielding a safe `Io` error.
        // Canonicalize both sides (`dunce` avoids `\\?\` UNC false-negatives
        // on Windows) and assert containment component-by-component via
        // `Path::starts_with` — never a string prefix. Return the
        // canonicalized path so callers act on the validated, resolved path.
        //
        // The leaf stat happens ONCE, here, and is reused by both the branch
        // condition and the carve-out below. `Path::is_symlink()` is itself a
        // `symlink_metadata` call, and stat'ing *after* the canonicalize would
        // widen the TOCTOU window in the exploitable direction.
        let leaf_is_symlink = std::fs::symlink_metadata(&candidate).is_ok_and(|m| m.file_type().is_symlink());
        if candidate.exists() || leaf_is_symlink {
            let canon_root = dunce::canonicalize(&root).map_err(|source| AnchorError::Io {
                path: root.clone(),
                source,
            })?;
            let canon_candidate = dunce::canonicalize(&candidate).map_err(|source| AnchorError::Io {
                path: candidate.clone(),
                source,
            })?;
            if !canon_candidate.starts_with(&canon_root) {
                // Exhaustive match, never `==`: a future third variant must be
                // classified here or the build breaks — the fail-closed
                // discipline the `Containment` doc claims.
                match containment {
                    // Unix only: the layouts this exists for (GNU stow, yadm,
                    // an iCloud/Dropbox-synced config dir) are Unix-only, and
                    // `is_symlink()` does not cover every Windows reparse tag
                    // (`LX_SYMLINK`, `APPEXECLINK`, WCI) — a security guard
                    // must not rest on that predicate. On Windows the variant
                    // is a synonym for `Strict`.
                    #[cfg(unix)]
                    Containment::AllowRelocatedAncestor if !leaf_is_symlink => {
                        // `warn!`, not `debug!`: every destructive caller
                        // refuses this path, so this is the only signal that
                        // grim is reading through a relocated ancestor.
                        tracing::warn!(
                            anchor = %self.anchor,
                            path = %canon_candidate.display(),
                            "resolving through a relocated ancestor outside the anchor root"
                        );
                        return Ok(canon_candidate);
                    }
                    Containment::AllowRelocatedAncestor | Containment::Strict => {}
                }
                return Err(AnchorError::EscapedAnchor {
                    anchor: self.anchor,
                    resolved: canon_candidate,
                });
            }
            // Return the canonicalized path (not the raw join) to close the
            // TOCTOU window: the caller acts on the symlink-resolved path that
            // was verified to be within the anchor root.
            return Ok(canon_candidate);
        }

        Ok(candidate)
    }
}

/// The closed candidate anchor set for a `(scope, client, kind)` install
/// target, from the §1.1 root/remainder table.
///
/// Project scope is always `[Workspace]` — a project target that does not
/// fall under the workspace is an [`AnchorError::UnknownAnchor`], never a
/// silently absolute path. Global scope uses an explicit match over every
/// materializable `(client, kind)` combination so that a future new
/// `ClientTarget` or `ArtifactKind` variant fails to compile here rather than
/// silently anchoring to `GrimHome`. The caller tries the returned anchors
/// longest-root-first so the more specific root wins.
///
/// Pairs that have **no materialization anchor** map to `None`, which the
/// caller turns into an empty candidate set:
///
/// - **Globally-declined pairs** (see [`is_declined_global_pair`]): a fresh
///   install never reaches `from_target` with one (the installer's
///   `kind_support` gate skips it first), and the `is_declined_global_pair`
///   guard short-circuits them here too. The explicit `None` arms are the
///   belt-and-suspenders fallback: if a future ADR flips a declined kind to
///   supported and the vendor's `kind_support` starts returning
///   [`KindSupport::Native`] before this table is updated, the guard stops
///   firing but the arm still degrades to `None` → [`AnchorError::UnknownAnchor`]
///   instead of the old `unreachable!()` panic.
/// - **Bundles**: a bundle is never materialized (it expands into members), so
///   no `(client, Bundle)` pair has an anchor. A fresh install never anchors a
///   bundle, but a hand-edited or legacy V1 state file can carry one and reach
///   [`convert_v1_records`](super::install_state) directly — that historically
///   hit an `unreachable!()` and panicked the whole load. It now degrades to
///   the already-handled [`AnchorError::UnknownAnchor`] (a lossy drop).
///
/// Every caller handles `UnknownAnchor`, so an unanchorable persisted record is
/// a graceful lossy drop, never a panic. Project scope is untouched — it is
/// unconditionally `[Workspace]` for every `(client, kind)` pair.
fn candidate_anchors(scope: ConfigScope, client: ClientTarget, kind: ArtifactKind) -> Vec<PathAnchor> {
    /// `client`'s own global config root — the [`VENDOR_ROOTS`] row keyed by
    /// the vendor's name.
    ///
    /// Deriving the key from the vendor instead of spelling it at each arm
    /// makes a typo impossible. It does tie the on-disk tag to
    /// [`Vendor::name`](crate::install::vendor::Vendor::name), which is why
    /// `every_reachable_anchor_tag_is_pinned` exists: renaming a vendor would
    /// silently re-point its anchor tag, and that test refuses it.
    fn vendor_root(client: ClientTarget) -> Option<PathAnchor> {
        Some(PathAnchor::VendorRoot(client.vendor().name()))
    }

    match scope {
        ConfigScope::Project => vec![PathAnchor::Workspace],
        ConfigScope::Global if is_declined_global_pair(client, kind) => Vec::new(),
        ConfigScope::Global => {
            let primary: Option<PathAnchor> = match (client, kind) {
                // Claude: all three materializable kinds live under the Claude root.
                (ClientTarget::Claude, ArtifactKind::Skill)
                | (ClientTarget::Claude, ArtifactKind::Rule)
                | (ClientTarget::Claude, ArtifactKind::Agent) => vendor_root(client),

                // Copilot: skills and agents live under the native $COPILOT_HOME root.
                (ClientTarget::Copilot, ArtifactKind::Skill) | (ClientTarget::Copilot, ArtifactKind::Agent) => {
                    vendor_root(client)
                }

                // Copilot: rules live under the native $COPILOT_HOME root
                // (`instructions/`). GrimHome stays the appended fallback so
                // pre-move records (workspace layout under $GRIM_HOME) still
                // classify — the layout-migration reaper collects them on the
                // next re-install.
                (ClientTarget::Copilot, ArtifactKind::Rule) => vendor_root(client),

                // OpenCode: skills live under the OpenCode skills root.
                (ClientTarget::OpenCode, ArtifactKind::Skill) => Some(PathAnchor::OpenCodeSkills),

                // OpenCode: agents live in the sibling `agents/` dir under the OpenCode
                // config root (parent of the skills root).
                (ClientTarget::OpenCode, ArtifactKind::Agent) => Some(PathAnchor::OpenCodeRoot),

                // OpenCode: rules live under $GRIM_HOME (loaded via the managed glob
                // in opencode.json — no native rules directory).
                (ClientTarget::OpenCode, ArtifactKind::Rule) => Some(PathAnchor::GrimHome),

                // Codex: skills live under the cross-vendor $HOME/.agents/skills.
                (ClientTarget::Codex, ArtifactKind::Skill) => Some(PathAnchor::AgentsSkills),

                // Codex: agents live in the sibling `agents/` dir under the Codex
                // config root ($CODEX_HOME|~/.codex).
                (ClientTarget::Codex, ArtifactKind::Agent) => vendor_root(client),

                // ── Wave-1 vendors (adr_vendor_wave_expansion mapping table) ──
                // Cursor: all four kinds native under `~/.cursor`.
                (ClientTarget::Cursor, ArtifactKind::Skill)
                | (ClientTarget::Cursor, ArtifactKind::Rule)
                | (ClientTarget::Cursor, ArtifactKind::Agent)
                | (ClientTarget::Cursor, ArtifactKind::Mcp) => vendor_root(client),

                // Kiro: skills, steering rules, MCP under `~/.kiro` (agents
                // declined — handled by the guard above).
                (ClientTarget::Kiro, ArtifactKind::Skill)
                | (ClientTarget::Kiro, ArtifactKind::Rule)
                | (ClientTarget::Kiro, ArtifactKind::Mcp) => vendor_root(client),

                // Junie: skills + MCP under `~/.junie` (agents declined).
                (ClientTarget::Junie, ArtifactKind::Skill) | (ClientTarget::Junie, ArtifactKind::Mcp) => {
                    vendor_root(client)
                }

                // Junie rules are `Degraded`, not declined — but only at
                // PROJECT scope, where every target anchors at `Workspace`
                // before this match is reached. Globally there is no
                // `~/.junie/rules/` at all, so there is no destination to
                // classify and `Vendor::kind_surface` refuses the scope before
                // the installer ever asks for one. `None` is the honest answer,
                // and it keeps global Junie rules at the zero outputs they have
                // always produced.
                (ClientTarget::Junie, ArtifactKind::Rule) => None,

                // Gemini: skills via the shared `.agents/skills` pool; agents +
                // MCP under `~/.gemini` (rules declined).
                (ClientTarget::Gemini, ArtifactKind::Skill) => Some(PathAnchor::AgentsSkills),
                (ClientTarget::Gemini, ArtifactKind::Agent) | (ClientTarget::Gemini, ArtifactKind::Mcp) => {
                    vendor_root(client)
                }

                // Zed: skills via the shared pool; MCP under `~/.config/zed`
                // (rules + agents declined).
                (ClientTarget::Zed, ArtifactKind::Skill) => Some(PathAnchor::AgentsSkills),
                (ClientTarget::Zed, ArtifactKind::Mcp) => vendor_root(client),

                // Amp: skills via the shared pool; MCP under `~/.config/amp`
                // (rules + agents declined).
                (ClientTarget::Amp, ArtifactKind::Skill) => Some(PathAnchor::AgentsSkills),
                (ClientTarget::Amp, ArtifactKind::Mcp) => vendor_root(client),

                // Generic client: the shared pool is its ONLY surface — rules,
                // agents, and MCP are all declined (no vendor-neutral format).
                (ClientTarget::Agents, ArtifactKind::Skill) => Some(PathAnchor::AgentsSkills),

                // Antigravity: the ONE pool member whose global skills are not
                // pooled. Project skills share `.agents/skills`, but globally
                // all three kinds live under its own `~/.gemini/config` root —
                // skills included. `AgentsSkills` here would write where
                // Antigravity does not read (rules declined; handled above).
                (ClientTarget::Antigravity, ArtifactKind::Skill)
                | (ClientTarget::Antigravity, ArtifactKind::Agent)
                | (ClientTarget::Antigravity, ArtifactKind::Mcp) => vendor_root(client),

                // Wave-2 skills-only batch. Each owns its global root except
                // Goose, whose skills are pooled at both scopes.
                (ClientTarget::Cline, ArtifactKind::Skill)
                | (ClientTarget::Droid, ArtifactKind::Skill)
                | (ClientTarget::Warp, ArtifactKind::Skill)
                | (ClientTarget::OpenClaw, ArtifactKind::Skill)
                | (ClientTarget::Kilo, ArtifactKind::Skill) => vendor_root(client),
                (ClientTarget::Goose, ArtifactKind::Skill) => Some(PathAnchor::AgentsSkills),

                // MCP config-entry anchors: Claude's user config file dir
                // (`.claude.json` — a sibling of `~/.claude`), OpenCode's
                // config dir (`opencode.json`), Copilot's native root
                // (`mcp-config.json`), Codex's config root (`config.toml`,
                // alongside the `agents/` dir — same root as the agent anchor).
                (ClientTarget::Claude, ArtifactKind::Mcp) => Some(PathAnchor::ClaudeUserDir),
                (ClientTarget::OpenCode, ArtifactKind::Mcp) => Some(PathAnchor::OpenCodeRoot),
                (ClientTarget::Copilot, ArtifactKind::Mcp) => vendor_root(client),
                (ClientTarget::Codex, ArtifactKind::Mcp) => vendor_root(client),

                // Declined (client, kind) pairs — normally short-circuited by
                // the `is_declined_global_pair` guard above; kept as explicit
                // arms so a future `kind_support` flip degrades to
                // `UnknownAnchor` (empty set) rather than panicking, and so a
                // new vendor's kind gap must still be classified here.
                (ClientTarget::Codex, ArtifactKind::Rule)
                | (ClientTarget::Kiro, ArtifactKind::Agent)
                | (ClientTarget::Junie, ArtifactKind::Agent)
                | (ClientTarget::Gemini, ArtifactKind::Rule)
                | (ClientTarget::Zed, ArtifactKind::Rule)
                | (ClientTarget::Zed, ArtifactKind::Agent)
                | (ClientTarget::Amp, ArtifactKind::Rule)
                | (ClientTarget::Amp, ArtifactKind::Agent)
                | (ClientTarget::Agents, ArtifactKind::Rule)
                | (ClientTarget::Agents, ArtifactKind::Agent)
                | (ClientTarget::Agents, ArtifactKind::Mcp)
                | (ClientTarget::Antigravity, ArtifactKind::Rule)
                | (ClientTarget::Cline, ArtifactKind::Rule)
                | (ClientTarget::Cline, ArtifactKind::Agent)
                | (ClientTarget::Cline, ArtifactKind::Mcp)
                | (ClientTarget::Droid, ArtifactKind::Rule)
                | (ClientTarget::Droid, ArtifactKind::Agent)
                | (ClientTarget::Droid, ArtifactKind::Mcp)
                | (ClientTarget::Goose, ArtifactKind::Rule)
                | (ClientTarget::Goose, ArtifactKind::Agent)
                | (ClientTarget::Goose, ArtifactKind::Mcp)
                | (ClientTarget::Warp, ArtifactKind::Rule)
                | (ClientTarget::Warp, ArtifactKind::Agent)
                | (ClientTarget::Warp, ArtifactKind::Mcp)
                | (ClientTarget::OpenClaw, ArtifactKind::Rule)
                | (ClientTarget::OpenClaw, ArtifactKind::Agent)
                | (ClientTarget::OpenClaw, ArtifactKind::Mcp)
                | (ClientTarget::Kilo, ArtifactKind::Rule)
                | (ClientTarget::Kilo, ArtifactKind::Agent)
                | (ClientTarget::Kilo, ArtifactKind::Mcp) => None,

                // Bundles are never materialized; they expand into members, so
                // no (client, Bundle) pair has an anchor. A legacy/hand-edited
                // state record carrying one degrades to `UnknownAnchor`.
                (_, ArtifactKind::Bundle) => None,
            };
            // `GrimHome` is the universal fallback; deduplicate when the
            // primary already is `GrimHome`. An anchorless pair (declined /
            // bundle) yields the empty set → `UnknownAnchor` at the caller.
            //
            // `AgentsSkills` is appended for a pool-capable client whose
            // native skills root is NOT already the pool: that client can be
            // opted into `.agents/skills` with
            // `[options.vendors.<name>].shared_skills`, and this function
            // cannot see the config — so the triple must classify BOTH
            // layouts, or a pooled dest would return `UnknownAnchor` and fail
            // the install *after* the files are on disk. It is additive, never
            // a replacement: the longest-root-first loop still lands a native
            // dest on the vendor's own root, because `$HOME/.agents/skills`
            // does not prefix `$HOME/.cursor/skills/<name>`.
            //
            // It goes AFTER the native anchor, not before. `from_target` sorts
            // by component count with a *stable* sort, so insertion order is
            // what breaks a tie — and the two roots do tie when a vendor
            // override points at the pool's parent (`OPENCODE_CONFIG_DIR=
            // $HOME/.agents` makes the OpenCode skills root the pool itself).
            // Native-first keeps that user's recorded anchor tag exactly where
            // it was; pool-first would silently re-anchor it on upgrade for
            // someone who never opted in. Where the roots genuinely nest
            // rather than tie, length decides and this order is irrelevant.
            //
            // Derived from [`Vendor::pool_capable`] rather than a literal
            // client list so a vendor joining the roster cannot forget this.
            match primary {
                None => Vec::new(),
                Some(PathAnchor::GrimHome) => vec![PathAnchor::GrimHome],
                // OpenCode MCP: the config FILE grim splices resolves from
                // `$OPENCODE_CONFIG` else XDG, while `OpenCodeRoot` is derived
                // from the skills root and therefore follows
                // `$OPENCODE_CONFIG_DIR`. The two coincide in the default
                // layout and diverge the moment EITHER variable is set — and
                // the write path is authoritative, so both must classify or a
                // global MCP install warns `UnknownAnchor` and skips, while an
                // uninstall resolves the record to the wrong file, tolerates
                // the NotFound, and leaves the real entry live forever.
                //
                // `OpenCodeRoot` stays FIRST: `from_target`'s sort is stable,
                // so insertion order breaks the tie the coinciding case
                // produces, keeping the `open-code-root` tag every shipped
                // record already carries.
                Some(PathAnchor::OpenCodeRoot) if kind == ArtifactKind::Mcp => vec![
                    PathAnchor::OpenCodeRoot,
                    PathAnchor::VendorRoot(OPENCODE_CONFIG_ROW),
                    PathAnchor::GrimHome,
                ],
                Some(anchor) if anchor != PathAnchor::AgentsSkills && pool_opt_in_capable(client, kind) => {
                    vec![anchor, PathAnchor::AgentsSkills, PathAnchor::GrimHome]
                }
                Some(anchor) => vec![anchor, PathAnchor::GrimHome],
            }
        }
    }
}

/// Whether `(client, kind)` is a globally-declined pair — the vendor's
/// [`kind_support`](crate::install::vendor::Vendor::kind_support) returns
/// [`KindSupport::Declined`], so it has no anchor remainder.
/// [`candidate_anchors`] returns an empty set for these instead of anchoring
/// (matching the Codex-rule precedent). Delegating straight to the vendor
/// keeps this in lockstep with every `kind_support` override — no separate
/// list to drift.
fn is_declined_global_pair(client: ClientTarget, kind: ArtifactKind) -> bool {
    client.vendor().kind_support(kind) == KindSupport::Declined
}

/// Whether `(client, kind)` can be moved into the shared `.agents/skills`
/// pool by `[options.vendors.<name>].shared_skills`, and therefore needs
/// [`PathAnchor::AgentsSkills`] among its candidates in addition to its
/// native anchor.
///
/// Only skills pool, and only for a client
/// [`pool_capable`](crate::install::vendor::Vendor::pool_capable) reports as a
/// verified reader of the pool — the same predicate the config gate uses, so
/// the accepted set and the classifiable set cannot drift apart.
fn pool_opt_in_capable(client: ClientTarget, kind: ArtifactKind) -> bool {
    kind == ArtifactKind::Skill && client.vendor().pool_capable()
}

/// Lexically subtract `root` from `abs` and return the forward-slash,
/// Normal-only remainder. Purely lexical — **never** canonicalizes, so it is
/// existence-independent: a V1→V2 migration of a legacy record whose target
/// file is gone still classifies (no silent data loss). `None` when `root`
/// is not a component-level prefix of `abs`, when the remainder is empty
/// (abs equals root exactly — rooting at the anchor itself is rejected), or
/// when the remainder is not Normal-only after `CurDir` stripping (a guard
/// against a malformed candidate ever yielding a non-portable remainder).
///
/// A `Normal` component whose bytes are not valid UTF-8 also yields `None`
/// (the `relative` field is invariantly UTF-8).
///
/// The prefix match walks components: `root`'s components must each match the
/// corresponding `abs` component. `Normal` components compare
/// **case-insensitively on Windows (NTFS) and macOS (HFS+/APFS default)**,
/// where the filesystem is not case-sensitive, and **byte-exact on Linux**,
/// where it is. The comparison is per-component Unicode lowercase — the whole
/// path string is never lowercased (that would corrupt case-sensitive Linux
/// segments embedded in a portable record). The stored remainder always
/// preserves the ORIGINAL case of `abs`'s components.
fn strip_prefix_relative(abs: &Path, root: &Path) -> Option<String> {
    let mut abs_components = abs.components();

    // Consume `root` component-by-component; each must match the next `abs`
    // component. This is the lexical, existence-independent replacement for
    // `Path::strip_prefix` — needed so the per-component, case-insensitive
    // compare can run on platforms with a case-insensitive filesystem.
    for root_component in root.components() {
        let abs_component = abs_components.next()?;
        if !components_match(&abs_component, &root_component) {
            return None;
        }
    }

    // The remainder is whatever is left of `abs` after the root prefix is
    // consumed. Keep only `Normal` segments — strip any `CurDir` (`.`); a
    // remainder carrying any other non-`Normal` component is rejected (never
    // stored). The remainder preserves the original case of `abs`.
    let mut parts: Vec<&str> = Vec::new();
    for component in abs_components {
        match component {
            Component::Normal(os) => parts.push(os.to_str()?),
            Component::CurDir => {}
            _ => return None,
        }
    }

    // Reject an empty remainder (abs == root exactly): storing the anchor
    // root itself as a relative path would be dangerous.
    if parts.is_empty() {
        return None;
    }

    Some(parts.join("/"))
}

/// Compare two path components for the lexical store-time prefix match.
///
/// Two `Normal` components compare case-insensitively on Windows/macOS
/// (case-insensitive filesystems) and byte-exact on Linux. The comparison is
/// per-component Unicode lowercase, never on the whole path string. Any
/// other component pair (`RootDir`, `Prefix`, `CurDir`, `ParentDir`) compares
/// structurally for equality.
fn components_match(abs: &Component<'_>, root: &Component<'_>) -> bool {
    match (abs, root) {
        (Component::Normal(a), Component::Normal(b)) => normal_components_match(a, b),
        (a, b) => a == b,
    }
}

/// Case-insensitive (Windows/macOS) or byte-exact (Linux) compare of two
/// `Normal` component values. Returns `false` when either side is not valid
/// UTF-8 on the case-insensitive platforms (the `relative` invariant is
/// UTF-8, so a non-UTF-8 root segment cannot match portably).
#[cfg(any(windows, target_os = "macos"))]
fn normal_components_match(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> bool {
    match (a.to_str(), b.to_str()) {
        (Some(a), Some(b)) => a.to_lowercase() == b.to_lowercase(),
        _ => false,
    }
}

/// Byte-exact compare of two `Normal` component values on Linux, where the
/// filesystem is case-sensitive.
#[cfg(not(any(windows, target_os = "macos")))]
fn normal_components_match(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> bool {
    a == b
}

/// An anchor resolution / containment failure.
///
/// `thiserror`, lowercase no-period messages (`quality-rust-errors.md`),
/// `#[non_exhaustive]` (error-enum convention).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AnchorError {
    /// Layer-1 rejection: a stored `relative` carried a non-`Normal`
    /// component (`..`, leading `/`, `.`, or a drive prefix), or was empty.
    #[error("path traversal rejected in stored relative path '{relative}'")]
    TraversalAttempt {
        /// The offending stored remainder.
        relative: String,
    },

    /// Layer-2 rejection: the canonicalized join escaped its anchor root
    /// (symlink tampering).
    ///
    /// The `resolved` field carries the absolute path. It is withheld from
    /// `Display` because that message is a one-line refusal shown wherever an
    /// error surfaces, not because the path is a secret — grim runs as the
    /// user whose own layout it is describing. Callers that can afford the
    /// detail surface it deliberately and structurally: `warn!` on every
    /// tolerance path, and the `retained` / `clients_unresolved` report
    /// fields, which exist precisely so the user can find what was left
    /// behind.
    #[error("resolved path escapes its anchor root (anchor: {anchor})")]
    EscapedAnchor {
        /// The anchor whose root was escaped.
        anchor: PathAnchor,
        /// The resolved path that fell outside the root. Not rendered in
        /// `Display` — see the variant docs.
        resolved: PathBuf,
    },

    /// Store-time ([`AnchoredPath::from_target`]) failure: no candidate
    /// anchor root prefixes the target.
    #[error("cannot classify install target '{path}' under any known anchor")]
    UnknownAnchor {
        /// The unclassifiable absolute target.
        path: PathBuf,
    },

    /// Resolve-time failure: the anchor's root is unresolvable (no env /
    /// home).
    #[error("anchor root '{anchor}' is unresolvable (no env / home)")]
    AnchorRootAbsent {
        /// The anchor whose root could not be resolved.
        anchor: PathAnchor,
    },

    /// A read / canonicalize I/O failure.
    #[error("I/O error at '{path}'")]
    Io {
        /// The path the failing operation acted on.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },
}

// ── T2: Specify resolve containment ─────────────────────────────────────────
// ── T4: Specify from_target classification ──────────────────────────────────
#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::OPENCODE_CONFIG_ROW;
    use crate::config::scope::ConfigScope;
    use crate::install::client_target::ClientTarget;
    use crate::oci::ArtifactKind;

    use super::{AnchorError, AnchorRoots, AnchoredPath, Containment, PathAnchor};

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Build an `AnchorRoots` with every field set to known paths.
    /// No environment is consulted — this is the "pure table lookup" setup.
    fn all_roots() -> AnchorRoots {
        AnchorRoots {
            workspace: PathBuf::from("/ws"),
            grim_home: PathBuf::from("/grim"),
            vendor_roots: [
                ("claude", PathBuf::from("/claude")),
                ("copilot", PathBuf::from("/copilot")),
                ("codex", PathBuf::from("/codex")),
            ]
            .into(),
            opencode_skills: Some(PathBuf::from("/oc/skills")),
            claude_user_dir: None,
            agents_skills: Some(PathBuf::from("/agents/skills")),
        }
    }

    // ── The on-disk anchor-tag contract ───────────────────────────────────

    /// Every anchor tag a shipped `grim` has ever written into `state.json` —
    /// i.e. the **serde** vocabulary, which is what reaches disk.
    ///
    /// **This list is an on-disk contract, not an inventory.** A tag may be
    /// APPENDED when a new anchor ships; a tag may never be removed, renamed,
    /// or re-pointed, because a `state.json` written by an older grim must keep
    /// deserializing forever (Principle 9, `docs/src/stability.md`).
    ///
    /// `open-code-*` is not a typo: `rename_all = "kebab-case"` split those two
    /// variant names on their internal capital, and what reached disk is frozen
    /// — see [`display_agrees_with_the_serde_tag_for_every_anchor`].
    const SHIPPED_ANCHOR_TAGS: &[&str] = &[
        "workspace",
        "claude-root",
        "copilot-root",
        "open-code-skills",
        "open-code-root",
        "grim-home",
        "claude-user-dir",
        "agents-skills",
        "codex-root",
        "cursor-root",
        "kiro-root",
        "junie-root",
        "gemini-root",
        "zed-root",
        "amp-root",
        "antigravity-root",
        "cline-root",
        "droid-root",
        "warp-root",
        "openclaw-root",
        "kilo-root",
        "opencode-config-root",
    ];

    /// Every shipped tag still loads from a LITERAL JSON string, and
    /// re-serializes to the identical bytes.
    ///
    /// This is deliberately a literal-fixture test, not a serialize-then-parse
    /// round-trip: the latter passes even if the whole tag vocabulary is
    /// renamed in lockstep, which is precisely the break this guards. Asserting
    /// only through `serde` (never through variant names) is what lets it
    /// survive a refactor of the enum's internal shape unmodified — a test that
    /// has to be edited alongside the code it guards proves nothing.
    #[test]
    fn every_shipped_anchor_tag_round_trips_from_literal_json() {
        for tag in SHIPPED_ANCHOR_TAGS {
            let literal = format!("\"{tag}\"");
            let anchor: PathAnchor = serde_json::from_str(&literal)
                .unwrap_or_else(|e| panic!("shipped anchor tag {literal} no longer deserializes: {e}"));
            assert_eq!(
                serde_json::to_string(&anchor).unwrap(),
                literal,
                "re-serializing {literal} must reproduce the identical on-disk bytes"
            );
        }
    }

    /// `Display` renders exactly what serde writes, for **every** anchor —
    /// the anchor an error names is the literal string in `state.json`.
    ///
    /// This replaces a characterization of the inverse: `Display` used to
    /// hand-write `opencode-skills` / `opencode-root` while
    /// `rename_all = "kebab-case"` had minted `open-code-*`, so a user grepping
    /// their state file for the anchor an error just named found nothing. Only
    /// `Display` could move — the serde tag is on-disk state, and aligning it
    /// the other way would have broken every shipped `state.json`.
    ///
    /// Asserted over the whole vocabulary rather than the two former offenders,
    /// so a future anchor cannot reintroduce the split.
    #[test]
    fn display_agrees_with_the_serde_tag_for_every_anchor() {
        let every = PathAnchor::FIXED_ANCHORS
            .iter()
            .copied()
            .chain(super::VENDOR_ROOTS.iter().map(|(name, _)| PathAnchor::VendorRoot(name)));
        for anchor in every {
            let on_disk: String = serde_json::from_value(serde_json::to_value(anchor).unwrap()).unwrap();
            assert_eq!(
                anchor.to_string(),
                on_disk,
                "Display must render the on-disk tag verbatim so an error message is greppable in state.json"
            );
        }
        // The two that used to diverge, spelled out: a regression here is the
        // whole point of the fix.
        assert_eq!(PathAnchor::OpenCodeSkills.to_string(), "open-code-skills");
        assert_eq!(PathAnchor::OpenCodeRoot.to_string(), "open-code-root");
    }

    /// No anchor reachable from `candidate_anchors` may carry a tag that is not
    /// pinned above. Adding a vendor mints a new `<name>-root` tag — a new
    /// on-disk literal — and this fails until it is appended to
    /// `SHIPPED_ANCHOR_TAGS`, so the contract is acknowledged rather than
    /// discovered later in a user's state file.
    #[test]
    fn every_reachable_anchor_tag_is_pinned() {
        let kinds = [
            ArtifactKind::Skill,
            ArtifactKind::Rule,
            ArtifactKind::Agent,
            ArtifactKind::Mcp,
            ArtifactKind::Bundle,
        ];
        for scope in [ConfigScope::Project, ConfigScope::Global] {
            for client in ClientTarget::ALL {
                for kind in kinds {
                    for anchor in super::candidate_anchors(scope, client, kind) {
                        // The SERDE tag, not `Display` — only serde reaches disk.
                        let tag = serde_json::to_value(anchor).unwrap();
                        let tag = tag.as_str().unwrap();
                        assert!(
                            SHIPPED_ANCHOR_TAGS.contains(&tag),
                            "({scope:?}, {client:?}, {kind:?}) anchors to the unpinned tag '{tag}' — \
                             append it to SHIPPED_ANCHOR_TAGS: it is now written into state.json forever"
                        );
                    }
                }
            }
        }
    }

    /// Hermetic roots for the `shared_skills` anchor tests: fixed, disjoint,
    /// and with the vendor rows derived from [`VENDOR_ROOTS`] so a vendor W-G
    /// adds is covered without touching this fixture.
    fn pool_test_roots() -> AnchorRoots {
        AnchorRoots {
            workspace: PathBuf::from("/ws"),
            grim_home: PathBuf::from("/grim"),
            opencode_skills: Some(PathBuf::from("/oc/skills")),
            claude_user_dir: Some(PathBuf::from("/claude-user")),
            agents_skills: Some(PathBuf::from("/agents/skills")),
            vendor_roots: super::VENDOR_ROOTS
                .iter()
                .map(|(name, _)| (*name, PathBuf::from(format!("/vendor-{name}"))))
                .collect(),
        }
    }

    /// Every client the `shared_skills` opt-in accepts must be able to
    /// classify a **pooled** global skill destination. Without it the opt-in
    /// silently does nothing at an unchanged pin — `output_at_current_layout`
    /// reads `from_target`'s `Err(_)` as "already at the current layout", so
    /// the integrity gate short-circuits to `AlreadyInstalled` and the skill
    /// never moves. Change the pin as well and the same `UnknownAnchor` is
    /// propagated with `?` from the materialize loop instead, **after the
    /// files are already on disk**. Silent no-op or hard failure, depending on
    /// the pin: neither is shippable.
    ///
    /// Adding a vendor to `POOL_CAPABLE_VENDORS` widens the set of configs
    /// `grim config set` accepts; this asserts the anchor table widened with
    /// it. It is not vacuous — the assertion fails for any pool-capable client
    /// whose global skill arm is reachable but whose candidate set omits
    /// `AgentsSkills`.
    #[test]
    fn every_pool_capable_client_can_anchor_a_pooled_global_skill() {
        let roots = pool_test_roots();
        let pool = PathAnchor::AgentsSkills.root(&roots).expect("hermetic pool root");
        let mut checked = 0usize;
        for client in ClientTarget::ALL {
            if !client.vendor().pool_capable() {
                continue;
            }
            checked += 1;
            let candidates = super::candidate_anchors(ConfigScope::Global, client, ArtifactKind::Skill);
            assert!(
                candidates.contains(&PathAnchor::AgentsSkills),
                "{client} is pool-capable, so `[options.vendors.{client}].shared_skills = true` is \
                 accepted — but its global skill candidates {candidates:?} cannot classify a pooled \
                 destination, which fails the install AFTER writing the files"
            );
            let ap = AnchoredPath::from_target(
                &pool.join("demo"),
                ConfigScope::Global,
                client,
                ArtifactKind::Skill,
                &roots,
            )
            .unwrap_or_else(|e| panic!("{client}: a pooled global skill must classify, got {e:?}"));
            assert_eq!(ap.anchor, PathAnchor::AgentsSkills, "{client}");
            assert_eq!(ap.relative, "demo", "{client}");
        }
        assert!(
            checked >= 8,
            "the capability roster shrank unexpectedly ({checked} clients)"
        );
    }

    /// The opt-in must be **additive**: appending `AgentsSkills` to a
    /// pool-capable client's candidates may not steal its NATIVE destination
    /// away from its own vendor root. The longest-root-first loop is what
    /// makes that hold, and it holds only while the two roots are disjoint.
    #[test]
    fn appending_the_pool_anchor_leaves_native_skill_dests_on_their_own_root() {
        let roots = pool_test_roots();
        // The three clients the opt-in actually moves: pool-capable, but with
        // a native skills root of their own.
        for (client, expected, relative) in [
            (ClientTarget::Cursor, PathAnchor::VendorRoot("cursor"), "skills/demo"),
            (ClientTarget::Copilot, PathAnchor::VendorRoot("copilot"), "skills/demo"),
            (ClientTarget::OpenCode, PathAnchor::OpenCodeSkills, "demo"),
        ] {
            let native = expected.root(&roots).unwrap().join(relative);
            let ap =
                AnchoredPath::from_target(&native, ConfigScope::Global, client, ArtifactKind::Skill, &roots).unwrap();
            assert_eq!(
                ap.anchor, expected,
                "{client}'s native skill dest must still anchor at its own root, not the pool"
            );
            assert_eq!(ap.relative, relative, "{client}");
        }
    }

    /// When the two roots are not disjoint but **equal**, the tie decides the
    /// recorded tag — and it must keep falling to the client's own anchor.
    ///
    /// `OPENCODE_CONFIG_DIR=$HOME/.agents` makes the OpenCode skills root the
    /// pool itself. Both candidates then have the same component count, so
    /// only the stable sort in [`AnchoredPath::from_target`] plus this
    /// function's native-first ordering keeps that user's existing
    /// `open-code-skills` record from being silently re-anchored on upgrade,
    /// for a feature they never enabled. The sibling test above uses disjoint
    /// roots, where length decides and the ordering is untested.
    #[test]
    fn a_root_tie_keeps_the_clients_own_anchor_not_the_pool() {
        let mut roots = pool_test_roots();
        let pool = PathBuf::from("/agents/skills");
        roots.opencode_skills = Some(pool.clone());
        let ap = AnchoredPath::from_target(
            &pool.join("demo"),
            ConfigScope::Global,
            ClientTarget::OpenCode,
            ArtifactKind::Skill,
            &roots,
        )
        .expect("a tied root must still classify");
        assert_eq!(
            ap.anchor,
            PathAnchor::OpenCodeSkills,
            "a root tie must resolve to the client's own anchor — re-anchoring an \
             existing record to 'agents-skills' would rewrite state.json for a user \
             who never enabled shared_skills"
        );
        assert_eq!(ap.relative, "demo");
    }

    /// Every [`VENDOR_ROOTS`] row resolves to **its own** vendor's root.
    ///
    /// The gap this closes: the tag-vocabulary tests police tag *names*, and
    /// `arch2_from_target_and_resolve_are_coherent_for_all_scope_client_kind_triples`
    /// builds its `AnchorRoots` from literal paths — so before this, no TEST
    /// anywhere called `AnchorRoots::resolve` (production callers exist:
    /// `command::scope_resolution`, `command::search`, `catalog_service`).
    /// Swapping two rows' resolvers
    /// (`("cursor", |_, home| kiro_root(home))` and vice versa) anchored every
    /// global Cursor install under `~/.kiro` with the whole suite still green.
    ///
    /// Asserted as one map equality rather than per-row `contains`, so a row
    /// that vanishes, or resolves to the wrong vendor's root, fails here. A
    /// newly *added* row is not this test's job — `hermetic_vendor_roots`
    /// filters it out; `vendor_root_rows_and_reachable_vendor_roots_agree` and
    /// `every_reachable_anchor_tag_is_pinned` are what refuse an
    /// unacknowledged one.
    ///
    #[test]
    fn every_vendor_root_row_resolves_to_its_own_root() {
        let home = PathBuf::from("/fake/home");
        // No variable is set: every row falls back to its `$HOME`-derived
        // default, which is where the per-vendor differences live.
        let no_env = |_: &str| None;
        let roots = AnchorRoots::resolve_from(
            PathBuf::from("/ws"),
            PathBuf::from("/grim"),
            &no_env,
            Some(home.clone()),
        );

        let expected = hermetic_vendor_roots(&home);
        let actual: std::collections::BTreeMap<&'static str, PathBuf> = roots
            .vendor_roots
            .iter()
            .filter(|(name, _)| expected.contains_key(**name))
            .map(|(name, root)| (*name, root.clone()))
            .collect();
        assert_eq!(
            actual, expected,
            "each VENDOR_ROOTS row must resolve to its own vendor's root — a swapped resolver \
             silently anchors one vendor's global installs under another's directory"
        );
        // No row-count assertion: `len() == VENDOR_ROOTS.len()` would
        // additionally require `zed`, whose macOS/Windows arms read the real
        // `$HOME` / `%APPDATA%` inside `vendor_zed`, making the test depend on
        // the host env rather than the injected inputs. A row that stops
        // resolving still fails the equality above; an UNACKNOWLEDGED new row
        // is caught by `vendor_root_rows_and_reachable_vendor_roots_agree`,
        // not here — the filter would drop it from `actual`.
        // The non-vendor roots resolve from the same injected inputs.
        assert_eq!(roots.agents_skills, Some(home.join(".agents").join("skills")));
        assert_eq!(roots.claude_user_dir, Some(home.clone()));
        assert_eq!(
            roots.opencode_skills,
            Some(home.join(".config").join("opencode").join("skills"))
        );
        assert_eq!(roots.workspace, PathBuf::from("/ws"));
        assert_eq!(roots.grim_home, PathBuf::from("/grim"));
    }

    /// The [`VENDOR_ROOTS`] rows whose root is a pure function of the injected
    /// `(env, home)` **on this target**, with the value each must produce
    /// under a resolvable home and no variable set.
    ///
    /// `zed` is the one exception, and only off Linux: `vendor_zed::zed_root`
    /// reads the real `$HOME` on macOS and `%APPDATA%` on Windows internally.
    /// Injecting through those arms means reshaping `vendor_zed`, which this
    /// change does not own — its own `zed_root_from` tests cover them. `cfg!`
    /// rather than `#[cfg]` so every arm keeps type-checking on every host.
    fn hermetic_vendor_roots(home: &std::path::Path) -> std::collections::BTreeMap<&'static str, PathBuf> {
        let mut expected: std::collections::BTreeMap<&'static str, PathBuf> = [
            ("claude", home.join(".claude")),
            ("copilot", home.join(".copilot")),
            ("codex", home.join(".codex")),
            ("cursor", home.join(".cursor")),
            ("kiro", home.join(".kiro")),
            ("junie", home.join(".junie")),
            ("gemini", home.join(".gemini")),
            ("amp", home.join(".config").join("amp")),
            ("antigravity", home.join(".gemini").join("config")),
            ("cline", home.join(".cline")),
            // `droid-root` is `~/.factory` — client name, vendor directory.
            ("droid", home.join(".factory")),
            ("warp", home.join(".warp")),
            ("openclaw", home.join(".openclaw")),
            ("kilo", home.join(".kilo")),
        ]
        .into();
        if cfg!(all(not(windows), not(target_os = "macos"))) {
            expected.insert("zed", home.join(".config").join("zed"));
        }
        expected
    }

    /// Each row reads **its own** environment override, and no other's.
    ///
    /// The map-equality test above runs with no variable set, so a row that
    /// consulted the wrong variable name would still land on the right `$HOME`
    /// default and pass. This feeds one variable at a time and asserts that
    /// only the vendor owning it moves.
    #[test]
    fn each_vendor_root_row_reads_only_its_own_env_override() {
        let home = PathBuf::from("/fake/home");
        let hermetic = hermetic_vendor_roots(&home);
        for (var, name, expected) in [
            ("CLAUDE_CONFIG_DIR", "claude", PathBuf::from("/ovr")),
            ("COPILOT_HOME", "copilot", PathBuf::from("/ovr")),
            ("CODEX_HOME", "codex", PathBuf::from("/ovr")),
            ("KIRO_HOME", "kiro", PathBuf::from("/ovr")),
            // `GEMINI_CLI_HOME` replaces the home, not the root: the `.gemini`
            // segment is still appended (the opposite shape to CODEX/KIRO).
            ("GEMINI_CLI_HOME", "gemini", PathBuf::from("/ovr/.gemini")),
        ] {
            let one = |v: &str| (v == var).then(|| PathBuf::from("/ovr"));
            let roots =
                AnchorRoots::resolve_from(PathBuf::from("/ws"), PathBuf::from("/grim"), &one, Some(home.clone()));
            assert_eq!(
                roots.vendor_roots.get(name),
                Some(&expected),
                "${var} must relocate the '{name}' root"
            );
            for (other, unmoved) in hermetic.iter().filter(|(other, _)| **other != name) {
                assert_eq!(
                    roots.vendor_roots.get(other),
                    Some(unmoved),
                    "${var} must move only '{name}', but it also moved '{other}'"
                );
            }
        }
    }

    /// An unknown tag is refused, not silently coerced. A hand-edited or
    /// future-version `state.json` must fail to load loudly.
    #[test]
    fn an_unknown_anchor_tag_is_refused() {
        assert!(serde_json::from_str::<PathAnchor>("\"not-a-real-anchor\"").is_err());
        assert!(serde_json::from_str::<PathAnchor>("\"nosuchvendor-root\"").is_err());
        // The derived impl additionally accepted serde's externally-tagged map
        // encoding of a unit variant; the hand-written one takes strings only.
        // A deliberate narrowing, pinned rather than left incidental: grim's
        // writer has only ever emitted the plain string, so no state.json grim
        // produced can carry this form.
        assert!(serde_json::from_str::<PathAnchor>("{\"claude-root\":null}").is_err());
        // Fail-closed against the hostile shapes a `git clone` can deliver in a
        // `.grimoire/state.json`. `from_serde_tag` matches a closed set, so
        // every one of these lands in the `None` arm — pinned rather than
        // argued, because "the match is closed" is exactly the property a
        // future prefix/suffix-tolerant tag parser would quietly break.
        for hostile in [
            "\u{1b}[2Jclaude-root",   // ESC, drives the terminal
            "claude\u{202e}-root",    // U+202E — not `char::is_control`
            "claude-root\0",          // NUL
            "\u{441}laude-root",      // Cyrillic homoglyph for `c`
            "claude-root ",           // trailing space
            "CLAUDE-ROOT",            // case
            &"a".repeat(1024 * 1024), // 1 MiB, no unbounded work
        ] {
            assert!(
                serde_json::from_value::<PathAnchor>(serde_json::Value::String(hostile.to_string())).is_err(),
                "a hostile tag must be refused, not coerced onto a real anchor"
            );
        }
    }

    /// The refused tag is echoed **escaped and bounded**. This message reaches
    /// a terminal through `tracing::error!("{err:#}")`, and a `state.json` can
    /// arrive with a `git clone` — grim's own `.gitignore` exists only after
    /// grim has run, so a repo can ship a hostile one.
    ///
    /// Both hostile shapes, because they need different guards: ESC drives the
    /// terminal, and U+202E is **not** `char::is_control`, so a test that only
    /// injects ESC would pass while the bidi case still shipped. The 1 MiB tag
    /// is the third: escaping alone would echo all of it verbatim.
    #[test]
    fn a_refused_anchor_tag_is_escaped_and_bounded_in_the_error() {
        let msg = |tag: &str| {
            serde_json::from_value::<PathAnchor>(serde_json::Value::String(tag.to_string()))
                .expect_err("a hostile tag must be refused")
                .to_string()
        };

        let esc = msg("\u{1b}[2J\u{202e}evil-root");
        assert!(!esc.contains('\u{1b}'), "raw ESC must not reach the terminal: {esc:?}");
        assert!(
            !esc.contains('\u{202e}'),
            "raw U+202E must not reach the terminal: {esc:?}"
        );
        assert!(
            esc.contains(r"\u{1b}[2J\u{202e}evil-root"),
            "the escaped tag must still be greppable in the user's state.json: {esc:?}"
        );

        // Bounded independently of the escaping: an unbounded echo is its own
        // problem, and `escape_debug` would only inflate it.
        let long = msg(&"a".repeat(1024 * 1024));
        assert!(
            !long.contains(&"a".repeat(65)),
            "a 1 MiB tag must be truncated, not echoed whole (message is {} bytes)",
            long.len()
        );
    }

    /// The write path, the table, and the read path agree.
    ///
    /// Two failure modes, both invisible to the tag-vocabulary tests above:
    ///
    /// 1. A [`VENDOR_ROOTS`] row whose `<name>-root` tag collides with a fixed
    ///    tag would be written under its own name and read back as the *other*
    ///    anchor, resolving against a different root. `from_serde_tag` matches
    ///    the fixed arms first, so the collision is silent.
    /// 2. A `candidate_anchors` arm that mints `VendorRoot(name)` for a vendor
    ///    with no table row. Its root never resolves, so `from_target` filters
    ///    the candidate out entirely — the target then anchors to the appended
    ///    `GrimHome` fallback, or fails as `UnknownAnchor`. Silently
    ///    misanchored or dropped output, not a bad tag on disk.
    /// 3. A [`PathAnchor::FIXED_ANCHORS`] that has fallen out of step with the
    ///    variants `serde_tag` actually knows: the exhaustive match forces a
    ///    new variant to gain a tag, but nothing forces it into that const, and
    ///    the omission would silently drop a valid tag from the deserialize
    ///    error's vocabulary.
    #[test]
    fn vendor_root_rows_and_reachable_vendor_roots_agree() {
        // A duplicate name would split the table's truth in two: `resolve`
        // collects into a map so the LAST row's root wins, while
        // `vendor_root_name` returns the FIRST row's key. The anchor would
        // still round-trip its tag, so nothing else below would notice.
        let unique: std::collections::BTreeSet<_> = super::VENDOR_ROOTS.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            unique.len(),
            super::VENDOR_ROOTS.len(),
            "VENDOR_ROOTS has a duplicate name; its anchor would resolve against one row's \
             root while being named by another's"
        );
        for (name, _) in super::VENDOR_ROOTS {
            let anchor = PathAnchor::VendorRoot(name);
            let tag = anchor.serde_tag();
            assert_eq!(
                PathAnchor::from_serde_tag(&tag),
                Some(anchor),
                "the '{name}' row's tag '{tag}' does not read back as its own anchor — \
                 it collides with a fixed tag and would silently resolve elsewhere"
            );
        }
        // The vocabulary the deserialize error offers must be the whole pinned
        // contract — no missing tag, no invented one. Set equality is what
        // catches a FIXED_ANCHORS that lost a member: a per-tag round-trip
        // cannot, because it only ever visits the tags the const already lists.
        let vocabulary: std::collections::BTreeSet<String> = PathAnchor::FIXED_ANCHORS
            .iter()
            .map(|a| a.serde_tag().into_owned())
            .chain(super::VENDOR_ROOTS.iter().map(|(name, _)| format!("{name}-root")))
            .collect();
        let pinned: std::collections::BTreeSet<String> = SHIPPED_ANCHOR_TAGS.iter().map(|t| (*t).to_string()).collect();
        assert_eq!(
            vocabulary, pinned,
            "FIXED_ANCHORS + VENDOR_ROOTS must name exactly the pinned tag contract; \
             a variant that gained a serde_tag arm but no FIXED_ANCHORS entry drops \
             silently out of the deserialize error's vocabulary"
        );
        let kinds = [
            ArtifactKind::Skill,
            ArtifactKind::Rule,
            ArtifactKind::Agent,
            ArtifactKind::Mcp,
            ArtifactKind::Bundle,
        ];
        for scope in [ConfigScope::Project, ConfigScope::Global] {
            for client in ClientTarget::ALL {
                for kind in kinds {
                    for anchor in super::candidate_anchors(scope, client, kind) {
                        if let PathAnchor::VendorRoot(name) = anchor {
                            assert!(
                                super::vendor_root_name(name).is_some(),
                                "({scope:?}, {client:?}, {kind:?}) mints VendorRoot(\"{name}\") but \
                                 VENDOR_ROOTS has no such row: its root would never resolve and the \
                                 tag it writes would not deserialize"
                            );
                        }
                    }
                }
            }
        }
    }

    // ── T2: PathAnchor::root — pure table lookup ──────────────────────────

    /// Workspace anchor returns the workspace field verbatim (no env).
    #[test]
    fn t2_workspace_root_returns_workspace() {
        let roots = all_roots();
        let got = PathAnchor::Workspace.root(&roots);
        assert_eq!(got, Some(PathBuf::from("/ws")));
    }

    /// GrimHome anchor returns the grim_home field verbatim (no env).
    #[test]
    fn t2_grim_home_root_returns_grim_home() {
        let roots = all_roots();
        let got = PathAnchor::GrimHome.root(&roots);
        assert_eq!(got, Some(PathBuf::from("/grim")));
    }

    /// The claude vendor-root anchor returns exactly its stored vendor_roots value.
    #[test]
    fn t2_claude_root_returns_claude_root_field() {
        let roots = all_roots();
        let got = PathAnchor::VendorRoot("claude").root(&roots);
        assert_eq!(got, Some(PathBuf::from("/claude")));
    }

    /// The copilot vendor-root anchor returns its stored vendor_roots value.
    #[test]
    fn t2_copilot_root_returns_copilot_root_field() {
        let roots = all_roots();
        let got = PathAnchor::VendorRoot("copilot").root(&roots);
        assert_eq!(got, Some(PathBuf::from("/copilot")));
    }

    /// OpenCodeSkills anchor returns the opencode_skills field.
    #[test]
    fn t2_opencode_skills_root_returns_opencode_skills_field() {
        let roots = all_roots();
        let got = PathAnchor::OpenCodeSkills.root(&roots);
        assert_eq!(got, Some(PathBuf::from("/oc/skills")));
    }

    /// With no "claude" key in vendor_roots, the anchor's root is None.
    #[test]
    fn t2_anchor_root_absent_when_option_is_none() {
        let roots = AnchorRoots {
            workspace: PathBuf::from("/ws"),
            grim_home: PathBuf::from("/grim"),
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
        };
        assert!(PathAnchor::VendorRoot("claude").root(&roots).is_none());
        assert!(PathAnchor::VendorRoot("copilot").root(&roots).is_none());
        assert!(PathAnchor::OpenCodeSkills.root(&roots).is_none());
    }

    /// OpenCodeRoot anchor returns None when opencode_skills is None.
    /// The root is derived as the parent of opencode_skills; if that field
    /// is absent, OpenCodeRoot has no resolvable root.
    #[test]
    fn f06_opencode_root_is_none_when_opencode_skills_is_none() {
        let roots = AnchorRoots {
            workspace: PathBuf::from("/ws"),
            grim_home: PathBuf::from("/grim"),
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
        };
        assert!(
            PathAnchor::OpenCodeRoot.root(&roots).is_none(),
            "OpenCodeRoot must be None when opencode_skills is None"
        );
    }

    // ── T2: AnchoredPath::resolve — Layer 1 rejections ───────────────────

    /// A normal relative path resolves to root.join(relative) without
    /// touching the filesystem (the candidate does not exist).
    #[test]
    fn t2_resolve_normal_path_returns_root_join_relative() {
        let roots = all_roots();
        let ap = AnchoredPath {
            anchor: PathAnchor::Workspace,
            relative: "skills/foo".to_string(),
        };
        let result = ap.resolve(&roots, Containment::Strict);
        assert_eq!(result.unwrap(), PathBuf::from("/ws/skills/foo"));
    }

    /// Layer 1 rejects a `..` component WITHOUT touching the filesystem.
    /// The candidate path need not exist for the rejection to fire.
    #[test]
    fn t2_resolve_parent_dir_component_returns_traversal_attempt() {
        let roots = all_roots();
        let ap = AnchoredPath {
            anchor: PathAnchor::Workspace,
            relative: "../secret".to_string(),
        };
        let err = ap.resolve(&roots, Containment::Strict).unwrap_err();
        assert!(
            matches!(err, AnchorError::TraversalAttempt { .. }),
            "expected TraversalAttempt, got {err:?}"
        );
    }

    /// Layer 1 rejects a leading `/` (RootDir component).
    #[test]
    fn t2_resolve_leading_slash_returns_traversal_attempt() {
        let roots = all_roots();
        let ap = AnchoredPath {
            anchor: PathAnchor::Workspace,
            relative: "/absolute/path".to_string(),
        };
        let err = ap.resolve(&roots, Containment::Strict).unwrap_err();
        assert!(
            matches!(err, AnchorError::TraversalAttempt { .. }),
            "expected TraversalAttempt, got {err:?}"
        );
    }

    /// Layer 1 rejects a CurDir (`.`) component — §1.2 decision: CurDir is
    /// rejected, not tolerated, to keep the invariant simple.
    #[test]
    fn t2_resolve_cur_dir_component_returns_traversal_attempt() {
        let roots = all_roots();
        let ap = AnchoredPath {
            anchor: PathAnchor::Workspace,
            relative: "./skills/foo".to_string(),
        };
        let err = ap.resolve(&roots, Containment::Strict).unwrap_err();
        assert!(
            matches!(err, AnchorError::TraversalAttempt { .. }),
            "expected TraversalAttempt for CurDir, got {err:?}"
        );
    }

    /// Layer 1 rejects an empty `relative` (F4): storing the anchor root
    /// itself is dangerous — the caller should always store a non-empty
    /// sub-path relative to the anchor.
    #[test]
    fn f4_empty_relative_returns_traversal_attempt() {
        let roots = all_roots();
        let ap = AnchoredPath {
            anchor: PathAnchor::Workspace,
            relative: String::new(),
        };
        let err = ap.resolve(&roots, Containment::Strict).unwrap_err();
        assert!(
            matches!(err, AnchorError::TraversalAttempt { .. }),
            "expected TraversalAttempt for empty relative, got {err:?}"
        );
    }

    /// A candidate that does not exist on disk skips Layer 2 and returns Ok.
    /// This proves Layer 1 works standalone (no canonicalize needed for absent paths).
    #[test]
    fn t2_resolve_absent_path_skips_layer2_returns_ok() {
        let roots = all_roots();
        // /ws/nonexistent/deeply/nested does not exist — Layer 2 is skipped.
        let ap = AnchoredPath {
            anchor: PathAnchor::Workspace,
            relative: "nonexistent/deeply/nested".to_string(),
        };
        let result = ap.resolve(&roots, Containment::Strict);
        assert!(result.is_ok(), "absent path should return Ok, got {result:?}");
        assert_eq!(result.unwrap(), PathBuf::from("/ws/nonexistent/deeply/nested"));
    }

    /// When the anchor's root is None, resolve returns AnchorRootAbsent.
    #[test]
    fn t2_resolve_anchor_root_none_returns_anchor_root_absent() {
        let roots = AnchorRoots {
            workspace: PathBuf::from("/ws"),
            grim_home: PathBuf::from("/grim"),
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
        };
        let ap = AnchoredPath {
            anchor: PathAnchor::VendorRoot("claude"),
            relative: "skills/foo".to_string(),
        };
        let err = ap.resolve(&roots, Containment::Strict).unwrap_err();
        assert!(
            matches!(
                err,
                AnchorError::AnchorRootAbsent {
                    anchor: PathAnchor::VendorRoot("claude")
                }
            ),
            "expected AnchorRootAbsent(claude vendor root), got {err:?}"
        );
    }

    /// A forward-slash relative path resolves identically regardless of OS.
    /// The result is root.join("skills/foo") on all platforms.
    #[test]
    fn t2_forward_slash_relative_resolves_cross_platform() {
        let roots = all_roots();
        let ap = AnchoredPath {
            anchor: PathAnchor::VendorRoot("claude"),
            relative: "skills/my-skill".to_string(),
        };
        let result = ap.resolve(&roots, Containment::Strict).unwrap();
        // Must equal the anchor root with each segment appended.
        let expected = PathBuf::from("/claude/skills/my-skill");
        assert_eq!(result, expected);
    }

    // ── T4: AnchoredPath::from_target — classification ───────────────────

    /// Project-scope Claude install classifies to Workspace anchor with the
    /// sub-path as the remainder: `<ws>/.claude/rules/x.md` →
    /// `(Workspace, ".claude/rules/x.md")`.
    #[test]
    fn t4_project_claude_rule_classifies_to_workspace() {
        let roots = all_roots();
        let abs = PathBuf::from("/ws/.claude/rules/x.md");
        let result = AnchoredPath::from_target(
            &abs,
            ConfigScope::Project,
            ClientTarget::Claude,
            ArtifactKind::Rule,
            &roots,
        );
        let ap = result.unwrap();
        assert_eq!(ap.anchor, PathAnchor::Workspace);
        assert_eq!(ap.relative, ".claude/rules/x.md");
    }

    /// Global Claude skill → `(VendorRoot("claude"), "skills/<name>")`.
    #[test]
    fn t4_global_claude_skill_classifies_to_claude_root() {
        let roots = all_roots();
        // abs is built as root.join(relative) per §1.5 caller invariant.
        let abs = PathBuf::from("/claude/skills/my-skill");
        let result = AnchoredPath::from_target(
            &abs,
            ConfigScope::Global,
            ClientTarget::Claude,
            ArtifactKind::Skill,
            &roots,
        );
        let ap = result.unwrap();
        assert_eq!(ap.anchor, PathAnchor::VendorRoot("claude"));
        assert_eq!(ap.relative, "skills/my-skill");
    }

    /// Global OpenCode skill → `(OpenCodeSkills, "<name>")`.
    /// The OpenCodeSkills root already ends in `/skills`, so the remainder
    /// is just the skill name with no prefix.
    #[test]
    fn t4_global_opencode_skill_classifies_to_opencode_skills() {
        let roots = all_roots();
        let abs = PathBuf::from("/oc/skills/my-skill");
        let result = AnchoredPath::from_target(
            &abs,
            ConfigScope::Global,
            ClientTarget::OpenCode,
            ArtifactKind::Skill,
            &roots,
        );
        let ap = result.unwrap();
        assert_eq!(ap.anchor, PathAnchor::OpenCodeSkills);
        assert_eq!(ap.relative, "my-skill");
    }

    /// Global OpenCode rule → `(GrimHome, ".opencode/rules/<name>.md")`.
    /// OpenCode global rules live under grim_home (the global "workspace").
    #[test]
    fn t4_global_opencode_rule_classifies_to_grim_home() {
        let roots = all_roots();
        // For global scope the "workspace" passed to vendor is grim_home.
        let abs = PathBuf::from("/grim/.opencode/rules/my-rule.md");
        let result = AnchoredPath::from_target(
            &abs,
            ConfigScope::Global,
            ClientTarget::OpenCode,
            ArtifactKind::Rule,
            &roots,
        );
        let ap = result.unwrap();
        assert_eq!(ap.anchor, PathAnchor::GrimHome);
        assert_eq!(ap.relative, ".opencode/rules/my-rule.md");
    }

    /// Global Copilot skill → `(VendorRoot("copilot"), "skills/<name>")`.
    #[test]
    fn t4_global_copilot_skill_classifies_to_copilot_root() {
        let roots = all_roots();
        let abs = PathBuf::from("/copilot/skills/my-skill");
        let result = AnchoredPath::from_target(
            &abs,
            ConfigScope::Global,
            ClientTarget::Copilot,
            ArtifactKind::Skill,
            &roots,
        );
        let ap = result.unwrap();
        assert_eq!(ap.anchor, PathAnchor::VendorRoot("copilot"));
        assert_eq!(ap.relative, "skills/my-skill");
    }

    /// Global Copilot agent → `(VendorRoot("copilot"), "agents/<name>.md")` — agents
    /// live under the native `$COPILOT_HOME` root beside `skills/`.
    #[test]
    fn t4_global_copilot_agent_classifies_to_copilot_root() {
        let roots = all_roots();
        let abs = PathBuf::from("/copilot/agents/my-agent.md");
        let ap = AnchoredPath::from_target(
            &abs,
            ConfigScope::Global,
            ClientTarget::Copilot,
            ArtifactKind::Agent,
            &roots,
        )
        .unwrap();
        assert_eq!(ap.anchor, PathAnchor::VendorRoot("copilot"));
        assert_eq!(ap.relative, "agents/my-agent.md");
    }

    /// Global OpenCode agent → `(OpenCodeRoot, "agents/<name>.md")` — agents
    /// live in the sibling `agents/` dir under the OpenCode config root
    /// (parent of the skills root), NOT under the skills root itself.
    #[test]
    fn t4_global_opencode_agent_classifies_to_opencode_root() {
        let roots = all_roots();
        let abs = PathBuf::from("/oc/agents/my-agent.md");
        let ap = AnchoredPath::from_target(
            &abs,
            ConfigScope::Global,
            ClientTarget::OpenCode,
            ArtifactKind::Agent,
            &roots,
        )
        .unwrap();
        assert_eq!(ap.anchor, PathAnchor::OpenCodeRoot);
        assert_eq!(ap.relative, "agents/my-agent.md");
    }

    /// Regression: the global OpenCode MCP config file classifies wherever
    /// the WRITE path puts it, under either OpenCode variable.
    ///
    /// The anchor set used to hold `OpenCodeRoot` alone, which is derived
    /// from the skills root and so follows `$OPENCODE_CONFIG_DIR` — while
    /// `opencode_config::config_path_for_scope` resolves the file from
    /// `$OPENCODE_CONFIG` else XDG. Setting either variable made the two
    /// disagree: install warned `UnknownAnchor` and skipped, and an uninstall
    /// after the fact resolved the record to the wrong file, tolerated the
    /// NotFound, and dropped the record while the real entry stayed live.
    #[test]
    fn global_opencode_mcp_config_classifies_under_either_opencode_variable() {
        let home = PathBuf::from("/fake/home");

        // Direction 1: `$OPENCODE_CONFIG_DIR` moves the skills root (and with
        // it `OpenCodeRoot`) while the config file stays at the XDG default.
        let config_dir_set = |v: &str| (v == "OPENCODE_CONFIG_DIR").then(|| PathBuf::from("/custom/oc"));
        let roots = AnchorRoots::resolve_from(
            PathBuf::from("/ws"),
            PathBuf::from("/grim"),
            &config_dir_set,
            Some(home.clone()),
        );
        let written = home.join(".config").join("opencode").join("opencode.json");
        let ap = AnchoredPath::from_target(
            &written,
            ConfigScope::Global,
            ClientTarget::OpenCode,
            ArtifactKind::Mcp,
            &roots,
        )
        .expect("the config file the write path targets must classify");
        assert_eq!(ap.anchor, PathAnchor::VendorRoot(OPENCODE_CONFIG_ROW));
        assert_eq!(ap.relative, "opencode.json");
        assert_eq!(ap.resolve(&roots, Containment::Strict).unwrap(), written);

        // Direction 2: `$OPENCODE_CONFIG` moves the config file to an
        // arbitrary path while the skills root stays at the XDG default.
        let config_set = |v: &str| (v == "OPENCODE_CONFIG").then(|| PathBuf::from("/etc/grim/oc.json"));
        let roots = AnchorRoots::resolve_from(
            PathBuf::from("/ws"),
            PathBuf::from("/grim"),
            &config_set,
            Some(home.clone()),
        );
        let ap = AnchoredPath::from_target(
            Path::new("/etc/grim/oc.json"),
            ConfigScope::Global,
            ClientTarget::OpenCode,
            ArtifactKind::Mcp,
            &roots,
        )
        .expect("an $OPENCODE_CONFIG-relocated config file must classify");
        assert_eq!(ap.anchor, PathAnchor::VendorRoot(OPENCODE_CONFIG_ROW));
        assert_eq!(ap.relative, "oc.json");

        // Default layout: the two roots coincide, and the tie must keep the
        // `open-code-root` tag every shipped `state.json` already carries.
        let no_env = |_: &str| None;
        let roots = AnchorRoots::resolve_from(
            PathBuf::from("/ws"),
            PathBuf::from("/grim"),
            &no_env,
            Some(home.clone()),
        );
        let ap = AnchoredPath::from_target(
            &written,
            ConfigScope::Global,
            ClientTarget::OpenCode,
            ArtifactKind::Mcp,
            &roots,
        )
        .unwrap();
        assert_eq!(
            ap.anchor,
            PathAnchor::OpenCodeRoot,
            "the coinciding default must not re-anchor an existing record"
        );
        assert_eq!(ap.relative, "opencode.json");
    }

    /// The new row is additive: a global OpenCode **agent** still anchors at
    /// `OpenCodeRoot`, which follows the skills root, not the config file.
    #[test]
    fn opencode_config_row_does_not_capture_the_agent_anchor() {
        let home = PathBuf::from("/fake/home");
        let config_set = |v: &str| (v == "OPENCODE_CONFIG").then(|| PathBuf::from("/etc/grim/oc.json"));
        let roots = AnchorRoots::resolve_from(
            PathBuf::from("/ws"),
            PathBuf::from("/grim"),
            &config_set,
            Some(home.clone()),
        );
        let abs = home.join(".config").join("opencode").join("agents").join("a.md");
        let ap = AnchoredPath::from_target(
            &abs,
            ConfigScope::Global,
            ClientTarget::OpenCode,
            ArtifactKind::Agent,
            &roots,
        )
        .unwrap();
        assert_eq!(ap.anchor, PathAnchor::OpenCodeRoot);
        assert_eq!(ap.relative, "agents/a.md");
    }

    /// The AgentsSkills and codex vendor-root anchors return their stored roots verbatim.
    #[test]
    fn t2_codex_anchors_return_their_fields() {
        let roots = all_roots();
        assert_eq!(
            PathAnchor::AgentsSkills.root(&roots),
            Some(PathBuf::from("/agents/skills"))
        );
        assert_eq!(
            PathAnchor::VendorRoot("codex").root(&roots),
            Some(PathBuf::from("/codex"))
        );
    }

    /// Global Codex skill → `(AgentsSkills, "<name>")` — the root already
    /// ends in `/skills`, so the remainder is just the skill name.
    #[test]
    fn t4_global_codex_skill_classifies_to_agents_skills() {
        let roots = all_roots();
        let abs = PathBuf::from("/agents/skills/my-skill");
        let ap = AnchoredPath::from_target(
            &abs,
            ConfigScope::Global,
            ClientTarget::Codex,
            ArtifactKind::Skill,
            &roots,
        )
        .unwrap();
        assert_eq!(ap.anchor, PathAnchor::AgentsSkills);
        assert_eq!(ap.relative, "my-skill");
    }

    /// Global Codex agent → `(VendorRoot("codex"), "agents/<name>.toml")`.
    #[test]
    fn t4_global_codex_agent_classifies_to_codex_root() {
        let roots = all_roots();
        let abs = PathBuf::from("/codex/agents/my-agent.toml");
        let ap = AnchoredPath::from_target(
            &abs,
            ConfigScope::Global,
            ClientTarget::Codex,
            ArtifactKind::Agent,
            &roots,
        )
        .unwrap();
        assert_eq!(ap.anchor, PathAnchor::VendorRoot("codex"));
        assert_eq!(ap.relative, "agents/my-agent.toml");
    }

    /// A project target NOT under the workspace produces UnknownAnchor.
    #[test]
    fn t4_project_target_outside_workspace_returns_unknown_anchor() {
        let roots = all_roots();
        // /other/path is not under /ws.
        let abs = PathBuf::from("/other/path/.claude/rules/x.md");
        let result = AnchoredPath::from_target(
            &abs,
            ConfigScope::Project,
            ClientTarget::Claude,
            ArtifactKind::Rule,
            &roots,
        );
        let err = result.unwrap_err();
        assert!(
            matches!(err, AnchorError::UnknownAnchor { .. }),
            "expected UnknownAnchor, got {err:?}"
        );
    }

    /// The stored `relative` is forward-slash, Normal-only:
    /// no leading slash, no `..` segments, no `.` segments.
    #[test]
    fn t4_stored_relative_is_normal_only_no_leading_slash_no_dotdot_no_dot() {
        let roots = all_roots();
        let abs = PathBuf::from("/claude/skills/my-skill");
        let ap = AnchoredPath::from_target(
            &abs,
            ConfigScope::Global,
            ClientTarget::Claude,
            ArtifactKind::Skill,
            &roots,
        )
        .unwrap();

        // No leading slash.
        assert!(
            !ap.relative.starts_with('/'),
            "relative must not start with '/': {}",
            ap.relative
        );
        // No ParentDir segments.
        assert!(
            !ap.relative.contains(".."),
            "relative must not contain '..': {}",
            ap.relative
        );
        // No CurDir segments (no bare '.' components).
        // A '.' followed by a non-dot char (like ".claude") is fine.
        for component in std::path::Path::new(&ap.relative).components() {
            assert!(
                matches!(component, std::path::Component::Normal(_)),
                "relative must contain only Normal components, found: {component:?}"
            );
        }
    }

    /// A remainder that would contain a CurDir component after strip_prefix
    /// must be stripped: the stored relative must have no `.` segments.
    /// We test by constructing an abs path whose relative portion after
    /// strip_prefix could be `./skills/foo` (hypothetical), confirming the
    /// output has no CurDir components.
    #[test]
    fn t4_cur_dir_stripped_from_remainder() {
        // This test documents the contract: even if a constructed path were
        // to yield a CurDir segment, from_target must strip it. We verify
        // by checking the returned relative is free of CurDir for a normal case.
        let roots = all_roots();
        let abs = PathBuf::from("/ws/.claude/rules/my-rule.md");
        let ap = AnchoredPath::from_target(
            &abs,
            ConfigScope::Project,
            ClientTarget::Claude,
            ArtifactKind::Rule,
            &roots,
        )
        .unwrap();

        for component in std::path::Path::new(&ap.relative).components() {
            assert!(
                matches!(component, std::path::Component::Normal(_)),
                "remainder has non-Normal component: {component:?}"
            );
        }
    }

    /// Non-canonicalized abs path built via root.join(relative) must
    /// classify successfully. The caller invariant (§1.5) states abs MUST
    /// be the pre-symlink form — strip_prefix against the (also
    /// non-canonicalized) root succeeds lexically.
    #[test]
    fn t4_non_canonicalized_abs_path_classifies_correctly() {
        let roots = all_roots();
        // Build abs as root.join(relative) — the caller invariant.
        let root = &roots.vendor_roots["claude"];
        let abs = root.join("skills").join("my-skill");
        let ap = AnchoredPath::from_target(
            &abs,
            ConfigScope::Global,
            ClientTarget::Claude,
            ArtifactKind::Skill,
            &roots,
        )
        .unwrap();
        assert_eq!(ap.anchor, PathAnchor::VendorRoot("claude"));
        assert_eq!(ap.relative, "skills/my-skill");
    }

    /// When GrimHome would be a prefix of a vendor root (hermetic test
    /// layout), longest-root-first ensures the more specific root wins.
    /// Layout: grim_home=/a, opencode_skills=/a/skills — a path under
    /// /a/skills matches OpenCodeSkills first (longer prefix).
    #[test]
    fn t4_longest_root_first_when_grim_home_prefixes_vendor_root() {
        // Hermetic layout where grim_home is an ancestor of opencode_skills.
        let roots = AnchorRoots {
            workspace: PathBuf::from("/ws"),
            grim_home: PathBuf::from("/a"),
            vendor_roots: [
                ("claude", PathBuf::from("/a/claude")),
                ("copilot", PathBuf::from("/a/copilot")),
            ]
            .into(),
            opencode_skills: Some(PathBuf::from("/a/skills")),
            claude_user_dir: None,
            agents_skills: None,
        };
        let abs = PathBuf::from("/a/skills/my-skill");
        let ap = AnchoredPath::from_target(
            &abs,
            ConfigScope::Global,
            ClientTarget::OpenCode,
            ArtifactKind::Skill,
            &roots,
        )
        .unwrap();
        // OpenCodeSkills has a longer root (/a/skills) than GrimHome (/a).
        assert_eq!(ap.anchor, PathAnchor::OpenCodeSkills);
        assert_eq!(ap.relative, "my-skill");
    }

    /// Global Copilot rule → `(VendorRoot("copilot"), "instructions/…")` — the native
    /// `$COPILOT_HOME|~/.copilot/instructions/` layout (the render-layout
    /// move away from the inert `$GRIM_HOME` workspace layout).
    #[test]
    fn t4_global_copilot_rule_classifies_to_copilot_root() {
        let roots = all_roots();
        let abs = roots.vendor_roots["copilot"].join("instructions/my-rule.instructions.md");
        let ap = AnchoredPath::from_target(
            &abs,
            ConfigScope::Global,
            ClientTarget::Copilot,
            ArtifactKind::Rule,
            &roots,
        )
        .unwrap();
        assert_eq!(ap.anchor, PathAnchor::VendorRoot("copilot"));
        assert_eq!(ap.relative, "instructions/my-rule.instructions.md");
    }

    /// A pre-move record's path (workspace layout under `$GRIM_HOME`) still
    /// classifies via the appended `GrimHome` fallback — required so the
    /// layout-migration reaper can resolve and collect the old outputs.
    #[test]
    fn t4_global_copilot_rule_grim_home_fallback_still_classifies() {
        let roots = all_roots();
        let abs = PathBuf::from("/grim/.github/instructions/my-rule.instructions.md");
        let ap = AnchoredPath::from_target(
            &abs,
            ConfigScope::Global,
            ClientTarget::Copilot,
            ArtifactKind::Rule,
            &roots,
        )
        .unwrap();
        assert_eq!(ap.anchor, PathAnchor::GrimHome);
        assert_eq!(ap.relative, ".github/instructions/my-rule.instructions.md");
    }

    /// F07: a global OpenCode skill path that falls under grim_home
    /// (because opencode_skills is None) must classify to GrimHome.
    ///
    /// When `opencode_skills` is None, the `OpenCodeSkills` anchor has no
    /// root. `GrimHome` is the fallback candidate, so a path under
    /// `grim_home/.opencode/skills/<name>` must still classify.
    #[test]
    fn f07_global_opencode_skill_falls_back_to_grim_home_when_opencode_skills_none() {
        let roots = AnchorRoots {
            workspace: PathBuf::from("/ws"),
            grim_home: PathBuf::from("/grim"),
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
        };
        // When there is no opencode_skills root, the vendor falls back to
        // the workspace layout under grim_home.
        let abs = PathBuf::from("/grim/.opencode/skills/my-skill");
        let ap = AnchoredPath::from_target(
            &abs,
            ConfigScope::Global,
            ClientTarget::OpenCode,
            ArtifactKind::Skill,
            &roots,
        )
        .unwrap();
        assert_eq!(ap.anchor, PathAnchor::GrimHome);
        assert_eq!(ap.relative, ".opencode/skills/my-skill");
    }

    // ── T3: resolve Layer-2 symlink-escape acceptance ────────────────────

    /// A symlink inside the anchor pointing OUTSIDE it must be caught by
    /// Layer 2: the candidate exists, so `dunce::canonicalize` resolves the
    /// symlink and `Path::starts_with` rejects the escape.
    #[cfg(unix)]
    #[test]
    fn t3_resolve_symlink_escape_returns_escaped_anchor() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        // Anchor root and a sibling "outside" dir holding the secret.
        let anchor_root = tmp.path().join("anchor");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&anchor_root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let secret = outside.join("secret.txt");
        std::fs::write(&secret, "top secret").unwrap();

        // A symlink INSIDE the anchor whose name is Normal but whose target
        // escapes the anchor root.
        let link = anchor_root.join("escape");
        symlink(&secret, &link).unwrap();

        let roots = AnchorRoots {
            workspace: anchor_root.clone(),
            grim_home: PathBuf::from("/unused"),
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
        };
        let ap = AnchoredPath {
            anchor: PathAnchor::Workspace,
            relative: "escape".to_string(),
        };
        let err = ap.resolve(&roots, Containment::Strict).unwrap_err();
        assert!(
            matches!(err, AnchorError::EscapedAnchor { .. }),
            "expected EscapedAnchor for a symlink pointing outside the root, got {err:?}"
        );
    }

    /// W2: a DANGLING symlink under the anchor root (link present, target
    /// absent) must still trip the Layer-2 containment guard. `exists()` is
    /// `false` for a dangling symlink, so the guard also tests `is_symlink()`;
    /// canonicalize then fails (the target is gone) and resolve returns an
    /// error rather than `Ok(root.join(symlink))`. The documented "Layer 2
    /// catches symlink escape" invariant must hold even when the target is
    /// missing — returning `Ok` here would hand the caller an unvalidated path.
    #[cfg(unix)]
    #[test]
    fn w2_resolve_dangling_symlink_returns_err() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let anchor_root = tmp.path().join("anchor");
        std::fs::create_dir_all(&anchor_root).unwrap();

        // A symlink INSIDE the anchor whose target does not exist (dangling).
        let link = anchor_root.join("dangling");
        symlink(tmp.path().join("nonexistent-target"), &link).unwrap();

        let roots = AnchorRoots {
            workspace: anchor_root.clone(),
            grim_home: PathBuf::from("/unused"),
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
        };
        let ap = AnchoredPath {
            anchor: PathAnchor::Workspace,
            relative: "dangling".to_string(),
        };
        let result = ap.resolve(&roots, Containment::Strict);
        assert!(
            result.is_err(),
            "dangling symlink must trip Layer 2 (canonicalize fails) rather than return Ok, got {result:?}"
        );
    }

    // ── A9: the relocated-ancestor carve-out (adr_anchor_escape_recovery §D2) ──

    /// A9(a) — the grimoire#57 regression test. A symlinked *ancestor* inside
    /// the anchor root with a REAL (non-symlink) leaf is the layout GNU stow,
    /// yadm and an iCloud/Dropbox-synced config dir produce. Layer 1 already
    /// guarantees a Normal-only remainder, so such an escape can only originate
    /// from the user's own directory layout — `AllowRelocatedAncestor` permits
    /// it and hands the caller the canonicalized outside path, so a read-only
    /// probe sees the file the user actually has installed.
    #[cfg(unix)]
    #[test]
    fn t3_relocated_ancestor_with_real_leaf_resolves_under_allow() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        // Canonical tmp root: macOS's /var -> /private/var symlink would
        // otherwise make the resolved-path assertion drift.
        let tmp = dunce::canonicalize(tmp.path()).unwrap();

        // The anchor root, with `.claude/skills` relocated OUT of it — the
        // ancestor is the symlink, the leaf is a real directory.
        let anchor_root = tmp.join("anchor");
        std::fs::create_dir_all(anchor_root.join(".claude")).unwrap();
        let store = tmp.join("elsewhere/skills");
        let leaf = store.join("demo-skill");
        std::fs::create_dir_all(&leaf).unwrap();
        std::fs::write(leaf.join("SKILL.md"), b"# demo\n").unwrap();
        symlink(&store, anchor_root.join(".claude/skills")).unwrap();

        let roots = AnchorRoots {
            workspace: anchor_root.clone(),
            grim_home: PathBuf::from("/unused"),
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
        };
        let ap = AnchoredPath {
            anchor: PathAnchor::Workspace,
            relative: ".claude/skills/demo-skill".to_string(),
        };

        let resolved = ap
            .resolve(&roots, Containment::AllowRelocatedAncestor)
            .expect("a relocated ancestor with a real leaf must resolve for a read-only caller (grimoire#57)");
        assert_eq!(
            resolved, leaf,
            "the caller must receive the canonicalized outside path, not the raw anchor join"
        );
    }

    /// A9(b) — the same tree under `Containment::Strict` stays refused. A
    /// destructive caller acts on a record, and a stored record must never be
    /// able to direct a delete or rewrite outside the anchor root — that is the
    /// invariant the read/destructive split preserves.
    #[cfg(unix)]
    #[test]
    fn t3_relocated_ancestor_with_real_leaf_still_escapes_under_strict() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let tmp = dunce::canonicalize(tmp.path()).unwrap();

        let anchor_root = tmp.join("anchor");
        std::fs::create_dir_all(anchor_root.join(".claude")).unwrap();
        let store = tmp.join("elsewhere/skills");
        let leaf = store.join("demo-skill");
        std::fs::create_dir_all(&leaf).unwrap();
        std::fs::write(leaf.join("SKILL.md"), b"# demo\n").unwrap();
        symlink(&store, anchor_root.join(".claude/skills")).unwrap();

        let roots = AnchorRoots {
            workspace: anchor_root.clone(),
            grim_home: PathBuf::from("/unused"),
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
        };
        let ap = AnchoredPath {
            anchor: PathAnchor::Workspace,
            relative: ".claude/skills/demo-skill".to_string(),
        };

        let err = ap.resolve(&roots, Containment::Strict).unwrap_err();
        assert!(
            matches!(err, AnchorError::EscapedAnchor { .. }),
            "Strict must refuse a relocated ancestor so a record can never direct a delete outside the root, got {err:?}"
        );
    }

    /// A9(c) — leaf-strictness holds UNDER a relocated ancestor. This is the
    /// case a naive relax breaks: once the ancestor is symlinked, a symlinked
    /// LEAF (the CWE-59 shape, and where the installer writes) must still be
    /// refused even for a read-only caller.
    #[cfg(unix)]
    #[test]
    fn t3_relocated_ancestor_with_symlinked_leaf_escapes_under_allow() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let tmp = dunce::canonicalize(tmp.path()).unwrap();

        // Same relocated ancestor as A9(a) …
        let anchor_root = tmp.join("anchor");
        std::fs::create_dir_all(anchor_root.join(".claude")).unwrap();
        let store = tmp.join("elsewhere/skills");
        std::fs::create_dir_all(&store).unwrap();
        symlink(&store, anchor_root.join(".claude/skills")).unwrap();

        // … but the leaf inside it is ITSELF a symlink, pointing at a secret
        // the anchor root has no claim to.
        let secret = tmp.join("secret.txt");
        std::fs::write(&secret, b"top secret\n").unwrap();
        symlink(&secret, store.join("demo-skill")).unwrap();

        let roots = AnchorRoots {
            workspace: anchor_root.clone(),
            grim_home: PathBuf::from("/unused"),
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
        };
        let ap = AnchoredPath {
            anchor: PathAnchor::Workspace,
            relative: ".claude/skills/demo-skill".to_string(),
        };

        let err = ap.resolve(&roots, Containment::AllowRelocatedAncestor).unwrap_err();
        assert!(
            matches!(err, AnchorError::EscapedAnchor { .. }),
            "a symlinked leaf stays refused even under a relocated ancestor, got {err:?}"
        );
    }

    /// A9(d) — a relocated ancestor with an ABSENT leaf resolves in both
    /// containment modes, and yields the raw anchor join. Layer 2 only engages
    /// for a candidate that exists or is a symlink, so an absent target is
    /// mode-independent: uninstall's "a missing target is operated on via the
    /// raw anchor join" contract, and an install into a relocated ancestor
    /// (which never calls `resolve`) must not start failing here either.
    #[cfg(unix)]
    #[test]
    fn t3_relocated_ancestor_with_absent_leaf_resolves_in_both_modes() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let tmp = dunce::canonicalize(tmp.path()).unwrap();

        let anchor_root = tmp.join("anchor");
        std::fs::create_dir_all(anchor_root.join(".claude")).unwrap();
        let store = tmp.join("elsewhere/skills");
        std::fs::create_dir_all(&store).unwrap();
        symlink(&store, anchor_root.join(".claude/skills")).unwrap();
        // No leaf is created: `.claude/skills/demo-skill` does not exist.

        let roots = AnchorRoots {
            workspace: anchor_root.clone(),
            grim_home: PathBuf::from("/unused"),
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
        };
        let ap = AnchoredPath {
            anchor: PathAnchor::Workspace,
            relative: ".claude/skills/demo-skill".to_string(),
        };
        let raw_join = anchor_root.join(".claude/skills/demo-skill");

        assert_eq!(
            ap.resolve(&roots, Containment::AllowRelocatedAncestor).unwrap(),
            raw_join,
            "an absent leaf yields the raw anchor join for a read-only caller"
        );
        assert_eq!(
            ap.resolve(&roots, Containment::Strict).unwrap(),
            raw_join,
            "an absent leaf is mode-independent — Strict must not start refusing it"
        );
    }

    // ── B2 / W5: store-time anchoring is existence-independent + lexical ───

    /// B2 proof: `from_target` must classify a target whose file does NOT
    /// exist on disk. Store-time anchoring is purely lexical — no canonicalize
    /// — so a V1→V2 migration of a legacy record whose target file is gone
    /// still classifies (no silent data loss). This must hold on EVERY
    /// platform, including macOS/Windows where the prior code canonicalized
    /// (and so dropped the record when the path was absent).
    #[test]
    fn b2_from_target_classifies_absent_target_path() {
        // A hermetic, non-existent workspace root and an absent sub-path under
        // it. Neither is created on disk.
        let roots = AnchorRoots {
            workspace: PathBuf::from("/definitely/not/on/disk/ws"),
            grim_home: PathBuf::from("/definitely/not/on/disk/grim"),
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
        };
        let abs = PathBuf::from("/definitely/not/on/disk/ws/.claude/rules/gone.md");
        let ap = AnchoredPath::from_target(
            &abs,
            ConfigScope::Project,
            ClientTarget::Claude,
            ArtifactKind::Rule,
            &roots,
        )
        .expect("absent target must still classify (existence-independent store-time anchoring)");
        assert_eq!(ap.anchor, PathAnchor::Workspace);
        assert_eq!(ap.relative, ".claude/rules/gone.md");
    }

    /// B2 (macOS): the per-component prefix match is case-insensitive on
    /// macOS. An `abs` built with a case-variant of a root path segment must
    /// still classify, and the stored remainder preserves the ORIGINAL case
    /// of the remainder components (not the root's).
    #[cfg(target_os = "macos")]
    #[test]
    fn b2_macos_case_insensitive_root_segment_classifies() {
        let roots = AnchorRoots {
            workspace: PathBuf::from("/Users/Alice/ws"),
            grim_home: PathBuf::from("/grim"),
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
        };
        // abs uses a different case for the "Users"/"Alice"/"ws" segments —
        // on macOS (case-insensitive FS) this is the same path.
        let abs = PathBuf::from("/users/alice/WS/.claude/rules/MixedCase.md");
        let ap = AnchoredPath::from_target(
            &abs,
            ConfigScope::Project,
            ClientTarget::Claude,
            ArtifactKind::Rule,
            &roots,
        )
        .expect("case-variant root segment must classify on macOS");
        assert_eq!(ap.anchor, PathAnchor::Workspace);
        // Remainder preserves the ORIGINAL case of the abs components.
        assert_eq!(ap.relative, ".claude/rules/MixedCase.md");
    }

    /// W5: a non-UTF-8 `Normal` component in the remainder yields
    /// `UnknownAnchor` — the `relative` field is invariantly UTF-8, so a
    /// path that cannot round-trip through UTF-8 is unclassifiable.
    #[cfg(unix)]
    #[test]
    fn w5_non_utf8_component_returns_unknown_anchor() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let roots = AnchorRoots {
            workspace: PathBuf::from("/ws"),
            grim_home: PathBuf::from("/grim"),
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
        };
        // Build /ws/<non-utf8> by appending a non-UTF-8 component.
        let mut abs = PathBuf::from("/ws");
        abs.push(OsStr::from_bytes(&[0x66, 0x80])); // "f" + lone continuation byte
        let err = AnchoredPath::from_target(
            &abs,
            ConfigScope::Project,
            ClientTarget::Claude,
            ArtifactKind::Rule,
            &roots,
        )
        .unwrap_err();
        assert!(
            matches!(err, AnchorError::UnknownAnchor { .. }),
            "non-UTF-8 component must yield UnknownAnchor, got {err:?}"
        );
    }

    /// W5: an empty remainder (`abs == root` exactly) yields `UnknownAnchor` —
    /// storing the anchor root itself as a relative path is rejected at store
    /// time.
    #[test]
    fn w5_empty_remainder_returns_unknown_anchor() {
        let roots = AnchorRoots {
            workspace: PathBuf::from("/ws"),
            grim_home: PathBuf::from("/grim"),
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
        };
        // abs equals the workspace root exactly.
        let abs = PathBuf::from("/ws");
        let err = AnchoredPath::from_target(
            &abs,
            ConfigScope::Project,
            ClientTarget::Claude,
            ArtifactKind::Rule,
            &roots,
        )
        .unwrap_err();
        assert!(
            matches!(err, AnchorError::UnknownAnchor { .. }),
            "abs == root must yield UnknownAnchor, got {err:?}"
        );
    }

    // ── ARCH-2: from_target / candidate_anchors / resolve coherence ──────
    //
    // This test locks the three-way coherence of `from_target`,
    // `candidate_anchors`, and `resolve`: for every (scope, client, kind)
    // triple the anchor + sub-path derived from the §1.1 anchor-remainder
    // table must survive a full classification → re-resolve round-trip
    // against a hermetic, env-free `AnchorRoots`.
    //
    // WHY HERMETIC: `path_for` reads the real environment ($HOME,
    // $CLAUDE_CONFIG_DIR, $XDG_CONFIG_HOME, …) and therefore produces a
    // path that does NOT fall under the fixed roots used here for global
    // scope.  Using `path_for` to build the input path and then trying
    // `from_target` causes every global vendor-anchor combo to silently
    // hit `UnknownAnchor`, hiding classification bugs.  Instead we build
    // the input path DIRECTLY from the expected (anchor, sub-path) pair
    // defined in the table below — same source of truth as §1.1 / the
    // subsystem-file-structure rule — which means no env is consulted at
    // any point.
    //
    // path_for end-to-end coherence (env-sensitive) is additionally covered
    // by the acceptance suite (test_agents / test_targets install via the
    // real `path_for`, then `status` resolves back — so the full pipeline
    // is exercised under a real env in CI).

    /// Hermetic anchor-remainder table: the expected `(anchor, relative)`
    /// for every materializable `(scope, client, kind)` triple, derived
    /// directly from §1.1 of the design record and
    /// `subsystem-file-structure.md`.  Any mismatch between this table and
    /// `candidate_anchors` / `AnchoredPath::from_target` / `resolve` is
    /// caught as a test failure.
    ///
    /// Rules for the table:
    /// - `relative` uses the same sub-path the vendor would produce, but
    ///   rooted at the anchor, not at the on-disk absolute path.
    /// - For OpenCode agents the anchor is `OpenCodeRoot` (the parent of the
    ///   skills dir) and the sub-path is `agents/<name>.md`.
    /// - GrimHome entries (inert Copilot rule, OpenCode rule) are still
    ///   classified — they must NOT become `UnknownAnchor`.
    fn expected_anchor_and_relative(
        scope: ConfigScope,
        client: ClientTarget,
        kind: ArtifactKind,
        name: &str,
    ) -> (PathAnchor, String) {
        match (scope, client, kind) {
            // ── Project scope ──────────────────────────────────────────────
            // All project targets land under Workspace; the sub-path is
            // the vendor's dot-dir relative to the workspace root.
            (ConfigScope::Project, ClientTarget::Claude, ArtifactKind::Skill) => {
                (PathAnchor::Workspace, format!(".claude/skills/{name}"))
            }
            (ConfigScope::Project, ClientTarget::Claude, ArtifactKind::Rule) => {
                (PathAnchor::Workspace, format!(".claude/rules/{name}.md"))
            }
            (ConfigScope::Project, ClientTarget::Claude, ArtifactKind::Agent) => {
                (PathAnchor::Workspace, format!(".claude/agents/{name}.md"))
            }
            (ConfigScope::Project, ClientTarget::Copilot, ArtifactKind::Skill) => {
                (PathAnchor::Workspace, format!(".github/skills/{name}"))
            }
            (ConfigScope::Project, ClientTarget::Copilot, ArtifactKind::Rule) => (
                PathAnchor::Workspace,
                format!(".github/instructions/{name}.instructions.md"),
            ),
            (ConfigScope::Project, ClientTarget::Copilot, ArtifactKind::Agent) => {
                (PathAnchor::Workspace, format!(".github/agents/{name}.md"))
            }
            (ConfigScope::Project, ClientTarget::OpenCode, ArtifactKind::Skill) => {
                (PathAnchor::Workspace, format!(".opencode/skills/{name}"))
            }
            (ConfigScope::Project, ClientTarget::OpenCode, ArtifactKind::Rule) => {
                (PathAnchor::Workspace, format!(".opencode/rules/{name}.md"))
            }
            (ConfigScope::Project, ClientTarget::OpenCode, ArtifactKind::Agent) => {
                (PathAnchor::Workspace, format!(".opencode/agents/{name}.md"))
            }

            // ── Global scope ───────────────────────────────────────────────
            // Claude: all three kinds → the claude vendor root.
            (ConfigScope::Global, ClientTarget::Claude, ArtifactKind::Skill) => {
                (PathAnchor::VendorRoot("claude"), format!("skills/{name}"))
            }
            (ConfigScope::Global, ClientTarget::Claude, ArtifactKind::Rule) => {
                (PathAnchor::VendorRoot("claude"), format!("rules/{name}.md"))
            }
            (ConfigScope::Global, ClientTarget::Claude, ArtifactKind::Agent) => {
                (PathAnchor::VendorRoot("claude"), format!("agents/{name}.md"))
            }

            // Copilot skill / agent → the copilot vendor root.
            (ConfigScope::Global, ClientTarget::Copilot, ArtifactKind::Skill) => {
                (PathAnchor::VendorRoot("copilot"), format!("skills/{name}"))
            }
            (ConfigScope::Global, ClientTarget::Copilot, ArtifactKind::Agent) => {
                (PathAnchor::VendorRoot("copilot"), format!("agents/{name}.md"))
            }

            // Copilot rule (inert) → GrimHome.
            (ConfigScope::Global, ClientTarget::Copilot, ArtifactKind::Rule) => (
                PathAnchor::GrimHome,
                format!(".github/instructions/{name}.instructions.md"),
            ),

            // OpenCode skill → OpenCodeSkills (root already ends in /skills).
            (ConfigScope::Global, ClientTarget::OpenCode, ArtifactKind::Skill) => {
                (PathAnchor::OpenCodeSkills, name.to_string())
            }

            // OpenCode agent → OpenCodeRoot (parent of the skills dir).
            (ConfigScope::Global, ClientTarget::OpenCode, ArtifactKind::Agent) => {
                (PathAnchor::OpenCodeRoot, format!("agents/{name}.md"))
            }

            // OpenCode rule → GrimHome.
            (ConfigScope::Global, ClientTarget::OpenCode, ArtifactKind::Rule) => {
                (PathAnchor::GrimHome, format!(".opencode/rules/{name}.md"))
            }

            // Codex project: skills in `.agents/skills`, agents as `.codex` TOML.
            (ConfigScope::Project, ClientTarget::Codex, ArtifactKind::Skill) => {
                (PathAnchor::Workspace, format!(".agents/skills/{name}"))
            }
            (ConfigScope::Project, ClientTarget::Codex, ArtifactKind::Agent) => {
                (PathAnchor::Workspace, format!(".codex/agents/{name}.toml"))
            }
            // Codex global: skill → AgentsSkills (root ends in /skills);
            // agent → the codex vendor root + `agents/<name>.toml`.
            (ConfigScope::Global, ClientTarget::Codex, ArtifactKind::Skill) => {
                (PathAnchor::AgentsSkills, name.to_string())
            }
            (ConfigScope::Global, ClientTarget::Codex, ArtifactKind::Agent) => {
                (PathAnchor::VendorRoot("codex"), format!("agents/{name}.toml"))
            }
            // Codex rules are unsupported — excluded from the loop below.
            (_, ClientTarget::Codex, ArtifactKind::Rule) => {
                unreachable!("Codex rules are skipped, not classified")
            }

            // ── Wave-1 vendors (adr_vendor_wave_expansion mapping table) ──

            // Cursor: all four kinds native under `.cursor` / `~/.cursor`
            // (Mcp is excluded from this file loop, handled by the vendor
            // MCP writers).
            (ConfigScope::Project, ClientTarget::Cursor, ArtifactKind::Skill) => {
                (PathAnchor::Workspace, format!(".cursor/skills/{name}"))
            }
            (ConfigScope::Project, ClientTarget::Cursor, ArtifactKind::Rule) => {
                (PathAnchor::Workspace, format!(".cursor/rules/{name}.mdc"))
            }
            (ConfigScope::Project, ClientTarget::Cursor, ArtifactKind::Agent) => {
                (PathAnchor::Workspace, format!(".cursor/agents/{name}.md"))
            }
            (ConfigScope::Global, ClientTarget::Cursor, ArtifactKind::Skill) => {
                (PathAnchor::VendorRoot("cursor"), format!("skills/{name}"))
            }
            (ConfigScope::Global, ClientTarget::Cursor, ArtifactKind::Rule) => {
                (PathAnchor::VendorRoot("cursor"), format!("rules/{name}.mdc"))
            }
            (ConfigScope::Global, ClientTarget::Cursor, ArtifactKind::Agent) => {
                (PathAnchor::VendorRoot("cursor"), format!("agents/{name}.md"))
            }

            // Kiro: skills + steering rules native (agents declined — skipped
            // before this table).
            (ConfigScope::Project, ClientTarget::Kiro, ArtifactKind::Skill) => {
                (PathAnchor::Workspace, format!(".kiro/skills/{name}"))
            }
            (ConfigScope::Project, ClientTarget::Kiro, ArtifactKind::Rule) => {
                (PathAnchor::Workspace, format!(".kiro/steering/{name}.md"))
            }
            (ConfigScope::Global, ClientTarget::Kiro, ArtifactKind::Skill) => {
                (PathAnchor::VendorRoot("kiro"), format!("skills/{name}"))
            }
            (ConfigScope::Global, ClientTarget::Kiro, ArtifactKind::Rule) => {
                (PathAnchor::VendorRoot("kiro"), format!("steering/{name}.md"))
            }

            // Junie: skills at both scopes; rules PROJECT-ONLY (degraded —
            // `paths` is dropped, but the file installs and loads). There is
            // no `~/.junie/rules/`, so the global triple has no entry here and
            // the loop skips it on an empty candidate set.
            (ConfigScope::Project, ClientTarget::Junie, ArtifactKind::Skill) => {
                (PathAnchor::Workspace, format!(".junie/skills/{name}"))
            }
            (ConfigScope::Project, ClientTarget::Junie, ArtifactKind::Rule) => {
                (PathAnchor::Workspace, format!(".junie/rules/{name}.md"))
            }
            (ConfigScope::Global, ClientTarget::Junie, ArtifactKind::Skill) => {
                (PathAnchor::VendorRoot("junie"), format!("skills/{name}"))
            }

            // Gemini: skills via the shared `.agents/skills` pool; agents
            // native under `.gemini` / `~/.gemini` (rules declined).
            (ConfigScope::Project, ClientTarget::Gemini, ArtifactKind::Skill) => {
                (PathAnchor::Workspace, format!(".agents/skills/{name}"))
            }
            (ConfigScope::Project, ClientTarget::Gemini, ArtifactKind::Agent) => {
                (PathAnchor::Workspace, format!(".gemini/agents/{name}.md"))
            }
            (ConfigScope::Global, ClientTarget::Gemini, ArtifactKind::Skill) => {
                (PathAnchor::AgentsSkills, name.to_string())
            }
            (ConfigScope::Global, ClientTarget::Gemini, ArtifactKind::Agent) => {
                (PathAnchor::VendorRoot("gemini"), format!("agents/{name}.md"))
            }

            // Zed: skills via the shared pool only (rules + agents declined).
            (ConfigScope::Project, ClientTarget::Zed, ArtifactKind::Skill) => {
                (PathAnchor::Workspace, format!(".agents/skills/{name}"))
            }
            (ConfigScope::Global, ClientTarget::Zed, ArtifactKind::Skill) => {
                (PathAnchor::AgentsSkills, name.to_string())
            }

            // Amp: skills via the shared pool only (rules + agents declined).
            (ConfigScope::Project, ClientTarget::Amp, ArtifactKind::Skill) => {
                (PathAnchor::Workspace, format!(".agents/skills/{name}"))
            }
            (ConfigScope::Global, ClientTarget::Amp, ArtifactKind::Skill) => {
                (PathAnchor::AgentsSkills, name.to_string())
            }

            // Generic client: the shared pool is its only surface (rules,
            // agents, and MCP all declined).
            (ConfigScope::Project, ClientTarget::Agents, ArtifactKind::Skill) => {
                (PathAnchor::Workspace, format!(".agents/skills/{name}"))
            }
            (ConfigScope::Global, ClientTarget::Agents, ArtifactKind::Skill) => {
                (PathAnchor::AgentsSkills, name.to_string())
            }

            // Antigravity: project skills join the shared pool and agents sit
            // beside them under `.agents`; GLOBAL skills are its own, under
            // `antigravity-root` (`~/.gemini/config`), NOT `AgentsSkills`.
            (ConfigScope::Project, ClientTarget::Antigravity, ArtifactKind::Skill) => {
                (PathAnchor::Workspace, format!(".agents/skills/{name}"))
            }
            (ConfigScope::Project, ClientTarget::Antigravity, ArtifactKind::Agent) => {
                (PathAnchor::Workspace, format!(".agents/agents/{name}.md"))
            }
            (ConfigScope::Global, ClientTarget::Antigravity, ArtifactKind::Skill) => {
                (PathAnchor::VendorRoot("antigravity"), format!("skills/{name}"))
            }
            (ConfigScope::Global, ClientTarget::Antigravity, ArtifactKind::Agent) => {
                (PathAnchor::VendorRoot("antigravity"), format!("agents/{name}.md"))
            }

            // ── Wave-2 skills-only batch ──
            // Own-directory clients: Workspace + dot-dir at project scope,
            // their own vendor root at global scope.
            (ConfigScope::Project, ClientTarget::Cline, ArtifactKind::Skill) => {
                (PathAnchor::Workspace, format!(".cline/skills/{name}"))
            }
            (ConfigScope::Global, ClientTarget::Cline, ArtifactKind::Skill) => {
                (PathAnchor::VendorRoot("cline"), format!("skills/{name}"))
            }
            // Droid: tag `droid-root`, directory `.factory` — deliberate.
            (ConfigScope::Project, ClientTarget::Droid, ArtifactKind::Skill) => {
                (PathAnchor::Workspace, format!(".factory/skills/{name}"))
            }
            (ConfigScope::Global, ClientTarget::Droid, ArtifactKind::Skill) => {
                (PathAnchor::VendorRoot("droid"), format!("skills/{name}"))
            }
            // Goose: the shared pool at BOTH scopes — no vendor root exists.
            (ConfigScope::Project, ClientTarget::Goose, ArtifactKind::Skill) => {
                (PathAnchor::Workspace, format!(".agents/skills/{name}"))
            }
            (ConfigScope::Global, ClientTarget::Goose, ArtifactKind::Skill) => {
                (PathAnchor::AgentsSkills, name.to_string())
            }
            // Warp: native by default. Pool-capable, so `candidate_anchors`
            // also offers `AgentsSkills` — but the native dest must still sort
            // to Warp's own root, which is what this row pins.
            (ConfigScope::Project, ClientTarget::Warp, ArtifactKind::Skill) => {
                (PathAnchor::Workspace, format!(".warp/skills/{name}"))
            }
            (ConfigScope::Global, ClientTarget::Warp, ArtifactKind::Skill) => {
                (PathAnchor::VendorRoot("warp"), format!("skills/{name}"))
            }
            // OpenClaw: GLOBAL ONLY. The project triple has no entry — the
            // loop skips it on `kind_surface`, exactly like global Junie rules.
            (ConfigScope::Global, ClientTarget::OpenClaw, ArtifactKind::Skill) => {
                (PathAnchor::VendorRoot("openclaw"), format!("skills/{name}"))
            }
            (ConfigScope::Project, ClientTarget::Kilo, ArtifactKind::Skill) => {
                (PathAnchor::Workspace, format!(".kilo/skills/{name}"))
            }
            (ConfigScope::Global, ClientTarget::Kilo, ArtifactKind::Skill) => {
                (PathAnchor::VendorRoot("kilo"), format!("skills/{name}"))
            }

            // Bundles are never materialised — exclude from the test loop.
            (_, _, ArtifactKind::Bundle) => unreachable!("bundles excluded from this loop"),
            // MCP descriptors register into client configs, not files —
            // their entry anchors land with the vendor MCP writers.
            (_, _, ArtifactKind::Mcp) => unreachable!("mcp excluded from this loop"),
            // Triples with no anchor never reach `from_target` — either the
            // vendor declines the kind outright, or (global Junie rules) it
            // hosts the kind but has no surface at that scope. Both yield an
            // empty candidate set, on which the test loop `continue`s before
            // calling this function — unreachable here.
            (_, ClientTarget::Kiro, ArtifactKind::Agent)
            | (_, ClientTarget::Junie, ArtifactKind::Rule)
            | (_, ClientTarget::Junie, ArtifactKind::Agent)
            | (_, ClientTarget::Gemini, ArtifactKind::Rule)
            | (_, ClientTarget::Zed, ArtifactKind::Rule)
            | (_, ClientTarget::Zed, ArtifactKind::Agent)
            | (_, ClientTarget::Amp, ArtifactKind::Rule)
            | (_, ClientTarget::Amp, ArtifactKind::Agent)
            | (_, ClientTarget::Agents, ArtifactKind::Rule)
            | (_, ClientTarget::Agents, ArtifactKind::Agent)
            | (_, ClientTarget::Antigravity, ArtifactKind::Rule)
            | (_, ClientTarget::Cline, ArtifactKind::Rule)
            | (_, ClientTarget::Cline, ArtifactKind::Agent)
            | (_, ClientTarget::Droid, ArtifactKind::Rule)
            | (_, ClientTarget::Droid, ArtifactKind::Agent)
            | (_, ClientTarget::Goose, ArtifactKind::Rule)
            | (_, ClientTarget::Goose, ArtifactKind::Agent)
            | (_, ClientTarget::Warp, ArtifactKind::Rule)
            | (_, ClientTarget::Warp, ArtifactKind::Agent)
            | (_, ClientTarget::OpenClaw, ArtifactKind::Rule)
            | (_, ClientTarget::OpenClaw, ArtifactKind::Agent)
            | (_, ClientTarget::Kilo, ArtifactKind::Rule)
            | (_, ClientTarget::Kilo, ArtifactKind::Agent)
            // OpenClaw has no project scope at all — `kind_surface` refuses it.
            | (ConfigScope::Project, ClientTarget::OpenClaw, ArtifactKind::Skill) => {
                unreachable!("declined (client, kind) pairs are skipped by the test loop before this call")
            }
        }
    }

    /// For every materializable (scope, client, kind) triple:
    ///
    /// 1. Derive the expected (anchor, relative) from the §1.1 table.
    /// 2. Build `dest`. **Project scope**: from
    ///    `client.path_for(&roots.workspace, …)` — the vendor trait itself
    ///    (every vendor's project-scope layout is a pure function of
    ///    `workspace`, no ambient env), so a divergence between
    ///    `vendor_x::{skills_root,rule_path,agent_path}` and this table's
    ///    hand-written `relative` string fails right here — ties copy 1
    ///    (the trait) to copy 2 (this table). **Global scope**: every
    ///    vendor's global root resolution reads live `$HOME` / vendor env
    ///    overrides directly inside `path_for`, not through `AnchorRoots` —
    ///    tying it into this hermetic fixture would make pass/fail depend
    ///    on the running machine's real environment instead of the table,
    ///    so `dest` still builds from
    ///    `expected_anchor.root(&roots).join(expected_relative)` as before.
    ///    Global-scope `path_for`/anchor-table parity is covered separately
    ///    by `path_for_global_scope_uses_vendor_native_roots` in
    ///    `client_target.rs` (guarded on the real `home_dir()`/env, the
    ///    established pattern for that half of the surface).
    /// 3. Assert `AnchoredPath::from_target(&dest, …)` classifies to the
    ///    expected (anchor, relative).
    /// 4. Assert `ap.resolve(&roots, Containment::Strict)` round-trips back to `dest`.
    ///
    /// No `continue` on `UnknownAnchor`: every combo MUST classify; a miss
    /// is a test failure.  The counter assertion at the end guarantees that
    /// adding a new client or kind without updating the table also fails.
    ///
    /// This locks `from_target` / `candidate_anchors` / `resolve` coherence,
    /// and — for project scope — `path_for` too. In particular, dropping the
    /// claude vendor root from `candidate_anchors(Global, Claude, Skill)` would
    /// cause assertion (3) to return `GrimHome` instead, failing this test.
    #[test]
    fn arch2_from_target_and_resolve_are_coherent_for_all_scope_client_kind_triples() {
        // Hermetic roots: fixed paths, so no env variable is consulted
        // during classification or resolution.
        // /oc is the OpenCode config root; /oc/skills is the skills root, so
        // OpenCodeRoot resolves to /oc (the parent of /oc/skills).
        //
        // Two pairs deliberately OVERLAP, mirroring the real layout: antigravity
        // roots INSIDE gemini (`~/.gemini/config` under `~/.gemini`), and the
        // OpenCode pair. This is fidelity, not extra detection strength — an
        // arm handing one client the other's root fails either way (disjoint
        // roots fail as `UnknownAnchor`, nested ones as an anchor mismatch).
        // The nesting is here so the fixture cannot drift into asserting
        // containment behaviour the real layout does not have.
        let roots = AnchorRoots {
            workspace: PathBuf::from("/ws"),
            grim_home: PathBuf::from("/grim"),
            vendor_roots: [
                ("claude", PathBuf::from("/claude")),
                ("copilot", PathBuf::from("/copilot")),
                ("codex", PathBuf::from("/codex")),
                ("cursor", PathBuf::from("/cursor")),
                ("kiro", PathBuf::from("/kiro")),
                ("junie", PathBuf::from("/junie")),
                ("gemini", PathBuf::from("/gemini")),
                ("zed", PathBuf::from("/zed")),
                ("amp", PathBuf::from("/amp")),
                // Nested under gemini's root ON PURPOSE — see the note above.
                ("antigravity", PathBuf::from("/gemini/config")),
                ("cline", PathBuf::from("/cline")),
                ("droid", PathBuf::from("/factory")),
                ("warp", PathBuf::from("/warp")),
                ("openclaw", PathBuf::from("/openclaw")),
                ("kilo", PathBuf::from("/kilo")),
            ]
            .into(),
            opencode_skills: Some(PathBuf::from("/oc/skills")),
            claude_user_dir: None,
            // /agents/skills is the shared skills root (every pool member);
            // the rest are each vendor's own config root.
            agents_skills: Some(PathBuf::from("/agents/skills")),
        };

        let name = "test-artifact";
        let scopes = [ConfigScope::Project, ConfigScope::Global];
        let clients = ClientTarget::ALL;
        let kinds = [ArtifactKind::Skill, ArtifactKind::Rule, ArtifactKind::Agent];
        // ArtifactKind::Bundle is excluded — bundles are never materialised.

        let mut combo_count = 0usize;

        for scope in scopes {
            for client in clients {
                for kind in kinds {
                    // A triple the installer never materializes has no
                    // anchor-remainder entry. This mirrors `installer.rs`'s
                    // `client_supports_kind` (minus its Mcp arm — Mcp is not in
                    // `kinds`), so it covers both producers: an outright
                    // decline (Codex rules) and a kind the vendor hosts but has
                    // no surface for at this scope (global Junie rules, which
                    // are `Degraded` — `kind_support` alone would wrongly
                    // demand a table entry for them).
                    //
                    // NOT keyed on an empty candidate set: project scope is
                    // unconditionally `[Workspace]` for every triple, declined
                    // ones included, so that predicate silently stops skipping
                    // anything at project scope. The count assertion below is
                    // what stops this from quietly shrinking coverage.
                    let vendor = client.vendor();
                    let hosted = vendor.kind_support(kind) != crate::install::vendor::KindSupport::Declined
                        && vendor.kind_surface(kind, scope);
                    if !hosted {
                        continue;
                    }
                    combo_count += 1;

                    // Step 1: expected anchor + relative from the §1.1 table.
                    let (expected_anchor, expected_relative) = expected_anchor_and_relative(scope, client, kind, name);

                    // Step 2: build the absolute dest. The anchor root must
                    // resolve (all roots are Some in this fixture) — an
                    // unwrap failure here means the table entry references
                    // an anchor whose root is absent, which is a bug in the
                    // test table itself.
                    let anchor_root = expected_anchor.root(&roots).unwrap_or_else(|| {
                        panic!(
                            "anchor {expected_anchor:?} has no root in hermetic fixture \
                             for ({scope:?}, {client:?}, {kind:?})"
                        )
                    });
                    // Project scope: drive `dest` from the vendor trait
                    // directly (see the function doc) — ties `path_for` to
                    // the `expected_anchor_and_relative` table. Global
                    // scope: `path_for`'s vendor roots read live env, so
                    // stay on the hermetic anchor-root composition.
                    let dest = match scope {
                        ConfigScope::Project => client.path_for(&roots.workspace, scope, kind, name),
                        ConfigScope::Global => anchor_root.join(&expected_relative),
                    };

                    // Step 3: from_target must classify to the expected pair —
                    // NO silent skip on UnknownAnchor; every combo must match.
                    let ap = AnchoredPath::from_target(&dest, scope, client, kind, &roots).unwrap_or_else(|e| {
                        panic!(
                            "from_target returned {e:?} for ({scope:?}, {client:?}, {kind:?}): \
                             expected anchor={expected_anchor:?} relative={expected_relative:?}"
                        )
                    });

                    assert_eq!(
                        ap.anchor, expected_anchor,
                        "anchor mismatch for ({scope:?}, {client:?}, {kind:?}): \
                         expected {expected_anchor:?}, got {:?}",
                        ap.anchor
                    );
                    assert_eq!(
                        ap.relative, expected_relative,
                        "relative mismatch for ({scope:?}, {client:?}, {kind:?}): \
                         expected {expected_relative:?}, got {:?}",
                        ap.relative
                    );

                    // Step 4: resolve must round-trip back to dest (absent
                    // path → Layer 2 skipped → raw join == dest).
                    let resolved = ap
                        .resolve(&roots, Containment::Strict)
                        .unwrap_or_else(|e| panic!("resolve failed for ({scope:?}, {client:?}, {kind:?}): {e:?}"));
                    assert_eq!(
                        resolved, dest,
                        "resolve round-trip mismatch for ({scope:?}, {client:?}, {kind:?})"
                    );
                }
            }
        }

        // Exhaustiveness guard: 2 scopes × 18 clients × 3 kinds = 108, minus
        // the 23 declined (client, kind) pairs × 2 scopes = 46, minus the two
        // scope gaps that are NOT declines — global Junie-Rule and project
        // OpenClaw-Skill, both refused by `kind_surface` — → 60 combos.
        //
        // The 23: Codex-Rule, Kiro-Agent, Junie-Agent, Gemini-Rule,
        // Zed-Rule/Agent, Amp-Rule/Agent, Agents-Rule/Agent, Antigravity-Rule,
        // and Rule+Agent for each of the six skills-only wave-2 clients
        // (Cline, Droid, Goose, Warp, OpenClaw, Kilo).
        //
        // If a new ClientTarget or ArtifactKind variant is added, this fails,
        // forcing the table to be extended.
        assert_eq!(
            combo_count, 60,
            "expected 60 (scope × client × kind) combos but counted {combo_count}; \
             update the table in expected_anchor_and_relative() and this assertion"
        );
    }
}
