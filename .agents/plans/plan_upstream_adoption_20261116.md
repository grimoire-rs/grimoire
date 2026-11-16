# Plan: adopt the 2026-11-16 upstream findings

## Status

- **Plan:** plan_upstream_adoption_20261116
- **Active phase:** 6 — docs and records
- **Step:** /hex-execute → Implement
- **Last update:** 2026-11-16 (after 6c6d3b9f: fix(kilo): decline a global agent whose body holds an expansion token)


## Context

`/upstream-refresh` swept all 23 targets on 2026-11-16 and committed the
ledgers on `feat/upstream-adoption-20261116` (4 commits, unpushed, equal to
`origin/main` plus those). The sweep reported findings from vendor docs only.
Three read-only verifiers then checked each finding against grim's code and
re-fetched the upstream source. This plan implements what survived, on that
same branch, and ends with one linear-history PR that is ready to merge.

Owner decisions (2026-11-16): everything additive; both path moves now; Droid
issuer field with a warning; `product-context.md` facts and re-rank.

## What verification changed

| Finding | Verdict | Consequence |
|---|---|---|
| Antigravity MCP SSE / `httpUrl` | not real: grim writes `serverUrl` only | dropped |
| Amp `${amp:id-token}` | not real: passes through as Amp intends | dropped |
| Kiro `agentEngine` `v1` | not real: excluded on purpose, retired upstream | dropped |
| Goose `GOOSE_PATH_ROOT` wording | not real: grim's text is right | dropped |
| OpenCode V2 ignores `OPENCODE_CONFIG` | refuted: `server-process.ts:132` reads it | sweep records corrected |
| OpenCode V2 settings keys | all still accepted, only deprecated | watchlist note, no code |
| Cursor `${PLUGIN_ROOT}` | not real: `family::agent_plugins_mcp_entry` has one caller, `stage::mcp_value`, so `${CURSOR_PLUGIN_ROOT}` already applies on both export paths | dropped |
| Kilo agent-body expansion | global scope only; OpenCode does not expand | narrow fix, landed in `6c6d3b9f` |
| Copilot sandbox-path expansion | version is 1.0.109; grim has no such row | note only |
| Qoder env expansion in settings | real, but upstream states no syntax | deferred, needs a CLI probe |

## Work packages

Serial, in this one worktree (one `target/`, no merge step, history stays
linear). One sonnet worker per package, strict brief, `model: sonnet` with a
`Model rationale:` line. Workers do not commit: the commit hook needs a
`task verify` stamp under 300 s old, so after each package I run
`env -u OCX_GLOBAL task verify` and commit. A heartbeat watcher runs beside
every worker. A package that fails twice on sonnet goes to opus.

Every brief carries: the subsystem rules to read first, failing test first
for a fix, the exact file list, Principle 9 (additive only; an installed
artifact must still report `installed` after re-materialize), reuse of the
named existing pattern, `cargo fmt` + targeted `cargo test`, and a reply of
files changed, tests added, deviations.

**WP0 — setup (me).** Rename the branch to `feat/upstream-adoption-20261116`.
Copy this plan to `.agents/plans/plan_upstream_adoption_20261116.md` with the
`## Status` block.

**WP1 — export fixes.** Landed in `e36551e8`; the Cursor item below was dropped (not real).
- `src/export/marketplace_export.rs:890-894` (`EntryDoc`): stop writing
  `documentationUrl` / `supportUrl` on marketplace entries for Claude
  only (Qoder entries are unchanged: nothing upstream says Qoder rejects them);
  the Claude `plugin.json` now carries them (`family.rs`, `stage.rs`). Update the test at `:1803-1820` and the
  e2e assertions at `:2752-2782`.
- ~~`src/export/family.rs:405-412` Cursor `${CURSOR_PLUGIN_ROOT}`~~ dropped:
  `stage::mcp_value` is the only caller and already emits it.

