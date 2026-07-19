---
name: grim-usage
description: Drive the grim CLI — the OCI package manager for AI skills, rules, agents, and bundles. Use when installing, updating, searching, or publishing AI-config artifacts with grim; when composing grim init, config, add, lock, install, update, status, context, fetch, describe, search, tui, mcp, build, release, publish, login, or logout commands; when configuring settings, multiple registries, or qualified alias/repo references; or when resolving registries, project vs global scope, client targets, or offline mode.
license: Apache-2.0
compatibility: grim>=0.9
metadata:
  summary: How to use the grim CLI end to end
  keywords: grim,grimoire,cli,oci,registry,install,update,publish,skills,rules,agents,bundles,mcp,multi-registry
  repository: https://github.com/grimoire-rs/grimoire
---

# Grim Usage

Grimoire (binary: `grim`) is a package manager for AI-agent configuration.
It distributes five artifact kinds — **skills**, **rules**, **agents**,
**MCP servers**, and **bundles** — through any standard OCI registry (GHCR,
Docker Hub, a private Distribution), with lockfile-pinned installs into AI
clients such as Claude Code, OpenCode, GitHub Copilot, and Codex. An MCP
server artifact installs by registering an entry in each client's native MCP
config file (never as a file of its own); uninstall removes only that
entry, never the file. (Codex supports skills, agents, and MCP servers;
rules are not supported and grim warns and skips them when `--client codex`
is specified.)

## Verify Before Acting

Before composing any non-trivial grim command:

1. Run `grim --version`. This guide is written against grim 0.10.x; on a
   different minor, treat every flag mentioned here as a hypothesis.
2. Run `grim <command> --help` before using flags you have not verified
   this session — it is the authoritative, always-current flag list.
3. On any conflict between this skill and live `--help` output, **trust
   `--help`**. It ships with the binary; this guide can lag.

These pages teach workflows and semantics, never exhaustive flags. The
full reference is `--help` plus the docs site linked below.

## Command Map

| Command | Purpose | Details |
|---|---|---|
| `grim init` | Create a fresh `grimoire.toml` | [consume](references/consume.md) |
| `grim config` | Read/write `grimoire.toml` settings and registries | [registries](references/registries.md) |
| `grim add` | Declare an artifact and pin it in the lock | [consume](references/consume.md) |
| `grim lock` | Resolve floating tags to digests | [consume](references/consume.md) |
| `grim install` | Materialize the lock into AI clients | [consume](references/consume.md) |
| `grim update` | Re-resolve, re-materialize, prune | [consume](references/consume.md) |
| `grim status` | Report each declared artifact's state | [consume](references/consume.md) |
| `grim context` | Report the resolved scope, paths, clients, registries | [consume](references/consume.md) |
| `grim fetch` | Print an artifact's content without installing | [consume](references/consume.md) |
| `grim describe` | Report an artifact's metadata (kind, annotations, tags) without downloading content | [consume](references/consume.md) |
| `grim remove` / `uninstall` | Undeclare vs full inverse of install | [consume](references/consume.md) |
| `grim search` / `tui` | Browse your declared registries' catalogs | [registries](references/registries.md) |
| `grim mcp` | Run a local STDIO MCP server for AI agent integration | [registries](references/registries.md) |
| `grim build` | Validate and pack locally, no push | [publish](references/publish.md) |
| `grim release` | Validate, pack, push with cascade tags | [publish](references/publish.md) |
| `grim publish` | Batch-release packages from a `publish.toml` manifest | [publish](references/publish.md) |
| `grim login` / `logout` | Manage registry credentials | [publish](references/publish.md) |
| `grim schema` | Emit the JSON Schema for `grimoire.toml` / `publish.toml` / `grimoire.lock` / the MCP descriptor | [publish](references/publish.md) |

> **Deprecation:** a publisher can retire a package without
> unpublishing it; `add` and `status` flag it as deprecated (an `add` of a
> deprecated reference still succeeds). `search` and `tui` **hide**
> deprecated artifacts by default unless they are installed — reveal them
> with `grim search --show-deprecated`, the TUI `h` key, or by setting
> `options.show_deprecated = true` (`grim config set options.show_deprecated
> true`). A `replaced-by` successor reference, when the publisher named
> one, surfaces in `grim search` / `grim describe`. See [Publishing][publishing].
>
> **Git provenance:** `build`, `release`, and `publish` can embed
> the publishing commit, date, and origin as OCI annotations via opt-in
> `--git`; confirm with `grim release --help`.

## Reference Syntax

An artifact is named `registry/repository:tag` (a floating tag — `:1`
follows the newest `1.x` release) or `registry/repository@sha256:…` (an
immutable digest). A bare reference defaults to `:latest`.

A third form skips the registry: a **local path** — `./skills/x`,
`../shared/rule.md`, or an absolute path — names a directory or file on
disk directly. The discriminant is used everywhere a reference is accepted
(`grim add`, `grim install`, a `[skills]`/`[rules]`/`[agents]`/`[bundles]`
value): a value starting with `./` or `../`, or an absolute path, is a
local path source; anything else is an OCI reference. See
[references/consume.md](references/consume.md#declaring) for how it is
declared and installed.

A short reference with no registry resolves against the default registry —
`--registry` flag, then `GRIM_DEFAULT_REGISTRY`, then config, then the
built-in fallback registry `ghcr.io/grimoire-rs`; full
precedence in [references/registries.md](references/registries.md). Browsing
with nothing configured (`grim search`, `grim tui`, `grim mcp`) falls back
to the public package index at `https://index.grimoire.rs` instead — see
[references/registries.md](references/registries.md#multiple-registries).

When a config declares `[[registries]]` with aliases, a **qualified
reference** `alias/repo[:tag]` expands the alias to its configured URL —
for example `acme/code-review:1` becomes `ghcr.io/acme/code-review:1`
when `acme` is aliased to `ghcr.io/acme`. Full details and the
multi-registry browse behavior in
[references/registries.md](references/registries.md).

## Routing Table

| Read... | ...when |
|---|---|
| [references/consume.md](references/consume.md) | Installing, updating, or removing artifacts in a project |
| [references/publish.md](references/publish.md) | Building, releasing, tagging, or logging in to publish |
| [references/registries.md](references/registries.md) | Resolving registries, scopes, client targets, offline mode, or searching |
| [references/troubleshooting.md](references/troubleshooting.md) | A grim command failed — exit codes, integrity gates, common causes |
| [references/updating.md](references/updating.md) | Maintaining this skill itself against newer grim releases |

## Further Reading

- [Command reference][commands] — every command with current flags.
- [Concepts][concepts] — kinds, references, the lock, bundles, scopes,
  clients.
- [Configuration][config] — `grimoire.toml`, `grimoire.lock`, environment
  variables.
- [Publishing][publishing] — the author-to-release workflow.
- [Authentication][auth] — credential store, login/logout, CI recipes.

[commands]: https://grimoire.rs/commands.html
[concepts]: https://grimoire.rs/concepts.html
[config]: https://grimoire.rs/configuration.html
[publishing]: https://grimoire.rs/publishing.html
[auth]: https://grimoire.rs/authentication.html

---

Verified against grim 0.10.0.
