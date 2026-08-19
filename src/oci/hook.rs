// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The hook artifact format: `hook.toml`, the canonical event/tier model,
//! and the one per-`(vendor, event)` response-projection table.
//!
//! A `hook` artifact is a **directory** artifact (`is_dir_artifact() ==
//! true`): a `hook.toml` manifest at the root plus the payload files its
//! handlers invoke. Installing it materializes the payload once per scope and
//! registers **one grim-owned dispatcher entry per `(client, event, scope,
//! matcher)`** whose command invokes `grim hook run` through a generated
//! launcher — grim, not the vendor, owns matching, ordering, failure policy
//! and response projection. Design record: `.agents/adr/adr_hooks_support.md`
//! (decisions A–Q, contracts C-001…C-014) and
//! `.agents/plans/plan_hooks_artifact_kind.md` (C-015…C-026).
//!
//! Three properties of this module are load-bearing and easy to erode:
//!
//! - **C-021 — the `(vendor, event)` projection table has exactly one
//!   instance, and it is [`RESPONSE_PROJECTION`] here.** Both the render-time
//!   refusal (the `Vendor` hook seam) and the runtime response projector query
//!   it; a second hand-maintained copy would drift, and the drift direction is
//!   "runtime emits a field render-time forbade", which is the Codex
//!   fail-closed bug the table exists to prevent.
//! - **C-018 / C-018b — publisher-authored text never reaches a shell.** A
//!   `matcher` is charset-allowlisted and length-capped at `grim build`
//!   ([`MATCHER_ALLOWED`], [`MATCHER_MAX_BYTES`]), and reaches the vendor's own
//!   *structured* matcher field; the registered command string is assembled
//!   from grim-owned literals plus the resolved absolute launcher path only.
//! - **Canonical breadth is exactly four events** ([`CanonicalEvent`]).
//!   Anything else is native passthrough for one declared vendor via a
//!   `<vendor>.event` key inside the `[[hooks]]` table — never a fifth
//!   canonical variant, and never a *moment substitution* (relocating a
//!   `PreToolUse` guardrail onto `PostToolUse` runs it after the damage).
//!
//! ```toml
//! schema      = 1
//! name        = "shell-guard"
//! description = "Refuse curl-pipe-to-shell in Bash tool calls"
//!
//! [[hooks]]
//! id      = "deny-curl-pipe-sh"
//! event   = "PreToolUse"
//! tier    = "gatekeeper"
//! matcher = "Bash"
//! argv    = ["sh", "${GRIM_HOOK_DIR}/guard.sh"]   # exactly one of argv | command
//! timeout = 30
//! payload = "stdin"
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::install::client_target::ClientTarget;

/// Manifest file name at the root of a hook artifact.
pub const HOOK_MANIFEST_FILE: &str = "hook.toml";

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

/// The charset a `[[hooks]]` `id` may use — lowercase-friendly ASCII word
/// characters, `-` and `.`, and nothing else (audit finding P-6).
///
/// `id` was uncharted: `validate` checked only uniqueness, and the value reached
/// a filesystem path interpolation, `GRIM_HOOK_NAME`, the envelope's `hook` field,
/// the audit trail and `tracing` lines. The path sink is now hash-derived
/// (`command::hook::pipeline::write_payload_file`), so **this rule is
/// defence in depth rather than the control** — I5's distinction, stated here so
/// no later reader treats it as the thing that stops traversal.
///
/// Deliberately narrower than [`matcher_char_allowed`]: an `id` names one entry
/// within one artifact, so there is no dialect to preserve and no reason to admit
/// `*`, `?`, `/` or `|`. `/` in particular is what made the traversal probe look
/// plausible.
pub const fn hook_id_char_allowed(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')
}

/// Maximum bytes a `[[hooks]]` `id` may carry.
///
/// An `id` is an identifier, not prose: it reaches log lines, the envelope and
/// the audit trail, and a publisher must not be able to write a megabyte into a
/// consumer's terminal through one. Checked **before** the charset, so the value a
/// charset diagnostic quotes is always already bounded — the same ordering
/// [`validate_matcher`] uses and for the same reason.
pub const HOOK_ID_MAX_BYTES: usize = 128;

/// Artifact names the launcher namespace reserves; a hook artifact may not use
/// one, and `grim build` refuses it (exit 65,
/// [`HookError::ReservedArtifactName`]).
///
/// `$GRIM_HOME/hooks/` holds the generated launcher (`bin/grim-hook`), the
/// dispatch table (`dispatch.json`) and the `payload/` tree of per-workspace
/// payload roots, all beside the **global** per-artifact payload trees — so each
/// is a legal artifact name whose global payload would materialize **over** the
/// directory, and uninstalling that artifact would then drag the launcher, the
/// table, or every workspace's payloads into prune's reach. Attacker T1 (a
/// published hook artifact named `bin`); invariant I1.
///
/// `payload` joined the list when project-scope payloads moved under
/// `$GRIM_HOME` (SEC-1 — see
/// [`hook_dispatch::payload_dir`](crate::install::hook_dispatch::payload_dir)).
/// The earlier note here reasoned that reserving two names was "cheaper and more
/// additive than nesting payloads under a `payload/` segment, which would move a
/// shipped layout": the nesting became necessary for a security fix, and the
/// layout was never shipped (hooks are gated off and absent from 0.13.0), so the
/// trade the note weighed no longer applies. Appending a name is additive for
/// the same reason.
///
/// `root-key` joined it in round 2 of review, and how it was missed is the
/// lesson: this is a **literal list that has to track a layout defined
/// elsewhere**, and it silently fell one behind when
/// [`hook_dispatch::ROOT_KEY_FILE`](crate::install::hook_dispatch::ROOT_KEY_FILE)
/// was added. A hook bound as `root-key` materialized a *directory* over the
/// machine's HMAC key on a fresh `$GRIM_HOME`, after which no root token could
/// ever be derived — `read_root_key` hits `EISDIR` on every later run and the
/// mint path is `create_new` on the same path — so **no dispatch table was
/// written at all and every hook on the machine reported `installed` while
/// nothing could fire**. One artifact, every guardrail off, permanently (T1;
/// invariants I3 and I5).
///
/// `dispatch.json.lock` joined it in round 3, and it is the same lesson a second
/// time — the paragraph above claimed a drift test that had never been written,
/// so nothing caught that the list was *already* one behind when that claim was
/// made. The name is the advisory-lock sidecar of the table
/// ([`AdvisoryFileLock::try_acquire`](crate::lock::advisory_lock::AdvisoryFileLock::try_acquire)
/// appends `.lock` to the **full** file name), it parses as a `SkillName`, and a
/// hook bound to it materializes a directory where the sidecar must be opened:
/// `try_acquire` then hits `EISDIR`, `converge_root` returns
/// `DispatchError::Io`, and **no table is written for as long as that artifact is
/// installed** — every hook on the machine stops arming (T1; invariants I3, I5).
/// Milder than `root-key` in two ways that do not change the verdict: a `warn!`
/// fires, and `grim uninstall` recovers it.
///
/// The list cannot be derived here without inverting the module direction
/// (`oci` must not depend on `install`), so the drift is prevented from the other
/// side instead: `hook_dispatch`'s
/// [`every_grim_owned_name_under_hooks_is_a_reserved_binding_name`] provokes
/// install's writes under `hooks/` — mint the root key, generate the launcher,
/// converge a root, observing the namespace both while the dispatch lock is held
/// and after it is released — then requires **every directory entry it finds** to
/// be refused as a binding name. So it fails for the next file **install** writes
/// there, whether or not anyone remembers to tell it about the file.
///
/// **That test exists, and it enumerates the filesystem rather than a list.** Its
/// first version iterated the layout constants by hand, which reproduced this very
/// defect one level down — a new `const` would not have appeared in its loop, so
/// it would have passed while this paragraph promised otherwise. Do not restate
/// this guarantee, or weaken the test back to a literal list, without reading it.
///
/// **Its reach is install's dispatch-side writes, and no wider.** The provocation
/// mints the key, generates the launcher and converges a root, so a file written
/// by the *runtime* path — the audit trail, a payload envelope — is invisible to
/// it; round-3 fix-verify proved that twice (V2, then W1) by renaming a
/// runtime-side writer's file and watching the suite stay green. The guarantee
/// above is therefore scoped to install in its own sentence rather than walked back
/// here — a reader who stops at the mechanism must not come away with a wider
/// promise than the test keeps.
///
/// The runtime-side names are covered instead by
/// `hook_dispatch`'s `every_runtime_written_name_at_the_hooks_root_is_unusable_as_a_binding`,
/// which lists them. **A brand-new runtime writer is what neither test catches** —
/// add a row there when you add one.
///
/// Only that one direction is asserted. The reverse ("nothing reserved that is not
/// grim's own") would have to encode exceptions and re-create the drift. The
/// test's own escape hatch is the safe direction of the same trade: it exempts
/// *unreserved* names, so forgetting an entry makes it fail rather than pass.
/// `hook_audit.jsonl` and the transient `payload_<pid>_<slot>.json` envelopes need
/// no exemption, and the reason is the same for both and is **not** where they
/// live — both sit at the root of `hooks/`, which *is* the binding namespace. It
/// is that an underscore is outside `SkillName`'s grammar, so neither name is
/// representable as a binding. The envelope's separators were hyphens until
/// round-3 fix-verify executed a hook bound to `payload-12345-0.json` and had it
/// accepted.
///
/// [`every_grim_owned_name_under_hooks_is_a_reserved_binding_name`]: crate::install::hook_dispatch
pub const RESERVED_ARTIFACT_NAMES: [&str; 5] = ["bin", "dispatch.json", "dispatch.json.lock", "payload", "root-key"];

/// Whether `name` is reserved as a hook **binding** name — the question
/// [`HookManifest::validate`] structurally cannot ask (audit finding P-2).
///
/// `validate` guards the manifest's own `name`, on the publisher's machine. The
/// payload directory is
/// [`payload_dir`](crate::install::hook_dispatch::payload_dir) over the
/// **binding** name, which is the key the *consumer* writes into their own
/// `grimoire.toml` — or, in the case that matters, the key a **bundle** picks for
/// its member. So the shipped check guarded the wrong string, and a hook bound as
/// `bin` materialized `$GRIM_HOME/hooks/bin/hook.toml`.
///
/// The write collision itself was survivable — convergence regenerates
/// `bin/grim-hook` afterwards inside the same command, so grim's own shim wins.
/// **The reap is the sharp edge**: `grim uninstall --global hook bin` deletes the
/// artifact's recorded output tree and takes the launcher with it, after which
/// every armed hook on the machine, for every client and every workspace,
/// silently stops firing — the registered command's own
/// `[ -f "$L" ] && [ -x "$L" ] || exit 0` guard degrades to exit 0.
///
/// A predicate rather than open `RESERVED_ARTIFACT_NAMES.contains` calls, so every
/// site that asks it — `grim add`'s
/// [`refuse_bad_binding_name`](crate::command::add::refuse_bad_binding_name), the
/// installer's pre-materialization gate, and the arming seam — shares one doc home
/// and cannot drift into asking it about different strings.
pub fn is_reserved_binding_name(name: &str) -> bool {
    RESERVED_ARTIFACT_NAMES.contains(&name)
}

/// Why this binding name must not become a hook payload directory, or `None`.
///
/// **Two questions, one answer, and the order matters.** The reserved-name check
/// below asks whether a *valid* name collides with grim's own files. This asks
/// the prior question — whether the string is a name at all — and it is the one
/// that was missing.
///
/// # The gap this closes
///
/// `is_reserved_binding_name` is exact equality against three literals, and
/// [`payload_dir`](crate::install::hook_dispatch::payload_dir) then joins the
/// same string onto `$GRIM_HOME` with no containment check, with anchor
/// classification running *after* materialization. A `[hooks]` table key never
/// passed through [`SkillName::parse`](crate::skill::SkillName::parse) — that
/// call exists for typed `grim add` names and for bundle members, neither of
/// which a committed `grimoire.toml` reaches.
///
/// So a cloned repository could ship `[hooks] "../../../../victimdir" = …` and
/// **overwrite files outside `$GRIM_HOME`** with published content, or write into
/// `$GRIM_HOME/hooks/bin/` straight past the reserved-name gate. Attacker **T3**
/// escalating with **T1**; invariant **I1** — SEC-1's class reopened through the
/// binding name instead of the install record. Found by the wave-8 review panel
/// with an executed reproduction.
///
/// Equality against three literals was never going to be the control when the
/// string reaches a `Path::join`. `SkillName::parse`'s grammar
/// (`[a-z0-9]+([.-][a-z0-9]+)*`, ≤64 chars) makes every traversal form
/// unrepresentable rather than enumerated — no `/`, no `..`, no leading dot.
///
/// # Why this is not `SkillName` in the type system
///
/// A binding name arrives as a `String` from serde on a `BTreeMap` key, and the
/// config layer deliberately does not reject a whole `grimoire.toml` because one
/// key is malformed — other artifacts in the same file still install (I3). So the
/// refusal is per-artifact and returns a reason to report, not an error to
/// propagate.
pub fn binding_name_refusal(name: &str) -> Option<String> {
    if let Err(reason) = crate::skill::SkillName::parse(name) {
        return Some(format!(
            "'{name}' is not a usable artifact name ({reason}); a hook's binding name becomes a \
             directory under $GRIM_HOME, so it must be a single plain name"
        ));
    }
    if is_reserved_binding_name(name) {
        // The namespace is rendered from the array, never re-typed: the enumeration
        // in this message was itself one of the hand-maintained lists that fell
        // behind the layout (round 3, B2).
        let namespace = RESERVED_ARTIFACT_NAMES.join(",");
        return Some(format!(
            "'{name}' is reserved: grim's own launcher, dispatch table (and its lock), payload \
             root and machine key live at $GRIM_HOME/hooks/{{{namespace}}}, and a hook bound as \
             '{name}' would materialize over one of them; rebind it under another name"
        ));
    }
    None
}

