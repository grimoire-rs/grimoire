// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The `grim rate` write path's provider knowledge: which host a provider
//! votes against, which GraphQL document each forge's vote primitive
//! needs, and how to read the payload back.
//!
//! This module owns **no HTTP of its own** — it composes documents and
//! hands them to [`super::forge::graphql`], which owns the client, the
//! auth header and the `{data, errors}` envelope. Dispatch between the two
//! providers is a `match` on the provider string at the single call site
//! in `command::rate`, deliberately **not** a trait (ADR D10): two
//! implementations that never vary at runtime do not need a vtable, and
//! the asymmetry with the indexer's `RatingProvider` interface is
//! intentional — the indexer's is selected from config, grim's is selected
//! from data the index published.
//!
//! Endpoint resolution lives here too, because it is provider knowledge:
//! `github` votes against `api.github.com`, `gitlab` against `gitlab.com`,
//! and either may be redirected to a GitHub Enterprise Server or
//! self-managed GitLab host **only** from the user's own environment —
//! never from index-fetched content (plan C-007). Host comparison is
//! exact: ports included, ASCII-lowercased, IDNA-normalised, and with no
//! suffix matching whatsoever, so `evil-github.com` and
//! `github.com.evil.tld` are simply different hosts from `github.com`.

use secrecy::SecretString;

use super::forge::ForgeKind;

/// Which way a vote moves.
///
/// Not a `bool`: the two arms select different GraphQL documents on GitHub
/// and different toggle targets on GitLab, and a boolean parameter at the
/// call site would read as `github_vote(…, true)` (quality-core.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoteAction {
    /// Register this user's upvote.
    Up,
    /// Retract this user's **own** upvote. Not a downvote — votes are
    /// up-only and binary, and both forges' primitives are toggles.
    Remove,
}

impl VoteAction {
    /// The wire spelling used in the report's `action` field.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Remove => "remove",
        }
    }

    /// Whether the requested end state is "upvoted".
    fn wants_upvote(self) -> bool {
        matches!(self, Self::Up)
    }
}

/// A failure on the `grim rate` write path.
///
/// Every variant maps to exactly one exit code in
/// [`crate::error::classify`]; the mapping is the contract
/// (`docs/src/commands.md`), so a new variant must choose one there before
/// it compiles. No variant ever carries a credential — the token is a
/// [`SecretString`] that is exposed once, at the `Authorization` header,
/// and never enters an error, a report, or a panic message.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RateError {
    /// The reference did not parse as an artifact reference (exit 64).
    #[error("invalid artifact reference '{reference}': {reason}")]
    MalformedRef { reference: String, reason: String },

    /// A flag combination the surface does not accept (exit 64).
    #[error("{0}")]
    Usage(String),

    /// The reference resolved to no catalog row (exit 79).
    #[error("'{reference}' is not in any configured registry's catalog")]
    NoSuchRef { reference: String },

    /// The row exists but the index publishes no rating for it (exit 65).
    #[error("'{reference}' carries no rating; the index it comes from publishes no vote target for it")]
    NotRated { reference: String },

    /// The row is rated, but the sidecar declared no rating producer, so
    /// there is no mutation to issue (exit 65). Readable, not writable.
    #[error("the index publishing '{reference}' declares no rating provider; its ratings are readable, not votable")]
    NoProvider { reference: String },

    /// The index declared a rating provider grim cannot vote through
    /// (exit 65). The raw value is carried verbatim so the operator can
    /// see what the index actually published.
    #[error("unsupported rating provider '{0}'; grim can vote through 'github' and 'gitlab'")]
    UnsupportedProvider(String),

    /// `--token-host` named a different host than the one the vote
    /// resolves to (exit 80). Raised **before** the credential is read, so
    /// the token never reaches a header — the whole point of the flag.
    #[error(
        "--token-host '{declared}' does not match the host this vote resolves to ('{resolved}'); refusing to send the credential"
    )]
    TokenHostMismatch { declared: String, resolved: String },

    /// An injected credential (`--token-stdin` or `GRIM_RATE_TOKEN`) was
    /// heading for a host the **index** declared, without `--token-host`
    /// naming it (exit 80). Raised before the credential is read.
    ///
    /// The host-matched rungs of the ladder need no such gate — they only
    /// ever resolve a credential the user already holds *for that host* —
    /// so this fires for the two credentials nothing else binds to a
    /// destination. See `adr_index_declared_rating_host.md`.
    #[error(
        "this index declares its own rating host ('{host}'); a piped or GRIM_RATE_TOKEN credential must name where it may go — pass --token-host {host}"
    )]
    UndeclaredTokenHost { host: String },

    /// No credential could be resolved (exit 80).
    #[error("{0}")]
    NoCredential(String),

    /// `--offline` (or `GRIM_OFFLINE`) blocked a forge round trip (exit 81).
    ///
    /// Covers the vote itself and the credentialed `--dry-run` viewer-state
    /// read, which both need the network. A *bare* `--dry-run` resolves from
    /// the catalog alone and is deliberately offline-safe (C-022).
    #[error(
        "offline mode blocks this: `grim rate` reaches a forge for both a vote and a credentialed --dry-run, and has no cached path; a bare --dry-run works offline"
    )]
    Offline,

    /// The forge is unreachable, answered 5xx, or applied a secondary rate
    /// limit (exit 69).
    #[error("{0}")]
    Unavailable(String),

    /// The forge rejected the credential with 401/403 (exit 80).
    #[error("{0}")]
    Unauthorized(String),

    /// The forge answered 404, or the vote subject no longer exists
    /// (exit 79).
    #[error("{0}")]
    NotFound(String),

    /// The GraphQL response carried a populated top-level `errors` array,
    /// or a body that is not a GraphQL envelope at all (exit 65).
    ///
    /// Checked **independently of the HTTP status**: a 200 carrying
    /// `errors` with partial `data` is the one door the transport-level
    /// invariants do not watch, and reading it as success is silent data
    /// loss.
    #[error("{0}")]
    Graphql(String),
}

