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
| `catalog/hooks/<name>/hook.toml` | Source-of-truth sample hooks + their payload scripts (committed) |
| `catalog/bundles/starter-pack.toml` | Bundle v1 member set (committed) |
| `catalog/bundles/starter-pack-v2.toml` | Bundle v2 member set — adds + removes members (committed) |
| `catalog/bundles/review-pack.toml` | Bundle sharing `code-reviewer` with starter-pack + an agent member (committed) |
| `project/grimoire.toml` | Ready-made single-registry consumer project (floating `:1` tags) |
| `project-multi/grimoire.toml` | Multi-registry consumer project (`[[registries]]` aliases across 5050 + 5051) |
| `scripts/env.sh` | `source` it to point `grim` at the rig |
| `scripts/bootstrap.sh` | Build `grim`, start both registries, publish the version matrix + multi-registry subset + deep-fold solo package, write the global two-registry config |
| `scripts/release-update.sh` | Publish `code-reviewer` 1.3.0 (post-lock outdated / rolling-release demo) |
| `scripts/teardown.sh` | Wipe rig state (`--registry` also stops both registries) |
| `docker-compose.yml` | `registry:2` on `localhost:5050` (primary) and `localhost:5051` (`tools` subset) |
| `.grim-home/` | Isolated `GRIM_HOME` (gitignored, ephemeral) |
| `.grim-home/grimoire.toml` | Global config bootstrap writes via `grim config` — two `[[registries]]` (5050/grimoire default + 5051/tools); browse both from anywhere with `--global` |

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
| skill | `localhost:5050/grimoire/playbooks/ci/release/cut-release` | 1.0.0 (deep solo path — drives the tree-fold demo) |
| rule | `localhost:5050/grimoire/rules/rust-style` | 1.0.0, 1.1.0 |
| rule | `localhost:5050/grimoire/rules/security-baseline` | 1.0.0 |
| rule | `localhost:5050/grimoire/rules/architecture-guide` | 1.0.0 |
| agent | `localhost:5050/grimoire/agents/reviewer` | 1.0.0, 1.1.0 |
| agent | `localhost:5050/grimoire/agents/release-bot` | 1.0.0 (vendor-override demo) |
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
| `↑`/`↓` | move selection (scroll the detail pane while it is open) |
| `pgup`/`pgdn` | scroll the detail pane from any mode (no focus needed) |
| `t` | toggle between tree view and flat list view |
| `→` | expand the selected group in tree view |
| `←` | collapse the selected group in tree view |
| `Enter` on group | fold/unfold group (on a leaf: open detail pane) |
| `space` | mark/unmark the selected row; on a group: mark all descendant leaves |
| `a` / `c` | mark all visible / clear marks |
| `i` / `u` / `d` | install / update / **uninstall** the marked set (or the selection if nothing marked); on a group with no marks: acts on the whole subtree |
| `o` | open the selected entry's repository URL in the browser |
| `g` | toggle scope (project ⇄ global) — title shows the active scope |
| `/` | search; `enter` browse detail (`j`/`k` also scroll there); `r` refresh catalog; `q` quit |

Try: mark a couple with `space`, press `i` (batch install), watch the
state glyphs flip to green; `d` to batch-uninstall; `g` to see the same
catalog against the global scope's state. Tamper a file
(`echo x >> test/manual/project/.claude/skills/hello-world/SKILL.md`)
then refresh — it shows `✱ modified`; delete the dir and it shows
`✘ integrity-missing`.

The detail pane (`enter`) shows the centered identifier, a `Summary:` /
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

`bootstrap.sh` already wrote the global config (`$GRIM_HOME/grimoire.toml`
with the two registries), so skip `grim --global init` here — it would error
`config already exists` (exit 64) on the existing file. Just add + install:

```sh
grim --global add localhost:5050/grimoire/rules/security-baseline:1
grim --global install
```

### 8. Offline behavior

```sh
GRIM_OFFLINE=1 grim search        # serves cached catalog, exit 0
GRIM_OFFLINE=1 grim install       # warm blob cache succeeds; cold -> exit 81
```

### 9. Hooks — arming, then the dispatcher

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

**b. Gate one: the experimental flag.** Per scope, off by default:

```sh
grim config set options.experimental.hooks true
grim install                          # now it arms: "updated  claude (observer)"
grim hook list
```

**Why one gate was enough here.** The second gate is per-registry trust, and
this rig already satisfies it *implicitly*: `.grim-home/grimoire.toml` declares
`oci = "localhost:5050/grimoire"` — a **global**, **namespaced** `oci` entry,
which is the documented implicit grant. A **bare host** (`oci =
"localhost:5050"`) would not grant, because that is the whole shared registry;
it would prompt on a TTY and report `registry-not-trusted` otherwise. This is
worth internalizing: trust is granted at namespace granularity, and adding the
namespace to a locator is what consents.

**c. The dispatcher.** `grim hook run` is invoked by the launcher grim
generated, not by you — but driving it by hand shows the envelope and the
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
    | grim hook run --client claude --event PreToolUse --table "$TABLE" --root "$ROOT" >/dev/null
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
  | grim hook run --client claude --event PreToolUse --table "$TABLE" --root "$ROOT"
echo "exit=$?"
```

```
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"write-guard (a manual-rig demonstration hook) refused a Write mentioning .env"}}
exit=0
```

The payload wrote `{"decision":"deny","reason":"…"}`; grim projected it onto
Claude's own response shape. The exit code is `0` on every path grim controls —
deliberately, because a client that fails closed on a non-zero hook exit must
never be denied a tool call by grim's internals.

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

**f. Things that should NOT work.** Each exits `0` and runs nothing:

```sh
P="{\"hook_event_name\":\"PreToolUse\",\"tool_name\":\"Write\",\"tool_input\":{},\"cwd\":\"$PWD\",\"session_id\":\"s3\"}"

# A relative --table would resolve against the client-spawned CWD — the
# repository — which is what makes a table repo-plantable.
echo "$P" | grim hook run --client claude --event PreToolUse --table hooks/dispatch.json --root "$ROOT"

# An unknown root token is indistinguishable from an absent one.
echo "$P" | grim hook run --client claude --event PreToolUse --table "$TABLE" --root deadbeef
```

**g. Revoking.** An explicit `trust_hooks = false` beats every grant, in either
scope — including the implicit namespace grant from step (b):

```sh
grim --global config set registry.primary.trust_hooks false
grim install                          # both hooks skipped
grim status --format json | jq -c '[.items[] | select(.kind=="hook") | {name, state}]'
grim uninstall hook tool-logger       # reaps the dispatch row AND the registration
```

```
[{"name":"tool-logger","state":"gated"},{"name":"write-guard","state":"gated"}]
```

The reported `cause` is `registry-not-trusted` for both the never-granted and
the explicitly-opted-out case: one cause on purpose, since the remedy is the
same `trust_hooks` line. Restore the rig with
`grim --global config unset registry.primary.trust_hooks`.

> **A grim hook is defence in depth, never a security boundary.** `write-guard`
> is a demonstration: its check is a substring match that any rewording evades,
> and every layer of this design fails open so a broken guardrail can never deny
> you a tool call. Copy the shape, never the check.

## Teardown

```sh
test/manual/scripts/teardown.sh             # wipe rig state, keep registry
test/manual/scripts/teardown.sh --registry  # also stop + remove the registry
```

Re-run `scripts/bootstrap.sh` any time to recreate from the committed catalog.
