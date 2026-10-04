// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim hook list` (S-015) — the user-facing hook inventory.
//!
//! An ordinary report command in every respect: it resolves a scope, reads the
//! declared set, and renders a [`HookListReport`] through `Printable` so
//! `--format json` and the plain table come from one source. It is the
//! *supported* half of `grim hook`; [`super::run`] is grim's own generated
//! caller's entry point and is documented as not intended for direct
//! invocation.
//!
//! **This module is deliberately exempt from the runtime's import ban.** The
//! source-level test in [`super`] forbids configuration, scope resolution, the
//! per-invocation context and the environment data root inside the dispatch
//! path; a report command whose whole job is to describe the resolved scope
//! needs all four. Keeping the two in separate files is what lets one be
//! checked and the other be useful.
//!
//! ## Why this reports the same tokens `grim status` does
//!
//! The per-client arming verdicts are `HookArmingCause` values with
//! `ArtifactStatus` tokens — the same vocabulary `grim status` renders, from
//! the same enum. C-017's point is that a refusal names one cause with one
//! remedy; a second vocabulary for the same facts is how two commands come to
//! describe one hook differently, and the user cannot tell which is right.
//!
//! That is a statement about the *derivation*, not only the vocabulary: every
//! verdict here is produced by [`status::hook_arming`] over
//! [`status::HookArmingInputs`], and the fallback lifecycle token by
//! [`status::derive_state`] — the seams `grim status` itself calls. Nothing in
//! this file re-derives the feature flag, the workspace consent decision, the
//! `$GRIM_HOME` refusal, or the dispatch table's arming authority. A second derivation of those gates is exactly how the two
//! commands would come to disagree about one hook.
//!
//! ## Where the entries come from
//!
//! The `[[hooks]]` entries are read from the artifact's **payload directory**,
//! whose location is derived from `$GRIM_HOME` and the resolved scope through
//! [`hook_dispatch::payload_dir`] — never from the install record. A record is
//! attacker-supplied in the cloned-repository case, so a payload directory read
//! out of one is a directory the attacker chose (SEC-1); convergence derives it
//! the same way and this command must not be the second, weaker route to the
//! same bytes.
//!
//! A payload that is absent or whose `hook.toml` no longer parses **degrades**:
//! one warning naming the artifact, no items for it, exit 0. A gated hook is
//! skipped before its blob is ever fetched (S-001), so "no payload" is the
//! ordinary state of a declared-but-never-armed hook, not a failure — and a
//! report command that refused to render because one artifact's manifest is
//! missing would violate invariant I3.

use std::collections::BTreeSet;

use clap::Args;

use crate::api::artifact_status::ArtifactStatus;
use crate::api::hook_report::{HookListEntry, HookListReport};
use crate::api::status_report::HookArming;
use crate::cli::exit_code::ExitCode;
use crate::context::Context;
use crate::install::hook_dispatch::{self, RootScope};
use crate::install::hook_registrar::root_scope_for;
use crate::install::target::{InstallTarget, detect_clients_or_all};
use crate::lock::lock_io;
use crate::oci::ArtifactKind;
use crate::oci::hook::{HOOK_MANIFEST_FILE, HookEntry, HookManifest};

use super::super::{scope_resolution, status};

/// `grim hook list` arguments.
///
/// No flags yet. Scope comes from the root `--global` / `--config` options,
/// exactly as it does for every other scope-aware command, so there is
/// nothing to declare here — the struct exists so the subcommand can grow one
/// additively without changing its shape.
#[derive(Debug, Args)]
pub struct ListArgs {}

/// One declared hook artifact, with everything the per-entry rows need that is
/// a property of the *artifact* rather than of a `[[hooks]]` entry.
///
/// Assembled by [`run`] from the resolved scope and consumed by
/// [`assemble`], which is the half that reads the filesystem — so the
/// filesystem half is testable without a `Context`, a lock, or an install
/// state.
struct HookSubject {
    /// The config binding name, which is also the payload directory's stem.
    name: String,
    /// The per-client arming verdicts, already produced by
    /// [`status::hook_arming`]. Empty means armed on every configured client.
    arming: Vec<HookArming>,
    /// The materialization lifecycle token from [`status::derive_state`], used
    /// only when the arming verdicts imply no state of their own.
    lifecycle: ArtifactStatus,
    /// The clients an install recorded an output for — the evidence that
    /// convergence ran for this artifact on that client, and therefore that a
    /// missing dispatch row is a refusal rather than a not-yet-installed hook.
    /// Rule 1 of [`status::merge_not_registered`].
    installed: BTreeSet<String>,
}

