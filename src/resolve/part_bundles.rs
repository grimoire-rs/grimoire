// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! A marketplace part's bundle state, decided once after the graph.
//!
//! A part keeps no `[[bundle]]` row: a bundle's pin lives in the stamps of the rows it
//! provides, as `<tag>@<digest>`. The relock reads the part's stamps as carry, so the
//! graph reads a bundle the part records at its recorded digest (`Carry::part`), and
//! this step sorts what the graph read: which bundles a pick walks, which it writes a
//! digest for, and which rows it keeps as the previous part holds them. Every consumer
//! reads [`PartBundles`], so none recomputes a selection or the stamps.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use super::members::fetch_members_blob;
use super::resolve_error::{MAX_TRANSITIVE_NODES, ResolveError, ResolveErrorKind};
use super::resolve_options::ResolveOptions;
use super::resolver::{pin_name, retry_chain};
use crate::config::declaration::{DeclaredSource, DesiredSet};
use crate::lock::closure::closure_nodes;
use crate::lock::effective_set::{NodeKey, reached_rows};
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::locked_artifact::{BundleProvenance, LockedArtifact};
use crate::lock::locked_bundle::{LockedBundle, LockedBundleSource};
use crate::lock::part_stamps::{PartStamps, split_stamp};
use crate::lock::pin_rule::{self, HashProof, PinRecord, bundle_provenance_tag};
use crate::lock::refresh::Refresh;
use crate::oci::access::OciAccess;
use crate::oci::access::error::{AccessError, AccessErrorKind};
use crate::oci::bundle::BundleMember;
use crate::oci::identifier::is_valid_oci_tag;
use crate::oci::manifest::LayerLayout;
use crate::oci::reference::ArtifactRef;
use crate::oci::{ArtifactKind, Digest, Identifier, PinnedIdentifier};

/// A part relock's bundle state (owner decision on how a part records a bundle).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartBundles {
    /// Bundles whose members this run resolved: those a pick selects and every one they
    /// nest, and on any other write all of them.
    pub walked: BTreeSet<String>,
    /// Bundles whose digest this run writes into the stamps.
    pub recorded: BTreeSet<String>,
    /// Rows a pick keeps as the previous part holds them: those outside its selection's
    /// closure, and the requirements of those. Empty off a pick.
    pub held_rows: BTreeSet<NodeKey>,
}

/// What [`sort_part_bundles`] reads besides the lock.
pub(crate) struct PartInputs<'a> {
    /// The part as it stands, stamps stripped; `None` for a plugin with no part.
    pub previous: Option<&'a GrimoireLock>,
    /// The declaration the part's hash matches: what its edges prove about a key.
    pub proof: Option<&'a DesiredSet>,
    /// The digests the part's stamps record.
    pub stamps: &'a PartStamps,
    pub refresh: &'a Refresh,
    /// The plugin's set, after the include kinds were decided.
    pub set: &'a DesiredSet,
    pub access: &'a Arc<dyn OciAccess>,
    pub options: &'a ResolveOptions,
}

/// The bundles of `bundles` a part pick moves: those `refresh` selects and every
/// bundle they nest, at any depth.
pub(crate) fn selected_bundles(bundles: &[LockedBundle], refresh: &Refresh) -> BTreeSet<String> {
    let mut selected = BTreeSet::new();
    let picked = |bundle: &&LockedBundle| refresh.selects(&(ArtifactKind::Bundle, bundle.name.clone()));
    let mut queue: Vec<&LockedBundle> = bundles.iter().filter(picked).collect();
    while let Some(bundle) = queue.pop() {
        if selected.insert(bundle.name.clone()) {
            for member in bundle.members.iter().filter(|m| m.kind == ArtifactKind::Bundle) {
                queue.extend(bundles.iter().filter(|b| b.name == member.name));
            }
        }
    }
    selected
}

/// Whether `bundle` has a digest to record: a registry bundle requested by tag.
pub(crate) fn records_a_digest(bundle: &LockedBundle) -> bool {
    bundle.pinned().is_some() && is_valid_oci_tag(&bundle.provenance_pair().1)
}

