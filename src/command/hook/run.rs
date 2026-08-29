// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim hook run` — the dispatcher runtime.
//!
//! Invoked by the generated launcher, once per armed `(client, event)` per
//! tool call. Its whole contract fits in three sentences:
//!
//! 1. **It cannot fail.** [`run`] returns
//!    [`ExitCode`](crate::cli::exit_code::ExitCode), not `Result`. Every
//!    internal problem — a malformed argv, an unreadable table, an unknown
//!    root, a version-skewed schema — degrades to *exit 0 with one log line*
//!    (decision G, invariant **I3**). Some clients fail **closed** on a
//!    non-zero hook exit, so an error here would deny the user's own tool
//!    call; on Claude `exit 2` *is* the deny code.
//! 2. **The dispatch table is its only input** (C-007), located by the
//!    absolute `--table` path baked into the launcher at install time. It
//!    resolves no scope, reads no configuration, and never asks the
//!    environment where the data root is — see the module doc of
//!    [`super`] and the source-level test that pins it.
//! 3. **It hashes nothing** (C-009). Integrity is pinned at resolution;
//!    `DispatchEntry::resolved_digest` is provenance carried into the audit
//!    record, never a gate. The owner's decision A3 deleted the exec-time
//!    re-check, and re-adding one here would reintroduce a hash on the hot
//!    path of every tool call to defend against **N2**, an explicit non-goal.
//!
//! ## There is exactly one table reader, and it is not here
//!
//! W2's whole reader contract — size cap, parse, `schema` first, per-row
//! `MATCHER_MAX_BYTES` and absolute-`payload_dir` re-checks, never an `Err`,
//! never a panic — lives in
//! [`read_table`](crate::install::hook_dispatch::read_table). This module
//! calls it and does not deserialize the file itself: two readers of one
//! format drift, and the drift direction is *"the runtime honours a row the
//! writer would not have written"* — the C-021 lesson applied to the table
//! instead of the projection.
//!
//! ## The order of operations, and why it is this order
//!
//! [`run`] is a guard path followed by [`dispatch`], and every step before the
//! spawn exists to make the step after it cheaper or safer:
//!
//! 1. **argv validation** ([`argv::validate`]) — the only untrusted input grim
//!    holds, checked before anything touches the filesystem.
//! 2. **the table read** ([`read_table`]) — one reader, W2's whole contract,
//!    every failure an empty table plus one log line.
//! 3. **root then `(client, event)` row selection** — a linear scan over a
//!    handful of keys, and the client dimension is part of the key because a row
//!    exists per `(hook, client)` (see [`client_admits`]).
//! 4. **the payload read**, capped, and deliberately *after* row selection is
//!    known to be non-empty: nothing armed means nothing to hand a hook, and the
//!    no-match path is the one every tool call on an armed client pays.
//! 5. **[`dispatch`]** — grim's own matcher, the C-002 envelope, Decision O's
//!    tier pipeline, the audit record, the projection, exit 0.
//!
//! Nothing here returns an error, and nothing panics. Every refusal — a
//! malformed argv, an unreadable table, an unknown root, an over-cap payload, a
//! payload that is not a JSON object, an unprojectable response — is one log
//! line and [`ExitCode::Success`].

use std::path::{Path, PathBuf};

use clap::Args;
use globset::GlobBuilder;
use tokio::io::AsyncReadExt as _;

use crate::cli::exit_code::ExitCode;
use crate::hook::audit::AuditLog;
use crate::install::hook_dispatch::{DispatchEntry, DispatchRoot, DispatchTable, read_table};

use super::argv;
use super::envelope;
use super::pipeline::{self, Invocation};
use super::projector;

/// The audit trail's file name, **beside the dispatch table**.
///
/// The name is declared here because the runtime is its first writer; if the
/// install-side record path (WP-I/WP-J2) needs it too, it should move beside
/// [`AuditLog`](crate::hook::audit::AuditLog) rather than be spelled twice.
pub const AUDIT_FILE: &str = "hook_audit.jsonl";

