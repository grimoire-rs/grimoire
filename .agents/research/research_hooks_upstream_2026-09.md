# Research: Upstream hook-system currency check for PR #98 (hooks artifact kind)

**Date:** 2026-09-28

**Question:** PR [grimoire-rs/grimoire#98](https://github.com/grimoire-rs/grimoire/pull/98)
(branch `origin/hex/hooks-artifact-kind`, created 2026-08-19) builds a `Hook`
artifact kind on specific upstream claims about client hook systems — event
names, config location/shape, trust/consent mechanism, matcher semantics,
response/verdict schema, plugin-bundled hooks — drawn from
[`adr_hooks_support.md`](../adr/adr_hooks_support.md),
[`adr_hook_workspace_consent.md`](../adr/adr_hook_workspace_consent.md), and
their cited research (`research_hooks_trampoline.md`,
`research_hooks_vendor_survey.md`, `hooks_vendor_reports/*.md`,
`research_hooks_codex_surface.md`, `research_hooks_autoexec_supply_chain.md`,
all dated 2026-08-14, on the PR branch only, not merged to `main`). Does the
real upstream state as of **2026-09-28** still match those claims? Report
matches, divergences, and any client that gained or lost hook support since
mid-August 2026. Stays neutral on the PR's design.

**Sources:** official vendor docs and changelogs fetched 2026-09-28 by four
parallel research passes; each citation below carries its own URL. Grim's own
current record: `.claude/rules/vendor-capability-watchlist.md` (rows verified
2026-09-27) and `.agents/research/research_ide_hooks.md` (2026-06-03, the
predecessor of the PR's own vendor survey).

---

## Per-client findings

### Claude Code — mostly match, one schema nuance unconfirmed

- Config locations, matcher semantics (empty/exact-or-pipe-list/else-regex),
  folder-level trust (no per-hook digest approval), `allowManagedHooksOnly`,
  plugin `hooks/hooks.json`, and fail-**open** on unexpected non-zero exit —
  all **MATCH** current docs ([code.claude.com/docs/en/hooks](https://code.claude.com/docs/en/hooks),
  fetched 2026-09-28).
- Event surface **grew** since the ADR's "30+" figure: ~35+ events now
  documented, including new `Setup`, `UserPromptExpansion`, `StopFailure`,
  `PostToolBatch`, `PermissionDenied`, `PreModelSwitch`/`PostModelSwitch`,
  `DirectoryAdded`. Grim's four canonical events (PreToolUse, PostToolUse,
  SessionStart, Stop) are unaffected; the native-event escape hatch
  (`<vendor>.event`) now has more targets than the ADR's research captured —
  additive, not a divergence.
- **Unconfirmed, worth a primary-source diff before implementation continues:**
  current docs' `PostToolUse`/`Stop` sections read differently than the ADR's
  C-004 table — `PostToolUse` appears to also accept
  `hookSpecificOutput.permissionDecision`/`additionalContext`/`updatedInput`
  alongside the ADR's claimed "top-level `decision:block`+`reason`" shape, and
  `Stop` in the fetched docs shows only `continue`/`stopReason`, not the
  `decision:block`+`reason` variant the ADR cites. This may be a summarization
  artifact of the fetch rather than a real change — flagged as **UNKNOWN**,
  not **DIVERGED**.
- Hardening since the ADR: subagent-frontmatter hooks now also require
  workspace trust (previously ran untrusted) — consistent with, not against,
  the ADR's framing.

### Codex CLI — match, parity work still active

- Config locations, hash-based trust + `/hooks` TUI approval,
  `--dangerously-bypass-hook-trust`, `requirements.toml`, union-and-warn on
  duplicate hook sources (`hooks.json` + inline `[hooks]`), `PreToolUse`/
  `PermissionRequest`/`Stop`/`SubagentStop` response fields — all **MATCH**
  ([learn.chatgpt.com/docs/hooks](https://learn.chatgpt.com/docs/hooks), fetched
  2026-09-28).
- [openai/codex#21753](https://github.com/openai/codex/issues/21753) ("Full
  Claude Code Hook Parity") is **still open**, 34 comments, last updated
  2026-09-17 — parity work continues after the ADR's 2026-08-14 citation, not
  closed or abandoned.
- Source-level claims (internal `ClaudeHooksEngine` type name, plugin
  `is_managed: false`, "needs two `config.toml` table registrations") are
  **UNKNOWN** — not verifiable from docs alone, need a repo-checkout diff
  against `openai/codex`.

### GitHub Copilot CLI — match on the safety-critical property, two real gaps closed by upstream since the ADR

- Config locations, `.claude/settings.json` compat (confirmed: Copilot reads a
  shared subset of Claude's hook config using Claude's event names/matchers),
  PascalCase aliasing, workspace-trust-free arming (no per-hook consent step) —
  **MATCH** ([hooks-reference](https://docs.github.com/en/copilot/reference/hooks-reference),
  [use-hooks](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/use-hooks),
  fetched 2026-09-28).
- **The load-bearing safety property still holds**: `preToolUse` fails
  **closed** on any non-zero exit or exit code 2 (denies even if stdout claims
  `allow`); only a timeout fails open. This is what makes grim's "launcher
  never signals failure through exit code" design (Decision G) actually safe
  on Copilot — confirmed current and unchanged.
- **DIVERGED (closes an open question in grim's favor):** the ADR's Open
  Question 1 — Copilot's `PreToolUse` mutator/rewrite field spelling was
  "unverified," so grim declined the `mutator` tier for Copilot at v1 — is now
  answered: the field is **`modifiedArgs`**, documented. The `Declined`
  decision could be revisited now that the field is confirmed.
- **DIVERGED:** `SessionStart` now documents an `additionalContext` field;
  the ADR's C-004 table marks it "NOT DOCUMENTED." That flag is stale.
- **DIVERGED (new scope surface, not a break):** Copilot's cloud/coding-agent
  product **now supports hooks** (reads `.github/hooks/*.json` from the cloned
  repo, `command`-only, no PowerShell, `notification`/`permissionRequest`
  don't fire, tool calls pre-approved). The ADR's Decision D7 explicitly scopes
  Copilot cloud-agent **out** of v1 — that exclusion is now a deliberate choice
  against a real, documented surface rather than a non-issue, and the plan
  text should say so explicitly before landing.
- One nuance beyond the ADR's research: a hook requesting
  `requestSandboxBypass: true` still forces an interactive confirmation even on
  an `allow` verdict — a partial exception to "no consent step at all," worth a
  line for `/security-auditor`.

### Gemini CLI — mechanism confirmed live, but the ADR's cited file name is wrong

- Events (11, same names), `BeforeTool` firing pre-execution with matcher and
  mutation semantics, parallel-by-default execution, millisecond timeouts, and
  response shape (`additionalContext` for context, `hookSpecificOutput.tool_input`
  for mutation) — **MATCH** ([geminicli.com/docs/hooks/](https://geminicli.com/docs/hooks/),
  fetched 2026-09-28).
- **DIVERGED, and this is the load-bearing precedent in the whole consent
  ADR:** the fingerprint-and-reprompt trust mechanism (`name:command`, warn on
  change) is confirmed **live and current** — but the store is
  `~/.gemini/trustedFolders.json` (override:
  `GEMINI_CLI_TRUSTED_FOLDERS_PATH`), **not** a separate `trusted_hooks.json`
  as both ADRs name it. `adr_hooks_support.md` Key Insight 6 and
  `adr_hook_workspace_consent.md`'s "Industry Context" section both cite
  `trusted_hooks.json` by name as Gemini's precedent for the content-trust
  axis. The mechanism the ADRs describe is real; the artifact name they use to
  describe it is not what upstream calls the file today (and may never have
  been — worth checking whether this was always a paraphrase or a real rename).
- `CLAUDE_PROJECT_DIR` compatibility-alias claim: **UNKNOWN** — not present on
  the fetched reference page; may live in a different doc section.

### Cursor — out of beta, one cited bug still open (not fixed, not a regression)

- Config locations, ~20 events, workspace trust, exit-code semantics, cloud-
  agent command-only hooks — **MATCH**, with additive new events since August
  (`postToolUseFailure`, `afterShellExecution`, `afterMCPExecution`,
  `afterAgentThought`, Tab hooks, `workspaceOpen`)
  ([cursor.com/docs/hooks](https://cursor.com/docs/hooks), fetched 2026-09-28).
- **DIVERGED:** Cursor hooks are **no longer flagged beta** in current docs —
  reads as a stable, fully documented feature (was "beta, v1.7 ~Oct 2025" per
  the PR's research).
- The `.claude/settings.json` importer is confirmed and now documented in more
  detail than the PR's research had: on by default, explicit PascalCase→camelCase
  table, with two named gaps (`Notification` and `PermissionRequest` have no
  Cursor equivalent, `Glob` matcher unsupported) — see
  [Third-Party Hooks](https://cursor.com/docs/reference/third-party-hooks).
- **Key Insight 3's "staff-confirmed open bug" (allow/ask verdicts not
  enforced) is STILL OPEN**, confirmed by a Cursor forum bug report dated
  **2026-09-25** — three days before this check — plus an open feature request
  asking for authoritative verdict enforcement. Not fixed; the ADR's
  characterization stands unchanged.

### OpenCode — diverged: not the config-splice hook mechanism the ADR implies

- OpenCode has **no native declarative `hooks.json`**. A feature request for
  native hooks ([anomalyco/opencode#14863](https://github.com/anomalyco/opencode/issues/14863),
  filed 2026-02-24) is **closed as "not planned."** What exists instead is a
  ~25-event **JS/TS plugin API**, with a community wrapper
  ([biszx/OpenCode-Hooks](https://github.com/biszx/OpenCode-Hooks)) that reads
  a `hooks.yaml` on top of it. If the PR's vendor survey counted OpenCode among
  the "15 of 17 clients with a hook mechanism" on the same footing as
  Claude/Codex/Copilot (config-file registration), that's a materially
  different (plugin-code) surface — worth cross-checking directly against
  `hooks_vendor_reports/opencode.md` on the PR branch, which this pass did not
  re-read.

### Kiro — match, still no documented trust model

- 11 events, JSON files in `.kiro/hooks/` (schema `"v1"`) — consistent with the
  PR's research. Hooks **activate automatically at session start with no
  consent/hash step** documented anywhere
  ([kiro.dev/docs/hooks/](https://kiro.dev/docs/hooks/), current as of
  2026-09-02 per the docs' own dating). The ADR's "security: not fully
  documented" characterization still holds.

### Qoder — new data, not covered by the PR's research at all

- Qoder (not in the PR's 17-client survey or its `hooks_vendor_reports/`) has
  a real, Claude-shaped hook system: 12 events (SessionStart, UserPromptSubmit,
  PreToolUse, PermissionRequest, PostToolUse, PostToolUseFailure,
  SubagentStart/Stop, Stop, SessionEnd, PreCompact, Notification), config at
  `.qoder/settings.json` (+ `.local.json`, `~/.qoder/settings.json`), and the
  same exit-code/JSON verdict schema (`hookSpecificOutput.permissionDecision`,
  `decision`, `reason`) as Claude. **No consent/trust model is documented** —
  just "must be executable and registered." Source:
  [docs.qoder.com/extensions/hooks](https://docs.qoder.com/extensions/hooks).
  If Qoder is a future v2 hook target, its trust posture matches the "no
  hook-specific consent at all" bucket the ADR already names for Cursor,
  Kiro, Antigravity, Amp, Goose, Cline, Kilo.

### Zed and Warp — match, both still without a lifecycle-hook system

- **Zed**: v1.8 (2026-06-24) shipped only `agent.terminal_init_command` (a
  single terminal-bootstrap hook), not the proposed general lifecycle-hooks
  system ([discussion #57943](https://github.com/zed-industries/zed/discussions/57943),
  still open and unshipped). Matches the ADR's "Zed has none."
- **Warp**: its documented "lifecycle events" (`loop.started`, `warp.run.*`,
  etc.) are internal orchestration signals for Cloud Agents/webhooks, not a
  user-authorable hook config. No `hooks.json`-equivalent found. Matches the
  ADR's "Warp has none."

### Antigravity — newly gained capability, narrower than a new event system

- v2.17.0, **2026-09-22** — after the PR's 2026-08-14 vendor survey — added a
  `hooks:` key to custom-agent frontmatter. It lets an agent **reference or
  bundle existing hook files by path**; it is not a new lifecycle-event system
  of its own. Source: [antigravity.google/changelog](https://antigravity.google/changelog/).
  Grim's own current watchlist
  (`.claude/rules/vendor-capability-watchlist.md`, row verified 2026-09-27)
  already tracks this correctly as an agent-registry addition, not a hook-kind
  capability change — consistent with how the PR's ADR would need to treat it.

### Others (brief pass) — no material change found

- **Goose**: documents lifecycle hooks via an "Open Plugins spec"
  (`hooks/hooks.json` under `.agents/plugins/<name>/`, ~12 events) — matches
  the ADR's citation of Goose PR #9304 following Claude Code's hook
  conventions. **MATCH**.
- **Cline**: 10 named hook-file events under `.clinerules/hooks/`, consistent
  with prior state. **MATCH**, with a live reliability caveat — an open bug
  ([cline/cline#14593](https://github.com/cline/cline/issues/14593)) reports
  plugin lifecycle hooks not firing.
- **Amp, Kilo, Droid, Junie, OpenClaw**: not deep-dived; no signal found
  suggesting a status change since mid-August 2026. **UNKNOWN/MATCH by
  default** — not independently re-verified here.

---

## Net assessment

No client **lost** hook support since mid-August 2026, and no wholly new
hook-capable client appeared in the v1-adjacent set — Qoder is new *data* (not
in the PR's research) rather than a new capability that shipped since the PR.
Antigravity gained a narrow, agent-level hook-file reference capability
(2026-09-22), already correctly scoped by grim's own current watchlist as
something other than a hook-kind change.

The two findings most worth a maintainer's attention before the PR lands:

1. **Gemini's cited trust-store filename (`trusted_hooks.json`) does not match
   the real file (`trustedFolders.json`)**, in the one precedent both ADRs lean
   on hardest for the workspace-consent design. The underlying
   fingerprint-and-reprompt mechanism is real and current; only the artifact
   name in the ADR text is wrong.
2. **OpenCode has no native `hooks.json`** — a "not planned" closed feature
   request, plugin-API-only. If the PR's 15/17 vendor count treats OpenCode as
   a config-splice hook client on par with Claude/Codex/Copilot, that count
   needs re-checking against `hooks_vendor_reports/opencode.md` on the PR
   branch.

Everything else is either an unchanged match, upstream growth that is additive
to the ADR's four canonical events, or a genuinely open (not newly regressed)
issue the ADR already characterized correctly (Cursor's allow/ask
non-enforcement, still open as of a 2026-09-25 forum report).
