# Research: What released grims do with the `mod` artifact kind

## Metadata

**Date:** 2026-11-18
**Domain:** cli
**Triggered by:** `/hex-architect` run on the experimental `mod` kind (`.agents/discussions/harness-mods-artifact-kind.md`)
**Expires:** re-verify when 0.17.0 ships or when `KNOWN_FEATURES` changes

## Direct Answer

Both released grims fail closed on every surface a `mod` can reach, with one
exception that already ships and cannot be repaired: grim 0.15.1 silently
rewrites a lock that carries `[[mod]]`, dropping the entry, on `lock`, `update`
and `add`. Grim 0.16.0 does not: it refuses all five commands on such a lock
(exit 78). Neither released grim can be told about `mod` by a manifest
annotation in a way that improves 0.15.1; 0.16.0 honours `requires-features`
and gives a clearer message when a skill or bundle declares one. The consent
move is safe for both: neither reads or touches `$GRIM_HOME/consent/`, and
they treat an absent `hooks/consent/` as "not consented" (hooks are skipped,
fail closed).

## Method

Real release binaries, downloaded from the GitHub release pages
(`https://github.com/grimoire-rs/grimoire/releases/download/v0.15.1/` and
`.../v0.16.0/`, asset `grimoire-x86_64-unknown-linux-gnu.tar.gz`). Both print
the expected version. `target/release/grim` and `test/bin/grim` in this
worktree print `grim 0.15.1` but are dev builds (`Cargo.toml` says 0.16.0, HEAD
is `v0.16.0-5`), so they were not used. No in-repo helper downloads the old
binary: `task test:old-grim` (`test/taskfile.yml` line 115) requires
`GRIM_OLD_BIN` and `test/src/old_grim.py` verifies it. A registry came from the
repo's own `test/src/zot.py` (`start_zot`, zot from `.ocx/toolchain`), no new
install. Every case ran in a fresh project and a fresh `GRIM_HOME` and `HOME`
under the scratchpad. All rows below are **run** unless marked **read**.
Driver scripts: `.../scratchpad/oldgrim/drv/{run,consent,fetch}.py`; raw rows:
`.../scratchpad/oldgrim/results.jsonl`.

`gen` = the project's `grimoire.toml` held one real skill. "T" = the toml bytes
changed, "L" = the lock bytes changed.

## Q1. Declaration and lock surfaces

### Config (`grimoire.toml`)

| Case | Command | 0.15.1 | 0.16.0 |
|---|---|---|---|
| `[mods]` table | `status`, `install`, `lock`, `update`, `add <skill> --name s2` (5 commands) | exit 78, `unknown field \`mods\`, expected one of options, registries, skills, rules, agents, bundles, mcp, plugin, hooks`; no T, no L | exit 78, same, list ends `hooks, settings` |
| `[options.experimental] mods = true`, no table | same 5 commands | exit 78, `unknown field \`mods\`, expected \`hooks\`` | exit 78, `unknown field \`mods\`, expected \`hooks\` or \`settings\`` |
| both together | same 5 commands | exit 78, fails at the `[mods]` line first | exit 78, same |

Both print the hint `grim <version> does not recognize that key or value ...
upgrade grim if the file was written by a newer version`. The files are left
byte for byte as they were. This matches the `settings` precedent
(`stability.md` lines 883 to 899): unknown keys in the declaration are fatal.

### Lock (`grimoire.lock` with a `[[mod]]` array appended to a lock the binary wrote)

| Command | 0.15.1 | 0.16.0 |
|---|---|---|
| `status` | exit 78, `unknown field \`mod\`, expected one of metadata, skill, rule, agent, mcp, hook, bundle, plugin` | exit 78, `the lock cannot be read; restore it from git or repair it, because deleting the lock moves every pin`, then the same unknown-field text |
| `install` | exit 78 as above | exit 78 as above |
| `lock` | **exit 0, lock rewritten (L), `[[mod]]` gone** | exit 78, lock untouched |
| `update` | **exit 0, lock rewritten (L), `[[mod]]` gone**, skill re-materialized | exit 78, lock untouched |
| `add <skill> --name s2` | **exit 0, T and L, `[[mod]]` gone** | exit 78, nothing changed |

