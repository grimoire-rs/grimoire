// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The kinds of artifact Grimoire manages: skills, rules, agents,
//! bundles, MCP server descriptors, and hooks.

use serde::{Deserialize, Serialize};

/// The manifest annotation key carrying the artifact kind (`"skill"` /
/// `"rule"` / `"agent"` / `"bundle"`). Since `adr_oci_empty_config_compat.md`
/// this is the **sole discriminator grim writes** — no custom `artifactType`
/// or config media type reaches the wire (GitLab rejects both). On the read
/// path it is the third (last) resolution tier, after the legacy `artifactType`
/// and legacy config media type that pre-ADR artifacts still carry. The value
/// matches the pre-ADR format so even older grim readers resolve a new
/// artifact's kind.
pub const KIND_ANNOTATION: &str = "com.grimoire.kind";

/// A Grimoire-managed artifact kind.
///
/// Closed internal enum: the binary is the only consumer, so matches stay
/// total — no `#[non_exhaustive]`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactKind {
    /// An Agent Skill: a `SKILL.md` directory with YAML frontmatter.
    ///
    /// Also the `Default`: the lock layer's `LockedArtifact::kind` is
    /// `#[serde(skip)]` and re-stamped from the array it was read from, so
    /// the deserialization placeholder is never observed.
    #[default]
    Skill,
    /// A rule: a single `paths:`-scoped markdown file.
    Rule,
    /// An agent: a single markdown file whose required frontmatter
    /// (`name`, `description`) plus optional common fields (`model`,
    /// `tools`) define an AI agent; the body is the system prompt.
    /// Projected per client at install time.
    Agent,
    /// A bundle: a curated set of members (`skill`, `rule`, `agent`, `mcp`
    /// or `hook`), declared in
    /// `[bundles]` and expanded into its members at resolve time. A bundle
    /// is never materialized or written to the lock itself — only the
    /// members it expands to are.
    Bundle,
    /// An MCP server descriptor: a vendor-agnostic definition of a Model
    /// Context Protocol server (transport, command/url, env). Installs by
    /// registering an entry in each client's native MCP config file —
    /// never as a materialized file of its own.
    Mcp,
    /// A hook: a directory artifact whose `hook.toml` manifest declares
    /// lifecycle handlers (see [`crate::oci::hook`]). The payload
    /// materializes once per scope; activation is a grim-owned dispatcher
    /// entry registered in each client's own hook surface, never a
    /// per-client copy of the payload.
    Hook,
}

impl ArtifactKind {
    /// Every kind, in declaration order.
    ///
    /// The single spelled-out variant list in this module: both
    /// [`from_artifact_type`](Self::from_artifact_type) and
    /// [`from_config_media_type`](Self::from_config_media_type) derive their
    /// candidate set from it rather than repeating a literal array, so a kind
    /// can no longer be added to one reverse lookup and forgotten in the other
    /// (C-016(a) in `plan_hooks_artifact_kind.md`). An array is not itself
    /// compiler-checked for exhaustiveness — [`all_index`](Self::all_index) is
    /// the total `match` that makes a new variant a `cargo check` failure here.
    pub const ALL: [Self; 6] = [
        Self::Skill,
        Self::Rule,
        Self::Agent,
        Self::Bundle,
        Self::Mcp,
        Self::Hook,
    ];

