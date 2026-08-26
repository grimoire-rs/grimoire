# ADR: Artifact Trust Model (what grim defends against, and what it does not)

## Metadata

**Status:** Accepted
**Date:** 2026-08-26
**Deciders:** Michael Herwig + Claude (review session on MCP-as-bundle-member)
**Beads Issue:** N/A
**Related PRD:** N/A
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md` (no new dependency; documents existing behavior)
**Domain Tags:** security
**Supersedes:** N/A
**Superseded By:** N/A

## Context

grim installs third-party configuration into the user's agent clients.
Every feature that widens what an artifact can reach re-opens the same
unresolved argument, because the project has never written its trust
boundary down. The concrete instance that forced this ADR: *may an MCP
server descriptor be a bundle member?* The objection raised was "an MCP
descriptor makes the client spawn a process, so it needs a consent gate
and must not arrive transitively through a bundle."

That objection assumes MCP is a distinct trust class. It is not:

- A **skill** is a directory. It may ship scripts, and its `SKILL.md` is
  read by an agent that will run commands on the user's behalf. Installed
  with no gate today.
- An **MCP descriptor** names a `command` + `args` the client spawns, or a
  URL it connects to. Installed with no gate today.

Both reach the same place. Both arrive by the same gesture. The installer
already treats them as siblings — the untracked-clobber gate for
materialized files and the one for MCP config entries are the same shape,
declared as siblings in `src/install/installer.rs` — so the asymmetry
exists only in review discussion, never in the code.

Without a written boundary, each reviewer re-derives one, and the answer
depends on who reviews. This ADR fixes it.

## Decision

**1. The trust decision happens once, at the declaration gesture.**
`grim add <ref>`, an edit to `grimoire.toml`, or adding a bundle *is* the
user's statement of trust. Everything that gesture transitively pulls in
inherits it. Transitivity is the bundle feature, not a loophole in it.

**2. All artifact kinds are equally trusted.** Skill, rule, agent, MCP
descriptor, bundle — no per-kind consent prompt, no per-kind exclusion
from bundles, no "this kind is more dangerous" carve-out. A gate on one
kind while the others reach equally far is theatre: it trains
click-through and stops nothing.

**3. Until signatures exist, an artifact is treated as if signed by a
trusted authority.** The trust anchors grim actually has are registry
authentication and digest pinning: the lock pins a manifest digest, and a
pinned artifact is byte-identical or the install fails. That is an
*integrity* guarantee — the content has not changed since it was pinned —
not an *authenticity* one.

**4. Integrity, not consent, is grim's mechanism.** Where grim spends
effort: digest pinning, `deny_unknown_fields` (never silently drop a field
it cannot represent), content-hash-verified untracked-clobber gates, and
the rule that no artifact may carry a literal credential (`${VAR}`
references only; the descriptor's OAuth block deliberately has no
`client_secret` field).

### In scope — grim must defend against these

| Threat | Mechanism |
|---|---|
| Clobbering a user-authored file or config entry | Untracked gate: content-hash match required, refuse otherwise, `--force` is the only override; identical content is adopted |
| Writing outside the intended root (traversal, symlink escape) | `src/path_safety.rs`, `AnchoredPath` containment |
| Resource exhaustion from hostile or corrupt registry data | Layer size caps per kind, `MAX_BUNDLE_MEMBERS`, bounded fetch |
| A credential reaching the wrong host | Per-registry credential scoping, exact-match `GRIM_INSECURE_REGISTRIES`, separate ladders for the announce and rating tokens, no plaintext-realm downgrade from an HTTPS registry |
| Silent misrepresentation of newer data by an older binary | Hard-reject (`deny_unknown_fields`), documented in `docs/src/stability.md` |
| Registering a connection a client cannot authenticate | Per-vendor decline with a warning, never a half-working entry |

### Out of scope — not grim's concern at this stage

- Whether the artifact's author is who they claim to be.
- Whether trusted-by-the-user content is malicious or merely bad.
- Compromise of a registry account or of the publisher's toolchain.
- Anything the *harness* owns: which tools an agent may run, sandboxing,
  approval prompts at execution time. grim installs config; it is not the
  execution policy layer.

### When signatures arrive

Verification becomes a gate at fetch/resolve time, applied uniformly to
every kind, with a policy setting for how strict it is. It does **not**
become a per-kind consent prompt. Revisit this ADR then; until then the
above holds.

## Consequences

- **MCP descriptors as bundle members are unblocked on trust grounds.**
  Remaining questions there are contract and reporting questions
  (forward compatibility of the member-kind enum, aggregate reporting of
  per-client declines), not trust ones.
- **Reviewers:** "kind X is executable, therefore gate it" is not a
  finding — cite this ADR and move on. Findings that remain valid:
  clobbering user content, containment escape, unbounded resource use,
  a credential reaching an unintended destination, or a bypass of the
  integrity checks above.
- The absence of a signature story is a **known, accepted** gap, not an
  oversight to be re-raised per feature. It is tracked as its own work.
