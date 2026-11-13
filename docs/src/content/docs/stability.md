---
title: "Stability and Versioning"
description: "Grimoire is pre-1.0 — this page documents which CLI, format, and pipeline contracts are frozen versus still evolving."
---
<!-- doc_type: explanation -->

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
Three exceptions sit inside these rows: the experimental [hook](./hooks.md)
surface, the experimental [settings][settings] surface, and the
[telemetry switches][telemetry-enable], [`GRIM_CONFIG`][telemetry-config] and
`GRIM_TELEMETRY_OTLP_HEADERS` in the environment variables row.
[Unstable](#unstable) lists all three by name.

| Area | Guarantee |
|------|-----------|
| CLI surface | Subcommand names, arguments, flags, and the [documented exit codes][exit-codes] |
| `--format json` reports | The report shape for every command that offers one, and the [error document][json-interface] — see [Additive fields](#frozen-additive-fields) and the [JSON interface reference][json-interface] |
| `grimoire.toml` / `grimoire.lock` | The [config and lock schema][configuration], and the rule that [`grim lock`][lock] keeps existing pins. See [Additive fields](#frozen-additive-fields) |
| `marketplace.toml` / `marketplace.lock` | The [manifest and lock wire shape][marketplace-toml], including the `plugin` field, the `[[plugin]]` table and the [`[marketplace]` table][marketplace-table] and the [plugin metadata keys][plugin-metadata] — under the same additive and pin rules as `grimoire.lock`, and never sharing a file with it (a `plugin`-bearing entry in `grimoire.lock`, or vice versa, is a rejected scope mismatch, not a migration path) |
| `publish.toml` | The [batch-publish manifest schema][batch-publish], including every spelling a key has ever accepted — see [Additive fields](#frozen-additive-fields) |
| [`grim export plugin`][export-plugin] | The `--format json` [export report shape][json-shapes-items], and the `<base>+<12-hex>` [version grammar][export-plugin-output] a rendered `plugin.json`'s `version` follows. Only the grammar is frozen, not what feeds the hash |
| [`grim export marketplace`][export-marketplace] | The command's flags and [exit codes][export-marketplace-exit]; the `{items, files}` [report shape][export-marketplace-json] and its `action` literals; the [`[marketplace]` keys and grammars][marketplace-table]; the client-to-file table and the `./<client>/<plugin>` [tree paths][export-marketplace-layout]; the default client set (`claude`, `copilot`, `codex`, `qoder`); the [ownership convention][export-marketplace-ownership]; the document guarantees (each plugin entry's `source` is `./`-relative inside the repository, and its `name`, `version`, `description` and any profile field the harness defines equal the tree's manifest); and the version grammar above |
| Bundle source manifest | The [bundle member declaration schema][bundles], under the same widening rule as the manifests above |
| [MCP descriptor][mcp-descriptor] (`mcp/<name>.toml`) | The published descriptor schema, including which fields an older grim rejects rather than drops |
| Install state (`state.json`) | Schema V2, governed by the same additive-field policy as JSON reports |
| OCI wire format | [Artifact kinds][artifacts-kinds], the [release/push mechanics][publishing-release], the [`com.grimoire.*` manifest annotations][annotations] written onto pushed artifacts, the [layer media types][artifacts-layers] grim reads (the grimoire and OCI tar and `tar+gzip` types, and the docker rootfs gzip type) and writes (the uncompressed tar type and the `tar+gzip` type), the refusal of any `+zstd` type, the `com.grimoire.uncompressed.digest` layer annotation, the [two-layer layout](./artifacts.md#requires-layer) of an artifact that declares dependencies (a grim convention: one content layer and one `application/vnd.grimoire.bundle.v1+json` requires layer, told apart by media type and written content first), and the `com.grimoire.requires-features` annotation grim reads to refuse a feature it does not know |
| [Package index][package-index] transport | The locators a published index serves. The client contract is [the root][index-root] at `<base>/c/root.json` and [the gzip blobs][index-blobs] it names at `<base>/c/f/sha256/<hex>.json.gz`, whose digest and size cover the inflated JSON. Both are permanent once an index emits them, and under `schema: 1` they grow only by additive fields. Their *presence* is not promised: a `404` is a normal answer that sends grim to `all.json`. Everything else under `c/` is private to the index pipeline. Also served: HTTP `<base>/all.json` and the optional `<base>/stats.json` [ratings sidecar][stats-sidecar], both while the index's `legacy` setting is true (the default), and the git-transport `index/<host>/<ns>/<pkg>/metadata.json` tree. The sidecar's *presence* is not promised either (most indexes publish none, and a `404` is a normal, non-error answer). Its **path and document shape** are promised, under its own monotonic `schema_version` and the consumer rule stated with it |
| [MCP server][mcp-server] tool surface | Tool names (`grim_search`, `grim_status`, `grim_fetch`, `grim_describe`, `grim_render`) and their argument names — the payloads are covered by the reports row |
| Published schema URLs | `https://grimoire.rs/schemas/{grimoire-config,grim-publish,grimoire-lock}.schema.json` keep resolving — [`grim init`][init] writes the first into every generated `grimoire.toml` |
| Environment variables | The documented [`GRIM_*` set and honored vendor overrides][env-vars], except the [telemetry switches][telemetry-enable], `GRIM_CONFIG` and `GRIM_TELEMETRY_OTLP_HEADERS`, Unstable until 1.0 |
| [`.grimignore`][grimignore] | gitignore syntax, defaults-extend-not-replace semantics, and the three never-ignored names (`SKILL.md`, a rule's index, `.grimignore` itself) — see [Additive fields](#frozen-additive-fields) for what growing the built-in default list does and does not cover. One kind-scoped exception: for a [hook](./hooks.md) root only, `hook.toml` is never ignored even when a `.grimignore` entry names it — its `hook.toml` is still packed and hashed |

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
drift) and `--check`-gated
`deprecated`/`replaced_by`/`update_available`/`moved_nested_bundles` (plus the
top-level `checked`), and [`grim update`][update]'s
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

[`grim search`][search]'s `rating`
(`{up, url}` or `null`), added with [artifact ratings][ratings], is the
worked example of what always-present-null buys a consumer: `null` means
*this artifact is unrated* — no sidecar published, no entry for the ref, or
no votes yet — while an **absent** `rating` key means *this grim predates
the field*. A client can tell those apart without sniffing the version,
which is the whole point of the rule. A consumer written against the
pre-rating `search` shape keeps parsing unchanged, because the field was
appended and nothing else moved. The `rating` object emits `up` and `url`
only; the forge's opaque vote target is deliberately kept out of the frozen
document, so no forge's node-id format is frozen with it.

A later instance is the sibling half of the same rule. [`grim
search`][search] grew a `sources` array beside `items`, naming every
browsed source and whether its catalog loaded — the reason the
[`items` envelope][json-items] exists at all, since a bare top-level array
could never have grown it. It is always present (`[]` when nothing was
browsed), so a consumer reads it unconditionally, and a pre-existing client
that ignores unknown keys is unaffected. See [the `sources`
array][json-sources].

A third instance is the same shape again. `grim search` gained `total`
and `truncated` beside `items` and `sources`, so a consumer can tell a
`--offset` or `--limit` cut or an incomplete `oci` listing from a complete answer. Both are
always present, so a pre-existing client that ignores unknown keys is
unaffected. See [the `total` and
`truncated` fields][json-total].

[Declared dependencies](./concepts.md#dependencies) arrived as a set of the
same additions. [`grim status`][status] rows gained `direct` (a boolean) and
`required_by` (a list of parents, or `null` when the dependency graph cannot be
computed offline), and its `source` string gained the values
`dependency: <kind>/<name>, …` and `dependency`. [`grim add`][add]'s report
gained `pulled_in`, and [`grim describe`][describe] (and the MCP `grim_describe`
tool) gained `requires`. All three are always present, `[]` when they apply and
are empty. [`grim uninstall`][uninstall]'s `status` gained the literal
`kept-by-dependency`, and the error document gained the reason
`unsupported-artifact-feature`. A consumer written against the earlier shapes
keeps parsing, because nothing existing moved. `grimoire.lock` gained the
optional `[[<kind>.requires]]` tables and a `transitive` flag on a `[[bundle]]`
entry. Both are omitted when unused, so a lock without dependencies is
identical to one written before apart from the tag in `pinned`, and
`lock_version` stays `1`. The new
[`grim why`][why] command and its report are covered by the CLI and reports
rows above from the release that introduced them.

Pin stability kept the lock format and changed what a lock write does.
[`grim lock`][lock] keeps every pin whose request is unchanged, and so do
[`grim add`][add], [TUI][tui] Install and the export stage.
[`grim update`][update] is the command that moves a pin whose request held. The
[few cases][lock-pins] where another command resolves one are listed with
`grim lock`. To compare a request with a pin, the lock records the reference that resolved it, so `pinned` reads
`registry/repo:tag@sha256:…` on every artifact entry of `grimoire.lock` and
`marketplace.lock`.

No field was added, `lock_version` stays `1`, and the published schema always
allowed that tag. A reference requested by digest alone stays tagless, and
`[[bundle]]` keeps its sibling `tag`. `marketplace.lock` has no bundle row, so a
bundle a plugin reaches keeps its pin in the rows below it, where `bundle_tag` and
`bundles[].tag` read `<tag>@<digest>`. Both stay strings in the same schema, and no
`--format json` field carries that form.

The promise that holds across that change is **reading on both sides**. A lock
is identical to one written before apart from the tag in `pinned`, and a real
grim 0.15.1 binary reads it, as
[Forward compatibility](#limitations-forward-compat) records. The check is the
[Verify Deep][verify-deep] workflow's old grim interop step, which runs
`task test:old-grim` with `GRIM_OLD_BIN` naming the released binary. That holds
for a project that declares no settings package, since 0.15.1 refuses the
settings kind.

One write differs further. A `grim remove` that leaves the lock stale can keep a row that 0.15.1's
`remove` drops. When it also changes the lock's rows, it writes a hash no
declaration produces, in the same `sha256:<hex>` form, and otherwise keeps the
hash. 0.15.1 reads that lock as stale. The first `grim lock` after
an upgrade adds the tags with no tag resolve, and its report reads `unchanged`.

Beside a dependent whose entry records no edges, `grim remove` drops what
0.15.1's drops. One online `grim lock` before the edit records those edges and
keeps the pins, and 0.15.1 refuses the lock it writes. The first `grim install`
on such a lock already exits `65` and names that `grim lock`.

Beside such a dependent, the manifest a bundle member's moved tag names can hold dependencies that conflict. They can also form a cycle, back at the member's pinned digest or at another one. `grim add` of the member's own reference then keeps its pin, as
`grim lock` does, and installs it at exit `0`. Online it also keeps the pin where
a tag below that manifest is gone, where c5e3824 exited `79`. Offline that add
still exits `69`, as before. An unreachable registry, a
refused credential, a timeout or a graph cap below the manifest keeps its exit.
0.15.1 moves the pin there. Its `grim add --no-install` exits `0`, and its
`grim add` exits `65` with `expected a single-layer artifact`, since that manifest
carries a [requires layer](#limitations-forward-compat). Offline,
`grim add --no-install` there exits `0` on an empty store and on one an online
add filled, where 0.15.1 exited `69`.

`marketplace.lock` keeps the same promise. grim 0.15.1 holds `bundle_tag` as an
opaque string, so it reads the file and its member update writes the value back. A
row it resolves again loses the digest, and a whole update or a stale export does
that to every row. Where no row records a digest, the next write by this grim that
covers the whole plugin follows the bundle's tag and records one. A member update
keeps every row outside its selection as 0.15.1 left it.

The reports grew additively. `grim lock` rows gained `disposition` (`added`,
`carried`, `moved` or `dropped`), and `action` gained `dropped` on a new row for
a pin no longer in the lock. `grim add` gained `moved` and `dropped`, both always
present and `[]` when empty.

`grim status` items gained `moved_nested_bundles`, always present and `null` unless
`--check` ran, and `update_available` keeps its per-row meaning. A dependency
row answers `true` or `false` too, once the local store confirms the edges onto
it, and `null` before. Unreleased builds of the dependency feature left it
`null`.

A failed re-resolve always read `null`. So does a row past the 30 s deadline,
the run's cap of 2,048 re-resolves or its 300 s deadline. A tag outside the OCI
grammar sends no request and reads `null` too. So does a key the lock records
under references that resolve to different digests. No lock grim 0.15.1 writes
holds such a row, since it records no dependency edges and refuses bundles that
list an undeclared member at different references.

`grim update`'s `action` gained `restored`, for an artifact this run wrote at an
unchanged pin. No report's `pinned` value changed, because the tag stays in the
lock file.

Eleven changes are not additive. Each fixes a shipped path, and each is named here
for anyone upgrading:

- `grim lock`, `grim add` and `grim update` exit `78` on a lock they cannot
  parse, where they used to exit `0`. The old read counted a failure as no lock,
  so the command rewrote the file and moved every pin. A lock they cannot open
  exits `77` or `74`.
- `grim lock` no longer re-resolves an existing pin, so a hand-edited digest
  under the same repository and tag survives it. For a checkout you do not
  trust, run `grim update` or delete the lock.
  [`grim status --check`][status-check] reports a pin that differs from its tag.
- A lock write exits `79` when neither the registry nor the local store holds
  a carried pin's manifest, since only `grim update` may move it. That covers a
  bundle digest `marketplace.lock` records, where 0.15.1 exited `0`.
  `grim update --marketplace` on the plugin moves it, and a lock with no
  recorded digest cannot reach this. It also covers a tagless pin on a lock with
  no member list, where 0.15.1 exited `0` and moved the pin. `grim update`
  recovers it.
- `grim update <names>` lists the closure of its names, what it pruned, and
  every other artifact whose lock entry changed. An unnamed artifact whose pin
  did not move has no row, where it used to read `unchanged`. A named run never
  reaps, so `reaped_clients` and `kept_modified_clients` stay `[]` on every row.
  Fewer rows in `items` is not a shape change, and no `action` literal changed
  meaning. See [the `update` rows][json-update-rows].
- `grim update <member>` exits `79` for a member that exists only under its
  bundle's moved tag, where it exited `0` and moved the bundle's pin. A name
  selects itself alone, so the bundle keeps its pin. Run `grim update <bundle>`
  instead.
- `grim install` exits `65` after `grim remove` or `grim uninstall` on a stale
  lock that no longer answers its declarations, where it exited `0`. The old
  `remove` restamped that lock, and `install` then wrote pins nobody had resolved
  for the edited entry. A stale lock that answers every declaration still
  restamps. A pin with no tag cannot show what it answers, so a lock 0.15.1
  wrote stays stale after such an edit.
- `grim uninstall` keeps the files of an artifact a bundle with no member list
  provides, where 0.15.1 deleted them. The local store shows it, or the bundle's
  stamp on the artifact's entry does. Such a lock was written by grim 0.3 or
  earlier, or had its `[[bundle]]` rows cut by hand. One with no direct
  declaration reports `kept-by-bundle`, not `uninstalled`, and the exit stays `0`.
- `grim update <names> --force` overwrites and restores only inside the closure
  of its names. Up to 0.15.1 it also overwrote an unnamed artifact you edited and
  restored one you deleted, and listed them. The exit stays `0`, so a repair
  script sees no signal. Run `grim install --force` to repair the whole lock.
- `grim install` reads `unchanged` where it read `updated` for an output this pass
  does not write. The output belongs to a client outside the target, one this grim
  does not know, or one that no longer hosts the kind. An MCP entry on a surface
  the vendor cannot write behaves the same. So does an MCP entry a target
  client's config already holds exactly, which grim adopts with no write. An
  edit to such an output refuses only when a target client writes that
  destination.
- `grim update --marketplace <manifest> <plugin>:<member>` exits `79` for a member
  only a moved bundle tag would bring, on a plugin that records that bundle's digest.
  0.15.1 exited `0` and added the row, and c5e3824 exited `78` where the moved tag
  brings a conflict. Over a part this grim wrote with dependencies, 0.15.1 exits
  `78`, since it cannot parse the `requires` field. Select `<plugin>`, which moves
  the bundle and brings the member, as `grim update <bundle>` does in a project.
- A `grim update --marketplace` pick of a member or bundle exits `78` and writes
  nothing in one case. The pick would keep a dependent at its pin beside another
  version of a row it requires. The dependent's own tag moved onto that version.
  0.15.1 and c5e3824 exited `0`, or `79` on a bundle selector. The row it
  requires can be a bundle too, or a bundle nested in one at any depth. Where
  the republished list nests that bundle, 0.15.1 exited `65` on a part it wrote
  instead, since it refuses the nested bundle. The owner ratified this on
  2026-11-08. The refusal needs proof, from the lock's records or from a
  manifest or tag read that succeeded. A failed read refuses nothing by itself.
  Run the whole plugin to move both.
- The same refusal also covers four more shapes of that pick, which exited `0`
  before. They arise on a part c5e3824 wrote, and the first two also on a part
  0.15.1 wrote. The last one also arises on a part this grim wrote, when the pick
  is a whole bundle such as `team:pack`. Each needs proof, as above, and writes
  nothing.
  - A nested list names the moved bundle under two bindings.
  - The dependent requires the moved bundle under another binding than the
    pick's. Grim matches it by repository where the pick reaches exactly one
    bundle of it.
  - The dependent requires a bundle that lists a row the pick moves to another
    version.
  - The dependent's requirement tag was republished onto the digest the pick
    moves to.

  A pick that leaves the dependent beside the same version exits `0` as before.
  So does one that reaches several bundles of the held repository. The refusal
  text says that the whole-plugin update it offers moves every pin of the plugin
  whose tag moved.

Some exits went the other way, and a job that read the failure as a signal loses
it:

- `grim lock` and `grim add` exit `0` with the pin kept on a diamond whose edges
  parted after a tag moved, where they exited `78`. Bare `grim update` still exits
  `78`, so run that to find the split.
- `grim add` of a bundle member beside a dependent whose entry records no edges,
  with the install, exits `0` and keeps the pin. That holds where the member's
  moved tag names a manifest whose dependencies conflict or form a cycle back at
  the pinned digest or another. Grim 0.15.1 exited `65` there. c5e3824 exited
  `78` on a conflict and `65` on a cycle. Online, a gone tag below the moved manifest keeps the pin too. There c5e3824 exited `79`, and 0.15.1 exited `0` with `--no-install` and `65` with the install. Offline that add still exits `69`, as both did.
- `grim update <names>` exits `0` beside bundles whose moved tags conflict, where
  it exited `78`. So does a `<plugin>:<member>` or `<plugin>:<bundle>` pick on a
  plugin that records the other bundle's digest. For `grim lock` that is the
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
  conflict. So does a name or pick that selects a moved
  bundle, unless the other bundle names the shared entry only under its own
  moved tag. For a pick, that exception holds only on a plugin that records the
  other bundle's digest. A `<plugin>:<bundle>` pick exits `78` too where the
  bundle it names newly nests one whose moved list conflicts with a direct
  include. That holds on a plugin that records no digest for it, where 0.15.1
  refused the nested bundle at `65`. A `<plugin>:<bundle>` pick of a bundle that
  nests nothing meets the same conflict there too. It exits `78` where 0.15.1 and
  c5e3824 exited `79`, since the pick is no longer refused as a selector.
- A `<plugin>:<member>` or `<plugin>:<bundle>` pick exits `0` when the tag of a
  bundle it does not select is withdrawn, where it exited `79`. That holds on a
  plugin that records that bundle's digest, while the registry still serves that
  digest. A run on the whole plugin still exits `79`.
- `grim lock` exits `0` after `grim remove` of a direct declaration that held a
  version conflict off, and keeps that key's pin, where it exited `78`.
  `grim update` still exits `78` on that conflict.
- A declared artifact's tag, or a bundle member's, can be withdrawn while the
  registry still serves its digest. `grim lock` then keeps the pin at exit `0`,
  where it exited `79`. `grim add` of the same reference keeps it too, where it
  exited `79`, and for a member also offline, where it exited `69`. Both hold on a
  clean lock 0.15.1 wrote, and for a member on one with no member list for its
  bundle. On a clean lock 0.15.1 wrote, any other `grim add` keeps the pin too,
  where it exited `79`, and so does TUI Install. Where another artifact asks for
  the member at another tag, the add still reads the member's tag and exits `79`
  when it is gone.
- Offline on an empty `GRIM_HOME`, `grim add --no-install` of a reference the
  lock pins keeps that pin at exit `0`, where it exited `69`. Without
  `--no-install` the add saves `grimoire.toml` and the lock, then exits `81`
  where it exited `69`, as the empty store holds no files to install. Both hold
  on a lock 0.15.1 wrote over one skill, its tag moved or not, and for a bundle
  member, its tag unchanged, moved or withdrawn. A bundle member's
  `--no-install` add keeps its pin at `0` on a store an online add filled too,
  where it exited `69`. That holds beside a dependent where the member's moved tag
  names a manifest whose dependencies conflict. It holds too where they form a
  cycle, back at the pinned digest or at another.
- `grim install` exits `0` after `grim remove` of a direct declaration that a
  bundle still lists at a reference its pin's digest answers, where it exited
  `65`. The lock keeps that pin and writes a real hash.
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
- `grim update <names>` exits `0` past an unnamed artifact you edited, where it
  exited `65`.
- Offline, `grim update <names>` exits `0` when an artifact outside its names has
  no content in the local store, where it exited `81`. Bare `grim update` and
  `grim install` still exit `81` there.
- `grim update <bundle>` exits `0`, where it exited `79`.
- `grim update <member>` exits `0` for a member its bundle's moved tag no longer
  lists, where it exited `79`. So does a `<plugin>:<member>` pick on a plugin that
  records the bundle's digest. The bundle keeps its pin. The member resolves the
  reference that pin lists for it, so its own pin moves only if that tag moved.
- `grim update --marketplace <manifest> <plugin>:<bundle>` exits `0` and moves the
  bundle, where it exited `79`. Each row that carries the bundle's stamp records
  its new digest.
  On a plugin with no lock entry yet it still exits `79`. It also still exits `79`
  for a bundle only a `requires` edge reaches, when 0.15.1 last wrote the plugin
  and its `include` lists no bundle. Select `<plugin>` in both cases.
- Offline over a `marketplace.lock` 0.15.1 wrote, `grim export plugin`,
  `grim export marketplace` and `grim update --marketplace`, whole or with a
  selector, exit `0` where they exited `81`. That holds while the tag cache answers
  every tag the run reads, each included bundle's among them. Otherwise they still
  exit `81`, and so does an export whose pinned content the store lacks. An export
  follows those cached tags and warns what that moved. A pick keeps every row
  outside its selection.

Smaller paths follow the same rule. TUI Update refuses a stale lock, as
`grim update <names>` does.

`grim remove` never changes a digest, and a kept pin keeps the tag that resolved
it beside the request that survives. The one tag it writes is on a tagless pin,
when the edit [ends the lock's proof][remove]. An edit that leaves the lock
stale and changes its rows writes a hash no declaration produces. Restoring
`grimoire.toml` by hand then does not make that lock read fresh.

On the input side, `pin = true` in `publish.toml` is accepted on every entry
kind, where it was once an error outside bundles, and `grim release --pin`
freezes the dependencies of any kind. A `publish.toml` that parsed before
parses unchanged.

A bundle that grew a `[bundles]` table and an artifact that grew a second
layer widen the OCI wire format in the same additive direction. A
dependency-free artifact and bundle publish the bytes they always did.

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

The [`.grimignore`][grimignore] built-in default list follows the additive
rule too, but only in one direction: growing it (adding a new pattern in a
minor release) changes what **new publishes** pack. A published layer is
never rewritten, so an artifact that already ships a now-defaulted file
keeps its bytes. The one visible effect: if such an artifact is installed
and its hash was recorded before the growth, it reads as ordinary drift,
recoverable the same way any local edit is: `grim install --force` /
`grim update --force`.

The compressed layer type follows the same rule. The `tar+gzip` layer media
type and the `com.grimoire.uncompressed.digest` layer annotation join the
frozen OCI wire format from the release that introduced them. They are
written only when a publisher opts in with
[`--compression gzip`][publishing-compression], so default output stays
byte-identical. grim never rewrites a published layer itself. `--force`
publishes a new manifest at the tag instead, which changes the version's
digest.

## Unstable — may change in any minor {#unstable}

Several things are deliberately excluded from the guarantee above. Freezing
them would block improving Grimoire without a major version bump. The
exclusions are what keep 1.x able to move at all:

- **The experimental parts of the [hook](./hooks.md) artifact kind.** Hooks
  are **experimental pre-1.0**. These may change in any minor release, in
  ways that are not always additive:
  - the `hook.toml` authoring surface (its fields and what `grim build`
    accepts), within the wire-format promise below,
  - the `[hooks]` table of `grimoire.toml`, the `[hooks]` table of a bundle
    source manifest, and the `[hooks.<name>]` entries of `publish.toml`,
  - the `grimoire.lock` `[[hook]]` array and the `[options.experimental] hooks` key,
  - every `grim hook` verb and its report JSON, and `grim schema --kind hook`,
  - the dispatch table's on-disk shape. The consent flag pair is no longer
    hook-only. Since 0.16.0 `--trust-hooks` and `--no-trust-hooks` are
    aliases of `--trust-workspace` and `--no-trust-workspace`, which the
    settings entry below lists.

  Four launcher contracts are built to be frozen. They are experimental today
  and grim already treats them as fixed, so they join the frozen list when
  hooks graduate:
  - the launcher's argv, `grim-hook <client> <event> <root>`: exactly three
    operands and no flags. It can gain new `<client>` and `<event>` values and
    never an operand, so one launcher serves every project on the machine,
  - the launcher's exit codes `0`, `1`, `64`, `65`, `74` and `79`, never `2`.
    See [Launcher exit codes](./hooks.md#launcher-exit),
  - the dispatch table's read rule: `schema` stays `1`, a reader ignores keys
    it does not know, and it skips an unreadable entry or root that is not
    addressed to the invocation. An unreadable entry the invocation selects
    makes the launcher exit `65`, because skipping it could drop a guard. New
    capability arrives as optional table fields, never as argv,
  - the registration shape: Copilot gets an `exec` and `args` pair, and
    Claude Code, Codex and Qoder get one shell string,
    `<launcher> <client> <event> <root>`. The same grim writes the same bytes
    every run. Codex keys its stored approval on that text, so a change would
    ask every Codex user to approve their hooks again.

  What is **not** exempted: the hook literals already shipped inside
  otherwise-frozen reports. These follow the same
  [additive-field policy](#frozen-additive-fields) as every other report
  field, and stop changing meaning once shipped:
  - the `install` row's `armed` field,
  - the `status`/`hook list` row's `state` and `cause` tokens,
  - the `export plugin` and `export marketplace` rows' hook-admission
    `reason` literals.

  See [Hook arming][json-hook-arming-stability] for the full list.

  The OCI wire format is frozen too. `grim build`/`grim release --kind
  hook` are not flag-gated, and a `schema = 1` `hook.toml` pushed today
  stays readable by every future grim. An incompatible manifest change
  ships as a new `schema` version and never changes what `schema = 1` means.

  A hook does not run anything until the `[options.experimental] hooks`
  flag is on and the workspace has consented. See [Arming
  gates](./hooks.md#why-gated). It graduates to a
  frozen contract once real consumers have shaped it, the same path
  `--progress json` is on below.

- **The experimental [settings][settings] artifact kind.** Settings ship in
  grim 0.16.0 behind the `[options.experimental] settings` key. While that key
  exists, these may change in any minor release:
  - the `[settings]` table of `grimoire.toml`, the `grimoire.lock`
    `[[settings]]` array, and the `[options.experimental] settings` key,
  - the [settings document][settings-format] grammar, the typed list of keys
    grim accepts, and `grim schema --kind settings`,
  - the `--trust-workspace` and `--no-trust-workspace` flags, and the
    `consent` alias of `grim hook`,
  - the `settings_changes` and `pieces` report keys, including both inside
    the error document,
  - the exit codes a settings cause returns, `65`, `75`, `78` and `81`,
  - the file format grim keeps its settings records in.

  grim keeps these additive by choice. The guarantee does not cover them.

  What is **not** exempted: the literals settings rows add to otherwise-frozen
  reports. They follow the [additive-field policy](#frozen-additive-fields):
  - the `gated` state on a settings row, and the `no-applicable-settings`,
    `client-config-absent`, `local-file-tracked`, `settings-root-relative`
    and `settings-root-in-workspace` causes,
  - the `element` field on `grim status` outputs and on `abandoned_entries`
    rows. It is always present, and `null` for every other kind and for a
    settings value that is not an array element.

- **Vendor render layout.** The exact files and paths grim writes under
  any client's own configuration root, and where an MCP entry lands
  inside a client's own config file, are not a contract. Neither is the
  shared `$HOME/.agents/skills` pool: *which* clients render into it
  moves as clients adopt or retire the convention. A pool-reading client
  joins the pool beside a client that writes it, and
  [`[options.vendors.<name>].shared_skills`][options-vendors] pins that
  choice either way — moving that client's skills out of its own directory
  and into the pool, or back. The per-client roots are
  enumerated by the [vendor projection layer][vendor-metadata] and in the
  [client compatibility matrix][clients]; those enumerations are
  documentation, not a promise, and are free to move between minors as
  clients change their own conventions.

  A root that a vendor environment variable relocates moves the layout
  with it, and a release that *starts* honoring one moves it for everyone
  who already set that variable. The most recent instance: `$JUNIE_HOME`,
  `$OPENCLAW_STATE_DIR` / `$OPENCLAW_HOME`, and a `CLAUDE_CONFIG_DIR` set
  in Claude Code's own settings `env` became honored. Before them,
  `$KIRO_HOME` and `$GEMINI_CLI_HOME` did, and Zed's macOS root stopped
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
- **Search relevance.** Which rows a query returns, and their order, can
  change between minors as ranking improves. The `grim search` and
  `grim_search` JSON shape, field names, exit codes and sort keys are
  contracts. The set of matches is not. Changes you would notice are
  listed in the [upgrading guide](./upgrading.md).
- **NDJSON progress events** (`--progress json`) stay **experimental
  through 1.0**, deliberately. The event shapes evolve additively (new
  fields may appear, existing ones keep their meaning), but the surface
  itself is not frozen by the 1.0 release: it shipped too recently for any
  external consumer to have built against it, and freezing a contract
  nobody has exercised buys a guarantee no one asked for at the cost of
  never being able to fix it. It freezes in a later minor, once a real
  consumer has shaped it. Anything you script against it today may move.
- **[Telemetry][telemetry].** The opt-in usage export is Unstable
  through 1.0. That covers the `GRIM_TELEMETRY` and
  `GRIM_TELEMETRY_REPOSITORY` switches, `GRIM_CONFIG`, the `[telemetry]`
  table of its files, `GRIM_TELEMETRY_OTLP_HEADERS`, the `grim.command` and
  `grim.artifact.changed` event names, every [attribute and resource
  key][telemetry-fields], the `GRIM_LOG=debug` payload line, and the
  `$GRIM_HOME/telemetry/installation-id` file. Keys may be added, renamed
  or removed in any minor release. Neither the switches nor the files
  change anything for a user who sets neither `GRIM_TELEMETRY` nor
  `GRIM_CONFIG`.
- **[`grim export plugin`][export-plugin] and [`grim export
  marketplace`][export-marketplace] tree bytes.** The report shape and
  version grammar are frozen under the table rows above. The commands'
  [exit codes][exit-codes] are frozen the same as every other command's. The
  exact bytes rendered under `skills/`, `agents/`, `.mcp.json`, or
  `mcp.json` inside an exported plugin are not. That is the same rule as
  [vendor render layout](#unstable) above, because export reuses the same
  per-client materializer `grim install` does. The same goes for a tree's
  manifests and `README.md`, including Cursor's second manifest
  `.cursor-plugin/plugin.json`. The `version` suffix hashes the rendered
  tree, so a minor release that changes how a client renders a skill also
  changes the suffix of every affected plugin. What feeds the hash is not
  frozen either. A later minor may change it, and every suffix moves once
  when it does. That only ever changes a suffix, never the grammar.
  Consumers that pin an exact plugin version should expect it.
  `export marketplace`'s plain output, the table on stdout and the per-file
  lines on stderr, carries no promise. Only its JSON report does.

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
locally-modified dropped-client output. That reaper only fires on a bare
`grim update` with `[options].clients` explicitly set, never on a named run.
With it unset (autodetect) `update` never reaps either, since the desired set
would otherwise track live client detection rather than the user's config.)

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

The same holds for an install-state file that records an anchor tag added
in a newer release, such as `goose-root`, `kilo-config-root` or
`qoder-root`. A `grim` build that predates the anchor refuses to load that
file rather than drop the record it cannot place. That refusal exits 74
(`EX_IOERR`), not 78, because grim reports a failed state read as an I/O
error on the install target. A command that reads a config or lock first
exits 78 when that earlier file is the one refused.

From 0.16.0, `grim lock`, `grim add` and `grim update` also exit `78` on a lock
they cannot parse. Up to 0.15.1 those three read such a lock as no lock, exited
`0` and overwrote it.

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

This only triggers when the feature is actually in use. A registry-only
lock or state file stays identical across the version boundary apart from the
tag in `pinned`. A project that never declares a path source is
unaffected either way.

That tag is the one difference in a lock that answers its declarations, and
**reading on both sides** is the promise that holds. A real grim 0.15.1 binary
read a lock that records its tags. `status`, `install` and `lock` all exited
`0`, and `install` wrote the pinned digest's bytes after the tag had moved.
[Verify Deep][verify-deep] repeats that run in its old grim interop step:
`task test:old-grim` with `GRIM_OLD_BIN` naming the released binary.

That promise covers a project with no settings package. The settings table
below lists what 0.15.1 does with one.

A [`grim remove`][remove] that leaves the lock stale is the exception. It can
keep a row that 0.15.1's `remove` drops. When it also changes the lock's rows,
it writes a hash no declaration produces, in the same `sha256:<hex>` form, and
otherwise keeps the hash. 0.15.1 reads that lock as stale:
`status` reads `stale` on every row and `install` exits `65`. Its own
`grim lock` resolves from scratch and exits `0`, or `78` where the sources
disagree. Offline it exits `69`, unless no bundle is declared and its tag
cache holds every tag.

Its `lock` strips the tags and rolls a moved tag, as it always did. This grim
reads that result as a clean lock, keeps those pins and adds the tags back on
its next write. 0.15.1's `install` copies the tag into `state.json`, and this
grim reads that record as the same pin. What a team on both versions sees is in
[Upgrading][upgrading-lock-keeps-pins].

The same `deny_unknown_fields` stance covers a [hook](./hooks.md)-bearing
lock. A `grim` build that predates the `hook` artifact kind refuses to
load a lock declaring one, exiting **78** the same as any other unknown
field. It never silently drops the entry and runs the rest of the
project's artifacts as if the hook did not exist.

A project that later drops its only hook declaration and re-locks
downgrades cleanly. The lock carries no `hook` entry any more, so a
`grim` build that predates the kind reads it exactly as it always has.

The experimental [settings][settings] kind, new in 0.16.0, holds the same
line. grim 0.15.1 predates it and meets each of its surfaces this way:

| What grim 0.15.1 reads | What it does |
|---|---|
| a `grimoire.lock` with a `[[settings]]` array | exit `78` on `status` and `install`. Its `lock`, `add` and `update` read a lock they cannot parse as no lock, so they exit `0` and write a new one |
| a `grimoire.toml` with a `[settings]` table or the `[options.experimental] settings` key | exit `78` |
| a bundle with a settings member, or an artifact that requires a settings package | exit `65`, and nothing is installed |
| `grim add` of a settings artifact | exit `65`, and nothing is declared |
| a `$GRIM_HOME/state/global.json` holding a settings record | exit `74`, and the file is left as it was |
| a consent record holding `settings:` keys | `grim hook allow` drops those keys, so a newer grim asks again |

The `74` row reaches only a user who downgrades after a global settings
install. A project-scope settings record never enters `.grimoire/state.json`,
so that file stays readable by 0.15.1. The kind adds nothing to the lock or
state of a project that declares no settings package.

[Declared dependencies](./concepts.md#dependencies) hold the line in both
places they reach an older grim: the lock and the artifact. A lock that holds
a `[[<kind>.requires]]` table or `transitive = true` on a bundle entry is
refused by grim 0.15.1's `status` and `install` with exit **78**, like any other
unknown field. Its `lock`, `add` and `update` do not refuse: they overwrite the
file, as above. A project drops back to a lock that 0.15.1 reads by removing its
last dependency and re-locking. A lock without dependencies stays identical to
what 0.15.1 wrote apart from the tag in `pinned`, and it reads on both
sides.

The artifact side was checked against a grim 0.15.1 binary:

| An artifact or manifest that has | grim 0.15.1 |
|---|---|
| a second (requires) layer, installed with `grim add` | exit `65`, `expected a single-layer artifact`; no files are installed. On a lock it wrote, that includes `grim add` of a bundle member whose moved tag names such a manifest, beside a dependent, after it moved the pin. Where that manifest's dependencies conflict or form a cycle, this grim keeps the pin and exits `0` |
| a second layer, met by `grim lock`, `grim update` or `grim add --no-install` | succeeds and writes a lock that **omits the dependencies**, with no warning |
| a bundle manifest listing a nested bundle | exit `65`, `nested bundle member … is not supported` |
| three or more layers | exit `65`, `expected a single-layer artifact` |
| a `com.grimoire.requires-features` annotation | ignored |

A lock whose bundle member merely has `kind = "bundle"`, without `transitive`,
is parsed and ignored by 0.15.1. It is not an error there. This grim marks the
nested snapshot `transitive = true` whenever it writes a nested-bundle lock, so
0.15.1's `status` and `install` refuse that lock with exit **78**.

This grim refuses the formats it does not know in the other direction. A
manifest that names a feature outside the ones this grim understands, or whose
layers are not one of the [accepted shapes](./artifacts.md#requires-layer),
exits `65` with the reason `unsupported-artifact-feature` and a message that
says to update grim. It does so on every path that reads the content:
`grim install`, `grim fetch`, a lock or update that has to read its
dependencies, and both exports. [`grim describe`](./commands.md#describe) only
reads metadata, so it still describes such an artifact. The same reason now
also covers a bundle manifest that does not have exactly one layer, which
reported a reason-less `65` before.

The hole in that table is the second row, and it cannot be closed from this
side. A project that has any machine on 0.15.1 can lock a dependent artifact
there without its dependencies. A newer grim that installs from that lock
refuses it with exit `65` and the reason `stale-lock`, because the lock's
dependencies do not match the pinned artifact, and asks for `grim lock`. A
0.15.1 install of the same lock fails at the artifact itself. Nothing is
installed incompletely, but the lock stays wrong until a newer grim re-locks
it. See [Upgrading](./upgrading.md#dependencies-need-new-grim) for what to do.

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
This release cannot reach a binary that already shipped.

Say you browse a [package index](./package-index.md) and then downgrade
to 0.13. From 0.14 this matters for an index that publishes ratings. After
0.14.2 it matters for every index. Then delete `$GRIM_HOME/catalog/` once.

See [Upgrading][upgrading-catalog-downgrade]. Both a rating and the tree
placement a package index gives each row add a field to the cache. An
entry with neither, and no `kind_read` marker, still reads in 0.13.

A direct OCI browse is exposed too. Since 0.16.0 a row for a foreign
repository, such as a container image, carries `kind_read` once grim has
read its manifest. So a registry that holds foreign repositories writes a
cache that 0.14.x and earlier refuse. Online, 0.14.x pays one rebuild.
Offline, the refusal shows until an online run or until `$GRIM_HOME/catalog/`
is deleted, and 0.13 and earlier need that deletion either way.

From this release the loader ignores cache fields it does not know. A cache
that a later grim extended is read, not refused, so the rebuild above applies
only to builds that predate this change. Beside each cache file grim keeps a private
[sync sidecar][data-layout]. It is not a contract: a sidecar version this grim
does not know is ignored, at the cost of one fresh sync.

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

Grimoire caches a fetched artifact's content layer, and the manifest it was
fetched through, content-addressed so identical bytes are never re-downloaded.
An offline [`grim install`][install] whose rendered output is still on disk is
network-free: the integrity gate compares the on-disk content hash against
the lock and needs nothing from the registry.

Deleting that output and asking `--offline` to re-materialize it works when
the store still holds the pinned manifest and its layer. A pinned manifest is
read from the store before the registry, in both modes, and only when the
stored bytes hash to the digest. Against a cold store, or one that never
fetched that digest, the same request exits `81`, because the manifest has to
be *fetched* to learn which layer blob to pull. A tag reference, as opposed to
a pinned digest, still needs the network to resolve.

This also widens two read-only commands. An offline
[`grim describe`](./commands.md#describe) or [`grim fetch`](./commands.md#fetch)
of a digest-pinned reference whose manifest is stored now succeeds, where it
exited `81` before. It is a widening and changes no field.

The constraint applies to every registry-sourced kind whose materialized
output has gone missing while offline. That is a skill, rule, agent, MCP
server, hook, settings package or bundle member. [Local path sources](./concepts.md#references-tags-and-digests) are
unaffected — they read straight from disk and never touch a manifest.

<!-- internal -->
[json-interface]: ./json-interface.md
[status]: ./commands.md#status
[add]: ./commands.md#add
[lock]: ./commands.md#lock
[lock-pins]: ./commands.md#lock-pins
[remove]: ./commands.md#remove
[tui]: ./commands.md#tui
[status-check]: ./commands.md#status-check
[describe]: ./commands.md#describe
[uninstall]: ./commands.md#uninstall
[why]: ./commands.md#why
[update]: ./commands.md#update
[publishing-report]: ./publishing.md#batch-publish-report
[install]: ./commands.md#install
[install-dev]: ./commands.md#install-dev
[context]: ./commands.md#context
[exit-codes]: ./json-interface.md#error-document
[annotations]: ./artifacts.md#annotations
[artifacts-layers]: ./artifacts.md#layers
[publishing-compression]: ./publishing.md#compression
[grimignore]: ./artifacts.md#grimignore
[bundles]: ./artifacts.md#bundles
[mcp-descriptor]: ./mcp-servers.md#format
[mcp-server]: ./commands.md#mcp
[package-index]: ./package-index.md#spec
[stats-sidecar]: ./package-index.md#spec-stats
[index-root]: ./package-index.md#spec-root
[index-blobs]: ./package-index.md#spec-blobs
[data-layout]: ./configuration.md#data-layout
[ratings]: ./ratings.md
[search]: ./commands.md#search
[json-items]: ./json-interface.md#items-envelope
[json-sources]: ./json-interface.md#search-sources
[json-total]: ./json-interface.md#search-total
[json-update-rows]: ./json-interface.md#update-rows
[upgrading-catalog-downgrade]: ./upgrading.md#catalog-cache-downgrade
[upgrading-lock-keeps-pins]: ./upgrading.md#lock-keeps-pins
[init]: ./commands.md#init
[configuration]: ./configuration.md
[options-vendors]: ./configuration.md#options-vendors
[clients]: ./clients.md#matrix
[gap-shared-pool]: ./clients.md#gap-shared-pool
[env-vars]: ./configuration.md#environment-variables
[telemetry]: ./telemetry.md
[telemetry-enable]: ./telemetry.md#enable
[telemetry-fields]: ./telemetry.md#fields
[telemetry-config]: ./telemetry.md#config-file
[artifacts-kinds]: ./artifacts.md#kinds
[batch-publish]: ./publishing.md#batch-publish
[publishing-release]: ./publishing.md#release
[vendor-metadata]: ./vendor-metadata.md
[path-sources]: ./concepts.md#references-tags-and-digests
[json-hint]: ./json-interface.md#error-hint
[marketplace-toml]: ./configuration.md#marketplace-toml
[export-plugin]: ./commands.md#export-plugin
[export-plugin-output]: ./commands.md#export-plugin-output
[export-marketplace]: ./commands.md#export-marketplace
[export-marketplace-exit]: ./commands.md#export-marketplace-exit
[export-marketplace-json]: ./commands.md#export-marketplace-json
[export-marketplace-layout]: ./commands.md#export-marketplace-layout
[export-marketplace-ownership]: ./commands.md#export-marketplace-ownership
[marketplace-table]: ./configuration.md#marketplace-table
[plugin-metadata]: ./configuration.md#plugin-metadata
[json-shapes-items]: ./json-interface.md#shapes-items
[json-hook-arming-stability]: ./json-interface.md#hook-arming
[settings]: ./settings.md
[settings-format]: ./settings.md#format

<!-- external -->
[verify-deep]: https://github.com/grimoire-rs/grimoire/blob/main/.github/workflows/verify-deep.yml
[gnu-make]: https://www.gnu.org/software/make/manual/make.html
[cargo-manifest]: https://doc.rust-lang.org/cargo/reference/manifest.html
[npm-package-json]: https://docs.npmjs.com/cli/v10/configuring-npm/package-json
[helm-chart]: https://helm.sh/docs/topics/charts/#the-chartyaml-file
[npm-install]: https://docs.npmjs.com/cli/commands/npm-install
