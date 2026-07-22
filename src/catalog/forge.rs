// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Forge resolution for `grim publish --announce` — which forge API (if
//! any) fronts the index git repository, and how to talk to it.
//!
//! A *forge* is the API flavor of the git host: GitHub (github.com or a
//! GitHub Enterprise instance), GitLab (gitlab.com or self-hosted), or
//! plain git (no vendor API). The kind is decoupled from the host name so
//! enterprise instances work without host constants. Resolution order:
//! explicit `[announce] forge` > the CI environment (only when the CI
//! server host equals the announce target host — a GitLab pipeline
//! announcing to a GitHub index must not inherit GitLab credentials) >
//! the github.com convention > plain.
//!
//! All forge traffic is REST — no `gh`/`glab` CLI dependency. Tokens are
//! sent as request headers only and never logged.

use serde::{Deserialize, Serialize};

use super::index_announce::AnnounceError;

/// The API flavor of the git host an index repository lives on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ForgeKind {
    /// github.com or a GitHub Enterprise instance (REST API v3).
    #[serde(rename = "github")]
    GitHub,
    /// gitlab.com or a self-hosted GitLab instance (REST API v4).
    #[serde(rename = "gitlab")]
    GitLab,
    /// A plain git host without a (known) forge API.
    Plain,
}

/// Snapshot of the CI/env variables forge resolution reads.
///
/// A plain struct rather than ambient `std::env` reads so resolution is
/// unit-testable (env mutation is `unsafe` in edition 2024 and the crate
/// forbids unsafe).
#[derive(Debug, Default, Clone)]
pub struct CiEnv {
    /// `GRIM_ANNOUNCE_TOKEN` — explicit announce token, always wins.
    pub announce_token: Option<String>,
    /// `GITHUB_ACTIONS` truthy.
    pub github_actions: bool,
    /// `GITHUB_SERVER_URL` (e.g. `https://github.example.corp`).
    pub github_server_url: Option<String>,
    /// `GITHUB_API_URL`.
    pub github_api_url: Option<String>,
    /// `GITHUB_REPOSITORY_OWNER` — namespace default in Actions.
    pub github_repository_owner: Option<String>,
    /// `GH_TOKEN` else `GITHUB_TOKEN`.
    pub github_token: Option<String>,
    /// `GITLAB_CI` truthy.
    pub gitlab_ci: bool,
    /// `CI_SERVER_HOST`.
    pub ci_server_host: Option<String>,
    /// `CI_API_V4_URL`.
    pub ci_api_v4_url: Option<String>,
    /// `CI_PROJECT_NAMESPACE` — namespace default in GitLab CI.
    pub ci_project_namespace: Option<String>,
    /// `GITLAB_TOKEN` (never `CI_JOB_TOKEN` — it cannot open MRs).
    pub gitlab_token: Option<String>,
    /// `CI_JOB_TOKEN` is set and non-empty. Presence only — the value is
    /// never read into grim (the git credential helper the announce push
    /// appends reads it from the child environment) and never used for
    /// the MR API (see `gitlab_token`).
    pub ci_job_token_present: bool,
}

impl CiEnv {
    /// Read the snapshot from the process environment.
    pub fn from_env() -> Self {
        let var = |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty());
        let truthy = |key: &str| {
            std::env::var(key)
                .is_ok_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        };
        Self {
            announce_token: var("GRIM_ANNOUNCE_TOKEN"),
            github_actions: truthy("GITHUB_ACTIONS"),
            github_server_url: var("GITHUB_SERVER_URL"),
            github_api_url: var("GITHUB_API_URL"),
            github_repository_owner: var("GITHUB_REPOSITORY_OWNER"),
            github_token: var("GH_TOKEN").or_else(|| var("GITHUB_TOKEN")),
            gitlab_ci: truthy("GITLAB_CI"),
            ci_server_host: var("CI_SERVER_HOST"),
            ci_api_v4_url: var("CI_API_V4_URL"),
            ci_project_namespace: var("CI_PROJECT_NAMESPACE"),
            gitlab_token: var("GITLAB_TOKEN"),
            ci_job_token_present: var("CI_JOB_TOKEN").is_some(),
        }
    }
}

// This type lives beside `ForgeKind` rather than in the `publish` command
// because it is a forge-domain decision that `ensure_fork` acts on, while the
// command layer only parses it; the manifest-side spelling — including the
// legacy boolean — is `command::publish::ForkSetting`'s concern. Kept as a
// plain comment, not rustdoc: this type's doc comment is published verbatim as
// the `description` of `$defs/ForkPolicy` in `grim schema --kind publish`, and
// internal crate layout has no business in a user-facing schema.
/// How `[announce]` decides whether to fork the index before pushing the
/// announce branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ForkPolicy {
    /// Never fork; always push to the upstream index directly.
    Never,
    /// Fork only when the authenticated token has no push access to the
    /// upstream index (the default).
    Auto,
    /// Always fork and open a cross-repository PR/MR, even when the token has
    /// push access — lets a maintainer prefer PR review over a direct push
    /// (and dogfood the external-contributor path).
    Always,
}

/// The resolved forge: kind, API endpoint, credential, CI namespace hint.
#[derive(Debug, Clone)]
pub struct ForgeContext {
    /// The resolved forge kind.
    pub kind: ForgeKind,
    /// API base URL without a trailing slash (`https://api.github.com`,
    /// `https://gitlab.example.com/api/v4`). `None` for plain git hosts.
    pub api_url: Option<String>,
    /// API token. Sent as a request header only — never logged.
    pub token: Option<String>,
    /// Namespace default contributed by a host-matched CI environment.
    pub ci_namespace: Option<String>,
}

/// A host-matched CI contribution (kind + endpoint + token + namespace).
struct CiCandidate {
    kind: ForgeKind,
    api_url: Option<String>,
    token: Option<String>,
    namespace: Option<String>,
}

/// Resolve the forge for an announce targeting `host`.
///
/// `host` is the index host path segment (`index/<host>/…`), already
/// derived from the repository URL or set explicitly. The CI environment
/// contributes API URL / token / namespace **only** when its server host
/// equals `host` and its forge kind survived resolution — explicit config
/// always overrides.
pub fn resolve(explicit: Option<ForgeKind>, api_url_override: Option<String>, host: &str, env: &CiEnv) -> ForgeContext {
    let ci = ci_candidate(host, env);
    let kind = explicit
        .or(ci.as_ref().map(|c| c.kind))
        .unwrap_or(if host.eq_ignore_ascii_case("github.com") {
            ForgeKind::GitHub
        } else {
            ForgeKind::Plain
        });
    // A CI candidate of a different kind than the resolved one contributes
    // nothing (e.g. explicit `forge = "gitlab"` inside GitHub Actions).
    let ci = ci.filter(|c| c.kind == kind);

    let api_url = match kind {
        ForgeKind::Plain => None,
        ForgeKind::GitHub | ForgeKind::GitLab => api_url_override
            .or_else(|| ci.as_ref().and_then(|c| c.api_url.clone()))
            .or_else(|| conventional_api_url(kind, host)),
    };
    let token = env
        .announce_token
        .clone()
        .or_else(|| ci.as_ref().and_then(|c| c.token.clone()));
    let ci_namespace = ci.and_then(|c| c.namespace);
    ForgeContext {
        kind,
        api_url,
        token,
        ci_namespace,
    }
}

/// The CI environment's contribution, gated on a server-host match.
fn ci_candidate(host: &str, env: &CiEnv) -> Option<CiCandidate> {
    if env.github_actions {
        let server_host = env
            .github_server_url
            .as_deref()
            .and_then(host_of_url)
            .unwrap_or_else(|| "github.com".to_string());
        if server_host.eq_ignore_ascii_case(host) {
            return Some(CiCandidate {
                kind: ForgeKind::GitHub,
                api_url: env.github_api_url.clone(),
                token: env.github_token.clone(),
                namespace: env.github_repository_owner.clone(),
            });
        }
    }
    if env.gitlab_ci
        && env
            .ci_server_host
            .as_deref()
            .is_some_and(|h| h.eq_ignore_ascii_case(host))
    {
        return Some(CiCandidate {
            kind: ForgeKind::GitLab,
            api_url: env.ci_api_v4_url.clone(),
            token: env.gitlab_token.clone(),
            namespace: env.ci_project_namespace.clone(),
        });
    }
    None
}

/// The conventional API base for a forge kind on `host`. github.com's API
/// lives on its own host; GitHub Enterprise serves `/api/v3`, GitLab
/// `/api/v4` — both on the instance host.
fn conventional_api_url(kind: ForgeKind, host: &str) -> Option<String> {
    match kind {
        ForgeKind::GitHub if host.eq_ignore_ascii_case("github.com") => Some("https://api.github.com".to_string()),
        ForgeKind::GitHub => Some(format!("https://{host}/api/v3")),
        ForgeKind::GitLab => Some(format!("https://{host}/api/v4")),
        ForgeKind::Plain => None,
    }
}

/// The host segment of an `https://host/...` (or other schemed) URL.
fn host_of_url(url: &str) -> Option<String> {
    let normalized = crate::oci::git_provenance::normalize_remote_url(url.trim_end_matches('/')).or_else(|| {
        // normalize_remote_url requires a path; a bare server URL
        // (`https://github.example.corp`) has none — take the
        // authority directly.
        let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
        let host = rest.split('/').next()?;
        (!host.is_empty()).then(|| format!("https://{host}/x"))
    })?;
    normalized
        .strip_prefix("https://")?
        .split('/')
        .next()
        .map(str::to_lowercase)
}

/// Percent-encode a path segment (RFC 3986 unreserved characters pass
/// through). Encodes `/` too — GitLab project paths and namespaces embed
/// in a single path segment (`platform%2Fai`).
fn encode_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Build a forge HTTP client with request `timeout`, grim user-agent,
/// embedded CA roots merged with the system trust store (see [`crate::tls`]),
/// and redirects disabled (see the `SECURITY` note on the builder).
fn build_client(timeout: std::time::Duration) -> Result<reqwest::Client, reqwest::Error> {
    crate::tls::merge_embedded_roots(
        reqwest::Client::builder()
            .timeout(timeout)
            // SECURITY: keep redirects disabled. reqwest otherwise follows up
            // to 10 redirects and replays the custom `PRIVATE-TOKEN` /
            // `Authorization` header on the redirect target, so a forge (or a
            // hijacked endpoint) answering 3xx with a cross-host `Location`
            // would exfiltrate the GitLab PAT / GitHub token. These REST
            // endpoints never legitimately redirect — a non-2xx is surfaced as
            // an error, not chased. Do not relax without re-solving that leak.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("grim/", env!("CARGO_PKG_VERSION"))),
    )
    .build()
}

/// The forge HTTP client for permission probes and API calls (see
/// [`build_client`]): 30s timeout. Built **once** at the announce entry
/// point and threaded through every forge call below as an explicit `http`
/// parameter, instead of rebuilt per call site (a single `grim publish
/// --announce` run against one host previously paid for up to five
/// avoidable TLS handshakes). Readiness polls use a separate short-timeout
/// client instead so a single hung request cannot consume the whole
/// deadline ([`poll_until_ready`]).
pub fn client() -> Result<reqwest::Client, reqwest::Error> {
    build_client(std::time::Duration::from_secs(30))
}

/// Attach the forge-appropriate auth header (GitHub: `Authorization:
/// Bearer`; GitLab: `PRIVATE-TOKEN`) when a token is present.
fn authorize(ctx: &ForgeContext, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    let request = match ctx.kind {
        ForgeKind::GitHub => request.header("Accept", "application/vnd.github+json"),
        ForgeKind::GitLab | ForgeKind::Plain => request,
    };
    // Exhaustive over ForgeKind so a future forge kind cannot fall through a
    // wildcard and be sent unauthenticated — each kind classifies its header.
    match &ctx.token {
        Some(token) => match ctx.kind {
            ForgeKind::GitHub => request.header("Authorization", format!("Bearer {token}")),
            ForgeKind::GitLab => request.header("PRIVATE-TOKEN", token.as_str()),
            ForgeKind::Plain => request,
        },
        None => request,
    }
}

