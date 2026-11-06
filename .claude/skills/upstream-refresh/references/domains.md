# Domain registry

One entry per freshness domain: the ledger file(s) holding its dated claims,
the code and doc sources those claims drive, the change feed, and what
"stale" means. The check ledger `.agents/upstream-checks.md` has one row per
harness (domain `vendors`) and one row per other domain.

The files the stale report scans are the `LEDGERS` constant in
`scripts/upstream_stale.py` — that constant is the single list; this file
does not copy it.

Staleness threshold everywhere: a dated claim older than **183 days**, a
`re-verify after` date in the past, or a research `**Expires:**` in the past.

## 1. `vendors` — harness capabilities, metadata keys, settings keys, config paths, env vars

- **Ledger:** `.agents/upstream-watchlist.md` (every
  `| Capability |` table; the `## Config roots and env vars` table once the
  first harness pass creates it; the `## Settings keys watchlist` table for
  the `settings` kind), plus `//! verified` / `//! live-verified`
  stamps in `src/install/vendor_*.rs`. A section's undated rows take its
  ``All rows `verified YYYY-MM-DD` `` sentence — keep that exact phrasing
  when editing a section intro, or the stale report reads those rows as
  undated. To re-date a stamp, add `re-verified <today>` on the line that
  carries the old date; the report takes the newest date on that line.
  Only `//!` lines in `src/install/vendor_*.rs` are scanned — a `///`
  stamp or one in `src/install/vendor.rs` is invisible to the report.
- **Code:** `src/install/vendor_*.rs`, `src/install/client_target.rs`
  (`KindSupport` grid and its two parity tests). Settings keys:
  `Vendor::settings_fields()` and `settings_config_path()` in each
  `src/install/vendor_<harness>.rs`; the `settings` record and splice live
  in `src/install/settings_*.rs` and `src/oci/settings.rs`.
- **Docs:** `docs/src/content/docs/clients.md`,
  `docs/src/content/docs/vendor-metadata.md`,
  `docs/src/content/docs/mcp-servers.md`,
  `docs/src/content/docs/settings.md` (`#fields`, `#files`, `#limits`), and
  the AGENTS.md
  environment-variable table (vendor config-dir override row). Env-var dates
  live in `.agents/upstream-watchlist.md`, never in AGENTS.md.
- **Check rows:** one per harness below. **Stale:** the row's newest
  `verified` date (or its section default) is past threshold, or the feed
  shows a relevant entry after the cursor.

### Harness sub-table

Tier from `.agents/adr/adr_vendor_support_tiers.md`. Config-root variable:
**honored** means grim follows it today (AGENTS.md); anything else is an
upstream variable grim does not follow, tracked in `.agents/upstream-watchlist.md`.