/// List every declared hook entry with its tier, events, and per-client
/// arming state.
///
/// # Errors
///
/// A scope that cannot be resolved, a configuration or lock file that does not
/// parse, an unreadable install state, or an invalid configured client name —
/// the ordinary report-command failures, classified into the ordinary exit
/// codes. Unlike [`super::run`] this command *may* fail: it is not on any
/// client's hot path, so an honest error is better than an empty answer.
///
/// A **payload** that cannot be read is deliberately not in that list — see the
/// module doc.
pub async fn run(ctx: &Context, _args: &ListArgs) -> anyhow::Result<(HookListReport, ExitCode)> {
    let scope = super::super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;

    // Same tolerance `grim status` has: an absent lock means nothing is pinned
    // yet, a corrupt one is a load failure (78) and propagates.
    let lock = match lock_io::load(&scope.lock_path) {
        Ok(l) => Some(l),
        Err(e) if e.is_not_found() => None,
        Err(e) => return Err(crate::error::Error::from(e).into()),
    };

    // Nothing declared and nothing locked ⇒ nothing to report, and no reason to
    // pay for the global-config read `HookArmingInputs::resolve` performs. This
    // is the *only* path on which an empty report is the correct answer.
    if !status::declares_a_hook(&scope, lock.as_ref()) {
        tracing::debug!("grim hook list: no hooks are declared for this scope");
        return Ok((HookListReport::new(Vec::new()), ExitCode::Success));
    }

    let state = super::super::grim(
        scope_resolution::load_state(&scope).map_err(|e| super::super::install::state_io(&scope.state_path, e)),
    )?;
    let target = super::super::grim(InstallTarget::parse(
        &scope.workspace,
        scope.scope,
        &[],
        &scope.options.clients,
        &scope.options.vendors,
    ))?;
    let active = detect_clients_or_all(&scope.workspace, scope.scope);
    let lock_matches_config =
        lock.as_ref().map(|l| l.metadata.declaration_hash.as_str()) == Some(scope.set.declaration_hash_cached());

    let inputs = status::HookArmingInputs::resolve(ctx, &scope, &target, lock.as_ref())?;

    // Directly-declared and bundle-provided hooks in one set: a bundle member
    // appears in no `[hooks]` table, and omitting it would report an unarmed
    // bundle hook as absent entirely. `BTreeSet` both dedups the overlap and
    // fixes the artifact order.
    let mut names: BTreeSet<String> = scope.set.hooks.keys().cloned().collect();
    if let Some(l) = lock.as_ref() {
        names.extend(
            l.iter_artifacts()
                .filter(|a| a.kind == ArtifactKind::Hook)
                .map(|a| a.name.clone()),
        );
    }

    let subjects: Vec<HookSubject> = names
        .into_iter()
        .map(|name| {
            let locked = lock
                .as_ref()
                .and_then(|l| status::find_locked(l, ArtifactKind::Hook, &name));
            // C-022 keys on the **resolved** registry and repository, never on
            // the reference the user typed (B5.4), so the pin is what the trust
            // gate is asked about.
            let pinned = locked.and_then(|l| l.source.pinned().cloned());
            let arming = status::hook_arming(ArtifactKind::Hook, &name, pinned.as_ref(), Some(&inputs));
            let lifecycle = status::derive_state(
                ArtifactKind::Hook,
                &name,
                locked,
                &state,
                &scope.roots,
                &active,
                lock_matches_config,
            );
            let installed = state
                .get(ArtifactKind::Hook, &name)
                .map(|record| record.outputs.iter().map(|o| o.client.clone()).collect())
                .unwrap_or_default();
            HookSubject {
                name,
                arming,
                lifecycle,
                installed,
            }
        })
        .collect();

    let root = root_scope_for(&scope.workspace, scope.scope);
    let items = assemble(ctx.grim_home(), root, subjects, Some(&inputs)).await;
    Ok((HookListReport::new(items), ExitCode::Success))
}

