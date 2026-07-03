# Introduction

Grimoire is an OCI-backed package manager for AI-agent configuration. Its
binary, `grim`, installs, updates, and publishes the **skills**, **rules**,
**agents**, and **MCP servers** that steer coding agents — plus **bundles**
that group them — distributing them through ordinary [OCI registries][oci]
the same way container images are shipped.

## The problem

Reusable agent configuration — skills, rules, prompt templates — is copied by
hand between repositories today. A useful rule written for one project is
pasted into the next, then drifts: no version, no provenance, no upgrade path.
There is no `npm install` for an agent skill.

## The solution

Grimoire treats a skill or rule as a versioned, content-addressed artifact and
stores it in a registry you already run. You declare what you want in
`grimoire.toml`, pin exact digests in `grimoire.lock`, and materialize the
files into your AI client of choice. Upgrading is `grim update`; sharing is
`grim release`.

Because the transport is plain OCI, you inherit a registry's authentication,
TLS, and replication for free — there is no bespoke server to operate.
[GitHub Container Registry][ghcr], [Docker Hub][hub], or a private
[Distribution][dist] instance all work unchanged.

> **Status:** Grimoire is young. The CLI documented here is real and tested,
> but the surface is still moving toward 1.0 — pin a version when you depend on
> it.

## Where to next

- [Installation][install] — get the `grim` binary.
- [Quick Start][quickstart] — install your first skill in five commands.
- [Concepts][concepts] — skills versus rules, scopes, locks, and clients.
- [Command Reference][commands] — every subcommand and flag.

<!-- external -->
[oci]: https://github.com/opencontainers/distribution-spec
[ghcr]: https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry
[hub]: https://hub.docker.com
[dist]: https://distribution.github.io/distribution/

<!-- internal -->
[install]: ./installation.md
[quickstart]: ./quickstart.md
[concepts]: ./concepts.md
[commands]: ./commands.md
