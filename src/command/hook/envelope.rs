// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The canonical stdin envelope (C-002) and the closed environment allowlist.
//!
//! One JSON object on the payload's stdin. Everything above `raw` is grim's
//! normalization, in Claude's spelling; `raw` is the client's own payload
//! **verbatim**:
//!
//! ```json
//! { "schema": 1,
//!   "event": "PreToolUse", "native_event": "PreToolUse",
//!   "client": "codex", "scope": "project",
//!   "hook": "shell-guard/deny-curl-pipe-sh", "tier": "gatekeeper",
//!   "cwd": "/repo", "session_id": "…", "correlation_id": "…",
//!   "tool": { "name": "Bash", "input": { "command": "curl x | sh" } },
//!   "raw": { "…": "the client's own payload, byte-for-byte" } }
//! ```
//!
//! ## `raw` is bytes, which is why [`build`] returns `Vec<u8>`
//!
//! C-002 says `raw` is byte-for-byte identical to the vendor payload and is
//! **never re-serialized through grim's serde**. That single sentence rules
//! out the obvious implementation: a `#[derive(Serialize)]` struct with a
//! `raw: serde_json::Value` field round-trips the payload through grim's
//! parser and emitter, which silently normalizes key order, number
//! formatting, escape forms and duplicate keys. A hook whose job is to judge
//! what the client actually said would then be judging grim's paraphrase of
//! it.
//!
//! So the envelope is **assembled**, not serialized: grim's own fields are
//! encoded with `serde_json` (they are grim's values, so there is nothing to
//! preserve), and the client's bytes are spliced in as the value of `raw`
//! exactly as they arrived. A `Serialize` impl on the whole envelope is
//! therefore not merely unnecessary — it is the defect, and it must not be
//! added as a convenience later.
//!
//! Reading the payload is a different matter from re-emitting it: `tool.name`
//! and `tool.input` are *parsed out* of the same bytes so grim can run its
//! own matcher and hand a normalized view to the hook. Parsing to read is
//! fine; parsing to rewrite is what C-002 forbids.
//!
//! ## The environment carries flat, non-secret scalars only (I6)
//!
//! [`ENV_ALLOWLIST`] is closed, and every name on it holds a value grim
//! chose. **No variable ever carries tool input.** The reason is not
//! stylistic: `argv` is world-readable through `/proc/<pid>/cmdline`, the
//! environment is readable through `/proc/<pid>/environ`, **inherited by
//! every grandchild**, and captured in crash dumps and CI logs — attacker
//! **T5**, invariant **I6**, and OWASP's own guidance. Post-tool payloads can
//! embed whole diffs and would overflow `ARG_MAX` (and Windows' ~32 KiB
//! per-variable cap) besides.
//!
//! `GRIM_HOOK_PAYLOAD` is the one conditional member: it names a file and is
//! exported **only** when the entry opted into
//! [`HookPayloadMode::File`](crate::oci::hook::HookPayloadMode::File), never
//! by default.

use std::path::Path;

use serde_json::value::RawValue;

use crate::oci::hook::{CanonicalEvent, HookTier};

/// The envelope's `schema` value — the contract version of the object above,
/// deliberately the same number as the manifest's
/// [`HOOK_SCHEMA_VERSION`](crate::oci::hook::HOOK_SCHEMA_VERSION).
///
/// One version for the manifest and the envelope together, because a payload
/// written against a manifest schema is written against the envelope shape
/// that schema implies; two numbers would let an author believe those can
/// move independently.
pub const ENVELOPE_SCHEMA: u32 = crate::oci::hook::HOOK_SCHEMA_VERSION;

