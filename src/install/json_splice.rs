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

use super::json_config::{invalid_data, sanitize_jsonc};

/// Split a two-level RFC-6901-style pointer (`/container/member`) into its
/// `(container, member)` pair. `None` for any other shape — the splice
/// operations manage exactly one nesting level.
pub fn split_pointer(pointer: &str) -> Option<(&str, &str)> {
    let rest = pointer.strip_prefix('/')?;
    let (container, member) = rest.split_once('/')?;
    (!container.is_empty() && !member.is_empty() && !member.contains('/')).then_some((container, member))
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
            "{{\n  {key}: {{\n    {inner}: {rendered}\n  }}\n}}\n",
            key = json_key(container),
            inner = json_key(member),
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
            "{key}: {{\n{inner}{name}: {rendered}\n{close}}}",
            key = json_key(container),
            name = json_key(member),
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
            let snippet = format!("{key}: {rendered}", key = json_key(member));
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
        let cut = cut_range(text, container_member.key_quote, container_member.value.end);
        let mut out = String::with_capacity(text.len());
        out.push_str(&text[..cut.start]);
        out.push_str(&text[cut.end..]);
        return Ok(Splice::Changed(out));
    }

    let cut = cut_range(inner_text, existing.key_quote, existing.value.end);
    let base = container_member.value.start;
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..base + cut.start]);
    out.push_str(&text[base + cut.end..]);
    Ok(Splice::Changed(out))
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
            name = json_key(key),
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
            name = json_key(key),
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

    let mut out = String::with_capacity(text.len());
    if array.elements.len() == 1 {
        let cut = cut_range(text, key_member.key_quote, key_member.value.end);
        out.push_str(&text[..cut.start]);
        out.push_str(&text[cut.end..]);
        return Ok(Splice::Changed(out));
    }
    let cut = cut_range(array_text, found.start, found.end);
    let base = key_member.value.start;
    out.push_str(&text[..base + cut.start]);
    out.push_str(&text[base + cut.end..]);
    Ok(Splice::Changed(out))
}

/// `element` rendered as a JSON string literal (escaped).
fn json_string(element: &str) -> String {
    serde_json::Value::String(element.to_string()).to_string()
}

/// `key` rendered as a JSON **object key** — the quoted, escaped literal,
/// ready to be written immediately before its `:`.
///
/// Every key grim writes into a managed config must go through this rather
/// than through `format!("\"{key}\"")`. A key carrying `"`, `\` or a
/// control character can otherwise close its own string literal and inject
/// arbitrary structure into a file grim does not own (GitHub #56,
/// CWE-116/CWE-74). Escaping here is deliberately independent of any
/// validation one layer up: #56's whole lesson is that a constraint owned
/// by a higher layer is not the layer to rely on. Nothing upstream
/// constrains this input today — the managed `member` is the config
/// binding name, an *unvalidated* map key (`config::project_config`
/// validates the binding's **value**, never its key), and TOML permits a
/// quoted key carrying any of the three escapable classes.
///
/// **Principle 9 / self-heal.** Escaping is the identity function exactly
/// on strings containing none of `"`, `\`, `U+0000..U+001F`: `serde_json`
/// escapes those three classes and nothing else — never `/`, never
/// non-ASCII. Every key whose pre-fix output was valid, re-readable JSON
/// therefore re-materializes byte-identically and leaves `status`
/// not-modified. A key needing escape never produced a re-readable prior
/// state in the first place (an unescaped `"` or control character makes
/// the document unparsable, so [`upsert_member`] already refuses it; an
/// unescaped `\` decodes to a *different* key than the one looked up, so
/// grim already re-inserts on every run), so for those there is no
/// byte-identical guarantee to break.
fn json_key(key: &str) -> String {
    // Identical mechanism to `json_string` — a JSON object key *is* a string
    // literal. Two names because the call sites are not interchangeable: one
    // writes an array element, the other writes the text immediately before a
    // `:`, and a future reader grepping for "where do keys get escaped" must
    // land on a function whose name says key.
    json_string(key)
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
#[allow(dead_code, reason = "constructed by the vendor hook seam (WP-I/WP-J2)")]
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
#[allow(dead_code, reason = "constructed by the vendor hook seam (WP-I/WP-J2)")]
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
#[allow(dead_code, reason = "the vendor hook seam (WP-I/WP-J2) is the production consumer")]
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
                    let snippet = format!("{key}: {rendered}", key = json_key(group.elements_key));
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
// `allow` not `expect` — see `upsert_nested_handler`. Production consumer still owed.
#[allow(dead_code, reason = "the vendor hook seam (WP-I/WP-J2) is the production consumer")]
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
            let cut = cut_range(group_text, elements_entry.key_quote, elements_entry.value.end);
            let new_group_text = cut_out(group_text, &cut);
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
        cut_out(array_text, &cut_range(array_text, group_span.start, group_span.end))
    } else {
        let cut = cut_range(elements_text, element_span.start, element_span.end);
        let new_elements_text = cut_out(elements_text, &cut);
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
// `allow` not `expect` — see `upsert_nested_handler`. Production consumer still owed.
#[allow(dead_code, reason = "the vendor hook seam (WP-I/WP-J2) is the production consumer")]
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
// `allow` not `expect` — see `upsert_nested_handler`. Production consumer still owed.
#[allow(dead_code, reason = "the vendor hook seam (WP-I/WP-J2) is the production consumer")]
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

/// Parse a (possibly JSONC) value span semantically.
fn parse_value(span: &str) -> Option<serde_json::Value> {
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

/// The byte range to delete for the span `start..end`: the span itself, its
/// separating comma (trailing when present, else the preceding one), and
/// the whitespace that would otherwise leave a blank line. Shared by object
/// members (`start` = the key's opening quote) and array elements.
fn cut_range(text: &str, start: usize, end: usize) -> Range<usize> {
    let bytes = text.as_bytes();
    let mut start = start;
    let mut end = end;

    // Trailing comma (plus horizontal whitespace before it)?
    let mut j = end;
    while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t' || bytes[j] == b'\r' || bytes[j] == b'\n') {
        j += 1;
    }
    let has_trailing_comma = j < bytes.len() && bytes[j] == b',';
    if has_trailing_comma {
        end = j + 1;
    } else {
        // Last member: eat the preceding comma when only whitespace
        // separates it (comments in between are preserved by leaving the
        // comma alone — a JSONC trailing comma is tolerated by rescans).
        let mut k = start;
        while k > 0 && (bytes[k - 1] == b' ' || bytes[k - 1] == b'\t' || bytes[k - 1] == b'\r' || bytes[k - 1] == b'\n')
        {
            k -= 1;
        }
        if k > 0 && bytes[k - 1] == b',' {
            start = k - 1;
        }
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
    let mut s = Scanner {
        bytes: text.as_bytes(),
        pos: 0,
    };
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
    let mut s = Scanner {
        bytes: text.as_bytes(),
        pos: 0,
    };
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

fn refused() -> io::Error {
    invalid_data("content is not a JSON object grim can edit; refusing to touch it".to_string())
}

struct Scanner<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Scanner<'_> {
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
                // delimiter.
                while self
                    .peek()
                    .is_some_and(|b| !b.is_ascii_whitespace() && b != b',' && b != b'}' && b != b']')
                {
                    self.pos += 1;
                }
            }
            None => return Err(refused()),
        }
        Ok(start..self.pos)
    }
}

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
            assert_eq!(json_key(name), format!("\"{name}\""), "name: {name:?}");
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
}
