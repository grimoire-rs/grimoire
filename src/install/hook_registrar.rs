// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Hook convergence: derive every registration and dispatch entry from install
//! state, arm what is wanted, reap what is not (Decision L, C-017).
//!
//! # Derive, never record
//!
//! A registration is a **projection of install state**, never a recorded
//! `ClientOutput`. That is deliberate and it is the structural fix issues #54 and
//! #55 both propose: `install_mcp`'s record-a-`ClientOutput`-per-registration
//! shape is what leaves an orphaned record naming a registration nothing can
//! find. So nothing here is written to `state.json`, and the algorithm follows
//! [`super::opencode_config::sync_for_state`]'s add-strict / remove-tolerant
//! discipline — grim never rewrites a user-owned config it cannot parse, but it
//! also never fails a command whose primary action already ran just because a
//! removal target was already gone.
//!
//! # Enumerate-and-reap is not optional plumbing
//!
//! The `opencode_config` precedent works only because its managed set has
//! cardinality ≤ 1 with a `const` member: it can always reconstruct the exact
//! element to remove. Hooks have **variable cardinality with members derived
//! from what is installed**, so after an uninstall the record naming a matcher
//! group is already gone from state and neither the group value nor the element
//! is reconstructible. The convergence loop must therefore call
//! [`super::json_splice::owned_nested_handlers`], compute the desired set, and
//! remove **`owned − desired`**. Without that explicit call the registration
//! stays armed forever in a file grim does not own —
//! `#[expect(dead_code)]` proves a function is *reachable*, never that it is
//! *consumed*.
//!
//! **Known limit, deferrable and additive** (WP-D re-validation N-3): that
//! enumeration reads per canonical-event `member`, so an entry written under an
//! event key *this* binary does not project — a future grim adds an event, or a
//! vendor's event-name projection changes — is invisible to it. Principle 9's
//! "enum literals are added, never removed" makes only one direction safe. The
//! fix, if it ever matters, is an additive variant enumerating every member of
//! `container`.
//!
//! # Two surfaces, two ownership models
//!
//! - **`HookSurface::SpliceConfig`** (claude, both scopes): grim splices one
//!   marked handler element into a config the *user* owns, and ownership is the
//!   `com.grimoire.managed` marker on the **element** (`HOOK_MARKER_KEY` /
//!   `HOOK_MARKER_VALUE` in [`super::vendor`]). The marker is re-asserted on
//!   **every** `grim install`, idempotently: it is unverified whether Claude
//!   preserves an unknown member when the *client itself* rewrites the `hooks`
//!   block, and until that is settled a client rewrite would silently orphan a
//!   registration grim can no longer recognise — D-1 by a third route.
//! - **`HookSurface::OwnFile`** (codex, copilot, global only): grim owns the
//!   whole file, so ownership is the *path* and reaping is regenerating or
//!   deleting it. No marker goes in either file — codex `deny_unknown_fields` at
//!   the top level and drops **every** hook in the file on one bad key.
//!
//! # A dispatch row *is* a registration (P-1)
//!
//! The dispatch table and the client's own config are two projections of **one**
//! [`Vendor::hook_registration`] verdict, computed once per `(client, hook)` in
//! [`register_desired`] and consumed by [`union_of`] and [`sync_for_state`].
//! They must never be derived independently: the wave-7 audit's P-1 is what
//! happens when they are — the table was built from the unfiltered desired set,
//! so a `HookDecline` removed the registration and left the row, and the runtime
//! selects rows by `(root, client, event)`, a key with no decline dimension. The
//! declined payload then ran for the declining client as soon as any *sibling*
//! entry armed at the same pair.
//!
//! The invariant that closes it, and the one to preserve: **a row exists for
//! `(client, hook)` if and only if that client's registration was written.** The
//! runtime's selection key is sufficient only while that holds.
//!
//! # `not-armed` is a reported state, never a silent skip (C-017)
//!
//! Five conditions refuse to arm. Four are in this fold; the fifth (W3 — a
//! group- or other-writable table or launcher) is deferred with W3. Each reports
//! `not-armed` naming the client and the hook: a refusal with no reported state
//! is exactly the silent-guardrail class C-017 and C-025 exist to prevent, and a
//! reported state with no refusal arms the thing B1 forbids. **The status token
//! and its message text are WP-H's; the refusal is this module's.**

use std::io;
use std::path::{Path, PathBuf};

use crate::config::ConfigScope;
use crate::hook::policy::HookPolicy;
use crate::install::path_anchor::{AnchorRoots, AnchoredPath, Containment, PathAnchor};
use crate::lock::locked_source::LockedSource;
use crate::oci::ArtifactKind;
use crate::oci::hook::{CanonicalEvent, HOOK_MANIFEST_FILE, HookEntry, HookManifest, HookSurface};
use crate::store::atomic_write::atomic_write;

use super::client_target::ClientTarget;
use super::hook_dispatch::{DispatchEntry, DispatchError, DispatchWrite, RootScope, RootToken};
use super::hook_launcher::CommandRefusal;
use super::install_state::InstallState;
use super::json_splice::{self, NestedGroupPath, NestedHandlerPath, Splice};
use super::vendor::{HOOK_MARKER_KEY, HOOK_MARKER_VALUE, Vendor};

/// The repo-resident file Claude writes its project-scope hook registration
/// into — the only armable thing grim writes inside a workspace, and the reason
/// invariant I1 tolerates it (the client treats it as per-developer local).
pub const CLAUDE_LOCAL_SETTINGS: &str = ".claude/settings.local.json";

/// The per-clone, never-committed ignore file grim appends to.
///
/// Not the repo's `.gitignore`: that is a tracked file, so editing it would put
/// a diff in front of every teammate for a purely local hygiene rule.
///
/// **The path grim writes is not this constant joined onto the workspace**, and
/// the difference is deliberate: `.git` is a *file* in a linked worktree or a
/// submodule, so the real target is `$GIT_DIR/info/exclude` as resolved by
/// `git_info_dir`. This constant is the plain-checkout spelling, for messages
/// and for `grim status` — never a join source.
pub const GIT_EXCLUDE_RELATIVE: &str = ".git/info/exclude";

/// Why `sync_config` refused to arm — C-017's `not-armed` causes 1–4.
///
/// A refusal is **not** an error: the exit code is unchanged, the tool call still
/// proceeds, and the fail-safe direction (I3) is preserved. What it must never
/// be is silent — hence one variant per cause, each with its own reason phrase,
/// so `grim status` can distinguish them and a user can act on the right one.
/// The plan is explicit that four causes sharing one message is a defect: a user
/// told only "not armed" cannot tell a relative `GRIM_HOME` from a concurrent
/// install and will retry the wrong thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmRefusal {
    /// **Cause 1 (B1 · T3 · I1, I4).** `grim_home()` is relative.
    ///
    /// [`crate::env::grim_home`] returns `$GRIM_HOME` verbatim with no
    /// absoluteness check, and falls back to a relative `.grimoire` when `HOME`
    /// is unset. The CWD of a `grim hook run` spawned by a client is the
    /// workspace, so a relative root makes the dispatch table — the arming
    /// authority — resolve *inside the repository*, where a committed file
    /// satisfies it. Executed against the shipped 0.13.0 binary.
    GrimHomeRelative,
    /// **Cause 2 (B1 · T3 · I1, I4).** `grim_home()` resolves inside the
    /// workspace being installed for.
    ///
    /// `subsystem-file-structure.md` records nesting as a state-record *caveat*;
    /// for hooks the same condition makes an **armable** file repo-resident,
    /// which I1 forbids outright, so it is a refusal rather than a caveat.
    GrimHomeInWorkspace,
    /// **Cause 3 (B2 · T3 · I1, I6).** The resolved launcher or table path
    /// carries a newline or another control character.
    LauncherPath(CommandRefusal),
    /// **Cause 4 (W1 · no attacker · I3).** Another `grim install` holds the
    /// dispatch lock.
    ///
    /// Reported rather than written over: without mutual exclusion two installs
    /// in two workspaces are last-writer-wins on the record set, and the loser's
    /// hooks are silently absent while `grim status` believes they are armed.
    DispatchLocked,
}

impl ArmRefusal {
    /// The user-facing reason phrase, library style (lowercase, no trailing
    /// punctuation) so it composes into the warning and into a `grim status`
    /// cell.
    ///
    /// Distinguishing text per cause; **WP-H owns the final wording** and the
    /// `not-armed` status token itself.
    pub fn reason(self) -> &'static str {
        match self {
            Self::GrimHomeRelative => {
                "GRIM_HOME is a relative path, so the dispatch table would resolve inside the workspace"
            }
            Self::GrimHomeInWorkspace => {
                "GRIM_HOME resolves inside this workspace, which would make an armable file repo-resident"
            }
            Self::LauncherPath(r) => r.reason(),
            Self::DispatchLocked => "another grim install holds the dispatch table lock",
        }
    }
}

impl std::fmt::Display for ArmRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

// **No `From` conversions into `ArmRefusal`, deliberately.**
//
// Two existed and both were dead: every call site matches `DispatchError`
// explicitly (`converge_clients`) or constructs `ArmRefusal::LauncherPath`
// directly. The `DispatchError` one was worse than unused — its own doc said "an
// I/O failure is deliberately **not** convertible", and its body mapped
// `Io(_) => DispatchLocked`, so had anything ever reached it through `?` a
// failing disk would have been reported to the user as another install holding
// the lock. A refusal is a *policy* verdict and an I/O error is not one, so there
// is no conversion worth writing; keeping the match explicit is what makes that
// impossible to get wrong by reflex.

/// What one client's hook convergence did — the value `sync_config` logs and
/// `grim status` reads.
///
/// A refusal is a **variant, not an `Err`**: `Vendor::sync_config` returns
/// `io::Result<()>` and every caller logs an `Err` as a warning, which would
/// flatten four distinguishable policy refusals into one opaque I/O line and
/// leave `grim status` nothing structured to report. `Err` stays for genuine I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookSync {
    /// No hook is recorded for this client at this scope, and nothing
    /// grim-owned was found to reap — the overwhelmingly common case, and it
    /// must cost no writes.
    NoHooks,
    /// The registrations and the dispatch entry already said exactly this.
    Unchanged,
    /// Armed: `n` dispatch entries and the client's registrations converged.
    Armed(usize),
    /// The last hook for this client at this scope went away; registrations and
    /// the root's dispatch entry were reaped.
    Disarmed,
    /// Refused to arm, with the cause (C-017). The primary command still
    /// succeeded; the exit code is unchanged.
    NotArmed(ArmRefusal),
}

/// Emit `outcome` at the level C-017 requires — the one place the three vendors'
/// `sync_config` bodies share.
///
/// A refusal is a **warning naming the client and the cause**, not a `debug!`
/// line: C-017 exists because today's convergence failure is invisible at the
/// default `warn` filter, and a refusal logged at `debug` would be a silent
/// guardrail with extra steps. Every other outcome is ordinary bookkeeping and
/// stays at `debug`, because it fires for every client on every command.
///
/// Naming the *hook* as well as the client is WP-H's, and it needs the status
/// surface to do it — causes 1–3 are properties of `$GRIM_HOME`, not of any one
/// hook, so there is no hook to name from here.
pub fn log_sync(client: &str, outcome: &HookSync) {
    match outcome {
        HookSync::NotArmed(refusal) => {
            tracing::warn!("hooks not armed for {client}: {}", refusal.reason());
        }
        other => tracing::debug!("{client} hook sync: {other:?}"),
    }
}

