// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The machine-local dispatch table (C-006) — the one file that says which
//! hooks are armed for which root, and the only runtime input `grim hook run`
//! reads.
//!
//! # The table is located by argv, never by the environment (B1)
//!
//! `$GRIM_HOME/hooks/dispatch.json` is where grim *writes* it. It is **not**
//! how the runtime *finds* it: [`crate::env::grim_home`] returns the
//! environment value verbatim, with no absoluteness check and a **relative**
//! `.grimoire` fallback when `HOME` is unset, and the CWD of a `grim hook run`
//! spawned by a client is the workspace. WP-P0 executed both variants against
//! the shipped 0.13.0 binary: `GRIM_HOME=.devcontainer/tools/grim` and
//! `env -u HOME -u GRIM_HOME` each resolve repo-relative. A hostile repo that
//! ships `.envrc` / `.mise.toml` / devcontainer `containerEnv` and commits its
//! own `dispatch.json` would then have grim read the attacker's table on the
//! victim's next tool call, with every launcher-path control intact and
//! *downstream* of it — the CWE-426 class decision I/P closed at the launcher
//! path, reappearing one layer down where nothing stands in front of it.
//!
//! So the registration bakes the **resolved absolute** table path into the
//! launcher argv (`--table '<abs>'`, chosen over the equivalent
//! `--home '<abs>'` — see the module note below), the runtime refuses a
//! non-absolute `--table`, and `sync_config` refuses to arm at all when
//! `grim_home()` is relative or resolves inside the workspace being installed
//! for (C-017 causes 1–2, [`super::hook_registrar`]).
//!
//! ## Why `--table`, not `--home` (owed choice, settled 2026-08-17)
//!
//! WP-P0 states the two forms are equivalent for B1's purpose and leaves the
//! pick to this WP. `--table` wins on least authority: it passes exactly the
//! one path the runtime needs, where `--home` passes a **directory** from which
//! the runtime could derive the launcher, the payload trees, the root-key file
//! and the content store. Every one of those derivations would be a second
//! runtime input smuggled in behind one argv value, and C-007's "the table is
//! the sole runtime input" would stop being checkable by reading the argv.
//!
//! # The root key is an opaque token (B3)
//!
//! The table is keyed by an unforgeable per-install token, never by `global`
//! and never by an absolute workspace path. Both of those are attacker-
//! supplyable: `global` is a fixed literal, and a workspace path is usually
//! guessable. Grim is not the only writer of client hook configs — a hostile
//! repo can commit **its own** registration invoking the victim's real launcher
//! (`${HOME}/.grimoire/hooks/bin/grim-hook` expands on Claude, WP-B § 6.1) with
//! an attacker-chosen event, matcher and root — so a guessable key lets it fire
//! the victim's globally-armed `gatekeeper`, whose `allow` verdict suppresses
//! the client's own tool-approval prompt (T3 to fire, T4 to profit). Checking
//! the root against the invoking workspace is exactly what C-007 forbids, so
//! the fix is an unforgeable key rather than a validated one, and an unknown
//! token degrades to no-match ⇒ exit 0.
//!
//! ## Why an HMAC, not 128 random bits (owed choice, settled 2026-08-17)
//!
//! WP-P0 offers either. The HMAC wins because the token must be **derivable on
//! demand**: re-materialization has to find its own entry, so the token for a
//! given root must be the same on every run — and with stored randomness that
//! means a path→token map, a second piece of mutable state whose loss or
//! partial write strands a workspace's hooks with no way to name the orphaned
//! record. `HMAC(key, root)` needs no map: the key lives in `$GRIM_HOME`
//! (`root-key`, mode `0o600`), never in a repository, and the token is a pure
//! function of it and the root.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::lock::advisory_lock::AdvisoryFileLock;
use crate::lock::lock_error::LockErrorKind;
use crate::oci::hook::{CanonicalEvent, HookHandler, HookPayloadMode, HookTier, MATCHER_MAX_BYTES};
use crate::store::atomic_write::atomic_write;

/// Directory under `$GRIM_HOME` holding everything hook-related: the generated
/// launcher (`bin/grim-hook`), the dispatch table, the root-key file, and the
/// per-artifact payload trees.
///
/// `bin` and `dispatch.json` are `RESERVED_ARTIFACT_NAMES`
/// (`crate::oci::hook`) precisely because payloads are siblings here.
pub const HOOKS_DIR: &str = "hooks";

/// The dispatch table's file name — and one of the reserved artifact names.
pub const DISPATCH_FILE: &str = "dispatch.json";

/// Subdirectory of [`HOOKS_DIR`] holding the **project-scope** payload trees,
/// one subdirectory per workspace.
///
/// A [`RESERVED_ARTIFACT_NAMES`](crate::oci::hook::RESERVED_ARTIFACT_NAMES)
/// entry for the same reason `bin` is: a *global* payload materialized over it
/// would shadow every workspace's payload root at once.
pub const PAYLOAD_DIR: &str = "payload";

/// The machine-local HMAC key file backing [`root_token`].
///
/// Lives beside the table under `$GRIM_HOME`, **never** in a repository: a
/// repo-carried key would let a hostile repo compute the victim's tokens, which
/// is the whole property B3 buys.
pub const ROOT_KEY_FILE: &str = "root-key";

/// The machine-local key's length — 256 bits, HMAC-SHA256's block-optimal size.
///
/// A key at least as long as the digest is what makes the MAC's security bound
/// the digest's rather than the key's.
const ROOT_KEY_BYTES: usize = 32;

/// How much of the MAC tag the token carries — 128 bits, 32 hex characters.
///
/// Truncating a MAC to 128 bits is standard practice (RFC 2104 § 5 permits it
/// down to half the output) and 2^128 preimage work is far beyond what a
/// guessing attacker has. The remaining bytes are discarded, never stored.
const ROOT_TOKEN_BYTES: usize = 16;

/// The dispatch table's schema version, read **before** anything else (W2).
///
/// The reader contract is asymmetric on purpose: an unrecognized value —
/// including a *newer* one after a grim downgrade — is treated as an **empty
/// table**, one log line, exit 0. Codex's own behaviour is the cautionary
/// precedent (WP-B § 2.2: one bad key silently drops every hook in the file),
/// and I3 makes "the feature is off" the only acceptable degrade direction.
pub const DISPATCH_SCHEMA: u32 = 1;

/// Hard ceiling on the table read (W2 (c)).
///
/// A build-time cap does not bind a file on disk, so the runtime re-checks both
/// this and `MATCHER_MAX_BYTES` at read time. 1 MiB is far above any real table
/// (one JSON object per armed hook) and far below anything that costs the hot
/// path a measurable read.
pub const MAX_TABLE_BYTES: u64 = 1 << 20;

/// Mode for `$GRIM_HOME/hooks/` — owner-only (W3).
///
/// The shared-`$GRIM_HOME` case (`subsystem-file-structure.md` contemplates one
/// across machines and containers) puts the **arming authority** in another
/// trust domain, not merely a record. `atomic_write` caps modes at `0o644` and
/// preserves an existing capped mode (`mode & 0o644`), so a `0o600` table
/// *stays* `0o600` across writes — a tighter mode is implementable with the
/// shipped primitive and costs nothing.
pub const HOOKS_DIR_MODE: u32 = 0o700;

/// Mode for the dispatch table and the root-key file — owner-only (W3).
pub const TABLE_MODE: u32 = 0o600;

/// `$GRIM_HOME/hooks`.
pub fn hooks_dir(grim_home: &Path) -> PathBuf {
    grim_home.join(HOOKS_DIR)
}

/// `$GRIM_HOME/hooks/dispatch.json` — the path baked into the launcher argv.
pub fn dispatch_path(grim_home: &Path) -> PathBuf {
    hooks_dir(grim_home).join(DISPATCH_FILE)
}

/// `$GRIM_HOME/hooks/root-key` — the machine-local HMAC key backing
/// [`root_token`].
pub fn root_key_path(grim_home: &Path) -> PathBuf {
    hooks_dir(grim_home).join(ROOT_KEY_FILE)
}

