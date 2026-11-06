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
//!
//! **Key escaping (audited against GitHub #56).** This module builds no
//! TOML by string interpolation: `container` and `member` reach the
//! document only as arguments to `Table::entry`/`Table::insert`, which
//! quote and escape a key that needs it, and values go through
//! [`json_to_toml_value`] into typed [`toml_edit`] nodes. The only
//! `format!` calls here compose *error messages*. #56's defect therefore
//! has no analogue on this side, and the invariant is pinned by
//! `upsert_member_name_injection_is_confined_to_one_key` and
//! `upsert_value_escaping_matrix_round_trips_and_confines_to_one_key`
//! below. **Never introduce a `format!`-built TOML fragment here** — that
//! is the one edit that would reopen the class.

use std::io;
use std::ops::Range;

use toml_edit::{Array, ArrayOfTables, Document, DocumentMut, InlineTable, Item, Key, Table, Value};

use super::json_config::invalid_data;
// `split_pointer` is format-independent (pointer string parsing only), so
// callers reuse the single `json_splice` instance for both formats; this
// module re-exports it anyway for API parity/discoverability alongside the
// TOML-specific splice functions.
use super::json_splice::PointerRead;
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
    // Hide the container header on a table grim creates — only its
    // sub-tables print their own `[container.member]` headers. Set inside
    // `or_insert_with` so it NEVER touches a table parsed from user text: a
    // hand-authored bare `[mcp_servers]` header (and any comment attached
    // to it) is dropped by `toml_edit` the moment the flag is set, and
    // `remove_member` cannot put it back.
    let container_item = root.entry(container).or_insert_with(|| {
        let mut created = Table::new();
        created.set_implicit(true);
        Item::Table(created)
    });
    let Some(container_table) = container_item.as_table_mut() else {
        return Err(invalid_data(format!(
            "'{container}' is not a TOML table; refusing to edit"
        )));
    };

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

/// Read the value at `path` in TOML `text`, at any depth, through plain and
/// dotted tables and inline tables alike. Only those are entered: an array
/// is a leaf, and an array of tables is a type mismatch at any depth, so a
/// value holding one reads [`PointerRead::Blocked`] at `path`, because its
/// JSON form would silently drop it from the integrity hash.
///
/// # Errors
///
/// `InvalidData` when the text is not valid TOML. The message is fixed:
/// `TomlError` quotes the offending line, and a settings file is the user's.
pub fn value_at(text: &str, path: &[String]) -> io::Result<PointerRead> {
    let doc: DocumentMut = text.parse().map_err(|_: toml_edit::TomlError| not_toml())?;
    let mut current = doc.as_item();
    for (depth, segment) in path.iter().enumerate() {
        let Some(table) = current.as_table_like() else {
            return Ok(PointerRead::Blocked {
                ancestor: path[..depth].to_vec(),
            });
        };
        let Some(next) = table.get(segment) else {
            return Ok(PointerRead::Absent);
        };
        current = next;
    }
    if holds_array_of_tables(current) {
        return Ok(PointerRead::Blocked {
            ancestor: path.to_vec(),
        });
    }
    Ok(toml_item_to_json(current).map_or(PointerRead::Absent, PointerRead::Found))
}