**WP2 — settings rows.** Landed in `58754de8` (318 rows). Pattern: `SettingsField` rows in
`src/install/vendor_<harness>.rs`; each change also edits the vendor unit
test, the value cell in `docs/src/content/docs/settings.md`, and the golden
schema `src/command/testdata/settings.schema.json` (regenerate with
`grim schema --kind settings`, then the `docs/public/schemas/` copy).
- New literals: Codex `/tui/notification_method` + `osc777`
  (`vendor_codex.rs:197`); Copilot `/effortLevel` + `none`, `minimal`, `max`
  (`vendor_copilot.rs:95`); Antigravity `/agentMode` + `""`
  (`vendor_antigravity.rs:91`, flip the tests at `:985-1018`).
- Widening: Kiro `/chat.showThinking` from `Ty::Bool` to
  `Ty::OneOf([Bool, Enum(expanded, collapsed, off)])` (`vendor_kiro.rs:114`;
  precedent `vendor_opencode.rs:110-115`). A boolean must render as before.
- Claude project `/permissions/defaultMode`: add `dontAsk`, `manual` to
  `project_values` (`vendor_claude.rs:220-236`), plus a case in
  `test/tests/test_settings_scope.py`. The `claude` binary is on this host:
  run the credential-free settings check from the skill's `deep-pass.md`
  first; if it cannot prove the values, drop this item and say so.
- New rows, appended at the end of the vendor slice: Cursor
  `/network/useHttp1ForAgent` (Bool, global; copy `/hints`,
  `vendor_cursor.rs:115`) and Kilo `/memory_model` (String; copy
  `/small_model`, `vendor_kilo.rs:158`). Also `SETTINGS_GOLDEN` and the count
  316 → 318 in three places in `src/install/vendor.rs` (`:1869`, `:2317`,
  `:2318`, `:2363`).

**WP3 — MCP fields.** Landed in `c1b453f2`. Rules: `subsystem-install-layout-agents-mcp.md`,
`adr_mcp_oauth_projection.md` (lossless-or-skip).
- Droid: add optional `authorization_server_issuer` to `McpOAuth`
  (`src/oci/mcp.rs`), write `oauth.authorizationServerIssuer`
  (`vendor_droid.rs:299-312`). `client_id` without an issuer still installs
  as today, with a new warning. Descriptor schema golden, `mcp-servers.md`,
  `catalog/skills/grim-authoring/references/mcp-spec.md`.