/// Look up the namespace's numeric owner id on the forge: the immutable
/// account id on GitHub; the group id for group namespaces or the user id
/// for user namespaces on GitLab.
///
/// # Errors
///
/// [`AnnounceError::OwnerLookup`] when the forge has no API, the request
/// fails, or the response carries no numeric id.
pub async fn lookup_owner_id(
    http: &reqwest::Client,
    ctx: &ForgeContext,
    namespace: &str,
) -> Result<u64, AnnounceError> {
    let wrap = |source: Box<dyn std::error::Error + Send + Sync>| AnnounceError::OwnerLookup {
        namespace: namespace.to_string(),
        source,
    };
    let api = ctx
        .api_url
        .as_deref()
        .ok_or_else(|| wrap("plain git host has no owner API — set `[announce] owner_id`".into()))?;
    match ctx.kind {
        ForgeKind::GitHub => {
            let url = format!("{api}/users/{}", encode_segment(namespace));
            let body = get_json(http, ctx, &url).await.map_err(|e| wrap(e.into()))?;
            body.get("id")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| wrap("response carries no numeric id".into()))
        }
        ForgeKind::GitLab => gitlab_owner_id(http, ctx, api, namespace).await.map_err(wrap),
        // Plain never carries an api_url, so the check above already
        // returned — kept as an error rather than a panic macro.
        ForgeKind::Plain => Err(wrap(
            "plain git host has no owner API — set `[announce] owner_id`".into(),
        )),
    }
}

/// GitLab owner-id resolution: the group id for group namespaces, the
/// user id for user namespaces.
///
/// `/namespaces` is membership-scoped — an index validator's project bot
/// token cannot see a foreign user namespace at all — so user namespaces
/// (kind `user` on a visible lookup, or a 404) resolve through the public
/// `/users?username=` endpoint instead: the user id is the only owner id
/// every index-side token can verify.
async fn gitlab_owner_id(
    http: &reqwest::Client,
    ctx: &ForgeContext,
    api: &str,
    namespace: &str,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let namespace_url = format!("{api}/namespaces/{}", encode_segment(namespace));
    let response = authorize(ctx, http.get(namespace_url)).send().await?;
    let status = response.status();
    if status.is_success() {
        let body: serde_json::Value = response.json().await?;
        if body.get("kind").and_then(serde_json::Value::as_str) != Some("user") {
            return body
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| "response carries no numeric id".into());
        }
    } else if status != reqwest::StatusCode::NOT_FOUND {
        return Err(format!("HTTP status {status} from the namespace lookup").into());
    }
    let users_url = format!("{api}/users?username={}", encode_segment(namespace));
    let body = get_json(http, ctx, &users_url).await?;
    let wanted = namespace.to_lowercase();
    let mut hits = body.as_array().into_iter().flatten().filter(|user| {
        user.get("username")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|name| name.to_lowercase() == wanted)
    });
    match (hits.next(), hits.next()) {
        (Some(user), None) => user
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| "response carries no numeric id".into()),
        _ => Err(format!("namespace '{namespace}' not found — set `[announce] owner_id`").into()),
    }
}

/// The authenticated GitHub login (`GET /user`), as a namespace default.
/// Best-effort: `None` without a token or on any API failure.
pub async fn github_login(http: &reqwest::Client, ctx: &ForgeContext) -> Option<String> {
    if ctx.kind != ForgeKind::GitHub || ctx.token.is_none() {
        return None;
    }
    let api = ctx.api_url.as_deref()?;
    let body = get_json(http, ctx, &format!("{api}/user")).await.ok()?;
    body.get("login")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// A resolved fork of the index repository: where to push the announce
/// branch and how to open the cross-repository change request.
///
/// Every field is read from the fork-API **response body**, never composed
/// from the upstream basename — a fork can be renamed (upstream `index`,
/// fork `grimoire-index`), so `{login}/{repo}` guessing would target the
/// wrong repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkTarget {
    /// The fork's push URL (GitHub `clone_url`, GitLab `http_url_to_repo`).
    pub push_url: String,
    /// The PR head owner (GitHub `owner.login`) for the `owner:branch` head.
    pub head_owner: String,
    /// The fork's canonical `owner/repo` (GitHub `full_name`, GitLab
    /// `path_with_namespace`): the poll target and the reported repo.
    pub full_name: String,
    /// The fork's numeric project id (GitLab `id`) — the project the
    /// cross-project MR is posted from; `None` on GitHub.
    pub fork_project_id: Option<u64>,
    /// The upstream project's numeric id (GitLab) — the cross-project MR's
    /// `target_project_id`; `None` on GitHub.
    pub upstream_project_id: Option<u64>,
    /// Whether this run created the fork (`false` when an existing fork was
    /// reused). Report-only — rides along on the push target.
    pub created: bool,
}

/// The pull-request head for `branch`: `"{owner}:{branch}"` when pushing
/// from a fork (GitHub cross-repository head), else the bare branch name
/// (byte-identical to a same-repository announce).
fn pr_head(fork: Option<&ForkTarget>, branch: &str) -> String {
    match fork {
        Some(f) => format!("{}:{}", f.head_owner, branch),
        None => branch.to_string(),
    }
}

/// Open (or find) the pull/merge request for a pushed announce branch via
/// the forge API. Best-effort by contract: every failure degrades to
/// `None` and the caller reports the pushed branch instead — a failed PR
/// is never worse than today's plain push.
///
/// `fork` is `Some` when the branch was pushed to a fork: the PR head
/// becomes `owner:branch`, the MR is posted from the fork with
/// `target_project_id` set to the upstream index, and the change request
/// still targets the upstream index.
pub async fn create_change_request(
    http: &reqwest::Client,
    ctx: &ForgeContext,
    repo_url: &str,
    branch: &str,
    title: &str,
    fork: Option<&ForkTarget>,
) -> Option<String> {
    let (Some(_), Some(api)) = (&ctx.token, ctx.api_url.clone()) else {
        return None;
    };
    let project = project_path(repo_url)?;
    let result = match ctx.kind {
        ForgeKind::GitHub => github_pull_request(http, ctx, &api, &project, branch, title, fork).await,
        ForgeKind::GitLab => gitlab_merge_request(http, ctx, &api, &project, branch, title, fork).await,
        ForgeKind::Plain => return None,
    };
    match result {
        Ok(url) => Some(url),
        Err(detail) => {
            tracing::info!(
                "forge API did not open the change request ({detail}); the branch is pushed — open it manually"
            );
            None
        }
    }
}

/// The forge project path (`owner/repo`, `group/subgroup/project`) from
/// the index repository URL.
fn project_path(repo_url: &str) -> Option<String> {
    let url = repo_url.strip_prefix("git+").unwrap_or(repo_url);
    let https = crate::oci::git_provenance::normalize_remote_url(url)?;
    let (_, path) = https.strip_prefix("https://")?.split_once('/')?;
    (!path.is_empty()).then(|| path.to_string())
}

