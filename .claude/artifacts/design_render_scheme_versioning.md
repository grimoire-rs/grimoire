# Renderer Versioning — Gap Analysis & Recommendation

Status: proposed, deferred — build only when the first renderer changes
an output's shape at a stable index path (plugin mode:
`render="plugin"`, ADR Decision 3/4). Date: 2026-07-18.

**Scope:** `v0.9.1..main`, focused on whether grim can migrate an artifact across a *render-scheme* change (not content change) — up to a radical shift like "skill files" → "Claude plugin registration."

**Bottom line:** The **cleanup half** (reap old paths by diffing recorded state against freshly-rendered state) is already fully generic and shipped. The **trigger half** (decide to re-render *because the scheme changed*) is today only a **proxy** — a stored-path-vs-current-path comparison — that catches *location* moves but is blind to *shape* changes that keep the index path stable, and to render-mode switches. A render-scheme version stamp is the minimal thing that closes that blind spot, but it is **not needed yet** — no shipped or ADR-planned migration triggers the blind spot. Recommendation: don't build it until the first stable-index-path shape change lands; when it does, it is a ~3-point additive change and the existing reaper needs zero modification.

---

## 1. What the current mechanism records and does

### What state records (per installed output)
`ClientOutput` — `src/install/install_state.rs:65-95`:

- `client` (string)
- `target: AnchoredPath` — a typed `{anchor, relative}` pair (`PathAnchor` enum + forward-slash relative), the **single index path**
- `content_hash: Digest` — footprint hash (index + support dir folded together; or *semantic* JSON hash for entry outputs)
- `support_dir: Option<AnchoredPath>` — one optional sibling dir (multi-file rules)
- `entry: Option<String>` — a JSON pointer when the "output" is a spliced member of a shared config file (MCP registration)

**There is no renderer/layout version stamp anywhere.** The only version in the file is `InstallStateVersion::{V1,V2}` (`install_state.rs:53-62`) — that versions the **state schema**, not the render scheme. Confirmed: `grep` for `scheme|render_version|layout_version` across `src/install/` finds nothing.

The record models exactly one on-disk footprint per `(client, kind, name)`: **one index file + at most one sibling directory**, *or* one config-file entry. That two-slot-or-entry footprint is the implicit shape ceiling.

### What "the current layout" is
`ClientTarget::path_for(workspace, scope, kind, name)` → **exactly one `PathBuf`** (`src/install/client_target.rs:136`), delegating to the vendor's per-kind `skills_root`/`rule_path`/`agent_path`/`mcp_config_path`. This single path is the sole "where does the current scheme put this" signal.

### The two moving parts on `grim update`
`grim update` calls `install_all_with_progress(...)` over the **entire** locked set, then `prune_orphans` (`src/command/update.rs:133,163`). So *every* locked artifact — pin changed or not — routes through `install_one` → `integrity_gate`, which is where the migration trigger lives. (This is why a pure layout move migrates on `update` even when the lock is byte-identical.)

**(a) The trigger — `integrity_gate` + `output_at_current_layout`** (`installer.rs:812`, `:888`):

```
output_at_current_layout(out): 
  if out.entry.is_some()          -> true   (entry outputs exempt)
  dest = path_for(client,kind,name)          (current layout)
  match AnchoredPath::from_target(dest,...):
     Ok(current) => current == out.target     (structural anchor+relative equality)
     Err(_)      => true                       (can't compute here -> "current")
```

`integrity_gate` short-circuits to `AlreadyInstalled` only when every output is `all_intact` (on-disk hash == recorded hash), the pin is unchanged, **and** `covers_targets` — which requires each target client's recorded output to pass `output_at_current_layout`. If a recorded path ≠ today's `path_for`, `covers_targets` is false → the gate **falls through** → the artifact is re-materialized at the new path.

**(b) The cleanup — `reap_moved_outputs`** (`installer.rs:924`): after re-materialize, it diffs `prior.outputs` against the freshly-produced `new_outputs`:

```
for out in prior.outputs:
  guard 1: entry.is_some()                  -> skip (never touch shared config)
  guard 2: new_outputs has same .target     -> skip (still produced)
  guard 3: !is_present / unresolvable        -> skip
  guard 4: on-disk hash != recorded hash     -> skip (user-edited, preserved)
  guard 5: canonicalizes onto a live new output's footprint (symlink alias) -> skip
  else: best-effort delete target (+ support dir)
```

---

## 2. Answer: what happens today when a renderer changes output *shape*?

