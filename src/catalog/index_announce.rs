// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Package-index announcement — the write side of [`super::index_source`].
//!
//! `grim publish --announce` records published packages in a package-index
//! git repository: clone, write `index/<host>/<ns>/<pkg>/metadata.json`
//! pointers, commit on a deterministic topic branch, push, and open the
//! pull/merge request through the resolved forge API ([`super::forge`],
//! GitHub or GitLab — enterprise instances included, no CLI dependency).
//! A GitLab host without an API token gets the MR via git push options
//! (`-o merge_request.create`); a plain git host gets the pushed branch.
//!
//! The announced metadata is the phone-book pointer only (name, kind,
//! tagless ref, description, ownership) — never versions. Re-announcing
//! unchanged content is detected via `git status` and reported as
//! [`AnnounceOutcome::UpToDate`] without a push.

use std::path::Path;

use super::forge::{ForgeContext, ForgeKind};

/// The default public index announcements target.
pub const DEFAULT_INDEX_REPO: &str = "https://github.com/grimoire-rs/index";

/// Derive the index host path segment (`index/<host>/…`) from the index
/// repository locator: strip a `git+` transport prefix, normalize the
/// remote shape (https / ssh / scp-like, credentials and ports stripped),
/// and take the lowercased host. `None` for locators without a host (a
/// local path, `file://`) — those need an explicit `[announce] host`.
pub fn index_host(repo_url: &str) -> Option<String> {
    let url = repo_url.strip_prefix("git+").unwrap_or(repo_url);
    let https = crate::oci::git_provenance::normalize_remote_url(url)?;
    https.strip_prefix("https://")?.split('/').next().map(str::to_lowercase)
}

/// One package pointer to announce.
#[derive(Debug, Clone)]
pub struct AnnouncePackage {
    /// Package name (the index directory name).
    pub name: String,
    /// `skill` / `rule` / `agent` / `mcp` / `bundle`.
    pub kind: String,
    /// Tagless OCI reference (`registry/repository`).
    pub reference: String,
    /// One-line description shown in `grim search`.
    pub description: String,
    /// HTTPS source-repository URL, if known.
    pub repository_url: Option<String>,
    /// Publisher keywords (`grim search` matches them alongside the
    /// description). Empty ⇒ omitted from the written pointer.
    pub keywords: Vec<String>,
    /// Short single-line blurb (`grim search` matches it too). `None` ⇒
    /// omitted from the written pointer.
    pub summary: Option<String>,
}

/// The announce request: where, as whom, and what.
#[derive(Debug)]
pub struct AnnounceRequest {
    /// The index git repository (https clone URL or local path).
    pub repo_url: String,
    /// The index host path segment — pointers land under
    /// `index/<host>/<namespace>/`.
    pub host: String,
    /// The `index/<host>/<namespace>/` the packages land under.
    pub namespace: String,
    /// The namespace's numeric owner id on the index host (resolved by the
    /// caller — explicitly configured or looked up via the forge API).
    pub owner_id: u64,
    /// The resolved forge fronting the index repository.
    pub forge: ForgeContext,
    /// The packages to announce.
    pub packages: Vec<AnnouncePackage>,
    /// A `credential.helper=…` git config value appended (`-c`) to the
    /// remote-touching git invocations (clone, push) as a fallback
    /// credential — see [`job_token_credential_config`]. `None` leaves
    /// the git transport on ambient credentials only.
    pub credential_config: Option<String>,
    /// Whether an auto-fork is permitted when the authenticated token has no
    /// push access to the upstream index (`[announce] fork` — default true;
    /// `false` forces the upstream push). A fork is never attempted without a
    /// forge API token regardless of this flag.
    pub allow_fork: bool,
}