/// Reserved `[[hooks]]` key, parsed as an opaque value in v1.
///
/// Declared here so the reservation is discoverable from the type: v1 stores
/// whatever the author wrote and round-trips it **unparsed**, so a future
/// policy vocabulary can land additively without invalidating artifacts
/// published against v1.
#[expect(
    dead_code,
    reason = "documentation-only by construction: `policy` is parsed by field name into \
              `HookEntry::policy`, so nothing reads the const. Naming the reservation from the type \
              is the point — a reader of this module learns the key is reserved without grepping the \
              parse code. NO REMOVAL TRIGGER: this stays dead until a policy vocabulary lands and \
              something matches on the key"
)]
pub const RESERVED_POLICY_KEY: &str = "policy";

/// The four canonical lifecycle events.
///
/// Canonical breadth is exactly these four (ADR decision D3). The names and
/// spelling are Claude Code's, which the survey established as the de facto
/// standard rather than a neutral invention. Every other vendor moment is
/// reached natively through a `<vendor>.event` override on the entry.
///
/// Closed internal enum: matches stay total, no `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema)]
pub enum CanonicalEvent {
    /// Before a tool call runs — the only event a `mutator` may declare, and
    /// the only moment at which rewriting a tool's input is meaningful.
    PreToolUse,
    /// After a tool call has run.
    PostToolUse,
    /// At session start.
    SessionStart,
    /// When the agent's turn stops.
    Stop,
}

impl CanonicalEvent {
    /// Every canonical event, in firing order.
    pub const ALL: [Self; 4] = [Self::PreToolUse, Self::PostToolUse, Self::SessionStart, Self::Stop];

    /// The canonical (PascalCase) event name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::SessionStart => "SessionStart",
            Self::Stop => "Stop",
        }
    }

    /// Whether **any** v1 vendor can express a blocking verdict at this event.
    ///
    /// A query over [`RESPONSE_PROJECTION`] (C-021), not a second table: true
    /// iff some row for this event has a non-empty
    /// [`ProjectionRow::verdict`]. A `gatekeeper` declared on an event that
    /// admits no verdict anywhere is a manifest error at `grim build`, which is
    /// what makes the per-event tier set well-defined (ADR decision F).
    /// Per-client verdicts stay a per-client `Declined`, the normal path.
    pub fn admits_verdict(self) -> bool {
        RESPONSE_PROJECTION
            .iter()
            .any(|row| row.event == self && !row.verdict.is_empty())
    }

    /// Whether this event admits an input rewrite at all — true only for
    /// [`PreToolUse`](Self::PreToolUse): nothing later has an input left to
    /// rewrite.
    ///
    /// Also a query over [`RESPONSE_PROJECTION`] rather than a `match` on the
    /// variant, so the table stays the single source (C-021). The two
    /// formulations agree today and a test pins that they do: if a row ever
    /// grows a [`ProjectionRow::mutation`] outside `PreToolUse`, the failing
    /// test is the intended outcome — a later event has no input left to
    /// rewrite, so such a row would be a vendor-survey error, not a widening.
    pub fn admits_mutation(self) -> bool {
        RESPONSE_PROJECTION
            .iter()
            .any(|row| row.event == self && row.mutation.is_some())
    }
}

impl std::fmt::Display for CanonicalEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a hook is allowed to do with the moment it fires at.
///
/// The tier is a **capability declaration**, resolved per client: a tier the
/// client cannot honour is `Declined`, never silently degraded into a weaker
/// one — degrading a guardrail into a logger reports a security control as
/// installed when it is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HookTier {
    /// Reads the event; its response cannot change what happens.
    Observer,
    /// May return a verdict that blocks the operation. Valid only on an event
    /// that admits a verdict ([`CanonicalEvent::admits_verdict`]).
    Gatekeeper,
    /// May rewrite the tool input. Valid only on
    /// [`CanonicalEvent::PreToolUse`], and `Declined` per client for tools
    /// whose input is a shell-command string (ADR decision K).
    Mutator,
}

impl HookTier {
    /// Every tier, weakest first.
    pub const ALL: [Self; 3] = [Self::Observer, Self::Gatekeeper, Self::Mutator];

    /// The authored tier string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observer => "observer",
            Self::Gatekeeper => "gatekeeper",
            Self::Mutator => "mutator",
        }
    }

    /// Whether this tier may be declared at `event` **in the manifest**.
    ///
    /// `mutator` only at [`CanonicalEvent::PreToolUse`]; `gatekeeper` only on a
    /// verdict-admitting event; `observer` everywhere. Enforced at `grim build`
    /// (exit 65) — a per-client refusal is a separate, later decision.
    pub fn is_valid_at(self, event: CanonicalEvent) -> bool {
        match self {
            // Reading the event is always possible; a tier that changes
            // nothing cannot be dishonoured by the moment it fires at.
            Self::Observer => true,
            Self::Gatekeeper => event.admits_verdict(),
            Self::Mutator => event.admits_mutation(),
        }
    }
}

impl std::fmt::Display for HookTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How the canonical envelope reaches the payload.
///
/// `stdin` is the default and the only transport that avoids the always-on
/// metadata vectors (`/proc/<pid>/cmdline`, `/proc/<pid>/environ`, crash dumps,
/// CI logs). `file` is an explicit opt-in for payloads that would overflow
/// `ARG_MAX`, never a default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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

/// One `[[hooks]]` entry: a single handler bound to a moment and a tier.
///
/// `[[hooks]]` is an array of tables because a pre/post pair sharing one
/// payload tree is the common case. Unknown top-level keys are **not** denied
/// here (unlike every other grim manifest type): the format reserves
/// `<vendor>.<field>` override tables and the [`RESERVED_POLICY_KEY`], both of
/// which must round-trip through a grim that does not understand them. What
/// closes the hole `deny_unknown_fields` would have closed is validation:
/// every client name in `ClientTarget::ALL` is a reserved key, and using one
/// for anything but a vendor override fails `grim build`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HookEntry {
    /// Stable id, unique within the artifact. Reaches the dispatch table and
    /// the audit trail as `<artifact>/<id>`; never interpolated into a
    /// generated shell string (C-018b).
    pub id: String,
    /// The canonical event. Omitted only when a `<vendor>.event` override
    /// stands alone (a native-only moment on exactly one client).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<CanonicalEvent>,
    /// What the handler is allowed to do.
    pub tier: HookTier,
    /// Grim's own matcher dialect: an exact name or a glob, never a regex.
    /// Charset-allowlisted and length-capped at `grim build` ([`MATCHER_ALLOWED`],
    /// [`MATCHER_MAX_BYTES`]); translated into the target client's own dialect
    /// at registration time, and the `(hook, client)` pair is `Declined` when
    /// the translation would not be lossless (C-025).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    /// The program to run — exactly one of `argv` / `command`.
    #[serde(flatten)]
    pub handler: HookHandler,
    /// Per-handler timeout in seconds ([`DEFAULT_TIMEOUT_SECS`]). **Grim**
    /// enforces it, not the vendor, so the behaviour is identical on every
    /// client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    /// Envelope transport; `stdin` unless the author opts into `file`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<HookPayloadMode>,
    /// The reserved [`RESERVED_POLICY_KEY`], captured **unparsed** so a grim
    /// that predates whatever vocabulary it eventually carries still preserves
    /// it.
    ///
    /// **Round-trip is faithful for the JSON-expressible TOML subset only — do
    /// not read it as byte-for-byte.** The value model is `serde_json::Value`
    /// (forced: [`HookManifest`] must derive `schemars::JsonSchema` and
    /// `toml::Value` does not implement it), so TOML's four natives JSON lacks
    /// are not representable. A datetime is the sharp case: `since = 2026-08-14`
    /// parses to `{"$__toml_private_datetime": "2026-08-14"}` and
    /// **re-serializes as a nested table leaking that private sentinel** — a
    /// structural corruption of a published format, not a lossy value.
    /// Ordinary tables and dotted keys round-trip correctly. `grim build` must
    /// therefore reject a datetime / local-date / local-time under `policy` or
    /// a vendor key rather than admit a value it cannot re-emit; the published
    /// format doc must state the same narrowing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<serde_json::Value>,
    /// Per-vendor override tables (`cursor.event`, `claude.timeout`, …),
    /// captured verbatim. Keys outside `ClientTarget::ALL` fail `grim build`;
    /// there is deliberately no free-form escape hatch, because a typo'd
    /// namespace would silently install a hook with none of its overrides.
    ///
    /// **Never interpolated into a generated command string** (C-018b) — a
    /// vendor override reaches that vendor's own structured field or nothing.
    #[serde(flatten)]
    pub vendor: BTreeMap<String, serde_json::Value>,
}

/// The `hook.toml` document (C-001).
///
/// Authored as TOML in the **TOML 1.0-compatible subset** — unquoted dotted
/// keys and single-line inline tables only. Grim's own parser accepts more
/// (its `toml` is `+spec-1.1.0`), but `hook.toml` is a published format read by
/// third-party tooling whose stock TOML 1.0 parsers hard-reject the 1.1 forms:
/// liberal in what grim accepts, conservative in what it documents and emits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HookManifest {
    /// Manifest + envelope contract version; [`HOOK_SCHEMA_VERSION`] today.
    pub schema: u32,
    /// Artifact name; must equal the directory stem (the agent-kind
    /// precedent).
    pub name: String,
    /// Catalog-facing description (becomes
    /// `org.opencontainers.image.description`).
    pub description: String,
    /// The declared handlers. One approval and one payload tree cover every
    /// entry, so the entries are a set, not independent artifacts.
    #[serde(default)]
    pub hooks: Vec<HookEntry>,
}

impl HookManifest {
    /// Parse a `hook.toml` document.
    ///
    /// # Errors
    ///
    /// [`HookError::Toml`] when the source does not parse or carries an unknown
    /// top-level key; [`HookError::UnsupportedSchema`] when `schema` names a
    /// version this grim does not understand (the explanatory error S-014
    /// requires, rather than a bare parse failure);
    /// [`HookError::MissingHandler`] when an entry names neither `argv` nor
    /// `command` — serde's internal `no variant of enum HookHandler found in
    /// flattened data` is re-mapped here under the same S-014 rule, because a
    /// published format owes its author a real message.
    pub fn from_toml_str(source: &str) -> Result<Self, HookError> {
        // Parsed twice, deliberately. The strict parse is the one that
        // produces the value; this shape-agnostic one exists so two errors can
        // be reported in the author's vocabulary instead of serde's, and
        // neither is derivable from the strict parse's failure — it stops at
        // the first problem and carries no manifest.
        let probe: toml::Value = toml::from_str(source).map_err(|e| HookError::Toml(Box::new(e)))?;
        // S-014: a manifest published against a schema this grim does not know
        // gets the explanatory error, not whichever field the strict parse
        // happened to reach first. A `schema` that is not a `u32` at all is a
        // malformed value rather than an unsupported version, so it falls
        // through to the strict parse, whose message names the real value.
        if let Some(raw) = probe.get("schema").and_then(toml::Value::as_integer)
            && let Ok(found) = u32::try_from(raw)
            && found != HOOK_SCHEMA_VERSION
        {
            return Err(HookError::UnsupportedSchema {
                found,
                supported: HOOK_SCHEMA_VERSION,
            });
        }
        match toml::from_str::<Self>(source) {
            Ok(manifest) => Ok(manifest),
            // The neither-`argv`-nor-`command` case fails inside serde with
            // `no variant of enum HookHandler found in flattened data`, which
            // names neither the entry nor a key the author wrote. It is
            // recovered structurally from the probe rather than by matching
            // that message: it is a dependency's internal string, and a
            // `serde` release is free to reword it — a string match would
            // regress to the bare parse failure S-014 forbids, silently.
            Err(e) => Err(first_entry_missing_handler(&probe).unwrap_or_else(|| HookError::Toml(Box::new(e)))),
        }
    }