/// The default host each rating provider votes against, or `None` for a
/// provider grim does not implement.
///
/// GitHub's is the **API** host (`api.github.com`) because that is where
/// its GraphQL endpoint lives; GitLab's is the instance host, which serves
/// `/api/graphql` directly.
pub fn default_host(provider: &str) -> Option<&'static str> {
    match provider {
        "github" => Some("api.github.com"),
        "gitlab" => Some("gitlab.com"),
        _ => None,
    }
}

/// The [`ForgeKind`] a rating provider maps onto, or `None` for a provider
/// grim does not implement. [`ForgeKind::Plain`] is never produced — a
/// plain git host has no vote API, and mapping onto it would let a
/// mutation be sent unauthenticated.
pub fn forge_kind(provider: &str) -> Option<ForgeKind> {
    match provider {
        "github" => Some(ForgeKind::GitHub),
        "gitlab" => Some(ForgeKind::GitLab),
        _ => None,
    }
}

/// Resolve the host a vote for `provider` is sent to: the provider default,
/// overridden by `user_override` when the user set one for a GitHub
/// Enterprise Server or self-managed GitLab instance.
///
/// `user_override` must come from the user's own environment and **never**
/// from index-fetched content (plan C-007) — `stats.json` carries a
/// provider name, a target and a url, and deliberately no host at all, so
/// there is nothing in the fetched document that could reach this
/// parameter.
///
/// Returns `None` when the provider is unrecognised: there is no host to
/// name, which is precisely the answer a `--dry-run` caller needs before
/// it picks an authentication provider.
pub fn resolve_host(provider: &str, user_override: Option<&str>) -> Option<String> {
    let default = default_host(provider)?;
    match user_override {
        Some(raw) => normalize_host(raw),
        None => normalize_host(default),
    }
}