/// Sort the bundles `lock` read, narrow `lock` to what a pick may change, and refuse a
/// pick that would keep a dependent beside another version of a row, or of a bundle, it
/// requires.
///
/// `lock.bundles` stays every bundle the run read: each takes part in the member check.
///
/// # Errors
///
/// [`ResolveErrorKind::HeldRequirement`] for a held row whose requirement, an artifact or
/// a bundle, the pick moved past; [`AccessErrorKind::OfflineMiss`] (as
/// [`ResolveErrorKind::RegistryUnreachable`]) when a pick needs the recorded list of a
/// bundle the store lacks while offline.
pub(crate) async fn sort_part_bundles(
    lock: &mut GrimoireLock,
    inputs: PartInputs<'_>,
) -> Result<PartBundles, ResolveError> {
    let PartInputs {
        previous,
        stamps,
        refresh,
        ..
    } = inputs;
    let names: BTreeSet<String> = lock.bundles.iter().map(|b| b.name.clone()).collect();
    let names_refresh = matches!(refresh, Refresh::Names(_));
    let selected = if names_refresh {
        selected_bundles(&lock.bundles, refresh)
    } else {
        names
    };
    let stamped_before: BTreeSet<(String, String)> = previous
        .into_iter()
        .flat_map(GrimoireLock::iter_artifacts)
        .flat_map(|row| &row.bundles)
        .map(|stamp| (stamp.repo.clone(), stamp.tag.clone()))
        .collect();
    let recorded: BTreeSet<String> = lock
        .bundles
        .iter()
        .filter(|bundle| records_a_digest(bundle))
        .filter(|bundle| {
            if !names_refresh {
                return true;
            }
            let pair = bundle.provenance_pair();
            stamps.pins.contains_key(&pair)
                || refresh.selects(&(ArtifactKind::Bundle, bundle.name.clone()))
                || (selected.contains(&bundle.name) && !stamped_before.contains(&pair))
        })
        .map(|bundle| bundle.name.clone())
        .collect();
    let walked: BTreeSet<String> = lock
        .bundles
        .iter()
        .filter(|bundle| selected.contains(&bundle.name))
        .map(|bundle| bundle.name.clone())
        .collect();
    let mut part = PartBundles {
        walked,
        recorded,
        held_rows: BTreeSet::new(),
    };
    if let (true, Some(previous)) = (names_refresh, previous) {
        part.held_rows = hold_outside_selection(lock, previous, &stamped_before, &part.walked, &inputs).await?;
    }
    Ok(part)
}

/// Narrow `lock` to what a pick may change: the rows its selection reaches in either
/// part come from `lock`, and every other row stays as `previous` holds it. A bundle the
/// pick selects may have been republished as another kind, or lost members, so the
/// previous closure of each selected name is rebuilt from the part's own stamps: the
/// list the part recorded for it, and for every recorded bundle that list nests.
async fn hold_outside_selection(
    lock: &mut GrimoireLock,
    previous: &GrimoireLock,
    stamped_before: &BTreeSet<(String, String)>,
    walked: &BTreeSet<String>,
    inputs: &PartInputs<'_>,
) -> Result<BTreeSet<NodeKey>, ResolveError> {
    let mut before = previous.clone();
    before.bundles = previous_bundle_rows(lock, previous, stamped_before, inputs).await?;
    let mut reached = closure_nodes(lock, inputs.refresh);
    reached.extend(closure_nodes(&before, inputs.refresh));

    let mut held: BTreeSet<NodeKey> = previous
        .iter_artifacts()
        .map(|row| (row.kind, row.name.clone()))
        .filter(|key| !reached.contains(key))
        .collect();
    let mut rows: Vec<LockedArtifact> = lock
        .iter_artifacts()
        .filter(|row| reached.contains(&(row.kind, row.name.clone())))
        .chain(
            previous
                .iter_artifacts()
                .filter(|row| held.contains(&(row.kind, row.name.clone()))),
        )
        .cloned()
        .collect();
    // A row the previous part holds keeps what it requires: a requirement the narrowed
    // lock lacks stays as the part holds it, recursively, as a project keeps a carried
    // dependent's requirement. The edges are those at the held pin, wherever the part
    // records them.
    let mut edges: BTreeMap<NodeKey, Vec<BundleMember>> = BTreeMap::new();
    loop {
        for key in &held {
            if edges.contains_key(key) {
                continue;
            }
            let row = previous
                .iter_artifacts()
                .find(|row| (row.kind, row.name.clone()) == *key);
            if let Some(row) = row {
                edges.insert(key.clone(), held_edges(row, lock, inputs).await);
            }
        }
        let present: BTreeSet<NodeKey> = rows.iter().map(|row| (row.kind, row.name.clone())).collect();
        let missing: BTreeSet<NodeKey> = edges
            .values()
            .flatten()
            .map(|edge| (edge.kind, edge.name.clone()))
            .filter(|key| !present.contains(key))
            .collect();
        let kept: Vec<LockedArtifact> = previous
            .iter_artifacts()
            .filter(|row| missing.contains(&(row.kind, row.name.clone())))
            .cloned()
            .collect();
        if kept.is_empty() {
            break;
        }
        held.extend(kept.iter().map(|row| (row.kind, row.name.clone())));
        rows.extend(kept);
    }

    let bundles = reached_bundles(lock, &reached, &held, walked);
    let run = Run {
        lock,
        previous,
        previous_bundles: &before.bundles,
        reached: &reached,
        held: &held,
        bundles: &bundles,
    };
    refuse_held_requirements(&run, &edges, inputs).await?;

    for (kind, table) in lock.tables_mut() {
        *table = rows.iter().filter(|row| row.kind == kind).cloned().collect();
        table.sort_by(|a, b| a.name.cmp(&b.name));
    }
    Ok(held)
}

