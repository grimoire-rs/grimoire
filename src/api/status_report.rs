// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim status` output.
//!
//! Plain format: 5-column table (Kind | Name | Source | Pinned | State), plus
//! a sixth `Note` column only when a `hook` or `settings` row is present — so
//! other output is unchanged. `Note` is `-` for every other row; for those two
//! it names the client and the cause behind a `gated` / `not-armed` /
//! `untrusted` state, or a note beside a settings row's lifecycle state. Plain
//! output carries no compatibility promise
//! (`docs/src/stability.md` § Unstable — "human-readable log or error text"),
//! unlike the JSON shape below.
//!
//! JSON format: `{"items": [...]}` where each item is a
//! `{kind, name, source, pinned, state, outputs}` object (uniform `items`
//! envelope, per subsystem-cli-api.md). `pinned` is `null` when the
//! artifact is declared but not yet locked. `source` is `"direct"` for a
//! declared artifact or `"bundle: <registry/repo>"` for a bundle member.
//! `outputs` is an array of `{client, path, element}` — the per-client
//! materialized locations recorded in install state, reconciled against the
//! currently-active client set. Always present; empty for an artifact with
//! no recorded outputs (not installed, or a bundle row). Vendor on-disk
//! layout is unstable — this field is the supported discovery channel for
//! where an artifact was materialized.
//!
//! `clients_missing` / `clients_extra` report client-set drift, entirely
//! from local state (config + install record) — no network. `desired` is
//! the project's configured client target (`[options].clients`, same seam
//! as `grim context`); `recorded` is the client names on the artifact's
//! install-state record. `clients_missing` is `desired − recorded`
//! (configured but never installed here); `clients_extra` is
//! `recorded − desired` (installed here but dropped from config). Both
//! sorted, both always present, both `[]` when the sets agree — including
//! for a declared-bundle row (never installs itself) and a dev-install row
//! (installed out-of-band via `grim install <path>`, independent of the
//! project's configured client set). A settings row's `clients_missing` is
//! always `[]`: install records a client only for the pieces a package
//! declares, so a client it declares nothing for is not drift. When
//! `[options].clients` is unset (autodetect), there is no explicit target
//! to diff against, so both
//! fields stay `[]` on every item rather than diffing against live client
//! detection — which is not a sound oracle for "grim installed here" in
//! either direction (see [`crate::command::status`]'s `client_drift` for
//! the vendor-by-vendor evidence and the reversal of ADR D5).
//!
//! `checked` (top-level, sibling of `items` — same envelope pattern as
//! `publish`'s `announce`) is `true` only when `--check` was passed **and**
//! the invocation ran online; `false` on a plain `grim status` (no `--check`
//! ⇒ no network, ever) or a `--check` run that is offline (degrades with one
//! stderr warning). This is the consumer rule for the four
//! `--check`-derived item fields below: **`checked == false` implies every one
//! of them is `null` on every item.** `checked == true` means the check ran
//! online across the scope's registries — one registry's catalog refresh
//! failing falls back to *that* registry's cached catalog, so its rows carry
//! the cached values and read `null` only when nothing is cached; `checked`
//! still reports `true` (the attempt was made online; see
//! [`crate::catalog::load_catalog`]'s per-registry fallback).
//!
//! `deprecated` / `replaced_by` mirror `grim search`'s fields of the same
//! name: the publisher's deprecation notice and named successor, matched
//! against the freshly-loaded catalog by `(registry, repository)` for a
//! registry-sourced item (`pinned` is `Some`). `null` for a declared-bundle
//! row, a dev-install row, or a path-sourced item (none carry a registry
//! pin) — and for any item when `checked` is `false`.
//!
//! `update_available` is a fresh per-artifact re-resolution under `--check`
//! (issue #43): for a registry-locked item grim re-resolves the reference
//! that row declares and compares its digest to the lock pin. `true` when
//! the registry is newer, `false` when it matches (or the tag vanished — a
//! completed re-resolve that finds nothing newer). Four row kinds carry a
//! reference: a directly-declared item (what `grimoire.toml` names), a
//! **declared bundle** (its own reference against the `[[bundle]]` snapshot's
//! manifest digest — the only signal an added or dropped member emits, since
//! the members already locked keep their pins), a **bundle member** (the id
//! its bundle listed, which can float independently of the bundle), and a
//! **dependency** (the tag its pin carries, or a tagless pin's recorded edge).
//! `null` when `checked` is `false`, for a row with no lock pin (dev-install /
//! path source / path-sourced bundle) or no recorded reference, or when the
//! re-resolution failed — absence never lies as `false`. The answer is that
//! row's own: `grim update -- <name>` moves that pin and clears it. A **nested
//! bundle** has no row, so `moved_nested_bundles` carries it, as
//! [`MovedNestedBundle`] says.
//!
//! `arming` (C-017) is the per-client verdict array of a `hook` or `settings`
//! row: `[{client, cause, message, transient}]`, sorted by client, **always
//! present**, `[]` for every other kind and for a row with nothing to report.
//! `cause` is the machine-readable
//! [`HookArmingCause`](crate::api::artifact_status::HookArmingCause) — branch
//! on it, never on `message`, whose wording is explicitly unstable. It is the
//! field that makes `state: not-armed` actionable: several different refusals
//! share that one token, and only `cause` says whether the remedy is fixing
//! `GRIM_HOME`, enabling the feature flag, consenting to this workspace, or
//! approving `/hooks` inside Codex. Reporting them as one undifferentiated
//! state is the silent-guardrail defect WP-P0 filed against the
//! pre-amendment C-017.
//!
//! **No reported cause is ever transient.** `grim status` filters a transient
//! cause out (`crate::command::status`'s refusal branch): a transient answer is
//! stale the moment it is printed, so it would tell the reader to retry
//! something that may already have succeeded. `transient` therefore reads
//! `false` on every status row today — it is the cause's own classification,
//! carried so a consumer never needs a lookup table, and the surface for a
//! transient write-time refusal is the install-time warning instead.
//!
//! `arming` never influences the exit code — `grim status` is a report, and a
//! gated hook is the documented default (invariant I4), not a failure.

