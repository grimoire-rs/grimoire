// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Span-preserving splice edits on JSON/JSONC config text.
//!
//! Vendor MCP configs are user-owned files (`~/.claude.json` most
//! critically — Claude Code's monolithic live user-state file). A
//! parse-and-reserialize rewrite would reorder every key and drop JSONC
//! comments, so grim never does that here: a byte-offset scanner locates
//! the one managed member (`<container>.<member>`, e.g.
//! `mcpServers.grim`) and splices only that span. Every other byte of
//! the file — key order, formatting, comments — survives untouched.
//!
//! The scanner tolerates the JSONC extensions the sibling
//! [`super::json_config`] parser accepts (comments, trailing commas).
//! Content that does not scan as a JSON object is refused, never
//! rewritten — the conservative contract shared by all managed-config
//! writers.

use std::io;
use std::ops::Range;

use serde::Serialize as _;
use serde_json::ser::{PrettyFormatter, Serializer};

use super::json_config::{invalid_data, sanitize_jsonc};

/// Split a two-level RFC-6901-style pointer (`/container/member`) into its
/// `(container, member)` pair. `None` for any other shape — the splice
/// operations manage exactly one nesting level.
pub fn split_pointer(pointer: &str) -> Option<(&str, &str)> {
    let rest = pointer.strip_prefix('/')?;
    let (container, member) = rest.split_once('/')?;
    (!container.is_empty() && !member.is_empty() && !member.contains('/')).then_some((container, member))
}

/// Split an RFC 6901 pointer of any depth into its decoded segments: `~1`
/// reads `/`, `~0` reads `~`, and `""` is the whole document. `None` when the
/// text is not a pointer, a nonempty one without its leading `/` or a `~`
/// outside those two escapes.
///
/// The record stays a string so the state file keeps one spelling per piece;
/// [`split_pointer`] stays two-level and raw for MCP entries.
pub fn split_pointer_path(pointer: &str) -> Option<Vec<String>> {
    if pointer.is_empty() {
        return Some(Vec::new());
    }
    pointer.strip_prefix('/')?.split('/').map(decode_segment).collect()
}

/// Decode one pointer segment. One left-to-right pass, because `~01` must
/// read `~1`: rewriting `~0` first would turn it into `/`.
fn decode_segment(segment: &str) -> Option<String> {
    let mut decoded = String::with_capacity(segment.len());
    let mut chars = segment.chars();
    while let Some(c) = chars.next() {
        match c {
            '~' => match chars.next()? {
                '0' => decoded.push('~'),
                '1' => decoded.push('/'),
                _ => return None,
            },
            other => decoded.push(other),
        }
    }
    Some(decoded)
}

/// What [`value_at`] found at a path. Shared by the JSON and TOML readers, so
/// a caller judges a piece the same way in either format.
///
/// Closed internal enum — matches stay total, no `#[non_exhaustive]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PointerRead {
    /// The path has no value, so a write only has to create it.
    Absent,
    /// The value at the path. `null` is a value here.
    Found(serde_json::Value),
    /// The value at `ancestor`, the path's own prefix, holds a type the walk
    /// cannot enter or read, so a write would have to replace it.
    Blocked {
        /// The decoded path of the blocking value.
        ancestor: Vec<String>,
    },
}

/// Read the value at `path` in JSON/JSONC `text`, at any depth. Only objects
/// are entered: an array is a leaf, so a numeric segment never indexes one.
/// Empty text is [`PointerRead::Absent`], because the splice operations read
/// it as "no document yet".
///
/// # Errors
///
/// `InvalidData` when the text is not a JSON/JSONC object. The message never
/// carries file content.
pub fn value_at(text: &str, path: &[String]) -> io::Result<PointerRead> {
    if text.trim().is_empty() {
        return Ok(PointerRead::Absent);
    }
    let document = parse_value(text)
        .filter(serde_json::Value::is_object)
        .ok_or_else(refused)?;
    let mut current = &document;
    for (depth, segment) in path.iter().enumerate() {
        let Some(members) = current.as_object() else {
            return Ok(PointerRead::Blocked {
                ancestor: path[..depth].to_vec(),
            });
        };
        let Some(next) = members.get(segment) else {
            return Ok(PointerRead::Absent);
        };
        current = next;
    }
    Ok(PointerRead::Found(current.clone()))
}

/// What a splice did to the text.
///
/// Closed internal enum — matches stay total, no `#[non_exhaustive]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Splice {
    /// The text changed; this is the full new content.
    Changed(String),
    /// The text already matched the desired state — nothing to write.
    Unchanged,
}

/// Ensure `container.member` equals `value`, creating the container
/// object (and, for empty input, the root object) as needed. All bytes
/// outside the spliced member survive verbatim.
///
/// Values are compared semantically (parsed, not byte-wise): a member
/// whose current value equals `value` up to key order and formatting is
/// [`Splice::Unchanged`].
///
/// # Errors
///
/// `InvalidData` when the text is not a JSON/JSONC object, or the
/// existing `container` value is not an object.
pub fn upsert_member(text: &str, container: &str, member: &str, value: &serde_json::Value) -> io::Result<Splice> {
    if text.trim().is_empty() {
        // No document yet: emit the minimal pretty skeleton.
        let rendered = indent_block(&pretty(value)?, "    ");
        return Ok(Splice::Changed(format!(
            "{{\n  {}: {{\n    {}: {rendered}\n  }}\n}}\n",
            json_string(container),
            json_string(member)
        )));
    }

    // The scanner only walks structure at splice depth; a full semantic
    // parse up front guarantees grim never touches a file it cannot read
    // back (the conservative managed-config contract).
    if !parse_value(text).is_some_and(|v| v.is_object()) {
        return Err(refused());
    }
    let root = scan_object(text)?;
    let Some(container_member) = last_member(&root.members, container) else {
        // Insert the whole container as a new root member.
        let rendered = indent_block(&pretty(value)?, &deeper(&root.member_indent(text)));
        let snippet = format!(
            "{}: {{\n{inner}{}: {rendered}\n{close}}}",
            json_string(container),
            json_string(member),
            inner = deeper(&root.member_indent(text)),
            close = root.member_indent(text),
        );
        return Ok(Splice::Changed(insert_member(text, &root, &snippet)));
    };

    let inner_text = &text[container_member.value.clone()];
    if !inner_text.trim_start().starts_with('{') {
        return Err(not_an_object(container));
    }
    let inner = scan_object(inner_text)?;

    match last_member(&inner.members, member) {
        Some(existing) => {
            // Semantic compare: formatting/key-order differences are not a change.
            let current = &inner_text[existing.value.clone()];
            if parse_value(current).as_ref() == Some(value) {
                return Ok(Splice::Unchanged);
            }
            let indent = existing.key_indent(inner_text);
            let rendered = indent_block(&pretty(value)?, &indent);
            let mut out = String::with_capacity(text.len() + rendered.len());
            let base = container_member.value.start;
            out.push_str(&text[..base + existing.value.start]);
            out.push_str(&rendered);
            out.push_str(&text[base + existing.value.end..]);
            Ok(Splice::Changed(out))
        }
        None => {
            let indent = inner.member_indent_or(inner_text, &deeper(&container_member.key_indent(text)));
            let rendered = indent_block(&pretty(value)?, &indent);
            let snippet = format!("{}: {rendered}", json_string(member));
            let new_inner =
                insert_member_with_indent(inner_text, &inner, &snippet, &indent, container_member.key_indent(text));
            let mut out = String::with_capacity(text.len() + new_inner.len());
            out.push_str(&text[..container_member.value.start]);
            out.push_str(&new_inner);
            out.push_str(&text[container_member.value.end..]);
            Ok(Splice::Changed(out))
        }
    }
}

/// Remove `container.member` when present; a container emptied by the
/// removal is removed too. Absent container/member is [`Splice::Unchanged`].
///
/// # Errors
///
/// `InvalidData` when the text is not a JSON/JSONC object (callers
/// implementing tolerant removal map this themselves), or the existing
/// `container` value is not an object.
pub fn remove_member(text: &str, container: &str, member: &str) -> io::Result<Splice> {
    if text.trim().is_empty() {
        return Ok(Splice::Unchanged);
    }
    if !parse_value(text).is_some_and(|v| v.is_object()) {
        return Err(refused());
    }
    let root = scan_object(text)?;
    let Some(container_member) = last_member(&root.members, container) else {
        return Ok(Splice::Unchanged);
    };
    let inner_text = &text[container_member.value.clone()];
    if !inner_text.trim_start().starts_with('{') {
        return Err(not_an_object(container));
    }
    let inner = scan_object(inner_text)?;
    let Some(existing) = last_member(&inner.members, member) else {
        return Ok(Splice::Unchanged);
    };

    if inner.members.len() == 1 {
        // Removing the last member: drop the whole container member so an
        // emptied `"mcpServers": {}` husk is not left behind.
        let siblings = root.members.iter().map(Member::span);
        return Ok(Splice::Changed(cut_among(text, siblings, container_member.span())));
    }

    let siblings = inner.members.iter().map(Member::span);
    let new_inner_text = cut_among(inner_text, siblings, existing.span());
    Ok(Splice::Changed(splice_span(
        text,
        &container_member.value,
        &new_inner_text,
    )))
}

/// Ensure the array at the root member `key` contains the string
/// `element`, creating the array (and, for empty input, the root object) as
/// needed. All bytes outside the inserted element survive verbatim.
///
/// The array counterpart of [`upsert_member`], for a managed *list* entry
/// rather than a managed object member (OpenCode's `instructions` glob).
/// Elements are compared semantically, so an element spelled with different
/// escaping still counts as present.
///
/// # Errors
///
/// `InvalidData` when the text is not a JSON/JSONC object, or the existing
/// `key` value is not an array.
pub fn upsert_array_element(text: &str, key: &str, element: &str) -> io::Result<Splice> {
    let rendered = json_string(element);
    if text.trim().is_empty() {
        // No document yet: emit the minimal pretty skeleton.
        return Ok(Splice::Changed(format!(
            "{{\n  {name}: [\n    {rendered}\n  ]\n}}\n",
            name = json_string(key),
        )));
    }
    if !parse_value(text).is_some_and(|v| v.is_object()) {
        return Err(refused());
    }
    let root = scan_object(text)?;
    let Some(key_member) = last_member(&root.members, key) else {
        // Insert the whole array as a new root member.
        let indent = root.member_indent(text);
        let snippet = format!(
            "{name}: [\n{inner}{rendered}\n{indent}]",
            name = json_string(key),
            inner = deeper(&indent),
        );
        return Ok(Splice::Changed(insert_member(text, &root, &snippet)));
    };

    let array_text = &text[key_member.value.clone()];
    let array = scan_array(array_text, key)?;
    if array
        .elements
        .iter()
        .any(|span| element_matches(array_text, span, element))
    {
        return Ok(Splice::Unchanged);
    }
    let new_array = insert_element(array_text, &array, &rendered, &key_member.key_indent(text));
    let mut out = String::with_capacity(text.len() + new_array.len());
    out.push_str(&text[..key_member.value.start]);
    out.push_str(&new_array);
    out.push_str(&text[key_member.value.end..]);
    Ok(Splice::Changed(out))
}

/// Remove the string `element` from the array at the root member `key`;
/// an array emptied by the removal takes its whole `key` member with it (no
/// `"key": []` husk, mirroring [`remove_member`]'s emptied-container rule).
/// Absent key/element is [`Splice::Unchanged`].
///
/// # Errors
///
/// `InvalidData` when the text is not a JSON/JSONC object (callers
/// implementing tolerant removal map this themselves), or the existing
/// `key` value is not an array.
pub fn remove_array_element(text: &str, key: &str, element: &str) -> io::Result<Splice> {
    if text.trim().is_empty() {
        return Ok(Splice::Unchanged);
    }
    if !parse_value(text).is_some_and(|v| v.is_object()) {
        return Err(refused());
    }
    let root = scan_object(text)?;
    let Some(key_member) = last_member(&root.members, key) else {
        return Ok(Splice::Unchanged);
    };
    let array_text = &text[key_member.value.clone()];
    let array = scan_array(array_text, key)?;
    let Some(found) = array
        .elements
        .iter()
        .find(|span| element_matches(array_text, span, element))
    else {
        return Ok(Splice::Unchanged);
    };

    if array.elements.len() == 1 {
        let siblings = root.members.iter().map(Member::span);
        return Ok(Splice::Changed(cut_among(text, siblings, key_member.span())));
    }
    let new_array_text = cut_among(array_text, array.elements.iter().cloned(), found.clone());
    Ok(Splice::Changed(splice_span(text, &key_member.value, &new_array_text)))
}

/// `element` rendered as a JSON string literal (escaped).
fn json_string(element: &str) -> String {
    serde_json::Value::String(element.to_string()).to_string()
}

/// Whether the element at `span` parses to exactly the string `element`.
fn element_matches(text: &str, span: &Range<usize>, element: &str) -> bool {
    parse_value(&text[span.clone()]).and_then(|v| v.as_str().map(str::to_string)) == Some(element.to_string())
}

// ── Object-in-nested-array splice ────────────────────────────────────────
//
// A third managed shape, alongside the object member and the array
// element above: an object member holding an array of *group* objects,
// each keyed by the value of one scalar field and each holding its own
// array of *element* objects. In Claude's hook dialect that reads:
//
// ```jsonc
// { "hooks": { "PreToolUse": [ { "matcher": "Bash",
//                                "hooks": [ { "type": "command",
//                                             "command": "…" } ] } ] } }
// ```
//
// — but the shape is the vendor-neutral one, and every name in it is
// caller-supplied. Neither level is addressable by a key the way
// `mcpServers.<name>` is, so both are located by *value*: the group by its
// `group_key` field, the element by the semantic identity of its own
// object. That is the whole reason this is a new primitive rather than a
// parameterization of `upsert_member`.
//
// **Why this module.** The scanner internals the primitive needs
// (`scan_object`, `last_member`, `insert_member_with_indent`, `pretty`,
// `indent_block`, `parse_value`) and the `Splice` result type are all
// private here, so a sibling `hook_splice.rs` would have to widen them to
// `pub(super)` for a single consumer and would duplicate this module's
// refuse-rather-than-rewrite contract; cohesion here is by *mechanism* —
// span-preserving JSON edits — and this is that mechanism one nesting
// level deeper.
//
// **The output is armable.** Unlike an MCP entry, a spliced element can
// cause the client to execute something. This module never chooses a
// destination (it is `&str -> io::Result<Splice>`), so the control lives
// at the vendor seam: a project-scope destination must be a surface the
// client itself gitignores (threat model I1), pinned there by literal
// filename rather than by review.

/// Addresses the group array under `container.member`, and names the
/// fields each group object carries — without naming any one group.
///
/// Every field is a caller-supplied name; this module stays
/// vendor-neutral, exactly as [`upsert_member`] takes `container`/`member`
/// rather than knowing about `mcpServers`. `container` + `member` is
/// literally [`split_pointer`]'s `(container, member)` pair.
///
/// Split out of [`NestedHandlerPath`] so a caller can address *every*
/// group under one member — which is what [`owned_nested_handlers`] needs
/// and what a converge-to-a-computed-set registrar cannot express
/// otherwise.
#[derive(Debug, Clone, Copy)]
pub struct NestedGroupPath<'a> {
    /// Root member holding the group-array map (e.g. `hooks`).
    pub container: &'a str,
    /// Member of `container` whose value is the group array (e.g. `PreToolUse`).
    pub member: &'a str,
    /// Field inside each group object carrying the group's key (e.g. `matcher`).
    pub group_key: &'a str,
    /// Field inside a group object holding its element array (e.g. `hooks`).
    pub elements_key: &'a str,
}

/// Addresses one managed element inside a two-level nested array: a group
/// under [`NestedGroupPath`] selected by value, then one element inside it
/// selected by semantic identity.
#[derive(Debug, Clone, Copy)]
pub struct NestedHandlerPath<'a> {
    /// Where the group array lives and how its objects are shaped.
    pub group: NestedGroupPath<'a>,
    /// The **value** at `group.group_key` that keys the managed group.
    pub group_value: &'a str,
    /// The element-object fields that jointly identify one managed element.
    ///
    /// **Semantic identity, not string equality.** Two elements are the
    /// same registration when, for every name listed here, both objects
    /// carry that member and the two values are equal *as parsed JSON*.
    /// Byte or whole-object comparison would both be wrong, and each in a
    /// way that duplicates an entry on re-install rather than updating it:
    /// byte equality because the same string has many spellings
    /// (`"run"` == `"run"`, key order, whitespace), whole-object
    /// equality because a user may hand-add a field grim does not manage
    /// (a `timeout`, a comment-adjacent key) and that must not fork the
    /// entry grim owns.
    ///
    /// **Only name fields grim does not recompute between installs.** A
    /// field derived from `$GRIM_HOME`, from the workspace path, or from
    /// the grim binary's own location is disqualified: relocating the
    /// workspace or moving `$GRIM_HOME` changes the value, the identity
    /// stops matching the entry already on disk, and the next install
    /// inserts a *second* element beside a husk that no later run can
    /// name. Recommended shape: one stable grim-owned marker member
    /// stamped on every element grim writes, serving as **both** the
    /// identity key here and the `owner` predicate of
    /// [`owned_nested_handlers`] — which also keeps the two in agreement
    /// by construction. Whether the target client tolerates an unknown
    /// member inside such an object is a vendor question and must be
    /// answered before a literal is chosen.
    ///
    /// **The marker's *value* must be a grim constant** — not the artifact
    /// name, not the scope, not the workspace. `owned_nested_handlers`
    /// matches `owner` by exact value, so an artifact-derived marker cannot
    /// be reconstructed for an *uninstalled* artifact: its name is
    /// precisely what has already left install state, which re-opens the
    /// unreapable-registration hole this pair exists to close — while
    /// technically satisfying "stable, not path-derived". If artifact
    /// identity is ever needed inside the registration, it belongs in a
    /// *different* member that is neither an identity key nor an `owner`
    /// field. A constant marker is unambiguous as an identity key because
    /// there is at most one grim-owned element per group: the registration
    /// is the dispatcher, not one entry per hook.
    pub identity_keys: &'a [&'a str],
}

/// Ensure the element identified by `path` exists inside its group under
/// `container.member`, creating the container, the group array, the group
/// object and its element array as needed. All bytes outside the spliced
/// element survive verbatim.
///
/// Locate-or-insert runs at both levels: the group whose `group_key`
/// equals `path.group_value` (compared parsed), then within that group's
/// `elements_key` array the element matching `path.identity_keys`. A
/// located element whose value already equals `handler` (compared parsed,
/// so formatting and key order do not count) is [`Splice::Unchanged`].
///
/// **First match wins at both levels**, matching the sibling
/// [`upsert_array_element`] / [`remove_array_element`] pair rather than
/// the *last*-wins [`last_member`] discipline that governs duplicate
/// object *keys*. Nothing forbids a hand-edited file from carrying two
/// groups with the same `group_value`, or two elements with the same
/// identity; upsert and remove must agree on which one they mean, or
/// "removal undoes insert byte-for-byte" is unsatisfiable.
///
/// # Errors
///
/// `InvalidData` when the text is not a JSON/JSONC object, when
/// `container` is not an object, when the member's value is not an array,
/// when a group element is not an object, or when a group's
/// `elements_key` value is not an array — the conservative
/// refuse-rather-than-rewrite contract shared with [`upsert_member`].
///
/// Also `InvalidData` for two degenerate identities, both of which would
/// otherwise corrupt a file grim does not own rather than fail:
/// - `path.identity_keys` is empty — the match is then vacuously true and
///   the first element in the group would be adopted and overwritten,
///   including one a *user* authored;
/// - `handler` itself lacks one of `path.identity_keys` — the match is
///   then never satisfiable, so every run would insert another copy,
///   which is the exact duplicate-on-reinstall bug `identity_keys` exists
///   to prevent.
// `allow`, not `expect`, and the choice is forced rather than lax: this
// module's own tests construct these paths, so in the **test** target an
// `expect(dead_code)` is unfulfilled and rejected, while in the **bin**
// target — where nothing calls them yet — its absence is a `dead_code`
// error. `allow` is the only attribute that satisfies both, so the Stub
// phase's `expect` mechanic ends here, having already done its job.
//
// The obligation it named is still open and no attribute can carry it:
// `hook_registrar::sync_for_state` must call `owned_nested_handlers` and
// remove `owned - desired`, or a registration stays armed forever in a file
// grim does not own (D-1). `expect(dead_code)` never proved that either —
// it proves an item is reachable, never that it is consumed.
pub fn upsert_nested_handler(
    text: &str,
    path: &NestedHandlerPath<'_>,
    handler: &serde_json::Value,
) -> io::Result<Splice> {
    validate_identity(path, handler)?;
    let group = &path.group;

    // The three "nothing to locate" cases — no document, no container, no
    // member — delegate to `upsert_member` with the whole group array as the
    // value. That reuses its skeleton, its insert-a-new-container and its
    // insert-into-an-existing-container paths rather than hand-rolling a
    // second copy of all three one nesting level down.
    if text.trim().is_empty() {
        return upsert_member(text, group.container, group.member, &fresh_group_array(path, handler));
    }
    if !parse_value(text).is_some_and(|v| v.is_object()) {
        return Err(refused());
    }
    let root = scan_object(text)?;
    let Some(container_member) = last_member(&root.members, group.container) else {
        return upsert_member(text, group.container, group.member, &fresh_group_array(path, handler));
    };
    let inner_text = &text[container_member.value.clone()];
    if !inner_text.trim_start().starts_with('{') {
        return Err(not_an_object(group.container));
    }
    let inner = scan_object(inner_text)?;
    let Some(member_entry) = last_member(&inner.members, group.member) else {
        return upsert_member(text, group.container, group.member, &fresh_group_array(path, handler));
    };
    let array_text = &inner_text[member_entry.value.clone()];
    let array = scan_array(array_text, group.member)?;

    // Level 1: the group, by the parsed value at `group_key`.
    let new_array_text = match find_group(array_text, &array, group, path.group_value)? {
        None => {
            let key_indent = member_entry.key_indent(inner_text);
            let indent = element_indent(array_text, &array, &key_indent);
            let rendered = indent_block(&pretty(&new_group(path, handler))?, &indent);
            insert_element(array_text, &array, &rendered, &key_indent)
        }
        Some(group_span) => {
            let group_text = &array_text[group_span.clone()];
            // `find_group` has already parsed this span as an object, so the
            // scan cannot fail on shape — but it is what yields the spans.
            let scanned = scan_object(group_text)?;
            let group_indent = line_indent(array_text, group_span.start);

            // Level 2: the element, by `identity_keys` inside the located group.
            let new_group_text = match last_member(&scanned.members, group.elements_key) {
                None => {
                    let indent = scanned.member_indent_or(group_text, &deeper(&group_indent));
                    let array = serde_json::Value::Array(vec![handler.clone()]);
                    let rendered = indent_block(&pretty(&array)?, &indent);
                    let snippet = format!("{key}: {rendered}", key = json_string(group.elements_key));
                    insert_member_with_indent(group_text, &scanned, &snippet, &indent, group_indent)
                }
                Some(elements_entry) => {
                    let elements_text = &group_text[elements_entry.value.clone()];
                    let elements = scan_array(elements_text, group.elements_key)?;
                    let new_elements_text = match find_element(elements_text, &elements, path, handler) {
                        Some(span) if parse_value(&elements_text[span.clone()]).as_ref() == Some(handler) => {
                            return Ok(Splice::Unchanged);
                        }
                        Some(span) => {
                            let indent = line_indent(elements_text, span.start);
                            let rendered = indent_block(&pretty(handler)?, &indent);
                            splice_span(elements_text, &span, &rendered)
                        }
                        None => {
                            let key_indent = elements_entry.key_indent(group_text);
                            let indent = element_indent(elements_text, &elements, &key_indent);
                            let rendered = indent_block(&pretty(handler)?, &indent);
                            insert_element(elements_text, &elements, &rendered, &key_indent)
                        }
                    };
                    splice_span(group_text, &elements_entry.value, &new_elements_text)
                }
            };
            splice_span(array_text, &group_span, &new_group_text)
        }
    };

    let new_inner_text = splice_span(inner_text, &member_entry.value, &new_array_text);
    Ok(Splice::Changed(splice_span(
        text,
        &container_member.value,
        &new_inner_text,
    )))
}