/// Whether `item` is, or nests, an array of tables. An inline table cannot
/// hold one, so plain tables are all the walk enters.
fn holds_array_of_tables(item: &Item) -> bool {
    match item {
        Item::ArrayOfTables(_) => true,
        Item::Table(table) => table.iter().any(|(_, child)| holds_array_of_tables(child)),
        Item::None | Item::Value(_) => false,
    }
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
pub(crate) fn toml_item_to_json(item: &Item) -> Option<serde_json::Value> {
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

// ── Settings write operations ──
//
// `DocumentMut` keeps one spelling of a repeated key and writes `\n` after every
// line, so rendering it rewrites bytes far from the edit. These operations parse
// for spans and change bytes only there; `toml_edit` still renders each fragment.

type Span = Range<usize>;
type Edit = (Span, String);

const INLINE_BLANKS: [char; 2] = [' ', '\t'];
const BLANKS: [char; 4] = [' ', '\t', '\r', '\n'];

fn not_toml() -> io::Error {
    invalid_data("content is not TOML grim can edit; refusing to touch it".to_string())
}

fn no_member() -> io::Error {
    invalid_data("an empty path names no member; refusing to edit".to_string())
}

fn not_a_table(path: &[String]) -> io::Error {
    invalid_data(format!("'{}' is not a TOML table; refusing to edit", path.join(".")))
}

fn not_an_array(path: &[String]) -> io::Error {
    invalid_data(format!("'{}' is not a TOML array; refusing to edit", path.join(".")))
}

fn holds_tables(path: &[String]) -> io::Error {
    invalid_data(format!(
        "'{}' holds an array of tables; refusing to edit",
        path.join(".")
    ))
}

/// Ensure the member at `path` equals `value`, creating each table on the way.
///
/// Every byte outside the edit survives. An existing value keeps its key and
/// its in-line comment; a table or inline table takes an object key by key. A
/// new key lands beside its table's own keys, and a new table goes in its own
/// block, implicit parents unwritten.
///
/// # Errors
///
/// `InvalidData` for unparseable `text`, an empty `path`, a `null` in `value`,
/// a non-table on the way, or an array of tables at or below `path`.
pub fn upsert_member_at(text: &str, path: &[String], value: &serde_json::Value) -> io::Result<Splice> {
    if path.is_empty() {
        return Err(no_member());
    }
    let rendered = json_to_toml_value(value)?;
    let document = Document::parse(text).map_err(|_| not_toml())?;
    let source = Source::of(text);
    match locate(document.as_table(), path) {
        Located::Blocked { depth } => Err(not_a_table(&path[..depth])),
        Located::Absent { parent, depth } => source.insert_member(parent, path, depth, value, rendered),
        Located::Found { member, .. } => replace_member(text, member, path, value, rendered),
    }
}

/// Remove the member at `path`; an absent member is [`Splice::Unchanged`], and
/// so is a path under a non-table.
///
/// A table goes whole, with the tables below it, and an array of tables is
/// removable here. An emptied parent stays. A comment on the removed key's line
/// stays on a line of its own.
///
/// # Errors
///
/// `InvalidData` for unparseable `text` or an empty `path`.
pub fn remove_member_at(text: &str, path: &[String]) -> io::Result<Splice> {
    if path.is_empty() {
        return Err(no_member());
    }
    // A dotted member of an inline table is one item per leaf: one cut a pass.
    let mut removed = None;
    while let Some(next) = remove_step(removed.as_deref().unwrap_or(text), path)? {
        removed = Some(next);
    }
    Ok(removed.map_or(Splice::Unchanged, Splice::Changed))
}

fn remove_step(text: &str, path: &[String]) -> io::Result<Option<String>> {
    let document = Document::parse(text).map_err(|_| not_toml())?;
    let source = Source::of(text);
    let Located::Found { parent, member } = locate(document.as_table(), path) else {
        return Ok(None);
    };
    let key = path.last().ok_or_else(no_member)?;
    let cuts = source.member_cuts(parent, member, key)?;
    // The caller loops until the member is gone: an empty pass would never return.
    if cuts.is_empty() {
        return Err(not_toml());
    }
    Ok(Some(source.cut(cuts)))
}

/// Add `element` to the array at `path`, creating the array when absent. An
/// element already there, in any spelling, is [`Splice::Unchanged`].
///
/// # Errors
///
/// `InvalidData` for unparseable `text`, an empty `path`, a non-array at `path`
/// or a non-table on the way.
pub fn upsert_array_element_at(text: &str, path: &[String], element: &str) -> io::Result<Splice> {
    if path.is_empty() {
        return Err(no_member());
    }
    let document = Document::parse(text).map_err(|_| not_toml())?;
    let source = Source::of(text);
    match locate(document.as_table(), path) {
        Located::Found {
            member: Member::Value(Value::Array(array)),
            ..
        } => {
            if array.iter().any(|item| item.as_str() == Some(element)) {
                return Ok(Splice::Unchanged);
            }
            let edits = List::of_array(array)?.append(source, &Value::from(element).to_string(), "");
            Ok(Splice::Changed(splice(text, edits)))
        }
        Located::Found { .. } => Err(not_an_array(path)),
        Located::Absent { .. } | Located::Blocked { .. } => upsert_member_at(text, path, &serde_json::json!([element])),
    }
}

/// Remove every copy of `element` from the array at `path`; absent is
/// [`Splice::Unchanged`], and an emptied array stays.
///
/// # Errors
///
/// `InvalidData` for unparseable `text`, an empty `path` or a non-array at `path`.
pub fn remove_array_element_at(text: &str, path: &[String], element: &str) -> io::Result<Splice> {
    if path.is_empty() {
        return Err(no_member());
    }
    // Adjacent copies share a comma, so each pass reads the array afresh.
    let mut removed = None;
    while let Some(next) = remove_element_step(removed.as_deref().unwrap_or(text), path, element)? {
        removed = Some(next);
    }
    Ok(removed.map_or(Splice::Unchanged, Splice::Changed))
}

fn remove_element_step(text: &str, path: &[String], element: &str) -> io::Result<Option<String>> {
    let document = Document::parse(text).map_err(|_| not_toml())?;
    let source = Source::of(text);
    match locate(document.as_table(), path) {
        Located::Found {
            member: Member::Value(Value::Array(array)),
            ..
        } => {
            let Some(index) = array.iter().position(|item| item.as_str() == Some(element)) else {
                return Ok(None);
            };
            Ok(Some(source.cut(List::of_array(array)?.cuts(source, index))))
        }
        Located::Found { .. } => Err(not_an_array(path)),
        Located::Absent { .. } | Located::Blocked { .. } => Ok(None),
    }
}

/// Whether the empty array or table at `path` holds a comment: inside the array,
/// or on the table's header line, which goes with the table. A comment below a
/// bare header stays when the table goes, so it does not count.
///
/// # Errors
///
/// `InvalidData` for unparseable `text`.
pub fn empty_container_has_comment(text: &str, path: &[String]) -> io::Result<bool> {
    let document = Document::parse(text).map_err(|_| not_toml())?;
    let source = Source::of(text);
    let Located::Found { member, .. } = locate(document.as_table(), path) else {
        return Ok(false);
    };
    let holder = match member {
        Member::Value(_) => member.span(),
        Member::Table(table) => table.span().map(|header| header.end..source.line_end(header.end)),
        Member::Tables(_) | Member::Inline(_) => None,
    };
    Ok(holder.is_some_and(|span| text[span].contains('#')))
}

/// What can hold a member, and where a key under it starts in `path`.
#[derive(Clone, Copy)]
enum Parent<'d> {
    /// `owner` is the depth of the header its lines sit under: below it, a key
    /// line spells the rest of the path as a dotted key.
    Table { table: &'d Table, owner: usize },
    /// `braces` is the real inline table, `table` a dotted part of it or itself.
    Inline {
        braces: &'d InlineTable,
        table: &'d InlineTable,
        owner: usize,
    },
}

impl<'d> Parent<'d> {
    fn member(self, key: &str) -> Option<Member<'d>> {
        match self {
            Parent::Table { table, .. } => table.get(key).and_then(Member::of_item),
            Parent::Inline { table, .. } => table.get(key).map(Member::of_value),
        }
    }

    fn enter(self, member: Member<'d>, depth: usize) -> Option<Self> {
        match (self, member) {
            (Parent::Table { owner, .. }, Member::Table(table)) => Some(Parent::Table {
                table,
                owner: if table.is_dotted() { owner } else { depth },
            }),
            (Parent::Inline { braces, owner, .. }, Member::Inline(table)) if table.is_dotted() => {
                Some(Parent::Inline { braces, table, owner })
            }
            (_, Member::Inline(table)) => Some(Parent::Inline {
                braces: table,
                table,
                owner: depth,
            }),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
enum Member<'d> {
    Table(&'d Table),
    Tables(&'d ArrayOfTables),
    Inline(&'d InlineTable),
    Value(&'d Value),
}

impl<'d> Member<'d> {
    fn of_item(item: &'d Item) -> Option<Self> {
        match item {
            Item::None => None,
            Item::Value(value) => Some(Self::of_value(value)),
            Item::Table(table) => Some(Member::Table(table)),
            Item::ArrayOfTables(tables) => Some(Member::Tables(tables)),
        }
    }

    fn of_value(value: &'d Value) -> Self {
        match value {
            Value::InlineTable(table) => Member::Inline(table),
            other => Member::Value(other),
        }
    }

    fn holds_tables(self) -> bool {
        match self {
            Member::Tables(_) => true,
            Member::Table(table) => table.iter().any(|(_, child)| holds_array_of_tables(child)),
            Member::Inline(_) | Member::Value(_) => false,
        }
    }

    fn json(self) -> Option<serde_json::Value> {
        match self {
            Member::Table(table) => Some(table_to_json(table)),
            Member::Inline(table) => Some(inline_table_to_json(table)),
            Member::Value(value) => toml_value_to_json(value),
            Member::Tables(_) => None,
        }
    }

    fn span(self) -> Option<Span> {
        match self {
            Member::Table(table) => table.span(),
            Member::Tables(tables) => tables.span(),
            Member::Inline(table) => table.span(),
            Member::Value(value) => value.span(),
        }
    }

    /// The first value under a dotted inline table, whose own span is a key's.
    fn leaf_span(self) -> Option<Span> {
        match self {
            Member::Inline(table) if table.is_dotted() => table
                .iter()
                .next()
                .and_then(|(_, value)| Self::of_value(value).leaf_span()),
            other => other.span(),
        }
    }
}

enum Located<'d> {
    Found {
        parent: Parent<'d>,
        member: Member<'d>,
    },
    Absent {
        parent: Parent<'d>,
        depth: usize,
    },
    /// `path[..depth]` holds something no member can sit under.
    Blocked {
        depth: usize,
    },
}

fn locate<'d>(root: &'d Table, path: &[String]) -> Located<'d> {
    let mut parent = Parent::Table { table: root, owner: 0 };
    for (depth, key) in path.iter().enumerate() {
        let Some(member) = parent.member(key) else {
            return Located::Absent { parent, depth };
        };
        if depth + 1 == path.len() {
            return Located::Found { parent, member };
        }
        let Some(next) = parent.enter(member, depth + 1) else {
            return Located::Blocked { depth: depth + 1 };
        };
        parent = next;
    }
    Located::Blocked { depth: 0 }
}

fn entry_span(table: &Table, key: &str, value: Option<Span>) -> io::Result<Span> {
    let key = table.key(key).and_then(Key::span).ok_or_else(not_toml)?;
    Ok(key.start..value.ok_or_else(not_toml)?.end)
}

/// The key lines under `table`'s own header, dotted ones included, in file order.
fn own_entries(table: &Table) -> io::Result<Vec<Span>> {
    fn collect(table: &Table, entries: &mut Vec<Span>) -> io::Result<()> {
        for (key, item) in table.iter() {
            match item {
                Item::Value(value) => entries.push(entry_span(table, key, value.span())?),
                Item::Table(child) if child.is_dotted() => collect(child, entries)?,
                _ => {}
            }
        }
        Ok(())
    }
    let mut entries = Vec::new();
    collect(table, &mut entries)?;
    entries.sort_by_key(|entry| entry.start);
    Ok(entries)
}

/// `leaf` under one table per segment of `path`, without the final line break.
/// Nothing else may build TOML here: `Table::insert` is what quotes a key.
fn render_under(path: &[String], leaf: Item, shape: fn(&mut Table, bool)) -> String {
    let Some((first, rest)) = path.split_first() else {
        return String::new();
    };
    let mut item = leaf;
    for segment in rest.iter().rev() {
        let mut table = Table::new();
        shape(&mut table, true);
        table.insert(segment, item);
        item = Item::Table(table);
    }
    // The root stays plain: a dotted root renders nothing.
    let mut root = Table::new();
    root.insert(first, item);
    let mut rendered = DocumentMut::from(root).to_string();
    rendered.truncate(rendered.trim_end_matches('\n').len());
    rendered
}

fn header_fragment(path: &[String]) -> String {
    render_under(path, Item::Table(Table::new()), Table::set_implicit)
}

fn entry_fragment(path: &[String], value: Value) -> String {
    render_under(path, Item::Value(value), Table::set_dotted)
}

/// Apply `edits`, which do not overlap, back to front so their spans stay valid.
fn splice(text: &str, mut edits: Vec<Edit>) -> String {
    edits.sort_by_key(|(span, _)| span.start);
    let mut out = text.to_string();
    for (span, replacement) in edits.into_iter().rev() {
        out.replace_range(span, &replacement);
    }
    out
}

fn advance(current: String, step: Splice) -> String {
    match step {
        Splice::Changed(next) => next,
        Splice::Unchanged => current,
    }
}

/// A bracketed sequence: an array, or an inline table with one item per leaf.
struct List {
    /// The byte after the opening bracket.
    open: usize,
    /// The closing bracket.
    close: usize,
    items: Vec<Span>,
    trailing_comma: bool,
}

impl List {
    fn of_array(array: &Array) -> io::Result<Self> {
        let span = array.span().ok_or_else(not_toml)?;
        let items = array
            .iter()
            .map(|item| item.span().ok_or_else(not_toml))
            .collect::<io::Result<_>>()?;
        Ok(Self {
            open: span.start + 1,
            close: span.end - 1,
            items,
            trailing_comma: array.trailing_comma(),
        })
    }

    /// A dotted key's first segment is out of reach of the leaf's own key span,
    /// so an item starts where the separators before it end.
    fn of_inline(source: Source<'_>, braces: &InlineTable) -> io::Result<Self> {
        fn leaves(table: &InlineTable, out: &mut Vec<Span>) -> io::Result<()> {
            for (_, value) in table.iter() {
                match value {
                    Value::InlineTable(child) if child.is_dotted() => leaves(child, out)?,
                    other => out.push(other.span().ok_or_else(not_toml)?),
                }
            }
            Ok(())
        }
        let span = braces.span().ok_or_else(not_toml)?;
        let mut values = Vec::new();
        leaves(braces, &mut values)?;
        values.sort_by_key(|value| value.start);
        let mut cursor = span.start + 1;
        let mut items = Vec::with_capacity(values.len());
        for value in values {
            items.push(source.skip_trivia(cursor)..value.end);
            cursor = source.comma_after(value.end).map_or(value.end, |comma| comma + 1);
        }
        Ok(Self {
            open: span.start + 1,
            close: span.end - 1,
            items,
            trailing_comma: braces.trailing_comma(),
        })
    }

    /// A comma goes straight after the last value so a comment keeps its
    /// element; a list closing on its own line takes the new item on one too.
    fn append(&self, source: Source<'_>, item: &str, pad: &str) -> Vec<Edit> {
        let last = self.items.last();
        let close_line = source.line_start(self.close);
        if close_line > last.map_or(self.open, |last| last.end) && source.is_inline_blank(close_line..self.close) {
            let indent = last.map_or_else(
                || [source.indent(self.close), "  "].concat(),
                |last| source.indent(last.start).to_string(),
            );
            let comma = if self.trailing_comma { "," } else { "" };
            let line = [indent.as_str(), item, comma, source.newline].concat();
            let mut edits = vec![(close_line..close_line, line)];
            if let Some(last) = last.filter(|_| !self.trailing_comma) {
                edits.push((last.end..last.end, ",".to_string()));
            }
            return edits;
        }
        match last {
            Some(last) => vec![(last.end..last.end, [", ", item].concat())],
            None => vec![(self.open..self.close, [pad, item, pad].concat())],
        }
    }

    /// Item `index` and one comma; a comment beside either stays.
    #[expect(
        clippy::single_range_in_vec_init,
        reason = "a list of one span to cut, not a list of its indices"
    )]
    fn cuts(&self, source: Source<'_>, index: usize) -> Vec<Span> {
        let item = &self.items[index];
        let own_comma = source.comma_after(item.end);
        if self.items.len() == 1 {
            let end = own_comma.map_or(item.end, |comma| comma + 1);
            if source.is_inline_blank(self.open..item.start) && source.is_inline_blank(end..self.close) {
                return vec![self.open..self.close];
            }
            return source.item_cuts(item, own_comma);
        }
        let prior = index
            .checked_sub(1)
            .filter(|_| index + 1 == self.items.len())
            .and_then(|before| source.comma_after(self.items[before].end));
        match prior {
            Some(prior) if source.is_blank(prior + 1..item.start) => vec![prior..item.end],
            Some(prior) if own_comma.is_none() => vec![prior..prior + 1, source.line_cut(item.clone())],
            _ => source.item_cuts(item, own_comma),
        }
    }
}

/// A document's bytes and its line break, for the arithmetic of a splice.
#[derive(Clone, Copy)]
struct Source<'t> {
    text: &'t str,
    newline: &'static str,
}

impl<'t> Source<'t> {
    fn of(text: &'t str) -> Self {
        let crlf = text.split_once('\n').is_some_and(|(line, _)| line.ends_with('\r'));
        Self {
            text,
            newline: if crlf { "\r\n" } else { "\n" },
        }
    }

    /// Where the document starts: a cut or insert before a BOM would break it.
    fn body(self) -> usize {
        if self.text.starts_with('\u{feff}') {
            '\u{feff}'.len_utf8()
        } else {
            0
        }
    }

    fn line_start(self, at: usize) -> usize {
        self.text[..at]
            .rfind('\n')
            .map_or(self.body(), |line_break| line_break + 1)
    }

    /// The byte after the line break ending the line of `at`.
    fn line_end(self, at: usize) -> usize {
        self.text[at..]
            .find('\n')
            .map_or(self.text.len(), |line_break| at + line_break + 1)
    }

    fn skip_blanks(self, at: usize) -> usize {
        self.text.len() - self.text[at..].trim_start_matches(INLINE_BLANKS).len()
    }

    /// Past whitespace, line breaks and comments.
    fn skip_trivia(self, mut at: usize) -> usize {
        loop {
            at = self.text.len() - self.text[at..].trim_start_matches(BLANKS).len();
            if !self.text[at..].starts_with('#') {
                return at;
            }
            at = self.line_end(at);
        }
    }

    fn comma_after(self, at: usize) -> Option<usize> {
        let next = self.skip_trivia(at);
        self.text[next..].starts_with(',').then_some(next)
    }

    fn is_blank(self, span: Span) -> bool {
        self.text[span].trim_matches(BLANKS).is_empty()
    }

    fn is_inline_blank(self, span: Span) -> bool {
        self.text[span].trim_matches(INLINE_BLANKS).is_empty()
    }

    fn indent(self, at: usize) -> &'t str {
        let start = self.line_start(at);
        &self.text[start..self.skip_blanks(start)]
    }

    /// The start of the blank line ending at `at`, a line start.
    fn blank_line_before(self, at: usize) -> Option<usize> {
        if at <= self.body() || !self.text[..at].ends_with('\n') {
            return None;
        }
        let start = self.line_start(at - 1);
        self.is_blank(start..at).then_some(start)
    }

    fn cut(self, cuts: Vec<Span>) -> String {
        splice(self.text, cuts.into_iter().map(|span| (span, String::new())).collect())
    }

    /// `span` and the blanks after it, or the whole line when nothing else is on it.
    fn line_cut(self, span: Span) -> Span {
        let rest = self.skip_blanks(span.end);
        let line = self.line_start(span.start);
        let next = self.line_end(rest);
        if self.is_inline_blank(line..span.start) && self.is_blank(rest..next) {
            line..next
        } else {
            span.start..rest
        }
    }

    fn item_cuts(self, item: &Span, comma: Option<usize>) -> Vec<Span> {
        match comma {
            Some(comma) if self.is_blank(item.end..comma) => vec![self.line_cut(item.start..comma + 1)],
            Some(comma) => vec![self.line_cut(item.clone()), self.line_cut(comma..comma + 1)],
            None => vec![self.line_cut(item.clone())],
        }
    }

    /// A key line, unless a comment follows it: that one stays, on its own line.
    fn entry_cut(self, entry: &Span) -> Span {
        let start = self.line_start(entry.start);
        let rest = self.skip_blanks(entry.end);
        if self.text[rest..].starts_with('#') {
            start..rest
        } else {
            start..self.line_end(entry.end)
        }
    }

    /// Each header in `table`'s subtree through its last own key. Comments after
    /// that key stay: they may be about what follows.
    fn header_blocks(self, table: &Table, blocks: &mut Vec<Span>) -> io::Result<()> {
        if !table.is_implicit() {
            let header = table.span().ok_or_else(not_toml)?;
            let last = own_entries(table)?.iter().map(|entry| entry.end).max();
            let end = last.map_or(header.end, |last| last.max(header.end));
            blocks.push(self.line_start(header.start)..self.line_end(end));
        }
        for (_, item) in table.iter() {
            match item {
                Item::Table(child) => self.header_blocks(child, blocks)?,
                Item::ArrayOfTables(tables) => {
                    for child in tables.iter() {
                        self.header_blocks(child, blocks)?;
                    }
                }
                Item::None | Item::Value(_) => {}
            }
        }
        Ok(())
    }

    /// Join `blocks` across blank lines and take the blank line a block leaves
    /// doubled or leading, so removing a created block restores the bytes.
    fn tidy_blanks(self, mut blocks: Vec<Span>) -> Vec<Span> {
        blocks.sort_by_key(|block| block.start);
        let mut merged: Vec<Span> = Vec::new();
        for block in blocks {
            match merged.last_mut() {
                Some(last) if self.is_blank(last.end..block.start) => last.end = block.end,
                _ => merged.push(block),
            }
        }
        for cut in &mut merged {
            let next = self.line_end(cut.end);
            let blank_below = self.is_blank(cut.end..next);
            match self.blank_line_before(cut.start) {
                Some(above) if blank_below => cut.start = above,
                None if blank_below && cut.start <= self.body() => cut.end = next,
                _ => {}
            }
        }
        merged
    }

    fn member_cuts(self, parent: Parent<'_>, member: Member<'_>, key: &str) -> io::Result<Vec<Span>> {
        let table = match parent {
            Parent::Table { table, .. } => table,
            Parent::Inline { braces, .. } => {
                let leaf = member.leaf_span().ok_or_else(not_toml)?;
                let list = List::of_inline(self, braces)?;
                let index = list.items.iter().position(|item| item.end == leaf.end);
                return Ok(index.map_or_else(Vec::new, |index| list.cuts(self, index)));
            }
        };
        let mut blocks = Vec::new();
        match member {
            Member::Value(_) | Member::Inline(_) => {
                return Ok(vec![self.entry_cut(&entry_span(table, key, member.span())?)]);
            }
            Member::Table(child) => self.header_blocks(child, &mut blocks)?,
            Member::Tables(tables) => {
                for child in tables.iter() {
                    self.header_blocks(child, &mut blocks)?;
                }
            }
        }
        let mut cuts = self.tidy_blanks(blocks);
        if let Member::Table(child) = member
            && child.is_dotted()
        {
            cuts.extend(own_entries(child)?.iter().map(|entry| self.entry_cut(entry)));
        }
        Ok(cuts)
    }

    /// The first key line of a document with no root keys: after the last blank
    /// line of the head, so a comment sitting on the first header stays on it.
    fn head_position(self) -> usize {
        let mut at = self.body();
        let mut past_blank = at;
        while at < self.text.len() {
            let next = self.line_end(at);
            let line = self.text[at..next].trim_matches(BLANKS);
            if line.is_empty() {
                past_blank = next;
            } else if !line.starts_with('#') {
                return past_blank;
            }
            at = next;
        }
        self.text.len()
    }

    /// Where a block goes after the text ending at `end`: past the comments that
    /// close it, and before a comment sitting directly on the next header.
    fn past_tail(self, end: usize) -> usize {
        let mut at = end;
        let mut closed = end;
        let mut comment_end = None;
        while at < self.text.len() {
            let next = self.line_end(at);
            let line = self.text[at..next].trim_matches(BLANKS);
            if line.is_empty() {
                closed = comment_end.take().unwrap_or(closed);
            } else if line.starts_with('#') {
                comment_end = Some(next);
            } else {
                return closed;
            }
            at = next;
        }
        self.text.len()
    }

    fn insert_line(self, at: usize, indent: &str, line: &str) -> Splice {
        let unterminated = at == self.text.len() && at > self.body() && !self.text.ends_with('\n');
        let lead = if unterminated { self.newline } else { "" };
        let inserted = [lead, indent, line, self.newline].concat();
        Splice::Changed(splice(self.text, vec![(at..at, inserted)]))
    }

    fn insert_block(self, at: usize, lines: &[String]) -> Splice {
        let mut inserted = String::new();
        if at > self.body() {
            if !self.text[..at].ends_with('\n') {
                inserted.push_str(self.newline);
            }
            if self.blank_line_before(at).is_none() {
                inserted.push_str(self.newline);
            }
        }
        inserted.push_str(&lines.join(self.newline));
        inserted.push_str(self.newline);
        Splice::Changed(splice(self.text, vec![(at..at, inserted)]))
    }

    fn insert_member(
        self,
        parent: Parent<'_>,
        path: &[String],
        depth: usize,
        value: &serde_json::Value,
        rendered: Value,
    ) -> io::Result<Splice> {
        let (table, owner) = match parent {
            Parent::Table { table, owner } => (table, owner),
            Parent::Inline { braces, owner, .. } => {
                let item = entry_fragment(&path[owner..], rendered);
                let edits = List::of_inline(self, braces)?.append(self, &item, " ");
                return Ok(Splice::Changed(splice(self.text, edits)));
            }
        };
        let direct = depth + 1 == path.len() && !value.is_object();
        if table.is_dotted() || (direct && !table.is_implicit()) {
            let entries = own_entries(table)?;
            let (at, indent) = match entries.last() {
                Some(last) => (self.line_end(last.end), self.indent(last.start)),
                None if depth == 0 => (self.head_position(), ""),
                None => (self.line_end(table.span().ok_or_else(not_toml)?.end), ""),
            };
            return Ok(self.insert_line(at, indent, &entry_fragment(&path[owner..], rendered)));
        }

        let mut lines = Vec::new();
        if let serde_json::Value::Object(fields) = value {
            lines.push(header_fragment(path));
            for (key, field) in fields {
                lines.push(entry_fragment(std::slice::from_ref(key), json_to_toml_value(field)?));
            }
        } else {
            let (header, key) = path.split_at(path.len() - 1);
            lines.push(header_fragment(header));
            lines.push(entry_fragment(key, rendered));
        }
        let at = if depth == 0 {
            self.text.len()
        } else {
            let mut blocks = Vec::new();
            self.header_blocks(table, &mut blocks)?;
            let end = blocks.iter().map(|block| block.end).max();
            end.map_or(self.text.len(), |end| self.past_tail(end))
        };
        Ok(self.insert_block(at, &lines))
    }
}

