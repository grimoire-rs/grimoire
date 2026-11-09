// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Every lock writer against one rule, table-driven: lock shapes crossed with the edits a user
//! can make and the tags a publisher can move, through the real `drop_from_lock`,
//! `undeclare_and_unlock`, `target_lock`, `plan_row` and `resolve_marketplace`. A row prints
//! as `shape/edits/move`.
//!
//! No oracle reads a writer's code, so a writer bug cannot hide behind a shared helper: what a
//! row records, what a pin answers and which binding holds a row are restated from the lock's
//! rows, the declaration and the store's edges and tag history. Writers meet only on purpose: a
//! fresh resolve (`exit-agrees`, `update-agrees`) and `grim lock` after an edit (`remove-agrees`)
//! or over a typed line (`held-pin`). Each clause: `adr_lock_pin_stability.md`, § Validation.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::resolve_error::{ResolveError, ResolveErrorKind};
use super::resolve_options::ResolveOptions;
use super::resolver::{LockRun, refuse_stale_names, target_lock};
use crate::catalog::test_hosts::LogCapture;
use crate::cli::exit_code::ExitCode;
use crate::command::uninstall::{EditTarget, undeclare_and_unlock};
use crate::config::declaration::{ConfigOptions, DeclaredSource, DesiredSet};
use crate::config::path_source::PathSource;
use crate::config::scope::{ConfigScope, Invocation};
use crate::error::classify_error;
use crate::export::marketplace::{MarketplaceManifest, PluginDecl};
use crate::export::resolve::{IncludeOrigin, PluginPick, PluginSelection, resolve_marketplace};
use crate::fetch::FetchScope;
use crate::lock::edit::drop_from_lock;
use crate::lock::effective_set::NodeKey;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::lock_io;
use crate::lock::locked_bundle::LockedBundle;
use crate::lock::locked_source::LockedSource;
use crate::lock::prior::Prior;
use crate::lock::refresh::{Refresh, Selected};
use crate::lock::verdict::RowAt;
use crate::lock::{LockedArtifact, MarketplaceLock};
use crate::oci::access::error::AccessError;
use crate::oci::access::memory_registry::MemoryRegistry;
use crate::oci::access::{CatalogListing, OciAccess, Operation};
use crate::oci::artifact_kind::KIND_ANNOTATION;
use crate::oci::bundle::{BUNDLE_LAYER_MEDIA_TYPE, BundleManifest, BundleMember};
use crate::oci::manifest::{Descriptor, OciManifest};
use crate::oci::{Algorithm, ArtifactKind, ArtifactKind::*, Digest, Identifier, PinnedIdentifier};
use crate::tui::app::{RowAction, RowPlan, plan_row};
use crate::tui::state::BatchOp;

const TAR: &str = "application/vnd.grimoire.artifact.layer.v1.tar";

/// A manifest's JSON and what it parses to.
type Served = (Vec<u8>, OciManifest);

/// A [`MemoryRegistry`] that counts tag resolves and remembers every answer a
/// tag ever gave, and the one it gives now. With `json`, a manifest's digest is
/// that of the OCI JSON it serves, which the local store re-hashes on every read.
#[derive(Clone, Default)]
struct Store {
    reg: MemoryRegistry,
    resolved: Arc<Mutex<Vec<String>>>,
    served: Arc<Mutex<BTreeSet<(String, Digest)>>>,
    serving: Arc<Mutex<BTreeMap<String, Digest>>>,
    json: bool,
    manifests: Arc<Mutex<BTreeMap<Digest, Served>>>,
    blobs: Arc<Mutex<BTreeMap<Digest, Vec<u8>>>>,
}

impl Store {
    fn take_resolves(&self) -> Vec<String> {
        std::mem::take(&mut *self.resolved.lock().unwrap())
    }

    fn served(&self, reference: String, digest: Digest) -> bool {
        self.served.lock().unwrap().contains(&(reference, digest))
    }

    /// Every digest `reference` ever served.
    fn ever_served(&self, reference: &str) -> Vec<Digest> {
        let served = self.served.lock().unwrap();
        served
            .iter()
            .filter(|(r, _)| r == reference)
            .map(|(_, d)| d.clone())
            .collect()
    }

    fn serving(&self, reference: &str) -> Option<Digest> {
        self.serving.lock().unwrap().get(reference).cloned()
    }
}

#[async_trait::async_trait]
impl OciAccess for Store {
    async fn resolve_digest(&self, id: &Identifier, op: Operation) -> Result<Option<Digest>, AccessError> {
        if id.digest().is_none() {
            self.resolved.lock().unwrap().push(id.to_string());
        }
        self.reg.resolve_digest(id, op).await
    }
    async fn fetch_manifest(&self, id: &PinnedIdentifier) -> Result<Option<OciManifest>, AccessError> {
        if let Some((_, manifest)) = self.manifests.lock().unwrap().get(&id.digest()) {
            return Ok(Some(manifest.clone()));
        }
        self.reg.fetch_manifest(id).await
    }
    async fn fetch_blob(&self, r: &Identifier, d: &Digest, m: u64) -> Result<Option<Vec<u8>>, AccessError> {
        self.reg.fetch_blob(r, d, m).await
    }
    async fn list_tags(&self, id: &Identifier) -> Result<Option<Vec<String>>, AccessError> {
        self.reg.list_tags(id).await
    }
    async fn list_catalog(&self, registry: &str) -> Result<CatalogListing, AccessError> {
        self.reg.list_catalog(registry).await
    }
    async fn push_blob(&self, repo: &Identifier, bytes: &[u8]) -> Result<Digest, AccessError> {
        let digest = self.reg.push_blob(repo, bytes).await?;
        self.blobs.lock().unwrap().insert(digest.clone(), bytes.to_vec());
        Ok(digest)
    }
    async fn push_manifest(&self, repo: &Identifier, m: &OciManifest) -> Result<Digest, AccessError> {
        if !self.json {
            return self.reg.push_manifest(repo, m).await;
        }
        let layer = |d: &Descriptor| serde_json::json!({"mediaType": d.media_type, "digest": d.digest.to_string(), "size": d.size});
        let document = serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": {
                "mediaType": "application/vnd.oci.empty.v1+json",
                "digest": Algorithm::Sha256.hash(b"{}").to_string(),
                "size": 2,
            },
            "layers": m.layers.iter().map(layer).collect::<Vec<_>>(),
            "annotations": m.annotations,
        });
        let bytes = serde_json::to_vec(&document).unwrap();
        let digest = Algorithm::Sha256.hash(&bytes);
        let manifest = OciManifest::from_json(&bytes).unwrap();
        self.manifests.lock().unwrap().insert(digest.clone(), (bytes, manifest));
        Ok(digest)
    }
    async fn put_tag(&self, repo: &Identifier, tag: &str, d: &Digest) -> Result<(), AccessError> {
        let reference = repo.clone_with_tag(tag).to_string();
        self.served.lock().unwrap().insert((reference.clone(), d.clone()));
        self.serving.lock().unwrap().insert(reference, d.clone());
        self.reg.put_tag(repo, tag, d).await
    }
}

/// `(kind, name, id)` of one edge. `{ref}` in an id stands for the digest `ref`
/// serves, for an edge that names one.
type Edge = (ArtifactKind, &'static str, &'static str);

/// One published artifact. Equal content and edges give equal digests, in any
/// repository: that is how a shape says two tags agree.
struct Art {
    kind: ArtifactKind,
    id: &'static str,
    content: &'static str,
    edges: &'static [Edge],
}

const fn art(kind: ArtifactKind, id: &'static str, content: &'static str, edges: &'static [Edge]) -> Art {
    Art {
        kind,
        id,
        content,
        edges,
    }
}

/// One lock shape.
struct Spec {
    name: &'static str,
    arts: &'static [Art],
    declare: &'static [(ArtifactKind, &'static str, &'static str)],
    /// The key the tag moves aim at.
    key: (ArtifactKind, &'static str),
    /// The requests onto `key`.
    tags: &'static [&'static str],
    /// Set up the pin on this tag rather than the one a fresh resolve wins.
    pin: Option<&'static str>,
    /// A declared bundle with no `[[bundle]]` row, as a pre-cache lock has.
    no_snapshot: Option<&'static str>,
    /// Files in the workspace, for a declaration that names a path.
    files: &'static [(&'static str, &'static str)],
    /// The binding [`Edit::Repoint`] retypes and what it then says, `{ref}` as
    /// in an edge.
    repoint: Option<(&'static str, &'static str)>,
    /// Every artifact pin without its tag, as a lock from before `pinned`
    /// recorded one.
    pre_change: bool,
    /// No row records an edge, as grim 0.15.1 writes a lock.
    no_requires: bool,
    /// Each row keeps one bundle stamp, the single `bundle` pair a lock carried
    /// before every contributor was recorded.
    one_stamp: bool,
    /// Declared bundles with no `[[bundle]]` row, which an edit may take out.
    list_less: &'static [&'static str],
    /// Lines typed into the declaration after the lock was written, with no
    /// relock since.
    typed: &'static [(ArtifactKind, &'static str, &'static str)],
    /// The local store holds what the lock's resolve read, as it left it.
    warm: bool,
    /// The bindings this scope's install state records at their pins.
    installed: &'static [&'static str],
    /// Edges an artifact other than a bundle publishes that the writer of the lock and
    /// the part never walked, as grim 0.15.1 reads no `requires` layer: both pin the
    /// manifest that carries them.
    unwalked: &'static [(&'static str, Edge)],
}

impl Spec {
    /// Whether the lock holds no `[[bundle]]` row for the declared bundle `name`.
    fn lacks_list(&self, name: &str) -> bool {
        self.no_snapshot == Some(name) || self.list_less.contains(&name)
    }
}

const X: (ArtifactKind, &str) = (Skill, "x");
const X1: &str = "ghcr.io/acme/x:1";
const X2: &str = "ghcr.io/acme/x:2";
const X3: &str = "ghcr.io/acme/x:3";
const BASE: Spec = Spec {
    name: "",
    arts: &[],
    declare: &[],
    key: X,
    tags: &[X1, X2],
    pin: None,
    no_snapshot: None,
    files: &[],
    repoint: None,
    pre_change: false,
    no_requires: false,
    one_stamp: false,
    list_less: &[],
    typed: &[],
    warm: false,
    installed: &[],
    unwalked: &[],
};

const P_X1: Art = art(Skill, "ghcr.io/acme/p:1", "p", &[(Skill, "x", X1)]);
const Q_X2: Art = art(Skill, "ghcr.io/acme/q:1", "q", &[(Skill, "x", X2)]);
const X1_SAME: Art = art(Skill, X1, "x", &[]);
const X2_SAME: Art = art(Skill, X2, "x", &[]);
const X1_OWN: Art = art(Skill, X1, "x1", &[]);
const X2_OWN: Art = art(Skill, X2, "x2", &[]);
const DECLARE_P: (ArtifactKind, &str, &str) = (Skill, "p", "ghcr.io/acme/p:1");
const DECLARE_Q: (ArtifactKind, &str, &str) = (Skill, "q", "ghcr.io/acme/q:1");
const DIRECT_X1: (ArtifactKind, &str, &str) = (Skill, "x", X1);
const DIRECT_X2: (ArtifactKind, &str, &str) = (Skill, "x", X2);
const Y_LEFT: Art = art(Skill, "ghcr.io/aa/y:1", "y", &[]);
const Y_RIGHT: Art = art(Skill, "ghcr.io/zz/y:1", "y", &[]);
const L: Art = art(Skill, "ghcr.io/acme/l:1", "l", &[(Skill, "y", "ghcr.io/aa/y:1")]);
const R: Art = art(Skill, "ghcr.io/acme/r:1", "r", &[(Skill, "y", "ghcr.io/zz/y:1")]);
const DECLARE_L: (ArtifactKind, &str, &str) = (Skill, "l", "ghcr.io/acme/l:1");
const DECLARE_R: (ArtifactKind, &str, &str) = (Skill, "r", "ghcr.io/acme/r:1");
const GONE: Art = art(Bundle, "ghcr.io/acme/gone:1", "gone", &[]);
const DECLARE_GONE: (ArtifactKind, &str, &str) = (Bundle, "gone", "ghcr.io/acme/gone:1");
const BYSTANDER: Art = art(Skill, "ghcr.io/acme/b:1", "b", &[]);
const DECLARE_B: (ArtifactKind, &str, &str) = (Skill, "b", "ghcr.io/acme/b:1");
const CARGO: Art = art(Skill, "ghcr.io/acme/c:1", "c", &[]);
const DECLARE_C: (ArtifactKind, &str, &str) = (Skill, "c", "ghcr.io/acme/c:1");
const NEWCOMER: Art = art(Skill, "ghcr.io/acme/z:1", "z", &[(Skill, "x", X1)]);
const B1_X2: Art = art(Bundle, "ghcr.io/acme/b1:1", "b1", &[(Skill, "x", X2)]);
const DECLARE_B1: (ArtifactKind, &str, &str) = (Bundle, "b1", "ghcr.io/acme/b1:1");
const PACK1: &str = "ghcr.io/acme/pack:1";
const PACK2: &str = "ghcr.io/acme/pack:2";
const OTHER1: &str = "ghcr.io/acme/other:1";
const HELD1: &str = "ghcr.io/acme/held:1";
const HELD2: &str = "ghcr.io/acme/held:2";
const M: Art = art(Skill, "ghcr.io/acme/m:1", "m", &[]);
const DEEP: Art = art(Skill, "ghcr.io/acme/d:1", "d", &[(Skill, "e", "ghcr.io/acme/e:1")]);
const DECLARE_DEEP: (ArtifactKind, &str, &str) = (Skill, "d", "ghcr.io/acme/d:1");
const HOOK_H: Art = art(Hook, "ghcr.io/acme/h:1", "h", &[]);
const HOOK_G: Art = art(Hook, "ghcr.io/acme/g:1", "g", &[]);

static SPECS: &[Spec] = &[
    Spec {
        name: "flat",
        arts: &[X1_SAME, art(Skill, "ghcr.io/acme/b:1", "b", &[])],
        declare: &[DIRECT_X1, (Skill, "b", "ghcr.io/acme/b:1")],
        tags: &[X1],
        ..BASE
    },
    Spec {
        name: "diamond",
        arts: &[X1_SAME, X2_SAME, P_X1, Q_X2],
        declare: &[DECLARE_P, DECLARE_Q],
        ..BASE
    },
    Spec {
        name: "diamond-pin-on-the-larger-tag",
        arts: &[X1_SAME, X2_SAME, P_X1, Q_X2],
        declare: &[DECLARE_P, DECLARE_Q],
        pin: Some(X2),
        ..BASE
    },
    Spec {
        name: "three-holders",
        arts: &[
            X1_SAME,
            X2_SAME,
            art(Skill, X3, "x", &[]),
            art(Skill, "ghcr.io/acme/h1:1", "h1", &[(Skill, "x", X1)]),
            art(Skill, "ghcr.io/acme/h2:1", "h2", &[(Skill, "x", X2)]),
            art(Skill, "ghcr.io/acme/h3:1", "h3", &[(Skill, "x", X3)]),
        ],
        declare: &[
            (Skill, "h1", "ghcr.io/acme/h1:1"),
            (Skill, "h2", "ghcr.io/acme/h2:1"),
            (Skill, "h3", "ghcr.io/acme/h3:1"),
        ],
        tags: &[X1, X2, X3],
        ..BASE
    },
    Spec {
        name: "direct-smaller-over-diamond",
        arts: &[X1_SAME, X2_SAME, P_X1, Q_X2],
        declare: &[DIRECT_X1, DECLARE_P, DECLARE_Q],
        ..BASE
    },
    Spec {
        name: "direct-larger-over-diamond",
        arts: &[X1_SAME, X2_SAME, P_X1, Q_X2],
        declare: &[DIRECT_X2, DECLARE_P, DECLARE_Q],
        ..BASE
    },
    Spec {
        name: "direct-smaller-over-conflict",
        arts: &[X1_OWN, X2_OWN, P_X1, Q_X2],
        declare: &[DIRECT_X1, DECLARE_P, DECLARE_Q],
        ..BASE
    },
    Spec {
        name: "direct-larger-over-conflict",
        arts: &[X1_OWN, X2_OWN, P_X1, Q_X2],
        declare: &[DIRECT_X2, DECLARE_P, DECLARE_Q],
        ..BASE
    },
    Spec {
        name: "direct-relative-edge",
        arts: &[
            X1_SAME,
            X2_SAME,
            art(Skill, "ghcr.io/acme/p:1", "p", &[(Skill, "x", "./x:1")]),
            art(Skill, "ghcr.io/acme/q:1", "q", &[(Skill, "x", "./x:2")]),
        ],
        declare: &[DIRECT_X2, DECLARE_P, DECLARE_Q],
        ..BASE
    },
    Spec {
        name: "direct-over-its-own-digest",
        arts: &[
            X1_SAME,
            P_X1,
            art(
                Skill,
                "ghcr.io/acme/q:1",
                "q",
                &[(Skill, "x", "ghcr.io/acme/x@{ghcr.io/acme/x:1}")],
            ),
        ],
        declare: &[DIRECT_X1, DECLARE_P, DECLARE_Q],
        tags: &[X1],
        ..BASE
    },
    Spec {
        name: "direct-over-another-digest",
        arts: &[
            X1_OWN,
            X2_OWN,
            P_X1,
            art(
                Skill,
                "ghcr.io/acme/q:1",
                "q",
                &[(Skill, "x", "ghcr.io/acme/x@{ghcr.io/acme/x:2}")],
            ),
        ],
        declare: &[DIRECT_X1, DECLARE_P, DECLARE_Q],
        ..BASE
    },
    Spec {
        name: "direct-over-two-flat-bundles",
        arts: &[
            X1_SAME,
            X2_SAME,
            art(Bundle, "ghcr.io/acme/b1:1", "b1", &[(Skill, "x", X1)]),
            art(Bundle, "ghcr.io/acme/b2:1", "b2", &[(Skill, "x", X2)]),
        ],
        declare: &[
            DIRECT_X2,
            (Bundle, "b1", "ghcr.io/acme/b1:1"),
            (Bundle, "b2", "ghcr.io/acme/b2:1"),
        ],
        ..BASE
    },
    Spec {
        name: "direct-over-two-disagreeing-flat-bundles",
        arts: &[
            X1_OWN,
            X2_OWN,
            art(Bundle, "ghcr.io/acme/b1:1", "b1", &[(Skill, "x", X1)]),
            art(Bundle, "ghcr.io/acme/b2:1", "b2", &[(Skill, "x", X2)]),
        ],
        declare: &[
            DIRECT_X1,
            (Bundle, "b1", "ghcr.io/acme/b1:1"),
            (Bundle, "b2", "ghcr.io/acme/b2:1"),
        ],
        ..BASE
    },
    Spec {
        name: "nested-bundle",
        arts: &[
            X1_SAME,
            X2_SAME,
            art(Bundle, "ghcr.io/acme/inner:1", "inner", &[(Skill, "x", X1)]),
            art(
                Bundle,
                "ghcr.io/acme/outer:1",
                "outer",
                &[(Bundle, "inner", "ghcr.io/acme/inner:1")],
            ),
            Q_X2,
        ],
        declare: &[DIRECT_X2, (Bundle, "outer", "ghcr.io/acme/outer:1"), DECLARE_Q],
        ..BASE
    },
    Spec {
        name: "declared-bundle-over-two-tags",
        arts: &[
            art(Skill, "ghcr.io/acme/m:1", "m", &[]),
            art(
                Bundle,
                "ghcr.io/acme/pack:1",
                "pack",
                &[(Skill, "m", "ghcr.io/acme/m:1")],
            ),
            art(
                Bundle,
                "ghcr.io/acme/pack:2",
                "pack",
                &[(Skill, "m", "ghcr.io/acme/m:1")],
            ),
            art(
                Skill,
                "ghcr.io/acme/p:1",
                "p",
                &[(Bundle, "pack", "ghcr.io/acme/pack:1")],
            ),
            art(
                Skill,
                "ghcr.io/acme/q:1",
                "q",
                &[(Bundle, "pack", "ghcr.io/acme/pack:2")],
            ),
        ],
        declare: &[(Bundle, "pack", "ghcr.io/acme/pack:2"), DECLARE_P, DECLARE_Q],
        key: (Bundle, "pack"),
        tags: &["ghcr.io/acme/pack:1", "ghcr.io/acme/pack:2"],
        ..BASE
    },
    Spec {
        name: "declared-bundle-over-a-conflict",
        arts: &[
            art(Skill, "ghcr.io/acme/m:1", "m", &[]),
            art(Skill, "ghcr.io/acme/n:1", "n", &[]),
            art(
                Bundle,
                "ghcr.io/acme/pack:1",
                "pack1",
                &[(Skill, "m", "ghcr.io/acme/m:1")],
            ),
            art(
                Bundle,
                "ghcr.io/acme/pack:2",
                "pack2",
                &[(Skill, "n", "ghcr.io/acme/n:1")],
            ),
            art(
                Skill,
                "ghcr.io/acme/p:1",
                "p",
                &[(Bundle, "pack", "ghcr.io/acme/pack:1")],
            ),
            art(
                Skill,
                "ghcr.io/acme/q:1",
                "q",
                &[(Bundle, "pack", "ghcr.io/acme/pack:2")],
            ),
        ],
        declare: &[(Bundle, "pack", "ghcr.io/acme/pack:2"), DECLARE_P, DECLARE_Q],
        key: (Bundle, "pack"),
        tags: &["ghcr.io/acme/pack:1", "ghcr.io/acme/pack:2"],
        ..BASE
    },
    Spec {
        name: "cross-repository-diamond",
        arts: &[Y_LEFT, Y_RIGHT, L, R],
        declare: &[DECLARE_L, DECLARE_R],
        key: (Skill, "y"),
        tags: &["ghcr.io/aa/y:1", "ghcr.io/zz/y:1"],
        ..BASE
    },
    // A declared bundle with no row leaves a dependency-bearing lock's closure
    // incomputable, which is the only way onto the target-only path.
    Spec {
        name: "incomputable-closure-elsewhere",
        arts: &[GONE, X1_SAME, X2_SAME, P_X1, Q_X2],
        declare: &[DIRECT_X2, DECLARE_P, DECLARE_Q, DECLARE_GONE],
        no_snapshot: Some("gone"),
        ..BASE
    },
    Spec {
        name: "incomputable-closure-over-a-conflict",
        arts: &[GONE, X1_OWN, X2_OWN, P_X1, Q_X2],
        declare: &[DIRECT_X1, DECLARE_P, DECLARE_Q, DECLARE_GONE],
        no_snapshot: Some("gone"),
        ..BASE
    },
    Spec {
        name: "dependency-free-legacy",
        arts: &[
            X1_SAME,
            art(Bundle, "ghcr.io/acme/b1:1", "b1", &[(Skill, "x", X1)]),
            art(Bundle, "ghcr.io/acme/gone:1", "gone", &[]),
        ],
        declare: &[
            DIRECT_X1,
            (Bundle, "b1", "ghcr.io/acme/b1:1"),
            (Bundle, "gone", "ghcr.io/acme/gone:1"),
        ],
        tags: &[X1],
        no_snapshot: Some("gone"),
        ..BASE
    },
    Spec {
        name: "dependency-free-legacy-over-two-tags",
        arts: &[
            X1_SAME,
            X2_SAME,
            art(Bundle, "ghcr.io/acme/b1:1", "b1", &[(Skill, "x", X1)]),
            art(Bundle, "ghcr.io/acme/b2:1", "b2", &[(Skill, "x", X2)]),
            art(Bundle, "ghcr.io/acme/gone:1", "gone", &[]),
        ],
        declare: &[
            DIRECT_X1,
            (Bundle, "b1", "ghcr.io/acme/b1:1"),
            (Bundle, "b2", "ghcr.io/acme/b2:1"),
            (Bundle, "gone", "ghcr.io/acme/gone:1"),
        ],
        no_snapshot: Some("gone"),
        ..BASE
    },
    Spec {
        name: "dependency-free-legacy-at-another-version",
        arts: &[
            X1_OWN,
            X2_OWN,
            art(Bundle, "ghcr.io/acme/b1:1", "b1", &[(Skill, "x", X1)]),
            art(Bundle, "ghcr.io/acme/gone:1", "gone", &[]),
        ],
        declare: &[
            DIRECT_X2,
            (Bundle, "b1", "ghcr.io/acme/b1:1"),
            (Bundle, "gone", "ghcr.io/acme/gone:1"),
        ],
        no_snapshot: Some("gone"),
        ..BASE
    },
    // A direct key over a member at another version: its remove leaves the key
    // with no row, and the bystander's remove must not call that a proof.
    Spec {
        name: "flat-at-another-version-with-a-bystander",
        arts: &[X1_OWN, X2_OWN, BYSTANDER, B1_X2],
        declare: &[DIRECT_X1, DECLARE_B1, DECLARE_B],
        ..BASE
    },
    Spec {
        name: "declared-digest-by-hand",
        arts: &[
            M,
            art(Skill, "ghcr.io/acme/n:1", "n", &[]),
            art(Bundle, PACK1, "pack1", &[(Skill, "m", "ghcr.io/acme/m:1")]),
            art(Bundle, PACK2, "pack2", &[(Skill, "n", "ghcr.io/acme/n:1")]),
            BYSTANDER,
        ],
        declare: &[(Bundle, "pack", PACK1), DECLARE_B],
        key: (Bundle, "pack"),
        tags: &[PACK1, PACK2],
        repoint: Some(("pack", "ghcr.io/acme/pack:1@{ghcr.io/acme/pack:2}")),
        ..BASE
    },
    Spec {
        name: "path-bundle-by-hand",
        arts: &[M, art(Skill, "ghcr.io/acme/n:1", "n", &[]), BYSTANDER],
        declare: &[(Bundle, "local", "./bundles/a.toml"), DECLARE_B],
        key: (Skill, "m"),
        tags: &["ghcr.io/acme/m:1"],
        files: &[
            ("bundles/a.toml", "[skills]\nm = \"ghcr.io/acme/m:1\"\n"),
            ("bundles/b.toml", "[skills]\nn = \"ghcr.io/acme/n:1\"\n"),
        ],
        repoint: Some(("local", "./bundles/b.toml")),
        ..BASE
    },
    // Edges at two depths: `x` is read on the nearer one alone, and the farther
    // one decides whether its pin stays.
    Spec {
        name: "direct-over-a-diamond-at-two-depths",
        arts: &[
            X1_SAME,
            X2_SAME,
            P_X1,
            art(Skill, "ghcr.io/acme/e:1", "e", &[(Skill, "x", X2)]),
            DEEP,
        ],
        declare: &[DIRECT_X2, DECLARE_P, DECLARE_DEEP],
        ..BASE
    },
    Spec {
        name: "direct-over-a-digest-at-two-depths",
        arts: &[
            X1_OWN,
            X2_OWN,
            P_X1,
            art(
                Skill,
                "ghcr.io/acme/e:1",
                "e",
                &[(Skill, "x", "ghcr.io/acme/x@{ghcr.io/acme/x:2}")],
            ),
            DEEP,
        ],
        declare: &[DIRECT_X1, DECLARE_P, DECLARE_DEEP],
        ..BASE
    },
    // The bystander's tag moves. A tagless pin needs the proof to hold it, and
    // these two edits take the proof away and leave the lock stale.
    Spec {
        name: "pre-change-over-a-diamond",
        arts: &[X1_SAME, X2_SAME, P_X1, Q_X2, BYSTANDER],
        declare: &[DIRECT_X1, DECLARE_P, DECLARE_Q, DECLARE_B],
        key: (Skill, "b"),
        tags: &["ghcr.io/acme/b:1"],
        pre_change: true,
        ..BASE
    },
    Spec {
        name: "pre-change-at-another-version",
        arts: &[X1_OWN, X2_OWN, Q_X2, BYSTANDER],
        declare: &[DIRECT_X1, DECLARE_Q, DECLARE_B],
        key: (Skill, "b"),
        tags: &["ghcr.io/acme/b:1"],
        pre_change: true,
        ..BASE
    },
    // Three tags of one digest, so a bundle can leave and the key stay contested.
    Spec {
        name: "three-flat-bundles",
        arts: &[
            X1_SAME,
            X2_SAME,
            art(Skill, X3, "x", &[]),
            art(Bundle, "ghcr.io/acme/b1:1", "b1", &[(Skill, "x", X1)]),
            art(Bundle, "ghcr.io/acme/b2:1", "b2", &[(Skill, "x", X2)]),
            art(Bundle, "ghcr.io/acme/b3:1", "b3", &[(Skill, "x", X3)]),
            BYSTANDER,
        ],
        declare: &[
            DECLARE_B1,
            (Bundle, "b2", "ghcr.io/acme/b2:1"),
            (Bundle, "b3", "ghcr.io/acme/b3:1"),
            DECLARE_B,
        ],
        tags: &[X1, X2, X3],
        ..BASE
    },
    Spec {
        name: "pre-cache-bundle-with-a-member",
        arts: &[
            M,
            art(Bundle, PACK1, "pack", &[(Skill, "m", "ghcr.io/acme/m:1")]),
            BYSTANDER,
        ],
        declare: &[(Bundle, "pack", PACK1), DECLARE_B],
        key: (Skill, "m"),
        tags: &["ghcr.io/acme/m:1"],
        no_snapshot: Some("pack"),
        ..BASE
    },
    // A hook declared and a hook only a skill requires: the lock's `[[hook]]`
    // rows go through every writer as the other kinds do.
    Spec {
        name: "hooks-beside-a-bystander",
        arts: &[
            HOOK_H,
            HOOK_G,
            art(Skill, "ghcr.io/acme/p:1", "p", &[(Hook, "g", HOOK_G.id)]),
            BYSTANDER,
        ],
        declare: &[(Hook, "h", HOOK_H.id), DECLARE_P, DECLARE_B],
        key: (Hook, "h"),
        tags: &[HOOK_H.id],
        ..BASE
    },
    // A tagless pin in the repository the winning request is not from: only the
    // pin's own repository can stamp it.
    Spec {
        name: "pre-change-cross-repository-diamond",
        arts: &[Y_LEFT, Y_RIGHT, L, R],
        declare: &[DECLARE_L, DECLARE_R],
        key: (Skill, "y"),
        tags: &["ghcr.io/aa/y:1", "ghcr.io/zz/y:1"],
        pin: Some("ghcr.io/zz/y:1"),
        pre_change: true,
        ..BASE
    },
    // A declared bundle's tag retyped by hand, to one that lists another member.
    Spec {
        name: "declared-bundle-retagged-by-hand",
        arts: &[
            M,
            art(Skill, "ghcr.io/acme/n:1", "n", &[]),
            art(Bundle, PACK1, "pack1", &[(Skill, "m", "ghcr.io/acme/m:1")]),
            art(Bundle, PACK2, "pack2", &[(Skill, "n", "ghcr.io/acme/n:1")]),
            BYSTANDER,
        ],
        declare: &[(Bundle, "pack", PACK1), DECLARE_B],
        key: (Bundle, "pack"),
        tags: &[PACK1, PACK2],
        repoint: Some(("pack", PACK2)),
        ..BASE
    },
    // Bindings no repository's last segment names: `alias` by a declaration, and
    // `dep` by the edge `p` publishes, at `w`.
    Spec {
        name: "alias-bindings",
        arts: &[
            X1_SAME,
            art(Skill, "ghcr.io/acme/w:1", "w", &[]),
            art(Skill, "ghcr.io/acme/p:1", "p", &[(Skill, "dep", "ghcr.io/acme/w:1")]),
            BYSTANDER,
        ],
        declare: &[(Skill, "alias", X1), DECLARE_P, DECLARE_B],
        key: (Skill, "alias"),
        tags: &[X1],
        ..BASE
    },
    // A line overrides what a bundle lists at another repository, so the bundle's
    // member row sits where no pin and no line of `y` does.
    Spec {
        name: "member-overridden-at-another-repository",
        arts: &[
            Y_LEFT,
            Y_RIGHT,
            art(Bundle, "ghcr.io/acme/b1:1", "b1", &[(Skill, "y", "ghcr.io/zz/y:1")]),
            BYSTANDER,
        ],
        declare: &[(Skill, "y", "ghcr.io/aa/y:1"), DECLARE_B1, DECLARE_B],
        key: (Skill, "y"),
        tags: &["ghcr.io/aa/y:1", "ghcr.io/zz/y:1"],
        ..BASE
    },
    // `x` wins on `q`'s digest edge and stays tagless; removing `y` leaves the
    // lock stale, so a later remove asks whether a restamp may state that pin.
    Spec {
        name: "tagless-digest-pin-beside-a-stale-key",
        arts: &[
            X1_SAME,
            P_X1,
            art(
                Skill,
                "ghcr.io/acme/q:1",
                "q",
                &[(Skill, "x", "ghcr.io/acme/x@{ghcr.io/acme/x:1}")],
            ),
            art(Skill, "ghcr.io/acme/y:1", "y", &[]),
            art(Skill, "ghcr.io/acme/y:2", "y", &[]),
            art(Skill, "ghcr.io/acme/c:1", "c", &[(Skill, "y", "ghcr.io/acme/y:1")]),
            art(Skill, "ghcr.io/acme/d:1", "d", &[(Skill, "y", "ghcr.io/acme/y:2")]),
            BYSTANDER,
        ],
        declare: &[
            (Skill, "y", "ghcr.io/acme/y:1"),
            DECLARE_P,
            DECLARE_Q,
            DECLARE_C,
            (Skill, "d", "ghcr.io/acme/d:1"),
            DECLARE_B,
        ],
        tags: &[X1],
        ..BASE
    },
    // A declared bundle `p` also includes, on a lock whose closure the row-less
    // `gone` hides: removing `pack` keeps its row for `p`'s edge alone.
    Spec {
        name: "incomputable-closure-over-an-included-bundle",
        arts: &[
            M,
            art(Bundle, PACK1, "pack", &[(Skill, "m", "ghcr.io/acme/m:1")]),
            art(Skill, "ghcr.io/acme/p:1", "p", &[(Bundle, "pack", PACK1)]),
            GONE,
        ],
        declare: &[(Bundle, "pack", PACK1), DECLARE_P, DECLARE_GONE],
        key: (Bundle, "pack"),
        tags: &[PACK1],
        no_snapshot: Some("gone"),
        ..BASE
    },
    // The row-less bundle lists the direct `x`: offline nothing shows it, and
    // the store says `gone` still asks for `x`.
    Spec {
        name: "incomputable-closure-listing-a-direct-key",
        arts: &[
            X1_SAME,
            art(Bundle, "ghcr.io/acme/gone:1", "gone", &[(Skill, "x", X1)]),
            DEEP,
            art(Skill, "ghcr.io/acme/e:1", "e", &[]),
        ],
        declare: &[DIRECT_X1, DECLARE_DEEP, DECLARE_GONE],
        tags: &[X1],
        no_snapshot: Some("gone"),
        ..BASE
    },
];