/// The bundles the pick's selection reaches: `walked`, those it selects and nests, and every
/// bundle a reached row `lock` takes into the part names in its `requires`, with the bundles
/// those nest. A bundle member's own requirements are among the rows, so one pass over them
/// follows a requirement at any depth. A held row keeps its old requirements, so it adds none.
fn reached_bundles(
    lock: &GrimoireLock,
    reached: &BTreeSet<NodeKey>,
    held: &BTreeSet<NodeKey>,
    walked: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut found = walked.clone();
    let mut queue: Vec<&str> = lock
        .iter_artifacts()
        .filter(|row| {
            let key = (row.kind, row.name.clone());
            reached.contains(&key) && !held.contains(&key)
        })
        .flat_map(|row| &row.requires)
        .filter(|edge| edge.kind == ArtifactKind::Bundle)
        .map(|edge| edge.name.as_str())
        .collect();
    while let Some(name) = queue.pop() {
        if !found.insert(name.to_string()) {
            continue;
        }
        for bundle in lock.bundles.iter().filter(|bundle| bundle.name == name) {
            queue.extend(
                bundle
                    .members
                    .iter()
                    .filter(|member| member.kind == ArtifactKind::Bundle)
                    .map(|member| member.name.as_str()),
            );
        }
    }
    found
}

/// The bundle rows the previous part's closure is walked from: every bundle `lock` read at
/// the digest the part records for it, then a row for each selected name whose stamp some
/// previous row carries, and for each recorded bundle those lists nest. `closure_nodes`
/// reads a row's `(repo, tag)` and members only, so the pin of a row built here is a
/// placeholder where no digest is recorded.
async fn previous_bundle_rows(
    lock: &GrimoireLock,
    previous: &GrimoireLock,
    stamped_before: &BTreeSet<(String, String)>,
    inputs: &PartInputs<'_>,
) -> Result<Vec<LockedBundle>, ResolveError> {
    let PartInputs { stamps, refresh, .. } = inputs;
    let mut rows: BTreeMap<String, LockedBundle> = BTreeMap::new();
    for bundle in &lock.bundles {
        let at_record = bundle
            .pinned()
            .zip(stamps.pins.get(&bundle.provenance_pair()))
            .is_some_and(|(pinned, recorded)| pinned.digest() == *recorded);
        if at_record {
            rows.insert(bundle.name.clone(), bundle.clone());
        }
    }
    // The selected names, each with the request that names its bundle: the include of any
    // kind it has now, and the bundle node the graph read for it.
    let mut asked: Vec<(String, Identifier)> = Vec::new();
    if let Refresh::Names(selected) = refresh {
        for s in selected {
            for kind in ArtifactKind::ALL {
                if s.kind.is_some_and(|known| known != kind) {
                    continue;
                }
                if let Some(DeclaredSource::Registry(id)) = inputs.set.table(kind).get(&s.name) {
                    asked.push((s.name.clone(), id.clone()));
                }
            }
            for bundle in lock
                .bundles
                .iter()
                .filter(|b| s.matches(&(ArtifactKind::Bundle, b.name.clone())))
            {
                if let Some(id) = bundle_request(bundle) {
                    asked.push((bundle.name.clone(), id));
                }
            }
        }
    }
    let mut queue: Vec<(String, Identifier)> = asked
        .into_iter()
        .filter(|(_, id)| stamped_before.contains(&pair_of(id)))
        .collect();
    let mut seen = BTreeSet::new();
    while let Some((name, id)) = queue.pop() {
        if rows.contains_key(&name) || !seen.insert(name.clone()) {
            continue;
        }
        let Some(row) = previous_row(&name, &id, lock, inputs).await? else {
            continue;
        };
        // The bundles this list nests, read at the digest the part records for them.
        for member in row.members.iter().filter(|m| m.kind == ArtifactKind::Bundle) {
            let anchor = row.pinned().map(PinnedIdentifier::as_identifier);
            if let Some(nested) = member
                .absolute_id(anchor)
                .filter(|id| stamps.pins.contains_key(&pair_of(id)))
            {
                queue.push((member.name.clone(), nested));
            }
        }
        rows.insert(name, row);
    }
    let _ = previous;
    Ok(rows.into_values().collect())
}

/// The row `name` had in the previous part for the request `id`: the list at the recorded
/// digest, which `lock` holds when the graph read the bundle there and the store or
/// registry holds otherwise; with no digest recorded, the graph's row if it read one, else
/// a row with no members.
async fn previous_row(
    name: &str,
    id: &Identifier,
    lock: &GrimoireLock,
    inputs: &PartInputs<'_>,
) -> Result<Option<LockedBundle>, ResolveError> {
    let pair = pair_of(id);
    let Some(recorded) = inputs.stamps.pins.get(&pair) else {
        let read = lock.bundles.iter().find(|b| b.name == name);
        return Ok(read.cloned().or_else(|| placeholder_row(name, id, Vec::new())));
    };
    let Ok(pinned) = PinnedIdentifier::try_from(id.clone_with_digest(recorded.clone())) else {
        return Ok(None);
    };
    let members = match members_at(&pinned, id, ArtifactKind::Bundle, inputs).await {
        Ok(members) => members,
        // The resolver reports a store miss as unreachable (69); offline the store is
        // the only source, so the pick refuses as the release did (81).
        Err(ResolveErrorKind::RegistryUnreachable(e)) if matches!(e.kind, AccessErrorKind::OfflineMiss) => {
            let reference = ArtifactRef::registry(ArtifactKind::Bundle, name.to_string(), id.clone());
            let at = pinned.as_identifier().clone();
            let err = AccessError::with_identifier(at, AccessErrorKind::OfflineMiss);
            return Err(ResolveError::new(reference, ResolveErrorKind::RegistryUnreachable(err)));
        }
        Err(_) => Vec::new(),
    };
    Ok(Some(LockedBundle {
        name: name.to_string(),
        source: LockedBundleSource::Registry {
            repo: pair.0,
            tag: pair.1,
            pinned,
        },
        members,
        transitive: false,
    }))
}

