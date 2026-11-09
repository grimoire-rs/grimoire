// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The bundle digests a marketplace part records in its stamps.
//!
//! A part holds no `[[bundle]]` row: a bundle's pin lives only in the stamp of
//! each member row it provides, as `bundle_tag = "<tag>@<digest>"` or the same
//! string in a `bundles` entry. [`read_stamps`] takes those digests off, so every
//! comparison of a stamp against a declared tag reads the plain tag a project lock
//! holds, and keeps what it took as a [`PartStamps`].

use std::collections::{BTreeMap, BTreeSet};

use crate::lock::effective_set::NodeKey;
use crate::lock::grimoire_lock::GrimoireLock;
use crate::oci::Digest;
use crate::oci::identifier::is_valid_oci_tag;

/// A bundle's `(repository, tag)`: the pair a stamp names it by.
type BundlePair = (String, String);

/// The digests a part's stamps record, read once from the part as saved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartStamps {
    /// The digest recorded for a bundle's `(repo, tag)`. A pair whose stamps disagree
    /// on the digest records none.
    pub pins: BTreeMap<BundlePair, Digest>,
    /// The pairs of `pins` each row's own stamp carries with that digest: the row came
    /// through that bundle at that digest.
    pub rows: BTreeMap<NodeKey, BTreeSet<BundlePair>>,
}

impl PartStamps {
    /// Whether no stamp of the part records a digest.
    pub fn is_empty(&self) -> bool {
        self.pins.is_empty()
    }

    /// The digest the part records for the bundle `(repo, tag)`.
    pub fn recorded(&self, repo: &str, tag: &str) -> Option<&Digest> {
        self.pins.get(&(repo.to_string(), tag.to_string()))
    }
}

/// A stamp's tag and the bundle digest it records, `<tag>@<digest>`. An OCI tag
/// holds no `@`, so the split is unambiguous.
pub fn split_stamp(tag: &str) -> Option<(&str, Digest)> {
    let (tag, digest) = tag.split_once('@')?;
    let digest = Digest::try_from(digest).ok()?;
    is_valid_oci_tag(tag).then_some((tag, digest))
}

/// `part` with every stamp's digest taken off, as a project lock stamps its rows,
/// and the digests by stamp. A stamp that records two digests records none.
pub fn read_stamps(part: &GrimoireLock) -> (GrimoireLock, PartStamps) {
    let mut part = part.clone();
    let mut seen: BTreeMap<BundlePair, Option<Digest>> = BTreeMap::new();
    let mut carried: BTreeMap<NodeKey, Vec<(BundlePair, Digest)>> = BTreeMap::new();
    for row in part.iter_artifacts_mut() {
        for stamp in &mut row.bundles {
            let Some((tag, digest)) = split_stamp(&stamp.tag) else {
                continue;
            };
            stamp.tag = tag.to_string();
            let pair = (stamp.repo.clone(), stamp.tag.clone());
            carried
                .entry((row.kind, row.name.clone()))
                .or_default()
                .push((pair.clone(), digest.clone()));
            seen.entry(pair)
                .and_modify(|at| {
                    if at.as_ref() != Some(&digest) {
                        *at = None;
                    }
                })
                .or_insert(Some(digest));
        }
        row.bundles.dedup();
    }
    let pins: BTreeMap<BundlePair, Digest> = seen
        .into_iter()
        .filter_map(|(pair, digest)| Some((pair, digest?)))
        .collect();
    let mut rows: BTreeMap<NodeKey, BTreeSet<BundlePair>> = BTreeMap::new();
    for (key, stamps) in carried {
        for (pair, digest) in stamps {
            if pins.get(&pair) == Some(&digest) {
                rows.entry(key.clone()).or_default().insert(pair);
            }
        }
    }
    (part, PartStamps { pins, rows })
}

