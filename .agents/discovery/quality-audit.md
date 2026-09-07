# Docs quality audit — docs-plan worktree

Scope: `docs/src/*.md` (21 pages) plus `docs/README.md` and `docs/src/SUMMARY.md`,
audited against `.claude/rules/docs-quality/{checks,examples,machine-readers,
navigation,observability,page-types,plain-english}.md` and this project's own
`.claude/rules/docs-style.md`. Read-only: no docs page or code was edited.

## Method

Python 3.14.5. All eight `checks/*.py --self-test` runs pass (0 fixture
mismatches) before any real-corpus run.

| Command | Exit |
|---|---|
| `python3 prose.py --root <wt>/docs --format json` | 1 (2351 findings) |
| `python3 doc_declaration.py --root <wt>/docs --format json` | 0 (0 findings) |
| `python3 page_type.py --root <wt>/docs --format json` | 1 (9 findings) |
| `python3 landing_check.py --root <wt>/docs --format json` | 1 (2 findings) |
| `python3 nav_depth.py --root <wt> --format json` | 1 (1 finding) |
| `python3 nav_depth.py <wt>/docs/src/*.md <wt>/docs/README.md --format json` | 1 (1 finding, DOC-NAV-06) |
| `python3 doc_examples.py --root <wt>/docs --format json` | 1 (11 findings) |
| `python3 links_raw.py --root <wt>/docs --format json` | 1 (1 finding) |
| `python3 strip_prose.py --root <wt>/docs --format json` | 0 (0 findings; used for word counts via stdout `wc -w` per page) |
| `npx markdownlint-cli2 --config checks/markdownlint.jsonc "docs/src/*.md" "docs/README.md"` | 1 (1447 raw findings) |

**Tools**: `npx` (markdownlint-cli2 v0.23.2, fetched on demand) is available and
was run. `vale` is **not installed** on this machine — the tier-1 prose layer
(`checks/vale.ini`, DOC-PLAIN-14 heading-case, honorific severity) was not run.

**Invocation notes** (deviations from the literal example commands in
`checks.md`, needed to get real output):
- `links_raw.py`'s positional `paths` argument requires existing **files**, not
  a directory — the documented `links_raw.py --root docs docs` form errors.
  Omitting the positional and passing only `--root` walks the tree correctly.
- `nav_depth.py`'s nav arm detects `docs/book.toml` + `docs/src/SUMMARY.md`
  relative to `--root`, so `--root` must be the **worktree root**, not
  `docs/`. Its per-page arm (heading depth, page length) takes explicit file
  paths and is independent of the nav arm; passed separately here.
- `doc_declaration.py` returned **zero** findings, not because it did not run,
  but because this worktree's branch (`docs/use-case-discovery`) already
  carries a `doc_type`/`doc_tier` declaration on all 21 content pages plus
  `README.md` and `SUMMARY.md` — the retrofit the rule set assumes has not
  happened elsewhere has already landed here.
- `markdownlint.jsonc` tunes only 7 rules (MD001, MD003, MD025, MD036, MD040,
  MD045, MD054) and leaves every other markdownlint-cli2 default rule on. Of
  1447 raw findings, 1355 come from three defaults the docs-quality rule set
  never names (MD060 table-column-style 550, MD013 line-length 306, plus
  MD033/MD014/MD028/MD053 noise) — filtered out of the counts below since no
  DOC-PLAIN or DOC-EX rule cites them.

## Per-page findings

Prose word counts from `strip_prose.py` (stdout, `wc -w`). Finding counts are
per-script hit counts on that page. "MD054" is the markdownlint link-style
count, reported separately from `prose.py` because DOC-PLAIN-15 is verified by
markdownlint, not by `prose.py`.

