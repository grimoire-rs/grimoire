---
title: "Artifact Reference"
description: "Grimoire ships seven artifact kinds: skills, rules, agents, MCP servers, bundles, and the experimental hooks and settings. Each has its own file layout and install target."
---
<!-- doc_type: reference -->

Grimoire ships seven artifact kinds: skills, rules, agents, MCP servers,
bundles, and the experimental [hooks](./hooks.md) and [settings][settings]. Each
has its own source shape, frontmatter schema, and validation rules. Before this
page, those details lived scattered across the publishing, agents, and
vendor-metadata chapters.

When you author an artifact you need one page that answers: which fields
exist, which are required, what values are valid, and what a correct file
looks like. This page is that reference. Narrative background stays in
[Concepts](./concepts.md); publishing mechanics stay in
[Publishing](./publishing.md); vendor projection semantics stay in
[Vendor-Specific Metadata](./vendor-metadata.md); the full MCP server
reference lives in its own chapter,
[MCP Server Artifacts](./mcp-servers.md).

## The seven kinds {#kinds}

Every artifact carries its kind in a `com.grimoire.kind` manifest
annotation, so registries and tooling can distinguish kinds without
downloading layers.

| Kind | Source shape | `com.grimoire.kind` | Installs as |
|------|--------------|---------------------|-------------|
| **Skill** | Directory with a `SKILL.md` index | `skill` | Directory tree under the client's `skills/` dir |
| **Rule** | Single `.md` file (+ optional sibling support directory) | `rule` | `rules/<name>.md` (+ `rules/<name>/…`), per-client transform |
| **Agent** | Single `.md` file | `agent` | One agent file per client, per-client rendering |
| **MCP server** | `mcp/<name>.toml` | `mcp` | Entry registered in each client's own MCP config file — never a materialized file |
| **Bundle** | `.toml` member list | `bundle` | Never materializes itself — expands to its members |
| **Hook** (experimental) | Directory with a `hook.toml` index | `hook` | Payload on disk plus a client-native registration, gated behind a feature flag and workspace consent — see [Hooks](./hooks.md) |
| **Settings** (experimental) | `settings/<name>.toml` document | `settings` | Values merged into each harness's own settings file, gated behind a feature flag and workspace consent. See [Settings][settings] |

