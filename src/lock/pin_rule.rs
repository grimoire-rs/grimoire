// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! What a pin answers: the one owner of the rule in
//! `adr_lock_pin_stability.md`, § The pin rule.
//!
//! A pin answers a request only where the lock shows that a resolve of that
//! request gave the pin's digest. The graph holds a set of requests for a key
//! and `pinned` holds one reference, so writers compose the answers defined
//! here rather than comparing the two themselves.

use std::collections::{BTreeMap, BTreeSet};

use crate::config::declaration::DesiredSet;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::locked_bundle::{LockedBundle, LockedBundleSource};
use crate::lock::locked_source::LockedSource;
use crate::lock::merge_policy::winning_request;
use crate::oci::bundle::BundleMember;
use crate::oci::identifier::is_valid_oci_tag;
use crate::oci::{ArtifactKind, Digest, Identifier, NodeKey, PinnedIdentifier};

/// A pin and the lock row that records it.
#[derive(Debug, Clone, Copy)]
pub struct PinRecord<'a> {
    pub pin: &'a PinnedIdentifier,
    /// A bundle's row. Its `repo` and `tag` name what `pin` cannot: it records
    /// a bare request as `latest` and a digest one by short digest.
    pub row: Option<&'a LockedBundle>,
    /// The key's config binding.
    pub binding: &'a str,
}

/// How much of a set of requests a pin answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answered<'r> {
    All,
    /// At least one, and not this one: the smallest request left over.
    Some {
        unanswered: &'r Identifier,
    },
    /// None, which is also what an empty set of requests gets.
    None,
}

impl PinRecord<'_> {
    /// Whether the pin's own record names `id`, in the pin's repository: the
    /// match rule, a bundle row at a digest the pin holds, or `id` carrying the
    /// pin's digest, since a digest names itself.
    pub fn names(&self, id: &Identifier) -> bool {
        let in_row = || self.row.is_some_and(|row| snapshot_matches(self.binding, id, row));
        self.pin.answers(id)
            || (holds_digest(self.pin, id) && same_repository(self.pin, id) && (in_row() || id.digest().is_some()))
    }

    /// How much of `requests` the pin's own record names. Takes no recorded edge
    /// as evidence, so it is the answer for a key whose edges were shadowed.
    pub fn classify<'r>(&self, requests: &'r BTreeSet<Identifier>) -> Answered<'r> {
        classify_by(requests, |id| self.names(id))
    }

    /// [`Self::classify`] for a key whose declaration just left. `declared` is
    /// the identifier the lock's proof held it at, which is the request a tagless
    /// pin resolved, so the same request from an edge is answered.
    pub fn classify_undeclared<'r>(
        &self,
        declared: Option<&Identifier>,
        requests: &'r BTreeSet<Identifier>,
    ) -> Answered<'r> {
        let stamped = |id: &Identifier| self.pin.tag().is_none() && declared == Some(id);
        classify_by(requests, |id| self.names(id) || stamped(id))
    }

    /// Whether a bundle row disagrees with its own pin about the repository. No
    /// writer leaves one. A digest selects a bundle's member list and the store
    /// serves a manifest by digest whatever repository asks, so such a row must
    /// answer nothing.
    fn contradicts_itself(&self) -> bool {
        let home = self.pin.registry_repository();
        self.row.is_some_and(|row| row.repo() != Some(home.as_str()))
    }
}

fn classify_by(requests: &BTreeSet<Identifier>, names: impl Fn(&Identifier) -> bool) -> Answered<'_> {
    match requests.iter().find(|id| !names(id)) {
        None if requests.is_empty() => Answered::None,
        None => Answered::All,
        Some(unanswered) if requests.iter().any(&names) => Answered::Some { unanswered },
        Some(_) => Answered::None,
    }
}

/// What a lock's hash shows about one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashProof {
    /// It proves a declaration with the key direct, so no edge was resolved.
    Direct,
    /// It proves a declaration without the key, which the graph settled by
    /// agreement: every edge gave one digest, or the run exited 78. The one
    /// exception is a direct line that held a disagreeing edge off and left: the pin
    /// stays at exit 0, and the next relock proves a hash over edges that disagree.
    Indirect,
    /// It matches no declaration in hand, and a stale hash carries no reason.
    Stale,
}

impl HashProof {
    /// What a lock's hash shows about `key`, `proof` being the declaration it
    /// matches.
    pub fn of(proof: Option<&DesiredSet>, key: &NodeKey) -> Self {
        match proof {
            Some(proof) if proof.table(key.0).contains_key(&key.1) => Self::Direct,
            Some(_) => Self::Indirect,
            None => Self::Stale,
        }
    }
}

/// What the previous lock's edges onto a key may do for the next relock.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EdgeEvidence {
    /// The recorded edges that answer as they stand.
    pub requests: BTreeSet<Identifier>,
    /// A tagless artifact pin on a key the proof holds indirectly, which any edge
    /// of a parent read as the proof shows answers: see [`answers_listed_edge`].
    pub takes_listed_edges: bool,
    /// A part's row whose own stamp records the bundle it came through, which that
    /// bundle's list read at the recorded digest answers: see [`answers_stamped_edge`].
    /// Under a proof, or for a pin with a tag.
    pub takes_stamped_edges: bool,
}

/// Keys `lock` lists more than once, as a keep-both merge leaves them. Nothing
/// says which side is current, so neither row is a pin.
pub fn listed_twice(lock: &GrimoireLock) -> BTreeSet<NodeKey> {
    let mut seen = BTreeSet::new();
    lock.node_keys().filter(|key| !seen.insert(key.clone())).collect()
}

