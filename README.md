<div align="center">

<img src="./assets/logo.png" width="192" />

# grimoire

**A package manager for AI-agent config — skills, rules, agents, and MCP
servers, installed into every coding agent you use**

[![CI][ci-badge]][ci]
[![Release][release-badge]][releases]
[![Docs][docs-badge]][docs]
[![License][license-badge]][license]

</div>

Declare a skill once and `grim` writes it into Claude Code, Copilot, Cursor,
Codex, Gemini, Zed, Amp, Kiro, Junie, and opencode — each in the format that
client actually reads, pinned by digest in a lockfile. Storage is any OCI
registry — GHCR, Docker Hub, or your own. There is no Grimoire service to
sign up for.

Where a client has no honest surface for an artifact, grim says so and skips
it rather than writing config that looks installed and does nothing. See the
[client compatibility matrix][docs-clients].

> **Status:** stabilizing toward 1.0 — released surfaces are frozen
> contracts; pin a version when you depend on it.

## Install

One-line installer (macOS / Linux) — detects your platform, verifies the
SHA-256 checksum, and drops `grim` onto your `PATH`:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://setup.grimoire.rs/sh | sh
```

On Windows (PowerShell 7.4+):

```powershell
irm https://setup.grimoire.rs/ps1 | iex
```

In GitHub Actions, use [`grimoire-rs/setup-grimoire@v1`][setup-grimoire].
Pre-built binaries for macOS, Linux, and Windows (aarch64 / x86_64) are on
the [latest release][releases]; other methods (ocx, source build) are in the
[installation docs][docs-install]. Or build from source:

```sh
cargo install --git https://github.com/grimoire-rs/grimoire grimoire
```

## Quick Start

```sh
grim init                                        # create grimoire.toml
grim add ghcr.io/grimoire-rs/skills/grim-usage   # declare, lock, install
grim install                                     # re-materialize after a clone
grim tui                                         # browse the index
```

Full documentation: **[grimoire docs][docs]**.

## Run Your Own Index

`grim search` reads a **package index** — a directory of pointers into your
registries. The public one is the default, not the system: one command
scaffolds your own, and GitHub or GitLab Pages serves it.

```sh
npx @grimoire-rs/indexer init                    # scaffold the index repo
git push                                         # Pages builds and serves it
grim config registry add acme --index https://acme.github.io/index
```

You get a searchable catalog site, the JSON grim resolves against, and a
contribution gate that refuses a pull request writing a namespace its author
does not own. No index server, no database, no account — private repos work
the same way. Walkthrough: [hosting an index][docs-hosting].

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full guide.

**Prerequisites:** [Rust](https://rustup.rs), [task](https://taskfile.dev),
[uv](https://docs.astral.sh/uv/) (for the Python acceptance suite).

## Community

- [Code of Conduct](CODE_OF_CONDUCT.md)
- [Security Policy](SECURITY.md)

## License

Grimoire is licensed under the [Apache License, Version 2.0][license].

<!-- badges -->
[ci]: https://github.com/grimoire-rs/grimoire/actions/workflows/verify-basic.yml
[ci-badge]: https://github.com/grimoire-rs/grimoire/actions/workflows/verify-basic.yml/badge.svg
[releases]: https://github.com/grimoire-rs/grimoire/releases
[release-badge]: https://img.shields.io/github/v/release/grimoire-rs/grimoire
[docs]: https://grimoire.rs/
[docs-badge]: https://img.shields.io/badge/docs-grimoire-blue
[docs-install]: https://grimoire.rs/installation.html
[docs-clients]: https://grimoire.rs/clients.html
[docs-hosting]: https://grimoire.rs/hosting-an-index.html
[setup-grimoire]: https://github.com/grimoire-rs/setup-grimoire
[license]: LICENSE
[license-badge]: https://img.shields.io/badge/license-Apache--2.0-blue.svg
