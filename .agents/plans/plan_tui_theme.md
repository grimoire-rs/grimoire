# Plan: TUI theme module

## Status

- **Plan:** plan_tui_theme
- **Active phase:** 1 — foundation (theme module, lint)
- **Step:** awaiting /finalize
- **Last update:** 2026-11-12 (implemented; awaiting verify and commit)

---

## Context

Users on light terminals cannot read the TUI. The cause: `src/tui/render.rs` and
`src/tui/install_progress.rs` hard-code about 90 style literals, among them
`Color::White` body text (13 sites), an `Indexed(236)` selection background
(3 sites + the gauge track) and a full-screen `Color::Black` backdrop. Only the
status column goes through a small theme (`ColorKey` / `color_for`,
`render.rs:36`, `:2140`).

Outcome: every TUI colour comes from `src/tui/theme.rs`; the default theme
follows the terminal's own palette; the user can pick another built-in theme;
a lint plus one table test stop a raw colour from coming back.

## Decisions

1. **Default theme `terminal` uses no detection.** Research verdict: letting the
   terminal remap `Color::Reset` and the ANSI colours follows *any* palette,
   which is strictly better than detecting light/dark (OSC 11,
   `terminal-colorsaurus`) and choosing between two fixed palettes. Detection
   also costs a dependency, up to 1 s startup wait over SSH and stray-reply
   risk under tmux. No new dependency.
2. **`terminal` theme rules** (the allowed set):
   - Body text and backgrounds: `Color::Reset` (or no colour at all).
   - Accents: only Red, Green, Yellow, Blue, Magenta, Cyan (and their Light
     variants). Never `White`, `Gray`, `DarkGray`, `Black`, `Indexed`, `Rgb`.
   - Muted text: `Modifier::DIM` on the default colour. It fails safe (plain
     text where a terminal ignores DIM); `DarkGray` fails invisible on
     Solarized Dark.
   - Selection (active tab, list row, picker row, chosen confirm button):
     `REVERSED | BOLD`. Survives `NO_COLOR`.
   - Headings that were `White + BOLD/UNDERLINED`: modifiers only.
   - Install-progress backdrop: removed. Gauge: Cyan fill, default track.
   - ITALIC stays only as a bonus cue on already-muted text.
3. **Built-in themes:** `terminal` (default), `dark` (today's exact look, kept
   for users who like it), `light` (fixed xterm-256 palette tuned for light
   backgrounds: dark accents, light selection background, no Yellow/Cyan text).
4. **Selection surface** (additive, copies the existing `sort` / `--sort`
   pattern): config key `[options.tui] theme = "terminal|dark|light"` and flag
   `grim tui --theme <NAME>`. Precedence: flag > config > default. No env var.
5. **Threading:** process-global `OnceLock<&'static Theme>` in `theme.rs`,
   `theme::init(TuiTheme)` called once from `src/command/tui.rs`,
   `theme::current()` returning `terminal` when unset. Mirrors
   `src/cli/color.rs:145`. No `draw()` signature change, so ~12 `app.rs` call
   sites and ~17 test call sites stay untouched and `RenderModel` stays
   ratatui-free.
6. **Guards against recurrence** (the render-to-`TestBackend` backstop test is
   skipped, as requested):
   - Root `clippy.toml` `disallowed-types` for `ratatui::style::Color` (and
     `ratatui::style::Stylize`), `#![allow(clippy::disallowed_types)]` only in
     `theme.rs`. `task verify` runs clippy with `-D warnings --all-targets`.
   - One pure table test in `theme.rs`: every role of `terminal` uses only the
     allowed set from decision 2. This guards the theme file itself.

## Theme API (fixed before the parallel workers start)

`src/tui/theme.rs`:

```rust
pub struct Theme { pub text: Style, pub muted: Style, /* … one Style per role */ }
pub static TERMINAL: Theme; pub static DARK: Theme; pub static LIGHT: Theme;
pub fn init(name: TuiTheme);            // OnceLock::set, first call wins
pub fn current() -> &'static Theme;     // TERMINAL when unset
impl Theme {
    pub fn status(&self, key: ColorKey) -> Style;   // replaces color_for
    pub fn heading(&self, level: u8) -> Style;
    pub fn scope(&self, …) -> Style;                // project / global / none
}
```

