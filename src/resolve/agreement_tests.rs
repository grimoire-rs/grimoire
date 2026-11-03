// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The online expander and the offline closure walk the same graph: fed one
//! published fixture, they must agree on the node set, who requires whom, and
//! how each node is provided.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use super::graph::{Carry, ExpandedGraph, GraphRoot, RootSource, expand_graph};
use super::resolve_options::ResolveOptions;
use super::resolver::resolve_lock;
use crate::config::declaration::{DeclaredSource, DesiredSet};
use crate::config::scope::ConfigScope;
use crate::lock::effective_set::{EffectiveGraph, NodeKey, Origin, effective_graph};
use crate::lock::locked_artifact::BundleProvenance;
use crate::oci::access::memory_registry::MemoryRegistry;
use crate::oci::access::{OciAccess, Operation};
use crate::oci::bundle::{BUNDLE_LAYER_MEDIA_TYPE, BundleManifest, BundleMember};
use crate::oci::manifest::{Descriptor, OciManifest};
use crate::oci::{Algorithm, ArtifactKind, ArtifactKind::*, Identifier};

/// `(kind, name, id)` of one edge.
type Edge = (ArtifactKind, &'static str, &'static str);

const TAR: &str = "application/vnd.grimoire.artifact.layer.v1.tar";

/// Publish one artifact whose `requires` (or bundle members) are `edges`.
async fn publish(reg: &MemoryRegistry, kind: ArtifactKind, id: &str, edges: &[Edge]) {
    let id = Identifier::parse(id).unwrap();
    let members: Vec<BundleMember> = edges
        .iter()
        .map(|(kind, name, id)| BundleMember {
            kind: *kind,
            name: (*name).to_string(),
            id: (*id).to_string(),
        })
        .collect();
    let mut layers = Vec::new();
    if kind != Bundle {
        layers.push(Descriptor {
            digest: Algorithm::Sha256.hash(format!("content {id}").as_bytes()),
            media_type: TAR.to_string(),
            size: 10,
            annotations: None,
        });
    }
    if kind == Bundle || !members.is_empty() {
        let bytes = BundleManifest::new(members).to_layer_bytes().unwrap();
        let digest = reg.push_blob(&id, &bytes).await.unwrap();
        layers.push(Descriptor {
            digest,
            media_type: BUNDLE_LAYER_MEDIA_TYPE.to_string(),
            size: bytes.len() as u64,
            annotations: None,
        });
    }
    let manifest = OciManifest {
        media_type: None,
        artifact_type: None,
        config_media_type: None,
        layers,
        annotations: BTreeMap::new(),
    };
    let digest = reg.push_manifest(&id, &manifest).await.unwrap();
    reg.put_tag(&id, id.tag_or_latest(), &digest).await.unwrap();
}

fn registry(id: &str) -> DeclaredSource {
    DeclaredSource::Registry(Identifier::parse(id).unwrap())
}

/// Direct entries, a declared bundle holding a nested bundle, a diamond whose
/// two arms agree on a digest, and a direct declaration that overrides an edge.
async fn fixture() -> (MemoryRegistry, DesiredSet) {
    let reg = MemoryRegistry::new();
    let all: [(ArtifactKind, &str, &[Edge]); 13] = [
        (
            Skill,
            "ghcr.io/acme/top:1",
            &[
                (Skill, "shared", "ghcr.io/acme/shared:1"),
                (Skill, "over", "ghcr.io/acme/over:1"),
                (Skill, "lib", "./lib:1"),
                // Asked for as `:2` here, as `:1` (a smaller id, a later wave) by `m1`.
                (Skill, "tagged", "ghcr.io/acme/tagged:2"),
                // Declared directly from another publisher below.
                (Skill, "xrepo", "ghcr.io/acme/xrepo:1"),
            ],
        ),
        (
            Skill,
            "ghcr.io/acme/lib:1",
            &[(Rule, "policy", "ghcr.io/acme/policy:1")],
        ),
        (Rule, "ghcr.io/acme/policy:1", &[]),
        (Skill, "ghcr.io/acme/shared:1", &[]),
        (Skill, "ghcr.io/acme/over:1", &[]),
        (Skill, "ghcr.io/acme/over:2", &[]),
        (
            Skill,
            "ghcr.io/acme/m1:1",
            &[
                (Skill, "shared", "ghcr.io/acme/shared:1"),
                (Skill, "tagged", "ghcr.io/acme/tagged:1"),
            ],
        ),
        (Skill, "ghcr.io/acme/deep:1", &[]),
        (Skill, "ghcr.io/acme/tagged:1", &[]),
        (Skill, "ghcr.io/acme/xrepo:1", &[]),
        (Skill, "ghcr.io/other/xrepo:1", &[]),
        (
            Bundle,
            "ghcr.io/acme/inner:1",
            &[
                (Skill, "deep", "ghcr.io/acme/deep:1"),
                (Skill, "shared", "ghcr.io/acme/shared:1"),
            ],
        ),
        (
            Bundle,
            "ghcr.io/acme/pack:1",
            &[
                (Skill, "m1", "ghcr.io/acme/m1:1"),
                (Bundle, "inner", "ghcr.io/acme/inner:1"),
            ],
        ),
    ];
    for (kind, id, edges) in all {
        publish(&reg, kind, id, edges).await;
    }
    // `tagged:2` is a second tag on the digest `tagged:1` serves.
    let tagged = Identifier::parse("ghcr.io/acme/tagged:1").unwrap();
    let digest = reg.resolve_digest(&tagged, Operation::Resolve).await.unwrap().unwrap();
    let alias = Identifier::parse("ghcr.io/acme/tagged:2").unwrap();
    reg.put_tag(&alias, "2", &digest).await.unwrap();
    let skills = BTreeMap::from([
        ("top".to_string(), registry("ghcr.io/acme/top:1")),
        // Overrides the edge `top` -> `over` (which asks for `:1`).
        ("over".to_string(), registry("ghcr.io/acme/over:2")),
        // Overrides the edge `top` -> `xrepo` from a different publisher.
        ("xrepo".to_string(), registry("ghcr.io/other/xrepo:1")),
    ]);
    let bundles = BTreeMap::from([("pack".to_string(), registry("ghcr.io/acme/pack:1"))]);
    let set = DesiredSet::from_parts_with_bundles(skills, BTreeMap::new(), bundles);
    (reg, set)
}

