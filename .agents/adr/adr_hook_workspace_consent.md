# ADR: Hook arming is gated by workspace consent, not registry trust

## Metadata

**Status:** Accepted
**Date:** 2026-08-28
**Deciders:** Michael Herwig + Claude (hex swarm, `hex/hooks-artifact-kind`)
**Beads Issue:** N/A
**Related PRD:** N/A
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md` (no new dependency; one JSON file and one pure predicate)
**Domain Tags:** security
**Supersedes:** the registry half of [`adr_hooks_support.md`](./adr_hooks_support.md) amendment **A2**, and contract **C-022** in [`plan_hooks_artifact_kind.md`](../plans/plan_hooks_artifact_kind.md) (with its B4 / B5 / B7 / W8 / S2-2 amendments)
**Superseded By:** N/A

## Context

The `hex/hooks-artifact-kind` branch gated hook arming on **registry-scoped
trust**: a `trust_hooks` field on a `[[registries]]` entry in *global* config,
plus a one-time prompt that wrote one. That gate answers *"which publisher's
code may run"*. Nothing on the branch answered *"which checkout may arm hooks
at all"* — and that is a threat the project has written down as in scope:

> **T3 — An untrusted repository the user clones or opens.** […] **The user is
> *not* vouching for a repo by cloning it.**
> — [`arch-threat-model.md`](../../.claude/rules/arch-threat-model.md)

The gap was not a new discovery. [`adr_hooks_support.md`](./adr_hooks_support.md)
**Decision E point 4** specified it by name, with the right precedent:

> **Approval is bound to the tuple, not the content — and the tuple names a
> directory, not a scope kind.** `direnv` shipped content-only trust and had to
> fix it to path + content, because an approved `.envrc` copied into a hostile
> directory executed ([`direnv/direnv#83`](https://github.com/direnv/direnv/issues/83)).
> […] Corrected key: **`(artifact content digest, scope root)`** […] Approving
> a hook in a scratch project therefore does not arm it in a production repo.

**Amendment A2 deleted that whole clause as collateral.** A2's stated motive
was *"no one wants to review every hook"* — an argument against the **digest**
half of the key. It threw out the **scope root** half with it, and replaced
*which directory* with *which publisher*. Those are orthogonal axes. Deleting
both left T3 uncovered by anything, and the branch's own acceptance suite had
no test for the clone case because there was no mechanism to test.

The registry gate had also accumulated predicate. Five amendments (B4's scope
precedence table, B5's locator matching with its bare-host and `index` rules,
B7's tri-state serializer hazard, W8's `insecure` rule, S2-2's transport
carve-out) all existed because *a config file was an input to an arming
decision*, and a project `grimoire.toml` is an ordinary repository file. Each
amendment closed one way a repo could speak into that decision. None of them
addressed the fact that the repo was being asked at all.

**Principle 9 does not bind.** `git grep trust_hooks origin/main` returns
nothing: the whole hook kind is unreleased. Removing `trust_hooks` costs
nothing. Adding a consent gate to any *shipped* kind would cost a great deal,
and that asymmetry is what scopes this change — see the amendment to
[`adr_artifact_trust_model.md`](./adr_artifact_trust_model.md).

## Decision Drivers

- **T3 must be covered by something.** A fresh clone of a hostile repository
  that declares a hook must arm nothing, whatever else the machine trusts.
- **A2's coarseness is kept deliberately, not conceded.** No per-hook prompt,
  no digest key, no approval store, no hash chain. The owner's reasoning
  against reviewing every hook stands; only its *axis* was wrong.
- **No config file may be an input to an arming decision.** That is what
  produced five amendments of accumulated predicate, and every one of them
  dissolves when the input goes away.
- **The gate must be answerable once, legibly, by the person whose machine it
  is** — and revocable the same way.
- **Prompt fatigue is a real failure mode**, named in the hooks ADR's own risk
  list. Once per workspace, re-asked only on declaration drift.

## Industry Context & Research

**Research artifact:**
[`research_hooks_autoexec_supply_chain.md`](../research/research_hooks_autoexec_supply_chain.md)
carries the incident evidence behind T1, T2 and I4; this decision adds three
precedents on the *directory* axis it did not cover.

