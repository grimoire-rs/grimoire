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

## 1. `vendors` — harness capabilities, metadata keys, config paths, env vars

- **Ledger:** `.claude/rules/vendor-capability-watchlist.md` (every
  `| Capability |` table; the `## Config roots and env vars` table once the
  first harness pass creates it), plus `//! verified` / `//! live-verified`
  stamps in `src/install/vendor_*.rs`. A section's undated rows take its
  ``All rows `verified YYYY-MM-DD` `` sentence — keep that exact phrasing
  when editing a section intro, or the stale report reads those rows as
  undated. To re-date a stamp, add `re-verified <today>` on the line that
  carries the old date; the report takes the newest date on that line.
  Only `//!` lines in `src/install/vendor_*.rs` are scanned — a `///`
  stamp or one in `src/install/vendor.rs` is invisible to the report.
- **Code:** `src/install/vendor_*.rs`, `src/install/client_target.rs`
  (`KindSupport` grid and its two parity tests).
- **Docs:** `docs/src/content/docs/clients.md`,
  `docs/src/content/docs/vendor-metadata.md`,
  `docs/src/content/docs/mcp-servers.md`, and the AGENTS.md
  environment-variable table (vendor config-dir override row). Env-var dates
  live in the watchlist, never in AGENTS.md.
- **Check rows:** one per harness below. **Stale:** the row's newest
  `verified` date (or its section default) is past threshold, or the feed
  shows a relevant entry after the cursor.

### Harness sub-table

Tier from `.agents/adr/adr_vendor_support_tiers.md`. Config-root variable:
**honored** means grim follows it today (AGENTS.md); anything else is an
upstream variable grim does not follow, tracked in the watchlist.

| Harness | Tier | Docs | Change feed (cursor = tag or heading) | Config-root variable | Live check (Tier 1) |
|---|---|---|---|---|---|
| `claude` | 1 | [code.claude.com/docs](https://code.claude.com/docs/en/settings) | GH `anthropics/claude-code` releases (`v*`); also [CHANGELOG.md](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md), [docs changelog](https://code.claude.com/docs/en/changelog) | `CLAUDE_CONFIG_DIR` (honored) | `claude mcp list` — MCP only |
| `codex` | 1 | [docs/config.md](https://github.com/openai/codex/blob/main/docs/config.md) | GH `openai/codex` releases — stable `rust-v*` only, skip prereleases (`-alpha`); also [dev changelog](https://developers.openai.com/codex/changelog) | `CODEX_HOME` (honored) | `codex mcp list` |
| `opencode` | 1 | [opencode.ai/docs](https://opencode.ai/docs/config/) | GH `anomalyco/opencode` releases (`v*`; `sst/opencode` redirects here); also [opencode.ai/changelog](https://opencode.ai/changelog) | `OPENCODE_CONFIG_DIR` (honored), `OPENCODE_CONFIG` (honored, file) | `opencode agent list`, `opencode debug skill`, `opencode debug config`, `opencode mcp list` |
| `copilot` | 1 | [Copilot CLI docs](https://docs.github.com/en/copilot/how-tos/copilot-cli) | GH `github/copilot-cli` releases — skip prereleases (`-N` suffix); also [github.blog changelog](https://github.blog/changelog/label/copilot/) | `COPILOT_HOME` (honored) | `copilot skill list`, `copilot instruction list`, `copilot mcp list`, `copilot -p x --agent no-such-agent` (agents) |
| `cursor` | 2 | [cursor.com/docs](https://cursor.com/docs) | page [cursor.com/changelog](https://cursor.com/changelog) | `CURSOR_CONFIG_DIR` (not honored) | — |
| `kiro` | 2 | [Kiro configuration](https://kiro.dev/docs/configuration/) | page [kiro.dev/changelog](https://kiro.dev/changelog/) | `KIRO_HOME` (honored) | — |
| `junie` | 2 | [Junie docs](https://junie.jetbrains.com/docs/guidelines-and-memory.html) | GH `JetBrains/junie` releases (newest tags are nightlies with empty bodies — read headings on [whats-new](https://junie.jetbrains.com/whats-new)) | `JUNIE_HOME` (not honored; replaces `~/.junie`); `JUNIE_*_LOCATIONS` only add search paths | — |
| `gemini` | 2 | [configuration.md](https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md) | GH `google-gemini/gemini-cli` releases — stable `v*` only, skip `-nightly`/`-preview`; also [docs/changelogs](https://github.com/google-gemini/gemini-cli/blob/main/docs/changelogs/index.md) | `GEMINI_CLI_HOME` (honored; replaces home, `.gemini` appended) | — |
| `zed` | 2 | [zed.dev/docs](https://zed.dev/docs) | GH `zed-industries/zed` releases — skip `-pre` | — (XDG on Linux only; macOS `~/.config/zed`) | — |
| `amp` | 2 | [ampcode.com/docs](https://ampcode.com/docs/markdown/customize/skills) (was `/manual`) | page [ampcode.com/chronicle](https://ampcode.com/chronicle) | `AMP_SETTINGS_FILE` (not honored, contested) | — |
| `antigravity` | 2 | [antigravity.google/docs](https://antigravity.google/docs/rules) | page [antigravity.google/changelog](https://antigravity.google/changelog/) | none documented | — |
| `cline` | 2 | [docs.cline.bot](https://docs.cline.bot) | GH `cline/cline` releases — extension `v*` and `cli-v*` streams only, skip `desktop-v*`, `sdk/*` | `CLINE_DATA_DIR` (not honored) | — |
| `droid` | 2 | [docs.factory.ai](https://docs.factory.ai) | page [release notes](https://docs.factory.ai/changelog/release-notes) | none documented | — |
| `goose` | 2 | [goose docs](https://goose-docs.ai) | GH `aaif-goose/goose` releases (`block/goose` redirects here) | `GOOSE_PATH_ROOT` (not honored) | — |
| `warp` | 2 | [docs.warp.dev](https://docs.warp.dev) | page [docs.warp.dev/changelog](https://docs.warp.dev/changelog) (one sub-page per year) | none documented | — |
| `openclaw` | 2 | [docs.openclaw.ai](https://docs.openclaw.ai) | GH `openclaw/openclaw` releases — newest mainline version, not newest date (older `v2026.7.x` patches still ship); per-release `CHANGELOG/<version>.md` in the repo | `OPENCLAW_HOME` (not honored; documented, replaces home) | — |
| `kilo` | 2 | [Kilo-Org/docs](https://github.com/Kilo-Org/docs) | GH `Kilo-Org/kilocode` releases (`v*`) | none documented | — |
| `qoder` | 2 | [docs.qoder.com](https://docs.qoder.com/cli/memory) | page [CLI release notes](https://docs.qoder.com/release-notes/qoder-cli) (`qoder.com/changelog` is a JS app WebFetch cannot read) | `QODER_CONFIG_DIR` (honored) | — |

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
  watchlist's MCP rows (the spec-level row is "HTTP+SSE transport (spec)");
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

- **Ledger:** the watchlist's "Ratings forge capability watchlist" table.
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
