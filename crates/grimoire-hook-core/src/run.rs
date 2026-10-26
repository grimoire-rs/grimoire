// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The dispatcher runtime: one call from table bytes and the client's stdin to the
//! bytes for stdout.
//!
//! Its whole contract fits in three sentences:
//!
//! 1. **Only an unusable table is an error.** [`dispatch`] returns
//!    `Err(`[`TableError`]`)` for a table the strict selector refuses (C-N10), which
//!    the launcher maps to exit 65. Every other problem — an unknown root, a
//!    payload that is not an object while no gatekeeper is selected, an
//!    unprojectable response — degrades to empty stdout plus a diagnostic
//!    (decision G, invariant **I3**). A gatekeeper that could not run for want of
//!    a usable payload is a [`PayloadFault`], never a pass. Some clients fail
//!    **closed** on a non-zero hook exit, so the *caller* decides whether a
//!    [`WithheldCause`] earns one.
//! 2. **The table bytes are its only input** (C-007), plus the stdin it reads
//!    only if a row matches. It resolves no scope, reads no configuration and
//!    never asks the environment where the data root is.
//! 3. **It hashes nothing** (C-009). Integrity is pinned at resolution;
//!    `DispatchEntry::resolved_digest` is provenance carried into the audit
//!    record, never a gate. The owner's decision A3 deleted the exec-time
//!    re-check, and re-adding one here would reintroduce a hash on the hot path
//!    of every tool call to defend against **N2**, an explicit non-goal.

use std::io::Read;
use std::path::Path;

use crate::argv;
use crate::audit::AuditLog;
use crate::diag::Diagnostic;
use crate::envelope;
use crate::event::{CanonicalEvent, HookTier};
use crate::glob;
use crate::pipeline::{self, Invocation, WithheldCause};
use crate::projector;
use crate::response::CanonicalResponse;
use crate::table::{DispatchEntry, TableError, select};

/// The audit trail's file name, **beside the dispatch table**.
pub const AUDIT_FILE: &str = "hook_audit.jsonl";

/// Maximum bytes read from the invoking client's stdin.
///
/// A `PostToolUse` payload can legitimately embed a whole file, so the ceiling
/// is generous — but it is a ceiling: the payload is untrusted input on the hot
/// path of every tool call, and an unbounded `read_to_end` is CWE-400. Over the
/// cap is one warning with nothing spawned, like every other refusal here.
pub const MAX_PAYLOAD_BYTES: u64 = 8 * 1024 * 1024;

/// Everything one dispatch produced.
#[derive(Debug, Clone, PartialEq)]
pub struct DispatchOutcome {
    /// The projected verdict document plus a trailing newline; empty when no hook
    /// expressed an opinion.
    pub stdout: Vec<u8>,
    /// Set only per PD-6: a selected gatekeeper was withheld and no gatekeeper
    /// that ran composed a `deny`. The caller maps it to its error channel.
    pub withheld: Option<WithheldCause>,
    /// Set when the client's stdin could not be used: nothing ran. The launcher
    /// maps it to its error exit; grim's adapter ignores it (exit 0).
    pub payload_fault: Option<PayloadFault>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Why the client's payload could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadFault {
    /// Over [`MAX_PAYLOAD_BYTES`] (launcher exit 65). Nothing ran.
    TooLarge,
    /// Not a JSON object while a gatekeeper is selected (launcher exit 65).
    /// Nothing ran.
    NotObject,
    /// A read error on stdin (launcher exit 74).
    Unreadable,
}

impl DispatchOutcome {
    fn nothing(diagnostics: Vec<Diagnostic>) -> Self {
        Self {
            stdout: Vec::new(),
            withheld: None,
            payload_fault: None,
            diagnostics,
        }
    }
}