| Page | Prose words | prose.py | page_type.py | landing_check.py | doc_examples.py | links_raw.py | MD054 | Top rule IDs |
|---|---|---|---|---|---|---|---|---|
| docs/README.md | 148 | 4 | 1 | 0 | 0 | 1 | 1 | DOC-PLAIN-01×3, DOC-TYPE-33×1, DOC-OBS-02×1 |
| docs/src/SUMMARY.md | 66 | 0 | 0 | 0 | 0 | 0 | 0 | DOC-NAV-03×1 (nav arm) |
| docs/src/agents.md | 849 | 34 | 1 | 0 | 0 | 0 | 52 | DOC-PLAIN-01×25, DOC-PLAIN-02×9, DOC-TYPE-07×1 |
| docs/src/artifacts.md | 1893 | 83 | 0 | 0 | 0 | 0 | 5 | DOC-PLAIN-01×56, DOC-PLAIN-02×25 |
| docs/src/authentication.md | 931 | 27 | 0 | 0 | 0 | 0 | 14 | DOC-PLAIN-01×18, DOC-PLAIN-02×9 |
| docs/src/ci.md | 1177 | 50 | 0 | 0 | 0 | 0 | 10 | DOC-PLAIN-01×32, DOC-PLAIN-02×17, Flesch 47.9 |
| docs/src/clients.md | 2891 | 100 | 1 | 0 | 0 | 0 | 7 | DOC-PLAIN-01×55, DOC-PLAIN-02×42, DOC-TYPE-07×1 |
| docs/src/commands.md | 13111 | 556 | 0 | 0 | 3 | 0 | 20 | DOC-PLAIN-01×350, DOC-PLAIN-02×188, DOC-PLAIN-03×11 |
| docs/src/concepts.md | 1742 | 63 | 0 | 0 | 1 | 0 | 11 | DOC-PLAIN-01×41, DOC-PLAIN-02×20, Flesch 49.5 |
| docs/src/configuration.md | 6005 | 216 | 0 | 0 | 0 | 0 | 22 | DOC-PLAIN-01×142, DOC-PLAIN-02×71 |
| docs/src/hosting-an-index.md | 2232 | 77 | 1 | 0 | 0 | 0 | 18 | DOC-PLAIN-01×48, DOC-PLAIN-02×27, DOC-TYPE-07×1 |
| docs/src/installation.md | 468 | 12 | 0 | 0 | 0 | 0 | 14 | DOC-PLAIN-01×9, DOC-PLAIN-02×2 |
| docs/src/introduction.md | 296 | 14 | 1 | 2 | 0 | 0 | 9 | DOC-PLAIN-01×13, DOC-DISC-16×1, DOC-TYPE-10×1, DOC-TYPE-11×1 |
| docs/src/json-interface.md | 4233 | 173 | 1 | 0 | 0 | 0 | 32 | DOC-PLAIN-01×99, DOC-PLAIN-02×62, DOC-PLAIN-11×10 |
| docs/src/mcp-servers.md | 2227 | 103 | 1 | 0 | 0 | 0 | 55 | DOC-PLAIN-01×65, DOC-PLAIN-02×36, DOC-TYPE-07×1 |
| docs/src/package-index.md | 2170 | 82 | 1 | 0 | 1 | 0 | 16 | DOC-PLAIN-01×52, DOC-PLAIN-02×27, DOC-TYPE-07×1 |
| docs/src/publishing.md | 6241 | 239 | 0 | 0 | 3 | 0 | 7 | DOC-PLAIN-01×165, DOC-PLAIN-02×70, DOC-NAV-06×1 |
| docs/src/quickstart.md | 511 | 20 | 0 | 0 | 0 | 0 | 12 | DOC-PLAIN-01×13, DOC-PLAIN-02×6 |
| docs/src/ratings.md | 2914 | 103 | 0 | 0 | 1 | 0 | 29 | DOC-PLAIN-01×61, DOC-PLAIN-02×40 |
| docs/src/self-hosted-gitlab.md | 1197 | 53 | 0 | 0 | 0 | 0 | 9 | DOC-PLAIN-01×34, DOC-PLAIN-02×19 |
| docs/src/stability.md | 3527 | 156 | 0 | 0 | 0 | 0 | 49 | DOC-PLAIN-01×72, DOC-PLAIN-02×66, Flesch 45.5 |
| docs/src/upgrading.md | 2550 | 100 | 1 | 0 | 0 | 0 | 16 | DOC-PLAIN-01×50, DOC-PLAIN-11×19, DOC-TYPE-08×1 |
| docs/src/vendor-metadata.md | 2964 | 86 | 0 | 0 | 2 | 0 | 91 | DOC-PLAIN-01×55, DOC-PLAIN-02×28 |

