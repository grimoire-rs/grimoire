// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Span-preserving splice edits on TOML config text (Codex's `config.toml`).
//!
//! The TOML analogue of [`super::json_splice`]: Codex's `config.toml` is a
//! user-owned file that may carry arbitrary hand-authored settings and
//! comments outside the managed `mcp_servers.<name>` table, so a
//! parse-and-reserialize rewrite (the plain `toml` crate's only mode) would
//! reorder keys and drop comments. This module edits through [`toml_edit`]
//! instead — a format-preserving parser/editor — so every byte outside the
//! one managed member survives untouched, mirroring the JSON splice
//! contract exactly.
//!
//! [`Vendor::mcp_entry`](super::vendor::Vendor::mcp_entry) stays
//! JSON-typed (the single source of truth shared with every other vendor);
//! the value is converted to a [`toml_edit::Item`] only at splice time.
//!
//! `container`/`member` and the [`Splice`] result type are shared verbatim
//! with [`super::json_splice`] — the pointer shape and "did the text
//! change" question are format-independent.

use std::io;

use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};

use super::json_config::invalid_data;
// `split_pointer` is format-independent (pointer string parsing only), so
// callers reuse the single `json_splice` instance for both formats; this
// module re-exports it anyway for API parity/discoverability alongside the
// TOML-specific splice functions.
#[allow(
    unused_imports,
    reason = "re-exported for API parity; callers reuse json_splice::split_pointer directly (format-independent)"
)]
pub use super::json_splice::{Splice, split_pointer};

/// Ensure `container.member` equals `value` in TOML text, creating the
/// container table (and, for empty input, the document) as needed. All
/// bytes outside the spliced member survive verbatim.
///
/// Values are compared semantically (parsed, not byte-wise): a member
/// whose current value equals `value` up to key order and formatting is
/// [`Splice::Unchanged`].
///
/// # Errors
///
/// `InvalidData` when the text is not valid TOML, the existing `container`
/// value is not a table, or `value` cannot be represented in TOML (e.g. a
/// bare JSON `null` — TOML has no null type).
pub fn upsert_member(text: &str, container: &str, member: &str, value: &serde_json::Value) -> io::Result<Splice> {
    let new_item = json_to_toml_item(value)?;
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| invalid_data(e.to_string()))?;

    let root = doc.as_table_mut();
    let container_item = root.entry(container).or_insert_with(|| Item::Table(Table::new()));
    let Some(container_table) = container_item.as_table_mut() else {
        return Err(invalid_data(format!(
            "'{container}' is not a TOML table; refusing to edit"
        )));
    };
    // Hide an empty container header (only its sub-tables print their own
    // `[container.member]` headers) — a no-op when the table already has
    // direct key/values, or was already implicit from parsing.
    container_table.set_implicit(true);

    if let Some(existing) = container_table.get(member)
        && toml_item_to_json(existing).as_ref() == Some(value)
    {
        return Ok(Splice::Unchanged);
    }

    container_table.insert(member, new_item);
    Ok(Splice::Changed(doc.to_string()))
}

/// Remove `container.member` when present; a container emptied by the
/// removal is removed too. Absent container/member is [`Splice::Unchanged`].
///
/// # Errors
///
/// `InvalidData` when the text is not valid TOML, or the existing
/// `container` value is not a table.
pub fn remove_member(text: &str, container: &str, member: &str) -> io::Result<Splice> {
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| invalid_data(e.to_string()))?;
    let root = doc.as_table_mut();
    let Some(container_item) = root.get_mut(container) else {
        return Ok(Splice::Unchanged);
    };
    let Some(container_table) = container_item.as_table_mut() else {
        return Err(invalid_data(format!(
            "'{container}' is not a TOML table; refusing to edit"
        )));
    };
    if container_table.get(member).is_none() {
        return Ok(Splice::Unchanged);
    }
    container_table.remove(member);
    if container_table.is_empty() {
        root.remove(container);
    }
    Ok(Splice::Changed(doc.to_string()))
}

