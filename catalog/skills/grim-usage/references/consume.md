# Consumer Lifecycle

You loaded this file because you are installing, updating, or removing
artifacts in a project with grim — the init → add → lock → install →
update loop and the two files it maintains.

Contents: [The Loop](#the-loop) · [The Two Files](#the-two-files) ·
[Declaring](#declaring) · [Installing](#installing) ·
[Updating](#updating) · [Inspecting](#inspecting) ·
[Removing](#removing) · [Bundles](#bundles) · [Hooks](#hooks) ·
[Settings](#settings)

Flags shown here track the release this package ships beside; confirm with
`grim <cmd> --help` before relying on one.

## The Loop

A complete first session, start to installed:

```sh
grim init                                # write grimoire.toml
grim add ghcr.io/acme/code-review:1      # declare, pin, and install
grim status                              # confirm what landed
```

No registry setup is required: out of the box grim browses the public
package index (`https://index.grimoire.rs`) and expands short references
against `ghcr.io/grimoire-rs`. Pulling from your own registry? Seed it as
the default with `grim init --registry ghcr.io/acme` — see
[registries.md](registries.md).

From then on the steady state is `grim update` to roll floating tags
forward and `grim status` to see where you stand.

## The Two Files

`grimoire.toml` is the declaration: an optional `[[registries]]` array (the
canonical way to set a default registry via a `default = true` entry, or
`grim config registry use <alias>`), an `[options]` table for other defaults
(`clients`, `[options.tui]` for the interactive browser), and `[skills]` /
`[rules]` / `[agents]` / `[mcp]` / `[hooks]` / `[settings]` / `[bundles]`
tables mapping a binding name to a reference (`[mcp]`, `[hooks]` and
`[settings]` take registry references only).
Manage settings and registries with `grim config` rather than by hand — see [registries.md](registries.md#managing-config); it never
touches the declaration tables, which stay under `grim add` / `grim remove`.
You may still edit by hand (run `grim lock` afterwards), but note that any
grim write strips comments and the `#:schema` directive.

`grimoire.lock` pins every declared tag to an exact digest and records a
hash of the declaration it came from, so drift is detectable. It is
machine-owned — never edit it, but **do commit it** beside
`grimoire.toml` so installs are reproducible for everyone. Full shape of
both files: [Configuration][config-toml].

## Declaring

`grim add <reference>` declares an artifact, pins it in the lock, and
installs it into your detected clients — one command from nothing to using
it. The reference is the only required argument:

```sh
grim add ghcr.io/acme/code-review:1
grim add --kind rule --name rust-style ghcr.io/acme/rust-style:2
grim add --kind bundle ghcr.io/acme/python-stack:1
grim add --no-install ghcr.io/acme/code-review:1   # declare + lock only
```

- `--kind` (skill, rule, agent, bundle, mcp, hook, settings) is normally inferred from
  the artifact's kind metadata set at release time (the `com.grimoire.kind`
  annotation; legacy `artifactType` on older artifacts). If grim cannot
  infer it (a non-Grimoire image), `add` errors and asks for `--kind`.
  `hook`, then `settings`, are **appended** to that value list, never
  inserted — the accepted set is a frozen CLI surface, so widening it is the
  only permitted direction. A hook or a settings package may be added with
  its experimental flag off: declaring and locking are not arming (see
  [Hooks](#hooks) and [Settings](#settings)).
- `--name` defaults to the reference's last path segment. Re-adding a declared
  name from the *same* repository at another tag or digest re-pins it. Config,
  lock and installed files all move to the new version. Re-adding the identical
  reference is a no-op that keeps its pin. One stderr note says so and names
  `grim update -- <name>`, which moves the pin or says why it cannot. If the tag
  is gone, the note offers to declare a reference that exists. Dropping a digest
  from `:1@sha256:…` back to `:1` keeps that digest until `grim update <name>`.
  A name already
  declared for that kind under a *different repository* refuses (exit 64)
  instead of silently replacing it — pass `--name` to bind the new
  reference under another name. A renamed skill installs under the binding name with its
  `SKILL.md` frontmatter `name` rewritten to match (no client-level name
  collision); a renamed multi-file rule may break the index's relative
  links (grim warns). Skill/rule/agent binding names must be 1–64 chars —
  `[a-z0-9]` runs joined by single `.` or `-` (exit 64 otherwise);
  installing a name already installed
  at the other scope for the same client warns about the shadow.
- `--no-install` stops at declare + lock — no materialization. Use it to
  add several artifacts before one `grim install`, or to pick clients with
  `grim install --client`. Only the added entry installs otherwise; the
  rest of the lock is left for `grim install`.
- Install-on-add honours the same integrity gates as `grim install`: a
  locally modified artifact, or a pre-existing destination grim has no
  record of, refuses (exit 65; JSON `reason` `modified` /
  `untracked-destination`). Re-run the same `grim add` with `--force` to
  overwrite deliberately — identical semantics to `grim install --force`,
  refused [settings](#settings) values included.
  `--force` is inert with `--no-install` (nothing is materialized).

If the reference is deprecated, `add` prints the publisher's deprecation
notice on stderr and still completes the add — treat it as a prompt to
look for a successor, not a failure.

`<reference>` may also be a local path (`./skills/x`, `../shared/rule.md`,
or an absolute path) for a skill, rule, or agent — declared verbatim and
pinned by the SHA-256 of its canonical packed layer instead of a registry
digest, then installed the same way:

```sh
grim add ./skills/my-skill
grim add --kind agent ../shared/reviewer.md
```

An **MCP server**, a **hook** or a **settings package** has no path form: the
config parser refuses a path value under `[mcp]`, `[hooks]` and `[settings]`,
and none of the three has a dev-install either (`grim install <path>` takes a
skill, rule or agent only). All three always come from a registry reference. See
[Hooks](#hooks) and [Settings](#settings).

The kind infers from the path's shape the same way `grim build` does (a
directory with `SKILL.md` is a skill, a bare `.md` is a rule; pass
`--kind agent` for an agent). A relative CLI path is rewritten
config-dir-relative before it is declared, so it stays portable across
clones; an absolute path is declared verbatim and warns in project scope
(not portable to another machine). Windows `\` separators in a CLI path
are accepted and normalized — the declared value is always the
forward-slash form. A bundle has no path form via `add` —
declare it in `[bundles]` directly and run `grim lock` instead; see
[Bundles](#bundles) below.

`grim lock` brings the lock in line with `grimoire.toml`: it pins what the
declaration added or changed and **keeps every existing pin**, even when its
tag has since moved. You need it only after hand-editing the config — `grim
add` already locks what it declares. To roll a tag forward run
`grim update <name>`. To see that a tag moved, run
`grim status --check` (`update_available`). `grim lock` does not re-check an
existing pin: for a checkout you do not trust, run `grim update` or delete
the lock.

A few pins still resolve on a relock: a local path source, a kept pin a new
dependency edge disagrees with, a tagless pin nothing vouches for, and a pin on
a stale lock that names none of its recorded edges. A warning names each pin
that moves this way.

A direct declaration's pin also wins over what a dependency asks for. When a
new or moved parent requests a reference that resolves to another digest than
the held pin, `grim add`, `grim update` and `grim lock` exit 0 with the same
lock and warn once per held artifact. A path parent counts when it is new or
its content changed. The warning names the requester, the requested reference
and the pin, and offers `grim update -- <name>` or declaring the requested
reference. A declaration pinned by digest is offered only the second. The probe
reads at most 64 references and stops after 60 seconds, dropping the rest
quietly. `grim status --check` lists the cases that stand, a declared bundle's
included. An offline run and a marketplace part run print none.

`grim add` at the reference a held pin answers, as a dependency, a bundle member
or a line you took out, keeps that pin. Typing the line and running `grim lock`
keeps it too on a lock this grim wrote.

On a lock grim 0.15.1 wrote, no pin
records a tag. A typed or deleted line leaves that lock stale, so the relock
resolves every such pin once. One warning names them and says which moved. A
`grim lock` or `grim add` run before the edit keeps them.

`grim add` of the line keeps the pin at exit 0 even where the dependencies of a
moved tag's manifest conflict. It keeps it too where they form a cycle, back at
the pinned digest or at another one. It keeps the pin past a gone tag below that
manifest too. An
unreachable registry, a refused credential, a timeout or a graph cap below it keeps
its exit. The relock after a hand edit exits 78, 65 or 79 there and writes nothing.

Where an edge 0.15.1 never recorded asks for a bundle member at another tag, the
add reads the tag and what its manifest asks for. A gone tag you typed refuses it
with 79. A gone tag below that manifest does not, and `grim lock` keeps the pin.

Every other dependency pin stays while the lock's hash matches, whichever tag
moved and in whichever repository. Deleting a direct line by hand keeps the pin
at exit 0 even where a fresh resolve exits 78. `grim update` then exits 78
naming the conflict, and `grim status --check` may read `false`, because it
asks only the pin's own tag while an edge names it.

A lock write exits 79 when neither the registry nor the local store holds a
kept pin's manifest, because only `grim update` may move that pin. Run the
`grim update -- <name>` the message names, or bare `grim update` when another pin
is gone too. If the tag is gone too, or a digest names the pin, the message
says to declare a reference that exists and run `grim lock`. Each command it
names carries the run's `--global` or `--config`. A marketplace plugin's message names its
`grim update --marketplace` command instead.

Under `--format json`, each `grim lock` row carries `disposition` (`added`,
`carried`, `moved` or `dropped`: how the pin relates to the previous lock),
and `grim add` reports `moved`, the pins it moved (an artifact target included
when the previous lock pinned it elsewhere), and `dropped`, the pins it took out
of the lock (`[]` usually, and plain output names each list on one stderr line).
A moved `[[bundle]]` pin has no row there and gets its own stderr line. A
`grim lock` row with action `dropped` names a pin that left the lock. Neither
command deletes files: `grim uninstall <kind> <name>` removes them, and a bare
`grim update` prunes them too but moves every pin. Treat unknown values as
additive.

An unreadable `grimoire.lock` (merge-conflict markers, bad TOML) makes
`grim lock`, `add` and `update` refuse with exit 78, because relocking
without it would move every pin. Restore it from git or resolve the
conflict; do not delete it, which re-resolves every tag.

`grim install` exits 65 on a stale lock, and `grim remove` can cause that. If a
removed declaration hid an edge its pin only partly answers, `remove` keeps the
row and leaves the lock stale. Restoring `grimoire.toml` by hand does not make
it fresh again, because such an edit writes a hash no declaration produces when
it changed the lock's rows. An edit that changes no row keeps the hash.
`grim lock` then keeps that pin and writes the hash back. A pin that names no
recorded edge can move, and a warning names it. `grim update` exits 78 if the
edges truly disagree.

## Installing

`grim install` materializes every locked artifact into your AI clients'
configuration directories. Because `grim add` already installs the entry it
declares, you reach for `grim install` to materialize the *whole* lock at
once — after cloning a repo, in CI, or after a batch of `grim add
--no-install`. It writes into the clients selected by `--client`, the
config's `clients` option, or auto-detection (details in
[registries.md](registries.md)):

```sh
grim install
grim install --client claude,copilot
```

Install never deletes anything, and it refuses to overwrite an artifact
you have modified locally — or any pre-existing same-named file it has
no record of writing — pass `--force` to overwrite deliberately. The same
gate holds for a [settings](#settings) value that changed after grim wrote it.
A refused run exits 65 and names the first refused artifact. One stderr warning
names every other refused file artifact, so one `--force` rerun covers them all.
See [troubleshooting.md](troubleshooting.md) for the integrity gates.

Install also refuses when the lock is stale. If `grimoire.lock`'s
`declaration_hash` no longer matches `grimoire.toml`, it exits 65 and
tells you to run `grim lock` first. This check runs before the two gates
above.

`grim install <path>` — the same local-path form as a positional argument
— is a different move: a throwaway dev-install. It renders a skill,
rule, or agent straight from disk into your clients **without** declaring
it (`grimoire.toml` and `grimoire.lock` stay untouched):

```sh
grim install ./skills/my-skill
```

The install record is still written, marked `dev`, so it stays visible
to `grim status` (`Source` reads `path: <path> (dev)`) and is refreshed by
`grim update` on content drift — but it is never pruned automatically;
`grim uninstall` is the only way to drop it. Reach for `grim add <path>`
instead when you want the source declared and shared with co-workers.

A dev-install refuses (exit 64) when the local artifact's own
`(kind, name)` — a skill's frontmatter `name`, or a rule/agent file's
stem — already matches a binding declared in `grimoire.toml`: remove the
declaration, or dev-install the local artifact under a different name or
`--kind`, before retrying. A name the lock installs through a bundle or a
dependency refuses the same way, and so does a `grim add` that would bring
in a dev-installed name. A plain `grim install` that meets a dev record
under a locked name exits 64 too; `--force` replaces the local copy.
Confirm the current flag set with `grim install --help`.

## Updating

`grim update [names…]` re-resolves floating tags, rolls the lock
forward, and re-materializes only what changed. It is the command that moves a
pin whose reference you did not change. With no names it moves every pin. A name moves that binding's
pin alone: a dependency keeps its pin even after its tag moved, unless the
binding's new version changed the edge. A bundle name moves the bundle, and a
member whose reference it left alone keeps its pin. A named run installs and
gates the closure of its names (they, a bundle's members, and all they
require) and never reaps clients. Its `items` are the closure, pruned rows and
any other lock change, so never read a short `items` as the whole lock: an
unnamed artifact whose pin held has no row. An unknown name exits 79 with a lock
and warns at exit 0 without one, and a stale lock exits 65:

```sh
grim update
grim update code-review rust-style
```

Update runs the **same integrity gate as install**. A new pin overwrites
grim-managed content with no flag — that is the whole point of a rolling
update — but an artifact whose bytes you edited yourself is refused with
exit 65 until you pass `--force`:

```sh
grim update           # exit 65: "installed artifact was modified locally"
grim update --force   # overwrite my edit deliberately
```

One flag therefore governs every way update can destroy hand-edited work:
this overwrite, a refused [settings](#settings) value, the prune below, and
the client reap below that. On a named run it reaches only the closure and
its prune, and it authorizes no reap. An edit outside the closure neither
fails the run nor gets overwritten.

Settings values follow the closure too:
an edited or deleted value outside it leaves a named run at exit 0. `grim install` then
refuses the edit and restores the deleted value. An edited value of a settings
package the named run prunes refuses without `--force`. The next named run no
longer sees it, so the refusal names `grim uninstall --force settings <name>`.

A value that a package outside the closure also holds stays when a package
inside drops it. So does one an outside package declares, if this run could
apply that package, and a warning names `grim install`, which hands it over.

When that value was changed outside grim, a plain install refuses the edit. The
warning says to delete that value or restore it to the value grim wrote, then
run `grim install`. That install still refuses every other edit, and it writes
no settings while it refuses any other settings value at the scope. The warning
names `--force` as the other route, which overwrites every file and value
changed outside grim at the scope, another artifact's local edit included. It
also removes the edited values of packages that left the lock.

The warning repeats `--trust-workspace` when only that flag armed the outside
package, or a hook the record would leave unarmed. It also says the flag arms
every hook and applies every settings package the workspace was not consented
for. If the outside package's layer cannot be read, the settings pass stops
before it writes, keeps the value, and exits with that read's error.

Hook arming and vendor config entries follow the closure as well. A hook
registration or a `claudeMdExcludes` element you removed by hand for an unnamed
artifact stays removed. `grim install`, `grim add` or a bare `grim update`
writes it back.

Two hook writes reach past the closure, each with a warning naming the hook.
One is a registration that still runs the retired `bin/grim-hook` script. The
other is a registration a hook outside shares with one inside.

A refusal does not abandon the pass. Every other artifact in scope is
still reconciled, the lock still takes the pins this run moved, and the
report is still printed — this is a refusal, not an error document. Under `--format json`
the refused row carries `refused: true` (always present, `false`
everywhere else), which is how you tell this exit 65 from any other
without scraping stderr; `action` keeps reporting the lock diff, because
the pin *did* move and only the materialization was refused. The action
`restored` means the pin did not move and this run wrote the files, a client's
first copy included. The plain
table has no such column, so on that surface the refusal is named on
stderr instead. Re-run with `--force` to complete it, or
[`grim remove`](#removing) the artifact to keep your edit and stop
tracking it.

Update is also the only command that **prunes**: an artifact that
dropped out of the lock (most often a bundle member the bundle stopped
including) is deleted and reported as `removed` — unless you edited it
locally, in which case it is kept and reported as `kept-modified` until
you re-run with `--force`. Your local edits are never silently
discarded. A pruned or reaped row can also report `retained` /
`abandoned_entries` — the footprint grim dropped from its record but
deliberately left on disk; same contract as on `uninstall`, see
[Removing](#removing).

A named run prunes only what left the previous lock's closure of its names.
A record the new lock lacks outside that closure, as after a run that crashed
past its lock write, stays on disk, and a warning names it.
`grim uninstall <kind> <name>` removes it. A bare `grim update` prunes it too,
moves every pin, and needs `--force` for one you edited. A `kept-modified`
orphan of a named run leaves the next named run's view:
`grim uninstall --force <kind> <name>` removes it, and so does a bare
`grim update --force`, which also moves every pin. Every command a warning
names carries the run's `--global` or `--config`.

The same reconciliation applies when you narrow the configured client
set (`[options].clients`): a client that leaves the set has its outputs
reaped, listed per update row under `reaped_clients` — again, a locally
edited output is preserved under `kept_modified_clients` (with a warning)
until `grim update --force` removes it. Widening the set re-materializes
the added client on the next update. This reap only fires when
`[options].clients` is explicitly set; left unset (autodetect), `update`
never reaps — the desired set would otherwise track live client detection
rather than your config. A named run never reaps either, so a dropped
client's outputs wait for a bare `grim update`.

A bare update also refreshes every **local path source**: declared path
dependencies and dev-installed records alike are re-packed, and a changed
content hash re-materializes them — the local equivalent of a floating
tag rolling forward. A named run re-packs only the path entries it selected
and the dev records inside its closure. `grim status` surfaces that drift ahead of time as
`outdated`; a path source that has gone missing (or no longer packs)
reports `outdated` too, never a false `installed`. A dev record is never
pruned — see [Installing](#installing).

`grim update --marketplace <path>` is a different mode entirely, reached
only through that flag: it rolls a declared plugin's own lockfile forward
— the same re-resolution as above, scoped to a `marketplace.toml` — and
**installs nothing**. It exists to keep a plugin you shipped with `grim
export plugin` or `grim export marketplace` (see
[publish.md](publish.md#export)) pinned to fresh digests between exports,
without touching your own project's
`grimoire.toml`/`grimoire.lock` at all.

A `<plugin>:<member>` selector moves that member, as `grim update <name>` does.
Every row outside its selection keeps its pin, its tag and its `bundle_tag`.
A selector naming a member the plugin does not resolve to exits 79 and writes
nothing. That covers two cases. A plugin that records the bundle's digest
refuses a member only a moved bundle tag would bring. A plugin that records none
refuses a member the moved tag no longer lists. A `<plugin>:<bundle>` selector moves that bundle and a
member whose reference its new list changed. A member it left alone keeps its pin
where the plugin records its tag. Each row that carries the bundle's stamp
records the new digest. It exits 79 on a plugin with no lock entry yet. It does
too for a bundle only a `requires` edge reaches, when grim 0.15.1 last wrote the
plugin and its `include` lists no bundle. Select `<plugin>` in both cases.
A `<plugin>:<bundle>` pick keeps a bundle nested in it at the recorded digest, and
the bundle's members at their pins. A member or bundle pick exits 78 and writes
nothing where it would keep a dependent beside another version of a row or
bundle it requires. It refuses only on proof from the lock or a read that
succeeded. A failed read refuses nothing by itself.
Offline, a member update whose recorded bundle digest the local store lacks
exits 81 and writes nothing. Confirm the exact selector grammar and flags with
`grim update --help`.

## Inspecting

`grim status` reports each declared artifact's state: `installed`,
`outdated`, `modified`, `missing`, or `stale`. A hook row adds three more:
`gated` (feature flag off, or consent missing or drifted), `not-armed` (a
host-level refusal such as `insecure-transport`, a relative or nested
`GRIM_HOME`, a client unverified on Windows, or a surface grim cannot register) and
`untrusted` (client-side trust still pending); the reason is in the
always-present `arming[].cause` array, see
[troubleshooting.md](troubleshooting.md#a-hook-that-never-fires). A
[settings](#settings) row can read `gated` or `not-armed` too, with the same
`arming` array. See [artifact
states][artifact-states] for what each one means. The
`Source` column shows provenance: `direct`, the bundle the artifact came
from, `path: <path>` for a declared local path source, or
`path: <path> (dev)` for a dev-install. Pair with `--format json` to
drive automation — its `outputs`
array lists the per-client paths an artifact was materialized to, and is
the supported way to script against install locations (the on-disk
vendor layout itself is not a stable contract). `outputs` is filtered to
the clients grim detects right now, plus the global `options.clients` list
when the project names none of its own. It is not the clients recorded at
install time. Drop a client and its still-on-disk output vanishes from the array,
even though the file has not moved and `state` stays put. Each item also carries
`clients_missing`/`clients_extra` — the *explicitly configured*
`[options].clients` diffed against what is actually recorded installed,
computed locally, no network. Left unset (autodetect), both stay `[]` on
every item instead of diffing against live client detection.
`clients_missing` also skips a configured client whose vendor cannot host
that kind at that scope (Codex declines rules, for one), since no output
was ever going to be recorded for it. On a settings row it is always `[]`,
as on a bundle row: an install records a client only for the values the
package declares for it, which `status` cannot see. So a settings row never
reports a newly selected client, in `clients_missing` or `outputs_pending`;
run `grim install` and it applies the package to that client.

`outputs_pending` answers a different question: **what would `grim install`
write right now that the record does not already cover?** Same
`{client, path}` shape, `[]` when an install would write nothing new. Two
things fill it, both real install work — a client that gained support since
the last install (you installed another AI client, or configured one), and a
render-layout move (reported at the new path). A recorded output whose file
was deleted is not one of them; that entry reads `state: missing`. This is
*materialization drift*: the artifact is intact at
its locked pin, so `state` correctly reads `installed` while an install
still has files to write, and no other field can see it. Unlike
`clients_missing` it is reported under autodetect too, because it asks what
an install would do rather than what you configured. Fix it with
`grim install` — not `grim update`, which would also roll your pins forward.

A third
array, `clients_unresolved`, names every active client whose recorded
output could not be resolved at all — the anchor root is gone, or the
containment guard refused the path — so that client is silently absent
from `outputs`. It is `[]` normally; a non-empty one is the signal to look
at [troubleshooting.md](troubleshooting.md#containment-refusals). `state`
stays `missing` and the exit code stays `0`: status reports, it never
gates.

`grim status --check` adds one live catalog round-trip: it fills in
`deprecated`/`replaced_by` on every registry-sourced item and, via a
fresh per-artifact re-resolution, `update_available`, plus
`moved_nested_bundles`. This is how you see that a tag moved while the lock kept
the old pin. Plain output keeps its columns, and stderr names each row with an
update and each nested bundle that moved.

That re-resolution follows **the reference the row declares**, tag and
all — so `update_available` answers *would `grim update` move this pin?*,
the same decision the TUI's `↑ outdated` badge makes. It is deliberately
not "is a newer version out there right now": a release the declared
reference does not point at is **not** an available update, so an exact
version pin, an unmoved advisory float, and a digest pin all report
`false` while the repository carries a strictly higher tag. Widen the
declared reference (`grim add <ref>` re-pins it) to follow those releases.

Bundles are covered from both sides. The **bundle row** re-resolves the
bundle's own declared reference — the only place a bundle that *added or
dropped a member* ever shows up, because every member already in the lock
keeps its own pin. Each **bundle member row** separately re-resolves the id
its bundle listed, so a member sitting at a floating tag reports an update
even while the bundle itself stays at an unchanged version.

A **dependency row** answers by the tag its pin records while an edge names
it, and otherwise by the smallest edge the lock holds. An edge that names a
digest holds the row to that digest, because `grim update` cannot move past it.
It answers only while each parent recording an edge onto it is in your local
store at its pin and lists the same edges, else `null`, as on a fresh clone
until `grim install` or `grim lock` stores the parents.
A **nested bundle** has no row and never flips `update_available`, which stays
each row's own answer. A row that reaches a moved one lists it in
`moved_nested_bundles` as `{name, reference}`, sorted, `[]` when nothing moved.
`grim update -- <listed name>` moves it and can change its members, and the
note names that command with the run's `--global` or `--config`. On a stale lock the
note names `grim lock` first instead, and a dependency the lock records under
more than one reference gets no command, because a named update re-resolves
every reference.
`grim update -- <row>` leaves the listing, because that row's own pin held. All
of this covers the recorded reference only: a moved parent that switches or drops an edge,
or a diamond that diverges, is not predicted.

The top-level `checked` field says whether the check actually ran online;
`checked == false` means all four of those item fields are `null`.
When scripting against it, treat `update_available: null` as "could not
determine", never as "up to date": it is `null` (not `false`) for a row
with no lock pin (dev-install, path source, path-sourced bundle), a row with
no readable reference in the lock, a dependency row whose edges your store does
not confirm, or a row whose own re-resolution failed, ran
past its 30-second deadline or the run's cap of 2,048, or holds a tag outside the
OCI grammar. A row the run's 300-second deadline cut off reads `null` too.
Read `moved_nested_bundles: null` the same way.

Even under `checked == true`, one registry's catalog refresh failing falls back to
its cached catalog, and with none cached degrades just that registry's rows'
`deprecated`/`replaced_by` to `null` — partial data, not a failed check. A
source still in backoff after earlier failures counts as failing the same way:
grim makes no request to it and uses its cache. Confirm the current flag set
with `grim status --help`.

Multi-item reports (`status`, `install`, `lock`, `update`, `search`,
`config list`, `config registry list`, `config registry fields`, `publish`)
wrap their rows in a uniform `{"items": [...]}` envelope under
`--format json` — read the array from `items`, never the top level.
`search` also carries the siblings `sources`, `total` and `truncated`;
check them before treating a short `items` as the whole catalog.
Failures under `--format json`
emit a structured `{"error": {code, exit, message}}` document on stdout;
some failures add a machine-readable `reason` (e.g. `stale-lock` when a
partial `grim update <name>` is refused because the lock no longer matches
the declaration — retry with `grim lock`, then the update; if `grim lock`
finds a pinned manifest gone, bare `grim update` moves every pin). Full contract: the
[JSON interface][json-interface] docs page.

Two read-only companions:

- `grim context` reports the resolved invocation context — scope,
  config/lock/state paths (with existence flags), effective client set
  (names only), registry browse set, default registry, offline mode —
  so scripts need not reimplement walk-up or precedence rules. `lock_error`
  carries the reason an *existing* lock cannot be read (else `null`):
  `lock_exists` answers "is it there", not "can grim use it". Each JSON
  `registries` entry carries an `authenticated` boolean: whether the
  docker-compatible credential store holds an entry for that registry's
  host (a file-only probe — it never invokes a credential helper, so a
  global `credsStore` alone reads as `false`). Each entry also carries
  `catalog`, that source's sync state read from the sidecar without a
  request (`null` when no sidecar is readable), so a script can tell a stale
  or backing-off source from a healthy one.
- `grim fetch <ref> [--vendor …] [--path …]` prints an artifact's
  content without installing (use != install). Plain output is the raw
  payload (pipe-able: `grim fetch skills/x > SKILL.md`); `--format json`
  adds the digest, kind, and a `files` listing. A binary `--path` file
  (e.g. `logo.png`) comes back base64 in JSON (an `encoding: "base64"`
  field marks it) while plain output decodes to the raw bytes, so a
  redirect round-trips byte-identical. `--description`
  retargets the fetch to the repository's description companion (README,
  logo, CHANGELOG) instead of the artifact — JSON inlines every member,
  plain requires `--out <dir>` to unpack the tree (no single payload to
  print). `--digest-only` resolves a digest without downloading anything —
  a cheap cache probe — and composes with `--description` to probe the
  companion tag instead. A missing artifact *or companion* is not-found
  (exit 79); offline against an uncached reference is 81, not a
  misleading 79. Confirm flags with `grim fetch --help`.
- `grim describe <ref>` reports an artifact's manifest-level metadata —
  kind, curated annotations, tags, and the verbatim annotation map —
  *without* downloading its content, so it is the cheap way to inspect a
  package or discover its versions. It also reports `has_description` —
  whether the repository has a description companion — derived from the
  tag listing it already fetches, at zero extra network cost. Curated
  fields include `deprecated` (the notice, or null) and `replaced_by`
  (the successor reference, or null); `tags[]` hides grim-internal
  companion tags. `compression` is `"none"`, `"gzip"` or `null` (a layer
  this grim cannot read); an older grim omits the key, so treat absent as
  unknown. `--format json` is a single object with every field
  always present; plain output is a flat key/value table.
  Confirm with `grim describe --help`.

## Removing

Two commands with deliberately different depths:

| Command | Config + lock | Installed files |
|---|---|---|
| `grim remove <kind> <name>` | undeclared | left on disk |
| `grim uninstall <kind> <name>` | undeclared | deleted, record dropped |

`remove` only undeclares; `uninstall` is the full inverse of install.
Both act on the **effective** declaration, fully offline: if a declared
bundle still names the artifact at the same identifier, the lock entry
survives via the bundle and `uninstall` keeps the files (a directly
declared one degrades to `remove` and a line names the bundle that keeps the
files). A bundle-only member is a no-op, so remove the bundle to remove it.

When the surviving bundle binds a *different* identifier, the pin's own
record decides. A pin that names every surviving request keeps the entry,
and a fresh lock is restamped. One that names some of them keeps the entry, leaves the lock
stale and says so. Only a pin that names none drops the entry, leaves the
lock stale, and tells you to run `grim lock` — never a silently wrong pin.
A stale lock stays stale unless it answers the edited declaration in full,
so the next `grim install` exits 65 until `grim lock`.

The target's entry also stays while an artifact may require it whose
requirements only `grim lock` reads, such as a line typed since the last lock.
A note names the row it kept, and the lock stays stale until `grim lock`.

An entry that records no dependencies, as grim 0.15.1 writes every entry,
counts as requiring nothing: what only it requires leaves the lock, and the next
`grim lock` resolves it afresh, so its pin can move. A note names that entry
where the local store holds its manifest. No note names it when the store shows
its list never reaches what left through entries the lock pins. The note's
recovery: restore the config the edit changed and `grimoire.lock`, run one
online `grim lock`, then repeat the edit. The config is `grimoire.toml`, or with
`--global` the one in `$GRIM_HOME`. Where that pair is not in the working
directory, as under `--config` or from a subdirectory, the note names both by
absolute path. A lock a hand edit already made stale resolves every tagless pin
once. The note says so, since a pin whose tag moved can still move. On a lock
0.15.1 wrote, run one online `grim lock` before `remove`, `uninstall` or a TUI
delete to record the edges and keep those pins, and 0.15.1 then refuses the lock.
The first `grim install` on such a lock already exits 65 and names `grim lock`.

"Deleted" has one documented exception, reported rather than hidden. The
JSON report of `uninstall` (and of a pruning `update`) carries two
always-present arrays, `[]` on a healthy run:

- `retained` — absolute paths grim dropped from its install record but
  refused to delete, because the recorded path resolved outside its anchor
  root. State and filesystem deliberately diverge; remove those paths by
  hand.
- `abandoned_entries` — the same idea for a managed MCP entry inside a
  shared, user-owned config file: `{path, pointer}` per entry, `path` the
  config file and `pointer` the JSON pointer of the member grim could not
  splice out. It is now unrecorded, so no later `uninstall` will remove it.

Both come from the containment guard — see
[troubleshooting.md](troubleshooting.md#containment-refusals).

**Hooks are the exception to "left on disk".** `grim remove hook <name>` drops
only the declaration and the lock entry: the dispatch row and the client
registration **keep firing** until the next `grim install` (which converges and
disarms) or `grim uninstall hook <name>` (which removes the dispatch row, the
client registration **and** the payload). grim warns that the hook stays armed.
To stop a hook now, use `uninstall` ([remove and uninstall][hooks-remove]).

## Bundles

A bundle is a curated set of members. Declare it once and it **expands**
into its members — skills, rules, agents, MCP servers, hooks, settings
packages and nested bundles — at lock time,
each pinned like a direct declaration and tagged with the bundle as its
provenance:

```sh
grim add --kind bundle ghcr.io/acme/python-stack:1
grim install
```

Membership tracks the bundle's pinned version: `grim lock` keeps the bundle
and its member pins as they are. A new bundle version, once you run
`grim update <bundle>`, adds a member (it expands into the lock) or drops one
(it leaves the lock, and `grim update` prunes its files and reports
`removed`, subject to `kept-modified` above). `dropped` is what `grim lock`
reports for a pin that left the lock, as after a hand edit.

Conflicts on the same `(kind, name)` slot resolve deterministically: a
direct declaration always wins over any bundle (the override mechanism),
agreeing bundles coalesce, and disagreeing bundles **fail closed** with
a conflict error at lock time — declare the member directly to pick a
winner. `grim remove bundle <name>` undeclares the bundle and drops only
the members no other declaration still holds.

A `[bundles]` value can also be a local path (`./bundles/x.toml`) instead
of a registry reference — a **local bundle**, resolved without a publish
step and pinned by the SHA-256 of its canonical members layer rather than
a manifest digest. Its members must still be registry references: `grim
lock` rejects a relative (`./`/`../`) member id, since a local bundle has
no registry identity of its own to resolve one against.

A bundle's member tables are `[skills]`, `[rules]`, `[agents]`, `[mcp]`,
`[hooks]`, `[settings]` and `[bundles]`; the `[mcp]` key is the name the
server registers under in each client's MCP config.

A bundle-provided **hook** is declared, never armed: it locks with the
bundle as its provenance and its own digest-pinned source, then faces the
same gates a directly declared hook faces — the feature flag, then this
workspace's consent. `grim add <bundle>` records the members that gesture
resolves to, so a bundle that later gains a hook member drifts and re-asks
rather than arming silently. Removing the bundle removes the hook, unless
you also declared it directly. A bundle-provided **settings package** is a
declaration in the same way, and meets the [settings](#settings) gates.

`grim add` records consent ahead of its install. It covers only hooks and
settings packages it newly brought in from a manifest this run resolved, behind
edges this run resolved.
Offline an edge counts only when the manifest your local store holds for its
parent lists it, or its parent is a local path this run re-packed, so a hook
behind any other edge needs `grim hook allow`. Unless you pass
`--no-trust-hooks`, an add never withdraws consent from a hook the record
covers. A failed add keeps its declaration, its lock and that consent, so the
next `grim install` arms the hook and disarms nothing. `--no-install` records
the same hook consent and no settings package, which the next `grim install`
asks about. Re-adding a hook whose pin the lock kept consents to it only
while the tag that pin records still names its digest, which needs the network.
If the tag moved, a warning names `grim update <name>` and then
`grim hook allow`, which takes the lock as it stands. With no answer within 10
seconds it stays gated and warns. A
hook the add does not consent to is drift until `grim hook allow` covers it.

## Dependencies

A skill, rule, agent, MCP server, hook or settings package can declare the
artifacts it needs
(the author's side: see the `grim-authoring` skill). Adding or locking it then
pulls the dependencies in, transitively, and a bundle can hold other
bundles. `grim add` reports what it pulled in as `pulled_in`, and any pin it
took out of the lock as `dropped`.

- **`grim status`** rows carry `direct` (you declared it) and `required_by`
  (the declared artifacts that reach it; `null` means the graph cannot be
  computed offline, run `grim lock`).
- **`grim why <kind> <name>`** prints the paths from your declarations to that
  artifact. It reads only the lock, so it works offline. Exit 79 means the
  artifact is not in the effective set.
- **A direct declaration wins** over a dependency edge. Two edges that name
  different digests for one slot fail closed with exit 78: declare the
  artifact directly to choose. An edge onto a directly declared artifact is never
  resolved, so a missing tag on that edge does not fail the lock.
- **`remove` and `uninstall`** take dependencies only the target needed. One
  another artifact still needs is kept (`uninstall` reports
  `kept-by-dependency`) at its digest and under its tag, because they resolve
  nothing. The one tag they write is on a tagless pin, when the edit ends the
  lock's proof. A cycle or a graph over the limits exits 65.
- **Needs grim 0.16.0 or later.** An older grim refuses a dependent artifact
  at `grim add`, or locks it silently without its dependencies. A newer grim
  then exits 65 `stale-lock` on that lock: run `grim lock`.

Details: [Dependencies][dependencies].

## Hooks

A **hook** is an artifact that registers with a client's lifecycle so a
handler runs at a defined moment — before a tool call, after one, at session
start, or when the agent's turn stops. It is the only kind that causes code
to execute, and grim treats it accordingly. It is workflow automation, not a
policy control. A gatekeeper handler that fails or times out is withheld
(the launcher exits non-zero), an observer's gives no opinion, and an
unknown workspace or an event no hook matches exits 0. A failure in grim itself
exits non-zero, and the client's own policy for a failing hook decides, because
grim never masks it. For enforcement, use the client's managed settings.

**It is experimental and off by default.** Arming is **two gates**, and the
second can be satisfied three ways — deliberately answered in separate places
rather than folded into one switch:

| Gate | Question | Where | Default |
|---|---|---|---|
| 1 | Is the feature on? | `options.experimental.hooks` in project or global config | `false` |
| 2 | Has this workspace consented? | a machine-local record under `$GRIM_HOME/hooks/consent/` | absent |
| 2 | …or did this one invocation get an explicit pass? | `--trust-workspace` (alias `--trust-hooks`) on `grim install` / `update` / `add` | off |
| 2 | …or did you answer the prompt? | asked once per workspace, on a TTY only | — |

The second gate does not ask *whose code is this* — the lock's digest pin
already answers that. It asks **may this checkout arm hooks on this machine at
all**, which a pin cannot answer. Cloning a repository is not vouching for it,
so a `grimoire.toml` that arrives with a clone declares hooks and arms nothing
until you say otherwise, in that workspace, by hand.

`--trust-workspace` and `--no-trust-workspace` are a pair — the same
question the record answers per workspace, asked for one invocation instead.
Since 0.16.0 `--trust-hooks` and `--no-trust-hooks` are their aliases, and
each answer covers hooks and [settings](#settings) together. Neither flag
writes a record, when both are given the later one wins, and neither turns the
feature on: gate 1 must already be open. `grim add` is the exception on the
record: typing the reference is itself the gesture, so an `add` records consent
with `--trust-workspace` too, ahead of its install, and only
`--no-trust-workspace` keeps it out. The record still holds only what that add
can vouch for. Any other hook or settings package it brings, such as a kept pin
whose tag moved, arms or applies under `--trust-workspace` for that run only and
is not recorded.

**The pair beats the record in both directions**, including a workspace you
deliberately revoked. A flag you typed on this run is the most recent and most
specific answer there is. It is a **flag only: there is no `GRIM_ALLOW_HOOKS`,
no `GRIM_HOOK_CONSENT`, and no environment form of the record's path**, and
that absence is what makes the ordering safe. The environment is repo-carried
— `.envrc`, `.mise.toml` and a devcontainer's `containerEnv` are ordinary
files in a repository — so an environment form would let a cloned repository
consent on your behalf. A file cannot type a flag.

The feature flag is **config-only** — no environment variable overrides it:

```sh
grim config get options.experimental.hooks          # prints nothing, exit 1, unless set true
grim config set options.experimental.hooks true     # opt in, per scope
```

Enabling the feature is not consent to run any particular hook. Consent is one
JSON file per workspace, naming the checkout by absolute path and listing the
hooks it covers as `<binding>@<registry>/<repository>` — no tag and no digest:

```sh
grim hook allow          # consent to what this workspace declares
grim install             # consent is read at convergence — this is what arms
grim hook revoke         # remove the record; the next install disarms
grim hook deny           # the same command, under direnv's name for it
```

Three things write a record and nothing else does: `grim hook allow`,
`grim add` (typing a reference is the declaration gesture, and adding a bundle
records its resolved hook members; an Install in `grim tui` that declares a row
is the same gesture), and a prompt you accepted. `grim install`, `update`,
`lock`, `status`, `context`, `hook list`, a TUI Update or an Install that
declares nothing, and the MCP server never write one — that is the whole point
of the gate.

Because the record lists no tag, a **version bump** of a hook you already
consented to does not re-ask; the bump is visible in the lock's `git diff`.
Declaring a **new** hook, binding one under a new name, or pulling one from a
new repository is drift: arming stops with cause `consent-drifted` and the
message names the new entry. `grim hook allow` again widens the record.

**Global scope is always consented and carries no record** —
`$GRIM_HOME/grimoire.toml` is your own file on your own machine, so
`grim hook allow --global` is a usage error (exit `64`), not a write.

One condition answers to no file at all: a hook whose pinned registry host is
not loopback and is reached over plain HTTP never arms (`insecure-transport`),
because the first resolution that produces the digest pin is itself
attacker-influenceable on the wire. Loopback is exempt, and only
`--trust-workspace` (alias `--trust-hooks`) overrides it.

Setting `options.experimental.hooks` back to `false` and running `grim install`
removes every registration and dispatch row at that scope (state returns to
`gated`, cause `feature-flag-off`) but **keeps** the payload, the launcher
`$GRIM_HOME/hooks/grim-hook` and the workspace consent record. Flipping the
flag back on re-arms with no new `grim hook allow`
([flag off][hooks-flag-off]).

**Team adoption gotcha:** a grim older than 0.15 refuses to load a
`grimoire.lock` (and config) that declares a hook and exits **78**
(`deny_unknown_fields`; it never silently drops the entry). Every machine and
CI job sharing the project must be on 0.15.0 or later before a hook is
declared. Dropping the only hook declaration and re-locking makes the lock
readable to older builds again. The `grim hook` verbs and JSON, the dispatch
table shape, `--trust-workspace` / `--trust-hooks` and the `[hooks]` tables
are Unstable until 1.0;
the OCI wire format for `hook.toml` `schema = 1` is frozen
([stability][stability-unstable]).

### What you get today

Declaring and locking a hook work with the flag off, and that is
intentional — declaring is not arming, so refusing the declaration would
make the flag mean something it does not:

```sh
grim add ghcr.io/acme/hooks/shell-guard:1     # declares under [hooks], pins the lock
```

`grimoire.toml` grows a `[hooks]` table and `grimoire.lock` a `[[hook]]`
section, alongside the other kinds. **`[hooks]` takes registry references
only** — a path value is refused, and there is no dev-install for the kind,
because a working-tree source would put something armable inside a
repository.

Only **Claude**, **Codex**, **Copilot**, and **Qoder** name a hook surface
at all, and only Claude at *project* scope — Codex's and Copilot's
registration files are tracked repository files, and Qoder is global-only
by policy, so grim writes all three at global scope only. Qoder also
declines the `mutator` tier outright, at every scope: its hook schema has
no rewrite field. Copilot's own dialect is probe-selected at build time —
`updatedInput` is the field grim currently emits for a Copilot mutator; a
future probe run may pick a different one, so branch on `grim hook list`'s
report, never on the field name. Codex's and Copilot's own, pre-existing
hook files (`~/.codex/hooks.json`, `hooks/grim.json`) are never overwritten
or deleted by grim: if the bytes on disk are not the ones grim last wrote,
the client reports `surface-user-owned` and nothing about that file
changes. Every other client declines hooks and records nothing. Qoder's
registration is unverified: no Qoder hook has been seen firing from a grim
registration yet. Hooks arm on Linux and macOS; on Windows they arm for Claude
only, and every other client reports `not-armed` with cause
`platform-unsupported`.

### `grim hook list`

The report command for hooks. One row per declared entry, or per
`(entry, client)` where clients differ:

```
$ grim hook list
Hook  Tier  Events  Client  State  Detail

$ grim hook list --format json
{
  "items": []
}
```

An empty table is the normal answer on a project that declares no hooks —
`Hook` is `<artifact>/<id>`, the `State` token comes from the same
vocabulary `grim status` uses, and the JSON `items` objects carry a nested
`arming` array so a consumer asking "which clients withheld this hook" does
not have to re-group rows.

**The launcher is not a command you type.** Clients run
`$GRIM_HOME/hooks/grim-hook <client> <event> <root>`, a small native program
`grim install` writes. It reads the dispatch table beside it, resolves no
scope and needs no `grim` binary. `grim hook list` reports its `launcher`
(`path`, `sha256`, `state`). After upgrading grim, re-run `grim install` in
each workspace that uses hooks.

## Settings

A **settings package** writes typed values — a model, permission allow and
deny rules, a plugin marketplace — into each harness's own settings file. It
installs no file of its own. The kind is **experimental, off by default**, and
Unstable until 1.0 ([Settings reference][settings-ref]; worked example:
[Share settings][share-settings]).

- **Needs grim 0.16.0 or later.** An older grim refuses, with exit 78, a
  `grimoire.toml` or `grimoire.lock` holding a `[settings]` table, and the
  `options.experimental.settings` key. Every machine and CI job sharing the
  project must be on 0.16.0 before a settings package is declared.
- **Two gates, shared with hooks.** `options.experimental.settings` must be
  true (`grim config set options.experimental.settings true`, with `--global`
  for global scope). It is config-only, with no environment variable, and it
  is not consent. At project scope the workspace must also have consented: the
  hook consent record holds `settings:<binding>@<registry>/<repository>` keys
  beside the hook keys. `grim hook allow` (alias `grim consent allow`) records
  both kinds whatever the flags say. `grim add`, and an accepted prompt, record
  a settings package only while the flag is on. An add records the packages it
  brings, under the rule for [hooks](#hooks), ahead of its install. `grim hook revoke`
  removes consent and leaves written values in place. Global scope is always
  consented.
- **Per-run answers.** `--trust-workspace` / `--no-trust-workspace` on `add`,
  `install` and `update` answer for one run, exactly as for [hooks](#hooks).
  Neither flag writes a record. At project scope `grim add` still records its
  gesture unless `--no-trust-workspace` is passed, and `grim add --no-install` records no
  settings package. A package consent withholds reads `skipped` in the
  report and warns `settings package '<name>' not applied: <reason>`, exit 0;
  with no record and no terminal the reason names `grim consent allow` and
  `--trust-workspace`.
- **Declaring.** A `[settings]` table, or `grim add --kind settings <ref>`.
  With the flag off, `install`, `update` and `add` (without `--no-install`)
  warn once per locked package, a named `update` only for its closure's
  packages, and write nothing (exit 0); `grim status` reads `gated`.
- **Where values land.** Claude, Codex, Copilot, Cursor, OpenCode, Qoder,
  Antigravity, Amp, Droid, Gemini, Kilo, Kiro, and Warp on macOS and Linux
  only. At project scope only Claude takes settings, in
  `.claude/settings.local.json`. grim changes only the values it wrote; your
  own entries in the same file stay. At global scope grim refuses a file
  whose config directory is relative (`settings-root-relative`) or lies inside
  the repository the command runs in (`settings-root-in-workspace`): the row
  reads `not-armed`, exit 0, and nothing is written there.
- **Refusals.** A value that changed after grim wrote it, or one grim never
  wrote, is refused: exit 65, `forceable: true`, `reason` `modified` or
  `untracked-destination`, and a `pieces` array naming each. `--force` writes
  them and keeps no backup of the value it replaces. A conflict between two
  packages exits 78, and no `--force` overrides it. A settings file grim
  cannot parse exits 65 and is not forceable. A file whose value changed
  during the run is left untouched and exits 75 (`locked`, retryable); one
  that then no longer parses exits 65.
- **Evidence.** Every run that changes a value reports it in
  `settings_changes`, always present on the `install`, `update`, `add` and
  `uninstall` reports (`update` also carries `pieces`). A version bump of a
  consented package asks nothing again, so read the lock diff and that list.
- **Removing.** `grim uninstall settings <name>` removes the values the
  package wrote, and its `--force` covers settings pieces too. A value grim
  adopted because it already matched stays, even under `--force`, and is
  named in `abandoned_entries`.

Confirm the flags with `grim install --help` and `grim hook allow --help`.

## Further Reading

- [Quickstart][quickstart] — the same loop as a guided walk.
- [Command reference][commands] — per-command pages with current flags.
- [Concepts: the lock][lock] and [bundles][bundles] — semantics in full.
- [Configuration][config-toml] — `grimoire.toml` and `grimoire.lock`
  shape, scopes on disk.

[quickstart]: https://grimoire.rs/quickstart.html
[commands]: https://grimoire.rs/commands.html
[lock]: https://grimoire.rs/concepts.html#the-lock
[bundles]: https://grimoire.rs/concepts.html#bundles
[dependencies]: https://grimoire.rs/concepts.html#dependencies
[config-toml]: https://grimoire.rs/configuration.html
[json-interface]: https://grimoire.rs/json-interface.html
[artifact-states]: https://grimoire.rs/commands.html#artifact-states
[hooks-remove]: https://grimoire.rs/hooks.html#remove-stays-armed
[hooks-flag-off]: https://grimoire.rs/hooks.html#flag-off-retention
[stability-unstable]: https://grimoire.rs/stability.html#unstable
[settings-ref]: https://grimoire.rs/settings.html
[share-settings]: https://grimoire.rs/guides/share-settings.html
