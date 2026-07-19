# Stability and Versioning

Grimoire is pre-1.0: the CLI, formats, and OCI pipeline documented across
this book are real and tested, but the surface has moved between minor
releases while the project found its shape.

1.0 draws a line. A script parsing [`grim status --format
json`][status], a shell `case` on an [exit code][config-exit-codes], or a
tool reading `grimoire.lock` needs to know what survives an upgrade
unmodified and what does not — otherwise "just run `grim update`" is a
gamble, not a routine operation.

This page names exactly what becomes a semver-guarded contract at 1.0 and
what is explicitly excluded from it.

## Frozen at 1.0 {#frozen}

Breaking any guarantee below is a major-version change, not a minor one.

| Area | Guarantee |
|------|-----------|
| CLI surface | Subcommand names, arguments, flags, and [documented exit codes][config-exit-codes] |
| `--format json` reports | The report shape for every command that offers one, and the [error document][json-interface] — see [Additive fields](#frozen-additive-fields) and the [JSON interface reference][json-interface] |
| `grimoire.toml` / `grimoire.lock` | The [config and lock schema][configuration] |
| Install state (`state.json`) | Schema V2, governed by the same additive-field policy as JSON reports |
| OCI wire format | [Artifact kinds][artifacts-kinds] and the [release/push mechanics][publishing-release] |
| Environment variables | The documented [`GRIM_*` set and honored vendor overrides][env-vars] |

### Additive fields {#frozen-additive-fields}

Two rows above — `--format json` reports and the install-state schema —
share one rule: a minor release may add a new optional field, but never
changes an existing field's type or meaning, and never removes one. The
matching obligation sits on the reader: a consumer of either format must
ignore fields it does not recognize rather than error on them. That pairing
is what makes "add a field in a minor" safe for every consumer, including
ones written before the field existed.

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

The newest instance is [`grim publish`][publishing-report]'s
`announce.fork` (`{repo, created}` or `null`), added when `--announce`
gained automatic fork detection. It extends the already-frozen `announce`
object the same always-present-null way: `null` when the branch pushed
straight to the index repository (no fork involved, or `[announce] fork =
false`), populated with the fork's full name and whether it was newly
created once forking activated.

## Unstable — may change in any minor {#unstable}

Two things are deliberately excluded from the guarantee above, because
freezing them would block improving Grimoire's on-disk footprint without a
major version bump:

- **Vendor render layout.** The exact files and paths grim writes under
  `~/.claude`, `.claude/`, `~/.copilot`, the OpenCode config directories,
  `~/.cursor`, `~/.kiro`, `~/.junie`, `~/.gemini`, the Zed and Amp
  settings directories, the shared `$HOME/.agents/skills` pool (Codex,
  Gemini, Zed, Amp), and where an MCP entry lands inside a client's own
  config file are not a contract. They are an implementation detail of
  the [vendor projection layer][vendor-metadata], free to move between
  minors as clients change their own conventions.
- **Everything else that is not exit codes or JSON.** State-file contents
  beyond the schema guarantee, TUI appearance and keybindings, and
  human-readable log or error text carry no compatibility promise — only
  exit codes and structured JSON output are contracts.
- **NDJSON progress events** (`--progress json`) are **experimental
  pre-1.0**: the event shapes evolve additively only (new fields may
  appear, existing ones keep their meaning), and the surface freezes at
  1.0 once external consumers have validated it.

### The supported discovery channel {#unstable-discovery}

Because render layout can move, scripting "where did grim put this skill?"
against a hardcoded path is unsupported and will eventually break. Use
[`grim status --format json`][status] instead: every entry carries an
`outputs` array of `{client, path}` pairs — the per-client materialized
locations read back from install state, empty for a declared-but-not-yet-
installed artifact. `outputs` is itself covered by the [additive-field
policy](#frozen-additive-fields) above, so code that reads it survives an
upgrade even as the paths inside it change.

## The compatibility promise {#promise}

Vendor layout moving is not, by itself, a compatibility break — provided
grim upholds this: artifacts remain discoverable by the target client;
status, update, and uninstall keep working across upgrades; exact vendor
paths may change in a minor release with automatic migration.

That migration is mechanical: the first install or update after an
upgrade that moved a layout re-materializes the artifact at its new
path, re-anchors the install record, and reaps the unmodified old
output. A locally modified old file is never deleted — the same
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
stability (`.claude/artifacts/adr_render_layout_stability.md`).

## Known limitations {#limitations}

Three behaviors fall outside every guarantee above — not because they are
likely to change, but because they are hard constraints of the current
design.

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
[config-exit-codes]: ./commands.md#config-exit-codes
[configuration]: ./configuration.md
[env-vars]: ./configuration.md#environment-variables
[artifacts-kinds]: ./artifacts.md#kinds
[publishing-release]: ./publishing.md#release
[vendor-metadata]: ./vendor-metadata.md
[path-sources]: ./concepts.md#references-tags-and-digests

<!-- external -->
[gnu-make]: https://www.gnu.org/software/make/manual/make.html
[cargo-manifest]: https://doc.rust-lang.org/cargo/reference/manifest.html
[npm-package-json]: https://docs.npmjs.com/cli/v10/configuring-npm/package-json
[helm-chart]: https://helm.sh/docs/topics/charts/#the-chartyaml-file
[npm-install]: https://docs.npmjs.com/cli/commands/npm-install
