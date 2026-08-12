// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Pre-store credential verification for `grim login` — the docker/oras
//! registry ping.
//!
//! `GET /v2/` on the bare registry host, then answer the returned
//! `WWW-Authenticate` challenge with the just-entered credential: a
//! `Basic` challenge is answered in place, a `Bearer` challenge via a
//! **scope-less** token request against the challenge's realm (realm +
//! service only). Scope-less is deliberate: it validates the credential
//! itself, not access to any given repository. A 2xx ping with no
//! challenge means the registry does not require authentication — there
//! is nothing to verify.
//!
//! Deliberately not built on `oci_client::Client::auth()`: that path
//! always requests a repository-scoped token (which can be DENIED for a
//! probe repo even with valid credentials) and, on a Basic-challenge
//! registry, returns without any network round-trip validating the
//! credential. Its challenge parser is private, so the small
//! [`parse_challenge`] here is unavoidable, not duplication.
//!
//! The secret flows only into the `Authorization` header — never into a
//! URL, an error message, or a log line (CWE-532).

use secrecy::ExposeSecret as _;

use crate::auth::auth_error::AuthError;
use crate::auth::credential::Credential;
use crate::oci::access::registry_client::registry_host;

/// The successful result of a verification ping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyOutcome {
    /// The registry's auth endpoint accepted the credential.
    Verified,
    /// The registry answered `/v2/` without demanding authentication —
    /// there is nothing the credential could be checked against.
    NoAuthRequired,
}

/// Verify `cred` against `registry`'s auth endpoint before it is stored.
///
/// A `Verified` outcome proves the credential is accepted by the
/// registry's authentication endpoint — it does **not** prove push or
/// pull access to any particular repository.
///
/// `plain_http` is the invocation's complete plain-HTTP exception list
/// (`command::plain_http_hosts`) — the loopback forms, the
/// `GRIM_INSECURE_REGISTRIES` entries, and every `[[registries]]` entry that
/// set `insecure = true`. It decides the ping scheme for **this** registry
/// and nothing else; it deliberately does not widen which `Bearer` realms
/// may be followed (see [`realm_is_secure`]).
///
/// # Errors
///
/// [`AuthError::VerifyRejected`] when the registry refuses the credential
/// (exit 80); [`AuthError::VerifyUnavailable`] when the registry or its
/// token endpoint cannot be reached or answers with a server-side failure
/// such as a 5xx or 429 (exit 69).
pub async fn verify_credential(
    registry: &str,
    cred: &Credential,
    plain_http: &[String],
) -> Result<VerifyOutcome, AuthError> {
    let host = ping_host(registry);
    let scheme = if plain_http.contains(&host) { "http" } else { "https" };
    let client = client().map_err(|e| unavailable(registry, Some(e)))?;

    let ping_url = format!("{scheme}://{host}/v2/");
    let ping = client
        .get(&ping_url)
        .send()
        .await
        .map_err(|e| unavailable(registry, Some(e)))?;

    if ping.status().is_success() {
        tracing::warn!("registry {registry} does not require authentication; nothing to verify");
        return Ok(VerifyOutcome::NoAuthRequired);
    }

    // Anything other than a 401 carrying a challenge grim can answer
    // (5xx, 429, a 401 with an unknown scheme, …) is a registry-side
    // fault: the credential can be neither confirmed nor refuted.
    if ping.status() != reqwest::StatusCode::UNAUTHORIZED {
        return Err(status_unavailable(registry, ping));
    }
    let challenge = ping
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_challenge);
    let Some(challenge) = challenge else {
        return Err(status_unavailable(registry, ping));
    };

    let request = match challenge {
        Challenge::Basic => client.get(&ping_url),
        Challenge::Bearer { realm, service } => {
            // A malicious registry can answer an HTTPS ping with an
            // `http://` realm to harvest the Basic credential in cleartext.
            // Refuse the downgrade rather than follow it (nothing stored).
            if !realm_is_secure(&realm, scheme) {
                return Err(AuthError::VerifyInsecureRealm {
                    registry: registry.to_string(),
                });
            }
            let mut request = client.get(realm);
            if let Some(service) = service {
                request = request.query(&[("service", service)]);
            }
            request
        }
    };
    let answer = request
        .basic_auth(&cred.username, Some(cred.password.expose_secret()))
        .send()
        .await
        .map_err(|e| unavailable(registry, Some(e)))?;

    match classify_answer(answer.status().as_u16()) {
        AnswerClass::Accepted => Ok(VerifyOutcome::Verified),
        AnswerClass::Rejected => Err(AuthError::VerifyRejected {
            registry: registry.to_string(),
        }),
        AnswerClass::Unavailable => Err(status_unavailable(registry, answer)),
    }
}

