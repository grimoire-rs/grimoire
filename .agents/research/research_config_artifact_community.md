# Research: How practitioners share AI coding harness settings (community lane)

## Metadata

**Date:** 2026-11-03
**Domain:** packaging
**Triggered by:** `.agents/discussions/config-artifact-type.md`
**Expires:** 2027-05-03

Method note. Counts come from `gh api` / `gh search issues` on 2026-11-03 (reactions = `+1` and total reactions on the issue body; comments = comment count). Web search was the only route to blogs and forums: no Hacker News thread and no Reddit thread on shared permission presets was found, so the "what people say" side rests on blogs, dev.to, issues and READMEs. Every source below is dated 2025-09 or later (inside ~18 months) unless flagged.

## Direct Answer

Demand exists, but it is overwhelmingly demand for **portability of one person's own config** (dotfiles, multi-machine sync) and for **team-shared project files**, not for installing someone else's settings. Evidence for demand for a packaged, third-party-authored harness-settings artifact is thin. The loudest, best-reacted requests concern instructions (AGENTS.md) and skills, not settings. Where settings are the subject, the pain is (a) machine-local state mixed into the shared file (Codex trust entries), (b) no layering or extends, (c) per-harness formats. Permission allow-lists are the most common settings content in the wild, and the security literature argues against copying them.

## Technology Landscape

### Trending (gaining momentum)

| Tool/Pattern | Adoption Signal | Key Benefit | Relevance |
|---|---|---|---|
| Multi-harness sync tools that now include permissions | rulesync 1.6k stars, ships a `permissions` feature (Kiro confirmed; per-tool matrix not verified) — https://github.com/dyoshikawa/rulesync | One source, many harnesses | First sign a generic permissions layer is being attempted |
| Account/vendor-hosted settings sync requests | Claude Code #22648 (51 thumbs-up, 26 comments, open) — https://github.com/anthropics/claude-code/issues/22648 ; Codex #11061 (69 thumbs-up) — https://github.com/openai/codex/issues/11061 | Same settings on every machine | Personal portability, not distribution |

### Established (proven, widely accepted)

| Tool/Pattern | Status | Notes |
|---|---|---|
| Commit `.claude/settings.json`, gitignore `settings.local.json` | Standard, vendor-documented | https://code.claude.com/docs/en/settings ; guides agree: https://dev.to/claudeguide/claude-code-permissions-trust-levels-allow-lists-and-safe-defaults-3ek6 |
| Private dotfiles repo with symlinks | Common for individuals | Commenter in https://github.com/anthropics/claude-code/issues/87068 ; Codex author's sync script https://github.com/openai/codex/issues/11061 |
| Committed rules for teams (Cursor `.cursor/rules`) | Common; settings not shared | https://dev.to/olivia_craft/cursor-rules-for-teams-how-to-share-ai-rules-across-your-entire-codebase-4l0g |
| Managed/MDM policy | Exists, buggy and bypass-prone | https://github.com/anthropics/claude-code/issues/82704 |

### Emerging (early but promising)

| Tool/Pattern | Signal | Worth Watching Because |
|---|---|---|
| Curated hardening configs shared as repos | trailofbits/claude-code-config, 2.1k stars — https://github.com/trailofbits/claude-code-config | Shows appetite for others' deny rules/hooks, with "patterns to adapt, not drop-in configs" caveat |
| agent-rules-sync (syncs Claude settings.json, hooks, MCP) | PyPI only, adoption unmeasured — https://pypi.org/project/agent-rules-sync/1.5.2/ | Permissions handled for Claude Code only |

### Declining (losing relevance)

None found.

## Key Findings

### 1. What people share (ranked, low-to-medium confidence)

Ranking is by presence in measured data first, issue reactions second, and guide/README prevalence third. No single dataset covers all categories.

