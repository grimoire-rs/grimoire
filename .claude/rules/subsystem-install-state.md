---
paths:
  - src/install/install_state.rs
  - src/path_safety.rs
---

# Install State

The project and global install-state records and the path containment guard. The telemetry id, a subsection of this record family, stays in [subsystem-file-structure.md](./subsystem-file-structure.md#telemetry-installation-id).

## Install State {#install-state}

Grimoire records what it installed, where, and at what content hash. The
record location differs by scope.

### Project state {#install-state-project}

Project install state lives at `<workspace>/.grimoire/state.json` — inside
a `.grimoire/` directory co-located with `grimoire.toml`. The workspace
directory is the key; there is no content hash of the config path in the
filename. Each project has exactly one state file at this fixed location,
so two projects sharing a common ancestor (or a shared `GRIM_HOME` volume)
cannot collide.

**Self-managed `.gitignore`**: the first time grim creates the `.grimoire/`
directory it writes `.grimoire/.gitignore` with contents `*` (if absent —
never overwrites a user-edited one). The consumer's root `.gitignore` is
never touched. This mirrors the convention used by [uv] (`.venv/.gitignore`)
and [pixi] (`.pixi/.gitignore`): the tool owns its dot-dir and excludes
its own contents from version control.

**Devcontainer named-volume caution**: if a devcontainer mounts a named
Docker volume at `<workspace>/.grimoire`, that volume shadows the
bind-mounted workspace state. A `grim install` inside the container writes
to the named volume, which is invisible to the host and to other containers
that bind-mount the same workspace directory. Use a bind-mount (not a
named volume) at `<workspace>/.grimoire` if you need state to be shared.

**Non-UTF-8 path components**: any path component that is not valid UTF-8
is rejected at store time with `UnknownAnchor`. All anchor roots must be
representable as UTF-8.

**Reap window**: between the read-only `load()` in a first post-upgrade
`status` call and the mutating `save()` in the next mutating command, the
old legacy state file (`$GRIM_HOME/state/projects/<sha>.json`) still
exists. A concurrent observer looking at the legacy path during this window
may see the old file even though the in-memory view is already migrated.
This is transient; the next mutating command reaps the legacy file.

**Nesting constraint**: `GRIM_HOME` must not be nested inside a workspace
directory. If it is, `from_target` may match `GrimHome` as the anchor for
a path that should classify as `Workspace`, producing an incorrect record.
For **hooks** the same condition is not a record caveat but a hard
**refusal to arm** — see [Hooks](./subsystem-install-layout-hooks.md#install-layout-hooks).

### Global state {#install-state-global}

Global install state lives at `$GRIM_HOME/state/global.json`. This
location is unchanged from previous versions.

**Residual risk under a shared GRIM_HOME**: when two machines or containers
share the same `GRIM_HOME` volume, both read and write the same
`global.json`. Anchoring makes the stored *paths* portable (each machine
resolves anchor roots from its own environment), but the *record set* in
the file is shared. Concurrent or serial `grim install --global` calls
from different machines are last-writer-wins on the record set — the same
class of collision that project state now avoids. **v1 stance: single
writer at a time.** Per-host segmentation (keyed by a machine identity
such as `devcontainerId`) is a tracked follow-up, not part of this change.
`atomic_write` prevents partial-file corruption; only record-set
last-writer-wins is a residual risk.


### Path containment guard {#path-containment-guard}

`AnchoredPath::resolve` enforces containment through two layers before any
filesystem operation runs on the joined path:

**Layer 1 (always, works for absent paths)**: every component of `relative`
must be `Normal`. Any `ParentDir` (`..`), `CurDir` (`.`), `RootDir`, or
`Prefix` component causes an immediate `TraversalAttempt` error without
touching the filesystem.

**Layer 2 (only when the candidate path exists)**: `dunce::canonicalize`
resolves both the candidate path and the anchor root, then asserts
`candidate.starts_with(anchor_root)` at the component boundary. A symlink
inside the anchor pointing outside it yields `EscapedAnchor`.

No consumer joins anchor + relative manually. Every filesystem operation
(read, hash, delete) receives the result of `resolve()`, never the raw
`relative` string.

The escape verdict is caller-typed (`Containment`): `Strict` for anything
that deletes, `AllowRelocatedAncestor` for read-only probes through a
symlinked ancestor (stow/yadm layouts, Unix only), and
`AllowRelocatedFile` — chosen by `ClientOutput::resolved_target` for every
**entry** output regardless of the caller — because a managed MCP member is
only ever read or spliced in place, never deleted, so a dotfiles-linked
vendor config (`~/.codex/config.toml → ~/dotfiles/…`) is the user's layout,
not an escape. The same reasoning is why the three user-authored files
grim rewrites (`grimoire.toml`, `grimoire.lock`, the docker credential
config) and every vendor-config splice go through
`atomic_write_through_symlink`, while grim-owned outputs keep the plain
rename that clobbers a planted link (issue #117).

See `quality-security.md` for the path-traversal and symlink-escape guard
principles that this two-layer pattern implements.

<!-- external -->
[uv]: https://docs.astral.sh/uv/
[pixi]: https://pixi.sh/