/// grim 0.15.1's lock of `pack:1`, which lists `held:1`, and of `p`, whose unwalked
/// edge asks `pack` at the digest `pack:2` serves, where `pack:1` later moves.
const A_0151_DEPENDENT_ASKS_A_BUNDLE_DIGEST: Spec = Spec {
    name: "a-0.15.1-dependent-asks-a-bundle-digest-its-tag-moves-to",
    arts: &[
        art(Skill, HELD1, "held1", &[]),
        art(Skill, HELD2, "held2", &[]),
        art(Bundle, PACK1, "pack1", &[(Skill, "held", HELD1)]),
        art(Bundle, PACK2, "pack2", &[(Skill, "held", HELD2)]),
        art(Skill, "ghcr.io/acme/p:1", "p", &[]),
    ],
    declare: &[(Bundle, "pack", PACK1), DECLARE_P],
    unwalked: &[(
        "ghcr.io/acme/p:1",
        (Bundle, "pack", "ghcr.io/acme/pack@{ghcr.io/acme/pack:2}"),
    )],
    key: (Bundle, "pack"),
    tags: &[PACK1, PACK2],
    pre_change: true,
    no_requires: true,
    ..BASE
};

/// Shapes for the part relock alone, kept out of the fuzz so its seeds keep
/// naming the same sequences.
static PART_SPECS: &[Spec] = &[
    // The outer tag moves onto a bundle nesting another inner one. A part follows
    // the outer tag only where no stamp records the digest it read.
    Spec {
        name: "nested-bundle-moved-onto-other-members",
        arts: &[
            M,
            art(Skill, "ghcr.io/acme/n:1", "n", &[]),
            art(
                Bundle,
                "ghcr.io/acme/inner:1",
                "inner",
                &[(Skill, "m", "ghcr.io/acme/m:1")],
            ),
            art(
                Bundle,
                "ghcr.io/acme/other:1",
                "other",
                &[(Skill, "n", "ghcr.io/acme/n:1")],
            ),
            art(
                Bundle,
                "ghcr.io/acme/outer:1",
                "outer",
                &[(Bundle, "inner", "ghcr.io/acme/inner:1")],
            ),
            art(
                Bundle,
                "ghcr.io/acme/outer:2",
                "outer2",
                &[(Bundle, "other", "ghcr.io/acme/other:1")],
            ),
            BYSTANDER,
        ],
        declare: &[(Bundle, "outer", "ghcr.io/acme/outer:1"), DECLARE_B],
        key: (Bundle, "outer"),
        tags: &["ghcr.io/acme/outer:1", "ghcr.io/acme/outer:2"],
        ..BASE
    },
    // The tag of a bundle a skill requires moves onto a list that names another tag
    // of its member. A part that records no digest for the bundle has nothing to
    // attest the member's pin with, so the pin resolves at the new tag rather than
    // taking it.
    Spec {
        name: "bundle-moved-onto-another-tag-of-its-member",
        arts: &[
            art(Skill, "ghcr.io/acme/held:1", "held1", &[]),
            art(Skill, "ghcr.io/acme/held:2", "held2", &[]),
            art(Bundle, PACK1, "pack1", &[(Skill, "held", "ghcr.io/acme/held:1")]),
            art(Bundle, PACK2, "pack2", &[(Skill, "held", "ghcr.io/acme/held:2")]),
            art(Skill, "ghcr.io/acme/p:1", "p", &[(Bundle, "pack", PACK1)]),
            BYSTANDER,
        ],
        declare: &[DECLARE_P, DECLARE_B],
        key: (Bundle, "pack"),
        tags: &[PACK1, PACK2],
        ..BASE
    },
    // `pack` and `pk2` both list `held:1`, and `pack:1` moves onto a list that names
    // `held:2`. A pick of `pack` still meets pk2's claim on `held`, so it refuses as a
    // write of the part from nothing does, rather than drop pk2's stamp.
    Spec {
        name: "a-bundle-moved-beside-a-sibling-sharing-its-member",
        arts: &[
            art(Skill, HELD1, "held1", &[]),
            art(Skill, HELD2, "held2", &[]),
            art(Bundle, PACK1, "pack1", &[(Skill, "held", HELD1)]),
            art(Bundle, PACK2, "pack2", &[(Skill, "held", HELD2)]),
            art(
                Bundle,
                "ghcr.io/acme/pk2:1",
                "pk2",
                &[(Skill, "held", HELD1), (Skill, "b", "ghcr.io/acme/b:1")],
            ),
            BYSTANDER,
            CARGO,
        ],
        declare: &[
            (Bundle, "pack", PACK1),
            (Bundle, "pk2", "ghcr.io/acme/pk2:1"),
            DECLARE_C,
        ],
        key: (Bundle, "pack"),
        tags: &[PACK1, PACK2],
        ..BASE
    },
    A_0151_DEPENDENT_ASKS_A_BUNDLE_DIGEST,
];

/// Longer shapes, for the fuzz alone: crossed with every edit they would be most
/// of the table's run time and none of its reach.
static FUZZ_SPECS: &[Spec] = &[
    Spec {
        name: "nested-with-a-bystander",
        arts: &[
            M,
            art(
                Bundle,
                "ghcr.io/acme/inner:1",
                "inner1",
                &[(Skill, "m", "ghcr.io/acme/m:1")],
            ),
            art(
                Bundle,
                "ghcr.io/acme/inner:2",
                "inner2",
                &[(Skill, "m", "ghcr.io/acme/m:1")],
            ),
            art(
                Bundle,
                "ghcr.io/acme/outer:1",
                "outer",
                &[(Bundle, "inner", "ghcr.io/acme/inner:1")],
            ),
            BYSTANDER,
        ],
        declare: &[(Bundle, "outer", "ghcr.io/acme/outer:1"), DECLARE_B],
        key: (Bundle, "inner"),
        tags: &["ghcr.io/acme/inner:1", "ghcr.io/acme/inner:2"],
        ..BASE
    },
    Spec {
        name: "flat-at-another-version-with-cargo",
        arts: &[X1_OWN, X2_OWN, BYSTANDER, CARGO, B1_X2, NEWCOMER],
        declare: &[DIRECT_X1, DECLARE_B1, DECLARE_B, DECLARE_C],
        ..BASE
    },
    Spec {
        name: "dependency-at-another-version",
        arts: &[X1_OWN, X2_OWN, BYSTANDER, CARGO, Q_X2, NEWCOMER],
        declare: &[DIRECT_X1, DECLARE_Q, DECLARE_B, DECLARE_C],
        ..BASE
    },
    Spec {
        name: "bystanders-over-a-diamond",
        arts: &[X1_SAME, X2_SAME, BYSTANDER, CARGO, P_X1, Q_X2, NEWCOMER],
        declare: &[DIRECT_X1, DECLARE_P, DECLARE_Q, DECLARE_B, DECLARE_C],
        ..BASE
    },
    Spec {
        name: "bystanders-over-a-conflict",
        arts: &[X1_OWN, X2_OWN, BYSTANDER, CARGO, P_X1, Q_X2, NEWCOMER],
        declare: &[DIRECT_X1, DECLARE_P, DECLARE_Q, DECLARE_B, DECLARE_C],
        ..BASE
    },
    Spec {
        name: "edge-only-diamond-with-bystanders",
        arts: &[X1_SAME, X2_SAME, BYSTANDER, CARGO, P_X1, Q_X2, NEWCOMER],
        declare: &[DECLARE_P, DECLARE_Q, DECLARE_B, DECLARE_C],
        ..BASE
    },
    Spec {
        name: "bundle-edge-diamond",
        arts: &[
            M,
            art(Bundle, PACK1, "pack", &[(Skill, "m", "ghcr.io/acme/m:1")]),
            art(Bundle, PACK2, "pack", &[(Skill, "m", "ghcr.io/acme/m:1")]),
            art(Skill, "ghcr.io/acme/p:1", "p", &[(Bundle, "pack", PACK1)]),
            art(Skill, "ghcr.io/acme/q:1", "q", &[(Bundle, "pack", PACK2)]),
            BYSTANDER,
        ],
        declare: &[DECLARE_P, DECLARE_Q, DECLARE_B],
        key: (Bundle, "pack"),
        tags: &[PACK1, PACK2],
        ..BASE
    },
    Spec {
        name: "declared-bundle-diamond-with-a-bystander",
        arts: &[
            M,
            art(Bundle, PACK1, "pack", &[(Skill, "m", "ghcr.io/acme/m:1")]),
            art(Bundle, PACK2, "pack", &[(Skill, "m", "ghcr.io/acme/m:1")]),
            art(Skill, "ghcr.io/acme/p:1", "p", &[(Bundle, "pack", PACK1)]),
            art(Skill, "ghcr.io/acme/q:1", "q", &[(Bundle, "pack", PACK2)]),
            BYSTANDER,
        ],
        declare: &[(Bundle, "pack", PACK1), DECLARE_P, DECLARE_Q, DECLARE_B],
        key: (Bundle, "pack"),
        tags: &[PACK1, PACK2],
        ..BASE
    },
    Spec {
        name: "cross-repository-direct",
        arts: &[Y_LEFT, Y_RIGHT, L, R, BYSTANDER],
        declare: &[(Skill, "y", "ghcr.io/aa/y:1"), DECLARE_L, DECLARE_R, DECLARE_B],
        key: (Skill, "y"),
        tags: &["ghcr.io/aa/y:1", "ghcr.io/zz/y:1"],
        ..BASE
    },
];

const SETTINGS_S1: &str = "ghcr.io/acme/s:1";
const SETTINGS_S2: &str = "ghcr.io/acme/s:2";
const SETTINGS_T: Art = art(ArtifactKind::Settings, "ghcr.io/acme/t:1", "t", &[]);
const DECLARE_T: (ArtifactKind, &str, &str) = (ArtifactKind::Settings, "t", "ghcr.io/acme/t:1");

/// Shapes the fuzz's shape draw leaves out, so every seed it recorded still names
/// the same sequence. Every table test takes them, and their own fuzz draws from
/// them alone.
static LATER_SPECS: &[Spec] = &[
    // A settings package a declared bundle lists and a skill requires at another tag
    // of its digest, beside a declared one: settings rows through every writer.
    Spec {
        name: "settings-in-a-bundle-and-an-edge",
        arts: &[
            art(ArtifactKind::Settings, SETTINGS_S1, "s", &[]),
            art(ArtifactKind::Settings, SETTINGS_S2, "s", &[]),
            M,
            art(
                Bundle,
                PACK1,
                "pack",
                &[
                    (ArtifactKind::Settings, "s", SETTINGS_S1),
                    (Skill, "m", "ghcr.io/acme/m:1"),
                ],
            ),
            art(
                Skill,
                "ghcr.io/acme/p:1",
                "p",
                &[(ArtifactKind::Settings, "s", SETTINGS_S2)],
            ),
            SETTINGS_T,
            BYSTANDER,
        ],
        declare: &[(Bundle, "pack", PACK1), DECLARE_P, DECLARE_T, DECLARE_B],
        key: (ArtifactKind::Settings, "s"),
        tags: &[SETTINGS_S1, SETTINGS_S2],
        ..BASE
    },
    // A settings line over the bundle member it overrides at another digest.
    Spec {
        name: "settings-line-over-its-bundle-member",
        arts: &[
            art(ArtifactKind::Settings, SETTINGS_S1, "s1", &[]),
            art(ArtifactKind::Settings, SETTINGS_S2, "s2", &[]),
            art(Bundle, PACK1, "pack", &[(ArtifactKind::Settings, "s", SETTINGS_S1)]),
            SETTINGS_T,
            BYSTANDER,
        ],
        declare: &[
            (ArtifactKind::Settings, "s", SETTINGS_S2),
            (Bundle, "pack", PACK1),
            DECLARE_T,
            DECLARE_B,
        ],
        key: (ArtifactKind::Settings, "s"),
        tags: &[SETTINGS_S1, SETTINGS_S2],
        ..BASE
    },
    // A lock grim 0.15.1 wrote: tagless pins, a settings member and a skill member
    // of one bundle, whose tags move. `grim add` at the member's tag keeps its pin.
    Spec {
        name: "pre-change-bundle-members",
        arts: &[
            art(ArtifactKind::Settings, SETTINGS_S1, "s", &[]),
            X1_SAME,
            art(
                Bundle,
                PACK1,
                "pack",
                &[(ArtifactKind::Settings, "s", SETTINGS_S1), (Skill, "x", X1)],
            ),
            BYSTANDER,
        ],
        declare: &[(Bundle, "pack", PACK1), DECLARE_B],
        tags: &[X1],
        pre_change: true,
        ..BASE
    },
    Spec {
        name: "pre-change-settings-member",
        arts: &[
            art(ArtifactKind::Settings, SETTINGS_S1, "s", &[]),
            X1_SAME,
            art(
                Bundle,
                PACK1,
                "pack",
                &[(ArtifactKind::Settings, "s", SETTINGS_S1), (Skill, "x", X1)],
            ),
            BYSTANDER,
        ],
        declare: &[(Bundle, "pack", PACK1), DECLARE_B],
        key: (ArtifactKind::Settings, "s"),
        tags: &[SETTINGS_S1],
        pre_change: true,
        ..BASE
    },
];

/// Shapes for the table alone, kept out of every fuzz draw so each recorded seed
/// still names the same sequence.
static TABLE_SPECS: &[Spec] = &[
    // A lock grim 0.15.1 wrote before `[[bundle]]` rows: the member's tagless pin
    // answers only the edge its unpinned bundle lists.
    Spec {
        name: "pre-change-pre-cache-bundle-with-a-member",
        arts: &[
            M,
            art(Bundle, PACK1, "pack", &[(Skill, "m", "ghcr.io/acme/m:1")]),
            BYSTANDER,
        ],
        declare: &[(Bundle, "pack", PACK1), DECLARE_B],
        key: (Skill, "m"),
        tags: &["ghcr.io/acme/m:1"],
        no_snapshot: Some("pack"),
        pre_change: true,
        ..BASE
    },
    // `x`'s line, in a third repository, holds off two requests at other digests.
    // With the line gone, a catalog row at the pin's repository has no request
    // there, and one at the requests' repository has them.
    Spec {
        name: "a-conflict-away-from-the-pin",
        arts: &[art(Skill, "ghcr.io/aa/x:1", "xa", &[]), X1_OWN, X2_OWN, P_X1, Q_X2],
        declare: &[(Skill, "x", "ghcr.io/aa/x:1"), DECLARE_P, DECLARE_Q],
        ..BASE
    },
    // `x` is declared and listed by `pack`, whose `[[bundle]]` row the lock lacks, and
    // no row records an edge. A cold store cannot show `pack` provides `x`, so its
    // remove drops the row as 0.15.1 does and says so.
    Spec {
        name: "direct-key-a-list-less-bundle-lists",
        arts: &[X1_SAME, art(Bundle, PACK1, "pack", &[(Skill, "x", X1)]), BYSTANDER],
        declare: &[DIRECT_X1, (Bundle, "pack", PACK1), DECLARE_B],
        tags: &[X1],
        no_snapshot: Some("pack"),
        ..BASE
    },
    // `pack`'s line retyped to a tag that lists another member, beside a second
    // declared bundle: the edit that takes `other` out keeps `pack`'s row at pack:1's
    // tag and digest, and the next relock moves `pack` to what pack:2 serves.
    Spec {
        name: "a-bundle-retagged-by-hand-beside-another",
        arts: &[
            M,
            art(Skill, "ghcr.io/acme/n:1", "n", &[]),
            art(Skill, "ghcr.io/acme/o:1", "o", &[]),
            art(Bundle, PACK1, "pack1", &[(Skill, "m", "ghcr.io/acme/m:1")]),
            art(Bundle, PACK2, "pack2", &[(Skill, "n", "ghcr.io/acme/n:1")]),
            art(Bundle, OTHER1, "other", &[(Skill, "o", "ghcr.io/acme/o:1")]),
        ],
        declare: &[(Bundle, "pack", PACK1), (Bundle, "other", OTHER1)],
        key: (Bundle, "pack"),
        tags: &[PACK1, PACK2],
        repoint: Some(("pack", PACK2)),
        ..BASE
    },
];

const M1: &str = "ghcr.io/acme/m:1";
const BA1: &str = "ghcr.io/acme/ba:1";
const BZ1: &str = "ghcr.io/acme/bz:1";
const BA_M: Art = art(Bundle, BA1, "ba", &[(Skill, "m", M1)]);
const BZ_M: Art = art(Bundle, BZ1, "bz", &[(Skill, "m", M1)]);
const A_0151_DEPENDENT: Spec = Spec {
    name: "a-0.15.1-dependent-the-store-shows",
    arts: &[X1_SAME, P_X1, BYSTANDER],
    declare: &[DECLARE_P, DIRECT_X1, DECLARE_B],
    tags: &[X1],
    pre_change: true,
    no_requires: true,
    warm: true,
    ..BASE
};
const SHARED_BY_LIST_LESS: Spec = Spec {
    name: "a-member-two-list-less-bundles-share",
    arts: &[M, BA_M, BZ_M, BYSTANDER],
    declare: &[(Bundle, "ba", BA1), (Bundle, "bz", BZ1), DECLARE_B],
    key: (Skill, "m"),
    tags: &[M1],
    list_less: &["ba", "bz"],
    one_stamp: true,
    warm: true,
    ..BASE
};

/// Shapes whose edits read the local store or a typed line, or run beside an
/// install record, for the edit table alone: no other writer reads what they add.
/// Every other shape's edits read an empty store and no install record, as `grim
/// remove` does on a fresh home. These fill the store once, as the build's resolve left it, so
/// no later tag move or relock reaches it.
static STORE_SPECS: &[Spec] = &[
    // `p`, which requires `x`, typed after the lock was written: the lock holds no
    // row for it, so only the next relock reads its edges.
    Spec {
        name: "a-typed-dependent-beside-a-direct-key",
        arts: &[X1_SAME, P_X1, BYSTANDER],
        declare: &[DIRECT_X1, DECLARE_B],
        typed: &[DECLARE_P],
        tags: &[X1],
        ..BASE
    },
    // grim 0.15.1's lock of `p` beside `x`'s own line: `p`'s row records no edge,
    // and the store holds `p`, showing it requires `x`: an edit that drops `x`
    // names `p`.
    A_0151_DEPENDENT,
    // An install record at `p`'s pin changes no verdict.
    Spec {
        name: "a-0.15.1-dependent-installed-at-its-pin",
        installed: &["p"],
        ..A_0151_DEPENDENT
    },
    // A cold store shows nothing of `p`.
    Spec {
        name: "a-0.15.1-dependent-the-store-lacks",
        warm: false,
        ..A_0151_DEPENDENT
    },
    // grim 0.15.1's lock of `pack`, which lists `m`, beside `p`, which requires `m`
    // at that tag, added with no install: removing `pack` drops `m`, as on 0.15.1,
    // and names `p`.
    Spec {
        name: "a-0.15.1-dependent-beside-the-bundle-that-lists-its-requirement",
        arts: &[
            M,
            art(Bundle, PACK1, "pack", &[(Skill, "m", M1)]),
            art(Skill, "ghcr.io/acme/p:1", "p", &[(Skill, "m", M1)]),
        ],
        declare: &[(Bundle, "pack", PACK1), DECLARE_P],
        key: (Skill, "m"),
        tags: &[M1],
        installed: &["m"],
        ..A_0151_DEPENDENT
    },
    // grim 0.15.1's lock of `q`, which requires the bundle `pack` declares, with `q`
    // stored but not installed: removing `pack` drops its row and names `q`, and
    // the next relock resolves `pack` afresh for `q`.
    Spec {
        name: "a-0.15.1-dependent-requires-a-declared-bundle",
        arts: &[
            art(Skill, "ghcr.io/acme/n:1", "n", &[]),
            art(Bundle, PACK2, "pack", &[(Skill, "n", "ghcr.io/acme/n:1")]),
            art(Skill, "ghcr.io/acme/q:1", "q", &[(Bundle, "pack", PACK2)]),
        ],
        declare: &[DECLARE_Q, (Bundle, "pack", PACK2)],
        key: (Bundle, "pack"),
        tags: &[PACK2],
        ..A_0151_DEPENDENT
    },
    // The same beside a bundle line typed with no relock, so the lock holds no list
    // for it: removing `pack` takes the pre-cache path, which drops the row too, since
    // `q`'s row records no edge.
    Spec {
        name: "a-0.15.1-dependent-requires-a-declared-bundle-beside-a-typed-bundle",
        arts: &[
            art(Skill, "ghcr.io/acme/n:1", "n", &[]),
            art(Bundle, PACK2, "pack", &[(Skill, "n", "ghcr.io/acme/n:1")]),
            art(Skill, "ghcr.io/acme/q:1", "q", &[(Bundle, "pack", PACK2)]),
            art(Skill, "ghcr.io/acme/o:1", "o", &[]),
            art(Bundle, OTHER1, "other", &[(Skill, "o", "ghcr.io/acme/o:1")]),
        ],
        declare: &[DECLARE_Q, (Bundle, "pack", PACK2)],
        typed: &[(Bundle, "other", OTHER1)],
        key: (Bundle, "pack"),
        tags: &[PACK2],
        ..A_0151_DEPENDENT
    },
    // `q`, which requires the bundle `pack` declares, typed with that bundle line: the
    // lock holds no row for `q`, so only the next relock reads its edges, and no list
    // for `other`, so removing `pack` takes the pre-cache path, which keeps its row.
    Spec {
        name: "a-typed-dependent-requires-a-declared-bundle-beside-a-typed-bundle",
        arts: &[
            art(Skill, "ghcr.io/acme/n:1", "n", &[]),
            art(Bundle, PACK2, "pack", &[(Skill, "n", "ghcr.io/acme/n:1")]),
            art(Skill, "ghcr.io/acme/q:1", "q", &[(Bundle, "pack", PACK2)]),
            art(Skill, "ghcr.io/acme/o:1", "o", &[]),
            art(Bundle, OTHER1, "other", &[(Skill, "o", "ghcr.io/acme/o:1")]),
        ],
        declare: &[(Bundle, "pack", PACK2)],
        typed: &[DECLARE_Q, (Bundle, "other", OTHER1)],
        key: (Bundle, "pack"),
        tags: &[PACK2],
        ..BASE
    },
    // `ba` and `bz` both list `m`, the lock holds neither list, and `m` carries one
    // stamp. The store shows both lists.
    SHARED_BY_LIST_LESS,
    // A cold store shows neither list.
    Spec {
        name: "a-member-two-list-less-bundles-share-cold",
        warm: false,
        ..SHARED_BY_LIST_LESS
    },
    // The edit table's route through a dependent that asks a bundle by a digest its tag
    // has since left: no row records `p`'s edge, and the digest names no tag.
    A_0151_DEPENDENT_ASKS_A_BUNDLE_DIGEST,
];

/// Locks and parts grim 0.15.1 wrote over a dependent whose edges it never walked, kept
/// out of every fuzz draw. A fresh resolve of each refuses, so the cold relock judges
/// only the rows whose online relock accepts.
static UNWALKED_SPECS: &[Spec] = &[
    // `x` requires `held:1`, and `pack` lists `held:2`, which serves other bytes.
    Spec {
        name: "a-0.15.1-dependent-asks-a-lower-tag-of-a-member",
        arts: &[
            art(Skill, HELD1, "held1", &[]),
            art(Skill, HELD2, "held2", &[]),
            art(Bundle, PACK1, "pack", &[(Skill, "held", HELD2)]),
            X1_SAME,
            BYSTANDER,
        ],
        declare: &[(Bundle, "pack", PACK1), DIRECT_X1, DECLARE_B],
        unwalked: &[(X1, (Skill, "held", HELD1))],
        key: (Skill, "held"),
        tags: &[HELD1, HELD2],
        pre_change: true,
        no_requires: true,
        ..BASE
    },
    // The same with the tags the other way round.
    Spec {
        name: "a-0.15.1-dependent-asks-a-later-tag-of-a-member",
        arts: &[
            art(Skill, HELD1, "held1", &[]),
            art(Skill, HELD2, "held2", &[]),
            art(Bundle, PACK1, "pack", &[(Skill, "held", HELD1)]),
            X1_SAME,
        ],
        declare: &[(Bundle, "pack", PACK1), DIRECT_X1],
        unwalked: &[(X1, (Skill, "held", HELD2))],
        key: (Skill, "held"),
        tags: &[HELD1, HELD2],
        pre_change: true,
        no_requires: true,
        ..BASE
    },
];

/// Shapes whose first move republishes a tag with an edge its old manifest lacked:
/// the tag, and the edge it gains. Kept out of every fuzz draw, as the table's are.
static EDGE_SPECS: &[(Spec, &str, Edge)] = &[
    // A lock grim 0.15.1 wrote before `[[bundle]]` rows, two tagless members: `x:1`
    // comes to require `y:2`, a tag of the other member.
    (
        Spec {
            name: "pre-change-pre-cache-bundle-whose-member-gains-an-edge",
            arts: &[
                X1_SAME,
                art(Skill, "ghcr.io/acme/y:1", "y", &[]),
                art(Skill, "ghcr.io/acme/y:2", "y2", &[]),
                art(
                    Bundle,
                    PACK1,
                    "pack",
                    &[(Skill, "x", X1), (Skill, "y", "ghcr.io/acme/y:1")],
                ),
                BYSTANDER,
            ],
            declare: &[(Bundle, "pack", PACK1), DECLARE_B],
            key: (Skill, "y"),
            tags: &["ghcr.io/acme/y:1", "ghcr.io/acme/y:2"],
            no_snapshot: Some("pack"),
            pre_change: true,
            ..BASE
        },
        X1,
        (Skill, "y", "ghcr.io/acme/y:2"),
    ),
];

/// One edit of a declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edit {
    Remove,
    /// `grim uninstall` and the TUI delete, which share `undeclare_and_unlock`;
    /// only the TUI takes a bundle.
    Undeclare,
    /// The line leaves `grimoire.toml`; nothing touches the lock.
    Hand,
    /// The line stays and names other bytes; nothing touches the lock.
    Repoint,
}

impl Edit {
    fn writes_the_lock(self) -> bool {
        !matches!(self, Self::Hand | Self::Repoint)
    }
}

/// What moves between the edit and the relock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Move {
    None,
    /// The tag the pin names, to a new digest.
    PinTag,
    /// Every other tag, to a new digest.
    OtherTag,
    /// Every tag, to one new digest.
    AllToOne,
    /// Every tag, each to its own new digest.
    AllApart,
    /// The pin's tag, to the digest the other tag serves.
    PinTagJoinsOther,
}

const MOVES: [Move; 6] = [
    Move::None,
    Move::PinTag,
    Move::OtherTag,
    Move::AllToOne,
    Move::AllApart,
    Move::PinTagJoinsOther,
];

fn options() -> ResolveOptions {
    ResolveOptions {
        per_artifact_timeout: std::time::Duration::from_secs(30),
        max_retries: 0,
        base_backoff: std::time::Duration::from_millis(1),
    }
}

fn parse(id: &str) -> Identifier {
    Identifier::parse(id).unwrap()
}

fn request(pin: &PinnedIdentifier) -> String {
    let id = Identifier::new_registry(pin.repository(), pin.registry());
    match pin.tag() {
        Some(tag) => id.clone_with_tag(tag).to_string(),
        None => id.to_string(),
    }
}

/// Whether the pin's own text names `id`, or `id` is its digest in its repository.
fn names(pin: &PinnedIdentifier, id: &Identifier) -> bool {
    let same_repository = pin.registry_repository() == id.registry_repository();
    match id.digest() {
        Some(digest) => same_repository && digest == pin.digest(),
        None => same_repository && pin.tag() == id.tag(),
    }
}

/// Whether a direct line at `id` leaves `pin` a reading: its repository, any digest
/// it names, and any tag both carry.
fn may_stand(pin: &PinnedIdentifier, id: &Identifier) -> bool {
    pin.registry_repository() == id.registry_repository()
        && id.digest().is_none_or(|digest| digest == pin.digest())
        && (pin.tag().is_none() || id.tag().is_none() || pin.tag() == id.tag())
}

/// `(kind, name)` to pin, every registry row of the lock.
fn pins(lock: &GrimoireLock) -> BTreeMap<NodeKey, PinnedIdentifier> {
    let artifacts = lock
        .iter_artifacts()
        .filter_map(|a| Some(((a.kind, a.name.clone()), a.source.pinned()?.clone())));
    let bundles = lock
        .bundles
        .iter()
        .filter_map(|b| Some(((Bundle, b.name.clone()), b.pinned()?.clone())));
    artifacts.chain(bundles).collect()
}

/// What `edge` asks for, as its publisher wrote it: `./name:tag` sits beside its
/// parent, and a path parent has no repository to sit in.
fn requested(edge: &BundleMember, parent: Option<&Identifier>) -> Option<Identifier> {
    let Some(sibling) = edge.id.strip_prefix("./") else {
        return Some(parse(&edge.id));
    };
    let home = parent?.registry_repository();
    let (namespace, _) = home.rsplit_once('/')?;
    Some(parse(&format!("{namespace}/{sibling}")))
}

/// The requests the lock's own rows record on each key, each against its row's pin.
fn requests(lock: &GrimoireLock) -> Requests {
    requests_from(lock, |_| true)
}

/// [`requests`], from the rows whose key `keep` holds.
fn requests_from(lock: &GrimoireLock, keep: impl Fn(&NodeKey) -> bool) -> Requests {
    let artifacts = lock
        .iter_artifacts()
        .map(|row| ((row.kind, row.name.clone()), row.source.pinned(), &row.requires));
    let bundles = lock
        .bundles
        .iter()
        .map(|row| ((Bundle, row.name.clone()), row.pinned(), &row.members));
    let mut out = Requests::new();
    for (_, pin, edges) in artifacts.chain(bundles).filter(|(key, ..)| keep(key)) {
        for edge in edges {
            if let Some(id) = requested(edge, pin.map(PinnedIdentifier::as_identifier)) {
                out.entry((edge.kind, edge.name.clone())).or_default().insert(id);
            }
        }
    }
    out
}