/// Every environment variable a hook payload may receive from grim — a closed
/// allowlist, not a starting point (C-002, I6).
///
/// Adding a name here is a threat decision, not a convenience: the
/// environment is readable by any local process at the same privilege,
/// inherited by every grandchild of the payload, and captured in crash dumps
/// and CI logs. Nothing derived from a tool call's input may ever appear —
/// that content goes on stdin, where none of those vectors reach it.
pub const ENV_ALLOWLIST: [&str; 9] = [
    // The envelope contract version, so a payload can branch without parsing.
    "GRIM_HOOK_SCHEMA",
    // The canonical event name.
    "GRIM_HOOK_EVENT",
    // grim's own name for the invoking client.
    "GRIM_HOOK_CLIENT",
    // `<artifact>/<id>` — the same identity the audit trail records.
    "GRIM_HOOK_NAME",
    // The declared tier.
    "GRIM_HOOK_TIER",
    // The tool NAME only. Never the tool input, which is the whole point.
    "GRIM_HOOK_TOOL",
    // The working directory the client reported.
    "GRIM_HOOK_CWD",
    // The artifact's own install directory — Claude's `$CLAUDE_PROJECT_DIR`
    // is the precedent for why a payload needs to find its own siblings.
    "GRIM_HOOK_DIR",
    // Conditional: exported only under `payload = "file"`.
    "GRIM_HOOK_PAYLOAD",
];

/// Grim's half of the envelope — every field above `raw`.
///
/// Borrowed throughout: these are values the runtime already holds, and an
/// owned copy would be a second place for one of them to drift.
#[derive(Debug, Clone, Copy)]
pub struct EnvelopeMeta<'a> {
    /// The canonical event.
    pub event: CanonicalEvent,
    /// The client's own spelling of the same moment, needed because
    /// `hookSpecificOutput.hookEventName` must echo the **firing** event on
    /// claude and codex.
    pub native_event: &'a str,
    /// grim's name for the invoking client.
    pub client: &'a str,
    /// `global` or `project`.
    pub scope: &'a str,
    /// `<artifact>/<id>`.
    pub hook: &'a str,
    /// The declared tier.
    pub tier: HookTier,
    /// The working directory the client reported, taken from its payload.
    pub cwd: &'a str,
    /// The client's session identifier, when it supplies one.
    pub session_id: Option<&'a str>,
    /// Joins this invocation's audit records to each other and to the
    /// `tracing` lines beside them.
    pub correlation_id: &'a str,
    /// The artifact's own materialized payload tree — the value of
    /// `GRIM_HOOK_DIR`, and the working directory the handler runs from.
    ///
    /// **One of the two fields the stub's signature could not express**
    /// (WP-K Specify finding F-A): [`ENV_ALLOWLIST`] names `GRIM_HOOK_DIR`, and
    /// no other field on this struct can produce it. Exporting it empty to
    /// satisfy the closed-set assertion would have been the wrong fix — the
    /// variable exists because a payload has to find its own siblings, which is
    /// what Claude's `$CLAUDE_PROJECT_DIR` is the precedent for.
    pub payload_dir: &'a Path,
    /// The tool this invocation is about, already read out of the client's
    /// payload by [`tool_from_raw`] — or, from the second mutator onward, the
    /// tool name with **grim's own re-encoded** input (see
    /// [`super::pipeline`]'s module doc on threading).
    ///
    /// The other field F-A required: `GRIM_HOOK_TOOL` carries the tool **name**
    /// and nothing on the stubbed struct held it. Carried here rather than
    /// re-parsed inside [`build`] so the envelope's `tool` member and the
    /// exported `GRIM_HOOK_TOOL` cannot disagree — one value, read twice.
    ///
    /// `None` for an event that carries no tool (`SessionStart`, `Stop`) and for
    /// a payload whose tool field is absent or unreadable.
    pub tool: Option<ToolRef<'a>>,
}

/// Why an envelope could not be built.
///
/// Closed internal enum: the binary is the only consumer, so matches stay
/// total — no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeError {
    /// The client's payload is not a JSON object, so there is nothing that
    /// could be spliced in as `raw` without changing its meaning.
    ///
    /// Degrades to no-spawn and exit 0 like every other runtime problem — a
    /// client that sent grim something it did not expect must not have its
    /// tool call denied for it (I3).
    RawNotAnObject,
}

impl EnvelopeError {
    /// The reason phrase, library style (lowercase, no trailing punctuation).
    pub fn reason(self) -> &'static str {
        match self {
            Self::RawNotAnObject => "the client's hook payload is not a JSON object",
        }
    }
}

impl std::fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

