// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Resolution-tier errors.
//!
//! Three-layer shape: top [`crate::error::Error`] → context-bearing
//! [`ResolveError`] (carries the boxed [`ArtifactRef`] the failure is
//! about — boxed to keep the kind small, mirroring OCX's precedent) →
//! discriminant [`ResolveErrorKind`].

use crate::config::declaration::DeclaredSource;
use crate::config::scope::Invocation;
use crate::oci::access::error::AccessError;
use crate::oci::features::ArtifactFeatureError;
use crate::oci::reference::ArtifactRef;
use crate::oci::text::{escape_controls, registry_text};
use crate::oci::{ArtifactKind, Digest, Identifier};

/// Registry of the placeholder identity an error carries when its name has
/// no real identifier (an undeclared name, an unparseable member id).
/// [`ResolveError`]'s `Display` never prints it.
const PLACEHOLDER_REGISTRY: &str = "invalid.localhost";

/// Most nodes reached through a requires edge (or a bundle reached through one) in one resolve.
pub const MAX_TRANSITIVE_NODES: usize = 2048;
/// Most requires and nested-bundle layer bytes fetched in one resolve.
pub const MAX_REQUIRES_BYTES: u64 = 32 * 1024 * 1024;
/// Most breadth-first levels below the roots.
pub const MAX_DEPTH: usize = 32;

/// The graph cap a resolve crossed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphLimit {
    /// [`MAX_TRANSITIVE_NODES`].
    Nodes,
    /// [`MAX_REQUIRES_BYTES`].
    RequiresBytes,
    /// [`MAX_DEPTH`].
    Depth,
}

impl std::fmt::Display for GraphLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nodes => write!(f, "more than {MAX_TRANSITIVE_NODES} transitive artifacts"),
            Self::RequiresBytes => write!(f, "more than {MAX_REQUIRES_BYTES} bytes of requires layers"),
            Self::Depth => write!(f, "deeper than {MAX_DEPTH} levels"),
        }
    }
}

/// `skill/a -> skill/b -> skill/a`: the cycle closed back on its first node.
fn cycle_path(cycle: &[(ArtifactKind, String)]) -> String {
    let mut nodes: Vec<String> = cycle
        .iter()
        .map(|(kind, name)| format!("{kind}/{}", registry_text(name)))
        .collect();
    nodes.extend(nodes.first().cloned());
    nodes.join(" -> ")
}

/// The closing edge of a cycle: its last node back to its first.
fn closing_edge(cycle: &[(ArtifactKind, String)]) -> String {
    let node = |n: Option<&(ArtifactKind, String)>| {
        n.map_or_else(String::new, |(k, name)| format!("{k}/{}", registry_text(name)))
    };
    format!("{} -> {}", node(cycle.last()), node(cycle.first()))
}

/// A resolution failed for one declared artifact.
#[derive(Debug)]
pub struct ResolveError {
    /// The artifact the failure is about. Boxed so [`ResolveErrorKind`]
    /// stays small (avoids a `clippy::result_large_err` suppression).
    pub reference: Box<ArtifactRef>,
    /// The specific failure.
    pub kind: ResolveErrorKind,
    /// The artifact whose requires edge led here, with its publisher, when
    /// the failing artifact is a dependency rather than a declaration.
    pub required_by: Option<String>,
    /// The `grim` the remedy names. The graph that raises the error holds no
    /// scope; [`Self::naming`] fills this in where one is known.
    pub grim: Invocation,
}

impl ResolveError {
    /// Construct from an artifact reference and a failure kind.
    pub fn new(reference: ArtifactRef, kind: ResolveErrorKind) -> Self {
        Self {
            reference: Box::new(reference),
            kind,
            required_by: None,
            grim: Invocation::WALKED,
        }
    }

    /// Name `grim` in this error's remedy.
    #[must_use]
    pub fn naming(mut self, grim: &Invocation) -> Self {
        self.grim = grim.clone();
        self
    }

