// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The launcher's frozen exit table (C-N7).
//!
//! Values are grim's `cli::exit_code::ExitCode` numbers, and a grim-side test
//! asserts the equality. There is no 2: Claude reads a hook exiting 2 as a block.

/// A launcher exit status. Never 2, never a forged 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DispatchExit {
    /// Rows ran and projected, or no row was selected.
    Success = 0,
    /// Any other internal error.
    Failure = 1,
    /// Operand count or grammar mismatch.
    UsageError = 64,
    /// Table unparsable, unknown or oversize schema, an unreadable selected row,
    /// or client stdin over the cap.
    DataError = 65,
    /// `current_exe` unresolvable, or a table or stdin I/O error other than absent.
    IoError = 74,
    /// `dispatch.json` absent while a registration fires.
    NotFound = 79,
}

impl DispatchExit {
    /// Every variant, for exhaustive tests.
    pub const ALL: [Self; 6] = [
        Self::Success,
        Self::Failure,
        Self::UsageError,
        Self::DataError,
        Self::IoError,
        Self::NotFound,
    ];

    /// The process exit status.
    pub const fn code(self) -> u8 {
        self as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_frozen() {
        let table: Vec<(DispatchExit, u8)> = DispatchExit::ALL.iter().map(|e| (*e, e.code())).collect();
        assert_eq!(
            table,
            [
                (DispatchExit::Success, 0),
                (DispatchExit::Failure, 1),
                (DispatchExit::UsageError, 64),
                (DispatchExit::DataError, 65),
                (DispatchExit::IoError, 74),
                (DispatchExit::NotFound, 79),
            ]
        );
    }

    /// Claude reads exit 2 as a block, so no launcher outcome may be 2.
    #[test]
    fn no_variant_is_two() {
        assert!(DispatchExit::ALL.iter().all(|e| e.code() != 2));
    }
}
