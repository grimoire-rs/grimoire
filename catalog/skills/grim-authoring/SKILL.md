---
name: grim-authoring
description: Author, validate, and package grim-publishable artifacts — skill directories, rule files, agent definitions, MCP server descriptors, hook manifests, and bundle TOMLs. Use when creating or editing an artifact for grim build or grim release; when choosing frontmatter or catalog metadata fields; when writing a hook.toml with its events, tiers, matchers, and handlers; when adding a vendor-namespaced metadata key for any client grim supports (claude, opencode, copilot, codex, cursor, kiro, junie, gemini, zed, amp, antigravity, cline, droid, goose, warp, openclaw, kilo — one namespace per client name); or when grim build fails validation with exit code 65.
license: Apache-2.0
compatibility: grim>=0.14
metadata:
  summary: Deep authoring guide for grim skill, rule, agent, mcp, hook, and bundle artifacts
  keywords: grim,grimoire,authoring,frontmatter,validation,vendor-metadata,skill,rule,agent,mcp,hook,bundle,packaging
  repository: https://github.com/grimoire-rs/grimoire
---

# Grim Artifact Authoring

Grim publishes six artifact kinds to OCI registries. Each has its own
source shape, frontmatter schema, and validation gates. This root file
holds the invariants that apply to every kind; per-kind depth lives in
`references/`, loaded via the routing table below.

## The Six Kinds

`grim build` and `grim release` infer the kind from the path — except
agents (always `--kind agent`, or they silently pack as rules) and MCP
servers (always `--kind mcp`, or the `.toml` is treated as a bundle).

| Kind | Source shape | Inference | Installs as |
|---|---|---|---|
| Skill | Directory with a `SKILL.md` index | directory → skill | Directory tree under the client's `skills/` dir |
| Rule | Single `.md` file | `.md` → rule | `rules/<name>.md`, per-client transform |
| Rule + support dir | `<name>.md` + sibling `<name>/` dir | sibling dir auto-discovered | Index file + `rules/<name>/…` side by side |
| Agent | Single `.md`, frontmatter required | **never — `--kind agent` mandatory** | One agent file per client, per-client render |
| MCP server | `.toml` descriptor with a `[server]` table | **never — `--kind mcp` mandatory** | Entry in each client's MCP config file, per-client render |
| Hook | Directory with a `hook.toml` manifest + its payload files | directory with `hook.toml` → hook | Payload tree materialized once per scope, one recorded output per client. **Arming is gated and off by default** — see [hook-spec.md](references/hook-spec.md) |
| Bundle | `.toml` member list | `.toml` → bundle | Never materializes — expands to its members |

Two directory kinds share one inference arm, and the order is
load-bearing: a directory carrying `hook.toml` is a **hook** even when a
`SKILL.md` also sits in it. So a stray `SKILL.md` in a hook tree cannot
silently publish the payload as a skill — but a stray `hook.toml` in a
skill tree flips the kind, which is the mistake to watch for.
`--kind hook` is accepted on `build` and `release` but is never *required*
— `grim build ./hooks/shell-guard` already reports `hook`. Pass it anyway
in scripted loops, the way the other kinds are passed, so a manifest
rename cannot silently reclassify the artifact.

## Which Clients Host Which Kind

Grim installs into a growing set of clients, and not every client can
host every kind — decide this **before** you author, because it changes
what you write. Treat the [enforced matrix][clients] as authoritative;
the summary below is a planning aid that the next client can age:

- **Skills** are the universal kind — no client declines them, which is why
  a skill is the portable choice. One scope caveat: OpenClaw is
  global-scope-only, so a *project* install for it writes nothing.
- **Rules** are native for Claude Code, Copilot, Cursor, and Kiro;
  degraded for OpenCode and Junie (the file installs and grim restates the
  scope as prose in the body, but nothing enforces it); and **declined**
  by everyone else — grim warns, skips, and
  writes no file. Most of the fleet cannot scope instructions: when the
  audience is broad, a skill reaches clients a rule never will.
- **Agents** install for Claude Code, OpenCode, Copilot, Codex, Cursor,
  Gemini, and Antigravity. Every other client declines them.
