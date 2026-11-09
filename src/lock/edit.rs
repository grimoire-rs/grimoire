// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The lock edit a declaration edit brings along, with no resolve: `grim remove`,
//! `grim uninstall` and the TUI delete action keep the lock in line with the
//! edited declaration offline (`adr_effective_set_mutations.md`).

use std::collections::{BTreeMap, BTreeSet};

use crate::config::declaration::{DeclaredSource, DesiredSet};
use crate::config::scope::Invocation;
use crate::lock::effective_set::{NodeKey, Origin, bundles_without_members, effective_set, reached_rows};
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::locked_artifact::{BundleProvenance, LockedArtifact};
use crate::lock::pin_rule::{self, Answered, PinRecord, snapshot_matches};
use crate::lock::stored_members::{self, StoredMembers};
use crate::oci::bundle::BundleMember;
use crate::oci::text::{escape_controls, registry_text};
use crate::oci::{ArtifactKind, Identifier};

/// Result of bringing the lock to the post-edit effective state.
pub(crate) struct UndeclareOutcome {
    /// The lock to save.
    pub lock: GrimoireLock,
    /// What the edit has to tell the user, for the caller to print.
    pub notes: Vec<EditNote>,
}

/// One thing a lock edit has to tell the user. Its [`Display`](std::fmt::Display)
/// form is the line a command logs and the TUI shows.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct EditNote {
    /// The `grim` every command in the note starts with: a plain one in a global
    /// or `--config` edit would act on the project in the working directory.
    grim: Invocation,
    what: NoteKind,
}

#[derive(Debug, PartialEq, Eq)]
enum NoteKind {
    /// A kept pin whose declaration hid a request this lock never resolved.
    HiddenRequest { key: NodeKey, unanswered: Identifier },
    /// A pin that left while something else still provides its key.
    Unmasked {
        key: NodeKey,
        by: Provider,
        version: VersionClaim,
    },
    /// A key bundles provide whose agreement on one reference nothing offline shows.
    Unsettled { key: NodeKey },
    /// A row kept because a declared bundle's list in the lock names it.
    ListedMember { key: NodeKey, bundle: String },
    /// A row kept because a declared bundle with no member list may provide it.
    /// `afresh`: its pin records no tag, so the relock resolves it afresh.
    MemberUnknown { key: NodeKey, bundle: String, afresh: bool },
    /// A row kept because a key the lock holds no row for may require it.
    /// `afresh` as for [`Self::MemberUnknown`].
    RequestUnknown {
        key: NodeKey,
        parent: NodeKey,
        afresh: bool,
    },
    /// Rows an edit that could not follow the dependency graph left unjudged.
    GraphUnknown { kept: Vec<NodeKey> },
    /// Rows that left beside bundles with no member list the store cannot speak
    /// for, as on 0.15.1: if one provides them, the next relock resolves them afresh.
    LeftUnread { keys: Vec<NodeKey>, bundles: Vec<String> },
    /// Rows that left beside rows that record no edge whose stored requires may
    /// reach them, as on 0.15.1: if one does, the next relock resolves them afresh.
    /// `shown`: the store shows each asker's requires reach them. `config` and `lock`:
    /// the config the recovery restores and the lock beside it, by file name or by path
    /// ([`StoredMembers::restoring`]). `removed`: the row the edit was
    /// asked to remove, when `shown` is false and rows besides it left: the recovery
    /// keeps it only if an asker requires it, so the note names it apart.
    Unwalked {
        keys: Vec<NodeKey>,
        askers: Vec<NodeKey>,
        shown: bool,
        config: String,
        lock: String,
        removed: Option<NodeKey>,
        stale: bool,
    },
}

/// What still provides a key whose pin left.
#[derive(Debug, PartialEq, Eq)]
enum Provider {
    Bundle(BundleProvenance),
    RequiredBy(Vec<NodeKey>),
}

/// How the version a dropped pin held compares with what now provides its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VersionClaim {
    Different,
    /// A tagless pin cannot show what it answered, so it may be the same version.
    Unconfirmed,
}

impl VersionClaim {
    fn text(self) -> &'static str {
        match self {
            Self::Different => "at a different version",
            Self::Unconfirmed => "at a version the lock cannot confirm",
        }
    }
}

/// The rows a note names: the first three, then how many more, so a note stays one
/// short line however many rows an edit leaves unjudged.
fn named_rows(rows: &[NodeKey]) -> String {
    named(
        rows.iter()
            .map(|(kind, name)| format!("{kind} '{}'", registry_text(name))),
    )
}

/// [`named_rows`] for any list of names.
fn named(names: impl ExactSizeIterator<Item = String>) -> String {
    const SHOWN: usize = 3;
    let more = names.len().saturating_sub(SHOWN);
    let mut names: Vec<String> = names.take(SHOWN).collect();
    if more > 0 {
        names.push(format!("{more} more"));
    }
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => names.concat(),
    }
}

impl EditNote {
    /// The `grim lock` the note asks for now; `None` where that lock is the move
    /// the note warns of. A status line leads with this rather than with a span
    /// of the note, whose quoted names could pick what the line advises.
    pub(crate) fn lock_command(&self) -> Option<String> {
        let warns = matches!(self.what, NoteKind::LeftUnread { .. } | NoteKind::Unwalked { .. });
        (!warns).then(|| format!("{} lock", self.grim))
    }

    /// Whether the note carries steps to follow before the next `grim lock`: only
    /// [`NoteKind::Unwalked`] does. The other note that warns of that lock leaves
    /// rows a relock may resolve afresh and has none.
    pub(crate) fn has_steps(&self) -> bool {
        matches!(self.what, NoteKind::Unwalked { .. })
    }

    /// Whether the note tells the user the lock is marked stale.
    fn says_stale(&self) -> bool {
        match self.what {
            NoteKind::HiddenRequest { .. }
            | NoteKind::Unmasked { .. }
            | NoteKind::ListedMember { .. }
            | NoteKind::MemberUnknown { .. }
            | NoteKind::RequestUnknown { .. } => true,
            NoteKind::Unsettled { .. }
            | NoteKind::GraphUnknown { .. }
            | NoteKind::LeftUnread { .. }
            | NoteKind::Unwalked { .. } => false,
        }
    }

    /// Mark a kept row whose pin `lock` saves with no tag: a relock of a lock this
    /// edit leaves stale resolves it afresh, so "keeps its pin" would be untrue.
    fn settle(&mut self, lock: &GrimoireLock) {
        if let NoteKind::MemberUnknown { key, afresh, .. } | NoteKind::RequestUnknown { key, afresh, .. } =
            &mut self.what
        {
            *afresh = lock
                .iter_artifacts()
                .find(|row| row.kind == key.0 && row.name == key.1)
                .and_then(|row| row.source.pinned())
                .is_some_and(|pin| pin.tag().is_none());
        }
    }
}

/// The verb and object a note uses for its rows: one row, or several.
fn leave_them(rows: &[NodeKey]) -> (&'static str, &'static str) {
    if rows.len() == 1 {
        ("leaves", "it")
    } else {
        ("leave", "them")
    }
}

/// How a kept row's note ends: the relock it names keeps the pin, unless the pin
/// records no tag.
fn kept_row(afresh: bool) -> (&'static str, &'static str) {
    if afresh {
        ("keeps its row", ", which resolves its pin afresh: it records no tag")
    } else {
        ("keeps its pin", "")
    }
}

impl std::fmt::Display for EditNote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let grim = &self.grim;
        match &self.what {
            NoteKind::HiddenRequest { key, unanswered } => write!(
                f,
                "{} '{}' keeps its pin, but its declaration hid {}, which this lock never resolved. \
                 The lock is marked stale: run `{grim} lock`, which keeps the pin, then `{}`. \
                 That update resolves every request and exits 78 if they resolve to different digests",
                key.0,
                registry_text(&key.1),
                registry_text(&unanswered.to_string()),
                grim.update(&key.1)
            ),
            NoteKind::Unmasked { key, by, version } => f.write_str(&unmasked_note(grim, key, by, version.text())),
            NoteKind::Unsettled { key } => write!(
                f,
                "{} '{}' is provided by bundles this lock cannot show agree on its reference; run `{grim} lock`",
                key.0,
                registry_text(&key.1)
            ),
            NoteKind::ListedMember { key, bundle } => write!(
                f,
                "{} '{}' keeps its pin: bundle '{}' still provides it. The lock is marked stale: run `{grim} lock`",
                key.0,
                registry_text(&key.1),
                registry_text(bundle),
            ),
            NoteKind::MemberUnknown { key, bundle, afresh } => {
                let (keeps, relock) = kept_row(*afresh);
                write!(
                    f,
                    "{} '{}' {keeps}: bundle '{}' has no member list in this lock to show it does not \
                     provide it. The lock is marked stale: run `{grim} lock`{relock}",
                    key.0,
                    registry_text(&key.1),
                    registry_text(bundle),
                )
            }
            NoteKind::RequestUnknown { key, parent, afresh } => {
                let (keeps, relock) = kept_row(*afresh);
                write!(
                    f,
                    "{} '{}' {keeps}: this lock cannot show whether {} '{}' requires it. \
                     The lock is marked stale: run `{grim} lock`{relock}",
                    key.0,
                    registry_text(&key.1),
                    parent.0,
                    registry_text(&parent.1),
                )
            }
            NoteKind::LeftUnread { keys, bundles } => {
                let (leave, them) = leave_them(keys);
                let (first, others) = bundles.split_first().map_or(("", &[][..]), |(f, o)| (f.as_str(), o));
                write!(
                    f,
                    "{} {leave} the lock: bundle '{}' has no member list in this lock, and the local \
                     store cannot show whether it provides {them}",
                    named_rows(keys),
                    registry_text(first),
                )?;
                if others.is_empty() {
                    return write!(f, ". If it does, the next `{grim} lock` resolves {them} afresh");
                }
                let others = named(others.iter().map(|b| format!("bundle '{}'", registry_text(b))));
                write!(
                    f,
                    ". The same holds for {others}. If one does, the next `{grim} lock` resolves {them} afresh"
                )
            }
            NoteKind::Unwalked {
                keys,
                askers,
                shown,
                config,
                lock,
                removed,
                stale,
            } => {
                let (leave, them) = leave_them(keys);
                let (records, require) = match (askers.len() == 1, *shown) {
                    (true, true) => ("records", "requires"),
                    (false, true) => ("record", "require"),
                    (one, false) => (if one { "records" } else { "record" }, "may require"),
                };
                let pin = if keys.len() == 1 { "its pin" } else { "their pins" };
                write!(
                    f,
                    "{} {leave} the lock, though {} {records} no dependency edges and {require} {them}. \
                     The next `{grim} lock` may move {pin}. ",
                    named_rows(keys),
                    named_rows(askers),
                )?;
                let kept: Vec<NodeKey> = keys
                    .iter()
                    .filter(|key| Some(*key) != removed.as_ref())
                    .cloned()
                    .collect();
                let to_keep = if removed.is_some() {
                    named_rows(&kept)
                } else {
                    them.to_string()
                };
                // The files the edit changed: under `--global` a project's own pair is another pair.
                // Local paths, not registry text: escaped, never cut, or a long directory names no file.
                let files = if grim.is_global() {
                    format!("$GRIM_HOME/{} and its grimoire.lock", escape_controls(config))
                } else {
                    format!("{} and {}", escape_controls(config), escape_controls(lock))
                };
                // A lock the config outran resolves every untagged pin on that relock.
                let aim = if *stale { "To try to keep" } else { "To keep" };
                write!(
                    f,
                    "{aim} {to_keep}, restore {files}, run `{grim} lock` online once \
                     before such an edit, then repeat it",
                )?;
                if let Some((kind, name)) = removed {
                    let asker = if askers.len() == 1 {
                        format!("{} requires it", named_rows(askers))
                    } else {
                        "one of them requires it".to_string()
                    };
                    write!(f, ". That keeps {kind} '{}' only if {asker}", registry_text(name))?;
                }
                if *stale {
                    write!(
                        f,
                        ". The recovery's `{grim} lock` resolves every untagged pin once, because the config \
                         changed since the last lock"
                    )?;
                }
                Ok(())
            }
            NoteKind::GraphUnknown { kept } => {
                let (keep, their) = if kept.len() == 1 {
                    ("keeps its pin", "its")
                } else {
                    ("keep their pins", "their")
                };
                write!(
                    f,
                    "{} {keep}: this lock cannot show {their} dependencies; run `{grim} lock`",
                    named_rows(kept)
                )
            }
        }
    }
}

#[cfg(test)]
impl EditNote {
    /// The note a TUI status line test shows for skill `name`.
    pub(crate) fn kept_for(grim: &Invocation, name: &str) -> Self {
        Self {
            grim: grim.clone(),
            what: NoteKind::MemberUnknown {
                key: (ArtifactKind::Skill, name.to_string()),
                bundle: "pack".to_string(),
                afresh: false,
            },
        }
    }

    /// The note a test shows when skill `name` leaves beside a bundle `pack` with no
    /// member list, a note with no steps.
    pub(crate) fn left_unread_for(grim: &Invocation, name: &str) -> Self {
        Self {
            grim: grim.clone(),
            what: NoteKind::LeftUnread {
                keys: vec![(ArtifactKind::Skill, name.to_string())],
                bundles: vec!["pack".to_string()],
            },
        }
    }

    /// The note a TUI status line test shows when skill `name` leaves beside a
    /// dependent `asker` the lock never walked.
    pub(crate) fn unwalked_for(grim: &Invocation, name: &str, asker: &str) -> Self {
        Self {
            grim: grim.clone(),
            what: NoteKind::Unwalked {
                keys: vec![(ArtifactKind::Skill, name.to_string())],
                askers: vec![(ArtifactKind::Skill, asker.to_string())],
                shown: true,
                config: "grimoire.toml".to_string(),
                lock: "grimoire.lock".to_string(),
                removed: None,
                stale: false,
            },
        }
    }
}

/// What the edited declaration needs of a key's row, the one answer every path of
/// [`drop_from_lock`] takes, so no two paths disagree on which rows survive.
enum Reach<'r> {
    /// The edited declaration's effective set holds the key.
    Reached(&'r Origin),
    /// Something the edit cannot follow may need the row: see [`Need`].
    Needed(Need<'r>),
    /// Nothing the edited declaration reaches can need the row.
    Gone,
    /// Nothing keeps the row, on a lock whose hash proves the declaration before or
    /// after the edit, beside these bundles with no list in the lock that the store
    /// cannot speak for: the row drops as on 0.15.1.
    Unknown(Vec<&'r str>),
}

/// What may still need a row the edited declaration's effective set does not hold.
enum Need<'r> {
    /// That set cannot be followed offline, and this bundle's list in the lock names
    /// the key.
    Listed(&'r str),
    /// That set cannot be followed offline, and this bundle, which the lock holds no
    /// list for, may provide the key ([`KeepRule::may_provide`]).
    Unread(&'r str),
    /// As [`Self::Unread`], on a lock with no dependency data whose hash proves the
    /// declaration before or after the edit, where only evidence keeps a row: the
    /// store or the row's stamp shows this bundle provides the key.
    Provided(&'r str),
    /// This key, which holds no row, so the lock records none of its edges, may
    /// require the key ([`KeepRule::unread_parent`]).
    Unrecorded(NodeKey),
}

/// What an edit's keep decisions read.
struct KeepRule<'e> {
    previous: &'e GrimoireLock,
    /// `previous`'s row keys, for [`Self::has_row`]: a scan of the lock per ask makes
    /// an edit of an unproven lock cubic in its rows.
    rows: BTreeSet<NodeKey>,
    set_after: &'e DesiredSet,
    /// `None` when the edited declaration's closure cannot be followed offline.
    after: Option<BTreeMap<NodeKey, Origin>>,
    stored: &'e StoredMembers,
    /// The lock's hash proves the declaration before or after the edit, so stamps
    /// answer for its bundles and a row kept on no evidence costs a `grim install`.
    proven: bool,
    dependency_data: bool,
}

impl<'e> KeepRule<'e> {
    fn new(
        previous: &'e GrimoireLock,
        set_before: &DesiredSet,
        set_after: &'e DesiredSet,
        stored: &'e StoredMembers,
    ) -> Self {
        Self {
            previous,
            rows: previous.node_keys().collect(),
            set_after,
            after: effective_set(set_after, previous),
            stored,
            proven: !stale_before_edit(previous, set_before)
                || previous.metadata.declaration_hash == set_after.declaration_hash_cached(),
            dependency_data: has_dependency_data(previous),
        }
    }

    fn reach(&self, key: &NodeKey) -> Reach<'_> {
        match &self.after {
            Some(after) => self.in_followed(after, key),
            None => self.in_unfollowed(key),
        }
    }

    /// [`Self::reach`] where the edited declaration's closure can be followed offline.
    fn in_followed<'a>(&'a self, after: &'a BTreeMap<NodeKey, Origin>, key: &NodeKey) -> Reach<'a> {
        if let Some(origin) = after.get(key) {
            return Reach::Reached(origin);
        }
        // With dependency data a followed closure names every edge a row records onto
        // `key`, and a row that records none reads as a leaf, as 0.15.1 reads it.
        if self.dependency_data {
            return Reach::Gone;
        }
        self.unread_parent(key)
            .map_or(Reach::Gone, |parent| Reach::Needed(Need::Unrecorded(parent)))
    }

    /// [`Self::reach`] where it cannot, so only the lock and the store show what may
    /// still need `key`.
    fn in_unfollowed(&self, key: &NodeKey) -> Reach<'_> {
        let lists = |row: &&crate::lock::locked_bundle::LockedBundle| {
            self.set_after.bundles.contains_key(&row.name)
                && row.members.iter().any(|m| m.kind == key.0 && m.name == key.1)
        };
        // With dependency data a list is a recorded edge, which `declaration_left`
        // judges by what the pin answers; kept here, a pin that answers nothing stays.
        if !self.dependency_data
            && let Some(row) = self.previous.bundles.iter().find(lists)
        {
            return Reach::Needed(Need::Listed(&row.name));
        }
        let unread = bundles_without_members(self.set_after, self.previous)
            .chain(self.bundles_without_rows(key))
            .find(|binding| self.may_provide(binding, key));
        let legacy_proof = self.proven && !self.dependency_data;
        match unread {
            Some(bundle) if legacy_proof && self.shows(bundle, key) => Reach::Needed(Need::Provided(bundle)),
            Some(bundle) => Reach::Needed(Need::Unread(bundle)),
            None => match self.unread_parent(key) {
                Some(parent) => Reach::Needed(Need::Unrecorded(parent)),
                None => {
                    let unknown: Vec<&str> = bundles_without_members(self.set_after, self.previous)
                        .filter(|binding| legacy_proof && !self.stored.knows(binding))
                        .collect();
                    if unknown.is_empty() {
                        Reach::Gone
                    } else {
                        Reach::Unknown(unknown)
                    }
                }
            },
        }
    }

    /// Whether `binding`, which the lock holds no list for, may provide `key`.
    fn may_provide(&self, binding: &str, key: &NodeKey) -> bool {
        if self.stored.rules_out(binding, key) {
            return false;
        }
        // A stale lock fails `grim install` whatever the edit keeps. With dependency
        // data only the edit's target asks, and its row carries no stamp a declared
        // bundle lacks, so stamps rule nothing out. Either way a key nothing rules out stays.
        if !self.proven || self.dependency_data {
            return true;
        }
        // A lock 0.15.1 reads that proved its declaration: a row kept on no evidence
        // leaves it stale, turning 0.15.1's `grim install` exit 0 into 65.
        self.shows(binding, key)
    }

    /// Whether the store or `key`'s own row shows `binding` provides it.
    fn shows(&self, binding: &str, key: &NodeKey) -> bool {
        self.stored.shows_provided(binding, key) || self.stamped_by(binding, key)
    }

    /// The edges of every row other than `key`'s.
    fn other_edges(&self, key: &NodeKey) -> impl Iterator<Item = &'e BundleMember> {
        let previous: &'e GrimoireLock = self.previous;
        let artifact_edges = previous
            .iter_artifacts()
            .filter(|row| row.kind != key.0 || row.name != key.1)
            .flat_map(|row| &row.requires);
        let bundle_edges = previous
            .bundles
            .iter()
            .filter(|row| key.0 != ArtifactKind::Bundle || row.name != key.1)
            .flat_map(|row| &row.members);
        artifact_edges.chain(bundle_edges)
    }

    /// Undeclared bundles that an edge of a row other than `key`'s names and the lock
    /// holds no row for. Only the next relock reads what they provide, so a row one of
    /// them provides, dropped, resolves afresh there and its pin can move.
    fn bundles_without_rows(&self, key: &NodeKey) -> impl Iterator<Item = &'e str> {
        let previous: &'e GrimoireLock = self.previous;
        let set_after: &'e DesiredSet = self.set_after;
        self.other_edges(key)
            .filter(|edge| edge.kind == ArtifactKind::Bundle && !set_after.bundles.contains_key(&edge.name))
            .filter(|edge| !previous.bundles.iter().any(|row| row.name == edge.name))
            .map(|edge| edge.name.as_str())
    }

    /// A key other than `key` and other than a bundle that holds no row, so the lock
    /// records none of its edges and it may require `key`: dropped, a row it requires
    /// resolves afresh at the next relock and its pin can move. With dependency data,
    /// a key declared or named by another row's edge; without it, a declared key on a
    /// stale lock. A row that records no edge reads as a leaf, as 0.15.1 reads it:
    /// kept for it, the stale lock changed exits against 0.15.1.
    fn unread_parent(&self, key: &NodeKey) -> Option<NodeKey> {
        let declared = ArtifactKind::ALL
            .into_iter()
            .filter(|kind| *kind != ArtifactKind::Bundle)
            .flat_map(|kind| self.set_after.table(kind).keys().map(move |name| (kind, name.clone())));
        if self.dependency_data {
            let named = self
                .other_edges(key)
                .filter(|edge| edge.kind != ArtifactKind::Bundle)
                .map(|edge| (edge.kind, edge.name.clone()));
            return declared
                .chain(named)
                .find(|parent| parent != key && !self.has_row(parent));
        }
        // A proven lock with a declared key and no row is one 0.15.1's remove
        // restamped: kept there, a row turns its `grim install` exit 0 into 65.
        declared
            .filter(|_| !self.proven)
            .find(|parent| parent != key && !self.has_row(parent))
    }

    /// Whether `key`'s row carries the stamp `binding`'s declaration writes, so that
    /// bundle provided it when the lock was written.
    fn stamped_by(&self, binding: &str, key: &NodeKey) -> bool {
        stored_members::stamped_by(self.set_after, self.previous, binding, key)
    }

    fn has_row(&self, key: &NodeKey) -> bool {
        self.rows.contains(key)
    }
}

/// What the lock's hash can say once the edit is done. The worst wins.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
enum HashAfter {
    /// The lock answers the edited declaration, so a stamp states a fact.
    #[default]
    Restamp,
    /// It may not. The hash stays as it was, which no longer matches, until
    /// [`drop_from_lock`] replaces it.
    Stale,
}

impl HashAfter {
    fn stamp(self, lock: &mut GrimoireLock, set_after: &DesiredSet) {
        match self {
            Self::Restamp => lock.metadata.declaration_hash = set_after.declaration_hash_cached().to_string(),
            Self::Stale => {}
        }
    }
}

/// One edit's running verdict.
struct LockEdit<'g> {
    hash: HashAfter,
    notes: Vec<EditNote>,
    /// The `grim` a note names: a plain one in a global or `--config` edit would
    /// act on the project in the working directory.
    grim: &'g Invocation,
}

impl<'g> LockEdit<'g> {
    fn new(grim: &'g Invocation) -> Self {
        Self {
            hash: HashAfter::default(),
            notes: Vec::new(),
            grim,
        }
    }

    fn note(&mut self, what: NoteKind) {
        self.notes.push(EditNote {
            grim: self.grim.clone(),
            what,
        });
    }

    /// Decide the row of a key whose direct declaration left while `requests`
    /// survive, true to keep it. Its edges were shadowed and never resolved, and
    /// after this edit nothing says so, which is why the pin answers only what
    /// its own record names (`adr_lock_pin_stability.md`, § The pin rule).
    /// `declared` is what the proof held the key at, and `gone` writes the note
    /// for a dropped row, given how it differs.
    fn declaration_left(
        &mut self,
        key: &NodeKey,
        record: Option<PinRecord<'_>>,
        declared: Option<&Identifier>,
        requests: &BTreeSet<Identifier>,
        gone: impl FnOnce(VersionClaim) -> Option<NoteKind>,
    ) -> bool {
        let answered = record.map_or(Answered::None, |record| record.classify_undeclared(declared, requests));
        match answered {
            Answered::All => true,
            Answered::Some { unanswered } => {
                self.hash = self.hash.max(HashAfter::Stale);
                self.note(NoteKind::HiddenRequest {
                    key: key.clone(),
                    unanswered: unanswered.clone(),
                });
                true
            }
            Answered::None => {
                let tagless = record.is_some_and(|record| record.pin.tag().is_none());
                let version = if tagless {
                    VersionClaim::Unconfirmed
                } else {
                    VersionClaim::Different
                };
                self.hash = self.hash.max(HashAfter::Stale);
                if let Some(note) = gone(version) {
                    self.note(note);
                }
                false
            }
        }
    }

    /// A row kept on a key a relock settles or exits 78 on: nothing shows its
    /// requests answered, so the lock stays stale. Notes it unless the verdict that
    /// kept it already did, which `noted`, the note count before it, shows.
    fn unsettled(&mut self, key: &NodeKey, noted: usize) {
        self.hash = self.hash.max(HashAfter::Stale);
        if self.notes.len() == noted {
            self.note(NoteKind::Unsettled { key: key.clone() });
        }
    }

    /// Keep the row of `key` where a bundle, or a key whose edges the lock does not
    /// record, may need it: dropped, the next `grim lock` resolves the key afresh and
    /// can move its pin. The lock stays stale, so that relock reads what needs it,
    /// unless the row is [shown](Need::Provided) to be provided. A key with no row
    /// has no pin to keep.
    fn kept_for_bundle(&mut self, rule: &KeepRule<'_>, key: &NodeKey) -> Kept {
        if !rule.has_row(key) {
            return Kept::Dropped;
        }
        self.kept_unread(rule, key, rule.reach(key))
    }

    /// [`Self::kept_for_bundle`] on a verdict already read.
    fn kept_unread(&mut self, rule: &KeepRule<'_>, key: &NodeKey, reach: Reach<'_>) -> Kept {
        match reach {
            Reach::Needed(need) => self.kept_needed(rule, key, need),
            Reach::Unknown(bundles) => {
                self.left_unread(key, &bundles);
                Kept::Dropped
            }
            Reach::Gone | Reach::Reached(_) => Kept::Dropped,
        }
    }

    /// Keep the row of `key` for what may need it, and note why.
    fn kept_needed(&mut self, rule: &KeepRule<'_>, key: &NodeKey, need: Need<'_>) -> Kept {
        let note = match need {
            Need::Listed(bundle) => NoteKind::ListedMember {
                key: key.clone(),
                bundle: bundle.to_string(),
            },
            // The stamp a relock writes keeps the lock proven, so `grim install` keeps
            // 0.15.1's exit 0 and the next `grim lock` keeps the pin.
            Need::Provided(bundle) => match stored_members::declared_stamp(rule.set_after, bundle) {
                Some(stamp) => return Kept::Stamped(stamp),
                None => NoteKind::MemberUnknown {
                    key: key.clone(),
                    bundle: bundle.to_string(),
                    afresh: false,
                },
            },
            Need::Unread(bundle) => NoteKind::MemberUnknown {
                key: key.clone(),
                bundle: bundle.to_string(),
                afresh: false,
            },
            Need::Unrecorded(parent) => NoteKind::RequestUnknown {
                key: key.clone(),
                parent,
                afresh: false,
            },
        };
        self.hash = self.hash.max(HashAfter::Stale);
        self.note(note);
        Kept::Stale
    }

    /// Name `key` among the rows that left beside `bundles`, one note per such set.
    fn left_unread(&mut self, key: &NodeKey, bundles: &[&str]) {
        for note in &mut self.notes {
            if let NoteKind::LeftUnread { keys, bundles: named } = &mut note.what
                && named.iter().map(String::as_str).eq(bundles.iter().copied())
            {
                keys.push(key.clone());
                return;
            }
        }
        self.note(NoteKind::LeftUnread {
            keys: vec![key.clone()],
            bundles: bundles.iter().map(|b| (*b).to_string()).collect(),
        });
    }
}