/// How a request onto a key reached the lock that a pin answers for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Made {
    /// A declaration, or an edge the lock does not record.
    Unrecorded,
    /// An edge a row of the lock records.
    Recorded,
    /// An edge a parent lists that the relock reads as the lock's proof shows it.
    Listed,
}

/// Whether `pin`, the row of `key` in a lock whose hash proves `proof`, answers
/// the request `id`, made as `made`. Evidence, as the design record lists it: the
/// pin's own record names `id`; `id` is a tagless pin's declaration in the proof;
/// `id` is a recorded edge onto a key the proof holds through edges alone, which
/// the graph settled by agreement; or, for a tagless pin on such a key, `id` sits in
/// its own repository and a parent read as the proof shows it lists it. A digest
/// named outright is that digest: no pin answers it for another.
fn answers(pin: &PinnedIdentifier, key: &NodeKey, proof: Option<&DesiredSet>, made: Made, id: &Identifier) -> bool {
    let holds = id.digest().is_none_or(|digest| digest == pin.digest());
    let direct = proof.and_then(|proof| proof.table(key.0).get(&key.1));
    let here = pin.registry_repository() == id.registry_repository();
    let stamped = pin.tag().is_none() && here && direct.and_then(DeclaredSource::identifier) == Some(id);
    let indirect = proof.is_some() && direct.is_none();
    let listed = made == Made::Listed && pin.tag().is_none() && here && key.0 != Bundle;
    names(pin, id) || (holds && stamped) || (holds && indirect && (made == Made::Recorded || listed))
}

/// The declaration among `sets` that `lock`'s hash proves.
fn proof_of<'s>(lock: &GrimoireLock, sets: &[&'s DesiredSet]) -> Option<&'s DesiredSet> {
    sets.iter()
        .copied()
        .find(|set| lock.metadata.declaration_hash == set.declaration_hash_cached())
}

/// `relocked` with each stamp undone. A relock stamps a tagless pin its proof
/// answers, an edit that keeps the proof leaves that to it, and neither moves
/// the digest.
fn without_stamps(
    after: &BTreeMap<NodeKey, PinnedIdentifier>,
    before: &BTreeMap<NodeKey, PinnedIdentifier>,
    relocked: String,
) -> String {
    before
        .iter()
        .filter(|(_, pin)| pin.tag().is_none())
        .filter_map(|(key, pin)| after.get(key).map(|now| (now, pin)))
        .filter(|(now, pin)| now.digest() == pin.digest())
        .fold(relocked, |rows, (now, pin)| {
            rows.replace(&now.to_string(), &pin.to_string())
        })
}

/// A lock with its volatile metadata gone: what two writers must agree on.
fn rows(lock: &GrimoireLock) -> String {
    let mut lock = lock.clone();
    lock.metadata.generated_at = String::new();
    lock.metadata.generated_by = String::new();
    lock.to_toml_string().unwrap()
}

/// [`rows`] without the hash: what an edit that changed no row leaves alone.
fn rows_alone(lock: &GrimoireLock) -> String {
    let mut lock = lock.clone();
    lock.metadata.declaration_hash = String::new();
    rows(&lock)
}

/// The requests on each key.
type Requests = BTreeMap<NodeKey, BTreeSet<Identifier>>;

/// What a walk of the store's edges from a declaration finds.
#[derive(Default)]
struct Walk {
    /// Every key reached, true where a bundle lies above it.
    below: BTreeMap<NodeKey, bool>,
    /// The requests the reached rows make on each key.
    requests: Requests,
    /// Each key a bundle lists, with the repository the list names it at: a member
    /// row in the TUI.
    members: BTreeSet<(NodeKey, String)>,
}

/// A row of the TUI's catalog, or a bundle's member row.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OfferedRow {
    kind: ArtifactKind,
    /// The binding a catalog row guesses from its repository, or a member's own.
    name: String,
    /// A member names its own binding.
    exact: bool,
    location: String,
}

/// What the user does on a row.
#[derive(Debug, Clone)]
struct RowStep {
    row: OfferedRow,
    op: BatchOp,
    /// A version the picker named, `tag`; else the catalog's tag.
    picked: bool,
    tag: String,
}

/// What an action wrote: which binding it took for the row, and whether it declared.
#[derive(Debug)]
struct RowWrote {
    binding: String,
    declares: bool,
}

/// What a resolve the user did not aim at owes the pins of a lock: the requests the
/// lock recorded, those the declaration still makes, and the declaration its hash
/// proves.
struct Owed<'a> {
    set: &'a DesiredSet,
    proof: Option<&'a DesiredSet>,
    recorded: Requests,
    asked: Requests,
    /// The requests parents read as the proof shows them list, which the lock may
    /// not record.
    listed: Requests,
}

impl Owed<'_> {
    /// Whether `pin` answers a request that stands on `key`.
    fn answered(&self, key: &NodeKey, pin: &PinnedIdentifier) -> bool {
        let declared = self.set.table(key.0).get(&key.1).and_then(DeclaredSource::identifier);
        let listed = || {
            let edges = self.listed.get(key).into_iter().flatten();
            edges
                .into_iter()
                .any(|id| answers(pin, key, self.proof, Made::Listed, id))
        };
        match (declared, surviving_edges(key, &self.recorded, &self.asked)) {
            (Some(id), _) => answers(pin, key, self.proof, Made::Unrecorded, id),
            (None, Some(edges)) => edges.iter().any(|id| answers(pin, key, self.proof, Made::Recorded, id)) || listed(),
            (None, None) => listed(),
        }
    }

    /// Whether `pin` must stay: it answers, a digest named outright is the request and
    /// nothing keeps a pin against it, and a new edge releases it. An edge a parent
    /// read as the proof shows it lists is none: the walk behind the hash met it. A
    /// declared key gains none, since no writer resolves an edge onto a direct key.
    fn holds(&self, key: &NodeKey, pin: &PinnedIdentifier) -> bool {
        let mut vouched = self.recorded.clone();
        for (child, ids) in &self.listed {
            vouched.entry(child.clone()).or_default().extend(ids.iter().cloned());
        }
        let elsewhere = surviving_edges(key, &vouched, &self.asked).is_some_and(|edges| {
            edges
                .iter()
                .any(|id| id.digest().is_some_and(|digest| digest != pin.digest()))
        });
        let declared = self.set.table(key.0).contains_key(&key.1);
        self.answered(key, pin) && !elsewhere && (declared || !gained_an_edge(key, &vouched, &self.asked))
    }
}

/// A workspace on tmpfs where the host has one: a scenario saves its lock a
/// handful of times, and a thousand fsyncs would make this the suite's slowest test.
fn scratch_dir() -> tempfile::TempDir {
    tempfile::tempdir_in("/dev/shm")
        .or_else(|_| tempfile::tempdir())
        .unwrap()
}

/// One scenario's workspace.
struct World {
    store: Store,
    access: Arc<dyn OciAccess>,
    set: DesiredSet,
    dir: tempfile::TempDir,
    config: PathBuf,
    lock_path: PathBuf,
    spec: &'static Spec,
    published: BTreeMap<String, (ArtifactKind, Vec<BundleMember>)>,
    /// The edges each published manifest carries, by its digest.
    edges_at: BTreeMap<Digest, Vec<BundleMember>>,
    /// `marketplace.lock` of the one plugin [`PLUGIN`], once a part was written.
    market: Option<MarketplaceLock>,
    /// The bundles the last write of the part read, by pin: a stored part keeps none.
    part_bundles: Vec<PinnedIdentifier>,
    /// The plugin's first write refused, so the shape has no part.
    no_part: bool,
    next_content: usize,
    /// A tag moved at some point.
    moved: bool,
    /// Keys that left the declaration by hand with no relock since.
    hand: BTreeSet<NodeKey>,
    /// Keys that ever did. A relock that carried one proved the lock, so every
    /// relock after it is the same no-op.
    carried_by_hand: BTreeSet<NodeKey>,
    /// What [`Edit::Repoint`] writes.
    repointed: Option<(NodeKey, DeclaredSource)>,
    /// A hand typed a line back or pinned one, with no relock since. It brings an
    /// edge the lock never recorded or a pair no tag served, and what a fresh
    /// resolve refuses then says nothing about the relock, so **exit-agrees** rests.
    typed: bool,
    /// A hand typed a tag beside another tag's digest. A pin keeps that pair past
    /// every relock, so **exit-agrees** rests from here on.
    typed_a_pair: bool,
    /// Keys whose declaration a tool took while their row stayed and the lock went
    /// unproven. A stale hash holds every recorded edge of a pin that names one, so
    /// a relock carries the key past an edge the declaration hid, where a fresh
    /// resolve may exit 78: the price of a hash no declaration produces.
    carried_by_tool: BTreeSet<NodeKey>,
    /// What the last lock edit told the user, each note as a command prints it.
    notes: Vec<String>,
    /// Rows an edit dropped, at their pins, as 0.15.1 drops them: beside a declared
    /// bundle the lock holds no list for and the store cannot speak for, or only
    /// through rows that record no edge ([`left_beside_unwalked`]). Until the next
    /// resolve the lock lacks them and that resolve reads them afresh.
    left_unread: BTreeMap<NodeKey, PinnedIdentifier>,
    /// The `GRIM_HOME` whose store a warm shape filled, and what it holds: each
    /// tag as it pointed then, and every manifest by digest.
    home: Option<crate::store::GrimPaths>,
    warm_tags: BTreeMap<String, Digest>,
    warm_manifests: BTreeSet<Digest>,
}

impl World {
    async fn build(spec: &'static Spec) -> Self {
        let store = Store {
            json: spec.warm,
            ..Store::default()
        };
        let access: Arc<dyn OciAccess> = Arc::new(store.clone());
        let dir = scratch_dir();
        let (config, lock_path) = (dir.path().join("grimoire.toml"), dir.path().join("grimoire.lock"));
        let mut world = Self {
            store,
            access,
            set: DesiredSet::default(),
            dir,
            config,
            lock_path,
            spec,
            published: BTreeMap::new(),
            edges_at: BTreeMap::new(),
            market: None,
            part_bundles: Vec::new(),
            no_part: false,
            next_content: 0,
            moved: false,
            hand: BTreeSet::new(),
            carried_by_hand: BTreeSet::new(),
            repointed: None,
            typed: false,
            typed_a_pair: false,
            carried_by_tool: BTreeSet::new(),
            notes: Vec::new(),
            left_unread: BTreeMap::new(),
            home: None,
            warm_tags: BTreeMap::new(),
            warm_manifests: BTreeSet::new(),
        };
        for (path, text) in spec.files {
            let path = world.dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        for art in spec.arts {
            let mut edges = Vec::new();
            for (kind, name, id) in art.edges {
                edges.push(BundleMember {
                    kind: *kind,
                    name: (*name).to_string(),
                    id: world.expand(id).await,
                });
            }
            world.publish(art.kind, art.id, art.content, &edges).await;
        }
        if let Some((name, value)) = spec.repoint {
            let kind = spec.declare.iter().find(|declared| declared.1 == name).unwrap().0;
            world.repointed = Some((key_of(kind, name), source_of(&world.expand(value).await)));
        }
        world.set = declared(spec);
        let mut lock = world.fresh().await.expect("every shape locks from nothing");
        if !spec.unwalked.is_empty() {
            let (part, _) = world.relock_part(&PluginSelection::All).await;
            part.expect("a part writes before its unwalked edges show");
            world.publish_unwalked(&mut lock).await;
        }
        if let Some(reference) = spec.pin {
            rewrite_pin(&mut lock, spec, reference);
        }
        lock.bundles.retain(|b| !spec.lacks_list(&b.name));
        if spec.pre_change {
            lock = lock.without_advisory_tags();
        }
        for row in lock.iter_artifacts_mut() {
            if spec.no_requires {
                row.requires.clear();
            }
            if spec.one_stamp {
                row.bundles.truncate(1);
            }
        }
        lock_io::save(&world.lock_path, &lock, None).unwrap();
        world.store.take_resolves();
        for (kind, name, id) in spec.typed {
            declare(&mut world.set, &key_of(*kind, name), source_of(id));
            world.typed = true;
        }
        if spec.warm {
            world.warm_up();
        }
        world.record_installs(&lock);
        world
    }

    /// Publish each artifact the shape names in `unwalked` again at its tag, with those
    /// edges beside its own, and move the pins of `lock` and the part onto it. The first
    /// manifest leaves the tag's history: the writer read this one and skipped its edges.
    async fn publish_unwalked(&mut self, lock: &mut GrimoireLock) {
        let ids: BTreeSet<&str> = self.spec.unwalked.iter().map(|(id, _)| *id).collect();
        for id in ids {
            let art = self
                .spec
                .arts
                .iter()
                .find(|art| art.id == id)
                .expect("an unwalked artifact");
            assert_ne!(art.kind, Bundle, "a bundle's list is always walked");
            let (kind, mut edges) = self.published[id].clone();
            for (_, (edge_kind, name, to)) in self.spec.unwalked.iter().filter(|(at, _)| *at == id) {
                let id = self.expand(to).await;
                edges.push(BundleMember {
                    kind: *edge_kind,
                    name: (*name).to_string(),
                    id,
                });
            }
            let old = self.digest_of(id).await;
            self.publish(kind, id, art.content, &edges).await;
            let new = self.digest_of(id).await;
            self.store.served.lock().unwrap().remove(&(id.to_string(), old.clone()));
            let parts = self.market.iter_mut().flat_map(|market| market.plugins.values_mut());
            let rows = std::iter::once(&mut *lock)
                .chain(parts)
                .flat_map(GrimoireLock::iter_artifacts_mut);
            for row in rows {
                if let Some(pin) = row.source.pinned().filter(|pin| pin.digest() == old) {
                    row.source = LockedSource::Registry(pin.clone_with_digest(new.clone()));
                }
            }
        }
    }

    /// Fill the local store as the lock's resolve left it: every manifest and blob
    /// published so far, and each tag where it points now.
    fn warm_up(&mut self) {
        let home = crate::store::GrimPaths::new(self.dir.path().join("home"));
        home.ensure_layout().unwrap();
        let blobs = crate::store::BlobStore::new(home.blobs_dir());
        for (digest, (bytes, _)) in self.store.manifests.lock().unwrap().iter() {
            blobs.put(digest, bytes).unwrap();
            self.warm_manifests.insert(digest.clone());
        }
        for (digest, bytes) in self.store.blobs.lock().unwrap().iter() {
            blobs.put(digest, bytes).unwrap();
        }
        let tags = crate::oci::tag_cache::TagCache::new(home.tags_dir());
        for (reference, digest) in self.store.serving.lock().unwrap().iter() {
            tags.put(&parse(reference), digest).unwrap();
            self.warm_tags.insert(reference.clone(), digest.clone());
        }
        self.home = Some(home);
    }

    /// Write this scope's install state: a record at its pin for each installed
    /// binding of the shape.
    fn record_installs(&mut self, lock: &GrimoireLock) {
        use crate::install::install_state::{InstallRecord, InstallState};
        if self.spec.installed.is_empty() {
            return;
        }
        let workspace = self.dir.path();
        InstallState::ensure_project_state_dir(workspace).unwrap();
        let mut state = InstallState::empty(&InstallState::project_state_path(workspace));
        let installed = lock
            .iter_artifacts()
            .filter(|row| self.spec.installed.contains(&row.name.as_str()));
        for row in installed {
            state.record(InstallRecord {
                kind: row.kind,
                name: row.name.clone(),
                source: row.source.clone(),
                dev: false,
                outputs: Vec::new(),
            });
        }
        state.save().unwrap();
    }

    /// Where an edit reads the local store: the warm one, or an empty one.
    fn edit_target(&self) -> EditTarget<'_> {
        let store = self
            .home
            .clone()
            .unwrap_or_else(|| crate::store::GrimPaths::new(self.config.with_file_name("cold-store")));
        EditTarget {
            config_path: &self.config,
            lock_path: &self.lock_path,
            grim: &Invocation::WALKED,
            store,
        }
    }

    /// `id` with its `{ref}` placeholder replaced by the digest `ref` serves.
    async fn expand(&self, id: &str) -> String {
        let Some((head, reference)) = id.split_once('{') else {
            return id.to_string();
        };
        let reference = reference.trim_end_matches('}');
        let digest = self.digest_of(reference).await;
        format!("{head}{digest}")
    }

    async fn digest_of(&self, reference: &str) -> Digest {
        self.store
            .reg
            .resolve_digest(&parse(reference), Operation::Resolve)
            .await
            .unwrap()
            .unwrap()
    }

    async fn publish(&mut self, kind: ArtifactKind, id: &str, content: &str, edges: &[BundleMember]) {
        self.published.insert(id.to_string(), (kind, edges.to_vec()));
        let id = parse(id);
        let mut layers = Vec::new();
        if kind != Bundle {
            layers.push(Descriptor {
                digest: Algorithm::Sha256.hash(content.as_bytes()),
                media_type: TAR.to_string(),
                size: content.len() as u64,
                annotations: None,
            });
        }
        if kind == Bundle || !edges.is_empty() {
            let bytes = BundleManifest::new(edges.to_vec()).to_layer_bytes().unwrap();
            layers.push(Descriptor {
                digest: self.store.push_blob(&id, &bytes).await.unwrap(),
                media_type: BUNDLE_LAYER_MEDIA_TYPE.to_string(),
                size: bytes.len() as u64,
                annotations: None,
            });
        }
        let annotations = BTreeMap::from([
            ("content".to_string(), content.to_string()),
            (KIND_ANNOTATION.to_string(), kind.to_string()),
        ]);
        let manifest = OciManifest {
            media_type: None,
            artifact_type: None,
            config_media_type: None,
            layers,
            annotations,
        };
        let digest = self.store.push_manifest(&id, &manifest).await.unwrap();
        self.edges_at.insert(digest.clone(), edges.to_vec());
        self.store.put_tag(&id, id.tag_or_latest(), &digest).await.unwrap();
    }

    fn lock(&self) -> GrimoireLock {
        lock_io::load(&self.lock_path).unwrap()
    }

    fn bytes(&self) -> Vec<u8> {
        std::fs::read(&self.lock_path).unwrap()
    }

    /// What a machine with no lock resolves now.
    async fn fresh(&self) -> Result<GrimoireLock, ResolveError> {
        let target = target_lock(
            &self.set,
            None,
            &Refresh::All,
            &self.access,
            &options(),
            self.dir.path(),
            LockRun::project(false),
        )
        .await;
        self.store.take_resolves();
        target.map(|t| t.lock)
    }

    /// `grim lock`: the tags it resolved, the lock saved.
    async fn relock(&mut self) -> Result<Vec<String>, ResolveError> {
        let previous = self.lock();
        let prior = Prior::checked(&previous, &self.set);
        self.store.take_resolves();
        let target = target_lock(
            &self.set,
            Some(&prior),
            &Refresh::Nothing,
            &self.access,
            &options(),
            self.dir.path(),
            LockRun::project(false),
        )
        .await?;
        self.settle(&target.lock, &previous);
        Ok(self.store.take_resolves())
    }

    /// Save what a resolve wrote: nothing a hand did is pending past it.
    fn settle(&mut self, lock: &GrimoireLock, previous: &GrimoireLock) {
        lock_io::save(&self.lock_path, lock, Some(previous)).unwrap();
        self.hand.clear();
        self.typed = false;
        self.left_unread.clear();
    }

    /// `grim add`: `key` joins the declaration at `id`.
    async fn add(&mut self, key: &NodeKey, id: Identifier) -> Result<(), ResolveError> {
        let previous = self.lock();
        let mut after = self.set.clone();
        declare(&mut after, key, DeclaredSource::Registry(id));
        let prior = Prior::edited(&previous, &self.set, &after);
        let target = target_lock(
            &after,
            Some(&prior),
            &Refresh::Nothing,
            &self.access,
            &options(),
            self.dir.path(),
            LockRun::project(false),
        )
        .await?;
        self.settle(&target.lock, &previous);
        self.set = after;
        Ok(())
    }

    /// `grim update`, of `names` or of everything.
    async fn update(&mut self, names: &[String]) -> Result<(), ResolveError> {
        let previous = self.lock();
        let prior = Prior::checked(&previous, &self.set);
        let target = target_lock(
            &self.set,
            Some(&prior),
            &Refresh::from_names(names),
            &self.access,
            &options(),
            self.dir.path(),
            LockRun::project(false),
        )
        .await?;
        self.settle(&target.lock, &previous);
        Ok(())
    }

    /// `grim lock` over `typed`, a declaration in hand, from `previous`, without saving.
    async fn relock_of(&self, previous: &GrimoireLock, typed: &DesiredSet) -> Result<GrimoireLock, ResolveError> {
        self.relock_reading(previous, typed).await.map(|(lock, _)| lock)
    }

    /// [`Self::relock_of`], and the tags it resolved.
    async fn relock_reading(
        &self,
        previous: &GrimoireLock,
        typed: &DesiredSet,
    ) -> Result<(GrimoireLock, Vec<String>), ResolveError> {
        let prior = Prior::checked(previous, typed);
        self.store.take_resolves();
        let target = target_lock(
            typed,
            Some(&prior),
            &Refresh::Nothing,
            &self.access,
            &options(),
            self.dir.path(),
            LockRun::project(false),
        )
        .await;
        let resolved = self.store.take_resolves();
        target.map(|target| (target.lock, resolved))
    }

    /// Whether `grim lock` of the declaration over `previous`, the lock before an edit
    /// dropped `key`, keeps `pin` there: the digest stays, no request in `requests`
    /// names it outright, and no tag of one was read. That is a pin the drop costs.
    async fn relock_keeps(
        &self,
        previous: &GrimoireLock,
        key: &NodeKey,
        pin: &PinnedIdentifier,
        requests: &BTreeSet<Identifier>,
    ) -> bool {
        let Ok((lock, resolved)) = self.relock_reading(previous, &self.set).await else {
            return false;
        };
        let held = pins(&lock).get(key).is_some_and(|now| now.digest() == pin.digest());
        let named_outright = requests.iter().any(|id| id.digest() == Some(pin.digest()));
        let read = requests
            .iter()
            .filter(|id| id.digest().is_none())
            .any(|id| resolved.contains(&id.to_string()));
        held && !named_outright && !read
    }

    /// `grim update` as a name reads it, over the lock on disk, without saving.
    async fn named_update(&self, name: &str) -> Result<GrimoireLock, ResolveError> {
        let previous = self.lock();
        let prior = Prior::checked(&previous, &self.set);
        let target = target_lock(
            &self.set,
            Some(&prior),
            &Refresh::from_names(&[name.to_string()]),
            &self.access,
            &options(),
            self.dir.path(),
            LockRun::project(false),
        )
        .await;
        self.store.take_resolves();
        target.map(|target| target.lock)
    }

    /// `step` on its row, decided by the production `plan_row`, then written as
    /// `perform` writes it: the relock `plan_row` found is saved, any other action
    /// relocks here. A failure writes nothing.
    async fn row_action(&mut self, step: &RowStep) -> anyhow::Result<RowWrote> {
        let previous = self.lock();
        let id = parse(&format!("{}:{}", step.row.location, step.tag));
        let (kind, op) = (step.row.kind, step.op);
        let row = RowAt {
            kind,
            name: &step.row.name,
            exact: step.row.exact,
            location: &step.row.location,
        };
        let action = RowAction {
            row,
            op,
            picked: step.picked,
            id: &id,
        };
        let run = LockRun::project(false);
        let plan = plan_row(&self.set, Some(&previous), action, &self.access, self.dir.path(), run).await?;
        let mut after = self.set.clone();
        let (binding, declares, relocked) = match plan {
            RowPlan::Keep { binding, target } => (binding, false, target),
            RowPlan::Declare { binding } => (binding, true, None),
        };
        let target = match relocked {
            Some(target) => *target,
            None => {
                if declares {
                    declare(&mut after, &(kind, binding.clone()), DeclaredSource::Registry(id));
                }
                let prior = Prior::edited(&previous, &self.set, &after);
                let refresh = match op {
                    BatchOp::Update => Refresh::Names(BTreeSet::from([Selected {
                        kind: Some(kind),
                        name: binding.clone(),
                    }])),
                    BatchOp::Install | BatchOp::Uninstall => Refresh::Nothing,
                };
                refuse_stale_names(&self.set, Some(&prior), &refresh, &Invocation::WALKED)?;
                let options = options();
                let run = LockRun::project(false);
                target_lock(
                    &after,
                    Some(&prior),
                    &refresh,
                    &self.access,
                    &options,
                    self.dir.path(),
                    run,
                )
                .await?
            }
        };
        self.settle(&target.lock, &previous);
        self.set = after;
        self.store.take_resolves();
        Ok(RowWrote { binding, declares })
    }

    /// Every row the TUI can offer over `lock`: a catalog row at each repository a
    /// pin, a request or a declaration sits in, and a member row wherever a bundle
    /// lists one.
    fn offered(&self, lock: &GrimoireLock) -> Vec<OfferedRow> {
        let walk = self.walk_over(lock);
        let pinned = pins(lock)
            .into_iter()
            .map(|(key, pin)| (key.0, pin.registry_repository()));
        let asked = walk
            .requests
            .iter()
            .flat_map(|(key, ids)| ids.iter().map(|id| (key.0, id.registry_repository())));
        let roots = self.roots();
        let declared = roots
            .iter()
            .filter_map(|(key, source)| Some((key.0, source.identifier()?.registry_repository())));
        let mut rows: BTreeSet<OfferedRow> = pinned
            .chain(asked)
            .chain(declared)
            .map(|(kind, location)| OfferedRow {
                kind,
                name: location.rsplit('/').next().unwrap().to_string(),
                exact: false,
                location,
            })
            .collect();
        rows.extend(walk.members.into_iter().map(|((kind, name), location)| OfferedRow {
            kind,
            name,
            exact: true,
            location,
        }));
        rows.into_iter().collect()
    }

    /// The binding that holds `row`, as the design record states it: a direct entry
    /// at the row's location under any binding, or a key `reached` holds whose pin
    /// (`pinned_here`) or a request onto it sits there. The row's own name answers
    /// first, and a member also holds its own name wherever it is bound.
    fn row_holder(
        &self,
        row: &OfferedRow,
        reached: &BTreeSet<NodeKey>,
        pinned_here: &BTreeSet<NodeKey>,
        asked: &Requests,
    ) -> Option<String> {
        let table = self.set.table(row.kind);
        let here = |id: &Identifier| id.registry_repository() == row.location;
        let declared = table
            .iter()
            .filter(|(_, source)| source.identifier().is_some_and(here))
            .map(|(binding, _)| binding.clone());
        let locked = reached
            .iter()
            .filter(|key| key.0 == row.kind && !table.contains_key(&key.1))
            .filter(|key| pinned_here.contains(*key) || asked.get(*key).is_some_and(|ids| ids.iter().any(here)))
            .map(|key| key.1.clone());
        let held: Vec<String> = declared.chain(locked).collect();
        let bound = table.contains_key(&row.name) || reached.contains(&(row.kind, row.name.clone()));
        if held.contains(&row.name) || (row.exact && bound) {
            Some(row.name.clone())
        } else if row.exact {
            None
        } else {
            held.into_iter().next()
        }
    }

    /// Whether the requests on `key` can settle at two digests. A 78 is owed nowhere
    /// else, and a direct line holds them all off.
    fn may_conflict(&self, key: &NodeKey, lock: &GrimoireLock, asked: &Requests) -> bool {
        if self.set.table(key.0).contains_key(&key.1) {
            return false;
        }
        let held = pins(lock).get(key).map(PinnedIdentifier::digest);
        let served = |id: &Identifier| id.digest().or_else(|| self.store.serving(&id.to_string()));
        let digests: BTreeSet<Digest> = asked
            .get(key)
            .into_iter()
            .flatten()
            .filter_map(served)
            .chain(held)
            .collect();
        digests.len() > 1
    }

    /// A hand types a line for `key` into the declaration, as often after an edit took it.
    fn redeclare(&mut self, key: &NodeKey, source: DeclaredSource) {
        declare(&mut self.set, key, source);
        self.hand.remove(key);
        self.typed = true;
    }

    /// A hand pins the declared `key` at `digest`. A typed pair is the request
    /// itself, so it counts as served.
    fn pin_by_hand(&mut self, key: &NodeKey, id: &Identifier, digest: Digest) {
        let served = (id.to_string(), digest.clone());
        self.store.served.lock().unwrap().insert(served);
        declare(
            &mut self.set,
            key,
            DeclaredSource::Registry(id.clone_with_digest(digest)),
        );
        self.typed_a_pair = true;
    }

    fn apply(&mut self, edit: Edit, key: &NodeKey) {
        let before = self.set.clone();
        let previous = self.lock();
        self.notes.clear();
        match edit {
            Edit::Remove => {
                undeclare(&mut self.set, key);
                let stored = self.edit_target().stored_members(&self.set, &previous);
                let out = drop_from_lock(
                    &previous,
                    key.0,
                    &key.1,
                    &before,
                    &self.set,
                    &Invocation::WALKED,
                    &stored,
                );
                lock_io::save(&self.lock_path, &out.lock, Some(&previous)).unwrap();
                self.notes = out.notes.iter().map(ToString::to_string).collect();
            }
            Edit::Undeclare => {
                let mut set = self.set.clone();
                let (_, notes) = undeclare_and_unlock(
                    &self.edit_target(),
                    &ConfigOptions::default(),
                    &[],
                    &mut set,
                    key.0,
                    &key.1,
                )
                .unwrap();
                self.set = set;
                self.notes = notes.iter().map(ToString::to_string).collect();
            }
            Edit::Hand => {
                undeclare(&mut self.set, key);
                self.hand.insert(key.clone());
                self.carried_by_hand.insert(key.clone());
            }
            Edit::Repoint => {
                let (key, source) = self.repointed.clone().unwrap();
                if let Some(pin) = source
                    .identifier()
                    .and_then(|id| PinnedIdentifier::try_from(id.clone()).ok())
                {
                    self.store.served.lock().unwrap().insert((request(&pin), pin.digest()));
                    self.typed_a_pair = true;
                }
                declare(&mut self.set, &key, source);
            }
        }
        let lock = self.lock();
        if edit.writes_the_lock() && !self.proven(&lock) && pins(&lock).contains_key(key) {
            self.carried_by_tool.insert(key.clone());
        }
    }

    async fn move_tags(&mut self, mv: Move) {
        if mv == Move::None {
            return;
        }
        let key = (self.spec.key.0, self.spec.key.1.to_string());
        let pin = pins(&self.lock()).get(&key).map(request);
        let tags: Vec<&str> = self.spec.tags.to_vec();
        let pin_tag = pin
            .as_deref()
            .filter(|p| tags.contains(p))
            .unwrap_or(tags[0])
            .to_string();
        let others: Vec<String> = tags
            .iter()
            .filter(|t| **t != pin_tag)
            .map(ToString::to_string)
            .collect();
        match mv {
            Move::None => {}
            Move::PinTag => self.retag_new(&[pin_tag]).await,
            Move::OtherTag => self.retag_new(&others).await,
            Move::AllToOne => {
                let mut all = others;
                all.push(pin_tag);
                self.retag_new(&all).await;
            }
            Move::AllApart => {
                for tag in others.into_iter().chain([pin_tag]) {
                    self.retag_new(&[tag]).await;
                }
            }
            Move::PinTagJoinsOther => {
                if let Some(other) = others.first() {
                    let digest = self.digest_of(other).await;
                    let id = parse(&pin_tag);
                    self.store.put_tag(&id, id.tag_or_latest(), &digest).await.unwrap();
                }
            }
        }
        self.moved = true;
    }

    /// Point every one of `tags` at one newly published digest.
    async fn retag_new(&mut self, tags: &[String]) {
        let Some(first) = tags.first() else { return };
        self.next_content += 1;
        let content = format!("moved {}", self.next_content);
        let (kind, edges) = self.published[first].clone();
        self.publish(kind, first, &content, &edges).await;
        let digest = self.digest_of(first).await;
        for tag in &tags[1..] {
            let id = parse(tag);
            self.store.put_tag(&id, id.tag_or_latest(), &digest).await.unwrap();
        }
    }