/// Dispatch the armed hooks for `inv` over the table in `table_bytes`.
///
/// `audit_path` is the audit trail, the dispatch table's sibling; its parent
/// directory also hosts the `payload = "file"` envelopes. It is a separate
/// parameter so [`argv::Invocation`] stays the three operands of the frozen argv
/// grammar.
///
/// The order Decision O fixes: select rows by `(root token, client, event)` through
/// [`select`]; read `stdin` **only if** a row is armed; evaluate grim's own matcher
/// against the tool name; build the C-002 envelope; run every `mutator` serially in
/// declaration order, threading each output into the next; submit the one final
/// input to **every** `gatekeeper`; aggregate with `deny` absorbing and `ask`
/// outranking `allow`; project the canonical response onto the client's shape;
/// write the audit records.
///
/// # Errors
///
/// [`TableError`] when [`select`] refuses the table or a selected row (exit 65 at
/// the launcher). Nothing has been read from `stdin` or spawned by then.
pub fn dispatch(
    table_bytes: &[u8],
    inv: &argv::Invocation,
    audit_path: &Path,
    stdin: &mut dyn Read,
) -> Result<DispatchOutcome, TableError> {
    // An unknown root is a no-match, exactly like an absent one: the forged-
    // registration case (B3) and the ordinary stale registration are
    // indistinguishable on purpose.
    let selection = select(table_bytes, inv)?;
    let mut diagnostics = selection.diagnostics;
    let armed: Vec<&DispatchEntry> = selection.rows.iter().collect();
    let Some(first) = armed.first() else {
        diagnostics.push(Diagnostic::debug(format!(
            "no hooks armed at {} for the requested root; nothing ran",
            inv.event
        )));
        return Ok(DispatchOutcome::nothing(diagnostics));
    };
    // Every selected row carries the one canonical event the invocation named.
    let event = first.event;

    let raw = match read_payload(stdin, &mut diagnostics) {
        Ok(raw) => raw,
        // The cap or an unreadable stdin. Nothing spawned — the payload is the
        // hook's whole input, and a hook cannot judge what grim could not read.
        Err(fault) => {
            return Ok(DispatchOutcome {
                payload_fault: Some(fault),
                ..DispatchOutcome::nothing(diagnostics)
            });
        }
    };
    let fired = Fired { inv, event, audit_path };
    Ok(run_matched(&fired, &selection.scope, &armed, &raw, diagnostics))
}

/// The invocation plus the canonical event its operands resolved to.
struct Fired<'a> {
    inv: &'a argv::Invocation,
    event: CanonicalEvent,
    audit_path: &'a Path,
}

/// Read the invoking client's payload from `stdin`, capped.
///
/// `Err` is "there is no payload grim can work with": one diagnostic, nothing
/// spawned.
fn read_payload(stdin: &mut dyn Read, diagnostics: &mut Vec<Diagnostic>) -> Result<Vec<u8>, PayloadFault> {
    let mut raw = Vec::new();
    // `take(cap + 1)` so an over-cap payload is *detectable* — reading exactly
    // `cap` bytes cannot tell a payload that fits from one that was truncated.
    if let Err(e) = stdin.take(MAX_PAYLOAD_BYTES + 1).read_to_end(&mut raw) {
        diagnostics.push(Diagnostic::warn(format!(
            "the client's hook payload could not be read ({e}); no hook ran"
        )));
        return Err(PayloadFault::Unreadable);
    }
    if raw.len() as u64 > MAX_PAYLOAD_BYTES {
        diagnostics.push(Diagnostic::warn(format!(
            "the client's hook payload is larger than {MAX_PAYLOAD_BYTES} bytes; no hook ran"
        )));
        return Err(PayloadFault::TooLarge);
    }
    Ok(raw)
}

