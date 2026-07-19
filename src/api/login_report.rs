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
#[derive(Debug, Serialize)]
pub struct LogoutReport {
    /// The registry the credential was removed for (canonical form).
    pub registry: String,
}

impl LogoutReport {
    /// Build from the resolved registry.
    pub fn new(registry: impl Into<String>) -> Self {
        Self {
            registry: registry.into(),
        }
    }
}

impl Printable for LogoutReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        print_table(w, &["Registry"], &[vec![self.registry.clone()]])
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
    fn logout_json_carries_only_registry() {
        let r = LogoutReport::new("ghcr.io");
        let mut buf = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["registry"], "ghcr.io");
        assert!(v.get("username").is_none());
    }
}