    /// Point `tag` at a newly published digest whose manifest lists `gained` beside the
    /// edges it had: a moved tag that changed what it requires.
    async fn retag_gaining(&mut self, tag: &str, gained: Edge) {
        self.next_content += 1;
        let content = format!("moved {}", self.next_content);
        let (kind, mut edges) = self.published[tag].clone();
        edges.push(BundleMember {
            kind: gained.0,
            name: gained.1.to_string(),
            id: self.expand(gained.2).await,
        });
        self.publish(kind, tag, &content, &edges).await;
        self.moved = true;
    }

    fn pair_false(&self, lock: &GrimoireLock) -> Vec<String> {
        // A bundle asked by digest alone records that digest's short form as its tag.
        let names_a_tag = |pin: &PinnedIdentifier| pin.tag().is_some_and(|tag| tag != pin.digest().to_short_string());
        pins(lock)
            .into_iter()
            .filter(|(_, pin)| names_a_tag(pin))
            .filter(|(_, pin)| !self.store.served(request(pin), pin.digest()))
            .map(|(key, pin)| format!("{}/{} wrote {pin}", key.0, key.1))
            .collect()
    }

    fn proven(&self, lock: &GrimoireLock) -> bool {
        lock.metadata.declaration_hash == self.set.declaration_hash_cached()
    }

    /// Every key `roots` reach through the edges the store publishes, whether a
    /// bundle lies on the way, and the requests made onto each. A key `pins` holds
    /// reads its edges at that digest, any other at the digests `unpinned` gives
    /// its request. A root holds itself, so no bundle above it counts.
    fn walk(
        &self,
        roots: &BTreeMap<NodeKey, DeclaredSource>,
        pins: &BTreeMap<NodeKey, PinnedIdentifier>,
        unpinned: impl Fn(&Identifier) -> Vec<Digest>,
    ) -> Walk {
        self.walk_marking(roots, pins, unpinned, |key, _| key.0 == Bundle)
    }

    /// [`Self::walk`], with `marks` naming the keys, by their request, whose
    /// descendants count as below.
    fn walk_marking(
        &self,
        roots: &BTreeMap<NodeKey, DeclaredSource>,
        pins: &BTreeMap<NodeKey, PinnedIdentifier>,
        unpinned: impl Fn(&Identifier) -> Vec<Digest>,
        marks: impl Fn(&NodeKey, Option<&Identifier>) -> bool,
    ) -> Walk {
        self.walk_leaving(roots, pins, unpinned, marks, |_| false)
    }

    /// [`Self::walk_marking`], with `leaf` naming the keys whose edges it does not
    /// follow.
    fn walk_leaving(
        &self,
        roots: &BTreeMap<NodeKey, DeclaredSource>,
        pins: &BTreeMap<NodeKey, PinnedIdentifier>,
        unpinned: impl Fn(&Identifier) -> Vec<Digest>,
        marks: impl Fn(&NodeKey, Option<&Identifier>) -> bool,
        leaf: impl Fn(&NodeKey) -> bool,
    ) -> Walk {
        let mut out = Walk::default();
        let mut queue: Vec<(NodeKey, Option<Identifier>, bool)> = roots
            .iter()
            .map(|(key, source)| (key.clone(), source.identifier().cloned(), false))
            .collect();
        while let Some((key, request, below)) = queue.pop() {
            let below = below && !roots.contains_key(&key);
            if out.below.get(&key).is_some_and(|seen| *seen || !below) {
                continue;
            }
            out.below.insert(key.clone(), below);
            let under = below || marks(&key, request.as_ref());
            // A line that names other bytes than the pin reads as what it names: the
            // relock this walk stands for takes the pin off it.
            let declared = roots.get(&key).and_then(DeclaredSource::identifier);
            let pin = pins
                .get(&key)
                .filter(|pin| declared.is_none_or(|id| may_stand(pin, id)))
                .filter(|pin| {
                    request
                        .as_ref()
                        .and_then(Identifier::digest)
                        .is_none_or(|d| d == pin.digest())
                });
            let (edges, parent) = match (roots.get(&key), pin) {
                _ if leaf(&key) => (Vec::new(), None),
                (Some(DeclaredSource::Path(path)), _) => (self.path_members(path), None),
                (_, Some(pin)) => (self.edges_of(&pin.digest()), Some(pin.as_identifier().clone())),
                (_, None) => match request.as_ref() {
                    Some(id) => {
                        let digests = id.digest().map_or_else(|| unpinned(id), |d| vec![d]);
                        (
                            digests.iter().flat_map(|d| self.edges_of(d)).collect(),
                            Some(id.clone()),
                        )
                    }
                    None => (Vec::new(), None),
                },
            };
            for edge in edges {
                let child = (edge.kind, edge.name.clone());
                let id = requested(&edge, parent.as_ref());
                if let Some(id) = &id {
                    out.requests.entry(child.clone()).or_default().insert(id.clone());
                    if key.0 == Bundle {
                        out.members.insert((child.clone(), id.registry_repository()));
                    }
                }
                queue.push((child, id, under));
            }
        }
        out
    }

    /// The requests a relock over `before` reads from parents it reads as the lock's
    /// proof shows them, each parent at its pin in `read`: one read at the digest
    /// `before` pins it at, one with no pin that the proof and the declaration hold
    /// alike, or one with no pin that such a parent lists. None without a proof.
    fn listed_requests(
        &self,
        before: &GrimoireLock,
        read: &BTreeMap<NodeKey, PinnedIdentifier>,
        proof: Option<&DesiredSet>,
    ) -> Requests {
        let Some(proof) = proof else {
            return Requests::new();
        };
        let (was, roots) = (pins(before), self.roots());
        let alike = |key: &NodeKey| {
            let declared = proof.table(key.0).get(&key.1);
            declared.is_some() && roots.get(key) == declared
        };
        let mut out = Requests::new();
        let mut seen = BTreeSet::new();
        let mut queue: Vec<(NodeKey, Option<Identifier>, bool)> = roots
            .iter()
            .map(|(key, source)| (key.clone(), source.identifier().cloned(), false))
            .collect();
        while let Some((key, request, by_a_listed_edge)) = queue.pop() {
            let proven = match was.get(&key) {
                Some(pin) => read.get(&key).is_some_and(|now| now.digest() == pin.digest()),
                None => alike(&key) || by_a_listed_edge,
            };
            if !seen.insert((key.clone(), proven)) {
                continue;
            }
            let (edges, parent) = match (roots.get(&key), read.get(&key)) {
                (Some(DeclaredSource::Path(path)), _) => (self.path_members(path), None),
                (_, Some(pin)) => (self.edges_of(&pin.digest()), Some(pin.as_identifier().clone())),
                (_, None) => match request.as_ref() {
                    Some(id) => {
                        let digests = id.digest().into_iter().chain(self.store.serving(&id.to_string()));
                        let edges = digests.take(1).flat_map(|digest| self.edges_of(&digest)).collect();
                        (edges, Some(id.clone()))
                    }
                    None => (Vec::new(), None),
                },
            };
            // A row other than a bundle's lists only the edges it records: the writer
            // of a lock with none, as grim 0.15.1 is, never walked the others.
            let row = before
                .iter_artifacts()
                .find(|row| (row.kind, &row.name) == (key.0, &key.1));
            let walked = |edge: &BundleMember| row.is_none_or(|row| row.requires.contains(edge));
            for edge in edges {
                let child = (edge.kind, edge.name.clone());
                let id = requested(&edge, parent.as_ref());
                let listed = proven && walked(&edge);
                if let Some(id) = id.as_ref().filter(|_| listed) {
                    out.entry(child.clone()).or_default().insert(id.clone());
                }
                queue.push((child, id, listed));
            }
        }
        out
    }

    /// The edges the store publishes at `digest`.
    fn edges_of(&self, digest: &Digest) -> Vec<BundleMember> {
        self.edges_at.get(digest).cloned().unwrap_or_default()
    }

    /// The keys the local store shows the bundle declared at `id` provides, for an
    /// edit of `lock`; `None` where it cannot speak for that bundle.
    fn stored_closure(&self, id: &Identifier, lock: &GrimoireLock) -> Option<BTreeSet<NodeKey>> {
        let tag = |id: &Identifier| self.warm_tags.get(&id.to_string()).cloned();
        let held = pins(lock);
        let mut keys = BTreeSet::new();
        let mut read = BTreeSet::new();
        let mut frontier = vec![(id.clone(), tag(id)?)];
        while let Some((at, digest)) = frontier.pop() {
            if !read.insert(digest.clone()) {
                continue;
            }
            if !self.warm_manifests.contains(&digest) {
                return None;
            }
            for edge in self.edges_of(&digest) {
                let key = (edge.kind, edge.name.clone());
                let member = requested(&edge, Some(&at))?;
                let pin = held.get(&key).map(PinnedIdentifier::digest).or_else(|| tag(&member))?;
                keys.insert(key);
                frontier.push((member, pin));
            }
        }
        // A list without a row this lock stamps with the bundle is another tag's list.
        let stamped = lock.iter_artifacts().filter(|row| carries_stamp(row, id));
        let holds = stamped
            .into_iter()
            .all(|row| keys.contains(&(row.kind, row.name.clone())));
        holds.then_some(keys)
    }

    /// Whether `key`'s row in `lock` records no edge while the store holds its
    /// manifest, which names a requires layer that lists edges or that the home lacks.
    fn names_requires(&self, lock: &GrimoireLock, key: &NodeKey) -> bool {
        let Some(row) = lock
            .iter_artifacts()
            .find(|row| (row.kind, &row.name) == (key.0, &key.1))
        else {
            return false;
        };
        let Some(pin) = row.source.pinned().filter(|_| row.requires.is_empty()) else {
            return false;
        };
        let digest = pin.digest();
        if !self.warm_manifests.contains(&digest) {
            return false;
        }
        match self.stores_requires_of(&digest, row.kind) {
            Some(true) => !self.edges_of(&digest).is_empty(),
            Some(false) => true,
            None => false,
        }
    }

    /// What the home shows `key`'s requires reach over `previous`'s pins alone, its
    /// row in `lock`: the keys, and whether the home held every link. A child with no
    /// pin is a link it lacks, since the relock reads that tag from the registry.
    fn stored_reach(&self, previous: &GrimoireLock, lock: &GrimoireLock, key: &NodeKey) -> (BTreeSet<NodeKey>, bool) {
        let (held, mut keys, mut whole) = (pins(previous), BTreeSet::new(), true);
        let Some(pin) = pins(lock).remove(key) else {
            return (keys, false);
        };
        let (mut read, mut frontier) = (
            BTreeSet::new(),
            vec![(key.0, pin.as_identifier().clone(), pin.digest())],
        );
        while let Some((kind, at, digest)) = frontier.pop() {
            if !read.insert(digest.clone()) {
                continue;
            }
            if !self.warm_manifests.contains(&digest) || self.stores_requires_of(&digest, kind) == Some(false) {
                whole = false;
                continue;
            }
            for edge in self.edges_of(&digest) {
                let child = (edge.kind, edge.name.clone());
                let next = requested(&edge, Some(&at))
                    .and_then(|id| Some((held.get(&child).map(PinnedIdentifier::digest)?, id)));
                keys.insert(child);
                match next {
                    Some((pin, id)) => frontier.push((edge.kind, id, pin)),
                    None => whole = false,
                }
            }
        }
        (keys, whole)
    }

    /// Whether the home holds the requires layer the manifest at `digest` names;
    /// `None` when it names none or the home is cold.
    fn stores_requires_of(&self, digest: &Digest, kind: ArtifactKind) -> Option<bool> {
        let home = self.home.as_ref()?;
        let (bytes, _) = self.store.manifests.lock().unwrap().get(digest).cloned()?;
        let manifest = OciManifest::from_json(&bytes).unwrap();
        let Ok(crate::oci::manifest::LayerLayout::ContentWithRequires { requires, .. }) = manifest.layout(kind) else {
            return None;
        };
        let blobs = crate::store::BlobStore::new(home.blobs_dir());
        Some(blobs.get(&requires.digest).unwrap().is_some())
    }

    /// The walk of the declaration over `lock`'s pins, a key with no row at what
    /// its request serves now: what a relock that keeps every pin reads.
    fn walk_over(&self, lock: &GrimoireLock) -> Walk {
        let serving = |id: &Identifier| self.store.serving(&id.to_string()).into_iter().collect();
        self.walk(&self.roots(), &pins(lock), serving)
    }

    /// The members the path bundle at `path` lists, as its file says.
    fn path_members(&self, path: &PathSource) -> Vec<BundleMember> {
        let text = std::fs::read_to_string(self.dir.path().join(path.as_str())).unwrap();
        let tables: BTreeMap<String, BTreeMap<String, String>> = toml::from_str(&text).unwrap();
        let mut out = Vec::new();
        for (table, members) in tables {
            let kind = ArtifactKind::ALL.into_iter().find(|k| k.subdir() == table).unwrap();
            out.extend(members.into_iter().map(|(name, id)| BundleMember { kind, name, id }));
        }
        out
    }

    /// Every declaration of the project, by key.
    fn roots(&self) -> BTreeMap<NodeKey, DeclaredSource> {
        let mut out = BTreeMap::new();
        for kind in ArtifactKind::ALL {
            for (name, source) in self.set.table(kind) {
                out.insert((kind, name.clone()), source.clone());
            }
        }
        out
    }

    /// The plugin [`PLUGIN`] of a `marketplace.toml` beside the project: every
    /// registry declaration of the shape as an include. A path bundle is none.
    fn manifest(&self) -> MarketplaceManifest {
        let include = self.spec.declare.iter().map(|(_, _, id)| (*id).to_string());
        let include = include.filter(|id| PathSource::parse(id).is_err()).collect();
        MarketplaceManifest {
            path: self.dir.path().join("marketplace.toml"),
            plugins: BTreeMap::from([(
                PLUGIN.to_string(),
                PluginDecl {
                    include,
                    ..PluginDecl::default()
                },
            )]),
            marketplace: None,
        }
    }

    /// `grim update --marketplace` or the export stage on the plugin, and what it
    /// logged. A refusal keeps the part as it was, as the caller saves nothing.
    async fn relock_part(&mut self, selection: &PluginSelection) -> (Result<(), crate::error::Error>, String) {
        let scope = FetchScope {
            registries: Vec::new(),
            short_id_default: "ghcr.io/acme".to_string(),
            scope: ConfigScope::Project,
            warnings: Vec::new(),
        };
        let manifest = self.manifest();
        let capture = LogCapture::start();
        let resolved = resolve_marketplace(
            &manifest,
            self.market.as_ref(),
            selection,
            &scope,
            &self.access,
            IncludeOrigin::Declared,
            false,
        )
        .await;
        let logs = capture.text();
        self.store.take_resolves();
        let outcome = resolved.map(|resolution| {
            self.part_bundles = resolution.bundle_pins.get(PLUGIN).cloned().unwrap_or_default();
            self.market = Some(resolution.lock);
        });
        (outcome, logs)
    }

    fn part(&self) -> Option<&GrimoireLock> {
        self.market.as_ref().and_then(|lock| lock.plugins.get(PLUGIN))
    }

    /// The key a write of the part from nothing refuses with 78 on, on the store as it
    /// stands. The part is left as it was.
    async fn fresh_part_conflict(&mut self) -> Option<NodeKey> {
        let saved = (self.market.take(), std::mem::take(&mut self.part_bundles));
        let (outcome, _) = self.relock_part(&PluginSelection::All).await;
        (self.market, self.part_bundles) = saved;
        let error = anyhow::Error::from(outcome.err()?);
        resolve_error(&error).and_then(conflict_key)
    }

    /// Whether the export stage, run on the part as it stands, writes a row for
    /// `key`, or for a bundle reads one; `None` when it refuses. The part is left as
    /// it was.
    async fn export_holds(&mut self, key: &NodeKey) -> Option<bool> {
        let saved = (self.market.clone(), self.part_bundles.clone());
        let (outcome, _) = self
            .relock_part(&PluginSelection::Reconcile(BTreeSet::from([PLUGIN.to_string()])))
            .await;
        let repositories = self.bundle_repositories(&key.1);
        let holds = outcome.ok().map(|()| match key.0 {
            Bundle => self
                .part_bundles
                .iter()
                .any(|pin| repositories.contains(&pin.registry_repository())),
            _ => self.part().is_some_and(|part| pins(part).contains_key(key)),
        });
        (self.market, self.part_bundles) = saved;
        holds
    }

    /// Every repository the shape asks the bundle bound as `name` at.
    fn bundle_repositories(&self, name: &str) -> BTreeSet<String> {
        let declared = self
            .includes()
            .into_iter()
            .filter(|(key, _)| *key == (Bundle, name.to_string()))
            .filter_map(|(_, source)| source.identifier().cloned());
        let edges = self.published.iter().flat_map(|(parent, (_, edges))| {
            let parent = parse(parent);
            edges
                .iter()
                .filter(|edge| edge.kind == Bundle && edge.name == name)
                .filter_map(move |edge| requested(edge, Some(&parent)))
        });
        declared.chain(edges).map(|id| id.registry_repository()).collect()
    }

    /// The registry declarations of the shape, which the plugin includes.
    fn includes(&self) -> BTreeMap<NodeKey, DeclaredSource> {
        self.spec
            .declare
            .iter()
            .map(|(kind, name, id)| (key_of(*kind, name), source_of(id)))
            .filter(|(_, source)| matches!(source, DeclaredSource::Registry(_)))
            .collect()
    }

    /// The keys a relock of `part` may move, before it or after it: those below a
    /// bundle it follows by its tag, a bundle requested by tag whose digest no stamp
    /// of `part` records, and, on the export stage, below a tagless pin, which resolves
    /// afresh there. A member pick attests the part, so a recorded edge holds a tagless
    /// pin. Each of those, and `member`, which the pick selects, reads every digest its
    /// request ever served; a bundle a stamp records reads that digest, and every other
    /// key the pin `part` held.
    fn followed_below(&self, part: &GrimoireLock, member: Option<&NodeKey>) -> BTreeSet<NodeKey> {
        let recorded = recorded_digests(part);
        let held: BTreeMap<NodeKey, PinnedIdentifier> = pins(part)
            .into_iter()
            .filter(|(key, pin)| Some(key) != member && pin.tag().is_some())
            .collect();
        let tagless: BTreeSet<NodeKey> = pins(part)
            .into_iter()
            .filter(|(_, pin)| pin.tag().is_none())
            .map(|(key, _)| key)
            .collect();
        let at_record = |id: &Identifier| recorded.get(&stamp_of(id)?).cloned().flatten();
        // A picked bundle reads its tag whatever the part recorded.
        let picked = member
            .filter(|key| key.0 == Bundle)
            .map(|key| self.bundle_repositories(&key.1))
            .unwrap_or_default();
        let reads = |id: &Identifier| match at_record(id).filter(|_| !picked.contains(&id.registry_repository())) {
            Some(digest) => vec![digest],
            None => self.store.ever_served(&id.to_string()),
        };
        let follows = |key: &NodeKey, id: Option<&Identifier>| {
            let by_tag = |id: &Identifier| id.tag().is_some() && at_record(id).is_none();
            let selected = key.0 == Bundle && member == Some(key);
            (member.is_none() && tagless.contains(key)) || selected || (key.0 == Bundle && id.is_some_and(by_tag))
        };
        let reached = self.walk_marking(&self.includes(), &held, reads, follows).below;
        reached
            .into_iter()
            .filter(|(_, below)| *below)
            .map(|(key, _)| key)
            .collect()
    }

    /// The keys a member pick of `member` may change: what it reaches through the
    /// edges its rows record, in the part before the pick or after it, and through
    /// every member list a bundle it reaches published at a digest a request onto it,
    /// an include or a pin names or a tag of one ever served.
    fn part_closure(&self, member: &NodeKey, before: &GrimoireLock, after: &GrimoireLock) -> BTreeSet<NodeKey> {
        let held = [pins(before), pins(after)];
        let mut asked = requests(before);
        for (key, ids) in requests(after) {
            asked.entry(key).or_default().extend(ids);
        }
        for (key, source) in self.includes() {
            asked.entry(key).or_default().extend(source.identifier().cloned());
        }
        // A part keeps no bundle row, so no row records a bundle's edge onto another.
        for (parent, (_, edges)) in &self.published {
            for edge in edges.iter().filter(|edge| edge.kind == Bundle) {
                let onto = (edge.kind, edge.name.clone());
                asked
                    .entry(onto)
                    .or_default()
                    .extend(requested(edge, Some(&parse(parent))));
            }
        }
        let mut closure = BTreeSet::new();
        let mut queue = vec![member.clone()];
        while let Some(key) = queue.pop() {
            if !closure.insert(key.clone()) {
                continue;
            }
            for row in before.iter_artifacts().chain(after.iter_artifacts()) {
                if (row.kind, &row.name) == (key.0, &key.1) {
                    queue.extend(row.requires.iter().map(|edge| (edge.kind, edge.name.clone())));
                }
            }
            if key.0 != Bundle {
                continue;
            }
            let mut digests: BTreeSet<Digest> = held
                .iter()
                .filter_map(|pins| pins.get(&key))
                .map(|pin| pin.digest())
                .collect();
            for id in asked.get(&key).into_iter().flatten() {
                digests.extend(
                    id.digest()
                        .map_or_else(|| self.store.ever_served(&id.to_string()), |d| vec![d]),
                );
            }
            for edge in digests.iter().flat_map(|digest| self.edges_of(digest)) {
                if let Some(id) = requested(&edge, None) {
                    asked.entry((edge.kind, edge.name.clone())).or_default().insert(id);
                }
                queue.push((edge.kind, edge.name.clone()));
            }
        }
        closure
    }
}

/// The stamp a bundle requested at `id` leaves on the rows it provides, by repository
/// and tag; none for a digest request, whose stamp records nothing to keep.
fn stamp_of(id: &Identifier) -> Option<(String, String)> {
    Some((id.registry_repository(), id.tag()?.to_string()))
}

/// A stamp's tag and the digest it records, `<tag>@<digest>`.
fn split_stamp(tag: &str) -> (&str, Option<Digest>) {
    match tag.split_once('@') {
        Some((tag, digest)) => (tag, Digest::try_from(digest).ok()),
        None => (tag, None),
    }
}

/// The digest each bundle stamp of `part` records, by the stamp's repository and tag.
/// A stamp rows record at two digests records none.
fn recorded_digests(part: &GrimoireLock) -> BTreeMap<(String, String), Option<Digest>> {
    let mut out: BTreeMap<(String, String), Option<Digest>> = BTreeMap::new();
    for stamp in part.iter_artifacts().flat_map(|row| &row.bundles) {
        let (tag, Some(digest)) = split_stamp(&stamp.tag) else {
            continue;
        };
        out.entry((stamp.repo.clone(), tag.to_string()))
            .and_modify(|seen| {
                if seen.as_ref() != Some(&digest) {
                    *seen = None;
                }
            })
            .or_insert(Some(digest));
    }
    out
}

/// How old the grim that last wrote a part is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vintage {
    /// This grim: tags in `pinned`, a digest in every bundle stamp.
    Current,
    /// grim 0.15.1: no tag in `pinned`, no digest in a stamp.
    Old,
    /// grim 0.15.1's member update over this grim's part: no tag in `pinned`, the
    /// stamps as this grim wrote them.
    Mixed,
}

/// `part` as a grim of `vintage` writes it.
fn aged(part: &GrimoireLock, vintage: Vintage) -> GrimoireLock {
    let mut part = match vintage {
        Vintage::Current => return part.clone(),
        Vintage::Old | Vintage::Mixed => part.clone().without_advisory_tags(),
    };
    if vintage == Vintage::Old {
        let lists = [
            &mut part.skills,
            &mut part.rules,
            &mut part.agents,
            &mut part.mcp,
            &mut part.hooks,
            &mut part.settings,
        ];
        for row in lists.into_iter().flatten() {
            for stamp in &mut row.bundles {
                stamp.tag = split_stamp(&stamp.tag).0.to_string();
            }
        }
    }
    part
}

const PLUGIN: &str = "team";

/// The source a line of the declaration reads as.
fn source_of(value: &str) -> DeclaredSource {
    match PathSource::parse(value) {
        Ok(path) => DeclaredSource::Path(path),
        Err(_) => DeclaredSource::Registry(parse(value)),
    }
}

fn declared(spec: &Spec) -> DesiredSet {
    let mut tables: BTreeMap<ArtifactKind, BTreeMap<String, DeclaredSource>> = BTreeMap::new();
    for (kind, name, id) in spec.declare {
        tables
            .entry(*kind)
            .or_default()
            .insert((*name).to_string(), source_of(id));
    }
    let mut take = |kind| tables.remove(&kind).unwrap_or_default();
    let (skills, rules, bundles) = (take(Skill), take(Rule), take(Bundle));
    let (agents, mcp, hooks, settings) = (take(Agent), take(Mcp), take(Hook), take(ArtifactKind::Settings));
    let mut set = DesiredSet::from_parts_with_bundles(skills, rules, bundles);
    (set.agents, set.mcp, set.hooks, set.settings) = (agents, mcp, hooks, settings);
    set.invalidate_declaration_hash_cache();
    set
}

fn undeclare(set: &mut DesiredSet, (kind, name): &NodeKey) {
    match kind {
        Skill => set.skills.remove(name),
        Rule => set.rules.remove(name),
        Bundle => set.bundles.remove(name),
        Agent => set.agents.remove(name),
        Mcp => set.mcp.remove(name),
        Hook => set.hooks.remove(name),
        ArtifactKind::Settings => set.settings.remove(name),
    };
    set.invalidate_declaration_hash_cache();
}

fn declare(set: &mut DesiredSet, (kind, name): &NodeKey, source: DeclaredSource) {
    let name = name.clone();
    match kind {
        Skill => set.skills.insert(name, source),
        Rule => set.rules.insert(name, source),
        Bundle => set.bundles.insert(name, source),
        Agent => set.agents.insert(name, source),
        Mcp => set.mcp.insert(name, source),
        Hook => set.hooks.insert(name, source),
        ArtifactKind::Settings => set.settings.insert(name, source),
    };
    set.invalidate_declaration_hash_cache();
}

/// Put the shape's key on another tag of its digest, a true pair a fresh
/// resolve would not pick.
fn rewrite_pin(lock: &mut GrimoireLock, spec: &Spec, reference: &str) {
    let list = lock.skills.iter_mut();
    for entry in list.filter(|a| a.name == spec.key.1) {
        let digest = entry.source.pinned().unwrap().digest();
        let pinned = PinnedIdentifier::try_from(parse(reference).clone_with_digest(digest)).unwrap();
        entry.source = LockedSource::Registry(pinned);
    }
}

fn key_of(kind: ArtifactKind, name: &str) -> NodeKey {
    (kind, name.to_string())
}

/// What one relock owes, checked against what a fresh resolve says on the same
/// store. `row` names the scenario in every failure.
async fn check_relock(world: &mut World, row: &str, failures: &mut Vec<String>) {
    check_proof(world, row, failures);
    let before = world.lock();
    let before_bytes = world.bytes();
    let (by_hand, carried_by_hand) = (world.hand.clone(), world.carried_by_hand.clone());
    let carried_by_tool = world.carried_by_tool.clone();
    let typed = world.typed || world.typed_a_pair;
    let proven = world.proven(&before);
    let left_unread = world.left_unread.clone();
    let (before_pins, onto) = (pins(&before), requests(&before));
    // What the declaration asks over the old pins: a typed parent with no row
    // brings an edge the lock never recorded.
    let asked = world.walk_over(&before).requests;
    let fresh = world.fresh().await;
    let refused_fresh = fresh.as_ref().err().and_then(conflict_key);
    let outcome = world.relock().await;
    let mut fail = |clause: &str, detail: String| failures.push(format!("{clause}: {row}: {detail}"));

    let resolves = match outcome {
        Err(error) => {
            match (conflict_key(&error), &refused_fresh) {
                (None, _) => fail("relock-error", error.to_string()),
                // A pin that names an edge reads the record, not the tag, so a tag
                // moved into agreement with a digest edge does not settle it: only
                // `grim update` follows a tag.
                (Some(key), None) if world.moved && named_any(&before_pins, &onto, &key) => {}
                // A line typed back brought an edge the lock never recorded. It
                // resolves, as a request the pin does not name does, so a 78 a fresh
                // resolve gives too is no carry the hand rule owes.
                (Some(key), Some(_)) if typed && gained_an_edge(&key, &onto, &asked) => {}
                (Some(key), None) => fail(
                    "exit-agrees",
                    format!("relock refused {}/{} where a fresh resolve accepts", key.0, key.1),
                ),
                // A hand-deleted line leaves a stale hash and nothing else, so a
                // once-direct key reads as an honest diamond: `grim lock` carries it
                // where a fresh resolve exits 78. Decided in the design record,
                // § The pin rule, "Hand edits, decided". A digest edge at another
                // digest stays out: no pin can answer one.
                (Some(key), Some(_)) if by_hand.contains(&key) && carries_by_hand(&before_pins, &onto, &key) => {
                    fail(
                        "exit-agrees",
                        format!("a hand edit must carry {}/{}, got 78", key.0, key.1),
                    );
                }
                (Some(_), Some(_)) => {}
            }
            assert_eq!(world.bytes(), before_bytes, "a refused relock writes nothing");
            return;
        }
        Ok(resolves) => resolves,
    };

    if let Some(key) = &refused_fresh {
        let carried_through_a_moved_tag = world.moved && (proven || named_any(&before_pins, &onto, key));
        let carried = carried_by_hand.contains(key) || carried_by_tool.contains(key);
        if !carried && !typed && !carried_through_a_moved_tag {
            fail(
                "exit-agrees",
                format!("relock accepted {}/{} a fresh resolve refuses", key.0, key.1),
            );
        }
    }
    check_relocked(world, &before, row, failures);

    let mut fail = |clause: &str, detail: String| failures.push(format!("{clause}: {row}: {detail}"));
    let after_pins = pins(&world.lock());
    if proven {
        // A declared bundle with no row has no pin to carry, so it reads once.
        let unlocked: Vec<&str> = world
            .spec
            .no_snapshot
            .iter()
            .chain(world.spec.list_less)
            .copied()
            .filter(|name| !before.bundles.iter().any(|row| row.name == *name))
            .collect();
        let reads_the_unlocked = |id: &String| unlocked.iter().any(|name| id.ends_with(&format!("/{name}:1")));
        // A row an edit dropped beside such a bundle, or beside a row that records no
        // edge, has no pin to carry either.
        let afresh = |id: &String| {
            let repository = parse(id).registry_repository();
            left_unread.values().any(|pin| pin.registry_repository() == repository)
        };
        // A lock with no dependency data records no edge, so one its writer never
        // walked is a new request: the pin it does not answer resolves. A declared
        // key keeps its pin, since a direct declaration wins over every edge.
        let declared = |key: &&NodeKey| world.set.table(key.0).contains_key(&key.1);
        let released: BTreeSet<&NodeKey> = match dependency_free(&before) {
            true => asked
                .keys()
                .filter(|key| !declared(key) && gained_an_edge(key, &onto, &asked))
                .collect(),
            false => BTreeSet::new(),
        };
        let at_released = |id: &String| {
            let repository = parse(id).registry_repository();
            let named = |key: &&NodeKey| asked[*key].iter().any(|id| id.registry_repository() == repository);
            released.iter().any(named)
        };
        let carried: Vec<&String> = resolves
            .iter()
            .filter(|id| !reads_the_unlocked(id) && !afresh(id) && !at_released(id))
            .collect();
        if !carried.is_empty() {
            fail("remove-agrees", format!("resolved {carried:?} under a proof"));
        }
        // A pre-cache lock gains the row it lacks, and a 0.15.1 lock the edges its
        // rows lack, so only what it held compares.
        let gained_a_snapshot =
            world.spec.no_snapshot.is_some() || !world.spec.list_less.is_empty() || world.spec.no_requires;
        if gained_a_snapshot {
            // A tagless pin may gain the tag that resolved it, and nothing else.
            let stamped = |pin: &PinnedIdentifier, now: &PinnedIdentifier| {
                pin.tag().is_none()
                    && now.digest() == pin.digest()
                    && now.registry_repository() == pin.registry_repository()
            };
            for (key, pin) in before_pins.iter().filter(|(key, _)| !released.contains(key)) {
                if after_pins.get(key).is_none_or(|now| now != pin && !stamped(pin, now)) {
                    fail("remove-agrees", format!("{}/{}: {pin} changed", key.0, key.1));
                }
            }
        } else if rows(&before) != without_stamps(&after_pins, &before_pins, rows(&world.lock())) {
            fail(
                "remove-agrees",
                format!(
                    "the edit wrote\n{}\nthe relock wrote\n{}",
                    rows(&before),
                    rows(&world.lock())
                ),
            );
        }
    }
    if !world.proven(&world.lock()) {
        fail("fixed-point", "a successful relock left a stale hash".to_string());
    }

    let settled = world.bytes();
    match world.relock().await {
        Err(error) => fail("fixed-point", format!("second relock failed: {error}")),
        Ok(resolves) if !resolves.is_empty() => fail("fixed-point", format!("second relock resolved {resolves:?}")),
        Ok(_) if world.bytes() != settled => fail("fixed-point", "second relock rewrote the file".to_string()),
        Ok(_) => {}
    }
    check_proof(world, row, failures);
}

