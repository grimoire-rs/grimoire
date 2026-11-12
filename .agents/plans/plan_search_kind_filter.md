# Plan: Explicit kind filter for search, TUI and MCP

## Status

- **Plan:** plan_search_kind_filter
- **Active phase:** 5 — Docs and follow-up (packages D, E)
- **Step:** awaiting /hex-review
- **Last update:** 2026-11-12 (packages A–D integrated; task verify green)

---

## Context

Issue [#321](https://github.com/grimoire-rs/grimoire/issues/321) (reported on 0.14.1) says there is no discoverable way to narrow `grim tui` or `grim search` to one artifact kind.

Research on current `main` (`duo` == `origin/main`) shows the problem is narrower than the issue states:

- **Help text.** The `grim search --help` complaint is already fixed. The `query` doc at `src/command/search.rs:75-77` lists all 7 kinds, and `search_help_names_the_cutoff_setting_and_every_kind_keyword` pins it.
- **Bare keywords.** A bare keyword already filters in the CLI, in MCP and in the TUI `/` search, all through `SearchQuery::parse` at `src/catalog/search_match.rs:179`. The issue is right that this is invisible. It also has a known cost: a keyword can never be a text term, so a skill named `vscode-settings` cannot be found by `settings`. `docs/src/content/docs/upgrading.md:575-581` documents that.
- **Remaining gaps.** There is no explicit `--kind` flag, the TUI has no visible kind control, and MCP agents only learn the trick from prose.

Owner decisions (2026-11-12):

1. **Explicit wins.** When an explicit kind filter is set (`--kind`, the TUI `f` filter or MCP `kind`), every query word is plain text. Without one, the bare keyword keeps working as shorthand.
2. **TUI control.** An `f` cycle key, with a `kind:` label in the Catalog title. A "Filter & sort" modal becomes a follow-up issue, landing after [#326](https://github.com/grimoire-rs/grimoire/issues/326).
3. **MCP.** `grim_search` gets an optional `kind` argument as well.

Every change is additive. A new flag, a new optional MCP argument and a new TUI key change no JSON shape and no exit code. The new MCP argument extends a frozen surface additively (`stability.md:44`).

## Approach

### 1. Matcher: one constructor carries explicit kinds

File: `src/catalog/search_match.rs`.

- Add `SearchQuery::parse_with_kinds(raw: &str, kinds: &[ArtifactKind]) -> Self`.
  - When `kinds` is empty, return `Self::parse(raw)`.
  - Otherwise, every whitespace token, lowercased, becomes a text term, and `kinds` becomes the kind gate. Kinds are OR'd among themselves and AND'd with the terms, the same as today's gate.
- `kind_keyword`, scoring, weights and `retain_relevant` stay unchanged. The cutoff still runs inside the kind-filtered set, because the gate lives in `score_fields`.
- Update the module doc (`:7-17`) to state the explicit-wins rule in one sentence.

### 2. Shared seam (dropped during execution)

The `load_catalog` signature stays `&str`. `grim search` already browses through `load_catalogs` + `browse(&SearchQuery)`, and every other `load_catalog` caller passes `""`, so changing the seam would only add churn. `zero_match_warning` needs no change: it already stays silent for any non-empty query, so `--kind` behaves like a keyword there.

### 3. CLI `--kind` on `grim search`

File: `src/command/search.rs`.

- Add `#[arg(long, short = 'k', value_delimiter = ',', value_parser = <7 kind strings>)] kind: Vec<String>`. The `value_parser` copies the list pattern of `add.rs:90`. Values are exact singular kind names, as on `add`, `build` and `release`.
- Doc: "Show only these kinds (repeatable or comma-separated). With `--kind`, every query word matches as text."
- Convert the values with `ArtifactKind::from_kind_str`. In `run`, build the query once with `SearchQuery::parse_with_kinds`. That replaces the re-parse at `:157`, and the same value goes to `load_catalog`.
- Edit the `query` doc (`:75-77`) so the keyword sentence ends with "unless `--kind` is given".
- Extend the help test at `:893` to name `--kind`.
- An unknown value exits 64 through clap. No new exit code.

### 4. MCP `grim_search` argument

Files: `src/mcp/tool_args.rs`, `src/mcp/server.rs:64`.

- Add `#[serde(default)] pub kind: Option<Vec<ArtifactKind>>` to `SearchToolArgs`. `ArtifactKind` already derives `Deserialize` and `JsonSchema` with lowercase names, so the schema enumerates the 7 values. An unknown value becomes an invalid-params error.
- Add one sentence to the `query` doc saying that `kind` makes every query word text.
- `server.rs` maps the array into `SearchArgs.kind`, either as kind strings or by giving `SearchArgs` a typed field. Pick whichever keeps a single conversion.
- Do not touch the SSRF comment or the registry lock.

### 5. TUI `f` key

Files: `src/tui/state.rs`, `src/tui/event.rs`, `src/tui/render.rs`.

- **State.**
  - Add `kind_filter: Option<ArtifactKind>`.
  - Add a private `fn search_query(&self) -> SearchQuery`, which is `parse_with_kinds(&self.query, kind_filter.as_slice())`. Use it at both parse sites, `apply_query` (`:1218`, `:1220`) and `recompute_filter` (`:2381`). The tree-flatten logic then sees typed keywords as text whenever the filter is on.
  - Add `pub fn cycle_kind_filter(&mut self)`. It steps `None` → the next kind in `ArtifactKind::ALL` order that has at least one row in `self.rows` → … → `None`, then recomputes and moves the cursor to the top. That mirrors `cycle_sort`/`resort` (`:1292-1320`). Rows compare by `row.kind == kind.kind_str()`, and `"-"` rows never match.
  - Add `pub fn kind_label(&self) -> String`, returning `kind: agent` or an empty string.
  - Fix the stale kind list in the comment at `:2365`.
- **Event.** In `handle_browse` (`event.rs:550-914`), `Char('f')` calls `state.cycle_kind_filter()`. Search mode is untouched, because printables are query text.
- **Render.** The Catalog border title shows `kind_label` and `sort_label` joined with ` · ` (`render.rs:1193`, `:1440`). Fold `f` into an existing `help_entries` row, for example `"g · t · s · f"`, ending in `… sort (S flips) · kind`. The array stays at 10 rows, and `help_overlay_fits_all_entries_on_standard_terminal` and `help_body_line_count_matches_state` must stay green.
- Skipped: no `grim tui --kind` seed flag, no config key, no reverse-cycle key.

### 6. Docs, catalog skill, rule index

- **`docs/src/content/docs/commands.md`.**
  - `#search` (`:2117`): add a `--kind` paragraph with an example. `grim search --kind skill settings` finds `vscode-settings`.
  - Update the MCP tools table row (`:4278`) to list `kind?`.
  - Add the `f` key wherever the `#tui` section names `s`.
- **`docs/src/content/docs/browse.md`.** Add an `f` row to the key map at `:103`, and add one step, "press `f` to show one kind", beside the `/` step.
- **`docs/src/content/docs/upgrading.md:575-581`.** Add one sentence: `grim search --kind skill settings` is the other way back.
- **`catalog/skills/grim-usage/references/registries.md:759-763`.** Drift review: mention `--kind`.
- **`.claude/rules/subsystem-cli-commands.md`.** Update the `grim search` and `grim tui` rows (`--kind`, the explicit-wins rule, the `f` key and the title label).

### 7. Follow-up issue (approved)

After the code lands, open an issue: "TUI: Filter & sort modal". It covers multi-select kinds, sort, direction and deprecated in one popup, with live apply and the `s`/`S`/`h`/`f` shortcuts kept. Mark it as blocked by [#326](https://github.com/grimoire-rs/grimoire/issues/326), the shared popup geometry work.

## Tests (write first)

- **`search_match.rs` unit tests.**
  - `parse_with_kinds` with an empty slice equals `parse`.
  - With `[Skill]`, the token `settings` is a text term that matches a skill whose leaf is `vscode-settings`.
  - `[Agent, Skill]` is OR'd.
  - The gate stays exact.
- **`search.rs`.**
  - clap accepts `--kind agent,skill` and `-k agent -k skill`.
  - `--kind bogus` is a usage error.
  - The help test names `--kind`.
- **TUI.**
  - State tests: the cycle skips kinds absent from the rows and wraps to `None`, and with the filter on, `apply_query("skill")` matches as text.
  - The kind-only filter keeps tree view.
  - An event test: `f` cycles.
  - A `TestBackend` render test: the Catalog title shows `kind: agent · sort: …`.
- **MCP.** `SearchToolArgs` deserializes `{"kind":["agent"]}` and rejects `{"kind":["bogus"]}`, and the generated schema lists the enum.
- **Acceptance, `test/tests/test_search.py`.**
  - `--kind agent` returns agents only.
  - `--kind skill settings` finds a `*-settings` skill that bare `settings` does not.
  - The comma and repeat forms are equal.
  - A bogus kind exits 64.
  - The JSON envelope keys are unchanged.
  - Add a `grim_search` `kind` case in `test/tests/test_mcp.py`.

## Workflow: delegate to sonnet with strict briefs

Integration happens in this worktree on branch `feat/search-kind-filter`, created from `duo` (which is the same commit as `origin/main`). I (Opus) only orchestrate: I write the briefs, apply the patches, review, run the one full gate and commit. I do not write code myself.

Every code package goes to `model: sonnet`, and each brief carries a `Model rationale:` line. A worker hands back a patch. It must not commit, because the commit hook needs a fresh verify stamp.

### Rules for every brief

Each brief repeats these rules word for word:

- **ALLOWED FILES.** You may edit only the files the brief lists. Editing any other file fails the package.
- **Signatures.** Use the exact names and signatures the brief gives. Do not rename, re-export or add a public item the brief does not name.
- **Tests first.** Write the listed tests first and run them. They must fail for the right reason. Then implement.
- **Out of scope.** Change no JSON report field, exit code, existing test assertion, comment you were not asked to change, or dependency in `Cargo.toml`/`Cargo.lock`. Do not refactor. Do not run `task verify`, `git commit` or `git push`.
- **Commands.** Run only the commands the brief lists: `cargo check --all-targets`, the named `cargo test` filters, and `cargo fmt`. Use `CARGO_TARGET_DIR=<shared>` where the brief says so.
- **Return.** Report `git diff --stat`, each test name with pass or fail, the last 20 lines of every failing command, and any deviation, with its reason. Do not narrate.
- **When stuck.** If the brief cannot be followed as written, stop and report why. Do not improvise.

### Packages

Packages run in this order: A, then B and C in parallel, then D, then E.

- **A (sonnet, this worktree, sequential).** Covers Approach steps 1 and 2: `parse_with_kinds`, its unit tests, and the `load_catalog(&SearchQuery)` signature change at every caller.
  - Allowed files: `src/catalog/search_match.rs`, `src/catalog/catalog_service.rs`, `src/command/status.rs`, `src/command/rate.rs`, `src/command/search.rs` (only the `load_catalog` call and the `:157` re-parse), `src/tui/app.rs` (only the call at `:1341`).
  - Gate: `cargo check --all-targets` and `cargo test search_match catalog_service`.
  - I review the diff, then make a WIP commit so B and C branch from it.
- **B (sonnet, worktree `.agents/worktrees/kind-cli`, shared `CARGO_TARGET_DIR`).** Covers Approach steps 3 and 4: the CLI `--kind` flag, the MCP `kind` argument, their Rust tests, and the `test_search.py`/`test_mcp.py` acceptance cases.
  - Allowed files: `src/command/search.rs`, `src/mcp/tool_args.rs`, `src/mcp/server.rs`, `test/tests/test_search.py`, `test/tests/test_mcp.py`, plus fixture files those tests need under `test/`.
  - Gate: `cargo check --all-targets`, `cargo test command::search mcp::`, and the two pytest files only.
- **C (sonnet, worktree `.agents/worktrees/kind-tui`, shared `CARGO_TARGET_DIR`).** Covers Approach step 5: the TUI `f` key.
  - Allowed files: `src/tui/state.rs`, `src/tui/event.rs`, `src/tui/render.rs`.
  - The brief quotes the cycle rule, the label format `kind: <kind>` joined to the sort label by ` · `, and the 10-row help limit.
  - Gate: `cargo check --all-targets` and `cargo test tui::`.
- **Integration (me).** Apply the B and C patches, remove both worktrees and their branches, and run one review pass (sonnet reviewer, diff only). Fix only Block findings, with at most one fix round, sent back to the package that owns the file. Then run `env -u OCX_GLOBAL task verify` once.
- **D (sonnet docs pass, after the code is final).** Covers Approach step 6.
  - Allowed files: `docs/src/content/docs/commands.md`, `docs/src/content/docs/browse.md`, `docs/src/content/docs/upgrading.md`, `catalog/skills/grim-usage/references/registries.md`, `.claude/rules/subsystem-cli-commands.md`.
  - The worker reads the merged code first and follows `docs-style.md` and `docs-quality.md`.
  - Gate: `task catalog:verify`, `task claude:tests`, and `task docs:check` when Node 24 is available.
- **E (me).** Write the final commits, run the gate again, and open the approved follow-up issue (Approach step 7).

Commits are `feat(search): kind filter flag and MCP argument`, `feat(tui): f cycles a kind filter` and `docs: kind filter`, each signed off with `-s`, and the plan is saved as `.agents/plans/plan_search_kind_filter.md`. Never push.

## Verification

1. Workers run only their targeted gates. The integration worktree runs `env -u OCX_GLOBAL task verify` once per merged batch.
2. Run `task catalog:verify` for the grim-usage drift.
3. Run `task docs:check` if Node 24 is available. Otherwise, report that it was skipped.
4. Manual checks:
   - `cargo run -- search --kind agent`
   - `cargo run -- search -k skill settings`
   - `cargo run -- search --kind bogus; echo $?` should print 64.
   - `cargo run -- tui`: press `f` repeatedly, check the title label and that the cycle skips absent kinds, then type `/skill` with the filter on.
5. MCP: run `grim mcp` and check that `tools/list` shows the `kind` enum on `grim_search`.
