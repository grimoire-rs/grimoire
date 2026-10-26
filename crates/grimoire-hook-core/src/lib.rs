// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Shared hook-dispatch types for `grim` and the `grim-hook` launcher.

// Tests are permitted to unwrap (quality-rust.md); the restriction lints do not
// skip the test target under `--all-targets`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod argv;
pub mod audit;
pub mod diag;
pub mod envelope;
pub mod event;
pub mod exit;
pub mod glob;
pub mod manifest;
pub mod pipeline;
pub mod projection;
pub mod projector;
pub mod response;
pub mod run;
pub mod table;
pub mod time;

pub use diag::{Diagnostic, Level};
pub use event::{CanonicalEvent, HookTier, canonical_event};
pub use exit::DispatchExit;
pub use manifest::{
    DEFAULT_TIMEOUT_SECS, HOOK_SCHEMA_VERSION, HookHandler, HookPayloadMode, MATCHER_ALLOWED, MATCHER_MAX_BYTES,
    MAX_TIMEOUT_SECS, MIN_TIMEOUT_SECS, matcher_char_allowed,
};
pub use pipeline::WithheldCause;
pub use projection::{EVENT_ECHO_FIELD, ProjectionRow, RESPONSE_PROJECTION, VerdictTokens, projection_for};
pub use run::{AUDIT_FILE, DispatchOutcome, PayloadFault, dispatch};

/// Maximum size of the `grim-hook` launcher binary.
#[cfg(not(target_env = "musl"))]
pub const LAUNCHER_SIZE_BUDGET: usize = 512 * 1024;

/// Maximum size of the `grim-hook` launcher binary (musl links a larger std).
#[cfg(target_env = "musl")]
pub const LAUNCHER_SIZE_BUDGET: usize = 640 * 1024;