/// Read each subject's `hook.toml` and expand it into one report item per
/// `[[hooks]]` entry.
///
/// Ordered by `(artifact, id)`: the subjects arrive in artifact order already,
/// and a manifest's own entry order is the author's, not a sort key a consumer
/// could rely on across a re-publish.
///
/// Never fails. Each unreadable manifest costs one warning and its own
/// artifact's rows; every other artifact still reports (I3).
async fn assemble(
    grim_home: &std::path::Path,
    root: RootScope<'_>,
    subjects: Vec<HookSubject>,
    inputs: Option<&status::HookArmingInputs>,
) -> Vec<HookListEntry> {
    let mut items = Vec::new();
    for subject in subjects {
        let mut entries = match read_manifest(grim_home, root, &subject.name).await {
            Ok(manifest) => manifest.hooks,
            Err(e) => {
                // Warn, not debug: `grim hook list` is a verb a human types, and
                // an artifact silently absent from the answer is the failure
                // this command exists to stop. A gated hook is never
                // materialized (S-001), so this is also the ordinary path for
                // one — hence the state token in the line.
                tracing::warn!(
                    "hook '{}' ({}): no readable manifest, so its entries cannot be listed: {e:#}",
                    subject.name,
                    subject.lifecycle,
                );
                continue;
            }
        };
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        for entry in entries {
            // Per **entry**, not per artifact, and that is P-1's reporting half.
            // `hook_arming` answers at artifact granularity, so one artifact with
            // one registered and one declined entry reported both as armed. The
            // dispatch table is the arming authority and it is keyed per entry,
            // so the refusal is legible here — and only here, because this is the
            // only surface with an entry dimension to put it on.
            let arming = match inputs {
                Some(inputs) => status::merge_not_registered(subject.arming.clone(), &subject.installed, |client| {
                    inputs.arms_entry(client, &subject.name, &entry.id)
                }),
                None => subject.arming.clone(),
            };
            // The roll-up: an arming verdict outranks the lifecycle token for the
            // same reason it does on a `grim status` row — a hook whose payload is
            // intact and whose registration was refused is `not-armed`, not
            // `installed`.
            let state = status::hook_row_state(&arming).unwrap_or(subject.lifecycle);
            items.push(HookListEntry {
                artifact: subject.name.clone(),
                id: entry.id.clone(),
                tier: entry.tier,
                events: events_of(&entry),
                state,
                arming,
            });
        }
    }
    items
}

/// One artifact's `hook.toml`, read from the payload directory derived from
/// `$GRIM_HOME` and the resolved scope.
///
/// **`name` comes out of an install record, so it is untrusted here** (round-2
/// S1). `binding_name_refusal` keeps a traversing name out of new records, and
/// `hook_registrar::desired_entries` re-checks the same path through
/// `Containment::Strict` anyway — this was the third consumer of
/// `payload_dir(…, record.name)` and the only one doing neither, so a
/// hand-edited or pre-fix state file could point it at an arbitrary file and have
/// it parsed as `hook.toml`. A read rather than a write, which is why it is a
/// hardening rather than a fix, and exactly why it should match its siblings.
async fn read_manifest(grim_home: &std::path::Path, root: RootScope<'_>, name: &str) -> anyhow::Result<HookManifest> {
    // The grammar gate rather than `Containment::Strict`: this function has no
    // `AnchorRoots` and threading one down from the command entry would be
    // plumbing for a weaker guarantee. `binding_name_refusal` makes a traversing
    // name *unrepresentable*, which is the stronger control for exactly this
    // threat — a name that cannot contain a separator cannot escape whatever it
    // is joined onto.
    if let Some(reason) = crate::oci::hook::binding_name_refusal(name) {
        anyhow::bail!("hook '{name}' has no usable payload directory: {reason}");
    }
    let path = hook_dispatch::payload_dir(grim_home, root, name).join(HOOK_MANIFEST_FILE);
    let source = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    Ok(HookManifest::from_toml_str(&source)?)
}

