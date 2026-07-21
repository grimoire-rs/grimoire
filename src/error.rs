// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Top-level error type and the error → exit-code classifier.
//!
//! [`classify`] is a free function (not a trait method) so the dependency
//! direction stays clean: errors do not depend on the exit-code taxonomy.
//! It walks the `anyhow` chain once, downcasts to [`Error`], and maps each
//! known kind to a [`Classification`] (exit code plus an optional
//! machine-readable [`ErrorReason`]). [`classify_error`] is a thin wrapper
//! kept for callers that only need the exit code.

use crate::auth::auth_error::AuthError;
use crate::catalog::catalog_error::{CatalogError, CatalogErrorKind};
use crate::catalog::index_announce::AnnounceError;
use crate::cli::exit_code::ExitCode;
use crate::cli::printer::StdoutPipeClosed;
use crate::command::command_error::CommandError;
use crate::config::config_error::{ConfigError, ConfigErrorKind};
use crate::install::install_error::{InstallError, InstallErrorKind};
use crate::install::path_anchor::AnchorError;
use crate::lock::lock_error::{LockError, LockErrorKind};
use crate::oci::access::error::{AccessError, AccessErrorKind};
use crate::oci::digest::error::DigestError;
use crate::oci::identifier::error::IdentifierError;
use crate::oci::pinned_identifier::PinnedIdentifierError;
use crate::oci::release::{ReleaseError, ReleaseErrorKind};
use crate::resolve::resolve_error::{ResolveError, ResolveErrorKind};
use crate::skill::skill_error::{SkillError, SkillErrorKind};