/// A bundle row with `members` and a pin no one reads: the closure of a lock looks only
/// at its `(repo, tag)` and members.
fn placeholder_row(name: &str, id: &Identifier, members: Vec<BundleMember>) -> Option<LockedBundle> {
    let digest = crate::oci::Algorithm::Sha256.hash(b"");
    let pinned = PinnedIdentifier::try_from(id.clone_with_digest(digest)).ok()?;
    let (repo, tag) = pair_of(id);
    Some(LockedBundle {
        name: name.to_string(),
        source: LockedBundleSource::Registry { repo, tag, pinned },
        members,
        transitive: false,
    })
}

/// `(repo, tag)` as a part's stamp names the bundle `id` asks for.
fn pair_of(id: &Identifier) -> (String, String) {
    (id.registry_repository(), bundle_provenance_tag(id))
}

/// The request a bundle row read by the graph answered: its repository at its tag.
fn bundle_request(bundle: &LockedBundle) -> Option<Identifier> {
    let (repo, tag) = bundle.provenance_pair();
    bundle.pinned()?;
    Identifier::parse(&format!("{repo}:{tag}")).ok()
}

/// The members `pinned` lists, each resolved against `id` as the resolve resolves them:
/// a bundle's list or an artifact's `requires`; none for a manifest that holds none.
///
/// # Errors
///
/// [`ResolveErrorKind::RegistryUnreachable`] when a read fails or times out, and whatever
/// [`fetch_members_blob`] refuses.
async fn members_at(
    pinned: &PinnedIdentifier,
    id: &Identifier,
    kind: ArtifactKind,
    inputs: &PartInputs<'_>,
) -> Result<Vec<BundleMember>, ResolveErrorKind> {
    let read = async {
        let fetched = inputs.access.fetch_manifest(&pinned.strip_advisory()).await;
        let manifest = fetched.map_err(ResolveErrorKind::RegistryUnreachable)?;
        let layout = manifest.as_ref().map(|manifest| manifest.layout(kind));
        let layer = match layout {
            Some(Ok(LayerLayout::BundleMembers(layer))) => layer,
            Some(Ok(LayerLayout::ContentWithRequires { requires, .. })) => requires,
            _ => return Ok(Vec::new()),
        };
        let document = fetch_members_blob(inputs.access, id, layer).await?;
        let mut members: Vec<BundleMember> = document
            .members
            .into_iter()
            .filter_map(|member| {
                let absolute = member.absolute_id(Some(id))?;
                Some(BundleMember {
                    id: absolute.to_string(),
                    ..member
                })
            })
            .collect();
        members.sort_by(|a, b| (a.kind, &a.name, &a.id).cmp(&(b.kind, &b.name, &b.id)));
        Ok(members)
    };
    match tokio::time::timeout(inputs.options.per_artifact_timeout, read).await {
        Ok(result) => result,
        Err(_elapsed) => Err(ResolveErrorKind::ResolveTimeout),
    }
}

/// The edges a held row records at its held pin: its own `requires`, else the row the
/// graph read at that digest, else its manifest and requires layer, store first. A read
/// that fails gives no edges: only what was seen counts.
async fn held_edges(row: &LockedArtifact, lock: &GrimoireLock, inputs: &PartInputs<'_>) -> Vec<BundleMember> {
    let Some(pin) = row.source.pinned() else {
        return Vec::new();
    };
    if !row.requires.is_empty() {
        return row.requires.clone();
    }
    let read = lock
        .iter_artifacts()
        .find(|r| (r.kind, &r.name) == (row.kind, &row.name))
        .filter(|r| r.source.pinned().is_some_and(|p| p.digest() == pin.digest()));
    match read {
        Some(read) => read.requires.clone(),
        None => {
            let request = pin.as_identifier().clone();
            members_at(pin, &request, row.kind, inputs).await.unwrap_or_default()
        }
    }
}

