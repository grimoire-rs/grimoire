# Research: OTEL telemetry — repo archaeology

<!--
Technology Landscape Research
Filename: .agents/research/research_otel_repo_archaeology.md
Owner: Researcher (hex research phase) — lane: repo archaeology, read-only
Handoff to: Architect (/hex-architect), /hex-plan
Related Skills: hex-architect, hex-plan
-->

## Metadata

**Date:** 2026-10-22
**Domain:** observability
**Triggered by:** `.agents/discussions/otel-usage-telemetry.md` (State: active, untracked); [grimoire-rs/grimoire#206](https://github.com/grimoire-rs/grimoire/issues/206) and [grimoire-rs/grimoire#83](https://github.com/grimoire-rs/grimoire/issues/83); open thread "Exporter implementation" (vendored rule OBS-12 vs. the crates)
**Expires:** 2027-04-22 (the crate-stability facts in finding 1.7 go stale first)

Tags: V = verified this session by reading a file, running git/gh, or fetching a URL. I = inferred, not run. Scope: history and prior art in the owner's repos. No recommendation; the sibling lane `research_otel_codebase_recon.md` covers grim's current code.

## Direct Answer

**Was OBS-12 measured or a default?** A reasoned default plus a pinned family decision. No binary-size, startup or flush measurement backs it anywhere in `ocx-sh/grimoire-lore` (V, finding 1.4). Its stated reasons are three documentation-derived claims: setup and flush cost "for no local benefit", the `opentelemetry-rust` stability warning, and an agent failure mode of reaching for OTel by default. The rule file itself calls it a "pinned decision ... already shipped and not re-litigated" (V, `.claude/rules/rust-quality/docs-and-tracing.md:17-23`).

**Evidence that the ban is contingent, not absolute:**
- The row text allows a daemon-mode exception "pinned, with the justification written down".
- The underlying research also allows OTel "when the org already runs a collector every invocation can ship to" (V, `tracing-and-observability.md:254` in lore).
- The ecosystem map dropped OTel because "no metrics pipeline exists or is planned" (V, lore `ecosystem-map.md:188`), a premise the current discussion changes.

**Evidence that the ban's premises still hold:**
- `tracing-opentelemetry` 0.34.0 still carries a stability warning on docs.rs (V, finding 1.7).
- grim's hook hot path has a measured 3.2 ms floor and a 29.5 MB binary that is already named as the cold-start cost (V, finding 3.6).

**Prior rejected or parked grim telemetry attempt:** none. [#83](https://github.com/grimoire-rs/grimoire/issues/83) and [#206](https://github.com/grimoire-rs/grimoire/issues/206) are the only records (V, finding 3.1).

## Technology Landscape

Archaeology lane: no market scan. The tables record what exists in the owner's repos.

### Trending (gaining momentum)

| Tool/Pattern | Adoption Signal | Key Benefit | Relevance to Grimoire |
|-------------|----------------|-------------|-------------------|
| n/a | not surveyed in this lane | n/a | n/a |

### Established (proven, widely accepted)

| Tool/Pattern | Status | Notes |
|-------------|--------|-------|
| OTLP export of CI test and build timings to `otel.ocx.sh` | In production across the ocx fleet (V) | ocx `subsystem-ci.md:136-182`; fleet wiring via `ocx-sh/conventions` `otel-auth` input. Not a CLI feature: no `opentelemetry*` crate in any ocx `Cargo.toml` (V). |
| Hand-written OTLP/HTTP+JSON exporter | One working instance, Python, in ocx (V) | `scripts/bep_to_otlp.py`: stdlib `json` + `urllib`, synchronous POST, response read. ocx rule `subsystem-ci.md:177`. |

### Emerging (early but promising)

| Tool/Pattern | Signal | Worth Watching Because |
|-------------|--------|----------------------|
| n/a | not surveyed in this lane | n/a |

### Declining (losing mindshare)

| Tool/Pattern | Signal | Avoid Because |
|-------------|--------|---------------|
| Phone-home opt-in telemetry for private-registry tools | Called "Declining" in ocx research of 2026-04-29 (V) | "Incompatible with offline + private-first constraints"; scoped to one question (tracking `--mode=full` drift), not a general ruling. See finding 2.4. |

## Design Patterns Worth Considering

Patterns found in the owner's own repos, not external survey.

- **Silent-by-default dev opt-in via a mode-600 env file.** ocx: no endpoint and no binary means no output and no failure (V, `subsystem-ci.md:165-169`).
- **Telemetry never decides a job.** `continue-on-error: true`, `timeout-minutes: 5`, and a secret-non-empty guard so fork PRs skip (V, `subsystem-ci.md:159`; fleet contract `ocx-conventions/docs/contract.md:86-89,118-119`).
- **Trust the payload count, not the exit status.** A push that lands nowhere still exits 0 (V, `subsystem-ci.md:147`). Applies to any flush-on-exit design.
- **Field allowlist, not event allowlist, with a planted-credential test.** ocx `bep_to_otlp.py` (V, `subsystem-ci.md:179-180`).
- **Stable-prefix stderr line as the no-phone-home alternative.** ocx research `research_deprecation_telemetry_patterns.md:58-60` (V).

## Key Findings

### 1. OBS-12 provenance

1. **Origin.** Introduced in `ocx-sh/grimoire-lore` commit [`30ef76e`](https://github.com/ocx-sh/grimoire-lore/commit/30ef76ef2eef6cf0c5f1b83097f5322fe15a50f0) (2026-08-16, "feat: the Rust quality catalog — rules, research corpus, and release gate"). `git log -S"opentelemetry" -- rules/rust-quality` returns only that commit (V; local checkout `/home/mherwig/dev/grimoire-lore`). Later edits to the file (`78b2889` 2026-09-27, `c85341c` 2026-10-11) did not touch the row. The row at lore HEAD is byte-identical to `grimoire-duo` (V, `diff`).
2. **Arrival here.** [`069bfe38`](https://github.com/grimoire-rs/grimoire/commit/069bfe389c2623dc50853bbd7756cdaba542d16d) (2026-09-06, "chore: added shared ocx agentic artifacts"), re-synced by `340184a9` (2026-10-04). ocx took it on 2026-08-18 (`32833e5e8`, "add ocx lore rust and cargo rules"). [#83](https://github.com/grimoire-rs/grimoire/issues/83) was opened 2026-08-17, before this repo vendored the rule. #83's text never mentions OTel or OBS-12. No grim ADR, plan or discussion weighed OBS-12 before 2026-10-22 (V, `git log --all -S"opentelemetry"` hits only `069bfe38`).
3. **Justification chain.** Rule row ← lore `.agents/research/rust-docs-observability.md` (model: opus, 2026-08; consolidation) Verdict 6 (lines 50-59), row OBS-12 (line 146), anti-pattern 11 (line 296), all as of `30ef76e` ← worker artifact `rust-docs-observability/tracing-and-observability.md` (model: sonnet, `date_researched: 2026-08`, 22 sources). Worker lines (V): 60 ("setup/flush cost and the 'opentelemetry-rust is still evolving, breaking changes occur' warning both argue against enabling it by default"), 254 (§14, "full OTel export adds provider/exporter setup and a flush-on-shutdown requirement for no local benefit — it earns its keep only when the CLI is embedded in a server/daemon mode (e.g. ocx-mirror running continuously) or when the org already runs a collector every invocation can ship to"), 335 (agent failure mode), 343 (churn caveat). The span-taxonomy worker states it "Does NOT cover: OTel SDK/exporter configuration" (`span-taxonomy-for-concurrent-io.md:17`).
4. **No measurement.** Searching the observability worker artifact for measure, benchmark, binary size, hyperfine and cargo-bloat finds nothing about OTel (V; the one hit, line 313, is `tracing`'s disabled-span design note). The sibling performance corpus says startup costs are "self-measure-only" (`rust-performance.md:58-64`) and `cli-startup-latency.md` never mentions OTel (V). Sources cited for OBS-12 are docs.rs pages and OTel semconv pages (`tracing-and-observability.md:354-360`).
5. **Rule text vs. research text.** The research says the crate stability warning argues against *enabling it by default* (line 60). The rule bans the *dependency* in a short-lived CLI outright, with a daemon-mode exception. Verification is review-only: "Any new `opentelemetry*` line in `Cargo.toml` needs an accompanying daemon-mode rationale" (V, `docs-and-tracing.md:99`). `deny.toml` `[bans]` holds only `multiple-versions = "warn"` (per sibling recon, I).
6. **Pinned, not measured.** The rule file's preamble lists it among "the pinned decisions ... this family's, already shipped and not re-litigated: ... no OpenTelemetry *crates* in a process measured in seconds, but its field-name vocabulary adopted anyway" (V, `docs-and-tracing.md:17-23`). The lore verdict calls the vocabulary half "the settled position (OBS-20)". The ecosystem map's premise for dropping OTel was "no metrics pipeline exists or is planned" (V, `ecosystem-map.md:188`).
7. **Premise currency, 2026-10-22.** (V, WebFetch, so paraphrase-level.) docs.rs `tracing-opentelemetry` 0.34.0, "Stability Status": the underlying `opentelemetry` crate "is not [stable] so some breaking changes will still occur in this crate as well"; "Metrics are not yet fully stable". The `opentelemetry-rust` README status table: Traces API/SDK/OTLP exporter **Beta**; Metrics API/SDK/OTLP exporter **Stable**; Logs API/SDK/OTLP exporter **Stable**; Prometheus exporter Beta. The README has no breaking-change warning of its own. The lore phrase "still evolving" is the older wording and no longer appears on the docs.rs page. The crate-stability concern is thus partly current (the tracing bridge, traces) and partly dated (metrics, logs).
8. **The named daemon candidate never used it.** Lore names `ocx-mirror` in daemon mode as "the only legitimate candidate". `/home/mherwig/dev/ocx-mirror` (HEAD `46262e4`, 2026-10-10) has no `opentelemetry` in `Cargo.toml` and no daemon mode in `Cargo.toml`/`CLAUDE.md` (V; not exhaustively checked for a long-running subcommand). So no instance of the exemption path exists in the fleet to copy.

### 2. Sibling projects

1. **ocx CLI: no OTel crate, ever.** `/home/mherwig/dev/ocx` (`e70bac609`, 2026-10-17): no `opentelemetry` in any `Cargo.toml`/`crates/`; `git log --all -S"opentelemetry"` and `-S"tracing-opentelemetry"` over manifests return nothing (V). Nothing added, nothing reverted.
2. **ocx CI/dev telemetry is the only OTel use, and it is opt-in.** Test and Bazel timings go to `https://otel.ocx.sh` as OTLP traces (V, `subsystem-ci.md:136-182`):
   - JUnit route: `junit2otlp` 0.1.2 (Go) over gRPC.
   - Bazel route: `scripts/bep_to_otlp.py`, OTLP/HTTP+JSON, hand-written (see Established table).
   - Local: off unless `~/.config/ocx-telemetry/env` (mode 600) exists.
   - CI: org secret `OTEL_OTLP_AUTH`, basic auth.
   - Commits: `44c950aba`, `48c0b8982` (Bazel BEP push), `acb68763b` / `85b642584` ("every test of a run reaches Tempo, not the 31% two OTel defaults ..."), `454fa243b`.
3. **Measured exporter pitfalls from that work (Go SDK, not Rust).** The SDK's 2,048-span batch queue silently dropped spans (2,560 of 8,250 landed, exit 0, nothing logged); export errors do not change the exit status; a batch size of 10 meant 825 gRPC round trips against 17 (V, `subsystem-ci.md:147-153`). The Python route has no queue: "the POST is synchronous and the response is read" and `partialSuccess.rejectedSpans` is checked (V, `:177`). Redirects are refused to keep `Authorization` from crossing origins (CWE-200) (V, `:178`).
4. **ocx product stance on user-facing telemetry.** `.claude/artifacts/research_deprecation_telemetry_patterns.md` (ocx commit `795920cb5`, 2026-05-02; dated 2026-04-29) rejects "OpenTelemetry opt-in" for a CLI flag-drift question as "incompatible with OCX's three constraints": backend-first (clean stdout), offline-first, private-first. It cites GitHub CLI (opt-out, `GH_TELEMETRY=log` prints the payload) and dbt as the counter-examples. Related lines: `prd_project_toolchain.md:65` ("never phone home" user story), `adr_shell_env_overhaul.md:516` ("no telemetry" as a cost line), `research_ai_config_gstack.md:187` (skip gstack-style skill telemetry). Scope caveat: written for one ADR open question, before ocx's CI telemetry existed.
5. **`~/dev/index/bot` and other repos.** No OTel or telemetry in `index/bot-tools`, `ocx-indexbot`, `grimoire-index`, `grimoire-indexer`, `grimoire-vscode`, `setup-ocx`, `setup-grimoire` (V, grep; only lighthouse reports, lockfiles and a code comment hit). `ocx-sdk-python`, `ocx-website` and `ocx-conventions` carry the CI `otel-auth` push step only. The lore plan `plan_ocx-project-baseline.md:99` and goal `goals/ocx-project-baseline.md:15,105` make "Bazel: shared cache + OTel (not mirrors)" a fleet baseline: CI telemetry, not CLI telemetry.
6. **gh/ocx-sh issue search.** `gh search issues` for "opentelemetry" or "telemetry" across `ocx-sh`, `grimoire-rs` and `michael-herwig` returns only #83 (V). No OBS-12 issue or PR exists in `ocx-sh` (V, `gh search prs "OBS-12"` empty).

### 3. This repo

1. **Issues.**
   - [#83](https://github.com/grimoire-rs/grimoire/issues/83) "Opt-in usage and error telemetry", created 2026-08-17, open, no comments. Shape: off by default; honours `DO_NOT_TRACK` and the host editor's setting; redirectable collector ("pointing it nowhere sends nothing"); payload is command name, version, platform, error kind; "Artifact names, registry hosts, workspace paths, and config values from private setups are never transmitted"; documented exact field list. No mention of OTel.
   - [#206](https://github.com/grimoire-rs/grimoire/issues/206) "otel support", created 2026-10-20, open, empty body, no comments, no labels.
   - No linked PR (`gh pr list --search`), no branch (`git branch -a`), no commit message mentioning either (V).
2. **Git pickaxe** (V). `-S telemetry`: 38 commits (listing capped at 30 per the brief); a path-limited run over `src/`, `Cargo.toml` and `test/` returns none, so no telemetry code ever landed or was removed. The sampled hits are `.agents/` research and skill text on vendor (Claude, Codex, Cline) telemetry from the harness-capability-freshness and hooks work, plus `91f521fd` (catalog-at-scale ADR's deferred-features line "usage telemetry / OpenTelemetry; search ranking"). `-S opentelemetry`: only `069bfe38`. `-S OTEL`: 6 commits, all hooks research; the text is in `.agents/research/hooks_vendor_reports/claude.md` and `openclaw.md` (vendors' OTEL behaviour), not grim exports. No rejected, reverted or parked telemetry attempt.
3. **`.agents/` references.**
   - `adr_vendor_support_tiers.md:289-291` (added `7dce918f`, 2026-08-25): "Re-check the Tier 1 set when usage telemetry lands (#83) — the first evidence-based promotion/demotion input grim will have."
   - `discovery/use-cases.yaml` (`c51aae35`, 2026-09-06): "There is no invocation telemetry (issue #83 is open)"; `X09` lists "telemetry (#83), download counts (#89)" as "open proposals, no shipped surface".
   - `adr_catalog_freshness_revalidation.md` and `adr_projection_over_index.md`: TUI background catalog refresh (not telemetry).
   - [#89](https://github.com/grimoire-rs/grimoire/issues/89) (download counts): reach signal via the registry or index `stats.json`, separate from client-reported usage.
4. **The current discussion** (untracked, `State: active`, 2026-10-22): no default endpoint; arm with a grim variable plus standard `OTEL_*`; `DO_NOT_TRACK` and `OTEL_SDK_DISABLED` off-switches; payload is #83's floor plus artifact refs. Its stated reason for a grim-specific gate is that "Claude Code sessions set [`OTEL_*`] for their own telemetry, and the per-tool-call hook dispatcher inherits them".
5. **Conflicting evidence on that reason.** Claude Code's monitoring docs say it "doesn't pass `OTEL_*` environment variables to the subprocesses it spawns, including the Bash tool, hooks, MCP servers, and language servers" (V, WebFetch of `code.claude.com/docs/en/monitoring-usage`; paraphrase-level, the page was read to character 100000 of 156659). The repo's own vendor report agrees: it "strips all `OTEL_*` exporter vars from every subprocess spawn" (V, `.agents/research/hooks_vendor_reports/claude.md:457`). The `env-vars` page's `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB` is a different, opt-in, credential-only scrub. The inheritance concern may still hold for other clients and for non-hook paths, which this lane did not test.
6. **Measured cost data that already exists in-repo (V).**
   - `.agents/hook_dispatch_latency.md`: no-match floor 3.2 ms p50 on WSL2.
   - `src/main.rs:255-272`: the 26.8 MB binary's cold page-fault cost is named, "the honest fix for the cold row is the binary's size". Current `target/release/grim` is 29.5 MB.
   - The multi-thread runtime was dropped to save ≈1.9 ms.
   - No pass/fail latency gate exists.
   None of this measures an OTel dependency.
7. **User-facing claims about telemetry.** None found in `docs/src`, `README.md` or `catalog/`. The only no-analytics statement is `docs/public/privacy.html` (`77558454`, 2026-09-07) and it covers the two websites, not the `grim` binary.

### 4. Recorded decisions on outbound calls and "phoning home"

1. **No decision names telemetry or phone-home for the CLI** (V). `arch-threat-model.md` has no outbound, egress, exfiltration or telemetry text. Its attackers are T1-T5 and its invariants I1-I6. I4 ("new execution capability ships off by default") and I6 ("secrets never in argv or the environment") are the nearest.
2. **OBS-13 (vendored MUST, `docs-and-tracing.md:100`).** "Panic handling makes zero network calls by default ... Remote crash reporting is opt-in behind explicit runtime consent — this is a package manager holding registry credentials." Verification: `rg 'sentry::init'` must be gated by a runtime config read. Grim has no `sentry`/`human_panic` (V, grep empty). This is the only written zero-by-default network rule near telemetry, and it covers crash reporting only.
3. **Env-var consent gates are a recorded no.** `AGENTS.md:112` (owner decisions 2026-08-17, commit `24a14bb`, and 2026-08-28; `adr_hook_workspace_consent.md`): hook consent and its record path are "deliberately unreachable from the environment" because a repository carries its own environment (`.envrc`, `.mise.toml`, devcontainer `containerEnv`, CI `variables:`), CWE-426. The discussion's gate (env var arms export) is a different decision, but a repo-supplied env could set an endpoint. The discussion already records the private-registry-refs counter.
4. **Credential-to-host discipline.**
   - `adr_artifact_ratings.md` D13 forbade index-supplied host data as a token-exfiltration vector.
   - `adr_index_declared_rating_host.md` (Accepted 2026-09-01) superseded it: a per-machine `GRIM_RATING_HOST` variable left "every self-hosted deployment ... misconfigured by default, and silently", and `GRIM_RATE_TOKEN` is gated by `--token-host`.
   - Precedent for both "env-only per-machine config silently fails fleets" and "credentials only go to a host the user vouched for".
5. **Product principles** (`product-context.md:76-85`): backend-friendly, **offline-first**, content-addressed, zero infrastructure cost, **private-first**. `GRIM_OFFLINE` = "Disable all network access" (`AGENTS.md` env table; `configuration.md:1285`). The offline check is applied per command, not at one chokepoint (per sibling recon, I).
6. **Existing outbound calls are all user-directed** (V, grep for `api.github.com` / `releases/latest` / `crates.io/api` in `src/`): registry pull and push, index fetch, `grim rate` (forge API, token), `--announce`, TUI background catalog/outdated checks. No update-check ping or analytics call found. Telemetry would be the first unsolicited egress to an operator-chosen host.

## Recommendation

None. This lane reports evidence for and against and does not choose. Open questions carried forward as leads: the OBS-12 exemption vehicle (a vendored rule cannot be edited locally, so an exemption must live in an ADR), and whether the claimed `OTEL_*` inheritance by hooks holds for any client.

## Sources

| Source | Type | Date | Relevance |
|--------|------|------|-----------|
| `/home/mherwig/dev/grimoire-lore` (`ocx-sh/grimoire-lore`) commit `30ef76e`, `.agents/research/rust-docs-observability.md`, `rust-docs-observability/tracing-and-observability.md`, `ecosystem-map.md`, `rules/rust-quality/docs-and-tracing.md` | Repo | 2026-08-16 | OBS-12 provenance |
| `/home/mherwig/dev/ocx` `.claude/rules/subsystem-ci.md:136-182`; `.claude/artifacts/research_deprecation_telemetry_patterns.md` | Repo | 2026-10-17 (HEAD); 2026-04-29 | CI OTLP export; telemetry stance |
| `/home/mherwig/dev/ocx-conventions/docs/contract.md`; `/home/mherwig/dev/ocx-mirror` | Repo | 2026-10 | Fleet CI telemetry; daemon candidate |
| `.claude/rules/rust-quality/docs-and-tracing.md:17-23,99,100`; `.claude/rules/arch-threat-model.md`; `AGENTS.md:112`; `.claude/rules/product-context.md:76-85` | Repo | 2026-10-22 | Rules and decisions here |
| `.agents/adr/adr_vendor_support_tiers.md:289`; `.agents/discovery/use-cases.yaml`; `.agents/adr/adr_index_declared_rating_host.md`; `.agents/hook_dispatch_latency.md`; `src/main.rs:255-272` | Repo | 2026-08..10 | Prior mentions; measurements |
| [grimoire-rs/grimoire#83](https://github.com/grimoire-rs/grimoire/issues/83), [#206](https://github.com/grimoire-rs/grimoire/issues/206), [#89](https://github.com/grimoire-rs/grimoire/issues/89) | Issue | 2026-08-17 / 2026-10-20 / 2026-08-18 | Requests |
| [docs.rs tracing-opentelemetry 0.34.0](https://docs.rs/tracing-opentelemetry/latest/tracing_opentelemetry/) | Docs | fetched 2026-10-22 | Stability warning |
| [open-telemetry/opentelemetry-rust README](https://github.com/open-telemetry/opentelemetry-rust) | Repo | fetched 2026-10-22 | Signal status table |
| [Claude Code monitoring-usage](https://code.claude.com/docs/en/monitoring-usage), [env-vars](https://code.claude.com/docs/en/env-vars) | Docs | fetched 2026-10-22 | `OTEL_*` not passed to hooks |