/// The parsed value of `container.member` in `text`, if present, converted
/// to its `serde_json::Value` equivalent (the canonical representation
/// [`super::vendor::Vendor::mcp_entry`] produces). `None` for unparsable
/// text or an absent member; the subsequent [`upsert_member`] surfaces a
/// parse error.
pub fn member_value(text: &str, container: &str, member: &str) -> Option<serde_json::Value> {
    let doc: DocumentMut = text.parse().ok()?;
    let container_table = doc.as_table().get(container)?.as_table()?;
    toml_item_to_json(container_table.get(member)?)
}

/// Convert an [`Vendor::mcp_entry`](super::vendor::Vendor::mcp_entry)
/// JSON value into a TOML item for splicing. The entry shape (stdio →
/// `command`/`args`/`env`/`cwd`; HTTP → `url`) is always a JSON object at
/// the top level, so this only needs to handle the JSON value kinds that
/// can appear inside one.
///
/// # Errors
///
/// `InvalidData` when `value` carries a JSON `null` — TOML has no null
/// type, so a descriptor field that resolves to `null` cannot round-trip.
fn json_to_toml_item(value: &serde_json::Value) -> io::Result<Item> {
    match value {
        // The entry's top level is always a JSON object (see doc comment
        // above): render it as a real TOML table so it gets its own
        // `[container.member]` header, matching the shape every hand-authored
        // `[mcp_servers.<name>]` entry already uses.
        serde_json::Value::Object(map) => {
            let mut table = Table::new();
            for (key, field) in map {
                table.insert(key, Item::Value(json_to_toml_value(field)?));
            }
            Ok(Item::Table(table))
        }
        other => Ok(Item::Value(json_to_toml_value(other)?)),
    }
}

/// Convert one JSON value (a field inside the entry object, possibly nested)
/// into a TOML value. A nested JSON object becomes a TOML inline table — the
/// descriptor schema nests at most one level deep (e.g. `env`), so an inline
/// table reads naturally as a single field's value rather than its own
/// sub-table.
fn json_to_toml_value(value: &serde_json::Value) -> io::Result<Value> {
    match value {
        serde_json::Value::Null => Err(invalid_data(
            "TOML has no null type; cannot represent a null value".to_string(),
        )),
        serde_json::Value::Bool(b) => Ok((*b).into()),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map(Value::from)
            .or_else(|| n.as_f64().map(Value::from))
            .ok_or_else(|| invalid_data(format!("number '{n}' cannot be represented in TOML"))),
        serde_json::Value::String(s) => Ok(Value::from(s)),
        serde_json::Value::Array(items) => {
            let mut arr = Array::new();
            for item in items {
                arr.push(json_to_toml_value(item)?);
            }
            Ok(Value::Array(arr))
        }
        serde_json::Value::Object(map) => {
            let mut table = InlineTable::new();
            for (key, field) in map {
                table.insert(key.as_str(), json_to_toml_value(field)?);
            }
            Ok(Value::InlineTable(table))
        }
    }
}

/// Convert a scanned TOML item back to its `serde_json::Value` equivalent,
/// for the semantic unchanged-check in [`upsert_member`] and the read path
/// in [`member_value`]. `None` only for an item with no JSON equivalent at
/// all (an array-of-tables). A non-finite float — which JSON's number type
/// cannot hold — degrades to its lexical string (see [`toml_value_to_json`])
/// rather than being dropped, so a hand-authored value never silently
/// vanishes from the semantic snapshot.
fn toml_item_to_json(item: &Item) -> Option<serde_json::Value> {
    match item {
        Item::None | Item::ArrayOfTables(_) => None,
        Item::Value(v) => toml_value_to_json(v),
        Item::Table(t) => Some(table_to_json(t)),
    }
}

