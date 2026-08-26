// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Description-companion cache for the detail pane — pure, ratatui-free.
//!
//! The browse catalog is built from manifest annotations and is written to
//! disk, so it can carry everything that is *version-scoped*. Two things it
//! deliberately cannot carry: the repository's `README.md` / `CHANGELOG.md`
//! (they live in the companion's tar layer, not in any annotation) and its
//! [support channels](crate::oci::description::SupportLinks) (which are
//! repository-level and **mutable** — a cached contact link is one that may
//! already have moved).
//!
//! Both are fetched live instead, once per repository per session, on the
//! explicit `enter` that opens the detail pane. This module holds the cache
//! shape; [`super::companion_fetch`] holds the background tasks that fill it.

use crate::oci::description::SupportLinks;

/// Everything the detail pane learned from one repository's companion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Companion {
    /// Repository support channels, read off the companion manifest's
    /// annotations. Every field `None` when the repository publishes none.
    pub support: SupportLinks,
    /// `README.md`, when the companion ships one.
    pub readme: Option<String>,
    /// `CHANGELOG.md`, when the companion ships one.
    pub changelog: Option<String>,
}

impl Companion {
    /// Whether the repository answered with nothing at all — no docs and no
    /// support channels. The pane then shows exactly what it showed before
    /// the fetch: no tab strip, no support section.
    pub fn is_empty(&self) -> bool {
        self.readme.is_none() && self.changelog.is_none() && self.support == SupportLinks::default()
    }
}

/// One repository's slot in the companion cache.
///
/// `Failed` and `Absent` are deliberately distinct: `Absent` is a repository
/// that answered and publishes nothing, `Failed` is one that could not be
/// asked. Both are settled to the automatic trigger — the idle tick runs about
/// five times a second, so re-fetching a failure from there is a request storm
/// — but `Failed` is retried by an explicit `enter`, at a rate bounded by how
/// fast someone can press a key. See `TuiState::companion_to_fetch` and
/// `companion_to_retry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompanionCache {
    /// A fetch is in flight.
    Loading,
    /// The repository answered and published something.
    Ready(Box<Companion>),
    /// The repository answered and publishes no companion and no channels.
    Absent,
    /// The fetch could not complete (offline, auth, transport). Carries the
    /// reason for the pane's one-line notice.
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_companion_is_empty() {
        assert!(Companion::default().is_empty());
    }

    #[test]
    fn any_single_field_makes_it_non_empty() {
        let readme = Companion {
            readme: Some("# hi".to_string()),
            ..Default::default()
        };
        assert!(!readme.is_empty());

        let changelog = Companion {
            changelog: Some("## 1.0.0".to_string()),
            ..Default::default()
        };
        assert!(!changelog.is_empty());

        // Support channels alone are enough: the repository publishes no docs
        // but does say where to file a ticket, and that must still surface.
        let support = Companion {
            support: SupportLinks {
                issues: Some("https://example.invalid/issues".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!support.is_empty());
    }
}