/// What the pick did, for [`refuse_held_requirements`] to find a held row's requirement in.
struct Run<'a> {
    /// The lock the run built, before it is narrowed.
    lock: &'a GrimoireLock,
    previous: &'a GrimoireLock,
    /// The previous part's bundle rows, rebuilt from its stamps.
    previous_bundles: &'a [LockedBundle],
    /// The rows the pick's selection reaches in either part.
    reached: &'a BTreeSet<NodeKey>,
    held: &'a BTreeSet<NodeKey>,
    /// The bundles the pick's selection reaches: those it selects and the ones they nest,
    /// and those a reached row requires.
    bundles: &'a BTreeSet<String>,
}

/// A row the pick moved: the pin as the run holds it, and as the previous part shows it,
/// which is unknown for a bundle the part records no digest for.
struct Moved<'a> {
    was: Option<PinRecord<'a>>,
    now: PinRecord<'a>,
}

impl<'a> Run<'a> {
    /// `key` as the run moved it, if it did. An artifact moved when its pin's digest differs
    /// from the previous part's. A bundle moved when the pick's selection reaches it, by
    /// selecting or nesting it or through a reached row's `requires`, and the part does not
    /// record the digest the run read, so a bundle the pick left alone, or one it kept at its
    /// recorded digest, is not moved.
    fn moved(&self, key: &'a NodeKey, stamps: &PartStamps) -> Option<Moved<'a>> {
        if key.0 == ArtifactKind::Bundle {
            return self.moved_bundle(key, stamps);
        }
        if !self.reached.contains(key) || self.held.contains(key) {
            return None;
        }
        let find = |lock: &'a GrimoireLock| {
            lock.iter_artifacts()
                .find(|row| row.kind == key.0 && row.name == key.1)
                .and_then(|row| row.source.pinned())
        };
        let (now, was) = (find(self.lock)?, find(self.previous)?);
        (now.digest() != was.digest()).then_some(Moved {
            was: Some(PinRecord {
                pin: was,
                row: None,
                binding: &key.1,
            }),
            now: PinRecord {
                pin: now,
                row: None,
                binding: &key.1,
            },
        })
    }

