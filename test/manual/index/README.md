# Static index fixture

Served by the `index` service in [`../docker-compose.yml`](../docker-compose.yml)
on **http://localhost:5052**, straight off this directory. Two files, both
committed, both edited by hand:

| File | What it is |
|---|---|
| `all.json` | The phone book — one pointer per artifact, exactly the shape a compiled index publishes. |
| `stats.json` | The publisher-statistics sidecar: `rating` and `downloads`, keyed by artifact ref. |

## Why it exists

**Sidecar stats ride the HTTP index transport only.** An OCI `_catalog` browse
has no `stats.json` to fetch, and a git-transport index has none either. The
rig's two registries are both OCI, so before this service existed every row in
the rig was permanently unrated and uncounted — the rating badge, the `Rating:`
detail row, the download badge and the `Downloads:` row could not be reviewed
at all.

The pointers name the **same refs** the OCI registries already serve, so every
row here is genuinely installable. That also means the browse shows the same
artifacts under two tree roots. This is real grim behaviour, not a rig
artefact: a repository served by two configured registries appears once per
registry.

## What each row demonstrates

Absence is the point as much as presence — most rows carry neither signal,
because on a real index most rows do.

| Ref | Rating | Downloads | Shows |
|---|---|---|---|
| `skills/code-reviewer` | 42 | 1416 + `versions` | Both signals, and the only row with a per-release breakdown: the DOWNLOADS rail shows `1.2.0` beside the total and folds `1.1.0`/`1.0.0` behind *older releases*. The three sum to 1400, **below** `total` — the other 16 pulls came through a channel tag that names no release, which is why neither figure may be derived from the other. |
| `skills/support-desk` | — | 250412 | Counted, unrated. Large enough to exercise the compact badge (`250K`). |
| `skills/hello-world` | 3 | 87 | Both, small enough that the badge shows the exact figure. |
| `skills/commit-helper` | 7 | — | Rated, **uncounted**. `downloads` is absent, not zero — this is what every GHCR- or GitLab-backed row looks like. |
| `agents/reviewer` | — | **0** | A measured zero. It renders `0`, and must not read the same as the uncounted row above. Carried here rather than on `old-reviewer` because `grim search` hides a deprecated row unless it is installed or `--show-deprecated` is passed — the case would never appear in review. |
| `skills/old-reviewer` | 1 | 0 | Both signals on a deprecated row. Visible with `grim search --show-deprecated`, or once it is declared. |
| `rules/rust-style` | — | 12, **no `as_of`** | The stamp is optional: the figure shows, the "as of" clause does not. |
| `rules/security-baseline` | 15 | 640, stale `as_of` | The stamp is three months behind `generated_at`, so the rail's date is visibly not today. |
| `localhost:5051/tools/skills/commit-helper` | — | 31 | An index serves rows from arbitrary hosts. Proves the join keys on the full ref, not the repository path. |
| everything else | — | — | Neither. The common case. |

## Editing it

The files are served straight off disk, so an edit is live — no rebuild, no
container restart. Re-browse to pick it up:

```sh
grim search --refresh          # grim caches the catalog per registry
```

`stats.json`'s `schema_version` is a monotonic integer. Bumping it past what
the reading binary understands makes grim treat the sidecar as *unobserved*
(carry-forward), not as empty — which is itself worth a look.