/// Converge every hook-capable client's registration surface **and** the
/// dispatch table on `state`, for one command.
///
/// The command-level driver, called once per mutating command with the policy
/// that command resolved. Returns one outcome per client, already logged, so a
/// caller is a single statement.
///
/// # Why this exists, and why `sync_for_state` is not called from `Vendor::sync_config`
///
/// Two reasons, both structural, and the second is a **correction to
/// `sync_for_state`'s own documented step order** (recorded rather than quietly
/// fixed):
///
/// 1. **`sync_config(state, workspace, scope)` cannot see the policy.** The
///    feature flag and per-registry trust are config facts, and the desired-set
///    projection takes them as a predicate. Nothing about that seam can supply
///    one, which is why nothing armed for four waves.
/// 2. **The dispatch table is written ONCE per command, not once per client.**
///    [`super::hook_dispatch::converge_root`] replaces a root's `hooks` vector
///    **wholesale**, while [`desired_entries`] is **per vendor**. Calling the
///    first with the second's output, once per client, makes the last client's
///    write erase every earlier client's rows — so `sync_for_state`'s step 4
///    ("write the dispatch entry") belongs *here*, above the per-client loop,
///    with the **union** over every hook-capable client. The per-client `client`
///    field on each row (F-1) is what makes that union unambiguous; it is not by
///    itself sufficient.
///
/// # The step order inverted for P-1, and what that cost
///
/// The registration verdict has to be known **before** the union is built (see
/// the module doc), and [`Vendor::hook_registration`] takes the launcher, the
/// table and the root token — the three values step 4 used to derive *after* the
/// desired sets. So the order is now:
///
/// 1. refuse early ([`arming_refusal`]) — unchanged, and still ahead of every
///    write;
/// 2. resolve the launcher path, the table path and the root token;
/// 3. project each client's desired set and run [`register_desired`] over it;
/// 4. generate the launcher (only if something registered), then write the union
///    of the **registered** rows.
///
/// One write moved with it: [`super::hook_dispatch::root_token`] mints the
/// machine key on first use, and it now does so ahead of the launcher write
/// rather than after it. Nothing gained a write it did not already have — the
/// token was already derived unconditionally past `arming_refusal`, including on
/// a pure reap, which needs it to name the root it is emptying.
///
/// The alternative — computing the verdict twice, once with probe paths for the
/// filter and once with the real ones for the surface — was rejected: the
/// refusal order provably does not read those three values
/// ([`super::client_target`]'s `HOOK_CELL_PROBE_LAUNCHER` pins that), so the two
/// calls would agree *today*, and a contract that depends on two calls agreeing
/// is the shape P-1 already was.
///
/// # The client set is derived, never passed in
///
/// Every client with a hook surface **at this scope**, not the command's
/// `--client` selection and not the record's involved clients. Both of those
/// would be wrong in the same direction: a hook armed for claude and codex, on a
/// command that happens to touch only claude, would have codex's rows dropped
/// from the union and its registration left stranded. Convergence is a function
/// of `state` alone, so deriving the set makes every command idempotent and
/// self-healing regardless of what it was asked to do.
pub fn converge_clients(
    state: &InstallState,
    workspace: &Path,
    scope: ConfigScope,
    roots: &AnchorRoots,
    policy: &HookPolicy,
) -> Vec<(ClientTarget, HookSync)> {
    let clients = hook_clients(scope);
    if clients.is_empty() {
        return Vec::new();
    }
    let grim_home = roots.grim_home.as_path();

    // The whole-command no-op fast path: no hook is recorded, no client owns a
    // registration on disk, and there is no dispatch table to reap from. It must
    // cost no writes and almost no reads — this runs on every install, update
    // and uninstall, for every user, whether or not they have ever seen a hook.
    //
    // The table probe is the file's existence, not its contents: an
    // over-approximation in the safe direction (one extra read of a small JSON
    // file when some *other* workspace has hooks) where the alternative — never
    // probing — would strand a root entry whose `state.json` was deleted.
    let surfaces: Vec<(ClientTarget, Option<PathBuf>)> = clients
        .iter()
        .map(|c| (*c, c.vendor().hook_config_path(workspace, scope)))
        .collect();
    let anything_local = surfaces.iter().any(|(client, surface)| {
        has_hook_record(client.vendor(), state) || owns_anything(client.vendor(), surface.as_deref())
    });
    if !anything_local && !super::hook_dispatch::dispatch_path(grim_home).is_file() {
        return Vec::new();
    }

    // Step 1 — refuse early, once. Causes 1–3 are properties of `$GRIM_HOME`
    // and the workspace, identical for every client, and a refusal must write
    // nothing at all: not the launcher, not the table, not a registration.
    if let Some(refusal) = arming_refusal(grim_home, workspace, scope) {
        return clients
            .into_iter()
            .map(|client| {
                let outcome = HookSync::NotArmed(refusal);
                log_sync(client.vendor().name(), &outcome);
                (client, outcome)
            })
            .collect();
    }

    // Step 3 — the desired set, per client, filtered structurally by the policy.
    // An ungated or untrusted hook simply is not in the set, so "off" and
    // "untrusted" reach the runtime as an absent entry rather than as a runtime
    // check the runtime is forbidden to make.
    let trust = |source: &LockedSource| policy.arms(source);
    // Derived once, above the per-client loop, and used for BOTH the payload
    // location every client reads its manifest from and the dispatch-table key
    // written in step 4 — so the two cannot disagree about which root is armed.
    let root = root_scope_for(workspace, scope);
    let table = super::hook_dispatch::dispatch_path(grim_home);
    let launcher = super::hook_launcher::launcher_path(grim_home);

    // The three paths a registration embeds, resolved BEFORE the desired sets —
    // because the registration verdict is what filters them (P-1). `dispatch_path`
    // and `launcher_path` are pure; `root_token` mints the machine key on first
    // use, so this is the one write that moved: it now happens ahead of the
    // launcher write rather than after it. Both were already unconditional past
    // `arming_refusal` — a pure reap derives the token too, because it has to
    // name the root it is emptying — so no path gained a write it did not have.
    // `arming_refusal` above still precedes every one of them, which is the
    // property that matters: a refusal writes nothing at all.
    let token = match super::hook_dispatch::root_token(grim_home, root) {
        Ok(token) => token,
        Err(e) => {
            tracing::warn!(error = %e, "hook root token could not be derived; nothing armed");
            return clients.into_iter().map(|client| (client, HookSync::NoHooks)).collect();
        }
    };
    let armed_paths = ArmedPaths {
        launcher: &launcher,
        table: &table,
        token: &token,
    };

    let mut per_client: Vec<ArmedClient> = Vec::new();
    let mut outcomes: Vec<(ClientTarget, HookSync)> = Vec::new();
    for (client, surface) in surfaces {
        match desired_entries(client.vendor(), state, roots, root, &trust) {
            Ok(desired) => per_client.push(register_desired(client, surface, &desired, &armed_paths)),
            Err(e) => {
                // A payload directory grim cannot read, or a `hook.toml` that no
                // longer parses. Warn-only, exactly as a `sync_config` failure
                // has always been: the artifacts and state are already on disk,
                // and blocking here would trade the user's agent for a
                // convergence detail (I3).
                tracing::warn!(
                    client = client.vendor().name(),
                    error = %e,
                    "hook desired-set projection failed; registration skipped"
                );
            }
        }
    }

    let union = union_of(&per_client);

    // Step 2 — generate the launcher, idempotently, and only when something is
    // actually being armed. A pure reap must not create the shim it is about to
    // orphan.
    if !union.is_empty()
        && let Err(e) = generate_launcher(grim_home)
    {
        tracing::warn!(error = %e, "hook launcher could not be written; nothing armed");
        return clients.into_iter().map(|client| (client, HookSync::NoHooks)).collect();
    }

    // Step 4 — write the dispatch entry wholesale under the lock, once, with the
    // union. Deliberately BEFORE the registrations: a registration whose table
    // entry is missing degrades to no-match ⇒ exit 0, whereas a table entry with
    // no registration is inert. Both orders are safe; this one leaves no window
    // in which a client can invoke a launcher that finds nothing to do.
    let write = match super::hook_dispatch::converge_root(grim_home, &token, root, &union) {
        Ok(write) => write,
        Err(DispatchError::Locked) => {
            // Cause 4. Reported rather than written over — the loser of a race
            // must not believe it armed.
            return clients
                .into_iter()
                .map(|client| {
                    let outcome = HookSync::NotArmed(ArmRefusal::DispatchLocked);
                    log_sync(client.vendor().name(), &outcome);
                    (client, outcome)
                })
                .collect();
        }
        Err(e @ DispatchError::Io(_)) => {
            tracing::warn!(error = %e, "dispatch table write failed; nothing armed");
            return clients.into_iter().map(|client| (client, HookSync::NoHooks)).collect();
        }
    };

    // Steps 5 and 6 — converge each client's own surface, then the git-exclude
    // hygiene that is never a gate.
    for armed in per_client {
        let client = armed.client;
        let outcome = match sync_for_state(
            client.vendor(),
            workspace,
            scope,
            armed.surface.as_deref(),
            &armed.registrations,
            write,
        ) {
            Ok(outcome) => outcome,
            Err(e) => {
                tracing::warn!(
                    client = client.vendor().name(),
                    error = %e,
                    "hook registration sync failed; the dispatch table is written, this client is not registered"
                );
                HookSync::NoHooks
            }
        };
        log_sync(client.vendor().name(), &outcome);
        outcomes.push((client, outcome));
    }
    outcomes
}

/// Every client with a hook surface at `scope`, in [`ClientTarget::ALL`] order.
///
/// The capability half comes from [`Vendor::declines_hooks_everywhere`] — the
/// one definition of "does this client host hooks at all", shared with
/// `installer::client_supports_kind`'s `Hook` arm and `path_anchor`'s
/// `is_declined_global_pair` — and the scope half from `kind_surface`. Neither
/// is `kind_support`, which defaults to `Native` for every vendor and so answers
/// `true` for the 15 clients with no hook mechanism at all (D-1's failure mode).
///
/// This function cannot call `client_supports_kind` directly: it takes no
/// workspace, and `hook_registrar` does not otherwise depend on `installer`, so
/// the call would buy agreement with a mutual module dependency. Sharing the
/// capability predicate buys the same agreement for nothing.
fn hook_clients(scope: ConfigScope) -> Vec<ClientTarget> {
    ClientTarget::ALL
        .into_iter()
        .filter(|c| !c.vendor().declines_hooks_everywhere() && c.vendor().kind_surface(ArtifactKind::Hook, scope))
        .collect()
}

/// The union of every client's **registered** rows, deterministically ordered.
///
/// # ⛔ The input is [`ArmedClient::rows`], never a desired set (P-1)
///
/// This function used to take the desired sets and never learned which of them
/// [`Vendor::hook_registration`] went on to decline, so a declined hook still
/// got a row — and the runtime selects rows by `(root, client, event)`, a key
/// with no decline dimension. The decline therefore held only while no *sibling*
/// entry registered at the same `(client, event)`: one artifact declaring two
/// entries put the declined payload back on the hot path, most sharply for
/// `HookDecline::MutatorOnShellCommandTool` (ADR decision K), which is the one
/// refusal that exists because the client displays the un-mutated command while
/// executing the mutated one.
///
/// [`register_desired`] now runs the verdict **once** and feeds both consumers
/// from it — this union and the client's own surface — so the table and the
/// registration cannot disagree about what is armed. The row's presence *is* the
/// registration: a client the registration was declined for contributes no row,
/// which is what makes `run.rs`'s selection key sufficient.
///
/// Sorted on `(artifact, id, event, client)` so a re-write with no change is
/// byte-identical and `converge_root` can answer `Unchanged` — Principle 9's
/// self-heal obligation. `client` is the last key, and it has to be *a* key:
/// without it two clients' rows for one hook compare equal on the first three
/// and the sort is unstable across runs.
fn union_of(per_client: &[ArmedClient]) -> Vec<DispatchEntry> {
    let mut union: Vec<DispatchEntry> = per_client.iter().flat_map(|armed| armed.rows.iter().cloned()).collect();
    union.sort_by(|a, b| {
        (&a.artifact, &a.id, a.event.as_str(), &a.client).cmp(&(&b.artifact, &b.id, b.event.as_str(), &b.client))
    });
    union
}

/// Write the launcher shim, resolving the running binary's absolute path.
///
/// `std::env::current_exe` is read here rather than inside
/// [`super::hook_launcher::generate`] so that generator stays a pure function of
/// its arguments and hermetically testable — its own doc requires exactly that.
/// A failure to resolve it is a genuine I/O error, not a policy refusal: there
/// is no [`ArmRefusal`] cause for "grim does not know where it lives", and
/// inventing one would report an environment failure as a consent decision.
fn generate_launcher(grim_home: &Path) -> io::Result<()> {
    let binary = std::env::current_exe()?;
    super::hook_launcher::generate(grim_home, &binary).map_err(|e| match e {
        super::hook_launcher::LauncherError::Io(io) => io,
        refused @ super::hook_launcher::LauncherError::Refused(_) => io::Error::other(refused.to_string()),
    })?;
    Ok(())
}

/// The three paths every registration embeds, resolved once per command.
///
/// Passed as one struct rather than three arguments because they are only ever
/// used together, and because a caller that got the launcher from one
/// `$GRIM_HOME` and the table from another would write a registration that can
/// never match.
struct ArmedPaths<'a> {
    launcher: &'a Path,
    table: &'a Path,
    token: &'a RootToken,
}

/// One hook the desired set wants armed: the dispatch row **and** the manifest
/// entry it was projected from.
///
/// Both halves are needed and neither substitutes for the other. The row goes in
/// the dispatch table; the manifest entry goes to
/// [`Vendor::hook_registration`], the single assembly site, which takes a
/// [`HookEntry`]. Carrying the pair from one read of one `hook.toml` is what
/// keeps the table and the registration derived from the same bytes — re-reading
/// the manifest for the second half would admit a window in which the two
/// disagree.
#[derive(Debug, Clone)]
struct DesiredHook {
    /// The dispatch-table row.
    entry: DispatchEntry,
    /// The manifest entry, for the registration assembly site.
    manifest: HookEntry,
    /// The canonical event, which `HookRegistration` keeps only in the client's
    /// own spelling.
    event: CanonicalEvent,
}

/// One client's set **after** the registration verdict: what its surface will
/// carry, and the dispatch rows those registrations authorize.
///
/// The two vectors are parallel by construction — [`register_desired`] pushes to
/// both or to neither — and that is the whole invariant P-1 was the absence of.
/// A declined hook is in neither, so it is neither registered nor dispatchable.
struct ArmedClient {
    client: ClientTarget,
    /// Where this client's registrations live, or `None` when it hosts none at
    /// this scope.
    surface: Option<PathBuf>,
    /// What [`Vendor::hook_registration`] accepted.
    registrations: Vec<crate::oci::hook::HookRegistration>,
    /// The dispatch rows of exactly those accepted registrations.
    rows: Vec<DispatchEntry>,
}

/// Run the registration verdict over one client's desired set, keeping only
/// what registered.
///
/// **The single call to [`Vendor::hook_registration`] on the convergence path,
/// with two consumers** (P-1). A decline is warn-and-skip, never a failure: a
/// client that cannot host this hook at this event, cannot honour its tier, or
/// cannot express its matcher losslessly is a reported outcome rather than a
/// broken command. What changed is that the skip is now *total* — the entry
/// reaches neither the client's config nor the dispatch table, so no sibling
/// entry registering at the same `(client, event)` can put it back on the
/// runtime's selection key.
///
/// # ⛔ The verdict is read here and nowhere else
///
/// Do not re-derive it. A second spelling of the refusal order
/// (surface → surface shape → event → tier → decision K → matcher) is exactly
/// the drift that produced P-1: `run::client_admits`' doc claimed a property the
/// table could not carry, because the table was built from a set the verdict had
/// never touched.
fn register_desired(
    client: ClientTarget,
    surface: Option<PathBuf>,
    desired: &[DesiredHook],
    armed: &ArmedPaths<'_>,
) -> ArmedClient {
    let vendor = client.vendor();
    let mut registrations = Vec::with_capacity(desired.len());
    let mut rows = Vec::with_capacity(desired.len());
    for hook in desired {
        match vendor.hook_registration(&hook.manifest, hook.event, armed.launcher, armed.table, armed.token) {
            Ok(registration) => {
                registrations.push(registration);
                rows.push(hook.entry.clone());
            }
            Err(decline) => tracing::warn!(
                "hook '{}/{}' not registered for {}: {}; it gets no dispatch entry there either, so nothing runs \
                 for that client",
                hook.entry.artifact,
                hook.entry.id,
                vendor.name(),
                decline.reason()
            ),
        }
    }
    ArmedClient {
        client,
        surface,
        registrations,
        rows,
    }
}