/// The `credential.helper=…` git config value injecting the GitLab CI job
/// token as a **fallback** transport credential.
///
/// `Some` only when all of: `GITLAB_CI` is truthy, `CI_JOB_TOKEN` is set,
/// and the index repository's git host equals `CI_SERVER_HOST` — i.e. the
/// index lives on the same GitLab instance whose job token the runner
/// holds. The returned text contains **no secret**: the `!`-helper runs
/// via `sh`, which expands the literal `${CI_JOB_TOKEN}` from the child
/// environment git inherits, so the token never enters grim's memory,
/// argv, or disk. The helper config key is **URL-scoped** to the gated
/// host (`credential.https://<host>.helper`), so git only offers the token
/// for that exact host — a redirect or submodule fetch to another host in
/// the same invocation cannot draw it (Clone2Leak / CVE-2024-53858 class).
/// Appended (never replacing) helper config means git consults ambient
/// helpers first — an explicitly configured credential always wins. The job
/// token stays banned from the forge MR API
/// ([`super::forge::CiEnv::gitlab_token`]).
pub fn job_token_credential_config(env: &super::forge::CiEnv, repo_url: &str) -> Option<String> {
    if !env.gitlab_ci || !env.ci_job_token_present {
        return None;
    }
    let host = index_host(repo_url)?;
    if !env.ci_server_host.as_deref()?.eq_ignore_ascii_case(&host) {
        return None;
    }
    // URL-scope the helper to the exact gated host so git only ever consults
    // it for that host: a server-side redirect, a future submodule fetch, or
    // any other credential lookup in the same git process to a different host
    // cannot draw the job token (Clone2Leak / CVE-2024-53858 class). The gate
    // above already proved this host equals `CI_SERVER_HOST`; `index_host`
    // lowercases it, and git matches credential URLs case-insensitively.
    Some(format!(
        "credential.https://{host}.helper=!f() {{ if [ \"$1\" = get ]; then \
         echo username=gitlab-ci-token; echo \"password=${{CI_JOB_TOKEN}}\"; fi; }}; f"
    ))
}

/// The fork an announce pushed its branch to, for the machine-readable
/// report. Present only when a cross-repository fork was opened or reused;
/// absent when the branch went straight to the upstream index.
#[derive(Debug, PartialEq, Eq)]
pub struct AnnouncedFork {
    /// The fork's canonical `owner/repo` (GitHub `full_name`, GitLab
    /// `path_with_namespace`).
    pub repo: String,
    /// Whether this run created the fork (`false` when an existing fork was
    /// reused).
    pub created: bool,
}

/// What the announce achieved. Every variant carries the deterministic
/// topic branch so CI can consume it regardless of outcome.
#[derive(Debug, PartialEq, Eq)]
pub enum AnnounceOutcome {
    /// A pull/merge request was opened — via the forge API, or by a forge
    /// honoring `merge_request.create` push options.
    PullRequest {
        /// The PR/MR URL.
        url: String,
        /// The pushed topic branch name.
        branch: String,
        /// The fork the branch was pushed to, or `None` for an upstream push.
        fork: Option<AnnouncedFork>,
    },
    /// The topic branch was pushed; open the merge request on the host.
    BranchPushed {
        /// The pushed branch name.
        branch: String,
        /// The fork the branch was pushed to, or `None` for an upstream push.
        fork: Option<AnnouncedFork>,
    },
    /// The index already carries exactly this metadata — nothing to do.
    UpToDate {
        /// The topic branch the metadata would have landed on.
        branch: String,
    },
}

