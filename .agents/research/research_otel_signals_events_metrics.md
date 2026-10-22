# Research: OTEL signals — events, span events, metrics for a short-lived CLI

<!--
Technology Landscape Research
Filename: .agents/research/research_otel_signals_events_metrics.md
Owner: Researcher (hex research phase)
Handoff to: Architect (/hex-architect), /hex-plan
Related Skills: hex-architect, hex-plan

Neutral evidence only: the requester asked for evidence for and against each option, no
recommendation. Sources older than ~18 months (before 2025-04-22) are marked [STALE].
Extends, does not redo: research_otel_cli_prior_art.md (spec env vars, receiver JSON support, crate
weight for traces only, failure latency for one signal, CLI gating prior art) and
research_otel_codebase_recon.md (attribute availability, tracing init).
-->

## Metadata

**Date:** 2026-10-22
**Domain:** observability
**Triggered by:** Owner follow-up on `.agents/discussions/otel-usage-telemetry.md`: "you wrote you put spans in, but what with events? ... a list of roughly planned metrics? And how context is collected?" This lane covers signal semantics (events, span events, logs, metrics) for the one-record-per-invocation design. Context collection (git-repo auto-detection) is not this lane's question; the adjacent facts found on the way are in finding 6.
**Expires:** 2027-04-22 (re-verify; the span-event deprecation, events semconv and delta ingestion in Prometheus/Mimir are all moving)

## Direct Answer

No recommendation. The four questions, answered from evidence (details and citations in **Key Findings**):

1. **Events.** An OTel "event" is a log record with a non-empty `EventName` (Logs data model, Stable). Span events (`Span.AddEvent`, `Span.RecordException`) are still normative in the spec at v1.61.0 (2026-09-14) with no deprecation marker in `trace/api.md`. OTEP 4430 (merged 2025-04-25) plans to deprecate that recording API in favour of log-based events, and an OTel blog post (March 2026) announces the direction. On 2026-10-15 a maintainer wrote on the open feedback issue that the project is "not rushing", and that span events might "never actually" be deprecated. The separate Events API was deprecated (spec v1.40) and removed (v1.41) in favour of the Logs API. The events semconv page is Development. The Logs API is Stable "except where otherwise specified"; its ergonomic event-emitting API is Development.
2. **Rust 0.33.1.** Logs API/SDK/OTLP exporter and `opentelemetry-appender-tracing` are Stable, metrics are Stable, traces are Beta. A binary can emit a named event with no tracing bridge by calling the `Logger` trait directly (`create_log_record()`, `set_event_name(&'static str)`, `emit()`); the crate docs say the logs module "is not intended to be called by application developers directly". Measured on a grim-like baseline: any OTel variant is 130 crates (baseline 106); logs add +127 KB and metrics +638 KB on top of traces (traces alone +604 KB); each signal is a separate HTTP request and a separate shutdown, and dead-collector latency adds up per signal (0.87 s, 1.8 s, 2.6 s for 1, 2, 3 signals, measured).
3. **Metrics from short-lived processes.** The Rust OTLP metric exporter defaults to cumulative and honours `OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE`. Backends differ: Datadog and New Relic document delta as preferred; Honeycomb accepts both and treats a start-time change as a counter reset; Prometheus' OTLP endpoint is cumulative-only unless an experimental flag is set; Mimir rejects delta unless an experimental flag is set; Grafana Cloud documents nothing on temporality in the pages read. The Collector's `deltatocumulative` processor is alpha and stateful and drops samples whose start time or timestamp is older than the series state. `spanmetrics` (alpha) derives calls, duration and a span-event count from spans; `count` (alpha) counts spans, span events and log records; `signaltometrics` (alpha) can sum an extracted value.
4. **Semconv.** The only CLI convention is `cli-spans` (Development): two spans, no CLI metrics and no CLI events. `process.*` metrics are OS-level (CPU, memory, I/O) and not command-level. Latest semconv release read: v1.44.0 (2026-08-04).

## Technology Landscape

### Trending (gaining momentum)

| Tool/Pattern | Adoption Signal | Key Benefit | Relevance to Grimoire |
|-------------|----------------|-------------|-------------------|
| Log-based events ("events are logs with names") | OTEP 4430 merged 2025-04-25; OTel blog 2026; Logs data model Stable | One event path for libraries and apps | Names the direction for any per-artifact record; Rust path is the `Logger` trait or `opentelemetry-appender-tracing` |
| Native delta ingestion in Prometheus-family backends | Prometheus `otlp-native-delta-ingestion` and Mimir `-distributor.otel-native-delta-ingestion` (Mimir PR merged 2025-07-07); Prometheus epic open | Removes the delta-to-cumulative conversion step | Delta is the natural shape for one-shot processes; both flags are experimental and off by default |
| Delta preferred by commercial backends | Datadog and New Relic docs recommend delta | Stateless per-process export | Rust SDK can emit it via env var |

### Established (proven, widely accepted)

| Tool/Pattern | Status | Notes |
|-------------|--------|-------|
| OTLP traces | Stable protocol; Rust SDK/exporter Beta | One span per process is the CLI-spans shape |
| Logs data model with `EventName` | Stable; proto `event_name` stable since proto 1.6.0 (2025-04-29) | Rust `LogRecord::set_event_name` exists |
| Rust metrics API/SDK/OTLP exporter | Stable per README | Periodic reader default 60 s, flushed on `shutdown()` |
| Collector `cumulativetodelta` | Beta | Stateful; Datadog docs name it for non-OTel sources |

### Emerging (early but promising)

| Tool/Pattern | Signal | Worth Watching Because |
|-------------|--------|----------------------|
| Collector `signaltometrics` connector | Alpha; traces, logs, metrics, profiles to metrics via OTTL | Can sum an extracted value, which `spanmetrics` and `count` do not document |
| Spec "Event to span event bridge" processor | Added to logs SDK spec in v1.56.0, status Development | The planned backward-compat path from log-based events to span events; no Rust implementation found |
| Prometheus start-timestamp flags (`st-storage`, `use-start-timestamps`) | Experimental | Would let `rate()`/`increase()` use `StartTimeUnixNano`; pages say nothing about the OTLP receiver |
| Semconv `vcs.*` and `mcp.*` namespaces | `vcs.*` Release Candidate; `mcp` model directory present in v1.44.0 | Adjacent to repo context and to `grim mcp`; not read in depth |

### Declining (losing mindshare)

| Tool/Pattern | Signal | Avoid Because |
|-------------|--------|---------------|
| `Span.AddEvent` / `Span.RecordException` for new code | Blog and OTEP 4430 say new code should use the Logs API; spec has not enacted a deprecation marker | Direction of travel, not yet a removal; existing span-event data stays valid OTLP |
| Separate Events API / `EventLogger` | Deprecated spec v1.40 (2024-12-12) [STALE], removed v1.41 (2025-01-21) [STALE] | Folded into the Logs API |
| `event.name` as a log attribute | Replaced by top-level `event_name` field in proto 1.5.0 (2024-12-12) [STALE] | Older semconv pages and mirrors still show attribute-style names |

