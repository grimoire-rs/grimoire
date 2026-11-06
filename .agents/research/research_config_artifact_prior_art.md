# Research: Harness configuration distribution prior art

## Metadata

**Date:** 2026-11-03
**Domain:** packaging
**Triggered by:** `.agents/discussions/config-artifact-type.md`
**Expires:** 2027-05-03

Method note: vendor docs fetched 2026-11-03 (WebFetch); security items are mostly secondary reporting (WebSearch) and are marked where the primary advisory was not read. Nothing cited here is older than about 18 months (earliest: Check Point hooks report, Jul-Aug 2025, and CVE-2025-53773, Aug 2025, both under 16 months). Undated pages (EditorConfig, devcontainer spec, Renovate, ESLint, VS Code profiles, chezmoi) are living docs; re-verify before relying on specifics. Kiro, Windsurf and Cursor coverage is thin (see `negative`).

## Direct Answer

Settings are already a first-class, but narrowly scoped and heavily guarded, shared artifact in every major harness. Evidence by payload shape (no pick made):

**One payload per harness (native shape)**
- For: every harness uses a different file, format and key vocabulary (JSON `settings.json`, TOML `config.toml`, `opencode.json`, `.kiro/settings/*.json`), and same-named concepts differ (see matrix). Microsoft APM and the devcontainer spec both end up emitting per-target output. APM writes MCP env refs differently per target (Codex `bearer_token_env_var`, Cursor/VS Code runtime refs) https://microsoft.github.io/apm/reference/manifest-schema/. The devcontainer spec delegates `customizations` merging to each tool: "Merging is left to the tools" https://containers.dev/implementors/spec/.
- Against: the author must know every harness; each vendor change breaks a payload. No evidence found of anyone shipping per-harness *settings* payloads in a registry at scale.

**One generic payload**
- For: EditorConfig shows a generic cross-tool vocabulary can work when the concept set is tiny and stable (indent, EOL, charset) https://editorconfig.org/. Only generic data that survives translation: MCP server definitions (APM translates one declaration to many harnesses), home-manager `programs.mcp.servers` merged into `programs.claude-code.mcpServers` https://nix-community.github.io/home-manager/options/home-manager/programs/claude-code.html.
- Against: permission/approval models are not isomorphic (matrix below). EditorConfig works because its keys are few; it also provides no per-tool escape hatch on the page read. No vendor-neutral standard for permissions or model defaults was found.

