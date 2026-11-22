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

## 0.17 — a skill refuses a plugin manifest {#v0-17}

0.17 closes one route for client-loaded code. A skill whose root carries a
Claude plugin manifest directory is [refused](#v0-17-0-skill-plugin-manifest)
at build, publish and install.

### 0.17.0 — unreleased {#v0-17-0}

0.17.0 has one change, and it is [breaking](#v0-17-0-skill-plugin-manifest).

#### A skill can no longer carry a Claude plugin manifest (breaking) {#v0-17-0-skill-plugin-manifest}

A skill package carries instructions, not code a client loads. A skill whose
root holds a `.claude-plugin` entry now exits `65` with the reason
`skill-plugin-manifest`. That covers `grim build`, `grim release`,
`grim publish`, `grim install`, `grim add`, `grim update`, `grim lock`,
`grim export` and the MCP `grim_render` tool. Nothing is pushed, and nothing is
written for any client.

As a publisher, ship client-loaded code as the consent-gated `mod` kind, ship
the plugin through a marketplace, or list the entry in `.grimignore`. A skill
already installed keeps its files, but every install-type run fails with `65`
until `grim update` reaches a clean version or `grim uninstall` removes it. A
grim before 0.17.0 still builds and installs such a skill. See [the upgrade
note][up-skill-plugin-manifest].

## 0.16 — dependencies, harness settings, and a lock that holds still {#v0-16}

0.16 lets an artifact [declare the artifacts it needs](#v0-16-0-dependencies),
and lets you ask [why something is installed](#v0-16-0-dependencies). It adds
[harness settings](#v0-16-0-settings) as an experimental kind. A lock now
[keeps its pins](#v0-16-0-pins) until you run `grim update`. Two changes need
action from publishers: [rules lose their support
directory](#v0-16-0-rule-support-dir) and [an SVG logo beats a
PNG](#v0-16-0-svg-logo).
0.16.1 adds a [filter and sort popup](#v0-16-1-filter-sort) and a [config
dialog](#v0-16-1-config-dialog) to the TUI.

### 0.16.1 — 2026-11-19 {#v0-16-1}

0.16.1 adds a [filter and sort popup](#v0-16-1-filter-sort) and a [config
dialog](#v0-16-1-config-dialog) to the TUI. [Fixes](#v0-16-1-fixes) make
`install`, `uninstall`, `add`, `status`, `why` and the TUI treat dependencies
and nested bundles correctly. The `ocx.sh/grimoire/cli` package is now
[signed](#v0-16-1-more).

#### Pick several kinds and a sort order in one popup {#v0-16-1-filter-sort}

The TUI kind filter held one kind, so you could not see skills and rules
together. The `f` and `s` keys also cycled blindly through their values.

Press `F` to open the **Filter & sort** popup. It holds a checkbox per kind,
the sort modes, the order and a `hide deprecated` toggle. Each change applies
to the catalog behind the popup at once, and `esc` closes it. `f`, `s` and `S`
still work and act on the same state.

The Catalog title names up to three checked kinds and counts more. See [the
filter and sort popup][browse-filter-sort].

#### Edit settings and registries without leaving the TUI {#v0-16-1-config-dialog}

Changing a theme, a sort order or a registry filter meant leaving the browser
for `grim config` or editing `grimoire.toml` by hand.

Press `C` to open the **Config** popup. It lists every key of `grim config
list --all` for the scope the browser is on, project or global. Each save goes through the code behind `grim config set`, so validation matches the
command line and comments in the file survive. The Registries group adds,
edits, removes and re-defaults a registry in a form.

Edits apply to the running browser. Theme, default view, sort order and
direction preview while you move through the pick list, and leaving without
saving puts the old value back. A changed registry list reloads the catalog.
Only a registry's `insecure` flag waits for the next start.

`x` resets a key, and `X` resets all of them after a confirm. See [change
settings in the browser][tui-config].

#### Smaller improvements {#v0-16-1-more}

- The `ocx.sh/grimoire/cli` package is signed and carries a signed SBOM, so
  you can [verify who published it][verify-ocx-package].

#### Fixes {#v0-16-1-fixes}

- `grim install` now refuses a lock whose `requires` rows were removed, even
  for entries it already installed, with exit `65`. The check reads the local
  store, so an unchanged install still sends no request
  ([`grim install`][install]).
- A repository republished as a different kind than you declared now exits
  `65` before grim writes the lock. It used to lock the old kind, and every
  later install failed ([`grim add` exit codes][add-exit-codes]).
- `state.json` no longer changes bytes between `add`, `update`, the TUI and
  `install` when nothing really changed ([records that follow the
  disk][status-adoption]).
- [`grim uninstall`][uninstall] judges every refusal before it deletes the
  first file. A dependency's hand-edited MCP entry now stops the run before
  anything is removed, not halfway through.
- `grim uninstall` warns when it removes a hand-edited file of a dependency or
  bundle member. When it keeps an artifact, the stderr line now names the
  requiring artifact or bundle that holds it ([`grim uninstall`][uninstall]).
- [`grim remove`][remove] warns about each dependency hook the edit drops from
  the lock, since it stays armed until `grim install` or `grim uninstall`
  ([hooks after remove][hooks-remove-armed]).
- [`grim add`][add] lists a bundle the target nests or requires in `pulled_in`
  ([JSON shapes][json-single]).
- `grim add` arms no hook while the consent record misses a declared hook,
  as `grim install` does. It warns and names `grim hook allow` ([workspace
  consent][hooks-consent]).
- [`grim search`][search] and the MCP `grim_search` tool show a bundle's badge
  from its members, nested bundles included.
- [`grim status --check`][status-check] no longer promises an update that
  `grim update` then refuses. Its note now says `grim update` moves the pin, or
  says why it cannot.
- [`grim why`][why] reports overrides and redundancy for a declared bundle that
  another artifact also requires.
- TUI: a bundle's state follows its nested members, and a dependency-only
  artifact reads `via-dependency`. A row with no kind takes it from the lock,
  and a bundle member of a namespaced registry installs. Bundle-provided and
  dependency rows now read `outdated` when an update is available. Nested
  views keep the cursor, the parent label and the pending count ([TUI][tui]).

### 0.16.0 — 2026-11-17 {#v0-16-0}

0.16.0 adds [artifact dependencies and `grim why`](#v0-16-0-dependencies),
[version claims on a requirement](#v0-16-0-version-claims) and the
experimental [`settings` kind](#v0-16-0-settings). Relocks [keep your
pins](#v0-16-0-pins), `--locked` [refuses a stale lock](#v0-16-0-locked), and
[install records follow the disk](#v0-16-0-records).

Two publisher changes are breaking: [a rule's support
directory](#v0-16-0-rule-support-dir) and [SVG logo
precedence](#v0-16-0-svg-logo). The release also brings [per-package READMEs
and logos](#v0-16-0-package-readme), [kind filters in
search](#v0-16-0-search-kind), [TUI themes](#v0-16-0-tui-themes) and
[telemetry config files](#v0-16-0-telemetry-config). Read [behaviour changes
to check](#v0-16-0-behaviour), [smaller improvements](#v0-16-0-more) and
[fixes](#v0-16-0-fixes) too.

#### Declare what an artifact needs, and ask why it is installed {#v0-16-0-dependencies}

A rule that points at a lookup skill, or an agent that breaks without an MCP
server, used to rely on you installing both. Now any artifact can name what
it requires. In a skill, rule or agent that is a `grim.requires.<table>.<name>`
key in the frontmatter `metadata` map:

```yaml
metadata:
  grim.requires.skills.gws-shared: "ghcr.io/acme/skills/gws-shared:1"
```

`grim publish` records the requirement in a second layer. [`grim add`][add]
installs the dependency too and reports what it pulled in. Bundles may nest
bundles. [`grim remove`][remove] and [`grim uninstall`][uninstall] take the
dependencies only the removed artifact needed, and report one another
artifact still needs as `kept-by-dependency`. [`grim status`][status] rows
gain `direct` and `required_by`.

When a dependency is a surprise, ask:

```sh
grim why skill gws-shared
```

`grim why` prints every chain from something you declared to that artifact.
It reads the lock and never contacts a registry. If your own declaration
pins a dependency below what another artifact requested, `add`, `update` and
`lock` now warn and name `grim update` as the remedy
([held-back pins][lock-held-back]).

See [Dependencies between artifacts][guide-dependencies], the
[`requires` reference][requires], and [`grim why`][why]. A lock that records
dependencies cannot be read by grim 0.15.1, and the first write prints one
warning ([upgrade note][up-dependencies]).

#### A tag on a requirement claims a version range {#v0-16-0-version-claims}

Two artifacts that require one package at `:2.1` and `:2.4` could not share
it. Now a tag on a `requires` edge is a Cargo-style claim. `2.1` means
at least 2.1.0 and below 3.0.0, and `0.2` means at least 0.2.0 and below
0.3.0. Both dependents share one release, the newest in the claimed major.
Claims on two different majors still exit `78`.

A claim that would move a pin to a lower major refuses with exit `78` and
`reason: major-downgrade`. Pass `--allow-downgrade` to `grim add`,
`grim update` or `grim lock` to accept it:

```sh
grim update --allow-downgrade
```

See [what the tag claims][tag-claims] and the [upgrade note][up-dependencies].

#### Share harness settings as an experimental artifact kind {#v0-16-0-settings}

A team's permission rules or default model live in each harness's own config
file, copied by hand. A `settings/<name>.toml` package now carries those
typed values. You build, publish, lock, bundle and search it like any other
kind, and grim writes each value into the file the harness reads.

`settings` is **experimental** and off by default. At project scope it also
needs the workspace's consent. A value grim did not write is never replaced
without `--force`.

```sh
grim config set options.experimental.settings true
grim schema --kind settings
```

Thirteen harnesses take settings at global scope. At project scope only
Claude does, through `.claude/settings.local.json`. [Settings][settings]
covers the format, the merge rules and the fields of each harness.
[Share harness settings][share-settings] walks through a package, and the
[client matrix][gap-settings] lists the limits. The kind is [unstable until
1.0][stability-unstable].

#### A relock keeps your pins; only `grim update` moves them {#v0-16-0-pins}

`grim lock`, `grim add`, a relock after `grim remove`, and the TUI actions
used to move a pin whose tag had moved. Now each keeps the existing pin. Only
[`grim update`][update] moves one. Every pin records the tag it resolved
from, and older locks migrate on read. Grim 0.15.1 still reads them.

[`grim remove`][remove], [`grim uninstall`][uninstall] and the TUI delete now
relock in the same run, so they no longer exit `0` with a stale lock. If the
relock fails, nothing changes. `--no-relock` restores the old in-place edit.
On a lock written by grim 0.15 or older, removing a directly declared artifact
that a bundle still provides no longer moves its pin. See [how `grim lock` keeps pins][lock-pins] and the
[upgrade note][up-lock-pins].

#### Refuse an out-of-sync lock with `--locked` {#v0-16-0-locked}

CI wants to know that `grimoire.lock` matches `grimoire.toml`, without a run
quietly fixing it. With `--locked`, or `GRIM_LOCKED=1`, a command that writes
a lock first checks it. A stale lock exits `65` with `reason: stale-lock`. A
missing project lock exits `79`. Nothing is written.

```sh
GRIM_LOCKED=1 grim lock
```

The command's own edit still updates the lock, which differs from Cargo's
`--locked`. See [`--locked`][locked] and [Team CI][team-ci].

#### Install records follow the files on disk {#v0-16-0-records}

After a `git pull`, a branch checkout or a second worktree, the install
record could lag the files, and `grim install` refused with exit `65`. Now
grim renders the locked pin and compares it with what is on disk. Matching
files are adopted and read `unchanged`. A real local edit still refuses with
`65`.

[`grim status`][status] and the TUI write the records they prove. An
unreadable state file exits `74` before the lock or config changes. See
[status adoption][status-adoption] and the [upgrade note][up-records].

#### A rule can no longer ship a support directory (breaking) {#v0-16-0-rule-support-dir}

A rule is one `.md` file. Clients that read `rules/` recursively loaded every
file in a support directory as a rule, so grim no longer packs one. The
directory sharing the rule's stem may hold only `README.md`, `logo.png` and
`logo.svg`. None of them is packed. Any other file makes `grim build`,
`grim release` and `grim publish` exit `65`.

A batch `grim publish` checks every rule and agent before its first push, so a
refused entry leaves nothing published ([validation][publish-validation]).

An agent changed the same way. It packs its `.md` file alone, so its README
and logo are no longer package files. `grim fetch <ref> --path
<name>/README.md` now exits `1`.

Move the files into a skill, and require that skill from the rule:

```yaml
metadata:
  grim.requires.skills.architecture-guide: "ghcr.io/acme/skills/architecture-guide:1"
```

Rules already published keep installing, and grim warns once per such rule.
Republishing unchanged from such a tree gives a new digest. See [the upgrade
note for rules][up-rule-support-dir] and [for agents][up-agent-companions],
and [support files in publishing][rule-support-dir].

#### An SVG logo now wins over a PNG (breaking) {#v0-16-0-svg-logo}

Logo lookup used to take a PNG first. Now `grim publish` tries `logo.svg`
before `logo.png` at every level: inside the package directory, beside the
source, and at the top level. A repository that holds both in one place
publishes the SVG on its next run.

Delete the file you do not want, or name one explicitly:

```toml
[description]
logo = "logo.png"
```

See the [lookup order][description-probe] and the [upgrade
note][up-package-readme].

#### Publish each package's own README and logo {#v0-16-0-package-readme}

A repository with one README and logo per package published the root ones for
every entry. Now `grim publish` looks per entry. It checks the package
directory, then a sibling file such as `<name>.README.md`, then the top
level. README and logo fall back independently. Set `entry_readme = false`
under `[description]` to turn the README lookup off.

A repository with package READMEs publishes a different companion on its next
run, even at an unchanged version. Lookup also works when `path` is spelled
through a symlink. See [each package's own README and logo][entry-readme] and
the [upgrade note][up-package-readme].

#### Filter search by kind {#v0-16-0-search-kind}

A query for `settings` could not tell a skill named `vscode-settings` from
the kind. `--kind` narrows results to skill, rule, agent, bundle, mcp, hook or
settings, and then every query word matches as text:

```sh
grim search --kind skill,rule review
```

Repeat the flag or separate kinds with commas. An unknown kind exits `64`.
The MCP `grim_search` tool takes a `kind` array, and `f` cycles kinds in the
[TUI][tui]. See [`grim search`][search].

#### Pick a TUI theme {#v0-16-0-tui-themes}

`grim tui` was hard to read on a light terminal. The default theme,
`terminal`, now uses your terminal's own colours. Thirteen fixed themes
include `light`, `nord`, `dracula` and the Catppuccin, Gruvbox, Solarized and
Tokyo Night families:

```sh
grim tui --theme nord
grim config set options.tui.theme nord
```

The flag wins over the config key. See [Choose a theme][themes].

#### Point telemetry at your collector with a config file {#v0-16-0-telemetry-config}

Telemetry used to read only `OTEL_*` variables from the shell. A file named by
`GRIM_CONFIG` can now carry a `[telemetry]` table with `enabled`, `endpoint`,
`timeout`, `headers` and more, so a fleet sets one destination for grim alone:

```toml
[telemetry]
enabled = true
endpoint = "https://collector.example"
```

With a file endpoint, grim reads no ambient `OTEL_EXPORTER_OTLP_*`.
`GRIM_TELEMETRY_OTLP_HEADERS` adds a token from the environment. A bad file
warns once and disables telemetry for that run, and `DO_NOT_TRACK` and offline
mode still disarm it. See [the config file][telemetry-config-file] and its
[keys][telemetry-config-keys]. The surface is [unstable until
1.0][stability-unstable].

#### Behaviour changes to check {#v0-16-0-behaviour}

Each of these changes what an existing setup does. None needs a migration
step, but each has an upgrade note.

- A project install now adds your global `options.clients` to the detected
  clients. Set a project `[options].clients` or pass `--client` to opt out
  ([upgrade note][up-global-clients]).
- A stored credential is no longer sent to a host that only a project file
  marks `insecure`. Declare it in `GRIM_INSECURE_REGISTRIES` or the global
  config too ([upgrade note][up-credential]).
- A dev-install that collides with a bundle member or dependency now exits
  `64` instead of silently replacing files. `grim install --force` replaces
  the dev copy ([upgrade note][up-dev-install]).
- A pool-reading client such as OpenCode joins the shared skills pool beside
  a pool writer such as Codex, so a skill installs once. Set
  `options.vendors.<name>.shared_skills` to `false` for its own directory
  ([shared pool][gap-shared-pool], [upgrade note][up-shared-pool]).
- `grim release` and `grim publish` never move `X.Y`, `X` or `latest` to a
  lower version. Kept tags are named in `floats_kept`
  ([upgrade note][up-release-floats]).
- A named pipe at a path grim derives is refused with exit `74` instead of
  hanging every command ([upgrade note][up-non-regular]).

#### Smaller improvements {#v0-16-0-more}

- `grim fetch` and the MCP `grim_fetch` accept layers up to 32 MiB, up from 8
  MiB ([`grim fetch`][fetch]).
- `--log-level` is now applied and wins over `GRIM_LOG`. A value that is not a
  level prints one stderr line, and the run continues
  ([upgrade note][up-smaller]).
- Copilot agents take `copilot.include-custom-instructions`,
  `copilot.reasoning-effort`, `copilot.models` and `copilot.model-policy`.
  Claude agents take `claude.auto-compact-window` ([Claude agent
  keys][vendor-claude-agent], [Copilot agent keys][vendor-copilot-agent]).
- An MCP server's `[server.oauth]` takes an optional
  `authorization_server_issuer`, grim writes Copilot `oauthScopes`, and
  Junie receives `timeout` ([upgrade note][up-smaller]).
- A global agent whose body holds a Kilo `{env:NAME}` or `{file:path}` token
  is skipped with a warning. A `KIRO_HOME` naming a filesystem root, your home
  or an ancestor falls back to `~/.kiro`. Claude marketplace entries move
  their documentation and support links into `plugin.json`
  ([upgrade note][up-smaller]).
- `grim search` and `grim tui` start faster because the global config loads
  once per run.
- At global scope the hook flag-off warning names `--global` in its remedy.

#### Fixes {#v0-16-0-fixes}

- An MCP entry you wrote and grim only adopted is no longer removed when its
  client drops out of the list. `--force` removes it.
- JSONC edits keep a file parseable when a removal sits beside a comment.
  A client config that starts with a byte-order mark is read and edited.
- In a linked worktree, grim writes the ignore line to the shared
  `info/exclude`, where git reads it.
- A client file grim cannot read, or unparseable TOML, no longer reads as
  absent or silently drops the record.
- `grim update --global` lists the MCP entries and edited files a relocated
  vendor root kept.
- A vendor's MCP decline line prints once per run.
- A settings package cached kindless by 0.15.1 is read again once, so
  `--kind` finds it.
- `grim publish --announce` no longer opens a new pull request when only the
  commit date changed.
- A `grim fetch` miss names the right kind, and a miss on the reserved
  companion tag says no description is published.
- A state file from `git clone` can no longer make grim act on a hostile
  output. Grim drops it with one warning.
- Marketplace runs relock a stale or missing part they did not pick, and
  Claude plugin links pass `claude plugin validate`.
- TUI: the held-delete remedy fits 80 columns offline, and the selected row
  keeps its status readable.
- A stored blob is never replaced, which fixes concurrent runs failing with
  "Access is denied" on Windows.
- `rmcp` moved to 3.9.0 to clear three RUSTSEC advisories.
- [`grim build`][build] exits `65` for a `grim.requires.*` key written twice
  in frontmatter. It used to keep the last value and exit `0`.
- [`grim search`][search] takes several unquoted words, so `grim search skill
  review` works.
- The [MCP server][mcp] declares the `tools` capability when a client sends
  `initialize`.

## 0.15 — marketplaces, hooks, and catalogs of any size {#v0-15}

0.15 turns a `marketplace.toml` into [a git repository your team adds as a
plugin marketplace](#v0-15-0-marketplace). It adds [experimental
hooks](#v0-15-0-hooks) that answer an agent's permission questions, and lets
[`grim search` page through catalogs of any size](#v0-15-0-large-catalogs).
0.15.1 makes grim [leave your `grimoire.toml` comments
alone](#v0-15-1-toml-comments).

### 0.15.1 — 2026-10-28 {#v0-15-1}

0.15.1 [keeps your comments when grim edits `grimoire.toml`](#v0-15-1-toml-comments)
and ships [a native hook launcher](#v0-15-1-launcher). A failing gatekeeper
hook is now [withheld](#v0-15-1-gatekeeper), and
[smaller improvements](#v0-15-1-more) cover config and error metadata.

#### grim no longer strips comments from grimoire.toml {#v0-15-1-toml-comments}

Every `grim add`, `grim remove`, `grim uninstall` and `grim config` write used
to re-serialize the whole file. Your comments vanished and the tables were
re-sorted, so a hand-annotated `grimoire.toml` looked different after one
command.

Now grim edits the file in place. Only the lines whose meaning changed move.
Comments, blank lines and key order survive. A changed value keeps its
trailing comment, and a new entry goes at the end of its table. See
[`grimoire.toml`][grimoire-toml].

#### Hooks run through a native launcher {#v0-15-1-launcher}

The `sh` shim that every hook registration called is replaced by a small
native `grim-hook` executable embedded in grim. It needs no `grim` binary on
the path and starts without a shell. `grim install` writes it and rewrites
the registrations that pointed at the old shim. Its exit codes are listed
under [launcher exit codes][launcher-exit].

Codex keys its stored approval on the exact command text, so it asks you to
approve each gatekeeper hook once more. Open Codex's `/hooks` screen
([Codex no longer trusts the hook command][codex-retrust]).

#### A gatekeeper whose handler fails is withheld {#v0-15-1-gatekeeper}

A gatekeeper hook answers an agent's permission question. If its handler
crashed, timed out or printed no usable verdict, the launcher could still
report a verdict. Now it withholds the gatekeeper and the client's own policy
decides. See [exit code and timeout][handler-exit]. Hooks remain workflow
automation, not a policy control.

#### Smaller improvements {#v0-15-1-more}

- `grim config list --format json` items carry `section` and `advanced`, so
  a settings UI can group keys without guessing from prefixes ([JSON
  output][config-json]).
- A missing `grimoire.lock` now fails with `reason: no-lock` (still exit
  `79`), so a client can offer "run `grim lock`" without matching message
  text ([the `reason` field][error-reason]).
- `grim search` no longer misaligns a table when a result cell holds control
  characters.
- Plugin READMEs from `grim export` drop the Contents list and size the logo.
- `grim status` resolves cached MCP descriptors when it computes
  `outputs_pending`, so MCP outputs no longer show as pending.
- `grim search` and catalog refreshes parse and clone less, which helps large
  catalogs.

### 0.15.0 — 2026-10-23 {#v0-15-0}

0.15.0 adds [`grim export marketplace`](#v0-15-0-marketplace),
[experimental hooks](#v0-15-0-hooks), [paged search over uncapped
catalogs](#v0-15-0-large-catalogs), [opt-in usage
telemetry](#v0-15-0-telemetry) and [gzip-compressed layers](#v0-15-0-gzip).
It also fixes a [set of install, config and catalog
bugs](#v0-15-0-fixes).

#### Host a plugin marketplace from one manifest {#v0-15-0-marketplace}

[`grim export plugin`][export-plugin] hands one plugin over as a folder or a
zip. A team wants an address instead: add it once, receive updates.

[`grim export marketplace`][export-marketplace] regenerates a git repository
from the plugins a `marketplace.toml` declares. It writes one marketplace
file per client (Claude Code, Copilot, Codex, Qoder and Cursor) and one plugin
tree per plugin and client. A root README tells visitors how to add the
marketplace. You commit the result, and a consumer adds the repository
with their client's own commands, with no grim installed.

```sh
grim export marketplace --marketplace tools/marketplace.toml -o ../acme-marketplace
```

Each plugin can now carry an author, licence, links and keywords, and grim
writes them into every client's files ([plugin metadata][plugin-metadata]).
Qoder joins [`grim export plugin`][export-plugin], and Cursor trees get a
`.cursor-plugin/plugin.json`. [Host a plugin marketplace][host-marketplace]
covers the scheduled regenerate job and the required check. [Use a plugin
marketplace without grim][use-marketplace] is the page to send your users.

Two things change for existing exports. A plugin's version suffix now hashes
the rendered tree, so every plugin you exported before gets a new suffix once
([upgrade note][plugin-version-move]). A configured `qoder` client is no
longer skipped on export ([upgrade note][qoder-plugin-export]). A manifest
that uses the new `[marketplace]` table or plugin metadata keys also needs
grim 0.15.0 everywhere it is read ([`[marketplace]`][marketplace-needs-new],
[metadata keys][plugin-metadata-needs-new]).

#### Let a hook answer your agent's permission questions {#v0-15-0-hooks}

An agent asks before it runs a shell command, and by the twentieth ask you
click yes without reading. A hook is a small program the client runs before
or after a tool call. Its answer lets the call run, asks you, or refuses it.
Publish a `hook.toml` once, and grim translates it into each client's own
hook format.

Hooks are a new, **experimental** artifact kind. They execute code, so grim
arms one only after you switch the feature on and consent for the workspace:

```sh
grim config set options.experimental.hooks true
grim add --kind hook ghcr.io/acme/hooks/approve-safe-commands:1
```

[Control agent commands with hooks][hooks-guide] installs one and writes
another. The [hooks reference][hooks-reference] lists the gates, the handler
contract and each client's limits, and [artifact formats][artifacts-hooks]
lists the manifest fields. The first-party catalog now publishes two example
hooks, `command-guard` and `tool-call-logger`. Plugin export omits hook
members. The kind is [unstable until 1.0][stability-unstable].

#### Page through catalogs of any size {#v0-15-0-large-catalogs}

An index source stopped at 500 entries, and an OCI registry's `_catalog` and
`tags/list` listings stopped at their first page. A catalog larger than that
looked complete and was not.

grim now lists every entry an index publishes, and follows a registry's own
paging to the end. A long OCI build checkpoints its rows, so an interrupted
run resumes on the next command. Other commands serve the rows built so far
and read `capped: true` for the source until it finishes. A refresh that
fails serves the cached catalog instead of an error.

[`grim search`][search] pages the result, and the JSON envelope reports
`total` and `truncated`:

```sh
grim search --sort rating --limit 10 --offset 10
```

The MCP `grim_search` tool takes the same `limit` and `offset`. See [paging
`search` results][search-paging], and [browse filters][browse-filters] for
what a filter does and does not narrow. A refresh of an HTTP package index
also starts with a root probe and backs off from a source that keeps failing
([upgrade note][index-sync]). [`grim context`][context] reports each source's
last sync.

#### Send anonymous usage telemetry to your own collector {#v0-15-0-telemetry}

You can now see how grim is used across a team without a vendor in the
middle. Set `GRIM_TELEMETRY` and point `OTEL_EXPORTER_OTLP_ENDPOINT` at an
OpenTelemetry collector you run. grim sends one record per command there.

It is off by default, and `DO_NOT_TRACK`, `OTEL_SDK_DISABLED` and offline mode
switch it off again. There is no default endpoint, and nothing leaves the
machine for any other address. [Telemetry][telemetry] lists every field, what
is never collected, and how to audit the bytes. The surface is [unstable until
1.0][stability-unstable].

#### Publish smaller layers with gzip {#v0-15-0-gzip}

A skill, rule, agent or hook packs into one uncompressed tar layer. Pass
`--compression gzip`, or a bare `--compression`, to publish a compressed layer
instead. Skills shrink about threefold, and small rules and agents about
sixfold.

```sh
grim release ./code-review ghcr.io/acme/code-review:1.2.3 --compression gzip
```

Default output is unchanged. A grim before 0.15.0 cannot read a gzip layer, so
upgrade every machine that installs the artifact first
([upgrade note][compressed-layers]). See [compressing layers][compression].

#### Skills that know this release {#v0-15-0-skills}

The first-party skills publish with 0.15.0. `grim-authoring` gains references
for hosting a plugin marketplace and a package index. `grim-usage` gains
references for running a marketplace and for telemetry. Run `grim update` to
pick them up.

#### Fixes {#v0-15-0-fixes}

- [`grim rate`][rate] refetches stats once before it reports a row unrated.
- A relative `--config` now resolves to an absolute workspace.
- A declaration key that could escape the install root is refused.
- Index requests refuse an https-to-http redirect, and errors and logs redact
  index locators.
- Plain-table cells escape hidden and control characters.
- The [TUI][tui] title row and tree labels fit the terminal width.
- Publish metadata defaults now apply to hook packages.
- Dependency updates close RUSTSEC-2026-0346 to 0349, 0367, 0382, 0383 and
  0385.

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
[export-marketplace]: ./commands.md#export-marketplace
[search-paging]: ./json-interface.md#search-paging
[context]: ./commands.md#context
[rate]: ./commands.md#rate
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
[plugin-metadata]: ./configuration.md#plugin-metadata
[compression]: ./publishing.md#compression

<!-- publishing / index -->

[git-provenance]: ./publishing.md#git-provenance
[support-channels]: ./publishing.md#support-channels
[announcing]: ./package-index.md#announcing

<!-- ratings -->

[ratings-setup]: ./ratings.md#setup
[voting-host]: ./ratings.md#voting-host

<!-- clients / artifacts / hooks -->

[hooks-guide]: ./guides/agent-hooks.md
[hooks-reference]: ./hooks.md
[artifacts-hooks]: ./artifacts.md#hooks
[host-marketplace]: ./guides/hosting-a-marketplace.md
[use-marketplace]: ./guides/use-a-marketplace.md
[telemetry]: ./telemetry.md
[stability-unstable]: ./stability.md#unstable
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
[plugin-version-move]: ./upgrading.md#plugin-version-move
[qoder-plugin-export]: ./upgrading.md#qoder-plugin-export
[marketplace-needs-new]: ./upgrading.md#marketplace-table-needs-new-grim
[plugin-metadata-needs-new]: ./upgrading.md#plugin-metadata-needs-new-grim
[compressed-layers]: ./upgrading.md#compressed-layers-need-new-grim
[index-sync]: ./upgrading.md#index-sync

<!-- json interface -->

[error-reason]: ./json-interface.md#error-reason
[config-json]: ./commands.md#config-json
[grimoire-toml]: ./configuration.md#grimoire-toml
[launcher-exit]: ./hooks.md#launcher-exit
[handler-exit]: ./hooks.md#handler-exit
[codex-retrust]: ./hooks-troubleshooting.md#codex-retrust

<!-- 0.16 -->

[why]: ./commands.md#why
[remove]: ./commands.md#remove
[update]: ./commands.md#update
[lock-pins]: ./commands.md#lock-pins
[lock-held-back]: ./commands.md#lock-held-back
[locked]: ./commands.md#locked
[status-adoption]: ./commands.md#status-adoption
[fetch]: ./commands.md#fetch
[requires]: ./artifacts.md#requires
[tag-claims]: ./artifacts.md#what-the-tag-claims
[guide-dependencies]: ./guides/dependencies.md
[settings]: ./settings.md
[share-settings]: ./guides/share-settings.md
[gap-settings]: ./clients.md#gap-settings
[team-ci]: ./guides/team-ci.md
[rule-support-dir]: ./publishing.md#rule-support-dir
[description-probe]: ./publishing.md#description-probe
[entry-readme]: ./publishing.md#description-entry-readme
[themes]: ./browse.md#themes
[telemetry-config-file]: ./telemetry.md#config-file
[telemetry-config-keys]: ./telemetry.md#config-file-keys
[vendor-claude-agent]: ./vendor-metadata.md#claude-agent-registry
[vendor-copilot-agent]: ./vendor-metadata.md#copilot-agent-registry
[up-skill-plugin-manifest]: ./upgrading.md#skill-plugin-manifest
[up-dependencies]: ./upgrading.md#dependencies-need-new-grim
[up-lock-pins]: ./upgrading.md#lock-keeps-pins
[up-records]: ./upgrading.md#records-follow-disk
[up-rule-support-dir]: ./upgrading.md#rule-support-dir
[up-agent-companions]: ./upgrading.md#agent-companions
[up-package-readme]: ./upgrading.md#package-readme-companion
[up-global-clients]: ./upgrading.md#global-clients-join
[up-credential]: ./upgrading.md#credential-project-insecure
[up-dev-install]: ./upgrading.md#dev-install-collision
[up-shared-pool]: ./upgrading.md#shared-pool-auto-join
[up-release-floats]: ./upgrading.md#release-floats
[up-non-regular]: ./upgrading.md#non-regular-file
[up-smaller]: ./upgrading.md#smaller-notes
[build]: ./commands.md#build
[mcp]: ./commands.md#mcp
[publish-validation]: ./publishing.md#batch-publish-validation
[browse-filter-sort]: ./browse.md#filter-sort
[tui-config]: ./browse.md#tui-config
[verify-ocx-package]: ./installation.md#verify-ocx-package
[status-check]: ./commands.md#status-check
[add-exit-codes]: ./commands.md#add-exit-codes
[json-single]: ./json-interface.md#shapes-single
[hooks-consent]: ./hooks.md#consent
[hooks-remove-armed]: ./hooks.md#remove-stays-armed
