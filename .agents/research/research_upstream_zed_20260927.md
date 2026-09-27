# Research: upstream refresh — zed (sweep)

## Metadata
**Date:** 2026-09-27 · **Domain:** vendors · **Depth:** `zed sweep never checked`
**Triggered by:** /upstream-refresh --domain vendors (WP-G, plan_harness_capability_freshness)
**Expires:** 2027-03-27

Ladder output for the whole `--domain vendors` run (the four Tier 1 rows
were deep-checked earlier today, so `noop`; the 14 Tier 2 rows `sweep`):

```text
cursor sweep never checked
kiro sweep never checked
junie sweep never checked
gemini sweep never checked
zed sweep never checked
amp sweep never checked
antigravity sweep never checked
cline sweep never checked
droid sweep never checked
goose sweep never checked
warp sweep never checked
openclaw sweep never checked
kilo sweep never checked
qoder sweep never checked
claude noop checked today
opencode noop checked today
copilot noop checked today
codex noop checked today
```

Vendor version at check: Zed **v1.21.0** (2026-09-23). Feed: GH
`zed-industries/zed` releases, `-pre` skipped; cursor was `—`, **v1.21.0**
recorded. Claims were checked against source at the `v1.21.0` tag.

## Claims

Researcher (sonnet) table, verbatim. Numbered rows re-verify ledger claims;
`I` rows are changelog or surface differences found in the sweep.

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | Rules declined: 9-file instruction precedence (`.rules` first … `AGENTS.md` 7th), no scoping | `.claude/rules/vendor-capability-watchlist.md:139` | declined; 9-file precedence, no scoping | unchanged | https://github.com/zed-industries/zed/blob/33c95853ed2b6956f339733c63a8220964ecbeb6/crates/prompt_store/src/prompts.rs#L22-L32 | `pub const RULES_FILE_NAMES: &[&str] = &[".rules", ".cursorrules", ".windsurfrules", ".clinerules", ".github/copilot-instructions.md", "AGENT.md", "AGENTS.md", "CLAUDE.md", "GEMINI.md"];` | v1.21.0 |
| 2 | MCP env refs: ref-bearing descriptors skipped; env-ref / keychain support tracked in #56881 | `.claude/rules/vendor-capability-watchlist.md:140` | skipped; tracked (#56881) | unchanged — discussion #56881 ("Allow context_servers settings to reference secrets stored in the system keychain") is still open/unanswered, no shipping PR | https://github.com/zed-industries/zed/discussions/56881 | "Add an indirection syntax to context\_servers.\*.settings: any string-typed leaf can be replaced by a one-key object of the form { \"$keychain\": \"<name>\" }…which Zed resolves at server-start time by reading zed://context_servers/<name> from the system keychain" | n/a (GitHub Discussion, unversioned; created 2026-05-15, still open 2026-09-27) |
| 3 | macOS `$XDG_CONFIG_HOME` no longer honored — `Xdg` (Linux/FreeBSD) / `HomeDotConfig` (macOS) / `Appdata` (Windows) split | `.claude/rules/vendor-capability-watchlist.md:143` | as stated | unchanged | https://github.com/zed-industries/zed/blob/33c95853ed2b6956f339733c63a8220964ecbeb6/crates/paths/src/paths.rs#L122-L140 | `} else if cfg!(any(target_os = "linux", target_os = "freebsd")) { ... dirs::config_dir()... } else { home_dir().join(".config").join(APP_NAME_LOWERCASE) }` | v1.21.0 |
| 4 | `//!` module doc + `mcp_entry`: no `${VAR}` substitution anywhere in `context_servers` settings (env or headers) → ref-bearing descriptors skipped | `src/install/vendor_zed.rs:17-18`, `:108-116` | no env-ref support upstream | unchanged for the general case, but the gap is now split: `env` (stdio) and `headers` (HTTP) still get no expansion, and the specific PR to add it to headers was closed unmerged | https://github.com/zed-industries/zed/pull/60457 | "Thanks for the PR! I think for this kind of feature we'll want to take a different approach that is more integrated with the zed experience, such as settings, etc. So I'm going to close this for now." | as of v1.21.0 (PR closed 2026-07-11) |
| 5 | `mcp_entry` code comment: "Zed's `context_servers` schema has no OAuth surface — a structured oauth block is auth-critical, so the whole descriptor is skipped with a warning" | `src/install/vendor_zed.rs:96-99` (code comment, not a watchlist row) | no OAuth surface in `context_servers` | **wrong, and predates grim's own 2026-07-19 verification stamp.** Zed's HTTP `context_servers` settings carry a real, documented `oauth: OAuthClientSettings { client_id, client_secret }` field for pre-registered OAuth clients, backed by a full dynamic-discovery / browser-flow / keychain-cached-session implementation. Shipped 2026-05-19 via #52900 | https://github.com/zed-industries/zed/blob/33c95853ed2b6956f339733c63a8220964ecbeb6/crates/settings_content/src/project.rs#L503-L514 | "Pre-registered OAuth client credentials for MCP servers that don't support Dynamic Client Registration. ... pub client_id: String, ... pub client_secret: Option<String>" | v1.21.0 (field introduced 2026-05-19) |