/// What a `grim lock` that wrote the lock on disk over `before` owes, whoever ran
/// it: no pin that answered a request it still meets moved, every pair is one
/// the store served, a digest an edge names is the pin, and a kept tagless pin
/// took the tag a request in its own repository gives.
fn check_relocked(world: &World, before: &GrimoireLock, row: &str, failures: &mut Vec<String>) {
    check_others_hold(world, before, proof_of(before, &[&world.set]), None, row, failures);
    check_recorded(world, row, failures);
    let mut fail = |clause: &str, detail: String| failures.push(format!("{clause}: {row}: {detail}"));
    let after = world.lock();
    let after_pins = pins(&after);
    let after_onto = world.walk_over(&after).requests;
    for pair in world.pair_false(&after) {
        fail("no-false-pair", format!("relock: {pair}"));
    }
    for (key, edges) in &after_onto {
        let direct = world.set.table(key.0).contains_key(&key.1);
        let digests = edges.iter().filter_map(Identifier::digest);
        if let Some(pin) = after_pins.get(key).filter(|_| !direct)
            && digests.into_iter().any(|digest| digest != pin.digest())
        {
            fail("held-digest", format!("{}/{}: {pin} under {edges:?}", key.0, key.1));
        }
    }
    // A kept tagless pin takes the tag of a request in its own repository, and a
    // pin no request answers resolves and gets one. A request naming a digest may
    // win and leave it tagless by design, and a bundle keeps its tag apart.
    for (key, pin) in after_pins
        .iter()
        .filter(|(key, pin)| key.0 != Bundle && pin.tag().is_none())
    {
        let declared = world.set.table(key.0).get(&key.1).and_then(DeclaredSource::identifier);
        let requests: Vec<&Identifier> = declared
            .into_iter()
            .chain(after_onto.get(key).into_iter().flatten())
            .collect();
        let here = |id: &&&Identifier| id.tag().is_some() && id.registry_repository() == pin.registry_repository();
        if let Some(id) = requests
            .iter()
            .find(here)
            .filter(|_| requests.iter().all(|id| id.digest().is_none()))
        {
            fail(
                "stamped",
                format!("{}/{}: the relock left {pin} under {id}", key.0, key.1),
            );
        }
    }
}

/// The recorded edges onto `key` a resolve still made. A row nothing reached left
/// with its edges, and no request of its survives to be answered.
fn surviving_edges(key: &NodeKey, before: &Requests, after: &Requests) -> Option<BTreeSet<Identifier>> {
    let edges: BTreeSet<Identifier> = before.get(key)?.intersection(after.get(key)?).cloned().collect();
    (!edges.is_empty()).then_some(edges)
}

/// Whether `key` holds an edge the old lock did not record. A declaration that
/// came back brought it, and such an edge releases the pin: no pin is carried
/// past a request it never answered, and the command reports the move.
fn gained_an_edge(key: &NodeKey, before: &Requests, after: &Requests) -> bool {
    let recorded = before.get(key);
    after
        .get(key)
        .is_some_and(|edges| edges.iter().any(|id| recorded.is_none_or(|old| !old.contains(id))))
}

/// What a hash that matches states about the lock on disk.
fn check_proof(world: &World, row: &str, failures: &mut Vec<String>) {
    let lock = world.lock();
    if !world.proven(&lock) {
        return;
    }
    let held = pins(&lock);
    let mut fail = |detail: String| failures.push(format!("proof: {row}: {detail}"));
    for (key, source) in world.roots() {
        let (kind, name) = (key.0, &key.1);
        match (source, held.get(&key)) {
            _ if world.spec.lacks_list(name) => {}
            (DeclaredSource::Path(path), _) => {
                if !lock.bundles.iter().any(|b| b.name == *name && b.path() == Some(&path)) {
                    fail(format!("{kind}/{name} declared {path}, no row at it"));
                }
            }
            (DeclaredSource::Registry(_), None) => fail(format!("{kind}/{name} declared, no row")),
            (DeclaredSource::Registry(id), Some(pin)) => {
                let elsewhere = id.digest().is_some_and(|digest| digest != pin.digest());
                // A bundle's tag is its row's, beside a tagless pin.
                let retagged = match lock.bundles.iter().find(|b| kind == Bundle && b.name == *name) {
                    Some(row) => {
                        row.repo() != Some(id.registry_repository().as_str())
                            || id.tag().is_some_and(|tag| row.tag() != Some(tag))
                    }
                    None => pin.tag().is_some() && id.digest().is_none() && !names(pin, &id),
                };
                if elsewhere || retagged {
                    fail(format!("{kind}/{name} declared {id}, pin {pin}"));
                }
            }
        }
    }
    // Read from the rows: a lost row leaves nothing below it to walk.
    let recorded = requests(&lock);
    for key in recorded.keys() {
        let direct = world.set.table(key.0).contains_key(&key.1);
        if !direct && !held.contains_key(key) {
            fail(format!("{}/{} has an edge, no row", key.0, key.1));
        }
    }
    // A bundle row the declaration does not name was reached through an edge.
    for bundle in &lock.bundles {
        let declared = world.set.bundles.contains_key(&bundle.name);
        if bundle.transitive == declared {
            fail(format!(
                "bundle/{} declared {declared}, transitive {}",
                bundle.name, bundle.transitive
            ));
        }
    }
    for (key, edges) in recorded {
        let direct = world.set.table(key.0).contains_key(&key.1);
        if let Some(pin) = held.get(&key).filter(|_| !direct)
            && edges
                .iter()
                .filter_map(Identifier::digest)
                .any(|digest| digest != pin.digest())
        {
            fail(format!("{}/{}: {pin} under {edges:?}", key.0, key.1));
        }
    }
    check_install(world, &held, row, failures);
}

/// What `grim install` does with a lock whose hash matches: it materializes the
/// rows and nothing else, so every key the declaration reaches needs a row, and a
/// row it does not reach would install. The reach is walked through the store's
/// edges at each row's digest, never through what the lock recorded.
fn check_install(world: &World, held: &BTreeMap<NodeKey, PinnedIdentifier>, row: &str, failures: &mut Vec<String>) {
    let roots = world.roots();
    let serving = |id: &Identifier| world.store.serving(&id.to_string()).into_iter().collect();
    let reached = world.walk(&roots, held, serving).below;
    // A path bundle's row has no pin, and a pre-cache lock holds no row for a
    // declared bundle: `proof` checks the first, and the second has none to check.
    let unpinned = |key: &NodeKey| {
        let path = matches!(roots.get(key), Some(DeclaredSource::Path(_)));
        path || (key.0 == Bundle && world.spec.lacks_list(&key.1))
    };
    // A row an edit dropped beside a bundle nothing offline reads, or beside a row that
    // records no edge, is 0.15.1's loss, which the next resolve repairs.
    let unread = |key: &NodeKey| world.left_unread.contains_key(key);
    for key in reached
        .keys()
        .filter(|key| !held.contains_key(*key) && !unpinned(key) && !unread(key))
    {
        failures.push(format!("install: {row}: {}/{} reached, no row", key.0, key.1));
    }
    for key in held.keys().filter(|key| !reached.contains_key(*key)) {
        failures.push(format!("install: {row}: {}/{} has a row nothing reaches", key.0, key.1));
    }
}

/// Pins of keys other than `exempt` that answered a request before a resolve the
/// user did not aim at them still hold after it. `proof` is the declaration the
/// lock's hash proved for that resolve.
fn check_others_hold(
    world: &World,
    before: &GrimoireLock,
    proof: Option<&DesiredSet>,
    exempt: Option<&NodeKey>,
    row: &str,
    failures: &mut Vec<String>,
) {
    let after = world.lock();
    let after_pins = pins(&after);
    // A row the declaration no longer reaches, as a line deleted by hand leaves, is a
    // ghost: no relock claims its edge, so counted here it holds a pin the writer frees.
    let reached = world.walk_over(before).below;
    let owed = Owed {
        set: &world.set,
        proof,
        recorded: requests_from(before, |key| reached.contains_key(key)),
        asked: world.walk_over(&after).requests,
        listed: world.listed_requests(before, &after_pins, proof),
    };
    for (key, pin) in &pins(before) {
        let Some(now) = after_pins.get(key).filter(|_| Some(key) != exempt) else {
            continue;
        };
        if owed.holds(key, pin) && now.digest() != pin.digest() {
            failures.push(format!("no-move: {row}: {}/{}: {pin} became {now}", key.0, key.1));
        }
    }
}

/// Every row records what the store publishes at its pin, and a path bundle what its
/// file lists: the clauses read these rows as the lock's own word on its edges.
fn check_recorded(world: &World, row: &str, failures: &mut Vec<String>) {
    let lock = world.lock();
    let artifacts = lock.iter_artifacts().filter_map(|entry| {
        let pin = entry.source.pinned()?;
        Some((
            (entry.kind, &entry.name),
            &entry.requires,
            world.edges_of(&pin.digest()),
        ))
    });
    let bundles = lock.bundles.iter().map(|entry| {
        let edges = match (entry.pinned(), entry.path()) {
            (Some(pin), _) => world.edges_of(&pin.digest()),
            (None, Some(path)) => world.path_members(path),
            (None, None) => Vec::new(),
        };
        ((Bundle, &entry.name), &entry.members, edges)
    });
    for ((kind, name), written, published) in artifacts.chain(bundles) {
        // grim 0.15.1 records no edge; the next relock reads them.
        if world.spec.no_requires && kind != Bundle && written.is_empty() {
            continue;
        }
        let set = |edges: &[BundleMember]| -> BTreeSet<(ArtifactKind, String, String)> {
            let edge = |edge: &BundleMember| (edge.kind, edge.name.clone(), edge.id.clone());
            edges.iter().map(edge).collect()
        };
        if set(written) != set(&published) {
            failures.push(format!(
                "recorded-edges: {row}: {kind}/{name} records {written:?}, the store {published:?}"
            ));
        }
    }
}

/// A refused `grim add` or `grim update` names the conflict it met: the store
/// keeps every manifest, so no other refusal has a cause here.
fn check_refusal(error: &ResolveError, row: &str, failures: &mut Vec<String>) {
    if conflict_key(error).is_none() {
        failures.push(format!("refusal: {row}: {error}"));
    }
}

/// `grim update <name>` of a key declared at a tag writes what that tag serves
/// now: it is the command that moves a pin whose request held.
fn check_update_moves(world: &World, key: &NodeKey, row: &str, failures: &mut Vec<String>) {
    let declared = world.set.table(key.0).get(&key.1).and_then(DeclaredSource::identifier);
    let Some(id) = declared.filter(|id| id.digest().is_none()) else {
        return;
    };
    let serving = world.store.serving(&id.to_string());
    let now = pins(&world.lock()).get(key).map(PinnedIdentifier::digest);
    if now != serving {
        failures.push(format!(
            "update-moves: {row}: {}/{} at {now:?}, {id} serves {serving:?}",
            key.0, key.1
        ));
    }
}

/// A bare `grim update` re-resolves every request, so whatever the lock held it
/// writes and refuses what a fresh resolve of the same declaration does.
fn check_update_all(
    world: &World,
    outcome: Result<(), ResolveError>,
    fresh: Result<GrimoireLock, ResolveError>,
    row: &str,
    failures: &mut Vec<String>,
) {
    match (outcome, fresh) {
        (Ok(()), Ok(fresh)) => {
            if rows(&world.lock()) != rows(&fresh) {
                let (wrote, fresh) = (rows(&world.lock()), rows(&fresh));
                failures.push(format!(
                    "update-agrees: {row}: the update wrote\n{wrote}\na fresh resolve\n{fresh}"
                ));
            }
            check_proof(world, row, failures);
        }
        (Err(error), Err(_)) => check_refusal(&error, row, failures),
        (Ok(()), Err(fresh)) => failures.push(format!("update-agrees: {row}: a fresh resolve refuses: {fresh}")),
        (Err(error), Ok(_)) => failures.push(format!("update-agrees: {row}: a fresh resolve succeeds: {error}")),
    }
}

fn named_any(pins: &BTreeMap<NodeKey, PinnedIdentifier>, onto: &Requests, key: &NodeKey) -> bool {
    match (pins.get(key), onto.get(key)) {
        (Some(pin), Some(edges)) => edges.iter().any(|id| names(pin, id)),
        _ => false,
    }
}

/// A pin that names an edge, with no digest edge holding another digest.
fn carries_by_hand(pins: &BTreeMap<NodeKey, PinnedIdentifier>, onto: &Requests, key: &NodeKey) -> bool {
    let at_another_digest = |pin: &PinnedIdentifier, id: &Identifier| id.digest().is_some_and(|d| d != pin.digest());
    named_any(pins, onto, key)
        && match (pins.get(key), onto.get(key)) {
            (Some(pin), Some(edges)) => !edges.iter().any(|id| at_another_digest(pin, id)),
            _ => false,
        }
}

fn conflict_key(error: &ResolveError) -> Option<NodeKey> {
    match &error.kind {
        ResolveErrorKind::BundleConflict { kind, name, .. } => Some((*kind, name.clone())),
        _ => None,
    }
}

/// What one lock edit owes. A row's pin that answers a request the edited
/// declaration still makes stays, and a row it keeps keeps its pin but for one
/// stamp; a key whose declaration left is settled by
/// what its pin's own record names among those requests (the design record's
/// consequence 3); a bundle only an edge holds is marked so; a stale lock is
/// restamped only where every row states what it answered, which a tagless pin
/// cannot; an edit that changed no row keeps the hash; and an edit that ends a
/// proof leaves no tagless pin it could tag.
struct EditSeen<'a> {
    edit: Edit,
    key: &'a NodeKey,
    previous: &'a GrimoireLock,
    set_before: &'a DesiredSet,
    ended_a_proof: bool,
    /// [`dropped_unread`] of this edit, and [`left_beside_unwalked`].
    unread: &'a BTreeMap<NodeKey, bool>,
    /// [`unchecked_drop`] of this edit.
    owed_drop: bool,
}

/// Whether `lock` is one grim 0.15.1 writes and reads alike: no row records an edge,
/// and no bundle row was reached through one or lists another bundle.
fn dependency_free(lock: &GrimoireLock) -> bool {
    let plain = |row: &LockedBundle| !row.transitive && row.members.iter().all(|member| member.kind != Bundle);
    lock.iter_artifacts().all(|row| row.requires.is_empty()) && lock.bundles.iter().all(plain)
}

/// The bundles `set` declares that `lock` holds no `[[bundle]]` row for, with the
/// registry line of each.
fn list_less<'s>(set: &'s DesiredSet, lock: &GrimoireLock) -> Vec<(&'s str, Option<&'s Identifier>)> {
    set.bundles
        .iter()
        .filter(|(name, _)| !lock.bundles.iter().any(|row| row.name == **name))
        .map(|(name, source)| (name.as_str(), source.identifier()))
        .collect()
}

/// Whether `row` carries the stamp the bundle declared at `id` leaves on what it
/// provides.
fn carries_stamp(row: &LockedArtifact, id: &Identifier) -> bool {
    stamp_of(id).is_some_and(|(repo, tag)| {
        row.bundles
            .iter()
            .any(|stamp| stamp.repo == repo && split_stamp(&stamp.tag).0 == tag)
    })
}

/// The rows the edit just made may drop by the design record's residual: on a lock
/// with no dependency data whose hash proves the declaration before or after the edit,
/// beside a declared bundle the lock holds no list for and the store cannot speak for,
/// a row neither the store nor its own stamp shows such a bundle provides drops as on
/// 0.15.1. Only a row the edited declaration no longer reaches but through such a
/// bundle qualifies, so a key it still declares never does. Each maps to whether a
/// note names it and such a bundle, which the drop owes.
fn dropped_unread(world: &World, previous: &GrimoireLock, set_before: &DesiredSet) -> BTreeMap<NodeKey, bool> {
    let mut out = BTreeMap::new();
    if proof_of(previous, &[set_before, &world.set]).is_none() || !dependency_free(previous) {
        return out;
    }
    let bundles = list_less(&world.set, previous);
    let stored = |id: Option<&Identifier>| id.and_then(|id| world.stored_closure(id, previous));
    let unknown: Vec<&str> = bundles
        .iter()
        .filter(|(_, id)| stored(*id).is_none())
        .map(|(name, _)| *name)
        .collect();
    let kept = pins(&world.lock());
    let mut roots = world.roots();
    roots.retain(|(kind, name), _| *kind != Bundle || !unknown.contains(&name.as_str()));
    let serving = |id: &Identifier| world.store.serving(&id.to_string()).into_iter().collect();
    let reached = world.walk(&roots, &pins(previous), serving).below;
    for entry in previous.iter_artifacts().filter(|_| !unknown.is_empty()) {
        let key = (entry.kind, entry.name.clone());
        let tied = |(_, id): &(&str, Option<&Identifier>)| {
            id.is_some_and(|id| carries_stamp(entry, id)) || stored(*id).is_some_and(|keys| keys.contains(&key))
        };
        let skip = reached.contains_key(&key) || kept.contains_key(&key) || entry.source.pinned().is_none();
        if skip || bundles.iter().any(tied) {
            continue;
        }
        let names = |note: &&String| note.contains(&format!("{} '{}'", key.0, key.1)) || note.contains(" more ");
        let beside = |note: &&String| {
            unknown.iter().any(|bundle| {
                note.contains(&format!(
                    "bundle '{bundle}' has no member list in this lock, and the local store cannot show"
                ))
            })
        };
        let noted = world.notes.iter().filter(names).any(|note| beside(&note));
        out.insert(key, noted);
    }
    out
}

/// The rows the edit just dropped that the edited declaration reaches over
/// `previous`'s pins only through rows recording no edge: read as leaves, as the
/// edit and 0.15.1 read them, they reach none of these. The design
/// record's residual, which the next relock resolves afresh.
fn left_beside_unwalked(
    world: &World,
    previous: &GrimoireLock,
    lock: &GrimoireLock,
) -> BTreeMap<NodeKey, PinnedIdentifier> {
    let (roots, held) = (world.roots(), pins(previous));
    let serving = |id: &Identifier| world.store.serving(&id.to_string()).into_iter().collect();
    let reached = world.walk(&roots, &held, serving).below;
    let unwalked = |key: &NodeKey| key.0 != Bundle && unread_asker(previous, key);
    let serving = |id: &Identifier| world.store.serving(&id.to_string()).into_iter().collect();
    let as_leaves = world
        .walk_leaving(&roots, &held, serving, |key, _| key.0 == Bundle, unwalked)
        .below;
    let left = pins(lock);
    held.into_iter()
        .filter(|(key, _)| !left.contains_key(key) && reached.contains_key(key) && !as_leaves.contains_key(key))
        .collect()
}

/// Whether `key`'s row in `lock` records no edge: an edit reads it as requiring
/// nothing, as 0.15.1 does, whatever the store holds.
fn unread_asker(lock: &GrimoireLock, key: &NodeKey) -> bool {
    lock.iter_artifacts()
        .any(|row| (row.kind, &row.name) == (key.0, &key.1) && row.requires.is_empty())
}

/// Whether the edit owes the drop of `target`: [`left_for_unread_askers`] on a lock
/// whose hash proved the declaration.
fn unchecked_drop(
    world: &World,
    edit: Edit,
    target: &NodeKey,
    previous: &GrimoireLock,
    set_before: &DesiredSet,
) -> bool {
    proof_of(previous, &[set_before]).is_some() && left_for_unread_askers(world, edit, target, previous, set_before)
}

/// Whether `target`'s line left, no bundle may provide it, every line has a row, and
/// each row asking for it records no edge: 0.15.1 drops such a row.
fn left_for_unread_askers(
    world: &World,
    edit: Edit,
    target: &NodeKey,
    previous: &GrimoireLock,
    set_before: &DesiredSet,
) -> bool {
    let held = pins(previous);
    let declared = |set: &DesiredSet| set.table(target.0).contains_key(&target.1);
    let left = declared(set_before) && !declared(&world.set);
    if !edit.writes_the_lock() || !left || target.0 == Bundle || !held.contains_key(target) {
        return false;
    }
    let lists = |row: &LockedBundle| row.members.iter().any(|m| (m.kind, &m.name) == (target.0, &target.1));
    let quiet = list_less(&world.set, previous).is_empty()
        && !previous.bundles.iter().any(lists)
        && world.roots().keys().all(|key| held.contains_key(key));
    if !quiet {
        return false;
    }
    only_unread_askers(world, previous, target)
}

/// Whether `target` has an asker in `previous`, read through the manifest each row is
/// pinned at, and every asker records no edge.
fn only_unread_askers(world: &World, previous: &GrimoireLock, target: &NodeKey) -> bool {
    let reached = world.walk_over(previous).below;
    let asks = |pin: &PinnedIdentifier| {
        let edges = world.edges_of(&pin.digest());
        edges
            .iter()
            .any(|edge| (edge.kind, &edge.name) == (target.0, &target.1))
    };
    let askers: Vec<NodeKey> = pins(previous)
        .into_iter()
        .filter(|(key, pin)| key != target && reached.contains_key(key) && asks(pin))
        .map(|(key, _)| key)
        .collect();
    !askers.is_empty() && askers.iter().all(|key| unread_asker(previous, key))
}

async fn check_edit(world: &World, seen: &EditSeen<'_>, row: &str, failures: &mut Vec<String>) {
    let EditSeen {
        edit,
        key: target,
        previous,
        set_before,
        ended_a_proof,
        unread,
        owed_drop,
    } = *seen;
    let lock = world.lock();
    for pair in world.pair_false(&lock) {
        failures.push(format!("no-false-pair: {row}: edit: {pair}"));
    }
    check_proof(world, row, failures);
    check_recorded(world, row, failures);
    let mut fail = |clause: &str, detail: String| failures.push(format!("{clause}: {row}: {detail}"));
    if ended_a_proof && world.spec.pre_change && !world.proven(&lock) {
        for (key, pin) in pins(&lock).iter().filter(|(_, pin)| pin.tag().is_none()) {
            fail("stamped", format!("{}/{} left at {pin}", key.0, key.1));
        }
    }
    let proof = proof_of(previous, &[set_before]);
    let recorded = requests(previous);
    let asked = world.walk_over(previous).requests;
    let kept = pins(&lock);
    let answered_by = |key: &NodeKey, pin: &PinnedIdentifier, id: &Identifier| {
        let on_record = recorded.get(key).is_some_and(|edges| edges.contains(id));
        let made = if on_record { Made::Recorded } else { Made::Unrecorded };
        answers(pin, key, proof, made, id)
    };
    // An edit's one tag write: a tagless pin takes the tag of a request in its own
    // repository when the edit ends a proof. Any other change to a kept pin passes
    // every later clause, since the next relock reads this lock as the one to keep.
    let stamps = ended_a_proof && !world.proven(&lock);
    let stamped = |key: &NodeKey, pin: &PinnedIdentifier, now: &PinnedIdentifier| {
        let declared = set_before.table(key.0).get(&key.1).and_then(DeclaredSource::identifier);
        let gives_the_tag = |id: &Identifier| {
            id.tag().is_some() && id.tag() == now.tag() && id.registry_repository() == now.registry_repository()
        };
        stamps
            && pin.tag().is_none()
            && now.strip_advisory() == *pin
            && declared
                .into_iter()
                .chain(recorded.get(key).into_iter().flatten())
                .any(gives_the_tag)
    };
    for (key, pin) in pins(previous) {
        let answered = asked
            .get(&key)
            .is_some_and(|edges| edges.iter().any(|id| answered_by(&key, &pin, id)));
        // A line the edit left in place is a request too: dropped, its row resolves
        // afresh at the next relock and the pin can move.
        let declared = world.set.table(key.0).get(&key.1).and_then(DeclaredSource::identifier);
        let answers_its_line = declared.is_some_and(|id| answers(&pin, &key, proof, Made::Unrecorded, id));
        // A residual drop is 0.15.1's only with the note that names its cost.
        let unnoted = match unread.get(&key) {
            Some(true) => None,
            Some(false) => Some(", and no note names it beside a bundle with no member list"),
            None => Some(""),
        };
        match kept.get(&key) {
            None if answered || answers_its_line => {
                if let Some(unnoted) = unnoted {
                    fail(
                        "no-move",
                        format!(
                            "the edit dropped {}/{} though {pin} answers a request{unnoted}",
                            key.0, key.1
                        ),
                    );
                }
            }
            Some(now) if *now != pin && !stamped(&key, &pin, now) => fail(
                "no-move",
                format!("the edit kept {}/{} at {now}, its pin was {pin}", key.0, key.1),
            ),
            _ => {}
        }
    }
    // A kept `[[bundle]]` row keeps the repository and tag beside its pin, and a path
    // bundle its hash: the next relock reads the members at that record.
    let record = |row: &LockedBundle| {
        (
            row.repo().map(str::to_string),
            row.tag().map(str::to_string),
            row.content_digest(),
        )
    };
    for row in &previous.bundles {
        if let Some(now) = lock.bundles.iter().find(|now| now.name == row.name)
            && record(now) != record(row)
        {
            fail(
                "no-move",
                format!(
                    "the edit kept bundle/{} at {:?}, it was {:?}",
                    row.name,
                    record(now),
                    record(row)
                ),
            );
        }
    }
    if !edit.writes_the_lock() {
        return;
    }
    if owed_drop && kept.contains_key(target) {
        fail(
            "unchecked-parent",
            format!(
                "{}/{} kept, though only rows that record no edge ask for it",
                target.0, target.1
            ),
        );
    }
    check_stamped_members(world, previous, set_before, &lock, &mut fail);
    check_unwalked_note(world, previous, &lock, &mut fail);
    let landed = world.set.declaration_hash_cached() == previous.metadata.declaration_hash;
    let requests = asked.get(target).cloned().unwrap_or_default();
    let left = set_before
        .table(target.0)
        .get(&target.1)
        .and_then(DeclaredSource::identifier)
        .is_some()
        && !world.set.table(target.0).contains_key(&target.1);
    // A key whose declaration left is judged by what its pin answers of the requests
    // the edited declaration makes, read from the store, and by the next relock: a row
    // kept on a stale lock that a note names, or an edge the lock records, is that
    // relock's to judge, and a row dropped costs no pin it would keep.
    if let Some(pin) = pins(previous).get(target).filter(|_| left && !landed) {
        let named = requests.iter().filter(|id| answered_by(target, pin, id)).count();
        let (stays, stale) = match named {
            0 if requests.is_empty() => (false, false),
            0 => (false, true),
            n if n == requests.len() => (true, false),
            _ => (true, true),
        };
        let (kind, name) = (target.0, &target.1);
        let kept_it = kept.contains_key(target);
        // Offline the edit reads the lock's edges, not the store's: one the store no
        // longer makes still names the row.
        let recorded_onto = recorded.get(target).is_some_and(|edges| !edges.is_subset(&requests));
        let says = |note: &String| note.starts_with(&format!("{kind} '{name}' ")) || note.contains(" more ");
        let unjudged = world.notes.iter().any(says) || recorded_onto;
        if kept_it && !stays && (world.proven(&lock) || !unjudged) {
            fail(
                "declaration-left",
                format!(
                    "{kind}/{name} kept, though its pin {pin} answers none of {requests:?}, the lock proven or \
                     no note or recorded edge naming it"
                ),
            );
        }
        let residual = unread.get(target) == Some(&true);
        if !kept_it && !residual && world.relock_keeps(previous, target, pin, &requests).await {
            fail(
                "declaration-left",
                format!("{kind}/{name} dropped, though `grim lock` keeps its pin {pin} for {requests:?}"),
            );
        }
        // The edit drops a bundle that only rows recording no edge ask for, as it reads
        // them: the residual, which the next relock resolves afresh.
        let residual_bundle = target.0 == Bundle && only_unread_askers(world, previous, target);
        if stale && world.proven(&lock) && !residual_bundle {
            fail(
                "declaration-left",
                format!("{kind}/{name}: {pin} names {named} of {requests:?}, and the lock is proven"),
            );
        }
    }
    if target.0 == Bundle && !world.set.bundles.contains_key(&target.1) {
        let edge_only = lock.bundles.iter().find(|row| row.name == target.1);
        let held_as_it_was = |row: &LockedBundle| dependency_free(previous) && previous.bundles.contains(row);
        if edge_only.is_some_and(|row| !row.transitive && !held_as_it_was(row)) {
            fail("transitive", format!("bundle/{} stays for an edge alone", target.1));
        }
    }
    let restamped =
        proof.is_none() && world.proven(&lock) && lock.metadata.declaration_hash != previous.metadata.declaration_hash;
    if restamped {
        let onto = world.walk_over(&lock).requests;
        for entry in lock.iter_artifacts() {
            let Some(pin) = entry.source.pinned() else {
                continue;
            };
            let key = (entry.kind, entry.name.clone());
            let named = |id: &Identifier| pin.tag().is_some() && names(pin, id);
            let stated = match world.set.table(key.0).get(&key.1) {
                Some(source) => source.identifier().is_some_and(named),
                None => onto.get(&key).is_some_and(|edges| edges.iter().any(named)),
            };
            if !stated {
                fail(
                    "restamp",
                    format!("{}/{}: the stale lock restamped over {pin}", key.0, key.1),
                );
            }
        }
    }
    if rows_alone(&lock) == rows_alone(previous)
        && lock.metadata.declaration_hash != previous.metadata.declaration_hash
        && !world.proven(&lock)
    {
        fail(
            "unchanged-rows",
            "an edit that changed no row moved the hash".to_string(),
        );
    }
}

/// On a lock 0.15.1 reads whose hash proved the declaration, a row the edit keeps
/// that no line declares and no surviving bundle's stamp or list names, beside a
/// declared bundle the lock holds no list for that the store shows provides it,
/// takes that bundle's stamp: a relock of the edited declaration writes it, so the
/// lock still proves that declaration and `grim install` keeps 0.15.1's exit 0.
fn check_stamped_members(
    world: &World,
    previous: &GrimoireLock,
    set_before: &DesiredSet,
    lock: &GrimoireLock,
    fail: &mut impl FnMut(&str, String),
) {
    if !dependency_free(previous) || proof_of(previous, &[set_before]).is_none() {
        return;
    }
    let declared: Vec<&Identifier> = world
        .set
        .bundles
        .values()
        .filter_map(DeclaredSource::identifier)
        .collect();
    let bundles = list_less(&world.set, previous);
    for entry in lock.iter_artifacts() {
        let key = (entry.kind, entry.name.clone());
        let was = previous
            .iter_artifacts()
            .find(|row| (row.kind, &row.name) == (key.0, &key.1));
        let listed = previous
            .bundles
            .iter()
            .any(|row| row.members.iter().any(|m| (m.kind, &m.name) == (key.0, &key.1)));
        let held = was.is_some_and(|was| declared.iter().any(|id| carries_stamp(was, id)));
        if was.is_none() || listed || held || world.set.table(key.0).contains_key(&key.1) {
            continue;
        }
        let shown: Vec<&Identifier> = bundles
            .iter()
            .filter_map(|(_, id)| *id)
            .filter(|id| {
                world
                    .stored_closure(id, previous)
                    .is_some_and(|keys| keys.contains(&key))
            })
            .collect();
        if !shown.is_empty() && !shown.iter().any(|id| carries_stamp(entry, id)) {
            fail(
                "stamped-member",
                format!(
                    "{}/{} kept with the stamps {:?}, beside {shown:?}, which the store shows provide it",
                    key.0, key.1, entry.bundles
                ),
            );
        }
    }
}