/// Remove the element identified by `path` and `handler`, **one per
/// call** — matching [`remove_array_element`]; a caller that must
/// converge over duplicates loops, as `opencode_config` already does.
///
/// Only `path.identity_keys` are consulted to locate the element, so the
/// caller may pass the same object it would register; the remaining fields
/// are ignored. An absent container / member / group / element is
/// [`Splice::Unchanged`]. First match wins, as in
/// [`upsert_nested_handler`].
///
/// **No-husk cascade.** An emptied element array drops its whole group
/// object; an emptied group array drops the `container.member` member; and
/// the emptied `container` member is dropped from the root object, exactly
/// as [`remove_member`] drops an emptied container — the root object
/// itself always survives, possibly as `{}` or as its unmanaged siblings.
///
/// **Amended at Implement (WP-D), one level narrower than the sentence
/// above:** an emptied element array drops its group object only when that
/// group carries **nothing but `group_key` and `elements_key`** — the two
/// members grim writes when it creates a group, so such a group is
/// indistinguishable from one grim authored. A group carrying any further
/// member is one grim demonstrably did not author alone, and only the
/// `elements_key` member grim added is cut, leaving the rest untouched. The
/// unamended rule deleted a user-authored group object whenever grim had
/// added the first handler array to it, which contradicts the
/// never-delete-foreign-data paragraph below in the same breath. The
/// remaining, irreducible case is a group whose *only* authored member is
/// the matcher: it is byte-identical to a group grim creates, so no rule
/// can separate them, and the byte-for-byte guarantee for grim's own group
/// is the side that must win.
///
/// **Reversibility is "no grim-owned bytes remain", not "byte-identical
/// to pre-install".** The cascade only fires on levels the removal
/// actually emptied, so a user's sibling element inside grim's group keeps
/// that group alive and grim leaves behind a group object it created
/// holding only user content. That is deliberate — never delete foreign
/// data — and it is why the stronger definition cannot be promised. Where
/// grim is the sole occupant the full cascade does restore the file
/// byte-for-byte.
///
/// # Errors
///
/// Same shape violations and the same two degenerate-identity refusals as
/// [`upsert_nested_handler`]. Callers implementing tolerant removal map
/// these themselves.
pub fn remove_nested_handler(
    text: &str,
    path: &NestedHandlerPath<'_>,
    handler: &serde_json::Value,
) -> io::Result<Splice> {
    // Ahead of the tolerant no-ops below, deliberately: a degenerate identity is
    // a *caller* defect, not a state of the file, and the same call would be
    // refused by `upsert_nested_handler`. Reporting it only for some inputs
    // would make the pair disagree about its own contract.
    validate_identity(path, handler)?;
    let group = &path.group;
    if text.trim().is_empty() {
        return Ok(Splice::Unchanged);
    }
    if !parse_value(text).is_some_and(|v| v.is_object()) {
        return Err(refused());
    }
    let root = scan_object(text)?;
    let Some(container_member) = last_member(&root.members, group.container) else {
        return Ok(Splice::Unchanged);
    };
    let inner_text = &text[container_member.value.clone()];
    if !inner_text.trim_start().starts_with('{') {
        return Err(not_an_object(group.container));
    }
    let inner = scan_object(inner_text)?;
    let Some(member_entry) = last_member(&inner.members, group.member) else {
        return Ok(Splice::Unchanged);
    };
    let array_text = &inner_text[member_entry.value.clone()];
    let array = scan_array(array_text, group.member)?;
    let Some(group_span) = find_group(array_text, &array, group, path.group_value)? else {
        return Ok(Splice::Unchanged);
    };
    let group_text = &array_text[group_span.clone()];
    let scanned = scan_object(group_text)?;
    let Some(elements_entry) = last_member(&scanned.members, group.elements_key) else {
        return Ok(Splice::Unchanged);
    };
    let elements_text = &group_text[elements_entry.value.clone()];
    let elements = scan_array(elements_text, group.elements_key)?;
    let Some(element_span) = find_element(elements_text, &elements, path, handler) else {
        return Ok(Splice::Unchanged);
    };

    // The no-husk cascade, innermost outwards. Each level fires only when the
    // removal actually emptied it, so a user's sibling anywhere on the chain
    // keeps that level — and everything above it — alive.
    let new_array_text = if elements.elements.len() == 1 {
        // The element array is emptied. Whether that takes the group object with
        // it turns on whether the group holds anything *else* a user authored:
        // `group_key` and `elements_key` are the two members grim writes when it
        // creates a group, so a group carrying only those is indistinguishable
        // from one grim created and is dropped whole (which is what makes
        // "removal undoes insert byte-for-byte" true in the shipped case). A
        // group carrying any further member is a group grim demonstrably did not
        // author alone, so only the `elements_key` member grim added is cut.
        let grim_shaped = scanned
            .members
            .iter()
            .all(|member| member.key == group.group_key || member.key == group.elements_key);
        if !grim_shaped {
            let siblings = scanned.members.iter().map(Member::span);
            let new_group_text = cut_among(group_text, siblings, elements_entry.span());
            let new_array_text = splice_span(array_text, &group_span, &new_group_text);
            let new_inner_text = splice_span(inner_text, &member_entry.value, &new_array_text);
            return Ok(Splice::Changed(splice_span(
                text,
                &container_member.value,
                &new_inner_text,
            )));
        }
        if array.elements.len() == 1 {
            // Emptied group array: `remove_member` drops the whole
            // `container.member` member and, when that empties the container,
            // the container too — the last two levels of the cascade, already
            // implemented and already tested.
            return remove_member(text, group.container, group.member);
        }
        cut_among(array_text, array.elements.iter().cloned(), group_span.clone())
    } else {
        let siblings = elements.elements.iter().cloned();
        let new_elements_text = cut_among(elements_text, siblings, element_span.clone());
        let new_group_text = splice_span(group_text, &elements_entry.value, &new_elements_text);
        splice_span(array_text, &group_span, &new_group_text)
    };

    let new_inner_text = splice_span(inner_text, &member_entry.value, &new_array_text);
    Ok(Splice::Changed(splice_span(
        text,
        &container_member.value,
        &new_inner_text,
    )))
}

/// The parsed value of the element identified by `path` and `handler`, if
/// present.
///
/// Semantic lookup (full parse, JSONC-tolerant) — the nested analogue of
/// [`member_value`], letting a caller distinguish "an element with this
/// identity exists but differs" from "absent" before an upsert. `None` for
/// unparsable text and for either degenerate identity; the subsequent
/// [`upsert_nested_handler`] surfaces the error.
// Test-only: no production caller; the registrar reaps through
// `owned_nested_handlers` instead.
#[cfg(test)]
pub fn nested_handler_value(
    text: &str,
    path: &NestedHandlerPath<'_>,
    handler: &serde_json::Value,
) -> Option<serde_json::Value> {
    if validate_identity(path, handler).is_err() {
        return None;
    }
    let group = &path.group;
    let document = parse_value(text)?;
    let groups = document.get(group.container)?.get(group.member)?.as_array()?;
    let located = groups.iter().find(|candidate| {
        candidate.get(group.group_key).and_then(serde_json::Value::as_str) == Some(path.group_value)
    })?;
    located
        .get(group.elements_key)?
        .as_array()?
        .iter()
        .find(|element| identity_matches(element, path.identity_keys, handler))
        .cloned()
}

/// Every element under `path` that grim owns, paired with the
/// `group_key` value of the group holding it — the read keyed on
/// **ownership** rather than on identity.
///
/// An element is owned when, for every `(name, value)` in `owner`, it
/// carries that member and the two values are equal as parsed JSON. That
/// is the same comparison [`NestedHandlerPath::identity_keys`] performs,
/// against a caller-supplied *predicate* instead of against a specific
/// element — so one stable grim-owned marker member can serve as both.
///
/// **Why this exists.** Registrations are recomputed wholesale from
/// install state rather than recorded, and the desired set is
/// variable-cardinality with members derived from what is installed. After
/// an uninstall the record naming a group is already gone from state, so a
/// registrar can construct neither the `group_value` nor the element it
/// would pass to [`remove_nested_handler`] — the registration would stay
/// armed forever in a file grim does not own. This read closes that: the
/// registrar enumerates what it owns, computes what it wants, and removes
/// `owned − desired`. Enumerating rather than retaining keeps each removal
/// a single reviewable span edit and composes with the existing
/// remove-in-a-loop precedent.
///
/// Returns pairs in document order. Empty when the container, the member
/// or the group array is absent, when the text does not parse, **and when
/// `owner` is empty** — a vacuous predicate would claim every element in
/// the file, including ones a user authored, so the safe direction for a
/// removal driver is to own nothing.
pub fn owned_nested_handlers(
    text: &str,
    path: &NestedGroupPath<'_>,
    owner: &[(&str, &serde_json::Value)],
) -> Vec<(String, serde_json::Value)> {
    // A vacuous predicate would claim every element in the file, a user's
    // included — and this read drives a *removal*, so the safe direction is to
    // own nothing.
    if owner.is_empty() {
        return Vec::new();
    }
    let located = parse_value(text)
        .as_ref()
        .and_then(|document| document.get(path.container))
        .and_then(|container| container.get(path.member))
        .and_then(|member| member.as_array())
        .cloned();
    let Some(groups) = located else {
        return Vec::new();
    };

    let mut owned = Vec::new();
    for group in &groups {
        // A group whose `group_key` is absent or not a string is skipped, and a
        // grim-owned element inside one is therefore not reported. That is the
        // conservative direction rather than an oversight: the pair returned
        // here feeds `NestedHandlerPath::group_value`, which is a `&str`, so a
        // non-string group key yields a group the caller could not address for
        // removal anyway — and grim only ever writes a string one, so reaching
        // this needs a hand-edit that moved grim's marker somewhere grim never
        // put it.
        let Some(key) = group.get(path.group_key).and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(elements) = group.get(path.elements_key).and_then(serde_json::Value::as_array) else {
            continue;
        };
        for element in elements {
            if owner_matches(element, owner) {
                owned.push((key.to_string(), element.clone()));
            }
        }
    }
    owned
}

// ── Nested-splice helpers ────────────────────────────────────────────────

/// Refuse the two degenerate identities, both of which corrupt a file grim does
/// not own rather than fail. Shared by the whole nested trio so the three
/// entry points cannot disagree about what a usable identity is.
///
/// # Errors
///
/// `InvalidData` when `identity_keys` is empty (the match is vacuously true, so
/// the first element in the group — possibly a *user's* — would be adopted and
/// overwritten), or when `handler` does not carry one of them (the match is
/// never satisfiable, so every run inserts another copy).
fn validate_identity(path: &NestedHandlerPath<'_>, handler: &serde_json::Value) -> io::Result<()> {
    if path.identity_keys.is_empty() {
        return Err(invalid_data(
            "an empty identity key set matches any element, a user's included; refusing to edit".to_string(),
        ));
    }
    for key in path.identity_keys {
        if handler.get(*key).is_none() {
            return Err(invalid_data(format!(
                "the managed element carries no '{key}' member, so it could never be located again; refusing to edit"
            )));
        }
    }
    Ok(())
}

/// Whether `candidate` carries every `keys` member with the same **parsed**
/// value as `handler` — the semantic identity comparison
/// [`NestedHandlerPath::identity_keys`] documents, and the same one
/// [`owner_matches`] performs against a caller-supplied predicate.
fn identity_matches(candidate: &serde_json::Value, keys: &[&str], handler: &serde_json::Value) -> bool {
    let (Some(candidate), Some(handler)) = (candidate.as_object(), handler.as_object()) else {
        return false;
    };
    keys.iter().all(|key| match (candidate.get(*key), handler.get(*key)) {
        (Some(mine), Some(theirs)) => mine == theirs,
        _ => false,
    })
}

/// Whether `element` carries every `(name, value)` pair in `owner`.
fn owner_matches(element: &serde_json::Value, owner: &[(&str, &serde_json::Value)]) -> bool {
    let Some(element) = element.as_object() else {
        return false;
    };
    owner.iter().all(|(name, value)| element.get(*name) == Some(*value))
}

/// The **first** group in `array_text` whose `group_key` member equals
/// `group_value`, as a span into `array_text`.
///
/// First-match, matching [`upsert_array_element`] rather than the last-wins
/// [`last_member`] discipline for duplicate object *keys*: upsert and remove
/// must agree on which duplicate they mean.
///
/// # Errors
///
/// `InvalidData` when any entry of the group array is not a JSON object —
/// refuse-rather-than-rewrite, since a group array of an unexpected shape is
/// not one grim can reason about.
fn find_group(
    array_text: &str,
    array: &ScannedArray,
    group: &NestedGroupPath<'_>,
    group_value: &str,
) -> io::Result<Option<Range<usize>>> {
    for span in &array.elements {
        let parsed = parse_value(&array_text[span.clone()]);
        let Some(object) = parsed.as_ref().and_then(serde_json::Value::as_object) else {
            return Err(invalid_data(format!(
                "a '{}' entry is not a JSON object; refusing to edit",
                group.member
            )));
        };
        if object.get(group.group_key).and_then(serde_json::Value::as_str) == Some(group_value) {
            return Ok(Some(span.clone()));
        }
    }
    Ok(None)
}

/// The **first** element of `elements` matching `path.identity_keys`, as a span
/// into `elements_text`.
///
/// A non-object element simply does not match — unlike [`find_group`], which
/// refuses. Nothing here has to interpret a foreign element to do its job, and
/// leaving one alone is the tolerant direction.
fn find_element(
    elements_text: &str,
    elements: &ScannedArray,
    path: &NestedHandlerPath<'_>,
    handler: &serde_json::Value,
) -> Option<Range<usize>> {
    elements
        .elements
        .iter()
        .find(|span| {
            parse_value(&elements_text[(*span).clone()])
                .is_some_and(|candidate| identity_matches(&candidate, path.identity_keys, handler))
        })
        .cloned()
}

/// One group object holding `handler` as its only element — what grim writes
/// when it creates the group itself.
///
/// Rendered through [`pretty`] like every other value this module emits, so the
/// member order is serde's (sorted) rather than authored. Consistent with the
/// whole-entry values `upsert_member` already writes into these files.
fn new_group(path: &NestedHandlerPath<'_>, handler: &serde_json::Value) -> serde_json::Value {
    let mut group = serde_json::Map::new();
    group.insert(
        path.group.group_key.to_string(),
        serde_json::Value::String(path.group_value.to_string()),
    );
    group.insert(
        path.group.elements_key.to_string(),
        serde_json::Value::Array(vec![handler.clone()]),
    );
    serde_json::Value::Object(group)
}

/// The whole `container.member` value as grim would author it from scratch.
fn fresh_group_array(path: &NestedHandlerPath<'_>, handler: &serde_json::Value) -> serde_json::Value {
    serde_json::Value::Array(vec![new_group(path, handler)])
}

/// The indent a newly inserted element's continuation lines need, matching what
/// [`insert_element`] will do with the element itself: the existing elements'
/// indent for a populated array, and for an empty one either a level in from the
/// key (`[\n]`) or nothing at all (inline `[]`).
fn element_indent(text: &str, array: &ScannedArray, key_indent: &str) -> String {
    match array.elements.first() {
        Some(first) => line_indent(text, first.start),
        None if text[..array.close_bracket].contains('\n') => deeper(key_indent),
        None => String::new(),
    }
}

/// `text` with `span` replaced by `replacement`; every other byte survives.
fn splice_span(text: &str, span: &Range<usize>, replacement: &str) -> String {
    let mut out = String::with_capacity(text.len() + replacement.len());
    out.push_str(&text[..span.start]);
    out.push_str(replacement);
    out.push_str(&text[span.end..]);
    out
}

/// `text` with `span` deleted; every other byte survives.
fn cut_out(text: &str, span: &Range<usize>) -> String {
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..span.start]);
    out.push_str(&text[span.end..]);
    out
}

/// The shared refusal for a container whose value is not a JSON object.
fn not_an_object(container: &str) -> io::Error {
    invalid_data(format!("'{container}' is not a JSON object; refusing to edit"))
}

// ── Formatting helpers ───────────────────────────────────────────────────

/// Pretty-print `value` with serde's 2-space indentation.
fn pretty(value: &serde_json::Value) -> io::Result<String> {
    serde_json::to_string_pretty(value).map_err(|e| invalid_data(e.to_string()))
}

/// Re-indent a pretty-printed block: every line after the first gains
/// `indent` (the first line sits after `"key": ` on the member's line).
fn indent_block(rendered: &str, indent: &str) -> String {
    rendered.replace('\n', &format!("\n{indent}"))
}

/// One level deeper than `indent` (two spaces — grim's emitted style).
fn deeper(indent: &str) -> String {
    format!("{indent}  ")
}

/// Parse a (possibly JSONC) value span semantically, past a leading
/// byte-order mark, which marks the encoding and holds no content.
fn parse_value(span: &str) -> Option<serde_json::Value> {
    let span = span.strip_prefix(BOM).unwrap_or(span);
    serde_json::from_str(span)
        .ok()
        .or_else(|| serde_json::from_str(&sanitize_jsonc(span)).ok())
}

/// The parsed value of `container.member` in `text`, if present.
///
/// Semantic lookup (full parse, JSONC-tolerant) — lets the installer
/// distinguish "member exists with a different value" (a clobber) from
/// "member absent" (a plain insert) before an upsert. Returns `None` for
/// unparsable text; the subsequent [`upsert_member`] surfaces the error.
pub fn member_value(text: &str, container: &str, member: &str) -> Option<serde_json::Value> {
    Some(parse_value(text)?.get(container)?.get(member)?.clone())
}

/// The last member named `key` (JSON duplicate-key semantics: last wins,
/// matching what serde_json and every client parser resolve).
fn last_member<'m>(members: &'m [Member], key: &str) -> Option<&'m Member> {
    members.iter().rev().find(|m| m.key == key)
}

/// Insert `snippet` as a new member of the scanned root object of `text`,
/// after the last existing member (or into the empty braces).
fn insert_member(text: &str, obj: &ScannedObject, snippet: &str) -> String {
    let indent = obj.member_indent(text);
    insert_member_with_indent(text, obj, snippet, &indent, String::new())
}

/// Core insertion: `indent` prefixes the new member line; `close_indent`
/// indents the closing brace when the object was empty.
fn insert_member_with_indent(
    text: &str,
    obj: &ScannedObject,
    snippet: &str,
    indent: &str,
    close_indent: String,
) -> String {
    let mut out = String::with_capacity(text.len() + snippet.len() + 8);
    match obj.members.last() {
        Some(last) => {
            // After the last member's value (an existing trailing comma, a
            // JSONC extension, stays where it is — insertion goes first).
            out.push_str(&text[..last.value.end]);
            out.push_str(",\n");
            out.push_str(indent);
            out.push_str(snippet);
            out.push_str(&text[last.value.end..]);
        }
        None => {
            // Empty object: `{}` (trivia between the braces is preserved
            // before the inserted line).
            let insert_at = obj.close_brace;
            out.push_str(text[..insert_at].trim_end());
            out.push('\n');
            out.push_str(indent);
            out.push_str(snippet);
            out.push('\n');
            out.push_str(&close_indent);
            out.push_str(&text[insert_at..]);
        }
    }
    out
}

/// Insert `rendered` as the array's new last element, matching the array's
/// existing layout: appended on the same line for a single-line array, on
/// its own line (at the elements' indent) for a multi-line one.
/// `key_indent` indents the closing bracket when the array was empty.
fn insert_element(text: &str, array: &ScannedArray, rendered: &str, key_indent: &str) -> String {
    let mut out = String::with_capacity(text.len() + rendered.len() + 8);
    match (array.elements.first(), array.elements.last()) {
        (Some(first), Some(last)) => {
            // An empty indent means the element does not start its own line,
            // i.e. a single-line array — keep it on one line.
            let indent = line_indent(text, first.start);
            out.push_str(&text[..last.end]);
            if indent.is_empty() {
                out.push_str(", ");
            } else {
                out.push_str(",\n");
                out.push_str(&indent);
            }
            out.push_str(rendered);
            // An existing trailing comma (a JSONC extension) stays where it
            // is — the insertion goes before it, as for object members.
            out.push_str(&text[last.end..]);
        }
        _ => {
            // Empty array: `[]` stays inline, `[\n]` keeps its own line.
            let head = &text[..array.close_bracket];
            out.push_str(head.trim_end());
            if head.contains('\n') {
                out.push('\n');
                out.push_str(&deeper(key_indent));
                out.push_str(rendered);
                out.push('\n');
                out.push_str(key_indent);
            } else {
                out.push_str(rendered);
            }
            out.push_str(&text[array.close_bracket..]);
        }
    }
    out
}

/// The byte range to delete for `item`: the item itself, the `comma` beside it
/// when given, and the whitespace that would otherwise leave a blank line.
/// Shared by object members (from the key's opening quote) and array elements.
///
/// `comma` comes from the tokenizer and only whitespace separates it from
/// `item`. A byte search back from `item` takes the `,` that ends a line
/// comment instead, so the cut eats into the comment and a closing bracket on
/// the item's line lands inside it: the file stops parsing.
fn cut_range(text: &str, item: Range<usize>, comma: Option<usize>) -> Range<usize> {
    let bytes = text.as_bytes();
    let (mut start, mut end) = (item.start, item.end);
    if let Some(comma) = comma {
        start = start.min(comma);
        end = end.max(comma + 1);
    }

    // Absorb the member's own line so no blank line is left: extend start
    // back over horizontal whitespace to the line break, and end forward
    // through one line break (only when the cut both starts and ends at
    // line boundaries).
    let mut s = start;
    while s > 0 && (bytes[s - 1] == b' ' || bytes[s - 1] == b'\t') {
        s -= 1;
    }
    if s == 0 || bytes[s - 1] == b'\n' {
        let mut e = end;
        while e < bytes.len() && (bytes[e] == b' ' || bytes[e] == b'\t' || bytes[e] == b'\r') {
            e += 1;
        }
        if e < bytes.len() && bytes[e] == b'\n' {
            start = s;
            end = e + 1;
        }
    }
    start..end
}

// ── Scanner ──────────────────────────────────────────────────────────────

/// A member of a scanned object. All offsets are byte offsets into the
/// scanned text (relative to the object's own text, not the whole file).
#[derive(Debug)]
struct Member {
    /// Decoded key.
    key: String,
    /// Offset of the key's opening quote.
    key_quote: usize,
    /// Byte range of the raw value span.
    value: Range<usize>,
}

impl Member {
    /// The member's own bytes, from the key's opening quote to the value's end.
    fn span(&self) -> Range<usize> {
        self.key_quote..self.value.end
    }

    /// The whitespace prefix of the line holding the key (used to indent
    /// replacements and siblings consistently).
    fn key_indent(&self, text: &str) -> String {
        line_indent(text, self.key_quote)
    }
}