- **MCP servers** register for the clients that ship a config file grim
  can splice — Claude, OpenCode, Copilot, Codex, Cursor, Kiro, Junie,
  Gemini, Zed, Amp, and Antigravity. Only Claude accepts the `ws`
  transport and the `[server.oauth]` block; every other client skips such
  a descriptor with a warning. The skills-only clients (and the
  vendor-neutral `agents` target) write no MCP config at all.
- **Hooks** are the narrowest kind by far. Only three clients name a hook
  registration surface at all — Claude, Codex, and Copilot — and of those
  only **Claude** hosts a hook at *project* scope; Codex and Copilot are
  global-scope-only, because their registration files are tracked
  repository files. Every other client declines hooks outright. On top of
  that, arming is gated (see below), so authoring a hook means accepting
  that most of the fleet will report it and never fire it.

A declined kind is an honest refusal, not a silent failure — but it is
still zero files. The enforced matrix and the upstream reason behind
every degrade and decline: [Client Compatibility][clients]. A
`compatibility:` frontmatter field is a human-facing hint only and never
overrides it.

## Hooks Ship Disarmed

Publishing a hook is not shipping something that runs. Hooks are behind an
experimental flag that is **off by default**:

```sh
grim config get options.experimental.hooks    # prints nothing, exit 1, unless someone set it true
```

The flag is config-only — no environment variable overrides it — and it is
only the **first of two gates** grim keeps deliberately
separate:

1. **Is the feature on?** `options.experimental.hooks`, off by default.
2. **Has this workspace consented?** A machine-local consent record under
   `$GRIM_HOME/hooks/consent/`, one file per workspace, naming the checkout
   by absolute path and listing the hooks it covers as
   `<binding>@<registry>/<repository>`. Global scope is always consented and
   carries no record.

Gate 2 has three satisfiers, and they are the *only* three things that write
a record: `grim hook allow`, `grim add` (typing a reference is the
declaration gesture), or an accepted prompt (asked once per workspace, TTY
only). `grim install`, `update`, `lock`, `status`, `context`, `hook list`,
the TUI and the MCP server never write one — a consumer who clones a
repository declaring your hook gets a declaration and a lock pin, and
nothing armed. `--trust-hooks` on `grim install` / `update` / `add` arms a
single invocation and writes nothing; its `--no-trust-hooks` counterpart
refuses one. Neither turns the feature on — gate 1 must already be open —
and the pair beats the record in **both** directions.

They are **flags only: there is no `GRIM_ALLOW_HOOKS`, no
`GRIM_HOOK_CONSENT`, and no environment form of the record's path**, and
that absence is what makes the pair safe to rank above the record. The
environment is repo-carried — `.envrc`, `.mise.toml` and a devcontainer's
`containerEnv` are ordinary files in a repository — so an environment form
would let a cloned repository consent on its cloner's behalf. A file cannot
type a flag.

One condition sits outside both gates and answers to no file at all: a hook
whose pinned registry host is not loopback and is reached over plain HTTP
never arms, because the first resolution that produces the digest pin is
itself attacker-influenceable on the wire.

Enabling the feature is not consent to run any particular hook.

What this means for you as a publisher: a consumer who runs `grim add` on
your hook gets a real declaration, a real lock pin, and a materialized
payload tree — and by default nothing armed. Write the hook's
`description` so it reads honestly under those terms, and do not document
your hook to consumers as something that fires on install. Consumers
inspect what they have with `grim hook list`.

## Universal Invariants

- Names are `[a-z0-9]` runs joined by single hyphens or periods
  (`[a-z0-9]+([.-][a-z0-9]+)*`) — non-empty, ≤ 64 chars, no leading or
  trailing separator, no adjacent separators (`a--b` and `a..b` are
  invalid). Periods are a grim superset of the Agent Skills standard
  (`[a-z0-9-]`) — prefer hyphens when portability to strict-standard
  tooling matters.
- A skill's `name` must equal its directory name; an agent's `name` must
  equal its file stem. Rule names come from the file stem and obey the
  same character rules. Bundle and MCP names also come from the file stem
  but are not charset-validated at build; bundle *member* names are
  validated against the same rules at resolve time.
