# Research: upstream refresh — goose (sweep)

## Metadata
**Date:** 2026-11-16 · **Domain:** vendors · **Depth:** sweep — checked 50d ago
**Triggered by:** /upstream-refresh
**Expires:** 2027-05-16

## Claims

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | MCP kind declined; config still YAML; ext types builtin/platform/stdio/streamable_http, no SSE | .agents/upstream-watchlist.md:118 | verified 2026-09-27 v1.52.0 | unchanged | https://goose-docs.ai/docs/guides/config-files.md | "Supported extension types are `builtin`, `platform`, `stdio`, and `streamable_http`. SSE is not supported; migrate old SSE configurations to `streamable_http`." | v1.59.2 (docs live 2026-11-16) |
| 2 | Re-check trigger (grim gains a YAML splice engine) has not fired | domains.md:113 | none | unchanged | https://github.com/aaif-goose/goose/blob/v1.59.2/crates/goose/src/config/paths.rs | (grim-side check: src/install has only json_splice.rs and toml_splice.rs; no YAML splice module) | n/a |
| 3 | macOS config root is ~/.config/goose (etcetera `choose_app_strategy`); env-var page still says Application Support | .agents/upstream-watchlist.md:119 | verified 2026-09-27 v1.52.0 | unchanged | https://goose-docs.ai/docs/guides/config-files.md ; https://goose-docs.ai/docs/guides/environment-variables.md | config-files: "macOS/Linux: `~/.config/goose/config.yaml`". env-vars: "- macOS: `~/Library/Application Support/Block/goose/`" (still inconsistent). Source: `let strategy = choose_app_strategy(AppStrategyArgs {` | v1.59.2 |
| 4 | Agent paths: `.goose/agents`, `.claude/agents`, `.agents/agents` local; `~/.goose/agents`, `~/.agents/agents`, `<config>/agents`, `~/.claude/agents` global, first name wins | .agents/upstream-watchlist.md:121; src/install/vendor_goose.rs:28 | verified 2026-09-27 v1.52.0 | unchanged | https://github.com/aaif-goose/goose/blob/v1.59.2/crates/goose/src/agents/platform_extensions/summon.rs | `working_dir.join(".goose/agents"), working_dir.join(".claude/agents"), working_dir.join(".agents/agents"),` and `home.as_ref().map(|h| h.join(".goose/agents")), home.as_ref().map(|h| h.join(".agents/agents")), Some(config.join("agents")), home.as_ref().map(|h| h.join(".claude/agents")),` | v1.59.2 |
| 5 | AgentMetadata fields name (required), description, model | .agents/upstream-watchlist.md:121 | verified 2026-09-27 | unchanged | https://goose-docs.ai/docs/guides/context-engineering/custom-agents.md | "Only `name` is required in the frontmatter. `description` and `model` are optional." | v1.59.2 |
| 6 | Docs: `.goose/agents/` compat; new shared agents use `.agents/agents/` | .agents/upstream-watchlist.md:121; clients.md:595 | verified 2026-09-27 | unchanged | https://goose-docs.ai/docs/guides/context-engineering/custom-agents.md | "New shared agents should use `.agents/agents/` for project agents or `~/.agents/agents/` for global agents." | v1.59.2 |
| 7 | Skills: `.agents/skills` recommended, `.goose/skills` backward-compat, both scopes; global is `$HOME/.agents/skills` | src/install/vendor_goose.rs:10; clients.md:590; vendor-metadata.md:498 | verified 2026-07-27/09-27 | unchanged | https://goose-docs.ai/docs/guides/context-engineering/using-skills.md | "goose also discovers skills from `.goose/skills/`, `.claude/skills/`, `~/.claude/skills/`, and platform-specific config directories, but `agents/skills/` is the recommended standard." Also "`~/.agents/skills/` — Global skills, available in all sessions" | v1.59.2 |
| 8 | Rule kind declined; `.goosehints`/AGENTS.md monolithic, no per-file scoping key | .agents/upstream-watchlist.md:120; vendor_goose.rs:19 | verified 2026-09-27 | unchanged (no scoping key seen in docs or release notes v1.53.0-v1.59.2; negative check only) | https://goose-docs.ai/docs/guides/context-engineering/using-skills.md | ".goosehints: Best for general preferences, project context, and repeated instructions like \"Always use TypeScript\"" | v1.59.2 |
| 9 | `$GOOSE_PATH_ROOT` does not move agent/skill dirs (grim: "not honored", agents.md:168, vendor_goose.rs:36, vendor-metadata.md:498) | domains.md:65; docs/src/content/docs/agents.md:168 | not honored for skills/agents | unchanged for skills (`dirs::home_dir()`). PARTIAL TRUTH: `Paths::agents_dir()`/`agents_home_dir()`/`plugins_dir()` DO move to `<root>/.agents/...` when the var is an absolute path, but `discover_filesystem_sources` (the agent scanner) still uses `dirs::home_dir()`, so scanning does not follow; same code existed at v1.52.0 | https://github.com/aaif-goose/goose/blob/v1.59.2/crates/goose/src/config/paths.rs | `DirType::Agents => base.join(".agents").join("agents"),` and `DirType::Plugins => base.join(".agents").join("plugins"),` | v1.59.2 (identical at v1.52.0) |
| 10 | GOOSE_PATH_ROOT replaces config root, creates config/ data/ state/ subdirs | vendor_goose.rs:196,399; configuration.md:1468; upgrading.md:42 | replaces candidate list | unchanged | https://goose-docs.ai/docs/guides/environment-variables.md | "When set, goose creates `config/`, `data/`, and `state/` subdirectories under the specified path." | v1.59.2 |
| 11 | Hook kind declined (Goose owns own hooks file) | .agents/upstream-watchlist.md:235 | grim-side schedule | CHECK: v1.55.0 "Bind subagents by their parent's hooks, mode and approvals", v1.59.0 "Stop, approval, and tool hook operations for goose-agent (GDK)": Goose has a hooks concept now; file location/format not verified | https://github.com/aaif-goose/goose/releases/tag/v1.59.0 | "Stop, approval, and tool hook operations for goose-agent (GDK) [#13169]" | v1.59.0 |
| 12 | Plugins surface: skills from `~/.agents/plugins/<plugin-name>/` (not covered by grim) | no ledger row | n/a | new, uncovered | https://goose-docs.ai/docs/guides/context-engineering/using-skills.md | "`~/.agents/plugins/<plugin-name>/` — Skills provided by installed [plugins](/docs/guides/context-engineering/plugins)" | v1.59.2 |

