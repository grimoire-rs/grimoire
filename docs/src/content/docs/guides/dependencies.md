---
title: Bundle or require
description: Choose between a bundle and a declared dependency, build one lifecycle package and one skill that needs an MCP server, then read grim why to see how each arrived.
---
<!-- doc_type: how-to -->
<!-- doc_tier: everyday -->

A team's skills rarely stand alone. The `build` skill leans on shared
templates. A [Google Drive][google-drive] skill is useless without its MCP
server. Ship each piece separately and every consumer has to find the rest, in
the right versions.

Grim has two ways to say "these belong together". A **bundle** is a named set
the consumer declares once. A **requirement** is a link one artifact carries
to another, so the dependency installs without being declared. This guide
builds one of each, installs them, and reads `grim why` to see which chain
brought in what.

## Before you start {#prerequisites}

- `grim` on your `PATH`, and a project with a `grimoire.toml` from `grim init`.
- A client in the project that takes agents and MCP servers, such as
  [Claude Code][claude-code]. Grim installs into the clients it detects, so a
  `.claude` directory is enough. With no client detected, grim installs the
  skills only and declines the agents and the server, as
  [Clients][clients-matrix] explains.
- A registry you can publish to. The examples use `localhost:5000`, which
  [Docker][docker] starts with `docker run -d -p 5000:5000 registry:2`. Any
  registry you can write to works the same. Replace the name.

## Pick bundle or requires {#pick}

| You want | Use | Why |
|---|---|---|
| Consumers to take a curated set with one declaration | A [bundle][bundles] | One line brings every member, and membership follows the published bundle. |
| An artifact that breaks without another | A [requirement][requires] | The dependency installs whenever the artifact does. |
| Both: a curated set whose members share a helper | A bundle of artifacts that require the helper | The set is a choice. The helper is not. |

The test is whether a piece is needed wherever its artifact goes. The review
agents are not: they belong to this set, so they are bundle members. The
templates that every stage skill reads are a requirement, because no stage
works without them, even one a consumer declares on its own.

The first case below is the third row. The second is the second.

## Ship a lifecycle package {#lifecycle}

The playbook runs a feature through six stages: plan, design, build, test,
deploy and maintain. Each stage is a skill. All six read the same templates,
so the templates are a skill the six require. Two review agents ride along as
bundle members, because no stage needs them to work.

1. Write the shared templates.

   <!-- doc: guide-dependencies -->
   ```bash-run
   mkdir -p src/templates
   cat > src/templates/SKILL.md <<'MD'
   ---
   name: templates
   description: Shared document templates for every lifecycle stage.
   ---
   # Templates

   Start each stage document from the template of the same name.
   MD
   ```

2. Write the six stage skills. Each one requires the templates with a
   `grim.requires` key, as a [relative reference][relative]. `./templates:1`
   means the `templates` repository beside this one, so the package survives a
   mirror.

   <!-- doc: guide-dependencies -->
   ```bash-run
   for stage in plan design build test deploy maintain; do
     mkdir -p "src/$stage"
     cat > "src/$stage/SKILL.md" <<MD
   ---
   name: $stage
   description: The $stage stage of the lifecycle.
   metadata:
     grim.requires.skills.templates: "./templates:1"
   ---
   # $stage

   Use the $stage template, then do the work.
   MD
   done
   ```

3. Write the two review agents.

   <!-- doc: guide-dependencies -->
   ```bash-run
   mkdir -p src/agents
   for reviewer in code-reviewer security-reviewer; do
     cat > "src/agents/$reviewer.md" <<MD
   ---
   name: $reviewer
   description: Reviews a change at the end of a stage.
   ---
   You review the change you are given and report findings by severity.
   MD
   done
   ```

4. List the members in a bundle source. The `[skills]` table names the six
   stages and not the templates, because the stages already bring them.

   <!-- doc: guide-dependencies -->
   ```bash-run
   cat > src/lifecycle.toml <<'TOML'
   [skills]
   plan = "./plan:1"
   design = "./design:1"
   build = "./build:1"
   test = "./test:1"
   deploy = "./deploy:1"
   maintain = "./maintain:1"

   [agents]
   code-reviewer = "./code-reviewer:1"
   security-reviewer = "./security-reviewer:1"
   TOML
   ```

5. Publish everything under one namespace. An agent needs `--kind agent`
   because its file looks like a rule. See [Publishing][publishing].

   <!-- doc: guide-dependencies -->
   ```bash-run
   for name in templates plan design build test deploy maintain; do
     grim release "./src/$name" "localhost:5000/acme/$name:1"
   done
   for reviewer in code-reviewer security-reviewer; do
     grim release --kind agent "./src/agents/$reviewer.md" "localhost:5000/acme/$reviewer:1"
   done
   grim release ./src/lifecycle.toml localhost:5000/acme/lifecycle:1
   ```

