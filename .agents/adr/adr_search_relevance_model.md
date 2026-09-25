# ADR: Search relevance — per-field matchers, weights, and a relative cutoff

## Metadata

**Status:** Accepted
**Date:** 2026-09-25
**Deciders:** Michael Herwig + Claude
**Beads Issue:** N/A
**Related PRD:** N/A
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md` (no new dependency — `fuzzy-matcher` 0.3.7 is already pinned)
**Domain Tags:** api, tui
**Supersedes:** N/A (fuzzy matching shipped in v0.13.0 without an ADR; this records the model it grew into)
**Superseded By:** N/A

## Context

v0.13.0 replaced substring matching with fuzzy matching: one skim subsequence
matcher (`fuzzy-matcher` 0.3.7, `SkimMatcherV2`) run over name, summary,
description and keywords, ranked by score. v0.14.0 added `--sort`. Neither
has an ADR.

On a real index this produced three defects:

1. **A common term returned everything.** `grim` appears in the description
   of 300+ entries on the public index. Every one matched, so the query listed
   the catalog.
2. **Field did not matter.** A hit in an artifact's name weighed the same as a
   passing mention in its description, so `grim-usage` did not reliably
   outrank a blurb that said "installed with grim".
3. **Scattered letters matched.** Subsequence matching accepts a term whose
   letters are spread through a long string. `grim` matched prose as
   g…r…i…m across a sentence, `hex` matched `michael-herwig/arcana/nox` across
   the org path, and every row on `ghcr.io/m…` matched `grim` through the
   registry host.

`grim search`, the TUI `/` filter and the MCP `grim_search` tool share one
matcher (`src/catalog/search_match.rs`), so all three had all three defects.

## Decision Drivers

- The motivating query (`grim` over the public index) must return the
  artifacts named for it, not the catalog.
- Abbreviations that work today (`kubctl` → `kube-control`) keep working.
- One matcher and one cutoff for CLI, TUI and MCP — the surfaces must agree on
  *which* rows a query returns, not only their order.
- No new dependency and no index build step; the catalog is at most a few
  thousand rows, matched in-process on every keystroke in the TUI.
- Principle 9: the JSON shape, field names, exit codes and ordering keys stay
  unchanged.

## Considered Options

### Option 1: Per-field matchers and weights, plus a relative cutoff — chosen

Fuzzy matching only on identifier-like fields, word-prefix matching on prose,
a weight per field, and a configurable cutoff relative to the best hit.

| Pros | Cons |
|------|------|
| Fixes all three defects with the matcher already pinned | Hand-tuned constants; calibrated on measurement, not learned |
| The cutoff is scale-free, so it holds for any term length | What a query returns depends on the best hit in the catalog |

### Option 2: Absolute score threshold

Drop every hit below a fixed score.

| Pros | Cons |
|------|------|
| A row's visibility depends only on the row | skim scores are unbounded and grow with term length — no single number separates noise from a hit for both `a` and `kubernetes` |

### Option 3: Rank only, default cutoff `0`

Weight the fields and sort, but return every match.

| Pros | Cons |
|------|------|
| Recall never changes | Fails the motivating case: `grim` still lists 300+ rows, only reordered. Rejected by the owner |

### Option 4: Top-N cap

Return at most N rows.

| Pros | Cons |
|------|------|
| Bounded output | Hides good hits arbitrarily when many rows are equally relevant, and still shows noise when few are. crates.io caps its *candidate* set for cost, not relevance — a different problem |

### Option 5: Fuzzy everywhere, with a per-character floor on prose

Keep one matcher and raise the floor.

| Pros | Cons |
|------|------|
| One code path | Measured: a short term's letters sit tightly inside *some* word of almost any sentence. `a` matched 5/5 unrelated descriptions at a score that passed any floor worth having |

### Option 6: A full-text engine (tantivy, meilisearch)

| Pros | Cons |
|------|------|
| Stemming, typo tolerance, BM25 | A new dependency and an index lifecycle — an innovation token spent on ≤ thousands of rows that a linear scan already handles |

### Option 7: Swap the matcher (nucleo, frizbee)

| Pros | Cons |
|------|------|
| nucleo is faster; frizbee tolerates typos | nucleo is MPL-2.0, a license review for a matcher that is not the bottleneck; typo tolerance is a separate feature. Deferred, not rejected |

## Decision Outcome

**Chosen Option:** Option 1.

### Matching

A query splits on Unicode whitespace; each token is lowercased. A bare kind
word (`skill`, `rule`, `bundle`, singular or plural) is an **exact kind
filter**, never a text term. Every text term must hit some field (AND across
terms). Each term scores its best field; the entry's score is the **sum of
per-term bests**, which is what lets one term hit the name and another only a
keyword.

| Field | Matcher | Weight | Name bonus (raw, per term char) |
|---|---|---|---|
| Leaf (last repository segment) | fuzzy, floor 12/char | ×3 | +8 exact, +4 whole word |
| Keywords (best keyword) | fuzzy, floor 12/char | ×2 | +8 exact, +4 whole word |
| Summary | word prefix | ×2 | — |
| Description | word prefix | ×1 | — |
| Namespace (path above the leaf) | word prefix | ×1 | — |
| Kind, as free text | fuzzy, floor 12/char | ×1 | — |
| Full reference, incl. registry host | fuzzy, floor 12/char | ×3 | — |

- **Fuzzy** is skim's subsequence match; a hit counts only when its raw score
  reaches `MIN_SCORE_PER_CHAR` = 12 per term character. Transpositions and
  substitutions do not match.
- **Word prefix** splits the field on every non-alphanumeric character
  (Unicode-aware). The term must start a word: 20 per character for a whole
  word, 16 for a prefix. A term carrying its own separator (`code-review`,
  `node.js`) matches contiguously anywhere, at 16 per character.
- **The registry host is excluded.** The full-reference pass runs only when
  the term contains `/`, `.` or `:` — a pasted `ghcr.io/acme/x`, or a `repo`
  copied from `--format json`. Host detection follows Docker: a first segment
  containing `.` or `:`, or exactly `localhost`.

### Calibration

Measured skim scores: a clean contiguous hit costs about 20 per character (16
per matched character plus a consecutive-run bonus), plus a start-of-word
bonus near 11. A letter-scatter scores about 2 per character, or about 10 when
every letter lands on a word start — hence the floor of 12. Prose scores are
deterministic and sit on the same scale without the start bonus, so the
weights alone decide name against blurb.

For a term of `n` characters:

| Hit | Score |
|---|---|
| Leaf, exact (`grim`) | (20n + 11 + 8n) × 3 = 84n + 33 |
| Leaf, whole word (`grim-usage`) | (20n + 11 + 4n) × 3 = 72n + 33 |
| Leaf, prefix (`grimoire`) — weakest clean name hit | (20n + 11) × 3 = 60n + 33 |
| Keyword, exact | (20n + 11 + 8n) × 2 = 56n + 22 |
| Summary, whole word | 20n × 2 = 40n |
| Description, whole word | 20n × 1 = 20n |

**The name-wins bound.** A whole-word summary hit is at most 2/3 of the weakest
clean name hit (40n < 40n + 22), and a whole-word description hit at most 1/3.
So at the default cutoff a description-only mention never survives beside a
name hit, and the `grim` family (`grim`, `grim-usage`, `grimoire`) survives
together. The bonus exists because skim scores `grim` identically in all
three; it is sized so the longer names stay above 50% of the exact one. An
exact keyword gets the same bonus: without it, a keyword tagged `grim` scored
49% of the exact name and dropped.

The bound holds for *clean* hits. A barely-tight abbreviation (raw 12/char,
36n weighted) scores below a whole-word summary hit.

### The cutoff

`[options].search_min_relevance` (a `u32` percentage):

- default `50`; `0` turns the cutoff off; maximum `100`
- `grim config set` with a value above 100 exits `65`; a `grimoire.toml` that
  authors one exits `78`
- a row is dropped when `score × 100 < best × percent` (`retain_relevant`)
- an all-zero result set (empty or kind-only query) keeps every row

The cutoff is applied in one shared function by `grim search` on every path
(including `--registry`, which falls back to the default with a warning when
the config fails to load), by the TUI filter, and by MCP `grim_search`
through `grim search`. It runs **before** `--sort`: the cutoff decides which
rows a query returns, and `--sort` only reorders them.

Hidden rows are announced as `N weaker matches hidden; set
options.search_min_relevance to 0 to list all` on stderr, and as `N weaker
matches hidden` in the TUI status line. They are **never** reported in JSON,
so the report stays byte-identical in shape, and stdout stays clean for
`grim mcp`'s JSON-RPC.

### Principle 9 ruling

**The set of rows `grim search` returns for a query is not a frozen
contract.** Frozen: the JSON report shape and field names, the exit codes, the
`--sort` keys and their ordering semantics, the tool and argument names on
MCP. Relevance and recall — which rows match, which survive the cutoff, and
their order under relevance ranking — may change in any minor, and each change
a user would notice is recorded in `upgrading.md`.

This ruling exists so that retuning a weight or a floor is not re-argued as a
breaking change. `docs/src/content/docs/stability.md` does not state it today:
its "Unstable" section covers render layout, human-readable text and NDJSON
progress, not search results. A one-line addition there is a follow-up.

### Consequences

**Positive:**
- `grim` over the public index returns the artifacts named for it.
- Letters scattered across prose, the org path or the registry host no longer
  match.
- CLI, TUI and MCP return the same rows for the same query and config.
- `0` restores every match; the notice names the key that does it.

**Negative:**
- **The cutoff is relative to the best hit.** Publishing a new artifact whose
  name matches a query can hide rows that query returned before, for every
  consumer of that index.
- **A namespace-only hit drops beside a name hit.** `arcana` finds the
  artifacts under `…/arcana/`, but they drop as soon as another artifact is
  itself named `arcana`.
- **Scripts without spaces (CJK) form one word.** A term matches only at the
  start of a run. A word segmenter is the upgrade.
- **No typo tolerance.** `kuberentes` does not find `kubernetes`.
- **An MCP client never sees the notice.** It goes to the server's stderr.
  A per-call cutoff argument is an additive follow-up.
- **Per-keystroke cost.** The TUI rescans every row on each key; description
  scoring (lowercase, then word scan) dominates at 5k rows. Key coalescing is
  deferred until it is measured as a problem.

**Risks:**
- *Constants drift from a real catalog.* The weights and floor are hand-tuned.
  Unit tests pin the orderings that matter (exact > whole word > prefix name,
  exact keyword beside exact name, the motivating `grim` case, the short-term
  prose regressions). Retune against a real index, never against a single
  query.

## Technical Details

```text
entry score = Σ over text terms of  max over fields of  raw(field, term) × weight(field)
              (a term that hits no field ⇒ entry does not match)