Roles (from the site inventory; `dark` holds today's value for each):
`text`, `muted`, `muted_dim`, `hint` (muted + italic), `key_hint`, `accent`,
`title`, `border`, `border_popup`, `border_confirm`, `selection`,
`button`, `button_selected`, `button_dimmed`, `tab_inactive`,
`status_message`, `deprecated`, `column_registry`, `column_kind`,
`column_version`, `column_rating`, `column_downloads`, `detail_label`,
`detail_section`, `bullet`, `code`, `pinned`, `rule`, `gauge`, plus the three
methods above. `render.rs` keeps `Modifier` for one-off additions
(`accent` + UNDERLINED at `:1384`); it never names `Color`.

`ColorKey` stays in `render.rs` (ratatui-free, used by `frame()`); `color_for`
is deleted.

## Execution

Branch `duo`, same checkout, all workers `model: sonnet` with a
`Model rationale:` line. Workers in step 2 edit only — no `cargo` runs, no
commits — because they share one tree and one target dir; the orchestrator
builds once after they return.

**Step 0 — orchestrator.** Check hex state (`.agents/memory/hex.md` pointers).
Copy this plan to `.agents/plans/plan_tui_theme.md` from the plan template.

**Step 1 — foundation, one worker, serial.**
- Create `src/tui/theme.rs` (API above, three themes, table test); add
  `mod theme;`.
- Add the `TuiTheme` enum to `src/config/declaration.rs` next to `DefaultView`
  (`:78-111`) — `theme.rs` needs it; the rest of the config surface is step 2C.
- Add `clippy.toml`. First prove in the scratchpad what `disallowed-types`
  catches (`use` import, `ratatui::prelude::*`, full-path `Color::Cyan`,
  `Stylize` helpers). If it misses the import path used in `src/tui/`, replace
  it with a `#[test]` in `theme.rs` that scans `src/tui/*.rs` (except
  `theme.rs`) for `Color::` / `Stylize` and report that swap.
- `cargo check` green before handing off. (The lint goes red until step 2
  finishes; expected.)

**Step 2 — three workers in parallel, disjoint line ranges.**
- **A — `render.rs` 732-790 and 1224-1618:** `tab_strip_line`, `tab_hint_line`,
  `draw()` (title row, scope, search, catalog list, detail pane, footer).
- **B — `render.rs` 1731-1930 and 2010-2154, plus `install_progress.rs`:**
  `draw_confirm`, `draw_picker`, `legend_line`, `help_lines`, `draw_help`,
  delete `color_for`, drop the black backdrop and `Indexed(236)` gauge track.
- **C — config surface + tests:**
  - `render.rs` 2155-end: the 6 colour asserts (`:2766, 3345, 3350, 3578,
    3641, 3696`) and 2 modifier asserts (`:2369, 3460`) compare against
    `theme::current()` roles instead of literals.
  - `TuiOptions.theme` in `declaration.rs`; `src/config/resolved.rs`
    (destructure ~`:105`, `ResolvedOptions`); `src/config/defaults.rs`.
  - `src/command/config_keys.rs`: append `ConfigKey::TuiTheme` (the `ALL`
    array is append-only), `KeySpec`, `spec()` arm, schema-match arm, the two
    `ALL.len()` asserts. Description must prefix the declaration doc comment
    (`subsystem-config-keys.md`).
  - `src/command/config.rs`: get / set / unset arms + `parse_theme` (copy
    `parse_sort_order`, `:1245`) + its unit test.
  - `src/command/tui.rs`: `--theme` on `TuiArgs`, resolve like `sort` (~`:139`),
    call `theme::init`.
  - Acceptance: extend `test/tests/test_config_tui_options.py` (get/set/unset
    round trip, invalid name rejected); check `FIXED_OPTION_KEYS` in
    `test/tests/test_config.py`.

**Step 3 — orchestrator integrates.** `cargo check`, `cargo clippy`,
`cargo nextest run` for `tui` and `config`; one fix worker for whatever the
merge of A/B/C leaves red.

**Step 4 — one docs worker, after the code is final.**
`docs/src/content/docs/configuration.md` (`[options.tui]` table),
`commands.md` (config-key table, `grim tui` flags), `browse.md` (themes, the
light-terminal note); catalog drift in
`catalog/skills/grim-usage/references/registries.md` and any `grim tui` flag
list in `grim-usage/SKILL.md`.

**Step 5 — gate and commit.** `env -u OCX_GLOBAL task verify`,
`task catalog:verify`, `task docs:check`. Commits, signed off, no push:
`fix(tui): follow the terminal theme by default` (theme module + render sites)
and `feat(tui): selectable built-in themes` (config key, flag, docs).

## Verification

- `cargo nextest run config_key_metadata_matches_published_schema` and the
  `theme.rs` table test pass.
- Lint proof: temporarily add `use ratatui::style::Color;` to `render.rs`,
  confirm `task rust:clippy:check` fails, revert.
- Manual, from `target/debug/grim` with `GRIM_HOME` in the scratchpad:
  `grim tui`, `grim tui --theme dark`, `grim tui --theme light`,
  `grim config set options.tui.theme light` then `grim tui`, and
  `NO_COLOR=1 grim tui` (selection still visible as reverse video).
- The owner checks `terminal` on a real light-profile terminal — no agent can
  see rendered colours; this is the one check only a human can do.

## Not in scope

Render-to-`TestBackend` backstop test (dropped on request), light/dark
auto-detection, user-defined theme files, an in-TUI key to switch themes,
cleaning up overloaded accent colours (Magenta/Green/Yellow each mean several
things today), Windows run (TUI styling is platform-neutral; CI stays the gate).

## Follow-ups landed

- Selection is one solid accent-colored bar in `terminal`.
- Fixed themes paint a canvas (`theme::paint_canvas`) and are built from 13-slot palettes (`from_palette`).
- 11 named themes added: catppuccin-latte, -frappe, -macchiato, -mocha, gruvbox-dark, gruvbox-light, solarized-dark, solarized-light, tokyo-night, nord, dracula.
