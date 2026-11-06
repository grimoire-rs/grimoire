# Grimoire Manual Test Rig

A hands-on harness for exercising `grim` against real local OCI registries
with a committed sample catalog of skills, rules, and agents. This is **fully
separate** from the pytest acceptance suite (`test/tests/`): it runs its
own `registry:2` containers on **`localhost:5050`** (the primary `grimoire`
catalog) and **`localhost:5051`** (a small `tools` subset for the
multi-registry demo), each with its own container + volume, while the suite
uses `localhost:5000`. They are isolated on purpose — sharing one registry
let the suite's hundreds of throwaway `grim-test/<uuid>` repos bleed into
`grim search` / `grim tui` here as junk. It exists so you can drive the tool
by hand and see how it behaves.

Pattern mirrors the OCX manual rig: committed source-of-truth catalog,
idempotent `bootstrap.sh`, an isolated `GRIM_HOME`, a ready-made consumer
project, and a `teardown.sh`.

## Layout

| Path | Purpose |
|------|---------|
| `catalog/skills/<name>/SKILL.md` | Source-of-truth sample skills (committed) |
| `catalog/rules/<name>.md` | Source-of-truth sample rules (committed) |
| `catalog/agents/<name>.md` | Source-of-truth sample agents (committed) |
| `catalog/publish.toml` | Batch-publish manifest for the annotation showcase — `[metadata]` defaults, `[description]` companion (README + CHANGELOG), manifest-level `[support]` channels (committed) |
| `catalog/hooks/<name>/hook.toml` | Source-of-truth sample hooks + their payload scripts (committed) |
| `catalog/settings/team-defaults.toml` | Source-of-truth sample settings package: a model, allow and deny rules, a marketplace (committed) |
| `catalog/bundles/starter-pack.toml` | Bundle v1 member set (committed) |
| `catalog/bundles/starter-pack-v2.toml` | Bundle v2 member set — adds + removes members (committed) |
| `catalog/mcp/docs-search.toml` | Sample MCP server descriptor — a dependency target (committed) |
| `catalog/bundles/dev-suite.toml` | Bundle whose members carry their own dependencies (committed) |
| `catalog/bundles/review-pack.toml` | Bundle sharing `code-reviewer` with starter-pack + an agent member (committed) |
| `index/all.json` | Static HTTP index phone book — the same refs the registries serve (committed) |
| `index/stats.json` | Publisher-statistics sidecar: ratings + download counts (committed) |
| `index/README.md` | What each fixture row demonstrates, and why the index exists at all |
| `project/grimoire.toml` | Ready-made consumer project (floating `:1` tags) — the OCI registry plus the static index |
| `project-deps/grimoire.toml` | Dependency-demo project: declares only dependents, see § 12 |
| `project-settings/grimoire.toml` | Settings-demo project: the experimental flag on, one settings package declared, see § 13 |
| `project-multi/grimoire.toml` | Multi-registry consumer project (`[[registries]]` aliases across 5050 + 5051) |
| `scripts/env.sh` | `source` it to point `grim` at the rig |
| `scripts/bootstrap.sh` | Build `grim`, start both registries + the static index, publish the version matrix + multi-registry subset + deep-fold solo package + the annotation showcase, write the global registry config |
| `scripts/settings-env.sh` | `source` it after `env.sh` for the global steps of § 7 and § 13e: redirects `CLAUDE_CONFIG_DIR`, `CODEX_HOME` and `COPILOT_HOME` into `.settings-roots/`, which `in_rig` (§ 7) checks |
| `scripts/release-update.sh` | Publish `code-reviewer` 1.3.0 (post-lock outdated / rolling-release demo) |
| `scripts/teardown.sh` | Wipe rig state (`--registry` also stops both registries) |
| `docker-compose.yml` | `registry:2` on `localhost:5050` (primary) and `localhost:5051` (`tools` subset), plus `nginx` on `localhost:5052` serving `index/` |
| `.settings-roots/` | The three redirected harness roots `settings-env.sh` creates, plus `in_rig`'s scratch `home/` and § 13e's own `GRIM_HOME`, `grim-home/` (gitignored, ephemeral) |
| `.grim-home/` | Isolated `GRIM_HOME` (gitignored, ephemeral) |
| `.grim-home/grimoire.toml` | Global config bootstrap writes via `grim config` — three `[[registries]]` (5050/grimoire default + 5051/tools + the 5052 index); browse all of them from anywhere with `--global` |

## Quick start

```sh
test/manual/scripts/bootstrap.sh        # one command: build + registry + publish
source test/manual/scripts/env.sh       # point `grim` at the rig
```

Published catalog (a small **version matrix** — most artifacts ship one
1.0.0, a few carry extra versions for the upgrade / `↑ outdated` demos):

| Kind | Repo | Versions |
|------|------|----------|
| skill | `localhost:5050/grimoire/skills/hello-world` | 1.0.0 |
| skill | `localhost:5050/grimoire/skills/code-reviewer` | 1.0.0, 1.1.0, 1.2.0 (1.3.0 via `release-update.sh`) |
| skill | `localhost:5050/grimoire/skills/commit-helper` | 1.0.0, 2.0.0 |
| skill | `localhost:5050/grimoire/skills/architecture-guide` | 1.0.0 |
| skill | `localhost:5050/grimoire/skills/old-reviewer` | 1.0.0 (deprecated — drives the deprecation surface) |
| skill | `localhost:5050/grimoire/skills/support-desk` | 1.0.0 (annotation showcase — published with `grim publish`, carries a description companion) |
| skill | `localhost:5050/grimoire/playbooks/ci/release/cut-release` | 1.0.0 (deep solo path — drives the tree-fold demo) |
| rule | `localhost:5050/grimoire/rules/rust-style` | 1.0.0, 1.1.0 |
| rule | `localhost:5050/grimoire/rules/security-baseline` | 1.0.0 |
| rule | `localhost:5050/grimoire/rules/architecture-guide` | 1.0.0 |
| agent | `localhost:5050/grimoire/agents/reviewer` | 1.0.0, 1.1.0 |
| agent | `localhost:5050/grimoire/agents/release-bot` | 1.0.0 (vendor-override demo) |
| settings | `localhost:5050/grimoire/settings/team-defaults` | 1.0.0 (experimental kind, see § 13) |
| hook | `localhost:5050/grimoire/hooks/tool-logger` | 1.0.0 (observer; alternation matcher) |
| hook | `localhost:5050/grimoire/hooks/write-guard` | 1.0.0 (gatekeeper; JSON verdict) |
| bundle | `localhost:5050/grimoire/bundles/starter-pack` | 1.0.0, 2.0.0 (v2 adds commit-helper, drops security-baseline) |
| bundle | `localhost:5050/grimoire/bundles/review-pack` | 1.0.0 (shares code-reviewer with starter-pack, adds the reviewer agent) |

