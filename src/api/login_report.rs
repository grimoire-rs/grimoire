// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim login` / `grim logout` output.
//!
//! Plain format: a single confirmation table — `Registry | Username |
//! Verification` for login, `Registry` for logout.
//!
//! JSON format: a single object
//! (`{"registry","username","verification"}` / `{"registry"}`), not an
//! array — there is exactly one subject.

use std::io::{self, Write};

use serde::Serialize;

use crate::cli::printer::{Printable, print_table};

/// How the credential was checked against the registry before it was
/// stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerificationStatus {
    /// The registry's auth endpoint accepted the credential.
    Verified,
    /// The registry does not require authentication; nothing to verify.
    NoAuthRequired,
    /// Verification was skipped (`--no-verify`, or offline mode).
    Skipped,
}

impl std::fmt::Display for VerificationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Verified => "verified",
            Self::NoAuthRequired => "no-auth-required",
            Self::Skipped => "skipped",
        })
    }
}

/// The result of a successful `grim login`.
#[derive(Debug, Serialize)]
pub struct LoginReport {
    /// The registry the credential was stored for (canonical form).
    pub registry: String,
    /// The account name that was authenticated.
    pub username: String,
    /// How the credential was verified before it was stored.
    pub verification: VerificationStatus,
}

impl LoginReport {
    /// Build from the resolved registry, username, and verification
    /// outcome.
    pub fn new(registry: impl Into<String>, username: impl Into<String>, verification: VerificationStatus) -> Self {
        Self {
            registry: registry.into(),
            username: username.into(),
            verification,
        }
    }
}

impl Printable for LoginReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        print_table(
            w,
            &["Registry", "Username", "Verification"],
            &[vec![
                self.registry.clone(),
                self.username.clone(),
                self.verification.to_string(),
            ]],
        )
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, self)
    }
}

/// The result of a successful `grim logout`.
///
/// Plain format: 1-column table (Registry), plus a trailing `note:` line —
/// **only** when the global config was dropped. The normal path is
/// byte-identical to before the field existed; a new *column* would have
/// shifted the layout for every invocation.
///
/// JSON format: one flat object.
#[derive(Debug, Serialize)]
pub struct LogoutReport {
    /// The registry the credential was removed for (canonical form).
    pub registry: String,
    /// Whether the global config was unreadable and its `[[registries]]` /
    /// `default_registry` tier was therefore dropped while resolving
    /// `registry` above. Exit stays **0** either way, so this is the only
    /// machine-readable signal that an alias declared solely in that tier
    /// did not substitute — and that no credential was erased for it.
    pub global_config_dropped: bool,
}

impl LogoutReport {
    /// Build from the resolved registry and the degrade flag from
    /// `command::resolve_login_registry_lenient`.
    pub fn new(registry: impl Into<String>, global_config_dropped: bool) -> Self {
        Self {
            registry: registry.into(),
            global_config_dropped,
        }
    }
}

