---
title: "Concepts"
description: "Grimoire borrows its mental model from package managers you already use, then adapts it for AI-agent configuration."
---
<!-- doc_type: explanation -->

Grimoire borrows its mental model from package managers you already use, then
swaps the transport for an [OCI registry][oci]. This page covers the handful of
ideas that make the [commands](./commands.md) feel obvious.

## Skills, rules, and agents {#skills-rules-and-agents}

Grimoire distributes three kinds of installable artifact. A **skill** is a
directory — a `SKILL.md` plus any supporting scripts or references — that
teaches an agent a capability. A **rule** is a Markdown file that states a
standard or constraint the agent should always follow. An **agent** is a
Markdown file that defines a delegatable assistant — a system prompt with a
name, description, and tool access; see [Agent Artifacts](./agents.md).

All three are declared the same way and travel through the same pipeline; the
differences are shape on disk (a folder versus a file) and the `kind` argument
(`skill`, `rule`, or `agent`) you pass to commands like
[`grim add`](./commands.md#add).

A fourth kind, the **MCP server**, follows the same declare-and-lock
pipeline but installs differently: instead of a file, it describes a
[Model Context Protocol](./mcp-servers.md) server, and `grim install`
registers it as an entry in each client's own MCP config file rather than
writing anything of its own. See [MCP Server Artifacts](./mcp-servers.md)
for the full format and per-client behavior.

A sixth kind, the **hook**, is experimental pre-1.0. It is a directory with
a `hook.toml` index describing a handler that runs before or after an
agent's tool calls. It is the only kind that does not arm on install
alone. It also needs a feature flag and a per-workspace consent gesture.
See [Use hooks](./guides/agent-hooks.md) for a worked example, the [hooks reference](./hooks.md) for the gates, and
[Stability](./stability.md#unstable) for the experimental carve-out.

A seventh kind, **settings**, is experimental pre-1.0 too. It ships in grim
0.16.0 behind the `[options.experimental] settings` flag. A settings package
is one TOML document of typed values for a harness's own configuration, such
as a permission rule or a default model.

On install, grim merges each value into the file the harness reads. A value grim
did not write is never replaced without `--force`. Like a hook, it applies at
project scope only after the workspace consents. See the
[settings reference][settings] and [Stability][stability-unstable].

### Rules with a support directory {#rule-support-dir}

A rule is often an *index* that points at extra context — a worked example, a
JSON schema, a script — that would clutter the rule itself. The convention
across AI-config tooling is to put that next to the rule in a sibling folder
with the same name (`rules/my-rule.md` referencing `./my-rule/examples.md`).

A bare index whose links resolved to nothing would be useless, so a rule may
carry an optional **support directory**: a folder beside the index sharing its
stem (`my-rule.md` + `my-rule/`). Grimoire packs the index and every file under
that folder into the one artifact, and installs them beside each other so the
index's relative links resolve:

```
.claude/rules/my-rule.md
.claude/rules/my-rule/examples.md
.claude/rules/my-rule/schema.json
```

A rule with no support directory is unchanged — it remains the single
`my-rule.md` file. See [Publishing](./publishing.md#rule-support-dir) for how
the directory is packed.

One client needs a little help here. [Claude Code][claude-memory] reads
`.claude/rules/` recursively and treats every unscoped file it finds as
always-on context, which would load the whole support tree into every session
instead of the one the index sends it to. So when grim installs a support-dir
rule for Claude it also registers the directory in `claudeMdExcludes` — the
files stay on disk and stay readable, only the automatic load goes away. See
[Claude support-directory exclusion](./vendor-metadata.md#claude-md-excludes).

## Artifacts as OCI content {#artifacts-as-oci-content}

Under the hood every skill, rule, or agent is packed into an OCI artifact and
addressed by content digest, exactly like a container image layer. Identical
content is stored once and is immutable: a `sha256:…` digest always names the
same bytes.

Each artifact declares its kind through a `com.grimoire.kind` manifest
annotation (`skill`, `rule`, `agent`, `mcp`, `bundle`, or the experimental
`hook` and `settings`), so `grim` (or any OCI-aware tool that reads manifest
annotations) can tell a Grimoire artifact apart without unpacking it. The
manifest's config descriptor is the OCI empty config, which keeps the wire
format acceptable to every registry, including GitLab (see
[Registry compatibility](./configuration.md#registry-compatibility)).

This is why Grimoire needs no server of its own. Any registry that speaks the
[distribution spec][oci] — [GHCR][ghcr], [Docker Hub][hub], a private
[Distribution][dist] — is a complete backend.

## References, tags, and digests {#references-tags-and-digests}

You name an artifact with a reference: `registry/repository:tag`, or
`registry/repository@sha256:…` for an exact digest. A **floating tag** such as
`:1` points at the newest `1.x` release and moves over time; a **digest** never
moves.

You declare floating tags for convenience and let Grimoire pin them to digests
for reproducibility — which is the job of the lock.

A third form skips the registry entirely: a **local path** — `./skills/x`,
`../shared/rule.md`, or an absolute path. Round-tripping every edit of a
skill you are actively writing through a registry push is slow, and a draft
that has never been published has no reference to point at in the first
place. A local path source names a directory or file on disk directly, and
Grimoire pins it by the SHA-256 of its canonical packed layer — the same
tar [`grim build`](./commands.md#build) would produce — instead of a
manifest digest. [`grim add`](./commands.md#add-path) declares one
permanently; [`grim install <path>`](./commands.md#install-dev) renders one
once, without declaring it.

## The lock {#the-lock}

`grimoire.lock` records the exact digest each declared tag resolved to, so an
install is byte-for-byte reproducible until you deliberately upgrade.
[`grim lock`](./commands.md#lock) pins what is new or changed in
`grimoire.toml`, and keeps every other pin even after its tag moved. Only
[`grim update`](./commands.md#update) moves those: it re-resolves the tags and
rolls the pins forward when a newer version appears behind the same tag.

Each pin records the reference that resolved it, so a re-lock can tell a
request that still holds from one you changed. The lock also stores a hash of
the declaration it was generated from, so Grimoire can tell when
`grimoire.toml` has drifted ahead of the lock. A
[`grim remove`](./commands.md#remove) that cannot settle a pin offline leaves
the lock stale until the next relock.

## Bundles {#bundles}

Declaring the same dozen skills and rules in every repository does not scale.
Teams end up copying a block of `grimoire.toml` between projects, and when the
approved set changes someone has to chase down every copy.

A **bundle** is a curated set of members — skills, rules, agents,
[MCP servers](./mcp-servers.md), hooks, [settings][settings] packages, and
other bundles. You
declare it once in `[bundles]`, and on [`grim lock`](./commands.md#lock) it
**expands** into its members, which are pinned into the lock exactly like a
direct declaration. A bundle that lists another bundle expands that one too, so
bundles nest. A bundle's members are its dependencies; the same mechanism also
lets a single artifact [require others](#dependencies).

A bundle comes from one of two sources — the same split as an individual
[reference](#references-tags-and-digests):

- **Registry bundle** — the common case. `[bundles]` names a registry
  reference (`ghcr.io/acme/python-stack:1`); the bundle is
  [published as its own OCI artifact](./publishing.md#bundles) and pinned
  in the lock by its manifest digest, exactly like a skill or rule. Publish
  a new version, and each project that declares it picks that version up on
  its next [`grim update`](./commands.md#update).
- **Local bundle** — `[bundles]` names a [local path](#references-tags-and-digests)
  (`./bundles/x.toml`) instead of a registry reference. There is no publish
  step: the bundle is pinned by the SHA-256 of its canonical members layer,
  and every member it names must be a registry reference — a local bundle
  has no registry identity of its own to resolve a relative member
  against. See [`[bundles]`](./configuration.md#grimoire-toml) for the
  declaration shape.

Each locked member records its **provenance** — `direct` for something you
declared yourself, or the bundle it came from. [`grim status`](./commands.md#status)
surfaces it, and [`grim why`](./commands.md#why) lists the whole chain, so you
always know why an artifact is installed.

### Adding and dropping members {#bundle-membership}

The pinned version's member list is authoritative, so membership tracks the
bundle version your lock holds. A bundle keeps its pin until
[`grim update`](./commands.md#update) moves it, bare or named after the bundle.
When the new version **adds** a member, that update expands it into the lock and
installs it.

When a version **drops** a member, that member leaves the lock and the update
prunes its materialized files. Files you have edited locally stay until a bare
`grim update --force`. This is the same reconciliation any artifact leaving the
lock receives. Bundles just make it routine.

### Conflict policy {#bundle-conflicts}

Because an artifact is keyed by `(kind, name)`, several sources can name the same
slot: a direct declaration, bundles, and the [dependencies](#dependencies) of
other artifacts. Grimoire settles it deterministically:

- a **direct** `[skills]`/`[rules]`/`[agents]`/`[mcp]`/`[hooks]` declaration
  always wins over every bundle and dependency. This is how you override a single
  member without forking the bundle. If the declaration comes from a different
  registry or repository than the request it overrides, `grim lock` warns, in case
  you meant to switch publishers. A path declaration wins silently, with no warning;
- claims that **agree** merge into one entry. Two claims agree when their
  references resolve to the same manifest digest, so `:1` and `:1.2` agree while
  they point at one digest. Every source is recorded;
- claims that **disagree** fail closed: `grim lock` stops with exit `78`, lists
  every conflicting source, and asks you to declare the artifact directly to
  choose one.

Failing closed is deliberate. Silently picking a winner would let an unrelated
bundle bump change what a project installs without anyone noticing.

Agreement is judged when you lock, not when the sources were published. Two tags
that point at one digest today can part ways when one of them moves. A later
`grim update` can then turn a lock that resolved cleanly into a conflict, while
`grim lock` keeps the pin and exits `0`. The fix is the one the error names:
declare the artifact directly.

When a direct declaration's pin holds back a version a dependency asks for,
the pin still wins and the run exits `0`. `grim add`, `grim update` and
`grim lock` then warn once per held artifact, naming the requester, the
requested reference and the held pin. The warning offers two ways out:
`grim update -- <name>` re-reads the declared tag, and declaring the requested
reference chooses it. [`grim status --check`](./commands.md#status) lists the
cases that still stand.

A conflicting artifact is never expanded, so its own dependencies do not add
further conflicts. Cycles are a separate failure, covered under
[dependencies](#dependency-cycles).

### Floating versus pinned members {#bundle-pinning}

A bundle's members, and the dependencies an artifact declares, can themselves be
floating tags or exact digests. A floating reference resolves when it first
enters a consumer's lock. It moves only on that consumer's
[`grim update`](./commands.md#update), so reproducibility comes from the
consumer's own lock.

Publishing with [`grim release --pin`](./publishing.md#bundles) instead freezes
every floating reference to a digest at publish time, so the bundle or artifact
is reproducible on its own — the stronger guarantee for air-gapped or tunneled
networks that cannot re-resolve a tag. Pinning also freezes any
[deployment-relative reference](./artifacts.md#bundle-relative-refs) to its
absolute form. See [Publishing](./publishing.md#bundles) for a bundle and
[Publishing dependencies](./publishing.md#requires) for any other kind.

## Dependencies {#dependencies}

A skill, rule, agent, MCP server, hook, or settings package can declare other
artifacts it needs.
Its publisher writes the declaration, so you never copy it into your own
`grimoire.toml`. Adding the artifact is enough:

```sh
grim add ghcr.io/acme/skills/gws-drive:1
```

If `gws-drive` requires the `gws-shared` skill and the `gdrive` MCP server,
[`grim lock`](./commands.md#lock) pins all three, and `grim install` installs all
three. Dependencies can have dependencies, to any depth. The declaration itself
is covered under [Artifacts](./artifacts.md#requires). Everything below is about
what the consumer sees.

Dependencies are a second use of the machinery behind [bundles](#bundles). A
bundle's member list is the same document an artifact publishes for its
dependencies, and both expand through one resolver. That is why a bundle can list
another bundle.

**What lands in the lock.** Each artifact a declaration reaches gets a lock entry
of its own, and the parent records the dependencies it declared. You only ever
declare the artifacts you chose. The rest are *dependencies*, and `grim status`
shows them with `direct: false` and the parents that require them.
[`grim why`](./commands.md#why) answers the question "why is this installed?"
with every chain from your `grimoire.toml` to the artifact.

**Removing.** [`grim remove`](./commands.md#remove) drops a declaration and the
dependencies nothing else reaches, from the lock. [`grim uninstall`](./commands.md#uninstall)
also deletes their files. A dependency another artifact still requires stays, at
its digest and under its tag, because `remove` resolves nothing. A dependency you
cannot undeclare, because you never declared it, is removed by removing the
artifact that requires it.

**Floating by default.** A dependency written `./gws-shared:1` is a floating tag.
`grim lock` keeps its pin and `grim update` re-resolves it, like any other. A
publisher who wants the closure fixed releases with `--pin`, see
[Floating versus pinned members](#bundle-pinning).

A relative reference resolves against the repository the parent was pulled from, so a mirror of the parent
serves its dependencies from the mirror too. An absolute reference may name
another registry. Each registry is contacted with its own credentials, and a
credential is never sent to a different host.

**Same slot, several claims.** When two artifacts require the same dependency, or
you declare one that another artifact also requires, the
[conflict policy](#bundle-conflicts) decides. Your direct declaration wins.

### Cycles {#dependency-cycles}

A dependency cycle, such as `a` requiring `b` requiring `a`, is refused with exit
`65`. The message names the cycle and the repository that published the edge
closing it. Grimoire refuses a cycle even where the closure would be finite,
because a cycle leaves no order to install or remove in.

### Limits {#dependency-limits}

Resolution stops at a graph it will not accept, with exit `65`. The message names
the artifact whose dependencies crossed the line and its publisher. These are the
limits for one lock:

| Limit | Value |
|-------|-------|
| Artifacts reached through dependencies, or through a bundle that was itself reached through one | 2,048 |
| Levels below your declarations | 32 |
| Dependency documents read in total | 32 MiB |
| Dependencies in one document | 512 members, 512 KiB |

The members of a bundle you declare directly do not count toward the first or
third limit.

### Offline and the store {#dependency-offline}

[`grim lock --offline`](./commands.md#lock) reads each pinned artifact's
dependencies from the local store when the manifest is there. When it is not,
grim keeps the dependencies the previous lock recorded for that exact digest,
prints a warning that they are carried unverified, and moves on.
[`grim install`](./commands.md#install) checks them against the pinned manifest
before it writes anything, so a stale carry cannot install the wrong closure.
A lock grim 0.15.1 wrote records none, so there the artifact counts as
requiring nothing, as 0.15.1 reads it. What only that artifact requires leaves
the lock, and the next online `grim lock` resolves it afresh, so its pin moves if
its tag moved.

The store can also hold a manifest but not the dependency list it names, as
0.15.1's `grim add --no-install` and `grim fetch` leave it. The artifact then
requires what the lock records for it, which is nothing on a lock 0.15.1 wrote.
So the same entries leave the lock, as on 0.15.1.
[`grim remove`](./commands.md#remove) reads such an artifact that way too, even
where the store holds the list.

One online `grim lock` before the edit records
the list, and the edit then keeps those pins. The first `grim install` on a lock
0.15.1 wrote over such an artifact already exits `65` and names `grim lock`.

A marketplace plugin's `marketplace.lock` drops such entries the same way. An
include deleted there leaves the plugin. If something still requires it,
`grim update --marketplace` resolves it afresh, so its pin moves if its tag
moved.

A declared bundle works the same way. Offline, its row in the lock stands in for
a manifest the store lacks, so a clean lock relocks with no network. A pin an
offline run does move comes from the local tag cache, which can lag behind the
lock. A warning names each one, and each row a tag read added or dropped. An
online `grim update` resolves them fresh.

## Scopes {#scopes}

Grimoire works in two scopes. The **project** scope is the `grimoire.toml`
discovered from the current directory — per-repository configuration that lives
beside your code. The **global** scope is a single config under `$GRIM_HOME`
for artifacts you want everywhere.

Most commands operate on the discovered project by default and switch to the
global scope with `--global`. The [TUI](./commands.md#tui) can flip between the
two at runtime.

## Clients {#clients}

An installed artifact has to land somewhere the agent reads. Grimoire calls
that destination a **client target**. The full set is listed in the
[client compatibility matrix](./clients.md#matrix) and includes
[Claude Code][claude], [opencode][opencode], [GitHub Copilot][copilot],
[Codex][codex], Cursor, Kiro, JetBrains Junie, Gemini CLI, Zed, Amp,
Antigravity, Cline, Droid, Goose, Warp, OpenClaw, Kilo and Qoder. The same skill is transformed into
each client's native layout on install. Not every client can host every
artifact kind — the [client compatibility matrix](./clients.md) is the
authoritative map of what installs where, and why a decline happens.

For example, [Codex][codex] covers skills, agents, and MCP servers but
declines rules: Codex uses an always-on, directory-granular instruction file
(`AGENTS.md`) with no path-glob or `applyTo` scoping mechanism, so there is no
native target for a rule artifact. Installing a rule with `--client codex`
produces a warning and writes no file. Other clients decline other kinds for
their own reasons, each recorded in the matrix.

[`grim install`](./commands.md#install) writes to the targets listed in the
`clients` option in your config. Without one it targets every client it
detects (vendor directory or marker present), plus the clients in your global
`options.clients` list when the project names none.

When nothing is detected and no global list is set, it targets the generic
**`agents`** client. That is not a product but the vendor-neutral pool at
`.agents/skills` that several clients already read. It is never *detected*, only selected: writing the pool directory changes no
future run's answer, which is exactly why grim no longer falls back to every
known client. `--client` overrides both and accepts a comma-separated list to
install into several AI clients at once.

## The catalog {#the-catalog}

[`grim search`](./commands.md#search) and the [TUI](./commands.md#tui) read a
**catalog** — an index of the artifacts your browse sources offer, cached
locally under `$GRIM_HOME` so repeat browsing is fast and works offline. The
catalog can span several sources at once: every configured `[[registries]]`
entry contributes, and out of the box it is fed by the public
[package index](./package-index.md). Pass `--refresh` to rebuild it.

## Online by default, offline on demand {#online-by-default-offline-on-demand}

By default Grimoire is **online**: every floating-tag lookup resolves fresh
against the registry, and the resolved digest is cached as a write-through so
later offline runs still work. A lookup therefore never answers from a stale
cache. There is no "use the cache first" mode to surprise you, and no `--remote`
flag to remember. A lookup is not a refresh: a pin stays in the lock until
[`grim update`](./commands.md#update) moves it.

Pass `--offline` to flip to **cache-only**: Grimoire forbids all network access
and fails rather than touch a registry — useful in sealed CI or an air-gapped
network. Warm the cache with a normal online `grim install` before
going offline. `grim lock` resolves references and writes the lockfile without
downloading content, so an `--offline` run after only a lock exits `81`.
`--offline` has an environment-variable equivalent described in
[Configuration](./configuration.md).

<!-- internal -->
[settings]: ./settings.md
[stability-unstable]: ./stability.md#unstable

<!-- external -->
[oci]: https://github.com/opencontainers/distribution-spec
[ghcr]: https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry
[hub]: https://hub.docker.com
[dist]: https://distribution.github.io/distribution/
[claude]: https://docs.anthropic.com/en/docs/claude-code/overview
[claude-memory]: https://code.claude.com/docs/en/memory
[opencode]: https://opencode.ai
[copilot]: https://github.com/features/copilot
[codex]: https://developers.openai.com/codex