Each full-semver release cascades the floating tags forward, e.g. `1.0.0`
also sets `1.0`, `1`, `latest`; publishing `code-reviewer` `1.2.0` then
moves `1.2`, `1`, `latest` onto it. Because of this, `bootstrap.sh`
publishes versions in **ascending** order per artifact, so the floating
`:1`/`:latest` the consumer project pins always land on the highest
version (code-reviewer `1.2.0`, commit-helper `2.0.0`, rust-style `1.1.0`).

It also publishes a small **second-registry subset** at `1.0.0` for the
multi-registry demo (see scenario 2a):

| Kind | Repo | Versions |
|------|------|----------|
| skill | `localhost:5051/tools/skills/commit-helper` | 1.0.0 |
| rule | `localhost:5051/tools/rules/security-baseline` | 1.0.0 |

## Scenarios

### 1. Browse the catalog

```sh
grim search                       # whole catalog
grim search review                # filter by keyword/description
grim search --format json
grim tui                          # interactive (requires a TTY)
```

### 1a. TUI: multi-select, batch, scope, delete

`grim tui` (needs a TTY). Each row shows a colored state glyph:
`✓ installed` (green), `↑ outdated` (yellow), `✱ modified` (red),
`✘ integrity-missing` (magenta — recorded but files gone/edited away),
`· not-installed` (grey).

| Key | Action |
|-----|--------|
| `↑`/`↓` | move selection — the detail pane always follows it |
| `pgup`/`pgdn` | scroll the detail pane from anywhere (there is no focus to take) |
| `t` | toggle between tree view and flat list view |
| `→` | expand the selected group in tree view |
| `←` | collapse the selected group in tree view |
| `Enter` on group | fold/unfold group (on a leaf: fetch its companion immediately, skipping the idle-settle wait) |
| `space` | mark/unmark the selected row; on a group: mark all descendant leaves |
| `a` / `c` | mark all visible / clear marks |
| `i` / `u` / `d` | install / update / **uninstall** the marked set (or the selection if nothing marked); on a group with no marks: acts on the whole subtree |
| `o` | open the selected entry's repository URL in the browser |
| `g` | toggle scope (project ⇄ global) — title shows the active scope |
| `/` | search; `tab` switches detail panel (`j`/`k` scroll it); `r` refresh catalog; `q` quit — `esc` quits on the first press |

Try: mark a couple with `space`, press `i` (batch install), watch the
state glyphs flip to green; `d` to batch-uninstall; `g` to see the same
catalog against the global scope's state. Tamper a file
(`echo x >> test/manual/project/.claude/skills/hello-world/SKILL.md`)
then refresh — it shows `✱ modified`; delete the dir and it shows
`✘ integrity-missing`.

The detail pane is always live for the selection. It shows the centered
identifier, a `Summary:` /
`Description:` section, and a `Metadata:` block (version + status stay on
the catalog row). Most rig artifacts carry an authored `repository` URL
(`https://github.com/grimoire-samples/…`, emitted as the
`org.opencontainers.image.source` annotation) — `o` opens it.
`hello-world` and `security-baseline` intentionally carry none, so they
demo the `Repository: -` fallback and the "no repository URL for this
entry" status line. The `architecture-guide` **skill** ships a
deliberately long description so its pane overflows a small terminal —
open it and scroll (`↑`/`↓` or `j`/`k`), or page it from the list with
`pgup`/`pgdn` without opening it at all.

**Tree view walkthrough**: run this one with a single registry forced so the
root elides — `grim tui --registry localhost:5050/grimoire`. (Bootstrap now
writes a global two-registry config, and global `[[registries]]` union into
every project's browse set, so a bare `grim tui` from `project/` resolves
BOTH registries and shows two roots. Only the `--registry` flag collapses the
browse to exactly one, which is what makes the lone root elide.) Press `t` to
switch from the flat list to tree mode. The registry host
(`localhost:5050/grimoire`) becomes the root node and is elided from display
because exactly one registry resolves; children group by path segment
(`skills`, `rules`, etc.). Press `→` on a group to expand it, `←` to collapse,
`Enter` to toggle.
Try `space` on the `skills` group — every descendant leaf gets
marked; the group glyph turns filled. Press `i` to batch-install the whole
subtree. Press `t` again to return to the flat list — marks survive the
toggle. Add `tree_separators = ["/", "-"]` to `test/manual/project/grimoire.toml`
under `[options.tui]` to see `code-reviewer` and `commit-helper` split
further at the hyphen.

Note the `playbooks/ci/release` node sitting beside `skills`/`rules`: it is a
single **folded** node, not three nested groups. `cut-release` is the only
package in that chain, so `playbooks` → `ci` → `release` (each a single-child
group) join into one row — the "longest empty prefix" / compact-folders
fold. Expand it (`→`) and the lone `cut-release` leaf sits directly under the
joined node. This is the dedicated check in scenario 1c.

### 1b. Deprecated package highlight (issue #15)

`old-reviewer` ships `metadata.deprecated`, published as the
`com.grimoire.deprecated` annotation. The notice surfaces on all three
discovery/acquisition paths:

```sh
grim search old-reviewer                  # plain: Status cell reads "...,deprecated"
grim search old-reviewer --format json    # JSON: a "deprecated" field carries the message
grim tui                                   # yellow "⚠ deprecated" after the status label in the Status column; detail pane (enter) shows Deprecated:
```

Acquiring the reference warns on stderr (the add still succeeds):

```sh
cd test/manual/project
grim add localhost:5050/grimoire/skills/old-reviewer:1
# stderr: "...old-reviewer:1 is deprecated: superseded by code-reviewer — migrate before the next release"
```

A current package (e.g. `code-reviewer`) carries no marker, a `null`
`deprecated` JSON field, and warns on neither search nor add — the contrast
is the point.

### 1c. TUI tree-fold ("longest empty prefix") check

`cut-release` is published ALONE under the deep path
`localhost:5050/grimoire/playbooks/ci/release/cut-release`. It is the only
package in that chain, so each of `playbooks`, `ci`, `release` has exactly one
child — the tree joins them into ONE folded node (VS Code "compact folders").
The namespace level directly above the package is kept, so the lone
`cut-release` leaf is NOT absorbed.

`grim tui` (needs a TTY). Two ways to see it:

```sh
# A) Global two-registry config (bootstrap wrote it): two roots, fold under primary.
grim tui --global
#   localhost:5050/grimoire
#     playbooks/ci/release        <- ONE folded node (not three nested groups)
#       cut-release               <- the single leaf, kept above the package
#     skills/ rules/ agents/ bundles/   <- siblings, each branches (no fold)
#   localhost:5051/tools          <- second root, not elided

# B) Single registry forced: root elides, the fold node stands alone.
grim tui --registry localhost:5050/grimoire
```

Verify:

- **One folded node, not a chain** (issue #19): the row reads
  `playbooks/ci/release` as a single `/`-joined label, not three separate
  expandable groups. Press `→` once and the `cut-release` leaf appears directly
  beneath it.
- **The leaf is never absorbed**: the join stops at `release` because its only
  child (`cut-release`) is a leaf — `playbooks/ci/release/cut-release` would be
  wrong (one namespace level is always kept above a package).
- **Siblings don't fold**: `skills`, `rules`, `agents`, `bundles` each branch
  (2+ children), so they stay as their own single-segment groups next to the
  folded `playbooks/ci/release`.

### 2. Lock & install into a client

```sh
cd test/manual/project
grim lock                         # floating :1 -> pinned @sha256
cat grimoire.lock                 # byte-stable, digest-pinned
grim install                      # targets the detected clients (all when none detected)
ls -R .claude/skills .claude/rules .claude/agents
grim status                       # every artifact 'installed'
```

### 2a. Multi-registry: browse-all + `[[registries]]` alias resolution

`bootstrap.sh` publishes the primary catalog to `localhost:5050/grimoire`
and a small `tools` subset (`commit-helper`, `security-baseline`) to a
SECOND registry `localhost:5051/tools`. The `project-multi/` consumer
declares both with `[[registries]]`, so one search browses both and
fully-qualified refs across the two hosts lock and install together.
`GRIM_DEFAULT_REGISTRY` (set by `env.sh`) is the short-id default only —
it does not collapse the browse; only `--registry` does.

```sh
cd test/manual/project-multi
cat grimoire.toml                 # two [[registries]]: primary (5050), tools (5051)

grim search                       # browses BOTH 5050/grimoire AND 5051/tools
grim search commit-helper         # the 5051/tools copy surfaces here too

grim lock                         # pins each FQ ref to its own registry
grep -E 'pinned' grimoire.lock    # hello-world @5050; commit-helper,
                                  #   security-baseline @5051
grim install && grim status       # all 'installed' from across both registries
```

Alias resolution is a `grim add` CLI convenience (the leading segment is the
alias, the rest is appended to its `url`), not a persisted config form:

```sh
# 'tools' -> localhost:5051/tools, so this resolves to
#   localhost:5051/tools/skills/commit-helper:1
grim add tools/skills/commit-helper:1
```

### 2b. Multi-registry TUI: registry-tree projection

The TUI browses every declared `[[registries]]` in one session, grouping by
registry. With `env.sh` sourced (`GRIM_DEFAULT_REGISTRY` set), both roots
still appear — the env var does not collapse the browse. Run it from the
multi-registry project (needs a TTY):

```sh
cd test/manual/project-multi
grim tui                          # interactive only — the TUI needs a real TTY
```

Verify (each maps to a design decision — see the plan / ADR):

- **Two registry roots, not elided** (D-ELIDE): with two registries resolved,
  neither host is elided — the tree shows BOTH `localhost:5050/grimoire` and
  `localhost:5051/tools` as top-level roots. (Elision only kicks in when
  *exactly one* registry resolves — that is the single-registry walkthrough in
  scenario 1a, where the lone root is hidden.)
- **Precedence order, not alphabetical** (F13): the `default = true` primary
  (`localhost:5050/grimoire`) sorts FIRST, `tools` (`localhost:5051/tools`)
  second — declaration/resolution order. Deeper levels stay alphabetical.
- **Namespaced roots stay distinct** (D-TREE): the roots are the full
  `host/namespace` (`…:5050/grimoire`, `…:5051/tools`), never collapsed under a
  bare `localhost`. `commit-helper` appears under BOTH roots — one copy per
  registry.
- **Cross-registry batch**: expand both roots, `space` on each registry group to
  mark all descendants, `i` to install — each package installs to its own
  registry and the glyphs flip green under both roots.
- **Empty / offline registry still shows as a root** (D-EMPTY + D-DEGRADE):
  stop the second registry (`docker stop grim-manual-registry-2`, or
  `docker compose -f test/manual/docker-compose.yml stop registry-2`), then `r`
  to refresh — the `tools` root still renders (as a `0/0` root) and the status
  line reports `offline: localhost:5051/tools`. Restart the container + `r` to
  restore it.
- **Status line clears after refresh** (regression guard): press `r`; once the
  reload finishes the transient `refreshing catalog…` message must CLEAR — it
  must not stay stuck. With everything healthy the status falls through to the
  health line (empty) or the marked-count hint.
- **Scope toggle re-elides** (D-ELIDE on `g`): press `g` to switch scope. A
  scope that declares a single registry re-elides its root; a scope with
  multiple registries keeps every root. The root set updates live.
- **Namespaced bundle members resolve correctly** (B1c): expand a bundle that
  lives in a namespaced registry — its members show their true install state
  (e.g. `✓`/`via bundle`), not a stale `· not-installed`. They are matched
  against the full `host/namespace`, never a first-`/` split to bare `localhost`.
- **Flat list shows Registry column** (feature A): press `t` to switch to flat
  view. With two registries resolved the table gains a leading **Registry** column
  showing each row's registry display label (alias or URL) and the Repo cell
  shortens to the registry-relative path. Switch back to single-registry project
  and verify the column is absent (single-registry elision unchanged).
- **Alias-based labels in tree roots and health line** (feature B): set
  `alias = "tools"` on the `localhost:5051/tools` entry in
  `test/manual/project-multi/grimoire.toml`, then `r` to refresh. The
  `localhost:5051/tools` tree root must now read `tools (localhost:5051/tools)`.
  Take the second registry offline — the status line must say `offline: tools
  (localhost:5051/tools)` instead of the raw URL.

### 3. Multi-client transform (Copilot rule transform)

```sh
grim install --client claude,copilot
cat .github/instructions/rust-style.instructions.md
# note: `paths:` frontmatter stripped + provenance header prepended
```

### 4. Integrity protection

`install` and `update` run the **same** gate — that is the point of this
scenario. Before 0.13.0 `update` forced unconditionally and silently ate the
edit, so check both:

```sh
echo "tampered" >> .claude/skills/hello-world/SKILL.md
grim status                       # hello-world -> 'modified'
grim install                      # refused (exit 65) — local edit protected
grim update                       # ALSO refused (exit 65), same error
tail -1 .claude/skills/hello-world/SKILL.md   # still "tampered"
grim install --force              # overwrite the local edit
```

`grim update --force` overwrites it just the same. What must never happen
again: a plain `grim update` exiting 0 with the edit gone.

The TUI carries the same gate, and this is the part only a human can check:

```sh
echo "tampered" >> .claude/skills/hello-world/SKILL.md
grim tui
```

Select the `hello-world` row and press **`u`** (update). Expect the
**Overwrite** confirmation dialog — the same one `i` (install) has always
shown — quoting `installed artifact was modified locally`. Choosing Overwrite
re-runs the action forced; cancelling leaves the edit intact. Before 0.13.0
`u` silently overwrote it and the dialog was unreachable.

On a **bundle member** node the dialog does not open (a member has no row
index for the retry to address). The member shows
`refused (locally modified)` plus a status line naming
`grim install --force` as the remedy. That gap is known and pre-existing;
verify the status line appears, not a dialog.

### 4a. Materialization drift (`outputs_pending`)

The drift no other field can see: the artifact is byte-intact at its locked
pin, but a client that appeared *after* the install has nothing recorded, so
an install still has files to write.

```sh
# in test/manual/project, with hello-world installed into .claude only:
grim status --format json | jq '.items[] | select(.name=="hello-world")
  | {state, outputs_pending}'          # state installed, outputs_pending []

mkdir -p .cursor                        # "install" a second client
grim status --format json | jq '.items[] | select(.name=="hello-world")
  | {state, outputs_pending}'          # state STILL installed; pending names cursor
grim tui                                # the row badge reads '+ pending'

grim install                            # the remedy (NOT update)
grim status --format json | jq '.items[] | select(.name=="hello-world")
  | .outputs_pending'                  # back to []
```

Check the promise holds both ways: whatever `outputs_pending` named must be
on disk after `grim install`, and the field must be `[]` again.

### 5. Rolling release / outdated / update

`bootstrap.sh` publishes `code-reviewer` ascending to 1.2.0, so locking the
floating `:1` records 1.2.0 (state `installed`, NOT `outdated`). To produce
a genuine `↑ outdated` lock, publish a version ABOVE the matrix top AFTER
locking — that is exactly what `release-update.sh` does (1.3.0):

```sh
# in test/manual/project, after `grim lock` (code-reviewer pinned at 1.2.0):
grep code-reviewer grimoire.lock          # 1.2.0 digest
../scripts/release-update.sh              # publishes code-reviewer 1.3.0, moves :1
grim status                               # code-reviewer -> 'outdated'
grim update                               # re-resolves :1 -> 1.3.0
grep code-reviewer grimoire.lock          # digest advanced
grim status                               # back to 'installed'
```

### 5a. Bundle add/remove on upgrade

The `starter-pack` bundle ships two versions with different member sets, so
upgrading `:1 -> :2` adds AND removes members:

```sh
# v1: code-reviewer + rust-style + security-baseline
# `add` infers kind=bundle from the published manifest's artifactType
grim add localhost:5050/grimoire/bundles/starter-pack:1
cat grimoire.toml grimoire.lock           # inspect the resolved members

# v2 ADDS commit-helper, DROPS security-baseline
grim add localhost:5050/grimoire/bundles/starter-pack:2
grim update                               # commit-helper added, security-baseline pruned
cat grimoire.toml grimoire.lock
```

### 5b. Shared bundle members

`starter-pack` and `review-pack` both declare `code-reviewer` at the same
identifier, so declaring both coalesces it to ONE lock entry that records
BOTH bundles as provenance. Removing one bundle strips only that bundle's
provenance entry — the member survives until the last holder goes.

Run this in a **scratch project**: the rig's ready-made project declares
every bundle member directly, and a direct declaration always wins over
bundle provenance (you would see `direct`, not `bundle: …`).

```sh
mkdir -p /tmp/grim-shared-demo && cd /tmp/grim-shared-demo
grim init
grim add localhost:5050/grimoire/bundles/starter-pack:1
grim add localhost:5050/grimoire/bundles/review-pack:1
grim status                       # code-reviewer source: "bundle: ...starter-pack, ...review-pack"
grep -B3 -A3 'skill.bundles' grimoire.lock    # multi-provenance [[skill.bundles]] rows

grim remove bundle review-pack
grim status                       # code-reviewer still locked (held by starter-pack)
grim remove bundle starter-pack
grim status                       # now gone — the last holder was removed
```

The same holds in the TUI: deleting one of the two bundle rows keeps the
shared member's files on disk; only members the deleted bundle exclusively
owns are uninstalled.

### 5c. Agents (per-client rendering + vendor overrides)

The project declares the `reviewer` agent; `release-bot` carries
vendor-namespaced metadata (`claude.model: opus`,
`claude.permission-mode: plan`, `opencode.temperature: "0.2"`) that
overrides or extends the projected common fields per client:

```sh
cd test/manual/project
grim add localhost:5050/grimoire/agents/release-bot:1
grim install --client claude,opencode,copilot

cat .claude/agents/release-bot.md     # claude.model override: model: opus (+ permissionMode)
cat .opencode/agents/release-bot.md   # common model: sonnet kept; temperature lifted; no name:
cat .github/agents/release-bot.md     # tools: as a YAML list; no model
cat .claude/agents/reviewer.md        # common fields only -> installed verbatim
```

### 6. add / remove

```sh
grim add localhost:5050/grimoire/skills/hello-world:1
grim remove skill commit-helper
cat grimoire.toml grimoire.lock
```

### 7. Global scope

Global scope writes into each client's own directory under your home, so every
global command in this rig runs through `in_rig`. It hands grim this rig's
paths and nothing else:

- **`GRIM_HOME`.** `in_rig global` takes `.grim-home/`, where `bootstrap.sh`
  wrote the global config. `in_rig settings` takes `.settings-roots/grim-home/`,
  which only § 13e uses, so a package § 13e declares never reaches this section.
- **Every other root.** `HOME` is `.settings-roots/home/`, and the Claude, Codex
  and Copilot roots are the ones `settings-env.sh` makes in `.settings-roots/`.
  grim runs under `env -i` with only these and the registry variables, so no
  other client variable reaches it.
- **Refusals.** It exits 1 before grim starts when a client root in this shell
  is not the rig's, compared after symlinks and `..` resolve. It also refuses
  when a Claude settings file could move Claude's root: managed settings, or a
  `CLAUDE_CONFIG_DIR` in the rig's own `settings.json`.

Managed settings are `managed-settings.json` or `managed-settings.d/` under
`/etc/claude-code` on Linux and `/Library/Application Support/ClaudeCode` on
macOS. On any other platform `in_rig` refuses, since it has no path to check.
The TUI's global scope (`g` in § 1a, `grim tui --global` in § 1c) runs without
this guard: browse there, and install at global scope only through `in_rig`.

Define it once per shell, from `project/` as in § 2 (§ 13e runs the same block
from `project-settings/`):

```sh
source ../scripts/settings-env.sh
grim_rig="$(cd -P .. >/dev/null && pwd -P)"         # test/manual, symlinks resolved
in_rig() (
  case "${1:-}" in
    global) home="$grim_rig/.grim-home" ;;
    settings) home="$grim_rig/.settings-roots/grim-home" ;;
    *) echo "usage: in_rig global|settings <command>..." >&2; exit 2 ;;
  esac
  shift
  roots="$grim_rig/.settings-roots"
  refuse() { printf 'refusing to run: %s\n' "$*" >&2; exit 1; }
  at() {
    case "$1" in /*) [ "$(cd -P -- "$1" >/dev/null 2>&1 && pwd -P)" = "$roots/$2" ] && return ;; esac
    refuse "'$1' does not resolve to $roots/$2; source scripts/settings-env.sh"
  }
  [ -f "$grim_rig/scripts/settings-env.sh" ] || refuse "'$grim_rig' is not test/manual"
  at "${CLAUDE_CONFIG_DIR:-}" claude
  at "${CODEX_HOME:-}" codex
  at "${COPILOT_HOME:-}" copilot
  case "$(uname -s)" in
    Linux) managed=/etc/claude-code ;;
    Darwin) managed="/Library/Application Support/ClaudeCode" ;;
    *) refuse "no Claude managed-settings path known for $(uname -s)" ;;
  esac
  if [ -e "$managed/managed-settings.json" ] || [ -e "$managed/managed-settings.d" ]; then
    refuse "Claude managed settings in '$managed' can move Claude's root"
  fi
  if grep -qs CLAUDE_CONFIG_DIR "$roots/claude/settings.json"; then
    refuse "'$roots/claude/settings.json' names CLAUDE_CONFIG_DIR"
  fi
  mkdir -p "$home" "$roots/home" || exit 1
  env -i PATH="$PATH" HOME="$roots/home" GRIM_HOME="$home" \
    GRIM_DEFAULT_REGISTRY="${GRIM_DEFAULT_REGISTRY:-}" GRIM_INSECURE_REGISTRIES="${GRIM_INSECURE_REGISTRIES:-}" \
    CLAUDE_CONFIG_DIR="$roots/claude" CODEX_HOME="$roots/codex" COPILOT_HOME="$roots/copilot" "$@"
)
```

`in_rig` is a subshell, so nothing it sets reaches your shell. `bootstrap.sh`
already wrote the global config (`.grim-home/grimoire.toml` with the
registries), so skip `grim --global init`: it exits 64, `config already exists`.
Add and install:

```sh
in_rig global grim --global add localhost:5050/grimoire/rules/security-baseline:1
in_rig global grim --global install
find ../.settings-roots -type f                    # every client file grim wrote, all in the rig
```

### 8. Offline behavior

```sh
GRIM_OFFLINE=1 grim search        # serves cached catalog, exit 0
GRIM_OFFLINE=1 grim install       # warm blob cache succeeds; cold -> exit 81
```

### 9. Annotations, provenance, and support channels

`support-desk` is the one artifact published with `grim publish` rather than
`grim release`, because only the batch command authors a **description
companion** — and the companion is where the repository's support channels
live. Everything else on it is authored in its `SKILL.md`.

```sh
# Every curated field at once. `revision`/`created` are derived from the
# publishing commit with NO flag: default-on since #106.
grim describe localhost:5050/grimoire/skills/support-desk --format json |
  jq '{license, created, revision, authors, vendor, url, documentation,
       compatibility, support}'
```

Expect `authors: "Grimoire Platform Team"` (the SKILL.md value, **not** the
`[metadata]` fallback in `publish.toml` — frontmatter outranks it), and a
populated four-field `support` object.

```sh
grim describe localhost:5050/grimoire/skills/hello-world --format json |
  jq '{authors, vendor, support}'      # vendor derived ("grimoire"), support all null
```

**Browse surfaces carry less, on purpose:**

```sh
grim search support-desk --format json | jq '.items[0].oci'
```

The `grim search` row carries every version-scoped field but **no** support
channels: the browse catalog is disk-cached, and a cached contact link is one
that may already have moved.

The TUI is different, because it fetches the companion live once the selection
settles (or immediately on `enter`):

```sh
grim tui                              # enter on support-desk
```

Arrow onto `support-desk` and **wait a beat** — the companion fetch fires when
the selection holds still, not on a keypress. The detail pane gains `License:`,
`Authors:`, `URL:`, `Documentation:`, `Vendor:`, `Compatibility:`, `Revision:`,
and `Created:` rows, plus a `Support:` section with all four channels. The
panel strip lives in the pane's top border:

```
──Overview──Readme──Changelog─────────────────────
```

`tab` / `shift-tab` cycle it; `support-desk` publishes both documents, so all
three labels are lit.

Compare with `hello-world`, which publishes no companion at all: the strip is
**still there** with all three labels — `Readme` and `Changelog` greyed — and
landing on one reads `not available`. That is the point of the fixed strip:
`tab` does the same thing on every row, and the pane never resizes as a fetch
lands.

`esc` quits on the first press from anywhere — there is no detail focus to
back out of first.

Worth checking by hand, since none of it is reachable from an acceptance test:

- `GRIM_OFFLINE=1 grim tui` — every catalog field intact, and the `Support:`
  section reads `not available — offline` rather than silently vanishing.
- A narrow terminal (the stacked layout) — the strip must not break the split.
- `r` after editing `skills/support-desk/README.md` and re-running
  `bootstrap.sh` — the Readme tab must show the new text, since the companion
  cache is cleared by a catalog reload.

**The mutability demo** — the point of putting support on the companion:

```sh
grim describe localhost:5050/grimoire/skills/support-desk --format json | jq -r '.digest'
$EDITOR test/manual/catalog/publish.toml      # change [support].chat
test/manual/scripts/bootstrap.sh              # re-seed
grim describe localhost:5050/grimoire/skills/support-desk --format json |
  jq '{digest, support}'
```

The `support.chat` value changes; the artifact `digest` does **not**. No
version bump, no re-release, and every already-published tag reports the new
link.

**The provenance disclosure guard:**

```sh
# Default: revision + created present, but NO source URL and NO commit author.
grim build test/manual/catalog/skills/hello-world --format json

# --no-git suppresses every derived value; --git in a non-repo exits 65.
grim build test/manual/catalog/skills/hello-world --no-git --format json
(cd /tmp && grim build "$OLDPWD/test/manual/catalog/skills/hello-world" --git); echo "exit=$?"
```

`--git` is what opts *in* to publishing the `origin` remote and the commit
author's name — the two values that name internal infrastructure and a
person. Neither is ever derived by default.

### 10. Ratings and download counts (the `stats.json` sidecar)

**Only the static index carries these.** Sidecar stats ride the HTTP index
transport — an OCI `_catalog` browse has no `stats.json` to fetch — so the
`index` service on port 5052 is the one source in the rig whose rows are rated
and counted. Both registries are OCI; without it every row is permanently
unrated and uncounted.

```sh
grim search --format json | jq '.items[] | select(.rating or .downloads)
  | {repo, up: .rating.up, pulls: .downloads.total, as_of: .downloads.as_of}'
grim tui                        # detail pane: `Rating:` and `Downloads:` rows
```

What to look for, and the absences that matter as much as the figures:

| Row | Check |
|---|---|
| `skills/code-reviewer` | Both signals. `Downloads: 1416 (as of 2026-09-09)` in the TUI; the extension's DOWNLOADS rail adds the per-release rows. The breakdown sums to 1400 — `total` is **not** the sum, the other 16 came through a channel tag naming no release. |
| `skills/support-desk` | Counted, unrated — 250412, large enough to exercise the extension's compact badge (`250K`). |
| `skills/commit-helper` | Rated, **uncounted**: `downloads` is `null`, and no `Downloads:` row appears. This is what every GHCR- or GitLab-backed row looks like. |
| `agents/reviewer` | A measured **zero**. It renders `0` and must not read the same as the row above — `null` is unknown, `0` is "counted, nobody pulled it". |
| `rules/rust-style` | A count with no `as_of`: the figure shows, the "as of" clause does not. |
| `rules/security-baseline` | A stamp three months behind the sidecar's `generated_at`, so the date is visibly not today. |
| `localhost:5051/tools/...` | An index serves rows from arbitrary hosts; the join keys on the full ref. |
| everything else | Neither signal — the common case, and there should be plenty of it. |

The fixture is served straight off disk, so editing
[`index/stats.json`](./index/stats.json) needs no restart — just
`grim search --refresh`. Row-by-row rationale in
[`index/README.md`](./index/README.md).

### 11. Hooks — arming, then the dispatcher

Hooks are the only kind grim declines on **policy** rather than capability, so
this scenario is mostly about arming. `bootstrap.sh` already published both;
nothing below is automatic. Every command here was run against this rig.

**a. Declaring a hook does not arm it.** `add` and `lock` behave like any other
kind, and `install` skips with one warning naming the single cause:

```sh
cd test/manual/project
grim add localhost:5050/grimoire/hooks/tool-logger:1
grim install                          # WARN … hooks are gated; …  exit 0
grim status --format json | jq -c '.items[] | select(.kind=="hook") | {name, state, arming}'
```

```
{"name":"tool-logger","state":"gated","arming":[{"client":"claude","cause":"feature-flag-off","message":"hooks are disabled for this scope; …","transient":false}]}
```

**Branch on `cause`, never on `message`** — the enum is frozen, the text is not.

> `grim add` rewrites `grimoire.toml` through a lossy re-serializer, so it
> **drops this file's comments** (a known limitation — `toml_edit` is a deferred
> upgrade). Restore the annotated original with
> `git checkout -- test/manual/project/grimoire.toml` when you are done, and do
> not commit the rewritten one: step (a) depends on the flag starting off.

