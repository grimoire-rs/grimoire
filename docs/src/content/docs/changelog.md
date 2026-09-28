---
title: "Changelog"
description: "What changed for you in each grim release, by outcome, with links to the pages that show how. The commit-level record lives in CHANGELOG.md."
tableOfContents:
  minHeadingLevel: 2
  maxHeadingLevel: 3
---
<!-- doc_type: changelog -->

This page tracks what changed for **you** in each release: a new command, a
new client, or a behavior worth knowing about. It links to the page that
shows how. [`CHANGELOG.md`][changelog] has every change at commit
granularity, including the refactors and internal work this page leaves out.

## 0.14 — ratings, downloads, and exporting a project as a plugin {#v0-14}

0.14 gives every catalog browse [a rating and a download count](#v0-14-0-ratings).
You can now [export a locked project as a plugin](#v0-14-3-export) for a
client with no OCI support of its own. It also lets you vote through
[the forge the index declares](#v0-14-0-ratings) instead of an environment
variable.

### 0.14.3 — 2026-09-28 {#v0-14-3}

0.14.3 lets you [export a project as a plugin](#v0-14-3-export) and adds
[Qoder support](#v0-14-3-qoder). It also improves [how search
matches](#v0-14-3-search-prefix) and [batch actions in the
TUI](#v0-14-3-tui-confirm).

#### Share a project as a plugin, no registry required {#v0-14-3-export}

Some clients only install [Claude-family or Agent Plugins][export-plugin],
not grim itself. You no longer need to publish anything to hand them your
skills, rules, and MCP servers.

```sh
grim export plugin --project --zip -o dist/
```

[`grim export plugin`][export-plugin] renders your project's locked
artifacts as a plugin directory or zip. `--project` uses your project's own
`grimoire.lock`, so what ships is exactly what `grim install` would put on
disk. A logo, a generated README, and per-client omissions all come along.
See [output naming and placement][export-plugin-output]. For a catalog you
maintain across several skills, [`grim update --marketplace`][update-marketplace]
re-resolves a whole `marketplace.toml` at once.

#### Confirm before acting on more than one artifact {#v0-14-3-tui-confirm}

Marking several rows in the [TUI][tui] and pressing `i`/`u`/`d` now opens a
confirmation summary naming the count and source. Nothing installs or
uninstalls until you confirm.

#### Search matches by word prefix {#v0-14-3-search-prefix}

[`grim search`][search] and the TUI's `/` filter now match a term against
the start of a word, not just any substring, and rank an exact name match
first.

#### Qoder support {#v0-14-3-qoder}

[Qoder][clients-matrix] joins the client roster, installing skills, rules,
agents, and MCP servers.

#### Smaller improvements {#v0-14-3-more}

- `-g` is now the short form of `--global` on every command.
- [`.grimignore`][grimignore] is honored when grim packs and hashes a local
  skill, matching what `grim build` already excluded from the packed tree.

### 0.14.2 — 2026-09-13 {#v0-14-2}

0.14.2 adds [download counts and browse ordering](#v0-14-2-downloads) to
the catalog, and lets you [skip the announce pointer](#v0-14-2-announce-skip)
for one published entry.

#### Sort the catalog by rating or downloads {#v0-14-2-downloads}

Every index-backed registry can now report a download count alongside the
rating 0.14.0 added, and you can browse by either:

```sh
grim search --sort downloads
grim tui --sort rating
```

[`--sort`][search-sort] takes `name`, `updated`, `rating`, or `downloads`.
In the TUI, `s` cycles it live and `S` flips the direction. `Home`/`End`
jump to the first and last row.

`[options.tui].sort` and `sort_order` seed the startup order, so you don't
have to set it every time. A row the source never counted sorts last, never
as zero. See the [downloads object][search-downloads] in the JSON
reference.

#### Skip the announce pointer for one entry {#v0-14-2-announce-skip}

Publishing a package you would rather not open a pull request for on every
release? An entry-level `announce = false` in `publish.toml` skips just
that [announce pointer][announcing] while still publishing the package
itself.

### 0.14.1 — 2026-09-01 {#v0-14-1}

0.14.1 [removes `GRIM_RATING_HOST`](#v0-14-1-rating-host) and improves
[status and search reporting](#v0-14-1-more).

#### Breaking change: `GRIM_RATING_HOST` is removed {#v0-14-1-rating-host}

The forge `grim rate` votes against now comes from the package index's own
`stats.json` sidecar, not an environment variable. See
[Upgrading][rating-host] for what changes if you set it. `--token-host` no
longer requires `--token-stdin`.

#### Smaller improvements {#v0-14-1-more}

- [`grim status`][status] now reports update availability for a bundle and
  its members, not just standalone artifacts.
- [`grim search`][search]'s JSON output reports per-source load status. A
  registry that failed to load is now visible instead of silently thinning
  the results.
- GitHub Enterprise data residency (`*.ghe.com`) is handled the same way as
  `github.com` when [voting][voting-host] against a private instance.

### 0.14.0 — 2026-08-28 {#v0-14-0}

0.14.0 adds [ratings you can vote on](#v0-14-0-ratings), stamps
[build provenance by default](#v0-14-0-provenance), and publishes
[repository support channels](#v0-14-0-support-channels).

#### See what other people think before you install {#v0-14-0-ratings}

A skill's rating now travels with the catalog, and you can vote on one
without leaving the terminal:

```sh
grim rate ghcr.io/acme/code-review --up
grim search --sort rating
```

[Ratings][ratings-setup] read from the index's `stats.json` sidecar. Voting
goes through the forge your index declares, GitHub Discussions or a GitLab
work item, with no server of grim's own in the loop. [`grim search`][search]
and the [TUI][tui] both show the score and can [order by it][search-sort].

#### Default build provenance {#v0-14-0-provenance}

`grim build`, `release`, and `publish` now stamp
`org.opencontainers.image.revision` and `…created` onto every release by
default, derived from the commit. No `--git` flag is needed.
[`--no-git`][git-provenance] restores the old, bare manifest. See [what
changes for an existing publisher][default-provenance].

#### Repository support channels {#v0-14-0-support-channels}

A manifest-level [`[support]` table][support-channels] publishes issue
tracker, chat, contact, and security links for a whole repository, read
back with `grim describe`. The [TUI][tui-detail-tabs] detail pane gained
matching tabs for repository docs and support channels, alongside the
compatibility annotation.

#### Smaller improvements {#v0-14-0-more}

- [`grim install`][install] now restates a dropped `paths:` scope in the
  rule body when a client can't represent it natively, instead of silently
  dropping the restriction.
- [`grim status`][status] names the cause when an unrecognized key blocks a
  report.

## 0.13 — narrowing a shared registry to what you browse {#v0-13}

0.13 adds [per-registry browse filters](#v0-13-0-filters) and
[fuzzy, ranked search](#v0-13-0-search), and restores
[the update integrity gate](#v0-13-0-update-integrity) that keeps a
hand-edited install from being silently overwritten.

### 0.13.0 — 2026-08-13 {#v0-13-0}

0.13.0 adds [browse filters](#v0-13-0-filters) and
[ranked search](#v0-13-0-search), and brings back
[the update integrity gate](#v0-13-0-update-integrity).

#### Filter what a registry shows you {#v0-13-0-filters}

A shared or public registry usually carries more than you personally
browse. [Browse filters][browse-filters] narrow it, per registry:

```sh
grim config registry set acme --include "skills/**" --exclude "**/internal-*"
```

Patterns match against both the repository path and the full reference.
[`grim config registry set`][config-registry-set] edits an existing entry
in place, so you don't need to remove and re-add it.

#### Search that ranks, not just matches {#v0-13-0-search}

[`grim search`][search] and the TUI's `/` filter now match fuzzily and rank
by relevance, instead of a plain substring check. JSON output attributes
every hit to the registry it came from.

#### The update integrity gate protects local edits {#v0-13-0-update-integrity}

`grim update` now runs through [the same integrity gate as
`install`][update-integrity]: a locally modified artifact is refused until
`--force`, instead of being overwritten on every re-resolve.

#### Smaller improvements {#v0-13-0-more}

- [`grim status`][status] reports `outputs_pending`: materialization drift
  the current install record doesn't cover, such as a newly supported
  client or a moved layout. No `--check` flag is needed.
- `insecure` is now a [per-registry config field][browse-filters], not only
  an environment variable.

## 0.12 — per-client settings, and six more clients {#v0-12}

0.12 adds an [`options.vendors.<name>` table](#v0-12-0-vendors) for
settings that only make sense for one client, and ships support for
[six more clients](#v0-12-0-clients).

### 0.12.1 — 2026-08-02 {#v0-12-1}

0.12.1 brings small fixes to [uninstall and install-state
reporting](#v0-12-1-more).

#### Smaller improvements {#v0-12-1-more}

- [`grim uninstall`][uninstall] gained a `--force` gate on a managed MCP
  entry that drifted from what grim recorded, instead of refusing outright.
- [`grim status`][status] records whether an installed output was adopted
  from an existing file or written fresh by grim.

### 0.12.0 — 2026-08-01 {#v0-12-0}

0.12.0 adds [per-client settings](#v0-12-0-vendors) and brings
[six more clients](#v0-12-0-clients) into the roster.

#### Settings that only apply to one client {#v0-12-0-vendors}

[`[options.vendors.<name>]`][options-vendors] holds a setting that makes
sense for exactly one client. `shared_skills` is the first: it opts a
[skills-only client][gap-shared-pool] into the cross-vendor pool at
`~/.agents/skills` instead of its own native directory.

#### Six more clients {#v0-12-0-clients}

grim now installs into six more clients. [Goose, Warp, Droid, OpenClaw,
Kilo, and Cline][gap-skills-only] install skills only, and the
vendor-neutral `agents` target and [Google Antigravity 2.0][clients-matrix]
round out the list.

#### Smaller improvements {#v0-12-0-more}

- Junie's rules now host at project scope and decline at global, matching
  what upstream actually reads.

## 0.11 — ten clients, styled output, shell completions {#v0-11}

0.11 rounds the client roster out to ten with
[five more clients](#v0-11-0-clients) and adds
[`--color` and `grim completions`](#v0-11-0-cli).
It also lets [`--announce` fork automatically](#v0-11-0-autofork)
when you lack push access.

### 0.11.1 — 2026-07-23 {#v0-11-1}

0.11.1 adds [a fork policy for `--announce`](#v0-11-1-fork-policy).

#### `--announce` gets a fork policy {#v0-11-1-fork-policy}

`grim publish --announce` gained a fork policy: `never`, `auto`, or
`always`. It controls when grim forks the [index repository][announcing]
before opening a pull or merge request, instead of always deciding for you.

### 0.11.0 — 2026-07-22 {#v0-11-0}

0.11.0 adds [five more clients](#v0-11-0-clients),
[colored output and completions](#v0-11-0-cli), and lets
[`--announce` fork automatically](#v0-11-0-autofork) when you lack push
access.

#### Five more clients {#v0-11-0-clients}

[Cursor, Gemini CLI, Kiro, Junie, Zed, and Amp][clients-matrix] join Claude
Code, OpenCode, Copilot, and Codex as install targets. Each renders rules,
agents, or MCP servers into its own native format.

#### Colored output and shell completions {#v0-11-0-cli}

```sh
grim search --color always
grim completions zsh > _grim
```

`--color <auto|always|never>` styles both `--format json` and clap's own
help text. `auto` honors `NO_COLOR` and detects a non-TTY automatically.
[`grim completions`][completions] prints a completion script for bash,
zsh, fish, elvish, or PowerShell.

#### `grim publish --announce` forks automatically {#v0-11-0-autofork}

Lacking push access to the [index repository][announcing] no longer fails
the run. `--announce` now forks it for you, reusing an existing fork when
you already have one.

## 0.10 — Codex support, license annotations, a push/pull registry split {#v0-10}

0.10 adds [OpenAI's Codex CLI as a client](#v0-10-0-codex) and stamps
[a license annotation](#v0-10-0-license) on every release. It also lets
[the registry you push to differ from the one your references point
at](#v0-10-0-push-registry).

### 0.10.0 — 2026-07-19 {#v0-10-0}

0.10.0 adds [Codex as a client](#v0-10-0-codex) and lets you [push to one
registry while referencing another](#v0-10-0-push-registry). It also
stamps [license annotations](#v0-10-0-license) and makes
[`grim login` verify your credential](#v0-10-0-login-verify) by default.

#### Codex joins the client roster {#v0-10-0-codex}

[OpenAI Codex CLI][gap-codex-rules] is now a supported install target for
skills, agents, and MCP servers. Codex has no path-scoped rule surface, so
rules decline there (see the gap note).

#### Push to one registry, reference another {#v0-10-0-push-registry}

Maybe your pipeline pushes through a staging registry that mirrors to a
public one. Or maybe it goes through an internal push endpoint fronted by a
read-only mirror. Either way, your references no longer have to follow it:

```toml
registry = "ghcr.io"                      # the canonical name in your refs
push_registry = "staging.example/mirror"  # where publish actually pushes
```

[`push_registry`][push-registry] (and the matching `--push-registry` flag on
`grim release`) splits the two roles. Every reference, pinned bundle member,
and announce pointer keeps the canonical pull name.

#### Release manifests carry a license annotation {#v0-10-0-license}

Every release now emits a [license annotation][annotations] for rules,
agents, bundles, and MCP descriptors, not just skills.

#### `grim login` verifies your credential first {#v0-10-0-login-verify}

[`grim login`][login-verify] now verifies a credential against the
registry before storing it, by default, instead of storing whatever you
typed and failing later.

#### Smaller improvements {#v0-10-0-more}

- [Artifact names][names] may now contain periods (`socket.io`), a
  deliberate superset of the stricter Agent Skills spec.
- [`grim add --force`][add] recovers a locally modified declaration instead
  of refusing it.
- `grim schema --kind mcp` prints the [MCP descriptor's JSON Schema][schema].
- An error document can now carry a `retryable` field, so a client knows
  whether to back off and retry. See [the error document][error-reason].

## Earlier releases {#earlier}

Releases before 0.10.0 predate this page. [`CHANGELOG.md`][changelog] has
the full commit-level record back to the first tagged release.

<!-- internal -->

[changelog]: https://github.com/grimoire-rs/grimoire/blob/main/CHANGELOG.md

<!-- commands -->

[export-plugin]: ./commands.md#export-plugin
[export-plugin-output]: ./commands.md#export-plugin-output
[update-marketplace]: ./commands.md#update-marketplace
[search]: ./commands.md#search
[search-sort]: ./commands.md#search-sort
[search-downloads]: ./json-interface.md#search-downloads
[tui]: ./commands.md#tui
[tui-detail-tabs]: ./commands.md#tui-detail-tabs
[status]: ./commands.md#status
[install]: ./commands.md#install
[uninstall]: ./commands.md#uninstall
[add]: ./commands.md#add
[schema]: ./commands.md#schema
[completions]: ./commands.md#completions

<!-- configuration -->

[browse-filters]: ./configuration.md#browse-filters
[config-registry-set]: ./commands.md#config-registry
[options-vendors]: ./configuration.md#options-vendors
[push-registry]: ./publishing.md#batch-publish-push-registry

<!-- publishing / index -->

[git-provenance]: ./publishing.md#git-provenance
[support-channels]: ./publishing.md#support-channels
[announcing]: ./package-index.md#announcing

<!-- ratings -->

[ratings-setup]: ./ratings.md#setup
[voting-host]: ./ratings.md#voting-host

<!-- clients / artifacts -->

[clients-matrix]: ./clients.md#matrix
[gap-skills-only]: ./clients.md#gap-skills-only
[gap-shared-pool]: ./clients.md#gap-shared-pool
[gap-codex-rules]: ./clients.md#gap-codex-rules
[names]: ./artifacts.md#names
[annotations]: ./artifacts.md#annotations
[grimignore]: ./artifacts.md#grimignore

<!-- auth -->

[login-verify]: ./authentication.md#login-verify

<!-- upgrading -->

[rating-host]: ./upgrading.md#rating-host
[update-integrity]: ./upgrading.md#update-integrity
[default-provenance]: ./upgrading.md#default-provenance

<!-- json interface -->

[error-reason]: ./json-interface.md#error-reason
