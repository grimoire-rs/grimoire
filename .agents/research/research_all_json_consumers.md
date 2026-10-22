# Research: all.json producers, consumers and byte shape

## Metadata

- Date: 2026-10-20
- Lane: codebase recon (hex-discuss `index-structure`)
- Repos read: `grimoire`, `grimoire-indexer`, `grimoire-vscode`, `grimoire-index`
- Live sample: `https://index.grimoire.rs/all.json`, 19 records — every scale figure is an extrapolation

## Record shape

Producer: `grimoire-indexer/src/data/index.ts:27-36`. The merge at `:148` is
`{...enrich, ...pointer, namespace}`, so the pointer wins on overlap. The type
has an open index signature.

| Source | Fields |
|---|---|
| Pointer, required | `schema` `name` `kind` `ref` `description` `owner{id, github\|login}` |
| Derived, required | `namespace` |
| Pointer, optional | `repository` `keywords[]` `summary` `deprecated` `replaced_by` `license` `created` |
| Enrich, always present | `keywords[]` `tags[]` (full tag list) `deprecated` `hasReadme` `hasChangelog` `hasContents` `contentDigest` `updated` |
| Enrich, optional | `title` `summary` `version` `license` `created` `revision` `authors` `vendor` `url` `documentation` `compatibility` `support{…}` `replacedBy` `logo` |

`rating` and `downloads` are never in `all.json`; they live in `stats.json` and
in the site's build-time data.

## Consumers

| Consumer | Fields read | Load |
|---|---|---|
| grim (`src/catalog/index_source.rs:62-178`) | 13 fields: `schema name namespace kind ref description repository keywords summary deprecated replaced_by license created` | Whole file every stale refresh (1 h TTL). Gzip is negotiated. Caps: 128 MiB decoded, 100,000 entries. No conditional GET. |
| Site build (`renderer/index.ts:339`, `p/[...slug].astro:11`) | Whole record | `readFileSync` of the whole file; one HTML page per package |
| Site landing island (`pages/index.astro:177`, `lib/catalog.ts:79`) | 15-field `cardPackage` projection, every record | Inlined as `<astro-island props>`; only the first 48 cards are server-rendered |
| Site search (`lib/search.ts:159-175`, `Catalog.tsx:1130`) | 13 search fields | `fetch("/all.json")` of the whole file on the first keystroke, with a 15 s timeout |
| VS Code (`src/catalog.ts:65`, `src/grim.ts:640-654`) | Through `grim search --format json` with an empty query, i.e. the whole catalog | `execFile`, `maxBuffer` 16 MiB, 120 s timeout |

Each catalog is therefore shipped about twice: once as island props in
`index.html` (props are about 95% of the `all.json` size at 256 records), and
again as `all.json` on the first search.

## Bytes (live, 19 records)

- 30,466 B raw, 6,820 B gzip, about 1.6 KB per record. Pretty-printed with `JSON.stringify(…, null, 1)`.
- Top fields by raw bytes: `description` 29%, `tags` 10%, `keywords` 9%, `contentDigest` 6%, `summary` 5%.
- `tags` grows with each release (up to 26 per record).
- Unread outside enrich bookkeeping: `contentDigest` `hasContents` `hasReadme` `hasChangelog`.
- Projections (raw / gzip):

  | Projection | Raw | Gzip |
  |---|---|---|
  | grim's 13 fields | 17,983 B | 4,911 B |
  | card, 15 fields | 18,340 B | 5,082 B |
  | search, 13 fields | 19,976 B | 5,131 B |

## Churn without a release

- Rebuilds run hourly (`grimoire-index/.github/workflows/pages.yml`).
- Each enrich run re-describes ⌈N/7⌉ packages regardless of digest (`enrich/index.ts:62,425`). That alone can change `support`, `deprecated`, `keywords`, `summary`, `license`, `tags` and `version`.
- `updated` is re-stamped only if the `enrich.json` seed is lost.
- `rating` and `downloads` churn only `stats.json`.
- `all.json` key and record order is stable, and the file carries no timestamp.

## Frozen statements

- `grimoire-indexer/docs/reference/output-layout.md:18-30`:
  - frozen: `/all.json` plus the per-record `schema`, and `/p/<ns>/<name>/`;
  - "may still move": everything else, the served `index/` tree included.
- `grimoire/docs/src/content/docs/stability.md:41`: `<base>/all.json`, the `stats.json` path and shape, and the git `index/…/metadata.json` tree are frozen locators.
- `package-index.md:171-175` says `all.json` is the only endpoint grim requires, and that unknown fields MUST be tolerated.

## Negative

- Nothing reads the served `dist/index/…/metadata.json` tree over HTTP.
- No ETag, conditional GET or paging anywhere.
- The site has no separate search-data file.

## Leads

- **Key mismatch.** grim reads `replaced_by`, while enrich writes `replacedBy` and the site reads that. A pointer's `replaced_by` is passed through, so grim and the site can disagree.
- **Doc drift.** `package-index.md:171` describes an element as pointer plus `namespace`, but the shipped element also carries the enrich fields.
- **Implied subsets.** The card, search and grim projections overlap heavily. A shared "summary row" shape is implied.
- **No real large-N data.** The `.lhci-*` synthetic builds are the only source.