/// Normalise a `host[:port]` for comparison: IDNA-encoded, ASCII-lowercased,
/// trailing root dot dropped, and the port kept when it is not the scheme
/// default.
///
/// Parsing through [`reqwest::Url`] rather than hand-rolling the rules is
/// what makes "IDNA-normalised" true rather than aspirational — the URL
/// parser already applies UTS-46 to the host and lowercases it, and it is
/// the same normalisation the HTTP client will apply when it actually
/// dials, so the string compared here is the string contacted. `None` for
/// anything that is not a bare host (a scheme, a path, userinfo, an empty
/// string), which fails the comparison closed.
pub fn normalize_host(raw: &str) -> Option<String> {
    let raw = raw.trim();
    // Reject anything carrying more than an authority up front: a value
    // like `github.com/../evil` or `user@evil.tld` must not normalise to
    // something that compares equal to a bare host.
    if raw.is_empty() || raw.contains(['/', '@', '?', '#', '\\', ' ']) || raw.contains("://") {
        return None;
    }
    // The URL parser keeps a trailing root dot as part of the host, so
    // `github.com.` and `github.com` would compare unequal. DNS treats them
    // as one name and this only ever *narrows* — no dot-stripping can make
    // `evil.tld` equal `github.com` — so a declaration written either way
    // matches the endpoint grim actually dials.
    let raw = raw.trim_end_matches('.');
    let url = reqwest::Url::parse(&format!("https://{raw}/")).ok()?;
    let host = url.host_str()?;
    // `Url::port()` is `None` for the scheme default (443), so an explicit
    // `:443` and a bare host compare equal — which is correct, they are the
    // same endpoint.
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

/// Whether two hosts are the same endpoint.
///
/// **Exact comparison, no suffix matching.** `evil-github.com` and
/// `github.com.evil.tld` are different hosts from `github.com`, and a host
/// that does not normalise at all never matches anything — including
/// itself.
pub fn hosts_equal(a: &str, b: &str) -> bool {
    match (normalize_host(a), normalize_host(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Whether `host` is a GitHub SaaS **API** host — the shape that fronts a
/// separate site host and serves GraphQL from `/graphql`.
///
/// Two spellings qualify: `api.github.com`, and GitHub Enterprise Cloud
/// with data residency (GHE.com), whose API host is
/// `api.<subdomain>.ghe.com` — see
/// <https://docs.github.com/en/enterprise-cloud@latest/graphql/guides/forming-calls-with-graphql>.
///
/// The GHE.com test is an exact shape, never a suffix match: the `api.`
/// prefix is a whole label and the subdomain is a single label with no dot
/// or port in it. So `api.github.com.evil.tld`, `xapi.octocorp.ghe.com`,
/// `api.ghe.com` and `api.a.b.ghe.com` are all ordinary hosts that keep
/// their own credentials and the instance-host layout. Getting that wrong
/// in the widening direction would send a bearer token to a host of the
/// attacker's choosing.
fn is_github_saas_api_host(host: &str) -> bool {
    host == "api.github.com"
        || host
            .strip_prefix("api.")
            .and_then(|rest| rest.strip_suffix(".ghe.com"))
            .is_some_and(|subdomain| !subdomain.is_empty() && !subdomain.contains(['.', ':']))
}

/// The site host behind an API host — what the forge CLIs and the CI
/// environment call the same instance.
///
/// Only the GitHub SaaS shapes split the two: `api.github.com` fronts
/// `github.com`, and GHE.com's `api.<subdomain>.ghe.com` fronts
/// `<subdomain>.ghe.com`, which is what `GITHUB_SERVER_URL` names and what
/// `gh auth token --hostname` expects. GitHub Enterprise Server and GitLab
/// serve their API from the instance host, so those pass through
/// unchanged. See [`is_github_saas_api_host`] for the exact shape.
pub fn site_host(host: &str) -> &str {
    if is_github_saas_api_host(host) {
        host.strip_prefix("api.").unwrap_or(host)
    } else {
        host
    }
}

/// The GraphQL endpoint on `host`.
///
/// The GitHub SaaS hosts serve GraphQL from the dedicated API host at
/// `/graphql` — github.com, and GHE.com data residency at
/// `https://api.<subdomain>.ghe.com/graphql`, per
/// <https://docs.github.com/en/enterprise-cloud@latest/graphql/guides/forming-calls-with-graphql>.
/// Every other case — GitHub Enterprise Server and GitLab, SaaS or
/// self-managed — serves it from `/api/graphql` on the instance host.
///
/// The scheme is `https://` for every host except the loopback forms, which
/// get plain HTTP — see [`is_loopback`] for the exact set and why widening
/// it would be a credential leak.
pub fn graphql_endpoint(host: &str) -> String {
    let scheme = if is_loopback(host) { "http" } else { "https" };
    if is_github_saas_api_host(host) {
        format!("{scheme}://{host}/graphql")
    } else {
        format!("{scheme}://{host}/api/graphql")
    }
}

/// Whether `host` is one of the loopback forms grim contacts over plain
/// HTTP: `localhost`, `127.0.0.1` and `::1`, bare or on any port.
///
/// Same motivation as the always-on loopback set of
/// `GRIM_INSECURE_REGISTRIES` — a local server has no TLS certificate to
/// present, and the acceptance suite needs a fake forge the full CLI can
/// vote against, the alternative being a test-only seam in the production
/// write path. Deliberately **not the same set**, so do not sync the two:
/// that one is four exact `host:port` strings on port 5000 only
/// (`registry_client::plain_http_hosts_with`), because `oci-client`'s
/// `HttpsExcept` matches exactly and the registry transport allowlist is a
/// shipped surface Principle 9 freezes. This one admits any port — the fake
/// forge binds an ephemeral one — and `[::1]`. Widening the registry list to
/// match would be a plain-HTTP downgrade on that frozen surface.
///
/// **It widens nothing here.** A loopback address is reachable only from
/// this machine, so a credential sent to one cannot leave it; every other
/// host keeps `https://` exactly as before, and no other narrowing moves —
/// `--token-host` still gates. A sidecar-declared host may name this set
/// only when the index is itself loopback, which is what stops a remote
/// index aiming a credential at a port on the reader's own machine
/// (`index_source::accepted_rating_host`). The
/// rest of `127.0.0.0/8` is loopback too (RFC 1122) and would have been
/// safe to admit; it is excluded because the accepted decision (D-1) names
/// three forms, and a set nobody has to reason about is worth more than the
/// addresses it leaves out.
///
/// The comparison is an equality test on the whole host — never a prefix,
/// suffix or substring one — so `127.0.0.1.evil.example`,
/// `localhost.evil.example`, `notlocalhost` and `127.0.0.1@evil.example`
/// are all simply different hosts and all stay `https://`. It runs on
/// [`normalize_host`]'s output while [`graphql_endpoint`] interpolates the
/// raw `host`: the two agree because both call sites pass what
/// [`resolve_host`] already normalised, and a value that does not normalise
/// is refused here outright rather than compared.
pub fn is_loopback(host: &str) -> bool {
    // Normalised first so anything that is not a bare `host[:port]` — a
    // path, userinfo, a scheme — fails closed here exactly as it does at
    // the `--token-host` gate.
    let Some(normalized) = normalize_host(host) else {
        return false;
    };
    // Re-parsed rather than split on the last colon: `normalize_host` keeps
    // the port, and an IPv6 literal's own colons make a rightmost-colon
    // split wrong for `[::1]`.
    reqwest::Url::parse(&format!("https://{normalized}/"))
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .is_some_and(|bare| matches!(bare.as_str(), "localhost" | "127.0.0.1" | "[::1]"))
}

/// Everything a vote needs about where it is going and who it is going as.
///
/// Deliberately **not** `#[derive(Debug)]`: the credential lives here, and
/// the whole `src/api/` always-present-null convention makes an
/// accidentally-printable secret specifically dangerous. The token is
/// reachable only through [`Self::token`], which the single
/// `Authorization`-header site in `forge::graphql` calls.
pub struct RateContext {
    kind: ForgeKind,
    endpoint: String,
    token: SecretString,
}

impl RateContext {
    /// Bind a credential to the forge kind and endpoint it may be sent to.
    pub fn new(kind: ForgeKind, endpoint: String, token: SecretString) -> Self {
        Self { kind, endpoint, token }
    }

    /// The forge flavor, which selects the auth header and the documents.
    pub fn kind(&self) -> ForgeKind {
        self.kind
    }

    /// The GraphQL endpoint this credential may be sent to.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The credential, still wrapped. Callers must not clone the exposed
    /// value out of the one header site.
    pub fn token(&self) -> &SecretString {
        &self.token
    }
}

/// What a completed vote reports back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoteOutcome {
    /// The subject's upvote count after the mutation, when the forge's
    /// payload carries one. GitLab's emoji toggle reports state rather
    /// than a total, so it is `None` there — the report renders that as an
    /// explicit `null`, never as `0`.
    pub up: Option<u32>,
    /// Whether the forge reports **this** account as having upvoted the
    /// subject once the mutation settled — GitHub's `viewerHasUpvoted`,
    /// GitLab's `toggledOn`. `None` means the payload said nothing about
    /// it, which is not the same as "not voted": the local record is left
    /// untouched rather than being set from a guess
    /// ([`crate::catalog::vote_store`], invariant R-3).
    pub voted: Option<bool>,
}

/// The authenticated account's login, for the confirmation prompt.
///
/// A vote posts publicly under this account, so the prompt names it. One
/// read-only query, issued only when a prompt is actually going to be
/// shown — `--yes` skips both.
///
/// # Errors
///
/// Any [`RateError`] [`super::forge::graphql`] raises, plus
/// [`RateError::Graphql`] when the response carries no login.
pub async fn viewer_identity(http: &reqwest::Client, ctx: &RateContext) -> Result<ViewerIdentity, RateError> {
    let (document, root, login_field) = match ctx.kind() {
        ForgeKind::GitHub => ("query { viewer { login databaseId } }", "viewer", "login"),
        ForgeKind::GitLab => ("query { currentUser { username id } }", "currentUser", "username"),
        // `forge_kind` never yields `Plain`; kept as an error rather than a
        // panic so the exhaustiveness stays a compile-time guard.
        ForgeKind::Plain => return Err(RateError::UnsupportedProvider("plain".to_string())),
    };
    let data = super::forge::graphql(http, ctx, document, serde_json::json!({})).await?;
    let viewer = data
        .get(root)
        .filter(|v| !v.is_null())
        .ok_or_else(|| RateError::Graphql("the forge reported no account for this credential".to_string()))?;
    let login = viewer
        .get(login_field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| RateError::Graphql("the forge reported no account for this credential".to_string()))?
        .to_string();
    Ok(ViewerIdentity {
        login,
        account_id: account_id_of(ctx.kind(), viewer)?,
    })
}

/// Who the credential belongs to on the forge.
///
/// The login is for the human — it is what the confirmation prompt names.
/// The account id is for the machine: [`crate::catalog::vote_store`] keys
/// its records on it precisely because a login can be renamed away and
/// handed to somebody else, and the next holder must not inherit the
/// first one's vote display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerIdentity {
    /// The account's current display login.
    pub login: String,
    /// The account's immutable numeric id, as a string.
    pub account_id: String,
}

/// Pull the immutable account id out of a viewer payload.
///
/// GitHub's `databaseId` is the number directly; GitLab's `id` is a
/// global id (`gid://gitlab/User/1234`) whose last segment is that same
/// number. Neither moves when the account is renamed, which is the whole
/// reason the vote key is not the login.
fn account_id_of(kind: ForgeKind, viewer: &serde_json::Value) -> Result<String, RateError> {
    let id = match kind {
        ForgeKind::GitHub => viewer
            .get("databaseId")
            .and_then(serde_json::Value::as_u64)
            .map(|n| n.to_string()),
        ForgeKind::GitLab => viewer
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(|gid| numeric_account_id(gid).to_string()),
        ForgeKind::Plain => None,
    };
    id.filter(|id| !id.is_empty())
        .ok_or_else(|| RateError::Graphql("the forge reported no account id for this credential".to_string()))
}

/// The numeric tail of a forge account id: everything after the last `/`.
/// A value with no `/` is already the id and passes through unchanged.
fn numeric_account_id(raw: &str) -> &str {
    raw.rsplit('/').next().unwrap_or(raw)
}

/// The read-only document that asks the forge for **this** credential's
/// own vote state on `target`.
///
/// One document per forge, and one round trip each — GitLab needs the
/// viewer's identity to recognise its own reaction, so `currentUser` is a
/// second **root field** of the same query rather than a second request
/// (which is also why this does not call [`viewer_identity`]).
///
/// # Errors
///
/// [`RateError::UnsupportedProvider`] for [`ForgeKind::Plain`], which has
/// no GraphQL surface at all.
fn viewer_state_document(kind: ForgeKind) -> Result<&'static str, RateError> {
    Ok(match kind {
        // The index binds a Discussion node id, and `viewerHasUpvoted` is
        // the forge's own statement about the authenticated account.
        ForgeKind::GitHub => "query($id: ID!) { node(id: $id) { ... on Discussion { viewerHasUpvoted } } }",
        // GitLab has no viewer-state field: the answer is "is one of these
        // reactions mine", so the awards and the account id come back
        // together. `hasNextPage` is selected because a list grim only saw
        // part of cannot prove absence.
        ForgeKind::GitLab => {
            "query($id: WorkItemID!) { currentUser { id } workItem(id: $id) { widgets { \
             ... on WorkItemWidgetAwardEmoji { awardEmoji(first: 100) { pageInfo { hasNextPage } \
             nodes { name user { id } } } } } } }"
        }
        ForgeKind::Plain => return Err(RateError::UnsupportedProvider("plain".to_string())),
    })
}

/// Read the viewer's own vote state out of a [`viewer_state_document`]
/// response.
///
/// `None` for **every** shape this cannot read with certainty — a missing
/// field, a subject the forge would not return, a reaction list that was
/// truncated. Invariant R-3: an unanswered question renders neutral, and
/// reporting it as "not voted" is the one wrong answer.
fn viewer_state_of(kind: ForgeKind, data: &serde_json::Value) -> Option<bool> {
    match kind {
        ForgeKind::GitHub => data.get("node")?.get("viewerHasUpvoted")?.as_bool(),
        ForgeKind::GitLab => gitlab_viewer_state(data),
        ForgeKind::Plain => None,
    }
}

/// Whether one of the work item's emoji reactions is this account's own
/// upvote.
///
/// The account is matched by **immutable id**, never by username, for the
/// same reason [`crate::catalog::vote_store`] keys on one: a renamed login
/// must not let the next holder inherit the first one's vote display.
fn gitlab_viewer_state(data: &serde_json::Value) -> Option<bool> {
    let me = account_id_of(ForgeKind::GitLab, data.get("currentUser")?).ok()?;
    let awards = data
        .get("workItem")?
        .get("widgets")?
        .as_array()?
        .iter()
        .find_map(|widget| widget.get("awardEmoji"))?;
    let mine = awards.get("nodes")?.as_array()?.iter().any(|award| {
        award.get("name").and_then(serde_json::Value::as_str) == Some(GITLAB_UPVOTE_EMOJI)
            && award
                .get("user")
                .and_then(|user| user.get("id"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|gid| numeric_account_id(gid) == me)
    });
    if mine {
        return Some(true);
    }
    // Not finding it in a list that continues past this page proves
    // nothing, so the honest answer is unknown rather than "not voted".
    let truncated = awards
        .get("pageInfo")
        .and_then(|page| page.get("hasNextPage"))
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    (!truncated).then_some(false)
}

/// Ask the forge whether this credential's account has already upvoted
/// `target`, changing nothing (plan C-023).
///
/// Exactly one read-only query. The credential travels the same path a
/// vote's does — [`super::forge::graphql`], whose `authorize_mutation` is
/// still the only `expose_secret()` site on this command.
///
/// # Errors
///
/// Any [`RateError`] [`super::forge::graphql`] raises, plus
/// [`RateError::UnsupportedProvider`] for a kind with no GraphQL surface.
/// The caller turns every one of them into a `null` report field — a
/// failed query cannot prove the user has not voted.
pub async fn viewer_upvoted(
    http: &reqwest::Client,
    ctx: &RateContext,
    target: &str,
) -> Result<Option<bool>, RateError> {
    let document = viewer_state_document(ctx.kind())?;
    let data = super::forge::graphql(http, ctx, document, serde_json::json!({ "id": target })).await?;
    Ok(viewer_state_of(ctx.kind(), &data))
}

/// GitHub: `addUpvote` / `removeUpvote` on the discussion the index bound
/// to this artifact.
///
/// Both mutations are explicit rather than toggles, so a repeated `--up`
/// is idempotent. The selection reads only `Votable` interface fields, so
/// it works for every upvotable subject the index may have created.
///
/// # Errors
///
/// Any [`RateError`] [`super::forge::graphql`] raises, plus
/// [`RateError::NotFound`] when the payload carries no subject — the
/// thread the index recorded no longer exists.
pub async fn github_vote(
    http: &reqwest::Client,
    ctx: &RateContext,
    target: &str,
    action: VoteAction,
) -> Result<VoteOutcome, RateError> {
    let (field, document) = match action {
        VoteAction::Up => (
            "addUpvote",
            "mutation($id: ID!) { addUpvote(input: {subjectId: $id}) { subject { upvoteCount viewerHasUpvoted } } }",
        ),
        VoteAction::Remove => (
            "removeUpvote",
            "mutation($id: ID!) { removeUpvote(input: {subjectId: $id}) { subject { upvoteCount viewerHasUpvoted } } }",
        ),
    };
    let data = super::forge::graphql(http, ctx, document, serde_json::json!({ "id": target })).await?;
    let subject = data
        .get(field)
        .and_then(|p| p.get("subject"))
        .filter(|s| !s.is_null())
        .ok_or_else(|| RateError::NotFound(format!("the vote subject '{target}' no longer exists on the forge")))?;
    Ok(VoteOutcome {
        up: subject
            .get("upvoteCount")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok()),
        // The forge's own statement about this account. Read rather than
        // inferred from `action`: what was asked for and what the forge
        // ended up holding are different facts, and only the second one
        // may be cached.
        voted: subject.get("viewerHasUpvoted").and_then(serde_json::Value::as_bool),
    })
}

/// The emoji a GitLab upvote is expressed as. GitLab has no upvote
/// primitive of its own; `:thumbsup:` is the reaction its own UI renders
/// as the upvote control on issues and work items.
const GITLAB_UPVOTE_EMOJI: &str = "thumbsup";

/// GitLab: `awardEmojiToggle` on the work item the index bound to this
/// artifact.
///
/// The primitive is a genuine **toggle**, not an explicit set, so the
/// requested end state and the resulting state can disagree — a `--up` on
/// an already-upvoted item would otherwise silently retract the user's
/// vote. One corrective toggle restores the requested state; it fires only
/// in that already-in-target-state case, never on the common path.
///
/// # Errors
///
/// Any [`RateError`] [`super::forge::graphql`] raises, plus
/// [`RateError::Graphql`] when the payload carries per-mutation `errors`
/// or no `toggledOn` at all.
pub async fn gitlab_vote(
    http: &reqwest::Client,
    ctx: &RateContext,
    target: &str,
    action: VoteAction,
) -> Result<VoteOutcome, RateError> {
    let mut settled = gitlab_toggle(http, ctx, target).await?;
    if settled != action.wants_upvote() {
        // The toggle moved the user away from what they asked for, which
        // means they were already in the requested state. Flip back.
        settled = gitlab_toggle(http, ctx, target).await?;
        if settled != action.wants_upvote() {
            return Err(RateError::Graphql(format!(
                "the forge did not settle on the requested vote state for '{target}'"
            )));
        }
    }
    // `awardEmojiToggle` reports state, not a total: the count stays
    // unknown rather than being invented, and the report renders it null.
    // `settled` is the forge's own last `toggledOn`, not the request.
    Ok(VoteOutcome {
        up: None,
        voted: Some(settled),
    })
}

/// One `awardEmojiToggle` round trip, returning the resulting state.
async fn gitlab_toggle(http: &reqwest::Client, ctx: &RateContext, target: &str) -> Result<bool, RateError> {
    const DOCUMENT: &str = "mutation($id: AwardableID!, $name: String!) \
         { awardEmojiToggle(input: {awardableId: $id, name: $name}) { errors toggledOn } }";
    let data = super::forge::graphql(
        http,
        ctx,
        DOCUMENT,
        serde_json::json!({ "id": target, "name": GITLAB_UPVOTE_EMOJI }),
    )
    .await?;
    let payload = data
        .get("awardEmojiToggle")
        .filter(|p| !p.is_null())
        .ok_or_else(|| RateError::NotFound(format!("the vote subject '{target}' no longer exists on the forge")))?;
    // GitLab reports mutation-level failures in a payload `errors` array
    // beside a 200 and an empty top-level `errors` — a second place the
    // "200 means success" reading loses data.
    let errors: Vec<&str> = payload
        .get("errors")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect();
    if !errors.is_empty() {
        return Err(RateError::Graphql(format!(
            "the forge refused the vote: {}",
            errors.join("; ")
        )));
    }
    payload
        .get("toggledOn")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| RateError::Graphql("the forge reported no vote state for this mutation".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // C-007 — endpoint resolution
    // -----------------------------------------------------------------

    #[test]
    fn default_host_per_provider() {
        assert_eq!(resolve_host("github", None).as_deref(), Some("api.github.com"));
        assert_eq!(resolve_host("gitlab", None).as_deref(), Some("gitlab.com"));
    }

    #[test]
    fn an_unrecognised_provider_resolves_no_host() {
        assert_eq!(resolve_host("bitbucket", None), None);
        assert_eq!(resolve_host("", None), None);
        // Even with an override in play: grim cannot vote through a
        // provider it does not implement, so there is no host to name.
        assert_eq!(resolve_host("bitbucket", Some("ghes.corp.example")), None);
    }

    #[test]
    fn a_user_config_override_replaces_the_default_host() {
        assert_eq!(
            resolve_host("github", Some("ghes.corp.example")).as_deref(),
            Some("ghes.corp.example")
        );
        assert_eq!(
            resolve_host("gitlab", Some("gitlab.corp.example:8443")).as_deref(),
            Some("gitlab.corp.example:8443")
        );
    }

    /// C-007: the override reaches [`resolve_host`] from the user's own
    /// environment only. `stats.json` carries `providers.rating`, `target`
    /// and `url` and no host field at all, so this test pins the *shape*
    /// that makes index-supplied host data unrepresentable: the provider
    /// string an index publishes can never become a host.
    #[test]
    fn an_index_supplied_provider_string_can_never_become_a_host() {
        for hostile in [
            "evil.tld",
            "github.com@evil.tld",
            "https://evil.tld",
            "api.github.com/../evil.tld",
        ] {
            assert_eq!(
                resolve_host(hostile, None),
                None,
                "a provider value from the index must resolve no host: {hostile}"
            );
        }
    }

    #[test]
    fn host_comparison_is_exact_and_never_matches_a_suffix() {
        assert!(hosts_equal("github.com", "github.com"));
        assert!(hosts_equal("GitHub.COM", "github.com"), "ASCII case-folded");
        assert!(hosts_equal("github.com.", "github.com"), "root dot dropped");
        assert!(hosts_equal("github.com:443", "github.com"), "the scheme default port");

        assert!(!hosts_equal("evil-github.com", "github.com"), "no prefix matching");
        assert!(!hosts_equal("github.com.evil.tld", "github.com"), "no suffix matching");
        assert!(!hosts_equal("api.github.com", "github.com"), "no subdomain matching");
        assert!(
            !hosts_equal("github.com:8443", "github.com"),
            "ports are part of the host"
        );
    }

    #[test]
    fn a_host_that_does_not_normalise_never_matches() {
        for bad in ["", "   ", "https://github.com", "github.com/x", "u@github.com"] {
            assert!(!hosts_equal(bad, "github.com"), "{bad} must not match github.com");
            assert!(!hosts_equal(bad, bad), "{bad} must not even match itself");
        }
    }

    #[test]
    fn idna_normalisation_folds_a_unicode_host_onto_its_punycode_form() {
        // UTS-46 is applied by the URL parser, so the unicode spelling and
        // the punycode spelling of one host are one host.
        assert_eq!(
            normalize_host("bücher.example").as_deref(),
            Some("xn--bcher-kva.example")
        );
        assert!(hosts_equal("bücher.example", "xn--bcher-kva.example"));
    }

    #[test]
    fn graphql_endpoints_follow_each_forge_layout() {
        assert_eq!(graphql_endpoint("api.github.com"), "https://api.github.com/graphql");
        assert_eq!(
            graphql_endpoint("ghes.corp.example"),
            "https://ghes.corp.example/api/graphql"
        );
        assert_eq!(graphql_endpoint("gitlab.com"), "https://gitlab.com/api/graphql");
        assert_eq!(
            graphql_endpoint("gitlab.corp.example:8443"),
            "https://gitlab.corp.example:8443/api/graphql"
        );
    }

    /// GitHub Enterprise Cloud with data residency (GHE.com) is github.com's
    /// layout on a customer subdomain, not GHES's: GraphQL lives at
    /// `/graphql` on `api.<subdomain>.ghe.com`, and the site host the forge
    /// CLI and `GITHUB_SERVER_URL` name is `<subdomain>.ghe.com`.
    ///
    /// The negative controls are the test, not decoration. The match is an
    /// exact shape — `api.` + one label + `.ghe.com` — so a lookalike host
    /// keeps the GHES layout and, more importantly, keeps its credential
    /// matched against its own host.
    #[test]
    fn ghe_com_data_residency_uses_the_github_com_layout() {
        assert_eq!(
            graphql_endpoint("api.octocorp.ghe.com"),
            "https://api.octocorp.ghe.com/graphql"
        );
        assert_eq!(site_host("api.octocorp.ghe.com"), "octocorp.ghe.com");

        // GHES keeps the instance-host layout and its own host.
        assert_eq!(
            graphql_endpoint("ghes.corp.example"),
            "https://ghes.corp.example/api/graphql"
        );
        assert_eq!(site_host("ghes.corp.example"), "ghes.corp.example");

        for other in [
            // Suffix lookalike: not github.com, and not the SaaS shape.
            "api.github.com.evil.tld",
            // The `api.` prefix is a whole label, never a substring.
            "xapi.octocorp.ghe.com",
            // No subdomain label at all.
            "api.ghe.com",
            // A GHE.com subdomain is a single label.
            "api.a.b.ghe.com",
        ] {
            assert!(
                graphql_endpoint(other).ends_with("/api/graphql"),
                "{other} is not the GHE.com SaaS shape and keeps /api/graphql"
            );
            assert_eq!(site_host(other), other, "{other} fronts no other site host");
        }
    }

    /// D-1: the scheme is plain HTTP for the loopback forms only, so an
    /// acceptance test can drive a real vote against a fake forge on
    /// `127.0.0.1` without a test-only seam in production code.
    ///
    /// The lookalikes are the point of the test, not decoration: a rule
    /// written as "contains `127.0.0.1`" or "ends with `localhost`" would
    /// send a bearer credential to an attacker-chosen host over plain
    /// HTTP.
    #[test]
    fn graphql_endpoint_is_plain_http_on_loopback_only() {
        for loopback in [
            "127.0.0.1",
            "127.0.0.1:8080",
            "localhost",
            "localhost:5000",
            "[::1]",
            "[::1]:8080",
        ] {
            assert_eq!(
                graphql_endpoint(loopback),
                format!("http://{loopback}/api/graphql"),
                "{loopback} is loopback and may be contacted over plain HTTP"
            );
        }

        for hostile in [
            "127.0.0.1.evil.example",
            "localhost.evil.example",
            "notlocalhost",
            "localhost-evil.example",
            "evil.example.localhost.tld",
            "127.0.0.1@evil.example",
            "127.0.0.2",
            "127.1.2.3",
            "[::2]",
            "gitlab.corp.example",
        ] {
            assert!(
                graphql_endpoint(hostile).starts_with("https://"),
                "{hostile} merely resembles a loopback host and must stay https"
            );
        }
    }

    #[test]
    fn forge_kind_never_maps_a_provider_onto_plain() {
        assert_eq!(forge_kind("github"), Some(ForgeKind::GitHub));
        assert_eq!(forge_kind("gitlab"), Some(ForgeKind::GitLab));
        assert_eq!(forge_kind("bitbucket"), None);
        assert_eq!(forge_kind("plain"), None);
    }

    #[test]
    fn vote_action_wire_spellings_are_locked() {
        assert_eq!(VoteAction::Up.as_str(), "up");
        assert_eq!(VoteAction::Remove.as_str(), "remove");
    }

    /// The credential never renders. `RateContext` deliberately has no
    /// `Debug`, so this pins the next-best observable: no error variant
    /// can be constructed carrying one, and the type that holds it is not
    /// printable at all.
    #[test]
    fn no_error_variant_carries_a_credential() {
        let messages = [
            RateError::MalformedRef {
                reference: "BAD".to_string(),
                reason: "uppercase".to_string(),
            }
            .to_string(),
            RateError::NoCredential("no credential resolvable".to_string()).to_string(),
            RateError::TokenHostMismatch {
                declared: "api.github.com".to_string(),
                resolved: "ghes.corp.example".to_string(),
            }
            .to_string(),
            RateError::UnsupportedProvider("bitbucket".to_string()).to_string(),
        ];
        for message in messages {
            assert!(
                !message.contains("ghp_") && !message.contains("glpat-"),
                "an error message must never carry a credential: {message}"
            );
        }
    }

    #[test]
    fn unsupported_provider_carries_the_raw_value() {
        let message = RateError::UnsupportedProvider("bitbucket".to_string()).to_string();
        assert!(
            message.contains("bitbucket"),
            "the operator must see what the index published: {message}"
        );
    }

    // -----------------------------------------------------------------
    // C-008 — the account id the vote record is keyed on
    // -----------------------------------------------------------------

    #[test]
    fn a_github_account_id_is_the_numeric_database_id() {
        let viewer = serde_json::json!({ "login": "octocat", "databaseId": 583231 });
        assert_eq!(
            account_id_of(ForgeKind::GitHub, &viewer).expect("an account id"),
            "583231"
        );
    }

    #[test]
    fn a_gitlab_account_id_is_the_tail_of_its_global_id() {
        let viewer = serde_json::json!({ "username": "tanuki", "id": "gid://gitlab/User/1234" });
        assert_eq!(
            account_id_of(ForgeKind::GitLab, &viewer).expect("an account id"),
            "1234"
        );
    }

    #[test]
    fn a_bare_gitlab_id_passes_through_unchanged() {
        let viewer = serde_json::json!({ "username": "tanuki", "id": "1234" });
        assert_eq!(
            account_id_of(ForgeKind::GitLab, &viewer).expect("an account id"),
            "1234"
        );
    }

    #[test]
    fn a_viewer_carrying_only_a_login_yields_no_account_id() {
        // A login is renameable and reusable, so it must never stand in
        // for the id the vote record is keyed on — the record is skipped
        // instead, and reads back as unknown.
        let viewer = serde_json::json!({ "login": "octocat" });
        assert!(account_id_of(ForgeKind::GitHub, &viewer).is_err());
        let viewer = serde_json::json!({ "username": "tanuki" });
        assert!(account_id_of(ForgeKind::GitLab, &viewer).is_err());
    }

    #[test]
    fn an_empty_account_id_is_refused() {
        let viewer = serde_json::json!({ "username": "tanuki", "id": "gid://gitlab/User/" });
        assert!(account_id_of(ForgeKind::GitLab, &viewer).is_err());
    }

    // -----------------------------------------------------------------
    // C-023 — the viewer-state read (S-022 / S-023)
    // -----------------------------------------------------------------

    /// S-022: "one read-only query, no mutation". The documents are the
    /// place that could stop being true, so the property is asserted on
    /// them literally.
    #[test]
    fn the_viewer_state_documents_are_queries_and_never_mutations() {
        for kind in [ForgeKind::GitHub, ForgeKind::GitLab] {
            let document = viewer_state_document(kind).expect("a document");
            assert!(document.starts_with("query("), "must be a query: {document}");
            assert!(!document.contains("mutation"), "must mutate nothing: {document}");
            for write in ["addUpvote", "removeUpvote", "awardEmojiToggle", "Create", "Update"] {
                assert!(
                    !document.contains(write),
                    "'{write}' has no place in a read: {document}"
                );
            }
        }
        // No GraphQL surface at all, so there is nothing to ask.
        assert!(viewer_state_document(ForgeKind::Plain).is_err());
    }

    #[test]
    fn github_reports_the_viewers_own_upvote_state() {
        let voted = serde_json::json!({ "node": { "viewerHasUpvoted": true } });
        let not_voted = serde_json::json!({ "node": { "viewerHasUpvoted": false } });
        assert_eq!(viewer_state_of(ForgeKind::GitHub, &voted), Some(true));
        assert_eq!(viewer_state_of(ForgeKind::GitHub, &not_voted), Some(false));
    }

    /// S-023: a payload that does not answer leaves the state **unknown**.
    /// `false` here would be the report claiming the user has not voted on
    /// the strength of a discussion the forge would not even return.
    #[test]
    fn an_unreadable_github_payload_is_unknown_not_not_voted() {
        for payload in [
            serde_json::json!({}),
            // The node id resolved to nothing — a deleted discussion.
            serde_json::json!({ "node": null }),
            // A node that is not a Discussion, so the fragment selected
            // nothing.
            serde_json::json!({ "node": {} }),
            serde_json::json!({ "node": { "viewerHasUpvoted": "yes" } }),
        ] {
            assert_eq!(
                viewer_state_of(ForgeKind::GitHub, &payload),
                None,
                "must be unknown: {payload}"
            );
        }
    }

    /// GitLab has no viewer-state field, so "did I vote" is "is one of
    /// these reactions mine" — matched on the immutable account id.
    #[test]
    fn gitlab_finds_this_accounts_own_reaction() {
        let data = gitlab_awards(
            "gid://gitlab/User/1234",
            &[("thumbsup", "gid://gitlab/User/1234")],
            false,
        );
        assert_eq!(viewer_state_of(ForgeKind::GitLab, &data), Some(true));
    }

    #[test]
    fn gitlab_reports_not_voted_only_when_it_saw_the_whole_list() {
        let others = gitlab_awards(
            "gid://gitlab/User/1234",
            &[
                ("thumbsup", "gid://gitlab/User/9999"),
                ("rocket", "gid://gitlab/User/1234"),
            ],
            false,
        );
        assert_eq!(
            viewer_state_of(ForgeKind::GitLab, &others),
            Some(false),
            "somebody else's upvote and my own unrelated reaction are both not my vote"
        );
        assert_eq!(
            viewer_state_of(ForgeKind::GitLab, &gitlab_awards("gid://gitlab/User/1234", &[], false)),
            Some(false)
        );
    }

    /// R-3: a reaction list that continues past the page grim read cannot
    /// prove absence, so it is unknown rather than "not voted".
    #[test]
    fn a_truncated_gitlab_reaction_list_is_unknown_not_not_voted() {
        let truncated = gitlab_awards(
            "gid://gitlab/User/1234",
            &[("thumbsup", "gid://gitlab/User/9999")],
            true,
        );
        assert_eq!(viewer_state_of(ForgeKind::GitLab, &truncated), None);
        // …unless the vote was already found, which no further page can
        // take back.
        let found = gitlab_awards(
            "gid://gitlab/User/1234",
            &[("thumbsup", "gid://gitlab/User/1234")],
            true,
        );
        assert_eq!(viewer_state_of(ForgeKind::GitLab, &found), Some(true));
    }

    #[test]
    fn an_unreadable_gitlab_payload_is_unknown_not_not_voted() {
        for payload in [
            serde_json::json!({}),
            // Authenticated as nobody: without an id there is nothing to
            // match a reaction against.
            serde_json::json!({ "currentUser": null, "workItem": { "widgets": [] } }),
            serde_json::json!({ "currentUser": { "username": "tanuki" }, "workItem": { "widgets": [] } }),
            // The work item is gone.
            serde_json::json!({ "currentUser": { "id": "gid://gitlab/User/1" }, "workItem": null }),
            // Present, but carrying no reactions widget at all — the work
            // item type does not support them.
            serde_json::json!({
                "currentUser": { "id": "gid://gitlab/User/1" },
                "workItem": { "widgets": [{ "description": "…" }] }
            }),
        ] {
            assert_eq!(
                viewer_state_of(ForgeKind::GitLab, &payload),
                None,
                "must be unknown: {payload}"
            );
        }
    }

    /// A `viewer_state_document` response shaped like the real one.
    fn gitlab_awards(me: &str, awards: &[(&str, &str)], has_next_page: bool) -> serde_json::Value {
        let nodes: Vec<serde_json::Value> = awards
            .iter()
            .map(|(name, user)| serde_json::json!({ "name": name, "user": { "id": user } }))
            .collect();
        serde_json::json!({
            "currentUser": { "id": me },
            "workItem": {
                "widgets": [
                    { "description": "…" },
                    { "awardEmoji": { "pageInfo": { "hasNextPage": has_next_page }, "nodes": nodes } }
                ]
            }
        })
    }

    #[test]
    fn no_account_id_error_carries_a_credential() {
        let message = account_id_of(ForgeKind::GitHub, &serde_json::json!({}))
            .expect_err("no id")
            .to_string();
        assert!(
            !message.contains("token") && !message.contains("Bearer"),
            "the message must describe the account, never the credential: {message}"
        );
    }
}