kind words  = exact gate on the entry's kind, AND with the terms
cutoff      = keep row  ⇔  score × 100 ≥ best × search_min_relevance
order       = cutoff, then --sort mode if given, else score descending (stable)
```

Code: `src/catalog/search_match.rs` (`SearchQuery::score_fields`,
`retain_relevant`), `src/command/search.rs` (`apply_relevance_cutoff`,
`hidden_rows_notice`), `src/tui/state.rs` (`recompute_filter`),
`src/config/defaults.rs` (`SEARCH_MIN_RELEVANCE`, `SEARCH_MIN_RELEVANCE_MAX`),
`src/command/config_keys.rs` (`options.search_min_relevance`).

## Validation

- [x] Unit tests pin the calibration orderings and the regressions named above.
- [x] JSON report shape unchanged; the hidden-row count is stderr-only.
- [ ] `upgrading.md` entry for the changed recall.
- [ ] One-line Principle 9 note in `stability.md` (see the ruling above).
- [ ] Doc drift: `SearchToolArgs::query` (`src/mcp/tool_args.rs`) and the
      `src/command/search.rs` module doc still say every field matches fuzzy.

## Links

- [`adr_multi_registry_mcp.md`](./adr_multi_registry_mcp.md) — the shared `load_catalog` seam all three surfaces search through
- [`adr_catalog_summary_annotation.md`](./adr_catalog_summary_annotation.md) — added summary as a matched field
- [`adr_registry_browse_filters.md`](./adr_registry_browse_filters.md) — browse filters narrow the set before the matcher runs
- `docs/src/content/docs/stability.md`, `docs/src/content/docs/upgrading.md`

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-09-25 | Michael Herwig + Claude | Initial decision |