    /// This kind's position in [`ALL`](Self::ALL).
    ///
    /// Exists only as the compile-time anchor for that array: a total match, so
    /// adding a variant is a `cargo check` failure until an arm exists here,
    /// and the `const` block below then pins every arm to its own index.
    ///
    /// **What this does and does not guarantee.** It guarantees the *two*
    /// reverse lookups ([`from_artifact_type`](Self::from_artifact_type),
    /// [`from_config_media_type`](Self::from_config_media_type)) and
    /// [`from_kind_str`](Self::from_kind_str) all derive from **one** list, so
    /// a kind can no longer be added to one and forgotten in the other
    /// (C-016(a)). It does **not** tie [`ALL`](Self::ALL)'s length to the
    /// variant count, and the escape is wider than the obvious one: a seventh
    /// variant compiles as long as its arm here returns **any** value —
    /// `6` (the loop never reaches index 6), *or* a value duplicating an
    /// existing index. Copy-pasting a neighbouring arm to `3` leaves
    /// `ALL[3] == Bundle` and `Bundle.all_index() == 3`, every assertion
    /// passes, `all_index` is silently non-injective, and the new kind is
    /// absent from every lookup. Completeness and injectivity are runtime
    /// assertions the Specify phase owns, not compile-time properties. Never
    /// used for ordering or serialization.
    const fn all_index(self) -> usize {
        match self {
            Self::Skill => 0,
            Self::Rule => 1,
            Self::Agent => 2,
            Self::Bundle => 3,
            Self::Mcp => 4,
            Self::Hook => 5,
        }
    }

    /// The lowercase kind string (`skill`/`rule`/`agent`/`bundle`/`mcp`/
    /// `hook`) — the value of the [`KIND_ANNOTATION`] and of the `--kind` CLI
    /// flag. The single source of truth for the spelling: [`Display`] and
    /// [`from_kind_str`](Self::from_kind_str) both go through it.
    ///
    /// [`Display`]: std::fmt::Display
    pub fn kind_str(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::Rule => "rule",
            Self::Agent => "agent",
            Self::Bundle => "bundle",
            Self::Mcp => "mcp",
            Self::Hook => "hook",
        }
    }

    /// The `$GRIM_HOME`/install subdirectory for this kind.
    pub fn subdir(self) -> &'static str {
        match self {
            Self::Skill => "skills",
            Self::Rule => "rules",
            Self::Agent => "agents",
            Self::Bundle => "bundles",
            Self::Mcp => "mcp",
            Self::Hook => "hooks",
        }
    }

    /// Parse the lowercase kind string
    /// (`skill`/`rule`/`agent`/`bundle`/`mcp`/`hook`) into a kind.
    /// `None` for any other string. Used to interpret the `--kind` CLI flag
    /// and the `com.grimoire.kind` annotation (the on-the-wire discriminator,
    /// see [`KIND_ANNOTATION`]); this string is not itself the wire format.
    pub fn from_kind_str(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.kind_str() == s)
    }

    /// The OCI `artifactType` media type for this kind. No longer stamped on
    /// the wire (GitLab's allowlist rejects a custom `artifactType`, see
    /// `adr_oci_empty_config_compat.md`); used only on the READ path to type
    /// artifacts published before that change. The single source of truth for
    /// the per-kind type string.
    pub fn artifact_type(self) -> &'static str {
        match self {
            Self::Skill => "application/vnd.grimoire.skill.v1",
            Self::Rule => "application/vnd.grimoire.rule.v1",
            Self::Agent => "application/vnd.grimoire.agent.v1",
            Self::Bundle => "application/vnd.grimoire.bundle.v1",
            Self::Mcp => "application/vnd.grimoire.mcp.v1",
            Self::Hook => "application/vnd.grimoire.hook.v1",
        }
    }

    /// The legacy per-kind OCI config-descriptor media type. No longer stamped
    /// on the wire (new manifests carry the OCI empty config — GitLab rejects a
    /// custom config type, see `adr_oci_empty_config_compat.md`); used only on
    /// the READ path as the second kind-resolution tier for artifacts published
    /// before that change.
    pub fn config_media_type(self) -> &'static str {
        match self {
            Self::Skill => "application/vnd.grimoire.skill.config.v1+json",
            Self::Rule => "application/vnd.grimoire.rule.config.v1+json",
            Self::Agent => "application/vnd.grimoire.agent.config.v1+json",
            Self::Bundle => "application/vnd.grimoire.bundle.config.v1+json",
            // Mcp and Hook postdate the legacy artifactType/config-media-type
            // wire formats; the strings exist only to keep these methods total.
            Self::Mcp => "application/vnd.grimoire.mcp.config.v1+json",
            Self::Hook => "application/vnd.grimoire.hook.config.v1+json",
        }
    }

    /// Parse an OCI `artifactType` media type back into a kind. `None` for any
    /// non-Grimoire type.
    pub fn from_artifact_type(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.artifact_type() == s)
    }

    /// Parse an OCI config media type back into a kind (the fallback read
    /// path). `None` for the generic OCI image config or any non-Grimoire type.
    pub fn from_config_media_type(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.config_media_type() == s)
    }

    /// Whether the artifact materializes as a directory tree (skill, hook)
    /// rather than a single file (rule, agent). Bundles never materialize; MCP
    /// descriptors register into client configs instead of materializing. A
    /// hook does both: the payload directory materializes once per scope, and
    /// the registration is a derived projection on top of it.
    #[allow(
        dead_code,
        reason = "exercised directly by this module's tests; install/materializer call sites match ArtifactKind inline instead"
    )]
    pub fn is_dir_artifact(self) -> bool {
        match self {
            Self::Skill | Self::Hook => true,
            Self::Rule | Self::Agent | Self::Bundle | Self::Mcp => false,
        }
    }
}

