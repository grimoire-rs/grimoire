# Rule Spec

You loaded this file because you are authoring or fixing a grim rule — a
single Markdown file, optionally with a sibling support directory — for
`grim build` or `grim release`.

Contents: [File Shape](#file-shape) · [Frontmatter](#frontmatter) ·
[The Asymmetry](#the-asymmetry) · [Support Directory](#support-directory) ·
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

## Support Directory

An index rule may carry extra context — examples, schemas, scripts — in
a sibling folder sharing its stem:

```
rules/
  my-rule.md     # the index you pass to build/release
  my-rule/       # optional support dir, same stem — auto-discovered
    examples.md
```

Both pack into one layer and install side by side (`rules/my-rule.md` +
`rules/my-rule/…`), so the index's relative links resolve on the
consumer. Support files are copied verbatim for every client — only the
index is ever transformed ([support directories][support-dir]). The
[well-known assets][well-known] convention applies here too: a
`README.md` or `logo.png`/`logo.svg` inside the support directory is
where catalog UIs look for a readme or icon.

## Per-Client Transforms

The same published rule lands differently per client:

| Client | Transform |
|---|---|
| Claude Code | ~Verbatim — `paths:` is native frontmatter; re-rendered only when `metadata` carries vendor keys |
| OpenCode | Frontmatter **stripped**; body written with a provenance comment; loading registered as a managed glob in `opencode.json` |
| Copilot | Written to `.github/instructions/<name>.instructions.md`; `paths` comma-joined into a single `applyTo:` string; `copilot.exclude-agent` → `excludeAgent` |

OpenCode never sees rule frontmatter at all — anything that must reach
OpenCode belongs in the body. Full mapping: [rule keys][rule-keys].

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
| Frontmatter carries both `name` and `description` | Warning — looks like an agent; did you forget `--kind agent`? |

## Further Reading

- [Rule schema and examples][rules-ref] — the authoritative field table.
- [Catalog metadata for rules][pub-rule] — where summary/keywords go.
- [Rules with a support directory][support-dir] — packing semantics.
- [Rule-level vendor keys][rule-keys] — per-client transform detail.

[rules-ref]: https://grimoire.rs/artifacts.html#rules
[pub-rule]: https://grimoire.rs/publishing.html#metadata-rule
[support-dir]: https://grimoire.rs/publishing.html#rule-support-dir
[rule-keys]: https://grimoire.rs/vendor-metadata.html#rule-keys
[rule-vendor-ex]: https://grimoire.rs/vendor-metadata.html#rule-authoring-example
[common-unique]: https://grimoire.rs/vendor-metadata.html#common-vs-unique
[well-known]: https://grimoire.rs/artifacts.html#well-known-assets