- Copilot: write `oauthScopes` when `client_id` is set (add
  `OAuthField::Scopes` to Copilot's allow-list).
- Junie: write the per-server `timeout` in ms (`vendor_junie.rs:~204`;
  precedent: Codex `timeout` → `startup_timeout_ms`, commit `81dbed05`).
- One self-heal test each for Copilot and Junie, shape copied from
  `test/tests/test_mcp_artifact.py` / `test_fix_followups.py`.

**WP4 — vendor-metadata keys.** Landed in `4eafa219` for the Copilot agent keys
and Claude `autoCompactWindow`; the Copilot skill keys were NOT implemented (a
vendor with `skill_fields` leaves the shared skills pool, and Copilot is in
`POOL_CAPABLE_VENDORS`: breaking; owner decision, issue draft). Registry + parity-test pattern:
`vendor_copilot.rs:42-74`, `vendor_claude.rs:109-190`, doc parity as in
`vendor_kilo.rs:477-489`; each key gets its row in
`docs/src/content/docs/vendor-metadata.md`.
- Copilot skill keys `argument-hint`, `user-invocable`,
  `disable-model-invocation`.
- Copilot agent keys `include-custom-instructions`, `reasoningEffort`,
  `models`, `modelPolicy` (`mcp-servers` stays out: recorded non-goal,
  `vendor_copilot.rs:54-55`).
- Claude agent key `autoCompactWindow`.
- The worker fetches each key's type from the vendor page first; a key the
  existing field spec cannot express is deferred with the reason.

**WP5a — Kiro root and Kilo hardening.** Kiro landed in `10dd77ae` for `/`, `$HOME`
and ancestors only: a relative `KIRO_HOME` is still followed, because `settings.md`
and `json-interface.md` document `KIRO_HOME=kiro-rel` as the `settings-root-relative`
example (owner decision, issue draft). Kilo landed in `6c6d3b9f`.
- `kiro_root` (`vendor_kiro.rs:315-317`): a relative value, `/`, `$HOME` or
  an ancestor of `$HOME` falls back to `~/.kiro` (upstream rule for V3;
  precedent `vendor_openclaw.rs:181-190`). Extend the test at `:364`.
- Kilo (`vendor_kilo.rs:294-298`): a global-scope agent whose body holds
  `{env:` or `{file:` is declined with a warning, through the existing
  per-artifact decline path. Project scope is untouched (Kilo does not
  expand there). Token matching reuses `token()` in `src/oci/settings.rs:624`.

**WP5b — OpenCode config file.** STOPPED at its gate; not landed. Implemented, tested
and reverted: `reap_relocated_roots` keys only on `PathAnchor::VendorRoot` and
`ClaudeUserDir`, OpenCode MCP and agent outputs anchor at the fixed `OpenCodeRoot`
(skipped), and the reaper excludes `ArtifactKind::Settings` by design, so neither old
XDG entry kind is reaped. Owner decision needed; issue draft in
`research_upstream_opencode_20261116.md`. Gate first: confirm from V1 source
(`anomalyco/opencode` at `v1.19.5`) that V1 reads
`$OPENCODE_CONFIG_DIR/opencode.json[c]`. If it does not, stop and report.
- `src/install/opencode_config.rs:70-80`, `:124-133`: order becomes
  `$OPENCODE_CONFIG`, then `$OPENCODE_CONFIG_DIR/opencode.json[c]` when the
  directory is set, then the XDG file.
- Layout move per `adr_render_layout_stability.md`: state migration, a reaper
  that removes grim's managed entries from the old XDG file (precedent
  `reap_moved_outputs`), an upgrade fixture, and a self-heal test. Tests
  beside `global_config_path_resolution_order` (`opencode_config.rs:443`).

**WP6 — docs, catalog, records (after the last code change).**
- Docs wording: Kiro inclusion modes (`clients.md:337-355`) with
  `catalog/skills/ai-config-authoring/references/rule-design.md:108-114`; Amp
  scoping (`clients.md:501`); Cline skill order (`vendor-metadata.md:468`);
  Kilo "legacy" (`clients.md:612`, `vendor-metadata.md:471`); GitLab token
  expiry (`ratings.md:306`); Claude sandbox limit (`settings.md:979-985`);
  credential variables (`mcp-servers.md:346-351`); OpenClaw registered
  projects (`clients.md:604`); OpenCode V1/V2 (`clients.md:447`,
  `configuration.md:1439`, `vendor-metadata.md:487`); skill-name wording
  (`artifacts.md:145`); trust-gate notes for Junie, Zed and `codex exec`.
- `docs/src/content/docs/upgrading.md`: entries for the Kiro fallback, the
  OpenCode config move, the Kilo decline and the export changes.
- Catalog drift review of `grim-usage`, `grim-authoring`,
  `ai-config-authoring` for every key, row and path that landed.
- Source comments that state the corrected facts (`vendor_cline.rs:11`,
  `vendor_kilo.rs:15-21`, `vendor_claude.rs:219,267`, `vendor_openclaw.rs:8-17`,
  `vendor_opencode.rs:13-16,160,436`, `vendor_amp.rs:124`) and
  `re-verified 2026-11-16` on the cleared stamps (goose, kilo, cursor,
  gemini, zed, codex).
