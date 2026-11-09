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
| Global | `~/.gemini/antigravity-cli`, the Antigravity CLI's settings directory, since 0.16.0 (Antigravity) |
| Global | `$XDG_CONFIG_HOME/warp-terminal` on Linux, Warp's settings directory, since 0.16.0 (Warp) |
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
Gemini install creates `~/.gemini/agents`, never `config/` or
`antigravity-cli/`.

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

## Downgrading to 0.13 or 0.14 after browsing {#catalog-cache-downgrade}

[Artifact ratings][ratings] add one field to the catalog cache
(`$GRIM_HOME/catalog/<hash>.json`). So does the tree placement a
[package index](./package-index.md) gives each row. Since 0.16.0 so does the
`kind_read` marker on a directly browsed row that names no kind, such as a
container image.

Through 0.14.x the cache parses strictly: a cache written by a newer grim is
refused wholesale by an older one. That is a deliberate, cheap trade — a
cache is not a contract, and a refusal should cost exactly one network
rebuild. From 0.15.0 grim reads past a field it does not know, so only a
downgrade to 0.13 or 0.14 meets the refusal.

**It did not cost one rebuild in 0.13.0 and earlier.** The parse error was
raised above the rebuild decision, so a refused cache degraded that
registry to an **empty browse** without overwriting the file — on every
later run, `--refresh` included, until someone deleted it by hand. This
release fixes it going forward: a cache the loader refuses now reads as
cold while online, so the next browse simply rewrites it. That fix cannot
reach a binary that already shipped.

**What to do.** Delete the cache once if you *downgrade* to 0.13 after a
browse that wrote one of those fields. Three browses do: an index that
publishes ratings (from 0.14), any index (after 0.14.2), and a registry
holding a repository with no grimoire kind (since 0.16.0).

```console
$ rm -rf "${GRIM_HOME:-$HOME/.grimoire}/catalog"
```

After a downgrade, 0.14 refuses the cache that registry browse wrote too,
and 0.14.0 to 0.14.2 also refuse one any index browse wrote. Online, 0.14
rebuilds the cache once. Offline, it fails until an online run or the
deletion above.

Upgrading needs nothing — the newer binary rebuilds an older cache
normally. A cache whose entries carry no rating, no index placement and no
`kind_read` marker still reads in 0.13.

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

## A plugin's version now hashes the rendered tree {#plugin-version-move}

[`grim export plugin`][export-plugin] appends `+<12 hex>` to every plugin
version. Through 0.14 that suffix hashed each member's kind, emitted name and
digest. From 0.15.0 it hashes the plugin tree grim renders for that client:
every file's path, executable bit and SHA-256, with the manifests written at
the base version. The `<base>+<12 hex>` grammar and the report shape are
unchanged.

**What you will see.** Every plugin you exported before gets a new suffix
the first time you export it again, even when its members are the same. After
that, edits that never moved the suffix now do. They are a change to a
plugin's `description`, a new logo, and a grim release that renders a skill
differently. One plugin can also carry a different suffix for each client.
Each client's tree has its own manifest path and README.

**What to do.** Nothing needs migrating. Hand out the re-exported plugin as
usual. Compare exported versions for equality, never for order, since the
suffix is build metadata. A script that pinned an old suffix needs the new
one. To change the base a consumer sees, set `--version` or the declared
`version`. What feeds the hash is [not frozen][unstable].

## Qoder plugins export, and change a default {#qoder-plugin-export}

[`grim export plugin`][export-plugin] now knows Qoder. A `qoder` plugin is a
Claude-family plugin with its manifest at `.qoder-plugin/plugin.json`. Two
behaviours change:

- **`--client qoder` succeeds.** It used to exit `78` (`NoPluginFormat`). It
  now writes `<plugin>.qoder`, or `<plugin>.qoder.zip` with `--zip`.
- **A configured `qoder` is no longer skipped.** With no `--client`, export
  reads `[options].clients`. A list naming `qoder` used to skip it with a
  warning, and a list naming only `qoder` fell back to `agents`. Now
  `<plugin>.qoder` is written beside the other clients' output, or alone.

**What you will see.** A new `<plugin>.qoder` output next to your existing
ones. It is a new path, so it replaces nothing. If a file or directory of that
name already exists, export refuses it at exit `65` as it always does, and
`--force` replaces it.

**What to do.** To keep the old output, pass `--client` with the clients you
want, or take `qoder` out of `[options].clients`. A script that treated exit
`78` for `--client qoder` as expected needs updating.

## `[marketplace]` needs grim 0.15.0 {#marketplace-table-needs-new-grim}

[`grim export marketplace`][export-marketplace] reads a new
[`[marketplace]` table][marketplace-table] from `marketplace.toml`. A manifest
without it works everywhere, exactly as before. A manifest with it works only
from 0.15.0.

**What you will see.** An older grim rejects the file with an unknown-field
error, exit `65`, on every command that reads it. That includes `grim export
plugin` and `grim update --marketplace`, which never needed the table. Adding
the table changes no lock bytes and re-pins nothing.

**What to do.** Upgrade every machine and CI job that reads the manifest
before you add the table. Nothing else changes: a lock written by an older
grim is read as is.

## Plugin metadata keys need grim 0.15.0 {#plugin-metadata-needs-new-grim}

