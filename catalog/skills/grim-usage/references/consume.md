# Consumer Lifecycle

You loaded this file because you are installing, updating, or removing
artifacts in a project with grim — the init → add → lock → install →
update loop and the two files it maintains.

Contents: [The Loop](#the-loop) · [The Two Files](#the-two-files) ·
[Declaring](#declaring) · [Installing](#installing) ·
[Updating](#updating) · [Inspecting](#inspecting) ·
[Removing](#removing) · [Bundles](#bundles)

Flags shown here are grim 0.10.x; confirm with `grim <cmd> --help` before
relying on one.

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
`[rules]` / `[agents]` / `[bundles]` tables mapping a binding name to a
reference. Manage settings and registries with `grim config` rather
than by hand — see [registries.md](registries.md#managing-config); it never
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

- `--kind` (skill, rule, agent, bundle, mcp) is normally inferred from the
  artifact's kind metadata set at release time (the `com.grimoire.kind`
  annotation; legacy `artifactType` on older artifacts). If grim cannot
  infer it (a non-Grimoire image), `add` errors and asks for `--kind`.
- `--name` defaults to the reference's last path segment. A name that is
  already declared for that kind under a *different* reference refuses
  (exit 64) instead of silently replacing it — pass `--name` to bind the
  new reference under another name. Re-adding the same reference is a
  no-op. A renamed skill installs under the binding name with its
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
  overwrite deliberately — identical semantics to `grim install --force`.
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

`grim lock` re-resolves the floating tags declared in `grimoire.toml`
and rewrites the lock. You need it only after hand-editing the config —
`grim add` already locks what it declares.

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
no record of writing — pass `--force` to overwrite deliberately.
See [troubleshooting.md](troubleshooting.md) for the integrity gates.

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
`--kind`, before retrying. Confirm the current flag set with
`grim install --help`.

## Updating

`grim update [names…]` re-resolves floating tags, rolls the lock
forward, and re-materializes only what changed. With no names it updates
everything; pass binding names to scope it:

```sh
grim update
grim update code-review rust-style
```

Update is also the only command that **prunes**: an artifact that
dropped out of the lock (most often a bundle member the bundle stopped
including) is deleted and reported as `removed` — unless you edited it
locally, in which case it is kept and reported as `kept-modified` until
you re-run with `--force`. Your local edits are never silently
discarded.

The same reconciliation applies when you narrow the configured client
set (`[options].clients`): a client that leaves the set has its outputs
reaped, listed per update row under `reaped_clients` — again, a locally
edited output is preserved under `kept_modified_clients` (with a warning)
until `grim update --force` removes it. Widening the set re-materializes
the added client on the next update. This reap only fires when
`[options].clients` is explicitly set; left unset (autodetect), `update`
never reaps — the desired set would otherwise track live client detection
rather than your config.

Update also refreshes every **local path source**: declared path
dependencies and dev-installed records alike are re-packed, and a changed
content hash re-materializes them — the local equivalent of a floating
tag rolling forward. `grim status` surfaces that drift ahead of time as
`outdated`; a path source that has gone missing (or no longer packs)
reports `outdated` too, never a false `installed`. A dev record is never
pruned — see [Installing](#installing).

## Inspecting

`grim status` reports each declared artifact's state — installed,
outdated, locally modified, integrity-missing, or not installed. The
`Source` column shows provenance: `direct`, the bundle the artifact came
from, `path: <path>` for a declared local path source, or
`path: <path> (dev)` for a dev-install. Pair with `--format json` to
drive automation — its `outputs`
array lists the per-client paths an artifact was materialized to, and is
the supported way to script against install locations (the on-disk
vendor layout itself is not a stable contract). Each item also carries
`clients_missing`/`clients_extra` — the *explicitly configured*
`[options].clients` diffed against what is actually recorded installed,
computed locally, no network. Left unset (autodetect), both stay `[]` on
every item instead of diffing against live client detection.

`grim status --check` adds one live catalog round-trip: it fills in
`deprecated`/`replaced_by` on every registry-sourced item and, via a
fresh per-artifact tag re-resolution, `update_available` — "is a newer
version out there right now" versus status's normal offline read. The
top-level `checked` field says whether the check actually ran online;
`checked == false` means all three of those item fields are `null`.
When scripting against it, treat `update_available: null` as "could not
determine", never as "up to date": it is `null` (not `false`) for a
bundle-member row, a row with no lock pin (declared bundle, dev-install,
path source), or a row whose own re-resolution failed. And even under
`checked == true`, one registry's catalog refresh failing degrades just
that registry's rows' `deprecated`/`replaced_by` to `null` — partial
data, not a failed check. Confirm the current flag set with
`grim status --help`.

Multi-item reports (`status`, `install`, `lock`, `update`, `search`,
`config list`, `config registry list`, `config registry fields`, `publish`)
wrap their rows in a uniform `{"items": [...]}` envelope under
`--format json` — read the array from `items`, never the top level.
Failures under `--format json`
emit a structured `{"error": {code, exit, message}}` document on stdout;
some failures add a machine-readable `reason` (e.g. `stale-lock` when a
partial `grim update <name>` is refused because the lock no longer matches
the declaration — retry with a full `grim update`). Full contract: the
[JSON interface][json-interface] docs page.

Two read-only companions:

- `grim context` reports the resolved invocation context — scope,
  config/lock/state paths (with existence flags), effective client set
  (names only), registry browse set, default registry, offline mode —
  so scripts need not reimplement walk-up or precedence rules. Each JSON
  `registries` entry carries an `authenticated` boolean: whether the
  docker-compatible credential store holds an entry for that registry's
  host (a file-only probe — it never invokes a credential helper, so a
  global `credsStore` alone reads as `false`).
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
  companion tags. `--format json` is a single object with every field
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
survives via the bundle (and `uninstall` still deletes the files — the
next `grim install` rematerializes them). When the surviving bundle
binds a *different* identifier, grim drops the entry, leaves the lock
stale, and tells you to run `grim lock` — never a silently wrong pin.

## Bundles

A bundle is a curated set of members. Declare it once and it **expands**
into its member skills, rules, and agents at lock time, each pinned like a
direct declaration and tagged with the bundle as its provenance:

```sh
grim add --kind bundle ghcr.io/acme/python-stack:1
grim install
```

Membership tracks the published bundle: a new bundle version that adds a
member expands it on the next `grim lock`; one that drops a member
removes it from the lock, and `grim update` prunes its files (subject to
`kept-modified` above).

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
[config-toml]: https://grimoire.rs/configuration.html
[json-interface]: https://grimoire.rs/json-interface.html
