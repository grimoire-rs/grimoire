//! Contract tests for C-N10, written from the read rule and not from `select`'s body.

use std::ffi::OsString;
use std::path::PathBuf;

use serde_json::{Value, json};

use super::*;
use crate::argv::{self, Invocation};

const ROOT: &str = "0123456789abcdef0123456789abcdef";
const OTHER_ROOT: &str = "fedcba9876543210fedcba9876543210";

fn inv(client: &str, event: &str, root: &str) -> Invocation {
    argv::parse(&[client, event, root].map(OsString::from)).unwrap()
}

fn entry(client: &str, event: CanonicalEvent, id: &str) -> DispatchEntry {
    DispatchEntry {
        artifact: "guard".to_owned(),
        id: id.to_owned(),
        client: client.to_owned(),
        event,
        tier: HookTier::Observer,
        matcher: None,
        handler: HookHandler::Command("true".to_owned()),
        timeout: None,
        payload: HookPayloadMode::Stdin,
        payload_dir: PathBuf::from(if cfg!(windows) { r"C:\payload" } else { "/payload" }),
        resolved_digest: None,
        policy: None,
    }
}

fn row(client: &str, event: CanonicalEvent, id: &str) -> Value {
    serde_json::to_value(entry(client, event, id)).unwrap()
}

fn table(roots: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({ "schema": 1, "roots": roots })).unwrap()
}

fn one_root(hooks: Vec<Value>) -> Vec<u8> {
    table(json!({ ROOT: { "root": "/ws", "hooks": hooks } }))
}

fn select_ids(bytes: &[u8], inv: &Invocation) -> Vec<String> {
    select(bytes, inv).unwrap().rows.into_iter().map(|r| r.id).collect()
}

#[test]
fn selects_the_rows_for_client_event_and_root() {
    let bytes = one_root(vec![
        row("claude", CanonicalEvent::PreToolUse, "a"),
        row("codex", CanonicalEvent::PreToolUse, "other-client"),
        row("claude", CanonicalEvent::Stop, "other-event"),
        row("claude", CanonicalEvent::PreToolUse, "b"),
    ]);
    let selection = select(&bytes, &inv("claude", "PreToolUse", ROOT)).unwrap();
    assert!(selection.root_known);
    assert_eq!(
        selection.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["a", "b"],
        "stored order is kept"
    );
}

#[test]
fn an_unknown_root_is_an_empty_selection_not_an_error() {
    let bytes = one_root(vec![row("claude", CanonicalEvent::Stop, "a")]);
    let selection = select(&bytes, &inv("claude", "Stop", OTHER_ROOT)).unwrap();
    assert!(!selection.root_known);
    assert!(selection.rows.is_empty());
}

#[test]
fn an_event_with_no_canonical_event_selects_nothing() {
    let bytes = one_root(vec![row("claude", CanonicalEvent::Stop, "a")]);
    let selection = select(&bytes, &inv("claude", "PermissionRequest", ROOT)).unwrap();
    assert!(selection.rows.is_empty());
}

#[test]
fn only_the_invocations_root_is_selected_from() {
    let bytes = table(json!({
        ROOT: { "root": "/a", "hooks": [row("claude", CanonicalEvent::Stop, "mine")] },
        OTHER_ROOT: { "root": "/b", "hooks": [row("claude", CanonicalEvent::Stop, "theirs")] },
    }));
    assert_eq!(select_ids(&bytes, &inv("claude", "Stop", ROOT)), ["mine"]);
}

#[test]
fn a_known_root_with_no_rows_is_known_and_empty() {
    let selection = select(&one_root(Vec::new()), &inv("claude", "Stop", ROOT)).unwrap();
    assert!(selection.root_known);
    assert!(selection.rows.is_empty());
}

#[test]
fn the_tool_matcher_is_not_applied_here() {
    let mut with_matcher = entry("claude", CanonicalEvent::PreToolUse, "m");
    with_matcher.matcher = Some("Bash".to_owned());
    let bytes = one_root(vec![serde_json::to_value(with_matcher).unwrap()]);
    assert_eq!(select_ids(&bytes, &inv("claude", "PreToolUse", ROOT)), ["m"]);
}

#[test]
fn a_schema_up_to_the_known_one_is_accepted() {
    for schema in [0, DISPATCH_SCHEMA] {
        let bytes = serde_json::to_vec(&json!({ "schema": schema, "roots": {} })).unwrap();
        assert!(select(&bytes, &inv("claude", "Stop", ROOT)).is_ok(), "schema {schema}");
    }
}

