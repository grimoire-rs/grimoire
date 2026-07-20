# Agent Artifacts

Skills teach an agent a capability and rules constrain it; an **agent
artifact** defines an agent itself — a named, delegatable assistant with
its own system prompt, model, and tool access.

Every major AI client has grown such a definition format: [Claude Code
subagents][claude-subagents-docs], [OpenCode agents][opencode-agents-docs],
[Copilot CLI custom agents][copilot-agents-docs], and [OpenAI Codex
agents][codex-subagents-docs]. Three of the four read a Markdown file with
YAML frontmatter whose body is the system prompt — but each with its own
field names, its own directory, and its own quirks. Codex takes a different
path entirely: it reads TOML. Teams end up copy-pasting near-identical
agent files between repositories and editing four variants by hand.

Grimoire treats an agent like any other artifact: author **one canonical
file**, publish it once, and let `grim install` project it into each
client's native format — the same model that powers
[vendor-specific metadata][vendor-metadata] for skills and rules.

## The canonical format {#format}

An agent is a single `.md` file. Unlike a [rule](./concepts.md), the
frontmatter is **required** — every client needs at least a description
to route work to the agent:

```yaml
# code-reviewer.md
---
name: code-reviewer
description: Reviews diffs for correctness, security, and style.
model: sonnet
tools: Read,Grep,Bash
metadata:
  summary: Multi-pass diff reviewer
  keywords: review,quality
  claude.memory: project
  opencode.mode: subagent
  opencode.temperature: "0.2"
---
You are a code reviewer. Analyze the diff and report specific,
actionable findings.
```

The body below the frontmatter is the agent's system prompt and installs
verbatim for every client.

### Common fields {#common-fields}