/// Announce-tier failures.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AnnounceError {
    /// A git subprocess failed.
    #[error("git {action} failed: {detail}")]
    Git {
        /// The git verb that failed (`clone`, `push`, …).
        action: &'static str,
        /// Trimmed stderr of the failing invocation.
        detail: String,
    },
    /// Local I/O around the working clone failed.
    #[error("I/O error during announce")]
    Io(#[from] std::io::Error),
    /// The namespace's GitHub account id could not be resolved.
    #[error("GitHub account lookup failed for '{namespace}'")]
    OwnerLookup {
        /// The namespace whose id lookup failed.
        namespace: String,
        /// The transport / parse cause.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// Forking the index repository failed after the permission probe showed
    /// no push access: the fork could not be created, security-verified, or
    /// made ready. Distinct from a plain push failure — a fork was required
    /// (the caller cannot push upstream) and could not be provided.
    #[error("fork of the index repository failed: {detail}")]
    Fork {
        /// The specific fork-step failure.
        detail: String,
    },
    /// The shared forge HTTP client (permission probes, owner lookup,
    /// forking, announcing) could not be constructed.
    #[error("could not build the forge HTTP client")]
    Client(#[from] reqwest::Error),
}

/// Announce `request.packages` to the index repository.
///
/// # Errors
///
/// [`AnnounceError`] for a git clone/commit/push failure, a local I/O
/// failure, or a failed owner-id lookup.
pub async fn announce(http: &reqwest::Client, request: &AnnounceRequest) -> Result<AnnounceOutcome, AnnounceError> {
    let workdir = tempfile::tempdir()?;
    let clone = workdir.path().join("index");
    // Fallback transport credential for every remote-touching invocation
    // (clone + pushes); local operations don't need it.
    let cred = request.credential_config.as_deref();
    git(
        None,
        "clone",
        &with_credential(
            cred,
            &[
                "clone",
                "--depth",
                "1",
                "--quiet",
                "--",
                &request.repo_url,
                &clone.display().to_string(),
            ],
        ),
    )
    .await?;

    // Deterministic topic branch: same package set + content ⇒ same branch,
    // so a retried announce force-updates its own branch instead of
    // littering. Fold REPO-RELATIVE paths — the clone lands in a fresh
    // tempdir every run, so absolute paths would break determinism.
    let mut rendered: Vec<(String, String)> = Vec::new();
    for pkg in &request.packages {
        let relative = format!(
            "index/{}/{}/{}/metadata.json",
            request.host, request.namespace, pkg.name
        );
        rendered.push((
            relative,
            metadata_json(pkg, &request.namespace, request.owner_id, request.forge.kind),
        ));
    }
    let branch = branch_name(&request.namespace, &rendered);

    git(Some(&clone), "checkout", &["checkout", "--quiet", "-b", &branch]).await?;
    for (relative, content) in &rendered {
        let path = clone.join(relative);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(path, content).await?;
    }

    let status = git_output(&clone, "status", &["status", "--porcelain"]).await?;
    if status.trim().is_empty() {
        return Ok(AnnounceOutcome::UpToDate { branch });
    }

    git(Some(&clone), "add", &["add", "-A"]).await?;
    let names: Vec<&str> = request.packages.iter().map(|p| p.name.as_str()).collect();
    let message = format!("announce: {}", names.join(", "));
    git(
        Some(&clone),
        "commit",
        &[
            "-c",
            "user.name=grim",
            "-c",
            "user.email=announce@grimoire.rs",
            "commit",
            "--quiet",
            "-m",
            &message,
        ],
    )
    .await?;

    // Force-push our own topic branch (deterministic name ⇒ safe to move),
    // then open the change request the best way the forge allows.
    let api_capable =
        request.forge.token.is_some() && matches!(request.forge.kind, ForgeKind::GitHub | ForgeKind::GitLab);
    if api_capable || request.forge.kind == ForgeKind::GitHub {
        // With no push access to the upstream index, fork it and push the
        // branch there instead — additive: turns today's exit-69 push failure
        // into a cross-repository PR/MR. Requires a forge API token;
        // `[announce] fork = false` opts out; ambiguous permission degrades to
        // the upstream push. The credential helper is host-scoped and the fork
        // shares the host, so it covers the fork push unchanged.
        let fork = if request.allow_fork && api_capable {
            super::forge::ensure_fork(http, &request.forge, &request.repo_url).await?
        } else {
            None
        };
        let push_target = fork.as_ref().map_or("origin", |f| f.push_url.as_str());
        push_announce_branch(&clone, cred, push_target, &branch, fork.is_some()).await?;
        let announced = fork.as_ref().map(|f| AnnouncedFork {
            repo: f.full_name.clone(),
            created: f.created,
        });
        if api_capable
            && let Some(url) = super::forge::create_change_request(
                http,
                &request.forge,
                &request.repo_url,
                &branch,
                &message,
                fork.as_ref(),
            )
            .await
        {
            return Ok(AnnounceOutcome::PullRequest {
                url,
                branch,
                fork: announced,
            });
        }
        return Ok(AnnounceOutcome::BranchPushed {
            branch,
            fork: announced,
        });
    }

    // GitLab without a token, or a plain git host: ask the server to open
    // the MR via push options (native GitLab feature, harmless elsewhere).
    // A server without push-options support fails the whole push — retry
    // once as a plain push rather than sniffing localized git stderr.
    let title_option = format!("merge_request.title={message}");
    let options_push = git_stderr(
        &clone,
        "push",
        &with_credential(
            cred,
            &[
                "push",
                "--force",
                "-o",
                "merge_request.create",
                "-o",
                &title_option,
                "--",
                "origin",
                &branch,
            ],
        ),
    )
    .await;
    match options_push {
        Ok(stderr) => Ok(match merge_request_url(&stderr) {
            Some(url) => AnnounceOutcome::PullRequest {
                url,
                branch,
                fork: None,
            },
            None => AnnounceOutcome::BranchPushed { branch, fork: None },
        }),
        Err(_) => {
            // `?` propagates the retry's error — same root cause when the
            // push itself (not the options) is broken.
            git(
                Some(&clone),
                "push",
                &with_credential(cred, &["push", "--quiet", "--force", "--", "origin", &branch]),
            )
            .await?;
            Ok(AnnounceOutcome::BranchPushed { branch, fork: None })
        }
    }
}

/// The created/updated MR URL from a `merge_request.create` push's stderr:
/// an `http(s)://` token containing `/merge_requests/` whose final path
/// segment is all digits. Deliberately rejects the `/merge_requests/new?…`
/// *suggestion* URL a plain GitLab push prints.
fn merge_request_url(push_stderr: &str) -> Option<String> {
    push_stderr
        .split_whitespace()
        .find(|token| {
            (token.starts_with("https://") || token.starts_with("http://"))
                && token.contains("/merge_requests/")
                && token
                    .rsplit('/')
                    .next()
                    .is_some_and(|last| !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(str::to_string)
}

/// Pointer metadata read back from a published artifact's manifest — the
/// display fields the index carries. Named (not a tuple) so the three
/// `Option<String>` fields can't be transposed at the call site.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PointerMetadata {
    /// `org.opencontainers.image.description`.
    pub description: Option<String>,
    /// `org.opencontainers.image.source`, kept only with an `https://` prefix.
    pub repository_url: Option<String>,
    /// `com.grimoire.keywords`, comma-split (trimmed, empties dropped).
    pub keywords: Vec<String>,
    /// `com.grimoire.summary`.
    pub summary: Option<String>,
}

/// Read the pointer metadata (description, HTTPS source URL, keywords,
/// summary) back from the just-published artifact's manifest annotations:
/// representative tag → digest → manifest, over the access seam. Every
/// failure degrades to the default — announce still proceeds with a
/// fallback description.
pub async fn pointer_metadata(
    access: &dyn crate::oci::access::OciAccess,
    id: &crate::oci::Identifier,
) -> PointerMetadata {
    let tags = match access.list_tags(id).await {
        Ok(Some(tags)) => tags,
        _ => return PointerMetadata::default(),
    };
    let Some(tag) = crate::catalog::registry_catalog::pick_latest_tag(&tags) else {
        return PointerMetadata::default();
    };
    let tagged = id.clone_with_tag(tag);
    let digest = match access
        .resolve_digest(&tagged, crate::oci::access::Operation::Query)
        .await
    {
        Ok(Some(d)) => d,
        _ => return PointerMetadata::default(),
    };
    let Ok(pinned) = crate::oci::PinnedIdentifier::try_from(tagged.clone_with_digest(digest)) else {
        return PointerMetadata::default();
    };
    let Ok(Some(manifest)) = access.fetch_manifest(&pinned).await else {
        return PointerMetadata::default();
    };
    PointerMetadata {
        description: manifest
            .annotations
            .get("org.opencontainers.image.description")
            .cloned(),
        repository_url: manifest
            .annotations
            .get("org.opencontainers.image.source")
            .filter(|s| s.starts_with("https://"))
            .cloned(),
        // Same read seam the catalog build and `grim describe` use.
        keywords: crate::oci::annotations::keywords_from_annotations(&manifest.annotations),
        summary: manifest
            .annotations
            .get(crate::oci::annotations::SUMMARY_ANNOTATION)
            .cloned(),
    }
}

/// Render the metadata.json pointer for `pkg` (index spec v1).
///
/// The owner key is `github` for GitHub-forge pointers (spec-v1 compatible
/// with the default index's validator) and the generic `login` for any
/// other host — the pointer's `index/<host>/` segment carries the forge
/// context.
fn metadata_json(pkg: &AnnouncePackage, namespace: &str, owner_id: u64, forge: ForgeKind) -> String {
    let owner_key = match forge {
        ForgeKind::GitHub => "github",
        ForgeKind::GitLab | ForgeKind::Plain => "login",
    };
    let mut value = serde_json::json!({
        "schema": 1,
        "name": pkg.name,
        "kind": pkg.kind,
        "ref": pkg.reference,
        "description": pkg.description,
        "owner": { owner_key: namespace, "id": owner_id },
    });
    if let Some(repo) = &pkg.repository_url {
        value["repository"] = serde_json::Value::String(repo.clone());
    }
    // Omit-empty: pointers for artifacts without keywords/summary stay
    // byte-identical to pre-search-metadata index files.
    if !pkg.keywords.is_empty() {
        value["keywords"] = serde_json::Value::from(pkg.keywords.clone());
    }
    if let Some(summary) = &pkg.summary {
        value["summary"] = serde_json::Value::String(summary.clone());
    }
    let mut out = serde_json::to_string_pretty(&value).unwrap_or_default();
    out.push('\n');
    out
}

/// Deterministic topic branch: `announce/<ns>-<hash8>` over the rendered
/// (repo-relative path, content) set, so identical content re-announces
/// onto the same branch.
fn branch_name(namespace: &str, rendered: &[(String, String)]) -> String {
    let mut folded = String::new();
    for (relative, content) in rendered {
        folded.push_str(relative);
        folded.push('\0');
        folded.push_str(content);
    }
    let hash = crate::oci::digest::Algorithm::Sha256.hash(&folded).hex()[..8].to_string();
    format!("announce/{namespace}-{hash}")
}

/// Prepend `-c <config>` (a git global option — must precede the verb)
/// when a transport credential config is present.
fn with_credential<'a>(config: Option<&'a str>, args: &[&'a str]) -> Vec<&'a str> {
    match config {
        Some(cfg) => {
            let mut v = Vec::with_capacity(args.len() + 2);
            v.push("-c");
            v.push(cfg);
            v.extend_from_slice(args);
            v
        }
        None => args.to_vec(),
    }
}

/// Delay before the single fork-push retry ([`push_announce_branch`]).
// ponytail: fixed 3s, one retry. GitHub provisions a fresh fork's git
// objects asynchronously — the fork's metadata reads ready before its first
// push can (a brand-new fork's initial push can 404) — so one short retry
// absorbs the normal provisioning delay. The readiness poll in forge.rs
// already bounded the metadata wait; bump this only if fresh-fork pushes
// routinely need longer than one 3s retry.
const FORK_PUSH_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(3);

/// Force-push `branch` to `target`. When `is_fork` (a just-resolved fork push
/// target), retry once on the transient "not ready" signature GitHub emits
/// while a fresh fork's git objects are still provisioning: a fork's metadata
/// reads ready before its objects do, so the first push to a brand-new fork
/// can 404 even though the readiness poll passed. The upstream push (and any
/// non-transient failure) is never retried.
async fn push_announce_branch(
    clone: &Path,
    cred: Option<&str>,
    target: &str,
    branch: &str,
    is_fork: bool,
) -> Result<(), AnnounceError> {
    let args = with_credential(cred, &["push", "--quiet", "--force", "--", target, branch]);
    match git(Some(clone), "push", &args).await {
        Err(err) if is_fork && fork_push_not_ready(&err) => {
            tracing::info!("fork push target not ready yet (git objects still provisioning); retrying once");
            tokio::time::sleep(FORK_PUSH_RETRY_DELAY).await;
            git(Some(clone), "push", &args).await
        }
        result => result,
    }
}

/// Whether a push failure looks like a fresh fork whose git objects are not
/// yet provisioned — GitHub answers the first push to a brand-new fork with a
/// 404 / "repository not found" even though its metadata already reads. The
/// push URL is already identity-verified ([`super::forge`]), so a "not found"
/// here is the provisioning gap, not a misdirected push.
fn fork_push_not_ready(err: &AnnounceError) -> bool {
    let AnnounceError::Git { detail, .. } = err else {
        return false;
    };
    let detail = detail.to_ascii_lowercase();
    detail.contains("not found") || detail.contains("404")
}

/// Run a git subprocess, mapping a nonzero exit to [`AnnounceError::Git`].
async fn git(cwd: Option<&Path>, action: &'static str, args: &[&str]) -> Result<(), AnnounceError> {
    git_output_impl(cwd, action, args).await.map(|_| ())
}

/// Run a git subprocess in `cwd` and return its stdout.
async fn git_output(cwd: &Path, action: &'static str, args: &[&str]) -> Result<String, AnnounceError> {
    let output = git_output_impl(Some(cwd), action, args).await?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run a git subprocess in `cwd` and return its stderr — where git remotes
/// print their responses (e.g. GitLab's "View merge request" line after a
/// `merge_request.create` push option).
async fn git_stderr(cwd: &Path, action: &'static str, args: &[&str]) -> Result<String, AnnounceError> {
    let output = git_output_impl(Some(cwd), action, args).await?;
    Ok(String::from_utf8_lossy(&output.stderr).into_owned())
}

async fn git_output_impl(
    cwd: Option<&Path>,
    action: &'static str,
    args: &[&str],
) -> Result<std::process::Output, AnnounceError> {
    let mut cmd = tokio::process::Command::new("git");
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let output = cmd
        // Hard-disable git's `ext::` remote-helper transport for every
        // invocation (clone + pushes route through here). A push target or
        // clone URL of the form `ext::sh -c '…'` — reachable from a hostile
        // forge's fork response or a crafted index URL — otherwise executes
        // an arbitrary command (RCE). A global `-c` must precede the verb, so
        // it is prepended before the caller's args. Credential-helper config
        // (`with_credential`) is unaffected.
        .arg("-c")
        .arg("protocol.ext.allow=never")
        .args(args)
        // Never hang on an interactive credential prompt.
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await?;
    if !output.status.success() {
        return Err(AnnounceError::Git {
            action,
            detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str) -> AnnouncePackage {
        AnnouncePackage {
            name: name.to_string(),
            kind: "skill".to_string(),
            reference: format!("ghcr.io/acme/skills/{name}"),
            description: "A test pointer".to_string(),
            repository_url: Some("https://github.com/acme/skills".to_string()),
            keywords: Vec::new(),
            summary: None,
        }
    }

    #[test]
    fn metadata_json_matches_index_spec() {
        let rendered = metadata_json(&pkg("code-review"), "acme", 42, ForgeKind::GitHub);
        let value: serde_json::Value = serde_json::from_str(&rendered).expect("valid JSON");
        assert_eq!(value["schema"], 1);
        assert_eq!(value["name"], "code-review");
        assert_eq!(value["kind"], "skill");
        assert_eq!(value["ref"], "ghcr.io/acme/skills/code-review");
        assert_eq!(value["owner"]["github"], "acme");
        assert_eq!(value["owner"]["id"], 42);
        assert!(value["owner"].get("login").is_none());
        assert_eq!(value["repository"], "https://github.com/acme/skills");
        // Omit-empty: a pointer with no keywords/summary carries neither key,
        // keeping pre-search-metadata index files byte-identical.
        assert!(value.get("keywords").is_none());
        assert!(value.get("summary").is_none());
        assert!(rendered.ends_with('\n'), "trailing newline for clean diffs");
    }

    #[test]
    fn metadata_json_carries_keywords_and_summary_when_present() {
        let mut p = pkg("code-review");
        p.keywords = vec!["review".to_string(), "quality".to_string()];
        p.summary = Some("Terse review".to_string());
        let value: serde_json::Value =
            serde_json::from_str(&metadata_json(&p, "acme", 42, ForgeKind::GitHub)).expect("valid JSON");
        assert_eq!(value["keywords"], serde_json::json!(["review", "quality"]));
        assert_eq!(value["summary"], "Terse review");
    }

    #[test]
    fn metadata_json_uses_generic_login_key_off_github() {
        for forge in [ForgeKind::GitLab, ForgeKind::Plain] {
            let rendered = metadata_json(&pkg("x"), "platform/ai", 44, forge);
            let value: serde_json::Value = serde_json::from_str(&rendered).expect("valid JSON");
            assert_eq!(value["owner"]["login"], "platform/ai", "{forge:?}");
            assert_eq!(value["owner"]["id"], 44, "{forge:?}");
            assert!(value["owner"].get("github").is_none(), "{forge:?}");
        }
    }

    #[test]
    fn metadata_json_omits_absent_repository() {
        let mut p = pkg("x");
        p.repository_url = None;
        let value: serde_json::Value = serde_json::from_str(&metadata_json(&p, "acme", 1, ForgeKind::GitHub)).unwrap();
        assert!(value.get("repository").is_none());
    }

    #[test]
    fn branch_name_is_deterministic_and_content_sensitive() {
        let a = vec![("index/x/metadata.json".to_string(), "one".to_string())];
        let b = vec![("index/x/metadata.json".to_string(), "two".to_string())];
        assert_eq!(branch_name("acme", &a), branch_name("acme", &a));
        assert_ne!(branch_name("acme", &a), branch_name("acme", &b));
        assert!(branch_name("acme", &a).starts_with("announce/acme-"));
    }

    #[test]
    fn index_host_derives_from_locator_shapes() {
        for (locator, expected) in [
            ("https://github.com/grimoire-rs/index", Some("github.com")),
            (
                "https://gitlab.example.com/platform/index.git",
                Some("gitlab.example.com"),
            ),
            (
                "git+https://gitlab.example.com/platform/index.git",
                Some("gitlab.example.com"),
            ),
            ("ssh://git@gitlab.corp:2222/platform/index.git", Some("gitlab.corp")),
            ("git@GitLab.Example.com:platform/index.git", Some("gitlab.example.com")),
            (
                "https://oauth2:token@gitlab.example.com/g/index.git",
                Some("gitlab.example.com"),
            ),
            ("/tmp/local-index.git", None),
            ("file:///tmp/local-index.git", None),
        ] {
            assert_eq!(index_host(locator).as_deref(), expected, "{locator}");
        }
    }

    fn gitlab_ci_env(server_host: &str) -> super::super::forge::CiEnv {
        super::super::forge::CiEnv {
            gitlab_ci: true,
            ci_server_host: Some(server_host.to_string()),
            ci_job_token_present: true,
            ..Default::default()
        }
    }

    #[test]
    fn job_token_credential_config_requires_full_gate() {
        let url = "https://gitlab.example.com/platform/index.git";

        // All conditions met — including case-insensitive host match and
        // URL shapes carrying credentials/ports (stripped by index_host).
        for repo in [
            url,
            "git+https://gitlab.example.com/platform/index.git",
            "ssh://git@GitLab.Example.com:2222/platform/index.git",
            "https://oauth2:tok@gitlab.example.com/platform/index.git",
        ] {
            let cfg = job_token_credential_config(&gitlab_ci_env("gitlab.example.com"), repo)
                .unwrap_or_else(|| panic!("expected Some for {repo}"));
            // The key is URL-scoped to the gated host (never the bare global
            // `credential.helper`), so git offers the token only for that
            // host — a redirect/submodule to another host can't draw it.
            assert!(
                cfg.starts_with("credential.https://gitlab.example.com.helper=!"),
                "{cfg}"
            );
            assert!(!cfg.starts_with("credential.helper="), "{cfg}");
            assert!(cfg.contains("username=gitlab-ci-token"), "{cfg}");
            // The literal, unexpanded env reference — never a token value.
            assert!(cfg.contains("${CI_JOB_TOKEN}"), "{cfg}");
        }

        // Host mismatch.
        assert_eq!(
            job_token_credential_config(&gitlab_ci_env("gitlab.other.com"), url),
            None
        );
        // Not GitLab CI.
        let mut env = gitlab_ci_env("gitlab.example.com");
        env.gitlab_ci = false;
        assert_eq!(job_token_credential_config(&env, url), None);
        // Job token absent.
        let mut env = gitlab_ci_env("gitlab.example.com");
        env.ci_job_token_present = false;
        assert_eq!(job_token_credential_config(&env, url), None);
        // CI_SERVER_HOST absent.
        let mut env = gitlab_ci_env("gitlab.example.com");
        env.ci_server_host = None;
        assert_eq!(job_token_credential_config(&env, url), None);
        // Local locator — no derivable host.
        assert_eq!(
            job_token_credential_config(&gitlab_ci_env("gitlab.example.com"), "/tmp/index.git"),
            None
        );
    }

    #[test]
    fn fork_push_not_ready_detects_transient_provisioning_signatures() {
        let git_err = |detail: &str| AnnounceError::Git {
            action: "push",
            detail: detail.to_string(),
        };
        // GitHub's metadata-ready-before-git-objects-ready signatures.
        assert!(fork_push_not_ready(&git_err("remote: Repository not found.")));
        assert!(fork_push_not_ready(&git_err(
            "fatal: unable to access '...': The requested URL returned error: 404"
        )));
        // A genuine auth/other failure is not the provisioning gap.
        assert!(!fork_push_not_ready(&git_err(
            "remote: Permission to acme/index.git denied"
        )));
        // A non-git error is never the fork-push gap.
        assert!(!fork_push_not_ready(&AnnounceError::Fork {
            detail: "unrelated".to_string(),
        }));
    }

    #[test]
    fn with_credential_prepends_config_before_verb() {
        let args = ["push", "--quiet", "origin", "b"];
        assert_eq!(with_credential(None, &args), args.to_vec());
        assert_eq!(
            with_credential(Some("credential.helper=!x"), &args),
            vec!["-c", "credential.helper=!x", "push", "--quiet", "origin", "b"]
        );
    }

    #[test]
    fn merge_request_url_extracts_created_mr_only() {
        let created = "remote: View merge request for announce/acme-12345678:\n\
                       remote:   https://gitlab.example.com/platform/index/-/merge_requests/7\n";
        assert_eq!(
            merge_request_url(created).as_deref(),
            Some("https://gitlab.example.com/platform/index/-/merge_requests/7")
        );

        let suggestion = "remote: To create a merge request for announce/acme-12345678, visit:\n\
                          remote:   https://gitlab.example.com/platform/index/-/merge_requests/new?merge_request%5Bsource_branch%5D=announce\n";
        assert_eq!(merge_request_url(suggestion), None);
        assert_eq!(merge_request_url(""), None);
    }
}