[`marketplace.toml` plugins and `grimoire.toml`'s `[plugin]` table][plugin-metadata]
accept optional `author`, `homepage`, `repository`, `documentation`,
`support`, `license`, `keywords`, `tags` and `category` keys, and
`[marketplace.owner]` accepts `url`. A manifest without them works
everywhere and renders the same files as before. One that sets them works
only from 0.15.0.

**What you will see.** An older grim rejects the file with an unknown-field
error, exit `65` for `marketplace.toml` and `78` for `grimoire.toml`. The new
keys change no lock bytes and re-pin nothing. They do change the files grim
renders, so a plugin's `<base>+<12-hex>` version changes once after you add
them. An ad-hoc `grim export plugin <ref>` now also reads the pinned
manifest's annotations, so its `plugin.json` and version suffix change once for
annotated references.

**What to do.** Upgrade every machine and CI job that reads the file before
you add the keys.

## Compressed layers need grim 0.15.0 {#compressed-layers-need-new-grim}

[`grim release` and `grim publish`][compression] can publish a skill,
rule, agent or hook as a gzip-compressed layer with `--compression gzip`.
Nothing changes unless a publisher opts in. Default output is byte-identical,
and every artifact published without the flag installs on every grim.

**What you will see.** From 0.15.0 grim reads gzip layers everywhere it
reads a layer, and `grim describe` reports a new
[`compression`][describe-compression] field. A grim before 0.15.0 cannot read a
gzip layer, and `grim fetch` fails with exit `65` (verified on 0.14.3).

**What to do.** Upgrade every machine and CI job that installs an artifact
before its publisher opts in to compression. As a publisher, compress new
versions only, and never `--force` an existing version, because that changes
its digest and breaks the locks that pin it. Re-releasing equal content under a
different manifest digest, for example after a compression flip, adopts the
existing version from 0.15.0 instead of refusing. That holds for skill, rule, agent and
hook releases, while bundle and MCP releases still refuse.

## Declared dependencies need grim 0.16.0 {#dependencies-need-new-grim}

A skill, rule, agent, MCP server or hook can now
[declare the artifacts it needs](./artifacts.md#requires), and a bundle can
hold other bundles. Nothing changes unless a publisher opts in. An artifact
without declarations publishes and installs byte-identical to before, and the
lock format version stays `1`.

**What you will see.** A project that installs a dependent artifact gets the
dependencies too, and [`grim status`][status] gains `direct` and `required_by`
on every row. Ask [`grim why`][why] why something is installed.
[`grim remove`][remove] and [`grim uninstall`][uninstall] now take the
dependencies that only the removed artifact needed. Uninstall reports an
artifact another one still needs as `kept-by-dependency`.

**What an older grim does.** Observed on 0.15.1:

| Met by 0.15.1 | Result |
|---|---|
| a dependent artifact, installed with `grim add` | exit `65`, `expected a single-layer artifact`; no files are installed. On a lock it wrote, that includes `grim add` of a bundle member whose moved tag names such an artifact, beside a dependent, after it moved the pin. Where that artifact's dependencies conflict or form a cycle, this grim keeps the pin and exits `0` |
| a dependent artifact, met by `grim lock`, `grim update` or `grim add --no-install` | succeeds and writes a lock **without the dependencies**, with no warning |
| a bundle that lists a nested bundle | exit `65`, `nested bundle member … is not supported` |
| a lock that records a nested bundle | exit `78` on `status` and `install`. `lock`, `add` and `update` overwrite it |

The second row is the one to watch. A lock written that way is wrong until a
newer grim re-locks it. A newer grim that installs from it exits `65` with
reason `stale-lock`, and asks for `grim lock`.

**What to do.** Upgrade every machine and CI job that installs from a project
before a publisher adds declarations to an artifact it uses. Never run
`grim lock` or `grim update` on an old grim in a project that already depends
on a dependent artifact. If a lock was written that way, run
[`grim lock`][lock] with 0.16.0 or later and commit the result. As a
publisher, put the first dependency-bearing version under a
[new major tag](./publishing.md#requires-consumers) so floating consumers stay
on a version their grim can read.

Two read paths also changed. [`grim describe`][describe] and
[`grim fetch`][fetch] on a digest-pinned manifest that is already in the local
store now succeed offline, where they exited `81` before. The reason
`unsupported-artifact-feature` is new, and a bundle manifest that does not have
exactly one layer now carries it instead of a reason-less `65`.

**Hooks are consented per target.** `grim add <bundle>` used to consent every
hook member of the bundle, including one that was already present and had never
been consented. It now consents only the target and the hooks it newly pulls
in. Hooks are experimental, so this ships without a shim. Re-run
`grim install --trust-hooks` for a hook you had relied on `add` to arm.

That consent is recorded ahead of the install, and only for hooks `add` read
from a manifest this run resolved, behind edges this run resolved. An offline
`grim add` of a bundle or skill consents to no hook it brings in, so run
`grim hook allow`. A failed `add` keeps its declaration, its lock and that
consent, so the next `grim install` arms the hook and disarms nothing. Retrying a
hook you typed consents to a kept pin once its repository confirms it, which
needs the network.

Two other paths record consent as well. `grim add --no-install` records what
`grim add` records, where it wrote nothing before. An Install in `grim tui` that
declares a row does too, and every install path there arms on the record as it
stands. It used to read a hook the record lacked as drift and disarm every hook.

## Install fixes in 0.16.0 {#install-fixes-0-16}

grim 0.16.0 fixes four install paths that wrote, removed or refused the wrong
thing. None of them changes a flag or a JSON key, and only the byte-order mark
fix changes an exit code. Each can still change what you see on the first run
after the upgrade.

**The worktree ignore rule moves to the shared exclude file.** grim adds
`.claude/settings.local.json` to git's `info/exclude` when it writes that
file for a hook. In a linked worktree, grim used to write that line to the
worktree's own git directory. Git reads `info/exclude` only from the common
directory, so the rule did nothing. Since 0.16.0 grim writes the common
`info/exclude`, which every checkout of the repository shares.

A linked worktree never removes that line, because a sibling checkout may
still rely on it. A line written to the per-worktree file before 0.16.0
stays there and stays inert. The next `grim install` writes the line git reads.

**An adopted MCP entry stays when its client is dropped.** grim adopts an
entry it finds already holding the value it would write. Before 0.16.0,
[`grim update`][update] spliced that entry out when it reaped a client that
`[options].clients` no longer names. Since 0.16.0 it leaves an adopted entry
in the file and drops only the record. The
update row names it in [`abandoned_entries`][json-update], and grim logs a
warning naming the file. An entry grim wrote itself is removed as before, and
`--force` removes an adopted one too.

The same holds when that update also moves the server's pin. `grim install`,
`grim add` and an install from `grim tui` read `[options].clients` too. On
every run, whether the pin moved or not, they leave the adopted entry of a
client outside that list alone.

When a new pin is one a client can no longer represent, `update`, `install`
and `add` all leave that client's adopted entry in place. `--force` removes
it. They also leave an entry grim cannot read, and warn about it. An entry you
edited refuses the server's row until you pass `--force`, and then stays in
place with a warning too. Only the `update` row names these entries.

**A JSON file that starts with a byte-order mark is edited.** Some editors
write a byte-order mark at the start of a JSON file. In 0.15.1 grim refused to
edit such a client config file and exited `74`, whether the file held MCP
servers, hooks or the `claudeMdExcludes` list. Since 0.16.0 grim reads past the
mark, keeps it first in the file, and reads and edits the rest as usual. The
new settings kind edits such a file the same way.

**A state load drops an output its kind may not address.** An install-state
file can arrive with a `git clone`. Since 0.16.0 every load checks each
recorded output against the surfaces its artifact kind may write. An output
outside them leaves the record, and grim warns once per output with
`dropped a recorded output of <kind> '<name>'`.

The file at that path is not touched. A state file grim wrote itself loads
unchanged.

**`grim search settings` filters by kind.** 0.16.0 adds the
experimental [`settings`][settings] kind, behind
`[options.experimental] settings` and listed under [Unstable][unstable].
`settings` and `setting` became kind keywords like `skill` and `rule`, so
[`grim search settings`][search] lists settings packages only. A skill named
`vscode-settings` no longer answers the bare word. Search for
`vscode-settings` to find it again.

## Index refreshes add a root probe and a sync sidecar {#index-sync}

A refresh of an HTTP [package index](./package-index.md) starts with a
request for the index root, `c/root.json`. An index that publishes a usable
root is read through it. Every other index is read through `all.json`, exactly
as before. Beside the probe, grim keeps a small state file for each source and
backs off from a source that keeps failing. No flag, exit code or cache byte
shape changes.

**What you will see.**

- A flat index answers the probe with a `404`, or with a root whose `flat` is
  `null`. Either way it costs one extra small request per refresh. Every other
  answer is listed under [Discovery][index-discovery].
- A file named `<hash>.sync.json` appears beside each
  `$GRIM_HOME/catalog/<hash>.json`. It is the sync sidecar. It holds digests,
  the adopted root's generation and a failure count, and it never holds a URL. See
  [Data layout][data-layout].
- After a failed refresh, grim makes no request to that source until a
  recorded time, one hour at most. `grim search` serves the cache and reports
  `refresh deferred until <timestamp>` in `sources[].error`. Before, every
  command retried while the cache stayed expired. See
  [Failures and backoff][index-backoff].
- On an index with a root, grim fetches the stats blob once per advertised
  digest and no longer on every refresh. A blob that changes a rating rewrites
  the cached listing once. See [The stats rule][index-stats-rule].
- `grim context --format json` gains `registries[].catalog`, always present.
  It reports the source's last sync from the sidecar alone, with no request,
  or `null` when no readable sidecar exists. See [`grim context`][commands-context].

**One fresh sync after a mixed-version rewrite.** The sidecar binds to the
exact bytes of the cache file. A grim without the sidecar that shares
`$GRIM_HOME` reads the cache as it always did. If it rewrites the cache, the
binding breaks, and the next refresh by this grim downloads the catalog once.

That grim may also refresh sooner than this one, because it sees only the
cache's own build time. It ignores the sidecar, so downgrading needs no
cleanup.

**What to do.** Nothing. `grim search --refresh` forces a sync and ignores a
backoff. Deleting a `<hash>.sync.json` file costs the same one fresh sync.

## `grim lock` keeps pins {#lock-keeps-pins}

A lock exists so that next month's install writes the bytes you tested today.
Up to 0.15.1 that held only until someone ran [`grim lock`][lock]. It resolved
every tag again, so a moved tag changed the lock with no update requested.
[`grim add`][add] did the same, and so did each "run `grim lock`" recovery.

From 0.16.0 [`grim update`][update] is the command that moves an existing pin.
Everything else keeps it, even when its tag moved, apart from the
[few cases][lock-pins] listed with `grim lock`. The lock format version stays `1` and the
lock gains no field. What changes is `pinned`: it records the tag that resolved
it, so a later run can compare it with the request.

**What you will see in the lock.**

- The first `grim lock`, `grim add` or `grim update` after the upgrade adds the
  tag to every tagged `pinned` line. `registry/repo@sha256:…` becomes
  `registry/repo:tag@sha256:…`. With `grim lock` no digest changes, no tag is
  resolved, the report reads `unchanged`, and `generated_at` stays. `grim add`
  stamps the tags with its own write, and `grim update` moves pins as it always
  does.
- A reference requested by digest alone stays tagless, and `[[bundle]]` keeps its
  own `tag` key. `grim install` leaves a tagless pin as it is. So does
  `grim remove`, unless it ends the lock's proof. Then it stamps kept tagless pins
  first, since nothing after it can show what they answered.
- A pin nothing vouches for resolves once, and one warning names it and the
  cause. That happens when `grimoire.toml` changed after the lock was written,
  or when a moved parent or a new key asks for something else. Later writes
  keep what that run pinned.
- A matching hash keeps a pin the lock records no edge onto when a bundle with no
  `[[bundle]]` row lists it. That costs one tag read, and the next write stamps
  the tag the bundle lists. If that bundle's tag moved, the stamp can pair the
  member's new tag with its old digest. `grim status --check` reports it as an
  update, and `grim update` repairs it. Only a lock grim 0.3 or earlier wrote,
  or one whose entries were cut by hand, has such a bundle.
- Where the parent has an entry in the lock, an edge counts only if that entry
  records it. A lock 0.15.1 wrote records no dependent's edges. `grim lock`
  settles those as a fresh resolve does: the pin can move, or the run exits `78`
  on a conflict.
- `grim add` of a bundle member's own reference reads that reference's tag
  first when such an unrecorded edge asks for another tag. It then reads what
  the manifest that tag names asks for. If the tag you typed is gone, the add
  refuses too. A gone tag below that manifest does not refuse it, and `grim lock`
  keeps the pin there as well. Otherwise the add keeps the
  member's pin only where `grim lock` keeps it, and records that tag.
- That manifest's dependencies can conflict or form a cycle, back at the member's
  pinned digest or at another one. The add then keeps the member's pin, as
  `grim lock` does, and installs it at exit `0`. An unreachable registry, a refused
  credential, a timeout or a graph cap below the manifest keeps its exit. 0.15.1
  moves the pin there. Its `grim add --no-install` exits `0`, and its `grim add`
  exits `65` with `expected a single-layer artifact`, since that manifest
  carries a dependency layer. Offline, `grim add --no-install` there exits `0`
  on an empty store and on one an online add filled, where 0.15.1 exited `69`.
- A script that splits `pinned` on `@sha256:` keeps working. A report's `pinned`
  stays as it was, because the tag lives only in the lock file.

**What changes for a moved tag.**

- `grim lock` no longer rolls a moved tag forward. A job that ran it to pick up
  new digests gets the same lock back.
- To roll, run `grim update` for every pin or `grim update <name>` for one. Both
  install what changed, and neither has a lock-only form.
- Re-running `grim add` on a reference already declared keeps its pin. An add
  also keeps a pin the lock already holds when that pin answers the reference, as
  a dependency or a bundle member does. The report still says `added`, and one
  stderr note names `grim update -- <name>` with the run's `--global` or
  `--config`. That command moves the pin to the digest its tag names today, or says why it
  cannot.
- [`grim status --check --format json`][status-check] shows what a moved tag
  would do. `update_available` reads `true` when a row's reference names another
  digest than its pin, on declared rows and on dependency rows alike. The plain
  table has no such column, so stderr names each row with an update.
- A dependency row answers for the tag its pin records while an edge names it,
  and otherwise for the smallest edge the lock holds. It cannot predict a moved
  parent that switches or drops the edge.
- `grim lock` no longer re-checks an existing pin, so a hand-edited digest under
  the same repository and tag survives it. For a checkout you do not trust, run
  `grim update` or delete the lock. `grim status --check` reports a pin that
  differs from its tag.

**What changes for `grim update <name>`.**

- A named run stops at a closure: the named bindings, a named bundle's members,
  and everything they require. Install and the integrity gate stop there.
  `--force` reaches the closure and what the run prunes.
- An unnamed artifact is left alone. A deleted file stays deleted, and a local
  edit no longer fails the run. A modified dependency inside the closure still
  exits `65`.
- Prune removes only what the closure held and the lock has dropped. A dropped
  client's outputs wait for a bare `grim update`, because a named run never
  reaps.
- A name moves that binding's pin alone. `grim update <parent>` leaves a
  dependency's pin where it is though its tag moved, unless the new parent
  changed the edge.
- A bundle name is accepted, where 0.15.1 exited `79`. It moves the
  bundle's own pin, and TUI Update on a bundle row does the same. Members keep
  their pins unless the new manifest changes them.
- `grim update <member>` exits `79` for a member that exists only under the
  bundle's moved tag, where it exited `0` and moved the bundle's pin. The
  unselected bundle keeps its pin. Run `grim update <bundle>` instead.
- A stale lock still exits `65`. From 0.16.0 the message says to run
  `grim lock`, then the same update.
- The report covers the closure and any lock change outside it. An unnamed
  artifact whose pin did not move has no row. See
  [the `update` rows][json-update-rows].
- Hook consent covers the workspace, not the closure. A named update that brings
  in a new hook gates every hook there, and one warning names those it disarmed.
- Managed vendor config entries follow the closure. A named run writes a
  `claudeMdExcludes` element or OpenCode's `instructions` glob only for a rule
  it names or reaches. An entry you removed by hand for another rule stays
  removed until `grim install`, `grim add` or a bare `grim update`.
  `grim uninstall` and the TUI delete write none back.
- Hook arming follows the closure. A named run writes the client registration
  and dispatch row of a hook it names or reaches, and leaves other hooks' as they
  stand. `grim uninstall` and a TUI delete remove their own hook and write no
  other. To arm a hook you only just allowed, or to restore one you disarmed by
  hand, run `grim install`.
- A registration grim 0.15.0 wrote still runs the retired `bin/grim-hook`
  script, which cannot enforce a gatekeeper's verdict. A named run,
  `grim uninstall` and a TUI delete rewrite it anyway while the hook keeps its
  dispatch row, and a warning names the hook.

**What refuses from 0.16.0.**

- `grim lock`, `grim add` and `grim update` exit `78` on a lock they cannot
  parse, before anything is written. They used to read it as no lock, exit `0`
  and overwrite it, which moved every pin.
- Deleting the file moves every pin too. Restore it from git, or resolve the
  conflict markers by hand.
- A lock only a later grim reads exits `78` as well, and the message says to
  upgrade grim. Restoring it from git only returns the same file.
- A `grimoire.lock` that links to a file that is not a lock refuses the same
  way, and the link and that file stay as they were. Up to 0.15.1 grim
  overwrote that file through the link. A link to a real lock keeps working.
- `grim remove` and `grim uninstall` still run on an unreadable lock. They warn
  once and exit `0`, and the lock stays as it is.
- `grim remove` of a bundle that hid a nested include leaves the lock stale until
  `grim lock`, and it says so on stderr. `grim install` exits `65` until then.
- `grim install` exits `65` after `grim remove` or `grim uninstall` on a stale
  lock that no longer answers its declarations. A stale lock that answers every
  declaration restamps, and `install` keeps exit `0` there. A pin with no tag
  cannot show what it answers, so a lock 0.15.1 wrote stays stale after such an
  edit.
- `grim uninstall` keeps the files of an artifact a bundle with no member list
  provides. The local store or the bundle's stamp on its entry shows it.
  0.15.1 deleted them and the next `grim install` after `grim lock` brought them
  back. An artifact with no direct declaration reports `kept-by-bundle`, not
  `uninstalled`, and the exit stays `0`.
- A lock write exits `79` when neither the registry nor the local store holds a
  carried pin's manifest. The message names `grim update <name>` and bare
  `grim update`, or the `--marketplace` command for a marketplace plugin. When
  the tag is gone too, or a digest names the pin, it names a reference that
  exists plus `grim lock`. `grim install` on a cold clone names the same remedy.
  This reaches a tagless pin on a lock with no member list too, which 0.15.1
  resolved afresh at exit `0`.
- `grim update --marketplace <manifest> <plugin>:<member>` exits `79` for a
  member only a moved bundle tag would bring, on a plugin that records that
  bundle's digest. 0.15.1 added the row at exit `0`, and c5e3824 exited `78` where
  the moved tag brings a conflict. Over a part this grim wrote with dependencies,
  0.15.1 exits `78`, since it cannot parse the `requires` field. Select `<plugin>`,
  which moves the bundle and brings the member.
- A `grim update --marketplace` pick of a member or bundle exits `78` and writes
  nothing in one case. The pick would keep a dependent at its pin beside another
  version of a row it requires. The dependent's own tag moved onto that version.
  0.15.1 exited `0`, or `79` on a bundle selector. The row it requires can be a
  bundle too, or a bundle nested in one at any depth. Where the republished list
  nests that bundle, 0.15.1 exited `65` on a part it wrote instead, since it
  refuses the nested bundle. The refusal needs proof, from the lock's records or
  from a manifest or tag read that succeeded. A failed read refuses nothing by
  itself. Select `<plugin>` to move both.

**What stops failing.** A job that read one of these failures as a signal loses
it.

- `grim lock` and `grim add` exit `0` on a diamond whose edges parted after a tag
  moved, where they exited `78`. Bare `grim update` still exits `78`.
- `grim add` of a bundle member beside a dependent whose entry records no edges,
  with the install, exits `0` and keeps the pin. That holds where the member's
  moved tag names a manifest whose dependencies conflict or form a cycle back at
  the pinned digest or another. Grim 0.15.1 exited `65` there. c5e3824 exited `78`
  on a conflict and `65` on a cycle. Online, a gone tag below the moved manifest keeps the pin too. There c5e3824 exited `79`, and 0.15.1 exited `0` with `--no-install` and `65` with the install. Offline that add still exits `69`, as both did.
- `grim update <names>` exits `0` beside bundles whose moved tags conflict, where
  0.15.1 exited `78`. So does a `<plugin>:<member>` or `<plugin>:<bundle>` pick on
  a plugin that records the other bundle's digest. For `grim lock` that is the
  diamond above. The conflict there comes from a moved tag the run never reads,
  since a pick reads that bundle at its recorded digest. The owner ratified this
  on 2026-11-08 for a pick beside a pinned sibling whose moved tag conflicts. That
  pick exits `0` and keeps the sibling at its recorded digest, where c5e3824
  exited `78`. Grim 0.15.1 exited `78`, or `65` on a plugin with a nested bundle.
  A whole-plugin update still refuses.
- A `<plugin>:<bundle>` pick keeps a bundle nested in it at the digest the plugin
  records, and keeps the bundle's members at their pins, and exits `0`. The
  unreleased c5e3824 followed the moved tag of the nested bundle or member and
  exited `78` on the conflict it brought. Grim 0.15.1 exited `65` on the nested
  bundle and `78` on the member. The owner ratified this on 2026-11-08. A pick
  that names the nested bundle or the member, and a whole-plugin update, still
  move it.
- Offline, a `<plugin>:<bundle>` pick exits `0` on a home the same pick warmed
  online, where 0.15.1 and c5e3824 exited `81`. Their online pick refused with `79`
  and cached nothing.
- On a plugin grim 0.15.1 last resolved, a `<plugin>:<member>` pick of a member
  its bundle dropped exits `0` when a kept row still requires it. The pick keeps
  the member for that row, as c5e3824 does. Grim 0.15.1 exited `79`, since the
  name was unknown. The owner ratified this on 2026-11-08.
- Bare `grim update` and a run on the whole plugin still exit `78` on that
  conflict. So does a name or pick that selects a
  moved bundle, unless the other bundle names the shared entry only
  under its own moved tag. For a pick, that exception holds only on a plugin that
  records the other bundle's digest. A `<plugin>:<bundle>` pick exits `78` too
  where the bundle it names newly nests one whose moved list conflicts with a
  direct include. That holds on a plugin that records no digest for it, where
  0.15.1 refused the nested bundle at `65`. A `<plugin>:<bundle>` pick of a bundle
  that nests nothing meets the same conflict there too. It exits `78` where 0.15.1
  and c5e3824 exited `79`, since the pick is no longer refused as a selector.
- A `<plugin>:<member>` or `<plugin>:<bundle>` pick exits `0` when the tag of a
  bundle it does not select is withdrawn, where 0.15.1 exited `79`. That holds on
  a plugin that records that bundle's digest, while the registry still serves
  that digest. A run on the whole plugin still exits `79`.
- `grim lock` exits `0` after `grim remove` of a direct declaration that held a
  version conflict off, and keeps that key's pin, where it exited `78`.
  `grim update` still exits `78` on that conflict.
- A declared artifact's tag, or a bundle member's, can be withdrawn while the
  registry still serves its digest. `grim lock` then keeps the pin at exit `0`,
  where 0.15.1 exited `79`. `grim add` of the same reference keeps it too, where
  0.15.1 exited `79`, and for a member also offline, where it exited `69`. Both
  hold on a clean lock 0.15.1 wrote, and for a member on one with no member list
  for its bundle. On a clean lock 0.15.1 wrote, any other `grim add` keeps the pin
  too, where 0.15.1 exited `79`, and so does TUI Install. Where another artifact
  asks for the member at another tag, the add still reads the member's tag and
  exits `79` when it is gone.
- Offline on an empty `GRIM_HOME`, `grim add --no-install` of a reference the
  lock pins keeps that pin at exit `0`, where 0.15.1 exited `69`. Without
  `--no-install` the add saves `grimoire.toml` and the lock, then exits `81`
  where it exited `69`, as the empty store holds no files to install. Both hold
  on a lock 0.15.1 wrote over one skill, its tag moved or not, and for a bundle
  member, its tag unchanged, moved or withdrawn. A bundle member's
  `--no-install` add keeps its pin at `0` on a store an online add filled too,
  where 0.15.1 exited `69`. That holds beside a dependent where the member's moved
  tag names a manifest whose dependencies conflict. It holds too where they form a
  cycle, back at the pinned digest or at another.
- `grim install` exits `0` after `grim remove` of a direct declaration that a
  bundle still lists at a reference its pin's digest answers, where it exited
  `65`.
- `grim lock --offline` exits `0` when the lock pins everything the
  declarations reach: on a clean lock, after `grim remove`, or after you delete
  a line by hand. It exited `69` there whenever a bundle was declared, or when
  its tag cache lacked a tag. A lock whose pins carry no tag yet, as 0.15.1
  writes them, still exits `69` after a deleted line without that cache.
  After a deleted line the entry leaves the lock,
  so the next `grim install` exits `0` where it exited `65`. Beside a dependent
  whose dependency list the store lacks, the entry leaves too, and `grim
  install` refuses the dependent with `65` online, as before. Offline it exits
  `81`. Where 0.15.1's offline lock exited `69` and left the lock stale, its
  install gave `65` there.
- Offline, `grim update <names>` exits `0` when an artifact outside its names has
  no content in the local store, where 0.15.1 exited `81`. Bare `grim update` and
  `grim install` still exit `81` there.
- `grim update <member>` exits `0` for a member its bundle's moved tag no longer
  lists, where 0.15.1 exited `79`. So does a `<plugin>:<member>` pick on a plugin
  that records the bundle's digest. The bundle keeps its pin. The member resolves
  the reference that pin lists for it, so its own pin moves only if that tag moved.
- `grim update --marketplace <manifest> <plugin>:<bundle>` exits `0` and moves the
  bundle, where 0.15.1 exited `79`. Each row that carries the bundle's stamp
  records its new digest. It still exits `79` on a plugin with no lock entry yet. It also still
  exits `79` for a bundle only a `requires` edge reaches, when 0.15.1 last wrote
  the plugin and its `include` lists no bundle. Select `<plugin>` in both cases.
- Offline over a `marketplace.lock` 0.15.1 wrote, `grim export plugin`,
  `grim export marketplace` and `grim update --marketplace`, whole or with a
  selector, exit `0` where 0.15.1 exited `81`. That holds while the tag cache
  answers every tag the run reads, each included bundle's among them. Otherwise
  they still exit `81`, and so does an export whose pinned content the store
  lacks. An export follows those cached tags and warns what that moved.

**Other paths.**

- `grim add` and TUI Install re-pack every path source, as `grim lock` does.
- `grim remove` and `grim uninstall` beside a bundle with no member list keep an
  artifact the local store or the bundle's stamp shows it provides. Such a lock
  was written by grim 0.3 or earlier, or had its `[[bundle]]` rows cut by hand.
  The entry takes that bundle's stamp, so `grim install` keeps exit `0`. Any
  other artifact drops as on 0.15.1, and the next `grim lock` may resolve a real
  member afresh: run `grim lock` once before such an edit to keep the pins.
- On a lock 0.15.1 wrote, `grim remove`, `grim uninstall` and a TUI delete read a
  dependent as requiring nothing, as 0.15.1 does. An artifact only that
  dependent requires leaves the lock. The next `grim lock` resolves it afresh, so
  its pin moves if its tag moved. Where the local store holds the dependent's
  manifest, a note names both, unless the store shows that the dependent's
  dependency list never reaches that artifact through entries the lock pins.
- After upgrading, run one online `grim lock` before `grim remove`,
  `grim uninstall` or a TUI delete. The lock then records each dependent's
  edges, and the edit keeps the pins they reach. The first `grim install` on a
  lock that holds such a dependent already exits `65` and names that
  `grim lock`. If that `grim lock` exits `78`, settle the conflict it names
  first. If the edit came first, restore the edited config and `grimoire.lock`, run
  that `grim lock`, then repeat the edit to keep the pins. The config is
  `grimoire.toml`, the file `--config` names, or with `--global` the one in
  `$GRIM_HOME`. Where that pair is not in the working directory, as under
  `--config` or from a subdirectory, the note names both by absolute path.
  A lock a hand edit already made stale resolves every tagless pin once, so a pin
  whose tag moved can still move.
- `grim remove` and `grim uninstall` keep the digest of every row they keep and
  write no tag on a tagged pin. A kept pin keeps the tag that resolved it, beside
  the request that survives.
- A key whose declaration left, with surviving edges its pin names only in part,
  keeps its row and leaves the lock stale. The next `grim lock` keeps that pin. If
  its pin names none of them, the row is dropped and the lock stays stale, so the
  next `grim lock` resolves those edges.
- An edit that changes the rows of a lock it leaves stale writes a hash no
  declaration produces. Restoring `grimoire.toml` by hand then does not make it
  read fresh. See [`grim remove`][remove].
- When two repositories agree on one digest, a recorded edge in either one answers
  the pin. No tag resolves, and a moved tag moves nothing.
- `marketplace.lock` follows the same rule. `grim update --marketplace` moves its
  pins and an export keeps them. A bundle a plugin reaches, by include, by
  `requires` or through another bundle, keeps its pin in the rows below it, where
  `bundle_tag` reads `<tag>@<digest>`. A run on the whole plugin moves it, and so
  does a `<plugin>:<bundle>` pick of that bundle. A pick of a bundle that nests it
  moves it only where the plugin records no digest for it. Every other write keeps
  it, whichever parent reaches it, and so does `grim export`.
- A plugin 0.15.1 wrote records no digest. Its first write that covers the whole
  plugin follows each bundle's tag and records the digest: an export of the changed
  plugin, or `grim update --marketplace` on it. The export warns once, naming the
  bundle and every row below it that changed. That includes rows below a nested
  bundle that was swapped or let go.
- A `<plugin>:<member>` update keeps every row outside its selection as the lock
  holds it and records no digest, as 0.15.1's member update did. A kept row with no
  tag in a plugin that records a digest was rewritten by 0.15.1 since. A warning
  names `grim update --marketplace` on the plugin, which records its tag.
- `grim lock --offline` on a clean lock with a bundle succeeds from the lock's
  own rows. It warns when the store no longer holds the bundle's manifest.
- An offline run warns about each pin it moved through the tag cache, which can
  predate the lock. It also warns about each row a tag read added or dropped.
  That covers an export and a `[[bundle]]` row's own pin. A marketplace warning
  names the plugin.
- An offline export or member update whose recorded bundle digest the store
  lacks exits `81` and writes nothing. A plugin 0.15.1 wrote records no digest,
  so these runs exit `0` there, as listed above.

**If part of the team stays on 0.15.1.** Observed on a 0.15.1 build:

- It reads a lock with tags. `status`, `install` and `lock` exit `0`, and
  `install` writes the pinned bytes even after the tag moved. A project with a
  settings package is the exception, because 0.15.1 refuses the settings kind.
  See [Forward compatibility][stability-forward-compat].
- A project with [declared dependencies][dependencies-new-grim] is an exception
  too. 0.15.1 refuses the lock's dependency rows, so its `status` and `install`
  exit `78`. Its `lock`, `add` and `update` read that lock as absent and exit
  `0`. They move every pin whose tag moved and drop the dependency rows.
  `update` also deletes the files of those dependencies.
- The online `grim lock` that keeps pins through `grim remove` records each
  dependent's edges. 0.15.1 then refuses that lock, as above. A team that keeps
  0.15.1 skips it, and an edit then drops what only a dependent requires, as
  0.15.1 does.
- Its writes strip the tags, and its `grim lock` and `grim add` still roll every
  moved tag. When the lock's hash matches `grimoire.toml`, the next write by
  0.16.0 or later reads it as clean, keeps those pins and adds the tags back.
- While both versions write, every `pinned` line changes at each hand-over. No
  digest changes unless a tag moved under 0.15.1.
- A `grim remove` by 0.16.0 that leaves the lock stale can keep a row 0.15.1's
  `remove` drops. When it also changes the lock's rows, it writes a hash no
  declaration produces. 0.15.1 reads that lock as stale.
- On such a lock 0.15.1's `status` reads `stale` on every row and `install`
  exits `65`. Its own `grim lock` resolves from scratch and exits `0`, or `78`
  where the sources disagree. Offline it exits `69`, unless no bundle is
  declared and its tag cache holds every tag.
- It reads a `marketplace.lock` whose `bundle_tag` holds a digest, and a fresh export
  leaves the file alone. Its member update writes the value back. A row it resolves
  again loses the digest, and a whole update or a stale export does that to every
  row. Where no row records a digest, the next write by 0.16.0 or later follows the
  bundle's tag and records one.
- It exits `0` when a recorded bundle's manifest is gone, because it reads no digest.
  0.16.0 exits `79` on that lock until a run on the whole plugin moves the bundle.

**If 0.15.1 ran `grim remove` on a stale lock.** It restamps such a lock, so
after an edit from `a:1` to `a:2` the lock reads as clean. The pin keeps the old
digest under the new tag: `a:2@<digest of a:1>`. Any 0.15.1 `remove` or
`uninstall` does this, of any artifact. That includes a lock this grim left
stale.

Only a resolve can tell this from a clean lock, and a resolve at upgrade would
move every moved tag. To find such a pin, run
`grim status --check --format json`, whose row reads `update_available: true`.
Then run `grim update <name>`.

**If a script reads the JSON.** The [report shapes][json-shapes-items] gain
these:

| Report | New | Meaning |
|---|---|---|
| `lock` | `disposition` on every row | `added`, `carried`, `moved` or `dropped`, against the previous lock |
| `lock` | `action: "dropped"` on a new row | A pin no longer in the lock, named in `pinned` |
| `add` | `moved` | The pins this add moved, the target included when it moved, `[]` when none |
| `add` | `dropped` | The pins this add took out of the lock, `[]` when none |
| `status` | `moved_nested_bundles` | The moved nested bundles a row reaches, `null` unless `--check` ran |
| `update` | `action: "restored"` | Files this run wrote at an unchanged pin, `old` equal to `new` |

A consumer that treats `action` as a closed enum meets two new literals, `dropped`
on `lock` and `restored` on `update`. `dropped` arrives on a new row for a pin no
longer in the lock. On a fresh runner with no install record, `update` rows read
`restored` where they used to read `unchanged`.

`grim install` rows read `unchanged` where they read `updated` for an output this
pass does not write, such as one of a client outside the target.

**What to do.**

1. Run `grim lock` once with the new grim before you hand-edit `grimoire.toml`,
   then commit its diff. That diff only adds tags unless the project uses
   [declared dependencies][dependencies-new-grim]. Then it also records their
   edges, settling each as a fresh resolve does, and 0.15.1 refuses that lock.
   An edit first makes the next write resolve every tagless pin once. One
   warning names those pins and says which moved.
2. Replace `grim lock` with `grim update` in any job that relied on it to roll
   tags forward.
3. Upgrade the team and CI together if you can, which avoids the churn above.

The compatibility contract behind this note is in
[Additive fields][stability-additive].

## A pool reader joins the pool beside a pool writer {#shared-pool-auto-join}

Some clients read the cross-vendor `.agents/skills` pool but install into a
directory of their own: [OpenCode][gap-shared-pool], Cursor, Copilot, Warp,
Droid, Kilo and Cline. Select one of them beside a client that writes the
pool, such as Codex or Gemini, and earlier versions installed each skill
twice. OpenCode then logged `duplicate skill name` on every start and kept
whichever copy it read last.

**What you will see.** Such a reader now installs its skills into the pool
whenever a pool writer is in the set, so each skill exists once. An existing
two-copy install moves on the next `grim install`, bare `grim update`, or
`grim update` that names or reaches the skill. That run removes the reader's own
copy, and [`grim status`][status] reads `installed` afterwards. A named update
that leaves the skill out leaves its copies too. `status` lists the pool path
under `outputs_pending` until one of those runs. `grim update` reports the move
as `restored`, and `grim install` as `updated`. A copy you
edited by hand is never deleted. grim keeps it and warns, naming both paths,
so you can merge your edit and delete the extra copy yourself.

**`shared_skills = false` now persists.** The
[`[options.vendors.<name>].shared_skills`][options-vendors] key used to accept
only `true` as a stored value. Setting `false` removed the entry, because
`false` was the default. It is now a third answer, "always use the client's
own directory", so [`grim config set`][config] stores it, `get` prints it, and
`config list` shows the row. `grim config list --all` reports the key's default
as `null`, meaning automatic, where it used to report `false`. Unsetting the
key returns the client to the automatic choice.

**What to do.** Nothing, in most cases. To keep the old layout for one client,
set its `shared_skills` to `false`. A downgrade to an earlier grim writes the
reader's own copy again beside the pool copy: two copies, but nothing is lost.

## Smaller notes {#smaller-notes}

- **The `--dry-run` preview digest equals the real push digest from 0.15.0.**
  Before that, the preview of `grim release` and `grim publish` printed a
  non-authoritative hash, bundles and MCP servers included. From 0.15.0 it
  hashes the exact bytes a real push sends.
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
- **An `index` source with more than 500 entries lists every entry.** The
  500-repository cap used to window index sources as well. A query, a
  [browse filter][browse-filters], `grim rate` and `grim status --check`
  reached only the first 500 rows, so `grim search` over a large index returns
  more rows. An index past 100,000 entries or 128 MiB decoded fails its fetch.
- **An `oci` source lists every repository.** The `_catalog` walk follows the
  registry's own paging to the end and no longer stops at 500. The first
  browse of a large registry takes longer. While it builds, other `grim`
  commands serve the rows built so far, and the source reads `capped: true`
  until the build finishes. An interrupted build resumes on the next command,
  and a refresh downloads a manifest again only when its digest changed. An
  `oci` cache an earlier grim capped at 500 rebuilds on first use, reusing the
  rows it already has.
- **The `oci` cap warning has one new wording.** A source whose listing is
  incomplete now warns `catalog for source '<name>' is incomplete: its listing
  has not finished, so results and the total are a lower bound; the next online grim search retries it`. The JSON `capped` and `truncated` fields keep their
  meaning, a lower bound on `total`, and no field is added or removed.
- **An older grim shows a build in progress with its 500 wording.** Run
  against the same `$GRIM_HOME`, 0.12 through 0.14 read a cache that is still
  being built as capped and warn `capped at 500 repositories` until the
  cache's one-hour lifetime expires. Nothing breaks. If it rebuilds the cache
  itself, it writes a 500-row cache that this grim then continues.
- **A cached index listing capped at 500 rebuilds on first use.** An earlier
  grim cached such a listing with `truncated: true`. This grim rebuilds it on
  the next command instead of serving it until the one-hour TTL expires. If
  that rebuild fails, it serves the capped rows with the incomplete-listing
  warning. `grim search --refresh` forces the rebuild.
- **`grim search` gains `total`, `truncated`, `--limit` and `--offset`.** The
  search document carries `total` and `truncated` beside `items` and
  `sources`. The `--limit` flag cuts the printed rows and `--offset` skips
  rows first, so a script can read every page. The MCP `grim_search` tool
  takes the same `limit` and `offset`. All four are additive.
- **A failed refresh serves the cached catalog.** A source that was
  unreachable, answered `5xx`, sent a body over the size cap or sent an
  unparsable file used to return an empty group. When a cached catalog
  exists, grim serves its rows and warns with the error and the time the
  cache was built. `grim rate` resolves against those cached rows instead of
  exiting `79`.
- **`ok: false` no longer means no rows.** A consumer that read it that way
  will see rows beside it, because the `sources` array still reports the
  failure while `items` carries the cached rows. `grim status --check` uses
  the cached rows too, and reads `null` only when no cache exists. With no
  cache the source degrades to an empty group, and the exit code stays `0`
  either way.
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
- **A plain table escapes hidden characters in a record.** Every plain table
  prints a hidden code point from a registry or index record as `\u{hex}`.
  That covers control characters, bidi controls, invisible format characters
  and tag characters, but not a well-formed flag sequence. A tab or line feed
  inside a cell prints as one space, because a cell never spans lines. JSON
  output is unchanged, and plain output carries [no compatibility
  promise][stability].

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
[export-plugin]: ./commands.md#export-plugin
[export-marketplace]: ./commands.md#export-marketplace
[marketplace-table]: ./configuration.md#marketplace-table
[plugin-metadata]: ./configuration.md#plugin-metadata
[install]: ./commands.md#install
[compression]: ./publishing.md#compression
[describe-compression]: ./json-interface.md#describe-compression
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
[index-discovery]: ./package-index.md#consuming-discovery
[index-backoff]: ./package-index.md#consuming-backoff
[index-stats-rule]: ./package-index.md#consuming-stats
[data-layout]: ./configuration.md#data-layout
[commands-context]: ./commands.md#context
[why]: ./commands.md#why
[uninstall]: ./commands.md#uninstall
[lock]: ./commands.md#lock
[lock-pins]: ./commands.md#lock-pins
[add]: ./commands.md#add
[status-check]: ./commands.md#status-check
[json-shapes-items]: ./json-interface.md#shapes-items
[json-update-rows]: ./json-interface.md#update-rows
[stability-additive]: ./stability.md#frozen-additive-fields
[stability-forward-compat]: ./stability.md#limitations-forward-compat
[dependencies-new-grim]: #dependencies-need-new-grim
[fetch]: ./commands.md#fetch
[describe]: ./commands.md#describe
[gap-shared-pool]: ./clients.md#gap-shared-pool
[options-vendors]: ./configuration.md#options-vendors
[config]: ./commands.md#config
[search]: ./commands.md#search
[settings]: ./settings.md
[json-update]: ./json-interface.md#shapes-items
