// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The canonical verdict and response a payload returns, before any
//! per-client projection.

use serde::{Deserialize, Serialize};

use crate::table::StrictValue;

/// A canonical verdict — grim's own small, closed vocabulary (C-003).
///
/// Anything richer is native passthrough for one declared vendor, never a
/// fifth variant here.
///
/// Closed internal enum: the binary is the only consumer, so matches stay
/// total — no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    /// Let the operation proceed.
    Allow,
    /// Block the operation. **Absorbing** — see `aggregate`.
    Deny,
    /// Escalate to the user. Outranks [`Allow`](Self::Allow).
    Ask,
    /// No opinion; the client's own default applies.
    ///
    /// The `Default`, because a payload that answers `{}` has expressed no
    /// verdict and every degrade path lands here (I3).
    #[default]
    None,
}

impl Decision {
    /// Aggregation rank, lowest first: `none` < `allow` < `ask` < `deny`.
    ///
    /// A total order rather than a pairwise `match`, because parts 3 and 4 of
    /// Decision O are one statement — "the most restrictive verdict wins" —
    /// and expressing it twice is how `ask` eventually loses to `allow` in one
    /// of the two places.
    pub fn rank(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Allow => 1,
            Self::Ask => 2,
            Self::Deny => 3,
        }
    }
}

impl std::fmt::Display for Decision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Ask => "ask",
            Self::None => "none",
        })
    }
}

/// The canonical response a payload returns, before any per-client projection
/// (C-003).
///
/// Small and closed on purpose. It is projected onto the invoking client's
/// shape by `projector`, which refuses any field that pair has no
/// target for rather than dropping it silently.
///
/// Deserialized straight from the payload's stdout, every field defaulted: a
/// hook answers only about what it has an opinion on, and `{}` is the
/// no-opinion answer rather than a malformed one. Unknown members are ignored
/// (not denied) so a payload written against a newer envelope schema still
/// answers this one — the same degrade direction W2 takes for the table.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(try_from = "StrictValue")]
pub struct CanonicalResponse {
    /// The verdict.
    pub decision: Decision,
    /// Human-readable justification accompanying a verdict. Required by codex
    /// when blocking — enforced in its **output parser** rather than its JSON
    /// schema, so an omitted reason fails closed rather than validating.
    pub reason: Option<String>,
    /// Extra context for the agent, where the client can express it.
    pub context: Option<String>,
    /// A message aimed at the user rather than the model.
    pub user_message: Option<String>,
    /// Whether the agent's turn should stop.
    pub stop: bool,
    /// The rewritten tool input — `mutator` tier only, `PreToolUse` only.
    ///
    /// **S-016:** where the client's response shape supports it, a rewrite
    /// also emits a line describing itself through `context` /
    /// `user_message`, so the agent's own transcript records that its command
    /// was altered. No vendor does this by default, and mutator control 5
    /// exists precisely because a silent rewrite is indistinguishable from the
    /// model having asked for the new command.
    pub updated_input: Option<serde_json::Value>,
}

/// The member a response failed on.
#[derive(Debug)]
pub struct BadMember(&'static str);

impl std::fmt::Display for BadMember {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "member `{}` has the wrong type", self.0)
    }
}

/// A response read from bytes refuses duplicate keys, so `{"decision":"allow",
/// "decision":"deny"}` is unreadable instead of whichever came last.
impl TryFrom<StrictValue> for CanonicalResponse {
    type Error = BadMember;

    fn try_from(value: StrictValue) -> Result<Self, BadMember> {
        Self::try_from(value.0)
    }
}

/// Extracted by hand: serde's derive is a measurable share of the launcher's
/// size budget, and this is the one reader for the launcher and `grim`. Every
/// member is optional, unknown members are ignored, and `null` reads as absent
/// for the optional ones.
impl TryFrom<serde_json::Value> for CanonicalResponse {
    type Error = BadMember;

    fn try_from(value: serde_json::Value) -> Result<Self, BadMember> {
        use serde_json::Value;

        let Value::Object(mut members) = value else {
            return Err(BadMember("response"));
        };
        let mut text = |key: &'static str| match members.remove(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s)),
            Some(_) => Err(BadMember(key)),
        };
        let (reason, context, user_message) = (text("reason")?, text("context")?, text("user_message")?);
        let decision = match members.remove("decision") {
            None => Decision::None,
            Some(Value::String(s)) => match s.as_str() {
                "allow" => Decision::Allow,
                "deny" => Decision::Deny,
                "ask" => Decision::Ask,
                "none" => Decision::None,
                _ => return Err(BadMember("decision")),
            },
            Some(_) => return Err(BadMember("decision")),
        };
        Ok(Self {
            decision,
            reason,
            context,
            user_message,
            stop: match members.remove("stop") {
                None => false,
                Some(Value::Bool(b)) => b,
                Some(_) => return Err(BadMember("stop")),
            },
            updated_input: members.remove("updated_input").filter(|v| !v.is_null()),
        })
    }
}

impl CanonicalResponse {
    /// The no-opinion response: what a timeout, an unparsable answer, a
    /// withheld verdict and an empty matched set all degrade to.
    ///
    /// Named rather than open-coded because "no opinion" is reached from many
    /// failure paths and each one must reach the *same* value — a `deny`
    /// assembled by accident on a failure path is the outcome I3 forbids.
    pub fn no_opinion() -> Self {
        // Spelled as the derived default so the two cannot drift: `#[serde(default)]`
        // on this struct makes `Default` the shape a `{}` answer deserializes to,
        // and that shape must be exactly "no opinion".
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(json: &str) -> Result<CanonicalResponse, serde_json::Error> {
        serde_json::from_str(json)
    }

    #[test]
    fn a_duplicate_key_is_an_unreadable_response_not_the_last_value() {
        for json in [
            r#"{"decision":"allow","decision":"deny"}"#,
            r#"{"decision":"deny","decision":"allow"}"#,
            r#"{"decision":"allow","updated_input":{"a":1,"a":2}}"#,
        ] {
            assert!(read(json).is_err(), "{json}");
        }
    }

    #[test]
    fn distinct_keys_and_unknown_members_still_read() {
        let response = read(r#"{"decision":"deny","reason":"no","added_later":{"x":[1,2.5,null]}}"#).unwrap();
        assert_eq!(response.decision, Decision::Deny);
        assert_eq!(response.reason.as_deref(), Some("no"));
    }
}