/// `POST /repos/{project}/pulls`, reusing an existing open PR on 422.
async fn github_pull_request(
    http: &reqwest::Client,
    ctx: &ForgeContext,
    api: &str,
    project: &str,
    branch: &str,
    title: &str,
    fork: Option<&ForkTarget>,
) -> Result<String, String> {
    let base = get_json(http, ctx, &format!("{api}/repos/{project}")).await?;
    let default_branch = base
        .get("default_branch")
        .and_then(serde_json::Value::as_str)
        .ok_or("repository response carries no default branch")?;

    let head = pr_head(fork, branch);
    let response = authorize(ctx, http.post(format!("{api}/repos/{project}/pulls")))
        .json(&serde_json::json!({
            "title": title,
            "head": head,
            "base": default_branch,
            "body": "Automated announcement via `grim publish --announce`.",
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status().as_u16() == 422 {
        // Likely "a pull request already exists" — the force-push above
        // already updated it; find and reuse its URL. The head owner is the
        // fork owner when forking, else the upstream owner.
        let owner = fork
            .map(|f| f.head_owner.as_str())
            .unwrap_or_else(|| project.split('/').next().unwrap_or_default());
        let existing: serde_json::Value = authorize(
            ctx,
            http.get(format!("{api}/repos/{project}/pulls"))
                .query(&[("head", format!("{owner}:{branch}")), ("state", "open".to_string())]),
        )
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
        return existing
            .get(0)
            .and_then(|pr| pr.get("html_url"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "pull request exists but could not be located".to_string());
    }
    let body: serde_json::Value = response
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    body.get("html_url")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "pull request response carries no URL".to_string())
}

/// `POST /projects/{path}/merge_requests`, reusing the open MR on 409.
///
/// When `fork` is set the MR is cross-project: it is posted from the fork's
/// project (`/projects/{fork_id}/merge_requests`) with `target_project_id`
/// set to the upstream index — the branch lives on the fork, GitLab's create
/// endpoint is always the source project.
async fn gitlab_merge_request(
    http: &reqwest::Client,
    ctx: &ForgeContext,
    api: &str,
    project: &str,
    branch: &str,
    title: &str,
    fork: Option<&ForkTarget>,
) -> Result<String, String> {
    let encoded = encode_segment(project);
    let base = get_json(http, ctx, &format!("{api}/projects/{encoded}")).await?;
    let default_branch = base
        .get("default_branch")
        .and_then(serde_json::Value::as_str)
        .ok_or("project response carries no default branch")?;

    // Cross-project MR: create from the fork's project, targeting the upstream.
    // Same-project MR: create on the upstream project (byte-identical to today).
    let (mr_endpoint, target_project_id) =
        match fork.and_then(|f| f.fork_project_id.map(|id| (id, f.upstream_project_id))) {
            Some((fork_id, upstream_id)) => (format!("{api}/projects/{fork_id}/merge_requests"), upstream_id),
            None => (format!("{api}/projects/{encoded}/merge_requests"), None),
        };
    let mut body = serde_json::json!({
        "source_branch": branch,
        "target_branch": default_branch,
        "title": title,
        "description": "Automated announcement via `grim publish --announce`.",
        "squash": true,
        "remove_source_branch": true,
    });
    if let Some(target_project_id) = target_project_id {
        body["target_project_id"] = serde_json::Value::from(target_project_id);
    }
    let response = authorize(ctx, http.post(mr_endpoint))
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status().as_u16() == 409 {
        // An MR for this source branch is already open — the force-push
        // above already updated it; find and reuse its URL. GitLab lists an
        // MR on its TARGET project, so query the upstream `encoded` project
        // by source_branch (works for both same- and cross-project MRs;
        // deterministic branch names make a source_branch collision
        // improbable).
        let existing: serde_json::Value = authorize(
            ctx,
            http.get(format!("{api}/projects/{encoded}/merge_requests"))
                .query(&[("source_branch", branch), ("state", "opened")]),
        )
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
        return existing
            .get(0)
            .and_then(|mr| mr.get("web_url"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "merge request exists but could not be located".to_string());
    }
    let body: serde_json::Value = response
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    body.get("web_url")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "merge request response carries no URL".to_string())
}

/// Fork-readiness poll bounds. GitHub provisions a fork's git objects
/// asynchronously — a fork's metadata reads ready minutes before its first
/// push can — so readiness runs to a wall-clock deadline with exponential
/// backoff, each attempt capped by a short per-request timeout, rather than a
/// fixed attempt count that only incidentally bounds wall-clock time.
// ponytail: exp backoff (2s doubling, 30s cap) under a 5-min wall-clock
// deadline; each poll request is 8s-timeout-bounded so one black-holed
// attempt can't consume the whole deadline. Bump FORK_POLL_DEADLINE if very
// large forks routinely exceed it. The GitHub metadata-ready-before-git-
// objects-ready gap is covered independently by the bounded push-retry at
// the fork-push site (index_announce.rs::push_announce_branch).
const FORK_POLL_INITIAL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
const FORK_POLL_MAX_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
const FORK_POLL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(300);
const FORK_POLL_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// Timing bounds for a bounded exponential-backoff retry loop. Shared by
/// [`poll_until_ready`] (fork readiness) and
/// [`gitlab_find_owned_fork_bounded`] (409 reuse enumeration retry), each
/// defaulting to their own module constants. A dedicated struct so a test
/// can drive either loop on a short deadline without real network waits.
#[derive(Debug, Clone, Copy)]
struct PollBounds {
    initial_interval: std::time::Duration,
    max_interval: std::time::Duration,
    deadline: std::time::Duration,
    request_timeout: std::time::Duration,
}

impl Default for PollBounds {
    fn default() -> Self {
        Self {
            initial_interval: FORK_POLL_INITIAL_INTERVAL,
            max_interval: FORK_POLL_MAX_INTERVAL,
            deadline: FORK_POLL_DEADLINE,
            request_timeout: FORK_POLL_REQUEST_TIMEOUT,
        }
    }
}

/// Outcome of a single fork-readiness poll.
#[derive(Debug)]
enum Readiness {
    /// The fork is ready to receive the announce branch.
    Ready,
    /// Not ready yet — keep polling until the deadline.
    Pending,
    /// The fork will never become ready (GitLab reported a failed import);
    /// stop polling and surface the cause.
    Failed(String),
}

/// Classify a GitLab project's `import_status` for readiness polling: a
/// failed import fast-fails (it will never finish) carrying its
/// `import_error`, rather than polling to the deadline and burying the cause.
fn gitlab_import_readiness(body: &serde_json::Value) -> Readiness {
    match body.get("import_status").and_then(serde_json::Value::as_str) {
        Some("finished") => Readiness::Ready,
        Some("failed") => Readiness::Failed(
            body.get("import_error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown import error")
                .to_string(),
        ),
        _ => Readiness::Pending,
    }
}

/// The next backoff interval: double the current, capped at `max`.
fn next_interval(current: std::time::Duration, max: std::time::Duration) -> std::time::Duration {
    current.saturating_mul(2).min(max)
}

/// The authenticated GitLab username (`GET /user`), used as the namespace
/// for an existing-fork lookup. Best-effort: `None` without a token or on
/// any API failure (mirror of [`github_login`]).
pub async fn gitlab_current_user(http: &reqwest::Client, ctx: &ForgeContext) -> Option<String> {
    if ctx.kind != ForgeKind::GitLab || ctx.token.is_none() {
        return None;
    }
    let api = ctx.api_url.as_deref()?;
    let body = get_json(http, ctx, &format!("{api}/user")).await.ok()?;
    body.get("username")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// Ensure a fork of the index repository exists and is ready to receive the
/// announce branch, returning the push target (fork clone URL, PR/MR head
/// owner, and — GitLab — the source project id).
///
/// Returns `Ok(None)` — push to the upstream index unchanged — when `policy`
/// is [`ForkPolicy::Never`] (checked first, before any request is issued),
/// there is no token, the forge is plain, the project path is not derivable,
/// the permission probe is ambiguous or shows push access (under
/// [`ForkPolicy::Auto`]), or the authenticated user owns the upstream (a
/// self-fork is impossible). Returns [`AnnounceError::Fork`] only once forking
/// is required and the fork cannot be created, security-verified (its parent
/// must match the upstream), or made ready.
///
/// [`ForkPolicy::Always`] forks even when the token has push access to the
/// upstream — the only guard it lifts is the push-access early return; the
/// self-fork and namespace-verification guards still apply.
///
/// # Errors
///
/// [`AnnounceError::Fork`] when a required fork cannot be created,
/// verified, or polled ready.
pub async fn ensure_fork(
    http: &reqwest::Client,
    ctx: &ForgeContext,
    repo_url: &str,
    policy: ForkPolicy,
) -> Result<Option<ForkTarget>, AnnounceError> {
    if policy == ForkPolicy::Never {
        tracing::debug!("`[announce] fork` is `never` — pushing the announce branch at the upstream index");
        return Ok(None);
    }
    // Past this point only `Auto` and `Always` remain, and the two differ by
    // exactly one thing: whether the push-access probe can veto the fork.
    let force = policy == ForkPolicy::Always;
    let (Some(_token), Some(api)) = (&ctx.token, ctx.api_url.clone()) else {
        tracing::debug!("no announce token or forge API endpoint — degrading the announce to the upstream push");
        return Ok(None);
    };
    let Some(project) = project_path(repo_url) else {
        tracing::debug!("could not derive a forge project path from '{repo_url}' — degrading to the upstream push");
        return Ok(None);
    };
    // The trusted upstream host: a fork always lives on the same forge
    // instance, so the fork push URL must resolve here. Publisher-configured
    // (from `repo_url`), never taken from a forge response.
    let Some(host) = super::index_announce::index_host(repo_url) else {
        tracing::debug!("could not derive the index host from '{repo_url}' — degrading to the upstream push");
        return Ok(None);
    };
    match ctx.kind {
        ForgeKind::GitHub => github_ensure_fork(http, ctx, &api, &project, &host, force).await,
        ForgeKind::GitLab => gitlab_ensure_fork(http, ctx, &api, &project, &host, force).await,
        ForgeKind::Plain => {
            tracing::debug!("forge is plain git with no fork API — degrading the announce to the upstream push");
            Ok(None)
        }
    }
}

/// Log a failed or ambiguous fork permission probe before degrading to the
/// upstream push. Identical wording on both the GitHub and GitLab fork
/// flows ([`github_ensure_fork`], [`gitlab_ensure_fork`]).
fn log_probe_failed(project: &str, detail: &str) {
    tracing::info!(
        "fork permission probe failed for '{project}' ({detail}) — degrading the announce to the upstream push"
    );
}

/// Log that the token already has (or ambiguously might have) push access,
/// so no fork is needed. Identical wording on both the GitHub and GitLab
/// fork flows ([`github_ensure_fork`], [`gitlab_ensure_fork`]).
fn log_already_pushable(project: &str) {
    tracing::debug!("token already has push access to '{project}' (or the permission is ambiguous) — not forking");
}

/// GitHub fork flow: probe push permission → self-fork guard → reuse a
/// verified existing fork → create one → poll ready.
async fn github_ensure_fork(
    http: &reqwest::Client,
    ctx: &ForgeContext,
    api: &str,
    project: &str,
    host: &str,
    force: bool,
) -> Result<Option<ForkTarget>, AnnounceError> {
    // A failed or permission-less probe is ambiguous — degrade to the
    // upstream push rather than force a fork.
    let repo = match get_json(http, ctx, &format!("{api}/repos/{project}")).await {
        Ok(repo) => repo,
        Err(detail) => {
            log_probe_failed(project, &detail);
            return Ok(None);
        }
    };
    // `force` (`[announce] fork = "always"`) forks even with push access.
    if !force && github_can_push(&repo) != Some(false) {
        log_already_pushable(project);
        return Ok(None);
    }
    // Self-fork guard: GitHub refuses to fork your own repository. Compared
    // case-insensitively for the same reason as `verify_fork_push_url`: the
    // owner is spelled by the publisher, the login reported by the API.
    let upstream_owner = project.split('/').next().unwrap_or_default();
    let login = github_login(http, ctx).await;
    if login.as_deref().is_some_and(|l| l.eq_ignore_ascii_case(upstream_owner)) {
        tracing::debug!(
            "authenticated login '{upstream_owner}' owns the upstream '{project}' — a self-fork is impossible, degrading to the upstream push"
        );
        return Ok(None);
    }
    // Without a known authenticated login the fork's namespace can't be
    // verified, so no push target is safe — degrade to the upstream push.
    let Some(login) = login else {
        tracing::debug!(
            "could not resolve the authenticated GitHub login — degrading the announce to the upstream push"
        );
        return Ok(None);
    };
    // Reuse a verified existing fork at the conventional path (created = false).
    let basename = project.rsplit('/').next().unwrap_or(project);
    if let Ok(json) = get_json(http, ctx, &format!("{api}/repos/{login}/{basename}")).await
        && let Ok(target) = github_fork_target(&json, project, host, &login)
    {
        return Ok(Some(target));
    }
    // Create (or adopt a renamed) fork; build the target from the response
    // body — never from the upstream basename (a fork can be renamed).
    let (status, json) = send_json(authorize(ctx, http.post(format!("{api}/repos/{project}/forks"))))
        .await
        .map_err(|detail| AnnounceError::Fork { detail })?;
    if !status.is_success() {
        return Err(AnnounceError::Fork {
            detail: format!("POST forks returned HTTP status {status}"),
        });
    }
    let mut target =
        github_fork_target(&json, project, host, &login).map_err(|detail| AnnounceError::Fork { detail })?;
    // ponytail: a fork renamed away from the basename probes 404 above, so it
    // reaches this POST and reports created=true even though it pre-existed —
    // accepted ceiling; GitHub's fork POST is idempotent, so it's a reuse
    // reported as a create. Upgrade: match the fork by parent among the user's
    // repos if the created flag ever needs to be exact for renamed forks.
    target.created = true;
    wait_ready(ctx, &format!("{api}/repos/{}", target.full_name), |_| Readiness::Ready).await?;
    Ok(Some(target))
}

/// GitLab fork flow: probe push permission → create the fork (201) or adopt
/// an existing one (409 → look it up) → poll `import_status == finished`.
async fn gitlab_ensure_fork(
    http: &reqwest::Client,
    ctx: &ForgeContext,
    api: &str,
    project: &str,
    host: &str,
    force: bool,
) -> Result<Option<ForkTarget>, AnnounceError> {
    let encoded = encode_segment(project);
    let upstream = match get_json(http, ctx, &format!("{api}/projects/{encoded}")).await {
        Ok(upstream) => upstream,
        Err(detail) => {
            log_probe_failed(project, &detail);
            return Ok(None);
        }
    };
    // `force` (`[announce] fork = "always"`) forks even with push access.
    if !force && gitlab_can_push(&upstream) != Some(false) {
        log_already_pushable(project);
        return Ok(None);
    }
    let upstream_id = upstream
        .get("id")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| AnnounceError::Fork {
            detail: "upstream project response carries no id".to_string(),
        })?;
    // The authenticated user anchors the fork-namespace binding on both the
    // create and reuse paths (D4) — resolve it, and refuse to fork at all
    // when it's unknown, BEFORE the POST below. A fork created with no
    // verifiable namespace is an orphan side effect, not a safe push target;
    // degrading here (like a missing token) is strictly better than creating
    // one and only noticing afterward.
    let Some(user) = gitlab_current_user(http, ctx).await else {
        tracing::info!(
            "fork of '{project}' skipped because the authenticated username is unknown — degrading the announce to the upstream push"
        );
        return Ok(None);
    };
    // Self-fork guard (mirror of GitHub's): a project cannot be forked into
    // the namespace it already occupies — GitLab 409s, and the reuse path
    // below finds nothing because no project is a fork of itself. Only
    // reachable under `force`, since owning the project implies push access.
    // Compared case-insensitively like every other identity check on this
    // path (see `verify_fork_push_url`): the namespace comes from the
    // publisher's manifest, the username from the API, and GitLab routes
    // both case-insensitively.
    if project
        .split('/')
        .next()
        .is_some_and(|ns| ns.eq_ignore_ascii_case(&user))
    {
        tracing::debug!(
            "authenticated user '{user}' owns the upstream '{project}' — a self-fork is impossible, degrading to the upstream push"
        );
        return Ok(None);
    }

    let (status, json) = send_json(authorize(ctx, http.post(format!("{api}/projects/{encoded}/fork"))))
        .await
        .map_err(|detail| AnnounceError::Fork { detail })?;
    if status.as_u16() == 409 {
        // The fork already exists. Enumerate the upstream's forks and adopt
        // the one in the authenticated user's namespace (created = false) —
        // robust to a renamed or group-namespaced fork that the old
        // `{user}/{basename}` path guess would miss (ADR D6), and to a
        // concurrently-created fork the listing hasn't caught up to yet
        // (GitLab's eventual consistency) via the bounded retry in
        // `gitlab_find_owned_fork`. The D4 source-lineage guard and the
        // push-URL identity binding still gate it via `gitlab_select_fork` →
        // `gitlab_fork_target`.
        let target = gitlab_find_owned_fork(ctx, api, upstream_id, host, &user).await?;
        let fork_id = target.fork_project_id.ok_or_else(|| AnnounceError::Fork {
            detail: "selected fork response carries no project id".to_string(),
        })?;
        wait_ready(ctx, &format!("{api}/projects/{fork_id}"), gitlab_import_readiness).await?;
        return Ok(Some(target));
    }
    if !status.is_success() {
        return Err(AnnounceError::Fork {
            detail: format!("POST fork returned HTTP status {status}"),
        });
    }
    let mut target =
        gitlab_fork_target(&json, upstream_id, host, &user).map_err(|detail| AnnounceError::Fork { detail })?;
    target.created = true;
    wait_ready(
        ctx,
        &format!("{api}/projects/{}", encode_segment(&target.full_name)),
        gitlab_import_readiness,
    )
    .await?;
    Ok(Some(target))
}

/// GitHub push permission from a repository object: `.permissions.push`.
/// `None` when the field is absent (an unauthenticated or scope-limited
/// response) — ambiguous, so the caller degrades rather than forking.
fn github_can_push(repo: &serde_json::Value) -> Option<bool> {
    repo.get("permissions")
        .and_then(|p| p.get("push"))
        .and_then(serde_json::Value::as_bool)
}

/// GitLab push permission from a project object: Developer access (level
/// ≥ 30) via either `project_access` or `group_access`. `None` when neither
/// access level is present (a public project viewed without membership) —
/// ambiguous, so the caller degrades rather than forking.
fn gitlab_can_push(project: &serde_json::Value) -> Option<bool> {
    let level = |key: &str| {
        project
            .get("permissions")
            .and_then(|p| p.get(key))
            .and_then(|a| a.get("access_level"))
            .and_then(serde_json::Value::as_u64)
    };
    match (level("project_access"), level("group_access")) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0).max(b.unwrap_or(0)) >= 30),
    }
}