Unsourced:
- none — every claim in scope was backed by a primary source (Zed source at the `v1.21.0` tag commit, or a live GitHub Discussion/PR).

Feed notes:
- Newest tag or heading seen: `v1.21.0` (published 2026-09-23T15:42:38Z; `v1.22.0-pre` exists but is a `-pre` release, skipped per feed instructions). Cursor found: n/a — cursor was `—` (never checked before this sweep).
- Vendor version current today: `v1.21.0`, https://github.com/zed-industries/zed/releases/tag/v1.21.0.
- Anything grim renders or documents that the feed shows changed but no ledger row covers: Zed's `context_servers` HTTP entries now support a real `oauth: {client_id, client_secret}` block (pre-registered OAuth client credentials, shipped 2026-05-19 via https://github.com/zed-industries/zed/pull/52900, still present at v1.21.0) — grim's `vendor_zed.rs::mcp_entry` unconditionally skips any descriptor with `oauth` set, on a doc claim ("no OAuth surface") that was already false when it was written. No watchlist row exists for this at all (row 140 covers only `${VAR}` env-ref support, a separate gap). This needs its own watchlist row and likely a `src/install/vendor_zed.rs` code/comment fix — flagging for triage, not fixed here (read-only task).

## Verifier sign-off

One opus verifier (C-008) re-fetched each claim that drives a docs, comment,
watchlist-wording or issue-draft change. No claim in this pass drives a
renderer, metadata or validation change unless Routing says so.

| Claim | Verdict | Evidence URL | Verbatim quote / note |
|---|---|---|---|
| V6 zed oauth | CONFIRMED | https://github.com/zed-industries/zed/blob/v1.21.0/crates/settings_content/src/project.rs (L446-460, L503-515); https://github.com/zed-industries/zed/pull/52900 | The `Http` variant has `oauth: Option<OAuthClientSettings>`, and `pub struct OAuthClientSettings { pub client_id: String, … pub client_secret: Option<String> }`. PR #52900 ("Implement MCP OAuth client preregistration", merged 2026-05-19) adds both lines in this file, and v1.21.0 is ahead of its merge commit. |

## Live CLI check

n/a — `sweep` depth on a Tier 2 harness (the live check is `deep` only).

## Routing

| Claim | Class | Landed as |
|---|---|---|
| 5 / V6 `context_servers` `oauth` | (a) + (c) | `vendor_zed.rs` `mcp_entry` comment corrected (the old "no OAuth surface" was already wrong when written); new watchlist row; issue draft 1 |
| 4 env refs | (a) | watchlist row: headers-expansion PR [zed #60457](https://github.com/zed-industries/zed/pull/60457) closed unmerged |
| 1, 2, 3 | (a) | rows dated; stamp dated |

Security: none found.

## Issue drafts

Filed as https://github.com/grimoire-rs/grimoire/issues/156.

### 1. Zed: project `[server.oauth]` onto `context_servers` `oauth`?

**Labels:** `enhancement`, `vendor:zed`, `needs-design`

Zed's HTTP `context_servers` accept `oauth: { client_id, client_secret }` for
pre-registered OAuth clients (added by
[zed #52900](https://github.com/zed-industries/zed/pull/52900), 2026-05-19;
[project.rs @ v1.21.0](https://github.com/zed-industries/zed/blob/v1.21.0/crates/settings_content/src/project.rs)).
Otherwise Zed runs dynamic discovery and a browser flow itself. grim skips
every descriptor with `[server.oauth]` for Zed. Only `client_id` has a
target; `scopes`, `callback_port` and `auth_server_metadata_url` have none.
Decide whether a lossy projection (`client_id` only, warn on the rest) is
acceptable for an auth-critical block, or whether the skip stays. A change
turns a skip into an install for existing descriptors, so it needs an
upgrade note.

## Friction

- Researcher sandboxes denied network `curl` (and, for some, `gh`); the brief's "`curl -sL <url>` is fine" tip was wrong there. Researchers fell back to WebFetch, whose text is model-extracted, so quotes from vendor sites are summarizer-sourced; GitHub sources were read raw via `gh api` where allowed. Brief fixed.
- Driver cost: the brief told researchers to return only the table, so a 14-target run would have re-typed every table. This run gave each researcher one scratchpad output file and assembled the artifacts from it. Brief now names that file.

## Skill changes

One `chore(skills):` commit for the whole WP-G run, [f9effcc1](https://github.com/grimoire-rs/grimoire/commit/f9effcc1): researcher-brief scratchpad output file, no-curl fallback and re-fetch warning; domains.md docs and feed URLs (kiro, junie, amp, antigravity, goose, openclaw, qoder), `JUNIE_HOME` / `OPENCLAW_HOME` cells, section-default and stamp re-dating note; SKILL.md one artifact per target in a `--domain` run.
