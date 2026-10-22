// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! HTTP transport for package-index documents: the shared client, the
//! gzip-aware size-capped body reader, the bounded blob inflate, the
//! per-refresh deadline and the URLs derived from an index locator.

use reqwest::Url;
use sha2::{Digest, Sha256};

/// Time allowed to open the connection to the index host.
const INDEX_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Longest silence tolerated before the response headers and between body
/// reads. It resets on every read, so a large index on a slow link survives
/// for as long as bytes keep arriving.
const INDEX_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Ceiling for one whole fetch, however steadily the body arrives.
pub(crate) const INDEX_TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Decoded-body ceiling for index documents, several times the size of a
/// large index; it bounds a decompression bomb.
pub(crate) const INDEX_BODY_LIMIT: usize = 128 * 1024 * 1024;

/// Redirects followed before the next one is refused as a loop.
const MAX_REDIRECTS: usize = 5;

/// Why the hop from the last of `previous` to `next` must not be followed.
/// Following an https-to-http hop would send the request in the clear, and
/// the caller never chose plain HTTP.
fn redirect_refusal(previous: &[Url], next: &Url) -> Option<&'static str> {
    if previous.len() > MAX_REDIRECTS {
        return Some("too many redirects");
    }
    match previous.last() {
        Some(from) if from.scheme() == "https" && next.scheme() != "https" => {
            Some("redirect from https to http refused")
        }
        _ => None,
    }
}

/// The shared HTTP client for index fetches (embedded TLS roots, timeouts,
/// grim user-agent).
pub(crate) fn http_client() -> reqwest::Result<reqwest::Client> {
    build_client(INDEX_CONNECT_TIMEOUT, INDEX_READ_TIMEOUT, INDEX_TOTAL_TIMEOUT)
}

/// [`http_client`] with the three deadlines injected, so tests can use short ones.
fn build_client(
    connect: std::time::Duration,
    read: std::time::Duration,
    total: std::time::Duration,
) -> reqwest::Result<reqwest::Client> {
    crate::tls::merge_embedded_roots(
        reqwest::Client::builder()
            .connect_timeout(connect)
            .read_timeout(read)
            .timeout(total)
            .user_agent(concat!("grim/", env!("CARGO_PKG_VERSION")))
            .referer(false)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                match redirect_refusal(attempt.previous(), attempt.url()) {
                    Some(reason) => attempt.error(reason),
                    None => attempt.follow(),
                }
            })),
    )
    .build()
}

