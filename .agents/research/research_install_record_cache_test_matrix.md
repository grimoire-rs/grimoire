# Research: Test matrix for "Install records as a cache of the disk"

## Metadata

**Date:** 2026-11-15
**Domain:** testing
**Triggered by:** test design for `.agents/adr/adr_output_ownership_without_state.md` (owner: "very good acceptance tests covering the different states and even copying/moving files over")
**Expires:** 2027-02-15 (code facts; re-read the cited functions before relying on them)

## Direct Answer

This matrix has 14 core lines by 8 command columns (112 cells, folded into 14 parametrized tests), 65 scenario tests and 24 edge cases.
Of the 89 scenario and edge cases, 58 are not in the ADR's 36 validation cases, and 31 overlap with one of them fully or in part.
The ADR's 36 stay valid. None is contradicted, but a few need a sharper setup (§6, items 19 and 20).
The biggest gaps:
- **One node at two versions.** One client's files match the record and another's match the lock. A3 then refuses with `65`. This happens with per-machine client sets and with client folders that are only partly gitignored.
- **A rule that gains or loses a support directory** across the pin move. A3 keeps the old `support_dir`, so the next `status` reads `modified`.
- **Pins dropped by `grim lock` and `grim add`**, which have no pre-pass.
- **Commits that move the lock but not the files**, such as a bot bump.
- **`uninstall` and `remove` in a cold clone** leave the files and destroy the only evidence.
- **CRLF checkouts** never prove.
- **Symbolic links.** Proof uses `Strict` containment while rows 1 to 3 read through relocated ancestors, and the install gate adopts a file link that A2 never proves.

## State model

Per artifact node, per recorded output, these dimensions matter:

| Dimension | Values that change the outcome |
|---|---|
| Evidence lock | absent · names the record's pin · names another pin · stale (declaration hash) · unparseable |
| Record | absent · present and current · present and lagging · `dev` · MCP `adopted: true` · legacy unfiltered hash · unreadable file |
| Files, per output | absent · equal to the record · equal to a render of the lock's pin · equal to a render of a third pin · neither (an edit) · a link · the wrong file type |
| Store | holds the record's pin · holds the lock's pin · holds neither |
| Network | online · offline · host down · host silent · pin gone (404) |
| Checkout model | committed outputs (CO) · gitignored outputs (GO) · mixed (some client folders ignored, or teammates with different client sets) · dotfiles at global scope (CO for `~/.claude`) |
| Kind | skill (verbatim or rendered `SKILL.md`) · rule with or without a support directory · agent (generated, pin embedded) · MCP entry in JSON or TOML · bundle members |
| Clients | one · several with separate folders · several sharing one pool folder · a client whose files exist but this machine does not select |
| Scope | project (anchored at `workspace`) · global (vendor roots, `global.json`) |

"Pin" in the ADR table means registry, repository and digest. The tag is ignored (`src/oci/pinned_identifier.rs:36-40`), and a path source compares its content hash only (`src/lock/locked_source.rs:61-66`).
A footprint hash covers relative path bytes plus file bytes of regular files only. It ignores modes, mtimes, links, empty folders and the ignore set (`src/install/content_hash.rs:176-231`).
An MCP entry hash is semantic, with sorted keys and no whitespace (`src/install/install_state.rs:341-344`).

Combinations that do not need their own tests:
- **GO plus row 4.** It arises only when files are copied in or a folder is partly ignored, and the expectations are the CO ones.
- **Hooks and settings in any row.** They are never proven (A1), so one "left alone" test is enough.
- **`dev` records in rows 4 and 5.** They are never proven (A1).
- **Offline with a warm store.** It behaves like online.
- **Store and network on rows 1 to 3.** No render happens.

## Core matrix: five rows × commands × two checkout models

Columns:
- **a** is `status` under Part A (store only, no write).
- **b** is `status`, TUI open and `grim_status --allow-writes` under Part B.
- **c** is `install`, which also covers `add`'s install and TUI Install.
- **d** is bare `update`, with tags unmoved unless stated.
- **e** is `update <name>`.
- **f** is `uninstall`.
- **g** is `remove`, then a bare `update`.
- **h** is `--force` on `install` or `update`.

A cell gives the expected result and its test ID. GO lines run the CO test with the model parameter flipped, so they share its assertions unless a cell says otherwise. "Today" values are from the code read on 2026-11-15.

