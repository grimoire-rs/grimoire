# Handover: description companion v2 — VS Code extension migration

**Audience:** grimoire-vscode maintainers (parallel adoption while grim
lands the rework).
**Source of truth:** [`adr_description_companion.md`](./adr_description_companion.md).
**Status:** contract accepted 2026-07-12; JSON shapes below are frozen at
the contract level (field names/semantics), exact wording of docs pending
`json-interface.md` update. grim work happens on `feat/vscode-extension-api`
(PR #33 — v1 surface there is being reworked in place; do NOT build
against v1's `grim desc publish` or the raw `__grimoire` ref contract).

## TL;DR

- Stop building `repo:__grimoire` refs. The tag becomes grim-internal.
- One call replaces the companion probe + per-file follow-ups:
  `grim fetch <ref> --description` returns **all files inline**.
- New cache primitive: `grim fetch <ref> [--description] --digest-only`
  → `{ref, digest}`, no download. Cache everything by digest.
- `grim describe` gains `has_description: bool` — no more blind probe on
  miss. (Not `description` — that key already carries the description-text
  annotation and stays unchanged.)
- README is no longer guaranteed present in a companion (all members
  optional). Extension already null-safe — keep it that way.
- `grim desc publish` is removed (write moves into `grim publish`
  `[description]`). Extension never called it — no action.

## Call-site migration map

References = current extension code (as explored 2026-07-12).

| Extension site | Today | After |
|---|---|---|
| `grim.ts` `DESC_TAG`, `descRef()` (:287-294) | builds `repo:__grimoire` | **delete**; pass `--description` flag via `fetchArgs` |
| `details.ts:462` companion fetch | `fetch <repo:__grimoire>` then reuse | `fetch <repo> --description` → one report with ALL files inline |
| `details.ts:408-432` `fetchLogo` (`fetch --path <logo>`) | separate call, base64 | read `files[]` entry from the `--description` report (base64 already inline) |
| `details.ts:479-481` CHANGELOG (`fetch --path CHANGELOG.md`) | separate call | same — read from `files[]` |
| `details.ts:436-450` in-tree readme (`fetch --path <name>/README.md`) | fallback channel | unchanged; companion still wins when present (`details.ts:488` precedence stays correct) |
| `details.ts:404` / `pickVersion.ts:25` `describe` | metadata + tags | unchanged; additionally read new `has_description: bool` to skip the companion call when `false` |
| companion miss handling | swallow not-found error | mostly obsolete — gate on `describe.has_description`; keep the null fallback for older grim |

## New JSON shapes

### `grim fetch <ref> --description --format json`

```jsonc
{
  "ref": "ghcr.io/acme/thing:__grimoire",   // resolved companion ref
  "digest": "sha256:…",                      // companion manifest digest → cache key
  "kind": "desc",
  "files": [
    { "path": "README.md",    "size": 812,  "content": "…" },
    { "path": "logo.svg",     "size": 4096, "content": "…", "encoding": "base64" },
    { "path": "CHANGELOG.md", "size": 301,  "content": "…" }
  ]
}
```

- `encoding` present only for binary members (`"base64"`), same convention
  as today's fetch `--path` binary handling. Omit-empty shape (fetch is
  the documented exemption to always-present-null).
- Well-known member names: `README.md`, `logo.png` | `logo.svg`,
  `CHANGELOG.md`. Extra files possible (README-referenced assets) — ignore
  unknown paths.
- **Every member optional.** Companion may exist with only a logo.
- Bounded by grim's 8 MiB layer gate; no truncation inside the report.
- Missing companion: standard error envelope, exit 79 (not-found).

### `grim fetch <ref> [--description] --digest-only --format json`

```jsonc
{ "ref": "ghcr.io/acme/thing:1.2.0", "digest": "sha256:…" }
```

- No layer download — cheap enough to fire on every details-view open.
- Without `--description`: the artifact's manifest digest. One digest
  covers annotations AND content (they live in the same manifest), so it
  invalidates both your `describe` cache and your `fetch` cache.
- With `--description`: the companion tag's manifest digest.

### `grim describe <ref>` — additive field

```jsonc
{ …existing fields…, "has_description": true }
```

The existing `description` key (description-text annotation, rendered in
the details header) is untouched — the presence flag is a separate
`has_description` key.

Always present (describe is the always-present-null contract). Treat
missing key as "older grim → unknown, fall back to probe" — same
tolerance pattern as `registries[].authenticated`.

## Recommended caching flow

```
open details(ref):
  d1 = fetch ref --digest-only                     # 1 HEAD
  if cache[d1] miss:
      describe ref  +  fetch ref                   # metadata + content
      cache[d1] = …
  if describe.has_description:                      # or cached value
      d2 = fetch ref --description --digest-only   # 1 HEAD
      if cache[d2] miss:
          fetch ref --description                  # ONE call: readme+logo+changelog
          cache[d2] = …
```

Warm path: 2 digest probes, 0 downloads (today: 3+ full calls). Digests
are content addresses — cache never stales silently; evict LRU.

## Unchanged surfaces (rely on them)

- `describe` metadata fields, tags[], merge precedence you implement today.
- `search`, `status`, `context` (incl. `registries[].authenticated`).
- Error envelope `{error:{code,exit,message,reason?}}`; `reason:
  "stale-lock"` semantics. (A broader typed reason taxonomy is planned
  grim-side — additive, kebab-case, unknown values must stay tolerated.
  Your current handling is already correct.)
- In-tree README channel (`fetch --path <name>/README.md`) as fallback.

## Sequencing / feature detection

1. grim lands the rework on `feat/vscode-extension-api` (reworks v1 in
   place; ships in the next 0.x release after merge).
2. Extension can prepare now behind detection:
   - `describe.has_description` key present → new grim; use
     `--description` and `--digest-only`.
   - Key absent → old grim; keep current `descRef()` probe path until the
     minimum supported grim version includes v2, then delete it.
3. `grim fetch <repo>:__grimoire` keeps resolving (exact-tag resolution is
   not removed) — your current code won't break during the transition; it
   just stops being the documented contract.

## Open items (grim-side, tracked in ADR implementation plan)

- Exact `json-interface.md` wording for the tri-shaped fetch report.
- MCP `grim_fetch` arg names (`description`, `digest_only`) — parity with
  CLI, relevant only if the extension ever moves to the MCP server.
- `ErrorReason` enum refactor (wire-compatible, no extension action).