use std::io::{self, Write};
use std::path::PathBuf;

use serde::{Serialize, Serializer};

use crate::cli::printer::{Printable, print_table, screen_plain};
use crate::config::scope::Invocation;
use crate::install::launcher::LauncherState;
use crate::oci::{ArtifactKind, PinnedIdentifier};

use super::artifact_status::{ArtifactStatus, HookArmingCause};

/// Why one `(hook, client)` pair is not armed and running (C-017).
///
/// One element per affected client, so a hook armed on `claude` and gated on
/// `codex` reports both rather than collapsing to a single verdict the user
/// cannot act on. `message` is [`HookArmingCause::message`] — carried in the
/// payload rather than left for the consumer to look up, because the cause
/// token is stable and the wording is not (`docs/src/stability.md` § Unstable
/// covers human-readable text).
/// `Clone` because `grim hook list` reports one item per `[[hooks]]` entry and
/// every entry of one artifact carries that artifact's whole verdict set — the
/// verdicts are a property of the `(artifact, client)` pair, not of the entry,
/// so they are copied rather than re-derived per entry.
#[derive(Debug, Clone, Serialize)]
pub struct HookArming {
    /// The client target this verdict is about.
    pub client: String,
    /// The machine-readable cause. Consumers branch on this, never on
    /// `message`.
    pub cause: HookArmingCause,
    /// The distinguishing, remedy-bearing text — the half of C-017 that keeps
    /// four different refusals from reading identically.
    pub message: String,
    /// Whether re-running may succeed with no user action
    /// ([`HookArmingCause::transient`]). True only for a held dispatch lock;
    /// every other cause needs the user to change something first.
    ///
    /// Consequently `false` on every `grim status` row: a transient cause is
    /// filtered before it reaches a report (see the module doc). Present rather
    /// than omitted so a consumer branching on `cause` gets the classification
    /// without maintaining its own copy of the table.
    pub transient: bool,
}

/// One client's materialized output location for a status entry.
#[derive(Debug, Serialize)]
pub struct StatusOutput {
    /// The client target name (`claude`/`opencode`/`copilot`).
    pub client: String,
    /// The resolved on-disk path the artifact was materialized to for this
    /// client.
    pub path: PathBuf,
    /// The array element a settings piece contributed. Always present, and
    /// `null` for every other kind and for a settings scalar or own file.
    pub element: Option<String>,
}