/// **unwalked-note**: an edit that drops rows beside a surviving row that records no
/// edge while the store holds its manifest naming requires names each row those
/// requires may reach through the store, that row and the online `grim lock` that
/// records its edges; beside no such row it names none.
fn check_unwalked_note(
    world: &World,
    previous: &GrimoireLock,
    lock: &GrimoireLock,
    fail: &mut impl FnMut(&str, String),
) {
    let left: BTreeSet<NodeKey> = lock.node_keys().collect();
    let mut dropped: Vec<NodeKey> = previous.node_keys().filter(|key| !left.contains(key)).collect();
    let mut asked = BTreeSet::new();
    let askers: Vec<NodeKey> = lock
        .iter_artifacts()
        .map(|row| (row.kind, row.name.clone()))
        .filter(|key| world.names_requires(lock, key))
        .filter(|key| {
            let (reached, whole) = world.stored_reach(previous, lock, key);
            let mine: Vec<&NodeKey> = dropped.iter().filter(|left| !whole || reached.contains(left)).collect();
            asked.extend(mine.iter().copied().cloned());
            !mine.is_empty()
        })
        .collect();
    dropped.retain(|key| asked.contains(key));
    let noted: Vec<&String> = world
        .notes
        .iter()
        .filter(|note| note.contains("no dependency edges"))
        .collect();
    if dropped.is_empty() || askers.is_empty() {
        if !noted.is_empty() {
            fail(
                "unwalked-note",
                format!("noted {noted:?}, dropped {dropped:?} beside {askers:?}"),
            );
        }
        return;
    }
    let names = |key: &NodeKey| {
        noted.iter().any(|note| {
            (note.contains(&format!("{} '{}'", key.0, key.1)) || note.contains(" more "))
                && note.contains("run `grim lock` online once before such an edit")
        })
    };
    for key in dropped.iter().chain(&askers).filter(|key| !names(key)) {
        fail(
            "unwalked-note",
            format!(
                "{}/{} unnamed: dropped {dropped:?} beside {askers:?}, noted {noted:?}",
                key.0, key.1
            ),
        );
    }
}

async fn edit_and_check(world: &mut World, edit: Edit, key: &NodeKey, row: &str, failures: &mut Vec<String>) {
    let previous = world.lock();
    let set_before = world.set.clone();
    let ended_a_proof = edit.writes_the_lock() && world.proven(&previous);
    world.apply(edit, key);
    let mut unread = dropped_unread(world, &previous, &set_before);
    let owed_drop = unchecked_drop(world, edit, key, &previous, &set_before);
    if edit.writes_the_lock() {
        for (key, _) in left_beside_unwalked(world, &previous, &world.lock()) {
            unread.insert(key, true);
        }
    }
    for (key, _) in unread.iter().filter(|(_, noted)| **noted) {
        world.left_unread.insert(key.clone(), pins(&previous)[key].clone());
    }
    let seen = EditSeen {
        edit,
        key,
        previous: &previous,
        set_before: &set_before,
        ended_a_proof,
        unread: &unread,
        owed_drop,
    };
    check_edit(world, &seen, row, failures).await;
}

fn edits_of(spec: &Spec) -> Vec<Vec<(Edit, NodeKey)>> {
    let targets: Vec<NodeKey> = spec
        .declare
        .iter()
        .map(|(kind, name, _)| key_of(*kind, name))
        .filter(|key| spec.no_snapshot != Some(key.1.as_str()))
        .collect();
    let mut rows: Vec<Vec<(Edit, NodeKey)>> = vec![Vec::new()];
    for target in &targets {
        for edit in [Edit::Remove, Edit::Undeclare, Edit::Hand] {
            rows.push(vec![(edit, target.clone())]);
        }
        for second in targets.iter().filter(|second| *second != target) {
            rows.push(vec![(Edit::Remove, target.clone()), (Edit::Remove, second.clone())]);
            // A hand edit hides that the key was direct from the edit after it.
            rows.push(vec![(Edit::Hand, target.clone()), (Edit::Remove, second.clone())]);
            if spec.repoint.is_some_and(|(name, _)| name == target.1) {
                rows.push(vec![(Edit::Repoint, target.clone()), (Edit::Remove, second.clone())]);
            }
        }
    }
    rows
}

fn row_name(spec: &Spec, edits: &[(Edit, NodeKey)], mv: Move) -> String {
    let edits: Vec<String> = edits.iter().map(|(e, k)| format!("{e:?}({})", k.1)).collect();
    format!("{}/[{}]/{mv:?}", spec.name, edits.join(","))
}

fn report(failures: &[String], scenarios: usize) {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for failure in failures {
        *counts.entry(failure.split(':').next().unwrap_or("")).or_default() += 1;
    }
    for failure in failures {
        eprintln!("{failure}");
    }
    assert!(
        failures.is_empty(),
        "{} failures over {scenarios} scenarios: {counts:?}",
        failures.len()
    );
}

#[tokio::test]
async fn every_writer_holds_the_pin_rule() {
    let mut failures = Vec::new();
    let mut scenarios = 0;
    let shapes = SPECS.iter().chain(LATER_SPECS).chain(TABLE_SPECS);
    for spec in shapes.chain(STORE_SPECS).chain(UNWALKED_SPECS) {
        for edits in edits_of(spec) {
            for mv in MOVES {
                let name = row_name(spec, &edits, mv);
                let mut world = World::build(spec).await;
                for (edit, key) in &edits {
                    edit_and_check(&mut world, *edit, key, &name, &mut failures).await;
                }
                world.move_tags(mv).await;
                check_relock(&mut world, &name, &mut failures).await;
                scenarios += 1;
            }
        }
    }
    report(&failures, scenarios);
}

/// `x`'s tag moved to bytes with the same edges, and `b` leaves: the edit keeps `x`
/// at the digest it pinned. The table moves tags only after its edits, and a relock
/// after the edit compares against the lock the edit wrote.
#[tokio::test]
async fn an_edit_keeps_a_digest_whose_tag_moved() {
    let spec = SPECS.iter().find(|s| s.name == "flat").unwrap();
    let b = key_of(Skill, "b");
    let mut failures = Vec::new();
    for edit in [Edit::Remove, Edit::Undeclare] {
        let mut world = World::build(spec).await;
        world.move_tags(Move::PinTag).await;
        let row = format!("flat/{edit:?}(b)");
        edit_and_check(&mut world, edit, &b, &row, &mut failures).await;
        check_relock(&mut world, &row, &mut failures).await;
    }
    report(&failures, 2);
}

/// What `grim --offline lock` reads with an empty `GRIM_HOME`: no cached tag, no
/// stored manifest or layer.
fn empty_home(store: &Store) -> (tempfile::TempDir, Arc<dyn OciAccess>) {
    let dir = scratch_dir();
    let paths = crate::store::GrimPaths::new(dir.path().join("home"));
    paths.ensure_layout().unwrap();
    let access = crate::oci::access::cached_access::CachedAccess::new(
        store.clone(),
        crate::oci::tag_cache::TagCache::new(paths.tags_dir()),
        crate::store::BlobStore::new(paths.blobs_dir()),
        crate::oci::access::AccessMode::Offline,
    );
    (dir, Arc::new(access))
}

/// What `grim --offline lock` reads with a `GRIM_HOME` holding every manifest the
/// registry served and no layer. Only a store that serves manifest JSON fills it.
fn manifests_home(store: &Store) -> (tempfile::TempDir, Arc<dyn OciAccess>) {
    let dir = scratch_dir();
    let paths = crate::store::GrimPaths::new(dir.path().join("home"));
    paths.ensure_layout().unwrap();
    let blobs = crate::store::BlobStore::new(paths.blobs_dir());
    for (digest, (bytes, _)) in store.manifests.lock().unwrap().iter() {
        blobs.put(digest, bytes).unwrap();
    }
    let access = crate::oci::access::cached_access::CachedAccess::new(
        store.clone(),
        crate::oci::tag_cache::TagCache::new(paths.tags_dir()),
        blobs,
        crate::oci::access::AccessMode::Offline,
    );
    (dir, Arc::new(access))
}

/// The keys `roots` reach through the edges `lock` records: all an offline relock reads
/// of a requester whose requires layer the store lacks or whose manifest it never read.
fn recorded_reach(lock: &GrimoireLock, roots: impl IntoIterator<Item = NodeKey>) -> BTreeSet<NodeKey> {
    let key = |edge: &BundleMember| (edge.kind, edge.name.clone());
    let mut edges: BTreeMap<NodeKey, Vec<NodeKey>> = BTreeMap::new();
    for row in lock.iter_artifacts() {
        edges.insert((row.kind, row.name.clone()), row.requires.iter().map(key).collect());
    }
    for row in &lock.bundles {
        edges.insert((Bundle, row.name.clone()), row.members.iter().map(key).collect());
    }
    let (mut reach, mut queue) = (BTreeSet::new(), roots.into_iter().collect::<Vec<_>>());
    while let Some(next) = queue.pop() {
        if reach.insert(next.clone()) {
            queue.extend(edges.get(&next).into_iter().flatten().cloned());
        }
    }
    reach
}

/// **cold-relock**: an offline `grim lock` that succeeds over a store with no manifest,
/// or with every manifest and no layer. A requester whose manifest or requires layer
/// the store lacks requires what its row records, as grim 0.15.1 reads it, so the relock
/// drops every row no recorded edge reaches, whatever its kind, keeps every pin `grim
/// lock` keeps that one reaches, and writes the declaration's hash. The export stage's
/// relock of the same lock as a part holds no such row either, and drops or moves none of
/// those it keeps.
async fn check_cold_relock(world: &World, row: &str, failures: &mut Vec<String>) {
    let previous = world.lock();
    let Ok(online) = world.relock_of(&previous, &world.set).await else {
        return;
    };
    let online = pins(&online);
    let reach = recorded_reach(&previous, world.roots().into_keys());
    let mut homes = vec![("empty", empty_home(&world.store))];
    if world.spec.warm {
        homes.push(("manifests", manifests_home(&world.store)));
    }
    let at = |pins: &BTreeMap<NodeKey, PinnedIdentifier>, key: &NodeKey, pin: &PinnedIdentifier| {
        pins.get(key).is_some_and(|now| now.digest() == pin.digest())
    };
    for (store, (_home, cold)) in homes {
        for part in [None, Some(PLUGIN)] {
            let run = LockRun {
                offline: true,
                part,
                stamps: None,
                grim: &Invocation::WALKED,
            };
            let prior = match part {
                None => Prior::checked(&previous, &world.set),
                Some(_) => Prior::unproven(&previous),
            };
            let (set, dir) = (&world.set, world.dir.path());
            let offline = target_lock(set, Some(&prior), &Refresh::Nothing, &cold, &options(), dir, run).await;
            // A run that needs a tag or a member list the store lacks refuses and writes
            // nothing, as 0.15.1's offline lock does with an empty tag cache.
            let Ok(offline) = offline else {
                continue;
            };
            let relocked = offline.lock;
            let label = if part.is_some() { "part" } else { "lock" };
            let mut fail = |detail: String| failures.push(format!("cold-relock: {row}/{store}/{label}: {detail}"));
            if part.is_none() && relocked.metadata.declaration_hash != set.declaration_hash_cached() {
                fail("the relocked lock proves no declaration".to_string());
            }
            let now = pins(&relocked);
            for (kind, name) in now.keys().filter(|key| !reach.contains(*key)) {
                fail(format!("{kind}/{name} held, though no recorded edge reaches it"));
            }
            // A row a recorded edge reaches and the online relock keeps is owed, in a
            // part as in the lock: its absence and a digest other than its pin's are two
            // faults, and neither hides the other.
            for (key, pin) in pins(&previous) {
                let (kind, name) = &key;
                if !reach.contains(&key) || !at(&online, &key, &pin) {
                    continue;
                }
                match now.get(&key) {
                    None => fail(format!("{kind}/{name} dropped, though a recorded edge reaches it")),
                    Some(held) if held.digest() != pin.digest() => {
                        fail(format!(
                            "{kind}/{name} moved from {pin} to {held}, though a recorded edge reaches it"
                        ));
                    }
                    Some(_) => {}
                }
            }
        }
    }
}

/// Every edit row, then an offline relock over a cold store, on the lock as written and
/// with every recorded edge cut, as a lock 0.15.1 wrote records none. A shape whose
/// store serves manifest JSON also relocks over a store with every manifest and no
/// layer, where 0.15.1's dependent stands unread.
#[tokio::test]
async fn a_cold_offline_relock_keeps_only_what_a_recorded_edge_reaches() {
    let mut failures = Vec::new();
    let mut scenarios = 0;
    let shapes = SPECS.iter().chain(LATER_SPECS).chain(TABLE_SPECS);
    for spec in shapes.chain(STORE_SPECS).chain(UNWALKED_SPECS) {
        for edits in edits_of(spec) {
            for mv in MOVES {
                for edgeless in [false, true] {
                    let name = format!("{}/edgeless={edgeless}", row_name(spec, &edits, mv));
                    let mut world = World::build(spec).await;
                    if edgeless {
                        let mut lock = world.lock();
                        lock.iter_artifacts_mut().for_each(|row| row.requires.clear());
                        lock_io::save(&world.lock_path, &lock, None).unwrap();
                    }
                    for (edit, key) in &edits {
                        world.apply(*edit, key);
                    }
                    world.move_tags(mv).await;
                    check_cold_relock(&world, &name, &mut failures).await;
                    scenarios += 1;
                }
            }
        }
    }
    report(&failures, scenarios);
}

/// A stale lock lost `p`'s row, and an edit walks the declaration back to the hash
/// it still holds. Fuzz seed 27688.
#[tokio::test]
async fn an_edit_back_to_a_stale_hash_proves_no_lock_that_lost_a_row() {
    let spec = SPECS.iter().find(|s| s.name == "direct-over-another-digest").unwrap();
    let mut world = World::build(spec).await;
    let (p, q) = (key_of(Skill, "p"), key_of(Skill, "q"));
    let mut failures = Vec::new();
    edit_and_check(&mut world, Edit::Undeclare, &q, "delete q", &mut failures).await;
    world.redeclare(&q, source_of(DECLARE_Q.2));
    edit_and_check(&mut world, Edit::Undeclare, &p, "uninstall p", &mut failures).await;
    world.redeclare(&p, source_of(DECLARE_P.2));
    edit_and_check(&mut world, Edit::Undeclare, &q, "delete q again", &mut failures).await;
    report(&failures, 1);
}

/// The legacy path re-derives no bundle, so the row of `x` lacks the one a relock
/// writes, and a restamp must not call that a proof. Fuzz seed 85394.
#[tokio::test]
async fn a_restamp_states_the_row_a_relock_writes() {
    let spec = SPECS.iter().find(|s| s.name == "direct-over-two-flat-bundles").unwrap();
    let mut world = World::build(spec).await;
    let (b1, x) = (key_of(Bundle, "b1"), key_of(Skill, "x"));
    let mut failures = Vec::new();
    edit_and_check(&mut world, Edit::Remove, &b1, "remove b1", &mut failures).await;
    check_relock(&mut world, "first relock", &mut failures).await;
    edit_and_check(&mut world, Edit::Hand, &x, "hand x", &mut failures).await;
    world.redeclare(&b1, source_of("ghcr.io/acme/b1:1"));
    edit_and_check(&mut world, Edit::Undeclare, &b1, "delete b1", &mut failures).await;
    check_relock(&mut world, "second relock", &mut failures).await;
    report(&failures, 1);
}

/// `x` over two bundles that disagree: its row loses a bundle's stamp on the way,
/// and the last edit must not restamp a lock whose row a relock writes otherwise.
/// Fuzz seed 26046.
#[tokio::test]
async fn a_restamp_waits_for_the_stamp_an_earlier_edit_took() {
    let spec = SPECS
        .iter()
        .find(|s| s.name == "direct-over-two-disagreeing-flat-bundles")
        .unwrap();
    let mut world = World::build(spec).await;
    let (b2, x) = (key_of(Bundle, "b2"), key_of(Skill, "x"));
    let mut failures = Vec::new();
    edit_and_check(&mut world, Edit::Remove, &b2, "remove b2", &mut failures).await;
    world.redeclare(&b2, source_of("ghcr.io/acme/b2:1"));
    edit_and_check(&mut world, Edit::Remove, &x, "remove x", &mut failures).await;
    edit_and_check(&mut world, Edit::Undeclare, &b2, "delete b2", &mut failures).await;
    check_relock(&mut world, "relock", &mut failures).await;
    report(&failures, 1);
}

/// A clean lock from before `pinned` carried a tag, a line typed by hand, and a
/// tool taking that line out again: the old hash is a true proof once more, and
/// no tagless pin may cost it that.
#[tokio::test]
async fn a_true_proof_survives_a_tool_undo() {
    static SHAPE: Spec = Spec {
        name: "true-proof",
        arts: &[X1_SAME, BYSTANDER, CARGO],
        declare: &[DIRECT_X1, DECLARE_B],
        tags: &[X1],
        pre_change: true,
        ..BASE
    };
    let mut world = World::build(&SHAPE).await;
    let hash = world.lock().metadata.declaration_hash.clone();
    let c = key_of(Skill, "c");
    declare(&mut world.set, &c, source_of(DECLARE_C.2));
    world.apply(Edit::Remove, &c);
    assert_eq!(world.lock().metadata.declaration_hash, hash);
}

/// The design record's by-hand undo, on a row that stayed: the edit only marked
/// the declared bundle `transitive`, and the old hash read as a proof of a row no
/// relock writes. An edit that changes an unproven lock ends its hash. Fuzz seed 25318.
#[tokio::test]
async fn a_hand_undo_counts_a_row_an_edit_changed() {
    let spec = SPECS
        .iter()
        .find(|s| s.name == "declared-bundle-over-two-tags")
        .unwrap();
    let mut world = World::build(spec).await;
    let (p, pack) = (key_of(Skill, "p"), key_of(Bundle, "pack"));
    let mut failures = Vec::new();
    edit_and_check(&mut world, Edit::Hand, &p, "hand p", &mut failures).await;
    edit_and_check(&mut world, Edit::Remove, &pack, "remove pack", &mut failures).await;
    world.redeclare(&pack, source_of("ghcr.io/acme/pack:2"));
    world.redeclare(&p, source_of(DECLARE_P.2));
    assert!(
        !world.proven(&world.lock()),
        "the hash the edit left proves no declaration"
    );
    check_relock(&mut world, "relock", &mut failures).await;
    report(&failures, 1);
}

/// `a`'s own conflict keeps it unread in the first pass, so the pass ends with `x`
/// holding only the edge its pin does not name. The next pass brings the edge that
/// names it, both agree, and the pin stays.
#[tokio::test]
async fn a_pin_named_only_in_a_later_pass_stays() {
    static RELEASE_ORDER: Spec = Spec {
        name: "release-order",
        arts: &[
            X1_SAME,
            X2_SAME,
            art(Skill, "ghcr.io/acme/w:1", "w", &[]),
            art(Skill, "ghcr.io/acme/w:2", "w", &[]),
            art(Skill, "ghcr.io/acme/c:1", "c", &[(Skill, "w", "ghcr.io/acme/w:1")]),
            art(Skill, "ghcr.io/acme/d:1", "d", &[(Skill, "w", "ghcr.io/acme/w:2")]),
            art(Skill, "ghcr.io/acme/a:1", "a1", &[(Skill, "x", X1)]),
            art(Skill, "ghcr.io/acme/a:9", "a9", &[(Skill, "x", X1)]),
            art(Skill, "ghcr.io/acme/p:1", "p", &[(Skill, "a", "ghcr.io/acme/a:1")]),
            art(Skill, "ghcr.io/acme/b:1", "b", &[(Skill, "x", X2)]),
            art(Skill, "ghcr.io/acme/z:1", "z", &[(Skill, "a", "ghcr.io/acme/a:9")]),
        ],
        declare: &[
            (Skill, "w", "ghcr.io/acme/w:1"),
            (Skill, "c", "ghcr.io/acme/c:1"),
            (Skill, "d", "ghcr.io/acme/d:1"),
            DECLARE_P,
            DECLARE_B,
        ],
        ..BASE
    };
    let mut world = World::build(&RELEASE_ORDER).await;
    let held = pins(&world.lock())[&key_of(Skill, "x")].digest();
    world.apply(Edit::Remove, &key_of(Skill, "w"));
    assert!(!world.proven(&world.lock()), "the fixture");
    // Both of `x`'s tags move to one digest, and `a:1` joins `a:9`.
    world.move_tags(Move::AllToOne).await;
    let digest = world.digest_of("ghcr.io/acme/a:9").await;
    world
        .store
        .put_tag(&parse("ghcr.io/acme/a:1"), "1", &digest)
        .await
        .unwrap();
    // A hand declares `z`, which claims `a:9` in the same wave as `p`'s `a:1`.
    declare(&mut world.set, &key_of(Skill, "z"), source_of("ghcr.io/acme/z:1"));

    world.relock().await.expect("the edges agree");

    let now = &pins(&world.lock())[&key_of(Skill, "x")];
    assert_eq!(now.digest(), held, "{now}");
}

/// `x` over two flat bundles at two digests, its line deleted by hand and its row
/// gone. Holding exits 78, whose text says to declare the key, and a member row is
/// how the TUI does that.
#[tokio::test]
async fn a_member_install_declares_a_key_the_lock_cannot_show_settled() {
    let spec = SPECS
        .iter()
        .find(|s| s.name == "direct-over-two-disagreeing-flat-bundles")
        .unwrap();
    let mut world = World::build(spec).await;
    let x = key_of(Skill, "x");
    world.apply(Edit::Hand, &x);
    let mut lock = world.lock();
    lock.skills.retain(|row| row.name != "x");
    let id = parse(X2);
    let location = id.registry_repository();
    let row = RowAt {
        kind: Skill,
        name: "x",
        exact: true,
        location: &location,
    };
    let action = RowAction {
        row,
        op: BatchOp::Install,
        picked: false,
        id: &id,
    };
    let run = LockRun::project(false);
    let plan = plan_row(&world.set, Some(&lock), action, &world.access, world.dir.path(), run).await;
    assert!(
        matches!(&plan, Ok(RowPlan::Declare { binding }) if binding == "x"),
        "{plan:?}"
    );

    let relock = |declared: DesiredSet| {
        let (world, lock) = (&world, &lock);
        async move {
            let prior = Prior::edited(lock, &world.set, &declared);
            target_lock(
                &declared,
                Some(&prior),
                &Refresh::Nothing,
                &world.access,
                &options(),
                world.dir.path(),
                LockRun::project(false),
            )
            .await
            .map(|target| target.lock)
        }
    };
    let held = relock(world.set.clone()).await;
    assert_eq!(held.err().as_ref().and_then(conflict_key), Some(x.clone()), "held");
    let mut declared = world.set.clone();
    declare(&mut declared, &x, DeclaredSource::Registry(id));
    relock(declared).await.expect("declaring answers it");
}

/// A hand took `x`, which `p` asks for by its tag and `e` by a digest it does not
/// hold, and a typed line's removal would stamp that: the relock exits 78, and with
/// `p` gone a proof holds a pin against the digest. Fuzz seed 46699.
#[tokio::test]
async fn no_stamp_holds_a_pin_against_a_digest_edge() {
    let spec = SPECS
        .iter()
        .find(|s| s.name == "direct-over-a-digest-at-two-depths")
        .unwrap();
    let mut world = World::build(spec).await;
    let (x, e, p) = (key_of(Skill, "x"), key_of(Skill, "e"), key_of(Skill, "p"));
    let mut failures = Vec::new();
    edit_and_check(&mut world, Edit::Hand, &x, "hand x", &mut failures).await;
    world.redeclare(&e, source_of("ghcr.io/acme/e:1"));
    edit_and_check(&mut world, Edit::Remove, &e, "remove e", &mut failures).await;
    edit_and_check(&mut world, Edit::Remove, &p, "remove p", &mut failures).await;
    check_relock(&mut world, "relock", &mut failures).await;
    report(&failures, 1);
}

/// A hand took `l`, whose edge held `y` at `aa/y`, while `r` asks `zz/y` for the
/// same digest. `grim lock` re-resolves `y` at `zz/y`, so the declaration reaches
/// `y` there: Install on the `aa/y` row refuses and writes nothing, since declaring
/// `y` would replace what `r` asks for. The `l` row, whose name nothing reaches,
/// still declares. Fuzz seed 812 before the step mix grew.
#[tokio::test]
async fn install_refuses_a_row_whose_name_the_declaration_reaches_elsewhere() {
    let spec = SPECS.iter().find(|s| s.name == "cross-repository-diamond").unwrap();
    let (l, y) = (key_of(Skill, "l"), key_of(Skill, "y"));
    let mut failures = Vec::new();
    let mut probe = World::build(spec).await;
    let pin = pins(&probe.lock())[&y].clone();
    assert_eq!(
        pin.registry_repository(),
        "ghcr.io/aa/y",
        "the shape pins y on the left"
    );
    edit_and_check(&mut probe, Edit::Hand, &l, "hand l", &mut failures).await;
    probe.relock().await.expect("r alone resolves");
    let relocked = pins(&probe.lock())[&y].clone();
    assert_eq!(relocked.registry_repository(), "ghcr.io/zz/y", "{relocked}");
    assert_eq!(relocked.digest(), pin.digest(), "{relocked}");

    let mut world = World::build(spec).await;
    edit_and_check(&mut world, Edit::Hand, &l, "hand l", &mut failures).await;
    let install = |name: &str, location: String| RowStep {
        row: OfferedRow {
            kind: Skill,
            name: name.to_string(),
            exact: false,
            location,
        },
        op: BatchOp::Install,
        picked: false,
        tag: "1".to_string(),
    };
    let (bytes, roots, held) = (world.bytes(), world.roots(), pins(&world.lock()));
    let refused = world
        .row_action(&install("y", pin.registry_repository()))
        .await
        .expect_err("r reaches y at zz/y");
    assert!(resolve_error(&refused).is_none(), "{refused:#}");
    assert!(format!("{refused:#}").contains("ghcr.io/zz/y"), "{refused:#}");
    assert_eq!(world.bytes(), bytes, "the refusal wrote the lock");
    assert_eq!(world.roots(), roots, "the refusal wrote the declaration");
    assert_eq!(pins(&world.lock()), held, "the refusal moved a pin");

    let step = install("y", pin.registry_repository());
    let wrote = row_and_check(&mut world, &step, "tui-install(y,1)", &mut failures).await;
    assert!(wrote.is_none(), "{failures:?}");
    let step = install("l", "ghcr.io/acme/l".to_string());
    let wrote = row_and_check(&mut world, &step, "tui-install(l,1)", &mut failures).await;
    assert!(
        wrote.is_some_and(|wrote| wrote.declares && wrote.binding == "l"),
        "{failures:?}"
    );
    report(&failures, 2);
}

/// `pack`'s line holds off `p` and `q`, which ask two digests. `p`'s row left with a
/// tool while the line was out, so typing `p` back brings an edge the lock never
/// recorded, and with the line out again that edge resolves. `grim lock` and a fresh
/// resolve both exit 78: no hand rule carries a pin past a request it never answered,
/// and the design record lets a request that does not match resolve. Fuzz seed 3478
/// before the step mix grew.
#[tokio::test]
async fn a_line_typed_back_brings_an_edge_no_hand_rule_carries() {
    let spec = SPECS
        .iter()
        .find(|s| s.name == "declared-bundle-over-a-conflict")
        .unwrap();
    let mut world = World::build(spec).await;
    let (pack, p) = (key_of(Bundle, "pack"), key_of(Skill, "p"));
    let mut failures = Vec::new();
    edit_and_check(&mut world, Edit::Hand, &pack, "hand pack", &mut failures).await;
    edit_and_check(&mut world, Edit::Remove, &p, "remove p", &mut failures).await;
    world.redeclare(&pack, source_of(PACK2));
    world.redeclare(&p, source_of(DECLARE_P.2));
    edit_and_check(&mut world, Edit::Hand, &pack, "hand pack again", &mut failures).await;

    let onto = requests(&world.lock());
    assert!(
        !onto[&pack].contains(&parse(PACK1)),
        "the fixture: the lock never recorded p's edge"
    );
    let bytes = world.bytes();
    let refused = world.relock().await.unwrap_err();
    assert_eq!(conflict_key(&refused).as_ref(), Some(&pack), "{refused}");
    assert_eq!(world.bytes(), bytes, "a refused relock writes nothing");
    let fresh = world.fresh().await.unwrap_err();
    assert_eq!(conflict_key(&fresh).as_ref(), Some(&pack), "{fresh}");
    check_relock(&mut world, "relock", &mut failures).await;
    report(&failures, 1);
}

/// A line typed back for `p` beside `x`'s line taken by hand: `p` brings no edge the
/// lock lacks, so `grim lock` still carries `x` past the conflict a fresh resolve
/// refuses, as the hand rule does with no typed line.
#[tokio::test]
async fn a_line_typed_elsewhere_leaves_the_hand_rule_its_carry() {
    let spec = SPECS.iter().find(|s| s.name == "direct-smaller-over-conflict").unwrap();
    let mut world = World::build(spec).await;
    let (p, x) = (key_of(Skill, "p"), key_of(Skill, "x"));
    let mut failures = Vec::new();
    edit_and_check(&mut world, Edit::Hand, &p, "hand p", &mut failures).await;
    world.redeclare(&p, source_of(DECLARE_P.2));
    edit_and_check(&mut world, Edit::Hand, &x, "hand x", &mut failures).await;
    assert!(world.typed, "the fixture: a typed line is pending");
    check_relock(&mut world, "relock", &mut failures).await;
    report(&failures, 1);
}

/// A row cut to another repository under a hash that still matches, as a hand can.
/// A direct key holds a row only through its declaration, so a row at the cut pin's
/// repository is another artifact's and its guessed name refuses.
#[tokio::test]
async fn a_direct_key_holds_no_row_through_its_lock_row() {
    let spec = FUZZ_SPECS.iter().find(|s| s.name == "cross-repository-direct").unwrap();
    let mut world = World::build(spec).await;
    let mut lock = world.lock();
    let entry = lock.skills.iter_mut().find(|row| row.name == "y").unwrap();
    let digest = entry.source.pinned().unwrap().digest();
    let cut = parse("ghcr.io/zz/y:1").clone_with_digest(digest);
    entry.source = LockedSource::Registry(PinnedIdentifier::try_from(cut).unwrap());
    lock_io::save(&world.lock_path, &lock, None).unwrap();
    let step = RowStep {
        row: OfferedRow {
            kind: Skill,
            name: "y".to_string(),
            exact: false,
            location: "ghcr.io/zz/y".to_string(),
        },
        op: BatchOp::Update,
        picked: false,
        tag: "1".to_string(),
    };
    let mut failures = Vec::new();
    let wrote = row_and_check(&mut world, &step, "update(y)", &mut failures).await;
    assert!(wrote.is_none(), "{wrote:?}");
    report(&failures, 1);
}

/// `pack`'s line pinned by hand to the digest `pack:2` serves, which lists `n` where
/// `pack:1` lists `m`. A relock reads `pack` at that digest, so nothing holds `m`'s
/// member row and its Install at a tag nobody published refuses. Fuzz seed 1003
/// before the shapes grew.
#[tokio::test]
async fn a_bundle_line_pinned_by_hand_reads_the_members_its_digest_lists() {
    let spec = SPECS
        .iter()
        .find(|s| s.name == "declared-bundle-retagged-by-hand")
        .unwrap();
    let mut world = World::build(spec).await;
    let mut failures = Vec::new();
    let row = |name: &str, exact: bool| OfferedRow {
        kind: Skill,
        name: name.to_string(),
        exact,
        location: format!("ghcr.io/acme/{name}"),
    };
    let pick = RowStep {
        row: row("b", false),
        op: BatchOp::Install,
        picked: true,
        tag: "1".to_string(),
    };
    row_and_check(&mut world, &pick, "tui(b)", &mut failures).await;
    let digest = world.digest_of(PACK2).await;
    world.pin_by_hand(&key_of(Bundle, "pack"), &parse(PACK1), digest);
    check_proof(&world, "hand-pin(pack)", &mut failures);
    let install = RowStep {
        row: row("m", true),
        op: BatchOp::Install,
        picked: false,
        tag: "9".to_string(),
    };
    row_and_check(&mut world, &install, "tui(m)", &mut failures).await;
    report(&failures, 1);
}

