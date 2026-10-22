# Plan: Primitive OTel usage telemetry

## Status

- **Plan:** plan_otel_usage_telemetry
- **Active phase:** 4 — Review
- **Step:** awaiting /finalize
- **Last update:** 2026-10-22 (after 6b2c2b6b: style: format the restored scope lookups)
- State:   review
- Tier:    high
- Tier-grammar: 5
- Updated: 2026-10-22
- Reviewed: 6b2c2b6b
- Next:    /hex-finalize

---

## Header

- **Goal contract (binding):** `.agents/goals/otel-usage-telemetry.md` — DoD R1–R10 (+ PR merge-ready, CI green, deep verify in the DONE block only).
- **Source:** `.agents/discussions/otel-usage-telemetry.md` (ratified 2026-10-22). Every Decision there is frozen; this plan does not re-open them.
- **ADR (Accepted, written by this run):** [`adr_otel_usage_telemetry.md`](../adr/adr_otel_usage_telemetry.md) — OBS-12 exemption and its eight conditions.
- **Research:** `.agents/research/research_otel_*.md` (8 files, not repeated). This run's Discover added an API probe of opentelemetry-rust 0.33.1 (findings in the ADR § Technical Details and § Decisions recorded below).
- **Tracking:** [grimoire-rs/grimoire#206](https://github.com/grimoire-rs/grimoire/issues/206); overlaps [grimoire-rs/grimoire#83](https://github.com/grimoire-rs/grimoire/issues/83).
- **Classification:** scope medium (1–2 weeks) · reversibility one-way (medium): new dependency family, new env vars, new `$GRIM_HOME` file, new egress path — all listed Unstable until 1.0 · tier high (`architect=inline`: the owner ratified every design decision; `research=skip`: 8 persisted artifacts; `adversary=on`: new egress and credential-bearing headers).
- **Branch:** `hex/otel-usage-telemetry` (worktree `grimoire-duo`).

## Objective

`GRIM_TELEMETRY=1` plus the standard `OTEL_EXPORTER_OTLP_*` variables makes
every CLI invocation send one OTLP/HTTP protobuf request to `/v1/logs`
holding one `grim.command` event and one `grim.artifact.changed` event per
changed artifact. Without `GRIM_TELEMETRY`, grim makes no telemetry call and
writes no telemetry file. Export never changes an exit code and never prints
at the default log level.

### In scope

`src/telemetry/` (new), the `Context` seam, `app::run` export point,
`src/env.rs` accessors, record calls in `scope_resolution`, `install`,
`update`, `add`; acceptance tests against a local capture server; docs
(new `telemetry.md`, configuration env table, stability Unstable entries,
AGENTS.md env table, grim-usage reference row, file-structure rule); the
ADR and its size record.

### Out of scope (discussion › Intent)

Default or maintainer collector; crash reporting; per-tool-call export from
`hook run`/`mcp`; traces and metrics signals; gRPC; any `grimoire.toml`
form; user/host identity, branch, commit, author; events from `remove`,
`uninstall`, `lock`, `rate`, `release`, `publish` (additive later);
`OTEL_EXPORTER_OTLP_CERTIFICATE`/`_CLIENT_*` and gzip compression
(documented as unsupported).

## Design guarantees

### Backwards compatibility (Principle 9 gate)

Additive only; no Constitution Deviations.

| Surface | Change | Proof |
|---|---|---|
| Every command's stdout, `--format json` output, schemas | none | no file under `src/api/` or `schema/` changes; existing suite passes with zero expectation edits |
| Exit codes, `ErrorReason`, `ExitCode::slug` | none | C-015 tests; `exit_code.rs` / `error.rs` tables untouched |
| Stderr at default log level | none, armed or not, collector up or down; the one exception is a Rust panic message from a bug in telemetry code (exit code still unchanged) | C-015 compares stderr to an unarmed baseline |
| `org.opencontainers.image.source` annotation | none: `normalize_remote_url` is not edited; telemetry wraps it | **control:** `git diff main -- src/oci/git_provenance.rs src/oci/annotations.rs` shows no non-test line (review gate); C-011 characterization test, written first, green before and after |
| Documented env set (`stability.md:44`, frozen) | + `GRIM_TELEMETRY`, `GRIM_TELEMETRY_REPOSITORY`, explicitly carved out as Unstable until 1.0 | C-022 |
| `$GRIM_HOME` layout (`subsystem-file-structure.md`) | + `telemetry/installation-id`, Unstable, created only on armed runs | C-007, C-022 |
| Hook runtime guarantees (I3, the hook plan's import-ban and B1 order-pin tests in `src/command/hook.rs`) | none | C-012 pin + acceptance |
| Event names, attribute keys, resource keys | new, all Unstable until 1.0 | `stability.md` bullet (C-022) |

Migration: no layout move, no state, no reaper, no upgrade fixture. An
unarmed run is byte-identical in behaviour to `main`.

### TDD approach

Every pipeline step runs **Specify → Implement → Self-check**:

1. **Specify** — write the unit and acceptance tests named in the step's
   contracts from this plan, not from code. Run them; record the red run
   (failure for the stated reason, not a compile error elsewhere).
2. **Implement** — until the step's tests and the existing suite pass.
3. **Self-check** — `task rust:verify` (Rust steps) or `task docs:check`
   (docs steps), targeted `pytest`, and **one empirical mutation per
   contract**: apply a single-token mutation to real source, show a named
   test fails, revert. Report it.

Acceptance tests rebuild `test/bin/grim` (`cp -f target/release/grim
test/bin/grim` or go through `task`); a stale binary passes negative tests
vacuously, so every "nothing was sent" assertion carries a positive control
in the same test (precedent `test_hook_run_runtime.py:17-26`).
C-011 is characterization: it must pass **before** any change.
Debug output is enabled through `runner.env["GRIM_LOG"] = "debug"` — the
`--log-level` flag has no reader (`src/main.rs:400`). Plan IDs (C-0xx) stay
out of source code and test names (code-docs LNK-01); name tests
descriptively as listed.

### JSON interface

No CLI JSON output changes. The new structured surfaces are not CLI output:

1. **OTLP wire payload** (C-005, C-006, C-016): protobuf
   `ExportLogsServiceRequest`, one `ResourceLogs`, one `ScopeLogs` (scope
   `grim`, version = crate version), records in emit order: `grim.command`
   first, then artifact events in report order.
2. **Debug audit line** (C-009): at `GRIM_LOG=debug`, exactly one stderr line
   from target `grim::telemetry` (every telemetry `tracing` macro passes
   `target: "grim::telemetry"` explicitly; the default would be
   `grimoire::telemetry`) whose message is
   `telemetry payload <json>`, `<json>` a single-line object running to the
   end of the line:

```json
{"resource":{"service.name":"grim","service.version":"0.15.0","os.type":"linux","host.arch":"amd64","grim.installation.id":"3f0c…"},
 "events":[{"name":"grim.command","attributes":{"grim.command.name":"install","grim.command.duration":0.412,"process.exit.code":0,"grim.ci":false,"grim.scope":"project","grim.clients":["claude"],"grim.clients.detected":["claude","codex"]}},
           {"name":"grim.artifact.changed","attributes":{"grim.artifact.ref":"ghcr.io/acme/review:1","grim.artifact.kind":"skill","grim.artifact.action":"installed"}}]}
```

Consumer rules: attribute presence follows the C-005/C-006 tables
exactly (an absent key means "not applicable to this command", never
"false"); string arrays are sorted in `ClientTarget::ALL` order; numbers
are JSON numbers (ints for `process.exit.code`, float seconds for
`grim.command.duration`). The JSON and the wire carry the same keys and
values (C-016 equivalence test). Neither is frozen (Unstable).

### Exit codes

Zero new exit codes. Telemetry has no failure that reaches the exit code.

| Telemetry condition | Exit | Output at default level | At `GRIM_LOG=debug` |
|---|---|---|---|
| Unarmed / disarmed (`OTEL_SDK_DISABLED`, `DO_NOT_TRACK`, offline) | unchanged | none | none |
| No endpoint set, unsupported `OTEL_EXPORTER_OTLP_[LOGS_]PROTOCOL`, unparsable endpoint | unchanged | none | one skip line, payload line still printed |
| Collector refused / 4xx / 5xx / timeout / TLS error | unchanged | none | one failure line (redacted endpoint, no headers) |
| `$GRIM_HOME/telemetry/` unwritable | unchanged | none | one line; export proceeds without `grim.installation.id` |
| `git` missing or failing under `GRIM_TELEMETRY_REPOSITORY` | unchanged | none | one line; no `vcs.*` |
| Ctrl-C, kill | as today | as today | record lost (accepted, discussion) |
| Panic inside telemetry code | unchanged (isolated by the spawned task) | the default panic hook prints one panic message on stderr before isolation catches it; documented in `telemetry.md` § Limits | one line for the `JoinError` |

The `is_stdout_pipe_closed` mapping (`src/main.rs:207-209`, exit 0) is
mirrored in `process.exit.code`.

## Component contracts

Unit tests live in the module's `#[cfg(test)]` block; acceptance tests in
the named `test/tests/test_telemetry_*.py` file. "Armed" below means
C-001 evaluates true.

### Gate and configuration (P1 unless noted)

- **C-001 Arming.** Armed ⇔ `truthy(GRIM_TELEMETRY)` ∧ ¬`truthy(OTEL_SDK_DISABLED)` ∧ ¬`truthy(DO_NOT_TRACK)` ∧ ¬offline (`GRIM_OFFLINE` or `--offline`). `truthy` is `src/env.rs`'s `1|true|yes|on`, case-insensitive. Decided in `Telemetry::from_env` (CW) with no I/O and no OTel type built. Unarmed: no network call, no `$GRIM_HOME/telemetry/` directory, no `git` spawn, no debug payload line, even when every `OTEL_*` variable is set. Tests: CW unit (pure gate fn over a lookup closure, 16-row truth table); P1 acceptance `test_unarmed_sends_nothing_with_ambient_otel` (positive control: same env + `GRIM_TELEMETRY=1` → 1 request), `test_disarm_switches_win[OTEL_SDK_DISABLED|DO_NOT_TRACK|GRIM_OFFLINE|--offline]`.
- **C-002 Endpoint and headers.** `config::from_env(lookup) -> Result<ExportConfig, Skip>`:
  - `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` non-empty → URL used verbatim; headers = `OTEL_EXPORTER_OTLP_LOGS_HEADERS` **only** (generic `_HEADERS` never sent there — the documented deviation).
  - else `OTEL_EXPORTER_OTLP_ENDPOINT` non-empty → base with trailing `/` trimmed + `/v1/logs`; headers = `_LOGS_HEADERS` if set, else `_HEADERS`.
  - else (neither set) → `Skip::Endpoint`: **no request**. There is no default endpoint, not even the spec's `localhost:4318` (discussion › Intent: nothing is sent "unless an operator arms it and points it somewhere"); so generic headers only ever go to the base endpoint.
  - URL must parse with scheme `http`/`https` and carry **no userinfo** (reqwest would turn it into an `Authorization: Basic` header outside the header rule; credentials belong in the headers variables), else `Skip::Endpoint`.
  - Header list: `k=v` pairs split on `,` then on the first `=`, trimmed, value percent-decoded; a malformed pair is dropped (debug line naming the key only), the rest kept. Header values are typed so no `Debug`/`Display` prints them (OBS-05).
  - Header set on the wire: every header except the transport ones reqwest/hyper add (`host`, `content-length`, `accept`) is in {`content-type: application/x-protobuf`, `user-agent: grim/<version>`} ∪ the selected headers. Grim's `HttpClient` wrapper removes everything else the crate set, including the crate's own `OTEL_EXPORTER_OTLP_[LOGS_]HEADERS` merge (ADR condition 4).
  - Tests: unit table over the endpoint cases × header combinations; acceptance `test_base_endpoint_gets_generic_headers` (asserts the header-set rule above, with `_LOGS_HEADERS` unset), `test_signal_endpoint_never_gets_generic_headers` (sets `_HEADERS=authorization=Bearer%20LEAK`, `_LOGS_ENDPOINT`, `_LOGS_HEADERS=x-ok=1`; asserts `authorization` absent and `x-ok: 1` present — this also proves the crate's env merge is stripped, since the crate would add `_LOGS_HEADERS` itself), `test_no_endpoint_sends_nothing` (armed, no endpoint var, positive control with endpoint), `test_redirect_is_not_followed` (collector answers 307 to another path; that path receives zero requests).
- **C-003 Timeout and protocol.** Timeout = `OTEL_EXPORTER_OTLP_LOGS_TIMEOUT`, else `OTEL_EXPORTER_OTLP_TIMEOUT` (integer ms > 0), else 500 ms; a non-integer or ≤ 0 value falls back to the default. The reqwest client gets connect and total timeouts equal to it, `redirect(Policy::none())`, roots via `crate::tls::merge_embedded_roots`; the export future is also wrapped in `tokio::time::timeout(timeout)`. Retries disabled. Protocol = `_LOGS_PROTOCOL`, else `_PROTOCOL`; unset or `http/protobuf` proceeds, anything else → `Skip::Protocol` (no request). `OTEL_EXPORTER_OTLP_[LOGS_]COMPRESSION=gzip|none` sends an uncompressed body (the crate's gzip feature is off); any other value may make the crate's builder fail, which is a debug-logged export failure like any other. Tests: unit; acceptance `test_unsupported_protocol_sends_nothing[grpc|http/json]` with positive control, also asserting at `GRIM_LOG=debug` the payload line plus one skip line; `test_compression_gzip_is_ignored` (request still arrives, no `content-encoding`); `test_no_retry_on_503` (503 collector, `OTEL_EXPORTER_OTLP_TIMEOUT=3000`, exactly one request — ADR condition 5; the outer timeout cannot mask retries at that budget). Residual (documented in `telemetry.md`): a hostname endpoint whose DNS resolver hangs can exceed the timeout, because the runtime drop waits for the blocking resolver thread.
- **C-004 Resource.** Built from `Resource::builder_empty()` only. Keys, in merge order (later wins):
  1. `service.name="grim"`, `service.version=env!("CARGO_PKG_VERSION")`, `os.type` (from `std::env::consts::OS`: `macos→darwin`, `dragonfly→dragonflybsd`, else as-is), `host.arch` (from `ARCH`: `x86_64→amd64`, `aarch64→arm64`, `arm→arm32`, `powerpc64→ppc64`, else as-is), `grim.installation.id` (C-007, omitted when unavailable), `vcs.*` (C-008, when present);
  2. every pair of `OTEL_RESOURCE_ATTRIBUTES` (same `k=v,…` grammar, percent-decoded, malformed pairs dropped) — so an operator `vcs.*` overrides the derived one;
  3. `service.name` from `OTEL_SERVICE_NAME` when non-empty (wins over 1 and 2, per the OTel spec).
  No `telemetry.sdk.*`, `host.name`, `process.*`, `user.*`. The os/arch maps are pure fns in `payload.rs` (CW, unit-tested there); env parsing is `config::resource_overrides(lookup)` (P1), applied by `finish` before the debug line (C-009 order). Tests: unit; acceptance `test_resource_attributes_exact` (exact key set), `test_otel_resource_attributes_and_service_name_override`.
- **C-007 Installation ID** (`identity.rs`). `installation_id(grim_home) -> Option<String>`: reads `$GRIM_HOME/telemetry/installation-id`; a value that parses as a lowercase hyphenated UUID (surrounding whitespace ignored) is returned; otherwise a new `uuid::Uuid::new_v4()` is written atomically (reuse `src/store/atomic_write.rs`, run under `tokio::task::spawn_blocking` — no blocking fs on the async path) and returned; any I/O error → `None` (debug line). A non-absolute `GRIM_HOME` (e.g. `.grimoire` when `$HOME` is unset) → `None` without writing, so the file can never land in the cwd or a repo. A shared `GRIM_HOME` volume gives several machines one ID (documented in `telemetry.md#installation-id`). Called only on armed runs, before config resolution, so an armed run with no or a bad endpoint still creates it; never on unarmed runs. Concurrent first runs may each write once; last rename wins (accepted). Tests: unit (missing → created and its version nibble is 4, valid → reused, garbage → replaced, read-only dir → None); acceptance `test_installation_id_absent_when_unarmed`, `test_installation_id_stable_across_armed_runs` (same value in two captures, file content equals it).
- **C-009 Debug audit line and `finish` order** (orchestration CW; wire equivalence P1). Format per § JSON interface. Never contains a header name or value, nor the endpoint's userinfo or query (endpoint, if logged, goes through `redact::url`). `Telemetry::finish` runs exactly this sequence, and the ADR diagram mirrors it:
  `armed? (else return) → identity::installation_id → vcs::derive (only if repo opt-in ∧ scope = project ∧ workspace recorded) → config::resource_overrides → payload::build → debug line → config::from_env (Skip → debug line, return) → export::send under timeout (error → debug line)`.
  Isolation: everything after the `armed?` check runs in one `tokio::spawn`ed task awaited by `finish`; a `JoinError` (panic inside telemetry) becomes a debug line, never a changed exit code. Wall-time budget of that task: export timeout + 2 × 1 s `git` bound (repo opt-in only) + identity I/O; a hanging DNS resolver is the documented residual.
  Tests: P1 acceptance `test_debug_line_matches_wire` (decode the capture, compare to the JSON: same resource and event attributes), `test_debug_line_never_carries_header_values`.
- **C-015 Failure isolation.** For a refused port, a 503 response, and a collector that accepts and never answers: exit code equals the unarmed baseline's; stderr at default level equals the baseline's; added wall time < timeout + 1.5 s slack (CI margin, precedent `test_hook_run_runtime.py:1019-1031`), with `OTEL_EXPORTER_OTLP_TIMEOUT=300`. Every `runner` call in these tests passes `timeout=10` (the kwarg CW adds to `GrimRunner.run`) so a hang fails rather than stalls. Tests: P1 acceptance `test_export_failure_never_changes_outcome[refused|503|hang]`, plus one failing command (`grim install` on a missing config, exit 79) to show `error.type` path also stays unchanged.
- **C-016 One request.** Armed run with a working collector: exactly one `POST /v1/logs`, `content-type: application/x-protobuf`, zero requests to `/v1/traces` or `/v1/metrics`, decoded body has one `ResourceLogs` / one `ScopeLogs` named `grim`. Tests: P1 acceptance `test_one_post_per_invocation`; P5 `test_multi_record_single_post` on an install that changes 2 artifacts (3 records, still 1 POST — the observable proof of ADR condition 3, no crate processor).

### Records and events (P2 unless noted)

- **C-005 `grim.command` event.** Event name `grim.command`, severity INFO, timestamp at finish. Attributes:

  | Key | Type | Presence |
  |---|---|---|
  | `grim.command.name` | string | always (C-014) |
  | `grim.command.subcommand` | string | nested commands only (C-014) |
  | `grim.command.duration` | double, seconds | always (from `start` to `finish`) |
  | `process.exit.code` | int | always (numeric `ExitCode`; pipe-closed → 0) |
  | `error.type` | string | only when exit ≠ 0: `ErrorReason` slug when `classify` yields one, else `ExitCode::slug()` |
  | `grim.ci` | bool | always (`env::ci()`: truthy `CI`, `GITHUB_ACTIONS` or `GITLAB_CI`, names only) |
  | `grim.scope` | string `project`/`global` | when the command resolved its own scope; never for `mcp`/`tui` (rule below) |
  | `grim.clients` | string[] | `install`/`update`/`add` only: selected `ClientTarget` names |
  | `grim.clients.detected` | string[] | same commands: `detect_clients(workspace, scope)` names, computed only when armed |

  Exit-code/`error.type` mapping, attribute assembly **and scope recording** are CW (`payload::build`, unit-tested there; `record_scope(scope, workspace)` called at both `Ok` returns of `scope_resolution::resolve_in`, `scope_resolution.rs:95,113`, so P3 sees a project workspace without P2); clients recording is P2. Scope rule: `resolve_in` records only when its `global` argument equals `ctx.global()` and its `workspace` argument is `None` (the user-requested scope, not an incidental internal resolve such as `export.rs:330`); a lookup that only picks registries goes through `scope_resolution::resolve_untracked`, which never records (`release_default_registry`, `declared_insecure_hosts`) — `fetch`, `describe`, `rate`, `login` and `logout` record their own resolve, and the **first** such resolution wins. Unit test: a global command that also resolves a project scope internally reports `global` and derives no `vcs.*`. Tests: CW unit `build_*`; P2 acceptance via the debug line: `test_command_event_status_project`, `test_command_event_failure_sets_error_type`, `test_command_event_global_scope`, `test_clients_recorded_on_install`.
- **C-006 `grim.artifact.changed` event.** One per changed artifact, after the command event, in report order. Attributes, all always present except where noted: `grim.artifact.kind` (`skill|rule|agent|bundle|mcp|hook`), `grim.artifact.action` (`installed|updated|removed|added`), `grim.artifact.ref` (`registry/repository[:tag]`, built from `Identifier` accessors, never from a raw user string; **omitted** for path sources — no local path ever). Never a digest, never userinfo. Ref sources:
  - `registry/repository` comes from the row's own source: install `ArtifactInstall.reference.source` (`src/install/installer.rs:108-116`, covers bundle members); update `new_lock` for `updated` rows, `previous` lock for `removed` rows; add the parsed `Identifier`.
  - `:tag` is added only when a declared `Identifier` in `scope.set` (or the add argument) has the same registry and repository and carries a tag; otherwise omitted (bundle members, digest-only declarations).
  Emit points (exact):
  - `install::run`: from `outcomes`, immediately before `install::finish(outcomes, &armed)` (`install.rs:134`); `Installed`→`installed`, `Updated`→`updated`. `dev_install` (local path, early return at `install.rs:83`) emits no artifact events.
  - `update::run`: after the refusal loop that calls `mark_refused` (`update.rs:~344-349`) and before each `Ok` return; final rows `Updated` ∧ ¬refused → `updated`, `Removed` → `removed`. The loop's `Err` return emits nothing.
  - `add`: both `AddReport::new` sites — registry (`add.rs:~237`) → `added` with ref; path source (`add.rs:~439`) → `added` without ref. Bundle: one event, kind `bundle`. The install-on-add emits no `installed` events.
  - Unchanged, skipped, refused, errored rows emit nothing. Other commands emit none.
  Tests: P2 unit (record → payload); acceptance `test_install_emits_one_event_per_changed_artifact` (2 artifacts → 2 events; re-run → 0 events, command event still sent), `test_add_emits_added`, `test_update_emits_updated`, `test_path_source_has_no_ref`.
- **C-013 Long-lived commands.** For `grim mcp` and `grim tui`, the handle records nothing during the session: `start` sets a `long_lived` flag in CW's `State` when it sees `mcp`/`tui`, and every `record_*` — CW's `record_scope` included — returns early on it (CW unit test `record_scope_noop_after_start_mcp`; P2's `record_*` reuse the same flag); finish sends one `grim.command` with name, duration, exit code, `grim.ci`, and no `grim.scope`, clients or artifact events. Tests: P2 unit; P5 acceptance `test_mcp_session_sends_one_record` (drive `grim mcp` via `test_mcp.py::_drive` with **`offline=False`** — its default passes `--offline`, which disarms — and a `grim_status` tool call that resolves a scope without network; exactly 1 request, no `grim.scope`).
- **C-014 Command names.** `command_name(Option<&Command>) -> (&'static str, Option<&'static str>)`, an exhaustive `match` (a new variant is a compile error), canonical names (aliases map to the canonical, `hook deny`→`revoke`): top-level `config context init lock install update status build release publish add remove uninstall search rate fetch describe export schema completions tui mcp hook login logout`; subcommand for `config` (`get set unset list registry`), `config registry` → subcommand `registry <add|set|rm|use|show|list|fields>`, `export` (`plugin marketplace`), `hook` (`run list allow revoke`). Bare `grim`, `--help`, `--version`, clap usage errors never reach the handle (no record). Tests: unit table covering every variant.

### Repository context (P3)

- **C-008 `vcs.*`.** Only when armed ∧ `truthy(GRIM_TELEMETRY_REPOSITORY)` ∧ resolved scope = project ∧ a workspace was recorded. `git` runs via `tokio::process::Command` (as `git_provenance.rs:319-341` does) with `kill_on_drop(true)`, each call under `tokio::time::timeout(1 s)`; a timeout means no `vcs.*`. The child env drops the repo-locating and config-injecting variables (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_COMMON_DIR`, `GIT_CONFIG`, `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_SYSTEM`, `GIT_CONFIG_PARAMETERS`, `GIT_CONFIG_COUNT`) and sets `GIT_OPTIONAL_LOCKS=0`, so a run inside a git hook still reads the workspace's repo. Then: `git -C <workspace> rev-parse --show-toplevel`; no repo, git missing, or toplevel equal to the home directory (`env::home_dir`) **compared after `dunce::canonicalize` of both sides** (symlinked macOS homes, Windows separators) → no `vcs.*`. Else `git -C <toplevel> config --local --get remote.origin.url`; no origin → none. Redact with C-010; `None` → none. Attributes (resource): `vcs.repository.url.full` (redacted URL), `vcs.owner.name` (path segments except the last, joined by `/`), `vcs.repository.name` (last segment), `vcs.provider.name` (`github`/`gitlab`/`bitbucket`/`gitea` when the host contains that word, `codeberg.org`→`gitea`; otherwise omitted). Never branch, commit, author, or any other git value. `vcs::derive(workspace, home) -> Option<Vcs>` is called from CW `finish`. Tests: unit (pure parts: owner/name/provider split); acceptance `test_no_vcs_and_no_git_spawn_without_opt_in` (a `git` shim first on `PATH` writes a marker; marker absent; positive control: with opt-in, marker present — POSIX only), `test_vcs_from_credentialed_origin` (origin `https://user:tok@Host/O/R.git?x=1#f` → `vcs.repository.url.full=https://host/o/r`, `vcs.owner.name=o`, `vcs.repository.name=r`, no `tok`, `x=1` or `f` anywhere in the line), `test_no_vcs_for_global_scope`, `test_no_vcs_for_home_repo` (HOME itself is the repo; positive leg: a subdirectory repo under HOME does carry `vcs.*`), `test_no_vcs_without_git_on_path` (PATH without git → no `vcs.*`, stderr at default level unchanged). `test_operator_vcs_override` needs P1's resource overrides, so it lives in P5.
- **C-010 Redaction** (`redact.rs`, OBS-22). `repository_url(raw) -> Option<String>` wraps `normalize_remote_url` without editing it. Order: find the authority (after `scheme://`, up to the first `/` that follows the last `@` before any path), strip userinfo (to that last `@`), then drop `?query` and `#fragment` from the remainder, drop a trailing `/` and `.git`, lowercase, convert scp/ssh forms to `https://host/path`; return `None` for local paths, `file://`, empty input, or any result still containing `@`. `url(raw) -> String` (for the debug endpoint): scheme, host, port and path kept; userinfo, query, fragment removed. Unit vectors (exact):

  | Input | `repository_url` |
  |---|---|
  | `https://user:tok@Host/O/R.git?x=1#f` | `https://host/o/r` |
  | `git@GitHub.com:Acme/Repo.git` | `https://github.com/acme/repo` |
  | `ssh://git@host:2222/o/r.git` | `https://host/o/r` |
  | `https://host/o/r.git/` | `https://host/o/r` |
  | `https://tok@host/o/r?x=a@b` | `https://host/o/r` |
  | `https://user:p#ss@host/o/r` | `https://host/o/r` |
  | `https://host/o/r?X-Amz-Signature=abc&sig=1` | `https://host/o/r` |
  | `https://gitlab.com/group/sub/proj.git` | `https://gitlab.com/group/sub/proj` |
  | `https://host/o/r?x=/SECRET`, `https://host/o/r#f/SECRET` | `https://host/o/r` |
  | `/srv/git/repo.git`, `file:///srv/repo`, ``, `https://host?token=ab/SECRET`, `https://host#frag/SECRET` | `None` |

- **C-011 Release annotation unchanged.** The control is the diff gate in § Backwards compatibility (`git_provenance.rs` and the non-test part of `annotations.rs` unedited). Belt and braces: a characterization unit test in `src/oci/annotations.rs`' test module feeds every non-`None` `normalize_remote_url` vector from `git_provenance.rs` tests through `normalize_remote_url` and then `source_annotation` and asserts the current strings. Written and green **before** any P3 code.

### Hook path (CW + P5)

- **C-012 Hook run never exports.** Unit pin in `src/command/hook.rs` tests (same `code_lines` idiom, test named e.g. `telemetry_is_never_built_before_the_hook_runtime_returns`): `main.rs` code lines contain no `telemetry::`/`Telemetry` (only `mod telemetry;`), and in `app.rs` the first `telemetry` occurrence follows `hook::run::run(` — so `app.rs` must not `use crate::telemetry` at the top; it reaches the handle only through `ctx.telemetry()`. Also add `crate::telemetry` and `Telemetry` to the existing `FORBIDDEN` needle table of the hook-runtime import-ban test (`src/command/hook.rs:154`), so the five runtime files can never reach it. Acceptance (P5) `test_hook_run_never_exports`: armed env + capture server, `grim hook run` (copy `_hook_run` from `test_hook_run_runtime.py:143-170`) → zero requests, no `$GRIM_HOME/telemetry/`, **and the hook handler's marker file exists** (proves the run reached dispatch rather than exiting early); positive control `grim status` in the same test → one request. POSIX only, like its precedent.

### Contract-level (P5)

- **C-017 Never collected.** On wire captures from an install in a git repo with branch `sentinel-branch`, author `Sentinel Author`, a `--config <path>/SENTINEL-CFG/grimoire.toml` arg, and `GRIM_TELEMETRY_REPOSITORY=1`: no decoded string value contains the cwd, the `GRIM_HOME` path, `SENTINEL`, `sentinel-branch`, `Sentinel Author` or the HEAD SHA; and no decoded value **equals** the hostname (`socket.gethostname()`) or the username (`getpass.getuser()`) (whole-value comparison: substring matching false-positives on short names like `ci`). Test `test_never_collected`.
- **C-018 Field table.** Every attribute key on any captured resource or event appears in the field table of `docs/src/content/docs/telemetry.md` (`{#fields}` section), and every key the table marks "always" appears in a capture. Test `test_wire_keys_match_documented_field_table`; the same module asserts `stability.md` names both switches and `$GRIM_HOME/telemetry/installation-id` in its Unstable section (C-022 check).

### Docs (P4)

- **C-019 `telemetry.md`** (`<!-- doc_type: reference -->`, sidebar Reference group): what is sent and when; `{#enable}` env table (`GRIM_TELEMETRY`, `GRIM_TELEMETRY_REPOSITORY`, honoured `OTEL_*` set incl. defaults: 500 ms timeout, no default endpoint (an endpoint variable is required), the header-scoping deviation, protocol rule, unsupported compression/certificate vars, `OTEL_SDK_DISABLED`, `DO_NOT_TRACK`, offline, env proxies honoured, the DNS-hang residual, endpoint userinfo refused, and a plain warning that an environment which sets `GRIM_TELEMETRY` and the base endpoint receives the ambient `OTEL_EXPORTER_OTLP_HEADERS` — so set headers only in shells you control and prefer `https://`); `{#fields}` field table (resource, `grim.command`, `grim.artifact.changed`: key, type, presence, example) matching C-004–C-006/C-008 exactly; `{#never-collected}` list (R8); `{#installation-id}`; `{#audit}` (`GRIM_LOG=debug` line); `{#collector-recipe}` — an `otelcol-contrib` config deriving `grim.invocations`, `grim.errors`, `grim.artifact.changes` (`count` connector) and `grim.invocation.duration` (`signaltometrics`), with the alpha-maturity caveat and a no-run marker per docs-quality; stability note linking `stability.md#unstable`. Passes `task docs:check` (sidebar coverage DOC-URL-06, declaration).
- **C-020 Configuration and AGENTS.** `configuration.md` env table rows for `GRIM_TELEMETRY` and `GRIM_TELEMETRY_REPOSITORY` (link to `telemetry.md#enable`); `AGENTS.md` env table rows in its column format; `catalog/skills/grim-usage/references/registries.md` env table row(s) + drift review of `SKILL.md` and `references/troubleshooting.md` (record "no change needed" or the edit).
- **C-022 Stability and layout.** `stability.md` Unstable section: a **Telemetry** bullet (both switches, event names, attribute and resource keys, the debug line, `$GRIM_HOME/telemetry/installation-id`) next to the NDJSON bullet; the frozen Environment-variables row (`stability.md:44`) gains "except the telemetry switches, Unstable until 1.0". `.claude/rules/subsystem-file-structure.md` lists `$GRIM_HOME/telemetry/installation-id` (Unstable, armed runs only).

### Env accessors and exemption pins (CW)

- **C-021 `src/env.rs`.** `telemetry()`, `telemetry_repository()`, `otel_sdk_disabled()`, `do_not_track()`, `ci()` — all `truthy`-based bools in the `offline()` shape. Gate and config logic take a lookup closure so unit tests never mutate the process env (SEC-05). Tests: pure helper tests in `env.rs`.
- **C-023 Exemption pins.** Unit test over `include_str!` of `Cargo.toml` (CW): the four `opentelemetry*` entries are `=0.33.1` with `default-features = false`, and none enables `reqwest-client`, `reqwest-blocking-client`, `grpc-tonic`, `internal-logs` (ADR condition 1). Condition 3 is proven by `test_multi_record_single_post`, condition 5's redirect rule by `test_redirect_is_not_followed`; condition 8 is a recorded measurement (Phase V).

## User-experience scenarios

- **S-001 Arm and observe.** Operator exports `GRIM_TELEMETRY=1`, `OTEL_EXPORTER_OTLP_ENDPOINT=https://otel.corp:4318`, `OTEL_EXPORTER_OTLP_HEADERS=authorization=Bearer …`, runs `grim install` → output unchanged; collector receives one request with one `grim.command` and one event per changed artifact. Errors: collector down → S-004.
- **S-002 Ambient `OTEL_*`, no switch.** Developer shell carries `OTEL_*` for another tool; `grim status` → no request, no file under `$GRIM_HOME/telemetry/`. Error cases: none.
- **S-003 Opt-out.** `DO_NOT_TRACK=1` or `OTEL_SDK_DISABLED=true` or `--offline` with `GRIM_TELEMETRY=1` → nothing sent. Error cases: none.
- **S-004 Dead collector.** Refused, 503 or hanging collector → same exit code and stderr as unarmed; at most the export timeout added (plus ≤ 2 s of bounded `git` calls with repo context on). Error case: operator sets a 10 s timeout → up to 10 s added (documented).
- **S-005 Signal endpoint.** Repo env sets `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT`; user's ambient `OTEL_EXPORTER_OTLP_HEADERS` is not sent there.
- **S-006 Audit.** `GRIM_LOG=debug grim status` → one `telemetry payload {…}` line, no header values. Error case: invalid protocol → payload line plus one skip line, no request.
- **S-007 Repository context.** `GRIM_TELEMETRY_REPOSITORY=1` in a project repo → `vcs.*` with the redacted origin; at global scope or a home-directory repo → no `vcs.*`. Error case: no `git` on PATH → no `vcs.*`, nothing printed.
- **S-008 Hooks and MCP.** Client-triggered `grim hook run` never exports; a `grim mcp` session sends one record at its end.
- **S-009 Failure command.** `grim install` with no config, armed → exit 79 as before; event carries `process.exit.code=79`, `error.type=no-config`.
- **S-010 Install fleet view.** Two artifacts installed → two `grim.artifact.changed` events; immediate re-run → none, command event still sent.

## Error taxonomy

Internal only, never surfaced: `config::Skip { Endpoint, Protocol }`,
`export::ExportError { Network, Status(u16), Timeout, Build }`,
identity `io::Error`, vcs spawn failure. Every one ends in a single
`tracing::debug!` with target `grim::telemetry`, with the endpoint passed
through `redact::url`. Remediation lives in `telemetry.md`.

## Edge cases

Empty `GRIM_TELEMETRY=""` → unarmed; `GRIM_TELEMETRY=0`/`false` → unarmed;
`OTEL_EXPORTER_OTLP_ENDPOINT` with a path (`https://h/otlp/`) → `https://h/otlp/v1/logs`;
headers with `=` in the value (`k=a=b`) → split on the first `=`;
duplicate header keys → last wins; header names are case-insensitive;
installation-id file with CRLF → accepted; `GRIM_HOME` on a read-only mount →
no id, export proceeds; bundle install → one event per changed member;
`grim install` where every row is unchanged → command event only;
pipe-closed stdout → `process.exit.code=0`; TUI non-TTY early exit → one
command event; Windows: `os.type=windows`, no POSIX-only test runs there.

## Trade-off analysis

The exporter trade-off is the ADR's (Option 2 chosen by the owner). This
plan's own choice is the **seam**:

| Criterion (weight) | A: `Context` field + export in `app::run` (chosen) | B: process-global `OnceLock` + export in `main.rs` |
|---|---|---|
| Hook guarantee by construction (5) | 5 — `Context::new` runs after the `Hook(Run)` return, already pinned (B1) | 2 — needs a new pin; `main.rs` runs for hook run too |
| `main.rs` structural pins (3) | 5 — `main.rs` gains only `mod telemetry;` | 2 — edits next to pinned strings |
| Testability / ARCH-13 (3) | 5 — injectable, no global | 2 — a global tests must vary |
| Diff size (2) | 4 — 5 one-line `Context` edits + one wrapped `match` | 5 |
| Covers usage errors / `--help` (1) | 1 | 3 |
| **Weighted** | **64** | **35** |

Reversibility: two-way (internal seam). Risk of A: a future command that
spawns a long-lived task holding a `Context` clone could record after
`finish`; mitigated by C-013's no-op rule and `finish` consuming the state.

## Parallelization

Contract wave first (one commit), then four parallel pipelines, then one
contract pipeline.

| Pipeline | Scope | Expected files | Wave | Depends on | Marks | Status |
|---|---|---|---|---|---|---|
| CW | C-001 (gate), C-004 (os/arch maps), C-005 (assembly + scope recording), C-013 (`long_lived` flag + `record_scope` no-op), C-009 (orchestration + order), C-012 (unit pin), C-021, C-023; stubs for every other contract | `Cargo.toml`, `Cargo.lock`, `src/main.rs` (`mod telemetry;` only), `src/context.rs`, `src/app.rs`, `src/env.rs`, `src/command/scope_resolution.rs`, `src/telemetry.rs` + `src/telemetry/payload.rs` (real), `src/telemetry/scope.rs` (real: `record_scope`), `src/telemetry/{config,export,identity,record,command_name,vcs,redact}.rs` (stubs), `src/command/hook.rs` (test module), `test/src/otlp_fixture.py`, `test/src/runner.py` (`timeout` kwarg), `test/tests/conftest.py`, `test/pyproject.toml`, `test/uv.lock`, `test/tests/test_otlp_fixture.py` (fixture self-test) | 0 | — | — | merged |
| P1 | C-001 (acceptance), C-002, C-003, C-004, C-007, C-009 (wire), C-015, C-016; S-001–S-006 | `src/telemetry/{config,export,identity}.rs`, `test/tests/test_telemetry_export.py` | 1 | CW | review | merged |
| P2 | C-005 (clients), C-006, C-013 (unit), C-014; S-009, S-010 | `src/telemetry/{record,command_name}.rs`, `src/command/{install,update,add}.rs`, `test/tests/test_telemetry_events.py` | 1 | CW | — | merged |
| P3 | C-008, C-010, C-011; S-007 | `src/telemetry/{vcs,redact}.rs`, `src/oci/annotations.rs` (test module only), `test/tests/test_telemetry_repository.py` | 1 | CW | review | merged |
| P4 | C-019, C-020, C-022 | `docs/src/content/docs/{telemetry,configuration,stability}.md`, `docs/astro.config.mjs`, `AGENTS.md`, `catalog/skills/grim-usage/**`, `.claude/rules/subsystem-file-structure.md` | 1 | CW | — | merged |
| P5 | C-008 (operator override), C-012 (acceptance), C-013 (acceptance), C-016 (multi-record), C-017, C-018, C-022 (check); S-008; verification records | `test/tests/test_telemetry_contract.py`, `.agents/adr/adr_otel_usage_telemetry.md` (§ Quantified Impact), this plan (§ Verification record) | 2 | P1, P2, P3, P4 | — | active |

Hub files: `Cargo.lock` and `test/uv.lock` belong to CW only; no pipeline
adds a dependency. Observation channel: P2 and P3 acceptance tests read the
C-009 debug line (real from CW on, since `finish` builds and logs the payload
before calling the P1 stub), so they do not wait for P1's transport; P1 owns
the wire; P5 re-checks the joined result on the wire. Scope recording is CW
so P3 sees a project workspace without P2. Tests that need two wave-1
pipelines (`test_operator_vcs_override`: P1 + P3) live in P5.

Steps (each a fresh agent, Specify → Implement → Self-check):

- **CW** — 1) `Cargo.toml` (exact pins per ADR condition 1, `uuid` direct with `v4`) + the C-023 pin test, `src/env.rs` C-021, `src/telemetry/payload.rs` (types, key constants — one place, OBS-20 — `build()`, os/arch maps, exit mapping) with unit tests, `src/telemetry.rs` (named module file — the repo has no `mod.rs`, `arch-principles.md` › Code Style; `Telemetry` handle: `Arc<Mutex<State>>`, `from_env(offline)`, `start`, `finish(&anyhow::Result<ExitCode>)` in the C-009 order), `src/telemetry/scope.rs` (`record_scope`, real, called at `scope_resolution.rs:95,113`, no-op once `start` set `long_lived` — C-013), stubs (`todo!()`-free: `None`/no-op/`Err(Skip::Endpoint)`; the `redact::url` stub returns the constant `"<redacted>"`, never its input), `Context` field + `telemetry()` accessor (hand `Debug`/`Clone`, 3 test ctors), `app.rs` (`start` after `Context::new`; dispatch `match` wrapped as `let outcome = async { Ok(match …) }.await;` then `ctx.telemetry().finish(&outcome).await; outcome` — no top-level `use crate::telemetry`), `mod telemetry;`, C-012 unit pin. Gate: `task rust:verify`, existing pytest green. 2) `test/src/otlp_fixture.py` (ThreadingHTTPServer on port 0 per `test/src/index_fixture.py:125-138`, with `daemon_threads = True` and `block_on_close = False`, and `hang` waiting on a `threading.Event` that teardown sets; records method, path, headers, raw body, decoded `ExportLogsServiceRequest` via `opentelemetry-proto`; modes `ok`, `status=503`, `hang`, `redirect`; `closed_port()` helper; debug-line parser), `otlp_collector` fixture in `test/tests/conftest.py`, `timeout: float | None = None` kwarg on `GrimRunner.run` (passed to `subprocess.run`), `uv add --group dev opentelemetry-proto`. Gate: fixture self-test (POST a hand-built request, decode it; `hang` teardown returns).
- **P1** — 1) `config.rs` (C-002, C-003, resource overrides of C-004) unit-first. 2) `identity.rs` (C-007) unit-first. 3) `export.rs`: `GrimHttpClient` (header strip + insert, response body discarded), exporter per ADR § Exporter glue, conversion `Payload → SdkLogRecord`s via a collect-only processor and one `LogBatch`; acceptance `test_telemetry_export.py`. Mark `review`: credential headers and egress get the security perspective.
- **P2** — 1) `command_name.rs` (C-014) + `record.rs` (`record_clients`, `record_artifact`, C-013 no-op rule) unit-first. 2) Call sites (exact, per C-006): `install.rs:94` (clients; not the `dev_install` parse at `:339`) and immediately before `install::finish(outcomes, &armed)` at `:134` (artifacts); `update.rs:145` (clients; not the reap-set parse at `:253`) and after the refusal loop before each `Ok` return (artifacts); `add.rs:495` (clients) and both `AddReport::new` sites (`:~237`, `:~439`); acceptance `test_telemetry_events.py`. Every record call is a no-op when unarmed (no detection, no allocation beyond the call).
- **P3** — 1) C-011 characterization test (green on unmodified code, commit it first), then `redact.rs` (C-010) unit-first. 2) `vcs.rs` (C-008): `tokio::process::Command` with `-C`, `kill_on_drop(true)`, 1 s timeout per call, `dunce::canonicalize` home comparison; acceptance `test_telemetry_repository.py`. Mark `review`: URL redaction is a credential boundary.
- **P4** — 1) `telemetry.md` + sidebar entry. 2) configuration, stability, AGENTS.md, grim-usage, file-structure rule. Write from this plan's contracts **and re-check against the merged code before P5 closes** (docs written from a drifted plan is a recorded failure mode, hex.md › Convergence-round learnings).
- **P5** — 1) `test_telemetry_contract.py` (C-008 `test_operator_vcs_override`, C-012, C-013, C-016 `test_multi_record_single_post`, C-017, C-018, C-022 check). 2) Verification record (below).