/// A nested bundle, a `[[bundle]]` only an edge reaches, whose recorded reference
/// moved past its pin. `moved_nested_bundles` lists, sorted by name, the ones a
/// row reaches through nodes with no row of their own. `[]` when nothing moved or
/// nothing applies. `null` when `checked` is `false`, when a reached bundle has no
/// answer and none reached moved, or when grim cannot place one under its rows
/// (the graph cannot be computed offline, or the listing passed its work budget).
/// A list is what is known to have moved, so a failed re-resolve beside a moved
/// bundle leaves it out. `grim update -- <name>` with a listed name moves that
/// bundle's pin; the row's own pin stays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MovedNestedBundle {
    /// The `[[bundle]]` name, which `grim update -- <name>` takes.
    pub name: String,
    /// The reference grim re-resolved for it.
    pub reference: String,
}

/// One immediate parent of a status row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequiredBy {
    #[serde(serialize_with = "serialize_kind")]
    pub kind: ArtifactKind,
    pub name: String,
}

/// One declared artifact's status row.
#[derive(Debug, Serialize)]
pub struct StatusEntry {
    #[serde(serialize_with = "serialize_kind")]
    pub kind: ArtifactKind,
    pub name: String,
    /// Provenance: `"direct"` for a declared artifact, `"path: <p>"` for a
    /// path declaration, `"bundle: <registry/repo>, …"` for a member a bundle
    /// contributes, `"dependency: <kind>/<name>, …"` for one reached only
    /// through requires edges, and a bare `"dependency"` when that graph
    /// cannot be computed offline. Consumers read `direct` and `required_by`
    /// instead of parsing this string.
    pub source: String,
    /// Declared in this scope's config under its own kind table (`[bundles]`
    /// for a bundle row). Always computable, never null.
    pub direct: bool,
    /// Immediate parents (bundles included as `kind: "bundle"`), sorted by
    /// `(kind, name)`. `[]` = nothing requires this row; `null` = the
    /// dependency graph cannot be computed offline.
    pub required_by: Option<Vec<RequiredBy>>,
    /// The locked pin, if the artifact is locked.
    #[serde(serialize_with = "serialize_opt_pinned")]
    pub pinned: Option<PinnedIdentifier>,
    pub state: ArtifactStatus,
    /// Per-client materialized output locations. Empty when the artifact
    /// has no recorded install-state outputs.
    pub outputs: Vec<StatusOutput>,
    /// Outputs `grim install` would write **right now** that the install
    /// record does not already account for — materialization drift. Sorted by
    /// client; `[]` when an install would write nothing new.
    ///
    /// Two causes, both of them real work an install would do: a client that
    /// gained support since the last install (installed after the fact, or
    /// newly configured), and a render-layout move (reported at the NEW path).
    /// A present-but-drifted output is **not** here — that is
    /// `state: modified`, a different problem with a different remedy — and
    /// neither is a recorded output whose file was deleted, which surfaces as
    /// `state: missing`.
    ///
    /// Derived from the same seam the installer's own no-op check uses
    /// (`crate::install::expected_outputs`).
    ///
    /// Remediation is `grim install` — **not** `grim update`, which
    /// re-resolves floating tags and rolls the lock forward. `state` is
    /// unaffected: a byte-intact artifact at the locked pin still reads
    /// `installed`, and this field never influences the exit code.
    pub outputs_pending: Vec<StatusOutput>,
    /// Clients the project's config targets but this artifact has no
    /// recorded output for (`desired − recorded`). Sorted; `[]` when there
    /// is no such drift, and always `[]` when `[options].clients` is unset
    /// (autodetect — no explicit target to diff against) and on a settings row.
    pub clients_missing: Vec<String>,
    /// Clients this artifact has a recorded output for but the project's
    /// config no longer targets (`recorded − desired`). Sorted; `[]` when
    /// there is no such drift, and always `[]` when `[options].clients` is
    /// unset (autodetect — no explicit target to diff against).
    pub clients_extra: Vec<String>,
    /// Clients whose recorded output could not be resolved at all — the
    /// anchored target hit an [`AnchorError`](crate::install::AnchorError)
    /// (an escaped anchor, or an anchor root absent on this machine). Sorted;
    /// `[]` when every recorded output resolved. `state` still reads
    /// `missing` and the exit code stays 0 — `status` is a report.
    ///
    /// Under the read/destructive containment split this fires only for a
    /// symlinked **leaf** or an absent anchor root, never for a relocated
    /// ancestor (which now resolves cleanly).
    pub clients_unresolved: Vec<String>,
    /// The publisher's deprecation message for a registry-sourced item,
    /// from `--check`'s catalog load. `null` when `checked` is `false`, the
    /// item carries no registry pin, or the catalog carries no notice.
    pub deprecated: Option<String>,
    /// The publisher-named successor reference for a registry-sourced item,
    /// from `--check`'s catalog load. `null` under the same conditions as
    /// `deprecated`.
    pub replaced_by: Option<String>,
    /// Whether the registry carries a newer digest than the lock pin, from
    /// `--check`'s fresh per-artifact re-resolution. `null` when the check
    /// did not run, the row has no lock pin or recorded reference, or the
    /// re-resolution failed. See the module doc for the full contract.
    pub update_available: Option<bool>,
    /// Per-client verdicts for a `hook` or `settings` row (C-017). Sorted by
    /// client; **always present**, and `[]` for every other kind and for a row
    /// with nothing to report.
    ///
    /// `[]` therefore means "nothing to report", never "unknown" — the
    /// always-present-null discipline applied to an array, exactly as
    /// `clients_missing` does it.
    ///
    /// A hook row's [`ArtifactStatus`] is derived from these by the precedence
    /// in [`crate::command::status`]'s `hook_row_state`; a settings row's comes
    /// from `settings_arming`, and some of its entries are notes that leave the
    /// state alone. This field is what makes the state actionable, since the
    /// token alone does not say which client or which cause.
    pub arming: Vec<HookArming>,
    /// The nested bundles this row reaches whose reference moved, from
    /// `--check`. `[]` when none did, `null` when the check did not run or
    /// could not answer. See [`MovedNestedBundle`] for the full contract.
    pub moved_nested_bundles: Option<Vec<MovedNestedBundle>>,
}

