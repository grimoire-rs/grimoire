# Plan: TUI Phase 3 — Collapsible Bundle Leaves + Individually-Actionable Members

## Status

- **Plan:** plan_tui_member_nodes
- **Active phase:** 5 — review-fix + Codex gate (complete)
- **Step:** finalized
- **Last update:** 2026-06-20 (after 171c329 feat + 9da4115 chore: P0-P5 done, fix round 1 landed all Codex+review findings, ff-merged to local main; task --force verify green)

---

## Classification

| Field | Value |
|-------|-------|
| Tier | high |
| Scope | Small–Medium |
| Reversibility | reversible (additive variants/fields; two-way door) |
| Overlays | codex=on (untrusted-input surface: member repo flows into an install Identifier) |
| Subsystems touched | `src/tui` |
| CLI surface change | none (TUI runtime behavior only) |

---

## Problem

User report (verbatim): *"the dynamic bundle node has no collapsable icon, it should be shown in the tui as collapsed state, further i cannot install skills within a bundle alone, i would expect them to be like general nodes."*

### Issue 1 — bundle leaf is not visibly collapsible and does not default to collapsed

A bundle artifact is always a `Node::Leaf` (`tree.rs:610` `walk` builds `DisplayRow::Leaf` for every `Node::Leaf`), never a `Node::Group`. The `DisplayRow::Leaf` render arm (`render.rs:422-451`) emits `format!("{indent}{label}")` with **no arrow** — only `DisplayRow::Group` renders the `▸`/`▾` affordance (`render.rs:398`). `DisplayRow::Leaf` carries no `key`, `kind`, or `collapsed` field (`tree.rs:167-177`), so the render layer cannot tell a bundle leaf from any other leaf and has no collapse state to read.

Member splicing in `flatten_with_members` (`tree.rs:503-525`) gates **only on cache presence** (`bundle_members.get(&key)` is `Some`) and `tui_row.kind == "bundle"` — there is **no collapsed gate**. Combined with the Expand handler eagerly inserting a `Loading` cache entry (`event.rs:374-384`), once a bundle is expanded it can never be visually re-collapsed: there is no key in `collapsed` for a bundle leaf, and even if there were, the splice ignores it.

**Root cause:** bundle leaves are modelled as plain `DisplayRow::Leaf` with no kind/collapse awareness, the render arm has no glyph branch for them, and the member splice has no collapsed gate.

### Issue 2 — bundle members cannot be installed/updated/deleted individually

`DisplayRow::Member` is read-only by Phase 2 design (`adr_projection_over_index.md` Consequences: "Future per-member install (Phase 3) must introduce a *separate* targeting path"). Concretely: `selected_row_index()` returns `None` for a `Member` (`state.rs:446`); `action_targets()` contributes nothing for a `Member` selection (`state.rs:543-559`, no `Member` arm); `toggle_mark_selected()` is a no-op for `Member` (`state.rs:477`). The `i`/`u`/`d` keys route through `batch()` (`event.rs:244-251`) → `action_targets()` → empty → `TuiAction::None`. `MemberNode` already carries `member_repo: Option<String>` (`bundle_members.rs:85`, `Identifier`-validated) and a lock-derived `state` (`bundle_members.rs:89`), but `DisplayRow::Member` does **not** carry `member_repo` (`tree.rs:189-204`), so the event layer cannot synthesize an action.

**Root cause:** members have no targeting path into the action layer, and `DisplayRow::Member` drops the `member_repo` the action would need.

---

## Map corrections (verified against HEAD)

The supplied map is accurate in substance; line numbers drifted because Phase 2 (commit `ad06482`/`452afb3`) already landed the `Member` machinery. Corrections:

1. **`DisplayRow::Member` already exists** (`tree.rs:189-204`) with fields `label, depth, kind, state, related, parent_bundle_repo`. It is **missing only `member_repo`** — D6 is a single field addition, not a new variant.
2. **`DisplayRow::Leaf`** (`tree.rs:167-177`) has fields `label, depth, row, state` — confirmed **no** `key`/`kind`/`collapsed` (D1/D2 add them).
3. **`flatten_with_members`** is at `tree.rs:483-591` (not 483 only); the splice loop at `:503-525` gates on cache-presence + `kind=="bundle"`, **no collapsed gate** (D4 adds the `expanded_bundles` member-splice gate). The inner `flatten(tree, collapsed)` (`tree.rs:458-462`, calling `walk` at `:593-618`) gates GROUP children via `collapsed` only and **stays unchanged** — `flatten_with_members` first calls `flatten(tree, collapsed)` (`:496`) to handle group collapse, then post-passes to splice members keyed on `expanded_bundles`. The two sets are orthogonal: `collapsed` gates groups inside `flatten`, `expanded_bundles` gates member splicing inside the `flatten_with_members` post-pass.
4. **The Expand handler already eagerly inserts `Loading`** (`event.rs:374-384`) before emitting `LoadBundleMembers`. Consequence: a bundle leaf that has *ever* been expanded always has a cache entry, so the "default collapsed" affordance (D3) cannot key off cache-presence — it must key off an explicit `collapsed`-set membership plus a "bundle leaf is always expandable" flag.
5. **All `DisplayRow` exhaustive-match sites** already have `Member` arms (`render.rs:452`, `state.rs:446/472-477/766/793/936-939/957-962`). Adding `member_repo` to the `Member` variant touches only construction sites (`tree.rs:533-578`) and any site that destructures all fields — most use `..` so will compile unchanged.
6. **`SelectionAnchor::Member { parent_bundle_repo }`** already exists (`state.rs:766`, `:793`). No new anchor needed.
7. **`perform(&TuiRow)`** (`app.rs:1200`) builds the `Identifier` from `row.repo` + `row.pinned_version`/`latest_tag`. A `DisplayRow::Member` carries `member_repo` + `kind` but no `TuiRow`, so the member action needs a thin entry (`perform_member`) that resolves a tag, then **reuses the very same declare → relock → materialize seam `perform` uses** (`app.rs:1216-1285`) — no forked install logic, no new provenance code. A standalone member install is an independent `grimoire.toml` declaration, identical to a leaf install of the same repo (D8a, GAP-4). Tag resolution follows the leaf precedence: the related catalog row's `pinned_version`-or-`latest_tag`, else `"latest"` — the same predictable semantics as every other TUI install. (Installing at the bundle-pinned digest is a deferred future enhancement, not a current requirement.)

---

## Decisions (binding)

### Issue 1 — collapsible bundle leaf

- **D1 — Reuse `collapsed: BTreeSet<String>` for bundle-leaf keys.** Bundle-leaf keys are full path strings (`LeafNode.key`, e.g. `acme/bundle-x`); group keys are path *prefixes*, so the two key spaces do not collide (`state.rs:190` confirms `collapsed` holds group keys only today). *Rationale:* one collapse-state structure, no new field on `TuiState`; persists across reshapes like group collapse.

