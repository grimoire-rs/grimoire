---
paths:
  - "src/install/vendor_*.rs"
  - "src/oci/mcp.rs"
  - "src/catalog/rating_provider.rs"
  - "docs/src/content/docs/ratings.md"
  - "src/install/settings_phase.rs"
  - "docs/src/content/docs/settings.md"
---

# Vendor Capability Watchlist

Auto-fires on vendor renderer / MCP descriptor edits, on the two files
carrying the ratings feature's own upstream forge claims, and on the
`settings` kind's phase and reference page. Purpose:
**re-check upstream before patching a decline** — or before touching a
vendor-version claim. Every skip/warn/decline in a renderer, and every
dated row in the "Ratings forge capability watchlist" of [the data file](../../.agents/upstream-watchlist.md), encodes an
upstream fact verified at a point in time — vendors ship features and
change behavior continuously, and either can silently rot into a grim
regression or a stale doc (it happened: `xhigh` reasoning-effort, Codex
`additionalContext`).

The dated rows live in [`.agents/upstream-watchlist.md`](../../.agents/upstream-watchlist.md),
outside `.claude/rules/`: Claude Code treats every `.md` there as a rule, so a
data file there without `paths:` would load in every session, and rows kept in
this file would load on every edit its globs match.

## Re-verify procedure

1. Before changing any decline/skip/warn in `src/install/vendor_*.rs` or
   validation in `src/oci/mcp.rs`, check the watchlist row in `.agents/upstream-watchlist.md` and its
   upstream doc link. Row stale (> ~6 months since `verified` date) →
   re-verify upstream first, with `/upstream-refresh <harness>`.
2. Upstream shipped the capability → patch renderer + docs
   (`docs/src/content/docs/vendor-metadata.md` / `docs/src/content/docs/mcp-servers.md`) + tests in
   **one commit** (parity tests require doc row and registry change
   together), then move/update the row there in the same commit.
3. Compatibility doctrine applies (AGENTS.md principle 9): additive-only,
   never remove accepted literals, layout moves ship migration + reaper.
4. Upstream did **not** ship it and someone wants grim to close the gap
   anyway → classify the request first (next section). The class decides
   whether it is in scope at all; the tier decides whether it is in scope
   for that client.

## Compensation classes and support tiers

`KindSupport` says what a client can host. This says how hard grim works to
close a gap it cannot. Decision recorded in
[adr_vendor_support_tiers.md](../../.agents/adr/adr_vendor_support_tiers.md);
the consumer-facing half is `docs/src/content/docs/clients.md#compensation`.

> **grim renders artifacts. grim does not run at the client's runtime.**

| Class | What it is | Policy |
|---|---|---|
| **1 — repair** | grim making its **own** output behave as authored | Always in scope, every client, every tier. Not a feature, sets no precedent. |
| **2 — compensating render** | A static artifact the vendor already documents and interprets: a config key, an extra generated file, adjusted body text. Deterministic, removed on uninstall, inert if grim vanishes. | Tier 1 only. |
| **3 — runtime code** | Anything grim must keep alive against someone else's API: plugin, extension, wrapper process, injected script. | **Never.** Any client, any tier. |

Class 3 is refused on maintenance shape, not difficulty. The test: *can a
client release break grim's contribution without grim being touched, and
does it break silently?* A plugin fails both.

Class 1 reads like scope creep and is not. grim wrote the files, so grim
owns their behavior — see grimoire-rs/grimoire#102, where grim's own
support-directory copy auto-loads as unconditional Claude context.

**Tier 1 (compensating render in scope): `claude`, `codex`, `opencode`,
`copilot`.** Every other client is Tier 2 — faithful render, gaps declined
or warned per `KindSupport`, never compensated.

Tier 1 costs a `docs/src/content/docs/clients.md` matrix row, a dated row in `.agents/upstream-watchlist.md`, and
re-verification when the vendor moves; a client is promoted when someone
commits to paying that. Tier is a statement about maintainer capacity, not
about the client — Cursor and Kiro are Tier 2 with `Native` scoped rules,
fuller support than Tier 1 OpenCode.

**Carve-out:** a class-2 render derived solely from the artifact's own
frontmatter — no upstream key, surface or version behind it — is not
tier-gated, because there is nothing to re-verify and so nothing the tier
buys (`render::scope_notice`, the dropped-`paths:` prose, is the case).
A class-2 render that *tracks* a vendor surface still is.

**Adding a vendor?** Assign its tier here in the same commit as its matrix
row. **Adding a grimoire frontmatter field to express intent?** Only when
two artifacts of the same kind want different answers *and* the author is
the one who knows which — a field whose value never varies only rots (ADR
§4).

## Fragility note

Overlap detection in `test_path_overlaps_declared_or_absent` compares
`paths:` patterns as **exact strings**. This rule's globs
(`src/install/vendor_*.rs`, `src/oci/mcp.rs`, `src/catalog/rating_provider.rs`,
`docs/src/content/docs/ratings.md`, `src/install/settings_phase.rs`,
`docs/src/content/docs/settings.md`) are unique strings today except
`src/install/vendor_*.rs`, which `subsystem-vendor-paths.md` shares through a
declared group in `.claude/rules.md`. They also *semantically* overlap
`src/**`/`**/*.rs` (the `src/` entries) and `docs/**` (the two docs pages,
which `docs-style.md` and `product-context.md` already glob-match — editing
either file loads all three rules). If another rule ever adopts one of the
other exact strings, a declared group in `.claude/rules.md` becomes mandatory.