/// Every moment one entry names.
///
/// The canonical event first, then each `<vendor>.event` native moment as
/// `<vendor>.<native>` — qualified, because a native event name is one vendor's
/// vocabulary and an unqualified one would read as a canonical event grim does
/// not have. `BTreeMap` iteration fixes the vendor order.
///
/// Never empty in practice: `grim build` rejects an entry that names no moment
/// at all. It is not asserted here, because a report command must render what
/// is on disk rather than argue with it.
fn events_of(entry: &HookEntry) -> Vec<String> {
    let mut events = Vec::new();
    if let Some(event) = entry.event {
        events.push(event.as_str().to_string());
    }
    for (vendor, table) in &entry.vendor {
        if let Some(native) = table
            .as_object()
            .and_then(|t| t.get("event"))
            .and_then(serde_json::Value::as_str)
        {
            events.push(format!("{vendor}.{native}"));
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::artifact_status::HookArmingCause;
    use crate::oci::hook::HookTier;

    const TWO_ENTRIES: &str = r#"
schema = 1
name = "shell-guard"
description = "Observes and gates Bash tool calls."

[[hooks]]
id = "post"
event = "PostToolUse"
tier = "observer"
command = "sh observe.sh"

[[hooks]]
id = "pre"
event = "PreToolUse"
tier = "gatekeeper"
matcher = "Bash"
command = "sh guard.sh"
"#;

    /// Materialize `name`'s payload the way an install does — through the same
    /// `$GRIM_HOME`-derived path the command reads back.
    fn plant(grim_home: &std::path::Path, root: RootScope<'_>, name: &str, manifest: &str) {
        let dir = hook_dispatch::payload_dir(grim_home, root, name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(HOOK_MANIFEST_FILE), manifest).unwrap();
    }

    fn subject(name: &str, arming: Vec<HookArming>, lifecycle: ArtifactStatus) -> HookSubject {
        HookSubject {
            name: name.to_string(),
            arming,
            lifecycle,
            // No `HookArmingInputs` in these tests, so the merge never runs and
            // the set is never read — see each `assemble` call's `None`.
            installed: BTreeSet::new(),
        }
    }

    fn arming(client: &str, cause: HookArmingCause) -> HookArming {
        HookArming {
            client: client.to_string(),
            cause,
            message: cause.message().to_string(),
            transient: cause.transient(),
        }
    }

    /// The defect this command shipped with: an installed, armed hook produced
    /// **no** items at all. One `[[hooks]]` entry ⇒ one item, carrying the
    /// entry's own tier and event.
    #[tokio::test]
    async fn every_manifest_entry_becomes_one_item() {
        let home = tempfile::tempdir().unwrap();
        plant(home.path(), RootScope::Global, "shell-guard", TWO_ENTRIES);

        let items = assemble(
            home.path(),
            RootScope::Global,
            vec![subject("shell-guard", Vec::new(), ArtifactStatus::Installed)],
            None,
        )
        .await;

        assert_eq!(
            items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            ["post", "pre"],
            "one item per [[hooks]] entry, ordered by id: {items:?}"
        );
        assert_eq!(items[0].tier, HookTier::Observer);
        assert_eq!(items[1].tier, HookTier::Gatekeeper);
        assert_eq!(items[1].events, ["PreToolUse"]);
        assert!(
            items
                .iter()
                .all(|i| i.arming.is_empty() && i.state == ArtifactStatus::Installed),
            "an armed hook keeps its lifecycle token and reports no per-client verdict: {items:?}"
        );
    }

    /// The arming verdicts are attached verbatim, and the roll-up state comes
    /// from them rather than from the lifecycle — the same precedence a
    /// `grim status` hook row has.
    #[tokio::test]
    async fn an_arming_verdict_outranks_the_lifecycle_token() {
        let home = tempfile::tempdir().unwrap();
        plant(home.path(), RootScope::Global, "shell-guard", TWO_ENTRIES);

        let items = assemble(
            home.path(),
            RootScope::Global,
            vec![subject(
                "shell-guard",
                vec![arming("claude", HookArmingCause::FeatureFlagOff)],
                ArtifactStatus::Installed,
            )],
            None,
        )
        .await;

        assert_eq!(items.len(), 2, "{items:?}");
        for item in &items {
            assert_eq!(item.state, ArtifactStatus::Gated, "{item:?}");
            assert_eq!(
                item.arming.iter().map(|a| a.cause).collect::<Vec<_>>(),
                [HookArmingCause::FeatureFlagOff],
                "the verdict is carried per item, not collapsed into the token"
            );
        }
    }

    /// An artifact whose payload was never materialized (the ordinary state of
    /// a gated hook, S-001) costs its own rows and nothing else — the sibling
    /// still reports. Degrading rather than failing is invariant I3.
    #[tokio::test]
    async fn a_missing_manifest_degrades_without_taking_its_siblings() {
        let home = tempfile::tempdir().unwrap();
        plant(home.path(), RootScope::Global, "shell-guard", TWO_ENTRIES);

        let items = assemble(
            home.path(),
            RootScope::Global,
            vec![
                subject("never-installed", Vec::new(), ArtifactStatus::Gated),
                subject("shell-guard", Vec::new(), ArtifactStatus::Installed),
            ],
            None,
        )
        .await;

        assert_eq!(
            items.iter().map(|i| i.artifact.as_str()).collect::<Vec<_>>(),
            ["shell-guard", "shell-guard"],
            "the readable artifact still reports: {items:?}"
        );
    }

    /// A payload directory is derived from `$GRIM_HOME` **and the scope**, so a
    /// project-scope read never finds a global-scope payload (SEC-1's
    /// derivation, exercised rather than asserted about).
    #[tokio::test]
    async fn the_payload_is_read_from_the_scope_own_root() {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        plant(home.path(), RootScope::Global, "shell-guard", TWO_ENTRIES);

        let items = assemble(
            home.path(),
            RootScope::Workspace(workspace.path()),
            vec![subject("shell-guard", Vec::new(), ArtifactStatus::Installed)],
            None,
        )
        .await;

        assert!(
            items.is_empty(),
            "a workspace root must not read the global payload: {items:?}"
        );
    }

    /// A `<vendor>.event` native moment is listed beside the canonical one, and
    /// qualified by its vendor so it can never read as a canonical event.
    #[test]
    fn a_native_moment_is_listed_qualified_beside_the_canonical_one() {
        let manifest = HookManifest::from_toml_str(
            r#"
schema = 1
name = "shell-guard"
description = "d"

[[hooks]]
id = "pre"
event = "PreToolUse"
tier = "observer"
command = "sh guard.sh"
cursor = { event = "beforeShellExecution" }
"#,
        )
        .unwrap();

        assert_eq!(
            events_of(&manifest.hooks[0]),
            ["PreToolUse", "cursor.beforeShellExecution"]
        );
    }

    /// ⛔ **S4/M5.** `read_manifest` refuses a traversing record name before it
    /// joins it onto `$GRIM_HOME`.
    ///
    /// Round-3 S4 found this guard untested: deleting it left the suite green. The
    /// name here comes from an install **record**, which in the cloned-repository
    /// case is attacker-supplied (SEC-1's class), so `grim hook list` — a
    /// read-only command — would otherwise read a `hook.toml` from anywhere on
    /// disk. The refusal is asserted, not the read: no payload tree is staged, so
    /// a guard that let the name through would fail differently (a missing file),
    /// which is exactly the distinction the message assertion pins.
    #[test]
    fn read_manifest_refuses_a_traversing_record_name() {
        let home = tempfile::tempdir().expect("tempdir");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");

        for bad in ["../../../../etc", "..", "a/b", "bin", "dispatch.json.lock"] {
            let err = rt
                .block_on(read_manifest(home.path(), RootScope::Global, bad))
                .expect_err("a name with no usable payload directory must be refused");
            let msg = err.to_string();
            assert!(
                msg.contains("has no usable payload directory"),
                "{bad:?} must be refused by the grammar/reserved gate, not by a later file \
                 read — got: {msg}"
            );
        }
    }
}