/// A lock's hash as a message prints it: an empty one is no hash at all.
pub fn printed_hash(hash: &str) -> &str {
    if hash.is_empty() { "none" } else { hash }
}

/// Every edge `lock` records, per child key, each against its parent's pin. A
/// row that cannot be read vouches for nothing: the graph reads the real edge
/// itself.
pub fn recorded_requests(lock: &GrimoireLock) -> BTreeMap<NodeKey, BTreeSet<Identifier>> {
    reached_requests(lock, None)
}

/// [`recorded_requests`] less the edges of rows outside `reached`. Such a row is a
/// ghost: no relock claims its edge, so the edge vouches for nothing. With `None`,
/// a lock that cannot show the closure, every edge stays.
pub fn reached_requests(
    lock: &GrimoireLock,
    reached: Option<&BTreeSet<NodeKey>>,
) -> BTreeMap<NodeKey, BTreeSet<Identifier>> {
    let artifacts = lock
        .iter_artifacts()
        .map(|a| ((a.kind, a.name.clone()), a.source.pinned(), &a.requires));
    let bundles = lock
        .bundles
        .iter()
        .map(|b| ((ArtifactKind::Bundle, b.name.clone()), b.pinned(), &b.members));
    let mut edges: BTreeMap<NodeKey, BTreeSet<Identifier>> = BTreeMap::new();
    for (parent, pin, members) in artifacts.chain(bundles) {
        if reached.is_some_and(|reached| !reached.contains(&parent)) {
            continue;
        }
        let anchor = pin.map(PinnedIdentifier::as_identifier);
        for member in members {
            if let Some(id) = member.absolute_id(anchor) {
                edges.entry((member.kind, member.name.clone())).or_default().insert(id);
            }
        }
    }
    edges
}

/// The edge lists a lock's rows record: a bundle row's members and an artifact's
/// `requires`, each at the digest its row pins, `None` for a path row. They are the
/// only edges a lock shows its writer walked: grim 0.15.1 read every bundle's list
/// and no `requires` layer, so its lock records no dependent's edges.
#[derive(Debug, Clone, Default)]
pub struct WalkedEdges(BTreeMap<NodeKey, Vec<RowEdges>>);

/// One row's digest, `None` for a path row, and the edges it records.
type RowEdges = (Option<Digest>, Vec<BundleMember>);

impl WalkedEdges {
    /// Every row's list in `lock`, both rows of a key it lists twice included.
    pub fn of(lock: &GrimoireLock) -> Self {
        let mut walked = Self::default();
        for row in lock.iter_artifacts() {
            let digest = row.source.pinned().map(PinnedIdentifier::digest);
            walked.insert((row.kind, row.name.clone()), digest, row.requires.clone());
        }
        for row in &lock.bundles {
            let digest = row.pinned().map(PinnedIdentifier::digest);
            walked.insert((ArtifactKind::Bundle, row.name.clone()), digest, row.members.clone());
        }
        walked
    }

    /// Record that a row of `key` at `digest` lists `members`.
    pub fn insert(&mut self, key: NodeKey, digest: Option<Digest>, members: Vec<BundleMember>) {
        self.0.entry(key).or_default().push((digest, members));
    }

    /// Whether a row of `parent` at `digest` lists the edge onto `child` at `id`, a
    /// relative member resolved against `anchor` as the read resolved it. `None` when
    /// the lock has no row for `parent`, which shows nothing either way.
    pub fn records(
        &self,
        parent: &NodeKey,
        digest: Option<&Digest>,
        anchor: Option<&Identifier>,
        (kind, name): &NodeKey,
        id: &Identifier,
    ) -> Option<bool> {
        let rows = self.0.get(parent)?;
        let lists = rows.iter().filter(|(at, _)| at.as_ref() == digest);
        let names = |member: &BundleMember| member.kind == *kind && member.name == *name;
        let walked = |member: &BundleMember| names(member) && member.absolute_id(anchor).as_ref() == Some(id);
        Some(lists.flat_map(|(_, members)| members).any(walked))
    }
}

/// The request a tagless pin takes its tag from, among those on its key: the
/// winner in its repository, since relative children anchor there. It may be
/// digest-only, which leaves the pin tagless.
pub fn stamp_request<'c>(
    pin: &PinnedIdentifier,
    claims: impl IntoIterator<Item = &'c Identifier>,
) -> Option<&'c Identifier> {
    winning_request(claims.into_iter().filter(|id| same_repository(pin, id)))
}

/// Stamp the tagless pins of `lock`, as a relock under `proof` would, for an edit
/// of `previous` that ends its proof: past it nothing shows what they answered, and
/// the next relock would resolve them all. A key direct in `proof` takes its
/// declaration, any other an edge from a row in `reached`, which the proof vouched
/// for: one `lock` still records, which the next relock reads, else one `previous`
/// recorded from a parent the edit took. A key listed twice is no pin. Writes no
/// digest.
pub fn stamp_kept_pins(
    lock: &mut GrimoireLock,
    previous: &GrimoireLock,
    proof: &DesiredSet,
    reached: Option<&BTreeSet<NodeKey>>,
) {
    let recorded = [reached_requests(lock, reached), reached_requests(previous, reached)];
    let twice = listed_twice(lock);
    for entry in lock.iter_artifacts_mut() {
        let Some(pin) = entry.source.pinned().filter(|pin| pin.tag().is_none()) else {
            continue;
        };
        let key = (entry.kind, entry.name.clone());
        if twice.contains(&key) {
            continue;
        }
        let request = match proof.table(entry.kind).get(&entry.name) {
            Some(declared) => declared.identifier(),
            None => recorded
                .iter()
                .find_map(|edges| edges.get(&key).and_then(|edges| stamp_request(pin, edges))),
        };
        if let Some(stamped) = request.and_then(|id| with_tag_of(pin, id)) {
            entry.source = LockedSource::Registry(stamped);
        }
    }
}