/// A scanned top-level object: its members and the offset of the closing
/// brace.
#[derive(Debug)]
struct ScannedObject {
    members: Vec<Member>,
    close_brace: usize,
}

impl ScannedObject {
    /// Indent used by this object's members (from the first member), or
    /// two spaces for an empty object at the root.
    fn member_indent(&self, text: &str) -> String {
        self.member_indent_or(text, "  ")
    }

    fn member_indent_or(&self, text: &str, fallback: &str) -> String {
        match self.members.first() {
            Some(m) => m.key_indent(text),
            None => fallback.to_string(),
        }
    }
}

/// The whitespace run between the previous newline and `at`.
fn line_indent(text: &str, at: usize) -> String {
    let bytes = text.as_bytes();
    let mut s = at;
    while s > 0 && (bytes[s - 1] == b' ' || bytes[s - 1] == b'\t') {
        s -= 1;
    }
    if s == 0 || bytes[s - 1] == b'\n' {
        text[s..at].to_string()
    } else {
        // Key does not start its own line (single-line object): no indent.
        String::new()
    }
}

/// Scan `text` as a single JSON/JSONC object and index its members.
fn scan_object(text: &str) -> io::Result<ScannedObject> {
    let mut s = Scanner::at_start(text);
    s.skip_trivia();
    if s.peek() != Some(b'{') {
        return Err(refused());
    }
    s.pos += 1;
    let mut members = Vec::new();
    loop {
        s.skip_trivia();
        match s.peek() {
            Some(b'}') => {
                let obj = ScannedObject {
                    members,
                    close_brace: s.pos,
                };
                // The document must end after the object (trivia only).
                s.pos += 1;
                s.skip_trivia();
                if s.pos != s.bytes.len() && !s.at_root_end() {
                    return Err(refused());
                }
                return Ok(obj);
            }
            Some(b'"') => {
                let key_quote = s.pos;
                let key_span = s.skip_string()?;
                let key: String = serde_json::from_str(std::str::from_utf8(&s.bytes[key_span]).map_err(|_| refused())?)
                    .map_err(|_| refused())?;
                s.skip_trivia();
                if s.peek() != Some(b':') {
                    return Err(refused());
                }
                s.pos += 1;
                s.skip_trivia();
                let value = s.skip_value()?;
                members.push(Member { key, key_quote, value });
                s.skip_trivia();
                match s.peek() {
                    Some(b',') => s.pos += 1, // trailing comma before `}` tolerated by the loop
                    Some(b'}') => {}
                    _ => return Err(refused()),
                }
            }
            _ => return Err(refused()),
        }
    }
}

/// A scanned array: the byte range of each element and the offset of the
/// closing bracket, both relative to the array's own text.
#[derive(Debug)]
struct ScannedArray {
    elements: Vec<Range<usize>>,
    close_bracket: usize,
}

/// Scan `text` as a single JSON/JSONC array and index its elements. `key`
/// names the member holding it, for the error message.
fn scan_array(text: &str, key: &str) -> io::Result<ScannedArray> {
    let not_an_array = || invalid_data(format!("'{key}' is not a JSON array; refusing to edit"));
    let mut s = Scanner::at_start(text);
    s.skip_trivia();
    if s.peek() != Some(b'[') {
        return Err(not_an_array());
    }
    s.pos += 1;
    let mut elements = Vec::new();
    loop {
        s.skip_trivia();
        match s.peek() {
            Some(b']') => {
                let array = ScannedArray {
                    elements,
                    close_bracket: s.pos,
                };
                // The span must end after the array (trivia only).
                s.pos += 1;
                s.skip_trivia();
                if !s.at_root_end() {
                    return Err(refused());
                }
                return Ok(array);
            }
            Some(_) => {
                elements.push(s.skip_value()?);
                s.skip_trivia();
                match s.peek() {
                    Some(b',') => s.pos += 1, // trailing comma before `]` tolerated by the loop
                    Some(b']') => {}
                    _ => return Err(refused()),
                }
            }
            None => return Err(refused()),
        }
    }
}

/// The byte-order mark some editors write first; it is no part of the document.
const BOM: char = '\u{feff}';

fn refused() -> io::Error {
    invalid_data("content is not a JSON object grim can edit; refusing to touch it".to_string())
}