**The reaper is generic, not hardcoded per migration.** It knows a path is "old layout" purely structurally: *any recorded output target not present in the freshly-rendered target set* is an orphan (guard 2 is the whole test). There is no per-migration table, no "if Copilot old path" special-case. The first consumer (Copilot global-rule move, commit `4670de4`/`02e313d`) exercises exactly this generic path — old `$GRIM_HOME/.opencode/...` vs new `~/.copilot/instructions/...`, both single files, detected by path inequality and reaped by diff.

**So for a shape change that manifests as a different index path** (different file, different directory), `grim update` on an old-shape artifact: re-renders at the new path, reaps the old one (unless user-modified), re-anchors the record. Works, generically.

**But the trigger is a proxy for the wrong thing.** `output_at_current_layout` compares only the **index path**. It is a proxy for "did the scheme change" that actually measures "did the index location move." That proxy has three blind spots (next section).

---

## 3. Gap: is a version stamp needed?

Decompose the maintainer's target ("uninstall the old-scheme files by state, re-render fresh under the new scheme") into two obligations:

| Obligation | Mechanism today | Generalizes to arbitrary scheme change? |
|---|---|---|
| **Know what to delete** | State records exact anchored paths; reaper diffs recorded-vs-rendered | **Yes.** Already an uninstall-by-recorded-state-then-diff. Fully generic. |
| **Decide to re-render** | `output_at_current_layout` (index-path move) ∨ pin change ∨ hash drift ∨ client-coverage change | **No.** Blind to shape changes at a stable index path, and to render-mode switches that don't move `path_for`. |

The "know what to delete" half already **is** the "record exact paths + hashes; uninstall-by-state then re-render" design the question asks about, and it does generalize. The missing piece is a reliable **"the scheme changed" signal**. Today that signal is inferred from a path move. Where the inference fails:

**Blind spot A — shape change at a stable index path (the real gap).**
If a future scheme keeps `path_for` returning the same index path but changes the surrounding footprint — adds/removes sibling files, restructures the support dir, splits one file into several beside it — and the pin is unchanged, then on `grim update`: the on-disk old file still hashes to the recorded `content_hash` (`all_intact = true`), `output_at_current_layout` returns true (path unchanged), pin unchanged → **`AlreadyInstalled` short-circuit. No re-render, no reap. The migration silently never happens.** `content_hash` covers the full footprint, so a re-render *would* produce a different hash — but the re-render is never reached, because nothing in the trigger sees a shape change that doesn't move the index path.

**Blind spot B — file → entry (skill → plugin), the ADR's own direction.**
`adr_render_layout_stability.md` Decision 3 records that plugin mode renders sources into **grim-owned roots** (`$GRIM_HOME/claude/marketplace/…`) and registers them **entry-typed** (`ClientOutput.entry` into `known_marketplaces.json`/`enabledPlugins`) — explicitly *"no state V3, no new PathAnchor variant."* On the files→plugin switch, the old file-scheme skill outputs (`entry=None`, real files) *would* be reaped correctly by the generic diff (new outputs are entry-typed, so guard 2 never matches → old files deleted) — **provided the re-render is triggered.** But plugin mode is opt-in via a config key (`[options] render="plugin"`, Decision 4), and a **config-mode flip does not move `path_for` and is not seen by any current trigger.** The proxy cannot observe a mode switch. So B is really a special case of A: the deciding signal is absent.

**Blind spot C — entry → entry / entry → file relocations.**
Entry outputs are doubly exempt: `output_at_current_layout` returns `true` for them, and `reap_moved_outputs` guard 1 skips them. So relocating an MCP registration (e.g. `.mcp.json` `/mcpServers/x` → a different file or pointer) is neither triggered nor reaped. Not on the skill→plugin path, but a real limitation of "arbitrary scheme change."

**Non-gaps (correctly handled, worth stating):**
- **User-edited old files** (guard 4) are preserved forever → after a real shape migration the user keeps *both* the edited old-scheme file and the new-scheme output (**duplication**). This is the deliberate preserve-when-modified rule (`stability.md`: "a locally modified old file is never deleted... no `--force` override"). Unavoidable given that policy; the cost is transient duplication, never data loss.
- **Declined outputs** (e.g. Codex rules) record zero outputs → nothing to migrate or reap. Fine.
- **Offline** re-materialize of a *missing* output still needs a manifest fetch (`stability.md` §limitations-offline-remat) — orthogonal to scheme versioning but worth noting: a scheme migration that deletes the old output then can't re-render offline would leave the artifact missing until back online. The layout reaper deletes *after* a successful re-render in the same `install_one`, so this only bites if the re-render itself fails.

