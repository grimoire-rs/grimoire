// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The embedded native launcher bytes, and the path check a registration needs.
//!
//! The registered command string is rendered by `hook_command` over
//! `grimoire_hook_core::argv::render`, and `launcher::path` names the native binary it
//! points at. The old `bin/grim-hook` shell shim is gone: convergence deletes it
//! (`launcher::retire_legacy_shim`).

use std::path::Path;

/// The native `grim-hook` launcher, built by `build.rs` (C-N2) and embedded.
///
/// Debug builds embed the `hook-dev` profile; release builds embed `hook` and
/// are held to [`grimoire_hook_core::LAUNCHER_SIZE_BUDGET`] at compile time.
pub static LAUNCHER_BYTES: &[u8] = include_bytes!(env!("GRIM_HOOK_BIN"));

#[cfg(not(debug_assertions))]
const _: () = assert!(LAUNCHER_BYTES.len() <= grimoire_hook_core::LAUNCHER_SIZE_BUDGET);

/// Why a command string could not be generated.
///
/// One variant, and it is a **refusal to arm**, not a failure: the registrar
/// maps it to `ArmRefusal::LauncherPathControlChar` and reports `not-armed`
/// (C-017 cause 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandRefusal {
    /// The resolved launcher or table path contains a newline or another control
    /// character (B2).
    ///
    /// No vendor's JSON-plus-shell round trip has a correct quoting for a
    /// newline, and no legitimate path needs one — so grim refuses rather than
    /// writing a registration whose behaviour it cannot predict. Note this is a
    /// property of `$GRIM_HOME`, which is environment-derived, which is exactly
    /// why C-018b had to be widened from "no publisher-controlled value" to "no
    /// value grim did not itself choose".
    ControlCharacterInPath,
}

impl CommandRefusal {
    /// The reason phrase, library style (lowercase, no trailing punctuation).
    pub fn reason(self) -> &'static str {
        match self {
            Self::ControlCharacterInPath => "the resolved launcher or table path contains a control character",
        }
    }
}

impl std::fmt::Display for CommandRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

/// Whether `path` can be embedded in a registration at all (B2).
///
/// Read-only, so [`super::hook_registrar::arming_refusal`] can answer before a
/// client, an event or a root token exist.
///
/// # Errors
///
/// [`CommandRefusal::ControlCharacterInPath`].
pub fn path_is_representable(path: &Path) -> Result<(), CommandRefusal> {
    if path.to_string_lossy().chars().any(char::is_control) {
        return Err(CommandRefusal::ControlCharacterInPath);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_launcher_is_a_non_empty_binary() {
        assert!(!LAUNCHER_BYTES.is_empty());
    }
}