/// Converge one client's own hook surface on `registrations`.
///
/// Steps 5 and 6 of the contract; steps 1–4 belong to
/// [`converge_clients`], which performs them once for the whole command (see
/// its doc for why step 4 could not stay here).
///
/// **The assembly moved out** (P-1). This function used to call
/// [`Vendor::hook_registration`] itself, which is why the dispatch table — built
/// one step earlier, from the unfiltered desired set — never learned what it
/// declined. [`register_desired`] now runs the verdict once, above the table
/// write, and this function is handed the result: it writes exactly the
/// registrations that also have a dispatch row, and nothing else.
///
/// Two surfaces, two ownership models:
///
/// - [`HookSurface::SpliceConfig`] — grim splices one marked handler element per
///   registration into a config the *user* owns, then **enumerates and reaps
///   `owned − desired`**. Without that explicit reap the registration stays
///   armed forever in a file grim does not own (D-1).
/// - [`HookSurface::OwnFile`] — grim owns the whole file: the desired set is
///   rendered wholesale, and an empty set removes the file.
///
/// # Errors
///
/// A genuine I/O failure reading or writing the client's config. A policy
/// refusal is not an error — it is [`HookSync::NotArmed`], and it is decided by
/// the caller.
fn sync_for_state(
    vendor: &dyn Vendor,
    workspace: &Path,
    scope: ConfigScope,
    surface: Option<&Path>,
    registrations: &[crate::oci::hook::HookRegistration],
    table_write: DispatchWrite,
) -> io::Result<HookSync> {
    // `None` ⇒ this client has no hook surface at this scope, so there is
    // nothing to write and nothing to own (codex and copilot at project scope).
    let Some(surface) = surface else {
        return Ok(HookSync::NoHooks);
    };

    let surface_write = match vendor.hook_surface() {
        Some(HookSurface::OwnFile) => converge_own_file(vendor, surface, registrations)?,
        Some(HookSurface::SpliceConfig) => converge_splice(vendor, surface, registrations)?,
        // `CodegenModule` ships no template in v1 and `None` has no surface at
        // all. A decline plus a warning, never a panic (I3).
        Some(HookSurface::CodegenModule) | None => {
            tracing::warn!("{} hook surface shape is not implemented", vendor.name());
            SurfaceWrite::Unchanged
        }
    };

    // Step 6 — best-effort git-exclude hygiene, and it is NEVER a gate. A git
    // ignore rule governs `git add` and `git status`, not reads, so this stops
    // the *user* publishing their own arming; T3 is held by the absolute
    // launcher path, the ownership marker and digest-pinned approval.
    if scope == ConfigScope::Project && vendor.hook_surface() == Some(HookSurface::SpliceConfig) {
        let outcome = if registrations.is_empty() {
            drop_settings_local_exclude(workspace)
        } else {
            ensure_settings_local_excluded(workspace)
        };
        match outcome {
            ExcludeOutcome::Added
            | ExcludeOutcome::AlreadyPresent
            | ExcludeOutcome::Removed
            | ExcludeOutcome::Absent => {
                tracing::debug!("{} {GIT_EXCLUDE_RELATIVE} hygiene: {outcome:?}", vendor.name());
            }
            // The one outcome worth surfacing: an ignore rule is inert against a
            // tracked file, so the user's own arming *will* show up in
            // `git status`. Arms anyway.
            ExcludeOutcome::AlreadyTracked => tracing::warn!(
                "{CLAUDE_LOCAL_SETTINGS} is tracked in this repository, so no {GIT_EXCLUDE_RELATIVE} rule can hide \
                 grim's hook registration from `git status`"
            ),
            ExcludeOutcome::NotAGitWorktree | ExcludeOutcome::Unwritable => {
                tracing::debug!("{GIT_EXCLUDE_RELATIVE} not updated ({outcome:?}); arming anyway");
            }
        }
    }

    Ok(match (registrations.is_empty(), surface_write, table_write) {
        // Nothing wanted and nothing was owned: a genuine no-op.
        (true, SurfaceWrite::Unchanged, _) => HookSync::NoHooks,
        // Nothing wanted and something went away.
        (true, SurfaceWrite::Changed, _) => HookSync::Disarmed,
        // Something wanted, and neither the registration nor the table moved.
        (false, SurfaceWrite::Unchanged, DispatchWrite::Unchanged) => HookSync::Unchanged,
        (false, _, _) => HookSync::Armed(registrations.len()),
    })
}

/// Whether converging a client's surface changed any bytes.
///
/// Narrower than [`DispatchWrite`] on purpose: the registrar only needs to tell
/// "this client moved" from "it already said this", and a third variant would
/// have to be mapped onto a [`HookSync`] that has no room for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceWrite {
    /// The surface already said exactly this — no bytes written.
    Unchanged,
    /// Written, rewritten, or removed.
    Changed,
}

/// Converge a [`HookSurface::OwnFile`] client: render `registrations` wholesale,
/// or remove the file when there are none.
///
/// Ownership is the **path**, so there is no marker to enumerate and no user
/// bytes to preserve — which is exactly why codex's `deny_unknown_fields` top
/// level is safe here and would not be if grim spliced.
///
/// # Errors
///
/// Any I/O failure creating the parent directory, writing the file, or removing
/// it. A file that is already absent is not a failure.
fn converge_own_file(
    vendor: &dyn Vendor,
    surface: &Path,
    registrations: &[crate::oci::hook::HookRegistration],
) -> io::Result<SurfaceWrite> {
    if registrations.is_empty() {
        return match std::fs::remove_file(surface) {
            Ok(()) => Ok(SurfaceWrite::Changed),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(SurfaceWrite::Unchanged),
            Err(e) => Err(e),
        };
    }
    let Some(document) = vendor.hook_file_document(registrations) else {
        // An `OwnFile` vendor that renders no document is a programming error,
        // not a user-visible state; refuse rather than write an empty file over
        // a working registration.
        return Err(io::Error::other(format!(
            "{} declares an own-file hook surface but rendered no document",
            vendor.name()
        )));
    };
    let mut bytes = serde_json::to_vec_pretty(&document).map_err(io::Error::other)?;
    bytes.push(b'\n');
    // Byte-identical rewrites are skipped so re-materialization leaves `status`
    // not-modified (Principle 9's self-heal obligation).
    if std::fs::read(surface).is_ok_and(|existing| existing == bytes) {
        return Ok(SurfaceWrite::Unchanged);
    }
    if let Some(parent) = surface.parent() {
        std::fs::create_dir_all(parent)?;
    }
    atomic_write(surface, &bytes)?;
    Ok(SurfaceWrite::Changed)
}

/// Converge a [`HookSurface::SpliceConfig`] client: upsert every desired
/// element into the user's config, then remove `owned − desired`.
///
/// The reap is not optional plumbing. After an uninstall the record naming a
/// matcher group has already left state, so neither the group value nor the
/// element is reconstructible — only
/// [`json_splice::owned_nested_handlers`] can name what grim still owns.
///
/// Add-strict, remove-tolerant, following `opencode_config`'s discipline: grim
/// never rewrites a user-owned config it cannot parse, and it never fails a
/// command whose primary action already ran just because a removal target was
/// already gone.
///
/// # Errors
///
/// An I/O failure reading or writing the config, or an unparsable config when
/// something is being **added** — the add-strict half. A reap over an unparsable
/// config is tolerated (the enumeration returns nothing) rather than failing an
/// uninstall.
fn converge_splice(
    vendor: &dyn Vendor,
    surface: &Path,
    registrations: &[crate::oci::hook::HookRegistration],
) -> io::Result<SurfaceWrite> {
    let Some(shape) = vendor.hook_splice_shape() else {
        return Err(io::Error::other(format!(
            "{} declares a splice hook surface but no splice shape",
            vendor.name()
        )));
    };
    let original = match std::fs::read_to_string(surface) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut text = original.clone();

    // Add first, so a config grim cannot parse fails the command that is trying
    // to arm rather than silently reaping the elements it can no longer see.
    let owner = [(
        HOOK_MARKER_KEY,
        serde_json::Value::String(HOOK_MARKER_VALUE.to_string()),
    )];
    let owner_ref: Vec<(&str, &serde_json::Value)> = owner.iter().map(|(k, v)| (*k, v)).collect();
    let mut wanted: Vec<(String, serde_json::Value)> = Vec::new();
    for registration in registrations {
        let Some(handler) = vendor.hook_spliced_handler(registration) else {
            continue;
        };
        let path = NestedHandlerPath {
            group: NestedGroupPath {
                container: handler.shape.container,
                member: &handler.member,
                group_key: handler.shape.group_key,
                elements_key: handler.shape.elements_key,
            },
            group_value: &handler.group_value,
            identity_keys: &[HOOK_MARKER_KEY],
        };
        if let Splice::Changed(next) = json_splice::upsert_nested_handler(&text, &path, &handler.element)? {
            text = next;
        }
        wanted.push((handler.member.clone(), handler.element.clone()));
    }

    // Then reap `owned − desired`, over EVERY event member this client can
    // spell — not only the members the desired set names. An element left under
    // an event that dropped out of the manifest is exactly the stranded
    // registration this pass exists to remove.
    for event in CanonicalEvent::ALL {
        let Some(member) = vendor.hook_event_name(event) else {
            continue;
        };
        let group = NestedGroupPath {
            container: shape.container,
            member,
            group_key: shape.group_key,
            elements_key: shape.elements_key,
        };
        for (group_value, element) in json_splice::owned_nested_handlers(&text, &group, &owner_ref) {
            let keep = wanted
                .iter()
                .any(|(wanted_member, wanted_element)| wanted_member == member && *wanted_element == element);
            if keep {
                continue;
            }
            let path = NestedHandlerPath {
                group,
                group_value: &group_value,
                identity_keys: &[HOOK_MARKER_KEY],
            };
            match json_splice::remove_nested_handler(&text, &path, &element) {
                Ok(Splice::Changed(next)) => text = next,
                Ok(Splice::Unchanged) => {}
                // Remove-tolerant: a removal target that is already gone, or a
                // shape the primitive cannot address, must not fail the command
                // whose primary action already ran.
                Err(e) => tracing::debug!(error = %e, "hook registration reap skipped one element"),
            }
        }
    }

    if text == original {
        return Ok(SurfaceWrite::Unchanged);
    }
    if let Some(parent) = surface.parent() {
        std::fs::create_dir_all(parent)?;
    }
    atomic_write(surface, text.as_bytes())?;
    Ok(SurfaceWrite::Changed)
}

/// Whether a grim-owned registration is present on `surface` — the second half
/// of [`sync_for_state`]'s no-op guard.
///
/// **A deliberate over-approximation, and the direction matters.** For an
/// [`HookSurface::OwnFile`] client grim owns the path, so the file's existence
/// *is* the answer. For [`HookSurface::SpliceConfig`] the precise answer is
/// [`super::json_splice::owned_nested_handlers`], which **is** implemented and is
/// called in production a few lines below — but it answers a different question
/// (see the section at the end of this doc), so ownership is probed here as "the
/// marker value appears in the file's bytes". That can say `true` where no marked element exists (the string in a
/// comment, or inside an unrelated value), and it can **never** say `false` when
/// one does, because a marked element cannot exist without its marker in the
/// bytes. A false positive costs one convergence pass that finds nothing to
/// reap; a false negative would strand an armed registration.
///
/// # Do NOT replace this with the enumeration — they answer different questions
///
/// An earlier revision of this doc said to swap in
/// [`owned_nested_handlers`](super::json_splice::owned_nested_handlers) once it
/// landed, "the guard's contract does not change, only its precision". **Both
/// halves of that were wrong**, and the WP-D author established why:
///
/// - `owns_anything` asks *"could anything grim-owned be here?"* — the **skip**
///   decision, where an over-approximation is the correct shape.
/// - `owned_nested_handlers` asks *"what exactly do I own, so I can reap what is
///   no longer wanted?"* — the **reap** decision, taken inside the convergence
///   body, where exactness is the correct shape.
///
/// Swapping would *lower* precision for the skip decision, because the
/// enumeration has three false-negative classes this probe covers:
///
/// 1. **Unparsable text ⇒ owns nothing.** The enumeration returns an empty `Vec`
///    when the value fails to parse. So a config the **user** broke while grim's
///    marked element is still inside it would read as `NoHooks` and be skipped
///    **silently**; the probe never parses, so convergence runs, the add-strict
///    splice refuses, and the user is warned. This is the contract change: the
///    same state flips from reported to silent.
/// 2. **Per-event blindness.** The enumeration reads one member per canonical
///    event, so an element under an event key *this* binary does not project — a
///    future grim adds one, or a vendor's projection moves — is invisible to it.
///    The substring probe is what covers that gap.
/// 3. **A non-string group key is skipped** by the enumeration (such a group is
///    unaddressable through a `&str` group value). The probe still sees it.
///
/// Cost points the same way: this guard runs per client on every install,
/// update, uninstall and TUI action, and must stay nearly free. A substring scan
/// is memchr-class; the enumeration is a full parse plus a JSONC-sanitize
/// fallback over Claude's monolithic `~/.claude.json`.
///
/// If the false-positive rate ever measures, narrow the **substring** further —
/// never narrow it into a parse.
fn owns_anything(vendor: &dyn Vendor, surface: Option<&Path>) -> bool {
    // `None` ⇒ this client has no hook surface at this scope, so it can own
    // nothing here (codex and copilot at project scope, A1).
    let Some(surface) = surface else {
        return false;
    };
    match vendor.hook_surface() {
        Some(HookSurface::OwnFile) => surface.is_file(),
        Some(HookSurface::SpliceConfig) => {
            // Both marker strings, not just the value: a marked element cannot
            // exist without *both* in the bytes, so requiring both cannot
            // introduce a false negative — while it does kill the accidental
            // positive where the value alone appears in a comment or an
            // unrelated string.
            std::fs::read_to_string(surface)
                .is_ok_and(|text| text.contains(HOOK_MARKER_KEY) && text.contains(HOOK_MARKER_VALUE))
        }
        // `CodegenModule` ships no template in v1 and `None` has no surface at
        // all; neither can hold a grim-owned registration.
        Some(HookSurface::CodegenModule) | None => false,
    }
}

