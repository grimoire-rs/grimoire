// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Lock and declaration fixtures shared by tests across modules.

use std::collections::BTreeMap;

use crate::config::{DeclaredSource, DesiredSet};
use crate::lock::grimoire_lock::GrimoireLock;
use crate::oci::Identifier;

pub(crate) fn sha(c: char) -> String {
    c.to_string().repeat(64)
}

/// A lock whose skills carry `requires`: `(name, repo, [(kind, name, id)])`.
pub(crate) type LockedSkill<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str, &'a str)]);

pub(crate) fn lock_of(skills: &[LockedSkill], bundles: &str) -> GrimoireLock {
    let mut out = format!(
        "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\ndeclaration_hash = \"sha256:{}\"\n\
         generated_by = \"grim test\"\ngenerated_at = \"2026-01-01T00:00:00Z\"\n\n",
        sha('d')
    );
    for (name, repo, requires) in skills {
        out.push_str(&format!(
            "[[skill]]\nname = \"{name}\"\npinned = \"{repo}@sha256:{}\"\n\n",
            sha('a')
        ));
        for (kind, rname, id) in *requires {
            out.push_str(&format!(
                "[[skill.requires]]\nkind = \"{kind}\"\nname = \"{rname}\"\nid = \"{id}\"\n\n"
            ));
        }
    }
    out.push_str(bundles);
    GrimoireLock::from_toml_str(&out).expect("test lock parses")
}

pub(crate) fn declared(skills: &[(&str, &str)]) -> DesiredSet {
    let skills: BTreeMap<String, DeclaredSource> = skills
        .iter()
        .map(|(n, i)| {
            (
                (*n).to_string(),
                DeclaredSource::Registry(Identifier::parse(i).unwrap()),
            )
        })
        .collect();
    DesiredSet::from_parts(skills, BTreeMap::new())
}
