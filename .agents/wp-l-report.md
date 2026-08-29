# WP-L — `clients.md` Hook column + parity test

- **Worktree:** `/mnt/wsl/share/dev/grimoire/grimoire/.agents/worktrees/wp4-l`
- **Branch:** `hex/hooks-artifact-kind--wp4-l` (based on `3772b76`)
- **Commit:** `221e27e` — `docs(clients): add the Hook column to the compatibility matrix`
- **Files:** `docs/src/clients.md`, `src/install/client_target.rs` (tests + the
  matrix-parsing helper only). Nothing else touched.

## `parse_first_matrix` call sites changed

Grepped, not assumed — `grep -rn "parse_first_matrix" --include='*.rs' .` returns
exactly two hits, both in `src/install/client_target.rs`:

1. **`src/install/client_target.rs:1009`** — the definition. Return type
   `Vec<(String, [Option<Cell>; 4])>` → `[Option<Cell>; 5]`, and the per-cell
   closure now takes an **index** and reads through `cells.get(idx)` instead of
   indexing `cells[..]` directly.
2. **`src/install/client_target.rs:1074`** — the single caller,
   `docs_matrix_row_set_matches_all_and_cells_track_kind_support`. Gains a
   `probe_root()` token and the Hook assert.

The `Vec::get` change is load-bearing, not cosmetic. The row guard is still
`cells.len() < 5`, so an old-shape five-cell row (client + four kinds) still
lands in the returned set with `Hook == None` and fails the **placeholder**
assert, which names the missing column. Direct indexing would have panicked
with an index message, and tightening the guard to `< 6` would have dropped the
row and failed the row-set equality assert instead — a failure message about
the row set says nothing about the column that went missing.

## The computed column, client by client

Computed from code (temporary `#[test]` printing `hook_matrix_cell(client, &root)`
for every `ClientTarget::ALL`, run with `--nocapture`, then removed):

| Client | Hook | | Client | Hook |
|---|---|---|---|---|
| claude | `✓` Native | | antigravity | `✗` Declined |
| opencode | `✗` Declined | | cline | `✗` Declined |
| copilot | `◐` Degraded | | droid | `✗` Declined |
| codex | `✓` Native | | goose | `✗` Declined |
| cursor | `✗` Declined | | warp | `✗` Declined |
| kiro | `✗` Declined | | openclaw | `✗` Declined |
| junie | `✗` Declined | | kilo | `✗` Declined |
| gemini | `✗` Declined | | agents | `✗` Declined |
| zed | `✗` Declined | | amp | `✗` Declined |

**This agrees with the plan's C-013 reading exactly** — claude `✓`, codex `✓`,
copilot `◐`, the other fifteen `✗`. No discrepancy to report; the doc column was
written from these values, not from the plan text.

### Copilot's `◐` verified matcher-independent

A second temporary test dumped, for each of the three v1 clients, every
declarable `(event, tier)` pair with both `HOOK_CELL_PROBE_MATCHERS` entries and
the decline reason. Observed:

- **copilot `PostToolUse` × `Gatekeeper`**: `read=ERR all=ERR`, both
  `TierUnsupported`, `hook_tier_support == Declined`. One pair, both matcher
  forms → matcher-independent projection-table gap. This is the only pair that
  degrades copilot's cell.
- **claude / codex / copilot `PreToolUse` × `Mutator`**: `read=ok all=ERR`, the
  `all` arm `MutatorOnShellCommandTool` (Decision K). Because the quantifier is
  **any-arms**, the pair still counts as armed, so Decision K contributes
  **nothing** to any cell. Confirmed empirically, not inferred from the doc
  comment.
- Every other declarable pair on all three clients arms for both matcher forms.

The Known-gaps prose therefore attributes copilot's `◐` to its own event table
and explicitly says it is not the per-matcher refusal.

## Failing-test-first — the output actually observed

After editing the test and **before** touching `docs/src/clients.md`:

