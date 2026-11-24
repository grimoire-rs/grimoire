// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The subset of grim's exit codes the mirror uses. Values are pinned to
//! `src/cli/exit_code.rs` by a test.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExitCode {
    Success = 0,
    Failure = 1,
    UsageError = 64,
    DataError = 65,
    Unavailable = 69,
    IoError = 74,
    TempFail = 75,
    NoPermission = 77,
    ConfigError = 78,
    NotFound = 79,
    AuthError = 80,
    OfflineBlocked = 81,
}

impl From<ExitCode> for std::process::ExitCode {
    fn from(value: ExitCode) -> Self {
        std::process::ExitCode::from(value as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::ExitCode::*;

    const GRIM: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../src/cli/exit_code.rs"));

    fn grim_variants() -> Vec<(String, u8)> {
        let body = GRIM
            .split("pub enum ExitCode {")
            .nth(1)
            .and_then(|r| r.split("\n}").next())
            .unwrap_or_default();
        body.lines()
            .filter_map(|l| {
                let (name, value) = l.trim().strip_suffix(',')?.split_once(" = ")?;
                Some((name.to_owned(), value.parse().ok()?))
            })
            .collect()
    }

    #[test]
    fn values_match_grims_exit_codes() {
        let mirror = [
            ("Success", Success),
            ("Failure", Failure),
            ("UsageError", UsageError),
            ("DataError", DataError),
            ("Unavailable", Unavailable),
            ("IoError", IoError),
            ("TempFail", TempFail),
            ("NoPermission", NoPermission),
            ("ConfigError", ConfigError),
            ("NotFound", NotFound),
            ("AuthError", AuthError),
            ("OfflineBlocked", OfflineBlocked),
        ];
        let grim = grim_variants();
        assert!(grim.len() >= mirror.len(), "grim variants not parsed: {grim:?}");
        for (name, code) in mirror {
            let theirs = grim.iter().find(|(n, _)| n == name).map(|(_, v)| *v);
            assert_eq!(theirs, Some(code as u8), "{name}");
        }
    }
}