**b. Gate two: the feature flag, then workspace consent.** Two independent
gates, per `adr_hook_workspace_consent.md` — the flag turns the *feature* on,
consent says *this workspace* may arm what it declares:

```sh
grim config set options.experimental.hooks true
grim install                          # still gated: cause "workspace-not-consented"
grim hook allow                       # writes $GRIM_HOME/hooks/consent/<workspace-key>.json
grim install                          # now it arms: "updated  claude (observer)"
grim hook list
```

**Why two gates, and what each answers.** The flag has no environment form and
no per-registry shape — it is a plain config-only boolean,
`options.experimental.hooks`. Consent is **workspace-keyed, not
registry-keyed**: the record names this checkout by absolute path and lists
the hook set it covers as `<binding>@<registry>/<repository>` (no tag, no
digest), so a cloned `grimoire.toml` declares hooks and arms nothing until
someone at the keyboard runs `grim hook allow` here. `--trust-hooks` on
`install`/`update`/`add` answers the same question for one invocation only and
writes no record.

**c. The dispatcher.** `$GRIM_HOME/hooks/grim-hook` is invoked by the client's
registration, not by you — but driving it by hand shows the envelope and the
verdict with no client in the loop:

```sh
TABLE="$GRIM_HOME/hooks/dispatch.json"
ROOT=$(jq -r '.roots | keys[0]' "$TABLE")
jq -c '.roots[].hooks[] | {artifact,id,matcher,handler}' "$TABLE"
```

