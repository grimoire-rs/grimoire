# Plan: Conclude the 2026-09-27 upstream follow-ups

## Status

- **Plan:** plan_upstream_followups_2
- **Active phase:** 3 — Integration (complete)
- **Step:** squashed to one commit on `feat/upstream-followups`, awaiting owner push/merge
- **Last update:** 2026-09-27 (WPs A–I landed, final adversarial + cross-model review fixes applied; `task verify`, `docs:check`, Windows acceptance green)

---

## Overview

**Status:** In Progress
**Author:** maintainer
**Date:** 2026-09-27
**Beads Issue:** N/A — GitHub issues [#139](https://github.com/grimoire-rs/grimoire/issues/139), [#141](https://github.com/grimoire-rs/grimoire/issues/141)–[#149](https://github.com/grimoire-rs/grimoire/issues/149), [#151](https://github.com/grimoire-rs/grimoire/issues/151), [#152](https://github.com/grimoire-rs/grimoire/issues/152), [#155](https://github.com/grimoire-rs/grimoire/issues/155), [#156](https://github.com/grimoire-rs/grimoire/issues/156)
**Related PRD:** N/A
**Related ADR:** [adr_mcp_oauth_projection.md](../adr/adr_mcp_oauth_projection.md)

## Objective

[#160](https://github.com/grimoire-rs/grimoire/pull/160) shipped the safe fixes of the first upstream sweep.
14 of its issues stay open, holding 19 findings marked *design*, *breaking* or
*skipped* (`.agents/upstream-followups-20260927.md`). This plan concludes every
one of them. The work runs in parallel worktrees, lands on
`feat/upstream-followups`, and is squashed to **one commit**. Nothing is
pushed, and no issue closes before the merge.

Two facts settle the earlier "breaking / not opt-in" verdicts:

- `docs/src/content/docs/stability.md#unstable` puts vendor render layout
  outside the frozen contract. A newly honored vendor env var has precedent
  (`KIRO_HOME`, `GEMINI_CLI_HOME`) as long as the reaper, the upgrade fixture
  and the upgrading note ship with it.
- A Declined→Native kind flip, or a newly written MCP entry, is documented
  drift: `outputs_pending` cause #1, healed by the next install. The Copilot
  global-MCP change `c76d25f2` did exactly this.

## Scope

### In Scope

Every row of the decisions table below.

### Out of Scope

- Oauth projection for Codex, Kiro, Cursor, Qoder, Amp, Antigravity, Gemini, Warp.
- Object-valued Kilo `permission` (waits on `FieldType::Json`).
- Any push, merge or issue close.

## Key Decisions

| # | Finding | Decision |
|---|---|---|
| 139 | Antigravity Rule kind | **Enable.** Project `.agents/rules/<n>.md`, global `~/.gemini/config/rules/<n>.md`. Non-empty `paths` → `trigger: glob` + `globs: "<comma-joined>"` (reuse the Cursor join, `vendor_cursor.rs`). Empty `paths` → `trigger: always_on`. |
| 141a | Claude kinds (commands, output styles, LSP, workflows, themes, monitors) | **Decline all.** Commands merged into skills upstream; output styles are Claude-only; the rest are plugin-only, and workflows are executable JS (class 3). Record in the Claude section of `clients.md`; watchlist revisit trigger for output styles (a second harness, or a user asks). |
| 141b | `CLAUDE_CONFIG_DIR` via settings `env` | **Honor.** Order: shell env > managed-settings `env` > `~/.claude/settings.json` `env` > `~/.claude`. Claude ≥ 2.1.251 ignores project/local `env`, so grim reads only those two files. Re-verify shell-vs-settings precedence before coding. `relocated_vendor_roots` row plus the KIRO_HOME fixture set; the managed path sits behind an injectable lookup. |
| 142 | Cline MCP | **Enable at global scope** (no project MCP upstream). File: `CLINE_MCP_SETTINGS_PATH` > `$CLINE_DATA_DIR/settings/cline_mcp_settings.json` > `<cline root>/data/settings/cline_mcp_settings.json`. Env refs render `${env:VAR}` (reuse Copilot's `translate_env_refs`), flat entry form. grim joins Cline's lock: create `<file>.lock` dir holding `owner.<pid>.<ts>.<uuid>` (stage then rename), reclaim a lock older than 10 s, write atomically, release. Oauth always skipped (shape unverified). Check whether `CLINE_DIR` moves skills upstream: if yes, honor it for the whole root with a reaper row; if not, MCP only. |
| 143a | Codex `agents/openai.yaml` sidecar | **Decline permanently.** It would sit in the shared `.agents/skills` pool and break `pool_vendors_render_byte_identical_skill_bytes` and the empty-registry contract. Codex section + watchlist. |
| 143b | Codex `http_headers_helper` | **Decline permanently.** Cleared env, cache until 401/403, explicit `Authorization` wins — a shared descriptor key would misbehave silently. Same docs as 143a. |
| 144a | Copilot project MCP never reaches the CLI | **Second project output `.github/mcp.json`**, keep `.vscode/mcp.json`. Shape `mcpServers`, refs verbatim `${VAR}` like global. Uses `Vendor::mcp_config_paths` (WP0). Document the `.mcp.json`-wins precedence when Claude is also selected. |
| 144b | Copilot oauth | **Lossless-or-skip:** `client_id` → `oauthClientId`. `callback_port` is unmapped (`auth.redirectPort` is changelog-only). |
| 145a | Droid Agent kind | **Enable** `.factory/droids/<n>.md` at both scopes; fix the stub path (today `.factory/agents/`). `tools` → YAML list via `render::comma_list_value`. Registry key `droid.reasoning-effort` (Enum low/medium/high). A name with `.` is skipped with a warning (`NameGrammar::NoDot`). |
| 145b | Droid MCP | **Enable** `.factory/mcp.json` / `~/.factory/mcp.json`, `mcpServers`. `${VAR}` allowed only in `env`, `headers`, `oauth.clientId`; a ref in `command`, `args` or `url` skips the server with a warning. Oauth `clientId` only. Document that toggling a project server in Droid's UI copies it to global, where a later global install of the same name hits the exit-65 guard. |
| 146 | Gemini `timeout` bounds every tool call | **Stop projecting.** Drop `timeout` for Gemini with a warning; Gemini's 10-min default applies. Self-heal test + upgrading note. |
| 147 | Goose Agent kind | **Enable** `.goose/agents/<n>.md` at both scopes (Goose-only path, avoids the Antigravity `.agents/agents/` collision). Fields `name`, `description`, `model`; `tools` dropped with a warning; empty registry. Watchlist: compat path, revisit if deprecated. |
| 148a | Junie Agent kind | **Enable** `.junie/agents/<n>.md` at both scopes. `tools` → YAML list. Registry `junie.permission-mode` (Enum), `junie.reasoning-level` (Enum low/medium/high), `junie.max-turns` (Integer). Name outside `[a-z][a-z0-9_-]*` skipped with a warning (`NameGrammar::LeadingLetterNoDot`). |
| 148b | `JUNIE_HOME` | **Honor**, replace shape (`$JUNIE_HOME` replaces `~/.junie`) for skills, agents, MCP and detection. Reaper row + KIRO_HOME fixture set. |
| 149 | Kilo Agent kind | **Enable** project `.kilo/agents/`, global `~/.config/kilo/agents/` (check `XDG_CONFIG_HOME`). Reuse the OpenCode renderer: filename is the identity, `name` and `tools` dropped. `kilo.*` registry holds only the OpenCode scalar keys re-verified against Kilo v7.8.1. |
| 151 | `OPENCLAW_HOME` | **Honor**, home-replace shape `$OPENCLAW_HOME/.openclaw`. `OPENCLAW_STATE_DIR` wins if the docs confirm it. Reaper row + fixtures. |
| 152 | OpenCode oauth | **Lossless-or-skip:** `clientId`, `scope` (space-joined), `callbackPort`. A set `auth_server_metadata_url` skips. |
| 155 | Warp MCP | **Enable** `.warp/.mcp.json` / `~/.warp/.mcp.json`; stdio `command`/`args`/`env`/`working_directory`, URL `url`/`headers`. Any `${VAR}` ref skips the server (expansion unverified). Document Warp's optional `.mcp.json` read (double-registration risk). |
| 156 | Zed oauth | **Lossless-or-skip:** `client_id` only. |

**Oauth rule** (owner, lossless-or-skip): [adr_mcp_oauth_projection.md](../adr/adr_mcp_oauth_projection.md).

## Backwards Compatibility

Principle 9 applies to every package. What each kind of change owes:

| Change | Classification | Owed |
|---|---|---|
| Kind flip Declined→Native (139, 145a, 147, 148a, 149) | Documented drift, heals on install | `upgrading.md` note; matrix row; acceptance render test |
| Newly written MCP entry (142, 144a, 145b, 155; oauth 144b, 145b, 152, 156) | Documented drift | `upgrading.md` note naming the exit-65 case for a same-named hand-authored entry |
| Newly honored env root (141b, 148b, 151) | Layout move | `relocated_vendor_roots` row, reaper, upgrade fixture (reap unmodified, keep edited, un-splice stranded MCP entry, uninstall path) |
| Renderer output change (146) | Renderer change | Self-heal proof: install on the old render, reinstall, `status` not-modified |
| Stub path fix (145a `.factory/agents/` → `.factory/droids/`) | Kind was Declined, nothing recorded there | None beyond the kind flip |

No state schema, lock, config or JSON report field changes. `PathAnchor` rows
are append-only; no tag is renamed.

## JSON Interface

No report shape changes. Effects on existing fields:

- `grim status --format json` `outputs_pending` lists each MCP surface of a
  multi-surface vendor on its own (`{client, path}` per file, sorted by
  client, vendor order within a client). A Copilot project MCP record that
  covers only `.vscode/mcp.json` reports `.github/mcp.json` pending after WP F.
- `grim install --format json` items are unchanged; a newly supported client
  appears where it was skipped before.

## Exit Codes

No new codes. Paths that newly reach an existing code:

| Code | When |
|---|---|
| 65 | A newly written MCP entry (or oauth block) meets a same-named entry the user added by hand (`RefusedUntracked`) — forceable |
| 65 | Droid project→global copy collision (145b), same guard |
| 0 | Every skip introduced here (name grammar, unmapped oauth, env ref in a forbidden field, Cline oauth) warns and continues |

## Implementation Steps

> **Contract-first TDD.** Every package writes its failing unit or acceptance
> test first, then the implementation.

### Phase 1: WP0 — contracts (integrator, opus) — done

- [x] `Vendor::mcp_config_paths(&self, workspace, scope) -> Vec<PathBuf>`,
  default `mcp_config_path(..).into_iter().collect()`. Moved callers:
  `installer.rs` (`client_supports_kind` MCP arm; `install_mcp` loops every
  path, one `ClientOutput` per path and pointer), `client_target.rs`
  (`target_path` MCP arm takes the first path; the two parity tests),
  `status.rs` (its `client_supports_kind` test), and `expected_outputs.rs`
  (a vendor with more than one surface counts each surface as covered only by
  an entry output anchored there).
- [x] `OAuthField` + `McpOAuth::unmapped` in `src/oci/mcp.rs`, table tests.
- [x] `render::NameGrammar` + `render::agent_name_fits`, table tests.
- [x] ADR `adr_mcp_oauth_projection.md` + ADR index row.
- [x] This plan.

Vendor-internal `self.mcp_config_path(..)` calls in `detect` (Claude,
OpenCode, Copilot) stay on the per-vendor hook; WP F moves Copilot's detect
if `.github/mcp.json` should also count as a marker.

### Phase 2: Parallel work packages

One worktree each under `.agents/worktrees/<wp>`; opus implementers.

| WP | Scope | Main files |
|---|---|---|
| A antigravity-rules | 139 | `vendor_antigravity.rs`, `path_anchor.rs` rule arm |
| B junie-agents | 148a | `vendor_junie.rs` |
| C droid | 145a + 145b | `vendor_droid.rs` |
| D goose-kilo-agents | 147, 149 | `vendor_goose.rs`, `vendor_kilo.rs` (reuses `vendor_opencode` fns, no edits there) |
| E cline-mcp | 142 (+ `CLINE_DIR` if confirmed) | `vendor_cline.rs`, new `src/install/cline_lock.rs` |
| F copilot-mcp | 144a + 144b | `vendor_copilot.rs` |
| G oauth | 152, 156 | `vendor_opencode.rs`, `vendor_zed.rs` |
| H vendor-roots | 141b, 148b, 151 | `path_anchor.rs` `VENDOR_ROOTS`, `installer.rs::relocated_vendor_roots*`, `vendor_claude.rs`/`claude_config.rs`, `vendor_openclaw.rs`, `test/tests/test_global.py` |
| I gemini-warp-declines | 146, 155, 141a, 143 | `vendor_gemini.rs`, `vendor_warp.rs`, docs and watchlist decline entries |

Each WP also updates its own rows in the shared docs: `clients.md` matrix,
`vendor-metadata.md` registries, `mcp-servers.md`, `upgrading.md`, the
watchlist, `subsystem-file-structure.md`, the catalog skills (drift duty,
`catalog/README.md`) and the `AGENTS.md` env table (H). Conflicts are expected
only in doc table rows and are resolved at integration.

Parity tests every WP keeps green:
`docs_matrix_row_set_matches_all_and_cells_track_kind_support`,
`docs_reference_matches_<vendor>_registry`,
`agents_emit_matrix_lists_every_client`,
`mcp_servers_emit_matrix_lists_every_client`.
Precedent diffs: `f1b78d44` (kind flip), `c76d25f2` (MCP enablement), the
KIRO_HOME fixtures in `test_global.py` (root relocation).

**Per-WP gate:** `task rust:verify` + the WP's pytest subset
(`test_render_clients.py`, `test_clients.py`, `test_mcp_artifact.py`,
`test_global.py`), then an opus reviewer pass (spec + quality). The Cline
lock, the Claude settings read and the oauth work also get a security
perspective.

### Phase 3: Integration → one commit

1. Take the WPs in order A, B, C, D, G, F, E, I, H. For each: rebase onto
   `feat/upstream-followups`, resolve doc conflicts, `task rust:verify`,
   `git merge --ff-only`.
2. Full `task verify`, and `task docs:check` (Node 24).
3. Local Windows acceptance (`/mnt/c/Users/ecom/grim-wintest/run.sh <wt>`),
   compared against `origin/main` because main carries the known
   [#159](https://github.com/grimoire-rs/grimoire/issues/159) set.
4. Final opus adversarial review of the whole diff (`/hex-review`, codex
   adversary per `hex.md`).
5. `git reset --soft main`, one signed-off commit, e.g.
   `feat(vendor): conclude the 2026-09-27 upstream follow-ups`, body lists
   the decisions and carries
   `Closes #139, #141, #142, #143, #144, #145, #146, #147, #148, #149, #151, #152, #155, #156`.
   Update `.agents/upstream-followups-20260927.md` with the final
   dispositions in the same commit.
6. Remove the WP worktrees and branches. Keep `feat/upstream-followups`
   local, not pushed.

## Testing Strategy

### Unit Tests

| Component | Behavior | Edge cases |
|---|---|---|
| `McpOAuth::unmapped` | set-but-unsupported names, declaration order | empty block, unset fields, full support (done) |
| `agent_name_fits` | Junie and Droid grammars | leading digit, `.`, empty, uppercase, `_` (done) |
| `uncovered_mcp_surfaces` | each surface covered only by its own entry | no record, both recorded (done) |
| Vendor registries | `docs_reference_matches_<vendor>_registry` per new key | — |
| Cline lock | acquire, stale reclaim (> 10 s), release, against a temp dir | concurrent holder |
| Claude settings env | precedence with an injected env lookup | managed absent, malformed JSON |

### Acceptance Tests

- A render test per enabled kind and MCP vendor (paths, bytes, warnings).
- Oauth written-vs-skipped matrix per vendor.
- Gemini self-heal: install on the old render, `grim install`, `status`
  not-modified, `timeout` absent.
- Copilot writes both project files; `status` lists both outputs.
- Upgrade fixtures for `JUNIE_HOME`, `OPENCLAW_HOME` and settings-sourced
  `CLAUDE_CONFIG_DIR`: reap an unmodified old copy, keep an edited one,
  un-splice a stranded MCP entry, uninstall path.

### Gates

`task verify`, `task docs:check`, `task catalog:verify`, the local Windows
run with no new failure against main.

## Rollback Plan

1. Nothing is pushed; drop the branch.
2. After merge, revert the single squash commit; the reaper rows it adds are
   append-only and harmless if the relocation code is reverted with them.

## Risks

| Risk | Mitigation |
|---|---|
| Newly written entries collide with hand-authored ones (exit 65) | `upgrading.md` notes per vendor; `--force` path documented |
| Cline lock protocol drift | Lock behavior pinned by unit tests; re-verify against Cline source before WP E lands |
| Doc-table merge conflicts across nine WPs | Least-shared-first integration order; env roots last |

---

## Progress Log

| Date | Update |
|------|--------|
| 2026-09-27 | WP0 contracts landed: plural MCP config paths, oauth `unmapped` helper, agent-name grammar helper, oauth ADR, this plan |
