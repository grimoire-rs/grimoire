# Research: consuming a grim-generated marketplace repo natively (without grim)

## Metadata

**Date:** 2026-09-29
**Domain:** packaging
**Triggered by:** owner requirement for a use-case docs page "Use a grim marketplace without grim" (marketplace phase 2, `grim export marketplace`)
**Expires:** 2027-03-29 (harness plugin surfaces move monthly; re-verify per-harness rows first)
**Access date for every URL below:** 2026-09-29
**Extends (not repeated):** `research_marketplace_manifest_schemas.md`, `research_marketplace_multi_harness_root.md`, `research_claude_app_install_surfaces.md`, `research_url_marketplace_trust.md`

## Direct Answer

Yes for the CLI/IDE surfaces of Claude Code, Copilot, Codex and Qoder: `<tool> plugin marketplace add owner/repo` (Qoder: `qoder plugins marketplace add`) then install by `plugin@marketplace`, no grim needed. The generated repo's per-harness manifest paths are exactly the ones each tool reads. Three honest limits belong on the docs page:

1. **Claude Code on the web / cloud sessions ignore repo-declared marketplaces and plugins** (`.claude/settings.json` `extraKnownMarketplaces` + `enabledPlugins` are documented as NOT installed there). Only Team/Enterprise **server-managed settings** deliver plugins to a cloud session. Everything committed under `.claude/skills|agents|rules|commands` and `CLAUDE.md` does reach it, but that is not marketplace consumption (it is what `grim install` writes).
2. **claude.ai chat / Claude Desktop DO support "Add marketplace" by GitHub URL** (`Customize > Plugins > Add > Add marketplace`), for skills/commands/remote connectors only; agents and hooks are ignored in chat, a top-level `bin/` makes chat refuse the plugin. Org rollout there is the admin "Sync from GitHub" path, which requires a private/internal repo, default branch only, no tags.
3. **Cursor has no individual "add a git marketplace" flow.** Consumption is: public Cursor marketplace listing, admin **Team Marketplace** import (Teams/Enterprise, dashboard, GitHub/GitLab/Bitbucket/Azure DevOps), or copy into `~/.cursor/plugins/local`. Copilot's per-repo `.github/copilot/settings.json` and Codex's lack of any repo-scoped marketplace config ([openai/codex#18115](https://github.com/openai/codex/issues/18115), open) are the other asymmetries.

Team pinning by config file works natively in Claude Code (`.claude/settings.json`, trust dialog gated), Copilot CLI/VS Code/cloud agent (`.github/copilot/settings.json`, or `.claude/settings.json` in VS Code), and not in Codex. Native pin-to-ref exists on add in Claude (`#ref`), Copilot (`owner/repo#ref`), Codex (`--ref`), and in VS Code (`owner/repo#ref`, merged in 1.123.0). Auto-update is off by default in Claude for third-party marketplaces; Copilot and Codex require explicit opt-in/upgrade too.

## 1. Claude Code (CLI, desktop Code tab, IDE)

Source pages: [install] https://code.claude.com/docs/en/plugins/install (URL served as `/docs/en/discover-plugins`), [create-mk] https://code.claude.com/docs/en/plugin-marketplaces, [host] https://code.claude.com/docs/en/plugins/host-marketplace, [org] https://code.claude.com/docs/en/plugins/org, [loading] https://code.claude.com/docs/en/plugins/loading, [settings] https://code.claude.com/docs/en/settings, [managed] https://code.claude.com/docs/en/managed-settings

### Individual developer, CLI

- Add, in-session: `/plugin marketplace add owner/repo` (GitHub shorthand; `#ref` pins a branch or tag: `/plugin marketplace add your-org/plugins#v1.2.0`). Any git host: full clone URL, `#ref` supported: `https://gitlab.example.com/g/mk.git#v1.0.0`. Local path must start `./` or `../` (a bare `a/b` is read as GitHub shorthand). Also accepts a hosted `https://.../marketplace.json`. [install] "Add a marketplace" table.
- Add, shell (non-interactive): `claude plugin marketplace add <same sources>`; install `claude plugin install <plugin>@<marketplace> [--scope user|project|local]` (user default). Prints `Successfully installed plugin: ... (scope: ...)`. [install] "Install from your shell".
- One-step add+install (v2.1.275+): `/plugin install <plugin> --marketplace owner/repo`. [install] "Add a marketplace and install in one command".
- `/plugin market` is a short form of `/plugin marketplace`. Marketplace name comes from `name` in `marketplace.json`, not the repo name. [host]
- Validate: `claude plugin validate ./my-marketplace` (checks JSON, required fields, `..` in relative source, unknown fields as warnings, per-plugin `plugin.json`). It does NOT catch a missing relative-source dir; that fails at install. [create-mk] "Validate and test".
- List/update/remove: `claude plugin marketplace list|update <name>|remove <name>`; `claude plugin update <plugin>@<marketplace>`. Removing a marketplace uninstalls its plugins. [install] "Manage marketplaces".
- Reserved names: exact official names (e.g. `claude-plugins-official`) refused on add; names imitating official ones fail validate; origin names `inline`, `skills-dir`, `synced` reserved. [create-mk], [loading]

### Update semantics (important for grim-generated repos)

- Version = `plugin.json` `version` first, then marketplace entry `version`, else for `github`/`url`/`git-subdir` sources the 12-char commit SHA; for a relative path inside a git-hosted marketplace the commit SHA of the installed directory. Update detected only when computed version changes. A pinned `version` string + new commits = users get NOTHING. Omit `version` to track commits. Do not set version in both places (validate warns; plugin.json wins silently). [loading] "How Claude Code computes the version", [host] "Release a new version".
- Auto-update is OFF by default for third-party marketplaces (on for official ones and claude.ai-hosted ones). User toggles under `/plugin` > Marketplaces > Enable auto-update, or admin sets `autoUpdate: true` in `extraKnownMarketplaces` entry. Without it: `/plugin marketplace update <name>` or `claude plugin update`. First auto-update pass runs a random delay of up to 10 min after first message; running session keeps loaded versions, prompt `/reload-plugins`. [install] "Keep plugins updated", [loading] "When auto-update runs"
- `claude plugin install name@marketplace` refreshes that marketplace first (not gated by auto-update). [loading]
- Kill switches: `DISABLE_AUTOUPDATER=1`, `DISABLE_UPDATES=1`, `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1` (turn the pass off unless `FORCE_AUTOUPDATE_PLUGINS=1`). [loading]

### What gets cloned

- A git-hosted marketplace is cloned whole onto the machine into `~/.claude/plugins/marketplaces/<name>/` (`CLAUDE_CODE_PLUGIN_CACHE_DIR` relocates). The docs describe no sparse/partial checkout for the marketplace clone itself, and a whole-repo clone is what makes relative `./claude/<plugin>` sources work. Relative-path plugins are copied into `cache/<marketplace>/<plugin>/<version>/`. Git LFS content never downloaded (pointer files only). A bare `marketplace.json` URL downloads only that file, so relative-path entries then fail ("path does not stay inside the marketplace directory"). [host] "Avoid relative-path entries in a URL-hosted marketplace", "Keep plugin files out of Git LFS", [loading] "Find plugins on disk".
- Consequence for a multi-harness repo: the Claude clone contains `copilot/`, `codex/`, `qoder/` trees too (dead weight, harmless). Marketplace `source` entries stay `./claude/<plugin>` (must not contain `..`).
- Sparse: `sparsePaths` (array of dirs, e.g. `[".claude-plugin", "plugins"]`) IS a field of `github`/`git` marketplace sources in `extraKnownMarketplaces`, and `claude plugin marketplace add --sparse` sets it. So a Claude consumer of a multi-harness repo can restrict the clone to `[".claude-plugin", "claude"]`. `git-subdir` plugin sources use a sparse partial clone by definition. https://code.claude.com/docs/en/plugins/marketplace-reference#marketplace-sources (Fields by type). Also accepted add forms: `owner/repo`, `owner/repo@ref`, `owner/repo#ref`; marketplace source `path` field lets the manifest live elsewhere than `.claude-plugin/marketplace.json`.
- Marketplace-source shapes confirmed (settings): `{"source":"github","repo":"o/r","ref":"v1","path":"...","sparsePaths":[...]}`, `{"source":"git","url":"https://...git","ref":"main"}`, `url` (bare marketplace.json), `file`, `directory`, inline `settings`. https://code.claude.com/docs/en/plugins/marketplace-reference#marketplace-sources

### Private repos

- Claude Code has no token of its own and `marketplace.json` has no credential field. Clone runs `git` on the user's machine with prompts suppressed: HTTPS uses the user's credential helper (`gh auth login` + `gh auth setup-git`, macOS Keychain, GCM, `git-credential-store`); SSH needs the host in `known_hosts` and a passphrase-less/agent key. GitHub `owner/repo` shorthand probes `ssh -T git@github.com`, SSH if OK else HTTPS; `CLAUDE_CODE_PLUGIN_PREFER_HTTPS=1` forces HTTPS. `GITHUB_TOKEN` in env alone does not authenticate background auto-update unless a credential helper (e.g. gh) reads it. `CLAUDE_CODE_PLUGIN_KEEP_MARKETPLACE_ON_FAILURE=1` keeps the checkout instead of re-cloning on auth failure. [install] "Add a private marketplace", [host] "Grant access to a private marketplace", "What background auto-update does with credentials".
- CI/CD: `GH_TOKEN` + `gh auth setup-git`; default workflow token only reaches its own repo. [org] "Seed containers and CI".
- GHES: see https://code.claude.com/docs/en/github-enterprise-server#plugin-marketplaces-on-ghes (not fetched here).

### Team config file (repo-pinned)

- Fast path for a repo: `claude plugin marketplace add your-org/mk --scope project` then commit the `.claude/settings.json` it writes. [host] "Register the marketplace for everyone in a repository"
- Hand-written form (same keys as managed): in `.claude/settings.json`
  ```json
  {
    "extraKnownMarketplaces": {
      "your-marketplace": { "source": { "source": "github", "repo": "your-org/your-marketplace", "ref": "v1.2.0" } }
    },
    "enabledPlugins": { "code-formatter@your-marketplace": true }
  }
  ```
  (`ref` on a `github` marketplace source in `extraKnownMarketplaces` confirmed in the marketplace reference, "Fields by type": `ref`, `path`, `sparsePaths` are valid on `github` and `git` sources. https://code.claude.com/docs/en/plugins/marketplace-reference#marketplace-sources)
- Trust gate: `extraKnownMarketplaces` in a repo applies only after the contributor accepts the workspace trust dialog (interactive) or, for `claude -p`, only in folders already trusted (or `hasTrustDialogAccepted` in `~/.claude.json`); in an untrusted folder it is ignored silently. `enabledPlugins` alone applies at session start. [org] "Require plugins per repository", "When each surface applies the plugin keys"
- Relative-path plugins (which is what a grim marketplace repo uses) load from the marketplace copy once registered. But if the plugin lives elsewhere (external source) the contributor must still run `claude plugin install <p>@<mk> --scope project` once; otherwise `/plugin` Errors tab: `Plugin "<n>" is enabled in project settings but isn't installed here`. [org], [loading] "Enabled in project settings but not installed"
- Precedence: local > project > user for `enabledPlugins`; disable locally via `.claude/settings.local.json`. [install] "Choose an install scope"

### Claude Code on the web / cloud sessions (claude.ai/code, Desktop Cloud, mobile, routines)

- **Repo `.claude/settings.json` marketplaces/plugins are NOT honoured in cloud sessions.** Verbatim: "Plugins and marketplaces declared in your repo's `.claude/settings.json` | No | A cloud session doesn't install the plugins a repository turns on under `enabledPlugins`, including ones from the marketplaces it lists under `extraKnownMarketplaces`." Reason given: `extraKnownMarketplaces` needs the workspace trust dialog which a cloud session never shows. User-scope plugins are also not carried. https://code.claude.com/docs/en/cloud-environments#what-carries-over-from-your-setup, https://code.claude.com/docs/en/plugins/loading#plugins-shared-through-a-repository, https://code.claude.com/docs/en/settings#settings-in-cloud-sessions
- Also: `/plugin` command is not available in cloud sessions (terminal-only command). https://code.claude.com/docs/en/claude-code-on-the-web#manage-context
- What DOES reach a cloud session: (a) everything committed under `.claude/skills/`, `.claude/agents/`, `.claude/commands/`, `.claude/rules/`, `CLAUDE.md`, and (single-repo sessions) `.mcp.json`; (b) org **server-managed settings** ("A cloud session fetches these settings before it installs plugins": managed `extraKnownMarketplaces` + `enabledPlugins` do apply in Anthropic-hosted environments); (c) skills enabled on claude.ai ("Cloud sessions automatically load skills you enable on claude.ai"). https://code.claude.com/docs/en/plugins/org#when-each-surface-applies-the-plugin-keys, https://code.claude.com/docs/en/cloud-environments#what-carries-over-from-your-setup
- Synced claude.ai plugins (`<name>@synced`) are documented for Cowork and terminal sessions signed in to claude.ai (v2.1.273+); cloud sessions are NOT listed. https://code.claude.com/docs/en/plugins/loading#synced-plugins
- Server-managed settings require Team/Enterprise and an Owner: https://code.claude.com/docs/en/plugins/org#choose-a-delivery-mechanism ; MDM/`managed-settings.json` on a device do NOT reach an Anthropic-hosted cloud session (they do in a self-hosted environment runner image if the file is in the image). https://code.claude.com/docs/en/settings#settings-in-cloud-sessions
- Network: default **Trusted** access allowlists github.com, api.github.com, codeload.github.com, raw.githubusercontent.com etc.; **Custom** takes own domain list; **None** blocks. GitHub traffic goes through a dedicated proxy that only reaches repositories attached to the session ("GitHub API and release-asset requests reach only repositories attached to the session"). Whether a session-side `git clone` of a second (marketplace) repo works through that proxy is NOT documented: unverified. https://code.claude.com/docs/en/cloud-environments#github-proxy, #default-allowed-domains
- Honest story for the docs: in a Claude Code cloud session, a grim marketplace repo is not natively consumable through per-repo config. Options: (1) org with Team/Enterprise: server-managed settings deliver `extraKnownMarketplaces` + `enabledPlugins`; (2) commit the plugin's skills/agents/rules into the consuming repo's `.claude/` (this is what `grim install` project scope does; not native marketplace); (3) unofficial and untested: a repo SessionStart hook or environment setup script running `claude plugin marketplace add` + `claude plugin install`, subject to network/proxy limits and load-at-next-start timing. Do not document (3) as supported.

### Organisations / customers (Claude Code)

- Managed settings, three delivery mechanisms, first wins by default (server-managed > MDM > file; `managedSourcesBehavior: "merge"` applies all): server-managed (Owner, claude.ai admin console > Claude Code > Managed settings), MDM (plist / registry), file: macOS `/Library/Application Support/ClaudeCode/managed-settings.json`, Linux and WSL `/etc/claude-code/managed-settings.json`, Windows `C:\Program Files\ClaudeCode\managed-settings.json` (legacy `C:\ProgramData\ClaudeCode\` NOT read), plus `managed-settings.d/*.json` drop-ins. https://code.claude.com/docs/en/managed-settings (lines: "File-based"), https://code.claude.com/docs/en/plugins/org#choose-a-delivery-mechanism
- Force-install: managed `extraKnownMarketplaces` (keyed by marketplace name, with `source` and optional `autoUpdate`) + managed `enabledPlugins` (`true` force-enables and users cannot disable; `false` blocks and hides). Applies at next session start. https://code.claude.com/docs/en/plugins/org#require-a-marketplace-and-its-plugins
- Restrict: `strictKnownMarketplaces` (alias `allowedMarketplaces`, v2.1.232+) allowlist of sources: `github` (+ optional `ref`, `path`; owner wildcard `your-org/*`, v2.1.223+), `git`, `url`, `file`, `directory`, `hostPattern`, `pathPattern`, `skills-dir`. Matching is exact including `ref`/`path`: an entry without `ref` does NOT cover a source with `ref: "main"`; a `github` entry does not cover the equivalent `git` URL. `blockedMarketplaces` (canonicalised matching, checked first). `[]` locks out everything including official. `disableSideloadFlags`. https://code.claude.com/docs/en/plugins/org#restrict-what-users-can-install
- `forceRemoveDeletedPlugins` is a TOP-LEVEL field of **marketplace.json** (not a settings key): when true, plugins removed from the marketplace are uninstalled at session start; pairs with `renames` map (v2.1.193+) that migrates users when an entry is renamed or removed (`null`). https://code.claude.com/docs/en/plugins/host-marketplace#rename-or-remove-a-plugin
- Release channels: no channel concept; host two marketplaces (different `name`) pointing at different refs; admin assigns per group via separate endpoint-managed settings or Claude apps gateway policy (server-managed applies org-wide, cannot target groups). https://code.claude.com/docs/en/plugins/org#assign-release-channels-to-user-groups, https://code.claude.com/docs/en/plugins/host-marketplace#run-release-channels
- Seed for no-git-account users / air-gapped: `CLAUDE_CODE_PLUGIN_CACHE_DIR=/opt/seed claude plugin marketplace add ...` at image build; runtime `CLAUDE_CODE_PLUGIN_SEED_DIR=/opt/seed`; read-only, autoUpdate forced off; still needs `enabledPlugins`. https://code.claude.com/docs/en/plugins/org#seed-containers-and-ci
- Audit: OTel `claude_code.plugin_installed`, `claude_code.plugin_loaded`; Enterprise Analytics API `GET /v1/organizations/analytics/plugins`. https://code.claude.com/docs/en/plugins/org#audit-and-review

### claude.ai web app, Claude Desktop (chat + Cowork)

- YES, marketplaces are supported by URL: `Customize > Plugins > Add > Add marketplace`, enter `https://github.com/your-org/your-plugins` or `owner/repo`. Self-added: GitHub, GitHub Enterprise, public GitLab and public Bitbucket. Private GitHub: connect GitHub account + grant the Claude GitHub App access to the repo. Up to 25 self-added marketplaces per account per org; plugin limit 5,000 files / 200 MB. Updates: **Check for updates**, and for github.com marketplaces **Sync automatically**. Plugin lands on the account, so also in Cowork, and Claude Code terminal as `@synced`. https://claude.com/docs/plugins/overview#find-and-add-a-plugin, https://claude.com/docs/plugins/platform-support#compare-installation-sync-and-admin-controls
- Components in chat: skills and commands (as skills) and remote MCP connectors load; agents, hooks, local MCP, `bin/` do not (a top-level `bin/` makes chat and Cowork refuse the whole plugin). Cowork adds agents/hooks/local MCP. https://claude.com/docs/plugins/platform-support#compare-component-support-by-app
- Anthropic does not review marketplace-URL plugins ("add those only from sources you trust"). https://claude.com/docs/plugins/overview#before-you-add-a-plugin
- Team/Enterprise: Owner sets **User-created skills** policy; when OFF, members cannot use Add marketplace/Upload (plugin options vanish). https://claude.com/docs/plugins/admin#control-what-members-add-themselves
- Org distribution to claude.ai + Cowork: Owner **Organization settings > Plugins & skills > Add > Sync from GitHub / Sync from GitLab**; per-plugin **Default access** = Not available / Available to install / Installed by default / Required (Enterprise: per user group). Members need no repo access. Requirements: marketplace repo must be **private or internal** on github.com and gitlab.com; plugin sources only `github`, `url`, `git-subdir`, or relative `./` path (bare names under `metadata.pluginRoot` rejected); reads **default branch only**, tags not read; syncs on **Re-sync** click or push to default branch with **Sync automatically** (webhook); nothing on a schedule; top-level `bin/` in a plugin rejected (rest of marketplace still syncs). Claude GitHub App must be installed on the repo; GitLab needs an admin GitLab configuration (beta). https://claude.com/docs/plugins/org-sync, https://claude.com/docs/plugins/admin#add-your-own-plugins, https://claude.com/docs/plugins/org-rollout#update-through-organization-settings
- Marketplace repo on the org route for a *public* github.com repo: org-sync requires private/internal; so a public grim marketplace repo cannot be org-synced on github.com (members add it themselves via Add marketplace instead, if User-created skills is on). Inference from the stated rule; not tested.
- Skill-only zip upload and `.mcpb` are covered in `research_claude_app_install_surfaces.md`.

### Gotchas (Claude) affecting a grim-generated repo

1. `#ref` pin on add is the only native pin for the *catalog*; relative-path plugins then come from the same ref. Per-plugin `ref`/`sha` only applies to `github`/`url`/`git-subdir` sources, not relative paths.
2. Do not emit `version` in `plugin.json` unless grim bumps it on every publish, else consumers silently stay stale. Commit SHA tracking is free when `version` is omitted.
3. Auto-update off by default: docs must tell consumers to run `/plugin marketplace update <name>` or turn it on.
4. Marketplace name = `name` field; keep entry name = plugin.json name.
5. `bin/` at plugin top level breaks chat/Cowork/org-sync install of the whole plugin.
6. Cloud sessions ignore repo plugin config (above).
7. Allowlist entries must match `ref`/`path` exactly, so an org that pins `#v1` in users' add command needs the same `ref` in `strictKnownMarketplaces`.


## 2. GitHub Copilot (CLI, VS Code, JetBrains, Copilot app, cloud agent)

Sources: [gh-mk] https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/plugins-marketplace, [gh-ref] https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-plugin-reference, [gh-cfg] https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference, [vsc] https://code.visualstudio.com/docs/agent-customization/agent-plugins, [ent-ref] https://docs.github.com/en/copilot/reference/enterprise-administrators/enterprise-managed-settings, [ent-how] https://docs.github.com/en/copilot/how-tos/administer-copilot/manage-for-enterprise/manage-agents/configure-enterprise-plugin-standards

### Individual

- CLI add: `copilot plugin marketplace add SOURCE` where SOURCE is `owner/repo`, `owner/repo#ref`, any git URL, or a local path. Install: `copilot plugin install PLUGIN@MARKETPLACE` (also `OWNER/REPO`, `OWNER/REPO:PATH/TO/PLUGIN`, git URL, local path; no ref syntax documented for install). Also `marketplace list|browse NAME|update [NAME] (alias refresh)|remove NAME`, `plugin update NAME|--all`, `enable|disable|uninstall`. [gh-ref] "Plugin source specifications" and "Marketplace subcommands"
- `marketplace.json` lookup in a marketplace repo, first hit wins: `marketplace.json`, `.plugin/marketplace.json`, `.github/plugin/marketplace.json`, `.claude-plugin/marketplace.json`. So in a grim multi-harness repo Copilot CLI picks `.github/plugin/marketplace.json` (`./copilot/<p>`) over `.claude-plugin`, as intended. [gh-ref] "Manifest discovery and caching"; probe-confirmed in `research_marketplace_multi_harness_root.md` (local-path only)
- Cache: `~/.cache/copilot/marketplaces/` (Linux), `~/Library/Caches/copilot/marketplaces/` (macOS), `COPILOT_CACHE_HOME` overrides; installed plugins under `installed-plugins/<marketplace>/<plugin>/` in the config dir. [gh-ref], [gh-cfg]
- VS Code: no add command; set `chat.plugins.marketplaces` in user settings: array of `owner/repo` (public GitHub queried through the public GitHub API), `https://...git`, SCP `git@github.com:org/repo.git`, `file:///...`; example `"chat.plugins.marketplaces": ["anthropics/claude-code"]`. Ref syntax `owner/repo#ref` merged May 2026 (milestone 1.123.0). Search `@agentPlugins` in Extensions view to install. Command Palette `Chat: Install Plugin From Source` takes a git URL for a single plugin. Default marketplaces: copilot-plugins (removal proposed, [vscode#336163](https://github.com/microsoft/vscode/issues/336163)) and awesome-copilot. [vsc]; https://github.com/microsoft/vscode/pull/317901
- VS Code accepts Claude-format plugins (`.claude-plugin/plugin.json`, `${CLAUDE_PLUGIN_ROOT}`). VS Code's per-marketplace `marketplace.json` lookup order is NOT documented in the fetched docs: unverified whether `.github/plugin` or `.claude-plugin` wins in a repo that has both. [vsc]

### Team config file

- Copilot CLI settings hierarchy: `~/.copilot/settings.json`, `.github/copilot/settings.json` (repo, shared), `.github/copilot/settings.local.json`. Keys `extraKnownMarketplaces` (source `directory`, `git` or `github`; `autoUpdate: true` opt-in per entry) and `enabledPlugins` ("declarative plugin auto-install", `"plugin@marketplace": true`). [gh-cfg]
- Repo file is read by **Copilot cloud agent too**: "you install plugins declaratively by adding them to the `enabledPlugins` field of the repository's `.github/copilot/settings.json` ... can also add the marketplace to `extraKnownMarketplaces` in the same file". https://docs.github.com/en/copilot/concepts/agents/about-plugins (via search-result excerpt plus fetch)
- VS Code reads workspace recommendations from `.claude/settings.json` or `.github/copilot/settings.json` with the same two keys (`{"extraKnownMarketplaces":{"company-tools":{"source":{"source":"github","repo":"your-org/plugin-marketplace"}}},"enabledPlugins":{"code-formatter@company-tools":true}}`). [vsc]. Behaviour differences vs CLI are being aligned: [microsoft/vscode#336858](https://github.com/microsoft/vscode/issues/336858) (title only, not fetched).
- Caveats: [github/copilot-cli#2249](https://github.com/github/copilot-cli/issues/2249) (closed) reported `enabledPlugins` in `.github/copilot/settings.json` not auto-installing on CLI 1.0.11 (2026-03-24); no fix statement visible. Internal/private-repo marketplaces configured via `.github-private` are not reachable by Copilot cloud agent ([community#200387](https://github.com/orgs/community/discussions/200387), open, unanswered by staff, 2026-06-28). So repo-pinned config works for a PUBLIC grim marketplace repo on the cloud agent; private is unsupported/unclear.

### Org / enterprise

- Copilot Business/Enterprise: enterprise-managed `managed-settings.json` (formerly `settings.json`) at `copilot/managed-settings.json` (with `team-mappings.json`, `teams/*.json`) in the `.github-private` repo of a designated org; applies at sign-in, propagates ~1 h, restart/re-sign-in refreshes. Keys: `enabledPlugins`, `extraKnownMarketplaces` (`github`: `repo`, `ref`, `path`; `git`: `url`, `ref`, `path`; `directory`: `path`; `autoUpdate`), `strictKnownMarketplaces` (`github`, `git`, `url`+`headers`, `npm`, `file`, `directory`, `hostPattern`, `pathPattern`; `[]` = lockdown). Honoured by Copilot CLI, VS Code, Copilot app, **Copilot cloud agent**, JetBrains. Managed values cannot be overridden locally. [ent-ref], [ent-how]; changelogs https://github.blog/changelog/2026-05-06-enterprise-managed-plugins-in-github-copilot-cli-are-now-in-public-preview/, https://github.blog/changelog/2026-06-25-enterprise-managed-settings-now-support-strictknownmarketplaces-in-vs-code-and-the-cli/, https://github.blog/changelog/2026-08-26-enterprise-managed-settings-now-support-autoupdate-for-plugin-marketplaces/, https://github.blog/changelog/2026-07-01-enterprise-managed-settings-json-is-generally-available/ (title only)
- MDM route exists for VS Code and CLI: https://github.blog/changelog/2026-07-08-deploy-managed-copilot-settings-via-mdm-in-vs-code-and-cli/ (title only, not fetched).

### Update semantics, auth, clone

- Auto-update: first-party plugins auto-update at session start; custom marketplaces opt in with `autoUpdate: true` on the `extraKnownMarketplaces` entry (interactive and `-p` only); disable globally with `autoUpdate: false` or `COPILOT_AUTO_UPDATE=false`. Otherwise `copilot plugin marketplace update` + `copilot plugin update`. VS Code checks on "Check for Extension Updates" or every 24 h when `extensions.autoUpdate` is on; npm/PyPI-sourced plugins never auto-update. [gh-ref], [vsc]
- Private repos: not documented for the CLI. Reported behaviour: 1.0.70 "fail fast when git auth needs a terminal prompt" regression disabled credential helpers (`-c credential.helper=`), breaking private HTTPS (Azure DevOps) marketplaces ([github/copilot-cli#4103](https://github.com/github/copilot-cli/issues/4103), closed, 2026-07-12); workaround was manual clone + local-directory marketplace. VS Code: "Private repositories are also supported. If a public lookup fails, VS Code falls back to cloning" and since 1.140.0 (2026-09-24) reuses an existing VS Code GitHub session on 401/403 ([vscode#337586](https://github.com/microsoft/vscode/pull/337586), [vscode#337199](https://github.com/microsoft/vscode/issues/337199)). Copilot CLI token env vars `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN` authenticate Copilot itself, not necessarily the marketplace git clone (fine-grained PAT required; classic `ghp_` ignored): https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/authenticate-copilot-cli (via search excerpt, not fetched).
- Clone: VS Code clones git remotes on first `@agentPlugins` search (cache `~/.config/Code/agentPlugins/github.com/{org}/{repo}` Linux; `~/Library/Application Support/Code/agentPlugins/...` macOS; `%APPDATA%\Code\agentPlugins\...` Windows). Copilot CLI clones into its cache. No sparse option documented for either.

### Gotchas (Copilot)

- Entry `source` in Copilot's file may omit `./`; both forms resolve to the same dir. [gh-mk]
- Copilot requires `owner` (name+email) in its marketplace file per prior research; not re-checked.
- Pin: `owner/repo#ref` on add (CLI, VS Code). Enterprise `extraKnownMarketplaces` github/git sources accept `ref`.
- Plugin marketplace name for Copilot: kebab-case, max 64 chars, dots allowed. [gh-ref]

## 3. Codex (CLI, ChatGPT desktop/web workspace)

Sources: [cx-build] https://developers.openai.com/codex/plugins/build, [cx-cli] https://learn.chatgpt.com/docs/developer-commands?surface=cli (developers.openai.com/codex/cli/reference 308-redirects here), [cx-plugins] https://learn.chatgpt.com/docs/plugins (from developers.openai.com/codex/plugins), [cx-admin] https://learn.chatgpt.com/docs/enterprise/plugin-management, PR https://github.com/openai/codex/pull/21396 (merged 2026-05-14)

- Add: `codex plugin marketplace add owner/repo`, `... owner/repo --ref main`, `... https://github.com/example/plugins.git --sparse .agents/plugins`, `... ./local-marketplace-root`. Sources: GitHub shorthand (`owner/repo` or `owner/repo@ref`), HTTP(S) git URL, SSH git URL, local dir. `--ref REF` pins; repeatable `--sparse PATH` for git sources only; `--json`. [cx-build], [cx-cli]
- Install: `codex plugin add PLUGIN@MARKETPLACE` (or `PLUGIN -m MARKETPLACE`), `codex plugin list [--json] [--available]`, `codex plugin remove`. In the TUI: `/plugins`; start a new session after install. [cx-cli], [cx-plugins], PR #21396 (marketplaces are apt-style "only install sources", cached artifacts alone cannot enable installs)
- Manage: `codex plugin marketplace list|upgrade [name]|remove <name>`. `upgrade` refreshes git marketplaces (re-runs `git ls-remote`, re-clones only when SHA moved, per third-party write-up of config `last_revision`; not primary-confirmed). No auto-upgrade documented. [cx-cli]
- Marketplace file lookup: `$REPO_ROOT/.agents/plugins/marketplace.json`, legacy `$REPO_ROOT/.claude-plugin/marketplace.json`, personal `~/.agents/plugins/marketplace.json`; first hit wins (code, prior research: `.agents/plugins` beats `.claude-plugin`, so a grim repo yields only the Codex marketplace with `./codex/<p>`). [cx-build]; `research_marketplace_multi_harness_root.md`
- Double-listing risk: [openai/codex#19372](https://github.com/openai/codex/issues/19372) (open, 2026-04-24, v0.124.0) is Codex auto-importing/mirroring marketplaces found in `.claude-plugin/marketplace.json` (Claude's `~/.claude/plugins` marketplaces, code in `external-agent-migration/source_cla.rs`) into `~/.codex/.tmp/marketplaces/` without the user declaring them, so a machine with Claude Code plugins installed can show Claude-format plugins in Codex, whose `${CLAUDE_PLUGIN_ROOT}` MCP configs then fail handshake. It concerns the user's Claude install state, not a repo added by `codex plugin marketplace add`; prior research left the by-URL case unprobed. Workaround: `enabled = false` per leaked plugin in `config.toml`. For grim docs: mention as a known upstream quirk, not a grim bug.
- Plugin manifest: portable `plugin.json` at plugin root; `.codex-plugin/plugin.json` remains a supported compatibility fallback. Cache: `~/.codex/plugins/cache/<marketplace>/<plugin>/<version|local>/`. [cx-build]
- Config storage: `$CODEX_HOME/config.toml` `[marketplaces.<name>]` (`source_type`, `source`, `ref`, `sparse_paths`, `last_updated`, `last_revision`) recorded by add (third-party summary of PR #17087; not in official docs). Plugin enable flags: `[plugins."plugin@marketplace"] enabled = true`, documented in repo-level `.codex/config.toml` example in [cx-build], but repo-scoped **marketplace registration is not supported**: [openai/codex#18115](https://github.com/openai/codex/issues/18115) (open, 2026-04-16): "marketplace registrations ... remain user-scoped"; a repo can only provide its own local marketplace at `./.agents/plugins/marketplace.json`. Consequence: no committed team file can make Codex fetch a remote marketplace; teams must document the `codex plugin marketplace add` command, or vendor the marketplace/plugins into the consuming repo.
- Org: ChatGPT workspace admins import a GitHub marketplace (Workspace settings > Plugins > Add > Import marketplace; GitHub repo URL, optional subdirectory and branch/tag/commit, authorize GitHub; public and private repos; new marketplaces sync daily, **Sync now** on demand; installation policy Available/Installed per role; sync imports plugin content but does not connect member accounts to a plugin's apps). Source: OpenAI Help Center article https://help.openai.com/en/articles/20001504-importing-and-syncing-plugin-marketplaces-from-github (HTTP 403 to fetch; facts taken from a web-search result summary of it, treat as medium confidence) and [cx-admin]. Whether workspace marketplaces reach Codex CLI is not stated. `requirements.toml` `features.plugin_sharing = false` disables workspace publishing of plugins ([cx-build]); no documented allowlist for marketplace sources in the fetched pages. IDE extension: plugins not available ([cx-plugins]).
- Private repo auth: not documented; assume git credentials on the machine (unverified). Codex cloud tasks: no documented plugin/marketplace support found.

### Gotchas (Codex)

- Codex requires `./`-prefixed local sources and rejects non-normal path components; the file's entry shape differs from Claude's (`{"source":"local","path":"./x"}`): prior research.
- Pin only on add (`--ref`); nothing rewrites when the ref moves except `marketplace upgrade`.
- Two Codex plugin manifests coexist (root `plugin.json` vs `.codex-plugin/plugin.json`): a grim-generated `./codex/<plugin>/` must follow whichever the pinned Codex accepts; both are accepted per docs.

## 4. Cursor

Sources: [cur-ref] https://cursor.com/docs/reference/plugins, [cur-doc] https://cursor.com/docs/plugins, forum https://forum.cursor.com/t/cursor-2-6-team-marketplaces-for-plugins/153484, template https://github.com/fieldsphere/cursor-team-marketplace-template (not fetched)

- Repo layout: `.cursor-plugin/marketplace.json` at repo root; for an entry `"source": "my-plugin"` the parser looks for `my-plugin/.cursor-plugin/plugin.json`; every plugin needs `.cursor-plugin/plugin.json` (only `name` required: lowercase kebab-case, alphanumerics, hyphens, periods). [cur-ref] So a Cursor tree in a grim repo (`./cursor/<p>/`) needs its own `.cursor-plugin/plugin.json`. Note Cursor docs (2026-09) also mention a root `plugin.json` "either plugin format" for local testing. [cur-doc]
- Individual users: install from the Customize panel ("Select Install and choose a project or user scope") from marketplaces Cursor surfaces (public listing at cursor.com/marketplace). **No documented way for an individual to add an arbitrary git repo as a marketplace.** Publishing to the public marketplace: submit repo at https://cursor.com/marketplace/publish. Manual/local: put a plugin folder in `~/.cursor/plugins/local/<name>` and restart ("Symlinks load only when the target resolves inside that folder"). [cur-doc]; admin toggle **Allow Local Plugin Imports** (off by default on Enterprise) can block this.
- Team marketplaces (Teams: 1, Enterprise: unlimited): admin `Dashboard > Plugins & MCPs > Team Marketplaces > Add Marketplace > Import from Repo`; docs now say GitHub, GitLab, Bitbucket, Azure DevOps URLs (forum thread from the 2.6 launch said GitHub only: docs win, but verify). Private GitHub repos need the Cursor GitHub App; **Enable Auto Refresh** (GitHub imports, push webhook, needs the GitHub App) or manual **Refresh**. Distribution: Default Off / Default On / Required; Marketplace Access restricts to Organization Groups. [cur-doc]; forum lists limits: "cannot reference external repos within marketplace definitions", IDE sync lags dashboard discovery.
- Pin: tracks "the branch the marketplace tracks"; no tag/SHA pin documented. Sparse: no. Update for individuals: not applicable. Cursor CLI and cloud agents loading marketplace plugins: not documented (only Team MCP servers are said to reach Cloud Agents). [cur-doc]
- Gotcha: forum says team marketplace manifests cannot reference external repos; grim's relative `./cursor/<p>` layout is compatible, external-source entries are not.

## 5. Qoder (grim also emits `.qoder-plugin/marketplace.json` -> `./qoder/<plugin>/`)

Source: https://docs.qoder.com/cli/plugins.md; prior research `research_marketplace_manifest_schemas.md` (qodercli 1.1.64 bundle read).

- `qoder plugins marketplace add https://git.example.com/org/mk.git | git@git.example.com:org/mk.git | org/mk | https://example.com/marketplace.json`; install `qoder plugins install <name>` (searches all configured marketplaces; ids `name@marketplace`); `marketplace list|update [name]|remove <name>`; `plugins list --available --json`; `plugins update <name>`. Docs do not state manifest location, team config keys, private auth or enterprise controls. No `#ref` syntax documented.

## 6. Per-harness table

| Harness | Add command (exact) | Config-file form for teams | Admin / managed form | Update semantics | Private-repo auth | Gotchas |
|---|---|---|---|---|---|---|
| Claude Code CLI / desktop Code tab / IDE | `claude plugin marketplace add owner/repo[#ref]` or `/plugin marketplace add ...`; then `claude plugin install p@mk [--scope project]`; one-step `/plugin install p --marketplace owner/repo` | `.claude/settings.json`: `extraKnownMarketplaces` (`github`/`git` + `ref`,`path`,`sparsePaths`) + `enabledPlugins`; or `claude plugin marketplace add o/r --scope project` and commit; applies after workspace trust; external-source plugins still need per-user `install --scope project` | managed `extraKnownMarketplaces` + `enabledPlugins` (force), `strictKnownMarketplaces`/`blockedMarketplaces`, via server-managed (Owner, claude.ai), MDM, or `managed-settings.json` (`/etc/claude-code/`, `/Library/Application Support/ClaudeCode/`, `C:\Program Files\ClaudeCode\`) | version = plugin.json > entry > commit SHA; auto-update OFF by default for 3rd-party; `/plugin marketplace update`, `claude plugin update`; install refreshes marketplace first | user's git credential helper / SSH agent, prompts suppressed; `gh auth setup-git`; `CLAUDE_CODE_PLUGIN_PREFER_HTTPS=1` | pinned `version` starves updates; cloud sessions ignore repo config; allowlist matches `ref` exactly; whole-repo clone (use `sparsePaths`) |
| Claude Code on the web / cloud | none (no `/plugin`); nothing native from repo | repo `enabledPlugins`/`extraKnownMarketplaces` NOT honoured; commit `.claude/skills|agents|rules` instead | server-managed settings only (Team/Enterprise Owner) | n/a | GitHub proxy; session-side clone of a second repo undocumented | see Section 1; don't promise |
| claude.ai web / Claude Desktop chat + Cowork | `Customize > Plugins > Add > Add marketplace` (GitHub URL or `owner/repo`; GHE; public GitLab/Bitbucket) | n/a | Owner: `Organization settings > Plugins & skills > Add > Sync from GitHub/GitLab`, Default access per plugin (Enterprise per group); policy `User-created skills` gates member adds | manual **Check for updates**; **Sync automatically** for self-added github.com; org sync = default branch only, on push (webhook) or **Re-sync**, tags ignored | private GitHub: connect GitHub + Claude GitHub App on repo; org sync needs private/internal repo | chat ignores agents/hooks/local MCP; `bin/` rejects plugin; 25 self-added marketplaces; 200 MB/5,000 files |
| Copilot CLI | `copilot plugin marketplace add owner/repo[#ref]` (or URL/path); `copilot plugin install p@mk` | `.github/copilot/settings.json` (also `~/.copilot/settings.json`): `extraKnownMarketplaces` + `enabledPlugins` | enterprise `managed-settings.json` in `.github-private` (`extraKnownMarketplaces`, `enabledPlugins`, `strictKnownMarketplaces`, `autoUpdate`); Business/Enterprise | `autoUpdate: true` per marketplace entry opts in; else `marketplace update` + `plugin update`; lookup `.github/plugin` before `.claude-plugin` | undocumented; regressions reported (#4103, closed); SSH URL or manual clone as workaround | repo-config auto-install had a bug report (#2249); no install-time ref syntax |
| Copilot VS Code / JetBrains | setting `chat.plugins.marketplaces: ["owner/repo#ref", "https://...git", "git@...", "file:///..."]`; install via `@agentPlugins`; `Chat: Install Plugin From Source` | workspace `.claude/settings.json` or `.github/copilot/settings.json` with same two keys | same enterprise file; MDM route | checks on demand / every 24 h when `extensions.autoUpdate` | public via GitHub API, falls back to clone; reuses VS Code GitHub session since 1.140.0 | marketplace.json lookup order undocumented for VS Code |
| Copilot cloud agent | none | repo `.github/copilot/settings.json` `enabledPlugins` + `extraKnownMarketplaces` (documented) | enterprise managed file honoured | n/a | private/internal marketplace repos unreachable (community#200387) | public marketplace repos only, in practice |
| Codex CLI | `codex plugin marketplace add owner/repo [--ref r] [--sparse p]`; `codex plugin add p@mk` | none for remote marketplaces (#18115 open); repo `.codex/config.toml` can enable `[plugins."p@mk"]` and repo can carry its own local `.agents/plugins/marketplace.json` | ChatGPT workspace admin **Import marketplace** (GitHub, daily sync, per-role Available/Installed); `requirements.toml` `features.plugin_sharing` | no auto-upgrade documented; `codex plugin marketplace upgrade [name]` | undocumented (git credentials assumed) | `.agents` beats `.claude-plugin`; #19372 auto-import quirk; IDE extension has no plugins |
| Cursor | none for individuals; Customize panel install from Cursor-visible marketplaces; manual `~/.cursor/plugins/local/<name>` | none | admin Team Marketplace `Import from Repo` (Teams 1 / Enterprise unlimited), Default Off/On/Required, org groups; `Allow Local Plugin Imports` | Enable Auto Refresh on push (GitHub App) or manual Refresh; tracks a branch, no tag pin | Cursor GitHub App for private | per-plugin `.cursor-plugin/plugin.json` required; no external-repo entries |
| Qoder CLI | `qoder plugins marketplace add org/repo` (or git URL/SSH/JSON URL); `qoder plugins install name` | not documented | not documented | `marketplace update`, `plugins update` | not documented | no ref pin syntax documented |

## 7. Pinning, sparse, clone summary

| Harness | Pin to ref/tag | Sparse / partial | What gets cloned |
|---|---|---|---|
| Claude Code | `#ref` on add; `ref` (+`path`,`sparsePaths`) in settings source; plugin-entry `ref`/`sha` for external sources | yes: `--sparse` / `sparsePaths` on `github`/`git` marketplace sources | whole repo to `~/.claude/plugins/marketplaces/<name>/` (LFS as pointers) |
| Copilot CLI / VS Code | `owner/repo#ref` (CLI reference; VS Code 1.123+), `ref` in enterprise sources | not documented | repo cloned to Copilot cache / VS Code `agentPlugins` dir; VS Code reads public GitHub repos via API first |
| Codex | `--ref` (or `owner/repo@ref`) | yes `--sparse PATH` (repeatable, git only) | git checkout to a Codex-managed marketplace dir; plugin copy to `plugins/cache/<mk>/<p>/<ver>/` |
| Cursor | tracks a branch; no tag pin documented | no | server-side import by Cursor (not on user machine) |
| Qoder | not documented | not documented | not documented |

## 8. Recommended outline: "Use a grim marketplace without grim"

Doc type: how-to (declare `doc_type: how-to`, `doc_tier: integration`, or split into three pages if the length gate trips). Each scenario section: goal sentence first, exact commands, a "you will see" line, then a one-line limit.

0. **Before you start** (3 lines): the repo exposes one marketplace per tool at the paths above; pick your tool's tab; `grim` is not needed; a pinned `#ref` is how you freeze what you get. Name the marketplace `name` you registered (needed for `plugin@marketplace`).
1. **One developer, one machine** (per-harness tabs, each 3-4 lines): Claude Code (`claude plugin marketplace add owner/repo#v1.2.0`, `claude plugin install p@mk`, `claude plugin marketplace update mk` to refresh, note auto-update off by default, note `--sparse` optional), Copilot CLI (`copilot plugin marketplace add owner/repo#v1.2.0`), VS Code (`chat.plugins.marketplaces` JSON), Codex (`codex plugin marketplace add owner/repo --ref v1.2.0`, `codex plugin add p@mk`, `marketplace upgrade`), Qoder, Cursor (honest: no individual git add; use admin path or local folder). Private repos: one shared paragraph (credential helper / SSH agent, `gh auth setup-git`), with the Copilot CLI caveat.
2. **A repository that pins the marketplace for its team**: Claude Code `.claude/settings.json` snippet (with `ref`), trust dialog note, one-time `claude plugin install ... --scope project` only if a plugin uses an external source (grim relative-path plugins do not need it); Copilot/VS Code/cloud agent `.github/copilot/settings.json` snippet; Codex: no committed config (state it, link #18115) so put the `codex plugin marketplace add` line in the README/onboarding script; Cursor: n/a at repo level. Sub-section **Claude Code on the web**: state plainly that repo plugin config is ignored in cloud sessions; choices are server-managed settings (Team/Enterprise) or commit skills into `.claude/` (that is what `grim install` does, link the grim page). Sub-section **Copilot cloud agent**: repo file works for public marketplace repos.
3. **Organisation-managed rollout**: Claude Code (managed `extraKnownMarketplaces` + `enabledPlugins`; `strictKnownMarketplaces` incl. exact-`ref` matching; file paths per OS; server-managed reaches cloud sessions; `forceRemoveDeletedPlugins` + `renames` are marketplace.json fields grim would need to emit to retire plugins); claude.ai/Cowork admin Sync from GitHub (private/internal repo, default branch, no tags, no `bin/`); Copilot enterprise `.github-private/copilot/managed-settings.json` (all surfaces incl. cloud agent); Codex workspace Import marketplace; Cursor Team Marketplace. One comparison table (rows = harness, columns = mechanism, who configures, pin semantics, reaches cloud?).
4. **Customers and external audiences** (public repo): publish repo public; give each audience the one-liner for their tool; claude.ai chat users use Add marketplace (mention components ignored in chat); remind that Anthropic does not review marketplace-URL plugins so consumers must trust the source (link the trust page).
5. **Updates and pinning cheat-sheet** (table above, short) and **Troubleshooting** stubs: `Plugin "x" is enabled in project settings but isn't installed`, `marketplace not refreshed`, allowlist `ref` mismatch, Copilot private clone, Codex double-listing.

Implication for `grim export marketplace` itself (feeds design, not docs): omit `version` in emitted Claude `plugin.json`/entries unless bumped each publish; avoid top-level `bin/` in plugins if claude.ai/org-sync is a target; keep entry name = plugin name; keep relative sources so `url`-only marketplaces are not needed; consider emitting `forceRemoveDeletedPlugins`/`renames` when a plugin is retired.

## Recommendation

Write the page around the four tools with a real add command (Claude Code, Copilot, Codex, Qoder), plus honest sections for the three surfaces that break the pattern: Claude Code cloud (repo config ignored, server-managed only), Codex teams (no committed config), Cursor (admin import only). Pin every example with a tag (`#v1.2.0` / `--ref`) so the page teaches a reproducible install. Do not document the unofficial cloud workaround (SessionStart hook or setup script running `claude plugin marketplace add`) as supported.

## Gaps and unverified (do not state as fact in docs without a probe)

- VS Code's `marketplace.json` lookup order (`.github/plugin` vs `.claude-plugin`) in a multi-file repo.
- Whether a git clone of a second repo (the marketplace) works inside an Anthropic-hosted cloud session through the GitHub proxy or Trusted allowlist; whether `claude plugin marketplace add` can run from a setup script.
- Copilot CLI private HTTPS auth today (regression fixed or not on current builds), and whether repo `enabledPlugins` auto-install works on current CLI (#2249 closed without visible fix).
- Codex private-repo auth, Codex cloud plugin support, whether workspace-imported marketplaces reach the CLI, any `requirements.toml` marketplace allowlist, and whether #19372 fires for a repo added by URL.
- Cursor: whether Import from Repo really accepts GitLab/Bitbucket/Azure DevOps (docs vs forum), and behaviour with several `*-plugin` dirs in one repo (untested).
- Qoder team/admin/private-repo behaviour.
- OpenAI Help Center article 20001504 could not be fetched (HTTP 403); its facts come from a search-result summary.
- Version-specific: Claude one-step `--marketplace` install needs v2.1.275+; `strictKnownMarketplaces` alias v2.1.232+; owner wildcard v2.1.223+; claude.ai marketplace add v2.1.273+; VS Code ref syntax 1.123.0+; VS Code private session auth 1.140.0.

## Sources

| Source | Type | Date accessed | Relevance |
|---|---|---|---|
| https://code.claude.com/docs/en/plugins/install (fetched as /docs/en/discover-plugins) | Docs | 2026-09-29 | Claude add/install/scopes/auto-update/private |
| https://code.claude.com/docs/en/plugin-marketplaces | Docs | 2026-09-29 | create/validate/host |
| https://code.claude.com/docs/en/plugins/host-marketplace | Docs | 2026-09-29 | versions, private auth, channels, renames, forceRemoveDeletedPlugins |
| https://code.claude.com/docs/en/plugins/org | Docs | 2026-09-29 | managed settings, allowlist, seed, surface table |
| https://code.claude.com/docs/en/plugins/loading | Docs | 2026-09-29 | version resolution, synced plugins, project-not-installed, cloud |
| https://code.claude.com/docs/en/plugins/marketplace-reference | Docs | 2026-09-29 | source shapes, sparsePaths, ref, validation |
| https://code.claude.com/docs/en/cloud-environments | Docs | 2026-09-29 | what carries over, network, GitHub proxy |
| https://code.claude.com/docs/en/claude-code-on-the-web | Docs | 2026-09-29 | cloud session limits, /plugin unavailable |
| https://code.claude.com/docs/en/settings | Docs | 2026-09-29 | settings in cloud sessions |
| https://code.claude.com/docs/en/managed-settings | Docs | 2026-09-29 | managed file paths |
| https://claude.com/docs/plugins/overview | Docs | 2026-09-29 | claude.ai Add marketplace |
| https://claude.com/docs/plugins/admin | Docs | 2026-09-29 | org admin availability, sync |
| https://claude.com/docs/plugins/org-sync | Docs | 2026-09-29 | sync repo requirements |
| https://claude.com/docs/plugins/org-rollout | Docs | 2026-09-29 | rollout routes, update via sync |
| https://claude.com/docs/plugins/platform-support | Docs | 2026-09-29 | component support, limits |
| https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/plugins-marketplace | Docs | 2026-09-29 | Copilot marketplace add |
| https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-plugin-reference | Docs | 2026-09-29 | Copilot commands, lookup, autoUpdate |
| https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference | Docs | 2026-09-29 | settings.json keys |
| https://docs.github.com/en/copilot/concepts/agents/about-plugins | Docs | 2026-09-29 | cloud agent reads repo settings |
| https://docs.github.com/en/copilot/reference/enterprise-administrators/enterprise-managed-settings | Docs | 2026-09-29 | enterprise keys, sources, client matrix |
| https://docs.github.com/en/copilot/how-tos/administer-copilot/manage-for-enterprise/manage-agents/configure-enterprise-plugin-standards | Docs | 2026-09-29 | .github-private paths, propagation |
| https://code.visualstudio.com/docs/agent-customization/agent-plugins | Docs | 2026-09-29 | chat.plugins.marketplaces, workspace config |
| https://github.blog/changelog/2026-08-12-agent-plugins-1-0-in-vs-code-copilot-cli-and-the-copilot-app/ | Changelog | 2026-09-29 | Agent Plugins 1.0 GA |
| https://github.blog/changelog/2026-06-25-enterprise-managed-settings-now-support-strictknownmarketplaces-in-vs-code-and-the-cli/ | Changelog | 2026-09-29 | strictKnownMarketplaces |
| https://github.blog/changelog/2026-08-26-enterprise-managed-settings-now-support-autoupdate-for-plugin-marketplaces/ | Changelog | 2026-09-29 | enterprise autoUpdate |
| https://github.blog/changelog/2026-05-06-enterprise-managed-plugins-in-github-copilot-cli-are-now-in-public-preview/ | Changelog | 2026-09-29 | enterprise managed plugins |
| https://github.com/github/copilot-cli/issues/2249, /4103 | Issues | 2026-09-29 | repo-config and private-clone bugs |
| https://github.com/orgs/community/discussions/200387 | Discussion | 2026-09-29 | cloud agent internal marketplace gap |
| https://github.com/microsoft/vscode/pull/317901, /pull/337586, /issues/337199 | PR/Issue | 2026-09-29 | VS Code ref support, private auth |
| https://developers.openai.com/codex/plugins/build | Docs | 2026-09-29 | Codex add/lookup/cache/config |
| https://learn.chatgpt.com/docs/developer-commands?surface=cli | Docs | 2026-09-29 | codex plugin subcommands |
| https://learn.chatgpt.com/docs/plugins, /docs/enterprise/plugin-management | Docs | 2026-09-29 | Codex UI, workspace import |
| https://github.com/openai/codex/pull/21396, /issues/19372, /issues/18115 | PR/Issue | 2026-09-29 | CLI commands, auto-import, repo-scoped gap |
| https://help.openai.com/en/articles/20001504-importing-and-syncing-plugin-marketplaces-from-github | Help Center | 2026-09-29 | workspace import (403; via search summary) |
| https://cursor.com/docs/reference/plugins, https://cursor.com/docs/plugins | Docs | 2026-09-29 | Cursor layout, team marketplaces, local |
| https://forum.cursor.com/t/cursor-2-6-team-marketplaces-for-plugins/153484 | Forum | 2026-09-29 | team marketplace limits |
| https://docs.qoder.com/cli/plugins.md | Docs | 2026-09-29 | Qoder commands |

## Durable search terms

`plugin marketplace add`, `claude plugin marketplace add --sparse`, `extraKnownMarketplaces`, `strictKnownMarketplaces`, `forceRemoveDeletedPlugins`, `CLAUDE_CODE_PLUGIN_SEED_DIR`, `managed-settings.json plugins`, "What carries over from your setup" (cloud-environments), `syncClaudeAiPlugins`, "Sync from GitHub" organization plugins, `copilot plugin marketplace add`, `chat.plugins.marketplaces`, `.github/copilot/settings.json enabledPlugins`, `managed-settings.json .github-private copilot`, `codex plugin marketplace add --ref --sparse`, `codex plugin marketplace upgrade`, `.agents/plugins/marketplace.json`, openai/codex#18115 (repo-scoped marketplace), openai/codex#19372 (claude marketplace auto-import), `learn.chatgpt.com plugins`, "Import marketplace" workspace Codex, Cursor "Team Marketplaces" "Import from Repo", `~/.cursor/plugins/local`, `.cursor-plugin/marketplace.json`, `qoder plugins marketplace add`.