/// The settings shape's lock holds settings rows the way other kinds hold theirs: a
/// declared one, and one a bundle stamps as its member and a skill records an edge onto.
#[tokio::test]
async fn a_settings_shape_locks_settings_rows() {
    let spec = LATER_SPECS
        .iter()
        .find(|s| s.name == "settings-in-a-bundle-and-an-edge")
        .unwrap();
    let lock = World::build(spec).await.lock();
    let names: Vec<&str> = lock.settings.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["s", "t"], "{lock:?}");
    assert!(!lock.settings[0].bundles.is_empty(), "{:?}", lock.settings[0]);
    let onto = requests(&lock);
    let edges = &onto[&key_of(ArtifactKind::Settings, "s")];
    assert!(
        edges.contains(&parse(SETTINGS_S1)) && edges.contains(&parse(SETTINGS_S2)),
        "{edges:?}"
    );
}

/// A line typed by hand and a tool taking it out again. What the hash proved
/// before, it proves after: the same lock.
fn check_round_trip(world: &World, before: &GrimoireLock, row: &str, failures: &mut Vec<String>) {
    let after = world.lock();
    if after.metadata.declaration_hash != before.metadata.declaration_hash {
        failures.push(format!("round-trip: {row}: the proof went"));
    }
    if rows(&after) != rows(before) {
        failures.push(format!("round-trip: {row}: the rows changed"));
    }
}

/// `key` typed at `id` and taken out by `edit`.
async fn type_and_take_out(
    world: &mut World,
    edit: Edit,
    key: &NodeKey,
    id: Identifier,
    row: &str,
    f: &mut Vec<String>,
) {
    let typed = world.typed;
    world.redeclare(key, DeclaredSource::Registry(id));
    edit_and_check(world, edit, key, row, f).await;
    // The line is gone again, so **exit-agrees** has no reason left to rest.
    world.typed = typed;
}

/// Every reached key of every shape at each tag, and a line nothing reached: the
/// fuzz meets these only by chance.
#[tokio::test]
async fn a_typed_line_costs_no_true_proof() {
    let mut failures = Vec::new();
    let mut scenarios = 0;
    for spec in SPECS.iter().chain(FUZZ_SPECS).chain(LATER_SPECS) {
        for relocked in [false, true] {
            let mut probe = World::build(spec).await;
            if relocked && probe.relock().await.is_err() {
                continue;
            }
            let before = probe.lock();
            if !probe.proven(&before) {
                continue;
            }
            let mut lines = vec![(key_of(Skill, "zz"), parse("ghcr.io/acme/zz:1"))];
            for (key, pin) in pins(&before) {
                if !probe.set.table(key.0).contains_key(&key.1) {
                    let repository = Identifier::new_registry(pin.repository(), pin.registry());
                    lines.extend(["1", "2"].map(|tag| (key.clone(), repository.clone_with_tag(tag))));
                }
            }
            for (key, id) in lines {
                for edit in [Edit::Remove, Edit::Undeclare] {
                    let mut world = World::build(spec).await;
                    if relocked {
                        world.relock().await.expect("it relocked above");
                    }
                    let name = format!("{}/relocked={relocked}/{edit:?}({id})", spec.name);
                    type_and_take_out(&mut world, edit, &key, id.clone(), &name, &mut failures).await;
                    check_round_trip(&world, &before, &name, &mut failures);
                    scenarios += 1;
                }
            }
        }
    }
    report(&failures, scenarios);
}

/// A small xorshift, so a failing seed replays with no dependency.
struct Rng(u64);

impl Rng {
    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % bound as u64) as usize
    }
}

/// `GRIM_PIN_FUZZ_SEEDS` is the last seed, not a count, raised for a longer hunt, and
/// `GRIM_PIN_FUZZ_FROM` the first, so a failing seed replays alone.
fn fuzz_seeds() -> std::ops::RangeInclusive<u64> {
    let read = |name: &str| match std::env::var(name) {
        Ok(set) => Some(set),
        Err(std::env::VarError::NotPresent) => None,
        Err(e) => panic!("{name}: {e}"),
    };
    seeds_between(read("GRIM_PIN_FUZZ_FROM"), read("GRIM_PIN_FUZZ_SEEDS"))
}

/// The seeds from `first` to `last`, each 1 and 64 when unset. A value that is set
/// and is no seed panics: falling back would run 64 seeds and pass.
fn seeds_between(first: Option<String>, last: Option<String>) -> std::ops::RangeInclusive<u64> {
    let seed = |set: Option<String>, default| {
        set.map_or(default, |seed| {
            seed.parse().unwrap_or_else(|e| panic!("{seed:?} is no fuzz seed: {e}"))
        })
    };
    let (first, last) = (seed(first, 1), seed(last, 64));
    // An empty range runs no seed, and the sequence test then passes on nothing.
    assert!(first <= last, "first fuzz seed {first} is past the last, {last}");
    first..=last
}

#[test]
#[should_panic(expected = "\"30_000\" is no fuzz seed")]
fn a_seed_written_with_separators_is_refused() {
    let _ = seeds_between(None, Some("30_000".to_string()));
}

#[test]
#[should_panic(expected = "\"3e4\" is no fuzz seed")]
fn a_seed_written_as_a_float_is_refused() {
    let _ = seeds_between(Some("3e4".to_string()), None);
}

#[test]
#[should_panic(expected = "first fuzz seed 10 is past the last, 5")]
fn a_first_seed_past_the_last_is_refused() {
    let _ = seeds_between(Some("10".to_string()), Some("5".to_string()));
}

#[test]
fn unset_seeds_run_the_default_range() {
    assert_eq!(seeds_between(None, None), 1..=64);
    assert_eq!(seeds_between(Some("5".into()), Some("9".into())), 5..=9);
}

/// One random step of a sequence.
async fn fuzz_step(
    world: &mut World,
    rng: &mut Rng,
    row: impl Fn(&str) -> String,
    failures: &mut Vec<String>,
) -> String {
    let spec = world.spec;
    let declared: Vec<NodeKey> = spec
        .declare
        .iter()
        .map(|(kind, name, _)| key_of(*kind, name))
        .filter(|key| world.set.table(key.0).contains_key(&key.1))
        .filter(|key| spec.no_snapshot != Some(key.1.as_str()))
        .collect();
    let gone: Vec<(ArtifactKind, &str, &str)> = spec
        .declare
        .iter()
        .copied()
        .filter(|(kind, name, _)| !world.set.table(*kind).contains_key(*name))
        .collect();
    match rng.below(13) {
        0..=2 if !declared.is_empty() => {
            let key = declared[rng.below(declared.len())].clone();
            let edit = [Edit::Remove, Edit::Undeclare, Edit::Hand, Edit::Repoint][rng.below(4)];
            let edit = match edit {
                Edit::Repoint if spec.repoint.is_none_or(|(name, _)| name != key.1) => Edit::Hand,
                edit => edit,
            };
            let step = format!("{edit:?}({})", key.1);
            edit_and_check(world, edit, &key, &row(&step), failures).await;
            step
        }
        3 => {
            let mv = MOVES[rng.below(MOVES.len())];
            world.move_tags(mv).await;
            // A part follows a bundle's tag, so only a moved tag gives its clauses work.
            if rng.below(2) == 0 {
                let part = part_step(world, rng, &row, failures).await;
                return format!("{mv:?},{part}");
            }
            format!("{mv:?}")
        }
        4 => part_step(world, rng, row, failures).await,
        5 if !gone.is_empty() || spec.arts.iter().any(|art| art.id == NEWCOMER.id) => {
            let newcomer = gone.is_empty() || rng.below(3) == 0;
            let (kind, name, id) = if newcomer {
                (Skill, "z", NEWCOMER.id)
            } else {
                gone[rng.below(gone.len())]
            };
            let key = key_of(kind, name);
            let step = format!("add({name})");
            if world.set.table(kind).contains_key(name) || !spec.arts.iter().any(|art| art.id == id) {
                return step;
            }
            add_and_check(world, &key, parse(id), &row(&step), failures).await;
            step
        }
        6 => {
            let previous = world.lock();
            let keys: Vec<NodeKey> = pins(&previous).into_keys().collect();
            if keys.is_empty() {
                return "update(nothing)".to_string();
            }
            let key = keys[rng.below(keys.len())].clone();
            let step = format!("update({})", key.1);
            let proven = world.proven(&previous);
            match world.update(std::slice::from_ref(&key.1)).await {
                Ok(()) if !proven => {
                    failures.push(format!("refusal: {}: a stale lock took a named update", row(&step)))
                }
                Ok(()) => {
                    let proof = proof_of(&previous, &[&world.set]);
                    check_others_hold(world, &previous, proof, Some(&key), &row(&step), failures);
                    check_proof(world, &row(&step), failures);
                    check_update_moves(world, &key, &row(&step), failures);
                }
                // Names select against a proof, so a stale lock refuses before any read.
                Err(error) if !proven => {
                    if !matches!(error.kind, ResolveErrorKind::StaleLock { .. }) {
                        failures.push(format!("refusal: {}: a stale lock refused with {error}", row(&step)));
                    }
                }
                Err(error) => check_refusal(&error, &row(&step), failures),
            }
            step
        }
        7 => {
            let fresh = world.fresh().await;
            let outcome = world.update(&[]).await;
            check_update_all(world, outcome, fresh, &row("update"), failures);
            "update".to_string()
        }
        8 if !gone.is_empty() => {
            let (kind, name, id) = gone[rng.below(gone.len())];
            world.redeclare(&key_of(kind, name), source_of(id));
            check_proof(world, &row("hand-redeclare"), failures);
            format!("hand-redeclare({name})")
        }
        9 | 10 if !declared.is_empty() => {
            let key = declared[rng.below(declared.len())].clone();
            let id = world.set.table(key.0).get(&key.1).and_then(DeclaredSource::identifier);
            let Some(id) = id.filter(|id| id.digest().is_none()).cloned() else {
                return "hand-pin(nothing)".to_string();
            };
            let elsewhere = |tag: &&&str| {
                parse(tag).registry_repository() == id.registry_repository()
                    && world.published.contains_key(**tag)
                    && **tag != id.to_string()
            };
            let Some(other) = spec.tags.iter().find(elsewhere) else {
                return "hand-pin(nothing)".to_string();
            };
            let digest = world.digest_of(other).await;
            world.pin_by_hand(&key, &id, digest);
            check_proof(world, &row("hand-pin"), failures);
            format!("hand-pin({},{other})", key.1)
        }
        11 | 12 => {
            let rows = world.offered(&world.lock());
            if rows.is_empty() {
                return "tui(nothing)".to_string();
            }
            let (op, picked) = ROW_ACTIONS[rng.below(ROW_ACTIONS.len())];
            let step = RowStep {
                row: rows[rng.below(rows.len())].clone(),
                op,
                picked,
                tag: ["1", "2", "9"][rng.below(3)].to_string(),
            };
            let name = format!("tui({step:?})");
            row_and_check(world, &step, &row(&name), failures).await;
            name
        }
        _ => {
            check_relock(world, &row("relock"), failures).await;
            "relock".to_string()
        }
    }
}

/// One random step of a sequence that types a line and takes it out again: a key
/// the lock reaches, or one nothing reaches.
async fn round_trip_step(
    world: &mut World,
    rng: &mut Rng,
    row: impl Fn(&str) -> String,
    failures: &mut Vec<String>,
) -> String {
    let before = world.lock();
    let proved = world.proven(&before);
    let reached: Vec<(NodeKey, PinnedIdentifier)> = pins(&before)
        .into_iter()
        .filter(|(key, _)| !world.set.table(key.0).contains_key(&key.1))
        .collect();
    let (key, id) = if reached.is_empty() || rng.below(2) == 0 {
        (key_of(Skill, "zz"), parse("ghcr.io/acme/zz:1"))
    } else {
        let (key, pin) = reached[rng.below(reached.len())].clone();
        let tag = ["1", "2"][rng.below(2)];
        (
            key,
            Identifier::new_registry(pin.repository(), pin.registry()).clone_with_tag(tag),
        )
    };
    let edit = [Edit::Remove, Edit::Undeclare][rng.below(2)];
    let step = format!("round-trip({edit:?},{id})");
    let name = row(&step);
    type_and_take_out(world, edit, &key, id, &name, failures).await;
    if proved {
        check_round_trip(world, &before, &name, failures);
    }
    step
}

/// `grim add` of `key` at `id`. It owes every pin what a relock owes, its target
/// included, and writes the target's pin `grim lock` writes over the same line typed
/// by hand.
async fn add_and_check(world: &mut World, key: &NodeKey, id: Identifier, row: &str, failures: &mut Vec<String>) {
    let previous = world.lock();
    let set_before = world.set.clone();
    let mut typed = world.set.clone();
    declare(&mut typed, key, DeclaredSource::Registry(id.clone()));
    // A line typed by hand ends the proof that vouches for a tagless pin, which then
    // resolves once, warned: the design record's stale residual. The add has that
    // proof, so its peer is the lock `grim lock` stamps before the line is typed.
    let stamped = match world.proven(&previous) {
        true => Some(world.relock_of(&previous, &set_before).await),
        false => None,
    };
    // A proof whose own relock refuses stamps nothing, so the line typed over it reads a
    // tagless pin afresh, that stale residual again, and no relock is the add's peer.
    let tagless = pins(&previous).values().any(|pin| pin.tag().is_none());
    let by_hand = match &stamped {
        Some(Err(_)) if tagless => None,
        Some(Ok(lock)) => Some(world.relock_of(lock, &typed).await),
        _ => Some(world.relock_of(&previous, &typed).await),
    };
    match world.add(key, id).await {
        Ok(()) => {
            for pair in world.pair_false(&world.lock()) {
                failures.push(format!("no-false-pair: {row}: add: {pair}"));
            }
            let proof = proof_of(&previous, &[&set_before, &world.set]);
            check_others_hold(world, &previous, proof, None, row, failures);
            if let Some(by_hand) = by_hand {
                check_held_pin(world, key, by_hand, row, failures);
            }
            check_proof(world, row, failures);
            check_recorded(world, row, failures);
        }
        Err(error) => {
            check_refusal(&error, row, failures);
            if by_hand.is_some_and(|lock| lock.is_ok()) {
                failures.push(format!(
                    "held-pin: {row}: `grim lock` takes the typed line, the add refused: {error}"
                ));
            }
        }
    }
}

/// The `grim add` that wrote the lock on disk against `by_hand`, `grim lock` over the
/// same line typed by hand: one target pin.
fn check_held_pin(
    world: &World,
    key: &NodeKey,
    by_hand: Result<GrimoireLock, ResolveError>,
    row: &str,
    failures: &mut Vec<String>,
) {
    let digest = |lock: &GrimoireLock| pins(lock).get(key).map(PinnedIdentifier::digest);
    match by_hand {
        Ok(typed) if digest(&typed) != digest(&world.lock()) => failures.push(format!(
            "held-pin: {row}: {}/{}: the add wrote {:?}, `grim lock` over the typed line {:?}",
            key.0,
            key.1,
            digest(&world.lock()),
            digest(&typed)
        )),
        Ok(_) => {}
        // A typed line ends the proof the add relocks under, so `grim lock`
        // may release another key's pin that names no recorded edge: only a
        // refusal on the target compares the two writers.
        Err(error) if conflict_key(&error).is_none_or(|refused| refused == *key) => {
            failures.push(format!(
                "held-pin: {row}: the add took {}/{}, `grim lock` over the typed line refuses: {error}",
                key.0, key.1
            ));
        }
        Err(_) => {}
    }
}

/// A row action's failure as the resolve raised it.
fn resolve_error(error: &anyhow::Error) -> Option<&ResolveError> {
    error.chain().find_map(|cause| match cause.downcast_ref() {
        Some(crate::error::Error::Resolve(error)) => Some(error),
        _ => cause.downcast_ref(),
    })
}

/// Which binding holds a row, read from the lock on disk, the declaration and the
/// store before the action runs.
struct RowHolder {
    /// The binding that must hold it.
    sure: Option<String>,
    /// `None` when nothing can: a row no pin that stands and no request sits at.
    maybe: Option<String>,
}

/// An Update and a pick on a proven lock read it as it stands. An Install reads its
/// relock, which keeps what a pin answers and resolves the rest, so a pin no request
/// answers does not hold the row and one that only may stay leaves the answer open.
/// The same holds on the way down: the relock reads a parent whose pin may move where
/// its request resolves now, so a row only that parent's old digest reaches is open.
fn row_holder(world: &World, previous: &GrimoireLock, step: &RowStep, walk: &Walk, owed: &Owed<'_>) -> RowHolder {
    let row = &step.row;
    let held = pins(previous);
    let sits_here = |keep: &dyn Fn(&NodeKey, &PinnedIdentifier) -> bool| -> BTreeSet<NodeKey> {
        held.iter()
            .filter(|(key, pin)| pin.registry_repository() == row.location && keep(key, pin))
            .map(|(key, _)| key.clone())
            .collect()
    };
    let reads_the_lock = owed.proof.is_some() && (step.op == BatchOp::Update || step.picked);
    if reads_the_lock {
        let rows: BTreeSet<NodeKey> = held.keys().cloned().collect();
        let holder = world.row_holder(row, &rows, &sits_here(&|_, _| true), &walk.requests);
        return RowHolder {
            sure: holder.clone(),
            maybe: holder,
        };
    }
    let standing: BTreeMap<NodeKey, PinnedIdentifier> = held
        .iter()
        .filter(|(key, pin)| owed.holds(key, pin))
        .map(|(key, pin)| (key.clone(), pin.clone()))
        .collect();
    let serving = |id: &Identifier| world.store.serving(&id.to_string()).into_iter().collect();
    let resolved = world.walk(&world.roots(), &standing, serving);
    let reached: BTreeSet<NodeKey> = held
        .keys()
        .filter(|key| walk.below.contains_key(*key) && resolved.below.contains_key(*key))
        .cloned()
        .collect();
    let asked: Requests = walk
        .requests
        .iter()
        .filter_map(|(key, ids)| {
            let both: BTreeSet<Identifier> = ids.intersection(resolved.requests.get(key)?).cloned().collect();
            (!both.is_empty()).then(|| (key.clone(), both))
        })
        .collect();
    let either: BTreeSet<NodeKey> = held.keys().chain(walk.below.keys()).cloned().collect();
    RowHolder {
        sure: world.row_holder(row, &reached, &sits_here(&|key, pin| owed.holds(key, pin)), &asked),
        maybe: world.row_holder(
            row,
            &either,
            &sits_here(&|key, pin| owed.answered(key, pin)),
            &walk.requests,
        ),
    }
}

/// Whether a catalog row nothing holds guesses the name of a key the declaration
/// reaches without declaring it: that key sits at another location, so an Install or
/// a pick that declared the name would replace what its parents ask for.
struct Replacing {
    /// The key keeps a row the walk reaches, and no conflict on it can declare.
    sure: bool,
    /// A lock row or the walk names the key, or the row's holder is open.
    maybe: bool,
    /// The relock the action reads exits 78 on the row's own key: `Some(true)` when
    /// the action owes the row's line, a member's or one a request onto the key sits
    /// at the row's location for, `Some(false)` when it owes a refusal.
    declare_owed: Option<bool>,
}

