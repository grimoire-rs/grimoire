# Handover: indexer pickup for the new annotation data

Consumer-side follow-up to
[#106](https://github.com/grimoire-rs/grimoire/issues/106). Everything grim
owes is shipped; this is what `~/dev/grimoire-indexer` needs to do to receive
it. Nothing here is blocking — the index keeps working untouched, it just
carries less than it now could.

**Delivered.** The actionable copy lives in the indexer's own repo at
`~/dev/grimoire-indexer/.agents/handover_grim_annotation_fields.md` — it adds
the renderer/fixture steps that only that repo knows about. This file is the
grim-side record; that one is the work item.

## What grim publishes now

Build provenance (`org.opencontainers.image.{revision,created}`) is derived on
every `build`/`release`/`publish` rather than only under `--git`, so `created`
is populated for the first time on ordinary releases. See
[`adr_default_provenance_and_support_channels.md`](./adr/adr_default_provenance_and_support_channels.md).

## 1. The enrich sidecar copies a fixed key list

`src/enrich/index.ts` — `mapMeta` copies
`["title","summary","version","license","created"]` out of
`grim describe --format json`. Anything not in that list is invisible to the
site build, even though the payload carries it.

Newly available curated fields:

| Field | Meaning |
|---|---|
| `revision` | publishing commit SHA (`-dirty` suffixed from a dirty tree) |
| `authors` | maintainer — only present when published with `--git` or authored |
| `vendor` | distributing organization; derived from the repo namespace when unset |
| `url` | project home page |
| `documentation` | docs URL |
| `compatibility` | a skill's editor/runtime hint; `null` for every other kind |
| `support` | object `{issues, chat, contact, security}` — see below |

`created` was already in the list and starts arriving with a value.

## 2. `support` is repository-level and mutable

`describe.support` is read from the **description companion's** manifest, not
the version's, because a maintainer contact belongs to the repository and
changes over time. Consequences for the indexer:

- It is the same for every version of a repository — cache it per repo, not
  per version.
- It changes **without a digest change on any artifact**. The enrich pass
  skips work by artifact digest; that skip would miss a support-link edit.
  The companion has its own digest (`grim fetch <ref> --description
  --digest-only`), which is what the existing companion-skip already probes.
- All four fields are `null` for a repository that publishes no companion.

## 3. `stats.json` gains a recency signal

The ADR's decision was that grim writes **no** recency data: the indexer
already runs `describe` per package on every build, so `updated` is a pure
indexer-side join.

- Write `entries[<ref>].updated` from the representative tag's `created`.
- Fall back to indexer observation time (first build that saw the current
  digest) when the artifact carries no `created` — published without a
  repository, or with `--no-git`.
- Both sides are already forward-compatible: grim's `WireStats`
  (`src/catalog/index_source.rs`) is documented as "a ref may carry a future
  `downloads` and no `rating`", and the indexer's `StatEntries` carries
  unknown stat keys forward. **No `schema_version` bump.**

Do **not** route this through `metadata.json`: that pointer is contractually
"never versions" (`src/catalog/index_announce.rs`).

## 4. `all.json` pointers gain two fields

`grim publish --announce` now writes `license` and `created` into each
`metadata.json`, and grim reads both back (`license` lands in the row's `oci`
object, `created` in `created`). Existing pointers lack them and stay valid —
both are optional. Packages re-announce on their next publish; a backfill pass
over `enrich/` would populate the rest sooner.

Spec rows are already in `docs/src/package-index.md`.

## Testing the visualization

The manual rig now publishes an artifact carrying the full set, including
support channels: `test/manual/scripts/bootstrap.sh`, then scenario 9 in
`test/manual/README.md`. It walks every read surface and demonstrates changing
a support link without changing the artifact digest — the case the indexer's
digest-keyed enrich skip has to handle.

Where each field belongs on a page is settled the same way grim settles it —
see `docs/src/publishing.md#metadata-surfaces`.

## Verification

```sh
grim describe ghcr.io/grimoire-rs/skills/grim-usage --format json | jq '{created, revision, authors, vendor, url, documentation, support}'
```

A repository with a companion carrying `[description.support]` returns a
populated `support` object; one without returns four `null`s.
