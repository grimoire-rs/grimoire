# Upgrading

[`CHANGELOG.md`][changelog] lists every change, one line per commit. This page
carries the ones that need more than a line: a behaviour you will notice on
the first command after an upgrade, and what to do about it.

Nothing here is a breaking change. Grimoire is [stabilizing toward
1.0][stability] and evolution is additive-only — but "additive" is a statement
about contracts, not about what you *see*, and three of these are visible
enough to look like bugs if you meet them cold.

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

**What you will see** if you had already set `$KIRO_HOME` or
`$GEMINI_CLI_HOME`: artifacts installed before the upgrade sit at a root grim
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

<!-- internal -->
[changelog]: https://github.com/grimoire-rs/grimoire/blob/main/CHANGELOG.md
[browse-filters]: ./configuration.md#browse-filters
[stability]: ./stability.md
[unstable]: ./stability.md#unstable
[status]: ./commands.md#status
[install]: ./commands.md#install
[vendor-metadata]: ./vendor-metadata.md#projection-semantics
[no-clobber]: ./json-interface.md#error-reason
[multi-registry]: ./configuration.md#multiple-registries