This is the silent-rewrite case. It is the same one `adr_artifact_dependencies.md`
and `stability.md` already record for `[[settings]]` and `[[<kind>.requires]]`
on 0.15.1: its `lock`, `update` and `add` read a lock they cannot parse as no
lock. It ships in 0.15.1 and cannot be repaired there. 0.16.0 closed it (the
"restore it from git" refusal, commit `ee8ec9be`), so the hole is 0.15.1 only
and only for users who downgrade, or share a project with someone on 0.15.1.

### Registry (local zot; artifacts pushed by hand, `com.grimoire.kind = mod`)

Wire shapes pushed: grim's own shape (empty config, no `artifactType`, kind by
annotation only), the legacy shape with `artifactType =
application/vnd.grimoire.mod.v1`, and a variant with an unknown layer media
type. The layer was a plain tar holding `claude/` and `opencode/` directories.

| Situation | Command | 0.15.1 | 0.16.0 |
|---|---|---|---|
| mod, grim shape | `grim add <ref> --name x` | exit 65, `could not infer the kind of '<ref>'; pass --kind skill\|rule\|agent\|bundle\|mcp\|hook`; nothing changed | exit 65, same text, list ends `hook\|settings`; nothing changed |
| mod, `artifactType` also set | `grim add` | exit 65, same | exit 65, same |
| mod, unknown layer media type | `grim add` | exit 65, same | exit 65, same |
| any of the above | `grim add <ref> --name x --kind mod` | exit 64, `invalid value 'mod' for '--kind'` | exit 64, same |
| mod, direct | `grim fetch <ref>` | exit 1, `is not a Grimoire artifact (no kind on the manifest)` | exit 1, same |
| bundle with a `mod` member (sorted last) | `grim add <bundle> --name x` | exit 65, `invalid bundle: unknown variant \`mod\`, expected one of skill, rule, agent, bundle, mcp, hook`; nothing changed | exit 65, same, list ends `hook, settings`; nothing changed |
| skill with a `requires` edge to a mod, no feature annotation | `grim add <skill> --name x` | **exit 65, but after declaring and locking**: `expected a single-layer artifact, manifest has 2 layers`; T and L changed, the lock names the skill, `.claude/` empty | exit 65, `invalid bundle: unknown variant \`mod\`` at the edge; no T, no L |
| same, annotation `requires-features = dependencies,mods` on the skill | `grim add` | same as the row above (annotation ignored, 2-layer refusal) | exit 65, `artifact requires features this grim does not support (mods); update grim (this is grim 0.16.0)`; nothing changed |
| a plain skill with `requires-features = mods` | `grim add` | **exit 0, installed** (annotation ignored) | exit 65, same "update grim" text; nothing changed |
| mod carrying `requires-features = mods` | `grim add` | exit 65, could not infer kind | exit 65, could not infer kind (the kind check runs first; the feature check never fires) |
| bundle whose member mod carries the feature, bundle itself does not | `grim add` | exit 65, unknown variant `mod` | exit 65, unknown variant `mod` |

Residual on 0.15.1 for the requires row: the dependent skill is declared and
locked before the refusal (the same residual `test_old_grim_rejects_a_requires_edge_to_settings`
pins for settings). No `mod` entry, no `mod` file and no client output appear.

An unknown layer media type on a **known** kind is read as raw tar on both
(`classify`, `src/oci/layer.rs` lines 92 to 105): a skill pushed with layer
type `application/vnd.grimoire.mod.v1.tar` was decoded as a tar on both
binaries (`fetch` failed on a fixture path mismatch, `artifact is missing its
'sku/SKILL.md' index`, after the tar was read). That fallback is never reached
for a `mod` on an old grim because the kind is unresolved first.

`grim describe` exits 0 on a mod on both; I did not inspect its body (kind
display for an unknown kind is **not checked**).

## Q2. `requires-features` signalling

What exists (**read**, `src/oci/features.rs`, `git show v0.16.0:src/oci/features.rs`):

- 0.15.1 has no `features.rs`; it ignores the annotation. **Run**: a skill with
  `requires-features = mods` adds cleanly, exit 0.
