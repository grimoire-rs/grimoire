// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! What the local store shows of a bundle the lock holds no member list for.
//!
//! An edit runs offline, and the lock cannot tell such a bundle's member from a
//! stranger: grim stamps no bundle on a declared row. What an earlier resolve left
//! in the store is the only evidence left. A read that cannot finish says nothing,
//! and the caller treats that bundle as one that may provide any key.

use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::rc::Rc;

use crate::config::declaration::DesiredSet;
use crate::config::scope::Invocation;
use crate::lock::effective_set::{NodeKey, bundles_without_members, declared_parent_provides};
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::locked_artifact::{BundleProvenance, LockedArtifact};
use crate::lock::pin_rule::bundle_provenance_tag;
use crate::oci::bundle::{BundleManifest, BundleMember};
use crate::oci::manifest::{LayerLayout, OciManifest};
use crate::oci::tag_cache::TagCache;
use crate::oci::{ArtifactKind, Digest, Identifier, PinnedIdentifier};
use crate::resolve::resolve_error::MAX_TRANSITIVE_NODES;
use crate::store::{BlobStore, GrimPaths};

/// What the local store shows that the lock cannot: the closures of the bundles a
/// lock has no list for, by binding, and the rows whose stored manifest names
/// requires the lock records no edge of, each with what the store shows those
/// requires reach. A bundle absent here is one the store cannot speak for.
#[derive(Debug, Default)]
pub(crate) struct StoredMembers {
    closures: BTreeMap<String, BTreeSet<NodeKey>>,
    /// The store and the lock the reaches below are read from, on the first ask.
    walked: Option<(GrimPaths, GrimoireLock)>,
    unrecorded_requires: OnceCell<BTreeMap<NodeKey, StoredReach>>,
    /// The config the edit changed and the lock beside it, as its recovery names them.
    restore: Option<(String, String)>,
}

/// What the store shows a row's requires reach, where the row records no edge.
#[derive(Debug, Default)]
pub(crate) struct StoredReach {
    /// Every key the store shows they reach, read at the lock's pins.
    keys: BTreeSet<NodeKey>,
    /// Whether the store held every link, so they reach nothing else.
    whole: bool,
}

impl StoredReach {
    /// Whether the store shows they reach `key`.
    pub(crate) fn shows(&self, key: &NodeKey) -> bool {
        self.keys.contains(key)
    }

    /// Whether they may reach `key`: shown, or past a link the store lacks.
    pub(crate) fn may_reach(&self, key: &NodeKey) -> bool {
        self.shows(key) || !self.whole
    }
}

impl StoredMembers {
    /// The store's closure of every bundle `set` declares and `lock` holds no list
    /// for, and the rows that record no edge whose stored manifest
    /// [names requires](StoreView::names_requires). Judged per row: one row's
    /// recorded edges say nothing of another's. A row that records an edge, or holds
    /// a path pin, costs no read, and the rows are read on the first
    /// [`Self::unrecorded_reach`], so an edit that asks none reads none.
    pub(crate) fn read(paths: &GrimPaths, set: &DesiredSet, lock: &GrimoireLock) -> Self {
        Self {
            closures: StoreView::new(paths, lock).closures(set),
            walked: Some((paths.clone(), lock.clone())),
            ..Self::default()
        }
    }

    /// The same view, for an edit of the config at `config` and the lock at `lock`,
    /// which [`Self::restore_files`] then names. A file name tells a pair only where
    /// the shell stands beside it: a global run's is under `$GRIM_HOME`, and a walked
    /// project's is bare when it is in the working directory. Any other pair, a
    /// `--config` run's or a walked project found in an ancestor directory, is named
    /// by absolute path: the same names there are another pair.
    pub(crate) fn restoring(self, config: &Path, lock: &Path, grim: &Invocation) -> Self {
        let cwd = std::env::current_dir().ok();
        let named = |path: &Path| {
            let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
            let beside_shell = *grim == Invocation::WALKED && absolute.parent() == cwd.as_deref();
            if beside_shell || grim.is_global() {
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .into_owned()
            } else {
                absolute.to_string_lossy().into_owned()
            }
        };
        Self {
            restore: Some((named(config), named(lock))),
            ..self
        }
    }

    /// The config the edit changed and the lock beside it, which its recovery
    /// restores: `grimoire.toml` and `grimoire.lock` unless the edit named them.
    pub(crate) fn restore_files(&self) -> (&str, &str) {
        match &self.restore {
            Some((config, lock)) => (config, lock),
            None => ("grimoire.toml", "grimoire.lock"),
        }
    }

    /// Whether the store shows that `binding` does not provide `key`.
    pub(crate) fn rules_out(&self, binding: &str, key: &NodeKey) -> bool {
        self.closures.get(binding).is_some_and(|closure| !closure.contains(key))
    }

    /// Whether the store shows that `binding` provides `key`.
    pub(crate) fn shows_provided(&self, binding: &str, key: &NodeKey) -> bool {
        self.closures.get(binding).is_some_and(|closure| closure.contains(key))
    }

    /// Whether the store holds `binding`'s whole closure, so it can say what the
    /// bundle provides.
    pub(crate) fn knows(&self, binding: &str) -> bool {
        self.closures.contains_key(binding)
    }

    /// What the store shows the requires of `key`'s row reach, where that row records
    /// no edge while its stored manifest names requires, as on a row grim 0.15.1
    /// locked: a dependent the lock never walked, which the edit names beside a row
    /// it drops. `None` for any other row.
    pub(crate) fn unrecorded_reach(&self, key: &NodeKey) -> Option<&StoredReach> {
        let read = || match &self.walked {
            Some((paths, lock)) => StoreView::new(paths, lock).unrecorded_requires(),
            None => BTreeMap::new(),
        };
        self.unrecorded_requires.get_or_init(read).get(key)
    }
}

/// Whether a delete must keep `key`'s files because something declared still
/// provides it: [`declared_parent_provides`], plus what the store shows of a bundle
/// the lock has no list for, or that bundle's stamp on `key`'s row. The lock edit
/// keeps a row on the same evidence, so files deleted beside a kept row would
/// come back at the next `grim install`. A store that cannot speak for an
/// unstamped row keeps nothing, as a lock with no list did before.
pub(crate) fn parent_provides(paths: &GrimPaths, set: &DesiredSet, lock: &GrimoireLock, key: &NodeKey) -> bool {
    declared_parent_provides(set, lock, key)
        || bundles_without_members(set, lock).any(|binding| stamped_by(set, lock, binding, key))
        || StoreView::new(paths, lock)
            .closures(set)
            .values()
            .any(|closure| closure.contains(key))
}