**Conclusion:** A version stamp is not needed to record *what* to delete (state already does). It is the minimal *direct* signal for *when to re-render* — closing blind spots A and B, which the path-move proxy structurally cannot see.

---

## 4. Recommendation — laziest mechanism that works

### Tier 0 — now: build nothing (YAGNI)
Every migration that ships or is ADR-planned today is a **path move**, which the generic reaper + index-path proxy already handle:
- Copilot global-rule move — shipped, works.
- Plugin mode's "sources into grim-owned roots" — a path move of grim-owned files + *additive* entry registration into a new file; the old file-scheme outputs it supersedes are file outputs the generic reaper deletes.

No renderer changes an output's shape at a stable index path. The ADR itself deferred plugin mode and committed to "no state V3." Adding a scheme stamp now is speculative infrastructure for a migration that does not exist. **Stop here until the first shape-change-at-stable-path renderer is actually written.**

### Tier 1 — when the first stable-index-path shape change lands: one additive field, three integration points, zero reaper change

Add an additive optional field to `ClientOutput` (`install_state.rs:65`), governed by the existing state additive-field policy (default = "rendered before scheme versioning"):

```rust
#[serde(default, skip_serializing_if = "is_zero")] // matches support_dir/entry precedent
pub render_scheme: u16,   // 0 = pre-versioning; bumped per vendor when a scheme's SHAPE changes
```

(Guard the serde default so a state file with no scheme stamps stays byte-identical for older grim — same trick `entry`/`support_dir` already use, `install_state.rs:82-94`.)

Three integration points:

1. **Stamp on write** — the materialize loop that builds `ClientOutput`, `installer.rs:664`. Set `render_scheme: client.vendor().render_scheme(kind)` via a new cheap `Vendor` trait method defaulting to `0`. A vendor bumps its own counter only when it changes a kind's on-disk *shape*.

2. **Trigger on mismatch** — `output_at_current_layout`, `installer.rs:888`. Add one clause: the output is "current" only if `out.render_scheme == vendor.render_scheme(kind)` **and** the path matches. A scheme mismatch makes `covers_targets` false → `integrity_gate` falls through → re-render. This is the direct signal the proxy lacks; it also naturally captures the `render="files"→"plugin"` config flip (plugin mode is simply a different scheme number for that vendor), so files→plugin migrates on the next `update` with **no separate migration code**.

3. **Cleanup — nothing.** `reap_moved_outputs` already deletes prior outputs absent from the freshly-rendered set by structural diff. A re-render under the new scheme produces the new footprint; the old footprint (different path, or file-vs-entry) falls out of guard 2 and is reaped, under all five existing safety guards. **No change.**

For blind spot C (entry relocations), when/if ever needed: make the `entry.is_some()` early-returns in `output_at_current_layout` and reaper guard 1 *scheme-aware* (compare stamps before exempting). Flag as a known limitation now; do not build.

### What `stability.md` would need to say
One additive sentence in the `## The compatibility promise` render-layout section (the current text only covers *path* moves — "re-materializes the artifact at its new path... reaps the unmodified old output"). Extend it to shape:

> A renderer that changes an output's on-disk **shape** without moving its index path bumps a per-vendor render-scheme counter; a recorded output at an older counter is re-rendered on the next `install`/`update` and its old shape reaped — under the same preserve-when-modified rule (a locally-modified old output is kept, so a shape migration may transiently leave both the edited old files and the new output on disk).

This stays inside the existing state additive-field guarantee (`render_scheme` defaults to 0) and the existing "vendor render layout is unstable / discover via `status --format json` `outputs`" contract — no new frozen surface, no schema-version bump.

---

### One-line summary for the maintainer
State already records exact paths+hashes and the reaper is already a generic uninstall-by-state-then-diff, so the *cleanup* generalizes to arbitrary scheme changes today. The only missing piece is a reliable *"re-render because the scheme changed"* trigger: the current path-comparison proxy catches location moves but is blind to shape changes at a stable index path and to render-mode flips. Fix that — when a shape-changing renderer first exists, not before — with a `render_scheme: u16` stamp on `ClientOutput` checked in `output_at_current_layout`; the reaper needs no change, and `stability.md` needs one additive sentence.

Key files: `src/install/install_state.rs:65` (`ClientOutput`), `src/install/installer.rs:812` (`integrity_gate`), `:888` (`output_at_current_layout`), `:924` (`reap_moved_outputs`), `src/install/client_target.rs:136` (`path_for`), `src/command/update.rs:133` (update routes all locked artifacts through the install path), `docs/src/stability.md` (compatibility-promise section), `.claude/artifacts/adr_render_layout_stability.md` Decision 3 (plugin = entry-typed, no state V3).