fn toml_value_to_json(value: &Value) -> Option<serde_json::Value> {
    match value {
        Value::String(s) => Some(serde_json::Value::String(s.value().clone())),
        Value::Integer(i) => Some(serde_json::Value::Number((*i.value()).into())),
        Value::Float(f) => {
            // JSON's number type has no NaN/±Infinity; `Number::from_f64`
            // returns None for them. A hand-authored non-finite float in grim's
            // managed entry must NOT vanish from the semantic snapshot — that
            // would hide the field from the untracked-clobber gate and the
            // integrity hash. Preserve it as its TOML lexical form (`NaN`/`inf`/
            // `-inf`) so the field still counts (deterministic, so the hash
            // stays stable). Finite floats keep their native JSON number.
            let n = *f.value();
            Some(match serde_json::Number::from_f64(n) {
                Some(num) => serde_json::Value::Number(num),
                None => serde_json::Value::String(n.to_string()),
            })
        }
        Value::Boolean(b) => Some(serde_json::Value::Bool(*b.value())),
        Value::Datetime(d) => Some(serde_json::Value::String(d.value().to_string())),
        Value::Array(arr) => {
            let items: Option<Vec<serde_json::Value>> = arr.iter().map(toml_value_to_json).collect();
            items.map(serde_json::Value::Array)
        }
        Value::InlineTable(t) => Some(inline_table_to_json(t)),
    }
}

fn table_to_json(table: &Table) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (key, item) in table.iter() {
        if let Some(v) = toml_item_to_json(item) {
            map.insert(key.to_string(), v);
        }
    }
    serde_json::Value::Object(map)
}

fn inline_table_to_json(table: &InlineTable) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (key, value) in table.iter() {
        if let Some(v) = toml_value_to_json(value) {
            map.insert(key.to_string(), v);
        }
    }
    serde_json::Value::Object(map)
}

