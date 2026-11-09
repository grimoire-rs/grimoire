// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Which binding of a declaration holds a catalog row, read from a lock written
//! for that declaration rather than guessed offline.

use std::collections::BTreeMap;

use crate::config::declaration::DesiredSet;
use crate::lock::effective_set::{NodeKey, Origin, effective_set};
use crate::lock::grimoire_lock::GrimoireLock;
use crate::oci::{ArtifactKind, Identifier, PinnedIdentifier};

/// A catalog row, or a bundle's member row, as a declaration sees it.
#[derive(Debug, Clone, Copy)]
pub struct RowAt<'a> {
    pub kind: ArtifactKind,
    /// The binding a catalog row guesses from its repository, or a member's own.
    pub name: &'a str,
    /// A member names its own binding.
    pub exact: bool,
    /// The row's `registry/repository`, whole: the fields of a namespaced
    /// registry split where a parsed reference does not.
    pub location: &'a str,
}

impl RowAt<'_> {
    /// Whether `set` declares the row under its own name. A member's name holds
    /// at any repository or a path, so an override stays.
    pub fn declared_by_name(&self, set: &DesiredSet) -> bool {
        set.table(self.kind)
            .get(self.name)
            .is_some_and(|source| self.exact || source.identifier().is_some_and(|id| self.at(id)))
    }

    fn at(&self, id: &Identifier) -> bool {
        id.registry_repository() == self.location
    }
}

/// The binding through which `set` holds `row`, read from `relocked`, a lock written
/// for `set` (a relock's, or one whose hash proves `set`): a direct entry at the
/// row's location under any binding, or a locked key `set` reaches whose pin, or a
/// request onto it, sits there. The row's own name
/// answers first. A member also holds its own name wherever `set` or `relocked`
/// binds it. `None` when nothing holds the row.
pub fn row_binding(set: &DesiredSet, relocked: &GrimoireLock, row: RowAt<'_>) -> Option<String> {
    let declared = set
        .table(row.kind)
        .iter()
        .filter(|(_, source)| source.identifier().is_some_and(|id| row.at(id)))
        .map(|(binding, _)| binding.as_str());
    let nodes = effective_set(set, relocked);
    let asked_here = |binding: &str| {
        nodes
            .as_ref()
            .and_then(|nodes| nodes.get(&(row.kind, binding.to_string())))
            .and_then(Origin::requests)
            .is_some_and(|requests| requests.iter().any(|id| row.at(id)))
    };
    let reached = reached_undeclared(set, relocked, nodes.as_ref(), row.kind)
        .filter(|(binding, pin)| pin.is_some_and(|pin| row.at(pin.as_identifier())) || asked_here(binding))
        .map(|(binding, _)| binding);
    let held: Vec<&str> = declared.chain(reached).collect();
    let bound = || {
        set.table(row.kind).contains_key(row.name)
            || reached_undeclared(set, relocked, nodes.as_ref(), row.kind).any(|(b, _)| b == row.name)
    };
    if held.contains(&row.name) || (row.exact && bound()) {
        Some(row.name.to_string())
    } else if row.exact {
        None
    } else {
        held.first().map(ToString::to_string)
    }
}

/// Where `relocked`, a lock written for `set`, holds the key a catalog row guesses
/// when that key sits at another location: the `registry/repository` of a key of
/// the row's kind and name that `set` reaches without declaring it. `None` for a
/// member, which names its own binding, and where `set` does not reach the key.
pub fn reached_elsewhere(set: &DesiredSet, relocked: &GrimoireLock, row: RowAt<'_>) -> Option<String> {
    if row.exact {
        return None;
    }
    let nodes = effective_set(set, relocked);
    reached_undeclared(set, relocked, nodes.as_ref(), row.kind)
        .filter(|(binding, _)| *binding == row.name)
        .find_map(|(_, pin)| match pin {
            Some(pin) if row.at(pin.as_identifier()) => None,
            Some(pin) => Some(pin.as_identifier().registry_repository()),
            None => Some("a path".to_string()),
        })
}

