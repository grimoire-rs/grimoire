---
title: Share harness settings
description: Publish a team's default model, its allow and deny rules and a plugin marketplace once, and let grim write them into each harness's own settings file. Experimental pre-1.0.
---
<!-- doc_type: how-to -->
<!-- doc_tier: everyday -->

**Experimental pre-1.0**. See [Stability][stability-unstable].

The `settings` kind ships in grim 0.16.0 behind the
`[options.experimental] settings` flag. It may change in any minor release.

Without a shared package, every teammate picks the model, approves the same
commands and adds the team's plugin marketplace by hand. This guide publishes
those three as settings packages, installs them for you and a teammate, and
removes them again. Each value lands in the file the harness reads, beside the
values you set yourself.

## Before you start {#prerequisites}

- `grim` on your `PATH`, and a project where you run [Claude Code][claude]
  with a `grimoire.toml` from `grim init`.
- A registry you can publish to. The examples use `localhost:5000`, which
  [Docker][docker] starts with `docker run -d -p 5000:5000 registry:2`.
  Replace the name with your own registry.
- For the marketplace, [Copilot][copilot] and [Codex][codex] beside
  [Claude Code][claude]. Leave out the ones you do not run.

## Turn the kind on {#enable}

The kind is off by default. Turn it on for this project:

<!-- doc: guide-share-settings -->
```bash-run
grim config set options.experimental.settings true
```

The flag lands in `grimoire.toml`, so a teammate gets it with the next pull.
It is not consent. [Turn the kind on][ref-enable] lists what it gates.

## Set behaviour defaults {#defaults}

