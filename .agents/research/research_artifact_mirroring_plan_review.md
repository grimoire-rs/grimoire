# Research: Artifact mirroring plan review (SOTA and pitfall gap check)

## Metadata

**Date:** 2026-11-24
**Domain:** cli | packaging | security
**Triggered by:** hex review panel on `adr_artifact_mirroring.md` and `design_artifact_mirroring.md`
**Expires:** 2027-05-24

## Direct Answer

Two design blocks, both in marketplace enumeration (spec 5.3): the default-directory rule for `agents`, and the missing `strict` handling. The rest is Warn or Suggest. Sources read 2026-11-24:
- https://code.claude.com/docs/en/plugins/manifest-reference (read)
- https://code.claude.com/docs/en/plugins/marketplace-reference.md (read)
- https://code.claude.com/docs/en/plugin-marketplaces (read)
- https://code.claude.com/docs/en/skills (read, first 100k chars)
- https://code.claude.com/docs/en/sub-agents (read, first 100k chars)
- https://doc.rust-lang.org/std/process/struct.Command.html (read)
- https://docs.rs/spdx/latest/spdx/ (read; thin)

Licence-threshold comparisons (licensee, askalono, scancode) are from memory and not re-fetched.

## Findings

1. Component default directories (Block, spec 5.3). Per the manifest reference, `skills` ADDS to the default `skills/` scan. `agents` and `commands` REPLACE their defaults, and `agents` takes `.md` files only, never directories. Spec 5.3 says "agents from `agents/*.md` plus listed `agents` paths", which is wrong whenever `plugin.json` or the entry sets `agents`: then `agents/` is not scanned. Entry `skills` for a root-source entry also replaces the default `skills/` (marketplace reference, "How entry fields combine"). Fix: model per key (skills add, agents replace, entry-skills on a root source replace), or the mirror publishes units Claude Code would not load, or misses ones it would.
2. `strict` (Block, spec 5.3). The spec never mentions `strict`. Default `true`: `plugin.json` is authoritative and entry `skills`/`agents`/`commands` are appended to it. No `plugin.json`: the entry is the manifest. `strict: false` plus a `plugin.json` plus entry components is a load conflict. Fix: read `plugin.json` when present, union the entry's component fields, skip a conflicting plugin as `component-unsupported` or `refused`.
3. `metadata.pluginRoot` (Warn, spec 5.3). The spec joins it to every string source. Docs: it applies only to a bare name (no `/`, no leading `./`) and needs Claude Code 2.1.239 or later. A `./x` source ignores it. `"."` is the marketplace root. Fix: apply it only to bare names. Reject `..` and a backslash in the source.
4. Source shapes (Suggest, spec 5.3). The docs list `github`, `url` (any git URL, `file://` too), `git-subdir`, `npm`, `archive`, `command`. Spec 5.3 covers these. Gaps: `url` may be `git@` or `http://`; `git-subdir.url` may be `owner/repo` shorthand; `github.repo` is `owner/repo`; `sha` wins over `ref`. A marketplace added by `url` has no relative-path plugins at all. Fix: normalise `git@`/`http://` as unsupported (https and file only), and handle the shorthand.
5. Commands (Warn, ADR D8). The skills page says custom commands "have been merged into skills", and the manifest reference says "Prefer `skills/` for new plugins". So skipping `commands/` is defensible, but D8's "documented merged" claim now makes the skip a policy choice, not a necessity. A command already reaches the model as a skill-like entry, so skipping it loses real content. Fix: keep the skip, label it deferred, and name the semantic gap (commands default user-invocable and have no required `description`).
6. Skill frontmatter (Suggest, spec 3.4). The native skill keys are `when_to_use`, `disable-model-invocation`, `user-invocable`, `allowed-tools`, `disallowed-tools` (hyphenated for skills), `arguments`, `argument-hint`, `model`, `effort`, `context`, `agent`, `background`, `hooks`, `paths`, `shell`, `metadata`, `license`, `compatibility`. `vendor_claude.rs` covers all non-executable rows, `hooks` excluded on purpose. Nothing to add. Note: claude.ai and the Skills API accept only `name`, `description`, `license`, `compatibility`, `metadata`, `allowed-tools`.
7. Agent frontmatter (Suggest, spec 3.4). Native keys (camelCase) also include `color`, `isolation`, `initialPrompt`, `omitClaudeMd`, `autoCompactWindow`, `experimental`, `mcpServers`, `hooks`. The registry lacks `color`, `isolation`, `initialPrompt`, `omitClaudeMd`, `autoCompactWindow`. Convert drops them as `unknown-field`, which is safe for `mcpServers`/`hooks`/`isolation`/`initialPrompt` but loses harmless `color`. Fix: list `color` in the registry, or accept the drop and say so in the docs.
8. Windows exit and Ctrl-C (Warn, ADR D2). Spawn-and-wait maps a child code outside 0 to 255 to 1. A Windows exit code is a u32 (NTSTATUS like `0xC0000005`), and git passes it through. Fix: pass the child's full code (`as i32`). Also install an ignore-Ctrl-C handler in the parent while it waits, or Ctrl-C kills grim and orphans the child.
9. `GRIM_BIN` on Windows (Warn, ADR D3). `std::fs::canonicalize` returns a `\\?\C:\...` verbatim path. Many tools and `cmd.exe` mishandle it. Fix: use the `dunce` approach or skip canonicalising on Windows, and keep `current_exe` as is.
10. Windows lookup (Suggest, ADR D2). Rust's own spawn search (exe dir, system dir, parent PATH) applies to a bare name only. The design walks PATH itself, so pass the found absolute path to `Command` and the `.exe`-only rule holds, with no CVE-2024-24576 surface. Say so in a test: a `grim-x.bat` on PATH must not run.
11. Non-UTF-8 argv (Warn, spec 3.1). The scan and the child argv must stay `OsString`. A lossy conversion corrupts plugin arguments on Unix. Fix: scan with `OsStr`, compare ASCII only.
12. Licence text matcher (Warn, spec 5.6). Word-bigram Dice is the askalono approach (askalono uses a confidence threshold of about 0.8 plus optimisation for partial texts); licensee uses a word-set Dice at a 0.98 confidence with a length-delta check; scancode uses rule-based sequence matching. A 0.93 threshold with a 0.05 margin is plausible. Real gap: GPL-2.0, GPL-3.0 and LGPL texts are identical for `-only` and `-or-later`. The spec assigns `-only`, which can assert a wrong, narrower licence. Fix: map those files to no source unless an SPDX header or a field decides, or record `-only` only when a header says so. Also add Apache-2.0 with the appendix removed and MIT with a title line as fixtures.
13. `spdx` crate (Suggest, ADR D11). `LicenseId::is_osi_approved()` and `Expression::evaluate` exist in the default build (the `text` and `detection` features are opt-in and unrelated). Not verified from the page: deprecated ids. `Expression::parse` is strict and rejects `GPL-3.0` and `GPL-2.0+`; `parse_mode` with `ParseMode::LAX` accepts them. Real frontmatter often carries these. Fix: parse lax for the declared source, then judge the base id.
14. Snapshot version (Warn, ADR D5). Go pseudo-versions use `vX.Y.(Z+1)-0.<UTC ts>-<12 hex commit>`, so ties disambiguate by hash and it sorts below the next release. `<M>.<m>.<ts>` has neither: a later real `M.m.(Z+1)` sorts below it until `M.(m+1).0` (the spec admits this), and a same-second collision refuses. Both follow from strict semver without prerelease, so the choice is sound but the stuck-float cost is real: fix by documenting a "patch bump to above the snapshot, or skip to the next minor" rule in the mirror reference. The 14-digit patch (about 2.0e13) is below `Number.MAX_SAFE_INTEGER` (9.0e15), so the npm `semver` consumers (VS Code extension, indexer) are safe.
15. Base drift (Warn, spec 5.7). The base `M.m` comes from `git tag --merged`, so a tag added later makes the same revision map to a new version. In forced-snapshot mode the next run would republish identical content as `1.3.<ts>`. Fix: before publishing, skip a unit when any existing version carries the same `revision` (report `unchanged`), or freeze the base to the first run's.
16. Debian and conda-forge (Suggest, ADR D5). Both sort a snapshot below the real release by design (`~`, `0.0.0.` prefix). D5 inverts this on purpose (snapshots above the last tag), which is right for floats but unlike those schemes. Say it in the reference.

## Not verified

- Licensee, askalono, scancode thresholds (memory).
- Whether `spdx` default features are empty (docs page silent).
- The last 20k characters of the skills page and 15k of the sub-agents page.
