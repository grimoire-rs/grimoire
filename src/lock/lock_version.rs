// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grimoire.lock` on-disk format version discriminant.

use serde_repr::{Deserialize_repr, Serialize_repr};

/// Lock-file format version.
///
/// Serialized as a bare integer via `serde_repr`; an unknown discriminant
/// fails deserialization at the serde layer (no silent fallback). A future
/// format adds `V2 = 2` alongside `V1`; existing v1 files keep parsing.
///
/// Closed internal on-disk discriminant — not `#[non_exhaustive]`, per the
/// project convention that internal non-error enums stay total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize_repr, Deserialize_repr)]
#[repr(u8)]
pub enum LockVersion {
    /// Version 1 of the on-disk format.
    V1 = 1,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_serializes_as_bare_integer() {
        assert_eq!(serde_json::to_string(&LockVersion::V1).unwrap(), "1");
    }

    #[test]
    fn known_discriminant_round_trips() {
        let v: LockVersion = serde_json::from_str("1").unwrap();
        assert_eq!(v, LockVersion::V1);
    }

    #[test]
    fn unknown_discriminant_rejected() {
        assert!(serde_json::from_str::<LockVersion>("2").is_err());
        assert!(serde_json::from_str::<LockVersion>("0").is_err());
    }
}
