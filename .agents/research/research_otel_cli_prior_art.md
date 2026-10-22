# Research: OTEL export from short-lived CLIs — prior art

<!--
Technology Landscape Research
Filename: .agents/research/research_otel_cli_prior_art.md
Owner: Researcher (hex research phase)
Handoff to: Architect (/hex-architect), /hex-plan
Related Skills: hex-architect, hex-plan

Neutral evidence only: the requester asked for findings, negatives and leads with
no recommendation. Sources older than ~18 months (before 2025-04-22) are marked [STALE].
-->

## Metadata

**Date:** 2026-10-22
**Domain:** observability
**Triggered by:** `grim` considering opt-in OpenTelemetry usage export (about one record per invocation, env-var configured, operator-owned collector). Discussion: `.agents/discussions/otel-usage-telemetry.md`. Decisions informed: (1) opentelemetry-rust crates vs a hand-written OTLP/HTTP exporter; (2) what activates export; (3) which signal and attributes.
**Expires:** 2027-04-22 (re-verify; crate versions, vendor receiver support and semconv status move fast)

## Direct Answer

No recommendation (out of scope for this lane). The six questions, answered with evidence below in **Key Findings**:

1. Spec env vars: the OTLP exporter spec is Stable; env support at all is MAY for an SDK (sdk-environment-variables), the OTLP option set is MUST-available. Details in finding 1.
2. OTLP/JSON is Stable since proto v0.20.0 (2023-06-06). Receiver support is uneven: accepted by Collector, Alloy, Jaeger v2, Honeycomb, Grafana Cloud (testing only), Datadog agentless intake; protobuf-only at Dynatrace; unconfirmed or not documented at New Relic, Splunk, Azure Monitor, Datadog Agent, current Elastic.
3. opentelemetry-rust 0.33.1: traces still Beta, metrics and logs Stable; honors the endpoint/headers/timeout/protocol/compression family but, measured, ignores `OTEL_SDK_DISABLED` and `OTEL_TRACES_EXPORTER`. Probe cost with http-proto + reqwest-blocking: +24 crates, +641 KB (+9.7%); grpc/tonic: +36 crates, +1.95 MB (+29.5%); hand-written JSON: +0 crates, +46 KB.
4. Comparable tools split: Claude Code, Gemini CLI, Codex, Terraform, OpenTofu, Docker CLI, Langfuse require a tool-specific or exporter-selector switch; Copilot (per one 2026-10-21 Microsoft page) activates on `OTEL_EXPORTER_OTLP_ENDPOINT` alone; otel-cli goes non-recording when no endpoint is set.
5. Failure latency is bounded by retries and timeouts, not by the exporter being "async": measured 0.8-1.0 s to give up on a refused/503 collector, 5 s (SDK shutdown default) or 10 s (export default) on a hanging one. No tool found that exports from a per-tool-call hook process; per-turn Stop hooks exist.
6. A CLI-spans semconv exists (Development): span name `{process.executable.name}`, required `process.exit.code`, `process.pid`, `process.executable.name`. Spans vs events vs metrics trade-offs in finding 6.

## Technology Landscape

### Trending (gaining momentum)

| Tool/Pattern | Adoption Signal | Key Benefit | Relevance to Grimoire |
|-------------|----------------|-------------|-------------------|
| OTLP export from AI coding agents | Claude Code, Codex, Gemini CLI, Copilot all ship it (docs cited in findings 4) | Operator-owned fleet visibility | grim is invoked by these agents (hooks, MCP) and shares their operator audience |
| CLI-program semconv | `cli-spans` page exists, Development | Standard span shape for short-lived programs | Directly applicable to one-record-per-invocation |

### Established (proven, widely accepted)

| Tool/Pattern | Status | Notes |
|-------------|--------|-------|
| OTLP/HTTP protobuf | Stable, the spec default and the one encoding every surveyed receiver takes | Dynatrace, Splunk, Elastic docs list protobuf only |
| `OTEL_EXPORTER_OTLP_*` env family | Stable spec, honored by most SDKs | Env support is MAY for an SDK |
| Collector-side spanmetrics | alpha connector, widely used | Derives counts and latency from spans |
| Tool-specific opt-out switch for default-on telemetry | dotnet (`DOTNET_CLI_TELEMETRY_OPTOUT`), Homebrew (`HOMEBREW_NO_ANALYTICS`) | First-run notice pattern |

### Emerging (early but promising)

| Tool/Pattern | Signal | Worth Watching Because |
|-------------|--------|----------------------|
| `DO_NOT_TRACK=1` convention | donottrack.sh proposal; Claude Code and (stale) Dagger document it | A single veto variable; contested (see finding 4) |
| OTLP/JSON direct to vendor intake | Honeycomb, Datadog agentless, Grafana Cloud accept it | Makes a no-protobuf hand-written exporter viable |

### Declining (losing mindshare)

| Tool/Pattern | Signal | Avoid Because |
|-------------|--------|---------------|
| consoledonottrack.com as the DNT-env reference | Domain now serves unrelated content (checked 2026-10-22); the original proposal's author called it "a mistake" in 2024 per an HN comment | Dead canonical source |
| Zipkin/Jaeger-native exporters | `OTEL_TRACES_EXPORTER=zipkin` Deprecated in spec; BuildKit docs still show `JAEGER_TRACE` [STALE] | Not an OTLP path |

## Design Patterns Worth Considering

