// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Command output data layer.
//!
//! Each command builds one of these report types from its operation
//! results (never from echoed args) and renders it through [`Printable`]:
//! `print_plain` makes exactly one `print_table` call with static
//! headers; `print_json` emits a pretty JSON array (or, for `init`, a
//! single object) with no wrapper.

pub mod artifact_status;
pub mod init_report;
pub mod install_report;
pub mod lock_report;
pub mod status_report;
pub mod update_report;

#[allow(unused_imports)]
pub use artifact_status::{ArtifactStatus, InitStatus, InstallStatus, LockAction, UpdateAction};
#[allow(unused_imports)]
pub use init_report::InitReport;
#[allow(unused_imports)]
pub use install_report::{InstallEntry, InstallReport};
#[allow(unused_imports)]
pub use lock_report::{LockEntry, LockReport};
#[allow(unused_imports)]
pub use status_report::{StatusEntry, StatusReport};
#[allow(unused_imports)]
pub use update_report::{UpdateEntry, UpdateReport};