```
{"artifact":"tool-logger","id":"log-tool-calls","matcher":"Bash|Write","handler":{"argv":["sh","${GRIM_HOOK_DIR}/log.sh"]}}
```

Fire it for each alternative and for a tool the matcher excludes:

```sh
for tool in Bash Write Read; do
  echo "{\"hook_event_name\":\"PreToolUse\",\"tool_name\":\"$tool\",\"tool_input\":{\"command\":\"ls\"},\"cwd\":\"$PWD\",\"session_id\":\"s-1\"}" \
    | "$GRIM_HOME/hooks/grim-hook" claude PreToolUse "$ROOT" >/dev/null
done
cat "$GRIM_HOME"/hooks/payload/*/tool-calls.log
```

```
PreToolUse	claude	Bash	547 bytes
PreToolUse	claude	Write	549 bytes
```

Two lines, not three: `Bash|Write` is an alternation matcher and `Read` is in
neither alternative. Note where the log landed — **beside** the payload tree,
not inside it. `GRIM_HOOK_DIR` is the materialized artifact tree whose content
hash `grim status` compares, so a payload that writes into it would report
itself locally modified on the next `grim status`.

**d. A gatekeeper's refusal travels as a document, not an exit code.**

```sh
grim add localhost:5050/grimoire/hooks/write-guard:1 && grim install
echo "{\"hook_event_name\":\"PreToolUse\",\"tool_name\":\"Write\",\"tool_input\":{\"file_path\":\".env\"},\"cwd\":\"$PWD\",\"session_id\":\"s-2\"}" \
  | "$GRIM_HOME/hooks/grim-hook" claude PreToolUse "$ROOT"
echo "exit=$?"
```