/// Whether `state` records a hook artifact with an output for this client.
///
/// The `want` computation of [`super::opencode_config::sync_for_state`], one
/// kind over: the state file is already scope-specific, so no scope filter is
/// needed here.
fn has_hook_record(vendor: &dyn Vendor, state: &InstallState) -> bool {
    state.iter_records().any(|record| {
        record.kind == ArtifactKind::Hook && record.outputs.iter().any(|output| output.client == vendor.name())
    })
}

/// The refusal `grim status` would report for this scope **without writing
/// anything** — the read-only half of C-017, for WP-H.
///
/// Causes 1–3 are pure functions of `$GRIM_HOME`, the workspace and the resolved
/// paths, so they re-derive identically at status time and need no record —
/// which is what makes them reportable at all under Decision L, where nothing
/// about a registration is ever recorded.
///
/// **Cause 4 is deliberately absent, and that is a gap worth naming.**
/// `DispatchLocked` is observable only at write time and is transient, so an
/// install that reported `not-armed` for a held lock will read as *armed* at the
/// next `grim status`. Nothing can close that without recording the failure,
/// which Decision L forbids; the honest surface is the install-time warning plus
/// the fact that the next `grim install` converges. WP-H must not present the
/// status answer as authoritative for cause 4.
pub fn arming_refusal(grim_home: &Path, workspace: &Path, scope: ConfigScope) -> Option<ArmRefusal> {
    if let Err(refusal) = validate_grim_home(grim_home, workspace, scope) {
        return Some(refusal);
    }
    // Cause 3, over both paths the registration embeds. Deriving them here
    // rather than taking them as arguments is what keeps this re-derivable: a
    // status caller has `$GRIM_HOME` and nothing else.
    for path in [
        super::hook_launcher::launcher_path(grim_home),
        super::hook_dispatch::dispatch_path(grim_home),
    ] {
        if let Err(refusal) = super::hook_launcher::path_is_representable(&path) {
            return Some(ArmRefusal::LauncherPath(refusal));
        }
    }
    None
}

/// C-017 causes 1 and 2: `grim_home` must be absolute and must not resolve
/// inside `workspace`.
///
/// Both are B1. The check is deliberately on the *resolved* path — a
/// `GRIM_HOME` of `.grimoire`, or one reached through a symlink into the
/// workspace, is the same defect as a literally nested one.
///
/// # Errors
///
/// [`ArmRefusal::GrimHomeRelative`] or [`ArmRefusal::GrimHomeInWorkspace`].
pub fn validate_grim_home(grim_home: &Path, workspace: &Path, scope: ConfigScope) -> Result<(), ArmRefusal> {
    if !grim_home.is_absolute() {
        return Err(ArmRefusal::GrimHomeRelative);
    }
    // Cause 2 asks whether `$GRIM_HOME` is nested inside the **repository**
    // being installed for. At global scope there is no such repository:
    // `scope_resolution` sets `workspace = paths.root()`, i.e. `$GRIM_HOME`
    // itself, as a placeholder. Comparing a path to itself always matches, so
    // before this carve-out **`grim install --global` could never arm anything**
    // — the payload materialized and every client reported
    // `grim-home-in-workspace`, which is the "installed but does nothing" shape
    // the whole `not-armed` vocabulary exists to avoid. Global is also the only
    // scope Codex and Copilot arm at all, so it disabled two of the three v1
    // clients outright. Found by WP-O with executed evidence.
    //
    // The carve-out is an **equality** test rather than `scope`-gating, so the
    // check still fires if a caller ever passes a real workspace at global
    // scope. `scope` stays in the signature for exactly that reason and is
    // asserted below.
    //
    // ⛔ **What this deliberately does NOT cover**, so nobody reads the fix as
    // wider than it is. The check is `grim_home.starts_with(workspace)`, i.e.
    // **workspace-relative**, so it misses two shapes and not one:
    //
    // 1. at **global** scope, a `$GRIM_HOME` that genuinely sits inside some
    //    repository — global scope never learns a project root, so there is no
    //    repository to compare against;
    // 2. at **project** scope, a `$GRIM_HOME` inside a *different* repository
    //    than the one being installed for (`GRIM_HOME=/repos/a/.grim`,
    //    `grim install` in `/repos/b`) — the comparison is against *this*
    //    workspace, and /repos/a is not it.
    //
    // The second was missing from this comment until the wave-7 audit read it
    // (P-7); project scope was never covered either, and claiming it was is the
    // kind of comment that makes a later reader stop looking.
    // Both are real I1 concerns and neither is **answerable from this call site**.
    // Answering it needs a separate check (an ancestor walk for a repository
    // marker), which is a behaviour change rather than a bug fix. Tracked
    // separately; do not "restore" the self-comparison to paper over it, because
    // that trades a real gap for a total outage.
    let nested = resolved(grim_home).starts_with(resolved(workspace));
    let placeholder_workspace = scope == ConfigScope::Global && resolved(workspace) == resolved(grim_home);
    if nested && !placeholder_workspace {
        return Err(ArmRefusal::GrimHomeInWorkspace);
    }
    Ok(())
}

/// `path` canonicalized, or `path` verbatim when it cannot be.
///
/// The containment check has to be on the *resolved* path — a `$GRIM_HOME`
/// reached through a symlink into the workspace is the same defect as a literally
/// nested one — but a path that does not exist yet (a first install) cannot be
/// canonicalized, and refusing to arm because of that would be a fail-closed
/// answer to an ordinary situation (I3). Falling back to the lexical path keeps
/// the literal nesting caught in that case, which is the common shape.
fn resolved(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The dispatch entries `state` wants armed for `vendor`, in a deterministic
/// order.
///
/// Reads each recorded hook artifact's materialized `hook.toml` from its
/// `ClientOutput` payload directory — the manifest is not in install state, and
/// duplicating it there would create a second copy of a published format to keep
/// in sync. `trust` is the per-registry consent predicate (C-022, WP-G): a
/// `false` answer drops the artifact from the set entirely, which is how "no
/// dispatch entry for an untrusted registry" is expressed structurally rather
/// than as a runtime check.
///
/// A plain `&dyn Fn` rather than a trait: there is one caller and one
/// implementation, and a trait for that is the premature abstraction
/// `quality-core.md` names.
///
/// # `trust` must be **pure**, and the grant it may imply belongs above this
///
/// The predicate is evaluated here, once per recorded hook artifact, inside a
/// function `Vendor::sync_config` calls **once per client per command** — three
/// v1 clients × install / update / uninstall / every TUI action. So a predicate
/// that *prompted* would prompt up to three times for one consent, and one that
/// *persisted* a grant would rewrite global config up to three times. WP-G's own
/// [`Arming::ConsentRequired`](crate::hook::trust::Arming::ConsentRequired)
/// states the same split from the other side: the prompt is kept out of
/// `arming` so the decision stays pure and the prompt lives in exactly one
/// place. That place is the command boundary, above the per-client loop — never
/// this seam.
///
/// **And when it runs there, it must hold the config lock.**
/// [`crate::hook::trust::persist_grant`] deliberately takes none: it is a
/// read-modify-write of the **global** `grimoire.toml` whose write seam
/// re-serializes the entire file, so two grim processes granting concurrently
/// are last-writer-wins on *every* declaration in that file, not merely on the
/// grant. Wrap it the way `command::config::commit_config` already does:
/// `command::scope_resolution::lockable_path` →
/// `lock::file_lock::ConfigFileLock::try_acquire`. **The project-scope lock a
/// `grim install` already holds does not cover this** — it guards a different
/// file, and assuming otherwise is the trap that makes the omission look safe
/// from inside an install.
///
/// # ⛔ The payload directory is derived from `$GRIM_HOME`, **never** from the
/// record (SEC-1)
///
/// This function used to resolve `output.target` — the install record's own
/// stored path — and read `hook.toml` out of it. That is what made SEC-1
/// exploitable: a cloned repository carrying its own committed
/// `.grimoire/state.json` *and* payload armed on a fresh machine, offline, with
/// no install history, because the integrity gate compares the recorded hash
/// against the on-disk payload and the attacker supplies both. **The relocation
/// of the payload out of the workspace is not by itself the fix; this derivation
/// is.** A record must never be able to name the directory grim reads an armed
/// manifest from — see
/// [`hook_dispatch::payload_dir`](super::hook_dispatch::payload_dir) for the full
/// finding.
///
/// The record still says *which clients* armed (that is what an install wrote),
/// and its pin still supplies `resolved_digest`. Neither can redirect a read.
///
/// The directory goes through [`AnchoredPath`](super::path_anchor::AnchoredPath)
/// under [`Containment::Strict`] rather than a bare join, because `record.name`
/// arrives from a state file and is therefore untrusted: Layer 1 refuses a `..`
/// in it without touching the filesystem, and Layer 2 refuses a symlinked
/// ancestor escaping `$GRIM_HOME`. `Strict`, not `AllowRelocatedAncestor` — this
/// path becomes the working directory of an executed handler, not a read-only
/// probe.
///
/// # The signature gained `vendor` and `roots`, and lost `workspace`/`scope`
///
/// The stub's `(state, workspace, scope, trust)` shape could not be implemented
/// as written, and the reason is worth recording. Resolving anything to a
/// directory takes [`AnchorRoots`], which no ambient lookup may substitute
/// (`subsystem-file-structure.md`: the roots are resolved once, at
/// scope-resolution time). `scope` came back as `root`, the semantic
/// [`RootScope`] the caller already derives for the dispatch table — one value,
/// so the table key and the payload location cannot disagree about which
/// workspace is being armed.
///
/// # Errors
///
/// An I/O failure reading a payload directory, or a `hook.toml` that no longer
/// parses — the second is *not* tolerated silently: a manifest that changed
/// under grim means the payload drifted, and arming a set derived from an
/// unparsable manifest is worse than reporting it. Both reach the caller as a
/// per-client `warn` and zero armed entries, never as a failed command (I3).
///
/// A manifest that parses but fails
/// [`HookManifest::validate_installed`](crate::oci::hook::HookManifest::validate_installed)
/// is **not** an error here (P-3): it costs that one artifact its rows, with a
/// warning, and every other artifact still arms. The difference is deliberate —
/// an unparsable manifest means grim cannot tell what the payload is at all,
/// while an invalid one is a publisher who skipped `grim build`, and one such
/// publisher must not disarm a user's other hooks.
fn desired_entries(
    vendor: &dyn Vendor,
    state: &InstallState,
    roots: &AnchorRoots,
    root: RootScope<'_>,
    trust: &dyn Fn(&LockedSource) -> bool,
) -> io::Result<Vec<DesiredHook>> {
    let mut entries = Vec::new();
    for record in state.iter_records() {
        if record.kind != ArtifactKind::Hook || !trust(&record.source) {
            continue;
        }
        // P-2, the second half. The primary control is in `installer::install_one`,
        // before materialization — nothing is written for a refused binding, so
        // nothing can be reaped. This one covers a record that predates that gate
        // (or was hand-written), and it asks the same two questions: is the name a
        // name at all, and does a valid name collide with grim's own files. A
        // traversing binding would have grim read an armed manifest out of a
        // directory the *repository* chose (T3/I1); a reserved one would read
        // grim's own launcher directory. Warn and skip, never fail (I3).
        if let Some(reason) = crate::oci::hook::binding_name_refusal(&record.name) {
            tracing::warn!(client = vendor.name(), "hook '{}' is not armed: {reason}", record.name,);
            continue;
        }
        let resolved_digest = match &record.source {
            LockedSource::Registry(pinned) => Some(pinned.digest().to_string()),
            // A path-sourced (dev) install has no registry pin, and W4 is
            // explicit that this field is provenance rather than a gate — so
            // there is nothing to invent here.
            LockedSource::Path { .. } => None,
        };
        // Nothing recorded for this client means this client never armed this
        // hook — skipped before the manifest read, so a hook armed only for a
        // sibling client costs no I/O here.
        if !record.outputs.iter().any(|o| o.client == vendor.name()) {
            continue;
        }
        let anchored = AnchoredPath {
            anchor: PathAnchor::GrimHome,
            relative: super::hook_dispatch::payload_relative(root, &record.name),
        };
        let payload_dir = anchored
            .resolve(roots, Containment::Strict)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        for output in record.outputs.iter().filter(|o| o.client == vendor.name()) {
            let manifest_path = payload_dir.join(HOOK_MANIFEST_FILE);
            let source = std::fs::read_to_string(&manifest_path)?;
            let manifest = HookManifest::from_toml_str(&source)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", manifest_path.display())))?;

            // P-3 — re-apply the vendor-independent build rules against the
            // MATERIALIZED payload, because `HookManifest::validate`'s only
            // caller is `grim build` and a publisher who pushes with any other
            // OCI client satisfied none of them. See
            // `HookManifest::validate_installed` for the rules it cannot
            // re-apply here (the binding name is the user's, not the
            // publisher's) and for the ones that are a per-client verdict rather
            // than a manifest rule.
            //
            // Drop the artifact with a warning; never fail the command (I3). A
            // whole-artifact skip rather than a per-entry one is deliberate: the
            // rules are cross-entry (duplicate `id`s) and an author who got one
            // entry wrong has published a manifest grim would not have built, so
            // arming the rest of it would be arming a set nothing checked.
            if let Err(e) = manifest.validate_installed(&payload_dir) {
                tracing::warn!(
                    client = vendor.name(),
                    "hook '{}' is not armed: its installed {HOOK_MANIFEST_FILE} does not satisfy the rules `grim \
                     build` enforces, so it was published without them: {e}",
                    record.name,
                );
                continue;
            }

            for hook in manifest.hooks {
                // The manifest entry is kept beside the row it produces: the
                // dispatch table takes the row, `Vendor::hook_registration`
                // takes the entry, and both must come from this one read.
                let manifest_entry = hook.clone();
                // A native-only moment (`<vendor>.event` with no canonical
                // `event`) has no `CanonicalEvent`, and the dispatch table is
                // keyed on one. Skipped with a line rather than defaulted: a
                // substituted moment runs a guardrail at the wrong time, which
                // the format's own docs call out as the failure to avoid.
                let Some(event) = hook.event else {
                    tracing::debug!(
                        "hook '{}/{}' declares a native-only moment; not dispatchable in v1",
                        record.name,
                        hook.id
                    );
                    continue;
                };
                entries.push(DesiredHook {
                    manifest: manifest_entry,
                    event,
                    entry: DispatchEntry {
                        artifact: record.name.clone(),
                        id: hook.id,
                        // The arming client, taken from the output the per-vendor
                        // filter above already selected — **not** re-derived from
                        // `vendor.name()`. Same value today; the output is the
                        // record's statement that this client armed, which is the
                        // one thing the record is still allowed to say (the
                        // `payload_dir` above deliberately no longer comes from it
                        // — SEC-1). Required, because every other field here is
                        // client-independent and without it two clients' rows
                        // would be byte-identical (F-1 — see
                        // `DispatchEntry::client`).
                        client: output.client.clone(),
                        event,
                        tier: hook.tier,
                        matcher: hook.matcher,
                        handler: hook.handler,
                        timeout: hook.timeout,
                        payload: hook.payload.unwrap_or_default(),
                        payload_dir: payload_dir.clone(),
                        resolved_digest: resolved_digest.clone(),
                        policy: hook.policy,
                    },
                });
            }
        }
    }
    // Deterministic order, so a re-write with no change is byte-identical and
    // `converge_root` can answer `Unchanged` (Principle 9's self-heal
    // obligation). `iter_records` order is already stable; the sort makes the
    // guarantee local rather than inherited.
    entries.sort_by(|a, b| {
        (&a.entry.artifact, &a.entry.id, a.entry.event.as_str()).cmp(&(
            &b.entry.artifact,
            &b.entry.id,
            b.entry.event.as_str(),
        ))
    });
    Ok(entries)
}

/// The semantic root `scope` arms for — [`RootScope::Global`] at global scope,
/// [`RootScope::Workspace`] at project scope.
///
/// One line, and it exists so that no caller writes the mapping itself: the
/// whole of B3's discipline is that the root is grim-chosen from the resolved
/// scope and never from `$PWD`, the envelope `cwd`, or a walk-up.
///
/// `pub` because the payload location keys on the same semantic root
/// ([`hook_dispatch::payload_dir`](super::hook_dispatch::payload_dir)) and
/// [`InstallTarget::path_for`](super::target::InstallTarget::path_for) has to
/// spell it. One mapping, two derivations off it — an opaque HMAC for the wire,
/// a plain path digest for the machine-local directory.
pub fn root_scope_for<'a>(workspace: &'a Path, scope: ConfigScope) -> RootScope<'a> {
    match scope {
        ConfigScope::Global => RootScope::Global,
        ConfigScope::Project => RootScope::Workspace(workspace),
    }
}