/// Fail unless every bundle pair a stamp of `part` records a digest on reads back from it:
/// [`read_stamps`] drops a pair whose stamps disagree, which loses the bundle's pin.
#[cfg(test)]
pub(crate) fn assert_every_recorded_pair_reads_back(part: &GrimoireLock) {
    let (_, read) = read_stamps(part);
    for row in part.iter_artifacts() {
        for stamp in &row.bundles {
            let Some((tag, _)) = split_stamp(&stamp.tag) else {
                continue;
            };
            let pair = (stamp.repo.clone(), tag.to_string());
            assert!(
                read.pins.contains_key(&pair),
                "row '{}' records {pair:?} at a digest another row of the part contradicts: {:?}",
                row.name,
                part.iter_artifacts().map(|r| (&r.name, &r.bundles)).collect::<Vec<_>>()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::grimoire_lock::LockMetadata;
    use crate::lock::lock_version::LockVersion;
    use crate::lock::locked_artifact::{BundleProvenance, LockedArtifact};
    use crate::lock::locked_source::LockedSource;
    use crate::oci::{ArtifactKind, Identifier, PinnedIdentifier};

    const PACK: &str = "localhost:5000/acme/pack";

    fn row(name: &str, digest: &Digest) -> LockedArtifact {
        let id = Identifier::parse(&format!("localhost:5000/acme/{name}:1@{digest}")).unwrap();
        LockedArtifact {
            name: name.to_string(),
            kind: ArtifactKind::Skill,
            source: LockedSource::Registry(PinnedIdentifier::try_from(id).unwrap()),
            bundles: vec![BundleProvenance::new(PACK, format!("1@{digest}"))],
            requires: Vec::new(),
        }
    }

    fn blank() -> GrimoireLock {
        GrimoireLock {
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: crate::config::hash::MARKETPLACE_HASH_VERSION,
                declaration_hash: String::new(),
                generated_by: String::new(),
                generated_at: String::new(),
            },
            skills: Vec::new(),
            rules: Vec::new(),
            agents: Vec::new(),
            mcp: Vec::new(),
            hooks: Vec::new(),
            settings: Vec::new(),
            bundles: Vec::new(),
        }
    }

    #[test]
    fn a_stamp_splits_only_into_a_tag_and_a_digest() {
        let digest = Digest::Sha256("a".repeat(64));
        assert_eq!(split_stamp(&format!("1.2@{digest}")), Some(("1.2", digest.clone())));
        assert_eq!(split_stamp("1.2"), None, "a plain tag records nothing");
        assert_eq!(split_stamp(&digest.to_short_string()), None, "a digest request");
        assert_eq!(split_stamp("1.2@sha256:short"), None);
        assert_eq!(split_stamp(&format!("bad tag@{digest}")), None);
    }

    /// Two rows that disagree on a bundle's digest record neither, and every stamp
    /// still reads as the plain tag a provenance match compares.
    #[test]
    fn a_stamp_two_rows_record_at_two_digests_records_neither() {
        let first = Digest::Sha256("a".repeat(64));
        let second = Digest::Sha256("b".repeat(64));
        let mut part = blank();
        part.skills = vec![row("a", &first), row("b", &second)];

        let (read, stamps) = read_stamps(&part);

        assert!(stamps.is_empty(), "{stamps:?}");
        assert!(stamps.rows.is_empty(), "{stamps:?}");
        for row in &read.skills {
            assert_eq!(row.bundles, [BundleProvenance::new(PACK, "1")]);
        }
    }

    /// A row carries a pair only where its own stamp records the digest the pair
    /// agrees on; a plain stamp beside it carries nothing.
    #[test]
    fn a_row_carries_the_pairs_its_own_stamp_records() {
        let digest = Digest::Sha256("a".repeat(64));
        let mut plain = row("b", &digest);
        plain.bundles = vec![BundleProvenance::new(PACK, "1")];
        let mut part = blank();
        part.skills = vec![row("a", &digest), plain];

        let (read, stamps) = read_stamps(&part);

        let pair = (PACK.to_string(), "1".to_string());
        assert_eq!(stamps.pins, BTreeMap::from([(pair.clone(), digest)]));
        assert_eq!(
            stamps.rows,
            BTreeMap::from([((ArtifactKind::Skill, "a".to_string()), BTreeSet::from([pair]))])
        );
        assert!(
            read.skills
                .iter()
                .all(|r| r.bundles == [BundleProvenance::new(PACK, "1")])
        );
    }
}
