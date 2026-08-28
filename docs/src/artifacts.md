# Artifact Reference

Grimoire ships six artifact kinds — skills, rules, agents, MCP servers,
hooks, and bundles. Each has its own source shape, frontmatter schema, and
validation rules, and until now those details lived scattered across the
publishing, agents, and vendor-metadata chapters.

When you author an artifact you need one page that answers: which fields
exist, which are required, what values are valid, and what a correct file
looks like. This page is that reference. Narrative background stays in
[Concepts](./concepts.md); publishing mechanics stay in
[Publishing](./publishing.md); vendor projection semantics stay in
[Vendor-Specific Metadata](./vendor-metadata.md); the full MCP server
reference lives in its own chapter,
[MCP Server Artifacts](./mcp-servers.md).

## Artifact kinds {#kinds}

Every artifact carries its kind in a `com.grimoire.kind` manifest
annotation, so registries and tooling can distinguish kinds without
downloading layers.

| Kind | Source shape | `com.grimoire.kind` | Installs as |
|------|--------------|---------------------|-------------|
| **Skill** | Directory with a `SKILL.md` index | `skill` | Directory tree under the client's `skills/` dir |
| **Rule** | Single `.md` file (+ optional sibling support directory) | `rule` | `rules/<name>.md` (+ `rules/<name>/…`), per-client transform |
| **Agent** | Single `.md` file | `agent` | One agent file per client, per-client rendering |
| **MCP server** | `mcp/<name>.toml` | `mcp` | Entry registered in each client's own MCP config file — never a materialized file |
| **Hook** | Directory with a `hook.toml` manifest | `hook` | A shared payload directory plus a registration in the client's own hooks config — and only once [both gates](#hook-gates) allow it |
| **Bundle** | `.toml` member list | `bundle` | Never materializes itself — expands to its members |