- Any violation of the validated names fails `grim build`/`grim release`
  with exit code 65.
- Unknown top-level frontmatter keys are *preserved* round-trip (forward
  compatibility) — never rejected, so a typo'd optional key is silent.

## The Metadata-Location Asymmetry

Where catalog metadata (`summary`, `keywords`, `repository`, `deprecated`,
`replaced-by`, `authors`, `vendor`, `homepage`, `documentation`) is authored
differs by kind. This is the #1 authoring confusion — misplaced keys are not
errors, they just silently never reach the catalog:

| Kind | Catalog metadata keys live… |
|---|---|
| Skill | inside the `metadata:` map of `SKILL.md` frontmatter |
| Agent | inside the `metadata:` map of the agent frontmatter |
| Rule | at the **top level** of the rule frontmatter (not in `metadata`) |
| MCP server | as **top-level TOML keys**, above the `[server]` table (`replaced-by` not read for MCP) |
| Hook | **nowhere — a hook has no catalog metadata surface.** `hook.toml` is strict (`deny_unknown_fields`): its only top-level keys are `schema`, `name`, `description`, and `[[hooks]]`, so `summary`, `keywords`, `repository`, `deprecated`, and `replaced-by` are each a hard parse error (exit 65), not silent loss. `description` is the one catalog-facing field. Write it to carry the whole blurb — `grim search` has nothing else to show |
| Bundle | as **top-level TOML keys**, above the member tables |

In every kind, `keywords` is one comma-separated string and `repository`
must be an `https://` URL (anything else fails the release with 65). The
`deprecated` notice obeys the same per-kind location; an
empty or whitespace-only value means *not* deprecated and emits no
annotation. `replaced-by` names the successor artifact, authored
independently of `deprecated`; its value must parse as a reference or the
release fails with 65 — detail in [Publishing][publishing].