/// Compile-time consistency check binding [`ArtifactKind::ALL`] to the total
/// match in [`ArtifactKind::all_index`]: every entry sits at its own index.
/// Adding a variant forces an `all_index` arm (a `cargo check` failure until
/// it exists), and its doc comment points the author at `ALL`.
const _: () = {
    let mut i = 0;
    while i < ArtifactKind::ALL.len() {
        assert!(ArtifactKind::ALL[i].all_index() == i);
        i += 1;
    }
};

impl std::fmt::Display for ArtifactKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.kind_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subdir_and_dir_artifact() {
        assert_eq!(ArtifactKind::Skill.subdir(), "skills");
        assert_eq!(ArtifactKind::Rule.subdir(), "rules");
        assert_eq!(ArtifactKind::Agent.subdir(), "agents");
        assert_eq!(ArtifactKind::Bundle.subdir(), "bundles");
        assert_eq!(ArtifactKind::Mcp.subdir(), "mcp");
        assert_eq!(ArtifactKind::Hook.subdir(), "hooks");
        assert!(ArtifactKind::Skill.is_dir_artifact());
        assert!(!ArtifactKind::Rule.is_dir_artifact());
        assert!(!ArtifactKind::Agent.is_dir_artifact());
        assert!(!ArtifactKind::Bundle.is_dir_artifact());
        assert!(!ArtifactKind::Mcp.is_dir_artifact());
        // A hook is a directory artifact: `hook.toml` plus the payload tree its
        // handlers invoke (C-001).
        assert!(ArtifactKind::Hook.is_dir_artifact());
    }

    /// The runtime half of C-016(a). `all_index`'s total `match` forces an arm
    /// for a new variant, but it does not force a *distinct* one — an arm
    /// duplicating a neighbour's index leaves every `const` assertion passing
    /// while `ALL` silently omits the kind, and with it all three reverse
    /// lookups. Completeness and injectivity are runtime properties, asserted
    /// here where the compiler cannot help.
    #[test]
    fn all_is_complete_and_injective() {
        let subdirs: std::collections::BTreeSet<_> = ArtifactKind::ALL.iter().map(|k| k.subdir()).collect();
        let kinds: std::collections::BTreeSet<_> = ArtifactKind::ALL.iter().map(|k| k.kind_str()).collect();
        let types: std::collections::BTreeSet<_> = ArtifactKind::ALL.iter().map(|k| k.artifact_type()).collect();
        let configs: std::collections::BTreeSet<_> = ArtifactKind::ALL.iter().map(|k| k.config_media_type()).collect();
        for (label, set) in [
            ("subdir", &subdirs),
            ("kind_str", &kinds),
            ("artifact_type", &types),
            ("config_media_type", &configs),
        ] {
            assert_eq!(
                set.len(),
                ArtifactKind::ALL.len(),
                "two kinds share a {label} — a copy-pasted arm, and one of them is unreachable"
            );
        }
    }

    #[test]
    fn from_kind_str_round_trips_and_rejects_unknown() {
        assert_eq!(ArtifactKind::from_kind_str("skill"), Some(ArtifactKind::Skill));
        assert_eq!(ArtifactKind::from_kind_str("rule"), Some(ArtifactKind::Rule));
        assert_eq!(ArtifactKind::from_kind_str("agent"), Some(ArtifactKind::Agent));
        assert_eq!(ArtifactKind::from_kind_str("bundle"), Some(ArtifactKind::Bundle));
        assert_eq!(ArtifactKind::from_kind_str("mcp"), Some(ArtifactKind::Mcp));
        assert_eq!(ArtifactKind::from_kind_str("hook"), Some(ArtifactKind::Hook));
        assert_eq!(ArtifactKind::from_kind_str("Skill"), None);
        assert_eq!(ArtifactKind::from_kind_str("widget"), None);
        // Display ⇄ from_kind_str round-trip for every kind. Driven off `ALL`,
        // not a second hand-maintained array — that array shape is D-5, the
        // reason a new kind can be added and silently skipped by a test that
        // still passes.
        for k in ArtifactKind::ALL {
            assert_eq!(ArtifactKind::from_kind_str(&k.to_string()), Some(k));
        }
    }

    #[test]
    fn artifact_type_and_config_media_type_round_trip() {
        for k in ArtifactKind::ALL {
            assert_eq!(ArtifactKind::from_artifact_type(k.artifact_type()), Some(k));
            assert_eq!(ArtifactKind::from_config_media_type(k.config_media_type()), Some(k));
        }
        // Exact wire strings (the published contract).
        assert_eq!(ArtifactKind::Skill.artifact_type(), "application/vnd.grimoire.skill.v1");
        assert_eq!(
            ArtifactKind::Skill.config_media_type(),
            "application/vnd.grimoire.skill.config.v1+json"
        );
        assert_eq!(ArtifactKind::Agent.artifact_type(), "application/vnd.grimoire.agent.v1");
        assert_eq!(
            ArtifactKind::Agent.config_media_type(),
            "application/vnd.grimoire.agent.config.v1+json"
        );
        // The generic OCI image config and foreign types are not a kind.
        assert_eq!(
            ArtifactKind::from_config_media_type("application/vnd.oci.image.config.v1+json"),
            None
        );
        // The OCI empty config type (the new default config descriptor since
        // `adr_oci_empty_config_compat.md`) is NOT a kind — the read path must
        // fall through to the `artifactType` / annotation tiers, never infer a
        // kind from the empty config blob.
        assert_eq!(
            ArtifactKind::from_config_media_type("application/vnd.oci.empty.v1+json"),
            None
        );
        assert_eq!(
            ArtifactKind::from_artifact_type("application/vnd.cncf.helm.config.v1+json"),
            None
        );
    }

    #[test]
    fn display_and_serde_are_lowercase_and_agree() {
        assert_eq!(ArtifactKind::Skill.to_string(), "skill");
        assert_eq!(ArtifactKind::Rule.to_string(), "rule");
        assert_eq!(ArtifactKind::Agent.to_string(), "agent");
        assert_eq!(ArtifactKind::Bundle.to_string(), "bundle");
        assert_eq!(ArtifactKind::Mcp.to_string(), "mcp");
        assert_eq!(ArtifactKind::Hook.to_string(), "hook");
        assert_eq!(
            serde_json::from_str::<ArtifactKind>("\"hook\"").unwrap(),
            ArtifactKind::Hook
        );
        assert_eq!(
            serde_json::from_str::<ArtifactKind>("\"mcp\"").unwrap(),
            ArtifactKind::Mcp
        );
        assert_eq!(serde_json::to_string(&ArtifactKind::Skill).unwrap(), "\"skill\"");
        assert_eq!(
            serde_json::from_str::<ArtifactKind>("\"agent\"").unwrap(),
            ArtifactKind::Agent
        );
        assert_eq!(
            serde_json::from_str::<ArtifactKind>("\"rule\"").unwrap(),
            ArtifactKind::Rule
        );
        assert_eq!(
            serde_json::from_str::<ArtifactKind>("\"bundle\"").unwrap(),
            ArtifactKind::Bundle
        );
    }
}
