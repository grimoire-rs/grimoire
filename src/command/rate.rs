// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim rate` — vote on an artifact through the forge the index publishes
//! its ratings from.
//!
//! There is no Grimoire service: the vote is an ordinary upvote on the
//! index operator's own discussion thread (GitHub) or work item (GitLab),
//! cast under the user's **own** forge account. That is what makes the
//! confirmation the default rather than the flag — a single un-flagged
//! command must not post publicly on someone's behalf.
//!
//! ```text
//! grim rate <ref> [--up|--remove] [--yes] [--dry-run]
//!                 [--token-stdin] [--token-host <host>]
//!                 [--registry <ref>] [--format json]
//! ```
//!
//! The credential path is the security core. There is intentionally **no**
//! `--token VALUE` flag — argv lands in world-readable `/proc/<pid>/cmdline`
//! (CWE-214) — so a credential arrives either on stdin (`--token-stdin`,
//! mirroring `grim login --password-stdin`) or from grim's own narrow
//! ladder, which is deliberately **not** the announce token. It is held in
//! `Zeroizing<String>` while it is read, wrapped in a `SecretString`, and
//! exposed exactly once, at the `Authorization` header in
//! [`crate::catalog::forge::graphql`].
//!
//! Two additive flags exist because a client cannot otherwise know where
//! its credential is going (plan C-022). `--dry-run` resolves the row, the
//! provider and the host and prints the report without a credential, a
//! forge request or a mutation — so a caller learns the host *before* it
//! authenticates. `--token-host` lets a caller declare which host its
//! piped credential belongs to; grim compares it to the host it is about
//! to contact and refuses **before the token reaches any header**. The
//! first lets a correct client get it right; the second makes the property
//! hold whether or not the client is correct.
//!
//! `--dry-run` combined with `--token-stdin` adds the one thing a fresh
//! machine cannot otherwise learn (plan C-023): whether *this* account has
//! already voted, as the report's `viewer_up` field. It is a single
//! read-only query — it posts nothing, so it needs no `--yes` — and every
//! way it can fail leaves the field `null` rather than `false`. That
//! asymmetry is invariant R-3: unknown renders neutral, while "not voted"
//! from a query that never answered is a claim grim has no basis for.

use std::io::{IsTerminal as _, Read as _, Write as _};

use clap::Args;
use secrecy::SecretString;
use secrecy::zeroize::Zeroizing;

use crate::api::rate_report::RateReport;
use crate::catalog::forge::{self, CiEnv, ForgeKind};
use crate::catalog::rating_provider::{self, RateContext, RateError, ViewerIdentity, VoteAction, VoteOutcome};
use crate::catalog::registry_catalog::RatingSummary;
use crate::catalog::vote_store::{VoteIdentity, VoteStore};
use crate::cli::exit_code::ExitCode;
use crate::context::Context;

use super::scope_resolution;

/// Where the resolved forge host came from.
///
/// Not cosmetic. `Index` is exactly the case in which an *injected*
/// credential must name its destination with `--token-host`, so a piping
/// client reads this to know whether it has to, and a consent dialog reads
/// it to say the destination was the index's choice rather than grim's
/// default (`adr_index_declared_rating_host.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostSource {
    /// The built-in per-provider default (`api.github.com` / `gitlab.com`).
    Default,
    /// `providers.rating_host`, declared by the index and accepted at
    /// sidecar-parse time (`index_source::accepted_rating_host`).
    Index,
}

impl HostSource {
    /// The wire spelling carried by the report.
    fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Index => "index",
        }
    }
}

/// The environment variable naming grim's own vote credential.
///
/// Distinct from `GRIM_ANNOUNCE_TOKEN` on purpose: voting needs a far
/// narrower grant than publishing, and a user who exported a publish token
/// must not find it cast a public vote.
pub const RATING_TOKEN_ENV: &str = "GRIM_RATE_TOKEN";

/// `grim rate` arguments.
///
/// `--registry` is deliberately absent: the global `--registry` flag
/// (`GlobalOptions::registry`, `global = true`) already applies to every
/// subcommand and is read through [`Context::registry_flags`]. Declaring a
/// second field of that name would share one clap argument id with it (see
/// the note on `LoginArgs::host`).
#[derive(Debug, Args)]
pub struct RateArgs {
    /// The artifact reference to vote on. A short identifier expands
    /// against the resolved default registry; the vote applies to the
    /// repository, so a tag or digest suffix is ignored.
    pub reference: String,

    /// Register an upvote. The default when neither flag is given.
    #[arg(long, conflicts_with = "remove")]
    pub up: bool,

    /// Retract your own upvote. **Not** a downvote — votes are up-only and
    /// binary, and both forges' primitives are toggles.
    #[arg(long)]
    pub remove: bool,

    /// Skip the confirmation prompt. Required for every non-interactive
    /// run: a vote posts publicly under your own forge account, so grim
    /// refuses to cast one nobody confirmed rather than hanging on a
    /// prompt no caller can answer.
    #[arg(long)]
    pub yes: bool,

    /// Resolve the row, the rating provider and the host it would vote
    /// against, print the report, and change nothing. On its own it needs
    /// no credential and makes no forge request, so it works offline; add
    /// `--token-stdin` and it also reports whether you have already voted.
    #[arg(long)]
    pub dry_run: bool,

    /// Read the forge credential from stdin. There is no `--token VALUE`
    /// flag — argv is world-readable. Implies a non-interactive run and
    /// therefore requires `--yes` when it votes; with `--dry-run` nothing
    /// is posted, so no confirmation is needed.
    #[arg(long)]
    pub token_stdin: bool,

    /// Declare which host the piped credential belongs to. grim refuses
    /// before sending anything when it does not match the host the vote
    /// resolves to. Valid only with `--token-stdin`.
    #[arg(long, value_name = "HOST")]
    pub token_host: Option<String>,
}

/// Whether the run has to ask before it votes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmMode {
    /// `--yes` was given: vote without asking.
    Skip,
    /// Interactive run without `--yes`: prompt and honour the answer.
    Prompt,
}