## Design Patterns Worth Considering

- **One span per process with attributes only.** The CLI-spans shape. Used by: semconv `cli-spans` ([page](https://opentelemetry.io/docs/specs/semconv/cli/cli-spans/)).
- **Span plus log-record events correlated by trace context.** `Logger::emit` copies the active context's trace and span IDs into the record, and `LogRecord::set_trace_context` sets them explicitly ([logger.rs](https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry/src/logs/logger.rs), [LogRecord docs](https://docs.rs/opentelemetry/0.33.1/opentelemetry/logs/trait.LogRecord.html)). Described as the target shape in the [span events blog](https://opentelemetry.io/blog/2026/deprecating-span-events/).
- **Span events on the one span.** Still supported; ships in the same OTLP payload as the span ([blog](https://opentelemetry.io/blog/2026/deprecating-span-events/): "some users depend on events being sent in the same OTLP export payload as their parent span", as quoted in [community#3312](https://github.com/open-telemetry/community/issues/3312)).
- **Direct metric per invocation.** Docker CLI exports one `command.time` metric ([docs](https://docs.docker.com/engine/cli/otel/), Engine 26.1.0, 2024 [STALE], via the prior-art file finding 6); Claude Code exports metrics with delta temporality by default ([monitoring docs](https://code.claude.com/docs/en/monitoring-usage), via prior-art finding 4).
- **Collector-derived metrics from spans or logs.** `spanmetrics`, `count`, `signaltometrics` (finding 3.4). Alloy wraps all three ([spanmetrics](https://grafana.com/docs/grafana-cloud/send-data/alloy/reference/components/otelcol/otelcol.connector.spanmetrics/), [signaltometrics](https://grafana.com/docs/grafana-cloud/observe-and-act/send-data/alloy/reference/components/otelcol/otelcol.connector.signaltometrics/)).
- **Rust project guidance on who calls what.** Use the OTel Tracing API directly for custom spans, use `tracing` for logs and events, bridge logs with the appender ([docs/traces.md](https://github.com/open-telemetry/opentelemetry-rust/blob/main/docs/traces.md), [docs/logs.md](https://github.com/open-telemetry/opentelemetry-rust/blob/main/docs/logs.md); both pages are marked Work-In-Progress or Stable respectively, see finding 2.1).

## Key Findings

### 1. What an "event" is today, and the span-event migration

**1.1 Definition.** "Events are represented in the data model as an `EventRecord`, which is a log record with an event name." Events "describe named occurrences at a meaningful point in time" ([events semconv](https://opentelemetry.io/docs/specs/semconv/general/events/), Development, links spec v1.59.0). Spec data model: "A log record with a non-empty event name is an Event"; `EventName` "SHOULD uniquely identify the event structure (both attributes and body)" ([logs data model](https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/logs/data-model.md), Status Stable). The event name is a field, not an `event.name` attribute; the proto moved from the attribute to a top-level `event_name` field in 1.5.0 (2024-12-12) [STALE] and declared it stable in 1.6.0 (2025-04-29) ([proto CHANGELOG](https://github.com/open-telemetry/opentelemetry-proto/blob/main/CHANGELOG.md)). The field is present in the generated Rust proto (`opentelemetry-proto` 0.33.1, `logs.v1.rs:199`, from the local cargo registry).

**1.2 When to use which (semconv wording).** Spans: "For operations that have a duration and meaningful boundary - use spans instead." Span attribute: "when the data describes the operation as a whole." Events: occurrences that can happen zero or more times or need their own timestamp or severity, emitted inside or outside a trace context. "Event names MUST NOT include dynamic values" ([events semconv](https://opentelemetry.io/docs/specs/semconv/general/events/)). The OTel blog adds: "Not all logs are events, but all events are logs", events "do not have an explicit duration" and spans have explicit hierarchy that events lack ([Logging and You](https://opentelemetry.io/blog/2025/opentelemetry-logging-and-you/), URL year 2025, exact date not on the page).

**1.3 Span-event deprecation: timeline.**

| Date | Event | Source |
|---|---|---|
| 2024-12-12 [STALE] | Spec v1.40: separate Events API and SDK deprecated in favour of events in the Logs API | [spec CHANGELOG](https://github.com/open-telemetry/opentelemetry-specification/blob/main/CHANGELOG.md) (v1.40.0) |
| 2025-01-21 [STALE] | Spec v1.41: deprecated Events API and SDK removed | same (v1.41.0) |
| 2025-04-15 | Spec v1.44: `EventName` stabilized | same (v1.44.0) |
| 2025-04-25 | OTEP 4430 "Span Event API deprecation plan" merged (PR opened 2025-02-24) | [PR 4430](https://github.com/open-telemetry/opentelemetry-specification/pull/4430), via `gh api` |
| 2025-05-14 | OTEP listed in spec v1.45.0 changelog | spec CHANGELOG (v1.45.0) |
| 2026-03-05 | Feedback issue community#3312 opened | [community#3312](https://github.com/open-telemetry/community/issues/3312) |
| 2026-03 | Blog "Deprecating Span Events API" (page carries no date; a 2026-03-18 comment on community#3312 already quotes it; the Datadog March 2026 newsletter links it) | [blog](https://opentelemetry.io/blog/2026/deprecating-span-events/), [Datadog OSS newsletter](https://opensource.datadoghq.com/otel-news/2026/03/) |
| 2026-04 (heading reads 2025-04-20; sequence implies 2026) | Spec v1.56.0: "Add event to span event bridge" (logs SDK, Development) | spec CHANGELOG (v1.56.0), [logs SDK](https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/logs/sdk.md) |
| 2026-09-14 | Spec v1.61.0 is latest; `trace/api.md` still says a Span "MUST have the ability to add events" and carries no deprecation marker; the changelog sections for v1.55-v1.61 and Unreleased list no `AddEvent`/`RecordException` deprecation | [trace API](https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/trace/api.md), spec CHANGELOG (own grep) |
| 2026-10-15 | Maintainer (pellared) on community#3312: "We are not rushing with deprecating anything in the specification and APIs (rather the opposite) ... it is even possible that we may even never actually deprecate span events but just recommend log-based events." Another commenter (yurishkuro, Jaeger) is "shocked" and asks what happens to OTLP | [community#3312](https://github.com/open-telemetry/community/issues/3312) |

**1.4 What the plan says.** OTEP 4430 is narrower than the earlier OTEP 265: "we are only planning to deprecate the Span Event API, while retaining the ability to emit Span Events via the Logs API" ([OTEP 4430](https://github.com/open-telemetry/opentelemetry-specification/blob/fd43145dde7e5192ebc59a20992d98a3e6af5553/oteps/4430-span-event-api-deprecation-plan.md)). Steps: stabilize events and exceptions via the Logs API; deprecate `RecordException`; deprecate `AddEvent`; add an SDK log processor that turns Event log records into span events on the current span; stable instrumentations switch in their next major version. The blog adds: existing data "remains part of the supported OTLP trace model", stable majors stay behaviourally compatible "for now", and nothing is dated.

**1.5 Status per piece.**

| Piece | Status | Source |
|---|---|---|
| Logs data model, `EventName` | Stable | data model page above |
| Logs API | "Stable, except where otherwise specified" | [logs API](https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/logs/api.md) |
| Logs API ergonomic API ("convenient to emit event records following the event semantics") | Development | same (line 183) |
| Spec status page: Log Bridge API experimental support for emitting event-convention records | experimental wording | [status](https://opentelemetry.io/docs/specs/status/) |
| Events semconv | Development; "Event API is not yet available in all SDKs" per search summary of the page | [events semconv](https://opentelemetry.io/docs/specs/semconv/general/events/) |
| Event to span event bridge (SDK processor) | Development | logs SDK spec |
| Span `AddEvent`/`RecordException` | normative, no deprecation marker (v1.61.0) | trace API |

**1.6 Objections on record (community#3312).** Backends and pipelines that rely on events travelling with the span: tail sampling (borland, 2026-03-24, Honeycomb/refinery user: log events cannot follow a trace sampling decision made elsewhere), split trace and log Collector deployments (danielgblanco, 2026-07-21), pipeline transforms that read `spanevent.attributes` (amccague, 2026-03-18), backends that store traces atomically (yurishkuro, 2026-10-15). The maintainer reply on sampling: a Collector component to keep or drop log-based events by their trace's tail-sampling decision is being considered but "wouldn't be trivial to implement" (2026-03-24). Support comments: simpler mental model, one signal for events (software-artificer, 2026-04-01).

**1.7 What backends do with log-based events (pages read).** Jaeger v2 OTLP receiver: "Only tracing data is accepted, since Jaeger does not store other telemetry types" ([APIs](https://www.jaegertracing.io/docs/latest/apis/)). Honeycomb lists `v1/logs` and says you can "send traces, logs, and metrics" ([docs](https://docs.honeycomb.io/send-data/opentelemetry/)). Grafana Cloud/Loki: log `Body` becomes the line, attributes and scope go to structured metadata, `event_name` is not mentioned on either page ([Grafana Cloud format considerations](https://grafana.com/docs/grafana-cloud/send-data/otlp/otlp-format-considerations/), [Loki OTel](https://grafana.com/docs/loki/latest/send-data/otel/)); Loki states "Everything under LogRecord except `LogRecord.Body`, `LogRecord.TimeUnixNano` and sometimes `LogRecord.ObservedTimestamp`" is structured metadata, from which `event_name` landing there is an inference, not a statement.

### 2. opentelemetry-rust 0.33.x

**2.1 Stability and guidance** ([README](https://github.com/open-telemetry/opentelemetry-rust/blob/main/README.md), repo state 2026-10-22, 0.33.1 released 2026-10-08):

| Component | Status |
|---|---|
| Logs-API (footnoted), Logs-SDK, Logs-OTLP exporter, Logs-Appender-Tracing | Stable |
| Metrics-API, Metrics-SDK, Metrics-OTLP exporter | Stable (Prometheus exporter Beta) |
| Traces-API, Traces-SDK, Traces-OTLP exporter | Beta |
| `docs/logs.md` | "Status: Stable" |
| `docs/metrics.md`, `docs/traces.md` | "Status: Work-In-Progress" |

README footnote on the Logs-API row: "OpenTelemetry Rust is not introducing a new end user callable Logging API. Instead, it provides Logs Bridge API ... that allows one to write log appenders." If starting fresh it recommends `tracing` as the logging API. The `opentelemetry` crate docs repeat it: the `logs` module "is not intended to be called by application developers directly" ([lib.rs](https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry/src/lib.rs), lines 148-157). The spec's Logs API page, by contrast, says the API "can also be directly called by instrumentation libraries as well as instrumented libraries or applications" ([logs API](https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/logs/api.md)).

**2.2 Emitting a named event without a tracing bridge.** The `Logger` trait is `create_log_record()` and `emit(record)`; `LogRecord` has `set_event_name(&'static str)`, `set_target`, `set_timestamp`, `set_severity_*`, `set_body`, `add_attribute(s)`, and a provided `set_trace_context(trace_id, span_id, flags)` ([logger.rs](https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry/src/logs/logger.rs), [LogRecord](https://docs.rs/opentelemetry/0.33.1/opentelemetry/logs/trait.LogRecord.html)). `emit` "set[s] the record's TraceContext to the active trace context" when a thread-current `Context` exists. Own probe (0.33.1, compiled, ran, and a fake collector received `POST /v1/logs`; payload bytes not decoded):

```rust
let exporter = opentelemetry_otlp::LogExporter::builder().with_http().build()?;
let lp = opentelemetry_sdk::logs::SdkLoggerProvider::builder().with_batch_exporter(exporter).build();
let logger = lp.logger("grim");
let mut r = logger.create_log_record();
r.set_event_name("grim.artifact.installed");   // &'static str
r.add_attribute("artifact.kind", "skill");
logger.emit(r);
lp.shutdown().ok();
```

`set_event_name` takes `&'static str`, so a name built at run time does not fit the signature; the semconv rule "no dynamic values" matches that. No separate Events API or `EventLogger` type exists in the crate (searched; see Negative results).

**2.3 `opentelemetry-appender-tracing` 0.33.1** (2026-10-08, [CHANGELOG](https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry-appender-tracing/CHANGELOG.md), [Cargo.toml](https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry-appender-tracing/Cargo.toml)): default features none; optional `experimental_metadata_attributes`, `experimental_span_attributes`. It calls `log_record.set_event_name(metadata.name())` for every tracing event ([layer.rs](https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry-appender-tracing/src/layer.rs), `on_event`). In `tracing`, an event without an explicit `name:` gets the name `"event {file}:{line}"` ([tracing macros.rs](https://github.com/tokio-rs/tracing/blob/master/tracing/src/macros.rs), `concat!("event ", file!(), ":", line!())`), while `error!(name: "my-event-name", ...)` sets it explicitly (used in the appender's own tests). It forwards every event that reaches the layer: Rust's guidance is to filter on the OTel layer (`EnvFilter` per layer) or with a `LogProcessor`, and to exclude `reqwest`/`hyper`/`h2` targets to avoid export feedback loops ([docs/logs.md](https://github.com/open-telemetry/opentelemetry-rust/blob/main/docs/logs.md)). Local fact: grim has 353 `tracing` event sites and no spans today ([recon](research_otel_codebase_recon.md):17).

**2.4 `tracing-opentelemetry` 0.34.0** ([docs.rs](https://docs.rs/tracing-opentelemetry/latest/tracing_opentelemetry/)): targets `opentelemetry ^0.33.0`; "only traces and metrics" (logs go through the appender); "Metrics are not yet fully stable"; its `metrics` feature (default on) enables a `MetricsLayer` that "exports OpenTelemetry metrics from specifically-named events". Rust project guidance: for new code prefer the OTel Tracing API directly; the bridge is third-party ([docs/traces.md](https://github.com/open-telemetry/opentelemetry-rust/blob/main/docs/traces.md), item 4).

**2.5 Crate and binary cost** (own probe, 0.33.1, `opentelemetry-otlp` `http-proto` + `reqwest-blocking-client`, baseline mirrors grim's reqwest 0.13 rustls + tokio full + serde_json + tracing-subscriber, thin LTO, strip; single builds, Rust 1.95, x86_64 Linux):

| Variant | Crates (`cargo tree -e normal`) | Stripped binary | Delta vs baseline | Increment over traces-only |
|---|---|---|---|---|
| Baseline | 106 | 5,917,488 B | n/a | n/a |
| traces | 130 | 6,521,464 B | +603,976 B (+10.2%) | n/a |
| logs only | 130 | 6,470,848 B | +553,360 B (+9.4%) | n/a |
| metrics only | 130 | 6,934,952 B | +1,017,464 B (+17.2%) | n/a |
| traces + logs | 130 | 6,648,128 B | +730,640 B (+12.3%) | +126,664 B |
| traces + metrics | 130 | 7,159,784 B | +1,242,296 B (+21.0%) | +638,320 B |
| traces + logs + metrics | 130 | 7,290,288 B | +1,372,800 B (+23.2%) | +768,824 B |
| traces + logs + appender-tracing | 131 | 6,661,856 B | +744,368 B (+12.6%) | +140,392 B |
| all three + appender | 131 | 7,327,856 B | +1,410,368 B (+23.8%) | +806,392 B |

The 130 crates are the same set for every signal combination; `opentelemetry-appender-tracing` is the single added crate for the appender (`diff` of `cargo tree`). One `reqwest` (0.13.5) serves both the baseline and `opentelemetry-http` 0.33.1; the only duplicate in the tree is `syn` 2.x/3.x (`cargo tree -d`). That answers the discussion's open question on reqwest 0.13 compatibility for 0.33.1 with the blocking-client feature (the earlier probe's tree implied it; this one checked duplicates). The traces delta (+604 KB) is close to the prior file's +641 KB (heavier baseline there).

**2.6 One request, one shutdown per signal** (own probe, loopback fake collectors, WSL2, n=1 each, default retry on, the probe shuts the providers down one after another):

| Collector behaviour | traces | traces + logs | traces + logs + metrics |
|---|---|---|---|
| 200 | 0.02 s | 0.02 s | 0.02 s |
| connection refused | 0.87 s | 1.82 s | 2.62 s |
| HTTP 503 (each signal posted 4 times) | 0.92 s | 1.67 s | 2.67 s |
| accepts, never answers, `OTEL_EXPORTER_OTLP_TIMEOUT=500` | 0.52 s | 1.02 s | 1.52 s |
| accepts, never answers, default timeouts | 5.03 s | 10.04 s | 15.06 s |

Each signal arrived at its own path (`/v1/traces`, `/v1/logs`, `/v1/metrics`). The additive cost comes from the probe's sequential `shutdown()` calls; running the shutdowns concurrently was not measured. No multi-signal OTLP request exists; a commenter on community#3312 asks whether "multi-signal OTLP" would help, with no proposal located.

**2.7 Metrics specifics in the Rust SDK.** OTLP metric exporter: temporality "defaults to cumulative"; precedence is the builder's `with_temporality`, then `OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE`, then default; an unparsable value warns and falls back ([metric.rs](https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry-otlp/src/metric.rs)). Periodic reader: `OTEL_METRIC_EXPORT_INTERVAL`, default 60 s ([periodic_reader.rs](https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry-sdk/src/metrics/periodic_reader.rs)). `docs/metrics.md` (Work-In-Progress): "Always call `shutdown` on the `MeterProvider` at the end of your application to ensure all metrics are properly flushed and exported." Cardinality limits apply per export interval and are documented there.

### 3. Metrics from many short-lived processes

**3.1 Data model.** Cumulative: "successive data points repeat the starting timestamp"; delta: "successive data points advance the starting timestamp". "Cumulative data requires the sender to remember all previous measurements". "Delta temporality enables sampling and supports shifting the cost of cardinality outside of the process." `StartTimeUnixNano` is "strongly recommended" for Sum/Histogram "to correctly interpret the rate from an OTLP stream, in a manner that is aware of restarts". Delta-to-cumulative: "we need all incoming delta points to reach one destination". The page gives no guidance on short-lived or serverless processes ([metrics data model](https://opentelemetry.io/docs/specs/otel/metrics/data-model/); Temporality section Stable, Delta-to-Cumulative Development).

**3.2 Instance identity.** `service.instance.id` is Required (Identity, Stable): "MUST be unique for each instance of the same `service.namespace,service.name` pair", it distinguishes concurrently running instances, and the page does not address CLIs ([service resource semconv](https://opentelemetry.io/docs/specs/semconv/resource/service/)). Rust SDK default resource detectors: service name, telemetry SDK, `OTEL_RESOURCE_ATTRIBUTES` only; no `service.instance.id`; the contrib crate `opentelemetry-resource-detectors` (Beta, v0.13.0, 2026-09-21) has a `ServiceInstanceIdResourceDetector` ([resource/mod.rs](https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry-sdk/src/resource/mod.rs), [contrib README](https://github.com/open-telemetry/opentelemetry-rust-contrib/blob/main/opentelemetry-resource-detectors/README.md)). Grafana Cloud maps `service.instance.id` to the `instance` label, with `service.name` and others promoted to labels ([format considerations](https://grafana.com/docs/grafana-cloud/send-data/otlp/otlp-format-considerations/)). Inference, not sourced: a unique ID per invocation creates one series per invocation in label-based backends.

**3.3 Backend behaviour (pages read; nothing here was tested against a live backend).**

| Backend | Cumulative from many short processes | Delta | Source |
|---|---|---|---|
| Prometheus (OTLP endpoint) | Stored as sent; "cumulative temporality is the default in Prometheus". `increase()`/`rate()` treat "Breaks in monotonicity (such as counter resets due to target restarts)" as resets; the PromQL `resets()` entry defines a reset as "Any decrease in the value between two consecutive float samples". Inference: successive per-process values that do not decrease (for example 1, 1, 1) are not seen as resets. Start timestamps in PromQL need the experimental `use-start-timestamps` flag; the page does not say whether it covers OTLP | Experimental flag `otlp-deltatocumulative` converts in memory; state lost on restart; "can have negative impact on performance". Experimental flag `otlp-native-delta-ingestion` stores raw deltas with metric type unknown, `StartTimeUnixNano` "is ignored", query with `sum_over_time`; cannot be combined with the first flag; "very early stage of development" | [guide](https://prometheus.io/docs/guides/opentelemetry/), [feature flags](https://prometheus.io/docs/prometheus/latest/feature_flags/), [functions](https://prometheus.io/docs/prometheus/latest/querying/functions/); epic [prometheus#17649](https://github.com/prometheus/prometheus/issues/17649) open |
| Grafana Mimir | Accepted (OTLP endpoint) | Experimental `-distributor.otel-native-delta-ingestion`, default false: "If disabled, delta metrics will be rejected" (config reference); added by [PR 11631](https://github.com/grafana/mimir/pull/11631) (merged 2025-07-07); upstream conversion was ruled out because distributors are stateless ([issue 10439](https://github.com/grafana/mimir/issues/10439), comment 2025-01-15 [STALE as date]); open, last activity 2026-09-15 | [config reference](https://github.com/grafana/mimir/blob/main/docs/sources/mimir/configure/configuration-parameters/index.md) |
| Grafana Cloud OTLP | Not stated in the two pages read | Not stated; Alloy docs say Prometheus does not natively support delta and point to `otelcol.processor.deltatocumulative` (experimental in Alloy) | [send-data-otlp](https://grafana.com/docs/grafana-cloud/send-data/otlp/send-data-otlp/), [Alloy deltatocumulative](https://grafana.com/docs/grafana-cloud/observe-and-act/send-data/alloy/reference/components/otelcol/otelcol.processor.deltatocumulative/) |
| Datadog | Supported, but "Datadog takes the difference between consecutive points", requires all points of a series to reach one Agent/exporter, "might not send the first point", "may lead to missing points upon restarts"; histogram min/max lost | Recommended: "works best with delta aggregation temporality for monotonic sums, histograms, and exponential histograms"; set `OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE=Delta`. Short-lived processes not mentioned | [Datadog guide](https://docs.datadoghq.com/opentelemetry/guide/otlp_delta_temporality/) |
| Honeycomb | "Honeycomb supports both cumulative and delta metrics natively"; cumulative `INCREASE()` treats a lower later value as a reset; with a start time, "a change [in start time] is a reset, even if the new value is higher" | Delta suits `SUMMARIZE()`; many short-lived producers not addressed | [temporal aggregation](https://docs.honeycomb.io/investigate/query/temporal-aggregation) |
| New Relic | Supported, "generally a delta metrics system"; default cumulative "will generally incur more memory usage from SDKs and result in high data ingest" | "you should configure your OTLP metrics exporter to prefer delta" (counters, histograms delta; UpDownCounter/gauge cumulative) | [OTLP best practices](https://docs.newrelic.com/docs/opentelemetry/best-practices/opentelemetry-otlp/) |
| Azure Monitor | Metrics "need delta temporality" (prior-art file, finding 2 table) | n/a | [Azure docs](https://learn.microsoft.com/en-us/azure/azure-monitor/containers/opentelemetry-protocol-ingestion), ms.date 2026-05-27 |
| Dynatrace | Not stated (the OTLP export page read does not cover temporality; the metrics page URL tried returned 404) | not verified | [OTLP export](https://docs.dynatrace.com/docs/ingest-from/opentelemetry/getting-started/otlp-export) |

**3.4 Collector components (all from `metadata.yaml`/README on main, 2026-10-22).**

| Component | Stability | What it does | Relevant limits |
|---|---|---|---|
| `spanmetrics` connector | alpha (traces to metrics) | Metrics `calls` (request/error counts) and `duration` histogram per unique dimension set; optional `events` metric from span events ([README](https://github.com/open-telemetry/opentelemetry-collector-contrib/blob/main/connector/spanmetricsconnector/README.md)) | Default dimensions `service.name`, `span.name`, `span.kind`, `status.code`, `collector.instance.id`; more via `dimensions`; `aggregation_temporality` default CUMULATIVE (DELTA available); flush 60 s; `aggregation_cardinality_limit` default 0 (unlimited), overflow tagged `otel.metric.overflow="true"`; `resource_metrics_key_attributes` exists "in case changing resource attributes (e.g. process id) are breaking counter metrics"; a Single Writer Principle section; README documents no attribute-value sum |
| `count` connector | alpha (traces, metrics, logs, profiles to metrics) | Counts spans, span events, metrics, data points, log records; defaults `trace.span.count`, `trace.span.event.count`, `log.record.count`; custom counts by OTTL conditions and attributes ([README](https://github.com/open-telemetry/opentelemetry-collector-contrib/blob/main/connector/countconnector/README.md)) | Counts occurrences; no numeric-attribute sum documented; a separate count series per unique attribute-value set |
| `signaltometrics` connector | alpha (traces, logs, metrics, profiles to metrics) | `sum`, `gauge`, `histogram`, `exponential_histogram` from an OTTL `value` expression; example config says it produces "delta temporality counters"; "does NOT perform any stateful or time based aggregations" ([README](https://github.com/open-telemetry/opentelemetry-collector-contrib/blob/main/connector/signaltometricsconnector/README.md)) | Output is per `Consume` call, so a downstream aggregator is needed for Prometheus-family backends (Grafana's example routes it through `deltatocumulative`) |
| `deltatocumulative` processor | alpha, warning "Statefulness" | Accumulates delta streams; `max_stale` default 5m, `max_streams` default unlimited ([README](https://github.com/open-telemetry/opentelemetry-collector-contrib/blob/main/processor/deltatocumulativeprocessor/README.md)) | Source ([delta.go](https://github.com/open-telemetry/opentelemetry-collector-contrib/blob/main/processor/deltatocumulativeprocessor/internal/delta/delta.go)): a sample with a start time older than the series' start is dropped ("consider checking for multiple processes sending the exact same series"), and a sample with a timestamp at or before the series' last timestamp is dropped as out of order |
| `cumulativetodelta` processor | beta, warning "Statefulness" | Converts cumulative to delta | Datadog's guide names it for non-OTel sources and warns min/max cannot be recovered |

Derivable per source, from the READMEs above (not tested live):

| Question an operator may ask | From the span (spanmetrics) | From spans or log records (count / signaltometrics) | Emitted directly by grim (SDK metric) |
|---|---|---|---|
| Invocations by command, version, OS | `calls` by `span.name` or a span-attribute dimension | `count` of spans by attribute | Counter with attributes |
| Duration by command | `duration` histogram (explicit or exponential buckets; defaults 2 ms to 15 s) | `signaltometrics` histogram from span start/end | Histogram (Docker CLI `command.time` is the precedent) |
| Failures by exit code / `error.type` | `status.code` dimension plus an `error.type` dimension | `count` of spans with a condition | Counter with `error.type` |
| Sum of a per-invocation number (artifacts changed) | Not documented | `signaltometrics` `sum` with an OTTL value expression | `Counter::add(n, attrs)` |
| Counts per artifact kind or client | Needs one span attribute per kind, or a per-artifact span | `count` of log records or span events by attribute (one record per artifact) | Counter with a `kind` attribute |

Semconv naming rules for custom metrics: prefix with the application name (`grim.*`), lowercase dot-delimited snake_case, no units in the name, integer counts annotated like `{artifact}`, durations in seconds, and never an `otel.*` or existing-semconv prefix ([naming](https://opentelemetry.io/docs/specs/semconv/general/naming/), [metrics](https://opentelemetry.io/docs/specs/semconv/general/metrics/)).

### 4. Semconv for CLIs (semconv v1.44.0, 2026-08-04)

- `model/cli/` holds only `spans.yaml`; `docs/cli/` holds `README.md` and `cli-spans.md`. Two spans (`span.cli.internal`, `span.cli.client`), both `stability: development`, name `{process.executable.name}` with low-cardinality alternatives allowed if documented; error when `process.exit.code` is not 0; context propagation is a TODO ([cli-spans](https://opentelemetry.io/docs/specs/semconv/cli/cli-spans/), [model](https://github.com/open-telemetry/semantic-conventions/blob/main/model/cli/spans.yaml), [issue 1612](https://github.com/open-telemetry/semantic-conventions/issues/1612) open since 2024-11-25 [STALE as date]). The convention came from [issue 1577](https://github.com/open-telemetry/semantic-conventions/issues/1577) (opened 2024-11-14, closed 2024-11-26 [STALE]).
- No CLI metrics and no CLI events are defined on that page or in the model directory. `process.*` metrics are OS-level (`process.cpu.time`, `process.memory.usage`, `process.disk.io`, `process.uptime`, ...), page status Development, individual metrics mostly Release Candidate ([process metrics](https://opentelemetry.io/docs/specs/semconv/system/process-metrics/)). A search of semconv issues for "cli metrics" returned nothing.
- Prior-file point still holds: `process.executable.name`, `process.exit.code`, `process.pid` Required (RC), `process.command_args` Recommended and "SHOULD NOT be collected by default" without sanitization.

### 5. Evidence for and against each option

Neutral; each bullet points to the finding that carries the source. "Local" marks a statement about this repo's discussion.

**(a) Span only, one per invocation**
- For: matches the only CLI semconv (finding 4); one HTTP request and one shutdown (2.6); Rust guidance prefers the OTel Tracing API directly for custom spans (2.4); `spanmetrics` can derive calls, errors and duration from it (3.4); no dependence on the Logs path or on Events semconv; carries per-invocation fields as attributes. Local: matches the discussion's "one record per invocation" requirement (discussion line 45) and its collector-side derivation plan (line 115).
- Against: Rust traces SDK and exporter are Beta (2.1); per-artifact detail has to be flattened into span attributes or lost; `spanmetrics` documents no attribute-value sum, so "number of artifacts changed" needs `signaltometrics` or similar (3.4); derivation needs an operator-run alpha Collector component and a backend or Collector that handles the resulting temporality (3.3); operators on a trace-only store (Jaeger) get spans but no metrics derivation of their own.

**(b) Span plus log-record events per changed artifact** (and the variant (b') span events on the same span)
- For (b): events are the stated target for new code (1.3, 1.4); `EventName` and the Logs data model are Stable and the Rust logs path is Stable (1.1, 2.1); events carry their own timestamp and severity and correlate by trace context (1.2, 2.2); `count` can count log records by attribute (3.4); the event/span split matches the semconv rule "spans for duration, events for occurrences" (1.2).
- Against (b): the events semconv, the Logs API ergonomic API and the event-to-span-event bridge are Development (1.5); the Rust crate docs say not to call the Logs API directly and recommend `tracing` plus the appender (2.1), while the appender forwards every tracing event (353 sites locally, 2.3) and names unnamed events `event {file}:{line}`; a second signal is a second endpoint path, request and shutdown, so dead-collector latency roughly doubles (2.6); +127 KB (2.5); the operator needs a logs pipeline (Jaeger takes traces only, 1.7); no event-to-span-event bridge found in Rust (Negative results); log events do not follow trace sampling decisions made elsewhere (1.6). Local: the discussion requires "never one per artifact" (line 45) and lists logs as out of scope (line 21), so (b) revisits both.
- For (b'): no new signal or request; events travel in the span's own payload, which some pipelines depend on (1.6); `spanmetrics` can produce an events metric from span events and `count` counts `spanevents` (3.4); `Span::add_event` is still normative (1.3).
- Against (b'): the recording API is the one OTEP 4430 and the blog plan to deprecate (1.4), with the maintainer caveat of 2026-10-15 that it may never be formally deprecated (1.3); instrumentation authors are told to move in their next major versions (1.4).

**(c) Event only (one log record per invocation, no span)**
- For: one signal; an invocation can be described as an "occurrence"; Rust logs path is Stable; the log body and attributes are free-form; `count` and `signaltometrics` accept log records (3.4).
- Against: the semconv puts operations with duration and a meaningful boundary on spans (1.2) and defines CLI conventions only for spans, with `process.exit.code` and error status on the span (finding 4); no CLI event convention exists, so attribute names and the event name would be grim-defined; `spanmetrics` does not read logs (3.4); log events need a logs pipeline on the operator side (1.7) and the Logs-API-direct caveat in 2.1 applies; Loki documents no `event_name` mapping (1.7).

**(d) grim emits metrics directly, versus collector-derived**
- For direct: no dependency on alpha Collector components; Rust metrics SDK/exporter are Stable (2.1); `OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE=delta` is honoured by the SDK (2.7), and Datadog and New Relic document delta as preferred (3.3); Honeycomb accepts either (3.3); Docker CLI and Claude Code export metrics directly (design patterns); `sum` of per-invocation numbers is a plain `Counter::add`.
- Against direct: metrics add +638 KB over traces and another request and shutdown (2.5, 2.6); the SDK default is cumulative, which Mimir/Prometheus/Datadog/New Relic each handle differently for one-shot processes (3.3); delta is rejected by Mimir by default and needs an experimental flag on Prometheus (3.3); a per-invocation `service.instance.id` makes one series per invocation on label-based backends (3.2), while a shared identity makes concurrent processes overwrite one another (inference from the processor's drop rules in 3.4 and the Single Writer notes); 60 s default periodic interval means everything depends on the shutdown flush (2.7); `docs/metrics.md` is Work-In-Progress (2.1). Local: the discussion marks metrics out of scope (line 21).
- For collector-derived: the stateful, temporality-sensitive part runs in a long-lived component rather than in each short process; `spanmetrics` offers cumulative or delta output and a cardinality limit (3.4); the span is already the planned record.
- Against collector-derived: all three connectors are alpha; the operator must deploy and configure them (an operator exporting straight to a vendor intake has no derivation step); `spanmetrics` default dimensions are fixed and extra ones must be listed per operator, with an unlimited default cardinality (3.4); delta output from `signaltometrics` still needs conversion before Prometheus-family storage (3.4).

### 6. Adjacent: context collection (not this lane's question; facts found on the way)

- The Rust SDK's default resource has no host, OS, process or VCS information (3 detectors: SDK-provided service name, telemetry SDK, `OTEL_RESOURCE_ATTRIBUTES`). OS, process, host, container, k8s and service-instance detectors live in the separate Beta crate `opentelemetry-resource-detectors`, whose process detector reports `process.command_args` and `process.pid` and whose host detector reports `host.name` ([contrib README](https://github.com/open-telemetry/opentelemetry-rust-contrib/blob/main/opentelemetry-resource-detectors/README.md)). Semconv says `process.command_args` "SHOULD NOT be collected by default unless there is sanitization" (prior-art finding 6).
- No git or VCS resource detector was found in `opentelemetry-rust` or its contrib repo. The semconv `vcs.*` attributes exist (Release Candidate): `vcs.repository.url.full` ("canonical URL ... providing the complete HTTP(S) address"), `vcs.repository.name`, `vcs.ref.head.name`, `vcs.ref.head.revision` ([registry model](https://github.com/open-telemetry/semantic-conventions/blob/main/model/vcs/registry.yaml)); the registry text says nothing about credentials in URLs.
- Local facts for the owner's question (from the recon, not new): scope, clients and artifact refs resolve inside each command and would be recorded late onto the root span; only command, version, platform and exit code are known at dispatch (discussion lines 101-103; recon finding 6 table, `research_otel_codebase_recon.md:93-107`). grim already has `normalize_remote_url` for stripping git-remote credentials (`src/oci/git_provenance.rs:363`, per recon line 32).

## Method notes (own measurements)

- Probe crate `~/.cache/otel-signals-probe/` (outside tmpfs; deleted after this run). Baseline deps mirror grim: `tokio` full, `reqwest 0.13` default-features off + `rustls`, `serde_json`, `serde`, `tracing`, `tracing-subscriber` (env-filter, std, fmt, json, registry). `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp`, `opentelemetry-appender-tracing` pinned `=0.33.1`; otlp features `http-proto` + `reqwest-blocking-client` + the signal feature. Profile: opt-level 3, thin LTO, strip. Sizes are single builds on Rust 1.95; crate counts are unique `name version` lines from `cargo tree -e normal --prefix none`. The probe's `main` exercises each signal (a span with `process.exit.code`, a log record with an event name, a counter) so the code is not dead-stripped.
- Latency probe: Python-socket fake collectors on 127.0.0.1 (200, 503, accept-and-hang) plus a closed port; each binary run once per cell, wall time from a Python harness; env stripped of all inherited `OTEL_*` variables. WSL2, order-of-magnitude only. The earlier prior-art file measured one signal at 0.82-0.97 s for refused/503; this run's 0.87/0.92 s agree.
- GitHub reads via `gh api` raw content on `main` at 2026-10-22 (spec v1.61.0, semconv v1.44.0, opentelemetry-rust 0.33.1, collector-contrib `main`). WebFetch answers are paraphrases by a small model; verbatim quotes above were taken from the fetched text or the raw files read with `gh`.
- No live backend (Prometheus, Mimir, Grafana Cloud, Datadog, Honeycomb, Collector) was contacted.

## Recommendation

None. The requester asked for evidence for and against each option; the decisions are for `/hex-architect`.

## Negative results

- The span-event blog post page carries no date, author or timeline, and OTEP 4430 gives no dates or version milestones; the blog month rests on comment dates and a newsletter, not on the page.
- No spec change enacting the `AddEvent`/`RecordException` deprecation was found (trace API at v1.61.0, changelog v1.55-v1.61 and Unreleased).
- No separate Events API or `EventLogger` type in `opentelemetry-rust`; the only related issue found is [#1349](https://github.com/open-telemetry/opentelemetry-rust/issues/1349) (2023-11-07 [STALE], closed). No Rust implementation of the event-to-span-event bridge processor found (code search for "span event" hit only unrelated files).
- No CLI metrics or CLI events convention in semconv; no `cli.*` namespace; semconv issue search for "cli metrics" empty.
- Grafana Cloud, Mimir (docs pages read) and Dynatrace pages do not describe temporality handling; the Mimir statements come from its config reference and PR. Honeycomb and Datadog pages do not mention short-lived processes.
- No source found on what Prometheus, Mimir or Grafana Cloud do when many short-lived processes export cumulative counters with the same resource; the equal-value reset case above is inference from PromQL wording. No source on deltatocumulative behaviour specifically for parallel short-lived writers beyond the drop rules in `delta.go`.
- Loki and Grafana Cloud pages do not mention `event_name`.
- No proposal located for multi-signal OTLP requests (one commenter's question only).
- The Prometheus guide page has no mention of short-lived/batch jobs, and the feature-flag page gives no version for the delta flags.
- Honeycomb's metrics-specific pages were reached through search summaries and the temporal-aggregation page; the OTLP page read does not mention temporality.
- Elastic 9.5 native delta/cumulative storage appeared only in a search summary and was not opened.

## Leads

- **backend-interop-test:** run N short-lived processes against a sandbox Prometheus (default, `otlp-deltatocumulative`, `otlp-native-delta-ingestion`), a Mimir, and a Collector with `deltatocumulative`, in cumulative and delta modes, with and without a per-process `service.instance.id`; the inference rows in 3.3 are unverified. Production-adjacent targets need owner confirmation first.
- **event-render-test:** send one span plus correlated log-record events (and the span-event variant) through a Collector debug exporter and a sandbox Honeycomb/Grafana stack to see how each shows up in trace views; `event_name` handling in Loki/Grafana Cloud is undocumented.
- **concurrent-shutdown-measurement:** measure shutting down trace, log and metric providers in parallel against the dead-collector cases in 2.6; only the sequential cost was measured.
- **collector-recipe:** write and run `spanmetrics` + `signaltometrics` + `deltatocumulative` configs for the capability table in 3.4 against recorded spans, to confirm the "not documented" cells (attribute-value sum, events metric).
- **appender-filtering-design:** how a per-layer filter would admit only named telemetry events from the 353 existing `tracing` sites (and the `event {file}:{line}` default name) if the appender path were used.
- **semconv-upstream-check:** whether a CLI metrics or CLI events proposal exists beyond issue 1612, and whether the `mcp` model directory (v1.44.0, not read) covers `grim mcp` sessions for v2.
- **context-collection-design:** git/VCS attributes (`vcs.*` RC), URL sanitization (OBS-22, `normalize_remote_url`), and the process/host detector hazards; belongs to the security-threat-model and attribute-set-design leads of the prior-art file.

## Sources

| Source | Type | Date | Relevance |
|--------|------|------|-----------|
| https://opentelemetry.io/blog/2026/deprecating-span-events/ | Blog | 2026-03 (undated on page) | Span Event API deprecation announcement |
| https://github.com/open-telemetry/opentelemetry-specification/blob/fd43145dde7e5192ebc59a20992d98a3e6af5553/oteps/4430-span-event-api-deprecation-plan.md | OTEP | merged 2025-04-25 | Plan and stages |
| https://github.com/open-telemetry/opentelemetry-specification/pull/4430 | PR | 2025-02-24 to 2025-04-25 | Merge dates |
| https://github.com/open-telemetry/community/issues/3312 | Issue | opened 2026-03-05, active to 2026-10-15 | Objections, maintainer statement |
| https://opentelemetry.io/blog/2025/opentelemetry-logging-and-you/ | Blog | 2025 (exact date not on page) | Events are logs with names |
| https://opensource.datadoghq.com/otel-news/2026/03/ | Newsletter | 2026-03 | Second source for the blog's timing |
| https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/trace/api.md | Spec | v1.61.0 | AddEvent still normative |
| https://github.com/open-telemetry/opentelemetry-specification/blob/main/CHANGELOG.md | Spec | v1.61.0 (2026-09-14) | Events API deprecation/removal, EventName, bridge |
| https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/logs/api.md | Spec | v1.61.0 | Logs API status, direct-use statement |
| https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/logs/data-model.md | Spec | v1.61.0 | EventName, Stable |
| https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/logs/sdk.md | Spec | v1.61.0 | Event to span event bridge, Development |
| https://opentelemetry.io/docs/specs/status/ | Spec | undated | Signal status table |
| https://opentelemetry.io/docs/specs/semconv/general/events/ | Semconv | Development (links v1.59.0) | Event definition and naming |
| https://github.com/open-telemetry/opentelemetry-proto/blob/main/CHANGELOG.md | Repo | 1.11.1 = 2026-09-25 | event_name field history |
| https://github.com/open-telemetry/opentelemetry-rust/blob/main/README.md | Repo | 0.33.1 | Per-signal stability, Logs-API footnote |
| https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry/src/lib.rs | Source | 0.33.1 | "not intended to be called by application developers directly" |
| https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry/src/logs/logger.rs | Source | 0.33.1 | Logger trait, emit context |
| https://docs.rs/opentelemetry/0.33.1/opentelemetry/logs/trait.LogRecord.html | Docs | 0.33.1 | set_event_name, set_trace_context |
| https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry-appender-tracing/src/layer.rs | Source | 0.33.1 | Appender sets event_name |
| https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry-appender-tracing/CHANGELOG.md | Repo | 0.33.1 = 2026-10-08 | Appender history |
| https://github.com/open-telemetry/opentelemetry-rust/blob/main/docs/logs.md | Repo doc | Status Stable | Rust logs guidance, filtering |
| https://github.com/open-telemetry/opentelemetry-rust/blob/main/docs/traces.md | Repo doc | Work-In-Progress | Direct tracing API vs bridge |
| https://github.com/open-telemetry/opentelemetry-rust/blob/main/docs/metrics.md | Repo doc | Work-In-Progress | Shutdown, cardinality |
| https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry-otlp/src/metric.rs | Source | 0.33.1 | Temporality env var |
| https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry-sdk/src/metrics/periodic_reader.rs | Source | 0.33.1 | 60 s default interval |
| https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry-sdk/src/resource/mod.rs | Source | 0.33.1 | Default resource detectors |
| https://github.com/open-telemetry/opentelemetry-rust-contrib/blob/main/opentelemetry-resource-detectors/README.md | Repo | v0.13.0 = 2026-09-21 | Detector list, Beta |
| https://docs.rs/tracing-opentelemetry/latest/tracing_opentelemetry/ | Docs | 0.34.0 | Traces and metrics only |
| https://github.com/tokio-rs/tracing/blob/master/tracing/src/macros.rs | Source | main | Default event name `event file:line` |
| https://opentelemetry.io/docs/specs/otel/metrics/data-model/ | Spec | Temporality Stable | Cumulative vs delta |
| https://opentelemetry.io/docs/specs/semconv/resource/service/ | Semconv | Stable | service.instance.id |
| https://opentelemetry.io/docs/specs/semconv/general/naming/ | Semconv | undated | Custom name rules |
| https://opentelemetry.io/docs/specs/semconv/general/metrics/ | Semconv | undated | Metric naming |
| https://opentelemetry.io/docs/specs/semconv/cli/cli-spans/ | Semconv | Development | CLI spans |
| https://github.com/open-telemetry/semantic-conventions/blob/main/model/cli/spans.yaml | Repo | v1.44.0 = 2026-08-04 | Only CLI model file |
| https://github.com/open-telemetry/semantic-conventions/issues/1577 and /1612 | Issues | 2024-11 [STALE] | CLI convention origin, propagation TODO |
| https://opentelemetry.io/docs/specs/semconv/system/process-metrics/ | Semconv | Development | process.* metrics are OS-level |
| https://github.com/open-telemetry/semantic-conventions/blob/main/model/vcs/registry.yaml | Repo | v1.44.0 | vcs.* attributes, RC |
| https://prometheus.io/docs/guides/opentelemetry/ | Docs | undated | Cumulative default, deltatocumulative flag |
| https://prometheus.io/docs/prometheus/latest/feature_flags/ | Docs | undated | Native delta flag, start-timestamp flags |
| https://prometheus.io/docs/prometheus/latest/querying/functions/ | Docs | undated | Reset semantics |
| https://github.com/prometheus/prometheus/issues/17649 | Issue | opened 2025-12-04, updated 2026-09-17 | Delta epic |
| https://github.com/grafana/mimir/pull/11631 | PR | merged 2025-07-07 | Mimir delta flag |
| https://github.com/grafana/mimir/issues/10439 | Issue | opened 2025-01-15 [STALE as date], updated 2026-09-15 | Why Mimir cannot convert |
| https://github.com/grafana/mimir/blob/main/docs/sources/mimir/configure/configuration-parameters/index.md | Docs | main | "delta metrics will be rejected" |
| https://grafana.com/docs/grafana-cloud/send-data/otlp/otlp-format-considerations/ | Docs | undated | instance label, Loki mapping |
| https://grafana.com/docs/grafana-cloud/send-data/otlp/send-data-otlp/ | Docs | undated | No temporality statement |
| https://grafana.com/docs/loki/latest/send-data/otel/ | Docs | undated | Log record mapping |
| https://grafana.com/docs/grafana-cloud/observe-and-act/send-data/alloy/reference/components/otelcol/otelcol.processor.deltatocumulative/ | Docs | undated | Alloy wrapper, experimental |
| https://docs.datadoghq.com/opentelemetry/guide/otlp_delta_temporality/ | Docs | undated | Delta recommended |
| https://docs.honeycomb.io/investigate/query/temporal-aggregation | Docs | undated | Both temporalities, start-time reset |
| https://docs.honeycomb.io/send-data/opentelemetry/ | Docs | undated | Logs endpoint |
| https://docs.newrelic.com/docs/opentelemetry/best-practices/opentelemetry-otlp/ | Docs | undated | Prefer delta |
| https://www.jaegertracing.io/docs/latest/apis/ | Docs | undated | Traces only |
| https://github.com/open-telemetry/opentelemetry-collector-contrib/blob/main/connector/spanmetricsconnector/README.md | Repo | main | spanmetrics options |
| https://github.com/open-telemetry/opentelemetry-collector-contrib/blob/main/connector/countconnector/README.md | Repo | main | count connector |
| https://github.com/open-telemetry/opentelemetry-collector-contrib/blob/main/connector/signaltometricsconnector/README.md | Repo | main | signaltometrics |
| https://github.com/open-telemetry/opentelemetry-collector-contrib/blob/main/processor/deltatocumulativeprocessor/internal/delta/delta.go | Source | main | Drop rules |
| https://github.com/open-telemetry/opentelemetry-collector-contrib/blob/main/processor/deltatocumulativeprocessor/README.md | Repo | main | Alpha, defaults |
| https://docs.dynatrace.com/docs/ingest-from/opentelemetry/getting-started/otlp-export | Docs | undated | Temporality not covered |
| `.agents/research/research_otel_cli_prior_art.md`, `research_otel_codebase_recon.md`, `.agents/discussions/otel-usage-telemetry.md` | Local | 2026-10-22 | Prior findings and requirements |