## Verifier sign-off

none required — no code landed in this run.

## Routing

- #8 Rule kind declined; .goosehints/AGENTS.md monolithic, no per-file scoping key → (a) ledger or doc text fix
- #9 $GOOSE_PATH_ROOT does not move agent/skill dirs (grim: "not honored",... → (a) ledger or doc text fix
- #11 Hook kind declined (Goose owns own hooks file) → (c) issue draft
- #12 Plugins surface: skills from ~/.agents/plugins/<plugin-name>/ (not covered by... → (b) additive renderer or metadata support — reported, not implemented in this run

## Issue drafts

### Goose hooks: v1.55.0 and v1.59.0 add a hooks concept, location unverified

Goose release notes list "Bind subagents by their parent's hooks, mode and approvals" (v1.55.0) and "Stop, approval, and tool hook operations for goose-agent (GDK)" (v1.59.0). Source: https://github.com/aaif-goose/goose/releases/tag/v1.59.0. File location and format were not verified. grim declines the hook kind on the premise that Goose owns its hooks file (`.agents/upstream-watchlist.md:235`).

Labels: enhancement

## Friction

- dropped (unsourced): Claims in clients.md:420,482,527,556-567, vendor-metadata.md:131-171, mcp-servers.md:238-240 are grim-side statements (pool membership, YAML limitation) already covered by rows 1,2,7; no separate upstream fact.
- dropped (unsourced): vendor_goose.rs stamp "verified 2026-07-27" (line 7): historical, superseded by the 2026-09-27 re-verify; not re-fetched at the 2026-07 revision.
- dropped (unsourced): Negative claim "no per-file rule scoping key": only docs and release notes searched, not the full docs tree.
- not reached: Hook file location/format for Goose (row 11 follow-up): the plugins docs page and any hooks docs page.
- not reached: Full per-PR read of release notes v1.53.0-v1.59.2 (only keyword-grepped bodies).
- not reached: v1.59.1/v1.58.1/v1.59.2 bodies matched no keyword (patch releases).
- feed: newest v1.59.2 (2026-11-16), cursor found yes

## Skill changes

Host caps concurrent subagents at 20; the researcher built its own claim list; replies are a short summary with the table in a file. Fixed in this run's chore(skills) commit.
