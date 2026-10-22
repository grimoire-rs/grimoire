# Discussion: Primitive OTEL usage telemetry

State: handed-off → loop · Updated: 2026-10-22
Ratified: 2026-10-22 → loop
Confidence: ratified by the owner (explicit "drain it"); decisions backed by 8 research artifacts dated 2026-10-22 (`.agents/research/research_otel_*.md`), 4 of them adversarially fact-checked
Tracking issue: [grimoire-rs/grimoire#206](https://github.com/grimoire-rs/grimoire/issues/206)

## Intent

Owner's ask: "primitive OTEL support, to observe usage. The OTEL endpoint
should be fully configurable via env vars." Tracked as
[grimoire-rs/grimoire#206](https://github.com/grimoire-rs/grimoire/issues/206);
overlaps the older, broader
[grimoire-rs/grimoire#83](https://github.com/grimoire-rs/grimoire/issues/83)
(opt-in usage + error telemetry, which assumed a maintainer-hosted default
collector). This discussion's shape has **no default endpoint**: grim sends
nothing unless an operator arms it and points it somewhere. Grim has no
unsolicited egress today, and this keeps it that way.

Outcome target: goal loop (`/hex-loop`).

Out of scope:
- a maintainer-hosted or default collector;
- crash reporting (OBS-13 stays as is);
- per-tool-call export from `grim hook run` and `grim mcp`;
- the traces and metrics signals (metrics are derived collector-side;
  traces wait until registry-request spans are wanted);
- gRPC transport;
- any config-file (`grimoire.toml`) form of the switches;
- user or host identity collected by grim, and branch, commit or author.

Touches (expected): `Cargo.toml`; new `src/telemetry.rs`; `src/main.rs`
(subscriber init at `:396-417`, flush at the exit seam `:199`/`:223-225`;
`src/command/hook.rs:315-337` pins strings in `main.rs`); `src/env.rs`
(`GRIM_TELEMETRY`, `GRIM_TELEMETRY_REPOSITORY` accessors);
`src/oci/git_provenance.rs` (origin read and `normalize_remote_url`
hardening); `src/catalog/forge.rs` (`CiEnv` reuse);
`src/install/target.rs:308` (`detect_clients`); the install/update/add
report paths (`src/command/install.rs:495-542`) for artifact events; the
command sites that hand late fields to the `grim.command` event;
`docs/src/content/docs/configuration.md` (env table), a telemetry
docs page with the field table, `docs/src/content/docs/stability.md`
(Unstable); the `AGENTS.md` env table; a drift review of
`catalog/skills/grim-usage`; and a new ADR under `.agents/adr/`.

## Requirements

- Export is opt-in and env-only. `GRIM_TELEMETRY` (a bool, shaped like
  `GRIM_OFFLINE`) arms it. Every other knob is the standard OpenTelemetry SDK
  env set (`OTEL_EXPORTER_OTLP_[LOGS_]ENDPOINT`, `_HEADERS`, `_TIMEOUT`,
  `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES`). `OTEL_SDK_DISABLED=true`
  and `DO_NOT_TRACK=1` switch it off, even when armed.
- Without `GRIM_TELEMETRY`, grim makes no telemetry network call, even when
  `OTEL_*` variables are present.
- Scope v1: CLI commands only. `grim hook run` never exports. `grim mcp`
  records only its session (one record, written at session end), with no
  per-tool-call records.
- Signal: logs only, as OTel events. Per invocation, one `grim.command` event
  plus one `grim.artifact.changed` event per changed artifact, all in one
  OTLP POST. grim exports no traces and no metrics.
- Resource attributes: `service.name` (`grim`, overridable by
  `OTEL_SERVICE_NAME`), `service.version`, `os.type` and `host.arch` (set from
  `std::env::consts`, not resource detectors), `grim.installation.id`, plus
  operator `OTEL_RESOURCE_ATTRIBUTES`.
- Installation ID: a random uuid v4, written under `$GRIM_HOME` on the first
  armed run only and never created when telemetry is off.
- Repository context only with `GRIM_TELEMETRY_REPOSITORY=1`:
  - attributes `vcs.repository.url.full`, `vcs.owner.name`,
    `vcs.repository.name` and `vcs.provider.name`;
  - derived from `git config --get remote.origin.url`;
  - stripped of credentials, query and fragment, and lowercased;
  - project scope only, and skipped when the repo is the home directory;
  - a `vcs.*` key in `OTEL_RESOURCE_ATTRIBUTES` overrides the derived value.
- Never collected: branch, commit, author, host name, username, cwd, argv.
- No credentials, ever: any URL passes a redactor (vendored rule OBS-22 in
  `.claude/rules/rust-quality/docs-and-tracing.md`; no redactor exists yet).
  Field names follow OTel semconv (OBS-20), and raw args and local paths are
  never recorded.
- Export failure (refused, hung, 5xx) never changes the exit code and never
  prints at the default log level.

## Decisions

- **Gate: `GRIM_TELEMETRY` + standard `OTEL_*`.** Reason: `OTEL_*` is ambient in
  developer shells (this session's shell already carries
  `OTEL_EXPORTER_OTLP_ENDPOINT` and `_HEADERS` for another tool). Claude Code
  (`CLAUDE_CODE_ENABLE_TELEMETRY`), Gemini CLI and Terraform/OpenTofu gate the
  same way. Correction: Claude Code does *not* pass `OTEL_*` to hooks (stripped
  since 2.1.128), so hook inheritance is not part of the reason. Strongest
  counter: ops teams that set `OTEL_*` fleet-wide must also roll out
  `GRIM_TELEMETRY`.
- **Scope: CLI commands only** (see Requirements). Strongest counter: hook
  adoption stays invisible until v2.
- **Switch name: `GRIM_TELEMETRY`.**
- **Payload: #83 floor + artifact refs.** This is the owner's pick. It stands:
  the revisit with the council's evidence was not taken up. The dissent
  (defer refs, keep the decision reversible) is recorded under Threads. Refs
  are normalised (`registry/repo:tag`, userinfo stripped), recorded only on
  resolving commands, and carried as an attribute of the
  `grim.artifact.changed` event, never as a metric dimension.
- **Exporter: the `opentelemetry-otlp` crate** (owner's pick, against the
  recommendation to hand-write it). Obligations it creates:
  - An ADR records the OBS-12 exemption with its justification. The rule is
    vendored and cannot be edited locally.
  - Exact versions are pinned.
  - grim checks `GRIM_TELEMETRY`, `OTEL_SDK_DISABLED` and `DO_NOT_TRACK`
    itself, before building any provider, because the crate ignores
    `OTEL_SDK_DISABLED`.
  - Retries are off (the crate default is 3), and shutdown is bounded by the
    export timeout (the crate hung 5–10 s on a dead collector).
  - The exporter uses grim's own `reqwest` client built via `crate::tls`, so
    a corporate CA in the OS store works; the crate does not honor
    `OTEL_EXPORTER_OTLP_CERTIFICATE`.
  - Features: `http-proto` only, no gRPC/tonic (+1.95 MB).
  Strongest counter: +24 crates and about +731 KB for the logs signal.
- **Signals: events only (logs signal)**, chosen 2026-10-22 on the
  verified facts under Threads. This supersedes the earlier "one span per
  invocation" working position. Strongest counter: no trace waterfall
  until traces are added.
- **Metrics: collector-derived, none emitted by grim.** The docs ship a
  collector recipe. Strongest counter: the connectors are alpha, which is
  the operator's maturity risk.
- **Git context: opt-in via `GRIM_TELEMETRY_REPOSITORY`** (the Claude Code
  shape; see Requirements). Strongest counter: one more switch.
- **Identity: a random installation ID** (`grim.installation.id`), plus
  optional operator `enduser.id`. Strongest counter: a stable pseudonymous
  ID is what employee-monitoring reviews scrutinise, and a shared
  `GRIM_HOME` merges machines.
- **Contract: Unstable until 1.0.** Both switches, the event names, the
  attribute keys and the installation-ID file under `$GRIM_HOME` are listed in
  the Unstable section of `docs/src/content/docs/stability.md`,
  as the NDJSON progress events are. Strongest counter: a minor release can
  break an operator's dashboards.

## Threads

- **Payload (closed: owner's pick stands).** The council split 2–1:
  - User-advocate and operability seats: ship refs, but scoped to resolving
    commands only, normalised (`registry/repo:tag`, userinfo stripped), as a
    span attribute and never a metric dimension.
  - Premortem seat: defer refs. Adding a key later is additive, but removing a
    released key is breaking (Principle 9). A repo-supplied environment (T3)
    can arm switch and endpoint together and harvest the refs of global-scope
    installs.
  - Recon: only command, version, platform and exit code are known at
    dispatch. Scope, clients and refs resolve inside each command and would be
    recorded onto the root span.
- **Exporter implementation (closed: crate).** The evidence weighed:
  - Hand-written OTLP/HTTP+JSON: 0 new crates and +47 KB, but http/json only.
    Dynatrace is protobuf-only, and New Relic, Splunk and Azure don't document
    JSON, so those backends would need a Collector hop.
  - `opentelemetry-otlp` http-proto: +24 crates and +641 KB.
  The owner chose the crate, which speaks protobuf to every backend. Its
  measured gaps are under Decisions.
- **Record shape (superseded 2026-10-22 by the events-only decision).** The
  earlier position was one span per invocation, per the cli-spans semconv.
  What survives: late fields (scope, clients, artifact outcomes) are
  collected during the run and written into the `grim.command` event at
  exit. `process.exit.code` and `error.type` keep their semconv names
  (OBS-20).
- **Contract (closed: Unstable until 1.0).**
- **Signals: events and metrics (closed: events only, collector-derived metrics).** Verified
  facts:
  - An OTel event is a log record with an event name; the Logs data model is
    Stable.
  - Span events: a deprecation was announced (OTEP 4430, blog 2026-03) but
    not enacted as of spec v1.61.0, and a maintainer said on 2026-10-15 it
    may never happen.
  - opentelemetry-rust 0.33.1: Logs and Metrics are Stable, Traces Beta. A
    single reqwest 0.13.5 serves both grim and the crate.
  - Every signal is its own POST and its own shutdown. Dead-collector wall
    time stacks: refused 0.87, 1.82 and 2.62 s for 1, 2 and 3 signals; hung
    5, 10 and 15 s at default timeouts.
  - Binary size: traces +604 KB, logs +731 KB, metrics +1.24 MB.
  - Prior art: 4 of 5 agent CLIs use log-record events as their primary
    signal (Claude Code `plugin_installed` / `skill_activated`, Codex
    `skill_invocation`, Gemini `extension_install`). Docker CLI emits one
    metric.
  - Metrics from short-lived processes: the SDK defaults to cumulative.
    Prometheus and Mimir need experimental flags for delta (Mimir rejects
    delta by default). Collector connectors (count, spanmetrics,
    signaltometrics, deltatocumulative) are all alpha.
  - Semconv has no CLI metrics or events; cli-spans is Development.
  Working proposal: **events only (the logs signal)**, which is one POST
  and a Stable API:
  - `grim.command`, one per invocation, carrying duration, exit code,
    `error.type`, command, scope, CI and clients;
  - `grim.artifact.changed`, one per artifact, carrying ref, kind and
    action.
  grim emits no metrics; the docs ship a collector recipe that derives
  `grim.invocations`, `grim.invocation.duration`, `grim.errors`,
  `grim.clients`, `grim.artifact.changes` and distinct installations.
  Traces come later, if registry-request spans are wanted.
- **Identity (closed: random installation ID).**
  - grim has no persistent identifier today. `uuid` v4 is already in the
    build graph via `rmcp`.
  - Prior art: Claude Code `user.id` and Gemini `installation.id` are random
    persisted UUIDs. Codex limits `user.email` to log events. Copilot's
    identity switch is off by default.
  - `service.instance.id` is Stable and must be unique per instance; the
    Rust SDK does not set it.
- **Context collection (closed: opt-in repository context).** Three layers:
  - Resource: `service.*`, `os.type`, `host.arch`, and operator-supplied
    `OTEL_RESOURCE_ATTRIBUTES`. No host name or username.
  - Dispatch (argv and env only): command, scope, CI. CI detection exists at
    `src/catalog/forge.rs:84-89`.
  - Late, recorded by commands: clients, artifact events, workspace.
  Git, verified facts:
  - grim reads origin only by spawning `git config --get remote.origin.url`
    (`src/oci/git_provenance.rs:199`, about 1.2 ms on WSL2), in
    build/release/publish. That form ignores `url.insteadOf` token rewrites,
    which `git remote get-url` would return.
  - `normalize_remote_url` (`:363`) strips userinfo but keeps `?query` and
    `#fragment`, so it must be hardened (OBS-22).
  - No general repo detector exists. `git_info_dir`
    (`src/install/hook_registrar.rs:1859`) is wrong for linked worktrees.
  - The workspace root is not the git root (monorepo subdirectories), and
    the global scope's workspace is `$GRIM_HOME`.
  - Precedent: release's `Auto` mode withholds origin as an "infrastructure
    disclosure".
  - Closest prior art: Claude Code `OTEL_METRICS_INCLUDE_REPOSITORY`, off by
    default. It sends plaintext `vcs.repository.url.full`, `vcs.owner.name`,
    `vcs.repository.name` and `vcs.provider.name` (semconv RC), stripped of
    credentials, query and fragment, lowercased, skips a home-directory repo,
    and can be overridden via `OTEL_RESOURCE_ATTRIBUTES`. Copilot sends repo,
    branch and SHA with no gate.
  - Do not use `opentelemetry-resource-detectors`: it collects unredacted
    argv, spawns `hostname`, reads the raw machine-id, and reports
    `os.type=macos` where semconv says `darwin`. Set `os.type` and
    `host.arch` from `std::env::consts` instead.
  - Never collected: branch, author.

## Research

- `.agents/research/research_otel_codebase_recon.md`: tracing init
  (`src/main.rs:396-417`), the exit seam (`src/main.rs:199,223-225`),
  `src/main.rs` strings pinned by `src/command/hook.rs:315-337`, the hook
  config-import ban, reqwest/TLS, and attribute availability.
- `.agents/research/research_otel_cli_prior_art.md`: the OTel env-var spec,
  receiver JSON support, opentelemetry-rust behaviour and weight (measured),
  CLI gating prior art, and CLI-span semconv.
- `.agents/research/research_otel_repo_archaeology.md`: OBS-12 provenance (an
  unmeasured, doc-derived default from `ocx-sh/grimoire-lore` `30ef76e`), and
  ocx's hand-written OTLP/HTTP+JSON exporter as precedent.
- `.agents/research/research_otel_community_threads.md`: Go/.NET/Homebrew/gh
  telemetry reception, ambient-`OTEL_*` incidents, payload leaks (paths and
  args, as at inlang), and hosts kept out by rule (gh, Homebrew, .NET).
- Council (inline, not persisted): premortem / user-advocate / operability
  seats on the payload, summarised under Threads.
- `.agents/research/research_otel_signals_events_metrics.md`: events vs span
  events, Rust signal stability, measured per-signal size and shutdown cost,
  temporality per backend, collector connector maturity.
- `.agents/research/research_otel_prior_art_signal_catalog.md`: exact
  metric, event and attribute catalogs of Claude Code, Gemini CLI, Codex,
  Copilot and Docker CLI; identity and repo gating; name-redaction
  precedent.
- `.agents/research/research_otel_context_semconv.md`: vcs.*, cicd.*,
  host.* and user.* semconv status; resource-detector hazards; credential
  URLs; hashing evidence.
- `.agents/research/research_otel_codebase_context_sources.md`: where each
  context attribute is computed, when it is known, what it costs; gaps in
  `normalize_remote_url` and `git_info_dir`.
- All four were adversarially fact-checked (one skeptic per lane, 33
  claims). Corrections applied: process.* became RC in v1.42.0; the Docker
  CLI does export registry HTTP client spans; a crate count of 106/130
  depends on how it is counted, while the +24 delta is stable.

## Related

- [grimoire-rs/grimoire#206](https://github.com/grimoire-rs/grimoire/issues/206): otel support
- [grimoire-rs/grimoire#83](https://github.com/grimoire-rs/grimoire/issues/83): opt-in usage and error telemetry
- `.claude/rules/rust-quality/docs-and-tracing.md` (OBS-12, OBS-17, OBS-20, OBS-22, OBS-23)
- `.claude/rules/arch-threat-model.md` (T3: repo-supplied input; no egress ruling yet)
- `.agents/adr/adr_vendor_support_tiers.md:289-290` (re-check the Tier 1 set "when usage telemetry lands"; it needs `detected clients`)
- `docs/src/content/docs/stability.md` (Environment variables row, Unstable section)

## Open questions

- [NEEDS CLARIFICATION: Which headers go to which endpoint?]
  Recommended: send `OTEL_EXPORTER_OTLP_HEADERS` only with the base
  `OTEL_EXPORTER_OTLP_ENDPOINT`, and give a signal-specific endpoint only
  `OTEL_EXPORTER_OTLP_LOGS_HEADERS`. Reason: otherwise a repo-set
  `*_LOGS_ENDPOINT` receives the user's ambient collector credentials. This
  is a small, documented deviation from the spec.
- [NEEDS CLARIFICATION: What is the export timeout default?]
  Recommended: honor `OTEL_EXPORTER_OTLP_[LOGS_]TIMEOUT`; when unset, use
  500 ms, one attempt and no retries (not the spec's 10 s). Reason: a CLI exit
  must not stall on a dead collector. Docker CLI uses 50 ms, which suits only
  a local collector.
- [NEEDS CLARIFICATION: How does an operator audit the payload?]
  Recommended: log the exact JSON body at `GRIM_LOG=debug`, plus a frozen field
  table in the docs, and add no separate dry-run mode. Reason: zero new
  surface; gh's `GH_TELEMETRY=log` precedent.
- Resolved: `opentelemetry-http` 0.33.1 uses the same reqwest 0.13.5 as grim,
  so there is no second TLS stack (fact-checked against `Cargo.lock` and the
  upstream workspace pin).
- [NEEDS CLARIFICATION: How are events emitted?]
  Recommended: call `Logger::create_log_record` + `set_event_name` + `emit`
  directly, with a fixed set of event names. Reason: this is verified working
  (POST /v1/logs). `opentelemetry-appender-tracing` would forward every
  `tracing` event (353 sites) and need a filter. Caveat: the crate docs call
  the logs API a bridge "not intended to be called by application developers
  directly".
- [NEEDS CLARIFICATION: Where does init happen relative to the hook-run return?]
  Recommended: initialise telemetry only after the `Hook(Run)` early return,
  and add a test pinning that. Reason: the hook import-ban tests
  (`src/command/hook.rs:123-129`) scan only 5 files, so init code that reads
  config or `grim_home()` before the return would break the hook
  guarantee untested.
- [NEEDS CLARIFICATION: What happens when `OTEL_EXPORTER_OTLP_PROTOCOL` is not
  `http/protobuf`?]
  Recommended: skip export and log once at debug. Reason: without the
  `http-json` feature, the crate silently sends protobuf instead.
- [NEEDS CLARIFICATION: Where is `normalize_remote_url` hardened?]
  Recommended: strip query and fragment and lowercase inside a
  telemetry-side wrapper, and leave the shared function alone unless a test
  proves that release's `org.opencontainers.image.source` annotation output
  is unchanged. Reason: that annotation is a frozen OCI wire contract
  (`docs/src/content/docs/stability.md`), and a changed value alters manifest
  digests on re-release.
- [NEEDS CLARIFICATION: Where does the installation ID live?]
  Recommended: `$GRIM_HOME/telemetry/installation-id`, created with an atomic
  write on the first armed run and listed as Unstable. Reason: the
  `$GRIM_HOME` layout is a contract (`.claude/rules/subsystem-file-structure.md`),
  and a dedicated directory keeps it clear of install state.
- Ctrl-C and panics skip the flush, so that record is lost. Accepted.
- `service.version` is the crate version (`CARGO_PKG_VERSION`) and rides on
  every event. A build git SHA is not available, because OBS-15 is unmet
  (no `build.rs`). That is out of scope here.

## Verification

- Acceptance tests (`test/`, pytest) against a local OTLP/HTTP capture server:
  - nothing is sent without `GRIM_TELEMETRY`, even with `OTEL_*` set;
  - exactly one `/v1/logs` request per invocation when armed, carrying one
    `grim.command` event plus one `grim.artifact.changed` event per changed
    artifact, with the documented attribute set and no others;
  - every event carries `service.version` and `grim.installation.id`;
  - no `/v1/traces` or `/v1/metrics` request, ever;
  - `OTEL_SDK_DISABLED=true` and `DO_NOT_TRACK=1` suppress the export;
  - `OTEL_EXPORTER_OTLP_HEADERS` reach the collector;
  - `grim hook run` never exports;
  - the installation-ID file is absent after unarmed runs and stable across
    armed runs.
- Repository context:
  - without `GRIM_TELEMETRY_REPOSITORY`, no `vcs.*` attribute is sent and
    no `git` process is spawned;
  - with it, in a repo whose origin is
    `https://user:tok@Host/O/R.git?x=1#f`, the event carries
    `vcs.repository.url.full=https://host/o/r` and nothing else from the
    URL;
  - a global-scope run and a home-directory repo carry no `vcs.*`.
- Release regression: the `org.opencontainers.image.source` annotation is
  unchanged for the existing `normalize_remote_url` test vectors.
- Failure tests: with the collector refused, returning 503, or hanging, the
  exit code is unchanged, stderr is clean at the default log level, and the
  added wall time stays under the export timeout.
- A unit test for the URL redactor (OBS-22), and a test that fails on any
  attribute key missing from the documented field table.
- One interop run against a real OpenTelemetry Collector container, with the
  events asserted in its debug exporter output, plus the documented collector
  recipe deriving `grim.invocations` from them.
- Size check: record the release binary size before and after in the ADR.
- `task verify` green, and the `grim-usage` drift review done
  (`task catalog:verify`).
