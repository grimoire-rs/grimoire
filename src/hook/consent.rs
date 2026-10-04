// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Workspace-scoped hook consent: the machine-local record that says *"hooks
//! declared by this checkout may arm"*, and the pure predicate that reads it.
//!
//! ## What this replaces, and why the replacement is a restoration
//!
//! Until now arming was gated on **registry-scoped trust** — a `trust_hooks`
//! field on a `[[registries]]` entry in global config. That gate answers *which
//! publisher's code may run*. It never answered *which checkout may arm hooks at
//! all*, and that is attacker **T3**, whose entry in the threat model is one
//! sentence long: *"The user is **not** vouching for a repo by cloning it."*
//!
//! The gap was specified once and then lost. `adr_hooks_support.md` Decision E
//! point 4 bound approval to a tuple that *"names a directory, not a scope
//! kind"*, citing `direnv/direnv#83` — direnv shipped content-only trust and had
//! to fix it to path + content, because an approved `.envrc` copied into a
//! hostile directory executed. **Amendment A2 deleted that clause as
//! collateral.** A2's motive was *"no one wants to review every hook"*, which
//! argues against the *digest* half of the key; it took the *directory* half
//! with it and put a publisher where the directory had been. The two are
//! orthogonal, and deleting both left T3 covered by nothing.
//!
//! This module keeps A2's coarseness — no per-hook prompt, no digest key, no
//! approval store, no hash chain — and restores the directory binding. Which is
//! also Claude Code's own folder-level trust, dismissed as *"coarser"* by the
//! ADR's Key insight 6 **before** A2 chose coarseness deliberately.
//!
//! ## The record is machine-local, and that is invariant I1, not a preference
//!
//! `$GRIM_HOME/hooks/consent/<workspace-key>.json`. Nothing armable lives inside
//! a repository, and an answer to *"may this repo arm code?"* stored **in that
//! repo** is the purest form of the thing I1 forbids. There is deliberately no
//! environment form of the record's location, for the same CWE-426 reason
//! `GRIM_ALLOW_HOOKS` does not exist: a repository routinely carries its own
//! environment (`.envrc`, `.mise.toml`, devcontainer `containerEnv`), so an
//! env-settable consent path would let a repo hand itself consent.
//!
//! ## Five details borrowed from OCX's stamp, each load-bearing
//!
//! The sibling OCX project shipped this shape first
//! (`crates/ocx_lib/src/project/consent.rs`); these are the parts that look like
//! details and are not:
//!
//! 1. **The record's `workspace` is the identity; the filename is an index.**
//!    The key is a hash, and a hash is a lookup convenience. A record filed under
//!    this key naming a *different* directory is not consent for this one.
//! 2. **Every field is required — no `#[serde(default)]`, `deny_unknown_fields`.**
//!    A truncated record must not deserialize into a valid-looking one.
//! 3. **An unusable record is an absent record.** Unknown version, parse error,
//!    I/O error — all `None`, logged at debug, never warned, never an error
//!    (**I3**).
//! 4. **The write seam is a closed allowlist** — see [`record`].
//! 5. **Drift re-gates.** Consent is over a *set*, and a declaration that grows
//!    past it is a new question.
//!
//! ## What this does not defend against, stated once
//!
//! The consented set carries no tag and no digest, so a hostile upstream that
//! owns a repository the user already consented to can publish a **new version**
//! of that hook without a re-prompt. That is **T1**, and grim's answer to T1 is
//! the digest pin, not consent (`adr_artifact_trust_model.md` decisions 3 and 4)
//! — the bump rewrites `grimoire.lock`, which is visible in `git diff` and in
//! `grim status`. **I5**: evidence, not prevention. Say it that way.
//!
//! Adding the digest to the set would close it and would also re-prompt on every
//! routine version bump, which is exactly the habituation A2 reversed D5 to
//! avoid. The trade is deliberate.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

