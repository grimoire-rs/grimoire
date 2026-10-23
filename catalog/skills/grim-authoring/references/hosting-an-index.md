# Hosting a Package Index

You loaded this file because you are standing up or operating your own
**package index**: the repository of pointers that `grim search` and the
TUI browse, answering "what packages exist?". This is the operator side.
Consumers only need `grim config registry add` (see
[Consumers](#consumers)).

The sync layout, index root and recovery inputs below need **grim 0.15 or
later** on the consuming side to take effect, and an indexer version that
knows the `sync` block.

Contents: [What an Index Is](#what-it-is) · [Scaffold and Push](#scaffold) ·
[Files and the CI](#files) · [Consumers](#consumers) ·
[Filling the Index](#filling) · [Accepting Contributions](#gate) ·
[Branding and Freshness](#branding) · [Sync Layout](#sync) ·
[Ratings](#ratings) · [Private and Plain Git](#private) ·
[A Marketplace from the Same Repository](#marketplace)

## What an Index Is {#what-it-is}

An index is a repository of pointers with **no server**. Each entry is a
name, a kind, an OCI reference (no tag) and an owner. The forge you
already use builds and serves it. Confirm commands with
`npx @grimoire-rs/indexer init --help`; the narrative here is stable, the
flags are not.

## Scaffold and Push {#scaffold}

```sh
npx @grimoire-rs/indexer init acme-index        # needs Node.js 22.14 or newer
npx @grimoire-rs/indexer build                  # build locally before pushing
git push
```

- `init` asks a few questions. `--quick` skips the wizard for scripts,
  `--forge github|gitlab` picks the pipeline (otherwise derived from the
  `origin` remote), and `--with-skills` scaffolds the combined layout (your
  skills beside the index that lists them, announced into itself).
- **GitHub:** set Settings > Pages > Source to **GitHub Actions**, then
  push to the default branch. **GitLab:** push; the `pages` job publishes
  `public/`.
- Walkthrough: [Host your own index][hosting-an-index] and the tutorial
  [Run your own index][own-index].

## Files and the CI {#files}

On disk: `index/` (one `metadata.json` per package), `index.config.json`
(site identity, branding, the `ci` and `sync` and `ratings` blocks),
`index-policy.json` (the committed allowlist the gate reads),
`publish.toml`, and the forge's workflow files.

- Re-running `init` is safe. It reports each file `unchanged`, `skipped`
  (you edited it) or `preserved`. `publish.toml` and `index-policy.json`
  are **never** overwritten.
- The CI is **rendered from the `ci` block**. `npm run ci` re-renders and
  `npm run ci:check` exits 65 on drift. Never hand-edit the rendered
  workflows. To pick up a renderer fix, bump `@grimoire-rs/indexer`, run
  `npm install` and `npm run ci`, commit. `allowManualEdits` is the escape
  hatch that drops the drift check.
- Details of every `ci` key: [Changing the CI][upgrading].

## Consumers {#consumers}

```sh
grim config registry add acme --index https://acme.github.io/index
```

Consumer-side behavior (cache TTL, `--refresh`, offline) is in
[Consuming an Index][consuming].

## Filling the Index {#filling}

- **By hand:** commit `index/<host>/<namespace>/<package>/metadata.json`.
  The `ref` carries **no tag**, because versions resolve live from the
  registry. Field contract: [index specification][spec-metadata].
- **By announcement:** publishers point `publish.toml` at your index and
  run `grim publish --announce`. grim pushes the packages, then opens a
  pull or merge request with the pointers, forking when needed. Publish
  **before** announcing, because the gate needs at least one registry tag.
  Behavior: [Announcing Packages][announcing].

## Accepting Contributions {#gate}

The scaffolded `validate` job judges every pull request against the
**committed** state, never the proposed one. It refuses a contribution
that touches paths other than a `metadata.json`, claims a namespace whose
forge owner is not the author (checked by **numeric** account id), fails
the schema, points at a registry host outside `index-policy.json`, or names
a reference the registry cannot serve.

- `index-policy.json` carries `registryHosts`, `reservedNamespaces` and
  `trustedBots`. Widening trust is a reviewed commit, not a CI variable.
- `"autoMerge": true` in the `ci` block (then `npm run ci`) squash-merges
  passing contributions. **GitHub only.**
- **Require the `validate` check** in branch protection, together with the
  branch rule. A pull request opened with the built-in token triggers no
  workflows, so a combined-layout announce arrives **ungated** and needs a
  human. On GitLab a merge request supplies its own `.gitlab-ci.yml` and
  can delete the gate job, so enforce it instance-side or treat the GitLab
  gate as advisory.

## Branding and Freshness {#branding}

`index.config.json` is the whole customization surface and every key is
optional (`site`, `brand`, `logo`, `install`, `registry`, `repoUrl`,
`customCss`, `footerLinks`, `ratings`, ...). Keys:
[Make it yours][branding]. `npx @grimoire-rs/indexer enrich` reads
READMEs, changelogs and versions from the registry; the scaffolded CI runs
it before every build, and `"enrich": false` renders a pointers-only
pipeline that never goes online.

## Sync Layout {#sync}

The `sync` block in `index.config.json` decides how a client learns that
nothing changed. Invariants:

- `sync.layout` is `flat` (default) or `anchored`. `tree` is reserved and
  exits 65.
- The index **root** (`c/root.json`) is published in every layout. It must
  be uploaded **last** on hosts without atomic deploy.
- `sync.legacy: false` drops `/all.json` and `/stats.json`. It needs
  `anchored` and must be set **last**: a grim from before the root never
  requests `c/` and fails to refresh against a `c/`-only index. Keep
  `site` set and `enrich` on with it.
- Rollback is redeploying the previous content. It lands on each client's
  next refresh, not instantly.
- The host ceiling (decoded size and entry count) is a number that moves,
  so read it from [The Host Ceiling][sync-ceiling]; the build warns at 80
  percent.
- After an indexer upgrade, run `npm run ci`. The rendered pipeline then
  takes manual-run inputs `fresh` (re-enrich everything) and `purge` (drop
  a package reference or blob digest from the build state). On GitLab these
  are `GRIM_INDEX_FRESH` and `GRIM_INDEX_PURGE`. A mistyped `purge` exits
  65 before anything deploys.

Switching and recovery tables: [Choose a Sync Layout][sync].

## Ratings {#ratings}

Ratings turn the forge into the vote database. Enable them with a
`ratings` block in `index.config.json`, then re-render with `npm run ci`.
Both `site` and at least one `trustedBots[]` entry with a numeric `id` in
`index-policy.json` are required (exit 65 otherwise). A GitLab tally needs
a project access token with `api` scope stored as `GRIM_RATINGS_TOKEN`;
`CI_JOB_TOKEN` is deliberately never read. A consumer reads stats through
the root advertising a stats blob, fetched once per digest. A
git-transport index has no ratings. Do not inline the schedule or resource
group details: [Turn It On][ratings-setup], [GitHub][ratings-github],
[GitLab][ratings-gitlab], [Rolling Back][ratings-rollback]. Voting from
the CLI is `grim rate`, covered by the `grim-usage` skill.

## Private and Plain Git {#private}

- **Private repository, git transport:** skip Pages and give consumers the
  clone URL as the index. grim shallow-clones and authenticates through
  ambient git credentials.
- **Authenticated Pages:** GitLab Pages access control, or GitHub
  Enterprise Cloud for an organization project site. Otherwise a private
  repository's Pages site is still public.
- **Air-gapped:** `index-policy.json` bounds which registries entries may
  name, so an internal index can refuse anything off your mirror.
- **No toolchain:** a git repository containing
  `index/<host>/<ns>/<pkg>/metadata.json` is already a working index
  through the git transport. Reach for the scaffolder only for the catalog
  site, the gate, or both.

See [Private and Internal][private].

## A Marketplace from the Same Repository {#marketplace}

The same index repository can serve a plugin marketplace: commit a
`marketplace.toml` beside the package pointers and deploy the export to a
sibling repository, so bundles reach every supported tool as plugins.
Confirm with `grim export marketplace --help` and read
[hosting-a-marketplace.md](hosting-a-marketplace.md), especially the
department and sibling-deploy sections ([guide anchor][department]).

## Further Reading

- [Host your own index][hosting-an-index] — the full reference.
- [The package index][self-hosting] — the specification and self-hosting.
- [updating.md](updating.md) — refresh protocol for this skill.

[hosting-an-index]: https://grimoire.rs/hosting-an-index.html
[own-index]: https://grimoire.rs/tutorials/own-index.html
[upgrading]: https://grimoire.rs/hosting-an-index.html#upgrading
[branding]: https://grimoire.rs/hosting-an-index.html#branding
[sync]: https://grimoire.rs/hosting-an-index.html#sync
[sync-ceiling]: https://grimoire.rs/hosting-an-index.html#sync-ceiling
[private]: https://grimoire.rs/hosting-an-index.html#private
[consuming]: https://grimoire.rs/package-index.html#consuming
[spec-metadata]: https://grimoire.rs/package-index.html#spec-metadata
[announcing]: https://grimoire.rs/package-index.html#announcing
[self-hosting]: https://grimoire.rs/package-index.html#self-hosting
[ratings-setup]: https://grimoire.rs/ratings.html#setup
[ratings-github]: https://grimoire.rs/ratings.html#setup-github
[ratings-gitlab]: https://grimoire.rs/ratings.html#setup-gitlab
[ratings-rollback]: https://grimoire.rs/ratings.html#rollback
[department]: https://grimoire.rs/guides/hosting-a-marketplace.html#department