```
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"write-guard (a manual-rig demonstration hook) refused a Write mentioning .env"}}
exit=0
```

The payload wrote `{"decision":"deny","reason":"…"}`; grim projected it onto
Claude's own response shape. A verdict never travels as an exit code, and the
launcher never exits `2`, which is Claude's own deny code.

**e. The audit trail answers "did the guardrail apply here?"**

```sh
tail -3 "$GRIM_HOME/hooks/hook_audit.jsonl" | jq -c '{hook_id, outcome, verdict}'
```

```
{"hook_id":"log-tool-calls","outcome":"completed","verdict":"noopinion"}
{"hook_id":"log-tool-calls","outcome":"completed","verdict":"noopinion"}
{"hook_id":"log-tool-calls","outcome":"no-match","verdict":null}
```

The `no-match` record is the point: "this hook did not apply to that call" has
to stay answerable, so a decline is recorded rather than passed over silently.

**f. Things that should NOT work.** Each runs nothing:

```sh
P="{\"hook_event_name\":\"PreToolUse\",\"tool_name\":\"Write\",\"tool_input\":{},\"cwd\":\"$PWD\",\"session_id\":\"s3\"}"

# A well-formed root token no table holds is indistinguishable from an absent
# one: exit 0, nothing spawned.
echo "$P" | "$GRIM_HOME/hooks/grim-hook" claude PreToolUse ffffffffffffffffffffffffffffffff; echo "exit=$?"

# A malformed argv is a usage error: exit 64, one usage line on stderr.
echo "$P" | "$GRIM_HOME/hooks/grim-hook" claude PreToolUse deadbeef; echo "exit=$?"
```