- Records: watchlist rows for everything landed and for the corrected sweep
  errors; `AGENTS.md` env table (`KIRO_HOME`, OpenCode rows); the matching
  `subsystem-*.md` rules; trust-gate keys added to `domains.md:84-91` and
  ADR Appendix A; `## Verifier sign-off` in the affected research artifacts
  and the corrected OpenCode draft; bold `**Expires:**` in the four research
  files the stale report cannot read.
- `.claude/rules/product-context.md` and
  `.agents/research/research_promotion_positioning.md`: comparable-tool
  facts, a skillctl row, a new `re-verify after` date, and the re-ranked
  differentiators, following that file's Update Protocol.

**WP7 — review, one round.** One fresh sonnet reviewer on the code diff
against `origin/main`: Principle 9 first, then correctness and tests,
findings with `file:line` and Block / Warn / Suggest. One sonnet fixer for
functional defects only; docs and hardening findings are listed, not looped.

**WP8 — finalize and PR (me).** Fold review fixes into their commits
(`--fixup` + `rebase --autosquash`), rebase on `origin/main` if it moved,
then `task verify`, `task docs:check`, `task catalog:verify`, `task git:dco`.
Push the branch, open the PR (`## Summary`, `## Test plan`, deferred items),
watch CI, rerun infrastructure flakes once. Windows proof comes from CI:
this host has no `/mnt/c`.

## Resulting history (after the 4 sweep commits)

As `git log --oneline origin/main..HEAD`, oldest first, plus the pending records:

1. `4f302b0f` `chore: add the upstream adoption plan`
2. `e36551e8` `fix(export): move Claude doc and support links into plugin.json`
3. `58754de8` `feat(settings): accept the values upstream added and add two rows`
4. `c1b453f2` `feat(mcp): write the Droid issuer, Copilot scopes and Junie timeout`
5. `4eafa219` `feat(vendor): lift the new Copilot and Claude agent frontmatter keys`
6. `10dd77ae` `fix(kiro): fall back to ~/.kiro for an unusable KIRO_HOME`
7. `6c6d3b9f` `fix(kilo): decline a global agent whose body holds an expansion token`
8. pending: `docs: describe the adopted upstream changes`
9. pending: `chore: update the rules, ledgers and positioning records`

Dropped from the earlier list: the Cursor export commit (not real) and the OpenCode
config-file commit (WP5b stopped at its gate).

## Not in this PR

- Qoder env expansion in settings values: no documented syntax; needs a CLI.
- Hooks findings (Junie, Antigravity, Goose, Codex): findings only by rule.
- Kiro extra rows: session recap bills one model call per turn, the rest
  cannot be validated.
- Amp rules via `globs`, Droid `enabledTools` and folder scope, Goose plugin
  skills, Antigravity agent `model` validation, Junie `effort` alias.
- Deep passes for `claude`, `opencode`, `copilot`, `codex`.
- Decided not to implement, each an issue draft with a watchlist row: OpenCode
  config-file move (reaper), Copilot skill keys (leave the pool), relative `KIRO_HOME`.
- The issue drafts stay in the research artifacts; none is filed.

## Verification

- Per package: the worker's targeted `cargo test`, then
  `env -u OCX_GLOBAL task verify` before each commit.
- Schema: `grim schema --kind settings` matches the golden and shows the new
  literals and rows; the MCP descriptor schema shows the issuer field.
- Stability: each self-heal test installs with the old layout or key set,
  re-materializes, and asserts `grim status` reports `installed`; the
  OpenCode upgrade fixture (dropped with WP5b) would have asserted the old XDG file lost grim's entries.
- Export: the marketplace e2e test shows no entry-level link keys for Claude,
  and the Claude `plugin.json` carries them.
- Final: `task verify`, `task docs:check`, `task catalog:verify`,
  `task git:dco`, `python3 .claude/skills/upstream-refresh/scripts/upstream_stale.py report`,
  then green PR CI including the Windows jobs.