/// Bind a fork push URL to the verified fork identity before it becomes a
/// `git push` target.
///
/// The scheme guard alone is insufficient: a hostile or self-hosted forge
/// controls the whole response and can pass the parent/source guard while
/// pointing the push URL at `https://<same-host>/victim/other.git`. grim would
/// then force-push the announce branch there with the host-scoped git
/// credential — an unauthorized cross-repo write (CWE-441 / CWE-345). Worse, a
/// non-https value like `ext::sh -c '…'` reaches git's remote-helper transport
/// and executes an arbitrary command (RCE). So the push URL must be all of:
/// (1) https; (2) on the trusted upstream host; (3) the response's own fork
/// identity; (4) inside the authenticated user's namespace.
///
/// `trusted_host` and `login` are the two anchors that come from **outside**
/// the response: the publisher-configured index URL host and the authenticated
/// `github_login` / `gitlab_current_user`. The GitLab `login` binding shares
/// the `gitlab_current_user` identity check that the deferred GitLab
/// identity-based fork-reuse work will also key on.
fn verify_fork_push_url(push_url: &str, identity: &str, trusted_host: &str, login: &str) -> Result<(), String> {
    let rest = push_url.strip_prefix("https://").ok_or_else(|| {
        format!("fork push URL '{push_url}' is not an https URL — refusing to push to an unexpected transport")
    })?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    if !host.eq_ignore_ascii_case(trusted_host) {
        return Err(format!(
            "fork push URL host '{host}' is not the index host '{trusted_host}' — refusing to push to an unrelated host"
        ));
    }
    let path = path.strip_suffix(".git").unwrap_or(path);
    if !path.eq_ignore_ascii_case(identity) {
        return Err(format!(
            "fork push URL path '{path}' does not match the fork identity '{identity}' — refusing to push to an unrelated repository"
        ));
    }
    let namespace_root = identity.split('/').next().unwrap_or_default();
    if !namespace_root.eq_ignore_ascii_case(login) {
        return Err(format!(
            "fork namespace '{namespace_root}' is not the authenticated user '{login}' — refusing to push outside your namespace"
        ));
    }
    Ok(())
}

/// Parse a GitHub fork response into a [`ForkTarget`]. Security guards: the
/// fork's `parent.full_name` must equal `upstream` (case-insensitive) —
/// otherwise this is a same-named stranger repository — and the push URL must
/// bind to the fork identity on the trusted host in the authenticated user's
/// namespace ([`verify_fork_push_url`]).
fn github_fork_target(
    fork: &serde_json::Value,
    upstream: &str,
    trusted_host: &str,
    login: &str,
) -> Result<ForkTarget, String> {
    let field = |key: &str| fork.get(key).and_then(serde_json::Value::as_str);
    match fork
        .get("parent")
        .and_then(|p| p.get("full_name"))
        .and_then(serde_json::Value::as_str)
    {
        Some(parent) if parent.eq_ignore_ascii_case(upstream) => {}
        Some(parent) => {
            return Err(format!(
                "fork parent '{parent}' does not match upstream '{upstream}' — refusing to push to an unrelated repository"
            ));
        }
        None => {
            return Err("fork response carries no parent — refusing to push to an unverified repository".to_string());
        }
    }
    let full_name = field("full_name").ok_or("fork response carries no full_name")?;
    let push_url = field("clone_url").ok_or("fork response carries no clone_url")?;
    verify_fork_push_url(push_url, full_name, trusted_host, login)?;
    let head_owner = fork
        .get("owner")
        .and_then(|o| o.get("login"))
        .and_then(serde_json::Value::as_str)
        .ok_or("fork response carries no owner login")?;
    Ok(ForkTarget {
        push_url: push_url.to_string(),
        head_owner: head_owner.to_string(),
        full_name: full_name.to_string(),
        fork_project_id: None,
        upstream_project_id: None,
        created: false,
    })
}

/// Parse a GitLab fork response into a [`ForkTarget`]. Security guards: the
/// fork's `forked_from_project.id` must equal the upstream project id, and the
/// push URL must bind to the fork identity on the trusted host in the
/// authenticated user's namespace ([`verify_fork_push_url`]).
fn gitlab_fork_target(
    fork: &serde_json::Value,
    upstream_id: u64,
    trusted_host: &str,
    login: &str,
) -> Result<ForkTarget, String> {
    match fork
        .get("forked_from_project")
        .and_then(|p| p.get("id"))
        .and_then(serde_json::Value::as_u64)
    {
        Some(id) if id == upstream_id => {}
        Some(id) => {
            return Err(format!(
                "fork source project {id} does not match upstream {upstream_id} — refusing to push to an unrelated project"
            ));
        }
        None => {
            return Err(
                "fork response carries no forked_from_project — refusing to push to an unverified project".to_string(),
            );
        }
    }
    let full_name = fork
        .get("path_with_namespace")
        .and_then(serde_json::Value::as_str)
        .ok_or("fork response carries no path_with_namespace")?;
    let push_url = fork
        .get("http_url_to_repo")
        .and_then(serde_json::Value::as_str)
        .ok_or("fork response carries no http_url_to_repo")?;
    verify_fork_push_url(push_url, full_name, trusted_host, login)?;
    let id = fork
        .get("id")
        .and_then(serde_json::Value::as_u64)
        .ok_or("fork response carries no id")?;
    let head_owner = full_name.split('/').next().unwrap_or(full_name);
    Ok(ForkTarget {
        push_url: push_url.to_string(),
        head_owner: head_owner.to_string(),
        full_name: full_name.to_string(),
        fork_project_id: Some(id),
        upstream_project_id: Some(upstream_id),
        created: false,
    })
}

/// GitLab caps `per_page` at 100; `owned=true` narrows the forks listing to
/// forks the authenticated user owns — a user can fork a project at most once
/// into their own namespace — so the reuse target is a tiny, page-1 set.
const FORK_ENUM_PER_PAGE: usize = 100;
/// Page ceiling for the fork enumeration.
// ponytail: 10-page cap (≤1000 forks scanned). owned=true already guarantees
// the target is on page 1 for v1 (personal namespace) — the loop is
// belt-and-suspenders for an odd server default or a future group-namespace
// lookup. Raise if a group lookup ever needs to scan a larger owned set.
const FORK_ENUM_MAX_PAGES: u32 = 10;

/// The forks-listing URL for `page`: `owned=true` (server-side filter to the
/// authenticated user's forks, pagination-proof for the personal-namespace
/// reuse target) at the 100/page maximum.
fn forks_page_url(api: &str, upstream_id: u64, page: u32) -> String {
    format!("{api}/projects/{upstream_id}/forks?owned=true&per_page={FORK_ENUM_PER_PAGE}&page={page}")
}

/// Whether `forks` is the last page (a page shorter than the requested
/// `per_page`, or not an array at all — stop paging either way).
fn is_last_page(forks: &serde_json::Value) -> bool {
    forks.as_array().is_none_or(|page| page.len() < FORK_ENUM_PER_PAGE)
}

/// Enumeration-retry bounds for the 409 "fork already exists" reuse path
/// ([`gitlab_find_owned_fork`]): GitLab's forks listing can lag a
/// concurrently-created fork by a few seconds (eventual consistency), so a
/// single enumeration pass can legitimately find nothing yet — short
/// exponential backoff under a bounded deadline gives the listing time to
/// catch up without hanging the announce on a fork that truly doesn't exist.
const FORK_ENUM_RETRY_INITIAL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);
const FORK_ENUM_RETRY_MAX_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
const FORK_ENUM_RETRY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// Find the authenticated user's fork of `upstream_id`, retrying the
/// enumeration under the default enumeration-retry [`PollBounds`]. See
/// [`gitlab_find_owned_fork_bounded`].
async fn gitlab_find_owned_fork(
    ctx: &ForgeContext,
    api: &str,
    upstream_id: u64,
    trusted_host: &str,
    user: &str,
) -> Result<ForkTarget, AnnounceError> {
    let bounds = PollBounds {
        initial_interval: FORK_ENUM_RETRY_INITIAL_INTERVAL,
        max_interval: FORK_ENUM_RETRY_MAX_INTERVAL,
        deadline: FORK_ENUM_RETRY_DEADLINE,
        request_timeout: FORK_POLL_REQUEST_TIMEOUT,
    };
    gitlab_find_owned_fork_bounded(bounds, ctx, api, upstream_id, trusted_host, user).await
}

/// Find the authenticated user's fork of `upstream_id` by enumerating the
/// upstream's forks (`owned=true`, 100/page), following pages until a match
/// or the list is exhausted, and retrying the whole scan under `bounds` — the
/// same [`next_interval`] exponential backoff [`poll_until_ready`] uses —
/// until a match appears or the deadline runs out. This is what makes the
/// reuse path actually robust to a concurrently-created fork: the listing
/// may not include it yet on the first pass. `owned=true` makes the
/// personal-namespace target pagination-proof; the page loop covers an odd
/// server default / future group lookup. Selection funnels through
/// [`gitlab_select_fork`] → the full D4 source guard +
/// [`verify_fork_push_url`], unchanged. A dedicated `bounds` parameter lets a
/// test drive the retry on a short deadline without real wall-clock waits.
/// Builds its own `bounds.request_timeout`-bounded client for the
/// enumeration GETs (mirroring [`poll_until_ready`]) so a single black-holed
/// page request cannot stall past `bounds.deadline` on the shared client's
/// longer timeout.
async fn gitlab_find_owned_fork_bounded(
    bounds: PollBounds,
    ctx: &ForgeContext,
    api: &str,
    upstream_id: u64,
    trusted_host: &str,
    user: &str,
) -> Result<ForkTarget, AnnounceError> {
    let http = build_client(bounds.request_timeout).map_err(|e| AnnounceError::Fork {
        detail: format!("could not build the fork-enumeration client: {e}"),
    })?;
    let deadline = std::time::Instant::now() + bounds.deadline;
    let mut interval = bounds.initial_interval;
    loop {
        for page in 1..=FORK_ENUM_MAX_PAGES {
            let forks = get_json(&http, ctx, &forks_page_url(api, upstream_id, page))
                .await
                .map_err(|detail| AnnounceError::Fork {
                    detail: format!("existing fork enumeration failed: {detail}"),
                })?;
            if let Some(target) = gitlab_select_fork(&forks, upstream_id, trusted_host, user) {
                return Ok(target);
            }
            if is_last_page(&forks) {
                break;
            }
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(AnnounceError::Fork {
                detail: format!(
                    "no fork of upstream project {upstream_id} found in the authenticated namespace '{user}' after retrying the forks listing for {}s",
                    bounds.deadline.as_secs()
                ),
            });
        }
        tokio::time::sleep(interval.min(remaining)).await;
        interval = next_interval(interval, bounds.max_interval);
    }
}

