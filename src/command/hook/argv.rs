// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The untrusted-argv contract for `grim hook run` (C-006's runtime half,
//! WP-P0 **B1** and **B3**).
//!
//! **Every value in the launcher argv is untrusted input.** The registration
//! grim writes is not the only registration a client reads: a hostile
//! repository can commit its own, naming the victim's real launcher with an
//! attacker-chosen client, event and root (WP-B § 6.1 watched
//! `${HOME}/.grimoire/hooks/bin/grim-hook` expand on Claude). So this module
//! treats all four arguments as **lookup keys**, and the one that is a path
//! is checked for the single property that makes it safe to read rather than
//! being trusted because grim usually wrote it.
//!
//! ## The three rules, and why each refusal exits 0
//!
//! 1. **`--table` must be absolute** (B1). A relative value would resolve
//!    against the process CWD, which for a client-spawned `grim hook run`
//!    *is the workspace* — so a repo could ship its own table. Refused,
//!    nothing read.
//! 2. **`--event` must name a canonical event** grim understands. The value
//!    arrives in the *client's* own spelling; on all three v1 clients that is
//!    the canonical PascalCase name, so the mapping is a lookup over
//!    [`CanonicalEvent::ALL`] and an unrecognized value simply matches
//!    nothing.
//! 3. **`--root` is an opaque lookup key, and nothing else** (B3). Never a
//!    path, never a trust input, and — this is C-007's amendment, not an
//!    oversight — **never validated against `$PWD`**: checking the root
//!    against the invoking workspace is the scope resolution C-007 forbids.
//!    An unknown token matches no root, which is the same outcome as an
//!    absent one. The token's unguessability, not a check here, is what
//!    denies the forged-registration case.
//!
//! **Every refusal is exit 0 with one log line, never an error.** This is not
//! politeness. On Copilot's `preToolUse` *any* non-zero exit denies the
//! user's tool call, and on Claude `exit 2` **is** the deny code — so
//! returning an error for a malformed argv would let anyone who can write a
//! registration deny every tool call in the session. Invariant **I3**: grim
//! fails in the direction that does not block the user.
//!
//! Note the asymmetry with clap: a *missing* required flag is a usage error
//! (exit 64) raised during parse, before this module exists. That is correct
//! and harmless — the launcher's own `case` collapses every non-verdict exit
//! code to 0 before the client sees it (`hook_launcher.rs`), so the only
//! reader of a 64 here is a human who typed the command.

use std::path::Path;

use crate::oci::hook::CanonicalEvent;

use super::run::RunArgs;

/// A validated dispatch target: exactly the one absolute path and the three
/// lookup keys the runtime is allowed to hold.
///
/// Borrowed from the parsed args rather than owned, so there is no second
/// copy of an untrusted value to keep in step — and so this type cannot be
/// constructed out of thin air by a later caller with a `String` in hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunTarget<'a> {
    /// The dispatch table, already known absolute (rule 1).
    pub table: &'a Path,
    /// The invoking client's grim name — a **lookup key** into
    /// [`RESPONSE_PROJECTION`](crate::oci::hook::RESPONSE_PROJECTION), never
    /// a trust input.
    pub client: &'a str,
    /// The firing event, mapped from the client's spelling (rule 2).
    pub event: CanonicalEvent,
    /// The client's own spelling of that event, kept because the projector must
    /// echo the **firing** event's native name and the canonical name is not
    /// always it. Carried rather than re-derived: it is an argv value, so
    /// re-deriving it would mean a second read of untrusted input.
    pub native_event: &'a str,
    /// The opaque root token (rule 3) — compared to stored keys by value and
    /// used for nothing else.
    pub root: &'a str,
}

