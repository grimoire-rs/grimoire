// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Scripted loopback HTTP hosts, gzip helpers and a log capture shared by
//! the index transport and sync tests.

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

/// A loopback port nobody listens on: bound, then released, so a connect is
/// refused at once on every platform (`127.0.0.1:1` waits out the connect
/// timeout on Windows).
pub(crate) fn closed_loopback_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
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

/// Everything logged on this thread at any level until dropped.
pub(crate) struct LogCapture {
    buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    _guard: tracing::subscriber::DefaultGuard,
}

struct Sink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("log buffer is never poisoned")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl LogCapture {
    pub(crate) fn start() -> Self {
        crate::log_switch::tracing_capture::arm();
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&buf);
        let guard = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_writer(move || Sink(std::sync::Arc::clone(&sink)))
                .with_max_level(tracing::Level::TRACE)
                .with_ansi(false)
                .without_time()
                .finish(),
        );
        Self { buf, _guard: guard }
    }

    pub(crate) fn text(&self) -> String {
        String::from_utf8(self.buf.lock().expect("log buffer is never poisoned").clone()).expect("tracing writes UTF-8")
    }
}