    /// Name the parent whose requires edge led to this failure.
    #[must_use]
    pub fn required_by(mut self, parent: impl Into<String>) -> Self {
        self.required_by = Some(parent.into());
        self
    }

    /// An error about `name` when no real identifier exists for it. The
    /// reference carries a placeholder identity that `Display` never prints.
    pub fn unidentified(artifact: ArtifactKind, name: impl Into<String>, kind: ResolveErrorKind) -> Self {
        let name = name.into();
        let id = Identifier::new_registry(name.clone(), PLACEHOLDER_REGISTRY);
        Self::new(ArtifactRef::registry(artifact, name, id), kind)
    }

    /// Whether the reference is [`Self::unidentified`]'s placeholder.
    fn is_unidentified(&self) -> bool {
        matches!(&self.reference.source, DeclaredSource::Registry(id) if id.registry() == PLACEHOLDER_REGISTRY)
    }

    /// The digest the reference asks the registry for, when it names one.
    fn requested_digest(&self) -> Option<Digest> {
        match &self.reference.source {
            DeclaredSource::Registry(id) => id.digest(),
            DeclaredSource::Path(_) => None,
        }
    }

    /// `grim add` binds the repository's last segment, so declaring a
    /// conflicting binding its repository does not name needs `--name`.
    fn add_name_hint(&self) -> Option<String> {
        let ResolveErrorKind::BundleConflict { name, .. } = &self.kind else {
            return None;
        };
        let DeclaredSource::Registry(id) = &self.reference.source else {
            return None;
        };
        (id.name() != name).then(|| {
            format!(
                "its repository is named {}, so `{} add` needs `--name {}`",
                registry_text(id.name()),
                self.grim,
                registry_text(name)
            )
        })
    }
}

/// How to move a registry pin whose manifest is gone. A pin a digest names
/// cannot move: `grim update` asks for that digest again.
pub(crate) fn gone_pin_remedy(grim: &Invocation, name: &str) -> String {
    format!(
        "run `{}` to move this pin. If the lock no longer matches what is declared or another pin is gone \
         too, `{grim} update` with no names moves every pin. If its tag is gone too, or a digest names it, \
         declare a reference that exists in grimoire.toml and run `{grim} lock`",
        grim.update(name)
    )
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A placeholder's kind and identity are invented: name only.
        if self.is_unidentified() {
            return write!(f, "'{}': {}", escape_controls(&self.reference.name), self.kind);
        }
        match self.requested_digest() {
            // A digest names one manifest: no tag was asked for, so none is missing.
            Some(digest) if matches!(self.kind, ResolveErrorKind::TagNotFound) => {
                write!(f, "{}: manifest {digest} not found", self.reference)?;
            }
            _ => write!(f, "{}: {}", self.reference, self.kind)?,
        }
        if let ResolveErrorKind::PinnedManifestGone { name, .. } = &self.kind {
            write!(f, "; {}", gone_pin_remedy(&self.grim, name))?;
        }
        if let Some(hint) = self.add_name_hint() {
            write!(f, "; {hint}")?;
        }
        match &self.required_by {
            Some(parent) => write!(f, "; required by {}", registry_text(parent)),
            None => Ok(()),
        }
    }
}

impl std::error::Error for ResolveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // `Display` already embeds the kind's message; expose the kind's own
        // cause so `{:#}` chains do not print the kind twice.
        self.kind.source()
    }
}

/// What a plugin's conflict on `name` asks the reader to change in `manifest`.
fn plugin_conflict_fix(
    kind: ArtifactKind,
    name: &str,
    plugin: &str,
    manifest: &std::path::Path,
    from_bundles: bool,
) -> String {
    if from_bundles {
        return format!(
            "drop one of these bundles from plugin '{}', or include versions of them that agree",
            registry_text(plugin)
        );
    }
    format!(
        "include {kind} {} directly in plugin '{}' of {} to choose one; \
         a bundle that lists another version still conflicts",
        registry_text(name),
        registry_text(plugin),
        escape_controls(&manifest.display().to_string())
    )
}