The manifest's config descriptor is the OCI empty config
(`application/vnd.oci.empty.v1+json`) — universally allow-listed, including
on GitLab Container Registry, which rejects custom config and `artifactType`
media types (see [Registry compatibility](./configuration.md#registry-compatibility)).
Earlier releases instead stamped a custom OCI `artifactType`
(`application/vnd.grimoire.<kind>.v1`) and a per-kind config media type;
grim still reads those when present, so artifacts published before this
change resolve their kind unchanged.

`grim build` and `grim release` infer the kind from the path — a directory
is a skill, a `.md` file is a rule, a `.toml` file is a bundle. Two kinds
are the exception, because their shape collides with another kind's:
an agent `.md` is indistinguishable from a rule, so `--kind agent` is
required (see [Agent Artifacts](./agents.md#publishing)); an MCP
descriptor `.toml` is indistinguishable from a bundle, so `--kind mcp`
is required (see [MCP Server Artifacts](./mcp-servers.md#publishing)).
A settings document is a `.toml` file too, so `--kind settings` is
required (see [Document format][settings-format]).

A third directory shape, `hook.toml`, needs no flag. Inference checks for
`SKILL.md` before `hook.toml`. So a skill that happens to ship a
`hook.toml` alongside it stays a skill. A bare hook directory with no
`SKILL.md` is unambiguous on its own.

### Hooks (experimental) {#hooks}

A **hook** is a directory with a `hook.toml` index describing one or more
handlers. See [Control agent commands with hooks](./guides/agent-hooks.md) for a worked example:

```toml
schema = 1
name = "shell-guard"
description = "Observes Bash tool calls before they run."

[[hooks]]
id = "guard"
event = "PreToolUse"
tier = "observer"
matcher = "Bash"
command = "sh guard.sh"
timeout = 5
```

`event` names the point in an agent's tool-call lifecycle the hook
attaches to. It is vendor-neutral, and grim translates it into each
client's own event vocabulary.

`tier` is one of three. `observer` cannot change the call. `gatekeeper`
can allow, ask, or deny it, where the client's own format admits a
verdict at that event. `mutator` can rewrite it, where the client's own
format supports that. `matcher` scopes which tool names the hook applies
to.

The handler is exactly one of two forms. `command` is a string handed to
`/bin/sh -c`, the common case shown above. `argv` is an exec-form
argument vector, with no shell involved.

`timeout` is the handler's limit in seconds, 30 when unset, and at least 1.
`grim build` refuses a smaller value with exit code 65. [Exit code and
timeout][hooks-exit] says how the limits combine across handlers.

Unlike every other kind, a hook never arms on its own. See [Hooks](./hooks.md#why-gated)
for the feature-flag and consent gates every installed hook still has to
clear before anything runs.

### Settings (experimental) {#settings}

**Experimental pre-1.0**, behind the `[options.experimental] settings` flag
since grim 0.16.0. See [Stability][stability-unstable].

A **settings** package is one TOML document. Its top level holds descriptive
keys, and each harness gets a table named after its grim client name:

<!-- doc-norun: a source document, built by the settings reference page -->
```toml
description = "Team defaults for Claude Code"

[claude]
model = "sonnet"
```
<!-- /doc-norun -->

grim merges each value into the file the harness reads, and reads that file
first. A value grim did not write is never replaced without `--force`.
Like a hook, a settings package applies only past the feature flag and,
at project scope, workspace consent.

The [Settings reference][settings] covers the format, the merge rules, the
refusal error and the fields each harness takes.

## Names {#names}

Every skill and agent carries a `name` in frontmatter, and grim validates
it at build time. The same character rules apply to rule names taken from
the file stem. Bundle and MCP names also come from the file stem but are
not charset-validated at build; bundle *member* names are validated
against the same rules at resolve time.

A valid name matches `[a-z0-9]+([.-][a-z0-9]+)*`:

- contains only lowercase letters, digits, hyphens, and periods,
- does not start or end with a hyphen or period,
- does not contain two adjacent separators (`a--b`, `a..b`, and `a.-b`
  are all invalid),
- is 1–64 characters.

Periods are a deliberate superset of the
[Agent Skills standard][agentskills-spec], which allows only `[a-z0-9-]`.
A dotted name such as `socket.io` publishes and installs fine with grim,
but tooling that enforces the strict standard may reject it — prefer
hyphens when portability across vendors matters.

For skills the `name` must equal the directory name containing `SKILL.md`;
for agents it must equal the file stem (`reviewer.md` → `name: reviewer`).
A mismatch fails the build with exit code 65 (data error).

## Skills {#skills}

A skill is a directory: the `SKILL.md` index plus any supporting files
(scripts, templates, references). Everything in the tree is packed into a
single [tar layer](#layers) and installed verbatim, except files excluded by
[`.grimignore`](#grimignore) — only `SKILL.md` itself is ever re-rendered,
and only when it carries vendor-namespaced metadata keys.

The frontmatter follows the [agentskills specification][agentskills-spec].
Parsing is forward-compatible: unknown top-level keys are preserved
round-trip rather than rejected.

| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `name` | yes | string | Must equal the skill directory name; see [Names](#names) |
| `description` | yes | string | What the skill does and when to use it |
| `license` | no | string | SPDX-style identifier (e.g. `Apache-2.0`); emitted as the OCI license annotation |
| `compatibility` | no | string | Environment requirements as free text; the spec caps it at 500 characters — grim warns (never fails) on empty or oversized values |
| `allowed-tools` | no | string | Space-separated list of pre-approved tools; experimental in the spec, and support varies by client |
| `metadata` | no | string→string map | Catalog keys + vendor extensions, see below |
| *(any other key)* | no | any YAML | Preserved verbatim (forward compatibility) |

Inside `metadata`, all values are strings. Three plain keys are read by
grim itself; everything else either passes through untouched or is a
[vendor extension](#vendor-extensions):

| Metadata key | Read by | Meaning |
|--------------|---------|---------|
| `summary` | catalog | Short one-line blurb for `grim search` / the TUI |
| `keywords` | catalog | Comma-separated tags, matched by search |
| `author` | nothing (convention) | Attribution; passes through verbatim |
| `<vendor>.<field>` | install renderer | Lifted into native client frontmatter, see [Vendor extensions](#vendor-extensions) |

### Example — minimal skill {#skill-example-minimal}

The smallest valid skill is a directory with a two-field `SKILL.md`:

```yaml
# hello-world/SKILL.md
---
name: hello-world
description: A minimal smoke-test skill that prints a greeting.
---

# Hello World

Say hello.
```

### Example — full-featured skill {#skill-example-full}

A skill using every top-level field, catalog metadata, and a Claude-only
capability key:

```yaml
# code-reviewer/SKILL.md
---
name: code-reviewer
description: Review a diff for SOLID/DRY violations, missing tests, and
  risky changes. Use when asked to review a pull request or audit a patch.
license: Apache-2.0
compatibility: claude>=2
allowed-tools: Read Grep Bash
metadata:
  summary: Multi-pass diff reviewer
  keywords: review,quality,solid,dry,audit
  author: acme-platform-team
  claude.user-invocable: "true"
  claude.effort: high
---

# Code Reviewer

Run the review in three passes...
```

The `claude.*` keys are string-valued here and become typed native
frontmatter (`user-invocable: true`, `effort: high`) in the file Claude
Code receives; other clients never see them. The projection rules live in
[Vendor-Specific Metadata](./vendor-metadata.md#projection-semantics).

### Well-known assets — README, logo {#well-known-assets}

Because a skill packs its **whole** directory tree verbatim, two conventional
files placed beside `SKILL.md` ride along at no extra cost:

- `README.md` — a human-facing readme for the artifact.
- `logo.png` / `logo.svg` — an icon for a catalog or gallery UI.

They are ordinary tree files, so they need no frontmatter and no special
handling: they appear in the [`grim fetch`](./commands.md#fetch) `files[]`
listing and can be pulled individually with `--path` (a binary `logo.png`
comes back base64-encoded, decoding to the exact bytes in plain mode). This
is a **convention, not a schema** — grim reads no meaning from these names;
they are simply the agreed spot a tool looks for a readme or an icon.

A rule follows the same convention inside its [sibling support
directory](#rules) (`architecture-guide/README.md`,
`architecture-guide/logo.png`), which packs into the same layer tree.

An [agent](#agents) — a single `.md` file — carries the same well-known
files from a sibling directory sharing its stem (`agents/<name>/README.md`,
`agents/<name>/logo.png`, `agents/<name>/logo.svg`). Unlike a rule's support
directory, an agent packs **only** those allowlisted files, never an arbitrary
tree: the agent's identity stays the standalone `<name>.md`, and the companions
ride the layer purely so a catalog UI can show them. They land under
`<name>/…` in the layer, so the retrieval path is identical for every tree-backed
kind — `grim fetch <ref> --path <name>/README.md`. The companions are not
installed to a client (an agent installs as its lone `.md`); they exist for
`grim fetch`/catalog consumers.

[MCP servers](#mcp-servers) and [bundles](#bundles) have no file tree of their
own — their layer is a single JSON document — so they carry no *in-tree* README.
For a README that works uniformly across **every** kind (including mcp and
bundle), publish a [repository description companion](./publishing.md#description-companion):
declare a `[description]` table in `publish.toml` (or let grim probe the
conventional `README.md` / `CHANGELOG.md` / `logo.*` files) and it rides
[`grim publish`](./commands.md#publish); read it back with
[`grim fetch <repo> --description`](./commands.md#fetch-description).

### Excluding files with `.grimignore` {#grimignore}

A skill directory, or a rule's [support directory](#rules), accumulates
files that have nothing to do with the artifact: a `__pycache__/` left
behind by running a bundled script, a stray `.DS_Store`, a `.venv/` built
for local testing. None of that belongs in a published layer, and none of
it should make [`grim status`](./commands.md#artifact-states) report the
artifact `modified` just because a consumer ran a script.

Grim applies a built-in default ignore list — [gitignore][gitignore-spec]
syntax — to every skill directory and rule support directory, both when
packing (`grim build`/`release`/`publish`) and when hashing an installed
tree (`grim status`/`update`):

```
__pycache__/
*.py[co]
.venv/
venv/
.mypy_cache/
.pytest_cache/
.ruff_cache/
*.egg-info/
node_modules/
.git/
.svn/
.hg/
.DS_Store
Thumbs.db
desktop.ini
*.swp
*.swo
*~
.idea/
.vscode/
```

`target/` and `.gitignore` are deliberately absent: both are plausible
artifact content (a Rust example, a shipped `.gitignore` template) rather
than junk, so a publisher who wants either ignored adds it explicitly.

Drop a `.grimignore` at the directory root — a skill's own root, or a
support directory's root for a multi-file rule — to extend the defaults
with full gitignore syntax, including `!pattern` to re-include a default
(`!node_modules/` ships it after all). Lines *add to* the built-in list; a
`.grimignore` never replaces it, so an empty or trivial file cannot
accidentally widen what gets published — the opposite of npm's
`.npmignore`, whose mere presence discards the `.gitignore`-derived
defaults. Only the walk root's `.grimignore` counts — one nested inside a
subdirectory is an ordinary file, packed like any other.

`SKILL.md`, a rule's index file, a hook's `hook.toml`, and `.grimignore`
itself are never ignored, however a pattern is written, so a stray `*.md` or `.*` line
cannot strip the artifact's own identity. `.grimignore` is itself packed,
installed, and hashed, so a publisher's ignore rules travel with the
artifact — editing them after install is itself drift. An invalid line
fails `grim build` at exit 65, naming the file and line; the same line
found on an already-installed tree is skipped with a warning rather than
failing the read. A `.grimignore` over 64 KiB is refused the same way:
`grim build` exits 65, and an installed one is set aside with a warning so
only the defaults apply. Matching is case-sensitive on every OS, so the packed
bytes hash identically regardless of platform.

Ignored files are never packed and never count toward drift — `grim
status`/`update` treat them as if they did not exist — and an ignored
directory is pruned rather than walked, so a large `node_modules/` cannot
trip the packaging size limits. `grim update` still replaces the whole
installed directory on a pin change, so anything living only under an
ignored path — a hand-built `.venv/`, a `__pycache__/` — is disposable and
is wiped along with everything else; `.grimignore` hides local junk from
drift detection, it does not protect it. This holds for *every* path the
artifact's `.grimignore` ignores, not just the defaults: a hand edit there
is never reported as drift and `grim update` wipes it without the
`--force` refusal, so keep no hand-edited files under an ignored path.

A [hook](./hooks.md) handler runs with its installed payload directory as
the working directory. So anything it writes there, such as a
`__pycache__/` or a state file, lands inside the artifact. The defaults
already hide Python and Node caches. Add any other runtime byproduct to
`.grimignore`, or `grim status` reports the hook as modified and the next
install declines to arm it. The ignore rules cut the other way too. A
handler that names an ignored file, such as `.venv/lib/dep.py`, fails
`grim build` at exit 65, because the file would never be packed. An
ignored `node_modules/`, `.venv/` or `venv/` that no handler names only
draws a build warning.

[Agents](#agents) get no `.grimignore`: their companion directory is
already an allowlist of exactly `README.md`, `logo.png`, and `logo.svg`,
so there is nothing else to exclude.

## Rules {#rules}

A rule is a single Markdown file. Frontmatter is entirely optional — a
bare `.md` with no `---` fence is a valid rule whose body is the whole
document. When grim needs a description for the catalog it derives one
from the first Markdown heading or first non-empty line.

| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `paths` | no | list of strings | Glob patterns the rule auto-loads on; empty/absent = always active |
| `summary` | no | string | Short one-line blurb for the catalog |
| `keywords` | no | string or list | Comma-separated tags (a YAML list is comma-joined) |
| `license` | no | string | SPDX-style identifier (e.g. `Apache-2.0`); emitted as the OCI license annotation |
| `metadata` | no | string→string map | Vendor extensions (e.g. `copilot.exclude-agent`) |
| *(any other key)* | no | any YAML | Preserved verbatim (forward compatibility) |

Note the asymmetry with skills: rule `summary`/`keywords` are **top-level**
frontmatter keys, not `metadata` entries.

A rule may also carry a sibling support directory sharing its stem
(`architecture-guide.md` + `architecture-guide/`); both pack into one
artifact and install side by side — see
[Rules with a support directory](./publishing.md#rule-support-dir).

### Example — minimal rule {#rule-example-minimal}

```markdown
# commit-style.md

Use Conventional Commits. Subject ≤ 50 characters.
```

No fence at all — valid. The catalog description becomes the first
heading-less line.

### Example — path-scoped rule with catalog metadata {#rule-example-scoped}

```yaml
# rust-style.md
---
paths:
  - "**/*.rs"
  - "**/Cargo.toml"
summary: Idiomatic Rust style rules
keywords: rust,style,lints,quality
---

# Rust Style

Prefer `&str` over `String` parameters...
```

### Example — rule with a vendor extension {#rule-example-vendor}

```yaml
# security-baseline.md
---
paths:
  - "**/*.rs"
summary: Security review baseline
metadata:
  copilot.exclude-agent: code-review
---

# Security Baseline

Validate all external input at system boundaries...
```

`copilot.exclude-agent` becomes `excludeAgent: code-review` in the
Copilot instructions file and is invisible to Claude and OpenCode — see
[Rule-level vendor keys](./vendor-metadata.md#rule-keys).

## Agents {#agents}

An agent is a single `.md` defining a delegatable assistant. Unlike rules,
agent frontmatter is **required**: every client needs at least a
`description` to decide when to route work to the agent.

| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `name` | yes | string | Must equal the file stem; see [Names](#names) |
| `description` | yes | string | When a client should delegate to this agent |
| `model` | no | string | Passed through verbatim, no alias translation; override per vendor via `<vendor>.model` |
| `tools` | no | string | Comma-separated allowlist, projected per client (string vs. list) |
| `metadata` | no | string→string map | Catalog keys (`summary`, `keywords`, `license`) + vendor extensions |
| *(any other key)* | no | any YAML | Preserved verbatim (forward compatibility) |

Like skills, agent `summary`/`keywords` live **inside** `metadata`. When a
vendor key lifts to the same native field as a common field (`model`,
`tools`), the vendor key silently wins for that client — the documented
override escape hatch
([override precedence](./agents.md#override-precedence)).

An agent may ship a `README.md` and a `logo.png`/`logo.svg` from a sibling
directory sharing its stem (`agents/<name>/`) — the
[well-known assets](#well-known-assets) convention. Only those allowlisted
files ride the layer; every other file in that directory is ignored, so the
installed agent stays the single `<name>.md`.

### Example — minimal agent {#agent-example-minimal}

```yaml
# reviewer.md
---
name: reviewer
description: Reviews a diff for correctness, style, and missing tests.
---

You are a code reviewer. Examine the diff...
```

### Example — agent with common fields and vendor overrides {#agent-example-vendor}

```yaml
# release-bot.md
---
name: release-bot
description: Prepares release notes and version bumps on request.
model: sonnet
tools: Read,Grep,Bash
metadata:
  summary: Release preparation agent
  keywords: release,changelog,versioning
  claude.permission-mode: plan
  claude.max-turns: "20"
  opencode.model: anthropic/claude-sonnet-4-5
  opencode.temperature: "0.2"
  copilot.tools: read,grep
---

You prepare releases. Collect commits since the last tag...
```

Claude Code receives `model: sonnet` plus `permissionMode: plan` and
`maxTurns: 20`; OpenCode receives `model: anthropic/claude-sonnet-4-5`
(its vendor key overrides the common `model`) and `temperature: 0.2`;
Copilot receives a `tools:` list of `read, grep`. The full emit matrix is
in [Agent Artifacts](./agents.md#emit-matrix).

## MCP Servers {#mcp-servers}

An MCP server describes one [Model Context Protocol][mcp-spec] server —
how to launch it or how to reach it — not a file to install. Its source
is a single `mcp/<name>.toml`; the descriptor name is the file stem,
like a rule or agent. There is no forward-compatible `extra` bucket
here: any field outside the tables below is a hard parse error.

| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `description` | yes | string | Must be non-empty; becomes the OCI description annotation |
| `summary` | no | string | Catalog blurb (`com.grimoire.summary`) |
| `keywords` | no | string | Comma-separated tags (`com.grimoire.keywords`) |
| `license` | no | string | SPDX-style identifier (`org.opencontainers.image.licenses`) |
| `repository` | no | string | HTTPS source URL (`org.opencontainers.image.source`) |
| `deprecated` | no | string | Deprecation notice (`com.grimoire.deprecated`) |
| `server` | yes | table | Transport plus launch/connection fields, see below |

The `[server]` table's required fields depend on `transport`:

| Field | Required for | Notes |
|-------|--------------|-------|
| `transport` | always | `stdio`, `http`, or `sse` |
| `command` | `stdio` | Executable to launch |
| `args` | `stdio`, optional | Arguments appended to `command` |
| `env` | `stdio`, optional | String→string map; values may reference `${VAR}` |
| `url` | `http`/`sse` | Must start with `http://` or `https://` |
| `headers` | `http`/`sse`, optional | String→string map, same `${VAR}` referencing |

### Example — a local server {#mcp-example-stdio}

```toml
# mcp/grim.toml
description = "Grimoire catalog search and install status over MCP."

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
```

Full field reference, the per-client emit matrix, publishing, and the
semantic modification-detection model live in
[MCP Server Artifacts](./mcp-servers.md).

## Bundles {#bundles}

A bundle is a curated set of references to other artifacts. Its source is
a `.toml` file; the published artifact carries only a JSON members
document, so a bundle never materializes files of its own — installing it
expands to installing its members.

Top-level keys and member tables:

| Key / table | Required | Type | Notes |
|-------------|----------|------|-------|
| `summary` | no | string | Short one-line blurb for the catalog |
| `keywords` | no | string | Comma-separated tags |
| `description` | no | string | Longer description; defaults to a deterministic `grimoire bundle of N members` |
| `license` | no | string | SPDX-style identifier (`org.opencontainers.image.licenses`) |
| `[skills]` | no | name → ref table | Skill members |
| `[rules]` | no | name → ref table | Rule members |
| `[agents]` | no | name → ref table | Agent members |
| `[mcp]` | no | name → ref table | MCP server members; the name is the key the server registers under in each client's MCP config |
| `[bundles]` | no | name → ref table | Bundle members: a bundle that groups another bundle, see [nested bundles](#bundle-nested) |
| `[hooks]` | no | name → ref table | Hook members (experimental). A hook arms only past the `[options.experimental] hooks` flag and, at project scope, workspace consent. See [Hooks][hooks] |
| `[settings]` | no | name → ref table | Settings members (experimental). A package applies only past the `[options.experimental] settings` flag and, at project scope, workspace consent. See [Settings][settings] |

Each member entry maps the **config binding name** (the name the member is
declared under when the bundle is added) to a fully-qualified reference —
`registry/repo:tag` or `registry/repo@sha256:…` — or a
[deployment-relative reference](#bundle-relative-refs). Floating tags
re-resolve on `grim update`; digest pins never move
([floating versus pinned members](./concepts.md#bundle-pinning)).

Limits enforced at parse time: at most 512 members per bundle, and the
members document is capped at 512 KiB.

### Nested bundles {#bundle-nested}

A `[bundles]` table lists other bundles as members. Resolution expands a
nested bundle exactly as it expands a declared one, to any depth, within the
[graph limits](./concepts.md#dependency-limits). A bundle that names itself
fails `grim build` with exit `65`, and a cycle through several bundles is
refused at lock time. A bundle's members are its dependencies: the members
document is the same document an artifact publishes as its
[requires layer](#requires-layer).

```toml
# full-stack.toml
[bundles]
python-stack = "ghcr.io/acme/bundles/python-stack:1"
web-stack = "./web-stack:1"
```

A grim before 0.16.0 refuses a bundle with a nested bundle member (exit
`65`), see [Upgrading](./upgrading.md#dependencies-need-new-grim).

### Deployment-relative members {#bundle-relative-refs}

A bundle authored with fully-qualified members hard-codes one registry: a
mirror of the bundle — or a publish under an enforced
[`--registry host/prefix`](./publishing.md#batch-publish-namespace)
namespace — still points its consumers at the original member locations.

A member value may instead be **relative to the bundle's own deployment**:
a leading `./` names the directory of the bundle's repository, and each
leading `../` climbs one directory. The relative form is stored verbatim
in the published bundle and resolved at install time against wherever the
bundle was actually pulled from, so one published bundle works unchanged
under any mirror or namespace:

```toml
# Published at ghcr.io/acme/bundles/tools — and any mirror of it.
[skills]
x = "../skills/x:0"   # → <bundle-registry>/<prefix>/skills/x:0
y = "./y:1"           # → …/bundles/y:1 (the bundle's own directory)
```

Relativity is explicit: a bare `skills/x:0` is still rejected as missing
its registry. `.`/`..` are only valid as the leading run (`./a/../b` is
an error), and a `../` chain that would climb above the registry root
fails the release (exit 65) — at publish time, not at a consumer's
install. Releasing with [`--pin`](./concepts.md#bundle-pinning) resolves
a relative member against the release target first, then freezes it to
its absolute digest-pinned form — reproducibility forfeits late binding.
Bundles carrying relative members require a grim release with this
feature; an older grim fails such a bundle cleanly as invalid.

### Example — bundle with all member kinds {#bundle-example}

```toml
# starter-pack.toml
summary = "Curated starter pack"
keywords = "starter,review,style,security"
description = "The code-review skill plus the Rust style rule and review agent"

[skills]
code-reviewer = "registry.example.com/grimoire/skills/code-reviewer:1"

[rules]
rust-style = "registry.example.com/grimoire/rules/rust-style:1"

[agents]
reviewer = "registry.example.com/grimoire/agents/reviewer@sha256:8f4b…"

[mcp]
docs-search = "registry.example.com/grimoire/mcp/docs-search:1"

[hooks]
shell-guard = "registry.example.com/grimoire/hooks/shell-guard:1"

[settings]
team-defaults = "registry.example.com/grimoire/settings/team-defaults:1"
```

## Declaring dependencies {#requires}

A skill, rule, agent, MCP server, hook, or settings package can declare the
artifacts it needs.
Installing it then installs them too, and removing it removes what only it
needed. A [bundle's](#bundles) member list already works this way. See
[Dependencies](./concepts.md#dependencies) for what resolution does with the
declarations.

The grammar is the same for every kind: a table per target kind, a binding name
per entry, and a reference as the value.

| Table | Target kind |
|-------|-------------|
| `skills` | skill |
| `rules` | rule |
| `agents` | agent |
| `mcp` | MCP server |
| `hooks` | hook |
| `bundles` | bundle |
| `settings` | settings |

The binding name is the name the dependency installs under, and it follows the
[name rules](#names). The reference is a fully-qualified `registry/repo:tag` or
`registry/repo@sha256:…`, or a
[deployment-relative reference](#bundle-relative-refs) (`./gws-shared:1`)
that resolves against the repository the declaring artifact was pulled from.
Prefer the relative form for artifacts published side by side, because it
survives a mirror.

Where the declaration lives depends on the kind:

| Kind | Declaration |
|------|-------------|
| Skill, rule, agent | `grim.requires.<table>.<name>` keys in the frontmatter `metadata` map |
| MCP server | `[requires.<table>]` tables in `mcp/<name>.toml` |
| Hook | `[requires.<table>]` tables in `hook.toml` |
| Settings | `[requires.<table>]` tables in the settings document, kept out of its layer |
| Bundle | The `[bundles]` table of the source, see [nested bundles](#bundle-nested) |

```yaml
---
name: gws-drive
description: Read and edit Google Drive files
metadata:
  grim.requires.skills.gws-shared: "./gws-shared:1"
  grim.requires.mcp.gdrive: "ghcr.io/acme/mcp/gdrive:2"
---
```

```toml
# mcp/gdrive.toml
description = "Google Drive access over MCP."

[server]
transport = "stdio"
command = "gdrive-mcp"

[requires.skills]
gws-shared = "ghcr.io/acme/skills/gws-shared:1"
```

Metadata values are strings, so each dependency is one flat key. The keys ride
in the packed skill, rule or agent as ordinary metadata, which no client
reads. A `[requires]` table in an MCP descriptor is split off before the
descriptor is packed, so it never changes the descriptor's bytes.

[`grim build`](./commands.md#build) checks every declaration offline. It exits
`65` for an unknown table, an invalid binding name, an unparseable reference,
the same `(kind, name)` declared twice, or an artifact that requires itself. [`grim release`](./commands.md#release) additionally checks that each
relative reference stays under the registry root of the release target. Without
`--pin` neither looks the dependency up in a registry, so a dependency may be
published after the artifact that needs it.

Declared dependencies are stored sorted by kind, in the order skill, rule,
agent, bundle, mcp, hook, settings, and then by name. That order is part of the
published bytes. It is not the alphabetical order of the table names.

## Vendor extensions {#vendor-extensions}

Client-specific capabilities are authored as string-valued
`<vendor>.<field>` keys in the artifact's `metadata` map and lifted into
native typed frontmatter at install time. The published artifact stays
spec-compliant; each client sees only its own namespace.

The recognized keys per vendor and kind — full type and projection detail
in [Vendor-Specific Metadata](./vendor-metadata.md):

| Vendor | Skills | Rules | Agents |
|--------|--------|-------|--------|
| `claude.*` | `disable-model-invocation`, `user-invocable`, `model`, `effort`, `context`, `agent`, `argument-hint`, `when-to-use`, `arguments`, `allowed-tools`, `disallowed-tools`, `shell`, `paths`, `background` ([registry](./vendor-metadata.md#claude-registry)) | *(none today — unknown keys warn + drop)* | `model`, `tools`, `disallowed-tools`, `permission-mode`, `max-turns`, `skills`, `memory`, `background`, `effort`, `isolation`, `color`, `initial-prompt`, `omit-claude-md` ([registry](./vendor-metadata.md#claude-agent-registry)) |
| `opencode.*` | *(none — universal fields only)* | *(none)* | `model`, `mode`, `temperature`, `top-p`, `steps`, `prompt`, `disable`, `hidden`, `color` ([registry](./vendor-metadata.md#opencode-agent-registry)) |
| `copilot.*` | *(none — universal fields only)* | `exclude-agent` ([registry](./vendor-metadata.md#rule-keys)) | `tools`, `model`, `disable-model-invocation`, `user-invocable`, `target` ([registry](./vendor-metadata.md#copilot-agent-registry)) |
| `codex.*` | *(none — universal fields only)* | **unsupported** — warns + skips | `model`, `reasoning-effort`, `sandbox-mode` ([registry](./vendor-metadata.md#codex-agent-registry)) |

Every value is authored as a string and converted at install time:

| Declared type | Accepted literals | On bad literal |
|---------------|-------------------|----------------|
| bool | `"true"`, `"false"` | hard error (exit 65) |
| enum | the closed set listed in the registry | hard error (exit 65) |
| integer | base-10 digits | hard error (exit 65) |
| float | any finite float | hard error (exit 65) |
| comma list | any; split on `,` into a YAML list | never fails |
| string | any | never fails |

A **known** key with a bad literal fails the publish. An **unknown** key in
your own namespace (a typo like `claude.efort`) warns and drops. A key in a
**foreign** namespace drops silently — that is how one canonical file
serves several clients
([publish-time validation](./vendor-metadata.md#publish-validation)).

## Layer encoding {#layers}

A skill, rule, agent or hook publishes as one content layer holding a tar
archive. When it [declares dependencies](#requires), a second layer follows it,
see [the requires layer](#requires-layer). The content layer is either a plain
tar or a gzip-compressed tar. A publisher opts in
to gzip with [`--compression gzip`](./publishing.md#compression). The default
stays an uncompressed tar.

grim classifies a layer by its exact media type, with no parameters and no
content sniffing:

| Layer media type | How grim reads it |
|------------------|-------------------|
| `application/vnd.grimoire.artifact.layer.v1.tar+gzip` | gzip tar. The type grim writes. |
| `application/vnd.oci.image.layer.v1.tar+gzip` | gzip tar |
| `application/vnd.docker.image.rootfs.diff.tar.gzip` | gzip tar |
| any type ending `+zstd` | refused with exit `65` and the error reason `unsupported-layer-media-type` |
| any other type, `…layer.v1.tar` included | raw tar |

An unknown type still reads as a raw tar, because refusing it would break
artifacts that install today. grim writes only the first type above.

A gzip layer must be a single gzip member with no trailing bytes. Its header
must carry no extra, name or comment field. Anything else exits `65`.
The same size limit that applies without compression bounds the decoded
output. A compressed artifact therefore never carries more content than an
uncompressed one.

A gzip layer's descriptor carries a `com.grimoire.uncompressed.digest`
annotation. It holds the `sha256:` digest of the uncompressed tar. grim checks
it against the decoded bytes on every read. A mismatch or a malformed value
exits `65`. The annotation is written only on compressed layers.

A gzip layer without the annotation, such as one a third-party tool wrote, is
decoded without that check. An annotation with an unknown digest algorithm is
skipped for the same reason.

The decoded size is bounded twice. The caller's own cap applies, and so does
an expansion-ratio bound of 100 times the stored bytes, with a floor of
64 MiB. The tighter of the two wins, so a small layer cannot inflate far past
its stored size. Exceeding either exits `65`.

Bundle, MCP and settings layers are small JSON documents and are never
compressed.

### The requires layer {#requires-layer}

An artifact that declares [dependencies](#requires) carries them as a second
layer, after its content layer:

| Layer | Media type | Holds |
|-------|------------|-------|
| 1 | the artifact's own content type | the skill, rule, agent, hook or MCP descriptor, unchanged |
| 2 | `application/vnd.grimoire.bundle.v1+json` | the dependencies, as a JSON document |

The second layer is the same members document a bundle publishes as its only
layer. It holds a `members` array of `{kind, name, id}` objects in the
canonical order. Each `id` is stored exactly as written, so a relative
`./gws-shared:1` stays relative. The layer is never compressed and its
descriptor carries no annotations. `--compression` applies to layer 1 only.

An artifact that declares nothing publishes the single layer it always did. Its
manifest is byte-identical to one built before dependencies existed.

A two-layer manifest is a grim convention, not part of the OCI specification.
A registry stores it like any other manifest. grim tells the two layers apart
by media type, not by position. It accepts exactly two shapes:

- one layer, for every kind (for a bundle it is the members document)
- one content layer and one requires layer, in either order, for every kind
  except a bundle

grim always writes the content layer first. Any other shape is refused with
exit `65` and the error reason `unsupported-artifact-feature`, which tells the
user to update grim. Three layers, two layers of one type, and two layers on a
bundle are all refused. So is a skill, rule, agent, hook or MCP server whose
only layer is a requires layer.
[Stability](./stability.md#limitations-forward-compat) covers how an earlier
grim meets a two-layer artifact.

A manifest may also list the features a reader must understand in the
`com.grimoire.requires-features` annotation, a comma-separated list. This grim
understands `dependencies`. It refuses an artifact that names any other feature,
with the same exit and reason. It refuses a malformed value the same way: over
1024 bytes, more than 32 features, or a name outside lowercase letters, digits
and hyphens.

grim reads the annotation but does not write it today, because a dependent
artifact is recognized by its layout alone. The key is reserved so that a later
format can turn readers that predate it away cleanly.

## Catalog annotations {#annotations}

On the wire, catalog metadata travels as OCI manifest annotations. grim
emits standard [OCI image-spec annotation][oci-annotations] keys plus a few
Grimoire-specific ones.

Where a kind keeps a field differs — skills and agents use the `metadata` map,
rules put it at the top level of their frontmatter, bundles and MCP
descriptors at the top level of their TOML. "authored" below means whichever
of those applies.

| Annotation | Source | Emitted |
|------------|--------|---------|
| `org.opencontainers.image.title` | artifact name | always |
| `org.opencontainers.image.description` | `description` field, or derived from the rule body | always |
| `org.opencontainers.image.version` | release version | always |
| `org.opencontainers.image.licenses` | authored `license` | when present |
| `org.opencontainers.image.source` | authored `repository` HTTPS URL; then the git `origin` remote under [`--git`](./publishing.md#git-disclosure); falls back to the tagless release ref | always on release |
| `org.opencontainers.image.revision` | the `HEAD` commit SHA | [by default](./publishing.md#git-provenance), inside a repository |
| `org.opencontainers.image.created` | the commit date, else `SOURCE_DATE_EPOCH` | [by default](./publishing.md#git-provenance), when either is available |
| `org.opencontainers.image.authors` | authored `authors`; then the commit author's name under [`--git`](./publishing.md#git-disclosure) | when present |
| `org.opencontainers.image.vendor` | authored `vendor`; else the release repository's namespace | when present or derivable |
| `org.opencontainers.image.url` | authored `homepage`; else the authored `repository` | when present or derivable |
| `org.opencontainers.image.documentation` | authored `documentation`; else `<repository>#readme` | when present or derivable |
| `com.grimoire.summary` | authored `summary` | when present |
| `com.grimoire.keywords` | authored `keywords` | when present |
| `com.grimoire.compatibility` | a skill's top-level `compatibility` | when present (skills only) |
| `com.grimoire.deprecated` | authored `deprecated` | when non-empty |
| `com.grimoire.replaced-by` | authored `replaced-by` | when non-empty |

Every authored value can also be supplied by a
[flag or a `publish.toml` table](./publishing.md#metadata-flags), which fill a
gap without ever overriding what the artifact file says about itself.

An authored `repository` must be an `https://` URL — anything else fails
the publish (exit 65). Readers distinguish a real repository URL from the
legacy release-ref fallback by that `https://` prefix; on registries that
honor the key (e.g. [ghcr.io][ghcr-source-label]) the source annotation
also links the package back to its repository.

**Nothing in this map is read from the clock.** `…image.created` is a commit
date or a fixed `SOURCE_DATE_EPOCH` instant, so re-releasing identical content
from the same commit stays byte-identical (idempotent re-release). A wall-clock
timestamp would break that guarantee, and grim never writes one.

Repository-level support channels
(`com.grimoire.support.{issues,chat,contact,security}`) are **not** in this
map: they live on the mutable
[description companion](./publishing.md#support-channels) so they can be
updated without re-releasing every published version.

<!-- internal -->
[hooks]: ./hooks.md
[hooks-exit]: ./hooks.md#handler-exit
[settings]: ./settings.md
[settings-format]: ./settings.md#format
[stability-unstable]: ./stability.md#unstable

<!-- external -->
[agentskills-spec]: https://agentskills.io/specification
[gitignore-spec]: https://git-scm.com/docs/gitignore
[oci-annotations]: https://github.com/opencontainers/image-spec/blob/main/annotations.md
[ghcr-source-label]: https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry#labelling-container-images
[mcp-spec]: https://modelcontextprotocol.io/specification/latest
