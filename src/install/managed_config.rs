// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The reversible config-registration driver: add or remove one managed
//! element in a root-level JSON array of a config file grim does not own.
//!
//! Two vendors need the same seam pointed in opposite directions.
//! [`super::opencode_config`] adds an `instructions` glob so OpenCode
//! rules load at all; [`super::claude_config`] adds a `claudeMdExcludes`
//! glob so a rule's support directory stops loading. The element is added
//! while the install state wants it and removed when it does not — the
//! reversible config-registration pattern from the hooks ADR.
//!
//! Both directions share one error policy, which is deliberately
//! **asymmetric** and must not be re-derived per vendor:
//!
//! - **Adding is strict.** A config that does not parse (even after JSONC
//!   comment / trailing-comma stripping), or whose managed key is not an
//!   array, is never rewritten — the sync fails rather than clobbering
//!   user content.
//! - **Removing is tolerant.** The same config has nothing grim-managed to
//!   remove, so it converges as [`ArraySync::Unchanged`] rather than
//!   failing a command whose primary action (an uninstall) already ran.
//!
//! The edit goes through the span-preserving [`super::json_splice`]
//! engine, so every byte outside the managed element — key order,
//! formatting, JSONC comments — survives untouched.

use std::io;
use std::path::Path;

use crate::store::atomic_write;

use super::json_config::with_path;
use super::json_splice::{self, Splice};

/// What a sync did to the vendor config.
///
/// Closed internal enum — matches stay total, no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArraySync {
    /// The managed element was appended to the array.
    Added,
    /// The managed element was removed (and an emptied key dropped).
    Removed,
    /// The config already matched the desired state — no write.
    Unchanged,
}

/// Idempotently add (`want = true`) or remove (`want = false`) the managed
/// `entry` in the root-level `key` array of the config at `config_path`.
///
/// - Adding creates the file (`{"<key>": [entry]}`) when absent.
/// - Removing an entry from an absent/never-registered config is a no-op.
/// - Other config keys and other elements of `key` are preserved.
///
/// Removal (`want == false`) is tolerant: an absent, unparseable, or
/// wrong-typed (`key` not an array) config has nothing grim-managed to
/// remove, so it converges as [`ArraySync::Unchanged`] rather than failing.
/// Adding (`want == true`) stays strict — grim never rewrites a file it
/// cannot parse or whose `key` is an unexpected type.
///
/// # Errors
///
/// An I/O failure, or — **only when adding** (`want == true`) — `InvalidData`
/// when the existing content is not a JSON/JSONC object, or its `key` is not
/// an array (grim never clobbers an unknown-schema file).
pub fn sync_managed_element(config_path: &Path, key: &str, entry: &str, want: bool) -> io::Result<ArraySync> {
    // A missing file reads as empty text — the splice engine's own
    // "no document yet" case, which emits the minimal skeleton on add and
    // is a no-op on remove.
    let raw = match std::fs::read_to_string(config_path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(with_path(config_path, e)),
    };

    let spliced = if want {
        json_splice::upsert_array_element(&raw, key, entry)
    } else {
        remove_every(&raw, key, entry)
    };
    let spliced = match spliced {
        Ok(splice) => splice,
        // Removal is tolerant (`want == false`): a config grim cannot parse —
        // or whose managed key is not an array — has nothing grim-managed to
        // remove, so converge as `Unchanged` rather than fail a command whose
        // primary action already ran. Adding stays strict (never rewrite an
        // unknown-schema file).
        Err(_) if !want => return Ok(ArraySync::Unchanged),
        Err(e) => return Err(with_path(config_path, e)),
    };

    match spliced {
        Splice::Unchanged => Ok(ArraySync::Unchanged),
        Splice::Changed(text) => {
            atomic_write(config_path, text.as_bytes()).map_err(|e| with_path(config_path, e))?;
            Ok(if want { ArraySync::Added } else { ArraySync::Removed })
        }
    }
}

