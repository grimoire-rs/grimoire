// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Install-tier errors.
//!
//! Three-layer shape mirroring [`crate::config::config_error`],
//! [`crate::lock::lock_error`], and [`crate::oci::access::error`]: top
//! [`crate::error::Error`] → context-bearing [`InstallError`] (carries the
//! boxed [`ArtifactRef`] the failure is about, when one applies) →
//! discriminant [`InstallErrorKind`].

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::api::settings_report::RefusedPiece;
use crate::config::declaration::DeclaredSource;
use crate::config::scope::Invocation;
use crate::oci::Digest;
use crate::oci::reference::ArtifactRef;
use crate::oci::settings::SettingsError;
use crate::oci::text::escape_controls;
use crate::resolve::resolve_error::gone_pin_remedy;

/// An install-tier operation failed, optionally on a specific artifact.
///
/// The reference is `None` for store-wide failures (install-state I/O not
/// attributable to one artifact); it is `Some` for per-artifact failures.
#[derive(Debug)]
pub struct InstallError {
    /// The artifact the failure is about. Boxed so [`InstallErrorKind`]
    /// stays small (avoids a `clippy::result_large_err` suppression).
    pub reference: Option<Box<ArtifactRef>>,
    /// The specific failure.
    pub kind: InstallErrorKind,
    /// The `grim` the remedy names. The fetch that raises the error holds no
    /// scope; [`Self::naming`] fills this in where one is known.
    pub grim: Invocation,
}

impl InstallError {
    /// Attach `reference` context to `kind`.
    pub fn with_reference(reference: ArtifactRef, kind: InstallErrorKind) -> Self {
        Self {
            reference: Some(Box::new(reference)),
            kind,
            grim: Invocation::WALKED,
        }
    }

    /// Construct without artifact context (store-wide failures).
    pub fn without_reference(kind: InstallErrorKind) -> Self {
        Self {
            reference: None,
            kind,
            grim: Invocation::WALKED,
        }
    }

    /// Name `grim` in this error's remedy.
    #[must_use]
    pub fn naming(mut self, grim: &Invocation) -> Self {
        self.grim = grim.clone();
        self
    }
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.reference {
            Some(r) => write!(f, "{r}: {}", self.kind)?,
            None => write!(f, "{}", self.kind)?,
        }
        let grim = &self.grim;
        match (&self.kind, self.reference.as_deref()) {
            // A path source re-packs on every relock, so only a registry pin has one to move.
            (InstallErrorKind::BlobMissing, Some(r)) if matches!(r.source, DeclaredSource::Registry(_)) => {
                write!(f, "; {}", gone_pin_remedy(grim, &r.name))
            }
            (InstallErrorKind::LocalContentChanged { name, .. }, _) => {
                write!(f, "; run `{}` or `{grim} lock`", grim.update(name))
            }
            (
                InstallErrorKind::StaleDependencies {
                    relock: Some(relock), ..
                },
                _,
            ) => write!(f, "; run {relock}"),
            (InstallErrorKind::StaleDependencies { .. }, _) => write!(f, "; run `{grim} lock`"),
            (InstallErrorKind::HeldByDevInstall, _) => write!(f, "; rerun with --force to replace it"),
            (InstallErrorKind::HeldByLock, Some(r)) => write!(
                f,
                "; rename the local {} or run `{}` first",
                r.kind,
                grim.uninstall("", r.kind, &r.name)
            ),
            _ => Ok(()),
        }
    }
}

impl std::error::Error for InstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // `Display` already embeds the kind's message; expose the kind's own
        // cause so `{:#}` chains do not print the kind twice.
        self.kind.source()
    }
}

