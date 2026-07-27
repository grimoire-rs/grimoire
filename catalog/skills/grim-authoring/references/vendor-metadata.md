# Vendor Metadata

You loaded this file because you are adding a key in a reserved vendor
namespace — `claude.*`, `opencode.*`, `copilot.*`, `codex.*`, `cursor.*`,
`kiro.*`, `junie.*`, `gemini.*`, `zed.*`, `amp.*`, `antigravity.*`,
`cline.*`, `droid.*`, `goose.*`, `warp.*`, `openclaw.*`, `kilo.*` — to an
artifact, or a publish failed or warned on a vendor key.

Contents: [Mental Model](#mental-model) · [Outcome Classes](#outcome-classes) ·
[Literal Discipline](#literal-discipline) ·
[Where the Registries Live](#where-the-registries-live) ·
[Worked Example](#worked-example) · [Legacy Migration](#legacy-migration)

## Mental Model

A published artifact stays spec-compliant: client-specific capabilities
are authored as **string-valued** `<vendor>.<field>` keys inside the
artifact's `metadata` map. At install time grim looks each key up in the
target vendor's registry and **projects** it — converts the string to
its native type and lifts it into top-level frontmatter of the written
file. Each client sees only its own namespace; one canonical file serves
all clients. The recognized namespaces are `claude`, `opencode`,
`copilot`, `codex`, `cursor`, `gemini`, `kiro`, `junie`, `zed`, `amp`,
`antigravity`, `cline`, `droid`, `goose`, `warp`, `openclaw`, and `kilo` —
**one per client name**, derived from every registered `ClientTarget`, so
the list grows every time grim adds a client ([canonical
list][projection]). The vendor-neutral `agents` target is the one
exception: it owns no namespace, because `agents.*` is an ordinary word
that plain metadata legitimately uses. Any prefix outside the reserved
set (e.g. `vendor.x`) is plain metadata and passes through untouched.

**Most of those namespaces carry no populated field registry.** Only
Claude has a *skill* registry; Claude, OpenCode, Copilot, Codex, Cursor,
and Gemini have an *agent* registry. Everyone else installs the universal
agentskills shape — but the namespace is still reserved, so a key using
one of those prefixes hits an **empty** registry and is **warned +
dropped**, the same typo-guard outcome as an unknown key in a populated
namespace, never a silent passthrough. The counter-intuitive consequence
worth internalizing: `goose.foo` on a skill is dropped **even when Goose
is the target**, because Goose's own registry is empty.

Reservation is retroactive by design: a `codex.*` key authored before
Codex client support landed was plain passthrough metadata, and today it
is a tool-namespaced key subject to the same known/unknown handling as
the others. The same happened to `antigravity.*`, `cline.*`, `droid.*`,
`goose.*`, `warp.*`, `openclaw.*` and `kilo.*` when those clients landed,
and it will happen to the next client's prefix. Do not use a client name
as a plain metadata prefix.

Note also that not every client hosts every kind — most decline rules,
most decline agents, and the skills-only clients write no MCP config at
all — and grim warns and skips a kind a client cannot host. The enforced
matrix is authoritative ([client matrix][clients]).

## Outcome Classes

Every vendor key lands in exactly one of these classes — memorize them,
they explain every vendor-metadata surprise:

| Input | Outcome |
|---|---|
| Known key, valid literal | Projected: converted to native type, lifted to top-level frontmatter |
| Known key, **bad literal** | **Hard error** — publish fails exit 65; install fails MaterializeFailed |
| Unknown key in your **own** namespace (typo: `claude.efort`) | Warning + dropped — the typo guard; silent data loss if the warning is ignored |
| Key in a **foreign** namespace (e.g. `opencode.*` rendering for Claude) | Dropped silently — by design, that is multi-client serving |

Two corollaries: **every** *skill* registry except Claude's is empty, so a
namespaced key on a skill is always unknown → warn + drop, whichever
client you are targeting. And when a namespaced key collides with a
same-named top-level field, the namespaced key wins — with a warning in the
legacy-migration case, silently for the agent `model`/`tools` override
escape hatch.

## Literal Discipline

All `metadata` values are strings; grim converts at install time. The
conversion is what fails publishes:

- **bool** — exactly `"true"` or `"false"`, quoted.
- **enum** — a closed set per key (e.g. `claude.effort` accepts
  `low|medium|high|xhigh|max`); anything else is exit 65.
- **integer** — base-10 digits only, quoted (`claude.max-turns: "20"`).
- **float** — any finite float, quoted (`opencode.temperature: "0.2"`).
- **comma list / string** — never fail.

Object-valued native fields (Claude's `hooks`/`mcpServers`, OpenCode's
`permission`, Copilot's `mcp-servers`) cannot be expressed as a string
and are **not authorable at all**.

## Where the Registries Live

Do not work from memory — the key registries are versioned with grim and
grow over time. The authoritative tables:

- [`claude.*` skill registry][claude-reg]
- [`claude.*` agent registry][claude-agent-reg]
- [`opencode.*` agent registry][opencode-agent-reg]
- [`copilot.*` agent registry][copilot-agent-reg]
- [`codex.*` agent registry][codex-agent-reg] (`codex.model`, `codex.reasoning-effort`, `codex.sandbox-mode`)
- [`cursor.*` agent registry][cursor-agent-reg] (`cursor.model`, `cursor.readonly`, `cursor.is-background`)
- [`gemini.*` agent registry][gemini-agent-reg] (`gemini.model`, `gemini.temperature`, `gemini.max-turns`, `gemini.timeout-mins`, `gemini.kind`)
- [Rule-level keys][rule-keys] (today: `copilot.exclude-agent` only)
- [Empty skill registries][empty-reg] — every client but Claude; a namespaced skill key always warns and drops

## Worked Example

```yaml
---
name: deep-review
description: A thorough security and correctness review.
metadata:
  claude.user-invocable: "true"
  claude.effort: "high"
---
```

Installed for Claude Code, the written `SKILL.md` carries native typed
frontmatter: `user-invocable: true` (a YAML bool) and `effort: high`;
for OpenCode or Copilot both keys drop and the render is universal.
`grim build` runs this projection for *every* supported client before
anything publishes, printing the full union of warnings — errors are
caught at your desk, not the consumer's ([validation][publish-val]).

## Legacy Migration

A pre-grim `SKILL.md` may carry Claude fields as top-level keys
(`user-invocable: true`). That installs verbatim — no breakage — but
build/release warn per key; move each into `metadata` under `claude.*`
to silence the nudge and gain type conversion ([migration][migration]).

## Further Reading

- [Why tool keys live in metadata][why] — the design rationale.
- [Projection semantics][projection] — the full outcome table.
- [Publish-time validation][publish-val] — when the gate runs.

[why]: https://grimoire.rs/vendor-metadata.html#why-metadata
[projection]: https://grimoire.rs/vendor-metadata.html#projection-semantics
[clients]: https://grimoire.rs/clients.html#matrix
[claude-reg]: https://grimoire.rs/vendor-metadata.html#claude-registry
[claude-agent-reg]: https://grimoire.rs/vendor-metadata.html#claude-agent-registry
[opencode-agent-reg]: https://grimoire.rs/vendor-metadata.html#opencode-agent-registry
[copilot-agent-reg]: https://grimoire.rs/vendor-metadata.html#copilot-agent-registry
[codex-agent-reg]: https://grimoire.rs/vendor-metadata.html#codex-agent-registry
[cursor-agent-reg]: https://grimoire.rs/vendor-metadata.html#cursor-agent-registry
[gemini-agent-reg]: https://grimoire.rs/vendor-metadata.html#gemini-agent-registry
[rule-keys]: https://grimoire.rs/vendor-metadata.html#rule-keys
[empty-reg]: https://grimoire.rs/vendor-metadata.html#empty-registries
[publish-val]: https://grimoire.rs/vendor-metadata.html#publish-validation
[migration]: https://grimoire.rs/vendor-metadata.html#migration
