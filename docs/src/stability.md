# Stability and Versioning

Grimoire is pre-1.0: the CLI, formats, and OCI pipeline documented across
this book are real and tested, but the surface has moved between minor
releases while the project found its shape.

1.0 draws a line. A script parsing [`grim status --format
json`][status], a shell `case` on an [exit code][exit-codes], or a
tool reading `grimoire.lock` needs to know what survives an upgrade
unmodified and what does not — otherwise "just run `grim update`" is a
gamble, not a routine operation.

This page names exactly what becomes a semver-guarded contract at 1.0 and
what is explicitly excluded from it. For the behaviour changes you will
actually notice on the first command after an upgrade — and what to do about
each — see [Upgrading](./upgrading.md).

## Frozen at 1.0 {#frozen}

Breaking any guarantee below is a major-version change, not a minor one.

| Area | Guarantee |
|------|-----------|
| CLI surface | Subcommand names, arguments, flags, and the [documented exit codes][exit-codes] |
| `--format json` reports | The report shape for every command that offers one, and the [error document][json-interface] — see [Additive fields](#frozen-additive-fields) and the [JSON interface reference][json-interface] |
| `grimoire.toml` / `grimoire.lock` | The [config and lock schema][configuration] |
| `publish.toml` | The [batch-publish manifest schema][batch-publish], including every spelling a key has ever accepted — see [Additive fields](#frozen-additive-fields) |
| Bundle source manifest | The [bundle member declaration schema][bundles], under the same widening rule as the manifests above |
| [MCP descriptor][mcp-descriptor] (`mcp/<name>.toml`) | The published descriptor schema, including which fields an older grim rejects rather than drops |
| Install state (`state.json`) | Schema V2, governed by the same additive-field policy as JSON reports |
| OCI wire format | [Artifact kinds][artifacts-kinds], the [release/push mechanics][publishing-release], and the [`com.grimoire.*` manifest annotations][annotations] written onto pushed artifacts |
| [Package index][package-index] transport | The locators a published index serves — HTTP `<base>/all.json`, the optional `<base>/stats.json` [ratings sidecar][stats-sidecar], and the git-transport `index/<host>/<ns>/<pkg>/metadata.json` tree. The sidecar's *presence* is not promised (most indexes publish none, and a `404` is a normal, non-error answer); its **path and document shape** are, under its own monotonic `schema_version` and the consumer rule stated with it |
| [MCP server][mcp-server] tool surface | Tool names (`grim_search`, `grim_status`, `grim_fetch`, `grim_describe`, `grim_render`) and their argument names — the payloads are covered by the reports row |
| Published schema URLs | `https://grimoire.rs/schemas/{grimoire-config,grim-publish,grimoire-lock}.schema.json` keep resolving — [`grim init`][init] writes the first into every generated `grimoire.toml` |
| Environment variables | The documented [`GRIM_*` set and honored vendor overrides][env-vars] |

### Additive fields {#frozen-additive-fields}

Two of the rows above — `--format json` reports and the install-state
schema — share one rule on the *output* side: a minor release may add a new
optional field, but never changes an existing field's type or meaning, and
never removes one. (Manifest *inputs* obey the mirror-image rule, at the end
of this section.) The
matching obligation sits on the reader: a consumer of either format must
ignore fields it does not recognize rather than error on them. That pairing
is what makes "add a field in a minor" safe for every consumer, including
ones written before the field existed.

Manifest **annotations** follow the same additive rule from the other
direction: a minor release may start emitting a key it did not emit before, or
add a new `com.grimoire.*` key, but never changes what an existing key means
and never stops emitting one a reader could already rely on. Unlike a report
field, an annotation that does not apply is simply **absent** — an OCI
annotation map has no null. A reader must treat an absent key as "this
artifact does not say", never as "this grim is too old".
[Build provenance](./publishing.md#git-provenance) is an instance: `revision`
and `created` were once emitted only under `--git` and are now emitted by
default, which adds values where consumers previously saw nothing.

Optional report fields are **always present**: a field that does not apply
serializes as an explicit `null`, never as an absent key. A consumer can
therefore distinguish "not applicable" (`null`) from "talking to an older
grim that predates the field" (key missing) without version sniffing.

[`grim status`][status]'s `clients_missing`/`clients_extra` (client-set
drift) and `--check`-gated `deprecated`/`replaced_by`/`update_available`
(plus the top-level `checked`), and [`grim update`][update]'s
`reaped_clients`/`kept_modified_clients`, are instances of this
pattern: each shipped as an additive field on an already-frozen report
shape, each always-present (`[]`/`null` when inapplicable, never an
absent key), so a consumer written against the pre-#43 `status`/`update`
shape keeps parsing unchanged. Both drift and reap are measured only
against an *explicitly set* `[options].clients`; when it is unset
(autodetect), `clients_missing`/`clients_extra` stay `[]` and
`reaped_clients` stays `[]` on every row — neither ever keys off live
client detection, which can drift independently of the user's config.
`clients_missing` narrowed once more, additively: a configured client
whose vendor cannot host that artifact kind at that scope drops out of
it, because the install was never going to record an output there.
Strictly fewer names in an already-present array is not a shape change.

[`grim publish`][publishing-report]'s `announce.fork` (`{repo, created}` or
`null`) is the same shape, added when `--announce` gained automatic fork
detection. It extends the already-frozen `announce` object the same
always-present-null way: `null` when the branch pushed straight to the
index repository (no fork involved, or the `[announce] fork` policy
resolved to `never`), populated with the fork's full name and whether it
was newly created once forking activated.

The newest instance is [`grim search`][search]'s `rating`
(`{up, url}` or `null`), added with [artifact ratings][ratings]. It is the
worked example of what always-present-null buys a consumer: `null` means
*this artifact is unrated* — no sidecar published, no entry for the ref, or
no votes yet — while an **absent** `rating` key means *this grim predates
the field*. A client can tell those apart without sniffing the version,
which is the whole point of the rule. A consumer written against the
pre-rating `search` shape keeps parsing unchanged, because the field was
appended and nothing else moved. The `rating` object emits `up` and `url`
only; the forge's opaque vote target is deliberately kept out of the frozen
document, so no forge's node-id format is frozen with it.

Manifest *inputs* are covered by the mirror image of that rule. A minor
release may widen what a key accepts — a new optional key, a new accepted
value, a new spelling — but never narrows it: a `publish.toml` or
`grimoire.toml` that parses today parses on every later 1.x, and a value
that was once valid stays valid even after a better spelling replaces it
in the documentation.

`[announce] fork` is the worked example. It began as a boolean and grew
into the `never | auto | always` policy described under [Announcing
Packages](./package-index.md#announcing); the boolean spelling stays
accepted permanently (`true` = `auto`, `false` = `never`) rather than
being deprecated out. The obligation this places on grim is deliberate
and one-directional: old manifests keep working, so the cost of a widened
key is paid once by the implementation instead of repeatedly by everyone
who wrote a manifest against an earlier release.

## Unstable — may change in any minor {#unstable}

Three things are deliberately excluded from the guarantee above, because
freezing them would block improving Grimoire without a major version bump —
the exclusions are what keep 1.x able to move at all:

- **Vendor render layout.** The exact files and paths grim writes under
  any client's own configuration root, and where an MCP entry lands
  inside a client's own config file, are not a contract. Neither is the
  shared `$HOME/.agents/skills` pool: *which* clients render into it
  moves as clients adopt or retire the convention, and
  [`[options.vendors.<name>].shared_skills`][options-vendors] lets a
  pool-reading client opt in — moving that client's skills out of its own
  directory and into the pool, or back. The per-client roots are
  enumerated by the [vendor projection layer][vendor-metadata] and in the
  [client compatibility matrix][clients]; those enumerations are
  documentation, not a promise, and are free to move between minors as
  clients change their own conventions.

  A root that a vendor environment variable relocates moves the layout
  with it, and a release that *starts* honoring one moves it for everyone
  who already set that variable. The most recent instance: `$KIRO_HOME`
  and `$GEMINI_CLI_HOME` became honored, and Zed's macOS root stopped
  consulting `$XDG_CONFIG_HOME` (upstream never read it there). An
  artifact installed under the old resolution then sits at a root grim no
  longer resolves. The next `install`, `update`, or `uninstall` reaps
  that stranded copy under the [migration promise](#promise) below; until
  one of them runs, [`grim status`][status] reports the artifact's `state`
  as `missing` even though the file is still on disk. A stranded copy
  you edited yourself is preserved rather than deleted, and grim warns
  naming both paths; on the `uninstall` path it is additionally listed in
  `retained` — or, for an MCP entry spliced into a config file grim does
  not own, in `abandoned_entries`.
- **Everything else that is not exit codes or JSON.** State-file contents
  beyond the schema guarantee, TUI appearance and keybindings, and
  human-readable log or error text carry no compatibility promise — only
  exit codes and structured JSON output are contracts.
- **NDJSON progress events** (`--progress json`) stay **experimental
  through 1.0**, deliberately. The event shapes evolve additively (new
  fields may appear, existing ones keep their meaning), but the surface
  itself is not frozen by the 1.0 release: it shipped too recently for any
  external consumer to have built against it, and freezing a contract
  nobody has exercised buys a guarantee no one asked for at the cost of
  never being able to fix it. It freezes in a later minor, once a real
  consumer has shaped it. Anything you script against it today may move.

### The supported discovery channel {#unstable-discovery}

Because render layout can move, scripting "where did grim put this skill?"
against a hardcoded path is unsupported and will eventually break. Use
[`grim status --format json`][status] instead: every entry carries an
`outputs` array of `{client, path}` pairs — the per-client materialized
locations read back from install state, empty for a declared-but-not-yet-
installed artifact. `outputs` is itself covered by the [additive-field
policy](#frozen-additive-fields) above, so code that reads it survives an
upgrade even as the paths inside it change.

The same entry carries `outputs_pending` — the outputs an install *would*
write that the record does not yet cover, in the same `{client, path}`
shape. It is the supported way to detect **materialization drift**, which
has two causes: a client installed after the fact, or a layout move not yet
migrated. An output deleted out from under grim is not one of them — that
entry reads `state: missing` instead. `outputs_pending` is derived from the
same seam the installer uses to decide whether a pass is a no-op.
Remediation is `grim install`; `outputs_pending` never moves `state` and
never affects the exit code.

## The compatibility promise {#promise}

Vendor layout moving is not, by itself, a compatibility break — provided
grim upholds this: artifacts remain discoverable by the target client;
status, update, and uninstall keep working across upgrades; exact vendor
paths may change in a minor release with automatic migration.

That migration is mechanical: the first install or update after an
upgrade that moved a layout re-materializes the artifact at its new
path, re-anchors the install record, and reaps the unmodified old
output. `uninstall` collects the same old output, so removing an
artifact before ever re-installing it does not strand a copy either. A locally modified old file is never deleted — the same
preservation rule the [untracked-destination guard](#unstable) applies.
This layout-migration reaper has no `--force` override: it always
preserves a modified file. (The distinct dropped-client reaper on
[`grim update`](./commands.md#update) — which removes the outputs of a
client you dropped from `[options].clients` — applies the same
preserve-when-modified default, but there `--force` does delete a
locally-modified dropped-client output. That reaper only fires when
`[options].clients` is explicitly set; left unset — autodetect — `update`
never reaps, since the desired set would otherwise track live client
detection rather than the user's config.)

The reasoning for keeping render layout out of the 1.0 contract while still
holding that promise is recorded in the project's ADR on render-layout
stability (`.agents/adr/adr_render_layout_stability.md`).

### The update gate, restored in 0.13.0 {#promise-update-integrity}

`grim update` over a **locally modified** artifact exits `65` instead of
overwriting it, and `--force` overwrites. That is a behaviour change on a
shipped exit-0 path, so it is called out here for anyone upgrading — but it
is a restoration, not a break. The promise it now keeps is the one the
reference had already made.

[`grim update`][update] is documented as sharing `--force` with
[`grim install`][install], where `--force` "overwrites a locally modified
artifact instead of refusing it". The update reference invokes that gate by
name: a locally modified orphan survives prune because that "mirrors the
install integrity gate, where a locally modified artifact is refused rather
than overwritten without `--force`."

The code did the opposite. `update` passed a hard-coded force into the
installer, so it destroyed hand-edited work silently — no warning, no report
field — while `grim install` refused the identical bytes with `65`, and
while update's own prune and client-reap passes already gated the same class
of destruction behind `--force`. One command gave two opposite answers about
one file, and only the silent one lost data. Code depending on the old
behaviour was depending on a defect, not on a contract.

## Known limitations {#limitations}

The behaviors below fall outside every guarantee above — not because they
are likely to change, but because they are hard constraints of the
current design.

### A shared `GRIM_HOME` has a single writer {#limitations-shared-home}

Global install state lives in one file, `$GRIM_HOME/state/global.json`.
When two machines or containers share that directory — a mounted volume
across devcontainers, a shared home on a build host — they read and write
the same file, and concurrent `grim install --global` runs are
last-writer-wins on the record set. Anchored paths make the file portable
between machines, but portability is not coordination.

Writes are atomic, so the file is never left half-written; what is lost is
one run's *records*, not the file's integrity, and a subsequent `grim
install` re-materializes anything dropped. The supported arrangement for
1.0 is one writer at a time. Project scope is unaffected: each workspace
owns `<workspace>/.grimoire/state.json`, so two projects never collide
even on a shared volume.

### The shared-pool refcount trusts install state {#limitations-pool-refcount}

Several clients read one physical `$HOME/.agents/skills/<name>` directory
(see [Shared skills pool visibility][gap-shared-pool]), so grim must not
delete it while another client still wants it. The guard that decides
this — reached only from the dropped-client reaper on [`grim
update`][update] — is a **refcount over install state alone**: it walks
the *same artifact record's* `outputs` array and keeps the directory if
any output that is neither being removed nor itself dropped in this pass
resolves to the same target **and the same support directory**. There is
no filesystem fallback and no cross-record scan.

That is exact whenever the record matches reality, which is the case grim
maintains. It is not self-correcting when the record does not: a
hand-edited or truncated `.grimoire/state.json` / `global.json` that no
longer lists a pool client's output makes that client invisible to the
guard, and removing the last client the record *does* list deletes a
directory the missing client still scans. The files are recoverable —
re-running [`grim install`][install] re-materializes them — but the
deletion is not announced as shared, because from the record's point of
view nothing was sharing it.

Editing install state by hand is outside the supported envelope for the
same reason [local path sources](#limitations-path-source-trust) are: the
state file is grim's own bookkeeping, and grim reads it back as fact.

### Forward compatibility {#limitations-forward-compat}

Every lock and install-state field parses with `deny_unknown_fields`: a
`grim` binary that does not recognise a field refuses to load the file
rather than silently drop it. That protects a downgrade from misreading
data it cannot faithfully represent, but it cuts both ways — a lock or
state entry using [local path sources](./concepts.md#references-tags-and-digests)
(a path-declared skill, rule, or agent, or a [local
bundle](./concepts.md#bundles)) is unreadable by a `grim` build that
predates the feature. It exits 78 (`EX_CONFIG`), the same code any other
config or lock parse failure uses.

This hard-reject is a deliberate departure from the ecosystem norm:
[Cargo][cargo-manifest] warns rather than errors on an unrecognized
`Cargo.toml` key and reserves `package.metadata` as a designated
pass-through table, [npm][npm-package-json] generally tolerates unknown
`package.json` fields, and [Helm][helm-chart] silently drops an
unrecognized `Chart.yaml` key, gating compatibility on `apiVersion`
instead — none of the three hard-reject a manifest for a field they don't
recognize. Grimoire trades that forward-tolerance for an explicit signal:
a lock or state file is read back by every subsequent command, and a
silently-dropped field there would let a newer file downgrade into a
report that looks complete but is not.

This only triggers when the feature is actually in use: a registry-only
lock or state file stays byte-identical across the version boundary, so a
project that never declares a path source is unaffected either way.

The [MCP descriptor](./mcp-servers.md) layer holds the same line: a
descriptor published with fields an older grim predates (the refinement
fields, the `ws` transport, the `oauth` block) fails to parse there —
a data error (65) at install or fetch, never a silent drop. A descriptor
that does not author the new fields serializes byte-identically across
the boundary.

`grimoire.toml` holds the same line too: both the top-level shape and each
`[[registries]]` entry (`RegistryConfig`) parse with `deny_unknown_fields`,
so an older `grim` reading a newer config hard-rejects it (exit 78) rather
than silently dropping a field it does not recognize. This branch is the
first minor release to add a key under `RegistryConfig` — [`include` and
`exclude`](./configuration.md#browse-filters) — so it is the first case
where that matters in practice: a project that adopts a browse filter is
unreadable by any `grim` build that predates it, the same trade the lock
and install-state already make.

Across all of these the *stance* is unchanged — a hard reject, never a
silent drop — but the message now says so: whenever a rendered error names
a key or value this build does not recognize, grim appends a guidance line
naming its own version and the two possible causes, a typo or a file
written by a newer grim. Under `--format json` the same text rides the
error document's optional [`hint` field][json-hint].

The **catalog cache** (`$GRIM_HOME/catalog/<hash>.json`) parses the same
strict way, and it is the one place where that strictness used to be worse
than a rebuild. Its `CatalogEntry` carries `deny_unknown_fields`, so a
cache written by a newer grim is refused wholesale by an older one — which
is *fine*, because a cache is not a contract and rebuilding it costs one
network refresh. That is now true: from this release on, a cache the loader
refuses reads as **cold** while online, so the very next browse overwrites
it (offline the refusal still surfaces, because there is nothing to rebuild
from and reporting the registry as empty would be a lie).

It was **not** true before this release. In 0.13.0 and earlier the parse
error was raised above the rebuild decision, so a refused cache degraded
that registry to an empty browse *without* overwriting the file — on every
subsequent run, `--refresh` included, until someone deleted it by hand.
This release cannot reach a binary that already shipped, so a user who
browses a [rating-publishing index][ratings] on 0.14 and then downgrades to
0.13 has to delete `$GRIM_HOME/catalog/` once; see
[Upgrading][upgrading-catalog-downgrade]. An entry with no rating still
serializes byte-identically to what 0.13 wrote, so only a user who actually
browsed a rated index is exposed at all.

A lock's **size** cuts the same way, for one release boundary. Builds up
to and including 0.12.0 read `grimoire.toml`, `grimoire.lock`, and
`publish.toml` under a 64 KiB cap; later builds read them under 8 MiB. A
lock past 64 KiB — reachable around 140 artifacts with deep registry paths,
sooner once [bundle](./concepts.md#bundles) provenance is counted — is
therefore unreadable by a 0.12.0-or-older binary, which exits 78 like any
other lock failure. Nothing regressed at that boundary: those builds could
not read such a lock even when they had written it themselves, which is why
the cap moved. A lock under 64 KiB is unaffected in both directions.

### Local path sources are trusted like a build script {#limitations-path-source-trust}

A [local path source][path-sources] — a `grimoire.toml` skill, rule,
agent, or bundle declared as `./…`, `../…`, or an absolute path, and the
equivalent entries a [dev-install][install-dev] writes into
`.grimoire/state.json` — names a file on the invoking user's own
filesystem, read with that user's own permissions. There is no registry
boundary, no signature, and no sandbox around that read: a path source is
trusted the same way a `Makefile` or a `package.json` script is trusted.
`grim lock` and `grim install` can read any file the invoking user can
read at that path, including one outside the project's own directory tree.

This is deliberate — path sources exist so local development and
monorepo cross-references work without a registry round-trip — but it
means a cloned repository's `grimoire.toml` (or a hand-edited
`.grimoire/state.json`) is exactly as trustworthy as its build scripts.
Review a project's path-sourced declarations before running `grim` inside
an untrusted checkout, the same way you would review its `Makefile` or CI
config before running it locally. grim warns to stderr — a SECURITY-framed
message — on **every command that resolves the project scope**
([`status`][status], [`install`][install], `add`, `update`, `remove`,
`uninstall`, [`context`][context], `lock`, all sharing one resolution seam),
not `grim lock` alone, whenever a declared source is absolute or a relative
source resolves outside the workspace root; the warning is advisory only,
and the command's exit code stays `0`.

That out-of-workspace check is **lexical**: it walks the path's own `../`
and `.` components against the workspace root and never touches the
filesystem, so it does not catch a symlink-mediated escape. A relative
source that looks in-tree but whose root — or an ancestor directory on the
way to it — is a symlink pointing outside the workspace is read and packed
with no warning at all. This follows from the same "trusted like a build
script" model above: grim does not resolve symlinks to police the trust
boundary any more than [`make`][gnu-make] or [`npm install`][npm-install] do.

### Offline re-materialization needs a manifest {#limitations-offline-remat}

Grimoire caches a fetched artifact's content layer — content-addressed, so
identical bytes are never re-downloaded — but not its manifest. An offline
[`grim install`][install] whose rendered output is still on disk is
network-free: the integrity gate compares the on-disk content hash against
the lock and needs nothing from the registry.

Deleting that output and asking `--offline` to re-materialize it is a
different story. Even a pinned manifest digest has to be *fetched* to learn
which layer blob to pull, and that fetch always needs the network — grim
keeps no local manifest cache to serve it from. This is a general
constraint of the content-cache design, not specific to path sources: it
applies to every registry-sourced kind (skill, rule, agent, MCP server, or
bundle member) whose materialized output has gone missing while offline.
[Local path sources](./concepts.md#references-tags-and-digests) are
unaffected — they read straight from disk and never touch a manifest.

<!-- internal -->
[json-interface]: ./json-interface.md
[status]: ./commands.md#status
[update]: ./commands.md#update
[publishing-report]: ./publishing.md#batch-publish-report
[install]: ./commands.md#install
[install-dev]: ./commands.md#install-dev
[context]: ./commands.md#context
[exit-codes]: ./json-interface.md#error-document
[annotations]: ./artifacts.md#annotations
[bundles]: ./artifacts.md#bundles
[mcp-descriptor]: ./mcp-servers.md#format
[mcp-server]: ./commands.md#mcp
[package-index]: ./package-index.md#spec
[stats-sidecar]: ./package-index.md#spec-stats
[ratings]: ./ratings.md
[search]: ./commands.md#search
[upgrading-catalog-downgrade]: ./upgrading.md#catalog-cache-downgrade
[init]: ./commands.md#init
[configuration]: ./configuration.md
[options-vendors]: ./configuration.md#options-vendors
[clients]: ./clients.md#matrix
[gap-shared-pool]: ./clients.md#gap-shared-pool
[env-vars]: ./configuration.md#environment-variables
[artifacts-kinds]: ./artifacts.md#kinds
[batch-publish]: ./publishing.md#batch-publish
[publishing-release]: ./publishing.md#release
[vendor-metadata]: ./vendor-metadata.md
[path-sources]: ./concepts.md#references-tags-and-digests
[json-hint]: ./json-interface.md#error-hint

<!-- external -->
[gnu-make]: https://www.gnu.org/software/make/manual/make.html
[cargo-manifest]: https://doc.rust-lang.org/cargo/reference/manifest.html
[npm-package-json]: https://docs.npmjs.com/cli/v10/configuring-npm/package-json
[helm-chart]: https://helm.sh/docs/topics/charts/#the-chartyaml-file
[npm-install]: https://docs.npmjs.com/cli/commands/npm-install