/// The **semantic** root a set of hooks is armed for.
///
/// Distinct from the [`RootToken`] that reaches the argv, and that separation is
/// the point: grim reasons about scopes, the wire carries an opaque key. Two
/// variants because there are exactly two scopes, and neither is `$PWD`, the
/// envelope `cwd`, or a walk-up (C-006, C-007 — the derivation discipline WP-P0
/// attacked and found sound; B3 is about the key being *guessable*, not derived).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootScope<'a> {
    /// The machine-local scope — every client's global registration.
    Global,
    /// One workspace, by absolute path — claude project scope only in v1.
    Workspace(&'a Path),
}

impl RootScope<'_> {
    /// The human-readable form stored **beside** the token in the table for
    /// diagnostics, and the HMAC message [`root_token`] keys on.
    ///
    /// `global` for [`Self::Global`]; the absolute workspace path otherwise.
    /// The literal is a message to a keyed MAC, not a lookup key, so it being
    /// guessable costs nothing — the key is what the attacker lacks.
    pub fn display(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Self::Global => std::borrow::Cow::Borrowed("global"),
            Self::Workspace(path) => path.to_string_lossy(),
        }
    }
}

/// The `$GRIM_HOME`-relative location of `name`'s payload tree for `root` —
/// `hooks/<name>` globally, `hooks/payload/<workspace-key>/<name>` for a
/// workspace.
///
/// Forward-slash, `Normal`-only components, so it is directly usable as an
/// [`AnchoredPath`](super::path_anchor::AnchoredPath) `relative` under
/// [`PathAnchor::GrimHome`](super::path_anchor::PathAnchor::GrimHome) — which is
/// how the arming path gets the containment guard rather than joining by hand
/// (`name` reaches the arming side out of an install record, so it is untrusted
/// input; see [`payload_dir`]).
pub fn payload_relative(root: RootScope<'_>, name: &str) -> String {
    match root {
        RootScope::Global => format!("{HOOKS_DIR}/{name}"),
        RootScope::Workspace(workspace) => {
            format!("{HOOKS_DIR}/{PAYLOAD_DIR}/{}/{name}", workspace_key(workspace))
        }
    }
}

/// The absolute directory grim materializes `name`'s payload tree into —
/// under `$GRIM_HOME` at **both** scopes.
///
/// # Why the payload left the workspace (SEC-1)
///
/// A project-scope payload used to live at `<workspace>/.grimoire/hooks/<name>`,
/// on the reasoning that "nothing here is armable — the *registration* is".
/// That reasoning was false, and WP-R falsified it by execution: a workspace
/// carrying its **own committed** `.grimoire/state.json` *plus* payload armed on
/// a fresh machine with **no network fetch and no install history**, as soon as
/// the victim's *global* config trusted the registry the committed record named.
/// The integrity gate compares the *recorded* hash against the on-disk payload
/// and an attacker who ships a repository supplies both, so it short-circuits to
/// `AlreadyInstalled` without fetching; convergence then read `hook.toml` out of
/// the record's own directory and armed it. The payload **is** the code the
/// dispatcher executes, so invariant **I1** covers it exactly as it covers the
/// launcher and the table (attacker **T3**; **I2**'s "approved the right thing,
/// checked at the wrong time").
///
/// **This function is only half the fix.** The other half is that
/// [`super::hook_registrar`]'s `desired_entries` derives the directory it reads
/// `hook.toml` from through *this* function rather than from the install
/// record's stored target. Relocating the directory without moving that read
/// would leave the hole exactly as open as it was.
///
/// # Why the key is a plain path digest and NOT [`root_token`]
///
/// The dispatch table keys the same workspace by an opaque HMAC token, and
/// reusing it here would have kept one spelling of "which workspace". It is the
/// wrong choice, for two independent reasons:
///
/// 1. **It would disclose the token.** A recorded install target is ordinary
///    user-visible data: it is written into `state.json`, printed by
///    `grim install`, and reported by `grim status --format json`. Putting the
///    token in the path publishes it — and B3's whole property is that a hostile
///    repository *cannot* compute the victim's token, because a registration
///    naming a known `--root` fires the victim's already-armed hooks from any
///    workspace (T3 to fire, T4 to profit). A path key must be safe to print;
///    the wire key must not be printable at all. They are different jobs and
///    they get different derivations. The **semantic** root
///    ([`RootScope`]) stays the one shared spelling.
/// 2. **It is not derivable without side effects.** [`root_token`] reads —
///    and on first use *creates*, `0o600` inside a `0o700` directory — the
///    machine-local key. Install destinations are computed by read-only
///    commands too (`grim status`'s materialization-drift check runs through the
///    same seam), and a read-only command must not mint arming key material.
///
/// So: SHA-256 of the workspace path, hex — the same formula
/// [`InstallState::legacy_project_path`](super::install_state::InstallState::legacy_project_path)
/// uses to hash a path into a `$GRIM_HOME` segment, and pure, so the seam stays
/// infallible. Guessability costs nothing here: `$GRIM_HOME` is not writable by
/// a repository, so knowing a payload path grants nothing.
pub fn payload_dir(grim_home: &Path, root: RootScope<'_>, name: &str) -> PathBuf {
    grim_home.join(payload_relative(root, name))
}

/// The per-workspace path segment under [`PAYLOAD_DIR`] — SHA-256 of the
/// workspace path, hex.
///
/// Non-UTF-8 components go through `to_string_lossy`, matching
/// `legacy_project_path`'s formula. Two paths differing only in a non-UTF-8
/// byte would collide, which is unreachable in practice: a path component that
/// is not valid UTF-8 is already rejected at install-record store time
/// (`AnchorError::UnknownAnchor`), so no such workspace ever gets this far.
fn workspace_key(workspace: &Path) -> String {
    // Function-local so the `Digest` trait never enters the module scope, where
    // `hmac::Mac` provides same-named `update`/`finalize` methods.
    use sha2::Digest as _;
    let mut hasher = Sha256::new();
    hasher.update(workspace.to_string_lossy().as_bytes());
    hex::encode(hasher.finalize())
}

/// An opaque per-install root key: `HMAC-SHA256(machine key, root)`, hex,
/// truncated to 128 bits.
///
/// Newtype rather than `String` so it cannot be confused with the
/// human-readable root at a call site — which is the confusion B3 is about.
/// **Never** `global`, never a path.
/// # Why this deliberately does NOT derive `Deserialize`
///
/// It used to, and that quietly falsified the safety argument stated on
/// [`Vendor::hook_registration`](super::vendor::Vendor::hook_registration):
/// serde gives a private-field newtype a **transparent** deserializer, so
/// `serde_json::from_str::<RootToken>("\"anything\"")` minted one — making the
/// type exactly as forgeable from a `&str` as the absolute path it replaced,
/// which is the property its own existence is meant to deny. Found by the WP-J1
/// author while looking for a legitimate way to build a probe token.
///
/// The dispatch table still has to round-trip its keys, so the capability lives
/// on the one field that needs it ([`roots_map`]) rather than on the type, where
/// any caller could reach it. `Serialize` stays: writing a token is not minting
/// one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct RootToken(String);