/// Why [`read_capped`] stopped before the body ended.
#[derive(Debug, thiserror::Error)]
pub(crate) enum BodyError {
    #[error("index body exceeds {} MiB once decoded", .limit / MIB)]
    OverLimit { limit: usize },
    #[error("unsupported Content-Encoding '{0}'")]
    UnsupportedEncoding(String),
    #[error("corrupt gzip index body: {0}")]
    Gzip(#[source] std::io::Error),
    #[error(transparent)]
    Transport(#[from] reqwest::Error),
}

const MIB: usize = 1024 * 1024;

/// GET `url` advertising gzip. The header is set by hand because reqwest's
/// `gzip` feature is crate-wide: enabling it would also make the OCI, forge
/// and auth clients send it and auto-decode, with no decoded-size cap.
pub(crate) fn index_get(client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
    client.get(url).header(reqwest::header::ACCEPT_ENCODING, "gzip")
}

/// The one `Content-Encoding` [`read_capped`] can undo besides none.
fn is_gzip(response: &reqwest::Response) -> Result<bool, BodyError> {
    let mut values = response.headers().get_all(reqwest::header::CONTENT_ENCODING).iter();
    let Some(first) = values.next() else {
        return Ok(false);
    };
    let value = String::from_utf8_lossy(first.as_bytes());
    match (value.trim().to_ascii_lowercase().as_str(), values.next()) {
        ("identity", None) => Ok(false),
        ("gzip" | "x-gzip", None) => Ok(true),
        // `br`, `deflate`, `zstd`, a coding list or repeated headers: only
        // gzip was advertised, so none of them is decodable.
        _ => Err(BodyError::UnsupportedEncoding(value.trim().chars().take(32).collect())),
    }
}

/// Inflate `compressed` (every gzip member), stopping one byte past `limit`
/// so a bomb never grows the buffer beyond the cap.
fn gunzip(compressed: &[u8], limit: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::MultiGzDecoder::new(compressed)
        .take(limit as u64 + 1)
        .read_to_end(&mut out)?;
    Ok(out)
}

/// Read `response` to the end, failing as soon as more than `limit` bytes
/// have arrived, then undo a gzip `Content-Encoding`. Returns the bytes and
/// their SHA-256.
///
/// The cap applies to the wire bytes and again to the decoded bytes: a gzip
/// bomb is tiny on the wire, and a decoded size is not in any header. The
/// digest names the returned bytes, so a transport encoding never changes it.
pub(crate) async fn read_capped(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<(Vec<u8>, [u8; 32]), BodyError> {
    let gzip = is_gzip(&response)?;
    let mut body = Vec::new();
    let mut hasher = Sha256::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > limit {
            return Err(BodyError::OverLimit { limit });
        }
        if !gzip {
            hasher.update(&chunk);
        }
        body.extend_from_slice(&chunk);
    }
    if !gzip {
        return Ok((body, hasher.finalize().into()));
    }
    // Inflating and hashing up to `limit` bytes is CPU-bound, so both stay off
    // the async workers.
    let (decoded, digest) = tokio::task::spawn_blocking(move || {
        let decoded = gunzip(&body, limit)?;
        let digest: [u8; 32] = Sha256::digest(&decoded).into();
        Ok::<_, std::io::Error>((decoded, digest))
    })
    .await
    .map_err(std::io::Error::other)
    .and_then(|inflated| inflated)
    .map_err(BodyError::Gzip)?;
    if decoded.len() > limit {
        return Err(BodyError::OverLimit { limit });
    }
    Ok((decoded, digest))
}

/// A published blob is a plain `.json.gz` file, but a host or CDN may have
/// decoded it in transit. Inflate `body` once when it opens with the gzip
/// magic bytes, otherwise keep it as served; fail once the result passes
/// `limit`. It never inflates twice: a nested gzip stream stays compressed
/// and fails the caller's digest check.
///
/// CPU-bound: call it from `spawn_blocking`.
pub(crate) fn inflate_bounded(body: Vec<u8>, limit: usize) -> Result<Vec<u8>, BodyError> {
    let out = if body.starts_with(&GZIP_MAGIC) {
        gunzip(&body, limit).map_err(BodyError::Gzip)?
    } else {
        body
    };
    if out.len() > limit {
        return Err(BodyError::OverLimit { limit });
    }
    Ok(out)
}

/// The first two bytes of every gzip member.
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// One refresh's time budget: every request takes the smaller of its own
/// timeout and what is left, so a slow source cannot outlast the total
/// however many requests it is spread over.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Deadline {
    end: std::time::Instant,
}

impl Deadline {
    pub(crate) fn new(total: std::time::Duration) -> Self {
        Self {
            end: std::time::Instant::now() + total,
        }
    }

