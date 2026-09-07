# Review record — docs site redesign

Companion to [`plan_docs_site_redesign.md`](./plans/plan_docs_site_redesign.md).
Everything here needs a human decision or is a known gap left open on purpose.
Nothing below blocks the three PRs, which are green independently.

## What the review found and fixed

One round of six perspectives (use-case coverage, factual accuracy against the
binary, cross-page consistency, the URL contract, security, landing and
accessibility), then a second round scoped to the first round's own fix diff.

Eight statements were plausible, passed every check, and were false. The
fact-check pass reproduced whole pages end to end against `grim` 0.14.1 rather
than reading them, which is the only reason they surfaced:

| Page | Said | Actually |
|---|---|---|
| `quickstart.md` | a rule aimed at Codex is skipped **with a warning** | `installer.rs:556` logs it at `debug!`, silent on stderr |
| `browse.md` | the browser's state words are `grim status`'s | seven versus five, overlapping on three |
| `browse.md` | the zero-match filter diagnostic never reaches the screen | `grim search` prints it to stderr |
| `guides/registries.md` | `GRIM_DEFAULT_REGISTRY` outranks `[[registries]]` | `registry_resolve.rs:526` consults the entry first |
| `guides/versioning.md` | only `:latest` is floating | contradicted `concepts.md`, and itself two paragraphs later |
| `concepts.md` | warm the cache with `grim lock` | `lock` writes no content, so `--offline` then exits 81 |
| `guides/team-ci.md` | two digests for one artifact | both fabricated |
| `guides/lifecycle.md` | `grim status --check` costs one round trip | it is 1 + N |

The URL contract itself held: 21 of 21 pre-migration pages, 289 of 289
`{#custom-id}` anchors, 4 of 4 schema `$id` values byte-identical to what
`grim schema` generates, 5892 of 5892 internal links resolving page and
fragment. That was derived from `src/`, `catalog/**` and the install scripts
independently, before reading the gate that claims it.

Four holes in the gate itself were found by planting breaks and watching it exit
0, and all four are closed and re-verified the same way.

Round two re-checked the first round's own fixes, because two of them had
already introduced defects. Verdict: no fix was itself wrong and no new
assertion was inert — each was proven by planting the break it claims to catch.
It found three neighbour breaks, all fixed: two link texts falsified by the
registries rewrite, and a gate that closed the dead-recording hole while leaving
the dead-player one open.

It also found two things outside the diff, both now fixed. `landing_check.py`
compared the five Principle 9 footer links against every link in the footer, and
the site nav repeats three of them there, so removing one from the site-links
block stayed green — the assertion is now scoped to that block and fails on
exactly that break. And `subsystem-file-structure.md` said the installer "warns"
on a kind a client declines, in three places; it logs at `debug`, which is the
same error `quickstart.md` had. The fourth instance in that file is correct and
was left alone: a kind declined only at one scope does warn.

## Decisions I made that are yours to overturn

**The router stays at eight cards.** `use-cases.yaml:885` says nine, its own
`router:` list has eight. `:476` records your decision of 2026-09-06 that
`guides/mcp-everywhere.md` is sidebar-only, "folded into the shared-skills
card", and the design record agrees in four places. A fix pass shipped nine and I
reverted it. The reachability problem behind it is fixed instead:
`shared-skills.md` now covers MCP and links the guide. Adding the ninth card is
five lines in `landing.ts` and needs no gate change.

**`upgrading.md` stays non-compliant with `page_type.py`.** Fixing DOC-TYPE-08
trades it for DOC-TYPE-09, which the rule's own type table says must not fire on
a troubleshooting page (`page-types.md:75` against `page_type.py:240`). The
checker is vendored from `ocx-sh/grimoire-lore`; the one-line scope fix belongs
upstream.

## Open, ranked by what it costs you to leave

1. **Every built page renders two `<h1>`** — Starlight's frame title plus the
   body's own, on all 35 pages, introduced by the migration. Anchor-safe to fix:
   no shipped link targets any page's body H1 slug. Not free: dropping `# Title`
   moves what DOC-TYPE-07 and DOC-TYPE-22 measure, so all 35 need re-checking.
2. **`docs-style.md` and DOC-PLAIN-15 contradict each other** on reference-style
   versus inline links, and neither is in any gate. Twelve new pages are inline,
   twenty-one old ones are reference-style. Identical rendered output.
3. **Heading case is inconsistent site-wide** and predates this work. Every
   heading carries an explicit `{#…}`, so a sweep is anchor-safe.
4. **npm dependabot bumps get no pre-merge validation.** `docs.yml` triggers on
   `push: main` only, so a bad bump merges green, fails `build`, and the site
   freezes on the last good deploy with no red check. Fail-safe, but the fix adds
   a docs build to every docs-touching PR, which is a CI-cost call.
5. **`AGENTS.md`'s short-id precedence ladder is incomplete** — accurate about
   `fallback_primary`, silent that a `[[registries]]` entry precedes the whole
   chain. I fixed the docs page and left project context alone.
6. **The URL gate cannot see a *deleted* anchor.** It proves every anchor that
   exists resolves. Closing it means a committed baseline diffed each run, which
   is a real maintenance cost on a file that changes with every page edit. Two
   generated anchors did go in this migration, both unreferenced.
7. **`installation.md` opens with "Recommended: install with ocx"**, putting a
   second package manager in front of someone who wants `grim`. A product call,
   not a docs fix.
8. **`docs/migrate_frontmatter.py`** is committed, unwired, and rewrites pages in
   place with no guard. Delete it or give it a `--dry-run` default.
9. **The footer repeats the whole sidebar** on all 21 documentation pages, where
   the rail already shows it. C-020 and C-023 specify it; one conditional makes
   it landing-only.
10. **`product-context.md` calls its headline verbatim** and that headline
    contains an em dash DOC-PLAIN-01 bans on every page quoting it.
11. **The right-hand ToC is 250px where the canvas draws 220px.**
    `--sl-sidebar-width` drives both columns.
12. **The landing's styles stay landing-only by Astro's chunking**, which nothing
    asserts.

## History

Not rewritten. Every non-merge commit is already a Conventional Commit, signed
off, with no attribution trailers and no fixups — the shape `/finalize` produces.
The only remaining action would be compacting 42 `chore:` bookkeeping commits and
38 merges, and that means rewriting three nested branches with no gate to catch a
mistake. Run it on whichever branch you choose to keep, if you want it.