The table is always the `dispatch.json` beside the launcher, whatever the
working directory or environment says, so a repository cannot supply its own.

**g. Revoking.** `grim hook revoke` (alias `deny`) withdraws the workspace's
consent record; the next converging command disarms — there is no runtime
kill switch, only a deferred one:

```sh
grim hook revoke                      # removes $GRIM_HOME/hooks/consent/<workspace-key>.json
grim install                          # both hooks skipped
grim status --format json | jq -c '[.items[] | select(.kind=="hook") | {name, state}]'
grim uninstall hook tool-logger       # reaps the dispatch row AND the registration
```

```
[{"name":"tool-logger","state":"gated"},{"name":"write-guard","state":"gated"}]
```

The reported `cause` is `workspace-not-consented` — the same cause a checkout
that never ran `grim hook allow` reports, because revoked and never-granted are
the same state on purpose (there are three consent states, not four: absent,
granted, drifted). Restore the rig with `grim hook allow`.

> **A grim hook is defence in depth, never a security boundary.** `write-guard`
> is a demonstration: its check is a substring match that any rewording evades,
> and every layer of this design fails open so a broken guardrail can never deny
> you a tool call. Copy the shape, never the check.

## 12. Dependencies (declared `requires`)

The catalog ships artifacts that declare what they need. `bootstrap.sh` builds
the branch `grim` into `test/bin/grim` (rebuilt when `src/**/*.rs` is newer), so
the rig always runs the requires-aware build.

| Artifact | Requires |
|---|---|
| skill `pr-assistant` | mcp `docs-search`, rule `security-baseline` |
| agent `triage-bot` | skills `code-reviewer`, `commit-helper` |
| rule `commit-conventions` | skill `commit-helper` (diamond with `triage-bot`) |
| bundle `dev-suite` | `pr-assistant` + `triage-bot`, each with its own closure |
| skill `conflict-demo` | `commit-helper:2` (clashes with `:1`) |
| skills `cycle-a` / `cycle-b` | each other |

`project-deps/` declares only `pr-assistant`, `triage-bot` and
`commit-conventions`.

```sh
source test/manual/scripts/env.sh
cd test/manual/project-deps
grim lock && grim install          # also installs docs-search, security-baseline, code-reviewer, commit-helper
grim status                        # Source column: "direct" vs "dependency: <required_by>"
grim status --format json          # "direct": false + "required_by": [...]
grim why skill commit-helper       # two chains: the diamond merged into one install
grim why mcp docs-search           # skill/pr-assistant -> mcp/docs-search
grim describe localhost:5050/grimoire/skills/pr-assistant:1   # requires row

# Removing a dependent removes what only it needed.
grim remove skill pr-assistant     # security-baseline + docs-search leave; commit-helper stays
grim add localhost:5050/grimoire/skills/pr-assistant:1        # "pulled in 2 dependencies"

# Nested closure: bundle -> member -> member's dependency.
grim add localhost:5050/grimoire/bundles/dev-suite:1
grim why skill pr-assistant        # direct, and via bundle/dev-suite
grim remove bundle dev-suite       # members stay: they are declared directly

# Conflict (exit 78): triage-bot wants commit-helper:1, conflict-demo wants :2.
grim add localhost:5050/grimoire/skills/conflict-demo:1; echo $?
# Cycle (exit 65): cycle-a -> cycle-b -> cycle-a.
grim add localhost:5050/grimoire/skills/cycle-a:1; echo $?
```

Failed adds leave `grimoire.toml` untouched. In `grim tui` (run it in
`project-deps/`), a dependency's detail pane has a **Required by** row. The
local index (`index/all.json`) lists the new artifacts too; browse them with
`grim tui --global`. That index is static JSON served by nginx, not an indexer
site, so it has no web details page.

**VS Code:** the extension runs the `grim` it finds on its own `PATH`, which
is not `env.sh`'s. A released grim (0.15.x and older) cannot read a requires
layer, so the Dependencies panel stays empty and installing a dependent fails
with exit 65. Point `grimoire.path.executable` at the absolute path of this
checkout's `test/bin/grim` (the setting takes no `${workspaceFolder}`), then
open `project-deps/`.

## 13. Settings (experimental)

A `settings` package carries harness configuration (a model, permission rules,
a plugin marketplace) and merges it into the file each client already reads.
The kind is off by default and needs two answers before it writes anything: the
feature flag (`[options.experimental] settings = true`) and this workspace's
consent. `bootstrap.sh` published `team-defaults`; its source is
`catalog/settings/team-defaults.toml`. It sets, for Claude, a `model` (Scalar),
`permissions.allow` and `permissions.deny` (Set) and an `acme` marketplace
(Map), plus the same marketplace for Codex and Copilot.

