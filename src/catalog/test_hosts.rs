// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Scripted loopback HTTP hosts and gzip helpers shared by the index
//! transport tests.

use std::io::Write;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Spawn a host that hands every accepted connection to `serve`, so a
/// test can capture the request head, stall, or drip a body. Returns the
/// base URL.
pub(crate) async fn spawn_scripted_host<F, Fut>(serve: F) -> String
where
    F: Fn(TcpStream) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            tokio::spawn(serve(sock));
        }
    });
    base
}

/// A `200` carrying `body` verbatim, with `extra` raw header lines.
pub(crate) fn http_200(extra: &str, body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n{extra}\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}

pub(crate) async fn read_head(sock: &mut TcpStream) -> String {
    let mut buf = [0u8; 4096];
    let n = sock.read(&mut buf).await.unwrap_or(0);
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

pub(crate) async fn spawn_fixed_host(response: Vec<u8>) -> String {
    spawn_scripted_host(move |mut sock| {
        let response = response.clone();
        async move {
            let _ = read_head(&mut sock).await;
            let _ = sock.write_all(&response).await;
            let _ = sock.shutdown().await;
        }
    })
    .await
}

/// A host answering `[]` and handing the first request head to the
/// returned receiver.
pub(crate) async fn spawn_capturing_host() -> (String, tokio::sync::mpsc::Receiver<String>) {
    let (heads, seen) = tokio::sync::mpsc::channel::<String>(1);
    let base = spawn_scripted_host(move |mut sock| {
        let heads = heads.clone();
        async move {
            let _ = heads.try_send(read_head(&mut sock).await);
            let _ = sock.write_all(&http_200("", b"[]")).await;
            let _ = sock.shutdown().await;
        }
    })
    .await;
    (base, seen)
}

/// Whether the request head offers `Accept-Encoding: gzip`.
pub(crate) fn accepts_gzip(head: &str) -> bool {
    head.to_ascii_lowercase()
        .lines()
        .any(|line| line.starts_with("accept-encoding:") && line.contains("gzip"))
}

pub(crate) fn gzip(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}
