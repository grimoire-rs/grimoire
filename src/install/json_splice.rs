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
            "{{\n  \"{container}\": {{\n    \"{member}\": {rendered}\n  }}\n}}\n"
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
            "\"{container}\": {{\n{inner}\"{member}\": {rendered}\n{close}}}",
            inner = deeper(&root.member_indent(text)),
            close = root.member_indent(text),
        );
        return Ok(Splice::Changed(insert_member(text, &root, &snippet)));
    };

    let inner_text = &text[container_member.value.clone()];
    if !inner_text.trim_start().starts_with('{') {
        return Err(invalid_data(format!(
            "'{container}' is not a JSON object; refusing to edit"
        )));
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
            let snippet = format!("\"{member}\": {rendered}");
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
        return Err(invalid_data(format!(
            "'{container}' is not a JSON object; refusing to edit"
        )));
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
            "{{\n  \"{key}\": [\n    {rendered}\n  ]\n}}\n"
        )));
    }
    if !parse_value(text).is_some_and(|v| v.is_object()) {
        return Err(refused());
    }
    let root = scan_object(text)?;
    let Some(key_member) = last_member(&root.members, key) else {
        // Insert the whole array as a new root member.
        let indent = root.member_indent(text);
        let snippet = format!("\"{key}\": [\n{inner}{rendered}\n{indent}]", inner = deeper(&indent));
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

/// Whether the element at `span` parses to exactly the string `element`.
fn element_matches(text: &str, span: &Range<usize>, element: &str) -> bool {
    parse_value(&text[span.clone()]).and_then(|v| v.as_str().map(str::to_string)) == Some(element.to_string())
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
}