1. **Allow-lists** — only measured dataset: UpGuard, 18,470 public `.claude/settings.local.json` files, published 2026-10-29. `git add` 32.6%, `find` 29.0%, `git commit` 24.5%, `rm` 22.2%, `curl` 21.3%, `git push` 19.7%. https://www.upguard.com/blog/yolo-mode-hidden-risks-in-claude-code-permissions. Note these are the personal, local-override file, not the shared one: they are accumulated "always allow" clicks (https://www.developersdigest.tech/blog/claude-code-permissions-settings-guide).
2. **Whole dotfiles / instructions / skills** — most-reacted adjacent requests: AGENTS.md support in Claude Code, 5,191 thumbs-up (closed) https://github.com/anthropics/claude-code/issues/6235 ; follow-up 440 thumbs-up https://github.com/anthropics/claude-code/issues/31005 ; skills sync 162 thumbs-up https://github.com/anthropics/claude-code/issues/20697. Settings are one row in the sync request #22648.
3. **Deny rules / secrets protection** — rare in practice: only 1.1% of the UpGuard files have any deny rule (same URL), but heavily recommended in guides (https://angelo-lima.fr/en/claude-code-permissions-security/) and shipped in Trail of Bits' config (https://github.com/trailofbits/claude-code-config).
4. **Sandbox / approval modes, model defaults** — Codex #11061 names "model, sandbox policy, MCP servers" as the portable preferences wanted. Copilot CLI "Globally configurable allowed tools", 43 thumbs-up, open https://github.com/github/copilot-cli/issues/179.
5. **Hooks** — shipped in Trail of Bits config (block `rm -rf`, block push to main); no issue-count evidence.
6. **Env vars, status line / UI** — appear inside shared templates (Trail of Bits: privacy env vars, statusline); no standalone demand signal found.

### 2. How they share, and the complaints

- **Committed project file.** Complaint: the same file carries personal rules, so a command that works for you prompts a teammate (https://www.developersdigest.tech/blog/claude-code-permissions-settings-guide). Gemini CLI users asked for a local override layer next to the project file (#25380, 1 reaction, closed) https://github.com/google-gemini/gemini-cli/issues/25380 ; Codex users asked for the same (#24961, 15 thumbs-up, open) https://github.com/openai/codex/issues/24961.
- **Dotfiles repo / symlink.** Complaint: Codex appends per-repo `trust_level` entries to `~/.codex/config.toml`, making it non-portable; author wrote a script to re-attach `[projects.*]` blocks (#11061, 69 thumbs-up, 5 comments, open, no vendor reply seen). Related #14601, 88 thumbs-up, 19 comments, open https://github.com/openai/codex/issues/14601. Claude Code: atomic writes break symlinked settings https://github.com/anthropics/claude-code/issues/78162.
- **Managed policy.** Complaint: remote and MDM settings "trivially bypassable", no layering/extend (#82704, 4 reactions, closed not_planned) https://github.com/anthropics/claude-code/issues/82704 ; several open bugs where managed keys are ignored, e.g. https://github.com/anthropics/claude-code/issues/98376 and https://github.com/anthropics/claude-code/issues/89346.
- **Copy-paste from gists/repos.** thegeosman/claude-code-settings (900+ allow patterns, 4 stars, last push 2025-06, so over 18 months-ish, stale) https://github.com/thegeosman/claude-code-settings. Low adoption.
- **Plugins.** Not found as a settings-sharing route in this lane.

### 3. Vendor trackers

- Claude Code #22648 account-level sync: 51 thumbs-up, 54 total, 26 comments, open since 2026-02; body says it was requested before (#6037, #19634, #13461, #12119) "with no official response" https://github.com/anthropics/claude-code/issues/22648 . Duplicate #87068 closed by a maintainer pointing at #22648 https://github.com/anthropics/claude-code/issues/87068.
- Permission presets: #48097 "Named permission preset tiers" closed not_planned, 0 reactions https://github.com/anthropics/claude-code/issues/48097 ; #8172 "Configurable Permission Presets" closed not_planned, 0 reactions https://github.com/anthropics/claude-code/issues/8172. Both were stale-closed, not rejected on argument. Demand signal: near zero.
- Codex #11061 (69), #14601 (88), #24961 (15); Copilot #179 (43); OpenCode account sync #55118 (0 reactions, 3 comments) https://github.com/anomalyco/opencode/issues/55118.
- Codex ships `requirements.toml` managed policy with allowed permission profiles (open bugs show real use: https://github.com/openai/codex/issues/47464), so a vendor-side "profile" concept exists for admins.

### 4. Attitudes to third-party presets

- Guidance is against inheriting broad allow-lists: "if you copy this permissive config over, the risk travels with it" (summary of https://dev.to/claude_code_security/locking-down-claude-code-with-settingsjson-a-practical-guide-to-allow-and-deny-lists-20jk ; search-engine paraphrase, not verified verbatim). UpGuard warns against "copying overly broad permissions" (https://www.upguard.com/blog/yolo-mode-hidden-risks-in-claude-code-permissions).
- Trail of Bits, the one high-profile shared config, says to merge entries into your own file and calls its contents "patterns to adapt, not drop-in configs"; also keep `enableAllProjectMcpServers` false because a repo can ship malicious servers https://github.com/trailofbits/claude-code-config.
- Integrity concern: an unofficial tracker mirror reports the permission files have no integrity protection (https://claudeissues.com/issue/62506-security-settings-local-json-permissions-file-has-no-integrity-protection-silent ; mirror, not verified on GitHub).
- No documented incident of a copied third-party allow-list causing harm was found. No regret threads found.

### 5. Cross-harness drift

- People mostly sync instructions, skills and MCP, not permissions: Ruler (2,957 stars, 31 agents; rules, MCP, skills, subagents; permissions only for OpenCode subagent tools) https://github.com/intellectronica/ruler ; vsync (MCP only) and the survey post https://codex.danielvaughan.com/2026/03/30/codex-cli-config-management-multi-tool/ which says permissions are "not discussed for any tool".
- rulesync (1,582 stars) added a `permissions` feature, Kiro confirmed; `ignore` deprecated in its favour https://github.com/dyoshikawa/rulesync. This is the strongest cross-harness permissions signal found.
- agent-rules-sync: permissions for Claude Code only; strips rules with absolute paths when generating repo copies https://pypi.org/project/agent-rules-sync/1.5.2/.
- Small dotfiles tools generate per-tool formats (Gemini commands as TOML, others Markdown) https://awesome.ecosyste.ms/projects/github.com%2Fjsavyasachi%2Fai-dotfiles. No source claimed a generic settings format is unworkable, and none requested one explicitly.

## Negative

- No Hacker News or Reddit thread on sharing or adopting permission presets surfaced (searches returned guides only). Absence reflects search reach, not proof.
- Both permission-preset requests in Claude Code had zero reactions and were closed not_planned (#8172, #48097).
- Requests that do have volume (AGENTS.md 5,191; skills 162) are about instructions and skills, not settings.
- Sync demand is about one person's machines, not distribution to strangers. Claude Code #22648 lists settings as one of several items.
- 98.9% of sampled public files have no deny rules: people who accumulate allow-lists mostly do not curate them, which cuts against "adopt someone else's vetted preset".
- thegeosman settings repo has 4 stars and is stale (2025-06).
- Cursor: no settings-sharing evidence found; teams share rules only. MCP allowlists for Cursor not covered.
- Gemini CLI, OpenCode: little issue-tracker evidence for settings sharing (Gemini 1 reaction; OpenCode 0).
- Several web sources are SEO guides of uncertain provenance; claims taken from search summaries were not all verified against the page.

## Leads

- Vendor docs lane: Codex `requirements.toml` and Claude managed settings already define admin-side profiles; check what they merge and refuse.
- Security lane: UpGuard and Trail of Bits positions on copied allow-lists versus the repo's trust model.
- Format lane: rulesync `permissions` feature matrix, to see which harnesses map to a common schema.
- Machine-local state lane: Codex `trust_level` and Claude `settings.local.json` show settings files mix portable and local keys.