/// `pin` under the tag of `id`. A request elsewhere answers nothing, and a
/// digest-only one has no tag to give.
fn with_tag_of(pin: &PinnedIdentifier, id: &Identifier) -> Option<PinnedIdentifier> {
    let answers = same_repository(pin, id) && holds_digest(pin, id);
    id.tag().filter(|_| answers)?;
    PinnedIdentifier::try_from(id.clone_with_digest(pin.digest())).ok()
}

/// The evidence `recorded`, the previous lock's edges onto a key, gives under
/// `proof`.
pub fn edge_evidence(record: PinRecord<'_>, proof: HashProof, recorded: BTreeSet<Identifier>) -> EdgeEvidence {
    stamped_edge_evidence(record, proof, recorded, false)
}

/// [`edge_evidence`] for a row of a marketplace part, `stamped` being whether its own
/// stamp records a bundle's digest. A part keeps no `[[bundle]]` row, so no recorded edge
/// names the pin of a row a bundle provided: the stamp shows that a claim settled it, and
/// a tag on the pin shows which, so on a stale hash it counts as the recorded claim the
/// pin names.
pub fn stamped_edge_evidence(
    record: PinRecord<'_>,
    proof: HashProof,
    recorded: BTreeSet<Identifier>,
    stamped: bool,
) -> EdgeEvidence {
    if record.contradicts_itself() {
        return EdgeEvidence::default();
    }
    let tagged = record.pin.tag().is_some();
    match proof {
        HashProof::Direct => EdgeEvidence::default(),
        HashProof::Indirect => {
            // A bundle's row already names its request.
            let tagless = record.row.is_none() && !tagged;
            EdgeEvidence {
                takes_listed_edges: tagless,
                takes_stamped_edges: true,
                requests: recorded,
            }
        }
        // A tagged row took its tag from a claim the part recorded, which a stamp then
        // shows came through this bundle.
        HashProof::Stale if (stamped && tagged) || recorded.iter().any(|id| record.names(id)) => EdgeEvidence {
            requests: recorded,
            takes_stamped_edges: tagged,
            ..EdgeEvidence::default()
        },
        HashProof::Stale => EdgeEvidence {
            takes_stamped_edges: tagged,
            ..EdgeEvidence::default()
        },
    }
}

/// The provenance tag recorded for a bundle member: the bundle's own tag,
/// or its short digest when the bundle is declared digest-only — never a
/// fabricated `latest`.
pub fn bundle_provenance_tag(bundle_id: &Identifier) -> String {
    match bundle_id.tag() {
        Some(tag) => tag.to_string(),
        None => bundle_id
            .digest()
            .map(|d| d.to_short_string())
            .unwrap_or_else(|| "latest".to_string()),
    }
}

fn same_repository(pin: &PinnedIdentifier, id: &Identifier) -> bool {
    pin.registry_repository() == id.registry_repository()
}

/// Whether `pin` is the digest `id` names, if it names one: a digest named outright
/// is that digest, and no pin answers it for another.
fn holds_digest(pin: &PinnedIdentifier, id: &Identifier) -> bool {
    id.digest().is_none_or(|digest| digest == pin.digest())
}

/// Whether a cached snapshot corresponds to the declared bundle binding:
/// same binding name, same `registry/repo`, and the snapshot's provenance
/// tag equals the declared tag (or the short digest for a digest-only
/// declaration — mirroring how the resolver stamps it).
pub fn snapshot_matches(binding: &str, declared_id: &Identifier, snapshot: &LockedBundle) -> bool {
    if snapshot.name != binding {
        return false;
    }
    match &snapshot.source {
        LockedBundleSource::Registry { repo, tag, .. } => {
            if *repo != declared_id.registry_repository() {
                return false;
            }
            let declared_tag = match declared_id.tag() {
                Some(t) => t.to_string(),
                None => declared_id
                    .digest()
                    .map(|d| d.to_short_string())
                    .unwrap_or_else(|| "latest".to_string()),
            };
            *tag == declared_tag
        }
        // A path-sourced snapshot never corresponds to a registry-declared
        // identifier: the two source kinds carry disjoint identity, mirroring
        // `LockedSource::eq_content`'s cross-variant-is-always-false rule.
        // `effective_set` already bails on a path-declared bundle (its
        // `identifier()` is `None`) before reaching this arm, so a path
        // snapshot is only ever compared against a registry declaration here —
        // always a miss (graceful degrade; full path-keyed eviction is a
        // deferred follow-up, see plan_local_bundles_tui_group.md).
        LockedBundleSource::Path { .. } => false,
    }
}

/// Whether the pin answers a direct claim `id`: by its record, or as a tagless
/// pin whose proof declares `id`, which is all a tagless pin has to show. An edge
/// says what a parent asked for, never what a declaration resolved to, so the
/// edges stay out: a line at the tag they stamp waits for the walk to show that
/// every edge onto the key still keeps the pin, as `grim lock` would.
pub fn answers_direct(record: PinRecord<'_>, declared: Option<&Identifier>, id: &Identifier) -> bool {
    let stamped = record.pin.tag().is_none() && declared == Some(id);
    // The store serves a manifest by digest whatever repository asks, so a
    // `pinned` from another repository would pick a bundle's member list.
    same_repository(record.pin, id) && (record.names(id) || stamped)
}

