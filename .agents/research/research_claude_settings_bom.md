# Research: Does Claude Code read a settings file that starts with a UTF-8 BOM?

## Metadata

**Date:** 2026-11-10
**Domain:** cli
**Triggered by:** grimoire issue #306 — `settings_env_config_dir` (`src/install/vendor_claude.rs`) feeds raw text to `serde_json::from_str`, which fails on U+FEFF
**Expires:** 2027-05-10 (re-verify on any Claude Code release that touches settings loading)

## Direct Answer

**Verdict: reads, for all three files.** Claude Code 2.1.321 parses a BOM-prefixed
user `settings.json`, `managed-settings.json` and `managed-settings.d/*.json` and
honours the values. grim must therefore strip a leading U+FEFF before parsing, to match.

Confidence: **high** for the user file (static code + live run); **high-by-shared-code**
for the managed files (same loader, not run live: the paths need root under `/etc/claude-code`).

## Evidence

Version inspected: `@anthropic-ai/claude-code@2.1.321` (npm, `claude-code-linux-x64` native
binary, commit `76bc735070b3`), 2026-11-10. Source is the embedded bun bundle, read with `strings`.

1. **Shared read+parse path.** Managed base file, each drop-in and the user file all go
   through `FY(path, ...)` -> `UT(path, ...)` -> `BT(text, path, isManaged)`:
   - managed: `h=dp(n,be(e),Ce(e,"managed-settings.json"),...)` and, per drop-in,
     `FY(Ce(y,E),n,void 0,!0)` inside the `managed-settings.d` walk;
   - `UT`: `l=aF(c.resolvedPath,fp,s)` ... `return BT(l,e,r)`; a parse failure is surfaced, not silent.
2. **Text read keeps the BOM.** `aF` -> `VVs` does `i.readFileSync(n,{encoding:s})` with
   `HEe` returning `"utf8"` for EF BB BF; Node does not strip it. So `BT` receives a string starting with U+FEFF.
3. **The JSON parse strips it.** `BT` calls `Ze(e,!1)`. `Ze` is `safeParseJSON`:
   `function oe(e,l){try{return{ok:!0,value:JSON.parse(gs(e))}}...}` with
   `function gs(e){return e.startsWith("﻿")?e.slice(1):e}`. The same `gs` guards the JSONC helpers.
4. **Live check (credential-free `claude doctor`)**, scratch install, run as
   `env -i HOME=<scratch> PATH=/usr/bin:/bin CLAUDE_CONFIG_DIR=<scratch dir> DISABLE_TELEMETRY=1 DISABLE_AUTOUPDATER=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`:
   - broken JSON `{"env":{"BOM_PROBE":"1",,}` -> `Invalid settings: .../settings.json: Invalid or malformed JSON`
   - `{"cleanupPeriodDays":"notanumber"}` -> `settings.json › cleanupPeriodDays: Expected number, but received string`
   - same content prefixed with `EF BB BF` (verified with `xxd`) -> the identical `cleanupPeriodDays` message and **no** "malformed JSON".
   The BOM file was parsed and schema-validated, which a rejected file would not be.
   Not run: managed files (need `/etc/claude-code`, root) and a visible `env` effect.
5. **Docs/changelog/issues:** not consulted; the shipped code and the live run settle it.

## Per-file result

| File | Result | Basis |
|---|---|---|
| user `settings.json` | reads | code + live |
| `managed-settings.json` | reads | code (`FY`->`UT`->`BT`->`Ze`) |
| `managed-settings.d/*.json` | reads | code (same `FY` call) |

## What would falsify it

A later release replacing `gs(e)` in `safeParseJSON` / the settings loader, or a live
`claude doctor` on a BOM managed file reporting "Invalid or malformed JSON". Managed
files are the unverified-live part: one run with a writable `/etc/claude-code` would close it.

## Proposed watchlist row

Section: `## Config roots and env vars` in `.agents/upstream-watchlist.md`
(columns: Variable | Vendor | grim behavior | Upstream status).

```
| Settings-file BOM | Claude | `settings_env_config_dir` strips a leading U+FEFF before parsing `env.CLAUDE_CONFIG_DIR` from `managed-settings.json`, `managed-settings.d/*.json` and the user `settings.json` (issue #306) | *verified 2026-11-10 against CLI 2.1.321.* Claude reads a BOM-prefixed settings file and honours it: the loader (`FY`->`UT`->`BT`) parses through `safeParseJSON`, whose `JSON.parse(gs(e))` drops a leading `﻿`; `claude doctor` on a BOM `settings.json` reports the same schema errors as the BOM-less file and no "Invalid or malformed JSON". Managed files share the loader (not run live: need `/etc/claude-code`). No changelog or docs line states it; re-verify if the settings loader changes |
```

## grim's own readers (checked 2026-11-10, issue #347)

Every grim reader of a Claude settings or MCP file reads past one leading U+FEFF, and every
reader that writes back keeps it. The shared paths are `json_splice` (`Scanner::at_start` skips
the mark, `parse_value` strips it; generic test `a_byte_order_mark_is_read_past_and_kept`) and
`json_config::parse_object` (strips it). A production sweep of `serde_json::from_str` and
`from_slice` found no other reader of these files. One test per reader pins the behaviour.

| Reader | Claude file | Parse path | Verdict | Pinned by |
|---|---|---|---|---|
| `vendor_claude::settings_env_config_dir` | `settings.json`, `managed-settings.json`, `managed-settings.d/*.json` | `serde_json` after `strip_prefix('\u{feff}')` | reads past the BOM (read-only) | `bom_prefixed_user_settings_still_relocate_the_root`, `bom_prefixed_managed_settings_still_relocate_the_root` |
| `managed_config::sync_managed_element` (`claudeMdExcludes`) | `settings.json` | `json_splice` upsert/remove array element | reads past the BOM and keeps it | `a_bom_prefixed_settings_file_keeps_its_bom_when_claude_md_excludes_is_registered`, `..._is_removed`; acceptance `test_global_claude_md_excludes_registers_into_a_bom_prefixed_settings_file` |
| `hook_registrar` converge and status paths | `settings.local.json`, `settings.json` | `json_splice` | reads past the BOM and keeps it | `a_bom_prefixed_registration_file_is_read_registered_into_and_keeps_its_bom`, `a_bom_prefixed_registration_file_keeps_its_bom_when_the_registration_is_removed` |
| `settings_writer` (`reread`, `Splices`) | `settings.json`, `settings.local.json` | `json_splice::value_at`, `upsert_member_at` | reads past the BOM and keeps it | `a_bom_prefixed_settings_file_is_judged_written_and_keeps_its_bom` |
| `install_state::read_entry_value` | `.mcp.json`, `.claude.json`, settings values | `json_config::parse_object`, `json_splice` | reads past the BOM (read-only) | `a_bom_prefixed_claude_file_is_read_for_an_mcp_entry_and_a_settings_value` |
| `uninstall::remove_entry` | `.mcp.json`, `.claude.json` | `json_splice::remove_member` | reads past the BOM and keeps it | `uninstall_removes_a_member_from_a_bom_prefixed_file_and_keeps_the_bom` |
| MCP install write (`installer::install_mcp`) | `.mcp.json`, `.claude.json` | `json_splice::member_value`, `upsert_member` | reads past the BOM and keeps it | `an_mcp_entry_written_into_a_bom_prefixed_claude_file_keeps_its_bom` |

Break check: with `Scanner::at_start` not skipping the mark, seven of the new tests fail; with
`parse_object` not stripping it, the `install_state` test fails.