/// Top-level Grimoire error. Subsystem errors compose in via `#[from]`.
///
/// `#[error(transparent)]` on every arm: there is nothing to add at this
/// layer — the inner error already carries the full message and source.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error(transparent)]
    Identifier(#[from] IdentifierError),

    #[error(transparent)]
    Digest(#[from] DigestError),

    #[error(transparent)]
    PinnedIdentifier(#[from] PinnedIdentifierError),

    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error(transparent)]
    Lock(#[from] LockError),

    #[error(transparent)]
    Access(#[from] AccessError),

    #[error(transparent)]
    Resolve(#[from] ResolveError),

    #[error(transparent)]
    Install(#[from] InstallError),

    #[error(transparent)]
    Anchor(#[from] AnchorError),

    #[error(transparent)]
    Skill(#[from] SkillError),

    #[error(transparent)]
    Release(#[from] ReleaseError),

    #[error(transparent)]
    Catalog(#[from] CatalogError),

    #[error(transparent)]
    Auth(#[from] AuthError),

    #[error(transparent)]
    Command(#[from] CommandError),

    #[error(transparent)]
    Announce(#[from] AnnounceError),
}

/// Machine-readable failure `reason` subtype for the JSON error envelope
/// (`docs/src/json-interface.md`).
///
/// Kebab-case via [`std::fmt::Display`] — the wire path (`main.rs`
/// `error_document`) builds the JSON `reason` string through that
/// rendering, not a `Serialize` derive. Additive and forward-compatible:
/// consumers must tolerate both absence and unknown future values.
/// Annotated today: the stale-lock resolve refusal and the two
/// force-recoverable install refusals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorReason {
    /// A resolve was refused because the lock's recorded hash no longer
    /// matches what the registry currently serves.
    StaleLock,
    /// An install was refused because the installed artifact was modified
    /// locally (slug `modified`, matching the `grim status` state string);
    /// retry with `--force` to overwrite.
    LocalModified,
    /// An install was refused because the destination already exists on
    /// disk with no install record; retry with `--force` to overwrite and
    /// record it.
    UntrackedDestination,
    /// A project-scope command found no `grimoire.toml` by walking up from
    /// the working directory (`ConfigErrorKind::NotDiscovered`). Distinct
    /// from an explicit `--config <path>` that does not exist, which stays
    /// reason-less (see `classify_config`).
    NoConfig,
    /// A config-file write was refused because another process holds the
    /// `<file>.lock` advisory sidecar (`LockErrorKind::Locked`). Transient —
    /// retry may succeed once the other writer releases the lock.
    Locked,
    /// A recorded path resolved outside its anchor root
    /// (`AnchorError::EscapedAnchor`). NEVER forceable: `--force` does not
    /// bypass containment. The remediation is uninstall + reinstall.
    AnchorEscape,
}

impl std::fmt::Display for ErrorReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::StaleLock => "stale-lock",
            Self::LocalModified => "modified",
            Self::UntrackedDestination => "untracked-destination",
            Self::NoConfig => "no-config",
            Self::Locked => "locked",
            Self::AnchorEscape => "anchor-escape",
        })
    }
}

impl ErrorReason {
    /// Whether a consumer should treat this reason as transient — worth
    /// retrying the same command unchanged. Keyed on the reason (single
    /// source of truth), never on the exit code: `AuthError::Helper::Timeout`
    /// also exits `TempFail` (75) but carries no reason at all, so it can
    /// never reach this method — a caller that keyed on exit code instead
    /// would wrongly mark it retryable.
    pub fn retryable(self) -> bool {
        matches!(self, Self::Locked)
    }

    /// Whether re-running the same command with `--force` can resolve this
    /// refusal. Keyed on the reason, never on the exit code: `DataError` (65)
    /// covers both the forceable drift refusals and the non-forceable
    /// containment one, so a caller keying on the exit code would offer an
    /// override that cannot work. `--force` never bypasses containment.
    pub fn forceable(self) -> bool {
        matches!(self, Self::LocalModified | Self::UntrackedDestination)
    }
}

/// The result of classifying an error chain: the process exit code plus
/// an optional machine-readable [`ErrorReason`] subtype.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Classification {
    pub exit: ExitCode,
    pub reason: Option<ErrorReason>,
}

impl Classification {
    /// A classification carrying no reason subtype — the common case.
    fn new(exit: ExitCode) -> Self {
        Self { exit, reason: None }
    }
}

/// Maps an error chain to a [`Classification`] (exit code + optional
/// reason subtype).
///
/// Walks `err.chain()` once, downcasts each cause to [`Error`], and
/// exhaustively maps every Phase 1 variant. Anything not classified falls
/// through to [`ExitCode::Failure`] with no reason; the fall-through is
/// locked by a test so it cannot silently change.
pub fn classify(err: &anyhow::Error) -> Classification {
    for cause in err.chain() {
        if let Some(e) = cause.downcast_ref::<Error>() {
            // Exhaustive match: a new variant fails to compile until it is
            // explicitly classified here.
            return match e {
                Error::Identifier(_) => Classification::new(ExitCode::DataError),
                Error::Digest(_) => Classification::new(ExitCode::DataError),
                Error::PinnedIdentifier(_) => Classification::new(ExitCode::DataError),
                Error::Config(ce) => classify_config(ce),
                Error::Lock(le) => classify_lock(le),
                Error::Access(ae) => Classification::new(classify_access(ae)),
                Error::Resolve(re) => classify_resolve(re),
                Error::Install(ie) => classify_install(ie),
                Error::Anchor(ae) => classify_anchor(ae),
                Error::Skill(se) => Classification::new(classify_skill(se)),
                Error::Release(re) => Classification::new(classify_release(re)),
                Error::Catalog(ce) => Classification::new(classify_catalog(ce)),
                Error::Auth(ae) => Classification::new(classify_auth(ae)),
                Error::Command(ce) => Classification::new(match ce {
                    CommandError::LockMissing { .. } => ExitCode::NotFound,
                    CommandError::LockStale { .. } => ExitCode::DataError,
                    CommandError::NoLoginRegistry => ExitCode::ConfigError,
                    CommandError::LoginInput(_) => ExitCode::UsageError,
                    CommandError::KindInferenceFailed { .. } => ExitCode::DataError,
                    CommandError::DeclareConflict { .. } => ExitCode::UsageError,
                    CommandError::InvalidBindingName { .. } => ExitCode::UsageError,
                    CommandError::ConfigUsage(_) => ExitCode::UsageError,
                    CommandError::ConfigValue(_) => ExitCode::DataError,
                }),
                // Announce needs remote resources (the index repository, the
                // GitHub API); a local I/O fault classifies as I/O. A failed
                // required fork is a remote-resource fault too — the packages
                // ARE published; only the cross-repo announce needs a retry.
                Error::Announce(ae) => Classification::new(match ae {
                    AnnounceError::Io(io) => classify_io(io),
                    AnnounceError::Git { .. }
                    | AnnounceError::OwnerLookup { .. }
                    | AnnounceError::Fork { .. }
                    | AnnounceError::Client(_) => ExitCode::Unavailable,
                }),
            };
        }
    }
    Classification::new(ExitCode::Failure)
}

/// Maps an error chain to a process exit code.
///
/// Thin wrapper over [`classify`] for callers that only need the exit
/// code, kept so the many exit-code-only call sites across the codebase
/// need no changes.
pub fn classify_error(err: &anyhow::Error) -> ExitCode {
    classify(err).exit
}

/// Whether the error chain carries the [`StdoutPipeClosed`] sentinel — grim's
/// own stdout was closed by a downstream reader (`grim … | head`). `main.rs`
/// short-circuits to a silent exit 0 when this is true.
///
/// Walks the chain (not just the outermost error) so a `.context(...)` layer
/// cannot hide the sentinel. Deliberately does **not** match a bare
/// `io::Error` of kind `BrokenPipe`: a registry TCP or file EPIPE must stay a
/// loud failure, so only the sentinel tagged at grim's stdout write sites
/// ([`crate::cli::printer::tag_stdout_pipe`]) qualifies.
pub fn is_stdout_pipe_closed(err: &anyhow::Error) -> bool {
    err.chain().any(|c| c.downcast_ref::<StdoutPipeClosed>().is_some())
}

/// Map a config-tier error to a classification. `NotDiscovered` (no
/// `grimoire.toml` found walking up from cwd) carries the `NoConfig` reason;
/// every other arm — including the explicit-`--config`-path-missing `Io`
/// case right below it — carries none. The two `NotFound` arms look
/// similar but must never merge: `NotDiscovered` is "no project config
/// exists anywhere", `Io(NotFound)` is "the path the user named does not
/// exist" — only the former is a `NoConfig` refusal.
fn classify_config(err: &ConfigError) -> Classification {
    match &err.kind {
        ConfigErrorKind::TomlParse(_)
        | ConfigErrorKind::FileTooLarge { .. }
        | ConfigErrorKind::RegistryInvalid { .. }
        | ConfigErrorKind::TreeSeparatorInvalid { .. }
        | ConfigErrorKind::ClientsInvalid { .. } => Classification::new(ExitCode::ConfigError),
        ConfigErrorKind::NotDiscovered => Classification {
            exit: ExitCode::NotFound,
            reason: Some(ErrorReason::NoConfig),
        },
        ConfigErrorKind::ArtifactValueMissingRegistry { .. }
        | ConfigErrorKind::ArtifactValueInvalid { .. }
        | ConfigErrorKind::ArtifactValuePathInvalid { .. }
        | ConfigErrorKind::ArtifactValueRelativeInvalid { .. } => Classification::new(ExitCode::DataError),
        ConfigErrorKind::ConfigAlreadyExists => Classification::new(ExitCode::UsageError),
        // A missing config file is a NotFound contract case (docs:
        // "Explicit --config <path> not found" ⇒ 79), not a generic I/O
        // failure: discovery guards existence and the global config
        // absorbs absence, so config-tier ENOENT means an explicit path.
        // No reason: unlike `NotDiscovered` above, this is a wrong path the
        // user typed, not "no config anywhere" — a consumer must not
        // conflate the two.
        ConfigErrorKind::Io(io) if io.kind() == std::io::ErrorKind::NotFound => Classification::new(ExitCode::NotFound),
        ConfigErrorKind::Io(io) => Classification::new(classify_io(io)),
    }
}

/// Map a lock-tier error to a classification. `Locked` (another writer
/// holds the config-file advisory sidecar) carries the `Locked` reason —
/// the only reason [`ErrorReason::retryable`] reports `true` for; every
/// other arm carries none.
fn classify_lock(err: &LockError) -> Classification {
    match &err.kind {
        LockErrorKind::Locked => Classification {
            exit: ExitCode::TempFail,
            reason: Some(ErrorReason::Locked),
        },
        LockErrorKind::TomlParse(_)
        | LockErrorKind::TomlSerialize(_)
        | LockErrorKind::FileTooLarge { .. }
        | LockErrorKind::UnsupportedVersion { .. } => Classification::new(ExitCode::ConfigError),
        LockErrorKind::Io(io) => Classification::new(classify_io(io)),
    }
}

/// Map an OCI-access-tier error to an exit code.
fn classify_access(err: &AccessError) -> ExitCode {
    match &err.kind {
        AccessErrorKind::Authentication(_) => ExitCode::AuthError,
        AccessErrorKind::Registry(_) => ExitCode::Unavailable,
        AccessErrorKind::OfflineMiss => ExitCode::OfflineBlocked,
        AccessErrorKind::DigestMismatch { .. }
        | AccessErrorKind::InvalidManifest(_)
        | AccessErrorKind::OversizeBlob { .. } => ExitCode::DataError,
        AccessErrorKind::Io { source, .. } => classify_io(source),
    }
}

/// Map a resolution-tier error to a classification. The only arm carrying
/// a reason subtype is `StaleLock` — assigned right here, in the same
/// match arm that decides its exit code, so the two can never drift apart.
fn classify_resolve(err: &ResolveError) -> Classification {
    match &err.kind {
        ResolveErrorKind::TagNotFound | ResolveErrorKind::BundleNotFound => Classification::new(ExitCode::NotFound),
        ResolveErrorKind::AuthFailure(_) => Classification::new(ExitCode::AuthError),
        ResolveErrorKind::RegistryUnreachable(_) | ResolveErrorKind::ResolveTimeout => {
            Classification::new(ExitCode::Unavailable)
        }
        ResolveErrorKind::StaleLock { .. } => Classification {
            exit: ExitCode::DataError,
            reason: Some(ErrorReason::StaleLock),
        },
        ResolveErrorKind::BundleInvalid(_) | ResolveErrorKind::LocalSource(_) => {
            Classification::new(ExitCode::DataError)
        }
        // A bundle conflict is a misconfiguration of the user's own
        // declaration (two bundles disagree), not malformed external data.
        ResolveErrorKind::BundleConflict { .. } => Classification::new(ExitCode::ConfigError),
    }
}

/// Map an install-tier error to a classification. The two force-recoverable
/// refusals carry a reason subtype — assigned in the same match arms that
/// decide their exit codes, so the two can never drift apart (mirrors
/// [`classify_resolve`]'s `StaleLock` arm).
fn classify_install(err: &InstallError) -> Classification {
    match &err.kind {
        InstallErrorKind::BlobMissing => Classification::new(ExitCode::NotFound),
        InstallErrorKind::IntegrityMismatch { .. } => Classification {
            exit: ExitCode::DataError,
            reason: Some(ErrorReason::LocalModified),
        },
        InstallErrorKind::UntrackedDestination { .. } => Classification {
            exit: ExitCode::DataError,
            reason: Some(ErrorReason::UntrackedDestination),
        },
        InstallErrorKind::BlobDigestMismatch { .. }
        | InstallErrorKind::OversizeLayer { .. }
        | InstallErrorKind::MaterializeFailed(_)
        | InstallErrorKind::LocalSource(_)
        | InstallErrorKind::LocalContentChanged { .. } => Classification::new(ExitCode::DataError),
        InstallErrorKind::TargetIo { source, .. } => Classification::new(classify_io(source)),
        InstallErrorKind::UnsupportedClient(_) => Classification::new(ExitCode::ConfigError),
    }
}

/// Map an anchor-tier error to a classification. A traversal/escape is bad
/// on-disk state data (65); an I/O failure is I/O (74); an unclassifiable or
/// unresolvable anchor falls through to the generic failure (1). Only
/// `EscapedAnchor` carries a reason — a client needs to tell a containment
/// refusal apart from the forceable drift refusals that share exit 65.
fn classify_anchor(err: &AnchorError) -> Classification {
    match err {
        AnchorError::EscapedAnchor { .. } => Classification {
            exit: ExitCode::DataError,
            reason: Some(ErrorReason::AnchorEscape),
        },
        AnchorError::TraversalAttempt { .. } => Classification::new(ExitCode::DataError),
        AnchorError::Io { .. } => Classification::new(ExitCode::IoError),
        AnchorError::UnknownAnchor { .. } | AnchorError::AnchorRootAbsent { .. } => {
            Classification::new(ExitCode::Failure)
        }
    }
}

/// Map a skill-standard-tier error to an exit code. A spec/parse/mismatch
/// failure is bad input data (65); an I/O failure is I/O / NoPermission.
fn classify_skill(err: &SkillError) -> ExitCode {
    match &err.kind {
        SkillErrorKind::MissingSkillMd
        | SkillErrorKind::NameMismatch { .. }
        | SkillErrorKind::NameInvalid(_)
        | SkillErrorKind::FrontmatterParse(_)
        | SkillErrorKind::MissingFrontmatter
        | SkillErrorKind::MetadataInvalid(_)
        | SkillErrorKind::ValidationFailed(_)
        | SkillErrorKind::TooLarge(_)
        | SkillErrorKind::GitProvenance(_) => ExitCode::DataError,
        SkillErrorKind::Io(io) => classify_io(io),
    }
}

/// Map a release-tier error to an exit code. A missing tag or a refused tag
/// overwrite is a data error (65).
fn classify_release(err: &ReleaseError) -> ExitCode {
    match &err.kind {
        ReleaseErrorKind::MissingTag
        | ReleaseErrorKind::TagExists { .. }
        | ReleaseErrorKind::CascadeRequiresSemver { .. } => ExitCode::DataError,
        // A reserved-tag collision is a bad CLI/manifest argument, not bad
        // artifact data — usage error (64), mirroring the other tag-input gates.
        ReleaseErrorKind::ReservedTag { .. } => ExitCode::UsageError,
    }
}

/// Map a catalog-tier error to an exit code. A parse / unknown-version
/// failure is bad on-disk data (65); an I/O failure is I/O / NoPermission;
/// an OCI-access failure delegates to the access classifier.
fn classify_catalog(err: &CatalogError) -> ExitCode {
    match &err.kind {
        CatalogErrorKind::Parse(_) | CatalogErrorKind::UnsupportedVersion { .. } => ExitCode::DataError,
        CatalogErrorKind::Io(io) => classify_io(io),
        CatalogErrorKind::Access(ae) => classify_access(ae),
        // An index fetch failure is a remote-resource fault: the index
        // host is unreachable or served a non-success status.
        CatalogErrorKind::IndexFetch { .. } => ExitCode::Unavailable,
    }
}

/// Map an auth-tier error to an exit code. Store I/O delegates to the I/O
/// classifier; malformed on-disk config is bad data (65); a missing store
/// or config location is a configuration problem (78); helper failures map
/// per the underlying `docker_credential` error kind.
fn classify_auth(err: &AuthError) -> ExitCode {
    use docker_credential::CredentialRetrievalError as Helper;
    match err {
        AuthError::StoreIo { source, .. } => classify_io(source),
        AuthError::MalformedConfig { .. } => ExitCode::DataError,
        AuthError::NoCredentialStore | AuthError::NoConfigLocation => ExitCode::ConfigError,
        AuthError::HelperFailed { .. } => ExitCode::AuthError,
        // Login verification: a refused credential is an auth failure; an
        // unanswerable registry / token endpoint is a remote-resource
        // fault; an explicit --verify under offline is deliberate policy.
        AuthError::VerifyRejected { .. } => ExitCode::AuthError,
        AuthError::VerifyUnavailable { .. } => ExitCode::Unavailable,
        // A downgrade-refusing insecure realm means verification could not
        // complete safely — treat it like an unreachable endpoint.
        AuthError::VerifyInsecureRealm { .. } => ExitCode::Unavailable,
        AuthError::VerifyOffline => ExitCode::OfflineBlocked,
        AuthError::Helper(inner) => match inner {
            Helper::NotOnPath { .. } | Helper::UnsafePath { .. } => ExitCode::ConfigError,
            Helper::Timeout { .. } => ExitCode::TempFail,
            Helper::InvalidJson(_) | Helper::MalformedHelperResponse | Helper::CredentialDecodingError => {
                ExitCode::DataError
            }
            Helper::HelperCommunicationError => ExitCode::IoError,
            // OutputTooLarge / HelperFailure / the config-miss sentinels are
            // all treated as auth failures (the miss variants are never
            // wrapped in `Helper` — `map_helper_err`/`get_blocking` divert
            // them — but the arm keeps the match total).
            _ => ExitCode::AuthError,
        },
    }
}

/// `PermissionDenied` → `NoPermission` (77); any other I/O → `IoError` (74).
fn classify_io(io: &std::io::Error) -> ExitCode {
    if io.kind() == std::io::ErrorKind::PermissionDenied {
        ExitCode::NoPermission
    } else {
        ExitCode::IoError
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::Identifier;
    use crate::oci::digest::Digest;
    use crate::oci::identifier::error::IdentifierErrorKind;
    use crate::oci::pinned_identifier::PinnedIdentifier;

    #[test]
    fn identifier_error_classifies_as_data_error() {
        let inner = IdentifierError::new("bad", IdentifierErrorKind::MissingRegistry);
        let err: anyhow::Error = Error::from(inner).into();
        assert_eq!(classify_error(&err), ExitCode::DataError);
    }

    #[test]
    fn auth_errors_classify_per_kind() {
        let cases = [
            (
                AuthError::StoreIo {
                    path: std::path::PathBuf::from("/x/config.json"),
                    source: std::io::Error::other("disk full"),
                },
                ExitCode::IoError,
            ),
            (
                AuthError::StoreIo {
                    path: std::path::PathBuf::from("/x/config.json"),
                    source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
                },
                ExitCode::NoPermission,
            ),
            (
                AuthError::MalformedConfig {
                    path: std::path::PathBuf::from("/x/config.json"),
                    source: serde_json::from_str::<serde_json::Value>("{").expect_err("must err"),
                },
                ExitCode::DataError,
            ),
            (AuthError::NoCredentialStore, ExitCode::ConfigError),
            (AuthError::NoConfigLocation, ExitCode::ConfigError),
            (
                AuthError::HelperFailed {
                    helper: "test".to_string(),
                },
                ExitCode::AuthError,
            ),
            (
                AuthError::VerifyRejected {
                    registry: "ghcr.io".to_string(),
                },
                ExitCode::AuthError,
            ),
            (
                AuthError::VerifyUnavailable {
                    registry: "ghcr.io".to_string(),
                    source: None,
                },
                ExitCode::Unavailable,
            ),
            (AuthError::VerifyOffline, ExitCode::OfflineBlocked),
            (
                AuthError::VerifyInsecureRealm {
                    registry: "ghcr.io".to_string(),
                },
                ExitCode::Unavailable,
            ),
        ];
        for (inner, expected) in cases {
            let err: anyhow::Error = Error::from(inner).into();
            assert_eq!(classify_error(&err), expected);
        }
    }

    #[test]
    fn helper_error_kinds_classify_per_variant() {
        use docker_credential::CredentialRetrievalError as Helper;
        let cases = [
            (Helper::NotOnPath { name: "x".into() }, ExitCode::ConfigError),
            (Helper::Timeout { seconds: 30 }, ExitCode::TempFail),
            (Helper::MalformedHelperResponse, ExitCode::DataError),
            (Helper::CredentialDecodingError, ExitCode::DataError),
            (Helper::HelperCommunicationError, ExitCode::IoError),
        ];
        for (helper, expected) in cases {
            let err: anyhow::Error = Error::from(AuthError::Helper(helper)).into();
            assert_eq!(classify_error(&err), expected);
        }
    }

    #[test]
    fn command_login_errors_classify_per_kind() {
        let no_registry: anyhow::Error = Error::from(CommandError::NoLoginRegistry).into();
        assert_eq!(classify_error(&no_registry), ExitCode::ConfigError);

        let usage: anyhow::Error = Error::from(CommandError::LoginInput("bad input")).into();
        assert_eq!(classify_error(&usage), ExitCode::UsageError);
    }

    #[test]
    fn command_declare_conflict_classifies_as_usage_error() {
        // A same-name re-declare against a different identifier is a
        // conflicting invocation the caller fixes with `--name` — same
        // exit-code contract as `ConfigErrorKind::ConfigAlreadyExists`.
        let inner = CommandError::DeclareConflict {
            kind: crate::oci::ArtifactKind::Skill,
            name: "code-review".to_string(),
            existing: "ghcr.io/acme/code-review:stable".to_string(),
            requested: "ghcr.io/other/code-review:stable".to_string(),
        };
        let err: anyhow::Error = Error::from(inner).into();
        assert_eq!(classify_error(&err), ExitCode::UsageError);
    }

    #[test]
    fn digest_error_classifies_as_data_error() {
        let inner = DigestError::Invalid("nope".to_string());
        let err: anyhow::Error = Error::from(inner).into();
        assert_eq!(classify_error(&err), ExitCode::DataError);
    }

    #[test]
    fn pinned_identifier_error_classifies_as_data_error() {
        let id = Identifier::new_registry("cmake", "example.com");
        let inner = PinnedIdentifier::try_from(id).unwrap_err();
        let err: anyhow::Error = Error::from(inner).into();
        assert_eq!(classify_error(&err), ExitCode::DataError);
    }

    #[test]
    fn install_errors_classify_per_kind() {
        use crate::install::install_error::{InstallError, InstallErrorKind};

        // The two force-recoverable refusals carry a reason subtype so a
        // consumer can retry with `--force`; every other kind carries none.
        let cases = [
            (InstallErrorKind::BlobMissing, ExitCode::NotFound, None),
            (
                InstallErrorKind::IntegrityMismatch {
                    recorded: Digest::Sha256("a".repeat(64)),
                    actual: Digest::Sha256("b".repeat(64)),
                },
                ExitCode::DataError,
                Some(ErrorReason::LocalModified),
            ),
            (
                InstallErrorKind::UntrackedDestination {
                    client: "claude".to_string(),
                    path: std::path::PathBuf::from("/w/.claude/skills/x"),
                },
                ExitCode::DataError,
                Some(ErrorReason::UntrackedDestination),
            ),
            (
                InstallErrorKind::MaterializeFailed("bad tar".to_string()),
                ExitCode::DataError,
                None,
            ),
            (
                InstallErrorKind::OversizeLayer {
                    limit: 512 * 1024 * 1024,
                    actual: 512 * 1024 * 1024 + 1,
                },
                ExitCode::DataError,
                None,
            ),
            (
                InstallErrorKind::UnsupportedClient("vscode".to_string()),
                ExitCode::ConfigError,
                None,
            ),
            (
                InstallErrorKind::TargetIo {
                    path: std::path::PathBuf::from("/x"),
                    source: std::io::Error::other("disk full"),
                },
                ExitCode::IoError,
                None,
            ),
        ];
        for (kind, expected_exit, expected_reason) in cases {
            let err: anyhow::Error = Error::from(InstallError::without_reference(kind)).into();
            assert_eq!(
                classify(&err),
                Classification {
                    exit: expected_exit,
                    reason: expected_reason,
                }
            );
        }
    }

    #[test]
    fn error_reason_slugs_are_locked() {
        // The slugs are a 1.0-track wire contract (docs/src/json-interface.md
        // #error-reason): the JSON `reason` field renders through `Display`,
        // so a changed literal here is a breaking change for consumers.
        // `modified` deliberately matches the `grim status` state string.
        assert_eq!(ErrorReason::StaleLock.to_string(), "stale-lock");
        assert_eq!(ErrorReason::LocalModified.to_string(), "modified");
        assert_eq!(ErrorReason::UntrackedDestination.to_string(), "untracked-destination");
        assert_eq!(ErrorReason::NoConfig.to_string(), "no-config");
        assert_eq!(ErrorReason::Locked.to_string(), "locked");
        assert_eq!(ErrorReason::AnchorEscape.to_string(), "anchor-escape");
    }

    #[test]
    fn retryable_is_true_only_for_locked() {
        // Single source of truth: retryable is keyed on the reason, never on
        // the exit code — `AuthError::Helper::Timeout` also exits TempFail
        // (75) but carries no `ErrorReason` at all, so it can never reach
        // this method.
        assert!(!ErrorReason::StaleLock.retryable());
        assert!(!ErrorReason::LocalModified.retryable());
        assert!(!ErrorReason::UntrackedDestination.retryable());
        assert!(!ErrorReason::NoConfig.retryable());
        assert!(ErrorReason::Locked.retryable());
        // A containment refusal is a decision about the filesystem's shape,
        // not a transient condition — retrying the identical command can only
        // fail identically.
        assert!(!ErrorReason::AnchorEscape.retryable());
    }

    /// A1: `forceable` is keyed on the reason, and this variant-by-variant
    /// enumeration is its ONLY registration guard — `matches!` gives no
    /// compile-time backstop, so a new reason added without a line here would
    /// silently default to non-forceable (a lost dialog) or, worse, be folded
    /// into the `matches!` arm and silently offer an override that cannot work.
    #[test]
    fn forceable_is_true_only_for_the_two_drift_refusals() {
        assert!(!ErrorReason::StaleLock.forceable());
        assert!(ErrorReason::LocalModified.forceable());
        assert!(ErrorReason::UntrackedDestination.forceable());
        assert!(!ErrorReason::NoConfig.forceable());
        assert!(!ErrorReason::Locked.forceable());
        // NEVER forceable: `--force` does not bypass containment. Offering an
        // override on a security refusal trains click-through, and exit 65
        // covers both this and the forceable refusals — which is exactly why
        // the client must key on the reason, not the exit code.
        assert!(!ErrorReason::AnchorEscape.forceable());
    }

    /// A2: an escaped anchor classifies as bad on-disk state data (65) AND
    /// carries the `anchor-escape` reason, so a client can tell it apart from
    /// the forceable drift refusals that share exit 65.
    #[test]
    fn escaped_anchor_classifies_reason_as_anchor_escape() {
        let err: anyhow::Error = Error::from(AnchorError::EscapedAnchor {
            anchor: crate::install::path_anchor::PathAnchor::ClaudeRoot,
            resolved: std::path::PathBuf::from("/elsewhere/skills/demo-skill"),
        })
        .into();
        assert_eq!(
            classify(&err),
            Classification {
                exit: ExitCode::DataError,
                reason: Some(ErrorReason::AnchorEscape),
            }
        );
    }

    /// A2 companion: a tampered stored `..` is a DIFFERENT failure — no
    /// filesystem is involved and there is no user layout to accommodate — so
    /// it keeps exit 65 with no reason. Merging the two would let a client
    /// present a traversal attempt as a recoverable relocated install.
    #[test]
    fn traversal_attempt_carries_no_reason() {
        let err: anyhow::Error = Error::from(AnchorError::TraversalAttempt {
            relative: "../../etc/passwd".to_string(),
        })
        .into();
        assert_eq!(
            classify(&err),
            Classification {
                exit: ExitCode::DataError,
                reason: None,
            }
        );
    }

    #[test]
    fn config_not_discovered_classifies_reason_as_no_config() {
        let err: anyhow::Error = Error::from(ConfigError::new(
            std::path::PathBuf::from("/w"),
            ConfigErrorKind::NotDiscovered,
        ))
        .into();
        assert_eq!(
            classify(&err),
            Classification {
                exit: ExitCode::NotFound,
                reason: Some(ErrorReason::NoConfig),
            }
        );
    }

    #[test]
    fn lock_locked_classifies_reason_as_locked() {
        let err: anyhow::Error = Error::from(LockError::new(
            std::path::PathBuf::from("/w/grimoire.toml.lock"),
            LockErrorKind::Locked,
        ))
        .into();
        assert_eq!(
            classify(&err),
            Classification {
                exit: ExitCode::TempFail,
                reason: Some(ErrorReason::Locked),
            }
        );
        assert!(
            classify(&err).reason.is_some_and(ErrorReason::retryable),
            "lock contention must be marked retryable"
        );
    }

    #[test]
    fn anchor_errors_classify_per_kind() {
        use crate::install::path_anchor::{AnchorError, PathAnchor};

        let cases = [
            (
                AnchorError::TraversalAttempt {
                    relative: "../escape".to_string(),
                },
                ExitCode::DataError,
            ),
            (
                AnchorError::EscapedAnchor {
                    anchor: PathAnchor::Workspace,
                    resolved: std::path::PathBuf::from("/outside"),
                },
                ExitCode::DataError,
            ),
            (
                AnchorError::Io {
                    path: std::path::PathBuf::from("/x"),
                    source: std::io::Error::other("disk full"),
                },
                ExitCode::IoError,
            ),
            (
                AnchorError::UnknownAnchor {
                    path: std::path::PathBuf::from("/other/path"),
                },
                ExitCode::Failure,
            ),
            (
                AnchorError::AnchorRootAbsent {
                    anchor: PathAnchor::ClaudeRoot,
                },
                ExitCode::Failure,
            ),
        ];
        for (inner, expected) in cases {
            let err: anyhow::Error = Error::from(inner).into();
            assert_eq!(classify_error(&err), expected);
        }
    }

    #[test]
    fn skill_git_provenance_error_classifies_as_data_error() {
        use crate::oci::git_provenance::GitProvenanceError;
        // The `--git` opt-in surfaces a missing `git` as a path-attributed
        // SkillError; it must classify as a DataError (65), never a generic
        // failure — the user explicitly asked for provenance.
        let inner = SkillError::new(
            "/w/skill",
            SkillErrorKind::GitProvenance(GitProvenanceError::GitNotFound),
        );
        let err: anyhow::Error = Error::from(inner).into();
        assert_eq!(classify_error(&err), ExitCode::DataError);
    }

    #[test]
    fn oversize_blob_classifies_as_data_error() {
        // A registry serving more bytes than its descriptor declared is
        // hostile/malformed data (CWE-770) — same tier as a digest mismatch.
        let id = Identifier::parse("ghcr.io/acme/x:stable").unwrap();
        let err: anyhow::Error = Error::from(AccessError::with_identifier(
            id,
            AccessErrorKind::OversizeBlob { limit: 8 * 1024 * 1024 },
        ))
        .into();
        assert_eq!(classify_error(&err), ExitCode::DataError);
    }

    #[test]
    fn release_errors_classify_as_data_error() {
        // Every release-tier kind — missing tag, refused overwrite, and the
        // new --cascade-on-non-semver assertion — is bad input data (65).
        use crate::oci::release::{ReleaseError, ReleaseErrorKind};
        let cases = [
            ReleaseErrorKind::MissingTag,
            ReleaseErrorKind::TagExists {
                tag: "1.2.3".to_string(),
                existing: "sha256:a".to_string(),
                new: "sha256:b".to_string(),
            },
            ReleaseErrorKind::CascadeRequiresSemver {
                tag: "canary".to_string(),
            },
        ];
        for kind in cases {
            let err: anyhow::Error = Error::from(ReleaseError::without_reference(kind)).into();
            assert_eq!(classify_error(&err), ExitCode::DataError);
        }
    }

    #[test]
    fn reserved_tag_error_classifies_as_usage_error() {
        // W1: a user-supplied tag colliding with grim's reserved internal
        // namespace (the `__grimoire` companion tag) is a bad CLI/manifest
        // argument — a usage error (64), NOT bad artifact data (65). This locks
        // the exit-code contract for the reserved-tag write guard
        // (`validate_user_tag`), so the release/publish call sites that route
        // through it exit 64 on `<repo>:__grimoire`.
        use crate::oci::release::{ReleaseError, ReleaseErrorKind};
        let err: anyhow::Error = Error::from(ReleaseError::without_reference(ReleaseErrorKind::ReservedTag {
            tag: "__grimoire".to_string(),
        }))
        .into();
        assert_eq!(classify_error(&err), ExitCode::UsageError);
    }

    #[test]
    fn stale_lock_classifies_reason_as_stale_lock() {
        use crate::oci::ArtifactKind;
        use crate::oci::reference::ArtifactRef;
        let reference = ArtifactRef::registry(
            ArtifactKind::Skill,
            "code-review",
            Identifier::parse("ghcr.io/acme/code-review:stable").unwrap(),
        );
        let err: anyhow::Error = Error::from(ResolveError::new(
            reference,
            ResolveErrorKind::StaleLock {
                previous_hash: "sha256:aaa".to_string(),
                current_hash: "sha256:bbb".to_string(),
            },
        ))
        .into();
        assert_eq!(classify_error(&err), ExitCode::DataError);
        assert_eq!(
            classify(&err),
            Classification {
                exit: ExitCode::DataError,
                reason: Some(ErrorReason::StaleLock),
            }
        );
    }

    #[test]
    fn other_errors_carry_no_reason() {
        // A representative data error and a not-found error both classify to
        // an exit code but to no reason subtype — the field must stay absent.
        let data: anyhow::Error = Error::from(DigestError::Invalid("nope".to_string())).into();
        assert_eq!(classify(&data).reason, None);

        let usage: anyhow::Error = Error::from(CommandError::LoginInput("bad input")).into();
        assert_eq!(classify(&usage).reason, None);

        // A non-Grimoire error (the classify_error fall-through case) also
        // has no reason.
        assert_eq!(classify(&anyhow::anyhow!("unrelated")).reason, None);
    }

    #[test]
    fn reason_survives_anyhow_context_layers() {
        use crate::oci::ArtifactKind;
        use crate::oci::reference::ArtifactRef;
        let reference = ArtifactRef::registry(
            ArtifactKind::Skill,
            "code-review",
            Identifier::parse("ghcr.io/acme/code-review:stable").unwrap(),
        );
        let err = anyhow::Error::from(Error::from(ResolveError::new(
            reference,
            ResolveErrorKind::StaleLock {
                previous_hash: "sha256:aaa".to_string(),
                current_hash: "sha256:bbb".to_string(),
            },
        )))
        .context("while re-resolving the lock");
        assert_eq!(classify(&err).reason, Some(ErrorReason::StaleLock));
    }

    #[test]
    fn classification_survives_anyhow_context_layers() {
        let inner = DigestError::Invalid("nope".to_string());
        let err = anyhow::Error::from(Error::from(inner)).context("while resolving lock");
        assert_eq!(classify_error(&err), ExitCode::DataError);
    }

    #[test]
    fn is_stdout_pipe_closed_detects_sentinel_bare_and_wrapped() {
        let bare: anyhow::Error = anyhow::Error::new(StdoutPipeClosed);
        assert!(is_stdout_pipe_closed(&bare), "bare sentinel must match");

        let wrapped = anyhow::Error::new(StdoutPipeClosed).context("while rendering the report");
        assert!(
            is_stdout_pipe_closed(&wrapped),
            "sentinel behind a .context(...) layer must still match"
        );
    }

    #[test]
    fn is_stdout_pipe_closed_false_for_bare_broken_pipe_io_error() {
        // Regression lock for the network-EPIPE false-positive design
        // decision: a bare io::Error of kind BrokenPipe (a registry TCP write
        // or file EPIPE) must NOT be treated as grim's own stdout closing.
        // Only the sentinel tagged at grim's stdout write sites qualifies, so
        // a mid-push connection reset still fails loudly instead of
        // masquerading as a graceful exit 0.
        let io = std::io::Error::from(std::io::ErrorKind::BrokenPipe);
        let err: anyhow::Error = io.into();
        assert!(
            !is_stdout_pipe_closed(&err),
            "a bare BrokenPipe io::Error must not match"
        );
    }

    #[test]
    fn is_stdout_pipe_closed_false_for_other_and_unrelated_errors() {
        let other: anyhow::Error = std::io::Error::from(std::io::ErrorKind::PermissionDenied).into();
        assert!(!is_stdout_pipe_closed(&other), "another io kind must not match");
        assert!(
            !is_stdout_pipe_closed(&anyhow::anyhow!("unrelated")),
            "an unrelated error must not match"
        );
    }

    #[test]
    fn unclassified_error_falls_through_to_failure() {
        // Locks the documented v1 fall-through behaviour: any error that is
        // not a Grimoire `Error` maps to Failure (1), never a semantic code.
        let err = anyhow::anyhow!("some unrelated failure");
        assert_eq!(classify_error(&err), ExitCode::Failure);

        // A bare std::io::Error is also unclassified in Phase 1.
        let io = std::io::Error::other("disk gone");
        let err: anyhow::Error = io.into();
        assert_eq!(classify_error(&err), ExitCode::Failure);
    }

    #[test]
    fn config_io_not_found_classifies_as_not_found() {
        // Contract (docs "Exit codes"): a missing explicit `--config
        // <path>` exits 79 (NotFound), not 74 (IoError). Config-tier
        // ENOENT only arises from an explicit path — discovery checks
        // existence and the global config absorbs absence.
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let err: anyhow::Error = Error::from(ConfigError::new(
            std::path::PathBuf::from("/x/grimoire.toml"),
            ConfigErrorKind::Io(io),
        ))
        .into();
        assert_eq!(classify_error(&err), ExitCode::NotFound);
        // CRITICAL negative: an explicit `--config <path>` that does not
        // exist is a wrong path, not "no config anywhere" — it must NEVER
        // carry the `NoConfig` reason `ConfigErrorKind::NotDiscovered` gets.
        assert_eq!(
            classify(&err).reason,
            None,
            "explicit --config path missing must stay reason-less, unlike NotDiscovered"
        );

        // Any other config-tier I/O failure keeps the generic mapping.
        let io = std::io::Error::other("disk gone");
        let err: anyhow::Error = Error::from(ConfigError::new(
            std::path::PathBuf::from("/x/grimoire.toml"),
            ConfigErrorKind::Io(io),
        ))
        .into();
        assert_eq!(classify_error(&err), ExitCode::IoError);
    }

    #[test]
    fn announce_client_build_failure_classifies_as_unavailable() {
        // Regression: `forge::client()` used to be wrapped in a bare
        // `anyhow::anyhow!` at the publish call site, which `classify`
        // never matches and silently falls through to exit 1 — contradicting
        // the documented "git/API failures exit 69" invariant. Routing the
        // failure through `AnnounceError::Client` must classify as
        // Unavailable like every other announce-tier remote-resource fault.
        let bad_url = reqwest::Client::new()
            .get("not a valid url")
            .build()
            .expect_err("an unparseable URL must fail to build a request");
        let err: anyhow::Error = Error::from(AnnounceError::Client(bad_url)).into();
        assert_eq!(classify_error(&err), ExitCode::Unavailable);
    }

    #[test]
    fn from_impls_round_trip_into_top_level_error() {
        let _: Error = DigestError::Invalid("x".into()).into();
        let _: Error = IdentifierError::new("x", IdentifierErrorKind::Empty).into();
        let id = Identifier::new_registry("c", "e");
        let _: Error = PinnedIdentifier::try_from(id).unwrap_err().into();
        // Smoke: the Digest type stays reachable through the error module's
        // re-export path used by callers building pinned identifiers.
        let _ = Digest::Sha256("a".repeat(64));
    }
}