**Mix (generic core + per-tool namespaces)**
- For: devcontainer.json is exactly this: generic fields with defined merge rules plus `customizations.<tool>` that tools interpret (https://containers.dev/implementors/spec/). Gemini extensions mix generic MCP/context fields with a few vendor keys (https://geminicli.com/docs/extensions/reference/). Claude plugin manifest has typed generic slots (`mcpServers`, `hooks`, `userConfig`) plus a narrow vendor `settings` object.
- Against: the mix inherits both costs; devcontainer leaves merging undefined for the per-tool part, so conflict semantics live in each tool.

**Could settings be a distributable type? Evidence from vendors:** all vendors that let a package carry settings restrict it to a tiny allowlist (Claude: two keys; Gemini: no `trust`, no `allow`/`yolo` policies). Every settings-carrying path that is repo-resident has produced CVEs (section Security). Vendors that centralise policy (Claude managed settings, Codex `requirements.toml`, OpenCode managed config, Windsurf org ceilings) do so through admin channels, not packages.

### Harness comparison (compact)

| Harness | File / format | Scopes, precedence (high to low) | Permissions model | Carries hooks / env / cred helpers / endpoints |
|---|---|---|---|---|
| Claude Code | `settings.json` JSON | managed, `--settings`, project local, project shared, user; lists merge across files; deny beats allow from any scope https://code.claude.com/docs/en/settings , https://code.claude.com/docs/en/permissions | allow/ask/deny rule strings per tool; modes; project `allow` waits for workspace trust, `deny`/`ask` apply at once | hooks, `env`, `apiKeyHelper` yes (used in project settings only after trust) |
| Codex CLI | `config.toml`; profiles `~/.codex/<name>.config.toml` | CLI flags, project (closest wins, trusted only), profile, user, cloud-managed, `/etc/codex/config.toml`, defaults https://learn.chatgpt.com/docs/config-file/config-basic | `approval_policy` + `sandbox_mode`; named permission profiles; `requirements.toml` forbids values (e.g. `never`, `danger-full-access`) | hooks (`[features] hooks = true`), `shell_environment_policy`, model providers |
| Gemini CLI | `settings.json` | defaults, user, project, system (override), env, CLI https://geminicli.com/docs/reference/configuration | `tools.allowed`, `confirmationRequired`, `defaultApprovalMode` (YOLO CLI-only), policy files | hooks, env refs, MCP env; `security.allowedExtensions` regex list |
| OpenCode | `opencode.json` | remote `.well-known`, global, `OPENCODE_CONFIG`, project, `.opencode` dirs, inline env, managed file, macOS MDM; "merged, not replaced" https://opencode.ai/docs/config/ | `permission` per tool, default mostly allow | plugins (hooks), `{env:}`/`{file:}` substitution |
| Kiro | `.kiro/settings/mcp.json` etc. | user and workspace MCP merged, workspace wins https://kiro.dev/docs/mcp/configuration | not verified | MCP only verified |
| Windsurf | IDE settings; org admin console | org ceiling on terminal auto-exec (Disabled/Allowlist/Auto/Turbo); users pick at or below https://docs.devin.ai/windsurf/plugins/guide-for-admins | four-level auto-execution + allow/deny lists | MCP allowlist; hooks via `.windsurf/hooks.json` per APM |
| Copilot / VS Code | VS Code `settings.json`, `.vscode/mcp.json`, `.github/copilot-instructions.md` | user, workspace; GitHub org policies | `chat.tools.*` settings | MCP in `.vscode/mcp.json`; org policy for coding agent and MCP |
| Cursor | `.cursor/mcp.json`, rules | not verified (docs URL redirected) | not verified | MCP |

Same-name differences: "approval" (Codex `approval_policy` is when to pause; Gemini `defaultApprovalMode` is edit/plan modes; Claude has rule lists plus modes); "profile" (Codex = overlay file; VS Code = bundle of settings/extensions/MCP; Windsurf has none); "managed" (Claude/OpenCode = highest precedence file; Codex `requirements.toml` = constraint not default; Windsurf = ceiling). Claude honors a *stricter* project value over managed for some keys (table "Exceptions to managed settings precedence" in https://code.claude.com/docs/en/settings), a third semantics.

## Technology Landscape

### Trending (gaining momentum)

| Tool/Pattern | Adoption Signal | Key Benefit | Relevance |
|--------------|------------------|-------------|-----------|
| Multi-harness package managers for skills/instructions (APM, `npx skills`, Tessl, ClawHub) | Vercel launched skills.sh Jan 2026; ClawHub counts 13k-44k skills (sources disagree) | One manifest, many harness outputs | Same space as grim; none found distributing permissions/model settings |
| Admin-delivered managed policy (server-managed, MDM, `requirements.toml`) | Present in Claude, Codex, OpenCode, Windsurf | Org-wide enforcement without repo files | Settings distribution that vendors accept goes through this, not packages |

### Established (proven, widely accepted)

| Tool/Pattern | Status | Notes |
|--------------|--------|-------|
| Scope stack user < project < local < managed | Standard in Claude, Gemini, Codex, OpenCode | Lists merge in Claude and OpenCode, per-key override elsewhere |
| Workspace trust gating for repo-carried config | Claude, Codex, Gemini | Codex: "Codex loads project `.codex/` layers only when you trust the project" |
| devcontainer features with `customizations.<tool>` | Spec-defined | Generic + per-tool + defined merge for core keys |
| ESLint flat config arrays, Renovate `extends`/`ignorePresets`, EditorConfig cascade | Mature | Layered, last-wins, explicit opt-out mechanism |

### Emerging (early but promising)

| Tool/Pattern | Signal | Worth Watching Because |
|--------------|--------|-------------------------|
| Plugin-shipped `settings.json` (Claude) | Two keys only: `agent`, `subagentStatusLine` | Vendor-sanctioned precedent and its deliberate narrowness |
| Plugin `userConfig` with `sensitive` keychain storage (Claude), extension `settings` (Gemini) | Both vendors added prompted per-user values | Separates shareable shape from per-person secrets |
| APM `policy:` block, `apm-policy.yml` | Working draft schema 2026-03-06 | Consumer-side allow-list of dependencies and targets |

### Declining (losing mindshare)

| Tool/Pattern | Signal | Avoid Because |
|--------------|--------|-----------------|
| Silent trust of repo-resident config pre-prompt | Fixed after several CVEs (see Security) | Vendors moved to trust-before-load and consent-on-change |
| Codex `untrusted` approval policy | Page says retired | Naming churn: approval vocabularies shift between releases |

## Design Patterns Worth Considering

- **Key allowlist for package-carried settings.** Claude plugin settings: "Only `agent` and `subagentStatusLine` take effect; other keys are dropped at load" https://code.claude.com/docs/en/plugins-reference ; plugin defaults are the lowest layer, user settings override https://code.claude.com/docs/en/plugins/components . No reason is stated on those pages; the surrounding material implies least privilege.
- **Packages may not self-approve.** Gemini extension policies cannot use `allow` or `yolo`; MCP `trust` unsupported; stated reason: extensions should not approve tool calls or bypass safeguards https://geminicli.com/docs/extensions/reference/ . Extensions do not inherit the shell environment.
- **Restrict-only keys honored from any scope.** Claude honors `true` of e.g. `permissions.blockReadsOutsideWorkingDirectories` from any scope; `deny`/`ask` apply before trust "since they only restrict" https://code.claude.com/docs/en/permissions .
- **Value-level prompting, secrets out of files.** Claude `userConfig` `sensitive: true` goes to secure storage; non-sensitive to `pluginConfigs` in user settings https://code.claude.com/docs/en/plugins-reference .
- **Separate included file instead of editing the user's file.** Claude plugin defaults layer (never writes user settings); VS Code profile is a separate bundle that cannot inherit another profile https://code.visualstudio.com/docs/configure/profiles ; chezmoi `modify_` scripts for partial-file edits https://www.chezmoi.io/user-guide/manage-different-types-of-file/ .
- **Explicit opt-out of inherited layers.** Renovate `ignorePresets` (must match the fully resolved preset string, repo setting beats preset) https://docs.renovatebot.com/config-presets/ ; EditorConfig `unset` and `root = true` https://editorconfig.org/ .
- **Per-variable merge rules declared in the spec.** devcontainer: `mounts` "Last source wins" for conflicts, `containerEnv` "Per variable, last value wins", `devcontainer.json` considered last https://containers.dev/implementors/spec/ .
- **Install-time resolution of secrets by reference.** APM writes env references per target instead of literal tokens https://microsoft.github.io/apm/reference/manifest-schema/ .

## Key Findings

1. A Claude Code plugin may ship settings, but only `agent` and `subagentStatusLine`; all other keys are dropped at load; a root `settings.json` beats the manifest `settings` key; user settings override plugin defaults; two plugins setting one key: "the value from the plugin loaded last applies" (last-writer-wins, debug-logged). https://code.claude.com/docs/en/plugins/components , https://code.claude.com/docs/en/plugins-reference
2. Claude plugins can also ship hooks, MCP servers (with `env`), LSP servers, monitors, `bin/` executables, plus `userConfig`; i.e. executable surface is wide while the *settings* surface is two keys. https://code.claude.com/docs/en/plugins-reference
3. Claude lists merge across scopes; `fallbackModel`, `modelPicker` take whole value from the highest scope; deny from any scope beats allow. https://code.claude.com/docs/en/settings
4. Project `permissions.allow`, `additionalDirectories`, `extraKnownMarketplaces` and most `env` values wait for workspace trust; `permissions.defaultMode` `auto`/`bypassPermissions` do not take effect from project or local settings; telemetry env vars ignored from project files. https://code.claude.com/docs/en/settings
5. Codex: project `.codex/` layers (config, hooks, rules) load only for trusted projects; `requirements.toml` constrains values (config only supplies defaults); profiles are separate files `<name>.config.toml`; cloud-managed config can define plugin marketplaces and default enablement. https://learn.chatgpt.com/docs/config-file/config-basic . Whether Codex plugins can carry settings was not found.
6. Gemini extensions carry `mcpServers`, `settings` (user-supplied values, keychain if sensitive), `contextFileName`, `excludeTools`, themes; on conflict workspace/`settings.json` wins; policies sit in tier 2 above defaults, below user/admin. https://geminicli.com/docs/extensions/reference/
7. OpenCode merges config files key-wise across eight sources; managed file and macOS MDM profile are unoverridable. https://opencode.ai/docs/config/
8. APM (Microsoft) distributes instructions, skills, prompts, agents, hooks, plugins, MCP; no documented distribution of permissions or model settings; reinstall preserves "unrelated servers and settings"; `AGENTS.md` `managed_section` replaces only the marker-delimited block; conflicts merged "at instruction level (by `applyTo`), not file level"; schema is a Working Draft. https://microsoft.github.io/apm/reference/manifest-schema/
9. Tessl tiles hold docs, rules, skills; Vercel `npx skills` installs skills with `skills-lock.json`; ClawHub is a versioned skill/plugin registry. None documented as distributing harness settings. https://docs.tessl.io/reference/configuration , https://vercel.com/docs/agent-resources/skills , https://docs.openclaw.ai/clawhub.md
10. VS Code profiles bundle settings, keybindings, MCP servers, snippets, tasks, extensions; shareable by gist or `.code-profile` file; no inheritance between profiles; machine-specific settings excluded from export. https://code.visualstudio.com/docs/configure/profiles
11. Security: CVE-2026-33068 (Claude, fixed 2.1.53): permission mode read from repo `.claude/settings.json` before the trust dialog, so the prompt could be skipped. CVE-2026-21852: project `ANTHROPIC_BASE_URL` redirected API traffic pre-trust (fixed 2.0.65; secondary source). CVE-2025-59536: code ran before trust acceptance (fixed 1.0.111). CVE-2026-40068: trust bypass via git worktree `commondir` spoof. Check Point hooks report (GHSA-ph6w-f82w-28w6). https://raxe.ai/labs/advisories/RAXE-2026-040 , https://advisories.gitlab.com/npm/@anthropic-ai/claude-code/CVE-2026-40068/ , https://www.sentinelone.com/vulnerability-database/cve-2025-59536/ , https://www.theregister.com/2026/02/26/clade_code_cves/
12. Security: Cursor CVE-2025-54136 (MCPoison): approved MCP config trusted forever even if later changed; fix: re-approval on every modification of an entry. CVE-2025-54135 (CurXecute): prompt injection created `.cursor/mcp.json` without confirmation. https://www.tenablecloud.cn/blog/faq-cve-2025-54135-cve-2025-54136-vulnerabilities-in-cursor-curxecute-mcpoison
13. Security: Copilot CVE-2025-53773: injected prompt wrote `chat.tools.autoApprove: true` to workspace settings, enabling auto-approve; fix: approval for config changes that affect security settings (Aug 2025). https://embracethered.com/blog/posts/2025/github-copilot-remote-code-execution-via-prompt-injection
14. Security: Codex CVE-2025-61260: `.env` setting `CODEX_HOME=./.codex` made repo `config.toml` `mcp_servers` run at startup without approval; fix in 0.23.0 stops `.env` redirecting `CODEX_HOME` into the project. https://research.checkpoint.com/2025/openai-codex= (fixed-version wording disagrees between Check Point and CVE text).
15. Security: Gemini CLI GHSA-wpqr-6v78-jr5g (reported fixed in 0.17.2): malicious `.gemini/settings.json` ran commands under insecure default workspace trust; primary advisory not read. https://dev.to/cverports/ghsa-wpqr-6v78-jr5g-ghsa-wpqr-6v78-jr5g-remote-code-execution-in-google-gemini-cli-via-workspace-f7e
16. Marketplace-distributed config: PromptArmor shows an injected marketplace plugin bypassing human-in-the-loop and exfiltrating files; Prompt Security: connecting to a marketplace implies every plugin may change agent behaviour; a campaign wrote `ANTHROPIC_BASE_URL` into `~/.claude/settings.json` and shell profile. https://promptarmor.substack.com/p/hijacking-claude-code-via-injected , https://prompt.security/blog/when-your-plugin-starts-picking-your-dependencies-marketplace-skills-and-dependency-hijack-in-claude-code , https://xygeni.io/?p=74389
17. Conflict and reversal in structural prior art: devcontainer per-variable last-wins; ESLint flat config objects "merged with later objects overriding previous objects"; Renovate `ignorePresets` removes inherited presets; EditorConfig `unset`; home-manager declarative generation (MCP servers merged, Claude entries win); chezmoi `modify_` edits parts of a file. https://containers.dev/implementors/spec/ , https://eslint.org/docs/latest/use/configure/configuration-files , https://docs.renovatebot.com/config-presets/ , https://editorconfig.org/ , https://nix-community.github.io/home-manager/options/home-manager/programs/claude-code.html
18. Mitigation patterns vendors chose: trust-before-load; re-consent on change (Cursor); allowlist of package-settable keys (Claude); no self-approval by packages (Gemini); restrict-only keys effective without trust (Claude); admin ceilings (Windsurf, Codex `requirements.toml`, OpenCode managed).

## Conflict and reversal approaches seen (evidence only)

| Approach | Where seen | Evidence |
|---|---|---|
| Key-level ownership by layer (package defaults lowest layer; user wins) | Claude plugin settings | https://code.claude.com/docs/en/plugins/components |
| Last-writer-wins between packages | Claude (plugins), devcontainer (`mounts`, `containerEnv`), ESLint flat config, APM MCP name collisions | same sources as above |
| List append/union across scopes | Claude permission lists; devcontainer `mounts` (collected list) | https://code.claude.com/docs/en/settings |
| Whole-value replace for order-sensitive keys | Claude `fallbackModel`, `modelPicker` | https://code.claude.com/docs/en/settings |
| Marker-delimited managed region in a shared file | APM `AGENTS.md managed_section` | https://microsoft.github.io/apm/reference/manifest-schema/ |
| Separate included/overlay file, tool never edits user file | Claude plugin defaults, Codex profile files, VS Code profile | sources above |
| Partial-file patch script | chezmoi `modify_` | https://www.chezmoi.io/user-guide/manage-different-types-of-file/ |
| Explicit removal from inheritance | Renovate `ignorePresets`, EditorConfig `unset` | sources above |
| Uninstall restoring a previous value | Not found documented in any system read; Claude deletes `${CLAUDE_PLUGIN_DATA}` on last uninstall (data, not settings) https://code.claude.com/docs/en/plugins-reference ; layered approaches get reversal implicitly by dropping the layer | gap |

## Sources

| Source | Type | Date | Relevance |
|--------|------|------|-----------|
| https://code.claude.com/docs/en/plugins-reference | Docs | live 2026-11 | Plugin manifest, `settings` key limit, `userConfig`, merge of hooks/MCP |
| https://code.claude.com/docs/en/plugins/components | Docs | live | Default settings rules, last plugin wins, agent frontmatter ignored fields |
| https://code.claude.com/docs/en/settings | Docs | live | Scopes, precedence, list merge, trust-gated keys, exceptions table |
| https://code.claude.com/docs/en/permissions | Docs | live | Trust rules, deny>allow across scopes |
| https://learn.chatgpt.com/docs/config-file/config-basic | Docs | live | Codex layers, profiles, requirements, trust |
| https://geminicli.com/docs/reference/configuration | Docs | live (last 16k chars unread) | Gemini scopes, tools, hooks, trust |
| https://geminicli.com/docs/extensions/reference/ | Docs | live | Extension manifest, restrictions and reasons |
| https://opencode.ai/docs/config/ | Docs | live | OpenCode merge order, managed config |
| https://kiro.dev/docs/mcp/configuration | Docs (via search snippet) | live | Kiro MCP merge |
| https://docs.devin.ai/windsurf/plugins/guide-for-admins | Docs (via search snippet) | live | Windsurf org auto-exec ceiling |
| https://microsoft.github.io/apm/reference/manifest-schema/ | Docs | Working Draft 2026-03-06 | APM targets, MCP, merge, policy |
| https://docs.tessl.io/reference/configuration , https://vercel.com/docs/agent-resources/skills , https://docs.openclaw.ai/clawhub.md | Docs / search summary | 2026 | Tessl, skills.sh, ClawHub |
| https://containers.dev/implementors/spec/ | Spec | undated | devcontainer merge and `customizations` |
| https://code.visualstudio.com/docs/configure/profiles | Docs | undated | VS Code profiles |
| https://eslint.org/docs/latest/use/configure/configuration-files | Docs | undated | Flat config composition |
| https://docs.renovatebot.com/config-presets/ | Docs | undated | Presets, `ignorePresets` |
| https://editorconfig.org/ | Docs | undated | Generic cross-tool format |
| https://nix-community.github.io/home-manager/options/home-manager/programs/claude-code.html | Docs | undated | home-manager Claude module |
| https://www.chezmoi.io/user-guide/manage-different-types-of-file/ | Docs | undated | `modify_` scripts |
| https://raxe.ai/labs/advisories/RAXE-2026-040 | Advisory (third party) | 2026-03 | CVE-2026-33068 |
| https://advisories.gitlab.com/npm/@anthropic-ai/claude-code/CVE-2026-40068/ | Advisory | 2026 | Worktree trust bypass |
| https://www.sentinelone.com/vulnerability-database/cve-2025-59536/ | CVE DB | 2025 | Pre-trust execution |
| https://www.theregister.com/2026/02/26/clade_code_cves/ | News | 2026-02 | Claude CVE roundup |
| https://adversa.ai/blog/trustfall-coding-agent-security-flaw-rce-claude-cursor-gemini-cli-copilot/ | Blog | 2026-04 | TrustFall, vendor position contested |
| https://research.checkpoint.com/2025/openai-codex= | Research | 2025-12 | Codex CVE-2025-61260 |
| https://www.tenablecloud.cn/blog/faq-cve-2025-54135-cve-2025-54136-vulnerabilities-in-cursor-curxecute-mcpoison | Blog | 2025-08 | Cursor MCPoison, CurXecute |
| https://embracethered.com/blog/posts/2025/github-copilot-remote-code-execution-via-prompt-injection | Blog | 2025-08 | Copilot CVE-2025-53773 |
| https://promptarmor.substack.com/p/hijacking-claude-code-via-injected , https://prompt.security/blog/when-your-plugin-starts-picking-your-dependencies-marketplace-skills-and-dependency-hijack-in-claude-code , https://xygeni.io/?p=74389 | Blogs | 2026 | Marketplace and settings-persistence attacks |

## Gaps and negative evidence

- Contradiction: Adversa's TrustFall says Anthropic's position is that accepting the trust dialog covers the whole project configuration (report declined); other vendors (Cursor) chose re-consent on change. Vendor stances differ.
- Contradiction: Codex CVE-2025-61260 fixed version disagrees (Check Point 0.23.0 fix vs CVE text "0.23.0 and earlier").
- Not found: any package manager distributing permissions/approval settings; `skillctl` (no matching tool; closest `skill-cli`, `skillsctl` which distributes skills catalogs); Codex plugin settings payload; Kiro/Cursor/Windsurf settings-file details and Cursor docs (URL redirected); a documented uninstall-restores-prior-value mechanism; home-manager `programs.claude-code` `settings` merge behaviour; Windsurf system-config file.
- Several security items rest on secondary reporting; primary advisories for CVE-2026-21852 and the Gemini GHSA were not read.