/// [`json_splice::remove_array_element`] driven to convergence.
///
/// That primitive removes one matching element per call. Grim's own upsert is
/// idempotent so it never writes a duplicate, but a hand-edited config can
/// hold the managed element twice — and leaving the second one behind would
/// keep the vendor acting on a registration the uninstall just retired. The
/// emptied-array case takes the whole key with it, so the loop terminates on
/// the following `Unchanged`.
fn remove_every(raw: &str, key: &str, entry: &str) -> io::Result<Splice> {
    let mut removed: Option<String> = None;
    loop {
        let current = removed.as_deref().unwrap_or(raw);
        match json_splice::remove_array_element(current, key, entry)? {
            Splice::Changed(next) => removed = Some(next),
            Splice::Unchanged => break,
        }
    }
    Ok(removed.map_or(Splice::Unchanged, Splice::Changed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The driver is key-agnostic — the same asymmetry holds for any root
    /// array. Vendor-specific coverage lives with each vendor's module;
    /// these pin the contract the two share.
    const KEY: &str = "someArray";

    #[test]
    fn add_creates_file_then_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("cfg.json");

        assert_eq!(sync_managed_element(&cfg, KEY, "a", true).unwrap(), ArraySync::Added);
        let parsed: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(parsed[KEY], serde_json::json!(["a"]));

        assert_eq!(
            sync_managed_element(&cfg, KEY, "a", true).unwrap(),
            ArraySync::Unchanged
        );
    }

    #[test]
    fn remove_drops_the_emptied_key_and_keeps_foreign_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("cfg.json");
        std::fs::write(&cfg, r#"{"other": 1, "someArray": ["mine", "a"]}"#).unwrap();

        assert_eq!(sync_managed_element(&cfg, KEY, "a", false).unwrap(), ArraySync::Removed);
        let parsed: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(parsed[KEY], serde_json::json!(["mine"]));

        assert_eq!(
            sync_managed_element(&cfg, KEY, "mine", false).unwrap(),
            ArraySync::Removed
        );
        let parsed: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert!(
            parsed.get(KEY).is_none(),
            "an emptied managed key is dropped, not left as []"
        );
        assert_eq!(parsed["other"], serde_json::json!(1));
    }

    #[test]
    fn remove_clears_every_copy_of_a_hand_duplicated_element() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("cfg.json");
        std::fs::write(&cfg, r#"{"someArray": ["a", "keep", "a"]}"#).unwrap();

        assert_eq!(sync_managed_element(&cfg, KEY, "a", false).unwrap(), ArraySync::Removed);
        let parsed: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(parsed[KEY], serde_json::json!(["keep"]));
    }

    #[test]
    fn add_is_strict_and_remove_is_tolerant_on_an_unparseable_config() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("cfg.json");
        let garbage = "not json at all {{{";
        std::fs::write(&cfg, garbage).unwrap();

        let err = sync_managed_element(&cfg, KEY, "a", true).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            garbage,
            "a config grim cannot parse is never rewritten"
        );

        assert_eq!(
            sync_managed_element(&cfg, KEY, "a", false).unwrap(),
            ArraySync::Unchanged
        );
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), garbage);
    }

    #[test]
    fn remove_tolerates_a_non_array_managed_key() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("cfg.json");
        let body = r#"{"someArray": "not-an-array"}"#;
        std::fs::write(&cfg, body).unwrap();

        assert_eq!(
            sync_managed_element(&cfg, KEY, "a", false).unwrap(),
            ArraySync::Unchanged
        );
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), body);
    }

    #[test]
    fn remove_from_an_absent_file_is_a_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("cfg.json");

        assert_eq!(
            sync_managed_element(&cfg, KEY, "a", false).unwrap(),
            ArraySync::Unchanged
        );
        assert!(!cfg.exists(), "removal never creates the config file");
    }
}
