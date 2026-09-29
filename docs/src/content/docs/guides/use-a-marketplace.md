---
title: Use a plugin marketplace without grim
description: Add a grim-generated marketplace repository straight from Claude Code, Copilot, Codex, Qoder or Cursor, pin it for a team, and roll it out to an organization or to customers, with no grim install.
---
<!-- doc_type: how-to -->
<!-- doc_tier: integration -->

Goal: install a plugin from a marketplace repository that someone else
generated with grim, using only your own coding tool. Nothing on this
page needs `grim` on your machine. It also covers pinning a version,
sharing the marketplace with a team, and rolling it out to an
organization or to customers.

## Before you start {#before-you-start}

You need the marketplace repository's address and one of the supported
tools. The examples use the repository `grimoire-rs/e2e-marketplace`,
which registers itself under the marketplace name `grimoire-e2e` and
offers the plugins `grim-essentials` and `hex`. Swap in your own names.

A grim marketplace repository carries one marketplace file per tool,
each pointing at that tool's own plugin trees:

| Tool | Marketplace file | Plugin trees |
|---|---|---|
| Claude Code | `.claude-plugin/marketplace.json` | `claude/<plugin>/` |
| Copilot | `.github/plugin/marketplace.json` | `copilot/<plugin>/` |
| Codex | `.agents/plugins/marketplace.json` | `codex/<plugin>/` |
| Qoder | `.qoder-plugin/marketplace.json` | `qoder/<plugin>/` |
| Cursor | `.cursor-plugin/marketplace.json` | `cursor/<plugin>/` |

Your tool reads only its own file, so the other trees stay unused. The
curator picks which tools the repository serves. Cursor is opt-in, so
its file may be missing. Two names matter for every command below: the
marketplace name from the repository's manifest, and the plugin name. You
install as `<plugin>@<marketplace>`.

Anthropic does not review plugins that come from a marketplace address
you add yourself. Add only repositories you trust, because an installed
plugin can carry skills, agents and MCP servers that run on your machine.

## Install on one machine {#one-machine}