- **Presence of destination only, no switch** (activate when the standard endpoint var is set). Used by: GitHub Copilot per [VS Code guide, 2026-10-21](https://code.visualstudio.com/docs/copilot/guides/monitoring-agents); otel-cli goes non-recording with no endpoint ([repo README](https://github.com/equinix-labs/otel-cli)).
- **Exporter selector as the switch** (`OTEL_TRACES_EXPORTER=otlp`, default off in the tool). Used by: [Terraform](https://github.com/hashicorp/terraform/blob/main/telemetry.go), [OpenTofu](https://opentofu.org/docs/internals/tracing/).
- **Tool-prefixed enable plus standard destination vars**. Used by: [Claude Code](https://code.claude.com/docs/en/monitoring-usage) (`CLAUDE_CODE_ENABLE_TELEMETRY=1` required), [Gemini CLI](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/telemetry/config.ts) (`GEMINI_TELEMETRY_ENABLED`, endpoint falls back to `OTEL_EXPORTER_OTLP_ENDPOINT`).
- **Tool-prefixed destination variable** (standard var not used). Used by: [Docker CLI](https://docs.docker.com/engine/cli/otel/) (`DOCKER_CLI_OTEL_EXPORTER_OTLP_ENDPOINT`).
- **Config-file switch**. Used by: [Codex CLI](https://learn.chatgpt.com/docs/config-file/config-advanced) (`[otel]` in config.toml, off by default).
- **Fail-open, time-capped flush in the hook process**. Used by: [Langfuse Claude Code hook](https://langfuse.com/integrations/developer-tools/claude-code) (flush+shutdown capped at 5 s in a daemon thread), [OpenObserve hook](https://openobserve.ai/docs/integration/ai/claude-code-tracing/) (exits 0 on any failure).
- **Local collector as the buffer** for short-lived callers. Recommended by [otel-cli](https://github.com/equinix-labs/otel-cli): direct-to-SaaS "will slow your code down on the roundtrips".

## Key Findings

### 1. Spec: OTLP exporter and SDK env vars

Sources: [exporter spec](https://opentelemetry.io/docs/specs/otel/protocol/exporter/) (Status Stable, undated, links spec v1.61.0), [source markdown](https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/protocol/exporter.md), [SDK env vars](https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/) (undated).

| Variable | Behavior | Level |
|---|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Base URL. Signal path is appended: `v1/traces`, `v1/metrics`, `v1/logs`. Default `http://localhost:4318` (http) / `:4317` (grpc) | Options "MUST be available"; honor scheme, host, port, path MUST; default http scheme SHOULD |
| `OTEL_EXPORTER_OTLP_{TRACES,METRICS,LOGS}_ENDPOINT` | Used "as-is without any modification"; no path means root `/`; no `/v1/...` appended; takes precedence over the base | MUST |
| `OTEL_EXPORTER_OTLP_HEADERS` (+ per signal) | `key1=value1,key2=value2`, W3C Baggage style, values are strings; semicolon-delimited metadata not supported | MUST available |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | Default 10 s; overall limit per batch export including all retries and waits; timer MUST NOT reset per attempt | MUST available |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc`, `http/protobuf`, `http/json`; default `http/protobuf` (SHOULD unless strong reasons for grpc) | MUST support at least one of grpc or http/protobuf; SHOULD both (if one, http/protobuf); http/json is MAY |
| `OTEL_EXPORTER_OTLP_COMPRESSION` | `gzip` or `none`; no default value (each SDK chooses) | MUST available |
| `..._CERTIFICATE`, `..._CLIENT_KEY`, `..._CLIENT_CERTIFICATE` | Trusted CA file; mTLS key and chain, PEM | MUST available (as options) |
| `OTEL_SDK_DISABLED` | Boolean, default false; `true` gives a no-op SDK for all signals; only case-insensitive "true" counts | Stable; env support itself is MAY |
| `OTEL_TRACES_EXPORTER` | Default `otlp`; values `otlp`, `zipkin` (Deprecated), `console`, `logging` (deprecated), `none`; list allowed (MAY) | Stable |
| `OTEL_SERVICE_NAME` | Sets `service.name`; beats the same key in `OTEL_RESOURCE_ATTRIBUTES` | Stable |
| `OTEL_RESOURCE_ATTRIBUTES` | Comma-separated `key=value` | Stable |

- Retry: transient errors MUST be retried with exponential backoff and jitter. OTLP/HTTP: 429, 502, 503, 504 SHOULD be retried, all other 4xx/5xx MUST NOT; 400 MUST NOT; honor `Retry-After` SHOULD ([OTLP spec 1.11.1](https://opentelemetry.io/docs/specs/otlp/)). `partial_success` responses: client MUST NOT retry.
- Parsing: an empty value MUST be treated as unset; an unrecognized enum value MUST warn and be ignored; a bad number SHOULD warn and be treated as unset. Exporters SHOULD send a `User-Agent`.
- Env-var support is optional: "implementations MAY, but are not required to", and env config MUST have a code equivalent ([sdk-environment-variables](https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/)).

### 2. OTLP/JSON: stability and receiver support

- **Stable.** proto `CHANGELOG` 0.20.0 (2023-06-06): "Declare OTLP/JSON Stable"; the README maturity table has a "JSON Maturity" column, Stable for common/resource/metrics/trace/logs ([README](https://github.com/open-telemetry/opentelemetry-proto/blob/main/README.md), [CHANGELOG](https://github.com/open-telemetry/opentelemetry-proto/blob/main/CHANGELOG.md)). An older vendor claim that JSON is "not yet stable" [STALE] (surfaced in a New Relic page snippet) predates this.
- Wire rules a hand-written encoder must follow: `traceId`/`spanId` hex strings (not base64); enums as integers (names MUST NOT be used); 64-bit ints and `fixed64` timestamps as decimal strings (numbers or strings accepted on decode); lowerCamelCase keys; receivers MUST ignore unknown fields; `Content-Type: application/json` ([OTLP spec 1.11.1](https://opentelemetry.io/docs/specs/otlp/)). proto 1.11.1 (2026-09-25) clarified that JSON timestamps are integers, not RFC 3339 strings.
- Servers SHOULD (not MUST) accept both encodings on one port; servers MUST support `none` and `gzip`. Default path `/v1/traces`, port 4318.

| Receiver | http/json | http/protobuf | gRPC | Source / caveat |
|---|---|---|---|---|
| OTel Collector `otlpreceiver` | Yes ("HTTP/JSON in addition to gRPC") | Yes (spec) | Yes | [README](https://github.com/open-telemetry/opentelemetry-collector/blob/main/receiver/otlpreceiver/README.md) |
| Grafana Alloy `otelcol.receiver.otlp` | Yes (docs show HTTP/JSON) | Not stated on page | Yes | [docs](https://grafana.com/docs/alloy/latest/reference/components/otelcol/otelcol.receiver.otlp/); wrapper of the Collector receiver |
| Grafana Cloud OTLP gateway | Accepted, "ideally only for low-traffic testing"; "should not be sent via JSON in bulk" | Yes, none or gzip | Not stated on this page | [format considerations](https://grafana.com/docs/grafana-cloud/send-data/otlp/otlp-format-considerations/) |
| Datadog Agent OTLP ingest | Not documented (page silent on encoding; says compatible versions are those of the Collector's OTLP receiver) | Not stated | Yes | [docs](https://docs.datadoghq.com/opentelemetry/setup/otlp_ingest_in_the_agent/) |
| Datadog agentless trace intake | Yes: "supports `http/protobuf` and `http/json`. `grpc` is not supported" | Yes | No | [docs](https://docs.datadoghq.com/opentelemetry/setup/agentless/traces); search snippet of an older page said protobuf only, so it changed; Preview/allowlist status per older text |
| Honeycomb | Yes ("gRPC, HTTP/protobuf, and HTTP/JSON") | Yes | Yes | [docs](https://docs.honeycomb.io/send-data/opentelemetry/) |
| New Relic | Not mentioned anywhere on the page | Recommended default | Yes | [docs](https://docs.newrelic.com/docs/opentelemetry/best-practices/opentelemetry-otlp/); `api-key` header; gzip and zstd; 1 MB payload cap |
| Elastic | Serverless limitations page: "Elastic does not yet support JSON Encoding for OTLP/HTTP" [STALE: that page says it is no longer updated]; the current limitations page has no such statement, so current status is unverified | Yes | Yes | [old](https://www.elastic.co/guide/en/serverless/main/observability-apm-agents-opentelemetry-limitations.html), [current](https://www.elastic.co/docs/reference/opentelemetry/compatibility/limitations) |
| Jaeger v2 | Yes: port 4318 `/v1/traces` "OTLP Protobuf or OTLP JSON" | Yes | Yes | [APIs](https://www.jaegertracing.io/docs/latest/apis/) |
| Azure Monitor | Not documented. Native OTLP ingestion is via a Collector (>= 0.132.0) with Microsoft Entra auth, not a static header; metrics need delta temporality | Not stated | Not stated | [docs](https://learn.microsoft.com/en-us/azure/azure-monitor/containers/opentelemetry-protocol-ingestion) (ms.date 2026-05-27) |
| Splunk Observability Cloud | Not listed for OTLP (`/v2/trace/otlp` lists `application/x-protobuf`; JSON listed only for Zipkin); no explicit "rejected" statement | Yes | Yes (gRPC endpoint) | [span formats](https://help.splunk.com/en/splunk-observability-cloud/monitor-application-performance/manage-services-spans-and-traces-in-splunk-apm/compatible-span-formats-for-splunk-apm) |
| Dynatrace | No: "JSON is not supported for Protocol Buffers. Binary format must be used." | Yes (only option) | No ("gRPC is not supported") | [docs](https://docs.dynatrace.com/docs/ingest-from/opentelemetry/getting-started/otlp-export) |

### 3. opentelemetry-rust (as of 2026-10-22)

- **Versions:** `opentelemetry-otlp` 0.33.1 (2026-10-09), 0.33.0 (2026-09-18), 0.32.0 (2026-05-08), 0.31.0 (2025-09-25) per [crates.io](https://crates.io/crates/opentelemetry-otlp); `cargo generate-lockfile` resolved `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-proto`, `opentelemetry-http` all to 0.33.1 (own run).
- **Stability:** Logs API/SDK/OTLP exporter and Metrics API/SDK/OTLP exporter Stable; Traces API, SDK and OTLP exporter **Beta**; Context/Propagators Beta, Baggage RC ([README](https://github.com/open-telemetry/opentelemetry-rust/blob/main/README.md), [opentelemetry.io](https://opentelemetry.io/docs/languages/rust/)). Crates are 0.x: 0.33.0 shipped many breaking changes (sealed config traits, removed public builders, `#[non_exhaustive]` enums, retries on by default) ([CHANGELOG](https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry-otlp/CHANGELOG.md)). MSRV 1.75.
- **Env vars documented as honored** ([docs.rs 0.33.1](https://docs.rs/opentelemetry-otlp/0.33.1/opentelemetry_otlp/)): `ENDPOINT` (+ per signal; with_endpoint and per-signal used verbatim, base gets `/v1/traces`), `PROTOCOL` (+ per signal since 0.32.0), `TIMEOUT` (milliseconds since 0.29.0), `HEADERS` (URL-decoded), `COMPRESSION` (`gzip|zstd|none`, needs `gzip-http`/`zstd-http`), `INSECURE` (gRPC only), `METRICS_TEMPORALITY_PREFERENCE`. `CERTIFICATE`, `CLIENT_KEY`, `CLIENT_CERTIFICATE` are not in that list (TLS is by cargo feature). The [compliance matrix](https://github.com/open-telemetry/opentelemetry-specification/blob/main/spec-compliance-matrix.md) (undated) marks Rust `-` for `OTEL_SDK_DISABLED` and for `OTEL_{TRACES,METRICS,LOGS}_EXPORTER`, `+` for service name and resource attributes.
- **Measured** (own probe, 0.33.1, `http-proto` + `reqwest-blocking-client`, `trace`; see Method notes): `OTEL_SDK_DISABLED=true` still exported; `OTEL_TRACES_EXPORTER=none` still exported; `OTEL_EXPORTER_OTLP_PROTOCOL=http/json` without the `http-json` feature silently sent `application/x-protobuf`; `OTEL_EXPORTER_OTLP_COMPRESSION=gzip` without `gzip-http` sent no `Content-Encoding`, no error; `OTEL_SERVICE_NAME` and `OTEL_RESOURCE_ATTRIBUTES` appeared in the payload resource; `ENDPOINT=http://h/base/` posted to `/base/v1/traces`, `TRACES_ENDPOINT=http://h/custom` posted to `/custom`; header `v%20w` arrived as `v w`; `User-Agent: OTel-OTLP-Exporter-Rust/0.33.1`.
- **Retry default (0.33.0):** retries on for HTTP and gRPC, up to 3, exponential backoff with jitter, HTTP only on 429/502/503/504, honors `Retry-After` on 503; request body capped at 64 MiB. Measured: one 503 export produced 4 POSTs (about +130, +260, +440 ms), 400 produced 1.
- **Weight** (own probe, Method notes): baseline already holding tokio, reqwest 0.13 rustls, serde_json, tracing-subscriber, thin LTO, strip.

| Variant | Crates in `cargo tree -e normal` | Stripped binary | Delta |
|---|---|---|---|
| Baseline | 106 | 6,617,456 B | n/a |
| Hand-written OTLP/JSON POST (reqwest + serde_json, 32 lines) | 106 | 6,664,512 B | +47,056 B (+0.7%), 0 crates |
| opentelemetry-otlp `http-proto` + `reqwest-blocking-client`, trace only, batch exporter | 130 | 7,259,136 B | +641,680 B (+9.7%), +24 crates |
| Same, simple exporter (earlier probe build) | 130 | 7,148,872 B | +531,416 B (+8.0%) |
| opentelemetry-otlp `grpc-tonic` + `tls-aws-lc`, trace only | 142 | 8,567,104 B | +1,949,648 B (+29.5%), +36 crates |

  Of the 24 http crates, at least 11 are absent from grim's `Cargo.lock` by name check (httpdate, ppv-lite86, prost, prost-derive, rand_chacha, zerocopy, plus opentelemetry, opentelemetry_sdk, opentelemetry-proto, opentelemetry-http, opentelemetry-otlp); the rest (anyhow, async-trait, thiserror, itertools, futures-*, either, getrandom, rand/rand_core) are already present. Default features of the crate are `http-proto`, `reqwest-blocking-client`, `trace`, `metrics`, `logs`, `internal-logs` ([Cargo.toml](https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry-otlp/Cargo.toml)).
- **Flush at exit (documented):** `BatchSpanProcessor` "uses a dedicated background thread"; "Shutdown also triggers an export of all buffered spans and is recommended to be called before the application exits"; calling blocking `shutdown()` from the main thread of a tokio current-thread runtime can deadlock; async HTTP clients (`reqwest-client`, `hyper-client`) are not supported by the default processor ([BatchSpanProcessor](https://docs.rs/opentelemetry_sdk/0.33.1/opentelemetry_sdk/trace/struct.BatchSpanProcessor.html)). Dropping the last provider reference triggers shutdown ([SdkTracerProvider](https://docs.rs/opentelemetry_sdk/0.33.1/opentelemetry_sdk/trace/struct.SdkTracerProvider.html)). The spec sets no default Shutdown/ForceFlush timeout; batch `exportTimeoutMillis` default 30000, `scheduledDelayMillis` 5000 ([trace SDK spec](https://opentelemetry.io/docs/specs/otel/trace/sdk/)).

### 4. How comparable tools activate and configure export

| Tool | Default | Activation | Standard `OTEL_*` honored? | Notable deliberate choice |
|---|---|---|---|---|
| Claude Code ([monitoring](https://code.claude.com/docs/en/monitoring-usage), [env-vars](https://code.claude.com/docs/en/env-vars)) | Off | `CLAUDE_CODE_ENABLE_TELEMETRY=1` required, ignored in project/local settings; then `OTEL_{METRICS,LOGS,TRACES}_EXPORTER` | Yes for endpoint/headers/protocol/mTLS; "no default protocol, so set this" | Ignores exporter vars in repo `.claude/settings*.json` ("a repository can't use them to turn telemetry on"); does not pass `OTEL_*` to subprocesses; prompts/tool details redacted by default; flush timeout 5000 ms, shutdown 2000 ms (`CLAUDE_CODE_OTEL_FLUSH_TIMEOUT_MS`, `..._SHUTDOWN_TIMEOUT_MS`); delta temporality default; managed settings can pin destination |
| Codex CLI ([config](https://learn.chatgpt.com/docs/config-file/config-advanced)) | Off | `[otel]` in config.toml, `exporter = none|otlp-http|otlp-grpc` | Not documented on that page | "Exporters batch asynchronously and flush on shutdown"; `log_user_prompt=false`; metrics are separate (`[analytics] enabled=false`). Claims about `trace_exporter`/`metrics_exporter` and endpoint-verbatim come only from third-party guides (unverified) |
| Gemini CLI ([docs](https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/telemetry.md), [source](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/telemetry/config.ts)) | Off | `telemetry.enabled` / `GEMINI_TELEMETRY_ENABLED`; a standard `OTEL_*` var alone does not enable | Endpoint only: `GEMINI_TELEMETRY_OTLP_ENDPOINT` then `OTEL_EXPORTER_OTLP_ENDPOINT`; protocol only via `GEMINI_TELEMETRY_OTLP_PROTOCOL` (grpc/http) | `logPrompts` defaults true per docs |
| GitHub Copilot ([VS Code guide, 2026-10-21](https://code.visualstudio.com/docs/copilot/guides/monitoring-agents), [Coralogix](https://www.coralogix.com/docs/integrations/ai-observability/copilot-cli/), [GitHub docs](https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/configure-copilot-cli)) | Off | VS Code page: activates when any of setting, `COPILOT_OTEL_ENABLED=true`, or `OTEL_EXPORTER_OTLP_ENDPOINT` is set. Coralogix page: "Without [COPILOT_OTEL_ENABLED], Copilot emits no telemetry". **The two conflict.** GitHub's own page only says "OpenTelemetry exports configured by you or your enterprise" | Yes | Default protocol http/protobuf; CLI supports only otlp-http; content capture off by default; the extension forwards the vars to the CLI process |
| Terraform ([telemetry.go](https://github.com/hashicorp/terraform/blob/main/telemetry.go)) | Off | `OTEL_TRACES_EXPORTER=otlp`; any other value discards | Yes (via autoexport) | "we don't assume we're running in an environment with a telemetry collector available"; "experimental ... not a committed external interface"; file shows no flush logic |
| OpenTofu ([tracing](https://opentofu.org/docs/internals/tracing/)) | "completely disabled by default" | `OTEL_TRACES_EXPORTER=otlp` | Yes | Experimental; 100% sampling, no sampling config |
| Docker CLI ([otel](https://docs.docker.com/engine/cli/otel/), Engine 26.1.0, 2024) [STALE] | Off | `DOCKER_CLI_OTEL_EXPORTER_OTLP_ENDPOINT` ("Only if you've set an environment variable") | Page does not say the standard var is honored | Metrics only: `command.time` with `command.name`, `command.status.code`, stdin/stdout/stderr `isatty` |
| BuildKit/buildx ([debug docs](https://docs.docker.com/build/debug/opentelemetry/)) [STALE] | Off | Documented path is `JAEGER_TRACE` via builder `driver-opt env.` | Official page silent; third-party blogs claim `OTEL_EXPORTER_OTLP_ENDPOINT` (unverified) | n/a |
| Dagger ([0.2 archive](https://archive.docs.dagger.io/0.2/1218/cli-telemetry)) [STALE] | On | Opt-out `DO_NOT_TRACK=1` | Not covered | Current docs page not retrievable; telemetry (version, platform, command, anonymous device ID) |
| dotnet CLI ([telemetry](https://learn.microsoft.com/en-us/dotnet/core/tools/telemetry), ms.date 2026-10-01) | On (Microsoft SDK builds) | Opt-out `DOTNET_CLI_TELEMETRY_OPTOUT=1|true`; first-run notice on stdout/stderr | OTel and `DO_NOT_TRACK` not mentioned | Detects calling LLM agent by env var (`CLAUDECODE`, `GITHUB_COPILOT_CLI_MODE`, `CURSOR_EDITOR`, `GEMINI_CLI`); detects CI by env; hashes values |
| Homebrew ([analytics](https://docs.brew.sh/Analytics)) | Notice shown "before analytics are enabled" | Opt-out `HOMEBREW_NO_ANALYTICS=1`, `brew analytics off` | `DO_NOT_TRACK` not mentioned | Sent "over HTTPS in a detached background process" to InfluxDB; no user id or IP field; 365-day retention |
| otel-cli ([README](https://github.com/equinix-labs/otel-cli)) | Non-recording | No endpoint: "will run in non-recording mode and not attempt to contact any servers"; endpoint set: records | Yes (`OTEL_EXPORTER_OTLP_*` incl. CERTIFICATE/CLIENT_*) | Timeout example `1s`; "highly recommended to use a localhost opentelemetry collector that can buffer spans" |
| cargo, uv, npm | n/a | No telemetry/OTel documentation found ([uv policies](https://docs.astral.sh/uv/reference/policies/) is silent) | n/a | Absence of docs, not proof of absence |

**Evidence for activating on standard `OTEL_*` presence alone**
- Copilot's 2026-10-21 page lists `OTEL_EXPORTER_OTLP_ENDPOINT` as a sufficient trigger; otel-cli is presence-driven.
- The SDK spec treats the env family as the standard operator interface and `OTEL_TRACES_EXPORTER` defaults to `otlp` once an SDK is configured ([env vars](https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/)).
- Gemini CLI reads the standard endpoint var as a fallback destination.

**Evidence against**
- Claude Code, Gemini CLI, Codex, Terraform, OpenTofu, Docker CLI and the Langfuse hook (`TRACE_TO_LANGFUSE=true` exact string, [docs](https://langfuse.com/integrations/developer-tools/claude-code)) all gate on an explicit tool-side switch.
- Claude Code additionally refuses repo-resident config and does not forward `OTEL_*` to child processes; that is a deliberate choice about who may turn export on.
- Standard vars are shared across tools: the shell of this research session already carried `OTEL_EXPORTER_OTLP_ENDPOINT` and `OTEL_EXPORTER_OTLP_HEADERS` (names only recorded here, origin not located). In the probe the Rust SDK attached an ambient `Authorization` header from `OTEL_EXPORTER_OTLP_HEADERS` to a request sent to a different, locally chosen endpoint. A presence-only trigger would therefore export in that environment, with those credentials.
- The Rust SDK does not honor `OTEL_SDK_DISABLED` or `OTEL_TRACES_EXPORTER=none` (finding 3), so a veto via those vars must be implemented by the tool.

**DO_NOT_TRACK**
- Proposal at [donottrack.sh](https://donottrack.sh/) (undated): `DO_NOT_TRACK=1` means disable "ad tracking, usage reporting, telemetry, crash reporting, and non-essential requests"; only value defined is `1`; it lists no supporting tools, only nine tools with their own opt-outs (.NET, AWS SAM CLI, Azure CLI, Gatsby, Go, Google Cloud SDK, Homebrew, Netlify CLI, Syncthing).
- Verified adopters in this research: Claude Code (`DO_NOT_TRACK=1` "with the same effect as `DISABLE_TELEMETRY`", "honors it as the cross-tool convention", `0` leaves telemetry on; [env-vars](https://code.claude.com/docs/en/env-vars)); Dagger [STALE]. dotnet and Homebrew docs do not mention it. Whether it also vetoes an operator-configured OTLP export in Claude Code is not stated.
- Contested: an [HN thread](https://hn.nuxt.dev/item/47988592) (mirror, relative date "172 days ago") objects that the name implies tracking is the default, notes it is unenforceable, and quotes the original Console Do Not Track author calling it "a mistake" (2024). The original site consoledonottrack.com served unrelated content when fetched on 2026-10-22.

### 5. Short-lived-process export: latency, failure, hooks

- **Measured** (own probe, loopback fake collectors, WSL2, n=1 each, opentelemetry 0.33.1, default retry on). "Export" is the in-line cost of the simple exporter at span end; "shutdown" is the cost at exit with the batch exporter.

| Scenario | Simple exporter, span end | Batch exporter, `shutdown()` |
|---|---|---|
| Collector OK (200) | 3.4 ms | 0.7 ms |
| Connection refused | 863 ms | 819 ms |
| HTTP 503 (4 POSTs) | 901 ms | 965 ms |
| HTTP 400 (no retry) | 3.4 ms | 0.7 ms |
| DNS failure | n/a | 856 ms |
| Accepts, never answers | 10.0 s (default export timeout) | 5.0 s, `Err(Timeout(5s))` (default `shutdown()` timeout); with `OTEL_EXPORTER_OTLP_TIMEOUT=1000`: 1.0 s; with `shutdown_with_timeout(500ms)`: 0.5 s |
| Unroutable IP (10.255.255.1) | n/a | 5.0 s, `Err(Timeout(5s))` |

  Process wall time for the hang case was 5.005 s total; the process exited after the timeout.
- **Documented timeouts in the wild:** Claude Code flush 5000 ms, shutdown 2000 ms, metrics interval 60 s, logs 5 s ([env-vars](https://code.claude.com/docs/en/env-vars), [monitoring](https://code.claude.com/docs/en/monitoring-usage)); Langfuse hook flush+shutdown capped at 5 s in a daemon thread so "a slow/unreachable Langfuse can't stall Claude Code" ([docs](https://langfuse.com/integrations/developer-tools/claude-code)); otel-cli timeout example 1 s; OTLP spec default 10 s per batch ([exporter spec](https://opentelemetry.io/docs/specs/otel/protocol/exporter/)); Homebrew uses a detached background process ([analytics](https://docs.brew.sh/Analytics)).
- **Hook processes:** no tool found that exports OTLP from its own per-tool-call hook process. Found: Claude Code emits `hook` spans from the parent process ([monitoring](https://code.claude.com/docs/en/monitoring-usage)); Langfuse and OpenObserve export per turn from a Stop hook script (Python, SDK `flush` then `shutdown`, fail-open exit 0, file lock with a 2 s best-effort timeout) ([OpenObserve, updated 2026-10-09](https://openobserve.ai/docs/integration/ai/claude-code-tracing/), [Langfuse](https://langfuse.com/integrations/developer-tools/claude-code)).
- Spec note: `ForceFlush` "SHOULD only be called in cases where it is absolutely necessary", e.g. FaaS that may suspend the process ([trace SDK](https://opentelemetry.io/docs/specs/otel/trace/sdk/)).

### 6. Semantic conventions and signal choice

- **CLI semconv exists, Development** ([cli-spans](https://opentelemetry.io/docs/specs/semconv/cli/cli-spans/)): scope "short-lived programs that end their execution"; span name `{process.executable.name}` (a documented low-cardinality alternative is allowed); kind `INTERNAL` (callee) or `CLIENT` (caller); a non-zero `process.exit.code` is an error and then `error.type` is required (and `error.type` SHOULD NOT be set on success); context propagation for CLI programs is a TODO.
- Attributes: `process.executable.name`, `process.exit.code`, `process.pid` Required (all Release Candidate); `error.type` Conditionally Required (Stable); `process.command_args` and `process.executable.path` Recommended. All `process.*` attributes are RC in the [registry](https://opentelemetry.io/docs/specs/semconv/registry/attributes/process/); `process.command_args` and `process.command_line` "SHOULD NOT be collected by default unless there is sanitization that excludes sensitive data". No `cli.*` namespace appears on the CLI page.
- **Span:** matches "operations that have a duration and meaningful boundary" ([events semconv](https://opentelemetry.io/docs/specs/semconv/general/events/)); Rust traces SDK/exporter is Beta; Collector spanmetrics can derive metrics from spans.
- **Log record / event:** an Event is a LogRecord with an event name; semconv for events is Development; the log data model is Stable ([data model](https://opentelemetry.io/docs/specs/otel/logs/data-model/)); event names "MUST NOT include dynamic values"; use an event "instead of a span attribute when the data describes a distinct occurrence within the operation"; Rust logs and OTLP logs exporter are Stable. Copilot CLI is reported to emit span events rather than standalone log records ([Coralogix](https://www.coralogix.com/docs/integrations/ai-observability/copilot-cli/), secondary).
- **Metric:** Docker CLI exports one histogram-style metric `command.time` with exit code and TTY attributes ([docs](https://docs.docker.com/engine/cli/otel/)); Claude Code defaults to delta temporality and Azure Monitor requires delta ([Claude monitoring](https://code.claude.com/docs/en/monitoring-usage), [Azure](https://learn.microsoft.com/en-us/azure/azure-monitor/containers/opentelemetry-protocol-ingestion)). Rust metrics SDK is Stable. Inference (not sourced): per-process counters reset each run, so cross-invocation aggregation depends on delta temporality and a collector.
- **Collector-side derivation:** the [spanmetrics connector](https://github.com/open-telemetry/opentelemetry-collector-contrib/blob/main/connector/spanmetricsconnector/README.md) (alpha) turns spans into a calls counter and a duration histogram, default dimensions `service.name`, `span.name`, `span.kind`, `status.code`, extra dimensions from span or resource attributes via `dimensions`, default cumulative temporality (delta available), default cardinality limit 0 (unlimited), overflow series tagged `otel.metric.overflow="true"`.
- Cardinality reference points: Claude Code ships toggles because "lower cardinality generally means better performance and lower storage costs" (`OTEL_METRICS_INCLUDE_SESSION_ID` default true, `..._VERSION` false, `..._ACCOUNT_UUID` true) ([monitoring](https://code.claude.com/docs/en/monitoring-usage)).

## Method notes (own measurements)

- Probe crates under `/home/mherwig/.cache/otel-probe/` (deleted after this run). Baseline deps: `tokio` full, `reqwest 0.13` `default-features=false, features=["rustls"]`, `serde_json`, `serde`, `tracing`, `tracing-subscriber` (env-filter, std, fmt, json, registry) — chosen to mirror grim's `Cargo.toml`. Profile: `opt-level=3`, `lto="thin"`, `strip=true`. Sizes are single builds on Rust 1.95 x86_64 Linux; crate counts from `cargo tree -e normal --prefix none` unique `name version` lines (root included).
- Latency probe: one span named `grim.invocation`, `with_http()`, `SdkTracerProvider` with `with_simple_exporter` or `with_batch_exporter`, then `shutdown()`. Fake collectors were Python sockets on 127.0.0.1 answering 200, 503, 400 or never. Wall times include the exporter's default retry policy. WSL2, one run per cell; treat as order-of-magnitude.
- Hand-written probe: 32 lines building the OTLP/JSON body with `serde_json::json!`, `reqwest` POST, 2 s timeout. **Not validated against a live receiver.**
- The ambient `OTEL_EXPORTER_OTLP_HEADERS` value was sent only to loopback fake servers; no value is recorded in this file; later runs unset it.

## Recommendation

None. The requester asked for evidence only (findings, negatives, leads); the decisions are for `/hex-architect`.

## Negative results

- No tool found that exports OTLP from its own per-tool-call hook process (only per-turn Stop hooks and parent-process `hook` spans).
- OTLP/JSON acceptance is not documented for the Datadog Agent receiver, New Relic, Azure Monitor, or Splunk (implicit no only), and is unverified for current Elastic.
- No official GitHub documentation of Copilot CLI OTel activation; the two available sources conflict on whether the standard endpoint var suffices.
- `OTEL_EXPORTER_OTLP_{CERTIFICATE,CLIENT_KEY,CLIENT_CERTIFICATE}` are not listed as honored by opentelemetry-otlp 0.33.1; `OTEL_SDK_DISABLED` and `OTEL_TRACES_EXPORTER` measured as ignored.
- Codex: official page does not document `OTEL_*` handling, `trace_exporter` or endpoint-verbatim behavior; Codex source not read.
- BuildKit/buildx: official activation by standard `OTEL_*` not confirmed (only `JAEGER_TRACE`, stale page). Dagger: current docs page not retrievable, `DO_NOT_TRACK` evidence is the 0.2 archive.
- cargo, uv, npm: no telemetry or OTel documentation found; no source for Terraform flush-at-exit behavior.
- Homebrew and dotnet docs do not mention `DO_NOT_TRACK`; donottrack.sh lists no adopters; consoledonottrack.com is no longer a usable source.
- No published WAN latency numbers for OTLP export from CLIs; only the loopback measurements above.
- Claude Code docs do not say whether `DO_NOT_TRACK` / `DISABLE_TELEMETRY` suppress an operator-configured OTLP export.

## Leads

- **security-threat-model:** ambient shared `OTEL_EXPORTER_OTLP_HEADERS` credentials, endpoint reuse and `process.command_args` sanitization need review against `arch-threat-model.md` and the hook-consent precedent (env-unreachable switches).
- **receiver-interop-test:** live-test a hand-written OTLP/JSON span (hex IDs, int64 as string, integer enums) against Collector, Alloy, Jaeger v2, Honeycomb, Grafana Cloud and Datadog agentless to confirm the unvalidated payload.
- **hook-latency-measurement:** measure grim's hook dispatcher cold-start plus export cost per tool call (reqwest/TLS init, 0.8 s refused-collector worst case, 5 s hang case) against the hook budget.
- **attribute-set-design:** choose attributes against `process.*` (RC) and CLI-spans (Development), including cardinality limits and what a hook or MCP caller can add (client name; dotnet's agent-detection env vars are a precedent).
- **mcp-long-running-mode:** the MCP server needs periodic export (batch thread, tokio deadlock warning for blocking `shutdown()`), a different shape from the one-record CLI path.
- **stability-freeze-contract:** env var names and attributes become additive-only public contract (Principle 9); check `docs/src/content/docs/stability.md` before naming.
- **windows-verification:** the blocking reqwest client thread and timeout behavior should be run through the Windows local harness before any choice is final.

## Sources

| Source | Type | Date | Relevance |
|--------|------|------|-----------|
| https://opentelemetry.io/docs/specs/otel/protocol/exporter/ | Spec | undated (spec v1.61.0 links) | Env vars, MUST/SHOULD/MAY, retry, timeout |
| https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/ | Spec | undated | `OTEL_SDK_DISABLED`, exporter selectors, parsing rules |
| https://opentelemetry.io/docs/specs/otlp/ | Spec | OTLP 1.11.1 | HTTP/JSON rules, retry codes |
| https://github.com/open-telemetry/opentelemetry-proto/blob/main/CHANGELOG.md | Repo | 1.11.1 = 2026-09-25 | JSON Stable at 0.20.0 (2023-06-06) |
| https://github.com/open-telemetry/opentelemetry-proto/blob/main/README.md | Repo | undated | JSON maturity column |
| https://github.com/open-telemetry/opentelemetry-collector/blob/main/receiver/otlpreceiver/README.md | Repo | undated | Collector accepts HTTP/JSON |
| https://grafana.com/docs/grafana-cloud/send-data/otlp/otlp-format-considerations/ | Docs | undated | Grafana Cloud JSON caveat |
| https://docs.honeycomb.io/send-data/opentelemetry/ | Docs | undated | Honeycomb protocols |
| https://docs.datadoghq.com/opentelemetry/setup/agentless/traces | Docs | undated | Datadog intake encodings |
| https://docs.dynatrace.com/docs/ingest-from/opentelemetry/getting-started/otlp-export | Docs | undated | Protobuf only |
| https://www.jaegertracing.io/docs/latest/apis/ | Docs | undated | Jaeger OTLP JSON |
| https://learn.microsoft.com/en-us/azure/azure-monitor/containers/opentelemetry-protocol-ingestion | Docs | 2026-05-27 | Azure Entra auth, Collector path |
| https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry-otlp/CHANGELOG.md | Repo | 0.33.1 = 2026-10-08 | Breaking changes, retry default, env handling |
| https://docs.rs/opentelemetry-otlp/0.33.1/opentelemetry_otlp/ | Docs | 0.33.1 | Honored env vars, features |
| https://github.com/open-telemetry/opentelemetry-rust/blob/main/README.md | Repo | undated | Per-signal stability |
| https://crates.io/crates/opentelemetry-otlp | Registry | updated 2026-10-09 | Versions and dates |
| https://docs.rs/opentelemetry_sdk/0.33.1/opentelemetry_sdk/trace/struct.BatchSpanProcessor.html | Docs | 0.33.1 | Flush at exit, deadlock note |
| https://github.com/open-telemetry/opentelemetry-specification/blob/main/spec-compliance-matrix.md | Spec | undated | Rust env var support (may lag) |
| https://code.claude.com/docs/en/monitoring-usage | Docs | undated (mentions v2.1.269) | Claude Code activation, redaction |
| https://code.claude.com/docs/en/env-vars | Docs | undated | Flush/shutdown timeouts, DO_NOT_TRACK |
| https://learn.chatgpt.com/docs/config-file/config-advanced | Docs | undated | Codex `[otel]` |
| https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/telemetry/config.ts | Source | undated | Gemini env resolution |
| https://code.visualstudio.com/docs/copilot/guides/monitoring-agents | Docs | 2026-10-21 | Copilot activation conditions |
| https://github.com/hashicorp/terraform/blob/main/telemetry.go | Source | undated | Terraform opt-in |
| https://opentofu.org/docs/internals/tracing/ | Docs | undated | OpenTofu opt-in |
| https://docs.docker.com/engine/cli/otel/ | Docs | Engine 26.1.0 (2024) [STALE] | Docker CLI metrics |
| https://archive.docs.dagger.io/0.2/1218/cli-telemetry | Docs | Dagger 0.2 [STALE] | DO_NOT_TRACK adoption |
| https://learn.microsoft.com/en-us/dotnet/core/tools/telemetry | Docs | 2026-10-01 | dotnet telemetry |
| https://docs.brew.sh/Analytics | Docs | undated | Homebrew analytics |
| https://donottrack.sh/ | Site | undated | DO_NOT_TRACK definition |
| https://hn.nuxt.dev/item/47988592 | Forum mirror | relative "172 days ago" | Objections, adopters named |
| https://github.com/equinix-labs/otel-cli | Repo | undated | Short-lived export pattern |
| https://langfuse.com/integrations/developer-tools/claude-code | Docs | undated | Stop-hook export, 5 s cap |
| https://openobserve.ai/docs/integration/ai/claude-code-tracing/ | Docs | 2026-10-09 | Stop-hook export |
| https://opentelemetry.io/docs/specs/semconv/cli/cli-spans/ | Spec | Development, undated | CLI span conventions |
| https://opentelemetry.io/docs/specs/semconv/registry/attributes/process/ | Spec | RC, undated | `process.*` stability, sensitivity |
| https://opentelemetry.io/docs/specs/semconv/general/events/ | Spec | undated (spec v1.59.0 links) | Spans vs events |
| https://github.com/open-telemetry/opentelemetry-collector-contrib/blob/main/connector/spanmetricsconnector/README.md | Repo | undated | Collector-side derivation |