| Field | Required | Type | Validation |
|---|---|---|---|
| `name` | yes | string | Must equal the file stem (`code-reviewer.md` ⇒ `code-reviewer`); lowercase letters, digits, hyphens, periods — see [Names](./artifacts.md#names) |
| `description` | yes | string | Free text — when a client should delegate to this agent |
| `model` | no | string | Passed through verbatim to each client; **no alias translation** |
| `tools` | no | string | Comma-separated tool list, projected into each client's native shape |
| `metadata` | no | string→string map | Catalog keys (`summary`, `keywords`) plus [vendor-namespaced keys][vendor-metadata] (`<vendor>.<field>`) |

The name-equals-stem rule exists because [OpenCode][opencode-agents-docs]
derives an agent's identity from its filename; Grimoire enforces the rule
for every client so the identity is consistent everywhere.

Everything a single vendor understands — Claude's `permissionMode`,
OpenCode's `temperature`, Copilot's tool restrictions — is authored as a
`<vendor>.<field>` string key inside `metadata`. The full key tables live
in the [vendor metadata reference][vendor-agent-registries].

### Override precedence {#override-precedence}

The common `model` and `tools` fields are *defaults*. When a vendor key
lifts to the same native field, the vendor key **wins for that vendor** —
silently, because the collision is the documented escape hatch:

```yaml
model: sonnet
metadata:
  claude.model: opus                       # Claude installs model: opus
  opencode.model: anthropic/claude-sonnet-4-5  # OpenCode gets this instead of "sonnet"
```

This matters most for `model`: [Claude Code][claude-subagents-docs] reads
aliases like `sonnet`, while [OpenCode][opencode-agents-docs] expects a
`provider/model-id` string. Grimoire deliberately does **not** translate
between the two — set `opencode.model` when the common value is not what
OpenCode needs.

## What each client receives {#emit-matrix}

On install, grim projects the canonical agent file into each client's native
format, or declines it where the client ships no ownable agent surface. Agents
are the least uniform kind — only six clients have a file format grim can own.

| Client | Output | What grim projects |
|---|---|---|
| [Claude Code][claude-subagents-docs] | `.md` + YAML frontmatter | the canonical format verbatim — a plain agent installs byte-identical (`generated: false`), no provenance comment |
| [OpenCode][opencode-agents-docs] | `.md`, no frontmatter | `name` dropped (the filename is the identity), `tools` dropped (deprecated upstream), `opencode.*` lifted; provenance comment |
| [Copilot CLI][copilot-agents-docs] | `.md` + YAML frontmatter | `tools` emitted as a YAML **list**, `copilot.*` lifted; provenance comment |
| [Codex][codex-subagents-docs] | **TOML** (`<name>.toml`) | body → `developer_instructions`, `tools` dropped with a warning, `codex.*` lifted; `#` provenance |
| [Cursor][cursor-subagents-docs] | `.md` + YAML frontmatter | `cursor.*` lifted (`model`, `readonly`, `is_background`), `tools` dropped with a warning; provenance comment |
| [Gemini CLI][gemini-subagents-docs] | `.md` + YAML frontmatter | `gemini.*` lifted (`model`, `temperature`, `max_turns`, `timeout_mins`, `kind`); loaded only when `experimental.enableAgents` is on (the default) |
| [Kiro][kiro-docs] | not supported | agents declined — the Kiro CLI expects an incompatible schema in the same directory ([kiro #8040]) |
| [Junie][junie-docs] | not supported | agents declined — the `.junie/agents/` format is early-access-preview only |
| [Zed][zed-docs] | not supported | agents declined — external agents run over ACP with no installable file |
| [Amp][amp-docs] | not supported | agents declined — subagents are spawned at runtime with no file format |

The canonical format **is** Claude Code's native subagent format, so a plain
agent — one with no `<vendor>.<field>` metadata keys — installs for Claude
byte-identical to the published file (`generated: false`). Every other output
is a generated transform carrying a provenance comment; editing one by hand is
detected as [drift][vendor-drift], exactly like any generated file. See the
[client compatibility matrix][clients-matrix] for the full support picture and
the per-vendor [agent registries][vendor-agent-registries] for the lifted
`<vendor>.*` keys.

## Install locations {#locations}

These concrete paths are **not** part of the stability contract — vendor render
layout may change in any minor release (see [stability][stability-unstable]).

**Project scope:**

| Client | Path |
|---|---|
| [Claude Code][claude-subagents-docs] | `.claude/agents/<name>.md` |
| [OpenCode][opencode-agents-docs] | `.opencode/agents/<name>.md` |
| [Copilot CLI][copilot-agents-docs] | `.github/agents/<name>.md` |
| [Codex][codex-subagents-docs] | `.codex/agents/<name>.toml` |
| [Cursor][cursor-subagents-docs] | `.cursor/agents/<name>.md` |
| [Gemini CLI][gemini-subagents-docs] | `.gemini/agents/<name>.md` |

**Global scope** (native user-level discovery directories, honoring each
client's directory-override variable — the same resolution as
[skill discovery][vendor-discovery]):

| Client | Path | Env override |
|---|---|---|
| [Claude Code][claude-subagents-docs] | `~/.claude/agents/<name>.md` | `$CLAUDE_CONFIG_DIR/agents/` |
| [OpenCode][opencode-agents-docs] | `~/.config/opencode/agents/<name>.md` (XDG) | `$OPENCODE_CONFIG_DIR/agents/` |
| [Copilot CLI][copilot-agents-docs] | `~/.copilot/agents/<name>.md` | `$COPILOT_HOME/agents/` |
| [Codex][codex-subagents-docs] | `~/.codex/agents/<name>.toml` | `$CODEX_HOME/agents/` |
| [Cursor][cursor-subagents-docs] | `~/.cursor/agents/<name>.md` | None — `CURSOR_CONFIG_DIR` is not honored |
| [Gemini CLI][gemini-subagents-docs] | `~/.gemini/agents/<name>.md` | None — no `GEMINI_CONFIG_DIR` exists upstream |

Unlike global rules, Copilot agents have a real user-level home — no
inert-install warning applies.

## Publishing {#publishing}

`grim build` and `grim release` need `--kind agent` for an agent file:

```sh
grim build ./code-reviewer.md --kind agent
grim release ./code-reviewer.md ghcr.io/acme/code-reviewer:1.0.0 --kind agent
```

The flag is required because a bare `.md` path is indistinguishable from a
[rule](./publishing.md) by shape — and rules accept arbitrary frontmatter,
so guessing from content would silently flip kinds. When a file released
as a rule carries both `name` and `description`, grim warns that it looks
like an agent definition.

Publishing runs the same gate as skills and rules: every
`<vendor>.<field>` metadata key is validated against the vendor
registries, and an invalid literal (say `claude.permission-mode: yolo`)
fails the release with exit 65 before anything reaches the registry. The
artifact publishes with a `com.grimoire.kind` annotation of `agent`, so
[`grim add`](./commands.md#add) infers the kind with no flag.

Catalog metadata (`summary`, `keywords`) is authored in the `metadata`
map, like a skill — see [catalog metadata](./publishing.md#metadata).

## Consuming {#consuming}

Agents ride the standard lifecycle. Declarations live in an `[agents]`
table of `grimoire.toml`; the lock carries `[[agent]]` entries; and
[bundles](./concepts.md#bundles) accept agent members alongside skills
and rules:

```sh
grim add ghcr.io/acme/code-reviewer:1     # kind inferred from com.grimoire.kind
grim install                               # projects into every selected client
grim status                                # shows the agent row
grim uninstall agent code-reviewer         # removes files + declaration
```

## Limitations {#limitations}

- **Object-valued vendor fields** cannot be authored: the `metadata` map
  is string-valued by the agentskills contract, so Claude's `mcpServers`
  and `hooks`, OpenCode's `permission`, and Copilot's `mcp-servers` are
  not projectable. Add them by editing the installed file (Claude/Copilot)
  or the client's own config.
- **No support directory.** An agent packs to exactly one file
  (`<name>.md`, or `<name>.toml` for [Codex][codex-subagents-docs]); a
  sibling folder sharing the stem is ignored (unlike
  [rules](./concepts.md#rule-support-dir)).
- **No model translation.** The common `model` passes through verbatim;
  use `opencode.model` when the OpenCode side needs a
  `provider/model-id` value.
- **[Codex][codex-subagents-docs] `tools` field dropped.** [Codex][codex-subagents-docs]
  has no native equivalent for the `tools` field. When installing for
  [Codex][codex-subagents-docs], grim drops `tools` entirely and emits a
  warning. This is a hard drop, not a projection — no Codex TOML key
  carries the value.
- **[Codex][codex-subagents-docs] agents are TOML, not Markdown.** The
  installed file at `.codex/agents/<name>.toml` (or
  `$CODEX_HOME/agents/<name>.toml` globally) is a TOML document, not a
  Markdown file. The canonical agent body lands in the
  `developer_instructions` key; `name`, `description`, and optionally
  `model` are top-level TOML keys. Vendor-namespaced `codex.*` keys in
  `metadata` (e.g., `codex.reasoning-effort`, `codex.sandbox-mode`) lift
  to their native TOML counterparts.

<!-- external -->
[claude-subagents-docs]: https://code.claude.com/docs/en/sub-agents
[opencode-agents-docs]: https://opencode.ai/docs/agents/
[copilot-agents-docs]: https://docs.github.com/en/copilot/concepts/agents/copilot-cli/about-custom-agents
[codex-subagents-docs]: https://developers.openai.com/codex/subagents
[cursor-subagents-docs]: https://cursor.com/docs/context/subagents
[gemini-subagents-docs]: https://geminicli.com/docs/core/subagents
[kiro-docs]: https://kiro.dev
[junie-docs]: https://www.jetbrains.com/junie/
[zed-docs]: https://zed.dev
[amp-docs]: https://ampcode.com
[kiro #8040]: https://github.com/kirodotdev/Kiro/issues/8040

<!-- internal -->
[vendor-metadata]: ./vendor-metadata.md
[vendor-agent-registries]: ./vendor-metadata.md#claude-agent-registry
[vendor-discovery]: ./vendor-metadata.md#discovery-locations
[vendor-drift]: ./vendor-metadata.md#drift
[clients-matrix]: ./clients.md
[stability-unstable]: ./stability.md#unstable
