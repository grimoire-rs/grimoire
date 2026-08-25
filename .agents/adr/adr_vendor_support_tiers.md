# ADR: Where grim stops compensating for a client capability gap

## Metadata

**Status:** Proposed
**Date:** 2026-08-25
**Deciders:** maintainer
**Beads Issue:** N/A (GitHub tracking: grimoire-rs/grimoire#104)
**Related PRD:** N/A
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md` (no new tech; a boundary, a docs section, and a rule)
**Domain Tags:** integration
**Supersedes:** N/A

## Context

grim ships 18 client vendors. `KindSupport { Native, Degraded, Declined }`
(`src/install/vendor.rs:74`, from
[adr_vendor_wave_expansion.md](./adr_vendor_wave_expansion.md) §2) answers
**what a client can host**, and `docs/src/clients.md` publishes that answer
under a table-parity test
([adr_client_compat_matrix.md](./adr_client_compat_matrix.md)).

Nothing answers the second question: **how hard grim works to close a gap it
finds.** Three open requests land in the same inbox and are not the same kind
of thing at all:

- **grimoire-rs/grimoire#100** floats auto-installing an OpenCode plugin that
  reimplements Claude's rule engine via `tool.execute.before` and the
  `experimental.chat.*` transforms — hooks upstream marks unstable.
- **grimoire-rs/grimoire#102** needs grim to register `claudeMdExcludes` in a
  consumer's `.claude/settings.json`, because grim's own support-directory copy
  auto-loads as unconditional global context in Claude Code.
- **grimoire-rs/grimoire#103** wants a `paths:` scope that a client cannot
  express rendered into the rule body as prose.

Each was argued from first principles, and the answers were drifting apart.
The pressure is real and increasing: grim is in production use inside large
engineering orgs, so "the client cannot do this" is increasingly answered with
"then make grim do it". With 18 vendors, compensating everywhere is not
affordable; compensating nowhere is not honest.

The `KindSupport` axis cannot absorb this. It describes the *client*. The
missing axis describes *grim's willingness*, and the two are independent: a
`Degraded` cell can be left as-is, papered over with a render, or fixed with a
runtime shim, and today nothing says which.

## Decision Drivers

- **Every gap request is currently a fresh argument.** The cost is not the
  individual answer, it is re-deriving the principle each time and getting
  inconsistent results.
- **Maintenance asymmetry.** A render is paid once. Runtime code loaded into
  another product is paid on that product's release cadence, forever, and its
  failure mode is silent — rules quietly stop loading and the consumer's agent
  gets worse with no error.
- **De-facto tiers already exist.** Claude, Codex, OpenCode and Copilot get
  first attention; the other fourteen do not. Unwritten tiers produce
  inconsistent promises rather than no promises.
- **Principle 9 (compatibility).** A compensating behavior is easy to add and
  breaking to withdraw. Declining to compensate stays additive to reverse.
- **Honest declines over silent lossy installs** — the doctrine already set by
  the Codex-rules gate and generalized in the wave-expansion ADR.

## Industry Context & Research

The package managers grim resembles do not cross this line. Homebrew renders
formulae and writes into prefixes it owns; it does not ship plugins into the
applications it installs. `asdf`/`mise` shim binaries they themselves place on
`PATH` — their own output — and stop there. The closest counter-example is the
editor-extension ecosystem, where a package *is* runtime code — but there the
extension host is a stable, versioned, contract-bearing API, which
`experimental.chat.*` explicitly is not.

The relevant local precedent is grim's own `src/install/opencode_config.rs`:
grim already writes a managed entry into a vendor's config file so its rules
load. That is accepted practice here and is not what this ADR restricts. What
it restricts is the step past it — shipping executable behavior.

**Research artifact:** N/A (decision derived from the three open issues and
existing ADR doctrine).
**Key insight:** the three requests differ by *what grim has to keep alive
afterwards*, not by how hard they are to build. That is the axis worth
encoding.

## Decision Outcome

**Chosen Option:** Option A — a three-class boundary plus a maintainer-internal
tier list, published as a consumer-facing statement of behavior.

The line, stated once:

> **grim renders artifacts. grim does not run at the client's runtime.**

### 1. Three classes of gap response

| Class | What it is | Policy |
|---|---|---|
| **1 — repair** | grim making its **own** output behave as authored | Always in scope. Every client, every tier. Not a feature, not negotiable, sets no precedent. |
| **2 — compensating render** | A static artifact the vendor already documents and interprets: a config key, an extra generated file, adjusted body text. Deterministic, removed on uninstall, inert if grim vanishes. | Tier 1 clients only. |
| **3 — runtime code** | Anything grim must keep alive against someone else's API: plugin, extension, wrapper process, injected script. | Never. Any client, any tier. |

Class 1 is the one most likely to be mistaken for scope creep and is not. If
grim wrote the files, grim owns their behavior; a consumer who installed a
path-scoped rule did not consent to unconditional context. Repairing that is
the same duty as writing correct bytes in the first place.

Class 3 is refused on maintenance shape, not on difficulty. The test is
whether a client release can break grim's contribution without grim being
touched, and whether that break is silent. A plugin fails both.

Worked classifications, recorded so the boundary has calibration points:

| Request | Class | Why |
|---|---|---|
| grimoire-rs/grimoire#102 — register `claudeMdExcludes` | 1 | grim's own support-directory copy caused the over-load |
| grimoire-rs/grimoire#103 — render a dropped `paths:` scope as prose | 2 | Body text, inside the existing `rule_index` transform |
| Render a command file so an OpenCode skill is `/`-invocable | 2 | One more generated file, derived from `name` + `description` |
| grimoire-rs/grimoire#100 — an OpenCode rule-engine plugin | 3 | Executable code in another product, against unstable hooks |

### 2. Tier 1 is `claude`, `codex`, `opencode`, `copilot`

Every other client is Tier 2 by default: faithful render, native layout and
frontmatter mapping, gaps declined or warned per `KindSupport`, never
compensated.

Tier 1 has a stated price, and it is the price that defines the tier rather
than any judgement about the client's quality:

- a `docs/src/clients.md` matrix row under the parity test,
- a dated row in `.claude/rules/vendor-capability-watchlist.md`,
- re-verification against upstream when the vendor moves.

A client is promoted when someone commits to paying that; demoted when nobody
does. Tier is a statement about maintainer capacity, not about the client.

Note the tier and the `KindSupport` cell are independent. Cursor and Kiro are
Tier 2 yet have `Native` scoped rules — fuller native support than Tier 1
OpenCode. Tier governs compensation for what a client *lacks*, so a client that
lacks little needs little.

One carve-out, taken from the price rather than from the principle: a class-2
render derived **solely from the artifact's own frontmatter** — depending on no
upstream key, surface or version — is not tier-gated, because there is nothing
upstream to re-verify and so nothing for Tier 1 to buy. The `paths:` prose
notice (grimoire-rs/grimoire#103) is exactly that: it restates grim's own
metadata as body text and reads the same whatever the client ships next. A
class-2 render that *tracks* a vendor surface — a config key, a generated file
in the client's own format — remains tier-gated, because tracking is the cost
the tier exists to bound.

### 3. Publication split: the boundary is public, the tier list is not

`docs/src/clients.md` gains a `## What grim will and will not do
{#compensation}` section stating the three classes in consumer language — with
no tier names and no tier table. A consumer of any of the 18 clients learns
what grim will do about a gap, and specifically that grim will never install
runtime code into their client.

The tier *list* stays internal, in this ADR and in
`.claude/rules/vendor-capability-watchlist.md`. Publishing it would rank
fourteen clients second-class in exchange for no consumer benefit: what a
consumer can act on is the per-cell matrix (already published) and the
behavioral promise (now published). "Which clients the maintainer prioritizes"
is a roadmap fact that changes with capacity, and freezing it into a docs page
under Principle 9 would make a capacity statement read as a contract.

### 4. The frontmatter gate

Recorded here because it is the same temptation arriving from the authoring
side: every gap invites a new grimoire-namespaced field carrying *intent*.

> Add a field only when **two artifacts of the same kind want different
> answers, and the author is the one who knows which.**

Both live candidates fail it:

- A support directory is *always* non-auto-loading. Its presence is already the
  signal (grimoire-rs/grimoire#102) — a field whose value never varies only
  rots.
- A skill wants a command wherever the client supports one. That is a
  client-capability decision, which grim already knows, not an artifact
  decision the author can inform.

This is Principle 9 applied forward: an optional field is additive to add and
permanent to keep.

### 5. Enforcement is the rule and the hook, not a new test

`.claude/rules/vendor-capability-watchlist.md` carries the tier list and the
classify-before-you-compensate gate. It auto-loads on
`src/install/vendor_*.rs`, which is exactly the edit where the question arises,
and `.claude/hooks/post_tool_use_tracker.py` already fires a reminder naming
that file on the same paths.

No parity test is added for the tier list. A Rust test reading `.claude/rules/*`
would be a novel coupling for a drift already covered by two layers. If it
drifts once, the upgrade is a row-presence test in `client_target.rs` reusing
`heading_section` + `contains_word` — the shape the emit-matrix tests already
use — pointed at the rule file.

## Considered Options

### Option A: Three-class boundary + internal tier list (chosen)

| Pros | Cons |
|------|------|
| Future gap requests resolve by lookup, not by argument | The class of a novel request still needs a judgement call |
| Consumers get the promise that actually affects them, without a public ranking | Tier list is invisible to consumers, so priority questions still arrive |
| Refuses the unbounded-maintenance class before the first one ships | Refusing class 3 will disappoint a real request (#100) |

### Option B: `fn tier()` on the `Vendor` trait

**Description:** Encode the tier in code beside `kind_support`, so it is
introspectable and testable.

| Pros | Cons |
|------|------|
| Machine-checkable, cannot drift from `ALL` | Nothing branches on it — a trait method with no caller is speculative generality (YAGNI) |
| Could feed a future `grim clients --format json` | Turns a capacity statement into a code fact, which then wants a stability promise |

### Option C: Publish the tier table in `clients.md`

**Description:** A tier column or a second table beside the support matrix.

| Pros | Cons |
|------|------|
| Maximum transparency about what to expect | Publicly ranks 14 clients second-class for no actionable consumer benefit |
| One page answers every question | Freezes a capacity statement into a docs contract under Principle 9 |

### Option D: Publish criteria, keep no fixed list

**Description:** State what earns compensation (wide use, cheap gap, someone
pays the upkeep) and decide per request.

| Pros | Cons |
|------|------|
| Maximum flexibility; no client is written off | Reintroduces the per-request argument this ADR exists to end |
| No list to maintain | Criteria without a list resolve nothing at the moment of decision |

**Rationale:** A is chosen because the expensive problem is the *re-derivation*,
not the individual verdicts, and A ends that with a lookup while B, C and D each
pay a cost for a property nobody consumes. B adds an uncalled API, C converts
capacity into contract, D changes nothing at decision time.

### Consequences

**Positive:**
- grimoire-rs/grimoire#103 resolves by lookup: prose render is class 2, and
  frontmatter-only class-2 renders are not tier-gated, so it lands for both
  `Degraded`-scoping clients — Tier 1 OpenCode and Tier 2 Junie.
- grimoire-rs/grimoire#102 is class 1 and was never gated on tier at all.
- grimoire-rs/grimoire#100's plugin proposal has a written, citable refusal
  reason that is not "we don't want to".
- The consumer-facing promise ("no runtime code in your client") is a genuine
  trust property for the orgs deploying grim centrally.

**Negative:**
- A real user need — full `paths:` semantics in OpenCode — stays unmet by
  design. The class-2 prose render is a partial answer, not a fix.
- Tier 2 clients accumulate unaddressed gaps, and the tier list being internal
  means those users cannot see why.

**Risks:**
- *Class boundary erosion*: a future request will sit right on the class 2/3
  line (a generated file the client happens to execute). Mitigation: the test
  is "can a client release break this silently without grim being touched",
  applied to the artifact, not to its file extension.
- *Tier list rots as capacity changes*: mitigated by the watchlist rule and the
  hook reminder firing on exactly the vendor edits that should prompt review.

## Related

- [adr_vendor_wave_expansion.md](./adr_vendor_wave_expansion.md) — §2
  `KindSupport`, the orthogonal axis this sits on top of
- [adr_client_compat_matrix.md](./adr_client_compat_matrix.md) — the enforced
  matrix and the docs-parity discipline inherited here
- [adr_managed_context_block.md](./adr_managed_context_block.md) — the
  class-2 injection engine that wave 2 rules depend on
- `.claude/rules/vendor-capability-watchlist.md` — carries the tier list and
  the classify-first gate

## Follow-ups

- ~~grimoire-rs/grimoire#103 — implement the class-2 prose render for Degraded
  rule scoping.~~ Landed for OpenCode and Junie, the two `Degraded` clients.
  Kiro, named here in the first draft, is not one: it is `Native`, writes
  correct `fileMatch` scoping, and its warning is about upstream inertness
  (kirodotdev/Kiro#9176), which self-heals without a grim change.
- Re-check the Tier 1 set when usage telemetry lands
  (grimoire-rs/grimoire#83) — the first evidence-based promotion/demotion
  input grim will have.

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-08-25 | maintainer | Initial draft |
| 2026-08-25 | maintainer | Frontmatter-only class-2 renders are not tier-gated (#103); Kiro removed from the #103 follow-up — it is `Native`, not `Degraded` |