**`direnv`** is the load-bearing one, and it is a correction rather than a
design: `direnv` shipped **content-only** trust and had to add the path,
because an approved `.envrc` copied into a hostile directory executed
([`direnv/direnv#83`](https://github.com/direnv/direnv/issues/83)). The
allow-file is keyed on the absolute path *and* the content hash, and lives
under the user's own data directory — never in the repository. That is the
exact shape adopted here, minus the content half A2 removed.

**Claude Code's folder trust** is the second, and citing it requires
correcting this project's own record.
[`adr_hooks_support.md`](./adr_hooks_support.md) **Key insight 6** dismissed it:
*"Claude's mechanism is coarser (folder-level trust). Citing Claude as the
precedent would cite the wrong vendor."* That judgement was made **before A2
chose coarseness deliberately**. Once per-hook digest approval is off the
table, "coarser" is no longer a defect in the precedent — it is the property
being adopted. Gemini CLI's `trusted_hooks.json` fingerprinting stays the
precedent for the *content* axis grim no longer walks; Claude's folder trust
is the precedent for the axis it now does.

**OCX's per-project consent stamp** is the sibling implementation
(`~/dev/ocx/crates/ocx_lib/src/project/consent.rs`, `ocx shell allow|revoke`).
Its shape is adopted deliberately, including the parts that look like
incidental details, because each is a recorded lesson:

| OCX property | Why it is not incidental |
|---|---|
| identity stored **inside** the record, not just in the filename | the filename is a truncatable hash; the path is not (A-25) |
| unknown schema version ≡ absent, never an error | a damaged record must degrade to "not consented", never block (I3) |
| a **closed allowlist** of write seams, enforced by test | visibility is not a control; a read-only command that grants is the defect (A-29) |
| drift measured on the declared **source set** | a new source is a new decision; a new version of a known one is not |
| the tool's own home needs no stamp | there is no third party's checkout to gate (A-44) |

**Key insight:** the two questions grim was conflating have different shapes.
*Whose bytes are these* is answered by content, and grim already answers it
with the lock's digest pin. *May this checkout arm hooks* has no content to
key on — a workspace is not an artifact. Trying to answer the second question
with a publisher name produced a gate that a cloned repository could
participate in.

## Considered Options

### Option 1: Keep registry trust, harden it further

**Description:** Retain `trust_hooks`, and close the remaining ways a
project-scope file can influence the decision.

| Pros | Cons |
|------|------|
| No new file format, no new commands | Does not cover T3 **at all** — a victim who legitimately trusts `ghcr.io/acme` arms `ghcr.io/acme`'s hook from any clone |
| Answers "which publisher" legibly | Five amendments already; each new one is another way a repo speaks into an arming decision |
| Visible in `git diff` of global config | Restores nothing of Decision E point 4 |

### Option 2: Restore Decision E in full — per-hook digest approval plus scope root

**Description:** The original `(artifact content digest, scope root)` key, with
the approval store and hash chain.

| Pros | Cons |
|------|------|
| Covers T3 and T1's version axis together | **Reverses an owner decision on its own merits** (A2), which is not this change's mandate |
| Strictly the strongest option on paper | Re-prompt habituation is a named risk in the hooks ADR's own list |
| | Reintroduces `hook_approvals.json`, the chain, and the per-artifact key that A2 deleted for cost reasons that still hold |

### Option 3: Workspace consent — restore E4's directory half only — **chosen**

**Description:** One machine-local record per workspace, listing the hooks that
workspace declared, written only by an explicit gesture. Registry trust is
removed outright.

| Pros | Cons |
|------|------|
| Covers T3 by construction: nothing a repository carries is an input | A hostile upstream can still ship a new *version* of a consented hook without a re-prompt (T1 residual, named below) |
| Keeps A2's coarseness exactly — no per-hook prompt, no digest key, no store, no chain | Records are never garbage-collected (accepted, below) |
| B4, B5, B7 and W8's config-shaped predicate **dissolve** — no config file is an input | A stale `trust_hooks` line in a branch tester's global config becomes a hard exit 78 (accepted, below) |
| The predicate is pure — no I/O, no clock, no environment — so every row is a unit test | |

## Decision Outcome

**Chosen Option:** Option 3.

**Rationale:** It is the only option that covers T3, and it does so without
re-opening a decision the owner took on its merits. A2 reversed D5 on the
grounds that reviewing every hook is unacceptable; that argument is about the
*digest*, and Option 3 concedes it entirely. What Option 3 restores is the
half of Decision E that A2 never argued against and deleted as collateral —
the directory binding, with `direnv` as its named precedent, which the hooks
ADR claimed to follow and did not.

### The decision, stated

**1. Arming is gated on the workspace, not the publisher.** The record is
`$GRIM_HOME/hooks/consent/<workspace-key>.json`, one file per workspace,
machine-local, never repo-resident (**I1**). `<workspace-key>` is
`hook_dispatch::workspace_key` **verbatim** — the SHA-256 of the workspace
path, hex — reused rather than spelled a second time, so the payload directory
and the consent record agree by construction.

```json
{
  "v": 1,
  "workspace": "/abs/path/to/checkout",
  "hooks": ["guard@ghcr.io/acme/hooks/command-guard"],
  "consented_at": "2026-08-28T10:00:00Z"
}
```

Every field is required and unknown fields are denied: a truncated record must
not deserialize into a valid-looking one. `workspace` is the **identity** and
the filename is a lookup index only — a record whose `workspace` does not equal
the resolved one is not consent for it. An unknown `v`, a parse error or an I/O
error all read as **absent**: logged at debug, never warned, never an error
(**I3**).

> **The workspace path must be absolute, and making it so was a fix, not a
> restatement.** Adversarial review found that an explicit `--config` is used
> verbatim, so `grim --config grimoire.toml` left `config_path.parent()` empty
> and `--config ./grimoire.toml` left it `"."` — *the same value in every
> checkout on the machine*. The consent record, the hook payload directory and
> the project install-state path all key on it, so two unrelated repositories
> shared one consent key and armed each other's hooks: `direnv/direnv#83`
> reproduced inside grim, by the very mechanism meant to prevent it. Closed at
> the source (`scope_resolution::workspace_of`) rather than as an arming rung,
> because the payload directory and the state path key on the same value and a
> rung would have fixed only the third consumer. It resolves against the
> process CWD and deliberately does **not** canonicalize: canonicalizing needs
> I/O, fails on a path that does not exist yet, and resolves symlinks — which
> would make two spellings of one directory *agree*. Two spellings disagreeing
> merely re-gates, which is the fail-safe direction.

**One file per workspace, never one shared file.** Decision E's own reasoning
about the dispatch table applies unchanged: a whole-file rewrite would let one
workspace's operation disarm every other. Per-workspace files remove the
read-modify-write entirely — no lock, no cross-workspace blast radius.
`consent` joins `RESERVED_ARTIFACT_NAMES`, because a hook bound to a name grim
writes under `$GRIM_HOME/hooks/` lands a directory on that path.

**2. The consented set is `<binding>@<registry>/<repository>` — no tag, no
digest, and no tier.** A new hook, a rebinding, or one from a new repository is **drift**:
arming stops, and the report names the new entry. A version bump of an
already-consented hook is **not** drift and does not re-ask. That is A2's
coarseness held deliberately; the bump is visible in the lock's `git diff` and
in `grim status`, which is **I5** — tamper-evidence, not prevention.

> **The tier is not in the key, and that was asked and answered.** Adversarial
> review proposed adding it, on the ground that a consented hook moving from
> `observer`/`PostToolUse` to `gatekeeper` on a `PreToolUse` `Bash` matcher is
> a capability escalation that reads as a routine version bump. The escalation
> is real. The tier is simply **not knowable at this seam**: the consented set
> is computed from the *lock*, and the tier lives in the materialized
> `hook.toml`, which for a never-installed hook does not exist yet. Keying on
> it would mean fetching and unpacking every hook in order to decide whether it
> may arm — a resolution step ahead of the gate that authorizes it. It is the
> same T1 shape as the version bump, with the same answer: the digest pin, plus
> `grim hook list` and the lock's `git diff` as evidence.

**3. Global scope is always consented and never carries a record.**
`$GRIM_HOME/grimoire.toml` is the user's own file on the user's own machine.
T3 does not reach it, and there is no third party's checkout being gated, so
consent has nothing to decide. This is OCX A-44, and it is also
[`adr_artifact_trust_model.md`](./adr_artifact_trust_model.md) decision 1 —
editing your own config *is* the declaration gesture. `record()` refuses global
scope, so "nothing ever writes a global record" is a testable invariant rather
than a convention, and `grim hook allow --global` exits **64**: a usage error,
not a failed write.

**4. The write seam is a closed allowlist, stated as a negative contract.**

- **May write:** `grim hook allow`; `grim add` (typing a ref *is* the
  declaration gesture); an accepted interactive prompt.
- **Must never write:** `grim install`, `grim update`, `grim lock`,
  `grim status`, `grim context`, `grim hook list`, `grim hook run`, the TUI,
  the MCP server.

**This is the T3 control**, and it is enforced by an acceptance test rather
than by visibility (OCX A-29). `grim install` materializes what is already
declared; a cloned repository's `grimoire.toml` is not the user's gesture.
Never put the write in a shared loader — per-caller opt-in only, or a
read-only command silently grants.

`grim add <bundle>` consents to the bundle's hook members, which the user did
not name individually. That is deliberate and already settled by
[`adr_artifact_trust_model.md`](./adr_artifact_trust_model.md) decision 1:
*"adding a bundle is the user's statement of trust. Everything that gesture
transitively pulls in inherits it."* The record stores the **resolved** set, so
a bundle that later gains a hook member drifts and re-gates. Bundles are not
special-cased.

**5. The plain-HTTP control survives, relocated to the transport layer.**
`trust::decide` condition 5 withheld an implicit grant from any entry reached
over plain HTTP, because *the first resolution that produces the digest pin is
itself attacker-influenceable on the wire, so the pin cannot rescue it*
(**T2**). Workspace consent has no natural place for that condition, and
dropping it silently would lose a real control. It is orthogonal to consent and
belongs at the transport layer anyway, so it becomes a standalone gate: a hook
whose pinned registry host is **non-loopback and reached over plain HTTP** does
not arm, cause `insecure-transport`.

This is **stronger** than condition 5, not weaker. Condition 5 could be escaped
by writing `trust_hooks = true` — i.e. from a config file. This one cannot be
escaped by any file, only by `--trust-hooks` on the invocation (**N4**). The
host list is still computed from **both** config scopes' authored entries: a
cloned repo declaring `insecure = true` can now only stop its own hook from
arming, which is fail-safe, and narrowing the list to global scope would give
that direction away for nothing.

**6. `--trust-hooks` / `--no-trust-hooks` keep their names and semantics.**
Per-invocation, never persisted, beats the record in both directions — owner
decision 2026-08-28, **N4**. A flag typed on this run is the most specific
answer there is, and no file can type one. The spelling also keeps maximum
distance from `GRIM_ALLOW_HOOKS`, permanently forbidden by owner decision
2026-08-17 (commit `24a14bb`, withdrawing C-026). There is likewise **no
`GRIM_HOOK_CONSENT`** and no environment form of the record's path, for the
identical CWE-426 reason: an env-settable path would let a repository supply a
pre-consented file.

**7. Reporting.** `HookArmingCause::RegistryNotTrusted` becomes
**`WorkspaceNotConsented`**; **`ConsentDrifted`** and **`InsecureTransport`**
are added. `ConsentDrifted` closes issue #92 on the axis that turned out to
matter: the two states worth telling apart are *never consented* vs *consented,
then the declaration changed* — not *never granted* vs *explicitly opted out*.
`grim status` still consults the dispatch table **first**, so a `--trust-hooks`
arming keeps reporting `installed`.

### Relationship to invariant I2

**I2 says trust keys on a content digest, "never on a name", and that keying
trust on a name is a Block.** A workspace path is a name. This must be stated
rather than glossed, and it is now recorded in
[`arch-threat-model.md`](../../.claude/rules/arch-threat-model.md) under I2
itself so a reviewer meets it before filing a finding.

Three facts bound the trade:

1. **I2 already carried a hook exception.** A3 dropped the exec-time digest
   re-check — *"a recorded trade against I2, resolved as I5"*.
2. **Registry-scoped `trust_hooks` was itself a name key.** A registry locator
   is a name. Workspace consent changes **which** name is keyed on; it does not
   introduce name-keying, and it does not widen the trade.
3. **The digest half still holds at install time.** *Which bytes* an artifact
   is remains the lock's pinned manifest digest, verified against the
   registry's answer — T1 and T2 are answered there, per
   [`adr_artifact_trust_model.md`](./adr_artifact_trust_model.md) decisions 3
   and 4. Consent never substitutes for a pin.

And one fact justifies it rather than merely bounding it: *may this checkout
arm hooks at all* is not a property of any artifact. A workspace has no digest,
so keying it on content is not a stricter option — it is the documented bug.
`direnv/direnv#83` is precisely that: content-only trust, and an approved
`.envrc` executing in a directory nobody approved.

### Consequences

**Positive:**

- T3 is covered by construction, and testable: a fresh clone declaring a hook
  from a registry the victim trusts in every other sense arms nothing, exits
  `0`, and `grim status` says why.
- Five amendments' worth of predicate dissolve — B4's scope precedence table,
  B5's locator matching, the bare-host and `index` rules, B7's tri-state
  serializer hazard, W8's `insecure` clause. No config file is an input.
- The predicate is pure (`record`, `workspace`, `declared` → `Granted |
  Drifted | Absent`), so every row is a unit test — the property `trust::decide`
  got right and worth keeping.
- `RegistryField::ALL` returns to the **six shipped names in their shipped
  order**, restoring the frozen list; the seventh existed only on this branch.
- Prompt frequency drops from "once per registry, forever" to "once per
  workspace, re-asked only on declaration drift".

**Negative:**

- Consent is per machine, so a developer working the same repository on two
  machines answers twice. That is the direnv property, and it is the point.
- Two commands and one file format that did not exist before.

**Risks — three residuals, named rather than engineered against:**

1. **A hostile upstream can ship a new *version* of an already-consented
   hook without a re-prompt.** The consented set carries no tag and no digest.
   That is **T1**; grim's answer to T1 is the digest pin
   ([`adr_artifact_trust_model.md`](./adr_artifact_trust_model.md) decisions 3
   and 4), and its visibility is `git diff` on the lock plus `grim status` —
   **I5**, evidence, not prevention. Do not describe consent as preventing it.
2. **A stale `trust_hooks` line becomes a hard load failure.**
   `RegistryConfig` is `deny_unknown_fields`, so a global config still carrying
   `trust_hooks = true` exits **78** on every command touching that file.
   **Accepted, with no compatibility shim** — `quality-core.md` forbids shims
   when the code can simply change. No released grim ever wrote the field; the
   blast radius is people testing this branch, who delete one line. It is said
   in the commit body and here so it never surfaces as a mystery 78.
3. **Consent records are never garbage-collected.** A record for a deleted
   workspace, or one whose hooks were all uninstalled, lingers. It is inert —
   consent grants nothing when nothing is declared — and `grim hook revoke` is
   the only remover. OCX has `ocx clean`'s liveness sweep; grim has no
   equivalent and does not need one for v1. Recorded as accepted, not as an
   oversight.

## Technical Details

### The gesture

| Command | Behaviour |
|---|---|
| `grim hook allow [path]` | Record consent for the resolved workspace over its currently-declared hook set. `--global`, or a resolved global scope, exits **64** naming the reason. |
| `grim hook revoke [path]` | Remove the record. **Idempotent**: `Removed` / `Absent`, both exit **0** — revoking what was never granted leaves exactly the state asked for, and erroring on the command's most ordinary outcome would be the defect. |

Both are ordinary scope-resolving `Printable` commands returning a single-object
report (`{workspace, action, hooks}`), following the wiring `grim hook list`
already established. Neither disarms by itself: the record is read at
convergence, so `grim install` is the second step — exactly as it is for the
feature flag.

### Arming composition

Order, and the reason for each step:

1. `!feature_enabled` → `NotArmed(FeatureOff)` — **I4**, default-deny first,
   unreachable past. The flag pair does not open it.
2. `flag == Some(false)` → `NotArmed(FlagDenied)`.
3. `flag == Some(true)` → `Armed(Flag)` — **N4**.
4. non-loopback host over plain HTTP → `NotArmed(InsecureTransport)`.
5. global scope → `Armed(GlobalScope)`.
6. `Consent::Granted` → `Armed(WorkspaceConsent)`.
7. `Consent::Drifted` → interactive ? prompt : `NotArmed(ConsentDrifted)`.
8. `Consent::Absent` → interactive ? prompt : `NotArmed(NoTtyToAsk)`.

Steps 4–8 need the artifact's pinned source; steps 1–3 do not. **Arming is
invocation-level, not per-artifact**: every hook in a workspace arms or none
do. `Interactivity` is carried over unchanged — stdin **and** stderr both
terminals, prompt on stderr (finding W5) — and the prompt names the
**workspace**, states the file it will write, and on drift names the new hooks.

Do not fold the transport gate back into the consent predicate. Keeping it
separate is what makes `consent()` pure and keeps the environment read
(`GRIM_INSECURE_REGISTRIES`) at the command boundary, where `policy.rs` already
puts it.

## Validation

- [x] Threat-model relationship recorded in
      [`arch-threat-model.md`](../../.claude/rules/arch-threat-model.md) under
      I2, with the residual named — so the exception is met before it is filed.
- [x] The kind-scoped exception to
      [`adr_artifact_trust_model.md`](./adr_artifact_trust_model.md) decision 2
      recorded there, with Principle 9 as its actual reason.
- [ ] Acceptance: the clone gate (T3), workspace-A-does-not-arm-workspace-B,
      drift naming the new entry, a version bump *not* re-gating, the
      write-seam allowlist asserted as "no file exists under
      `$GRIM_HOME/hooks/consent/`", global scope arming with no record, and
      `revoke` twice exiting 0 both times.
- [ ] Unit: the predicate table (absent · granted · drifted · a record for a
      *different* workspace under the same key · unknown `v` · truncated JSON ·
      unknown field), all eight arming steps in isolation, and the transport
      gate's loopback rows **moved, not rewritten**, from
      `only_a_loopback_host_is_exempt_from_the_insecure_rule_b3`. The
      acceptance registry is `localhost` and therefore exempt from the
      transport gate, so it needs a unit test — do not assume the suite covers
      it.

## Links

- [`adr_hooks_support.md`](./adr_hooks_support.md) — A2 (partially reversed
  here), Decision E point 4 (restored here), Key insight 6 (annotated here)
- [`adr_artifact_trust_model.md`](./adr_artifact_trust_model.md) — decision 1
  (the declaration gesture), decision 2 (the no-per-kind-gate rule, now
  carrying a scoped exception), decisions 3 and 4 (integrity is the mechanism)
- [`plan_hooks_artifact_kind.md`](../plans/plan_hooks_artifact_kind.md) —
  C-022 (superseded), C-023 (amended: the contract survives, its subject
  changes from registry to workspace)
- [`arch-threat-model.md`](../../.claude/rules/arch-threat-model.md) — T1, T3,
  N4, and invariants I1–I5
- [`direnv/direnv#83`](https://github.com/direnv/direnv/issues/83) — the
  content-only-trust defect this design's directory binding exists to avoid
- `~/dev/ocx/crates/ocx_lib/src/project/consent.rs` — the sibling
  implementation whose shape is adopted, `ocx shell allow|revoke`

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-08-28 | Michael Herwig + Claude (hex swarm) | Initial draft, **Accepted**. Reverses the registry half of `adr_hooks_support.md` A2 and restores Decision E point 4's directory binding, keeping A2's coarseness. Supersedes contract C-022 with its B4/B5/B7/W8/S2-2 amendments; amends C-023's subject. Records the I2 relationship, the T1 version residual, the exit-78 residual, and the never-garbage-collected residual. |