impl Printable for LogoutReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        // Escaped for the same reason the note below is: under `Lenient` this
        // is the authored alias that fell through to a literal, so an
        // unescaped U+202E would reorder the row sitting directly above a
        // line explaining that nothing was removed.
        print_table(w, &["Registry"], &[vec![self.registry.escape_debug().to_string()]])?;
        if self.global_config_dropped {
            // Escaped: `registry` can be the literal argv value when the
            // alias did not substitute, which is exactly this branch.
            writeln!(
                w,
                "note: the global config could not be read; if '{}' names an alias declared only there, \
                 no credential was removed for it",
                self.registry.escape_debug()
            )?;
        }
        Ok(())
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_plain_is_single_table_with_header() {
        let r = LoginReport::new("ghcr.io", "alice", VerificationStatus::Verified);
        let mut buf = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].starts_with("Registry"));
        assert!(lines[0].contains("Username"));
        assert!(lines[0].contains("Verification"));
        assert!(lines[1].contains("ghcr.io"));
        assert!(lines[1].contains("alice"));
        assert!(lines[1].contains("verified"));
    }

    #[test]
    fn login_json_is_single_object() {
        let r = LoginReport::new("ghcr.io", "alice", VerificationStatus::Verified);
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(v.is_object());
        assert_eq!(v["registry"], "ghcr.io");
        assert_eq!(v["username"], "alice");
        assert_eq!(v["verification"], "verified");
    }

    #[test]
    fn verification_status_serializes_kebab_case() {
        for (status, expected) in [
            (VerificationStatus::Verified, "verified"),
            (VerificationStatus::NoAuthRequired, "no-auth-required"),
            (VerificationStatus::Skipped, "skipped"),
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), expected);
            assert_eq!(status.to_string(), expected, "Display must match Serialize");
        }
    }

    #[test]
    fn logout_json_never_carries_a_username() {
        let r = LogoutReport::new("ghcr.io", false);
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["registry"], "ghcr.io");
        assert!(v.get("username").is_none());
    }

    /// W-S4: the disclosure field is **always present**, both values — an
    /// absent-key optional cannot be distinguished from an older `grim`,
    /// which is exactly what a consumer reading it needs to tell apart.
    #[test]
    fn logout_json_always_carries_the_degrade_flag_ws4() {
        for dropped in [false, true] {
            let mut buf = Vec::new();
            LogoutReport::new("ghcr.io", dropped).print_json(&mut buf).unwrap();
            let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
            assert_eq!(
                v.get("global_config_dropped"),
                Some(&serde_json::Value::Bool(dropped)),
                "the key must be present for both values; got: {v}"
            );
        }
    }

    /// W-S4: the plain surface is additive in the one direction Principle 9
    /// allows — the undegraded path is byte-identical to the pre-field
    /// output, so a positional consumer (`awk '{print $1}'`) is untouched,
    /// and the degraded path appends a line rather than widening the table.
    #[test]
    fn logout_plain_appends_a_note_only_when_degraded_ws4() {
        let mut clean = Vec::new();
        LogoutReport::new("ghcr.io", false).print_plain(&mut clean).unwrap();
        let clean = String::from_utf8(clean).unwrap();
        assert!(
            !clean.contains("note:"),
            "the normal path must not grow output; got: {clean:?}"
        );

        let mut table_only = Vec::new();
        LogoutReport::new("acme", false).print_plain(&mut table_only).unwrap();
        let table_only = String::from_utf8(table_only).unwrap();

        let mut degraded = Vec::new();
        LogoutReport::new("acme", true).print_plain(&mut degraded).unwrap();
        let degraded = String::from_utf8(degraded).unwrap();
        assert!(
            degraded.starts_with(&table_only),
            "the note must come AFTER a byte-identical table; got: {degraded:?}"
        );
        // Exit is 0 on this path, so the line has to say what did NOT happen.
        assert!(
            degraded.contains("no credential was removed"),
            "the note must name the exit-0 hazard, not merely 'config degraded'; got: {degraded:?}"
        );
        assert!(
            degraded.contains("'acme'"),
            "the note must name this invocation's registry; got: {degraded:?}"
        );
    }

    /// W-S4 + W-S3: on the degraded path `registry` is the raw argv value
    /// (the alias did not substitute), and the note wraps it in prose — so
    /// a bidi override would reorder the sentence around it. `is_control`
    /// is false for U+202E, so no upstream screen catches it.
    #[test]
    fn logout_plain_note_escapes_the_registry_ws4() {
        let mut buf = Vec::new();
        LogoutReport::new("ac\u{202e}me", true).print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        let note = out.lines().find(|l| l.starts_with("note:")).expect("the note fires");
        assert!(
            !note.contains('\u{202e}'),
            "no raw bidi override may reach the note; got: {note:?}"
        );
        assert!(
            note.contains("ac\\u{202e}me"),
            "the registry must stay readable, escaped; got: {note:?}"
        );
    }

    /// The sweep the note alone did not finish: the table cell echoes the same
    /// authored value, on the same page, and escaping one of the two is a fix
    /// in name only. Asserted on the **undegraded** path so it holds for every
    /// `grim logout`, not just the branch that prints a note.
    #[test]
    fn logout_plain_table_cell_escapes_the_registry() {
        let mut buf = Vec::new();
        LogoutReport::new("ac\u{202e}me", false).print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(
            !out.contains('\u{202e}'),
            "no raw bidi override may reach the table; got: {out:?}"
        );
        assert!(
            out.contains("ac\\u{202e}me"),
            "the registry must stay readable, escaped; got: {out:?}"
        );
    }
}