Totals: prose.py 2351 (DOC-PLAIN-01 1458, DOC-PLAIN-02 795, DOC-PLAIN-11 73,
DOC-PLAIN-03 20, DOC-PLAIN-05 3, DOC-PLAIN-12 1, DOC-PLAIN-13 1) · page_type.py
9 · landing_check.py 2 · doc_examples.py 11 (all DOC-EX-05) · links_raw.py 1 ·
nav_depth.py 2 (DOC-NAV-03 ×1, DOC-NAV-06 ×1) · doc_declaration.py 0 ·
markdownlint MD054 499, MD040 15 (11 overlap with doc_examples.py, 4 extra:
`markdown`/`powershell` fences outside the declared tier list).

## Cross-cutting findings

1. **DOC-PLAIN-15 / markdownlint MD054, 499 hits across 22 of 23 pages.**
   `.claude/rules/docs-style.md:102-114` mandates reference-style links
   ("never inline `[text](url)` in the body") while
   `.claude/rules/docs-quality/checks/markdownlint.jsonc:26-36` configures
   MD054 to require inline/autolink style. The two rule sets specify opposite
   link syntaxes for the same corpus, so satisfying one guarantees failing the
   other on nearly every page. Heaviest: `docs/src/vendor-metadata.md` (91),
   `docs/src/mcp-servers.md` (55), `docs/src/agents.md` (52).

2. **Two landing surfaces, only one visible to any check.** The generated site
   root (`docs/theme/index.hbs:53-559`, the `is_index` Handlebars branch) is
   hand-authored HTML containing the hero, the "your own index" feature
   section, and the asciinema demo — it is never Markdown, so
   `doc_declaration.py`, `page_type.py`, and `landing_check.py` never see it.
   `docs/src/introduction.md` (`doc_type: landing`) is the only landing-shaped
   page any check reaches, and it fails both DOC-TYPE-10 (`introduction.md:4`,
   4 sentences of lead-in prose against the 1-sentence shape) and DOC-TYPE-11
   (`introduction.md:38`, first runnable command/link menu lands at word 222
   against a 150-word budget).

3. **`publishing.md` mixes a reference page into a how-to page's flow.**
   Declared `doc_type: how-to`, 6241 prose words (DOC-NAV-06,
   over the 4000-word split trigger, `nav_depth.py`), 45 `##`/`###`
   subheadings. `publishing.md:226-244` ("Catalog metadata") is an 11-row
   field/annotation/purpose reference table with no procedural framing around
   it, structurally identical to a `doc_type: reference` entry, embedded
   mid-page between two task sections.

4. **`docs/src/SUMMARY.md` nav is a flat 20-entry list (DOC-NAV-03,
   `nav_depth.py`, `SUMMARY.md:1`).** Reference pages (`agents.md`,
   `artifacts.md`, `clients.md`, `commands.md`, `configuration.md`,
   `json-interface.md`, `mcp-servers.md`, `package-index.md`,
   `vendor-metadata.md`), how-to pages (`ci.md`, `hosting-an-index.md`,
   `installation.md`, `publishing.md`, `quickstart.md`), one runbook
   (`self-hosted-gitlab.md`), one explanation page each on concepts,
   ratings, and stability, and one troubleshooting page (`upgrading.md`) all
   sit in one ungrouped list with no heading distinguishing them.