- **D2 — Add `key: String`, `is_bundle: bool`, `collapsed: bool` to `DisplayRow::Leaf`, set in `walk()`.** `key` mirrors `LeafNode.key`; `is_bundle` is computed in `walk` from `rows[l.row].kind == "bundle"`; `collapsed` = `collapsed_set.contains(&l.key)`. *Rationale:* a typed projection beats a render-time secondary lookup into `TuiState`; keeps `tree_render_rows` a pure `DisplayRow → RenderRow` map. `walk` must receive `rows: &[TuiRow]` to read the kind (it currently does not — small signature change, threaded from `flatten`/`flatten_with_members`).

- **D3 — A bundle leaf ALWAYS renders an arrow and defaults to collapsed.** Render arm: when `is_bundle`, prefix `▸` (collapsed) or `▾` (expanded), with ASCII fallback `>`/`v` consistent with how the codebase already chose plain ASCII for member placeholders (`tree.rs:546`). "Expanded" iff the bundle key is **NOT** in `collapsed`. "Collapsed" otherwise. *Default-collapsed* is achieved by **seeding the bundle leaf's key into `collapsed` when it first appears** (see D3a) — so the user sees `▸` before any fetch. Non-bundle leaves render exactly as today (no arrow). *Rationale:* the affordance must be visible before any network/lock fetch, matching the user's "shown as collapsed state" ask.

- **D3a — Default-collapsed seeding.** A bundle leaf is collapsed by default. Implement by treating "bundle key absent from `collapsed` AND never explicitly expanded" as **collapsed** rather than mutating `collapsed` on every flatten (flatten must stay pure — it takes `&BTreeSet`, `state.rs:874`). Decision: **invert the gate for bundle leaves** — a bundle leaf's members splice iff its key is present in an explicit `expanded_bundles: BTreeSet<String>` set on `TuiState`, NOT keyed off `collapsed`. *Rationale:* groups default-expanded (absence-from-`collapsed` = expanded); bundle leaves must default-**collapsed** (absence = collapsed). Overloading one set with two opposite default polarities is a latent bug. A dedicated `expanded_bundles` set gives bundle leaves the opposite default cleanly, keeps `collapsed` semantics intact for groups, and stays a pure `&BTreeSet` input to flatten. **This refines D1:** bundle expand-state lives in a NEW `expanded_bundles: BTreeSet<String>`, not in `collapsed`. (Disagreement with orchestrator D1 — reasoning: default polarity. See "Flagged disagreements".)

- **D3b — `expanded_bundles` lifecycle mirrors the `bundle_members` cache EXACTLY (DECISION, GAP-3).** Expand state and the member cache are two halves of the same per-bundle interaction, so they must be invalidated together — any divergence leaves a bundle "expanded" with a stale or cleared cache (or vice versa), producing phantom or missing member rows. Three sites, each mirroring the existing `bundle_members` handling:
  - **`set_rows` (`state.rs:282-301`)**: `self.expanded_bundles.clear()` alongside the existing `self.bundle_members.clear()` at `:300`. A full reload invalidates the bundle set entirely, so all expand state is dropped — even if a `Ready` cache entry somehow survived, no member rows would splice (the gate is empty).
  - **`merge_catalog_rows` (`state.rs:338-388`)**: prune `expanded_bundles` the **same prune-bundle-rows-only way** `bundle_members` is pruned (`:365-388`): snapshot via `std::mem::take` before `set_rows` clears it, then retain only keys whose `bundle_repo` still appears as a `kind == "bundle"` row in the post-merge `live_repos` set (W7 scope-to-bundle-rows discipline). Because `bundle_members` keys are `(scope, repo)` tuples while `expanded_bundles` keys are bundle-leaf path strings, the membership test is repo-derived; reuse the same `live_repos` bundle-repo set for the containment check.
  - **Scope toggle (`app.rs:366-388`)**: `state.expanded_bundles.clear()` alongside the existing `state.bundle_members.clear()` at `:381`. A scope swap changes the lock/install context and the cache key, so no expand state may leak across scopes.
  *Rationale:* one lifecycle, no drift; the gate and the cache are always cleared/pruned in lockstep.

- **D4 — `flatten_with_members` gates member splicing on `expanded_bundles`, orthogonally to `collapsed`.** The inner `flatten(tree, collapsed)` (`tree.rs:458`) is **UNCHANGED**: it gates GROUP children via `collapsed` only (`walk` at `:593-618`). `flatten_with_members` gains a NEW distinct parameter `expanded_bundles: &BTreeSet<String>`, **in ADDITION to** (never replacing) the existing `collapsed: &BTreeSet<String>`. The signature becomes `flatten_with_members(tree, collapsed, expanded_bundles, bundle_members, scope, rows)`. The member-splice gate is `expanded_bundles.contains(&leaf_key)`. The two sets are **orthogonal**: `collapsed` gates groups (consumed by the inner `flatten` call at `:496`), `expanded_bundles` gates member splicing (consumed in the post-pass at `:503-525`). Splice members only when the cache entry is present AND the bundle leaf's key ∈ `expanded_bundles`. A collapsed bundle leaf (key absent from `expanded_bundles`) renders the leaf with a `▸` arrow and zero member rows, even if the cache is `Ready`. A bundle leaf inside a `collapsed` group is hidden entirely (the inner `flatten` already dropped it), independent of `expanded_bundles`. *Rationale:* re-expand is instant (cache retained); collapse hides members without dropping the fetch; one set per concern keeps the two default polarities from colliding.

  **Exact call site:** `flattened()` (`state.rs:874-880`) passes BOTH `&self.collapsed` (as `effective_collapsed`, the query-aware ref at `:873`) AND `&self.expanded_bundles` to `flatten_with_members`, in addition to `&self.bundle_members`, `&self.scope_label`, `&self.rows`.

- **D5 — Key bindings on a bundle leaf.**
  - `→` (Expand): insert key into `expanded_bundles`; if no cache entry, also emit `LoadBundleMembers` (existing eager-`Loading` path). On a non-bundle leaf, `→` keeps today's `collapse_or_jump_to_parent`-free no-op-then-nothing behavior (`expand_selected` already no-ops on leaves).
  - `←` (Collapse): on an expanded bundle leaf, remove key from `expanded_bundles` (keep cache). On a collapsed bundle leaf, fall through to `collapse_or_jump_to_parent` (jump to parent group) — same as a normal leaf.
  - `Enter`: **keeps opening the detail pane** for a bundle leaf (a bundle is an inspectable artifact). It does NOT toggle collapse. *Decision + rationale below (D5a).*

- **D5a — Enter does NOT toggle bundle-leaf collapse; Enter on a member OPENS member detail (DECISION).** For groups, `Enter` toggles collapse because a group has no detail pane (`event.rs:301-305`: group → `toggle_collapse_selected`, else → `enter_detail`). A bundle leaf **has** a detail pane (repo URL via `o`, summary, version). Stealing `Enter` for collapse would remove the only way to inspect the bundle. `→`/`←` already own expand/collapse and are the ARIA-standard tree gestures the codebase uses (`event.rs:342-396`). *Decision:* `Enter` on a BUNDLE LEAF opens leaf detail (unchanged leaf behavior); `→`/`←` own bundle expand/collapse.