    /// Validate the manifest against the `grim build` rules (exit 65 on
    /// failure).
    ///
    /// The complete set. Every rule below has exactly one [`HookError`] variant;
    /// a rule added later must be added to **this list** as well, because the
    /// Specify phase writes its tests from it.
    ///
    /// **Not the only enforcement point any more** (audit finding P-3). This
    /// function runs on the *publisher's* machine, so a manifest pushed with any
    /// other OCI client reaches an installer having satisfied none of it.
    /// [`validate_installed`](Self::validate_installed) re-applies the
    /// vendor-independent subset at the install seam and names, at that site,
    /// exactly which rules it cannot re-apply and why. A rule added here must be
    /// classified there too.
    ///
    /// 1. **Exactly one of `argv`/`command`** — enforced *here*, not by the
    ///    type. Both keys parse (see [`HookHandler`]): the surplus one lands in
    ///    [`HookEntry::vendor`], so this rule reads `vendor` for `argv` /
    ///    `command` and fails with [`HookError::AmbiguousHandler`]. The
    ///    neither-case is caught one layer up, in
    ///    [`from_toml_str`](Self::from_toml_str), as
    ///    [`HookError::MissingHandler`].
    /// 2. `tier`/`event` validity ([`HookTier::is_valid_at`]).
    /// 3. `matcher` charset and length (C-018).
    /// 4. C-019's payload-relative first-token rule.
    /// 5. Unique `id`s within the artifact.
    /// 6. Reserved client-name keys used only for vendor override tables.
    /// 7. `name` equal to the artifact directory stem.
    /// 8. **`name` is not a [`RESERVED_ARTIFACT_NAMES`] entry** — `bin`,
    ///    `dispatch.json` and `payload` belong to the launcher namespace under
    ///    `$GRIM_HOME/hooks/`, and a payload materialized over any of them arms
    ///    or disarms the dispatcher itself (or shadows every workspace's
    ///    payload root).
    /// 9. Every entry names a moment: a canonical `event`, a single
    ///    `<vendor>.event` override, or both.
    /// 10. `id` charset and length ([`hook_id_char_allowed`],
    ///     [`HOOK_ID_MAX_BYTES`]) — P-6. Defence in depth: the path sink that
    ///     made an unvalidated `id` interesting is hash-derived now, so no
    ///     control depends on this rule.
    ///
    /// **The native-only-moment rule for rule 2** (ADR decision F): an entry
    /// whose only moment is a `<vendor>.event` override has no
    /// [`CanonicalEvent`], so [`HookTier::is_valid_at`] cannot judge it and is
    /// not consulted. Such an entry may declare `observer` or `gatekeeper`
    /// only; **`mutator` on a native-only moment fails `grim build`**
    /// ([`HookError::TierNotValidAtEvent`] is the wrong shape here, so the
    /// entry is rejected as a missing-canonical-event case). Whether that
    /// native moment *actually* admits a verdict is a per-client `Declined`
    /// resolved by the `Vendor` seam, not a manifest error — the build-time
    /// rule is the vendor-independent one. The refusal is
    /// [`HookError::MutatorRequiresCanonicalEvent`], not
    /// [`HookError::TierNotValidAtEvent`], which carries a
    /// [`CanonicalEvent`] this entry does not have.
    ///
    /// # Errors
    ///
    /// One [`HookError`] variant per rule above.
    pub fn validate(&self, artifact_dir: &Path) -> Result<(), HookError> {
        // Rules 8 then 7 — the artifact's own name first. A hook named `bin`
        // fails on the name it may not have, not on some entry detail three
        // rules further down, and the reserved-name check precedes the stem
        // comparison so a directory *called* `bin` reports the reservation
        // rather than a name/stem mismatch it cannot fix by renaming the file.
        self.validate_reserved_name()?;
        let stem = artifact_dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if self.name != stem {
            return Err(HookError::NameMismatch {
                name: self.name.clone(),
                stem,
            });
        }
        self.validate_entries(artifact_dir)
    }

    /// Re-validate a **materialized** manifest at the install seam (P-3).
    ///
    /// # Why this exists at all
    ///
    /// [`validate`](Self::validate)'s only caller is `grim build`, i.e. the
    /// *publisher's* machine. A publisher who pushes with any other OCI client
    /// skips every rule in it, and the install path copies the manifest's fields
    /// into the dispatch table verbatim — the wave-7 audit proved a hand-pushed
    /// `tier = "mutator", event = "PostToolUse"` with `matcher = "Bash$(id)"`
    /// (two hard `grim build` refusals) installing at exit 0 and landing in the
    /// table unchanged. So the build-time rules were **authoring ergonomics, not
    /// a boundary**. This makes the vendor-independent ones a boundary too.
    ///
    /// `payload_dir` is the materialized payload directory, which is the
    /// installed artifact's own directory — so rule 4's payload-relative probe
    /// (C-019) asks the same filesystem question here that it asks at build.
    ///
    /// # What it deliberately does **not** re-check, and why
    ///
    /// - **Rule 7 (`name` equals the directory stem).** At install the stem is
    ///   the *binding* name, which the user chooses and is free to make differ
    ///   (`[hooks] my-guard = "…/shell-guard:1"`). The build rule compares the
    ///   manifest against the *published* directory name, a comparison this seam
    ///   cannot make, so applying it here would refuse every renamed binding.
    /// - **The binding name against [`RESERVED_ARTIFACT_NAMES`]** (rule 8 is
    ///   re-checked against `self.name`, the manifest's own name, only). A
    ///   *binding* named `bin` or `payload` still materializes over the launcher
    ///   namespace — that is the audit's separate finding P-2, and it belongs at
    ///   the seam that chooses the payload directory, not here.
    /// - **Nothing about `HookEntry::id` is exempt any more.** P-6's charset and
    ///   length rules live in the shared per-entry pass, so they hold at build
    ///   *and* here — which is what the audit asked for, and what keeps the two
    ///   seams from disagreeing about what publishes.
    /// - **Anything a client decides.** Whether a tier is honourable at an event
    ///   *for a given client*, and whether a matcher translates losslessly, are
    ///   [`Vendor::hook_registration`](crate::install::vendor::Vendor::hook_registration)'s
    ///   per-client verdict and are enforced by that call, not by a manifest
    ///   rule.
    ///
    /// # Errors
    ///
    /// The same [`HookError`] variants [`validate`](Self::validate) raises for
    /// the rules above. The install path must treat one as *drop this artifact
    /// with a warning*, never as a failed command (invariant I3).
    pub fn validate_installed(&self, payload_dir: &Path) -> Result<(), HookError> {
        self.validate_reserved_name()?;
        self.validate_entries(payload_dir)
    }

    /// Rule 8 over the manifest's **own** `name` — the half of the name check
    /// that is a property of the published manifest rather than of where it
    /// happens to be unpacked.
    fn validate_reserved_name(&self) -> Result<(), HookError> {
        // The prior question first: is this a *name*? `grim build` used to ask
        // only about collisions, so a name no consumer would accept still built
        // and released (round-2 F5). Same grammar as the binding-name gate, one
        // seam earlier — a publisher should not learn this from a consumer's
        // install log.
        if let Err(reason) = crate::skill::SkillName::parse(&self.name) {
            return Err(HookError::ArtifactNameInvalid {
                name: self.name.clone(),
                reason: reason.to_string(),
            });
        }
        if RESERVED_ARTIFACT_NAMES.contains(&self.name.as_str()) {
            return Err(HookError::ReservedArtifactName {
                name: self.name.clone(),
            });
        }
        Ok(())
    }

    /// Rules 5, 10, 1, 6, 9, 2, 3 and 4 — every per-entry rule, in the order
    /// [`validate`](Self::validate) documents.
    ///
    /// Shared by `grim build` and the install-seam re-check so the two can never
    /// drift into applying different entry rules. `artifact_dir` is the directory
    /// the entry's handler is resolved against: the source tree at build, the
    /// materialized payload at install.
    fn validate_entries(&self, artifact_dir: &Path) -> Result<(), HookError> {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for entry in &self.hooks {
            // Rule 5.
            if !seen.insert(entry.id.as_str()) {
                return Err(HookError::DuplicateId(entry.id.clone()));
            }
            // Rule 10 (P-6). Length before charset, so a rejected `id` quoted into
            // a diagnostic is always already bounded.
            if entry.id.len() > HOOK_ID_MAX_BYTES {
                return Err(HookError::IdTooLong {
                    bytes: entry.id.len(),
                    max: HOOK_ID_MAX_BYTES,
                });
            }
            if entry.id.is_empty() || !entry.id.chars().all(hook_id_char_allowed) {
                return Err(HookError::IdCharset { id: entry.id.clone() });
            }
            // Rule 1 — read from `vendor`, which is exactly where
            // `FlatMapDeserializer` put the surplus key (see [`HookHandler`]).
            // Checked before rule 6 so a shadow handler reports as one instead
            // of as an unknown override namespace.
            if entry.vendor.contains_key("argv") || entry.vendor.contains_key("command") {
                return Err(HookError::AmbiguousHandler { id: entry.id.clone() });
            }
            // Rule 6 — everything still in `vendor` must be a per-client
            // override table. This is what `deny_unknown_fields` would have
            // done for the keys it can see; a typo'd namespace must fail here
            // or the hook installs with none of its overrides.
            for (key, value) in &entry.vendor {
                if key.parse::<ClientTarget>().is_err() || !value.is_object() {
                    return Err(HookError::ReservedClientKey(key.clone()));
                }
            }
            // Rules 9 and 2. An entry whose only moment is a `<vendor>.event`
            // override has no `CanonicalEvent`, so `is_valid_at` cannot judge
            // it and is not consulted — `mutator` is refused outright, because
            // the tier is *defined* by `PreToolUse`.
            match entry.event {
                Some(event) => {
                    if !entry.tier.is_valid_at(event) {
                        return Err(HookError::TierNotValidAtEvent {
                            tier: entry.tier,
                            event,
                        });
                    }
                }
                None => {
                    if !entry.declares_native_event() {
                        return Err(HookError::MissingEvent(entry.id.clone()));
                    }
                    if entry.tier == HookTier::Mutator {
                        return Err(HookError::MutatorRequiresCanonicalEvent(entry.id.clone()));
                    }
                }
            }
            // Rule 3.
            if let Some(matcher) = &entry.matcher {
                validate_matcher(matcher)?;
            }
            // Rule 4 (C-019).
            if let Some(token) = entry.handler.first_token()
                && payload_relative_file(artifact_dir, token)
            {
                return Err(HookError::PayloadNotExecutable {
                    id: entry.id.clone(),
                    token: token.to_string(),
                });
            }
        }
        Ok(())
    }
}

impl HookEntry {
    /// Whether any per-client override table on this entry names a native
    /// moment (`<vendor>.event`) — the only way an entry with no canonical
    /// [`HookEntry::event`] still names one (validation rule 9).
    ///
    /// Reads the captured override tables rather than a parsed type: v1 stores
    /// them unparsed on purpose, and the question here is only *whether* a
    /// moment was named, never what it says. The value is not inspected beyond
    /// the key's presence — a native event name is one vendor's vocabulary and
    /// grim has no table for it.
    fn declares_native_event(&self) -> bool {
        self.vendor
            .values()
            .filter_map(serde_json::Value::as_object)
            .any(|table| table.contains_key("event"))
    }
}

/// The first `[[hooks]]` entry that names neither `argv` nor `command`, as the
/// author-facing [`HookError::MissingHandler`].
///
/// Reads the shape-agnostic parse, so it sees the keys as authored. An entry
/// with no string `id` is skipped rather than reported under a placeholder:
/// the missing `id` is the more fundamental problem and the strict parse's own
/// missing-field message names it precisely.
fn first_entry_missing_handler(probe: &toml::Value) -> Option<HookError> {
    let entries = probe.get("hooks")?.as_array()?;
    entries.iter().find_map(|entry| {
        let table = entry.as_table()?;
        let id = table.get("id").and_then(toml::Value::as_str)?;
        (!table.contains_key("argv") && !table.contains_key("command"))
            .then(|| HookError::MissingHandler { id: id.to_string() })
    })
}

/// Whether `token` names a file inside the payload tree at `artifact_dir`
/// (C-019's build-time rule).
///
/// `${GRIM_HOOK_DIR}` is stripped because C-002 exports it as the artifact's
/// own install directory, making `${GRIM_HOOK_DIR}/guard.sh` the idiom the
/// envelope invites — and the one a shell would `execve` into `EACCES`, since
/// a payload delivered through a registry arrives `0o644`.
///
/// **The token is publisher-authored, so it reaches nothing but an `is_file`
/// probe under `artifact_dir`.** Anything absolute, drive-prefixed, or
/// carrying a `..` component is *not* payload-relative by definition and
/// returns `false` without a filesystem call — refusing to probe rather than
/// canonicalizing, because a name like `../../etc/shadow` is an interpreter
/// path as far as this rule is concerned and grim has no business stat-ing it.
fn payload_relative_file(artifact_dir: &Path, token: &str) -> bool {
    let relative = token
        .strip_prefix("${GRIM_HOOK_DIR}/")
        .or_else(|| token.strip_prefix("$GRIM_HOOK_DIR/"))
        .unwrap_or(token);
    let mut normalized = std::path::PathBuf::new();
    for component in Path::new(relative).components() {
        match component {
            std::path::Component::Normal(part) => normalized.push(part),
            // `./guard.sh` is the same payload file as `guard.sh`.
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return false;
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return false;
    }
    artifact_dir.join(normalized).is_file()
}

/// Validate one `matcher` value against C-018's allowlist and length cap.
///
/// Belt and braces with the splice layer, which escapes keys and string values
/// through `serde_json` regardless: #56's whole lesson is not to depend on a
/// rule one layer up.
///
/// # Errors
///
/// [`HookError::MatcherEmpty`] for `""`; [`HookError::MatcherTooLong`] past
/// [`MATCHER_MAX_BYTES`]; [`HookError::MatcherCharset`] for a character
/// [`matcher_char_allowed`] rejects.
///
/// The length cap is checked **before** the charset, so the rejected value that
/// [`HookError::MatcherCharset`] quotes into a log line is always already
/// bounded — a manifest is untrusted input, and the diagnostic must not become
/// the way a publisher writes a megabyte into someone's terminal.
pub fn validate_matcher(matcher: &str) -> Result<(), HookError> {
    // Owed to `HookDecline::MatcherEmpty` (`src/install/vendor.rs`): Copilot
    // rejects an empty matcher outright while Claude reads it as match-all, so
    // no translation is both faithful and non-skipped. Refusing it at build
    // makes that seam's backstop unreachable, which is the direction a
    // guardrail contract should travel — the alternative is a hook that reports
    // as installed and matches either nothing or everything, depending on the
    // client.
    if matcher.is_empty() {
        return Err(HookError::MatcherEmpty);
    }
    if matcher.len() > MATCHER_MAX_BYTES {
        return Err(HookError::MatcherTooLong {
            actual: matcher.len(),
            limit: MATCHER_MAX_BYTES,
        });
    }
    if !matcher.chars().all(matcher_char_allowed) {
        return Err(HookError::MatcherCharset {
            matcher: matcher.to_string(),
            allowed: MATCHER_ALLOWED,
        });
    }
    Ok(())
}

/// Where a client keeps its hook registrations, and therefore how grim writes
/// one (C-005).
///
/// The install *shape*, not the path: each vendor resolves its own target. A
/// vendor that names no surface declines hooks, which is why
/// `Vendor::hook_surface` defaults to `None` — a forgotten vendor fails safe
/// instead of silently claiming support (ADR decision A).
///
/// **This enum is scope-blind, and scope is a hard gate — do not read
/// `hook_surface() == Some(_)` as "install here at any scope."** ADR amendment
/// A1 reduces project scope to **claude only**: codex and copilot are
/// **global-only**, because their registration files (`.codex/hooks.json`,
/// `.github/hooks/*.json`) are *committed* repository files, and anything
/// armable living inside a repository violates invariant **I1** (attacker T3,
/// a repo you cloned but do not control). The gate is the shipped
/// `Vendor::kind_surface(kind, scope) -> bool` seam and its pinned `SCOPE_GAPS`
/// set in `src/install/vendor.rs` — the identical mechanism already used for
/// Junie-rules-at-global and OpenClaw-skills-at-project. Add the two hook rows
/// there; never widen `kind_support`, and never encode scope in this enum.
///
/// **Never `match` a variant of this enum into a panic.** An unhandled surface
/// resolves to `Declined` plus one warning — the same failure direction every
/// other unsupported `(client, kind)` pair takes (invariant I3). That matters
/// today for [`CodegenModule`](Self::CodegenModule), which nothing constructs
/// at v1: a `match surface` arm reaching for `unimplemented!()` would
/// re-create, inside new code, exactly the reachable-panic defect the hook
/// marker arms were corrected for.
///
/// Not serialized: this is an internal install-shape discriminant with no wire,
/// file, or schema form (see [`HookRegistration`] for why the derives are gone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookSurface {
    /// grim wholly owns the registration file (codex, copilot) — **global
    /// scope only** per A1; see the type doc.
    OwnFile,
    /// grim splices a managed member into a config the client owns (claude) —
    /// the only surface grim writes at project scope.
    SpliceConfig,
    /// grim generates a module from a template. **No v1 implementor** — the
    /// variant exists so the seam does not need reshaping when
    /// opencode/kilo/amp/openclaw land. Until one lands, every `match` on it
    /// resolves to `Declined` + warn, **never** a panic.
    #[expect(
        dead_code,
        reason = "**No v1 implementor**, by design and documented as such above. NO REMOVAL \
                  TRIGGER for WP-K: this variant becomes live only when a codegen-surface client \
                  lands, which is why the module-wide attribute that used to cover it was \
                  undischargeable rather than merely not-yet-discharged"
    )]
    CodegenModule,
}