/// Run the matched set and project one response onto the client's shape.
fn run_matched(
    fired: &Fired<'_>,
    scope: &str,
    armed: &[&DispatchEntry],
    raw: &[u8],
    mut diagnostics: Vec<Diagnostic>,
) -> DispatchOutcome {
    let Some(payload) = envelope::read_client_payload(raw) else {
        diagnostics.push(Diagnostic::warn(
            "the client's hook payload is not a JSON object; no hook ran",
        ));
        // A selected gatekeeper that never ran must not read as a pass: exit 0 here
        // would tell a fail-closed client its guard approved.
        let fault = armed
            .iter()
            .any(|entry| entry.tier == HookTier::Gatekeeper)
            .then_some(PayloadFault::NotObject);
        return DispatchOutcome {
            payload_fault: fault,
            ..DispatchOutcome::nothing(diagnostics)
        };
    };

    let audit = AuditLog::at(fired.audit_path.to_path_buf());
    let correlation_id = correlation_id();
    let invocation = Invocation {
        client: &fired.inv.client,
        scope,
        event: fired.event,
        native_event: &fired.inv.event,
        // The cwd the **client** reported, never the process cwd — which for a
        // client-spawned run is the workspace (B1).
        cwd: payload.cwd.as_deref().unwrap_or_default(),
        session_id: payload.session_id.as_deref(),
        correlation_id: &correlation_id,
        audit: &audit,
    };

    // Grim owns matching, and this is the authoritative pass — the registered
    // vendor matcher is only the coarse filter that got grim invoked (C-006).
    let tool = payload.tool.map(|tool| tool.name);
    if tool.is_none() && armed.iter().any(|entry| entry.matcher.is_some()) {
        // **Loud on purpose.** A matcher that can never fire because grim could
        // not read the tool out of the payload is an S-013-shaped silent
        // guardrail: `grim status` would still report the hook armed. The keys
        // grim reads are named so the diagnosis is one line long.
        diagnostics.push(Diagnostic::warn(format!(
            "no tool could be read from the client's payload (grim reads `tool_name` / \
             `tool_input`), so every matcher at {} declined; if this client spells them \
             differently, its matchers can never fire",
            fired.event
        )));
    }
    let mut matched: Vec<&DispatchEntry> = Vec::with_capacity(armed.len());
    let mut declined: Vec<&DispatchEntry> = Vec::new();
    for entry in armed {
        if matcher_admits(entry.matcher.as_deref(), tool) {
            matched.push(entry);
        } else {
            diagnostics.push(Diagnostic::debug(format!(
                "{}/{}: its matcher did not match {tool:?}",
                entry.artifact, entry.id
            )));
            declined.push(entry);
        }
    }
    // **One write for every decline of this invocation** (F-2). The set is
    // complete once the loop is, so collecting first saves an open, a
    // `create_dir_all` and a rotation `statx` per armed hook — +14.1 ms *each* on
    // a 9P `$GRIM_HOME`. Recorded before the early return below, because a run
    // where every hook declined is exactly the run those records answer for.
    pipeline::record_no_matches(&invocation, &declined, &mut diagnostics);
    if matched.is_empty() {
        diagnostics.push(Diagnostic::debug(format!(
            "no armed hook matched {tool:?} at {}; nothing was spawned",
            fired.event
        )));
        return DispatchOutcome::nothing(diagnostics);
    }

    let plan = pipeline::order(&matched);
    let composition = pipeline::compose(&plan, &invocation, raw, &mut diagnostics);
    let withheld = composition.withheld;

    // Empty stdout is the fail-safe shape on all three v1 clients, so a
    // no-opinion answer says nothing at all rather than emitting a document whose
    // only member is the event echo.
    if composition.response == CanonicalResponse::no_opinion() {
        diagnostics.push(Diagnostic::debug(
            "no hook expressed an opinion; nothing was written to stdout",
        ));
        return DispatchOutcome {
            stdout: Vec::new(),
            withheld,
            payload_fault: None,
            diagnostics,
        };
    }
    let stdout = match projector::project(
        &fired.inv.client,
        fired.event,
        &fired.inv.event,
        &composition.response,
        &mut diagnostics,
    ) {
        // `to_vec`, not `Display`: the same compact bytes through the one serializer
        // instantiation the rest of the launcher already links.
        Ok(document) => {
            let mut bytes = serde_json::to_vec(&document).unwrap_or_default();
            bytes.push(b'\n');
            bytes
        }
        // A refusal here is no verdict: a client that fails closed must not be
        // denied a tool call because grim could not express an answer.
        Err(e) => {
            diagnostics.push(Diagnostic::warn(format!(
                "the hook response could not be projected onto {} ({e}); no verdict",
                fired.inv.client
            )));
            Vec::new()
        }
    };
    DispatchOutcome {
        stdout,
        withheld,
        payload_fault: None,
        diagnostics,
    }
}