#[test]
fn a_newer_schema_is_an_error() {
    let newer = u64::from(DISPATCH_SCHEMA) + 1;
    let bytes = serde_json::to_vec(&json!({ "schema": newer, "roots": {} })).unwrap();
    assert_eq!(
        select(&bytes, &inv("claude", "Stop", ROOT)),
        Err(TableError::Schema(SchemaFault::Newer(newer)))
    );
}

#[test]
fn a_missing_or_non_integer_schema_is_an_error() {
    for body in [
        json!({ "roots": {} }),
        json!({ "schema": "1", "roots": {} }),
        json!({ "schema": 1.5, "roots": {} }),
        json!({ "schema": -1, "roots": {} }),
        json!({ "schema": null, "roots": {} }),
        json!({ "schema": [1], "roots": {} }),
    ] {
        let bytes = serde_json::to_vec(&body).unwrap();
        assert_eq!(
            select(&bytes, &inv("claude", "Stop", ROOT)),
            Err(TableError::Schema(SchemaFault::Missing)),
            "{body}"
        );
    }
}

#[test]
fn roots_must_be_an_object() {
    for body in [
        json!({ "schema": 1 }),
        json!({ "schema": 1, "roots": [] }),
        json!({ "schema": 1, "roots": "x" }),
        json!({ "schema": 1, "roots": null }),
    ] {
        let bytes = serde_json::to_vec(&body).unwrap();
        assert_eq!(
            select(&bytes, &inv("claude", "Stop", ROOT)),
            Err(TableError::RootsNotObject),
            "{body}"
        );
    }
}

#[test]
fn unparsable_bytes_and_non_objects_are_errors() {
    for bytes in [&b""[..], b"}}} not json", b"[]", b"1", b"null", b"\"x\"", &[0xff, 0xfe]] {
        assert_eq!(
            select(bytes, &inv("claude", "Stop", ROOT)),
            Err(TableError::Unparsable),
            "{bytes:?}"
        );
    }
}

#[test]
fn a_table_over_the_cap_is_an_error_before_it_is_parsed() {
    let mut bytes = one_root(Vec::new());
    bytes.resize(usize::try_from(MAX_TABLE_BYTES).unwrap() + 1, b' ');
    assert_eq!(select(&bytes, &inv("claude", "Stop", ROOT)), Err(TableError::TooLarge));
    bytes.truncate(usize::try_from(MAX_TABLE_BYTES).unwrap());
    assert!(
        select(&bytes, &inv("claude", "Stop", ROOT)).is_ok(),
        "exactly the cap is fine"
    );
}

#[test]
fn a_row_failing_the_probe_is_skipped() {
    let bytes = one_root(vec![
        json!({ "artifact": "x" }),
        json!({ "client": 7, "event": "Stop" }),
        json!({ "client": "claude", "event": ["Stop"] }),
        json!("not an object"),
        json!(null),
        row("claude", CanonicalEvent::Stop, "good"),
    ]);
    assert_eq!(select_ids(&bytes, &inv("claude", "Stop", ROOT)), ["good"]);
}

#[test]
fn a_mismatching_row_is_skipped_even_when_it_would_not_parse() {
    let bytes = one_root(vec![
        json!({ "client": "codex", "event": "Stop", "tier": "from-the-future" }),
        json!({ "client": "claude", "event": "SessionEnd", "handler": 7 }),
        row("claude", CanonicalEvent::Stop, "good"),
    ]);
    assert_eq!(select_ids(&bytes, &inv("claude", "Stop", ROOT)), ["good"]);
}

#[test]
fn a_selected_row_that_does_not_parse_is_an_error() {
    let mut future_tier = row("claude", CanonicalEvent::Stop, "x");
    future_tier["tier"] = json!("from-the-future");
    let mut no_handler = row("claude", CanonicalEvent::Stop, "x");
    no_handler.as_object_mut().unwrap().remove("handler");
    for bad in [future_tier, no_handler] {
        let bytes = one_root(vec![row("claude", CanonicalEvent::Stop, "ok"), bad]);
        assert!(
            matches!(select(&bytes, &inv("claude", "Stop", ROOT)), Err(TableError::Row(_))),
            "a gatekeeper must not be silently dropped"
        );
    }
}