/// What an edit does with a row only something it cannot follow may need.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kept {
    Dropped,
    /// Kept as it stands, with the lock left stale for the relock to judge.
    Stale,
    /// Kept under the stamp of the bundle shown to provide it, the stamp a relock
    /// of the edited declaration writes, so the lock still proves it.
    Stamped(BundleProvenance),
}

impl Kept {
    fn kept(&self) -> bool {
        !matches!(self, Self::Dropped)
    }

    /// Settle `row` by the verdict, true to keep it.
    fn apply(self, row: &mut LockedArtifact) -> bool {
        match self {
            Self::Dropped => false,
            Self::Stale => true,
            Self::Stamped(stamp) => {
                row.bundles = vec![stamp];
                true
            }
        }
    }
}

/// What the proof held `(kind, name)` at before the edit, a registry declaration
/// on a lock that was fresh. A stale lock shows no declaration resolved.
fn proven_declaration<'s>(
    previous: &GrimoireLock,
    set_before: &'s DesiredSet,
    kind: ArtifactKind,
    name: &str,
) -> Option<&'s Identifier> {
    let declared = set_before.table(kind).get(name)?.identifier()?;
    (!stale_before_edit(previous, set_before)).then_some(declared)
}

/// The note for a pin that left while something else still provides its key,
/// naming `grim` as [`LockEdit::grim`] does.
fn unmasked_note(grim: &Invocation, key: &NodeKey, by: &Provider, version: &str) -> String {
    let name = registry_text(&key.1);
    match by {
        Provider::Bundle(bundle) if bundle.tag.is_empty() => format!(
            "{} '{name}' is now provided by bundle '{}' {version}. \
             The lock is marked stale: run `{grim} lock` to pin the bundle's version",
            key.0,
            registry_text(&bundle.repo)
        ),
        Provider::Bundle(bundle) => format!(
            "{} '{name}' is now provided by bundle '{}:{}' {version}. \
             The lock is marked stale: run `{grim} lock` to pin the bundle's version",
            key.0,
            registry_text(&bundle.repo),
            registry_text(&bundle.tag)
        ),
        Provider::RequiredBy(required_by) => {
            let parents: Vec<String> = required_by
                .iter()
                .map(|(kind, name)| format!("{kind}/{}", registry_text(name)))
                .collect();
            format!(
                "{} '{name}' is now required by {} {version}. The lock is marked stale: run `{grim} lock` to pin it",
                key.0,
                parents.join(", ")
            )
        }
    }
}

/// What still provides a key, as a note names it: its first bundle, else its parents.
fn provider(contributors: &[BundleProvenance], required_by: &[NodeKey]) -> Provider {
    match contributors.first() {
        Some(bundle) => Provider::Bundle(bundle.clone()),
        None => Provider::RequiredBy(required_by.to_vec()),
    }
}

/// Bring `previous` to the state the edited declaration implies, from the effective
/// sets before and after the edit (`adr_effective_set_mutations.md`): an entry
/// `E_after` lacks is dropped, one both hold keeps its pin and tag, and a key whose
/// declaration left stays while its pin answers a surviving request
/// ([`LockEdit::declaration_left`]). Writes no tag but [`pin_rule::stamp_kept_pins`]'s.
/// A stale lock stays stale unless it [answers](answers) the edited declaration, and
/// an edit that leaves a lock unproven with changed rows writes [a hash no
/// declaration produces](edited_hash). A declared bundle with no usable `[[bundle]]`
/// cache takes the legacy path, where `stored` says which keys it may provide. Every
/// command a note names starts with `grim`.
pub(crate) fn drop_from_lock(
    previous: &GrimoireLock,
    kind: ArtifactKind,
    name: &str,
    set_before: &DesiredSet,
    set_after: &DesiredSet,
    grim: &Invocation,
    stored: &StoredMembers,
) -> UndeclareOutcome {
    let rule = KeepRule::new(previous, set_before, set_after, stored);
    let key = (kind, name.to_string());
    let stale = stale_before_edit(previous, set_before);
    let lands = stale && previous.metadata.declaration_hash == set_after.declaration_hash_cached();
    if lands && !(rule.has_row(&key) && matches!(rule.reach(&key), Reach::Gone | Reach::Unknown(_))) {
        // A line typed over a settled key and taken out again. Every edit that changes
        // an unproven lock also changes its hash, so this one proves these rows. A row
        // nothing reaches stays out of that: kept, `grim install` brings it back.
        return UndeclareOutcome {
            lock: previous.clone(),
            notes: Vec::new(),
        };
    }
    let mut outcome = drop_rows(&rule, kind, name, set_before, grim);
    let lock = &mut outcome.lock;
    if lock.metadata.declaration_hash != set_after.declaration_hash_cached() {
        if !stale {
            // The proof ends here, and past it nothing shows what a tagless pin answered.
            let reached = reached_rows(set_before, previous, Some(set_before));
            pin_rule::stamp_kept_pins(lock, previous, set_before, reached.as_ref());
        }
        // Kept, the old hash proves this lock again once `grimoire.toml` is restored
        // by hand, and `grim install` passes with a row this edit took.
        if changed_rows(lock, previous) {
            lock.metadata.declaration_hash = edited_hash(&previous.metadata.declaration_hash);
        }
    }
    // The verdicts ran before the hash was settled, and a note read as stale on a
    // lock that still proves its declaration sends the user to a `grim lock` with
    // nothing to do.
    if outcome.lock.metadata.declaration_hash == set_after.declaration_hash_cached() {
        outcome.notes.retain(|note| !note.says_stale());
    }
    for note in &mut outcome.notes {
        note.settle(&outcome.lock);
    }
    name_unwalked(&mut outcome, previous, stored, grim, &key, stale);
    outcome
}

/// Name the rows the edit took that the stored requires of a surviving row may
/// reach, where that row records no edge: a dependent the lock never walked may
/// require them, and only the online `grim lock` before the edit records its edges.
fn name_unwalked(
    outcome: &mut UndeclareOutcome,
    previous: &GrimoireLock,
    stored: &StoredMembers,
    grim: &Invocation,
    removed: &NodeKey,
    stale: bool,
) {
    let left: BTreeSet<NodeKey> = outcome.lock.node_keys().collect();
    let dropped: Vec<NodeKey> = previous.node_keys().filter(|key| !left.contains(key)).collect();
    if dropped.is_empty() {
        // The first reach asked for walks every dependent's stored requires.
        return;
    }
    let (mut reached, mut askers, mut shown) = (BTreeSet::new(), Vec::new(), true);
    for key in outcome.lock.iter_artifacts().map(|row| (row.kind, row.name.clone())) {
        let Some(reach) = stored.unrecorded_reach(&key) else {
            continue;
        };
        let asked: Vec<&NodeKey> = dropped.iter().filter(|left| reach.may_reach(left)).collect();
        if asked.is_empty() {
            continue;
        }
        shown &= asked.iter().all(|left| reach.shows(left));
        reached.extend(asked.into_iter().cloned());
        askers.push(key);
    }
    if askers.is_empty() {
        return;
    }
    let keys: Vec<NodeKey> = dropped.into_iter().filter(|key| reached.contains(key)).collect();
    let removed = (!shown && keys.len() > 1 && keys.contains(removed)).then(|| removed.clone());
    let (config, lock) = stored.restore_files();
    outcome.notes.push(EditNote {
        grim: grim.clone(),
        what: NoteKind::Unwalked {
            keys,
            askers,
            shown,
            config: config.to_string(),
            lock: lock.to_string(),
            removed,
            stale,
        },
    });
}

/// Whether `lock` differs from `previous` in anything but its metadata.
fn changed_rows(lock: &GrimoireLock, previous: &GrimoireLock) -> bool {
    let mut rows = lock.clone();
    rows.metadata.clone_from(&previous.metadata);
    rows != *previous
}

/// The hash an edit that changes rows gives a lock it leaves unproven, from the hash
/// it replaces. A declaration hashes JSON text, which opens with `{`, so no declaration's hash
/// equals this one short of a SHA-256 collision, and every reader takes its
/// `sha256:<hex>` for a stale lock.
pub(crate) fn edited_hash(previous: &str) -> String {
    crate::oci::Algorithm::Sha256
        .hash(format!("edited:{previous}"))
        .to_string()
}

/// The rows and the hash of [`drop_from_lock`].
fn drop_rows(
    rule: &KeepRule<'_>,
    kind: ArtifactKind,
    name: &str,
    set_before: &DesiredSet,
    grim: &Invocation,
) -> UndeclareOutcome {
    let (previous, set_after) = (rule.previous, rule.set_after);
    let (Some(before), Some(after)) = (effective_set(set_before, previous), rule.after.as_ref()) else {
        // A dependency-bearing lock whose closure cannot be followed offline
        // must not take the legacy path: it would drop or restamp entries the
        // graph still needs. A dependency-free lock keeps it unchanged.
        return if has_dependency_data(previous) {
            target_only_drop(rule, kind, name, set_before, grim)
        } else {
            legacy_drop_from_lock(rule, kind, name, set_before, grim)
        };
    };
    let dependency_bearing = has_dependency_data(previous);
    // On a dependency-free lock only a direct row carries no stamp, so an unstamped
    // row is one whose declaration a hand edit took earlier.
    let once_direct =
        |entry: &LockedArtifact, was_direct: bool| was_direct || (entry.bundles.is_empty() && !dependency_bearing);

    let mut lock = previous.clone();
    let mut edit = LockEdit::new(grim);

    let mut process = |list: &mut Vec<LockedArtifact>| {
        list.retain_mut(|entry| {
            let key = (entry.kind, entry.name.clone());
            let was_direct = matches!(before.get(&key), Some(Origin::Direct(_)));
            // An entry in NEITHER set is a drifted ghost (a legacy lock-only
            // install) — preserved unless it is the named target, so a remove
            // never silently reaps unrelated drift.
            let ghost = !(before.contains_key(&key) || (entry.kind == kind && entry.name == name));
            match rule.reach(&key) {
                // Dropped from the effective set by this edit.
                Reach::Gone | Reach::Unknown(_) => ghost,
                Reach::Needed(_) if ghost => true,
                Reach::Needed(need) => edit.kept_needed(rule, &key, need).apply(entry),
                // A declaration typed back over a member's row: a relock writes no
                // bundle on a declared row, and a restamp must state the same row.
                Reach::Reached(Origin::Direct(_)) => {
                    entry.bundles.clear();
                    true
                }
                Reach::Reached(
                    origin @ Origin::Implied {
                        contributors,
                        required_by,
                        requests,
                        ..
                    },
                ) if once_direct(entry, was_direct) => {
                    let record = artifact_record(entry);
                    let declared = proven_declaration(previous, set_before, entry.kind, &entry.name);
                    let kept = edit.declaration_left(&key, record, declared, requests, |version| {
                        Some(NoteKind::Unmasked {
                            key: key.clone(),
                            by: provider(contributors, required_by),
                            version,
                        })
                    });
                    if kept {
                        entry.bundles = origin.contributors().to_vec();
                    }
                    kept
                }
                Reach::Reached(Origin::Conflicted { contributors, requests }) if once_direct(entry, was_direct) => {
                    let record = artifact_record(entry);
                    let declared = proven_declaration(previous, set_before, entry.kind, &entry.name);
                    let noted = edit.notes.len();
                    let kept = edit.declaration_left(&key, record, declared, requests, |_| {
                        Some(NoteKind::Unsettled { key: key.clone() })
                    });
                    if kept {
                        entry.bundles = contributors.clone();
                        edit.unsettled(&key, noted);
                    }
                    kept
                }
                // On a dependency-bearing lock an entry that was already not
                // directly declared is dependency-only: nothing was unmasked,
                // so it is kept as locked.
                Reach::Reached(Origin::Implied { contributors, .. }) => {
                    entry.bundles = contributors.clone();
                    true
                }
                // A relock settles this key or exits 78, and one that settles it writes
                // these bundles.
                Reach::Reached(Origin::Conflicted { contributors, .. }) => {
                    entry.bundles = contributors.clone();
                    true
                }
            }
        });
    };
    process(&mut lock.skills);
    process(&mut lock.rules);
    process(&mut lock.agents);
    // `mcp` is a first-class lock list (`iter_artifacts` chains it, so the
    // installer materializes it): omitting it here left an undeclared server
    // in a lock the restamp below marks FRESH, and the next `grim install`
    // re-spliced it into every client config.
    process(&mut lock.mcp);
    // `hooks` is the same shape, and its blast radius is the largest of the
    // five: an undeclared `[[hook]]` surviving in a lock the restamp marks
    // FRESH is a hook the next `grim install` re-materializes AND re-arms —
    // code a client then runs automatically, after the user asked for it to be
    // gone. Reachable two ways: `grim remove --kind hook <name>` (the direct
    // declaration is dropped from `set.hooks` above, so `after` no longer holds
    // the key) and removing a bundle that provided a hook member.
    process(&mut lock.hooks);
    // Same shape: a surviving `[[settings]]` entry under a FRESH restamp is a
    // package the next `grim install` applies after the user removed it.
    process(&mut lock.settings);

    // Prune cached snapshots for bundles nothing reaches any more (or retagged).
    lock.bundles.retain_mut(|b| match set_after.bundles.get(&b.name) {
        Some(declared) => match declared.identifier() {
            Some(id) => snapshot_matches(&b.name, id, b),
            // A path binding keeps its path snapshot.
            None => b.path().is_some(),
        },
        None => {
            let key = (ArtifactKind::Bundle, b.name.clone());
            let origin = after.get(&key);
            let Some(requests) = origin.and_then(Origin::requests) else {
                // A key whose edges the lock does not record may ask for it. Kept, the
                // row keeps its `transitive`: written on a lock 0.15.1 wrote, it makes
                // 0.15.1 refuse that lock.
                return edit.kept_unread(rule, &key, rule.reach(&key)).kept();
            };
            // Only an edge can hold it now, which a relock records as `transitive`.
            b.transitive = true;
            if !set_before.bundles.contains_key(&b.name) {
                return true;
            }
            let record = b.pinned().map(|pin| PinRecord {
                pin,
                row: Some(&*b),
                binding: &key.1,
            });
            let declared = proven_declaration(previous, set_before, ArtifactKind::Bundle, &b.name);
            let noted = edit.notes.len();
            let kept = edit.declaration_left(&key, record, declared, requests, |version| {
                Some(NoteKind::Unmasked {
                    key: key.clone(),
                    by: Provider::RequiredBy(Vec::new()),
                    version,
                })
            });
            if kept && matches!(origin, Some(Origin::Conflicted { .. })) {
                edit.unsettled(&key, noted);
            }
            kept
        }
    });

    let hash = edit.hash.max(hash_before_edit(previous, set_before, &lock, set_after));
    hash.stamp(&mut lock, set_after);
    UndeclareOutcome {
        lock,
        notes: edit.notes,
    }
}

/// The row of a registry artifact, as [`pin_rule`] reads it.
fn artifact_record(entry: &LockedArtifact) -> Option<PinRecord<'_>> {
    entry.source.pinned().map(|pin| PinRecord {
        pin,
        row: None,
        binding: &entry.name,
    })
}

/// What the lock brings into an edit: a stale lock restamps only where the stamp
/// states a fact.
fn hash_before_edit(
    previous: &GrimoireLock,
    set_before: &DesiredSet,
    lock: &GrimoireLock,
    set_after: &DesiredSet,
) -> HashAfter {
    if stale_before_edit(previous, set_before) && !answers(lock, set_after) {
        HashAfter::Stale
    } else {
        HashAfter::Restamp
    }
}

/// Whether `previous` already disagreed with the declaration before this edit.
/// Such a lock must not be restamped unless it [`answers`] the edited one: a
/// restamp would mark the old pin of a hand-edited entry as locked, and the next
/// `grim install` would install it.
fn stale_before_edit(previous: &GrimoireLock, set_before: &DesiredSet) -> bool {
    previous.metadata.declaration_hash != set_before.declaration_hash_cached()
}

/// Whether `lock`'s rows are the ones a relock of `set` writes, pins aside: no key
/// twice, every key the declaration reaches has a row, carrying the bundles a relock
/// writes, and no row is left that nothing reaches. `effective` is `set`'s effective
/// set over `lock`: a closure that cannot be read has none and shows none of that,
/// and a lost row is one way to lose it.
fn rows_stand(lock: &GrimoireLock, set: &DesiredSet, effective: &BTreeMap<NodeKey, Origin>) -> bool {
    if !pin_rule::listed_twice(lock).is_empty() {
        return false;
    }
    let rows: BTreeSet<NodeKey> = lock.node_keys().collect();
    let bundled = |entry: &LockedArtifact| {
        let origin = effective.get(&(entry.kind, entry.name.clone()));
        origin.is_some_and(|origin| entry.bundles == origin.contributors())
    };
    let bundle_reached = |row: &crate::lock::locked_bundle::LockedBundle| {
        set.bundles.contains_key(&row.name) || effective.contains_key(&(ArtifactKind::Bundle, row.name.clone()))
    };
    effective.keys().all(|key| rows.contains(key))
        && lock.iter_artifacts().all(bundled)
        && lock.bundles.iter().all(bundle_reached)
}

/// Whether a stale `lock` answers `set` in full, so that a stamp changes nothing
/// a relock or an install would do: its [rows stand](rows_stand), and every row is
/// the one a relock writes ([`pin_rule::restamp_answers_declared`],
/// [`pin_rule::restamp_answers_edges`]). A row nothing reaches would install, and
/// for a hook arm, on the stamp's word.
fn answers(lock: &GrimoireLock, set: &DesiredSet) -> bool {
    let Some(effective) = effective_set(set, lock).filter(|effective| rows_stand(lock, set, effective)) else {
        return false;
    };
    let row_answers = |entry: &LockedArtifact| match effective.get(&(entry.kind, entry.name.clone())) {
        Some(Origin::Direct(DeclaredSource::Path(path))) => entry.source.path() == Some(path),
        Some(Origin::Direct(DeclaredSource::Registry(id))) => {
            artifact_record(entry).is_some_and(|record| pin_rule::restamp_answers_declared(record, id))
        }
        Some(Origin::Implied { requests, .. }) => {
            artifact_record(entry).is_some_and(|record| pin_rule::restamp_answers_edges(record, requests))
        }
        Some(Origin::Conflicted { .. }) | None => false,
    };
    let bundle_answers = |row: &crate::lock::locked_bundle::LockedBundle| {
        let record = row.pinned().map(|pin| PinRecord {
            pin,
            row: Some(row),
            binding: &row.name,
        });
        match set.bundles.get(&row.name) {
            Some(DeclaredSource::Path(path)) => row.path() == Some(path),
            Some(DeclaredSource::Registry(id)) => {
                record.is_some_and(|record| pin_rule::restamp_answers_declared(record, id))
            }
            None => match effective.get(&(ArtifactKind::Bundle, row.name.clone())) {
                Some(Origin::Implied { requests, .. }) => {
                    record.is_some_and(|record| pin_rule::restamp_answers_edges(record, requests))
                }
                _ => false,
            },
        }
    };
    lock.iter_artifacts().all(row_answers) && lock.bundles.iter().all(bundle_answers)
}

/// Whether the lock carries any dependency data: a `requires` edge, a nested
/// bundle, or a transitively pulled-in bundle. Such a lock is never handled by
/// the pre-dependency fallbacks. A nested bundle counts even with every row
/// declared: a direct declaration can hide the version the include asks for,
/// and the legacy path would restamp that lock fresh.
pub(crate) fn has_dependency_data(lock: &GrimoireLock) -> bool {
    let nests = |b: &crate::lock::locked_bundle::LockedBundle| b.members.iter().any(|m| m.kind == ArtifactKind::Bundle);
    lock.iter_artifacts().any(|a| !a.requires.is_empty()) || lock.bundles.iter().any(|b| b.transitive || nests(b))
}

/// The list of the lock that holds a non-bundle `kind`.
fn rows_mut(lock: &mut GrimoireLock, kind: ArtifactKind) -> Option<&mut Vec<LockedArtifact>> {
    match kind {
        ArtifactKind::Skill => Some(&mut lock.skills),
        ArtifactKind::Rule => Some(&mut lock.rules),
        ArtifactKind::Agent => Some(&mut lock.agents),
        ArtifactKind::Mcp => Some(&mut lock.mcp),
        ArtifactKind::Hook => Some(&mut lock.hooks),
        ArtifactKind::Settings => Some(&mut lock.settings),
        ArtifactKind::Bundle => None,
    }
}

fn remove_row(lock: &mut GrimoireLock, kind: ArtifactKind, name: &str) {
    match rows_mut(lock, kind) {
        Some(rows) => rows.retain(|a| a.name != name),
        None => lock.bundles.retain(|b| b.name != name),
    }
}

/// The closure of a dependency-bearing lock cannot be computed offline, so
/// only the target can leave the lock: its own entry, or a bundle's snapshot,
/// and that by what its pin answers of the rows that stay. Every other entry is
/// written untouched and the declaration hash is not restamped, so the lock
/// stays honestly stale until `grim lock`.
fn target_only_drop(
    rule: &KeepRule<'_>,
    kind: ArtifactKind,
    name: &str,
    set_before: &DesiredSet,
    grim: &Invocation,
) -> UndeclareOutcome {
    let (previous, set_after) = (rule.previous, rule.set_after);
    let mut lock = previous.clone();
    remove_row(&mut lock, kind, name);
    let key = (kind, name.to_string());
    let requests = pin_rule::recorded_requests(&lock).remove(&key).unwrap_or_default();

    let mut edit = LockEdit::new(grim);
    let record = match kind {
        ArtifactKind::Bundle => previous.bundles.iter().find(|b| b.name == name).and_then(|row| {
            let pin = row.pinned()?;
            Some(PinRecord {
                pin,
                row: Some(row),
                binding: name,
            })
        }),
        _ => previous
            .iter_artifacts()
            .find(|a| a.kind == kind && a.name == name)
            .and_then(artifact_record),
    };
    let declared = proven_declaration(previous, set_before, kind, name);
    let kept = edit.kept_for_bundle(rule, &key).kept()
        || (!requests.is_empty() && edit.declaration_left(&key, record, declared, &requests, |_| None));
    if kept {
        lock = previous.clone();
        // Only an edge holds it now.
        if let Some(row) = lock
            .bundles
            .iter_mut()
            .find(|b| kind == ArtifactKind::Bundle && b.name == name)
        {
            row.transitive = true;
        }
    }

    let direct = |a: &LockedArtifact| set_after.table(a.kind).contains_key(&a.name);
    let kept: Vec<NodeKey> = lock
        .iter_artifacts()
        .filter(|a| !direct(a))
        .map(|a| (a.kind, a.name.clone()))
        .collect();
    if !kept.is_empty() {
        edit.note(NoteKind::GraphUnknown { kept });
    }
    edit.hash.max(HashAfter::Stale).stamp(&mut lock, set_after);
    UndeclareOutcome {
        lock,
        notes: edit.notes,
    }
}

/// Pre-cache fallback, taken only when a declared bundle has no member list in
/// the lock: a row stays while [`KeepRule::reach`] says that bundle may provide it,
/// and a bundle's other members leave by provenance. The hash restamps unless the
/// lock is or goes stale.
fn legacy_drop_from_lock(
    rule: &KeepRule<'_>,
    kind: ArtifactKind,
    name: &str,
    set_before: &DesiredSet,
    grim: &Invocation,
) -> UndeclareOutcome {
    let (previous, set_after) = (rule.previous, rule.set_after);
    let mut lock = previous.clone();
    let mut edit = LockEdit::new(grim);
    match kind {
        ArtifactKind::Bundle => {
            // The `(repo, tag)` member-provenance pair the removed bundle
            // stamped onto its members. A registry bundle derives it from its
            // declared identifier; a **path** bundle has no identifier, so
            // read it from the bundle's cached `[[bundle]]` snapshot — whose
            // `provenance_pair()` is the same `(path, hash-short)` pair the
            // resolver stamped onto every member. Without the snapshot
            // fallback a path bundle's members are never evicted and survive
            // as orphans under a freshly restamped hash (the B1 bug: the lock
            // reads fresh while listing a member no declaration provides, so
            // the next `grim install` re-materializes the removed bundle).
            let provenance = set_before
                .bundles
                .get(name)
                .and_then(|source| source.identifier())
                .map(|id| (id.registry_repository(), id.tag_or_latest().to_string()))
                .or_else(|| {
                    previous
                        .bundles
                        .iter()
                        .find(|b| b.name == name)
                        .map(crate::lock::locked_bundle::LockedBundle::provenance_pair)
                });
            match provenance {
                Some((repo, tag)) => evict_bundle_members(&mut lock, &repo, &tag, set_after, |member| {
                    edit.kept_for_bundle(rule, &(member.kind, member.name.clone()))
                        .apply(member)
                }),
                // No identifier and no cached snapshot (a hand-edited lock):
                // membership is unknowable offline, so the removed bundle's
                // members may still be listed. Skip the hash restamp so the
                // lock reads honestly stale instead of fresh-with-orphans —
                // the next `grim lock` reconciles it.
                None => edit.hash = HashAfter::Stale,
            }
            // A key whose edges the lock does not record may ask for the bundle itself.
            let key = (kind, name.to_string());
            let unread = rule.unread_parent(&key).filter(|_| rule.has_row(&key));
            if !unread.is_some_and(|parent| edit.kept_needed(rule, &key, Need::Unrecorded(parent)).kept()) {
                lock.bundles.retain(|b| b.name != name);
            }
        }
        // A dependency-free lock comes here only when a declared bundle has no member
        // list. A relock stamps no bundle on a declared row, so dropping the row here
        // lets the next `grim lock` resolve a key that bundle provides and move its pin.
        _ => {
            let kept = edit.kept_for_bundle(rule, &(kind, name.to_string()));
            let row = rows_mut(&mut lock, kind).and_then(|rows| rows.iter_mut().find(|a| a.name == name));
            if !row.is_some_and(|row| kept.apply(row)) {
                remove_row(&mut lock, kind, name);
            }
        }
    }
    let hash = edit.hash.max(hash_before_edit(previous, set_before, &lock, set_after));
    hash.stamp(&mut lock, set_after);
    UndeclareOutcome {
        lock,
        notes: edit.notes,
    }
}

