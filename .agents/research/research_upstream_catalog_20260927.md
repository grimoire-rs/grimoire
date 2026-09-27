# Research: upstream refresh — catalog (sweep)

## Metadata
**Date:** 2026-09-27 · **Domain:** catalog · **Depth:** sweep (never checked)
**Triggered by:** /upstream-refresh --domain catalog
**Expires:** 2027-03-27

## Claims

| # | Claim | Source | Verdict |
|---|---|---|---|
| 1 | Latest real grim release tag is `v0.14.2` (minor `0.14`); `v99.0.0` excluded as a trial tag | `git tag --list 'v*' --sort=-v:refname`; `gh release list --repo grimoire-rs/grimoire --limit 5` (both agree, `0.14.2 - 2026-09-13` latest) | confirmed |
| 2 | `catalog/skills/grim-authoring/SKILL.md` line 5: `compatibility: grim>=0.14` | file read | matches release minor — no drift |
| 3 | `catalog/skills/grim-usage/SKILL.md` line 5: `compatibility: grim>=0.14` | file read | matches release minor — no drift |
| 4 | `catalog/skills/ai-config-authoring/SKILL.md` has no `compatibility:` line | `grep -rn '^compatibility:' catalog/` | confirmed — skip per domains.md (known state, unchanged from prior sweeps) |
| 5 | `catalog/skills/grim-usage/references/updating.md` step 1 command re-verification list omits `rate`, which `grim-usage/SKILL.md` documents (line 75: `grim rate` — vote via the index's rating forge) and which exists as a real subcommand (`src/command/rate.rs`) | file reads: `updating.md` lines 8–13, `SKILL.md` line 75, `src/command/rate.rs` present | stale — the checklist under-covers the command surface it's meant to re-verify each release |
| 6 | `catalog/skills/grim-authoring/references/updating.md` and `catalog/skills/grim-usage/references/updating.md` canonical links (`grimoire.rs/*.html`) resolve to still-existing source pages under `docs/src/content/docs/` (commands.md, concepts.md, configuration.md, publishing.md, authentication.md, json-interface.md, artifacts.md, vendor-metadata.md, agents.md, mcp-servers.md, clients.md, package-index.md) and `src/skill/`, `src/install/` (with `vendor_claude.rs` and siblings) still exist as claimed | `find docs/src/content/docs`, `find src/skill src/install` | confirmed — no drift |

## Verifier sign-off

None required — confirm-only domain (domains.md § 2), no renderer, metadata,
or validation change proposed. No code-driving claim in this pass.

## Live CLI check

n/a — confirm-only domain, no `deep` rung.

## Routing

| Claim | Class | Landing |
|---|---|---|
| #1–4, #6 | (a) | Ledger row update only (no mismatch, no edit needed under `catalog/**`) |
| #5 | (c) | Issue draft below — `catalog/**` is a publish surface, not edited in this pass |

## Issue drafts

Filed as https://github.com/grimoire-rs/grimoire/issues/140.

1. **`catalog/skills/grim-usage/references/updating.md`: refresh-protocol command list omits `rate`**
   - Body: Step 1 of the Re-Verification Protocol lists the commands to diff
     against `--help` output on every grim release: `init, config, add, lock,
     install, update, status, context, fetch, describe, remove, uninstall,
     search, schema, completions, tui, mcp, build, release, publish, login,
     logout`. `grim rate` (documented in `SKILL.md` line 75, backed by
     `src/command/rate.rs`) is missing from that list, so a maintainer
     following the checklist verbatim skips re-verifying `grim rate --help`
     on every minor release. Add `rate` to the step-1 list.
   - Labels: `catalog`, `docs`, `chore`
   - Found by: `research_upstream_catalog_20260927.md` (this pass)

## Friction

None — the confirm-only check ran cleanly against `domains.md § 2` as
written; no researcher spawned (per WP-H, catalog neither re-researches nor
spawns a vendor researcher).

## Skill changes

none