    /// `own`, or the time left when that is less; zero once the deadline passed.
    pub(crate) fn cap(&self, own: std::time::Duration) -> std::time::Duration {
        own.min(self.end.saturating_duration_since(std::time::Instant::now()))
    }
}

/// The index directory of an HTTP locator, ending in `/`: `https://h/idx` and
/// `https://h/idx/all.json` both give `https://h/idx/`. `None` when the
/// locator carries a query or fragment, or names another `.json` document;
/// grim then probes no root.
pub(crate) fn index_dir(locator: &str) -> Option<Url> {
    let mut url = parse_plain(locator)?;
    let (dir, foreign_json) = split_locator(&url);
    if foreign_json {
        return None;
    }
    url.set_path(&format!("{dir}/"));
    Some(url)
}

/// `<dir>c/root.json`.
pub(crate) fn root_url(dir: &Url) -> Url {
    child(dir, "c/root.json")
}

/// `<dir>c/f/sha256/<digest>.json.gz`, or `None` unless `digest` is 64
/// lowercase hex digits, so a digest from the wire never shapes the path.
pub(crate) fn blob_url(dir: &Url, digest: &str) -> Option<Url> {
    super::index_root::is_digest(digest).then(|| child(dir, &format!("c/f/sha256/{digest}.json.gz")))
}

/// The flat package list of an HTTP locator: the locator itself when it names a
/// `.json` document, else `<locator>/all.json`.
pub(crate) fn flat_url(locator: &str) -> String {
    let base = locator.trim_end_matches('/');
    if base.ends_with(".json") {
        base.to_owned()
    } else {
        format!("{base}/all.json")
    }
}

/// The `stats.json` sidecar beside the index document: `<dir>stats.json`, also
/// when the locator names a `.json` document itself. `None` for a locator with
/// a query or fragment.
pub(crate) fn stats_url(locator: &str) -> Option<Url> {
    let mut url = parse_plain(locator)?;
    let (dir, _) = split_locator(&url);
    url.set_path(&format!("{dir}/stats.json"));
    Some(url)
}

/// An `http(s)` locator with no query and no fragment.
fn parse_plain(locator: &str) -> Option<Url> {
    let url = Url::parse(locator).ok()?;
    let plain = matches!(url.scheme(), "http" | "https") && url.query().is_none() && url.fragment().is_none();
    plain.then_some(url)
}

/// The locator's directory path without a trailing `/`, and whether its last
/// segment names a `.json` document other than `all.json`.
fn split_locator(url: &Url) -> (String, bool) {
    let path = url.path().trim_end_matches('/');
    match path.rsplit_once('/') {
        Some((parent, "all.json")) => (parent.to_owned(), false),
        Some((parent, last)) if last.ends_with(".json") => (parent.to_owned(), true),
        _ => (path.to_owned(), false),
    }
}

fn child(dir: &Url, relative: &str) -> Url {
    let mut url = dir.clone();
    url.set_path(&format!("{}{relative}", dir.path()));
    url
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::test_hosts::{
        accepts_gzip, gzip, http_200, read_head, spawn_capturing_host, spawn_fixed_host, spawn_scripted_host,
    };
    use sha2::{Digest, Sha256};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;

    async fn get(base: &str) -> reqwest::Response {
        http_client().unwrap().get(base).send().await.unwrap()
    }

    /// `read_capped` over a `200` carrying `body` under `content_encoding`.
    async fn read_encoded(content_encoding: &str, body: &[u8], limit: usize) -> Result<Vec<u8>, BodyError> {
        let extra = format!("content-encoding: {content_encoding}\r\n");
        let base = spawn_fixed_host(http_200(&extra, body)).await;
        read_capped(get(&base).await, limit).await.map(|(body, _)| body)
    }

    #[tokio::test]
    async fn reqwest_does_not_advertise_gzip_crate_wide() {
        // The OCI, forge and auth clients are built outside this module and
        // must stay raw: reqwest's `gzip` feature would turn the header on for
        // every one of them, and its decoder has no size cap.
        let (base, mut seen) = spawn_capturing_host().await;
        let plain = crate::tls::merge_embedded_roots(reqwest::Client::builder())
            .build()
            .unwrap();

        plain.get(&base).send().await.expect("the host answers");

        let head = seen.recv().await.expect("the host saw a request");
        assert!(
            !accepts_gzip(&head),
            "a client built without index_get must not offer gzip; the reqwest `gzip` feature leaked on:\n{head}"
        );
    }

    #[tokio::test]
    async fn read_capped_refuses_one_byte_past_the_limit() {
        let base = spawn_fixed_host(http_200("", &[b'x'; 1025])).await;

        assert!(
            matches!(
                read_capped(get(&base).await, 1024).await,
                Err(BodyError::OverLimit { limit: 1024 })
            ),
            "limit + 1 bytes must be refused"
        );
        let exact = read_capped(get(&base).await, 1025)
            .await
            .expect("a body of exactly the limit is accepted");
        assert_eq!(exact.0.len(), 1025);
    }

    #[tokio::test]
    async fn read_capped_counts_across_chunks() {
        let base = spawn_drip_host(5, Duration::from_millis(5)).await;

        assert!(
            matches!(
                read_capped(get(&base).await, 49).await,
                Err(BodyError::OverLimit { limit: 49 })
            ),
            "the fifth ten-byte chunk crosses a 49-byte limit"
        );
        let whole = read_capped(get(&base).await, 50)
            .await
            .expect("five chunks fit exactly");
        assert_eq!(whole.0.len(), 50);
    }

    #[tokio::test]
    async fn read_capped_counts_decoded_bytes() {
        let compressed = gzip(&[b'a'; 8192]);
        assert!(compressed.len() < 100, "the bomb must stay tiny on the wire");

        assert!(
            matches!(
                read_encoded("gzip", &compressed, 4096).await,
                Err(BodyError::OverLimit { limit: 4096 })
            ),
            "the cap must count the decoded size"
        );
        assert!(
            matches!(
                read_encoded("gzip", &compressed, 8191).await,
                Err(BodyError::OverLimit { .. })
            ),
            "decoded limit + 1 must be refused"
        );
        let decoded = read_encoded("gzip", &compressed, 8192)
            .await
            .expect("a decoded body of exactly the limit is accepted");
        assert_eq!(decoded, vec![b'a'; 8192]);
    }

    #[tokio::test]
    async fn read_capped_decodes_every_gzip_member() {
        let members = [gzip(b"abc"), gzip(b"def")].concat();

        let decoded = read_encoded("gzip", &members, 1024).await.expect("both members decode");

        assert_eq!(decoded, b"abcdef");
    }

    #[tokio::test]
    async fn read_capped_matches_the_encoding_case_insensitively() {
        for name in ["X-GZip", "GZIP"] {
            let decoded = read_encoded(name, &gzip(b"[]"), 1024).await.expect(name);
            assert_eq!(decoded, b"[]", "{name}");
        }
        let raw = read_encoded("Identity", b"[]", 1024).await.expect("identity");
        assert_eq!(raw, b"[]");
    }

    #[tokio::test]
    async fn read_capped_refuses_an_encoding_it_did_not_advertise() {
        for name in ["br", "deflate", "zstd", "gzip, br", "identity, gzip"] {
            let err = read_encoded(name, &gzip(b"[]"), 1024)
                .await
                .expect_err("only gzip was advertised");
            assert!(
                matches!(&err, BodyError::UnsupportedEncoding(v) if v == name),
                "{name}: {err}"
            );
        }
    }

    #[tokio::test]
    async fn read_capped_refuses_corrupt_gzip() {
        let whole = gzip(&[b'a'; 8192]);
        let truncated = &whole[..whole.len() - 8];
        for (what, body) in [("truncated", truncated), ("not gzip", b"not gzip at all".as_slice())] {
            let err = read_encoded("gzip", body, 1 << 20).await.expect_err(what);
            assert!(matches!(err, BodyError::Gzip(_)), "{what}: {err}");
        }
    }

    #[test]
    fn the_over_limit_message_counts_mebibytes() {
        let err = BodyError::OverLimit {
            limit: INDEX_BODY_LIMIT,
        };

        assert_eq!(err.to_string(), "index body exceeds 128 MiB once decoded");
    }

    /// A host answering with `chunks` ten-byte pieces, `gap` apart, after
    /// sending headers at once.
    async fn spawn_drip_host(chunks: usize, gap: Duration) -> String {
        spawn_scripted_host(move |mut sock| async move {
            let _ = read_head(&mut sock).await;
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                chunks * 10
            );
            let _ = sock.write_all(head.as_bytes()).await;
            for _ in 0..chunks {
                tokio::time::sleep(gap).await;
                if sock.write_all(&[b'x'; 10]).await.is_err() {
                    return;
                }
            }
            let _ = sock.shutdown().await;
        })
        .await
    }