/// Run `grim rate`.
///
/// # Errors
///
/// A [`RateError`] for every contracted failure — see the exit-code table
/// in `docs/src/commands.md` — plus a config-tier error (78) when a
/// project config exists and fails to parse.
pub async fn run(ctx: &Context, args: &RateArgs) -> anyhow::Result<(RateReport, ExitCode)> {
    // Every check that needs neither the catalog nor the network runs
    // first, so a contradictory invocation fails identically everywhere.
    super::grim(usage_gate(args))?;
    let action = action_of(args);
    // A bare dry run resolves and reports without a request, which is
    // exactly what an offline client needs (S-020); a vote — and a dry run
    // that also reads viewer state — hard-refuses rather than degrading
    // silently into an answer it could not check.
    if ctx.offline() && wants_network(args) {
        return Err(anyhow::Error::from(crate::error::Error::from(RateError::Offline)));
    }

    let (reference, rating) = resolve_row(ctx, &args.reference).await?;
    // The index may name the host it rates on, and grim takes it — the
    // reversal of C-007's "never from index-fetched content", argued in
    // `adr_index_declared_rating_host.md`. It was already validated and
    // normalised when the sidecar was parsed; what makes taking it safe is
    // the gate below, not a check here.
    let host = rating
        .provider
        .as_deref()
        .and_then(|p| rating_provider::resolve_host(p, rating.host.as_deref()));
    let host_source = if rating.host.is_some() {
        HostSource::Index
    } else {
        HostSource::Default
    };

    if args.dry_run {
        // Still before any credential is read, exactly as on the voting
        // path: a mismatching declaration is refused with nothing to leak
        // (C-022 / C-023). No host resolved means no request either, so
        // there is nothing for the gate to protect.
        if let Some(host) = host.as_deref() {
            super::grim(token_host_gate(args.token_host.as_deref(), host))?;
            // `--token-stdin` alone, never `injects_credential`: this
            // path reads the piped credential and nothing else
            // (`viewer_state` calls `resolve_token(true, ..)`), so
            // `GRIM_RATE_TOKEN` is not in play here. Gating on it would
            // also break the handshake outright — a bare `--dry-run` is
            // how a client *learns* the host it would have to declare.
            super::grim(declared_host_gate(
                args.token_stdin,
                args.token_host.as_deref(),
                host_source,
                host,
            ))?;
        }
        // Without a piped credential this is resolution only: no
        // credential, no forge request, no mutation (C-022). With one, it
        // is that plus a single read-only query (C-023).
        let viewer_up = match (args.token_stdin, host.as_deref(), rating.provider.as_deref()) {
            (true, Some(host), Some(provider)) => viewer_state(host, provider, &rating.target).await?,
            _ => None,
        };
        return Ok((
            build_report(
                reference,
                action,
                Some(rating.up),
                &rating,
                host,
                host_source,
                viewer_up,
            ),
            ExitCode::Success,
        ));
    }

    let provider = rating
        .provider
        .as_deref()
        .ok_or_else(|| RateError::NoProvider {
            reference: reference.clone(),
        })
        .map_err(map_rate)?;
    let (host, kind) = match (host, rating_provider::forge_kind(provider)) {
        (Some(host), Some(kind)) => (host, kind),
        _ => return Err(map_rate(RateError::UnsupportedProvider(provider.to_string()))),
    };

    // Before the credential is read, not after: a mismatching caller is
    // refused with nothing to leak (plan C-022 / S-021), and an injected
    // credential heading for an index-declared host is refused unless the
    // caller named that host.
    super::grim(token_host_gate(args.token_host.as_deref(), &host))?;
    super::grim(declared_host_gate(
        injects_credential(args),
        args.token_host.as_deref(),
        host_source,
        &host,
    ))?;

    let confirm = super::grim(confirmation_mode(args.yes, std::io::stdin().is_terminal()))?;
    let token = resolve_token(args.token_stdin, &host, kind).await?;
    let rate_ctx = RateContext::new(kind, rating_provider::graphql_endpoint(&host), token);
    let http = super::grim(
        forge::client().map_err(|e| RateError::Unavailable(format!("could not build an HTTP client: {e}"))),
    )?;

    // Kept when the prompt already resolved it, so the confirmed path
    // never asks the forge who the user is twice.
    let mut viewer: Option<ViewerIdentity> = None;
    if confirm == ConfirmMode::Prompt {
        let identity = super::grim(rating_provider::viewer_identity(&http, &rate_ctx).await)?;
        if !prompt_for_confirmation(&confirm_prompt(provider, &identity.login, host_source, &host))? {
            // Declined: nothing was mutated, so the report is the row as
            // it stands and the exit stays 0. `viewer_up` is null — the
            // vote-state question was never asked on this path.
            return Ok((
                build_report(
                    reference,
                    action,
                    Some(rating.up),
                    &rating,
                    Some(host),
                    host_source,
                    None,
                ),
                ExitCode::Success,
            ));
        }
        viewer = Some(identity);
    }

    let outcome = super::grim(dispatch_vote(&http, provider, &rate_ctx, &rating.target, action).await)?;
    // Only reachable once the mutation actually landed — every earlier
    // failure has already returned through `?`, which is what makes "a
    // timeout leaves the entry untouched" true by construction rather
    // than by a branch that could be forgotten (plan C-008 rule 1).
    record_local_vote(ctx, &http, &rate_ctx, provider, &reference, viewer, outcome.voted).await;
    Ok((
        // The forge just told us this account's state, so report it: the
        // field means "what the forge last said about this account", and a
        // mutation is the freshest possible answer. Reporting null here
        // would make `grim rate --up --format json` claim it does not know
        // the thing it just did.
        build_report(
            reference,
            action,
            outcome.up,
            &rating,
            Some(host),
            host_source,
            outcome.voted,
        ),
        ExitCode::Success,
    ))
}

/// Fold the mutation's own report into `$GRIM_HOME/state/votes.json`.
///
/// Best-effort by construction, and deliberately infallible: the vote is
/// already public by the time this runs, so nothing here may fail the
/// command or change its exit code. A forge that will not name the
/// account, and a file that will not be written, both leave the record
/// alone — which reads back as **unknown**, the honest answer under
/// invariant R-3, never "not voted".
async fn record_local_vote(
    ctx: &Context,
    http: &reqwest::Client,
    rate_ctx: &RateContext,
    provider: &str,
    reference: &str,
    viewer: Option<ViewerIdentity>,
    voted: Option<bool>,
) {
    // The forge reported no vote state — nothing to cache. Checked before
    // the identity query so a silent forge costs no round trip.
    if voted.is_none() {
        return;
    }
    let viewer = match viewer {
        Some(viewer) => viewer,
        // Only the `--yes` path arrives here, and only after a vote
        // landed: at most one extra query per successful vote.
        None => match rating_provider::viewer_identity(http, rate_ctx).await {
            Ok(viewer) => viewer,
            Err(err) => {
                tracing::debug!("leaving the local vote record unknown; the forge named no account: {err}");
                return;
            }
        },
    };
    let store = VoteStore::at(ctx.paths().votes_file());
    let identity = VoteIdentity::new(provider, viewer.account_id);
    if let Err(err) = store.record(&identity, reference, voted, chrono::Utc::now()) {
        tracing::debug!("leaving the local vote record unknown; it could not be written: {err}");
    }
}

/// Route a [`RateError`] through the top-level error so the classifier
/// sees it (a bare `anyhow` conversion would bypass the exit-code map).
fn map_rate(err: RateError) -> anyhow::Error {
    anyhow::Error::from(crate::error::Error::from(err))
}