/// The host the verification ping targets, derived from the (possibly
/// namespaced or docker-aliased) registry string.
///
/// The stored credential key may carry a namespace (`ghcr.io/acme`) but
/// `/v2/` is served by the bare host; docker.io's canonical key
/// `https://index.docker.io/v1/` is a legacy alias whose actual OCI
/// endpoint is `registry-1.docker.io`.
fn ping_host(registry: &str) -> String {
    let canonical = crate::auth::canonicalize_registry(registry);
    if canonical == "https://index.docker.io/v1/" {
        return "registry-1.docker.io".to_string();
    }
    registry_host(&canonical).to_string()
}

/// Whether a `Bearer` realm may be followed without downgrading the
/// credential to cleartext.
///
/// **A registry reached over HTTPS must keep the credential on HTTPS.** The
/// realm host is named by the *registry*, not by the user, so it is not a
/// host the user opted in to — and the answer to that challenge carries the
/// password itself.
///
/// This deliberately does **not** consult the invocation's plain-HTTP set.
/// That set exists for `oci-client`'s `HttpsExcept`, where a wrong match
/// costs an unwanted HTTP connection; here it would cost a leaked secret,
/// and the two are not the same decision. Consulting it let *any*
/// `insecure = true` entry — declared for some entirely different registry,
/// and committed to a shared `grimoire.toml` — authorize an HTTPS
/// registry's challenge to redirect the credential to that host in
/// cleartext. `Url::host_str()` also drops the port, so a port-less entry
/// matched the named host on *every* port.
///
/// A plain-HTTP ping is a registry the user did declare insecure, so any
/// realm is fine there — the credential was already going to travel in
/// cleartext. An unparseable realm is not trusted.
fn realm_is_secure(realm: &str, ping_scheme: &str) -> bool {
    if ping_scheme != "https" {
        return true;
    }
    matches!(reqwest::Url::parse(realm), Ok(url) if url.scheme() == "https")
}

/// A parsed `WWW-Authenticate` challenge grim knows how to answer.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Challenge {
    /// `Basic realm=…` — answer `/v2/` itself with basic auth.
    Basic,
    /// `Bearer realm=…,service=…` — request a scope-less token from the
    /// realm with basic auth.
    Bearer { realm: String, service: Option<String> },
}

/// Parse a `WWW-Authenticate` header value into a [`Challenge`].
///
/// Parameters are split on `,` — a quoted realm/service containing a
/// literal comma would mis-split, but no real registry emits one.
fn parse_challenge(header: &str) -> Option<Challenge> {
    let header = header.trim();
    let (scheme, params) = header.split_once(char::is_whitespace).unwrap_or((header, ""));
    match scheme.to_ascii_lowercase().as_str() {
        "basic" => Some(Challenge::Basic),
        "bearer" => {
            let mut realm = None;
            let mut service = None;
            for part in params.split(',') {
                let Some((key, value)) = part.split_once('=') else {
                    continue;
                };
                let value = value.trim().trim_matches('"');
                match key.trim().to_ascii_lowercase().as_str() {
                    "realm" => realm = Some(value.to_string()),
                    "service" => service = Some(value.to_string()),
                    _ => {}
                }
            }
            realm.map(|realm| Challenge::Bearer { realm, service })
        }
        _ => None,
    }
}

/// How a challenge-answer response classifies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnswerClass {
    /// 2xx — the credential was accepted.
    Accepted,
    /// 401 / 403 — the credential was refused.
    Rejected,
    /// Anything else (5xx, 429, …) — the endpoint failed; the credential
    /// is unproven either way.
    Unavailable,
}

/// Classify a challenge-answer HTTP status code.
fn classify_answer(status: u16) -> AnswerClass {
    match status {
        200..=299 => AnswerClass::Accepted,
        401 | 403 => AnswerClass::Rejected,
        _ => AnswerClass::Unavailable,
    }
}

/// Build a [`AuthError::VerifyUnavailable`] from an unexpected response,
/// carrying the status-bearing `reqwest` error when the status yields one.
fn status_unavailable(registry: &str, response: reqwest::Response) -> AuthError {
    unavailable(registry, response.error_for_status().err())
}

/// Build a [`AuthError::VerifyUnavailable`].
fn unavailable(registry: &str, source: Option<reqwest::Error>) -> AuthError {
    AuthError::VerifyUnavailable {
        registry: registry.to_string(),
        source: source.map(Box::new),
    }
}