fn replace_member(
    text: &str,
    member: Member<'_>,
    path: &[String],
    value: &serde_json::Value,
    rendered: Value,
) -> io::Result<Splice> {
    if member.holds_tables() {
        return Err(holds_tables(path));
    }
    if member.json().as_ref() == Some(value) {
        return Ok(Splice::Unchanged);
    }
    match (member, value) {
        (Member::Table(table), serde_json::Value::Object(fields)) => {
            let old = table.iter().map(|(key, _)| key.to_string()).collect();
            replace_fields(text, path, old, fields, value)
        }
        (Member::Inline(table), serde_json::Value::Object(fields)) => {
            let old = table.iter().map(|(key, _)| key.to_string()).collect();
            replace_fields(text, path, old, fields, value)
        }
        (Member::Table(_), _) => replace_whole(text, path, value),
        (Member::Inline(table), _) if table.is_dotted() => replace_whole(text, path, value),
        _ => {
            let span = member.span().ok_or_else(not_toml)?;
            Ok(Splice::Changed(splice(text, vec![(span, rendered.to_string())])))
        }
    }
}

/// A table has no single span to overwrite, so its text goes first.
fn replace_whole(text: &str, path: &[String], value: &serde_json::Value) -> io::Result<Splice> {
    let without = advance(text.to_string(), remove_member_at(text, path)?);
    let step = upsert_member_at(&without, path, value)?;
    Ok(Splice::Changed(advance(without, step)))
}