/// The action the flags select. `--up` is the default; clap already
/// refuses the two together.
fn action_of(args: &RateArgs) -> VoteAction {
    if args.remove {
        VoteAction::Remove
    } else {
        VoteAction::Up
    }
}

/// Compare a `--token-host` declaration against the host the vote resolves
/// to, with C-007's exact rules.
///
/// Runs on the resolved host **alone**, before any credential is read, so
/// a mismatching caller is refused with nothing to leak. An absent
/// declaration is not a mismatch — a client that pipes a credential
/// without declaring its host still works, it simply does not get the
/// second check.
fn token_host_gate(declared: Option<&str>, resolved: &str) -> Result<(), RateError> {
    match declared {
        Some(declared) if !rating_provider::hosts_equal(declared, resolved) => Err(RateError::TokenHostMismatch {
            declared: declared.to_string(),
            resolved: resolved.to_string(),
        }),
        _ => Ok(()),
    }
}

/// Refuse an **injected** credential heading for an **index-declared**
/// host unless the caller named that host with `--token-host`.
///
/// The rest of the ladder needs no gate: a CI token and a `gh`/`glab`
/// stored credential are looked up *for the host grim is about to
/// contact*, so an index naming a host the user never authenticated
/// against simply resolves nothing (`adr_artifact_ratings.md` D13). The
/// two that bypass that are `GRIM_RATE_TOKEN`, which is host-agnostic by
/// construction, and `--token-stdin`, where the caller supplies a
/// credential grim never looked up — so those two, and only those two,
/// have to say where their credential belongs.
///
/// It runs before `resolve_token`, so a refusal happens with nothing read.
/// It never falls through to the host-matched rungs either: voting as a
/// different identity than the caller supplied is the same failure C-006
/// refuses for empty `--token-stdin`.
fn declared_host_gate(
    injected: bool,
    declared: Option<&str>,
    source: HostSource,
    resolved: &str,
) -> Result<(), RateError> {
    if source == HostSource::Index && injected && declared.is_none() {
        return Err(RateError::UndeclaredTokenHost {
            host: resolved.to_string(),
        });
    }
    Ok(())
}

/// Whether this run supplies a credential grim did not look up itself.
///
/// The environment is read for **presence only**, never for the value —
/// what matters is that a host-agnostic credential is in play, not what it
/// says.
fn injects_credential(args: &RateArgs) -> bool {
    args.token_stdin || std::env::var_os(RATING_TOKEN_ENV).is_some()
}

/// Assemble the report from what was actually resolved.
fn build_report(
    reference: String,
    action: VoteAction,
    up: Option<u32>,
    rating: &RatingSummary,
    host: Option<String>,
    host_source: HostSource,
    viewer_up: Option<bool>,
) -> RateReport {
    // No host resolved means nothing was chosen, by the index or by grim —
    // reporting a source there would name a decision that never happened.
    let host_source = host.as_ref().map(|_| host_source.as_str().to_string());
    RateReport::new(
        reference,
        action.as_str().to_string(),
        up,
        Some(rating.url.clone()),
        rating.provider.clone(),
        host,
        host_source,
        viewer_up,
    )
}

/// Whether this invocation will contact the forge.
///
/// A bare `--dry-run` answers from the catalog alone, which is what lets a
/// client learn the host *before* it authenticates and what makes that
/// path offline-safe (C-022). Adding `--token-stdin` asks the forge for
/// the viewer's own vote state (C-023) — an ordinary request, and one that
/// `--offline` must refuse rather than answer from nothing.
fn wants_network(args: &RateArgs) -> bool {
    !args.dry_run || args.token_stdin
}

/// Ask the forge whether this credential's account has already voted, for
/// `--dry-run --token-stdin` (plan C-023).
///
/// **Read-only and unconfirmed by design.** Nothing is posted, so there is
/// nothing for `--yes` to confirm; the credential is still read through
/// C-006's path, so an empty, multi-line or terminal stdin is refused here
/// exactly as it is before a vote.
///
/// Every *query* failure — unreachable, unauthorised, a populated GraphQL
/// `errors` array, a payload this cannot read — becomes `None`. A query
/// that did not answer cannot prove the user has not voted, and reporting
/// one as the other is the lie invariant R-3 exists to prevent.
///
/// # Errors
///
/// The credential-shaped refusals from [`resolve_token`] (64/80), and a
/// client that will not build (69). Those are not "the forge said no" —
/// they are the caller's own invocation being wrong.
async fn viewer_state(host: &str, provider: &str, target: &str) -> anyhow::Result<Option<bool>> {
    // `forge_kind` is total over the providers `resolve_host` answers for,
    // so a resolved host implies a kind; the refusal is kept rather than
    // unwrapped so the pairing stays enforced rather than assumed.
    let kind = rating_provider::forge_kind(provider)
        .ok_or_else(|| map_rate(RateError::UnsupportedProvider(provider.to_string())))?;
    let token = resolve_token(true, host, kind).await?;
    let rate_ctx = RateContext::new(kind, rating_provider::graphql_endpoint(host), token);
    let http = super::grim(
        forge::client().map_err(|e| RateError::Unavailable(format!("could not build an HTTP client: {e}"))),
    )?;
    match rating_provider::viewer_upvoted(&http, &rate_ctx, target).await {
        Ok(state) => Ok(state),
        Err(err) => {
            tracing::debug!("leaving the viewer's vote state unknown; the forge did not answer: {err}");
            Ok(None)
        }
    }
}

/// Flag-combination checks that need no catalog, no network and no
/// credential, run before anything else so a contradictory invocation
/// fails fast and identically in every environment.
fn usage_gate(args: &RateArgs) -> Result<(), RateError> {
    // `--token-host` used to require `--token-stdin`. It no longer can:
    // `GRIM_RATE_TOKEN` is injected too, and it needs the same way to
    // declare where it may go, so the flag stands on its own. Declaring a
    // host without supplying a credential is harmless — the declaration is
    // still compared to the resolved host, so a wrong one is refused
    // rather than ignored.
    // Narrowed by C-023, and only for `--dry-run`: the confirmation exists
    // because a vote posts publicly under the user's account, and a dry
    // run posts nothing at all, so there is nothing to confirm. The rule
    // stands unchanged on the voting path — getting this branch wrong in
    // the other direction sends a vote out unconfirmed.
    if args.token_stdin && !args.yes && !args.dry_run {
        return Err(RateError::Usage(
            "--token-stdin implies a non-interactive run: stdin carries the credential, so it cannot also carry a \
             confirmation — pass --yes"
                .to_string(),
        ));
    }
    Ok(())
}