pub(crate) fn serialize_kind<S: Serializer>(kind: &ArtifactKind, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&kind.to_string())
}

fn serialize_opt_pinned<S: Serializer>(pinned: &Option<PinnedIdentifier>, s: S) -> Result<S::Ok, S::Error> {
    match pinned {
        Some(p) => s.serialize_some(&p.strip_advisory().to_string()),
        None => s.serialize_none(),
    }
}

/// The `Note` cell: `<client>: <cause>` per arming verdict, `-` when there is
/// none.
///
/// The short **cause token**, not [`HookArmingCause::message`] — a table cell
/// is not a place for a sentence, and the full remedy text reaches the user on
/// stderr from [`crate::command::status`] (the repo's standing split: tables on
/// stdout, human guidance through `tracing`). The token alone still
/// distinguishes all four `not-armed` refusals from each other, which is what
/// C-017 requires of the human surface.
fn note_cell(arming: &[HookArming]) -> String {
    if arming.is_empty() {
        return "-".to_string();
    }
    arming
        .iter()
        .map(|a| format!("{}: {}", a.client, a.cause))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The result of a status query: one row per declared artifact.
#[derive(Debug, Serialize)]
pub struct StatusReport {
    items: Vec<StatusEntry>,
    /// Whether `--check` ran a live catalog lookup for this report. See the
    /// module doc for the full consumer contract.
    checked: bool,
    /// Plain text only: the launcher when hooks are in play and it is not
    /// current. Skipped in JSON, whose shape is frozen.
    #[serde(skip)]
    launcher_note: Option<(LauncherState, String)>,
    /// Plain text only: the `grim` the launcher note names.
    #[serde(skip)]
    grim: Invocation,
    /// Plain text only: advisory lines for stderr. Skipped in JSON, which
    /// carries the same facts in `direct`, `required_by`, `update_available`
    /// and `moved_nested_bundles`.
    #[serde(skip)]
    notes: Vec<String>,
}

impl StatusReport {
    /// Build from operation results. `checked` is `true` iff `--check` was
    /// passed and the run was online (see the module doc).
    pub fn new(items: Vec<StatusEntry>, checked: bool) -> Self {
        Self {
            items,
            checked,
            launcher_note: None,
            grim: Invocation::WALKED,
            notes: Vec::new(),
        }
    }

    /// Name `grim` in every command the plain rendering prints.
    #[must_use]
    pub fn naming(mut self, grim: Invocation) -> Self {
        self.grim = grim;
        self
    }

    /// Have the plain rendering print `notes` on stderr.
    #[must_use]
    pub fn with_notes(mut self, notes: Vec<String>) -> Self {
        self.notes = notes;
        self
    }

    /// Have the plain rendering name a launcher that is not current.
    #[must_use]
    pub fn with_launcher(mut self, state: LauncherState, path: String) -> Self {
        self.launcher_note = (state != LauncherState::Current).then_some((state, path));
        self
    }
}

impl Printable for StatusReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        // The `Note` column appears only when a hook or settings row is
        // present, so any other table stays byte-identical to what it was
        // before those kinds existed (WP-01 review W4).
        let has_note = self
            .items
            .iter()
            .any(|e| matches!(e.kind, ArtifactKind::Hook | ArtifactKind::Settings));
        let rows: Vec<Vec<String>> = self
            .items
            .iter()
            .map(|e| {
                let mut row = vec![
                    e.kind.to_string(),
                    e.name.clone(),
                    e.source.clone(),
                    e.pinned
                        .as_ref()
                        .map(|p| p.strip_advisory().to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    e.state.to_string(),
                ];
                if has_note {
                    row.push(note_cell(&e.arming));
                }
                row
            })
            .collect();
        const HEADERS: [&str; 5] = ["Kind", "Name", "Source", "Pinned", "State"];
        const NOTE_HEADERS: [&str; 6] = ["Kind", "Name", "Source", "Pinned", "State", "Note"];
        print_table(w, if has_note { &NOTE_HEADERS } else { &HEADERS }, &rows)?;
        // Every hook row fires through the launcher; the table would not say it is stale.
        if let Some((state, path)) = &self.launcher_note {
            writeln!(
                w,
                "\nLauncher {state}: {} (run {} install to write it)",
                screen_plain(path),
                self.grim
            )?;
        }
        print_notes(&mut io::stderr(), &self.notes);
        Ok(())
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, self)
    }
}

