---
title: "Introduction"
description: "Grimoire is a package manager for AI-agent configuration, distributed through standard OCI registries."
---
<!-- doc_type: landing -->
<!-- doc_tier: first-steps -->

Grimoire is a package manager for AI-agent config: the skills, rules, agents,
and MCP servers you install into every coding agent you use.

## Install your first skill

With `grim` on your `PATH`, one command declares a skill, pins it, and
materializes it into every AI client it finds in the project:

```sh
grim add ghcr.io/grimoire-rs/skills/grim-usage
```

The files land in each client's own configuration directory, in the format
that client reads. [Installation](./installation.md) covers getting the
binary, and [Quick Start](./quickstart.md) walks the whole loop.

## Why it exists

A rule written for one project gets pasted into the next, then drifts. It
carries no version, no provenance, and no upgrade path. Grimoire treats a
skill or a rule as a versioned, content-addressed artifact. You declare it in
`grimoire.toml`, pin its digest in `grimoire.lock`, and roll it forward with
`grim update`.

Storage is any [OCI registry](https://github.com/opencontainers/distribution-spec):
[GHCR](https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry),
[Docker Hub](https://hub.docker.com), or your own. There is no Grimoire
service to sign up for. Because the transport is plain OCI, you inherit the
authentication, TLS, and replication of a registry you already run. A private
[Distribution](https://distribution.github.io/distribution/) instance works
unchanged.

> **Status:** the CLI documented here is real and tested, and the surface is
> stabilizing toward 1.0. Pin a version when you depend on it.

## Where to next

- [Getting started](./quickstart.md): declare a skill, install it, and upgrade it.
- [Guides](./guides/scopes-and-clients.md): project or global scope, and which clients get the files.
- [Teams and automation](./publishing.md): publish config your colleagues install.
- [Reference](./commands.md): every subcommand, flag, and config key.