    fn moved_bundle(&self, key: &'a NodeKey, stamps: &PartStamps) -> Option<Moved<'a>> {
        if !self.bundles.contains(&key.1) {
            return None;
        }
        let row = self.lock.bundles.iter().find(|bundle| bundle.name == key.1)?;
        let now = PinRecord {
            pin: row.pinned()?,
            row: Some(row),
            binding: &key.1,
        };
        // Only a digest the part records shows where the bundle stood; the row a pick
        // rebuilds for one it records none for is the one the run read.
        let was = self.previous_bundles.iter().find(|bundle| {
            bundle.name == key.1 && stamps.pins.get(&bundle.provenance_pair()) == Some(&bundle.content_digest())
        });
        let was = was.and_then(|row| {
            Some(PinRecord {
                pin: row.pinned()?,
                row: Some(row),
                binding: &key.1,
            })
        });
        match &was {
            Some(was) if was.pin.digest() == now.pin.digest() => None,
            _ => Some(Moved { was, now }),
        }
    }

    /// Whether the part's own record shows the pick moved the bundle the edge `id` names to
    /// `now`. A part that records a digest for the `(repo, tag)` shows it when the run read
    /// another; one that records none shows it when a row it stamps through that pair is a
    /// row the pick moved. A part that stamps the pair nowhere shows nothing.
    fn record_moved(&self, id: &Identifier, now: PinRecord<'_>, stamps: &PartStamps) -> bool {
        let pair = pair_of(id);
        if let Some(recorded) = stamps.pins.get(&pair) {
            return *recorded != now.pin.digest();
        }
        self.previous
            .iter_artifacts()
            .filter(|row| {
                row.bundles
                    .iter()
                    .any(|stamp| (&stamp.repo, &stamp.tag) == (&pair.0, &pair.1))
            })
            .any(|row| self.moved(&(row.kind, row.name.clone()), stamps).is_some())
    }
}

/// Refuse a pick that keeps a dependent beside another version of a row or bundle it
/// requires.
///
/// For each held row, `edges` are those it records at its held pin. An edge onto a row or
/// bundle the run moved conflicts when the moved pin does not answer it: it names another
/// digest; or the previous part shows the pin it moved from answered it; or its tag, read
/// now, resolves elsewhere. An edge onto a bundle is judged on the run's bundle of that
/// name, and so is each bundle its list nests now. A direct include shadows an edge onto
/// its key. A tag or list that cannot be read refuses nothing: only proof refuses.
async fn refuse_held_requirements(
    run: &Run<'_>,
    edges: &BTreeMap<NodeKey, Vec<BundleMember>>,
    inputs: &PartInputs<'_>,
) -> Result<(), ResolveError> {
    let recorded = pin_rule::reached_requests(
        run.previous,
        reached_rows(inputs.set, run.previous, inputs.proof).as_ref(),
    );
    let mut lists = Lists::default();
    for key in run.held {
        let Some(row) = run
            .previous
            .iter_artifacts()
            .find(|row| (row.kind, row.name.clone()) == *key)
        else {
            continue;
        };
        let Some(pin) = row.source.pinned() else { continue };
        for edge in edges.get(key).into_iter().flatten() {
            let Some(id) = edge.absolute_id(Some(pin.as_identifier())) else {
                continue;
            };
            let target = (edge.kind, edge.name.clone());
            let (required, parting) = match judge_edge(run, &id, &target, &recorded, inputs).await {
                Judged::Shadowed => continue,
                Judged::Parting(parting) => (short_ref(&id), parting),
                Judged::Answered if target.0 == ArtifactKind::Bundle && !run.bundles.is_empty() => {
                    match nested_parting(run, &id, &recorded, inputs, &mut lists).await {
                        Some(found) => found,
                        None => continue,
                    }
                }
                Judged::Answered => continue,
            };
            let dependent = ArtifactRef::registry(row.kind, row.name.clone(), request_of(pin));
            return Err(ResolveError::new(
                dependent,
                ResolveErrorKind::HeldRequirement {
                    required,
                    moved: parting.moved,
                    to: parting.to,
                    dependent: row.name.clone(),
                    retry: String::new(),
                },
            ));
        }
    }
    Ok(())
}

/// What a held row's edge came to.
enum Judged {
    /// A direct include of the key shadows the edge and everything below it.
    Shadowed,
    /// The pick left the edge's target alone, or moved it to a pin that answers the edge.
    Answered,
    Parting(Parting),
}

/// The row or bundle a pick moves past an edge, as the error names it.
struct Parting {
    moved: String,
    to: String,
}

/// Judge the edge `id` of a held row onto `target`: the one rule for a direct edge and for
/// each bundle a requirement nests, so the proof it takes cannot differ between them.
async fn judge_edge(
    run: &Run<'_>,
    id: &Identifier,
    target: &NodeKey,
    recorded: &BTreeMap<NodeKey, BTreeSet<Identifier>>,
    inputs: &PartInputs<'_>,
) -> Judged {
    if inputs.set.table(target.0).contains_key(&target.1) {
        return Judged::Shadowed;
    }
    let Some(moved) = run.moved(target, inputs.stamps) else {
        return Judged::Answered;
    };
    let answered = moved
        .was
        .is_some_and(|was| previous_pin_answered(id, target, was, recorded, inputs));
    if !conflicts(run, id, target, &moved, answered, inputs).await {
        return Judged::Answered;
    }
    Judged::Parting(Parting {
        moved: pin_name(target.0, &target.1),
        to: moved_to(moved.now),
    })
}

/// The lists one run has read through, so a bundle two held rows reach is read once.
#[derive(Default)]
struct Lists {
    seen: BTreeSet<Identifier>,
    reads: usize,
}

/// The first bundle below the held requirement `via` that the pick moves past, with the
/// requirement it came through. Every list is read as its edge names it now, and each
/// bundle it nests is judged as a direct edge is, whether or not the pick moved it.
async fn nested_parting(
    run: &Run<'_>,
    via: &Identifier,
    recorded: &BTreeMap<NodeKey, BTreeSet<Identifier>>,
    inputs: &PartInputs<'_>,
    lists: &mut Lists,
) -> Option<(String, Parting)> {
    if !lists.seen.insert(via.clone()) {
        return None;
    }
    let mut queue = vec![via.clone()];
    while let Some(parent) = queue.pop() {
        let Some(members) = list_now(run, &parent, inputs, lists).await else {
            continue;
        };
        for member in members.iter().filter(|member| member.kind == ArtifactKind::Bundle) {
            let Some(id) = member.absolute_id(Some(&parent)) else {
                continue;
            };
            if !lists.seen.insert(id.clone()) {
                continue;
            }
            let target = (ArtifactKind::Bundle, member.name.clone());
            match judge_edge(run, &id, &target, recorded, inputs).await {
                Judged::Shadowed => {}
                Judged::Answered => queue.push(id),
                Judged::Parting(parting) => {
                    return Some((format!("{} through {}", short_ref(&id), short_ref(via)), parting));
                }
            }
        }
    }
    None
}

/// The members of the bundle `id` names now: the row the run read for that request, else
/// the list at its digest, else at the digest its tag resolves to. A read that fails, times
/// out or passes the cap gives none.
async fn list_now(
    run: &Run<'_>,
    id: &Identifier,
    inputs: &PartInputs<'_>,
    lists: &mut Lists,
) -> Option<Vec<BundleMember>> {
    let pair = pair_of(id);
    let named = id.digest();
    let read = run.lock.bundles.iter().find(|bundle| {
        bundle.provenance_pair() == pair && named.as_ref().is_none_or(|digest| *digest == bundle.content_digest())
    });
    if let Some(read) = read {
        return Some(read.members.clone());
    }
    if lists.reads >= MAX_TRANSITIVE_NODES {
        return None;
    }
    lists.reads += 1;
    let digest = match named {
        Some(digest) => digest,
        None => {
            let reference = ArtifactRef::registry(ArtifactKind::Bundle, id.name().to_string(), id.clone());
            let tag = tokio::time::timeout(
                inputs.options.per_artifact_timeout,
                retry_chain(&reference, id, inputs.access, inputs.options),
            )
            .await;
            tag.ok()?.ok()?
        }
    };
    let pinned = PinnedIdentifier::try_from(id.clone_with_digest(digest)).ok()?;
    members_at(&pinned, id, ArtifactKind::Bundle, inputs).await.ok()
}

/// Where the pick moves a row or bundle, as the error names it: a bundle by the request
/// its row records, since its pin carries a digest alone.
fn moved_to(now: PinRecord<'_>) -> String {
    match now.row.and_then(bundle_request) {
        Some(request) => short_ref(&request),
        None => short_ref(now.pin.as_identifier()),
    }
}

/// Whether the previous part shows that `was`, the pin the run moved, answered the edge
/// `id` onto `target`: the pin rule's evidence under the part's hash, which gives a key a
/// declaration holds directly none, and a recorded edge only where the proof shows it.
fn previous_pin_answered(
    id: &Identifier,
    target: &NodeKey,
    was: PinRecord<'_>,
    recorded: &BTreeMap<NodeKey, BTreeSet<Identifier>>,
    inputs: &PartInputs<'_>,
) -> bool {
    let requests = recorded.get(target).cloned().unwrap_or_default();
    let stamped = inputs.stamps.rows.contains_key(target);
    let evidence = pin_rule::stamped_edge_evidence(was, HashProof::of(inputs.proof, target), requests, stamped);
    pin_rule::answers_edge(was, &evidence, id)
}

/// Whether the edge `id` of a held row, onto the row or bundle the run moved as `moved`,
/// settles on another digest than `moved.now`. `answered` is whether the previous part shows
/// the pin it moved from answered the edge. A tag that cannot be read leaves a bundle with no
/// previous row to the part's own record ([`Run::record_moved`]).
async fn conflicts(
    run: &Run<'_>,
    id: &Identifier,
    target: &NodeKey,
    moved: &Moved<'_>,
    answered: bool,
    inputs: &PartInputs<'_>,
) -> bool {
    let now = moved.now;
    if let Some(digest) = id.digest() {
        return digest != now.pin.digest();
    }
    if now.names(id) {
        return false;
    }
    // The pick moved the pin that answered the edge.
    if answered {
        return true;
    }
    let reference = ArtifactRef::registry(target.0, target.1.clone(), id.clone());
    let read = tokio::time::timeout(
        inputs.options.per_artifact_timeout,
        retry_chain(&reference, id, inputs.access, inputs.options),
    )
    .await;
    match read {
        Ok(Ok(digest)) => digest != now.pin.digest(),
        Ok(Err(_)) | Err(_) => {
            target.0 == ArtifactKind::Bundle && moved.was.is_none() && run.record_moved(id, now, inputs.stamps)
        }
    }
}

/// `id` as a request: its tag, or the repository alone for a digest.
fn request_of(pin: &PinnedIdentifier) -> Identifier {
    let id = pin.as_identifier();
    match id.tag() {
        Some(tag) => {
            Identifier::new_registry(id.repository().to_string(), id.registry().to_string()).clone_with_tag(tag)
        }
        None => id.clone(),
    }
}

/// `name:tag`, or `name@short-digest` for a request that names a digest.
fn short_ref(id: &Identifier) -> String {
    match (id.tag(), id.digest()) {
        (Some(tag), _) => format!("{}:{tag}", id.name()),
        (None, Some(digest)) => format!("{}@{}", id.name(), digest.to_short_string()),
        (None, None) => id.name().to_string(),
    }
}

impl PartBundles {
    /// The rows of `original` a pick kept as the previous part holds them, by node.
    pub(crate) fn kept_rows<'a>(&self, original: &'a GrimoireLock) -> BTreeMap<NodeKey, &'a LockedArtifact> {
        original
            .iter_artifacts()
            .map(|row| ((row.kind, row.name.clone()), row))
            .filter(|(key, _)| self.held_rows.contains(key))
            .collect()
    }

    /// Write the part's stamps: the stamp walk below the bundles no row carries, the
    /// digest each recorded bundle resolved to as `<tag>@<digest>`, and the rows a pick
    /// holds as `original` has them, then clear `lock.bundles`, since a part keeps no row
    /// for a bundle.
    pub(crate) fn write_stamps(&self, lock: &mut GrimoireLock, original: Option<&GrimoireLock>) {
        let retained: Vec<&LockedBundle> = lock
            .bundles
            .iter()
            .filter(|bundle| self.walked.contains(&bundle.name) || self.recorded.contains(&bundle.name))
            .collect();
        let mut pins: BTreeMap<(String, String), Option<Digest>> = BTreeMap::new();
        for bundle in lock.bundles.iter().filter(|b| self.recorded.contains(&b.name)) {
            let Some(pinned) = bundle.pinned() else { continue };
            let digest = pinned.digest();
            pins.entry(bundle.provenance_pair())
                .and_modify(|seen| {
                    if seen.as_ref() != Some(&digest) {
                        *seen = None;
                    }
                })
                .or_insert(Some(digest));
        }
        let below = stamps_below_unstamped_bundles(lock, &retained);
        let selected: BTreeSet<(String, String)> = lock
            .bundles
            .iter()
            .filter(|b| self.walked.contains(&b.name) && self.recorded.contains(&b.name))
            .map(LockedBundle::provenance_pair)
            .collect();
        for row in lock.iter_artifacts_mut() {
            if let Some(stamps) = below.get(&(row.kind, row.name.clone())) {
                row.bundles.extend(stamps.iter().cloned());
            }
            row.bundles.sort_by(|a, b| (&a.repo, &a.tag).cmp(&(&b.repo, &b.tag)));
            row.bundles.dedup();
            for stamp in &mut row.bundles {
                if let Some(Some(digest)) = pins.get(&(stamp.repo.clone(), stamp.tag.clone())) {
                    stamp.tag = format!("{}@{digest}", stamp.tag);
                }
            }
        }
        // A row a pick left as the part holds it keeps the part's own stamps: re-stamped, a
        // stamp no run recorded a digest on would gain one. A stamp the row lacks from a
        // bundle the pick moves is this run's record: dropped, none holds it.
        let kept = original.map(|original| self.kept_rows(original)).unwrap_or_default();
        let pair = |stamp: &BundleProvenance| {
            (
                stamp.repo.clone(),
                stamp.tag.split('@').next().unwrap_or_default().to_string(),
            )
        };
        for row in lock.iter_artifacts_mut() {
            let Some(original) = kept.get(&(row.kind, row.name.clone())) else {
                continue;
            };
            let lacks = |stamp: &&BundleProvenance| !original.bundles.iter().any(|o| pair(o) == pair(stamp));
            let added: Vec<BundleProvenance> = row
                .bundles
                .iter()
                .filter(|s| selected.contains(&pair(s)))
                .filter(lacks)
                .cloned()
                .collect();
            *row = (*original).clone();
            if !added.is_empty() {
                row.bundles.extend(added);
                row.bundles.sort_by(|a, b| (&a.repo, &a.tag).cmp(&(&b.repo, &b.tag)));
            }
        }
        settle_digests(lock, &pins);
        lock.bundles.clear();
    }
}