/// The tool a payload names, parsed out of the client's own bytes.
///
/// `input` is the raw slice of the payload's tool-input object rather than a
/// parsed value: it is handed to the payload as part of the envelope, so the
/// same byte-preservation rule that governs `raw` governs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolRef<'a> {
    /// The tool name — the string grim's own matcher runs against.
    pub name: &'a str,
    /// The tool-input object's bytes, as the client wrote them.
    pub input: &'a [u8],
}

/// Assemble the C-002 envelope: grim's fields, then `raw` spliced in verbatim.
///
/// `raw` is the exact byte sequence read from the invoking client's stdin. It
/// is validated as a JSON object and then **copied**, never re-encoded — see
/// the module doc on why a `Serialize` impl over the whole envelope would be
/// the defect rather than the shortcut.
///
/// # Errors
///
/// [`EnvelopeError::RawNotAnObject`] when the client's payload is not a JSON
/// object.
pub fn build(meta: &EnvelopeMeta<'_>, raw: &[u8]) -> Result<Vec<u8>, EnvelopeError> {
    // The one validation: the bytes must be a JSON object, or splicing them in
    // as the value of `raw` would change what the client said. A parse and not a
    // first-byte peek — the spliced result has to be one document the payload
    // can parse, and only a parse establishes that. Shallow, through
    // [`RawValue`]: a `PostToolUse` payload can embed a whole file, and grim has
    // no reason to walk it.
    if top_level_members(raw).is_none() {
        return Err(EnvelopeError::RawNotAnObject);
    }

    let mut out = Vec::with_capacity(raw.len() + ENVELOPE_OVERHEAD_HINT);
    out.push(b'{');
    push_number(&mut out, "schema", ENVELOPE_SCHEMA);
    push_string(&mut out, "event", meta.event.as_str());
    push_string(&mut out, "native_event", meta.native_event);
    push_string(&mut out, "client", meta.client);
    push_string(&mut out, "scope", meta.scope);
    push_string(&mut out, "hook", meta.hook);
    push_string(&mut out, "tier", meta.tier.as_str());
    push_string(&mut out, "cwd", meta.cwd);
    // Always present, `null` when the client supplies none: an absent key
    // cannot be told apart from an older grim by a payload that branches on it
    // (the same always-present-null rule `src/api/` follows).
    match meta.session_id {
        Some(id) => push_string(&mut out, "session_id", id),
        None => out.extend_from_slice(b",\"session_id\":null"),
    }
    push_string(&mut out, "correlation_id", meta.correlation_id);
    match meta.tool {
        Some(tool) => {
            out.extend_from_slice(b",\"tool\":{\"name\":");
            push_json_string(&mut out, tool.name);
            out.extend_from_slice(b",\"input\":");
            // The tool-input span, spliced exactly like `raw` and for the same
            // reason: a hook inspecting a command's quoting is inspecting the
            // client's bytes, not grim's re-encoding of them.
            out.extend_from_slice(tool.input);
            out.push(b'}');
        }
        None => out.extend_from_slice(b",\"tool\":null"),
    }
    out.extend_from_slice(b",\"raw\":");
    out.extend_from_slice(raw);
    out.push(b'}');
    Ok(out)
}

/// Bytes to reserve for grim's own half of the envelope, so the common case is
/// one allocation. A hint, never a limit — every `push_*` below grows the
/// buffer as it needs to.
const ENVELOPE_OVERHEAD_HINT: usize = 512;

/// Append `,"key":<number>` to a partially built object.
fn push_number(out: &mut Vec<u8>, key: &str, value: u32) {
    push_key(out, key);
    out.extend_from_slice(value.to_string().as_bytes());
}

/// Append `,"key":"escaped value"` to a partially built object.
fn push_string(out: &mut Vec<u8>, key: &str, value: &str) {
    push_key(out, key);
    push_json_string(out, value);
}

/// Append `,"key":` — the separator is unconditional because `schema` is
/// written before any of these and is never omitted.
fn push_key(out: &mut Vec<u8>, key: &str) {
    if out.len() > 1 {
        out.push(b',');
    }
    push_json_string(out, key);
    out.push(b':');
}