/// The stamp a relock writes on a row `binding`'s declaration provides. `None`
/// for a path bundle, whose stamp only its lock row records.
pub(crate) fn declared_stamp(set: &DesiredSet, binding: &str) -> Option<BundleProvenance> {
    let id = set.bundles.get(binding)?.identifier()?;
    Some(BundleProvenance::new(
        id.registry_repository(),
        bundle_provenance_tag(id),
    ))
}

/// Whether `key`'s row carries the stamp `binding`'s declaration writes, so that
/// bundle provided it when the lock was written.
pub(crate) fn stamped_by(set: &DesiredSet, lock: &GrimoireLock, binding: &str, key: &NodeKey) -> bool {
    let Some(stamp) = declared_stamp(set, binding) else {
        return false;
    };
    lock.iter_artifacts()
        .find(|row| row.kind == key.0 && row.name == key.1)
        .is_some_and(|row| row.bundles.contains(&stamp))
}

/// Whether `closure` holds every row `lock` stamps with the bundle `id`. A list
/// lacking one is not the list this lock was written from: the tag moved since,
/// so what it leaves out says nothing of what the lock's bundle provided.
fn holds_stamped_rows(lock: &GrimoireLock, id: &Identifier, closure: &BTreeSet<NodeKey>) -> bool {
    let (repo, tag) = (id.registry_repository(), id.tag_or_latest());
    lock.iter_artifacts()
        .filter(|row| row.bundles.iter().any(|b| b.repo == repo && b.tag == tag))
        .all(|row| closure.contains(&(row.kind, row.name.clone())))
}

/// How a walk reads a member the lock holds no pin for.
#[derive(Clone, Copy)]
enum Unpinned {
    /// At the digest the local tag cache names, as a bundle's closure is read.
    CachedTag,
    /// Not at all, as a link the store lacks: the next online `grim lock` reads its
    /// tag from the registry, and a cached tag may name an older manifest.
    Unread,
}

/// The members or requires a stored manifest carries; `None` where the store
/// cannot read them.
type Edges = Option<Rc<[BundleMember]>>;

struct StoreView<'l> {
    tags: TagCache,
    blobs: BlobStore,
    lock: &'l GrimoireLock,
    /// Each key's digest in the lock, from its first row, as `locked_digest` reads it.
    pins: BTreeMap<NodeKey, Option<Digest>>,
    /// The members or requires each manifest read so far carries, by kind and digest.
    members: RefCell<BTreeMap<(ArtifactKind, Digest), Edges>>,
}

impl<'l> StoreView<'l> {
    fn new(paths: &GrimPaths, lock: &'l GrimoireLock) -> Self {
        let bundles = lock
            .bundles
            .iter()
            .map(|b| ((ArtifactKind::Bundle, b.name.clone()), b.pinned()));
        let rows = lock
            .iter_artifacts()
            .map(|row| ((row.kind, row.name.clone()), row.source.pinned()));
        let mut pins = BTreeMap::new();
        for (key, pin) in bundles.chain(rows) {
            pins.entry(key).or_insert_with(|| pin.map(PinnedIdentifier::digest));
        }
        Self {
            tags: TagCache::new(paths.tags_dir()),
            blobs: BlobStore::new(paths.blobs_dir()),
            lock,
            pins,
            members: RefCell::default(),
        }
    }

    /// What the stored requires of each row that records no edge and
    /// [names requires](Self::names_requires) reach, by row.
    fn unrecorded_requires(&self) -> BTreeMap<NodeKey, StoredReach> {
        self.lock
            .iter_artifacts()
            .filter(|row| self.names_requires(row))
            .map(|row| ((row.kind, row.name.clone()), self.requester_reach(row)))
            .collect()
    }

    /// The store's closure of every bundle `set` declares and the lock holds no
    /// list for, by binding.
    fn closures(&self, set: &DesiredSet) -> BTreeMap<String, BTreeSet<NodeKey>> {
        bundles_without_members(set, self.lock)
            .filter_map(|binding| {
                let id = set.bundles.get(binding)?.identifier()?;
                let closure = self
                    .closure(id)
                    .filter(|closure| holds_stamped_rows(self.lock, id, closure))?;
                Some((binding.to_string(), closure))
            })
            .collect()
    }

    /// Whether a registry row that records no edge has a stored manifest that names
    /// requires the row does not record, so the edit names them beside a row it drops.
    /// That is a layer the store lacks, as 0.15.1's `add` and `grim fetch` leave it, or
    /// one listing edges the lock does not record, as another project's `grim add`
    /// leaves it. A manifest this grim cannot read, or with no layer or an empty one,
    /// names none, and the edit reads the row as a leaf, as it does on a lock 0.15.1
    /// wrote: a row only it requires drops, and the next `grim lock` resolves it afresh.
    /// Kept for it instead, a row left the lock stale and changed `grim install` and
    /// `grim update` exits against 0.15.1.
    fn names_requires(&self, row: &LockedArtifact) -> bool {
        let Some(pin) = row.source.pinned().filter(|_| row.requires.is_empty()) else {
            return false;
        };
        let manifest = self
            .blob(&pin.digest())
            .and_then(|bytes| OciManifest::from_json(&bytes).ok());
        let layout = manifest.as_ref().map(|manifest| manifest.layout(row.kind));
        let Some(Ok(LayerLayout::ContentWithRequires { requires, .. })) = layout else {
            return false;
        };
        let layer = self
            .blob(&requires.digest)
            .and_then(|bytes| BundleManifest::from_layer_bytes(&bytes).ok());
        layer.is_none_or(|layer| !layer.members.is_empty())
    }

    /// Every key `root` reaches through members and requires, as a relock would
    /// read them; `None` when the store lacks any link. A member is read at its
    /// lock pin first, the version a relock keeps.
    fn closure(&self, root: &Identifier) -> Option<BTreeSet<NodeKey>> {
        let root_digest = self.tags.get(root).ok()??;
        let reach = self.reach(ArtifactKind::Bundle, root.clone(), root_digest, Unpinned::CachedTag);
        reach.whole.then_some(reach.keys)
    }

    /// What `row`'s requires reach at the lock's pins alone.
    fn requester_reach(&self, row: &LockedArtifact) -> StoredReach {
        let Some(pin) = row.source.pinned() else {
            return StoredReach::default();
        };
        self.reach(row.kind, pin.as_identifier().clone(), pin.digest(), Unpinned::Unread)
    }