/// What the best-effort git-exclude hygiene step did.
///
/// **Never a gate.** Every non-`Added`/`AlreadyPresent` outcome still arms, and
/// is noted in `grim status`. A git ignore rule governs `git add` and
/// `git status`, not reads — an attacker's committed `settings.local.json` is
/// read on any machine whatever any ignore rule says — so this is hygiene (it
/// stops the *user* publishing their own arming), not the T3 control. T3 is held
/// by the absolute launcher path, the ownership marker and digest-pinned
/// approval. Blocking here would trade a real availability failure for a hygiene
/// benefit, which is exactly what invariant **I3** forbids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExcludeOutcome {
    /// The rule was appended to `.git/info/exclude`.
    Added,
    /// It was already there — nothing written.
    AlreadyPresent,
    /// The rule was removed (the last project-scope hook went away).
    Removed,
    /// Nothing to remove.
    Absent,
    /// Not a git worktree — no `.git` at all, or a shape grim does not
    /// recognise. Arms anyway.
    NotAGitWorktree,
    /// `.git/info/exclude` could not be created or appended to. Arms anyway.
    Unwritable,
    /// `.claude/settings.local.json` is **already tracked**, which makes any
    /// ignore rule inert. Arms anyway — and this is the outcome most worth
    /// surfacing, because it means the user's own arming *will* show up in
    /// `git status`.
    AlreadyTracked,
}

/// Append `.claude/settings.local.json` to `.git/info/exclude` — per-clone,
/// never committed, no diff for anyone to review.
///
/// grim does this itself because it may register *before* the client ever
/// creates that file, and because Claude Code writes its own ignore rule to the
/// **user's global excludes** (`core.excludesfile`, default
/// `~/.config/git/ignore`) rather than to the repository — executed against
/// Claude Code 2.1.233. That is a sensible implementation of the `.local.`
/// convention and not a defect; it just means the repo has no rule until grim
/// adds one.
///
/// Idempotent, and it never rewrites an existing line.
pub fn ensure_settings_local_excluded(workspace: &Path) -> ExcludeOutcome {
    let Some(info_dir) = git_info_dir(workspace) else {
        return ExcludeOutcome::NotAGitWorktree;
    };
    if is_tracked(workspace, CLAUDE_LOCAL_SETTINGS) {
        return ExcludeOutcome::AlreadyTracked;
    }

    let exclude = info_dir.join("exclude");
    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    if exclude_lines(&existing).any(|line| line == CLAUDE_LOCAL_SETTINGS) {
        return ExcludeOutcome::AlreadyPresent;
    }

    // Append, never rewrite: `.git/info/exclude` is the user's file too, and
    // git's own default ships it with a comment header worth keeping.
    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(CLAUDE_LOCAL_SETTINGS);
    next.push('\n');
    if std::fs::create_dir_all(&info_dir).is_err() || std::fs::write(&exclude, next).is_err() {
        return ExcludeOutcome::Unwritable;
    }
    ExcludeOutcome::Added
}

/// `$GIT_DIR/info` for `workspace`, or `None` when it is not a git worktree.
///
/// Both `.git` shapes, because the linked-worktree one is not exotic — this
/// repository's own agent worktrees live under `.agents/worktrees/`. A directory
/// gives `<ws>/.git/info`; a `gitdir: <path>` file (linked worktree, submodule)
/// gives `<that path>/info`, which is the right target because git reads
/// `$GIT_DIR/info/exclude`, and `$GIT_DIR` for a linked worktree is its own
/// per-worktree directory rather than the shared common dir.
fn git_info_dir(workspace: &Path) -> Option<PathBuf> {
    let dot_git = workspace.join(".git");
    let meta = std::fs::metadata(&dot_git).ok()?;
    if meta.is_dir() {
        return Some(dot_git.join("info"));
    }
    let pointer = std::fs::read_to_string(&dot_git).ok()?;
    let target = pointer.lines().find_map(|line| line.strip_prefix("gitdir:"))?.trim();
    let target = Path::new(target);
    let git_dir = if target.is_absolute() {
        target.to_path_buf()
    } else {
        workspace.join(target)
    };
    Some(git_dir.join("info"))
}

/// The exclude file's rule lines — comments and blanks dropped, each trimmed.
///
/// A separate function so the append and the removal agree on what "the rule is
/// already there" means; two slightly different readings of one file is how an
/// idempotent step stops being idempotent.
fn exclude_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
}

