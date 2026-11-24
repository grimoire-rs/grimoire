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
**Superseded By:** N/A (decision 2 amended 2026-11-19 by [`adr_mod_artifact_kind.md`](./adr_mod_artifact_kind.md) D15 — one owner-authorised refusal on a shipped kind, see the AMENDED line after decision 2; decision 2 amended 2026-08-28 by [`adr_hook_workspace_consent.md`](./adr_hook_workspace_consent.md) — one scoped exception — and 2026-11-04 by [`adr_settings_artifact_kind.md`](./adr_settings_artifact_kind.md) — a second scoped exception — and 2026-11-19 by [`adr_mod_artifact_kind.md`](./adr_mod_artifact_kind.md) — a third, the mods; see the boxes under Decision. A narrowing of decision 1 for hook and settings consent is proposed, awaiting the owner; see the block under decision 1)

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

> **Proposed — the trust narrowing, goal-loop decisions RD-3 (as corrected in
> RF2), RD3-6 and, for settings, RB-4 of
> [`plan_lock_pin_stability.md`](../plans/plan_lock_pin_stability.md), awaiting
> the owner.** For hook and settings consent the inheritance is
> narrowed, as the proposed amendment after
> [`adr_hook_workspace_consent.md`](./adr_hook_workspace_consent.md) decision 4
> records. A hook or settings package already in the workspace inherits
> nothing. Neither does one behind an edge this run did not resolve or (offline)
> its store does not list, nor a kept pin whose tag moved. `grim hook allow`
> answers them. The shipped code follows this narrowing (CWE-863, CWE-345).
> Until the owner ratifies it, decision 1 above is the owner's text.

**2. All artifact kinds are equally trusted.** Skill, rule, agent, MCP
descriptor, bundle — no per-kind consent prompt, no per-kind exclusion
from bundles, no "this kind is more dangerous" carve-out. A gate on one
kind while the others reach equally far is theatre: it trains
click-through and stops nothing.

> **AMENDED (2026-11-19): one owner-authorised exception to equal trust.** A
> shipped kind refuses content that reaches client-loaded code, at build and
> publish and at install ([`adr_mod_artifact_kind.md`](./adr_mod_artifact_kind.md)
> D15). It is a Principle 9 exception, as `adr_rule_support_dir_removal.md` was;
> client-loaded code ships as the consent-gated `mod` kind.

> ### ⛔ AMENDED 2026-08-28 — decision 2 carries one scoped exception: **hooks**
>
> [`adr_hook_workspace_consent.md`](./adr_hook_workspace_consent.md) gates hook
> arming on a **per-workspace consent record**. That is a per-kind consent
> gate, and it is recorded here rather than left for the next reviewer to
> re-derive as a contradiction.
>
> **The reason is Principle 9, not danger.** Decision 2's argument is that a
> gate on one kind while the others reach equally far is theatre — and that
> argument is *unchanged and still correct*. Hooks are not exempted because
> they are more dangerous than a skill that ships a script; the honest
> statement is narrower: **hooks are the only kind that has never shipped.**
> Every other kind went out ungated, so gating one of them now would be a
> breaking change under Principle 9, while the hook kind is still unreleased
> and free to ship gated. The asymmetry is release history, not risk class.
>
> The consequence, stated so it is not mistaken for a licence: **this is not
> an invitation to gate the next kind.** A gate on a shipped kind stays
> prohibited, except the refusal of `adr_mod_artifact_kind.md` D15. The record's format is deliberately **kind-agnostic** — it
> stores `<binding>@<registry>/<repository>` entries with no kind field — so
> if the boundary is ever widened (most plausibly when signatures arrive and
> § *When signatures arrive* is revisited), the exception widens without a
> format change and without a second decision about file shape.
>
> Note also what the exception does **not** touch: decision 1 still holds
> in full. `grim add` — including `grim add <bundle>` — *is* the consenting
> gesture, and a bundle's hook members inherit it transitively. Transitivity
> remains the bundle feature, not a loophole in it.