/// The command form a client's registration field accepts.
///
/// Not cosmetic: exec form removes shell quoting *and* shell expansion from the
/// client boundary, which is why claude's project-scope registration is safe to
/// carry an absolute launcher path. A shell string cannot carry a fail-open
/// guard in exec form and so needs the `[ -x "$L" ] || exit 0` prefix — testing
/// **the launcher**, never `grim` on `$PATH`, because a failed `exec` exits 127
/// and never reaches `|| exit 0`.
///
/// Not serialized — see [`HookRegistration`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookCommand {
    /// Exec-form argv (claude): `["<launcher>", "run", "--client", …]`.
    ///
    /// **Never constructed in v1.** WP-B established by execution that claude
    /// has no argv array — its `command` string is run by `/bin/sh` with full
    /// expansion — so every v1 registration is a [`Shell`](Self::Shell) carrying
    /// the fail-open guard. The variant stays because the *seam* is argv-shaped
    /// and a client with a real exec form must not need it reshaped.
    #[expect(
        dead_code,
        reason = "never constructed in v1 (hook_launcher.rs documents it: all three clients take a \
                  guard-prefixed shell string). NO REMOVAL TRIGGER for WP-K: it becomes live when a \
                  client with a real exec-form registration field lands"
    )]
    Argv(Vec<String>),
    /// A single shell string (codex, copilot), guard-prefixed and quoted.
    Shell(String),
}

/// One dispatcher registration: what a vendor writes into its own hook surface
/// for a `(client, event, scope, matcher)`.
///
/// **Every string here is grim-owned** (C-018b): the command is assembled from
/// grim's own literals plus the absolute launcher path grim resolved at install
/// time. `matcher` is the publisher's value *translated into this client's
/// dialect* and lands in the client's structured matcher field, never in the
/// command text.
///
/// **This type is data, not a constructor — WP-A ships no assembly seam.** The
/// builder is `Vendor::hook_registration` (C-005/C-025, WP-F,
/// `src/install/vendor.rs`), and **C-018b's test obligation travels with it**:
/// building a registration from a metacharacter-laden manifest and asserting a
/// byte-identical command string needs the builder, and adding a second
/// assembly site here to have something to test would be the very duplication
/// C-018b and C-021 exist to prevent.
///
/// **Deliberately not `Serialize`/`Deserialize`/`JsonSchema`.** This is the
/// vendor-neutral *description* of a registration, not any vendor's file
/// format: no vendor writes `command: {"argv": [...]}` or
/// `{"shell": "..."}`, codex spells the Windows form as a top-level
/// `commandWindows` while copilot spells it `powershell`, and claude's entry is
/// a member spliced into a matcher group rather than a standalone object. It
/// reaches no wire and no `grim schema --kind hook` output. Each vendor's own
/// writer converts this into that vendor's shape; deriving serde here would
/// invite someone to write the struct straight into a client's config and
/// produce a file no client reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRegistration {
    /// The client's own name for the firing event (`hook_event_name`).
    pub event: String,
    /// The matcher in this client's dialect, or `None` for "every tool".
    pub matcher: Option<String>,
    /// The command the client invokes.
    pub command: HookCommand,
    /// The Windows form where the client has a separate field for it
    /// (codex `commandWindows`, copilot `powershell`).
    pub command_windows: Option<String>,
    /// Client-side timeout in seconds, where the surface accepts one. Grim
    /// enforces the authored timeout itself regardless; this is a backstop.
    pub timeout: Option<u64>,
}

/// The **vendor tokens** one verdict target accepts, per canonical verdict.
///
/// **Added by WP-K's Implement phase (its Specify finding F-E), because the
/// table gated field *names* and nothing held the per-pair *value*.** The
/// vocabulary genuinely differs per target, and the differences are not
/// derivable from the field name:
///
/// | Target | allow | deny | ask |
/// |---|---|---|---|
/// | claude·`PreToolUse` `permissionDecision` | `allow` | `deny` | `ask` |
/// | claude·`PostToolUse` / `Stop` `decision` | ⊘ (absence allows) | `block` | ⊘ |
/// | codex·`PreToolUse` `decision` | `approve` | `block` | ⊘ |
/// | codex·`PreToolUse` `permissionDecision` | `allow` | `deny` | `ask` |
/// | copilot·`Stop` `decision` | `allow` | `block` | ⊘ |
///
/// So codex's `PreToolUse` row spells one canonical `deny` as **`block` in one
/// field and `deny` in the other**, and a projector working from the field name
/// alone would write the wrong literal into the coarse channel. `None` means the
/// target has no spelling for that verdict: absence is how every one of them
/// says "allow", which is why a permissive verdict with no token is dropped
/// while a **restrictive** one is refused outright (see
/// [`ProjectionError`](crate::command::hook::projector::ProjectionError)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerdictTokens {
    /// The token that lets the operation proceed — and, on claude and copilot,
    /// **suppresses the client's own approval prompt**, which is why it is a
    /// privilege statement rather than a no-op.
    pub allow: Option<&'static str>,
    /// The token that blocks the operation. Present at every verdict target of
    /// every shipped row, which is what makes "all verdict targets are written
    /// together, never a subset" true of a `deny` by data rather than by hope.
    pub deny: Option<&'static str>,
    /// The token that escalates to the user. Only the `permissionDecision`
    /// targets have one.
    pub ask: Option<&'static str>,
}

/// The field that echoes the **firing** event's native name, where a client
/// requires it.
///
/// Declared beside the table rather than in the runtime projector (WP-K's stub
/// finding F-6): it is a projection fact, and a projection fact spelled outside
/// [`RESPONSE_PROJECTION`] is a second, one-fact table — exactly the drift C-021
/// exists to prevent. Which clients require it is now
/// [`ProjectionRow::event_echo`], because it is **not** derivable from a row:
/// keying on "some field nested under `hookSpecificOutput`" would wrongly
/// include copilot's `PreToolUse`.
pub const EVENT_ECHO_FIELD: &str = "hookSpecificOutput.hookEventName";

/// One row of the per-`(vendor, event)` response-projection table.
///
/// Field values are the **vendor's own** JSON spellings, dotted from the root
/// of that vendor's hook response object. `None` (and, for
/// [`verdict`](Self::verdict), the empty slice) is the ADR's `⊘`: the canonical
/// field has no equivalent, so it is dropped with a one-time warning
/// (`Degraded`) — and a tier that *requires* it is `Declined`, never degraded.
///
/// **Eight columns, because a projection needs eight facts** — six for C-004's
/// field names, and two the runtime projector proved were missing the moment it
/// was written (WP-K's F-E and F-6). A row that carried only
/// verdict/context/mutation could not express four things the table actually
/// says, and a consumer that needs one would have to build a second
/// hand-maintained `(vendor, event)` map — the exact duplicate C-021 exists to
/// prevent:
///
/// - **A verdict can be two fields.** codex·`PreToolUse` blocks through the
///   top-level `decision` **and** `hookSpecificOutput.permissionDecision`, so
///   [`verdict`](Self::verdict) is a slice, not a single name.
/// - **Every verdict has a differently-spelled reason companion**, and the
///   spelling varies per pair ([`reason`](Self::reason)). Codex enforces its
///   presence in its *output parser* rather than its JSON schema, which is the
///   fail-closed bug this column exists to keep the projector out of.
/// - **The field name does not determine the verdict's *value*.** One canonical
///   `deny` is `block` in codex's coarse `decision` and `deny` in its
///   `permissionDecision` — on the *same row* — so
///   [`verdict_tokens`](Self::verdict_tokens) is per target.
/// - **The event echo is per row, not per client**
///   ([`event_echo`](Self::event_echo)), because "some field nested under
///   `hookSpecificOutput`" would wrongly include copilot's `PreToolUse`.
///
/// Columns are **append-only** and their positions are frozen: every reader
/// consults the row through [`projection_for`], and a removed column is a
/// consumer that starts guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectionRow {
    /// `Vendor::name()` of the client this row describes.
    pub client: &'static str,
    /// The canonical event.
    pub event: CanonicalEvent,
    /// Every field a blocking verdict must be written to, in the vendor's own
    /// spelling. **All** of them are written together, never a subset — codex's
    /// `PreToolUse` carries the verdict in two places and honours neither half
    /// alone. Empty ⇒ `⊘` ⇒ this client cannot block here, so `gatekeeper` is
    /// `Declined` for the pair (and [`admits_verdict`] is the query over it).
    ///
    /// [`admits_verdict`]: CanonicalEvent::admits_verdict
    pub verdict: &'static [&'static str],
    /// Where the human-readable justification accompanying a verdict goes.
    /// `None` exactly when [`verdict`](Self::verdict) is empty — a reason with
    /// no verdict to explain is not a thing any v1 client accepts.
    pub reason: Option<&'static str>,
    /// Where added context goes; `None` ⇒ dropped with a warning.
    pub context: Option<&'static str>,
    /// Where a rewritten tool input goes; `None` ⇒ `mutator` is `Declined` for
    /// the pair.
    pub mutation: Option<&'static str>,
    /// Fields that **fail the render** if emitted for this pair. Closed sets,
    /// not advisory: Codex reserves some upstream and fails **closed** when it
    /// sees them.
    pub forbidden: &'static [&'static str],
    /// The vendor token vocabulary of each [`verdict`](Self::verdict) target,
    /// **index-aligned** with it (`verdict_tokens[i]` describes `verdict[i]`).
    ///
    /// Index-aligned rather than a map, because a map would key on the field
    /// name — the one thing that does *not* determine the vocabulary (codex's
    /// `PreToolUse` proves it: two targets, two different token sets). The
    /// alignment is pinned by `verdict_tokens_align_with_their_targets` below,
    /// so a row that grows a target without its tokens fails a test rather than
    /// silently writing nothing there.
    pub verdict_tokens: &'static [VerdictTokens],
    /// Where the firing event's native name must be echoed
    /// ([`EVENT_ECHO_FIELD`]), or `None` when this pair requires no echo.
    ///
    /// Constant across a client's four rows today, and still a per-row column:
    /// the alternative is the per-client list the runtime used to carry, which is
    /// a second table keyed on a client name (C-021).
    pub event_echo: Option<&'static str>,
}

