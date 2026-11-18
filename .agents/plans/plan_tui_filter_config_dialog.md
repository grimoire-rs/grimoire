# Plan: TUI filter & sort popup (#355) and config dialog

## Status

- **Plan:** plan_tui_filter_config_dialog
- **Active phase:** 2 — Phase B: config dialog
- **Step:** awaiting /finalize
- **Last update:** 2026-11-18 (after 48dc6667: feat(tui): manage registries from the config dialog)

---

## Context

`grim tui` filters and sorts through single-key cycles (`s`, `S`, `h`, `f`). They
are fast but hard to discover, and `f` can show only one kind at a time
([#355](https://github.com/grimoire-rs/grimoire/issues/355); its blocker
[#326](https://github.com/grimoire-rs/grimoire/issues/326) closed 2026-11-13).
Separately, grim's own config can only be changed from the shell
(`grim config …`) or the VS Code extension's settings page; the TUI has no way
to edit it.

Outcome: two popups in `grim tui`.

- **`F` Filter & sort** — kinds as a multi-select, sort mode, direction,
  deprecated. Applies live. Closes #355.
- **`C` Config** — a form over the same rows as `grim config list --all`, plus
  registry management, mirroring the extension's settings page. It edits the
  config file of the scope already selected with `g` (owner ruling: no scope
  switch inside the dialog, no new CLI surface).

No schema, CLI or JSON change. The dialog writes only what `grim config`
already writes, through the same code.

## Delivery

Two phases, two branches, two PRs. Phase B stacks on A because both use one
new widget file.

- A: `feat/tui-filter-sort-popup` (3 commits, about 600 lines)
- B: `feat/tui-config-dialog` (4 commits, about 1,700 lines)

Step 0 (before code): copy this plan to
`.agents/plans/plan_tui_filter_config_dialog.md` (project convention), and open
a GitHub issue for Phase B so its PR has something to close.

Execution: sonnet workers with strict briefs, one step at a time in this
worktree (no parallel worktrees, one build dir); I review each diff. Test
first for every step. One sonnet docs pass per phase after the last code
commit. Nothing is pushed; the owner pushes.

---

## Phase A — Filter & sort popup

### A1. Kind filter becomes a set (`src/tui/state.rs`)

- `kind_filter: Option<ArtifactKind>` → `Vec<ArtifactKind>`, kept in
  `ArtifactKind::ALL` order; empty means all kinds.
- `search_query()` passes `&self.kind_filter` to
  `SearchQuery::parse_with_kinds` (`src/catalog/search_match.rs`), which already
  takes a slice and ORs the kinds.
- New `set_kind_filter(Vec)` holding the tail now inside `cycle_kind_filter`
  (reset detail, refilter, move selection); new `toggle_kind(k)`.
- New `step_sort(delta)` over `[None, Name, Updated, Rating, Downloads]`;
  `cycle_sort` becomes `step_sort(1)`. New `set_sort_mode(Option<SortMode>)`
  for the popup's radio rows.
- Direction and deprecated reuse `toggle_sort_direction` and
  `toggle_hide_deprecated`, so keys and popup cannot disagree.
- Existing tests to adapt: the `kind_filter_*` tests in `state.rs`, one in
  `event.rs`, the title test in `render.rs` (`None` → `vec![]`, `Some(k)` →
  `vec![k]`).

### A2. Shared widget: new `src/tui/list_popup.rs`

The only shared piece; no form framework.

- `ListPopupView { title, rows: Vec<PopupRow>, cursor, notes, message, hint, area }`
- `PopupRow { mark: Mark::{None, Check(bool), Radio(bool)}, label, value, header, muted, modified, editing }`
- Pure geometry: `anchored(anchor, w, h, screen)` and `centered(...)` (reusing
  `centered_area_rows` in `render.rs`).
- `draw()`: `Clear`, a `Block` padded with `confirm::PAD` / `PAD_ROWS`, a
  scrolling `List`, note and message lines, the hint on the bottom border with
  `HINT_INSET`. Theme roles only (raw colours are banned outside `theme.rs`).
- `RenderModel` gains `list_popup: Option<ListPopupView>`, filled in `frame()`;
  `draw()` order becomes help → list popup → picker → confirm → `paint_canvas`.

### A3. The popup

- `Mode::FilterSort`, `filter_cursor: usize`, a fixed row list
  `FilterRow::{Kind(k)×7, Sort(Option<SortMode>)×5, Direction, Deprecated}`.
- Open with `F` from `handle_browse` (free; uppercase survives `map_key` as `S`
  does). New `handle_filter_sort` arm in `event.rs::handle`.
- Keys: `↑↓ j k Home End` move; `space`/`Enter` toggle a kind, pick a sort,
  flip the direction, toggle deprecated; `Esc` or `F` closes (changes stay,
  they applied live); `q` quits.
- Body: `Kinds` 7 checkboxes (kinds absent from the catalog are muted, still
  toggleable), `Sort` 5 radio rows (default, name, updated, rating, downloads),
  `Order ‹asc|desc›` (muted `—` on the default sort), `[ ] hide deprecated`.
- Placement: right-aligned inside the catalog area, one row below its top
  border, so the live title stays visible. Clamped to the screen; the list
  scrolls to the cursor on short terminals. No size gate (view-only toggles).
- Help overlay is exactly 24 rows at 80×24, so no new row: add `F` to the
  existing `g · t · s · f` entry in `help_entries()` without widening it. Hint
  tiers: `s sort` → `s/F sort`.

### A4. Docs (after code)

`docs/src/content/docs/browse.md` (step 3, `f` row, new `F` row),
`commands.md` `grim tui` section, the `grim tui` row in
`.claude/rules/subsystem-cli-commands.md`. Drift review of
`catalog/skills/grim-usage` (expected: no change). No hand edit of
`CHANGELOG.md` (git-cliff).

### A5. Commits

1. `feat(tui): filter the catalog by several kinds`
2. `feat(tui): open a filter and sort popup with F`
3. `docs(tui): document the filter and sort popup`

Tests, written first: `state.rs` — `toggle_kind_keeps_all_order_and_removes_on_second_press`,
`two_kinds_show_rows_of_both`, `kind_label_names_three_kinds_then_counts`,
`f_from_several_kinds_steps_past_the_last`,
`step_sort_back_from_default_lands_on_downloads`. `event.rs` —
`shift_f_opens_filter_sort`, `space_toggles_kind_under_cursor`,
`enter_on_sort_row_picks_that_mode`, `esc_closes_and_keeps_filter`,
`q_in_filter_popup_quits`. `render.rs` (TestBackend) —
`filter_popup_pads_rows_inside_border`,
`filter_popup_leaves_catalog_title_visible`,
`filter_popup_scrolls_to_cursor_on_short_terminal`,
`catalog_title_counts_four_kinds`. `list_popup.rs` —
`anchored_area_clamps_to_screen`.

---

## Phase B — Config dialog

### B0. Write seam: refactor commit, no test edits (`src/command/config.rs`)

Exactly one write path. The TUI calls the `grim config` mutation and
validation code; nothing is reimplemented.

- Make `pub(crate)` unchanged: `ParsedKey`, `parse_key`, `apply_set`,
  `apply_unset`, `collect_entries`, `apply_plugin_set`, `apply_plugin_unset`,
  `plugin_entries`.
- `commit_config` takes `(config_path, set, options, registries)` instead of a
  `ResolvedScope` (it reads only those two fields). `TuiContext` cannot build a
  `ResolvedScope` honestly: its options and registries are resolved views, not
  the declared file.
- Extract from `run_registry_add`: `RegistryDraft` struct, `check_draft`,
  `add_registry(&mut Vec<RegistryConfig>, RegistryDraft, &Invocation)`.
- Extract from `run_registry_set`: `RegistryEdit` struct, `check_edit`,
  `edit_registry(...)`.
- Remove and set-default need no extraction: `apply_unset("registry.<alias>")`
  and `apply_set("registry.<alias>.default", …)` already do them.
- Split `warn_config_write_does_not_disarm` into `disarm_note(before, after) ->
  Option<String>` plus the existing `tracing::warn!` call, so the dialog can
  show the text (a `tracing` warning only reaches `tui.log`).
- `ConfigEntry` (`src/api/config_report.rs`) gains `Clone`.
- First check the existing `config.rs` tests cover duplicate alias, index plus
  insecure, `--clear-include` and a kind switch; add a characterization test in
  its own commit if one is missing.

### B1. Dialog state: new pure file `src/tui/config_dialog.rs` (no I/O, no ratatui)

```rust
pub(crate) struct ConfigDialog { scope, path, exists, entries: Vec<ConfigEntry>,
    registries: Vec<RegistryConfig>, cursor, advanced, view: View,
    pending_remove: Option<String>, message: Option<Note> }
enum View { List, Text{key, buf}, Pick{key, picked, cursor, single: bool},
    Registry(RegistryForm), Patterns{field, items, cursor, buf: Option<String>} }
pub(crate) enum ConfigEdit { Set{key, value}, Unset{key},
    AddRegistry(RegistryDraft), EditRegistry{alias, edit: RegistryEdit} }
```

`ConfigDialog::handle(TuiInput)` returns `Stay | Close | Write(ConfigEdit)`.
`state.rs` gains `Mode::Config` and `config: Option<ConfigDialog>`; `TuiAction`
gains `OpenConfig` and `ConfigWrite(ConfigEdit)`; `event.rs` gains one arm.

### B2. The popup

- Open with `C`. The app reads the current scope's file with the existing
  `load_scope_declaration` (`src/tui/app.rs`), builds rows with
  `collect_entries(true, …)`, and adds `plugin_entries` at project scope only.
  Opening never writes.
- Title: `Config · project · ./grimoire.toml` (path cut from the left). If the
  global file does not exist yet: `· not created yet`, rows show defaults, the
  first edit creates it.
- Sections in `ConfigSection` order: General, TUI, Experimental, Vendors,
  Registries, Plugin. `registry.*` rows are dropped; the Registries section is
  drawn from the declared `[[registries]]`. Plugin rows appear after `A`
  (advanced); at global scope `A` shows "plugin settings are project-scope only".
- Rows: `label  value`, a `•` when the key is set in the file; the selected
  row's description wraps into two note lines (`confirm::wrap`).
- Controls by `ValueType`:

  | Type | Control | Saves |
  |---|---|---|
  | Bool | toggle (`space`/`Enter`); vendor tri-state shows `auto` | per key press |
  | Enum | `←→` cycle inline; `Enter` opens a single-select list (radio) | on pick |
  | StringSet (`options.clients`) | multi-select checklist | `Enter`; none = unset (auto-detect) |
  | String / U32 | one-line text field (U32 ignores non-digits) | `Enter`; empty = unset |
  | StringList (`tree_separators`) | small list editor | per add/remove |

- `x` resets a row (unset). `Esc` or `C` closes. `q` does nothing here (text
  fields).
- Geometry: centered, width `min(screen−4, 96)`, height `screen−2`, scrolling.
  Below 44×12 the body is one line, "enlarge the terminal to edit settings".
- Help: `C` joins the existing `h · r · ?` row; hint tiers gain `C config` in
  the two widest tiers only.

### B3. Applying an edit: `apply_config_edit(ctx, edit)` in `src/tui/app.rs`

Same order as the existing TUI config write: `config_guard(ctx)` (flock, a held
lock is an error, not a wait) → `load_scope_declaration` fresh under the lock →
apply one edit (`parse_key` + `apply_set`/`apply_unset`; `plugin.*` through
`read_plugin_table` + `apply_plugin_*` + `write_config_with_plugin`;
`check_draft` + `add_registry`; `check_edit` + `edit_registry`) →
`commit_config`. The dialog sends single edits, never a snapshot, so a
concurrent change to another key survives.

A refused value shows in the dialog's message row in the error colour and the
text field keeps what was typed. A held lock shows "another grim is writing
<file>; try again".

### B4. Refresh after a write

- `src/command/tui.rs` passes a `rescope` closure (built like the existing
  project-init `create` closure, through `scope_swap`) that re-resolves a scope
  from disk.
- `toggle_scope` is refactored into `swap_in(ctx, ScopeSwap) -> ScopeSwap`.
  After a write: swap in a fresh copy of the active scope; if the write was to
  global and the cached other scope is the project, rebuild that too (the
  project folds in global registries and clients); run `after_scope_swap`;
  if the registry list changed, reload the catalog as `r` does.
- The dialog re-reads its rows and keeps the cursor on the same key.
- Keys that are read once at launch show "saved · applies when grim tui starts
  again": `theme` (a `OnceLock`), `default_view`, `sort`, `sort_order`,
  `show_deprecated`, registry `insecure`.

### B5. Registries

- Rows: `● alias  oci  ghcr.io/acme  2 include, 1 exclude` (radio = default);
  alias-less legacy rows muted and read-only; last row `+ add registry`.
- On a row: `Enter` edits; `space` makes it default (clears it on the current
  default); `x` arms an inline confirm (`remove <alias>? enter removes · any
  other key keeps`).
- Form: Alias (read-only on edit; grim has no rename), Kind `‹oci|index›`
  (index forces Insecure off), Locator, Default, Insecure (OCI only),
  Include/Exclude (open the list editor; items kept verbatim, with a note that
  a comma is glob alternation), centered `[ Save ] [ Cancel ]`
  (`confirm::button_label`). Save sends only changed fields.
- When `grim tui` was started with `--registry`, a note says registry edits are
  saved but the browse set stays as the flag set it.

### B6. Docs (after code)

`browse.md` (key row, short "Change settings in the browser" section),
`commands.md` (`grim tui`, one line under `grim config`), `configuration.md`
pointers, the `grim tui` row in `subsystem-cli-commands.md`, drift review of
`catalog/skills/grim-usage`.

### B7. Commits

4. `refactor(config): expose the config edit seam to the TUI`
5. `feat(tui): edit settings from a config dialog`
6. `feat(tui): manage registries from the config dialog`
7. `docs(tui): document the config dialog`

Tests, written first: `config_dialog.rs` — `sections_follow_config_list_order`,
`plugin_rows_hidden_at_global_even_with_advanced`, `space_on_bool_emits_set`,
`x_emits_unset`, `empty_text_enter_emits_unset`, `enum_right_steps_value`,
`enum_enter_opens_single_select_and_pick_emits_set`,
`checklist_enter_saves_joined_esc_discards`,
`add_form_keeps_comma_patterns_verbatim`, `swap_to_index_forces_insecure_off`,
`turning_default_off_writes_default_false`, `x_then_enter_removes_registry`,
`x_then_other_key_keeps_registry`, `legacy_registry_row_is_read_only`.
`app.rs` — `dialog_open_never_creates_global_file`,
`dialog_write_creates_absent_global_file`,
`global_edit_rebuilds_cached_project_alt`, `project_edit_keeps_global_alt`,
`dialog_write_refreshes_clients`, `held_lock_reports_contention_in_dialog`,
`bad_value_keeps_typed_text_and_shows_error`,
`registry_add_reloads_catalog_roots`. `render.rs` (TestBackend) —
`config_dialog_title_names_scope_and_file`,
`config_dialog_marks_absent_file_not_created`,
`config_dialog_asks_to_enlarge_when_too_small`,
`registry_form_buttons_centered`.

---

## Rulings in this plan (mine, change any)

1. **`f` after a multi-kind selection.** The popup can select skill + rule;
   then `f` is pressed. Ruling: `f` steps from the last selected kind to the
   next single kind (skill + rule → agent). Alternative: `f` clears to all
   kinds first.
2. **Title with many kinds.** Up to three are named (`kind: skill, agent`),
   more become `kind: 5 kinds`, so the border label fits at 80 columns.
   Alternative: always list all, truncated.
3. **Enum rows open a pick list.** `theme` has 14 values; cycling is blind.
   Ruling: `←→` cycles, `Enter` opens a radio list. Alternative: cycle only
   (about 40 lines less).
4. **Registry remove confirms inline** in the row instead of the big confirm
   popup, which would close the dialog and need its own mode. The extension
   removes with no confirm at all.
5. **Six keys apply at next start**, not live (list in B4). Making `theme`
   live means replacing a `OnceLock`; left out.
6. **Text fields are append and Backspace only**, like the search box: no
   caret movement, no paste. Marked with a `shortcut:` comment.
7. **No login/logout** in the registry view; the extension has none either.

## Verify first during execution (not checked while planning)

- A first write to an absent global file writes only the set key, not every
  default.
- `scope_resolution::resolve(ctx, true, …)` re-reads the global file on each
  call (the `rescope` closure depends on it).
- The edited help rows still fit 80×24 (the existing fit test catches it).

## Known limits

- Adding a registry reloads the catalog synchronously, like `r` and startup:
  the UI freezes while it fetches.
- Acceptance tests cannot drive the TUI; coverage is in-module unit and
  `TestBackend` tests only.
- This host has no Windows; the change is platform-neutral Rust, CI is the gate.

## Verification

- Per step: `cargo test -p grimoire tui::` and `cargo test -p grimoire config`
  for the touched module, then `task rust:verify`.
- Per phase, before the last commit: `env -u OCX_GLOBAL task verify`,
  `task claude:tests` (rule catalog row changed), `task catalog:verify`,
  `task docs:check` (finish its last step with `/usr/bin/python3` on this host).
- Manual, in a scratch project against a local registry:
  - `grim tui`, press `F`: toggle two kinds and watch the list and the title
    change behind the popup; pick a sort; `Esc`; confirm `s`, `S`, `f`, `h`
    still act on the same state.
  - Press `C`: toggle a bool, pick `options.clients` values, set and reset a
    text key; add, edit, default and remove a registry. After each, check
    `grim config list --all` and `grim config registry list` in a shell show
    the same values and that comments in `grimoire.toml` survived.
  - `g` to the other scope, `C` again: title names the other file.
  - Resize to 80×24 and below 44×12.
