// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim why <kind> <name>` — every chain from a declaration in
//! `grimoire.toml` to one artifact, read from the lock alone.
//!
//! [`Provenance`] is also what `grim status` uses for `direct`,
//! `required_by` and the dependency source literals, so both commands answer
//! from one graph.

use clap::Args;

use crate::api::status_report::RequiredBy;
use crate::api::why_report::{WhyNode, WhyOverride, WhyReport};
use crate::cli::exit_code::ExitCode;
use crate::config::{DeclaredSource, DesiredSet};
use crate::context::Context;
use crate::lock::effective_set::{
    EffectiveGraph, MAX_WHY_PATHS, NodeKey, Origin, WHY_STEP_BUDGET, effective_graph, find_cycle, why_paths,
};
use crate::lock::grimoire_lock::GrimoireLock;
use crate::lock::lock_io;
use crate::oci::member_ref::MemberRef;
use crate::oci::{ArtifactKind, Identifier};
use crate::resolve::resolve_error::{ResolveError, ResolveErrorKind};

use super::command_error::CommandError;
use super::scope_resolution;
use super::status::find_locked;

/// `grim why` arguments.
#[derive(Debug, Args)]
pub struct WhyArgs {
    /// `skill`, `rule`, `agent`, `bundle`, `mcp`, `hook`, or `settings`.
    #[arg(value_parser = ["skill", "rule", "agent", "bundle", "mcp", "hook", "settings"])]
    pub kind: String,

    /// The binding name to explain.
    pub name: String,
}

/// The effective graph of one scope plus the lock it was read from.
pub(super) struct Provenance<'a> {
    lock: Option<&'a GrimoireLock>,
    graph: Option<EffectiveGraph>,
}

impl<'a> Provenance<'a> {
    /// `graph()` is `None` without a lock or when the closure is not
    /// computable offline.
    pub(super) fn new(set: &DesiredSet, lock: Option<&'a GrimoireLock>) -> Self {
        let graph = lock.and_then(|l| effective_graph(set, l));
        Self { lock, graph }
    }

    pub(super) fn graph(&self) -> Option<&EffectiveGraph> {
        self.graph.as_ref()
    }

    /// Immediate parents of `key`, sorted by `(kind, name)`; `None` when the
    /// graph is incomputable.
    pub(super) fn required_by(&self, key: &NodeKey) -> Option<Vec<RequiredBy>> {
        let mut parents: Vec<&NodeKey> = self
            .graph
            .as_ref()?
            .children
            .iter()
            .filter(|(_, kids)| kids.contains(key))
            .map(|(parent, _)| parent)
            .collect();
        parents.sort();
        Some(
            parents
                .into_iter()
                .map(|(kind, name)| RequiredBy {
                    kind: *kind,
                    name: name.clone(),
                })
                .collect(),
        )
    }

    /// The `source` literal of a row reached through requires edges only.
    pub(super) fn dependency_source(&self, key: &NodeKey) -> String {
        let parents: Vec<String> = self
            .required_by(key)
            .unwrap_or_default()
            .iter()
            .filter(|p| p.kind != ArtifactKind::Bundle)
            .map(|p| format!("{}/{}", p.kind, p.name))
            .collect();
        if parents.is_empty() {
            "dependency".to_string()
        } else {
            format!("dependency: {}", parents.join(", "))
        }
    }

    /// The identifier `parent` asked for `child`, resolved against the
    /// parent's own pin; `None` when the lock does not say.
    fn requested_id(&self, parent: &NodeKey, child: &NodeKey) -> Option<Identifier> {
        let lock = self.lock?;
        let (members, parent_id) = if parent.0 == ArtifactKind::Bundle {
            let b = lock.bundles.iter().find(|b| b.name == parent.1)?;
            (&b.members, b.pinned().map(|p| p.as_identifier().clone()))
        } else {
            let a = find_locked(lock, parent.0, &parent.1)?;
            (&a.requires, a.source.pinned().map(|p| p.as_identifier().clone()))
        };
        let member = members.iter().find(|m| m.kind == child.0 && m.name == child.1)?;
        match MemberRef::parse(&member.id).ok()? {
            MemberRef::Absolute(id) => Some(id),
            relative => relative.resolve(&parent_id?).ok(),
        }
    }

    /// Whether `requested` is what the direct declaration of `key` already
    /// resolves to. Offline there is no registry to ask, so this compares
    /// digests when both sides carry one and repository plus tag otherwise.
    fn same_as_direct(&self, requested: &Identifier, key: &NodeKey) -> bool {
        let Some(Origin::Direct(source)) = self.graph().and_then(|g| g.nodes.get(key)) else {
            return false;
        };
        let DeclaredSource::Registry(declared) = source else {
            return false;
        };
        if declared.registry_repository() != requested.registry_repository() {
            return false;
        }
        let locked = self
            .lock
            .and_then(|l| find_locked(l, key.0, &key.1))
            .and_then(|a| a.source.pinned().map(|p| p.digest()));
        if let (Some(asked), Some(have)) = (requested.digest(), locked) {
            return asked == have;
        }
        requested.tag_or_latest() == declared.tag_or_latest()
    }