/// Select the authenticated user's fork of `upstream_id` from one page of an
/// upstream project's forks listing. Filters to the user's own namespace, then
/// re-applies the full create-path guards ([`gitlab_fork_target`]:
/// `forked_from_project.id` must equal `upstream_id`, and the push URL must
/// bind to the fork identity on the trusted host — [`verify_fork_push_url`]).
/// `None` when this page holds no match. Robust to a renamed or
/// concurrently-created fork the old `{user}/{basename}` guess would miss.
fn gitlab_select_fork(
    forks: &serde_json::Value,
    upstream_id: u64,
    trusted_host: &str,
    user: &str,
) -> Option<ForkTarget> {
    forks
        .as_array()
        .into_iter()
        .flatten()
        .filter(|fork| {
            fork.get("path_with_namespace")
                .and_then(serde_json::Value::as_str)
                .and_then(|path| path.split('/').next())
                .is_some_and(|namespace| namespace.eq_ignore_ascii_case(user))
        })
        .find_map(|fork| gitlab_fork_target(fork, upstream_id, trusted_host, user).ok())
}

/// Poll `url` until `check` reports [`Readiness::Ready`], using the default
/// [`PollBounds`]. See [`poll_until_ready`].
async fn wait_ready(
    ctx: &ForgeContext,
    url: &str,
    check: impl Fn(&serde_json::Value) -> Readiness,
) -> Result<(), AnnounceError> {
    poll_until_ready(PollBounds::default(), ctx, url, check).await
}

/// Poll `url` until `check` reports [`Readiness::Ready`], on an exponential
/// backoff bounded by `bounds.deadline`. Each request uses a short-timeout
/// client so one black-holed attempt cannot consume the whole deadline; a
/// transient transport/parse failure is retried, a [`Readiness::Failed`]
/// fast-fails, and exhausting the deadline yields [`AnnounceError::Fork`]. A
/// single `tracing::warn!` announces the wait before the first sleep so it
/// does not read as a hang at the default log level; the per-attempt detail
/// is `tracing::info!`, visible with `GRIM_LOG=info`.
async fn poll_until_ready(
    bounds: PollBounds,
    ctx: &ForgeContext,
    url: &str,
    check: impl Fn(&serde_json::Value) -> Readiness,
) -> Result<(), AnnounceError> {
    let poll = build_client(bounds.request_timeout).map_err(|e| AnnounceError::Fork {
        detail: format!("could not build the fork-readiness client: {e}"),
    })?;
    let deadline = std::time::Instant::now() + bounds.deadline;
    let mut interval = bounds.initial_interval;
    let mut attempt: usize = 0;
    loop {
        attempt += 1;
        tracing::info!("polling fork readiness (attempt {attempt}): {url}");
        // A transient transport/parse failure is not fatal (the fork may simply
        // not be answering yet); only a decoded body decides readiness. Keep
        // polling either way until the deadline below.
        if let Ok(body) = get_json(&poll, ctx, url).await {
            match check(&body) {
                Readiness::Ready => return Ok(()),
                Readiness::Failed(cause) => {
                    return Err(AnnounceError::Fork {
                        detail: format!("fork import failed: {cause}"),
                    });
                }
                Readiness::Pending => {}
            }
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(AnnounceError::Fork {
                detail: format!("fork not ready within {}s: {url}", bounds.deadline.as_secs()),
            });
        }
        if attempt == 1 {
            // The per-attempt line above is `info`, which the default `warn`
            // filter hides — so without this the wait is a silent stall of up
            // to `deadline`. Announced once, not per attempt, to stay quiet.
            tracing::warn!(
                "waiting for the fork to become ready (up to {}s): {url}",
                bounds.deadline.as_secs()
            );
        }
        tokio::time::sleep(interval.min(remaining)).await;
        interval = next_interval(interval, bounds.max_interval);
    }
}

/// Send `request`, returning `(status, json-body-or-null)`. A transport
/// failure is `Err`; a non-2xx status is a successful send with the status
/// carried for the caller to interpret (GitLab's 409 fork-exists, say).
async fn send_json(request: reqwest::RequestBuilder) -> Result<(reqwest::StatusCode, serde_json::Value), String> {
    let response = request.send().await.map_err(|e| e.to_string())?;
    let status = response.status();
    let body = response
        .json::<serde_json::Value>()
        .await
        .unwrap_or(serde_json::Value::Null);
    Ok((status, body))
}