```mermaid
graph TD
  subgraph W0[Wave 0]
    CW[CW contract wave]
  end
  subgraph W1[Wave 1]
    P1[P1 export core]
    P2[P2 events]
    P3[P3 repository]
    P4[P4 docs]
  end
  subgraph W2[Wave 2]
    P5[P5 contract tests + verification]
  end
  CW --> P1 & P2 & P3 & P4
  P1 & P2 & P3 & P4 --> P5
```

**Critical path:** CW → P1 → P5 (P1 is the largest pipeline).
**Shippable after wave: 1** — all requirements implemented and individually tested; wave 2 adds the cross-pipeline proofs and the verification record that the goal's DoD needs.
Parallelism equals what file-disjointness allows (four wave-1 pipelines).
Heavy gates run one at a time (hex.md: parallel `task --force verify` OOM'd WSL); light pipelines (P4) may use worktrees, cargo-heavy ones share the warm `target/` per hex.md › disk note.

## Verification (Phase V, run by /hex-execute after P5, recorded in § Verification record)

1. `task --force verify` green (never plain `task verify` — cache).
2. `task docs:check` green (Node 24).
3. `task rust:license:deps` and `task rust:audit` green on the new graph; `cargo tree -e features -i opentelemetry-otlp` and `-i opentelemetry_sdk` show none of `reqwest-client`, `reqwest-blocking-client`, `grpc-tonic`, `internal-logs` enabled through unification (C-023 reads only `Cargo.toml`).
4. Binary size: the shipped `--profile dist` build (thin LTO + strip, `Cargo.toml` `[profile.dist]`) and `release`, on `main`'s merge base vs branch tip → ADR § Quantified Impact (both rows).
5. Collector interop: `otel/opentelemetry-collector-contrib` (exact tag pinned in the record) with the `telemetry.md` recipe plus a `debug` exporter; run `grim status` and an install armed against it; record the debug-exporter output showing both event names and the derived `grim.invocations` count. If `docker` is unavailable, record "not run" with the reason — never claim it.
6. Windows: `/mnt/c/Users/ecom/grim-wintest/run.sh /home/mherwig/dev/grimoire-duo -k telemetry`; compare against `origin/main` before blaming the branch.
7. Catalog drift review (`grim-usage`) recorded with its outcome; `task catalog:verify` green.

## Goal DoD trace (ticks happen between hex-mode runs, with this evidence)

| DoD | Contracts | Evidence (tests) |
|---|---|---|
| R1 env-only opt-in, disarm switches | C-001, C-002, C-003, C-021 | `test_disarm_switches_win[*]`, `test_base_endpoint_gets_generic_headers`, `test_signal_endpoint_never_gets_generic_headers`, `test_no_endpoint_sends_nothing`, CW gate truth table |
| R2 no export without `GRIM_TELEMETRY` | C-001 | `test_unarmed_sends_nothing_with_ambient_otel` (+ positive control) |
| R3 CLI only; hook run never; mcp one session record | C-012, C-013, C-014 | `test_hook_run_never_exports`, unit pin in `hook.rs`, `test_mcp_session_sends_one_record` |
| R4 events only, one POST | C-005, C-006, C-016 | `test_one_post_per_invocation`, P5 multi-record test, `test_install_emits_one_event_per_changed_artifact` |
| R5 resource set | C-004, C-007 | `test_resource_attributes_exact`, `test_otel_resource_attributes_and_service_name_override` |
| R6 uuid v4 under `$GRIM_HOME`, armed only | C-007 | `test_installation_id_absent_when_unarmed`, `test_installation_id_stable_across_armed_runs`, identity unit tests |
| R7 `vcs.*` opt-in, redacted, project only, no home repo | C-008, C-010 | `test_no_vcs_and_no_git_spawn_without_opt_in`, `test_vcs_from_credentialed_origin`, `test_no_vcs_for_global_scope`, `test_no_vcs_for_home_repo`, `test_operator_vcs_override` |
| R8 never collected | C-017 | `test_never_collected` |
| R9 redaction, semconv names, no raw args/paths | C-006, C-010, C-011, C-017, C-018 | redact unit vectors, `test_path_source_has_no_ref`, C-011 characterization + diff gate, `test_never_collected`, `test_wire_keys_match_documented_field_table` |
| R10 failure never changes exit code or default output | C-015 | `test_export_failure_never_changes_outcome[refused|503|hang]` |

## Decisions recorded by the orchestrator (goal § Issue resolution)

Each: question → research → decision. The discussion's eight open
questions take their Recommended answer unless noted.

1. **Headers per endpoint?** → discussion recommendation + probe (the crate merges env headers unconditionally) → C-002, enforced by grim's own `HttpClient` that strips crate headers.
2. **Export timeout default?** → recommendation; probe showed the exporter's `with_timeout` is only a retry deadline → 500 ms default enforced by the reqwest client timeouts plus `tokio::time::timeout` (C-003).
3. **Payload audit?** → recommendation (`GRIM_LOG=debug`, field table) → C-009; the line is also the P2/P3 observation channel.
4. **How are events emitted?** → recommendation (`Logger::create_log_record`/`set_event_name`/`emit`); probe proved the crate's Simple processor deadlocks on a current-thread runtime and posts per record, the Batch processor panics with async reqwest → collect-only processor + one `LogExporter::export` call (ADR condition 3).
5. **Init vs hook-run return?** → recommendation; Discover found `Context::new` already sits after the return, pinned by B1 → handle lives in `Context`, plus the C-012 pin.
6. **Non-`http/protobuf` protocol?** → recommendation: skip, debug line (C-003).
7. **Where is `normalize_remote_url` hardened?** → recommendation: telemetry-side wrapper (C-010), shared fn untouched, C-011 pins the annotation.
8. **Installation ID location?** → recommendation: `$GRIM_HOME/telemetry/installation-id`, atomic write. Refinement: created on every **armed** run (before endpoint validation) so the audit line always shows it; never on an unarmed run (C-007).
9. **Does offline mode suppress export?** (not in the discussion) → `GRIM_OFFLINE` is documented as "disable all network access" (`src/env.rs:14-15`, configuration env table) → yes, offline disarms (C-001).
10. **Endpoint when only `GRIM_TELEMETRY` is set?** → first draft used the spec default `http://localhost:4318`; spec review showed it contradicts the discussion's Intent ("nothing … unless an operator arms it and points it somewhere") and would hand ambient `OTEL_EXPORTER_OTLP_HEADERS` to whatever listens on loopback 4318 (T5 on a shared machine) → **no default endpoint**: no endpoint variable → no request (C-002).
11. **`mcp`/`tui` scope and artifacts?** → recon: both swap scope per call/live (`tui.rs:91,107`), and the discussion bans per-tool-call records → session event only (C-013).
12. **Which commands emit artifact events?** → discussion "resolving commands" + recon: only install/update/add hold a declared ref → those three (C-006); remove/uninstall additive later.
13. **Test decoder: `opentelemetry-proto` or a hand varint parser?** → Discover: hand parser ~60–80 lines plus a golden blob; `opentelemetry-proto` decodes exactly and resolves from the uv cache and PyPI (CI already needs PyPI) → `opentelemetry-proto` as a test-only dev dependency.
14. **Operator override of grim's own resource keys?** → OTel spec merge order (user-provided last) → `OTEL_RESOURCE_ATTRIBUTES` wins over every derived key, `OTEL_SERVICE_NAME` wins over both (C-004); documented.
15. **Duration unit?** → semconv durations are seconds as double → `grim.command.duration` double seconds.
16. **Seam: `Context` vs global?** → § Trade-off analysis → `Context`.
17. **CI detection: reuse `catalog::forge::CiEnv` (discussion › Touches) or a new accessor?** → recon: `CiEnv::from_env` loads `GH_TOKEN`/`GITLAB_TOKEN`/`GRIM_ANNOUNCE_TOKEN` into a `#[derive(Debug)]` struct (`forge.rs:46-92`) → a names-only `env::ci()` (C-021); loading credentials to compute a bool is the wrong trade.
19. **Can a repo-supplied environment harvest ambient generic headers?** (adversary A-1, Block as raised) → yes: a repo that sets `GRIM_TELEMETRY=1` plus the base `OTEL_EXPORTER_OTLP_ENDPOINT` receives the shell's `OTEL_EXPORTER_OTLP_HEADERS`, in cleartext over `http://`. Research: every mechanism that injects repo env into grim (`.envrc` is shell code; `mise` needs `mise trust`; devcontainer and CI are repo-owned) already grants that repo code execution or ownership of the environment, and every OTel SDK on the machine reads the same variables the same way; the owner accepted the T3 exposure of this gate (discussion › Decisions › Gate, Threads › Payload). Decision: no extra gate in v1 (an https-only or `--token-host`-style rule would be a new deviation from the OTel spec the owner did not ask for); the exposure is written down plainly in the ADR § Consequences and `telemetry.md`, and it is raised to the owner as a deferred finding (threat-model egress ruling) rather than silently accepted.
20. **Debug target name?** → tracing's default would be `grimoire::telemetry` (crate name) → every telemetry macro passes `target: "grim::telemetry"` so the documented `GRIM_LOG=grim::telemetry=debug` filter matches the binary name users know.

## Open questions

None. (The discussion's markers are resolved in § Decisions recorded.)

## Remote acts (none performed by this plan)

Push, PR, CI dispatch and deep verify belong to the goal loop's
meta-orchestrator; this plan commits locally only.

## Risks

| Risk | Mitigation |
|---|---|
| opentelemetry 0.33 glue breaks on a bump | exact `=` pins; glue confined to `export.rs` |
| Header-strip wrapper regresses and leaks ambient credentials | C-002 acceptance test with a sentinel bearer value |
| A hung collector stalls exits | C-003 double bound; C-015 acceptance with subprocess timeouts |
| Docs drift from code | P4 re-check step; C-018 test binds the field table to the wire |
| Stale `test/bin/grim` hides failures | positive control in every negative test; rebuild before pytest |
| T3: repo env arms export and harvests refs | owner-accepted (ADR § Consequences); header scoping; documented in `telemetry.md` |
| `uv.lock` / `Cargo.lock` conflicts | CW owns both; no pipeline adds dependencies |

## Verification record

All on 2026-10-22, branch `hex/otel-usage-telemetry`.

1. `task --force verify` on `6b2c2b6b`: exit 0 — nextest 4298/4298, acceptance 1845 passed / 8 skipped / 1 xfailed, `.claude` tests 91 passed.
2. `task docs:check` on `15686d4b`: every step green (Astro build, link validation, `check_urls.py`, `doc_declaration.py`, `theme_check.sh`, `landing_check.py`) except the final `npm audit --audit-level=high` (10 advisories, 2 high) — `docs/package-lock.json` is unchanged by this branch (last touched on main), so pre-existing.
3. `task rust:license:deps` exit 0, `task rust:audit` exit 0; `cargo tree -e features -i opentelemetry-otlp` / `-i opentelemetry_sdk` enable none of `reqwest-client`, `reqwest-blocking-client`, `grpc-tonic`, `internal-logs`.
4. Binary size: recorded in the ADR § Quantified Impact (dist +415,984 B / +1.6 %, release +655,056 B / +2.1 %, merge base `7b21abce`).
5. Collector interop: `otel/opentelemetry-collector-contrib@sha256:7f22d2a6748110ffb6a59bc0df448bccbf501cdb667d610ede4c61d3863d5849` (otelcol-contrib 0.163.2) with the `telemetry.md` recipe and a `debug` exporter. `validate` passes; startup has zero warnings after renaming the deprecated `signaltometrics` alias to `signal_to_metrics`. Armed `grim install` (exit 79), `init`, `status` and a path-source `add`: the debug exporter showed `grim.invocations` per command, `grim.errors{error.type=no-config}` = 1, `grim.artifact.changes{kind=skill,action=added}` = 1 and a `grim.invocation.duration` histogram per command.
6. Windows: `/mnt/c/Users/ecom/grim-wintest/run.sh … -k telemetry` on `15686d4b`: 52 passed, 3 skipped (POSIX-only tests), exit 0.
7. `task catalog:verify` exit 0; grim-usage drift review: `references/registries.md` gained the two env rows, `SKILL.md` and `references/troubleshooting.md` need no change (no command, flag or exit code changed).
8. Review: tier high, 7 native seats + round-2 delta seat; cross-model leg skipped (codex usage limit). Round 1 Needs Work → fix pass 1 (3 fixers) → round 2 one Warn (scope recording) → fixed in `ef401ce8`.

## Schedule log

- 2026-10-22 CW committed on `hex/otel-usage-telemetry` (`task --force rust:verify` 4253/4253, `task test:parallel` 1790 passed). Deviations accepted: gate is a pure fn of four booleans (C-021 accessors stay the only env readers); `record_scope(ctx_global, global, seed, scope, workspace)`; the outer `tokio::time::timeout` lives in `finish`; `Cargo.lock` moved `tempfile`'s `getrandom` 0.3.4→0.4.3 as cargo's own minimal resolution for `uuid`.
- 2026-10-22 P1–P4 merged (file sets validated against the CW commit). Contract drift fixed on the feature branch: `deliver` takes an injected env lookup (`finish` passes the process env, tests an empty one) so unit tests never export to an ambient endpoint; the test log capture arms `tracing_capture` (parallel-test interest-cache flake). P2 added `record_clients_with` / `record_artifacts_with` (lazy builders, run only when armed); the eager stubs stay test-only. Open for P5/review: a one-segment remote path yields no `vcs.*` (owner is non-optional); `redact::url` returns `<unparsable url>` for non-base URLs.