/// Inner discriminant for resolution-tier failures.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ResolveErrorKind {
    /// The declared tag does not exist on the registry (`Ok(None)` from
    /// the access layer). Not retried.
    #[error("tag not found")]
    TagNotFound,

    /// A requested name is not declared (not a direct entry, path entry,
    /// or bundle member). Classified like [`Self::TagNotFound`] (79).
    #[error("not declared")]
    NotDeclared,

    /// A carried pin's manifest is gone from the registry. Only `grim update`
    /// may move a pin, so a relock refuses rather than resolve past it.
    /// Classified like [`Self::TagNotFound`] (79). [`ResolveError`] prints the
    /// remedy, which names the run's `grim`.
    #[error("the registry no longer holds the pinned manifest {digest}")]
    PinnedManifestGone { digest: Digest, name: String },

    /// [`Self::PinnedManifestGone`] in a marketplace part, where only a
    /// `--marketplace` update moves a pin; `retry` names that command. It reads
    /// the include's tag again, so a gone tag needs the include in `manifest`
    /// changed. Classified like [`Self::TagNotFound`] (79).
    #[error(
        "the registry no longer holds the pinned manifest {digest}; run {} to move the pin. If its tag is gone too, \
         or a digest names it, change the include that reaches it in `{}`",
        escape_controls(.retry),
        escape_controls(&.manifest.display().to_string())
    )]
    MarketplacePinGone {
        digest: Digest,
        retry: String,
        manifest: std::path::PathBuf,
    },

    /// The registry rejected the request for authentication reasons.
    /// Terminal — not retried.
    #[error("authentication failed")]
    AuthFailure(#[source] AccessError),

    /// The registry was unreachable after exhausting the retry budget, or
    /// an offline miss blocked the resolve.
    #[error("registry unreachable")]
    RegistryUnreachable(#[source] AccessError),

    /// Resolution for one artifact exceeded the per-artifact timeout.
    #[error("resolve timed out")]
    ResolveTimeout,

    /// A declared bundle could not be fetched: its tag did not resolve, or
    /// its manifest/layer was missing on the registry.
    #[error("bundle not found")]
    BundleNotFound,

    /// A bundle resolved but its members document is malformed (missing
    /// layer, invalid JSON, a nested bundle, or an unparseable member id).
    #[error("invalid bundle: {}", escape_controls(.0))]
    BundleInvalid(String),

    /// A local bundle file could not be read or parsed. The file is the user's
    /// own, so its cause keeps its lines, where [`Self::BundleInvalid`] escapes
    /// registry text that could forge one. Classified like it (65).
    #[error("invalid bundle")]
    LocalBundleInvalid(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The same `(kind, name)` is claimed by two or more bundles or requires
    /// edges that resolve to different digests. Fail-closed: the user resolves
    /// it by declaring the artifact directly to choose one.
    ///
    /// `others` names every *further* conflicting artifact found in the same
    /// pass (empty when this is the only one), so one run reports the whole
    /// set instead of one per run. It sits before the fix so the message
    /// always ends with it.
    #[error(
        "conflicting versions ({}){}; declare {kind} {} directly in grimoire.toml to choose one",
        escape_controls(.sources),
        escape_controls(.others),
        registry_text(.name)
    )]
    BundleConflict {
        kind: ArtifactKind,
        name: String,
        sources: String,
        others: String,
        /// The `registry/repository` of each request in conflict on `name`,
        /// sorted and deduplicated: where declaring it picks a side.
        locations: Vec<String>,
        /// Whether every request in conflict on `name` comes from a bundle's list.
        from_bundles: bool,
    },

    /// [`Self::BundleConflict`] in a declared plugin of a marketplace manifest, where the
    /// fix is an include in `manifest`, not a line of `grimoire.toml`. A direct include
    /// shadows a `requires` edge, but a bundle that lists another version of it still
    /// conflicts, and when every request comes from a bundle's list no include settles it:
    /// one of the bundles goes, or both list versions that agree. Classified like
    /// [`Self::BundleConflict`] (78).
    #[error(
        "conflicting versions ({}){}; {}",
        escape_controls(.sources),
        escape_controls(.others),
        plugin_conflict_fix(*.kind, .name, .plugin, .manifest, *.from_bundles)
    )]
    PluginConflict {
        kind: ArtifactKind,
        name: String,
        sources: String,
        others: String,
        plugin: String,
        manifest: std::path::PathBuf,
        /// Whether every request in conflict on `name` comes from a bundle's list.
        from_bundles: bool,
    },

    /// A part pick keeps the dependent the reference names at its pin, which requires a
    /// version of a row or bundle the pick moves. The part would hold the pair together
    /// only until `grim export` reads it. `retry` names the pick that takes the dependent
    /// too and the update that moves the whole plugin; the marketplace seam fills it in.
    /// Classified 78.
    #[error(
        "requires {} at its kept pin; this pick moves {} to {}; {}",
        escape_controls(.required),
        escape_controls(.moved),
        escape_controls(.to),
        escape_controls(.retry)
    )]
    HeldRequirement {
        /// The requirement as the dependent's manifest names it.
        required: String,
        /// The row or bundle the pick moves, as `skill 'name'` or `bundle 'name'`.
        moved: String,
        /// Where the pick moves it.
        to: String,
        /// The dependent's binding name.
        dependent: String,
        /// What to run instead, filled in by the caller that knows the plugin.
        retry: String,
    },

    /// The requires edges form a cycle. `cycle` lists each node once,
    /// smallest key first; the edge from its last node back to its first is
    /// the one `publisher` (that last node's repository) published.
    #[error(
        "dependency cycle: {}; the edge {} is published by {}; contact that publisher",
        cycle_path(.cycle),
        closing_edge(.cycle),
        registry_text(.publisher)
    )]
    DependencyCycle {
        cycle: Vec<(ArtifactKind, String)>,
        publisher: String,
    },

    /// A graph cap was crossed. `via` names the parent whose edges crossed
    /// it, with its publisher repository, so the user knows whom to contact.
    #[error(
        "dependency graph too large: {limit}; the edges of {} crossed the limit; contact that publisher",
        registry_text(.via)
    )]
    GraphTooLarge { limit: GraphLimit, via: String },

    /// The artifact needs features, or uses a layer layout, this grim does not know.
    #[error("{0}")]
    UnsupportedFeature(ArtifactFeatureError),

    /// A local path source failed to validate or pack at lock time: the
    /// path does not exist, the artifact fails kind validation, or a
    /// tool-namespaced metadata literal is invalid.
    #[error("local source invalid")]
    LocalSource(#[source] Box<crate::skill::SkillError>),

    /// A named run refused: the predecessor lock's declaration hash does not
    /// match the current declaration. Both are surfaced so an operator can
    /// diff the lock against the live config. `retry` names what to run, and
    /// the message ends with it.
    #[error(
        "the lock no longer matches what is declared (lock hash {}, declared hash {}); run {}",
        escape_controls(crate::lock::pin_rule::printed_hash(.previous_hash)),
        escape_controls(.current_hash),
        escape_controls(.retry)
    )]
    StaleLock {
        previous_hash: String,
        current_hash: String,
        retry: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact_ref() -> ArtifactRef {
        ArtifactRef::registry(
            ArtifactKind::Skill,
            "code-review",
            Identifier::parse("ghcr.io/acme/code-review:stable").unwrap(),
        )
    }

    #[test]
    fn display_includes_artifact_context() {
        let err = ResolveError::new(artifact_ref(), ResolveErrorKind::TagNotFound);
        let s = err.to_string();
        assert!(s.contains("skill"));
        assert!(s.contains("code-review"));
        assert!(s.contains("tag not found"));
    }

    #[test]
    fn unidentified_display_never_prints_the_placeholder() {
        let stale = ResolveErrorKind::StaleLock {
            previous_hash: "sha256:a".to_string(),
            current_hash: "sha256:b".to_string(),
            retry: "a full resolve".to_string(),
        };
        for kind in [ResolveErrorKind::NotDeclared, stale] {
            let s = ResolveError::unidentified(ArtifactKind::Skill, "ghost", kind).to_string();
            assert!(!s.contains(PLACEHOLDER_REGISTRY), "{s}");
            assert!(s.starts_with("'ghost': "), "{s}");
        }
        assert_eq!(
            ResolveError::unidentified(ArtifactKind::Skill, "ghost", ResolveErrorKind::NotDeclared).to_string(),
            "'ghost': not declared"
        );
    }

    /// A part conflict is fixed in the marketplace manifest, never in `grimoire.toml`, and a
    /// bundle that lists the other version still conflicts after the include is added.
    #[test]
    fn a_plugin_conflict_names_the_include_to_add_and_the_manifest_to_edit() {
        let text = ResolveErrorKind::PluginConflict {
            kind: ArtifactKind::Skill,
            name: "x".to_string(),
            sources: "acme/x:1, acme/x:2".to_string(),
            others: String::new(),
            plugin: "team".to_string(),
            manifest: std::path::PathBuf::from("dir/marketplace.toml"),
            from_bundles: false,
        }
        .to_string();

        assert!(text.contains("conflicting versions (acme/x:1, acme/x:2)"), "{text}");
        assert!(
            text.contains("include skill x directly in plugin 'team' of dir/marketplace.toml"),
            "{text}"
        );
        assert!(
            text.contains("a bundle that lists another version still conflicts"),
            "{text}"
        );
        assert!(!text.contains("grimoire.toml"), "{text}");
    }

    /// Where every request comes from a bundle's list, an include of the key cannot choose:
    /// the other bundle still lists its version. The text names the bundles, and a request
    /// that does not come from a bundle keeps the include.
    #[test]
    fn a_plugin_conflict_between_bundles_names_the_bundles_to_drop() {
        let sources = "acme/x:1 from bundle acme/p1:1, acme/x:2 from bundle acme/p2:1";
        let conflict = |from_bundles| {
            ResolveErrorKind::PluginConflict {
                kind: ArtifactKind::Skill,
                name: "x".to_string(),
                sources: sources.to_string(),
                others: String::new(),
                plugin: "team".to_string(),
                manifest: std::path::PathBuf::from("dir/marketplace.toml"),
                from_bundles,
            }
            .to_string()
        };

        assert_eq!(
            conflict(true),
            format!(
                "conflicting versions ({sources}); drop one of these bundles from plugin 'team', \
                 or include versions of them that agree"
            )
        );
        assert!(
            conflict(false).contains("include skill x directly in plugin 'team' of dir/marketplace.toml"),
            "{}",
            conflict(false)
        );
    }

    #[test]
    fn a_held_requirement_names_what_it_requires_what_moves_and_the_retry() {
        let text = ResolveErrorKind::HeldRequirement {
            required: "skill 'held' at acme/held:1".to_string(),
            moved: "skill 'held'".to_string(),
            to: "acme/held:2".to_string(),
            dependent: "y".to_string(),
            retry: "pick 'team:y' too, or update 'team'".to_string(),
        }
        .to_string();

        assert_eq!(
            text,
            "requires skill 'held' at acme/held:1 at its kept pin; this pick moves skill 'held' to \
             acme/held:2; pick 'team:y' too, or update 'team'"
        );
    }

    #[test]
    fn an_empty_hash_prints_as_none() {
        let stale = ResolveErrorKind::StaleLock {
            previous_hash: String::new(),
            current_hash: "sha256:b".to_string(),
            retry: "a full resolve".to_string(),
        };
        assert!(
            stale.to_string().contains("(lock hash none, declared hash sha256:b)"),
            "{stale}"
        );
    }

    #[test]
    fn source_chain_skips_kind_layer() {
        use std::error::Error;
        // Display embeds the kind, so the chain must not re-expose it: a
        // kind without an underlying cause terminates the chain.
        let err = ResolveError::new(artifact_ref(), ResolveErrorKind::ResolveTimeout);
        assert!(err.source().is_none());
    }
    const FORGED: &str = "1\nerror: forged\u{202e}";

    /// `Identifier` keeps a tag verbatim, so nothing but this `Display` stands
    /// between it and a printed line.
    #[test]
    fn a_hostile_reference_stays_on_one_line() {
        let id = Identifier::parse(&format!("ghcr.io/acme/tool:{FORGED}")).unwrap();
        let reference = ArtifactRef::registry(ArtifactKind::Skill, FORGED, id);
        let identified = ResolveError::new(reference, ResolveErrorKind::TagNotFound);
        let unidentified = ResolveError::unidentified(ArtifactKind::Skill, FORGED, ResolveErrorKind::NotDeclared);

        for text in [identified.to_string(), unidentified.to_string()] {
            assert!(!text.contains('\n'), "{text:?}");
            assert!(!text.contains('\u{202e}'), "{text:?}");
        }
    }

    #[test]
    fn a_hostile_kind_field_stays_on_one_line() {
        let conflict = ResolveErrorKind::BundleConflict {
            kind: ArtifactKind::Skill,
            name: "tool".to_string(),
            sources: FORGED.to_string(),
            others: FORGED.to_string(),
            locations: Vec::new(),
            from_bundles: false,
        };
        let stale = ResolveErrorKind::StaleLock {
            previous_hash: FORGED.to_string(),
            current_hash: "sha256:b".to_string(),
            retry: FORGED.to_string(),
        };
        let gone = ResolveErrorKind::MarketplacePinGone {
            digest: Digest::Sha256("a".repeat(64)),
            retry: FORGED.to_string(),
            manifest: std::path::PathBuf::from(FORGED),
        };
        let invalid = ResolveErrorKind::BundleInvalid(FORGED.to_string());

        for kind in [conflict, stale, gone, invalid] {
            let text = kind.to_string();
            assert!(!text.contains('\n'), "{text:?}");
            assert!(!text.contains('\u{202e}'), "{text:?}");
        }
    }

    /// Past [`crate::oci::text::MAX_REGISTRY_TEXT`] on purpose: an id is never cut.
    #[test]
    fn a_clean_reference_prints_byte_for_byte() {
        let repository = "a".repeat(200);
        let id = Identifier::parse(&format!("ghcr.io/{repository}:stable")).unwrap();
        let err = ResolveError::new(
            ArtifactRef::registry(ArtifactKind::Skill, "code-review", id),
            ResolveErrorKind::TagNotFound,
        );

        assert_eq!(
            err.to_string(),
            format!("skill 'code-review' (ghcr.io/{repository}:stable): tag not found")
        );
    }

    fn cycle_of(names: &[&str]) -> Vec<(ArtifactKind, String)> {
        names.iter().map(|n| (ArtifactKind::Skill, (*n).to_string())).collect()
    }

    #[test]
    fn cycle_message_names_the_closing_edge_and_its_publisher() {
        let kind = ResolveErrorKind::DependencyCycle {
            cycle: cycle_of(&["a", "b"]),
            publisher: "ghcr.io/acme/skills/b".to_string(),
        };
        assert_eq!(
            kind.to_string(),
            "dependency cycle: skill/a -> skill/b -> skill/a; the edge skill/b -> skill/a is published by ghcr.io/acme/skills/b; contact that publisher"
        );
    }

    #[test]
    fn self_cycle_closes_on_itself() {
        let kind = ResolveErrorKind::DependencyCycle {
            cycle: cycle_of(&["a"]),
            publisher: "ghcr.io/acme/a".to_string(),
        };
        let text = kind.to_string();
        assert!(
            text.starts_with("dependency cycle: skill/a -> skill/a; the edge skill/a -> skill/a is published by"),
            "{text}"
        );
    }

    #[test]
    fn cap_messages_name_the_limit_its_value_and_the_parent() {
        for (limit, expect) in [
            (GraphLimit::Nodes, "more than 2048 transitive artifacts"),
            (GraphLimit::RequiresBytes, "more than 33554432 bytes of requires layers"),
            (GraphLimit::Depth, "deeper than 32 levels"),
        ] {
            let text = ResolveErrorKind::GraphTooLarge {
                limit,
                via: "skill/a (ghcr.io/acme/a)".to_string(),
            }
            .to_string();
            assert_eq!(
                text,
                format!(
                    "dependency graph too large: {expect}; the edges of skill/a (ghcr.io/acme/a) crossed the limit; contact that publisher"
                )
            );
        }
    }

    #[test]
    fn registry_strings_in_new_messages_are_escaped() {
        let cycle = ResolveErrorKind::DependencyCycle {
            cycle: cycle_of(&["a\u{1b}[2J"]),
            publisher: "evil\u{7}.example/x".to_string(),
        }
        .to_string();
        let cap = ResolveErrorKind::GraphTooLarge {
            limit: GraphLimit::Depth,
            via: "x\u{1b}[2J".to_string(),
        }
        .to_string();
        let gone = ResolveError::new(
            ArtifactRef::registry(ArtifactKind::Skill, "x", Identifier::parse("ghcr.io/acme/x:1").unwrap()),
            ResolveErrorKind::PinnedManifestGone {
                digest: crate::oci::Algorithm::Sha256.hash(b"gone"),
                name: "x\u{1b}[2J".to_string(),
            },
        )
        .to_string();
        for text in [&cycle, &cap, &gone] {
            assert!(!text.chars().any(char::is_control), "{text:?}");
        }
        for text in [cycle, cap] {
            assert!(text.contains("\\u{"), "{text}");
        }
        // The name the terminal shows is not the row's name, so no command names it.
        assert!(gone.contains("`grim update -- <name>`"), "{gone}");
    }

    fn gone(digest: &Digest) -> ResolveError {
        ResolveError::new(
            ArtifactRef::registry(
                ArtifactKind::Skill,
                "code-review",
                Identifier::parse("ghcr.io/acme/code-review:1").unwrap(),
            ),
            ResolveErrorKind::PinnedManifestGone {
                digest: digest.clone(),
                name: "code-review".to_string(),
            },
        )
    }

    #[test]
    fn gone_pin_names_the_digest_and_the_command_that_moves_it() {
        let digest = crate::oci::Algorithm::Sha256.hash(b"gone");
        assert_eq!(
            gone(&digest).to_string(),
            format!(
                "skill 'code-review' (ghcr.io/acme/code-review:1): the registry no longer holds the pinned manifest {digest}; run `grim update -- code-review` to move this pin. If the lock no longer matches what is declared or another pin is gone too, `grim update` with no names moves every pin. If its tag is gone too, or a digest names it, declare a reference that exists in grimoire.toml and run `grim lock`"
            )
        );
    }

    /// A printed remedy no longer leaves the user to work out a flag, which said
    /// nothing to a `--config` run.
    #[test]
    fn gone_pin_names_the_run_it_failed_in() {
        let digest = crate::oci::Algorithm::Sha256.hash(b"gone");
        let config = Invocation::config(std::path::Path::new("/srv/my team/grimoire.toml"));

        for grim in [Invocation::GLOBAL, config] {
            let text = gone(&digest).naming(&grim).to_string();

            assert!(
                text.contains(&format!("run `{grim} update -- code-review` to move")),
                "{text}"
            );
            assert!(
                text.contains(&format!("`{grim} update` with no names moves every pin")),
                "{text}"
            );
            assert!(text.ends_with(&format!("and run `{grim} lock`")), "{text}");
            assert!(
                !text.contains("`grim update") && !text.contains("`grim lock`"),
                "{text}"
            );
            assert!(!text.contains("to each command"), "{text}");
        }
    }

    /// Plain `grim add` declares the target in whatever project the shell stands in.
    #[test]
    fn the_add_hint_names_the_run_it_failed_in() {
        let conflict = ResolveError::new(
            ArtifactRef::registry(
                ArtifactKind::Bundle,
                "x",
                Identifier::parse("ghcr.io/acme/x-impl:1").unwrap(),
            ),
            ResolveErrorKind::BundleConflict {
                kind: ArtifactKind::Skill,
                name: "x".to_string(),
                sources: String::new(),
                others: String::new(),
                locations: Vec::new(),
                from_bundles: false,
            },
        );

        let text = conflict.naming(&Invocation::GLOBAL).to_string();

        assert!(text.ends_with("so `grim --global add` needs `--name x`"), "{text}");
    }

    /// A digest names one manifest: no tag was asked for, so none is missing.
    #[test]
    fn a_digest_request_names_its_digest_not_a_tag() {
        let digest = crate::oci::Algorithm::Sha256.hash(b"gone");
        let id = Identifier::parse(&format!("ghcr.io/acme/a:1@{digest}")).unwrap();
        let err = ResolveError::new(
            ArtifactRef::registry(ArtifactKind::Skill, "a", id),
            ResolveErrorKind::TagNotFound,
        );

        let text = err.to_string();

        assert!(text.ends_with(&format!("): manifest {digest} not found")), "{text}");
        assert!(!text.contains("tag not found"), "{text}");
    }

    /// `grim add` binds the repository's last segment, so without `--name` it
    /// declares `x-impl` and the conflict on `x` stays.
    #[test]
    fn a_conflict_on_a_binding_its_repository_does_not_name_names_the_add_flag() {
        let conflict = |reference: &str| {
            let id = Identifier::parse(reference).unwrap();
            ResolveError::new(
                ArtifactRef::registry(ArtifactKind::Skill, "x", id),
                ResolveErrorKind::BundleConflict {
                    kind: ArtifactKind::Skill,
                    name: "x".to_string(),
                    sources: "a from bundle b:1, c from bundle d:1".to_string(),
                    others: String::new(),
                    locations: Vec::new(),
                    from_bundles: true,
                },
            )
            .to_string()
        };

        let renamed = conflict("ghcr.io/acme/tools/x-impl:1");
        assert!(
            renamed.ends_with("to choose one; its repository is named x-impl, so `grim add` needs `--name x`"),
            "{renamed}"
        );
        assert!(!conflict("ghcr.io/acme/tools/x:1").contains("--name"));
    }

    #[test]
    fn gone_pin_in_a_marketplace_part_names_its_retry() {
        let digest = crate::oci::Algorithm::Sha256.hash(b"gone");
        let text = ResolveErrorKind::MarketplacePinGone {
            digest: digest.clone(),
            retry: "`grim update --marketplace /m.toml team`".to_string(),
            manifest: std::path::PathBuf::from("/m.toml"),
        }
        .to_string();
        assert_eq!(
            text,
            format!(
                "the registry no longer holds the pinned manifest {digest}; run `grim update --marketplace /m.toml team` \
                 to move the pin. If its tag is gone too, or a digest names it, change the include that reaches it in \
                 `/m.toml`"
            )
        );
    }

    #[test]
    fn unsupported_feature_shows_the_feature_error_text() {
        let inner = ArtifactFeatureError::MalformedFeatures {
            reference: "ghcr.io/acme/x:1".to_string(),
            grim_version: "0.0.0",
        };
        let expected = inner.to_string();
        assert_eq!(ResolveErrorKind::UnsupportedFeature(inner).to_string(), expected);
    }
}
