---
title: "Upgrading"
description: "CHANGELOG.md lists every change, one line per commit — this page covers upgrading grim itself version to version."
---
<!-- doc_type: troubleshooting -->

[`CHANGELOG.md`][changelog] lists every change, one line per commit. This page
carries the ones that need more than a line: a behaviour you will notice on
the first command after an upgrade, and what to do about it.

Almost nothing here is a breaking change. Grimoire is [stabilizing toward
1.0][stability] and evolution is additive-only — but "additive" is a statement
about contracts, not about what you *see*, and every one of these is visible
enough to look like a bug if you meet it cold. The one exception is
[`GRIM_RATING_HOST`](#rating-host), removed before the 1.0 freeze takes
effect and called out as breaking where it appears.

## Adding a client changes what autodetect targets {#autodetect}

grim installs into every AI client it detects, unless you pin the set with
[`--client`][install] or `[options].clients`. Supporting a new client
therefore changes what an existing project installs into, the moment that
client's marker is already on disk.

**What you will see.** An install that reported `already-installed` now
reports `installed` or `updated`, because the recorded install no longer
covers every target. No schema or status literal changed — those values
already existed. The alternative would be a newly supported client staying
invisible until you intervened, which is worse.

**The markers that newly trigger detection**, by scope:

| Scope | Marker |
|---|---|
| Project | `.cline` or `.clinerules` (Cline) |
| Project | `.factory` (Droid) |
| Project | `.goose` (Goose) |
| Project | `.warp` (Warp) |
| Project | `.kilo` or `.kilocode` (Kilo — `.kilocode` is accepted for detection only, never written) |
| Global | `~/.cline`, `~/.factory`, `~/.warp`, `~/.kilo` or `$XDG_CONFIG_HOME/kilo`, `~/.openclaw` |
| Global | Goose's config roots — `$XDG_CONFIG_HOME/goose` and `~/Library/Application Support/goose`, or `$GOOSE_PATH_ROOT` alone when that variable is set, which **replaces** the candidate list rather than extending it |
| Global | `~/.gemini/config` (Antigravity) |
| Project | `.qoder` (Qoder) |
| Global | `~/.qoder`, or `$QODER_CONFIG_DIR` when that variable is set (Qoder) |

`.clinerules` and `.kilocode` are project markers only; they never fire from
your home directory. Antigravity has no project marker at all — all of its
project surfaces live under the shared `.agents/` directory, and detecting on
that would install Antigravity into every workspace that ever used a pool
client.

**One knock-on worth knowing.** Antigravity's global root `~/.gemini/config`
sits *inside* Gemini CLI's own `~/.gemini` marker, so a global Antigravity
install makes **gemini** detected too — and uninstalling does not undo it,
because grim removes the files it wrote, not the directories they lived in.
Pass `--client` if you want only one of them. The reverse never happens: a
Gemini install creates `~/.gemini/agents`, never `config/`.

Pin `[options].clients` (or pass `--client`) for a deterministic target set.

## Every client name reserves a metadata namespace {#reserved-namespaces}

A `<client>.<field>` key inside an artifact's `metadata` map is a **tool key**:
grim looks it up in that client's registry, projects it into native
frontmatter when it is known, and **drops it with a warning** when it is not.
The set of reserved prefixes is derived from the client list, so **every new
client reserves its own name automatically** — see [Vendor
metadata][vendor-metadata] for the full projection table.

**What you will see.** A published skill carrying `metadata.goose.foo`
previously took the verbatim fast path and installed byte-identically. That
key is now stripped, and the warning names both the key and the client.

**The counter-intuitive part, because it reads like a bug.** Most reserved
namespaces carry an **empty** field registry — only Claude has a *skill*
registry at all. So a `goose.foo` key on a skill is dropped **even when Goose
is the target**, not only for other clients. That is the typo guard working
as designed: an unknown key in a reserved namespace is far more often a
misspelling than deliberate data.

**What to do.** Move client-specific data out of a client-named prefix, or
rename the prefix. Any prefix outside the reserved set — `vendor.foo`,
`internal.foo` — still passes through untouched. `grim build` and `grim
publish` surface an affected key through the same warning, so you find it at
your desk rather than at a consumer's.

Reservation is retroactive by design and has happened before: `codex.*`
became a tool namespace when Codex support landed, and the wave-1 clients
(`cursor`, `kiro`, `junie`, `gemini`, `zed`, `amp`) took theirs together. It
will happen again with the next client. **Do not use a client name as a plain
metadata prefix.**

The drop always warns. Some clients previously dropped keys in their *own*
namespace silently, which contradicted grim's own documented projection
table; that is now consistent across every namespace-owning client.

## A vendor directory override can move an existing install {#relocated-roots}

grim honors each client's own directory-override environment variable, so
global-scope installs land where that client actually reads. When a release
*starts* honoring one, the render root moves for everyone who had already set
it — a [layout move][unstable], which the compatibility promise covers
explicitly.

Three roots moved most recently:

| Variable | Shape |
|---|---|
| `$KIRO_HOME` | Replaces `~/.kiro` **outright** — no `.kiro` segment appended. The `$CODEX_HOME` shape |
| `$GEMINI_CLI_HOME` | Replaces the **home directory**, so Gemini's root becomes `$GEMINI_CLI_HOME/.gemini` — the segment **is** still appended. The opposite shape to the other two |
| `$XDG_CONFIG_HOME` (Zed, macOS only) | No longer consulted for Zed on macOS, which now uses a hardcoded `~/.config/zed`. Upstream never read the variable there. Linux and FreeBSD are unchanged |

The two shapes are opposites. Getting them the wrong way round is the easiest
mistake to make here.

Three more moved on 2026-09-27, with the same reaper and the same promises:

| Variable | Shape |
|---|---|
| `$JUNIE_HOME` | Replaces `~/.junie` **outright** — the `$KIRO_HOME` shape. Junie skills, agents, and `mcp/mcp.json` follow it |
| `$OPENCLAW_STATE_DIR`, `$OPENCLAW_HOME` | `$OPENCLAW_STATE_DIR` names OpenClaw's state root itself and wins; else `$OPENCLAW_HOME` replaces the **home directory**, so the root is `$OPENCLAW_HOME/.openclaw` — the `$GEMINI_CLI_HOME` shape. A leading `~` expands to your home directory; any other relative value is ignored |
| `CLAUDE_CONFIG_DIR` in Claude's own settings | grim now reads the `env` block of Claude's managed settings file and of your user `settings.json`, as Claude does. A value set there wins over the shell export, so Claude skills, rules, agents, `.claude.json` and the `settings.json` grim writes for `claudeMdExcludes` move to it. Only a value that differs from what the shell export alone resolved counts as a move |

The `claudeMdExcludes` element grim wrote into the **old** `settings.json`
stays behind: it names a directory the reaper removed, so it excludes
nothing. Remove it by hand if you want the file tidy.

**What you will see** if you had already set one of these variables: artifacts installed before the upgrade sit at a root grim
no longer resolves — which is to say, where your CLI was never reading them
anyway. Until you run a mutating command, [`grim status`][status] reports the
artifact's `state` as `missing` even though the file is on disk.

**What grim does about it.** The next `install`, `update`, or `uninstall`
reaps the stranded copy automatically, including an MCP entry spliced into
the old settings file — the case no file delete could recover. A copy you
edited yourself is **preserved, never deleted**, and grim warns naming both
paths; on the `uninstall` path it is additionally listed in `retained`, or in
`abandoned_entries` for a stranded MCP entry.

**One deliberate exception.** The shared `$HOME/.agents/skills` pool does not
follow `$GEMINI_CLI_HOME`. One physical tree serves every pool client under a
single refcount, and a client-private root would fork it. Gemini upstream
*does* derive its pool from the overridden home directory, so a user who sets
that variable gets pool skills at `$HOME/.agents/skills` while Gemini reads
`$GEMINI_CLI_HOME/.agents/skills`. That gap is known and open.

## An install no longer clobbers a hand-authored file {#untracked-destination}

grim [does not overwrite files it did not create][no-clobber]. One path
escaped that: when a release moved a destination, the migration wrote over
whatever was already there.

**What you will see.** That install now exits **65** with `reason:
"untracked-destination"` and `forceable: true`, and touches nothing. Exactly
one released path reached it — global Copilot rules migrating from
`grim-home/.github/instructions/<name>` to `copilot-root/instructions/<name>`,
shipped in 0.10.0.

**What to do.** Re-run with `--force` to complete the migration and overwrite
the file. No exit code was added or repurposed; this narrows one path back to
the documented behaviour.

## grim update no longer overwrites an edit you made {#update-integrity}

[`grim install`][install] has always refused to overwrite an artifact whose
bytes drifted from the hash grim recorded when it wrote them. [`grim
update`][update] did not: it passed force to the installer unconditionally, so
a roll-forward silently replaced a file you had edited in place. Since 0.13.0
both commands go through the same gate.

**What you will see.** An update over a locally-modified artifact exits **65**
and leaves the file untouched. The rest of the pass still reconciles — one
refused artifact does not abandon the others — and the report is still printed,
because this is a refusal, not an error document. Under `--format json` the
refused row carries `refused: true`; the field is always present and `false` on
every other row, which is how a consumer tells this exit 65 from any other.
`action` keeps reporting the lock diff: the pin *did* roll forward, only the
materialization was refused. In [`grim tui`][tui], `u` on such a row opens the
Overwrite dialog instead of failing.

A changed pin still re-materializes with no force at all. The gate compares
against the **recorded** hash, so rolling a floating tag forward over an
untouched file is unaffected — the rolling-release path is exactly as it was.

**What to do.** Re-run with `--force` to overwrite your edit, or answer the
TUI's Overwrite dialog. To keep the edit instead, stop tracking the artifact:
[`grim remove`][remove] undeclares it and leaves the file on disk.

## A broken global config now fails more commands the same way {#global-config-strict}

An unreadable or invalid `$GRIM_HOME/grimoire.toml` used to fail cleanly
only for a global-scope run or a project run that explicitly resolved
`[[registries]]`. Every command that resolves a registry now does the same
check, including several that previously degraded silently instead.

**What you will see.** `grim context`, `grim add`, `grim login`, `grim
fetch`, `grim describe`, and a default `grim search` now exit **78**
(`EX_CONFIG`) when the global config is unreadable or fails registry
validation — a parse error, or something as easy to miss as two
`[[registries]]` entries both setting `default = true`, which is invalid
even though it is well-formed TOML. Some of these previously exited `0` and
quietly dropped the global registry tier instead. `grim status --check` and
the MCP `grim_fetch`, `grim_describe`, and `grim_render` tools resolve the
same way and are affected identically.

**What stays exit `0`.** `grim search --registry <ref>` collapses the browse
set from the flag before any config is consulted; `grim status` without
`--check` resolves no registries at all; and `grim logout` degrades to a
warning and carries on, because erasing a credential is the direction
where refusing to act is the worse failure mode — `grim login` keeps the
hard failure, since storing one is the direction that can send a secret to
the wrong host.

**What to do.** Fix or remove the offending entry in
`$GRIM_HOME/grimoire.toml` — the error names both the file and the rule it
violates. See [Multiple registries][multi-registry] for the full
affected-command list and the reasoning.

## Nothing detected no longer installs into every client {#autodetect-fallback}

With no `--client`, no `[options].clients`, and no client marker present,
earlier versions targeted *every* known client. That wrote one directory per
vendor into a workspace that had asked for none of them — and those
directories were exactly what made the next run "detect" all of them, so the
footprint was self-perpetuating and there was no way to tell a real client set
apart from grim's own leftovers.

**What you will see.** That fallback is now a single vendor-neutral `agents`
client, which writes one copy into the cross-vendor `.agents/skills` pool. It
is never *detected* — only selected — so writing the pool changes nothing about
what the next run resolves.

**The one case that now fails.** If the fallback is active **and** the declared
set holds nothing that client can install — only rules, agents and/or MCP
servers, all of which it declines — `grim install` and `grim add` exit **78**
instead of succeeding. This is an exit-code change on a path that previously
returned **0**: before, the same command installed rules into every known
client's rule directory.

**What to do.** Pass `--client <name>` or set `[options].clients` — the error
names both. `grim add` still records the declaration and the lock entry before
it exits, so a follow-up `grim install --client <name>` completes without
re-adding anything. If you *want* the old behaviour, name the clients
explicitly; nothing else recovers it, and that is deliberate.

## `GRIM_RATING_HOST` is gone; the index declares its rating host {#rating-host}

**This one is breaking**, and it is deliberate: it lands before the 1.0
freeze that makes the documented `GRIM_*` set a semver contract.

`grim rate` used to resolve its forge host from a built-in per-provider
default plus `GRIM_RATING_HOST`, an environment variable each voter set on
each machine. That made every self-hosted deployment misconfigured by
default and silently — one team, one index, N machines, and a vote that
went to gitlab.com until somebody exported the variable. The host is a
property of the index that holds the threads, so the index now states it:
`providers.rating_host` in [`stats.json`](./package-index.md#spec-stats).

**What you will see.** `GRIM_RATING_HOST` is no longer read. If you export
it, nothing happens; if you relied on it, the vote resolves to the
provider default until the index declares a host.

**What to do**, in one of two roles:

- **Index operator** — add `providers.rating_host` to your published
  sidecar (the [indexer][indexer] derives it from the CI-provided GraphQL
  endpoint). Every consumer then votes against your instance with no
  per-machine setup at all.
- **Voter** — unset the variable. If your index does not declare a host
  yet, ask its operator to; there is no per-machine override any more, and
  that is the point.

**One new refusal comes with it.** Because the host now arrives in fetched
content, the two credentials grim does *not* look up itself are bound to
it: against an index-declared host, `--token-stdin` and `GRIM_RATE_TOKEN`
must be accompanied by `--token-host <host>` naming it, or the run exits
`80` before the credential is read. A CI token or a `gh`/`glab` stored
credential is unaffected — grim only ever resolves those for the host it is
about to contact. `--token-host` no longer requires `--token-stdin`, which
retires the `64` that combination used to produce.

Background: [Voting against a private instance](./ratings.md#voting-host).

## Downgrading to 0.13 after browsing a rated index {#catalog-cache-downgrade}

[Artifact ratings][ratings] add one field to the catalog cache
(`$GRIM_HOME/catalog/<hash>.json`), which parses strictly: a cache written
by a newer grim is refused wholesale by an older one. That is a deliberate,
cheap trade — a cache is not a contract, and a refusal should cost exactly
one network rebuild.

**It did not cost one rebuild in 0.13.0 and earlier.** The parse error was
raised above the rebuild decision, so a refused cache degraded that
registry to an **empty browse** without overwriting the file — on every
later run, `--refresh` included, until someone deleted it by hand. This
release fixes it going forward: a cache the loader refuses now reads as
cold while online, so the next browse simply rewrites it. That fix cannot
reach a binary that already shipped.

**What to do.** Only if you *downgrade*, and only if you browsed an index
that publishes ratings: delete the cache once.

```console
$ rm -rf "${GRIM_HOME:-$HOME/.grimoire}/catalog"
```

Upgrading needs nothing — the newer binary rebuilds an older cache
normally. And a cache whose entries are all unrated is byte-identical to
what 0.13 wrote, so a user who never browsed a rating-publishing index is
not affected in either direction.

## Publishing stamps provenance without `--git` {#default-provenance}

Through 0.13.0, `grim build` / `release` / `publish` wrote **no** build
provenance unless you passed `--git`. Since 0.14.0 they derive
`org.opencontainers.image.revision` and `…created` on every run, and fill the
descriptive `vendor`, `url` and `documentation` keys wherever a `repository`
(authored, or from `[metadata]`) makes them derivable — see [build
provenance][git-provenance].

Two things change for an existing publisher:

- **Manifests carry annotations you did not author.** Consumers that
  enumerate the annotation map see more keys; the curated readers
  (`grim describe`, `grim search --format json`, the TUI detail pane) simply
  stop reporting `null` for fields they already had. Nothing was renamed or
  retyped.
- **Re-releasing an exact version from a *different* commit now needs
  `--force`.** The revision is part of the manifest, so identical content
  built from a new commit is a new digest, and the immutability gate refuses
  to move the tag. A re-release from the **same** commit stays byte-identical
  and idempotent, because no derived value is read from the clock — `created`
  is the commit's own date, or a `SOURCE_DATE_EPOCH` instant outside a
  repository.

Nothing new is disclosed by default: the `origin` remote and the commit
author's name — the two values that name infrastructure and a person rather
than the artifact — remain behind `--git`. `--no-git` suppresses every derived
annotation and restores the 0.13.0 manifest shape exactly.

## Smaller notes {#smaller-notes}

- **A live symlink at an install destination now exits 65, not 74.** A
  symlink-to-directory at a destination grim was about to write used to abort
  the whole install with an I/O error, because the footprint hash followed the
  link and read a directory as a file. It is now refused as an untracked
  destination — same `forceable: true`, so `--force` completes it. A symlink to
  a *file* still adopts exactly as before.
- **`grim fetch --format json` and the MCP `grim_fetch` tool** now populate
  the existing optional `warnings` array for `codex`, `gemini`, `zed`, `amp`
  and `antigravity`, where they previously emitted none. The field already
  documented "projection typo guards" as its content; nothing was added,
  removed, or retyped.
- **`grim config registry fields` and `grim config list --format json` emit
  two more rows.** The addressable per-registry field set grew from three
  (`oci`, `index`, `default`) to five with the [browse
  filters][browse-filters] — `include` and `exclude` joined it. Rows are
  **appended** to the existing `{"items": […]}` list and nothing was removed,
  renamed or retyped, so a consumer indexing the first three positionally is
  unaffected; one that assumed the list had exactly three entries will see
  five. Field positions are frozen and the list is append-only.
- **`grim search`, `--format json`, and the MCP `grim_search` tool drop weak
  hits by default now.** A hit below `options.search_min_relevance` percent
  of the best score is hidden. A stderr notice says how many. Run
  `grim config set options.search_min_relevance 0` to list every match
  again, or add `--global` outside a project. Prose fields (summary,
  description, and the namespace) now match by word prefix, not by fuzzy
  subsequence.
- **`grim config list --all` gains one more row.** The new
  `options.search_min_relevance` key is appended at the end, with the usual
  type, title, description, and default metadata. Existing rows keep their
  positions.
- **Codex MCP servers pick up a shorter startup wait.** A descriptor's
  `timeout` (milliseconds) now renders as `startup_timeout_ms` in Codex's
  `config.toml`, so a short `timeout` actually shortens Codex's startup
  wait — before it had no effect there. An artifact installed before this
  release keeps its old bytes until the pin changes (`grim update`).
  `--force` alone does not rewrite it: delete the server's
  `[mcp_servers.<name>]` table from `config.toml`, then run `grim install`.
- **Two more Claude keys render instead of warning.** `claude.background`
  (skill) and `claude.omit-claude-md` (agent) are now known keys that
  render natively — an unknown key used to warn and drop. Values must be
  `true` or `false`; any other literal now fails `grim build` (exit 65)
  and a fresh install. Same rule as above: an artifact installed before
  this release keeps its old bytes until the pin changes (`grim update`).
  `--force` alone does not rewrite it: delete the rendered skill or agent
  file, then run `grim install`.
- **A global MCP server with a `${VAR}` reference registers for Copilot
  CLI.** grim used to skip it, because Copilot CLI did not expand
  references in `mcp-config.json`. Current releases do (verified against
  CLI 1.0.88), so grim writes the reference as authored.
  `grim status` lists the missing registration under `outputs_pending`, and
  the next `grim install` adds it without touching the other clients'
  entries. If you worked around the old skip by hand-adding a same-named
  `mcpServers.<name>` entry directly in `~/.copilot/mcp-config.json`, grim
  now refuses to install over it as untracked — delete your hand-added
  entry, or run `grim install --global --force` to replace it with the
  `${VAR}` form. Copilot CLI releases older than about April 2026 pass an
  `env` reference through unexpanded rather than expanding it. See
  [Environment references][env-refs].
- **Rules install for Antigravity.** grim used to skip every rule for
  `antigravity`. It writes `.agents/rules/<name>.md` in a workspace and
  `~/.gemini/config/rules/<name>.md` globally. A scoped rule gets
  `trigger: glob`, any other rule `trigger: always_on`. An existing Antigravity
  install shows the new file under `outputs_pending` in `grim status`, and the
  next `grim install` writes it without touching other clients. A
  hand-written file already at that path is refused (exit `65`) until you
  remove it or pass `--force` ([Antigravity][gap-antigravity]).
- **A project MCP server reaches Copilot CLI.** grim writes a Copilot
  server into `.github/mcp.json` as well as `.vscode/mcp.json`, because the
  CLI does not read the VS Code file. An install from an earlier release
  lists `.github/mcp.json` under `outputs_pending`, and the next
  `grim install` writes it without touching the VS Code entry. If you
  hand-added a same-named `mcpServers.<name>` entry to `.github/mcp.json`,
  grim refuses to install over it as untracked (exit 65). Delete yours, or
  run `grim install --force`. A workspace `.mcp.json` takes precedence over
  `.github/mcp.json` in the CLI, as [MCP limitations][mcp-limitations]
  explains.
- **An MCP server whose `[server.oauth]` block sets only `client_id`
  registers for Copilot.** grim used to skip every oauth-bearing server for
  Copilot. It writes the client ID as `oauthClientId` (and
  `oauth.clientId` in `.vscode/mcp.json`). A block with scopes, a callback
  port, or a metadata URL is still skipped with a warning naming the field.
  A same-named entry you added by hand is refused as untracked (exit 65)
  until you delete it or pass `--force`.
- **Junie receives agents from this release on.** grim used to decline
  them. It writes `.junie/agents/<name>.md`, or `~/.junie/agents/<name>.md`
  with `--global`. `grim status` lists each missing agent under
  `outputs_pending`, and the next `grim install` writes it. The other
  clients' files stay untouched.
  - **A hand-written agent of the same name** in that directory is
    untracked, so grim refuses to install over it (exit 65). Delete yours,
    or run `grim install --force`.
  - **A name Junie rejects**, with a leading digit or a `.`, is skipped for
    Junie with a warning and never reported as pending.
  - **The `junie.permission-mode`, `junie.reasoning-level` and
    `junie.max-turns` agent keys render natively.** An unknown key used to
    warn and drop. A bad literal (outside the enum, or not an integer) now
    fails `grim build` (exit 65) and a fresh install. No Junie agent was
    rendered before this release, so no existing file keeps old bytes.

  See [Junie][junie-gap].
- **A global MCP server registers for Cline.** grim used to decline MCP for
  Cline entirely. It writes the flat entry into the file the Cline CLI and VS
  Code extension share, `~/.cline/data/settings/cline_mcp_settings.json`,
  under Cline's own lock. `grim status --global` lists the missing
  registration under `outputs_pending`, and the next `grim install --global`
  adds it. A same-named `mcpServers.<name>` entry you added there yourself
  makes that install exit **65**. Delete yours, or run
  `grim install --global --force` to replace it.
- **A project-scope MCP install warns for Cline.** Cline has no project MCP
  file, so grim names it in a warning and writes nothing for it.
- **Droid now installs agents and MCP servers.** An agent lands as a custom
  droid in `.factory/droids/<name>.md`, and an MCP server as an entry in
  `.factory/mcp.json` (`~/.factory/` with `--global`). Before, both were
  skipped for Droid. `grim status` lists the new outputs under
  `outputs_pending`, and the next `grim install` writes them without touching
  other clients. A file or a same-named `mcpServers` entry you added there by
  hand is refused as untracked (exit `65`) until you remove it or run
  `grim install --force`. That includes the copy Droid's UI writes to
  `~/.factory/mcp.json` when you toggle a project server. `droid.reasoning-effort`
  (`low`, `medium` or `high`) now renders instead of warning; any other
  literal fails `grim build` (exit `65`). See [Droid's entry][droid-gap].
- **The TUI now asks before acting on more than one artifact.** Pressing `i`,
  `u`, or `d` on a marked set or a selected group opens a confirmation naming
  the count and source before anything runs. A single artifact still acts on
  the first press.
- **Three more Copilot agent keys render instead of warning.**
  `copilot.disable-model-invocation`, `copilot.user-invocable` (both bool),
  and `copilot.target` (`vscode` or `github-copilot`) are now known keys
  that render natively — an unknown key used to warn and drop. A bad
  literal (not `true`/`false`, or a `target` outside the two-value enum)
  now fails `grim build` (exit 65) and a fresh install. An artifact
  installed before this release keeps its old bytes until the pin changes
  (`grim update`). `--force` alone does not rewrite it: delete the rendered
  agent file, then run `grim install`.
- **An OpenCode agent with an invalid `opencode.color` or `opencode.steps`
  now installs with the field dropped, not rejected wholesale.** OpenCode's
  own schema accepts only `#RRGGBB` or one of seven theme names for
  `color`, and a positive integer for `steps`; a bad value used to reach the
  written agent file unchecked, and OpenCode would then refuse its entire
  config — every agent and verb, not just the offending one. grim now
  validates the lifted value at install time and drops it with a warning
  instead. An existing broken install repairs itself on the next pin change
  that touches the agent. `--force` alone does not repair it: delete the
  rendered agent file first, then run `grim install` to repair it right
  away. See the [`opencode.*` agent registry][vendor-metadata-opencode].
- **An MCP server with a `[server.oauth]` block now registers for OpenCode
  and Zed when they can carry it.** grim used to skip such a server for
  both. OpenCode now gets an `oauth` object when the block sets no
  `auth_server_metadata_url` and no `callback_port = 0`. Zed gets one when
  the block sets only a literal `client_id`; a `${VAR}` id still skips,
  because Zed expands no references. Any other block still skips, and the warning now names the
  fields that could not be carried. The next `grim install` writes the new
  entry without touching the other clients' entries.
  If you worked around the old skip by adding a same-named entry to
  `opencode.json` (`mcp.<name>`) or Zed's `settings.json`
  (`context_servers.<name>`) by hand, grim now refuses to install over it
  as untracked (exit 65). Delete your entry, or run `grim install --force`
  to replace it. See [the oauth block][mcp-oauth].
- **Goose and Kilo install agents.** Earlier releases skipped an agent
  artifact for both, and this release writes it to the paths listed under
  [agent install locations][agent-locations]. On a project that already has
  agents installed, `grim status` lists the new files under
  `outputs_pending`, and the next `grim install` writes them. A file you
  placed at one of those paths by hand is refused as untracked (exit 65).
  Delete it, or run `grim install --force`.
- **Gemini CLI no longer receives a server's `timeout`.** Gemini applies
  that value to every tool call as well as startup, so a descriptor tuned for
  a quick start also cut long tool calls short. grim now drops it with a
  warning, and Gemini's 10-minute default applies. An entry installed before
  this release keeps its `timeout` until the pin changes (`grim update`).
  `--force` alone does not rewrite an entry that is still intact. To drop it
  now, delete the server's entry under `mcpServers` in `.gemini/settings.json`
  (globally `~/.gemini/settings.json`, or `$GEMINI_CLI_HOME/.gemini/settings.json`
  when that is set) and run `grim install`. An entry you edited yourself reads `modified` as
  before, and grim leaves it alone unless you pass `--force`. See
  [`timeout`][mcp-server-table].
- **MCP servers now register for Warp.** grim writes them into
  `.warp/.mcp.json`, or `~/.warp/.mcp.json` with `--global`. `grim status`
  lists the missing Warp registration under `outputs_pending`, and the next
  `grim install` adds it without touching other clients' entries. A server
  using a `${VAR}` reference, any `oauth` field or the `ws` transport is
  skipped for Warp with a warning, as the next entry describes. If you already added a same-named server to
  that file by hand, grim refuses to install over it (exit 65): delete your
  entry, or pass `--force` to replace it. Warp can also read Claude's
  `.mcp.json`, so with both clients selected a server can show up twice in
  Warp. See [Warp: MCP][clients-warp-mcp].
- **A server a client cannot represent no longer makes every install report
  `updated`.** Some clients skip some servers with a warning. Warp skips a
  `${VAR}` reference, and Copilot CLI skips a url invalid before expansion.
  Cline skips any oauth block. grim used to count that skipped file as
  missing, so each `grim install` re-ran the MCP pass. Now the install
  reports the server `unchanged` and writes nothing. `grim status` may still
  list that client under `outputs_pending` for a registry artifact, because
  status reads no server descriptor offline.

<!-- internal -->
[agent-locations]: ./agents.md#locations
[changelog]: https://github.com/grimoire-rs/grimoire/blob/main/CHANGELOG.md
[mcp-oauth]: ./mcp-servers.md#server-oauth
[git-provenance]: ./publishing.md#git-provenance
[browse-filters]: ./configuration.md#browse-filters
[ratings]: ./ratings.md
[indexer]: ./hosting-an-index.md
[stability]: ./stability.md
[unstable]: ./stability.md#unstable
[status]: ./commands.md#status
[install]: ./commands.md#install
[update]: ./commands.md#update
[remove]: ./commands.md#remove
[tui]: ./commands.md#tui
[vendor-metadata]: ./vendor-metadata.md#projection-semantics
[vendor-metadata-opencode]: ./vendor-metadata.md#opencode-agent-registry
[no-clobber]: ./json-interface.md#error-reason
[multi-registry]: ./configuration.md#multiple-registries
[droid-gap]: ./clients.md#gap-droid
[env-refs]: ./mcp-servers.md#env-references
[gap-antigravity]: ./clients.md#gap-antigravity
[mcp-limitations]: ./mcp-servers.md#limitations
[mcp-server-table]: ./mcp-servers.md#server-table
[clients-warp-mcp]: ./clients.md#gap-warp-mcp
[junie-gap]: ./clients.md#gap-junie