/// Maximum bytes grim reads from the invoking client's stdin.
///
/// A `PostToolUse` payload can legitimately embed a whole file, so the ceiling
/// is generous — but it is a ceiling: the payload is untrusted input on the hot
/// path of every tool call, and an unbounded `read_to_end` is CWE-400. Over the
/// cap is one warning and exit 0, with nothing spawned, like every other refusal
/// here.
const MAX_PAYLOAD_BYTES: u64 = 8 * 1024 * 1024;

/// `grim hook run` arguments — **the whole of the runtime's untrusted input**
/// (B3).
///
/// One absolute path and three lookup keys, and nothing else will ever be
/// added without re-reading C-007: each new argument is a second runtime
/// input, and the property "the table is the sole input" is only checkable by
/// reading this declaration.
///
/// All four are required. A missing one is a clap usage error (exit 64)
/// raised during parse; the launcher's own `case` collapses every non-verdict
/// exit code to 0 before the client sees it, so the only reader of that 64 is
/// a human who typed the command.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// The invoking client's grim name (`claude`, `codex`, `copilot`).
    ///
    /// A lookup key into the response-projection table. Never a trust input:
    /// naming a client grants nothing that client does not already have.
    #[arg(long)]
    pub client: String,

    /// The firing event in the client's own spelling (`PreToolUse`, …).
    ///
    /// Mapped to a canonical event by lookup; an unrecognized value — including
    /// one a *newer* grim's launcher wrote — matches nothing and exits 0.
    #[arg(long)]
    pub event: String,

    /// Absolute path of the dispatch table, baked in at install time.
    ///
    /// **Must be absolute.** A relative value would resolve against the
    /// process CWD, which for a client-spawned run is the workspace — so a
    /// hostile repository could ship its own table (B1). A non-absolute value
    /// is refused with one log line and exit 0, never an error.
    #[arg(long)]
    pub table: PathBuf,

    /// The opaque per-install root token.
    ///
    /// A lookup key only: never a path, never a trust input, and never
    /// validated against the current directory — that comparison is the scope
    /// resolution C-007 forbids. An unknown token matches no root and exits 0,
    /// which is the forged-registration case (B3).
    #[arg(long)]
    pub root: String,
}

/// Dispatch every hook armed for this `(root, event)`.
///
/// Never returns an error and never panics. The exit code is
/// [`ExitCode::Success`] on every path grim controls; a deliberate verdict
/// reaches the client as a JSON document on stdout, never as an exit code
/// (`VERDICT_EXIT_CODES` in `hook_launcher.rs` is empty for all three v1
/// clients for exactly this reason).
pub async fn run(args: &RunArgs) -> ExitCode {
    let target = match argv::validate(args) {
        Ok(target) => target,
        Err(refusal) => {
            // One line, at `warn`: a refusal here means a launcher — grim's or
            // someone else's — passed something grim will not act on, and that
            // is worth seeing. It is not an error and must never become one.
            tracing::warn!("grim hook run: {refusal}");
            return ExitCode::Success;
        }
    };

    // W2: the single reader collapses every unreadable shape to the empty
    // table plus a reason. An empty table is also how "the feature is off"
    // reaches the runtime (decision N), so absence is logged at `debug` while
    // a table that exists but could not be trusted is logged at `warn`.
    let (table, degrade) = read_table(target.table);
    if let Some(reason) = degrade {
        match reason {
            crate::install::hook_dispatch::DispatchDegrade::Absent => {
                tracing::debug!("no dispatch table at {}; nothing is armed", target.table.display());
            }
            other => tracing::warn!(
                "dispatch table at {} was not usable ({other:?}); no hook ran",
                target.table.display()
            ),
        }
    }

    let Some(root) = root_entry(&table, target.root) else {
        // The forged-registration case (B3), and also the ordinary case of a
        // stale registration outliving its root. Indistinguishable on purpose:
        // an unknown token is a no-match, exactly like an absent one.
        tracing::debug!("no hooks armed for the requested root; nothing ran");
        return ExitCode::Success;
    };

    // The selection key is `(root token, client, event)` — all three, because a
    // row exists per `(hook, client)`. Selecting on the event alone would run a
    // hook armed for two clients twice per tool call, and would run one grim
    // `Declined` for the invoking client at all.
    let armed: Vec<&DispatchEntry> = root
        .hooks
        .iter()
        .filter(|hook| hook.event == target.event && client_admits(target.client, hook))
        .collect();
    if armed.is_empty() {
        tracing::debug!("no hooks armed at {} for the requested root; nothing ran", target.event);
        return ExitCode::Success;
    }

    let Some(raw) = read_payload().await else {
        // The cap or an unreadable stdin. Nothing spawned, exit 0 — the payload
        // is the hook's whole input, and a hook cannot judge what grim could not
        // read.
        return ExitCode::Success;
    };
    dispatch(&target, &root.root, &armed, &raw).await
}