/// Whether the pin answers an edge claim `id`. An edge the lock recorded answers
/// in any repository, so a mirror's moved tag cannot move a pin. An edge it did
/// not record needs the pin's record, which is bound to the pin's repository, so
/// a pin is never carried into a new one.
pub fn answers_edge(record: PinRecord<'_>, evidence: &EdgeEvidence, id: &Identifier) -> bool {
    evidence.requests.contains(id) || record.names(id)
}

/// Whether the pin answers an edge a parent lists where the lock's proof shows that
/// parent, so the walk behind the hash met the edge and settled the key on this
/// digest. A tagless pin takes it from the manifest with no recorded edge, which is
/// what stamps a pre-change lock, though only in its own repository, where a stamp
/// can land: a pin is never carried into a new one.
pub fn answers_listed_edge(record: PinRecord<'_>, evidence: &EdgeEvidence, id: &Identifier) -> bool {
    answers_edge(record, evidence, id) || (evidence.takes_listed_edges && same_repository(record.pin, id))
}

/// Whether the pin answers an edge a bundle lists where the row's own stamp records
/// that bundle at the digest the edge was read at: the part shows the row came through
/// that bundle at that digest and settled at its pin, as a `[[bundle]]` row's member
/// list would for a project. Inert with no part stamps, which mark no edge.
pub fn answers_stamped_edge(record: PinRecord<'_>, evidence: &EdgeEvidence, id: &Identifier) -> bool {
    answers_listed_edge(record, evidence, id) || evidence.takes_stamped_edges
}

/// Whether the pin stays for a key `claims`, `listed` and `stamped`, the edges of
/// parents read as the proof shows and the edges of bundles read as a row's stamp
/// records, settled by agreement: a claim it answers holds it, so a recorded edge no
/// claim makes carries it nowhere. A digest named outright is that digest, and no pin
/// answers it for another.
pub fn keeps_pin(
    record: PinRecord<'_>,
    evidence: &EdgeEvidence,
    claims: &[&Identifier],
    listed: &[&Identifier],
    stamped: &[&Identifier],
) -> bool {
    let holds = |id: &&&Identifier| holds_digest(record.pin, id);
    claims.iter().filter(holds).any(|id| answers_edge(record, evidence, id))
        || listed
            .iter()
            .filter(holds)
            .any(|id| answers_listed_edge(record, evidence, id))
        || stamped
            .iter()
            .filter(holds)
            .any(|id| answers_stamped_edge(record, evidence, id))
}

/// Whether the edges onto a key hold its pin: it names one, and none names a digest
/// it does not hold. No pin answers that edge, and a relock follows it or exits 78,
/// so a stamp would state a row no relock writes.
pub fn held_by_edges(record: PinRecord<'_>, requests: &BTreeSet<Identifier>) -> bool {
    record.classify(requests) != Answered::None && requests.iter().all(|id| holds_digest(record.pin, id))
}

/// Whether `record` is the row a relock writes for a key `id` declares, so that a
/// restamp of a stale lock states a fact. A bundle row records the request it
/// resolved, which its record [names](PinRecord::names). An artifact pin shows it
/// only by its tag, under the match rule and not [`answers_direct`]: a digest names
/// itself under any tag, but a relock writes the declaration's own reference.
pub fn restamp_answers_declared(record: PinRecord<'_>, id: &Identifier) -> bool {
    match record.row {
        Some(_) => record.names(id),
        None => record.pin.tag().is_some() && record.pin.answers(id),
    }
}

/// Whether `record` is the row a relock writes for a key no declaration holds,
/// `requests` being the edges onto it: they [hold it](held_by_edges), and the record
/// shows what it answered, which a tagless artifact pin cannot.
pub fn restamp_answers_edges(record: PinRecord<'_>, requests: &BTreeSet<Identifier>) -> bool {
    (record.row.is_some() || record.pin.tag().is_some()) && held_by_edges(record, requests)
}