/// GET `url`, requiring a success status. Any non-success or transport
/// failure is `Err` — the fork probe treats it as ambiguous (degrade), the
/// readiness poll as not-yet-ready.
async fn get_json(http: &reqwest::Client, ctx: &ForgeContext, url: &str) -> Result<serde_json::Value, String> {
    let (status, body) = send_json(authorize(ctx, http.get(url))).await?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(format!("HTTP status {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gitlab_ci_env(host: &str) -> CiEnv {
        CiEnv {
            gitlab_ci: true,
            ci_server_host: Some(host.to_string()),
            ci_api_v4_url: Some(format!("https://{host}/api/v4")),
            ci_project_namespace: Some("platform".to_string()),
            gitlab_token: Some("glpat-x".to_string()),
            ..CiEnv::default()
        }
    }

    fn github_actions_env(server: &str) -> CiEnv {
        CiEnv {
            github_actions: true,
            github_server_url: Some(server.to_string()),
            github_api_url: Some(format!("{server}/api/v3")),
            github_repository_owner: Some("acme".to_string()),
            github_token: Some("ghp-x".to_string()),
            ..CiEnv::default()
        }
    }

    #[test]
    fn explicit_forge_wins_over_ci_and_convention() {
        let ctx = resolve(
            Some(ForgeKind::Plain),
            None,
            "gitlab.example.com",
            &gitlab_ci_env("gitlab.example.com"),
        );
        assert_eq!(ctx.kind, ForgeKind::Plain);
        assert_eq!(ctx.api_url, None, "plain forge never carries an API URL");
        assert_eq!(ctx.ci_namespace, None, "kind-mismatched CI contributes nothing");
    }

    #[test]
    fn gitlab_ci_matching_host_contributes_api_token_namespace() {
        let ctx = resolve(None, None, "gitlab.example.com", &gitlab_ci_env("gitlab.example.com"));
        assert_eq!(ctx.kind, ForgeKind::GitLab);
        assert_eq!(ctx.api_url.as_deref(), Some("https://gitlab.example.com/api/v4"));
        assert_eq!(ctx.token.as_deref(), Some("glpat-x"));
        assert_eq!(ctx.ci_namespace.as_deref(), Some("platform"));
    }

    #[test]
    fn ci_host_mismatch_contributes_nothing() {
        // A GitLab pipeline announcing to a github.com index must not
        // inherit GitLab credentials or API config.
        let ctx = resolve(None, None, "github.com", &gitlab_ci_env("gitlab.example.com"));
        assert_eq!(ctx.kind, ForgeKind::GitHub, "github.com convention still applies");
        assert_eq!(ctx.api_url.as_deref(), Some("https://api.github.com"));
        assert_eq!(ctx.token, None);
        assert_eq!(ctx.ci_namespace, None);
    }

    #[test]
    fn github_actions_on_enterprise_host_matches() {
        let ctx = resolve(
            None,
            None,
            "github.example.corp",
            &github_actions_env("https://github.example.corp"),
        );
        assert_eq!(ctx.kind, ForgeKind::GitHub);
        assert_eq!(ctx.api_url.as_deref(), Some("https://github.example.corp/api/v3"));
        assert_eq!(ctx.token.as_deref(), Some("ghp-x"));
        assert_eq!(ctx.ci_namespace.as_deref(), Some("acme"));
    }

    #[test]
    fn enterprise_github_api_convention_without_ci() {
        let ctx = resolve(Some(ForgeKind::GitHub), None, "github.example.corp", &CiEnv::default());
        assert_eq!(ctx.api_url.as_deref(), Some("https://github.example.corp/api/v3"));
        assert_eq!(ctx.token, None);
    }

    #[test]
    fn announce_token_beats_ci_token_and_api_override_beats_ci() {
        let mut env = gitlab_ci_env("gitlab.example.com");
        env.announce_token = Some("explicit".to_string());
        let ctx = resolve(
            None,
            Some("https://proxy.example.com/api/v4".to_string()),
            "gitlab.example.com",
            &env,
        );
        assert_eq!(ctx.token.as_deref(), Some("explicit"));
        assert_eq!(ctx.api_url.as_deref(), Some("https://proxy.example.com/api/v4"));
    }

    #[test]
    fn unknown_host_without_ci_resolves_plain() {
        let ctx = resolve(None, None, "git.example.test", &CiEnv::default());
        assert_eq!(ctx.kind, ForgeKind::Plain);
        assert_eq!(ctx.api_url, None);
        assert_eq!(ctx.token, None);
    }

    #[test]
    fn announce_token_applies_even_without_ci() {
        let env = CiEnv {
            announce_token: Some("t".to_string()),
            ..CiEnv::default()
        };
        let ctx = resolve(Some(ForgeKind::GitLab), None, "gitlab.example.com", &env);
        assert_eq!(ctx.token.as_deref(), Some("t"));
    }

    #[test]
    fn encode_segment_escapes_slashes_and_specials() {
        assert_eq!(encode_segment("platform/ai"), "platform%2Fai");
        assert_eq!(encode_segment("a-b.c_d~e"), "a-b.c_d~e");
        assert_eq!(encode_segment("sp ace"), "sp%20ace");
    }

    #[test]
    fn project_path_from_url_shapes() {
        assert_eq!(
            project_path("https://gitlab.example.com/platform/ai/index.git").as_deref(),
            Some("platform/ai/index")
        );
        assert_eq!(
            project_path("git+ssh://git@gitlab.example.com:2222/platform/index.git").as_deref(),
            Some("platform/index")
        );
        assert_eq!(project_path("/tmp/local-index.git"), None);
    }

    #[test]
    fn host_of_url_handles_bare_server_urls() {
        assert_eq!(host_of_url("https://github.com").as_deref(), Some("github.com"));
        assert_eq!(
            host_of_url("https://github.example.corp/").as_deref(),
            Some("github.example.corp")
        );
    }

    #[test]
    fn github_can_push_reads_permission_tri_state() {
        assert_eq!(
            github_can_push(&serde_json::json!({ "permissions": { "push": true } })),
            Some(true)
        );
        assert_eq!(
            github_can_push(&serde_json::json!({ "permissions": { "push": false } })),
            Some(false)
        );
        // Permissions absent (unauthenticated / scope-limited) is ambiguous.
        assert_eq!(github_can_push(&serde_json::json!({ "default_branch": "main" })), None);
        assert_eq!(github_can_push(&serde_json::json!({ "permissions": {} })), None);
    }

    #[test]
    fn gitlab_can_push_reads_access_level_tri_state() {
        // Developer (30) on either project or group access ⇒ can push.
        assert_eq!(
            gitlab_can_push(
                &serde_json::json!({ "permissions": { "project_access": { "access_level": 30 }, "group_access": null } })
            ),
            Some(true)
        );
        assert_eq!(
            gitlab_can_push(
                &serde_json::json!({ "permissions": { "project_access": null, "group_access": { "access_level": 40 } } })
            ),
            Some(true)
        );
        // Reporter (20) only ⇒ definitively no push.
        assert_eq!(
            gitlab_can_push(
                &serde_json::json!({ "permissions": { "project_access": { "access_level": 20 }, "group_access": null } })
            ),
            Some(false)
        );
        // No access levels visible (public project, no membership) ⇒ ambiguous.
        assert_eq!(
            gitlab_can_push(&serde_json::json!({ "permissions": { "project_access": null, "group_access": null } })),
            None
        );
        assert_eq!(gitlab_can_push(&serde_json::json!({ "id": 1 })), None);
    }

    #[test]
    fn pr_head_uses_owner_prefix_only_when_forking() {
        let fork = ForkTarget {
            push_url: "https://github.com/forkuser/index.git".to_string(),
            head_owner: "forkuser".to_string(),
            full_name: "forkuser/index".to_string(),
            fork_project_id: None,
            upstream_project_id: None,
            created: true,
        };
        assert_eq!(
            pr_head(Some(&fork), "announce/acme-1234"),
            "forkuser:announce/acme-1234"
        );
        // Non-fork head stays the bare branch (byte-identical to today).
        assert_eq!(pr_head(None, "announce/acme-1234"), "announce/acme-1234");
    }

    #[test]
    fn github_fork_target_parses_and_guards_parent() {
        let body = serde_json::json!({
            "full_name": "forkuser/index",
            "clone_url": "https://github.com/forkuser/index.git",
            "owner": { "login": "forkuser" },
            "parent": { "full_name": "acme/index" },
        });
        let target = github_fork_target(&body, "acme/index", "github.com", "forkuser").expect("verified fork");
        assert_eq!(target.push_url, "https://github.com/forkuser/index.git");
        assert_eq!(target.head_owner, "forkuser");
        assert_eq!(target.full_name, "forkuser/index");
        assert_eq!(target.fork_project_id, None);
        assert_eq!(target.upstream_project_id, None);
        assert!(!target.created, "the parser never asserts creation");

        // Parent case-insensitivity (GitHub owners are case-insensitive).
        assert!(github_fork_target(&body, "ACME/index", "github.com", "forkuser").is_ok());

        // A same-named stranger repository (wrong parent) is rejected.
        let stranger = serde_json::json!({
            "full_name": "forkuser/index",
            "clone_url": "https://github.com/forkuser/index.git",
            "owner": { "login": "forkuser" },
            "parent": { "full_name": "stranger/index" },
        });
        assert!(github_fork_target(&stranger, "acme/index", "github.com", "forkuser").is_err());

        // No parent at all ⇒ unverified ⇒ rejected.
        let no_parent = serde_json::json!({
            "full_name": "forkuser/index",
            "clone_url": "https://github.com/forkuser/index.git",
            "owner": { "login": "forkuser" },
        });
        assert!(github_fork_target(&no_parent, "acme/index", "github.com", "forkuser").is_err());

        // A verified parent but a hostile clone_url scheme is rejected: an
        // `ext::` push URL reaches git's remote-helper transport (RCE), and a
        // `-`-leading value could be parsed as a git flag. Only https is a
        // legitimate fork push URL.
        for hostile in [
            "ext::sh -c 'touch /tmp/pwn'",
            "-oProxyCommand=evil",
            "http://github.com/forkuser/index.git",
        ] {
            let malicious = serde_json::json!({
                "full_name": "forkuser/index",
                "clone_url": hostile,
                "owner": { "login": "forkuser" },
                "parent": { "full_name": "acme/index" },
            });
            assert!(
                github_fork_target(&malicious, "acme/index", "github.com", "forkuser").is_err(),
                "non-https clone_url {hostile:?} must be rejected"
            );
        }
    }

    #[test]
    fn github_fork_target_binds_push_url_to_verified_identity() {
        // Even with a valid parent, the push URL must resolve to the trusted
        // host, the response's own identity, and the authenticated user's
        // namespace — otherwise a hostile forge redirects the force-push with
        // the host-scoped credential to an unrelated repo (CWE-441/CWE-345).
        let identity = serde_json::json!({
            "full_name": "forkuser/index",
            "owner": { "login": "forkuser" },
            "parent": { "full_name": "acme/index" },
        });
        let with_clone = |clone_url: &str| {
            let mut v = identity.clone();
            v["clone_url"] = serde_json::Value::from(clone_url);
            v
        };

        // (a) same host, different repo than the fork identity ⇒ rejected.
        assert!(
            github_fork_target(
                &with_clone("https://github.com/forkuser/other.git"),
                "acme/index",
                "github.com",
                "forkuser"
            )
            .is_err()
        );
        // (b) cross-host push URL ⇒ rejected.
        assert!(
            github_fork_target(
                &with_clone("https://evil.com/forkuser/index.git"),
                "acme/index",
                "github.com",
                "forkuser"
            )
            .is_err()
        );
        // (c) identity in a namespace other than the authenticated user ⇒ rejected.
        let stranger_ns = serde_json::json!({
            "full_name": "attacker/index",
            "clone_url": "https://github.com/attacker/index.git",
            "owner": { "login": "attacker" },
            "parent": { "full_name": "acme/index" },
        });
        assert!(github_fork_target(&stranger_ns, "acme/index", "github.com", "forkuser").is_err());
        // (d) a legitimate fork URL is accepted.
        assert!(
            github_fork_target(
                &with_clone("https://github.com/forkuser/index.git"),
                "acme/index",
                "github.com",
                "forkuser"
            )
            .is_ok()
        );
    }

    #[test]
    fn github_fork_target_uses_response_full_name_for_renamed_fork() {
        // Real case: upstream repo is `index`, the user's fork is renamed
        // `grimoire-index`. The target must come from the response body, not
        // a `{login}/{basename}` guess. (e) same owner, different basename is
        // accepted — the binding constrains the namespace, not the basename.
        let renamed = serde_json::json!({
            "full_name": "forkuser/grimoire-index",
            "clone_url": "https://github.com/forkuser/grimoire-index.git",
            "owner": { "login": "forkuser" },
            "parent": { "full_name": "acme/index" },
        });
        let target =
            github_fork_target(&renamed, "acme/index", "github.com", "forkuser").expect("verified renamed fork");
        assert_eq!(target.full_name, "forkuser/grimoire-index");
        assert_eq!(target.push_url, "https://github.com/forkuser/grimoire-index.git");
        assert_eq!(target.head_owner, "forkuser");
    }

    #[test]
    fn gitlab_fork_target_parses_and_guards_source() {
        let body = serde_json::json!({
            "id": 200,
            "path_with_namespace": "forkuser/index",
            "http_url_to_repo": "https://gitlab.example.com/forkuser/index.git",
            "forked_from_project": { "id": 100 },
            "import_status": "finished",
        });
        let target = gitlab_fork_target(&body, 100, "gitlab.example.com", "forkuser").expect("verified fork");
        assert_eq!(target.push_url, "https://gitlab.example.com/forkuser/index.git");
        assert_eq!(target.head_owner, "forkuser");
        assert_eq!(target.full_name, "forkuser/index");
        assert_eq!(target.fork_project_id, Some(200));
        assert_eq!(target.upstream_project_id, Some(100));

        // Wrong source project (a stranger's project) is rejected.
        let stranger = serde_json::json!({
            "id": 200,
            "path_with_namespace": "forkuser/index",
            "http_url_to_repo": "https://gitlab.example.com/forkuser/index.git",
            "forked_from_project": { "id": 999 },
        });
        assert!(gitlab_fork_target(&stranger, 100, "gitlab.example.com", "forkuser").is_err());

        // No forked_from_project ⇒ unverified ⇒ rejected.
        let unverified = serde_json::json!({
            "id": 200,
            "path_with_namespace": "forkuser/index",
            "http_url_to_repo": "https://gitlab.example.com/forkuser/index.git",
        });
        assert!(gitlab_fork_target(&unverified, 100, "gitlab.example.com", "forkuser").is_err());

        // A verified source project but a hostile http_url_to_repo scheme is
        // rejected: an `ext::` push URL reaches git's remote-helper transport
        // (RCE), and a `-`-leading value could be parsed as a git flag. Only
        // https is a legitimate fork push URL.
        for hostile in [
            "ext::sh -c 'touch /tmp/pwn'",
            "-oProxyCommand=evil",
            "http://gitlab.example.com/forkuser/index.git",
        ] {
            let malicious = serde_json::json!({
                "id": 200,
                "path_with_namespace": "forkuser/index",
                "http_url_to_repo": hostile,
                "forked_from_project": { "id": 100 },
            });
            assert!(
                gitlab_fork_target(&malicious, 100, "gitlab.example.com", "forkuser").is_err(),
                "non-https http_url_to_repo {hostile:?} must be rejected"
            );
        }
    }

    #[test]
    fn gitlab_fork_target_binds_push_url_to_verified_identity() {
        // Mirror of the GitHub identity binding: a valid source project is not
        // enough — the push URL must resolve to the trusted host, the response
        // identity, and the authenticated user's namespace.
        let verified = |http_url: &str, path_with_namespace: &str| {
            serde_json::json!({
                "id": 200,
                "path_with_namespace": path_with_namespace,
                "http_url_to_repo": http_url,
                "forked_from_project": { "id": 100 },
            })
        };

        // (a) same host, different repo than the identity ⇒ rejected.
        assert!(
            gitlab_fork_target(
                &verified("https://gitlab.example.com/forkuser/other.git", "forkuser/index"),
                100,
                "gitlab.example.com",
                "forkuser",
            )
            .is_err()
        );
        // (b) cross-host push URL ⇒ rejected.
        assert!(
            gitlab_fork_target(
                &verified("https://evil.example.com/forkuser/index.git", "forkuser/index"),
                100,
                "gitlab.example.com",
                "forkuser",
            )
            .is_err()
        );
        // (c) identity in a namespace other than the authenticated user ⇒ rejected.
        assert!(
            gitlab_fork_target(
                &verified("https://gitlab.example.com/attacker/index.git", "attacker/index"),
                100,
                "gitlab.example.com",
                "forkuser",
            )
            .is_err()
        );
        // (d) a legitimate fork URL is accepted; (e) a renamed fork (same
        // owner, different basename) is accepted too.
        assert!(
            gitlab_fork_target(
                &verified("https://gitlab.example.com/forkuser/index.git", "forkuser/index"),
                100,
                "gitlab.example.com",
                "forkuser",
            )
            .is_ok()
        );
        assert!(
            gitlab_fork_target(
                &verified(
                    "https://gitlab.example.com/forkuser/grimoire-index.git",
                    "forkuser/grimoire-index"
                ),
                100,
                "gitlab.example.com",
                "forkuser",
            )
            .is_ok()
        );
    }

    #[test]
    fn authorize_attaches_forge_specific_auth_header() {
        let http = client().expect("client builds");
        let ctx = |kind, token: Option<&str>| ForgeContext {
            kind,
            api_url: Some("https://forge.example.com/api".to_string()),
            token: token.map(str::to_string),
            ci_namespace: None,
        };

        // GitHub ⇒ Bearer + the GitHub Accept header.
        let req = authorize(
            &ctx(ForgeKind::GitHub, Some("ght")),
            http.get("https://forge.example.com/x"),
        )
        .build()
        .expect("request builds");
        assert_eq!(req.headers().get("Authorization").unwrap(), "Bearer ght");
        assert_eq!(req.headers().get("Accept").unwrap(), "application/vnd.github+json");

        // GitLab ⇒ PRIVATE-TOKEN, no Authorization, no GitHub Accept.
        let req = authorize(
            &ctx(ForgeKind::GitLab, Some("glt")),
            http.get("https://forge.example.com/x"),
        )
        .build()
        .expect("request builds");
        assert_eq!(req.headers().get("PRIVATE-TOKEN").unwrap(), "glt");
        assert!(req.headers().get("Authorization").is_none());

        // Plain ⇒ no auth header of any kind, even with a token present (the
        // exhaustive match must not fall through to sending it unauthenticated
        // OR authenticated under the wrong scheme).
        let req = authorize(
            &ctx(ForgeKind::Plain, Some("t")),
            http.get("https://forge.example.com/x"),
        )
        .build()
        .expect("request builds");
        assert!(req.headers().get("Authorization").is_none());
        assert!(req.headers().get("PRIVATE-TOKEN").is_none());

        // GitHub without a token ⇒ Accept only, never a bare Authorization.
        let req = authorize(&ctx(ForgeKind::GitHub, None), http.get("https://forge.example.com/x"))
            .build()
            .expect("request builds");
        assert!(req.headers().get("Authorization").is_none());
        assert_eq!(req.headers().get("Accept").unwrap(), "application/vnd.github+json");
    }

    #[test]
    fn gitlab_select_fork_matches_by_source_and_namespace() {
        // A stranger's fork of the same upstream plus the authenticated user's
        // own fork, renamed away from the upstream basename.
        let forks = serde_json::json!([
            {
                "id": 11,
                "path_with_namespace": "stranger/index",
                "http_url_to_repo": "https://gitlab.example.com/stranger/index.git",
                "forked_from_project": { "id": 100 },
            },
            {
                "id": 22,
                "path_with_namespace": "forkuser/grimoire-index",
                "http_url_to_repo": "https://gitlab.example.com/forkuser/grimoire-index.git",
                "forked_from_project": { "id": 100 },
            },
        ]);
        let target = gitlab_select_fork(&forks, 100, "gitlab.example.com", "forkuser").expect("selects own fork");
        assert_eq!(target.full_name, "forkuser/grimoire-index", "tolerates a renamed fork");
        assert_eq!(target.fork_project_id, Some(22));
        assert_eq!(target.upstream_project_id, Some(100));
        assert!(!target.created, "the reuse path never reports creation");

        // A fork in the user's namespace but of a different upstream ⇒ rejected
        // (the D4 source-lineage guard, re-applied via gitlab_fork_target).
        let wrong_source = serde_json::json!([
            {
                "id": 33,
                "path_with_namespace": "forkuser/index",
                "http_url_to_repo": "https://gitlab.example.com/forkuser/index.git",
                "forked_from_project": { "id": 999 },
            },
        ]);
        assert!(gitlab_select_fork(&wrong_source, 100, "gitlab.example.com", "forkuser").is_none());

        // No fork in the user's namespace (only a stranger's) ⇒ no match.
        let none_owned = serde_json::json!([
            {
                "id": 44,
                "path_with_namespace": "stranger/index",
                "http_url_to_repo": "https://gitlab.example.com/stranger/index.git",
                "forked_from_project": { "id": 100 },
            },
        ]);
        assert!(gitlab_select_fork(&none_owned, 100, "gitlab.example.com", "forkuser").is_none());
    }

    #[test]
    fn forks_enumeration_is_owned_scoped_and_paginated() {
        // owned=true makes the personal-namespace reuse target pagination-proof
        // (a user owns at most one fork of a project), at the 100/page maximum;
        // the page number advances so a match past page 1 is still reachable.
        let url = forks_page_url("https://gitlab.example.com/api/v4", 100, 2);
        assert!(url.contains("/projects/100/forks?"), "{url}");
        assert!(url.contains("owned=true"), "{url}");
        assert!(url.contains("per_page=100"), "{url}");
        assert!(url.contains("page=2"), "{url}");
    }

    #[test]
    fn is_last_page_stops_on_a_short_or_empty_page() {
        // A full 100-entry page might have a successor — keep paging.
        let full = serde_json::Value::Array(vec![serde_json::json!({}); 100]);
        assert!(!is_last_page(&full));
        // A short page is the last one; so is an empty page or a non-array body.
        assert!(is_last_page(&serde_json::json!([{}, {}, {}])));
        assert!(is_last_page(&serde_json::json!([])));
        assert!(is_last_page(&serde_json::json!({ "error": "nope" })));
    }

    #[test]
    fn gitlab_import_readiness_classifies_status() {
        assert!(matches!(
            gitlab_import_readiness(&serde_json::json!({ "import_status": "finished" })),
            Readiness::Ready
        ));
        assert!(matches!(
            gitlab_import_readiness(&serde_json::json!({ "import_status": "started" })),
            Readiness::Pending
        ));
        // Absent status is still "pending", not a fatal — GitHub responses
        // carry no import_status and rely on the always-ready check instead.
        assert!(matches!(
            gitlab_import_readiness(&serde_json::json!({ "id": 1 })),
            Readiness::Pending
        ));
        match gitlab_import_readiness(&serde_json::json!({ "import_status": "failed", "import_error": "disk full" })) {
            Readiness::Failed(cause) => assert_eq!(cause, "disk full"),
            other => panic!("expected a failed import, got {other:?}"),
        }
    }

    #[test]
    fn next_interval_doubles_up_to_the_cap() {
        use std::time::Duration;
        assert_eq!(
            next_interval(Duration::from_secs(2), Duration::from_secs(30)),
            Duration::from_secs(4)
        );
        assert_eq!(
            next_interval(Duration::from_secs(20), Duration::from_secs(30)),
            Duration::from_secs(30),
            "doubling past the cap clamps to it"
        );
        assert_eq!(
            next_interval(Duration::from_secs(30), Duration::from_secs(30)),
            Duration::from_secs(30)
        );
    }

    #[tokio::test]
    async fn poll_until_ready_honors_the_deadline() {
        // An unreachable endpoint never becomes ready — the loop must give up
        // on the wall-clock deadline (a bounded schedule), not spin forever.
        let ctx = ForgeContext {
            kind: ForgeKind::GitLab,
            api_url: Some("http://127.0.0.1:1".to_string()),
            token: Some("t".to_string()),
            ci_namespace: None,
        };
        let bounds = PollBounds {
            initial_interval: std::time::Duration::from_millis(5),
            max_interval: std::time::Duration::from_millis(10),
            deadline: std::time::Duration::from_millis(40),
            request_timeout: std::time::Duration::from_millis(50),
        };
        let started = std::time::Instant::now();
        let result = poll_until_ready(bounds, &ctx, "http://127.0.0.1:1/never", gitlab_import_readiness).await;
        assert!(
            matches!(result, Err(AnnounceError::Fork { .. })),
            "an unreachable fork must fail on the deadline as AnnounceError::Fork"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "the short deadline must bound the wait"
        );
    }

    // ── W6/W7 regression: GitLab fork identity gate + reuse-enumeration retry ──

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A 200 OK JSON response framed the way every mock forge test server
    /// below writes it.
    fn ok_json_response(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        )
    }

    /// A 404 Not Found response with no body — the fallback for any request
    /// a mock forge test server doesn't recognize.
    fn not_found_response() -> String {
        "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_string()
    }

    /// Spawn a throwaway raw-HTTP mock server: accepts connections in a loop,
    /// reads each request up to the blank line terminating the head, and
    /// answers with whatever `respond` computes from the lowercased request
    /// line. Shared connection-handling scaffold for the fork-identity (W6)
    /// and reuse-enumeration (W7) regression mocks — only the per-test
    /// dispatch in `respond` differs between them.
    async fn spawn_mock_forge(
        respond: impl Fn(&str) -> String + Send + 'static,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let mut req = Vec::new();
                let mut buf = [0u8; 1024];
                loop {
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    req.extend_from_slice(&buf[..n]);
                    if req.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let req = String::from_utf8_lossy(&req).to_ascii_lowercase();
                let first = req.lines().next().unwrap_or("");
                let response = respond(first);
                let _ = sock.write_all(response.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        (host, handle)
    }

    /// A throwaway GitLab-shaped forge for the fork-identity gate (W6): the
    /// permission probe reports a certain no-push, `GET /user` fails (500)
    /// so the authenticated identity is unknown, and any `POST .../fork` is
    /// counted via `fork_posts` — the regression asserts the count stays
    /// zero.
    async fn spawn_gitlab_unknown_identity(fork_posts: Arc<AtomicUsize>) -> (String, tokio::task::JoinHandle<()>) {
        spawn_mock_forge(move |first| {
            if first.contains("/fork") {
                fork_posts.fetch_add(1, Ordering::SeqCst);
                ok_json_response(
                    r#"{"id":99,"path_with_namespace":"forkuser/index","http_url_to_repo":"https://gitlab.example.com/forkuser/index.git","forked_from_project":{"id":42}}"#,
                )
            } else if first.contains("/user") {
                "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_string()
            } else if first.contains("/projects/") {
                ok_json_response(r#"{"id":42,"permissions":{"project_access":{"access_level":10}}}"#)
            } else {
                not_found_response()
            }
        })
        .await
    }

    /// W6 regression: when the current-user lookup fails, `gitlab_ensure_fork`
    /// must degrade to the upstream push WITHOUT ever issuing the fork POST —
    /// a fork created with no verifiable namespace is an orphan side effect,
    /// not a safe push target (previously the POST fired unconditionally and
    /// only degraded *after* creating the orphan).
    #[tokio::test]
    async fn gitlab_ensure_fork_skips_post_when_identity_unknown() {
        let fork_posts = Arc::new(AtomicUsize::new(0));
        let (host, handle) = spawn_gitlab_unknown_identity(fork_posts.clone()).await;
        let api = format!("http://{host}/api/v4");
        let ctx = ForgeContext {
            kind: ForgeKind::GitLab,
            api_url: Some(api.clone()),
            token: Some("glpat-x".to_string()),
            ci_namespace: None,
        };
        let http = client().expect("build forge client");
        let result = gitlab_ensure_fork(&http, &ctx, &api, "acme/index", "gitlab.example.com", false).await;
        handle.abort();
        assert!(
            matches!(result, Ok(None)),
            "unknown identity must degrade to the upstream push, got {result:?}"
        );
        assert_eq!(
            fork_posts.load(Ordering::SeqCst),
            0,
            "no fork POST must be issued when the authenticated identity is unknown"
        );
    }

    /// A throwaway GitHub-shaped forge whose token *has* push access to the
    /// upstream and already owns a valid fork at the conventional path.
    ///
    /// Every branch is method-guarded: `POST /repos/acme/index/forks` is a
    /// prefix-extension of the `GET /repos/acme/index` probe path, so without
    /// the guard a fork POST would silently be answered with the upstream's
    /// permission body. This forge serves the reuse path only — a fork POST
    /// reaching it is a test-wiring bug and must 404 rather than look like a
    /// success.
    async fn spawn_github_pushable_with_fork() -> (String, tokio::task::JoinHandle<()>) {
        spawn_mock_forge(move |first| {
            if !first.starts_with("get") {
                not_found_response()
            } else if first.contains("/user") {
                ok_json_response(r#"{"login":"forkuser"}"#)
            } else if first.contains("/repos/forkuser/index") {
                ok_json_response(
                    r#"{"full_name":"forkuser/index","clone_url":"https://github.com/forkuser/index.git","owner":{"login":"forkuser"},"parent":{"full_name":"acme/index"}}"#,
                )
            } else if first.contains("/repos/acme/index") {
                ok_json_response(r#"{"permissions":{"push":true}}"#)
            } else {
                not_found_response()
            }
        })
        .await
    }

    /// `[announce] fork = "always"` (the `force` flag) forks even when the
    /// token has push access to the upstream; without it, push access degrades
    /// to the upstream push (today's default). Both are asserted against the
    /// same forge so the flag is the only difference.
    #[tokio::test]
    async fn github_ensure_fork_force_forks_despite_push_access() {
        let (host, handle) = spawn_github_pushable_with_fork().await;
        let api = format!("http://{host}");
        let ctx = ForgeContext {
            kind: ForgeKind::GitHub,
            api_url: Some(api.clone()),
            token: Some("ghp-x".to_string()),
            ci_namespace: None,
        };
        let http = client().expect("build forge client");

        // Default (force = false): push access ⇒ no fork.
        let no_fork = github_ensure_fork(&http, &ctx, &api, "acme/index", "github.com", false).await;
        assert!(
            matches!(no_fork, Ok(None)),
            "push access must degrade to the upstream push by default, got {no_fork:?}"
        );

        // force = true: fork anyway, reusing the existing fork (created = false).
        let forced = github_ensure_fork(&http, &ctx, &api, "acme/index", "github.com", true).await;
        handle.abort();
        match forced {
            Ok(Some(target)) => {
                assert_eq!(target.full_name, "forkuser/index");
                assert!(!target.created, "the existing fork is reused, not created");
            }
            other => panic!("force must fork despite push access, got {other:?}"),
        }
    }

    /// An upstream project the token can push to (Maintainer access).
    const GITLAB_UPSTREAM_PUSHABLE: &str = r#"{"id":42,"permissions":{"project_access":{"access_level":40}}}"#;
    /// An upstream project carrying no `permissions` block at all — the
    /// ambiguous probe `gitlab_can_push` answers `None` for.
    const GITLAB_UPSTREAM_OPAQUE: &str = r#"{"id":42}"#;

    /// A throwaway GitLab-shaped forge that answers the identity probe as
    /// `forkuser` and serves a ready fork from the fork POST, with the
    /// upstream project object supplied by the caller (`GITLAB_UPSTREAM_*`)
    /// so a test can pick the permission shape it needs. Every `POST
    /// .../fork` is counted so a test can assert the call did or did not
    /// happen.
    async fn spawn_gitlab_forge(
        fork_posts: Arc<AtomicUsize>,
        upstream: &'static str,
    ) -> (String, tokio::task::JoinHandle<()>) {
        spawn_mock_forge(move |first| {
            // Dispatch on the method for the fork call: the project path
            // `/projects/forkuser%2findex` contains `/fork` as a substring.
            if first.starts_with("post") && first.contains("/fork") {
                fork_posts.fetch_add(1, Ordering::SeqCst);
                ok_json_response(
                    r#"{"id":22,"path_with_namespace":"forkuser/index","http_url_to_repo":"https://gitlab.example.com/forkuser/index.git","forked_from_project":{"id":42}}"#,
                )
            } else if first.contains("/api/v4/user") {
                ok_json_response(r#"{"username":"forkuser"}"#)
            } else if first.contains("forkuser%2findex") {
                // The fork — also the upstream in the self-fork test — and the
                // readiness poll's target, hence both fields.
                ok_json_response(
                    r#"{"id":22,"permissions":{"project_access":{"access_level":40}},"import_status":"finished"}"#,
                )
            } else if first.contains("/projects/") {
                ok_json_response(upstream)
            } else {
                not_found_response()
            }
        })
        .await
    }

    /// GitLab counterpart to [`github_ensure_fork_force_forks_despite_push_access`]:
    /// the two forges parse permissions differently (`project_access.access_level`
    /// vs `permissions.push`) and gate the fork in separate functions, so the
    /// `force` bypass needs its own proof on each.
    #[tokio::test]
    async fn gitlab_ensure_fork_force_forks_despite_push_access() {
        let fork_posts = Arc::new(AtomicUsize::new(0));
        let (host, handle) = spawn_gitlab_forge(fork_posts.clone(), GITLAB_UPSTREAM_PUSHABLE).await;
        let api = format!("http://{host}/api/v4");
        let ctx = ForgeContext {
            kind: ForgeKind::GitLab,
            api_url: Some(api.clone()),
            token: Some("glpat-x".to_string()),
            ci_namespace: None,
        };
        let http = client().expect("build forge client");

        // Default (force = false): push access ⇒ no fork, no POST.
        let no_fork = gitlab_ensure_fork(&http, &ctx, &api, "acme/index", "gitlab.example.com", false).await;
        assert!(
            matches!(no_fork, Ok(None)),
            "push access must degrade to the upstream push by default, got {no_fork:?}"
        );
        assert_eq!(
            fork_posts.load(Ordering::SeqCst),
            0,
            "the default policy must not issue a fork POST when the token can push"
        );

        // force = true: fork anyway.
        let forced = gitlab_ensure_fork(&http, &ctx, &api, "acme/index", "gitlab.example.com", true).await;
        handle.abort();
        match forced {
            Ok(Some(target)) => {
                assert_eq!(target.full_name, "forkuser/index");
                assert!(target.created, "a fork the forced path just POSTed is newly created");
            }
            other => panic!("force must fork despite push access, got {other:?}"),
        }
    }

    /// `force` deliberately skips the push-access probe, so it also reaches
    /// the case where that probe could not decide at all (a project object
    /// carrying no `permissions` block — `gitlab_can_push` answers `None`).
    /// That combination is what `always` promises: fork without consulting
    /// the permission read, rather than degrading the way `auto` does.
    #[tokio::test]
    async fn gitlab_ensure_fork_force_forks_on_an_ambiguous_permission_probe() {
        let fork_posts = Arc::new(AtomicUsize::new(0));
        let (host, handle) = spawn_gitlab_forge(fork_posts.clone(), GITLAB_UPSTREAM_OPAQUE).await;
        let api = format!("http://{host}/api/v4");
        let ctx = ForgeContext {
            kind: ForgeKind::GitLab,
            api_url: Some(api.clone()),
            token: Some("glpat-x".to_string()),
            ci_namespace: None,
        };
        let http = client().expect("build forge client");

        // Default (force = false): an undecidable probe degrades to the push.
        let no_fork = gitlab_ensure_fork(&http, &ctx, &api, "acme/index", "gitlab.example.com", false).await;
        assert!(
            matches!(no_fork, Ok(None)),
            "an ambiguous probe must degrade to the upstream push by default, got {no_fork:?}"
        );
        assert_eq!(
            fork_posts.load(Ordering::SeqCst),
            0,
            "the default policy must not fork on an ambiguous probe"
        );

        // force = true: fork without a decidable permission read.
        let forced = gitlab_ensure_fork(&http, &ctx, &api, "acme/index", "gitlab.example.com", true).await;
        handle.abort();
        match forced {
            Ok(Some(target)) => assert_eq!(target.full_name, "forkuser/index"),
            other => panic!("force must fork on an ambiguous probe, got {other:?}"),
        }
    }

    /// `force` must not defeat the self-fork guard: forking a project into the
    /// namespace it already occupies is impossible, so the announce degrades to
    /// the upstream push. Without the guard the fork POST fires and its response
    /// fails the parent-lineage check in [`gitlab_fork_target`] (no project is a
    /// fork of itself), turning a working push into exit 69. Real GitLab answers
    /// that POST with a 409; the mock returns a body whose lineage cannot match,
    /// which trips the same backstop one step later.
    #[tokio::test]
    async fn gitlab_ensure_fork_force_skips_self_owned_upstream() {
        let fork_posts = Arc::new(AtomicUsize::new(0));
        let (host, handle) = spawn_gitlab_forge(fork_posts.clone(), GITLAB_UPSTREAM_PUSHABLE).await;
        let api = format!("http://{host}/api/v4");
        let ctx = ForgeContext {
            kind: ForgeKind::GitLab,
            api_url: Some(api.clone()),
            token: Some("glpat-x".to_string()),
            ci_namespace: None,
        };
        let http = client().expect("build forge client");
        // The authenticated user (`forkuser`) owns the upstream itself.
        let result = gitlab_ensure_fork(&http, &ctx, &api, "forkuser/index", "gitlab.example.com", true).await;
        handle.abort();
        assert!(
            matches!(result, Ok(None)),
            "a self-owned upstream must degrade to the upstream push even under force, got {result:?}"
        );
        assert_eq!(
            fork_posts.load(Ordering::SeqCst),
            0,
            "no fork POST must be issued for a self-owned upstream"
        );
    }

    /// `never` must short-circuit before any network call: the policy is the
    /// user saying "do not fork", not "try and give up". Previously the caller
    /// in `index_announce` gated the call away entirely; the gate now lives
    /// here, so this pins that it still costs nothing.
    #[tokio::test]
    async fn ensure_fork_never_issues_no_forge_request() {
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let (host, handle) = spawn_mock_forge(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            not_found_response()
        })
        .await;
        let api = format!("http://{host}/api/v4");
        let ctx = ForgeContext {
            kind: ForgeKind::GitLab,
            api_url: Some(api.clone()),
            token: Some("glpat-x".to_string()),
            ci_namespace: None,
        };
        let http = client().expect("build forge client");
        let result = ensure_fork(&http, &ctx, "https://gitlab.example.com/acme/index", ForkPolicy::Never).await;
        handle.abort();
        assert!(
            matches!(result, Ok(None)),
            "`never` must degrade to the upstream push, got {result:?}"
        );
        assert_eq!(
            requests.load(Ordering::SeqCst),
            0,
            "`never` must not touch the forge API at all"
        );
    }

    /// The self-fork guard compares a namespace the publisher typed (in
    /// `[announce] repository`) against a username the forge API reports.
    /// GitLab routes namespaces case-insensitively, so those two spellings
    /// can differ for one and the same account — an ASCII-case-sensitive
    /// compare would miss the guard and fork the user's own project.
    #[tokio::test]
    async fn gitlab_ensure_fork_self_fork_guard_ignores_namespace_case() {
        let fork_posts = Arc::new(AtomicUsize::new(0));
        let (host, handle) = spawn_gitlab_forge(fork_posts.clone(), GITLAB_UPSTREAM_PUSHABLE).await;
        let api = format!("http://{host}/api/v4");
        let ctx = ForgeContext {
            kind: ForgeKind::GitLab,
            api_url: Some(api.clone()),
            token: Some("glpat-x".to_string()),
            ci_namespace: None,
        };
        let http = client().expect("build forge client");
        // The API answers `forkuser`; the manifest spells it `Forkuser`.
        let result = gitlab_ensure_fork(&http, &ctx, &api, "Forkuser/index", "gitlab.example.com", true).await;
        handle.abort();
        assert!(
            matches!(result, Ok(None)),
            "a case-different self-owned upstream must still degrade to the upstream push, got {result:?}"
        );
        assert_eq!(
            fork_posts.load(Ordering::SeqCst),
            0,
            "no fork POST must be issued for a self-owned upstream spelled in a different case"
        );
    }

    /// A throwaway GitLab-shaped forge for the reuse-enumeration retry (W7):
    /// the forks listing is empty on the first `empty_responses` calls, then
    /// returns the authenticated user's fork on the next one — simulating
    /// GitLab's eventual consistency for a concurrently-created fork.
    async fn spawn_gitlab_eventually_consistent_forks(
        empty_responses: usize,
        calls: Arc<AtomicUsize>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        spawn_mock_forge(move |first| {
            if first.contains("/forks") {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                let body = if n < empty_responses {
                    "[]".to_string()
                } else {
                    r#"[{"id":22,"path_with_namespace":"forkuser/index","http_url_to_repo":"https://gitlab.example.com/forkuser/index.git","forked_from_project":{"id":42}}]"#.to_string()
                };
                ok_json_response(&body)
            } else {
                not_found_response()
            }
        })
        .await
    }

    /// W7 regression: a fork created concurrently by another process may not
    /// be listed yet on the first enumeration pass — `gitlab_find_owned_fork`
    /// must retry the scan under a bounded deadline rather than failing
    /// outright the moment page 1 comes back empty.
    #[tokio::test]
    async fn gitlab_find_owned_fork_retries_until_the_listing_catches_up() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (host, handle) = spawn_gitlab_eventually_consistent_forks(2, calls.clone()).await;
        let api = format!("http://{host}/api/v4");
        let ctx = ForgeContext {
            kind: ForgeKind::GitLab,
            api_url: Some(api.clone()),
            token: Some("glpat-x".to_string()),
            ci_namespace: None,
        };
        let bounds = PollBounds {
            initial_interval: std::time::Duration::from_millis(5),
            max_interval: std::time::Duration::from_millis(10),
            deadline: std::time::Duration::from_secs(5),
            request_timeout: std::time::Duration::from_millis(200),
        };
        let target = gitlab_find_owned_fork_bounded(bounds, &ctx, &api, 42, "gitlab.example.com", "forkuser")
            .await
            .expect("the fork is adopted once the listing catches up");
        handle.abort();
        assert_eq!(target.full_name, "forkuser/index");
        assert_eq!(target.fork_project_id, Some(22));
        assert!(
            calls.load(Ordering::SeqCst) >= 3,
            "expected at least 2 empty responses before the match, got {} calls",
            calls.load(Ordering::SeqCst)
        );
    }

    /// A4 regression: when the forks listing never includes a match (every
    /// enumeration page comes back empty), the retry loop must give up on
    /// the wall-clock deadline rather than looping forever — mirrors
    /// [`poll_until_ready_honors_the_deadline`] for the reuse-enumeration
    /// retry instead of the readiness poll.
    #[tokio::test]
    async fn gitlab_find_owned_fork_bounded_gives_up_on_the_deadline() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (host, handle) = spawn_gitlab_eventually_consistent_forks(usize::MAX, calls.clone()).await;
        let api = format!("http://{host}/api/v4");
        let ctx = ForgeContext {
            kind: ForgeKind::GitLab,
            api_url: Some(api.clone()),
            token: Some("glpat-x".to_string()),
            ci_namespace: None,
        };
        let bounds = PollBounds {
            initial_interval: std::time::Duration::from_millis(5),
            max_interval: std::time::Duration::from_millis(10),
            deadline: std::time::Duration::from_millis(40),
            request_timeout: std::time::Duration::from_millis(50),
        };
        let started = std::time::Instant::now();
        let result = gitlab_find_owned_fork_bounded(bounds, &ctx, &api, 42, "gitlab.example.com", "forkuser").await;
        handle.abort();
        assert!(
            matches!(result, Err(AnnounceError::Fork { .. })),
            "an always-empty forks listing must fail on the deadline as AnnounceError::Fork, got {result:?}"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "the short deadline must bound the wait"
        );
    }
}
