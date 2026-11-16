# Research: upstream refresh — catalog (sweep)

## Metadata
**Date:** 2026-11-16 · **Domain:** catalog · **Depth:** sweep — checked 50d ago
**Triggered by:** /upstream-refresh
**Expires:** 2027-05-16

## Claims

| # | Claim | Ledger location | Old value | New value | Source URL | Verbatim quote | Vendor version |
|---|---|---|---|---|---|---|---|
| 1 | latest real release tag | `.agents/upstream-checks.md` catalog row | `v0.14.2` | `v0.15.1` (2026-10-28) | `gh release list --repo grimoire-rs/grimoire` | `0.15.1 - 2026-10-28	Latest	v0.15.1` | v0.15.1 |
| 2 | `grim-usage` compatibility minor equals release minor | `catalog/skills/grim-usage/SKILL.md:5` | `grim>=0.14` matches | mismatch: `compatibility: grim>=0.16`, release minor is 15 | `gh release list --repo grimoire-rs/grimoire` | `compatibility: grim>=0.16` | v0.15.1 |
| 3 | `grim-authoring` compatibility minor equals release minor | `catalog/skills/grim-authoring/SKILL.md:5` | `grim>=0.14` matches | mismatch: `compatibility: grim>=0.16`, release minor is 15 | `gh release list --repo grimoire-rs/grimoire` | `compatibility: grim>=0.16` | v0.15.1 |
| 4 | `ai-config-authoring` has no `compatibility:` line | `catalog/skills/ai-config-authoring/SKILL.md` | skipped per domains.md | unchanged | n/a | n/a | v0.15.1 |
| 5 | `grim-usage/references/updating.md` checklist omits `rate` (2026-09-27 draft) | `catalog/skills/grim-usage/references/updating.md:10` | omits `rate` | `rate` is listed (`updating.md:10`); resolved | same source | n/a | v0.15.1 |

## Verifier sign-off

none required — no code landed in this run.

## Routing

- #1 latest real release tag → (a) ledger or doc text fix
- #2 grim-usage compatibility minor ahead of release → (c) issue draft
- #3 grim-authoring compatibility minor ahead of release → (c) issue draft
- #5 updating.md checklist omits rate, now resolved → (a) ledger or doc text fix

## Issue drafts

### Catalog skills declare grim>=0.16 while the latest release is v0.15.1

The `compatibility:` lines at `catalog/skills/grim-usage/SKILL.md:5` and `catalog/skills/grim-authoring/SKILL.md:5` say `grim>=0.16`, while the latest release is v0.15.1 (`gh release list --repo grimoire-rs/grimoire`). The skills moved ahead in commit `b54061e1` (declared dependencies) and `Cargo.toml` is still `0.15.1`. It resolves itself when 0.16 ships and matters only if the catalog is published first.

Labels: bug

## Friction

none

## Skill changes

Host caps concurrent subagents at 20; the researcher built its own claim list; replies are a short summary with the table in a file. Fixed in this run's chore(skills) commit.
