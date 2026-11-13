# Goal: Telemetry: configure the collector without process-wide OTEL_* variables

Source: https://github.com/grimoire-rs/grimoire/issues/334 · Written: 2026-11-13 by /hex-loop

## Definition of done

- [x] only grim reads grim's telemetry destination and auth — evidence: commits 050999e0, a0bf1074: `test_grim_config_alone_arms_and_exports`,
  `test_grim_config_endpoint_gets_only_grim_headers`, `test_grim_telemetry_otlp_headers_supply_token` and the Rust
  `destination_unit_table` pass in the integration gate on tree d6925f2e.
- [x] deliverable by shipping a file plus environment variables, with no write to $GRIM_HOME or project files — evidence: commit a0bf1074: `test_grim_config_alone_arms_and_exports` starts from an empty `GRIM_HOME`, compares it with an
  env-armed twin run and checks the project is unchanged; docs `telemetry.md#config-file-fleet` (87c9199b, 8b00c650).
- [x] the user stays in control: own settings and DO_NOT_TRACK, OTEL_SDK_DISABLED, --offline still win — evidence: commits 050999e0, a0bf1074, f67b641c: `test_grim_config_opt_outs_win`, `test_opted_out_run_never_opens_grim_config`,
  `test_later_grim_config_file_wins`, `test_broken_grim_config_file_disarms_quietly`,
  `test_non_unicode_grim_telemetry_is_an_explicit_off` and the Rust `gate_truth_table_with_layers` pass on tree d6925f2e.
- [x] standard OTEL_* variables keep working when set for grim alone — evidence: the four pre-existing telemetry test files are unchanged since 93ada969 and their 57 tests pass on tree d6925f2e;
  `test_file_without_endpoint_uses_otel_endpoint` (a0bf1074).
- [ ] PR merge-ready (DONE block only) — evidence: the PR URL, and the
  PR is not a draft.
- [ ] CI green per job (DONE block only) — evidence: every check run on
  the PR head SHA with its conclusion; every skipped job states its
  skip reason (its `if:` or path filter).
  A skip with no reason is not green.
- [ ] Deep verify passed (DONE block only) — evidence: the full documented verification result.

## Autonomy

- Prompting: never — no question waits for a human; a doubt runs
  § Issue resolution.
- Granted acts (authority: the pasted prompt): none
- Forbidden or narrowed acts: None.

## Issue resolution

Every doubt — an ambiguous requirement, a design question, a failure
with an unclear cause — runs this protocol:

1. Delegate the research to a sub-orchestrator.
2. Record question → research → decision in this file or in the PR.
3. Defer to a GitHub issue only in hard cases — the research ends with no
   decision, or the decision needs an act outside the grants; the issue
   link is the recorded decision.
4. Every doubt not deferred ends in an action.
5. A pre-existing failure that blocks done is in scope and runs this
   protocol.
6. Findings outside this goal's scope become follow-up issues.
7. Secrets and credential-bearing logs are never written to any committed
   file, commit message, PR text, PR comment or review comment, or issue.
   Security findings are never filed as issues:
   this file records them by reference only — location and class, no
   secret value or exploit detail — and only the DONE block reports them
   in full.

## Loop shape

- Entry point: Run the /hex-plan skill on https://github.com/grimoire-rs/grimoire/issues/334.
- Refinement rounds: 2 — counts outer cycles: each review ⇄ execute
  pass is one, and so is every failed repair or retry cycle — a red
  gate's fix pass, an execute or finalize retry, a post-finalize CI fix ⇄
  re-finalize pass. Inner review-fix rounds do not count, and their limit
  is untouched.
  Past `2`, the DONE block reports every remaining criterion `not met`
  and the run stops.
- Ticks: /hex-loop commits nothing. The session creates or switches to
  the branch the pasted prompt's I9 names and commits this file first on
  it. A box is ticked only between hex-mode runs, never during one, and
  each tick is committed at once; every tick lands before the final
  /hex-finalize, none after it. The `(DONE block only)` criteria are
  evidenced only in the closing DONE block, never ticked.

## Rules

