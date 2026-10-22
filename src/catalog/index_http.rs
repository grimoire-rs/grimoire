// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! HTTP transport for package-index documents: the shared client, the
//! gzip-aware size-capped body reader, and the deadlines both rely on.

/// Time allowed to open the connection to the index host.
const INDEX_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Longest silence tolerated before the response headers and between body
/// reads. It resets on every read, so a large index on a slow link survives
/// for as long as bytes keep arriving.
const INDEX_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Ceiling for one whole fetch, however steadily the body arrives.
const INDEX_TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Decoded-body ceiling for index documents, several times the size of a
/// large index; it bounds a decompression bomb.
pub(crate) const INDEX_BODY_LIMIT: usize = 128 * 1024 * 1024;

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
            .user_agent(concat!("grim/", env!("CARGO_PKG_VERSION"))),
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
/// have arrived, then undo a gzip `Content-Encoding`.
///
/// The cap applies to the wire bytes and again to the decoded bytes: a gzip
/// bomb is tiny on the wire, and a decoded size is not in any header.
pub(crate) async fn read_capped(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, BodyError> {
    let gzip = is_gzip(&response)?;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > limit {
            return Err(BodyError::OverLimit { limit });
        }
        body.extend_from_slice(&chunk);
    }
    if !gzip {
        return Ok(body);
    }
    // Inflating is CPU-bound, so it stays off the async workers.
    let decoded = tokio::task::spawn_blocking(move || gunzip(&body, limit))
        .await
        .map_err(std::io::Error::other)
        .and_then(|inflated| inflated)
        .map_err(BodyError::Gzip)?;
    if decoded.len() > limit {
        return Err(BodyError::OverLimit { limit });
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::test_hosts::{
        accepts_gzip, gzip, http_200, read_head, spawn_capturing_host, spawn_fixed_host, spawn_scripted_host,
    };
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;

    async fn get(base: &str) -> reqwest::Response {
        http_client().unwrap().get(base).send().await.unwrap()
    }

    /// `read_capped` over a `200` carrying `body` under `content_encoding`.
    async fn read_encoded(content_encoding: &str, body: &[u8], limit: usize) -> Result<Vec<u8>, BodyError> {
        let extra = format!("content-encoding: {content_encoding}\r\n");
        let base = spawn_fixed_host(http_200(&extra, body)).await;
        read_capped(get(&base).await, limit).await
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
        assert_eq!(exact.len(), 1025);
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
        assert_eq!(whole.len(), 50);
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

        assert_eq!(body.len(), 120);
        assert!(
            started.elapsed() > read_timeout,
            "the fetch must outlast the read timeout"
        );
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