/// Read the invoking client's payload from stdin, capped.
///
/// `None` is "there is no payload grim can work with", which degrades exactly
/// like every other refusal: one line, nothing spawned, exit 0.
async fn read_payload() -> Option<Vec<u8>> {
    let mut raw = Vec::new();
    // `take(cap + 1)` so an over-cap payload is *detectable* — reading exactly
    // `cap` bytes cannot tell a payload that fits from one that was truncated.
    if let Err(e) = tokio::io::stdin()
        .take(MAX_PAYLOAD_BYTES + 1)
        .read_to_end(&mut raw)
        .await
    {
        tracing::warn!("the client's hook payload could not be read ({e}); no hook ran");
        return None;
    }
    if raw.len() as u64 > MAX_PAYLOAD_BYTES {
        tracing::warn!("the client's hook payload is larger than {MAX_PAYLOAD_BYTES} bytes; no hook ran");
        return None;
    }
    Some(raw)
}

/// The armed set for `token`, by value comparison over the stored keys.
///
/// A linear scan rather than a `BTreeMap` lookup, and that is forced rather
/// than sloppy: [`RootToken`](crate::install::hook_dispatch::RootToken)
/// deliberately has no `Deserialize` and no production constructor from a
/// `&str` — it used to have a transparent one, which made it exactly as
/// forgeable as the absolute path it replaced. So the runtime, which holds
/// the token only as an argv string, compares against
/// [`RootToken::as_str`](crate::install::hook_dispatch::RootToken::as_str)
/// instead of minting one to look up. The table holds one entry per armed
/// root on one machine, so the scan is over a handful of keys.
fn root_entry<'a>(table: &'a DispatchTable, token: &str) -> Option<&'a DispatchRoot> {
    table
        .roots
        .iter()
        .find(|(stored, _)| stored.as_str() == token)
        .map(|(_, root)| root)
}

/// The audit trail for a run located by `table` — **the table's sibling**.
///
/// One derivation from `--table`, not two. The ADR put the trail at
/// `<data root>/state/hook_audit.jsonl` and the stub implemented that; it was
/// settled the other way (plan finding F-2) because climbing two levels from
/// `$GRIM_HOME/hooks/dispatch.json` reconstructs exactly the `$GRIM_HOME`
/// authority `--table` was chosen to withhold — from one argv value the runtime
/// could then derive the launcher, the payload trees, the root-key file and the
/// content store, and C-007's "the table is the sole runtime input" would stop
/// being checkable by reading the argv. The other candidate, a baked
/// `--audit '<abs>'` element, would move a registration string WP-I pins byte
/// for byte.
///
/// Two properties come free from the move: `ensure_hooks_dir` already guarantees
/// the hooks directory is `0o700` where `$GRIM_HOME/state/` guarantees nothing,
/// and the **reader** (`grim status`) runs install-side where it does hold
/// `$GRIM_HOME`, so it computes the location the way `dispatch_path` does —
/// writer without home authority, reader with it.
///
/// `None` only when `table` has no parent at all. Argv is untrusted, so this is
/// an `Option` rather than an `expect`: it must not panic its way out of a
/// command whose only permitted exit code is 0.
fn audit_trail_path(table: &Path) -> Option<PathBuf> {
    Some(table.parent()?.join(AUDIT_FILE))
}