/// Leave no bundle pair recording two digests. A row restored from the previous part keeps
/// its own stamp, which may record a digest this run replaced, and `read_stamps` drops a
/// pair whose stamps disagree, with the bundle's pin. The digest the run recorded stays
/// where it is written; a stamp at any other digest of a contested pair loses its digest,
/// and when the run recorded none the pair records none.
fn settle_digests(lock: &mut GrimoireLock, pins: &BTreeMap<(String, String), Option<Digest>>) {
    let mut digests: BTreeMap<(String, String), BTreeSet<Digest>> = BTreeMap::new();
    for stamp in lock.iter_artifacts().flat_map(|row| &row.bundles) {
        if let Some((tag, digest)) = split_stamp(&stamp.tag) {
            digests
                .entry((stamp.repo.clone(), tag.to_string()))
                .or_default()
                .insert(digest);
        }
    }
    for stamp in lock.iter_artifacts_mut().flat_map(|row| &mut row.bundles) {
        let Some((tag, digest)) = split_stamp(&stamp.tag) else {
            continue;
        };
        let pair = (stamp.repo.clone(), tag.to_string());
        let contested = digests.get(&pair).is_some_and(|seen| seen.len() > 1);
        let recorded = matches!(pins.get(&pair), Some(Some(run)) if *run == digest);
        if contested && !recorded {
            stamp.tag = pair.1;
        }
    }
}

