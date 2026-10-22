// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The OTLP/HTTP transport.
//!
//! Records are minted through an `SdkLoggerProvider` whose only processor
//! keeps them in memory, then `opentelemetry-otlp`'s exporter is called once
//! with one batch over grim's own HTTP client. The crate's processors and its
//! default transport are never used; the exemption is argued in
//! `.agents/adr/adr_otel_usage_telemetry.md`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use opentelemetry::logs::{AnyValue, LogRecord as _, Logger as _, LoggerProvider as _, Severity};
use opentelemetry::{InstrumentationScope, KeyValue};
use opentelemetry_http::{Bytes, HttpClient, HttpError, Request, Response};
use opentelemetry_otlp::{LogExporter, Protocol, RetryPolicy, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::{LogBatch, LogExporter as _, LogProcessor, SdkLogRecord, SdkLoggerProvider};
use reqwest::header::{CONTENT_TYPE, HeaderName, HeaderValue, USER_AGENT};

use crate::telemetry::config::{ExportConfig, Headers};
use crate::telemetry::lock;
use crate::telemetry::payload::{AttrValue, Payload};

/// Why the request failed. Internal: ends in one debug line, endpoint redacted.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("network error")]
    Network,
    #[error("collector answered {0}")]
    Status(u16),
    #[error("timed out")]
    Timeout,
    #[error("could not build the exporter")]
    Build,
}

/// Sends the payload as one `POST` to `cfg.endpoint`.
///
/// # Errors
///
/// [`ExportError`] on any failure; the caller only logs it.
pub async fn send(cfg: &ExportConfig, payload: &Payload) -> Result<(), ExportError> {
    let failure = Arc::new(Mutex::new(None));
    let client = GrimHttpClient::new(cfg, Arc::clone(&failure)).ok_or(ExportError::Build)?;
    let scope = InstrumentationScope::builder("grim")
        .with_version(env!("CARGO_PKG_VERSION"))
        .build();
    let records = mint(payload, &scope);

    let mut exporter = LogExporter::builder()
        .with_http()
        .with_endpoint(cfg.endpoint.as_str())
        .with_protocol(Protocol::HttpBinary)
        .with_timeout(cfg.timeout)
        .with_retry_policy(RetryPolicy::disabled())
        .with_http_client(client)
        .build()
        .map_err(|_| ExportError::Build)?;
    exporter.set_resource(&resource(payload));

    let batch: Vec<(&SdkLogRecord, &InstrumentationScope)> = records.iter().map(|r| (r, &scope)).collect();
    match exporter.export(LogBatch::new(&batch)).await {
        Ok(()) => Ok(()),
        // The crate flattens every failure into a string; the client knows which it was.
        Err(_) => Err(lock(&failure).take().unwrap_or(ExportError::Build)),
    }
}

fn resource(payload: &Payload) -> Resource {
    Resource::builder_empty()
        .with_attributes(
            payload
                .resource
                .iter()
                .map(|(k, v)| KeyValue::new(k.clone(), v.clone())),
        )
        .build()
}

/// One INFO log record per event, in order, stamped with `observed_at`.
fn mint(payload: &Payload, scope: &InstrumentationScope) -> Vec<SdkLogRecord> {
    let collected = Collect::default();
    let provider = SdkLoggerProvider::builder()
        .with_log_processor(collected.clone())
        .build();
    let logger = provider.logger_with_scope(scope.clone());
    for event in &payload.events {
        let mut record = logger.create_log_record();
        record.set_event_name(event.name);
        record.set_severity_number(Severity::Info);
        record.set_severity_text("INFO");
        record.set_timestamp(payload.observed_at);
        record.set_observed_timestamp(payload.observed_at);
        for (key, value) in &event.attributes {
            record.add_attribute(*key, any_value(value));
        }
        logger.emit(record);
    }
    drop(provider);
    std::mem::take(&mut *lock(&collected.0))
}

fn any_value(value: &AttrValue) -> AnyValue {
    match value {
        AttrValue::Str(s) => s.clone().into(),
        AttrValue::Int(i) => (*i).into(),
        AttrValue::Double(d) => (*d).into(),
        AttrValue::Bool(b) => (*b).into(),
        AttrValue::StrArray(items) => AnyValue::ListAny(Box::new(items.iter().cloned().map(AnyValue::from).collect())),
    }
}

/// A log processor that only remembers what it was given: no worker, no flush.
#[derive(Debug, Clone, Default)]
struct Collect(Arc<Mutex<Vec<SdkLogRecord>>>);

impl LogProcessor for Collect {
    fn emit(&self, record: &mut SdkLogRecord, _scope: &InstrumentationScope) {
        lock(&self.0).push(record.clone());
    }

    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }

    fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
        Ok(())
    }
}

