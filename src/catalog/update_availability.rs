// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The registry-aware "is a newer version available?" seam.
//!
//! Extracted from `tui::update_check` (issue #43): the command layer must
//! not depend on the TUI, but `grim status`'s update-availability check
//! needs the exact same decision the TUI's background re-check uses. This
//! module holds only the pure decision and the OCI-access read that feeds
//! it — no TUI types, no background-task machinery. [`super::super::tui`]'s
//! `UpdateChecker` consumes this seam; it still owns the concurrency bound,
//! the results channel, and the in-flight bookkeeping around it.

use crate::catalog::registry_catalog::pick_latest_tag;
use crate::oci::access::error::AccessError;
use crate::oci::access::{OciAccess, Operation};
use crate::oci::{Digest, Identifier};

/// The pure registry-aware "outdated" decision.
///
/// `true` ⇒ the registry resolved the floating tag to a digest that differs
/// from the locked pin ⇒ a newer version is available. A resolve of `None`
/// (the tag vanished, or offline returned nothing) is **not** "outdated":
/// absence is never treated as a newer pin, so the icon never lies on a
/// transient miss.
pub fn outdated_from_resolve(locked: &Digest, resolved: Option<&Digest>) -> bool {
    matches!(resolved, Some(d) if d != locked)
}

/// Discover the registry's current representative-tag digest for `base` (a
/// `registry/repository` identifier; any tag already on it is ignored),
/// independent of any cached catalog tag.
///
/// Issue #21 ("update not shown"): the cached catalog row's `latest_tag` is
/// captured at build time and served from a cache with a freshness window. On
/// a registry that carries only immutable semver tags (no moving `latest`),
/// resolving that captured tag can never reveal a newer release — the new
/// version lands as a brand-new higher tag the stale catalog has not seen. So
/// the background check re-discovers the latest tag *fresh*: list the repo's
/// tags, pick the same representative tag the catalog build would
/// ([`pick_latest_tag`] — prefer `latest`, else the highest semver), and
/// resolve it. `Ok(None)` when the repo has no tags or no resolvable
/// representative; absence is never treated as a newer pin (the icon never
/// lies on a transient miss). This costs one extra round-trip (`list_tags`)
/// per eligible row over resolving a fixed tag — paid only for
/// installed/outdated rows, not the whole catalog.
pub async fn resolve_latest_digest(access: &dyn OciAccess, base: &Identifier) -> Result<Option<Digest>, AccessError> {
    let Some(tags) = access.list_tags(base).await? else {
        return Ok(None);
    };
    let Some(tag) = pick_latest_tag(&tags) else {
        return Ok(None);
    };
    access.resolve_digest(&base.clone_with_tag(tag), Operation::Query).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::Algorithm;

    fn digest(seed: &[u8]) -> Digest {
        Algorithm::Sha256.hash(seed)
    }

    // ── outdated_from_resolve truth table ────────────────────────────────

    #[test]
    fn outdated_when_resolved_differs_from_locked() {
        let locked = digest(b"locked");
        let newer = digest(b"newer");
        assert!(
            outdated_from_resolve(&locked, Some(&newer)),
            "different digest ⇒ outdated"
        );
    }

    #[test]
    fn not_outdated_when_resolved_equals_locked() {
        let locked = digest(b"same");
        let same = digest(b"same");
        assert!(
            !outdated_from_resolve(&locked, Some(&same)),
            "identical digest ⇒ up to date"
        );
    }

    #[test]
    fn not_outdated_when_resolve_is_none() {
        let locked = digest(b"locked");
        assert!(
            !outdated_from_resolve(&locked, None),
            "a vanished/offline tag is never treated as a newer pin"
        );
    }
}