/// Inner discriminant for install-tier failures.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum InstallErrorKind {
    /// The pinned blob is absent from the registry and the cache.
    #[error("blob not found in registry or local cache")]
    BlobMissing,

    /// A previously installed artifact was modified on disk: the recorded
    /// content hash no longer matches what is on disk. Refused unless the
    /// caller forces the reinstall.
    #[error(
        "installed artifact was modified locally: recorded {recorded}, found {actual}; rerun with --force to overwrite"
    )]
    IntegrityMismatch { recorded: Digest, actual: Digest },

    /// A filesystem operation on an install target failed.
    #[error("I/O error for {}", shown_path(.path))]
    TargetIo {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// The blob could not be materialized (corrupt tar, unsafe entry).
    #[error("failed to materialize artifact: {}", escape_controls(.0))]
    MaterializeFailed(String),

    /// Fetched blob bytes did not hash to the pinned digest.
    #[error("blob digest mismatch: expected {expected}, got {actual}")]
    BlobDigestMismatch { expected: Digest, actual: Digest },

    /// The manifest declares a layer larger than the install policy cap.
    /// Rejected before download so a hostile declared size cannot become
    /// the fetch memory cap and OOM the install (CWE-770).
    #[error("layer size {actual} exceeds the install limit of {limit} bytes")]
    OversizeLayer { limit: u64, actual: u64 },

    /// An install destination exists on disk but is not recorded for
    /// that client: grim refuses to overwrite files it did not create.
    #[error(
        "destination '{}' already exists for client {} and was not created by grim; rerun with --force to overwrite",
        shown_path(.path),
        escape_controls(.client)
    )]
    UntrackedDestination { client: String, path: PathBuf },

    /// The configured client target is not supported by this build.
    #[error(
        "unsupported client target '{}'; supported clients are {list}",
        escape_controls(.0),
        list = supported_clients_list()
    )]
    UnsupportedClient(String),

    /// No AI client was detected, so the install fell back to the generic
    /// `agents` client — and nothing in the artifact set can install into
    /// it. The generic client renders skills only; rules, agents, and MCP
    /// have no vendor-neutral surface at all. Selection failure, not data
    /// failure: it classifies to the same exit 78 as
    /// [`InstallErrorKind::UnsupportedClient`].
    ///
    /// The message names **both** knobs deliberately: `grim install` and
    /// `grim update` take `--client`, but `grim add` reaches this error
    /// through the same install seam without having that flag, so pointing
    /// only at `--client` would be unactionable there.
    #[error(
        "no AI client detected and nothing here installs into the generic 'agents' client (it renders skills only; \
         rules, agents, and MCP have no vendor-neutral surface); select a client with --client or \
         `[options].clients` — supported clients are {list}",
        list = supported_clients_list()
    )]
    NoInstallableClient,

    /// A local path source failed to pack at install time: it is missing,
    /// fails validation, or is unreadable. Carries the packing
    /// [`crate::skill::SkillError`] structurally (boxed to keep the kind
    /// small) so the `{:#}` chain and `source()` expose the real cause
    /// rather than a flattened string.
    #[error("local source unusable")]
    LocalSource(#[source] Box<crate::skill::SkillError>),

    /// A local path source's packed content no longer hashes to the locked
    /// pin — the source drifted since the lock was written. Wraps no inner
    /// error; the mismatch is fully described by the recorded vs. found
    /// digests (edit the source, then `grim update <name>` / `grim lock`).
    /// [`InstallError`] prints that remedy, which names the run's `grim`.
    #[error(
        "local source '{shown}' changed since the lock was written (locked {locked}, found {actual})",
        shown = escape_controls(.name)
    )]
    LocalContentChanged {
        name: String,
        locked: Digest,
        actual: Digest,
    },

    /// A lock entry's `requires` differ from the requires layer of the
    /// digest-verified manifest it pins (a lock written before dependencies
    /// existed, a hand edit, an offline carry-forward). Raised without an
    /// artifact reference: the message names the artifact itself. [`InstallError`]
    /// prints the remedy, which names the run's `grim`.
    #[error(
        "{kind} '{}': the lock's dependencies do not match the pinned artifact",
        escape_controls(.name)
    )]
    StaleDependencies {
        kind: crate::oci::ArtifactKind,
        name: String,
        /// The remedy that relocks the lock holding the row where that is no
        /// `grim lock`: a marketplace part's, which `grim lock` never writes. A
        /// backticked command, with the clause that says what it relocks.
        relock: Option<String>,
    },

    /// Settings pieces the phase refused, across every package. `--force`
    /// overrides; each piece carries its own state, so a mixed list loses
    /// nothing. The message names every package, since one refusal can hold
    /// pieces of several.
    #[error("{}; rerun with --force to change them", settings_refusal_heading(.pieces))]
    SettingsRefused { pieces: Arc<[RefusedPiece]> },

    /// Packages declare settings that cannot hold together. No `--force`
    /// applies, because neither value is the user's.
    #[error(
        "settings conflict across packages: {} value(s) cannot all apply; change or remove one of {}, \
         since no --force overrides a conflict",
        .conflicts.len(),
        package_list(.conflicts)
    )]
    SettingsConflict { conflicts: Vec<RefusedPiece> },

    /// A settings file changed between the plan and the write, so the plan no
    /// longer holds. Retryable: the rerun plans against the new bytes.
    #[error(
        "settings file '{}' changed during the run, so grim left it untouched; rerun the command",
        shown_path(.path)
    )]
    SettingsRaced { path: PathBuf },

    /// A settings file stopped parsing between the plan and the write. A
    /// rerun refuses it too, so this is data, not a race.
    #[error(
        "settings file '{}' changed during the run and no longer parses; grim never replaces such a file, \
         so repair it and rerun",
        shown_path(.path)
    )]
    SettingsBecameUnparseable { path: PathBuf },

    /// A settings file a piece adds to or changes cannot be parsed. Names the
    /// file and nothing from it: a user's file may hold what a package may not.
    #[error(
        "{}: settings file '{}' cannot be parsed, and grim never replaces it, \
         so repair its syntax and rerun",
        escape_controls(.client),
        shown_path(.path)
    )]
    SettingsUnparseable { client: String, path: PathBuf },

    /// A package's layer holds a document this grim refuses. Carried here so
    /// the row names the package, which the document error alone cannot.
    #[error("settings document refused")]
    SettingsDocument(#[source] SettingsError),

    /// A binding name outside the artifact-name grammar, met at install
    /// rather than at `add`. `reason` is already escaped for a terminal.
    #[error("invalid {kind} binding name: {reason} (allowed: lowercase letters, digits, hyphens, periods)")]
    InvalidBindingName {
        kind: crate::oci::ArtifactKind,
        reason: String,
    },

    /// A declared install met a dev-install record under the same
    /// `(kind, name)`. [`InstallError`] prints the remedy.
    #[error("a dev-installed local copy holds this name")]
    HeldByDevInstall,

    /// A dev-install met the record of a declared artifact, a bundle member
    /// or a dependency under the same `(kind, name)`. [`InstallError`] prints
    /// the remedy.
    #[error("this name is already installed from a declared source")]
    HeldByLock,
}