/// The one instance of the `(vendor, event)` projection table (C-004, C-021).
///
/// Consumed by both the render-time refusal and the runtime projector. Adding a
/// client means adding rows **here**; a second copy anywhere else is the defect
/// C-021 exists to prevent.
///
/// Two rules follow from the table rather than from any client's docs. Grim
/// registers **PascalCase** event names on Copilot, because Copilot's stdin
/// payload shape differs by the casing used in config and the PascalCase path
/// is the Claude-shaped one. And `hookSpecificOutput.hookEventName` must echo
/// the *firing* event on claude and codex, so the projector needs the native
/// event name and not only the canonical one.
///
/// Native-only moments (codex's `PermissionRequest`) are deliberately **absent**:
/// they are not canonical events, so they are reached through a `<vendor>.event`
/// override and projected by that vendor's native passthrough, not by this
/// table.
pub const RESPONSE_PROJECTION: &[ProjectionRow] = &[
    ProjectionRow {
        client: "claude",
        event: CanonicalEvent::PreToolUse,
        // `hookSpecificOutput.hookEventName` is additionally a required const
        // here; it echoes the *firing* event and is emitted by the projector
        // for every claude/codex row, so it is not a per-row column.
        verdict: &["hookSpecificOutput.permissionDecision"],
        reason: Some("hookSpecificOutput.permissionDecisionReason"),
        context: Some("hookSpecificOutput.additionalContext"),
        mutation: Some("hookSpecificOutput.updatedInput"),
        forbidden: &[],
        verdict_tokens: &[VerdictTokens {
            allow: Some("allow"),
            deny: Some("deny"),
            ask: Some("ask"),
        }],
        event_echo: Some(EVENT_ECHO_FIELD),
    },
    ProjectionRow {
        client: "claude",
        event: CanonicalEvent::PostToolUse,
        verdict: &["decision"],
        reason: Some("reason"),
        context: Some("hookSpecificOutput.additionalContext"),
        mutation: None,
        forbidden: &["updatedInput"],
        verdict_tokens: &[VerdictTokens {
            allow: None,
            deny: Some("block"),
            ask: None,
        }],
        event_echo: Some(EVENT_ECHO_FIELD),
    },
    ProjectionRow {
        client: "claude",
        event: CanonicalEvent::SessionStart,
        verdict: &[],
        reason: None,
        context: Some("hookSpecificOutput.additionalContext"),
        mutation: None,
        forbidden: &["decision", "updatedInput"],
        verdict_tokens: &[],
        event_echo: Some(EVENT_ECHO_FIELD),
    },
    ProjectionRow {
        client: "claude",
        event: CanonicalEvent::Stop,
        // C-004 documents a second, equivalent blocking form here —
        // `continue: false` + `stopReason`. Grim projects the `decision` form
        // only: one shape per pair is what makes the render-time forbidden-set
        // check decidable, and both forms block identically.
        verdict: &["decision"],
        reason: Some("reason"),
        context: None,
        mutation: None,
        forbidden: &["updatedInput"],
        verdict_tokens: &[VerdictTokens {
            allow: None,
            deny: Some("block"),
            ask: None,
        }],
        event_echo: Some(EVENT_ECHO_FIELD),
    },
    ProjectionRow {
        client: "codex",
        event: CanonicalEvent::PreToolUse,
        // Two fields, both required: Codex reads the top-level `decision` and
        // the nested `permissionDecision`, and honours neither half alone.
        verdict: &["decision", "hookSpecificOutput.permissionDecision"],
        reason: Some("reason"),
        context: Some("hookSpecificOutput.additionalContext"),
        mutation: Some("hookSpecificOutput.updatedInput"),
        forbidden: &[],
        verdict_tokens: &[
            VerdictTokens {
                allow: Some("approve"),
                deny: Some("block"),
                ask: None,
            },
            VerdictTokens {
                allow: Some("allow"),
                deny: Some("deny"),
                ask: Some("ask"),
            },
        ],
        event_echo: Some(EVENT_ECHO_FIELD),
    },
    ProjectionRow {
        client: "codex",
        event: CanonicalEvent::PostToolUse,
        verdict: &["decision"],
        reason: Some("reason"),
        context: Some("hookSpecificOutput.additionalContext"),
        // `updatedMCPToolOutput` is a *result* rewrite, not an input rewrite —
        // native-only, and not a mutation target.
        mutation: None,
        forbidden: &["updatedInput"],
        verdict_tokens: &[VerdictTokens {
            allow: None,
            deny: Some("block"),
            ask: None,
        }],
        event_echo: Some(EVENT_ECHO_FIELD),
    },
    ProjectionRow {
        client: "codex",
        event: CanonicalEvent::SessionStart,
        verdict: &[],
        reason: None,
        context: Some("hookSpecificOutput.additionalContext"),
        mutation: None,
        forbidden: &["decision", "updatedInput"],
        verdict_tokens: &[],
        event_echo: Some(EVENT_ECHO_FIELD),
    },
    ProjectionRow {
        client: "codex",
        event: CanonicalEvent::Stop,
        verdict: &["decision"],
        // REQUIRED when blocking — enforced in Codex's output parser, not in
        // its JSON schema, so an omitted `reason` fails **closed** rather than
        // validating. This column is where the projector reads it from.
        reason: Some("reason"),
        context: None,
        mutation: None,
        forbidden: &["updatedInput"],
        verdict_tokens: &[VerdictTokens {
            allow: None,
            deny: Some("block"),
            ask: None,
        }],
        event_echo: Some(EVENT_ECHO_FIELD),
    },
    ProjectionRow {
        client: "copilot",
        event: CanonicalEvent::PreToolUse,
        verdict: &["hookSpecificOutput.permissionDecision"],
        reason: Some("hookSpecificOutput.permissionDecisionReason"),
        context: Some("hookSpecificOutput.additionalContext"),
        // Open Question 2, resolved the other way by execution (WP-B § 3.3):
        // `modifiedArgs` and `updatedInput` are BOTH real, each working in
        // exactly one of Copilot's two dialects — selected by the casing of the
        // event key. Grim registers PascalCase (WP-B requirement 1, forced
        // because camelCase makes `matcher = "Bash"` never fire and skips `*`
        // as an invalid regex), so the Claude-compat spelling is the live one
        // here and the mutation applies. The earlier `Declined` was the
        // documentation-only answer; do not restore it.
        mutation: Some("hookSpecificOutput.updatedInput"),
        forbidden: &[],
        verdict_tokens: &[VerdictTokens {
            allow: Some("allow"),
            deny: Some("deny"),
            ask: Some("ask"),
        }],
        event_echo: None,
    },
    ProjectionRow {
        client: "copilot",
        event: CanonicalEvent::PostToolUse,
        verdict: &[],
        reason: None,
        context: Some("additionalContext"),
        // `modifiedResult` is a result rewrite — native-only.
        mutation: None,
        forbidden: &["updatedInput"],
        verdict_tokens: &[],
        event_echo: None,
    },
    ProjectionRow {
        client: "copilot",
        event: CanonicalEvent::SessionStart,
        verdict: &[],
        reason: None,
        // NOT DOCUMENTED upstream — treated as absent rather than guessed.
        context: None,
        mutation: None,
        forbidden: &["decision"],
        verdict_tokens: &[],
        event_echo: None,
    },
    ProjectionRow {
        client: "copilot",
        // Runaway guard: 8 consecutive blocks and the CLI forces the turn to
        // end. Grim does not model that; it is the client's own ceiling.
        event: CanonicalEvent::Stop,
        verdict: &["decision"],
        reason: Some("reason"),
        context: None,
        mutation: None,
        forbidden: &["updatedInput"],
        verdict_tokens: &[VerdictTokens {
            allow: Some("allow"),
            deny: Some("block"),
            ask: None,
        }],
        event_echo: None,
    },
];

/// The projection row for a `(client, event)` pair, or `None` when that client
/// hosts no hook at that event (⇒ `Declined`, warn, zero outputs).
///
/// The one lookup into [`RESPONSE_PROJECTION`]; both the render-time refusal
/// and the runtime projector go through it rather than scanning the table
/// themselves (C-021).
pub fn projection_for(client: &str, event: CanonicalEvent) -> Option<&'static ProjectionRow> {
    RESPONSE_PROJECTION
        .iter()
        .find(|row| row.client == client && row.event == event)
}

/// A hook manifest rejected at parse or `grim build` validation time — plus
/// the one seam refusal, [`HookError::UnsupportedKind`].
///
/// Every variant classifies as DataError (65). Nothing here is raised by the
/// **dispatcher**: its only failure direction is exit 0 with one log line (ADR
/// decision G), so a runtime variant would be a contradiction rather than an
/// omission. [`UnsupportedKind`](Self::UnsupportedKind) is not a dispatcher
/// failure — it is grim refusing a hook artifact at a seam that does not
/// implement the kind yet, which is a data refusal like any other.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HookError {
    /// The TOML source did not parse, or carried an unknown top-level key.
    #[error("invalid hook manifest: {0}")]
    Toml(#[source] Box<toml::de::Error>),
    /// The `schema` value is not one this grim understands.
    #[error("hook manifest schema version {found} is not supported (this grim understands {supported})")]
    UnsupportedSchema {
        /// The version the manifest declares.
        found: u32,
        /// The version this binary understands ([`HOOK_SCHEMA_VERSION`]).
        supported: u32,
    },
    /// `name` does not equal the artifact directory stem.
    #[error("hook manifest name '{name}' must equal the directory stem '{stem}'")]
    NameMismatch {
        /// The authored name.
        name: String,
        /// The directory stem it must match.
        stem: String,
    },
    /// Two `[[hooks]]` entries share an `id`.
    #[error("duplicate hook id '{0}'")]
    DuplicateId(String),
    /// An `id` is empty or carries a character outside
    /// [`hook_id_char_allowed`] (P-6).
    #[error("invalid hook id '{id}': expected only ASCII letters, digits, '_', '-' and '.'")]
    IdCharset {
        /// The rejected id. Already length-bounded — [`HookError::IdTooLong`] is
        /// checked first.
        id: String,
    },
    /// An `id` is longer than [`HOOK_ID_MAX_BYTES`]. Does **not** quote the
    /// value: quoting it is what the cap exists to prevent.
    #[error("hook id is {bytes} bytes, over the {max}-byte limit")]
    IdTooLong { bytes: usize, max: usize },
    /// A `matcher` carries a character outside [`MATCHER_ALLOWED`].
    #[error("invalid matcher '{matcher}': expected only [{allowed}]")]
    MatcherCharset {
        /// The rejected matcher.
        matcher: String,
        /// The allowed character class ([`MATCHER_ALLOWED`]).
        allowed: &'static str,
    },
    /// A `matcher` is the empty string.
    ///
    /// Not a charset failure — `""` violates no character rule, which is
    /// exactly why it needs its own variant rather than a misleading
    /// "expected only [...]" message. It is refused because the v1 clients
    /// disagree on what it *means*: Copilot rejects it and skips the hook,
    /// Claude reads it as match-all. Omit `matcher` entirely for match-all.
    #[error("invalid matcher: an empty matcher is ambiguous across clients; omit 'matcher' to match every tool")]
    MatcherEmpty,
    /// A `matcher` exceeds [`MATCHER_MAX_BYTES`].
    #[error("matcher of {actual} bytes exceeds the {limit}-byte limit")]
    MatcherTooLong {
        /// Authored length in bytes.
        actual: usize,
        /// The enforced cap.
        limit: usize,
    },
    /// A tier declared on an event that cannot honour it — `mutator` outside
    /// `PreToolUse`, or `gatekeeper` on an event admitting no verdict.
    #[error("tier '{tier}' is not valid at event '{event}'")]
    TierNotValidAtEvent {
        /// The declared tier.
        tier: HookTier,
        /// The declared event.
        event: CanonicalEvent,
    },
    /// An entry that names neither a canonical `event` nor a `<vendor>.event`.
    #[error("hook '{0}' declares no event: set 'event' or a single '<vendor>.event' override")]
    MissingEvent(String),
    /// C-019: the handler's first token resolves to a payload-relative file.
    /// A payload fetched through OCI arrives `0o644`, so the exec bit is never
    /// load-bearing — the message names the interpreter form instead.
    #[error(
        "hook '{id}' runs the payload file '{token}' directly; \
         a payload delivered through a registry is not executable — name an interpreter instead, \
         e.g. argv = [\"sh\", \"${{GRIM_HOOK_DIR}}/{token}\"]"
    )]
    PayloadNotExecutable {
        /// The offending entry id.
        id: String,
        /// The first token that resolved to a payload file.
        token: String,
    },
    /// A `[[hooks]]` key that is not a per-client override table — either an
    /// unknown key (a typo'd namespace: `cursour.event`), or a real client name
    /// carrying something other than a table (`claude = "yes"`).
    ///
    /// One variant for both shapes because the entry admits exactly one kind of
    /// extra key, so both readings are the same rule ([`HookEntry`] carries no
    /// `deny_unknown_fields`; this rule is what closes it). The message names
    /// the requirement rather than the shape, since an author who typo'd a
    /// namespace and an author who wrote a scalar need the same correction.
    #[error("key '{0}' is not a per-client override table: expected '<client>.<field>' naming a client grim supports")]
    ReservedClientKey(String),
    /// The artifact name collides with the launcher namespace
    /// ([`RESERVED_ARTIFACT_NAMES`]).
    #[error(
        "hook artifact name '{name}' is reserved: \
         '{name}' names part of grim's own hook launcher under $GRIM_HOME/hooks/ — rename the artifact"
    )]
    ReservedArtifactName {
        /// The rejected artifact name.
        name: String,
    },
    /// The artifact name is not a plain artifact name at all — it cannot become
    /// a directory under `$GRIM_HOME` (round-2 finding F5).
    ///
    /// Asked at `grim build` so a publisher learns at authoring time. Without
    /// it, `build` and `release` accepted a name that **every consumer refuses**
    /// (`grim add` and `installer::install_one` both run
    /// [`binding_name_refusal`]), so a hook could be published that nobody can
    /// install and the publisher learned nothing.
    #[error("hook artifact name '{name}' is not usable: {reason}")]
    ArtifactNameInvalid {
        /// The rejected artifact name.
        name: String,
        /// Why it is not a usable artifact name.
        reason: String,
    },
    /// An entry names **both** `argv` and `command`. Not caught by the type:
    /// serde's flatten resolution picks `argv` and sweeps `command` into the
    /// vendor override map, so a shadow handler would otherwise ship silently
    /// (see [`HookHandler`]).
    #[error("hook '{id}' declares both 'argv' and 'command'; exactly one of them is required")]
    AmbiguousHandler {
        /// The offending entry id.
        id: String,
    },
    /// An entry names **neither** `argv` nor `command`. Raised by
    /// [`HookManifest::from_toml_str`], which re-maps serde's internal
    /// flattened-enum failure into this message.
    #[error("hook '{id}' declares no handler: set exactly one of 'argv' or 'command'")]
    MissingHandler {
        /// The offending entry id.
        id: String,
    },
    /// A `mutator` on an entry whose only moment is a `<vendor>.event`
    /// override. `mutator` is defined by [`CanonicalEvent::PreToolUse`], and a
    /// native-only moment carries no canonical event to compare against, so
    /// the tier cannot be honoured — refused rather than silently degraded.
    #[error("hook '{0}' declares tier 'mutator' on a native-only event: 'mutator' requires event = \"PreToolUse\"")]
    MutatorRequiresCanonicalEvent(String),
    /// The `hook` artifact kind reached a seam this build of grim does not
    /// implement yet. Constructed only through [`unsupported_kind`].
    #[error(
        "the 'hook' artifact kind is not supported by this build of grim; \
         upgrade grim, or pin a reference that is not a hook"
    )]
    UnsupportedKind,
}

