# ADR: MCP oauth projection is lossless or skipped

## Metadata

**Status:** Accepted
**Date:** 2026-09-27
**Deciders:** maintainer (owner decision 2026-09-27)
**Beads Issue:** N/A (GitHub tracking: [grimoire-rs/grimoire#144](https://github.com/grimoire-rs/grimoire/issues/144), [grimoire-rs/grimoire#145](https://github.com/grimoire-rs/grimoire/issues/145), [grimoire-rs/grimoire#152](https://github.com/grimoire-rs/grimoire/issues/152), [grimoire-rs/grimoire#156](https://github.com/grimoire-rs/grimoire/issues/156))
**Related PRD:** N/A
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md` (no new tech; a projection rule and one helper)
**Domain Tags:** security | integration
**Supersedes:** N/A
**Superseded By:** N/A

## Context

An MCP descriptor may carry a structured `[server.oauth]` block
(`McpOAuth` in `src/oci/mcp.rs`): `client_id`, `scopes`, `callback_port`,
`auth_server_metadata_url`. Claude maps all four. Every other client skipped
the whole server with a warning, because no other client had an oauth shape
grim could carry.

The 2026-09-27 upstream sweep found that several clients now ship a native
oauth object, each covering a different subset of grim's four fields. A
vendor that maps only part of the block has two choices: write what it can
and drop the rest, or keep skipping. That choice has to be made once, for
every vendor, rather than per renderer.

## Decision Drivers

- **Auth surface.** The fields grim would drop are the ones that limit a
  grant. Dropping `scopes` lets the client request its default scope set,
  which may be wider than what the author asked for. Dropping
  `auth_server_metadata_url` lets the client discover an authorization
  server on its own instead of the one the author pinned.
- **Honest render.** grim's rule is that a vendor render either behaves as
  authored or is declined with a reason. A half-written oauth block behaves
  differently from the authored one, and nothing in the output shows it.
- **One rule, many vendors.** Seven clients are affected. A shared helper
  keeps their decisions consistent and makes the per-vendor code a single
  list of supported fields.

## Industry Context & Research

**Research artifact:** `.agents/research/research_upstream_<vendor>_20260927.md`
(Copilot, OpenCode, Zed, Droid, Cline) and the watchlist rows in
`.claude/rules/vendor-capability-watchlist.md`.
**Key insight:** no two clients agree on the oauth field set, and none
besides Claude maps `auth_server_metadata_url`.

## Considered Options

### Option 1: Lossless-or-skip

**Description:** A vendor writes the oauth block only when every field the
descriptor sets has a native target. Otherwise it skips the server and the
warning names the unmapped fields.

| Pros | Cons |
|------|------|
| A written block always behaves as authored | A server whose descriptor sets one unmappable field stays unavailable for that client |
| No silent grant widening | Authors must drop a field to reach a narrower client |
| One helper, one rule | |

### Option 2: Best-effort projection

**Description:** Write every field that maps, drop the rest with a warning.

| Pros | Cons |
|------|------|
| More servers reach more clients | A dropped `scopes` or metadata URL widens what the client may be granted |
| | The warning is on stderr at install time; the resulting config shows nothing |

### Option 3: Keep skipping every non-Claude oauth server

**Description:** No change.

| Pros | Cons |
|------|------|
| Zero risk | Ignores native surfaces that map the descriptor exactly (for example Zed with `client_id` only) |

## Decision Outcome

**Chosen Option:** Option 1, lossless-or-skip.

**Rationale:** it is the only option under which a written oauth block is
always the authored one. Option 2 trades correctness on an auth surface for
coverage, which is the wrong direction for a credential flow.

### Mechanism

`McpOAuth::unmapped(&self, supported: &[OAuthField]) -> Vec<&'static str>`
returns the descriptor field names the block sets (`Some`, or a non-empty
`scopes`) that `supported` does not list, in declaration order. A vendor's
`mcp_entry` calls it with its own supported set:

- empty result: project the block into the vendor's native shape;
- non-empty result: return `None` and keep today's skip warning, naming the
  returned fields.

An oauth block that sets no fields at all projects nothing and is never a
reason to skip.

### Per-vendor mapping

| Client | Supported fields | Native shape | Notes |
|---|---|---|---|
| Claude | `client_id`, `scopes`, `callback_port`, `auth_server_metadata_url` | unchanged | Full mapping, already shipped |
| OpenCode | `client_id`, `scopes`, `callback_port` | `oauth.clientId`, `oauth.scope` (space-joined), `oauth.callbackPort` | Remote entries only; a set `auth_server_metadata_url` skips. `callback_port = 0` also skips: OpenCode's schema takes 1..=65535 and rejects its whole config on anything else |
| Copilot | `client_id` | `oauthClientId` | `callback_port` counts as unmapped: `auth.redirectPort` appears only in the changelog, never in the reference |
| Zed | `client_id` | `oauth.client_id` | HTTP `context_servers` only. A `${VAR}` client id still skips, under Zed's existing env-ref skip (Zed expands none) |
| Droid | `client_id` | `oauth.clientId` | `${VAR}` allowed in `oauth.clientId` |
| Cline | none | — | Always skip: Cline's oauth shape is unverified |
| Codex, Kiro, Cursor, Qoder, Amp, Antigravity, Gemini, Warp | unchanged | — | Out of scope for this decision; each keeps its current skip until its own row is decided |

### Consequences

**Positive:**
- Servers whose descriptor matches a client's native fields now install for
  that client instead of being skipped.
- A dropped scope or metadata URL can no longer widen a grant.

**Negative:**
- A descriptor that sets `scopes` still skips Copilot, Zed and Droid. The
  author has to publish a variant without it to reach them.

**Risks:**
- **Newly written entries meet hand-authored ones.** A server that grim used
  to skip is now written. A user who added a same-named entry to that
  client's config by hand hits exit 65 (`UntrackedDestination`) until they
  remove it or re-run with `--force`. Each work package that enables a
  vendor adds this to `docs/src/content/docs/upgrading.md`.
- **Upstream drift.** A client that later documents a new field makes a
  skip unnecessary but never makes a write wrong. Re-verify against the
  watchlist row before extending a supported set.

## Technical Details

### API Contract

```rust
// src/oci/mcp.rs
pub enum OAuthField { ClientId, Scopes, CallbackPort, AuthServerMetadataUrl }

impl McpOAuth {
    pub fn unmapped(&self, supported: &[OAuthField]) -> Vec<&'static str>;
}
```

The returned names are the descriptor's own field names, so the skip
warning reads in the author's vocabulary.

## Implementation Plan

1. [x] Add `OAuthField` and `McpOAuth::unmapped` with table-driven unit tests
   (WP0, `plan_upstream_followups_2.md`).
2. [x] OpenCode and Zed projections (WP G).
3. [ ] Copilot projection (WP F).
4. [x] Droid projection (WP C).
5. [ ] Cline always skips (WP E).
6. [ ] `upgrading.md` note per enabled vendor.

## Validation

- [x] Unit tests for `unmapped` cover the full, partial, empty and unset cases.
- [ ] Acceptance matrix per vendor: written vs skipped.
- [ ] Security review of each projection (reviewer perspective in the WP gate).

## Links

- [adr_vendor_support_tiers.md](./adr_vendor_support_tiers.md)
- [adr_artifact_trust_model.md](./adr_artifact_trust_model.md)
- [plan_upstream_followups_2.md](../plans/plan_upstream_followups_2.md)

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-09-27 | maintainer | Initial draft, owner decision recorded |
| 2026-09-27 | WP G | OpenCode and Zed projections landed; recorded the OpenCode port-0 skip and Zed's env-ref interaction |