- Run rules:
  - Provenance: `/hex-loop` was invoked with no arguments on 2026-11-13,
    straight after a design conversation with the owner about this issue.
    The rules below record that conversation. They are not verbatim
    extras. The first group is what the owner stated or confirmed. The
    second group is what the orchestrator recommended and the owner did
    not object to.
  - Owner decisions. They replace the "B alone, sealed" recommendation in
    `.agents/research/research_otel_scoped_destination.md`:
    - `GRIM_CONFIG` is a new environment variable that holds a list of
      TOML config files, split with the platform path-list separator
      (`:` on Unix, `;` on Windows). The files merge as layers, and a
      later file wins over an earlier one.
    - Grim-specific telemetry settings live in a `[telemetry]` table in
      those files. They are additive with the standard `OTEL_*`
      variables, not a sealed replacement: a host-wide `OTEL_*` endpoint
      keeps working when no grim file sets one.
    - A `[telemetry]` table may arm the export, so a deployment needs
      only `GRIM_CONFIG` in the environment. `DO_NOT_TRACK`,
      `OTEL_SDK_DISABLED` and offline mode still disarm.
    - A `GRIM_CONFIG` file may later carry the `[options.experimental]`
      flags. A committed `grimoire.toml` can already set them, so they
      are not the gate. Consent records and artifact declarations never
      come from a `GRIM_CONFIG` file.
    - Reword the comment in `src/config/declaration.rs` that says no
      environment variable selects the hook feature, and the matching
      `AGENTS.md` environment-table row, so both name consent as the
      gate.
  - Orchestrator recommendations the owner did not object to. Revise one
    only with a recorded strong reason:
    - Version 1 reads only the `[telemetry]` table from `GRIM_CONFIG`
      files and ignores other tables with one warning. It auto-detects
      no system or user config file.
    - The destination is one unit. When any grim file sets `endpoint`,
      then headers, protocol and timeout come only from grim files and
      `GRIM_TELEMETRY_OTLP_HEADERS`, never from ambient `OTEL_*`.
      Resource attributes and the service name merge per key, with the
      grim file over `OTEL_*`.
    - Credentials are OTLP headers, given inline in `[telemetry.headers]`
      or in the one variable `GRIM_TELEMETRY_OTLP_HEADERS` (the OTel
      `k=v,k=v` format, merged over the file's headers). There is no
      `${VAR}` substitution, no `headers_file` key and no helper
      command: a repository can point `GRIM_CONFIG` at its own file, and
      any of the three would let that file send the user's secrets to an
      endpoint the repository chose. Header values never appear in
      `grim config` output or in a log line. Credentials inside the
      endpoint URL are refused.
    - An explicit falsy `GRIM_TELEMETRY` disarms a file that arms.

## Emphasis

- Open questions the plan decides and records: what an unparseable
  `GRIM_CONFIG` file does (telemetry failure must never change the exit
  code today); the stability class of `GRIM_CONFIG`, the `[telemetry]`
  table and `GRIM_TELEMETRY_OTLP_HEADERS`; where a later auto-detected
  user config file sits relative to `GRIM_CONFIG`.
- The telemetry ADR needs an amendment: it records env-only arming, no
  config-file form, and generic headers only to the base endpoint.
- Threat model: a repository-supplied environment can name a
  `GRIM_CONFIG` file (attacker class T3). Check the design against the
  recorded trade in the telemetry plan (decision 19, A-1).
- Unverified: whether an ocx patch can put its own install path into a
  variable value, which the deployment in the issue needs.

## Context

- Issue: https://github.com/grimoire-rs/grimoire/issues/334
- Research: .agents/research/research_otel_scoped_destination.md (prior art and options; its recommendation is replaced by § Rules)
- ADR: .agents/adr/adr_otel_usage_telemetry.md
- Earlier discussion and goal: .agents/discussions/otel-usage-telemetry.md, .agents/goals/otel-usage-telemetry.md
- Code: src/telemetry/config.rs (`from_env`, `resource_overrides`), src/telemetry/payload.rs, src/config/declaration.rs
- User contract: docs/src/content/docs/telemetry.md, docs/src/content/docs/configuration.md, docs/src/content/docs/stability.md
- Related issue: https://github.com/grimoire-rs/grimoire/issues/83
- Run record (2026-11-13): plan .agents/plans/plan_grim_config_telemetry.md (decisions PD-1…PD-22), ADR
  .agents/adr/adr_grim_config_layers.md, amendment in .agents/adr/adr_otel_usage_telemetry.md. /hex-review call 1 of 3
  ran inside /hex-execute: two rounds, converged, nothing open. Outer cycles used: 1 of 2.
- Follow-up filed (outside scope): https://github.com/grimoire-rs/grimoire/issues/356 — a FIFO at `$GRIM_HOME/grimoire.toml` hangs
  the global config read.
- Owed to CI: Windows proof of the `;` list separator and the `cfg(windows)` unit tests (this host has no Windows).