    /// Longer than any test waits, so only a deadline can end the wait.
    const AMPLE: Duration = Duration::from_secs(60);

    /// The read timeout must fire on its own, far inside [`AMPLE`]'s total.
    const PROMPT: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn a_host_that_never_answers_trips_the_read_timeout() {
        let base = spawn_scripted_host(|mut sock| async move {
            let _ = read_head(&mut sock).await;
            tokio::time::sleep(AMPLE).await;
        })
        .await;
        let client = build_client(AMPLE, Duration::from_millis(200), AMPLE).unwrap();

        let started = std::time::Instant::now();
        let err = client.get(&base).send().await.expect_err("no answer must fail");

        assert!(err.is_timeout(), "the stall must surface as a timeout: {err}");
        assert!(started.elapsed() < PROMPT, "the read timeout, not the total, must fire");
    }

    #[tokio::test]
    async fn a_body_that_stalls_midway_trips_the_read_timeout() {
        let base = spawn_drip_host(1, AMPLE).await;
        let client = build_client(AMPLE, Duration::from_millis(200), AMPLE).unwrap();
        let response = client.get(&base).send().await.expect("headers arrive at once");

        let started = std::time::Instant::now();
        let err = read_capped(response, 1024).await.expect_err("a stalled body must fail");

        assert!(
            matches!(&err, BodyError::Transport(e) if e.is_timeout()),
            "the stall must surface as a timeout: {err}"
        );
        assert!(started.elapsed() < PROMPT, "the read timeout, not the total, must fire");
    }