/// Append one JSON string literal, escaped by `serde_json`.
///
/// Grim's own values are escaped rather than copied: they are grim's, so there
/// is nothing to preserve, and a `cwd` the *client* reported can legitimately
/// contain a quote or a backslash. Only `raw` and the tool-input span are
/// spliced verbatim (C-002).
fn push_json_string(out: &mut Vec<u8>, value: &str) {
    // `to_writer` over a `&str` into a `Vec<u8>` has no failure mode —
    // string serialization cannot fail and `Vec` never short-writes — but the
    // signature is fallible, so the impossible branch writes a valid empty
    // string rather than being discarded silently or panicking in a command
    // whose only permitted exit code is 0.
    if serde_json::to_writer(&mut *out, value).is_err() {
        out.extend_from_slice(b"\"\"");
    }
}

/// The tool a client's payload names, if it names one.
///
/// `None` for an event that carries no tool (`SessionStart`, `Stop`) and for a
/// payload whose tool field is absent or not a string — a missing tool name
/// means grim's matcher has nothing to match, which is a no-match rather than
/// an error.
/// **The key spelling is Claude's on all three v1 clients, and that is
/// evidence rather than an assumption** (WP-K Specify finding F-F).
///
/// C-002 defines the envelope grim *emits* and never the payload keys grim
/// *reads*, so this was the one place a silent per-client no-match could hide:
/// a wrong spelling makes the matcher never fire while `grim status` still
/// reports the hook armed (an S-013-shaped silent guardrail). Settled from the
/// vendor reports in `.agents/research/hooks_vendor_reports/`:
///
/// | Client | Keys | Evidence |
/// |---|---|---|
/// | claude | `tool_name`, `tool_input`, `cwd`, `session_id`, `hook_event_name` | `claude.md` — verbatim `PreToolUse` example from the hooks guide |
/// | codex | identical, snake_case throughout | `codex.md` — the generated `*.command.input.schema.json` (draft-07, `additionalProperties: false`); the report calls the casing "an intentional asymmetry that exactly mirrors Claude Code's own hook wire format" |
/// | copilot | identical **because grim registers PascalCase** | `copilot.md` — the payload shape itself switches with the casing of the registered event name: camelCase registration yields `toolName`/`toolArgs`, PascalCase yields the "VS Code-compatible" snake_case shape. WP-B requirement 1 already forces grim onto PascalCase for an unrelated reason (`matcher = "Bash"` never fires under camelCase), so the Claude-shaped payload is the live one |
///
/// The copilot row is the reason [`RESPONSE_PROJECTION`](crate::oci::hook::RESPONSE_PROJECTION)'s
/// own doc says "grim registers **PascalCase** event names on Copilot, because
/// Copilot's stdin payload shape differs by the casing used in config" — the
/// input side and the output side of that decision are the same decision.
///
/// **When the tool cannot be read, the failure is loud** — see
/// [`super::run::matches_tool`]'s call site, which warns rather than quietly
/// declining to match, because a silent no-match is indistinguishable from a
/// guardrail that is not armed.
pub fn tool_from_raw(raw: &[u8]) -> Option<ToolRef<'_>> {
    read_client_payload(raw)?.tool
}

/// Everything grim reads out of one client payload, in one shallow parse.
///
/// Read once per invocation rather than per hook: [`build`] runs for every armed
/// row, and re-deriving the same three facts each time would be the only part of
/// the hot path that scales with the armed set for no reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientPayload<'a> {
    /// The working directory the **client** reported. Never the process CWD —
    /// for a client-spawned `grim hook run` that is the workspace, and reporting
    /// it would be reporting an attacker-chosen value as the client's own (B1).
    pub cwd: Option<String>,
    /// The client's session identifier, when it supplies one.
    pub session_id: Option<String>,
    /// The tool this call is about, with its input as a **verbatim span** of
    /// `raw`.
    pub tool: Option<ToolRef<'a>>,
}

