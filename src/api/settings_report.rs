// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The settings change list and refused-piece rows.
//!
//! Neither is a report of its own: [`SettingsChange`] rows ride the
//! `settings_changes` key of the install, update, add and uninstall reports,
//! and [`RefusedPiece`] rows ride `pieces` on the update report and inside the
//! error document's `error` object. Both are wire contracts: a new field is
//! appended, and no existing field moves.

use serde::Serialize;

/// One value a run added, changed, removed or overwrote in a harness settings
/// file. Holds no value, old or new: element text is the identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingsChange {
    /// The package binding that declared the value.
    pub package: String,
    /// grim's name for the client whose file changed.
    pub client: String,
    /// The settings file, as shown to the user.
    pub file: String,
    /// The pointer of the value inside `file`.
    pub key: String,
    /// The array element, when the value is one element of a shared array.
    pub element: Option<String>,
    /// What the run did to the value.
    pub action: SettingsChangeAction,
}

/// What a run did to one settings value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SettingsChangeAction {
    Added,
    Changed,
    Removed,
    Overwritten,
}

/// One piece a run refused to apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefusedPiece {
    /// The package binding that declared the piece.
    pub package: String,
    /// grim's name for the client the piece targets.
    pub client: String,
    /// The settings file, or `null` on a conflict row, which is about
    /// documents rather than one file.
    pub file: Option<String>,
    /// The pointer of the value.
    pub pointer: String,
    /// The array element, when the piece is one element of a shared array.
    pub element: Option<String>,
    /// How the piece refused.
    pub marked: PieceMark,
    /// Why an overwrite refused, or `null` on a conflict row.
    pub state: Option<PieceState>,
    /// The value the package wants.
    pub wanted: Option<String>,
    /// The value the file holds now.
    pub current: Option<String>,
}

/// How a piece refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PieceMark {
    /// Applying it would replace a value grim did not write; `--force` overrides.
    Overwrite,
    /// Two packages want different values; no `--force` applies.
    Conflict,
}

/// Why an overwrite piece refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PieceState {
    /// A record exists and the value no longer matches it.
    Modified,
    /// No record exists and the file already holds a different value.
    UntrackedDestination,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(action: SettingsChangeAction) -> SettingsChange {
        SettingsChange {
            package: "team".to_string(),
            client: "claude".to_string(),
            file: "~/.claude/settings.json".to_string(),
            key: "/model".to_string(),
            element: None,
            action,
        }
    }

    fn piece() -> RefusedPiece {
        RefusedPiece {
            package: "team".to_string(),
            client: "claude".to_string(),
            file: Some("~/.claude/settings.json".to_string()),
            pointer: "/model".to_string(),
            element: None,
            marked: PieceMark::Overwrite,
            state: Some(PieceState::Modified),
            wanted: Some("sonnet".to_string()),
            current: Some("opus".to_string()),
        }
    }

    #[test]
    fn settings_change_keys_keep_their_frozen_order() {
        assert_eq!(
            serde_json::to_string(&change(SettingsChangeAction::Added)).unwrap(),
            r#"{"package":"team","client":"claude","file":"~/.claude/settings.json","key":"/model","element":null,"action":"added"}"#
        );
    }

    #[test]
    fn refused_piece_keys_keep_their_frozen_order() {
        assert_eq!(
            serde_json::to_string(&piece()).unwrap(),
            r#"{"package":"team","client":"claude","file":"~/.claude/settings.json","pointer":"/model","element":null,"marked":"overwrite","state":"modified","wanted":"sonnet","current":"opus"}"#
        );
    }

    #[test]
    fn a_conflict_row_serializes_a_null_file_and_state() {
        let conflict = RefusedPiece {
            file: None,
            marked: PieceMark::Conflict,
            state: None,
            ..piece()
        };
        let v = serde_json::to_value(&conflict).unwrap();
        assert_eq!(v["file"], serde_json::Value::Null);
        assert_eq!(v["state"], serde_json::Value::Null);
        assert_eq!(v["marked"], "conflict");
    }

    #[test]
    fn every_action_serializes_its_frozen_token() {
        for (action, token) in [
            (SettingsChangeAction::Added, "added"),
            (SettingsChangeAction::Changed, "changed"),
            (SettingsChangeAction::Removed, "removed"),
            (SettingsChangeAction::Overwritten, "overwritten"),
        ] {
            assert_eq!(serde_json::to_value(action).unwrap(), token);
        }
    }

    #[test]
    fn piece_marks_and_states_serialize_their_frozen_tokens() {
        assert_eq!(serde_json::to_value(PieceMark::Overwrite).unwrap(), "overwrite");
        assert_eq!(serde_json::to_value(PieceMark::Conflict).unwrap(), "conflict");
        assert_eq!(serde_json::to_value(PieceState::Modified).unwrap(), "modified");
        assert_eq!(
            serde_json::to_value(PieceState::UntrackedDestination).unwrap(),
            "untracked-destination"
        );
    }
}