#[test]
fn a_selected_row_failing_the_read_time_rechecks_is_an_error() {
    let mut long_matcher = row("claude", CanonicalEvent::Stop, "x");
    long_matcher["matcher"] = json!("a".repeat(MATCHER_MAX_BYTES + 1));
    let mut relative_dir = row("claude", CanonicalEvent::Stop, "x");
    relative_dir["payload_dir"] = json!("relative/dir");
    for bad in [long_matcher, relative_dir] {
        let bytes = one_root(vec![bad]);
        assert!(matches!(
            select(&bytes, &inv("claude", "Stop", ROOT)),
            Err(TableError::Row(_))
        ));
    }
}

#[test]
fn a_bad_row_for_another_client_or_root_does_not_matter() {
    let mut long_matcher = row("codex", CanonicalEvent::Stop, "x");
    long_matcher["matcher"] = json!("a".repeat(MATCHER_MAX_BYTES + 1));
    let bytes = table(json!({
        ROOT: { "root": "/a", "hooks": [long_matcher, row("claude", CanonicalEvent::Stop, "ok")] },
        OTHER_ROOT: { "root": "/b", "hooks": "garbage" },
        "third": 7,
    }));
    assert_eq!(select_ids(&bytes, &inv("claude", "Stop", ROOT)), ["ok"]);
}

#[test]
fn a_root_this_build_cannot_read_is_skipped() {
    for unreadable in [
        json!(7),
        json!("x"),
        json!({ "root": "/a" }),
        json!({ "root": "/a", "hooks": {} }),
    ] {
        let bytes = table(json!({ ROOT: unreadable.clone() }));
        let selection = select(&bytes, &inv("claude", "Stop", ROOT)).unwrap();
        assert!(selection.rows.is_empty(), "{unreadable}");
    }
}

#[test]
fn unknown_keys_are_ignored_at_every_level() {
    let mut future_row = row("claude", CanonicalEvent::Stop, "x");
    future_row["a_field_from_the_future"] = json!({ "nested": true });
    let bytes = serde_json::to_vec(&json!({
        "schema": 1,
        "added_later": [1, 2, 3],
        "roots": { ROOT: { "root": "/ws", "also_new": null, "hooks": [future_row] } },
    }))
    .unwrap();
    assert_eq!(select_ids(&bytes, &inv("claude", "Stop", ROOT)), ["x"]);
}

#[test]
fn table_errors_exit_65() {
    for err in [
        TableError::TooLarge,
        TableError::Unparsable,
        TableError::Schema(SchemaFault::Missing),
        TableError::Schema(SchemaFault::Newer(2)),
        TableError::RootsNotObject,
        TableError::Row("x".to_owned()),
    ] {
        assert_eq!(err.exit().code(), 65, "{err}");
    }
}

/// C-N10 point 2: readers ignore unknown keys, so no reader type may refuse them.
/// A source scan, because a derive attribute leaves no runtime trace.
#[test]
fn no_reader_type_denies_unknown_fields() {
    for file in ["table.rs", "manifest.rs", "event.rs", "argv.rs"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(file);
        let source = std::fs::read_to_string(&path).unwrap();
        let production = source.split("#[cfg(test)]").next().unwrap_or(&source);
        assert!(
            !production.contains("deny_unknown_fields"),
            "{file} marks a reader type deny_unknown_fields, which breaks the additive read rule"
        );
    }
}

// --- one reader contract, strict keys, additive handlers --------------------

