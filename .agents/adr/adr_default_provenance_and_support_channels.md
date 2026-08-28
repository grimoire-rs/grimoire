# ADR: default build provenance, descriptive annotations, and repository support channels

## Metadata

**Status:** Accepted
**Date:** 2026-08-26
**Deciders:** Michael Herwig (maintainer)
**Issue:** [#106](https://github.com/grimoire-rs/grimoire/issues/106) "More annotation data"
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md`
      (git subprocess, no new crates, OCI substrate, existing fetch/publish machinery)
**Domain Tags:** api, integration, security
**Supersedes:** the "Automatic when in a git repo" rejected alternative in
[`adr_git_provenance_annotations.md`](./adr_git_provenance_annotations.md);
scopes the "No metadata in the companion" clause of
[`adr_description_companion.md`](./adr_description_companion.md) § 1

## Context

Published artifacts carried a thin annotation set, and the read side was
already built for more:

- `OciMeta` (`src/catalog/registry_catalog.rs`) reads five curated
  `org.opencontainers.image.*` keys — `licenses`, `authors`, `url`,
  `documentation`, `vendor` — and the TUI detail pane renders all five. The
  write path emitted only `licenses`. Four were dead read code.
- The VS Code extension renders a **"Published"** rail row from
  `describe.created ?? searchItem.created`.
- The indexer copies `["title","summary","version","license","created"]` out of
  `grim describe --format json` into its enrich sidecar.

None could ever receive `created`: it was gated behind `--git`
(`adr_git_provenance_annotations.md`). That gate protected the exact-version
overwrite guard in `src/command/release.rs`, which compares the pushed manifest
digest against what the version tag already resolves to and refuses a mismatch
without `--force`. A wall-clock timestamp would make every re-release of
identical content a "different" artifact.

Separately, a corporate deployment needs to answer *who maintains this and
where do I file a ticket* — data that changes over time and belongs to no
single version.

## Decision Drivers

- The idempotent-re-release contract is load-bearing and must survive
- Publishing to a wider audience than the checkout must not disclose internal
  infrastructure by default
- Downstream consumers (indexer, VS Code extension, TUI) read curated fields,
  not raw annotation maps
- Principle 9: additive-only evolution on the road to 1.0.0

## Decision Outcome

### 1. Provenance is derived by default, from fixed inputs only

`org.opencontainers.image.{revision,created}` are derived on every
`build`/`release`/`publish`. The contract survives because **no derived value
is read from the clock**: `created` is the commit date (`git show -s
--format=%cI`), or a [`SOURCE_DATE_EPOCH`](https://reproducible-builds.org/docs/source-date-epoch/)
instant outside a repository, or absent. A re-release from the same commit
yields the same digest.

This reverses `adr_git_provenance_annotations.md`'s rejection of "Automatic
when in a git repo", whose stated reason was *"silently changes every
publisher's manifest digests … without consent."* `--no-git` is the consent
mechanism that alternative was missing. The residual cost stands and is
accepted: re-releasing an existing version from a *different* commit now
produces a different digest and is refused without `--force` — the same trade
`--git` already made, now on the default path.

### 2. `--git` / `--no-git`: a tri-state, and a disclosure boundary

| Mode | Behavior |
|---|---|
| `auto` (default) | Derive what is available; never fail; never disclose |
| `--git` | Derivation is mandatory (a non-git path is a data error, 65), **and** the `origin` remote and commit author are published |
| `--no-git` | Emit nothing derived |

The split is the point. `revision` (a bare SHA) and `created` (a commit date)
describe the content. The `origin` remote names the **forge host and repository
path** the build came from, and the commit author names a **person**. Neither
describes the artifact, and publishing either by default would leak into every
manifest anyone can pull. Both stay behind the explicit opt-in.

`GitProvenance::resolve` is the single seam that clears them, so a future
annotation builder cannot reintroduce the disclosure by forgetting a rule. The
author name is `%an` only — never `%ae`, matching the credential-stripping
invariant in `normalize_remote_url` (an address in a public manifest is
harvestable).

### 3. Descriptive annotations, derived only when not authored

| Key | Authored as | Derived from |
|---|---|---|
| `…image.authors` | `authors` | the commit author, **under `--git` only** |
| `…image.vendor` | `vendor` | the release repository's namespace (`ghcr.io/acme/skills/x` → `acme`) |
| `…image.url` | `homepage` | the authored `repository` |
| `…image.documentation` | `documentation` | `<repository>#readme` |

A reference with no namespace (`registry/name`) derives no vendor — publishing
the artifact's own name as its distributor would be worse than absence.

Two asymmetries closed while here: an MCP descriptor could not name a successor
(`replaced-by`, which every other kind had), and a skill's authored
`compatibility` was never published (now `com.grimoire.compatibility`).

### 4. Support channels live on the companion, not on a version

`com.grimoire.support.{issues,chat,contact,security}` are published on the
**`__grimoire` companion manifest**, authored in `publish.toml`.

> **Amendment (pre-release, before the 0.14.0 tag).** The authoring surface
> was originally `[description.support]`, a sub-table of `[description]`. It is
> now a manifest-level `[support]` table, sibling of `[metadata]` — see § 5.
> The decision this section records (support on the companion, CycloneDX
> names, flat string keys, mutable answer) is unchanged.

They answer "who maintains this repository and where do I reach them" — a
property of the repository that changes over time. On a version's manifest a
moved chat link would be frozen into every already-published tag, fixable only
by re-releasing history. The companion tag is mutable by design, so one
`grim publish` re-run updates the answer for every version at once.

This is a **scoped exception** to `adr_description_companion.md` § 1's "No
metadata in the companion", which was written against *versioned* metadata
(summary/keywords/license) and justified by its own principle: *"versioned
metadata on the manifest, repo-level docs on the companion."* Support contact
is repository-level, so it falls on the companion side of that same line.
Versioned metadata stays banned there.

Field names borrow [CycloneDX's `externalReferences`
vocabulary](https://cyclonedx.org/docs/1.5/json/) — the established naming for
exactly these channels — but stay **flat string keys**. An OCI annotation value
is a string, so a list-of-objects would have to be JSON- or YAML-in-a-string
(Artifact Hub's approach), buying extensibility at the cost of an untrusted
parser on the read path for what is three or four links. A new channel is one
more key.

Read back by `grim describe`, which fetches the companion manifest **only when
the tag listing it already holds shows one exists**, and degrades to empty
channels on any failure rather than failing a describe over optional metadata.

### 5. One tier chain, three authoring surfaces

`--license`, `--repository`, `--authors`, `--vendor`, `--url`,
`--documentation` on `build`/`release`/`publish`; a top-level `[metadata]`
table and a per-entry `[<kind>.<name>.metadata]` override in `publish.toml`.

```
artifact frontmatter > flag > per-entry table > top-level table > derived
```

**Support is the fourth surface and it is not on this chain**: a single
manifest-level `[support]` table, sibling of `[metadata]`, fanning out to every
companion the run publishes — no per-entry `[<kind>.<name>.support]` override,
and no flag tier, because support rides the companion and only `grim publish`
produces one. Keeping it off `[description]` leaves that table's
wholesale-replace rule exactly as shipped (a per-entry table replaces the file
set, support included, would have been the alternative) and keeps it clear of
the "explicit table resolving to zero files is a data error" gate.

Merging is field by field, never wholesale: a per-entry `authors` must not
silently drop the catalog-wide `license`. The `publish.toml` tiers are a pure
convenience layer — they add no capability the flags lack. A default
`repository` lands in the *authored* tier, so it still outranks a git-derived
remote exactly as frontmatter does.

### 6. Recency is joined by the indexer, not carried by grim

`stats.json` gains no grim-side write path. The indexer already runs
`grim describe` per package on every build and already keeps `created` in its
enrich sidecar, so `entries[<ref>].updated` is a pure indexer-side join. The
`metadata.json` pointer is contractually "never versions", so a publish
timestamp must not travel that way.

The pointer does gain `license` and `created`, because `all.json` is the only
browse-time source and a browse row could show neither.

## Consequences

**Positive:**
- Four curated read fields stop being dead code; the extension's "Published"
  row and the indexer's sidecar receive real values
- A support contact is updatable without re-releasing published versions
- Internal forge hostnames and committer names are strictly opt-in, where
  before the only gate was a flag nobody had to remember to *avoid*

**Negative:**
- Re-releasing an existing version from a different commit now needs `--force`
- `grim describe` costs one extra manifest fetch for a repository that
  publishes a companion
- Six new flags on three commands (a generic `--annotation k=v` was rejected
  again — see below)

**Risks:**
- `[support]` fans one contact out to every entry in the manifest, the same
  hazard `[description]` already documents — and unlike `[description]` there
  is deliberately no per-entry override (see § 5). Nothing in-tree needs one;
  adding the key later is purely additive.

## Alternatives considered

- **Wall-clock `created` with a content-identity overwrite guard** — fetch the
  existing manifest on conflict and compare everything except volatile keys.
  The only route to a true publish timestamp; rejected because it trades the
  cleanest invariant in the release path, and a new "volatile annotation"
  concept, for a value the commit date already approximates.
- **Auto-deriving `source` from the `origin` remote** — rejected as an
  infrastructure disclosure on by default (§ 2).
- **Deriving `authors` from git in `auto` mode** — rejected: a person's name in
  every published manifest, by default, is a stronger disclosure than the
  remote URL this ADR gates.
- **Generic `--annotation key=value`** — deferred again (as in
  `adr_git_provenance_annotations.md`). Named flags are discoverable,
  validatable, and documented; the generic form can be added later without
  breaking them.
- **Support metadata on the version manifest** — free to read, but freezes a
  contact into every published tag (§ 4).
- **CycloneDX-shaped list-of-objects for support links** — rejected for the
  untrusted-parser cost (§ 4).

## Links

- [adr_git_provenance_annotations.md](./adr_git_provenance_annotations.md) — the ADR this supersedes in part
- [adr_description_companion.md](./adr_description_companion.md) — the companion's wire format and mutability
- [adr_repository_annotation.md](./adr_repository_annotation.md) — precedent: authored metadata wins over derivation
- [adr_artifact_trust_model.md](./adr_artifact_trust_model.md) — the trust boundary this disclosure reasoning sits inside
- [OCI image spec annotations](https://github.com/opencontainers/image-spec/blob/main/annotations.md)
- [SOURCE_DATE_EPOCH](https://reproducible-builds.org/docs/source-date-epoch/)
