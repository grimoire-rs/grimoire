# Plan: Telemetry destination through `GRIM_CONFIG` (#334)

## Status

- **Plan:** plan_grim_config_telemetry
- **Active phase:** 2 — Wave 2 (P3 docs and records) done; execution complete
- **Step:** awaiting /finalize
- **Last update:** 2026-11-13 (after 795efefb: chore: record review round 1 and the docs pass for GRIM_CONFIG telemetry (#334))
- State:   review
- Tier:    high
- Tier-grammar: 5
- Updated: 2026-11-13
- Next:    /hex-finalize
- Reviewed: 795efefbc9103af4d5084b627b9e296861f6d85f

---

## Header

- **Goal contract (binding):** `.agents/goals/issue-334.md`. Its four source-derived Done criteria are mapped to evidence in § Done criteria. Its § Rules › Owner decisions are implemented as written; the orchestrator recommendations stand unrevised.
- **Source:** [grimoire-rs/grimoire#334](https://github.com/grimoire-rs/grimoire/issues/334); overlaps [grimoire-rs/grimoire#83](https://github.com/grimoire-rs/grimoire/issues/83) ("Redirectable collector").
- **ADRs (written by this run, Accepted):** [`adr_grim_config_layers.md`](../adr/adr_grim_config_layers.md) (the `GRIM_CONFIG` mechanism, D-1…D-4, threat check) and the 2026-11-13 amendment of [`adr_otel_usage_telemetry.md`](../adr/adr_otel_usage_telemetry.md) (the `[telemetry]` table, arming order, destination unit).
- **Research:** `.agents/research/research_otel_scoped_destination.md` (landscape, not repeated) plus its 2026-11-13 addendum (ocx `${installPath}` verified from local docs).
- **Classification:** scope medium (about a week) · reversibility one-way (medium): a new environment variable, a new config-file table and a new credential variable, all Unstable until 1.0 · tier high, `architect=inline` (owner decisions are the design), `research=1` (the one unverified ocx point), `adversary=on` (new config surface and credential path that no accepted ADR covered before this run).
- **Branch:** `hex/issue-334` (worktree `/home/mherwig/.herdr/worktrees/grimoire/duo`).

## Objective

An organisation ships one TOML file and sets one variable, `GRIM_CONFIG`, and
every grim command sends its usage events to the organisation's collector with
the organisation's headers. No other tool sees the destination or the token,
nothing is written into `$GRIM_HOME` or a project by the deployer, every
opt-out still wins, and a setup that uses only `OTEL_*` behaves exactly as
today.

### In scope

- `GRIM_CONFIG` path-list reader (`src/config/env_layers.rs`, new) and its `src/env.rs` accessor.
- The `[telemetry]` table: schema, validation, layer merge (`src/telemetry/layer.rs`, new).
- Gate and destination changes in `src/telemetry.rs` and `src/telemetry/config.rs`; `GRIM_TELEMETRY_OTLP_HEADERS`.
- The owner-ordered rewording in `src/config/declaration.rs` and `AGENTS.md`.
- Acceptance tests (`test/tests/test_telemetry_config_file.py`, new).
- Docs and records: `telemetry.md`, `configuration.md`, `stability.md`, `AGENTS.md`, `catalog/skills/grim-usage`, two rule files' factual lines.

### Out of scope (each with its reason)

- Any table other than `[telemetry]` from `GRIM_CONFIG`, auto-detected system or user files, a `[telemetry]` table in `grimoire.toml` (owner recommendation; ADR D-4 reserves the user-file rung).
- `GRIM_TELEMETRY_OTLP_ENDPOINT` or other grim-prefixed endpoint variables: the owner chose the file for the destination; a user overrides it with a later `GRIM_CONFIG` entry.
- A `repository` key (env `GRIM_TELEMETRY_REPOSITORY` already grim-namespaced, so it has no collision to fix).
- `grim config` / `grim context` reporting of `GRIM_CONFIG` layers (YAGNI; nothing in the issue needs it, and it is the surface most likely to leak a header).
- A permissions check on the file (the org token sits in a package anyone with registry access can read; docs recommend restrictive modes).
- `packaging/grim/metadata.json` changes: the deployer writes the patch, grim ships nothing for it.

## Decisions recorded by the orchestrator

Question → research → decision. Owner decisions and the four ADR points
(D-1…D-4 in `adr_grim_config_layers.md`) are not repeated here.

1. **PD-1. Which tier and gate?** Classifier: new env var + config table, `enhancement` label, design settled by the owner → `high`, confident. The meta-plan gate is announce-and-proceed at confident `high`; the run is unattended, so no human gate was held.
2. **PD-2. `GRIM_TELEMETRY_OTLP_HEADERS` and file headers with an `OTEL_*` endpoint?** The owner rule fixes headers to grim sources when a grim file sets `endpoint`, and is silent on the reverse. Research: a file that sets headers without an endpoint, plus a host-wide `OTEL_EXPORTER_OTLP_ENDPOINT`, would send the organisation's token to an endpoint the file did not choose. Decision: grim-owned headers ride only with a grim-owned endpoint; with an `OTEL_*` endpoint, today's `OTEL_*` header rules apply unchanged (amendment § Destination). So that a user who sets only `GRIM_TELEMETRY_OTLP_HEADERS` plus `OTEL_EXPORTER_OTLP_ENDPOINT` is not left with a silent unauthenticated export, an armed run whose grim headers are unused prints one warning naming the two sources, never their values (architect review).
3. **PD-3. Header merge across files: per entry, or only from layers at or after the one that sets the winning `endpoint`?** Research: the second rule keeps an earlier file's token off a later file's endpoint. The only party that can stack a hostile later file is a repository environment, which decision 19 already treats as code execution: it can read the organisation file directly, so the rule buys no protection against it. It also adds a list-order trap: a credentials file listed before the endpoint file would lose its headers with no error. Decision: per-entry merge (the owner's "later wins"), rejecting the alternative for those two reasons (architect review). Recorded in the ADR threat check.
4. **PD-4. `protocol` key with a single legal value?** The owner listed protocol among the unit's keys. Decision: accept `protocol = "http/protobuf"` (case-insensitive); any other value is an invalid layer (D-1). It costs a few lines and lets a deployer copy an OTel-shaped config without a warning.
5. **PD-5. What counts as an explicit falsy `GRIM_TELEMETRY`?** Today an empty value, `0` and `false` all mean off. Decision: blank (empty or whitespace) is unset and defers to the files; any other non-truthy value is an explicit off that also disarms a file. Fail-safe on garbage.
6. **PD-6. Where are layers read?** Research (architecture map): the gate is decided in `Context::new` (`src/context.rs:187-191`) from env only, and `OTEL_*` is read at `finish`. Decision: read and merge the layers in `Telemetry::from_env` after the env opt-outs, store the merged `[telemetry]` in the armed `State`, and use it in `deliver`. One read per run; the `grim-hook` launcher has no telemetry code and stays untouched.
7. **PD-7. Unknown keys in `[telemetry]`.** Decision: named in the one ignored-keys warning and skipped (forward compatibility); wrong type or value on a known key is an invalid layer (D-1).
8. **PD-8. Warning channel.** Research: `tracing::warn!` reaches stderr at the default `warn` filter (`src/command/scope_resolution.rs:234`), runs after `init_tracing`, and never touches the exit code. Decision: `tracing::warn!`, at most one per run for each of: an invalid layer, ignored keys (only when the load succeeded), unused grim headers (PD-2). None quotes file content or a header value.
9. **PD-9. Contract wave.** P1 (Rust) and P2 (pytest) share no code symbol; their contract is the CLI, environment and file surface fixed in C-001…C-009 and S-001…S-012 below. Decision: no stub commit; P2 writes tests that fail until P1 lands, which is the TDD order the contract wave exists for.
10. **PD-10. Docs pipeline last.** Owner feedback (memory "Docs after implementation"): one docs and records pass after the last code fix, so pages match the final code. P3 depends on P1 and P2.
11. **PD-11. Adversary.** Configured `codex:rescue` is not installed as a skill here; the cross-model pass runs as `codex exec -m gpt-6.1-sol --sandbox read-only … < /dev/null` (hex.md Memory pitfall). Result in § Review record.
12. **PD-12. No `hex.md › Memory` write.** The goal loop's brief forbids writing `.agents/memory/hex.md`; the active-plan pointer is `.claude/state/current_plan.md` (gitignored) and this Status block. The loop owner records the Memory line.
13. **PD-13. Non-regular files (codex review, Block).** `config::read_capped` opens with `File::open`, which blocks on a FIFO with no writer, and reads to EOF, which a held-open FIFO never sends. In `Context::new` that hangs every command, a denial grim itself causes (threat model I3/N5 boundary). Decision: `env_layers` refuses any entry whose `std::fs::metadata` (symlinks followed) is not a regular file, before `read_capped` opens it; that is an invalid layer. `read_capped` stays unchanged. Swapping the path for a FIFO between the check and the open needs write access to it (N2), so no handle re-check is added (re-validation review). The same latent hang exists for a FIFO named as `grimoire.toml`; it is outside this issue and is reported to the loop owner as a follow-up, not fixed here.
14. **PD-14. `GRIM_TELEMETRY_REPOSITORY` on a file-armed run (architect review).** Its docs say "Needs `GRIM_TELEMETRY`". Decision: it applies whenever telemetry is armed, by either source; it stays environment-only (no file key, see Out of scope). The docs line changes to "needs telemetry armed".
15. **PD-15. A missing listed file (architect review, Suggest).** Alternative: skip a `NotFound` entry with a debug line, so an optional second entry does not warn on every command. Rejected: version 1 has no optional-entry use case (the ocx patch always ships its file), and a mistyped organisation path would then drop the organisation's destination silently. Strict and loud is easier to diagnose; revisit if optional entries are asked for.
16. **PD-16. Pipelines run in the feature worktree; workers never commit (hex-execute).** The project's PreToolUse hook refuses a commit in any tree without a `task verify` stamp under 300 s old, and the stamp may not be written by hand, so a pipeline worktree cannot hold commits. P1 and P2 own disjoint file sets and only P1 builds, so both edit `/home/mherwig/.herdr/worktrees/grimoire/duo` directly. No pipeline branch or worktree is created, which also avoids a second 7 GB target dir and the shared-target stale-binary trap (hex.md Memory). File-set re-validation is `git status --porcelain` against the Expected Files column before each commit; the orchestrator commits once per pipeline after a green `task verify`.
17. **PD-17. Where the docs pass sits in the one review call (hex-execute).** Two rules meet: P3 runs after the last code fix (PD-10), and the run has one review call. Decision: review round 1 covers wave 1 (code and acceptance tests); its fix pass lands; then P3 runs; round 2 covers the fix delta plus the P3 pages and adds the `doc-reviewer` seat for them. A round-2 code fix updates the matching page in the same fix pass.
18. **PD-18. Does `GRIM_CONFIG` forward to child processes? (`subsystem-cli.md` § Configuration Forwarding).** Research: the rule covers resolution-affecting flags that grim lands on a child environment. Grim lands no `GRIM_*` key on any child today (no forwarding function exists; children inherit grim's environment), and `GRIM_CONFIG` has no flag form and affects only telemetry. Decision: no forwarding code. Grim neither adds nor strips `GRIM_CONFIG` or `GRIM_TELEMETRY_OTLP_HEADERS` for children, so they inherit both as they inherit `GRIM_TELEMETRY` today (ADR I6 records the header-variable inheritance and the docs' file-form advice). The env-var reference needs only the two rows C-011 adds.
19. **PD-19. Exact warning texts (hex-execute, so P1 and P2 share one contract).** The error taxonomy gave examples only. Decision, matched by substring in the acceptance tests: an unusable layer warns `<reason>; telemetry is off for this run`, where `<reason>` is the `LayerError` Display: `GRIM_CONFIG entry '<entry>' is not an absolute path`, `cannot read GRIM_CONFIG file '<path>': <io error>`, `GRIM_CONFIG file '<path>' is not a regular file`, `GRIM_CONFIG file '<path>' is larger than <N> bytes`, `GRIM_CONFIG file '<path>' is not valid TOML (line L, column C)`, or `GRIM_CONFIG file '<path>' has an unusable value for '<dotted key>'`. Ignored keys: `ignored GRIM_CONFIG keys in '<path>': <k1>, <k2>` (files joined by `; `). Unused grim headers: `telemetry headers from GRIM_CONFIG or GRIM_TELEMETRY_OTLP_HEADERS are not sent: no GRIM_CONFIG file sets an endpoint`.
20. **PD-20. Review baseline (hex-execute).** The plan carries no `Reviewed:` anchor, so a default review would read `main..HEAD` and re-review the goal file and the plan and ADR commits, which hex-plan already reviewed in plan-artifact scope. Decision: review call 1 runs with `--base=93ada969` (the plan commit), so it reads exactly the execution commits.
21. **PD-21. Review seats (hex-execute).** `hex.md › Preferences` maps `reviewer:spec/quality/security` to `deep-reasoning`; the loop owner's brief for this run routes review seats to sonnet and keeps opus for a subtask that failed twice. Decision: every seat runs sonnet, stated here as the one deviation from the Preferences overrides. Tier high calls for separate performance and user-feedback seats; their checklists were folded into the Stage 2 quality seat (the change adds one read per process in `Context::new` and three stderr messages, so neither surface warranted its own seat).
22. **PD-22. Windows proof of the `;` separator (hex-execute).** No Windows host exists for this run. Research: grim adds no separator logic of its own; `read_layers` delegates to `std::env::split_paths`, and `Path::is_absolute` decides drive-letter and rooted entries. Decision: a Linux-runnable unit test proves `;` does not split off Windows (so grim never hard-codes a separator), the `#[cfg(windows)]` unit rows (`\foo`, `C:foo`, `;` splitting) and the acceptance tests (`os.pathsep`) run on the CI Windows legs, and the Windows proof is owed to a CI deep run.

## Component contracts

IDs are the coverage join keys. "Invalid layer" means the D-1 handling: one
warning on stderr, telemetry unarmed for the run, command outcome unchanged.

- **C-001 `GRIM_CONFIG` value.** `env::grim_config() -> Option<OsString>` reads `GRIM_CONFIG` with `var_os`; blank (empty after trimming, where the value is valid Unicode) is `None`. The reader splits with `std::env::split_paths`, skips empty entries, and keeps list order. A relative entry (`!Path::is_absolute`; on Windows this includes `\foo` and `C:foo`) is an invalid layer naming the entry. Duplicates are read twice (harmless).
- **C-002 Layer read.** Each entry must be a regular file by `std::fs::metadata` before it is opened (PD-13); then it is read with `config::read_capped` and parsed as a `toml::Table`. Errors: not found / not a regular file / unreadable / too large / not TOML. The error's `Display` holds the path and, for a parse error, `line L, column C` derived from the error span; it never holds file content and never formats the `toml` error itself (its `Display` quotes the source line). Type: `config::env_layers::Layer { path: PathBuf, table: toml::Table }`, error `LayerError` (`thiserror`, lowercase messages).
- **C-003 `[telemetry]` table.** Keys and types per the amendment table: `enabled` bool, `endpoint` string, `headers` table of strings, `timeout` integer > 0 (ms), `protocol` string equal to `http/protobuf` ignoring case, `service_name` string, `resource_attributes` table of strings. Validation at load: `endpoint` must pass `telemetry::config::parse_endpoint(raw, true)` (base URL, `/v1/logs` appended; a `Skip` maps to a `LayerError` naming `endpoint`); a header name must pass a new `pub(crate) fn valid_header_name` in `telemetry::config` (valid `HeaderName`, not in `RESERVED_HEADERS`), a value a valid `HeaderValue`; any failure, a wrong type, or `telemetry` not being a table is an invalid layer whose message names the key, never the value. Top-level keys other than `telemetry`, and unknown keys inside it, are collected into one warning per run naming file and keys. Merge (C-003b): files in list order; scalars later wins; `headers` per name case-insensitively, later wins; `resource_attributes` per exact key, later wins. Result: `telemetry::layer::TelemetryLayer` (all fields `Option`, headers in `telemetry::config::Headers`). The generic loader (`config::env_layers`) only reads and parses; each consumer merges its own table (this one by C-003b).
- **C-004 Gate.** A pure function over: `GRIM_TELEMETRY` as a tri-state (`Unset` blank or absent, `On` truthy, `Off` anything else), the three opt-outs (`OTEL_SDK_DISABLED`, `DO_NOT_TRACK`, offline), and a lazily called layer loader. Order: any opt-out → unarmed, loader not called; `Off` → unarmed, loader not called; loader error → warning, unarmed; else armed iff `On` or merged `enabled == Some(true)`. The armed `State` carries the merged `TelemetryLayer`. Today's 16-row truth table keeps its results with an empty layer list. `GRIM_TELEMETRY=1` with an invalid layer is unarmed too (D-1). The doc line on `Telemetry::from_env` ("four reads, no I/O") is updated to say it reads `GRIM_CONFIG` files past the opt-outs.
- **C-005 Destination.** `config` gains one selection entry point taking the merged layer and the `Lookup`. Layer `endpoint` set → `ExportConfig { endpoint: <base>/v1/logs, headers, timeout: layer or 500 ms }`, where `headers` = layer headers, then `GRIM_TELEMETRY_OTLP_HEADERS` read through the `Lookup` and parsed with the existing `parse_headers` (percent-decoded, `log_dropped` names keys only), merged case-insensitively with the env value replacing a file value of the same name. Grim's selection looks up no `OTEL_EXPORTER_OTLP_*` key, and no ambient value changes the request; the pinned SDK builder's own env-header read is discarded by `GrimHttpClient` (ADR condition 4), so the wire test, not the lookup test, proves the request. Layer `endpoint` unset → `from_env(lookup)` exactly as today, and neither layer headers nor `GRIM_TELEMETRY_OTLP_HEADERS` is used. `GrimHttpClient` unchanged.
- **C-006 Resource.** `resource_overrides` takes the layer and folds every override into its ordered `attributes`, lowest first: `OTEL_RESOURCE_ATTRIBUTES` pairs, `service.name` from `OTEL_SERVICE_NAME`, the layer's `resource_attributes`, `service.name` from the layer's `service_name`; its `service_name` field is then `None` (codex review: `payload::resource` applies `service_name` last, so leaving `OTEL_SERVICE_NAME` there would beat a file's `service.name` attribute). With no layer the wire result equals today's (`OTEL_SERVICE_NAME` still beats an `OTEL_RESOURCE_ATTRIBUTES` `service.name`). Applies to both destinations. `payload::resource` unchanged.
- **C-007 Secrecy.** No warning, log line (any level), error `Display`, `Debug` impl or stdout of any command contains a header value from a file or from `GRIM_TELEMETRY_OTLP_HEADERS`, or the userinfo of a refused endpoint. File header values live only in `SecretString`. A sentinel header value planted on the failing line of a broken file is absent from stderr.
- **C-008 Outcome isolation.** Every `GRIM_CONFIG` failure leaves the exit code and stdout of the command unchanged; at most one warning of each PD-8 kind per run, and the ignored-keys warning only when the load succeeded. An opted-out run opens no `GRIM_CONFIG` file and prints no `GRIM_CONFIG` warning.
- **C-009 Unchanged paths.** With `GRIM_CONFIG` unset, behaviour and wire bytes equal today's for every existing telemetry test. `crates/grim-hook` gains nothing (its dependency allowlist test stays green).
- **C-010 Gate wording.** The comment block at `src/config/declaration.rs:279-293`, the matching argument on `settings_enabled` (`:363`), and the `AGENTS.md` "(no variable)" row name **workspace consent** as the gate: the feature flag has no environment form today, is not the gate (a committed `grimoire.toml` can set it), and a `GRIM_CONFIG` file may carry it later; consent never has an environment or `GRIM_CONFIG` form. `GRIM_ALLOW_HOOKS` stays removed. No behaviour change.
- **C-011 Docs contract.** `telemetry.md` gains a section on the file form (precedence, keys, destination unit, warnings, a fleet example with the ocx patch entry from the research addendum) and the plain T3 paragraph decision 19 promised, and its stale env-only lines change: `:43-45` (only switch), `:62` (`GRIM_TELEMETRY_REPOSITORY` "Needs `GRIM_TELEMETRY`", PD-14), and any line saying an empty `GRIM_TELEMETRY` is off without naming the file case; `configuration.md` env table rows for `GRIM_CONFIG` and `GRIM_TELEMETRY_OTLP_HEADERS`; `stability.md` lists all three names under Unstable, keeps them out of the frozen env row, and fixes the Unstable telemetry bullet's claim (about `:609`) that nothing changes for a user who never sets `GRIM_TELEMETRY`; `AGENTS.md` rows edited in place (file stays at 280 lines); `catalog/skills/grim-usage` drift fixed per `catalog/README.md`.

## User-experience scenarios

Each: action → expected outcome → error cases.

- **S-001 Fleet deploy.** File `/opt/org/telemetry.toml` with `[telemetry] enabled = true`, `endpoint`, `[telemetry.headers] authorization = "Bearer …"`; env `GRIM_CONFIG=/opt/org/telemetry.toml`, no `GRIM_TELEMETRY`, no `OTEL_*` → `grim status` sends one POST to `<endpoint>/v1/logs` with the authorization header; nothing on stderr at the default level. Errors: endpoint unreachable → debug line only, exit unchanged.
- **S-002 Coexists with a host-wide OTel setup.** S-001 plus `OTEL_EXPORTER_OTLP_ENDPOINT=<B>`, `OTEL_EXPORTER_OTLP_HEADERS=x-ambient=1`, `OTEL_EXPORTER_OTLP_PROTOCOL=grpc`, `OTEL_EXPORTER_OTLP_TIMEOUT=1` → only the file endpoint receives a POST, carrying file headers and no `x-ambient`; B receives nothing; grpc and the 1 ms timeout are ignored.
- **S-003 Token in the environment.** S-001 with no header in the file and `GRIM_TELEMETRY_OTLP_HEADERS=authorization=Bearer%20tok` → the POST carries `authorization: Bearer tok`. Both present → env value wins per name, other file headers kept.
- **S-004 File arms, no grim endpoint.** File with only `enabled = true`; `OTEL_EXPORTER_OTLP_ENDPOINT=<B>` and `OTEL_EXPORTER_OTLP_HEADERS=x-otel=1` → B receives one POST with `x-otel`, as today. A file header or `GRIM_TELEMETRY_OTLP_HEADERS` is not sent to B. No endpoint anywhere → nothing sent (debug skip).
- **S-005 Opt-outs win.** (a) S-001 with a valid file plus any of `DO_NOT_TRACK=1`, `OTEL_SDK_DISABLED=true`, `GRIM_OFFLINE=1`, `--offline`, `GRIM_TELEMETRY=0` → no POST; the same environment without the opt-out → one POST (positive control). (b) A broken listed file plus the opt-out → no `GRIM_CONFIG` warning (the file is never opened); without the opt-out → one warning and no POST (codex review: a broken file cannot be the positive control).
- **S-006 User layer wins.** `GRIM_CONFIG=<org>:<user>` (`;` on Windows), user file sets `endpoint = <U>` → POST to U only. User file sets `enabled = false` and `GRIM_TELEMETRY` is unset → nothing sent. `GRIM_TELEMETRY=1` with a file `enabled = false` → armed (environment over file).
- **S-007 Broken file.** Unterminated string on a line holding a header value → stderr has one warning naming the path and `line`/`column`, saying telemetry is off for this run; the header value never appears; exit code and stdout equal a run without `GRIM_CONFIG`. Same for a missing file, a directory, an oversized file, `timeout = "fast"`, `protocol = "grpc"`, `telemetry = 1`.
- **S-008 Relative entry.** `GRIM_CONFIG=telemetry.toml` with that file present in the working directory → one warning naming the entry, telemetry off, the file is not read (its endpoint receives nothing).
- **S-009 Credentials in the endpoint.** `endpoint = "https://user:secret@host"` → invalid layer warning naming `endpoint`, without `secret` or the URL; nothing sent.
- **S-010 Other tables.** A file with `[telemetry]` plus `[options]` and `[[registries]]`, and an unknown `[telemetry] compression = "gzip"` → one warning naming the file and `options`, `registries`, `telemetry.compression`; telemetry otherwise works as S-001.
- **S-011 No `GRIM_CONFIG`.** Every existing scenario in `test_telemetry_export.py`, `test_telemetry_contract.py`, `test_telemetry_events.py`, `test_telemetry_repository.py` behaves as today.
- **S-012 Debug output stays clean.** `GRIM_LOG=trace` with S-001 + S-003 → stderr carries the payload debug line and no header value; `grim config list --format json` with the same environment prints no header value.

## Error taxonomy

| Failure | Detected | User sees | Exit | Remedy (in the warning or docs) |
|---|---|---|---|---|
| Relative entry | C-001 | `GRIM_CONFIG entry '<entry>' is not an absolute path; telemetry is off for this run` | unchanged | use an absolute path |
| Missing / unreadable / directory / too large | C-002 | warning with path and cause | unchanged | fix the path or permissions |
| Not TOML | C-002 | warning with path, line, column | unchanged | fix the file |
| Wrong type, bad value, userinfo, bad header name | C-003 | warning with path and key, never the value | unchanged | fix the key |
| Unknown or non-telemetry keys | C-003 | one warning listing them | unchanged | remove, or upgrade grim |
| Export failure | existing | debug line only | unchanged | — |

## Edge cases

Blank `GRIM_CONFIG`; `::` or trailing separator; the same file twice; a
non-UTF-8 path (handled as `OsString`); a symlinked file (followed, as
`read_capped` does); a FIFO or device (refused before opening, PD-13); a
`[telemetry]` table that is empty; `enabled = true` and no endpoint anywhere;
`GRIM_TELEMETRY` of `"  "` (blank, defers) versus `"no"` (off); header names
differing only in case across files; Windows `;` separator and drive-letter
paths; `grim mcp` and `grim tui` (one `Context`, one read).

## Trade-offs

The option analysis is in `adr_grim_config_layers.md` § Considered Options
(env-named layers vs grim-prefixed variables vs a fixed managed path; the
owner chose layers). Within the chosen option, the plan-level choices are
PD-2 (credential binding to the endpoint family), PD-3 (per-entry merge over
per-file ownership) and D-1 (fail closed for telemetry, never for the
command), each with its reason above.

## Constitution check (AGENTS.md › Core Principles)

| Principle | Check |
|---|---|
| 9 Preserve Compatibility | Additive only: a new variable, a new file table, a new credential variable; unset `GRIM_CONFIG` is byte-identical (C-009, S-011). `GRIM_TELEMETRY` gains one reading (blank defers to files); its documented truthy and off values keep their effect. All new names are Unstable until 1.0. No schema, layout or renderer change. |
| 3 Keep It Safe | C-007, D-3 relative refusal, PD-2, no substitution or helper. |
| 6 Ship It | Local commits only; no push. |

No deviation row needed.

## Done criteria → evidence

The four source-derived criteria of `.agents/goals/issue-334.md`:

| Criterion | Evidence |
|---|---|
| Only grim reads grim's telemetry destination and auth | Two variants. `test_grim_config_alone_arms_and_exports` (S-001: no `OTEL_*` variable at all; the destination and token reach the collector only through `GRIM_CONFIG`). `test_grim_config_endpoint_gets_only_grim_headers` (S-002: with a host-wide `OTEL_*` set present, ambient headers, protocol and timeout never reach the grim endpoint and the `OTEL_*` endpoint receives nothing). Plus `test_grim_telemetry_otlp_headers_supply_token` (S-003), `test_file_without_endpoint_uses_otel_endpoint` (S-004: grim headers never go to an `OTEL_*` endpoint), Rust `destination_unit_table` in `src/telemetry/config.rs` |
| Deliverable by a file plus environment variables, no write to `$GRIM_HOME` or project files | `test_grim_config_alone_arms_and_exports` (S-001): before the run `GRIM_HOME` is empty and the project holds only `grimoire.toml` (the deployer wrote nothing there), the file sits outside both, and the environment adds only `GRIM_CONFIG`; after the run the paths added under `GRIM_HOME` equal those of an env-armed control run in a twin home (grim's own `telemetry/installation-id`), and the project tree is unchanged. Plus the fleet example in `telemetry.md` with the verified ocx `${installPath}` entry |
| The user stays in control | `test_grim_config_opt_outs_win` (S-005 (a), parametrized over the five opt-outs, valid file, positive control each), `test_opted_out_run_never_opens_grim_config` (S-005 (b)), `test_later_grim_config_file_wins` (S-006), `test_broken_grim_config_file_disarms_quietly` (S-007), Rust `gate_truth_table_with_layers` in `src/telemetry.rs` |
| Standard `OTEL_*` keep working when set for grim alone | S-011: `test_telemetry_export.py` and the other three telemetry test files pass with zero edits (C-009; `git diff --stat main -- test/tests/test_telemetry_*.py` lists only the new files), plus `test_file_without_endpoint_uses_otel_endpoint` (S-004, parametrized: an `OTEL_*` endpoint receives today's request; no endpoint anywhere sends nothing) |

The other three goal criteria (PR merge-ready, CI green, deep verify) are the
DONE block's, after `/hex-finalize`.

## Pipelines

### Contract wave

None (PD-9). P1 and P2 share no code symbol; the shared contract is the
surface above, fixed by this plan.

### Parallelization

| Pipeline | Scope | Expected Files | Wave | Depends on | Marks | Status |
|---|---|---|---|---|---|---|
| P1 code | C-001, C-002, C-003, C-003b, C-004…C-009, C-010 (code half) · S-001…S-012 behaviour | `src/env.rs`, `src/config.rs`, `src/config/env_layers.rs` (new), `src/telemetry.rs`, `src/telemetry/config.rs`, `src/telemetry/layer.rs` (new), `src/context.rs` (only if the `from_env` signature must change), `src/config/declaration.rs` (comment only) | 1 | — | | merged |
| P2 acceptance | S-001…S-012 · C-002 (non-regular file), C-006 (wire collision), C-007, C-008, C-009 | `test/tests/test_telemetry_config_file.py` (new) | 1 | — | | merged |
| P3 docs and records | C-010 (`AGENTS.md` half), C-011 | `docs/src/content/docs/telemetry.md`, `docs/src/content/docs/configuration.md`, `docs/src/content/docs/stability.md`, `AGENTS.md`, `catalog/skills/grim-usage/references/telemetry.md`, `catalog/skills/grim-usage/references/registries.md` (+ any other grim-usage file the drift review names), `.claude/rules/subsystem-file-structure.md` (§ Telemetry arming sentence), `test/tests/test_telemetry_config_docs.py` (new) | 2 | P1, P2 | | merged |

```mermaid
graph TD
  subgraph W1[Wave 1]
    P1[P1 code]
    P2[P2 acceptance]
  end
  subgraph W2[Wave 2]
    P3[P3 docs and records]
  end
  P1 --> P3
  P2 --> P3
```

- **Critical path:** P1 (4 steps) → P3.
- **Shippable after wave: 1** — the feature works and is tested (P2's file reads no doc page); docs still describe env-only telemetry. **Shippable after wave 2** — everything.
- **Parallelism:** two pipelines in wave 1 is the widest cut: the Rust files form one chain (the gate, layer type and destination selection change together), so they are steps of one pipeline.
- **Review perspectives for `/hex-execute`:** `reviewer:security` on `src/telemetry/**` and `src/config/env_layers.rs` (credential path; not covered by hex.md's always-on globs), `doc-reviewer` on P3, plus the stability perspective (Principle 9) on every diff.

### P1 code — steps

1. **Layer reader (C-001, C-002).** Add `GRIM_CONFIG` to `src/env.rs` (`grim_config() -> Option<OsString>`, same doc style as its neighbours) and a `telemetry_switch()` tri-state accessor next to `telemetry()` (PD-5). New `src/config/env_layers.rs`: `read_layers(value: &OsStr) -> Result<Vec<Layer>, LayerError>` with `split_paths`, absolute check, `read_capped`, `toml::from_str::<toml::Table>`, span → line and column. Register the module in `src/config.rs`. Unit tests: empty entries, relative refusal (plus `#[cfg(windows)]` rows for `\foo` and `C:foo`), missing file, directory, a FIFO (`#[cfg(unix)]`, made with the `mkfifo` command, refused without blocking), oversize (reuse the cap constant), parse error message carries line and column and **not** a planted secret on the failing line, order preserved. Gate: `cargo test -p grimoire env_layers` and `cargo clippy` clean.
2. **`[telemetry]` table (C-003, C-003b).** New `src/telemetry/layer.rs`: `TelemetryLayer` (all `Option`; headers as `telemetry::config::Headers`), `TelemetryLayer::merge(layers: &[Layer]) -> Result<(TelemetryLayer, IgnoredKeys), LayerError>`. Reuse the rules in `src/telemetry/config.rs`, never copy them: call `parse_endpoint(raw, true)` and map its `Skip` to a `LayerError` naming `endpoint`; add `pub(crate) fn valid_header_name` there (valid `HeaderName`, not in `RESERVED_HEADERS`). `Debug` must not print header values (reuse `Headers`' redacting `Debug`). Unit tests: each key's type error names the key and not the value, userinfo refusal, protocol rule, per-entry header merge ignoring case, resource-attribute merge, unknown and non-telemetry keys collected, empty table.
3. **Gate (C-004, C-008).** In `src/telemetry.rs`: replace `armed(..)` with the C-004 pure function (keep a 16-row-equivalent table and add the layer rows: `gate_truth_table_with_layers`), call the loader from `Telemetry::from_env` only past the env opt-outs, emit the PD-8 warnings (the PD-2 one too: an armed run with layer headers or `GRIM_TELEMETRY_OTLP_HEADERS` and no layer endpoint), store the merged layer in `State`, and update the `from_env` doc line. `env::telemetry()` has one caller (`src/telemetry.rs:117`), so replacing it with `telemetry_switch()` is local. Gate: `cargo test -p grimoire telemetry` and clippy clean.
4. **Destination, resource, wording (C-005, C-006, C-007, C-009, C-010).** Depends only on `TelemetryLayer` (step 2) and the `State` field (step 3). In `src/telemetry/config.rs`: the C-005 selection entry point (`destination_unit_table`: a file endpoint looks up no `OTEL_EXPORTER_OTLP_*` key, checked with a recording lookup; no file endpoint equals `from_env`; `GRIM_TELEMETRY_OTLP_HEADERS` merge, case-insensitive, env wins, percent-decoding) and the C-006 fold (`resource_overrides_with_layer_table`, including the `OTEL_SERVICE_NAME` against file `service.name` row); wire both into `deliver`. Reword the `src/config/declaration.rs` comments per C-010 (code-docs caps: guard clauses keep constraint plus consequence). Gate: `env -u OCX_GLOBAL task rust:verify`.

### P2 acceptance — steps

1. **`test/tests/test_telemetry_config_file.py`.** One test per scenario, named as in § Done criteria, plus `test_relative_grim_config_entry_is_refused` (S-008), `test_endpoint_userinfo_in_file_is_refused` (S-009), `test_other_tables_warn_once` (S-010), `test_grim_config_secrets_never_printed` (S-012, `GRIM_LOG=trace` and `grim config list --format json`, and the S-007 sentinel), `test_fifo_grim_config_entry_never_blocks` (C-002, POSIX only: a FIFO with no writer and one held open by a writer; grim finishes within 10 s with the warning and the unchanged exit code), `test_file_resource_beats_ambient_service_name` (C-006: `OTEL_SERVICE_NAME=ambient` and file `resource_attributes."service.name"` → the wire carries the file value). Use the `otlp_collector` fixture and `serve_collector()` for the second collector, `os.pathsep` to join entries, files written under `tmp_path` outside `GRIM_HOME` and the project. Follow the file convention of `test_telemetry_export.py`: every negative carries a positive control in the same test. Do not edit any existing telemetry test or fixture. Gate: `uv run pytest test/tests/test_telemetry_config_file.py --collect-only` succeeds; the tests fail before P1 lands (record which in the Schedule log).

### P3 docs and records — steps

1. **User docs (C-011).** `telemetry.md`: a "Configure with a file" section after "Switch it on" (`{#config-file}` anchor): what `GRIM_CONFIG` is, the key table, arming order, the destination unit, the warnings, `GRIM_TELEMETRY_OTLP_HEADERS`, a fleet example with the ocx patch entry (`"visibility": "public"`, absolute `${installPath}`), and the T3 paragraph (a repository environment can name a file, same exposure as `GRIM_TELEMETRY` plus an `OTEL_*` endpoint, prefer the file for tokens). Update "Switch it on" so `GRIM_TELEMETRY` is no longer the only arming switch, and the Stability section. `configuration.md` env table: two rows. `stability.md`: the Unstable telemetry bullet names the three new surfaces; the frozen env row excludes them. Docs-quality gate: declarations kept, sentences ≤ 25 words, `{#kebab-id}` anchors; run `task docs:check` if Node 24 is present, else record why not. New `test/tests/test_telemetry_config_docs.py`: `test_stability_page_lists_grim_config_as_unstable` (the three names in the Unstable telemetry bullet), `test_configuration_env_table_names_grim_config` (both variables in the `configuration.md` env table, the `telemetry.md` `{#config-file}` anchor present), `test_agents_md_names_consent_as_the_gate` (the "(no variable)" row says workspace consent is the gate). C-010's wording is otherwise review-gated.
2. **Records (C-010 `AGENTS.md` half, C-011).** `AGENTS.md`: edit the `GRIM_TELEMETRY` row in place to name `GRIM_CONFIG` and `GRIM_TELEMETRY_OTLP_HEADERS` (file arming, grim-only destination), and the "(no variable)" row to name consent as the gate, matching the code comment P1 wrote; line count stays 280 (`task claude:tests`). `catalog/skills/grim-usage`: drift review per `catalog/README.md` (telemetry procedure and env table at least), then `task catalog:verify`. (The `arch-principles.md` ADR index rows landed with the plan, because `task claude:tests` requires a row per ADR.) `.claude/rules/subsystem-file-structure.md` § Telemetry: "first armed run" wording covers file arming. Gate: `env -u OCX_GLOBAL task verify`.

## Verification

- Dev loop: `env -u OCX_GLOBAL task rust:verify` (P1), the new pytest file (P2), `task claude:tests` + `task catalog:verify` (P3).
- Integration gate: `env -u OCX_GLOBAL task verify` on the merged tree; `task docs:check` where Node 24 exists.
- Windows: `/mnt/c/Users/ecom/grim-wintest/run.sh` with `-k telemetry_config_file` when `/mnt/c` exists (the `;` separator and drive-letter paths); on a host without it, the CI deep run is the proof (memory "Herdr host has no Windows").
- Baseline at planning: see § Review record.

## Review record

hex-plan Review phase, plan-artifact scope, one round (2026-11-13), then one
re-validation after the fixed Blocks.

- **Seats:** `reviewer:spec` (standard), `architect` (trade-offs and threat check), cross-model adversary `codex exec -m gpt-6.1-sol --sandbox read-only` (completed, `REVIEW COMPLETE`). The panel `researcher` seat was not run: `research_otel_scoped_destination.md` (2026-11-09) covers the landscape and research is never repeated.
- **Blocks (3, all fixed):** the no-write test could not pass because grim writes its own `installation-id` (spec; Done table now compares against an env-armed twin); a FIFO entry hangs every command in `Context::new` (codex; PD-13, C-002); an ambient `OTEL_SERVICE_NAME` beat a file's `service.name` attribute (codex; C-006 fold order). Re-validation: all three closed; one follow-on (a handle re-check `read_capped` cannot do) closed by dropping the re-check, PD-13.
- **Warns applied:** S-003 and S-004 named tests; C-003b in a Scope cell; the stability test moved to P3 so wave 1 ships alone; P1 step 3 split into 3 and 4; C-005 merge pinned (case-insensitive, env wins, read through `Lookup`); C-008 ignored-keys warning only on a successful load; C-010 `:363` unconditional; `parse_endpoint` / `valid_header_name` reuse spelled out; `telemetry::config::Headers`; stale doc lines named in C-011 (architect); PD-2 warning for unused grim headers (architect); D-1 reason restated, including `GRIM_TELEMETRY=1` with a broken layer (architect); S-005 positive control split from the broken-file check (codex); the "no `OTEL_*` reads" claim scoped to grim's selection because the SDK builder reads env headers itself (codex); D-4 unit-rule note; I6 child inheritance; Windows `\foo` / `C:foo` rows; PD-14 `GRIM_TELEMETRY_REPOSITORY`.
- **Kept against a finding, with reasons:** PD-3 per-entry header merge (architect asked for endpoint-layer ownership; rejected as no protection against a code-executing repository plus a list-order trap); PD-15 a missing file stays an invalid layer (architect Suggest).
- **Threat check (T3 against decision 19 / A-1):** holds, per the architect seat and the ADR § Threat model; no widening; relative entries refused (D-3).
- **Deferred to the loop owner (outside #334):** a FIFO named as `grimoire.toml` hangs `read_capped` the same way (PD-13); candidate follow-up issue.
- **Verify baseline:** first `task verify` after writing the ADR failed only `TestAdrIndex::test_every_adr_has_an_index_row` (the new ADR had no `arch-principles.md` row); the row was added with the plan. No pre-existing failure.

### hex-review call 1 (hex-execute, 2026-11-13), range `93ada969..`

- **Round 1** over `93ada969..80dd1865` (P1 code, P2 tests), tier high, loop mode. Seats (all sonnet, PD-21): spec P1 with the convergence check, test coverage P1, spec P2, seams, quality (with the performance and user-feedback checklists), security; cross-model adversary `codex exec -m gpt-6.1-sol --sandbox read-only` (completed, `REVIEW COMPLETE`). Gate evidence handed to the seats: `task verify` exit 0 on tree `7144db6c`.
- **Block (1):** a non-Unicode `GRIM_TELEMETRY` read as unset, so a file could arm over an explicit off (codex Block; spec P1 Warn; seams Suggest; one defect, max-wins). Location `src/env.rs` `telemetry_switch`, class: gate bypass.
- **Warn, actionable (8):** the `read_capped` fallback arm erased its source through `.to_string()`; `put` duplicated `payload::set` (twice); the empty `service_name` filter had no test; no Linux-runnable separator test (PD-22); the secrets test's broken-file leg could pass vacuously; no acceptance test that ignored keys stay silent when the load fails; the file resource override was untested on the `OTEL_*` destination; the ambient 1 ms timeout could still succeed on loopback, so the collision test did not prove timeout isolation (codex).
- **Suggest, accepted (5):** double lowercasing in the headers arm, a duplicated assertion, a redacting `Debug` for `TelemetryLayer` (a query string could carry a token), a non-empty guard on the no-write twin comparison, `assert cwd is not None` in place of `type: ignore`.
- **Declined with reasons:** echoing a header name or a resource-attribute key in an `InvalidKey` message (the contract names the key, never the value; names are keys); printing the ignored key names (S-010 requires naming them); warning on `http://` with headers (operator choice, same as the `OTEL_*` path); the metadata-then-open race (PD-13, needs write access, N2); an `env.rs` accessor for `GRIM_TELEMETRY_OTLP_HEADERS` (the name must stay one constant beside the `Lookup` key); extending the unused-headers warning to a file `timeout` without `endpoint` (documented in `telemetry.md` by P3 instead); rewording the PD-19 texts (owner-fixed contract); a warning on every command for an unusable file while `GRIM_TELEMETRY` is unset (D-1 and the ADR's Consequences accept it).
- **Convergence (round 1):** C-001…C-010 delivered (C-008 partial through the Block); C-011 pending P3 by design (PD-17). No unrequested behaviour.
- **Fix pass 1** landed as `f67b641c` (all actionable items and the five accepted Suggests); P3 then ran as `87c9199b` (PD-17). Gate: `task verify` exit 0 and `task docs:check` exit 0 on tree `a969c0e0`.
- **Round 2** over `80dd1865..795efefb` (fix delta plus the P3 pages), seats: spec (with test coverage), quality, `doc-reviewer`. No adversary (one-shot per call). Spec and quality: all ten round-1 items resolved with tests that fail on revert, no regression. Convergence: C-001…C-011 delivered; `test_telemetry_config_docs.py` is the P3 step the plan names, not unrequested.
- **Round 2 findings (doc-reviewer), all actionable, fixed in fix pass 2:** High — the fleet example said a user's second file replaces the organisation's headers, but headers merge per name, so an organisation header the user's file does not name also reaches the user's collector. Warn — the `headers` key row did not say an invalid header disarms the file; the `AGENTS.md` telemetry row still called an `OTEL_*` endpoint variable required and said "Both" for four variables; the catalog skill pointed at `grim --help`, which says nothing about `GRIM_CONFIG`. Suggest — `stability.md`'s exceptions sentence lacked `GRIM_TELEMETRY_OTLP_HEADERS`, a lost referent in the header-scoping paragraph, a redundant parenthetical in the `AGENTS.md` consent row, two small test-hygiene items (`_read` annotation, `find` over `index`) and a bound on the collector `delay=` mode.
- **Declined (round 2):** the unreachable `read_capped` fallback arm would print the path twice if it ever fired (it cannot: `read_capped` yields only `Io` and `FileTooLarge`).
- **Converged:** yes. Fix pass 2 changes docs and test text only; it was checked by the orchestrator's diff read and the gate (`task verify`, `task docs:check`), not by a third round (cap 2 fix rounds). No finding returned twice; nothing open.

## Schedule log

- 2026-11-13T00:03:50Z · step P2/1 · model standard · work 1m34s
- 2026-11-13T00:03:50Z · step P1/1 · model standard · work 3m50s
- 2026-11-13T00:08:39Z · step P1/2 · model standard · work 2m27s · wait 1m
- 2026-11-13T00:12:19Z · step P1/3 · model standard · work 3m08s · wait 32s
- 2026-11-13T00:16:24Z · step P1/4 · model standard · work 9m40s · wait 58s
- 2026-11-13T00:31:16Z · merged P1 @ 050999e0 · ready: P3 (after P2) · blocked: —
- 2026-11-13T00:31:16Z · merged P2 @ a0bf1074 · ready: P3 (held for the round-1 fix pass, PD-17) · blocked: —
- 2026-11-13T00:44:00Z · step fix-R1/rust · model standard · work 7m55s
- 2026-11-13T00:44:00Z · step fix-R1/pytest · model standard · work 27s
- 2026-11-13T00:57:18Z · gate integration-2 · task verify exit 0, docs:check exit 0 · tree a969c0e0
- 2026-11-13T00:51:05Z · step P3/1 · model standard · work 1m16s
- 2026-11-13T00:51:05Z · step P3/2 · model standard · work 39s
- 2026-11-13T00:59:41Z · merged P3 @ 87c9199b · ready: — · blocked: —
- 2026-11-13T01:05:30Z · step fix-R2/docs · model standard · work 25s