/// Evict the lock members the bundle `(repo, tag)` contributed.
///
/// Each member loses the matching provenance entry; the member itself is
/// dropped only when no other bundle's provenance remains (a member two
/// bundles share survives the removal of one), unless `keep` holds it for a
/// bundle that may still provide it. When another still-declared binding in `set`
/// resolves to the same `(repo, tag)` — the same bundle declared under two names —
/// nothing is evicted at all.
fn evict_bundle_members(
    lock: &mut GrimoireLock,
    repo: &str,
    tag: &str,
    set: &DesiredSet,
    mut keep: impl FnMut(&mut LockedArtifact) -> bool,
) {
    // A surviving binding still declares `(repo, tag)` when its registry
    // identifier matches — or, for a **path** binding (whose `identifier()`
    // is always `None`), when its cached `[[bundle]]` snapshot resolves to the
    // same `provenance_pair()`. Without the snapshot check the same local
    // bundle declared under two names would wrongly evict a member the
    // surviving binding still provides.
    let still_declared = set.bundles.iter().any(|(binding, source)| {
        source
            .identifier()
            .is_some_and(|id| id.registry_repository() == repo && id.tag_or_latest() == tag)
            || lock.bundles.iter().any(|b| {
                b.name == *binding && {
                    let (b_repo, b_tag) = b.provenance_pair();
                    b_repo == repo && b_tag == tag
                }
            })
    });
    if still_declared {
        return;
    }
    let of_bundle = |b: &BundleProvenance| b.repo == repo && b.tag == tag;
    let mut evict = |a: &mut LockedArtifact| {
        if !a.bundles.iter().any(of_bundle) {
            return true; // direct entry or another bundle's member: keep as-is
        }
        if a.bundles.iter().all(of_bundle) && keep(a) {
            return true;
        }
        a.bundles.retain(|b| !of_bundle(b));
        !a.bundles.is_empty()
    };
    lock.skills.retain_mut(&mut evict);
    lock.rules.retain_mut(&mut evict);
    lock.agents.retain_mut(&mut evict);
    lock.mcp.retain_mut(&mut evict);
    // The legacy path is not a rare corner: `effective_set` degrades for the
    // WHOLE call, so a single path-declared bundle anywhere in the project (or
    // a pre-cache lock) routes every mutation through here. Omitting `hooks`
    // left the dropped bundle's hook member in the lock with `stale` still
    // false, so the hash was restamped fresh and the next `grim install`
    // re-armed it — the same re-arm hazard as `drop_from_lock` above.
    lock.hooks.retain_mut(&mut evict);
    lock.settings.retain_mut(&mut evict);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::declaration::DesiredSet;
    use crate::lock::grimoire_lock::LockMetadata;
    use crate::lock::lock_version::LockVersion;
    use crate::lock::locked_artifact::LockedArtifact;
    use crate::lock::locked_bundle::{LockedBundle, LockedBundleSource};
    use crate::lock::locked_source::LockedSource;
    use crate::oci::pinned_identifier::PinnedIdentifier;
    use crate::oci::{Digest, Identifier};
    use std::collections::BTreeMap;

    /// An edit's outcome, its notes as a command prints them.
    struct Edited {
        lock: GrimoireLock,
        notes: Vec<String>,
    }

    fn rendered(notes: &[EditNote]) -> Vec<String> {
        notes.iter().map(ToString::to_string).collect()
    }

    /// The edit in a walked-up project with a cold store, whose notes name plain `grim`.
    fn drop_from_lock(
        previous: &GrimoireLock,
        kind: ArtifactKind,
        name: &str,
        before: &DesiredSet,
        after: &DesiredSet,
    ) -> Edited {
        drop_beside(previous, kind, name, before, after, &StoredMembers::default())
    }

    /// The edit with a store that holds `stored`.
    fn drop_beside(
        previous: &GrimoireLock,
        kind: ArtifactKind,
        name: &str,
        before: &DesiredSet,
        after: &DesiredSet,
        stored: &StoredMembers,
    ) -> Edited {
        let out = super::drop_from_lock(previous, kind, name, before, after, &Invocation::WALKED, stored);
        Edited {
            notes: rendered(&out.notes),
            lock: out.lock,
        }
    }

    fn locked(name: &str) -> LockedArtifact {
        let id = Identifier::new_registry(name, "localhost:5000").clone_with_digest(Digest::Sha256("a".repeat(64)));
        LockedArtifact::direct(
            name.to_string(),
            ArtifactKind::Skill,
            PinnedIdentifier::try_from(id).unwrap(),
        )
    }

    fn lock_of(skills: Vec<LockedArtifact>) -> GrimoireLock {
        GrimoireLock {
            hooks: vec![],
            settings: vec![],
            metadata: LockMetadata {
                lock_version: LockVersion::V1,
                declaration_hash_version: 1,
                declaration_hash: format!("sha256:{}", "d".repeat(64)),
                generated_by: "grim 0.1.0".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            skills,
            rules: vec![],
            agents: vec![],
            mcp: vec![],
            bundles: vec![],
        }
    }

    /// Stamp `lock` the way `grim lock` does for `set`, so it reads fresh.
    fn stamp(lock: &mut GrimoireLock, set: &DesiredSet) {
        lock.metadata.declaration_hash = set.declaration_hash_cached().to_string();
    }

    fn member_of(name: &str, bundles: &[(&str, &str)]) -> LockedArtifact {
        let mut a = locked(name);
        a.bundles = bundles
            .iter()
            .map(|(repo, tag)| crate::lock::locked_artifact::BundleProvenance::new(*repo, *tag))
            .collect();
        a
    }

    /// A locked `[[mcp]]` entry, optionally carrying bundle provenance.
    fn locked_mcp(name: &str, bundles: &[(&str, &str)]) -> LockedArtifact {
        let id = Identifier::new_registry(name, "localhost:5000").clone_with_digest(Digest::Sha256("a".repeat(64)));
        let mut a = LockedArtifact::direct(
            name.to_string(),
            ArtifactKind::Mcp,
            PinnedIdentifier::try_from(id).unwrap(),
        );
        a.bundles = bundles
            .iter()
            .map(|(repo, tag)| crate::lock::locked_artifact::BundleProvenance::new(*repo, *tag))
            .collect();
        a
    }

    /// A locked `[[hook]]` entry, optionally carrying bundle provenance.
    fn locked_hook(name: &str, bundles: &[(&str, &str)]) -> LockedArtifact {
        let id = Identifier::new_registry(name, "localhost:5000").clone_with_digest(Digest::Sha256("a".repeat(64)));
        let mut a = LockedArtifact::direct(
            name.to_string(),
            ArtifactKind::Hook,
            PinnedIdentifier::try_from(id).unwrap(),
        );
        a.bundles = bundles
            .iter()
            .map(|(repo, tag)| crate::lock::locked_artifact::BundleProvenance::new(*repo, *tag))
            .collect();
        a
    }

    /// A `DesiredSet` declaring `hooks` bindings plus `bundles`.
    fn hook_set_of(hooks: &[(&str, &str)], bundles: &[(&str, &str)]) -> DesiredSet {
        let mut set = set_of(&[], bundles);
        for (n, i) in hooks {
            set.hooks.insert(
                (*n).to_string(),
                crate::config::declaration::DeclaredSource::Registry(Identifier::parse(i).unwrap()),
            );
        }
        set.invalidate_declaration_hash_cache();
        set
    }

    fn locked_settings(name: &str, bundles: &[(&str, &str)]) -> LockedArtifact {
        let id = Identifier::new_registry(name, "localhost:5000").clone_with_digest(Digest::Sha256("a".repeat(64)));
        let mut a = LockedArtifact::direct(
            name.to_string(),
            ArtifactKind::Settings,
            PinnedIdentifier::try_from(id).unwrap(),
        );
        a.bundles = bundles
            .iter()
            .map(|(repo, tag)| crate::lock::locked_artifact::BundleProvenance::new(*repo, *tag))
            .collect();
        a
    }

    /// A `DesiredSet` declaring `settings` bindings plus `bundles`.
    fn settings_set_of(settings: &[(&str, &str)], bundles: &[(&str, &str)]) -> DesiredSet {
        let mut set = set_of(&[], bundles);
        for (n, i) in settings {
            set.settings.insert(
                (*n).to_string(),
                crate::config::declaration::DeclaredSource::Registry(Identifier::parse(i).unwrap()),
            );
        }
        set.invalidate_declaration_hash_cache();
        set
    }

    /// A one-member `[[bundle]]` snapshot for `kind`/`member`.
    fn snapshot_of_kind(
        binding: &str,
        repo: &str,
        tag: &str,
        kind: ArtifactKind,
        member: (&str, &str),
    ) -> crate::lock::locked_bundle::LockedBundle {
        let pinned_id = Identifier::parse(&format!("{repo}:{tag}"))
            .unwrap()
            .clone_with_digest(Digest::Sha256("b".repeat(64)));
        crate::lock::locked_bundle::LockedBundle {
            name: binding.to_string(),
            source: crate::lock::locked_bundle::LockedBundleSource::Registry {
                repo: repo.to_string(),
                tag: tag.to_string(),
                pinned: PinnedIdentifier::try_from(pinned_id).unwrap(),
            },
            members: vec![crate::oci::bundle::BundleMember {
                kind,
                name: member.0.to_string(),
                id: member.1.to_string(),
            }],
            transitive: false,
        }
    }

    /// A `DesiredSet` declaring `mcp` bindings plus `bundles`.
    fn mcp_set_of(mcp: &[(&str, &str)], bundles: &[(&str, &str)]) -> DesiredSet {
        let mut set = set_of(&[], bundles);
        for (n, i) in mcp {
            set.mcp.insert(
                (*n).to_string(),
                crate::config::declaration::DeclaredSource::Registry(Identifier::parse(i).unwrap()),
            );
        }
        set.invalidate_declaration_hash_cache();
        set
    }

    fn set_of(skills: &[(&str, &str)], bundles: &[(&str, &str)]) -> DesiredSet {
        let skills: BTreeMap<String, crate::config::declaration::DeclaredSource> = skills
            .iter()
            .map(|(n, i)| {
                (
                    (*n).to_string(),
                    crate::config::declaration::DeclaredSource::Registry(Identifier::parse(i).unwrap()),
                )
            })
            .collect();
        let mut set = DesiredSet::from_parts(skills, BTreeMap::new());
        for (n, i) in bundles {
            set.bundles.insert(
                (*n).to_string(),
                crate::config::declaration::DeclaredSource::Registry(Identifier::parse(i).unwrap()),
            );
        }
        set.invalidate_declaration_hash_cache();
        set
    }

    fn snapshot(
        binding: &str,
        repo: &str,
        tag: &str,
        members: &[(&str, &str)],
    ) -> crate::lock::locked_bundle::LockedBundle {
        let pinned_id = Identifier::parse(&format!("{repo}:{tag}"))
            .unwrap()
            .clone_with_digest(Digest::Sha256("b".repeat(64)));
        crate::lock::locked_bundle::LockedBundle {
            name: binding.to_string(),
            source: crate::lock::locked_bundle::LockedBundleSource::Registry {
                repo: repo.to_string(),
                tag: tag.to_string(),
                pinned: PinnedIdentifier::try_from(pinned_id).unwrap(),
            },
            members: members
                .iter()
                .map(|(name, id)| crate::oci::bundle::BundleMember {
                    kind: ArtifactKind::Skill,
                    name: (*name).to_string(),
                    id: (*id).to_string(),
                })
                .collect(),
            transitive: false,
        }
    }

    #[test]
    fn drop_from_lock_removes_only_named_entry() {
        let mut prev = lock_of(vec![locked("a"), locked("b")]);
        let before = set_of(
            &[("a", "localhost:5000/acme/a:1"), ("b", "localhost:5000/acme/b:1")],
            &[],
        );
        stamp(&mut prev, &before);
        let after_set = set_of(&[("b", "localhost:5000/acme/b:1")], &[]);
        let out = drop_from_lock(&prev, ArtifactKind::Skill, "a", &before, &after_set);
        assert_eq!(out.lock.skills.len(), 1);
        assert_eq!(out.lock.skills[0].name, "b");
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert!(out.notes.is_empty());
    }

    #[test]
    fn drop_from_lock_preserves_unrelated_ghost_entries() {
        // "ghost" sits in the lock but in NEITHER declaration — a legacy
        // lock-only install. Removing an unrelated entry must not reap it.
        let prev = lock_of(vec![locked("a"), locked("ghost")]);
        let before = set_of(&[("a", "localhost:5000/acme/a:1")], &[]);
        let after_set = set_of(&[], &[]);
        let out = drop_from_lock(&prev, ArtifactKind::Skill, "a", &before, &after_set);
        let names: Vec<&str> = out.lock.skills.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["ghost"], "unrelated drift is preserved");
    }

    #[test]
    fn remove_direct_flips_to_bundle_provenance_when_ids_agree() {
        let mut prev = lock_of(vec![locked("cr")]);
        prev.bundles = vec![snapshot(
            "stack",
            "localhost:5000/acme/stack",
            "1",
            &[("cr", "localhost:5000/cr:1")],
        )];
        // `locked("cr")` pins repo `cr` on localhost:5000; declare the
        // direct id to match the bundle member's id exactly.
        let before = set_of(
            &[("cr", "localhost:5000/cr:1")],
            &[("stack", "localhost:5000/acme/stack:1")],
        );
        stamp(&mut prev, &before);
        let after_set = set_of(&[], &[("stack", "localhost:5000/acme/stack:1")]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "cr", &before, &after_set);
        assert_eq!(out.lock.skills.len(), 1, "the bundle still holds the artifact");
        assert_eq!(
            out.lock.skills[0].bundles,
            vec![crate::lock::locked_artifact::BundleProvenance::new(
                "localhost:5000/acme/stack",
                "1"
            )],
            "provenance flips from direct to the bundle"
        );
        assert_eq!(
            out.lock.metadata.declaration_hash,
            after_set.declaration_hash_cached(),
            "same-id flip keeps the lock fresh"
        );
    }

    #[test]
    fn remove_direct_with_id_mismatch_drops_and_skips_restamp() {
        let mut prev = lock_of(vec![locked("cr")]);
        prev.bundles = vec![snapshot(
            "stack",
            "localhost:5000/acme/stack",
            "1",
            &[("cr", "localhost:5000/cr:other")],
        )];
        let before = set_of(
            &[("cr", "localhost:5000/cr:1")],
            &[("stack", "localhost:5000/acme/stack:1")],
        );
        stamp(&mut prev, &before);
        let after_set = set_of(&[], &[("stack", "localhost:5000/acme/stack:1")]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "cr", &before, &after_set);
        assert!(out.lock.skills.is_empty(), "the unresolvable variant is dropped");
        assert_eq!(
            out.lock.metadata.declaration_hash,
            edited_from(&prev),
            "the hash restamp is skipped — the lock is honestly stale"
        );
        // The note must be explanatory (names the bundle, mentions grim lock,
        // says "stale") — not a dead-end warning that reads like an error.
        let note = out.notes.first().expect("one note produced");
        assert!(note.contains("grim lock"), "the user is told to re-resolve: {note}");
        assert!(
            note.contains("stale"),
            "the note communicates the lock is stale, not an error: {note}"
        );
        assert!(
            note.contains("localhost:5000/acme/stack"),
            "the note names the bundle so the user knows which bundle provides it: {note}"
        );
    }

    #[test]
    fn legacy_remove_direct_keeps_member_a_registry_bundle_still_provides() {
        // C1: a project with even one path bundle degrades the WHOLE
        // effective_set to the legacy path (effective_set's whole-call `?`).
        // Removing a directly-declared skill that a REGISTRY bundle also
        // provides must NOT drop it under a freshly restamped hash — keep the
        // lock entry and mark the lock honestly stale (run `grim lock`).
        use crate::config::path_source::PathSource;

        let mut prev = lock_of(vec![locked("shared")]);
        // A registry bundle whose cached snapshot still lists `shared`.
        prev.bundles = vec![snapshot(
            "regb",
            "localhost:5000/acme/stack",
            "1",
            &[("shared", "localhost:5000/shared:1")],
        )];

        // Declared set: direct skill `shared` + the registry bundle + an
        // unrelated PATH bundle whose presence forces the legacy path.
        let path = PathSource::parse("./bundles/local.toml").unwrap();
        let mut before = set_of(
            &[("shared", "localhost:5000/shared:1")],
            &[("regb", "localhost:5000/acme/stack:1")],
        );
        before.bundles.insert(
            "pathb".to_string(),
            crate::config::declaration::DeclaredSource::Path(path.clone()),
        );
        before.invalidate_declaration_hash_cache();
        stamp(&mut prev, &before);
        let mut after_set = set_of(&[], &[("regb", "localhost:5000/acme/stack:1")]);
        after_set.bundles.insert(
            "pathb".to_string(),
            crate::config::declaration::DeclaredSource::Path(path),
        );
        after_set.invalidate_declaration_hash_cache();

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "shared", &before, &after_set);

        let names: Vec<&str> = out.lock.skills.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["shared"],
            "a member a still-declared registry bundle provides must survive the direct removal"
        );
        assert_eq!(
            out.lock.metadata.declaration_hash,
            edited_from(&prev),
            "the restamp is skipped — the lock is honestly stale, not fresh-with-omission"
        );
    }

    #[test]
    fn remove_bundle_via_sets_keeps_shared_member_and_prunes_snapshot() {
        let mut prev = lock_of(vec![
            member_of(
                "shared",
                &[
                    ("localhost:5000/acme/stack-a", "1"),
                    ("localhost:5000/acme/stack-b", "1"),
                ],
            ),
            member_of("only-a", &[("localhost:5000/acme/stack-a", "1")]),
        ]);
        prev.bundles = vec![
            snapshot(
                "a",
                "localhost:5000/acme/stack-a",
                "1",
                &[("shared", "localhost:5000/m:1"), ("only-a", "localhost:5000/oa:1")],
            ),
            snapshot(
                "b",
                "localhost:5000/acme/stack-b",
                "1",
                &[("shared", "localhost:5000/m:1")],
            ),
        ];
        let before = set_of(
            &[],
            &[
                ("a", "localhost:5000/acme/stack-a:1"),
                ("b", "localhost:5000/acme/stack-b:1"),
            ],
        );
        let after_set = set_of(&[], &[("b", "localhost:5000/acme/stack-b:1")]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "a", &before, &after_set);
        let names: Vec<&str> = out.lock.skills.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["shared"],
            "the shared member survives, the exclusive one drops"
        );
        assert_eq!(
            out.lock.skills[0].bundles,
            vec![crate::lock::locked_artifact::BundleProvenance::new(
                "localhost:5000/acme/stack-b",
                "1"
            )],
            "contributors re-derived from the surviving bundle"
        );
        assert_eq!(out.lock.bundles.len(), 1, "the removed bundle's snapshot is pruned");
        assert_eq!(out.lock.bundles[0].name, "b");
    }

    // ── MCP entries (regression: the retain fan-out skipped lock.mcp) ────

    #[test]
    fn remove_mcp_drops_its_lock_entry() {
        // Regression: `drop_from_lock` fanned the effective-set retain pass
        // over skills/rules/agents only, then restamped `declaration_hash`
        // anyway — so an undeclared MCP server survived in a lock that read
        // FRESH, and the next `grim install` re-spliced it into every client
        // config (`iter_artifacts` chains `mcp`).
        let mut prev = lock_of(vec![]);
        prev.mcp = vec![locked_mcp("grim", &[])];
        let before = mcp_set_of(&[("grim", "localhost:5000/acme/grim:1")], &[]);
        stamp(&mut prev, &before);
        let after_set = mcp_set_of(&[], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Mcp, "grim", &before, &after_set);
        assert!(
            out.lock.mcp.is_empty(),
            "the undeclared mcp entry must leave the lock, or install resurrects it: {:?}",
            out.lock.mcp
        );
        assert_eq!(
            out.lock.metadata.declaration_hash,
            after_set.declaration_hash_cached(),
            "nothing is stale, so the hash restamps fresh"
        );
    }

    #[test]
    fn remove_mcp_keeps_an_unrelated_entry() {
        let mut prev = lock_of(vec![]);
        prev.mcp = vec![locked_mcp("grim", &[]), locked_mcp("other", &[])];
        let before = mcp_set_of(
            &[
                ("grim", "localhost:5000/acme/grim:1"),
                ("other", "localhost:5000/acme/other:1"),
            ],
            &[],
        );
        let after_set = mcp_set_of(&[("other", "localhost:5000/acme/other:1")], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Mcp, "grim", &before, &after_set);
        let names: Vec<&str> = out.lock.mcp.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["other"], "only the named entry drops");
    }

    #[test]
    fn remove_bundle_via_sets_evicts_its_mcp_member() {
        // The `Origin::Implied` re-derivation reaches lock.mcp too: removing
        // the only bundle that provided an mcp member must evict it.
        let mut prev = lock_of(vec![]);
        prev.mcp = vec![locked_mcp("grim", &[("localhost:5000/acme/stack", "1")])];
        prev.bundles = vec![crate::lock::locked_bundle::LockedBundle {
            name: "stack".to_string(),
            source: crate::lock::locked_bundle::LockedBundleSource::Registry {
                repo: "localhost:5000/acme/stack".to_string(),
                tag: "1".to_string(),
                pinned: PinnedIdentifier::try_from(
                    Identifier::parse("localhost:5000/acme/stack:1")
                        .unwrap()
                        .clone_with_digest(Digest::Sha256("b".repeat(64))),
                )
                .unwrap(),
            },
            members: vec![crate::oci::bundle::BundleMember {
                kind: ArtifactKind::Mcp,
                name: "grim".to_string(),
                id: "localhost:5000/acme/grim:1".to_string(),
            }],
            transitive: false,
        }];
        let before = set_of(&[], &[("stack", "localhost:5000/acme/stack:1")]);
        let after_set = set_of(&[], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "stack", &before, &after_set);
        assert!(
            out.lock.mcp.is_empty(),
            "the removed bundle's mcp member must be evicted: {:?}",
            out.lock.mcp
        );
    }

    // ── Legacy fallback (no usable [[bundle]] cache) ────────────────────

    #[test]
    fn legacy_bundle_eviction_drops_its_mcp_member() {
        // Same gap on the legacy path: `evict_bundle_members` retained over
        // skills/rules/agents only, so a bundle-provided mcp member survived
        // as an orphan under a freshly restamped hash. `prev.bundles` is
        // empty while a bundle is declared, which degrades `effective_set`
        // to `None` and forces the legacy path.
        let mut prev = lock_of(vec![]);
        prev.mcp = vec![locked_mcp("grim", &[("ghcr.io/acme/stack-a", "1")])];
        let before = set_of(&[], &[("a", "ghcr.io/acme/stack-a:1")]);
        let after_set = set_of(&[], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "a", &before, &after_set);
        assert!(
            out.lock.mcp.is_empty(),
            "the sole contributor's removal must evict the mcp member: {:?}",
            out.lock.mcp
        );
    }

    // ── Settings: an undeclared entry must leave a lock that reads fresh ─

    #[test]
    fn remove_settings_drops_its_lock_entry() {
        let mut prev = lock_of(vec![]);
        prev.settings = vec![locked_settings("team", &[])];
        let before = settings_set_of(&[("team", "localhost:5000/acme/team:1")], &[]);
        stamp(&mut prev, &before);
        let after_set = settings_set_of(&[], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Settings, "team", &before, &after_set);
        assert!(
            out.lock.settings.is_empty(),
            "the undeclared entry must leave the lock: {:?}",
            out.lock.settings
        );
        assert_eq!(
            out.lock.metadata.declaration_hash,
            after_set.declaration_hash_cached(),
            "nothing is stale, so the hash restamps fresh"
        );
    }

    #[test]
    fn remove_settings_keeps_an_unrelated_entry() {
        let mut prev = lock_of(vec![]);
        prev.settings = vec![locked_settings("team", &[]), locked_settings("other", &[])];
        let before = settings_set_of(
            &[
                ("team", "localhost:5000/acme/team:1"),
                ("other", "localhost:5000/acme/other:1"),
            ],
            &[],
        );
        let after_set = settings_set_of(&[("other", "localhost:5000/acme/other:1")], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Settings, "team", &before, &after_set);
        let names: Vec<&str> = out.lock.settings.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["other"], "only the named entry drops");
    }

    #[test]
    fn remove_bundle_via_sets_evicts_its_settings_member() {
        let mut prev = lock_of(vec![]);
        prev.settings = vec![locked_settings("team", &[("localhost:5000/acme/stack", "1")])];
        prev.bundles = vec![snapshot_of_kind(
            "stack",
            "localhost:5000/acme/stack",
            "1",
            ArtifactKind::Settings,
            ("team", "localhost:5000/acme/team:1"),
        )];
        let before = set_of(&[], &[("stack", "localhost:5000/acme/stack:1")]);
        let after_set = set_of(&[], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "stack", &before, &after_set);
        assert!(
            out.lock.settings.is_empty(),
            "the removed bundle's settings member must be evicted: {:?}",
            out.lock.settings
        );
    }

    #[test]
    fn legacy_bundle_eviction_drops_its_settings_member() {
        // `prev.bundles` empty while a bundle is declared forces the legacy path.
        let mut prev = lock_of(vec![]);
        prev.settings = vec![locked_settings("team", &[("ghcr.io/acme/stack-a", "1")])];
        let before = set_of(&[], &[("a", "ghcr.io/acme/stack-a:1")]);
        let after_set = set_of(&[], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "a", &before, &after_set);
        assert!(
            out.lock.settings.is_empty(),
            "the sole contributor's removal must evict the member: {:?}",
            out.lock.settings
        );
    }

    #[test]
    fn legacy_target_drop_keeps_a_settings_entry_an_unlisted_bundle_may_provide() {
        // A declared bundle with no member list degrades the whole call to the
        // target-only path, and nothing there shows that bundle no longer
        // provides the package: its row stays and the lock reads stale.
        let mut prev = lock_of(vec![]);
        prev.settings = vec![locked_settings("team", &[])];
        let mut before = settings_set_of(
            &[("team", "localhost:5000/acme/team:1")],
            &[("a", "ghcr.io/acme/stack-a:1")],
        );
        before.invalidate_declaration_hash_cache();
        let after_set = settings_set_of(&[], &[("a", "ghcr.io/acme/stack-a:1")]);

        let out = drop_from_lock(&prev, ArtifactKind::Settings, "team", &before, &after_set);
        let names: Vec<&str> = out.lock.settings.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["team"], "the row a declared bundle may provide stays");
        assert_ne!(
            out.lock.metadata.declaration_hash,
            after_set.declaration_hash_cached(),
            "the lock reads stale"
        );
        // The pin records no tag, and nothing proves the lock, so the relock
        // resolves it afresh.
        assert!(
            out.notes
                .iter()
                .any(|note| note.contains("settings 'team' keeps its row") && note.contains("afresh")),
            "{:?}",
            out.notes
        );
    }

    /// A dependency another row's edge names, with no row in the lock, is read only by
    /// the next relock, so nothing offline shows it does not require the removed key.
    /// Dropped, that key's pin moves at the relock though the dependency asks for it.
    #[test]
    fn incomputable_graph_keeps_a_settings_target_a_dependency_with_no_row_may_require() {
        // A `requires` edge makes the lock dependency-bearing, so the
        // incomputable graph takes the target-only path, not the legacy one.
        let mut other = locked("other");
        other.requires = vec![crate::oci::bundle::BundleMember {
            kind: ArtifactKind::Skill,
            name: "absent".to_string(),
            id: "localhost:5000/acme/absent:1".to_string(),
        }];
        let mut prev = lock_of(vec![other]);
        prev.settings = vec![locked_settings("team", &[])];
        let before = settings_set_of(&[("team", "localhost:5000/acme/team:1")], &[]);
        let mut after_set = settings_set_of(&[], &[]);
        after_set.skills.insert(
            "other".to_string(),
            crate::config::declaration::DeclaredSource::Registry(
                Identifier::parse("localhost:5000/acme/other:1").unwrap(),
            ),
        );
        after_set.invalidate_declaration_hash_cache();
        let mut before = before;
        before.skills = after_set.skills.clone();
        before.invalidate_declaration_hash_cache();

        let out = drop_from_lock(&prev, ArtifactKind::Settings, "team", &before, &after_set);
        assert_eq!(out.lock.settings, prev.settings, "the row stays");
        assert_eq!(out.lock.skills.len(), 1, "every other entry is written untouched");
        assert_ne!(
            out.lock.metadata.declaration_hash,
            after_set.declaration_hash_cached(),
            "the lock stays honestly stale"
        );
        assert!(
            out.notes.iter().any(|note| note.contains("skill 'absent'")),
            "{:?}",
            out.notes
        );
    }

    // ── Hook: the same fan-out gap, with re-arming as its blast radius ───

    #[test]
    fn remove_hook_drops_its_lock_entry() {
        // Regression: `drop_from_lock`'s retain pass never covered `lock.hooks`,
        // yet the hash restamped anyway — so an undeclared `[[hook]]` survived
        // in a lock reading FRESH, and the next `grim install` re-materialized
        // AND re-armed it. Strictly worse than the `mcp` case this mirrors: the
        // resurrected artifact is code a client runs automatically.
        let mut prev = lock_of(vec![]);
        prev.hooks = vec![locked_hook("shell-guard", &[])];
        let before = hook_set_of(&[("shell-guard", "localhost:5000/acme/shell-guard:1")], &[]);
        stamp(&mut prev, &before);
        let after_set = hook_set_of(&[], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Hook, "shell-guard", &before, &after_set);
        assert!(
            out.lock.hooks.is_empty(),
            "the undeclared hook must leave the lock, or install re-arms it: {:?}",
            out.lock.hooks
        );
        assert_eq!(
            out.lock.metadata.declaration_hash,
            after_set.declaration_hash_cached(),
            "nothing is stale, so the hash restamps fresh"
        );
    }

    #[test]
    fn remove_hook_keeps_an_unrelated_entry() {
        let mut prev = lock_of(vec![]);
        prev.hooks = vec![locked_hook("shell-guard", &[]), locked_hook("fmt-gate", &[])];
        let before = hook_set_of(
            &[
                ("shell-guard", "localhost:5000/acme/shell-guard:1"),
                ("fmt-gate", "localhost:5000/acme/fmt-gate:1"),
            ],
            &[],
        );
        let after_set = hook_set_of(&[("fmt-gate", "localhost:5000/acme/fmt-gate:1")], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Hook, "shell-guard", &before, &after_set);
        let names: Vec<&str> = out.lock.hooks.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["fmt-gate"], "only the named hook drops");
    }

    #[test]
    fn remove_bundle_via_sets_evicts_its_hook_member() {
        // The requirement's inverse direction, on the primary (snapshot) path:
        // dropping the bundle drops the hook it provided.
        let mut prev = lock_of(vec![]);
        prev.hooks = vec![locked_hook("shell-guard", &[("localhost:5000/acme/stack", "1")])];
        prev.bundles = vec![snapshot_of_kind(
            "stack",
            "localhost:5000/acme/stack",
            "1",
            ArtifactKind::Hook,
            ("shell-guard", "localhost:5000/acme/shell-guard:1"),
        )];
        let before = set_of(&[], &[("stack", "localhost:5000/acme/stack:1")]);
        let after_set = set_of(&[], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "stack", &before, &after_set);
        assert!(
            out.lock.hooks.is_empty(),
            "the removed bundle's hook member must be evicted: {:?}",
            out.lock.hooks
        );
    }

    #[test]
    fn remove_bundle_keeps_a_directly_declared_hook() {
        // The other half of the requirement: a hook the bundle provided AND the
        // user declared directly survives the bundle's removal, because the
        // direct declaration still holds it in the effective set. The entry's
        // bundle provenance is re-derived away, not the entry itself.
        let mut prev = lock_of(vec![]);
        prev.hooks = vec![locked_hook("shell-guard", &[("localhost:5000/acme/stack", "1")])];
        prev.bundles = vec![snapshot_of_kind(
            "stack",
            "localhost:5000/acme/stack",
            "1",
            ArtifactKind::Hook,
            ("shell-guard", "localhost:5000/acme/shell-guard:1"),
        )];
        let before = hook_set_of(
            &[("shell-guard", "localhost:5000/acme/shell-guard:1")],
            &[("stack", "localhost:5000/acme/stack:1")],
        );
        let after_set = hook_set_of(&[("shell-guard", "localhost:5000/acme/shell-guard:1")], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "stack", &before, &after_set);
        let names: Vec<&str> = out.lock.hooks.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["shell-guard"],
            "a directly-declared hook outlives the bundle that also provided it"
        );
    }

    #[test]
    fn legacy_bundle_eviction_drops_its_hook_member() {
        // Same gap on the legacy path, and this one is not a corner case:
        // `effective_set` degrades for the WHOLE call, so one path-declared
        // bundle anywhere routes every mutation through `evict_bundle_members`.
        // `prev.bundles` empty while a bundle is declared forces that path.
        let mut prev = lock_of(vec![]);
        prev.hooks = vec![locked_hook("shell-guard", &[("ghcr.io/acme/stack-a", "1")])];
        let before = set_of(&[], &[("a", "ghcr.io/acme/stack-a:1")]);
        let after_set = set_of(&[], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "a", &before, &after_set);
        assert!(
            out.lock.hooks.is_empty(),
            "the sole contributor's removal must evict the hook member: {:?}",
            out.lock.hooks
        );
    }

    #[test]
    fn legacy_bundle_eviction_keeps_hook_shared_with_other_bundle() {
        let mut prev = lock_of(vec![]);
        prev.hooks = vec![locked_hook(
            "shared-guard",
            &[("ghcr.io/acme/stack-a", "1"), ("ghcr.io/acme/stack-b", "1")],
        )];
        let before = set_of(&[], &[("a", "ghcr.io/acme/stack-a:1"), ("b", "ghcr.io/acme/stack-b:1")]);
        let after_set = set_of(&[], &[("b", "ghcr.io/acme/stack-b:1")]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "a", &before, &after_set);
        let names: Vec<&str> = out.lock.hooks.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["shared-guard"],
            "a surviving bundle still provides it, so it stays"
        );
        assert_eq!(
            out.lock.hooks[0].bundles.len(),
            1,
            "only the removed bundle's provenance is dropped"
        );
    }

    #[test]
    fn bundle_eviction_keeps_member_shared_with_other_bundle() {
        let mut prev = lock_of(vec![
            member_of(
                "shared",
                &[("ghcr.io/acme/stack-a", "1"), ("ghcr.io/acme/stack-b", "1")],
            ),
            member_of("only-a", &[("ghcr.io/acme/stack-a", "1")]),
            locked("direct"),
        ]);
        // The lock carries no snapshots while bundles are declared — the
        // legacy provenance-based eviction must run.
        let before = set_of(&[], &[("a", "ghcr.io/acme/stack-a:1"), ("b", "ghcr.io/acme/stack-b:1")]);
        let after_set = set_of(&[], &[("b", "ghcr.io/acme/stack-b:1")]);
        stamp(&mut prev, &before);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "a", &before, &after_set);

        let names: Vec<&str> = out.lock.skills.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["shared", "direct"],
            "exclusive member evicted, shared + direct stay"
        );
        assert_eq!(
            out.lock.skills[0].bundles,
            vec![crate::lock::locked_artifact::BundleProvenance::new(
                "ghcr.io/acme/stack-b",
                "1"
            )],
            "the evicted bundle's provenance entry is stripped"
        );
    }

    #[test]
    fn bundle_eviction_drops_member_when_last_provenance_goes() {
        let prev = lock_of(vec![member_of("m", &[("ghcr.io/acme/stack-a", "1")])]);
        let before = set_of(&[], &[("a", "ghcr.io/acme/stack-a:1")]);
        let after_set = set_of(&[], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "a", &before, &after_set);
        assert!(
            out.lock.skills.is_empty(),
            "the sole contributor's removal evicts the member"
        );
    }

    #[test]
    fn remove_local_bundle_evicts_member_via_snapshot_provenance() {
        // B1: a path (local) bundle has no `(repo, tag)`, so its members must
        // be evicted by keying on the `[[bundle]]` snapshot's
        // `provenance_pair()` — otherwise the member the bundle provided
        // survives as an orphan under a freshly restamped hash and the next
        // `grim install` re-materializes it.
        use crate::config::path_source::PathSource;
        use crate::lock::locked_bundle::{LockedBundle, LockedBundleSource};

        let path = PathSource::parse("./bundles/x.toml").unwrap();
        let hash = Digest::Sha256("c".repeat(64));
        let (repo, tag) = (path.as_str().to_string(), hash.to_short_string());

        let mut prev = lock_of(vec![member_of("code-review", &[(repo.as_str(), tag.as_str())])]);
        prev.bundles = vec![LockedBundle {
            name: "x".to_string(),
            source: LockedBundleSource::Path {
                path: path.clone(),
                hash: hash.clone(),
            },
            members: vec![crate::oci::bundle::BundleMember {
                kind: ArtifactKind::Skill,
                name: "code-review".to_string(),
                id: "localhost:5000/code-review:1".to_string(),
            }],
            transitive: false,
        }];

        let mut before = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        before
            .bundles
            .insert("x".to_string(), crate::config::declaration::DeclaredSource::Path(path));
        before.invalidate_declaration_hash_cache();
        stamp(&mut prev, &before);
        let mut after_set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        after_set.invalidate_declaration_hash_cache();

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "x", &before, &after_set);
        assert!(
            out.lock.skills.is_empty(),
            "the member only the path bundle provided must be evicted: {:?}",
            out.lock.skills
        );
        assert!(out.lock.bundles.is_empty(), "the path-bundle snapshot must be pruned");
        assert_eq!(
            out.lock.metadata.declaration_hash,
            after_set.declaration_hash_cached(),
            "eviction removed every orphan, so the hash restamps fresh"
        );
    }

    #[test]
    fn bundle_eviction_skipped_while_duplicate_binding_remains() {
        // The same bundle (repo AND tag) declared under a second binding
        // name: removing one binding must not evict anything (legacy path —
        // the remaining binding has no matching snapshot).
        let prev = lock_of(vec![member_of("m", &[("localhost:5000/acme/stack", "1")])]);
        let before = set_of(
            &[],
            &[
                ("first", "localhost:5000/acme/stack:1"),
                ("second", "localhost:5000/acme/stack:1"),
            ],
        );
        let after_set = set_of(&[], &[("second", "localhost:5000/acme/stack:1")]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "first", &before, &after_set);
        assert_eq!(
            out.lock.skills.len(),
            1,
            "members survive while a duplicate binding remains"
        );
        assert_eq!(
            out.lock.skills[0].bundles.len(),
            1,
            "the provenance entry is kept for the remaining binding"
        );
    }

    #[test]
    fn evict_skips_member_when_second_path_binding_shares_bundle() {
        // R1: two path bindings (`x`, `y`) point at the SAME local bundle, so
        // both stamp the same `(path, hash-short)` provenance onto the shared
        // member. Removing `x` must NOT evict the member `y` still provides —
        // a path binding has no `identifier()`, so its duplicate-binding
        // survival is read from its `[[bundle]]` snapshot's `provenance_pair()`.
        use crate::config::path_source::PathSource;
        use crate::lock::locked_bundle::{LockedBundle, LockedBundleSource};

        let path = PathSource::parse("./bundles/shared.toml").unwrap();
        let hash = Digest::Sha256("c".repeat(64));
        let (repo, tag) = (path.as_str().to_string(), hash.to_short_string());

        let mut prev = lock_of(vec![member_of("code-review", &[(repo.as_str(), tag.as_str())])]);
        let path_snapshot = |binding: &str| LockedBundle {
            name: binding.to_string(),
            source: LockedBundleSource::Path {
                path: path.clone(),
                hash: hash.clone(),
            },
            members: vec![crate::oci::bundle::BundleMember {
                kind: ArtifactKind::Skill,
                name: "code-review".to_string(),
                id: "localhost:5000/code-review:1".to_string(),
            }],
            transitive: false,
        };
        prev.bundles = vec![path_snapshot("x"), path_snapshot("y")];

        let path_source = |p: &PathSource| crate::config::declaration::DeclaredSource::Path(p.clone());
        let mut before = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        before.bundles.insert("x".to_string(), path_source(&path));
        before.bundles.insert("y".to_string(), path_source(&path));
        before.invalidate_declaration_hash_cache();
        let mut after_set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        after_set.bundles.insert("y".to_string(), path_source(&path));
        after_set.invalidate_declaration_hash_cache();

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "x", &before, &after_set);
        let names: Vec<&str> = out.lock.skills.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["code-review"],
            "the member the second path binding still provides must survive: {:?}",
            out.lock.skills
        );
        assert_eq!(
            out.lock.bundles.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            vec!["y"],
            "only the removed binding's snapshot is pruned; the survivor stays"
        );
    }

    #[test]
    fn remove_local_bundle_without_snapshot_keeps_member_and_marks_stale() {
        // R5: a path bundle removal where the lock carries NO matching
        // `[[bundle]]` snapshot (hand-edited lock). With neither an
        // `identifier()` nor a cached snapshot, membership is unknowable
        // offline — the orphaned member is retained and the hash restamp is
        // skipped so the lock reads honestly stale rather than fresh-with-orphans.
        use crate::config::path_source::PathSource;

        let path = PathSource::parse("./bundles/gone.toml").unwrap();
        let repo = path.as_str().to_string();

        // `lock_of` leaves `prev.bundles` empty — no snapshot for the bundle.
        let mut prev = lock_of(vec![member_of("code-review", &[(repo.as_str(), "abcdef12")])]);

        let mut before = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        before
            .bundles
            .insert("x".to_string(), crate::config::declaration::DeclaredSource::Path(path));
        before.invalidate_declaration_hash_cache();
        stamp(&mut prev, &before);
        let mut after_set = DesiredSet::from_parts(BTreeMap::new(), BTreeMap::new());
        after_set.invalidate_declaration_hash_cache();

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "x", &before, &after_set);
        assert_eq!(
            out.lock.metadata.declaration_hash, prev.metadata.declaration_hash,
            "no snapshot: the hash restamp is skipped, so the lock reads honestly stale"
        );
        let names: Vec<&str> = out.lock.skills.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["code-review"],
            "the orphaned member is retained — it cannot be safely evicted offline"
        );
    }

    #[test]
    fn remove_on_a_stale_lock_keeps_it_stale() {
        // The hand edit moved `edited` to tag 2 after `grim lock` pinned tag 1.
        let mut prev = lock_of(vec![locked("edited"), locked("other")]);
        stamp(
            &mut prev,
            &set_of(
                &[
                    ("edited", "localhost:5000/acme/edited:1"),
                    ("other", "localhost:5000/acme/other:1"),
                ],
                &[],
            ),
        );
        let before = set_of(
            &[
                ("edited", "localhost:5000/acme/edited:2"),
                ("other", "localhost:5000/acme/other:1"),
            ],
            &[],
        );
        let after_set = set_of(&[("edited", "localhost:5000/acme/edited:2")], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "other", &before, &after_set);

        let names: Vec<&str> = out.lock.skills.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["edited"], "the named entry still leaves the lock");
        assert_eq!(
            out.lock.metadata.declaration_hash,
            edited_from(&prev),
            "a restamp would mark the old pin of the edited entry as locked"
        );
    }

    /// Every legacy-path removal of a row is one where a declared bundle has no
    /// member list. A stale lock keeps the row; a proven one, where nothing offline
    /// shows the bundle provides it, takes 0.15.1's drop and restamp.
    #[test]
    fn the_legacy_path_keeps_the_row_only_on_a_stale_lock() {
        use crate::config::path_source::PathSource;

        // A path bundle with no snapshot makes `effective_set` give up, so both
        // runs take the legacy path.
        let with_path_bundle = |skills: &[(&str, &str)]| {
            let mut set = set_of(skills, &[]);
            set.bundles.insert(
                "pathb".to_string(),
                crate::config::declaration::DeclaredSource::Path(PathSource::parse("./bundles/local.toml").unwrap()),
            );
            set.invalidate_declaration_hash_cache();
            set
        };
        let before = with_path_bundle(&[
            ("edited", "localhost:5000/acme/edited:2"),
            ("other", "localhost:5000/acme/other:1"),
        ]);
        let after_set = with_path_bundle(&[("edited", "localhost:5000/acme/edited:2")]);

        let mut prev = lock_of(vec![locked("edited"), locked("other")]);
        let stale = drop_from_lock(&prev, ArtifactKind::Skill, "other", &before, &after_set);
        assert_eq!(skill_names(&stale.lock), ["edited", "other"], "the row stays");
        assert_eq!(
            stale.lock.metadata.declaration_hash, prev.metadata.declaration_hash,
            "no row changed, so a stale lock keeps its stale hash"
        );

        stamp(&mut prev, &before);
        let fresh = drop_from_lock(&prev, ArtifactKind::Skill, "other", &before, &after_set);
        assert_eq!(
            skill_names(&fresh.lock),
            ["edited"],
            "nothing shows `pathb` provides it"
        );
        assert_eq!(
            fresh.lock.metadata.declaration_hash,
            after_set.declaration_hash_cached(),
            "a proven lock restamps, so `grim install` exits 0 as on 0.15.1"
        );
        assert_eq!(fresh.notes.len(), 1, "{:?}", fresh.notes);
        assert!(fresh.notes[0].contains("bundle 'pathb'"), "{:?}", fresh.notes);
    }

    // ── Nested bundles: a snapshot follows the effective set ─────────────

    const OUTER: &str = "localhost:5000/acme/outer";
    const INNER: &str = "localhost:5000/acme/inner";

    fn skill(name: &str) -> (ArtifactKind, String, String) {
        (
            ArtifactKind::Skill,
            name.to_string(),
            format!("localhost:5000/{name}:1"),
        )
    }

    fn include(name: &str, tag: &str) -> (ArtifactKind, String, String) {
        (
            ArtifactKind::Bundle,
            name.to_string(),
            format!("localhost:5000/acme/{name}:{tag}"),
        )
    }

    /// A snapshot of `localhost:5000/acme/<binding>:<tag>` listing `members`.
    fn row(binding: &str, tag: &str, members: &[(ArtifactKind, String, String)]) -> LockedBundle {
        let repo = format!("localhost:5000/acme/{binding}");
        let pinned = Identifier::parse(&format!("{repo}:{tag}"))
            .unwrap()
            .clone_with_digest(Digest::Sha256("b".repeat(64)));
        LockedBundle {
            name: binding.to_string(),
            source: LockedBundleSource::Registry {
                repo,
                tag: tag.to_string(),
                pinned: PinnedIdentifier::try_from(pinned).unwrap(),
            },
            members: members
                .iter()
                .map(|(kind, name, id)| crate::oci::bundle::BundleMember {
                    kind: *kind,
                    name: name.clone(),
                    id: id.clone(),
                })
                .collect(),
            transitive: false,
        }
    }

    fn nested(binding: &str, tag: &str, members: &[(ArtifactKind, String, String)]) -> LockedBundle {
        LockedBundle {
            transitive: true,
            ..row(binding, tag, members)
        }
    }

    /// `outer` holds `beta` and nests `inner`, which holds `gamma`.
    fn nested_lock() -> GrimoireLock {
        let mut lock = lock_of(vec![
            member_of("beta", &[(OUTER, "1")]),
            member_of("gamma", &[(INNER, "1")]),
        ]);
        lock.bundles = vec![
            nested("inner", "1", &[skill("gamma")]),
            row("outer", "1", &[skill("beta"), include("inner", "1")]),
        ];
        lock
    }

    fn bundle_names(lock: &GrimoireLock) -> Vec<&str> {
        lock.bundles.iter().map(|b| b.name.as_str()).collect()
    }

    fn skill_names(lock: &GrimoireLock) -> Vec<&str> {
        lock.skills.iter().map(|a| a.name.as_str()).collect()
    }

    #[test]
    fn removing_an_unrelated_skill_keeps_a_nested_snapshot() {
        let mut prev = nested_lock();
        prev.skills.push(locked("alpha"));
        let before = set_of(
            &[("alpha", "localhost:5000/alpha:1")],
            &[("outer", "localhost:5000/acme/outer:1")],
        );
        stamp(&mut prev, &before);
        let after_set = set_of(&[], &[("outer", "localhost:5000/acme/outer:1")]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "alpha", &before, &after_set);

        assert_eq!(
            out.lock.bundles, prev.bundles,
            "the lock's only pin of the nested bundle must survive"
        );
        assert_eq!(skill_names(&out.lock), ["beta", "gamma"]);
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    #[test]
    fn a_snapshot_stays_at_any_depth_the_effective_set_reaches() {
        let mut prev = lock_of(vec![locked("alpha"), member_of("leaf", &[(INNER, "1")])]);
        prev.bundles = vec![
            nested("inner", "1", &[skill("leaf")]),
            nested("mid", "1", &[include("inner", "1")]),
            row("outer", "1", &[include("mid", "1")]),
        ];
        let before = set_of(
            &[("alpha", "localhost:5000/alpha:1")],
            &[("outer", "localhost:5000/acme/outer:1")],
        );
        stamp(&mut prev, &before);
        let after_set = set_of(&[], &[("outer", "localhost:5000/acme/outer:1")]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "alpha", &before, &after_set);

        assert_eq!(out.lock.bundles, prev.bundles);
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
    }

    #[test]
    fn a_snapshot_a_requires_edge_reaches_stays_while_a_parent_remains() {
        let needs_pack = |name: &str| {
            let mut a = locked(name);
            a.requires = vec![crate::oci::bundle::BundleMember {
                kind: ArtifactKind::Bundle,
                name: "pack".to_string(),
                id: "localhost:5000/acme/pack:1".to_string(),
            }];
            a
        };
        let pack = "localhost:5000/acme/pack";
        let mut prev = lock_of(vec![
            needs_pack("one"),
            needs_pack("two"),
            member_of("mem", &[(pack, "1")]),
        ]);
        prev.bundles = vec![nested("pack", "1", &[skill("mem")])];
        let before = set_of(&[("one", "localhost:5000/one:1"), ("two", "localhost:5000/two:1")], &[]);
        stamp(&mut prev, &before);
        let after_set = set_of(&[("two", "localhost:5000/two:1")], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "one", &before, &after_set);

        assert_eq!(bundle_names(&out.lock), ["pack"]);
        assert_eq!(skill_names(&out.lock), ["two", "mem"]);
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
    }

    #[test]
    fn removing_one_of_two_nesting_bundles_keeps_the_include() {
        let mut prev = lock_of(vec![member_of("gamma", &[(INNER, "1")])]);
        prev.bundles = vec![
            row("first", "1", &[include("inner", "1")]),
            nested("inner", "1", &[skill("gamma")]),
            row("second", "1", &[include("inner", "1")]),
        ];
        let before = set_of(
            &[],
            &[
                ("first", "localhost:5000/acme/first:1"),
                ("second", "localhost:5000/acme/second:1"),
            ],
        );
        stamp(&mut prev, &before);
        let after_set = set_of(&[], &[("second", "localhost:5000/acme/second:1")]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "first", &before, &after_set);

        assert_eq!(bundle_names(&out.lock), ["inner", "second"]);
        assert_eq!(skill_names(&out.lock), ["gamma"]);
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
    }

    #[test]
    fn removing_a_direct_skill_a_nested_bundle_provides_keeps_the_snapshot() {
        let mut prev = lock_of(vec![locked("gamma"), member_of("beta", &[(OUTER, "1")])]);
        prev.bundles = vec![
            nested("inner", "1", &[skill("gamma")]),
            row("outer", "1", &[skill("beta"), include("inner", "1")]),
        ];
        let before = set_of(
            &[("gamma", "localhost:5000/gamma:1")],
            &[("outer", "localhost:5000/acme/outer:1")],
        );
        stamp(&mut prev, &before);
        let after_set = set_of(&[], &[("outer", "localhost:5000/acme/outer:1")]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "gamma", &before, &after_set);

        assert_eq!(out.lock.bundles, prev.bundles);
        assert_eq!(skill_names(&out.lock), ["gamma", "beta"]);
        assert_eq!(
            out.lock.skills[0].bundles,
            vec![crate::lock::locked_artifact::BundleProvenance::new(INNER, "1")],
            "provenance flips from direct to the nested bundle"
        );
    }

    #[test]
    fn removing_the_last_nesting_bundle_drops_the_include_and_its_members() {
        let mut prev = nested_lock();
        let before = set_of(&[], &[("outer", "localhost:5000/acme/outer:1")]);
        stamp(&mut prev, &before);
        let after_set = set_of(&[], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "outer", &before, &after_set);

        assert!(out.lock.bundles.is_empty(), "{:?}", out.lock.bundles);
        assert!(out.lock.skills.is_empty(), "{:?}", out.lock.skills);
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
    }

    #[test]
    fn a_dependency_loses_the_stamp_of_a_bundle_that_left_the_lock() {
        let pack = "localhost:5000/acme/pack";
        let requiring = |name: &str, member: crate::oci::bundle::BundleMember| {
            let mut a = locked(name);
            a.requires = vec![member];
            a
        };
        let mut prev = lock_of(vec![
            requiring(
                "k",
                crate::oci::bundle::BundleMember {
                    kind: ArtifactKind::Bundle,
                    name: "pack".to_string(),
                    id: "localhost:5000/acme/pack:1".to_string(),
                },
            ),
            requiring(
                "q",
                crate::oci::bundle::BundleMember {
                    kind: ArtifactKind::Skill,
                    name: "shared".to_string(),
                    id: "localhost:5000/shared:1".to_string(),
                },
            ),
            member_of("mem", &[(pack, "1")]),
            member_of("shared", &[(pack, "1")]),
        ]);
        prev.bundles = vec![nested("pack", "1", &[skill("mem"), skill("shared")])];
        let before = set_of(&[("k", "localhost:5000/k:1"), ("q", "localhost:5000/q:1")], &[]);
        stamp(&mut prev, &before);
        let after_set = set_of(&[("q", "localhost:5000/q:1")], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "k", &before, &after_set);

        assert!(out.lock.bundles.is_empty(), "nothing reaches the bundle any more");
        assert_eq!(skill_names(&out.lock), ["q", "shared"]);
        assert!(
            out.lock.skills[1].bundles.is_empty(),
            "`q` alone holds it, as a relock would record: {:?}",
            out.lock.skills[1].bundles
        );
    }

    // ── A removed direct bundle unmasks the include it overrode ──────────

    /// The direct `inner:2` overrides the `inner:1` that `outer` nests.
    fn overriding_lock() -> (GrimoireLock, DesiredSet, DesiredSet) {
        let mut lock = lock_of(vec![member_of("a2", &[(INNER, "2")]), member_of("b1", &[(OUTER, "1")])]);
        lock.bundles = vec![
            row("inner", "2", &[skill("a2")]),
            row("outer", "1", &[skill("b1"), include("inner", "1")]),
        ];
        let before = set_of(
            &[],
            &[
                ("inner", "localhost:5000/acme/inner:2"),
                ("outer", "localhost:5000/acme/outer:1"),
            ],
        );
        stamp(&mut lock, &before);
        let after_set = set_of(&[], &[("outer", "localhost:5000/acme/outer:1")]);
        (lock, before, after_set)
    }

    fn assert_honestly_stale(out: &Edited, prev: &GrimoireLock) {
        assert_eq!(
            out.lock.metadata.declaration_hash,
            edited_from(prev),
            "a restamp would call a lock fresh that holds what the removed bundle asked for"
        );
        assert_eq!(bundle_names(&out.lock), ["outer"], "the override's snapshot leaves");
        assert_eq!(
            skill_names(&out.lock),
            skill_names(prev),
            "every other entry stays as locked"
        );
        assert_eq!(out.notes.len(), 1, "{:?}", out.notes);
        assert!(out.notes[0].contains("grim lock"), "{:?}", out.notes);
    }

    #[test]
    fn removing_a_bundle_that_hid_a_nested_include_leaves_the_lock_stale() {
        let (prev, before, after_set) = overriding_lock();

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "inner", &before, &after_set);

        assert_honestly_stale(&out, &prev);
    }

    #[test]
    fn a_dependency_bearing_lock_goes_stale_on_the_same_override() {
        let (mut prev, before, after_set) = overriding_lock();
        prev.skills[1].requires = vec![crate::oci::bundle::BundleMember {
            kind: ArtifactKind::Skill,
            name: "a2".to_string(),
            id: "localhost:5000/a2:1".to_string(),
        }];

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "inner", &before, &after_set);

        assert_honestly_stale(&out, &prev);
    }

    #[test]
    fn a_second_parent_at_another_version_ends_the_proof() {
        // `inner:2` answers `outer`, and not `other`, whose `:3` it hid and no
        // resolve ever answered.
        let mut prev = lock_of(vec![member_of("a2", &[(INNER, "2")])]);
        prev.bundles = vec![
            row("inner", "2", &[skill("a2")]),
            row("other", "1", &[include("inner", "3")]),
            row("outer", "1", &[include("inner", "2")]),
        ];
        let before = set_of(
            &[],
            &[
                ("inner", "localhost:5000/acme/inner:2"),
                ("other", "localhost:5000/acme/other:1"),
                ("outer", "localhost:5000/acme/outer:1"),
            ],
        );
        stamp(&mut prev, &before);
        let after_set = set_of(
            &[],
            &[
                ("other", "localhost:5000/acme/other:1"),
                ("outer", "localhost:5000/acme/outer:1"),
            ],
        );

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "inner", &before, &after_set);

        assert_eq!(
            out.lock.metadata.declaration_hash,
            edited_from(&prev),
            "the edit ended the proof"
        );
        assert_eq!(bundle_names(&out.lock), ["inner", "other", "outer"]);
        assert!(out.lock.bundles[0].transitive, "only an edge reaches it now");
        assert_eq!(out.notes.len(), 1, "{:?}", out.notes);
        assert!(out.notes[0].contains("inner:3"), "{:?}", out.notes);
        assert!(out.notes[0].contains("grim lock"), "{:?}", out.notes);
    }

    #[test]
    fn removing_a_bundle_at_the_version_its_include_asks_for_stays_fresh() {
        let mut prev = lock_of(vec![member_of("a1", &[(INNER, "1")]), member_of("b1", &[(OUTER, "1")])]);
        prev.bundles = vec![
            row("inner", "1", &[skill("a1")]),
            row("outer", "1", &[skill("b1"), include("inner", "1")]),
        ];
        let before = set_of(
            &[],
            &[
                ("inner", "localhost:5000/acme/inner:1"),
                ("outer", "localhost:5000/acme/outer:1"),
            ],
        );
        stamp(&mut prev, &before);
        let after_set = set_of(&[], &[("outer", "localhost:5000/acme/outer:1")]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "inner", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert_eq!(skill_names(&out.lock), ["a1", "b1"]);
        assert_eq!(bundle_names(&out.lock), ["inner", "outer"]);
        assert!(
            out.lock.bundles[0].transitive,
            "only an edge reaches it now, which is how a relock records it"
        );
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    /// With no closure to follow only the target leaves, and a bundle an include
    /// still reaches stays the way a relock records it: held by an edge alone.
    #[test]
    fn the_target_only_path_marks_a_bundle_an_include_keeps_transitive() {
        let mut unread = locked("h");
        unread.requires = vec![crate::oci::bundle::BundleMember {
            kind: ArtifactKind::Skill,
            name: "gone".to_string(),
            id: "localhost:5000/acme/gone:1".to_string(),
        }];
        let mut prev = lock_of(vec![
            unread,
            member_of("a1", &[(INNER, "1")]),
            member_of("b1", &[(OUTER, "1")]),
        ]);
        prev.bundles = vec![
            row("inner", "1", &[skill("a1")]),
            row("outer", "1", &[skill("b1"), include("inner", "1")]),
        ];
        let h = [("h", "localhost:5000/h:1")];
        let outer = ("outer", "localhost:5000/acme/outer:1");
        let before = set_of(&h, &[("inner", "localhost:5000/acme/inner:1"), outer]);
        stamp(&mut prev, &before);
        let after_set = set_of(&h, &[outer]);
        assert!(crate::lock::effective_set::effective_set(&after_set, &prev).is_none());

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "inner", &before, &after_set);

        assert_eq!(bundle_names(&out.lock), ["inner", "outer"]);
        assert!(out.lock.bundles[0].transitive, "only an edge reaches it now");
    }

    // ── A kept pin answers an edge that survives ─────────────────────────

    const M1: &str = "localhost:5000/acme/m:1";
    const M2: &str = "localhost:5000/acme/m:2";
    const M3: &str = "localhost:5000/acme/m:3";
    /// What every pin in this section holds.
    const PIN_DIGEST: char = 'c';

    fn pin_digest() -> Digest {
        Digest::Sha256(PIN_DIGEST.to_string().repeat(64))
    }

    fn pinned_at(reference: &str) -> PinnedIdentifier {
        let id = Identifier::parse(reference).unwrap().clone_with_digest(pin_digest());
        PinnedIdentifier::try_from(id).unwrap()
    }

    /// The dependency `m`, pinned at `reference`.
    fn m_at(reference: &str) -> LockedArtifact {
        LockedArtifact::direct("m".to_string(), ArtifactKind::Skill, pinned_at(reference))
    }

    fn m_edge(id: &str) -> crate::oci::bundle::BundleMember {
        crate::oci::bundle::BundleMember {
            kind: ArtifactKind::Skill,
            name: "m".to_string(),
            id: id.to_string(),
        }
    }

    /// A declared holder of `m`, asking for it at `id`.
    fn holder(name: &str, id: &str) -> LockedArtifact {
        LockedArtifact {
            requires: vec![m_edge(id)],
            ..locked(name)
        }
    }

    fn holders_set(names: &[&str]) -> DesiredSet {
        let ids: Vec<(String, String)> = names
            .iter()
            .map(|n| ((*n).to_string(), format!("localhost:5000/{n}:1")))
            .collect();
        let pairs: Vec<(&str, &str)> = ids.iter().map(|(n, i)| (n.as_str(), i.as_str())).collect();
        set_of(&pairs, &[])
    }

    /// `holders` declared, each asking for `m` at its id, and `m` pinned at
    /// `pin`. The lock reads fresh.
    fn diamond(holders: &[(&str, &str)], pin: &str) -> (GrimoireLock, DesiredSet) {
        let mut skills: Vec<LockedArtifact> = holders.iter().map(|(n, id)| holder(n, id)).collect();
        skills.push(m_at(pin));
        let names: Vec<&str> = holders.iter().map(|(n, _)| *n).collect();
        let set = holders_set(&names);
        let mut lock = lock_of(skills);
        stamp(&mut lock, &set);
        (lock, set)
    }

    fn pin_of<'a>(lock: &'a GrimoireLock, name: &str) -> &'a PinnedIdentifier {
        lock.skills
            .iter()
            .find(|s| s.name == name)
            .and_then(|s| s.source.pinned())
            .unwrap_or_else(|| panic!("{name} holds a registry pin"))
    }

    #[test]
    fn removing_the_holder_whose_tag_won_keeps_the_pin_and_its_tag() {
        let (prev, before) = diamond(&[("h1", M1), ("h2", M2)], M1);
        let after_set = holders_set(&["h2"]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "h1", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M1), "no writer here resolved a tag");
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    #[test]
    fn removing_a_holder_whose_tag_lost_rewrites_nothing() {
        let (prev, before) = diamond(&[("h1", M1), ("h2", M2)], M1);
        let after_set = holders_set(&["h1"]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "h2", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M1));
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    #[test]
    fn of_several_survivors_none_relabels_the_pin() {
        let (prev, before) = diamond(&[("h1", M1), ("h2", M3), ("h3", M2)], M1);
        let after_set = holders_set(&["h2", "h3"]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "h1", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M1));
    }

    #[test]
    fn a_relative_survivor_relabels_nothing() {
        let (mut prev, before) = diamond(&[("h1", M1)], M1);
        let h2 = LockedArtifact {
            source: LockedSource::Registry(pinned_at("localhost:5000/acme/h2:1")),
            requires: vec![m_edge("./m:2")],
            ..locked("h2")
        };
        prev.skills.insert(1, h2);
        let before = {
            let mut set = before;
            set.skills.insert(
                "h2".to_string(),
                crate::config::declaration::DeclaredSource::Registry(
                    Identifier::parse("localhost:5000/acme/h2:1").unwrap(),
                ),
            );
            set.invalidate_declaration_hash_cache();
            set
        };
        stamp(&mut prev, &before);
        let mut after_set = before.clone();
        after_set.skills.remove("h1");
        after_set.invalidate_declaration_hash_cache();

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "h1", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M1));
    }

    #[test]
    fn removing_one_of_two_flat_bundles_keeps_their_shared_member_and_its_tag() {
        let mut m = m_at(M1);
        m.bundles = vec![
            crate::lock::locked_artifact::BundleProvenance::new("localhost:5000/acme/a", "1"),
            crate::lock::locked_artifact::BundleProvenance::new("localhost:5000/acme/b", "1"),
        ];
        let mut prev = lock_of(vec![m]);
        prev.bundles = vec![
            snapshot("a", "localhost:5000/acme/a", "1", &[("m", M1)]),
            snapshot("b", "localhost:5000/acme/b", "1", &[("m", M2)]),
        ];
        let before = set_of(
            &[],
            &[("a", "localhost:5000/acme/a:1"), ("b", "localhost:5000/acme/b:1")],
        );
        stamp(&mut prev, &before);
        assert!(!has_dependency_data(&prev), "the flat path");
        let after_set = set_of(&[], &[("b", "localhost:5000/acme/b:1")]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "a", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M1));
        assert_eq!(
            out.lock.skills[0].bundles,
            [crate::lock::locked_artifact::BundleProvenance::new(
                "localhost:5000/acme/b",
                "1"
            )]
        );
    }

    const SHARED: &str = "localhost:5000/acme/shared";

    fn shared_edge(id: &str) -> (ArtifactKind, String, String) {
        (ArtifactKind::Bundle, "shared".to_string(), id.to_string())
    }

    /// `p` and `q` nest the transitive bundle `shared` at two tags of one
    /// digest, and `p`'s tag won. `other` is the row's member.
    fn nested_diamond(q_asks: &str) -> (GrimoireLock, DesiredSet, DesiredSet) {
        let mut prev = lock_of(vec![member_of("gamma", &[(SHARED, "1")])]);
        prev.bundles = vec![
            row("p", "1", &[shared_edge(&format!("{SHARED}:1"))]),
            row("q", "1", &[shared_edge(q_asks)]),
            LockedBundle {
                source: LockedBundleSource::Registry {
                    repo: SHARED.to_string(),
                    tag: "1".to_string(),
                    pinned: pinned_at(&format!("{SHARED}:1")),
                },
                ..nested("shared", "1", &[skill("gamma")])
            },
        ];
        let p = ("p", "localhost:5000/acme/p:1");
        let q = ("q", "localhost:5000/acme/q:1");
        let before = set_of(&[], &[p, q]);
        stamp(&mut prev, &before);
        (prev, before, set_of(&[], &[q]))
    }

    fn shared_row(lock: &GrimoireLock) -> &LockedBundle {
        lock.bundles.iter().find(|b| b.name == "shared").expect("the row stays")
    }

    #[test]
    fn a_transitive_bundle_row_keeps_its_tag_and_its_members_stamp() {
        let (prev, before, after_set) = nested_diamond(&format!("{SHARED}:2"));

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "p", &before, &after_set);

        let row = shared_row(&out.lock);
        assert_eq!(row.tag(), Some("1"));
        assert_eq!(row.repo(), Some(SHARED));
        assert_eq!(row.pinned(), Some(&pinned_at(&format!("{SHARED}:1"))));
        let pair = row.provenance_pair();
        assert_eq!(
            out.lock.skills[0].bundles,
            [crate::lock::locked_artifact::BundleProvenance::new(pair.0, pair.1)],
            "a member's stamp is the row's pair"
        );
        let contributors = match crate::lock::effective_set::effective_set(&after_set, &out.lock)
            .unwrap()
            .remove(&(ArtifactKind::Skill, "gamma".to_string()))
        {
            Some(crate::lock::effective_set::Origin::Implied { contributors, .. }) => contributors,
            other => panic!("{other:?}"),
        };
        assert_eq!(contributors, out.lock.skills[0].bundles, "the offline walk agrees");
        let reachable = crate::lock::closure::reachable_lock(&out.lock, &(ArtifactKind::Bundle, "shared".to_string()));
        assert_eq!(
            skill_names(&reachable.unwrap()),
            ["gamma"],
            "the stamp still selects it"
        );
    }

    #[test]
    fn a_digest_only_survivor_leaves_the_row_its_tag() {
        let digest_only = format!("{SHARED}@{}", pin_digest());
        let (prev, before, after_set) = nested_diamond(&digest_only);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "p", &before, &after_set);

        let row = shared_row(&out.lock);
        assert_eq!(row.tag(), Some("1"));
        assert_eq!(row.pinned(), Some(&pinned_at(&format!("{SHARED}:1"))));
        assert_eq!(out.lock.skills[0].bundles[0].tag, "1");
    }

    #[test]
    fn a_bare_survivor_leaves_the_row_its_tag() {
        let (prev, before, after_set) = nested_diamond(SHARED);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "p", &before, &after_set);

        assert_eq!(shared_row(&out.lock).tag(), Some("1"));
        assert_eq!(out.lock.skills[0].bundles[0].tag, "1");
    }

    fn twin_edge(tag: &str) -> crate::oci::bundle::BundleMember {
        crate::oci::bundle::BundleMember {
            kind: ArtifactKind::Bundle,
            name: "twin".to_string(),
            id: format!("{SHARED}:{tag}"),
        }
    }

    fn stamp_tags(lock: &GrimoireLock) -> Vec<&str> {
        lock.skills[0].bundles.iter().map(|b| b.tag.as_str()).collect()
    }

    #[test]
    fn a_stamp_two_rows_share_stays_one_stamp() {
        let (mut prev, before, after_set) = nested_diamond(&format!("{SHARED}:0"));
        // A second binding of the same reference, which `q` holds at `:1`.
        prev.bundles.push(LockedBundle {
            name: "twin".to_string(),
            ..shared_row(&prev).clone()
        });
        prev.bundles[1].members.push(twin_edge("1"));
        stamp(&mut prev, &before);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "p", &before, &after_set);

        assert_eq!(stamp_tags(&out.lock), ["1"]);
    }

    #[test]
    fn two_rows_of_one_pair_keep_their_stamp_whatever_survives() {
        for twin_tag in ["3", "2"] {
            let (mut prev, before, after_set) = nested_diamond(&format!("{SHARED}:2"));
            prev.bundles.push(LockedBundle {
                name: "twin".to_string(),
                ..shared_row(&prev).clone()
            });
            prev.bundles[0].members.push(twin_edge("1"));
            prev.bundles[1].members.push(twin_edge(twin_tag));
            stamp(&mut prev, &before);

            let out = drop_from_lock(&prev, ArtifactKind::Bundle, "p", &before, &after_set);

            assert_eq!(stamp_tags(&out.lock), ["1"], "against {twin_tag}");
        }
    }

    #[test]
    fn a_bundle_row_keeps_its_repository_and_its_stamp() {
        const ELSEWHERE: &str = "localhost:5000/other/shared";
        let (prev, before, after_set) = nested_diamond(&format!("{ELSEWHERE}:2"));

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "p", &before, &after_set);

        let row = shared_row(&out.lock);
        assert_eq!(row.repo(), Some(SHARED));
        assert_eq!(row.pinned(), Some(&pinned_at(&format!("{SHARED}:1"))));
        assert_eq!(
            out.lock.skills[0].bundles,
            [crate::lock::locked_artifact::BundleProvenance::new(SHARED, "1")]
        );
    }

    #[test]
    fn a_row_keeps_the_bare_request_its_tag_records() {
        let (mut prev, before, after_set) = nested_diamond(&format!("{SHARED}:2"));
        // As a load hands it over: `pinned` takes the row's `latest` back, so
        // only the row, not the pin, shows it answered a bare request.
        let latest = LockedBundle {
            source: LockedBundleSource::Registry {
                repo: SHARED.to_string(),
                tag: "latest".to_string(),
                pinned: pinned_at(&format!("{SHARED}:latest")),
            },
            ..shared_row(&prev).clone()
        };
        prev.bundles[2] = latest;
        prev.bundles[0].members = vec![crate::oci::bundle::BundleMember {
            kind: ArtifactKind::Bundle,
            name: "shared".to_string(),
            id: SHARED.to_string(),
        }];
        prev.skills[0].bundles = vec![crate::lock::locked_artifact::BundleProvenance::new(SHARED, "latest")];
        stamp(&mut prev, &before);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "p", &before, &after_set);

        assert_eq!(shared_row(&out.lock).tag(), Some("latest"));
        assert_eq!(out.lock.skills[0].bundles[0].tag, "latest");
    }

    #[test]
    fn a_digest_bearing_survivor_leaves_the_pin_its_tag() {
        let frozen = format!("localhost:5000/acme/m@{}", pin_digest());
        let (prev, before) = diamond(&[("h1", M1), ("h2", &frozen)], M1);
        let after_set = holders_set(&["h2"]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "h1", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M1));
    }

    #[test]
    fn a_tagless_pin_stays_tagless_past_a_remove() {
        let frozen = format!("localhost:5000/acme/m@{}", pin_digest());
        let (prev, before) = diamond(&[("h1", &frozen), ("h2", M2)], &frozen);
        let after_set = holders_set(&["h2"]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "h1", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(&frozen), "only a stamp tags a pin");
    }

    #[test]
    fn a_survivor_only_in_another_repository_keeps_the_pin_and_the_proof() {
        const ELSEWHERE: &str = "localhost:5000/other/m:2";
        let (prev, before) = diamond(&[("h1", M1), ("h2", ELSEWHERE)], M1);
        let after_set = holders_set(&["h2"]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "h1", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M1));
        assert_eq!(
            out.lock.metadata.declaration_hash,
            after_set.declaration_hash_cached(),
            "a stale hash would let the next relock move the pin the holder's edge named"
        );
    }

    #[test]
    fn a_survivor_in_another_repository_relabels_nothing() {
        const SMALLER_REPOSITORY: &str = "localhost:5000/aaa/m:2";
        let (prev, before) = diamond(&[("h1", M1), ("h2", SMALLER_REPOSITORY), ("h3", M3)], M1);
        let after_set = holders_set(&["h2", "h3"]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "h1", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M1));
    }

    #[test]
    fn a_pin_a_survivor_answers_stays_beside_a_smaller_one() {
        let (prev, before) = diamond(&[("h1", M1), ("h2", M3), ("h3", M2)], M3);
        let after_set = holders_set(&["h2", "h3"]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "h1", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M3), "it still answers an edge");
    }

    #[test]
    fn a_pin_no_recorded_edge_names_stays() {
        let (prev, before) = diamond(&[("h1", M1), ("h2", M1)], M2);
        let after_set = holders_set(&["h2"]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "h1", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M2), "the edit writes no pair");
    }

    #[test]
    fn a_tagless_pin_stays_for_the_proof_to_judge() {
        const TAGLESS: &str = "localhost:5000/acme/m";
        let (prev, before) = diamond(&[("h1", M1), ("h2", M2)], TAGLESS);
        let after_set = holders_set(&["h2"]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "h1", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(TAGLESS));
    }

    /// `holders` and `m` declared at `m`, over the rows of `diamond`.
    fn declaring_m(holders: &[&str], m: &str) -> DesiredSet {
        let mut set = holders_set(holders);
        set.skills.insert(
            "m".to_string(),
            crate::config::declaration::DeclaredSource::Registry(Identifier::parse(m).unwrap()),
        );
        set.invalidate_declaration_hash_cache();
        set
    }

    /// On a stale lock nothing shows that a declaration resolved, so a tagless pin's
    /// declaration vouches for no edge once it leaves.
    #[test]
    fn a_stale_declaration_vouches_for_no_tagless_pin() {
        const TAGLESS: &str = "localhost:5000/acme/m";
        let (prev, _) = diamond(&[("h1", M1)], TAGLESS);
        let prev = drifted(prev);
        let before = declaring_m(&["h1"], M1);
        let after_set = holders_set(&["h1"]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(skill_names(&out.lock), ["h1"], "the pin cannot show it answered m:1");
    }

    /// An edge nobody can read hides what its parent asks for, so the pin answers
    /// the readable ones at most: its row stays, and the lock goes stale.
    #[test]
    fn an_unreadable_edge_keeps_a_row_whose_pin_answers_the_readable_one() {
        let (mut prev, _) = diamond(&[("h1", "not a reference"), ("h2", M1)], M1);
        let before = declaring_m(&["h1", "h2"], M1);
        stamp(&mut prev, &before);
        let after_set = holders_set(&["h1", "h2"]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M1), "{:?}", out.notes);
        assert_ne!(
            out.lock.metadata.declaration_hash,
            after_set.declaration_hash_cached(),
            "no pin answers an edge nobody can read"
        );
    }

    #[test]
    fn a_survivor_naming_another_digest_leaves_the_pin() {
        let other = format!("{M2}@{}", Digest::Sha256("e".repeat(64)));
        let (prev, before) = diamond(&[("h1", M1), ("h2", &other)], M1);
        let after_set = holders_set(&["h2"]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "h1", &before, &after_set);

        assert_eq!(
            pin_of(&out.lock, "m"),
            &pinned_at(M1),
            "it could only claim a false pair"
        );
    }

    #[test]
    fn a_key_still_declared_keeps_its_pin() {
        let (mut prev, _) = diamond(&[("h1", M1), ("h2", M2)], M1);
        let before = {
            let mut set = holders_set(&["h1", "h2"]);
            set.skills.insert(
                "m".to_string(),
                crate::config::declaration::DeclaredSource::Registry(Identifier::parse(M1).unwrap()),
            );
            set.invalidate_declaration_hash_cache();
            set
        };
        stamp(&mut prev, &before);
        let mut after_set = before.clone();
        after_set.skills.remove("h1");
        after_set.invalidate_declaration_hash_cache();

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "h1", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M1), "the declaration answers it");
    }

    #[test]
    fn a_key_the_lock_lists_twice_keeps_both_rows() {
        let (mut prev, before) = diamond(&[("h1", M1), ("h2", M2)], M1);
        prev.skills.push(m_at(M1));
        let after_set = holders_set(&["h2"]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "h1", &before, &after_set);

        let pins: Vec<&PinnedIdentifier> = out
            .lock
            .skills
            .iter()
            .filter(|s| s.name == "m")
            .filter_map(|s| s.source.pinned())
            .collect();
        assert_eq!(pins, [&pinned_at(M1), &pinned_at(M1)]);
    }

    #[test]
    fn the_target_only_path_keeps_the_pin_and_its_tag() {
        // `gone` has no row, so the closure cannot be followed offline.
        let (mut prev, before) = diamond(&[("h1", M1), ("h2", M2)], M1);
        prev.skills[1].requires.push(crate::oci::bundle::BundleMember {
            kind: ArtifactKind::Skill,
            name: "gone".to_string(),
            id: "localhost:5000/acme/gone:1".to_string(),
        });
        assert!(crate::lock::effective_set::effective_set(&before, &prev).is_none());
        let after_set = holders_set(&["h2"]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "h1", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M1));
        assert_eq!(out.lock.metadata.declaration_hash, edited_from(&prev));
    }

    /// `set` and bundle `pack`, which the lock holds no member list for: the closure
    /// cannot be followed offline, and with [`StoredMembers::holding`] `pack` and no
    /// key, nothing the edit cannot read may need a row, so the target can leave.
    fn beside_a_pack_that_provides_nothing(set: &DesiredSet) -> DesiredSet {
        let mut set = set.clone();
        let pack = Identifier::parse(PACK_1).unwrap();
        set.bundles.insert(
            "pack".to_string(),
            crate::config::declaration::DeclaredSource::Registry(pack),
        );
        set.invalidate_declaration_hash_cache();
        set
    }

    /// The target-only note names the rows the edit could not judge, in words that
    /// agree with their number, and names the command: `remove` never resolves, so
    /// "offline" would explain nothing.
    #[test]
    fn the_target_only_note_names_what_it_kept() {
        let (mut prev, before) = diamond(&[("h1", M1), ("h2", M2)], M1);
        let before = beside_a_pack_that_provides_nothing(&before);
        stamp(&mut prev, &before);
        assert!(crate::lock::effective_set::effective_set(&before, &prev).is_none());
        let after_set = beside_a_pack_that_provides_nothing(&holders_set(&["h2"]));
        let stored = StoredMembers::holding("pack", &[]);

        let one = drop_beside(&prev, ArtifactKind::Skill, "h1", &before, &after_set, &stored);
        prev.skills.push(m_named("z", M3));
        let two = drop_beside(&prev, ArtifactKind::Skill, "h1", &before, &after_set, &stored);

        let one_note = "skill 'm' keeps its pin: this lock cannot show its dependencies; run `grim lock`";
        let two_note =
            "skill 'm' and skill 'z' keep their pins: this lock cannot show their dependencies; run `grim lock`";
        assert_eq!(one.notes, [one_note]);
        assert_eq!(two.notes, [two_note]);
    }

    #[test]
    fn the_legacy_path_keeps_the_pin_and_its_tag() {
        let stamp_of = |repo: &str| crate::lock::locked_artifact::BundleProvenance::new(repo, "1");
        let (a, b) = ("localhost:5000/acme/a", "localhost:5000/acme/b");
        let mut m = m_at(M1);
        m.bundles = vec![stamp_of(a), stamp_of(b)];
        let mut prev = lock_of(vec![m]);
        prev.bundles = vec![snapshot("a", a, "1", &[("m", M1)]), snapshot("b", b, "1", &[("m", M2)])];
        // `c` has no snapshot, which degrades the whole call to the legacy path.
        let declared = [
            ("a", "localhost:5000/acme/a:1"),
            ("b", "localhost:5000/acme/b:1"),
            ("c", "localhost:5000/acme/c:1"),
        ];
        let before = set_of(&[], &declared);
        stamp(&mut prev, &before);
        assert!(crate::lock::effective_set::effective_set(&before, &prev).is_none());
        assert!(!has_dependency_data(&prev));
        let after_set = set_of(&[], &declared[1..]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "a", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M1));
        assert_eq!(out.lock.skills[0].bundles, [stamp_of(b)]);
    }

    /// `pack` as a declaration names it; `PACK` is its repository.
    const PACK_1: &str = "localhost:5000/acme/pack:1";

    /// The removed declaration `m`, beside `pack`, which has no member list in a
    /// lock that was fresh, and `others` declared and locked too.
    fn m_beside_a_pack_with_no_member_list(
        mut rows: Vec<LockedArtifact>,
        others: &[(&str, &str)],
    ) -> (GrimoireLock, DesiredSet, DesiredSet) {
        rows.insert(0, row_at("m", M1));
        let mut declared = vec![("m", M1)];
        declared.extend_from_slice(others);
        let mut prev = lock_of(rows);
        let before = set_of(&declared, &[("pack", PACK_1)]);
        stamp(&mut prev, &before);
        let after_set = set_of(&declared[1..], &[("pack", PACK_1)]);
        assert!(crate::lock::effective_set::effective_set(&after_set, &prev).is_none());
        (prev, before, after_set)
    }

    /// The target-only path never restamps, so keeping the row a bundle with no
    /// member list may provide costs no proof, and the relock reads the bundle.
    #[test]
    fn the_target_only_path_keeps_a_row_a_bundle_with_no_member_list_may_provide() {
        let (prev, before, after_set) =
            m_beside_a_pack_with_no_member_list(vec![holder("h", M2)], &[("h", "localhost:5000/h:1")]);
        assert!(has_dependency_data(&prev));

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert!(skill_names(&out.lock).contains(&"m"), "the row stays");
        assert_ne!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert!(out.notes.iter().any(|note| note.contains("pack")), "{:?}", out.notes);
    }

    /// A relock writes no bundle stamp on a declared row, so with a cold store nothing
    /// offline shows whether `pack` provides `m`. On a lock 0.15.1 reads that proved
    /// its declaration the edit is 0.15.1's: the row drops and the hash restamps, so
    /// `grim install` exits 0, and a member's pin can move at the next `grim lock`.
    /// A note names that cost and the bundle, which no exit code shows.
    #[test]
    fn a_fresh_lock_drops_a_direct_row_nothing_offline_shows_a_bundle_provides() {
        let y = ("y", "localhost:5000/acme/y:1");
        let (prev, before, after_set) = m_beside_a_pack_with_no_member_list(vec![row_at(y.0, y.1)], &[y]);
        assert!(!has_dependency_data(&prev));

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(skill_names(&out.lock), ["y"]);
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert_eq!(
            out.notes,
            [
                "skill 'm' leaves the lock: bundle 'pack' has no member list in this lock, and the local \
              store cannot show whether it provides it. If it does, the next `grim lock` resolves it afresh"
            ]
        );
    }

    /// Either bundle with no member list may provide `m`, and the store cannot speak
    /// for either, so the note names both: naming one hides another that may.
    #[test]
    fn a_row_that_leaves_beside_two_unread_bundles_names_both() {
        let pk2 = ("pk2", "localhost:5000/acme/pk2:1");
        let mut prev = lock_of(vec![row_at("m", M1)]);
        let before = set_of(&[("m", M1)], &[("pack", PACK_1), pk2]);
        let after_set = set_of(&[], &[("pack", PACK_1), pk2]);
        stamp(&mut prev, &before);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert!(skill_names(&out.lock).is_empty());
        assert_eq!(
            out.notes,
            [
                "skill 'm' leaves the lock: bundle 'pack' has no member list in this lock, and the local \
                 store cannot show whether it provides it. The same holds for bundle 'pk2'. If one \
                 does, the next `grim lock` resolves it afresh"
            ]
        );
    }

    /// A stamp on another row shows what the bundle provides, not what it leaves out,
    /// so the unstamped row drops as 0.15.1's edit drops it.
    #[test]
    fn a_fresh_lock_drops_an_unstamped_row_beside_a_member_the_bundle_stamped() {
        let x = member_of("x", &[(PACK, "1")]);
        let (prev, before, after_set) = m_beside_a_pack_with_no_member_list(vec![x], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(skill_names(&out.lock), ["x"]);
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
    }

    /// A row stamped by a bundle with no member list is one that bundle provided,
    /// so it stays under that stamp, and the lock stays proven: `grim install`
    /// keeps 0.15.1's exit 0, and `grim lock` reads the bundle and keeps the pin.
    #[test]
    fn a_row_a_bundle_with_no_member_list_stamped_stays_in_a_proven_lock() {
        let (mut prev, before, after_set) = m_beside_a_pack_with_no_member_list(vec![], &[]);
        let stamp_of_pack = crate::lock::locked_artifact::BundleProvenance::new(PACK, "1");
        prev.skills[0].bundles = vec![stamp_of_pack.clone()];

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(skill_names(&out.lock), ["m"]);
        assert_eq!(out.lock.skills[0].bundles, [stamp_of_pack]);
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    /// A stale lock holds no proof a kept row could cost, and its rows answer an
    /// older declaration, so the row a bundle with no member list may provide stays.
    #[test]
    fn a_stale_lock_keeps_a_direct_row_a_bundle_with_no_member_list_may_provide() {
        let (prev, before, after_set) = m_beside_a_pack_with_no_member_list(vec![], &[]);
        let prev = drifted(prev);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(skill_names(&out.lock), ["m"]);
        assert_eq!(out.lock.metadata.declaration_hash, prev.metadata.declaration_hash);
    }

    /// A bundle a kept row's edge names, with no row in the lock, is read only by the
    /// next relock, so nothing offline shows it does not provide the removed key.
    /// Dropped, the row's pin moves at that relock though the bundle still asks for it.
    #[test]
    fn a_row_a_bundle_only_an_edge_reaches_may_provide_stays() {
        let mut p = locked("p");
        p.requires = vec![crate::oci::bundle::BundleMember {
            kind: ArtifactKind::Bundle,
            name: "pack".to_string(),
            id: PACK_1.to_string(),
        }];
        let m = LockedArtifact {
            bundles: vec![crate::lock::locked_artifact::BundleProvenance::new(PACK, "2")],
            ..row_at("m", M1)
        };
        let p_line = ("p", "localhost:5000/p:1");
        let before = set_of(&[("m", M1), p_line], &[]);
        let after_set = set_of(&[p_line], &[]);
        let prev = drifted(lock_of(vec![m, p]));
        assert!(crate::lock::effective_set::effective_set(&after_set, &prev).is_none());

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(skill_names(&out.lock), ["m", "p"]);
        assert_eq!(out.lock.metadata.declaration_hash, prev.metadata.declaration_hash);
        assert!(
            out.notes.iter().any(|note| note.contains("bundle 'pack'")),
            "{:?}",
            out.notes
        );
    }

    /// A declaration the lock holds no row for is read only by the next relock, so
    /// nothing offline shows it does not require the removed key. Dropped, the row's
    /// pin moves at that relock though that declaration asks for it.
    #[test]
    fn a_row_a_declaration_with_no_lock_row_may_require_stays() {
        let lines = [("p", "localhost:5000/p:1"), ("q", "localhost:5000/q:1")];
        let before = set_of(&[("m", M1), lines[0], lines[1]], &[]);
        let after_set = set_of(&lines, &[]);
        let prev = drifted(lock_of(vec![row_at("m", M2), holder("p", M1)]));
        assert!(crate::lock::effective_set::effective_set(&after_set, &prev).is_none());

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(skill_names(&out.lock), ["m", "p"]);
        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M2));
        assert_eq!(out.lock.metadata.declaration_hash, prev.metadata.declaration_hash);
        assert!(
            out.notes.iter().any(|note| note.contains("skill 'q'")),
            "{:?}",
            out.notes
        );
    }

    /// No lock 0.15.1 writes holds dependency data, and its remove restamped a stale
    /// lock, so its hash can prove a declaration with no row. That key reads as a leaf,
    /// as the effective set reads it there, and the edit stays 0.15.1's: the row the
    /// stored list rules out leaves, and the lock restamps.
    #[test]
    fn a_lock_with_no_dependency_data_reads_a_declaration_with_no_row_as_a_leaf() {
        let (prev, before, after_set) = m_beside_a_pack_with_no_member_list(vec![], &[("q", "localhost:5000/q:1")]);
        assert!(!has_dependency_data(&prev));
        let stored = StoredMembers::holding("pack", &[]);

        let out = drop_beside(&prev, ArtifactKind::Skill, "m", &before, &after_set, &stored);

        assert!(skill_names(&out.lock).is_empty());
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    /// A lock with no dependency data and its hash proves a declaration `q` has no row
    /// in: only 0.15.1's remove restamps such a lock. Kept, `m` would turn its
    /// `grim install` from 0 to 65, so `q` reads as a leaf and the edit is 0.15.1's.
    #[test]
    fn a_proven_lock_with_no_dependency_data_reads_a_declaration_with_no_row_as_a_leaf() {
        let q_line = ("q", "localhost:5000/q:1");
        let before = set_of(&[("m", M1), q_line], &[]);
        let after_set = set_of(&[q_line], &[]);
        let mut prev = lock_of(vec![row_at("m", M1)]);
        stamp(&mut prev, &before);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert!(skill_names(&out.lock).is_empty());
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    /// A line typed since the last `grim lock` has edges only the next relock reads,
    /// on a lock with no dependency data too. Dropped, `m` resolves afresh there and
    /// its pin moves; kept, the lock stays stale, as `grim install` already found it.
    #[test]
    fn a_stale_lock_with_no_dependency_data_keeps_a_row_a_typed_line_may_require() {
        let p_line = ("p", "localhost:5000/p:1");
        let mut prev = lock_of(vec![row_at("m", M1)]);
        stamp(&mut prev, &set_of(&[("m", M1)], &[]));
        let before = set_of(&[("m", M1), p_line], &[]);
        let after_set = set_of(&[p_line], &[]);
        assert!(!has_dependency_data(&prev));
        assert!(crate::lock::effective_set::effective_set(&after_set, &prev).is_some());

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(skill_names(&out.lock), ["m"]);
        assert_eq!(out.lock.metadata.declaration_hash, prev.metadata.declaration_hash);
        assert!(
            out.notes.iter().any(|note| note.contains("skill 'p'")),
            "{:?}",
            out.notes
        );
    }

    /// Whether a note names `dropped` beside `asker` and the `grim lock` that keeps
    /// such pins, as an edit tells the user it took a row a dependent may require.
    fn names_unwalked(notes: &[String], dropped: &str, asker: &str) -> bool {
        notes.iter().any(|note| {
            note.starts_with(dropped)
                && note.contains(&format!("{asker} records no dependency edges"))
                && note.contains("run `grim lock` online once before such an edit")
        })
    }

    /// 0.15.1 locks a dependent with no edges, and another project's `grim add` stores
    /// its manifest and requires layer. That layer shows `p` requires something the
    /// lock does not record, and `m` still drops, as on 0.15.1, with a note naming
    /// `p` and the online `grim lock` that records its edges. Found by review: kept,
    /// the stale lock changed exits against 0.15.1.
    #[test]
    fn a_row_whose_stored_manifest_requires_more_than_its_edges_drops_with_a_note() {
        let p_line = ("p", "localhost:5000/p:1");
        let before = set_of(&[("m", M1), p_line], &[]);
        let after_set = set_of(&[p_line], &[]);
        let mut prev = lock_of(vec![row_at("m", M1), row_at("p", p_line.1)]);
        stamp(&mut prev, &before);

        let cold = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);
        let out = drop_beside(
            &prev,
            ArtifactKind::Skill,
            "m",
            &before,
            &after_set,
            &StoredMembers::requiring(&[skill_key("p")]),
        );

        assert_eq!(skill_names(&cold.lock), ["p"]);
        assert_eq!(out.lock, cold.lock, "the store changes no row");
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert!(names_unwalked(&out.notes, "skill 'm'", "skill 'p'"), "{:?}", out.notes);
        assert!(
            !names_unwalked(&cold.notes, "skill 'm'", "skill 'p'"),
            "{:?}",
            cold.notes
        );
    }

    /// As above beside `q`, whose row records its edge onto `n`, as an offline relock
    /// writes it when the store held `q`'s requires layer and not `p`'s, which another
    /// project stored since. `m` drops, and the note names `p` alone: `q`'s recorded
    /// edge is the one a relock reads.
    #[test]
    fn a_row_beside_one_that_records_its_edges_drops_with_a_note() {
        let (p_line, q_line, n_line) = (
            ("p", "localhost:5000/p:1"),
            ("q", "localhost:5000/q:1"),
            ("n", "localhost:5000/n:1"),
        );
        let before = set_of(&[("m", M1), n_line, p_line, q_line], &[]);
        let after_set = set_of(&[n_line, p_line, q_line], &[]);
        let q = LockedArtifact {
            requires: vec![crate::oci::bundle::BundleMember {
                kind: ArtifactKind::Skill,
                name: "n".to_string(),
                id: n_line.1.to_string(),
            }],
            ..row_at("q", q_line.1)
        };
        let rows = vec![row_at("m", M1), row_at("n", n_line.1), row_at("p", p_line.1), q];
        let mut prev = lock_of(rows);
        stamp(&mut prev, &before);
        assert!(has_dependency_data(&prev));

        let leaf = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);
        let out = drop_beside(
            &prev,
            ArtifactKind::Skill,
            "m",
            &before,
            &after_set,
            &StoredMembers::requiring(&[skill_key("p")]),
        );

        assert_eq!(skill_names(&leaf.lock), ["n", "p", "q"]);
        assert_eq!(out.lock, leaf.lock, "the store changes no row");
        assert!(names_unwalked(&out.notes, "skill 'm'", "skill 'p'"), "{:?}", out.notes);
        assert!(
            !out.notes.iter().any(|note| note.contains("skill 'q'")),
            "{:?}",
            out.notes
        );
    }

    /// A lock 0.15.1 wrote, as it writes one: `pack:1` lists `n`, whose row is
    /// tagless and stamped, and `q`'s row records no edge. `lines` join `q` with a
    /// row each, and `bundles` join `pack` with none. `pack` leaves with the edit.
    fn q_beside_pack(lines: &[(&str, &str)], bundles: &[(&str, &str)]) -> (GrimoireLock, DesiredSet, DesiredSet) {
        let lines = [&[("q", "localhost:5000/acme/q:1")][..], lines].concat();
        let n = LockedArtifact {
            bundles: vec![BundleProvenance::new(PACK, "1")],
            ..row_at("n", "localhost:5000/acme/n")
        };
        let rows = lines.iter().map(|(name, id)| row_at(name, id.trim_end_matches(":1")));
        let mut prev = lock_of(std::iter::once(n).chain(rows).collect());
        prev.bundles = vec![snapshot("pack", PACK, "1", &[("n", "localhost:5000/acme/n:1")])];
        let before = set_of(&lines, &[[("pack", PACK_1)].as_slice(), bundles].concat());
        let after_set = set_of(&lines, bundles);
        stamp(&mut prev, &before);
        assert!(!has_dependency_data(&prev));
        (prev, before, after_set)
    }

    /// `q`'s stored requires layer lists edges its row does not record, and `q` may ask
    /// for the bundle itself. The bundle's row and its member drop as on a store that
    /// never read `q`, as on 0.15.1, and the note names both beside `q`. Found by
    /// review: kept, the row's `transitive` made 0.15.1 refuse the lock.
    #[test]
    fn a_removed_bundle_drops_its_row_beside_a_requester_the_lock_records_no_edge_for() {
        let unread = StoredMembers::requiring(&[skill_key("q")]);
        for bundles in [&[][..], &[("bz", "localhost:5000/acme/bz:1")][..]] {
            let (prev, before, after_set) = q_beside_pack(&[], bundles);
            assert_eq!(
                crate::lock::effective_set::effective_set(&after_set, &prev).is_some(),
                bundles.is_empty(),
                "a list-less bundle takes the legacy path"
            );

            let cold = drop_from_lock(&prev, ArtifactKind::Bundle, "pack", &before, &after_set);
            let out = drop_beside(&prev, ArtifactKind::Bundle, "pack", &before, &after_set, &unread);

            assert!(bundle_names(&out.lock).is_empty(), "{bundles:?}");
            assert_eq!(skill_names(&out.lock), ["q"], "{bundles:?}");
            assert_eq!(out.lock, cold.lock, "{bundles:?}");
            assert!(
                names_unwalked(&out.notes, "skill 'n' and bundle 'pack'", "skill 'q'"),
                "{bundles:?}: {:?}",
                out.notes
            );
        }
    }

    /// The other arm of the same verdict: a store that never read `q`'s manifest
    /// shows no edge of it, so `q` is a leaf and the edit is 0.15.1's.
    #[test]
    fn a_removed_bundle_leaves_beside_a_requester_this_store_never_read() {
        for bundles in [&[][..], &[("bz", "localhost:5000/acme/bz:1")][..]] {
            let (prev, before, after_set) = q_beside_pack(&[], bundles);

            let out = drop_from_lock(&prev, ArtifactKind::Bundle, "pack", &before, &after_set);

            assert!(bundle_names(&out.lock).is_empty(), "{bundles:?}");
            assert_eq!(skill_names(&out.lock), ["q"], "{bundles:?}");
            assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
            assert!(!out.notes.iter().any(|note| note.contains("keeps")), "{:?}", out.notes);
            assert!(
                !out.notes.iter().any(|note| note.contains("no dependency edges")),
                "nothing shows q has edges: {:?}",
                out.notes
            );
        }
    }

    /// A dependent the lock never walked asks for a dropped row only where its stored
    /// requires reach it, or reach past a link the store lacks. Found by review: the
    /// note named every such dependent, and a removed bundle beside it, whatever its
    /// stored requires listed.
    #[test]
    fn a_note_names_a_dependent_only_where_its_stored_requires_may_reach_the_row() {
        let p_line = ("p", "localhost:5000/p:1");
        let before = set_of(&[("m", M1), p_line], &[]);
        let after_set = set_of(&[p_line], &[]);
        let mut prev = lock_of(vec![row_at("m", M1), row_at("p", p_line.1)]);
        stamp(&mut prev, &before);
        let edit = |stored: &StoredMembers| drop_beside(&prev, ArtifactKind::Skill, "m", &before, &after_set, stored);

        let elsewhere = edit(&StoredMembers::reaching(&skill_key("p"), &[skill_key("z")]));
        let shown = edit(&StoredMembers::reaching(&skill_key("p"), &[skill_key("m")]));
        let unread = edit(&StoredMembers::requiring(&[skill_key("p")]));

        assert!(
            !elsewhere.notes.iter().any(|note| note.contains("no dependency edges")),
            "{:?}",
            elsewhere.notes
        );
        assert!(
            names_unwalked(&shown.notes, "skill 'm'", "skill 'p'"),
            "{:?}",
            shown.notes
        );
        assert!(
            shown.notes.iter().any(|note| note.contains("edges and requires it")),
            "{:?}",
            shown.notes
        );
        assert!(
            unread
                .notes
                .iter()
                .any(|note| note.contains("edges and may require it")),
            "{:?}",
            unread.notes
        );
        for out in [&elsewhere, &shown, &unread] {
            assert_eq!(skill_names(&out.lock), ["p"], "the store changes no row");
        }
    }

    /// `q`'s stored requires reach the bundle's member and not the bundle, so the note
    /// names the member alone, with the recovery that keeps its pin after the edit:
    /// both files restored, one online `grim lock`, the edit again.
    #[test]
    fn a_removed_bundle_names_only_the_member_a_stored_requires_reaches() {
        let stored = StoredMembers::reaching(&skill_key("q"), &[skill_key("n")]);
        let (prev, before, after_set) = q_beside_pack(&[], &[]);

        let out = drop_beside(&prev, ArtifactKind::Bundle, "pack", &before, &after_set, &stored);

        assert!(bundle_names(&out.lock).is_empty());
        assert_eq!(
            out.notes,
            [
                "skill 'n' leaves the lock, though skill 'q' records no dependency edges and requires it. \
                 The next `grim lock` may move its pin. To keep it, restore grimoire.toml and \
                 grimoire.lock, run `grim lock` online once before such an edit, then repeat it"
            ]
        );
    }

    /// A store that never read `q`'s requires cannot say `q` reaches the removed
    /// bundle, so the recovery's promise to keep rows is made for `n` alone and the
    /// bundle the user removed is named apart, kept only where `q` requires it.
    #[test]
    fn a_removed_bundle_is_named_apart_from_the_rows_a_recovery_keeps() {
        let unread = StoredMembers::requiring(&[skill_key("q")]);
        let (prev, before, after_set) = q_beside_pack(&[], &[]);

        let out = drop_beside(&prev, ArtifactKind::Bundle, "pack", &before, &after_set, &unread);

        assert_eq!(
            out.notes,
            [
                "skill 'n' and bundle 'pack' leave the lock, though skill 'q' records no dependency edges \
                 and may require them. The next `grim lock` may move their pins. To keep skill 'n', restore \
                 grimoire.toml and grimoire.lock, run `grim lock` online once before such an edit, then \
                 repeat it. That keeps bundle 'pack' only if skill 'q' requires it"
            ]
        );
    }

    /// No sentence of a note with one name in each list passes 25 words, with the
    /// longest forms of the choices a note has.
    #[test]
    fn every_note_keeps_its_sentences_within_25_words() {
        let key = || (ArtifactKind::Skill, "alpha-tools".to_string());
        let pack = || BundleProvenance::new("localhost:5000/acme/pack", "1");
        let kinds = [
            NoteKind::HiddenRequest {
                key: key(),
                unanswered: Identifier::parse("localhost:5000/acme/beta-tools:1").unwrap(),
            },
            NoteKind::Unmasked {
                key: key(),
                by: Provider::Bundle(pack()),
                version: VersionClaim::Unconfirmed,
            },
            NoteKind::Unmasked {
                key: key(),
                by: Provider::RequiredBy(vec![(ArtifactKind::Agent, "beta-tools".to_string())]),
                version: VersionClaim::Unconfirmed,
            },
            NoteKind::Unsettled { key: key() },
            NoteKind::ListedMember {
                key: key(),
                bundle: "pack".to_string(),
            },
            NoteKind::MemberUnknown {
                key: key(),
                bundle: "pack".to_string(),
                afresh: true,
            },
            NoteKind::RequestUnknown {
                key: key(),
                parent: (ArtifactKind::Agent, "beta-tools".to_string()),
                afresh: true,
            },
            NoteKind::LeftUnread {
                keys: vec![key()],
                bundles: vec!["pack".to_string(), "pk2".to_string()],
            },
            NoteKind::GraphUnknown { kept: vec![key()] },
        ];
        for what in kinds {
            let note = EditNote {
                grim: Invocation::WALKED,
                what,
            }
            .to_string();
            for sentence in note.split(". ") {
                let words = sentence.split_whitespace().count();
                assert!(words <= 25, "{words} words: {sentence}");
            }
        }
    }

    /// With one or two rows on each side, no sentence of the note passes 25 words,
    /// whether the lock was stale before the edit or not, and under `--global`.
    #[test]
    fn an_unwalked_note_keeps_every_sentence_within_25_words() {
        let names = |kind: ArtifactKind, count: usize| -> Vec<NodeKey> {
            ["alpha-tools", "beta-tools"][..count]
                .iter()
                .map(|name| (kind, (*name).to_string()))
                .collect()
        };
        for (dropped, askers, shown) in [(1, 1, true), (1, 1, false), (2, 2, true), (2, 2, false), (2, 1, false)] {
            for stale in [false, true] {
                let named = Invocation::config(std::path::Path::new("/srv/team/grimoire.toml"));
                for (grim, config, lock) in [
                    (Invocation::WALKED, "grimoire.toml", "grimoire.lock"),
                    (Invocation::GLOBAL, "grimoire.toml", "grimoire.lock"),
                    (named, "/srv/team/grimoire.toml", "/srv/team/grimoire.lock"),
                ] {
                    let keys = names(ArtifactKind::Skill, dropped);
                    let note = EditNote {
                        grim: grim.clone(),
                        what: NoteKind::Unwalked {
                            removed: (dropped == 2 && !shown).then(|| keys[1].clone()),
                            keys,
                            askers: names(ArtifactKind::Agent, askers),
                            shown,
                            config: config.to_string(),
                            lock: lock.to_string(),
                            stale,
                        },
                    }
                    .to_string();

                    for sentence in note.split(". ") {
                        let words = sentence.split_whitespace().count();
                        assert!(
                            words <= 25,
                            "{words} words ({dropped}, {askers}, {shown}, {stale}, {grim}): {sentence}"
                        );
                    }
                }
            }
        }
    }

    /// Two stamps on one row show a lock records every bundle per member, but only
    /// for the bundles declared when it was written. `nb` was typed since, so its
    /// missing stamp on `m` rules nothing out, and `m` stays for the relock.
    #[test]
    fn a_bundle_line_typed_since_the_last_lock_is_not_ruled_out_by_stamps() {
        const PK2: &str = "localhost:5000/acme/pk2";
        let y = ("y", "localhost:5000/acme/y:1");
        let m = member_of("m", &[(PACK, "1")]);
        let s = member_of("s", &[(PACK, "1"), (PK2, "1")]);
        let mut prev = lock_of(vec![m, s, row_at(y.0, y.1)]);
        prev.bundles = vec![
            snapshot("pack", PACK, "1", &[("m", M1), ("s", "localhost:5000/s:1")]),
            snapshot("pk2", PK2, "1", &[("s", "localhost:5000/s:1")]),
        ];
        let locked = [("pack", PACK_1), ("pk2", "localhost:5000/acme/pk2:1")];
        stamp(&mut prev, &set_of(&[y], &locked));
        let nb = ("nb", "localhost:5000/acme/nb:1");
        let before = set_of(&[y], &[locked[0], locked[1], nb]);
        let after_set = set_of(&[y], &[locked[1], nb]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "pack", &before, &after_set);

        assert!(skill_names(&out.lock).contains(&"m"), "{:?}", skill_names(&out.lock));
        assert_ne!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert!(
            out.notes.iter().any(|note| note.contains("bundle 'nb'")),
            "{:?}",
            out.notes
        );
    }

    fn skill_key(name: &str) -> NodeKey {
        (ArtifactKind::Skill, name.to_string())
    }

    /// A stored list that lacks the key shows the bundle does not provide it, so the
    /// edit is 0.15.1's: the row leaves, the hash restamps, `grim install` exits 0.
    #[test]
    fn a_key_the_stored_list_lacks_leaves_a_proven_lock() {
        let (prev, before, after_set) = m_beside_a_pack_with_no_member_list(vec![], &[]);
        let stored = StoredMembers::holding("pack", &[skill_key("x")]);

        let out = drop_beside(&prev, ArtifactKind::Skill, "m", &before, &after_set, &stored);

        assert!(skill_names(&out.lock).is_empty());
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    /// A hook takes the same answer: kept, it stays armed past a `grim install` that
    /// exits 65, where 0.15.1's exit 0 converged and disarmed it.
    #[test]
    fn a_hook_the_stored_list_lacks_leaves_a_proven_lock() {
        const H: &str = "localhost:5000/h:1";
        let before = hook_set_of(&[("h", H)], &[("pack", PACK_1)]);
        let after_set = hook_set_of(&[], &[("pack", PACK_1)]);
        let mut prev = lock_of(vec![]);
        prev.hooks = vec![locked_hook("h", &[])];
        stamp(&mut prev, &before);
        let stored = StoredMembers::holding("pack", &[]);

        let out = drop_beside(&prev, ArtifactKind::Hook, "h", &before, &after_set, &stored);

        assert!(out.lock.hooks.is_empty());
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
    }

    /// A stored list naming the key is the case the keep exists for: dropped, the row
    /// takes its pin along and the next `grim lock` resolves the key afresh. Kept
    /// under the bundle's stamp, as a relock writes it, the lock stays proven, so
    /// `grim install` keeps 0.15.1's exit 0.
    #[test]
    fn a_key_the_stored_list_names_stays_under_the_bundle_stamp_in_a_proven_lock() {
        let (prev, before, after_set) = m_beside_a_pack_with_no_member_list(vec![], &[]);
        let stored = StoredMembers::holding("pack", &[skill_key("m")]);

        let out = drop_beside(&prev, ArtifactKind::Skill, "m", &before, &after_set, &stored);

        assert_eq!(skill_names(&out.lock), ["m"]);
        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M1));
        assert_eq!(
            out.lock.skills[0].bundles,
            [crate::lock::locked_artifact::BundleProvenance::new(PACK, "1")]
        );
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    /// On a stale lock one bundle the store cannot speak for is enough to keep the
    /// row: `grim install` exits 65 there whatever the edit keeps.
    #[test]
    fn a_key_stays_while_any_bundle_with_no_member_list_may_provide_it() {
        const OTHER: &str = "localhost:5000/acme/other-pack:1";
        let bundles = [("other", OTHER), ("pack", PACK_1)];
        let before = set_of(&[("m", M1)], &bundles);
        let after_set = set_of(&[], &bundles);
        let prev = drifted(lock_of(vec![row_at("m", M1)]));
        let stored = StoredMembers::holding("pack", &[]);

        let out = drop_beside(&prev, ArtifactKind::Skill, "m", &before, &after_set, &stored);

        assert_eq!(skill_names(&out.lock), ["m"]);
        assert!(out.notes[0].contains("bundle 'other'"), "{:?}", out.notes);
    }

    /// The store rules `bz` out, so `m` leaves with the bundle that stamped it and the
    /// hash restamps: 0.15.1's edit, on a key nothing still provides.
    #[test]
    fn a_removed_bundle_evicts_a_member_the_store_shows_no_other_bundle_provides() {
        let (prev, before, after_set) = m_shared_by_two_bundles_with_no_member_list();
        let stored = StoredMembers::holding("bz", &[]);

        let out = drop_beside(&prev, ArtifactKind::Bundle, "ba", &before, &after_set, &stored);

        assert!(skill_names(&out.lock).is_empty());
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
    }

    /// A list the lock does hold keeps its member beside a bundle it holds none for,
    /// as 0.15.1 kept it: the note names the bundle that provides the key.
    #[test]
    fn a_member_list_in_the_lock_keeps_its_key_beside_a_bundle_with_none() {
        const LISTED: &str = "localhost:5000/acme/listed";
        let bundles = [("listed", "localhost:5000/acme/listed:1"), ("pack", PACK_1)];
        let before = set_of(&[("m", M1)], &bundles);
        let after_set = set_of(&[], &bundles);
        let mut prev = lock_of(vec![row_at("m", M1)]);
        prev.bundles = vec![snapshot("listed", LISTED, "1", &[("m", M1)])];
        stamp(&mut prev, &before);
        let stored = StoredMembers::holding("pack", &[]);

        let out = drop_beside(&prev, ArtifactKind::Skill, "m", &before, &after_set, &stored);

        assert_eq!(skill_names(&out.lock), ["m"]);
        assert_ne!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert!(out.notes[0].contains("bundle 'listed'"), "{:?}", out.notes);
    }

    /// The target-only path never restamps, but the row of a key the store rules out
    /// leaves as it does everywhere else.
    #[test]
    fn the_target_only_path_drops_a_row_the_store_rules_out() {
        let (prev, before, after_set) =
            m_beside_a_pack_with_no_member_list(vec![holder("h", M2)], &[("h", "localhost:5000/h:1")]);
        let stored = StoredMembers::holding("pack", &[skill_key("h")]);

        let out = drop_beside(&prev, ArtifactKind::Skill, "m", &before, &after_set, &stored);

        assert_eq!(skill_names(&out.lock), ["h"]);
        assert_ne!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
    }

    /// A key with no row has no pin to keep, so the remove restamps as 0.15.1's did,
    /// even with nothing to show what the bundle provides.
    #[test]
    fn a_key_with_no_row_keeps_the_lock_proven_beside_a_bundle_with_no_member_list() {
        let (mut prev, before, after_set) = m_beside_a_pack_with_no_member_list(vec![], &[]);
        prev.skills.clear();

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    /// A lock-only row a command names leaves when the store shows nothing needs it,
    /// and the hash, which never named it, still proves the declaration.
    #[test]
    fn a_named_ghost_the_store_rules_out_leaves_a_proven_lock() {
        let set = set_of(&[], &[("pack", PACK_1)]);
        let mut prev = lock_of(vec![row_at("m", M1)]);
        stamp(&mut prev, &set);
        let stored = StoredMembers::holding("pack", &[]);

        let out = drop_beside(&prev, ArtifactKind::Skill, "m", &set, &set, &stored);

        assert!(skill_names(&out.lock).is_empty());
        assert_eq!(out.lock.metadata.declaration_hash, set.declaration_hash_cached());
    }

    /// `m` stamped by `ba` and shared with `bz`, on a lock with no member list for
    /// either. Grim 0.3 recorded one bundle per member, so the stamp shows only one.
    fn m_shared_by_two_bundles_with_no_member_list() -> (GrimoireLock, DesiredSet, DesiredSet) {
        const BA: &str = "localhost:5000/acme/ba:1";
        const BZ: &str = "localhost:5000/acme/bz:1";
        let mut prev = lock_of(vec![member_of("m", &[("localhost:5000/acme/ba", "1")])]);
        let before = set_of(&[], &[("ba", BA), ("bz", BZ)]);
        stamp(&mut prev, &before);
        (prev, before, set_of(&[], &[("bz", BZ)]))
    }

    /// The store shows `bz` provides `m`, so `m` stays for the relock instead of
    /// resolving afresh there. Its stamp moves from `ba` to `bz`, as a relock writes
    /// it, so the lock stays proven and `grim install` keeps 0.15.1's exit 0. The
    /// tagless pin grim 0.3 wrote keeps its digest for `grim lock` to tag.
    #[test]
    fn a_removed_bundle_restamps_a_member_the_store_shows_another_bundle_provides() {
        let (prev, before, after_set) = m_shared_by_two_bundles_with_no_member_list();
        let stored = StoredMembers::holding("bz", &[skill_key("m")]);
        assert!(prev.skills[0].source.pinned().unwrap().tag().is_none());

        let out = drop_beside(&prev, ArtifactKind::Bundle, "ba", &before, &after_set, &stored);

        assert_eq!(skill_names(&out.lock), ["m"]);
        assert_eq!(out.lock.skills[0].source, prev.skills[0].source);
        assert_eq!(
            out.lock.skills[0].bundles,
            [crate::lock::locked_artifact::BundleProvenance::new(
                "localhost:5000/acme/bz",
                "1"
            )]
        );
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    /// With a cold store nothing shows `bz` provides `m`, so `m` leaves with `ba` and
    /// the hash restamps, as on 0.15.1: `grim install` exits 0, and if `bz` does
    /// provide `m`, the next `grim lock` resolves it afresh.
    #[test]
    fn a_removed_bundle_evicts_a_member_nothing_offline_shows_another_bundle_provides() {
        let (prev, before, after_set) = m_shared_by_two_bundles_with_no_member_list();

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "ba", &before, &after_set);

        assert!(skill_names(&out.lock).is_empty());
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert_eq!(out.notes.len(), 1, "{:?}", out.notes);
        assert!(
            out.notes[0].starts_with("skill 'm' leaves the lock: bundle 'bz' has no member list"),
            "{:?}",
            out.notes
        );
    }

    /// Nothing left the declaration, so the hash still proves the lock, and a note
    /// calling it stale would send the user to a `grim lock` with nothing to do.
    #[test]
    fn a_delete_that_leaves_the_declaration_alone_calls_no_lock_stale() {
        let set = set_of(&[], &[("pack", PACK_1)]);
        let mut prev = lock_of(vec![member_of("m", &[(PACK, "1")])]);
        stamp(&mut prev, &set);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &set, &set);

        assert_eq!(out.lock, prev);
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    /// A note says "marked stale" exactly when the saved hash proves no declaration.
    #[test]
    fn a_note_calls_a_lock_stale_only_when_its_hash_proves_nothing() {
        let (prev, before, after_set) = direct_over_holders(M1, M2);
        let kept_stale = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);
        assert_ne!(
            kept_stale.lock.metadata.declaration_hash,
            after_set.declaration_hash_cached()
        );
        assert!(kept_stale.notes.iter().any(|note| note.contains("marked stale")));

        let (prev, before, after_set) = direct_over_holders(M1, M1);
        let proven = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);
        assert_eq!(
            proven.lock.metadata.declaration_hash,
            after_set.declaration_hash_cached()
        );
        assert!(proven.notes.is_empty(), "{:?}", proven.notes);
    }

    // ── What the hash says afterwards ────────────────────────────────────

    fn row_at(name: &str, reference: &str) -> LockedArtifact {
        LockedArtifact::direct(name.to_string(), ArtifactKind::Skill, pinned_at(reference))
    }

    const EDITED: &str = "localhost:5000/acme/edited:2";
    const OTHER_SKILL: &str = "localhost:5000/acme/other:1";

    fn drifted(mut lock: GrimoireLock) -> GrimoireLock {
        lock.metadata.declaration_hash = format!("sha256:{}", "f".repeat(64));
        lock
    }

    /// The hash an edit writes when it leaves `prev` unproven and changes its rows.
    fn edited_from(prev: &GrimoireLock) -> String {
        edited_hash(&prev.metadata.declaration_hash)
    }

    #[test]
    fn remove_on_a_stale_lock_that_answers_the_declaration_restamps() {
        let prev = drifted(lock_of(vec![row_at("edited", EDITED), row_at("other", OTHER_SKILL)]));
        let before = set_of(&[("edited", EDITED), ("other", OTHER_SKILL)], &[]);
        let after_set = set_of(&[("edited", EDITED)], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "other", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
    }

    /// A stamp must change nothing a relock or an install would do, so each of
    /// these keeps the hash stale: a row nothing reaches would install, and for
    /// a hook arm, on the stamp's word.
    #[test]
    fn a_ghost_row_keeps_a_stale_lock_stale() {
        let rows = vec![
            row_at("edited", EDITED),
            row_at("ghost", OTHER_SKILL),
            row_at("other", OTHER_SKILL),
        ];
        let prev = drifted(lock_of(rows));
        let before = set_of(&[("edited", EDITED), ("other", OTHER_SKILL)], &[]);
        let after_set = set_of(&[("edited", EDITED)], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "other", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, edited_from(&prev));
    }

    #[test]
    fn a_pin_no_edge_names_keeps_a_stale_lock_stale() {
        let (prev, before) = diamond(&[("h1", M2), ("h2", M2)], M1);
        let prev = drifted(prev);
        let after_set = holders_set(&["h2"]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "h1", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, edited_from(&prev));
    }

    #[test]
    fn a_tagless_pin_keeps_a_stale_lock_stale() {
        const BARE: &str = "localhost:5000/acme/bare";
        let prev = drifted(lock_of(vec![row_at("bare", BARE), row_at("other", OTHER_SKILL)]));
        let before = set_of(&[("bare", BARE), ("other", OTHER_SKILL)], &[]);
        let after_set = set_of(&[("bare", BARE)], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "other", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, edited_from(&prev));
    }

    #[test]
    fn a_declared_digest_its_row_does_not_hold_keeps_a_stale_lock_stale() {
        const PACK: &str = "localhost:5000/acme/pack";
        let mut prev = drifted(lock_of(vec![row_at("other", OTHER_SKILL)]));
        prev.bundles = vec![snapshot("pack", PACK, "1", &[])];
        let hand_pinned = format!("{PACK}:1@{}", Digest::Sha256("e".repeat(64)));
        let bundles = [("pack", hand_pinned.as_str())];
        let before = set_of(&[("other", OTHER_SKILL)], &bundles);
        let after_set = set_of(&[], &bundles);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "other", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, edited_from(&prev));
    }

    #[test]
    fn a_declared_path_its_row_does_not_hold_keeps_a_stale_lock_stale() {
        let path = |value: &str| crate::config::path_source::PathSource::parse(value).unwrap();
        let mut prev = drifted(lock_of(vec![row_at("other", OTHER_SKILL)]));
        prev.bundles = vec![LockedBundle {
            name: "local".to_string(),
            source: LockedBundleSource::Path {
                path: path("./bundles/a.toml"),
                hash: pin_digest(),
            },
            members: vec![],
            transitive: false,
        }];
        let declared = |skills: &[(&str, &str)]| {
            let mut set = set_of(skills, &[]);
            let source = crate::config::declaration::DeclaredSource::Path(path("./bundles/b.toml"));
            set.bundles.insert("local".to_string(), source);
            set.invalidate_declaration_hash_cache();
            set
        };
        let (before, after_set) = (declared(&[("other", OTHER_SKILL)]), declared(&[]));

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "other", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, edited_from(&prev));
    }

    #[test]
    fn a_reached_key_with_no_row_keeps_a_stale_lock_stale() {
        let mut prev = drifted(lock_of(vec![row_at("other", OTHER_SKILL)]));
        prev.bundles = vec![snapshot("pack", "localhost:5000/acme/pack", "1", &[("m", M1)])];
        let bundles = [("pack", "localhost:5000/acme/pack:1")];
        let before = set_of(&[("other", OTHER_SKILL)], &bundles);
        let after_set = set_of(&[], &bundles);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "other", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, edited_from(&prev));
    }

    #[test]
    fn a_declared_bundle_its_row_holds_lets_a_stale_lock_restamp() {
        let pack = "localhost:5000/acme/pack";
        for declared in [
            format!("{pack}:1"),
            format!(
                "{pack}:1@{}",
                snapshot("pack", pack, "1", &[]).pinned().unwrap().digest()
            ),
        ] {
            let mut prev = drifted(lock_of(vec![row_at("other", OTHER_SKILL)]));
            prev.bundles = vec![snapshot("pack", pack, "1", &[])];
            let bundles = [("pack", declared.as_str())];
            let before = set_of(&[("other", OTHER_SKILL)], &bundles);
            let after_set = set_of(&[], &bundles);

            let out = drop_from_lock(&prev, ArtifactKind::Skill, "other", &before, &after_set);

            assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        }
    }

    #[test]
    fn a_transitive_bundle_an_edge_names_lets_a_stale_lock_restamp() {
        const PACK: &str = "localhost:5000/acme/pack";
        const HOLDER: &str = "localhost:5000/acme/h:1";
        let mut holder = row_at("h", HOLDER);
        holder.requires = vec![crate::oci::bundle::BundleMember {
            kind: ArtifactKind::Bundle,
            name: "pack".to_string(),
            id: format!("{PACK}:1"),
        }];
        let mut prev = drifted(lock_of(vec![holder, row_at("other", OTHER_SKILL)]));
        prev.bundles = vec![LockedBundle {
            transitive: true,
            ..snapshot("pack", PACK, "1", &[])
        }];
        let before = set_of(&[("h", HOLDER), ("other", OTHER_SKILL)], &[]);
        let after_set = set_of(&[("h", HOLDER)], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "other", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
    }

    const PACK: &str = "localhost:5000/acme/pack";
    const PACK_HOLDERS: [(&str, &str); 2] = [("h1", "localhost:5000/acme/h1:1"), ("h2", "localhost:5000/acme/h2:1")];

    /// `h1` asks for bundle `pack` by an edge nobody can read, `h2` for `pack:1`.
    fn pack_holders() -> Vec<LockedArtifact> {
        let holder = |(name, reference): (&str, &str), edge: &str| {
            let mut row = row_at(name, reference);
            row.requires = vec![crate::oci::bundle::BundleMember {
                kind: ArtifactKind::Bundle,
                name: "pack".to_string(),
                id: edge.to_string(),
            }];
            row
        };
        vec![
            holder(PACK_HOLDERS[0], "not a reference"),
            holder(PACK_HOLDERS[1], &format!("{PACK}:1")),
        ]
    }

    /// A bundle with an edge nobody can read stays conflicted, so its row answers no
    /// request for a restamp, even one its pin names.
    #[test]
    fn a_bundle_with_an_unreadable_edge_keeps_a_stale_lock_stale() {
        let mut rows = pack_holders();
        rows.push(row_at("other", OTHER_SKILL));
        let mut prev = drifted(lock_of(rows));
        prev.bundles = vec![LockedBundle {
            transitive: true,
            ..snapshot("pack", PACK, "1", &[])
        }];
        let [h1, h2] = PACK_HOLDERS;
        let before = set_of(&[h1, h2, ("other", OTHER_SKILL)], &[]);
        let after_set = set_of(&PACK_HOLDERS, &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "other", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, edited_from(&prev));
    }

    /// A removed bundle whose key keeps an edge nobody can read stays, and the lock
    /// goes stale: no pin answers that edge, so a relock must settle it.
    #[test]
    fn an_unreadable_edge_keeps_a_removed_bundle_row_and_the_lock_stale() {
        let mut prev = lock_of(pack_holders());
        prev.bundles = vec![snapshot("pack", PACK, "1", &[])];
        let before = set_of(&PACK_HOLDERS, &[("pack", "localhost:5000/acme/pack:1")]);
        stamp(&mut prev, &before);
        let after_set = set_of(&PACK_HOLDERS, &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "pack", &before, &after_set);

        assert_eq!(bundle_names(&out.lock), ["pack"], "{:?}", out.notes);
        assert_ne!(
            out.lock.metadata.declaration_hash,
            after_set.declaration_hash_cached(),
            "no pin answers an edge nobody can read"
        );
        assert!(out.notes.iter().any(|note| note.contains("pack")), "{:?}", out.notes);
    }

    /// A hand typed `m` back after it fell to a bundle's member. A relock writes no
    /// bundle on a declared row, so a restamp must not leave one.
    #[test]
    fn a_declaration_typed_back_over_a_member_sheds_the_bundle() {
        const BUNDLE: &str = "localhost:5000/acme/b";
        let mut m = m_at(M1);
        m.bundles = vec![crate::lock::locked_artifact::BundleProvenance::new(BUNDLE, "1")];
        let mut prev = drifted(lock_of(vec![m]));
        prev.bundles = vec![snapshot("b", BUNDLE, "1", &[("m", M1)])];
        let skills = [("m", M1)];
        let before = set_of(&skills, &[("b", "localhost:5000/acme/b:1")]);
        let after_set = set_of(&skills, &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "b", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert_eq!(out.lock.skills[0].bundles, []);
    }

    /// The legacy path re-derives no bundle, so `m` stays unstamped where a relock
    /// would stamp it: a restamp would state a row no relock writes.
    #[test]
    fn a_row_without_the_bundle_a_relock_writes_keeps_a_stale_lock_stale() {
        const B: &str = "localhost:5000/acme/b";
        let mut prev = drifted(lock_of(vec![m_at(M1)]));
        prev.bundles = vec![snapshot("b", B, "1", &[("m", M1)])];
        let before = set_of(
            &[],
            &[("a", "localhost:5000/acme/a:1"), ("b", "localhost:5000/acme/b:1")],
        );
        let after_set = set_of(&[], &[("b", "localhost:5000/acme/b:1")]);
        assert!(crate::lock::effective_set::effective_set(&before, &prev).is_none());

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "a", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, prev.metadata.declaration_hash);
    }

    /// Rows that would earn the stamp, but for a keep-both merge's second `edited`.
    #[test]
    fn a_key_listed_twice_keeps_a_stale_lock_stale() {
        let moved = Identifier::parse(EDITED)
            .unwrap()
            .clone_with_digest(Digest::Sha256("b".repeat(64)));
        let mut merged = row_at("edited", EDITED);
        merged.source = LockedSource::Registry(PinnedIdentifier::try_from(moved).unwrap());
        let rows = vec![row_at("edited", EDITED), merged, row_at("other", OTHER_SKILL)];
        let prev = drifted(lock_of(rows));
        let before = set_of(&[("edited", EDITED), ("other", OTHER_SKILL)], &[]);
        let after_set = set_of(&[("edited", EDITED)], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "other", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, edited_from(&prev));
    }

    #[test]
    fn a_bundle_listed_twice_keeps_a_stale_lock_stale() {
        let pack = "localhost:5000/acme/pack";
        let mut prev = drifted(lock_of(vec![row_at("other", OTHER_SKILL)]));
        prev.bundles = vec![snapshot("pack", pack, "1", &[]), snapshot("pack", pack, "1", &[])];
        let bundles = [("pack", "localhost:5000/acme/pack:1")];
        let before = set_of(&[("other", OTHER_SKILL)], &bundles);
        let after_set = set_of(&[], &bundles);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "other", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, edited_from(&prev));
    }

    /// A hand pinned `edited:2` at the digest the row holds under `edited:1`. A digest
    /// names itself, but a relock writes the declaration's own reference, and a
    /// restamp must state the same row. Fuzz seed 84032.
    #[test]
    fn a_declared_digest_under_another_tag_keeps_a_stale_lock_stale() {
        let declared = format!("{EDITED}@{}", pin_digest());
        let rows = vec![
            row_at("edited", "localhost:5000/acme/edited:1"),
            row_at("other", OTHER_SKILL),
        ];
        let prev = drifted(lock_of(rows));
        let before = set_of(&[("edited", &declared), ("other", OTHER_SKILL)], &[]);
        let after_set = set_of(&[("edited", &declared)], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "other", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, edited_from(&prev));
    }

    #[test]
    fn a_declared_digest_under_its_own_tag_lets_a_stale_lock_restamp() {
        let declared = format!("{EDITED}@{}", pin_digest());
        let prev = drifted(lock_of(vec![row_at("edited", EDITED), row_at("other", OTHER_SKILL)]));
        let before = set_of(&[("edited", &declared), ("other", OTHER_SKILL)], &[]);
        let after_set = set_of(&[("edited", &declared)], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "other", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
    }

    /// `prev`'s hash is `after_set`'s, from before an edit took the row of `other`
    /// and a hand then put `third` in the declaration.
    fn landing(rows: Vec<LockedArtifact>) -> (GrimoireLock, DesiredSet, DesiredSet) {
        const THIRD: &str = "localhost:5000/acme/third:1";
        let after_set = set_of(&[("edited", EDITED), ("other", OTHER_SKILL)], &[]);
        let before = set_of(&[("edited", EDITED), ("other", OTHER_SKILL), ("third", THIRD)], &[]);
        let mut prev = lock_of(rows);
        stamp(&mut prev, &after_set);
        (prev, before, after_set)
    }

    /// Only a hand or an older grim writes this lock, since an edit that changes rows
    /// writes a hash of its own: the landing reads the hash, never the rows.
    #[test]
    fn an_edit_back_to_the_hash_a_stale_lock_holds_returns_it_as_it_is() {
        let (prev, before, after_set) = landing(vec![row_at("edited", EDITED)]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "third", &before, &after_set);

        assert_eq!(out.lock, prev);
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    #[test]
    fn an_edit_back_to_the_hash_of_a_lock_that_answers_it_stays_a_proof() {
        let rows = vec![row_at("edited", EDITED), row_at("other", OTHER_SKILL)];
        let (prev, before, after_set) = landing(rows);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "third", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
    }

    /// A tagless pin cannot show what it answers, but the hash did when a relock
    /// wrote it, and that hash reads true again.
    #[test]
    fn an_edit_back_to_the_hash_of_a_tagless_lock_stays_a_proof() {
        let rows = vec![
            row_at("edited", "localhost:5000/acme/edited"),
            row_at("other", "localhost:5000/acme/other"),
        ];
        let (prev, before, after_set) = landing(rows);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "third", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
    }

    /// An edge with no row leaves no closure to read, and the landing needs none.
    #[test]
    fn an_edit_back_to_the_hash_of_a_lock_with_no_closure_returns_it_as_it_is() {
        let rows = vec![row_at("edited", EDITED), row_at("other", OTHER_SKILL)];
        let (mut prev, before, after_set) = landing(rows);
        prev.skills[0].requires = vec![m_edge(M1)];
        assert!(crate::lock::effective_set::effective_set(&before, &prev).is_none());

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "third", &before, &after_set);

        assert_eq!(out.lock, prev);
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    /// A declared bundle with no row, as a pre-cache lock has it, and the hash the
    /// edit lands on proves it all the same.
    #[test]
    fn a_pre_cache_lock_on_the_hash_the_edit_lands_on_is_returned_as_it_is() {
        let after_set = set_of(
            &[("edited", EDITED), ("other", OTHER_SKILL)],
            &[("pack", "localhost:5000/acme/pack:1")],
        );
        let mut before = after_set.clone();
        let third = Identifier::parse("localhost:5000/acme/third:1").unwrap();
        let line = crate::config::declaration::DeclaredSource::Registry(third);
        before.skills.insert("third".to_string(), line);
        before.invalidate_declaration_hash_cache();
        let mut prev = lock_of(vec![row_at("edited", EDITED), row_at("other", OTHER_SKILL)]);
        stamp(&mut prev, &after_set);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "third", &before, &after_set);

        assert_eq!(out.lock, prev);
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    const A1: &str = "localhost:5000/acme/a:1";
    const Y1: &str = "localhost:5000/acme/y:1";
    const Z1: &str = "localhost:5000/acme/z:1";

    /// A lock proving `a`, `y` and `z`, and the declaration after a hand moved `a`
    /// to another tag.
    fn hand_edited() -> (GrimoireLock, DesiredSet, DesiredSet) {
        let proven = set_of(&[("a", A1), ("y", Y1), ("z", Z1)], &[]);
        let mut prev = lock_of(vec![row_at("a", A1), row_at("y", Y1), row_at("z", Z1)]);
        stamp(&mut prev, &proven);
        let edited = set_of(&[("a", "localhost:5000/acme/a:2"), ("y", Y1), ("z", Z1)], &[]);
        (prev, proven, edited)
    }

    fn without(set: &DesiredSet, name: &str) -> DesiredSet {
        let mut set = set.clone();
        set.skills.remove(name);
        set.invalidate_declaration_hash_cache();
        set
    }

    /// The edit kept the old hash, so restoring `grimoire.toml` by hand proved a
    /// lock without `y`, and `grim install` exited 0.
    #[test]
    fn an_edit_that_takes_a_row_from_a_stale_lock_ends_its_hash() {
        let (prev, proven, edited) = hand_edited();
        let after = without(&edited, "y");

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "y", &edited, &after);

        assert_eq!(skill_names(&out.lock), ["a", "z"], "the fixture takes a row");
        let hash = &out.lock.metadata.declaration_hash;
        assert_ne!(hash, proven.declaration_hash_cached(), "a hand undo would prove it");
        assert_ne!(hash, after.declaration_hash_cached());
        assert_eq!(hash.strip_prefix("sha256:").map(str::len), Some(64), "{hash}");
    }

    /// After the hand undo the lock read fresh, so the next edit restamped over the
    /// lost row of `y`.
    #[test]
    fn a_hand_undo_leaves_a_later_edit_no_proof_to_restamp() {
        let (prev, proven, edited) = hand_edited();
        let first = drop_from_lock(&prev, ArtifactKind::Skill, "y", &edited, &without(&edited, "y"));
        let after = without(&proven, "z");

        let out = drop_from_lock(&first.lock, ArtifactKind::Skill, "z", &proven, &after);

        assert_eq!(
            skill_names(&out.lock),
            ["a", "z"],
            "`y` has no row, so it may require `z`"
        );
        assert_ne!(out.lock.metadata.declaration_hash, after.declaration_hash_cached());
    }

    /// Each edit that leaves the lock unproven writes a hash that follows from the
    /// lock it edited alone, and none is a declaration's. Taking `a` is not one:
    /// what is left answers the edited declaration and takes its stamp.
    #[test]
    fn a_hash_an_edit_writes_is_deterministic_and_no_declarations() {
        let (prev, proven, edited) = hand_edited();
        let edit = |name: &str| drop_from_lock(&prev, ArtifactKind::Skill, name, &edited, &without(&edited, name));
        let declarations = [
            DesiredSet::default(),
            proven.clone(),
            edited.clone(),
            without(&edited, "y"),
            without(&edited, "z"),
        ];

        for name in ["y", "z"] {
            let (once, again) = (edit(name).lock, edit(name).lock);
            assert_eq!(once, again, "{name}");
            for set in &declarations {
                assert_ne!(once.metadata.declaration_hash, set.declaration_hash_cached(), "{name}");
            }
        }
    }

    /// `prev` proves `h1` and `h2` asking for `m` at two tags, kept at `pin`, and a
    /// hand typed `m` at `typed` over that.
    fn typed_over_a_diamond(pin: &str, typed: &str) -> (GrimoireLock, DesiredSet, DesiredSet) {
        let (prev, after_set) = diamond(&[("h1", M1), ("h2", M2)], pin);
        let mut before = after_set.clone();
        let line = crate::config::declaration::DeclaredSource::Registry(Identifier::parse(typed).unwrap());
        before.skills.insert("m".to_string(), line);
        before.invalidate_declaration_hash_cache();
        (prev, before, after_set)
    }

    #[test]
    fn a_line_typed_over_a_settled_key_costs_no_proof() {
        for typed in [M1, M2] {
            let (prev, before, after_set) = typed_over_a_diamond(M1, typed);

            let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

            assert_eq!(out.lock, prev, "typed {typed}");
            assert!(out.notes.is_empty(), "typed {typed}: {:?}", out.notes);
        }
    }

    /// A proof reached `m` by an edge, and its tagless pin names no request: the row
    /// and the proof stay all the same.
    #[test]
    fn a_line_typed_over_a_tagless_row_keeps_the_row() {
        const TAGLESS: &str = "localhost:5000/acme/m";
        let (prev, before, after_set) = typed_over_a_diamond(TAGLESS, M1);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(out.lock, prev);
    }

    /// A row gone under a hash that still matches is a lock only a hand or an older
    /// grim wrote, and the landing takes it as it is.
    #[test]
    fn a_line_typed_over_a_lost_row_returns_the_lock_as_it_is() {
        let (mut prev, before, after_set) = typed_over_a_diamond(M1, M1);
        prev.skills.retain(|row| row.name != "m");

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(out.lock, prev);
    }

    /// A line typed over a row nothing else reaches, then taken out. The hash proves
    /// the declaration, not the row: kept, the next `grim install` brings back what
    /// `grim uninstall` just deleted.
    #[test]
    fn a_line_typed_over_a_ghost_takes_the_ghost_when_it_leaves() {
        let (prev, before, after_set) = landing(vec![row_at("edited", EDITED), row_at("third", OTHER_SKILL)]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "third", &before, &after_set);

        assert_eq!(skill_names(&out.lock), ["edited"]);
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    /// Rows that stand say nothing of a hash the edit does not land on: this lock
    /// answers what is left, so it takes the stamp.
    #[test]
    fn a_stale_lock_on_another_hash_takes_the_stamp_it_earns() {
        let rows = vec![row_at("edited", EDITED), row_at("other", OTHER_SKILL)];
        let (prev, before, after_set) = landing(rows);
        let prev = drifted(prev);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "third", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
    }

    /// No proof in hand settled `m` here, so taking its line out is the edit it always
    /// was, and an edit that changes no row keeps the stale hash.
    #[test]
    fn a_line_typed_over_a_key_on_another_hash_goes_through_the_edit() {
        let (prev, before, after_set) = typed_over_a_diamond(M1, M1);
        let prev = drifted(prev);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(out.lock, prev);
        assert_eq!(out.notes.len(), 1, "{:?}", out.notes);
    }

    /// `h1` and `h2`, tagged and declared, each with its edge from `edges`, and `rest`
    /// beside them. The edit takes `other`, which leaves a lock it could restamp.
    fn stale_holders(
        edges: [crate::oci::bundle::BundleMember; 2],
        rest: Vec<LockedArtifact>,
    ) -> (GrimoireLock, DesiredSet, DesiredSet) {
        let after_set = holders_set(&["h1", "h2"]);
        let mut before = after_set.clone();
        let other = Identifier::parse("localhost:5000/other:1").unwrap();
        let line = crate::config::declaration::DeclaredSource::Registry(other);
        before.skills.insert("other".to_string(), line);
        before.invalidate_declaration_hash_cache();
        let mut rows = vec![row_at("other", "localhost:5000/other:1")];
        for (name, edge) in ["h1", "h2"].into_iter().zip(edges) {
            let mut row = row_at(name, &format!("localhost:5000/{name}:1"));
            row.requires = vec![edge];
            rows.push(row);
        }
        rows.extend(rest);
        (drifted(lock_of(rows)), before, after_set)
    }

    #[test]
    fn a_digest_edge_the_pin_does_not_hold_keeps_a_stale_lock_stale() {
        let elsewhere = format!("localhost:5000/acme/m@sha256:{}", "e".repeat(64));
        let (prev, before, after_set) = stale_holders([m_edge(M1), m_edge(&elsewhere)], vec![m_at(M1)]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "other", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, edited_from(&prev));
    }

    #[test]
    fn a_digest_edge_the_pin_holds_lets_a_stale_lock_restamp() {
        let own = format!("localhost:5000/acme/m@{}", pin_digest());
        let (prev, before, after_set) = stale_holders([m_edge(M1), m_edge(&own)], vec![m_at(M1)]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "other", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
    }

    /// A required bundle takes the same rule as an artifact.
    #[test]
    fn a_digest_edge_a_bundle_row_does_not_hold_keeps_a_stale_lock_stale() {
        let edge = |id: &str| crate::oci::bundle::BundleMember {
            kind: ArtifactKind::Bundle,
            name: "pack".to_string(),
            id: id.to_string(),
        };
        let elsewhere = format!("localhost:5000/acme/pack@sha256:{}", "e".repeat(64));
        let edges = [edge("localhost:5000/acme/pack:1"), edge(&elsewhere)];
        let (mut prev, before, after_set) = stale_holders(edges, vec![]);
        prev.bundles = vec![snapshot("pack", "localhost:5000/acme/pack", "1", &[])];
        prev.bundles[0].transitive = true;

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "other", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, edited_from(&prev));
    }

    /// A `[[bundle]]` row nothing reaches is no row a relock writes, and the landing
    /// leaves it to the next one.
    #[test]
    fn a_bundle_row_nothing_reaches_stays_on_the_hash_the_edit_lands_on() {
        let rows = vec![row_at("edited", EDITED), row_at("other", OTHER_SKILL)];
        let (mut prev, before, after_set) = landing(rows);
        prev.bundles = vec![snapshot("gone", "localhost:5000/acme/gone", "1", &[])];

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "third", &before, &after_set);

        assert_eq!(out.lock, prev);
    }

    /// No relock writes a row nothing reaches, and the landing leaves it to the next one.
    #[test]
    fn an_edit_back_to_a_hash_over_a_row_nothing_reaches_returns_it_as_it_is() {
        let rows = vec![
            row_at("edited", EDITED),
            row_at("other", OTHER_SKILL),
            row_at("ghost", "localhost:5000/acme/ghost:1"),
        ];
        let (prev, before, after_set) = landing(rows);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "third", &before, &after_set);

        assert_eq!(out.lock, prev);
    }

    /// An older `remove` marked the row `transitive` when only an edge held it, and
    /// the declaration came back: the landing takes the row as it is.
    #[test]
    fn an_edit_back_to_a_hash_over_a_declared_row_marked_transitive_returns_it_as_it_is() {
        const PACK: &str = "localhost:5000/acme/pack";
        let bundles = [("pack", "localhost:5000/acme/pack:1")];
        let after_set = set_of(&[("edited", EDITED)], &bundles);
        let before = set_of(
            &[("edited", EDITED), ("third", "localhost:5000/acme/third:1")],
            &bundles,
        );
        let mut prev = lock_of(vec![row_at("edited", EDITED)]);
        prev.bundles = vec![snapshot("pack", PACK, "1", &[])];
        prev.bundles[0].transitive = true;
        stamp(&mut prev, &after_set);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "third", &before, &after_set);

        assert_eq!(out.lock, prev);
    }

    /// An empty hash read from disk proves nothing, as any stale one, so rows that
    /// answer the edited declaration in full take its stamp.
    #[test]
    fn an_empty_hash_on_disk_is_stale_and_takes_the_stamp_it_earns() {
        let mut prev = lock_of(vec![row_at("edited", EDITED), row_at("other", OTHER_SKILL)]);
        prev.metadata.declaration_hash = String::new();
        let before = set_of(&[("edited", EDITED), ("other", OTHER_SKILL)], &[]);
        let after_set = set_of(&[("edited", EDITED)], &[]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "other", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
    }

    /// `m` unstamped under two flat bundles: a hand edit took its direct line, so
    /// the missing stamp is all that says so.
    fn hand_deleted_direct(b_asks: &str) -> (GrimoireLock, DesiredSet, DesiredSet) {
        let mut prev = drifted(lock_of(vec![m_at(M1)]));
        prev.bundles = vec![
            snapshot("a", "localhost:5000/acme/a", "1", &[("m", M1)]),
            snapshot("b", "localhost:5000/acme/b", "1", &[("m", b_asks)]),
        ];
        let before = set_of(
            &[],
            &[("a", "localhost:5000/acme/a:1"), ("b", "localhost:5000/acme/b:1")],
        );
        let after_set = set_of(&[], &[("b", "localhost:5000/acme/b:1")]);
        assert!(!has_dependency_data(&prev), "the flat path");
        (prev, before, after_set)
    }

    #[test]
    fn an_unstamped_row_no_surviving_request_names_leaves() {
        let (prev, before, after_set) = hand_deleted_direct(M2);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "a", &before, &after_set);

        assert!(out.lock.skills.iter().all(|s| s.name != "m"), "{:?}", out.lock.skills);
        assert_eq!(out.notes.len(), 1, "{:?}", out.notes);
        assert!(out.notes[0].contains("acme/b:1"), "{:?}", out.notes);
    }

    #[test]
    fn an_unstamped_row_a_surviving_request_names_stays() {
        let (prev, before, after_set) = hand_deleted_direct(M1);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "a", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M1));
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    /// `m` unstamped under flat bundles `a` and `b` naming two tags, and `c` the edit
    /// takes: a hand took the line that held it, and offline the tags read as a conflict.
    fn hand_deleted_over_two_tags(pin: &str) -> (GrimoireLock, DesiredSet, DesiredSet) {
        let mut prev = drifted(lock_of(vec![m_at(pin)]));
        prev.bundles = vec![
            snapshot("a", "localhost:5000/acme/a", "1", &[("m", M1)]),
            snapshot("b", "localhost:5000/acme/b", "1", &[("m", M2)]),
            snapshot("c", "localhost:5000/acme/c", "1", &[]),
        ];
        let kept = [("a", "localhost:5000/acme/a:1"), ("b", "localhost:5000/acme/b:1")];
        let before = set_of(&[], &[kept[0], kept[1], ("c", "localhost:5000/acme/c:1")]);
        let after_set = set_of(&[], &kept);
        assert!(!has_dependency_data(&prev), "the flat path");
        (prev, before, after_set)
    }

    #[test]
    fn an_unstamped_row_either_tag_names_stays_and_the_proof_ends() {
        for (pin, unnamed) in [(M1, "m:2"), (M2, "m:1")] {
            let (prev, before, after_set) = hand_deleted_over_two_tags(pin);

            let out = drop_from_lock(&prev, ArtifactKind::Bundle, "c", &before, &after_set);

            assert_eq!(pin_of(&out.lock, "m"), &pinned_at(pin));
            assert_eq!(out.lock.skills[0].bundles.len(), 2, "both bundles stamp it");
            assert_eq!(out.lock.metadata.declaration_hash, edited_from(&prev));
            assert_eq!(out.notes.len(), 1, "{:?}", out.notes);
            assert!(out.notes[0].contains(unnamed), "{:?}", out.notes);
            assert!(!out.notes[0].contains("disagree"), "{:?}", out.notes);
        }
    }

    /// Two tags can hold one digest, and with no digest to share the lock cannot tell.
    #[test]
    fn an_unstamped_row_neither_tag_names_leaves_without_calling_them_a_conflict() {
        let (prev, before, after_set) = hand_deleted_over_two_tags(M3);

        let out = drop_from_lock(&prev, ArtifactKind::Bundle, "c", &before, &after_set);

        assert!(out.lock.skills.is_empty(), "{:?}", out.lock.skills);
        assert_ne!(out.lock.metadata.declaration_hash, "");
        assert_eq!(out.notes.len(), 1, "{:?}", out.notes);
        assert!(out.notes[0].contains("grim lock"), "{:?}", out.notes);
        assert!(!out.notes[0].contains("disagree"), "{:?}", out.notes);
    }

    /// `h1` and `h2` over a direct `m`, whose declaration hid `h2`'s edge.
    fn direct_over_holders(pin: &str, h2: &str) -> (GrimoireLock, DesiredSet, DesiredSet) {
        let (mut prev, _) = diamond(&[("h1", M1), ("h2", h2)], pin);
        let after_set = holders_set(&["h1", "h2"]);
        let mut before = after_set.clone();
        before.skills.insert(
            "m".to_string(),
            crate::config::declaration::DeclaredSource::Registry(Identifier::parse(M1).unwrap()),
        );
        before.invalidate_declaration_hash_cache();
        stamp(&mut prev, &before);
        (prev, before, after_set)
    }

    #[test]
    fn a_direct_key_over_an_edge_it_never_resolved_keeps_its_pin_and_ends_the_proof() {
        let (prev, before, after_set) = direct_over_holders(M1, M2);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M1));
        assert_eq!(out.lock.metadata.declaration_hash, edited_from(&prev));
        assert_eq!(out.notes.len(), 1, "{:?}", out.notes);
        assert!(out.notes[0].contains("m:2"), "{:?}", out.notes);
        assert!(out.notes[0].contains("grim lock"), "{:?}", out.notes);
    }

    /// `grim lock` carries the pin at exit 0 even over a real conflict, so it checks
    /// nothing: the note says it keeps the pin and names the command that resolves
    /// the hidden request.
    #[test]
    fn the_note_for_a_hidden_edge_names_the_command_that_resolves_it() {
        let (prev, before, after_set) = direct_over_holders(M1, M2);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        let note = &out.notes[0];
        assert!(!note.contains("to check it"), "{note}");
        assert!(note.contains("run `grim lock`, which keeps the pin"), "{note}");
        assert!(note.contains("then `grim update -- m`"), "{note}");
        assert!(note.contains("exits 78"), "{note}");
    }

    /// A global or `--config` edit's notes name that scope: a plain `grim` run
    /// inside a project would lock and update that project instead.
    #[test]
    fn a_global_edit_names_the_global_flag_in_every_note() {
        every_note_names(&Invocation::GLOBAL);
    }

    #[test]
    fn a_config_edit_names_the_config_path_in_every_note() {
        every_note_names(&Invocation::config(std::path::Path::new("/srv/my team/grimoire.toml")));
    }

    /// A status line leads with [`EditNote::lock_command`], so no note may ask for
    /// anything else.
    /// A note names at most three rows and counts the rest, so it stays one short line
    /// however many rows an edit leaves unjudged.
    #[test]
    fn a_note_names_three_rows_and_counts_the_rest() {
        let keys: Vec<NodeKey> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|name| (ArtifactKind::Skill, (*name).to_string()))
            .collect();
        assert_eq!(super::named_rows(&keys[..1]), "skill 'a'");
        assert_eq!(super::named_rows(&keys[..3]), "skill 'a', skill 'b' and skill 'c'");
        assert_eq!(super::named_rows(&keys), "skill 'a', skill 'b', skill 'c' and 2 more");
    }

    /// A note that warns the next `grim lock` may move a pin asks for none: a status
    /// line leading with it would advise the very move.
    #[test]
    fn a_note_asks_for_its_lock_command_unless_that_lock_is_the_move_it_warns_of() {
        let key = || (ArtifactKind::Skill, "m".to_string());
        let kinds = [
            NoteKind::HiddenRequest {
                key: key(),
                unanswered: Identifier::parse(M2).unwrap(),
            },
            NoteKind::Unmasked {
                key: key(),
                by: Provider::RequiredBy(vec![key()]),
                version: VersionClaim::Different,
            },
            NoteKind::Unsettled { key: key() },
            NoteKind::ListedMember {
                key: key(),
                bundle: "pack".to_string(),
            },
            NoteKind::MemberUnknown {
                key: key(),
                bundle: "pack".to_string(),
                afresh: false,
            },
            NoteKind::MemberUnknown {
                key: key(),
                bundle: "pack".to_string(),
                afresh: true,
            },
            NoteKind::RequestUnknown {
                key: key(),
                parent: (ArtifactKind::Skill, "p".to_string()),
                afresh: true,
            },
            NoteKind::GraphUnknown {
                kept: vec![key(), key()],
            },
        ];
        for what in kinds {
            let note = EditNote {
                grim: Invocation::GLOBAL,
                what,
            };
            let asked = format!("`{}`", note.lock_command().expect("a lock to run now"));
            assert!(note.to_string().contains(&asked), "{note}");
        }
        let warnings = [
            NoteKind::LeftUnread {
                keys: vec![key()],
                bundles: vec!["pack".to_string(), "pk2".to_string()],
            },
            NoteKind::Unwalked {
                keys: vec![key()],
                askers: vec![(ArtifactKind::Skill, "p".to_string())],
                shown: false,
                config: "grimoire.toml".to_string(),
                lock: "grimoire.lock".to_string(),
                removed: None,
                stale: false,
            },
        ];
        for what in warnings {
            let note = EditNote {
                grim: Invocation::GLOBAL,
                what,
            };
            assert_eq!(note.lock_command(), None, "{note}");
        }
    }

    fn every_note_names(grim: &Invocation) {
        let global = |prev: &GrimoireLock, before: &DesiredSet, after: &DesiredSet| -> Vec<String> {
            let cold = StoredMembers::default();
            rendered(&super::drop_from_lock(prev, ArtifactKind::Skill, "m", before, after, grim, &cold).notes)
        };
        let (prev, before, after_set) = direct_over_holders(M1, M2);
        let hidden = global(&prev, &before, &after_set);
        let (mut prev, before, after_set) = direct_over_holders(M1, M2);
        prev.skills[0].requires = vec![m_edge(M3)];
        stamp(&mut prev, &before);
        let unmasked = global(&prev, &before, &after_set);
        let (prev, before, after_set) = m_beside_a_pack_with_no_member_list(vec![], &[]);
        let legacy = global(&drifted(prev), &before, &after_set);
        let h = ("h", "localhost:5000/h:1");
        let (prev, before, after_set) = m_beside_a_pack_with_no_member_list(vec![holder(h.0, M2)], &[h]);
        let target_only = global(&prev, &before, &after_set);
        let mut unsettled = LockEdit::new(grim);
        unsettled.unsettled(&(ArtifactKind::Skill, "m".to_string()), 0);
        let unsettled = rendered(&unsettled.notes);

        let advice = format!("run `{grim} lock`, which keeps the pin, then `{grim} update -- m`");
        assert!(hidden.iter().any(|note| note.contains(&advice)), "{hidden:?}");
        for notes in [hidden, unmasked, legacy, target_only, unsettled] {
            assert!(!notes.is_empty());
            for note in notes {
                assert!(note.contains(&format!("`{grim} lock`")), "{note}");
                assert!(
                    !note.contains("`grim lock`") && !note.contains("`grim update"),
                    "{note}"
                );
            }
        }
    }

    #[test]
    fn a_direct_key_every_edge_names_keeps_the_proof() {
        let (prev, before, after_set) = direct_over_holders(M1, M1);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M1));
        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert!(out.notes.is_empty(), "{:?}", out.notes);
    }

    #[test]
    fn a_direct_key_no_edge_names_leaves_and_says_so() {
        let (mut prev, before, after_set) = direct_over_holders(M1, M2);
        prev.skills[0].requires = vec![m_edge(M3)];
        stamp(&mut prev, &before);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert!(prev.skills.len() > out.lock.skills.len(), "the row leaves");
        assert_eq!(out.lock.metadata.declaration_hash, edited_from(&prev));
        assert!(out.notes[0].contains("at a different version"), "{:?}", out.notes);
    }

    #[test]
    fn a_tagless_pin_no_edge_names_is_not_called_another_version() {
        const TAGLESS: &str = "localhost:5000/acme/m";
        let (prev, before, after_set) = direct_over_holders(TAGLESS, M2);
        let mut before = before;
        before.skills.insert(
            "m".to_string(),
            crate::config::declaration::DeclaredSource::Registry(Identifier::parse(M3).unwrap()),
        );
        before.invalidate_declaration_hash_cache();
        let mut prev = prev;
        stamp(&mut prev, &before);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert!(!out.notes[0].contains("at a different version"), "{:?}", out.notes);
        assert!(out.notes[0].contains("cannot confirm"), "{:?}", out.notes);
    }

    // ── An edit that ends a proof stamps what the proof showed ───────────

    /// A row no declaration holds, `requests` of it recorded by `h1` and `h2`.
    fn with_dependency(prev: &mut GrimoireLock, pin: &str, requests: [&str; 2]) {
        for (holder, id) in prev.skills.iter_mut().zip(requests) {
            holder.requires.push(crate::oci::bundle::BundleMember {
                kind: ArtifactKind::Skill,
                name: "t".to_string(),
                id: id.to_string(),
            });
        }
        prev.skills.push(LockedArtifact::direct(
            "t".to_string(),
            ArtifactKind::Skill,
            pinned_at(pin),
        ));
    }

    /// `m` direct over `h1` and `h2`, as a lock from before `pinned` held a tag.
    fn pre_change(h2: &str) -> (GrimoireLock, DesiredSet, DesiredSet) {
        let (prev, before, after_set) = direct_over_holders(M1, h2);
        (prev.without_advisory_tags(), before, after_set)
    }

    #[test]
    fn a_proof_the_edit_ends_takes_the_stamps_it_showed() {
        let (prev, before, after_set) = pre_change(M2);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, edited_from(&prev));
        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M1), "the declared tag");
        assert_eq!(pin_of(&out.lock, "h1").tag(), Some("1"), "a bystander");
        assert_eq!(pin_of(&out.lock, "h1").digest(), pin_of(&prev, "h1").digest());
    }

    #[test]
    fn a_hash_the_edit_leaves_stale_takes_the_same_stamps() {
        let (mut prev, before, after_set) = pre_change(M2);
        prev.skills[0].requires = vec![m_edge(M3)];

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, edited_from(&prev));
        assert!(out.lock.skills.iter().all(|s| s.name != "m"), "the row left");
        assert_eq!(pin_of(&out.lock, "h1").tag(), Some("1"));
    }

    #[test]
    fn a_key_no_declaration_holds_takes_the_edge_a_relock_would() {
        const TAGLESS: &str = "localhost:5000/acme/t";
        let cases = [
            (["localhost:5000/acme/t:2", "localhost:5000/acme/t:1"], Some("t:1")),
            (["localhost:5000/aaa/t:1", "localhost:5000/acme/t:2"], Some("t:2")),
            (["localhost:5000/aaa/t:1", "localhost:5000/zzz/t:2"], None),
        ];
        for (requests, expected) in cases {
            let (mut prev, before, after_set) = pre_change(M2);
            with_dependency(&mut prev, TAGLESS, requests);

            let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

            let expected = expected.map_or(TAGLESS.to_string(), |tagged| format!("localhost:5000/acme/{tagged}"));
            assert_eq!(pin_of(&out.lock, "t"), &pinned_at(&expected), "{requests:?}");
        }
    }

    /// `h1` left with the smaller tag: a pin stamped from it would name no edge
    /// that stays, and on a stale hash such a pin vouches for nothing.
    #[test]
    fn a_stamp_reads_the_edges_that_stayed() {
        let (mut prev, before, _) = pre_change(M2);
        let t = ["localhost:5000/acme/t:1", "localhost:5000/acme/t:2"];
        with_dependency(&mut prev, "localhost:5000/acme/t", t);
        let before = beside_a_pack_that_provides_nothing(&before);
        stamp(&mut prev, &before);
        assert!(crate::lock::effective_set::effective_set(&before, &prev).is_none());
        let mut after_set = before.clone();
        after_set.skills.remove("h1");
        after_set.invalidate_declaration_hash_cache();
        let stored = StoredMembers::holding("pack", &[]);

        let out = drop_beside(&prev, ArtifactKind::Skill, "h1", &before, &after_set, &stored);

        assert_eq!(pin_of(&out.lock, "t"), &pinned_at(t[1]));
        assert_eq!(
            pin_of(&out.lock, "m"),
            &pinned_at(M1),
            "the target-only path stamps too"
        );
    }

    #[test]
    fn the_legacy_path_stamps_what_it_leaves_stale() {
        let stamp_of = |repo: &str| crate::lock::locked_artifact::BundleProvenance::new(repo, "1");
        let (a, b) = ("localhost:5000/acme/a", "localhost:5000/acme/b");
        let mut m = m_at("localhost:5000/acme/m");
        m.bundles = vec![stamp_of(a), stamp_of(b)];
        let mut prev = lock_of(vec![m, m_named("y", "localhost:5000/acme/y")]);
        prev.bundles = vec![snapshot("a", a, "1", &[("m", M1)]), snapshot("b", b, "1", &[("m", M1)])];
        // `c` has no snapshot, which degrades the whole call to the legacy path.
        let bundles = [
            ("a", "localhost:5000/acme/a:1"),
            ("b", "localhost:5000/acme/b:1"),
            ("c", "localhost:5000/acme/c:1"),
        ];
        let skills = [("m", M1), ("y", "localhost:5000/acme/y:1")];
        let before = set_of(&skills, &bundles);
        stamp(&mut prev, &before);
        assert!(crate::lock::effective_set::effective_set(&before, &prev).is_none());
        assert!(!has_dependency_data(&prev));
        let after_set = set_of(&skills[1..], &bundles);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, edited_from(&prev));
        assert_eq!(pin_of(&out.lock, "y").tag(), Some("1"));
    }

    #[test]
    fn a_digest_only_request_stays_tagless_when_the_edit_ends_the_proof() {
        let tagless = |name: &str| format!("localhost:5000/acme/{name}");
        let frozen = format!("{}@{}", tagless("y"), pin_digest());
        let (prev, before, after_set) = pre_change(M2);
        let (mut prev, before, after_set) = declaring(prev, before, after_set, "y", &frozen);
        let edge = format!("{}@{}", tagless("t"), pin_digest());
        with_dependency(&mut prev, &tagless("t"), [edge.as_str(), "localhost:5000/acme/t:1"]);
        prev.skills.push(m_named("y", &tagless("y")));

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(
            out.lock.metadata.declaration_hash,
            edited_from(&prev),
            "the edit ended the proof"
        );
        assert_eq!(pin_of(&out.lock, "y").tag(), None);
        assert_eq!(
            pin_of(&out.lock, "t").tag(),
            None,
            "a digest-only edge wins, as in a relock"
        );
    }

    #[test]
    fn a_stamp_never_changes_a_digest() {
        let elsewhere = format!("localhost:5000/acme/y:3@{}", Digest::Sha256("e".repeat(64)));
        let own = format!("localhost:5000/acme/z:3@{}", pin_digest());
        let (prev, before, after_set) = pre_change(M2);
        let (prev, before, after_set) = declaring(prev, before, after_set, "y", &elsewhere);
        let (mut prev, before, after_set) = declaring(prev, before, after_set, "z", &own);
        prev.skills.push(m_named("y", "localhost:5000/acme/y"));
        prev.skills.push(m_named("z", "localhost:5000/acme/z"));

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(
            out.lock.metadata.declaration_hash,
            edited_from(&prev),
            "the edit ended the proof"
        );
        assert_eq!(pin_of(&out.lock, "y"), &pinned_at("localhost:5000/acme/y"));
        assert_eq!(pin_of(&out.lock, "z"), &pinned_at("localhost:5000/acme/z:3"));
    }

    #[test]
    fn a_declaration_in_another_repository_stamps_nothing() {
        let (prev, before, after_set) = pre_change(M2);
        let (mut prev, before, after_set) = declaring(prev, before, after_set, "y", "localhost:5000/other/y:1");
        prev.skills.push(m_named("y", "localhost:5000/acme/y"));

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "y"), &pinned_at("localhost:5000/acme/y"));
    }

    #[test]
    fn a_key_the_proof_declares_takes_that_tag_over_a_smaller_edge() {
        let (prev, _) = diamond(&[("h1", M1), ("h2", M2)], M2);
        let after_set = holders_set(&["h1", "h2"]);
        let mut before = after_set.clone();
        before.skills.insert(
            "m".to_string(),
            crate::config::declaration::DeclaredSource::Registry(Identifier::parse(M2).unwrap()),
        );
        before.invalidate_declaration_hash_cache();
        let mut prev = prev.without_advisory_tags();
        stamp(&mut prev, &before);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(
            out.lock.metadata.declaration_hash,
            edited_from(&prev),
            "the edit ended the proof"
        );
        assert_eq!(pin_of(&out.lock, "m"), &pinned_at(M2));
    }

    #[test]
    fn a_kept_tag_stays_when_the_edit_ends_the_proof() {
        const KEPT: &str = "localhost:5000/acme/t:2";
        let (mut prev, before, after_set) = direct_over_holders(M1, M2);
        with_dependency(&mut prev, KEPT, ["localhost:5000/acme/t:1", KEPT]);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(
            out.lock.metadata.declaration_hash,
            edited_from(&prev),
            "the edit ended the proof"
        );
        assert_eq!(pin_of(&out.lock, "t"), &pinned_at(KEPT));
    }

    #[test]
    fn a_lock_already_stale_takes_no_stamp() {
        let (prev, before, after_set) = pre_change(M2);
        let prev = drifted(prev);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(pin_of(&out.lock, "h1"), pin_of(&prev, "h1"));
    }

    #[test]
    fn a_hash_that_stays_a_proof_leaves_the_stamps_to_the_relock() {
        let (prev, before, after_set) = pre_change(M1);

        let out = drop_from_lock(&prev, ArtifactKind::Skill, "m", &before, &after_set);

        assert_eq!(out.lock.metadata.declaration_hash, after_set.declaration_hash_cached());
        assert_eq!(pin_of(&out.lock, "h1"), pin_of(&prev, "h1"));
    }

    /// `prev`, proven again with `name` declared at `id` on both sides of the edit.
    fn declaring(
        mut prev: GrimoireLock,
        mut before: DesiredSet,
        mut after_set: DesiredSet,
        name: &str,
        id: &str,
    ) -> (GrimoireLock, DesiredSet, DesiredSet) {
        for set in [&mut before, &mut after_set] {
            set.skills.insert(
                name.to_string(),
                crate::config::declaration::DeclaredSource::Registry(Identifier::parse(id).unwrap()),
            );
            set.invalidate_declaration_hash_cache();
        }
        stamp(&mut prev, &before);
        (prev, before, after_set)
    }

    fn m_named(name: &str, reference: &str) -> LockedArtifact {
        LockedArtifact::direct(name.to_string(), ArtifactKind::Skill, pinned_at(reference))
    }

    #[test]
    fn a_hostile_name_reaches_every_note_escaped() {
        const NAME: &str = "a\u{1b}[2J";
        let key = (ArtifactKind::Skill, NAME.to_string());
        let contributor = BundleProvenance::new("r\u{1b}[2J", "t\u{1b}[2J");
        let parent = [(ArtifactKind::Skill, "p\u{1b}[2J".to_string())];
        let mut edit = LockEdit::new(&Invocation::WALKED);
        let requests = BTreeSet::from([Identifier::parse(M1).unwrap(), Identifier::parse(M2).unwrap()]);
        let pin = pinned_at(M1);
        let record = PinRecord {
            pin: &pin,
            row: None,
            binding: NAME,
        };
        assert!(edit.declaration_left(&key, Some(record), None, &requests, |_| None));

        let unmasked = |by: Provider| NoteKind::Unmasked {
            key: key.clone(),
            by,
            version: VersionClaim::Different,
        };
        edit.note(unmasked(Provider::Bundle(contributor)));
        edit.note(unmasked(Provider::RequiredBy(parent.to_vec())));
        edit.note(NoteKind::Unsettled { key: key.clone() });
        edit.note(NoteKind::ListedMember {
            key: key.clone(),
            bundle: NAME.to_string(),
        });
        edit.note(NoteKind::MemberUnknown {
            key: key.clone(),
            bundle: NAME.to_string(),
            afresh: true,
        });
        edit.note(NoteKind::RequestUnknown {
            key: key.clone(),
            parent: parent[0].clone(),
            afresh: true,
        });
        edit.left_unread(&key, &[NAME, NAME]);
        for shown in [true, false] {
            edit.note(NoteKind::Unwalked {
                keys: vec![key.clone()],
                askers: parent.to_vec(),
                shown,
                config: NAME.to_string(),
                lock: NAME.to_string(),
                removed: None,
                stale: false,
            });
        }
        let notes = rendered(&edit.notes);
        for note in &notes {
            assert!(!note.contains('\u{1b}'), "{note}");
        }
        let unwalked = notes.iter().filter(|note| note.contains("no dependency edges"));
        assert_eq!(unwalked.count(), 2, "{notes:?}");
    }
}
