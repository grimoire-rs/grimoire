# Quick Start

This walkthrough declares a skill, installs it into a project, and then
upgrades it. It assumes `grim` is on your `PATH` (see [Installation][install]).
Nothing else needs configuring: out of the box `grim` browses the public
[package index][index] and expands short references against
`ghcr.io/grimoire-rs` — point it at your own registry only when you have one.

## 1. Create a project config

`grim init` writes a fresh `grimoire.toml` in the current directory:

```sh
grim init
```

Pulling from your own registry instead of the defaults? Seed it as the
default `[[registries]]` entry so short references resolve against it:

```sh
grim init --registry ghcr.io/acme
```

## 2. Declare an artifact

`grim add` records a skill or rule in `grimoire.toml` and immediately pins it
in `grimoire.lock`. The only required argument is the reference to fetch; the
kind is inferred from the published manifest and the binding name defaults to
the reference's last path segment:

```sh
grim add ghcr.io/grimoire-rs/skills/grim-usage
```

The reference is `registry/repo:tag` (or `registry/repo@sha256:…` to pin an
exact digest). Without a tag, `:latest` is assumed; a floating tag like `:1`
tracks the newest `1.x` release, which is what makes
[`grim update`](#5-upgrade) meaningful later. To find something worth
declaring, search the index first — `grim search` matches names, summaries,
and keywords, and [`grim tui`][tui] browses the same catalog interactively:

```sh
grim search authoring
```

## 3. Install into your AI client(s)

`grim install` materializes every locked artifact into your AI client's
configuration directory. By default it targets every AI client it detects in
the workspace ([Claude Code][claude], [opencode][opencode], [GitHub
Copilot][copilot], [OpenAI Codex][codex]); pass `--client` to pick explicitly,
with a comma-separated list to install into several at once. Note that
[Codex][codex] supports skills and agents only — rules are not supported and
are skipped with a warning:

```sh
grim install
grim install --client claude,copilot
```

## 4. Check the state

`grim status` reports each declared artifact as installed, outdated, locally
modified, or missing — the same model the [TUI][tui] paints in colour.

```sh
grim status
```

## 5. Upgrade {#5-upgrade}

When the publisher ships a newer version behind the same floating tag,
`grim update` re-resolves the tag, rolls the lock forward, and re-materializes
only what changed:

```sh
grim update            # everything
grim update grim-usage # one binding by name
```

## Go global {#global}

Everything above also works user-wide. Pass `--global` and the declaration
lands in the global config (`$GRIM_HOME/grimoire.toml`, created on demand —
no `init` needed) while `install` writes into each client's user-level
directory (e.g. `~/.claude/skills/`), so the artifact follows you across
projects:

```sh
grim add --global ghcr.io/grimoire-rs/skills/grim-usage
grim install --global
```

## Undo

To take an artifact back out completely — files, install record, and config
entry — use [`grim uninstall`][uninstall]. To browse what the index offers
before declaring anything, launch the interactive browser with
[`grim tui`][tui].

<!-- external -->
[claude]: https://docs.anthropic.com/en/docs/claude-code/overview
[opencode]: https://opencode.ai
[copilot]: https://github.com/features/copilot
[codex]: https://openai.com/index/openai-codex/

<!-- internal -->
[index]: ./package-index.md
[install]: ./installation.md
[tui]: ./commands.md#tui
[uninstall]: ./commands.md#uninstall