/// A join key for this invocation's records and log lines.
///
/// Not a secret and not a capability, so unguessability buys nothing — and
/// **not a digest**, because the runtime hashes nothing (C-009). The process id
/// plus the current instant distinguishes the concurrent invocations one client
/// can have in flight, which is all a join key has to do.
fn correlation_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("{:x}-{nanos:x}", std::process::id())
}

/// Whether grim's own matcher dialect admits `tool`.
///
/// Grim owns matching, not the vendor (C-006): the registered vendor matcher is
/// the coarse filter that gets grim invoked at all, and this is the authoritative
/// pass. `None` matches every tool. The dialect is an exact name, an `A|B`
/// alternation of those, or a glob — **never a regex**, which is both a latency
/// decision on the hot path and the reason `MATCHER_ALLOWED` is an allowlist.
fn matcher_admits(matcher: Option<&str>, tool: Option<&str>) -> bool {
    let Some(matcher) = matcher else {
        return true;
    };
    let Some(tool) = tool else {
        // A payload naming no tool gives grim's matcher nothing to match, and a
        // matcher that matches nothing is the safe direction: it withholds the
        // hook rather than firing it on an unknown tool.
        return false;
    };
    glob::matches_tool(matcher, tool)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "0123456789abcdef0123456789abcdef";

    fn selected_table() -> Vec<u8> {
        table_with_tier("observer")
    }

    fn table_with_tier(tier: &str) -> Vec<u8> {
        let row = serde_json::json!({
            "artifact": "guard", "id": "g", "client": "claude", "event": "PreToolUse",
            "tier": tier, "matcher": null, "handler": { "argv": ["true"] },
            "timeout": null, "payload": "stdin", "payload_dir": std::env::temp_dir(),
            "resolved_digest": null,
        });
        serde_json::to_vec(&serde_json::json!({
            "schema": 1, "roots": { ROOT: { "root": "/ws", "hooks": [row] } }
        }))
        .unwrap()
    }

    fn fire(stdin: &mut dyn Read) -> DispatchOutcome {
        let inv = argv::parse(&["claude", "PreToolUse", ROOT].map(std::ffi::OsString::from)).unwrap();
        dispatch(&selected_table(), &inv, Path::new("/nonexistent/audit"), stdin).unwrap()
    }

    fn fire_tier(tier: &str, stdin: &[u8]) -> DispatchOutcome {
        let inv = argv::parse(&["claude", "PreToolUse", ROOT].map(std::ffi::OsString::from)).unwrap();
        let mut stdin = stdin;
        dispatch(
            &table_with_tier(tier),
            &inv,
            Path::new("/nonexistent/audit"),
            &mut stdin,
        )
        .unwrap()
    }

    #[test]
    fn a_non_object_payload_with_a_gatekeeper_selected_is_a_payload_fault() {
        for payload in [&b"[1]"[..], b"null", b"\"x\"", b"not json", b""] {
            let outcome = fire_tier("gatekeeper", payload);
            assert_eq!(outcome.payload_fault, Some(PayloadFault::NotObject), "{payload:?}");
            assert!(outcome.stdout.is_empty() && outcome.withheld.is_none());
        }
    }

    #[test]
    fn a_non_object_payload_without_a_gatekeeper_keeps_the_no_hook_ran_behaviour() {
        for tier in ["observer", "mutator"] {
            let outcome = fire_tier(tier, b"[1]");
            assert_eq!(outcome.payload_fault, None, "{tier}");
            assert!(outcome.stdout.is_empty() && outcome.withheld.is_none(), "{tier}");
        }
    }

    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("boom"))
        }
    }

    #[test]
    fn an_oversize_payload_is_a_payload_fault_and_runs_nothing() {
        let big = vec![b'x'; MAX_PAYLOAD_BYTES as usize + 1];
        let outcome = fire(&mut big.as_slice());
        assert_eq!(outcome.payload_fault, Some(PayloadFault::TooLarge));
        assert!(outcome.stdout.is_empty() && outcome.withheld.is_none());
    }

    #[test]
    fn an_unreadable_payload_is_a_payload_fault_and_runs_nothing() {
        let outcome = fire(&mut Broken);
        assert_eq!(outcome.payload_fault, Some(PayloadFault::Unreadable));
        assert!(outcome.stdout.is_empty() && outcome.withheld.is_none());
    }

    /// **Grim owns matching, and the dialect is exact-name, `A|B` alternation,
    /// or glob — never a regex.**
    ///
    /// The negative half is the load-bearing one, and it is why
    /// `MATCHER_ALLOWED` is an allowlist rather than a denylist: a regex engine
    /// on the hot path of every tool call is both a latency decision and a
    /// catastrophic-backtracking surface, and a dialect that *accidentally*
    /// honours `.` or `^…$` is over-broad while still reporting as installed.
    #[test]
    fn the_matcher_dialect_is_an_exact_name_or_a_glob_never_a_regex() {
        // (matcher, tool, matches, why)
        let cases: &[(Option<&str>, Option<&str>, bool, &str)] = &[
            (None, Some("Bash"), true, "no matcher matches every tool"),
            (
                None,
                None,
                true,
                "no matcher, no tool — an event that carries none still fires",
            ),
            (Some("Bash"), Some("Bash"), true, "an exact name"),
            (Some("Bash"), Some("Read"), false, "a different tool"),
            (Some("Bash"), Some("bash"), false, "tool names are case-sensitive"),
            (Some("Ba*"), Some("Bash"), true, "a trailing glob"),
            (Some("*"), Some("Bash"), true, "the match-all glob"),
            (Some("B?sh"), Some("Bash"), true, "a single-character glob"),
            (
                Some("Ba.h"),
                Some("Bash"),
                false,
                "`.` is a literal in a glob; honouring it as a regex wildcard is over-broad",
            ),
            (
                Some("^Bash$"),
                Some("Bash"),
                false,
                "regex anchors are literal characters, so this names no tool grim knows",
            ),
            // Alternation, both sides and a miss. This case asserted `false`
            // until round 1 of review: `|` is in `MATCHER_ALLOWED` and
            // `classify_matcher` calls `A|B` losslessly translatable, so the
            // client's own matcher already fires on either name — grim comparing
            // the whole string armed the hook everywhere and fired it nowhere.
            (Some("Bash|Read"), Some("Bash"), true, "the first alternative"),
            (Some("Bash|Read"), Some("Read"), true, "the second alternative"),
            (Some("Bash|Read"), Some("Write"), false, "a tool in neither alternative"),
            (
                Some("Bash|Read"),
                Some("Bash|Read"),
                false,
                "the matcher is the alternation, not a tool literally named that",
            ),
            (
                Some("Bash|"),
                Some("Read"),
                false,
                "an empty alternative matches NOTHING here, though an empty regex \
                 alternative matches everything in the vendor dialect",
            ),
            // NOT reachable from a dispatch row, and recorded as such so a later
            // reader does not take it as evidence that glob matchers arm:
            // `classify_matcher` returns `NotTranslatable` for anything but
            // whole-string `*` or an alternation of exact names, and
            // `hook_registration` turns that into `MatcherNotLossless`, so only
            // `All` and `ExactOrAlternation` ever reach the table. This row pins
            // the dialect's internal consistency — each alternative is matched in
            // the full dialect — not a shipped capability.
            (
                Some("Ba*|Read"),
                Some("Bash"),
                true,
                "an alternative may itself be a glob; each is matched in the full dialect",
            ),
            (
                Some("Bash"),
                None,
                false,
                "a payload naming no tool gives grim's matcher nothing to match",
            ),
        ];
        for (matcher, tool, expected, why) in cases {
            assert_eq!(
                matcher_admits(*matcher, *tool),
                *expected,
                "matcher {matcher:?} against tool {tool:?}: {why}"
            );
        }
    }
}