impl RootToken {
    /// The token's wire form — 32 lowercase hex characters.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A token with a caller-chosen value, **tests only**.
    ///
    /// `#[cfg(test)]`, so it does not exist in the shipped binary and cannot
    /// widen the type for production callers. It exists because a byte-exact
    /// assertion on a generated command string needs a known token, while
    /// [`root_token`] derives one from a per-`$GRIM_HOME` random key.
    ///
    /// Before this existed, a test helper minted tokens with
    /// `serde_json::from_value` — the very transparent-newtype route that
    /// falsified the type's safety claim. Removing `Deserialize` broke that
    /// helper, which is how the hole was closed rather than merely documented.
    #[cfg(test)]
    pub fn for_test(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl std::fmt::Display for RootToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// [`root_token`] for a **read-only** caller: never creates the machine key.
///
/// `Ok(None)` when no key file exists yet, which is exactly the case where
/// nothing on this machine can be armed — no key means no token was ever
/// written into a dispatch table.
///
/// # Why this exists as a separate function
///
/// [`root_token`] *mints* the key on first use, `0o600` inside a `0o700`
/// directory. That is correct for the install path and wrong for every
/// read-only command: `grim status` reports arming, and a report that creates
/// arming key material as a side effect would break the structural guarantee
/// that `status`/`search`/`context` cannot touch the arming path. It would also
/// make the caller fallible in a place that has no honest failure mode — a
/// report cannot refuse to render because a key could not be written.
///
/// The same reasoning kept the payload directory off the root token: a
/// read-only command must be able to ask every question a report needs to
/// answer without writing anything.
///
/// # Errors
///
/// An I/O failure *reading* the key file, or a key file too short to be usable.
/// A missing file is `Ok(None)`, not an error.
pub fn existing_root_token(grim_home: &Path, root: RootScope<'_>) -> io::Result<Option<RootToken>> {
    let Some(key) = read_root_key(&root_key_path(grim_home))? else {
        return Ok(None);
    };
    Ok(Some(token_from_key(&key, root)?))
}

/// Derive `root`'s token under the machine-local key, creating that key on
/// first use.
///
/// Stable across re-installs of the same workspace — which it **must** be:
/// re-materialization has to find its own entry, and a token that moved would
/// leave an armed record no later run can name. Rotating the key is therefore a
/// disarm-everything operation, not a maintenance action.
///
/// # Errors
///
/// An I/O failure creating `$GRIM_HOME/hooks/` or reading/writing the key file.
/// A key file that exists but is too short to be a key is an
/// [`io::ErrorKind::InvalidData`] rather than a silent regeneration: silently
/// re-keying would orphan every armed registration on the machine.
pub fn root_token(grim_home: &Path, root: RootScope<'_>) -> io::Result<RootToken> {
    let key = machine_key(grim_home)?;
    token_from_key(&key, root)
}

/// The HMAC itself, shared by the creating and non-creating entry points so the
/// two can never derive different tokens from one key.
fn token_from_key(key: &[u8], root: RootScope<'_>) -> io::Result<RootToken> {
    // `new_from_slice` is infallible for HMAC (any key length is accepted —
    // RFC 2104 hashes an over-long key and zero-pads a short one), but the
    // signature is fallible, so the error is mapped rather than unwrapped.
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "hook root key is not a usable HMAC key"))?;
    mac.update(root.display().as_bytes());
    let tag = mac.finalize().into_bytes();
    Ok(RootToken(hex::encode(&tag[..ROOT_TOKEN_BYTES])))
}

/// Create `$GRIM_HOME/hooks` (and its parents) with owner-only permissions.
///
/// Separate from [`atomic_write`]'s own `create_dir_all`, because that one
/// leaves the directory at the process umask and W3 wants `0o700` on the
/// directory that holds the arming authority. Idempotent, and it never
/// *loosens* an existing mode — it only ever narrows a fresh one, so a user who
/// deliberately widened the directory is not silently overridden on a path that
/// has no refusal for it yet (C-017 cause 5 is deferred with W3).
fn ensure_hooks_dir(grim_home: &Path) -> io::Result<PathBuf> {
    let dir = hooks_dir(grim_home);
    let existed = dir.exists();
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    if !existed {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(HOOKS_DIR_MODE))?;
    }
    Ok(dir)
}

/// Read (or create) the machine-local HMAC key.
///
/// 32 bytes from the OS CSPRNG (`getrandom`) on first use, written `0o600`
/// inside a `0o700` directory. A non-cryptographic source (`fastrand`, a clock,
/// a pid) is not an option: a guessable key is B3 with extra steps.
///
/// The create path is `create_new` (`O_EXCL`), **not** [`atomic_write`], for two
/// independent reasons. `atomic_write` caps at `0o644`, which would publish the
/// key to every local reader (T5); and `O_EXCL` makes a concurrent first install
/// converge — the loser of the race adopts the winner's key instead of replacing
/// it, which matters because replacing it orphans every token already written
/// into a registration.
///
/// # Errors
///
/// An I/O failure, or [`io::ErrorKind::InvalidData`] for a short/corrupt key.
fn machine_key(grim_home: &Path) -> io::Result<Vec<u8>> {
    let path = root_key_path(grim_home);
    if let Some(key) = read_root_key(&path)? {
        return Ok(key);
    }

    ensure_hooks_dir(grim_home)?;
    let mut key = vec![0u8; ROOT_KEY_BYTES];
    getrandom::fill(&mut key)
        .map_err(|e| io::Error::other(format!("could not read {ROOT_KEY_BYTES} bytes from the OS CSPRNG: {e}")))?;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(TABLE_MODE);
    }
    match options.open(&path) {
        Ok(mut file) => {
            use std::io::Write;
            file.write_all(&key)?;
            file.sync_data()?;
            Ok(key)
        }
        // Another install won the race and wrote a key between our read and
        // our create. Its key is as good as ours and is already referenced by
        // whatever it registered, so adopt it.
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => match read_root_key(&path)? {
            Some(key) => Ok(key),
            None => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("the hook root key at {} is too short to be a key", path.display()),
            )),
        },
        Err(e) => Err(e),
    }
}

/// The key at `path`, or `None` when there is no key file yet.
///
/// A file that exists but is too short is [`io::ErrorKind::InvalidData`] rather
/// than a silent regeneration: re-keying would orphan every armed registration
/// on the machine, and reporting a corrupt key is recoverable where silently
/// disarming everything is not.
fn read_root_key(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(key) if key.len() >= ROOT_KEY_BYTES => Ok(Some(key)),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the hook root key at {} is too short to be a key", path.display()),
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// One armed hook, as the runtime needs it — the dispatch table's leaf.
///
/// Everything here is grim-resolved at install time. The runtime looks a row up
/// by `(root token, client, event)` plus its own matcher pass and spawns
/// `handler` from `payload_dir`; it **hashes nothing** (C-009).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchEntry {
    /// The config binding name of the artifact this entry came from. Reaches
    /// the audit trail as `<artifact>/<id>`; never a shell string (C-018b).
    pub artifact: String,
    /// [`crate::oci::hook::HookEntry::id`], unique within the artifact.
    pub id: String,
    /// **The client this row is armed for** — the second component of the
    /// `(root token, client, event)` selection key above, and the reason a row
    /// exists per `(hook, client)` rather than per hook.
    ///
    /// # Why this field is required, and why that is the *safe* choice
    ///
    /// Without it the dimension is not merely unpopulated, it is
    /// **unrecoverable**. Every other field on this struct is client-*independent*:
    /// `artifact`, `id`, `event`, `tier`, `matcher`, `handler`, `timeout`,
    /// `payload` and `policy` come from the record and the manifest, and
    /// `payload_dir` is one directory per scope shared by every arming client
    /// (S-003). So the two shapes the convergence loop bridges cannot be joined:
    /// [`super::hook_registrar::desired_entries`] is **per vendor** (it filters
    /// `record.outputs` by `o.client`), while [`converge_root`] replaces a root's
    /// whole `hooks` vector **wholesale**. Calling it once per vendor makes each
    /// vendor's write wipe the previous vendor's rows; unioning across vendors
    /// instead yields *N byte-identical rows*, which is worse than a duplicate —
    /// it means a hook grim `Declined` for one client (an untranslatable matcher
    /// per C-025, a tier that client cannot honour) is indistinguishable from one
    /// it armed there, so **the declining client runs code the user was told was
    /// not armed for it.** That converts a render-time decline into a silent
    /// arming, which is the failure C-017 and C-025 exist to prevent.
    ///
    /// # Required, not `#[serde(default)]` — Principle 9 does not apply yet
    ///
    /// Principle 9 governs **released** surfaces, and this table has never
    /// shipped: `git ls-tree 03e59b0 -- src/install/` (the v0.13.0 release
    /// commit) contains **no hook file at all**, and hooks are gated off behind
    /// `[options.experimental]` regardless. There is no `dispatch.json` anywhere
    /// to be compatible with, so this is the one free moment to make the field
    /// mandatory. A defaulted `String` would yield `""` for a client-less row,
    /// and `""` would then have to mean either "matches nothing" or "matches
    /// every client" — an ambiguity sitting in exactly the row-selection path
    /// that decides whether a declining client executes code. Required makes
    /// that row unrepresentable, and W2's row-reject-with-a-log-line already
    /// makes a malformed row degrade to *not armed* rather than to *armed for
    /// everyone*. [`DISPATCH_SCHEMA`] therefore stays `1`.
    ///
    /// A `String` and not [`ClientTarget`](super::client_target::ClientTarget):
    /// this is the same spelling [`ClientOutput::client`](super::install_state::ClientOutput)
    /// carries, so the two structures the convergence loop bridges name a client
    /// one way, and a legacy or unparsable client name recorded in `state.json`
    /// stays representable here — it simply selects no row at dispatch time,
    /// which is the fail-safe direction.
    pub client: String,
    /// The canonical firing event.
    pub event: CanonicalEvent,
    /// What the handler may do with the moment.
    pub tier: HookTier,
    /// grim's **own** matcher dialect, kept verbatim so grim — not the vendor —
    /// owns matching (`None` = every tool). Re-checked against
    /// `MATCHER_MAX_BYTES` at read time (W2 (c)): the build-time cap does not
    /// bind a file on disk.
    pub matcher: Option<String>,
    /// The program to run.
    pub handler: HookHandler,
    /// Grim-enforced timeout in seconds; `None` = the format default.
    pub timeout: Option<u64>,
    /// Envelope transport.
    pub payload: HookPayloadMode,
    /// The materialized payload tree the handler runs from — absolute, resolved
    /// from the recorded `ClientOutput` at convergence time.
    pub payload_dir: PathBuf,
    /// The digest the artifact resolved to at lock time — **provenance for
    /// diagnostics only, never a gate** (W4 · I5).
    ///
    /// It was `approved digest` and that name was a false security claim: A2
    /// deleted the approval store, A3 deleted the exec-time re-check, and the
    /// runtime hashes nothing (C-009). A field named `approved` that gates
    /// nothing gets read as a control by the next reviewer. `None` for a
    /// path-sourced (dev) install, which has no registry pin.
    pub resolved_digest: Option<String>,
    /// The reserved `policy` key, carried through unparsed so a future
    /// vocabulary lands additively.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<serde_json::Value>,
}