/// Read grim's three facts out of a client payload; `None` when the payload is
/// not a JSON object.
///
/// Each field is recovered **independently**, which is why this reads a map of
/// [`RawValue`] rather than deserializing a struct with borrowed `&str` fields:
/// a `cwd` of `"C:\\repo"` (every Windows path in JSON) cannot be borrowed as a
/// `&str`, and a struct-shaped read would fail *as a whole* there — losing the
/// tool name to an unrelated field's escape, on one platform only.
pub fn read_client_payload(raw: &[u8]) -> Option<ClientPayload<'_>> {
    let members = top_level_members(raw)?;
    // Owned, because these two are re-escaped into grim's own half of the
    // envelope anyway — the byte-preservation rule governs `raw` and the
    // tool-input span, and nothing else (C-002).
    let owned_string = |key: &str| -> Option<String> { serde_json::from_str::<String>(members.get(key)?.get()).ok() };
    let tool = members.get(TOOL_NAME_KEY).and_then(|name| {
        // Borrowed, so the name is a slice of `raw`. A name carrying a JSON
        // escape (no v1 client emits one) reads as "no tool named" rather than
        // as a silently re-encoded name.
        let name: &str = serde_json::from_str(name.get()).ok()?;
        Some(ToolRef {
            name,
            // A named tool with no input is a legal payload shape (an
            // argument-less tool call); the empty object is what a payload
            // reading `.tool.input.command` finds, rather than a missing member.
            input: members
                .get(TOOL_INPUT_KEY)
                .map_or(&b"{}"[..], |input| input.get().as_bytes()),
        })
    });
    Some(ClientPayload {
        cwd: owned_string("cwd"),
        session_id: owned_string("session_id"),
        tool,
    })
}

/// The client payload's top-level members, values left **unparsed**.
///
/// A `BTreeMap` and not `serde_json::Map`: the latter's value type is fixed to
/// `Value`, which would walk every nested document grim has no interest in. A
/// duplicate key resolves last-wins, the same way `serde_json` itself resolves
/// one.
fn top_level_members(raw: &[u8]) -> Option<std::collections::BTreeMap<String, &RawValue>> {
    serde_json::from_slice(raw).ok()
}

/// The payload key naming the tool — Claude's spelling, which is also codex's
/// and (under grim's PascalCase registration) copilot's. See
/// [`tool_from_raw`]'s doc for the per-client evidence.
const TOOL_NAME_KEY: &str = "tool_name";

/// The payload key carrying the tool's input, same evidence as
/// [`TOOL_NAME_KEY`].
const TOOL_INPUT_KEY: &str = "tool_input";

/// The environment grim exports for one invocation: `(name, value)` pairs
/// drawn **only** from [`ENV_ALLOWLIST`].
///
/// Returned as pairs rather than applied to a `Command` so the allowlist
/// property is assertable without spawning anything: a test can compare the
/// returned names against `ENV_ALLOWLIST` and prove no value carries tool
/// input, which is I6 as a checkable fact rather than a comment.
/// **Driven by [`ENV_ALLOWLIST`], not merely checked against it.** The array
/// decides which names exist and in what order; this function only supplies
/// each name's value. A name added to the array with no value arm below is
/// therefore absent-with-a-warning rather than silently unexported, and a value
/// arm with no array entry is unreachable — which is what makes the allowlist
/// the enforcement rather than a comment.
///
/// Two values come from the **client's** payload (`GRIM_HOOK_CWD`,
/// `GRIM_HOOK_TOOL`), so each is checked for the flat-scalar property the
/// allowlist promises: a value carrying a brace, a bracket or a control
/// character is a payload that leaked into `/proc/<pid>/environ`, every
/// grandchild's environment and the next crash dump (T5, I6). Such a value is
/// **dropped with a warning**, never truncated and never re-spelled — a partial
/// `cwd` reads like a different directory.
pub fn environment(meta: &EnvelopeMeta<'_>, payload_file: Option<&Path>) -> Vec<(String, String)> {
    let value_for = |name: &str| -> Option<String> {
        match name {
            "GRIM_HOOK_SCHEMA" => Some(ENVELOPE_SCHEMA.to_string()),
            "GRIM_HOOK_EVENT" => Some(meta.event.as_str().to_owned()),
            "GRIM_HOOK_CLIENT" => Some(meta.client.to_owned()),
            "GRIM_HOOK_NAME" => Some(meta.hook.to_owned()),
            "GRIM_HOOK_TIER" => Some(meta.tier.as_str().to_owned()),
            "GRIM_HOOK_TOOL" => meta.tool.map(|tool| tool.name.to_owned()),
            "GRIM_HOOK_CWD" => Some(meta.cwd.to_owned()),
            "GRIM_HOOK_DIR" => Some(meta.payload_dir.to_string_lossy().into_owned()),
            "GRIM_HOOK_PAYLOAD" => payload_file.map(|path| path.to_string_lossy().into_owned()),
            other => {
                tracing::warn!("{other} is on the hook environment allowlist but grim has no value for it");
                None
            }
        }
    };
    ENV_ALLOWLIST
        .iter()
        .filter_map(|name| {
            let value = value_for(name)?;
            if !is_flat_scalar(&value) {
                tracing::warn!(
                    "{name} was not exported: its value is not the flat scalar the hook environment \
                     allowlist promises, and the environment is readable by any process at this \
                     privilege (T5, I6)"
                );
                return None;
            }
            Some(((*name).to_owned(), value))
        })
        .collect()
}

