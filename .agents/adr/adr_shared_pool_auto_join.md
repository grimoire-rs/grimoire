# ADR: A pool reader joins the shared pool beside a pool writer

## Metadata

**Status:** Accepted
**Date:** 2026-11-05
**Deciders:** maintainer (owner decisions 2026-11-05)
**Beads Issue:** N/A (GitHub tracking: [grimoire-rs/grimoire#262](https://github.com/grimoire-rs/grimoire/issues/262))
**Related PRD:** N/A
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md` (no new tech; a layout rule over the shipped reapers)
**Domain Tags:** integration | api
**Supersedes:** N/A. Amends [`adr_vendor_config_and_selection.md`](./adr_vendor_config_and_selection.md) D1, D2 and A7
**Superseded By:** N/A

## Context

A skill installs twice when one selected client writes the shared pool and
another client reads the pool but renders natively. Issue
[#262](https://github.com/grimoire-rs/grimoire/issues/262) reports it for a
global OpenCode install beside Codex: grim writes
`~/.agents/skills/<name>` for Codex and `~/.config/opencode/skills/<name>`
for OpenCode. OpenCode v1.18.32 scans both, logs `duplicate skill name`, and
the last copy wins.

The same pair forms for every **pool reader** that renders natively by
default: Cursor, Copilot, OpenCode, Warp, Droid, Kilo and Cline. The **pool
writers** are Codex, Gemini, Zed, Amp, Goose, the generic `agents` client,
Antigravity at project scope, and any client set to `shared_skills = true`.

[`adr_vendor_config_and_selection.md`](./adr_vendor_config_and_selection.md)
D1 kept native rendering and called the duplicate harmless, because scanning
is additive. #262 shows the cost: two copies that can drift, a client that
reports a conflict, and a winner the user did not pick. D2's opt-in was the
only way out, and nothing told users they needed it.

## Decision Drivers

- **One copy on disk** whenever a pool writer is present, with no config.
- **Existing installs migrate** without user action. Principle 9 allows a
  layout move only with automatic migration, an old-path reaper and an
  upgrade fixture.
- **No data loss.** A hand-edited skill is never deleted, as A7 already
  decided.
- **Stable under drift.** Autodetection can change the selected set from run
  to run. Disk must never hold two copies at any step.
- **An explicit way to opt out.**

## Considered Options

### Option 1: Auto-join, with a tri-state opt-out (chosen)

A pool reader puts its skills in the pool when another client in the
effective set lands there unconditionally. `shared_skills` becomes
tri-state: unset means auto, `true` means always pool, `false` means always
native.

| Pros | Cons |
|------|------|
| Fixes every reader/writer pair, not only OpenCode | `config set … shared_skills false` changes meaning: it now persists |
| Reuses the shipped migration reapers | A reader's layout depends on its peers |
| `false` keeps today's layout for anyone who wants it | |

### Option 2: Make the pool the default for every reader

| Pros | Cons |
|------|------|
| Simplest rule | Moves every OpenCode-only or Cursor-only install for no gain |
| | Contradicts the D1 owner principle: prefer a vendor directory where one exists |

### Option 3: Keep D1 and document the duplicate

| Pros | Cons |
|------|------|
| No change | The reported bug stays. The client keeps logging a conflict on every start |

## Decision Outcome

**Chosen option: Option 1.**

### D1: The join rule {#d1}

For a **skill** and a client C:

- `shared_skills = false`: native. `true`: pool, as before.
- Unset: pool when **all** of these hold:
  1. C is `pool_capable()` and does not already land in the pool.
  2. C reads the pool in this environment (`Vendor::reads_pool(scope)`).
  3. The pool path anchors for C.
  4. Another client in the effective set **lands in the pool
     unconditionally**.

Every other kind keeps `InstallTarget::path_for`.

**"Lands unconditionally"** means `path_for(P, Skill) == pool_path`: a
default pool writer or a client set to `true`. The test never looks at an
auto decision, so readers cannot pull each other in. OpenCode with Cursor
alone stays native, and the rule has no cycles. Implementation:
`InstallTarget::joins_pool` (`src/install/target.rs`).

### D2: The effective set is selected plus recorded {#d2}

The peers are this run's target clients plus every client the artifact's
install record still holds whose output still resolves
(`expected_outputs::effective_clients`). A recorded client stays a peer
because the installer keeps re-materializing it (ADR
`adr_vendor_config_and_selection.md` D5). Using the selection alone would
let autodetect drift flip a reader back to native while the writer's copy
still sits in the pool, which is two copies again.

`expected_outputs::destination` is the one seam. Install, the integrity
gate, `status` `outputs_pending` and the TUI badge all ask it, so they
cannot disagree about where a skill belongs.

### D3: `reads_pool` gates {#d3}

A reader whose pool scan is switched off stays native, because a joined
skill would go unread:

| Client | Gate |
|---|---|
| OpenCode | `OPENCODE_DISABLE_EXTERNAL_SKILLS` set (v1.18.32 `skill/index.ts`) |
| Kilo | `KILO_DISABLE_EXTERNAL_SKILLS` set |
| Copilot, global scope | `COPILOT_HOME` set to anything but `~/.copilot` |

A flag reads as set unless it is unset, empty, or a false spelling (`0`,
`false`, `no`, `off`, `n`). An unclear value keeps the skill native. Two
copies are the safer failure, because a skill that does not load at all is
worse. `reads_pool` implies `pool_capable` for every vendor, and a test
pins it.

### D4: Migration uses the shipped reapers {#d4}

No new reaper is written.

- **Native to pool** (first run after upgrade, or a writer added):
  `reap_moved_outputs` deletes the unmodified native copy. An edited copy is
  kept and a warning names both paths. This is
  `adr_vendor_config_and_selection.md` A7-a unchanged: warn, never delete.
- **Pool to native** (the writer removed from `[options].clients`, then
  `grim update`): `reap_dropped_clients` drops the writer's record, and
  `shared_by_surviving_sibling` keeps the pool directory while the reader
  still records it. The next run moves the reader native and reaps the pool
  copy. It takes two runs, with one copy on disk at each step.

The upgrade fixture is
`test_shared_skills.py::test_a_native_opencode_copy_from_an_older_grim_moves_into_the_pool`.

### D5: Accepted contract change {#d5}

`shared_skills` was a `bool` whose `false` collapsed to unset. It is now
`Option<bool>` (`#[schemars(with = "bool")]`, so the schema type stays
`boolean`):

- `grim config set options.vendors.<n>.shared_skills false` now persists.
  `get` returns `false` and `config list` shows the row.
- `grim config list --all` reports the key's default as `null` instead of
  `false`.
- `config unset` removes the entry and returns the client to auto.

Before this change, writing `false` meant "native", which was already the
default. It still means native, now explicitly. The owner accepted the
visible change on 2026-11-05. It is recorded in
`docs/src/content/docs/upgrading.md#shared-pool-auto-join`.

The layout move itself is outside the freeze:
`docs/src/content/docs/stability.md` excludes the render layout, and D4
supplies the migration, reaper and upgrade fixture that the exclusion
requires.

### Consequences

**Positive:**
- One skill copy whenever a pool writer is present, at both scopes.
- Existing two-copy installs heal on the next `grim install` or `update`.

**Negative:**
- A downgrade to an older grim brings the native copy back beside the pool
  copy. That makes two copies, but no data is lost.
- OpenCode also scans `~/.claude/skills`. A Claude plus OpenCode user still
  sees a Claude-rendered duplicate. That is out of scope, because Claude
  never writes the pool.

**Risks:**
- A reader whose pool scan breaks upstream would stop seeing joined skills.
  Mitigation: the `reads_pool` gates are watchlisted in
  `.claude/rules/vendor-capability-watchlist.md`.

## Links

- [`adr_vendor_config_and_selection.md`](./adr_vendor_config_and_selection.md):
  D1, D2 and A7, amended here
- [`adr_render_layout_stability.md`](./adr_render_layout_stability.md): the
  layout-move contract D4 satisfies
- `.agents/plans/bugfix_plan_shared_pool_auto_join.md`: the fix plan

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-11-05 | maintainer | Initial decision: auto-join, selected plus recorded peers, tri-state `shared_skills`, accepted contract change |