/// Run the matched set and project one response onto `client`'s shape.
///
/// The Implement phase's whole job, in the order Decision O fixes:
/// evaluate grim's own matcher against the tool name; build the C-002
/// envelope; run every `mutator` serially in declaration order, threading
/// each output into the next; submit the one final input to **every**
/// `gatekeeper`; aggregate with `deny` absorbing and `ask` outranking
/// `allow`; project the canonical response through
/// [`super::projector`]; write the audit record; exit 0.
///
/// Every path here returns [`ExitCode::Success`], including the ones that refuse
/// to do anything: a verdict travels as a JSON document on stdout, and grim's
/// process-level codes must never share that channel (I3, decision G).
async fn dispatch(target: &argv::RunTarget<'_>, scope: &str, armed: &[&DispatchEntry], raw: &[u8]) -> ExitCode {
    let Some(payload) = envelope::read_client_payload(raw) else {
        tracing::warn!("the client's hook payload is not a JSON object; no hook ran");
        return ExitCode::Success;
    };
    let Some(trail) = audit_trail_path(target.table) else {
        // Unreachable for a validated absolute `--table`. Refusing rather than
        // running unlogged keeps C-012's invariant true even here.
        tracing::warn!("the audit trail has no location beside the dispatch table; no hook ran");
        return ExitCode::Success;
    };

    let audit = AuditLog::at(trail);
    let correlation_id = correlation_id();
    let invocation = Invocation {
        client: target.client,
        scope,
        event: target.event,
        native_event: target.native_event,
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
        // grim reads are named so the diagnosis is one log line long.
        tracing::warn!(
            "no tool could be read from the client's payload (grim reads `tool_name` / \
             `tool_input`), so every matcher at {} declined; if this client spells them \
             differently, its matchers can never fire",
            target.event
        );
    }
    let mut matched: Vec<&DispatchEntry> = Vec::with_capacity(armed.len());
    let mut declined: Vec<&DispatchEntry> = Vec::new();
    for entry in armed {
        if matches_tool(entry.matcher.as_deref(), tool) {
            matched.push(entry);
        } else {
            tracing::debug!("{}/{}: its matcher did not match {tool:?}", entry.artifact, entry.id);
            declined.push(entry);
        }
    }
    // **One write for every decline of this invocation** (F-2). The set is
    // complete once the loop is: a decline is decided by grim's own matcher and
    // nothing after this point can add one, so collecting first costs nothing and
    // saves an open, a `create_dir_all` and a rotation `statx` per armed hook —
    // +14.1 ms *each* on a 9P `$GRIM_HOME`. Recorded before the early return
    // below, because a run where every hook declined is exactly the run those
    // records answer for.
    pipeline::record_no_matches(&invocation, &declined).await;
    if matched.is_empty() {
        tracing::debug!(
            "no armed hook matched {tool:?} at {}; nothing was spawned",
            target.event
        );
        return ExitCode::Success;
    }

    let plan = pipeline::order(&matched);
    let response = pipeline::compose(&plan, &invocation, raw).await;

    // Exit 0 with **empty stdout** is the fail-safe shape on all three v1
    // clients, so a no-opinion answer says nothing at all rather than emitting a
    // document whose only member is the event echo.
    if response == crate::command::hook::pipeline::CanonicalResponse::no_opinion() {
        tracing::debug!("no hook expressed an opinion; nothing was written to stdout");
        return ExitCode::Success;
    }
    match projector::project(target.client, target.event, target.native_event, &response) {
        Ok(document) => println!("{document}"),
        // Even a refusal here exits 0: a client that fails closed must not be
        // denied a tool call because grim could not express an answer.
        Err(e) => tracing::warn!(
            "the hook response could not be projected onto {} ({e}); no verdict",
            target.client
        ),
    }
    ExitCode::Success
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

/// Whether `client` may run `entry` at all.
///
/// **A string equality, and that is the whole of it** — because the row *names*
/// its client. The stub could not implement this: `DispatchEntry` had no
/// `client` field, so the runtime's only way to answer was to re-derive the
/// per-client decline from `Vendor::hook_tier_support` and the matcher
/// translation, which is a second spelling of a render-time decision and exactly
/// the drift C-021 exists to prevent. WP-J2 added the field as **required**
/// instead, so the answer is now a lookup rather than a re-derivation.
///
/// What it prevents, and what it does **not**:
///
/// - a hook armed for two clients is **two rows**, so selecting on the event
///   alone would spawn the payload twice for one tool call. This is the failure
///   the check exists for.
/// - it does **not** carry a decline. An earlier revision of this doc claimed
///   that without the client check "the declining client would execute code the
///   user was told was not armed there" — which was false, and the wave-7 audit
///   filed it as P-1: the row named the *arming* client, so the declining
///   client's own rows were admitted, and the check filtered only *other*
///   clients' rows. A `HookDecline` is now represented by the row's **absence**
///   (`hook_registrar::register_desired` filters the union by the same verdict
///   that writes the registration), so the decline is enforced by there being
///   nothing to select — never by this predicate.
///
/// Stated plainly, because I5 forbids describing evidence as prevention: a table
/// written by a grim that predates that fix still holds its declined rows, and
/// this predicate does nothing about them — the row names the client that armed
/// it, which is the client that declined it, so the equality holds. Such a table
/// is corrected by the next `grim install`, which replaces the root's `hooks`
/// vector wholesale.
///
/// A client name the runtime does not recognize simply matches no row, which is
/// the fail-safe direction and the reason `DispatchEntry::client` is a `String`
/// rather than a `ClientTarget`.
fn client_admits(client: &str, entry: &DispatchEntry) -> bool {
    entry.client == client
}

/// Whether grim's own matcher dialect admits `tool`.
///
/// Grim owns matching, not the vendor (C-006): the registered vendor matcher
/// is the coarse filter that gets grim invoked at all, and this is the
/// authoritative pass. `None` matches every tool. The dialect is an exact
/// name, an `A|B` alternation of those, or a glob — **never a regex**, which is
/// both a latency decision on the hot path and the reason `MATCHER_ALLOWED` is
/// an allowlist.
///
/// **Alternation is split here because the vendor field carries it verbatim.**
/// `A|B` is one of C-025's three losslessly translatable forms, so
/// `classify_matcher` returns `ExactOrAlternation` and the client's own matcher
/// fires on either name. Comparing the whole `"Bash|Read"` string for equality
/// would therefore arm the hook on every client and fire it on nothing — armed
/// everywhere, matching nowhere, which is the worst of both directions because
/// `grim status` and `grim hook list` both report it armed.
/// [`matcher_may_select_shell_command_tool`](crate::install::vendor::matcher_may_select_shell_command_tool)
/// already splits the same way for Decision K, so this is the dialect agreeing
/// with itself rather than a new rule.
fn matches_tool(matcher: Option<&str>, tool: Option<&str>) -> bool {
    let Some(matcher) = matcher else {
        return true;
    };
    let Some(tool) = tool else {
        // A payload naming no tool gives grim's matcher nothing to match, and a
        // matcher that matches nothing is the safe direction: it withholds the
        // hook rather than firing it on an unknown tool.
        return false;
    };
    // A matcher with no `|` yields exactly one alternative, so the common case
    // pays one `split` and is otherwise unchanged.
    matcher
        .split('|')
        .any(|alternative| matches_one_alternative(alternative, tool))
}

/// Whether one alternative of [`matches_tool`]'s dialect admits `tool`.
///
/// An alternative with no `*` and no `?` is compared as an **exact name**, which
/// is both the common case and the reason `Ba.h` cannot match `Bash`: there is no
/// glob engine involved at all there, so a regex metacharacter is just a
/// character. One that *does* wildcard is compiled with the settings named below
/// — deliberately **not** the ones `registry_filter.rs` pins for browse filters,
/// because a tool name is one segment with no path semantics.
///
/// An **empty** alternative (`A|`, `|B`, `A||B`) matches nothing: it takes the
/// exact-name path and no tool is named `""`. That is the opposite of what the
/// vendor dialect does with one — an empty regex alternative matches *everything*
/// on claude and codex — and the divergence is deliberate. `is_exact_tool_name`
/// already refuses an empty alternative at `grim build`, so such a matcher never
/// reaches a dispatch row; if one ever did, grim withholding the hook is the safe
/// direction and silently widening it to every tool is not.
fn matches_one_alternative(matcher: &str, tool: &str) -> bool {
    if !matcher.contains(['*', '?']) {
        return matcher == tool;
    }
    match GlobBuilder::new(matcher)
        // `*` may cross any character: a tool name is a single token, and the
        // path semantics `literal_separator` protects do not apply to one.
        .literal_separator(false)
        // Uniform across platforms, and `MATCHER_ALLOWED` admits no backslash
        // anyway — so a backslash is a literal character everywhere rather than
        // an escape on unix and a literal on Windows.
        .backslash_escape(false)
        .case_insensitive(false)
        .build()
    {
        Ok(glob) => glob.compile_matcher().is_match(tool),
        Err(e) => {
            // A matcher grim cannot compile matches nothing, loudly: firing it on
            // every tool would widen a guardrail on a parse failure.
            tracing::warn!("the matcher `{matcher}` is not a valid glob ({e}); it matched nothing");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                matches_tool(*matcher, *tool),
                *expected,
                "matcher {matcher:?} against tool {tool:?}: {why}"
            );
        }
    }

    /// An unknown root token is indistinguishable from an absent one — the
    /// forged-registration case (B3), and also an ordinary stale registration.
    ///
    /// Already implemented at stub phase; this pins it so a later "optimization"
    /// that re-adds a `&str` → `RootToken` constructor for an `O(log n)` lookup
    /// has to fail a test rather than merely change a comment.
    #[test]
    fn an_unknown_root_token_selects_nothing_b3() {
        let table = DispatchTable::empty();
        assert!(root_entry(&table, "0000000000000000000000000000ffff").is_none());
        assert!(root_entry(&table, "global").is_none(), "`global` is not a token");
        assert!(root_entry(&table, "").is_none());
    }

    /// **The audit trail is the dispatch table's SIBLING**, inside the same
    /// `0o700` hooks directory — one derivation from `--table`, not two.
    ///
    /// Settled after the stub phase (which implemented the ADR's
    /// `<data root>/state/hook_audit.jsonl` and recorded the gap as F-2). The
    /// two-level climb to the data root reconstructs exactly the `$GRIM_HOME`
    /// authority `--table` was chosen to withhold — the runtime could then
    /// derive the launcher, the payload trees, the root-key file and the content
    /// store from one argv value, and C-007's "the table is the sole runtime
    /// input" would stop being checkable by reading the argv. The other
    /// candidate, a baked `--audit '<abs>'` element, would move a registration
    /// string WP-I pins byte for byte.
    ///
    /// So this test **fails against the stub on purpose**: `audit_trail_path`
    /// still climbs two levels, and that is now a defect rather than a
    /// placeholder.
    #[test]
    fn the_audit_trail_is_the_dispatch_tables_sibling() {
        assert_eq!(
            audit_trail_path(Path::new("/home/u/.grimoire/hooks/dispatch.json")),
            Some(PathBuf::from("/home/u/.grimoire/hooks/hook_audit.jsonl")),
            "the trail lives beside the table in the same 0o700 directory; climbing to the data \
             root re-acquires the authority `--table` exists to withhold"
        );
        // Argv is untrusted, so a legal-but-parentless absolute path must not
        // panic its way out of a command that cannot fail. One derivation means
        // one `parent()`, so `/dispatch.json` now HAS an answer where the
        // two-level climb had none — the root directory.
        assert_eq!(
            audit_trail_path(Path::new("/dispatch.json")),
            Some(PathBuf::from("/hook_audit.jsonl"))
        );
    }
}