/// Key by key, so what the object keeps keeps its place and its comments.
fn replace_fields(
    text: &str,
    path: &[String],
    old: Vec<String>,
    fields: &serde_json::Map<String, serde_json::Value>,
    value: &serde_json::Value,
) -> io::Result<Splice> {
    let child = |key: &String| [path, std::slice::from_ref(key)].concat();
    let mut out = text.to_string();
    for key in old.iter().filter(|key| !fields.contains_key(*key)) {
        let step = remove_member_at(&out, &child(key))?;
        out = advance(out, step);
    }
    for (key, field) in fields {
        let step = upsert_member_at(&out, &child(key), field)?;
        out = advance(out, step);
    }
    if fields.is_empty() {
        // The last key of an implicit or dotted table was all its text.
        let step = upsert_member_at(&out, path, value)?;
        out = advance(out, step);
    }
    Ok(Splice::Changed(out))
}

#[cfg(test)]
mod tests {
    use super::super::json_splice::split_pointer_path;
    use super::*;
    use serde_json::json;

    /// The settings writer codes against these four shapes.
    #[test]
    fn the_settings_write_operations_hold_their_signatures() {
        let _: fn(&str, &[String], &serde_json::Value) -> io::Result<Splice> = upsert_member_at;
        let _: fn(&str, &[String]) -> io::Result<Splice> = remove_member_at;
        let _: fn(&str, &[String], &str) -> io::Result<Splice> = upsert_array_element_at;
        let _: fn(&str, &[String], &str) -> io::Result<Splice> = remove_array_element_at;
    }

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

    /// Regression: a hand-authored bare `[mcp_servers]` header and the
    /// comment attached to it must survive an upsert. `set_implicit(true)`
    /// used to run on the parsed container unconditionally, which told
    /// `toml_edit` to stop printing a header the user wrote themselves —
    /// taking its comment with it, unrecoverably (`remove_member` cannot
    /// put back a header it never saw).
    #[test]
    fn upsert_preserves_a_hand_authored_container_header_and_its_comment() {
        let text = "# --- My MCP servers ---\n[mcp_servers]\n\n[mcp_servers.mine]\ncommand = \"x\"\n";
        let out = changed(upsert_member(text, "mcp_servers", "grim", &json!({"command": "grim"})).unwrap());
        assert!(
            out.contains("# --- My MCP servers ---"),
            "the comment attached to the user's header survives: {out}"
        );
        assert!(
            out.contains("\n[mcp_servers]\n"),
            "the user's bare container header survives: {out}"
        );
        let doc = parse(&out);
        assert_eq!(
            str_at(&doc, &["mcp_servers", "mine", "command"]),
            Some("x".to_string()),
            "sibling server untouched"
        );

        // Round trip: removing what was just added restores the original
        // bytes — the round-trip guarantee the implicit flag broke.
        let back = changed(remove_member(&out, "mcp_servers", "grim").unwrap());
        assert_eq!(back, text, "remove undoes upsert exactly");
    }

