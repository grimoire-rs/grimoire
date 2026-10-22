// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! URL redaction for telemetry output.

use crate::oci::git_provenance::normalize_remote_url;

/// Reduces a git remote to `https://host/owner/repo`, or `None` when it
/// cannot be done without keeping a secret or a local path.
///
/// Wraps [`normalize_remote_url`] (which release annotations also use, so it
/// is not edited) and strips first what it does not: query and fragment,
/// which would otherwise survive into the path.
pub fn repository_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if !raw.contains("://") && raw.split(':').next().is_some_and(|head| head.contains('/')) {
        return None;
    }
    let (scheme, rest) = raw.split_once("://").map_or(("", raw), |(s, r)| (s, r));
    // The authority is everything up to the first `/`; userinfo ends at its
    // last `@` (RFC 3986 §3.2.1), so a `#` or `@` inside a password stays in
    // the part that is dropped.
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (userinfo, host) = authority.rsplit_once('@').unwrap_or(("", authority));
    // `?` before the last `@` means the "userinfo" is really a query that
    // carries an `@`; refuse rather than guess which part is the host.
    if userinfo.contains('?') {
        return None;
    }
    let path = path.split(['?', '#']).next().unwrap_or_default();
    // A `?` or `#` in the host part ends the authority: whatever follows,
    // `/` included, is query or fragment text and must not become the path.
    // An scp-form host carries its path after `:`; the cut applies there too.
    let (host, path) = match host.split_once(['?', '#']) {
        Some((host, _)) => (host, ""),
        None => (host, path),
    };
    let path = path.trim_end_matches('/');
    let cleaned = if scheme.is_empty() {
        // An scp-form remote has its whole path in `host`; a trailing `/`
        // would hide its `.git` suffix from the normalizer.
        if path.is_empty() {
            host.to_string()
        } else {
            format!("{host}/{path}")
        }
    } else {
        format!("{scheme}://{host}/{path}")
    };
    let normalized = normalize_remote_url(&cleaned)?.to_lowercase();
    let normalized = normalized.trim_end_matches('/');
    (!normalized.contains('@')).then(|| normalized.to_string())
}

/// A URL safe to show in a debug line: scheme, host, port and path only.
///
/// An input that does not parse yields a constant, never the input.
pub fn url(raw: &str) -> String {
    const UNPARSABLE: &str = "<unparsable url>";
    let Ok(mut parsed) = reqwest::Url::parse(raw) else {
        return UNPARSABLE.to_string();
    };
    if parsed.set_username("").is_err() || parsed.set_password(None).is_err() {
        return UNPARSABLE.to_string();
    }
    parsed.set_query(None);
    parsed.set_fragment(None);
    parsed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_url_vectors() {
        let cases = [
            ("https://user:tok@Host/O/R.git?x=1#f", "https://host/o/r"),
            ("git@GitHub.com:Acme/Repo.git", "https://github.com/acme/repo"),
            ("ssh://git@host:2222/o/r.git", "https://host/o/r"),
            ("https://host/o/r.git/", "https://host/o/r"),
            ("git@srv:repo.git", "https://srv/repo"),
            ("https://host/repo.git", "https://host/repo"),
            ("https://tok@host/o/r?x=a@b", "https://host/o/r"),
            ("https://user:p#ss@host/o/r", "https://host/o/r"),
            ("https://host/o/r?X-Amz-Signature=abc&sig=1", "https://host/o/r"),
            ("https://host/o/r?x=/SECRET", "https://host/o/r"),
            ("https://host/o/r#f/SECRET", "https://host/o/r"),
            (
                "https://gitlab.com/group/sub/proj.git",
                "https://gitlab.com/group/sub/proj",
            ),
        ];
        for (raw, expected) in cases {
            assert_eq!(repository_url(raw).as_deref(), Some(expected), "{raw}");
        }
    }

    #[test]
    fn repository_url_rejects_local_and_empty() {
        for raw in [
            "/srv/git/repo.git",
            "file:///srv/repo",
            "",
            "   ",
            // No scheme and a `/` before the first `:` is a relative local
            // path (git's own rule), not scp-form.
            "work/a:b/repo.git",
            "../a:b/repo.git",
            // A query carrying an `@` before the path: which part is the host
            // is a guess, so refuse.
            "https://host?token=SECRET@other.example/o/r",
            // A `?`/`#` ends the authority, so a `/` after it is query text
            // and leaves no repository path.
            "https://host?token=ab/SECRET",
            "https://host#frag/SECRET",
        ] {
            assert_eq!(repository_url(raw), None, "{raw}");
        }
    }

    #[test]
    fn repository_url_never_keeps_a_secret() {
        // Hostile shapes: none may yield a string containing the secret.
        for raw in [
            "https://tok@host/o/r?x=a@b",
            "https://host?token=SECRET@other.example/o/r",
            "https://user:SECRET@host/o/r#SECRET",
            "https://user:SECRET@host:8443/o/r?access_token=SECRET",
            "git@host:o/r?token=SECRET@x",
            "https://SECRET@/o/r",
            "https://host?token=ab/SECRET",
            "https://host#frag/SECRET",
            "https://host/o/r?x=/SECRET",
            "https://host/o/r#f/SECRET",
        ] {
            if let Some(out) = repository_url(raw) {
                assert!(
                    !out.to_lowercase().contains("secret") && !out.contains('@'),
                    "{raw} -> {out}"
                );
            }
        }
    }

    #[test]
    fn url_keeps_scheme_host_port_path_only() {
        assert_eq!(
            url("https://user:pw@Collector.example:4318/v1/logs?token=abc#frag"),
            "https://collector.example:4318/v1/logs"
        );
        assert_eq!(url("http://127.0.0.1:4318/v1/logs"), "http://127.0.0.1:4318/v1/logs");
    }

    #[test]
    fn url_never_echoes_an_unparsable_input() {
        let out = url("not a url with SECRET");
        assert!(!out.contains("SECRET"));
        assert!(!url("mailto:SECRET@host").contains("SECRET"));
    }
}
