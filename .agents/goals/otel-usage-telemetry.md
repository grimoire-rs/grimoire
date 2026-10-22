# Goal: Primitive OTEL usage telemetry

Source: .agents/discussions/otel-usage-telemetry.md · Written: 2026-10-22 by /hex-loop

## Definition of done

- [x] R1 env-only opt-in: GRIM_TELEMETRY arms, OTEL_* configure, OTEL_SDK_DISABLED/DO_NOT_TRACK disarm — evidence: code 6b2c2b6b, gate `task --force verify` green (nextest 4298/4298, acceptance 1845 passed); tests: test_disarm_switches_win[*], test_base_endpoint_gets_generic_headers, test_signal_endpoint_never_gets_generic_headers, test_no_endpoint_sends_nothing, unit gate_truth_table_arms_only_for_switch_without_any_opt_out.
- [x] R2 no export call without GRIM_TELEMETRY, even with OTEL_* set — evidence: code 6b2c2b6b, gate `task --force verify` green (nextest 4298/4298, acceptance 1845 passed); tests: test_unarmed_sends_nothing_with_ambient_otel.
- [x] R3 CLI commands only; hook run never exports; mcp one session record — evidence: code 6b2c2b6b, gate `task --force verify` green (nextest 4298/4298, acceptance 1845 passed); tests: test_hook_run_never_exports, test_mcp_session_sends_one_record, unit telemetry_is_never_built_before_the_hook_runtime_returns.
- [x] R4 events only: grim.command + grim.artifact.changed per artifact, one POST — evidence: code 6b2c2b6b, gate `task --force verify` green (nextest 4298/4298, acceptance 1845 passed); tests: test_one_post_per_invocation, test_multi_record_single_post, test_install_emits_one_event_per_changed_artifact, test_add_emits_added, test_update_emits_updated.
- [x] R5 resource: service.*, os.type, host.arch, grim.installation.id, OTEL_RESOURCE_ATTRIBUTES — evidence: code 6b2c2b6b, gate `task --force verify` green (nextest 4298/4298, acceptance 1845 passed); tests: test_resource_attributes_exact, test_otel_resource_attributes_and_service_name_override, test_operator_vcs_override.
- [x] R6 uuid v4 installation ID under $GRIM_HOME, created only when armed — evidence: code 6b2c2b6b, gate `task --force verify` green (nextest 4298/4298, acceptance 1845 passed); tests: test_installation_id_absent_when_unarmed, test_installation_id_stable_across_armed_runs, identity unit tests.
- [x] R7 vcs.* only with GRIM_TELEMETRY_REPOSITORY=1: redacted origin, project scope, no home repo — evidence: code 6b2c2b6b, gate `task --force verify` green (nextest 4298/4298, acceptance 1845 passed); tests: test_vcs_from_credentialed_origin, test_no_vcs_and_no_git_spawn_without_opt_in, test_no_vcs_for_global_scope, test_no_vcs_for_home_repo.
- [x] R8 never collected: branch, commit, author, host, user, cwd, argv — evidence: code 6b2c2b6b, gate `task --force verify` green (nextest 4298/4298, acceptance 1845 passed); tests: test_never_collected.
- [x] R9 URLs redacted (OBS-22), semconv names (OBS-20), no raw args or paths — evidence: code 6b2c2b6b, gate `task --force verify` green (nextest 4298/4298, acceptance 1845 passed); tests: repository_url_vectors, source_annotation_strings_for_normalized_remotes_are_pinned, test_wire_keys_match_documented_field_table, test_debug_line_never_carries_header_values.
- [x] R10 export failure never changes the exit code or prints at default level — evidence: code 6b2c2b6b, gate `task --force verify` green (nextest 4298/4298, acceptance 1845 passed); tests: test_export_failure_never_changes_outcome[refused|503|hang x success|exit-79].
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

- Entry point: Run the /hex-plan skill on "Primitive OTEL usage telemetry, per .agents/discussions/otel-usage-telemetry.md".
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
  - None.

## Emphasis

None.

## Context

- Discussion: .agents/discussions/otel-usage-telemetry.md (Decisions, Open questions with recommendations, Verification)
- Research: .agents/research/research_otel_*.md (8 files, 2026-10-22)
- Tracking issue: https://github.com/grimoire-rs/grimoire/issues/206
- Related issue: https://github.com/grimoire-rs/grimoire/issues/83
- First deliverable per the discussion: an ADR under .agents/adr/ recording the OBS-12 exemption for the opentelemetry-otlp crate
- 2026-10-22 — R1–R10 ticked after /hex-execute of `.agents/plans/plan_otel_usage_telemetry.md` (State review; gate green on 6b2c2b6b; Windows `-k telemetry` 52 passed / 3 POSIX-only skipped; collector interop on otelcol-contrib 0.163.2; review tier high converged, no open Block/Warn; cross-model leg skipped — codex usage limit).
- Security trade by reference: plan decision 19 / A-1 (`src/telemetry/` export config, class: repo-supplied env can redirect export with ambient generic OTLP headers) — accepted for v1 pending a threat-model egress ruling; owner decision.
- Pre-existing on main, out of scope: `task docs:check` final `npm audit --audit-level=high` step (docs/package-lock.json, dependency-advisory class) — owner decision.