fn id(s: &str) -> Identifier {
    Identifier::parse(s).unwrap()
}

fn roots(set: &DesiredSet) -> Vec<GraphRoot> {
    let mut roots = Vec::new();
    for (table, kind) in [(&set.skills, Skill), (&set.bundles, Bundle)] {
        for (name, source) in table {
            let DeclaredSource::Registry(id) = source else {
                unreachable!("fixture declares registry sources only")
            };
            roots.push(GraphRoot {
                kind,
                name: name.clone(),
                source: RootSource::Registry(id.clone()),
            });
        }
    }
    roots
}

fn opts() -> ResolveOptions {
    ResolveOptions {
        per_artifact_timeout: std::time::Duration::from_secs(30),
        max_retries: 0,
        base_backoff: std::time::Duration::from_millis(1),
    }
}

fn label(key: &NodeKey) -> String {
    format!("{}/{}", key.0, key.1)
}

/// Everything the two sides can be compared on, per node.
#[derive(Debug, PartialEq, Eq)]
struct View {
    direct: bool,
    /// `registry/repo:tag` the node was requested as; absent for a direct node.
    id: Option<String>,
    required_by: BTreeSet<String>,
    contributors: Vec<BundleProvenance>,
}

fn online_view(graph: &ExpandedGraph, declared_bundles: &BTreeSet<String>) -> BTreeMap<String, View> {
    graph
        .nodes
        .iter()
        // A declared bundle is a root, not a member of the effective set.
        .filter(|(key, _)| !(key.0 == Bundle && declared_bundles.contains(&key.1)))
        .map(|(key, node)| {
            let id = match &node.reference.source {
                DeclaredSource::Registry(id) => Some(id.to_string()),
                DeclaredSource::Path(_) => None,
            };
            let view = View {
                direct: node.direct,
                id: (!node.direct).then_some(id).flatten(),
                required_by: node.parents.iter().map(label).collect(),
                contributors: node.provenance.clone(),
            };
            (label(key), view)
        })
        .collect()
}

fn offline_view(graph: &EffectiveGraph) -> BTreeMap<String, View> {
    let mut parents: BTreeMap<&NodeKey, BTreeSet<String>> = BTreeMap::new();
    for (parent, kids) in &graph.children {
        for kid in kids {
            parents.entry(kid).or_default().insert(label(parent));
        }
    }
    graph
        .nodes
        .iter()
        .map(|(key, origin)| {
            let (direct, id, contributors) = match origin {
                Origin::Direct(_) => (true, None, Vec::new()),
                Origin::Implied { id, contributors, .. } => (false, Some(id.to_string()), contributors.clone()),
                Origin::Conflicted => panic!("{} came out conflicted", label(key)),
            };
            let view = View {
                direct,
                id,
                required_by: parents.remove(key).unwrap_or_default(),
                contributors,
            };
            (label(key), view)
        })
        .collect()
}