    #[tokio::test]
    async fn a_slow_but_progressing_body_outlives_the_read_timeout() {
        // Twelve pieces 50 ms apart take 600 ms in all, longer than the
        // 500 ms read timeout: only a per-read deadline lets this through.
        let base = spawn_drip_host(12, Duration::from_millis(50)).await;
        let read_timeout = Duration::from_millis(500);
        let client = build_client(AMPLE, read_timeout, AMPLE).unwrap();

        let started = std::time::Instant::now();
        let response = client.get(&base).send().await.expect("headers arrive at once");
        let body = read_capped(response, 1024)
            .await
            .expect("progress keeps the fetch alive");

        assert_eq!(body.0.len(), 120);
        assert!(
            started.elapsed() > read_timeout,
            "the fetch must outlast the read timeout"
        );
    }

    fn sha256_of(bytes: &[u8]) -> [u8; 32] {
        Sha256::digest(bytes).into()
    }

    #[tokio::test]
    async fn read_capped_returns_the_sha256_of_the_bytes_it_returns() {
        let plain = spawn_fixed_host(http_200("", b"[1,2,3]")).await;
        let (body, digest) = read_capped(get(&plain).await, 1024).await.unwrap();
        assert_eq!(body, b"[1,2,3]");
        assert_eq!(digest, sha256_of(b"[1,2,3]"));

        // A content-encoding is transport: the digest names the decoded bytes.
        let gz = spawn_fixed_host(http_200("content-encoding: gzip\r\n", &gzip(b"[1,2,3]"))).await;
        let (body, digest) = read_capped(get(&gz).await, 1024).await.unwrap();
        assert_eq!(body, b"[1,2,3]");
        assert_eq!(digest, sha256_of(b"[1,2,3]"));
    }

    #[tokio::test]
    async fn read_capped_hashes_a_body_that_arrives_in_pieces() {
        let base = spawn_drip_host(5, Duration::from_millis(5)).await;

        let (body, digest) = read_capped(get(&base).await, 1024).await.unwrap();

        assert_eq!(digest, sha256_of(&body));
        assert_eq!(body, [b'x'; 50]);
    }

    #[tokio::test]
    async fn a_lying_content_length_does_not_widen_the_cap() {
        // The header claims 1 GiB; the cap counts the bytes that arrive and
        // never sizes a buffer from the header.
        let base = spawn_scripted_host(|mut sock| async move {
            let _ = read_head(&mut sock).await;
            let head = "HTTP/1.1 200 OK\r\ncontent-length: 1073741824\r\nconnection: close\r\n\r\n";
            let _ = sock.write_all(head.as_bytes()).await;
            let _ = sock.write_all(&[b'x'; 4096]).await;
            tokio::time::sleep(AMPLE).await;
        })
        .await;

        let started = std::time::Instant::now();
        let err = read_capped(get(&base).await, 1024).await.expect_err("past the cap");

        assert!(matches!(err, BodyError::OverLimit { limit: 1024 }), "{err}");
        assert!(
            started.elapsed() < PROMPT,
            "the cap must fire without waiting for the header's length"
        );
    }

    #[tokio::test]
    async fn read_capped_aborts_an_oversize_compressed_body_at_the_cap() {
        // Noise does not compress, so the blob stays larger than the cap on the wire.
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let noise: Vec<u8> = (0..4096)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect();
        let blob = gzip(&noise);
        assert!(blob.len() > 1024);
        let base = spawn_fixed_host(http_200("", &blob)).await;

        let err = read_capped(get(&base).await, 1024).await.expect_err("past the cap");

        assert!(matches!(err, BodyError::OverLimit { limit: 1024 }), "{err}");
    }