`vendor` / `homepage` / `documentation` are derived when omitted — vendor
from the release repository's namespace, homepage from `repository`,
documentation from `<repository>#readme`. `authors` is **not**: the only
automatic source is the commit author under `--git`, which publishes a
person's name, so author a team name or alias instead — a manifest is
readable by anyone who can pull the artifact. Optional in the schema does
not mean optional in practice: the set to write on every artifact is
[the default six](references/release-checklist.md#metadata-defaults). A
skill's top-level `compatibility` is published too, as
`com.grimoire.compatibility`.

Repository-level support channels (`issues` / `chat` / `contact` /
`security`) are **not** artifact metadata — they are authored as
a manifest-level `[support]` table in `publish.toml` and ride the mutable
description companion, so changing a link needs no re-release. The table fans
out to every companion the run pushes; there is no per-entry override and no
`grim release` flag.

## Companion: Content Craft

This skill covers grim **packaging and validation** only — including build
provenance, which is embedded by default at build/release time (`--git`
additionally discloses the `origin` remote and commit author; `--no-git`
suppresses everything derived); confirm flags with `grim release --help`. For the craft of
the content itself — progressive disclosure, context budgets, description
triggering, choosing skill vs rule vs agent — read the companion skill
`ai-config-authoring` at
[`../ai-config-authoring/SKILL.md`](../ai-config-authoring/SKILL.md);
both ship together in the `grim-essentials` bundle. When creating a new
artifact from scratch, read it FIRST — write good content, then package
it here. If that file is missing, install it by identifier:

```sh
grim add ghcr.io/grimoire-rs/skills/ai-config-authoring:0   # installs by default
# fresh project (no grimoire.toml yet): run `grim init` first
```

## The Local Dev Loop

Iterate on an artifact **before** its first release with local path
sources — no registry round-trip:

- `grim install <path>` — **dev-install**: renders the working tree into
  the clients without declaring anything (`grimoire.toml` and
  `grimoire.lock` stay untouched). The record is marked `dev` in
  `grim status`, refreshed by `grim update`, removed by `grim uninstall`.
- `grim add <path>` — declares the local path in the config and pins it
  by content hash, like any other source. Re-adding over an output you
  hand-edited in a client is refused as modified; `grim add <path>
  --force` is the sanctioned overwrite.

A path is anything starting `./` or `../`, or absolute. Both commands
cover **skills, rules, and agents** only; kind is inferred from the
path's shape exactly as `grim build` infers it (directory → skill, bare
`.md` → rule, `--kind agent` for agents). A local *bundle* is declared
directly in the config's `[bundles]` table instead (`grim add --kind
bundle <path>` refuses with a hint); its members must be registry
references — a local bundle has no registry identity to resolve a
relative member against. Typical loop: edit → `grim build <path>`
(validation) → `grim install <path>` (see it in a real client) →
repeat → release. Confirm flags with `grim install --help`.

**A hook has no local-path loop at all**, and this is deliberate rather
than unfinished: the config parser rejects a path value under `[hooks]`,
and there is no dev-install for the kind. A dev-install's source is a
working-tree path, so the natural "edit the hook in the repo, re-install"
loop would put something armable inside a repository — exactly what grim
refuses to do. The hook loop is therefore `grim build` to validate, then
`grim release` (a local registry works fine for iterating), then
`grim add` from that reference.

## Routing Table

| Read… | …when |
|---|---|
| [references/skill-spec.md](references/skill-spec.md) | Authoring a skill directory or its `SKILL.md` frontmatter |
| [references/rule-spec.md](references/rule-spec.md) | Authoring a rule file, its globs, or a support directory |
| [references/agent-spec.md](references/agent-spec.md) | Authoring an agent definition or its vendor overrides |
| [references/mcp-spec.md](references/mcp-spec.md) | Authoring an MCP server descriptor or its env references |
| [references/hook-spec.md](references/hook-spec.md) | Authoring a `hook.toml`, choosing an event and tier, writing a matcher, or fixing the payload-not-executable refusal |
| [references/bundle-spec.md](references/bundle-spec.md) | Authoring a bundle TOML or choosing pinning strategy |
| [references/vendor-metadata.md](references/vendor-metadata.md) | Adding a key in a reserved `<vendor>.*` namespace — one per client name (`claude.*`, `opencode.*`, `copilot.*`, `codex.*`, `cursor.*`, `kiro.*`, `junie.*`, `gemini.*`, `zed.*`, `amp.*`, `antigravity.*`, `cline.*`, `droid.*`, `goose.*`, `warp.*`, `openclaw.*`, `kilo.*`) |
| [references/release-checklist.md](references/release-checklist.md) | Before `grim release`/`grim publish`, the metadata every package should set, repository-path layout, batch manifests, description companions, or triaging an exit-65 failure |
| [references/bootstrap-existing-repo.md](references/bootstrap-existing-repo.md) | Turning an existing skill repo (agentskills.io `skills/<name>/SKILL.md` or `.claude/skills/`) into a grim publisher — inventorying artifacts, fixing names, backfilling catalog metadata, wiring publish CI |
| [references/updating.md](references/updating.md) | Maintaining this skill package itself |

## Schema Authority

This skill teaches the craft and the pitfalls; the authoritative schema
reference is the Grimoire docs site. When a field table here feels
incomplete, the docs page is the source of truth:
[Artifact Reference][artifacts] · [Vendor-Specific Metadata][vendor] ·
[Publishing][publishing] · [Agent Artifacts][agents] ·
[Client Compatibility][clients]. For the TOML
surfaces, `grim schema --kind <config|publish|lock|mcp|hook>` prints the
JSON Schema generated from grim's own parsers — bind it in your editor to
catch manifest typos before any command runs. `--kind hook` is the fastest
way to read the authoritative `hook.toml` shape, because the schema
carries grim's own doc comments for every event, tier, and field.

## Verify Before Acting

`grim build <path>` validates without pushing — run it after every edit;
its output is ground truth for the grim version actually installed. On
any conflict between this skill and `grim build` output or `grim --help`,
trust the tool. Treat this skill as the map, not the territory.

---

Verified against the grim release this package ships beside.

[artifacts]: https://grimoire.rs/artifacts.html
[vendor]: https://grimoire.rs/vendor-metadata.html
[publishing]: https://grimoire.rs/publishing.html
[agents]: https://grimoire.rs/agents.html
[clients]: https://grimoire.rs/clients.html
