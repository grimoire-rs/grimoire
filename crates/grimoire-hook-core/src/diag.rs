// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Diagnostics as values: core logs nothing, it returns what a log line would say.
//!
//! The caller renders them: grim through `tracing` at the same levels, the launcher as
//! one stderr line each. A message never carries a payload, an envelope or an
//! environment value (I6); it may name a hook, a tier, a path grim owns or an OS error.

/// How loudly a [`Diagnostic`] is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Always shown: something degraded or was refused.
    Warn,
    /// Shown only when the caller asks for debug output.
    Debug,
}

/// One log line, as a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub level: Level,
    pub message: String,
}

impl Diagnostic {
    pub fn warn(message: impl Into<String>) -> Self {
        Self {
            level: Level::Warn,
            message: message.into(),
        }
    }

    pub fn debug(message: impl Into<String>) -> Self {
        Self {
            level: Level::Debug,
            message: message.into(),
        }
    }
}