/// Every hook armed for one root, plus that root in readable form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchRoot {
    /// [`RootScope::display`] — **diagnostics only.** Never matched against
    /// anything at runtime, never compared to the invoking workspace (C-007),
    /// and never derived back into a token.
    pub root: String,
    /// The armed hooks, in a stable order so the written bytes are
    /// deterministic.
    pub hooks: Vec<DispatchEntry>,
}

/// The whole table: `schema` plus one entry per root token.
///
/// One file for the whole machine — but see the module doc: "machine-local" is
/// a property of *where grim writes it*, made true at runtime only by the
/// argv-supplied absolute path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchTable {
    /// Read first, before any other field (W2 (a)).
    pub schema: u32,
    /// Armed hooks per opaque root token. `BTreeMap` so the serialization is
    /// deterministic and a re-write with no change is byte-identical.
    ///
    /// Read through [`roots_map`], because [`RootToken`] deliberately has no
    /// `Deserialize` impl — see that type's doc. Reconstituting a key from a
    /// table grim itself wrote is not minting a token, and confining the
    /// capability to this one field is what keeps "no caller can build one from
    /// a `&str`" true.
    #[serde(with = "roots_map")]
    pub roots: BTreeMap<RootToken, DispatchRoot>,
}

/// The only place a [`RootToken`] is reconstituted from stored bytes.
///
/// Scoped to [`DispatchTable::roots`] so the type itself stays unbuildable from
/// an arbitrary string. A child module, so it can reach the private tuple field
/// without any of that reach leaking to callers.
mod roots_map {
    use super::{DispatchRoot, RootToken};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;

    pub(super) fn serialize<S: Serializer>(
        roots: &BTreeMap<RootToken, DispatchRoot>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        roots.serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<RootToken, DispatchRoot>, D::Error> {
        // Keys arrive as plain strings and are re-wrapped here. No validation:
        // an unrecognised token simply matches no root at dispatch time, which
        // is the same outcome as an absent one — the table is grim-written under
        // `$GRIM_HOME`, and a forged key still has to match an entry to reach
        // anything (T3/T4).
        let stored = BTreeMap::<String, DispatchRoot>::deserialize(deserializer)?;
        Ok(stored.into_iter().map(|(key, root)| (RootToken(key), root)).collect())
    }
}

impl DispatchTable {
    /// The empty table at the current schema — also what every W2 degrade
    /// resolves to, so "unreadable" and "nothing armed" are one code path.
    pub fn empty() -> Self {
        Self {
            schema: DISPATCH_SCHEMA,
            roots: BTreeMap::new(),
        }
    }
}

/// Why a table read produced [`DispatchTable::empty`] instead of content —
/// one log line's worth of reason, never an error (W2 · I3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchDegrade {
    /// No table on disk — the ordinary "no hooks armed" state, including the
    /// feature being off (Decision N: "off" reaches the runtime structurally,
    /// as an empty table).
    Absent,
    /// Larger than [`MAX_TABLE_BYTES`].
    Oversize,
    /// Not readable as JSON, or not an object.
    Unparsable,
    /// A `schema` this binary does not recognize — **including a newer one**
    /// after a downgrade. Never an error: a downgraded grim must degrade to
    /// "hooks off", not block the agent.
    UnknownSchema,
    /// A row failed the read-time re-check (`MATCHER_MAX_BYTES`, a
    /// non-absolute `payload_dir`).
    ///
    /// Whole-table, not per-row: a table grim cannot fully vouch for arms
    /// nothing, because "some rows survived" is how a tampered table gets a
    /// partial verdict honoured.
    RowRejected,
}

/// Read the table at `path` — **never fails, never panics** (W2).
///
/// The whole of the reader contract, in the order the checks run: size cap →
/// parse → `schema` → per-row re-checks. Any failure yields
/// [`DispatchTable::empty`] plus the [`DispatchDegrade`] reason for one log
/// line, and the caller exits 0. No `unwrap`, no `expect`, no `Err`
/// (`quality-rust.md`; W2 (d)).
///
/// `path` must already be absolute — the runtime refuses a non-absolute
/// `--table` before it gets here (B1 (3)), and this function deliberately does
/// **not** re-derive it, because the only other way to get one is
/// [`crate::env::grim_home`], which the runtime may never call (C-007).
///
/// **One parser, and it lives here.** WP-K's runtime calls this rather than
/// deserializing the file itself: two readers of one format drift, and the
/// drift direction is "the runtime honours a row the writer would not have
/// written" — the C-021 lesson applied to the table instead of the projection.
pub fn read_table(path: &Path) -> (DispatchTable, Option<DispatchDegrade>) {
    let degraded = |reason: DispatchDegrade| (DispatchTable::empty(), Some(reason));

    // (c) The size cap, before the read — a cap applied after reading the file
    // is not a cap.
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > MAX_TABLE_BYTES => return degraded(DispatchDegrade::Oversize),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return degraded(DispatchDegrade::Absent),
        // Unreadable for any other reason is not "absent": the file may well be
        // there. It is still the same degrade direction (I3) but a different
        // log line, so it is reported as unparsable rather than as absent.
        Err(_) => return degraded(DispatchDegrade::Unparsable),
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return degraded(DispatchDegrade::Absent),
        Err(_) => return degraded(DispatchDegrade::Unparsable),
    };

    // (a) `schema` first, and through an untyped parse — deserializing straight
    // into `DispatchTable` would report a *newer* schema's unknown row shape as
    // `Unparsable`, when the honest answer is `UnknownSchema`.
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return degraded(DispatchDegrade::Unparsable),
    };
    let Some(object) = value.as_object() else {
        return degraded(DispatchDegrade::Unparsable);
    };
    // (b) Any unrecognized value — including a newer one after a downgrade.
    if object.get("schema").and_then(serde_json::Value::as_u64) != Some(u64::from(DISPATCH_SCHEMA)) {
        return degraded(DispatchDegrade::UnknownSchema);
    }

    let table: DispatchTable = match serde_json::from_value(value) {
        Ok(table) => table,
        Err(_) => return degraded(DispatchDegrade::Unparsable),
    };

    // (c) Row re-checks. Whole-table, not per-row: a table grim cannot fully
    // vouch for arms nothing, because "some rows survived" is how a tampered
    // table gets a partial verdict honoured.
    let rows_ok = table.roots.values().flat_map(|root| &root.hooks).all(|hook| {
        hook.matcher.as_ref().is_none_or(|m| m.len() <= MATCHER_MAX_BYTES) && hook.payload_dir.is_absolute()
    });
    if !rows_ok {
        return degraded(DispatchDegrade::RowRejected);
    }

    (table, None)
}