    /// The complement: a container grim itself creates stays implicit, so
    /// no bare `[mcp_servers]` husk is emitted above the entry table.
    #[test]
    fn grim_created_container_stays_implicit() {
        let out = changed(upsert_member("", "mcp_servers", "grim", &json!({"command": "grim"})).unwrap());
        assert!(
            !out.contains("[mcp_servers]"),
            "a grim-created container prints no bare header: {out}"
        );
        assert!(out.contains("[mcp_servers.grim]"), "only the entry table: {out}");
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

    // ── `value_at` at any depth ─────────────────────────────────────────

    fn path(segments: &[&str]) -> Vec<String> {
        segments.iter().map(ToString::to_string).collect()
    }

    fn found(value: serde_json::Value) -> PointerRead {
        PointerRead::Found(value)
    }

    fn blocked(segments: &[&str]) -> PointerRead {
        PointerRead::Blocked {
            ancestor: path(segments),
        }
    }

    #[test]
    fn value_at_reads_a_one_segment_pointer() {
        let text = "model = \"gpt-5\"\n\n[tui]\nnotifications = true\n";
        assert_eq!(value_at(text, &path(&["model"])).unwrap(), found(json!("gpt-5")));
        assert_eq!(
            value_at(text, &path(&["tui"])).unwrap(),
            found(json!({"notifications": true}))
        );
    }

    #[test]
    fn value_at_reads_every_table_spelling_at_any_depth() {
        for text in [
            "[a.b.c]\nd = 1\n",
            "[a]\n[a.b]\n[a.b.c]\nd = 1\n",
            "a.b.c.d = 1\n",
            "[a]\nb.c.d = 1\n",
            "a = { b = { c = { d = 1 } } }\n",
            "[a]\nb = { c = { d = 1 } }\n",
            "a = { b.c.d = 1 }\n",
        ] {
            assert_eq!(
                value_at(text, &path(&["a", "b", "c", "d"])).unwrap(),
                found(json!(1)),
                "{text}"
            );
            assert_eq!(
                value_at(text, &path(&["a", "b", "c"])).unwrap(),
                found(json!({"d": 1})),
                "{text}"
            );
        }
    }

    #[test]
    fn value_at_keeps_a_segment_holding_a_slash_tilde_or_dot_whole() {
        let text = "[t]\n\"a/b\" = 1\n\"m~n\" = 2\n\"~1\" = 3\n\"x.y\" = 4\n";
        for (pointer, want) in [("/t/a~1b", 1), ("/t/m~0n", 2), ("/t/~01", 3), ("/t/x.y", 4)] {
            let segments = split_pointer_path(pointer).unwrap();
            assert_eq!(
                value_at(text, &segments).unwrap(),
                found(json!(want)),
                "pointer: {pointer}"
            );
        }
    }

    #[test]
    fn value_at_tells_absent_from_blocked() {
        let text = "[a]\nb = 1\n";
        assert_eq!(value_at(text, &path(&["z"])).unwrap(), PointerRead::Absent);
        assert_eq!(value_at(text, &path(&["a", "z"])).unwrap(), PointerRead::Absent);
        assert_eq!(value_at(text, &path(&["z", "y", "x"])).unwrap(), PointerRead::Absent);
        assert_eq!(value_at(text, &path(&["a", "b", "c"])).unwrap(), blocked(&["a", "b"]));
    }

    #[test]
    fn value_at_blocks_on_the_first_ancestor_that_is_not_a_table() {
        let text = "s = \"text\"\nl = [{ x = 1 }]\n[a]\nt = true\n[[w]]\nx = 1\n";
        assert_eq!(value_at(text, &path(&["s", "x"])).unwrap(), blocked(&["s"]));
        assert_eq!(
            value_at(text, &path(&["l", "0"])).unwrap(),
            blocked(&["l"]),
            "no array index"
        );
        assert_eq!(
            value_at(text, &path(&["a", "t", "x", "y"])).unwrap(),
            blocked(&["a", "t"])
        );
        assert_eq!(
            value_at(text, &path(&["w", "x"])).unwrap(),
            blocked(&["w"]),
            "array of tables"
        );
    }

    #[test]
    fn value_at_reads_an_array_of_tables_at_the_path_as_a_type_mismatch() {
        let text = "[[a.w]]\nx = 1\n\n[b]\nc = 1\n[[b.d.e]]\ny = 2\n";
        assert_eq!(value_at(text, &path(&["a", "w"])).unwrap(), blocked(&["a", "w"]));
        assert_eq!(
            value_at(text, &path(&["b"])).unwrap(),
            blocked(&["b"]),
            "a table cannot read as whole while a nested array of tables is dropped from it"
        );
        assert_eq!(value_at(text, &path(&["b", "c"])).unwrap(), found(json!(1)), "control");
    }

    #[test]
    fn value_at_reads_an_empty_document_as_absent() {
        for text in ["", "  \n", "# only a comment\n"] {
            assert_eq!(
                value_at(text, &path(&["a", "b"])).unwrap(),
                PointerRead::Absent,
                "{text:?}"
            );
        }
    }

    #[test]
    fn value_at_refuses_unparseable_text_without_echoing_it() {
        assert_eq!(value_at("a = 1\n", &path(&["a"])).unwrap(), found(json!(1)), "control");
        let err = value_at("token = hunter2-sentinel\n", &path(&["a"])).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(!err.to_string().contains("hunter2-sentinel"), "{err}");
    }

    #[test]
    fn value_at_agrees_with_member_value_on_a_two_level_pointer() {
        let text = "[mcp_servers.grim]\ncommand = \"grim\"\nargs = [\"mcp\"]\nenv = { A = \"1\" }\n";
        assert_eq!(
            value_at(text, &path(&["mcp_servers", "grim"])).unwrap(),
            found(member_value(text, "mcp_servers", "grim").unwrap())
        );
    }

    #[test]
    fn value_at_keeps_a_non_finite_float_in_its_lexical_form() {
        assert_eq!(value_at("w = inf\n", &path(&["w"])).unwrap(), found(json!("inf")));
    }

    // ── Settings write operations: the whole file is the unit under test ──

    fn upsert(text: &str, target: &[&str], value: serde_json::Value) -> String {
        changed(upsert_member_at(text, &path(target), &value).unwrap())
    }

    fn remove(text: &str, target: &[&str]) -> String {
        changed(remove_member_at(text, &path(target)).unwrap())
    }

    fn add_element(text: &str, target: &[&str], element: &str) -> String {
        changed(upsert_array_element_at(text, &path(target), element).unwrap())
    }

    fn drop_element(text: &str, target: &[&str], element: &str) -> String {
        changed(remove_array_element_at(text, &path(target), element).unwrap())
    }

    fn assert_refused(result: io::Result<Splice>) {
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    /// The document as a second parser reads it, arrays of tables included.
    fn meaning(text: &str) -> serde_json::Value {
        serde_json::to_value(parse(text)).unwrap()
    }

    #[test]
    fn upsert_leaves_an_untouched_array_of_tables_byte_identical() {
        // Two spellings of one header: toml_edit keeps one key per table, so
        // rendering its document rewrites the second to the first.
        let plugins = "# first plugin\n[[ plugins ]]\nname = \"a\"\n\n# second plugin\n[[plugins]]\nname = \"b\"\n";
        let text = format!("model = \"x\"\n\n{plugins}");

        assert_eq!(
            upsert(&text, &["sandbox", "mode"], json!("ro")),
            format!("model = \"x\"\n\n{plugins}\n[sandbox]\nmode = \"ro\"\n")
        );
        assert_eq!(
            upsert(&text, &["effort"], json!("high")),
            format!("model = \"x\"\neffort = \"high\"\n\n{plugins}")
        );

        let nested = "[[ a . b ]]\nx = 1\n\n# c\n[[a.\"b\"]]\nx = 2\n\n[z]\ny = 1\n";
        assert_eq!(
            upsert(nested, &["z", "k"], json!(true)),
            "[[ a . b ]]\nx = 1\n\n# c\n[[a.\"b\"]]\nx = 2\n\n[z]\ny = 1\nk = true\n"
        );
    }

    #[test]
    fn upsert_keeps_every_other_byte_a_document_render_would_change() {
        for (text, target, value, want) in [
            (
                "[ a . b ]\nx = 1\n\n[a.c]\ny = 2\n",
                &["a", "d", "e"][..],
                json!(1),
                "[ a . b ]\nx = 1\n\n[a.c]\ny = 2\n\n[a.d]\ne = 1\n",
            ),
            (
                "a.x = 1\nb = 2\na.y = 3\n",
                &["c"],
                json!(4),
                "a.x = 1\nb = 2\na.y = 3\nc = 4\n",
            ),
            ("a = 1", &["b"], json!(2), "a = 1\nb = 2\n"),
            ("[t]\nk = \"v\"", &["t", "j"], json!(2), "[t]\nk = \"v\"\nj = 2\n"),
            ("a = 1\n\n\n", &["b", "c"], json!(2), "a = 1\n\n\n[b]\nc = 2\n"),
        ] {
            assert_eq!(upsert(text, target, value), want, "{text:?}");
        }
    }

    #[test]
    fn dotted_table_leading_comment_is_not_duplicated() {
        let text = "# about a\na.b = 1\na.c = 2\n\n[t]\nx = 1\n";
        assert_eq!(
            upsert(text, &["a", "d"], json!(3)),
            "# about a\na.b = 1\na.c = 2\na.d = 3\n\n[t]\nx = 1\n"
        );
        for out in [
            upsert(text, &["t", "y"], json!(2)),
            upsert(text, &["a", "b"], json!(9)),
            upsert(text, &["z"], json!(1)),
            upsert(text, &["a", "e", "f"], json!(1)),
            remove(text, &["a", "c"]),
        ] {
            assert_eq!(out.matches("# about a").count(), 1, "{out:?}");
        }

        let under_a_header = "[h]\n# about x\nx.y = 1\n";
        assert_eq!(
            upsert(under_a_header, &["h", "x", "z"], json!(2)),
            "[h]\n# about x\nx.y = 1\nx.z = 2\n"
        );
        assert_eq!(
            upsert(under_a_header, &["h", "w"], json!(2)),
            "[h]\n# about x\nx.y = 1\nw = 2\n"
        );
        assert_eq!(
            upsert(under_a_header, &["h", "x", "q", "r"], json!(2)),
            "[h]\n# about x\nx.y = 1\nx.q.r = 2\n"
        );
    }

    #[test]
    fn dotted_table_takes_a_new_key_beside_its_own_keys() {
        assert_eq!(
            upsert("a.x = 1\nb = 2\n", &["a", "y"], json!(3)),
            "a.x = 1\na.y = 3\nb = 2\n"
        );
        assert_eq!(
            upsert("a.x = 1\n", &["a", "y"], json!({"k": 1})),
            "a.x = 1\na.y = { k = 1 }\n"
        );
        assert_eq!(
            upsert("[h]\nx.y = 1\n[h.x.z]\nq = 1\n", &["h", "x", "w"], json!(2)),
            "[h]\nx.y = 1\nx.w = 2\n[h.x.z]\nq = 1\n"
        );
    }

    #[test]
    fn head_comment_survives_the_first_table_created_and_pruned() {
        for head in ["# head\n", "# head\n# more\n", "\u{feff}# head\n"] {
            let created = upsert(head, &["a", "b"], json!(1));
            assert_eq!(created, format!("{head}\n[a]\nb = 1\n"));
            assert_eq!(remove(&created, &["a"]), head);
        }

        assert_eq!(
            remove("# head\n\n[a]\nb = 1\n\n[c]\nd = 2\n", &["a"]),
            "# head\n\n[c]\nd = 2\n"
        );
        assert_eq!(remove("# head\n[a]\nb = 1\n", &["a"]), "# head\n");
        assert_eq!(upsert("# head", &["a", "b"], json!(1)), "# head\n\n[a]\nb = 1\n");
    }

    #[test]
    fn root_key_lands_above_the_first_header_and_its_own_comment() {
        for (text, want) in [
            ("", "z = 1\n"),
            ("# head\n", "# head\nz = 1\n"),
            ("[a]\nb = 1\n", "z = 1\n[a]\nb = 1\n"),
            ("# head\n\n[a]\nb = 1\n", "# head\n\nz = 1\n[a]\nb = 1\n"),
            ("# about a\n[a]\nb = 1\n", "z = 1\n# about a\n[a]\nb = 1\n"),
            ("# head\n\n# about a\n[a]\n", "# head\n\nz = 1\n# about a\n[a]\n"),
            ("\u{feff}[a]\nb = 1\n", "\u{feff}z = 1\n[a]\nb = 1\n"),
            ("y = 2 # c\n\n[a]\n", "y = 2 # c\nz = 1\n\n[a]\n"),
        ] {
            let out = upsert(text, &["z"], json!(1));
            assert_eq!(out, want, "{text:?}");
            assert_eq!(remove(&out, &["z"]), text, "{text:?}");
        }
    }

    #[test]
    fn scalar_replacement_keeps_an_in_line_comment() {
        assert_eq!(
            upsert("a = 1 # keep\nb = 2\n", &["a"], json!(2)),
            "a = 2 # keep\nb = 2\n"
        );
        assert_eq!(
            upsert("[t]\n  a   =   \"x\"   # keep\n", &["t", "a"], json!("y")),
            "[t]\n  a   =   \"y\"   # keep\n"
        );
        assert_eq!(
            upsert("a.b = 1 # keep\n", &["a", "b"], json!([1, 2])),
            "a.b = [1, 2] # keep\n"
        );
        assert_eq!(
            upsert("a = 1 # keep\n", &["a"], json!({"k": 1})),
            "a = { k = 1 } # keep\n"
        );
    }

    #[test]
    fn bare_table_header_with_a_comment_survives() {
        let text = "# about servers\n[mcp_servers] # hand written\n\n[mcp_servers.other]\ncommand = \"x\"\n";

        let out = upsert(text, &["mcp_servers", "grim", "command"], json!("grim"));
        assert_eq!(out, format!("{text}\n[mcp_servers.grim]\ncommand = \"grim\"\n"));
        assert_eq!(remove(&out, &["mcp_servers", "grim"]), text);

        assert_eq!(
            upsert(text, &["mcp_servers", "on"], json!(true)),
            "# about servers\n[mcp_servers] # hand written\non = true\n\n[mcp_servers.other]\ncommand = \"x\"\n"
        );
        assert_eq!(
            remove("# about\n[t] # note\n\n[t.a]\nb = 1\n", &["t", "a"]),
            "# about\n[t] # note\n"
        );
    }

    #[test]
    fn created_tables_stay_implicit_where_allowed() {
        assert_eq!(upsert("", &["a", "b", "c"], json!(1)), "[a.b]\nc = 1\n");
        assert_eq!(upsert("", &["a", "b"], json!({"k": 1})), "[a.b]\nk = 1\n");
        assert_eq!(upsert("", &["a", "b"], json!({})), "[a.b]\n");
        assert_eq!(
            upsert("[x]\ny = 1\n", &["a", "b", "c"], json!(1)),
            "[x]\ny = 1\n\n[a.b]\nc = 1\n"
        );
        assert_eq!(
            upsert("[x]\ny = 1\n\n[z]\n", &["x", "p", "q"], json!(1)),
            "[x]\ny = 1\n\n[x.p]\nq = 1\n\n[z]\n"
        );
        assert_eq!(
            upsert("[a.b]\nx = 1\n", &["a", "k"], json!(1)),
            "[a.b]\nx = 1\n\n[a]\nk = 1\n",
            "a key on a table with no text of its own needs that table's header"
        );
    }

    #[test]
    fn new_block_keeps_a_tail_comment_with_the_table_it_closes() {
        assert_eq!(
            upsert("[x]\na = 1\n# tail\n\n[z]\n", &["x", "k", "m"], json!(1)),
            "[x]\na = 1\n# tail\n\n[x.k]\nm = 1\n\n[z]\n"
        );
        assert_eq!(
            upsert("[x]\na = 1\n# about z\n[z]\n", &["x", "k", "m"], json!(1)),
            "[x]\na = 1\n\n[x.k]\nm = 1\n# about z\n[z]\n"
        );
    }

    #[test]
    fn inline_tables_edit_in_place() {
        for (text, target, value, want) in [
            (
                "a = { b = 1 } # keep\nc = 2\n",
                &["a", "d"][..],
                json!(2),
                "a = { b = 1, d = 2 } # keep\nc = 2\n",
            ),
            ("a = {}\n", &["a", "d"], json!(2), "a = { d = 2 }\n"),
            ("a = { }\n", &["a", "d"], json!(2), "a = { d = 2 }\n"),
            ("a = { b = 1, }\n", &["a", "d"], json!(2), "a = { b = 1, d = 2, }\n"),
            ("a = { b = 1 }\n", &["a", "b"], json!(5), "a = { b = 5 }\n"),
            (
                "a = { b = { c = 1 } }\n",
                &["a", "b", "d"],
                json!(2),
                "a = { b = { c = 1, d = 2 } }\n",
            ),
            (
                "a = { b = 1 }\n",
                &["a", "x", "y"],
                json!(2),
                "a = { b = 1, x.y = 2 }\n",
            ),
            (
                "a = { b = 1 }\n",
                &["a", "x"],
                json!({"y": 2}),
                "a = { b = 1, x = { y = 2 } }\n",
            ),
            (
                "[t]\nenv = { A = \"1\" }\nz = 1\n",
                &["t", "env", "B"],
                json!("2"),
                "[t]\nenv = { A = \"1\", B = \"2\" }\nz = 1\n",
            ),
            (
                "a = { b.c = 1 }\n",
                &["a", "b", "d"],
                json!(2),
                "a = { b.c = 1, b.d = 2 }\n",
            ),
            (
                "a = {\n  b = 1, # c\n  c = 2,\n}\n",
                &["a", "d"],
                json!(3),
                "a = {\n  b = 1, # c\n  c = 2,\n  d = 3,\n}\n",
            ),
        ] {
            let out = upsert(text, target, value.clone());
            assert_eq!(out, want, "{text:?}");
            assert_eq!(value_at(&out, &path(target)).unwrap(), found(value.clone()), "{text:?}");
        }

        for (text, target, want) in [
            ("a = { b = 1, c = 2 }\n", &["a", "b"][..], "a = { c = 2 }\n"),
            ("a = { b = 1, c = 2 }\n", &["a", "c"], "a = { b = 1 }\n"),
            ("a = { b = 1 }\n", &["a", "b"], "a = {}\n"),
            ("a = { b = 1, }\n", &["a", "b"], "a = {}\n"),
            ("a = { b.c = 1, b.d = 2, e = 3 }\n", &["a", "b"], "a = { e = 3 }\n"),
            ("a = { b.c = 1, e = 3 }\n", &["a", "b", "c"], "a = { e = 3 }\n"),
            (
                "a = {\n  b = 1, # c\n  c = 2,\n}\n",
                &["a", "c"],
                "a = {\n  b = 1, # c\n}\n",
            ),
        ] {
            assert_eq!(remove(text, target), want, "{text:?}");
        }
    }

    #[test]
    fn inline_table_takes_a_replacement_member_by_member() {
        assert_eq!(
            upsert("a = { b = 1, c = 2 } # keep\n", &["a"], json!({"b": 1, "d": 3})),
            "a = { b = 1, d = 3 } # keep\n"
        );
        assert_eq!(upsert("a = { b.c = 1 }\n", &["a", "b"], json!(5)), "a = { b = 5 }\n");
        assert_eq!(
            upsert("a = {b=1, c=2}\n", &["a"], json!({"b": 1, "d": 3})),
            "a = {b=1, d = 3}\n"
        );
        assert_eq!(
            upsert("a = {\n  b = 1, # keep\n  c = 2,\n}\n", &["a"], json!({"b": 1, "d": 3})),
            "a = {\n  b = 1, # keep\n  d = 3,\n}\n"
        );
        assert_eq!(
            upsert("a = { b.c = 1 } # keep\n", &["a", "b"], json!({"c": 1, "d": 2})),
            "a = { b.c = 1, b.d = 2 } # keep\n"
        );
    }

    #[test]
    fn array_of_tables_is_a_type_mismatch() {
        let text = "[[w]]\nx = 1\n\n[b]\nc = 1\n[[b.d.e]]\ny = 2\n";
        for target in [
            &["w"][..],
            &["w", "x"],
            &["w", "new"],
            &["b"],
            &["b", "d"],
            &["b", "d", "e"],
            &["b", "d", "e", "y"],
        ] {
            assert_refused(upsert_member_at(text, &path(target), &json!(1)));
        }
        assert_refused(upsert_member_at(text, &path(&["w"]), &json!({"x": 1})));
        assert_refused(upsert_array_element_at(text, &path(&["w"]), "x"));
        assert_refused(remove_array_element_at(text, &path(&["w"]), "x"));
        assert_eq!(upsert(text, &["b", "c"], json!(2)).matches("[[").count(), 2, "control");
    }

    #[test]
    fn remove_takes_an_array_of_tables_whole() {
        let text = "# c\n[[w]]\nx = 1\n\n# d\n[[w]]\nx = 2\n\n[b]\nc = 1\n";
        let out = remove(text, &["w"]);
        assert_eq!(meaning(&out), json!({"b": {"c": 1}}));
        assert!(out.contains("# c") && out.contains("# d"), "{out:?}");
        assert_eq!(
            remove_member_at(&out, &path(&["w"])).unwrap(),
            Splice::Unchanged,
            "{out:?}"
        );
    }

    #[test]
    fn remove_takes_arrays_of_tables_below_the_member() {
        for (text, target) in [
            ("[b]\nc = 1\n[[b.d]]\ny = 2\n[[b.d]]\ny = 3\n\n[z]\nq = 1\n", "b"),
            ("[[w]]\nx = 1\n[[w.v]]\ny = 1\n[w.t]\nz = 2\n\n[z]\nq = 1\n", "w"),
            ("[[b.d.e]]\ny = 2\n\n[z]\nq = 1\n", "b"),
        ] {
            assert_eq!(remove(text, &[target]), "[z]\nq = 1\n", "{text:?}");
        }
    }

    #[test]
    fn upsert_at_depth_creates_containers() {
        assert_eq!(value_at("", &path(&["a"])).unwrap(), PointerRead::Absent, "control");
        let out = upsert("[x]\ny = 1\n", &["a", "b", "c", "d"], json!({"k": ["v"]}));
        assert_eq!(meaning(&out)["a"]["b"]["c"]["d"], json!({"k": ["v"]}));
        assert_eq!(meaning(&out)["x"], json!({"y": 1}));

        for text in [
            "[a]\n",
            "[a.b]\n",
            "[a.z]\nq = 1\n",
            "a.z = 1\n",
            "a = {}\n",
            "a = { b = {} }\n",
        ] {
            let out = upsert(text, &["a", "b", "c"], json!(1));
            assert_eq!(
                value_at(&out, &path(&["a", "b", "c"])).unwrap(),
                found(json!(1)),
                "{out:?}"
            );
        }
    }

    #[test]
    fn upsert_keeps_comments_and_sibling_order() {
        let text = "# one\nz = 1 # two\n\n# three\n[t] # four\n# five\nm = 1 # six\na = [\n  # seven\n  1,\n] # eight\n# nine\n\n# ten\n[u]\nb = 2\n";
        let out = upsert(text, &["t", "c"], json!(3));
        assert_eq!(
            out,
            "# one\nz = 1 # two\n\n# three\n[t] # four\n# five\nm = 1 # six\na = [\n  # seven\n  1,\n] # eight\nc = 3\n# nine\n\n# ten\n[u]\nb = 2\n"
        );
        assert_eq!(remove(&out, &["t", "c"]), text);
    }

    #[test]
    fn upsert_copies_the_indent_of_the_last_entry() {
        assert_eq!(
            upsert("[t]\n\tb = 2\n", &["t", "c"], json!(3)),
            "[t]\n\tb = 2\n\tc = 3\n"
        );
        assert_eq!(upsert("  a.x = 1\n", &["a", "y"], json!(2)), "  a.x = 1\n  a.y = 2\n");
    }

    #[test]
    fn upsert_keeps_crlf_and_tabs() {
        let text = "a = 1\r\n\r\n[t]\r\n\tb = 2\r\n";

        let entry = upsert(text, &["t", "c"], json!(3));
        assert_eq!(entry, "a = 1\r\n\r\n[t]\r\n\tb = 2\r\n\tc = 3\r\n");
        assert_eq!(remove(&entry, &["t", "c"]), text);

        let block = upsert(text, &["u"], json!({"v": 1, "w": 2}));
        assert_eq!(block, "a = 1\r\n\r\n[t]\r\n\tb = 2\r\n\r\n[u]\r\nv = 1\r\nw = 2\r\n");
        assert_eq!(remove(&block, &["u"]), text);

        assert_eq!(upsert("[t]\r\n", &["t", "a"], json!(1)), "[t]\r\na = 1\r\n");
    }

    #[test]
    fn upsert_keeps_a_string_value_exact_in_a_crlf_document() {
        for value in ["line\nbreak", "line\r\nbreak", "x]\n[y", "# not a comment", "tab\there"] {
            let out = upsert("[t]\r\na = 1\r\n", &["t", "s"], json!(value));
            assert_eq!(
                value_at(&out, &path(&["t", "s"])).unwrap(),
                found(json!(value)),
                "{out:?}"
            );
            assert_eq!(meaning(&out)["t"].as_object().unwrap().len(), 2, "{out:?}");
        }
    }

    #[test]
    fn remove_at_depth_leaves_siblings() {
        let text = "# c\n[a]\nb = 1\nd = 2 # keep\n\n[a.e]\nf = 3\n\n[z]\ny = 1\n";
        assert_eq!(
            remove(text, &["a", "b"]),
            "# c\n[a]\nd = 2 # keep\n\n[a.e]\nf = 3\n\n[z]\ny = 1\n"
        );
        assert_eq!(
            remove(text, &["a", "e", "f"]),
            "# c\n[a]\nb = 1\nd = 2 # keep\n\n[a.e]\n\n[z]\ny = 1\n"
        );
        assert_eq!(
            remove(text, &["a", "e"]),
            "# c\n[a]\nb = 1\nd = 2 # keep\n\n[z]\ny = 1\n"
        );
        assert_eq!(remove(text, &["a"]), "# c\n\n[z]\ny = 1\n");
        assert_eq!(remove("[a]\nb = 1\n\n[c]\nd = 2\n", &["a"]), "[c]\nd = 2\n");
        assert_eq!(remove("a.b = 1\na.c.d = 2\ne = 3\n", &["a"]), "e = 3\n");
        assert_eq!(remove("[a.b]\nc = 1\n\n[a.d]\ne = 2\n\n[z]\n", &["a"]), "[z]\n");
    }

    #[test]
    fn remove_keeps_a_comment_on_the_removed_line() {
        assert_eq!(remove("a = 1 # keep\nb = 2\n", &["a"]), "# keep\nb = 2\n");
        assert_eq!(remove("[t]\n  a = 1 # keep\n", &["t", "a"]), "[t]\n# keep\n");
    }

    #[test]
    fn remove_takes_a_multi_line_value_whole() {
        assert_eq!(
            remove("a = [\n  1, # one\n  2,\n]\nb = \"\"\"\n[x]\n\"\"\"\nc = 3\n", &["a"]),
            "b = \"\"\"\n[x]\n\"\"\"\nc = 3\n"
        );
        assert_eq!(remove("b = \"\"\"\n[x]\n\"\"\"\nc = 3\n", &["b"]), "c = 3\n");
    }

    #[test]
    fn remove_leaves_an_emptied_container_in_place() {
        assert_eq!(remove("[a]\nb = 1\n", &["a", "b"]), "[a]\n");
        assert_eq!(remove("a = { b = 1 }\n", &["a", "b"]), "a = {}\n");
        assert_eq!(
            value_at(&remove("[a]\nb = 1\n", &["a", "b"]), &path(&["a"])).unwrap(),
            found(json!({}))
        );
    }

    #[test]
    fn remove_of_an_absent_member_is_unchanged() {
        for (text, target) in [
            ("", &["a"][..]),
            ("# c\n", &["a", "b"]),
            ("[a]\nb = 1\n", &["a", "z"]),
            ("[a]\nb = 1\n", &["z", "y", "x"]),
            ("a = { b = 1 }\n", &["a", "z"]),
        ] {
            assert_eq!(
                remove_member_at(text, &path(target)).unwrap(),
                Splice::Unchanged,
                "{text:?}"
            );
        }
    }

    #[test]
    fn remove_under_a_non_container_is_unchanged() {
        for (text, target) in [
            ("a = 1\n", &["a", "b"][..]),
            ("[t]\na = [1]\n", &["t", "a", "0"]),
            ("[[w]]\nx = 1\n", &["w", "x"]),
        ] {
            assert_eq!(
                remove_member_at(text, &path(target)).unwrap(),
                Splice::Unchanged,
                "{text:?}"
            );
        }
    }

    #[test]
    fn key_with_quote_is_escaped() {
        for hostile in ["a\"b.c]\n[x]", "a\"b", "it's", "a.b", "a b"] {
            for (text, target) in [
                ("", vec![hostile]),
                ("", vec![hostile, "k"]),
                ("", vec!["t", hostile, "k"]),
                ("[t]\n", vec!["t", hostile]),
                ("d.e = 1\n", vec!["d", hostile]),
                ("a = {}\n", vec!["a", hostile]),
            ] {
                let out = upsert(text, &target, json!(1));
                assert_eq!(value_at(&out, &path(&target)).unwrap(), found(json!(1)), "{out:?}");
                assert_eq!(
                    meaning(&out).as_object().unwrap().len(),
                    1,
                    "one key at the root: {out:?}"
                );
                assert_eq!(
                    value_at(&remove(&out, &target), &path(&target)).unwrap(),
                    PointerRead::Absent,
                    "{out:?}"
                );
            }
        }
    }

    #[test]
    fn value_with_a_table_header_stays_data() {
        let value = json!("x\"]\n[pwned]\nevil = \"yes");
        for out in [
            upsert("", &["a", "b"], value.clone()),
            upsert("a = {}\n", &["a", "b"], value.clone()),
            upsert("a = \"old\"\n", &["a"], value.clone()),
            add_element("a = []\n", &["a"], value.as_str().unwrap()),
        ] {
            assert!(meaning(&out).get("pwned").is_none(), "{out:?}");
            assert!(out.contains("pwned"), "{out:?}");
        }
    }

    #[test]
    fn non_container_ancestor_is_invalid_data() {
        for (text, target) in [
            ("a = 1\n", &["a", "b"][..]),
            ("a = 1\n", &["a", "b", "c"]),
            ("[t]\na = \"x\"\n", &["t", "a", "b"]),
            ("a = [1]\n", &["a", "0"]),
            ("a = { b = 1 }\n", &["a", "b", "c"]),
            ("a.b = 1\n", &["a", "b", "c"]),
        ] {
            assert_refused(upsert_member_at(text, &path(target), &json!(1)));
        }
    }

    #[test]
    fn upsert_of_an_equal_value_is_unchanged() {
        for (text, target, value) in [
            ("a = 1\n", &["a"][..], json!(1)),
            ("a  =  [ 1,2 ]\n", &["a"], json!([1, 2])),
            ("[a]\nb = 1\nc = 2\n", &["a"], json!({"c": 2, "b": 1})),
            ("a = { b = 1 }\n", &["a"], json!({"b": 1})),
            ("a.b = 1\n", &["a"], json!({"b": 1})),
            ("[a]\n", &["a"], json!({})),
            ("a = {}\n", &["a"], json!({})),
        ] {
            assert_eq!(
                upsert_member_at(text, &path(target), &value).unwrap(),
                Splice::Unchanged,
                "{text:?}"
            );
        }
    }

    #[test]
    fn upsert_replaces_a_table_member_by_member() {
        let text = "# about t\n[t] # note\n# about a\na = 1 # keep\n# about b\nb = 2\n\n[u]\nc = 3\n";
        assert_eq!(
            upsert(text, &["t"], json!({"a": 1, "z": 9})),
            "# about t\n[t] # note\n# about a\na = 1 # keep\nz = 9\n# about b\n\n[u]\nc = 3\n"
        );
        assert_eq!(
            upsert("[t]\na = 1 # keep\n", &["t"], json!({"a": 2})),
            "[t]\na = 2 # keep\n"
        );
        assert_eq!(upsert("[t]\na = 1\n", &["t"], json!({})), "[t]\n");
        assert_eq!(upsert("[a.b]\nc = 1\n", &["a"], json!({})), "[a]\n");
    }

    #[test]
    fn upsert_of_a_scalar_over_a_table_replaces_it() {
        assert_eq!(
            upsert("[t]\na = 1\n\n[u]\nb = 2\n", &["t"], json!(5)),
            "t = 5\n[u]\nb = 2\n"
        );
        assert_eq!(upsert("x = 1\n\n[t]\na = 1\n", &["t"], json!(5)), "x = 1\nt = 5\n");
        assert_eq!(upsert("a.b = 1\n", &["a"], json!(5)), "a = 5\n");
    }

    #[test]
    fn upsert_refuses_a_null_anywhere_in_the_value() {
        for (text, value) in [
            ("", json!(null)),
            ("", json!({"k": null})),
            ("", json!([null])),
            ("a = 1\n", json!(null)),
            ("[a]\nb = 1\n", json!({"b": null})),
        ] {
            assert_refused(upsert_member_at(text, &path(&["a"]), &value));
        }
    }

    #[test]
    fn settings_ops_refuse_the_empty_path() {
        assert_refused(upsert_member_at("a = 1\n", &[], &json!(1)));
        assert_refused(remove_member_at("a = 1\n", &[]));
        assert_refused(upsert_array_element_at("a = 1\n", &[], "x"));
        assert_refused(remove_array_element_at("a = 1\n", &[], "x"));
    }

    #[test]
    fn settings_ops_refuse_an_unparseable_document_without_echoing_it() {
        let text = "token = hunter2-sentinel\n";
        let target = path(&["a"]);
        for result in [
            upsert_member_at(text, &target, &json!(1)),
            remove_member_at(text, &target),
            upsert_array_element_at(text, &target, "x"),
            remove_array_element_at(text, &target, "x"),
        ] {
            let err = result.unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData);
            assert!(!err.to_string().contains("hunter2-sentinel"), "{err}");
        }
    }

    /// A shape-agnostic check over layouts the splice has to find its way
    /// through: the meaning changes at the addressed member and nowhere else.
    #[test]
    fn splices_change_only_the_addressed_member() {
        let target = path(&["a", "new"]);
        let value = json!({"x": [1]});
        for text in [
            "[a]\n",
            "[a]\nb = 1\n",
            "a = {}\n",
            "a = { b = 1 }\n",
            "a = { b = 1, }\n",
            "a.b = 1\n",
            "a.b.c = 1\nd = 2\n",
            "[a.b]\nc = 1\n",
            "[a.b]\nc = 1\n\n[z]\nd = 1\n",
            "z = 1\n\n[a]\nb = \"#not a comment\"\n",
            "[a]\nb = [\n  1, # one\n  2,\n]\n\n[[w]]\nx = 1\n",
            "# head\n\n[a] # c\n# d\nb = 1 # e\n# f\n\n# g\n[z]\n",
            "[z]\nq = 1\n\n[a]\nb = 1\n",
            "[a.c]\nd = 1\n[z]\ne = 1\n[a.f]\ng = 1\n",
            "[a]\nb = '''\n[x]\n'''\n",
            "[a]\nb = \"\"\"\n# not a comment\n\"\"\"\n",
            "[a]\r\nb = 1\r\n",
            "[a]\nb = 1",
            "\u{feff}[a]\nb = 1\n",
            "[z]\n[[w]]\n[w.y]\nk = 1\n[a]\n",
        ] {
            let base = meaning(text);
            let mut expected = base.clone();
            expected["a"]["new"] = value.clone();

            let out = upsert(text, &["a", "new"], value.clone());
            assert_eq!(meaning(&out), expected, "upsert of {text:?} gave {out:?}");
            for comment in ["# head", "# c", "# d", "# e", "# f", "# g", "# one"] {
                assert_eq!(
                    out.matches(comment).count(),
                    text.matches(comment).count(),
                    "{comment} in {out:?}"
                );
            }

            let back = changed(remove_member_at(&out, &target).unwrap());
            assert_eq!(meaning(&back), base, "remove from {out:?} gave {back:?}");
        }
    }

    #[test]
    fn upsert_then_remove_restores_the_bytes() {
        for (text, target, value) in [
            ("", &["a"][..], json!({"b": 1})),
            ("[a]\n", &["a", "b"], json!(1)),
            ("[a]\nb = 1\n\n[z]\n", &["a", "c"], json!({"d": 1})),
            ("x = 1\n\n[z]\ny = 2\n", &["new"], json!({"k": 1})),
            ("a = {}\n", &["a", "b"], json!(1)),
            ("a = { b = 1 }\n", &["a", "c"], json!(2)),
            ("a.b = 1\n", &["a", "c"], json!(2)),
            ("[a.b]\nc = 1\n", &["a", "b", "d"], json!(2)),
        ] {
            let out = upsert(text, target, value);
            assert_eq!(remove(&out, target), text, "from {out:?}");
        }
    }

    #[test]
    fn element_upsert_is_idempotent() {
        for text in ["[a]\nb = [\"x\"]\n", "a.b = [\"y\", \"x\"]\n", "a = { b = ['x'] }\n"] {
            assert_eq!(
                upsert_array_element_at(text, &path(&["a", "b"]), "x").unwrap(),
                Splice::Unchanged,
                "{text:?}"
            );
        }
        let once = add_element("[a]\nb = [\"y\"]\n", &["a", "b"], "x");
        assert_eq!(once, "[a]\nb = [\"y\", \"x\"]\n");
        assert_eq!(
            upsert_array_element_at(&once, &path(&["a", "b"]), "x").unwrap(),
            Splice::Unchanged
        );
    }

    #[test]
    fn element_upsert_creates_the_array() {
        for (text, want) in [
            ("", "[a]\nb = [\"x\"]\n"),
            ("[a]\n", "[a]\nb = [\"x\"]\n"),
            ("[a]\nc = 1 # keep\n", "[a]\nc = 1 # keep\nb = [\"x\"]\n"),
            ("a.c = 1\n", "a.c = 1\na.b = [\"x\"]\n"),
            ("a = { c = 1 }\n", "a = { c = 1, b = [\"x\"] }\n"),
        ] {
            assert_eq!(add_element(text, &["a", "b"], "x"), want, "{text:?}");
        }
        assert_eq!(add_element("z = 1\n", &["a"], "x"), "z = 1\na = [\"x\"]\n");
    }

    #[test]
    fn element_ops_keep_array_comments() {
        let target = &["deny"];
        let text = "deny = [\n  # shell\n  \"Bash\", # never\n  # files\n  \"Write\" # last\n  # tail\n]\n";
        let comments = ["# shell", "# never", "# files", "# last", "# tail"];

        assert_eq!(
            add_element(text, target, "Read"),
            "deny = [\n  # shell\n  \"Bash\", # never\n  # files\n  \"Write\", # last\n  # tail\n  \"Read\"\n]\n",
            "a comment stays on its own element's line"
        );

        for (element, rest) in [("Bash", json!(["Write"])), ("Write", json!(["Bash"]))] {
            let out = drop_element(text, target, element);
            assert_eq!(meaning(&out)["deny"], rest, "{out:?}");
            for comment in comments {
                assert!(out.contains(comment), "{comment} in {out:?}");
            }
        }

        for (text, want) in [
            (
                "a = [\n  # only a comment\n]\n",
                "a = [\n  # only a comment\n  \"x\"\n]\n",
            ),
            ("a = [ # c\n]\n", "a = [ # c\n  \"x\"\n]\n"),
        ] {
            let out = add_element(text, &["a"], "x");
            assert_eq!(out, want);
            assert_eq!(drop_element(&out, &["a"], "x"), text);
        }
    }

    /// Removal stops at the element: dropping the emptied array is
    /// `PruneEmpty`'s step, because only the writer knows which keys are required.
    #[test]
    fn element_remove_last_leaves_empty_array() {
        for (text, want) in [
            ("[a]\nb = [\"x\"]\n", "[a]\nb = []\n"),
            ("[a]\nb = [\"x\",]\n", "[a]\nb = []\n"),
            ("[a]\nb = [ \"x\" ]\n", "[a]\nb = []\n"),
            ("[a]\nb = [\n  \"x\"\n]\n", "[a]\nb = [\n]\n"),
            ("[a]\r\nb = [\r\n\t\"x\",\r\n]\r\n", "[a]\r\nb = [\r\n]\r\n"),
            ("[a]\nb = [ # keep\n  \"x\"\n]\n", "[a]\nb = [ # keep\n]\n"),
        ] {
            let out = drop_element(text, &["a", "b"], "x");
            assert_eq!(out, want, "{text:?}");
            assert_eq!(value_at(&out, &path(&["a", "b"])).unwrap(), found(json!([])), "{out:?}");
            assert_eq!(
                remove_array_element_at(&out, &path(&["a", "b"]), "x").unwrap(),
                Splice::Unchanged,
                "{out:?}"
            );
        }
    }

    #[test]
    fn element_upsert_keeps_the_layout_of_the_array() {
        for (text, want) in [
            ("a = []\n", "a = [\"x\"]\n"),
            ("a = [ ]\n", "a = [\"x\"]\n"),
            ("a = [\n]\n", "a = [\n  \"x\"\n]\n"),
            ("a = [\"y\"]\n", "a = [\"y\", \"x\"]\n"),
            ("a = [\"y\",]\n", "a = [\"y\", \"x\",]\n"),
            ("a = [ \"y\" ]\n", "a = [ \"y\", \"x\" ]\n"),
            ("a = [1, 'y']\n", "a = [1, 'y', \"x\"]\n"),
            ("a = [\n  \"y\"\n]\n", "a = [\n  \"y\",\n  \"x\"\n]\n"),
            ("a = [\n  \"y\",\n]\n", "a = [\n  \"y\",\n  \"x\",\n]\n"),
            (
                "a = [\n    \"y\",\n    \"z\"\n  ]\n",
                "a = [\n    \"y\",\n    \"z\",\n    \"x\"\n  ]\n",
            ),
            (
                "a = [\n  \"y\" # c\n  , # d\n]\n",
                "a = [\n  \"y\" # c\n  , # d\n  \"x\",\n]\n",
            ),
            ("a = [ \"y\",\n  \"z\" ]\n", "a = [ \"y\",\n  \"z\", \"x\" ]\n"),
        ] {
            assert_eq!(add_element(text, &["a"], "x"), want, "{text:?}");
        }
    }

    #[test]
    fn element_upsert_keeps_crlf_and_tabs() {
        assert_eq!(
            add_element("a = [\r\n\t\"y\",\r\n]\r\n", &["a"], "x"),
            "a = [\r\n\t\"y\",\r\n\t\"x\",\r\n]\r\n"
        );
        assert_eq!(add_element("a = [\r\n]\r\n", &["a"], "x"), "a = [\r\n  \"x\"\r\n]\r\n");
    }

    #[test]
    fn element_splices_change_only_the_addressed_array() {
        let target = ["a", "b"];
        for text in [
            "[a]\nb = []\n",
            "[a]\nb = [\"y\"] # c\nc = [\"y\"]\n",
            "a.b = [\"y\", 1, { k = [\"x\"] }]\n",
            "a = { b = [\"y\"], c = [\"x\"] }\n",
            "z = [\"x\"]\n[a]\nb = [\n  \"y\", # one\n]\n[z2]\nb = [\"x\"]\n",
        ] {
            let base = meaning(text);
            let out = add_element(text, &target, "x");
            let mut expected = base.clone();
            expected["a"]["b"].as_array_mut().unwrap().push(json!("x"));
            assert_eq!(meaning(&out), expected, "{out:?}");
            assert_eq!(meaning(&drop_element(&out, &target, "x")), base, "{out:?}");
        }
    }

    #[test]
    fn element_remove_cuts_one_element_and_one_comma() {
        for (text, element, want) in [
            ("a = [\"x\", \"y\", \"z\"]\n", "x", "a = [\"y\", \"z\"]\n"),
            ("a = [\"x\", \"y\", \"z\"]\n", "y", "a = [\"x\", \"z\"]\n"),
            ("a = [\"x\", \"y\", \"z\"]\n", "z", "a = [\"x\", \"y\"]\n"),
            ("a = [\"x\", \"y\",]\n", "y", "a = [\"x\",]\n"),
            ("a = [\n  \"x\",\n  \"y\",\n]\n", "x", "a = [\n  \"y\",\n]\n"),
            ("a = [\n  \"x\",\n  \"y\"\n]\n", "y", "a = [\n  \"x\"\n]\n"),
            ("a = [\n  \"x\", # c\n  \"y\"\n]\n", "x", "a = [\n  # c\n  \"y\"\n]\n"),
            ("a = [\n  \"x\" # c\n  , \"y\"\n]\n", "x", "a = [\n  # c\n  \"y\"\n]\n"),
        ] {
            assert_eq!(drop_element(text, &["a"], element), want, "{text:?}");
        }
    }

    #[test]
    fn element_remove_takes_every_duplicate() {
        for (text, want) in [
            ("a = [\"x\", \"x\"]\n", "a = []\n"),
            ("a = [\"x\", \"y\", \"x\"]\n", "a = [\"y\"]\n"),
            ("a = [\"y\", 'x', \"x\", \"z\"]\n", "a = [\"y\", \"z\"]\n"),
            ("a = [\n  \"x\",\n  \"y\",\n  \"x\",\n]\n", "a = [\n  \"y\",\n]\n"),
        ] {
            assert_eq!(drop_element(text, &["a"], "x"), want, "{text:?}");
        }
    }

    #[test]
    fn element_identity_is_a_whole_string() {
        let target = path(&["a"]);
        for text in [
            "a = [\"xy\"]\n",
            "a = [1]\n",
            "a = [[\"x\"]]\n",
            "a = [{ b = \"x\" }]\n",
        ] {
            assert_eq!(
                remove_array_element_at(text, &target, "x").unwrap(),
                Splice::Unchanged,
                "{text:?}"
            );
            assert_ne!(
                upsert_array_element_at(text, &target, "x").unwrap(),
                Splice::Unchanged,
                "{text:?}"
            );
        }
    }

    #[test]
    fn element_identity_ignores_the_spelling() {
        for text in ["a = ['x']\n", "a = [\"\\u0078\"]\n", "a = [\"\"\"x\"\"\"]\n"] {
            assert_eq!(
                upsert_array_element_at(text, &path(&["a"]), "x").unwrap(),
                Splice::Unchanged,
                "{text:?}"
            );
            assert_eq!(drop_element(text, &["a"], "x"), "a = []\n", "{text:?}");
        }
        for text in ["a = ['q\"x']\n", "a = [\"q\\\"x\"]\n"] {
            assert_eq!(
                upsert_array_element_at(text, &path(&["a"]), "q\"x").unwrap(),
                Splice::Unchanged,
                "{text:?}"
            );
            assert_eq!(drop_element(text, &["a"], "q\"x"), "a = []\n", "{text:?}");
        }
    }

    #[test]
    fn element_ops_refuse_a_non_array() {
        for text in ["a = 1\n", "a = \"x\"\n", "a = { b = 1 }\n", "[a]\nb = 1\n", "a.b = 1\n"] {
            assert_refused(upsert_array_element_at(text, &path(&["a"]), "x"));
            assert_refused(remove_array_element_at(text, &path(&["a"]), "x"));
        }
    }

    #[test]
    fn element_upsert_refuses_a_non_container_ancestor() {
        for text in ["a = 1\n", "a = [\"x\"]\n", "[[a]]\nb = [\"x\"]\n"] {
            assert_refused(upsert_array_element_at(text, &path(&["a", "b"]), "x"));
        }
    }

    #[test]
    fn element_remove_under_a_non_container_is_unchanged() {
        for text in ["a = 1\n", "a = [\"x\"]\n", "[[a]]\nb = [\"x\"]\n", "", "[a]\n"] {
            assert_eq!(
                remove_array_element_at(text, &path(&["a", "b"]), "x").unwrap(),
                Splice::Unchanged,
                "{text:?}"
            );
        }
    }

    #[test]
    fn element_splices_keep_an_unrelated_array_of_tables_byte_identical() {
        let plugins = "# first\n[[ plugins ]]\nname = \"a\"\n\n[[plugins]]\nname = \"b\"\n";
        let text = format!("deny = [\"x\"]\n\n{plugins}");
        assert_eq!(
            add_element(&text, &["deny"], "y"),
            format!("deny = [\"x\", \"y\"]\n\n{plugins}")
        );
        assert_eq!(drop_element(&text, &["deny"], "x"), format!("deny = []\n\n{plugins}"));
    }

    #[test]
    fn remove_of_the_last_member_takes_a_parent_with_no_text() {
        assert_eq!(remove("[a.b]\nc = 1\n", &["a", "b"]), "");
        assert_eq!(remove("a.b = 1\nz = 2\n", &["a", "b"]), "z = 2\n");
        assert_eq!(remove("[t]\na.b = 1\n", &["t", "a", "b"]), "[t]\n");
    }

    #[test]
    fn a_byte_order_mark_stays_first() {
        assert_eq!(
            upsert("\u{feff}# head\n[t]\n", &["k"], json!(1)),
            "\u{feff}k = 1\n# head\n[t]\n"
        );
        assert_eq!(upsert("\u{feff}", &["k"], json!(1)), "\u{feff}k = 1\n");
        assert_eq!(remove("\u{feff}[t]\n\n[z]\n", &["t"]), "\u{feff}[z]\n");
    }

    #[test]
    fn interleaved_dotted_keys_keep_file_order() {
        let text = "a.x = 1\nb = 2\na.y = 3\n[t]\n";
        assert_eq!(upsert(text, &["c"], json!(4)), "a.x = 1\nb = 2\na.y = 3\nc = 4\n[t]\n");
        assert_eq!(
            upsert(text, &["a", "z"], json!(4)),
            "a.x = 1\nb = 2\na.y = 3\na.z = 4\n[t]\n"
        );
    }

    #[test]
    fn dotted_member_of_an_inline_table_goes_leaf_by_leaf() {
        let text = "t = { p.q = 1, p.r = 2, s = 3 }\n";
        assert_eq!(remove(text, &["t", "p"]), "t = { s = 3 }\n");
        assert_eq!(remove(text, &["t", "p", "r"]), "t = { p.q = 1, s = 3 }\n");
        assert_eq!(upsert(text, &["t", "p"], json!(5)), "t = { s = 3, p = 5 }\n");
    }
}
