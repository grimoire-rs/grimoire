// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The `grim-hook <client> <event> <root>` argv contract (C-N4), defined once.
//!
//! grim's registration builder calls [`render`] and the launcher calls [`parse`],
//! so the two cannot drift. The grammar is frozen: it may widen a charset but
//! never gains an operand or a flag, because one launcher serves every project on
//! the machine.

use std::ffi::OsString;
use std::fmt;

use crate::table::RootToken;

/// Longest `<client>`: a leading letter plus 31 of `[a-z0-9-]`.
pub const CLIENT_MAX_BYTES: usize = 32;
/// Longest `<event>`: a leading letter plus 63 of `[A-Za-z0-9._-]`.
pub const EVENT_MAX_BYTES: usize = 64;

/// A validated invocation: the three operands, grammar-checked.
///
/// `root` is a [`RootToken`], so the type carries the `[0-9a-f]{32}` proof and the
/// table lookup compares it without re-validating. Named apart from
/// `pipeline::Invocation`, which is the audit-side view of a dispatch in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// `[a-z][a-z0-9-]{0,31}`: a grim vendor name.
    pub client: String,
    /// `[A-Za-z][A-Za-z0-9._-]{0,63}`: the event string grim registered. It may
    /// name no canonical event; that selects no row and is not an error.
    pub event: String,
    /// `[0-9a-f]{32}`: the opaque root token.
    pub root: RootToken,
}

/// Why [`parse`] refused the operands. Every variant maps to exit 64.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ArgvError {
    /// Not exactly three operands (flags and `--` count as operands here, so they
    /// also land in a grammar variant when the count is right).
    OperandCount(usize),
    /// The operand at this zero-based index is not valid UTF-8.
    NotUtf8(usize),
    /// `<client>` is outside `[a-z][a-z0-9-]{0,31}`, empty included.
    Client,
    /// `<event>` is outside `[A-Za-z][A-Za-z0-9._-]{0,63}`.
    Event,
    /// `<root>` is outside `[0-9a-f]{32}`.
    Root,
}

impl fmt::Display for ArgvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OperandCount(n) => write!(f, "expected 3 operands, got {n}"),
            Self::NotUtf8(i) => write!(f, "operand {i} is not valid UTF-8"),
            Self::Client => f.write_str("client does not match [a-z][a-z0-9-]{0,31}"),
            Self::Event => f.write_str("event does not match [A-Za-z][A-Za-z0-9._-]{0,63}"),
            Self::Root => f.write_str("root does not match [0-9a-f]{32}"),
        }
    }
}

impl std::error::Error for ArgvError {}

/// The three operands grim registers for `(client, event, root)`, verbatim.
///
/// No quoting and no validation: every operand is grammar-safe by construction
/// when it comes from a vendor name, a registered event and a minted token, and
/// [`parse`] is where the grammar is enforced.
pub fn render(client: &str, event: &str, root: &str) -> [String; 3] {
    [client.to_owned(), event.to_owned(), root.to_owned()]
}

/// Parse the operands that follow `argv[0]`.
///
/// # Errors
///
/// [`ArgvError`] on a wrong operand count, a non-UTF-8 operand, or any operand
/// outside its grammar. There are no flags: `--` and `-x` are operands and fail
/// the grammar like any other.
pub fn parse(args: &[OsString]) -> Result<Invocation, ArgvError> {
    let [client, event, root] = args else {
        return Err(ArgvError::OperandCount(args.len()));
    };
    fn utf8(slot: usize, value: &OsString) -> Result<&str, ArgvError> {
        value.to_str().ok_or(ArgvError::NotUtf8(slot))
    }
    let (client, event, root) = (utf8(0, client)?, utf8(1, event)?, utf8(2, root)?);

    if !grammar(client, CLIENT_MAX_BYTES, u8::is_ascii_lowercase, |b| {
        b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-'
    }) {
        return Err(ArgvError::Client);
    }
    if !grammar(event, EVENT_MAX_BYTES, u8::is_ascii_alphabetic, |b| {
        b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')
    }) {
        return Err(ArgvError::Event);
    }
    let root = RootToken::parse(root).ok_or(ArgvError::Root)?;
    Ok(Invocation {
        client: client.to_owned(),
        event: event.to_owned(),
        root,
    })
}

/// `first rest*` within `max` bytes; an empty operand fails `first`.
fn grammar(value: &str, max: usize, first: fn(&u8) -> bool, rest: fn(&u8) -> bool) -> bool {
    let bytes = value.as_bytes();
    bytes.len() <= max && bytes.first().is_some_and(first) && bytes[1..].iter().all(rest)
}

#[cfg(test)]
mod contract_tests;