// The contract mirrors `json_splice.rs` exactly (see that module's test
// suite): span-preservation, semantic unchanged-detection, single-key
// replace, and tolerant no-op removal — retargeted at TOML's
// `mcp_servers.<name>` shape instead of JSON's `mcpServers.<name>`.
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn changed(s: Splice) -> String {
        match s {
            Splice::Changed(t) => t,
            Splice::Unchanged => panic!("expected Changed"),
        }
    }

    fn parse(text: &str) -> toml::Value {
        toml::from_str(text).unwrap_or_else(|e| panic!("expected valid TOML, got {e}: {text:?}"))
    }

    /// Safe nested lookup — `toml::Value`'s `Index` impl panics on a missing
    /// key (mirroring `std` map indexing), so every assertion below walks
    /// through `Value::get` instead of `doc["k"]` chains.
    fn get_path<'v>(v: &'v toml::Value, path: &[&str]) -> Option<&'v toml::Value> {
        let mut cur = v;
        for key in path {
            cur = cur.get(key)?;
        }
        Some(cur)
    }

    fn str_at(doc: &toml::Value, path: &[&str]) -> Option<String> {
        get_path(doc, path).and_then(toml::Value::as_str).map(str::to_string)
    }

    #[test]
    fn upsert_into_empty_text_creates_table() {
        let out =
            changed(upsert_member("", "mcp_servers", "grim", &json!({"command": "grim", "args": ["mcp"]})).unwrap());
        let doc = parse(&out);
        assert_eq!(
            str_at(&doc, &["mcp_servers", "grim", "command"]),
            Some("grim".to_string())
        );
        assert_eq!(
            get_path(&doc, &["mcp_servers", "grim", "args"]).and_then(toml::Value::as_array),
            Some(&vec![toml::Value::String("mcp".to_string())])
        );
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn upsert_into_missing_file_matches_upsert_into_empty_text() {
        // The module has no separate "file" concept: `install_mcp` reads a
        // NotFound config as an empty string for both JSON and TOML, so a
        // missing file and an already-empty file must splice identically.
        let missing = changed(upsert_member("", "mcp_servers", "grim", &json!({"command": "grim"})).unwrap());
        let blank = changed(upsert_member("   \n", "mcp_servers", "grim", &json!({"command": "grim"})).unwrap());
        assert_eq!(parse(&missing), parse(&blank));
    }

    #[test]
    fn upsert_preserves_surrounding_user_content_byte_exact() {
        let text = "# user comment\nmodel = \"gpt-5\"\n\n[sandbox]\nmode = \"workspace-write\"\n";
        let out = changed(upsert_member(text, "mcp_servers", "grim", &json!({"command": "grim"})).unwrap());
        assert!(out.contains("# user comment"), "comment preserved: {out}");
        assert!(out.contains("model = \"gpt-5\""), "unrelated key preserved: {out}");
        assert!(out.contains("[sandbox]"), "unrelated table preserved: {out}");
        assert!(out.contains("mode = \"workspace-write\""));
        let doc = parse(&out);
        assert_eq!(
            str_at(&doc, &["mcp_servers", "grim", "command"]),
            Some("grim".to_string())
        );

        // Round trip: removing what was just added restores the original
        // text byte-for-byte — the strongest span-preservation invariant,
        // independent of exactly how the new table is formatted on insert.
        let back = changed(remove_member(&out, "mcp_servers", "grim").unwrap());
        assert_eq!(back, text, "remove undoes upsert exactly");
    }

    #[test]
    fn upsert_inserts_member_into_existing_container_preserves_siblings() {
        let text = "[mcp_servers.other]\ncommand = \"x\"\n\n[theme]\nname = \"dark\"\n";
        let out = changed(upsert_member(text, "mcp_servers", "grim", &json!({"command": "grim"})).unwrap());
        let doc = parse(&out);
        assert_eq!(
            str_at(&doc, &["mcp_servers", "other", "command"]),
            Some("x".to_string()),
            "sibling server untouched"
        );
        assert_eq!(str_at(&doc, &["theme", "name"]), Some("dark".to_string()));
        assert_eq!(
            str_at(&doc, &["mcp_servers", "grim", "command"]),
            Some("grim".to_string())
        );
    }

    #[test]
    fn upsert_replaces_only_the_member_value_single_key() {
        let text = "[mcp_servers.grim]\ncommand = \"old\"\n\n[mcp_servers.other]\ncommand = \"x\"\n";
        let out = changed(upsert_member(text, "mcp_servers", "grim", &json!({"command": "new"})).unwrap());
        let doc = parse(&out);
        assert_eq!(
            str_at(&doc, &["mcp_servers", "grim", "command"]),
            Some("new".to_string())
        );
        assert_eq!(
            str_at(&doc, &["mcp_servers", "other", "command"]),
            Some("x".to_string()),
            "sibling untouched"
        );
        // Exactly one `grim` key in the mcp_servers table — no duplicate
        // left behind by the replace.
        let table = get_path(&doc, &["mcp_servers"])
            .and_then(toml::Value::as_table)
            .unwrap();
        assert_eq!(table.len(), 2, "no duplicate key: {table:?}");
    }

    #[test]
    fn upsert_identical_value_is_unchanged_despite_formatting() {
        let text = "[mcp_servers.grim]\nargs = [\"mcp\"]\ncommand = \"grim\"\n";
        let value = json!({"command": "grim", "args": ["mcp"]});
        assert_eq!(
            upsert_member(text, "mcp_servers", "grim", &value).unwrap(),
            Splice::Unchanged
        );
    }

    #[test]
    fn second_upsert_same_value_is_byte_identical() {
        let value = json!({"command": "grim", "args": ["mcp"]});
        let first = changed(upsert_member("", "mcp_servers", "grim", &value).unwrap());
        assert_eq!(
            upsert_member(&first, "mcp_servers", "grim", &value).unwrap(),
            Splice::Unchanged,
            "a second identical upsert must not rewrite the file (idempotency, plan C2)"
        );
    }

    #[test]
    fn changed_value_replaces_in_place_single_key() {
        let first = changed(upsert_member("", "mcp_servers", "grim", &json!({"command": "old"})).unwrap());
        let second = changed(upsert_member(&first, "mcp_servers", "grim", &json!({"command": "new"})).unwrap());
        let doc = parse(&second);
        assert_eq!(
            str_at(&doc, &["mcp_servers", "grim", "command"]),
            Some("new".to_string())
        );
        let table = get_path(&doc, &["mcp_servers"])
            .and_then(toml::Value::as_table)
            .unwrap();
        assert_eq!(table.len(), 1, "no duplicate key left behind: {table:?}");
    }

    #[test]
    fn remove_member_removes_only_managed_table_preserves_foreign_keys() {
        let text = "# top comment\ntheme = \"dark\"\n\n[mcp_servers.grim]\ncommand = \"grim\"\n\n[mcp_servers.other]\ncommand = \"x\"\n";
        let out = changed(remove_member(text, "mcp_servers", "grim").unwrap());
        assert!(out.contains("# top comment"), "comment survives: {out}");
        let doc = parse(&out);
        assert!(
            get_path(&doc, &["mcp_servers", "grim"]).is_none(),
            "managed member gone: {out}"
        );
        assert_eq!(
            str_at(&doc, &["mcp_servers", "other", "command"]),
            Some("x".to_string())
        );
        assert_eq!(str_at(&doc, &["theme"]), Some("dark".to_string()));
    }

    #[test]
    fn remove_last_member_drops_the_empty_container_table() {
        let text = "theme = \"dark\"\n\n[mcp_servers.grim]\ncommand = \"grim\"\n";
        let out = changed(remove_member(text, "mcp_servers", "grim").unwrap());
        let doc = parse(&out);
        assert!(
            get_path(&doc, &["mcp_servers"]).is_none(),
            "emptied container removed: {out}"
        );
        assert_eq!(str_at(&doc, &["theme"]), Some("dark".to_string()));
    }

    #[test]
    fn remove_absent_is_tolerant_no_op() {
        assert_eq!(remove_member("", "mcp_servers", "grim").unwrap(), Splice::Unchanged);
        assert_eq!(
            remove_member("[mcp_servers.other]\ncommand = \"x\"\n", "mcp_servers", "grim").unwrap(),
            Splice::Unchanged
        );
        assert_eq!(
            remove_member("theme = \"dark\"\n", "mcp_servers", "grim").unwrap(),
            Splice::Unchanged
        );
    }

    #[test]
    fn malformed_toml_is_refused() {
        for bad in ["not = toml = {{{", "[unterminated"] {
            let err = upsert_member(bad, "mcp_servers", "grim", &json!({})).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "input: {bad}");
        }
        // Container present but not a table.
        let err = upsert_member("mcp_servers = \"nope\"\n", "mcp_servers", "grim", &json!({})).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let err = remove_member("mcp_servers = 3\n", "mcp_servers", "grim").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn null_json_value_is_rejected_toml_has_no_null_type() {
        let err = upsert_member("", "mcp_servers", "grim", &json!({"command": null})).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn member_value_reads_existing_entry_as_json() {
        let text = "[mcp_servers.grim]\ncommand = \"grim\"\nargs = [\"mcp\"]\n";
        let value = member_value(text, "mcp_servers", "grim").expect("member present");
        assert_eq!(value["command"], "grim");
        assert_eq!(value["args"][0], "mcp");
    }

    #[test]
    fn member_value_is_none_for_absent_member_or_unparsable_text() {
        assert!(member_value("", "mcp_servers", "grim").is_none());
        assert!(member_value("not toml {{{", "mcp_servers", "grim").is_none());
        assert!(member_value("[mcp_servers.other]\ncommand = \"x\"\n", "mcp_servers", "grim").is_none());
    }

    #[test]
    fn nested_object_and_array_values_convert_faithfully() {
        let value = json!({
            "command": "grim",
            "args": ["mcp", "--flag"],
            "env": {"A": "1", "B": "${VAR}"},
        });
        let out = changed(upsert_member("", "mcp_servers", "grim", &value).unwrap());
        let doc = parse(&out);
        assert_eq!(
            get_path(&doc, &["mcp_servers", "grim", "args"])
                .and_then(toml::Value::as_array)
                .and_then(|a| a.get(1))
                .and_then(toml::Value::as_str),
            Some("--flag")
        );
        assert_eq!(
            str_at(&doc, &["mcp_servers", "grim", "env", "A"]),
            Some("1".to_string())
        );
        assert_eq!(
            str_at(&doc, &["mcp_servers", "grim", "env", "B"]),
            Some("${VAR}".to_string()),
            "env-ref literal preserved verbatim"
        );
    }

    #[test]
    fn upsert_is_idempotent_through_a_round_trip() {
        let value = json!({"command": "grim", "args": ["mcp"], "env": {"A": "${A}"}});
        let first = changed(upsert_member("", "mcp_servers", "grim", &value).unwrap());
        assert_eq!(
            upsert_member(&first, "mcp_servers", "grim", &value).unwrap(),
            Splice::Unchanged
        );
        let removed = changed(remove_member(&first, "mcp_servers", "grim").unwrap());
        let re_added = changed(upsert_member(&removed, "mcp_servers", "grim", &value).unwrap());
        let doc = parse(&re_added);
        assert_eq!(
            str_at(&doc, &["mcp_servers", "grim", "command"]),
            Some("grim".to_string())
        );
    }

    // ── C3.9 leftover: bool/int/float round-trip + the integer-overflow
    // boundary in `json_to_toml_value` ─────────────────────────────────────

    #[test]
    fn json_to_toml_bool_int_float_round_trip() {
        let value = json!({
            "enabled": true,
            "disabled": false,
            "port": 8080,
            "timeout": 2.5,
        });
        let out = changed(upsert_member("", "mcp_servers", "grim", &value).unwrap());
        let doc = parse(&out);
        assert_eq!(
            get_path(&doc, &["mcp_servers", "grim", "enabled"]).and_then(toml::Value::as_bool),
            Some(true)
        );
        assert_eq!(
            get_path(&doc, &["mcp_servers", "grim", "disabled"]).and_then(toml::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            get_path(&doc, &["mcp_servers", "grim", "port"]).and_then(toml::Value::as_integer),
            Some(8080)
        );
        assert_eq!(
            get_path(&doc, &["mcp_servers", "grim", "timeout"]).and_then(toml::Value::as_float),
            Some(2.5)
        );

        // The reverse conversion (`toml_item_to_json`, exercised via
        // `member_value`) reads every kind back to the same JSON value.
        assert_eq!(member_value(&out, "mcp_servers", "grim"), Some(value));
    }

    #[test]
    fn json_to_toml_integer_beyond_i64_falls_back_to_lossy_float() {
        // `json_to_toml_value`'s `Number` arm tries `as_i64()` first, then
        // `as_f64()`; the trailing `ok_or_else` error is defensive for a
        // JSON number `as_f64()` cannot represent at all — unreachable with
        // this crate's `serde_json = "1"` (no `arbitrary_precision`
        // feature), whose `Number::as_f64()` always succeeds (a lossy cast)
        // for every value it can hold. A number beyond `i64::MAX` exercises
        // the actual fallback: it converts, but loses integer precision,
        // rather than erroring.
        let huge = u64::MAX; // 18446744073709551615, beyond i64::MAX
        let value = json!({"big": huge});
        let out = changed(upsert_member("", "mcp_servers", "grim", &value).unwrap());
        let doc = parse(&out);
        let as_float = get_path(&doc, &["mcp_servers", "grim", "big"])
            .and_then(toml::Value::as_float)
            .expect("a beyond-i64 integer must fall back to a TOML float, not error");
        assert_eq!(as_float, huge as f64, "lossy f64 cast, not an error");
    }

    // ── C3.9 leftover: datetime + float read-back must not silently drop ──

    #[test]
    fn member_value_preserves_hand_authored_datetime_and_float_fields() {
        // `toml_value_to_json`'s `Datetime`/`Float` arms convert rather than
        // drop: `Datetime` degrades to its string form, `Float` converts to
        // a JSON number. A field that hand-authored TOML happens to spell
        // as one of these two kinds must still surface on read-back, not
        // vanish from the semantic entry value.
        let text = "[mcp_servers.grim]\ncommand = \"grim\"\ntimeout = 2.5\ncreated = 2024-01-01T00:00:00Z\n";
        let value = member_value(text, "mcp_servers", "grim").expect("member present");
        assert_eq!(value["command"], "grim");
        assert_eq!(value["timeout"], 2.5);
        let created = value["created"]
            .as_str()
            .expect("datetime degrades to a JSON string, not dropped");
        assert!(created.contains("2024-01-01"), "created: {created}");
    }

    // ── C3.9 leftover: member names requiring TOML quoting ────────────────

    #[test]
    fn upsert_member_name_requiring_quoting_round_trips() {
        for name in ["my server", "服务器"] {
            let value = json!({"command": "grim", "args": ["mcp"]});
            let out = changed(upsert_member("", "mcp_servers", name, &value).unwrap());
            let doc = parse(&out);
            assert_eq!(
                str_at(&doc, &["mcp_servers", name, "command"]),
                Some("grim".to_string()),
                "member name {name:?} must be addressable via toml::from_str: {out}"
            );
            assert_eq!(
                member_value(&out, "mcp_servers", name),
                Some(value),
                "member name {name:?} must round-trip through member_value"
            );
        }
    }

    // ── C3.9 leftover: deeper nesting (3-level object + array-of-objects) ─

    #[test]
    fn deeply_nested_object_and_array_of_objects_round_trip() {
        let value = json!({
            "command": "grim",
            "headers": {
                "auth": {"type": "bearer", "token": "${TOKEN}"}
            },
            "extra": [
                {"key": "a", "value": 1},
                {"key": "b", "value": 2}
            ]
        });
        let out = changed(upsert_member("", "mcp_servers", "grim", &value).unwrap());
        let doc = parse(&out);
        assert_eq!(
            get_path(&doc, &["mcp_servers", "grim", "headers", "auth", "type"]).and_then(toml::Value::as_str),
            Some("bearer")
        );
        // Read-back round-trips to the exact same JSON value, three levels
        // deep and through an array of objects.
        assert_eq!(member_value(&out, "mcp_servers", "grim"), Some(value));
    }

    #[test]
    fn realistic_codex_config_toml_only_touches_the_managed_span() {
        // A `~/.codex/config.toml`-shaped document: several foreign
        // top-level keys/tables plus a sibling MCP server entry.
        let text = concat!(
            "model = \"gpt-5-codex\"\n",
            "approval_policy = \"never\"\n",
            "\n",
            "[sandbox]\n",
            "mode = \"workspace-write\"\n",
            "\n",
            "[mcp_servers.other-server]\n",
            "command = \"npx\"\n",
            "args = [\"-y\", \"other\"]\n",
        );
        let out = changed(
            upsert_member(
                text,
                "mcp_servers",
                "grim-mcp",
                &json!({"command": "grim", "args": ["mcp"]}),
            )
            .unwrap(),
        );
        let doc = parse(&out);
        assert_eq!(str_at(&doc, &["model"]), Some("gpt-5-codex".to_string()));
        assert_eq!(str_at(&doc, &["approval_policy"]), Some("never".to_string()));
        assert_eq!(str_at(&doc, &["sandbox", "mode"]), Some("workspace-write".to_string()));
        assert_eq!(
            str_at(&doc, &["mcp_servers", "other-server", "command"]),
            Some("npx".to_string())
        );
        assert_eq!(
            str_at(&doc, &["mcp_servers", "grim-mcp", "command"]),
            Some("grim".to_string())
        );

        // Removal restores the original bytes exactly.
        let back = changed(remove_member(&out, "mcp_servers", "grim-mcp").unwrap());
        assert_eq!(back, text, "remove undoes upsert exactly");
    }

    // ── Warn: value escaping matrix + member-name injection confinement ────
    //
    // Mirrors `vendor_codex.rs`'s agent TOML escaping matrix on the splice
    // side: values needing TOML escaping (quotes/backslash/multiline/CRLF/
    // empty) and a value that itself looks like a table header must round-trip
    // byte-exact through `upsert_member` → `member_value` AND stay confined to
    // the one managed key (no sibling table injected via a crafted value).

    #[test]
    fn upsert_value_escaping_matrix_round_trips_and_confines_to_one_key() {
        let cases: &[serde_json::Value] = &[
            json!({"command": "grim", "note": "she said \"hi\" to a \"friend\""}),
            json!({"command": "C:\\Users\\test\\path and a literal \\n"}),
            json!({"command": "grim", "note": "line one\nline two\nline three"}),
            json!({"command": "grim", "note": "line one\r\nline two\r\n"}),
            json!({"command": ""}),
            // A value that itself spells a TOML table header must stay data,
            // never break out into a new `[mcp_servers.pwned]` table.
            json!({"command": "x\"]\n[mcp_servers.pwned]\nevil = \"yes"}),
        ];
        for value in cases {
            let out = changed(upsert_member("", "mcp_servers", "grim", value).unwrap());
            assert_eq!(
                member_value(&out, "mcp_servers", "grim").as_ref(),
                Some(value),
                "value must round-trip through member_value exactly: {value}"
            );
            let doc = parse(&out);
            let table = get_path(&doc, &["mcp_servers"])
                .and_then(toml::Value::as_table)
                .unwrap();
            assert_eq!(table.len(), 1, "exactly one member, no injected sibling: {table:?}");
            assert!(table.contains_key("grim"), "the one member is the intended one");
            assert!(
                get_path(&doc, &["mcp_servers", "pwned"]).is_none(),
                "no injected `pwned` table from a crafted value: {out}"
            );
        }
    }

    #[test]
    fn upsert_member_name_injection_is_confined_to_one_key() {
        // A member NAME crafted to break out of its quoted key must be written
        // as a single (quoted, escaped) key, not two tables. `toml_edit` quotes
        // and escapes the key; the round-trip proves the breakout is confined.
        let evil = "grim\"]\n[mcp_servers.pwned";
        let value = json!({"command": "grim"});
        let out = changed(upsert_member("", "mcp_servers", evil, &value).unwrap());
        let doc = parse(&out);
        let table = get_path(&doc, &["mcp_servers"])
            .and_then(toml::Value::as_table)
            .unwrap();
        assert_eq!(table.len(), 1, "one key only, no injected `pwned` table: {table:?}");
        assert!(table.contains_key(evil), "the crafted name is one literal key");
        assert!(
            get_path(&doc, &["mcp_servers", "pwned"]).is_none(),
            "no breakout table: {out}"
        );
        assert_eq!(
            member_value(&out, "mcp_servers", evil).as_ref(),
            Some(&value),
            "the crafted member name round-trips through member_value"
        );
    }

    // ── Warn: non-finite TOML floats survive into the semantic snapshot ────

    #[test]
    fn non_finite_float_is_preserved_not_dropped_from_snapshot() {
        // TOML allows nan/inf/-inf floats. A hand-authored one in grim's
        // managed entry must survive into the semantic snapshot — dropping it
        // would hide the field from the untracked-clobber gate and the
        // integrity hash. JSON's number type can't hold it, so it degrades to
        // its lexical string rather than vanishing.
        let text = "[mcp_servers.grim]\ncommand = \"grim\"\nweight = nan\nceiling = inf\nfloor = -inf\n";
        let value = member_value(text, "mcp_servers", "grim").expect("member present");
        assert_eq!(value["command"], "grim");
        for field in ["weight", "ceiling", "floor"] {
            assert!(
                value.get(field).is_some(),
                "non-finite float field '{field}' must not be dropped: {value}"
            );
            assert!(
                value[field].is_string(),
                "non-finite float '{field}' degrades to a string (JSON has no NaN/Infinity): {value}"
            );
        }
    }
}