- **D5b — Enter on a member opens the member detail view (DECISION, GAP-2).** The member-detail rendering **already exists**: it is a **passive pane shown on selection**, not a modal opened by Enter. The detail pane is always visible beside the list; in tree mode `frame()` dispatches a selected `DisplayRow::Member` to `member_detail_lines_from_state` → `detail::detail_lines_for_member(node, parent_bundle_repo)` (`render.rs:628-643`, `:555-575`), so the member's identifier + metadata already render whenever the cursor sits on a member. What is missing is the **focus** transition: `Enter` on a member is currently a **silent no-op** because `event.rs:295-306` routes non-group selections to `state.enter_detail()`, and `enter_detail` (`state.rs:993-998`) early-returns unless `self.selected_row().is_some()` — and `selected_row()` is `None` for a member (`selected_row_index()` returns `None` for `DisplayRow::Member`, `state.rs:446`). So pressing `Enter` on a member cannot enter `Mode::Detail` (cannot scroll the pane with `↑`/`↓`, cannot `Esc` it). **Wiring:** the Enter arm (`event.rs:295`) must, in tree mode when the selection is a `DisplayRow::Member`, set `state.mode = Mode::Detail` (focus the always-visible member-detail pane) instead of being a no-op. Implement via a member-aware entry — either (a) a new `enter_member_detail()` helper on `TuiState` that sets `detail_scroll = 0` + `mode = Mode::Detail` when the selection is a member, or (b) relax `enter_detail`'s guard so a selected member (detail-renderable, even though `selected_row()` is `None`) also enters detail. Builder picks the seam; the observable contract is: Enter-on-member ⇒ `state.mode == Mode::Detail` AND the rendered detail pane is the member's (`detail_lines_for_member`), NOT a no-op, NOT the leaf/group path. This keeps `Enter`'s "open/inspect" meaning consistent for groups (toggle — no pane), leaves (detail), and members (detail). It does NOT mutate `expanded_bundles` or `collapsed`.

### Issue 2 — individually-actionable members

- **D6 — Add `member_repo: Option<String>` to `DisplayRow::Member`, populated from `MemberNode.member_repo`.** Placeholder rows (Loading/Failed/Offline, built in `tree.rs:547-578`) set `member_repo: None`. *Rationale:* the action layer needs the validated repo; `None` on placeholders makes "not actionable" representable in the type.

- **D7 — Member action is a SINGLE-TARGET direct action outside the index space.** When the cursor is on a `DisplayRow::Member` with `member_repo = Some` AND `marked` is empty, `i`/`u`/`d` synthesize the action from member fields. `rows`/`filtered`/`marked` are never touched. *Rationale:* honors `adr_projection_over_index` — members stay a pure projection; no phantom enters the index.

- **D8 — New typed `TuiAction::MemberAction { op: BatchOp, repo: String, kind: ArtifactKind }`.** Not an overload of `Batch { rows: Vec<usize> }` (which is `rows`-indexed and would require a phantom index). The app handler (`app.rs` dispatch, new arm near `:274`) resolves the tag and calls a new `perform_member`/`perform_member_uninstall`. *Rationale:* closed-enum exhaustiveness forces the handler to exist; avoids smuggling a non-index target through an index-shaped variant.