6. Install it the way a consumer would: one declaration.

   <!-- doc: guide-dependencies -->
   ```bash-run
   grim add --kind bundle localhost:5000/acme/lifecycle:1
   ```

   The consumer's `grimoire.toml` holds one line under `[bundles]`. The
   lock holds the bundle, its eight members and the templates.

## Read the chain with grim why {#why}

The consumer never declared `templates`, yet it is installed. `grim why`
reads the config and the lock and never contacts a registry, so it works
offline.

<!-- doc: guide-dependencies -->
```bash-run
grim why skill templates
```

```text
Path
bundle/lifecycle -> skill/build -> skill/templates
bundle/lifecycle -> skill/deploy -> skill/templates
bundle/lifecycle -> skill/design -> skill/templates
bundle/lifecycle -> skill/maintain -> skill/templates
bundle/lifecycle -> skill/plan -> skill/templates
bundle/lifecycle -> skill/test -> skill/templates
```

Each row is one chain. The first node is what the project declared, and the
last is what you asked about. A bundle is the root of every chain through it,
and the middle nodes are the artifacts that required the last one.

Six chains tell you the templates are load-bearing: they stay installed while
any one of the six stages does. An artifact reached only by a bundle has a
shorter chain:

<!-- doc: guide-dependencies -->
```bash-run
grim why agent security-reviewer
```

```text
Path
bundle/lifecycle -> agent/security-reviewer
```

That chain has no requirement in it, so nothing else needs the agent. It is
there because the bundle lists it, and it stays while you declare the bundle.
A project that wants the set without it declares the members it wants
directly, instead of the bundle.

## Require an MCP server {#mcp}

The second case has no bundle. A skill reads and edits
[Google Drive][google-drive] files through an MCP server, so it is useless
without one. The skill carries the dependency, and the consumer declares only
the skill.

1. Write the skill and the server descriptor. The skill names the server with
   `grim.requires.mcp.gdrive`.

   <!-- doc: guide-dependencies -->
   ```bash-run
   mkdir -p src/gdrive-sync src/mcp
   cat > src/gdrive-sync/SKILL.md <<'MD'
   ---
   name: gdrive-sync
   description: Read and edit Google Drive files through the gdrive server.
   metadata:
     grim.requires.mcp.gdrive: "./gdrive:1"
   ---
   # Drive sync

   Use the gdrive server's tools to list and edit files.
   MD
   cat > src/mcp/gdrive.toml <<'TOML'
   description = "Google Drive access over MCP."

   [server]
   transport = "stdio"
   command = "gdrive-mcp"
   TOML
   ```

2. Publish both. The descriptor needs `--kind mcp`, because a `.toml` file
   looks like a bundle.

   <!-- doc: guide-dependencies -->
   ```bash-run
   grim release ./src/gdrive-sync localhost:5000/acme/gdrive-sync:1
   grim release --kind mcp ./src/mcp/gdrive.toml localhost:5000/acme/gdrive:1
   ```

3. Declare only the skill.

   <!-- doc: guide-dependencies -->
   ```bash-run
   grim add --kind skill localhost:5000/acme/gdrive-sync:1
   ```

4. Ask why the server is there.

   <!-- doc: guide-dependencies -->
   ```bash-run
   grim why mcp gdrive
   ```

   ```text
   Path
   skill/gdrive-sync -> mcp/gdrive
   ```

   The server registered in your clients' MCP settings, and you wrote no line
   for it. `grim uninstall skill gdrive-sync` removes the server's entry with
   the skill, since nothing else asks for it. `grim remove` drops both from the
   lock only, and leaves the entry in your clients' config.

## When the chain is not what you expected {#overrides}

A direct declaration beats whatever an artifact asked for. If you declare
`templates` yourself at another version, `grim why skill templates` also prints
an `overrides:` line naming each parent and the reference it wanted, and your
declaration wins. If the same digest is both declared and required, grim
notes on stderr that your declaration is redundant. The
[`grim why` reference][cmd-why] has the exit codes, including `65` for a lock
whose graph cannot be followed. `grim lock` repairs that.

## Next steps {#next-steps}

- [Artifact formats][artifacts-requires] lists every place a requirement is
  declared and the checks `grim build` runs on it.
- [Publishing bundles][publishing-bundles] covers `--pin`, which freezes
  floating members at release time.
- [Concepts][concepts-dependencies] explains how bundles and dependencies share
  one conflict policy.

<!-- internal -->
[bundles]: ../concepts.md#bundles
[requires]: ../artifacts.md#requires
[relative]: ../artifacts.md#bundle-relative-refs
[publishing]: ../publishing.md
[publishing-bundles]: ../publishing.md#bundles
[artifacts-requires]: ../artifacts.md#requires
[concepts-dependencies]: ../concepts.md#dependencies
[cmd-why]: ../commands.md#why
[clients-matrix]: ../clients.md#matrix

<!-- external -->
[docker]: https://docs.docker.com/get-started/get-docker/
[google-drive]: https://www.google.com/drive/
[claude-code]: https://code.claude.com