    /// Non-bundle parents of a direct `key`, split into those asking for what
    /// the declaration already pins and those it overrode.
    fn direct_edges(&self, key: &NodeKey) -> (Vec<NodeKey>, Vec<WhyOverride>) {
        let mut same = Vec::new();
        let mut overrides = Vec::new();
        let Some(graph) = self.graph() else {
            return (same, overrides);
        };
        if !matches!(graph.nodes.get(key), Some(Origin::Direct(_))) {
            return (same, overrides);
        }
        for (parent, kids) in &graph.children {
            if !kids.contains(key) {
                continue;
            }
            let Some(id) = self.requested_id(parent, key) else {
                continue;
            };
            if !self.same_as_direct(&id, key) {
                overrides.push(WhyOverride {
                    kind: parent.0,
                    name: parent.1.clone(),
                    id: id.to_string(),
                });
            } else if parent.0 != ArtifactKind::Bundle {
                same.push(parent.clone());
            }
        }
        overrides.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
        (same, overrides)
    }

    pub(super) fn overrides(&self, key: &NodeKey) -> Vec<WhyOverride> {
        self.direct_edges(key).1
    }

    /// One note per direct artifact that an artifact also requires at the
    /// digest the declaration already pins. Bundle parents never count, so a
    /// project without dependencies prints nothing.
    pub(super) fn redundant_note(&self, key: &NodeKey) -> Option<String> {
        let mut parents = self.direct_edges(key).0;
        if parents.is_empty() {
            return None;
        }
        parents.sort();
        let by: Vec<String> = parents.iter().map(|(k, n)| format!("{k}/{n}")).collect();
        Some(format!(
            "note: {}/{} is declared directly and also required by {} at the same digest; the declaration can be removed",
            key.0,
            key.1,
            by.join(", ")
        ))
    }
}

fn stale(path: &std::path::Path, message: String) -> anyhow::Error {
    crate::error::Error::from(crate::lock::lock_error::LockError::new(
        path.to_path_buf(),
        crate::lock::lock_error::LockErrorKind::UnfollowableGraph { message },
    ))
    .into()
}

fn node(lock: &GrimoireLock, key: &NodeKey) -> WhyNode {
    let pinned = if key.0 == ArtifactKind::Bundle {
        lock.bundles
            .iter()
            .find(|b| b.name == key.1)
            .and_then(|b| b.pinned())
            .map(|p| p.strip_advisory().to_string())
    } else {
        find_locked(lock, key.0, &key.1)
            .and_then(|a| a.source.pinned())
            .map(|p| p.strip_advisory().to_string())
    };
    WhyNode {
        kind: key.0,
        name: key.1.clone(),
        pinned,
    }
}

/// Run `grim why`.
///
/// # Errors
///
/// No project (79), no lock (79), a target outside the effective set (79),
/// an incomputable closure or a cycle in the lock (65 `stale-lock`).
pub fn run(ctx: &Context, args: &WhyArgs) -> anyhow::Result<(WhyReport, ExitCode)> {
    let kind = ArtifactKind::from_kind_str(&args.kind).ok_or_else(|| {
        crate::error::Error::from(CommandError::ConfigUsage(format!(
            "unknown artifact kind '{}'",
            args.kind
        )))
    })?;
    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;
    let lock = match lock_io::load(&scope.lock_path) {
        Ok(l) => l,
        Err(e) if e.is_not_found() => {
            return Err(crate::error::Error::from(CommandError::LockMissing {
                path: scope.lock_path.clone(),
            })
            .into());
        }
        Err(e) => return Err(crate::error::Error::from(e).into()),
    };

    let prov = Provenance::new(&scope.set, Some(&lock));
    let Some(graph) = prov.graph() else {
        return Err(stale(
            &scope.lock_path,
            "the lock's dependency graph cannot be computed offline; run `grim lock`".to_string(),
        ));
    };
    if let Some(cycle) = find_cycle(graph) {
        let chain: Vec<String> = cycle.iter().map(|(k, n)| format!("{k}/{n}")).collect();
        return Err(stale(
            &scope.lock_path,
            format!(
                "the lock's dependency graph has a cycle: {}; run `grim lock`",
                chain.join(" -> ")
            ),
        ));
    }

    let key: NodeKey = (kind, args.name.clone());
    let declared_bundle = kind == ArtifactKind::Bundle && scope.set.bundles.contains_key(&args.name);
    let origin = graph.nodes.get(&key);
    if origin.is_none() && !declared_bundle {
        return Err(crate::error::Error::from(ResolveError::unidentified(
            kind,
            args.name.clone(),
            ResolveErrorKind::NotDeclared,
        ))
        .into());
    }
    let direct = declared_bundle || matches!(origin, Some(Origin::Direct(_)));

    let found = why_paths(graph, &key, MAX_WHY_PATHS, WHY_STEP_BUDGET);
    let paths: Vec<Vec<WhyNode>> = found
        .paths
        .iter()
        .map(|p| p.iter().map(|k| node(&lock, k)).collect())
        .collect();
    let report = WhyReport {
        kind,
        name: args.name.clone(),
        pinned: node(&lock, &key).pinned,
        direct,
        paths,
        path_count: found.path_count,
        truncated: found.truncated,
        overrides: prov.overrides(&key),
        notes: prov.redundant_note(&key).into_iter().collect(),
    };
    Ok((report, ExitCode::Success))
}