- 0.16.0 has `KNOWN_FEATURES = ["dependencies"]`. `check_required_features`
  runs on `fetch` (`src/fetch.rs` 361), on the resolver graph (`src/resolve/graph.rs`
  721) and on install (`src/install/installer.rs` 2956). Unknown name: exit 65
  with "update grim". **Run**, see the table above. It runs only after the kind
  is resolved, so an unknown `mod` kind is refused earlier with the less helpful
  "could not infer the kind" or "unknown variant".
- The annotation key is a one-way door; `KNOWN_FEATURES` membership is two-way
  (`adr_artifact_dependencies.md` Decision 2, lines 297 to 345). `adr_artifact_dependencies.md`
  line 77 states the shipped limit: the annotation cannot gate 0.15.1, and this
  run confirms it.
- `settings` declared no feature. `grep` for `requires-features` finds nothing
  in `adr_settings_artifact_kind.md`, `src/oci/settings.rs` or `release.rs`.
  Settings relied on the unknown-kind and unknown-variant refusals, which this
  run shows are exit 65 on both binaries.

Effect of declaring `mods` on each artifact:

| Artifact | 0.15.1 | 0.16.0 | Worth it |
|---|---|---|---|
| the `mod` itself | no effect | no effect (kind check first) | no |
| skill, rule, agent with a `requires` edge to a mod | no effect (2-layer refusal after declare and lock) | clear "update grim" at exit 65, nothing declared, replaces `unknown variant \`mod\`` | yes |
| bundle with a mod member | no effect | **read, not run**: the graph check at `graph.rs` 721 runs per manifest, so an annotation on the bundle itself should fire before the members parse; not exercised | yes, if the writer can set it |
| skill that merely coexists with mods | n/a | would be refused for no reason | never |

Trade: the annotation costs the new writer one rule ("a manifest that lists a
mod, as member or as `requires` edge, sets `requires-features = mods`") and one
`KNOWN_FEATURES` entry. It buys a clear message on 0.16.0 only. It does nothing
for 0.15.1, and 0.15.1 already fails closed on the same artifacts. It does
**not** protect the 0.15.1 lock rewrite.

## Q3. Kind order

The order is `#[derive(PartialOrd, Ord)]` on `ArtifactKind` (`src/oci/artifact_kind.rs`
line 22), i.e. declaration order. Where it is observable:

- **Bundle members layer and requires layer**: `BundleManifest::new` sorts by
  `(kind, name)` using that derived `Ord` (`src/oci/bundle.rs` line 84), then
  serializes pretty JSON. The bytes are a layer blob, so the order is in the
  layer digest and therefore the manifest digest. `test/src/helpers.py`
  `_KIND_ORDER` mirrors it, with a comment that a text sort "changes the layer
  digest".
- Lock `requires` edges sorted by `(kind, name)` (`src/lock/grimoire_lock.rs`
  709) and the lock's array emission order ("grouped by kind in wire order",
  line 187). The lock text itself is ordering-sensitive only to the extent
  diffs are; the lock digest comparison reads fields.
- `ArtifactKind::ALL` and a unit test (`artifact_kind.rs` 290 to 294) assert
  `ALL` is sorted by `Ord`.
- Not on the wire: the `com.grimoire.kind` annotation is the lowercase string
  (`#[serde(rename_all = "lowercase")]`), so no discriminant is serialized.

**Correction to the claim.** Appending last is not what protects existing
bundles. Inserting `Mod` anywhere keeps the relative order of every existing
kind, and no existing bundle contains a `mod`, so no existing bundle's bytes or
digest change either way. What the order does is fix, forever, where a `mod`
sorts relative to `hook` and `settings` inside **new** bundles and requires
layers; a published bundle's digest would change if a later grim moved it. So
the real rule is "pick the position once, never move it", and the cheapest
position to defend is last, because `ALL`, `all_index`, the schema doc order
and `_KIND_ORDER` then only grow at the end (`settings` did the same, test
`a_settings_member_sorts_after_every_other_kind`, `bundle.rs` 149). Appending
last is a sound choice, not a forced one. Read, not run.

## Q4. Layer layout freedom

**Read**, `src/oci/manifest.rs` 78 to 104 and `src/oci/layer.rs`:

- `layout(kind)` accepts one content layer, or one content layer plus one
  requires layer in either order, for any non-bundle kind. A `mod` content layer
  fits the first two shapes with no change. The requires layer is recognized
  only by media type `application/vnd.grimoire.bundle.v1+json`.
- `classify` (`layer.rs` 80 to 88): the two gzip media types and the OCI and
  Docker gzip types decode as gzip; anything ending `+zstd` is refused; **any
  other type is read as an uncompressed tar**. So the existing default
  `application/vnd.grimoire.artifact.layer.v1.tar`, or any new type, reads as
  tar. A new layer type is therefore wire-legal today and degrades to tar.
- `is_tar_kind` (`layer.rs` line 106) must gain `Mod` as a tar kind. That is a
  code change in the new grim, not a wire change.
- The config is the OCI empty config, `{}` (`adr_oci_empty_config_compat.md`);
  `artifactType` is dropped on write because GitLab rejects it. Kind travels by
  the `com.grimoire.kind` annotation.
- "One directory per harness (`claude/`, `opencode/`) inside one tar layer"
  needs nothing the wire format lacks: a tar holds any tree, and hooks already
  use a directory tar (rooted at `<name>/`, `make_hook`). I pushed exactly that
  tar (`claude/hook.sh`, `opencode/p.js`) as a `mod` and the registry took it
  (zot, grim-shaped manifest). A new reader still has to cap sizes and entry
  counts for it; the existing tar read path limits (`MAX_EXPANSION_RATIO`, stored
  and decoded caps) apply as for a skill.
- An old grim never reaches the layer: the kind fails first (rows in Q1).
  A new layer media type for `mod` would make no difference to old grims and is
  not needed. Using the default tar type keeps `compression_of` and
  `grim inspect`-style readers working.

## Q5. Consent record move

Hook arming read from `$GRIM_HOME/hooks/consent/<workspace-key>.json` on both
released grims (`ls` of the tree after `hook allow`). Project: one real hook
(`make_hook`), `[options.experimental] hooks = true`, `GRIM_HOME` in scratch.
`install` columns show the `Armed` column; "skipped" means not armed.

| Situation | Command | 0.15.1 | 0.16.0 |
|---|---|---|---|
| no consent record at all | `grim install` | exit 0, hook `skipped`, no client entry | exit 0, hook `skipped` |
| after `grim hook allow` | `grim install` | exit 0, hook `updated`, armed `claude (gatekeeper)` | exit 0, same |
| `hooks/consent/` deleted, only `consent/<key>.json` present (any content) | `grim hook list`, `grim install` | exit 0, hook `skipped` (consent not found, re-consent needed); `consent/` never read | exit 0, `skipped`; never read |
| `hook allow` run with `consent/<key>.json` present | `grim hook allow` | exit 0, writes `hooks/consent/<key>.json` afresh; leaves `consent/<key>.json` byte for byte | same |
| both exist | `hook list`, `hook revoke` | exit 0; reads the `hooks/` copy only; revoke leaves `consent/<key>.json` in place | same |

The workspace key matched across binaries only within a binary (different
scratch roots), as expected. Result: moving the record to `$GRIM_HOME/consent/`
is safe for both released grims. They fail closed (hooks skipped, the user
re-runs `hook allow`) and never delete or rewrite the new directory. The
asymmetry to expect after the move: a user who runs the newer and the older
grim alternately must consent twice (once per record location), and a revoke
from the new grim does not revoke the old location. The new grim should either
read the old directory as a fallback for one release, or accept the double
consent. The `settings:` keys already behave this way: 0.15.1 `grim hook allow`
drops them (`stability.md` table, `test_old_grim_hook_allow_drops_settings_consent_keys`).

I read `git log -p src/hook/consent.rs` only by file history (5 commits, last
`9e7295a6`); the run answers the question, so no deeper history walk was done.

## Final table (copy into the ADR)

