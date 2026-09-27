# Researcher brief

The prompt template for the sonnet researcher spawned per target — one
researcher per harness or domain row, all spawned at once (no tier
ordering, no concurrency cap). Fill the `<…>` slots from the ledger row and [domains.md](domains.md); pass the
whole block as the spawn prompt, with `model: sonnet` and a
`Model rationale:` line.

## Template

```text
Model rationale: upstream fact research, one target — sonnet per routing policy.

You re-verify grim's upstream claims for ONE target. Read-only: do not edit
any repo file, do not commit, do not install anything outside a throwaway dir.
The one file you write is <scratchpad>/<target>.claims.md: your final answer,
verbatim, so the driver assembles the run artifact without re-typing it.

Target: <target>   Kind: <harness|domain>   Tier: <1|2|—>
Depth: <feed|sweep|deep>   Focus: <surface, or "all">
Feed: <feed from domains.md>   Cursor: <ledger Feed cursor, or "—">
Docs: <docs URL(s) from domains.md>

Claims to re-verify (ledger location → current text):
<one line per watchlist row / code stamp / docs claim this target owns,
 with file:line>

Feed entries to assess (feed depth only):
<new tags or headings above the cursor, with URLs>

Sweep/deep only: scan the changelog from <previous Last check, or the
newest `verified` date among the claims above> to today for entries that
touch skills, rules, agents, MCP, hooks, frontmatter keys, config paths or
env vars.

Deep only: inventory the vendor's whole current surface per
deep-pass.md › Full surface inventory <paste that table>. Every
difference from grim is one extra row, numbered I1, I2, ….

For every claim above, and for any undated claim you meet in the listed
sources, fetch the PRIMARY source (vendor docs, vendor repo, vendor
changelog, spec text). Summaries, blogs, search snippets and third-party
mirrors are not primary. Record the vendor version the source applies to
when one is stated.

Fetch tips: many doc sites serve raw markdown at `<page>.md` (e.g.
code.claude.com/docs/en/skills.md). WebFetch summarizes and truncates
large pages; when it persists output to a file, Read that file for the
verbatim quote. Bash network (`curl`, sometimes `gh`) may be denied in
your sandbox: then use WebFetch, and say in Feed notes that vendor-site
quotes came through a summarizing fetch. Copy a quote only from fetched
text — the verifier re-fetches it and drops one it cannot find. Read a GitHub CHANGELOG.md whole with
`gh api repos/<o>/<r>/contents/CHANGELOG.md --jq .content | base64 -d`.
Link liveness (does a cited URL still resolve, and where does it redirect?)
when `curl` is denied and WebFetch errors: `python3 -c` with
`urllib.request` and a `HEAD` request, timeout 20 — print the status and
final URL, or the error text. A TLS or socket error is a dead link, not a
missing source.
Use /usr/bin/grep, not bare grep (a wrapper truncates matches).

Return ONLY this table, one row per claim, plus the two lists below it:

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |

- "New value" = "unchanged" when the source confirms the old value.
- The quote is copied verbatim from the source, ≤ 3 sentences.
- A claim you cannot back with a primary URL AND a verbatim quote: do not
  put it in the table. List it under "Unsourced" with what you tried.
- Never guess, never infer a version, never fill a cell from memory.

Unsourced:
- <claim> — <what you searched / why no primary source>

Feed notes:
- Newest tag or heading seen: <value>. Cursor found: <yes|no|n/a —
  cursor was `—`>.
- Anything grim renders or documents that the feed shows changed but no
  ledger row covers: <item + URL>.
```

## Handling the output

- Rows go into the run artifact's `## Claims` verbatim.
- Every "Unsourced" item is **dropped** — it changes nothing — and is
  listed under `## Friction`.
- "Cursor found: no" → the target escalates to `sweep` (SKILL.md › Feed
  procedure) and the escalation is friction too.
- A researcher that returns nothing, or no table, is retried once with the
  same brief; a second miss leaves the ledger row unchanged with a note.
- A row whose new value would drive a renderer, metadata or validation
  change goes to the opus verifier before it lands.