/// Whether `relative` is tracked in `workspace`'s index, which makes any ignore
/// rule inert.
///
/// The one subprocess on this path, and it is gated on the file existing at all:
/// an absent file cannot be tracked, so the common case (grim registering before
/// Claude ever writes its settings) spawns nothing. Any failure to run `git` — not
/// installed, not a repository, an unexpected status — answers `false`, so the
/// rule is still appended. That is the right direction for a hygiene step: a
/// redundant exclude line costs nothing, a missing one is the thing this exists
/// to prevent.
fn is_tracked(workspace: &Path, relative: &str) -> bool {
    if !workspace.join(relative).exists() {
        return false;
    }
    std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(relative)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Remove grim's `.git/info/exclude` line when the last project-scope hook goes.
///
/// The inverse of [`ensure_settings_local_excluded`], and tolerant in the same
/// direction: an absent file, an absent line, or an unwritable file all converge
/// without failing the uninstall that triggered them.
pub fn drop_settings_local_exclude(workspace: &Path) -> ExcludeOutcome {
    let Some(info_dir) = git_info_dir(workspace) else {
        return ExcludeOutcome::NotAGitWorktree;
    };
    let exclude = info_dir.join("exclude");
    let Ok(existing) = std::fs::read_to_string(&exclude) else {
        return ExcludeOutcome::Absent;
    };
    if !exclude_lines(&existing).any(|line| line == CLAUDE_LOCAL_SETTINGS) {
        return ExcludeOutcome::Absent;
    }

    // Only grim's own line goes; every other byte — comments, blanks, the user's
    // rules, their order — survives verbatim.
    let mut next = String::with_capacity(existing.len());
    for line in existing.lines() {
        if line.trim() == CLAUDE_LOCAL_SETTINGS {
            continue;
        }
        next.push_str(line);
        next.push('\n');
    }
    if std::fs::write(&exclude, next).is_err() {
        return ExcludeOutcome::Unwritable;
    }
    ExcludeOutcome::Removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::vendor_claude::ClaudeVendor;
    use crate::install::vendor_codex::CodexVendor;

    fn git_worktree(root: &Path) {
        std::fs::create_dir_all(root.join(".git/info")).unwrap();
    }

    #[test]
    fn a_relative_grim_home_refuses_to_arm() {
        let refusal = validate_grim_home(Path::new(".grimoire"), Path::new("/home/dev/p"), ConfigScope::Project);
        assert_eq!(refusal, Err(ArmRefusal::GrimHomeRelative));
    }

    #[test]
    fn a_grim_home_inside_the_workspace_refuses_to_arm_at_either_scope() {
        let ws = tempfile::tempdir().unwrap();
        let nested = ws.path().join("tools/grim");
        std::fs::create_dir_all(&nested).unwrap();

        for scope in [ConfigScope::Project, ConfigScope::Global] {
            assert_eq!(
                validate_grim_home(&nested, ws.path(), scope),
                Err(ArmRefusal::GrimHomeInWorkspace),
                "{scope:?}"
            );
        }
    }

    #[test]
    fn a_symlinked_grim_home_pointing_into_the_workspace_is_the_same_refusal() {
        let ws = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let real = ws.path().join("tools/grim");
        std::fs::create_dir_all(&real).unwrap();
        let link = outside.path().join("grim-home");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        #[cfg(not(unix))]
        return;

        assert_eq!(
            validate_grim_home(&link, ws.path(), ConfigScope::Project),
            Err(ArmRefusal::GrimHomeInWorkspace)
        );
    }

    #[test]
    fn a_grim_home_outside_the_workspace_arms() {
        let ws = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        assert_eq!(validate_grim_home(home.path(), ws.path(), ConfigScope::Project), Ok(()));
        assert_eq!(arming_refusal(home.path(), ws.path(), ConfigScope::Project), None);
    }

    #[test]
    fn arming_refusal_reports_a_grim_home_it_could_not_quote() {
        let ws = tempfile::tempdir().unwrap();
        let home = ws.path().parent().unwrap().join("gr\nim-home");
        assert_eq!(
            arming_refusal(&home, ws.path(), ConfigScope::Project),
            Some(ArmRefusal::LauncherPath(CommandRefusal::ControlCharacterInPath))
        );
    }

    #[test]
    fn every_refusal_cause_has_its_own_message() {
        let causes = [
            ArmRefusal::GrimHomeRelative,
            ArmRefusal::GrimHomeInWorkspace,
            ArmRefusal::LauncherPath(CommandRefusal::ControlCharacterInPath),
            ArmRefusal::DispatchLocked,
        ];
        let mut reasons: Vec<&str> = causes.iter().map(|c| c.reason()).collect();
        reasons.sort_unstable();
        reasons.dedup();
        // C-017 is explicit that four causes sharing one message is a defect.
        assert_eq!(reasons.len(), causes.len());
        for reason in reasons {
            assert!(!reason.ends_with('.'), "library style: no trailing period — {reason}");
        }
    }

    /// A `PreToolUse` observer hook record named `shell-guard`, materialized for
    /// every client in `clients`, with its payload directory (and a real
    /// `hook.toml`) under `<root>/hooks/shell-guard`.
    ///
    /// The payload is **one directory shared by every arming client** (S-003),
    /// which is exactly why each dispatch row has to carry its own client name.
    fn hook_record(
        grim_home: &Path,
        root: RootScope<'_>,
        clients: &[&str],
    ) -> crate::install::install_state::InstallRecord {
        hook_record_named(
            grim_home,
            root,
            clients,
            "shell-guard",
            "localhost:5000",
            "acme/shell-guard",
        )
    }

    /// [`hook_record`] with the artifact name and the **member's own** registry
    /// and repository chosen — the shape a bundle-delivered member has, where the
    /// bundle's registry and the member's need not agree.
    ///
    /// The payload is written where a real install puts it — under `$GRIM_HOME`,
    /// at **both** scopes (SEC-1) — and the recorded target is derived from the
    /// same helper, so the fixture cannot drift into recording one location and
    /// populating another.
    fn hook_record_named(
        grim_home: &Path,
        root: RootScope<'_>,
        clients: &[&str],
        name: &str,
        registry: &str,
        repository: &str,
    ) -> crate::install::install_state::InstallRecord {
        let payload = crate::install::hook_dispatch::payload_dir(grim_home, root, name);
        std::fs::create_dir_all(&payload).unwrap();
        std::fs::write(
            payload.join(HOOK_MANIFEST_FILE),
            "schema = 1\nname = \"shell-guard\"\ndescription = \"a guard\"\n\n\
             [[hooks]]\nid = \"guard\"\nevent = \"PreToolUse\"\ntier = \"observer\"\n\
             matcher = \"Bash\"\ncommand = \"sh guard.sh\"\n"
                .replace("shell-guard", name),
        )
        .unwrap();
        let target = crate::install::path_anchor::AnchoredPath {
            anchor: crate::install::path_anchor::PathAnchor::GrimHome,
            relative: crate::install::hook_dispatch::payload_relative(root, name),
        };
        let pin = crate::oci::PinnedIdentifier::try_from(
            crate::oci::Identifier::new_registry(repository, registry)
                .clone_with_digest(crate::oci::Digest::Sha256("a".repeat(64))),
        )
        .unwrap();
        crate::install::install_state::InstallRecord {
            kind: ArtifactKind::Hook,
            name: name.to_string(),
            source: LockedSource::Registry(pin),
            dev: false,
            outputs: clients
                .iter()
                .map(|client| crate::install::install_state::ClientOutput {
                    client: (*client).to_string(),
                    target: target.clone(),
                    content_hash: crate::oci::Digest::Sha256("b".repeat(64)),
                    support_dir: None,
                    entry: None,
                    adopted: false,
                })
                .collect(),
        }
    }

    /// A hermetic [`AnchorRoots`] rooted at `grim_home` and `workspace`.
    ///
    /// Two **separate** temp dirs, always: a `$GRIM_HOME` inside the workspace
    /// is C-017 cause 2 and would make every convergence test assert the
    /// refusal path instead of the one it is about.
    fn roots_for(grim_home: &Path, workspace: &Path) -> AnchorRoots {
        AnchorRoots {
            workspace: workspace.to_path_buf(),
            grim_home: grim_home.to_path_buf(),
            vendor_roots: Default::default(),
            opencode_skills: None,
            claude_user_dir: None,
            agents_skills: None,
        }
    }

    /// The three values [`ArmedPaths`] borrows, owned, so a test that calls
    /// [`register_desired`] directly can hold them across the call.
    ///
    /// The [`RootToken`] is minted through [`super::hook_dispatch::root_token`]
    /// because that is the only way to obtain one — there is deliberately no
    /// `&str` constructor, since a forgeable token is B3 with extra steps.
    struct ProbeArmedPaths {
        launcher: PathBuf,
        table: PathBuf,
        token: RootToken,
    }

    impl ProbeArmedPaths {
        fn borrow(&self) -> ArmedPaths<'_> {
            ArmedPaths {
                launcher: &self.launcher,
                table: &self.table,
                token: &self.token,
            }
        }
    }

    fn probe_armed_paths(grim_home: &Path) -> ProbeArmedPaths {
        ProbeArmedPaths {
            launcher: crate::install::hook_launcher::launcher_path(grim_home),
            table: crate::install::hook_dispatch::dispatch_path(grim_home),
            token: crate::install::hook_dispatch::root_token(grim_home, RootScope::Global).unwrap(),
        }
    }

    /// A policy that arms everything it is asked about — the `--allow-hooks`
    /// shape, so a convergence test does not also have to build a trusted
    /// registry set.
    fn arming_policy() -> HookPolicy {
        HookPolicy::new(
            true,
            true,
            crate::hook::trust::Interactivity::NonInteractive,
            Vec::new(),
        )
    }

    /// A policy that arms nothing — the gated resting state.
    fn gated_policy() -> HookPolicy {
        HookPolicy::new(
            false,
            false,
            crate::hook::trust::Interactivity::NonInteractive,
            Vec::new(),
        )
    }

    #[test]
    fn no_hook_record_and_nothing_owned_writes_nothing_at_all() {
        // The whole-command fast path. It must not create the dispatch table, the
        // launcher, or `$GRIM_HOME/hooks/` — this runs on every install for every
        // user, most of whom have never seen a hook.
        let ws = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let state = InstallState::empty(&ws.path().join("state.json"));

        let outcomes = converge_clients(
            &state,
            ws.path(),
            ConfigScope::Project,
            &roots_for(home.path(), ws.path()),
            &arming_policy(),
        );
        assert!(outcomes.is_empty(), "{outcomes:?}");
        assert!(!home.path().join("hooks").exists(), "no writes at all on the fast path");
    }

    #[test]
    fn a_grim_owned_registration_with_no_record_is_reaped() {
        // The reap-after-uninstall case, and the one `owned − desired` exists
        // for: the record naming the matcher group has already left state, so
        // neither the group value nor the element is reconstructible and only the
        // enumeration can name what grim still owns.
        let ws = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let state = InstallState::empty(&ws.path().join("state.json"));
        let surface = ws.path().join(CLAUDE_LOCAL_SETTINGS);
        std::fs::create_dir_all(surface.parent().unwrap()).unwrap();
        std::fs::write(
            &surface,
            format!(
                r#"{{"hooks":{{"PreToolUse":[{{"matcher":"Bash","hooks":[{{"type":"command","command":"x","{HOOK_MARKER_KEY}":"{HOOK_MARKER_VALUE}"}}]}}]}}}}"#
            ),
        )
        .unwrap();

        let outcomes = converge_clients(
            &state,
            ws.path(),
            ConfigScope::Project,
            &roots_for(home.path(), ws.path()),
            &arming_policy(),
        );
        assert_eq!(
            outcomes,
            vec![(ClientTarget::Claude, HookSync::Disarmed)],
            "a grim-owned element with no record must be reaped, not left armed"
        );
        let text = std::fs::read_to_string(&surface).unwrap();
        assert!(
            !text.contains(HOOK_MARKER_VALUE),
            "the marked element must be gone — {text}"
        );
    }

    #[test]
    fn an_unmarked_user_owned_config_is_left_untouched() {
        // grim owns the marked element, never the file. A config carrying only
        // the user's own hooks is not grim's to converge, and the fast path must
        // not even open it for writing.
        let ws = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let state = InstallState::empty(&ws.path().join("state.json"));
        let surface = ws.path().join(CLAUDE_LOCAL_SETTINGS);
        std::fs::create_dir_all(surface.parent().unwrap()).unwrap();
        let authored = r#"{"hooks":{"PreToolUse":[{"hooks":[{"command":"echo hi"}]}]}}"#;
        std::fs::write(&surface, authored).unwrap();

        let outcomes = converge_clients(
            &state,
            ws.path(),
            ConfigScope::Project,
            &roots_for(home.path(), ws.path()),
            &gated_policy(),
        );
        assert!(outcomes.is_empty(), "{outcomes:?}");
        assert_eq!(std::fs::read_to_string(&surface).unwrap(), authored);
    }

    #[test]
    fn an_own_file_client_owns_its_path_outright() {
        // `OwnFile` ownership is the path: an empty desired set removes the file,
        // and removing one that is already gone is not a failure.
        let dir = tempfile::tempdir().unwrap();
        let surface = dir.path().join("hooks.json");

        assert_eq!(
            converge_own_file(&CodexVendor, &surface, &[]).unwrap(),
            SurfaceWrite::Unchanged
        );
        std::fs::write(&surface, "{}").unwrap();
        assert_eq!(
            converge_own_file(&CodexVendor, &surface, &[]).unwrap(),
            SurfaceWrite::Changed
        );
        assert!(!surface.exists(), "grim owns the path, so a reap deletes the file");
    }

    /// **The end-to-end arming proof at unit level.** A real hook record plus an
    /// arming policy must produce a dispatch row keyed on the arming client AND a
    /// marked registration in the client's own config — and dropping the record
    /// must take both away again.
    ///
    /// This is the first test in the whole hooks plan that can fail if nothing
    /// arms, which is precisely the state four waves shipped in.
    #[test]
    fn a_recorded_hook_arms_a_dispatch_row_and_a_registration_then_reaps_both() {
        let ws = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let roots = roots_for(home.path(), ws.path());
        let surface = ws.path().join(CLAUDE_LOCAL_SETTINGS);

        let mut state = InstallState::empty(&ws.path().join("state.json"));
        state.record(hook_record(home.path(), RootScope::Workspace(ws.path()), &["claude"]));

        let outcomes = converge_clients(&state, ws.path(), ConfigScope::Project, &roots, &arming_policy());
        assert_eq!(
            outcomes,
            vec![(ClientTarget::Claude, HookSync::Armed(1))],
            "{outcomes:?}"
        );

        // 1. The dispatch table carries exactly one row, keyed on the arming
        //    client, under an opaque root token (never `global`, never the
        //    absolute workspace path — B3).
        let table: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(crate::install::hook_dispatch::dispatch_path(home.path())).unwrap(),
        )
        .unwrap();
        let roots_map = table["roots"].as_object().unwrap();
        assert_eq!(roots_map.len(), 1, "{table:#}");
        let (token, entry) = roots_map.iter().next().unwrap();
        assert_ne!(token, "global");
        assert!(
            !token.contains(&*ws.path().to_string_lossy()),
            "the root key must be opaque"
        );
        let hooks = entry["hooks"].as_array().unwrap();
        assert_eq!(hooks.len(), 1, "{entry:#}");
        assert_eq!(hooks[0]["client"], "claude");
        assert_eq!(hooks[0]["artifact"], "shell-guard");
        assert_eq!(hooks[0]["id"], "guard");

        // 2. The launcher exists and is executable — a shim whose `chmod` failed
        //    makes `[ -x "$L" ]` false and the hook never fires (S1).
        let launcher = crate::install::hook_launcher::launcher_path(home.path());
        assert!(launcher.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&launcher).unwrap().permissions().mode() & 0o111,
                0o111
            );
        }

        // 3. Claude's own config carries one marked handler element under the
        //    matcher group, with the absolute launcher path and the opaque root
        //    token in its command — never `${GRIM_HOME}` and never `--root global`.
        let registration: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&surface).unwrap()).unwrap();
        let group = &registration["hooks"]["PreToolUse"][0];
        assert_eq!(group["matcher"], "Bash");
        let element = &group["hooks"][0];
        assert_eq!(element[HOOK_MARKER_KEY], HOOK_MARKER_VALUE);
        let command = element["command"].as_str().unwrap();
        assert!(command.contains(&*launcher.to_string_lossy()), "{command}");
        assert!(command.contains("--client claude"), "{command}");
        assert!(command.contains("--event PreToolUse"), "{command}");
        assert!(
            !command.contains("GRIM_HOME"),
            "no env-derived executed path (I1) — {command}"
        );
        assert!(
            !command.contains("--root global"),
            "B3 forbids that literal — {command}"
        );

        // 4. Re-materializing changes nothing (Principle 9's self-heal
        //    obligation): the table and the registration are byte-identical.
        let table_before = std::fs::read(crate::install::hook_dispatch::dispatch_path(home.path())).unwrap();
        let surface_before = std::fs::read(&surface).unwrap();
        let again = converge_clients(&state, ws.path(), ConfigScope::Project, &roots, &arming_policy());
        assert_eq!(again, vec![(ClientTarget::Claude, HookSync::Unchanged)], "{again:?}");
        assert_eq!(
            std::fs::read(crate::install::hook_dispatch::dispatch_path(home.path())).unwrap(),
            table_before
        );
        assert_eq!(std::fs::read(&surface).unwrap(), surface_before);

        // 5. The uninstall direction: drop the record and both go away — the
        //    `owned − desired` reap, which is the half no reconstruction can do.
        let empty = InstallState::empty(&ws.path().join("state.json"));
        let reaped = converge_clients(&empty, ws.path(), ConfigScope::Project, &roots, &arming_policy());
        assert_eq!(reaped, vec![(ClientTarget::Claude, HookSync::Disarmed)], "{reaped:?}");
        let table: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(crate::install::hook_dispatch::dispatch_path(home.path())).unwrap(),
        )
        .unwrap();
        assert!(
            table["roots"].as_object().unwrap().is_empty(),
            "the root entry must be removed, not emptied in place — {table:#}"
        );
        let text = std::fs::read_to_string(&surface).unwrap();
        assert!(
            !text.contains(HOOK_MARKER_VALUE),
            "the registration must be reaped — {text}"
        );
    }

    /// A gated policy arms nothing **and disarms what an earlier run armed** —
    /// turning the feature off is the same code path as uninstalling.
    #[test]
    fn turning_the_feature_flag_off_disarms() {
        let ws = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let roots = roots_for(home.path(), ws.path());
        let mut state = InstallState::empty(&ws.path().join("state.json"));
        state.record(hook_record(home.path(), RootScope::Workspace(ws.path()), &["claude"]));

        assert_eq!(
            converge_clients(&state, ws.path(), ConfigScope::Project, &roots, &arming_policy()),
            vec![(ClientTarget::Claude, HookSync::Armed(1))]
        );
        // Same state, same record, flag off ⇒ the desired set is empty and both
        // the row and the registration are reaped.
        assert_eq!(
            converge_clients(&state, ws.path(), ConfigScope::Project, &roots, &gated_policy()),
            vec![(ClientTarget::Claude, HookSync::Disarmed)]
        );
        let text = std::fs::read_to_string(ws.path().join(CLAUDE_LOCAL_SETTINGS)).unwrap();
        assert!(!text.contains(HOOK_MARKER_VALUE), "{text}");
    }

    /// A user's own hook in the same config survives grim's whole
    /// arm-then-reap cycle byte for byte (S-008's "user-authored entries
    /// untouched").
    #[test]
    fn a_user_authored_hook_in_the_same_config_survives_arming_and_reaping() {
        let ws = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let roots = roots_for(home.path(), ws.path());
        let surface = ws.path().join(CLAUDE_LOCAL_SETTINGS);
        std::fs::create_dir_all(surface.parent().unwrap()).unwrap();
        std::fs::write(
            &surface,
            "{\n    \"permissions\": {\n        \"allow\": [\"Read\"]\n    },\n    \"hooks\": {\n        \"PreToolUse\": [\n            {\n                \"matcher\": \"Write\",\n                \"hooks\": [\n                    {\n                        \"type\": \"command\",\n                        \"command\": \"echo mine\"\n                    }\n                ]\n            }\n        ]\n    }\n}\n",
        )
        .unwrap();
        let authored = std::fs::read_to_string(&surface).unwrap();

        let mut state = InstallState::empty(&ws.path().join("state.json"));
        state.record(hook_record(home.path(), RootScope::Workspace(ws.path()), &["claude"]));
        converge_clients(&state, ws.path(), ConfigScope::Project, &roots, &arming_policy());
        let armed = std::fs::read_to_string(&surface).unwrap();
        assert!(
            armed.contains("echo mine"),
            "the user's own handler must survive — {armed}"
        );
        assert!(
            armed.contains("\"allow\": [\"Read\"]"),
            "unrelated keys survive — {armed}"
        );
        assert!(armed.contains(HOOK_MARKER_VALUE));

        let empty = InstallState::empty(&ws.path().join("state.json"));
        converge_clients(&empty, ws.path(), ConfigScope::Project, &roots, &arming_policy());
        let reaped = std::fs::read_to_string(&surface).unwrap();
        assert!(!reaped.contains(HOOK_MARKER_VALUE));
        assert!(reaped.contains("echo mine"));
        assert!(
            reaped.contains("\"allow\": [\"Read\"]"),
            "a reap must not rewrite the user's document — {reaped}"
        );
        // The user's own bytes come back exactly: grim owned one element and put
        // the file back the way it found it.
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&reaped).unwrap(),
            serde_json::from_str::<serde_json::Value>(&authored).unwrap()
        );
    }

    /// ⛔ **SEC-1, at unit level.** A record that names its own payload
    /// directory arms **nothing**: convergence reads the manifest from
    /// `$GRIM_HOME`, never from the record.
    ///
    /// The fixture is the exploit, minus the registry: a complete, parseable
    /// `hook.toml` inside the workspace, and an install record whose stored
    /// target points straight at it — the state a hostile repository commits.
    /// Nothing exists under `$GRIM_HOME`, so the desired set must be empty even
    /// though every byte the record names is present and valid. Relocating the
    /// payload without moving this *read* would leave the hole open, which is why
    /// the assertion is about the derivation and not about the directory.
    #[test]
    fn a_record_that_names_its_own_payload_directory_arms_nothing() {
        let ws = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let roots = roots_for(home.path(), ws.path());
        let root = RootScope::Workspace(ws.path());

        // The attacker's committed payload, in the workspace, valid and complete.
        let planted = ws.path().join(".grimoire/hooks/shell-guard");
        std::fs::create_dir_all(&planted).unwrap();
        std::fs::write(
            planted.join(HOOK_MANIFEST_FILE),
            "schema = 1\nname = \"shell-guard\"\ndescription = \"a guard\"\n\n\
             [[hooks]]\nid = \"guard\"\nevent = \"PreToolUse\"\ntier = \"observer\"\n\
             matcher = \"Bash\"\ncommand = \"sh guard.sh\"\n",
        )
        .unwrap();

        // The attacker's committed record, pointing at it.
        let mut record = hook_record(home.path(), root, &["claude"]);
        record.outputs[0].target = crate::install::path_anchor::AnchoredPath {
            anchor: crate::install::path_anchor::PathAnchor::Workspace,
            relative: ".grimoire/hooks/shell-guard".to_string(),
        };
        // Remove what a *legitimate* install would have left under `$GRIM_HOME`,
        // so the only manifest on this machine is the planted one.
        std::fs::remove_dir_all(crate::install::hook_dispatch::payload_dir(
            home.path(),
            root,
            "shell-guard",
        ))
        .unwrap();
        let mut state = InstallState::empty(&ws.path().join("state.json"));
        state.record(record);

        let trust = |_: &LockedSource| true;
        assert!(
            desired_entries(&ClaudeVendor, &state, &roots, root, &trust).is_err(),
            "the manifest must be read from $GRIM_HOME, where nothing was installed — \
             a record must never be able to redirect that read"
        );
        // No outcome row at all, which is the shape a failed per-client
        // projection already had: the client is dropped from the loop with a
        // `warn` and nothing is written for it (I3 — a convergence detail never
        // fails the command).
        assert!(
            converge_clients(&state, ws.path(), ConfigScope::Project, &roots, &arming_policy()).is_empty(),
            "a repo-resident payload must not arm"
        );
        assert!(
            !crate::install::hook_dispatch::dispatch_path(home.path()).is_file()
                || !std::fs::read_to_string(crate::install::hook_dispatch::dispatch_path(home.path()))
                    .unwrap()
                    .contains("shell-guard"),
            "no dispatch row may name a payload grim did not install"
        );
        assert!(
            !ws.path().join(CLAUDE_LOCAL_SETTINGS).exists(),
            "no registration may be written for a payload grim did not install"
        );

        // Positive control: the same record, with the payload where a real
        // install puts it, arms — so the negative above is about the derivation
        // and not about a fixture that could never arm at all.
        let mut armed_state = InstallState::empty(&ws.path().join("state.json"));
        armed_state.record(hook_record(home.path(), root, &["claude"]));
        assert_eq!(
            converge_clients(&armed_state, ws.path(), ConfigScope::Project, &roots, &arming_policy()),
            vec![(ClientTarget::Claude, HookSync::Armed(1))]
        );
    }

    /// The trust predicate is evaluated against **each hook record's own**
    /// `LockedSource`, never a container's.
    ///
    /// This is the requirement a hook delivered by a **bundle** makes concrete: a
    /// bundle published from registry A can legitimately pin a member from
    /// registry B, and trusting A must not arm B. `desired_entries` takes the
    /// predicate and applies it per record, so the property is structural — this
    /// pins it against a refactor that hoists the call out of the record loop.
    #[test]
    fn the_trust_predicate_is_evaluated_per_record_not_once_per_install() {
        let home = tempfile::tempdir().unwrap();
        let roots = roots_for(home.path(), home.path());
        let mut state = InstallState::empty(&home.path().join("state.json"));
        state.record(hook_record_named(
            home.path(),
            RootScope::Global,
            &["claude"],
            "trusted-guard",
            "trusted.example",
            "acme/trusted-guard",
        ));
        state.record(hook_record_named(
            home.path(),
            RootScope::Global,
            &["claude"],
            "other-guard",
            "other.example",
            "acme/other-guard",
        ));

        // A predicate that trusts exactly one registry — the shape a real
        // `HookPolicy` produces from a single global `[[registries]]` grant.
        let trust = |source: &LockedSource| source.pinned().is_some_and(|pin| pin.registry() == "trusted.example");
        let desired = desired_entries(&ClaudeVendor, &state, &roots, RootScope::Global, &trust).unwrap();

        let armed: Vec<&str> = desired.iter().map(|d| d.entry.artifact.as_str()).collect();
        assert_eq!(
            armed,
            vec!["trusted-guard"],
            "only the member whose OWN registry is trusted may arm — a sibling from an \
             untrusted registry must be absent from the desired set"
        );
    }

    #[test]
    fn a_refusal_writes_nothing_and_reports_every_client() {
        // C-017 step 1: refuse early. A `$GRIM_HOME` nested inside the workspace
        // makes the arming authority repo-resident (I1), and the refusal must
        // land before the launcher, the table, or any registration is written.
        let ws = tempfile::tempdir().unwrap();
        let nested = ws.path().join("tools/grim");
        std::fs::create_dir_all(&nested).unwrap();
        let mut state = InstallState::empty(&ws.path().join("state.json"));
        state.record(hook_record(&nested, RootScope::Workspace(ws.path()), &["claude"]));

        let outcomes = converge_clients(
            &state,
            ws.path(),
            ConfigScope::Project,
            &roots_for(&nested, ws.path()),
            &arming_policy(),
        );
        assert_eq!(
            outcomes,
            vec![(
                ClientTarget::Claude,
                HookSync::NotArmed(ArmRefusal::GrimHomeInWorkspace)
            )]
        );
        // Nothing *armable*: no launcher, no dispatch table, no root key. The
        // payload directory itself is the fixture's own doing (a real install
        // materializes it before convergence runs, and a refusal must not delete
        // it), so the assertion names the three files that arm rather than the
        // `hooks/` directory as a whole.
        assert!(
            !crate::install::hook_launcher::launcher_path(&nested).exists(),
            "a refusal must not write the launcher"
        );
        assert!(
            !crate::install::hook_dispatch::dispatch_path(&nested).exists(),
            "a refusal must not write the dispatch table"
        );
        assert!(
            !crate::install::hook_dispatch::root_key_path(&nested).exists(),
            "a refusal must not mint the machine key"
        );
        assert!(!ws.path().join(CLAUDE_LOCAL_SETTINGS).exists());
    }

    /// **F-1.** `desired_entries` is **per vendor** — it filters
    /// `record.outputs` by client, and the shared `payload_dir` it reads comes
    /// from `$GRIM_HOME` (never from an output — SEC-1) — while `converge_root`
    /// replaces a root's `hooks` vector **wholesale**. The two only compose if
    /// each row says which client armed it, so this pins that every row carries
    /// its own vendor's name.
    ///
    /// It also pins *why* the field cannot be defaulted: a hook's payload is one
    /// directory per scope shared by every arming client (S-003) and every other
    /// field comes from the record or the manifest, so with `client` stripped
    /// the two vendors' rows are **byte-identical** — and "armed for claude
    /// only" would be indistinguishable from "armed for claude and codex". That
    /// is the leg where a client grim `Declined` runs code the user was told was
    /// not armed for it.
    #[test]
    fn desired_entries_stamps_each_row_with_its_own_arming_client() {
        let home = tempfile::tempdir().unwrap();
        let mut state = InstallState::empty(&home.path().join("state.json"));
        state.record(hook_record(home.path(), RootScope::Global, &["claude", "codex"]));
        let roots = roots_for(home.path(), home.path());

        let trust = |_: &LockedSource| true;
        let claude = desired_entries(&ClaudeVendor, &state, &roots, RootScope::Global, &trust).unwrap();
        let codex = desired_entries(&CodexVendor, &state, &roots, RootScope::Global, &trust).unwrap();

        assert_eq!(claude.len(), 1, "{claude:?}");
        assert_eq!(codex.len(), 1, "{codex:?}");
        assert_eq!(claude[0].entry.client, "claude");
        assert_eq!(codex[0].entry.client, "codex");
        assert_eq!(
            claude[0].entry.payload_dir, codex[0].entry.payload_dir,
            "the payload is client-independent (S-003) — which is exactly why the client field is needed"
        );
        assert_eq!(
            DispatchEntry {
                client: codex[0].entry.client.clone(),
                ..claude[0].entry.clone()
            },
            codex[0].entry,
            "the two rows differ in NOTHING but the client"
        );
    }

    /// The union `converge_root` is given is over **every** hook-capable client,
    /// not the per-client set — because it replaces a root's `hooks` vector
    /// wholesale, so a per-client call would make the last client's write erase
    /// every earlier client's rows.
    #[test]
    fn the_dispatch_union_carries_every_arming_client_deterministically() {
        let home = tempfile::tempdir().unwrap();
        let mut state = InstallState::empty(&home.path().join("state.json"));
        state.record(hook_record(home.path(), RootScope::Global, &["codex", "claude"]));
        let roots = roots_for(home.path(), home.path());
        let trust = |_: &LockedSource| true;

        let armed_paths = probe_armed_paths(home.path());
        let per_client = vec![
            register_desired(
                ClientTarget::Codex,
                None,
                &desired_entries(&CodexVendor, &state, &roots, RootScope::Global, &trust).unwrap(),
                &armed_paths.borrow(),
            ),
            register_desired(
                ClientTarget::Claude,
                None,
                &desired_entries(&ClaudeVendor, &state, &roots, RootScope::Global, &trust).unwrap(),
                &armed_paths.borrow(),
            ),
        ];
        let union = union_of(&per_client);
        let clients: Vec<&str> = union.iter().map(|e| e.client.as_str()).collect();
        assert_eq!(
            clients,
            vec!["claude", "codex"],
            "the union must hold BOTH clients' rows, ordered deterministically \
             regardless of the order the clients were converged in"
        );
    }

    #[test]
    fn root_scope_follows_the_resolved_scope_never_the_cwd() {
        let ws = Path::new("/home/dev/p");
        assert_eq!(root_scope_for(ws, ConfigScope::Global), RootScope::Global);
        assert_eq!(root_scope_for(ws, ConfigScope::Project), RootScope::Workspace(ws));
    }

    #[test]
    fn the_exclude_rule_is_appended_once_then_removed() {
        let ws = tempfile::tempdir().unwrap();
        git_worktree(ws.path());
        let exclude = ws.path().join(".git/info/exclude");
        std::fs::write(
            &exclude,
            "# git ls-files --others --exclude-from=.git/info/exclude\n*.tmp\n",
        )
        .unwrap();

        assert_eq!(ensure_settings_local_excluded(ws.path()), ExcludeOutcome::Added);
        let text = std::fs::read_to_string(&exclude).unwrap();
        assert!(text.contains("*.tmp"), "the user's own rules survive");
        assert!(text.contains(CLAUDE_LOCAL_SETTINGS));

        assert_eq!(
            ensure_settings_local_excluded(ws.path()),
            ExcludeOutcome::AlreadyPresent
        );
        assert_eq!(std::fs::read_to_string(&exclude).unwrap(), text);

        assert_eq!(drop_settings_local_exclude(ws.path()), ExcludeOutcome::Removed);
        let text = std::fs::read_to_string(&exclude).unwrap();
        assert!(text.contains("*.tmp"));
        assert!(!text.contains(CLAUDE_LOCAL_SETTINGS));
        assert_eq!(drop_settings_local_exclude(ws.path()), ExcludeOutcome::Absent);
    }

    #[test]
    fn a_missing_exclude_file_is_created_rather_than_refused() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(ws.path().join(".git")).unwrap();
        assert_eq!(ensure_settings_local_excluded(ws.path()), ExcludeOutcome::Added);
        assert!(ws.path().join(".git/info/exclude").is_file());
    }

    #[test]
    fn a_linked_worktree_writes_to_its_own_git_dir() {
        // `.git` is a *file* here, which is the shape a plain
        // `workspace.join(GIT_EXCLUDE_RELATIVE)` would silently miss.
        let common = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let git_dir = common.path().join("worktrees/impl-i");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(ws.path().join(".git"), format!("gitdir: {}\n", git_dir.display())).unwrap();

        assert_eq!(ensure_settings_local_excluded(ws.path()), ExcludeOutcome::Added);
        assert!(
            std::fs::read_to_string(git_dir.join("info/exclude"))
                .unwrap()
                .contains(CLAUDE_LOCAL_SETTINGS)
        );
        assert!(!ws.path().join(".git/info").exists());
    }

    #[test]
    fn a_non_git_directory_arms_anyway() {
        let ws = tempfile::tempdir().unwrap();
        assert_eq!(
            ensure_settings_local_excluded(ws.path()),
            ExcludeOutcome::NotAGitWorktree
        );
        assert_eq!(drop_settings_local_exclude(ws.path()), ExcludeOutcome::NotAGitWorktree);
    }

    #[test]
    fn a_tracked_settings_file_is_reported_rather_than_excluded() {
        let ws = tempfile::tempdir().unwrap();
        if std::process::Command::new("git").arg("--version").output().is_err() {
            return;
        }
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
        ] {
            std::process::Command::new("git")
                .arg("-C")
                .arg(ws.path())
                .args(args)
                .output()
                .unwrap();
        }
        std::fs::create_dir_all(ws.path().join(".claude")).unwrap();
        std::fs::write(ws.path().join(CLAUDE_LOCAL_SETTINGS), "{}").unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(ws.path())
            .args(["add", "-f", CLAUDE_LOCAL_SETTINGS])
            .output()
            .unwrap();

        // An ignore rule is inert against a tracked file, and this is the
        // outcome most worth surfacing — the user's arming *will* show in
        // `git status`.
        assert_eq!(
            ensure_settings_local_excluded(ws.path()),
            ExcludeOutcome::AlreadyTracked
        );
    }

    /// **P-1's regression pin, inverted from the wave-7 audit's demonstration:
    /// a `HookDecline` keeps the declined hook out of the dispatch table.**
    ///
    /// The audit committed this test asserting the *defect* — that the declined
    /// row was present — and required whoever fixed it to invert the assertions
    /// rather than delete them. Read `.agents/security_audit_hooks.md` § P-1 for
    /// the finding it closes.
    ///
    /// One artifact declares two entries at `PreToolUse`:
    ///
    /// - `watch` — an `observer` on `Bash`, which registers normally;
    /// - `rewrite` — a `mutator` on `Bash`, which
    ///   [`Vendor::hook_registration`] **declines** with
    ///   `HookDecline::MutatorOnShellCommandTool`. ADR decision K exists to
    ///   refuse exactly this shape: a mutator must never rewrite a
    ///   shell-command-string tool, because the client displays the un-mutated
    ///   command while executing the mutated one.
    ///
    /// The sibling entry is the whole point: the runtime selects rows by
    /// `(root, client, event)`, a key with no decline dimension, so `watch`
    /// registering at the same `(claude, PreToolUse)` is what used to make the
    /// declined row reachable. `union_of` is now built from
    /// [`register_desired`]'s accepted set, so the row is simply absent and no
    /// sibling can bring it back.
    #[test]
    fn a_declined_mutator_is_kept_out_of_the_dispatch_table_p1() {
        let ws = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let roots = roots_for(home.path(), ws.path());
        let root = RootScope::Workspace(ws.path());

        let mut state = InstallState::empty(&ws.path().join("state.json"));
        state.record(hook_record(home.path(), root, &["claude"]));

        // The payload: one artifact, two entries, the second the shape decision K
        // refuses. Written *after* `hook_record`, which materializes a
        // single-entry manifest of its own at the same path.
        let payload = crate::install::hook_dispatch::payload_dir(home.path(), root, "shell-guard");
        std::fs::create_dir_all(&payload).unwrap();
        std::fs::write(
            payload.join(HOOK_MANIFEST_FILE),
            "schema = 1\nname = \"shell-guard\"\ndescription = \"a guard\"\n\n\
             [[hooks]]\nid = \"watch\"\nevent = \"PreToolUse\"\ntier = \"observer\"\n\
             matcher = \"Bash\"\ncommand = \"sh guard.sh\"\n\n\
             [[hooks]]\nid = \"rewrite\"\nevent = \"PreToolUse\"\ntier = \"mutator\"\n\
             matcher = \"Bash\"\ncommand = \"sh rewrite.sh\"\n",
        )
        .unwrap();

        let outcomes = converge_clients(&state, ws.path(), ConfigScope::Project, &roots, &arming_policy());
        assert_eq!(
            outcomes,
            vec![(ClientTarget::Claude, HookSync::Armed(1))],
            "exactly ONE of the two entries registers — the mutator is declined: {outcomes:?}"
        );

        // Claude's own config carries one handler element, so the user's only
        // visible statement about `rewrite` is the decline warning.
        let registration: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(ws.path().join(CLAUDE_LOCAL_SETTINGS)).unwrap()).unwrap();
        let elements = registration["hooks"]["PreToolUse"][0]["hooks"].as_array().unwrap();
        assert_eq!(elements.len(), 1, "{registration:#}");

        // …and the dispatch table carries exactly the row that registered. The
        // declined mutator has none, so `--client claude --event PreToolUse`
        // cannot select it however many siblings arm at that pair.
        let table: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(crate::install::hook_dispatch::dispatch_path(home.path())).unwrap(),
        )
        .unwrap();
        let hooks = table["roots"].as_object().unwrap().values().next().unwrap()["hooks"]
            .as_array()
            .unwrap()
            .clone();
        assert!(
            hooks.iter().all(|row| row["id"] != "rewrite"),
            "the declined mutator still has a dispatch row — P-1 has regressed: {table:#}"
        );
        assert_eq!(hooks.len(), 1, "{table:#}");
        assert_eq!(hooks[0]["id"], "watch");
        assert_eq!(hooks[0]["client"], "claude");
        assert_eq!(hooks[0]["event"], "PreToolUse");
    }

    /// **P-3's regression pin: a materialized `hook.toml` that `grim build` would
    /// refuse arms nothing, and does not fail the command.**
    ///
    /// `HookManifest::validate`'s only caller is `grim build`, so a publisher who
    /// pushes with any other OCI client satisfied none of its rules and
    /// `desired_entries` used to copy the manifest into the dispatch table
    /// verbatim. This plants exactly that: a matcher outside `MATCHER_ALLOWED`
    /// (C-018) on one entry, beside a perfectly valid sibling.
    ///
    /// Both are dropped, because the rules are cross-entry and a manifest grim
    /// would not have built is not one to arm half of — and the command still
    /// reports rather than fails (I3).
    ///
    /// Fails against the pre-fix code, which armed both rows.
    #[test]
    fn a_manifest_the_build_rules_reject_arms_nothing_p3() {
        let ws = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let roots = roots_for(home.path(), ws.path());
        let root = RootScope::Workspace(ws.path());

        let mut state = InstallState::empty(&ws.path().join("state.json"));
        state.record(hook_record(home.path(), root, &["claude"]));

        let payload = crate::install::hook_dispatch::payload_dir(home.path(), root, "shell-guard");
        std::fs::write(
            payload.join(HOOK_MANIFEST_FILE),
            "schema = 1\nname = \"shell-guard\"\ndescription = \"a guard\"\n\n\
             [[hooks]]\nid = \"gate\"\nevent = \"PreToolUse\"\ntier = \"gatekeeper\"\n\
             matcher = \"Bash\"\ncommand = \"sh guard.sh\"\n\n\
             [[hooks]]\nid = \"shell\"\nevent = \"PreToolUse\"\ntier = \"observer\"\n\
             matcher = \"Bash$(id)\"\ncommand = \"sh guard.sh\"\n",
        )
        .unwrap();

        let outcomes = converge_clients(&state, ws.path(), ConfigScope::Project, &roots, &arming_policy());
        assert_eq!(
            outcomes,
            vec![(ClientTarget::Claude, HookSync::NoHooks)],
            "the artifact is dropped, and the command still reports rather than fails (I3): {outcomes:?}"
        );

        let table = crate::install::hook_dispatch::dispatch_path(home.path());
        let hooks: Vec<serde_json::Value> = if table.is_file() {
            let parsed: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&table).unwrap()).unwrap();
            parsed["roots"]
                .as_object()
                .unwrap()
                .values()
                .flat_map(|root| root["hooks"].as_array().cloned().unwrap_or_default())
                .collect()
        } else {
            Vec::new()
        };
        assert!(hooks.is_empty(), "an unvalidated manifest still armed: {hooks:?}");
    }

    /// **P-2's arming-seam pin: a reserved binding name arms nothing.**
    ///
    /// The primary control is `installer::install_one`, which refuses before
    /// materialization so nothing is written and nothing can be reaped — that half
    /// is pinned end-to-end by
    /// `test_hook_decline_dispatch.py::test_a_reserved_binding_name_is_refused_before_it_materializes_p2`.
    /// This is the second half: a record that predates that gate (or was
    /// hand-written) names `$GRIM_HOME/hooks/bin` as its payload directory, and
    /// grim must not read an armed manifest out of its own launcher directory.
    ///
    /// The fixture plants a perfectly valid manifest there, so the only thing that
    /// can withhold the arming is the binding name.
    #[test]
    fn a_reserved_binding_name_arms_nothing_p2() {
        for name in crate::oci::hook::RESERVED_ARTIFACT_NAMES {
            let ws = tempfile::tempdir().unwrap();
            let home = tempfile::tempdir().unwrap();
            let roots = roots_for(home.path(), ws.path());
            let root = RootScope::Workspace(ws.path());

            let mut state = InstallState::empty(&ws.path().join("state.json"));
            state.record(hook_record_named(
                home.path(),
                root,
                &["claude"],
                name,
                "localhost:5000",
                "acme/shell-guard",
            ));

            let outcomes = converge_clients(&state, ws.path(), ConfigScope::Project, &roots, &arming_policy());
            assert_eq!(
                outcomes,
                vec![(ClientTarget::Claude, HookSync::NoHooks)],
                "'{name}' is reserved for grim's own $GRIM_HOME/hooks/ namespace and must arm nothing: {outcomes:?}"
            );
        }
    }

    /// A **renamed binding** still arms — the one build rule the install-seam
    /// re-check deliberately drops.
    ///
    /// `HookManifest::validate` rule 7 compares `name` against the artifact
    /// directory's stem. At install the stem is the *binding* name, which the user
    /// chooses (`[hooks] my-guard = "…/shell-guard:1"`), so re-applying that rule
    /// would refuse every renamed binding. Pinned by execution, because the
    /// omission is invisible in the happy path.
    #[test]
    fn the_install_time_recheck_does_not_apply_the_name_equals_stem_rule() {
        let ws = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let roots = roots_for(home.path(), ws.path());
        let root = RootScope::Workspace(ws.path());

        let mut state = InstallState::empty(&ws.path().join("state.json"));
        state.record(hook_record_named(
            home.path(),
            root,
            &["claude"],
            "my-guard",
            "localhost:5000",
            "acme/shell-guard",
        ));
        // `hook_record_named` renames the manifest's own `name` along with the
        // binding, so overwrite it with a manifest whose `name` is the PUBLISHED
        // one — the shape a renamed binding really has on disk.
        let payload = crate::install::hook_dispatch::payload_dir(home.path(), root, "my-guard");
        std::fs::write(
            payload.join(HOOK_MANIFEST_FILE),
            "schema = 1\nname = \"shell-guard\"\ndescription = \"a guard\"\n\n\
             [[hooks]]\nid = \"guard\"\nevent = \"PreToolUse\"\ntier = \"observer\"\n\
             matcher = \"Bash\"\ncommand = \"sh guard.sh\"\n",
        )
        .unwrap();

        let outcomes = converge_clients(&state, ws.path(), ConfigScope::Project, &roots, &arming_policy());
        assert_eq!(
            outcomes,
            vec![(ClientTarget::Claude, HookSync::Armed(1))],
            "a binding renamed away from the published name must still arm: {outcomes:?}"
        );
    }
}