    #[test]
    fn inflate_bounded_aborts_a_bomb_at_the_limit() {
        let bomb = gzip(&vec![0u8; 1 << 20]);
        assert!(bomb.len() < 4096, "the bomb must stay tiny on the wire");

        assert!(matches!(
            inflate_bounded(bomb.clone(), 4096),
            Err(BodyError::OverLimit { limit: 4096 })
        ));
        // A ref that understates its size aborts at the ref size; one that
        // overstates it is held by the kind cap the caller passes instead.
        let exact = gzip(&[b'a'; 512]);
        assert_eq!(inflate_bounded(exact.clone(), 512).unwrap(), [b'a'; 512]);
        assert!(matches!(
            inflate_bounded(exact, 511),
            Err(BodyError::OverLimit { limit: 511 })
        ));
    }

    #[test]
    fn inflate_bounded_inflates_exactly_once() {
        let inner = gzip(b"payload");
        let twice = gzip(&inner);

        assert_eq!(inflate_bounded(twice, 1024).unwrap(), inner);
    }

    #[test]
    fn inflate_bounded_keeps_a_host_decoded_body_as_served() {
        let json = br#"{"schema":1}"#.to_vec();

        assert_eq!(inflate_bounded(json.clone(), 1024).unwrap(), json);
        assert!(matches!(
            inflate_bounded(json, 4),
            Err(BodyError::OverLimit { limit: 4 })
        ));
        assert!(inflate_bounded(Vec::new(), 4).unwrap().is_empty());
    }

    #[test]
    fn inflate_bounded_refuses_a_corrupt_member() {
        let whole = gzip(&[b'a'; 8192]);

        let err = inflate_bounded(whole[..whole.len() - 8].to_vec(), 1 << 20).expect_err("truncated");

        assert!(matches!(err, BodyError::Gzip(_)), "{err}");
    }

    #[tokio::test]
    async fn a_blob_is_inflated_once_whatever_the_content_encoding_says() {
        let json = br#"{"entries":{}}"#;
        // The blob as stored: no content-encoding, the magic bytes arrive.
        let stored = spawn_fixed_host(http_200("", &gzip(json))).await;
        // A store that labels the file's own gzip as a transport encoding.
        let labelled = spawn_fixed_host(http_200("content-encoding: gzip\r\n", &gzip(json))).await;
        // A host or CDN that decoded it in transit and dropped the label.
        let decoded = spawn_fixed_host(http_200("", json)).await;

        for (what, base) in [("stored", stored), ("labelled", labelled), ("decoded", decoded)] {
            let (body, _) = read_capped(get(&base).await, 1024).await.expect(what);
            assert_eq!(inflate_bounded(body, 1024).expect(what), json, "{what}");
        }
    }

    #[test]
    fn a_deadline_never_grants_more_than_it_has_left() {
        let ample = Deadline::new(Duration::from_secs(100));
        assert_eq!(ample.cap(Duration::from_secs(30)), Duration::from_secs(30));
        assert!(ample.cap(Duration::from_secs(1000)) <= Duration::from_secs(100));

        let spent = Deadline::new(Duration::ZERO);
        assert_eq!(spent.cap(Duration::from_secs(30)), Duration::ZERO);

        let shrinking = Deadline::new(Duration::from_millis(60));
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(shrinking.cap(Duration::from_secs(30)), Duration::ZERO);
    }

    fn dir_of(locator: &str) -> Option<String> {
        index_dir(locator).map(|u| u.to_string())
    }

    fn stats_of(locator: &str) -> Option<String> {
        stats_url(locator).map(|u| u.to_string())
    }

    #[test]
    fn the_index_directory_follows_the_discovery_table() {
        for (locator, dir) in [
            ("https://h/idx", "https://h/idx/"),
            ("https://h/idx/", "https://h/idx/"),
            ("https://h/idx//", "https://h/idx/"),
            ("https://h/idx/all.json", "https://h/idx/"),
            ("https://h/", "https://h/"),
            ("https://h", "https://h/"),
            ("https://h/all.json", "https://h/"),
            ("http://127.0.0.1:8080/a/b", "http://127.0.0.1:8080/a/b/"),
            ("https://user:pw@h/idx", "https://user:pw@h/idx/"),
        ] {
            assert_eq!(dir_of(locator).as_deref(), Some(dir), "{locator}");
        }
    }

    #[test]
    fn a_locator_the_table_leaves_without_a_probe_has_no_directory() {
        for locator in [
            "https://h/idx/other.json",
            "https://h/dist/index.json",
            "https://h/idx?token=1",
            "https://h/idx/all.json?token=1",
            "https://h/idx#frag",
            "https://h/idx?",
            "not a url",
            "ftp://h/idx",
        ] {
            assert_eq!(dir_of(locator), None, "{locator}");
        }
    }