    /// What the manifest of `kind` at `digest`, named `id`, reaches.
    fn reach(&self, kind: ArtifactKind, id: Identifier, digest: Digest, unpinned: Unpinned) -> StoredReach {
        let (mut keys, mut read, mut whole) = (BTreeSet::new(), BTreeSet::new(), true);
        let mut frontier = vec![(kind, id, digest)];
        while let Some((kind, id, digest)) = frontier.pop() {
            if !read.insert(digest.clone()) {
                continue;
            }
            if read.len() > MAX_TRANSITIVE_NODES {
                return StoredReach { keys, whole: false };
            }
            let Some(members) = self.edges(kind, &digest) else {
                whole = false;
                continue;
            };
            for member in members.iter() {
                let key = (member.kind, member.name.clone());
                let next = member.absolute_id(Some(&id)).and_then(|member_id| {
                    let cached = || match unpinned {
                        Unpinned::CachedTag => self.tags.get(&member_id).ok().flatten(),
                        Unpinned::Unread => None,
                    };
                    let pin = self.pins.get(&key).cloned().flatten().or_else(cached)?;
                    Some((member.kind, member_id, pin))
                });
                keys.insert(key);
                match next {
                    Some(next) => frontier.push(next),
                    None => whole = false,
                }
            }
        }
        StoredReach { keys, whole }
    }

    /// The members or requires the manifest at `digest` carries, read once per view.
    fn edges(&self, kind: ArtifactKind, digest: &Digest) -> Edges {
        let key = (kind, digest.clone());
        if let Some(members) = self.members.borrow().get(&key) {
            return members.clone();
        }
        let members = self.read_edges(kind, digest).map(Rc::from);
        self.members.borrow_mut().insert(key, members.clone());
        members
    }

    fn read_edges(&self, kind: ArtifactKind, digest: &Digest) -> Option<Vec<BundleMember>> {
        let manifest = OciManifest::from_json(&self.blob(digest)?).ok()?;
        let layer = match manifest.layout(kind).ok()? {
            LayerLayout::Content(_) => return Some(Vec::new()),
            LayerLayout::BundleMembers(layer) | LayerLayout::ContentWithRequires { requires: layer, .. } => layer,
        };
        Some(
            BundleManifest::from_layer_bytes(&self.blob(&layer.digest)?)
                .ok()?
                .members,
        )
    }

    /// The store trusts its file names, so a blob counts only if it re-hashes to
    /// its digest: a damaged list would otherwise rule a member out.
    fn blob(&self, digest: &Digest) -> Option<Vec<u8>> {
        #[cfg(test)]
        BLOB_READS.with(|reads| reads.set(reads.get() + 1));
        let bytes = self.blobs.get(digest).ok()??;
        (digest.algorithm().hash(&bytes) == *digest).then_some(bytes)
    }
}