A settings package is a TOML document with one table per harness. Each key
under `[claude]` is a key of [Claude Code's settings file][claude-settings]. A
single value, such as the model, is written whole.

1. Write the document.

   <!-- doc: guide-share-settings -->
   ```bash-run
   mkdir -p settings
   cat > settings/team-defaults.toml <<'EOF'
   description = "Team defaults for Claude Code"

   [claude]
   model = "sonnet"
   EOF
   ```

2. Publish it.

   <!-- doc: guide-share-settings -->
   ```bash-run
   grim release --kind settings settings/team-defaults.toml localhost:5000/settings/team-defaults:1.0.0
   ```

   `grim release` pushes `1.0.0` and moves the `1.0`, `1` and `latest` tags
   with it.

3. Declare it.

   <!-- doc: guide-share-settings -->
   ```bash-run
   grim add --kind settings localhost:5000/settings/team-defaults:1
   ```

   Typing the reference is your consent for this checkout. grim writes the
   value and lists each change on stderr:

   <!-- doc: guide-share-settings -->
   ```text
   added claude .claude/settings.local.json /model (team-defaults)
   a running client may need a restart to read its new settings
   ```

`.claude/settings.local.json` is the per-developer file, and grim keeps it out
of `git status`. [The project local file][ref-local] explains the rule, and
[Where each value lands][ref-files] names the file for every harness.

## Share allow and deny rules {#permissions}

`permissions.allow` and `permissions.deny` are lists. grim adds its own rules
to a list and leaves every other rule in it alone.

1. Write and publish the document.

   <!-- doc: guide-share-settings -->
   ```bash-run
   cat > settings/team-permissions.toml <<'EOF'
   description = "Team allow and deny rules for Claude Code"

   [claude.permissions]
   allow = ["Bash(npm run test:*)", "Bash(npm run lint)"]
   deny = ["Bash(rm -rf:*)", "Read(./.env)"]
   EOF
   grim release --kind settings settings/team-permissions.toml localhost:5000/settings/team-permissions:1.0.0
   ```

2. Declare it.

   <!-- doc: guide-share-settings -->
   ```bash-run
   grim add --kind settings localhost:5000/settings/team-permissions:1
   ```

   Each rule is a change of its own:

   <!-- doc: guide-share-settings -->
   ```text
   added claude .claude/settings.local.json /permissions/allow Bash(npm run lint) (team-permissions)
   added claude .claude/settings.local.json /permissions/allow Bash(npm run test:*) (team-permissions)
   added claude .claude/settings.local.json /permissions/deny Bash(rm -rf:*) (team-permissions)
   added claude .claude/settings.local.json /permissions/deny Read(./.env) (team-permissions)
   a running client may need a restart to read its new settings
   ```

## Bring teammates in {#teammates}

Commit `grimoire.toml` and `grimoire.lock`. A teammate's clone then declares
both packages, pinned to the digests you installed.

Your consent stays on your machine, so the clone applies nothing yet. In a
terminal, the teammate's first `grim install` asks once and records the answer.
Without a terminal it writes nothing, and `grim status` reads `gated`.

`--trust-workspace` answers yes for one run and records nothing:

<!-- doc: guide-share-settings -->
```bash-run
grim install --trust-workspace
```

<!-- doc: guide-share-settings -->
```text
added claude .claude/settings.local.json /model (team-defaults)
added claude .claude/settings.local.json /permissions/allow Bash(npm run lint) (team-permissions)
added claude .claude/settings.local.json /permissions/allow Bash(npm run test:*) (team-permissions)
added claude .claude/settings.local.json /permissions/deny Bash(rm -rf:*) (team-permissions)
added claude .claude/settings.local.json /permissions/deny Read(./.env) (team-permissions)
a running client may need a restart to read its new settings
```

For a lasting answer, the teammate runs `grim consent allow` once. Running
`grim add` with the same reference consents too, while the tag the lock records
still names the pinned digest. That add asks the registry once. When the tag
moved, or the registry does not answer, it warns and records nothing. A later version bump asks
nothing again, so read the lock diff it brings. [Consent][ref-consent] covers
the record and both flags.

## Pre-configure a marketplace {#marketplace}

A settings package can register a plugin marketplace in each harness. It pairs
with a marketplace you generate with `grim export marketplace`, as
[Host a marketplace][hosting] shows. [Claude Code][claude] and
[Copilot][copilot] take the entry under `extraKnownMarketplaces`, and
[Codex][codex] under `marketplaces`.

Only [Claude Code][claude] takes settings in a project, so this package
installs at global scope. Global scope is per machine, so each teammate runs
steps 1, 2 and 4 on their own machine. Only you publish.

1. Turn the kind on for global scope too. The project flag does not cover
   `--global` runs.

   <!-- doc: guide-share-settings -->
   ```bash-run
   grim config set --global options.experimental.settings true
   ```

2. Name your harnesses. Without an `options.clients` list, grim writes only to
   the harnesses it detects. The list replaces any list set before, so name
   every harness you keep.

   <!-- doc: guide-share-settings -->
   ```bash-run
   grim config set --global options.clients claude,copilot,codex
   ```

3. Write and publish the document. Use your `[marketplace].name` as the entry
   name. It is the name after the `@` in `<plugin>@<marketplace>`.

   <!-- doc: guide-share-settings -->
   ```bash-run
   cat > settings/team-marketplace.toml <<'EOF'
   description = "The acme plugin marketplace"

   [claude.extraKnownMarketplaces.acme]
   source = { source = "github", repo = "acme/plugins" }

   [copilot.extraKnownMarketplaces.acme]
   source = { source = "github", repo = "acme/plugins" }

   [codex.marketplaces.acme]
   source = "https://github.com/acme/plugins.git"
   source_type = "git"
   EOF
   grim release --kind settings settings/team-marketplace.toml localhost:5000/settings/team-marketplace:1.0.0
   ```

4. Declare it at global scope, which needs no consent.

   <!-- doc: guide-share-settings -->
   ```bash-run
   grim add --global --kind settings localhost:5000/settings/team-marketplace:1
   ```

   <!-- doc: guide-share-settings -->
   ```text
   added claude ~/.claude/settings.json /extraKnownMarketplaces/acme (team-marketplace)
   added codex ~/.codex/config.toml /marketplaces/acme (team-marketplace)
   added copilot ~/.copilot/settings.json /extraKnownMarketplaces/acme (team-marketplace)
   a running client may need a restart to read its new settings
   ```

grim compares each marketplace entry as a whole. [Merge rules][ref-merge]
covers how each kind of value merges, and [Per-harness fields][ref-fields]
lists the source fields each harness takes.

## Refuse and force {#refusal}

grim never overwrites a value that changed after it wrote it, unless you pass
`--force`. Change the model by hand, and add an allow rule of your own:

<!-- doc: guide-share-settings -->
```bash-run
cat > .claude/settings.local.json <<'EOF'
{
  "model": "opus",
  "permissions": {
    "allow": ["Bash(make:*)", "Bash(npm run lint)", "Bash(npm run test:*)"],
    "deny": ["Bash(rm -rf:*)", "Read(./.env)"]
  }
}
EOF
```

The next install refuses the model and exits `65`:

<!-- doc: guide-share-settings -->
```bash-run
grim install
```

<!-- doc: guide-share-settings -->
```text
overwrite claude .claude/settings.local.json /model (team-defaults): modified
```

Its error line ends `rerun with --force to change them`. With `--format json`,
the [error document][json-error] lists each refused piece.

`--force` writes the team's value over yours. grim keeps no copy of the value
it replaces:

<!-- doc: guide-share-settings -->
```bash-run
grim install --force
```

<!-- doc: guide-share-settings -->
```text
overwritten claude .claude/settings.local.json /model (team-defaults)
a running client may need a restart to read its new settings
```

Your own allow rule was never at risk, because grim only changes the rules it
added. [Refusals and `--force`][ref-force] lists every refusal.

## Remove a package {#uninstall}

`grim uninstall` removes the declaration and every value the package wrote:

<!-- doc: guide-share-settings -->
```bash-run
grim uninstall settings team-defaults
```

<!-- doc: guide-share-settings -->
```bash-run
grim uninstall settings team-permissions
```

<!-- doc: guide-share-settings -->
```text
removed claude .claude/settings.local.json /permissions/allow Bash(npm run lint) (team-permissions)
removed claude .claude/settings.local.json /permissions/allow Bash(npm run test:*) (team-permissions)
removed claude .claude/settings.local.json /permissions/deny Bash(rm -rf:*) (team-permissions)
removed claude .claude/settings.local.json /permissions/deny Read(./.env) (team-permissions)
a running client may need a restart to read its new settings
```

The rule you added stays:

<!-- doc: guide-share-settings -->
```bash-run
cat .claude/settings.local.json
```

<!-- doc: guide-share-settings -->
```json
{
  "permissions": {
    "allow": ["Bash(make:*)"]
  }
}
```

`grim uninstall --global settings team-marketplace` removes the marketplace
entries the same way. [Uninstall][ref-uninstall] covers values grim adopted
rather than wrote.

## Each harness's own settings {#harness-docs}

grim delivers a value into the file a harness reads, and does not enforce it
([Delivered, not enforced][ref-delivered]). A key's meaning lives in the
harness's own reference:

- [Claude Code settings][claude-settings]
- [Codex configuration reference][codex-config]
- [Copilot CLI settings][copilot-settings]
- [Cursor CLI configuration][cursor-config]
- [OpenCode config][opencode-config]
- [Qoder CLI settings reference][qoder-settings]

## Next steps {#next-steps}

- [Settings reference][ref] covers the document format, every merge rule and
  the fields each harness takes.
- [Host a marketplace][hosting] generates the marketplace this guide
  registers.
- [Use a marketplace][use-marketplace] installs its plugins in each harness.
- [Scopes and clients][scopes] explains project and global scope and the
  `options.clients` list.

<!-- internal -->
[stability-unstable]: ../stability.md#unstable
[ref]: ../settings.md
[ref-enable]: ../settings.md#enable
[ref-consent]: ../settings.md#consent
[ref-local]: ../settings.md#local-file
[ref-files]: ../settings.md#files
[ref-merge]: ../settings.md#merge
[ref-fields]: ../settings.md#fields
[ref-force]: ../settings.md#force
[ref-uninstall]: ../settings.md#uninstall
[ref-delivered]: ../settings.md#delivered
[json-error]: ../json-interface.md#error-document
[hosting]: ./hosting-a-marketplace.md
[use-marketplace]: ./use-a-marketplace.md
[scopes]: ./scopes-and-clients.md

<!-- external -->
[claude]: https://code.claude.com
[codex]: https://developers.openai.com/codex
[copilot]: https://github.com/features/copilot
[docker]: https://docs.docker.com/get-started/get-docker/
[claude-settings]: https://code.claude.com/docs/en/settings
[codex-config]: https://learn.chatgpt.com/docs/config-file/config-reference
[copilot-settings]: https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/change-settings
[cursor-config]: https://cursor.com/docs/cli/reference/configuration
[opencode-config]: https://opencode.ai/docs/config/
[qoder-settings]: https://docs.qoder.com/cli/settings-reference