/// The reference `grim status --check` re-resolves for a pin `edges` reach,
/// `tag` being the tag it carries. It answers for an edge that survives: a pin
/// keeps its tag past the edge that named it, and that tag may no longer be
/// what `grim update` asks for.
pub fn recorded_reference(
    edges: BTreeSet<Identifier>,
    pin: &PinnedIdentifier,
    tag: Option<&str>,
) -> Option<Identifier> {
    // A digest-bearing edge holds the pin wherever the tag beside it moved:
    // resolving that tag would report an update `grim update` never applies.
    if let Some(held) = edges.iter().find(|id| id.digest().is_some()) {
        return Some(held.clone());
    }
    // Built without the digest: a digest-bearing id resolves to itself, so the
    // row would read `false` however far the tag moved.
    // A lock is input: a tag outside the grammar must not reach a request URL.
    let own = tag
        .filter(|tag| is_valid_oci_tag(tag))
        .map(|tag| Identifier::new_registry(pin.repository(), pin.registry()).clone_with_tag(tag));
    match own {
        Some(own) if edges.is_empty() || edges.contains(&own) => Some(own),
        _ => edges.into_iter().next(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::declaration::DeclaredSource;
    use crate::lock::effective_set::reached_rows;
    use crate::lock::locked_bundle::LockedBundleSource;
    use crate::oci::{Algorithm, Digest};

    fn digest(content: &str) -> Digest {
        Algorithm::Sha256.hash(content.as_bytes())
    }

    fn id(reference: &str) -> Identifier {
        Identifier::parse(reference).unwrap()
    }

    fn pin(reference: &str, content: &str) -> PinnedIdentifier {
        PinnedIdentifier::try_from(id(reference).clone_with_digest(digest(content))).unwrap()
    }

    fn at(reference: &str, content: &str) -> Identifier {
        id(reference).clone_with_digest(digest(content))
    }

    fn record(pin: &PinnedIdentifier) -> PinRecord<'_> {
        PinRecord {
            pin,
            row: None,
            binding: "x",
        }
    }

    fn set(ids: &[Identifier]) -> BTreeSet<Identifier> {
        ids.iter().cloned().collect()
    }

    fn keeps(record: PinRecord<'_>, evidence: &EdgeEvidence, claims: &[Identifier]) -> bool {
        keeps_pin(record, evidence, &claims.iter().collect::<Vec<_>>(), &[], &[])
    }

    fn row(repo: &str, tag: &str, pin: &PinnedIdentifier) -> LockedBundle {
        LockedBundle {
            name: "x".to_string(),
            source: LockedBundleSource::Registry {
                repo: repo.to_string(),
                tag: tag.to_string(),
                pinned: pin.clone(),
            },
            members: Vec::new(),
            transitive: false,
        }
    }

    #[test]
    fn provenance_tag_uses_digest_when_bundle_is_digest_pinned() {
        let tagged = Identifier::parse("ghcr.io/acme/stack:1.0").unwrap();
        assert_eq!(bundle_provenance_tag(&tagged), "1.0");

        let hex = "a".repeat(64);
        let pinned = Identifier::parse(&format!("ghcr.io/acme/stack@sha256:{hex}")).unwrap();
        let tag = bundle_provenance_tag(&pinned);
        assert!(
            tag.starts_with("sha256:"),
            "digest-pinned bundle records its digest, got {tag}"
        );
        assert_ne!(tag, "latest", "must not fabricate a `latest` tag");
    }

    /// Every kind's tagless pin takes the tag its declaration names, settings
    /// included: an edit that ends a proof must not leave a pin the next relock moves.
    #[test]
    fn stamp_kept_pins_tags_a_settings_pin_from_its_declaration() {
        let mut lock = GrimoireLock::from_toml_str(&format!(
            "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\ndeclaration_hash = \"\"\n\
             generated_by = \"grim 0.1.0\"\ngenerated_at = \"2026-04-19T00:00:00Z\"\n\n\
             [[settings]]\nname = \"team\"\npinned = \"ghcr.io/acme/team@{}\"\n",
            digest("t")
        ))
        .unwrap();
        let mut proof = DesiredSet::default();
        proof.settings.insert(
            "team".to_string(),
            crate::config::declaration::DeclaredSource::Registry(id("ghcr.io/acme/team:1")),
        );

        let previous = lock.clone();
        let reached = reached_rows(&proof, &previous, Some(&proof));
        stamp_kept_pins(&mut lock, &previous, &proof, reached.as_ref());

        assert_eq!(lock.settings[0].source.pinned().unwrap().tag(), Some("1"));
    }

    #[test]
    fn a_pin_names_its_own_tag_and_no_other() {
        let pinned = pin("ghcr.io/acme/x:1", "d");
        assert!(record(&pinned).names(&id("ghcr.io/acme/x:1")));
        assert!(!record(&pinned).names(&id("ghcr.io/acme/x:2")));
        assert!(!record(&pinned).names(&id("ghcr.io/other/x:1")));
    }

    #[test]
    fn a_digest_names_itself_only_in_the_pins_repository() {
        let pinned = pin("ghcr.io/acme/x:1", "d");
        assert!(record(&pinned).names(&at("ghcr.io/acme/x", "d")));
        assert!(!record(&pinned).names(&at("ghcr.io/acme/x", "other")));
        assert!(!record(&pinned).names(&at("ghcr.io/other/x", "d")));
    }

    #[test]
    fn a_row_names_its_request_only_at_a_digest_the_pin_holds() {
        let pinned = pin("ghcr.io/acme/x:9", "d");
        let row = row("ghcr.io/acme/x", "1", &pinned);
        let record = PinRecord {
            pin: &pinned,
            row: Some(&row),
            binding: "x",
        };
        assert!(
            record.names(&id("ghcr.io/acme/x:1")),
            "the row names what the pin cannot"
        );
        assert!(record.names(&at("ghcr.io/acme/x:1", "d")));
        assert!(
            !record.names(&at("ghcr.io/acme/x:1", "other")),
            "a row's tag at another digest vouches for nothing"
        );
    }

    #[test]
    fn classify_splits_the_requests_three_ways() {
        let pinned = pin("ghcr.io/acme/x:1", "d");
        let (one, two) = (id("ghcr.io/acme/x:1"), id("ghcr.io/acme/x:2"));
        assert_eq!(
            record(&pinned).classify(&set(std::slice::from_ref(&one))),
            Answered::All
        );
        let both = set(&[one, two.clone()]);
        assert_eq!(record(&pinned).classify(&both), Answered::Some { unanswered: &two });
        assert_eq!(record(&pinned).classify(&set(&[two])), Answered::None);
        assert_eq!(record(&pinned).classify(&BTreeSet::new()), Answered::None);
    }

    #[test]
    fn evidence_follows_what_the_hash_shows() {
        let pinned = pin("ghcr.io/acme/x:1", "d");
        let named = set(&[id("ghcr.io/acme/x:1"), id("ghcr.io/acme/x:2")]);
        let unnamed = set(&[id("ghcr.io/acme/x:2")]);
        let evidence = |proof, edges: &BTreeSet<Identifier>| edge_evidence(record(&pinned), proof, edges.clone());

        assert_eq!(evidence(HashProof::Direct, &named), EdgeEvidence::default());
        assert_eq!(evidence(HashProof::Indirect, &unnamed).requests, unnamed);
        assert_eq!(evidence(HashProof::Stale, &named).requests, named);
        // A stale hash shows no edge for an unnamed request, but a tagged row took its tag
        // from a claim, which a stamp can then show came through its bundle.
        let stale = evidence(HashProof::Stale, &unnamed);
        assert_eq!(
            stale,
            EdgeEvidence {
                takes_stamped_edges: true,
                ..EdgeEvidence::default()
            }
        );
    }

    #[test]
    fn a_direct_claim_takes_the_pins_own_record() {
        let pinned = pin("ghcr.io/acme/x:1", "d");
        assert!(answers_direct(record(&pinned), None, &id("ghcr.io/acme/x:1")));
        assert!(!answers_direct(record(&pinned), None, &id("ghcr.io/acme/x:2")));
    }

    /// A tagless pin answers the line its proof declares. With no declaration it
    /// answers none, even the one its recorded edges stamp: whether `grim lock`
    /// keeps the pin depends on every edge the walk meets.
    #[test]
    fn a_tagless_pin_answers_only_the_declaration_its_proof_names() {
        let pinned = pin("ghcr.io/acme/x", "d");
        let declared = id("ghcr.io/acme/x:1");
        assert!(answers_direct(record(&pinned), Some(&declared), &declared));
        assert!(!answers_direct(
            record(&pinned),
            Some(&declared),
            &id("ghcr.io/acme/x:2")
        ));
        assert!(!answers_direct(record(&pinned), None, &declared), "no declaration");
        let foreign = id("ghcr.io/other/x:1");
        assert!(
            !answers_direct(record(&pinned), Some(&foreign), &foreign),
            "a declared id in another repository never reads this pin"
        );
    }

    /// Under a proof that holds the key indirectly, a tagless artifact pin takes an
    /// edge a parent lists as the proof shows it, with no record of the edge, in its
    /// own repository and at no other digest.
    #[test]
    fn a_tagless_pin_answers_a_listed_edge_only_under_a_proof() {
        let pinned = pin("ghcr.io/acme/x", "d");
        let (listed, mirror) = (id("ghcr.io/acme/x:1"), id("ghcr.io/other/x:1"));
        let none = BTreeSet::new();
        let proven = edge_evidence(record(&pinned), HashProof::Indirect, none.clone());
        assert!(answers_listed_edge(record(&pinned), &proven, &listed));
        assert!(!answers_listed_edge(record(&pinned), &proven, &mirror));
        assert!(
            !answers_edge(record(&pinned), &proven, &listed),
            "a plain edge stays out"
        );

        let stale = edge_evidence(record(&pinned), HashProof::Stale, none.clone());
        assert!(!answers_listed_edge(record(&pinned), &stale, &listed));
        let direct = edge_evidence(record(&pinned), HashProof::Direct, none.clone());
        assert!(!answers_listed_edge(record(&pinned), &direct, &listed));

        let tagged = pin("ghcr.io/acme/x:9", "d");
        let evidence = edge_evidence(record(&tagged), HashProof::Indirect, none);
        assert!(!answers_listed_edge(record(&tagged), &evidence, &listed), "a tag says");

        assert!(keeps_pin(record(&pinned), &proven, &[], &[&listed], &[]));
        assert!(!keeps_pin(record(&pinned), &proven, &[&listed], &[], &[]), "not listed");
        let other = at("ghcr.io/acme/x:1", "other");
        assert!(
            !keeps_pin(record(&pinned), &proven, &[], &[&other], &[]),
            "a digest it lacks"
        );
    }

    /// A lock shows an edge walked only where a row at the read digest lists it. A
    /// row grim 0.15.1 wrote for a dependent lists none of its edges.
    #[test]
    fn a_lock_shows_an_edge_walked_only_where_a_row_at_the_digest_lists_it() {
        let (pack, x, y) = (digest("pack"), digest("x"), digest("y"));
        let lock = GrimoireLock::from_toml_str(&format!(
            "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\ndeclaration_hash = \"\"\n\
             generated_by = \"grim 0.15.1\"\ngenerated_at = \"2026-04-19T00:00:00Z\"\n\n\
             [[skill]]\nname = \"x\"\npinned = \"ghcr.io/acme/x@{x}\"\n\n\
             [[skill]]\nname = \"y\"\npinned = \"ghcr.io/acme/y:1@{y}\"\n\n\
             [[skill.requires]]\nkind = \"skill\"\nname = \"held\"\nid = \"./held:0\"\n\n\
             [[bundle]]\nname = \"pack\"\nrepo = \"ghcr.io/acme/pack\"\ntag = \"1\"\n\
             pinned = \"ghcr.io/acme/pack@{pack}\"\n\n\
             [[bundle.member]]\nkind = \"skill\"\nname = \"held\"\nid = \"ghcr.io/acme/held:1\"\n"
        ))
        .unwrap();
        let walked = WalkedEdges::of(&lock);
        let held = (ArtifactKind::Skill, "held".to_string());
        let (one, zero) = (id("ghcr.io/acme/held:1"), id("ghcr.io/acme/held:0"));
        let skill = |name: &str| (ArtifactKind::Skill, name.to_string());
        let bundle = (ArtifactKind::Bundle, "pack".to_string());
        let anchor = id("ghcr.io/acme/y:1");

        assert_eq!(walked.records(&bundle, Some(&pack), None, &held, &one), Some(true));
        assert_eq!(walked.records(&bundle, Some(&pack), None, &held, &zero), Some(false));
        assert_eq!(
            walked.records(&bundle, Some(&x), None, &held, &one),
            Some(false),
            "a digest"
        );
        assert_eq!(walked.records(&skill("x"), Some(&x), None, &held, &zero), Some(false));
        let relative = walked.records(&skill("y"), Some(&y), Some(&anchor), &held, &zero);
        assert_eq!(relative, Some(true), "a relative edge resolves against the read");
        assert_eq!(walked.records(&skill("z"), None, None, &held, &zero), None, "no row");
    }

    /// A bundle's row names its request, so a listed edge adds nothing to it.
    #[test]
    fn a_bundle_row_takes_no_listed_edge() {
        let pinned = pin("ghcr.io/acme/x", "d");
        let row = row("ghcr.io/acme/x", "1", &pinned);
        let record = PinRecord {
            pin: &pinned,
            row: Some(&row),
            binding: "x",
        };
        let evidence = edge_evidence(record, HashProof::Indirect, BTreeSet::new());
        assert!(!answers_listed_edge(record, &evidence, &id("ghcr.io/acme/x:2")));
    }

    /// A part's row whose own stamp records the bundle answers that bundle's edge under
    /// a proof, or on a stale hash only through a tag; a declaration or a row that
    /// contradicts itself answers none.
    #[test]
    fn edge_evidence_takes_stamped_edges_under_a_proof_or_a_tag() {
        let tagless = pin("ghcr.io/acme/x", "d");
        let tagged = pin("ghcr.io/acme/x:9", "d");
        let edge = id("ghcr.io/acme/x:1");
        let none = BTreeSet::new;

        for (pinned, proof, takes) in [
            (&tagless, HashProof::Indirect, true),
            (&tagged, HashProof::Indirect, true),
            (&tagless, HashProof::Stale, false),
            (&tagged, HashProof::Stale, true),
            (&tagless, HashProof::Direct, false),
            (&tagged, HashProof::Direct, false),
        ] {
            let evidence = edge_evidence(record(pinned), proof, none());
            assert_eq!(evidence.takes_stamped_edges, takes, "{proof:?} {pinned:?}");
            assert_eq!(answers_stamped_edge(record(pinned), &evidence, &edge), takes);
        }
        let elsewhere = EdgeEvidence::default();
        assert!(
            !answers_stamped_edge(record(&tagged), &elsewhere, &edge),
            "no part stamps, no stamped edge"
        );
        assert!(keeps_pin(
            record(&tagless),
            &edge_evidence(record(&tagless), HashProof::Indirect, none()),
            &[],
            &[],
            &[&edge]
        ));
        assert!(!keeps_pin(record(&tagless), &elsewhere, &[], &[], &[&edge]));
    }

    #[test]
    fn a_recorded_edge_answers_in_any_repository_and_an_unrecorded_one_does_not() {
        let pinned = pin("ghcr.io/aa/x:1", "d");
        let mirror = id("ghcr.io/zz/x:1");
        let evidence = EdgeEvidence {
            requests: set(std::slice::from_ref(&mirror)),
            ..EdgeEvidence::default()
        };
        assert!(answers_edge(record(&pinned), &evidence, &mirror));
        assert!(!answers_edge(record(&pinned), &EdgeEvidence::default(), &mirror));
    }

    #[test]
    fn a_row_answers_an_edge_no_recorded_row_holds() {
        let pinned = pin("ghcr.io/acme/x:9", "d");
        let row = row("ghcr.io/acme/x", "1", &pinned);
        let record = PinRecord {
            pin: &pinned,
            row: Some(&row),
            binding: "x",
        };
        assert!(answers_edge(record, &EdgeEvidence::default(), &id("ghcr.io/acme/x:1")));
    }

    #[test]
    fn a_claim_the_pin_answers_keeps_it_and_a_stranger_alone_does_not() {
        let pinned = pin("ghcr.io/acme/x:1", "d");
        let none = EdgeEvidence::default();
        let claims = [id("ghcr.io/acme/x:1"), id("ghcr.io/acme/x:7")];
        assert!(keeps(record(&pinned), &none, &claims));
        let digest_only = [at("ghcr.io/acme/x", "d")];
        assert!(keeps(record(&pinned), &none, &digest_only), "a digest names itself");
        let stranger = [id("ghcr.io/acme/x:7")];
        assert!(!keeps(record(&pinned), &none, &stranger));
    }

    #[test]
    fn a_recorded_digest_elsewhere_keeps_no_pin() {
        let pinned = pin("ghcr.io/acme/x:1", "d");
        let elsewhere = at("ghcr.io/acme/x", "other");
        let evidence = EdgeEvidence {
            requests: set(std::slice::from_ref(&elsewhere)),
            ..EdgeEvidence::default()
        };
        assert!(!keeps(record(&pinned), &evidence, &[elsewhere]));
    }

    #[test]
    fn check_reads_the_pins_tag_only_while_an_edge_names_it() {
        let pinned = pin("ghcr.io/acme/x:1", "d");
        let (one, two) = (id("ghcr.io/acme/x:1"), id("ghcr.io/acme/x:2"));
        let both = set(&[one.clone(), two.clone()]);
        assert_eq!(recorded_reference(both, &pinned, Some("1")), Some(one.clone()));
        let survivor = set(std::slice::from_ref(&two));
        assert_eq!(recorded_reference(survivor, &pinned, Some("1")), Some(two));
        assert_eq!(recorded_reference(BTreeSet::new(), &pinned, Some("1")), Some(one));
    }

    #[test]
    fn check_reads_a_digest_edge_before_any_tag() {
        let pinned = pin("ghcr.io/acme/x:1", "d");
        let held = at("ghcr.io/acme/x", "d");
        let edges = set(&[id("ghcr.io/acme/x:1"), held.clone()]);
        assert_eq!(recorded_reference(edges, &pinned, Some("1")), Some(held));
    }

    fn declared(skills: &[(&str, &str)]) -> DesiredSet {
        let skills = skills
            .iter()
            .map(|(name, reference)| ((*name).to_string(), DeclaredSource::Registry(id(reference))))
            .collect();
        DesiredSet::from_parts(skills, BTreeMap::new())
    }

    /// A lock of `rows` (lock TOML) whose hash proves `proof`.
    fn proven(proof: &DesiredSet, rows: &str) -> GrimoireLock {
        let hash = proof.declaration_hash_cached();
        let metadata = format!(
            "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\ndeclaration_hash = \"{hash}\"\n\
             generated_by = \"grim test\"\ngenerated_at = \"2026-01-01T00:00:00Z\"\n\n"
        );
        GrimoireLock::from_toml_str(&(metadata + rows)).expect("test lock parses")
    }

    fn skill_row(name: &str, pinned: &str, requires: &[&str]) -> String {
        let mut row = format!("[[skill]]\nname = \"{name}\"\npinned = \"{pinned}\"\n\n");
        for edge in requires {
            row.push_str(&format!(
                "[[skill.requires]]\nkind = \"skill\"\nname = \"x\"\nid = \"{edge}\"\n\n"
            ));
        }
        row
    }

    fn sha(c: char) -> String {
        format!("sha256:{}", c.to_string().repeat(64))
    }

    fn pins_of<'l>(lock: &'l GrimoireLock, name: &str) -> Vec<&'l PinnedIdentifier> {
        lock.skills
            .iter()
            .filter(|row| row.name == name)
            .filter_map(|row| row.source.pinned())
            .collect()
    }

    /// The proof vouched for the edges of the rows it reaches. A row nothing
    /// reaches never resolved its edge, so it lends the stamp no tag.
    #[test]
    fn a_stamp_takes_no_tag_from_a_row_the_proof_does_not_reach() {
        let proof = declared(&[("p", "ghcr.io/acme/p:1")]);
        let rows = [
            skill_row(
                "ghost",
                &format!("ghcr.io/acme/ghost:1@{}", sha('a')),
                &["ghcr.io/acme/x:0"],
            ),
            skill_row("p", &format!("ghcr.io/acme/p:1@{}", sha('a')), &["ghcr.io/acme/x:1"]),
            skill_row("x", &format!("ghcr.io/acme/x@{}", sha('b')), &[]),
        ];
        let previous = proven(&proof, &rows.concat());
        let mut lock = previous.clone();
        let reached = reached_rows(&proof, &previous, Some(&proof));

        stamp_kept_pins(&mut lock, &previous, &proof, reached.as_ref());

        let stamped = pins_of(&lock, "x");
        assert_eq!(stamped.len(), 1);
        assert_eq!(stamped[0].tag(), Some("1"), "the edge the proof reached: {stamped:?}");
    }

    /// A pin an edit keeps past the parents that asked for it takes the tag the
    /// proof saw them ask for: tagless on the stale lock the edit leaves, the next
    /// relock would resolve it afresh. An edge the edited lock still records comes
    /// first, since that relock reads it and a pin naming it keeps its digest.
    #[test]
    fn a_stamp_takes_an_edge_the_edit_took_only_where_none_survives() {
        let proof = declared(&[("p", "ghcr.io/acme/p:1"), ("q", "ghcr.io/acme/q:1")]);
        let rows = [
            skill_row("p", &format!("ghcr.io/acme/p:1@{}", sha('a')), &["ghcr.io/acme/x:2"]),
            skill_row("q", &format!("ghcr.io/acme/q:1@{}", sha('a')), &["ghcr.io/acme/x:1"]),
            skill_row("x", &format!("ghcr.io/acme/x@{}", sha('b')), &[]),
        ];
        let previous = proven(&proof, &rows.concat());
        let reached = reached_rows(&proof, &previous, Some(&proof));
        let stamped = |taken: &[&str]| {
            let mut lock = previous.clone();
            lock.skills.retain(|row| !taken.contains(&row.name.as_str()));
            stamp_kept_pins(&mut lock, &previous, &proof, reached.as_ref());
            pins_of(&lock, "x")[0].tag().map(str::to_string)
        };

        assert_eq!(stamped(&["q"]).as_deref(), Some("2"), "the edge that survives");
        assert_eq!(stamped(&["p", "q"]).as_deref(), Some("1"), "the smallest the proof saw");
    }

    /// Nothing says which of two rows for one key is current, so neither is a pin
    /// and neither takes a tag.
    #[test]
    fn a_stamp_leaves_a_key_listed_twice_alone() {
        let proof = declared(&[("x", "ghcr.io/acme/x:1")]);
        let rows = [
            skill_row("x", &format!("ghcr.io/acme/x@{}", sha('a')), &[]),
            skill_row("x", &format!("ghcr.io/acme/x@{}", sha('b')), &[]),
        ];
        let previous = proven(&proof, &rows.concat());
        let mut lock = previous.clone();
        let reached = reached_rows(&proof, &previous, Some(&proof));

        stamp_kept_pins(&mut lock, &previous, &proof, reached.as_ref());

        let tags: Vec<Option<&str>> = pins_of(&lock, "x").iter().map(|pin| pin.tag()).collect();
        assert_eq!(tags, [None, None]);
    }

    #[test]
    fn check_of_a_tagless_pin_reads_the_smallest_edge() {
        let pinned = pin("ghcr.io/acme/x", "d");
        let edges = set(&[id("ghcr.io/acme/x:2"), id("ghcr.io/acme/x:1")]);
        assert_eq!(recorded_reference(edges, &pinned, None), Some(id("ghcr.io/acme/x:1")));
        assert_eq!(recorded_reference(BTreeSet::new(), &pinned, None), None);
    }
}