/// HTTP client for the ping: 30s timeout, grim user-agent, embedded CA
/// roots merged with the system trust store (mirrors the forge client —
/// see [`crate::tls`]).
fn client() -> Result<reqwest::Client, reqwest::Error> {
    crate::tls::merge_embedded_roots(
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent(concat!("grim/", env!("CARGO_PKG_VERSION"))),
    )
    .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn realm_scheme_guards_against_cleartext_downgrade() {
        // HTTPS ping: an https realm is fine; an http realm to a public
        // host is refused (credential-downgrade attempt).
        assert!(realm_is_secure("https://auth.example/token", "https"));
        assert!(!realm_is_secure("http://attacker.example/token", "https"));
        // Unparseable realm is not trusted under an HTTPS ping.
        assert!(!realm_is_secure("not a url", "https"));
        // Plain-http ping is already an insecure registry — any realm is fine.
        assert!(realm_is_secure("http://whatever/token", "http"));
    }

    /// Regression: an HTTPS registry's challenge must not be able to
    /// redirect the credential to a host that some *other* `[[registries]]`
    /// entry declared `insecure = true`.
    ///
    /// This was reachable with real captured bytes: a bare-hostname entry
    /// (`oci = "registry.corp.internal"`, the shape the field's own docs
    /// recommend for a TLS-less in-cluster registry) put that host in the
    /// invocation's plain-HTTP set; `realm_is_secure` compared the realm's
    /// **port-less** `host_str()` against that set, so a completely
    /// different HTTPS registry answering with
    /// `Bearer realm="http://registry.corp.internal:9999/token"` was trusted
    /// and the Basic credential went out in cleartext.
    ///
    /// The guard is now scheme-only under an HTTPS ping, so no entry in any
    /// config can arm this. Each case below passed before the fix.
    #[test]
    fn an_unrelated_insecure_entry_cannot_redirect_the_credential() {
        for realm in [
            // The captured exploit: declared bare, challenged with a port.
            "http://registry.corp.internal:9999/token",
            // The same host, no port — a port-less list entry matched this too.
            "http://registry.corp.internal/token",
            // Loopback: in the implicit set on every invocation, so this was
            // trusted for *any* HTTPS registry without any config at all.
            "http://127.0.0.1:5000/token",
            "http://localhost/token",
        ] {
            assert!(
                !realm_is_secure(realm, "https"),
                "an HTTPS ping must never follow {realm} — the answer carries the password"
            );
        }
    }

    #[test]
    fn parse_challenge_basic() {
        assert_eq!(
            parse_challenge(r#"Basic realm="Registry Realm""#),
            Some(Challenge::Basic)
        );
        assert_eq!(parse_challenge("basic"), Some(Challenge::Basic));
    }

    #[test]
    fn parse_challenge_bearer_with_quoted_realm_and_service() {
        let parsed =
            parse_challenge(r#"Bearer realm="https://ghcr.io/token",service="ghcr.io",scope="repository:x:pull""#);
        assert_eq!(
            parsed,
            Some(Challenge::Bearer {
                realm: "https://ghcr.io/token".to_string(),
                service: Some("ghcr.io".to_string()),
            })
        );
    }

    #[test]
    fn parse_challenge_bearer_missing_service() {
        let parsed = parse_challenge(r#"Bearer realm="https://auth.example/token""#);
        assert_eq!(
            parsed,
            Some(Challenge::Bearer {
                realm: "https://auth.example/token".to_string(),
                service: None,
            })
        );
    }

    #[test]
    fn parse_challenge_bearer_without_realm_is_unanswerable() {
        assert_eq!(parse_challenge(r#"Bearer service="x""#), None);
    }

    #[test]
    fn parse_challenge_unknown_scheme_is_none() {
        assert_eq!(parse_challenge(r#"Negotiate abcdef"#), None);
        assert_eq!(parse_challenge(""), None);
    }

    #[test]
    fn classify_answer_maps_statuses() {
        assert_eq!(classify_answer(200), AnswerClass::Accepted);
        assert_eq!(classify_answer(204), AnswerClass::Accepted);
        assert_eq!(classify_answer(401), AnswerClass::Rejected);
        assert_eq!(classify_answer(403), AnswerClass::Rejected);
        assert_eq!(classify_answer(429), AnswerClass::Unavailable);
        assert_eq!(classify_answer(500), AnswerClass::Unavailable);
        assert_eq!(classify_answer(503), AnswerClass::Unavailable);
        assert_eq!(classify_answer(404), AnswerClass::Unavailable);
    }

    #[test]
    fn ping_host_maps_docker_io_to_registry_1() {
        // Both the raw user forms and the canonical stored key must land
        // on the real OCI endpoint — missing the mapping would misreport
        // every Docker Hub login as unreachable.
        assert_eq!(ping_host("docker.io"), "registry-1.docker.io");
        assert_eq!(ping_host("index.docker.io"), "registry-1.docker.io");
        assert_eq!(ping_host("https://index.docker.io/v1/"), "registry-1.docker.io");
    }

    #[test]
    fn ping_host_strips_namespace_and_scheme() {
        assert_eq!(ping_host("ghcr.io/acme"), "ghcr.io");
        assert_eq!(ping_host("https://ghcr.io/v2/"), "ghcr.io");
        assert_eq!(ping_host("localhost:5000"), "localhost:5000");
        assert_eq!(ping_host("localhost:5000/team/sub"), "localhost:5000");
    }
}