use crate::config::scope::ConfigScope;
use crate::install::hook_dispatch;
use crate::lock::locked_source::LockedSource;

/// Timestamp format for [`ConsentRecord::consented_at`] — RFC 3339, UTC,
/// second precision. Provenance for a human reading the file; nothing reads it
/// back, and in particular consent **never expires**.
const CONSENTED_AT_FORMAT: &str = "%Y-%m-%dT%H:%M:%SZ";

/// The on-disk schema version.
///
/// A `serde_repr` enum rather than an integer field, following
/// [`crate::catalog::vote_store`]: an unknown version fails deserialization on
/// its own, so a record written by a newer grim is discarded as unknown rather
/// than read under the wrong rules. Discarded means **not consented**, which is
/// the fail-safe direction.
#[derive(Serialize_repr, Deserialize_repr, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum ConsentVersion {
    #[default]
    V1 = 1,
}

/// One workspace's recorded consent.
///
/// `deny_unknown_fields` **and** no field defaults: both halves are the contract.
/// Without the second, a zero-length or truncated file deserializes into a record
/// with an empty `workspace` and an empty `hooks` set — which
/// [`evaluate`] would then compare against, and an empty declared set is a
/// subset of an empty consented set. A truncated file would grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentRecord {
    /// Schema version. An unrecognized value makes the whole record unusable,
    /// which is the same as absent.
    pub v: ConsentVersion,
    /// The workspace this record consents to — **the identity**, compared by
    /// [`evaluate`]. Not merely the thing the filename hashes.
    pub workspace: PathBuf,
    /// The consented hook set, as [`consent_key`] spells it.
    pub hooks: BTreeSet<String>,
    /// RFC 3339 UTC instant the record was written. Provenance only.
    pub consented_at: String,
}

/// The consent answer for one workspace. Produced by [`evaluate`], which is pure.
///
/// Closed internal enum: the binary is the only consumer, so matches stay total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Consent {
    /// A record names this workspace and covers everything it declares.
    Granted,
    /// A record names this workspace but the declaration has **grown past it**.
    /// Carries exactly the entries that are new, so the report can name them —
    /// "something changed" is not an actionable sentence.
    Drifted(BTreeSet<String>),
    /// No usable record for this workspace. Indistinguishable from an
    /// unreadable one by design (detail 3 in the module doc).
    Absent,
}

/// What [`record`] did.
///
/// Both variants are a success. Global scope needing no record is the correct
/// outcome, not a failure to write, which is why they are told apart rather than
/// collapsed into `()` — `grim hook allow` reports them differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    /// A record was written for this workspace.
    Stamped,
    /// The scope is global. `$GRIM_HOME` is always consented and never carries
    /// a record; nothing was written.
    GlobalNeedsNoRecord,
}

/// What [`revoke`] did. [`Revoked::Absent`] is **not** an error: revoking what
/// was never granted leaves exactly the state the caller asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revoked {
    /// A record existed and was removed.
    Removed,
    /// There was no record to remove.
    Absent,
}

/// How one declared hook is spelled inside [`ConsentRecord::hooks`]:
/// `<binding>@<registry>/<repository>`. **No tag, no digest** — see the module
/// doc's closing note for what that trades away and why.
///
/// `None` for a source with no registry pin. A path-sourced hook has no
/// publisher identity to consent to and cannot arm on any path anyway, so it
/// contributes nothing to the set and never causes drift.
///
/// The **binding** is part of the key, not just the repository: a hostile
/// upstream adding a *second* hook from a repository the user already consented
/// to would otherwise arm with no re-prompt. That is one `BTreeSet<String>`
/// either way, so the stricter spelling is free.
pub fn consent_key(binding: &str, source: &LockedSource) -> Option<String> {
    let pinned = source.pinned()?;
    Some(format!("{binding}@{}/{}", pinned.registry(), pinned.repository()))
}