/// The outcome of a [`converge_root`] call, for one `tracing` line and for the
/// registrar's own report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchWrite {
    /// The table already said exactly this — no bytes written.
    Unchanged,
    /// This root's entry was written or replaced wholesale.
    Written,
    /// This root's entry was removed (the desired set was empty).
    Removed,
}

/// Why a dispatch write did not happen.
///
/// Deliberately narrow: [`super::hook_registrar::ArmRefusal`] is the reported
/// vocabulary, and this is the half this module can observe. Kept separate so
/// the module graph stays acyclic (registrar → dispatch, never back).
#[derive(Debug)]
pub enum DispatchError {
    /// Another `grim install` holds the dispatch lock (W1). Reported as
    /// `not-armed`, **not** written over: two installs in two workspaces are
    /// otherwise last-writer-wins on the record set, and the loser's hooks are
    /// silently absent while `grim status` believes they are armed.
    Locked,
    /// An I/O failure reading, locking, or writing the table.
    Io(io::Error),
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Locked => f.write_str("another grim install holds the dispatch table lock"),
            Self::Io(e) => write!(f, "dispatch table I/O failure: {e}"),
        }
    }
}

impl std::error::Error for DispatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Locked => None,
            Self::Io(e) => Some(e),
        }
    }
}

/// The fraction of [`MAX_TABLE_BYTES`] past which [`converge_root`] warns.
///
/// The cliff is total: one byte over the cap and [`read_table`] returns the
/// empty table, so **every hook for every root on the machine silently
/// disarms**. `grim hook run` does log that at `warn`, but its stderr is a
/// client's to swallow, so in practice the user sees their hooks stop. The
/// warning therefore has to fire on the *install* path, which is a terminal the
/// user is looking at, and it has to fire before the cliff rather than at it.
const TABLE_WARN_NUMERATOR: u64 = 8;
const TABLE_WARN_DENOMINATOR: u64 = 10;

// **A cross-root reap was implemented here and withdrawn. Do not re-add it
// without reading this.**
//
// The table only grows: `converge_root` converges the root it is called for and
// preserves every other verbatim, so a checkout deleted without
// `grim uninstall` strands its entry. The obvious fix — drop a root whose
// workspace directory is absent — is **unsound**, and two independent round-2
// reviewers reached that conclusion separately:
//
//   * `DispatchRoot.root` is documented "diagnostics only", and it is produced
//     by `RootScope::display()`, i.e. `to_string_lossy()`. A workspace path with
//     a non-UTF-8 byte (legal on Linux) is stored U+FFFD-substituted, so the
//     string does not exist and a LIVE root is reaped.
//   * `Path::exists()` answers "visible to this process", not "present". A
//     `$GRIM_HOME` shared between a host and a devcontainer — a setup
//     `adr_install_state_portability.md` exists to support — makes each side's
//     roots invisible to the other, so every install in the container reaps
//     every host root and vice versa. The two mutually disarm each other,
//     forever, on every install.
//
// Narrowing to a positively-confirmed `NotFound` fixes the `EACCES` case and
// neither of those.
//
// **What that leaves, stated honestly** (round 3, W3 — the earlier wording here
// claimed the growth was "bounded by the warning below", and a `warn!` is a
// diagnostic, not a bound): the table only grows, and the sole things that
// shrink it are `grim uninstall` and an operator deleting the file. The 80 %
// warning makes the approach to the cliff visible on a terminal the user is
// looking at; it does not prevent it. Issue #93 tracks the writer-side refusal
// at the cap, and its rationale for deferring — that this reap had removed the
// realistic path to the cap — was falsified by this withdrawal, so it is now
// the only thing standing between an accumulating table and a total disarm.
// The reap question itself (an opt-in prune, or a reap keyed on something
// actually authoritative) is issue #97; do not cite #93 for it.