- **D8a — Member tag resolution + lock provenance (BINDING, GAP-4).** Standalone member install resolves the tag exactly as a leaf install would:
  - **(a) member matches a catalog row (`related`)** — `member_repo` equals some `rows[].repo`: reuse that row's `pinned_version`-or-`latest_tag` (same precedence as `perform`, `app.rs:1208-1213`), i.e. the same tag a leaf install of that row would use.
  - **(b) member NOT in the catalog**: tag = `"latest"`, identical to `perform`'s empty-`latest_tag` fallback (`app.rs:1212-1213`).

  This is intentional **general-node semantics**: the user asked for members to behave "like general nodes," and a general node installs latest. The member action MUST route through the **SAME `perform()` / declare_and_lock path a leaf install uses** — `perform_member` resolves the tag, then drives the identical declare → `write_config` → `relock_declared` → `single_entry_lock` → `install_all` → `persist` → `sync_config` seam (`app.rs:1216-1285`), with `kind` from the member (never `Bundle` — members-that-are-bundles are rejected at resolution, `bundle_members.rs:72-74`). **NO special double-declaration logic, NO new provenance code:** a standalone member install is an independent declaration in `grimoire.toml`, exactly like installing the same artifact from the flat catalog. Concretely, the cleanest realization is for `perform_member` to synthesize the same inputs `perform` already consumes (repo + resolved tag + kind) and reuse the existing seam — it does NOT add a provenance branch. Uninstall reuses the `perform_uninstall` seam shape (`app.rs:1097`) keyed on `(kind, repo)`. *Rationale:* one install seam, no forked logic; tag precedence matches the leaf install the user already knows; no new lock/declare code to test or get wrong.

  **Deferred future enhancement (not this phase):** installing a non-catalog member at the **bundle-pinned digest** (the member's exact ref from the bundle lock) instead of `"latest"`. That would require plumbing the member's pinned ref from `BundleMember` through `MemberNode` into `DisplayRow::Member` and into the action — out of scope here. The `"latest"` fallback for a non-catalog member is **by-design and predictable**, the same semantics as every other TUI install, not a defect.

- **D9 — State-gating per `member.state` is MANDATORY, mirroring leaf gating (DECISION, IMPROVEMENT-3).** `i` acts only when `state ∈ {NotInstalled, IntegrityMissing}`; `u` only when `state ∈ {Installed, Outdated, Modified}`; `d` only when `state` is installed (`{Installed, Outdated, Modified, IntegrityMissing}`). `MemberNode.state` is already lock-derived (`app.rs:317-329`). A gated-out keypress sets a status breadcrumb (e.g. `"already installed"`) and is otherwise a no-op (`TuiAction::None`). *Rationale:* matches the implicit leaf semantics; avoids a redundant install/no-op uninstall. The gate is a firm contract (C-7) and is unit-tested across the full op×state matrix — there is **no "no-gate" fallback**.

- **D10 — Security: act only on `member_repo = Some`.** Never synthesize an action from a placeholder member (Loading/Failed/Offline → `member_repo None`) or an unparseable repo. `member_node_from` already drops unparseable-id members (`bundle_members.rs:113-125`), so a live `MemberNode` always has `Some`. `perform_member` re-validates via `Identifier::new_registry(...).clone_with_tag(...)` at the boundary (same as `perform`). Labels stay sanitized at render (unchanged). *Rationale:* defense in depth; the repo string flows into an install Identifier and a filesystem materialize.

- **D11 — Member marking (spacebar) stays a no-op this phase (DECISION).** `marked` is `rows`-index-based; a member has no `rows` index. Single-target `i`/`u`/`d` fully satisfies the user's "install skills within a bundle alone" ask. *Decision:* keep `toggle_mark_selected` a no-op for `Member` (unchanged, `state.rs:477`). *Rejected alternative:* mapping a member's spacebar to mark the *related* catalog row — it is surprising (marks a different visual row), only works for `related` members, and conflates "this member" with "the standalone catalog artifact." Not worth the inconsistency. (Confirms orchestrator D11.)

### Flagged disagreements with orchestrator leanings

- **D1/D3a (refinement, not rejection):** the orchestrator proposed reusing `collapsed` for bundle-leaf keys. I keep `collapsed` for the *render arrow* read-back is fine, BUT the *member-splice gate* must use a separate `expanded_bundles` set because bundle leaves default **collapsed** (absent = hidden) while groups default **expanded** (absent = shown). One set cannot carry both default polarities without a per-row "is this a group or a bundle?" branch at every read site — fragile and easy to regress. A dedicated `expanded_bundles: BTreeSet<String>` is the same memory cost, keeps each set's default unambiguous, and stays a pure `&BTreeSet` flatten input. The `DisplayRow::Leaf.collapsed` field (D2) is then computed as `is_bundle && !expanded_bundles.contains(key)` so the render arrow is correct.
- **D5 Enter:** orchestrator left this open → decided NO toggle (D5a), preserving detail-pane access.
- **D11 marking:** orchestrator recommended no-op → confirmed (D11).

---

## Contracts

> Each contract is phrased for a unit test unless marked impure. Pure = headless unit-testable (tree/state/render/event pure fns). Impure = needs network/lock/filesystem or manual TUI.

### Issue 1 — collapsible bundle leaf

- **C-1 (pure) — bundle-leaf arrow glyph.** `tree_render_rows` over a flattened list containing a `DisplayRow::Leaf { is_bundle: true, collapsed: true, .. }` produces a `RenderRow` whose repo column begins (after indent) with `▸ ` (ASCII fallback `> `). With `collapsed: false`, it begins with `▾ ` (ASCII `v `). A `DisplayRow::Leaf { is_bundle: false, .. }` produces a repo column with **no** leading arrow (byte-identical to today). *Assert on the column string prefix.*

- **C-2 (pure) — default-collapsed.** For a freshly built tree where no bundle key has been added to `expanded_bundles`, every bundle leaf's `DisplayRow::Leaf.collapsed` is `true` and `flatten_with_members` splices **zero** member rows after it (even when its cache entry is `Ready`). *Assert: a `Ready` cache + empty `expanded_bundles` ⇒ no `Member` rows in output.*

- **C-2b (pure) — `collapsed` / `expanded_bundles` orthogonality.** The two sets are independent and each gates its own concern. Build a tree with a GROUP `g` containing a bundle leaf `b` (with a `Ready` cache). With `collapsed = {g.key}` AND `expanded_bundles = {b.key}`: `flatten_with_members` emits only the collapsed group header — the group is hidden so neither the bundle leaf nor its members appear (group collapse wins, independent of `expanded_bundles`). With `collapsed = {}` (group expanded) AND `expanded_bundles = {b.key}`: the group header, the bundle leaf, AND its members all appear (member splicing fires). With `collapsed = {}` AND `expanded_bundles = {}`: the group header and the bundle leaf appear but **zero** members. *Assert all three combinations: group collapse and member splicing behave correctly and independently. Confirms the inner `flatten(tree, collapsed)` is untouched and the member-splice post-pass keys only on `expanded_bundles`.*

- **C-2c (pure) — `expanded_bundles` lifecycle (GAP-3).** Expand state is cleared/pruned in lockstep with the `bundle_members` cache:
  - **After `set_rows`**: insert a `Ready` cache entry AND a bundle key into `expanded_bundles`, call `set_rows(...)`, then assert `expanded_bundles.is_empty()` — and that `flattened()` splices **0** member rows even if a `Ready` cache entry somehow remained (the gate is empty).
  - **After `merge_catalog_rows`**: seed `expanded_bundles` with two bundle keys, one whose repo survives the merge and one whose repo vanishes; assert the surviving key is retained and the vanished key is pruned (same survives/prunes split as the `bundle_members` prune test, `state.rs:2360-2385`).
  - **After a scope toggle**: seed `expanded_bundles` under scope A, toggle scope, assert `expanded_bundles.is_empty()` so no stale expand state leaks across scopes. *Assert each site mirrors the corresponding `bundle_members` lifecycle test.*

- **C-3 (pure) — flatten gating on `expanded_bundles`.** Given a `Ready` cache entry for `(scope, bundle_repo)` and the bundle key ∈ `expanded_bundles`: `flatten_with_members` splices the members immediately after the bundle leaf, in `Ready` order, at `depth = leaf_depth + 1`. Remove the key from `expanded_bundles` ⇒ same input produces zero member rows. *Assert both directions; assert splice position + depth.*

- **C-4 (pure) — `walk`/`flatten` thread `rows` and compute leaf fields.** `walk` populates `DisplayRow::Leaf.key` from `LeafNode.key`, `is_bundle` from `rows[l.row].kind == "bundle"`, and `collapsed` from `is_bundle && !expanded_bundles.contains(key)`. *Assert field values for a bundle leaf and a non-bundle leaf.*

- **C-5 (pure) — `→`/`←`/`Enter` on a bundle leaf (event layer).**
  - `→` (`TuiInput::Expand`) with cursor on a bundle leaf: inserts the leaf key into `expanded_bundles`; returns `TuiAction::LoadBundleMembers { row, bundle_repo }` when no cache entry exists, else `TuiAction::None`.
  - **Idempotent double-`→` (IMPROVEMENT-2):** `→` on an **already-expanded** bundle leaf whose cache entry is `Ready`: no duplicate insert into `expanded_bundles` (a `BTreeSet` insert of an existing key is a no-op), no re-emit of `LoadBundleMembers` (the existing no-retry gate at `event.rs:364-373` returns `false` for a `Ready` entry), so the action is `TuiAction::None` and `expanded_bundles` is unchanged (still contains the key exactly once). *Assert: pressing `→` twice on a `Ready` bundle leaf leaves `expanded_bundles` with one entry and the second press returns `TuiAction::None`.*
  - `←` (`TuiInput::Collapse`) on an **expanded** bundle leaf: removes the key from `expanded_bundles`, cache retained, returns `TuiAction::None`.
  - `←` on a **collapsed** bundle leaf: jumps to parent (cursor moves; key stays absent).
  - `Enter` on a bundle leaf: enters detail mode (`state.mode == Mode::Detail`), `expanded_bundles` unchanged. *Assert state mutations + returned action per gesture.*

- **C-5b (pure) — `Enter` on a member opens member detail (GAP-2).** With the cursor on a `DisplayRow::Member`, `handle(TuiInput::Enter)` transitions `state.mode` to `Mode::Detail` (NOT a no-op — the pre-fix bug), resets `detail_scroll` to 0, and leaves `expanded_bundles`/`collapsed` unchanged. The rendered detail pane for that selection is the member's (`render` dispatches the selected `DisplayRow::Member` to `member_detail_lines_from_state` → `detail_lines_for_member`), not the leaf/group path. Returned action is `TuiAction::None`. *Assert: `mode == Mode::Detail` after Enter on a member (regression guard against the silent no-op); detail pane lines come from the member detail builder; `expanded_bundles` empty/unchanged.*

### Issue 2 — actionable members

- **C-6 (pure) — member i/u/d single-target dispatch.** With cursor on a `DisplayRow::Member { member_repo: Some(repo), kind, state, .. }`, `marked` empty, and `state` permitting the op (per D9): `handle(TuiInput::Install/Update/Delete)` returns `TuiAction::MemberAction { op, repo, kind }`. With `member_repo: None` (placeholder): returns `TuiAction::None`. *Assert the variant + payload.*

- **C-7 (pure) — state-gating per `member.state`.** For each `(op, state)` pair: `i` yields a `MemberAction` only for `state ∈ {NotInstalled, IntegrityMissing}`; `u` only for `{Installed, Outdated, Modified}`; `d` only for `{Installed, Outdated, Modified, IntegrityMissing}`. A gated-out combination yields `TuiAction::None` (and a status breadcrumb). *Assert the full op×state matrix.*

- **C-8 (pure) — marks-win preserved (IMPROVEMENT-4).** With `marked` non-empty AND the cursor on a `DisplayRow::Member` (member_repo `Some`): `i`/`u`/`d` take the `batch()` path and return `TuiAction::Batch { op, rows }` over the marked set, NOT `MemberAction`. *Assert: marks non-empty + member cursor ⇒ a `TuiAction::Batch` over the marks (the `batch()`/`action_targets()` path), never `TuiAction::MemberAction`. This pins the P4.1 branch ordering: `MemberAction` fires only when `marked` is empty AND the cursor is a member; any non-empty mark falls through to `batch()`.*

- **C-9 (pure) — index model untouched.** After a `MemberAction` is dispatched (event layer), `selected_row_index()` still returns `None` for the member, `action_targets()` still returns `[]`, `toggle_mark_selected()` on the member is still a no-op, and `rows`/`filtered`/`marked` are unchanged. *Assert no member index materialized.*

- **C-10 (pure) — `DisplayRow::Member.member_repo` population.** `flatten_with_members` sets `member_repo` from `MemberNode.member_repo` for `Ready` members and `None` for Loading/Failed/Offline placeholders. *Assert per cache state.*

- **C-11 (impure — install seam) — member tag resolution + perform_member.** `perform_member(ctx, repo, kind, op)` builds an `Identifier` whose tag is: the matching catalog row's `pinned_version`-or-`latest_tag` when `repo ∈ rows[].repo`; else `"latest"`. It declares + relocks + materializes the single member via the **existing leaf seam** (declare → `relock_declared` → `single_entry_lock` → `install_all` → persist → `sync_config`), then recomputes states. **No new provenance branch:** `perform_member` reuses the same lock/declare seam a leaf install uses (`app.rs:1216-1285`) — a standalone member install is an independent `grimoire.toml` declaration, so there is no double-declaration or bundle-provenance code to test here; the only member-specific logic is `resolve_member_tag`. *Unit-test the pure tag-resolution helper `resolve_member_tag(repo, rows) -> String` standalone (pure); the full perform_member is impure (needs `OciAccess` + filesystem) — cover via the existing `run_batch` test harness pattern (`app.rs:1898`), asserting the install record + lock entry + materialized file appear exactly as a leaf install of the same repo would (the seam is shared).*

- **C-12 (pure) — security: no action from placeholder/unvalidated member.** A `DisplayRow::Member` with `member_repo: None` never yields a `MemberAction` (C-6). `resolve_member_tag` and `perform_member` are only reached with a `Some` repo. A repo string that fails `Identifier::parse`/`split_repo` at the boundary returns an `Err` from `perform_member` (status breadcrumb, no install). *Assert: `None` repo ⇒ `None` action; malformed repo ⇒ `perform_member` errors without materializing.*
  - **Defense-in-depth: `Some` repo that fails `split_repo` (IMPROVEMENT-6).** Even though a live `MemberNode` always carries a parseable `member_repo` (`member_node_from` drops unparseable ids, `bundle_members.rs:113-125`), `perform_member` must not assume it. A `member_repo` that is `Some` but has **no `/` separator** (so `split_repo` returns `None`, `app.rs:1405-1407`) must produce **no action / a handled error — never a panic** (no `.unwrap()`/`.expect()` on `split_repo`'s `Option`; return an `anyhow::Err` exactly as `perform`/`perform_uninstall` do via `.ok_or_else(...)`, `app.rs:1098-1099`/`1201-1202`). *Assert: `perform_member` with a separator-less `Some` repo (e.g. `"noslash"`) returns `Err` (status breadcrumb) and materializes nothing — no panic.*

- **C-13 (impure — manual/acceptance) — offline degrade unchanged.** Member i/u: when `ctx.offline`, the existing `run_batch` offline guard analog applies — `perform_member` for install/update sets `"offline — cannot install/update"` and does nothing; `d` (uninstall) works offline (local). *Manual TUI check.*

---

## Workplan

> Contract-first TDD: stub → specify → implement. Pure-logic phases (P1–P3) land before the impure install-dispatch phase (P4). The field-addition refactor (P1) is behavior-preserving and isolated.

### P0 — Branch + safety net (no code)

- [ ] On a feature branch (not `main`). Confirm `task rust:verify` green at HEAD (baseline).

### P1 — Additive type changes + stubs (behavior-preserving) — `cargo check`

Files: `src/tui/tree.rs`, `src/tui/state.rs`, `src/tui/event.rs`, `src/tui/app.rs`.

- [ ] **P1.1** Add `key: String, is_bundle: bool, collapsed: bool` to `DisplayRow::Leaf` (`tree.rs:167`). Thread `rows: &[TuiRow]` + `expanded_bundles: &BTreeSet<String>` into `walk` and `flatten`/`flatten_with_members`; populate the three fields in the `Node::Leaf` arm (`tree.rs:610`). Keep all behavior identical (default `expanded_bundles` empty → today's splice gate must still produce members for currently-expanded bundles via the new gate — see P3).
- [ ] **P1.2** Add `member_repo: Option<String>` to `DisplayRow::Member` (`tree.rs:189`); set it at the three construction sites in `flatten_with_members` (`tree.rs:533` Ready → `m.member_repo.clone()`; `:547/:560/:571` placeholders → `None`).
- [ ] **P1.3** Add `expanded_bundles: BTreeSet<String>` field to `TuiState` (`state.rs`, near `collapsed` at `:190`); default empty. Wire its lifecycle to mirror `bundle_members` (D3b, GAP-3): clear in `set_rows` (`:300`); prune in `merge_catalog_rows` (snapshot-before-`set_rows`, retain bundle-repo-live keys, `:365-388`); clear on scope toggle (`app.rs:381`).
- [ ] **P1.4** Add `TuiAction::MemberAction { op: BatchOp, repo: String, kind: ArtifactKind }` (`event.rs:89`). Stub the app dispatch arm with `unimplemented!()`.
- [ ] **P1.5** Stub `perform_member`, `perform_member_uninstall`, and the pure `resolve_member_tag(repo, rows) -> String` in `app.rs` with `unimplemented!()`.
- **Gate:** `cargo check` passes; every `DisplayRow`/`TuiAction` match site compiles (most use `..`).

### P2 — Specify tests (fail with `unimplemented!`/stub)

Files: inline `#[cfg(test)]` in `tree.rs`, `render.rs`, `state.rs`, `event.rs`, `app.rs`.

- [ ] **P2.1** Pure tests for C-1…C-10 (incl. C-2b, C-2c, C-5b), C-12 (tree flatten/glyph/gating/orthogonality, `expanded_bundles` lifecycle, event dispatch incl. Enter-on-member + idempotent double-`→`, state index-isolation).
- [ ] **P2.2** Pure test for `resolve_member_tag` (C-11 pure half); extend the `run_batch`-style harness (`app.rs:1898`) for `perform_member` (C-11 impure half) and C-12 malformed-repo.
- **Gate:** tests compile and FAIL against stubs.

### P3 — Implement Issue 1 (collapsible bundle leaf) — pure — `task rust:verify`

Files: `src/tui/tree.rs`, `src/tui/render.rs`, `src/tui/state.rs`, `src/tui/event.rs`.

- [ ] **P3.1** `flatten_with_members` (`tree.rs:503-525`): change the splice gate from "cache entry present" to "cache entry present AND `expanded_bundles.contains(&leaf_key)`" (C-3, C-2). The leaf's `key` is now on the `DisplayRow::Leaf` (P1.1).
- [ ] **P3.2** `tree_render_rows` Leaf arm (`render.rs:422`): when `is_bundle`, prefix `▸`/`▾` (ASCII `>`/`v`) per `collapsed`; non-bundle leaves unchanged (C-1).
- [ ] **P3.3** `state.rs`: thread `expanded_bundles` into the `flattened()` call (`:874-880`) — pass BOTH `&self.collapsed` (as `effective_collapsed`) AND `&self.expanded_bundles`. Add `expand_bundle_leaf(key)` / `collapse_bundle_leaf(key)` helpers (insert/remove on `expanded_bundles`, re-clamp). Add the `enter_member_detail()` helper (or relaxed `enter_detail` guard) for C-5b/GAP-2. The lifecycle clears/prunes (D3b) land in P1.3.
- [ ] **P3.4** `event.rs` Expand arm (`:342`): on a bundle leaf, insert the leaf `key` into `expanded_bundles` before the existing `LoadBundleMembers` logic (C-5 `→`). Collapse arm (`:390`): on an expanded bundle leaf, remove the key (cache retained) instead of jumping to parent; on a collapsed bundle leaf, keep the `collapse_or_jump_to_parent` fall-through (C-5 `←`). Enter arm (`:295`): bundle leaves are not groups, so they already `enter_detail` (C-5 `Enter`); **add a member branch** so that in tree mode a selected `DisplayRow::Member` enters detail focus (`state.mode = Mode::Detail`, `detail_scroll = 0`) instead of the current silent no-op — via a new `enter_member_detail()` helper on `TuiState` (or a relaxed `enter_detail` guard), since `selected_row()` is `None` for a member (C-5b, GAP-2). The member detail pane already renders passively (`render.rs:628-643`); this only wires the focus transition.
- **Gate:** C-1…C-5, C-10 pass; `task rust:verify` green.

### P4 — Implement Issue 2 (actionable members) — pure event + impure install

Files: `src/tui/event.rs`, `src/tui/state.rs`, `src/tui/app.rs`, `src/tui/render.rs`.

> **render.rs note (IMPROVEMENT-5):** the `DisplayRow::Member` render arm (`render.rs:452-483`) destructures the variant; once `member_repo` is added (P1.2), this arm must pattern-match the new field — bind it or include it under the existing `..` rest pattern (the arm already uses `..` at `:458`, so it compiles unchanged, but confirm the build does not miss it). No display change: `member_repo` is not rendered.

- [ ] **P4.1** (pure) `event.rs` `handle_browse` i/u/d arms (`:321-323`): the new code returns `TuiAction::MemberAction { op, repo, kind }` **DIRECTLY** (it does **NOT** call `batch()`) **ONLY when** `marked` is empty **AND** the cursor is on a `DisplayRow::Member` with `member_repo = Some(repo)` (and `state` permits the op per the D9 gate). In **every other case** it **falls through to the existing `batch()` / `action_targets()` path** so marks-win is preserved — specifically: `marked` non-empty (regardless of cursor type, including a member cursor) ⇒ `batch()` (which yields `TuiAction::Batch` over the marks); cursor not a member ⇒ `batch()`; `member_repo = None` placeholder ⇒ `batch()` (whose `action_targets()` contributes nothing for a member, yielding `TuiAction::None`). A gated-out (state-disallowed) member op ⇒ status breadcrumb + `TuiAction::None`, NOT `batch()` (C-7). Add a small read-only accessor for the selected member's fields if needed (no index materialization — C-9). *This ordering — "marks empty AND member cursor" as the only `MemberAction` branch, everything else to `batch()` — is what C-8 pins.*
- [ ] **P4.2** (pure) Implement `resolve_member_tag(repo, &[TuiRow]) -> String` (D8a, C-11 pure): matching-row tag-or-`"latest"`.
- [ ] **P4.3** (impure) Implement `perform_member` / `perform_member_uninstall` (`app.rs`): split repo, resolve tag (P4.2), build `Identifier`, reuse the declare → relock → `single_entry_lock` → `install_all` → persist → `sync_config` seam (`app.rs:1216-1285`) for install/update, and the `perform_uninstall` seam (`app.rs:1097`) for delete. Validate `member_repo` at the boundary (C-12).
- [ ] **P4.4** (impure) Add the `TuiAction::MemberAction` dispatch arm (`app.rs:274` area): offline guard for install/update (mirror `run_batch:1040`), call the right perform fn, then `recompute_states` (`app.rs:1076`) so the member badge + parent-bundle rollup flip, and `recheck_rows` for the matching catalog row if `related`.
- **Gate:** C-6…C-9, C-11, C-12 pass; `task rust:verify` green.

### P5 — Review-fix loop + Codex gate

- [ ] **P5.1** Canonical Review-Fix Loop on the diff (high → up to 3 rounds). Perspectives: spec-compliance, **security (member repo → install Identifier)**, correctness, behavior-preservation (P1 field additions), quality.
- [ ] **P5.2** Codex cross-model adversarial pass (codex=on): focus on the member-action boundary (untrusted repo into a filesystem materialize), the `expanded_bundles` default-polarity gate, and index-isolation (no phantom in `rows`/`marked`).
- [ ] **P5.3** Catalog drift review per `catalog/README.md` (CLI surface unchanged; TUI keybinding behavior changed — check `grim-usage` TUI key references). Update `adr_projection_over_index.md` Consequences (Phase 3 now adds the separate member-targeting path it anticipated).
- **Gate:** no actionable findings; `task verify` green; deferred findings documented.

---

## Test plan

### Unit tests (pure — headless)

| Contract | Test focus | Location |
|----------|-----------|----------|
| C-1 | bundle leaf renders `▸`/`▾` (+ASCII); non-bundle leaf no arrow | `render.rs` |
| C-2 | default-collapsed: empty `expanded_bundles` + `Ready` cache ⇒ 0 member rows | `tree.rs` |
| C-2b | `collapsed`/`expanded_bundles` orthogonality (3 combinations) | `tree.rs` |
| C-2c | `expanded_bundles` lifecycle: clear on `set_rows`/scope-toggle, prune on `merge_catalog_rows` | `state.rs` |
| C-3 | splice iff key ∈ `expanded_bundles`; order + depth; both directions | `tree.rs` |
| C-4 | `walk` computes `key`/`is_bundle`/`collapsed` for bundle vs non-bundle | `tree.rs` |
| C-5 | `→`/`←`/`Enter` event outcomes on a bundle leaf (expand/collapse/detail) + idempotent double-`→` | `event.rs` |
| C-5b | `Enter` on a member ⇒ `Mode::Detail` (member detail), not a no-op | `event.rs` |
| C-6 | member i/u/d ⇒ `MemberAction`; `None` repo ⇒ `None` | `event.rs` |
| C-7 | full op×state gate matrix (mandatory gate) | `event.rs` |
| C-8 | marks-win: member cursor + marks ⇒ `Batch`, not `MemberAction` | `event.rs` |
| C-9 | index isolation after member dispatch (`selected_row_index`/`action_targets`/marks) | `state.rs` |
| C-10 | `member_repo` population per cache state | `tree.rs` |
| C-11 (pure) | `resolve_member_tag`: related-row tag vs `"latest"` | `app.rs` |
| C-12 | `None` repo ⇒ no action; malformed repo ⇒ `perform_member` errors, no materialize; `Some` repo without `/` ⇒ handled `Err`, no panic | `event.rs` + `app.rs` |

### Impure / harness tests

- **C-11 (impure)** — `perform_member` install via the in-memory `OciAccess` + temp-workspace harness used by `run_batch_on_a_bundle_recomputes_member_row_states` (`app.rs:1898`): install a single member, assert the install record + lock entry + materialized file appear, and the member badge flips to `Installed`.

### Manual / acceptance checks (`grim tui`)

- [ ] Open the tree, find an installed bundle: it shows a `▸` and **no** member rows (default-collapsed). (Issue 1 core ask.)
- [ ] `→` on the bundle leaf: arrow flips to `▾`, members appear (`(via bundle)` badge), from the lock snapshot offline / fetched online.
- [ ] `←` on the expanded bundle: members disappear, arrow `▸`, re-`→` is instant (cache retained).
- [ ] `Enter` on the bundle leaf: detail pane opens (not collapse).
- [ ] `Enter` on a member: the member detail pane focuses (enters detail mode — scrollable, `Esc` returns); NOT a silent no-op. (GAP-2.)
- [ ] Cursor onto a not-installed member, press `i`: that member installs alone; badge → `Installed`; parent-bundle rollup updates. (Issue 2 core ask.)
- [ ] `u` on an outdated member updates it; `d` on an installed member deletes it.
- [ ] `space` on a member is still a no-op; with catalog rows marked, `i` acts on the marks (marks-win), not the member.
- [ ] Offline: `i`/`u` on a member shows `"offline — cannot install/update"`; `d` works.

---

## Security

> Cites `quality-security.md` / `quality-core.md`: validate external input at boundaries; least privilege; defense in depth. `BundleMember.id`/`.name` are registry-controlled (untrusted), arriving over the network or from a lock written from such data.

| Threat | Vector | Neutralization | Citation |
|--------|--------|----------------|----------|
| Untrusted member repo → install Identifier | `member_repo` flows into `Identifier::new_registry` + a filesystem materialize | `member_node_from` already validates via `Identifier::parse` (`bundle_members.rs:114`), dropping unparseable ids; `perform_member` re-validates `split_repo` + `Identifier::new_registry` at the boundary and errors (status breadcrumb) on failure — never materializes from a bad repo (C-12) | quality-security input-at-boundary; arch three-layer errors |
| Action from a placeholder member | Loading/Failed/Offline rows | `member_repo: None` on placeholders (D6); event layer yields `TuiAction::None` for `None` repo (C-6/C-12) | quality-core: validate at boundary |
| Path-traversal / control-char in member name → install path | a `../`-style or control-char member name | name (`label`) is display-only and sanitized at render (`sanitize_member_label`, unchanged); the install path is derived from `member_repo` (Identifier-validated), NOT the label; `SkillName::parse` at the resolver data boundary already fails closed for the standalone install | CWE-22; `subsystem-file-structure.md` path-containment guard |
| Control chars / ANSI / bidi in member label | terminal injection in the new arrow-prefixed row | unchanged — label still passes through `sanitize_member_label` before `fit()` (`render.rs:463`); the new arrow prefix is a static literal | quality-security; Phase 2 C-2 |
| Phantom index into `rows`/`marked` | a member smuggled into the index space | `MemberAction` carries `repo`/`kind`, never a `rows` index; `selected_row_index`/`action_targets`/marks unchanged for members (C-9); `adr_projection_over_index` invariant held | `adr_projection_over_index.md` |
| Member install bypassing scope/offline policy | install from the TUI | reuses the same declare → relock → materialize seam as `perform` (scope-aware config flock, install-state persist) + the offline guard (D-impure P4.4) | `subsystem-cli.md` config forwarding; quality-rust async |

**Tag resolution is by-design, not a security gap (GAP-4).** A non-catalog member installs at `"latest"` — the identical, predictable semantics of every other TUI install (`perform`'s empty-`latest_tag` fallback, `app.rs:1212-1213`); a catalog-matched member reuses that row's resolved tag. This is **not** unsafe and is **not** a digest-pinning regression: standalone install is an independent declaration, exactly like installing the artifact from the flat catalog, and goes through the same Identifier-validated boundary above. Installing a non-catalog member at the bundle-pinned digest is a **deferred future enhancement** (would require plumbing the member's pinned ref from `BundleMember` → `MemberNode` → `DisplayRow::Member`), not a current defect.

---

## Files to modify

| File | Action | Description |
|------|--------|-------------|
| `src/tui/tree.rs` | Modify | `DisplayRow::Leaf` gains `key`/`is_bundle`/`collapsed`; `DisplayRow::Member` gains `member_repo`; inner `flatten(tree, collapsed)` UNCHANGED (gates groups); `walk` threads `rows`; `flatten_with_members` gains a distinct `expanded_bundles: &BTreeSet<String>` param (in addition to `collapsed`) and gates the member splice on it — the two sets stay orthogonal |
| `src/tui/render.rs` | Modify | Leaf arm renders `▸`/`▾` (ASCII fallback) for bundle leaves; member arm (`:452-483`) pattern-matches the new `member_repo` field (under `..` or bound — no display change); member detail pane already renders passively |
| `src/tui/state.rs` | Modify | `expanded_bundles: BTreeSet<String>` field (lifecycle mirrors `bundle_members`: clear in `set_rows`, prune in `merge_catalog_rows`, clear on scope toggle); `expand_bundle_leaf`/`collapse_bundle_leaf` helpers; `enter_member_detail` (or relaxed `enter_detail` guard) for Enter-on-member; `flattened()` passes BOTH `collapsed` AND `expanded_bundles`; read-only selected-member accessor |
| `src/tui/event.rs` | Modify | `TuiAction::MemberAction`; Expand/Collapse arms handle bundle leaves (idempotent double-`→`); Enter arm focuses member detail; i/u/d arms return `MemberAction` ONLY when marks-empty + member cursor, else fall through to `batch()` (marks-win + mandatory D9 gate preserved) |
| `src/tui/app.rs` | Modify | `MemberAction` dispatch arm (offline guard, recompute, recheck); `perform_member`/`perform_member_uninstall`; pure `resolve_member_tag` |
| `.agents/adr/adr_projection_over_index.md` | Modify | Consequences: Phase 3 adds the anticipated separate member-targeting path (`MemberAction`, never into `rows`/`marked`) |

---

## Risks

| Risk | Mitigation |
|------|------------|
| `expanded_bundles` vs `collapsed` default-polarity confusion (HIGH) | dedicated set (D3a); `collapsed` semantics for groups untouched; C-2/C-3 lock both defaults |
| Untrusted member repo → install (HIGH) | Identifier validation at two boundaries; Codex gate on this surface; C-12 |
| Index-model pollution via member action (HIGH) | `MemberAction` carries no index; C-9 pins isolation; ADR updated |
| Field additions break exhaustive matches (MEDIUM) | additive; most sites use `..`; `cargo check` gate at P1 |
| Member tag resolution wrong (member installs a different version) (MEDIUM) | reuse leaf precedence; `resolve_member_tag` unit-tested (C-11); manual check |
| Eager-`Loading` cache + collapse interaction (MEDIUM) | splice gate is `expanded_bundles`, independent of cache state (C-2) |

---

## Deferred / Follow-ups

- Member marking (spacebar) + multi-member batch — out of scope (D11); single-target satisfies the ask.
- Persisting `expanded_bundles` across sessions — out of scope (matches `collapsed`, which is also session-ephemeral).
- Re-finding a member's exact cursor position after a reshape (`restore_selection` TODO at `state.rs:791`) — pre-existing, not introduced here.
- **Bundle-pinned-digest standalone install** (GAP-4) — a non-catalog member installs at `"latest"` this phase (by-design, matches every TUI install). Installing at the member's bundle-locked digest is deferred; it would require plumbing the pinned ref from `BundleMember` → `MemberNode` → `DisplayRow::Member` → the action.

---

## Progress Log

| Date | Update |
|------|--------|
| 2026-06-20 | Plan authored from the supplied verified map + a full read of `tree.rs`/`render.rs`/`state.rs`/`event.rs`/`app.rs`/`bundle_members.rs`/`bundle_member_fetch.rs` at HEAD. Map corrected (Member variant + machinery already shipped in Phase 2; line numbers drifted). Decisions D1–D11 formalized; D3a refines D1 (separate `expanded_bundles` set); D5a + D11 resolved. 13 contracts, 6 phases. |
| 2026-06-20 | **Revision 1**: completeness-critic gaps folded in (see `## Revision 1`). Added D3b (lifecycle), D5b (Enter-on-member); extended D4/D8a/D9; added contracts C-2b, C-2c, C-5b; extended C-5/C-8/C-11/C-12. Now **16 contracts, 6 phases**. |

---

## Revision 1

Folded in 4 blocking gaps + 6 improvements raised by the completeness critic (auditable trail; no redesign — only tightening/extension). Verified against HEAD source before each change.

**Gaps closed:**

- **GAP-1 (signature orthogonality)** — Map item 3 + D4 now state explicitly: inner `flatten(tree, collapsed)` stays UNCHANGED (gates groups via `collapsed`); `flatten_with_members` gains a NEW distinct `expanded_bundles: &BTreeSet<String>` param IN ADDITION to `collapsed`; member splice gate is `expanded_bundles.contains(&leaf_key)`. Exact call site pinned: `flattened()` (`state.rs:874-880`) passes BOTH sets. Added contract **C-2b** (orthogonality, 3-combination unit test).
- **GAP-2 (Enter on a member)** — Verified the member detail is a passive selection-driven pane (`render.rs:628-643`), but `Enter` is a silent no-op because `enter_detail` (`state.rs:993`) requires `selected_row().is_some()` which is `None` for a member. New **D5b** + workplan wiring (`enter_member_detail` / relaxed guard so a member enters `Mode::Detail`). Added contract **C-5b** + manual check; D5a still keeps Enter-on-bundle-leaf opening leaf detail with →/← owning expand/collapse.
- **GAP-3 (lifecycle of `expanded_bundles`)** — New **D3b**: mirrors the `bundle_members` cache EXACTLY — clear in `set_rows` (`state.rs:300`), prune in `merge_catalog_rows` the bundle-rows-only way (`:365-388`), clear on scope toggle (`app.rs:381`). Added contract **C-2c** (clear-on-set_rows so 0 members splice; prune survives/drops by repo; no cross-scope leak). Wired into P1.3/P3.3.
- **GAP-4 (member tag + lock provenance)** — D8a made binding: (a) catalog-matched member reuses the row's `pinned_version`/`latest_tag`; (b) non-catalog member = `"latest"` (general-node semantics, same as `perform`'s empty-tag fallback). Routes through the SAME `perform()`/declare_and_lock seam — no double-declaration, no new provenance code; standalone install is an independent `grimoire.toml` declaration. Bundle-pinned-digest install documented as a DEFERRED enhancement (Decisions + Security + Follow-ups); removed any "unsafe-fallback" framing. C-11 notes `perform_member` reuses the leaf lock/declare seam (no new provenance branch to test).

**Improvements folded in:**

- **IMP-1** — see GAP-2 (Enter-on-member opens member detail).
- **IMP-2** — extended **C-5** with the idempotent double-`→` case (no duplicate `expanded_bundles` insert, no re-emit of `LoadBundleMembers` on a `Ready` cache ⇒ `TuiAction::None`) + unit test.
- **IMP-3** — committed the per-member state-gating matrix as firm contract **C-7** (mandatory); deleted the D9 "optional gate / no-gate fallback" footnote.
- **IMP-4** — P4.1 now states the new code returns `TuiAction::MemberAction{...}` DIRECTLY (not `batch()`) ONLY when marks-empty AND cursor is a `DisplayRow::Member` with `member_repo = Some`; everything else falls through to `batch()`. **C-8** strengthened: marks non-empty + member cursor ⇒ `Batch` over the marks, NOT `MemberAction`.
- **IMP-5** — added `src/tui/render.rs` to the P4 file list with a note that the `DisplayRow::Member` render arm must pattern-match the new `member_repo` field (under `..` or bound).
- **IMP-6** — extended security contract **C-12** with a defense-in-depth case: a `member_repo` that is `Some` but fails `split_repo` (no `/`) in `perform_member` must return a handled `Err` (no action), never a panic, + unit test.

Contract count: 13 → **16** (added C-2b, C-2c, C-5b). Phase count: **6** (unchanged).