/// Resolve consent for one workspace over what it declares. **Pure** — no I/O,
/// no clock, no environment, so every row below is a unit test.
///
/// `record` is `None` both when no record exists and when the one on disk is
/// unusable; the two are indistinguishable here by design.
///
/// The `workspace` comparison is what makes the filename a mere index. It is a
/// byte comparison of the stored path against the resolved one — deliberately
/// not a canonicalization, which would need I/O and would make this impure. Two
/// spellings of one directory therefore re-gate, which is the fail-safe
/// direction; the dangerous direction (two *different* directories reading as
/// one) is what the comparison prevents.
pub fn evaluate(record: Option<&ConsentRecord>, workspace: &Path, declared: &BTreeSet<String>) -> Consent {
    let Some(record) = record.filter(|r| r.workspace == workspace) else {
        return Consent::Absent;
    };
    let new: BTreeSet<String> = declared.difference(&record.hooks).cloned().collect();
    if new.is_empty() {
        Consent::Granted
    } else {
        Consent::Drifted(new)
    }
}

/// Read the record for `workspace`, or `None` if there is not a usable one.
///
/// Returns `None` on **every** failure — absent file, I/O error, malformed JSON,
/// unknown field, unrecognized version — logged at debug and never warned. An
/// unusable record is an absent record (**I3**).
#[must_use]
pub fn load(grim_home: &Path, workspace: &Path) -> Option<ConsentRecord> {
    load_at(&hook_dispatch::consent_path(grim_home, workspace))
}

/// [`load`] against an explicit path.
fn load_at(path: &Path) -> Option<ConsentRecord> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice::<ConsentRecord>(&bytes) {
        Ok(record) => Some(record),
        Err(err) => {
            tracing::debug!(
                "discarding an unusable hook consent record at {}: {err}",
                path.display()
            );
            None
        }
    }
}

/// Record consent for `workspace` over `hooks`.
///
/// # The write seam is a closed allowlist, stated as a negative contract
///
/// The **only** callers permitted to reach this function are:
///
/// - `grim hook allow` — the explicit gesture;
/// - `grim add` — typing a reference *is* the declaration gesture
///   (`adr_artifact_trust_model.md` decision 1);
/// - an **accepted** interactive prompt.
///
/// No command writes here on its own initiative. `grim install` and
/// `grim update` *do* reach this function — but only through the third bullet,
/// an accepted prompt, and only when there was a terminal to ask on. A
/// non-interactive run of either writes nothing, which is the state every CI
/// run and every fresh clone is in. `grim lock`, `grim status`,
/// `grim context`, `grim hook list`, `grim hook run`, the TUI and the MCP
/// server never reach it at all.
///
/// **This is the T3 control**: `grim install` materializes what is already
/// declared, and a cloned repository's `grimoire.toml` is not the user's
/// gesture. Enforcement is `test/tests/test_hook_consent.py`, not visibility —
/// the permitted callers live in `crate::command`, so this cannot be narrowed
/// and still compile.
///
/// Never wire this into a shared loader. `grim install` and `grim status` reach
/// the same scope-resolution seam a permitted caller does, so a blanket write
/// there would silently grant consent from a read-only command.
///
/// Returns [`Recorded::GlobalNeedsNoRecord`] for [`ConfigScope::Global`], which
/// is permanently outside this control: `$GRIM_HOME/grimoire.toml` is the user's
/// own file on the user's own machine, T3 does not reach it, and there is no
/// third party's checkout to gate. The guard lives here rather than at each
/// caller because this is the one point every write routes through — which is
/// what makes *"nothing ever writes a global record"* a testable invariant
/// rather than a convention.
///
/// # Errors
///
/// Any I/O error creating the directory or writing the record.
pub fn record(
    grim_home: &Path,
    scope: ConfigScope,
    workspace: &Path,
    hooks: &BTreeSet<String>,
) -> io::Result<Recorded> {
    if scope == ConfigScope::Global {
        return Ok(Recorded::GlobalNeedsNoRecord);
    }
    hook_dispatch::ensure_consent_dir(grim_home)?;
    let record = ConsentRecord {
        v: ConsentVersion::V1,
        workspace: workspace.to_path_buf(),
        hooks: hooks.clone(),
        consented_at: chrono::Utc::now().format(CONSENTED_AT_FORMAT).to_string(),
    };
    let bytes = serde_json::to_vec_pretty(&record).map_err(io::Error::other)?;
    // No advisory lock, and none is needed: one file per workspace means there
    // is no read-modify-write and no shared document to corrupt. Two concurrent
    // `grim hook allow` runs in the same workspace race to write the same
    // answer.
    crate::store::atomic_write(&hook_dispatch::consent_path(grim_home, workspace), &bytes)?;
    Ok(Recorded::Stamped)
}