/// Why an invocation resolves to "there is nothing to dispatch".
///
/// Not an error type, deliberately: every variant is a **refusal that exits
/// 0**, and modelling them as errors is how one of them would eventually
/// acquire a non-zero exit code and start denying tool calls. Each variant
/// carries the one log line the runtime emits.
///
/// Closed internal enum: the binary is the only consumer, so matches stay
/// total — no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgvRefusal {
    /// `--table` is not absolute, so reading it would resolve against the
    /// process CWD — the workspace, for a client-spawned run (B1).
    TableNotAbsolute,
    /// `--event` names no canonical event this grim understands. Includes the
    /// version-skew case: a launcher written by a *newer* grim naming an event
    /// this binary lacks degrades to no-match, never to an error.
    UnknownEvent,
    /// `--root` is empty. An empty token cannot match a stored key, so this
    /// is already a no-match; it is named separately because "the launcher
    /// passed no root" and "the root is unknown" are different bugs and a
    /// single log line for both would hide the first.
    EmptyRoot,
    /// `--client` is empty. Same reasoning: it cannot match a projection row,
    /// and conflating it with an unknown client hides a malformed launcher.
    EmptyClient,
}

impl ArgvRefusal {
    /// The reason phrase, library style (lowercase, no trailing punctuation).
    ///
    /// Reaches a `tracing` line and nothing else — no exit code, no report,
    /// no error document. Human-facing text carries no compatibility promise
    /// (`docs/src/stability.md` § Unstable).
    pub fn reason(self) -> &'static str {
        match self {
            Self::TableNotAbsolute => {
                "the --table path is not absolute, so it would resolve against the current \
                 directory; nothing was read and no hook ran"
            }
            Self::UnknownEvent => "the --event value names no lifecycle event this grim understands",
            Self::EmptyRoot => "the --root token is empty, so it matches no armed root",
            Self::EmptyClient => "the --client value is empty, so it matches no known client",
        }
    }
}

impl std::fmt::Display for ArgvRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

/// Validate one `grim hook run` invocation.
///
/// The complete argv contract, in the order the checks run: `--client`
/// non-empty → `--root` non-empty → `--event` recognized → `--table`
/// absolute. The path check is **last** on purpose: it is the only one that
/// says anything about the filesystem, and the cheaper key checks reject a
/// malformed launcher before the log line mentions a path at all.
///
/// # Errors
///
/// An [`ArgvRefusal`], which the caller turns into one log line and exit 0.
/// It is never propagated as a `crate::error::Error` and never classified —
/// see the module doc on why a non-zero exit here would deny tool calls.
pub fn validate(args: &RunArgs) -> Result<RunTarget<'_>, ArgvRefusal> {
    if args.client.is_empty() {
        return Err(ArgvRefusal::EmptyClient);
    }
    if args.root.is_empty() {
        return Err(ArgvRefusal::EmptyRoot);
    }
    let event = canonical_event(&args.event).ok_or(ArgvRefusal::UnknownEvent)?;
    // B1. `Path::is_absolute` is a lexical test with no filesystem access and
    // no canonicalization, which is exactly what is wanted: the question is
    // whether the value depends on the CWD, not whether the file exists or
    // where a symlink leads. Containment of what the table *names* is the
    // reader's job (`hook_dispatch::read_table` re-checks every row).
    if !args.table.is_absolute() {
        return Err(ArgvRefusal::TableNotAbsolute);
    }
    Ok(RunTarget {
        table: &args.table,
        client: &args.client,
        event,
        native_event: &args.event,
        root: &args.root,
    })
}

/// The canonical event a client's own event spelling names, if any.
///
/// A lookup over [`CanonicalEvent::ALL`], not a `match` on strings: the
/// canonical names are that enum's, and a second spelling of them here would
/// be a table to keep in step. All three v1 clients register the canonical
/// PascalCase name (`Vendor::hook_event_name`), and Copilot **must** get
/// PascalCase or its matchers never fire (WP-B requirement 1) — so the
/// identity mapping is the whole mapping today, and a client whose native
/// spelling differs needs its own arm in the *vendor*, not a dialect table
/// here.
///
/// A native-only moment (codex's `PermissionRequest`) deliberately resolves
/// to `None`: it is not a canonical event, the dispatch table is keyed on
/// one, and substituting the nearest canonical moment would run a guardrail
/// at the wrong time.
pub fn canonical_event(spelling: &str) -> Option<CanonicalEvent> {
    CanonicalEvent::ALL.into_iter().find(|e| e.as_str() == spelling)
}
