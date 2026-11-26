// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

use std::fmt;

use crate::process::Output;

/// A failed call to `grim`, carrying the exit code it contributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverError {
    /// Exit code of grim; `None` only for a spawn failure (a deadline is `Some(75)`).
    pub code: Option<i32>,
    pub message: String,
}

impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DriverError {}

pub(crate) fn unexpected(what: &str, o: &Output) -> DriverError {
    let detail = o.stderr.trim();
    DriverError {
        code: Some(69),
        message: format!("unexpected grim output: {what} (exit {:?}) {detail}", o.code),
    }
}

/// Keeps the infrastructure codes grim documents; anything else is 69.
pub(crate) fn exit_failure(o: &Output, what: &str) -> DriverError {
    match o.code {
        Some(c @ (69 | 74 | 75 | 80 | 81)) => DriverError {
            code: Some(c),
            message: format!("grim {what} failed ({c}): {}", o.stderr.trim()),
        },
        _ => unexpected(what, o),
    }
}