struct Scanner<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Scanner<'a> {
    /// A scanner at the start of `text`, past a leading byte-order mark: an
    /// insert or cut before the mark would move it into the document, which
    /// no parser then reads.
    fn at_start(text: &'a str) -> Self {
        Scanner {
            bytes: text.as_bytes(),
            pos: if text.starts_with(BOM) { BOM.len_utf8() } else { 0 },
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    /// True when only trivia remains — used after the root close brace.
    fn at_root_end(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    /// Skip whitespace and JSONC comments.
    fn skip_trivia(&mut self) {
        loop {
            while self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
                self.pos += 1;
            }
            match (self.peek(), self.bytes.get(self.pos + 1).copied()) {
                (Some(b'/'), Some(b'/')) => {
                    while self.peek().is_some_and(|b| b != b'\n') {
                        self.pos += 1;
                    }
                }
                (Some(b'/'), Some(b'*')) => {
                    self.pos += 2;
                    while self.pos + 1 < self.bytes.len()
                        && !(self.bytes[self.pos] == b'*' && self.bytes[self.pos + 1] == b'/')
                    {
                        self.pos += 1;
                    }
                    self.pos = (self.pos + 2).min(self.bytes.len());
                }
                _ => return,
            }
        }
    }

    /// Skip a string literal (cursor on the opening quote); returns its
    /// span including both quotes.
    fn skip_string(&mut self) -> io::Result<Range<usize>> {
        let start = self.pos;
        debug_assert_eq!(self.peek(), Some(b'"'));
        self.pos += 1;
        while let Some(b) = self.peek() {
            self.pos += 1;
            match b {
                b'\\' => self.pos += 1, // skip the escaped byte
                b'"' => return Ok(start..self.pos),
                _ => {}
            }
        }
        Err(refused())
    }

    /// Skip one JSON value (cursor on its first byte); returns its span.
    fn skip_value(&mut self) -> io::Result<Range<usize>> {
        let start = self.pos;
        match self.peek() {
            Some(b'"') => {
                self.skip_string()?;
            }
            Some(b'{') | Some(b'[') => {
                // Bracket matching with string awareness — nesting depth
                // only; member structure is not needed at this level.
                let mut depth = 0usize;
                while let Some(b) = self.peek() {
                    match b {
                        b'"' => {
                            self.skip_string()?;
                            continue;
                        }
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => {
                            depth -= 1;
                            if depth == 0 {
                                self.pos += 1;
                                return Ok(start..self.pos);
                            }
                        }
                        b'/' => {
                            self.skip_trivia();
                            continue;
                        }
                        _ => {}
                    }
                    self.pos += 1;
                }
                return Err(refused());
            }
            Some(_) => {
                // Scalar: number / true / false / null — runs to a
                // delimiter. A `/` is one too: no scalar holds it, and a
                // comment read as part of the value goes when the value does.
                while self
                    .peek()
                    .is_some_and(|b| !b.is_ascii_whitespace() && b != b',' && b != b'}' && b != b']' && b != b'/')
                {
                    self.pos += 1;
                }
            }
            None => return Err(refused()),
        }
        Ok(start..self.pos)
    }
}

/// The line break and indent step of a document. A spliced line copies both,
/// so an edit to a CRLF or tab-indented file adds no foreign line.
struct Layout {
    newline: &'static str,
    step: String,
}

impl Layout {
    /// Read from `text`: its first line break and its root members' indent,
    /// or two spaces when they have none.
    fn of(text: &str, root: &ScannedObject) -> Self {
        let crlf = text.split_once('\n').is_some_and(|(line, _)| line.ends_with('\r'));
        let step = root.member_indent(text);
        Self {
            newline: if crlf { "\r\n" } else { "\n" },
            step: if step.is_empty() { "  ".to_string() } else { step },
        }
    }

    /// `value` pretty-printed for a member whose line starts at `indent`.
    fn render(&self, value: &serde_json::Value, indent: &str) -> io::Result<String> {
        let mut bytes = Vec::new();
        let formatter = PrettyFormatter::with_indent(self.step.as_bytes());
        value.serialize(&mut Serializer::with_formatter(&mut bytes, formatter))?;
        let line_start = format!("{}{indent}", self.newline);
        Ok(String::from_utf8_lossy(&bytes).replace('\n', &line_start))
    }

    /// `object` with `snippet` as its new last member. `indent` starts the
    /// member's line and `close_indent` the closing brace of an empty object.
    fn append(&self, object: &str, scanned: &ScannedObject, snippet: &str, indent: &str, close_indent: &str) -> String {
        let close = scanned.close_brace;
        match scanned.members.last() {
            Some(last) => self.after_last(object, last.key_quote..last.value.end, close, snippet, indent),
            None => self.on_own_line(object, close, snippet, indent, close_indent),
        }
    }

    /// `array` with the string `element` as its new last element. An empty
    /// array keeps its shape: `[]` stays on its line, `[\n]` gives it one.
    fn append_element(&self, array: &str, scanned: &ScannedArray, element: &str) -> String {
        let close = scanned.close_bracket;
        let snippet = json_string(element);
        if let Some((first, last)) = scanned.elements.first().zip(scanned.elements.last()) {
            return self.after_last(array, last.clone(), close, &snippet, &line_indent(array, first.start));
        }
        if array[..close].contains('\n') {
            let close_indent = line_indent(array, close);
            let indent = format!("{close_indent}{}", self.step);
            return self.on_own_line(array, close, &snippet, &indent, &close_indent);
        }
        let head = without_trailing_blanks(&array[..close]);
        let after_comment = if head.len() > 1 { " " } else { "" };
        format!("{head}{after_comment}{snippet}{}", &array[close..])
    }

    /// `container`, which holds no item yet, with `snippet` on a line of its
    /// own. `close` is the offset of the closing bracket.
    fn on_own_line(&self, container: &str, close: usize, snippet: &str, indent: &str, close_indent: &str) -> String {
        let newline = self.newline;
        let head = without_trailing_blanks(&container[..close]);
        let bracket = &container[close..];
        format!("{head}{newline}{indent}{snippet}{newline}{close_indent}{bracket}")
    }

    /// `container` with `snippet` after `last`, the span of the item now last.
    /// `close` is the offset of the closing bracket, and `indent` starts the
    /// item's line.
    ///
    /// The comma goes straight after the last value and the item after
    /// everything else, so a comment stays on the line of the item it
    /// describes. A trailing comma stays in style: the new item takes one.
    fn after_last(&self, container: &str, last: Range<usize>, close: usize, snippet: &str, indent: &str) -> String {
        let head = without_trailing_blanks(&container[..close]);
        let mut after_value = Scanner {
            bytes: container.as_bytes(),
            pos: last.end,
        };
        after_value.skip_trivia();
        let (comma, trailing_comma) = if after_value.peek() == Some(b',') {
            ("", ",")
        } else {
            (",", "")
        };
        // Line breaks are read from the untrimmed span: joining onto a line
        // that ends in a `//` comment would swallow the new item.
        let line_start = if container[last.start..close].contains('\n') {
            format!("{}{indent}", self.newline)
        } else {
            " ".to_string()
        };
        let (through_value, comments) = head.split_at(last.end);
        let bracket = &container[head.len()..];
        format!("{through_value}{comma}{comments}{line_start}{snippet}{trailing_comma}{bracket}")
    }
}

/// `text` without the JSON whitespace that ends it. `str::trim_end` would also
/// stop inside a `//` comment ending in U+3000, and that tail, moved past a new
/// item, leaves a document no parser reads.
fn without_trailing_blanks(text: &str) -> &str {
    text.trim_end_matches([' ', '\t', '\r', '\n'])
}

/// `value` wrapped in one single-member object per segment of `path`, outermost
/// first; `value` itself for an empty `path`.
fn nest_under(path: &[String], value: &serde_json::Value) -> serde_json::Value {
    path.iter().rev().fold(value.clone(), |inner, key| {
        serde_json::Value::Object(serde_json::Map::from_iter([(key.clone(), inner)]))
    })
}

/// The refusal for an empty path. The whole document is no member, and
/// splicing it would replace every other key in the file.
fn no_member() -> io::Error {
    invalid_data("an empty path names no member; refusing to edit".to_string())
}

/// Ensure the member at `path` equals `value`, creating each object on the way.
/// Comments, trailing commas, line endings, indentation and the order of every
/// other member survive.
///
/// Values are compared semantically, so a member already equal to `value` up
/// to key order and formatting is [`Splice::Unchanged`].
///
/// # Errors
///
/// `InvalidData` for unparseable `text`, an empty `path`, or a non-object on
/// the way, which only a caller allowed to replace it may remove first.
pub fn upsert_member_at(text: &str, path: &[String], value: &serde_json::Value) -> io::Result<Splice> {
    let Some((key, rest)) = path.split_first() else {
        return Err(no_member());
    };
    if text.trim().is_empty() {
        return Ok(Splice::Changed(format!("{}\n", pretty(&nest_under(path, value))?)));
    }
    // A full semantic parse first: the scanner trusts structure it only skims,
    // and loops forever on a bare `/` a valid document cannot hold.
    if !parse_value(text).is_some_and(|v| v.is_object()) {
        return Err(refused());
    }
    let layout = Layout::of(text, &scan_object(text)?);
    let upserted = upsert_in(text, key, rest, value, &layout, "")?;
    Ok(upserted.map_or(Splice::Unchanged, Splice::Changed))
}

/// `object` with `key`, then `rest` below it, set to `value`; `None` when it
/// already holds that value. `object_indent` starts the line of its opening
/// brace.
fn upsert_in(
    object: &str,
    key: &str,
    rest: &[String],
    value: &serde_json::Value,
    layout: &Layout,
    object_indent: &str,
) -> io::Result<Option<String>> {
    let scanned = scan_object(object)?;
    let Some(member) = last_member(&scanned.members, key) else {
        let indent = scanned.members.last().map_or_else(
            || format!("{object_indent}{}", layout.step),
            |last| last.key_indent(object),
        );
        let rendered = layout.render(&nest_under(rest, value), &indent)?;
        let snippet = format!("{}: {rendered}", json_string(key));
        return Ok(Some(layout.append(object, &scanned, &snippet, &indent, object_indent)));
    };

    let current = &object[member.value.clone()];
    let indent = member.key_indent(object);
    let replacement = match rest.split_first() {
        None if parse_value(current).as_ref() == Some(value) => return Ok(None),
        None => layout.render(value, &indent)?,
        Some(_) if !current.starts_with('{') => return Err(not_an_object(key)),
        Some((next, below)) => match upsert_in(current, next, below, value, layout, &indent)? {
            Some(inner) => inner,
            None => return Ok(None),
        },
    };
    Ok(Some(splice_span(object, &member.value, &replacement)))
}

/// Remove the member at `path`. An absent member is [`Splice::Unchanged`], and
/// so is one under a non-object, which holds nothing at `path`.
///
/// Unlike [`remove_member`], an emptied container stays: which containers a
/// harness requires is the caller's knowledge, so pruning is a separate step.
///
/// # Errors
///
/// `InvalidData` for unparseable `text` or an empty `path`.
pub fn remove_member_at(text: &str, path: &[String]) -> io::Result<Splice> {
    let Some((key, rest)) = path.split_first() else {
        return Err(no_member());
    };
    if text.trim().is_empty() {
        return Ok(Splice::Unchanged);
    }
    if !parse_value(text).is_some_and(|v| v.is_object()) {
        return Err(refused());
    }
    Ok(remove_in(text, key, rest)?.map_or(Splice::Unchanged, Splice::Changed))
}

/// `object` without `key`, or without `rest` below it; `None` when there is
/// nothing to remove.
fn remove_in(object: &str, key: &str, rest: &[String]) -> io::Result<Option<String>> {
    let scanned = scan_object(object)?;
    let Some((next, below)) = rest.split_first() else {
        // Every duplicate goes: a survivor would become the live value, since
        // the last one wins.
        let spans: Vec<_> = scanned.members.iter().map(Member::span).collect();
        return Ok(cut_items(object, &spans, |i| scanned.members[i].key == key));
    };
    let Some(member) = last_member(&scanned.members, key) else {
        return Ok(None);
    };
    let current = &object[member.value.clone()];
    if !current.starts_with('{') {
        return Ok(None);
    }
    let removed = remove_in(current, next, below)?;
    Ok(removed.map(|inner| splice_span(object, &member.value, &inner)))
}

/// `text` without the `items` (spans in file order) whose index `doomed`
/// picks; `None` when it picks none. The last goes first, so the offsets of
/// the others stay valid.
fn cut_items(text: &str, items: &[Range<usize>], doomed: impl Fn(usize) -> bool) -> Option<String> {
    (0..items.len()).rev().filter(|&i| doomed(i)).fold(None, |cut, i| {
        let before = i.checked_sub(1).map(|j| items[j].end);
        Some(cut_item(cut.as_deref().unwrap_or(text), items[i].clone(), before))
    })
}

/// `text` without `item`, one of the sibling `items` (spans in file order).
fn cut_among(text: &str, items: impl Iterator<Item = Range<usize>>, item: Range<usize>) -> String {
    let before = items.take_while(|sibling| sibling.end <= item.start).last();
    cut_item(text, item, before.map(|sibling| sibling.end))
}

/// `text` without the object member or array element at `item`, and without
/// one comma beside it. `before` ends the sibling ahead of `item`, if any.
fn cut_item(text: &str, item: Range<usize>, before: Option<usize>) -> String {
    let is_comma = |at: &usize| text.as_bytes().get(*at) == Some(&b',');
    let after = Some(trivia_end(text, item.end)).filter(is_comma);
    match after.or_else(|| before.map(|end| trivia_end(text, end)).filter(is_comma)) {
        // A comment between the item and its comma stays, so each goes on its
        // own: a comma left after the item's removal follows nothing, and the
        // file stops parsing while the op reports a change.
        Some(comma) if comma >= item.end && text[item.end..comma].contains('/') => {
            cut_out(&cut_out(text, &(comma..comma + 1)), &item)
        }
        // A comment between the comma and the item stays too. Left in place,
        // the comma gives the sibling a trailing comma a strict reader refuses.
        Some(comma) if comma < item.start && text[comma..item.start].contains('/') => {
            let rest = cut_out(text, &cut_range(text, item, None));
            cut_out(&rest, &(comma..comma + 1))
        }
        comma => cut_out(text, &cut_range(text, item, comma)),
    }
}

/// The offset of the first byte at or after `pos` that is not whitespace or
/// a comment.
fn trivia_end(text: &str, pos: usize) -> usize {
    let mut scanner = Scanner {
        bytes: text.as_bytes(),
        pos,
    };
    scanner.skip_trivia();
    scanner.pos
}

/// The span of the value at `path`, which [`value_at`] has read as present.
fn value_span(text: &str, path: &[String]) -> io::Result<Range<usize>> {
    path.iter().try_fold(0..text.len(), |span, key| {
        let scanned = scan_object(&text[span.clone()])?;
        let member = last_member(&scanned.members, key).ok_or_else(refused)?;
        Ok(span.start + member.value.start..span.start + member.value.end)
    })
}

/// Whether the empty object or array at `path` holds a comment: only blanks and
/// comments can sit between its brackets, so a `/` opens one.
///
/// # Errors
///
/// `InvalidData` for unparseable `text`, or a `path` that reads no value.
pub fn empty_container_has_comment(text: &str, path: &[String]) -> io::Result<bool> {
    let span = value_span(text, path)?;
    Ok(text[span].contains('/'))
}

/// Ensure the array at `path` holds the string `element`, creating the array
/// and each object on the way. Comments, trailing commas, line endings and the
/// array's own layout survive.
///
/// Identity is the decoded string: other escapes spell the same element, so it
/// is [`Splice::Unchanged`], and a number or a nested array never matches.
///
/// # Errors
///
/// `InvalidData` for unparseable `text`, an empty `path`, a non-array at `path`,
/// or a non-object on the way, which only a caller allowed to replace it may
/// remove first.
pub fn upsert_array_element_at(text: &str, path: &[String], element: &str) -> io::Result<Splice> {
    let Some(key) = path.last() else {
        return Err(no_member());
    };
    match value_at(text, path)? {
        PointerRead::Found(_) => {}
        // A member upsert creates what is absent and refuses what is blocked.
        PointerRead::Absent | PointerRead::Blocked { .. } => {
            return upsert_member_at(text, path, &serde_json::json!([element]));
        }
    }
    let span = value_span(text, path)?;
    let array_text = &text[span.clone()];
    let array = scan_array(array_text, key)?;
    if array
        .elements
        .iter()
        .any(|item| element_matches(array_text, item, element))
    {
        return Ok(Splice::Unchanged);
    }
    let appended = Layout::of(text, &scan_object(text)?).append_element(array_text, &array, element);
    Ok(Splice::Changed(splice_span(text, &span, &appended)))
}

/// Remove every copy of the string `element` from the array at `path`, matched
/// as [`upsert_array_element_at`] does. An absent element is
/// [`Splice::Unchanged`], and so is one under a non-object, which holds nothing
/// at `path`.
///
/// An emptied array stays `[]`: which keys a harness requires is the caller's
/// knowledge, so pruning is a separate step.
///
/// # Errors
///
/// `InvalidData` for unparseable `text`, an empty `path`, or a non-array at `path`.
pub fn remove_array_element_at(text: &str, path: &[String], element: &str) -> io::Result<Splice> {
    let Some(key) = path.last() else {
        return Err(no_member());
    };
    match value_at(text, path)? {
        PointerRead::Found(_) => {}
        PointerRead::Absent | PointerRead::Blocked { .. } => return Ok(Splice::Unchanged),
    }
    let span = value_span(text, path)?;
    let array_text = &text[span.clone()];
    let array = scan_array(array_text, key)?;
    // Every copy goes: a read finds the element wherever it sits, so a survivor
    // keeps reading as present.
    let remaining = cut_items(array_text, &array.elements, |i| {
        element_matches(array_text, &array.elements[i], element)
    });
    Ok(remaining.map_or(Splice::Unchanged, |rest| {
        Splice::Changed(splice_span(text, &span, &rest))
    }))
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn split_pointer_accepts_exactly_two_levels() {
        assert_eq!(split_pointer("/mcpServers/grim"), Some(("mcpServers", "grim")));
        assert_eq!(split_pointer("/mcp/my-server"), Some(("mcp", "my-server")));
        // Amp's literal dotted container key is ONE segment (a `.` is not a
        // JSON Pointer separator), not a nested `/amp/mcpServers/` pointer.
        assert_eq!(split_pointer("/amp.mcpServers/grim"), Some(("amp.mcpServers", "grim")));
        for bad in ["mcpServers/grim", "/mcpServers", "/a/b/c", "//x", "/a/", ""] {
            assert_eq!(split_pointer(bad), None, "input: {bad}");
        }
    }

    #[test]
    fn upsert_into_amp_dotted_container_preserves_sibling_and_comment() {
        // Amp's container is the literal dotted key `"amp.mcpServers"` (a `.`
        // is not a JSON Pointer separator, so this is ONE key, not a nested
        // `amp` → `mcpServers`). A user's existing sibling server and a JSONC
        // comment both survive the splice; the grim entry lands under the
        // dotted key.
        let text = "{\n  // user config\n  \"amp.mcpServers\": {\n    \"other\": {\"command\": \"x\"}\n  }\n}\n";
        let out = changed(upsert_member(text, "amp.mcpServers", "grim", &json!({"command": "grim"})).unwrap());
        assert!(out.contains("// user config"), "comment preserved: {out}");
        assert!(
            out.contains("\"other\": {\"command\": \"x\"}"),
            "sibling server preserved: {out}"
        );
        let doc: serde_json::Value = serde_json::from_str(&crate::install::json_config::sanitize_jsonc(&out)).unwrap();
        assert_eq!(doc["amp.mcpServers"]["grim"]["command"], "grim");
        assert_eq!(doc["amp.mcpServers"]["other"]["command"], "x", "sibling still readable");
    }

    #[test]
    fn upsert_into_empty_text_creates_skeleton() {
        let out =
            changed(upsert_member("", "mcpServers", "grim", &json!({"command": "grim", "args": ["mcp"]})).unwrap());
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["mcpServers"]["grim"]["command"], "grim");
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn upsert_escapes_a_member_name_on_every_insert_path() {
        // A hand-edited lock can name an mcp `a"b` (`is_contained_mcp_name`
        // admits it): the key must stay one JSON string, never break out.
        let name = "a\"b\u{1}";
        let value = json!({"command": "x"});
        for text in ["", "{}\n", "{\n  \"mcpServers\": {\n    \"other\": {}\n  }\n}\n"] {
            let out = changed(upsert_member(text, "mcpServers", name, &value).unwrap());
            let parsed: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"));
            assert_eq!(parsed["mcpServers"][name], value, "{out}");
            assert_eq!(
                upsert_member(&out, "mcpServers", name, &value).unwrap(),
                Splice::Unchanged
            );
        }
    }

    #[test]
    fn upsert_inserts_container_preserving_every_other_byte() {
        let text = "{\n  \"zeta\": 1,\n  \"alpha\": {\"deep\": [1, 2]}\n}\n";
        let out = changed(upsert_member(text, "mcpServers", "grim", &json!({"command": "grim"})).unwrap());
        // Original content survives verbatim (key order intact, no reflow).
        assert!(out.contains("\"zeta\": 1"));
        assert!(out.contains("\"alpha\": {\"deep\": [1, 2]}"));
        assert!(
            out.find("\"zeta\"").unwrap() < out.find("\"alpha\"").unwrap(),
            "key order preserved"
        );
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["mcpServers"]["grim"]["command"], "grim");
    }

    #[test]
    fn upsert_inserts_member_into_existing_container() {
        let text = "{\n  \"mcpServers\": {\n    \"other\": {\"command\": \"x\"}\n  },\n  \"theme\": \"dark\"\n}\n";
        let out = changed(upsert_member(text, "mcpServers", "grim", &json!({"command": "grim"})).unwrap());
        assert!(
            out.contains("\"other\": {\"command\": \"x\"}"),
            "sibling server untouched"
        );
        assert!(out.contains("\"theme\": \"dark\""));
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["mcpServers"]["grim"]["command"], "grim");
        assert_eq!(doc["mcpServers"]["other"]["command"], "x");
    }

    #[test]
    fn upsert_replaces_only_the_member_value() {
        let text = "{\n  \"mcpServers\": {\n    \"grim\": {\"command\": \"old\"},\n    \"other\": {\"command\": \"x\"}\n  }\n}\n";
        let out = changed(upsert_member(text, "mcpServers", "grim", &json!({"command": "new"})).unwrap());
        assert!(out.contains("\"other\": {\"command\": \"x\"}"));
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["mcpServers"]["grim"]["command"], "new");
    }

    #[test]
    fn upsert_identical_value_is_unchanged_despite_formatting() {
        // Same semantic value, different key order and spacing.
        let text = "{\n  \"mcpServers\": {\n    \"grim\": {\n      \"args\":   [\"mcp\"],\n      \"command\": \"grim\"\n    }\n  }\n}\n";
        let value = json!({"command": "grim", "args": ["mcp"]});
        assert_eq!(
            upsert_member(text, "mcpServers", "grim", &value).unwrap(),
            Splice::Unchanged
        );
    }

    #[test]
    fn comments_and_trailing_commas_survive_outside_the_splice() {
        let text = "{\n  // user comment\n  \"theme\": \"dark\",\n  \"mcpServers\": {\n    \"grim\": {\"command\": \"old\"},\n  },\n}\n";
        let out = changed(upsert_member(text, "mcpServers", "grim", &json!({"command": "new"})).unwrap());
        assert!(out.contains("// user comment"), "comments preserved");
        assert!(out.contains("\"theme\": \"dark\""));
        assert!(out.contains("\"command\": \"new\""));
    }

    #[test]
    fn remove_member_preserves_siblings() {
        let text = "{\n  \"mcpServers\": {\n    \"grim\": {\"command\": \"grim\"},\n    \"other\": {\"command\": \"x\"}\n  },\n  \"theme\": \"dark\"\n}\n";
        let out = changed(remove_member(text, "mcpServers", "grim").unwrap());
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(doc["mcpServers"].get("grim").is_none());
        assert_eq!(doc["mcpServers"]["other"]["command"], "x");
        assert_eq!(doc["theme"], "dark");
        assert!(!out.contains("\n\n  "), "no blank line left behind: {out:?}");
    }

    #[test]
    fn remove_last_member_drops_the_container() {
        let text = "{\n  \"theme\": \"dark\",\n  \"mcpServers\": {\n    \"grim\": {\"command\": \"grim\"}\n  }\n}\n";
        let out = changed(remove_member(text, "mcpServers", "grim").unwrap());
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(doc.get("mcpServers").is_none(), "emptied container removed: {out}");
        assert_eq!(doc["theme"], "dark");
    }

    #[test]
    fn remove_first_member_keeps_valid_json() {
        let text = "{\n  \"mcpServers\": {\n    \"grim\": {\"command\": \"grim\"},\n    \"other\": {\"command\": \"x\"}\n  }\n}\n";
        let out = changed(remove_member(text, "mcpServers", "grim").unwrap());
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["mcpServers"]["other"]["command"], "x");
    }

    /// A comment on either side of a separating comma, and the comment. The
    /// second touches a scalar, which the scanner ends at the `/`.
    const COMMA_COMMENTS: [(&str, &str); 5] = [
        (" /* k */, ", "/* k */"),
        ("/* k */, ", "/* k */"),
        (", /* k */ ", "/* k */"),
        (" // k\n  , ", "// k"),
        (", // k\n  ", "// k"),
    ];

    /// `items` joined by `", "`, except the gap after item `at`, which is `sep`.
    fn joined(items: &[&str], at: usize, sep: &str) -> String {
        let mut out = String::new();
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                out.push_str(if i - 1 == at { sep } else { ", " });
            }
            out.push_str(item);
        }
        out
    }

    /// `out` read as strict JSON once `comment` is gone, so a comma left
    /// leading, dangling or trailing fails here even where JSONC tolerates it.
    fn strict_without(out: &str, comment: &str) -> serde_json::Value {
        serde_json::from_str(&out.replace(comment, "")).unwrap_or_else(|e| panic!("{e}: {out:?}"))
    }

    /// The cut must find the separating comma past a comment on either side of
    /// it, or the file is left with a comma no parser reads.
    #[test]
    fn remove_member_beside_a_comment_keeps_the_file_readable() {
        let members = [("a", "\"a\": 1"), ("b", "\"b\": 2"), ("x", "\"x\": 3")];
        let items = members.map(|(_, item)| item);
        for (sep, comment) in COMMA_COMMENTS {
            for at in 0..2 {
                let text = format!("{{\"mcp\": {{{}}}}}", joined(&items, at, sep));
                for (key, _) in members {
                    let out = changed(remove_member(&text, "mcp", key).unwrap());
                    let mut rest = json!({"mcp": {"a": 1, "b": 2, "x": 3}});
                    rest["mcp"].as_object_mut().unwrap().remove(key);
                    assert_eq!(parsed(&out), rest, "{key} from {text:?} gave {out:?}");
                    assert_eq!(strict_without(&out, comment), rest, "{key} from {text:?}");
                    assert!(out.contains(comment), "{key} from {text:?} gave {out:?}");
                }
            }
        }

        // The last member takes its container, which is itself a member.
        let container = "\"mcp\": {\"a\": 1}";
        for items in [
            [container, "\"y\": 0", "\"z\": 0"],
            ["\"y\": 0", container, "\"z\": 0"],
            ["\"y\": 0", "\"z\": 0", container],
        ] {
            for (sep, comment) in COMMA_COMMENTS {
                for at in 0..2 {
                    let text = format!("{{{}}}", joined(&items, at, sep));
                    let out = changed(remove_member(&text, "mcp", "a").unwrap());
                    assert_eq!(parsed(&out), json!({"y": 0, "z": 0}), "{text:?} gave {out:?}");
                    assert_eq!(strict_without(&out, comment), json!({"y": 0, "z": 0}), "{text:?}");
                    assert!(out.contains(comment), "{text:?} gave {out:?}");
                }
            }
        }
    }

    /// Without a comment every removal cuts exactly what it cut before comments
    /// were handled, so no comment-free file changes shape.
    #[test]
    fn remove_without_a_comment_cuts_the_same_bytes() {
        let pretty = "{\n  \"mcp\": {\n    \"a\": 1,\n    \"b\": 2,\n    \"x\": 3\n  }\n}\n";
        let compact = "{\"mcp\": {\"a\": 1, \"b\": 2, \"x\": 3}}";
        for (text, key, out) in [
            (pretty, "a", "{\n  \"mcp\": {\n    \"b\": 2,\n    \"x\": 3\n  }\n}\n"),
            (pretty, "b", "{\n  \"mcp\": {\n    \"a\": 1,\n    \"x\": 3\n  }\n}\n"),
            (pretty, "x", "{\n  \"mcp\": {\n    \"a\": 1,\n    \"b\": 2\n  }\n}\n"),
            (compact, "a", "{\"mcp\": { \"b\": 2, \"x\": 3}}"),
            (compact, "b", "{\"mcp\": {\"a\": 1,  \"x\": 3}}"),
            (compact, "x", "{\"mcp\": {\"a\": 1, \"b\": 2}}"),
        ] {
            assert_eq!(
                changed(remove_member(text, "mcp", key).unwrap()),
                out,
                "{key} from {text:?}"
            );
            let at = remove_member_at(text, &path(&["mcp", key])).unwrap();
            assert_eq!(changed(at), out, "{key} at depth from {text:?}");
        }
        for (text, out) in [
            ("{\"y\": 0, \"mcp\": {\"a\": 1}}", "{\"y\": 0}"),
            ("{\"mcp\": {\"a\": 1}, \"y\": 0}", "{ \"y\": 0}"),
            (
                "{\n  \"y\": 0,\n  \"mcp\": {\n    \"a\": 1\n  },\n  \"z\": 0\n}\n",
                "{\n  \"y\": 0,\n  \"z\": 0\n}\n",
            ),
        ] {
            assert_eq!(changed(remove_member(text, "mcp", "a").unwrap()), out, "{text:?}");
        }

        let pretty = "{\n  \"k\": [\n    \"a\",\n    \"b\",\n    \"x\"\n  ]\n}\n";
        let compact = "{\"k\": [\"a\", \"b\", \"x\"]}";
        for (text, element, out) in [
            (pretty, "a", "{\n  \"k\": [\n    \"b\",\n    \"x\"\n  ]\n}\n"),
            (pretty, "b", "{\n  \"k\": [\n    \"a\",\n    \"x\"\n  ]\n}\n"),
            (pretty, "x", "{\n  \"k\": [\n    \"a\",\n    \"b\"\n  ]\n}\n"),
            (compact, "a", "{\"k\": [ \"b\", \"x\"]}"),
            (compact, "b", "{\"k\": [\"a\",  \"x\"]}"),
            (compact, "x", "{\"k\": [\"a\", \"b\"]}"),
        ] {
            let removed = remove_array_element(text, "k", element).unwrap();
            assert_eq!(changed(removed), out, "{element} from {text:?}");
            let at = remove_array_element_at(text, &path(&["k"]), element).unwrap();
            assert_eq!(changed(at), out, "{element} at depth from {text:?}");
        }
        for (text, out) in [
            ("{\"y\": 0, \"k\": [\"a\"]}", "{\"y\": 0}"),
            ("{\"k\": [\"a\"], \"y\": 0}", "{ \"y\": 0}"),
        ] {
            assert_eq!(changed(remove_array_element(text, "k", "a").unwrap()), out, "{text:?}");
        }
    }

    #[test]
    fn remove_absent_is_unchanged() {
        let text = "{\"mcpServers\": {\"other\": {}}}";
        assert_eq!(remove_member(text, "mcpServers", "grim").unwrap(), Splice::Unchanged);
        assert_eq!(remove_member("{}", "mcpServers", "grim").unwrap(), Splice::Unchanged);
        assert_eq!(remove_member("", "mcpServers", "grim").unwrap(), Splice::Unchanged);
    }

    #[test]
    fn malformed_or_non_object_input_is_refused() {
        for bad in ["not json {{{", "[1, 2]", "42", "{\"a\": }"] {
            let err = upsert_member(bad, "mcpServers", "grim", &json!({})).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "input: {bad}");
        }
        // Container present but not an object.
        let err = upsert_member("{\"mcpServers\": []}", "mcpServers", "grim", &json!({})).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let err = remove_member("{\"mcpServers\": 3}", "mcpServers", "grim").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn escaped_and_unicode_strings_scan_correctly() {
        let text = "{\n  \"we\\\"ird\": \"va{lue\",\n  \"emoji\": \"🧙 // not a comment\",\n  \"mcpServers\": {}\n}\n";
        let out = changed(upsert_member(text, "mcpServers", "grim", &json!({"command": "grim"})).unwrap());
        assert!(out.contains("\"we\\\"ird\": \"va{lue\""));
        assert!(out.contains("🧙 // not a comment"));
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["mcpServers"]["grim"]["command"], "grim");
    }

    #[test]
    fn duplicate_keys_edit_the_last_occurrence() {
        // JSON duplicate-key semantics: parsers keep the last value, so the
        // splice must edit the one that wins.
        let text =
            "{\"mcpServers\": {\"grim\": {\"command\": \"a\"}}, \"mcpServers\": {\"grim\": {\"command\": \"b\"}}}";
        let out = changed(upsert_member(text, "mcpServers", "grim", &json!({"command": "c"})).unwrap());
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["mcpServers"]["grim"]["command"], "c");
        assert!(
            out.contains("{\"command\": \"a\"}"),
            "first (losing) occurrence untouched"
        );
    }

    #[test]
    fn upsert_is_idempotent_through_a_round_trip() {
        let value = json!({"command": "grim", "args": ["mcp"], "env": {"A": "${A}"}});
        let first = changed(upsert_member("", "mcpServers", "grim", &value).unwrap());
        assert_eq!(
            upsert_member(&first, "mcpServers", "grim", &value).unwrap(),
            Splice::Unchanged
        );
        // Remove → re-add round-trips to valid JSON.
        let removed = changed(remove_member(&first, "mcpServers", "grim").unwrap());
        let re_added = changed(upsert_member(&removed, "mcpServers", "grim", &value).unwrap());
        let doc: serde_json::Value = serde_json::from_str(&re_added).unwrap();
        assert_eq!(doc["mcpServers"]["grim"]["command"], "grim");
    }

    #[test]
    fn realistic_claude_json_only_touches_the_managed_span() {
        // A ~/.claude.json-shaped document: many foreign top-level keys.
        let text = concat!(
            "{\n",
            "  \"numStartups\": 42,\n",
            "  \"tipsHistory\": {\"tip-a\": 3, \"tip-b\": 9},\n",
            "  \"projects\": {\n",
            "    \"/home/u/dev/x\": {\"allowedTools\": [], \"history\": [{\"display\": \"hi\"}]}\n",
            "  },\n",
            "  \"mcpServers\": {\n",
            "    \"user-server\": {\"type\": \"http\", \"url\": \"https://x/mcp\"}\n",
            "  }\n",
            "}\n"
        );
        let out =
            changed(upsert_member(text, "mcpServers", "grim", &json!({"command": "grim", "args": ["mcp"]})).unwrap());
        // Everything outside mcpServers is byte-identical.
        let prefix_end = text.find("\"mcpServers\"").unwrap();
        assert_eq!(&out[..prefix_end], &text[..prefix_end], "prefix bytes untouched");
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["mcpServers"]["user-server"]["url"], "https://x/mcp");
        assert_eq!(doc["mcpServers"]["grim"]["args"][0], "mcp");
        assert_eq!(doc["numStartups"], 42);

        // And removal restores the original byte-for-byte.
        let back = changed(remove_member(&out, "mcpServers", "grim").unwrap());
        assert_eq!(back, text, "remove undoes upsert exactly");
    }

    // ── Array-element splice (OpenCode's managed `instructions` glob) ─────
    //
    // Same contract as the object-member splice above: span-preserving,
    // semantic presence-detection, tolerant no-op removal.

    #[test]
    fn array_upsert_into_empty_text_creates_the_document() {
        let out = changed(upsert_array_element("", "instructions", "a.md").unwrap());
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["instructions"], json!(["a.md"]));
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn array_upsert_preserves_comments_key_order_and_formatting() {
        let text = concat!(
            "{\n",
            "  // which model\n",
            "  \"model\":   \"a/b\",\n",
            "  \"instructions\": [\n",
            "    \"CONTRIBUTING.md\"\n",
            "  ],\n",
            "  \"alpha\": 1\n",
            "}\n",
        );
        let out = changed(upsert_array_element(text, "instructions", "g.md").unwrap());
        assert!(out.contains("// which model"), "comment preserved: {out}");
        assert!(out.contains("\"model\":   \"a/b\""), "formatting preserved: {out}");
        assert!(
            out.find("\"model\"") < out.find("\"alpha\""),
            "authored key order preserved: {out}"
        );
        assert!(out.contains("\"CONTRIBUTING.md\""), "sibling element preserved: {out}");
        assert!(out.contains("\"g.md\""), "managed element added: {out}");

        let back = changed(remove_array_element(&out, "instructions", "g.md").unwrap());
        assert_eq!(back, text, "remove undoes upsert exactly");
    }

    #[test]
    fn array_upsert_creates_an_absent_array_and_keeps_siblings() {
        let text = "{\n  \"model\": \"a/b\"\n}\n";
        let out = changed(upsert_array_element(text, "instructions", "g.md").unwrap());
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["model"], "a/b");
        assert_eq!(doc["instructions"], json!(["g.md"]));

        let back = changed(remove_array_element(&out, "instructions", "g.md").unwrap());
        assert_eq!(back, text, "remove undoes upsert exactly");
    }

    #[test]
    fn array_upsert_into_an_empty_array_keeps_its_layout() {
        for text in ["{\n  \"instructions\": []\n}\n", "{\n  \"instructions\": [\n  ]\n}\n"] {
            let out = changed(upsert_array_element(text, "instructions", "g.md").unwrap());
            let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
            assert_eq!(doc["instructions"], json!(["g.md"]), "input: {text:?} → {out}");
        }
    }

    #[test]
    fn array_upsert_appends_inline_for_a_single_line_array() {
        let text = "{\"instructions\": [\"a.md\", \"b.md\"], \"model\": \"x\"}";
        let out = changed(upsert_array_element(text, "instructions", "g.md").unwrap());
        assert_eq!(
            out,
            "{\"instructions\": [\"a.md\", \"b.md\", \"g.md\"], \"model\": \"x\"}"
        );

        let back = changed(remove_array_element(&out, "instructions", "g.md").unwrap());
        assert_eq!(back, text, "remove undoes upsert exactly");
    }

    #[test]
    fn array_upsert_is_idempotent_and_matches_semantically() {
        let text = "{\n  \"instructions\": [\n    \"g.md\"\n  ]\n}\n";
        assert_eq!(
            upsert_array_element(text, "instructions", "g.md").unwrap(),
            Splice::Unchanged
        );
        // Escaped spelling of the same string still counts as present.
        let escaped = "{\n  \"instructions\": [\n    \"g\\u002em\\u0064\"\n  ]\n}\n";
        assert_eq!(
            upsert_array_element(escaped, "instructions", "g.md").unwrap(),
            Splice::Unchanged
        );
    }

    #[test]
    fn array_remove_last_element_drops_the_whole_member() {
        let text = "{\n  \"model\": \"a/b\",\n  \"instructions\": [\n    \"g.md\"\n  ]\n}\n";
        let out = changed(remove_array_element(text, "instructions", "g.md").unwrap());
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(doc.get("instructions").is_none(), "no empty-array husk left: {out}");
        assert_eq!(doc["model"], "a/b");
    }

    #[test]
    fn array_remove_absent_key_or_element_is_a_tolerant_no_op() {
        assert_eq!(
            remove_array_element("", "instructions", "g.md").unwrap(),
            Splice::Unchanged
        );
        assert_eq!(
            remove_array_element("{\"model\": \"x\"}", "instructions", "g.md").unwrap(),
            Splice::Unchanged
        );
        assert_eq!(
            remove_array_element("{\"instructions\": [\"a.md\"]}", "instructions", "g.md").unwrap(),
            Splice::Unchanged
        );
    }

    #[test]
    fn array_splice_refuses_a_non_array_value_and_unparsable_text() {
        for (text, key) in [
            ("{\"instructions\": \"x\"}", "instructions"),
            ("{\"instructions\": {}}", "instructions"),
        ] {
            let err = upsert_array_element(text, key, "g.md").unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "input: {text}");
            assert!(err.to_string().contains("not a JSON array"), "input: {text}");
            let err = remove_array_element(text, key, "g.md").unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "input: {text}");
        }
        let err = upsert_array_element("not json {{{", "instructions", "g.md").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn array_element_needing_escaping_round_trips() {
        let element = "C:\\Users\\dev\\rules\\*.md";
        let out = changed(upsert_array_element("", "instructions", element).unwrap());
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["instructions"][0], element);
        assert_eq!(
            upsert_array_element(&out, "instructions", element).unwrap(),
            Splice::Unchanged,
            "an escaped element is detected as present on re-upsert"
        );
    }

    #[test]
    fn array_splice_tolerates_jsonc_trailing_commas() {
        let text = "{\n  // c\n  \"instructions\": [\n    \"a.md\",\n  ],\n}\n";
        let out = changed(upsert_array_element(text, "instructions", "g.md").unwrap());
        assert!(out.contains("// c"), "comment preserved: {out}");
        let doc: serde_json::Value = serde_json::from_str(&super::sanitize_jsonc(&out)).unwrap();
        assert_eq!(doc["instructions"], json!(["a.md", "g.md"]));

        let back = changed(remove_array_element(&out, "instructions", "g.md").unwrap());
        assert_eq!(back, text, "remove undoes upsert exactly");
    }

    /// The cut must find the separating comma past a comment on either side of
    /// it, or the file is left with a comma no parser reads.
    #[test]
    fn array_remove_beside_a_comment_keeps_the_file_readable() {
        let items = ["\"a\"", "\"b\"", "\"x\""];
        for (sep, comment) in COMMA_COMMENTS {
            for at in 0..2 {
                let text = format!("{{\"k\": [{}]}}", joined(&items, at, sep));
                for element in ["a", "b", "x"] {
                    let out = changed(remove_array_element(&text, "k", element).unwrap());
                    let rest: Vec<_> = ["a", "b", "x"].into_iter().filter(|e| *e != element).collect();
                    assert_eq!(parsed(&out)["k"], json!(rest), "{element} from {text:?} gave {out:?}");
                    assert_eq!(
                        strict_without(&out, comment)["k"],
                        json!(rest),
                        "{element} from {text:?}"
                    );
                    assert!(out.contains(comment), "{element} from {text:?} gave {out:?}");
                }
            }
        }

        // The last element takes its array, which is itself a member.
        let array = "\"k\": [\"a\"]";
        for items in [
            [array, "\"y\": 0", "\"z\": 0"],
            ["\"y\": 0", array, "\"z\": 0"],
            ["\"y\": 0", "\"z\": 0", array],
        ] {
            for (sep, comment) in COMMA_COMMENTS {
                for at in 0..2 {
                    let text = format!("{{{}}}", joined(&items, at, sep));
                    let out = changed(remove_array_element(&text, "k", "a").unwrap());
                    assert_eq!(parsed(&out), json!({"y": 0, "z": 0}), "{text:?} gave {out:?}");
                    assert_eq!(strict_without(&out, comment), json!({"y": 0, "z": 0}), "{text:?}");
                    assert!(out.contains(comment), "{text:?} gave {out:?}");
                }
            }
        }
    }

    // ── Key escaping (GitHub #56) ────────────────────────────────────────
    //
    // The reachable hostile input is a `grimoire.toml` **binding key**, which
    // becomes `artifact.name` and then the managed `member`. TOML permits a
    // quoted key carrying `"`, `\` or a control character and nothing between
    // that file and this module rejects one, so every key is escaped here
    // regardless of what any layer above claims to constrain.

    /// The three escapable classes, as a binding name would carry them.
    const HOSTILE_NAMES: [&str; 4] = [
        "a\"b",
        "a\\b",
        "a\u{0001}b",
        // The shape that would close the string literal and inject structure.
        "x\", \"injected\": {\"command\": \"payload\"}, \"y\": \"",
    ];

    #[test]
    fn hostile_member_name_survives_every_upsert_site() {
        for name in HOSTILE_NAMES {
            // Site 1: the empty-text skeleton.
            let skeleton = changed(upsert_member("", "mcpServers", name, &json!({"command": "grim"})).unwrap());
            let doc: serde_json::Value = serde_json::from_str(&skeleton).unwrap_or_else(|e| {
                panic!("skeleton for {name:?} is not valid JSON ({e}): {skeleton}");
            });
            assert_eq!(doc["mcpServers"][name]["command"], "grim", "name: {name:?}");
            assert_eq!(
                doc["mcpServers"].as_object().map(serde_json::Map::len),
                Some(1),
                "exactly one member, nothing injected: {skeleton}"
            );

            // Site 2: inserting the whole container into an existing document.
            let text = "{\n  \"theme\": \"dark\"\n}\n";
            let inserted = changed(upsert_member(text, "mcpServers", name, &json!({"command": "grim"})).unwrap());
            let doc: serde_json::Value = serde_json::from_str(&inserted).unwrap();
            assert_eq!(doc["mcpServers"][name]["command"], "grim", "name: {name:?}");
            assert_eq!(doc["theme"], "dark");

            // Site 3: inserting into an existing container.
            let text = "{\n  \"mcpServers\": {\n    \"other\": {}\n  }\n}\n";
            let inserted = changed(upsert_member(text, "mcpServers", name, &json!({"command": "grim"})).unwrap());
            let doc: serde_json::Value = serde_json::from_str(&inserted).unwrap();
            assert_eq!(doc["mcpServers"][name]["command"], "grim", "name: {name:?}");
            assert!(doc["mcpServers"]["other"].is_object(), "sibling survives");
        }
    }

    #[test]
    fn hostile_member_name_is_found_again_and_removed() {
        // The pre-fix defect was not only invalid JSON: a `\` emitted valid JSON
        // decoding to a *different* key, so the lookup never matched and grim
        // re-inserted a duplicate on every install.
        for name in HOSTILE_NAMES {
            let value = json!({"command": "grim"});
            let first = changed(upsert_member("", "mcpServers", name, &value).unwrap());
            assert_eq!(
                upsert_member(&first, "mcpServers", name, &value).unwrap(),
                Splice::Unchanged,
                "re-upsert must find its own entry, not duplicate it: {name:?}"
            );
            assert_eq!(member_value(&first, "mcpServers", name), Some(value));
            let removed = changed(remove_member(&first, "mcpServers", name).unwrap());
            let doc: serde_json::Value = serde_json::from_str(&removed).unwrap();
            assert!(doc.get("mcpServers").is_none(), "emptied container removed: {removed}");
        }
    }

    #[test]
    fn hostile_container_name_and_array_key_are_escaped_too() {
        // `container` is a vendor literal today and the array `key` is a const,
        // so neither is hostile-reachable — both are escaped anyway, because
        // "currently unreachable" is the reasoning that made #56 latent.
        for name in HOSTILE_NAMES {
            let out = changed(upsert_member("", name, "grim", &json!({"command": "grim"})).unwrap());
            let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
            assert_eq!(doc[name]["grim"]["command"], "grim", "container: {name:?}");

            // Array sites: the empty-text skeleton, then the absent-array insert.
            let skeleton = changed(upsert_array_element("", name, "g.md").unwrap());
            let doc: serde_json::Value = serde_json::from_str(&skeleton).unwrap();
            assert_eq!(doc[name], json!(["g.md"]), "array key: {name:?}");
            let inserted = changed(upsert_array_element("{\n  \"model\": \"a/b\"\n}\n", name, "g.md").unwrap());
            let doc: serde_json::Value = serde_json::from_str(&inserted).unwrap();
            assert_eq!(doc[name], json!(["g.md"]), "array key: {name:?}");
            assert_eq!(doc["model"], "a/b");
        }
    }

    #[test]
    fn escaping_is_the_identity_function_on_an_ordinary_name() {
        // Principle 9 / self-heal, unit half: `serde_json` escapes exactly `"`,
        // `\` and `U+0000..U+001F` — never `/`, never non-ASCII — so every name
        // whose pre-fix output was valid re-readable JSON re-materializes
        // byte-identically. (The on-disk pre-escaping-era fixture is the
        // acceptance half, owed to Specify.)
        for name in ["grim", "code-review", "a.b", "with/slash", "🧙", "UPPER_case-1"] {
            assert_eq!(json_string(name), format!("\"{name}\""), "name: {name:?}");
        }
    }

    // ── Object-in-nested-array splice (the hook dispatcher registration) ──

    /// The marker grim stamps on the element it owns — a **constant** value, so
    /// the `owner` predicate can still name it after the artifact has left
    /// install state (`HOOK_MARKER_KEY` / `HOOK_MARKER_VALUE` in `vendor.rs`).
    const MARKER: &str = "com.grimoire.managed";

    fn hook_path<'a>(event: &'a str, matcher: &'a str) -> NestedHandlerPath<'a> {
        NestedHandlerPath {
            group: NestedGroupPath {
                container: "hooks",
                member: event,
                group_key: "matcher",
                elements_key: "hooks",
            },
            group_value: matcher,
            identity_keys: &[MARKER],
        }
    }

    fn dispatcher(command: &str) -> serde_json::Value {
        json!({"type": "command", "command": command, "com.grimoire.managed": "hook-dispatcher"})
    }

    fn jsonc(text: &str) -> serde_json::Value {
        serde_json::from_str(&sanitize_jsonc(text)).unwrap_or_else(|e| panic!("not readable JSONC ({e}): {text}"))
    }

    #[test]
    fn nested_upsert_into_empty_text_creates_the_whole_chain() {
        let path = hook_path("PreToolUse", "Bash");
        let out = changed(upsert_nested_handler("", &path, &dispatcher("run")).unwrap());
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["hooks"]["PreToolUse"][0]["matcher"], "Bash");
        assert_eq!(doc["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "run");
        assert_eq!(
            upsert_nested_handler(&out, &path, &dispatcher("run")).unwrap(),
            Splice::Unchanged,
            "idempotent"
        );
    }

    #[test]
    fn nested_upsert_creates_an_absent_container_then_an_absent_member() {
        let path = hook_path("PreToolUse", "Bash");

        // No `hooks` container at all.
        let text = "{\n  \"permissions\": {\n    \"allow\": [\"Read\"]\n  }\n}\n";
        let out = changed(upsert_nested_handler(text, &path, &dispatcher("run")).unwrap());
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["permissions"]["allow"], json!(["Read"]));
        assert_eq!(doc["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "run");
        let back = changed(remove_nested_handler(&out, &path, &dispatcher("run")).unwrap());
        assert_eq!(back, text, "remove undoes upsert exactly");

        // Container present, this event absent.
        let text = "{\n  \"hooks\": {\n    \"Stop\": [\n      {\n        \"matcher\": \"*\",\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"user-stop\"\n          }\n        ]\n      }\n    ]\n  }\n}\n";
        let out = changed(upsert_nested_handler(text, &path, &dispatcher("run")).unwrap());
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["hooks"]["Stop"][0]["hooks"][0]["command"], "user-stop");
        assert_eq!(doc["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "run");
        let back = changed(remove_nested_handler(&out, &path, &dispatcher("run")).unwrap());
        assert_eq!(back, text, "remove undoes upsert exactly");
    }

    /// A realistic `settings.json`: two authored matcher groups, a comment, and
    /// deliberately irregular spacing on an unmanaged key.
    const CLAUDE_SETTINGS: &str = concat!(
        "{\n",
        "  // user settings\n",
        "  \"permissions\": {\"allow\":   [\"Read\"]},\n",
        "  \"hooks\": {\n",
        "    \"PreToolUse\": [\n",
        "      {\n",
        "        \"matcher\": \"Write\",\n",
        "        \"hooks\": [\n",
        "          {\n",
        "            \"type\": \"command\",\n",
        "            \"command\": \"user-write-hook\"\n",
        "          }\n",
        "        ]\n",
        "      },\n",
        "      {\n",
        "        \"matcher\": \"Bash\",\n",
        "        \"hooks\": [\n",
        "          {\n",
        "            \"type\": \"command\",\n",
        "            \"command\": \"user-bash-hook\"\n",
        "          }\n",
        "        ]\n",
        "      }\n",
        "    ]\n",
        "  },\n",
        "  \"model\": \"opus\"\n",
        "}\n",
    );

    #[test]
    fn nested_upsert_into_an_existing_group_preserves_every_other_byte() {
        let path = hook_path("PreToolUse", "Bash");
        let out = changed(upsert_nested_handler(CLAUDE_SETTINGS, &path, &dispatcher("run")).unwrap());

        assert!(out.contains("// user settings"), "comment preserved: {out}");
        assert!(
            out.contains("\"permissions\": {\"allow\":   [\"Read\"]},"),
            "authored formatting preserved: {out}"
        );
        assert!(
            out.find("\"permissions\"") < out.find("\"hooks\""),
            "key order preserved"
        );
        assert!(out.find("\"hooks\"") < out.find("\"model\""), "key order preserved");
        assert!(out.contains("user-write-hook"), "sibling group preserved");
        assert!(out.contains("user-bash-hook"), "sibling element preserved");

        let doc = jsonc(&out);
        let groups = doc["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(groups.len(), 2, "no third group invented: {out}");
        let bash = groups.iter().find(|g| g["matcher"] == "Bash").unwrap();
        assert_eq!(bash["hooks"].as_array().unwrap().len(), 2);
        assert_eq!(bash["hooks"][1], dispatcher("run"));

        assert_eq!(
            upsert_nested_handler(&out, &path, &dispatcher("run")).unwrap(),
            Splice::Unchanged,
            "idempotent against the file it just wrote"
        );
        let back = changed(remove_nested_handler(&out, &path, &dispatcher("run")).unwrap());
        assert_eq!(back, CLAUDE_SETTINGS, "remove undoes upsert exactly");
    }

    #[test]
    fn nested_upsert_creates_an_absent_group_beside_the_authored_ones() {
        let path = hook_path("PreToolUse", "Read");
        let out = changed(upsert_nested_handler(CLAUDE_SETTINGS, &path, &dispatcher("run")).unwrap());
        let doc = jsonc(&out);
        let groups = doc["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(groups.len(), 3, "one group added: {out}");
        assert_eq!(groups[2]["matcher"], "Read");
        assert_eq!(groups[2]["hooks"][0], dispatcher("run"));
        assert!(out.contains("user-write-hook") && out.contains("user-bash-hook"));

        let back = changed(remove_nested_handler(&out, &path, &dispatcher("run")).unwrap());
        assert_eq!(back, CLAUDE_SETTINGS, "remove undoes upsert exactly");
    }

    #[test]
    fn nested_upsert_creates_an_absent_element_array_in_an_authored_group() {
        // A group the user wrote with a matcher but no `hooks` array yet: grim
        // adds the array as a new member of THEIR object.
        let text = "{\n  \"hooks\": {\n    \"Stop\": [\n      {\n        \"matcher\": \"*\"\n      }\n    ]\n  }\n}\n";
        let path = hook_path("Stop", "*");
        let out = changed(upsert_nested_handler(text, &path, &dispatcher("run")).unwrap());
        let doc = jsonc(&out);
        assert_eq!(doc["hooks"]["Stop"][0]["hooks"][0], dispatcher("run"));
        assert_eq!(doc["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(doc["hooks"]["Stop"][0]["matcher"], "*");

        // The irreducible case, asserted so it can never change silently: a
        // group whose only authored member is the matcher is byte-identical to
        // one grim creates, so the cascade cannot spare it and the whole chain
        // goes. Reversibility here is "no grim-owned bytes remain".
        let back = changed(remove_nested_handler(&out, &path, &dispatcher("run")).unwrap());
        assert_eq!(back, "{\n}\n", "the empty root survives, the inert stub does not");
    }

    #[test]
    fn nested_remove_never_deletes_a_group_carrying_authored_content() {
        // Same shape as above plus one member grim never writes. That makes the
        // group provably not grim's, so only the `hooks` member grim added is
        // cut and the authored object survives byte-for-byte.
        let text = concat!(
            "{\n",
            "  \"hooks\": {\n",
            "    \"Stop\": [\n",
            "      {\n",
            "        \"matcher\": \"*\",\n",
            "        \"description\": \"mine, hands off\"\n",
            "      }\n",
            "    ]\n",
            "  }\n",
            "}\n",
        );
        let path = hook_path("Stop", "*");
        let out = changed(upsert_nested_handler(text, &path, &dispatcher("run")).unwrap());
        assert_eq!(jsonc(&out)["hooks"]["Stop"][0]["hooks"][0], dispatcher("run"));
        let back = changed(remove_nested_handler(&out, &path, &dispatcher("run")).unwrap());
        assert_eq!(back, text, "remove undoes upsert exactly, foreign data intact");
    }

    #[test]
    fn nested_upsert_updates_in_place_when_only_the_command_moved() {
        // D-2: identity keys on the constant marker, never on the command — the
        // command embeds the launcher path and the root token, both of which
        // grim recomputes. A relocation must UPDATE, not fork.
        let path = hook_path("PreToolUse", "Bash");
        let first = changed(upsert_nested_handler(CLAUDE_SETTINGS, &path, &dispatcher("/old/grim-hook run")).unwrap());
        let second = changed(upsert_nested_handler(&first, &path, &dispatcher("/new/grim-hook run")).unwrap());

        let doc = jsonc(&second);
        let bash = doc["hooks"]["PreToolUse"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["matcher"] == "Bash")
            .unwrap();
        let elements = bash["hooks"].as_array().unwrap();
        assert_eq!(elements.len(), 2, "exactly one grim element, not two: {second}");
        assert_eq!(elements[0]["command"], "user-bash-hook");
        assert_eq!(elements[1]["command"], "/new/grim-hook run");
        assert!(
            !second.contains("/old/grim-hook"),
            "the stale command is gone: {second}"
        );
        assert!(second.contains("// user settings"), "comment still preserved");
    }

    #[test]
    fn nested_remove_cascades_only_through_the_levels_it_emptied() {
        let path = hook_path("PreToolUse", "Bash");

        // Sole occupant at every level: the whole `hooks` container goes.
        let text = "{\n  \"model\": \"opus\",\n  \"hooks\": {\n    \"PreToolUse\": [\n      {\n        \"matcher\": \"Bash\",\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"run\",\n            \"com.grimoire.managed\": \"hook-dispatcher\"\n          }\n        ]\n      }\n    ]\n  }\n}\n";
        let out = changed(remove_nested_handler(text, &path, &dispatcher("run")).unwrap());
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(doc.get("hooks").is_none(), "no husk left at any level: {out}");
        assert_eq!(doc["model"], "opus");

        // A user's sibling element keeps grim's group — and therefore every
        // level above it — alive. This is why reversibility is "no grim-owned
        // bytes remain", not "byte-identical to pre-install".
        let with_sibling = changed(upsert_nested_handler(CLAUDE_SETTINGS, &path, &dispatcher("run")).unwrap());
        let out = changed(remove_nested_handler(&with_sibling, &path, &dispatcher("run")).unwrap());
        assert_eq!(out, CLAUDE_SETTINGS);

        // A sibling GROUP keeps the member and the container: removing grim's
        // only element drops grim's whole group object, nothing more.
        let path = hook_path("PreToolUse", "Read");
        let with_group = changed(upsert_nested_handler(CLAUDE_SETTINGS, &path, &dispatcher("run")).unwrap());
        let out = changed(remove_nested_handler(&with_group, &path, &dispatcher("run")).unwrap());
        assert_eq!(out, CLAUDE_SETTINGS);
    }

    #[test]
    fn nested_remove_is_tolerant_of_everything_absent() {
        let path = hook_path("PreToolUse", "Bash");
        let handler = dispatcher("run");
        for text in [
            "",
            "{}",
            "{\"model\": \"opus\"}",
            "{\"hooks\": {}}",
            "{\"hooks\": {\"Stop\": []}}",
            // Right event, wrong matcher group.
            "{\"hooks\": {\"PreToolUse\": [{\"matcher\": \"Write\", \"hooks\": [{\"type\": \"command\", \"command\": \"x\", \"com.grimoire.managed\": \"hook-dispatcher\"}]}]}}",
            // Right group, no element array.
            "{\"hooks\": {\"PreToolUse\": [{\"matcher\": \"Bash\"}]}}",
            // Right group, only a foreign element.
            "{\"hooks\": {\"PreToolUse\": [{\"matcher\": \"Bash\", \"hooks\": [{\"type\": \"command\", \"command\": \"x\"}]}]}}",
        ] {
            assert_eq!(
                remove_nested_handler(text, &path, &handler).unwrap(),
                Splice::Unchanged,
                "input: {text}"
            );
        }
    }

    #[test]
    fn nested_splice_refuses_both_degenerate_identities() {
        let group = NestedGroupPath {
            container: "hooks",
            member: "PreToolUse",
            group_key: "matcher",
            elements_key: "hooks",
        };
        // Vacuously-true identity: would adopt and overwrite the first element
        // in the group, including a user-authored one.
        let vacuous = NestedHandlerPath {
            group,
            group_value: "Bash",
            identity_keys: &[],
        };
        // Never-satisfiable identity: would insert another copy on every run.
        let unsatisfiable = NestedHandlerPath {
            group,
            group_value: "Bash",
            identity_keys: &["com.grimoire.managed"],
        };
        let unmarked = json!({"type": "command", "command": "run"});

        for (path, handler) in [(&vacuous, &dispatcher("run")), (&unsatisfiable, &unmarked)] {
            let err = upsert_nested_handler(CLAUDE_SETTINGS, path, handler).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData);
            let err = remove_nested_handler(CLAUDE_SETTINGS, path, handler).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "remove refuses it too");
            assert_eq!(
                nested_handler_value(CLAUDE_SETTINGS, path, handler),
                None,
                "and the read answers None rather than guessing"
            );
        }
        // Refused even for an empty document, where a bare insert would be
        // harmless — the pair must not disagree about its own contract.
        assert!(upsert_nested_handler("", &vacuous, &dispatcher("run")).is_err());
        assert!(remove_nested_handler("", &vacuous, &dispatcher("run")).is_err());
    }

    #[test]
    fn nested_splice_refuses_every_unexpected_shape() {
        let path = hook_path("PreToolUse", "Bash");
        let handler = dispatcher("run");
        for text in [
            "not json {{{",
            "[1, 2]",
            "42",
            // Container is not an object.
            "{\"hooks\": []}",
            // The member's value is not an array.
            "{\"hooks\": {\"PreToolUse\": {}}}",
            // A group entry is not an object.
            "{\"hooks\": {\"PreToolUse\": [null]}}",
            "{\"hooks\": {\"PreToolUse\": [\"Bash\"]}}",
            // The group's element key is not an array.
            "{\"hooks\": {\"PreToolUse\": [{\"matcher\": \"Bash\", \"hooks\": {}}]}}",
        ] {
            let err = upsert_nested_handler(text, &path, &handler).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "upsert, input: {text}");
            let err = remove_nested_handler(text, &path, &handler).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "remove, input: {text}");
        }
    }

    #[test]
    fn nested_splice_tolerates_comments_and_trailing_commas() {
        let text = concat!(
            "{\n",
            "  // hooks the user wrote\n",
            "  \"hooks\": {\n",
            "    \"PreToolUse\": [\n",
            "      {\n",
            "        \"matcher\": \"Bash\",\n",
            "        \"hooks\": [\n",
            "          {\"type\": \"command\", \"command\": \"user\"},\n",
            "        ],\n",
            "      },\n",
            "    ],\n",
            "  },\n",
            "}\n",
        );
        let path = hook_path("PreToolUse", "Bash");
        let out = changed(upsert_nested_handler(text, &path, &dispatcher("run")).unwrap());
        assert!(out.contains("// hooks the user wrote"), "comment preserved: {out}");
        let doc = jsonc(&out);
        assert_eq!(doc["hooks"]["PreToolUse"][0]["hooks"][1], dispatcher("run"));
        let back = changed(remove_nested_handler(&out, &path, &dispatcher("run")).unwrap());
        assert_eq!(back, text, "remove undoes upsert exactly");
    }

    /// Each level of the cascade cuts one item from its container, and each cut
    /// must find its comma past a comment, or the settings file stops parsing.
    #[test]
    fn nested_remove_beside_a_comment_keeps_the_file_readable() {
        let grim = "{\"type\": \"command\", \"command\": \"run\", \"com.grimoire.managed\": \"hook-dispatcher\"}";
        let user = "{\"type\": \"command\", \"command\": \"user\"}";
        let path = hook_path("PreToolUse", "Bash");
        let user_only =
            json!({"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "user"}]}]}});
        let write_only = json!({"hooks": {"PreToolUse": [{"matcher": "Write", "hooks": []}]}});
        let stop_only = json!({"hooks": {"Stop": []}});
        for (text, comment, rest) in [
            // The element, beside a user element.
            (
                format!(
                    "{{\"hooks\": {{\"PreToolUse\": [{{\"matcher\": \"Bash\", \"hooks\": [{grim} /* e */, {user}]}}]}}}}"
                ),
                "/* e */",
                user_only.clone(),
            ),
            (
                format!(
                    "{{\"hooks\": {{\"PreToolUse\": [{{\"matcher\": \"Bash\", \"hooks\": [{user}, // e\n{grim}]}}]}}}}"
                ),
                "// e",
                user_only.clone(),
            ),
            // The group, beside a user group.
            (
                format!(
                    "{{\"hooks\": {{\"PreToolUse\": [{{\"matcher\": \"Bash\", \"hooks\": [{grim}]}} /* g */, {{\"matcher\": \"Write\", \"hooks\": []}}]}}}}"
                ),
                "/* g */",
                write_only.clone(),
            ),
            (
                format!(
                    "{{\"hooks\": {{\"PreToolUse\": [{{\"matcher\": \"Write\", \"hooks\": []}}, /* g */ {{\"matcher\": \"Bash\", \"hooks\": [{grim}]}}]}}}}"
                ),
                "/* g */",
                write_only.clone(),
            ),
            // The element array of a group that holds authored content.
            (
                format!(
                    "{{\"hooks\": {{\"PreToolUse\": [{{\"hooks\": [{grim}] /* m */, \"matcher\": \"Bash\", \"note\": \"mine\"}}]}}}}"
                ),
                "/* m */",
                json!({"hooks": {"PreToolUse": [{"matcher": "Bash", "note": "mine"}]}}),
            ),
            // The event member, beside another event.
            (
                format!(
                    "{{\"hooks\": {{\"PreToolUse\": [{{\"matcher\": \"Bash\", \"hooks\": [{grim}]}}] /* p */, \"Stop\": []}}}}"
                ),
                "/* p */",
                stop_only.clone(),
            ),
            (
                format!(
                    "{{\"hooks\": {{\"Stop\": [], // p\n\"PreToolUse\": [{{\"matcher\": \"Bash\", \"hooks\": [{grim}]}}]}}}}"
                ),
                "// p",
                stop_only.clone(),
            ),
        ] {
            let out = changed(remove_nested_handler(&text, &path, &dispatcher("run")).unwrap());
            assert_eq!(parsed(&out), rest, "{text:?} gave {out:?}");
            assert_eq!(strict_without(&out, comment), rest, "{text:?} gave {out:?}");
            assert!(!out.contains("hook-dispatcher"), "{out:?}");
            assert!(out.contains(comment), "{text:?} gave {out:?}");
        }
    }

    #[test]
    fn nested_handler_value_separates_absent_from_differing() {
        let path = hook_path("PreToolUse", "Bash");
        assert_eq!(nested_handler_value(CLAUDE_SETTINGS, &path, &dispatcher("run")), None);

        let installed = changed(upsert_nested_handler(CLAUDE_SETTINGS, &path, &dispatcher("old")).unwrap());
        assert_eq!(
            nested_handler_value(&installed, &path, &dispatcher("new")),
            Some(dispatcher("old")),
            "found by identity, and the CURRENT value is what comes back"
        );
        assert_eq!(nested_handler_value("not json {{{", &path, &dispatcher("run")), None);
    }

    #[test]
    fn nested_splice_reads_no_element_field_but_the_identity_keys() {
        // The primitive is shape-blind: `command`-form, copilot's `exec`/`args`
        // argv form, and an element carrying neither all behave identically,
        // because nothing here consults a field `identity_keys` did not name.
        // (Belt and braces — the element grim splices is its own dispatcher
        // registration, never the authored handler. The handler C-019 validates
        // reaches `DispatchEntry.handler` in the dispatch table, which this
        // module never sees.)
        let path = hook_path("PreToolUse", "Bash");
        let shapes = [
            json!({"type": "command", "command": "run", "com.grimoire.managed": "hook-dispatcher"}),
            json!({"exec": "/abs/grim-hook", "args": ["run", "--client", "copilot"], "com.grimoire.managed": "hook-dispatcher"}),
            json!({"com.grimoire.managed": "hook-dispatcher"}),
        ];
        for element in &shapes {
            let out = changed(upsert_nested_handler(CLAUDE_SETTINGS, &path, element).unwrap());
            let doc = jsonc(&out);
            let bash = doc["hooks"]["PreToolUse"]
                .as_array()
                .unwrap()
                .iter()
                .find(|g| g["matcher"] == "Bash")
                .unwrap();
            assert_eq!(bash["hooks"][1], *element, "written verbatim: {out}");
            assert_eq!(
                upsert_nested_handler(&out, &path, element).unwrap(),
                Splice::Unchanged,
                "idempotent for shape {element}"
            );
            let back = changed(remove_nested_handler(&out, &path, element).unwrap());
            assert_eq!(
                back, CLAUDE_SETTINGS,
                "remove undoes upsert exactly for shape {element}"
            );
        }

        // Identity is the marker alone, so an element written in one shape is
        // located by a probe of any other — which is what makes a launcher or
        // root-token move an update rather than a fork (D-2).
        let installed = changed(upsert_nested_handler(CLAUDE_SETTINGS, &path, &shapes[1]).unwrap());
        assert_eq!(
            nested_handler_value(&installed, &path, &shapes[0]),
            Some(shapes[1].clone()),
            "found across shapes: only the marker is consulted"
        );
        let replaced = changed(upsert_nested_handler(&installed, &path, &shapes[0]).unwrap());
        let doc = jsonc(&replaced);
        let bash = doc["hooks"]["PreToolUse"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["matcher"] == "Bash")
            .unwrap();
        assert_eq!(
            bash["hooks"].as_array().unwrap().len(),
            2,
            "updated, not forked: {replaced}"
        );
        assert_eq!(bash["hooks"][1], shapes[0]);
    }

    #[test]
    fn a_command_string_reaches_the_config_as_escaped_data() {
        // C-018's belt-and-braces leg, from this module's side: whatever the
        // command string holds, it lands as a JSON string **value** and comes
        // back byte-identical. This module neither parses nor splits it — the
        // shell-metacharacter contract (C-018b) is enforced where the string is
        // assembled, not here, and the exec-bit rule (C-019) is enforced at
        // `grim build` against a handler this module never receives.
        let path = hook_path("PreToolUse", "Bash");
        for command in [
            "L='/abs/grim-hook'\n[ -f \"$L\" ] && [ -x \"$L\" ] || exit 0\n\"$L\" run",
            "$(id) `whoami` ; rm -rf / && echo \"quoted\" 'single'",
            "./guard.sh",
            "sh ./guard.sh",
            "C:\\Program Files\\grim\\grim-hook.exe run",
        ] {
            let element = json!({"type": "command", "command": command, "com.grimoire.managed": "hook-dispatcher"});
            let out = changed(upsert_nested_handler(CLAUDE_SETTINGS, &path, &element).unwrap());
            let doc = jsonc(&out);
            let bash = doc["hooks"]["PreToolUse"]
                .as_array()
                .unwrap()
                .iter()
                .find(|g| g["matcher"] == "Bash")
                .unwrap();
            assert_eq!(bash["hooks"][1]["command"], command, "verbatim through the splice");
            assert_eq!(
                upsert_nested_handler(&out, &path, &element).unwrap(),
                Splice::Unchanged,
                "and re-found on the next run: {command:?}"
            );
            let back = changed(remove_nested_handler(&out, &path, &element).unwrap());
            assert_eq!(back, CLAUDE_SETTINGS, "remove undoes upsert exactly: {command:?}");
        }
    }

    #[test]
    fn owned_nested_handlers_finds_what_no_record_could_name() {
        // D-1: after an uninstall the record naming the matcher group has already
        // left install state, so the registrar can reconstruct neither the group
        // value nor the element. The constant marker is what makes the
        // registration findable anyway.
        let group = NestedGroupPath {
            container: "hooks",
            member: "PreToolUse",
            group_key: "matcher",
            elements_key: "hooks",
        };
        let mut text = CLAUDE_SETTINGS.to_string();
        for matcher in ["Bash", "Read", "Write"] {
            text =
                changed(upsert_nested_handler(&text, &hook_path("PreToolUse", matcher), &dispatcher("run")).unwrap());
        }

        let managed = json!("hook-dispatcher");
        let owner = [(MARKER, &managed)];
        let owned = owned_nested_handlers(&text, &group, &owner);
        assert_eq!(
            owned.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(),
            ["Write", "Bash", "Read"],
            "every owned element, paired with its group value, in document order"
        );
        assert!(owned.iter().all(|(_, element)| *element == dispatcher("run")));

        // Removing `owned - desired` converges without any record.
        for (matcher, element) in &owned {
            text = changed(remove_nested_handler(&text, &hook_path("PreToolUse", matcher), element).unwrap());
        }
        assert_eq!(text, CLAUDE_SETTINGS, "reaped back to the authored file");
        assert!(owned_nested_handlers(&text, &group, &owner).is_empty());
    }

    #[test]
    fn owned_nested_handlers_owns_nothing_it_should_not() {
        let group = NestedGroupPath {
            container: "hooks",
            member: "PreToolUse",
            group_key: "matcher",
            elements_key: "hooks",
        };
        let managed = json!("hook-dispatcher");
        let installed = changed(
            upsert_nested_handler(CLAUDE_SETTINGS, &hook_path("PreToolUse", "Bash"), &dispatcher("run")).unwrap(),
        );

        // An empty predicate is vacuously true, so it would claim every element
        // in the file — a removal driver must own nothing instead.
        assert!(owned_nested_handlers(&installed, &group, &[]).is_empty());

        // A different marker value owns nothing (the freeze in `vendor.rs` is
        // what keeps a shipped registration findable).
        let other = json!("something-else");
        assert!(owned_nested_handlers(&installed, &group, &[(MARKER, &other)]).is_empty());

        // Unparsable text, absent container, absent member, non-array member.
        for text in [
            "not json {{{",
            "{}",
            "{\"hooks\": {}}",
            "{\"hooks\": {\"PreToolUse\": 3}}",
        ] {
            assert!(
                owned_nested_handlers(text, &group, &[(MARKER, &managed)]).is_empty(),
                "input: {text}"
            );
        }

        // The user's own elements are never owned.
        let owned = owned_nested_handlers(&installed, &group, &[(MARKER, &managed)]);
        assert_eq!(owned.len(), 1, "only grim's element: {owned:?}");
    }

    // ── RFC 6901 paths and `value_at` ───────────────────────────────────

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

    /// Each row spells a pointer by hand beside the segments it must decode to.
    const POINTER_ROUND_TRIPS: &[(&str, &[&str])] = &[
        ("/model", &["model"]),
        ("/permissions/deny", &["permissions", "deny"]),
        ("/a/b/c/d/e", &["a", "b", "c", "d", "e"]),
        ("/amp.mcpServers/grim", &["amp.mcpServers", "grim"]),
        ("/a~1b", &["a/b"]),
        ("/m~0n", &["m~n"]),
        ("/a~1b/c~0d", &["a/b", "c~d"]),
        ("/~01", &["~1"]),
        ("/~00", &["~0"]),
        ("/~10", &["/0"]),
        ("/~0~1", &["~/"]),
        ("/~1~0", &["/~"]),
        ("/~0~0", &["~~"]),
        ("/~1~1", &["//"]),
        ("/Bash(rm:*)/ü", &["Bash(rm:*)", "ü"]),
        ("/", &[""]),
        ("/a/", &["a", ""]),
        ("//a", &["", "a"]),
    ];

    fn encoded(segments: &[&str]) -> String {
        segments
            .iter()
            .map(|segment| format!("/{}", segment.replace('~', "~0").replace('/', "~1")))
            .collect()
    }

    fn nested(segments: &[&str], leaf: serde_json::Value) -> serde_json::Value {
        segments
            .iter()
            .rev()
            .fold(leaf, |inner, segment| serde_json::json!({ *segment: inner }))
    }

    #[test]
    fn a_pointer_segment_round_trips_through_the_table() {
        for (pointer, segments) in POINTER_ROUND_TRIPS {
            assert_eq!(split_pointer_path(pointer), Some(path(segments)), "pointer: {pointer}");
            assert_eq!(&encoded(segments), pointer, "segments: {segments:?}");
        }
    }

    #[test]
    fn a_pointer_reads_the_member_value_pointer_reads() {
        for (pointer, segments) in POINTER_ROUND_TRIPS {
            let document = nested(segments, serde_json::json!(42));
            assert_eq!(document.pointer(pointer), Some(&serde_json::json!(42)), "{pointer}");
            assert_eq!(
                value_at(&document.to_string(), &split_pointer_path(pointer).unwrap()).unwrap(),
                found(serde_json::json!(42)),
                "pointer: {pointer}"
            );
        }
    }

    #[test]
    fn split_pointer_path_decodes_tilde_one_before_tilde_zero_would_rewrite_it() {
        assert_eq!(split_pointer_path("/~01"), Some(path(&["~1"])), "never `/`");
    }

    #[test]
    fn split_pointer_path_refuses_what_rfc6901_forbids() {
        for bad in ["model", "a/b", " /a", "~", "/~", "/a~", "/a~2b", "/~x", "/a/~", "/~~1"] {
            assert_eq!(split_pointer_path(bad), None, "input: {bad}");
        }
    }

    #[test]
    fn split_pointer_path_reads_the_empty_pointer_as_the_whole_document() {
        assert_eq!(split_pointer_path(""), Some(Vec::new()));
        assert_eq!(value_at("{\"a\": 1}", &[]).unwrap(), found(serde_json::json!({"a": 1})));
    }

    #[test]
    fn split_pointer_stays_two_level_and_undecoded_beside_the_full_path() {
        assert_eq!(split_pointer("/a/b/c"), None);
        assert_eq!(split_pointer_path("/a/b/c"), Some(path(&["a", "b", "c"])));
        assert_eq!(split_pointer("/model"), None);
        assert_eq!(split_pointer_path("/model"), Some(path(&["model"])));
        assert_eq!(split_pointer("/a~1b/c"), Some(("a~1b", "c")));
    }

    #[test]
    fn value_at_reads_a_member_at_any_depth() {
        let text = r#"{"model": "opus", "a": {"b": {"c": {"d": [1, 2]}, "n": null}}, "o": {}}"#;
        assert_eq!(
            value_at(text, &path(&["model"])).unwrap(),
            found(serde_json::json!("opus"))
        );
        assert_eq!(
            value_at(text, &path(&["a", "b", "c", "d"])).unwrap(),
            found(serde_json::json!([1, 2]))
        );
        assert_eq!(
            value_at(text, &path(&["a", "b", "c"])).unwrap(),
            found(serde_json::json!({"d": [1, 2]}))
        );
        assert_eq!(value_at(text, &path(&["o"])).unwrap(), found(serde_json::json!({})));
        assert_eq!(
            value_at(text, &path(&["a", "b", "n"])).unwrap(),
            found(serde_json::Value::Null),
            "a null value is present"
        );
    }

    #[test]
    fn value_at_reads_jsonc() {
        let text = "{\n  // a comment\n  \"a\": {\n    /* block */ \"b\": \"http://x\",\n  },\n}\n";
        assert_eq!(
            value_at(text, &path(&["a", "b"])).unwrap(),
            found(serde_json::json!("http://x"))
        );
    }

    #[test]
    fn value_at_tells_absent_from_blocked() {
        let text = r#"{"a": {"b": 1}}"#;
        assert_eq!(value_at(text, &path(&["z"])).unwrap(), PointerRead::Absent);
        assert_eq!(value_at(text, &path(&["a", "z"])).unwrap(), PointerRead::Absent);
        assert_eq!(value_at(text, &path(&["z", "y", "x"])).unwrap(), PointerRead::Absent);
        assert_eq!(value_at(text, &path(&["a", "b", "c"])).unwrap(), blocked(&["a", "b"]));
    }

    #[test]
    fn value_at_blocks_on_the_first_ancestor_that_is_not_an_object() {
        let text = r#"{"s": "text", "n": null, "l": [{"x": 1}], "a": {"t": true}}"#;
        assert_eq!(value_at(text, &path(&["s", "x"])).unwrap(), blocked(&["s"]));
        assert_eq!(value_at(text, &path(&["n", "x"])).unwrap(), blocked(&["n"]));
        assert_eq!(
            value_at(text, &path(&["l", "0"])).unwrap(),
            blocked(&["l"]),
            "no array index"
        );
        assert_eq!(
            value_at(text, &path(&["a", "t", "x", "y"])).unwrap(),
            blocked(&["a", "t"])
        );
    }

    #[test]
    fn value_at_refuses_an_unparseable_or_non_object_document() {
        assert_eq!(value_at("{}", &path(&["a"])).unwrap(), PointerRead::Absent, "control");
        for text in [
            "not json {{{",
            "[1, 2]",
            "\"text\"",
            "3",
            "{\"a\":",
            "// only a comment\n",
        ] {
            let err = value_at(text, &path(&["a"])).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "input: {text}");
        }
    }

    #[test]
    fn value_at_reads_an_empty_file_as_absent() {
        for text in ["", "  \n"] {
            assert_eq!(
                value_at(text, &path(&["a", "b"])).unwrap(),
                PointerRead::Absent,
                "{text:?}"
            );
        }
    }

    #[test]
    fn value_at_takes_the_last_duplicate_key_as_the_splice_engine_does() {
        let text = r#"{"a": 1, "a": 2}"#;
        assert_eq!(value_at(text, &path(&["a"])).unwrap(), found(serde_json::json!(2)));
    }

    #[test]
    fn value_at_error_does_not_echo_the_file() {
        let err = value_at("{\"token\": hunter2-sentinel}", &path(&["a"])).unwrap_err();
        assert!(!err.to_string().contains("hunter2-sentinel"), "{err}");
    }

    // ── Member splices at any depth ─────────────────────────────────────

    fn parsed(text: &str) -> serde_json::Value {
        parse_value(text).unwrap_or_else(|| panic!("unparseable: {text}"))
    }

    #[test]
    fn upsert_at_depth_creates_containers() {
        let value = json!("sonnet");
        let target = path(&["a", "b", "c"]);
        let skeleton = "{\n  \"a\": {\n    \"b\": {\n      \"c\": \"sonnet\"\n    }\n  }\n}\n";
        assert_eq!(changed(upsert_member_at("", &target, &value).unwrap()), skeleton);
        assert_eq!(changed(upsert_member_at("{}\n", &target, &value).unwrap()), skeleton);

        let partial = "{\n  \"a\": {\n    \"keep\": 1\n  },\n  \"z\": 2\n}\n";
        assert_eq!(
            changed(upsert_member_at(partial, &target, &value).unwrap()),
            "{\n  \"a\": {\n    \"keep\": 1,\n    \"b\": {\n      \"c\": \"sonnet\"\n    }\n  },\n  \"z\": 2\n}\n"
        );

        for text in ["", "  \n", "{}\n", partial] {
            let out = changed(upsert_member_at(text, &target, &value).unwrap());
            assert_eq!(value_at(&out, &target).unwrap(), found(value.clone()), "{out}");
            assert_eq!(
                upsert_member_at(&out, &target, &value).unwrap(),
                Splice::Unchanged,
                "{out}"
            );
        }
    }

    #[test]
    fn upsert_keeps_comments_and_sibling_order() {
        let text = "// team config\n{\n  // the model\n  \"model\": \"sonnet\", // pinned\n  \"permissions\": {\n    /* first */\n    \"allow\": [\n      \"Read\", // keep\n    ],\n  }, // end\n  \"zeta\": true // last\n}\n";

        assert_eq!(
            changed(upsert_member_at(text, &path(&["permissions", "deny"]), &json!(["Bash"])).unwrap()),
            "// team config\n{\n  // the model\n  \"model\": \"sonnet\", // pinned\n  \"permissions\": {\n    /* first */\n    \"allow\": [\n      \"Read\", // keep\n    ],\n    \"deny\": [\n      \"Bash\"\n    ],\n  }, // end\n  \"zeta\": true // last\n}\n",
            "a trailing comma stays one, and the new member comes last"
        );
        assert_eq!(
            changed(upsert_member_at(text, &path(&["theme"]), &json!("dark")).unwrap()),
            "// team config\n{\n  // the model\n  \"model\": \"sonnet\", // pinned\n  \"permissions\": {\n    /* first */\n    \"allow\": [\n      \"Read\", // keep\n    ],\n  }, // end\n  \"zeta\": true, // last\n  \"theme\": \"dark\"\n}\n",
            "a comment stays on its own member's line"
        );
        assert_eq!(
            changed(upsert_member_at(text, &path(&["model"]), &json!("opus")).unwrap()),
            text.replace("\"sonnet\"", "\"opus\""),
            "a replacement touches the value span alone"
        );
    }

    #[test]
    fn upsert_keeps_crlf_and_tabs() {
        let text = "{\r\n\t\"a\": {\r\n\t\t\"b\": 1\r\n\t}\r\n}\r\n";

        let deep = changed(upsert_member_at(text, &path(&["a", "c", "d"]), &json!({"e": [1]})).unwrap());
        assert_eq!(
            deep,
            "{\r\n\t\"a\": {\r\n\t\t\"b\": 1,\r\n\t\t\"c\": {\r\n\t\t\t\"d\": {\r\n\t\t\t\t\"e\": [\r\n\t\t\t\t\t1\r\n\t\t\t\t]\r\n\t\t\t}\r\n\t\t}\r\n\t}\r\n}\r\n"
        );

        let replaced = changed(upsert_member_at(text, &path(&["a", "b"]), &json!({"x": 1})).unwrap());
        assert_eq!(
            replaced,
            "{\r\n\t\"a\": {\r\n\t\t\"b\": {\r\n\t\t\t\"x\": 1\r\n\t\t}\r\n\t}\r\n}\r\n"
        );

        let empty = "{\r\n\t\"a\": {}\r\n}\r\n";
        assert_eq!(
            changed(upsert_member_at(empty, &path(&["a", "b"]), &json!(1)).unwrap()),
            "{\r\n\t\"a\": {\r\n\t\t\"b\": 1\r\n\t}\r\n}\r\n"
        );

        for out in [deep, replaced] {
            assert!(!out.replace("\r\n", "").contains('\n'), "a bare line feed: {out:?}");
        }
    }

    #[test]
    fn upsert_copies_the_indent_step_of_the_document() {
        let text = "{\n    \"a\": 1\n}\n";
        assert_eq!(
            changed(upsert_member_at(text, &path(&["b", "c"]), &json!(2)).unwrap()),
            "{\n    \"a\": 1,\n    \"b\": {\n        \"c\": 2\n    }\n}\n"
        );
    }

    #[test]
    fn upsert_stays_on_one_line_in_a_one_line_object() {
        assert_eq!(
            changed(upsert_member_at("{\"a\": {\"b\": 1}}\n", &path(&["a", "c"]), &json!(2)).unwrap()),
            "{\"a\": {\"b\": 1, \"c\": 2}}\n"
        );
        assert_eq!(
            changed(upsert_member_at("{ \"a\": 1 /* x */, }", &path(&["b"]), &json!(2)).unwrap()),
            "{ \"a\": 1 /* x */, \"b\": 2, }"
        );
    }

    /// A new member after a line comment on the object's only line would land
    /// inside the comment, and the file would lose it while reporting a change.
    #[test]
    fn upsert_never_lands_inside_a_line_comment() {
        let target = path(&["b"]);
        let out = changed(upsert_member_at("{\"a\": 1 // note\n}", &target, &json!(2)).unwrap());
        assert_eq!(value_at(&out, &target).unwrap(), found(json!(2)), "{out}");
        assert!(out.contains("// note"), "{out}");
    }

    /// `1/*x*/` is one token to a scanner that runs a scalar to the next
    /// delimiter, so the comment would go with the replaced value.
    #[test]
    fn upsert_keeps_a_comment_touching_a_scalar() {
        assert_eq!(
            changed(upsert_member_at("{\"a\":1/*x*/,\"b\":2}", &path(&["a"]), &json!(3)).unwrap()),
            "{\"a\":3/*x*/,\"b\":2}"
        );
        let out = changed(upsert_member_at("{\n  \"a\": 1// note\n}\n", &path(&["b"]), &json!(2)).unwrap());
        assert_eq!(out, "{\n  \"a\": 1,// note\n  \"b\": 2\n}\n");
    }

    /// A comment may end in whitespace JSON does not know. Moved past the new
    /// member it is a stray token and the file stops parsing.
    #[test]
    fn upsert_keeps_unicode_whitespace_inside_a_comment() {
        let target = path(&["b"]);
        for blank in ['\u{3000}', '\u{a0}', '\u{c}'] {
            let after_member = format!("{{\n  \"a\": 1 // c{blank}\n}}\n");
            assert_eq!(
                changed(upsert_member_at(&after_member, &target, &json!(2)).unwrap()),
                format!("{{\n  \"a\": 1, // c{blank}\n  \"b\": 2\n}}\n"),
            );
            let in_empty_object = format!("{{\n  // c{blank}\n}}\n");
            assert_eq!(
                changed(upsert_member_at(&in_empty_object, &target, &json!(2)).unwrap()),
                format!("{{\n  // c{blank}\n  \"b\": 2\n}}\n"),
            );
        }
    }

    #[test]
    fn upsert_compares_the_value_semantically() {
        let text = "{\"a\": {\"b\": {\"y\":  2, /* c */ \"x\": [1,]}}}";
        assert_eq!(
            upsert_member_at(text, &path(&["a", "b"]), &json!({"x": [1], "y": 2})).unwrap(),
            Splice::Unchanged
        );
    }

    #[test]
    fn upsert_follows_the_last_duplicate_key() {
        let text = "{\"a\": 1, \"a\": {\"b\": 1}}";
        assert_eq!(
            changed(upsert_member_at(text, &path(&["a", "b"]), &json!(2)).unwrap()),
            "{\"a\": 1, \"a\": {\"b\": 2}}"
        );
    }

    /// A shape-agnostic check over layouts the scanner has to walk: the file's
    /// meaning changes at the addressed member and nowhere else, both ways.
    #[test]
    fn splices_change_only_the_addressed_member() {
        let target = path(&["a", "new"]);
        let value = json!({"x": [1]});
        for text in [
            "{\"a\": {}}",
            "{ \"a\": { } }",
            "  {\n  \"a\": {}\n}  \n",
            "// head\n{\"a\": {\"b\": 1}} // tail\n",
            "/* head */ {\"a\": {\"b\": 1}}",
            "{\n  \"a\": {\n    // only a comment\n  }\n}\n",
            "{\n  \"a\": { /* block\n  spanning lines */ },\n}\n",
            "{\"a\": {\"b\": 1 /* x */, /* y */}, /* z */}",
            "{\"a\": {\"b\": 1//line\n}}",
            "{\"a\": {\"b\": 1 // line\n  // more\n}, \"c\": 2}",
            "{\"a\": {\"b\": \"//not a comment\"}}",
            "{\"a\": {\"b\": \"}\"}, \"c\": \"{\"}",
            "{\"a\": {\"esc\\\"aped\": \"\\\\\"}}",
            "{\"a\": {\"b\": 1,\n  \"c\": [\n    {\"d\": null},\n  ],\n}}",
            "{\"a\": {\n  \"b\": 1 // ideographic space\u{3000}\n}}",
            "{\"a\": {\n  \"b\": 1 // no-break space\u{a0}\n}}",
            "{\"a\": {\n  \"b\": 1 // form feed\u{c}\n}}",
        ] {
            let base = parsed(text);
            let mut expected = base.clone();
            expected["a"]["new"] = value.clone();

            let out = changed(upsert_member_at(text, &target, &value).unwrap());
            assert_eq!(parsed(&out), expected, "upsert of {text:?} gave {out:?}");
            for comment in [
                "// head",
                "// tail",
                "/* head */",
                "// only a comment",
                "/* x */",
                "/* y */",
            ] {
                assert_eq!(out.contains(comment), text.contains(comment), "{comment} in {out:?}");
            }

            let back = changed(remove_member_at(&out, &target).unwrap());
            assert_eq!(parsed(&back), base, "remove from {out:?} gave {back:?}");
        }
    }

    #[test]
    fn remove_at_depth_leaves_siblings() {
        let text = "{\n  // keep\n  \"a\": {\n    \"b\": {\n      \"drop\": 1,\n      \"stay\": 2 // note\n    },\n    \"c\": 3\n  },\n  \"z\": 4\n}\n";
        assert_eq!(
            changed(remove_member_at(text, &path(&["a", "b", "drop"])).unwrap()),
            "{\n  // keep\n  \"a\": {\n    \"b\": {\n      \"stay\": 2 // note\n    },\n    \"c\": 3\n  },\n  \"z\": 4\n}\n"
        );
        assert_eq!(
            changed(remove_member_at(text, &path(&["a", "c"])).unwrap()),
            "{\n  // keep\n  \"a\": {\n    \"b\": {\n      \"drop\": 1,\n      \"stay\": 2 // note\n    }\n  },\n  \"z\": 4\n}\n"
        );

        let crlf = "{\r\n\t\"a\": {\r\n\t\t\"b\": 1,\r\n\t\t\"c\": 2\r\n\t}\r\n}\r\n";
        assert_eq!(
            changed(remove_member_at(crlf, &path(&["a", "b"])).unwrap()),
            "{\r\n\t\"a\": {\r\n\t\t\"c\": 2\r\n\t}\r\n}\r\n"
        );
        assert_eq!(
            changed(remove_member_at(crlf, &path(&["a", "c"])).unwrap()),
            "{\r\n\t\"a\": {\r\n\t\t\"b\": 1\r\n\t}\r\n}\r\n"
        );
    }

    /// Pruning is its own operation, because only the writer knows which
    /// containers a harness requires.
    #[test]
    fn remove_leaves_an_emptied_container_in_place() {
        let out = changed(remove_member_at("{\"a\": {\"b\": 1}}", &path(&["a", "b"])).unwrap());
        assert_eq!(out, "{\"a\": {}}");
        let out = changed(remove_member_at("{\n  \"a\": {\n    \"b\": 1\n  }\n}\n", &path(&["a", "b"])).unwrap());
        assert_eq!(parsed(&out), json!({"a": {}}), "{out}");
    }

    #[test]
    fn remove_of_an_absent_member_is_unchanged() {
        let text = "{\"a\": {\"b\": 1}}";
        assert_eq!(
            changed(remove_member_at(text, &path(&["a", "b"])).unwrap()),
            "{\"a\": {}}",
            "control"
        );
        for target in [path(&["z"]), path(&["a", "z"]), path(&["z", "y", "x"])] {
            assert_eq!(
                remove_member_at(text, &target).unwrap(),
                Splice::Unchanged,
                "{target:?}"
            );
        }
        for text in ["", "  \n"] {
            assert_eq!(remove_member_at(text, &path(&["a"])).unwrap(), Splice::Unchanged);
        }
    }

    /// A value of another type in the way means the path holds nothing, which
    /// is what a removal asks for, so a forced removal has nothing to fail on.
    #[test]
    fn remove_under_a_non_container_is_unchanged() {
        for text in ["{\"a\": 1}", "{\"a\": [1]}", "{\"a\": null}", "{\"a\": \"s\"}"] {
            assert_eq!(value_at(text, &path(&["a", "b"])).unwrap(), blocked(&["a"]), "control");
            assert_eq!(
                remove_member_at(text, &path(&["a", "b"])).unwrap(),
                Splice::Unchanged,
                "{text}"
            );
        }
    }

    /// The last duplicate wins for every reader, so cutting only that one
    /// would turn the shadowed value into the live one.
    #[test]
    fn remove_takes_every_duplicate_of_the_key() {
        let text = "{\n  \"a\": 1,\n  \"b\": 2,\n  \"a\": 3\n}\n";
        let target = path(&["a"]);
        assert_eq!(value_at(text, &target).unwrap(), found(json!(3)), "control");
        let out = changed(remove_member_at(text, &target).unwrap());
        assert_eq!(value_at(&out, &target).unwrap(), PointerRead::Absent, "{out}");
        assert_eq!(out, "{\n  \"b\": 2\n}\n");
    }

    /// Every reader takes the last duplicate, so a removal below the first one
    /// would report a change and leave the value a reader sees.
    #[test]
    fn remove_below_a_duplicate_key_follows_the_last_one() {
        let text = "{\"a\": {\"b\": 1}, \"a\": {\"b\": 2, \"c\": 3}}";
        let target = path(&["a", "b"]);
        assert_eq!(value_at(text, &target).unwrap(), found(json!(2)), "control");
        let out = changed(remove_member_at(text, &target).unwrap());
        assert_eq!(value_at(&out, &target).unwrap(), PointerRead::Absent, "{out}");
        assert_eq!(value_at(&out, &path(&["a", "c"])).unwrap(), found(json!(3)), "{out}");
        assert!(out.contains("{\"b\": 1}"), "the shadowed member stays: {out}");
    }

    /// A comment between a value and its comma hides the comma from the cut,
    /// and a comma left with nothing before it is a file no parser reads.
    #[test]
    fn remove_keeps_a_comment_before_the_comma() {
        let target = path(&["a"]);
        for (text, rest, comment) in [
            ("{\"a\": 1 /* c */, \"b\": 2}", json!({"b": 2}), "/* c */"),
            (
                "{\"z\": 0, \"a\": 1 /* c */, \"b\": 2}",
                json!({"z": 0, "b": 2}),
                "/* c */",
            ),
            (
                "{\"z\": 0, /* y */ \"a\": 1 /* c */, \"b\": 2}",
                json!({"z": 0, "b": 2}),
                "/* c */",
            ),
            ("{\n  \"a\": 1 // c\n  , \"b\": 2\n}\n", json!({"b": 2}), "// c"),
            ("{\"a\": 1 /* c */,}", json!({}), "/* c */"),
        ] {
            let out = changed(remove_member_at(text, &target).unwrap());
            assert_eq!(parsed(&out), rest, "{text:?} gave {out:?}");
            assert!(out.contains(comment), "{out:?}");
        }
    }

    /// The last item's comma sits before it, and past a comment there the cut
    /// would leave the previous item a trailing comma strict JSON rejects.
    #[test]
    fn remove_takes_the_comma_before_a_comment() {
        let target = path(&["a"]);
        for (text, out) in [
            ("{\n  \"z\": 0, // keep\n  \"a\": 1\n}\n", "{\n  \"z\": 0 // keep\n}\n"),
            ("{\"z\": 0, /* keep */ \"a\": 1}", "{\"z\": 0 /* keep */ }"),
            (
                "{\"z\": 0 /* c */, /* keep */ \"a\": 1}",
                "{\"z\": 0 /* c */ /* keep */ }",
            ),
            // A trailing comma the file already had stays.
            (
                "{\n  \"z\": 0, // keep\n  \"a\": 1,\n}\n",
                "{\n  \"z\": 0, // keep\n}\n",
            ),
            ("{\"a\": 1, /* keep */ \"z\": 0}", "{ /* keep */ \"z\": 0}"),
        ] {
            assert_eq!(changed(remove_member_at(text, &target).unwrap()), out, "{text:?}");
        }
        for (text, out) in [
            (
                "{\"a\": [\n  \"y\", // keep\n  \"x\"\n]}",
                "{\"a\": [\n  \"y\" // keep\n]}",
            ),
            ("{\"a\": [\"y\", /* keep */ \"x\"]}", "{\"a\": [\"y\" /* keep */ ]}"),
            (
                "{\"a\": [\"y\", // keep\n\"x\", /* c */ \"x\"]}",
                "{\"a\": [\"y\" // keep\n /* c */ ]}",
            ),
        ] {
            assert_eq!(remove_element(text, &target, "x"), out, "{text:?}");
        }
    }

    /// A line comment ending in `,` before the last or only item. Its comma is
    /// no separator, and taking it cuts into the comment, which swallows a
    /// closing bracket on the item's line.
    #[test]
    fn remove_keeps_every_byte_of_a_comment_ending_in_a_comma() {
        // The input, the removal under test, and the value it must leave.
        type Row = (String, fn(&str) -> String, serde_json::Value);
        let grim = "{\"type\": \"command\", \"command\": \"run\", \"com.grimoire.managed\": \"hook-dispatcher\"}";
        let user = "{\"type\": \"command\", \"command\": \"user\"}";
        for comment in ["// note,", "// a, b, ", "//,"] {
            // The closing bracket on the removed item's line, then on its own.
            for close in ["", "\n"] {
                let rows: [Row; 9] = [
                    (
                        format!("{{\"mcp\": {{\"a\": 1, {comment}\n\"x\": 2{close}}}}}"),
                        (|t: &str| changed(remove_member_at(t, &path(&["mcp", "x"])).unwrap())),
                        json!({"mcp": {"a": 1}}),
                    ),
                    (
                        format!("{{\"mcp\": {{{comment}\n\"x\": 2{close}}}}}"),
                        (|t: &str| changed(remove_member_at(t, &path(&["mcp", "x"])).unwrap())),
                        json!({"mcp": {}}),
                    ),
                    (
                        format!("{{\"mcp\": {{\"a\": 1, {comment}\n\"x\": 2{close}}}}}"),
                        (|t: &str| changed(remove_member(t, "mcp", "x").unwrap())),
                        json!({"mcp": {"a": 1}}),
                    ),
                    // The only member takes its container, the only root member.
                    (
                        format!("{{{comment}\n\"mcp\": {{\"x\": 2}}{close}}}"),
                        (|t: &str| changed(remove_member(t, "mcp", "x").unwrap())),
                        json!({}),
                    ),
                    (
                        format!("{{\"k\": [\"a\", {comment}\n\"x\"{close}]}}"),
                        (|t: &str| remove_element(t, &path(&["k"]), "x")),
                        json!({"k": ["a"]}),
                    ),
                    (
                        format!("{{\"k\": [{comment}\n\"x\"{close}]}}"),
                        (|t: &str| remove_element(t, &path(&["k"]), "x")),
                        json!({"k": []}),
                    ),
                    (
                        format!("{{\"k\": [\"a\", {comment}\n\"x\"{close}]}}"),
                        (|t: &str| changed(remove_array_element(t, "k", "x").unwrap())),
                        json!({"k": ["a"]}),
                    ),
                    (
                        format!("{{{comment}\n\"k\": [\"x\"]{close}}}"),
                        (|t: &str| changed(remove_array_element(t, "k", "x").unwrap())),
                        json!({}),
                    ),
                    (
                        format!(
                            "{{\"hooks\": {{\"PreToolUse\": [{{\"matcher\": \"Bash\", \"hooks\": [{user}, {comment}\n{grim}{close}]}}]}}}}"
                        ),
                        |t: &str| {
                            let path = hook_path("PreToolUse", "Bash");
                            changed(remove_nested_handler(t, &path, &dispatcher("run")).unwrap())
                        },
                        json!({"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "user"}]}]}}),
                    ),
                ];
                for (text, remove, rest) in rows {
                    let out = remove(&text);
                    assert!(out.contains(comment), "the comment lost a byte: {text:?} gave {out:?}");
                    assert_eq!(parsed(&out), rest, "{text:?} gave {out:?}");
                    assert_eq!(strict_without(&out, comment), rest, "{text:?} gave {out:?}");
                }
            }
        }
    }

    /// Every removal over a bounded grid of comment placements: before and after
    /// each comma, after the opening bracket and before the closing one, for
    /// every comment shape and item position. Each result reads as the input
    /// minus the item, keeps every comment whole, and leaves no stray comma.
    #[test]
    fn every_removal_beside_any_comment_parses_and_keeps_the_comment() {
        const SHAPES: [&str; 6] = ["// k", "// k,", "// k, ", "//,", "/* k */", "/* k, */"];
        let mut cases = 0;
        for comment in SHAPES {
            let line = comment.starts_with("//");
            let end = if line { "\n" } else { " " };
            let gaps = [
                ", ".to_string(),
                ",\n".to_string(),
                format!(" {comment}{end}, "),
                format!(", {comment}{end}"),
            ];
            let heads = [String::new(), "\n".to_string(), format!("{comment}\n")];
            let tails = [String::new(), "\n".to_string(), format!(" {comment}\n")];
            for count in 1..=3usize {
                let gap_rows = (0..count.saturating_sub(1)).fold(vec![Vec::new()], |rows, _| {
                    rows.iter()
                        .flat_map(|row: &Vec<&String>| gaps.iter().map(move |gap| [row.clone(), vec![gap]].concat()))
                        .collect()
                });
                for gap_row in &gap_rows {
                    for head in &heads {
                        for tail in &tails {
                            for doomed in 0..count {
                                for (text, out, rest) in removal_rows(count, doomed, head, gap_row, tail) {
                                    let held = text.matches(comment).count();
                                    assert_eq!(out.matches(comment).count(), held, "{text:?} gave {out:?}");
                                    assert_eq!(parsed(&out), rest, "{text:?} gave {out:?}");
                                    let strict = out.replace(comment, "");
                                    let read: serde_json::Value = serde_json::from_str(&strict)
                                        .unwrap_or_else(|e| panic!("{e}: {text:?} gave {out:?}"));
                                    assert_eq!(read, rest, "{text:?} gave {out:?}");
                                    cases += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(cases > 5_000, "the grid shrank to {cases} cases");
    }

    /// A byte-order mark is no part of the document: every read and splice goes
    /// past it, and every write keeps it first.
    #[test]
    fn a_byte_order_mark_is_read_past_and_kept() {
        let bom = "\u{feff}";
        let text = format!("{bom}{{\n  // keep\n  \"mcp\": {{\n    \"a\": 1\n  }},\n  \"k\": [\"a\"]\n}}\n");
        assert_eq!(value_at(&text, &path(&["mcp", "a"])).unwrap(), found(json!(1)));
        assert_eq!(member_value(&text, "mcp", "a"), Some(json!(1)));
        let hooks = hook_path("PreToolUse", "Bash");
        // The splice under test, its insert, and the removal that undoes it.
        type Pair = (&'static str, Splice, fn(&str) -> Splice);
        let pairs: [Pair; 5] = [
            (
                "member at depth",
                upsert_member_at(&text, &path(&["mcp", "x"]), &json!(2)).unwrap(),
                |t| remove_member_at(t, &path(&["mcp", "x"])).unwrap(),
            ),
            ("member", upsert_member(&text, "mcp", "x", &json!(2)).unwrap(), |t| {
                remove_member(t, "mcp", "x").unwrap()
            }),
            (
                "element at depth",
                upsert_array_element_at(&text, &path(&["k"]), "b").unwrap(),
                |t| remove_array_element_at(t, &path(&["k"]), "b").unwrap(),
            ),
            ("element", upsert_array_element(&text, "k", "b").unwrap(), |t| {
                remove_array_element(t, "k", "b").unwrap()
            }),
            (
                "nested handler",
                upsert_nested_handler(&text, &hooks, &dispatcher("run")).unwrap(),
                |t| remove_nested_handler(t, &hook_path("PreToolUse", "Bash"), &dispatcher("run")).unwrap(),
            ),
        ];
        for (op, upserted, remove) in pairs {
            let out = changed(upserted);
            assert!(out.starts_with(bom) && out.matches(bom).count() == 1, "{op}: {out:?}");
            assert!(out.contains("// keep"), "{op}: {out:?}");
            assert_ne!(parsed(&out), parsed(&text), "{op}: {out:?}");
            let back = changed(remove(&out));
            assert!(
                back.starts_with(bom) && back.matches(bom).count() == 1,
                "{op}: {back:?}"
            );
            assert_eq!(parsed(&back), parsed(&text), "{op}: {back:?}");
        }
    }

    /// One container of `count` items laid out by `head`, `gaps` and `tail`,
    /// with item `doomed` removed by every removal that cuts inside it: the
    /// input, the output and the value the output must read as.
    fn removal_rows(
        count: usize,
        doomed: usize,
        head: &str,
        gaps: &[&String],
        tail: &str,
    ) -> Vec<(String, String, serde_json::Value)> {
        let lay = |items: Vec<String>, open: &str, close: &str| {
            let mut body = format!("{open}{head}");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    body.push_str(gaps[i - 1]);
                }
                body.push_str(item);
            }
            format!("{{\"c\": {body}{tail}{close}}}")
        };
        let members = lay((0..count).map(|i| format!("\"m{i}\": {i}")).collect(), "{", "}");
        let elements = lay((0..count).map(|i| format!("\"e{i}\"")).collect(), "[", "]");
        let mut kept = serde_json::Map::new();
        for i in (0..count).filter(|&i| i != doomed) {
            kept.insert(format!("m{i}"), json!(i));
        }
        let kept_elements: Vec<_> = (0..count).filter(|&i| i != doomed).map(|i| format!("e{i}")).collect();
        let (member, element) = (format!("m{doomed}"), format!("e{doomed}"));
        let mut rows = vec![
            (
                members.clone(),
                changed(remove_member_at(&members, &path(&["c", &member])).unwrap()),
                json!({"c": kept.clone()}),
            ),
            (
                elements.clone(),
                remove_element(&elements, &path(&["c"]), &element),
                json!({"c": kept_elements.clone()}),
            ),
        ];
        // The flat removals drop an emptied container whole, comments inside it too.
        if count > 1 {
            rows.push((
                members.clone(),
                changed(remove_member(&members, "c", &member).unwrap()),
                json!({"c": kept}),
            ));
            rows.push((
                elements.clone(),
                changed(remove_array_element(&elements, "c", &element).unwrap()),
                json!({"c": kept_elements}),
            ));
        }
        rows
    }

    #[test]
    fn key_with_quote_is_escaped() {
        let value = json!({"command": "x"});
        for key in ["a\"b", "a\",\"evil\":\"x", "back\\slash", "ctrl\u{1}", "end\""] {
            for text in ["", "{}\n", "{\n  \"c\": {\n    \"other\": {}\n  }\n}\n"] {
                let leaf = path(&["c", key]);
                let out = changed(upsert_member_at(text, &leaf, &value).unwrap());
                let document: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"));
                assert_eq!(document["c"][key], value, "{out}");
                assert!(document.get("evil").is_none(), "{out}");
                assert_eq!(upsert_member_at(&out, &leaf, &value).unwrap(), Splice::Unchanged);

                let container = path(&[key, "leaf"]);
                let out = changed(upsert_member_at(text, &container, &value).unwrap());
                let document: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"));
                assert_eq!(document[key]["leaf"], value, "{out}");
                assert_eq!(
                    parsed(&changed(remove_member_at(&out, &container).unwrap()))[key],
                    json!({}),
                    "an escaped key is found again"
                );
            }
        }
    }

    #[test]
    fn non_container_ancestor_is_invalid_data() {
        let value = json!(1);
        assert_eq!(
            changed(upsert_member_at("{\"a\": {}}", &path(&["a", "b"]), &value).unwrap()),
            "{\"a\": {\n  \"b\": 1\n}}",
            "control"
        );
        for text in [
            "{\"a\": 1}",
            "{\"a\": \"sentinel-value\"}",
            "{\"a\": true}",
            "{\"a\": null}",
            "{\"a\": [{\"b\": 1}]}",
        ] {
            for target in [path(&["a", "b"]), path(&["a", "b", "c"])] {
                let err = upsert_member_at(text, &target, &value).unwrap_err();
                assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{text}");
                assert!(!err.to_string().contains("sentinel-value"), "{err}");
            }
        }
        let deep = upsert_member_at("{\"a\": {\"b\": 1}}", &path(&["a", "b", "c"]), &value).unwrap_err();
        assert_eq!(deep.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn member_splices_refuse_an_unparseable_document() {
        let target = path(&["a"]);
        assert!(upsert_member_at("{}", &target, &json!(1)).is_ok(), "control");
        assert!(remove_member_at("{}", &target).is_ok(), "control");
        for text in [
            "not json {{{",
            "[1, 2]",
            "\"text\"",
            "3",
            "{\"a\":",
            "{\"token\": hunter2-sentinel}",
            "// only a comment\n",
        ] {
            for err in [
                upsert_member_at(text, &target, &json!(1)).unwrap_err(),
                remove_member_at(text, &target).unwrap_err(),
            ] {
                assert_eq!(err.kind(), io::ErrorKind::InvalidData, "input: {text}");
                assert!(!err.to_string().contains("hunter2-sentinel"), "{err}");
            }
        }
    }

    /// The whole document is not a member: a piece that addresses it would
    /// replace every other key in the file.
    #[test]
    fn member_splices_refuse_the_empty_path() {
        assert!(upsert_member_at("{}", &path(&["a"]), &json!(1)).is_ok(), "control");
        for text in ["", "{}", "{\"a\": 1}"] {
            let upsert = upsert_member_at(text, &[], &json!({"b": 2})).unwrap_err();
            assert_eq!(upsert.kind(), io::ErrorKind::InvalidData, "{text:?}");
            let remove = remove_member_at(text, &[]).unwrap_err();
            assert_eq!(remove.kind(), io::ErrorKind::InvalidData, "{text:?}");
        }
    }

    // ── Element splices at any depth ────────────────────────────────────

    fn upsert_element(text: &str, target: &[String], element: &str) -> String {
        changed(upsert_array_element_at(text, target, element).unwrap())
    }

    fn remove_element(text: &str, target: &[String], element: &str) -> String {
        changed(remove_array_element_at(text, target, element).unwrap())
    }

    /// Every comment the layout tables below carry. A splice keeps exactly the
    /// ones its input held.
    const TABLE_COMMENTS: &[&str] = &[
        "// head",
        "// tail",
        "// line",
        "// more",
        "// only a comment",
        "/* block */",
        "/* block\n  spanning */",
        "/* x */",
        "/* y */",
        "/* z */",
    ];

    fn assert_comments_survive(text: &str, out: &str) {
        for comment in TABLE_COMMENTS {
            assert_eq!(out.contains(comment), text.contains(comment), "{comment} in {out:?}");
        }
    }

    #[test]
    fn element_upsert_is_idempotent() {
        let target = path(&["permissions", "deny"]);
        let text = "{\n  \"permissions\": {\n    \"deny\": [\n      \"Read\"\n    ]\n  }\n}\n";

        let once = upsert_element(text, &target, "Bash(rm:*)");
        assert_eq!(
            once,
            "{\n  \"permissions\": {\n    \"deny\": [\n      \"Read\",\n      \"Bash(rm:*)\"\n    ]\n  }\n}\n"
        );
        for element in ["Bash(rm:*)", "Read"] {
            assert_eq!(
                upsert_array_element_at(&once, &target, element).unwrap(),
                Splice::Unchanged,
                "{element}"
            );
        }

        let gone = remove_element(&once, &target, "Bash(rm:*)");
        assert_eq!(gone, text);
        assert_eq!(
            remove_array_element_at(&gone, &target, "Bash(rm:*)").unwrap(),
            Splice::Unchanged
        );
    }

    #[test]
    fn element_upsert_creates_the_array() {
        let target = path(&["a", "b", "c"]);
        let skeleton = "{\n  \"a\": {\n    \"b\": {\n      \"c\": [\n        \"x\"\n      ]\n    }\n  }\n}\n";
        assert_eq!(upsert_element("", &target, "x"), skeleton);
        assert_eq!(upsert_element("{}\n", &target, "x"), skeleton);
        assert_eq!(
            upsert_element(skeleton, &target, "y"),
            "{\n  \"a\": {\n    \"b\": {\n      \"c\": [\n        \"x\",\n        \"y\"\n      ]\n    }\n  }\n}\n",
            "an array made here is an array the next element finds"
        );

        let partial = "{\n  \"a\": {\n    \"keep\": 1\n  },\n  \"z\": 2\n}\n";
        assert_eq!(
            upsert_element(partial, &target, "x"),
            "{\n  \"a\": {\n    \"keep\": 1,\n    \"b\": {\n      \"c\": [\n        \"x\"\n      ]\n    }\n  },\n  \"z\": 2\n}\n"
        );

        for text in ["", "  \n", "{}\n", partial] {
            let out = upsert_element(text, &target, "x");
            assert_eq!(value_at(&out, &target).unwrap(), found(json!(["x"])), "{out}");
            assert_eq!(
                upsert_array_element_at(&out, &target, "x").unwrap(),
                Splice::Unchanged,
                "{out}"
            );
        }
    }

    #[test]
    fn element_ops_keep_array_comments() {
        let target = path(&["deny"]);
        let text = "{\n  \"deny\": [\n    // shell\n    \"Bash\", // never\n    /* files */\n    \"Write\" // last\n    // tail\n  ]\n}\n";
        let comments = ["// shell", "// never", "/* files */", "// last", "// tail"];

        assert_eq!(
            upsert_element(text, &target, "Read"),
            "{\n  \"deny\": [\n    // shell\n    \"Bash\", // never\n    /* files */\n    \"Write\", // last\n    // tail\n    \"Read\"\n  ]\n}\n",
            "a comment stays on its own element's line"
        );

        for (element, rest) in [("Bash", json!(["Write"])), ("Write", json!(["Bash"]))] {
            let out = remove_element(text, &target, element);
            assert_eq!(parsed(&out)["deny"], rest, "{out:?}");
            for comment in comments {
                assert!(out.contains(comment), "{comment} in {out:?}");
            }
        }

        for (text, want) in [
            (
                "{\"a\": [\n  // only a comment\n]}",
                "{\"a\": [\n  // only a comment\n  \"x\"\n]}",
            ),
            ("{\"a\": [ /* c */ ]}", "{\"a\": [ /* c */ \"x\"]}"),
        ] {
            let out = upsert_element(text, &path(&["a"]), "x");
            assert_eq!(out, want);
            assert_eq!(
                remove_element(&out, &path(&["a"]), "x").replace(' ', ""),
                text.replace(' ', "")
            );
        }
    }

    /// `Set` removal stops at the element: dropping the emptied array is
    /// `PruneEmpty`'s step, because only the writer knows which keys are required.
    #[test]
    fn element_remove_last_leaves_empty_array() {
        let target = path(&["a", "b"]);
        assert_eq!(
            remove_element("{\"a\": {\"b\": [\"x\"]}, \"z\": 1}", &target, "x"),
            "{\"a\": {\"b\": []}, \"z\": 1}"
        );
        for text in [
            "{\"a\": {\"b\": [\"x\"]}}",
            "{\"a\": {\"b\": [\"x\",]}}",
            "{\"a\": {\"b\": [ /* keep */ \"x\"]}}",
            "{\n  \"a\": {\n    \"b\": [\n      \"x\"\n    ]\n  }\n}\n",
            "{\r\n\t\"a\": {\r\n\t\t\"b\": [\r\n\t\t\t\"x\",\r\n\t\t]\r\n\t}\r\n}\r\n",
        ] {
            let out = remove_element(text, &target, "x");
            assert_eq!(value_at(&out, &target).unwrap(), found(json!([])), "{out:?}");
            assert_eq!(
                remove_array_element_at(&out, &target, "x").unwrap(),
                Splice::Unchanged,
                "{out:?}"
            );
            assert_eq!(out.contains("/* keep */"), text.contains("/* keep */"), "{out:?}");
        }
    }

    #[test]
    fn element_upsert_keeps_the_layout_of_the_array() {
        let target = path(&["a"]);
        for (text, want) in [
            ("{\"a\": []}", "{\"a\": [\"x\"]}"),
            ("{\"a\": [ ]}", "{\"a\": [\"x\"]}"),
            ("{\"a\": [\"p\", \"q\"]}", "{\"a\": [\"p\", \"q\", \"x\"]}"),
            ("{\"a\": [\"p\",]}", "{\"a\": [\"p\", \"x\",]}"),
            ("{\n  \"a\": [\n  ]\n}\n", "{\n  \"a\": [\n    \"x\"\n  ]\n}\n"),
            (
                "{\n  \"a\": [\n    \"p\",\n  ]\n}\n",
                "{\n  \"a\": [\n    \"p\",\n    \"x\",\n  ]\n}\n",
            ),
            (
                "{\n  \"a\": [\n    \"p\", \"q\"\n  ]\n}\n",
                "{\n  \"a\": [\n    \"p\", \"q\",\n    \"x\"\n  ]\n}\n",
            ),
            (
                "{\n    \"a\": [\n        \"p\"\n    ]\n}\n",
                "{\n    \"a\": [\n        \"p\",\n        \"x\"\n    ]\n}\n",
            ),
        ] {
            assert_eq!(upsert_element(text, &target, "x"), want, "{text:?}");
        }
    }

    #[test]
    fn element_upsert_keeps_crlf_and_tabs() {
        let target = path(&["a"]);
        assert_eq!(
            upsert_element("{\r\n\t\"a\": [\r\n\t\t\"b\"\r\n\t]\r\n}\r\n", &target, "c"),
            "{\r\n\t\"a\": [\r\n\t\t\"b\",\r\n\t\t\"c\"\r\n\t]\r\n}\r\n"
        );
        assert_eq!(
            upsert_element("{\r\n\t\"a\": [\r\n\t]\r\n}\r\n", &target, "c"),
            "{\r\n\t\"a\": [\r\n\t\t\"c\"\r\n\t]\r\n}\r\n"
        );
        assert_eq!(
            upsert_element("{\r\n\t\"z\": 1\r\n}\r\n", &path(&["a", "b"]), "c"),
            "{\r\n\t\"z\": 1,\r\n\t\"a\": {\r\n\t\t\"b\": [\r\n\t\t\t\"c\"\r\n\t\t]\r\n\t}\r\n}\r\n"
        );
        assert_eq!(
            remove_element(
                "{\r\n\t\"a\": [\r\n\t\t\"b\",\r\n\t\t\"c\"\r\n\t]\r\n}\r\n",
                &target,
                "b"
            ),
            "{\r\n\t\"a\": [\r\n\t\t\"c\"\r\n\t]\r\n}\r\n"
        );
        assert_eq!(
            remove_element(
                "{\r\n\t\"a\": [\r\n\t\t\"b\",\r\n\t\t\"c\"\r\n\t]\r\n}\r\n",
                &target,
                "c"
            ),
            "{\r\n\t\"a\": [\r\n\t\t\"b\"\r\n\t]\r\n}\r\n"
        );
    }

    /// A shape-agnostic check over layouts the scanner has to walk: the file's
    /// meaning changes at the addressed array and nowhere else, both ways.
    #[test]
    fn element_splices_change_only_the_addressed_array() {
        let target = path(&["a", "list"]);
        for text in [
            "{\"a\": {\"list\": []}}",
            "{ \"a\": { \"list\": [ ] } }",
            "{\"a\": {\"list\": [\n]}}",
            "{\n\t\"a\": {\n\t\t\"list\": [\n\t\t]\n\t}\n}\n",
            "{\"a\": {\"list\": [\"p\"]}}",
            "{\"a\": {\"list\": [\"p\",]}}",
            "{\"a\": {\"list\": [\"p\", \"q\"], \"z\": 1}, \"y\": 2}",
            "{\"a\": {\"list\": [\n  \"p\",\n  \"q\",\n]}}",
            "{\"a\": {\"list\": [\"p\" // line\n]}}",
            "{\"a\": {\"list\": [\"p\" // line\n  // more\n], \"z\": 1}}",
            "{\"a\": {\"list\": [\"p\" /* x */, /* y */ \"q\" /* z */]}}",
            "{\"a\": {\"list\": [\"p\" /* x */, /* y */]}}",
            "{\"a\": {\"list\": [\n  // only a comment\n]}}",
            "{\"a\": {\"list\": [ /* block */ ]}}",
            "{\"a\": {\"list\": [ /* block\n  spanning */ ]}}",
            "{\"a\": {\"list\": [1/* x */, {\"k\": \"]\"}, [\"new\"], null, \"//no\"]}}",
            "{\"a\": {\"list\": [\"p\"// line\n]}}",
            "// head\n{\"a\": {\"list\": [\"p\"]}} // tail\n",
            "{\"a\": {\"list\": [\"p\" // ideographic space\u{3000}\n]}}",
            "{\"a\": {\"list\": [ // no-break space\u{a0}\n]}}",
            "{\r\n\t\"a\": {\r\n\t\t\"list\": [\r\n\t\t\t\"p\"\r\n\t\t]\r\n\t}\r\n}\r\n",
        ] {
            let base = parsed(text);
            let mut expected = base.clone();
            expected["a"]["list"]
                .as_array_mut()
                .expect("every row holds the array")
                .push(json!("new"));

            let out = upsert_element(text, &target, "new");
            assert_eq!(parsed(&out), expected, "upsert of {text:?} gave {out:?}");
            assert_comments_survive(text, &out);
            assert_eq!(
                upsert_array_element_at(&out, &target, "new").unwrap(),
                Splice::Unchanged,
                "{out:?}"
            );

            let back = remove_element(&out, &target, "new");
            assert_eq!(parsed(&back), base, "remove from {out:?} gave {back:?}");
            assert_comments_survive(text, &back);
        }
    }

    /// A comment may sit on either side of the comma that separates an element
    /// from its neighbour, and either comma going wrong leaves an unreadable file.
    #[test]
    fn element_remove_cuts_one_element_and_one_comma() {
        let target = path(&["a"]);
        for (text, rest) in [
            ("{\"a\": [\"x\", \"y\"]}", json!(["y"])),
            ("{\"a\": [\"y\", \"x\"]}", json!(["y"])),
            ("{\"a\": [\"p\", \"x\", \"y\"]}", json!(["p", "y"])),
            ("{\"a\": [\"p\", \"x\",]}", json!(["p"])),
            ("{\"a\": [\"x\" /* c */, \"y\"]}", json!(["y"])),
            ("{\"a\": [\"p\", \"x\" /* c */, \"y\"]}", json!(["p", "y"])),
            ("{\"a\": [\"p\", /* b */ \"x\" /* c */, \"y\"]}", json!(["p", "y"])),
            ("{\"a\": [\"p\", /* b */ \"x\" /* c */]}", json!(["p"])),
            ("{\"a\": [\"p\", /* b */ \"x\"]}", json!(["p"])),
            ("{\"a\": [\"x\" // c\n, \"y\"]}", json!(["y"])),
            ("{\"a\": [\n  \"x\", // c\n  \"y\" // d\n]}", json!(["y"])),
            ("{\"a\": [\n  \"y\", // c\n  \"x\" // d\n]}", json!(["y"])),
            ("{\"a\": [\"x\" /* c */,]}", json!([])),
            ("{\"a\": [\"x\" /* b */, /* c */ \"x\"]}", json!([])),
        ] {
            let out = remove_element(text, &target, "x");
            assert_eq!(parsed(&out)["a"], rest, "{text:?} gave {out:?}");
            for comment in ["/* b */", "/* c */", "// c", "// d"] {
                assert_eq!(out.contains(comment), text.contains(comment), "{comment} in {out:?}");
            }
        }
    }

    #[test]
    fn element_remove_leaves_siblings() {
        let target = path(&["p", "deny"]);
        let text = "{\n  // keep\n  \"p\": {\n    \"allow\": [\"drop\"],\n    \"deny\": [\n      \"drop\",\n      \"stay\" // note\n    ],\n    \"c\": 3\n  }\n}\n";
        assert_eq!(
            remove_element(text, &target, "drop"),
            "{\n  // keep\n  \"p\": {\n    \"allow\": [\"drop\"],\n    \"deny\": [\n      \"stay\" // note\n    ],\n    \"c\": 3\n  }\n}\n"
        );
        for absent in [path(&["p", "z"]), path(&["z", "y"]), path(&["p", "allow", "z"])] {
            assert_eq!(
                remove_array_element_at(text, &absent, "drop").unwrap(),
                Splice::Unchanged,
                "{absent:?}"
            );
        }
        assert_eq!(
            remove_array_element_at(text, &target, "absent").unwrap(),
            Splice::Unchanged
        );
        for text in ["", "  \n"] {
            assert_eq!(remove_array_element_at(text, &target, "x").unwrap(), Splice::Unchanged);
        }
    }

    /// A read finds an element wherever it sits, so one survivor of a list that
    /// held it twice would keep reading as present.
    #[test]
    fn element_remove_takes_every_duplicate() {
        let target = path(&["a"]);
        let text = "{\n  \"a\": [\n    \"x\",\n    \"y\",\n    \"\\u0078\",\n    \"x\"\n  ]\n}\n";
        assert_eq!(remove_element(text, &target, "x"), "{\n  \"a\": [\n    \"y\"\n  ]\n}\n");
        assert_eq!(
            value_at(&remove_element("{\"a\": [\"x\", \"x\"]}", &target, "x"), &target).unwrap(),
            found(json!([]))
        );
    }

    #[test]
    fn element_identity_ignores_the_spelling() {
        let target = path(&["a"]);
        let text = "{\"a\": [\"\\u0042ash\"]}";
        assert_eq!(value_at(text, &target).unwrap(), found(json!(["Bash"])), "control");
        assert_eq!(
            upsert_array_element_at(text, &target, "Bash").unwrap(),
            Splice::Unchanged
        );
        assert_eq!(remove_element(text, &target, "Bash"), "{\"a\": []}");
    }

    #[test]
    fn element_identity_is_a_whole_string() {
        let target = path(&["a"]);
        let text = "{\"a\": [1, true, null, {\"x\": \"x\"}, [\"x\"], \"xx\", \" x\"]}";
        let held = parsed(text)["a"].as_array().expect("control").len();
        assert_ne!(remove_array_element_at(text, &target, "xx").unwrap(), Splice::Unchanged);

        for element in ["x", "1", "true", "null"] {
            assert_eq!(
                remove_array_element_at(text, &target, element).unwrap(),
                Splice::Unchanged,
                "{element}"
            );
            let out = upsert_element(text, &target, element);
            let array = parsed(&out)["a"].as_array().expect("still an array").clone();
            assert_eq!(array.len(), held + 1, "{out}");
            assert_eq!(array.last(), Some(&json!(element)), "{out}");
        }
    }

    #[test]
    fn element_with_quote_is_escaped() {
        let target = path(&["c", "list"]);
        for element in [
            "a\"b",
            "x\"],\"evil\":[\"y",
            "back\\slash",
            "ctrl\u{1}",
            "line\nfeed",
            "end\"",
            "ü",
        ] {
            for text in [
                "",
                "{}\n",
                "{\"c\": {\"list\": []}}",
                "{\n  \"c\": {\n    \"list\": [\n      \"other\"\n    ]\n  }\n}\n",
            ] {
                let out = upsert_element(text, &target, element);
                let document: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"));
                let held = document["c"]["list"].as_array().expect("still an array");
                assert!(held.contains(&json!(element)), "{out}");
                assert!(document.get("evil").is_none(), "{out}");
                assert_eq!(
                    upsert_array_element_at(&out, &target, element).unwrap(),
                    Splice::Unchanged,
                    "{out}"
                );

                let back = parsed(&remove_element(&out, &target, element));
                let kept = back["c"]["list"].as_array().expect("still an array");
                assert!(!kept.contains(&json!(element)), "an escaped element is found again");
            }
        }
    }

    #[test]
    fn element_ops_follow_the_last_duplicate_key() {
        let target = path(&["a"]);
        let text = "{\"a\": [\"p\"], \"a\": [\"q\"]}";
        assert_eq!(
            upsert_element(text, &target, "x"),
            "{\"a\": [\"p\"], \"a\": [\"q\", \"x\"]}"
        );
        assert_eq!(
            remove_array_element_at(text, &target, "p").unwrap(),
            Splice::Unchanged,
            "a shadowed array reads as absent"
        );
        assert_eq!(remove_element(text, &target, "q"), "{\"a\": [\"p\"], \"a\": []}");
    }

    #[test]
    fn element_ops_find_a_key_spelled_with_escapes() {
        let target = path(&["pAth", "list"]);
        let text = "{\"p\\u0041th\": {\"list\": [\"p\"]}}";
        assert_eq!(
            upsert_element(text, &target, "x"),
            "{\"p\\u0041th\": {\"list\": [\"p\", \"x\"]}}"
        );
        assert_eq!(remove_element(text, &target, "p"), "{\"p\\u0041th\": {\"list\": []}}");
    }

    /// A bare string equal to the element is not the element: the path holds a
    /// value of another type, and only a caller allowed to replace it may.
    #[test]
    fn element_ops_refuse_a_non_array() {
        let target = path(&["a", "b"]);
        assert!(
            upsert_array_element_at("{\"a\": {\"b\": []}}", &target, "x").is_ok(),
            "control"
        );
        assert!(
            remove_array_element_at("{\"a\": {\"b\": []}}", &target, "x").is_ok(),
            "control"
        );
        for text in [
            "{\"a\": {\"b\": 1}}",
            "{\"a\": {\"b\": \"x\"}}",
            "{\"a\": {\"b\": \"sentinel-value\"}}",
            "{\"a\": {\"b\": true}}",
            "{\"a\": {\"b\": null}}",
            "{\"a\": {\"b\": {\"x\": \"x\"}}}",
        ] {
            for err in [
                upsert_array_element_at(text, &target, "x").unwrap_err(),
                remove_array_element_at(text, &target, "x").unwrap_err(),
            ] {
                assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{text}");
                assert!(!err.to_string().contains("sentinel-value"), "{err}");
            }
        }
    }

    #[test]
    fn element_upsert_refuses_a_non_container_ancestor() {
        let target = path(&["a", "b", "c"]);
        assert!(upsert_array_element_at("{\"a\": {}}", &target, "x").is_ok(), "control");
        for text in [
            "{\"a\": 1}",
            "{\"a\": null}",
            "{\"a\": [\"x\"]}",
            "{\"a\": {\"b\": \"sentinel-value\"}}",
        ] {
            let err = upsert_array_element_at(text, &target, "x").unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{text}");
            assert!(!err.to_string().contains("sentinel-value"), "{err}");
        }
    }

    /// Nothing sits at the path, so the element is absent, as it is for a member
    /// under a value of another type.
    #[test]
    fn element_remove_under_a_non_container_is_unchanged() {
        let target = path(&["a", "b"]);
        for text in ["{\"a\": 1}", "{\"a\": [\"x\"]}", "{\"a\": null}", "{\"a\": \"x\"}"] {
            assert_eq!(value_at(text, &target).unwrap(), blocked(&["a"]), "control");
            assert_eq!(
                remove_array_element_at(text, &target, "x").unwrap(),
                Splice::Unchanged,
                "{text}"
            );
        }
    }

    #[test]
    fn element_splices_refuse_an_unparseable_document() {
        let target = path(&["a"]);
        assert!(upsert_array_element_at("{}", &target, "x").is_ok(), "control");
        assert!(remove_array_element_at("{}", &target, "x").is_ok(), "control");
        for text in [
            "not json {{{",
            "[1, 2]",
            "\"text\"",
            "3",
            "{\"a\":",
            "{\"a\": [\"x\"",
            "{\"a\": [/ ]}",
            "{\"token\": hunter2-sentinel}",
            "// only a comment\n",
        ] {
            for err in [
                upsert_array_element_at(text, &target, "x").unwrap_err(),
                remove_array_element_at(text, &target, "x").unwrap_err(),
            ] {
                assert_eq!(err.kind(), io::ErrorKind::InvalidData, "input: {text}");
                assert!(!err.to_string().contains("hunter2-sentinel"), "{err}");
            }
        }
    }

    #[test]
    fn element_splices_refuse_the_empty_path() {
        assert!(upsert_array_element_at("{}", &path(&["a"]), "x").is_ok(), "control");
        for text in ["", "{}", "{\"a\": [\"x\"]}"] {
            let upsert = upsert_array_element_at(text, &[], "x").unwrap_err();
            assert_eq!(upsert.kind(), io::ErrorKind::InvalidData, "{text:?}");
            let remove = remove_array_element_at(text, &[], "x").unwrap_err();
            assert_eq!(remove.kind(), io::ErrorKind::InvalidData, "{text:?}");
        }
    }
}