| Harness | Tier | Docs | Change feed (cursor = tag or heading) | Config-root variable | Live check (Tier 1) |
|---|---|---|---|---|---|
| `claude` | 1 | [code.claude.com/docs](https://code.claude.com/docs/en/settings) | GH `anthropics/claude-code` releases (`v*`); also [CHANGELOG.md](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md), [docs changelog](https://code.claude.com/docs/en/changelog) | `CLAUDE_CONFIG_DIR` (honored) | `claude mcp list` — MCP only; `claude doctor` — settings |
| `codex` | 1 | [docs/config.md](https://github.com/openai/codex/blob/main/docs/config.md) | GH `openai/codex` releases — stable `rust-v*` only, skip prereleases (`-alpha`); also [dev changelog](https://developers.openai.com/codex/changelog) | `CODEX_HOME` (honored) | `codex mcp list`; settings: `codex debug prompt-input`, `codex execpolicy check` |
| `opencode` | 1 | [opencode.ai/docs](https://opencode.ai/docs/config/) | GH `anomalyco/opencode` releases (`v*`; `sst/opencode` redirects here); also [opencode.ai/changelog](https://opencode.ai/changelog) | `OPENCODE_CONFIG_DIR` (honored), `OPENCODE_CONFIG` (honored, file) | `opencode agent list`, `opencode debug skill`, `opencode debug config`, `opencode mcp list` |
| `copilot` | 1 | [Copilot CLI docs](https://docs.github.com/en/copilot/how-tos/copilot-cli) | GH `github/copilot-cli` releases — skip prereleases (`-N` suffix); also [github.blog changelog](https://github.blog/changelog/label/copilot/) | `COPILOT_HOME` (honored) | `copilot skill list`, `copilot instruction list`, `copilot mcp list`, `copilot -p x --agent no-such-agent` (agents); settings: `copilot config <key> <value>` |
| `cursor` | 2 | [cursor.com/docs](https://cursor.com/docs) | page [cursor.com/changelog](https://cursor.com/changelog) | `CURSOR_CONFIG_DIR` (honored for the settings kind's `cli-config.json` only; not for skills, rules, agents, `mcp.json`) | — |
| `kiro` | 2 | [Kiro configuration](https://kiro.dev/docs/configuration/) | page [kiro.dev/changelog](https://kiro.dev/changelog/) | `KIRO_HOME` (honored) | — |
| `junie` | 2 | [Junie docs](https://junie.jetbrains.com/docs/guidelines-and-memory.html) | GH `JetBrains/junie` releases (newest tags are nightlies with empty bodies — read headings on [whats-new](https://junie.jetbrains.com/whats-new)) | `JUNIE_HOME` (not honored; replaces `~/.junie`); `JUNIE_*_LOCATIONS` only add search paths | — |
| `gemini` | 2 | [configuration.md](https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md) | GH `google-gemini/gemini-cli` releases — stable `v*` only, skip `-nightly`/`-preview`; also [docs/changelogs](https://github.com/google-gemini/gemini-cli/blob/main/docs/changelogs/index.md) | `GEMINI_CLI_HOME` (honored; replaces home, `.gemini` appended) | — |
| `zed` | 2 | [zed.dev/docs](https://zed.dev/docs) | GH `zed-industries/zed` releases — skip `-pre` | — (XDG on Linux only; macOS `~/.config/zed`) | — |
| `amp` | 2 | [ampcode.com/docs](https://ampcode.com/docs/markdown/customize/skills) (was `/manual`) | page [ampcode.com/chronicle](https://ampcode.com/chronicle) | `AMP_SETTINGS_FILE` (not honored; in the binary's help, not the docs) | — |
| `antigravity` | 2 | [antigravity.google/docs](https://antigravity.google/docs/rules) | page [antigravity.google/changelog](https://antigravity.google/changelog/) | none documented | — |
| `cline` | 2 | [docs.cline.bot](https://docs.cline.bot) | GH `cline/cline` releases — extension `v*` and `cli-v*` streams only, skip `desktop-v*`, `sdk/*` | `CLINE_DATA_DIR` (not honored) | — |
| `droid` | 2 | [docs.factory.ai](https://docs.factory.ai) | page [release notes](https://docs.factory.ai/changelog/release-notes) | none documented | — |
| `goose` | 2 | [goose docs](https://goose-docs.ai) | GH `aaif-goose/goose` releases (`block/goose` redirects here) | `GOOSE_PATH_ROOT` (not honored) | — |
| `warp` | 2 | [docs.warp.dev](https://docs.warp.dev) | page [docs.warp.dev/changelog](https://docs.warp.dev/changelog) (one sub-page per year) | none documented | — |
| `openclaw` | 2 | [docs.openclaw.ai](https://docs.openclaw.ai) | GH `openclaw/openclaw` releases — newest mainline version, not newest date (older `v2026.7.x` patches still ship); per-release `CHANGELOG/<version>.md` in the repo | `OPENCLAW_HOME` (not honored; documented, replaces home) | — |
| `kilo` | 2 | [Kilo-Org/docs](https://github.com/Kilo-Org/docs) | GH `Kilo-Org/kilocode` releases (`v*`) | none documented | — |
| `qoder` | 2 | [docs.qoder.com](https://docs.qoder.com/cli/memory) | page [CLI release notes](https://docs.qoder.com/release-notes/qoder-cli) (`qoder.com/changelog` is a JS app WebFetch cannot read) | `QODER_CONFIG_DIR` (honored) | — |

### Settings keys per harness

Thirteen harnesses take the `settings` kind: `claude`, `codex`,
`copilot`, `cursor`, `opencode`, `qoder`, `antigravity`, `amp`, `droid`,
`kilo`, `kiro`, `gemini` and `warp`. The other six clients have no rows;
their row below names the reason and the re-check trigger, and a sweep on
one of them checks that trigger. Each row of a harness's
`settings_fields()` is a claim that the key exists upstream, with the value
type and merge class grim gives it (scalar, set, map, own file). A sweep or
deep pass on one of these harnesses checks every row against the reference
below, plus the named limits in the Settings keys section of `.agents/upstream-watchlist.md`. A
row upstream dropped or retyped is a class (c) finding: a merge-class
change is a layout move (Principle 9), never a quiet edit. A key upstream
added is a candidate row, verified before it goes on. A key that switches
off a harness's workspace or folder trust gate, or pre-trusts a folder, is
never a candidate, whatever its type: threat model T3, decided for the
Gemini row in the settings follow-ups plan (PD-41). Known so far: gemini
`/security/folderTrust/enabled`, antigravity `/trustedWorkspaces`, kiro
`chat.disableTrustAllConfirmation`, copilot `trustedFolders`
(`adr_settings_artifact_kind.md`, Appendix A › Rows added under #270). A
new one is a finding.

| Harness | File grim edits | Upstream key reference | Named limits |
|---|---|---|---|
| `claude` | project `.claude/settings.local.json`; global `settings.json` in the `CLAUDE_CONFIG_DIR` root | [settings](https://code.claude.com/docs/en/settings), [settings reference](https://code.claude.com/docs/en/settings-reference), [sandboxing](https://code.claude.com/docs/en/sandboxing) | linked-worktree read and shadowing; project `/permissions/defaultMode`; `/sandbox/filesystem/allowWrite`, `/allowRead` (`on-limited`): an admin-required sandbox ignores them in a project file, and a relative path resolves per file; `/effortLevel` in the user file, project `/sandbox/enabled`, project `/permissions/defaultMode` in VS Code, project `/extraKnownMarketplaces` before the trust dialog (`settings.md#delivered`) |
| `codex` | `config.toml` and `rules/grim-<binding>.rules` in the `CODEX_HOME` root | [config reference](https://learn.chatgpt.com/docs/config-file/config-reference), [rules](https://learn.chatgpt.com/docs/agent-configuration/rules) | marketplace scope; one bad `.rules` file |
| `copilot` | `settings.json` in the `COPILOT_HOME` root | [config dir reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference), [change settings](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/change-settings) | reads `.claude/settings*.json` for a shared subset |
| `cursor` | an existing `cli-config.json` only | [CLI configuration](https://cursor.com/docs/cli/reference/configuration), [CLI permissions](https://cursor.com/docs/cli/reference/permissions) | path order; `/permissions/*` (`on-limited`) |
| `opencode` | `OPENCODE_CONFIG`, else XDG `opencode.json`/`.jsonc` | [config](https://opencode.ai/docs/config/), [permissions](https://opencode.ai/docs/permissions/) | a value holding `{env:` or `{file:` is refused (OpenCode expands both); OpenCode also reads a `config.json` beneath both files, which grim does not read (ADR Appendix B) |
| `qoder` | `settings.json` in the `QODER_CONFIG_DIR` root | [settings reference](https://docs.qoder.com/cli/settings-reference), [settings](https://docs.qoder.com/cli/settings), [permissions](https://docs.qoder.com/cli/permissions) | `/permissions/*` (`on-limited`) |
| `antigravity` | `settings.json` in `antigravity-cli-root` (`~/.gemini/antigravity-cli`); global only, CLI only | [settings](https://antigravity.google/docs/settings), [CLI reference](https://antigravity.google/docs/cli/reference), [schema](https://antigravity.google/schemas/v1/settings.json) | the CLI rewrites the file at startup and drops default-valued keys; `/permissions/*` (`on-limited`): project precedence |
| `amp` | `settings.json` (or an existing `settings.jsonc`) in `amp-root`, the file grim's MCP entry uses; global only | [settings](https://ampcode.com/docs/markdown/cli/settings) | which of `settings.json` and `settings.jsonc` wins is unstated; a workspace `.amp/settings.json[c]` overrides |
| `droid` | `settings.json` in `droid-root` (`~/.factory`); global only | [settings](https://docs.factory.com/droid-cli/settings), [hierarchical settings](https://docs.factory.com/enterprise/hierarchical-settings-and-org-control), [sandbox](https://docs.factory.com/autonomy-and-safety/sandbox) | org values win for hard controls, project values for session defaults; `settings.local.json` merges on top; `/sessionDefaultSettings/reasoningEffort`, `/sandbox/filesystem/allowWrite`, `/sandbox/network/allowedDomains` (`on-limited`) |
| `kilo` | `kilo.jsonc` in `kilo-config-root` (XDG `kilo`), or `kilo.json` there when it is the only one; global only | [docs corpus](https://kilo.ai/docs/llms.txt) (Settings, CLI Config Schema), [schema](https://app.kilo.ai/config.json) | a project `kilo.jsonc` or `.kilo/kilo.jsonc` overrides; `kilo.json`, `opencode.json[c]` and `config.json` in the same directory deep-merge in an unstated order; clearing a setting in Kilo's Settings UI removes it from every config file; a value holding `{env:` or `{file:` is refused (Kilo expands both) |
| `kiro` | `settings/cli.json` in the `KIRO_HOME` root; global only | [settings reference](https://kiro.dev/docs/reference/settings), [configuration](https://kiro.dev/docs/configuration) | flat dotted keys (one pointer segment); the CLI validates no value; a workspace `cli.json` overrides; `/knowledge.default*Patterns` (`on-limited`) |
| `gemini` | `settings.json` in `gemini-root` (`$GEMINI_CLI_HOME/.gemini`, else `~/.gemini`), shared with MCP registration; global only | [configuration reference](https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md), [settingsSchema.ts](https://github.com/google-gemini/gemini-cli/blob/main/packages/cli/src/config/settingsSchema.ts) | project and system files override; `/tools/allowed`, `/tools/confirmationRequired`, `/tools/sandboxAllowedPaths` (`on-limited`): a project list replaces; a value holding `$NAME` or `${NAME}` is refused (Gemini resolves them, [envVarResolver.ts](https://github.com/google-gemini/gemini-cli/blob/main/packages/cli/src/utils/envVarResolver.ts)) |
| `warp` | `settings.toml` in `warp-root` (`~/.warp`, macOS) or `warp-config-root` (XDG `warp-terminal`, Linux); none on Windows (`%LOCALAPPDATA%`, no grim root) or any other platform; global only | [settings file](https://docs.warp.dev/terminal/settings), [all settings](https://docs.warp.dev/terminal/settings/all-settings), [file locations](https://docs.warp.dev/terminal/settings/file-locations) | Warp's Settings panel and `modify-settings` skill also write the file, and Warp reloads it live; an unrecognized value falls back to the default; Preview builds use `warp-terminal-preview` and `~/.warp-preview`, not covered; `[agents.profiles]` and `execution_profiles` stay off while two pages disagree (PD-21) |
| `agents` | none: no settings file, a directory convention; re-check: a vendor-neutral settings file appears in the agentskills or AGENTS.md specification | — | — |
| `cline` | none: no key documented for `global-settings.json`; re-check: Cline docs list its keys, types and values, or a `cline config` verb prints them | [configuration](https://docs.cline.bot/cli/configuration.md), [CLI reference](https://docs.cline.bot/cli/cli-reference.md) | — |
| `junie` | none: no per-key precedence across `config.json` and `settings.json`; re-check: Junie documents that precedence, or `brave` and permission keys inside `config.json` | [CLI configuration](https://junie.jetbrains.com/docs/junie-cli-configuration.html) | — |
| `zed` | none: the macOS settings path contradicts itself across pages and Windows is unstated; re-check: Zed docs state one path per OS (then `/agent/tool_permissions/default` is the first row) | [all settings](https://zed.dev/docs/reference/all-settings.md), [tool permissions](https://zed.dev/docs/ai/tool-permissions.md) | — |
| `openclaw` | none: `openclaw.json` is JSON5, no splice engine for the format; re-check: grim gains a JSON5 splice engine, or OpenClaw documents plain-JSON `openclaw.json` | [configuration](https://docs.openclaw.ai/gateway/configuration) | — |
| `goose` | none: `config.yaml` is YAML, no splice engine for the format; re-check: grim gains a YAML splice engine | [config files](https://goose-docs.ai/docs/guides/config-files) | — |

`on-limited` marks a row verified with a named limit on the reference page
(`settings.md#shadowing`); each one has its own row in `.agents/upstream-watchlist.md`. Never infer
a key's accepted values from a harness's error text: test each value
(deep-pass.md § 2).

A "page" feed has no machine-readable form: fetch it and diff headings
against the cursor heading. A GitHub feed is always read through
`gh api --paginate repos/<o>/<r>/releases`, never `releases.atom`.

### Seed findings (first pass on the target must resolve these)

None open. The two `copilot` seeds were resolved on 2026-09-27
(`.agents/research/research_upstream_copilot_20260927.md`): global MCP
`${VAR}` expansion ships and is live-verified, and the five rejected
first-party skills had unquoted `description:` values containing `: `.
Add a seed here when a finding needs the next pass on a target.

## 2. `catalog` — first-party catalog skills

- **Ledger:** `catalog/skills/*/references/updating.md` (the per-release
  update protocol); the `compatibility: grim>=X.Y` line of each
  `catalog/skills/*/SKILL.md`.
- **Feed:** GH `grimoire-rs/grimoire` releases; cursor = latest real
  release tag. The `v99.0.0` trial tag is never a real release.
- **Check (confirm-only, never re-researched):** each catalog skill's
  `compatibility` minor equals the latest real release tag's minor. A skill
  with no `compatibility:` line (today `ai-config-authoring`) is skipped and
  the skip is noted in the run artifact. A mismatch is an issue draft —
  `catalog/**` is a publish surface and is not edited in the pass.
- **Stale:** a mismatch, or a release tag newer than the cursor.

## 3. `specs` — MCP and agentskills.io

- **Ledger:** MCP claims in `docs/src/content/docs/mcp-servers.md` and the
  MCP rows of `.agents/upstream-watchlist.md` (the spec-level row is "HTTP+SSE transport (spec)");
  every MCP spec link (`/usr/bin/grep -rn modelcontextprotocol.io docs/src
  catalog`); the `agents` client target (agentskills.io pool); the SKILL.md
  field table in `docs/src/content/docs/artifacts.md` (Skills) and its
  catalog twin `catalog/skills/grim-authoring/references/skill-spec.md`.
- **Code:** `src/oci/mcp.rs`; `src/install/client_target.rs` (`agents`);
  the agentskills field limits in `src/skill/skill_name.rs`,
  `skill_description.rs`, `skill_frontmatter.rs`. A limit grim does not
  enforce yet is an issue draft — enforcing it rejects published skills.
- **Feed:** GH `modelcontextprotocol/modelcontextprotocol` releases (cursor
  = spec revision tag, e.g. `2026-07-28`) and
  [versioning](https://modelcontextprotocol.io/specification/versioning);
  `agentskills/agentskills` has no releases — cursor = last seen commit
  (`gh api repos/agentskills/agentskills/commits`), docs
  [agentskills.io/specification](https://agentskills.io/specification) and
  the [client-implementation guide](https://agentskills.io/client-implementation/adding-skills-support),
  which (not the spec) names the `.agents/skills` convention. The MCP
  versioning URL redirects to `/docs/<revision>/learn/versioning`; spec text
  reads verbatim from the repo at the revision tag
  (`docs/specification/<rev>/`, `?ref=<rev>`).
- **Stale:** a new spec revision or spec commit after the cursor, or a
  dated MCP row past threshold. Any validation change is security-adjacent
  (`src/oci/**`) — the pass adds a security reviewer.

## 4. `forge` — ratings forge claims

- **Ledger:** the "Ratings forge capability watchlist" table of `.agents/upstream-watchlist.md`.
- **Code and docs:** `src/catalog/rating_provider.rs`,
  `docs/src/content/docs/ratings.md`.
- **Feed:** [GHES release notes](https://docs.github.com/en/enterprise-server@latest/admin/release-notes)
  (cursor = GHES version), [GitLab releases](https://about.gitlab.com/releases/categories/releases/)
  (cursor = GitLab version), and the
  [GraphQL removed-items log](https://docs.gitlab.com/api/graphql/removed_items/)
  for `awardEmojiToggle`.
- **Stale:** a row past threshold, or a removal/deprecation of an API the
  code hard-codes.

## 5. `landscape` — comparable tools

- **Ledger:** `.claude/rules/product-context.md` › Comparable Tools and its
  `re-verify after` date; the Competitive landscape section of
  `.agents/research/research_promotion_positioning.md`.
- **Feed:** none — sweep only. Sources are the ones cited in that
  research artifact.
- **Stale:** the `re-verify after` date has passed. Edits follow
  product-context.md's own Update Protocol.

## 6. `research` — expired research artifacts

- **Ledger:** `**Expires:**` in the Metadata block of every
  `.agents/research/*.md`.
- **Feed:** none — the date is the trigger.
- **Dating rule (first sweep):** an artifact with no `Expires:` line gets
  one. Vendor, ecosystem or other fact research → `**Expires:** <Date + 6
  months>`. Research that only informed a decision that has since landed →
  `**Expires:** n/a (historical — <ADR or plan>)`, which the stale report
  skips. A computed date already past → re-verify it or mark it historical
  in the same pass.
- **Stale:** `Expires:` in the past, or a value that is not a date and not
  `n/a …`.