/// Delete `workspace`'s consent record, if it has one.
///
/// Immediately effective: [`load`] reads the file on every arming decision, so
/// the next converge disarms unless `--trust-hooks` is passed on that run.
///
/// **Global scope needs no guard here.** [`record`] refuses to write one, so
/// there is nothing to remove and the answer is [`Revoked::Absent`] by
/// construction rather than by a second copy of that predicate.
///
/// # Errors
///
/// The removal's own I/O failure. An absent record is [`Revoked::Absent`], never
/// an error.
pub fn revoke(grim_home: &Path, workspace: &Path) -> io::Result<Revoked> {
    let target = hook_dispatch::consent_path(grim_home, workspace);
    match std::fs::remove_file(&target) {
        Ok(()) => Ok(Revoked::Removed),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Revoked::Absent),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set<const N: usize>(entries: [&str; N]) -> BTreeSet<String> {
        entries.iter().map(|s| (*s).to_string()).collect()
    }

    fn stamped<const N: usize>(workspace: &str, hooks: [&str; N]) -> ConsentRecord {
        ConsentRecord {
            v: ConsentVersion::V1,
            workspace: PathBuf::from(workspace),
            hooks: set(hooks),
            consented_at: "2026-08-28T10:00:00Z".to_string(),
        }
    }

    /// The predicate's whole table, each row isolated.
    #[test]
    fn evaluate_answers_absent_granted_or_drifted() {
        let workspace = Path::new("/w/proj");
        let declared = set(["fmt@ghcr.io/acme/fmt"]);

        assert_eq!(evaluate(None, workspace, &declared), Consent::Absent, "no record");

        let record = stamped("/w/proj", ["fmt@ghcr.io/acme/fmt"]);
        assert_eq!(evaluate(Some(&record), workspace, &declared), Consent::Granted);

        // A superset is still granted: consenting and then *removing* a hook is
        // not a new question.
        assert_eq!(
            evaluate(Some(&record), workspace, &BTreeSet::new()),
            Consent::Granted,
            "a shrunken declaration re-gates nothing"
        );

        // Drift names exactly what is new, never the whole set.
        assert_eq!(
            evaluate(
                Some(&record),
                workspace,
                &set(["fmt@ghcr.io/acme/fmt", "evil@ghcr.io/acme/evil"]),
            ),
            Consent::Drifted(set(["evil@ghcr.io/acme/evil"]))
        );
    }

    /// **The filename is an index; the record's `workspace` is the identity.**
    ///
    /// Without this comparison a record reachable under a workspace's key would
    /// consent for it whatever directory it names — which is the whole property
    /// that makes consenting in a scratch project not arm a production repo
    /// (`direnv/direnv#83`).
    #[test]
    fn a_record_naming_another_workspace_is_not_consent_for_this_one() {
        let record = stamped("/w/other", ["fmt@ghcr.io/acme/fmt"]);
        assert_eq!(
            evaluate(Some(&record), Path::new("/w/proj"), &set(["fmt@ghcr.io/acme/fmt"])),
            Consent::Absent
        );
    }

    /// **A truncated or foreign record must not deserialize into a granting
    /// one.** Every field is required and unknown fields are refused precisely
    /// so an empty `hooks` set can never arrive by omission — an empty consented
    /// set trivially covers an empty declaration, so a zero-length file would
    /// otherwise grant.
    #[test]
    fn an_unusable_record_is_an_absent_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.json");
        let cases: &[(&str, &str)] = &[
            ("", "an empty file"),
            ("{", "truncated JSON"),
            (r#"{"v":1,"workspace":"/w/proj"}"#, "a record missing `hooks`"),
            (
                r#"{"v":1,"hooks":[],"consented_at":"x"}"#,
                "a record missing `workspace`",
            ),
            (
                r#"{"v":99,"workspace":"/w/proj","hooks":[],"consented_at":"x"}"#,
                "a version this grim does not recognize",
            ),
            (
                r#"{"v":1,"workspace":"/w/proj","hooks":[],"consented_at":"x","extra":true}"#,
                "an unknown field",
            ),
        ];
        for (body, why) in cases {
            std::fs::write(&path, body).unwrap();
            assert!(load_at(&path).is_none(), "{why} must read as absent");
        }
        // And the honest positive control, so the cases above are not all
        // failing for some unrelated reason.
        std::fs::write(
            &path,
            r#"{"v":1,"workspace":"/w/proj","hooks":["fmt@r/x"],"consented_at":"x"}"#,
        )
        .unwrap();
        assert_eq!(load_at(&path).unwrap().workspace, PathBuf::from("/w/proj"));
    }

    /// Global scope is permanently outside this control, and the guard lives at
    /// the single write seam so the invariant is testable rather than
    /// conventional.
    #[test]
    fn recording_global_scope_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let grim_home = dir.path();
        let workspace = Path::new("/w/proj");

        assert_eq!(
            record(grim_home, ConfigScope::Global, workspace, &set(["fmt@r/x"])).unwrap(),
            Recorded::GlobalNeedsNoRecord
        );
        assert!(
            !hook_dispatch::consent_dir(grim_home).exists(),
            "a global record must not even create the directory"
        );
        assert_eq!(load(grim_home, workspace), None);
    }

    /// The round trip, and revoke's idempotence.
    #[test]
    fn a_project_record_round_trips_and_revoke_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let grim_home = dir.path();
        let workspace = Path::new("/w/proj");
        let hooks = set(["fmt@ghcr.io/acme/fmt"]);

        assert_eq!(revoke(grim_home, workspace).unwrap(), Revoked::Absent, "nothing yet");
        assert_eq!(
            record(grim_home, ConfigScope::Project, workspace, &hooks).unwrap(),
            Recorded::Stamped
        );

        let loaded = load(grim_home, workspace).expect("written record must read back");
        assert_eq!(loaded.workspace, workspace);
        assert_eq!(loaded.hooks, hooks);
        assert_eq!(evaluate(Some(&loaded), workspace, &hooks), Consent::Granted);

        assert_eq!(revoke(grim_home, workspace).unwrap(), Revoked::Removed);
        assert_eq!(revoke(grim_home, workspace).unwrap(), Revoked::Absent, "twice is fine");
        assert_eq!(load(grim_home, workspace), None);
    }

    /// **Consent in one workspace does not arm another** — the direnv property,
    /// executed against the real on-disk layout rather than asserted about the
    /// predicate alone.
    #[test]
    fn consent_in_one_workspace_does_not_arm_another() {
        let dir = tempfile::tempdir().unwrap();
        let grim_home = dir.path();
        let hooks = set(["fmt@ghcr.io/acme/fmt"]);

        record(grim_home, ConfigScope::Project, Path::new("/w/scratch"), &hooks).unwrap();

        let production = Path::new("/w/production");
        assert_eq!(load(grim_home, production), None);
        assert_eq!(
            evaluate(load(grim_home, production).as_ref(), production, &hooks),
            Consent::Absent
        );
    }
}
