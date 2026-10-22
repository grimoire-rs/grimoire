# Research: OTEL telemetry — codebase recon

## Metadata

**Date:** 2026-10-22
**Domain:** observability / security
**Triggered by:** `.agents/discussions/otel-usage-telemetry.md` (State: active; body empty at read time); GitHub [grimoire-rs/grimoire#83](https://github.com/grimoire-rs/grimoire/issues/83) (opt-in usage and error telemetry) and [#206](https://github.com/grimoire-rs/grimoire/issues/206) ("otel support", empty body)
**Expires:** 2027-04-22

Tags: V = verified by reading code or running a read-only command this session. I = inferred from code, not run. U = UNVERIFIED (needs a lane from "Open leads").
Scope: what the existing grim code offers and constrains. Crate versions, OTLP spec behaviour and opentelemetry-crate internals were NOT researched here (no network use).

## Direct Answer

Recon only; no recommendation. The five facts that bound the four decisions:

1. **No spans, no run_id, one fmt layer.** `init_tracing` installs a single `fmt` layer behind a per-layer `EnvFilter` on `GRIM_LOG` (`src/main.rs:396-417`). No `#[instrument]`, `info_span!`, `run_id` or `trace_id` exists in `src/` (V, grep empty). 353 event macro sites (V). The vendored rules OBS-17 and OBS-23 (root span, `run_id`) are unmet today.
2. **One exit seam, no `process::exit`.** `main` returns `std::process::ExitCode` (`src/main.rs:152`); `process::exit` appears nowhere in `src/` (V). The exit code is only known after `runtime.block_on(app::run(cli))` (`src/main.rs:199`, `:223-225`). No signal handler, no panic hook (V): Ctrl-C and panics skip any end-of-run flush.
3. **`opentelemetry*` is banned by a vendored MUST rule, enforced by review only.** OBS-12 (`.claude/rules/rust-quality/docs-and-tracing.md:99`) bans it in a short-lived CLI unless there is a daemon-mode rationale. `deny.toml` `[bans]` holds only `multiple-versions = "warn"` (V), so nothing machine-blocks the crates. The tree has no `prost`, `protobuf`, `tonic`, `h2` or `opentelemetry*` (V, `cargo tree -i` and `Cargo.lock`).
4. **The hook dispatcher is the hot path.** `grim hook run` runs once per armed `(client, event)` per tool call (`src/main.rs:250-251`), on a current-thread runtime, with a measured 3.2 ms warm floor and no pass/fail latency gate (`.agents/hook_dispatch_latency.md:3-6`). It returns before `Context::new` and is under source-level import bans (`src/command/hook.rs:154-170`).
5. **Scope, clients, artifacts and registries are not known at dispatch.** `Context::new` carries only flags and a few env values (`src/context.rs:167-183`); `ResolvedScope` and client detection happen inside each command. At dispatch only the clap `Command` variant, the global flags and the environment are in hand.

## Key Findings

### 1. Tracing setup

1. Init is the first statement of `main`, before argv parse: `init_tracing()` at `src/main.rs:153` (V).
2. Subscriber: `tracing_subscriber::registry().with(fmt::layer().with_writer(stored.clone()).with_filter(filter)).try_init()` at `src/main.rs:410-416`; the result is discarded (`let _ =`). The filter is `EnvFilter::try_from_env("GRIM_LOG")`, default `warn` (`src/main.rs:400`). It is attached **to the fmt layer**, not globally, so a second layer needs its own filter (I).
3. The writer is a `SwitchableWriter` (`src/log_switch.rs`), a process-global (`GLOBAL_WRITER`, `:123`, `:130`) swapped to `$GRIM_HOME/tui.log` while the TUI holds the alt-screen (`src/tui/app.rs:230-240`). OBS-24 is satisfied by this switch; a new layer that writes to stderr would bypass it (I).
4. Cargo features on `tracing-subscriber`: `env-filter, std, fmt, json, registry` (`Cargo.toml:21-27`), so `Registry`-based layering is already available (V).
5. `--log-level` is declared (`src/cli/options.rs:80-82`) but no code reads it: every `GlobalOptions` constructor sets `None` and `init_tracing` never sees parsed args (V, grep). `GRIM_LOG` is not in `docs/src/content/docs/configuration.md` or the `AGENTS.md` env table (V, grep empty). Logging control is env-only in practice.
6. OBS-17/OBS-23: zero spans, zero `run_id` (V). OBS-22 (URL redaction before recording): no general redactor exists. Partial prior art only: `normalize_remote_url` strips credentials from git remotes (`src/oci/git_provenance.rs:363`), `index_host` strips them for announce (`src/catalog/index_announce.rs:28`), `SecretString` types for credentials (`src/auth/credential.rs:7`). The hook audit trail is "redacted metadata" by default (`src/hook/audit.rs:4-7`).
7. A `tracing-opentelemetry`-style bridge would find nothing but the 353 events to export; a per-invocation record needs either a new root span (which also satisfies OBS-17/23) or direct OTel/hand-built records (I).

### 2. Process lifecycle

1. Sequence in `main` (`src/main.rs:152-226`): `init_tracing` → `color::mode_from_args` → `parse_cli` (`:157`) → `color::init` → `build_runtime(runtime_flavor(..))` (`:184`) → `runtime.block_on(app::run(cli))` (`:199`) → `match` that maps `Ok(code)` / `Err(err)` to a `std::process::ExitCode`.
2. Early returns that never reach the runtime: clap usage errors (exit 64, `:168`), `--help`/`--version` (exit 0, `:167`), runtime-build failure (`:186-195`). A record created only after `block_on` misses these (V).
3. Dispatch: `app::run` (`src/app.rs:26`). Bare `grim` prints help (`:29-40`); `Hook(Run)` returns at `:60-64` before `Context::new` (`:66`); the big `match` follows (`:70-197`). `Schema` and `Completions` skip rendering (`:171`, `:174`). There is no `Command::name()` accessor (V, grep); a command-name attribute needs a match over the enum or clap's matched subcommand.
4. Exit code: `ExitCode` enum with a stable `slug()` (`src/cli/exit_code.rs:64-79`: success, failure, usage, data, unavailable, io, temp-fail, no-permission, config, not-found, auth, offline-blocked). Errors classify via `classify(&err)` (`src/error.rs:200`) into `{exit, reason}`; `ErrorReason` is a closed 7-variant enum (`src/error.rs:113-141`). Both are bounded-cardinality candidates for an error-kind attribute (V).
5. Destructors: no `process::exit`, no `abort`, no `mem::forget` in `src/` (V). The runtime is dropped at `main`'s scope end, after the `match` value is computed. `tokio::runtime::Runtime` drop waits on `spawn_blocking` tasks (I, Tokio behaviour, not re-read).
6. A flush placed right after `block_on` returns (`src/main.rs:199`) sees the final exit code on both arms and runs before the stderr writes; the `Err` arm has early returns inside it (`:207-209` stdout-pipe-closed → exit 0), so a flush placed inside the arms would miss that one (V).
7. Structural pins on `main.rs`: `command/hook.rs:315-337` asserts `main.rs` contains `build_runtime(runtime_flavor(` and exactly one `tokio::runtime::Runtime::new()`. A change to `main.rs` must keep those strings (V).
8. Not covered by an end-of-run flush: SIGINT/SIGTERM (no `tokio::signal` or `ctrl_c` anywhere, V), panics (no `set_hook`, `catch_unwind`, `human_panic`; `resume_unwind` only re-raises task panics, V), and `kill -9`.
9. Network under offline: `GRIM_OFFLINE`/`--offline` means "Disable all network access" (`src/env.rs:14-15`, `docs/.../configuration.md` env table). It is honoured per command via `ctx.offline()` (`rate.rs:171`, `status.rs:484`, `search.rs:178`, …), not at a single chokepoint, because the direct `reqwest` clients are built independently (V). A telemetry export would be a new network path that must choose a side of this rule (decision, not code fact).

### 3. Long-running and hot-path modes

**`grim hook run`**
1. Invoked by the generated launcher once per armed `(client, event)` per tool call (`src/main.rs:250-251`; `.agents/hook_dispatch_latency.md:353-358`). The registered `sh` string forks the launcher shim, which execs grim (`src/install/hook_launcher.rs:31-60`).
2. Measured on WSL2 ext4, release binary: no-match floor 3.2 ms p50 (runtime only), 4.31 ms p50 / 5.54 ms p99 through the chain, cold page cache +4.3 to +5.1 ms; Linux native, macOS and Windows rows are "not measured" (`.agents/hook_dispatch_latency.md:39-52`, `:150-156`).
3. Budget: **no number is documented anywhere**. The latency doc has "No pass/fail gate depends on any number here" (`:3`); the ADR says "No target is invented here, deliberately" (`.agents/adr/adr_hooks_support.md:1682`). Prior optimisation: `current_thread` runtime chosen to save about 1.9 ms of the 3.2 ms floor (`src/main.rs:248-274`, pinned by `command/hook.rs:315-337`).
4. Binary size is already named as the cold-start cost: 26.8 MB then, "The honest fix for the cold row is the binary's size" (`src/main.rs:263-272`); the current `target/release/grim` is 29.5 MB (V, `ls`).
5. Import bans on the runtime files (source-level test `the_runtime_imports_no_scope_no_config_no_data_root_c007`, `src/command/hook.rs:154-207`): `crate::config`, `scope_resolution`, `crate::context`, and a call to `grim_home(`. An OTEL env read via `std::env::var` or a new `crate::env` accessor does not contain those needles (I); reading `[options]` config for an opt-in would (V). A separate C-009 test bans hashing symbols on this path (`src/command/hook.rs:209-231`).
6. Environment: the runtime inherits whatever the client process passes (the launcher is `sh`, no `env_clear`); the **handlers** it spawns inherit the parent env plus an allowlist of `GRIM_HOOK_*` names (`src/command/hook/pipeline.rs:896-903`, `envelope.rs:431-462`; no `env_clear` in `pipeline.rs`, V). The CWD of a client-spawned run is the workspace (`src/app.rs:45-58`), so repo-supplied env (`.envrc`, `.mise.toml`, devcontainer `containerEnv`) reaches the process (the repo's own threat-model reasoning, `src/command/hook.rs:44`, `AGENTS.md` "(no variable)" row).
7. Invariant I3: `hook run` must never fail or block the client; every refusal exits 0 (`src/command/hook/run.rs:137-146`, `arch-threat-model.md` I3). An export on this path would need to be fire-and-forget with a hard deadline, and it adds a TLS client build: `merge_embedded_roots` parses the compiled-in Mozilla root set on every client build (`src/tls.rs:55-62`).
8. Payloads carry tool-call data (stdin, up to 8 MiB, `run.rs:90`); nothing from it is currently exposed beyond the allowlisted env names.

**`grim mcp`**
9. Long-running STDIO server (`src/mcp/server.rs:183-201`); `service.waiting().await?` returns on client EOF, then `Ok(ExitCode)` falls through `app::run`. "One record per invocation" is one record per session here; per-tool-call spans would be a different shape (I). `info!` logs exist at start/stop (`:198`, `:201`). Env is whatever the harness passes.

**`grim tui`**
10. Raw-mode alt-screen session; non-TTY stdout exits 0 immediately (`src/command/tui.rs:76-84`). Logs go to `$GRIM_HOME/tui.log` for the session (`src/tui/app.rs:230-240`), so any stderr-bound error from an exporter would be redirected to that file (V). It already runs background network tasks (catalog refresh, per-row digest re-checks, bounded by `Semaphore`, `src/tui/update_check.rs:1-40`) and `tokio::spawn`s in `src/tui/app.rs:3694`; those are registry checks, not telemetry. There is **no** self-update check or phone-home to a Grimoire service anywhere in `src/` (V, grep for `releases/latest|self_update|check_for_update` empty).

### 4. HTTP client

1. `reqwest 0.13.5` (`Cargo.lock:2443-2445`), declared `default-features = false, features = ["rustls"]` (`Cargo.toml:58`). Effective features include `json`, `query`, `stream` via `oci-client` (V, `cargo tree -e features -i reqwest`) and `__rustls-aws-lc-rs`; `http2` is **off**, so `h2` is absent from the tree (V). `gzip` is deliberately off crate-wide (comment `Cargo.toml:60-62`; hand-decoded in `src/catalog/index_source.rs:629`).
2. TLS: `rustls 0.23.46` with `aws-lc-rs` (`Cargo.lock`, V); `ring` has a `Cargo.lock` entry (`:2484`) but is not in the built graph (V, `cargo tree -i ring --target all` prints nothing). Roots: reqwest's `rustls-platform-verifier` path plus compiled-in `webpki-root-certs`, merged as extra roots via `tls_certs_merge` so `SSL_CERT_FILE`/`SSL_CERT_DIR` still work (`src/tls.rs:37-62`, `Cargo.toml:63-67`).
3. **No shared client.** Four direct construction sites plus `oci-client`:

   | Site | Timeouts | Notes |
   |---|---|---|
   | `src/auth/verify.rs:250` | `.timeout(30s)` | no `connect_timeout` |
   | `src/catalog/forge.rs:279` | `.timeout(t)` (30 s via `client()` at `:304`) | `Policy::none()` redirects, with a SECURITY comment (`:282-290`); no `connect_timeout` |
   | `src/catalog/index_source.rs:596-608` | connect 10 s, read 30 s, total 300 s (`:34-42`) | the only site with all three |
   | `src/oci/access/registry_client.rs:178-183` | `oci-client` defaults | embedded roots via `extra_root_certificates` |

   Every site sets `user_agent("grim/<CARGO_PKG_VERSION>")` and goes through `crate::tls`. SEC-16 (vendored, MUST) asks for both timeouts on every builder; two of three direct sites lack `connect_timeout` (V).
4. Proxy: reqwest's `auto_sys_proxy` defaults to true (`reqwest-0.13.5/src/async_impl/client.rs:310,417-418`) and `Proxy::system()` calls hyper-util's `Matcher::from_system()`, which reads `ALL_PROXY`, `HTTPS_PROXY`, `HTTP_PROXY`, `NO_PROXY` regardless of the `system-proxy` feature; that feature only adds macOS/Windows OS-level settings (`hyper-util-0.1.20/src/client/proxy/matcher.rs:95-110`). So every grim client honours env proxies today (V by source read) and none is configured explicitly. No grim doc mentions proxies (V, grep: only `SSL_CERT_*`).
5. Serde: `serde_json 1` with `raw_value` is a direct dep (`Cargo.toml:29`), as are `serde`, `tokio` (`full`), `base64`, `flate2`, `chrono`. No `prost`, `protobuf`, `tonic`, `h2`, `hyper` as direct deps; `hyper 1.11`, `hyper-util`, `http-body-util` are present transitively through `reqwest` only (V, `cargo tree -i`). `prost`/`tonic` sources exist in `~/.cargo/registry` from other projects, not this tree.
6. `deny.toml`: `[bans]` has only `multiple-versions = "warn"`; no `openssl`/`native-tls`/crate bans although SEC-14 asks for them (V). License allow-list: Apache-2.0, MIT, BSD-3-Clause, ISC, Unicode-3.0, CC0-1.0, CDLA-Permissive-2.0, MIT-0, Zlib, OpenSSL, EPL-2.0, MPL-2.0 (no BSD-2-Clause). `unmaintained = "workspace"`, `unknown-git = "allow"`. The CI step runs `cargo deny check advisories bans sources` (`taskfiles/rust.taskfile.yml:109`) and `licenses` (`:98`).

### 5. Env-var conventions

1. `src/env.rs` holds four typed accessors (`GRIM_HOME`, `GRIM_DEFAULT_REGISTRY`, `GRIM_OFFLINE`, `DOCKER_CONFIG`) with `non_empty_var` and a `truthy` parser (`1|true|yes|on`, `env.rs:12-17,69-76`); its header says `GRIM_LOG` is "intentionally absent" (`:6`). Reality is scattered: `GRIM_INSECURE_REGISTRIES` at `src/oci/access/registry_client.rs:156`, `GRIM_RATE_TOKEN` at `src/command/rate.rs:93,424,753`, colour vars at `src/cli/color.rs:75-78`, vendor dirs at `src/install/vendor.rs:1551`, CI vars at `src/catalog/forge.rs:77-79` (V).
2. Production `env::set_var` is banned (SEC-05); `env.rs` tests use pure inner helpers (`env.rs` test-module comment).
3. `DO_NOT_TRACK`: **zero references in `src/`, `docs/` or `catalog/`** (V). It appears only in `.claude/skills/upstream-refresh/references/deep-pass.md:66` and three `.agents/research/research_upstream_*` files, as a sandbox variable set on *vendor* CLIs. Issue #83 asks grim to honour `DO_NOT_TRACK` plus "the host editor's telemetry setting".
4. `[options.experimental]`: a struct with one field `hooks: bool`, `deny_unknown_fields`, omitted when empty (`src/config/declaration.rs:202-247, 324-327`). Set via `grim config set options.experimental.hooks true` (`src/command/config.rs:816-820`). **Deliberately config-only: "no environment variable overrides this"** (`declaration.rs:206-207`; docs `configuration.md:89-110`). `AGENTS.md`'s "(no variable)" row explains why for hooks: a repo carries its own env (`.envrc`, `.mise.toml`, devcontainer `containerEnv`, CI `variables:`), so an env-settable gate lets a repo consent on a cloner's behalf (CWE-426). Both flags and consent records stay outside the environment.
5. Where users read env vars: `docs/src/content/docs/configuration.md:1279` ("Environment variables" table, ~35 rows) and the `AGENTS.md` "Environment Variables" table. Adding a documented `GRIM_*` var also triggers the catalog drift review for `grim-usage` ("CLI or docs-page changes", `AGENTS.md` "First-Party Catalog").
6. The acceptance runner builds the child env from scratch (`test/src/runner.py:65-77`: `GRIM_HOME`, `PATH`, `HOME`, `USERPROFILE`, `APPDATA`, `XDG_CONFIG_HOME` only), so ambient `OTEL_*` on a developer machine cannot reach the tests; a telemetry test must inject its own env (V).
7. Stability: "Environment variables: the documented `GRIM_*` set and honored vendor overrides" is a frozen row (`docs/src/content/docs/stability.md:44`); "human-readable log or error text carry no compatibility promise — only exit codes and structured JSON output are contracts" (`:231-234`). A new documented variable extends the frozen set additively (I, Principle 9); a new log or stderr line is outside the contract (V). Nothing in `stability.md` covers a new outbound network surface (V, grep).

### 6. Attribute candidates and hazards

Availability at dispatch (`src/app.rs:66` `Context::new`, then the `match`):

| Candidate | Where it is available | Cost / hazard |
|---|---|---|
| Command name | clap `Command` variant (`src/main.rs:92-146`); no accessor | needs a match arm per variant or clap's matched subcommand; `Hook` has sub-subcommands |
| grim version | `env!("CARGO_PKG_VERSION")` (used as user-agent at 4 sites); clap `--version` | no git SHA or build date (no `build.rs`, no VERGEN; OBS-15 unmet) |
| OS / arch | `std::env::consts` not used anywhere in `src/` (V); only `cfg!(windows/target_os)` branches | trivial to add |
| Exit code / error kind | after `block_on` only (`src/main.rs:199-225`) | closed enums, `slug()` and `ErrorReason` Display |
| Output format, offline, `--global`, `--registry` flag count | `GlobalOptions` / `Context` accessors (`src/context.rs:186-240`) | cheap; `--registry` values themselves are hosts |
| Scope (project/global) | `ResolvedScope.scope` (`src/command/scope_resolution.rs:26-28`) inside each command; `ctx.global()` only says whether `--global` was typed | per-command plumbing |
| Detected / selected clients | `detect_clients` (`src/install/target.rs:308`) called inside commands; `ClientTarget` is a closed enum | per-command plumbing; low cardinality |
| Artifact refs, registry hosts, paths | inside each command's report | see issue #83 constraint below |
| Duration | wall clock around `block_on` | trivial |
| `run_id` | does not exist | must be generated (OBS-23) |

Constraints and hazards:

1. **Issue #83's own scope statement**: "Artifact names, registry hosts, workspace paths, and config values from private setups are never transmitted. Payload is command name, version, platform, error kind." Off by default, one setting to turn off, honours `DO_NOT_TRACK`, redirectable collector (pointing it nowhere sends nothing), documented exact field list (V, issue body read via `gh`). The brief for this recon lists artifact refs and registry hosts as candidates; #83 excludes them.
2. **Threat model has no egress ruling.** `arch-threat-model.md` (T1-T5 in scope, N1-N5 out) and `quality-security.md` contain no text on outbound telemetry, exfiltration or data leaving the machine (V, grep empty for exfil/outbound/egress/phone home). Closest: T3 (cloning a repo is not consenting to it), I1/I4 (nothing armable in a repo; default-deny for execution), I6 (secrets never in argv/env), and SSRF noted for `grim mcp` ("no registry param on any tool"). A repo-supplied environment can set any `OTEL_*` variable (the project's own CWE-426 reasoning), so an activation rule that is "any `OTEL_EXPORTER_OTLP_ENDPOINT` present" is repo-settable (I).
3. **No offline-mode rule for telemetry** (see Lifecycle 9).
4. **Redaction**: OBS-22 requires a URL redactor before recording any URL; none exists as a general helper (Key Finding 1.6). OBS-05: nothing carrying a credential gets `Debug`/`Display`; credentials are `SecretString` (`src/auth/credential.rs`). Registry hosts, `GRIM_HOME` and workspace paths appear in many error messages (`classify` chains), so an error *message* attribute would leak what #83 excludes; an error *slug* would not (I).
5. **Hook payload data**: `grim hook run` handles tool-call content (stdin) and records a redacted audit trail by design (`src/hook/audit.rs:4-7`); that precedent defines "metadata only" locally.
6. **Credential proximity**: an exporter header (`OTEL_EXPORTER_OTLP_HEADERS`) is a bearer token in env; I6 and SEC-24 say secrets in env are readable by other processes at the same privilege (T5). Standard OTEL practice puts them there (U on the exact var set).

### 7. Contract and policy

1. Frozen: documented `GRIM_*` env set (`stability.md:44`); CLI flags and exit codes; JSON shapes (additive-only). Not frozen: log/stderr text, TUI appearance, plain-mode text (`:231-234`).
2. Principle 9 (AGENTS.md): breaking changes prohibited, evolution additive-only. A new opt-in env var or config key is additive; a changed default (telemetry on) would not be (I).
3. Prior mentions of telemetry in `.agents/`: `.agents/adr/adr_vendor_support_tiers.md:290` ("Re-check the Tier 1 set when usage telemetry lands (grimoire-rs/grimoire#83) — the first evidence-based promotion/demotion input grim will have"); `.agents/plans/plan_harness_capability_freshness.md:183-184` (DO_NOT_TRACK plus vendor opt-outs for *vendor* CLI sandboxes); `.agents/specs/design_brief_landing_page.md:151` ("no analytics" on the landing page). No ADR, plan or research on grim's own telemetry exists (V). `.claude/rules/docs-quality/observability.md` and `.claude/skills/docs-instrument/references/reader-signals.md` cover *docs-site* analytics, not CLI telemetry.
4. Vendored rules in play (all MUST unless noted): OBS-12 (no `opentelemetry*` in short-lived CLI), OBS-13 (remote crash reporting is opt-in behind explicit runtime consent), OBS-17/23 (root span + `run_id`, SHOULD for 23), OBS-20 (OTel semconv field names: `error.type`, `url.full`, `server.address`, `oci.*`), OBS-22 (URL redaction), OBS-03/19 (span guards across `.await`, `tokio::spawn` wrapped in `.in_current_span()`), SEC-14/16/18, EXIT-02. The `.claude/rules.md` catalog flags the vendored `rust-quality.md` vs first-party `quality-rust.md` overlap as an unresolved owner decision, so which set is authoritative for OBS-12 is not settled (V, `rules.md` "Open").
5. Cargo.toml dependency comments show a house style of one-sentence justification per dependency and a preference for avoiding crate-wide feature unification (`Cargo.toml:60-62`, `:97-103`).

## Evidence by option

### D1. `opentelemetry*` crates vs hand-written OTLP/HTTP

| | For | Against |
|---|---|---|
| **Crates** | OBS-20 already requires OTel semconv names, so vocabulary matches. `tracing-subscriber` `registry` is already enabled, so a layer attaches at `src/main.rs:410`. `reqwest 0.13` + rustls + `serde_json` already in tree (compat of `opentelemetry-http` with reqwest 0.13: U). Standard `OTEL_*` precedence, headers, compression and timeouts would come from the library (U). | OBS-12 MUST (needs a written daemon-mode rationale; the short-lived-CLI case is the rule's exact target). Review-only gate: `deny.toml` bans nothing. Today there are no spans, so a tracing bridge exports only events until OBS-17/23 work lands. No `prost`/`h2`/`tonic` in tree: http/protobuf adds `prost`, http/json would reuse `serde_json` (feature choice U). Binary is already 29.5 MB and size is the named cold-start cost for hooks. Exporter threads or runtime requirements vs the single-thread hook pin: U. `multiple-versions = "warn"` would likely warn on new transitive duplicates (I). |
| **Hand-written** | `reqwest`, `serde_json`, `tokio`, `chrono` already direct deps; zero new transitive crates, so no OBS-12 or `cargo-deny` delta. Repo precedent for small bespoke clients with tailored timeouts and redirect policy (`forge.rs:279-292`, `index_source.rs:596-608`) and for avoiding crate-wide features (`Cargo.toml:60-62`). Full control over what a record can contain (field list can be a closed struct, auditable per #83). | A fifth client site beside four unshared ones; each currently differs on timeouts (30 s / 30 s / 10-30-300 s / defaults). Spec surface to implement by hand: JSON-over-HTTP encoding, `OTEL_EXPORTER_OTLP_*` precedence, retry and partial-success semantics (U, spec not read). Semconv field names are by convention, not by library. Any later wish for traces, metrics or logs broadens the hand-written surface. |

### D2. Activation: standard `OTEL_*` only, or also a grim switch

| | For | Against |
|---|---|---|
| **`OTEL_*` only** | One mental model for users of OTel tooling; no new grim var in the frozen set; avoids a third config surface. | A repo can ship `OTEL_*` through `.envrc`/`mise`/devcontainer, so presence-of-endpoint activation lets a cloned repo turn export on and choose the collector (the project's own CWE-426/T3 reasoning for hooks, `AGENTS.md` "(no variable)" row). Standard names also activate for users who set them for *other* tools in the same shell (I). Nothing in grim honours `DO_NOT_TRACK` today; the OTel SDK env set's own off-switch is `OTEL_SDK_DISABLED`, not `DO_NOT_TRACK` (U, from memory of the spec). Hook and MCP processes inherit the client's env, not the user's shell (I). |
| **Also a grim switch** | Matches the existing opt-in precedent: `[options.experimental]` is config-only with an explicit "no environment variable overrides this" (`declaration.rs:206-207`), and `GRIM_OFFLINE` is the precedent for a boolean env var with a matching flag. Issue #83 wants one setting to turn it off and `DO_NOT_TRACK` honoured. Gives a place to document the exact field list. | A new env var joins the frozen documented set (`stability.md:44`) and needs docs + `grim-usage` drift review. If the switch is a config key, `hook run` cannot read it (C-007 import ban, `command/hook.rs:154-170`); if it is env-only it is repo-settable. Config key and env var both: a precedence rule to define (cf. `GRIM_DEFAULT_REGISTRY` vs config, `configuration.md` precedence text). |

### D3. Attributes

| | For | Against |
|---|---|---|
| **Closed, low-cardinality set** (command, version, OS/arch, exit slug, `ErrorReason`, duration, format/offline/global bools) | All available at or around `main.rs:199-225` without new plumbing beyond a command-name match; matches #83's stated payload; closed enums are auditable. | Scope and client set require per-command plumbing; `run_id` and OS/arch must be added. |
| **Open-ended set** (artifact refs, registry hosts, paths, error messages) | Richer for the maintainers' "which artifact fails" question. | Excluded by #83's text; no URL redactor (OBS-22); error chains quote hosts and `GRIM_HOME`; the threat model has no egress ruling; private-registry hosts are customer-identifying (I). |

### D4. Where the record is created and flushed

| | For | Against |
|---|---|---|
| **After `block_on`, before the exit `match`** (`src/main.rs:199`) | Single seam on all non-early paths; exit code and duration known; no destructor-skipping exit path exists. | Misses clap usage errors, `--help`/`--version`, runtime-build failure (they return before). Misses SIGINT, panics and SIGKILL (no handler or hook). `main.rs` is structurally pinned (`command/hook.rs:315-337`). A flush that waits on the network delays every command's exit; a deadline is mandatory (ASYNC-04). |
| **Root span at dispatch in `app::run`** (`src/app.rs:26`) | Satisfies OBS-17/23 as a side effect; scope/clients could be recorded as span fields from inside commands. | `hook run` returns at `:60-64` before any context; span guards across `.await` (OBS-03); needs every `tokio::spawn` (12 sites) wrapped for OBS-19. Early-return paths still missed. |
| **Mode-specific** (hook run, mcp, tui) | `mcp` and `tui` are long-lived, so a "record at exit" is a session record; `hook run` can opt out entirely. | `hook run` is where a per-invocation record is most numerous (per tool call) and most latency-sensitive (3.2 ms floor, no budget number, exit 0 mandatory). Excluding it needs a rule that is itself testable (the file-scan idiom exists, `command/hook.rs:195-207`). |

## Open leads (not done here)

See the return message; recorded for the next lane: crate/feature matrix and reqwest-0.13 compatibility of `opentelemetry-http`/`-otlp`; OTLP/HTTP JSON encoding and `OTEL_*` precedence as the spec states it; `DO_NOT_TRACK` and other opt-out signal conventions (and what Claude Code and Codex do); latency and binary-size delta measurement; prior art among comparable CLIs (cargo, uv, Homebrew, gh, Bun, Next.js); an egress ruling in the threat model.

## Recommendation

None by design. This is a codebase recon; the options above are evidence, not a pick.

## Sources

| Source | Type | Date | Relevance |
|---|---|---|---|
| `src/main.rs`, `src/app.rs`, `src/log_switch.rs`, `src/env.rs`, `src/tls.rs` | Code | read 2026-10-22 | tracing init, lifecycle, env, TLS |
| `src/command/hook.rs`, `src/command/hook/run.rs`, `src/command/hook/pipeline.rs`, `src/install/hook_launcher.rs` | Code | read 2026-10-22 | hook hot path, import bans, handler env |
| `Cargo.toml`, `Cargo.lock`, `deny.toml`, `cargo tree` output | Config / tool | run 2026-10-22 | dependency facts |
| `~/.cargo/registry/src/*/reqwest-0.13.5`, `hyper-util-0.1.20` | Dependency source | read 2026-10-22 | proxy-from-env behaviour |
| `.agents/hook_dispatch_latency.md`, `.agents/adr/adr_hooks_support.md:1682` | Internal record | read 2026-10-22 | latency numbers, no budget |
| `.claude/rules/rust-quality/docs-and-tracing.md` (OBS-12..26), `security.md` (SEC-14/16/18/24), `.claude/rules/arch-threat-model.md` | Internal rules | read 2026-10-22 | policy constraints |
| `docs/src/content/docs/stability.md`, `configuration.md` | User docs | read 2026-10-22 | frozen contract, env docs |
| [grimoire-rs/grimoire#83](https://github.com/grimoire-rs/grimoire/issues/83), [#206](https://github.com/grimoire-rs/grimoire/issues/206) | Issues | read via `gh` 2026-10-22 | requested scope; #206 empty |
| `.agents/adr/adr_vendor_support_tiers.md:290` | Internal record | read 2026-10-22 | only prior telemetry mention |