```
running 1 test
test install::client_target::tests::docs_matrix_row_set_matches_all_and_cells_track_kind_support ... FAILED

thread '...' panicked at src/install/client_target.rs:1111:51:
unparsed Hook cell for 'claude' (TODO placeholder?)

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2799 filtered out
```

## Placeholder-panic observation (the second half of Specify)

Not claimed — performed. With the doc column populated and the test green, the
codex Hook cell was replaced by the literal `TODO` and the test re-run:

```
thread '...' panicked at src/install/client_target.rs:1111:51:
unparsed Hook cell for 'codex' (TODO placeholder?)
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2799 filtered out
```

The cell was then restored to `✓` and the test re-run green. Note the panic
named `codex` — the row it was actually broken on — which is the property the
`Vec::get` choice buys.

## Doc changes

- **Matrix**: header, separator and all 18 rows gained a `Hook` cell, appended
  last so the order mirrors `ArtifactKind` (`Skill, Rule, Agent, …, Mcp, Hook`).
  Applied programmatically so no row could drift. `docs/theme/clients-matrix.css`
  needed no change — it carries no column-count or `nth-child` rule.
- **Intro**: one paragraph on why hooks are an *arming* question rather than a
  file-writing one.
- **Legend**: kept as-is, plus a note on the two things the Hook column
  deliberately does not encode — it is scope-blind, and it is capability, not
  consent.
- **`{#gap-hooks}`** — the three reasons a row reads `✗`, stated as three
  separate cases: *no upstream surface* (zed, warp — not reversible by a grim
  release), *not a client* (the `agents` target), *surface exists, grim
  scheduled* (the other twelve, grouped by install shape: splice-a-config /
  own-a-file / JS-plugin-codegen, so a reader can see which are close). Plus why
  the three that arm were chosen, and the warning that `✓` is not a promise
  every hook installs.
- **`{#gap-hooks-scope}`** — codex and copilot are global-scope only, with the
  tracked-repository-file reason; claude unaffected via
  `.claude/settings.local.json`.
- **`{#gap-copilot-hooks}`** — the `◐`, attributed to copilot's event table and
  explicitly **not** to Decision K.

Every fact came from code or from `.agents/research/research_hooks_vendor_survey.md`
(the master matrix: warp `none`, zed `none`, and a documented mechanism for the
other twelve). No client's hook facts were written from memory.

## Gates

| Gate | Result |
|---|---|
| `cargo fmt` | clean (run before commit) |
| `cargo clippy --locked --all-targets -- -D warnings` | `Finished dev profile`, zero warnings |
| `cargo test --bin grim` | `2800 passed; 0 failed` |
| `task --force verify` | passed — `1019 passed in 18.68s` on the pytest leg, gate marked |

`task verify` rewrote `.claude/tests/uv.lock` and `test/uv.lock`; both were
reverted with `git checkout --` before staging. The commit holds exactly the two
declared files (`git status --short` verified before and after).

## Defects found in the plan or merged code

**None.** Three things were checked specifically because they were plausible
defects, and all three held:

1. The plan's C-013 column (`claude ✓, codex ✓, copilot ◐, fifteen ✗`) matches
   the computed values exactly.
2. `hook_matrix_cell`'s doc-comment claim that copilot's `◐` is driven by
   `verdict: &[]` at `PostToolUse` — matcher-independently — is what the probes
   show, including the negative half (Decision K moves no cell).
3. `RootToken::for_test` was **not** needed. `probe_root()`
   (`src/install/client_target.rs:1970`) already derives a real token through the
   HMAC path in a temp `$GRIM_HOME`, and it sits in the same `mod tests` as the
   parity test, so no visibility was widened and the private field stayed closed.

## One note for WP-M / a later pass (not a defect, out of my file set)

`docs/src/clients.md` now uses the words *gatekeeper*-shaped ("a hook whose
response is meant to allow or deny") and *tier* without linking a definition,
because the page that will define them (`artifacts.md` / `commands.md` hook
sections) is WP-M's. Once those land, the two hook sections here are the natural
place to add the internal links — `docs-style.md` requires linking only sections
that already exist, which is why they are unlinked today.
