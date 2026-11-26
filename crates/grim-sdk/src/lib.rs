// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Drives the `grim` binary over its CLI and JSON output. Unstable and
//! unpublished; it links nothing from the `grimoire` crate.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod bin;
mod driver;
mod error;
mod exit_code;
#[cfg(all(test, unix))]
mod fake;
mod process;
mod version;

pub use bin::{bin_from_env, resolve_bin};
pub use driver::{ConvertOutcome, GrimDriver, Probe, PublishItem, PublishStatus, Recorded};
pub use error::DriverError;
pub use exit_code::ExitCode;
pub use process::{CALL_DEADLINE, Output, PUBLISH_DEADLINE, run, run_retry};
