// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Typed accessors over Grimoire's environment variables.
//!
//! `GRIM_LOG` is intentionally absent here — log filtering is initialized
//! directly in `main.rs` before anything else runs.

use std::path::PathBuf;

/// Environment variable holding the Grimoire data root.
const GRIM_HOME: &str = "GRIM_HOME";
/// Environment variable holding the default registry for short identifiers.
const GRIM_DEFAULT_REGISTRY: &str = "GRIM_DEFAULT_REGISTRY";
/// Environment variable that, when truthy, disables all network access.
const GRIM_OFFLINE: &str = "GRIM_OFFLINE";
/// Environment variable that, when truthy, arms opt-in usage telemetry.
const GRIM_TELEMETRY: &str = "GRIM_TELEMETRY";
/// Environment variable that, when truthy, adds the redacted git origin to
/// the telemetry resource.
const GRIM_TELEMETRY_REPOSITORY: &str = "GRIM_TELEMETRY_REPOSITORY";
/// The OpenTelemetry-standard kill switch; the OTLP crate ignores it, so
/// grim reads it itself.
const OTEL_SDK_DISABLED: &str = "OTEL_SDK_DISABLED";
/// The cross-tool opt-out convention.
const DO_NOT_TRACK: &str = "DO_NOT_TRACK";
/// Docker-compatible config directory override, honoured by `grim login`
/// / `grim logout` and the credential read path for parity with `docker`.
const DOCKER_CONFIG: &str = "DOCKER_CONFIG";

/// Resolves the Grimoire data root.
///
/// Uses `$GRIM_HOME` when set and non-empty, otherwise `~/.grimoire`. If
/// the home directory cannot be determined, falls back to `.grimoire`
/// relative to the current directory so the binary still runs.
pub fn grim_home() -> PathBuf {
    if let Some(dir) = non_empty_var(GRIM_HOME) {
        return PathBuf::from(dir);
    }
    match home_dir() {
        Some(home) => home.join(".grimoire"),
        None => PathBuf::from(".grimoire"),
    }
}

/// Returns the configured default registry, if `$GRIM_DEFAULT_REGISTRY`
/// is set and non-empty.
pub fn default_registry() -> Option<String> {
    non_empty_var(GRIM_DEFAULT_REGISTRY)
}

/// Whether offline mode is requested via `$GRIM_OFFLINE`.
pub fn offline() -> bool {
    truthy(GRIM_OFFLINE)
}

/// Whether usage telemetry is switched on via `$GRIM_TELEMETRY`.
pub fn telemetry() -> bool {
    truthy(GRIM_TELEMETRY)
}

/// Whether repository context is requested via `$GRIM_TELEMETRY_REPOSITORY`.
pub fn telemetry_repository() -> bool {
    truthy(GRIM_TELEMETRY_REPOSITORY)
}

/// Whether `$OTEL_SDK_DISABLED` is truthy.
pub fn otel_sdk_disabled() -> bool {
    truthy(OTEL_SDK_DISABLED)
}

/// Whether `$DO_NOT_TRACK` is truthy.
pub fn do_not_track() -> bool {
    truthy(DO_NOT_TRACK)
}

/// Whether the process runs under CI, judged by the names `CI`,
/// `GITHUB_ACTIONS` and `GITLAB_CI` only. Credentials are never loaded to
/// answer this (`catalog::forge::CiEnv` would).
pub fn ci() -> bool {
    ci_in(&|key| std::env::var(key).ok())
}

/// [`ci`] over an injected lookup, so tests never mutate the process env.
fn ci_in(lookup: &dyn Fn(&str) -> Option<String>) -> bool {
    ["CI", "GITHUB_ACTIONS", "GITLAB_CI"]
        .into_iter()
        .any(|key| lookup(key).is_some_and(|v| is_truthy(&v)))
}

/// Resolves the docker-compatible credential config path.
///
/// `$DOCKER_CONFIG/config.json` when `$DOCKER_CONFIG` is set and
/// non-empty, otherwise `~/.docker/config.json`. Returns `None` when
/// neither `$DOCKER_CONFIG` nor a home directory can be determined — the
/// caller surfaces that as a configuration error.
pub fn docker_config_path() -> Option<PathBuf> {
    if let Some(dir) = non_empty_var(DOCKER_CONFIG) {
        return Some(PathBuf::from(dir).join("config.json"));
    }
    home_dir().map(|home| home.join(".docker").join("config.json"))
}

fn non_empty_var(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(non_empty)
}

fn non_empty(value: String) -> Option<String> {
    Some(value).filter(|v| !v.is_empty())
}

/// Treats `1`, `true`, `yes`, `on` (case-insensitive) as enabled.
fn truthy(key: &str) -> bool {
    std::env::var(key).is_ok_and(|v| is_truthy(&v))
}

fn is_truthy(value: &str) -> bool {
    matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

/// Best-effort home directory without an external crate: `$HOME` on Unix,
/// `%USERPROFILE%` on Windows.
pub(crate) fn home_dir() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    non_empty_var(key).map(PathBuf::from)
}

/// The directory the project-scope config walk-up stops at (inclusive):
/// the user's home directory, if it can be determined. `None` lets the
/// walk run to the filesystem root.
pub fn home_dir_for_ceiling() -> Option<PathBuf> {
    home_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Env mutation is `unsafe` in edition 2024 and the crate forbids
    // unsafe, so the parse logic is tested via the pure inner helpers
    // rather than by mutating the process environment.

    #[test]
    fn is_truthy_recognizes_common_forms() {
        for v in ["1", "true", "YES", "On", " true "] {
            assert!(is_truthy(v), "expected truthy: {v:?}");
        }
        for v in ["0", "false", "", "no", "off", "x"] {
            assert!(!is_truthy(v), "expected falsy: {v:?}");
        }
    }

    #[test]
    fn truthy_set_is_exactly_the_four_documented_words() {
        // `ci()` and the telemetry gate both ride `is_truthy`: a fifth
        // accepted word would arm telemetry where the docs say it is off.
        for v in ["1", "true", "yes", "on", "TRUE", " On "] {
            assert!(is_truthy(v), "{v:?}");
        }
        for v in ["", "0", "false", "no", "off", "2", "enabled", "y"] {
            assert!(!is_truthy(v), "{v:?}");
        }
    }

    #[test]
    fn ci_reads_exactly_the_three_named_variables_and_nothing_else() {
        let with = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| (*v).to_string())
        };
        assert!(ci_in(&with(&[("CI", "true")])));
        assert!(ci_in(&with(&[("GITHUB_ACTIONS", "1")])));
        assert!(ci_in(&with(&[("GITLAB_CI", "yes")])));
        assert!(!ci_in(&with(&[])));
        assert!(!ci_in(&with(&[
            ("CI", "0"),
            ("GITHUB_ACTIONS", "false"),
            ("GITLAB_CI", "")
        ])));
        // A credential variable must never decide the answer.
        assert!(!ci_in(&with(&[
            ("GH_TOKEN", "1"),
            ("GITHUB_TOKEN", "1"),
            ("GITLAB_TOKEN", "1")
        ])));
    }

    #[test]
    fn non_empty_filters_empty_string() {
        assert_eq!(non_empty(String::new()), None);
        assert_eq!(non_empty("x".to_string()), Some("x".to_string()));
    }

    #[test]
    fn grim_home_default_ends_in_dot_grimoire() {
        // When GRIM_HOME is unset (the default for the test process), the
        // resolved path is `<home>/.grimoire` or the relative fallback.
        if std::env::var(GRIM_HOME).is_err() {
            assert!(grim_home().ends_with(".grimoire"));
        }
    }
}
