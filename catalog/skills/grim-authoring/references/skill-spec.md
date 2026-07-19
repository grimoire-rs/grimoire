# Skill Spec

You loaded this file because you are authoring or fixing a grim skill —
a directory with a `SKILL.md` index — for `grim build` or `grim release`.

Contents: [Directory Shape](#directory-shape) · [Frontmatter](#frontmatter) ·
[Catalog Metadata](#catalog-metadata) · [Bundled Directories](#bundled-directories) ·
[Client-Agnostic Content](#client-agnostic-content) · [Companion References](#companion-references) · [Minimal Example](#minimal-example) ·
[Validation Pitfalls](#validation-pitfalls)

## Directory Shape

A skill is a directory whose entrypoint is `SKILL.md`. Everything else in
the tree (scripts, templates, references) packs into one tar layer and
installs **verbatim**. Only `SKILL.md` itself is ever re-rendered per
client, and only when its `metadata` map carries vendor-namespaced keys —
a plain skill installs byte-identical to what you published.

The directory name is the skill's identity: `name` in frontmatter must
equal it exactly, subject to the universal name rules.

## Frontmatter

YAML between `---` fences at the top of `SKILL.md`; the fence is
mandatory for skills. The schema follows the [agentskills
specification][agentskills]; unknown top-level keys are preserved
round-trip, never rejected.

| Field | Required | Notes |
|---|---|---|
| `name` | yes | Must equal the directory name |
| `description` | yes | Non-empty, ≤ 1024 chars; what it does + when to use it |
| `license` | no | SPDX-style id (e.g. `Apache-2.0`); becomes the OCI license annotation |
| `compatibility` | no | Free-text environment hint (e.g. `grim>=0.4`) |
| `allowed-tools` | no | Comma-separated tool allowlist |
| `metadata` | no | String→string map: catalog keys + vendor extensions |

All `metadata` values are strings — quote anything that YAML would
otherwise type (`"true"`, `"0.2"`).

The top-level `allowed-tools` is the common field every client sees
verbatim. Claude additionally recognizes `claude.allowed-tools` /
`claude.disallowed-tools` inside `metadata` (`disallowed-tools` has no
common top-level field at all — vendor-only). Unlike the agent
`model`/`tools` override, this is **not** a silent escape hatch: setting
both the top-level `allowed-tools` and `claude.allowed-tools` warns on the
collision (metadata wins) — author only the vendor key for a Claude-only
value, skip the top-level field to avoid the redundant warning.

## Catalog Metadata

Skills author catalog metadata **inside the `metadata` map** (unlike
rules and bundles, where these keys are top-level):

| Key | Constraint |
|---|---|
| `metadata.summary` | One-line blurb; shown by `grim search` instead of the description |
| `metadata.keywords` | One comma-separated string — `review,quality`, never a YAML list |
| `metadata.repository` | `https://` URL only; `git@…` or `http://` fails the release (exit 65) |
| `metadata.deprecated` | Deprecation notice; non-empty marks the skill deprecated (flagged in search/TUI, warned on `add`). Empty ⇒ not deprecated |
| `metadata.replaced-by` | Successor reference (independent of `deprecated`); surfaced in search / `grim describe`. Must parse as a reference or the release fails (exit 65) |

Full annotation mapping: [catalog metadata][pub-metadata] and
[annotations][annotations].

## Bundled Directories

Follow the agentskills conventions so consumers' agents navigate the
package predictably:

- `references/` — docs read on demand; one focused topic per file.
- `scripts/` — executable code, run rather than read.
- `assets/` — static resources used in output (templates, schemas).

Keep `SKILL.md` an index that routes into these; relative links from the
skill root, forward slashes only.

Two **well-known assets** beside `SKILL.md` ride the tree at no extra
cost: `README.md` (human-facing readme) and `logo.png`/`logo.svg` (a
catalog/gallery icon). They are ordinary tree files — no frontmatter, no
schema meaning — that catalog UIs look for by convention and consumers
can pull with `grim fetch <ref> --path README.md`
([well-known assets][well-known]). For a readme at the *repository*
level (uniform across every kind, not installed with the tree), publish
a description companion instead — see
[release-checklist.md](release-checklist.md#description-companion).

## Client-Agnostic Content

One published skill serves Claude Code, OpenCode, and Copilot. The
OpenCode and Copilot skill registries are intentionally empty — both get
the identical universal render; only `claude.*` skill keys exist today
([empty registries][empty-registries]). Write the body client-neutrally:
never assume one client's tool names or directory layout; put
Claude-only behavior in `claude.*` keys, not prose.

## Companion References

When a skill depends on knowledge in another skill, reference the
companion three ways so any agent can resolve it: by **name**, by
**relative sibling path** (skills install side by side under the
client's `skills/` dir, so `../<name>/SKILL.md` resolves in every
client), and by **fully-qualified grim identifier** as the install
fallback:

```markdown
Read the companion skill `other-skill` at `../other-skill/SKILL.md`.
If that file is missing, install it:
`grim add registry.example.com/skills/other-skill:1` (installs by default)
```

Pin the fallback to a floating major tag (`:1`) so consumers get fixes
without the referencing skill needing a re-release. Shipping companions
together in a bundle makes co-presence the default; the identifier
covers standalone installs.

## Minimal Example

The smallest valid skill — a directory `hello-world/` containing:

```yaml
# hello-world/SKILL.md
---
name: hello-world
description: A minimal smoke-test skill that prints a greeting.
---

# Hello World

Say hello.
```

## Validation Pitfalls

All hard errors exit 65 (DataError) at `grim build` / `grim release`.

| Pitfall | Outcome |
|---|---|
| Directory has no `SKILL.md` | Hard error — missing index |
| No leading `---` fence, or fence never closed | Hard error — missing frontmatter |
| Malformed YAML; `name` or `description` absent | Hard error — frontmatter parse |
| Frontmatter `name` ≠ directory name | Hard error — name mismatch |
| Directory name violates the name charset | Hard error — invalid name |
| `description` empty/whitespace or > 1024 chars | Hard error — rejected at parse |
| `metadata.repository` not `https://` | Hard error — invalid repository URL |
| `keywords` written as a YAML list | Not accepted — must be one comma string |
| Known `claude.*` key with a bad literal (`claude.effort: extreme`) | Hard error — publish stops ([projection][projection]) |
| Typo'd own-namespace key (`claude.efort`) | **Warning + dropped** — silent loss if ignored |
| Any `opencode.*` / `copilot.*` key on a skill | Always unknown → warning + dropped |
| Legacy Claude field at top level (`user-invocable: true`) | Warning only — installs verbatim; migrate to `claude.user-invocable` ([migration][migration]) |
| `summary`/`keywords` at top level (rule-style) | No error — preserved as unknown keys, but the catalog never sees them |

## Further Reading

- [Skill schema and examples][skills-ref] — the authoritative field table.
- [Full skill example][skill-full] — every field in use.
- [Names][names] — the exact character rules.
- [Catalog metadata for skills][pub-metadata] — annotation mapping.
- [Vendor extensions][vendor-ext] — how `claude.*` keys project.
- [agentskills specification][agentskills] — the upstream open standard.

[skills-ref]: https://grimoire.rs/artifacts.html#skills
[names]: https://grimoire.rs/artifacts.html#names
[skill-full]: https://grimoire.rs/artifacts.html#skill-example-full
[pub-metadata]: https://grimoire.rs/publishing.html#metadata-skill
[annotations]: https://grimoire.rs/artifacts.html#annotations
[vendor-ext]: https://grimoire.rs/artifacts.html#vendor-extensions
[empty-registries]: https://grimoire.rs/vendor-metadata.html#empty-registries
[well-known]: https://grimoire.rs/artifacts.html#well-known-assets
[projection]: https://grimoire.rs/vendor-metadata.html#projection-semantics
[migration]: https://grimoire.rs/vendor-metadata.html#migration
[agentskills]: https://agentskills.io/specification
