// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Constants and charset of the `hook.toml` format, and the payload and handler
//! shapes an entry carries.

use serde::{Deserialize, Serialize};

/// The `schema` value this grim writes and understands. Bumping it is a wire
/// change; an unknown value must produce the explanatory error S-014 requires,
/// never a bare TOML parse failure.
pub const HOOK_SCHEMA_VERSION: u32 = 1;

/// Characters a `matcher` may contain (C-018).
///
/// An **allowlist**, deliberately not a denylist of quotes and control
/// characters: a denylist still admits bidi and homoglyph characters that let a
/// matcher spoof what an approval prompt or a vendor's own trust TUI displays,
/// and admits `$`/backtick forms that become a latency bomb on the hot path.
pub const MATCHER_ALLOWED: &str = "A-Za-z0-9_*?./-|";

/// Maximum `matcher` length in bytes (C-018). Longer fails `grim build`
/// (exit 65).
pub const MATCHER_MAX_BYTES: usize = 256;

/// The per-handler timeout an entry that declares none gets, in seconds.
///
/// Declared here rather than in the runtime that enforces it: it is a property
/// of the **format** (`hook.toml`'s `timeout` field documents "default 30"), and
/// a number the manifest documents in prose while the enforcer holds the only
/// literal is a number that drifts. **Grim** enforces it on every client, so a
/// vendor's own timeout is a backstop and never the contract.
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// The smallest `timeout` an entry may declare, in seconds. Below it the
/// dispatcher's budget (the shortest timeout, less a margin) leaves no time to
/// start any hook on that tool call.
pub const MIN_TIMEOUT_SECS: u64 = 1;

/// The largest `timeout` an entry may declare, in seconds. Far beyond any
/// client's own limit, and small enough that the dispatcher's deadline
/// arithmetic cannot overflow.
pub const MAX_TIMEOUT_SECS: u64 = 3600;

/// Whether one character is admissible in a `matcher` (C-018).
///
/// **This, not [`MATCHER_ALLOWED`], is the membership test.**
/// `MATCHER_ALLOWED` is a range *spelling* for the diagnostic, so the obvious
/// `MATCHER_ALLOWED.contains(c)` is wrong in both directions — it rejects `'B'`
/// and accepts `'-'` only incidentally. Keeping the set here and the notation
/// there means the error message stays readable without becoming the
/// implementation.
/// `|` is admitted because C-025's translation table lists `A|B` alternation
/// as one of exactly three forms that translate **losslessly** to all three v1
/// clients, and WP-B verified it fires on each. Without it the form would be
/// unauthorable at `grim build` while the vendor seam accepted it — the two
/// contracts have to agree, and this is the additive direction. It never
/// reaches a shell: C-018b routes every publisher-controlled value through
/// argv or a single-quoted string, where `|` is inert.
pub const fn matcher_char_allowed(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '*' | '?' | '.' | '/' | '-' | '|')
}

/// How the canonical envelope reaches the payload.
///
/// `stdin` is the default and the only transport that avoids the always-on
/// metadata vectors (`/proc/<pid>/cmdline`, `/proc/<pid>/environ`, crash dumps,
/// CI logs). `file` is an explicit opt-in for payloads that would overflow
/// `ARG_MAX`, never a default.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookPayloadMode {
    /// One JSON object on the payload's stdin (default).
    #[default]
    Stdin,
    /// The envelope is written to a file whose path is exported as
    /// `GRIM_HOOK_PAYLOAD`.
    File,
}

/// How a `[[hooks]]` entry names the program to run.
///
/// `argv` is the documented preferred form (no shell, no quoting); `command` is
/// a single string handed to the platform shell and is documented as the lesser
/// form. Serialized flattened into the entry table, so the variant name *is*
/// the authored key: `argv = ["sh", "guard.sh"]` or `command = "guard.sh"`.
///
/// **"Exactly one of `argv`/`command`" is a validation rule, NOT a type
/// invariant — do not document it as one.** Under `#[serde(flatten)]` the
/// externally-tagged enum is resolved by `FlatMapDeserializer`, which takes the
/// **first key matching a variant name in declaration order**, not the authored
/// order. An entry supplying *both* keys therefore parses cleanly: `argv` wins,
/// and the surplus `command` is swept into [`HookEntry::vendor`], the catch-all
/// flatten that follows it. Nothing errors, and a human reading that
/// `hook.toml` bottom-up would believe the *last* handler runs while grim runs
/// `argv` (attacker T1: a published manifest carrying a shadow handler).
/// [`HookManifest::validate`] closes it explicitly —
/// [`HookError::AmbiguousHandler`] — by inspecting `vendor` for `argv` /
/// `command`, which is exactly where the deserializer put them.
///
/// The *neither* case fails earlier, inside serde, with the internal message
/// `no variant of enum HookHandler found in flattened data`. That is not an
/// author-facing diagnostic for a published format, so
/// [`HookManifest::from_toml_str`] re-maps it to
/// [`HookError::MissingHandler`] — the same "explanatory error, never a bare
/// TOML parse failure" rule S-014 already imposes on the `schema` field.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookHandler {
    /// Exec-form argument vector — no shell involved.
    Argv(Vec<String>),
    /// A single string handed to the platform shell.
    Command(String),
}

impl HookHandler {
    /// The handler's first token: `argv[0]`, or the first whitespace-separated
    /// word of `command`.
    ///
    /// The input to C-019's build-time rule: a first token that resolves to a
    /// **payload-relative file** is rejected at `grim build` (exit 65), because
    /// a payload fetched through OCI arrives `0o644` and a shell would `execve`
    /// it into `EACCES`. The exec bit is never load-bearing; the message names
    /// the interpreter form instead.
    pub fn first_token(&self) -> Option<&str> {
        match self {
            Self::Argv(argv) => argv.first().map(String::as_str),
            // `split_whitespace` and not `split(' ')`: the shell that would run
            // this string splits on any run of blanks, so `"  sh guard.sh"`
            // executes `sh`. Taking the literal first `' '`-delimited field
            // would yield `""` there and skip C-019's check entirely.
            Self::Command(command) => command.split_whitespace().next(),
        }
    }
}