/// The one refusal every not-yet-implemented `ArtifactKind::Hook` seam returns.
///
/// During the stub phase `ArtifactKind::Hook` exists but no consumer implements
/// it, and the kind is derived from **registry-controlled** strings
/// (`artifactType`, the config media type, the `com.grimoire.kind` annotation),
/// so a published artifact can drive `grim add` / `grim fetch` / the MCP
/// `grim_render` tool straight into a hook-kind seam with no user action beyond
/// naming a reference (attacker T1/T2). Panicking there would exit **101**,
/// bypass `classify_error`, emit **no** JSON error document, and block the user
/// — inverting invariant **I3**. This returns the typed refusal instead:
/// [`crate::error::Error::Hook`], classified as `DataError` (exit **65**), the
/// same shape every other malformed-input refusal takes.
///
/// One symbol and one message on purpose: the per-site markers stay greppable
/// (`rg 'hook kind: WP-' src/`) while every site fails identically.
pub fn unsupported_kind() -> crate::error::Error {
    crate::error::Error::Hook(HookError::UnsupportedKind)
}

#[cfg(test)]
mod tests {
    /// **`command::add` must agree with [`binding_name_refusal`] on every name.**
    ///
    /// Round-2 finding S2: `add` asks the same two questions separately, because
    /// it needs the typed `CommandError::InvalidBindingName` /
    /// `ReservedBindingName` variants that a `String` reason cannot produce. That
    /// justification is real, so the second spelling stays — but the risk it
    /// carries is that `add` and `install` come to disagree about the same name,
    /// and one of them is the security control.
    ///
    /// Round-3 finding W5: this test used to recompute `add`'s conditions in its
    /// own body, which pinned nothing — deleting both of `add`'s checks left it
    /// green. It now **calls** `add`'s seam
    /// ([`refuse_bad_binding_name`](crate::command::add::refuse_bad_binding_name)),
    /// which is why that function exists as a function at all.
    #[test]
    fn the_add_path_and_the_install_path_agree_on_every_binding_name() {
        let names = [
            "shell-guard",
            "a",
            "x.y",
            "a1.b2-c3",
            "bin",
            "dispatch.json",
            "payload",
            "root-key",
            "Bin",
            "my_hook",
            "../../victim",
            "a/b",
            "",
            ".",
            "..",
            "C:/windows",
        ];
        for name in names {
            // `add`'s ACTUAL seam, called — not a re-spelling of it. The previous
            // version of this loop recomputed `add`'s two conditions inline, so it
            // stayed green when both of `add`'s call sites were deleted (W5, M7).
            let add_refuses =
                crate::command::add::refuse_bad_binding_name(crate::oci::ArtifactKind::Hook, name).is_err();
            assert_eq!(
                add_refuses,
                binding_name_refusal(name).is_some(),
                "`grim add` and the install seam disagree about {name:?}; one of them is the \
                 control, so a divergence means a name is refused at the friendly seam and \
                 accepted at the load-bearing one, or the reverse"
            );
        }
    }

    /// **A binding name that is not a plain name is refused before it can become
    /// a directory under `$GRIM_HOME`.**
    ///
    /// The gap this pins: `is_reserved_binding_name` is exact equality against
    /// three literals, and `payload_dir` then joins the same string onto
    /// `$GRIM_HOME` with no containment check, with anchor classification running
    /// *after* materialization. A `[hooks]` key in a **committed** `grimoire.toml`
    /// never passed through `SkillName::parse`, so a cloned repository could ship
    /// `"../../../../victimdir"` and overwrite files outside `$GRIM_HOME` with
    /// published content — T3 escalating with T1, invariant I1, SEC-1's class
    /// reopened through the binding name instead of the record. Reproduced by the
    /// wave-8 review panel and re-reproduced against this fix.
    ///
    /// The traversal cases are asserted **individually rather than as a list**
    /// because each is a different escape shape, and a `contains`-style guard
    /// would pass some and fail others. `SkillName::parse`'s grammar makes all of
    /// them unrepresentable, which is why the fix is a parse rather than a
    /// blocklist — a blocklist has to anticipate the next shape.
    #[test]
    fn a_binding_name_that_is_not_a_plain_name_is_refused() {
        for name in [
            "../../../../victimdir", // the executed reproduction
            "../../../hooks/bin",    // reaches grim's own namespace past the reserved gate
            "..",
            ".",
            "a/b",           // any separator at all
            "/etc/cron.d/x", // absolute
            ".hidden",       // leading dot: a dotfile, and `SkillName` forbids it
            "Bin",           // uppercase: NOT equal to "bin", so the reserved check misses it
            "",
        ] {
            assert!(
                binding_name_refusal(name).is_some(),
                "binding name {name:?} must be refused before it becomes a payload directory"
            );
        }

        // The reserved names still refuse, and still for the reserved reason —
        // the parse check must not have shadowed them, since every one is a valid
        // `SkillName`.
        //
        // **Spelled as literals, not iterated from `RESERVED_ARTIFACT_NAMES`.**
        // Iterating the array is vacuous for a name *missing* from it, which is
        // exactly how `root-key` passed this test while a hook bound as
        // `root-key` destroyed the machine key. A literal list fails when an
        // entry is dropped; the array-driven loop could not.
        for name in ["bin", "dispatch.json", "payload", "root-key"] {
            let reason = binding_name_refusal(name).unwrap_or_else(|| panic!("{name} must be refused"));
            assert!(
                reason.contains("reserved"),
                "{name} is a valid name that collides with grim's own files, so the reason must \
                 say reserved rather than malformed: {reason}"
            );
        }

        // And an ordinary name is still accepted — the negative control, without
        // which every assertion above is satisfied by a function that refuses
        // everything.
        for name in ["shell-guard", "a", "tool-call-logger", "x.y", "a1.b2-c3"] {
            assert_eq!(
                binding_name_refusal(name),
                None,
                "{name} is a valid binding name and must install"
            );
        }
    }
    use super::*;

    /// The manifest from this module's doc comment. Every parse test starts
    /// from a document that is documented to work, so a test failure means the
    /// published example broke rather than that a fixture drifted.
    const EXAMPLE: &str = r#"
schema      = 1
name        = "shell-guard"
description = "Refuse curl-pipe-to-shell in Bash tool calls"

[[hooks]]
id      = "deny-curl-pipe-sh"
event   = "PreToolUse"
tier    = "gatekeeper"
matcher = "Bash"
argv    = ["sh", "${GRIM_HOOK_DIR}/guard.sh"]
timeout = 30
payload = "stdin"
"#;

    /// A manifest with one entry, spliced from `body` (entry keys) so each test
    /// states only the field it is about.
    fn manifest(entry_body: &str) -> String {
        format!("schema = 1\nname = \"h\"\ndescription = \"d\"\n\n[[hooks]]\n{entry_body}\n")
    }

    /// ⛔ **W1(a).** `grim build` refuses a manifest `name` that no consumer
    /// could bind, not only one that collides.
    ///
    /// Round-2 F5: `validate` asked about reserved collisions but never whether
    /// the name was a *name*, so `my_hook` and `MyHook` built and released
    /// cleanly and then failed at the consumer's install seam — the publisher
    /// learning it from someone else's log. Round-3 W1 found the fix shipped with
    /// no test: `validate_reserved_name` could drop the `SkillName::parse` call
    /// invisibly. `Bin` is here because it is the case the reserved check alone
    /// cannot catch (exact equality misses it) while the grammar does.
    #[test]
    fn a_manifest_name_that_is_not_a_plain_name_is_refused_at_build() {
        for bad in ["my_hook", "MyHook", "Bin", "a b", "a/b", ".hidden", ""] {
            let toml = format!(
                "schema = 1\nname = \"{bad}\"\ndescription = \"d\"\n\n[[hooks]]\nid = \"a\"\n\
                 event = \"PreToolUse\"\ntier = \"observer\"\nargv = [\"true\"]\n"
            );
            let m = HookManifest::from_toml_str(&toml)
                .unwrap_or_else(|e| panic!("{bad:?} must PARSE — the rule is validation: {e:?}"));
            let err = m.validate(Path::new("/tmp/h")).unwrap_err();
            assert!(
                matches!(&err, HookError::ArtifactNameInvalid { name, .. } if name == bad),
                "manifest name {bad:?} must be refused at build, got {err:?}"
            );
        }

        // The gate is a refusal of unusable names, not of unfamiliar ones: the
        // shapes a consumer can actually bind still build.
        for good in ["shell-guard", "a", "x.y", "a1.b2-c3"] {
            let toml = format!(
                "schema = 1\nname = \"{good}\"\ndescription = \"d\"\n\n[[hooks]]\nid = \"a\"\n\
                 event = \"PreToolUse\"\ntier = \"observer\"\nargv = [\"true\"]\n"
            );
            let m = HookManifest::from_toml_str(&toml).expect("parses");
            // Asserted as "not refused *for its name*" rather than fully valid:
            // `validate` also resolves handlers against `artifact_dir`, and this
            // test deliberately does not stand up a payload tree for that.
            assert!(
                !matches!(
                    m.validate(Path::new("/tmp/h")),
                    Err(HookError::ArtifactNameInvalid { .. })
                ),
                "{good:?} is a bindable name and must not be refused as one"
            );
        }
    }

    #[test]
    fn parses_the_documented_example() {
        let m = HookManifest::from_toml_str(EXAMPLE).unwrap();
        assert_eq!(m.schema, HOOK_SCHEMA_VERSION);
        assert_eq!(m.name, "shell-guard");
        assert_eq!(m.hooks.len(), 1);
        let entry = &m.hooks[0];
        assert_eq!(entry.id, "deny-curl-pipe-sh");
        assert_eq!(entry.event, Some(CanonicalEvent::PreToolUse));
        assert_eq!(entry.tier, HookTier::Gatekeeper);
        assert_eq!(entry.matcher.as_deref(), Some("Bash"));
        assert_eq!(entry.timeout, Some(30));
        assert_eq!(entry.payload, Some(HookPayloadMode::Stdin));
        // § 5.3 risk 1: `HookEntry` carries TWO `#[serde(flatten)]` fields, and
        // which one claims `argv` is ordering-sensitive. Asserted, not assumed:
        // a mis-claim would route the handler into the vendor bag, and the hook
        // would install and run nothing.
        assert_eq!(
            entry.handler,
            HookHandler::Argv(vec!["sh".to_string(), "${GRIM_HOOK_DIR}/guard.sh".to_string()])
        );
        assert!(
            entry.vendor.is_empty(),
            "vendor bag captured a typed key: {:?}",
            entry.vendor
        );
        assert!(entry.policy.is_none());
    }

    #[test]
    fn round_trips_through_toml_with_the_handler_flattened() {
        let m = HookManifest::from_toml_str(EXAMPLE).unwrap();
        let emitted = toml::to_string(&m).unwrap();
        // The variant name IS the authored key — not a `handler = { argv = … }`
        // wrapper, which no third-party reader of the published format expects.
        assert!(emitted.contains("argv = ["), "handler was not flattened: {emitted}");
        assert_eq!(HookManifest::from_toml_str(&emitted).unwrap(), m);
    }

    #[test]
    fn command_handler_parses_as_the_lesser_form() {
        let m = HookManifest::from_toml_str(&manifest(
            "id = \"a\"\nevent = \"Stop\"\ntier = \"observer\"\ncommand = \"sh guard.sh\"",
        ))
        .unwrap();
        assert_eq!(m.hooks[0].handler, HookHandler::Command("sh guard.sh".to_string()));
    }

    #[test]
    fn both_handlers_parse_and_then_fail_validation() {
        // B1, proven empirically rather than reasoned: supplying BOTH keys
        // deserializes cleanly, `argv` wins by declaration order, and the
        // surplus `command` is swept into the vendor catch-all. A human reading
        // the manifest bottom-up would believe `command` runs.
        let m = HookManifest::from_toml_str(&manifest(
            "id = \"a\"\nevent = \"PreToolUse\"\ntier = \"observer\"\nargv = [\"sh\", \"a.sh\"]\ncommand = \"evil.sh\"",
        ))
        .expect("both keys must PARSE — the rule is validation, not a type invariant");
        assert_eq!(m.hooks[0].handler, HookHandler::Argv(vec!["sh".into(), "a.sh".into()]));
        assert!(m.hooks[0].vendor.contains_key("command"));
        let err = m.validate(Path::new("/tmp/h")).unwrap_err();
        assert!(
            matches!(&err, HookError::AmbiguousHandler { id } if id == "a"),
            "expected AmbiguousHandler, got {err:?}"
        );
    }