The manifest's config descriptor is the OCI empty config
(`application/vnd.oci.empty.v1+json`) — universally allow-listed, including
on GitLab Container Registry, which rejects custom config and `artifactType`
media types (see [Registry compatibility](./configuration.md#registry-compatibility)).
Earlier releases instead stamped a custom OCI `artifactType`
(`application/vnd.grimoire.<kind>.v1`) and a per-kind config media type;
grim still reads those when present, so artifacts published before this
change resolve their kind unchanged.

`grim build` and `grim release` infer the kind from the path — a directory
is a skill or a hook, a `.md` file is a rule, a `.toml` file is a bundle.
Directories are told apart by their index file, which is why a hook needs
no flag: a directory carrying a `hook.toml` is a hook, one carrying a
`SKILL.md` is a skill, and `hook.toml` is tested first. Two kinds *are* the
exception, because their shape collides with another kind's and no index
file separates them: an agent `.md` is indistinguishable from a rule, so
`--kind agent` is required (see [Agent
Artifacts](./agents.md#publishing)); an MCP descriptor `.toml` is
indistinguishable from a bundle, so `--kind mcp` is required (see [MCP
Server Artifacts](./mcp-servers.md#publishing)).

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
single tar layer and installed verbatim — only `SKILL.md` itself is ever
re-rendered, and only when it carries vendor-namespaced metadata keys.

The frontmatter follows the [agentskills specification][agentskills-spec].
Parsing is forward-compatible: unknown top-level keys are preserved
round-trip rather than rejected.

| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `name` | yes | string | Must equal the skill directory name; see [Names](#names) |
| `description` | yes | string | What the skill does and when to use it |
| `license` | no | string | SPDX-style identifier (e.g. `Apache-2.0`); emitted as the OCI license annotation |
| `compatibility` | no | string | Editor/runtime hint (free text) |
| `allowed-tools` | no | string | Comma-separated tool allowlist |
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
allowed-tools: Read,Grep,Bash
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

## Hooks {#hooks}

A hook binds a handler to a moment in an agent's lifecycle: run this
before a tool call, after one, at session start, when the turn stops. Its
source is a directory with a `hook.toml` manifest at the top and the
handler files beside it, so one artifact can ship a pre/post pair that
shares a payload tree.

Every other kind is inert until an agent chooses to read it. A hook is the
opposite — installing one means a client will execute it automatically,
without a prompt, on someone else's schedule. So a hook is the only kind
grim refuses to activate on the strength of a declaration alone: it needs
[two deliberate opt-ins](#hook-gates) as well.

| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `schema` | yes | integer | Manifest and envelope contract version. This release writes and reads `1` |
| `name` | yes | string | Artifact name, under the same [charset rules](#names) as every other kind. Must equal the containing directory's name — the same rule a skill's `SKILL.md` follows |
| `description` | yes | string | Becomes the OCI description annotation |
| `[[hooks]]` | no | array of tables | The handlers, one table per handler — see below |

Unknown keys at the **document** level are a hard parse error, as in every
other grim manifest. Inside a `[[hooks]]` entry they are deliberately
**preserved** instead, which is what lets a `<vendor>.<field>` override table
and the reserved `policy` key survive a round trip through a grim that
predates them.

Author `hook.toml` in the **TOML 1.0-compatible subset** — unquoted dotted
keys and single-line inline tables only. Grim's own parser accepts TOML 1.1
forms, but a published `hook.toml` is read by third-party tooling whose stock
1.0 parsers hard-reject them, so grim is liberal in what it accepts and
conservative in what it emits and documents. One narrowing follows from the
same reasoning: a TOML **datetime**, local-date, or local-time under `policy`
or a vendor key is rejected at `grim build`, because grim cannot re-emit those
types without corrupting them.

Each `[[hooks]]` entry is one handler bound to one moment and one tier:

| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `id` | yes | string | Stable id, unique within the artifact. ASCII letters, digits, `_`, `-` and `.` only, max 128 bytes. Reaches the dispatch table and the audit trail as `<artifact>/<id>` |
| `tier` | yes | enum | `observer`, `gatekeeper`, or `mutator` — see [Tiers](#hook-tiers) |
| `event` | no | enum | `PreToolUse`, `PostToolUse`, `SessionStart`, or `Stop`. Omitted only when a `<vendor>.event` override stands alone, naming a moment that exists on exactly one client |
| `command` | one of | string | The handler as a single string, handed to the platform shell |
| `argv` | one of | array of string | The handler in exec form — an argument vector, no shell involved |
| `matcher` | no | string | Which tool the handler applies to: an exact name or a glob, **never** a regex. Restricted to `A-Za-z0-9_*?./-\|` and 256 bytes, checked at `grim build` |
| `timeout` | no | integer | Per-handler timeout in seconds; `30` when omitted. **Grim** enforces it rather than the client, so the behaviour is identical everywhere |
| `payload` | no | enum | `stdin` (the default) hands the handler one JSON object on stdin; `file` writes the envelope to a file and exports its path as `GRIM_HOOK_PAYLOAD` |
| `policy` | no | table | Reserved for a future vocabulary. Captured unparsed and re-emitted, so a grim that predates it preserves it |

Exactly one of `command` and `argv` is required — they are the two spellings
of "what to run", not alternatives you may combine.

### Tiers {#hook-tiers}

A handler's tier is a declaration of how much power it is asking for, and
grim enforces the ceiling rather than trusting the handler to stay inside
it:

| Tier | May do | Restricted to |
|------|--------|---------------|
| `observer` | Read the event. Its response cannot change what happens | any event |
| `gatekeeper` | Return a verdict that blocks the operation | events that admit a verdict — and not every client admits one on every event (see [Client Compatibility](./clients.md#gap-copilot-hooks)) |
| `mutator` | Rewrite the tool's input | `PreToolUse` only, and refused **per tool** for tools whose input is a shell command |

A tier a client cannot honour is **declined** for that pair rather than
quietly downgraded, so a `gatekeeper` never silently becomes an
`observer`. The `mutator` refusal is the one that is per *tool* rather than
per client: a matcher that could select a shell-command tool is refused even
on a client that otherwise supports rewriting. The same rule covers `matcher`: grim's dialect is translated
into each client's own, and a matcher that cannot be translated losslessly
declines that `(hook, client)` pair rather than being approximated —
an inert or over-broad matcher that still reported as installed would be
worse than an honest refusal.

> **A grim hook is defence in depth, never a security boundary.** A
> `gatekeeper` that does not fire — because grim is not installed, the
> launcher is missing, or the client never registered it — is *by design*:
> every layer fails open so a broken guardrail can never deny you a tool
> call. Do not put a control you actually rely on behind one.
>
> One case is worth naming because it looks armed: on Codex, a hook runs
> through your **login shell**, so a `fish` or `nushell` user gets a hook
> that installs, reports `installed`, and never runs — see
> [Codex: hooks need a POSIX login shell](./clients.md#gap-codex-shell).

### The two gates {#hook-gates}

Declaring a hook is not arming it. `grim add` and `grim lock` treat a hook
like any other artifact, and `grim install` then **skips** it unless both
of these allow it:

1. **The feature flag** — `hooks = true` under
   [`[options.experimental]`](./configuration.md#options-experimental).
   Off by default, per scope.
2. **Workspace consent** — this checkout must carry a
   [consent record](./configuration.md#workspace-consent), written by
   `grim hook allow`, by `grim add`, or by a prompt you accepted. Cloning a
   repository is not consenting to it, so a declared hook in a fresh clone
   arms nothing until you say so. Global scope needs no record: it is your
   own config on your own machine, and is always consented.

Until both pass, [`grim status`](./commands.md#status) reads `gated` and
names which gate it was — `feature-flag-off`, `workspace-not-consented`, or
`consent-drifted`. The exit code stays `0`, because a gate doing its job is
not a failure. For a run with no terminal to prompt on — CI, a
cloud agent — [`grim install --trust-hooks`](./commands.md#install) answers
gate 2 for that invocation, and `--no-trust-hooks` refuses it; the pair
outranks the record in both directions, writes nothing, and there is
deliberately no environment variable that does the same thing. Neither opens
gate 1.

A third condition sits outside both gates and cannot be answered by any
file: a hook whose pinned registry host is not loopback and is reached over
[plain HTTP](./configuration.md#workspace-consent-transport) never arms,
whatever the record says.

### Example — a pre-tool observer {#hook-example-observer}

```toml
# shell-guard/hook.toml
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

Built from the directory holding it, with no `--kind` flag:

```sh
$ grim build ./shell-guard
Kind  Name         Path            Layer Digest      Status
hook  shell-guard  ./shell-guard   sha256:272331f3…  built
```

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
| `[hooks]` | no | name → ref table | [Hook](#hooks) members. A bundle carrying one is disclosed on install, and adding the bundle consents to the hook members that gesture resolves to — the record stores the resolved set, so a bundle that later gains a member drifts and re-asks rather than arming silently |
| `[mcp]` | no | name → ref table | MCP server members; the name is the key the server registers under in each client's MCP config |

Each member entry maps the **config binding name** (the name the member is
declared under when the bundle is added) to a fully-qualified reference —
`registry/repo:tag` or `registry/repo@sha256:…` — or a
[deployment-relative reference](#bundle-relative-refs). Floating tags
re-resolve on `grim update`; digest pins never move
([floating versus pinned members](./concepts.md#bundle-pinning)).

Limits enforced at parse time: at most 512 members per bundle, and the
members document is capped at 512 KiB. Nested bundles are invalid — a
member's kind may be `skill`, `rule`, `agent`, `hook`, or `mcp`, and a
`bundle` member is rejected at expansion.

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
```

## Vendor extensions {#vendor-extensions}

Client-specific capabilities are authored as string-valued
`<vendor>.<field>` keys in the artifact's `metadata` map and lifted into
native typed frontmatter at install time. The published artifact stays
spec-compliant; each client sees only its own namespace.

The recognized keys per vendor and kind — full type and projection detail
in [Vendor-Specific Metadata](./vendor-metadata.md):

| Vendor | Skills | Rules | Agents |
|--------|--------|-------|--------|
| `claude.*` | `disable-model-invocation`, `user-invocable`, `model`, `effort`, `context`, `agent`, `argument-hint`, `when-to-use`, `arguments`, `allowed-tools`, `disallowed-tools`, `shell`, `paths` ([registry](./vendor-metadata.md#claude-registry)) | *(none today — unknown keys warn + drop)* | `model`, `tools`, `disallowed-tools`, `permission-mode`, `max-turns`, `skills`, `memory`, `background`, `effort`, `isolation`, `color`, `initial-prompt` ([registry](./vendor-metadata.md#claude-agent-registry)) |
| `opencode.*` | *(none — universal fields only)* | *(none)* | `model`, `mode`, `temperature`, `top-p`, `steps`, `prompt`, `disable`, `hidden`, `color` ([registry](./vendor-metadata.md#opencode-agent-registry)) |
| `copilot.*` | *(none — universal fields only)* | `exclude-agent` ([registry](./vendor-metadata.md#rule-keys)) | `tools`, `model` ([registry](./vendor-metadata.md#copilot-agent-registry)) |
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

<!-- external -->
[agentskills-spec]: https://agentskills.io/specification
[oci-annotations]: https://github.com/opencontainers/image-spec/blob/main/annotations.md
[ghcr-source-label]: https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry#labelling-container-images
[mcp-spec]: https://spec.modelcontextprotocol.io/