/// Whether `value` is the flat, non-document scalar every allowlist member
/// promises to be.
///
/// The forbidden set is the same one the I6 test asserts over the exported
/// pairs: a JSON brace or bracket means a document leaked into the environment,
/// and a control character means a value that can forge a line in whatever
/// reads it.
fn is_flat_scalar(value: &str) -> bool {
    !value
        .chars()
        .any(|c| matches!(c, '{' | '}' | '[' | ']') || c.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    /// A legal but pathological vendor payload — every construct here changes
    /// under a parse-and-re-emit round trip, which is precisely what C-002
    /// forbids:
    ///
    /// | Construct | What serde would do to it |
    /// |---|---|
    /// | `"zebra"` before `"alpha"` | a `BTreeMap`-backed re-emit sorts them |
    /// | `"dup"` twice | last-wins collapses two members into one |
    /// | `1.0` | re-emitted as `1.0` by serde_json, but `1e3` is not |
    /// | `1e3` | re-emitted as `1000.0` |
    /// | `"\u0041"` | re-emitted as the literal `"A"` |
    /// | `  ` around `:` | whitespace is not reproduced |
    ///
    /// So a single byte-equality assertion over this slice catches every
    /// normalization a `#[derive(Serialize)]` envelope would introduce, which
    /// is why the fixture is deliberately ugly rather than representative.
    const HOSTILE_RAW: &[u8] = br#"{"zebra":1,"alpha":2,"trailing":1.0,"exponent":1e3,"dup":1,"dup":2,"escaped":"\u0041","hook_event_name":"PreToolUse","tool_name" : "Bash","tool_input":{"command":"curl x | sh"},"cwd":"/repo"}"#;

    /// **Grown by the Implement phase, exactly as F-A required.** The two new
    /// fields are `payload_dir` and `tool` — the inputs `GRIM_HOOK_DIR` and
    /// `GRIM_HOOK_TOOL` need and that the stubbed struct could not hold. The
    /// tool is read out of the same fixture the envelope splices, so the
    /// exported `GRIM_HOOK_TOOL` and the envelope's `tool.name` are one value.
    fn meta() -> EnvelopeMeta<'static> {
        EnvelopeMeta {
            event: CanonicalEvent::PreToolUse,
            native_event: "PreToolUse",
            client: "codex",
            scope: "project",
            hook: "shell-guard/deny-curl-pipe-sh",
            tier: HookTier::Gatekeeper,
            cwd: "/repo",
            session_id: Some("s-1"),
            correlation_id: "c0ffee",
            payload_dir: Path::new("/abs/payload"),
            tool: tool_from_raw(HOSTILE_RAW),
        }
    }

    /// **C-002, the whole of it.** `raw` must appear in the envelope as the
    /// exact byte sequence the client sent.
    ///
    /// The assertion is on **bytes**, not on a parsed value: a parsed
    /// comparison is satisfied by grim's paraphrase, and a hook whose job is to
    /// judge what the client said would then be judging the paraphrase. The
    /// `"raw":` prefix check is what makes this a statement about the envelope's
    /// `raw` member rather than about the bytes appearing somewhere.
    #[test]
    fn raw_is_spliced_byte_for_byte_and_never_re_serialized_c002() {
        let envelope = build(&meta(), HOSTILE_RAW).expect("a JSON object is a valid raw");
        let haystack = String::from_utf8(envelope.clone()).expect("the envelope is UTF-8");
        let needle = std::str::from_utf8(HOSTILE_RAW).expect("fixture is UTF-8");
        let at = haystack.find(needle).unwrap_or_else(|| {
            panic!(
                "the client's bytes were re-encoded somewhere on the way into `raw`; \
                 C-002 requires them verbatim.\nenvelope: {haystack}"
            )
        });
        assert!(
            haystack[..at].ends_with("\"raw\":"),
            "the verbatim bytes must be the value of `raw`, not just present somewhere: {haystack}"
        );
        // And the whole thing is still one JSON document — splicing must not
        // produce something the payload cannot parse.
        let parsed: serde_json::Value = serde_json::from_slice(&envelope).expect("the envelope is one JSON object");
        assert_eq!(
            parsed.get("raw").and_then(|raw| raw.get("alpha")),
            Some(&serde_json::json!(2)),
            "the spliced member must still parse as the client's object: {parsed}"
        );
    }

    /// Grim's own half of the envelope, in the shape the module doc documents.
    ///
    /// Every field here is grim's own value, so serializing it through serde is
    /// correct — the byte-preservation rule applies to `raw` alone.
    #[test]
    fn the_envelope_carries_grims_own_fields_beside_raw_c002() {
        let envelope = build(&meta(), HOSTILE_RAW).expect("valid raw");
        let parsed: serde_json::Value = serde_json::from_slice(&envelope).expect("one JSON object");
        assert_eq!(parsed["schema"], serde_json::json!(ENVELOPE_SCHEMA));
        assert_eq!(parsed["event"], serde_json::json!("PreToolUse"));
        assert_eq!(parsed["native_event"], serde_json::json!("PreToolUse"));
        assert_eq!(parsed["client"], serde_json::json!("codex"));
        assert_eq!(parsed["scope"], serde_json::json!("project"));
        assert_eq!(parsed["hook"], serde_json::json!("shell-guard/deny-curl-pipe-sh"));
        assert_eq!(parsed["tier"], serde_json::json!("gatekeeper"));
        assert_eq!(parsed["cwd"], serde_json::json!("/repo"));
        assert_eq!(parsed["session_id"], serde_json::json!("s-1"));
        assert_eq!(parsed["correlation_id"], serde_json::json!("c0ffee"));
        // The normalized tool view the module doc shows, so a payload never has
        // to know the client's own key spelling to read the tool name.
        assert_eq!(parsed["tool"]["name"], serde_json::json!("Bash"));
        assert_eq!(parsed["tool"]["input"]["command"], serde_json::json!("curl x | sh"));
    }

    /// A payload that is not a JSON object cannot become `raw` without changing
    /// its meaning, so it is refused — and the refusal is an
    /// [`EnvelopeError`], never a panic and never a non-zero exit.
    #[test]
    fn build_refuses_a_payload_that_is_not_a_json_object() {
        for raw in [&b"[1,2]"[..], &b"\"a string\""[..], &b"not json at all"[..], &b""[..]] {
            assert_eq!(
                build(&meta(), raw),
                Err(EnvelopeError::RawNotAnObject),
                "a non-object payload must refuse rather than be wrapped: {:?}",
                std::str::from_utf8(raw)
            );
        }
    }

    /// The tool a client's payload names, read out of the client's own bytes.
    ///
    /// `input` is asserted as an **exact byte span**, because it is handed to
    /// the payload and the same rule that governs `raw` governs it — a
    /// re-encoded span would drop the `  ` and the escape forms a hook may be
    /// inspecting.
    #[test]
    fn tool_from_raw_reads_the_name_and_the_verbatim_input_span() {
        let tool = tool_from_raw(HOSTILE_RAW).expect("the fixture names a tool");
        assert_eq!(tool.name, "Bash");
        assert_eq!(
            std::str::from_utf8(tool.input).expect("UTF-8"),
            r#"{"command":"curl x | sh"}"#,
            "the tool-input span must be the client's own bytes"
        );
    }

    /// No tool named ⇒ `None`, which is a no-match rather than an error.
    ///
    /// Three shapes reach this: an event that carries no tool at all
    /// (`SessionStart`, `Stop`), a payload whose tool field is absent, and one
    /// whose tool field is not a string.
    #[test]
    fn tool_from_raw_is_none_when_the_payload_names_no_tool() {
        for raw in [
            &br#"{"hook_event_name":"Stop","cwd":"/repo"}"#[..],
            &br#"{"tool_name":42,"tool_input":{}}"#[..],
            &br#"{}"#[..],
        ] {
            assert_eq!(
                tool_from_raw(raw),
                None,
                "a payload naming no tool has nothing for grim's matcher to match: {:?}",
                std::str::from_utf8(raw)
            );
        }
    }

    /// **C-002 · I6 — the closed allowlist, asserted as an equality.**
    ///
    /// Not "a subset": a subset assertion is satisfied by exporting nothing,
    /// and it is the *upper* bound that carries the security property while the
    /// *lower* bound is what stops a payload losing a variable it documents.
    /// `GRIM_HOOK_PAYLOAD` is the one conditional member and is excluded here —
    /// its own test below covers it.
    ///
    /// **This test does not compile-and-pass against
    /// `environment(&EnvelopeMeta, Option<&Path>)` as stubbed, and that is a
    /// finding rather than a test bug:** `GRIM_HOOK_TOOL` and `GRIM_HOOK_DIR`
    /// are on the allowlist, and neither the tool name nor the payload
    /// directory is reachable from an `EnvelopeMeta`. The signature has to grow
    /// those two inputs; exporting them empty to satisfy the assertion would be
    /// the wrong fix.
    #[test]
    fn the_exported_environment_is_exactly_the_closed_allowlist_c002_i6() {
        let exported = environment(&meta(), None);
        let names: BTreeSet<&str> = exported.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names.len(),
            exported.len(),
            "a name exported twice makes the last writer win invisibly: {exported:?}"
        );
        let expected: BTreeSet<&str> = ENV_ALLOWLIST
            .iter()
            .copied()
            .filter(|name| *name != "GRIM_HOOK_PAYLOAD")
            .collect();
        assert_eq!(
            names, expected,
            "the exported set must be exactly the allowlist minus the conditional member — \
             a name above it is an unreviewed threat decision, a name below it is a variable \
             the format documents and the payload will not find"
        );
        for (name, value) in &exported {
            assert!(!value.is_empty(), "{name} was exported with no value");
        }
    }

    /// The one conditional member, exported only under the `file` transport.
    #[test]
    fn the_payload_file_variable_is_exported_only_for_the_file_transport() {
        let stdin_names: Vec<String> = environment(&meta(), None).into_iter().map(|(name, _)| name).collect();
        assert!(
            !stdin_names.iter().any(|name| name == "GRIM_HOOK_PAYLOAD"),
            "the default transport is stdin; naming a payload file there points at nothing"
        );
        let file = Path::new("/abs/payload/envelope.json");
        let with_file = environment(&meta(), Some(file));
        let payload = with_file
            .iter()
            .find(|(name, _)| name == "GRIM_HOOK_PAYLOAD")
            .expect("`payload = \"file\"` must export the file it wrote");
        assert_eq!(payload.1, file.to_string_lossy());
    }

    /// **I6, as the negative it actually is.** No exported variable carries a
    /// tool call's input.
    ///
    /// Asserted structurally rather than by naming a sentinel: every legitimate
    /// value on the allowlist is a **flat scalar** grim chose, so a value
    /// containing a JSON brace, a bracket, or a newline is a payload that
    /// leaked into `/proc/<pid>/environ`, every grandchild's environment, and
    /// the next crash dump. `GRIM_HOOK_TOOL` carries the tool NAME, which is
    /// why the tool name is not part of the forbidden set here.
    #[test]
    fn no_environment_value_carries_a_json_document_i6() {
        for (name, value) in environment(&meta(), Some(Path::new("/abs/payload/envelope.json"))) {
            for forbidden in ['{', '}', '[', ']', '\n', '\r', '\0'] {
                assert!(
                    !value.contains(forbidden),
                    "{name} carries {forbidden:?}, so it is not the flat scalar the allowlist \
                     promises — the environment is world-readable at this privilege (T5, I6)"
                );
            }
        }
    }
}