    #[test]
    fn missing_handler_is_remapped_from_serde_internals() {
        let err =
            HookManifest::from_toml_str(&manifest("id = \"a\"\nevent = \"Stop\"\ntier = \"observer\"")).unwrap_err();
        assert!(
            matches!(&err, HookError::MissingHandler { id } if id == "a"),
            "expected MissingHandler, got {err:?}"
        );
        assert!(
            !err.to_string().contains("flattened data"),
            "serde's internal message leaked to the author: {err}"
        );
    }

    #[test]
    fn unsupported_schema_is_explanatory_not_a_parse_failure() {
        // S-014: an artifact published against a future schema. The `[[hooks]]`
        // shape here is deliberately one this grim cannot parse at all — the
        // schema error must win, or the author of a v2 manifest gets a field
        // error about a key they authored correctly for v2.
        let err = HookManifest::from_toml_str("schema = 2\nname = \"h\"\ndescription = \"d\"\n[[hooks]]\nwhat = 1\n")
            .unwrap_err();
        assert!(
            matches!(err, HookError::UnsupportedSchema { found: 2, supported: 1 }),
            "expected UnsupportedSchema, got {err:?}"
        );
        // A `schema` that is not a version number at all stays a parse failure:
        // it is a malformed value, and the TOML message names the real one.
        assert!(matches!(
            HookManifest::from_toml_str("schema = \"1\"\nname = \"h\"\ndescription = \"d\"\n").unwrap_err(),
            HookError::Toml(_)
        ));
    }

    #[test]
    fn unknown_top_level_key_is_refused() {
        let err =
            HookManifest::from_toml_str("schema = 1\nname = \"h\"\ndescription = \"d\"\nhookz = []\n").unwrap_err();
        assert!(matches!(err, HookError::Toml(_)), "expected Toml, got {err:?}");
    }

    #[test]
    fn matcher_allowlist_admits_the_three_lossless_forms() {
        // C-025's translation table: an exact name, alternation, and the bare
        // wildcard are the forms WP-B verified fire on all three v1 clients.
        for ok in ["Bash", "Edit|Write", "*", "mcp__server__tool", "src/*.rs", "a-b.c?"] {
            validate_matcher(ok).unwrap_or_else(|e| panic!("{ok:?} must be admitted: {e}"));
        }
    }

    #[test]
    fn matcher_charset_length_and_empty_are_refused() {
        // Length is checked first, so the charset message never quotes an
        // unbounded publisher string.
        let long = "a".repeat(MATCHER_MAX_BYTES + 1);
        assert!(matches!(
            validate_matcher(&long).unwrap_err(),
            HookError::MatcherTooLong { actual, limit } if actual == MATCHER_MAX_BYTES + 1 && limit == MATCHER_MAX_BYTES
        ));
        validate_matcher(&"a".repeat(MATCHER_MAX_BYTES)).unwrap();
        // Quotes, backslashes, `$`/backtick, whitespace, control and bidi
        // characters — the spoofing and latency-bomb classes the allowlist
        // exists for, not just the shell-metacharacter ones.
        for bad in [
            "Ba\"sh",
            "Ba\\sh",
            "$(id)",
            "`id`",
            "Ba sh",
            "Bash\n",
            "Ba\u{202e}sh",
            "Bash\u{200b}",
            "Ba;sh",
            "Ba{a,b}",
            "^Bash$",
            "(Bash)",
            "Ba+sh",
            "Ba[a]sh",
        ] {
            let err = validate_matcher(bad).unwrap_err();
            assert!(
                matches!(err, HookError::MatcherCharset { .. }),
                "{bad:?} must be a charset refusal, got {err:?}"
            );
        }
        assert!(matches!(validate_matcher("").unwrap_err(), HookError::MatcherEmpty));
    }

    #[test]
    fn matcher_char_allowed_is_the_membership_test() {
        // The constant is a range *spelling* for the diagnostic. Asserting the
        // obvious-but-wrong `MATCHER_ALLOWED.contains(c)` is wrong in BOTH
        // directions keeps someone from "simplifying" the predicate into it.
        assert!(matcher_char_allowed('B') && !MATCHER_ALLOWED.contains('B'));
        assert!(matcher_char_allowed('-') && MATCHER_ALLOWED.contains('-'));
        assert!(!matcher_char_allowed('$'));
    }

    #[test]
    fn tier_validity_follows_the_projection_table() {
        for event in CanonicalEvent::ALL {
            assert!(
                HookTier::Observer.is_valid_at(event),
                "observer must be valid everywhere"
            );
            assert_eq!(HookTier::Gatekeeper.is_valid_at(event), event.admits_verdict());
            assert_eq!(HookTier::Mutator.is_valid_at(event), event.admits_mutation());
        }
        // The concrete verdicts, so the table and the rule cannot drift together
        // into agreeing on something wrong.
        assert!(!HookTier::Gatekeeper.is_valid_at(CanonicalEvent::SessionStart));
        assert!(HookTier::Gatekeeper.is_valid_at(CanonicalEvent::PostToolUse));
        assert!(HookTier::Mutator.is_valid_at(CanonicalEvent::PreToolUse));
        assert!(!HookTier::Mutator.is_valid_at(CanonicalEvent::PostToolUse));
    }

    #[test]
    fn admits_mutation_is_pretooluse_only() {
        // Pins the agreement between the table query and the semantic claim in
        // `admits_mutation`'s doc. A row growing a `mutation` outside
        // `PreToolUse` fails HERE deliberately: nothing after the call has an
        // input left to rewrite, so such a row is a survey error.
        for event in CanonicalEvent::ALL {
            assert_eq!(event.admits_mutation(), event == CanonicalEvent::PreToolUse, "{event}");
        }
        assert_eq!(
            CanonicalEvent::ALL.map(CanonicalEvent::admits_verdict),
            [true, true, false, true]
        );
    }

    #[test]
    fn projection_table_has_one_row_per_client_and_event() {
        for client in ["claude", "codex", "copilot"] {
            for event in CanonicalEvent::ALL {
                let row = projection_for(client, event).unwrap_or_else(|| panic!("no row for {client}/{event}"));
                assert_eq!((row.client, row.event), (client, event));
                // A reason with no verdict to explain is not a thing any v1
                // client accepts, and a verdict with nowhere to put the reason
                // is the Codex fail-closed bug — so the two move together.
                assert_eq!(
                    row.verdict.is_empty(),
                    row.reason.is_none(),
                    "verdict/reason disagree for {client}/{event}"
                );
            }
        }
        assert_eq!(RESPONSE_PROJECTION.len(), 12, "one row per (3 clients x 4 events)");
        // A client with no hook surface resolves to None (⇒ Declined), never to
        // another client's row.
        assert!(projection_for("cursor", CanonicalEvent::PreToolUse).is_none());
        assert!(projection_for("", CanonicalEvent::Stop).is_none());
    }

    /// **The token column is index-aligned with the target column, and every
    /// target can spell a `deny`.**
    ///
    /// Two properties, and the second is the one that makes the table's own
    /// "**all** of them are written together, never a subset" claim checkable:
    /// codex's `PreToolUse` carries the verdict in two fields with *different*
    /// vocabularies (`block` and `deny`), so a row whose second target had no
    /// `deny` token would make grim write half a denial — a hook that reports as
    /// armed and blocks nothing.
    ///
    /// A missing `allow` or `ask` is legitimate and deliberately not asserted:
    /// absence is how `decision` says "allow", and only the `permissionDecision`
    /// targets have an `ask` at all.
    #[test]
    fn verdict_tokens_align_with_their_targets() {
        for row in RESPONSE_PROJECTION {
            assert_eq!(
                row.verdict.len(),
                row.verdict_tokens.len(),
                "{}/{}: a verdict target with no token vocabulary writes nothing where a verdict \
                 belongs",
                row.client,
                row.event
            );
            for (target, tokens) in row.verdict.iter().zip(row.verdict_tokens) {
                assert!(
                    tokens.deny.is_some(),
                    "{}/{}: `{target}` has no `deny` token, so a denial would be written to only \
                     part of this row's verdict",
                    row.client,
                    row.event
                );
            }
        }
    }

    /// The event echo is required by claude and codex and by neither copilot row
    /// — the fact `RESPONSE_PROJECTION` used to omit (WP-K stub finding F-6).
    ///
    /// Asserted per row rather than per client, because the column is per row:
    /// the point of moving it here was that "which clients require it" stops
    /// being a second table keyed on a client name.
    #[test]
    fn only_claude_and_codex_rows_echo_the_firing_event() {
        for row in RESPONSE_PROJECTION {
            let expected = matches!(row.client, "claude" | "codex").then_some(EVENT_ECHO_FIELD);
            assert_eq!(
                row.event_echo, expected,
                "{}/{}: the echo requirement moved without a decision",
                row.client, row.event
            );
        }
    }