/// Whether a run must prompt before voting.
///
/// A non-interactive run without `--yes` is a **usage error**, never a
/// prompt: there is nobody to answer it, and the alternative — voting
/// unconfirmed — is exactly what the default confirmation exists to
/// prevent.
fn confirmation_mode(yes: bool, stdin_is_tty: bool) -> Result<ConfirmMode, RateError> {
    match (yes, stdin_is_tty) {
        (true, _) => Ok(ConfirmMode::Skip),
        (false, true) => Ok(ConfirmMode::Prompt),
        (false, false) => Err(RateError::Usage(
            "a vote posts publicly under your own forge account; a non-interactive run must confirm it with --yes"
                .to_string(),
        )),
    }
}

/// The confirmation line. It names the provider and the account because
/// what the user is consenting to is a public post under that identity —
/// and the host too when the index chose it, because a destination the
/// user never configured is the one thing they cannot infer from the
/// provider name alone.
fn confirm_prompt(provider: &str, login: &str, source: HostSource, host: &str) -> String {
    match source {
        HostSource::Default => format!("This posts publicly to your {provider} account as {login}. Continue? [y/N] "),
        HostSource::Index => format!(
            "This posts publicly to your {provider} account as {login} on {host}, the host the index declared. \
             Continue? [y/N] "
        ),
    }
}