| Situation | 0.15.1 | 0.16.0 |
|---|---|---|
| `[mods]` table in `grimoire.toml`, any command | 78, `unknown field \`mods\``, untouched | 78, same |
| `[options.experimental] mods = true`, no table | 78, `unknown field \`mods\`, expected \`hooks\`` | 78, `expected \`hooks\` or \`settings\`` |
| lock with `[[mod]]`: `status`, `install` | 78, unknown field `mod` | 78, "the lock cannot be read; restore it from git" |
| lock with `[[mod]]`: `lock`, `update`, `add` | **0, silently rewritten, `[[mod]]` dropped** | 78, lock untouched |
| `grim add <mod>` | 65, could not infer the kind | 65, same |
| `grim add <mod> --kind mod` | 64, invalid value | 64, invalid value |
| `grim fetch <mod>` | 1, not a Grimoire artifact | 1, same |
| bundle with a mod member | 65, unknown variant `mod`; nothing changed | 65, same |
| skill with `requires` edge to a mod, no annotation | 65 after declare and lock (T and L changed, 2-layer refusal) | 65, unknown variant `mod`; nothing changed |
| same, `requires-features = mods` on the skill | same as no annotation (ignored) | 65, "requires features this grim does not support (mods); update grim" |
| plain skill with `requires-features = mods` | 0, installed (ignored) | 65, update grim |
| unknown layer media type, known kind | read as tar | read as tar |
| consent record only at `$GRIM_HOME/consent/`, `hooks/consent/` absent | hooks skipped (fail closed); new dir untouched | hooks skipped; new dir untouched |
| both consent locations present | reads `hooks/consent/` only; new dir untouched | same |

## Recommendation

1. **Declare `requires-features = mods` on dependents, not on the mod.** On any
   manifest that lists a mod as a `requires` edge (and on a bundle that lists one,
   after confirming the bundle check fires), emit the annotation and add `mods`
   to `KNOWN_FEATURES`. It turns "unknown variant `mod`" into "update grim" on
   every grim from 0.16.0 and later that predates `mod`, costs one writer rule,
   and the name is a two-way door. Do not put it on the `mod` itself (never read
   before the kind check) and never on artifacts that only coexist with mods.
2. **Do not claim it protects 0.15.1.** Nothing does: 0.15.1 ignores the
   annotation, and its lock rewrite on `[[mod]]` is shipped. State that as a
   known residual next to the settings one in `stability.md`. 0.16.0 already
   fails closed, so the user-visible gap is the 0.15.1 downgrade path only.
3. **Keep the `[mods]` / `[[mod]]` / flag names as decided.** The declaration
   and lock surfaces are exit 78 on both released grims, the same fail-closed
   shape as hooks and settings. Add a new interop test module modelled on
   `test/tests/test_old_grim_settings.py` (pin 0.15.1) and extend it to 0.16.0
   (the lock row differs from 0.15.1).
4. **Consent move.** Safe for both. Decide whether the new grim reads
   `hooks/consent/` as a one-release fallback; otherwise document that users
   who alternate grims consent twice, and that a revoke does not cross
   locations.
5. **Kind order.** Append last, but record the real reason (never move a
   published position), not "existing bundles would break".

## Not run, and why

- `grim describe` body for a mod, and a bundle with the annotation on the
  bundle manifest itself under 0.16.0 (read from `graph.rs` 721, not run).
- Windows and macOS. Linux x86_64 binaries only.
- `grim hook` with a hand-written new-shape consent record (the file content
  was an arbitrary JSON; no new grim exists that writes the real shape).

## Sources

- Release binaries: `https://github.com/grimoire-rs/grimoire/releases/download/v0.15.1/grimoire-x86_64-unknown-linux-gnu.tar.gz` and `.../v0.16.0/...` (the same release pages `task test:old-grim` expects).
- `docs/src/content/docs/stability.md` lines 883 to 899 (the 0.15.1 settings table), `test/tests/test_old_grim_settings.py`, `test/src/old_grim.py`, `test/src/zot.py`, `test/src/helpers.py`.
- `src/oci/features.rs`, `src/oci/manifest.rs`, `src/oci/layer.rs`, `src/oci/bundle.rs`, `src/oci/artifact_kind.rs`, `src/fetch.rs`, `src/resolve/graph.rs`.
- `.agents/adr/adr_artifact_dependencies.md` (Decision 2), `.agents/adr/adr_settings_artifact_kind.md`, `.agents/adr/adr_oci_empty_config_compat.md`.
