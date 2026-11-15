# Rule Spec

You loaded this file because you are authoring or fixing a grim rule — a
single Markdown file, never with a support directory — for
`grim build` or `grim release`.

Contents: [File Shape](#file-shape) · [Frontmatter](#frontmatter) ·
[The Asymmetry](#the-asymmetry) · [Support Directory (anti-pattern)](#support-directory) ·
[Per-Client Transforms](#per-client-transforms) · [Examples](#examples) ·
[Validation Pitfalls](#validation-pitfalls)

## File Shape

A rule is one `.md` file. Frontmatter is **entirely optional** — a bare
Markdown file with no `---` fence is a valid rule whose body is the
whole document. The rule's name is the file stem, subject to the
universal name rules.

Rules have no `description` field. When grim needs one for the catalog,
it derives it from the first Markdown heading or first non-empty line —
so make the opening line carry meaning.

## Frontmatter

When present, all fields are optional; unknown keys are preserved
round-trip.

| Field | Type | Notes |
|---|---|---|
| `paths` | list of strings | Glob patterns the rule auto-loads on; absent = always active |
| `summary` | string | **Top-level** — catalog blurb for `grim search` |
| `keywords` | string | **Top-level** — comma-separated tags (a YAML list is tolerated and comma-joined, but write the string form: it is the only shape valid in every kind) |
| `license` | string | **Top-level** — SPDX-style id (e.g. `Apache-2.0`); becomes the OCI license annotation |
| `authors` / `vendor` / `homepage` / `documentation` | string | **Top-level** — provenance and docs metadata. `vendor`/`homepage`/`documentation` are derived when omitted; `authors` never is, so author a team alias (see [catalog metadata][pub-metadata] and [the default set](./release-checklist.md#metadata-defaults)) |
| `repository` | string | **Top-level** — `https://` source URL, hard-gated at release |
| `deprecated` | string | **Top-level** — deprecation notice; non-empty marks the rule deprecated (flagged in search/TUI, warned on `add`) |
| `replaced-by` | string | **Top-level** — successor reference (independent of `deprecated`); surfaced in search / `grim describe`. Must parse as a reference or the release fails (exit 65) |
| `metadata` | string→string map | Vendor extensions only (e.g. `copilot.exclude-agent`) |

## The Asymmetry

Skills and agents put `summary`/`keywords`/`repository` inside their
`metadata` map. Rules put them at the **top level** of frontmatter;
`metadata` holds *only* vendor-namespaced keys. Swapping the conventions
is never an error — the keys are preserved as plain/unknown data and the
catalog silently never sees them. Check this first when a published rule
shows no summary in `grim search`.

`paths` is also top-level: it is a *common* capability every client
understands, not a vendor key ([common vs. unique][common-unique]).

## Support Directory {#support-directory}

**Anti-pattern: never ship depth files in a directory beside a rule.**
Clients that read `rules/` recursively load every file in
`rules/<name>/` as a rule of its own. grim's repair was Claude-only (a
`claudeMdExcludes` entry in one `settings.json`, ignored when the
repository is loaded from another directory), Qoder has no usable exclude
key, and other clients are unaudited. So grim 0.16.0 and later packs a
rule as its `.md` alone. The directory beside it (`rules/<name>/`) may hold
only `README.md`, `logo.png` and `logo.svg`, and none of them is packed;
any other file refuses the build (exit 65, `a rule can no longer ship a
support directory`). That applies to `grim build`, `release`, `publish` and
to a rule declared by local path. No `.grimignore` is read there; one in the
directory is itself a stray file. Only built-in junk such as `.DS_Store` is
skipped. Rules published before 0.16.0 still install; grim warns once per
such rule when it writes files.

The replacement is a **support skill**: put the depth files in a skill,
require it from the rule, and name it in the rule body so the agent loads it.

```
rules/
  architecture-guide.md          # the rule: short, always loaded
skills/
  architecture-guide/            # the depth files, loaded on demand
    SKILL.md
    patterns.md
```

```yaml
# rules/architecture-guide.md
---
paths:
  - "src/**"
metadata:
  grim.requires.skills.architecture-guide: "ghcr.io/acme/skills/architecture-guide:1"
---

# Architecture

Layering rules in two lines. For the full pattern catalog, load the
`architecture-guide` skill.
```

Rules for the support skill:

- It stays **model-invocable**. On Claude, never set
  `claude.disable-model-invocation: "true"` on it: its description leaves
  context and Claude cannot load it. It may set
  `claude.user-invocable: "false"` to hide it from the `/` menu while Claude
  can still load it. No other client has a skill-level key in grim, so there
  the skill's description and the explicit name in the rule body carry it.
- Keep its description short and specific: every skill description is loaded
  into context at session start.
- A rule's own README and logo may sit in `rules/<name>/` or beside it as
  `<name>.README.md`, `<name>.png`; they feed the catalog description only
  ([lookup](release-checklist.md#description-companion)).

Designing the split (what stays in the rule, what moves to the skill):
see the `ai-config-authoring` skill, `references/rule-design.md`. Vendor
options for the support skill: [vendor-metadata.md](vendor-metadata.md#support-skill).

## Per-Client Transforms

Rules reach the fewest clients of any kind — a minority host them at all
and everyone else declines outright, which is why a skill is the portable
choice when the audience is broad ([client matrix][clients]). The same
published rule lands differently per client:

| Client | Transform |
|---|---|
| Claude Code | ~Verbatim — `paths:` is native frontmatter; re-rendered only when `metadata` carries vendor keys |
| Copilot | Written to `.github/instructions/<name>.instructions.md` at project scope (global scope lands in native `~/.copilot/instructions/`); `paths` comma-joined into a single `applyTo:` string; `copilot.exclude-agent` → `excludeAgent` |
| Cursor | Written to `.cursor/rules/<name>.mdc`; `paths` comma-joined into a single `globs` string plus a computed `alwaysApply: false` — unscoped emits no `globs` and `alwaysApply: true` |
| Antigravity | Written to `.agents/rules/<name>.md` (global `~/.gemini/config/rules/<name>.md`); `paths` comma-joined into a single `globs` string plus `trigger: glob` — unscoped emits `trigger: always_on`. A `description` is written from the rule's first heading or line |
| Kiro | Written to `.kiro/steering/<name>.md`; `paths` become a `fileMatchPattern` YAML **array** (not comma-joined) plus `inclusion: fileMatch` — unscoped emits `inclusion: always`. Global-scope scoping is upstream-inert today; grim writes the correct file and warns |
| OpenCode | Frontmatter **stripped** and `paths` dropped with a warning; body written with a provenance comment, preceded by a one-line prose notice restating the dropped scope (`> Applies only when working on files matching …`) so the model still gates on it; loading registered as a managed glob in `opencode.json` |
| Junie | **Degraded, project scope only.** Written to `.junie/rules/<name>.md` as provenance + the same prose scope notice OpenCode gets + body, with `paths` dropped and a warning — the directory is ownable, but every Markdown file in it is concatenated automatically with no per-file activation key. At global scope grim warns, skips, and writes nothing: no `~/.junie/rules/` exists upstream |
| Codex · Gemini · Zed · Amp · everyone else | **Declined** — no ownable path-scoped surface (always-on `AGENTS.md`/`GEMINI.md` hierarchies, or a UI-managed surface with no on-disk path); grim warns, skips, and writes no file |

OpenCode never sees rule frontmatter at all — anything that must reach
OpenCode belongs in the body. Two authoring consequences of the rest:
Cursor and Antigravity split their `globs` string on **every** comma, including one inside
a `{a,b}` brace alternation, so author `src/**/*.rs` and `src/**/*.toml`
as two patterns rather than `src/**/*.{rs,toml}`; and when the audience
is broad, a skill reaches every client a rule cannot — most of the fleet.
Full mapping:
[rule keys][rule-keys] · [client matrix and gaps][clients].

## Examples

Bare — no fence at all, still valid:

```markdown
# commit-style.md
Use Conventional Commits. Subject ≤ 50 characters.
```

Full — scoped, catalog-visible, with a vendor key:

```yaml
# rust-style.md
---
paths:
  - "**/*.rs"
summary: Idiomatic Rust style rules
keywords: rust,style,lints
repository: https://github.com/acme/rust-style
metadata:
  copilot.exclude-agent: code-review
---

# Rust Style

Prefer `&str` over `String` parameters...
```

## Validation Pitfalls

| Pitfall | Outcome |
|---|---|
| File stem violates name rules (`Bad_Name.md`) | Hard error, exit 65 — invalid name |
| `---` fence present but YAML malformed | Hard error, exit 65 — frontmatter parse |
| `repository` not `https://` | Hard error, exit 65 |
| Vendor key authored top-level (`copilot.exclude-agent:` outside `metadata`) | **Warning** — key is not projected ([example][rule-vendor-ex]) |
| `copilot.exclude-agent` outside `code-review`/`cloud-agent` | Hard error, exit 65 |
| `summary`/`keywords` placed inside `metadata` (skill-style) | No error — catalog silently shows nothing |
| A file other than `README.md`/`logo.png`/`logo.svg` in `rules/<name>/` | Hard error, exit 65 (since 0.16.0) — move it into a required support skill |
| Frontmatter carries both `name` and `description` | Warning — looks like an agent; did you forget `--kind agent`? |

## Further Reading

- [Rule schema and examples][rules-ref] — the authoritative field table.
- [Catalog metadata for rules][pub-rule] — where summary/keywords go.
- [Rule support directory removal][support-dir] — the 0.16.0 refusal and
  the upgrade path.
- [Claude support-directory exclusion][claude-excl] — what grim still
  writes for rules published before 0.16.0.
- [Rule-level vendor keys][rule-keys] — per-client transform detail.
- [Client compatibility][clients] — which clients host rules, and why the
  rest decline.

[rules-ref]: https://grimoire.rs/artifacts.html#rules
[pub-rule]: https://grimoire.rs/publishing.html#metadata-rule
[support-dir]: https://grimoire.rs/upgrading.html#rule-support-dir
[claude-excl]: https://grimoire.rs/vendor-metadata.html#claude-md-excludes
[rule-keys]: https://grimoire.rs/vendor-metadata.html#rule-keys
[clients]: https://grimoire.rs/clients.html
[rule-vendor-ex]: https://grimoire.rs/vendor-metadata.html#rule-authoring-example
[common-unique]: https://grimoire.rs/vendor-metadata.html#common-vs-unique