5. **`introduction.md`'s "Where to next" list routes by feature, not by
   use case.** `introduction.md:36-44` lists five links — Installation, Quick
   Start, Concepts, Host Your Own Index, Command Reference — each a product
   noun or CLI-surface name. No link is phrased as a reader task or role
   ("share a skill with your team," "publish from CI"). The real site root's
   three feature sections (`docs/theme/index.hbs`: hero commands, "your own
   index," "see it run") route the same way.

6. **Friction log corroboration — fragmented reference, not duplicated
   reference.** T05 (`.agents/discovery/friction-logs/T05.md`, "Pros and
   cons") reports the TUI keymap is scattered across non-adjacent paragraphs
   of `commands.md:1387-1547` (`## grim tui`) rather than one table — confirmed
   by direct reading: the intro paragraph gives `t v o g h space`, a later
   paragraph at `commands.md:1547` gives arrow/vim keys, and `i u d` are
   defined only inside a tree-mode group table. No script in this rule set
   measures intra-page key-fact fragmentation; this is a reading-only finding.

7. **Friction log corroboration — the same fact stated two different ways on
   two pages.** T07 reports `quickstart.md`'s client-detection step names only
   4 clients "by default," while `concepts.md`'s Clients section documents the
   full 17-client list one page later. DOC-OBS-14's fork-detection pass
   (below) does not catch this because the two passages share no duplicated
   paragraph — they diverge in content, not in wording.

8. **DOC-TYPE-07, five pages carry an over-budget concept preamble
   (`page_type.py`).** `agents.md:3` (152 words), `clients.md:3` (185),
   `hosting-an-index.md:4` (230), `mcp-servers.md:3` (191),
   `package-index.md:3` (153) — all against the 150-word cap.

9. **DOC-TYPE-08, `upgrading.md` (`doc_type: troubleshooting`) entry titles
   don't follow the required prefix.** `upgrading.md:15`: 11 of 11 entry
   titles open with neither `Error:` nor `Warning:`.

10. **DOC-EX-05, two independent scans of the same rule disagree on scope.**
    `doc_examples.py` flags 11 bare (untagged) fences; `markdownlint` MD040
    with the project's tier list flags 15, the extra 4 being fences tagged
    with a language outside the declared tier list (`markdown` in
    `artifacts.md:220` and `clients.md:162`, `powershell` in
    `installation.md:26` and `:60`). `checks.md` names markdownlint as the
    verification for this rule; `doc_examples.py`'s own scan under-reports it.

11. **DOC-OBS-14 fork detection: zero literal duplicate paragraphs.** A
    40-word-paragraph hash pass across all 23 pages found no paragraph shared
    verbatim between any two files. Where the same concept appears on two
    pages (scope: `concepts.md:181` "Scopes" and `configuration.md:885`
    "Scopes on disk"; registries: `configuration.md:191` "Multiple
    registries" and `package-index.md:459` "Relationship to Registries"), the
    second page cross-references the first rather than restating its
    definition.

12. **DOC-OBS-02, one dead link.** `docs/README.md:6` links to `./src/`,
    which `links_raw.py` resolves to `docs/src.md` — a file that does not
    exist (the target is the `docs/src/` directory, not a page).

## Screencast usage

`docs/src/demo.cast` (asciinema v2 format, 129 lines, recorded at
180×30) plays in exactly one place: the `is_index` branch of
`docs/theme/index.hbs` (comment block `docs/theme/index.hbs:1-49`,
player init `docs/theme/index.hbs:525-543`), which mdBook renders only for
the generated site root, never for any chapter — the file's own comment
states `introduction.html included` still renders through the ordinary
mdBook branch. Consequences:

- No `.md` page, including `introduction.md` (`doc_type: landing`), embeds or
  references the screencast. The page every doc-quality check can see and the
  page a fresh visitor to the site root actually lands on are two different
  documents.
- Player assets (`asciinema-player.min.js`/`.css`, v3.17.0) are vendored,
  not CDN-loaded, per the same comment block.
- Playback starts only once the section scrolls into view
  (`docs/theme/index.hbs:542-543` comment), not on page load — an
  autoplay-on-view pattern, not autoplay-on-load.
- The cast is committed, not regenerated by the docs build; the comment names
  `task test:demo` / `test/recordings/test_record_demo.py` as the
  re-recording path against a real published package.
- `docs/src/start.html`, a second standalone non-Markdown page (a guided
  index-hosting wizard), deliberately shares the landing page's visual design
  tokens (per its own header comment) but carries no screencast.

## Landing routing

`introduction.md` and the real site root both route by product feature, not
by reader use case or role:

- `introduction.md:36-44` ("Where to next"): 5 links, each a product noun or
  CLI surface — Installation, Quick Start, Concepts, Host Your Own Index,
  Command Reference. None names a reader task ("share with your team",
  "publish from CI", "self-host") or a reader role.
- The site-root splash (`docs/theme/index.hbs`, `is_index` branch): three
  labelled feature sections in sequence — hero install/update commands,
  "your own index" (self-hosted-index feature), "see it run" (the demo). Same
  feature-oriented shape.
- `SUMMARY.md`'s flat 20-item nav (finding 4 above) carries no use-case
  grouping either — every page type sits in one undifferentiated list, sorted
  by neither task nor tier.
