# Upstream Follow-up Triage — 2026-09-27

Full disposition of every finding raised by the `upstream-refresh` sweep off
`67627aa5`. `docs-catalog-drift` landed first; a second integration pass then
approved and landed `copilot`, `opencode-agent-values`,
`skill-compatibility-warn`, and `vendor-text-fixes` onto
`hex/upstream-followups` (commits below). `cline-pool-decline-text`
landed last, after a third review round, directly on
`hex/harness-capability-freshness`. `skipped` means "not landed in this
batch", not "wrong". Commit SHAs below are from the integration branch and
change whenever the branch is re-landed or finalized — find a commit by
its subject with `git log --grep`.

| Issue | Finding | Verdict | Reason | Commit |
|---|---|---|---|---|
| [#139](https://github.com/grimoire-rs/grimoire/issues/139) | Antigravity: enable the Rule kind (per-file scoping shipped) | skipped | Not opt-in — flips Declined→Native for every existing Antigravity rule install; mapping choices (`trigger` vs `model_decision`, writing into `~/.gemini/config`) stay open. Stale decline text fixed separately, unapproved. | — |
| [#140](https://github.com/grimoire-rs/grimoire/issues/140) | grim-usage updating.md refresh-protocol command list omits `rate` | landed | One-word doc fix — `rate` is documented and implemented but was missing from the re-verification list. | `61adf282` |
| [#141](https://github.com/grimoire-rs/grimoire/issues/141) | Claude Code: artifact kinds grim does not model (commands, output styles, LSP, workflows, themes, monitors) | design | Each kind needs its own publish/lock/render/JSON contract; output styles is the natural first ADR. | — |
| [#141](https://github.com/grimoire-rs/grimoire/issues/141) | Document CLAUDE_CONFIG_DIR set via Claude's settings env as a known gap | landed | Docs-only Known-gaps entry; no matrix cell or install path changes. | `d0b24158` |
| [#141](https://github.com/grimoire-rs/grimoire/issues/141) | Honor CLAUDE_CONFIG_DIR read from Claude user/managed settings env | breaking | Relocates the global Claude root for existing settings-env users; needs an upgrade fixture and a settings-layer precedence design. Deferred past the freeze; the docs caveat covers the gap for now. | — |
| [#142](https://github.com/grimoire-rs/grimoire/issues/142) | Cline: enable MCP for the CLI's `~/.cline/mcp.json`? | design | Issue names the wrong file; the real target is the shared, lock-protected `cline_mcp_settings.json` — lock participation and env-ref form need a decision first. | — |
| [#142](https://github.com/grimoire-rs/grimoire/issues/142) | Cline: CLINE_DATA_DIR and the global skills dir disagree upstream | landed | Landed with the Cline work item after a third review round. | `docs(vendor): correct Cline CLINE_DATA_DIR and MCP-path claims` |
| [#142](https://github.com/grimoire-rs/grimoire/issues/142) | Cline reads the `.agents/skills` pool at both scopes → join POOL_CAPABLE_VENDORS | landed | Landed with the Cline work item after a third review round. | `feat(install): accept shared_skills for Cline` |
| [#142](https://github.com/grimoire-rs/grimoire/issues/142) | Correct the Cline MCP path recorded in the watchlist and module doc | landed | Landed with the Cline work item after a third review round. | `docs(vendor): correct Cline CLINE_DATA_DIR and MCP-path claims` |
| [#143](https://github.com/grimoire-rs/grimoire/issues/143) | Codex: emit the agents/openai.yaml skill sidecar | design | Would need a `codex.*` skill registry, contradicting the documented empty-registry contract for non-Claude clients; ADR first. | — |
| [#143](https://github.com/grimoire-rs/grimoire/issues/143) | Codex: project headers_helper onto http_headers_helper? | design | Contracts differ (env clearing, caching, no server name/URL); needs an ADR before either client's helper is reused for the other. | — |
| [#143](https://github.com/grimoire-rs/grimoire/issues/143) | Catalog grim-authoring drifted from the Codex docs | done | Already fixed at `bec100b3` on `hex/harness-capability-freshness` (a sibling branch, not this one) — `persistent` literal and the mcp-spec.md timeout/cwd vendor lists are current there. Nothing left to do on this branch. | `bec100b3` (other branch) |
| [#144](https://github.com/grimoire-rs/grimoire/issues/144) | Copilot: project-scope MCP never reaches Copilot CLI | design | `Vendor::mcp_config_path` returns one path per scope; adding `.github/mcp.json` changes the trait shape and every existing Copilot project install's `outputs_pending`. ADR territory. | — |
| [#144](https://github.com/grimoire-rs/grimoire/issues/144) | Copilot: project `[server.oauth]` onto Copilot's native OAuth fields? | design | Lossy on `scopes`/`auth_server_metadata_url`; same open decision as the OpenCode/Codex/Zed OAuth projections — wants one cross-vendor ADR. | — |
| [#144](https://github.com/grimoire-rs/grimoire/issues/144) | Copilot: `shared_skills` writes where Copilot stops reading once COPILOT_HOME is set | landed | Warning implemented in the `copilot` work item, decoupled from the integrity gate and extended to `grim update`. | `4b7234b6` |
| [#144](https://github.com/grimoire-rs/grimoire/issues/144) | Copilot: custom-agent frontmatter keys grim cannot author | landed | `disable-model-invocation`/`user-invocable`/`target` now render natively instead of warning. | `9f46264b` |
| [#145](https://github.com/grimoire-rs/grimoire/issues/145) | Droid: enable the Agent kind (custom droids at `.factory/droids/`) | design | `tools`/`model`/`reasoningEffort`/name-grammar mapping is a real design decision; needs an ADR entry and an upgrade note before the keys freeze. | — |
| [#145](https://github.com/grimoire-rs/grimoire/issues/145) | Droid: enable MCP for `.factory/mcp.json` / `~/.factory/mcp.json` | design | `${NAME}` expansion scope differs from grim's, and Droid's UI copies project servers into the same global file grim would splice; transport-row conflicts need settling first. | — |
| [#145](https://github.com/grimoire-rs/grimoire/issues/145) | User docs still say Droid ships no installable subagent format | landed | Landed with the Cline work item after a third review round. | `docs: correct Droid's agent-decline reason to grim capability gap` |
| [#146](https://github.com/grimoire-rs/grimoire/issues/146) | Gemini: MCP `timeout` also bounds every tool call | breaking | Removing the projection would change already-rendered bytes and existing installs' tool-call timeout (Principle 9). The non-breaking half — disclosing it — already landed at `67627aa5`. | — |
| [#147](https://github.com/grimoire-rs/grimoire/issues/147) | Goose: enable the Agent kind — file agents in `.agents/agents/` | design | Collides with Antigravity's existing writer at the same physical path with a divergent field registry; needs an agent-pool contract ADR first. | — |
| [#148](https://github.com/grimoire-rs/grimoire/issues/148) | Junie: enable the Agent kind — subagents are no longer EAP | design | Needs a typed `junie.*` registry, a name-grammar check, and ownership rules against Junie's own cross-vendor agent import. Wave 2 per `adr_vendor_wave_expansion.md`. | — |
| [#148](https://github.com/grimoire-rs/grimoire/issues/148) | Junie: JUNIE_HOME relocates `~/.junie` and grim does not follow it | breaking | Moves global skills/MCP/detection output for existing JUNIE_HOME users; needs the standard relocated-root reaper and upgrade fixture first. | — |
| [#148](https://github.com/grimoire-rs/grimoire/issues/148) | Stale "agents are EAP-only" text in docs, catalog and code comments | landed | Landed with the Cline work item after a third review round. | `docs: correct stale Junie EAP-only agents-decline reason` |
| [#149](https://github.com/grimoire-rs/grimoire/issues/149) | Kilo: enable the Agent kind — markdown subagents shipped | skipped | Design and too large for this pass: needs a new `kilo.*` registry, an ADR mapping edit and a new `candidate_anchors` arm, and is not opt-in. Stale decline text alone sits in the unapproved `cline-pool-decline-text` item. | — |
| [#150](https://github.com/grimoire-rs/grimoire/issues/150) | Kiro CLI ignores inclusion modes: should grim also warn on scoped rules at project scope? | done | Disclosure already landed at `67627aa5` (clients.md gap note + watchlist row); a render-layer warning is optional follow-up, not recommended. | — |
| [#150](https://github.com/grimoire-rs/grimoire/issues/150) | Kiro MCP oauth skip warning says "mcp.json has no oauth surface", which is now false | landed | Wording fix implemented in the `vendor-text-fixes` work item — reworded to "oauth shape differs", matching Qoder's phrasing. | `7a591f2e` |
| [#150](https://github.com/grimoire-rs/grimoire/issues/150) | Catalog skills lag the 2026-09-27 sweep: Kiro fileMatch claim, Goose URL, MCP timeout/cwd projection list | landed | Kiro fileMatch qualified IDE-only, Goose URL fixed; the Kiro #9176 open/closed correction was a same-finding fixup, squashed in. | `ccdcdcdf` |
| [#151](https://github.com/grimoire-rs/grimoire/issues/151) | OpenClaw: honor OPENCLAW_HOME | breaking | Relocates the global root for existing OPENCLAW_HOME users; needs anchor migration, an old-path reaper and an upgrade fixture. | — |
| [#151](https://github.com/grimoire-rs/grimoire/issues/151) | Adjacent: stale OPENCLAW_HOME and rule-frontmatter wording left at 67627aa5 | landed | Doc/comment fix implemented in the `vendor-text-fixes` work item — comment-only, no behavior change. | `b47cb5f2` |
| [#152](https://github.com/grimoire-rs/grimoire/issues/152) | OpenCode: project `[server.oauth]` onto OpenCode's native `oauth` object | skipped | Not opt-in — flips existing skipped oauth descriptors to written, and a hand-authored same-named entry would newly hit exit 65. Stays open for the projection. | — |
| [#152](https://github.com/grimoire-rs/grimoire/issues/152) | OpenCode: agent `color`/`steps` values OpenCode rejects break its whole config | landed | Fix implemented in the `opencode-agent-values` work item — invalid values drop with a warning instead of writing an agent that breaks OpenCode's whole config; self-heals on the next pin change or `--force`. | `b80aa269` |
| [#153](https://github.com/grimoire-rs/grimoire/issues/153) | Qoder: confirm that project `.qoder/settings.json` MCP entries load | landed | No bug — the two upstream pages answer different questions; watchlist row 215 reworded to the reference-table quote. | `e373105e` |
| [#154](https://github.com/grimoire-rs/grimoire/issues/154) | Warn on a skill `compatibility` longer than the agentskills 500-character cap | landed | Fix implemented in the `skill-compatibility-warn` work item — also warns on empty, bare/null (YAML explicit-null) compatibility, not just an empty string. | `e41959cf` |
| [#155](https://github.com/grimoire-rs/grimoire/issues/155) | Warp: enable MCP for `~/.warp/.mcp.json` and `.warp/.mcp.json` | skipped | Not opt-in and needs design: Warp's env-ref form is undocumented, and a Claude+Warp workspace would double-register a server. Stays open. | — |
| [#156](https://github.com/grimoire-rs/grimoire/issues/156) | Zed: project `[server.oauth]` onto `context_servers` oauth? | skipped | Not opt-in, same reasoning as the OpenCode projection — stays open. The stale clients.md ws/oauth gap-note claim this finding cited is now corrected (`vendor-text-fixes`, landed). | `f07920aa` (doc claim only; projection itself not landed) |

## Landed this pass

**Pass 1** — `docs-catalog-drift` (4 commits, `67627aa5..HEAD` on
`hex/upstream-followups`): `61adf282`, `d0b24158`, `ccdcdcdf`, `e373105e`.

**Pass 2** — `copilot`, `opencode-agent-values`, `skill-compatibility-warn`,
`vendor-text-fixes` (16 commits, cherry-picked in that order onto
`hex/upstream-followups`):

- `copilot`: `4b7234b6`, `9f46264b`, `5a729564`, `a03dc300`, `4cf58de6`
- `opencode-agent-values`: `b80aa269`, `cba11807`, `5407628a`
- `skill-compatibility-warn`: `e41959cf`, `1dccd425`, `1710a467`
- `vendor-text-fixes`: `7a591f2e`, `b47cb5f2`, `f07920aa`, `65b90734`,
  `d4236a30`

## Cline work item

`cline-pool-decline-text` needed a third review round for wording
contradictions (decline legends, the `ai-config-authoring` counts, the Kiro
subagent row). It landed with those fixed; the full `task verify` passed on
the combined branch.