#[tokio::test]
async fn online_offline_agree() {
    let (reg, set) = fixture().await;
    let access: Arc<dyn OciAccess> = Arc::new(reg);

    let online = expand_graph(roots(&set), &access, &opts(), &Carry::default())
        .await
        .expect("online expansion");
    let lock = resolve_lock(&set, &access, ConfigScope::Project, &opts(), std::path::Path::new("."))
        .await
        .expect("lock");
    let offline = effective_graph(&set, &lock).expect("closure computable from the lock");

    let declared: BTreeSet<String> = set.bundles.keys().cloned().collect();
    let (online, offline) = (online_view(&online, &declared), offline_view(&offline));

    // Positive control: the fixture really reaches transitive nodes.
    for expected in ["skill/lib", "rule/policy", "skill/deep", "bundle/inner", "skill/shared"] {
        assert!(online.contains_key(expected), "{expected} missing from {online:?}");
    }
    assert_eq!(
        online.keys().collect::<Vec<_>>(),
        offline.keys().collect::<Vec<_>>(),
        "node sets differ"
    );
    for (key, on) in &online {
        assert_eq!(on, &offline[key], "{key}: online (left) vs offline (right)");
    }
}

/// A path-declared direct is a direct claim: two edges at different digests
/// do not conflict online, and the offline closure resolves them the same way.
#[tokio::test]
async fn path_direct_wins_online_and_offline() {
    let reg = MemoryRegistry::new();
    for (kind, id, edges) in [
        (Skill, "ghcr.io/acme/shared:1", &[][..]),
        (Bundle, "ghcr.io/acme/pk:1", &[]),
        (
            Bundle,
            "ghcr.io/acme/pk:2",
            &[(Skill, "shared", "ghcr.io/acme/shared:1")],
        ),
        (Skill, "ghcr.io/acme/h1:1", &[(Bundle, "pk", "ghcr.io/acme/pk:1")]),
        (Skill, "ghcr.io/acme/h2:1", &[(Bundle, "pk", "ghcr.io/acme/pk:2")]),
    ] {
        publish(&reg, kind, id, edges).await;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("bundles")).unwrap();
    std::fs::write(
        dir.path().join("bundles/pk.toml"),
        "[skills]\nshared = \"ghcr.io/acme/shared:1\"\n",
    )
    .unwrap();
    let set = DesiredSet::from_parts_with_bundles(
        BTreeMap::from([
            ("h1".to_string(), registry("ghcr.io/acme/h1:1")),
            ("h2".to_string(), registry("ghcr.io/acme/h2:1")),
        ]),
        BTreeMap::new(),
        BTreeMap::from([(
            "pk".to_string(),
            DeclaredSource::Path(crate::config::path_source::PathSource::parse("./bundles/pk.toml").unwrap()),
        )]),
    );
    let access: Arc<dyn OciAccess> = Arc::new(reg);

    let lock = resolve_lock(&set, &access, ConfigScope::Project, &opts(), dir.path())
        .await
        .expect("a path-declared direct is never a conflict");
    let offline = effective_graph(&set, &lock).expect("closure computable from the lock");
    let path = crate::config::path_source::PathSource::parse("./bundles/pk.toml").unwrap();
    let manifest = BundleManifest::new(vec![BundleMember {
        kind: Skill,
        name: "shared".into(),
        id: "ghcr.io/acme/shared:1".into(),
    }]);
    let layer = manifest.to_layer_bytes().unwrap();
    let (repo, tag) = crate::lock::locked_bundle::LockedBundleSource::Path {
        path: path.clone(),
        hash: Algorithm::Sha256.hash(&layer),
    }
    .provenance_pair();
    let mut online_roots: Vec<GraphRoot> = ["h1", "h2"]
        .iter()
        .map(|n| GraphRoot {
            kind: Skill,
            name: (*n).into(),
            source: RootSource::Registry(id(&format!("ghcr.io/acme/{n}:1"))),
        })
        .collect();
    online_roots.push(GraphRoot {
        kind: Bundle,
        name: "pk".into(),
        source: RootSource::Path {
            path,
            edges: manifest.members,
            provenance: Some(BundleProvenance::new(repo, tag)),
        },
    });
    let online = expand_graph(online_roots, &access, &opts(), &Carry::default())
        .await
        .expect("online expansion");

    // A declared bundle is a root, not a member of the effective set.
    assert!(!offline.nodes.contains_key(&(Bundle, "pk".to_string())));
    assert!(offline.nodes.contains_key(&(Skill, "shared".to_string())));
    let declared: BTreeSet<String> = set.bundles.keys().cloned().collect();
    assert_eq!(online_view(&online, &declared), offline_view(&offline));
}