/// The stamps of the retained bundles no row carries, by the rows below each.
fn stamps_below_unstamped_bundles(
    lock: &GrimoireLock,
    retained: &[&LockedBundle],
) -> BTreeMap<NodeKey, Vec<BundleProvenance>> {
    let stamped: BTreeSet<(&str, &str)> = lock
        .iter_artifacts()
        .flat_map(|row| &row.bundles)
        .map(|stamp| (stamp.repo.as_str(), stamp.tag.as_str()))
        .collect();
    let by_name: BTreeMap<&str, &LockedBundle> = retained.iter().map(|b| (b.name.as_str(), *b)).collect();
    let mut below: BTreeMap<NodeKey, Vec<BundleProvenance>> = BTreeMap::new();
    for bundle in retained.iter().copied().filter(|bundle| records_a_digest(bundle)) {
        let (repo, tag) = bundle.provenance_pair();
        if stamped.contains(&(repo.as_str(), tag.as_str())) {
            continue;
        }
        let mut seen = BTreeSet::from([bundle.name.as_str()]);
        let mut queue = vec![bundle];
        while let Some(parent) = queue.pop() {
            for member in &parent.members {
                if member.kind != ArtifactKind::Bundle {
                    let stamp = BundleProvenance::new(repo.clone(), tag.clone());
                    below.entry((member.kind, member.name.clone())).or_default().push(stamp);
                } else if let Some(child) = by_name
                    .get(member.name.as_str())
                    .filter(|_| seen.insert(member.name.as_str()))
                {
                    queue.push(child);
                }
            }
        }
    }
    below
}