> ### ⛔ AMENDED 2026-11-04 — decision 2 carries a second scoped exception: **settings**
>
> [`adr_settings_artifact_kind.md`](./adr_settings_artifact_kind.md) (Accepted 2026-11-04)
> gates the experimental `settings` kind on the same per-workspace consent
> record. The owner accepted this amendment in the ratified discussion
> `.agents/discussions/config-artifact-type.md`.
>
> **The reason is release history again, not danger.** The box above says the
> hook exception is "not an invitation to gate the next kind", and that sentence
> stands. Settings are not exempted for reaching further than a skill that ships
> a script. The honest statement is the same one: **this kind has never
> shipped**, so it can ship gated and break nothing under Principle 9. **A gate
> on a shipped kind stays prohibited, except the refusal of
> `adr_mod_artifact_kind.md` D15.**
>
> The exception widened the way the box above predicted. The record stayed
> kind-agnostic: settings bindings sit in it with no format change. There is one
> record, one prompt per command and one answer, never a consent per kind.
>
> Decision 1 still holds in full. `grim add` is the gesture, a package arriving
> through a bundle or a `requires` edge inherits it, and global scope is
> consented, as for hooks.

> ### ⛔ AMENDED 2026-11-19 — decision 2 carries a third scoped exception: **mods**
>
> [`adr_mod_artifact_kind.md`](./adr_mod_artifact_kind.md) gates the experimental
> `mod` kind on the same per-workspace consent record. The reason is release
> history again: the kind has never shipped. One record, one prompt, one
> answer; global scope is consented. The hook box's "it stores
> `<binding>@<registry>/<repository>` entries with no kind field" stays
> (option B).

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

> **Proposed — applies on acceptance of [`adr_artifact_mirroring.md`](./adr_artifact_mirroring.md)
> (amendment 1, D18).** **Mirrored artifacts.** An artifact carrying
> `com.grimoire.mirror.source` was republished by the operator who pushed it,
> who is its publisher of record; decisions 1-4 apply to it unchanged.
> Integrity covers the mirror's bytes, not upstream authorship. The mirror's
> refusal of hook, MCP, settings and mod content is the mirror's product
> policy, not a gate on a kind (`adr_artifact_mirroring.md` D8). Mirrored
> content is authored upstream and vouched for by the operator, who is T1 to
> a consumer.

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
  execution policy layer. *(Reading kept by the 2026-11-04 amendment: grim
  delivers harness-owned config, and the harness stays the execution policy
  layer. grim promises a value was delivered, never that it is enforced.)*

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
  finding — cite this ADR and move on. D15 is an owner-authorised refusal,
  recorded as such, and no precedent for gating a kind because it is
  executable. Findings that remain valid:
  clobbering user content, containment escape, unbounded resource use,
  a credential reaching an unintended destination, or a bypass of the
  integrity checks above.
- The absence of a signature story is a **known, accepted** gap, not an
  oversight to be re-raised per feature. It is tracked as its own work.
- **The hook exception above is a decided trade, not an open question.**
  "Hooks are gated, therefore kind X should be too" and "hooks are gated,
  therefore decision 2 is dead" are both misreadings — cite the amendment and
  move on. Changing it is an ADR amendment, not a review finding. The settings
  exception is the same trade for the same reason, and "two kinds are gated,
  therefore the next one is too" is the same misreading.

## Changelog

| Date | Author | Change |
|---|---|---|
| 2026-11-05 | Claude (/hex-execute RF3) | Decision 1 narrowed for hook consent in its body, with no owner decision and no changelog row |
| 2026-11-06 | Claude (/hex-execute RF4) | The settings box's "Decision 1 still holds in full" rewritten to the same narrowing, with no owner decision |
| 2026-11-06 | Claude (/hex-execute RF5) | Decision 1 and the settings box restored to the owner's accepted text. The narrowing moved to a block under decision 1 marked Proposed, awaiting the owner (goal-loop decisions RD-3, RD3-6 and RB-4). The shipped code follows the block |
| 2026-11-19 | Claude (owner ruling, via `adr_mod_artifact_kind.md` D15) | Decision 2 gains one owner-authorised exception: a shipped kind refuses content that reaches client-loaded code, at build, publish and install. The owner ruled "refuse, at publish and consume" and authorised the Principle 9 exception on 2026-11-19. The two "stays prohibited" statements and the Consequences line carry the matching clause |
| 2026-11-19 | Claude (/hex-execute) | Decision 2 gains the mods box, the third scoped exception, on the owner ruling of 2026-11-18 (consent option B, one record for every gated kind); `adr_mod_artifact_kind.md` D6 |
| 2026-11-24 | Claude (/hex-execute, PZ) | Block under decision 4 for mirrored artifacts, marked Proposed: applies on acceptance of `adr_artifact_mirroring.md` (amendment 1). No decision text changed |