| Line (row · model · how it arises) | a | b | c | d | e | f | g | h |
|---|---|---|---|---|---|---|---|---|
| 01 R1 · CO · steady state | `installed` · M01a | no render, 0 requests, `state.json` bytes and mtime kept · M01b | `unchanged` 0, no write · M01c | `unchanged` 0; tag moved: `updated` · M01d | as d, closure only · M01e | files deleted, `uninstalled` 0 · M01f | files kept, then `removed` · M01g | `unchanged`: force writes nothing over intact files · M01h |
| 02 R1 · GO · steady state | as M01a · M02a | as M01b · M02b | as M01c · M02c | as M01d · M02d | as M01e · M02e | as M01f · M02f | as M01g · M02g | as M01h · M02h |
| 03 R2 · CO · lock-only commit on a machine with a record | `outdated` · M03a | no render, no write · M03b | `updated` 0, re-render, old layout reaped · M03c | no pre-pass render; re-render to the carried pin, row `restored` (label unchecked) · M03d | as d in closure · M03e | record-driven delete · M03f | pruned, `removed` · M03g | as c · M03h |
| 04 R2 · GO · pull or branch checkout | as M03a · M04a | as M03b · M04b | as M03c · M04c | as M03d · M04d | as M03e · M04e | as M03f · M04f | as M03g · M04g | as M03h · M04h |
| 05 R3 · CO · hand edit | `modified` · M05a | no write · M05b | 65 `reason: modified`, file and record kept · M05c | 65 `refused: true`; dropped node `kept-modified` · M05d | 65 inside closure, untouched outside · M05e | files deleted anyway (uninstall gates only drifted entries, `src/install/uninstall.rs:208-213`); MCP entry 65 · M05f | `kept-modified`, files kept · M05g | overwritten, 0 · M05h |
| 06 R3 · GO · hand edit | as M05a · M06a | as M05b · M06b | as M05c · M06c | as M05d · M06d | as M05e · M06e | as M05f · M06f | as M05g · M06g | as M05h · M06h |
| 07 R4 match · CO · pull, older-branch checkout, restored older `state.json` | `installed` if the lock's pin is in the store, else `modified` plus one note naming `grim install` · M07a | record re-pinned, `installed`; 2nd run silent, no write · M07b | `unchanged` 0, record re-pinned (today 65) · M07c | pre-pass re-pins from the previous lock, then rolls; 0 (today 65) · M07d | pre-pass on `NamedScope::force()` ∩ previous lock only · M07e | files deleted (as today); drifted MCP entry removed, no 65 (ADR #7) · M07f | `remove` re-pins before `save_lock`, `update` prunes; cold store at `remove`: `kept-modified` · M07g | overwrites identical bytes, 0 · M07h |
| 08 R4 match · GO · render copied in, or partly ignored folder | as M07a · M08a | as M07b · M08b | as M07c · M08c | as M07d · M08d | as M07e · M08e | as M07f · M08f | as M07g · M08g | as M07h · M08h |
| 09 R4 no match · CO · pull plus local edit, or other grim's bytes | `modified` · M09a | no write; `<u> left unproven` · M09b | 65 `modified`, record unchanged · M09c | shipped: 65; dropped: `kept-modified`, no second line · M09d | as d in closure · M09e | files deleted (today; ADR silent) · M09f | `kept-modified` · M09g | overwritten · M09h |
| 10 R4 no match · GO · copied-in render of a third pin | as M09a · M10a | as M09b · M10b | as M09c · M10c | as M09d · M10d | as M09e · M10e | as M09f · M10f | as M09g · M10g | as M09h · M10h |
| 11 R5 match · CO · clone, second worktree, `git clean -fdx` | `missing` · M11a | record written, `installed`, `adopted 1 …`; offline and cold: `missing` plus `offline:` (ADR #24-#26) · M11b | `unchanged` 0, adopted (today) · M11c | dropped files removed, rows `updated`/`removed`, 0 (today 65) · M11d | pre-pass on closure ∩ previous lock · M11e | warm: deleted; cold: left, named, `uninstalled` 0 (ADR #19) · M11f | warm: proven, then pruned; cold: stays, named (ADR #20) · M11g | install: same as c; update: proven drops removed, unproven stay named · M11h |
| 12 R5 match · GO · `.grimoire/` removed, outputs kept | as M11a · M12a | as M11b · M12b | as M11c · M12c | as M11d · M12d | as M11e · M12e | as M11f · M12f | as M11g · M12g | as M11h · M12h |
| 13 R5 no match · CO · clone of a lock-only commit, other grim, CRLF | `missing` · M13a | no write; `left unproven` · M13b | 65 untracked, today and after · M13c | shipped: 65; dropped: stays, A9 line · M13d | as d in closure · M13e | stays, A9 line, `uninstalled` 0 · M13f | stays, named · M13g | install `--force` overwrites; update `--force`: dropped stays named · M13h |
| 14 R5 no match · GO · copied-in render of another pin | as M13a · M14a | as M13b · M14b | as M13c · M14c | as M13d · M14d | as M13e · M14e | as M13f · M14f | as M13g · M14g | as M13h · M14h |

No cell is "not applicable". Column b runs only after Part B ships. Columns d and e on rows 4 and 5 need a registry: use a warm store unless the cell says cold.

## Scenario tests

"In 36" names the ADR validation case that already covers the scenario. "part" means a narrower overlap. Every acceptance test asserts tokens (kind, name, path, reason), never whole sentences (A9).

| ID | Scenario | Setup | Action | Expected | ADR | In 36 | Pri | Level |
|---|---|---|---|---|---|---|---|---|
| S01 | Clone, then install | Alice installs and commits; Bob clones with a fresh home | `install` | `unchanged` 0; `state.json` created; file bytes unchanged | A4 | #22 | must | acc |
| S02 | Clone, update drops a file inside a skill | Alice at A; tag moves to B without `ref.md`; Bob clones | `update` | 0; `ref.md` gone; row `updated`; record at B | A6, A7 | #8 | must | acc |
| S03 | Pull a teammate's update, then install | Bob installed A; Alice updates to B and commits; Bob pulls | `install` | failing first: 65; then 0 `unchanged`, record at B; `status` `installed` | A4 | #2 | must | acc |
| S04 | Pull, status, B not in Bob's store | as S03 | `status`, `--format json` | `modified` plus one note naming `grim install`; no new key; no write | A5 | #4 | must | acc |
| S05 | Pull, status, B in Bob's store | as S03, store primed with B | `status` | `installed`; `outputs` from the proven record; no write | A5 | #4 | must | acc |
| S06 | Pull, then update to a third version | as S03; tag moves to C | `update` | 0; pre-pass re-pins to B from the previous lock; C on disk | A6, A7 | #5 | must | acc |
| S07 | Branch ping-pong, committed outputs | Bob at B; checks out the A branch, then back | `install` and `status` after each checkout | 0 `unchanged` each time; `installed`; a 2nd `install` keeps file mtimes | A4 | #3 part | must | acc |
| S08 | Pull a bundle that dropped a member | Bob's record holds member m; git deleted m's files | bare `update`; `update <bundle>` | `removed`, 0; no warning; record gone | prune | no | should | acc |
| S09 | Pull a teammate's `remove x`; files still committed | Bob's record at the same pin | bare `update` | x `removed`. In a fresh clone x stays and nothing names it (the ADR's stated limit) | drift table | no | should | acc |
| S10 | Second worktree, shared `GRIM_HOME` | `git worktree add` | Part B `status`; `update` | `installed`; record written; 0 proof requests (counting proxy) | A6, B1, B2 | #18 part | must | acc |
| S11 | Merge: lock "theirs", outputs "ours" | resolve the conflict by side | `status`, `install` | row 2: `outdated`; `install` re-renders, 0 | table | no | should | acc |
| S12 | Merge: lock "ours", outputs "theirs" | reverse of S11 | `status`, `install` | row 3: `modified`, 65 (correct, but the message misleads: §6.14) | table | no | should | acc |
| S13 | Merge makes the lock stale | both branches edit `grimoire.toml` | `status` (A and B), `grim lock`, `install` | `stale`; no proof, no write; after `grim lock`, `install` 0 | A5 | no | should | acc |
| S14 | Lock left with conflict markers | unresolved merge | `update` | 78; `state.json` bytes unchanged; no lock write | A7 | no | could | acc |
| S15 | Stash an uncommitted update | Bob ran `update` to B, uncommitted; `git stash`; later `git stash pop` | `status`, `install` after each step | 0 everywhere, no 65; a new B artifact's untracked files stay with their record | A4 | no | should | acc |
| S16 | `git clean -fdx` | CO and GO | `status`, `install` | CO: row 5, adopted; GO: nothing on disk, plain install; no 65 | A4 | no | should | acc |
| S17 | Lock-only commit (a bot bumps the lock) | Lock at B, committed files at A | (a) Bob with a record: `install`; (b) fresh clone: `install`, `update` | (a) `updated` 0; (b) 65 untracked, reason names the pin; `update` 65 | table | no | must | acc |
| S18 | Teammate on another grim version | Alice renders with `checked_old_grim` | Bob `install` | identical skill bytes: 0; differing generated rule: 65 `modified`, record unchanged | drift table | no | should | acc |
| S19 | GO pull | GO repo; teammate updates to B | `status`, `install`, `update` | `outdated`; `install` re-renders and reaps; `update` sends 0 proof requests | table | #1 part | must | acc |
| S20 | GO branch ping-pong | GO repo | `install` after each checkout | re-renders each time, 0 | table | no | should | acc |
| S21 | One node at two versions | `.claude/` committed at B; `.github/` copilot output at A (ignored, or Alice selects only claude) | `status`, `install` | ADR: `modified`, 65. Suggested: `outdated`, `install` 0 (§6.1) | A3 | no | must | acc |
| S22 | Two clients after a pull, one hand-edited | | `install` | `modified`, 65, record unchanged | A3, A4 | #6 | must | acc |
| S23 | `cp -r` of the whole workspace, `.grimoire/` included | | in the copy: `status`, `uninstall x` | `installed`, no write; uninstall deletes the copy's files only; original untouched | anchors | no | should | acc |
| S24 | Copy without `.grimoire/` (rsync or tar exclude) | same machine | Part B `status` | adopted from the warm store, 0 requests | B1 | no | should | acc |
| S25 | Rename the workspace directory | | `status`, `update`, `uninstall` | `installed`, no write; every command works | anchors | no | should | acc |
| S26 | `state.json` copied from a worktree on another branch | | `status`, `install` | per node rows 1, 2 or 4; `install` 0, no 65 | A4 | no | should | acc |
| S27 | Older `state.json` restored from a backup | save state at A; update to B; restore the saved state | `status`, `install` | row 4 match: `installed`; `install` 0, record at B | A4, A5 | no | must | acc |
| S28 | Interrupted update, simulated | as S27, plus one artifact's files reverted to A | `install` | the B node `unchanged`, the A node `updated`; 0 | A4 | no | must | acc |
| S29 | Render copied by hand from another project | same pin; other pin; same digest at another repository | `install`, `update` | same pin: adopted (accepted residual); other pin: 65; other repository: verbatim skill adopted, generated rule 65 | A2 | no | should | acc |
| S30 | Skill copied into another client's folder | `.claude/skills/x` copied to `.cursor/skills/x` (cursor not selected), and to a selected client | `update` dropping x | unselected copy untouched and unnamed (§6.10); selected plain skill proven and removed; namespaced-metadata skill unproven, named | A6 | no | should | acc |
| S31 | Project to global by moving files by hand | files moved to `~/.claude/skills/x`; the global lock pins x | `--global status` (B); project `update` | global `installed`; project record pruned with nothing to delete | A10 | no | could | acc |
| S32 | Partial skill copy | one file missing; one extra regular file; extra `__pycache__/` | `update` dropping it | missing or extra file: unproven, named; `__pycache__/`: proven, removed with the folder | A2 | #13 part | should | acc |
| S33 | Rule gains a support directory | A has none, B ships `foo/`; pull (CO) | `status` twice, `install`, `uninstall` | `installed` on both runs (ADR text: 2nd run `modified`, §6.2); uninstall removes both roots and the `claudeMdExcludes` element | A3 | no | must | acc |
| S34 | Rule loses its support directory | A ships `foo/`, B has none; git deletes `foo/` | `status`, `install` | `installed`; the exclusion element retires on the next write | A3 | no | should | acc |
| S35 | Generated agent or rule embeds the pin | pull; and a clone with an old-pin file under a new lock | `install` | matches only a preview rendered with the lock's pin string; the old-pin file is unproven | A2 | no | should | acc |
| S36 | MCP entry in `.mcp.json` after a pull | Alice's update changes the server value | `install`, `status` | 0 `unchanged`; record re-pinned; `adopted` kept | A2-A4 | no | must | acc |
| S37 | MCP entry adopted in a clone, then uninstalled | adoption sets `adopted: true` | `uninstall mcp x`; again with `--force` | entry kept plus the kept-entry warning, `uninstalled` 0; `--force` removes it (§6.12) | A3, A8 | #21 part | must | acc |
| S38 | MCP entry in Codex TOML | `.codex/config.toml` | as S36 | as S36 | A2 | no | could | acc |
| S39 | Bundle renames, adds and drops members | clone | `update` | old members `removed`; new ones adopted or installed | A7 | #9 | must | acc |
| S40 | Shared project pool | three pool clients on `.agents/skills/x` | pull, then `install`; `update` dropping x | one proof; one record with three outputs; one delete | A2, A7 | no | should | acc |
| S41 | Path source after a pull | Alice edits `./skills/x`, relocks, commits | Bob `install` | 0 `unchanged` | A9 | no | should | acc |
| S42 | Committed dev-install outputs | clone | `status`, `update` | never a candidate; untouched; no line | A1 | no | should | acc |
| S43 | Declaration removed by hand, then `grim lock` | clone, no record | `grim lock`, later `update` | ADR: files stay, nothing names them (§6.4) | none | no | must | acc |
| S44 | `grim add y` whose relock drops pin z | clone | `add` | as S43 | none | no | should | acc |
| S45 | Hooks and settings are left alone | clone declaring a hook and a settings package | Part B `status`, `update`, TUI open | no hook or settings record; dispatch table and settings file untouched and unread | A1 | no | must | acc |
| S46 | `update --force` in a clone | dropped members, proven and unproven | `update --force` | proven `removed`; unproven stay, named; shipped overwritten | A8 | no | must | acc |
| S47 | `update <name>` in a clone | | named update | pre-pass only on closure ∩ previous lock; 0 requests for other nodes | A6 | no | should | acc |
| S48 | Dotfiles pull at global scope | `~/.claude` is a git repository; `global.json` lags | `--global install` | 0 `unchanged`; with a stow ancestor link see §6.7 | A10 | #23 part | should | acc |
| S49 | `status --check` on row 4, Part A | | `status --check` | proof stays store-only; `--check` fields unaffected | A5, B7 | no | could | acc |
| S50 | `grim_status`, with and without `--allow-writes` | clone, cold store | two MCP calls each | no write / write; no request; the 2nd call renders nothing | B7 | #34 | must | acc |
| S51 | TUI open, scope toggle, delete during the task | `TestBackend` | | badges re-derived; commit between actions | B6 | #35 | should | unit |
| S52 | Cold store, online | clone | `update` | old pin fetched; stale files go | A9 | #15 | must | acc |
| S53 | Old pin gone (404) | previous lock's digest rewritten to an absent digest | `update`, `uninstall` | unproven, named; exit 0; no 79 for the old pin | A9 | #16 | must | acc |
| S54 | Unreachable host | two dropped rows | `update` | one attempt; both named | A9 | #15 | should | acc |
| S55 | Silent registry | | Part B `status` twice | ends inside the deadline, exit 0; the 2nd run within 10 min sends nothing | B4 | #32 | must | acc |
| S56 | More candidates than the fetch cap | 70 recordless candidates, cold store | Part B `status` twice | 1st: at most 64 fetched, the rest named; 2nd: the rest adopted (§6.13) | B4 | no | should | acc |
| S57 | Offline update; old pin missing, new pin present | store primed with B only | `update --offline` | no 81; the old files are named | A9, A10 | #16 | must | acc |
| S58 | Registry that only the project file marks insecure | | | status fetching is gated on B9; pinned by ADR #33 | B9 | #33 | must | acc |
| S59 | Same command twice | after a row 4 or row 5 heal | `install` twice; `status` twice | 2nd run writes no file (mtime and inode kept) and no `state.json` byte; whether `install` skips the persist today is unchecked | B2 | #24 part | must | acc |
| S60 | Status writes, then pull, then install | Part B | | 0 `unchanged`, no 65 | plan order 3 | no | must | acc |
| S61 | Install on branch X, check out Y, update | CO and GO | `update` | CO: pre-pass row 4 against Y; GO: row 2, no proof render | A6 | no | should | acc |
| S62 | Status commit concurrent with install | | | no 75; the final state equals the install's | B3 | #28 | should | acc |
| S63 | Update fails after the pre-pass | offline; new pin not in the store | `update --offline`, then `install` | 81; records persisted before the lock save; the next `install` re-renders (row 2) | A7 | #17 part | should | acc |
| S64 | 0.15.1 reads a state file Part B wrote | | old `status` | reads it | compat | #36 | must | acc |
| S65 | 0.15.1 in a row 4 state | | old `install` | 65 (cannot change; pins the docs note) | compat | no | could | acc |

## Edge cases that are not edits

| ID | Case | Setup | Action | Expected | ADR | In 36 | Pri | Level | What the code does today |
|---|---|---|---|---|---|---|---|---|---|
| E01 | CRLF on checkout | `git config core.autocrlf true` (works on Linux), or rewrite bytes | `status`, `install` after a pull or clone | row 4 or 5 no match. Suggested: a named reason (§6.6) | A2 | no | must | acc | 65 in both cases; the hash is byte-exact (`content_hash.rs:176-186`), checked |
| E02 | Exec bit or mode change | `chmod 755` a skill script | `status`, `install` | `installed`, `unchanged` | none | no | should | acc | the hash ignores mode, checked |
| E03 | mtime-only change | `touch` every file | `status`, `install` | `installed`, no write | none | no | should | acc | ignored, checked |
| E04 | Case-only rename on a case-insensitive file system | `README.md` becomes `readme.md` | `status` | `modified`, or unproven | none | no | could | unit | the key is the on-disk name (`content_hash.rs:217`), checked; real file systems unchecked |
| E05 | Unicode normalization of a file name (NFC to NFD) | rename | `status` | `modified`, or unproven | none | no | could | unit | key bytes are the on-disk bytes, checked; macOS unchecked |
| E06 | Editor adds a trailing newline | after a pull | `install` | 65 `modified`: one byte is an edit | A2 | no | should | acc | checked |
| E07 | Live link to a file at the destination | identical target bytes | `install` vs `update` dropping it | both must give the same answer (§6.8) | A2, A4 | no | must | acc | `install` adopts identical bytes (`installer.rs:1259-1261`), checked |
| E08 | Link to a folder; dangling link | | `install`, `update` | 65, or unproven with "a symbolic link to <resolved>"; the target survives | A2 | #13 | must | acc | refused 65 (`installer.rs:1262-1267`), checked |
| E09 | Client root links inside the workspace | `.claude -> config/claude` | `update` dropping x | proven: `Strict` canonicalizes both sides | A2 | no | should | acc | unchecked |
| E10 | Client root links outside the workspace | `.claude -> ../dotfiles/claude` | `status`, `update` | rows 1-3 read through it on Unix; proof refuses with "under <link> outside its root" | A2, A9 | #13 | must | acc | `status` reads through `AllowRelocatedAncestor` (`status.rs:2950`), checked |
| E11 | Link on an ancestor below a global root (stow) | `~/.claude/skills -> ~/dotfiles/...` | `--global install` after a dotfiles pull | ADR: `modified` 65 for good (§6.7) | A2 | no | should | acc | Unix only; on Windows the relaxed mode equals `Strict` (`path_anchor.rs:496-500`), checked |
| E12 | `.grimoire` links out of the workspace | | Part B `status` | reports from memory, `records not written:`, exit 0, nothing written outside | B3 | #27 | must | acc | followed (`install_state.rs:799-815`), checked |
| E13 | `.grimoire` is a regular file | | Part B `status`; `install` | status degrades, exit kept; install keeps today's error | B3 | no | should | acc | descriptive error (`install_state.rs:805-814`), checked |
| E14 | Empty folder or extra link inside a skill folder | | `update` dropping it | proven; the link target survives | A2 | #13 | must | acc | skipped by the hash (`content_hash.rs:221-229`), checked |
| E15 | Ignored junk: `__pycache__/`, `.DS_Store`, `node_modules/`, `.git/` | | `update` dropping it | proven; removed with the folder (residual, including a user's `.git/`) | residual | no | should | acc | `DEFAULT_PATTERNS` (`ignore_set.rs:25`), checked |
| E16 | Destination's own `.grimignore` edited | a line that hides a new file | `status`; proof | `status` `installed` (edit hidden); proof fails because the `.grimignore` bytes differ | A2 | no | could | unit | the ignore file is read from disk (`content_hash.rs:193`), checked |
| E17 | Tag-only lock change, same digest | | `status`, `install` | row 1: `installed`, no render | table | no | should | acc | tag ignored (`pinned_identifier.rs:36-40`), checked |
| E18 | Same digest at another registry or repository (mirror) | | `status`, `install` | row 2 (skill bytes identical); generated files change their provenance line | table | no | should | acc | registry and repository compared; provenance drops only the tag (`locked_source.rs:111-116`), checked |
| E19 | Path source moved, same content | | `status` | row 1 | table | no | could | acc | hash-only compare, checked |
| E20 | One output absent, another drifted, pin changed | | `status`; `install` | `missing` comes before any proof (§6.15); `install` restores and re-records | A5 | no | should | acc | `status.rs:2948-2956`, checked |
| E21 | Legacy record hash from before `.grimignore` | | `status`, `install` after a pull | rows 1-3 via `footprint_hash_for_record`; row 4 compares filtered with filtered | A2 | no | could | unit | `content_hash.rs:114-126`, checked |
| E22 | A folder `foo.md/` where a rule file belongs | | `update` | not proven | A2 | #13 | must | acc | checked |
| E23 | Committed `state.json` whose hashes do not match | stale or hostile committed record | `status`, `install`, `update` | row 4 decides; the record alone grants no deletion | threat model | no | should | acc | load drops settings records and illegal surfaces only (`install_state.rs:1027-1032`), checked |
| E24 | Hook record in a copied workspace | `cp -r` including `.grimoire/` | `uninstall hook` in the copy | outside the ADR; see §8 | none | no | could | acc | payload key checked for shape, not equality (`install_state.rs:1964-1977`), checked; the effect is unchecked |

## Gaps in the ADR

Each suggestion below is mine and is not a decision.

1. **One node at two versions.** One client's files match the record (pin A) and another's match the lock (pin B). This happens when teammates' client sets differ (`src/install/target.rs:166-200`) or a repository ignores some client folders only.
   **Why unsettled:** A3 re-pins only when every present output proves against the lock, so the row stays `modified` and `install` exits 65. Yet neither output is a local edit.
   **Suggestion:** classify per output. At the gate, refuse only an output that matches neither the record nor the lock render. `status` reads `outdated`, and the record keeps pin A until `install` re-renders.
   **Test:** S21.
2. **Support directory across the pin move.** A3 row 4 says `support_dir` keeps its value. But `support_dir` is per output (`install_state.rs:102`) and `current_hash` hashes with it (`:180`).
   **Why unsettled:** when B adds a support directory, the record keeps `None` with a hash over index plus support, so the next `status` reads `modified`.
   **Suggestion:** take `support_dir` from the evidence version, as row 5 already does.
   **Test:** S33, two `status` runs.
3. **Lock-only movers.** The ADR assumes "whoever moves a committed lock commits the resulting tree". A bot bump, or a `grim lock` committed without `install`, breaks that assumption.
   **Why unsettled:** in a clone this is row 5 no match, so `install` and `update` exit 65, and the user's only route is `--force`.
   **Suggestion:** name the case. The refusal reason says "files differ from <pin>", and the docs say `--force` is safe when the files are an older grim render.
   **Test:** S17.
4. **`grim lock` and `grim add` drop pins but have no pre-pass.** A6 lists `update`, `uninstall` and `remove` only.
   **Why unsettled:** in a clone, files of a hand-removed declaration become unprovable forever and are never named.
   **Suggestion:** give both the `remove` pre-pass (lock as found, store only) for the nodes they drop, or at least the A9 line.
   **Test:** S43, S44.
5. **`uninstall` and `remove` in a cold clone.** A fresh clone always has a cold store, and these commands stay offline (`adr_effective_set_mutations.md:42`).
   **Why unsettled:** exit 0 leaves the files and the edit then destroys the only evidence. On a machine with a record the same `uninstall` deletes even edited files (`uninstall.rs:208-213`). The lagging record makes a difference, against owner decision 7.
   **Suggestion:** before the config edit, refuse when a target has present destinations that are unproven only for "not in the local store", naming `grim install` first.
   **Test:** M11f and M11g, cold.
6. **CRLF checkouts.** With `core.autocrlf` and committed outputs, every pulled or cloned text file differs from the LF render. This already breaks a clone today (65).
   **Why unsettled:** the ADR is silent.
   **Suggestion:** do not normalize the hash, because that changes recorded values. Add an A9 reason "line endings differ from the render" and document `eol=lf` in `.gitattributes` for client folders.
   **Test:** E01.
7. **Symlinked ancestors and the dotfiles model.** Rows 1 to 3 read through a relocated ancestor (`status.rs:2950`, `installer.rs:1989`), but A2 demands `Strict`.
   **Why unsettled:** a stow or yadm user's pull is row 4, never proven, so it stays `modified` for good. A dotfiles repository at global scope is a third checkout model the ADR does not name.
   **Suggestion:** split proof for re-recording (read, may follow an ancestor link) from proof for deletion (`Strict`).
   **Test:** E11, S48.
8. **A link to a file at the destination.** The gate adopts a live file link with identical bytes (`installer.rs:1259-1261`), and A4 keeps the gate's row 5. A2.2 says a link is never proven.
   **Why unsettled:** `install` adopts what `update`, `status` and `uninstall` call unproven.
   **Suggestion:** one predicate for both callers; I would refuse links in both and document the change.
   **Test:** E07.
9. **The row 4 match in `install` rewrites identical bytes.** The match falls through to materialize (`installer.rs:1297-1301`), while A3 says the pre-pass writes no byte under a client root.
   **Why unsettled:** mtimes change, watchers fire, and on Windows an open file can fail the swap.
   **Suggestion:** record only, in both paths.
   **Test:** S59, checking mtime and inode.
10. **Files at a client this machine does not select.** A6 candidates come from `effective_clients`, and with no record that means selected clients only.
    **Why unsettled:** a dropped node's copy in an unselected client's folder stays, and A9 does not name it.
    **Suggestion:** probe every client hosting the kind for the warning only, never for deletion, with the reason "client not selected here".
    **Test:** S30.
11. **"Digest" means the pin.** `eq_content` compares registry, repository and digest. A mirror move is row 2 or 4, and a tag-only change is row 1.
    **Why unsettled:** the ADR table does not say.
    **Suggestion:** state it in the table.
    **Test:** E17, E18.
12. **A row 5 MCP entry gets `adopted: true`.** A later `uninstall` in that clone keeps the entry, while the machine that wrote it removes it.
    **Why unsettled:** the outcome conflicts with owner decision 7.
    **Suggestion:** keep `true`, because a user-typed identical entry cannot be told apart, but say so in the ADR, and make the kept-entry warning name `--force`.
    **Test:** S37.
13. **Part B fetch caps.** Nothing says what candidates beyond 64 manifests read, or that later runs converge.
    **Suggestion:** reason "never attempted (fetch budget)", continued on the next run.
    **Test:** S56.
14. **Row 3 when the files are another pin's render** (lock "ours", outputs "theirs").
    **Why unsettled:** the result `modified` is safe but reads like a hand edit.
    **Suggestion:** no behaviour change; the refusal may say "files may come from another version; check the lock".
    **Test:** S12.
15. **`missing` outranks row 4 in `status`.** Any absent active output returns `missing` before drift is checked (`status.rs:2948-2956`).
    **Why unsettled:** A5 is silent on the mix.
    **Suggestion:** keep `missing`, since `install` restores, and pin it.
    **Test:** E20.
16. **Part B on a stale lock.** B1 adopts "against the lock as found", and A5 reads `stale` first.
    **Why unsettled:** it is unclear whether status writes records under a stale lock.
    **Suggestion:** skip adoption while stale, because a merged lock may describe neither side.
    **Test:** S13.
17. **`git stash -a` plus Part B.** The stash takes the ignored `.grimoire/`; `status` then writes a new `state.json`; `git stash pop` refuses because the file exists.
    **Suggestion:** document it, with no code change.
    **Test:** none required; could.
18. **`uninstall` of a row 4 no-match node with a record** deletes the edited files, as row 3 does today.
    **Why unsettled:** A8 is silent.
    **Suggestion:** keep today's rule and pin it.
    **Test:** M09f.
19. **ADR #16's "no 81" premise.** It holds only when the new pin is in the store. In a clone the new pin is cold too, so the install pass exits 81 for the new content.
    **Suggestion:** rewrite #16's setup as S57 (store primed with B only).
20. **ADR #28 needs a delay seam.** "Run install during a delayed status commit" needs a way to hold the commit open.
    **Suggestion:** a test-only delay, or `held_flock` on the sidecar instead (`test/src/helpers.py:435`).
    **Test:** S62.

## Fixtures and helpers the tests need

| Helper | Purpose | Exists? |
|---|---|---|
| `git_team(tmp, model)` | bare origin plus Alice and Bob clones, each with its own `GrimRunner`; GO writes a root `.gitignore` for client folders | new; `git` subprocess use exists in `test/tests/test_git_provenance.py` |
| `fresh_runner(tmp, name)` | `GrimRunner` with `GRIM_HOME` **and** `HOME` in their own parent | new. `GrimRunner` sets `HOME = grim_home.parent / "home"` (`test/src/runner.py:64`), so two homes in one parent share `HOME` and the global pool |
| Global cold store | fresh `GRIM_HOME` holding a copy of `grimoire.toml` and `grimoire.lock` only, since the global lock and `global.json` live in `$GRIM_HOME` | new, small |
| `state_snapshot()` / `assert_unchanged()` | `state.json` bytes, `st_mtime_ns` and output inodes | new, small |
| `render_at(pin)` | render a pin in a scratch project and copy the tree over a target (copy, partial copy, other client folder) | new, on top of `make_artifact` and `write_config` |
| `make_artifact(..., tag=)`, `retag`, `make_bundle` | versions A, B and C; members dropped, renamed, added | exist (`test/src/helpers.py:341`, `:118`; `test/src/registry.py:327`) |
| `counting_proxy(registry)` | forwards to the test registry and counts requests per path; configs name the proxy host so pins name it; `GRIM_INSECURE_REGISTRIES` for its port | new (no OCI proxy exists) |
| `silent_registry()` | accepts TCP and never answers | new (pattern: the `hang` mode in `test/src/otlp_fixture.py`) |
| `gone_pin(lock, name)` | rewrite a pin to a digest the registry lacks (no registry delete needed) | new |
| `checked_old_grim` | teammate's render on 0.15.1; interop | exists (`test/src/old_grim.py:29`) |
| `held_flock` | hold the state-file sidecar for B3 | exists (`test/src/helpers.py:435`) |
| MCP stdio driver | `grim_status` calls | exists (`test/src/stdio_drivers.py`) |
| `to_crlf(path)` | CRLF without Windows | new, one line; real `core.autocrlf true` also works on Linux |
| TUI `TestBackend` | B6 | exists (`src/tui/app.rs:10610`) |

Infeasible or fragile with today's harness:
- A real case-insensitive or Unicode-normalizing file system. The Linux runner has neither, so E04 and E05 are unit tests on the key bytes.
- Killing a process mid-write deterministically. S27 and S28 simulate the result by restoring state.
- ADR #28's delayed commit without a seam (§6.20).
- E12 and E13 permission tests, which pass vacuously as root; skip them when `os.geteuid() == 0`.
- A real Windows `autocrlf` run. On a host with no Windows (the herdr host), that needs a CI deep run.

## Cases deliberately left out

- **Proof of hooks and settings.** A1 excludes them, so only the "left alone" test S45 stays.
- **The credential-transport defect class.** B9 and ADR #33 own it, so no reproduction is written here.
- **A shared `GRIM_HOME` across machines.** `global.json` is last-writer-wins, the v1 stance in `subsystem-install-state.md`.
- **Devcontainer named volumes over `.grimoire/`**, and a workspace equal to `$HOME`, where the project pool is the global pool. Both are documented edge setups, not drift between lock, record and files.
- **Fixing 0.15.1.** It cannot get row 4; S65 only pins the refusal.
- **The `grimoire-vscode` change.** It is in a sibling repository.
- **Performance on large trees**, beyond the B2 no-op checks.
- **Hook records in a copied or moved workspace (E24).** Load accepts any well-shaped payload key, so a copy's hook record names the original workspace's payload folder. Whether `uninstall hook` in the copy deletes the original's payload is unchecked. It belongs in its own bugfix check, not in this ADR.