/// Whether a typed answer means yes. Anything else — including empty
/// input, EOF and a stray word — means no.
fn is_affirmative(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Interpret the raw bytes read from stdin under `--token-stdin`.
///
/// Separated from the read itself so every contracted edge is testable
/// without a pipe: one trailing newline is stripped, anything beyond a
/// single line is a usage error, and empty or whitespace-only input is an
/// auth error that **never falls through to the standalone ladder** — a
/// caller that stated it was supplying a credential must not silently vote
/// as somebody else.
fn read_piped_token(raw: &str) -> Result<SecretString, RateError> {
    // Strip a single trailing newline (the common `… | grim rate` case);
    // the credential is otherwise taken verbatim.
    let token = raw.strip_suffix('\n').unwrap_or(raw);
    let token = token.strip_suffix('\r').unwrap_or(token);
    if token.trim().is_empty() {
        return Err(RateError::NoCredential(
            "--token-stdin received empty input; refusing to fall back to another credential".to_string(),
        ));
    }
    if token.contains('\n') {
        // No credential is echoed: the message describes the shape of the
        // input, never its content.
        return Err(RateError::Usage(
            "--token-stdin expects a single line; the input carried more than one".to_string(),
        ));
    }
    Ok(SecretString::from(token.to_string()))
}

/// The environment tiers of the standalone credential ladder: grim's own
/// `GRIM_RATE_TOKEN`, then a **host-matched** CI token.
///
/// Host matching is what stops a GitLab pipeline's token from being sent
/// to a GitHub host; it is the announce ladder's own rule, reused.
fn env_token(host: &str, kind: ForgeKind, explicit: Option<String>, ci: &CiEnv) -> Option<Zeroizing<String>> {
    explicit
        .filter(|v| !v.trim().is_empty())
        // The CI tiers are matched on the *site* host: `GITHUB_SERVER_URL`
        // names github.com, while the vote endpoint is api.github.com.
        .or_else(|| forge::ci_token_for(rating_provider::site_host(host), kind, ci))
        .filter(|v| !v.trim().is_empty())
        .map(Zeroizing::new)
}

/// The forge CLI tier of the ladder: `gh auth token` / `glab auth token`
/// for the host the vote resolves to.
///
/// Spawned without a shell and with the hostname passed as an argument, so
/// nothing here is interpolated into a command line. Any failure — the CLI
/// absent, not logged in, or logged in elsewhere — is not an error, it is
/// simply the next rung.
async fn cli_token(host: &str, kind: ForgeKind) -> Option<Zeroizing<String>> {
    let program = match kind {
        ForgeKind::GitHub => "gh",
        ForgeKind::GitLab => "glab",
        // Never reached: `forge_kind` does not produce `Plain`.
        ForgeKind::Plain => return None,
    };
    let output = tokio::process::Command::new(program)
        .args(["auth", "token", "--hostname", rating_provider::site_host(host)])
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let token = Zeroizing::new(String::from_utf8(output.stdout).ok()?);
    let trimmed = Zeroizing::new(token.trim().to_string());
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Resolve the catalog row `reference` names, and the rating on it.
///
/// Resolution goes through the same shared catalog seam `grim search`
/// uses, under [`crate::catalog::CatalogScope::Complete`]: a browse filter
/// hiding a row from a *listing* must not make it unvotable.
///
/// # Errors
///
/// [`RateError::MalformedRef`] (64), [`RateError::NoSuchRef`] (79) or
/// [`RateError::NotRated`] (65); a config-tier error (78) propagates.
async fn resolve_row(ctx: &Context, reference: &str) -> anyhow::Result<(String, RatingSummary)> {
    let (registries, primary) = match scope_resolution::resolve_in(ctx, ctx.global(), ctx.config(), None) {
        Ok(scope) => {
            let registries = super::registries_for_scope(ctx, &scope)?;
            let primary = crate::config::registry_resolve::primary_registry(&registries).to_string();
            (registries, primary)
        }
        // No config to resolve: browse the flag/env/global fallback chain,
        // the same seam `search` takes on this path.
        Err(e) if scope_resolution::config_not_found(&e) => (
            super::registries_global_fallback(ctx)?,
            super::primary_registry_global_fallback(ctx)?,
        ),
        // A config that EXISTS and failed to parse is a different
        // condition: voting against a registry set grim could not assemble
        // would hide the very file the user has to fix.
        Err(e) => return Err(anyhow::Error::from(crate::error::Error::from(e))),
    };

    // The vote is per repository, so a tag or digest suffix is parsed and
    // then dropped — `stats.json` is keyed by `registry/repository`.
    let wanted = crate::oci::identifier::Identifier::parse_with_default_registry(reference, &primary)
        .map_err(|e| {
            map_rate(RateError::MalformedRef {
                reference: reference.to_string(),
                reason: e.to_string(),
            })
        })?
        .registry_repository();

    let access = super::access_seam(ctx)?;
    let roots = crate::install::path_anchor::AnchorRoots::resolve(std::path::PathBuf::new(), ctx);
    let state = crate::install::install_state::InstallState::empty(std::path::Path::new(""));
    let badges = crate::catalog::BadgeContext {
        lock: None,
        state: &state,
        roots: &roots,
        active: &crate::install::client_target::ClientTarget::ALL,
        target: None,
    };
    let results = super::grim(
        crate::catalog::load_catalog(
            &ctx.paths(),
            &registries,
            "",
            &access,
            &badges,
            ctx.offline(),
            false,
            // `Complete`, never `Browse`: a source's include/exclude filter
            // decides what is *listed*, and hiding a row from a listing
            // must not make an artifact unvotable.
            crate::catalog::CatalogScope::Complete,
        )
        .await,
    )?;

    let row = results
        .into_flat_rows()
        .into_iter()
        .find(|row| row.repo() == wanted)
        .ok_or_else(|| {
            map_rate(RateError::NoSuchRef {
                reference: wanted.clone(),
            })
        })?;
    let rating = row.rating.ok_or_else(|| {
        map_rate(RateError::NotRated {
            reference: wanted.clone(),
        })
    })?;
    Ok((wanted, rating))
}

/// Resolve the credential for `host`: the piped one under
/// `--token-stdin`, otherwise the standalone ladder.
///
/// The ladder is grim's own `GRIM_RATE_TOKEN`, then a host-matched CI
/// token, then the forge CLI's stored credential, then a refusal. The
/// **device-flow rung specified in the ADR is not implemented** — it needs
/// a registered OAuth client id the project deliberately does not have
/// (the ADR's own security section rules out registering an OAuth app) —
/// so the ladder ends one rung earlier, at the read-only refusal.
///
/// # Errors
///
/// [`RateError::NoCredential`] (80) when nothing resolves, plus the
/// `--token-stdin` edge cases from [`read_piped_token`] (64/80).
async fn resolve_token(token_stdin: bool, host: &str, kind: ForgeKind) -> anyhow::Result<SecretString> {
    if token_stdin {
        if std::io::stdin().is_terminal() {
            // The flag exists for piping; prompting here would invite a
            // paste into terminal scrollback.
            return Err(map_rate(RateError::Usage(
                "--token-stdin expects a piped credential, but stdin is a terminal".to_string(),
            )));
        }
        // Zeroized so the cleartext credential does not linger on the heap
        // after it is wrapped in a `SecretString`.
        let mut buf = Zeroizing::new(String::new());
        std::io::stdin().read_to_string(&mut buf).map_err(|e| {
            map_rate(RateError::NoCredential(format!(
                "could not read the credential from stdin: {e}"
            )))
        })?;
        return read_piped_token(&buf).map_err(map_rate);
    }

    let explicit = std::env::var(RATING_TOKEN_ENV).ok();
    let ci = CiEnv::from_env();
    let resolved = match env_token(host, kind, explicit, &ci) {
        Some(token) => Some(token),
        None => cli_token(host, kind).await,
    };
    let token = resolved.ok_or_else(|| {
        let cli = match kind {
            ForgeKind::GitHub => "gh auth login",
            ForgeKind::GitLab => "glab auth login",
            ForgeKind::Plain => "the forge CLI",
        };
        // Names the ladder, never a token.
        map_rate(RateError::NoCredential(format!(
            "no credential for '{host}': set {RATING_TOKEN_ENV}, run in a host-matched CI environment, or sign in with `{cli}`"
        )))
    })?;
    Ok(SecretString::from(token.to_string()))
}

/// Write the confirmation to stderr and read the answer from stdin.
///
/// stderr, not stdout: stdout stays a clean machine interface carrying
/// only the report (the same split `auth::prompt` uses). The prompt is
/// **never** rerouted to `/dev/tty` — a caller that cannot answer on stdin
/// is a program, and programs confirm with `--yes`.
///
/// # Errors
///
/// An I/O failure writing the prompt or reading the answer.
fn prompt_for_confirmation(prompt: &str) -> anyhow::Result<bool> {
    let mut stderr = std::io::stderr();
    write!(stderr, "{prompt}")?;
    stderr.flush()?;
    let mut answer = String::new();
    // A read failure (EOF included) leaves `answer` empty, which is a
    // decline — the safe direction.
    std::io::stdin().read_line(&mut answer)?;
    Ok(is_affirmative(&answer))
}

/// Cast the vote through the provider the index declared.
///
/// The single dispatch site, and deliberately a `match` on a string rather
/// than a trait (ADR D10): both implementations are known at compile time
/// and never vary at runtime, so a vtable would buy nothing.
async fn dispatch_vote(
    http: &reqwest::Client,
    provider: &str,
    rate_ctx: &RateContext,
    target: &str,
    action: VoteAction,
) -> Result<VoteOutcome, RateError> {
    match provider {
        "github" => rating_provider::github_vote(http, rate_ctx, target, action).await,
        "gitlab" => rating_provider::gitlab_vote(http, rate_ctx, target, action).await,
        other => Err(RateError::UnsupportedProvider(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    /// Helper harness so clap can parse the subcommand args in isolation.
    #[derive(clap::Parser)]
    struct Harness {
        #[command(subcommand)]
        cmd: Sub,
    }

    #[derive(clap::Subcommand)]
    enum Sub {
        Rate(RateArgs),
    }

    fn parse(args: &[&str]) -> Result<RateArgs, clap::Error> {
        let mut argv = vec!["grim", "rate"];
        argv.extend_from_slice(args);
        Harness::try_parse_from(argv).map(|h| match h.cmd {
            Sub::Rate(a) => a,
        })
    }

    fn parsed(args: &[&str]) -> RateArgs {
        parse(args).expect("parses")
    }

    fn exit_of(err: RateError) -> ExitCode {
        crate::error::classify_error(&anyhow::Error::from(crate::error::Error::from(err)))
    }

    fn exit_of_ref(err: &RateError) -> ExitCode {
        match err {
            RateError::MalformedRef { .. } | RateError::Usage(_) => ExitCode::UsageError,
            RateError::NotRated { .. }
            | RateError::NoProvider { .. }
            | RateError::UnsupportedProvider(_)
            | RateError::Graphql(_) => ExitCode::DataError,
            RateError::Unavailable(_) => ExitCode::Unavailable,
            RateError::NoSuchRef { .. } | RateError::NotFound(_) => ExitCode::NotFound,
            RateError::TokenHostMismatch { .. }
            | RateError::UndeclaredTokenHost { .. }
            | RateError::NoCredential(_)
            | RateError::Unauthorized(_) => ExitCode::AuthError,
            RateError::Offline => ExitCode::OfflineBlocked,
        }
    }

    // -----------------------------------------------------------------
    // C-005 — the surface and its exit codes
    // -----------------------------------------------------------------

    #[test]
    fn up_and_remove_together_is_a_usage_error() {
        // C-005 / S-009: clap refuses the combination, and `main.rs` maps
        // every parse failure to EX_USAGE (64).
        assert!(parse(&["ghcr.io/acme/x", "--up", "--remove"]).is_err());
    }

    #[test]
    fn up_is_the_default_action() {
        let bare = parsed(&["ghcr.io/acme/x"]);
        assert!(!bare.up && !bare.remove);
        assert_eq!(action_of(&bare), VoteAction::Up);
        assert_eq!(action_of(&parsed(&["ghcr.io/acme/x", "--up"])), VoteAction::Up);
        assert_eq!(action_of(&parsed(&["ghcr.io/acme/x", "--remove"])), VoteAction::Remove);
    }

    #[test]
    fn the_full_flag_set_parses() {
        let args = parsed(&[
            "ghcr.io/acme/x",
            "--remove",
            "--yes",
            "--token-stdin",
            "--token-host",
            "ghes.corp.example",
        ]);
        assert!(args.remove && args.yes && args.token_stdin);
        assert_eq!(args.token_host.as_deref(), Some("ghes.corp.example"));
    }

    /// C-006 (CWE-214): there is no `--token VALUE`, so a secret can never
    /// reach `/proc/<pid>/cmdline`. clap must reject the flag outright.
    #[test]
    fn rejects_a_token_value_flag() {
        assert!(parse(&["ghcr.io/acme/x", "--token", "ghp_secret"]).is_err());
    }

    /// The `--registry` in the documented surface is the **global** flag.
    /// Declaring a second field of that name here would share one clap
    /// argument id with it and poison the browse set (see `LoginArgs::host`).
    #[test]
    fn registry_comes_from_the_global_flag_not_a_local_one() {
        #[derive(clap::Parser)]
        struct FullHarness {
            #[command(flatten)]
            global: crate::cli::options::GlobalOptions,
            #[command(subcommand)]
            cmd: Sub,
        }
        let cli = FullHarness::try_parse_from(["grim", "rate", "x", "--registry", "hub"]).expect("parses");
        assert_eq!(cli.global.registry, vec!["hub".to_string()]);
    }

    #[test]
    fn every_error_maps_to_its_contracted_exit_code() {
        // The exit-code table in C-005, asserted literally rather than
        // derived — a re-classification has to be a deliberate edit here.
        let r = || "ghcr.io/acme/x".to_string();
        assert_eq!(
            exit_of(RateError::MalformedRef {
                reference: r(),
                reason: "uppercase".to_string()
            }),
            ExitCode::UsageError
        );
        assert_eq!(exit_of(RateError::Usage("--yes".to_string())), ExitCode::UsageError);
        assert_eq!(exit_of(RateError::NotRated { reference: r() }), ExitCode::DataError);
        assert_eq!(exit_of(RateError::NoProvider { reference: r() }), ExitCode::DataError);
        assert_eq!(
            exit_of(RateError::UnsupportedProvider("bitbucket".to_string())),
            ExitCode::DataError
        );
        assert_eq!(exit_of(RateError::Graphql("errors".to_string())), ExitCode::DataError);
        assert_eq!(
            exit_of(RateError::Unavailable("5xx".to_string())),
            ExitCode::Unavailable
        );
        assert_eq!(exit_of(RateError::NoSuchRef { reference: r() }), ExitCode::NotFound);
        assert_eq!(exit_of(RateError::NotFound("gone".to_string())), ExitCode::NotFound);
        assert_eq!(
            exit_of(RateError::TokenHostMismatch {
                declared: "api.github.com".to_string(),
                resolved: "ghes.corp.example".to_string()
            }),
            ExitCode::AuthError
        );
        assert_eq!(
            exit_of(RateError::NoCredential("ladder".to_string())),
            ExitCode::AuthError
        );
        assert_eq!(exit_of(RateError::Unauthorized("401".to_string())), ExitCode::AuthError);
        assert_eq!(exit_of(RateError::Offline), ExitCode::OfflineBlocked);
    }

    // -----------------------------------------------------------------
    // C-022 — the two host flags
    // -----------------------------------------------------------------

    #[test]
    fn token_host_stands_on_its_own() {
        // It used to require `--token-stdin`. `GRIM_RATE_TOKEN` is injected
        // too and needs the same way to declare its destination, so the
        // flag no longer depends on a sibling. Declaring without supplying
        // a credential stays harmless: the declaration is still compared to
        // the resolved host, so a wrong one is refused rather than ignored.
        usage_gate(&parsed(&["x", "--token-host", "github.com"])).expect("accepted on its own");
        assert!(
            token_host_gate(Some("evil.tld"), "api.github.com").is_err(),
            "and it still gates"
        );
    }

    /// The whole decision table for
    /// `adr_index_declared_rating_host.md`'s credential-class gate: an
    /// injected credential may reach an index-declared host only when the
    /// caller named it, and nothing else changes.
    #[test]
    fn an_injected_credential_must_name_an_index_declared_host() {
        let host = "gitlab.corp.example";
        for (injected, declared, source, want_refusal) in [
            // Index-declared destination.
            (true, None, HostSource::Index, true),
            (true, Some(host), HostSource::Index, false),
            (false, None, HostSource::Index, false),
            // The built-in default is unchanged in every combination.
            (true, None, HostSource::Default, false),
            (true, Some(host), HostSource::Default, false),
            (false, None, HostSource::Default, false),
        ] {
            let got = declared_host_gate(injected, declared, source, host);
            assert_eq!(
                got.is_err(),
                want_refusal,
                "injected={injected} declared={declared:?} source={source:?}"
            );
            if let Err(err) = got {
                assert_eq!(exit_of_ref(&err), ExitCode::AuthError);
                assert!(err.to_string().contains("--token-host"), "names the way out: {err}");
                assert!(err.to_string().contains(host), "names the host: {err}");
            }
        }
    }

    #[test]
    fn token_stdin_without_yes_is_a_usage_error() {
        // S-019: stdin carries the credential, so it cannot also carry a
        // `y`, and the prompt is never rerouted to /dev/tty.
        let err = usage_gate(&parsed(&["x", "--token-stdin"])).expect_err("must refuse");
        assert_eq!(exit_of_ref(&err), ExitCode::UsageError);
        assert!(err.to_string().contains("--yes"), "names the flag: {err}");
        // …and it stays a refusal for every voting invocation, so the
        // C-023 exemption below cannot be widened by accident.
        for voting in [
            vec!["x", "--token-stdin"],
            vec!["x", "--token-stdin", "--up"],
            vec!["x", "--token-stdin", "--remove"],
            vec!["x", "--token-stdin", "--token-host", "api.github.com"],
        ] {
            assert!(
                usage_gate(&parsed(&voting)).is_err(),
                "a vote must still confirm: {voting:?}"
            );
        }
    }

    /// C-023: `--dry-run` posts nothing, so there is nothing for `--yes` to
    /// confirm — the C-005 rule is narrowed here and **only** here.
    #[test]
    fn a_dry_run_with_a_piped_credential_needs_no_yes() {
        usage_gate(&parsed(&["x", "--dry-run", "--token-stdin"])).expect("a read confirms nothing");
        usage_gate(&parsed(&[
            "x",
            "--dry-run",
            "--token-stdin",
            "--token-host",
            "api.github.com",
        ]))
        .expect("declaring the host is still allowed");
    }

    /// C-022 / C-023: the bare dry run answers from the catalog alone and
    /// must stay offline-safe; adding `--token-stdin` asks the forge a
    /// question, which `--offline` refuses rather than invents.
    #[test]
    fn only_a_bare_dry_run_avoids_the_network() {
        assert!(!wants_network(&parsed(&["x", "--dry-run"])));
        assert!(wants_network(&parsed(&["x", "--dry-run", "--token-stdin"])));
        assert!(wants_network(&parsed(&["x", "--yes"])));
        assert!(wants_network(&parsed(&["x", "--yes", "--token-stdin"])));
    }

    #[test]
    fn a_valid_flag_combination_passes_the_usage_gate() {
        usage_gate(&parsed(&["x"])).expect("bare invocation");
        usage_gate(&parsed(&["x", "--dry-run"])).expect("dry run needs no confirmation flag");
        usage_gate(&parsed(&["x", "--dry-run", "--token-stdin"])).expect("a credentialed dry run reads, never posts");
        usage_gate(&parsed(&["x", "--token-stdin", "--yes"])).expect("piped credential with --yes");
        usage_gate(&parsed(&["x", "--token-stdin", "--yes", "--token-host", "github.com"]))
            .expect("declared host with a piped credential");
    }

    /// S-021: the declared host is compared with C-007's exact rules, and
    /// the refusal happens before a credential is ever read — this gate
    /// runs against the resolved host alone.
    #[test]
    fn token_host_must_match_the_resolved_host_exactly() {
        assert!(token_host_gate(Some("api.github.com"), "api.github.com").is_ok());
        assert!(token_host_gate(Some("API.GitHub.com"), "api.github.com").is_ok());
        assert!(
            token_host_gate(None, "api.github.com").is_ok(),
            "absent is not a mismatch"
        );

        for hostile in [
            "github.com",
            "evil-github.com",
            "api.github.com.evil.tld",
            "api.github.com:8443",
        ] {
            let err = token_host_gate(Some(hostile), "api.github.com").expect_err("must refuse");
            assert_eq!(exit_of_ref(&err), ExitCode::AuthError);
            let message = err.to_string();
            assert!(message.contains(hostile), "names the declared host: {message}");
            assert!(message.contains("api.github.com"), "names the resolved host: {message}");
        }
    }

    // -----------------------------------------------------------------
    // Confirmation — S-017 / S-018 / S-019
    // -----------------------------------------------------------------

    #[test]
    fn an_interactive_run_without_yes_prompts() {
        // S-017.
        assert_eq!(confirmation_mode(false, true).expect("prompts"), ConfirmMode::Prompt);
    }

    #[test]
    fn a_non_interactive_run_without_yes_refuses_rather_than_hanging() {
        // S-018: never a hang, never an unconfirmed vote.
        let err = confirmation_mode(false, false).expect_err("must refuse");
        assert_eq!(exit_of_ref(&err), ExitCode::UsageError);
        assert!(err.to_string().contains("--yes"), "names the flag: {err}");
    }

    #[test]
    fn yes_skips_the_prompt_in_both_modes() {
        assert_eq!(confirmation_mode(true, true).expect("skips"), ConfirmMode::Skip);
        assert_eq!(confirmation_mode(true, false).expect("skips"), ConfirmMode::Skip);
    }

    #[test]
    fn the_prompt_names_the_provider_and_the_account() {
        let line = confirm_prompt("github", "octocat", HostSource::Default, "api.github.com");
        assert!(line.contains("github"), "{line}");
        assert!(line.contains("octocat"), "{line}");
        assert!(line.contains("publicly"), "the disclosure is the point: {line}");
        assert!(line.contains("[y/N]"), "the default is no: {line}");
        assert!(
            !line.contains("api.github.com"),
            "a default destination needs no calling out: {line}"
        );

        // An index-declared destination is named, because it is the one
        // thing the user cannot infer from the provider alone.
        let declared = confirm_prompt("gitlab", "octocat", HostSource::Index, "gitlab.corp.example");
        assert!(declared.contains("gitlab.corp.example"), "{declared}");
        assert!(declared.contains("index"), "and says who chose it: {declared}");
        assert!(declared.contains("[y/N]"), "the default is still no: {declared}");
    }

    #[test]
    fn only_an_explicit_yes_confirms() {
        for yes in ["y", "Y", "yes", "YES", " y ", "Yes\n"] {
            assert!(is_affirmative(yes), "{yes:?} must confirm");
        }
        for no in ["", "\n", "n", "N", "no", "maybe", "yy", "1", "true"] {
            assert!(!is_affirmative(no), "{no:?} must not confirm");
        }
    }

    // -----------------------------------------------------------------
    // C-006 — the piped credential
    // -----------------------------------------------------------------

    #[test]
    fn a_piped_token_strips_exactly_one_trailing_newline() {
        use secrecy::ExposeSecret as _;
        for (raw, want) in [
            ("ghp_tok", "ghp_tok"),
            ("ghp_tok\n", "ghp_tok"),
            ("ghp_tok\r\n", "ghp_tok"),
        ] {
            let token = read_piped_token(raw).expect("parses");
            assert_eq!(token.expose_secret(), want, "input {raw:?}");
        }
    }

    #[test]
    fn empty_piped_input_is_an_auth_error_and_never_falls_through() {
        // C-006: the caller *stated* it was supplying a credential, so
        // silently voting with a different one is worse than failing.
        for raw in ["", "\n", "   ", " \t\r\n"] {
            let err = read_piped_token(raw).expect_err("must refuse");
            assert_eq!(exit_of_ref(&err), ExitCode::AuthError, "input {raw:?}");
            assert!(
                matches!(err, RateError::NoCredential(_)),
                "an empty pipe is a credential failure, never a fall-through: {err}"
            );
        }
    }

    #[test]
    fn multi_line_piped_input_is_a_usage_error() {
        for raw in ["ghp_tok\nextra", "ghp_tok\nextra\n", "a\nb\nc"] {
            let err = read_piped_token(raw).expect_err("must refuse");
            assert_eq!(exit_of_ref(&err), ExitCode::UsageError, "input {raw:?}");
        }
    }

    #[test]
    fn a_refused_pipe_never_echoes_the_credential() {
        // The token must not appear in an error, in `--format json`, or in
        // a panic message.
        let secret = "ghp_supersecretvalue";
        let err = read_piped_token(&format!("{secret}\ntrailing")).expect_err("must refuse");
        let rendered = format!("{err} {err:?}");
        assert!(
            !rendered.contains(secret),
            "the credential leaked into an error: {rendered}"
        );
    }

    // -----------------------------------------------------------------
    // The standalone credential ladder
    // -----------------------------------------------------------------

    #[test]
    fn grim_rate_token_wins_the_ladder() {
        let ci = CiEnv {
            github_actions: true,
            github_server_url: Some("https://github.com".to_string()),
            github_token: Some("ci-token".to_string()),
            ..CiEnv::default()
        };
        let token = env_token("github.com", ForgeKind::GitHub, Some("own-token".to_string()), &ci).expect("resolves");
        assert_eq!(token.as_str(), "own-token");
    }

    #[test]
    fn the_ci_token_tier_is_host_matched() {
        let ci = CiEnv {
            github_actions: true,
            github_server_url: Some("https://github.com".to_string()),
            github_token: Some("ci-token".to_string()),
            ..CiEnv::default()
        };
        assert_eq!(
            env_token("github.com", ForgeKind::GitHub, None, &ci)
                .as_deref()
                .map(String::as_str),
            Some("ci-token")
        );
        // A different host: the CI token must not follow the vote there.
        assert!(env_token("ghes.corp.example", ForgeKind::GitHub, None, &ci).is_none());
    }

    #[test]
    fn a_foreign_ci_forge_never_contributes_its_token() {
        // The announce ladder's rule: a GitLab pipeline voting against a
        // GitHub host must not inherit GitLab credentials.
        let ci = CiEnv {
            gitlab_ci: true,
            ci_server_host: Some("github.com".to_string()),
            gitlab_token: Some("glpat-x".to_string()),
            ..CiEnv::default()
        };
        assert!(env_token("github.com", ForgeKind::GitHub, None, &ci).is_none());
    }

    #[test]
    fn the_announce_token_is_not_a_vote_credential() {
        // ADR § API Contract: the vote ladder carries its own, narrower
        // credential and never borrows publishing authority.
        let ci = CiEnv {
            announce_token: Some("announce-token".to_string()),
            ..CiEnv::default()
        };
        assert!(env_token("github.com", ForgeKind::GitHub, None, &ci).is_none());
    }

    // -----------------------------------------------------------------
    // Report shaping
    // -----------------------------------------------------------------

    #[test]
    fn a_dry_run_report_carries_the_sidecar_count_and_the_resolved_host() {
        let rating = RatingSummary {
            up: 12,
            target: "D_kwDOAbCdEf".to_string(),
            url: "https://github.com/acme/index/discussions/7".to_string(),
            provider: Some("github".to_string()),
            host: None,
        };
        let report = build_report(
            "ghcr.io/acme/x".to_string(),
            VoteAction::Up,
            Some(rating.up),
            &rating,
            Some("api.github.com".to_string()),
            HostSource::Default,
            None,
        );
        let v = serde_json::to_value(&report).unwrap();
        assert_eq!(v["ref"], "ghcr.io/acme/x");
        assert_eq!(v["action"], "up");
        assert_eq!(v["up"], 12);
        assert_eq!(v["provider"], "github");
        assert_eq!(v["host"], "api.github.com");
        assert_eq!(v["host_source"], "default");
        assert_eq!(v["url"], "https://github.com/acme/index/discussions/7");
        assert!(
            v["viewer_up"].is_null(),
            "no credential was piped, so nothing was asked"
        );
    }

    /// C-023 / S-022: a credentialed dry run carries the forge's answer,
    /// and S-023's failure carries an explicit `null` — never `false`.
    #[test]
    fn a_credentialed_dry_run_reports_the_viewer_state_and_a_failure_reports_null() {
        let rating = RatingSummary {
            up: 12,
            target: "D_kwDOAbCdEf".to_string(),
            url: "https://github.com/acme/index/discussions/7".to_string(),
            provider: Some("github".to_string()),
            host: None,
        };
        for state in [Some(true), Some(false), None] {
            let v = serde_json::to_value(build_report(
                "ghcr.io/acme/x".to_string(),
                VoteAction::Up,
                Some(rating.up),
                &rating,
                Some("api.github.com".to_string()),
                HostSource::Default,
                state,
            ))
            .unwrap();
            assert!(
                v.as_object().unwrap().contains_key("viewer_up"),
                "always present, even unknown: {v}"
            );
            match state {
                Some(known) => assert_eq!(v["viewer_up"], known),
                None => assert!(v["viewer_up"].is_null(), "unknown is null, never false: {v}"),
            }
        }
    }

    #[test]
    fn an_unresolved_host_is_reported_as_null_not_omitted() {
        // C-022: `host` is always present, null when nothing resolves —
        // which is how a client learns "grim cannot vote here".
        let rating = RatingSummary {
            up: 3,
            target: "t".to_string(),
            url: "u".to_string(),
            provider: Some("bitbucket".to_string()),
            host: None,
        };
        let report = build_report(
            "ghcr.io/acme/x".to_string(),
            VoteAction::Up,
            Some(3),
            &rating,
            None,
            HostSource::Index,
            None,
        );
        let v = serde_json::to_value(&report).unwrap();
        assert!(v.as_object().unwrap().contains_key("host"));
        assert!(v["host"].is_null());
        assert!(
            v["host_source"].is_null(),
            "no host means no decision to attribute, whatever the source argument said: {v}"
        );
        assert_eq!(v["provider"], "bitbucket", "the raw provider is still reported");
        assert!(
            v["viewer_up"].is_null(),
            "no host resolves, so no query is possible and the state is unknown"
        );
    }

    /// A completed vote reports the state the forge just gave it.
    ///
    /// `viewer_up` means "what the forge last said about this account", and
    /// a mutation response is the freshest possible answer — reporting null
    /// here would make `grim rate --up --format json` claim not to know the
    /// thing it had just done. GitLab's toggle carries no count, so `up`
    /// stays nullable independently of it.
    #[test]
    fn a_completed_vote_reports_the_state_the_mutation_returned() {
        let rating = RatingSummary {
            up: 8,
            target: "t".to_string(),
            url: "u".to_string(),
            provider: Some("github".to_string()),
            host: None,
        };
        for (voted, want) in [(Some(true), true), (Some(false), false)] {
            let report = build_report(
                "ghcr.io/acme/x".to_string(),
                VoteAction::Up,
                Some(8),
                &rating,
                Some("api.github.com".to_string()),
                HostSource::Default,
                voted,
            );
            let v = serde_json::to_value(&report).unwrap();
            assert_eq!(v["viewer_up"], want, "the mutation's own answer is reported");
        }

        // A forge that reports no viewer state leaves it unknown rather
        // than guessing from the requested action (R-3).
        let report = build_report(
            "ghcr.io/acme/x".to_string(),
            VoteAction::Up,
            None,
            &rating,
            Some("gitlab.com".to_string()),
            HostSource::Default,
            None,
        );
        let v = serde_json::to_value(&report).unwrap();
        assert!(
            v.as_object().unwrap().contains_key("viewer_up"),
            "always present, never omitted"
        );
        assert!(v["viewer_up"].is_null(), "unknown, never inferred from --up");
    }
}
