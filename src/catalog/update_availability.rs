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
//!
//! **The question is "would `grim update` move this pin?", not "does the
//! repository carry a higher version?"** Those two differ the moment a
//! declaration is narrower than the repository — a `:0.12` float or a
//! `:0.12.0`/digest pin — and only the first is actionable: `↑ outdated`
//! and `update_available` drive the user to `grim update`, which re-resolves
//! the **declared** reference and nothing else. So the check resolves that
//! declared reference, not the repository's globally highest tag.

use crate::oci::access::error::AccessError;
use crate::oci::access::{OciAccess, Operation};
use crate::oci::{Digest, Identifier};

/// The pure registry-aware "outdated" decision.
///
/// `true` ⇒ the registry resolved the declared reference to a digest that
/// differs from the locked pin ⇒ a newer version is available. A resolve of
/// `None` (the tag vanished, or offline returned nothing) is **not**
/// "outdated": absence is never treated as a newer pin, so the icon never
/// lies on a transient miss.
pub fn outdated_from_resolve(locked: &Digest, resolved: Option<&Digest>) -> bool {
    matches!(resolved, Some(d) if d != locked)
}

/// Resolve the artifact's **declared** reference — `registry/repository:tag`
/// exactly as `grimoire.toml` spells it (or the member id a bundle baked) —
/// to the digest it points at right now.
///
/// This is deliberately the same single `resolve_digest` the resolver runs
/// under `grim update` (`resolve::resolver::retry_chain`), so the two can
/// never disagree about whether an update exists. [`Operation::Query`] keeps
/// it a read-only probe — no tag pointer is written through to the cache.
///
/// `Ok(None)` when the declared tag does not exist (a vanished tag, or an
/// offline miss); absence is never treated as a newer pin, so the icon never
/// lies on a transient miss.
///
/// **Never re-discover the repository's latest tag here.** An earlier
/// revision listed the repo's tags and resolved the highest one, which made
/// every row whose declaration pins or floats *below* the repository head
/// report an update that `grim update` would not apply — a `:0.12.0` pin sat
/// permanently at `↑ outdated` once `0.13.0` shipped. Freshness against a
/// stale cached catalog tag (issue #21) is preserved regardless: the tag
/// comes from the declaration, which the cache never touches.
pub async fn resolve_declared_digest(
    access: &dyn OciAccess,
    declared: &Identifier,
) -> Result<Option<Digest>, AccessError> {
    access.resolve_digest(declared, Operation::Query).await
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
