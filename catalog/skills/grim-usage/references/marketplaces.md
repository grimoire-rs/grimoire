# Plugin Marketplaces

You loaded this file because someone is adding, pinning, rolling out, or
troubleshooting a **grim-generated plugin marketplace**, or running the
`grim export marketplace` command that produces one. The consumer needs no
grim binary: the repository holds each coding tool's own marketplace file, and
the tool adds it with its own commands. Needs grim 0.15.0 or later on the
curator's side; consumers run no grim at all.

Creating and hosting one (layout, `[marketplace]` table, deploy repository,
scheduled regenerate, CI gate, tokens) is the curator's job: the
`grim-authoring` skill's marketplace reference and the
[hosting guide][hosting-a-marketplace]. This file is the consumer and
operator side.

Contents: [Running the export](#running-the-export) ·
[Add it in a client](#add-it-in-a-client) ·
[Private repositories](#private-repositories) · [Pin a team](#pin-a-team) ·
[Organization rollout](#organization-rollout) · [Update and pin](#update-and-pin) ·
[Troubleshooting](#troubleshooting)

## Running the export

`grim export marketplace` regenerates the whole repository from a
`marketplace.toml` that carries a `[marketplace]` table. It takes only
`--marketplace <path>` (default `./marketplace.toml`), `-o/--output` (the
repository root; default the manifest's directory) and `--force`. Anything
else the plugin export takes (refs, `--client`, `--plugin`, `--name`,
`--zip`, `--version`, `--description`, `--logo`, `--project`) is a usage error,
exit 64; the clients come from `[marketplace].clients`. Confirm with
`grim export marketplace --help`.

- **Lock location.** `marketplace.lock` is written beside the manifest, not
  into `-o`. A manifest without `[marketplace]` exits 65 and names the table.
- **No overlap.** Two runs cannot overlap: the manifest's lock plus a
  `.grim-export.lock` sidecar in the output root. Contention exits 75. Keep
  the sidecar and any `.grim-export-*` staging dir out of git. grim never
  sweeps a stale staging dir from an interrupted run; it warns and names it,
  so look for a `.replaced-<plugin>.<client>` entry inside before deleting.
- **Idempotent.** The same inputs on the same grim build rewrite nothing:
  every tree and file reports `unchanged`. A crash mid-write leaves a layout
  the next run owns and repairs.
- **Report.** `--format json` is `{items, files}`; `items[].action` is
  `written|unchanged|removed|empty`, `files[].action` is
  `written|unchanged|removed`, and the README row has `client: null`. `empty`
  is a plugin a client cannot carry, and `omitted` names every member. The
  report is emitted only on success; plain text is not a contract.
- **Safety refusals (65).** A symlink, non-directory or reparse point on any
  owned path; the manifest, its lock, a `project` dir, a `path:` include or a
  logo lying under a path the export owns (matched ignoring ASCII case);
  a logo, `path:` member or `project` dir outside the manifest directory or
  behind a symlink; a Windows reserved device name as a plugin name.
- **Ownership.** grim owns `./<client>/` and that client's marketplace file
  only. A foreign marketplace file, or a non-empty `./<client>/` that has no
  marketplace file of ours, is exit 65 `untracked-destination`; `--force`
  adopts it and deletes the strays. A renamed
  `[marketplace].name` is the usual cause, and consumers must re-add under
  the new name.
- **Progress.** `--progress` shows a bar here as it does for `install` and
  `export plugin`.

The root `README.md` lists each client's add and install commands from
`[marketplace].repository`, a plugin table, a private-repository note and a
consumer-guide link (Qoder gets `.git` appended on a non-GitHub host). Full
flags, layout and exit table: [command reference][export-marketplace].

## Add it in a client

A consumer needs the **marketplace name** (`[marketplace].name`) and a
**plugin name**, and installs as `<plugin>@<marketplace>`. Each tool reads only
its own marketplace file: Claude `.claude-plugin/`, Copilot `.github/plugin/`,
Codex `.agents/plugins/`, Qoder `.qoder-plugin/`, Cursor `.cursor-plugin/`.
A Cursor file exists only if the curator selected it. Command shapes, which
vendors change faster than grim does. They were verified 2026-09-29 against the
guide; if a vendor command fails, the tool's own `--help` and the guide's
per-client section (linked in the Tool column) win:

| Tool | Add, then install | Pin |
|---|---|---|
| [Claude Code][uam-claude] | `claude plugin marketplace add <owner/repo or clone URL>[#ref]`, then `claude plugin install <plugin>@<name>` (same as `/plugin ...`) | `#<tag>` |
| [Copilot CLI][uam-copilot] | `copilot plugin marketplace add <repo>[#ref]`, then `copilot plugin install <plugin>@<name>` | `#<tag>` |
| [VS Code][uam-copilot] | no add command: put the repo in the user setting `chat.plugins.marketplaces` | `#<tag>` (recent VS Code) |
| [Codex][uam-codex] | `codex plugin marketplace add <repo> --ref <tag>`, then `codex plugin add <plugin>@<name>`; start a new session | `--ref` |
| [Qoder][uam-qoder] (CLI only) | `qoder plugins marketplace add <repo>`, then `qoder plugins install <plugin>@<name>` | none documented |
| [Claude Desktop, claude.ai][uam-desktop] | Customize, Plugins, Add, Add marketplace, Add from a repository (skills, commands, remote connectors only) | none |
| [Cursor][uam-cursor] | no individual route: an administrator imports it as a Team Marketplace; `cursor-agent plugin marketplace add <url>` is interactive only; or copy one tree into `~/.cursor/plugins/local/<name>` | none, tracks a branch |

Qoder on a non-GitHub host needs the address to end in `.git`, otherwise it
reads an https address as a direct link to a JSON file; its IDE and desktop
apps ignore the marketplace file. A consumer following the default branch
trusts whatever it serves, so pin a tag. Plugins from an address you added
yourself are not vendor-reviewed and can carry skills, agents and MCP servers
that run locally: add only repositories you trust. Details per tool:
[Use a marketplace without grim][use-a-marketplace].

## Private repositories

No marketplace file carries a credential; each tool clones with the machine's
git access, so the fix is a credential that works without a prompt.

- **Claude Code:** documented. A credential helper for HTTPS, or an SSH key in
  an agent; `gh auth login` then `gh auth setup-git`. A bare `GITHUB_TOKEN`
  does not authenticate background updates.
- **Claude Desktop, claude.ai:** connect GitHub and give the Claude GitHub App
  access to the repository.
- **Copilot CLI:** undocumented; a credential-helper regression affected
  1.0.70 (see the guide). Clone it yourself if it fails. **VS Code** reuses
  its GitHub session.
- **Codex, Qoder:** undocumented; assume the machine's git credentials.
  **Cursor:** needs an administrator import.
- **Last resort:** clone the repository yourself, add the local directory, and
  `git pull` to update.

The marketplace must live on a git host; Artifactory VCS repositories only
proxy archives and cannot host one. To give customers a marketplace, make the
repository public, send each audience the one command for their tool, and say
which tag to pin. Anchor: [private repositories][private].

## Pin a team

A repository-committed pin works for Claude Code and Copilot only.

- **Claude Code:** `claude plugin marketplace add <repo>#<tag> --scope project`
  writes `.claude/settings.json` with `extraKnownMarketplaces.<name>.source`
  (`{source: github, repo, ref}`) and `enabledPlugins` (`"<plugin>@<name>": true`).
  It applies only after the contributor accepts the workspace-trust dialog; in
  an untrusted folder it is ignored silently. grim plugins use relative paths,
  so no separate install step is needed.
- **Copilot and VS Code:** the same two keys in `.github/copilot/settings.json`
  (VS Code also reads `.claude/settings.json`). The Copilot cloud agent can
  use it only for a **public** marketplace repository.
- **Codex** registers per user and a repository cannot commit a registration:
  put the `codex plugin marketplace add` line in the README or onboarding
  script. **Cursor** has no repo-level setting.
- **Claude Code on the web** ignores repo-declared marketplaces and plugins
  and has no `/plugin`. Use server-managed settings (Team or Enterprise admin
  console), or commit the content itself under `.claude/skills|agents|commands|rules`
  and `CLAUDE.md`, which is what `grim install` writes.

Anchors: [team pin][team-pin], [Claude Code on the web][claude-web].

## Organization rollout

Administrators install it for everyone. Mark anything the guide's table calls
"not documented" as such; do not state cloud-session behavior beyond it.

- **Claude Code** managed settings: `extraKnownMarketplaces` plus
  `enabledPlugins` (`true` locks a plugin on). `strictKnownMarketplaces`
  matches `ref` and `path` **exactly**: an entry without `ref` does not cover a
  source pinned to a tag, and a `github` entry does not cover the same repo
  written as a `git` URL.
- **Claude.ai and Cowork** "Sync from GitHub" needs a private or internal repo
  on github.com or gitlab.com, reads only the default branch, ignores tags,
  and rejects any plugin with a top-level `bin/` folder. A public marketplace
  cannot use it.
- **Copilot:** `managed-settings.json` in the enterprise `.github-private`
  repository, applied at sign-in, about an hour to propagate, not locally
  overridable, and it reaches the cloud agent.
- **ChatGPT/Codex:** workspace Admin, Plugins, Add, Import marketplace, synced
  daily plus a manual sync; whether it reaches the Codex CLI is unstated.
  **Cursor:** Dashboard, Team Marketplaces, Import from Repo, with Default
  Off/On/Required and group limits, on Teams or Enterprise plans.

Anchor: [organization rollout][organization].

## Update and pin

Updates are opt-in in every tool, and a pinned `#ref` never moves by itself,
even after a refresh. A grim plugin's version changes whenever its rendered
bytes change, so an update always has something to fetch.

| Tool | Refresh the marketplace | Update the plugin |
|---|---|---|
| Claude Code | `claude plugin marketplace update <name>` | `claude plugin update <plugin>@<name>` |
| Copilot CLI | `copilot plugin marketplace update` | `copilot plugin update <plugin>` |
| Codex | `codex plugin marketplace upgrade <name>` | none documented |
| Qoder | `qoder plugins marketplace update` | `qoder plugins update <plugin>` |
| Cursor | administrator refresh | automatic afterwards |

Claude Code leaves auto-update **off** for a marketplace you added: turn it on
under `/plugin`, Marketplaces, or with `autoUpdate: true` on the settings entry
(Copilot opts in per entry with the same key). To move a pinned Claude Code
install forward, remove the marketplace (this uninstalls its plugins), add it
again with the newer tag, and reinstall. No tool documents re-adding a
registered name in place. Anchor: [pin and update][pin-and-update].

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| Claude "Plugin is enabled in project settings but is not installed" | Accept the workspace-trust dialog or run the marketplace add, then restart ([anchor][ts-not-installed]) |
| Marketplace did not refresh | Third-party marketplaces do not auto-update in Claude Code; run the refresh command |
| Allowlist blocks it | `strictKnownMarketplaces` compares the whole source including `ref` and `path` ([anchor][ts-allowlist]) |
| Fails on Claude.ai or via org sync | A top-level `bin/` folder is rejected: ask the curator to drop it, or use Claude Code ([anchor][ts-bin]) |
| Codex lists a Claude plugin twice | Codex can mirror Claude Code marketplaces on the same machine (upstream behavior); set `enabled = false` for the leaked plugin in `config.toml` ([anchor][ts-codex-double]) |
| Private clone fails | Needs a credential that works without a prompt ([anchor][ts-private]) |

Export-side failures (exit 65 `untracked-destination`, exit 75, CI symptoms)
are in [troubleshooting.md](troubleshooting.md#marketplace-and-plugin-export).

[hosting-a-marketplace]: https://grimoire.rs/guides/hosting-a-marketplace.html
[use-a-marketplace]: https://grimoire.rs/guides/use-a-marketplace.html
[export-marketplace]: https://grimoire.rs/commands.html#export-marketplace
[uam-claude]: https://grimoire.rs/guides/use-a-marketplace.html#claude-code
[uam-desktop]: https://grimoire.rs/guides/use-a-marketplace.html#claude-desktop
[uam-copilot]: https://grimoire.rs/guides/use-a-marketplace.html#copilot
[uam-codex]: https://grimoire.rs/guides/use-a-marketplace.html#codex
[uam-qoder]: https://grimoire.rs/guides/use-a-marketplace.html#qoder
[uam-cursor]: https://grimoire.rs/guides/use-a-marketplace.html#cursor
[private]: https://grimoire.rs/guides/use-a-marketplace.html#private-repositories
[team-pin]: https://grimoire.rs/guides/use-a-marketplace.html#team-pin
[claude-web]: https://grimoire.rs/guides/use-a-marketplace.html#claude-web
[organization]: https://grimoire.rs/guides/use-a-marketplace.html#organization
[pin-and-update]: https://grimoire.rs/guides/use-a-marketplace.html#pin-and-update
[ts-not-installed]: https://grimoire.rs/guides/use-a-marketplace.html#ts-not-installed
[ts-allowlist]: https://grimoire.rs/guides/use-a-marketplace.html#ts-allowlist
[ts-bin]: https://grimoire.rs/guides/use-a-marketplace.html#ts-bin
[ts-codex-double]: https://grimoire.rs/guides/use-a-marketplace.html#ts-codex-double
[ts-private]: https://grimoire.rs/guides/use-a-marketplace.html#ts-private