#[cfg(test)]
thread_local! {
    /// The blobs this test thread's store views read, for the tests that bound them.
    static BLOB_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
impl StoredMembers {
    /// A store holding `binding`'s closure as `keys`.
    pub(crate) fn holding(binding: &str, keys: &[NodeKey]) -> Self {
        let closures = BTreeMap::from([(binding.to_string(), keys.iter().cloned().collect())]);
        Self {
            closures,
            ..Self::default()
        }
    }

    /// A store whose manifests show `keys` require artifacts their rows record no
    /// edge onto, through a link the store lacks.
    pub(crate) fn requiring(keys: &[NodeKey]) -> Self {
        let reaches: BTreeMap<_, _> = keys.iter().map(|key| (key.clone(), StoredReach::default())).collect();
        Self {
            unrecorded_requires: OnceCell::from(reaches),
            ..Self::default()
        }
    }

    /// A store whose manifests show `asker` requires `reached` and nothing else,
    /// onto which its row records no edge.
    pub(crate) fn reaching(asker: &NodeKey, reached: &[NodeKey]) -> Self {
        let reach = StoredReach {
            keys: reached.iter().cloned().collect(),
            whole: true,
        };
        Self {
            unrecorded_requires: OnceCell::from(BTreeMap::from([(asker.clone(), reach)])),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::command::uninstall::EditTarget;
    use crate::config::scope::Invocation;
    use crate::lock::edit::{drop_from_lock, has_dependency_data};
    use crate::lock::locked_artifact::{BundleProvenance, LockedArtifact};
    use crate::lock::locked_bundle::{LockedBundle, LockedBundleSource};
    use crate::lock::test_support::{declared, lock_of};
    use crate::oci::Algorithm;
    use crate::oci::PinnedIdentifier;
    use crate::oci::bundle::BUNDLE_LAYER_MEDIA_TYPE;

    const PACK: &str = "localhost:5000/acme/pack:1";
    const TAR: &str = "application/vnd.grimoire.artifact.layer.v1.tar";

    /// A `GRIM_HOME` the test fills by hand, as a resolve would.
    struct Home {
        _dir: tempfile::TempDir,
        paths: GrimPaths,
    }

    impl Home {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let paths = GrimPaths::new(dir.path().join("home"));
            paths.ensure_layout().unwrap();
            Self { _dir: dir, paths }
        }

        fn blobs(&self) -> BlobStore {
            BlobStore::new(self.paths.blobs_dir())
        }

        fn put(&self, bytes: &[u8]) -> Digest {
            let digest = Algorithm::Sha256.hash(bytes);
            self.blobs().put(&digest, bytes).unwrap();
            digest
        }

        /// Store a manifest over `layers` and, when given, point `tag_at` at it.
        fn publish(&self, tag_at: Option<&str>, layers: Vec<(&str, Vec<u8>)>) -> Digest {
            let layers: Vec<_> = layers
                .into_iter()
                .map(|(media_type, bytes)| {
                    serde_json::json!({
                        "mediaType": media_type,
                        "digest": self.put(&bytes).to_string(),
                        "size": bytes.len(),
                    })
                })
                .collect();
            let manifest = serde_json::json!({
                "schemaVersion": 2,
                "config": {
                    "mediaType": "application/vnd.oci.empty.v1+json",
                    "digest": format!("sha256:{}", "4".repeat(64)),
                    "size": 2,
                },
                "layers": layers,
            });
            let digest = self.put(&serde_json::to_vec(&manifest).unwrap());
            if let Some(id) = tag_at {
                TagCache::new(self.paths.tags_dir())
                    .put(&Identifier::parse(id).unwrap(), &digest)
                    .unwrap();
            }
            digest
        }

        fn leaf(&self, id: &str) -> Digest {
            self.publish(Some(id), vec![(TAR, id.as_bytes().to_vec())])
        }

        fn bundle(&self, id: &str, members: &[(ArtifactKind, &str, &str)]) -> Digest {
            let members = members
                .iter()
                .map(|(kind, name, id)| BundleMember {
                    kind: *kind,
                    name: (*name).to_string(),
                    id: (*id).to_string(),
                })
                .collect();
            let doc = BundleManifest::new(members).to_layer_bytes().unwrap();
            self.publish(Some(id), vec![(BUNDLE_LAYER_MEDIA_TYPE, doc)])
        }
    }

    fn pack_declared() -> DesiredSet {
        let mut set = declared(&[]);
        let pack = crate::config::declaration::DeclaredSource::Registry(Identifier::parse(PACK).unwrap());
        set.bundles.insert("pack".to_string(), pack);
        set.invalidate_declaration_hash_cache();
        set
    }

    fn skill(name: &str) -> NodeKey {
        (ArtifactKind::Skill, name.to_string())
    }

    fn read(home: &Home, lock: &GrimoireLock) -> StoredMembers {
        StoredMembers::read(&home.paths, &pack_declared(), lock)
    }

    #[test]
    fn a_stored_list_rules_out_what_it_does_not_name() {
        let home = Home::new();
        home.leaf("localhost:5000/acme/m:1");
        home.bundle(PACK, &[(ArtifactKind::Skill, "m", "localhost:5000/acme/m:1")]);

        let stored = read(&home, &lock_of(&[], ""));

        assert!(stored.rules_out("pack", &skill("y")));
        assert!(!stored.rules_out("pack", &skill("m")));
    }

    /// Cold, the bundle may provide anything: no tag, no manifest, or a member the
    /// store never saw, which may itself require the key.
    #[test]
    fn a_store_that_lacks_a_link_rules_out_nothing() {
        let lock = lock_of(&[], "");
        assert!(!read(&Home::new(), &lock).rules_out("pack", &skill("y")));

        let home = Home::new();
        let digest = home.bundle(PACK, &[]);
        std::fs::remove_file(walk_files(&home.paths.blobs_dir(), &digest)).unwrap();
        assert!(!read(&home, &lock).rules_out("pack", &skill("y")));

        let home = Home::new();
        home.bundle(PACK, &[(ArtifactKind::Skill, "m", "localhost:5000/acme/m:1")]);
        assert!(!read(&home, &lock).rules_out("pack", &skill("y")));
    }

    fn walk_files(blobs: &std::path::Path, digest: &Digest) -> std::path::PathBuf {
        let hex = digest.to_string();
        let hex = hex.rsplit(':').next().unwrap();
        let mut found = Vec::new();
        let mut dirs = vec![blobs.to_path_buf()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.to_string_lossy().contains(hex) {
                    found.push(path);
                }
            }
        }
        found.pop().unwrap()
    }

    /// With no list in the lock, the store is what shows a delete must keep the files
    /// of a key the bundle provides, and only of such a key.
    #[test]
    fn a_stored_list_holds_the_files_of_what_it_names() {
        let lock = lock_of(&[], "");
        let held = |paths: &GrimPaths, name| parent_provides(paths, &pack_declared(), &lock, &skill(name));
        let home = Home::new();
        home.leaf("localhost:5000/acme/m:1");
        home.bundle(PACK, &[(ArtifactKind::Skill, "m", "localhost:5000/acme/m:1")]);

        assert!(held(&home.paths, "m"));
        assert!(!held(&home.paths, "y"));
        assert!(!held(&Home::new().paths, "m"), "a cold store holds nothing");
    }

    /// A row the lock stamps with the bundle is one that bundle provided, which the
    /// lock edit keeps on a cold store too. Found by review: the files went while
    /// the row stayed, so the next `grim install` brought them back.
    #[test]
    fn a_stamped_row_holds_its_files_on_a_cold_store() {
        let mut lock = lock_of(&[], "");
        let mut x = row_at("x", Algorithm::Sha256.hash(b"x"));
        x.bundles = vec![BundleProvenance::new("localhost:5000/acme/pack", "1")];
        lock.skills = vec![x, row_at("y", Algorithm::Sha256.hash(b"y"))];
        let held = |name| parent_provides(&Home::new().paths, &pack_declared(), &lock, &skill(name));

        assert!(held("x"));
        assert!(!held("y"), "an unstamped row on a cold store holds nothing");
    }

    /// The store trusts its file names, and a damaged list naming nothing would
    /// rule out a real member.
    #[test]
    fn a_damaged_list_rules_out_nothing() {
        let home = Home::new();
        let digest = home.bundle(PACK, &[]);
        std::fs::write(walk_files(&home.paths.blobs_dir(), &digest), b"{}").unwrap();

        assert!(!read(&home, &lock_of(&[], "")).rules_out("pack", &skill("y")));
    }

    /// A member's requires and a nested bundle's members reach a key as surely as a
    /// flat member does, and a relock resolves them.
    #[test]
    fn the_closure_follows_requires_and_nested_bundles() {
        const M: &str = "localhost:5000/acme/m:1";
        const R: &str = "localhost:5000/acme/r:1";
        const INNER: &str = "localhost:5000/acme/inner:1";
        const N: &str = "localhost:5000/acme/n:1";
        let home = Home::new();
        home.leaf(R);
        home.leaf(N);
        let requires = BundleManifest::new(vec![BundleMember {
            kind: ArtifactKind::Skill,
            name: "r".to_string(),
            id: R.to_string(),
        }]);
        let requires = requires.to_layer_bytes().unwrap();
        home.publish(Some(M), vec![(TAR, b"m".to_vec()), (BUNDLE_LAYER_MEDIA_TYPE, requires)]);
        home.bundle(INNER, &[(ArtifactKind::Skill, "n", N)]);
        home.bundle(
            PACK,
            &[(ArtifactKind::Skill, "m", M), (ArtifactKind::Bundle, "inner", INNER)],
        );

        let stored = read(&home, &lock_of(&[], ""));

        for key in [
            skill("m"),
            skill("r"),
            skill("n"),
            (ArtifactKind::Bundle, "inner".to_string()),
        ] {
            assert!(!stored.rules_out("pack", &key), "{key:?}");
        }
        assert!(stored.rules_out("pack", &skill("y")));
    }

    /// A cycle ends the walk, and the keys on it are the closure.
    #[test]
    fn a_requires_cycle_ends() {
        let home = Home::new();
        home.bundle(PACK, &[(ArtifactKind::Bundle, "pack", PACK)]);

        let stored = read(&home, &lock_of(&[], ""));

        assert!(stored.rules_out("pack", &skill("localhost:5000/acme/a:1")));
    }

    /// The lock's pin is the version a relock keeps, and the store may hold no tag
    /// for it, since a relock that carries a pin resolves none.
    #[test]
    fn a_member_is_read_at_its_lock_pin() {
        const M: &str = "localhost:5000/acme/m:1";
        let home = Home::new();
        let digest = home.publish(None, vec![(TAR, b"m".to_vec())]);
        home.bundle(PACK, &[(ArtifactKind::Skill, "m", M)]);
        let id = Identifier::parse(M).unwrap().clone_with_digest(digest);
        let row = LockedArtifact::direct(
            "m".to_string(),
            ArtifactKind::Skill,
            PinnedIdentifier::try_from(id).unwrap(),
        );
        let mut lock = lock_of(&[], "");
        lock.skills.push(row);

        assert!(read(&home, &lock).rules_out("pack", &skill("y")));
    }

    /// A row the lock stamps with this bundle that the stored list lacks shows the
    /// list moved on from the one the lock was written from.
    #[test]
    fn a_list_that_lacks_a_stamped_row_rules_out_nothing() {
        const M: &str = "localhost:5000/acme/m:1";
        let home = Home::new();
        let digest = home.publish(None, vec![(TAR, b"m".to_vec())]);
        home.bundle(PACK, &[]);
        let id = Identifier::parse(M).unwrap().clone_with_digest(digest);
        let mut row = LockedArtifact::direct(
            "m".to_string(),
            ArtifactKind::Skill,
            PinnedIdentifier::try_from(id).unwrap(),
        );
        row.bundles = vec![BundleProvenance::new("localhost:5000/acme/pack", "1")];
        let mut lock = lock_of(&[], "");
        lock.skills.push(row);

        assert!(!read(&home, &lock).rules_out("pack", &skill("y")));
    }

    /// A bundle the lock holds a list for needs no read: that list is the proof's.
    #[test]
    fn a_bundle_the_lock_lists_is_not_read() {
        const P: &str = "localhost:5000/acme/pack";
        let home = Home::new();
        let digest = home.bundle(PACK, &[]);
        let id = Identifier::parse(PACK).unwrap().clone_with_digest(digest);
        let mut lock = lock_of(&[], "");
        lock.bundles.push(LockedBundle {
            name: "pack".to_string(),
            source: LockedBundleSource::Registry {
                repo: P.to_string(),
                tag: "1".to_string(),
                pinned: PinnedIdentifier::try_from(id).unwrap(),
            },
            members: Vec::new(),
            transitive: false,
        });

        assert!(!read(&home, &lock).rules_out("pack", &skill("y")));
    }

    /// A row at `digest`, recording no edge, as 0.15.1 locks every row.
    fn row_at(name: &str, digest: Digest) -> LockedArtifact {
        let id = Identifier::parse(&format!("localhost:5000/acme/{name}:1"))
            .unwrap()
            .clone_with_digest(digest);
        LockedArtifact::direct(
            name.to_string(),
            ArtifactKind::Skill,
            PinnedIdentifier::try_from(id).unwrap(),
        )
    }

    fn requires_doc(members: &[(ArtifactKind, &str, &str)]) -> Vec<u8> {
        let members = members
            .iter()
            .map(|(kind, name, id)| BundleMember {
                kind: *kind,
                name: (*name).to_string(),
                id: (*id).to_string(),
            })
            .collect();
        BundleManifest::new(members).to_layer_bytes().unwrap()
    }

    fn forget(home: &Home, bytes: &[u8]) {
        std::fs::remove_file(walk_files(&home.paths.blobs_dir(), &Algorithm::Sha256.hash(bytes))).unwrap();
    }

    /// Another project's `grim add` stores a dependent's manifest and requires layer,
    /// 0.15.1's `add` and `grim fetch` store a manifest with no requires layer, and
    /// 0.15.1 locks no edge: each such row may require what its row does not record,
    /// an install record at its pin included. A row with no requires layer, an empty
    /// one, or no stored manifest shows no edge to name.
    #[test]
    fn a_stored_manifest_names_a_dependent_whose_row_records_no_edge() {
        let home = Home::new();
        let edge = requires_doc(&[(ArtifactKind::Skill, "m", "localhost:5000/acme/m:1")]);
        let p = home.publish(
            None,
            vec![(TAR, b"p".to_vec()), (BUNDLE_LAYER_MEDIA_TYPE, edge.clone())],
        );
        forget(&home, b"p");
        let neighbour = home.publish(
            None,
            vec![(TAR, b"f".to_vec()), (BUNDLE_LAYER_MEDIA_TYPE, edge.clone())],
        );
        let installed = home.publish(None, vec![(TAR, b"i".to_vec()), (BUNDLE_LAYER_MEDIA_TYPE, edge)]);
        let unread_layer = requires_doc(&[(ArtifactKind::Skill, "z", "localhost:5000/acme/z:1")]);
        let fetched = home.publish(
            None,
            vec![(TAR, b"g".to_vec()), (BUNDLE_LAYER_MEDIA_TYPE, unread_layer.clone())],
        );
        forget(&home, &unread_layer);
        let leaf = home.publish(None, vec![(TAR, b"q".to_vec())]);
        forget(&home, b"q");
        let empty = home.publish(
            None,
            vec![(TAR, b"e".to_vec()), (BUNDLE_LAYER_MEDIA_TYPE, requires_doc(&[]))],
        );
        forget(&home, b"e");
        let mut lock = lock_of(&[], "");
        lock.skills = vec![
            row_at("p", p),
            row_at("neighbour", neighbour),
            row_at("i", installed),
            row_at("fetched", fetched),
            row_at("q", leaf),
            row_at("e", empty),
            row_at("cold", Algorithm::Sha256.hash(b"x")),
        ];
        let stored = StoredMembers::read(&home.paths, &declared(&[]), &lock);

        for name in ["p", "neighbour", "i", "fetched"] {
            assert!(stored.unrecorded_reach(&skill(name)).is_some(), "{name}");
        }
        for name in ["q", "e", "cold"] {
            assert!(stored.unrecorded_reach(&skill(name)).is_none(), "{name}");
        }
    }

    /// Each state the store can hold a requester in has one verdict, and a stored
    /// manifest that names requires, read or not, is the one the edit names.
    #[test]
    fn the_store_gives_one_verdict_per_requester_state() {
        let home = Home::new();
        let edge = requires_doc(&[(ArtifactKind::Skill, "m", "localhost:5000/acme/m:1")]);
        let unrecorded = home.publish(
            None,
            vec![(TAR, b"u".to_vec()), (BUNDLE_LAYER_MEDIA_TYPE, edge.clone())],
        );
        let missing = requires_doc(&[(ArtifactKind::Skill, "z", "localhost:5000/acme/z:1")]);
        let layer_missing = home.publish(
            None,
            vec![(TAR, b"l".to_vec()), (BUNDLE_LAYER_MEDIA_TYPE, missing.clone())],
        );
        forget(&home, &missing);
        let leaf = home.publish(None, vec![(TAR, b"q".to_vec())]);
        let empty = home.publish(
            None,
            vec![(TAR, b"e".to_vec()), (BUNDLE_LAYER_MEDIA_TYPE, requires_doc(&[]))],
        );
        let mut recorded = row_at("r", unrecorded.clone());
        recorded.requires = BundleManifest::from_layer_bytes(&edge).unwrap().members;
        let mut lock = lock_of(&[], "");
        lock.skills = vec![
            row_at("cold", Algorithm::Sha256.hash(b"x")),
            row_at("q", leaf),
            row_at("e", empty),
            row_at("u", unrecorded),
            row_at("l", layer_missing),
            recorded,
        ];

        let stored = StoredMembers::read(&home.paths, &declared(&[]), &lock);

        for (name, names_requires) in [
            ("cold", false),
            ("q", false),
            ("e", false),
            ("u", true),
            ("l", true),
            ("r", false),
        ] {
            assert_eq!(
                stored.unrecorded_reach(&skill(name)).is_some(),
                names_requires,
                "{name}"
            );
        }
    }

    /// A row that records no edge is judged by the store even where another row records
    /// its edges, as when an offline relock read one dependent's requires layer and
    /// another project stored `p`'s since. Found by review: `p` read as requiring
    /// nothing there. A row that records its edges takes no verdict and no read: `q`'s
    /// manifest is not in the store, which a read would call never read.
    #[test]
    fn a_row_beside_one_that_records_its_edges_is_judged_by_the_store() {
        let home = Home::new();
        let edge = requires_doc(&[(ArtifactKind::Skill, "m", "localhost:5000/acme/m:1")]);
        let p = home.publish(
            None,
            vec![(TAR, b"p".to_vec()), (BUNDLE_LAYER_MEDIA_TYPE, edge.clone())],
        );
        forget(&home, b"p");
        let mut q = row_at("q", Algorithm::Sha256.hash(b"never stored"));
        q.requires = vec![BundleMember {
            kind: ArtifactKind::Skill,
            name: "r".to_string(),
            id: "localhost:5000/acme/r:1".to_string(),
        }];
        let mut lock = lock_of(&[], "");
        lock.skills = vec![row_at("p", p), q];
        assert!(has_dependency_data(&lock));

        let stored = StoredMembers::read(&home.paths, &declared(&[]), &lock);

        assert!(stored.unrecorded_reach(&skill("p")).is_some());
        assert!(stored.unrecorded_reach(&skill("q")).is_none());
    }

    /// A row that records no edge reaches what its stored requires reach, read
    /// through the store at the lock's pins: `p` reaches `m` only through `n`. Read
    /// whole, it reaches nothing else, so the edit names `p` beside `m` and not
    /// beside `z`; past a link the store lacks, `q` may reach any key.
    #[test]
    fn a_requester_reaches_what_its_stored_requires_reach_through_the_lock_pins() {
        let home = Home::new();
        let m = home.leaf("localhost:5000/acme/m:1");
        let on = |name: &str| requires_doc(&[(ArtifactKind::Skill, name, &format!("localhost:5000/acme/{name}:1"))]);
        let n = home.publish(None, vec![(TAR, b"n".to_vec()), (BUNDLE_LAYER_MEDIA_TYPE, on("m"))]);
        let p = home.publish(None, vec![(TAR, b"p".to_vec()), (BUNDLE_LAYER_MEDIA_TYPE, on("n"))]);
        let q = home.publish(None, vec![(TAR, b"q".to_vec()), (BUNDLE_LAYER_MEDIA_TYPE, on("gone"))]);
        let mut lock = lock_of(&[], "");
        lock.skills = vec![row_at("m", m), row_at("n", n), row_at("p", p), row_at("q", q)];

        let stored = StoredMembers::read(&home.paths, &declared(&[]), &lock);

        let p = stored.unrecorded_reach(&skill("p")).expect("p names requires");
        assert!(p.shows(&skill("m")) && p.shows(&skill("n")));
        assert!(!p.may_reach(&skill("z")), "the store held every link");
        let q = stored.unrecorded_reach(&skill("q")).expect("q names requires");
        assert!(q.shows(&skill("gone")) && !q.shows(&skill("z")));
        assert!(q.may_reach(&skill("z")), "the store lacks `gone`");
    }

    /// A member with no lock row is read at its tag by the next online `grim lock`,
    /// and a cached tag may name an older manifest, so the walk shows nothing past
    /// it. Found by review: `n`'s cached manifest requires nothing, the edit named
    /// no dependent, and the relock brought `m` back at a moved pin.
    #[test]
    fn a_requester_walk_shows_nothing_past_a_member_only_the_tag_cache_pins() {
        const N: &str = "localhost:5000/acme/n:1";
        let home = Home::new();
        let m = home.leaf("localhost:5000/acme/m:1");
        home.leaf(N);
        let p = home.publish(
            None,
            vec![
                (TAR, b"p".to_vec()),
                (BUNDLE_LAYER_MEDIA_TYPE, requires_doc(&[(ArtifactKind::Skill, "n", N)])),
            ],
        );
        let mut lock = lock_of(&[], "");
        lock.skills = vec![row_at("m", m), row_at("p", p)];

        let stored = StoredMembers::read(&home.paths, &declared(&[]), &lock);

        let p = stored.unrecorded_reach(&skill("p")).expect("p names requires");
        assert!(p.shows(&skill("n")), "the link's own key stays");
        assert!(!p.shows(&skill("m")));
        assert!(
            p.may_reach(&skill("m")),
            "the relock reads `n` at its tag, not the cache"
        );
    }

    /// The skills `p0` → `p1` → … → `p{n-1}` → `m`, each a row that records no edge
    /// over a stored manifest and requires layer, every one declared.
    fn chain_of_requesters(home: &Home, n: usize) -> (GrimoireLock, Vec<(String, String)>) {
        let id = |name: &str| format!("localhost:5000/acme/{name}:1");
        let mut rows = vec![row_at("m", home.leaf(&id("m")))];
        let mut below = "m".to_string();
        for i in (0..n).rev() {
            let name = format!("p{i}");
            let layer = requires_doc(&[(ArtifactKind::Skill, &below, &id(&below))]);
            let digest = home.publish(
                None,
                vec![(TAR, name.as_bytes().to_vec()), (BUNDLE_LAYER_MEDIA_TYPE, layer)],
            );
            rows.push(row_at(&name, digest));
            below = name;
        }
        let mut lock = lock_of(&[], "");
        let declared = rows.iter().map(|row| (row.name.clone(), id(&row.name))).collect();
        lock.skills = rows;
        (lock, declared)
    }

    fn set_of(skills: &[(String, String)]) -> DesiredSet {
        let pairs: Vec<(&str, &str)> = skills.iter().map(|(n, i)| (n.as_str(), i.as_str())).collect();
        declared(&pairs)
    }

    /// An edit reads each stored blob of a chain of dependents a bounded number of
    /// times however long the chain, and an edit that drops no row reads none. Found
    /// by review: each dependent's walk read the whole chain below it again, on every
    /// edit.
    #[test]
    fn an_edit_reads_a_chain_of_requesters_a_linear_number_of_times() {
        const N: usize = 50;
        let home = Home::new();
        let (mut lock, skills) = chain_of_requesters(&home, N);
        let before = set_of(&skills);
        let after = set_of(&skills[1..]);
        lock.metadata.declaration_hash = before.declaration_hash_cached().to_string();
        let edit = |name: &str, after: &DesiredSet| {
            BLOB_READS.with(|reads| reads.set(0));
            let stored = StoredMembers::read(&home.paths, &before, &lock);
            let out = drop_from_lock(
                &lock,
                ArtifactKind::Skill,
                name,
                &before,
                after,
                &Invocation::WALKED,
                &stored,
            );
            (out.notes, BLOB_READS.with(std::cell::Cell::get))
        };

        let (notes, reads) = edit("m", &after);
        assert!(reads <= 5 * N, "{reads} blob reads for {N} dependents");
        let notes: Vec<String> = notes.iter().map(ToString::to_string).collect();
        assert!(
            notes
                .iter()
                .any(|note| note.starts_with("skill 'm' leaves the lock, though skill 'p")
                    && note.contains(" and 47 more record no dependency edges and require it")),
            "{notes:?}"
        );
        let (notes, reads) = edit("ghost", &before);
        assert_eq!(reads, 0, "an edit that drops no row reads no dependent");
        assert!(notes.is_empty(), "{notes:?}");
    }

    /// A walk reads at most `MAX_TRANSITIVE_NODES` manifests. Past that the store no
    /// longer shows the whole chain: a requester may reach any key, and a bundle has
    /// no closure to rule a key out of. The chain `pack` -> `p0` -> ... -> `m` is read
    /// by both walks, `pack`'s one manifest longer than `p0`'s.
    #[test]
    fn a_walk_past_the_manifest_cap_may_reach_any_key_and_shows_no_closure() {
        let walked = |requester_manifests: usize| {
            let home = Home::new();
            let (mut lock, _) = chain_of_requesters(&home, requester_manifests - 1);
            // Only `p0` records no edge, so it alone is judged: the walk under test is one.
            let recorded = BundleMember {
                kind: ArtifactKind::Skill,
                name: "m".to_string(),
                id: "localhost:5000/acme/m:1".to_string(),
            };
            for row in lock.skills.iter_mut().filter(|row| row.name != "p0") {
                row.requires = vec![recorded.clone()];
            }
            home.bundle(PACK, &[(ArtifactKind::Skill, "p0", "localhost:5000/acme/p0:1")]);
            let stored = read(&home, &lock);
            let requester = stored.unrecorded_reach(&skill("p0")).expect("p0 names requires");
            (
                requester.may_reach(&skill("unseen")),
                stored.rules_out("pack", &skill("unseen")),
            )
        };

        let whole = walked(MAX_TRANSITIVE_NODES - 1);
        assert_eq!(
            whole,
            (false, true),
            "within the cap the chain is read whole: `unseen` is not reached and `pack` rules it out"
        );
        let capped = walked(MAX_TRANSITIVE_NODES + 1);
        assert_eq!(
            capped,
            (true, false),
            "past the cap any key may be reached and `pack` rules none out"
        );
    }

    /// The recovery restores the config the edit changed, which a `--config` run may
    /// name other than `grimoire.toml`; the lock beside it is always `grimoire.lock`.
    #[test]
    fn the_recovery_names_the_config_the_edit_changed() {
        let home = Home::new();
        let (mut lock, skills) = chain_of_requesters(&home, 1);
        let before = set_of(&skills);
        let after = set_of(&skills[1..]);
        lock.metadata.declaration_hash = before.declaration_hash_cached().to_string();
        let config = Path::new("/work/team/dev.toml");
        let grim = Invocation::config(config);
        let target = EditTarget {
            config_path: config,
            lock_path: Path::new("/work/team/grimoire.lock"),
            grim: &grim,
            store: home.paths.clone(),
        };

        let stored = target.stored_members(&after, &lock);
        let out = drop_from_lock(&lock, ArtifactKind::Skill, "m", &before, &after, &grim, &stored);

        let notes: Vec<String> = out.notes.iter().map(ToString::to_string).collect();
        assert!(
            notes.iter().any(|note| note
                .contains("To keep it, restore /work/team/dev.toml and /work/team/grimoire.lock, run `grim --config")),
            "{notes:?}"
        );
    }

    /// Two directories hold a config and a lock of the same names. A run in one that
    /// names the other's config restores the other's pair, so the note names that pair
    /// by path; a project walked from the working directory is the pair beside the shell,
    /// and a bare file name there is right. Found by review: under `--config` the note
    /// named only `grimoire.toml` and `grimoire.lock`, the working directory's pair.
    #[test]
    fn the_recovery_names_the_pair_of_the_directory_the_config_is_in() {
        let here = std::env::current_dir().unwrap().join("grimoire.toml");
        let there = Path::new("/work/b/grimoire.toml");

        let named = recovery_notes(&Invocation::config(there), there, true);
        let walked = recovery_notes(&Invocation::WALKED, &here, true);

        let [named] = named.as_slice() else {
            panic!("one note: {named:?}")
        };
        assert!(
            named.contains("restore /work/b/grimoire.toml and /work/b/grimoire.lock, run `grim --config /work/b/grimoire.toml lock`"),
            "{named}"
        );
        assert!(!named.contains(here.to_str().unwrap()), "{named}");
        let [walked] = walked.as_slice() else {
            panic!("one note: {walked:?}")
        };
        assert!(
            walked.contains("restore grimoire.toml and grimoire.lock, run `grim lock`"),
            "{walked}"
        );
    }

    /// A walked run finds the project's config in an ancestor of the working
    /// directory, so the pair to restore is not the one beside the shell: the note
    /// names it by absolute path, and a bare name would restore a pair in the
    /// working directory instead. Found by review.
    #[test]
    fn the_recovery_of_a_walked_config_in_an_ancestor_names_the_pair_by_path() {
        let cwd = std::env::current_dir().unwrap();
        let parent = cwd.parent().expect("the working directory has a parent");
        let config = parent.join("grimoire.toml");

        let walked = recovery_notes(&Invocation::WALKED, &config, true);

        let [note] = walked.as_slice() else {
            panic!("one note: {walked:?}")
        };
        let (toml, lock) = (config.display(), parent.join("grimoire.lock"));
        assert!(
            note.contains(&format!("restore {toml} and {}, run `grim lock`", lock.display())),
            "{note}"
        );
    }

    /// The pair beside the shell is named by its file names, as before.
    #[test]
    fn the_recovery_of_a_walked_config_in_the_working_directory_names_bare_files() {
        let config = std::env::current_dir().unwrap().join("grimoire.toml");

        let walked = recovery_notes(&Invocation::WALKED, &config, true);

        let [note] = walked.as_slice() else {
            panic!("one note: {walked:?}")
        };
        assert!(
            note.contains("restore grimoire.toml and grimoire.lock, run `grim lock`"),
            "{note}"
        );
    }

    /// The files are local paths, not registry text: a directory name longer than the
    /// registry-text cap prints whole, or the note names a prefix that is no file.
    #[test]
    fn the_recovery_names_a_path_past_the_registry_text_cap_whole() {
        let dir = format!("/work/{}", "d".repeat(200));
        let config = Path::new(&dir).join("dev.toml");
        let grim = Invocation::config(&config);

        let named = recovery_notes(&grim, &config, true);

        let [note] = named.as_slice() else {
            panic!("one note: {named:?}")
        };
        assert!(
            note.contains(&format!(
                "restore {dir}/dev.toml and {dir}/dev.lock, run `grim --config {dir}/dev.toml lock`"
            )),
            "{note}"
        );
    }

    /// The notes of removing skill `m` from the chain of requesters, for the config
    /// at `config` run as `grim`; `proven` when the lock's hash proves the
    /// declaration before the edit.
    fn recovery_notes(grim: &Invocation, config: &Path, proven: bool) -> Vec<String> {
        let home = Home::new();
        let (mut lock, skills) = chain_of_requesters(&home, 1);
        let before = set_of(&skills);
        let after = set_of(&skills[1..]);
        if proven {
            lock.metadata.declaration_hash = before.declaration_hash_cached().to_string();
        }
        let target = EditTarget {
            config_path: config,
            lock_path: &config.with_extension("lock"),
            grim,
            store: home.paths.clone(),
        };

        let stored = target.stored_members(&after, &lock);
        let out = drop_from_lock(&lock, ArtifactKind::Skill, "m", &before, &after, grim, &stored);

        out.notes.iter().map(ToString::to_string).collect()
    }

    /// The lock a hand edit made stale resolves every untagged pin on its next
    /// `grim lock`, so the recovery's lock cannot promise to keep the rows: the
    /// note says what that lock does and promises nothing. Found by review: it
    /// said "To keep it", and a pin whose tag moved before the recovery moved.
    #[test]
    fn the_recovery_on_a_lock_the_config_outran_promises_no_pin() {
        let grim = Invocation::WALKED;
        let config = std::env::current_dir().unwrap().join("grimoire.toml");

        let stale = recovery_notes(&grim, &config, false);
        let proven = recovery_notes(&grim, &config, true);

        let [stale] = stale.as_slice() else {
            panic!("one note: {stale:?}")
        };
        assert!(!stale.contains("To keep"), "{stale}");
        assert!(
            stale.contains("restore grimoire.toml and grimoire.lock, run `grim lock` online once"),
            "{stale}"
        );
        assert!(
            stale.ends_with(
                "The recovery's `grim lock` resolves every untagged pin once, because the config changed since the last lock"
            ),
            "{stale}"
        );
        let [proven] = proven.as_slice() else {
            panic!("one note: {proven:?}")
        };
        assert!(
            proven.contains("To keep it, restore grimoire.toml and grimoire.lock"),
            "{proven}"
        );
        assert!(!proven.contains("resolves every untagged pin"), "{proven}");
    }

    /// Under `--global` the edit changed the files under `$GRIM_HOME`, and a user in
    /// a project would restore that project's pair, so the note names where they are.
    #[test]
    fn the_recovery_under_global_names_the_files_under_grim_home() {
        let config = Path::new("/home/me/.grimoire/grimoire.toml");

        let global = recovery_notes(&Invocation::GLOBAL, config, true);

        let [note] = global.as_slice() else {
            panic!("one note: {global:?}")
        };
        assert!(
            note.contains(
                "To keep it, restore $GRIM_HOME/grimoire.toml and its grimoire.lock, run \
                 `grim --global lock` online once before such an edit, then repeat it"
            ),
            "{note}"
        );
    }

    /// A path bundle has no tag and no digest, so the store cannot speak for it.
    #[test]
    fn a_path_bundle_is_left_unread() {
        let mut set = declared(&[]);
        let path = crate::config::path_source::PathSource::parse("./pack").unwrap();
        set.bundles.insert(
            "pack".to_string(),
            crate::config::declaration::DeclaredSource::Path(path),
        );
        set.invalidate_declaration_hash_cache();

        let stored = StoredMembers::read(&Home::new().paths, &set, &lock_of(&[], ""));

        assert!(!stored.rules_out("pack", &skill("y")));
    }
}