Every command here pins `v1.2.0`, an example release tag. A curator cuts
such a tag when a version is ready, and the repository above has
published none yet. Use a tag your curator published for a reproducible
result. Or leave `#v1.2.0` off to follow the default branch.
Details on pinning and updating are in the [cheat sheet](#pin-and-update).

### Claude Code {#claude-code}

*Verified 2026-09-29 against the [install
guide][claude-install] and the [marketplace reference][claude-reference].*

```sh
claude plugin marketplace add grimoire-rs/e2e-marketplace#v1.2.0
claude plugin install grim-essentials@grimoire-e2e
```

The same commands work in a running session as `/plugin marketplace add
…` and `/plugin install …`. A one-step form,
`/plugin install grim-essentials --marketplace grimoire-rs/e2e-marketplace`, needs
Claude Code 2.1.275 or later. Any git host works with a full clone URL
such as `https://gitlab.example.com/acme/mk.git#v1.2.0`.

Claude Code clones the whole repository, including the trees of the
other tools. The add command's `--sparse` option, and the `sparsePaths`
field in settings, restrict the clone to the `.claude-plugin` and
`claude` folders.

### Copilot CLI and VS Code {#copilot}

*Verified 2026-09-29 against the [Copilot CLI plugin
reference][copilot-reference] and the [VS Code agent plugins
page][vscode-plugins].*

```sh
copilot plugin marketplace add grimoire-rs/e2e-marketplace#v1.2.0
copilot plugin install grim-essentials@grimoire-e2e
```

Copilot looks for `.github/plugin/marketplace.json` before
`.claude-plugin/marketplace.json`, so it picks its own tree. The install
command takes no ref, so the pin lives on the marketplace add.

VS Code has no add command. Put the repository in your user settings,
then search `@agentPlugins` in the Extensions view and install from
there:

```json
{
  "chat.plugins.marketplaces": ["grimoire-rs/e2e-marketplace#v1.2.0"]
}
```

The `#ref` form needs VS Code 1.123 or later. VS Code documents no
preference between a Copilot file and a Claude file in one repository.
Check that the plugin you see is the one you expect.

### Codex {#codex}

*Verified 2026-09-29 against the [Codex plugin build guide][codex-build]
and the [command reference][codex-cli].*

```sh
codex plugin marketplace add grimoire-rs/e2e-marketplace --ref v1.2.0
codex plugin add grim-essentials@grimoire-e2e
```

Start a new Codex session after the install. Codex reads
`.agents/plugins/marketplace.json` first, so it ignores the Claude file.
To fetch less, repeat `--sparse` on the marketplace add with the paths
`.agents/plugins` and `codex`.

### Qoder {#qoder}

*Verified 2026-09-29 against the [Qoder plugin docs][qoder-plugins].*

```sh
qoder plugins marketplace add grimoire-rs/e2e-marketplace
qoder plugins install grim-essentials@grimoire-e2e
```

Qoder documents no way to pin a ref. On a host other than GitHub, end
the address in `.git`, because Qoder reads any other `https` address as
a direct link to a JSON file. Qoder's IDE and desktop apps ignore the
marketplace file. Only the CLI reads it.

### Cursor {#cursor}

*Verified 2026-09-29 against the [Cursor plugin docs][cursor-plugins].*

Cursor has no documented way for an individual to add an arbitrary git
repository as a marketplace. You have three routes:

- Ask your administrator to import the repository as a Team Marketplace,
  described under [organization rollout](#organization).
- Run `cursor-agent plugin marketplace add <repository-url>` and finish
  in the `/plugin` menu. Cursor's staff confirmed this route is
  interactive, with no scripted install yet (2026-07-19, [Cursor
  forum][cursor-forum]).
- Copy one plugin folder from the repository's `cursor/` tree into
  `~/.cursor/plugins/local/<name>` and restart Cursor. An administrator
  can block this with the "Allow Local Plugin Imports" toggle.

### Private repositories {#private}

Claude Code clones with `git` on your machine and suppresses prompts.
A private marketplace therefore needs credentials that work without
typing: a credential helper for HTTPS, or an SSH key loaded in an
agent. Codex and Qoder document no private-repository authentication,
so assume the same git credentials but treat it as unverified. For GitHub, run `gh auth login` and then `gh auth setup-git`.
Claude Code accepts `CLAUDE_CODE_PLUGIN_PREFER_HTTPS=1` to force HTTPS.

A token in `GITHUB_TOKEN` alone does not authenticate a background
update unless a credential helper reads it. Copilot CLI's private-clone
behavior has regressed before ([github/copilot-cli#4103][copilot-4103]).
If it fails, clone the repository yourself and add the local directory
as the marketplace.

## Pin a repository for a team {#team-pin}

A repository can name the marketplace in a committed file, so every
contributor gets the plugin without typing the add command. Claude
Code and Copilot support this. Codex and Cursor do not.

### Claude Code {#team-pin-claude}

Run the add command with project scope and commit the file it writes:

```sh
claude plugin marketplace add grimoire-rs/e2e-marketplace#v1.2.0 --scope project
```

The hand-written form of `.claude/settings.json` is:

```json
{
  "extraKnownMarketplaces": {
    "grimoire-e2e": {
      "source": {
        "source": "github",
        "repo": "grimoire-rs/e2e-marketplace",
        "ref": "v1.2.0"
      }
    }
  },
  "enabledPlugins": { "grim-essentials@grimoire-e2e": true }
}
```

The marketplace entry applies only after the contributor accepts the
workspace trust dialog. In an untrusted folder Claude Code ignores it
without a message. Plugins in a grim marketplace use relative paths, so
they load from the marketplace copy and need no separate install step.

### Copilot and VS Code {#team-pin-copilot}

Commit the same two keys to `.github/copilot/settings.json`. VS Code
also reads `.claude/settings.json` for them. The Copilot cloud agent
reads the repository file too. That works for a public marketplace
repository. Internal and private ones are unreachable from the cloud
agent ([community#200387][copilot-cloud]).

### Codex and Cursor {#team-pin-none}

Codex registers marketplaces per user, and a repository cannot commit a
registration ([openai/codex#18115][codex-18115], open). Put the
`codex plugin marketplace add` line in your README or onboarding
script. Cursor has no repository-level setting at all.

### Claude Code on the web {#claude-web}

*Verified 2026-09-29 against Anthropic's [cloud environments
page][claude-cloud].*

A cloud session ignores the marketplaces and plugins a repository
declares in `.claude/settings.json`. It shows no trust dialog, and the
`/plugin` command is not available there. Two routes reach a cloud
session:

- **Server-managed settings.** Team and Enterprise owners can push
  `extraKnownMarketplaces` and `enabledPlugins` from the admin console,
  and cloud sessions fetch them before installing plugins.
- **Commit the content instead.** Files under `.claude/skills/`,
  `.claude/agents/`, `.claude/commands/`, `.claude/rules/` and
  `CLAUDE.md` do reach a cloud session, and so do skills enabled on
  claude.ai. Committed files are not marketplace consumption, but they
  are what [`grim install`](../commands.md#install) writes for a project.

Anthropic documents no supported way to run `claude plugin marketplace
add` from a setup script. Treat that idea as untested.

## Roll out to an organization {#organization}

An administrator can install the marketplace for everyone, so members do
nothing. The mechanism differs by vendor.

| Tool | Where the administrator sets it | Reaches cloud sessions |
|---|---|---|
| Claude Code | Managed settings: server-managed, device policy or `managed-settings.json` | Only server-managed |
| Claude.ai and Cowork | Organization settings, Plugins and skills, **Sync from GitHub** | Not documented |
| Copilot | `managed-settings.json` in the enterprise `.github-private` repository | Yes, including the cloud agent |
| Codex | ChatGPT workspace settings, Plugins, **Import marketplace** | Not documented |
| Cursor | Dashboard, Team Marketplaces, **Import from Repo** | Not documented |

*Verified 2026-09-29 against these sources.*
[Claude organization guide][claude-org], [organization sync
guide][claude-org-sync], [Copilot enterprise settings
reference][copilot-enterprise], [Codex plugin management
page][codex-admin] and [Cursor docs][cursor-plugins].

Points to know before you choose:

- **Claude Code managed settings** force-install with
  `extraKnownMarketplaces` and `enabledPlugins`. Set `enabledPlugins` to
  `true` to lock a plugin on. Restrict sources with
  `strictKnownMarketplaces`. Its entries match `ref` and `path`
  exactly, so an allowlist entry without `ref` does not cover a source
  pinned to `v1.2.0`.
- **Claude.ai and Cowork sync** needs a private or internal repository on
  github.com or gitlab.com. It reads only the default branch, ignores
  tags, and rejects any plugin with a top-level `bin/` folder. A public
  marketplace repository cannot use it, so members add that one
  themselves.
- **Copilot** applies the enterprise file at sign-in and propagates it
  within about an hour. Managed values cannot be overridden locally.
- **Codex** syncs an imported marketplace daily, and a manual sync
  forces one. OpenAI does not state whether the import also reaches the Codex CLI.
- **Cursor** offers Default Off, Default On and Required per marketplace,
  and an administrator can limit access to organization groups. Team
  Marketplaces need a Teams or Enterprise plan.

## Share with customers {#customers}

To give customers a marketplace, make the repository public, then send
each audience the one command for their tool from
[Install on one machine](#one-machine). Claude.ai and Claude Desktop
users have a click path instead: **Customize**, **Plugins**, **Add**,
**Add marketplace**, then the repository address. That surface loads
skills, commands and remote connectors. It ignores agents, hooks and
local MCP servers.

Tell customers which tag to pin. A pinned `#ref` keeps their install
stable while you keep working on the default branch. A customer who
follows the default branch trusts whatever that address serves at each
update, so protect who can push to it.

## Pin and update {#pin-and-update}

Pinning a `#ref` freezes the marketplace. Updates are opt-in in every
tool, except where an administrator or a setting turns them on.

| Tool | Pin on add | Refresh the marketplace | Update the plugin |
|---|---|---|---|
| Claude Code | `#ref` | `claude plugin marketplace update grimoire-e2e` | `claude plugin update grim-essentials@grimoire-e2e` |
| Copilot CLI | `#ref` | `copilot plugin marketplace update` | `copilot plugin update grim-essentials` |
| VS Code | `#ref` in the setting | Check for Extension Updates | Same, or every 24 hours with auto-update on |
| Codex | `--ref` | `codex plugin marketplace upgrade grimoire-e2e` | No separate command documented |
| Qoder | None documented | `qoder plugins marketplace update` | `qoder plugins update grim-essentials` |
| Cursor | None, tracks a branch | Administrator refresh | Automatic after a refresh |

Claude Code leaves auto-update off for a marketplace you added. Turn it
on under `/plugin`, **Marketplaces**, **Enable auto-update**, or set
`autoUpdate: true` on the `extraKnownMarketplaces` entry. A grim
plugin's version changes whenever its bytes change, so an update always
has something to fetch. Copilot opts in per entry with the same
`autoUpdate: true` key.

To move a pinned install forward in Claude Code, remove the marketplace,
add it again with the newer tag, then install the plugin again. Removing
a marketplace uninstalls its plugins there. Copilot CLI and Qoder offer
`marketplace remove` too. No tool documents re-adding a registered name
in place, so do not rely on it.

## Troubleshoot {#troubleshoot}

### Plugin is enabled in project settings but is not installed {#ts-not-installed}

Claude Code shows this error in the `/plugin` Errors tab when
`enabledPlugins` names a plugin whose marketplace was never registered
on this machine. Accept the workspace trust dialog, or run `claude
plugin marketplace add` for the repository, then restart the session.

### The marketplace did not refresh {#ts-not-refreshed}

Third-party marketplaces do not auto-update in Claude Code. Run the
refresh command from the [cheat sheet](#pin-and-update). A pinned `#ref`
never moves on its own, even after a refresh.

### An allowlist blocks the marketplace {#ts-allowlist}

Claude Code's `strictKnownMarketplaces` compares the whole source,
including `ref` and `path`. A `github` entry does not cover the same
repository written as a `git` URL. Copy the exact source you pin into
the allowlist.

### The plugin fails to install on Claude.ai {#ts-bin}

The web app and organization sync reject a plugin that has a top-level
`bin/` folder. Ask the curator to drop it, or install the plugin with
Claude Code instead.

### Codex lists a Claude plugin twice {#ts-codex-double}

Codex can mirror marketplaces from a Claude Code install on the same
machine ([openai/codex#19372][codex-19372], open). Set `enabled =
false` for the leaked plugin in `config.toml`. This is an upstream
behavior, not a grim one.

### A private clone fails {#ts-private}

See [Private repositories](#private). The fix is almost always a
credential that works without a prompt.

## Related guides {#related}

- [Host a plugin marketplace](./hosting-a-marketplace.md) is the curator
  side: how the repository you just added is generated and kept current.
- [Hand a team a plugin](./team-plugin.md) exports one plugin file to
  send around.
- [`grim export plugin`](../commands.md#export-plugin) documents the plugin
  shape each tool receives.

<!-- external -->
[claude-install]: https://code.claude.com/docs/en/plugins/install
[claude-reference]: https://code.claude.com/docs/en/plugins/marketplace-reference
[claude-cloud]: https://code.claude.com/docs/en/cloud-environments#what-carries-over-from-your-setup
[claude-org]: https://code.claude.com/docs/en/plugins/org
[claude-org-sync]: https://claude.com/docs/plugins/org-sync
[copilot-reference]: https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-plugin-reference
[copilot-enterprise]: https://docs.github.com/en/copilot/reference/enterprise-administrators/enterprise-managed-settings
[copilot-4103]: https://github.com/github/copilot-cli/issues/4103
[copilot-cloud]: https://github.com/orgs/community/discussions/200387
[vscode-plugins]: https://code.visualstudio.com/docs/agent-customization/agent-plugins
[codex-build]: https://developers.openai.com/codex/plugins/build
[codex-cli]: https://learn.chatgpt.com/docs/developer-commands?surface=cli
[codex-admin]: https://learn.chatgpt.com/docs/enterprise/plugin-management
[codex-18115]: https://github.com/openai/codex/issues/18115
[codex-19372]: https://github.com/openai/codex/issues/19372
[qoder-plugins]: https://docs.qoder.com/cli/plugins.md
[cursor-plugins]: https://cursor.com/docs/plugins
[cursor-forum]: https://forum.cursor.com/t/unable-to-find-a-cli-command-to-install-a-cursor-plugin-after-adding-its-marketplace-repository/166016