    #[test]
    fn the_root_and_blob_urls_hang_off_the_directory() {
        let dir = index_dir("https://h/idx/all.json").unwrap();
        let digest = "0123456789abcdef".repeat(4);

        assert_eq!(root_url(&dir).as_str(), "https://h/idx/c/root.json");
        assert_eq!(
            blob_url(&dir, &digest).unwrap().as_str(),
            format!("https://h/idx/c/f/sha256/{digest}.json.gz")
        );
        let bare = index_dir("https://h").unwrap();
        assert_eq!(root_url(&bare).as_str(), "https://h/c/root.json");
    }

    #[test]
    fn a_blob_url_is_built_only_from_a_checked_digest() {
        let dir = index_dir("https://h/idx").unwrap();

        for digest in [
            "",
            "../../../../etc/passwd",
            &"A".repeat(64),
            &"a".repeat(63),
            &"a".repeat(65),
            &format!("{}/x", "a".repeat(62)),
            &"g".repeat(64),
        ] {
            assert_eq!(blob_url(&dir, digest), None, "{digest}");
        }
    }

    #[test]
    fn the_stats_url_is_the_sibling_of_the_index_document() {
        for (locator, stats) in [
            ("https://h/idx", "https://h/idx/stats.json"),
            ("https://h/idx/", "https://h/idx/stats.json"),
            ("https://h/idx/all.json", "https://h/idx/stats.json"),
            ("https://h", "https://h/stats.json"),
            ("https://h/", "https://h/stats.json"),
            (
                "https://index.example/dist/all.json",
                "https://index.example/dist/stats.json",
            ),
            ("https://h/dist/index.json", "https://h/dist/stats.json"),
            ("https://h/idx/other.json", "https://h/idx/stats.json"),
        ] {
            assert_eq!(stats_of(locator).as_deref(), Some(stats), "{locator}");
        }
    }

    #[test]
    fn a_query_or_fragment_fetches_no_stats() {
        for locator in [
            "https://h/idx?token=1",
            "https://h/idx/all.json?token=1",
            "https://h/idx#frag",
            "https://h/dist/index.json?x=1",
            "not a url",
        ] {
            assert_eq!(stats_of(locator), None, "{locator}");
        }
    }

    /// A host answering `/i` with a redirect to `/i+1` until `/hops`, which
    /// answers `200`. Every request head is recorded.
    async fn spawn_redirect_chain(hops: usize) -> (String, Arc<Mutex<Vec<String>>>) {
        let heads = Arc::new(Mutex::new(Vec::new()));
        let seen = heads.clone();
        let base = spawn_scripted_host(move |mut sock| {
            let seen = seen.clone();
            async move {
                let head = read_head(&mut sock).await;
                let step: usize = head
                    .split_whitespace()
                    .nth(1)
                    .and_then(|path| path.trim_start_matches('/').parse().ok())
                    .unwrap_or(0);
                seen.lock().unwrap().push(head);
                let response = if step < hops {
                    format!(
                        "HTTP/1.1 302 Found\r\nlocation: /{}\r\nset-cookie: s=1\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                        step + 1
                    )
                    .into_bytes()
                } else {
                    http_200("", b"[]")
                };
                let _ = sock.write_all(&response).await;
                let _ = sock.shutdown().await;
            }
        })
        .await;
        (base, heads)
    }

    fn header_lines(head: &str) -> Vec<String> {
        head.lines().map(|line| line.to_ascii_lowercase()).collect()
    }

    #[tokio::test]
    async fn a_chain_of_five_redirects_is_followed_and_a_sixth_hop_is_refused() {
        let (base, heads) = spawn_redirect_chain(5).await;
        let client = http_client().unwrap();
        let response = index_get(&client, &format!("{base}/0"))
            .send()
            .await
            .expect("five hops");
        assert_eq!(response.status(), 200);
        assert_eq!(heads.lock().unwrap().len(), 6);

        let (base, heads) = spawn_redirect_chain(6).await;
        let err = index_get(&client, &format!("{base}/0"))
            .send()
            .await
            .expect_err("the sixth redirect is refused");
        assert!(err.is_redirect(), "a refused redirect is a transport error: {err}");
        assert_eq!(heads.lock().unwrap().len(), 6, "no seventh request is made");
    }