    #[test]
    fn tier_and_event_mismatch_is_a_build_error() {
        let err = HookManifest::from_toml_str(&manifest(
            "id = \"a\"\nevent = \"PostToolUse\"\ntier = \"mutator\"\nargv = [\"sh\", \"a.sh\"]",
        ))
        .unwrap()
        .validate(Path::new("/tmp/h"))
        .unwrap_err();
        assert!(
            matches!(
                err,
                HookError::TierNotValidAtEvent {
                    tier: HookTier::Mutator,
                    event: CanonicalEvent::PostToolUse
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn native_only_moment_admits_observer_and_gatekeeper_but_never_mutator() {
        let native = |tier: &str| {
            manifest(&format!(
                "id = \"a\"\ntier = \"{tier}\"\nargv = [\"sh\", \"a.sh\"]\n\n[hooks.codex]\nevent = \"PermissionRequest\""
            ))
        };
        for tier in ["observer", "gatekeeper"] {
            HookManifest::from_toml_str(&native(tier))
                .unwrap()
                .validate(Path::new("/tmp/h"))
                .unwrap_or_else(|e| panic!("{tier} on a native-only moment must build: {e}"));
        }
        let err = HookManifest::from_toml_str(&native("mutator"))
            .unwrap()
            .validate(Path::new("/tmp/h"))
            .unwrap_err();
        assert!(
            matches!(&err, HookError::MutatorRequiresCanonicalEvent(id) if id == "a"),
            "must be the mutator-specific refusal, not TierNotValidAtEvent: {err:?}"
        );
    }

    #[test]
    fn an_entry_naming_no_moment_is_refused() {
        let err = HookManifest::from_toml_str(&manifest(
            "id = \"a\"\ntier = \"observer\"\nargv = [\"sh\", \"a.sh\"]\n\n[hooks.codex]\ntimeout = 5",
        ))
        .unwrap()
        .validate(Path::new("/tmp/h"))
        .unwrap_err();
        assert!(matches!(&err, HookError::MissingEvent(id) if id == "a"), "got {err:?}");
    }

    #[test]
    fn vendor_keys_must_name_a_supported_client_and_hold_a_table() {
        // A typo'd namespace: `HookEntry` cannot carry `deny_unknown_fields`
        // (the format reserves `<vendor>.<field>` tables and `policy`), so
        // without this rule the hook installs with none of its overrides.
        for body in [
            "id = \"a\"\nevent = \"Stop\"\ntier = \"observer\"\nargv = [\"sh\"]\n\n[hooks.cursour]\nevent = \"X\"",
            "id = \"a\"\nevent = \"Stop\"\ntier = \"observer\"\nargv = [\"sh\"]\nclaude = \"yes\"",
            "id = \"a\"\nevent = \"Stop\"\ntier = \"observer\"\nargv = [\"sh\"]\ntimeoot = 5",
        ] {
            let err = HookManifest::from_toml_str(&manifest(body))
                .unwrap()
                .validate(Path::new("/tmp/h"))
                .unwrap_err();
            assert!(
                matches!(err, HookError::ReservedClientKey(_)),
                "expected ReservedClientKey for {body:?}, got {err:?}"
            );
        }
        // Every client grim supports is a legal namespace, including ones with
        // no hook surface — the seam declines those later, per client.
        for client in ClientTarget::ALL {
            let body = format!(
                "id = \"a\"\nevent = \"Stop\"\ntier = \"observer\"\nargv = [\"sh\"]\n\n[hooks.{client}]\ntimeout = 5"
            );
            HookManifest::from_toml_str(&manifest(&body))
                .unwrap()
                .validate(Path::new("/tmp/h"))
                .unwrap_or_else(|e| panic!("'{client}' must be a legal override namespace: {e}"));
        }
    }

    #[test]
    fn duplicate_ids_are_refused() {
        let doc = format!(
            "{}\n[[hooks]]\nid = \"a\"\nevent = \"Stop\"\ntier = \"observer\"\nargv = [\"sh\"]\n",
            manifest("id = \"a\"\nevent = \"PreToolUse\"\ntier = \"observer\"\nargv = [\"sh\"]")
        );
        let err = HookManifest::from_toml_str(&doc)
            .unwrap()
            .validate(Path::new("/tmp/h"))
            .unwrap_err();
        assert!(matches!(&err, HookError::DuplicateId(id) if id == "a"), "got {err:?}");
    }

    #[test]
    fn name_must_equal_the_directory_stem() {
        let m = HookManifest::from_toml_str(EXAMPLE).unwrap();
        m.validate(Path::new("/tmp/shell-guard")).unwrap();
        let err = m.validate(Path::new("/tmp/other")).unwrap_err();
        assert!(
            matches!(&err, HookError::NameMismatch { name, stem } if name == "shell-guard" && stem == "other"),
            "got {err:?}"
        );
    }

    /// P-6: `id` is charset- and length-bounded, at build **and** at install.
    ///
    /// Before this, `validate` checked only uniqueness, and the value reached a
    /// filesystem path interpolation in `write_payload_file`. The traversal shape
    /// the audit probed is the case that matters, and `/` is what made it look
    /// plausible — so it is asserted explicitly rather than left to the charset
    /// loop. The path sink is `(pid, slot)`-derived now, so this rule is defence in
    /// depth; it still has to hold at *both* seams, or the two would disagree about
    /// what publishes.
    #[test]
    fn a_hook_id_is_charset_and_length_bounded() {
        let manifest = |id: &str| {
            format!(
                "schema = 1\nname = \"shell-guard\"\ndescription = \"d\"\n\n\
                 [[hooks]]\nid = \"{id}\"\nevent = \"PreToolUse\"\ntier = \"observer\"\n\
                 command = \"sh g.sh\"\n"
            )
        };
        let dir = Path::new("/tmp/shell-guard");

        // The audit's probe: a traversal-shaped id.
        let hostile = HookManifest::from_toml_str(&manifest("x/../../../../tmp/escaped/pwned")).unwrap();
        assert!(matches!(
            hostile.validate(dir).unwrap_err(),
            HookError::IdCharset { .. }
        ));
        // …refused at the install seam too, not only on the publisher's machine.
        assert!(matches!(
            hostile.validate_installed(dir).unwrap_err(),
            HookError::IdCharset { .. }
        ));

        // A terminal-escape attempt, which is the other sink `id` reaches. `\u001B`
        // is TOML's own escape, so the manifest carries a real ESC byte.
        let escape = HookManifest::from_toml_str(&manifest("a\\u001B[2Kb")).unwrap();
        assert_eq!(escape.hooks[0].id, "a\u{1b}[2Kb", "the fixture must carry a real ESC");
        assert!(matches!(escape.validate(dir).unwrap_err(), HookError::IdCharset { .. }));

        // Length before charset, so the diagnostic never quotes an unbounded value.
        let long = "a".repeat(HOOK_ID_MAX_BYTES + 1);
        assert!(matches!(
            HookManifest::from_toml_str(&manifest(&long)).unwrap().validate(dir),
            Err(HookError::IdTooLong { .. })
        ));
        let long_and_hostile = format!("{}/", "a".repeat(HOOK_ID_MAX_BYTES));
        let err = HookManifest::from_toml_str(&manifest(&long_and_hostile))
            .unwrap()
            .validate(dir)
            .unwrap_err();
        assert!(
            matches!(err, HookError::IdTooLong { .. }),
            "the length cap must be reported first so the charset diagnostic cannot quote a megabyte: {err:?}"
        );
        assert!(!err.to_string().contains(&long_and_hostile), "{err}");

        // And the ordinary shapes still build.
        for id in ["guard", "pre-tool-use", "a.b_c", "G9"] {
            HookManifest::from_toml_str(&manifest(id))
                .unwrap()
                .validate(dir)
                .unwrap();
        }
    }

    /// P-3: the install-seam re-check applies every per-entry rule **and** the
    /// reserved-name rule, and deliberately not the `name == stem` rule.
    ///
    /// `validate`'s only caller is `grim build`, on the publisher's machine, so
    /// the rules were authoring ergonomics rather than a boundary until
    /// `desired_entries` started calling this. The one rule it cannot re-apply is
    /// rule 7: at install the directory stem is the *binding* name, which the user
    /// chooses and may legitimately make differ from the manifest's `name`.
    #[test]
    fn the_install_time_subset_drops_the_stem_rule_and_keeps_the_rest() {
        let manifest = HookManifest::from_toml_str(EXAMPLE).unwrap();
        // Rule 7 fires for `validate`, and must not for `validate_installed` —
        // this is the whole difference between the two.
        assert!(manifest.validate(Path::new("/tmp/my-guard")).is_err());
        manifest.validate_installed(Path::new("/tmp/my-guard")).unwrap();

        // A per-entry rule still fires: C-018's matcher charset.
        let hostile = HookManifest::from_toml_str(
            "schema = 1\nname = \"shell-guard\"\ndescription = \"d\"\n\n\
             [[hooks]]\nid = \"g\"\nevent = \"PreToolUse\"\ntier = \"observer\"\n\
             matcher = \"Bash$(id)\"\ncommand = \"sh g.sh\"\n",
        )
        .unwrap();
        assert!(matches!(
            hostile.validate_installed(Path::new("/tmp/shell-guard")).unwrap_err(),
            HookError::MatcherCharset { .. }
        ));

        // …as does the tier/event rule, the second half of P-3's demonstration.
        let mutator_late = HookManifest::from_toml_str(
            "schema = 1\nname = \"shell-guard\"\ndescription = \"d\"\n\n\
             [[hooks]]\nid = \"m\"\nevent = \"PostToolUse\"\ntier = \"mutator\"\n\
             command = \"sh g.sh\"\n",
        )
        .unwrap();
        assert!(matches!(
            mutator_late
                .validate_installed(Path::new("/tmp/shell-guard"))
                .unwrap_err(),
            HookError::TierNotValidAtEvent { .. }
        ));

        // And rule 8 survives, because it reads the manifest's own `name` rather
        // than where the payload happens to be unpacked. (The *binding* name is a
        // separate question — audit finding P-2, at the seam that chooses the
        // payload directory.)
        let reserved = HookManifest::from_toml_str("schema = 1\nname = \"bin\"\ndescription = \"d\"\n").unwrap();
        assert!(matches!(
            reserved.validate_installed(Path::new("/tmp/anything")).unwrap_err(),
            HookError::ReservedArtifactName { .. }
        ));
    }

    #[test]
    fn launcher_namespace_names_are_reserved() {
        // A payload materialized over `$GRIM_HOME/hooks/bin` or
        // `.../dispatch.json` arms or disarms the dispatcher itself (T1, I1).
        // Checked before the stem comparison, so a directory genuinely called
        // `bin` reports the reservation it cannot rename its way out of.
        for name in RESERVED_ARTIFACT_NAMES {
            let doc = format!("schema = 1\nname = \"{name}\"\ndescription = \"d\"\n");
            let err = HookManifest::from_toml_str(&doc)
                .unwrap()
                .validate(Path::new("/tmp/hooks").join(name).as_path())
                .unwrap_err();
            assert!(
                matches!(&err, HookError::ReservedArtifactName { name: n } if n == name),
                "got {err:?}"
            );
        }
    }

    #[test]
    fn handler_first_token_is_the_program() {
        assert_eq!(
            HookHandler::Argv(vec!["sh".into(), "a.sh".into()]).first_token(),
            Some("sh")
        );
        assert_eq!(HookHandler::Argv(vec![]).first_token(), None);
        assert_eq!(HookHandler::Command("  sh   a.sh ".into()).first_token(), Some("sh"));
        assert_eq!(HookHandler::Command("   ".into()).first_token(), None);
    }

    #[test]
    fn running_the_payload_directly_is_refused_at_build() {
        // C-019: the exec bit is never load-bearing. A payload fetched through
        // OCI arrives 0o644, so every one of these forms would `execve` into
        // EACCES at the first tool call — which is the moment a guardrail is
        // least useful.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("h");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("guard.sh"), "#!/bin/sh\n").unwrap();
        for token in [
            "${GRIM_HOOK_DIR}/guard.sh",
            "$GRIM_HOOK_DIR/guard.sh",
            "guard.sh",
            "./guard.sh",
        ] {
            let body = format!("id = \"a\"\nevent = \"Stop\"\ntier = \"observer\"\ncommand = \"{token} --flag\"");
            let err = HookManifest::from_toml_str(&manifest(&body))
                .unwrap()
                .validate(&root)
                .unwrap_err();
            assert!(
                matches!(&err, HookError::PayloadNotExecutable { id, token: t } if id == "a" && t == token),
                "{token:?} must be refused, got {err:?}"
            );
            // The message must teach the interpreter form, since the exec bit
            // is not the fix.
            assert!(err.to_string().contains("argv = [\"sh\""), "unhelpful message: {err}");
        }
        // An interpreter is fine — including one that merely shares a name with
        // nothing in the payload tree.
        for ok in [
            "argv = [\"sh\", \"${GRIM_HOOK_DIR}/guard.sh\"]",
            "command = \"sh ${GRIM_HOOK_DIR}/guard.sh\"",
            "command = \"/usr/bin/env python3 ${GRIM_HOOK_DIR}/guard.sh\"",
            "argv = [\"absent.sh\"]",
        ] {
            let body = format!("id = \"a\"\nevent = \"Stop\"\ntier = \"observer\"\n{ok}");
            HookManifest::from_toml_str(&manifest(&body))
                .unwrap()
                .validate(&root)
                .unwrap_or_else(|e| panic!("{ok:?} must build: {e}"));
        }
    }

    #[test]
    fn a_traversing_first_token_is_never_probed_as_a_payload_file() {
        // `..` and absolute forms are interpreter paths as far as C-019 is
        // concerned: publisher-authored, so they reach no filesystem call at
        // all rather than being canonicalized and stat-ed outside the payload.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("h");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(dir.path().join("outside.sh"), "x").unwrap();
        assert!(!payload_relative_file(&root, "../outside.sh"));
        assert!(!payload_relative_file(&root, "${GRIM_HOOK_DIR}/../outside.sh"));
        assert!(!payload_relative_file(&root, "/bin/sh"));
        assert!(!payload_relative_file(&root, "${GRIM_HOOK_DIR}"));
        assert!(!payload_relative_file(&root, ""));
    }

    #[test]
    fn policy_and_vendor_overrides_round_trip_unparsed() {
        let doc = manifest(
            "id = \"a\"\nevent = \"PreToolUse\"\ntier = \"observer\"\nargv = [\"sh\"]\npolicy = { mode = \"warn\", limits = { calls = 3 } }\n\n[hooks.claude]\ntimeout = 90",
        );
        let m = HookManifest::from_toml_str(&doc).unwrap();
        m.validate(Path::new("/tmp/h")).unwrap();
        let entry = &m.hooks[0];
        // Stored whole and unparsed, so a grim that predates whatever vocabulary
        // `policy` eventually carries still preserves it.
        assert_eq!(
            entry.policy.as_ref().unwrap(),
            &serde_json::json!({"mode": "warn", "limits": {"calls": 3}})
        );
        assert_eq!(entry.vendor["claude"], serde_json::json!({"timeout": 90}));
        assert_eq!(HookManifest::from_toml_str(&toml::to_string(&m).unwrap()).unwrap(), m);
    }

    #[test]
    fn a_toml_datetime_under_policy_does_not_round_trip() {
        // Not a wish — a recorded narrowing (§ 5.3 risk 2). The value model is
        // `serde_json::Value` because `HookManifest` must derive `JsonSchema`
        // and `toml::Value` does not, so TOML's four native temporal types are
        // unrepresentable: a date re-serializes as a nested table leaking
        // serde-toml's private sentinel, which is a STRUCTURAL corruption of a
        // published format. `grim build` owes a refusal here; until it lands,
        // this test is the record of what the format cannot carry, so nobody
        // "fixes" the round-trip test by re-emitting the sentinel.
        let doc = manifest(
            "id = \"a\"\nevent = \"Stop\"\ntier = \"observer\"\nargv = [\"sh\"]\npolicy = { since = 2026-08-14 }",
        );
        let m = HookManifest::from_toml_str(&doc).unwrap();
        let policy = m.hooks[0].policy.as_ref().unwrap();
        assert!(
            policy
                .get("since")
                .and_then(|v| v.as_object())
                .is_some_and(|o| o.contains_key("$__toml_private_datetime")),
            "the sentinel moved; re-check the narrowing before trusting it: {policy}"
        );
        // The corruption is in the emitted TEXT, not in the value: the sentinel
        // table is itself stable across serialize→parse, so a value-equality
        // round-trip test PASSES here and hides the defect. What a third-party
        // reader of `hook.toml` gets back is a nested table where the author
        // wrote a date.
        let reemitted = toml::to_string(&m).unwrap();
        assert!(
            reemitted.contains("$__toml_private_datetime"),
            "expected the leaked sentinel in the re-emitted document: {reemitted}"
        );
        assert!(
            !reemitted.contains("since = 2026-08-14"),
            "a datetime under `policy` now re-emits faithfully — delete this test and document the widening"
        );
        assert_eq!(
            HookManifest::from_toml_str(&reemitted).unwrap(),
            m,
            "value-level stability is exactly why the text-level check above is the one that matters"
        );
    }

    #[test]
    fn canonical_breadth_is_exactly_four_events() {
        assert_eq!(CanonicalEvent::ALL.len(), 4);
        assert_eq!(
            CanonicalEvent::ALL.map(CanonicalEvent::as_str),
            ["PreToolUse", "PostToolUse", "SessionStart", "Stop"]
        );
        // A fifth canonical variant is a design change, never a parse-time
        // accident: a native moment reaches its client through `<vendor>.event`.
        assert!(serde_json::from_str::<CanonicalEvent>("\"PermissionRequest\"").is_err());
        for tier in HookTier::ALL {
            assert_eq!(
                serde_json::from_str::<HookTier>(&format!("\"{}\"", tier.as_str())).unwrap(),
                tier
            );
        }
    }

    #[test]
    fn unsupported_kind_classifies_as_a_data_error() {
        // The stub-phase seam refusal: a registry-controlled kind string must
        // never reach a panic (exit 101, no JSON error document, I3 inverted).
        let err = anyhow::Error::from(unsupported_kind());
        assert_eq!(crate::error::classify_error(&err), crate::ExitCode::DataError);
    }
}