/// One line per advisory note, screened: a note quotes names and references
/// from the lock, and these lines bypass the table that screens its cells.
pub(crate) fn print_notes(w: &mut impl Write, notes: &[String]) {
    // Best effort: a closed stderr must not fail the report.
    for note in notes {
        let _ = writeln!(w, "{}", screen_plain(note));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::{Digest, Identifier};

    fn pinned(repo: &str) -> PinnedIdentifier {
        let id = Identifier::new_registry(repo, "localhost:5000").clone_with_digest(Digest::Sha256("a".repeat(64)));
        PinnedIdentifier::try_from(id).unwrap()
    }

    /// A minimal entry with every field at its "nothing to report" default;
    /// tests override only the fields they exercise via struct-update syntax.
    fn base_entry(name: &str) -> StatusEntry {
        StatusEntry {
            kind: ArtifactKind::Rule,
            name: name.to_string(),
            source: "direct".to_string(),
            direct: true,
            required_by: Some(Vec::new()),
            pinned: None,
            state: ArtifactStatus::Missing,
            outputs: Vec::new(),
            outputs_pending: Vec::new(),
            clients_missing: Vec::new(),
            clients_extra: Vec::new(),
            clients_unresolved: Vec::new(),
            deprecated: None,
            replaced_by: None,
            update_available: None,
            arming: Vec::new(),
            moved_nested_bundles: None,
        }
    }

    /// WP-01 review W4: the `Note` column appears only beside a hook row, so a
    /// hook-free table keeps main's five columns byte for byte.
    #[test]
    fn the_note_column_appears_only_with_a_hook_row() {
        let plain = |items: Vec<StatusEntry>| {
            let mut buf = Vec::new();
            StatusReport::new(items, false).print_plain(&mut buf).unwrap();
            String::from_utf8(buf).unwrap()
        };
        let hook_free = plain(vec![base_entry("rust-style")]);
        assert_eq!(
            hook_free.lines().next().unwrap().split_whitespace().collect::<Vec<_>>(),
            ["Kind", "Name", "Source", "Pinned", "State"],
            "{hook_free}"
        );
        let with_hook = plain(vec![
            base_entry("rust-style"),
            StatusEntry {
                kind: ArtifactKind::Hook,
                ..base_entry("guard")
            },
        ]);
        assert!(
            with_hook.lines().next().unwrap().trim_end().ends_with("Note"),
            "{with_hook}"
        );
        let with_settings = plain(vec![StatusEntry {
            kind: ArtifactKind::Settings,
            arming: vec![HookArming {
                client: "claude".to_string(),
                cause: HookArmingCause::LocalFileTracked,
                message: String::new(),
                transient: false,
            }],
            ..base_entry("team")
        }]);
        assert!(
            with_settings.lines().next().unwrap().trim_end().ends_with("Note"),
            "{with_settings}"
        );
        assert!(with_settings.contains("claude: local-file-tracked"), "{with_settings}");
    }

    #[test]
    fn plain_names_a_launcher_that_is_not_current_and_json_is_unchanged() {
        let report = |state| {
            StatusReport::new(vec![base_entry("x")], false).with_launcher(state, "/h/hooks/grim-hook".to_string())
        };
        let plain = |r: &StatusReport| {
            let mut buf = Vec::new();
            r.print_plain(&mut buf).unwrap();
            String::from_utf8(buf).unwrap()
        };
        let stale = report(LauncherState::Missing);
        assert!(plain(&stale).contains("Launcher missing: /h/hooks/grim-hook (run grim install to write it)"));
        assert!(!plain(&report(LauncherState::Current)).contains("Launcher"));
        let mut buf = Vec::new();
        stale.print_json(&mut buf).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        let keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["checked", "items"]);
    }

    #[test]
    fn a_launcher_note_names_the_run() {
        let r = StatusReport::new(vec![base_entry("x")], false)
            .with_launcher(LauncherState::Missing, "/h/hooks/grim-hook".to_string())
            .naming(Invocation::GLOBAL);
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("(run grim --global install to write it)"), "{text}");
    }

    #[test]
    fn plain_screens_a_control_character_in_the_launcher_path() {
        let r = StatusReport::new(vec![base_entry("x")], false)
            .with_launcher(LauncherState::Missing, "/h/\u{1b}[31m\nx/grim-hook".to_string());
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(!text.contains('\u{1b}'), "{text:?}");
        assert!(
            text.contains("Launcher missing: /h/\\u{1b}[31m x/grim-hook (run"),
            "{text:?}"
        );
    }

    #[test]
    fn plain_single_table() {
        let r = StatusReport::new(
            vec![
                StatusEntry {
                    kind: ArtifactKind::Skill,
                    pinned: Some(pinned("code-review")),
                    state: ArtifactStatus::Installed,
                    outputs: vec![StatusOutput {
                        client: "claude".to_string(),
                        element: None,
                        path: "/w/.claude/skills/code-review".into(),
                    }],
                    ..base_entry("code-review")
                },
                StatusEntry {
                    source: "bundle: ghcr.io/acme/stack".to_string(),
                    ..base_entry("rust-style")
                },
            ],
            false,
        );
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.lines().next().unwrap().starts_with("Kind"));
        assert!(out.contains("Source"));
        assert!(out.contains("bundle: ghcr.io/acme/stack"));
        assert!(out.contains("installed"));
        assert!(out.contains("missing"));
    }

    #[test]
    fn json_pinned_null_when_unlocked() {
        let r = StatusReport::new(
            vec![StatusEntry {
                state: ArtifactStatus::Stale,
                ..base_entry("x")
            }],
            false,
        );
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(v.is_object());
        assert!(v["items"].is_array());
        assert!(v["items"][0]["pinned"].is_null());
        assert_eq!(v["items"][0]["source"], "direct");
        assert_eq!(v["items"][0]["state"], "stale");
        assert_eq!(v["items"][0]["outputs"], serde_json::json!([]));
        assert_eq!(v["items"][0]["clients_missing"], serde_json::json!([]));
        assert_eq!(v["items"][0]["clients_extra"], serde_json::json!([]));
    }

    #[test]
    fn json_outputs_carries_client_and_path() {
        let r = StatusReport::new(
            vec![StatusEntry {
                kind: ArtifactKind::Skill,
                pinned: Some(pinned("s")),
                state: ArtifactStatus::Installed,
                outputs: vec![StatusOutput {
                    client: "claude".to_string(),
                    element: None,
                    path: "/w/.claude/skills/s".into(),
                }],
                ..base_entry("s")
            }],
            false,
        );
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["items"][0]["outputs"][0]["client"], "claude");
        assert_eq!(v["items"][0]["outputs"][0]["path"], "/w/.claude/skills/s");
        assert!(v["items"][0]["outputs"][0]["element"].is_null());
    }

    #[test]
    fn json_client_drift_fields_are_always_present_arrays() {
        let r = StatusReport::new(
            vec![StatusEntry {
                kind: ArtifactKind::Skill,
                pinned: Some(pinned("s")),
                state: ArtifactStatus::Installed,
                clients_missing: vec!["opencode".to_string()],
                clients_extra: vec!["copilot".to_string()],
                ..base_entry("s")
            }],
            false,
        );
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["items"][0]["clients_missing"], serde_json::json!(["opencode"]));
        assert_eq!(v["items"][0]["clients_extra"], serde_json::json!(["copilot"]));
    }

    /// C3 contract: `checked` rides the envelope as a sibling of `items`,
    /// and `deprecated`/`replaced_by`/`update_available` are always-present
    /// (never an absent key) even when null — the additive-field policy.
    #[test]
    fn json_checked_and_remote_fields_always_present() {
        let r = StatusReport::new(vec![base_entry("x")], false);
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["checked"], false);
        let item = &v["items"][0];
        assert!(item.as_object().unwrap().contains_key("deprecated"));
        assert!(item.as_object().unwrap().contains_key("replaced_by"));
        assert!(item.as_object().unwrap().contains_key("update_available"));
        assert!(item.as_object().unwrap().contains_key("moved_nested_bundles"));
        assert!(item["deprecated"].is_null());
        assert!(item["replaced_by"].is_null());
        assert!(item["update_available"].is_null());
        assert!(item["moved_nested_bundles"].is_null());
    }

    #[test]
    fn json_lists_a_moved_nested_bundle_by_name_and_reference() {
        let r = StatusReport::new(
            vec![
                StatusEntry {
                    moved_nested_bundles: Some(vec![MovedNestedBundle {
                        name: "inner".to_string(),
                        reference: "ghcr.io/acme/inner:1".to_string(),
                    }]),
                    ..base_entry("outer")
                },
                StatusEntry {
                    moved_nested_bundles: Some(Vec::new()),
                    ..base_entry("stays")
                },
            ],
            true,
        );
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(
            v["items"][0]["moved_nested_bundles"],
            serde_json::json!([{"name": "inner", "reference": "ghcr.io/acme/inner:1"}])
        );
        assert_eq!(v["items"][1]["moved_nested_bundles"], serde_json::json!([]));
    }

    #[test]
    fn a_note_is_screened_on_its_way_to_stderr() {
        let mut buf = Vec::new();
        print_notes(
            &mut buf,
            &["note: \u{1b}[2J\u{202e}x\ry".to_string(), "plain".to_string()],
        );
        let text = String::from_utf8(buf).unwrap();
        assert_eq!(text, "note: \\u{1b}[2J\\u{202e}x\\u{d}y\nplain\n");
    }

    /// `checked: true` carries populated `deprecated`/`replaced_by` on a
    /// matched item; the report is a pure serializer, so an entry the command
    /// left with `update_available: None` still renders as `null` (the
    /// command owns the value — see `command::status`).
    #[test]
    fn json_checked_true_carries_populated_deprecation_fields() {
        let r = StatusReport::new(
            vec![StatusEntry {
                pinned: Some(pinned("old-skill")),
                deprecated: Some("use new-skill instead".to_string()),
                replaced_by: Some("ghcr.io/acme/new-skill".to_string()),
                ..base_entry("old-skill")
            }],
            true,
        );
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["checked"], true);
        assert_eq!(v["items"][0]["deprecated"], "use new-skill instead");
        assert_eq!(v["items"][0]["replaced_by"], "ghcr.io/acme/new-skill");
        assert!(v["items"][0]["update_available"].is_null());
    }
}