    #[tokio::test]
    async fn an_http_locator_follows_http_redirects() {
        let (base, heads) = spawn_redirect_chain(1).await;

        let response = index_get(&http_client().unwrap(), &format!("{base}/0"))
            .send()
            .await
            .expect("http to http is followed");

        assert_eq!(response.status(), 200);
        assert_eq!(heads.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_redirect_carries_no_referer_and_no_cookie() {
        let (base, heads) = spawn_redirect_chain(1).await;

        index_get(&http_client().unwrap(), &format!("{base}/0"))
            .send()
            .await
            .expect("followed");

        for head in heads.lock().unwrap().iter() {
            let lines = header_lines(head);
            for name in ["referer:", "cookie:", "authorization:"] {
                assert!(
                    !lines.iter().any(|line| line.starts_with(name)),
                    "{name} must not be sent:\n{head}"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_cross_origin_redirect_arrives_without_the_locator_userinfo() {
        let (elsewhere, mut landed) = spawn_capturing_host().await;
        let (home, mut asked) = {
            let (tx, rx) = tokio::sync::mpsc::channel::<String>(1);
            let target = elsewhere.clone();
            let base = spawn_scripted_host(move |mut sock| {
                let (tx, target) = (tx.clone(), target.clone());
                async move {
                    let _ = tx.try_send(read_head(&mut sock).await);
                    let response = format!(
                        "HTTP/1.1 302 Found\r\nlocation: {target}/next\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    );
                    let _ = sock.write_all(response.as_bytes()).await;
                    let _ = sock.shutdown().await;
                }
            })
            .await;
            (base, rx)
        };
        let with_userinfo = home.replacen("http://", "http://user:secret@", 1);

        let response = index_get(&http_client().unwrap(), &with_userinfo)
            .send()
            .await
            .expect("cross-origin is followed");
        assert_eq!(response.status(), 200);

        let first = asked.recv().await.expect("the locator's host was asked");
        assert!(
            header_lines(&first)
                .iter()
                .any(|line| line.starts_with("authorization: basic ")),
            "the locator's own origin gets its userinfo:\n{first}"
        );
        let second = landed.recv().await.expect("the redirect target was asked");
        assert!(
            !header_lines(&second)
                .iter()
                .any(|line| line.starts_with("authorization:")),
            "another origin must never see the userinfo:\n{second}"
        );
    }

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn a_redirect_from_https_to_http_is_refused() {
        let from = [url("https://h/idx/c/root.json")];

        assert!(redirect_refusal(&from, &url("http://h/idx/c/root.json")).is_some());
        assert!(redirect_refusal(&from, &url("http://elsewhere/other")).is_some());
    }

    #[test]
    fn every_other_scheme_pair_and_origin_is_followed() {
        let https = [url("https://h/a")];
        let http = [url("http://h/a")];

        assert_eq!(redirect_refusal(&https, &url("https://h/b")), None);
        assert_eq!(redirect_refusal(&https, &url("https://cdn.example/b")), None);
        assert_eq!(redirect_refusal(&http, &url("http://h/b")), None);
        assert_eq!(redirect_refusal(&http, &url("https://h/b")), None);
    }

    #[test]
    fn the_downgrade_check_looks_at_the_hop_just_taken() {
        // An http hop earlier in the chain does not make a later http hop a downgrade.
        let chain = [url("https://h/a"), url("http://h/b")];

        assert_eq!(redirect_refusal(&chain, &url("http://h/c")), None);
    }

    #[tokio::test]
    async fn the_total_ceiling_still_cuts_off_a_body_that_never_ends() {
        // Pieces every 50 ms never trip the 500 ms read timeout; the 800 ms
        // ceiling is the only thing that can stop this fetch.
        let base = spawn_drip_host(1000, Duration::from_millis(50)).await;
        let client = build_client(AMPLE, Duration::from_millis(500), Duration::from_millis(800)).unwrap();
        let response = client.get(&base).send().await.expect("headers arrive at once");

        let err = read_capped(response, 1024 * 1024)
            .await
            .expect_err("the ceiling must fire");
        assert!(
            matches!(&err, BodyError::Transport(e) if e.is_timeout()),
            "the ceiling must surface as a timeout: {err}"
        );
    }
}