**Where it writes.** Parts a-d stay at project scope: the only file written is
`project-settings/.claude/settings.local.json`, inside the rig (gitignored,
removed by `teardown.sh`). Codex and Copilot apply at global scope only, so part
e runs each global command through `in_rig settings`, the § 7 guard. grim then
sees only the rig's client roots, a scratch `HOME` and part e's own `GRIM_HOME`.
A `grim --global` typed without `in_rig` has no such guard: it writes wherever
the shell's variables point, which is `~/.claude`, `~/.codex` and `~/.copilot`
when `settings-env.sh` was not sourced.

```sh
source test/manual/scripts/env.sh
cd test/manual/project-settings
grim lock
```

**a. The flag is committed, consent is not.** This `grimoire.toml` already turns
the flag on, as a team would. A fresh clone is still gated. With no terminal to
ask on, `install` writes nothing and exits 0, and `status` names the cause:

```sh
grim install </dev/null; echo $?                  # 0, nothing written
ls .claude/settings.local.json                    # No such file
grim status --format json | jq -c '.items[] | select(.kind=="settings") | {name, state, arming}'
```

Expect `state` `gated` and one `arming` cause, `workspace-not-consented`.
Branch on `cause`, never on `message`. In a terminal, a plain `grim install` asks
the question instead (`[y/N]`). `y` records consent for this checkout, as
`grim consent allow` in part b does, so later installs apply without asking;
`n` writes nothing. Only `--trust-workspace` answers for one run and writes no
record. `grim add` also counts as the user's own consent, which is why
`grim add <ref>` in a project with the flag on installs at once.

**b. Consent, then install.** `grim consent allow` (alias of `grim hook allow`)
records the answer for this checkout under `$GRIM_HOME/hooks/consent/`:

```sh
grim consent allow
grim install
cat .claude/settings.local.json
```

`install` lists every key it changed on stderr, one line per key, as
`added claude .claude/settings.local.json /model (team-defaults)`, and closes with
the note `a running client may need a restart to read its new settings`. Expect
`/model`, one `/permissions/allow` element, one `/permissions/deny` element
and `/extraKnownMarketplaces/acme`. The same list is `settings_changes` in
`--format json`; it carries keys and elements, never values. This project
selects `clients = ["claude"]`, so grim reads only the `claude` table and says
nothing about the others. Name Codex as well, `grim install --client claude,codex`,
and the run also warns `settings package 'team-defaults': codex applies at
global scope only`, followed by the cause `(no-applicable-settings)`; part e
applies that table.

**c. A refusal, then `--force`.** Hand-edit a value grim wrote, then install:

```sh
jq '.model = "opus"' .claude/settings.local.json > .claude/m.tmp && mv -f .claude/m.tmp .claude/settings.local.json
grim install; echo $?                            # 65, names /model
grim install --format json 2>/dev/null | jq '.error | {code, exit, reason, forceable, pieces: [.pieces[] | {pointer, state, wanted, current}]}'
cat .claude/settings.local.json                  # still "opus": nothing was written
grim install --force; echo $?                   # 0, "overwritten claude … /model"
```

The refusal is `exit` 65, `reason` `modified`, `forceable` true. `current` is the only
place the old value ever appears (bounded, control characters escaped); the
record, the other output and the change list never carry it. The same refusal
arrives after a teammate bumps the package or a pull changes a value: nothing a
teammate's `--force` did reaches this machine, only your own flag writes.

**d. Uninstall removes only what grim wrote.** Add entries of your own beside
grim's, then uninstall:

```sh
jq '.permissions.deny += ["Bash(curl:*)"] | .extraKnownMarketplaces.mine = {"source": {"source": "github", "repo": "me/own"}}' \
  .claude/settings.local.json > .claude/m.tmp && mv -f .claude/m.tmp .claude/settings.local.json
grim uninstall settings team-defaults
cat .claude/settings.local.json
```

The list is `removed …` per key. What stays: your `Bash(curl:*)` deny element and
your `mine` marketplace, in the same arrays and maps grim wrote into. Gone: the
`model`, grim's two rule elements, the `acme` entry, and any container that
only held grim's keys. A value that already equalled grim's before the first
install is adopted, not written, and uninstall leaves that one too.

**e. Global scope: Codex and Copilot.** Declare the package at global scope,
through the § 7 guard. Nothing here asks the workspace question. From
`project-settings/`, in the shell that sourced `env.sh`, run § 7's block that
defines `in_rig` (`..` is `test/manual` from here too), then:

```sh
in_rig settings grim --global config set options.experimental.settings true
in_rig settings grim --global config set options.clients claude,codex,copilot
in_rig settings grim --global add localhost:5050/grimoire/settings/team-defaults:1
cat "$CLAUDE_CONFIG_DIR/settings.json" "$CODEX_HOME/config.toml" "$COPILOT_HOME/settings.json"
```

The first `config set` creates part e's own global config,
`.settings-roots/grim-home/grimoire.toml`. If `source` failed, a variable still
names a real root, or Claude managed settings exist, every `in_rig` line prints
`refusing to run` and exits 1 before grim starts.

Claude and Copilot take the marketplace under `/extraKnownMarketplaces/acme`,
Codex under `/marketplaces/acme` (a `[marketplaces.acme]` table in
`config.toml`). Pre-seed any of the three files with an entry of your own before
the `add` to see it survive, then
`in_rig settings grim --global uninstall settings team-defaults` to take only
grim's keys back out.

Until that uninstall or `teardown.sh`, part e's global config declares
`team-defaults` for Claude, Codex and Copilot. Only `in_rig settings` reads it:
`env.sh` sets `GRIM_HOME` to `.grim-home/`, so § 7 and a new terminal never
apply the package. Keep every global command behind `in_rig` all the same, since
a bare `grim --global install` writes the real client directories.

Re-run `scripts/bootstrap.sh` after editing `team-defaults.toml`; it releases with
`--force` like the rest. Restore the demo state with `teardown.sh`, which also
removes `project-settings/.claude`, the lock and `.settings-roots/`, part e's
global config with it.

## Teardown

```sh
test/manual/scripts/teardown.sh             # wipe rig state, keep registry
test/manual/scripts/teardown.sh --registry  # also stop + remove the registry
```

Re-run `scripts/bootstrap.sh` any time to recreate from the committed catalog.