/// What a value refused for its content is: true whether grim never wrote it
/// or wrote it and it changed since.
pub(crate) const CHANGED_OUTSIDE_GRIM: &str = "set or changed outside grim";

/// What a settings refusal of `count` pieces says.
pub(crate) fn settings_refusal_summary(count: usize) -> String {
    format!("{count} settings value(s) were {CHANGED_OUTSIDE_GRIM}")
}

/// A settings refusal's first line: every package `pieces` hold, then the count.
pub(crate) fn settings_refusal_heading(pieces: &[RefusedPiece]) -> String {
    format!(
        "settings {}: {}",
        package_list(pieces),
        settings_refusal_summary(pieces.len())
    )
}

/// The packages `pieces` belong to, single-quoted, sorted and deduplicated.
fn package_list(pieces: &[RefusedPiece]) -> String {
    let names: std::collections::BTreeSet<&str> = pieces.iter().map(|piece| piece.package.as_str()).collect();
    names
        .into_iter()
        .map(|name| format!("'{}'", escape_controls(name)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A path as an error prints it: a destination holds a declared name.
fn shown_path(path: &Path) -> String {
    escape_controls(&path.display().to_string())
}

/// The supported client names, single-quoted and comma-joined, for the
/// [`InstallErrorKind::UnsupportedClient`] message. Derived from
/// [`super::client_target::ClientTarget::VALUE_NAMES`] so the message can
/// never drift from the actual supported set.
fn supported_clients_list() -> String {
    super::client_target::ClientTarget::VALUE_NAMES
        .iter()
        .map(|n| format!("'{n}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::settings_report::{PieceMark, PieceState};
    use crate::oci::{Algorithm, ArtifactKind, Identifier};

    fn artifact_ref() -> ArtifactRef {
        ArtifactRef::registry(
            ArtifactKind::Skill,
            "code-review",
            Identifier::parse("ghcr.io/acme/code-review:stable").unwrap(),
        )
    }

    #[test]
    fn display_with_reference_uses_prefix() {
        let err = InstallError::with_reference(artifact_ref(), InstallErrorKind::BlobMissing);
        let s = err.to_string();
        assert!(s.contains("skill"));
        assert!(s.contains("code-review"));
        assert!(s.contains("blob not found"));
    }

    #[test]
    fn display_without_reference_no_leading_separator() {
        let err = InstallError::without_reference(InstallErrorKind::UnsupportedClient("vscode".to_string()));
        assert!(!err.to_string().starts_with(':'));
        assert!(!err.to_string().starts_with(' '));
        assert!(err.to_string().contains("vscode"));
    }

    #[test]
    fn stale_dependencies_message_names_the_artifact_and_the_remedy() {
        let err = InstallError::without_reference(InstallErrorKind::StaleDependencies {
            kind: crate::oci::ArtifactKind::Skill,
            name: "dep".to_string(),
            relock: None,
        });
        assert_eq!(
            err.to_string(),
            "skill 'dep': the lock's dependencies do not match the pinned artifact; run `grim lock`"
        );
    }

    #[test]
    fn unsupported_client_names_every_supported_client() {
        // The message lists the supported set from `ClientTarget::VALUE_NAMES`
        // so it cannot drift as vendors are added.
        let msg = InstallErrorKind::UnsupportedClient("vscode".to_string()).to_string();
        for name in crate::install::client_target::ClientTarget::VALUE_NAMES {
            assert!(msg.contains(&format!("'{name}'")), "message must name '{name}': {msg}");
        }
        assert!(msg.contains("vscode"), "message names the offending target: {msg}");
    }

    #[test]
    fn integrity_mismatch_renders_both_digests() {
        let recorded = Algorithm::Sha256.hash(b"a");
        let actual = Algorithm::Sha256.hash(b"b");
        let kind = InstallErrorKind::IntegrityMismatch {
            recorded: recorded.clone(),
            actual: actual.clone(),
        };
        let s = kind.to_string();
        assert!(s.contains(&recorded.to_string()));
        assert!(s.contains(&actual.to_string()));
    }

    #[test]
    fn source_chain_skips_kind_layer() {
        use std::error::Error;
        // Display embeds the kind, so the chain must not re-expose it: a
        // kind without an underlying cause terminates the chain, while a
        // kind carrying a cause surfaces that cause directly.
        let err = InstallError::with_reference(artifact_ref(), InstallErrorKind::BlobMissing);
        assert!(err.source().is_none());

        let io = InstallError::without_reference(InstallErrorKind::TargetIo {
            path: std::path::PathBuf::from("/x"),
            source: std::io::Error::other("disk full"),
        });
        assert!(io.source().expect("chain reaches the I/O cause").is::<std::io::Error>());
    }

    /// Regression lock (design record F8): a local-pack failure carries its
    /// `SkillError` structurally via `#[source]`, not flattened into a
    /// `String` — the source chain is walkable to the packing failure (its
    /// message survives verbatim, not re-derived from a stringified
    /// `Display`), and the exit-code classification stays `DataError` (65)
    /// unchanged.
    ///
    /// The `#[source]` field is `Box<SkillError>` (kept small per
    /// `quality-rust-errors.md`'s three-layer pattern), so the concrete
    /// type behind the returned trait object is `Box<SkillError>` — a
    /// documented `thiserror`/`std::error::Error` interaction for boxed
    /// source fields (`Box<T>: Error` is a distinct concrete type from
    /// `T` for downcasting purposes, even though `Display`/`source()`
    /// delegate transparently to the inner value).
    #[test]
    fn local_source_error_chain_walks_to_skill_error() {
        use std::error::Error;

        let skill_err = crate::skill::SkillError::new("/w/skill", crate::skill::SkillErrorKind::MissingSkillMd);
        let err = InstallError::with_reference(artifact_ref(), InstallErrorKind::LocalSource(Box::new(skill_err)));

        let source = err.source().expect("chain reaches the packing SkillError");
        let boxed = source
            .downcast_ref::<Box<crate::skill::SkillError>>()
            .expect("source downcasts to the boxed SkillError, not a flattened String");
        assert!(matches!(boxed.kind, crate::skill::SkillErrorKind::MissingSkillMd));
        assert_eq!(source.to_string(), "/w/skill: skill directory has no SKILL.md");

        let anyhow_err: anyhow::Error = crate::error::Error::from(err).into();
        assert_eq!(
            crate::error::classify_error(&anyhow_err),
            crate::cli::exit_code::ExitCode::DataError
        );
    }

    fn piece(package: &str, marked: PieceMark, state: Option<PieceState>) -> RefusedPiece {
        RefusedPiece {
            package: package.to_string(),
            client: "claude".to_string(),
            file: Some(".claude/settings.local.json".to_string()),
            pointer: "/model".to_string(),
            element: None,
            marked,
            state,
            wanted: Some("sonnet".to_string()),
            current: Some("opus".to_string()),
        }
    }

    /// A value grim wrote and the user then changed is no value "grim never
    /// wrote": the one sentence must hold for both states.
    #[test]
    fn a_settings_refusal_reads_true_for_a_value_grim_wrote() {
        let pieces = vec![piece("team", PieceMark::Overwrite, Some(PieceState::Modified))];
        let message = InstallErrorKind::SettingsRefused { pieces: pieces.into() }.to_string();
        assert_eq!(
            message,
            "settings 'team': 1 settings value(s) were set or changed outside grim; rerun with --force to change them"
        );
    }

    #[test]
    fn a_settings_refusal_names_every_package_once() {
        let pieces = vec![
            piece("team", PieceMark::Overwrite, Some(PieceState::Modified)),
            piece("base", PieceMark::Overwrite, Some(PieceState::UntrackedDestination)),
            piece("team", PieceMark::Overwrite, Some(PieceState::UntrackedDestination)),
        ];
        let message = InstallErrorKind::SettingsRefused { pieces: pieces.into() }.to_string();
        assert_eq!(
            message,
            "settings 'base', 'team': 3 settings value(s) were set or changed outside grim; \
             rerun with --force to change them"
        );
    }

    /// The pre-run refusal names the fix its mid-run sibling names: `--force`
    /// cannot help, so only a repaired file lets a rerun through.
    #[test]
    fn an_unparseable_settings_file_names_the_repair_and_the_rerun() {
        let message = InstallErrorKind::SettingsUnparseable {
            client: "claude".to_string(),
            path: "/w/.claude/settings.local.json".into(),
        }
        .to_string();
        assert_eq!(
            message,
            "claude: settings file '/w/.claude/settings.local.json' cannot be parsed, and grim never replaces it, \
             so repair its syntax and rerun"
        );
    }

    #[test]
    fn a_settings_conflict_names_its_packages_and_the_remedy() {
        let conflicts = vec![
            piece("b", PieceMark::Conflict, None),
            piece("a", PieceMark::Conflict, None),
        ];
        let message = InstallErrorKind::SettingsConflict { conflicts }.to_string();
        assert_eq!(
            message,
            "settings conflict across packages: 2 value(s) cannot all apply; change or remove one of 'a', 'b', \
             since no --force overrides a conflict"
        );
    }

    #[test]
    fn a_hostile_name_stays_on_one_line() {
        const FORGED: &str = "1\nerror: forged\u{202e}";
        let id = Identifier::parse(&format!("ghcr.io/acme/tool:{FORGED}")).unwrap();
        let reference = ArtifactRef::registry(ArtifactKind::Skill, FORGED, id);
        let digest = crate::oci::Digest::Sha256("a".repeat(64));
        let kinds = [
            InstallErrorKind::MaterializeFailed(FORGED.to_string()),
            InstallErrorKind::UntrackedDestination {
                client: FORGED.to_string(),
                path: std::path::PathBuf::from(FORGED),
            },
            InstallErrorKind::UnsupportedClient(FORGED.to_string()),
            InstallErrorKind::LocalContentChanged {
                name: FORGED.to_string(),
                locked: digest.clone(),
                actual: digest,
            },
            InstallErrorKind::StaleDependencies {
                kind: ArtifactKind::Skill,
                name: FORGED.to_string(),
                relock: None,
            },
            InstallErrorKind::SettingsRefused {
                pieces: vec![piece(FORGED, PieceMark::Overwrite, Some(PieceState::Modified))].into(),
            },
            InstallErrorKind::SettingsConflict {
                conflicts: vec![piece(FORGED, PieceMark::Conflict, None)],
            },
            InstallErrorKind::SettingsRaced { path: FORGED.into() },
            InstallErrorKind::SettingsBecameUnparseable { path: FORGED.into() },
            InstallErrorKind::SettingsUnparseable {
                client: FORGED.to_string(),
                path: FORGED.into(),
            },
            InstallErrorKind::BlobMissing,
        ];

        for kind in kinds {
            let text = InstallError::with_reference(reference.clone(), kind).to_string();
            assert!(!text.contains('\n'), "{text:?}");
            assert!(!text.contains('\u{202e}'), "{text:?}");
        }
    }

    #[test]
    fn a_clean_reference_prints_byte_for_byte() {
        let err = InstallError::with_reference(artifact_ref(), InstallErrorKind::BlobMissing);

        assert_eq!(
            err.to_string(),
            "skill 'code-review' (ghcr.io/acme/code-review:stable): blob not found in registry or local cache; \
             run `grim update -- code-review` to move this pin. If the lock no longer matches what is declared or \
             another pin is gone too, `grim update` with no names moves every pin. If its tag is gone too, or a \
             digest names it, declare a reference that exists in grimoire.toml and run `grim lock`"
        );
    }

    /// Followed from a project, a plain `grim` moves or relocks that project.
    #[test]
    fn every_remedy_names_the_run_it_failed_in() {
        let config = Invocation::config(Path::new("/srv/my team/grimoire.toml"));

        for grim in [Invocation::GLOBAL, config] {
            let missing = InstallError::with_reference(artifact_ref(), InstallErrorKind::BlobMissing)
                .naming(&grim)
                .to_string();
            assert!(
                missing.contains(&format!("run `{grim} update -- code-review`")),
                "{missing}"
            );

            let changed = InstallError::without_reference(InstallErrorKind::LocalContentChanged {
                name: "local".to_string(),
                locked: Algorithm::Sha256.hash(b"x"),
                actual: Algorithm::Sha256.hash(b"y"),
            })
            .naming(&grim)
            .to_string();
            assert!(
                changed.ends_with(&format!("run `{grim} update -- local` or `{grim} lock`")),
                "{changed}"
            );

            let stale = InstallError::without_reference(InstallErrorKind::StaleDependencies {
                kind: ArtifactKind::Skill,
                name: "dep".to_string(),
                relock: None,
            })
            .naming(&grim)
            .to_string();
            assert!(stale.ends_with(&format!("run `{grim} lock`")), "{stale}");

            for text in [missing, changed, stale] {
                assert!(
                    !text.contains("`grim update") && !text.contains("`grim lock`"),
                    "{text}"
                );
                assert!(!text.contains("to each command"), "{text}");
            }
        }
    }

    /// A cold clone whose pinned manifest is gone has no other way to learn
    /// that only `grim update` may move the pin.
    #[test]
    fn a_missing_registry_pin_names_the_update_that_moves_it() {
        let registry = InstallError::with_reference(artifact_ref(), InstallErrorKind::BlobMissing).to_string();
        assert!(
            registry.contains("run `grim update -- code-review` to move this pin"),
            "{registry}"
        );
        assert!(registry.contains("If its tag is gone too"), "{registry}");
        // A pin a digest names cannot move: `grim update` resolves that digest again.
        assert!(
            registry.contains("or a digest names it, declare a reference that exists"),
            "{registry}"
        );

        let path = ArtifactRef {
            kind: ArtifactKind::Skill,
            name: "local".to_string(),
            source: crate::config::declaration::DeclaredSource::Path(
                crate::config::path_source::PathSource::parse("./local").unwrap(),
            ),
        };
        let path = InstallError::with_reference(path, InstallErrorKind::BlobMissing).to_string();
        assert!(!path.contains("grim update"), "a path source has no pin: {path}");
        let bare = InstallError::without_reference(InstallErrorKind::BlobMissing).to_string();
        assert_eq!(bare, "blob not found in registry or local cache");
    }
}