/// An Update and a pick on a proven lock read the key's row as it stands; any other
/// action reads its relock, which a 78 on the row's own key, `own_conflict`, ends.
fn replacing(
    world: &World,
    previous: &GrimoireLock,
    step: &RowStep,
    (holder, walk, owed, own_conflict, declare_owed): (&RowHolder, &Walk, &Owed<'_>, bool, Option<bool>),
) -> Replacing {
    let row = &step.row;
    let named = step.op == BatchOp::Update && !step.picked;
    let declared = world.set.table(row.kind).contains_key(&row.name);
    let open = !row.exact && !named && !declared && holder.sure.is_none();
    let locked = previous
        .iter_artifacts()
        .any(|entry| entry.kind == row.kind && entry.name == row.name)
        || (row.kind == Bundle && previous.bundles.iter().any(|bundle| bundle.name == row.name));
    let reached = walk.below.contains_key(&(row.kind, row.name.clone()));
    let reads_the_lock = owed.proof.is_some() && (step.op == BatchOp::Update || step.picked);
    let settled = if reads_the_lock { locked } else { !own_conflict };
    Replacing {
        sure: open && holder.maybe.is_none() && reached && settled,
        maybe: open && (locked || reached),
        declare_owed,
    }
}

/// Whether the relock `step` reads before it writes exits 78 on the row's own key,
/// and if so whether the action owes the row's line: a member names its own binding,
/// and a catalog row's location holds a request onto the key or nothing does. That
/// relock is `grim lock` over the declaration as it stands.
async fn declare_owed(
    world: &World,
    previous: &GrimoireLock,
    step: &RowStep,
    (owed, own_conflict, asked): (&Owed<'_>, bool, &Requests),
) -> Option<bool> {
    let row = &step.row;
    let named = step.op == BatchOp::Update && !step.picked;
    let reads_the_lock = owed.proof.is_some() && (step.op == BatchOp::Update || step.picked);
    if !own_conflict || named || reads_the_lock || world.set.table(row.kind).contains_key(&row.name) {
        return None;
    }
    let own = (row.kind, row.name.clone());
    let refused = world.relock_of(previous, &world.set).await.err();
    if refused.as_ref().and_then(conflict_key) != Some(own.clone()) {
        return None;
    }
    let here = asked
        .get(&own)
        .is_some_and(|ids| ids.iter().any(|id| id.registry_repository() == row.location));
    Some(row.exact || here)
}

/// The requests the action's relock may read: the walk over the declaration, and over
/// it with the row's line typed under each binding the action may declare, whose
/// edges a pick or a declaring Install adds before its relock.
fn typed_requests(world: &World, previous: &GrimoireLock, step: &RowStep, holder: &RowHolder, walk: &Walk) -> Requests {
    let mut asked = walk.requests.clone();
    let id = parse(&format!("{}:{}", step.row.location, step.tag));
    let bindings = [Some(&step.row.name), holder.sure.as_ref(), holder.maybe.as_ref()];
    let serving = |id: &Identifier| world.store.serving(&id.to_string()).into_iter().collect();
    for binding in bindings.into_iter().flatten() {
        let mut roots = world.roots();
        roots.insert((step.row.kind, binding.clone()), DeclaredSource::Registry(id.clone()));
        for (key, ids) in world.walk(&roots, &pins(previous), serving).requests {
            asked.entry(key).or_default().extend(ids);
        }
    }
    asked
}

/// A refused action wrote nothing, and its reason is one the state gives: a 78 needs
/// requests that can disagree and is never an Install's on its own key, a stale lock
/// refuses a name, a row nothing holds refuses an Update, a guessed name bound
/// elsewhere refuses before any relock, and so does one reached elsewhere. A held
/// Install resolves no tag, unless it owes the row's line, which names the row's tag.
fn check_row_refusal(
    world: &World,
    step: &RowStep,
    error: &anyhow::Error,
    (holder, replaces, previous, asked): (&RowHolder, &Replacing, &GrimoireLock, &Requests),
    row: &str,
    failures: &mut Vec<String>,
) {
    let mut fail = |detail: String| failures.push(format!("tui-held: {row}: {detail}"));
    let repair = step.op == BatchOp::Install && !step.picked;
    let bound = world.set.table(step.row.kind).contains_key(&step.row.name);
    let resolved = resolve_error(error);
    let line = parse(&format!("{}:{}", step.row.location, step.tag));
    let owed_line = replaces.declare_owed == Some(true)
        && resolved.is_some_and(|error| error.reference.source.identifier() == Some(&line));
    match resolved.map(|error| &error.kind) {
        None if replaces.declare_owed == Some(true) => fail(format!(
            "{:?} refused on a 78 over its own key, though a request onto it sits at the row: {error:#}",
            step.row
        )),
        Some(ResolveErrorKind::BundleConflict { kind, name, .. }) => {
            let key = (*kind, name.clone());
            if !world.may_conflict(&key, previous, asked) {
                fail(format!("a 78 on {kind}/{name}, whose requests agree"));
            }
            if repair && *kind == step.row.kind && *name == step.row.name {
                fail(format!("Install exits 78 on its own key {kind}/{name}"));
            }
        }
        Some(ResolveErrorKind::StaleLock { .. }) if step.op == BatchOp::Update && !world.proven(previous) => {}
        Some(ResolveErrorKind::NotDeclared) if holder.sure.is_none() && step.op == BatchOp::Update => {}
        Some(ResolveErrorKind::TagNotFound) if !(repair && holder.sure.is_some()) || owed_line => {}
        None if holder.sure.is_none() && bound && !step.row.exact => {}
        None if replaces.maybe => {}
        _ => fail(format!("{:?} held by {:?} failed: {error:#}", step.row, holder.sure)),
    }
}

/// The binding an action took against the one the rule names. Only a 78 on the row's
/// own key that owes the row's line, `declare_owed`, lets an Install declare a row
/// something holds.
fn check_row_binding(
    step: &RowStep,
    wrote: &RowWrote,
    (holder, declare_owed, bound): (&RowHolder, Option<bool>, bool),
    row: &str,
    failures: &mut Vec<String>,
) {
    let mut fail = |detail: String| failures.push(format!("tui-held: {row}: {detail}"));
    let elsewhere = bound && !step.row.exact;
    let unbound_row = holder.sure.is_none() && holder.maybe.is_none();
    if unbound_row && elsewhere {
        fail(format!("{:?} went over a binding of another artifact", step.row));
    }
    let name = &step.row.name;
    match (step.op, step.picked) {
        (BatchOp::Update, false) => {
            if wrote.declares || holder.sure.as_ref() != Some(&wrote.binding) {
                fail(format!("Update took {wrote:?} where {:?} holds the row", holder.sure));
            }
        }
        (_, true) => {
            if !wrote.declares {
                fail(format!("a pick wrote no line, {wrote:?}"));
            }
            if holder.sure.as_ref().is_some_and(|binding| *binding != wrote.binding)
                || (unbound_row && wrote.binding != *name)
            {
                fail(format!("a pick took {wrote:?} where {:?} holds the row", holder.sure));
            }
        }
        _ => match &holder.sure {
            Some(_) if wrote.declares && declare_owed == Some(true) => {}
            Some(binding) if wrote.declares || wrote.binding != *binding => {
                fail(format!("Install took {wrote:?} where {binding} holds the row"));
            }
            None if unbound_row && (!wrote.declares || wrote.binding != *name) => {
                fail(format!("Install took {wrote:?} on a row nothing holds"));
            }
            _ => {}
        },
    }
}

/// `step` on its row, held to the rule stated from the lock, the declaration and the
/// store: never to the relock `plan_row` reads, which is the writer under test. What
/// it wrote owes every pin it did not aim at what a relock owes, an Update moves its
/// target and matches `grim update <binding>`, and a refusal writes nothing.
async fn row_and_check(world: &mut World, step: &RowStep, row: &str, failures: &mut Vec<String>) -> Option<RowWrote> {
    let previous = world.lock();
    let bytes = world.bytes();
    let set_before = world.set.clone();
    let proof = proof_of(&previous, &[&set_before]);
    let walk = world.walk_over(&previous);
    let owed = Owed {
        set: &set_before,
        proof,
        recorded: requests(&previous),
        asked: walk.requests.clone(),
        listed: world.listed_requests(&previous, &pins(&previous), proof),
    };
    let holder = row_holder(world, &previous, step, &walk, &owed);
    let own_key = (step.row.kind, step.row.name.clone());
    let own_conflict = world.may_conflict(&own_key, &previous, &walk.requests);
    let owes = declare_owed(world, &previous, step, (&owed, own_conflict, &walk.requests)).await;
    let replaces = replacing(world, &previous, step, (&holder, &walk, &owed, own_conflict, owes));
    let asked = typed_requests(world, &previous, step, &holder, &walk);
    let roots_before = world.roots();
    let bound = set_before.table(step.row.kind).contains_key(&step.row.name);
    let named = step.op == BatchOp::Update && !step.picked;
    let by_name = match holder.sure.as_ref().filter(|_| named && proof.is_some()) {
        Some(binding) => Some(world.named_update(binding).await),
        None => None,
    };

    let outcome = world.row_action(step).await;
    let wrote = match outcome {
        Ok(wrote) => wrote,
        Err(error) => {
            if world.bytes() != bytes || world.roots() != roots_before {
                failures.push(format!("tui-held: {row}: a refused action wrote"));
            }
            let read = (&holder, &replaces, &previous, &asked);
            check_row_refusal(world, step, &error, read, row, failures);
            return None;
        }
    };
    if replaces.declare_owed == Some(false) {
        let (kind, name) = (step.row.kind, &step.row.name);
        failures.push(format!(
            "tui-held: {row}: {wrote:?} on a 78 over {kind}/{name}, though no request onto it sits at the row"
        ));
    }
    if replaces.sure {
        let (kind, name) = (step.row.kind, &step.row.name);
        failures.push(format!(
            "tui-held: {row}: {wrote:?} over {kind}/{name}, which the declaration reaches at another location"
        ));
    }
    check_row_binding(step, &wrote, (&holder, owes, bound), row, failures);
    if !wrote.declares && world.set.declaration_hash_cached() != set_before.declaration_hash_cached() {
        failures.push(format!("tui-held: {row}: the declaration changed"));
    }
    let target = (step.row.kind, wrote.binding.clone());
    let exempt = (step.op == BatchOp::Update).then_some(&target);
    let proof = proof_of(&previous, &[&set_before, &world.set]);
    check_others_hold(world, &previous, proof, exempt, row, failures);
    check_proof(world, row, failures);
    check_recorded(world, row, failures);
    for pair in world.pair_false(&world.lock()) {
        failures.push(format!("no-false-pair: {row}: {pair}"));
    }
    if step.op == BatchOp::Update {
        check_update_moves(world, &target, row, failures);
    }
    if let Some(Ok(by_name)) = by_name
        && rows(&world.lock()) != rows(&by_name)
    {
        let (wrote, by_name) = (rows(&world.lock()), rows(&by_name));
        failures.push(format!(
            "update-agrees: {row}: the row's Update wrote\n{wrote}\n`grim update` of its binding\n{by_name}"
        ));
    }
    Some(wrote)
}

/// Every action on a row: Install and Update, each with the catalog's tag and a pick.
const ROW_ACTIONS: [(BatchOp, bool); 4] = [
    (BatchOp::Install, false),
    (BatchOp::Update, false),
    (BatchOp::Install, true),
    (BatchOp::Update, true),
];

/// Every row of every shape, proven and with a line a hand took out, under every
/// action: the fuzz reaches these only past the default seed count.
#[tokio::test]
async fn every_action_on_every_row_takes_the_binding_that_holds_it() {
    let mut failures = Vec::new();
    let mut scenarios = 0;
    let shapes = SPECS.iter().chain(FUZZ_SPECS).chain(LATER_SPECS);
    for spec in shapes.chain(TABLE_SPECS).chain(UNWALKED_SPECS) {
        let lines: Vec<NodeKey> = edits_of(spec)
            .into_iter()
            .filter_map(|mut edits| edits.pop().map(|(_, key)| key))
            .take(1)
            .collect();
        for hand in std::iter::once(None).chain(lines.iter().map(Some)) {
            let prepared = async |spec: &'static Spec| {
                let mut world = World::build(spec).await;
                if let Some(key) = hand {
                    world.apply(Edit::Hand, key);
                }
                world
            };
            let probe = prepared(spec).await;
            for offered in probe.offered(&probe.lock()) {
                for (op, picked) in ROW_ACTIONS {
                    for tag in ["1", "2"] {
                        let step = RowStep {
                            row: offered.clone(),
                            op,
                            picked,
                            tag: tag.to_string(),
                        };
                        let mut world = prepared(spec).await;
                        let row = format!("{}/hand={:?}/{step:?}", spec.name, hand.map(|key| &key.1));
                        row_and_check(&mut world, &step, &row, &mut failures).await;
                        scenarios += 1;
                    }
                }
            }
        }
    }
    report(&failures, scenarios);
}

/// A key the lock holds as a dependency at each tag it has, and a line a hand took
/// out, typed back by `grim add`, after every tag move.
#[tokio::test]
async fn an_add_keeps_the_pin_grim_lock_keeps_over_the_same_line() {
    let mut failures = Vec::new();
    let mut scenarios = 0;
    let shapes = SPECS.iter().chain(FUZZ_SPECS).chain(LATER_SPECS).chain(TABLE_SPECS);
    for spec in shapes.chain(UNWALKED_SPECS) {
        let probe = World::build(spec).await;
        let mut lines: Vec<(NodeKey, Identifier, bool)> = Vec::new();
        for (key, pin) in pins(&probe.lock()) {
            if !probe.set.table(key.0).contains_key(&key.1) {
                let repository = Identifier::new_registry(pin.repository(), pin.registry());
                let tags = ["1", "2"].map(|tag| repository.clone_with_tag(tag));
                let published = tags
                    .into_iter()
                    .filter(|id| probe.published.contains_key(&id.to_string()));
                lines.extend(published.map(|id| (key.clone(), id, false)));
            }
        }
        for (key, source) in probe.roots() {
            if let Some(id) = source.identifier().filter(|_| spec.no_snapshot != Some(key.1.as_str())) {
                lines.push((key, id.clone(), true));
            }
        }
        for mv in MOVES {
            for (key, id, by_hand) in &lines {
                let mut world = World::build(spec).await;
                if *by_hand {
                    world.apply(Edit::Hand, key);
                }
                world.move_tags(mv).await;
                let row = format!("{}/hand={by_hand}/{mv:?}/add({id})", spec.name);
                add_and_check(&mut world, key, id.clone(), &row, &mut failures).await;
                scenarios += 1;
            }
        }
    }
    report(&failures, scenarios);
}

/// A tag that moved onto a manifest with a new edge, then every move of the key's
/// tags: `grim lock`, and `grim add` of each line a pin the lock holds may answer. A
/// typed line reads its fresh tag until the edges onto it stamp the pin, and what that
/// read asked for may move no pin `grim lock` over the same line keeps.
#[tokio::test]
async fn a_tag_that_gains_an_edge_moves_no_pin_grim_lock_keeps() {
    let mut failures = Vec::new();
    let mut scenarios = 0;
    for (spec, tag, gained) in EDGE_SPECS {
        let probe = World::build(spec).await;
        let mut lines: Vec<(NodeKey, Identifier)> = Vec::new();
        for (key, pin) in pins(&probe.lock()) {
            if !probe.set.table(key.0).contains_key(&key.1) {
                let repository = Identifier::new_registry(pin.repository(), pin.registry());
                let tags = ["1", "2"].map(|tag| repository.clone_with_tag(tag));
                let published = tags
                    .into_iter()
                    .filter(|id| probe.published.contains_key(&id.to_string()));
                lines.extend(published.map(|id| (key.clone(), id)));
            }
        }
        for mv in MOVES {
            let mut world = World::build(spec).await;
            world.retag_gaining(tag, *gained).await;
            world.move_tags(mv).await;
            check_relock(
                &mut world,
                &format!("{}/gained/{mv:?}/relock", spec.name),
                &mut failures,
            )
            .await;
            scenarios += 1;
            for (key, id) in &lines {
                let mut world = World::build(spec).await;
                world.retag_gaining(tag, *gained).await;
                world.move_tags(mv).await;
                let row = format!("{}/gained/{mv:?}/add({id})", spec.name);
                add_and_check(&mut world, key, id.clone(), &row, &mut failures).await;
                scenarios += 1;
            }
        }
    }
    report(&failures, scenarios);
}

/// The bundle bindings of `spec`: each bundle it declares from a registry, which the
/// plugin includes, and each one an edge names. A `<plugin>:<bundle>` pick names one.
fn bundle_bindings(spec: &Spec) -> BTreeSet<NodeKey> {
    let declared = spec
        .declare
        .iter()
        .filter(|(kind, _, id)| *kind == Bundle && PathSource::parse(id).is_err());
    let edges = spec
        .arts
        .iter()
        .flat_map(|art| art.edges)
        .filter(|(kind, _, _)| *kind == Bundle);
    declared
        .chain(edges)
        .map(|(kind, name, _)| key_of(*kind, name))
        .collect()
}

/// One write of the plugin's part: its first, the export stage's relock of it,
/// or `grim update --marketplace` of one member.
async fn part_step(
    world: &mut World,
    rng: &mut Rng,
    row: impl Fn(&str) -> String,
    failures: &mut Vec<String>,
) -> String {
    if world.no_part {
        return "part(none)".to_string();
    }
    let Some(part) = world.part().cloned() else {
        first_part(world, &row("part(first)"), failures).await;
        return "part(first)".to_string();
    };
    // A stored part keeps no bundle row, so the shape names the bundle bindings. The
    // draws stay two either way, so every later step of a seed is the one it was.
    let members: Vec<NodeKey> = pins(&part).into_keys().chain(bundle_bindings(world.spec)).collect();
    let member = (!members.is_empty() && rng.below(2) == 0).then(|| members[rng.below(members.len())].clone());
    let step = member
        .as_ref()
        .map_or_else(|| "part(export)".to_string(), |key| format!("part-update({})", key.1));
    part_relock_and_check(world, member.as_ref(), &row(&step), failures).await;
    step
}

/// The part's first write. Only a conflict between the includes refuses it, and
/// then the shape has no part.
async fn first_part(world: &mut World, row: &str, failures: &mut Vec<String>) {
    // A shape whose part grim 0.15.1 wrote before its edges showed has it already.
    if let Some(part) = world.part().cloned() {
        check_part_stamps(world, &part, row, failures);
        return;
    }
    let (outcome, _) = world.relock_part(&PluginSelection::All).await;
    match outcome {
        Err(error) => {
            world.no_part = true;
            let error = anyhow::Error::from(error);
            if classify_error(&error) != ExitCode::ConfigError {
                failures.push(format!("part-error: {row}: {error:#}"));
            }
        }
        Ok(()) => {
            let part = world.part().cloned().expect("a written part");
            check_part_stamps(world, &part, row, failures);
        }
    }
}

/// A write that reads every bundle the part reaches, its first or the export
/// stage's, records in each bundle stamp the digest the bundle's tag served, so the
/// next write reads the bundle there.
fn check_part_stamps(world: &World, part: &GrimoireLock, row: &str, failures: &mut Vec<String>) {
    for entry in part.iter_artifacts() {
        for stamp in &entry.bundles {
            let (tag, digest) = split_stamp(&stamp.tag);
            let Ok(id) = Identifier::parse(&format!("{}:{tag}", stamp.repo)) else {
                continue;
            };
            let (kind, name) = (entry.kind, &entry.name);
            match digest {
                None => failures.push(format!(
                    "part-stamps: {row}: {kind}/{name} keeps the stamp {}:{tag} with no digest",
                    stamp.repo
                )),
                Some(digest) if !world.store.served(id.to_string(), digest.clone()) => failures.push(format!(
                    "no-false-pair: {row}: part: {kind}/{name} stamps {id} at {digest}, which it never served"
                )),
                Some(_) => {}
            }
        }
    }
}

/// The export stage's relock of the part, or with `member` `grim update
/// --marketplace` of that member: [`check_export`] and [`check_member_pick`].
/// A pick names a row exactly when the export would write one under that name:
/// otherwise it matches nothing, exits 79 and saves no lock.
async fn part_relock_and_check(world: &mut World, member: Option<&NodeKey>, row: &str, failures: &mut Vec<String>) {
    let before = world.part().cloned().expect("a part to relock");
    let plugin = PLUGIN.to_string();
    let exported = match member {
        Some(key) => world.export_holds(key).await,
        None => None,
    };
    let selection = match member {
        None => PluginSelection::Reconcile(BTreeSet::from([plugin])),
        Some(key) => {
            let pick = PluginPick::Members(BTreeSet::from([key.1.clone()]));
            PluginSelection::Some(BTreeMap::from([(plugin, pick)]))
        }
    };
    let (outcome, logs) = world.relock_part(&selection).await;
    if let Err(error) = outcome {
        let error = anyhow::Error::from(error);
        match classify_error(&error) {
            ExitCode::ConfigError => {}
            ExitCode::NotFound if exported == Some(false) => {}
            ExitCode::NotFound if member.is_some_and(|key| unrecorded_edge_bundle(world, key, &before)) => {}
            ExitCode::NotFound if exported == Some(true) => failures.push(format!(
                "part-pick: {row}: refused ({error:#}), though the export writes the row it names"
            )),
            _ => failures.push(format!("part-error: {row}: {error:#}")),
        }
        return;
    }
    if exported == Some(false) {
        let key = member.expect("a pick");
        failures.push(format!(
            "part-pick: {row}: exit 0 on {}/{}, which the export writes no row for; it must match nothing",
            key.0, key.1
        ));
    }
    let after = world.part().cloned().expect("a written part");
    if !written_by_no_grim(&before)
        && let Some(key) = world.fresh_part_conflict().await
        && settles(&before, &after, &key)
    {
        failures.push(format!(
            "part-exit-agrees: {row}: {}/{} settled at exit 0, where a write of the part from nothing refuses",
            key.0, key.1
        ));
    }
    match member {
        None => check_export(world, &before, &after, &logs, row, failures),
        Some(key) => check_member_pick(world, key, &before, &after, &logs, row, failures),
    }
    for pair in world.pair_false(&after) {
        failures.push(format!("no-false-pair: {row}: part: {pair}"));
    }
}

/// Whether no grim writes `part`: grim 0.15.1 leaves every pin without a tag, and it
/// reads no row that records an edge or a settings package.
fn written_by_no_grim(part: &GrimoireLock) -> bool {
    let tagless = part
        .iter_artifacts()
        .all(|row| row.source.pinned().is_none_or(|pin| pin.tag().is_none()));
    let unread = !part.settings.is_empty() || part.iter_artifacts().any(|row| !row.requires.is_empty());
    tagless && unread
}

/// Whether the write of `before` into `after` decided `key`: its pin took another
/// digest, a bundle's stamp left its row, or a request onto it joined the rows. A key
/// whose claims conflict exits 78 there; a pin a recorded claim carries stays put.
fn settles(before: &GrimoireLock, after: &GrimoireLock, key: &NodeKey) -> bool {
    let digest = |lock: &GrimoireLock| pins(lock).get(key).map(PinnedIdentifier::digest);
    let stamps = |lock: &GrimoireLock| -> BTreeSet<String> {
        let mut rows = lock.iter_artifacts();
        let row = rows.find(|row| (row.kind, &row.name) == (key.0, &key.1));
        row.into_iter()
            .flat_map(|row| &row.bundles)
            .map(|stamp| stamp.repo.clone())
            .collect()
    };
    let asked = |lock: &GrimoireLock| requests(lock).remove(key).unwrap_or_default();
    digest(before) != digest(after)
        || !stamps(before).is_subset(&stamps(after))
        || !asked(after).is_subset(&asked(before))
}

/// Whether a pick of `key` names a bundle only an edge asks for, in a part whose
/// stamps record no digest. Such a part holds no row for that bundle and the
/// plugin does not declare it, so the pick matches nothing (79), 0.15.1's exit for
/// every bundle pick, where the part this grim wrote takes it at exit 0.
fn unrecorded_edge_bundle(world: &World, key: &NodeKey, part: &GrimoireLock) -> bool {
    key.0 == Bundle && !world.includes().contains_key(key) && recorded_digests(part).values().all(Option::is_none)
}

/// The lines of `logs` that carry `marker`, and whether one names `key`.
fn warned(logs: &str, marker: &str, key: &NodeKey) -> bool {
    let name = format!("{} '{}'", key.0, key.1);
    logs.lines().any(|line| line.contains(marker) && line.contains(&name))
}

/// The export stage reads every bundle a stamp records at that digest and follows
/// the tag of every other, so no pin moves but one below a followed bundle or a
/// tagless one, and a warning names each of those that changed: the part warning,
/// or the warning that a tagless pin resolved afresh. It then records every
/// bundle's digest.
fn check_export(
    world: &World,
    before: &GrimoireLock,
    after: &GrimoireLock,
    logs: &str,
    row: &str,
    failures: &mut Vec<String>,
) {
    let below = world.followed_below(before, None);
    let includes = world.includes();
    let (was, now) = (pins(before), pins(after));
    let digest =
        |pins: &BTreeMap<NodeKey, PinnedIdentifier>, key: &NodeKey| pins.get(key).map(PinnedIdentifier::digest);
    let keys: BTreeSet<&NodeKey> = was.keys().chain(now.keys()).collect();
    for key in keys {
        let (old, new) = (digest(&was, key), digest(&now, key));
        if old == new {
            continue;
        }
        let tagless = was.get(key).is_some_and(|pin| pin.tag().is_none());
        let followed = format!("plugin '{PLUGIN}' follows the tag of");
        let resolved = format!("plugin '{PLUGIN}': re-resolved");
        // An include is the plugin's own line, which no warning names, as `grim lock`
        // names no declared key.
        let included = includes.contains_key(key);
        if !below.contains(key) && !tagless {
            failures.push(format!(
                "part-no-move: {row}: {}/{} went from {old:?} to {new:?}",
                key.0, key.1
            ));
        } else if !(warned(logs, &followed, key) || warned(logs, &resolved, key) || tagless && included) {
            failures.push(format!(
                "part-warning: {row}: {}/{} went from {old:?} to {new:?}, unnamed in {logs:?}",
                key.0, key.1
            ));
        }
    }
    check_part_stamps(world, after, row, failures);
}

/// A member pick changes nothing outside what the member reaches: every other row
/// stays as the part held it, stamps included, and no row joins. A digest the part
/// did not record lands only on a row the pick selects, at a digest its tag served.
/// A row kept with no tag in a part whose stamps record a digest is named, since
/// nothing offline shows the reference that resolved it; a part whose stamps record
/// none warns of nothing.
fn check_member_pick(
    world: &World,
    member: &NodeKey,
    before: &GrimoireLock,
    after: &GrimoireLock,
    logs: &str,
    row: &str,
    failures: &mut Vec<String>,
) {
    let closure = world.part_closure(member, before, after);
    let rows = |lock: &GrimoireLock| -> BTreeMap<NodeKey, LockedArtifact> {
        let entries = lock
            .iter_artifacts()
            .map(|entry| ((entry.kind, entry.name.clone()), entry.clone()));
        entries.collect()
    };
    let (was, now) = (rows(before), rows(after));
    let keys: BTreeSet<&NodeKey> = was.keys().chain(now.keys()).collect();
    // Inside the pick only the member, and what it reads by a tag the part does not
    // record, may move: every other pin answers the edge its row records.
    let below = world.followed_below(before, Some(member));
    let (pins_was, pins_now) = (pins(before), pins(after));
    for key in keys {
        if !closure.contains(key) {
            if was.get(key) != now.get(key) {
                failures.push(format!(
                    "part-held: {row}: {}/{} outside the pick went from {:?} to {:?}",
                    key.0,
                    key.1,
                    was.get(key),
                    now.get(key)
                ));
            }
            continue;
        }
        let digest = |pins: &BTreeMap<NodeKey, PinnedIdentifier>| pins.get(key).map(PinnedIdentifier::digest);
        let (old, new) = (digest(&pins_was), digest(&pins_now));
        if key != member && old.is_some() && old != new && !below.contains(key) {
            failures.push(format!(
                "part-no-move: {row}: {}/{} went from {old:?} to {new:?}",
                key.0, key.1
            ));
        }
    }
    if logs.contains(&format!("plugin '{PLUGIN}' follows the tag of")) {
        failures.push(format!("part-warning: {row}: a member pick warned {logs:?}"));
    }
    let held_stamps: BTreeSet<(&str, &str)> = was
        .values()
        .flat_map(|entry| &entry.bundles)
        .map(|stamp| (stamp.repo.as_str(), stamp.tag.as_str()))
        .collect();
    let picked: BTreeSet<String> = match member.0 {
        Bundle => {
            let requested = requests(before).remove(member).unwrap_or_default();
            let included = world
                .includes()
                .get(member)
                .and_then(DeclaredSource::identifier)
                .cloned();
            let pinned = [&pins_was, &pins_now].map(|pins| pins.get(member).map(|pin| pin.as_identifier().clone()));
            let ids = requested
                .into_iter()
                .chain(included)
                .chain(pinned.into_iter().flatten());
            // A part keeps no bundle row, so a nested bundle is asked for only by
            // the member list that names it.
            let listed = world.bundle_repositories(&member.1);
            ids.map(|id| id.registry_repository()).chain(listed).collect()
        }
        _ => BTreeSet::new(),
    };
    // A `<plugin>:<bundle>` pick reads the bundle at its tag, and every row its list
    // reaches there carries the digest that read gave, whatever the part held.
    for (key, entry) in now.iter().filter(|_| member.0 == Bundle) {
        for stamp in entry.bundles.iter().filter(|stamp| picked.contains(&stamp.repo)) {
            let (tag, digest) = split_stamp(&stamp.tag);
            let Ok(id) = Identifier::parse(&format!("{}:{tag}", stamp.repo)) else {
                continue;
            };
            let serving = |id: &Identifier| world.store.serving(&id.to_string()).into_iter().collect();
            let roots = BTreeMap::from([(member.clone(), DeclaredSource::Registry(id.clone()))]);
            let below = world.walk(&roots, &pins(after), serving).below;
            let read = world.store.serving(&id.to_string());
            if below.contains_key(key) && digest != read {
                failures.push(format!(
                    "part-stamps: {row}: {}/{} keeps {}:{} after a pick of bundle/{}, whose tag gave {read:?}",
                    key.0, key.1, stamp.repo, stamp.tag, member.1
                ));
            }
        }
    }
    for (key, entry) in &now {
        for stamp in &entry.bundles {
            let (tag, Some(digest)) = split_stamp(&stamp.tag) else {
                continue;
            };
            if held_stamps.contains(&(stamp.repo.as_str(), stamp.tag.as_str())) {
                continue;
            }
            let id = format!("{}:{tag}", stamp.repo);
            // A stamp a held row carries with no digest records one only for the
            // bundle the pick names: on any other, the rows would read two records.
            let bare = held_stamps.contains(&(stamp.repo.as_str(), tag));
            if !closure.contains(key) {
                failures.push(format!(
                    "part-stamps: {row}: {}/{} outside the pick records {id}@{digest}",
                    key.0, key.1
                ));
            } else if bare && !picked.contains(&stamp.repo) {
                failures.push(format!(
                    "part-stamps: {row}: {}/{} records {id}@{digest}, a stamp the part held with no digest",
                    key.0, key.1
                ));
            } else if !world.store.served(id.clone(), digest.clone()) {
                failures.push(format!(
                    "no-false-pair: {row}: part: {}/{} stamps {id} at {digest}, which it never served",
                    key.0, key.1
                ));
            }
        }
    }
    let records = recorded_digests(before).values().any(Option::is_some);
    let marker = format!("plugin '{PLUGIN}' kept");
    let edges = requests(before);
    let declared = world.includes();
    let digest_request = |key: &NodeKey| {
        let line = declared.get(key).and_then(DeclaredSource::identifier);
        line.into_iter()
            .chain(edges.get(key).into_iter().flatten())
            .any(|id| id.digest().is_some())
    };
    let kept_tagless = now.iter().filter(|(key, entry)| {
        let tagless = entry.source.pinned().is_some_and(|pin| pin.tag().is_none());
        tagless && was.get(*key) == Some(*entry) && !digest_request(key)
    });
    for (key, _) in kept_tagless {
        if records && !warned(logs, &marker, key) {
            failures.push(format!(
                "part-warning: {row}: {}/{} kept with no tag, unnamed in {logs:?}",
                key.0, key.1
            ));
        }
    }
    if !records && logs.lines().any(|line| line.contains(&marker)) {
        failures.push(format!(
            "part-warning: {row}: a part that records no digest warned {logs:?}"
        ));
    }
}

/// Every shape's part after each tag move, as this grim, grim 0.15.1, or grim
/// 0.15.1's member update over this grim's part last wrote it, relocked by the
/// export stage and by `grim update --marketplace` of each member it holds and of
/// each a bundle lists that it lacks.
#[tokio::test]
async fn every_part_relock_holds_the_pin_rule() {
    let mut failures = Vec::new();
    let mut scenarios = 0;
    let shapes = SPECS.iter().chain(FUZZ_SPECS).chain(PART_SPECS).chain(LATER_SPECS);
    for spec in shapes.chain(TABLE_SPECS).chain(UNWALKED_SPECS) {
        let mut probe = World::build(spec).await;
        first_part(&mut probe, &format!("{}/part(first)", spec.name), &mut failures).await;
        let Some(part) = probe.part() else { continue };
        let held = pins(part);
        let listed = spec
            .arts
            .iter()
            .filter(|art| art.kind == Bundle)
            .flat_map(|art| art.edges);
        let lacking = listed
            .map(|(kind, name, _)| key_of(*kind, name))
            .filter(|key| key.0 != Bundle && !held.contains_key(key));
        let members: BTreeSet<NodeKey> = held
            .keys()
            .cloned()
            .chain(lacking)
            .chain(bundle_bindings(spec))
            .collect();
        let members: Vec<Option<NodeKey>> = std::iter::once(None).chain(members.into_iter().map(Some)).collect();
        // A part with unwalked edges is one only grim 0.15.1 writes.
        let vintages = match spec.unwalked {
            [] => &[Vintage::Current, Vintage::Old, Vintage::Mixed][..],
            _ => &[Vintage::Old][..],
        };
        for &vintage in vintages {
            for mv in MOVES {
                for member in &members {
                    let mut world = World::build(spec).await;
                    first_part(&mut world, "", &mut failures).await;
                    if let Some(part) = world.market.as_mut().and_then(|lock| lock.plugins.get_mut(PLUGIN)) {
                        *part = aged(part, vintage);
                    }
                    world.move_tags(mv).await;
                    let step = member
                        .as_ref()
                        .map_or_else(|| "export".to_string(), |key| format!("update({})", key.1));
                    let row = format!("{}/{vintage:?}/part-{step}/{mv:?}", spec.name);
                    part_relock_and_check(&mut world, member.as_ref(), &row, &mut failures).await;
                    scenarios += 1;
                }
            }
        }
    }
    assert!(scenarios > 0, "no shape wrote a part");
    report(&failures, scenarios);
}

/// Random edits, tag moves and resolves in any order: the same clauses hold at
/// every relock, however the lock got there.
#[tokio::test]
async fn any_sequence_of_edits_holds_the_pin_rule() {
    let shapes: Vec<&'static Spec> = SPECS.iter().chain(FUZZ_SPECS).collect();
    fuzz_over(&shapes).await;
}

/// The same sequences over the shapes the first fuzz's draw leaves out.
#[tokio::test]
async fn any_sequence_over_later_shapes_holds_the_pin_rule() {
    let shapes: Vec<&'static Spec> = LATER_SPECS.iter().collect();
    fuzz_over(&shapes).await;
}

/// Seeds past the default range whose sequences end in an edit no table row makes.
/// At 1337 and 4426 the edit drops a row `grim lock` keeps, because a key with no row
/// may require it or an undeclared bundle with no row may provide it. At 120 the lock
/// is stale before the edit, so its declaration vouches for no row the edit keeps. At
/// 8575 `grim lock` over the add's line typed by hand refuses another key, a bundle two
/// parents ask for at two tags, which `held-pin` must not count against the add. At
/// 143682 the edit takes a bundle's line while a key whose edges only the next relock
/// reads may require the bundle, so its own row stays. At 1327528 an add follows a line
/// deleted by hand, whose row stays in the stale lock: that row's edge vouches for no pin.
#[tokio::test]
async fn sequences_past_the_default_seeds_hold_the_pin_rule() {
    let shapes: Vec<&'static Spec> = SPECS.iter().chain(FUZZ_SPECS).collect();
    replay(&shapes, [1337, 4426, 8575, 143682, 1327528]).await;
    let later: Vec<&'static Spec> = LATER_SPECS.iter().collect();
    replay(&later, [120]).await;
}

async fn fuzz_over(shapes: &[&'static Spec]) {
    let seeds = fuzz_seeds();
    assert!(
        !seeds.is_empty(),
        "no seeds in {seeds:?}: an inverted range tests nothing"
    );
    replay(shapes, seeds).await;
}

/// Each of `seeds`' sequence over `shapes`, then a relock.
async fn replay(shapes: &[&'static Spec], seeds: impl IntoIterator<Item = u64>) {
    let mut failures = Vec::new();
    let mut scenarios = 0;
    for seed in seeds {
        scenarios += 1;
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let spec = shapes[rng.below(shapes.len())];
        let mut world = World::build(spec).await;
        let mut trail: Vec<String> = Vec::new();
        for _ in 0..=rng.below(8) {
            let so_far = trail.clone();
            let row = |step: &str| format!("seed {seed} {}/{so_far:?}/{step}", spec.name);
            let step = if rng.below(3) == 0 {
                round_trip_step(&mut world, &mut rng, row, &mut failures).await
            } else {
                fuzz_step(&mut world, &mut rng, row, &mut failures).await
            };
            trail.push(step);
        }
        let name = format!("seed {seed} {}/{trail:?}", spec.name);
        check_relock(&mut world, &name, &mut failures).await;
    }
    report(&failures, scenarios);
}

const Y1: &str = "ghcr.io/acme/y:1";

/// grim 0.15.1's lock of `p`, which requires `x`, and `q`, which requires `y`, each
/// beside its requirement's own line: no row records an edge.
static MIXED_SPEC: Spec = Spec {
    name: "two-0.15.1-dependents",
    arts: &[
        X1_SAME,
        art(Skill, Y1, "y", &[]),
        P_X1,
        art(Skill, "ghcr.io/acme/q:1", "q", &[(Skill, "y", Y1)]),
        BYSTANDER,
    ],
    declare: &[
        DIRECT_X1,
        (Skill, "y", Y1),
        DECLARE_P,
        (Skill, "q", "ghcr.io/acme/q:1"),
        DECLARE_B,
    ],
    tags: &[X1],
    pre_change: true,
    no_requires: true,
    warm: true,
    ..BASE
};

/// Point the world's store at one holding every manifest and blob but the requires
/// layers of `unread`'s rows, as a home where another project read only the others.
fn evict_requires(world: &mut World, unread: &[&str]) {
    let lock = world.lock();
    let mut evicted = BTreeSet::new();
    for row in lock.iter_artifacts().filter(|row| unread.contains(&row.name.as_str())) {
        let digest = row.source.pinned().unwrap().digest();
        let bytes = world.store.manifests.lock().unwrap()[&digest].0.clone();
        let manifest = OciManifest::from_json(&bytes).unwrap();
        if let Ok(crate::oci::manifest::LayerLayout::ContentWithRequires { requires, .. }) = manifest.layout(row.kind) {
            evicted.insert(requires.digest.clone());
        }
    }
    let home = crate::store::GrimPaths::new(world.dir.path().join("mixed-home"));
    home.ensure_layout().unwrap();
    let blobs = crate::store::BlobStore::new(home.blobs_dir());
    for (digest, (bytes, _)) in world.store.manifests.lock().unwrap().iter() {
        blobs.put(digest, bytes).unwrap();
    }
    for (digest, bytes) in world.store.blobs.lock().unwrap().iter() {
        if !evicted.contains(digest) {
            blobs.put(digest, bytes).unwrap();
        }
    }
    let tags = crate::oci::tag_cache::TagCache::new(home.tags_dir());
    for (reference, digest) in world.store.serving.lock().unwrap().iter() {
        tags.put(&parse(reference), digest).unwrap();
    }
    world.home = Some(home);
}

/// Store every blob the registry served in the world's home, as another project's
/// `grim add` stores the requires layers an earlier read lacked.
fn restore_requires(world: &World) {
    let blobs = crate::store::BlobStore::new(world.home.as_ref().unwrap().blobs_dir());
    for (digest, bytes) in world.store.blobs.lock().unwrap().iter() {
        blobs.put(digest, bytes).unwrap();
    }
}

/// **mixed-store**: an offline `grim lock` over a store that holds some dependents'
/// requires layers and not others records edges for the first alone, so the lock it
/// writes has dependency data beside a row whose edges stay unread. Each edit of that
/// lock, then an online relock after a tag move, holds the pin rule: a row only such a
/// dependent requires drops, as on 0.15.1, whether the store holds that
/// dependent's layer since (`restored`) or not, and the edit names the dependent.
#[tokio::test]
async fn an_edit_after_an_offline_relock_over_a_mixed_store_holds_the_pin_rule() {
    let mut failures = Vec::new();
    let mut scenarios = 0;
    let unread_sets: [&[&str]; 4] = [&[], &["p"], &["q"], &["p", "q"]];
    let variants = unread_sets
        .into_iter()
        .flat_map(|unread| [(unread, false), (unread, true)])
        .filter(|(unread, restored)| !unread.is_empty() || !restored);
    for edits in edits_of(&MIXED_SPEC) {
        for (unread, restored) in variants.clone() {
            for mv in MOVES {
                let name = format!(
                    "{}/unread={unread:?}/restored={restored}",
                    row_name(&MIXED_SPEC, &edits, mv)
                );
                let mut world = World::build(&MIXED_SPEC).await;
                evict_requires(&mut world, unread);
                let previous = world.lock();
                let paths = world.home.clone().unwrap();
                let cold: Arc<dyn OciAccess> = Arc::new(crate::oci::access::cached_access::CachedAccess::new(
                    world.store.clone(),
                    crate::oci::tag_cache::TagCache::new(paths.tags_dir()),
                    crate::store::BlobStore::new(paths.blobs_dir()),
                    crate::oci::access::AccessMode::Offline,
                ));
                let prior = Prior::checked(&previous, &world.set);
                let (set, dir) = (&world.set, world.dir.path());
                let run = LockRun::project(true);
                let relocked = target_lock(set, Some(&prior), &Refresh::Nothing, &cold, &options(), dir, run)
                    .await
                    .expect("every pin is carried and every manifest stored")
                    .lock;
                for row in relocked
                    .iter_artifacts()
                    .filter(|row| ["p", "q"].contains(&row.name.as_str()))
                {
                    if row.requires.is_empty() != unread.contains(&row.name.as_str()) {
                        failures.push(format!("mixed-store: {name}: {} records {:?}", row.name, row.requires));
                    }
                }
                lock_io::save(&world.lock_path, &relocked, Some(&previous)).unwrap();
                if restored {
                    restore_requires(&world);
                }
                for (edit, key) in &edits {
                    edit_and_check(&mut world, *edit, key, &name, &mut failures).await;
                }
                world.move_tags(mv).await;
                check_relock(&mut world, &name, &mut failures).await;
                scenarios += 1;
            }
        }
    }
    report(&failures, scenarios);
}

/// grim 0.15.1's lock of `pack`, whose list names `held:1`, beside `x:1`, which
/// requires `held:2` at the same bytes: an edge that lock never walked, which agrees
/// with the member's tagless pin until a tag moves.
static UNWALKED_AGREEING_SPEC: Spec = Spec {
    name: "a-0.15.1-dependent-asks-another-tag-at-a-members-pin",
    arts: &[
        art(Skill, HELD1, "held", &[]),
        art(Skill, HELD2, "held", &[]),
        art(Bundle, PACK1, "pack", &[(Skill, "held", HELD1)]),
        X1_SAME,
    ],
    declare: &[(Bundle, "pack", PACK1), DIRECT_X1],
    unwalked: &[(X1, (Skill, "held", HELD2))],
    key: (Skill, "held"),
    tags: &[HELD1, HELD2],
    pre_change: true,
    no_requires: true,
    ..BASE
};

/// **held-pin** beside an unwalked edge onto a member's pin: after every tag move,
/// `grim lock`, then `grim add` of each tag of `held`, which writes the pin `grim lock`
/// keeps before the line is typed. Found by review: with `held:1` moved, the add read
/// only its line and moved the pin, where `grim lock` resolved `held:2` onto it.
#[tokio::test]
async fn an_add_beside_an_unwalked_edge_onto_its_pin_keeps_what_grim_lock_keeps() {
    let mut failures = Vec::new();
    let mut scenarios = 0;
    let spec = &UNWALKED_AGREEING_SPEC;
    let key = key_of(Skill, "held");
    for mv in MOVES {
        let mut world = World::build(spec).await;
        world.move_tags(mv).await;
        check_relock(&mut world, &format!("{}/{mv:?}/relock", spec.name), &mut failures).await;
        scenarios += 1;
        for line in [HELD1, HELD2] {
            let mut world = World::build(spec).await;
            world.move_tags(mv).await;
            let row = format!("{}/{mv:?}/add({line})", spec.name);
            add_and_check(&mut world, &key, parse(line), &row, &mut failures).await;
            scenarios += 1;
        }
    }
    report(&failures, scenarios);
}
