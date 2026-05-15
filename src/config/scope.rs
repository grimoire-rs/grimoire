// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Which configuration scope a command operates on.

/// The two independent configuration scopes.
///
/// Global (`$GRIM_HOME/grimoire.toml`) and project (discovered by walking
/// up from the working directory) each own their own lock and are **never
/// merged** — a command operates on exactly one scope.
///
/// Closed internal enum: the binary is the only consumer, so matches stay
/// total — no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfigScope {
    /// The per-user global declaration under `$GRIM_HOME`.
    Global,
    /// A project-local declaration discovered by walking up from the CWD.
    Project,
}

impl std::fmt::Display for ConfigScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Global => "global",
            Self::Project => "project",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_lowercase() {
        assert_eq!(ConfigScope::Global.to_string(), "global");
        assert_eq!(ConfigScope::Project.to_string(), "project");
    }
}
