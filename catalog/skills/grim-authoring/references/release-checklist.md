# Release Checklist

You loaded this file because you are about to `grim release` an
artifact, or a build/release just failed and you need the triage table.

Contents: [Pre-Release](#pre-release) · [Metadata Defaults](#metadata-defaults) ·
[Repository Layout](#flat-layout) · [Release Mechanics](#release-mechanics) ·
[Batch Publish](#batch-publish) · [Description Companion](#description-companion) ·
[Exit-65 Triage](#exit-65-triage)

## Pre-Release

Work through these in order; each catches a class of failure before it
reaches a registry:

1. **Catalog metadata authored** — the [default set](#metadata-defaults)
   below, in the right per-kind location: skill/agent → `metadata` map;
   rule → top-level frontmatter; mcp/bundle → top-level TOML. If this
   release retires a package, set the `deprecated` notice in the same
   location (a re-release without it clears the flag), and point consumers
   at the successor with `replaced-by` — authored independently of
   `deprecated`, must parse as a reference (65 otherwise), surfaced by
   `grim search`/`grim describe`. `deprecated` travels into the
   package-index pointer on the next announce, so `grim search`/the TUI
   hide the package — unless it is installed or `show_deprecated` is on —
   without fetching its manifest; `replaced-by` travels alongside it and
   only names the successor.
2. **`grim build <path>` exits 0** — and read the *warnings* too:
   warn-and-drop vendor keys and migration nudges are silent data loss
   if shipped.
3. **Agents: `--kind agent` on both build and release** — a forgotten
   flag publishes a rule, with only a warning. **MCP servers: `--kind
   mcp` likewise** — though there a forgotten flag errors with a hint.
4. **Bundles: members published first** — a bundle referencing unpushed
   members breaks the consumer's `grim lock`, not your release.
5. **`grim release … --dry-run`** — prints the exact push plan: every
   tag and the digest each will point at, without touching the registry.

### Metadata Defaults {#metadata-defaults}

Grim derives what it can, but a derivation is a guess and an unset key is
a missing row in `grim describe` and a blank line in the TUI detail pane.
Author all six on every package — the last two are the ones publishers
routinely skip because the field table calls them optional:

| Field | Why it is not optional in practice |
|---|---|
| `summary` | The single line `grim search` and the TUI show *instead of* the description |
| `keywords` | One comma-separated string (never a YAML/TOML list) — the only thing fuzzy search matches beyond the name |
| `repository` | An `https://` URL (anything else: 65). Also the source `homepage` and `documentation` derive from — one key, three annotations |
| `license` | Never derived. A manifest with no license is one a company's review cannot clear |
| `authors` | **Never derived.** The only automatic source is the commit author under `--git`, which publishes a person's name; a team alias here is what prevents that |
| `vendor` | Derived from the release namespace — author it when that namespace is not your organization's name |

Publishing a set of packages? Put the shared four (`license`,
`repository`, `authors`, `vendor`) in `publish.toml`'s `[metadata]` table
once instead of in every artifact; the table fills only what an artifact
leaves unset. Add `[description]` and the sibling `[support]` table in the
same manifest — [the companion](#description-companion) is what fills the TUI's
`Readme` / `Changelog` panels and the `Overview` panel's support section.

`compatibility` (skills only) and the retirement pair are the exceptions:
publish them when they are true, never as boilerplate.

### Repository Layout: Prefer Flat {#flat-layout}

The default layout puts a kind segment in the path —
`registry/skills/x`, `registry/bundles/y`. It is a **namespace partition,
not a type tag**: grim reads an artifact's kind from the
`com.grimoire.kind` annotation, never from the path. The segment buys one
thing — room for a skill and a bundle to share a name.

With names unique across kinds it buys nothing and still costs: every
reference your users type carries it (a short reference cannot shrink
below `skills/x`), and the TUI tree nests every artifact under a `skills`
/ `bundles` node restating what the row's kind already says — doubled
when `options.tui.group_by_type` is on, giving a `skill` group holding a
`skills` group.

Publish flat by naming your namespace as the prefix in `publish.toml`:

```toml
registry = "ghcr.io"
repository_prefix = "acme"

[skills.code-review]     # → ghcr.io/acme/code-review
[bundles.essentials]     # → ghcr.io/acme/essentials
```

A flat bundle names its members with the same-directory form
`./code-review:0` instead of `../skills/code-review:0`.

**Decide before the first publish.** A repository path is a public
reference the moment a consumer pins it in a `grimoire.lock`; moving it
later means republishing under new names and keeping the old ones alive
for everyone already on them. Full rules: [Batch publishing][batch-publish].

## Release Mechanics

- **Cascade tags.** Releasing `1.2.3` also moves the floating tags
  `1`, `1.2`, and `latest` to the new digest — that is how consumers
  tracking `:1` pick up your patch on `grim update`. Implication: a
  release is immediately visible to every floating consumer; do not
  release a breaking change under the same major.
- **Immutability gate.** An exact-version tag that already exists and
  points at different bytes refuses to move. `--force` overrides —
  use it only to deliberately rewrite history. Re-releasing *identical*
  bytes from the **same commit** is idempotent and always fine; from a
  *different* commit the provenance differs, so the digest does too and the
  release is refused unless `--force` (or `--no-git`, see below).
- **Build provenance is on by default.** `grim build`/`release`/`publish`
  stamp the source commit and its date onto the manifest as OCI annotations.
  Neither value comes from the clock — `created` is the commit's own date, or
  a `SOURCE_DATE_EPOCH` instant outside a repository — which is what keeps
  re-release byte-deterministic.
  - `--git` additionally *requires* provenance (a non-git path then fails,
    65) and discloses the `origin` remote and the commit author's name.
    Those two name infrastructure and a person rather than the artifact, so
    they are never published without asking.
  - `--no-git` suppresses every derived annotation — for a manifest that must
    say nothing about where it was built.

  Confirm the exact behavior with `grim release --help`.
- **Bundles: `--pin`** resolves every floating member to a digest at
  release time for a self-contained, reproducible bundle
  ([pinning][pin]).

## Batch Publish

`grim publish` releases every package declared in a `publish.toml`
manifest: whole-manifest validation before any push, then a fixed kind
order (skills → rules → agents → mcp → bundles) so bundle members always
exist first. Semantics to know ([full reference][batch-publish]):

- **One version for the catalog.** A top-level `version` covers every
  entry that omits its own (or writes the literal `${version}`); an
  explicit per-entry `version` always wins. An entry with no version from
  any source is a data error (65).
- **`--version <ref>` overrides per run** — the CI shape: publish at the
  git tag. The manifest's `version_prefix` (default `"v"`) is stripped
  first, so `--version v1.2.3` publishes `1.2.3`. A semver value cascades
  per entry; a **non-semver** value (`canary`, `edge`) is a movable
  channel tag applied to every entry uniformly, with no cascade.
- **Skip-existing by default.** Entries whose exact-version tag already
  exists are skipped; `--force` moves diverging tags deliberately.
  Channel tags obey the same rule. `--only <name>` (repeatable) publishes
  a subset.
- **Strict schema.** Unknown manifest keys are a hard parse error; entry
  names must match `[a-z0-9][a-z0-9._-]*`. Bind
  `grim schema --kind publish` in your editor to catch typos early.
- **Push vs pull registries.** The manifest `registry` is the canonical
  PULL name baked into every reference, source-annotation fallback,
  pinned member id, announce pointer, and report `ref`. An optional
  `push_registry = "host[/prefix]"` (or `--push-registry`, which wins —
  also on `grim release`) redirects only the network pushes to a
  deviating endpoint (staging registry, internal push URL fronted by a
  mirror). Report entries carry the push-side reference in the
  always-present `pushed_to` field (`null` when inactive). Malformed
  value: 65 before any push.
- **`--announce` needs no fork of your own.** After a fully successful,
  non-dry-run publish it writes the index pointers, pushes a topic
  branch, and opens the PR/MR through the forge API. When the credential
  lacks push access to the index — the normal case for a public one —
  grim forks it into the token's account (reusing an existing fork) and
  opens the request cross-repository against upstream. `[announce] fork
  = "never"` (or the legacy `false`) opts out and pushes straight at the
  index, failing on missing access the way it always did; `"always"`
  forks even when the token can push. The JSON report's `announce.fork` is
  `{repo, created}` when a fork carried the branch, `null` otherwise. An
  announce that fails *after* a successful publish exits 69 — the
  packages are pushed, only the announce needs re-running; announce
  misconfiguration exits 64 ([announcing][announcing]).
- Confirm flags with `grim publish --help`; `--dry-run` prints the full
  plan (including planned description companions) with zero registry
  writes.

## Description Companion

`grim publish` also (re)publishes a **repository description companion**
— a README/logo/changelog channel that works for *every* kind, including
mcp and bundle — to the reserved `__grimoire` tag of each entry's
repository. It is not an artifact: it never installs or resolves, and
the reserved tag is hidden from tag listings. Consumers read it with
`grim fetch <repo> --description`; `grim describe` reports
`has_description` ([full reference][description-companion]).

- **Zero config**: files named `README.md`, `CHANGELOG.md`,
  `assets/logo.*` or `logo.*` next to `publish.toml` publish
  automatically. Probe misses are silent.
- **`[description]` table** decouples layout: `readme`/`logo`/`changelog`
  map onto fixed wire names; `include` globs add extra assets (each keeps
  its manifest-relative path — README-referenced images keep working).
- **Per-entry `[<kind>.<name>.description]` REPLACES the top-level table**
  (it is not merged — repeat shared members); `description = false` opts
  one entry out; top-level `publish = false` kills the companion
  manifest-wide.
- **Hard gates**: an explicit path that does not exist, a table resolving
  to zero files, or any companion path escaping the manifest directory
  (`..`, absolute, symlink escape) is a data error (65). A user-supplied
  tag colliding with the `__grimoire` namespace is a usage error (64).
  A `--dry-run` validates and packs every companion — a bad companion
  fails the dry run too.

## Exit-65 Triage

`grim build`/`grim release` validation failures exit 65 (DataError).
Symptom → cause → fix:

| Symptom (error mentions…) | Cause | Fix |
|---|---|---|
| name "must contain only lowercase…" / separator rules | Charset, leading/trailing or adjacent separators (`--`, `..`, `.-`), > 64 chars | Rename to `[a-z0-9]` runs joined by single `.` or `-` |
| Name mismatch | Skill `name` ≠ directory name, or agent `name` ≠ file stem | Make them equal (rename file/dir or edit frontmatter) |
| Missing frontmatter | Skill without `---` fence, unclosed fence, or agent with no frontmatter | Add the fenced block with required fields |
| Frontmatter parse | Malformed YAML; missing `name`/`description`; empty or > 1024-char description | Fix the YAML; supply required fields |
| Missing `SKILL.md` | Directory built as a skill has no index | Add `SKILL.md` or point at the right path |
| Invalid value for metadata key `repository` | Non-`https://` URL (`git@…`, `http://`) | Use the `https://` forge URL |
| Bad vendor literal (bool/enum/int/float) | Known `<vendor>.<field>` key with an invalid string | Use the registry's accepted literals — see [vendor-metadata.md](vendor-metadata.md) |
| Invalid version / missing tag | Release ref has no tag or a malformed version | Release as `repo:X.Y.Z` |
| Tag exists | Exact-version tag points at different bytes | Bump the version; `--force` only for deliberate rewrites |
| `--git` on a non-git path | `--git` passed to build/release on an artifact path not inside a git repository, or no `git` on the host | Build/release an artifact that lives inside a git repo (with `git` installed), or drop `--git` — the default already derives what it can and never fails (confirm with `grim release --help`) |
| Entry has no version | `publish.toml` entry with no per-entry `version`, no top-level `version`, no `--version` | Set the top-level `version` (or the entry's own) |
| Companion path missing / escapes | Explicit `[description]` path that does not exist, resolves to zero files, or reaches outside the manifest dir (`..`, absolute, symlink) | Move the file inside the manifest directory; copy root-level assets in before publishing |

Bundle source errors (typo'd key, non-qualified member ref, > 512
members) surface as config/parse failures rather than 65 — see
[bundle-spec.md](bundle-spec.md). If the message fits no row, run
`grim build --format json` for the structured detail and check
[publish-time validation][publish-val].

## Further Reading

- [Validate before you push][validate] — the build-then-release flow.
- [Dry runs and overwrites][dry-run] — preview and immutability.
- [Cascade tags][cascade] — what a release actually moves.

[validate]: https://grimoire.rs/publishing.html#validate-before-you-push
[dry-run]: https://grimoire.rs/publishing.html#dry-runs-and-overwrites
[cascade]: https://grimoire.rs/publishing.html#cascade-tags
[pin]: https://grimoire.rs/publishing.html#pin
[publish-val]: https://grimoire.rs/vendor-metadata.html#publish-validation
[batch-publish]: https://grimoire.rs/publishing.html#batch-publish
[description-companion]: https://grimoire.rs/publishing.html#description-companion
[announcing]: https://grimoire.rs/package-index.html#announcing
