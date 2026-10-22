# ADR: OTel usage telemetry through `opentelemetry-otlp` (OBS-12 exemption)

## Metadata

**Status:** Accepted
**Date:** 2026-10-22
**Deciders:** Michael Herwig (maintainer; exporter choice ratified in `.agents/discussions/otel-usage-telemetry.md`), Claude (hex-plan, `hex/otel-usage-telemetry`)
**Beads Issue:** N/A — tracked as [grimoire-rs/grimoire#206](https://github.com/grimoire-rs/grimoire/issues/206), overlaps [grimoire-rs/grimoire#83](https://github.com/grimoire-rs/grimoire/issues/83)
**Related PRD:** N/A — input dossier `.agents/discussions/otel-usage-telemetry.md` (ratified 2026-10-22, handed off to a goal loop)
**Tech Strategy Alignment:**
- [ ] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md`
- [x] OR deviation justified in Rationale section (the golden path names no telemetry stack; the deviation is from the vendored rule OBS-12, justified below)
**Domain Tags:** integration, security, devops
**Supersedes:** N/A
**Superseded By:** N/A

## Context

The owner asked for "primitive OTEL support, to observe usage", with the
endpoint configured entirely through environment variables. The ratified
discussion fixes the shape: opt-in through `GRIM_TELEMETRY`, configured by
the standard `OTEL_*` variables, OTel **events** only (the logs signal),
one `grim.command` event per CLI invocation plus one `grim.artifact.changed`
event per changed artifact, all in one OTLP/HTTP request. No default
collector exists; grim sends nothing unless an operator arms it.

The vendored rule **OBS-12** (`.claude/rules/rust-quality/docs-and-tracing.md`)
says: no `opentelemetry*` dependency in a short-lived CLI; admissible only in
a daemon mode, pinned, with the justification written down. The rule is
vendored from `ocx-sh/grimoire-lore` and cannot be edited here
(`.claude/rules.md` › Vendored rules). Its provenance is a documentation-derived
default, never measured (`research_otel_repo_archaeology.md`). grim is a
short-lived CLI, so taking the crate needs this written exemption.

## Decision Drivers

- **Backend reach.** Dynatrace accepts only protobuf; New Relic, Splunk and
  Azure do not document OTLP/JSON. An http/json-only exporter forces a
  Collector hop on those operators (`research_otel_cli_prior_art.md`).
- **Owner preference** for the maintained crate over a hand-written encoder.
- **Exit latency.** A CLI exit must not stall on a dead collector; the hook
  path must not pay anything at all.
- **Credential hygiene.** `OTEL_EXPORTER_OTLP_HEADERS` is commonly a bearer
  token and is ambient in developer shells (this session's shell carries one
  for another tool).
- **Size and crate count.** The binary is 29.5 MB and its size is the named
  cold-start cost (`src/main.rs:263-272`).

## Industry Context & Research

**Research artifacts:** `.agents/research/research_otel_cli_prior_art.md`,
`research_otel_signals_events_metrics.md`, `research_otel_repo_archaeology.md`,
`research_otel_codebase_recon.md`, plus the API probe recorded in this ADR's
§ Technical Details (run 2026-10-22 against opentelemetry-rust 0.33.1).
**Trending approaches:** four of five agent CLIs use log-record events as
their primary signal (Claude Code, Codex, Gemini CLI); all gate export on a
tool-specific switch rather than on the presence of `OTEL_*`.
**Key insight:** the crate is usable from a one-shot CLI only if grim drives
the exporter itself. Its built-in processors either deadlock on a
current-thread runtime, send one POST per record, or panic without a reactor,
and it merges env headers into every request with no way to switch that off.

## Considered Options

### Option 1: Hand-written OTLP/HTTP+JSON over the existing `reqwest` + `serde_json`

| Pros | Cons |
|---|---|
| 0 new crates, about +47 KB; OBS-12 untouched | http/json only: Dynatrace (protobuf-only) and backends without documented JSON need a Collector hop |
| Full control of every header and byte | Spec surface (encoding, partial success) maintained by hand |

### Option 2: `opentelemetry-otlp` 0.33.1, `http-proto`, grim drives the exporter (chosen)

| Pros | Cons |
|---|---|
| Protobuf to every backend; maintained encoder | +24 crates, about +731 KB for the logs signal (`http-proto` also force-enables the trace and metrics code) |
| Stable Logs API/SDK/exporter in 0.33.1 | Logs API is a "bridge", documented as not meant for direct application calls |
| Same `reqwest` 0.13.5 as grim, no second TLS stack | Env headers, retries, processors and `OTEL_SDK_DISABLED` all need explicit counter-measures (below) |

### Option 3: `opentelemetry-otlp` with its built-in processors and `reqwest-client`

| Pros | Cons |
|---|---|
| Least glue code | Measured: `SimpleLogProcessor` deadlocks on a current-thread runtime and sends one POST per record; `BatchLogProcessor` panics with an async client; default retries (3) and a 10 s timeout stall exit by 5–15 s on a dead collector |

### Option 4: `tracing-opentelemetry` / `opentelemetry-appender-tracing` bridge

| Pros | Cons |
|---|---|
| Reuses the existing `tracing` subscriber | Forwards all 353 `tracing` event sites unless filtered; record shape follows log lines, not a frozen field table; same processor problems as Option 3 |

## Decision Outcome

**Chosen Option:** Option 2 — the owner's pick against the recommendation to
hand-write (discussion › Decisions › Exporter).

**Rationale:** protobuf reaches every backend without a Collector hop, and the
crate's measured defects are all avoidable by not using its processors and
transport. OBS-12's concern — provider setup and flush-on-shutdown cost in a
process that lives for seconds — is answered by construction: no provider
runs a background worker, nothing is flushed on shutdown, and the whole
export is one request bounded by a 500 ms default deadline that runs only
when an operator armed it. An unarmed run builds no provider, no exporter and
no client.

### OBS-12 exemption — the conditions it is granted on

The exemption holds only while every condition below holds. Conditions 1–7
are pinned by tests named in `.agents/plans/plan_otel_usage_telemetry.md`
(C-023 for 1, the multi-record single-POST test for 3, the redirect test for
5); condition 8 is a recorded measurement.

1. **Exact pins.** `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp`
   and `opentelemetry-http` are pinned `=0.33.1`, all `default-features = false`.
   Features: `logs` on the first two; `http-proto` + `logs` on the exporter;
   none on `opentelemetry-http` (only its `HttpClient` trait is used). No
   `reqwest-client`, `reqwest-blocking-client`, `grpc-tonic` or
   `internal-logs`. A bump is a deliberate edit with this ADR's changelog row.
2. **Gate before construction.** grim reads `GRIM_TELEMETRY`,
   `OTEL_SDK_DISABLED`, `DO_NOT_TRACK` and offline mode itself, before any
   OTel type is built. The crate ignores `OTEL_SDK_DISABLED`.
3. **No crate processor, no crate transport.** Records are minted through an
   `SdkLoggerProvider` whose only processor collects them in memory; grim then
   calls `LogExporter::export` once with one `LogBatch`, under
   `tokio::time::timeout`. The exporter receives grim's own `HttpClient`
   wrapper around a `reqwest::Client` built through `crate::tls`.
4. **Headers are grim's.** The wrapper drops every request header the crate
   set except `content-type` and inserts only `user-agent: grim/<version>`
   plus the headers grim selected; only transport headers reqwest adds
   (`host`, `content-length`, `accept`) join them. Generic
   `OTEL_EXPORTER_OTLP_HEADERS` go only to the base
   `OTEL_EXPORTER_OTLP_ENDPOINT`; a signal-specific endpoint gets only
   `OTEL_EXPORTER_OTLP_LOGS_HEADERS`; with no endpoint variable nothing is
   sent (no default endpoint). The crate merges `OTEL_EXPORTER_OTLP_[LOGS_]HEADERS` on top of
   explicit headers unconditionally, `opentelemetry-otlp-0.33.1
   src/exporter/http/mod.rs:293-298`; measured leaking a bearer token).
5. **No retries, bounded time.** `RetryPolicy::disabled()`; the reqwest client
   carries connect and total timeouts equal to the export timeout (default
   500 ms); redirects are off.
6. **No detectors.** The resource is built from `Resource::builder_empty()`;
   `OTEL_RESOURCE_ATTRIBUTES` is parsed by grim (percent-decoded, which the
   SDK's private parser does not do). No `telemetry.sdk.*`, no host, no argv.
7. **Hook path untouched.** Nothing telemetry-related is constructed before
   the `Hook(Run)` early return in `app::run`; `grim hook run` never exports.
8. **Size recorded.** The `dist` and `release` binary sizes before and after
   are recorded in § Quantified Impact before this branch lands.

If a later change needs the crate's processors, a background worker, traces
or metrics, this exemption does not cover it; write a new ADR.

### Quantified Impact

| Metric | Before | After | Notes |
|---|---|---|---|
| New crates | 0 | +24 (stable delta) | `prost` 0.14, `opentelemetry*` 0.33.1, `futures-*`, `httpdate`; no `tonic`, no `h2` |
| Shipped binary (`--profile dist`, thin LTO, stripped) | 25,333,416 B | 25,749,400 B | +415,984 B (+1.6 %); merge base `7b21abce` vs branch, linux x86_64, 2026-10-22 |
| Release binary (`--profile release`) | 30,882,208 B | 31,537,264 B | +655,056 B (+2.1 %); research estimate was +731 KB |
| Unarmed run cost | 0 | 4 env reads | no allocation of any OTel type |
| Armed, collector up | — | about 20 ms | probe, debug build, loopback |
| Armed, collector refused | — | about 18 ms | probe |
| Armed, collector hangs | — | ≤ timeout (500 ms default) + ε | probe: 520 ms; with `GRIM_TELEMETRY_REPOSITORY` add ≤ 2 × 1 s for the bounded `git` calls; a hanging DNS resolver is not bounded (documented residual) |

### Consequences

**Positive:**
- Protobuf export to any OTLP backend, configured with the variables
  operators already know.
- The exemption's conditions turn the crate's three measured hazards (header
  merge, retries, processors) into pinned, tested invariants.

**Negative:**
- +24 crates and a binary-size increase for a feature most users never arm.
- grim calls an API the crate documents as a bridge, so a minor bump may
  break the glue; the exact pins make that a visible edit, not a drift.
- A repository-supplied environment (`.envrc`, `.mise.toml`, devcontainer
  `containerEnv`, CI `variables:`) can set `GRIM_TELEMETRY` and an endpoint
  together and collect the command names and artifact refs of runs in that
  environment (threat-model **T3**). The owner accepted this
  (discussion › Threads › Payload). The same environment can also point
  the base `OTEL_EXPORTER_OTLP_ENDPOINT` at itself and so receive the
  user's ambient `OTEL_EXPORTER_OTLP_HEADERS` — in cleartext over `http://`.
  Every mechanism that injects repo env (`.envrc` shell code, trusted `mise`
  config, devcontainer, CI) already gives that repo code execution or
  ownership of the environment, and every OTel SDK on the machine reads the
  same variables the same way, so v1 adds no gate beyond `GRIM_TELEMETRY`.
  Header scoping (condition 4) keeps a signal-specific endpoint from
  receiving generic headers. An egress ruling in `arch-threat-model.md` is
  owed and deferred to the owner (plan › Decisions recorded › 19).

**Risks:**
- Upstream breaking change → exact pins, one glue module.
- Header-strip wrapper regresses → acceptance test asserts a header set only
  in env never reaches a signal-specific endpoint.
- Exit stall → acceptance tests for refused, 503 and hanging collectors bound
  added wall time by the timeout.

## Technical Details

### Architecture

```
app::run ─ Hook(Run)? ── return (no telemetry)
   │
   ├─ Context::new ── Telemetry::from_env()      (gate: 4 env reads, no I/O)
   ├─ ctx.telemetry().start(command name)
   ├─ command arm … records scope / clients / artifact changes into the handle
   └─ ctx.telemetry().finish(&outcome).await     (armed only, in this order:
          installation id → vcs (repo opt-in, project scope)
          → OTEL_RESOURCE_ATTRIBUTES / OTEL_SERVICE_NAME → payload
          → debug log of JSON → endpoint/headers/timeout/protocol (skip → debug)
          → SdkLoggerProvider(collect-only) → LogExporter::export(one batch)
          under timeout → errors to debug, never to the exit code)
```

### Exporter glue (verified by probe, 0.33.1)

```rust
let exporter = opentelemetry_otlp::LogExporter::builder()
    .with_http()
    .with_endpoint(full_logs_url)            // used verbatim; grim appends /v1/logs to a base URL
    .with_protocol(Protocol::HttpBinary)
    .with_timeout(timeout)
    .with_retry_policy(RetryPolicy::disabled())
    .with_http_client(GrimHttpClient { inner, headers }) // strips crate/env headers
    .build()?;
exporter.set_resource(&resource);            // Resource::builder_empty()
tokio::time::timeout(timeout, exporter.export(LogBatch::new(&records))).await
```

### Related decisions (ratified in the discussion, not re-decided here)

Gate `GRIM_TELEMETRY` + standard `OTEL_*`; CLI commands only (hook run never,
`grim mcp` one session record); events only, metrics derived collector-side;
random installation ID under `$GRIM_HOME/telemetry/`; repository context only
with `GRIM_TELEMETRY_REPOSITORY`; the whole surface Unstable until 1.0. The
field table and env semantics are the plan's contracts
(`.agents/plans/plan_otel_usage_telemetry.md` › Component contracts).

## Validation

- [ ] Exemption conditions 1–7 pinned by named tests (plan › C-023 and contracts)
- [ ] Binary size before/after recorded above (condition 8)
- [ ] `task rust:license:deps` and `task rust:audit` pass on the new graph
- [ ] Security review of the header and URL paths (hex-execute review panel)

## Links

- [Discussion](../discussions/otel-usage-telemetry.md)
- [Plan](../plans/plan_otel_usage_telemetry.md)
- [adr_vendor_support_tiers.md](./adr_vendor_support_tiers.md) — re-check the Tier 1 set once usage telemetry lands
- OBS-12, OBS-20, OBS-22 in `.claude/rules/rust-quality/docs-and-tracing.md`

---

## Changelog

| Date | Author | Change |
|---|---|---|
| 2026-10-22 | hex-plan | Initial record of the owner's exporter decision and the conditions of the OBS-12 exemption |
| 2026-10-22 | hex-plan adversary (native substitute; codex usage-limited) | Endpoint userinfo refused; `finish` isolated in a spawned task; git env scrubbed; credential exposure of the base endpoint written into § Consequences |
| 2026-10-22 | hex-plan review | Spec review: condition 4 states the exact header set and "no default endpoint"; conditions mapped to tests; `finish` order in the diagram; size measured on `dist` and `release` |