/// The rows of `kind` that `set` reaches without declaring them, with their pins. A
/// direct key holds a row only through its declaration, whatever its row says. A
/// row `nodes`, the walk of `set`, does not hold is a ghost an old TUI install or a
/// hand edit left under an intact hash, which `grim update` prunes, so it is none.
/// With no readable walk every row counts: a lock no relock wrote still has pins.
fn reached_undeclared<'a>(
    set: &'a DesiredSet,
    lock: &'a GrimoireLock,
    nodes: Option<&'a BTreeMap<NodeKey, Origin>>,
    kind: ArtifactKind,
) -> impl Iterator<Item = (&'a str, Option<&'a PinnedIdentifier>)> {
    locked_rows(lock, kind)
        .filter(move |(binding, _)| !set.table(kind).contains_key(*binding))
        .filter(move |(binding, _)| nodes.is_none_or(|nodes| nodes.contains_key(&(kind, (*binding).to_string()))))
}

/// Every row of `kind` in `lock` with its pin; a path source has none.
fn locked_rows(lock: &GrimoireLock, kind: ArtifactKind) -> impl Iterator<Item = (&str, Option<&PinnedIdentifier>)> {
    let artifacts = lock
        .iter_artifacts()
        .filter(move |row| row.kind == kind)
        .map(|row| (row.name.as_str(), row.source.pinned()));
    let bundles = lock
        .bundles
        .iter()
        .filter(move |_| kind == ArtifactKind::Bundle)
        .map(|row| (row.name.as_str(), row.pinned()));
    artifacts.chain(bundles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::test_support::{declared, lock_of, sha};

    fn row<'a>(kind: ArtifactKind, name: &'a str, exact: bool, location: &'a str) -> RowAt<'a> {
        RowAt {
            kind,
            name,
            exact,
            location,
        }
    }

    /// A lock where `s` requires `inner`, which `set` does not declare.
    fn lock_with_a_required_bundle() -> (GrimoireLock, DesiredSet) {
        let row = format!(
            "[[bundle]]\nname = \"inner\"\nrepo = \"localhost:5050/acme/inner\"\ntag = \"1\"\n\
             pinned = \"localhost:5050/acme/inner@sha256:{}\"\ntransitive = true\n\n",
            sha('e')
        );
        let edge = ("bundle", "inner", "localhost:5050/acme/inner:1");
        let lock = lock_of(&[("s", "localhost:5050/acme/s", &[edge])], &row);
        (lock, declared(&[("s", "localhost:5050/acme/s:1")]))
    }

    #[test]
    fn a_bundle_an_edge_requires_holds_its_row() {
        let (lock, set) = lock_with_a_required_bundle();
        let at = row(ArtifactKind::Bundle, "inner", false, "localhost:5050/acme/inner");

        assert_eq!(row_binding(&set, &lock, at).as_deref(), Some("inner"));
    }

    #[test]
    fn a_bundle_holds_only_its_own_row() {
        let (lock, set) = lock_with_a_required_bundle();
        let fork = row(ArtifactKind::Bundle, "fork", false, "localhost:5050/acme/fork");

        assert_eq!(row_binding(&set, &lock, fork), None);
    }

    /// A skill bound to the bundle's name, elsewhere: the bundle's row is no skill's.
    #[test]
    fn a_bundle_row_holds_a_bundle_alone() {
        let (with_a_bundle, _) = lock_with_a_required_bundle();
        let mut lock = lock_of(&[("inner", "localhost:5050/acme/other", &[])], "");
        lock.bundles = with_a_bundle.bundles;
        let set = declared(&[("inner", "localhost:5050/acme/other:1")]);
        let at = row(ArtifactKind::Skill, "inner", false, "localhost:5050/acme/inner");

        assert_eq!(row_binding(&set, &lock, at), None);
    }

    /// An exact name holds its kind at any repository, and no other kind or name.
    #[test]
    fn an_exact_name_holds_only_where_it_is_reached() {
        let (lock, set) = lock_with_a_required_bundle();
        let held = |kind, name| row_binding(&set, &lock, row(kind, name, true, "localhost:5050/acme/fork"));

        assert_eq!(held(ArtifactKind::Bundle, "inner").as_deref(), Some("inner"));
        assert_eq!(held(ArtifactKind::Skill, "inner"), None);
        assert_eq!(held(ArtifactKind::Bundle, "outer"), None);
    }

    /// `p` asks `acme/d` and `q` asks `acme/mirror/d` of one key, whose pin rests on
    /// `acme/d`: the mirror's row is that key's too.
    #[test]
    fn a_request_at_the_row_holds_a_key_pinned_elsewhere() {
        let lock = lock_of(
            &[
                (
                    "p",
                    "localhost:5050/acme/p",
                    &[("skill", "d", "localhost:5050/acme/d:1")],
                ),
                (
                    "q",
                    "localhost:5050/acme/q",
                    &[("skill", "d", "localhost:5050/acme/mirror/d:1")],
                ),
                ("d", "localhost:5050/acme/d", &[]),
            ],
            "",
        );
        let set = declared(&[("p", "localhost:5050/acme/p:1"), ("q", "localhost:5050/acme/q:1")]);
        let mirror = row(ArtifactKind::Skill, "d", false, "localhost:5050/acme/mirror/d");
        let elsewhere = row(ArtifactKind::Skill, "d", false, "localhost:5050/acme/fork/d");

        assert_eq!(row_binding(&set, &lock, mirror).as_deref(), Some("d"));
        assert_eq!(row_binding(&set, &lock, elsewhere), None);
    }

    /// `grim add --name y` leaves a row whose guessed name is not its binding.
    #[test]
    fn a_direct_entry_holds_its_row_under_any_binding() {
        let lock = lock_of(&[("y", "localhost:5050/acme/x", &[])], "");
        let set = declared(&[("y", "localhost:5050/acme/x:1")]);

        let at = row(ArtifactKind::Skill, "x", false, "localhost:5050/acme/x");
        assert_eq!(row_binding(&set, &lock, at).as_deref(), Some("y"));
        assert!(!at.declared_by_name(&set), "a relock decides what an alias holds");
    }

    /// A direct key at another repository holds no row of the repository its
    /// bundle names: an Install there would replace the override.
    #[test]
    fn an_override_holds_no_row_of_the_member_it_replaced() {
        let bundle = format!(
            "[[bundle]]\nname = \"pack\"\nrepo = \"localhost:5050/acme/pack\"\ntag = \"1\"\n\
             pinned = \"localhost:5050/acme/pack:1@sha256:{}\"\n\n\
             [[bundle.member]]\nkind = \"skill\"\nname = \"m\"\nid = \"localhost:5050/acme/m:1\"\n\n",
            sha('e')
        );
        let lock = lock_of(&[("m", "localhost:5050/acme/fork", &[])], &bundle);
        let mut set = declared(&[("m", "localhost:5050/acme/fork:1")]);
        let pack = Identifier::parse("localhost:5050/acme/pack:1").unwrap();
        set.bundles.insert(
            "pack".to_string(),
            crate::config::declaration::DeclaredSource::Registry(pack),
        );
        set.invalidate_declaration_hash_cache();

        let upstream = row(ArtifactKind::Skill, "m", false, "localhost:5050/acme/m");
        assert_eq!(row_binding(&set, &lock, upstream), None);
        let member = row(ArtifactKind::Skill, "m", true, "localhost:5050/acme/m");
        assert_eq!(row_binding(&set, &lock, member).as_deref(), Some("m"));
        assert!(member.declared_by_name(&set), "an override stays without a relock");
    }

    /// A lock row no declaration reaches, as an old TUI install left one under an
    /// intact hash, holds nothing: `grim update` prunes it.
    #[test]
    fn a_ghost_row_holds_no_row() {
        let lock = lock_of(
            &[("s", "localhost:5050/acme/s", &[]), ("o", "localhost:5050/acme/o", &[])],
            "",
        );
        let set = declared(&[("s", "localhost:5050/acme/s:1")]);
        let held = |exact| {
            row_binding(
                &set,
                &lock,
                row(ArtifactKind::Skill, "o", exact, "localhost:5050/acme/o"),
            )
        };

        assert_eq!(held(false), None);
        assert_eq!(held(true), None, "a member's own name holds no ghost either");
    }

    /// A hand repointed `y`: its old row says nothing of what the declaration reaches.
    /// A member stays bound.
    #[test]
    fn a_repointed_declaration_holds_no_row_at_its_old_repository() {
        let lock = lock_of(&[("y", "localhost:5050/acme/old", &[])], "");
        let set = declared(&[("y", "localhost:5050/acme/new:1")]);
        let held = |exact| {
            row_binding(
                &set,
                &lock,
                row(ArtifactKind::Skill, "y", exact, "localhost:5050/acme/old"),
            )
        };

        assert_eq!(held(false), None);
        assert_eq!(held(true).as_deref(), Some("y"));
    }
}
