// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Credential-free rendering of URLs embedded in diagnostic text.
//!
//! Index errors and warnings quote the URL they failed on, and an index
//! locator may carry a token in its userinfo or query. Every log and error
//! site that can print one routes the text through [`redact_url`] first.

/// Rewrites every `scheme://…` substring in `text` to scheme, host, port and
/// path only, dropping userinfo, query and fragment. Text without a URL comes
/// back unchanged.
pub fn redact_url(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(sep) = rest.find("://") {
        let scheme_start = scheme_start(&rest[..sep]);
        let Some(scheme_start) = scheme_start else {
            out.push_str(&rest[..sep + 3]);
            rest = &rest[sep + 3..];
            continue;
        };
        out.push_str(&rest[..scheme_start]);
        // Userinfo runs to the authority's last `@` and may hold a quote, so the
        // quote delimiters apply only after it: ending the token inside userinfo
        // would leave the credential in the output.
        let after_sep = sep + 3;
        let authority_end = rest[after_sep..]
            .find(|c: char| c.is_whitespace() || matches!(c, '/' | '?' | '#' | '\\'))
            .map_or(rest.len(), |i| after_sep + i);
        let delimiters_from = rest[after_sep..authority_end]
            .rfind('@')
            .map_or(sep, |at| after_sep + at + 1);
        let token_end = rest[delimiters_from..]
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`'))
            .map_or(rest.len(), |i| delimiters_from + i);
        let (url, trailer) = split_trailer(&rest[scheme_start..token_end]);
        out.push_str(&redact_one(url));
        out.push_str(trailer);
        rest = &rest[token_end..];
    }
    out.push_str(rest);
    out
}

/// Start of the RFC 3986 scheme that ends `before`, if one does.
fn scheme_start(before: &str) -> Option<usize> {
    let start = before
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        .last()
        .map(|(i, _)| i)?;
    // The scheme must begin with a letter; `1://` or `-://` is not a URL.
    before[start..]
        .find(|c: char| c.is_ascii_alphabetic())
        .map(|offset| start + offset)
}

/// Splits prose punctuation off the end of a URL token: `.,;:` always, and a
/// closing bracket only when the token holds no matching opener.
fn split_trailer(token: &str) -> (&str, &str) {
    let mut end = token.len();
    loop {
        let url = &token[..end];
        let Some(last) = url.chars().last() else { break };
        let peel = match last {
            '.' | ',' | ';' | ':' => true,
            ')' => url.matches('(').count() < url.matches(')').count(),
            ']' => url.matches('[').count() < url.matches(']').count(),
            _ => false,
        };
        if !peel {
            break;
        }
        end -= last.len_utf8();
    }
    token.split_at(end)
}

fn redact_one(url: &str) -> String {
    let sep = url.find("://").map_or(0, |i| i + 3);
    let (scheme, after) = url.split_at(sep);
    // Authority ends where WHATWG ends it; userinfo is everything up to its last `@`.
    let authority_end = after.find(['/', '?', '#', '\\']).unwrap_or(after.len());
    let authority = &after[..authority_end];
    let host = authority.rfind('@').map_or(authority, |at| &authority[at + 1..]);
    let tail = &after[authority_end..];
    let path = &tail[..tail.find(['?', '#']).unwrap_or(tail.len())];
    format!("{scheme}{host}{path}")
}

#[cfg(test)]
mod tests {
    use super::redact_url;

    #[test]
    fn redact_url_table() {
        let cases = [
            ("no url here", "no url here"),
            ("", ""),
            ("https://index.example/all.json", "https://index.example/all.json"),
            (
                "https://user:pw@index.example:8443/p/all.json",
                "https://index.example:8443/p/all.json",
            ),
            (
                "https://index.example/all.json?token=abc#frag",
                "https://index.example/all.json",
            ),
            (
                "http://127.0.0.1:5000/c/root.json?x=1",
                "http://127.0.0.1:5000/c/root.json",
            ),
            ("https://tok@h?q", "https://h"),
            ("https://[::1]:8080/a?b", "https://[::1]:8080/a"),
            (
                "error sending request for url (https://u:p@h.example/all.json?t=1)",
                "error sending request for url (https://h.example/all.json)",
            ),
            (
                "fetch https://a@x.example/1?k, then https://y.example/2#f.",
                "fetch https://x.example/1, then https://y.example/2.",
            ),
            ("\"https://u@h.example/p?q\"", "\"https://h.example/p\""),
            ("git+ssh://git@host.example/repo.git", "git+ssh://host.example/repo.git"),
            ("https://h.example/p?next=https://u:p@z.example/", "https://h.example/p"),
            ("a 1://x b", "a 1://x b"),
            ("://nothing", "://nothing"),
            ("https://h.example/wiki/A_(b)", "https://h.example/wiki/A_(b)"),
            ("https://u:p'w@h.example/x", "https://h.example/x"),
            ("https://u:p\"w@h.example/x", "https://h.example/x"),
        ];
        for (input, want) in cases {
            assert_eq!(redact_url(input), want, "input: {input:?}");
        }
    }
}