/// Replace `token`'s entry with exactly `hooks`, **wholesale, under the
/// dispatch lock** (C-006 (3), W1).
///
/// An empty `hooks` removes the root key entirely, which is what makes uninstall
/// and "the feature was turned off" the same code path — and what makes the
/// table converge rather than accumulate. Other roots' entries are read,
/// preserved and written back verbatim.
///
/// Three properties, and each one is a separate failure this shape prevents:
///
/// - **Wholesale per key**, so a partially-updated root is unrepresentable — a
///   root either has the set this install computed or the set the last one did.
/// - **Under [`crate::lock::advisory_lock::AdvisoryFileLock`]**, because the
///   file holds *all* root keys and this is a read-modify-write of shared
///   machine-global state. `arch-principles.md` already mandates the lock for
///   exactly this; W1 found it missing from C-006, whose "atomically and
///   wholesale per root key" covers tearing but not mutual exclusion.
/// - **Through `atomic_write`**, whose crash safety WP-P0 verified by reading
///   the primitive (tempfile → `sync_data` → mode capped → `persist` → parent
///   `fsync`): a crash mid-write leaves the previous table, and a concurrent
///   reader sees old or new, never torn. That half of C-006 was already sound.
///
/// # Errors
///
/// [`DispatchError::Locked`] when another install holds the lock — the caller
/// reports `not-armed` and writes nothing. [`DispatchError::Io`] otherwise.
pub fn converge_root(
    grim_home: &Path,
    token: &RootToken,
    root: RootScope<'_>,
    hooks: &[DispatchEntry],
) -> Result<DispatchWrite, DispatchError> {
    // The lock's sidecar is created beside the guarded path, so the directory
    // has to exist before the acquire — and it must be `0o700` before anything
    // is written into it, not after (W3).
    ensure_hooks_dir(grim_home).map_err(DispatchError::Io)?;
    let path = dispatch_path(grim_home);

    let _guard = AdvisoryFileLock::try_acquire(&path).map_err(|e| match e.kind {
        LockErrorKind::Locked => DispatchError::Locked,
        LockErrorKind::Io(io) => DispatchError::Io(io),
        // No other `LockErrorKind` is reachable from `try_acquire` — the
        // TOML/size variants belong to the lock *file* reader, not to the
        // advisory guard. Kept total rather than panicking (I3).
        other => DispatchError::Io(io::Error::other(other.to_string())),
    })?;

    // A degrade is deliberately not an error here. `read_table` already
    // collapses every unreadable shape to the empty table, and the empty table
    // is the correct base for a converge: this root gets the set grim just
    // computed, and a table this binary cannot understand is replaced by one it
    // can. The other direction — refusing to write because the existing file is
    // corrupt — would leave the corruption armed forever.
    let (mut table, _degrade) = read_table(&path);
    table.schema = DISPATCH_SCHEMA;

    let desired = (!hooks.is_empty()).then(|| DispatchRoot {
        root: root.display().into_owned(),
        hooks: hooks.to_vec(),
    });
    if table.roots.get(token) == desired.as_ref() {
        return Ok(DispatchWrite::Unchanged);
    }

    let outcome = match desired {
        Some(entry) => {
            table.roots.insert(token.clone(), entry);
            DispatchWrite::Written
        }
        None => {
            table.roots.remove(token);
            DispatchWrite::Removed
        }
    };

    let bytes = serde_json::to_vec_pretty(&table).map_err(|e| DispatchError::Io(io::Error::other(e)))?;
    // Warned on the install path, where a user is watching, and *before* the
    // cliff rather than at it — past the cap every root disarms at once, so a
    // diagnostic that only fires once it has happened is too late to act on.
    let warn_at = MAX_TABLE_BYTES * TABLE_WARN_NUMERATOR / TABLE_WARN_DENOMINATOR;
    if bytes.len() as u64 > warn_at {
        tracing::warn!(
            "the hook dispatch table is {} of its {MAX_TABLE_BYTES}-byte limit across {} root(s); \
             past the limit no hook arms for any workspace. Run `grim uninstall hook <name>` in \
             projects that no longer need theirs",
            bytes.len(),
            table.roots.len()
        );
    }
    atomic_write(&path, &bytes).map_err(DispatchError::Io)?;
    // `atomic_write` caps at `0o644` and preserves an existing capped mode, so a
    // table that was ever `0o644` would stay `0o644` across every later write.
    // The explicit narrow makes the mode a property of this writer rather than
    // of the file's history (W3).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(TABLE_MODE)).map_err(DispatchError::Io)?;
    }

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── reserved-name drift (round 3, B1/B2) ─────────────────────────────────

    /// ⛔ **B1/B2.** Every name grim itself writes directly under
    /// `$GRIM_HOME/hooks/` is refused as a hook **binding** name.
    ///
    /// This is the test [`crate::oci::hook::RESERVED_ARTIFACT_NAMES`]'s doc names,
    /// and it is here rather than in `oci` because the direction only works this
    /// way round: `install` may depend on `oci`, not the reverse, so the literal
    /// list over there cannot see the layout it has to track. It fell one behind
    /// twice — `root-key` (round 2) and `dispatch.json.lock` (round 3) — and for a
    /// while this test was only *claimed* to exist, which is how the second one
    /// shipped. The failure mode it prevents is total: a binding that lands a
    /// directory on one of these paths stops every hook on the machine from arming
    /// (invariants I3, I5).
    ///
    /// **The namespace is enumerated from the filesystem, not from a list in this
    /// test.** The first version of this test iterated the layout constants by
    /// hand, which reproduced the exact defect it exists to prevent one level up:
    /// a new `const` written under `hooks/` would not appear in the loop, so the
    /// test would pass and the doc's promise ("fails the build for the next file
    /// grim puts under `hooks/`") would be false again. Instead it *provokes* the
    /// writes — mint the root key, generate the launcher, converge a root, which
    /// between them create every entry install produces — and then requires each
    /// directory entry to be reserved.
    ///
    /// `EXPECTED_UNRESERVED` is the deliberate escape hatch and it is **fail
    /// closed**: a new file must be reserved, or a human must add it here with a
    /// reason. That asymmetry is the whole point — the reviewer who asked for this
    /// test warned against asserting the reverse direction ("nothing reserved that
    /// is not grim's own") because it would have to encode exceptions, and that is
    /// right; this direction encodes exceptions for *unreserved* names, where
    /// forgetting one makes the test fail rather than pass.
    #[test]
    fn every_grim_owned_name_under_hooks_is_a_reserved_binding_name() {
        /// Grim-owned entries under `hooks/` that are deliberately NOT reserved.
        ///
        /// Each needs a reason that survives review, because every entry here is a
        /// name a hook could be bound to:
        ///
        /// Empty today: everything the provocation produces is reserved. Note the
        /// provocation covers install's **dispatch-side** writes; `payload/` is
        /// created by materialization, which this test does not run, and is
        /// reserved regardless.
        ///
        /// Names that never reach a binding are absent rather than listed:
        /// `hook_audit.jsonl{,.1}` and the transient `payload_<pid>_<slot>.json`
        /// envelopes both carry an underscore, which is outside `SkillName`'s
        /// grammar, so neither is representable as a binding name. Both sit at the
        /// root of `hooks/` — an earlier version of this note said the envelopes
        /// lived inside a payload directory, which was false, and the grammar is
        /// the whole reason they are safe.
        const EXPECTED_UNRESERVED: [&str; 0] = [];

        let home = tempfile::tempdir().expect("tempdir");
        let grim = home.path().join("grim");
        std::fs::write(&grim, b"#!/bin/sh\nexit 0\n").expect("fake binary");

        // Provoke every write install performs under `hooks/`.
        let token = root_token(home.path(), RootScope::Global).expect("mint the root key");
        crate::install::hook_launcher::generate(home.path(), &grim).expect("generate the launcher");
        let rows = [entry("obs", &payload_dir(home.path(), RootScope::Global, "guard"))];
        converge_root(home.path(), &token, RootScope::Global, &rows).expect("write the table");

        let list = || -> std::collections::BTreeSet<String> {
            std::fs::read_dir(hooks_dir(home.path()))
                .expect("read hooks dir")
                .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
                .collect()
        };

        // Two observations, unioned. The lock sidecar exists only while the guard
        // is held — `converge_root` releases it before returning — so a single
        // post-provocation listing would silently miss exactly the entry round 3
        // found missing.
        let mut entries = list();
        {
            let _guard = AdvisoryFileLock::try_acquire(&dispatch_path(home.path())).expect("uncontended acquire");
            entries.extend(list());
        }
        let entries: Vec<String> = entries.into_iter().collect();
        assert!(
            entries.len() >= 4,
            "the provocation did not produce the layout it is meant to observe: {entries:?}"
        );

        for name in &entries {
            if EXPECTED_UNRESERVED.contains(&name.as_str()) {
                continue;
            }
            assert!(
                crate::oci::hook::binding_name_refusal(name).is_some(),
                "grim writes '{name}' under $GRIM_HOME/hooks/, so a hook must not be bindable to \
                 it — add it to RESERVED_ARTIFACT_NAMES, or to EXPECTED_UNRESERVED with a reason \
                 it is safe. Observed: {entries:?}"
            );
        }
    }

    /// ⛔ **V1's class, not just its instance.** Every name grim writes at the root
    /// of `hooks/` is either refused as a binding name or unrepresentable as one.
    ///
    /// The B1 test above walks the directory, so it covers whatever install actually
    /// wrote. It cannot cover the **runtime**-side writers, which no unit test
    /// provokes (V2), and those are exactly where V1 hid: the payload envelope sat
    /// in the binding namespace under a representable name. This closes the
    /// remaining axis by naming each runtime-side writer's format explicitly, which
    /// is a hand-maintained list — the thing this branch keeps getting wrong — so it
    /// is deliberately paired with the walk rather than replacing it. Three of the
    /// four rows read their name from the code that produces it (the audit trail's
    /// constant, its rotation suffix, and `envelope_file_name`); the `.tmp` row is a
    /// hand-written stand-in for a prefix `tempfile` owns, and says so at the call.
    ///
    /// A name is safe two ways and the distinction matters: **reserved** (the array
    /// refuses it) or **unrepresentable** (`SkillName`'s grammar cannot express it,
    /// which an underscore or a leading dot achieves). Dynamic names can only ever
    /// take the second route, because the array cannot hold a pid.
    #[test]
    fn every_runtime_written_name_at_the_hooks_root_is_unusable_as_a_binding() {
        let audit = crate::command::hook::run::AUDIT_FILE;
        let rotated = format!("{audit}{}", crate::hook::audit::ROTATED_SUFFIX);
        // Read from the function that produces it, so this row observes production
        // rather than re-spelling it (fix-verify pass 2, W1).
        let envelope = crate::command::hook::pipeline::envelope_file_name(4294967295, 0);
        // `atomic_write`'s temp sibling. This one IS a hand-written stand-in for
        // tempfile's default prefix, which grim does not choose and cannot import a
        // constant for — so the row rests on the leading dot being outside the
        // grammar, and would not notice tempfile changing its prefix to something
        // representable. Called out rather than glossed, because the row above it
        // is derived and this one is not.
        let temp = ".tmpAbC123";

        for name in [audit.to_string(), rotated, envelope, temp.to_string()] {
            assert!(
                crate::oci::hook::binding_name_refusal(&name).is_some(),
                "grim writes '{name}' at the root of $GRIM_HOME/hooks/, which is also the hook \
                 binding namespace, so it must be unusable as a binding name — reserve it, or \
                 give it a character outside SkillName's grammar (an underscore or a leading dot)"
            );
        }
    }

    // ── payload location (SEC-1) ─────────────────────────────────────────────

    /// ⛔ **SEC-1.** Both scopes resolve under `$GRIM_HOME`, and two workspaces
    /// get two directories.
    ///
    /// The remainder is asserted as a string because it is also the `relative`
    /// half of the install record, so it has to stay a `Normal`-only
    /// forward-slash path that the containment guard accepts.
    #[test]
    fn a_payload_is_under_grim_home_at_both_scopes_and_keyed_per_workspace() {
        let home = Path::new("/grim");
        assert_eq!(
            payload_dir(home, RootScope::Global, "shell-guard"),
            PathBuf::from("/grim/hooks/shell-guard")
        );

        let a = payload_relative(RootScope::Workspace(Path::new("/ws-a")), "shell-guard");
        let b = payload_relative(RootScope::Workspace(Path::new("/ws-b")), "shell-guard");
        assert_ne!(a, b, "two workspaces must not share one payload directory");
        assert!(a.starts_with("hooks/payload/"), "{a}");
        assert!(
            Path::new(&a)
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
            "the remainder must be Normal-only so `AnchoredPath::resolve` accepts it — {a}"
        );
        assert_eq!(
            payload_dir(home, RootScope::Workspace(Path::new("/ws-a")), "shell-guard"),
            {
                let mut p = PathBuf::from("/grim");
                p.push(&a);
                p
            }
        );

        // Stable across calls: re-materialization has to find the same directory.
        assert_eq!(
            a,
            payload_relative(RootScope::Workspace(Path::new("/ws-a")), "shell-guard")
        );
    }

    /// ⛔ **B3.** The payload key and the dispatch-table key are **different**
    /// derivations of the same semantic root, and the path must not leak the
    /// token.
    ///
    /// A recorded install target is ordinary user-visible data — it is written
    /// into `state.json` and printed by `grim status --format json` — while a
    /// guessable root token lets a hostile repository's own registration fire the
    /// victim's already-armed hooks. Sharing one value between the two would
    /// publish the wire key, so this pins that it does not.
    #[test]
    fn the_payload_key_is_not_the_dispatch_root_token() {
        let home = tempfile::tempdir().unwrap();
        let ws = Path::new("/ws-a");
        let root = RootScope::Workspace(ws);
        let token = root_token(home.path(), root).unwrap();
        let relative = payload_relative(root, "shell-guard");
        assert!(
            !relative.contains(token.as_str()),
            "a payload path must never carry the dispatch root token (B3) — {relative}"
        );

        // And the key is derivable with **no** side effects: computing a payload
        // path must not mint the machine key, because read-only commands compute
        // install destinations too.
        let fresh = tempfile::tempdir().unwrap();
        let _ = payload_dir(fresh.path(), root, "shell-guard");
        assert!(
            !root_key_path(fresh.path()).exists(),
            "deriving a payload path must not create the machine key"
        );
        assert!(
            !hooks_dir(fresh.path()).exists(),
            "deriving a payload path must write nothing"
        );
    }

    fn entry(id: &str, payload_dir: &Path) -> DispatchEntry {
        entry_for("claude", id, payload_dir)
    }

    /// [`entry`] with the arming client named — the shape every row really has,
    /// since a row is per `(hook, client)`.
    fn entry_for(client: &str, id: &str, payload_dir: &Path) -> DispatchEntry {
        DispatchEntry {
            artifact: "guard".to_string(),
            id: id.to_string(),
            client: client.to_string(),
            event: CanonicalEvent::PreToolUse,
            tier: HookTier::Observer,
            matcher: Some("Bash".to_string()),
            handler: HookHandler::Argv(vec!["sh".to_string(), "guard.sh".to_string()]),
            timeout: None,
            payload: HookPayloadMode::Stdin,
            payload_dir: payload_dir.to_path_buf(),
            resolved_digest: None,
            policy: None,
        }
    }

    // ── F-1: the row's client dimension ────────────────────────────

    /// **The `client` field is required, and that is the security property.**
    /// A row with no `client` must fail to deserialize, so it can never reach
    /// row selection as an empty string whose meaning ("matches nothing" or
    /// "matches everything") is undefined. W2's reject-with-a-log-line then
    /// degrades the whole table to *not armed*, never to *armed for everyone*.
    ///
    /// Legitimate to pin as a hard contract because the table has **never
    /// shipped**: `git ls-tree 03e59b0 -- src/install/` (v0.13.0) carries no
    /// hook file, so there is no on-disk `dispatch.json` for Principle 9 to
    /// protect and `DISPATCH_SCHEMA` stays 1.
    #[test]
    fn a_row_without_a_client_is_refused_never_defaulted() {
        let complete = serde_json::to_string(&entry_for("claude", "guard", Path::new("/h/hooks/g"))).unwrap();
        assert!(
            complete.contains(r#""client":"claude""#),
            "the client must be written, not skipped: {complete}"
        );
        serde_json::from_str::<DispatchEntry>(&complete).expect("a complete row round-trips");

        // The same row with `client` removed.
        let mut value: serde_json::Value = serde_json::from_str(&complete).unwrap();
        value.as_object_mut().unwrap().remove("client");
        let clientless = serde_json::to_string(&value).unwrap();
        let err = serde_json::from_str::<DispatchEntry>(&clientless)
            .expect_err("a row with no client must be refused, not defaulted to an empty string");
        assert!(
            err.to_string().contains("client"),
            "the error must name the field: {err}"
        );
    }

    /// **F-1's fix, end to end.** `desired_entries` is per **vendor** while
    /// `converge_root` replaces a root's `hooks` vector **wholesale**, so the
    /// two only compose if a row carries the client it was armed for.
    ///
    /// Every other field is client-independent — `payload_dir` most of all,
    /// because a hook's payload is one directory per scope shared by every
    /// arming client (S-003) — so without this field the two clients' rows
    /// would be **byte-identical**, and "armed for claude only" would be
    /// indistinguishable from "armed for claude and codex". This asserts both
    /// halves: the rows differ *only* in `client`, and they survive one write
    /// as two selectable rows.
    #[test]
    fn two_clients_arming_one_hook_are_two_selectable_rows_in_one_root() {
        let home = tempfile::tempdir().unwrap();
        let payload = home.path().join("hooks/shell-guard");
        let token = root_token(home.path(), RootScope::Global).unwrap();

        let claude = entry_for("claude", "guard", &payload);
        let codex = entry_for("codex", "guard", &payload);
        assert_ne!(claude, codex, "the client is what distinguishes the two rows");
        assert_eq!(
            DispatchEntry {
                client: codex.client.clone(),
                ..claude.clone()
            },
            codex,
            "the rows must differ in NOTHING but the client — that is why the field is load-bearing"
        );

        let rows = [claude, codex];
        assert_eq!(
            converge_root(home.path(), &token, RootScope::Global, &rows).unwrap(),
            DispatchWrite::Written
        );

        let (table, _) = read_table(&dispatch_path(home.path()));
        let stored = &table.roots.get(&token).expect("the root was written").hooks;
        assert_eq!(stored.len(), 2, "one wholesale write carries both clients' rows");
        let mut armed: Vec<&str> = stored.iter().map(|h| h.client.as_str()).collect();
        armed.sort_unstable();
        assert_eq!(armed, vec!["claude", "codex"]);

        // Row selection: the runtime's `(root token, client, event)` lookup can
        // now name exactly one row, which is what stops a client grim DECLINED
        // this hook for from executing it (C-017 / C-025).
        let selected: Vec<&DispatchEntry> = stored
            .iter()
            .filter(|h| h.client == "codex" && h.event == CanonicalEvent::PreToolUse)
            .collect();
        assert_eq!(selected.len(), 1, "{selected:?}");
        assert!(
            !stored.iter().any(|h| h.client == "copilot"),
            "a client that was never armed must not be selectable"
        );
    }

    #[test]
    fn root_token_is_stable_across_calls_and_distinct_per_root() {
        let home = tempfile::tempdir().unwrap();
        let ws = Path::new("/home/dev/project");

        let global = root_token(home.path(), RootScope::Global).unwrap();
        let again = root_token(home.path(), RootScope::Global).unwrap();
        let project = root_token(home.path(), RootScope::Workspace(ws)).unwrap();

        // Stability is the whole reason the token is an HMAC and not stored
        // randomness: re-materialization has to find its own entry.
        assert_eq!(global, again);
        assert_ne!(global, project);
        assert_eq!(global.as_str().len(), ROOT_TOKEN_BYTES * 2);
        assert!(
            global
                .as_str()
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        );
        // Never the forbidden wire forms (B3).
        assert_ne!(global.as_str(), "global");
        assert_ne!(project.as_str(), ws.to_string_lossy());
    }

    #[test]
    fn a_different_machine_key_yields_a_different_token() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        assert_ne!(
            root_token(a.path(), RootScope::Global).unwrap(),
            root_token(b.path(), RootScope::Global).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_key_file_and_its_directory_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        root_token(home.path(), RootScope::Global).unwrap();

        let dir = std::fs::metadata(hooks_dir(home.path())).unwrap();
        let key = std::fs::metadata(root_key_path(home.path())).unwrap();
        assert_eq!(dir.permissions().mode() & 0o777, HOOKS_DIR_MODE);
        assert_eq!(key.permissions().mode() & 0o777, TABLE_MODE);
        assert_eq!(std::fs::read(root_key_path(home.path())).unwrap().len(), ROOT_KEY_BYTES);
    }

    #[test]
    fn a_truncated_key_is_reported_never_silently_regenerated() {
        let home = tempfile::tempdir().unwrap();
        ensure_hooks_dir(home.path()).unwrap();
        std::fs::write(root_key_path(home.path()), b"short").unwrap();

        let err = root_token(home.path(), RootScope::Global).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        // Silently re-keying would orphan every armed registration.
        assert_eq!(std::fs::read(root_key_path(home.path())).unwrap(), b"short");
    }

    #[test]
    fn an_absent_table_is_absent_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let (table, degrade) = read_table(&dir.path().join("nope.json"));
        assert_eq!(degrade, Some(DispatchDegrade::Absent));
        assert_eq!(table, DispatchTable::empty());
    }

    #[test]
    fn every_malformed_shape_degrades_to_the_empty_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dispatch.json");

        for (bytes, expected) in [
            (b"not json".to_vec(), DispatchDegrade::Unparsable),
            (b"[]".to_vec(), DispatchDegrade::Unparsable),
            (br#"{"roots":{}}"#.to_vec(), DispatchDegrade::UnknownSchema),
            (br#"{"schema":2,"roots":{}}"#.to_vec(), DispatchDegrade::UnknownSchema),
            (br#"{"schema":"1","roots":{}}"#.to_vec(), DispatchDegrade::UnknownSchema),
        ] {
            std::fs::write(&path, &bytes).unwrap();
            let (table, degrade) = read_table(&path);
            assert_eq!(degrade, Some(expected), "{}", String::from_utf8_lossy(&bytes));
            assert_eq!(table, DispatchTable::empty());
        }
    }

    #[test]
    fn an_oversize_table_is_refused_before_it_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dispatch.json");
        std::fs::write(&path, vec![b'x'; usize::try_from(MAX_TABLE_BYTES).unwrap() + 1]).unwrap();
        assert_eq!(read_table(&path).1, Some(DispatchDegrade::Oversize));
    }

    #[test]
    fn one_bad_row_rejects_the_whole_table() {
        let home = tempfile::tempdir().unwrap();
        let token = root_token(home.path(), RootScope::Global).unwrap();
        let path = dispatch_path(home.path());

        // A relative payload_dir, and an over-long matcher: both are read-time
        // re-checks, because a build-time cap does not bind a file on disk.
        for bad in [entry("relative", Path::new("payloads/guard")), {
            let mut e = entry("long", Path::new("/abs/payloads/guard"));
            e.matcher = Some("A".repeat(MATCHER_MAX_BYTES + 1));
            e
        }] {
            let mut table = DispatchTable::empty();
            table.roots.insert(
                token.clone(),
                DispatchRoot {
                    root: "global".to_string(),
                    hooks: vec![bad],
                },
            );
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, serde_json::to_vec(&table).unwrap()).unwrap();
            assert_eq!(read_table(&path).1, Some(DispatchDegrade::RowRejected));
        }
    }

    #[test]
    fn converge_root_writes_then_is_idempotent_then_removes() {
        let home = tempfile::tempdir().unwrap();
        let token = root_token(home.path(), RootScope::Global).unwrap();
        let hooks = vec![entry("deny-curl", Path::new("/abs/payloads/guard"))];

        assert_eq!(
            converge_root(home.path(), &token, RootScope::Global, &hooks).unwrap(),
            DispatchWrite::Written
        );
        let written = std::fs::read(dispatch_path(home.path())).unwrap();

        assert_eq!(
            converge_root(home.path(), &token, RootScope::Global, &hooks).unwrap(),
            DispatchWrite::Unchanged
        );
        assert_eq!(std::fs::read(dispatch_path(home.path())).unwrap(), written);

        assert_eq!(
            converge_root(home.path(), &token, RootScope::Global, &[]).unwrap(),
            DispatchWrite::Removed
        );
        let (table, degrade) = read_table(&dispatch_path(home.path()));
        assert_eq!(degrade, None);
        assert!(table.roots.is_empty());
        assert_eq!(
            converge_root(home.path(), &token, RootScope::Global, &[]).unwrap(),
            DispatchWrite::Unchanged
        );
    }

    #[test]
    fn converging_one_root_leaves_every_other_root_verbatim() {
        let home = tempfile::tempdir().unwrap();
        let ws = Path::new("/home/dev/project");
        let global = root_token(home.path(), RootScope::Global).unwrap();
        let project = root_token(home.path(), RootScope::Workspace(ws)).unwrap();

        converge_root(
            home.path(),
            &global,
            RootScope::Global,
            &[entry("g", Path::new("/abs/g"))],
        )
        .unwrap();
        converge_root(
            home.path(),
            &project,
            RootScope::Workspace(ws),
            &[entry("p", Path::new("/abs/p"))],
        )
        .unwrap();

        let (table, _) = read_table(&dispatch_path(home.path()));
        assert_eq!(table.roots.len(), 2);
        assert_eq!(table.roots[&global].root, "global");
        assert_eq!(table.roots[&project].root, ws.to_string_lossy());

        // Removing one leaves the other.
        converge_root(home.path(), &project, RootScope::Workspace(ws), &[]).unwrap();
        let (table, _) = read_table(&dispatch_path(home.path()));
        assert_eq!(table.roots.keys().collect::<Vec<_>>(), vec![&global]);
    }

    #[test]
    fn a_held_dispatch_lock_is_reported_never_written_over() {
        let home = tempfile::tempdir().unwrap();
        let token = root_token(home.path(), RootScope::Global).unwrap();
        ensure_hooks_dir(home.path()).unwrap();

        let _held = AdvisoryFileLock::try_acquire(&dispatch_path(home.path())).unwrap();
        let err = converge_root(
            home.path(),
            &token,
            RootScope::Global,
            &[entry("g", Path::new("/abs/g"))],
        )
        .unwrap_err();

        assert!(matches!(err, DispatchError::Locked));
        assert!(!dispatch_path(home.path()).exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_pre_existing_group_readable_table_is_narrowed_on_write() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let token = root_token(home.path(), RootScope::Global).unwrap();
        let path = dispatch_path(home.path());
        std::fs::write(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        converge_root(
            home.path(),
            &token,
            RootScope::Global,
            &[entry("g", Path::new("/abs/g"))],
        )
        .unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            TABLE_MODE
        );
    }

    #[test]
    fn a_corrupt_table_is_replaced_rather_than_left_armed() {
        let home = tempfile::tempdir().unwrap();
        let token = root_token(home.path(), RootScope::Global).unwrap();
        let path = dispatch_path(home.path());
        std::fs::write(&path, b"}}} not json").unwrap();

        assert_eq!(
            converge_root(
                home.path(),
                &token,
                RootScope::Global,
                &[entry("g", Path::new("/abs/g"))]
            )
            .unwrap(),
            DispatchWrite::Written
        );
        assert_eq!(read_table(&path).1, None);
    }
}