/// Every row variant, fed as the same bytes to the launcher's reader and to
/// `parse_table`: a row is armed for one exactly when it is armed for the other.
#[test]
fn status_and_the_launcher_accept_the_same_rows() {
    let base = || row("claude", CanonicalEvent::Stop, "x");
    let mut future_tier = base();
    future_tier["tier"] = json!("from-the-future");
    let mut no_handler = base();
    no_handler.as_object_mut().unwrap().remove("handler");
    let mut long_matcher = base();
    long_matcher["matcher"] = json!("a".repeat(MATCHER_MAX_BYTES + 1));
    let mut relative_dir = base();
    relative_dir["payload_dir"] = json!("relative/dir");
    let mut extra_handler_key = base();
    extra_handler_key["handler"] = json!({ "argv": ["true"], "from_the_future": 1 });
    let mut both_handlers = base();
    both_handlers["handler"] = json!({ "argv": ["true"], "command": "true" });
    let mut wrong_timeout = base();
    wrong_timeout["timeout"] = json!("soon");

    for (name, candidate) in [
        ("good", base()),
        ("future tier", future_tier),
        ("no handler", no_handler),
        ("long matcher", long_matcher),
        ("relative payload_dir", relative_dir),
        ("extra handler key", extra_handler_key),
        ("both handler forms", both_handlers),
        ("wrong timeout type", wrong_timeout),
    ] {
        let bytes = one_root(vec![candidate]);
        let launcher_arms = select(&bytes, &inv("claude", "Stop", ROOT)).is_ok_and(|s| s.rows.len() == 1);
        let (parsed, degrade) = parse_table(&bytes, &mut Vec::new());
        let status_arms = degrade.is_none() && parsed.roots.values().all(|r| r.hooks.len() == 1);
        assert_eq!(launcher_arms, status_arms, "{name}");
    }
}

#[test]
fn an_unknown_handler_key_is_ignored() {
    let mut future = row("claude", CanonicalEvent::Stop, "x");
    future["handler"] = json!({ "argv": ["true", "-q"], "sandbox": { "net": false } });
    let selection = select(&one_root(vec![future]), &inv("claude", "Stop", ROOT)).unwrap();
    assert_eq!(
        selection.rows[0].handler,
        HookHandler::Argv(vec!["true".to_owned(), "-q".to_owned()])
    );
}

#[test]
fn a_handler_naming_both_forms_or_neither_is_unreadable() {
    for handler in [
        json!({ "argv": ["true"], "command": "true" }),
        json!({ "sandbox": true }),
        json!({}),
    ] {
        let mut bad = row("claude", CanonicalEvent::Stop, "x");
        bad["handler"] = handler;
        assert!(matches!(
            select(&one_root(vec![bad]), &inv("claude", "Stop", ROOT)),
            Err(TableError::Row(_))
        ));
    }
}

/// `serde_json::Value` keeps the last duplicate, which would let `"tier"` appear
/// twice and read as whichever came last. Both readers refuse the whole document.
#[test]
fn a_duplicate_key_makes_the_table_unreadable() {
    let good = serde_json::to_string(&row("claude", CanonicalEvent::Stop, "x")).unwrap();
    let dup_in_row = good.replacen(r#""tier":"observer""#, r#""tier":"observer","tier":"gatekeeper""#, 1);
    assert_ne!(dup_in_row, good);
    let tables = [
        format!(r#"{{"schema":1,"roots":{{"{ROOT}":{{"root":"/ws","hooks":[{dup_in_row}]}}}}}}"#),
        format!(r#"{{"schema":1,"schema":1,"roots":{{"{ROOT}":{{"root":"/ws","hooks":[{good}]}}}}}}"#),
        format!(
            r#"{{"schema":1,"roots":{{"{ROOT}":{{"root":"/a","hooks":[]}},"{ROOT}":{{"root":"/b","hooks":[{good}]}}}}}}"#
        ),
    ];
    for table in tables {
        assert_eq!(
            select(table.as_bytes(), &inv("claude", "Stop", ROOT)),
            Err(TableError::Unparsable),
            "{table}"
        );
        assert_eq!(
            parse_table(table.as_bytes(), &mut Vec::new()),
            (DispatchTable::empty(), Some(DispatchDegrade::Unparsable)),
            "{table}"
        );
    }
}

#[test]
fn a_client_with_no_projection_at_the_event_selects_nothing() {
    let bytes = one_root(vec![row("futureclient", CanonicalEvent::PreToolUse, "g")]);
    let selection = select(&bytes, &inv("futureclient", "PreToolUse", ROOT)).unwrap();
    assert!(selection.root_known);
    assert!(selection.rows.is_empty());
}

#[test]
fn read_table_diagnosed_returns_the_skipped_row_reasons() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dispatch.json");
    let mut future_tier = row("claude", CanonicalEvent::Stop, "x");
    future_tier["tier"] = json!("from-the-future");
    std::fs::write(&path, one_root(vec![future_tier])).unwrap();
    let mut diagnostics = Vec::new();
    let (table, degrade) = read_table_diagnosed(&path, &mut diagnostics);
    assert!(degrade.is_none());
    assert!(table.roots.values().all(|r| r.hooks.is_empty()));
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
}