/// The transport the exporter is given. It owns the request headers: whatever
/// the crate put on the request, including its own merge of
/// `OTEL_EXPORTER_OTLP_[LOGS_]HEADERS`, is discarded; `content-type` is reset.
#[derive(Debug)]
struct GrimHttpClient {
    inner: reqwest::Client,
    headers: Vec<(HeaderName, HeaderValue)>,
    failure: Arc<Mutex<Option<ExportError>>>,
}

impl GrimHttpClient {
    fn new(cfg: &ExportConfig, failure: Arc<Mutex<Option<ExportError>>>) -> Option<Self> {
        let inner = crate::tls::merge_embedded_roots(reqwest::Client::builder())
            .connect_timeout(cfg.timeout)
            .timeout(cfg.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok()?;
        Some(Self {
            inner,
            headers: selected(&cfg.headers)?,
            failure,
        })
    }

    fn fail(&self, error: ExportError) {
        *lock(&self.failure) = Some(error);
    }
}

fn selected(headers: &Headers) -> Option<Vec<(HeaderName, HeaderValue)>> {
    use secrecy::ExposeSecret;
    headers
        .iter()
        .map(|(name, value)| {
            let mut value = HeaderValue::from_str(value.expose_secret()).ok()?;
            value.set_sensitive(true);
            Some((HeaderName::from_bytes(name.as_bytes()).ok()?, value))
        })
        .collect()
}

#[async_trait]
impl HttpClient for GrimHttpClient {
    async fn send_bytes(&self, request: Request<Bytes>) -> Result<Response<Bytes>, HttpError> {
        let (mut parts, body) = request.into_parts();
        parts.headers.clear();
        parts.headers.insert(
            USER_AGENT,
            HeaderValue::from_static(concat!("grim/", env!("CARGO_PKG_VERSION"))),
        );
        for (name, value) in &self.headers {
            parts.headers.insert(name.clone(), value.clone());
        }
        // Set last and as a constant: the crate's env-header merge can replace
        // its own value, and the body is always protobuf.
        parts
            .headers
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/x-protobuf"));

        let request = reqwest::Request::try_from(Request::from_parts(parts, body)).map_err(|_| {
            self.fail(ExportError::Build);
            HttpError::from("could not build the request")
        })?;
        // `reqwest::Error` can embed the URL, so it never leaves this function.
        let response = self.inner.execute(request).await.map_err(|err| {
            self.fail(if err.is_timeout() {
                ExportError::Timeout
            } else {
                ExportError::Network
            });
            HttpError::from("transport failure")
        })?;

        let status = response.status();
        if !status.is_success() {
            self.fail(ExportError::Status(status.as_u16()));
        }
        // The body is dropped unread: grim never looks at what a collector says.
        let mut out = Response::new(Bytes::new());
        *out.status_mut() = status;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use opentelemetry::logs::AnyValue;

    use super::*;
    use crate::telemetry::payload::Event;

    fn payload() -> Payload {
        Payload {
            resource: vec![("service.name".into(), "grim".into())],
            events: vec![
                Event {
                    name: "grim.command",
                    attributes: vec![
                        ("process.exit.code", AttrValue::Int(0)),
                        (
                            "grim.clients",
                            AttrValue::StrArray(vec!["claude".into(), "codex".into()]),
                        ),
                    ],
                },
                Event {
                    name: "grim.artifact.changed",
                    attributes: vec![("grim.ci", AttrValue::Bool(false))],
                },
            ],
            observed_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        }
    }

    #[test]
    fn each_event_becomes_one_record_in_order_with_typed_attributes() {
        let scope = InstrumentationScope::builder("grim").build();
        let records = mint(&payload(), &scope);
        let names: Vec<_> = records.iter().map(|r| r.event_name()).collect();
        assert_eq!(names, [Some("grim.command"), Some("grim.artifact.changed")]);
        assert!(records.iter().all(|r| r.severity_number() == Some(Severity::Info)));
        assert!(records.iter().all(|r| r.timestamp() == Some(payload().observed_at)));
        let attrs: Vec<_> = records[0]
            .attributes_iter()
            .map(|(k, v)| (k.as_str().to_owned(), v.clone()))
            .collect();
        assert!(attrs.contains(&("process.exit.code".to_owned(), AnyValue::Int(0))));
        let list = AnyValue::ListAny(Box::new(vec![AnyValue::from("claude"), AnyValue::from("codex")]));
        assert!(attrs.contains(&("grim.clients".to_owned(), list)));
    }

    #[test]
    fn invalid_header_makes_the_client_unbuildable() {
        let bad = Headers::new(vec![("x-a".into(), secrecy::SecretString::from("line\nbreak"))]);
        assert!(selected(&bad).is_none());
    }
}
