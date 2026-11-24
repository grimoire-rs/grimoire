// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim-mirror`: mirror upstream git repositories and Claude plugin
//! marketplaces as grim artifacts. Drives `grim` through its CLI and JSON
//! output only.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod bundle;
mod cli;
mod enumerate;
mod exit_code;
mod git;
mod license;
mod marketplace;
mod model;
mod publish;
mod report;
mod retire;
mod run;
mod spec;
mod stage;
mod version;

fn main() -> std::process::ExitCode {
    cli::main()
}
