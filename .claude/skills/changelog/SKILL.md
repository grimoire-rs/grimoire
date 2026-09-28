---
name: changelog
description: Use when a release is being prepared and the site changelog page needs the new version, after `task release:prepare` regenerated CHANGELOG.md, or when the user asks to update the changelog page, write release notes for the site, or backfill a version on `docs/src/content/docs/changelog.md`. User-facing write-up per release — features by outcome with doc links, not the commit list.
user-invocable: true
argument-hint: "[version, e.g. 0.15.0 — default: Cargo.toml version]"
triggers:
  - "changelog page"
  - "site changelog"
  - "update the changelog"
  - "release notes"
---

# /changelog — Site Changelog for a Release

`CHANGELOG.md` is git-cliff's commit-level record. The site page
`docs/src/content/docs/changelog.md` (served at `/changelog.html`) is the
user-facing one: what a user can do now that they could not before, why it
matters, and which doc page shows how. This skill adds one release to it.

Runs in the release ceremony between `task release:prepare` and the
`release: vX.Y.Z` commit ([workflow-release.md](../../rules/workflow-release.md)).
An AI performing a release runs it too — the release is incomplete without it.

Before writing, load the `docs` skill, `.claude/rules/docs-style.md` and
`.claude/rules/product-context.md` for voice and link conventions.

## Page contract

Newest first. `page_type.py` requires every `###` inside a version section
to start with a version number, so feature headings are `####`.

```markdown
## 0.15 — <theme of the minor, user words> {#v0-15}

<2–3 sentences: who benefits and what changed for them, linking the
headline `####` sections across its releases.>

### 0.15.1 — 2026-10-12 {#v0-15-1}

<Highlights: 1–2 sentences naming this release's changes, each linked to
its `####` below.>

#### <Feature as the user's gain> {#v0-15-1-<slug>}

<Problem the user had → what they can do now → one short command or
config example → link to the guide or reference.>

#### Smaller improvements {#v0-15-1-more}

- <outcome bullet> — [doc page][ref].
```

- **Every version (`###`)** opens with a highlights sentence or two that
  links each `####` in it. No release body starts with a heading or a list.
- **One `####` per notable change**: a feature, a new client, a TUI
  change, a breaking change (heading says so, body gives the migration).
  Never let a bullet list follow a feature's body directly — it reads as
  that feature's sub-items. Loose items go under a closing
  `#### Smaller improvements` (and `#### Fixes` when fixes are worth
  listing on their own).
- **Minor (x.y.0)** — new `##` section above the previous minor, plus
  `### x.y.0`.
- **Patch (x.y.z)** — new `###` directly under the minor's theme paragraph
  (patches newest first, above `x.y.0`). A patch with nothing notable is
  highlights plus `#### Smaller improvements` only. Revisit the minor's
  theme paragraph when the patch shifts it.
- Heading anchors: `{#vX-Y}`, `{#vX-Y-Z}`, `{#vX-Y-Z-<slug>}` — never
  change an anchor once published.
- Date = the version heading date in `CHANGELOG.md`.

## Procedure

1. **Version.** `$ARGUMENTS`, else `[package] version` in `Cargo.toml`.
   Never derive it from git tags. Stop if the page already has that
   `### x.y.z` heading.
2. **Source.** Read that version's section in `CHANGELOG.md`
   (`git diff CHANGELOG.md` right after `release:prepare`).
3. **Filter.** Keep what a user notices: new commands, flags, clients,
   config keys, behaviour changes, fixes to visible bugs. Drop
   `Changed` refactors, CI, tests, internal docs, AI-config work. When
   unsure what an entry does, read the commit (`git log --grep`,
   `git show`) or its PR (`gh pr view`).
4. **Link.** For every kept item, grep `docs/src/content/docs/` for the
   page and heading anchor that documents it. Behaviour changes that need
   action also link the matching anchor in
   `docs/src/content/docs/upgrading.md`. An item with no
   doc page is a docs gap — report it, still write the entry.
5. **Write.** Group related entries into one feature. Per feature:
   problem → capability → example → link. User's words, not commit
   subjects (DOC-TYPE-38). No hype adjectives; the example makes the case.
   Reference-style links only, definitions at the bottom of the page.
6. **Gate.** Run `/docs-review` on the page, then
   `python3 .claude/rules/docs-quality/checks/page_type.py docs/src/content/docs/changelog.md`
   and `task docs:check`. Fix every finding.
7. **Hand off.** Leave the page changed in the working tree so it lands in
   the same `release: vX.Y.Z` commit as `CHANGELOG.md` and `Cargo.toml`.
   Report docs gaps from step 4 as one line each.

$ARGUMENTS
