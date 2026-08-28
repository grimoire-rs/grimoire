# ADR: Distribute lifecycle hooks as a `hook` artifact kind behind a `grim hook run` dispatcher

## Metadata

**Status:** Accepted
**Date:** 2026-08-14 (rewritten in place; first drafted 2026-06-03)
**Accepted:** 2026-08-16 by the maintainer, with the amendments in
§ "Amendments accepted 2026-08-16" and all three Open Questions resolved
**Deciders:** Architect (`/hex-architect`), maintainer
**Beads Issue:** N/A
**Related PRD:** N/A
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md`
      (Rust 2024, one binary, no new runtime dependency, no daemon)
**Domain Tags:** integration | security | api | packaging
**Supersedes:** N/A (this file replaces its own 2026-06-03 draft — see Changelog)
**Superseded By:** N/A

## Context

`product-context.md` names **hooks** as an in-scope artifact type. A hook is a
lifecycle event handler: a program the agent runs at a defined moment (before a
tool call, on session start, on stop). Grimoire today ships four passive kinds
(`Skill`, `Rule`, `Agent`, `Mcp`) plus the non-materializing `Bundle`. Hooks
break three assumptions baked into that model.

1. **A hook is code that executes automatically**, hundreds of times per
   session, at user privilege, delivered by a registry. `grim install` currently
   arms nothing; a hook kind changes grim's risk class outright.
2. **A hook is useless as a loose file.** It must be *registered* in a surface
   the client owns. Placement alone does not activate anything.
3. **The executable bit does not survive the registry round trip.** The packer
   hardcodes `header.set_mode(0o644)` for every tar entry with no parameter to
   vary it (`src/skill/skill_package.rs:435-443`); `DefaultMaterializer`
   unpacks with `std::fs::write` and never chmods
   (`src/install/materializer.rs:44-70`). `atomic_copy` deliberately *preserves*
   source permissions (`src/install/client_target.rs:583-588`), which is why the
   existing exec-bit test passes — it materializes from a local `0o755`
   directory. An artifact that travelled through OCI arrives `0o644`.

Three things changed since the 2026-06-03 draft, and each invalidates part of
it. The vendor landscape inverted: **15 of 17 grim clients now have a hook
mechanism; only Warp and Zed have none**
([`research_hooks_vendor_survey.md`](../research/research_hooks_vendor_survey.md)).
The "genuinely new machinery" that draft was built around — reversible
registration into a foreign config — **has since shipped** for a different
consumer, and its own source comment names hooks as the origin of the pattern:
`Vendor::sync_config` is documented verbatim as "the reversible
config-registration seam (hooks ADR pattern)" (`src/install/vendor.rs:368-379`),
with `opencode_config::sync_for_state` (`src/install/opencode_config.rs:168`) as
the working reference. And a third option that draft never considered — a
runtime dispatcher — turns out to dominate on this ADR's own criteria.

The draft's two durable contributions are carried forward unchanged: the
framing that **activation, not placement, is the feature**, and its security
analysis.

## Decision Drivers

- **Activation over placement.** Value lands only when the installed hook fires
  *and behaves the same* on every client it claims to support.
- **Security is the dominant NFR.** Arbitrary code execution at user privilege,
  on the agent's hot path, with a prompt-injection amplification path unique to
  AI hooks. Block-tier per `quality-core.md`.
- **Grim must own consent, because the clients will not.** On the majority of
  targets an installed hook simply runs — Cursor, Kiro, Antigravity, Amp, Goose,
  Cline and Kilo have no hook-specific consent step at all
  (`research_hooks_vendor_survey.md` § "Security posture per client").
- **Reversibility and idempotency** of every write into a config grim does not
  own, across install / update / uninstall / prune.
- **Principle 9.** `GrimoireLock` and `InstallState` are `deny_unknown_fields`;
  hook-free projects must keep byte-identical files and hashes.
- **The design must fit 15 clients while v1 ships 3** — the remaining twelve
  must be additive, not a redesign.
- **The hot path is real.** `PreToolUse` fires on every tool call. A design that
  is slow, or that fails the wrong direction, converts "hooks stopped working"
  into "my agent refuses to run any command".

## Industry Context & Research

**Research artifacts** (all 2026-08-14; do not re-derive):
[`research_hooks_trampoline.md`](../research/research_hooks_trampoline.md) —
grim-side findings F1–F13 and decisions D1–D10 ·
[`research_hooks_vendor_survey.md`](../research/research_hooks_vendor_survey.md)
— the 17-client matrix, with the full per-client reports under
[`hooks_vendor_reports/`](../research/hooks_vendor_reports/) ·
[`research_hooks_codex_surface.md`](../research/research_hooks_codex_surface.md)
· [`research_hooks_hotpath_cost.md`](../research/research_hooks_hotpath_cost.md)
· [`research_hooks_autoexec_supply_chain.md`](../research/research_hooks_autoexec_supply_chain.md).

**Key insight 1 — Claude Code's contract is the de facto standard, observed not
asserted.** Cursor ships an importer that reads `.claude/settings.json` and maps
PascalCase to its own camelCase; Copilot's VS Code hooks read
`.claude/settings.json` and are built to the same wire format; Gemini CLI
exports a `CLAUDE_PROJECT_DIR` alias verbatim "for compatibility"; Goose's own
PR #9304 admits following "precedent for adopting Claude Code's hook
conventions"; and **Codex's own internal Rust type is named
`ClaudeHooksEngine`**, with an open umbrella tracker titled *"Full Claude Code
Hook Parity (29+)"* (`openai/codex#21753`). Codex's generated output schema even
carries the comment *"Claude requires `reason` when `decision` is `block`; we
enforce that semantic rule during output parsing"*
(`hooks_vendor_reports/codex.md` §7). The canonical schema should therefore *be*
Claude's shape, not a neutral invention.

**Key insight 2 — the industry just declined to standardize hooks.** Agent
Plugins 1.0 (`agent-plugins.org` v1.0.0, published 2026-08-06 by GitHub, AWS,
Cursor, Microsoft, OpenAI, Vercel and Google) standardizes skills and MCP
packaging and **explicitly places hooks outside its portable core**. There is no
open spec to defer to; a portable hook layer is an uncontested gap.

**Key insight 3 — portability dies in the response contract, not the payload.**
Payloads are informational, so a superset is harmless. Responses are semantic
and diverge behaviourally: Claude, Cursor, Gemini, Goose, Junie and Droid fail
**open** on an unexpected non-zero exit, while Copilot's `preToolUse` fails
**closed** (`hooks_vendor_reports/copilot.md` §7). The same unchanged hook has
opposite behaviour on two clients when it crashes. Within Claude alone the
verdict field moves per event (`hookSpecificOutput.permissionDecision`,
top-level `decision`, `hookSpecificOutput.decision.behavior`) and
`hookSpecificOutput.hookEventName` must echo the firing event
(`hooks_vendor_reports/claude.md` §7). Cursor's `allow`/`ask` verdicts are
**not enforced at all** — staff-confirmed open bug.

**Key insight 4 — default-deny is the only control with a causal track record.**
Every ecosystem that shipped silent-execute-by-default (npm, RubyGems, PyPI,
Homebrew third-party taps, VS Code extensions, Cargo `build.rs`) has since eaten
a named incident traceable to that default, and the fix that changed outcomes
was **always a default flip** — pnpm 10.0.0 (2025-01-10, triggered by an rspack
cryptomining postinstall), npm v12, Homebrew 6.0 "Tap Trust" (2026-06-11) —
never an attestation layer added afterwards. npm provenance and mandatory 2FA
did not stop the 2025–26 Shai-Hulud worm, and the 2026 TanStack compromise
shipped **84 malicious versions carrying valid provenance attestations**
(`research_hooks_autoexec_supply_chain.md` §1–2). Cargo's stance is the
cautionary end state: build scripts are officially, explicitly unsandboxed "by
design", two documented exploitations later, and the sandboxing project goal was
discontinued.

**Key insight 5 — the closest structural analogue has the worst posture.**
`pre-commit` is literally "distribute hooks from a registry into a repo": it
pins by a force-pushable git tag, has **no** sandboxing and **no** trust model,
and its official docs contain no security discussion at all. No incident is
documented — an absence of evidence, not evidence of safety. This is the "what
not to imitate" case. Grim's OCI content-digest resolution is already strictly
stronger, and structurally immune to the mutable-identity bypass class that
defeated allow-lists in tj-actions (CVE-2025-30066, 23,000+ repos), trivy-action
(75 of 76 tags force-pushed) and Bun (`trustedDependencies` matched by name
only, CVE-2026-24910).

**Key insight 6 — the one validating precedent for grim's consent model is
Gemini CLI, not Claude Code.** Gemini fingerprints each hook by `name:command`
in `trusted_hooks.json` and re-prompts when the fingerprint changes. Claude's
mechanism is coarser (folder-level trust). Citing Claude as the precedent would
cite the wrong vendor. No vendor publishes incident-reduction metrics for this
control — describe it as best-available practice validated by one comparable
vendor, not as proven.

> ⛔ **ANNOTATED 2026-08-28 — this insight was correct for the model it was
> written against, and that model no longer exists.** It judged Claude's
> folder-level trust "coarser" **before A2 chose coarseness deliberately**.
> Once per-hook digest approval is off the table, coarseness stops being the
> defect in the precedent and becomes the property being adopted: grim now
> gates on the **folder** (the workspace), exactly as Claude does
> ([`adr_hook_workspace_consent.md`](./adr_hook_workspace_consent.md)). Gemini
> stays the correct precedent for the **content** axis — the one grim no longer
> walks, by A2 — and Claude's folder trust, together with `direnv`'s path key
> ([`direnv/direnv#83`](https://github.com/direnv/direnv/issues/83)), is the
> precedent for the axis grim now does. The insight's last sentence is
> unaffected: still best-available practice, still not proven.

## Considered Options

Weighted criteria. Weights reflect the Decision Drivers; scores are 1–5.

| Criterion | Weight |
|---|---|
| Activation fidelity — the hook fires *and behaves the same* per client | 5 |
| Security controllability — gated, revocable, one enforcement point | 5 |
| Portability — write once, install everywhere | 4 |
| Reversible / idempotent foreign-config writes | 4 |
| New machinery cost (inverse — 5 = cheap) | 3 |
| Hot-path latency risk (inverse — 5 = none) | 2 |
| Reversibility of *this* decision | 3 |
| Forward fit to the full 15-client roster | 3 |

### Option 1: Passive script drop, no registration

Place the payload at `<client>/hooks/<name>` and stop; the user wires it in.

| Pros | Cons |
|------|------|
| Tiny change; reuses materializer, lock and state as-is | **Does not activate the hook** — leaves the hard part to the user |
| No foreign-config mutation, so no clobber risk | Uninstall/prune cannot deregister a user-authored entry |
| grim never arms code execution | Still ships executable code with no gate, and F1 means the file is not even executable |

### Option 2: Native per-target hook + reversible registration (the 2026-06-03 choice)

The publisher ships one variant per client; grim materializes the payload with
the exec bit and splices the **native** registration entry per client. No
canonical translation.

| Pros | Cons |
|------|------|
| Real activation; smallest conceptual leap | Publisher ships N variants — no write-once |
| Native fidelity by construction | **One config entry per hook**, and no vendor but Antigravity has an identity field: idempotent ownership of an array element needs a grim-invented `command`-substring convention (`hooks_vendor_reports/claude.md` §10) |
| — | Codex and Gemini hash the command string ⇒ **a human trust prompt on every hook add or update**, and a headless CI stall |
| — | Needs the nested object-in-array splice (F12) *and* per-vendor writers |

### Option 3: Canonical manifest + install-time translation only

Define a canonical hook manifest; translate event name and registration shape
per client at install time. No runtime component.

| Pros | Cons |
|------|------|
| Write-once *registration* | Write-once **only for registration** — the payload still faces N payload dialects and N response dialects, so the portability promise is half-delivered |
| No new hot-path dependency | The fail-open/fail-closed divergence leaks straight through to the user (Insight 3) |
| — | Inherits Option 2's array-identity and trust-prompt problems verbatim |
| — | Per-vendor matcher dialects (JS regex vs glob vs category events) become a translation problem grim must get right |

### Option 4: Canonical manifest + runtime `grim hook run` dispatcher — **chosen**

One managed dispatcher entry per `(client, event)`, whose command is a
grim-owned launcher invoking `grim hook run --client <c> --event <E>`. Grim owns
matching, ordering, aggregation, failure policy, and response projection, and
spawns the payload itself.

| Pros | Cons |
|------|------|
| One canonical payload dialect and one canonical response dialect for hook authors | **grim becomes a runtime dependency of the agent loop**, on the hot path of every tool call, in a process the user did not launch |
| The managed unit is one array element whose identity *is* its own command string — exactly `json_splice::upsert_array_element`'s shipped contract (`src/install/json_splice.rs:184`). Installing hook #2 touches **no** vendor config *when it reuses an existing matcher on an existing event* (qualified per decision J — a new matcher costs one vendor-config write) | Needs a dispatch table, a trampoline subcommand, a launcher, and (for Claude) the nested-array splice anyway |
| **A policy enforcement point.** An unapproved or digest-drifted hook is refused *at fire time*. A plain config entry can never be revoked; a trampolined one can | The no-match path must be genuinely cheap, and that is a measured property, not a designed one |
| The trust key is stable: `grim hook run --client codex --event PreToolUse` is unchanged when hooks are added, updated or removed ⇒ trust cleared **once** | A grim panic becomes an agent-visible event |
| Grim decides the failure policy and emits the vendor response that *implements* it, instead of letting each vendor's default leak | |
| Sidesteps F1 entirely — grim launches the payload, so no client ever execs a registry-delivered file | |
| Ordering becomes deterministic: mutators run serially in declaration order, a capability **no vendor has** | |

### Option 5: Defer hooks until the accepted plugin render mode lands

`adr_render_layout_stability.md` §2 (**Accepted**) makes plugin rendering an
opt-in post-1.0 render mode on the Claude vendor, and the same plugin manifest
is accepted at `.codex-plugin/`, `.claude-plugin/` and `.cursor-plugin/`
(`codex-rs/exec-server-protocol/src/protocol.rs:46`, via
`research_hooks_codex_surface.md` §1). Claude plugins carry their own
`hooks/hooks.json`. Genuinely live: one carrier could serve three clients.

| Pros | Cons |
|------|------|
| Grim owns a fully namespaced directory — the cleanest reversibility story available | Blocks the feature on **unimplemented, unscheduled, post-1.0 work** |
| Dictionary-keyed identity instead of positional | Serves 3 of 15 hook-capable clients; the other 12 have no plugin surface |
| Cross-vendor manifest compatibility is real and deliberate | **Buys zero trust-friction relief**: `append_plugin_hook_sources` passes `is_managed: false` for every plugin-sourced hook, unconditionally (`codex-rs/hooks/src/engine/discovery.rs:238-283`) |
| — | Codex plugin hooks still need **two** `config.toml` table registrations plus a probable Codex-owned cache copy |

### Scoring

| Criterion (weight) | Opt 1 | Opt 2 | Opt 3 | **Opt 4** | Opt 5 |
|---|---|---|---|---|---|
| Activation fidelity (5) | 1 | 5 | 4 | **5** | 3 |
| Security controllability (5) | 3 | 3 | 3 | **5** | 4 |
| Portability (4) | 2 | 1 | 3 | **5** | 2 |
| Reversible foreign-config writes (4) | 5 | 3 | 3 | **5** | 5 |
| New machinery cost (3) | 5 | 2 | 2 | **3** | 4 |
| Hot-path latency risk (2) | 5 | 5 | 5 | **2** | 5 |
| Decision reversibility (3) | 5 | 3 | 3 | **4** | 2 |
| Forward fit to 15 clients (3) | 2 | 2 | 3 | **5** | 3 |
| **Weighted total (max 145)** | 94 | 87 | 93 | **130** | 100 |

Option 4 wins by 30 points on criteria written before it was scored. Its single
weak score is hot-path latency — the one property that must be *measured*, not
argued (see Out of Scope). Option 5 places second almost entirely on
reversibility and machinery cost, and loses on portability plus the
unimplemented dependency; its real content is a forward-compat obligation, not
an alternative, and is discharged in "Interaction with plugin render mode".

## Decision Outcome

**Chosen: Option 4** — `ArtifactKind::Hook`, a directory artifact whose
manifest is `hook.toml`, registered per `(client, event)` as one managed
dispatcher entry invoking a grim-owned launcher, with grim normalizing the
payload, owning matching and ordering, and projecting one canonical response
onto each vendor's per-event response shape. **Off by default** behind
`[options.experimental] hooks = false`.

Frozen inputs designed against, not re-litigated: dispatcher entry per
`(client, event)` (D1) · stdin canonical envelope carrying the verbatim vendor
payload under `raw`, flat scalars in env, `payload = "file"` opt-in (D2) ·
canonical == Claude's schema and PascalCase names, canonical breadth exactly
four events, everything else native via `<vendor>.event` (D3) · tiers
`observer` + `gatekeeper` + `mutator`, with mutator in v1 by owner decision and
all nine controls mandatory (D4) · ~~per-hook digest-pinned approval with
re-prompt on change and a CI escape (D5)~~ — **D5 is reversed at acceptance; see
§ "Amendments accepted 2026-08-16" A2** · v1 clients claude + codex + copilot,
Copilot cloud-agent out, no codegen client (D7) · Codex: grim owns
`hooks.json`, plugin route is the fallback, and grim **never** writes a vendor's
trust record (D7a) · `grim hook run` is a subcommand of the one binary (D9) ·
the projection rule · the TOML authoring syntax.

### Amendments accepted 2026-08-16

The plan built on this ADR ([`plan_hooks_artifact_kind.md`](../plans/plan_hooks_artifact_kind.md))
ran a five-perspective review panel and one owner-authorised re-validation, and
the maintainer took two further decisions during it. Those outcomes are folded in
here at acceptance. **Where this section and the body disagree, this section
wins**; the body text is left standing so the reasoning that produced the
original position stays legible.

**A1 — Project scope is Claude-only. Codex and Copilot ship global-only.** The
owner directed at the planning gate that project scope be widened to all three v1
clients via a portable launcher reference. The panel established, on documented
evidence, that this is not achievable in v1: a committed registration makes the
*executed binary path* environment-derived (`${GRIM_HOME:-$HOME/.grimoire}`), and
`.envrc`, `.mise.toml` and devcontainer `containerEnv` are ordinary repository
files — clone-to-RCE on one tool call, before any grim control runs. Copilot's
`{{project_dir}}` solves `--root` but not the launcher path; Codex interpolates
nothing at all. Decision I stands as originally written. **This restores the
recommendation the owner overrode at the gate, on evidence produced after it**,
and the owner accepted the reduction on 2026-08-16. Widening stays additive under
Principle 9 and requires a client offering a *non-environment-derived* launcher
reference. Invariant **I1** in
[`arch-threat-model.md`](../../.claude/rules/arch-threat-model.md) generalises
this beyond hooks.

**A2 — D5 is reversed: trust is registry-scoped, not per-hook.** Per-hook
digest-pinned approval with re-prompt on digest change is withdrawn. Owner
rationale, verbatim in intent: *no one wants to review every hook that is
installed* — which is the re-prompt-habituation failure this ADR's own risk list
names. Trust moves to the **registry**: a hook resolved from a registry with an
explicit `[[registries]]` entry arms with **no prompt**, because configuring the
registry *is* the trust act (Homebrew 6.0 "Tap Trust" and Docker's
registry-scoped trust are the precedent). An unconfigured registry prompts once;
accepting writes `trust_hooks = true` into global config, so trust is visible in
`git diff` and `grim config list` and revocable by editing config.
`trust_hooks = false` opts a registry out. `GRIM_ALLOW_HOOKS=1` alone arms
nothing. This **deletes** `hook_approvals.json`, the append-only hash chain and
the per-artifact approval key.

> ### ⛔ A2 IS PARTIALLY REVERSED 2026-08-28 — the registry half is gone; the coarseness stays
>
> [`adr_hook_workspace_consent.md`](./adr_hook_workspace_consent.md) replaces
> registry-scoped trust with **per-workspace consent**. Everything above that
> names `[[registries]]`, `trust_hooks`, "configuring the registry is the trust
> act", or the Homebrew/Docker tap precedent is **historical**. There is no
> `trust_hooks` field, no dotted `registry.<alias>.trust_hooks` key, and no
> config file of any scope is an input to an arming decision.
>
> **What survives, unchanged and deliberately:** A2's *coarseness*. No per-hook
> prompt, no digest key, no approval store, no hash chain — the owner's
> rationale (*no one wants to review every hook*) stands in full, and the new
> design concedes it. That argument was always about the **digest** half of
> Decision E point 4's key; A2 deleted the **scope root** half with it as
> collateral, and it is that half — the directory binding, with
> [`direnv/direnv#83`](https://github.com/direnv/direnv/issues/83) as its named
> precedent — that is now restored. The two axes are orthogonal: deleting both
> left **T3** uncovered by anything.
>
> Contract **C-022** in the plan is superseded outright, with its B4 / B5 / B7 /
> W8 / S2-2 amendments; **C-023**'s non-interactive contract survives with its
> subject changed from registry to workspace. `GRIM_ALLOW_HOOKS` remains
> permanently forbidden (owner decision 2026-08-17, `24a14bb`), and there is
> likewise no `GRIM_HOOK_CONSENT` and no environment form of the record's path.

**A3 — The exec-time digest re-check is dropped.** The runtime hashes nothing on
either path. The four resolution-identity CVEs it was credited with are closed at
*resolution* by the digest-pinned lock; post-install payload tampering is **N2**
in the threat model — explicitly out of scope. One in-scope residual remains — a
trusted hook rewriting a *sibling* hook's payload — and is covered as
**tamper-evidence** by `ClientOutput::content_hash` at the next `grim status` or
install, **not** as prevention (invariant I5). This qualifies invariant I2, whose
"re-checked at the moment of use" clause is a general aspiration that this
decision deliberately trades away for hooks; the trade is recorded rather than
hidden. Threat row 10 accordingly trades **consent for visibility**: a trusted
registry can add a hook to a bundle and it arms on next install, surfaced in the
`grim add` / `grim update` report and `grim status` rather than gated by a
prompt.

**A4 — The experimental flag's environment variable is `GRIM_EXPERIMENTAL_HOOKS`**,
deliberately distinct from `GRIM_ALLOW_HOOKS` (the CI consent escape). One turns
the feature on; the other says "arm without asking". Conflating them would make a
CI escape silently enable an experimental subsystem.

**A5 — The launcher `exec`s a recorded absolute grim path**, with a `$PATH`
lookup only as fallback. A `$PATH` lookup inside the trusted shim reintroduces
exactly the dependency Decision D rejected.

The full amended contract set — **C-015…C-026**, of which C-022 carries A2 and
C-009 carries A3 — plus threat rows 13/14 and the re-controls on rows 1/2/4/10
live in the plan's Component Contracts section and are normative there rather
than restated here, to keep one source of truth per contract.

### The decisions this ADR adds (A–Q)

Decisions **I–Q** were added after the design panel, which returned 14 Block
findings across five perspectives. Ten of those Blocks were one coupled problem
in three clusters — where the runtime is allowed to read, what travels with a
repository, and how the tiers compose — and the owner settled each. Panel
provenance is named inline so a later reader can see which decisions are original
and which are repairs.


**A. `Hook` support is opt-in per vendor through a new seam, not through
`kind_support`'s default.** `Vendor::kind_support` defaults to
`KindSupport::Native` (`src/install/vendor.rs:196-198`) and every vendor impl
closes its match with a wildcard — `vendor_warp.rs:58-63` is
`ArtifactKind::Rule | Agent | Mcp => Declined, _ => Native`, and
`vendor_codex.rs:104-112` the same shape. Adding `ArtifactKind::Hook` therefore
makes **all 17 vendors silently claim native hook support**, including Warp and
Zed which have no hook surface at all, with **no compile error**. Fourteen must
decline.

Decision: add `fn hook_surface(&self) -> Option<HookSurface> { None }` to the
`Vendor` trait and resolve `ArtifactKind::Hook` **exclusively** through it.
`installer::client_supports_kind` special-cases `Hook` to
`hook_surface().is_some()` and never consults `kind_support` for it. A vendor
that says nothing declines, so a forgotten vendor fails **safe**.

Rationale for the inversion rather than 14 explicit `Declined` arms: the
existing default was calibrated for passive text kinds, where a wrong guess
writes a stray inert file; for hooks a wrong claim either registers a dispatcher
into a client with no hook surface, or silently reports a guardrail as installed
on a client that cannot host it — a security failure, not a fidelity one. The
existing convention is still honoured where it is load-bearing: a pinned-set
test in the `SCOPE_GAPS` / `POOL_CAPABLE_VENDORS` style (`src/install/vendor.rs:163`,
`:621`) asserts the exact hook-capable set and its per-tier verdicts, so adding
a client is a deliberate two-line change with a failing test until the docs
match. Per-hook capability resolution is needed anyway (tier × event × client,
see C-005), so `kind_support(Hook)` was never the right primitive.

**B. The payload materializes once per scope, not once per client.** Project:
`<workspace>/.grimoire/hooks/<name>/`. Global: `$GRIM_HOME/hooks/<name>/`. Both
reuse anchors that already exist — `PathAnchor::Workspace` and
`PathAnchor::GrimHome` (`src/install/path_anchor.rs:161-186`) — so no new anchor
variant is needed, only `candidate_anchors` arms.

Only grim reads the payload, so N copies would be pure duplication. **The
payload is the recorded output; the registration is not.** Each participating
client gets one `ClientOutput` whose `target` is the **shared payload
directory** — N outputs onto 1 destination, which is exactly the shape
`prune::shared_by_surviving_sibling` (`src/install/prune.rs:660`) was built for,
releasing the directory only when the last client drops it.
`ClientOutput::content_hash` (`src/install/install_state.rs:114`) then gives
payload drift detection for free.

Registrations are **derived, never recorded** — see Decision L. That resolves the
review finding that the refcount "can never fire across hook clients": there is
nothing per-client to refcount, because the only recorded destination is shared
by construction.

`.grimoire/.gitignore` consequence: grim writes a self-managed
`.grimoire/.gitignore` containing `*` on first project-scope mutation
(`src/install/install_state.rs:497-538`), so a project-scope payload **never
travels with the repository**. Under Decision I the registration does not travel
either, which removes the "cloned repo carries a live dispatcher entry with no
payload" case as a *design* concern. It survives only as a **robustness**
requirement, because grim must never *rely* on a gitignore —
`src/install/path_anchor.rs:285` already records that a `state.json` can arrive
with a `git clone`, since the gitignore exists only after grim has run. So the
dispatcher still treats "registered but unknown, unapproved, or absent" as
**fail-open, exit 0, log once** (C-007); it is now a belt-and-braces path rather
than a first-class one.

**C. The `docs/src/clients.md` Hook cell means the best tier the client can
host, and it is computed, not written.** The parity test
(`src/install/client_target.rs:748-784`) iterates exactly `[Skill, Rule, Agent]`
by index and special-cases `cells[3]` as a *boolean* MCP column derived from
`mcp_config_path().is_some()`, so a fifth column is a design decision, not a
mechanical add. Cell semantics, consistent with the page's own legend
(`docs/src/clients.md:21-25`, where `◐` already means "supported with a
documented limitation"):

| Glyph | Meaning for the Hook column |
|---|---|
| `✓` | All four canonical events are hostable **and**, for each event, every tier **valid at that event** is expressible in this client's own response schema |
| `◐` | A hook surface exists and grim registers into it, but at least one canonical event or tier is unexpressible for **this client** |
| `✗` | grim installs no hook for this client |

**The cell is about the client's surface, not about grim's uniform policy.** The
review found the original wording made `✓` unsatisfiable — it required "every
tier including `mutator`", but Decision K declines `mutator` for
shell-command-string tools on *every* client, so no client could ever earn `✓`
and `hook_matrix_cell` would return `◐` forever. A cross-client policy is not a
per-client limitation and must not be encoded per row: it belongs in the page's
Known-gaps prose once, not in eleven cells.

**"Valid at that event" is load-bearing and was missing from the first
correction.** Without it the conjunct still quantified over tiers the event never
admits: `gatekeeper` on `SessionStart` is unexpressible in **both** claude's and
codex's response schema per this ADR's own C-004 table (`⊘ (cannot block)`), so
`hook_matrix_cell` would have returned `◐` for every client forever — the original
Block reached by a second route. Decision F now rejects `gatekeeper` on an event
that admits no verdict at `grim build`, which makes the tier set per-event
well-defined and the conjunct satisfiable.

**Whether a given client reaches `✓` is computed from C-004, not asserted here.**
The first correction claimed "claude and codex earn `✓`"; that was an assertion
about a table rather than a reading of it, and the re-validation showed it false as
written. `hook_matrix_cell` is the single source, the parity test compares the doc
against that one function, and copilot is `◐` at minimum while its mutator field
stays contested (Open Question 1).

The cell is a pure function `hook_matrix_cell(client)` over the vendor's
declared hook capability set, and the parity test asserts the doc against that
one function — a second special case beside MCP's, which is the precedent. Per
tier and per event detail is not compressible into a glyph and does not belong
there: it lives in the page's Known-gaps prose and in `grim status` /
`grim describe` output, which is where a user needs it operationally.

Required honesty in the Known-gaps prose: `✗` must distinguish **no upstream
surface** (warp, zed) from **upstream surface exists, grim support pending**
(the other 12). The legend's `✗` currently reads "no ownable surface", which
would be a false statement about Cursor or Gemini. Every `✗ → ◐/✓` move is
additive under Principle 9.

**D. The registration names a fixed-path, grim-generated launcher.** Three
constraints are in tension. Codex and Gemini gate a hook on a hash of its
command content — Codex's `hook_hash` covers `(event_name, matcher, normalized
handler)` and re-prompts through a `/hooks` TUI on change
(`research_hooks_codex_surface.md` §2), Gemini fingerprints `name:command` — so
the string must be **byte-stable across grim upgrades**, ruling out any
versioned or relocatable absolute path. A bare `grim` on `$PATH` is byte-stable
but may not resolve: all three v1 clients inherit the launching process's
environment, and a GUI-launched client on macOS gets a minimal launchd `$PATH`.
And exec-form argv (Claude's `args: []`, which removes shell-quoting risk
entirely) cannot carry a `command -v grim || exit 0` fail-open guard.

Decision: grim generates a launcher at a fixed path under its own home —
`$GRIM_HOME/hooks/bin/grim-hook` — and registers **that absolute path** in
exec form, with `["hook", "run", "--client", "<c>", "--event", "<E>",
"--root", "<abs|global>"]` as argv (decision P — the earlier `--scope <s>` form
could not name *which* workspace, and is superseded). This is simultaneously
absolute (a minimal `$PATH`
cannot break it), byte-stable across grim upgrades (`$GRIM_HOME` does not move
when the binary does), grim-owned (grim generates it locally, so it may `chmod`
freely and self-heals on re-install), and shell-free at the client boundary.
Per D9 it is a generated shim, never a second shipped binary: its body resolves
`grim` and `exec`s `grim hook run "$@"`.

**When the binary is absent the launcher exits 0 and writes nothing.** That is
the whole reason it exists: `grim` uninstalled, moved off `$PATH`, or mid-upgrade
degrades to "hooks silently off", never to "the agent is blocked". Copilot's
`preToolUse` fails **closed** on a non-zero exit
(`hooks_vendor_reports/copilot.md` §7), so an unguarded missing-binary case
would deny every tool call on that client. When the launcher *itself* is missing
— a client config restored from backup, or a `$GRIM_HOME` wipe — the client's own
failure handling applies and grim cannot intervene; the next `grim install`
regenerates it, and `grim status` reports the gap. Windows launcher form is
Open Question 2.

> ### ⚠ DECISION E IS PARTLY WITHDRAWN — read before implementing anything below
>
> **Amendments A2 and A3 delete the approval store.** There is no
> `hook_approvals.json`, no append-only hash chain, and no
> `(artifact content digest, scope root)` key. Trust is a **`[[registries]]`
> config fact** (contract C-022 in the plan); the runtime **hashes nothing**
> (C-009 as rewritten). Contract C-024 is withdrawn outright.
>
> **What survives from E:** the framing in its first paragraph — a hook at equal
> privilege can rewrite any file grim can, so "the file is ours" is not a
> control — and the consequence that **arming is moved out of the runtime path
> entirely**. That framing is now invariant **I5** (tamper-evidence, not
> tamper-resistance) in
> [`arch-threat-model.md`](../../.claude/rules/arch-threat-model.md), and the
> attacker it describes is **N2** — out of scope by decision.
>
> Everything below that names a store, a chain, or a per-artifact key is
> **historical**. Implement C-022 and C-009 from the plan, not this section.
>
> **⛔ UPDATED 2026-08-28 — point 4's *directory* half is restored, and C-022 is
> itself superseded.** This banner's "everything below is historical" was too
> wide for point 4. Its key had two halves, and A2 argued against only one of
> them: the **artifact content digest** stays deleted, but the **scope root** —
> approval bound to an absolute workspace directory, with
> [`direnv/direnv#83`](https://github.com/direnv/direnv/issues/83) as its
> precedent — is live again as the workspace consent record
> ([`adr_hook_workspace_consent.md`](./adr_hook_workspace_consent.md)). Point
> 4's closing sentence — *"Approving a hook in a scratch project therefore does
> not arm it in a production repo"* — is once more a statement of shipped
> behaviour. Do **not** implement C-022; implement the consent ADR. Points 1–3
> stay historical: there is still no store, no chain, and no per-artifact key.

**E. Grim's approval store cannot be protected from a hook at equal privilege —
so arming is moved out of the runtime path entirely.** Hooks run at user
privilege and can write any file grim can. "The file is ours" is not a control.
PromptArmor's *Hijacking Claude Code via Injected Marketplace Plugins*
(2025-10-16) is the exact attack: a malicious plugin shipped a
`UserPromptSubmit` **hook** that overwrote Claude's own `settings.local.json`
permissions, after which a separate injection payload drove a now-pre-approved
`curl` exfiltration. Four controls, in the order they matter:

1. **The runtime path reads only a derived artifact, and no edit to that
   artifact arms anything durably.** `grim hook run` consults the dispatch
   table (C-006) and nothing else.

   **The unit of wholesale regeneration is the root key, not the file.** Decision
   P puts every scope's entries in one machine-local table, and a project-scope
   command "operates on exactly one scope" — so regenerating the *whole file*
   would delete every other workspace's entries and silently disarm unrelated
   repositories. Instead `sync_config` replaces `entries["<abs root>"]` **atomically
   and wholesale for that key**, leaving sibling keys untouched. Within a key
   nothing is ever patched incrementally, which is what the tamper-revert property
   needs.

   Consequently the revert guarantee is scoped and must be stated that way: a hook
   that forges an entry under a workspace's key has it reverted by **the next
   mutating grim command in that workspace** — not by any mutating command
   anywhere. A hook that forges an *approval* record arms nothing at all, because
   the runtime never reads the approval store.
2. **Two-way agreement at exec time; the third leg is checked at convergence,
   not at runtime.** The original wording required the runtime to compare the
   dispatch table against install state — reads C-007 forbids, which the review
   flagged as a self-contradiction. Corrected: `sync_config` verifies
   table-vs-state-vs-`grimoire.lock` **when it writes the table**, and bakes the
   resulting digest into each entry. The runtime then performs exactly one
   comparison — **on-disk payload digest vs the digest in its dispatch entry** —
   and only on the matched branch, immediately before `exec` (C-009). The
   no-match path hashes nothing.

   Self-approval therefore still requires forging three files, one of which
   (`grimoire.lock`) shows up in `git diff` and in `grim status` as drift — but
   the forgery is caught by the next mutating grim command rather than by the
   hot path. This is tamper-evidence and non-arming, not prevention, and it is
   stated as such.
3. **Approvals are global-scope only, hash-chained, and fail closed.** The store
   is `$GRIM_HOME/state/hook_approvals.json` — never project state, never
   `<workspace>/.grimoire/state.json`. This closes the committed-approval path
   outright (threat row 9) rather than leaning on the gitignore. Records are
   append-only, each hashing its predecessor; a broken chain or a mismatched
   record makes grim refuse **all** hook execution until re-approval. Refusing a
   hook is fail-open with respect to the tool call: the tool proceeds, the hook
   does not run, grim says so loudly.
4. **Approval is bound to the tuple, not the content — and the tuple names a
   directory, not a scope kind.** `direnv` shipped content-only trust and had to
   fix it to path + content, because an approved `.envrc` copied into a hostile
   directory executed (`direnv/direnv#83`). The original key here was
   `(scope, artifact name, digest)`, where `scope` was `Project | Global` — which
   does **not** apply that lesson: every project shares the value `Project`, so
   one approval covered every workspace on the machine. Corrected key:

   > **`(artifact content digest, scope root)`** — the absolute resolved
   > workspace root for a project-scope approval, and the global marker for a
   > global one.

   Approving a hook in a scratch project therefore does not arm it in a
   production repo; the same bytes prompt again in a new workspace, which is the
   `direnv` behaviour the ADR claims to follow. The artifact name is dropped from
   the key as redundant — it is inside the digest.

Residual risk, stated plainly and handed to `/security-auditor`: a hook that
runs *while grim is idle* can corrupt any of these files; what it cannot do is
cause its own successor to run without a mutating grim command whose inputs are
version-controlled.

**F. `hook.toml` — the manifest.** A `Hook` is a **directory artifact**
(`is_dir_artifact() == true`): descriptor plus payload files, the shape
`adr_multifile_rules.md` already blessed for "index plus sibling directory
folded into one integrity hash and removed together". The manifest is
`hook.toml` at the artifact root.

```toml
schema      = 1                 # required; manifest + envelope contract version
name        = "shell-guard"     # required; must equal the directory stem (agent-kind precedent)
description = "Refuse curl-pipe-to-shell in Bash tool calls"   # required; catalog-facing

[[hooks]]
id      = "deny-curl-pipe-sh"   # required; unique within the artifact
event   = "PreToolUse"          # canonical event; omit only when a <vendor>.event stands alone
tier    = "gatekeeper"          # required: observer | gatekeeper | mutator
matcher = "Bash"                # optional; grim's own dialect (exact name or glob, never regex)
argv    = ["sh", "${GRIM_HOOK_DIR}/guard.sh"]   # exactly one of argv | command
timeout = 30                    # optional, seconds, default 30 — grim enforces it, not the vendor
payload = "stdin"               # optional: stdin | file, default stdin
# `policy` is RESERVED, unparsed in v1 — see the reservation note below
# `fail` is deliberately ABSENT in v1 — see the field rules below
cursor.event   = "beforeShellExecution"    # per-vendor override; parses to hooks[0].cursor.event
claude.timeout = 60                        # natively an integer — no FieldType::Json workaround
```

`[[hooks]]` array-of-tables **is** right: a pre/post pair sharing one payload
tree is the common case, and one artifact per hook would multiply install and
approval friction for no gain. Because the payload tree is shared, a per-entry
digest would be byte-identical to the artifact digest anyway — so **one approval
covers every `[[hooks]]` entry the manifest declares, keyed on the artifact
digest with the manifest inside it**. Adding an entry, changing a `command`, or
raising a `tier` changes the digest and re-prompts, which is exactly D5's
intent.

Field rules, all enforced at `grim build` (exit 65), never at runtime:

- Exactly one of `argv` or `command`. `argv` is the documented preferred form —
  no shell, no quoting. `command` is a single string grim hands to the platform
  shell and is documented as the lesser form.
- `tier = "mutator"` is valid **only** on `PreToolUse` and on native events that
  fire at the same moment. Nothing later has an input left to rewrite, and this
  one rule removes most of the mutator surface by construction. Decision K adds
  the second, sharper restriction: `mutator` is `Declined` for tools whose input
  is a shell-command string.
- **`tier = "gatekeeper"` is valid only on an event that admits a verdict.** No v1
  client can block at `SessionStart` (C-004 records `⊘` for claude and codex), so a
  `gatekeeper` declared there is a manifest error at `grim build`, not a per-client
  decline — the same shape as the `mutator` rule above. This is what makes the
  per-event tier set well-defined, and therefore Decision C's `✓` satisfiable.
  Where an event admits a verdict on *some* clients and not others, the tier is
  valid in the manifest and `Declined` per client, which is the normal path.
- **There is no `fail` key in v1.** It was specified as `open | closed` and the
  review showed it unimplementable as written: Decision G routes timeout and
  spawn failure to exit 0, so a hook declaring `fail = "closed"` would not
  actually fail closed. Rather than ship a control that lies, grim **always fails
  open** and says so — consistent with hooks being defence-in-depth rather than a
  security boundary (Decision M), and consistent with what grim's own published
  `ai-config-authoring` skill already tells authors. Adding `fail` later, once a
  real enforcement point exists, is additive under Principle 9.
- Every client name in `ClientTarget::ALL` is a **reserved key** inside a
  `[[hooks]]` table; using one for anything but a vendor override fails
  `grim build`. `render::reserves_namespace` already maintains this closed list.
- Documented dialect is the **TOML 1.0-compatible subset**: unquoted dotted
  keys and single-line inline tables. Grim's parser accepts multi-line inline
  tables and trailing commas (its `toml`/`toml_edit` are `+spec-1.1.0`), and may
  keep doing so — but `hook.toml` is a published format read by third-party
  tooling (`taplo`, CI validators, Python/Go catalog scripts), and a stock TOML
  1.0 parser hard-rejects those forms. Liberal in what grim accepts,
  conservative in what it documents and emits.

**G. Failure policy: the launcher never signals failure through its exit code.**
Grim's internal errors — dispatch table absent or unparsable, unknown hook id,
missing payload, digest mismatch, payload spawn failure, payload timeout — all
resolve to **exit 0 with no stdout and one log line**. A non-zero exit is emitted
*only* when a `gatekeeper` deliberately denies, and then per that vendor's
blocking convention. This is what makes grim, rather than each vendor's default,
the owner of the failure direction, and it is the only reason a Copilot
`preToolUse` registration is safe to ship.

**"Feature flag off" is deliberately absent from that list.** The original wording
included it, and the review showed the runtime cannot observe it: reading
`[options.experimental] hooks` needs a config parse at a resolved scope, which
C-007 forbids and a test pins. The flag is instead expressed **structurally** —
see Decision N — so "off" reaches the runtime as *an empty dispatch table*, which
it already handles as no-match, exit 0. Every condition named above is one the
runtime can actually observe by opening a single file.

**H. Interaction with plugin render mode — the two accepted directions
compose.** `adr_render_layout_stability.md` §2 (**Accepted**) makes plugin
rendering an opt-in post-1.0 render mode **on the existing Claude vendor**, not
a new `ClientTarget`, with `[options] render = "files" | "plugin"` already
reserved as a paper key in §4; a Claude plugin carries its own
`hooks/hooks.json`. The rule:

> **Exactly one dispatcher registration exists per
> `(client, event, scope, matcher)`, and which surface carries it is a function of
> the render mode.**

**`matcher` is part of that key because of Decision J.** Pushing the declared
matcher into the vendor's own field means two hooks with different matchers on one
event need two vendor entries — on Claude the matcher lives at
`hooks.PreToolUse[i].matcher` with the command at `.hooks[j]`, so N distinct
matchers is N groups. The earlier key omitted `matcher` and was therefore
falsified by J on claude and copilot; the pinned test keys on the four-tuple.

Two claims elsewhere in this record inherit the same correction: Option 4's pro
row and the NFR scalability line both said "installing hook #2 touches **no**
vendor config". Accurate form: **no vendor config when hook #2 reuses an existing
matcher on an existing event**; a new matcher costs one vendor-config write, and
on Claude that write is the nested object-in-array splice. J's hot-path argument
is unaffected — it trades a vendor-config write at install time for no process
spawn at run time, which is the trade it was chosen for.

In `files` mode the Claude registration is the `settings.json` splice. In
`plugin` mode it is an entry in the plugin's own `hooks/hooks.json` — a file
grim wholly owns, so the shape degenerates to `OwnFile` and **no splice is
needed at all**. Switching modes is a layout move, which §1 already places
outside the 1.0 semver contract and which already requires an automatic
migration with an old-path reaper: the same `sync_config` pass that writes the
plugin file removes the `settings.json` entry, in one convergence.

This must be an invariant with a test, not a convention, because Codex and
Copilot **union** hook sources rather than override — Codex loads both
`hooks.json` and inline `[hooks]` and only *warns*
(`codex-rs/hooks/src/engine/discovery.rs:144-172`), and Copilot's docs state
"all hook entries from all sources are run". A double registration therefore
double-fires, running every matched hook twice and, for a mutator, threading two
independent pipelines over the same tool call. Two secondary notes: the dispatch
table, the launcher, the envelope and the response projector are all
render-mode-independent — only the registration *writer* changes, which is
precisely why the `OwnFile` / `SpliceConfig` / `CodegenModule` install-shape
abstraction must exist in v1. And because the same manifest is accepted at
`.codex-plugin/`, `.claude-plugin/` and `.cursor-plugin/`, plugin mode may later
become one carrier for three clients — a reason to keep v1's registration layer
thin behind one seam, never a reason to build plugin mode now.

**I. The registration is machine-local, because it is materialized output.** The
panel found the same Block from two directions: a project-scope registration
commits one developer's absolute `$GRIM_HOME` launcher path, and on a teammate's
clone or in Copilot's cloud agent — which reads `.github/hooks/*.json` from the
**default branch** — Copilot's fail-closed `preToolUse` then denies **every tool
call**. Declaring the cloud agent out of scope does not opt it out: the
registration reaches it by construction the moment the branch merges.

The repair is not to guard the path but to stop committing it. A registration is
**materialized output, regenerable from `grimoire.lock` by `grim install`** —
the same property every other kind has. The lock travels; the rendered artifact
need not. So grim registers into each client's **machine-local** surface:

| Client · scope | Machine-local registration surface | Install shape |
|---|---|---|
| claude · project | `.claude/settings.local.json` — a first-class hook scope, and **Claude Code gitignores it itself** when it writes there (`hooks_vendor_reports/claude.md:73`, `:251`) | `SpliceConfig` |
| claude · global | `~/.claude/settings.json` (`$CLAUDE_CONFIG_DIR` honoured) | `SpliceConfig` |
| codex · global | `$CODEX_HOME/hooks.json` — outside every repository (D7a) | `OwnFile` |
| copilot · global | `~/.copilot/hooks/grim.json` — a **directory glob accepting any filename**, so no collision is possible, and outside every repository | `OwnFile` |

**Project-scope hooks are Claude-only in v1, and that is a stated scope
reduction, not an oversight.** The re-validation showed why the first draft of
this decision was incoherent. Decision P requires the launcher argv to carry
`--root <abs>` so the arming root is never derived from the client-supplied
envelope `cwd`. A **user-level** registration file is one file shared by every
project on the machine, so it *cannot* carry a per-project `--root`. Therefore a
client can host a project-scope hook only if it has a **project-local surface that
is also machine-local** — i.e. gitignored, so the absolute launcher path never
travels. Exactly one v1 client has that: Claude's `settings.local.json`. Codex's
`.codex/hooks.json` and Copilot's `.github/hooks/*.json` are committed files, so
using them would reintroduce the very Block this decision exists to close.

Codex and Copilot therefore ship **global-scope hooks only** in v1. Widening is
additive under Principle 9 and needs one of: a gitignored project surface
appearing upstream, or a portable (non-absolute) launcher reference that survives
Decision D's byte-stability and fail-open constraints.

**Copilot uses `~/.copilot/hooks/grim.json`, not `settings.local.json`** — the
earlier draft chose the latter and cited `hooks_vendor_reports/copilot.md:330` in
support, when that line says the opposite: *"Writing into
`settings.json`/`settings.local.json`'s inline `hooks` key would require true
JSON-merge splicing … the directory-glob path is clearly the friendlier
integration seam."* The glob path keeps Copilot an `OwnFile` client, keeps the
v1 machinery estimate honest (Claude remains the **only** client needing the
nested object-in-array splice primitive), and keeps "no collision risk" true.

This dissolves both original Blocks rather than mitigating them, makes the
fail-open guard optional rather than load-bearing (so Claude's exec form
survives), and leaves D6's byte-stability constraint untouched. **"Gitignored by
convention" is not "gitignored"**, and grim's policy never touches the consumer's
root `.gitignore` (`subsystem-file-structure.md:297-301`) — so for the one
repo-resident surface (Claude project scope) grim **checks whether the target is
ignored and warns when it is not**, rather than assuming Claude has done it.

Consequence for the scope semantic, stated because it surprises people: a
project-scope hook means *this hook applies to this project*, while its
**registration is machine-local**. Team sharing happens through `grimoire.toml`
and `grimoire.lock` — the artifacts that are meant to travel — and each
teammate's `grim install` renders their own registration.

**J. Matcher push-down where the trust key allows it; matcher-less where it does
not.** The panel surfaced a sixth option nobody had weighed, and with it an
unstated coupling: registering one *matcher-less* entry per `(client, event)`
means a `Bash`-only gatekeeper still spawns grim on every `Read`, `Glob` and
`Edit` call. Pushing the declared matcher into the vendor's own matcher field —
all three v1 clients have one — makes the no-match path **no spawn at all**.

The cost is precise: Codex's `hook_hash` covers `(event_name, matcher,
normalized handler)`, so changing the matcher set re-prompts trust. **The
"trust cleared once" property is bought by putting grim on the hot path of every
tool call**, and the original text presented both as independent wins in adjacent
rows. Decision: **push the matcher down on clients whose trust key excludes it
(claude, copilot); stay matcher-less on codex and gemini, whose hash covers it.**
Two registration shapes, each chosen for a stated reason, and the hot-path cost
is eliminated exactly where it is free.

**K. `mutator` is `Declined` per tool shape, not per client.** The permitted-field
table (C-004) gates field *names*, not contents, and for the Bash tool the
structured input **is** `{"command": "<string>"}` — so the string-rewrite path was
open in v1 on both mutator-enabled clients. That is the `sudo` CVE-2023-22809
shape, and the research names it as one of three controls it would not ship the
tier without.

Worse, the escape hatch the original control 8 offered — "round-trip it through
the same parser the executor uses" — is **not implementable by grim**: the
executor is the client's own shell, on the client's platform, invoked by the
client. Leaving that branch in the ADR invites an implementer to hand-roll a
shell tokenizer, which *is* the CVE. It is deleted.

Decision: `mutator` resolves **`Native` for tools with structured input** (argv
arrays, typed fields) and **`Declined` for tools whose input is a shell-command
string** (Bash and each vendor's equivalent), enforced in the same render-time
table that already enforces permitted fields, pinned by a test. This keeps the
tier the owner chose, applies the ADR's own "an honest decline beats a rewrite
whose meaning grim cannot verify" where it actually bites, and widening later is
additive.

**L. Registrations are `sync_config` projections, never recorded outputs.** A hook
has both a payload (a file tree) and registrations (per client). Recording both
as `ClientOutput`s required a refcount over *entry*-typed outputs that the review
showed **can never fire across hook clients**. Decision: grim records **only** the
payload (Decision B), and `sync_config` recomputes the **entire** registration set
from install state after every install / update / uninstall — the shipped
OpenCode `instructions`-glob pattern (`src/install/opencode_config.rs:13`, whose
own comment names the hooks ADR as the origin of the pattern).

Refcounting registrations disappears because there is nothing per-hook to
refcount, uninstall correctness follows from convergence rather than bookkeeping,
and this is the same principle as Decision N and the dispatch table: **derive,
do not record**.

**M. Hooks are defence-in-depth, not a security boundary.** grim's own published
`ai-config-authoring` skill already tells authors that a hook is for "invariants:
format-on-save, lint gates, blocking writes to protected paths, audit logging"
and **not** for "anything needing judgment; a *hard* security boundary — use the
client's permission system for that"
(`catalog/skills/ai-config-authoring/references/choosing-types.md:40`).

That guidance is correct and this ADR adopts it rather than overwriting it,
because this design's own findings are why: grim always fails open (Decision G);
the vendor — not grim — decides firing order between grim's dispatcher and a
foreign hook, so a `gatekeeper` verdict can be correct when issued and stale by
the time the tool runs (Decision O's residual risk); and every v1 client's own
trust gate can silently disarm a registration. **The client's permission system
remains the boundary.** Marketing `gatekeeper` as a security control without this
sentence would put a shipped first-party artifact and this ADR in direct
conflict, and the artifact would be the one that is right.

**N. The feature flag needs no new precedence rule; only the CI escape is
global-only.** The panel's Block was that a cloned repo could ship `hooks = true`
plus a pre-approved digest list and arm a hook with no human. The repair is not a
cross-scope precedence model: **grim never merges config across scopes.** Each
command "operates on exactly one scope (global or project)"
(`src/command/scope_resolution.rs:6`), and `grim config list` is documented as
"never merged across scopes". So `[options.experimental] hooks` is read exactly
like every other `[options]` key — from the scope the command operates in — which
*is* "global or project", with no second knob and no precedence table.

The control that stops a cloned hook arming is the one Decision E already makes:
per-hook digest approval keyed on `(digest, scope root)`. A cloned repo may enable
the *feature*; every hook in it still faces an approval prompt bound to that
workspace. **The one piece that must be global-only is the CI escape** —
`--allow-hooks`, `GRIM_ALLOW_HOOKS=1`, and the approved-digest list are honoured
from global config or the invoking environment **only**, never from a project
`grimoire.toml`, because that is the thing that bypasses the prompt. A repository
cannot carry its own permission to skip approval.

Structurally, the flag reaches the runtime as **an empty dispatch table**: flag
off ⇒ `sync_config` writes no entries and removes every registration. This is what
makes Decision G's list observable, and it fixes a second defect the review
found — that flipping the flag off previously disarmed *nothing* until a
convergence pass ran, while a user who had just disabled the feature after a
scare reasonably believed it was off. `grim config set` on that key therefore
either runs the convergence pass itself or refuses with "run `grim install` to
disarm".

`[options.experimental]` is specified as a **general rollout facility** — a plain
boolean table under the existing `[options]` shape (`VendorOptions` /
`TuiOptions` precedent, `src/config/declaration.rs:111-176`) — so a future
feature can ship behind it in a patch without inventing a new mechanism. It does
not exist in the codebase today.

**O. Tier composition order is an invariant: mutators first, then every
gatekeeper.** Ordering was specified *within* the mutator tier but not *between*
tiers, so a `gatekeeper` could allow `{"command": "cargo build"}` and a `mutator`
later in the same declaration-ordered list rewrite it to `curl … | sh`, with grim
emitting one aggregated `allow` plus `updatedInput`. **The guardrail approved
bytes that never ran** — and declaration order is the installing user's order,
mutable by an unrelated `grim add`.

The invariant, with a test:

1. all `mutator` hooks run first, serially, in declaration order, threading each
   output into the next, producing **one** final input;
2. that final input is submitted to **every** `gatekeeper`;
3. any `deny` is **absorbing** and suppresses the mutation entirely;
4. `ask` outranks `allow`.

A gatekeeper therefore always judges the bytes that will actually run, and never
sees pre-mutation input. The accepted cost is that a mutator cannot react to a
denial.

**Residual risk, stated because grim cannot fix it.** This orders only the hooks
*grim* dispatches. Codex and Copilot **union** hook sources, so a user's
hand-authored native hook can sit in the same per-event array, and the **vendor**
decides firing order between it and grim's dispatcher. Kubernetes hit exactly
this with independent mutating admission webhooks and answered it with
`reinvocationPolicy: IfNeeded`; no vendor here offers an equivalent. So a
`gatekeeper` verdict can be correct when grim issues it and stale by the time the
tool runs. This is a disclosure, not a mitigation, and it is a reason Decision M
is worded as it is.

**P. One machine-local dispatch table, located by an explicit `--root`.** The
runtime had no specified way to find its own project table: C-006 put it at
`<workspace>/.grimoire/hooks/dispatch.json`, C-007 forbids `walk_up_for_config`,
and a test pins that. Both escapes were attacks — walking up from the process cwd
is a **clone-to-RCE** (a committed `subdir/.grimoire/hooks/dispatch.json` arms
attacker entries for any tool call whose cwd is inside `subdir`, committable
precisely because grim's self-managed `.gitignore` only exists after grim has run
there), and deriving the root from the envelope's `cwd` takes the arming decision
from data the client supplies on stdin.

Decision: **one table at `$GRIM_HOME/hooks/dispatch.json`**, entries keyed by
workspace root, with the launcher passing `--root <abs>`. Same principle as
Decision I — nothing armable lives inside a repository, so there is nothing to
plant. `--root` is byte-stable per project, so Codex's and Gemini's command
hashes still clear once. The runtime refuses any table at any other path, and an
acceptance test asserts that a clone carrying its own table and payload executes
nothing.

**Q. `policy` is a reserved key for a declarative gatekeeper.** A **pure-decision**
hook — allow/deny with no side effects — does not need to be arbitrary code at
all. The shipped counter-example is Sondera, which evaluates Cedar policy in an
engine rather than spawning a script, **eliminating** the RCE surface instead of
containing it, and would make `gatekeeper` far cheaper to trust than the nine
controls `mutator` requires.

`policy = "cedar" | "rego"` is therefore **reserved on paper and unparsed in
v1**, exactly as `adr_render_layout_stability.md` §4 reserves
`[options] render = "files" | "plugin"` without parsing it. Cheap, additive, and
it records in the schema that the safest possible gatekeeper is declarative
rather than executable.

### Consequences

**Positive:**
- Hooks become installable *and active*, with one payload dialect and one
  response dialect for the author, on three clients at v1 and additively after.
- Grim gains a **revocable** capability: a hook can be refused at fire time.
  Every prior design could only gate at install time.
- Deterministic mutator ordering — a capability no vendor offers.
- The nested-array splice and the dispatch table are the last two pieces of the
  reversible-foreign-config engine; both generalize.
- F1 (the erased exec bit) becomes optional rather than blocking.

**Negative:**
- Grim is on the hot path of every tool call on every client where a hook is
  registered, including events where nothing matches.
- A grim panic becomes an agent-visible event.
- New machinery: nested object-in-array JSON splice, dispatch table, trampoline
  subcommand, launcher generation, response projector, approval store.
- A hand-written `[options.experimental]` or `[hooks]` table is a hard parse
  error (exit 78) on an older grim, because every config parse path is
  `deny_unknown_fields` (`src/config/declaration.rs:118,166,183,239`,
  `src/config/project_config.rs:78`) — the same trade `[options.vendors]` took.

**Risks (with mitigations):**
- **Grim becomes load-bearing at agent runtime** → decision G: internal errors
  never reach the client as a failure; the launcher fails open when the binary
  is absent; a pinned test asserts `grim hook run` touches no scope resolution.
- **Version skew** — config written by a newer grim, invoked by an older one →
  the dispatch table carries `schema`; an unrecognized version is treated as
  "no match", exit 0.
- **Vendor response schema drift** — Claude's `continueOnBlock` default flipped
  at v2.1.210 → the response projector is a data table with a closed permitted
  field set per `(vendor, event)`; drift is a table edit, and the vendor
  watchlist (`.claude/rules/vendor-capability-watchlist.md`) carries the
  date-stamped re-verification duty.
- **Codex positional trust key** — a third party inserting a hook group above
  grim's shifts `group_index`, orphans the trust record, and grim's
  byte-identical hook silently reverts to `Untrusted`, i.e. silently
  non-executing → D7a: grim owns the whole `hooks.json`, pinning both path and
  indices. Collision with a pre-existing user file is handled by the shipped
  untracked-destination refusal (exit 65, `--force` to adopt,
  `src/install/installer.rs:95,668,696,2235` → `src/error.rs:347-351`), not a
  special case. `hooks.json` and inline `[hooks]` union, so grim owning the file
  never deprives the user of a place for their own hooks.
- **grim forging a vendor trust record** — `openai/codex#21615` shows third-party
  integrators writing `[hooks.state."<key>"] trusted_hash` into the user's own
  `~/.codex/config.toml` to skip the prompt → **prohibited, absolutely.** Grim
  never writes into a consent mechanism it does not own. The same rule covers
  Gemini's `trusted_hooks.json`. Grim's own digest-pinned approval is grim's
  gate; the vendor's review is the vendor's, and the user clears it.
- **Re-prompt habituation** — the canonical TOFU critique is that users trained
  to click through routine "trust changed" prompts have habituated by the time a
  real attack arrives → the dispatcher design collapses vendor-side prompts to
  one per client; grim's own prompt fires only on a digest change, and the
  mutator wording is distinct from observer and gatekeeper.

## Technical Details

### Architecture (C4)

**Context.** A hook author publishes one artifact to an OCI registry. A
developer declares it; grim installs it into every enabled client. At runtime
the *client* — not grim, and not the user — invokes grim, which invokes the
author's payload and answers the client in the client's own dialect.

```
 hook author ──publish──► OCI registry ──resolve/fetch──► grim ──install──► AI client (claude|codex|copilot)
                                                                                  │
                                                                   fires event ────┘
                                                                                  ▼
                                                                    grim-hook launcher ──► grim hook run ──► payload
```

**Container.** One binary, `grim`. No daemon: `eslint_d`'s 700 ms → 160 ms win
is entirely about eliminating **Node** startup and does not transfer to a
compiled Rust binary; `pnpm` shipped `pnpm server` and removed it in pnpm 11.
Daemon stays the escape hatch if measurement demands one.

**Component.**

```
grimoire.toml [hooks]  ──► DesiredSet.hooks ──► resolve_lock ──► GrimoireLock.hooks
                                                                        │
                                             install_all / install_one   ▼
  ┌───────────────────────────────────────────────────────────────────────────────────┐
  │ per LockedArtifact(kind = Hook):                                                  │
  │   [options.experimental] hooks == false ⇒ warn + skip via the Declined path       │
  │   approval gate (C-009): digest known? unchanged? else prompt / CI escape         │
  │   fetch blob ──► DefaultMaterializer ──► <scope>/hooks/<name>/     (ONE per scope)│
  │   for each client where hook_surface().is_some():                                 │
  │       resolve per hook × event × tier ⇒ Native | Degraded | Declined  (C-005)     │
  │       record ClientOutput { target: shared payload dir }   ← payload only (L)       │
  └───────────────────────────────────────────────────────────────────────────────────┘
                                          │
              Vendor::sync_config ─────────┴────────► per shape:  OwnFile      (codex, copilot)
                (state-derived convergence;                       SpliceConfig (claude)
                 registrations are DERIVED, never                 CodegenModule(future: opencode…)
                 recorded — L; regenerated wholesale
                 PER ROOT KEY — E.1/F-3)
                                          │
                                          └────────► $GRIM_HOME/hooks/bin/grim-hook      (launcher)
                                                     $GRIM_HOME/hooks/dispatch.json      (C-006, P)
                                                       one machine-local table,
                                                       entries keyed by workspace root

runtime:  client ──► launcher ──► grim hook run --client C --event E --root <abs>
                                     │ read the ONE table, exact-key lookup on (root,C,E)
                                     │ no match ⇒ exit 0                    ← the hot path
                                     │ match ⇒ re-verify digest, spawn payload(s) with envelope
                                     └─► project canonical response ⇒ vendor+event shape (C-004)

uninstall / prune ──► sync_config removes the entry when the last hook for that
                      (client, event) is gone; shared_by_surviving_sibling releases
                      the payload dir when the last client drops it
```

**Code (where it earns its place) — the response projector.** This is the one
component where a natural implementation ships a security bug on day one. Codex
documents `hookSpecificOutput.decision.updatedInput` on `PermissionRequest` as
*"Reserved for a future input-rewrite capability … fail closed if present"*, and
the same for `interrupt` and `updatedPermissions`
(`hooks_vendor_reports/codex.md` §7). A pass-through projector that forwards
`updated_input` wherever the field name exists therefore **blocks the tool
call**. So the projector is not a function; it is a **table** keyed on
`(vendor, event)` with a closed permitted-field set and an explicit forbidden
set, and emitting a field outside the permitted set is a build-time /
render-time error, never a runtime surprise.

### Component contracts

**Panel amendments — applied in place.** The design panel (5 perspectives, 14 Block
findings) and the owner's decisions changed six contracts. The `C-###` numbers are
**not** renumbered, because `/hex-plan` and the Specify gate join on them — but the
contract *text* below has been **edited**, not merely annotated. The first attempt
at this table said "the decision wins where they disagree"; the re-validation
correctly rejected that, because a contract is the text a test gets generated
from, so a reading instruction cannot substitute for an edit. This table is now an
index of what changed, not a precedence rule.

| Contract | Amended by | What changed |
|---|---|---|
| C-004 (response projection) | **K** | Permitted-field gating is per field *name*; that is insufficient, because for a shell tool the structured input *is* `{"command": "<string>"}`. `mutator` is now `Declined` per **tool shape**, enforced in the same render-time table. The "round-trip through the executor's parser" branch is deleted as unimplementable. |
| C-006 (dispatch table) | **P**, **N** | One machine-local table at `$GRIM_HOME/hooks/dispatch.json`, entries keyed by workspace root, located by an explicit `--root` in the launcher argv — never by walk-up, never from the envelope `cwd`. Flag-off is expressed as an *empty* table. |
| C-007 (runtime fast path) | **E.1**, **N** | The table is the **sole** runtime input; flag state, approval, and cross-file digest agreement are compiled in at convergence rather than read at runtime. This is what makes C-007's no-reads property and the security composition consistent, which they were not. |
| C-009 (approval) | **E.4**, **N** | Key is `(artifact content digest, scope root)` — an absolute workspace root, not the `Project`/`Global` kind. The CI escape is honoured from global config or the environment only. Exec-time re-verification stays on the **matched** branch only; the no-match path hashes nothing. |
| C-011 (mutator controls) | **K**, **O**, and the audit revision | Control 8 is replaced by K. Tier composition becomes the decision O invariant (mutators first → one final input → every gatekeeper; `deny` absorbing; `ask` > `allow`). Audit defaults to a **redacted metadata** view with full-body capture behind a separately-enabled mode — the Kubernetes `Metadata`/`Request`/`RequestResponse` and CloudTrail-truncation pattern — because an unbounded unredacted trail of mutated tool input is itself a secret-exposure and log-injection surface (CWE-117, CWE-400). |

| C-010 (state / lock / config deltas) | **L**, **N** | **`ClientOutput.entry` is no longer widened at all** — registrations are not recorded, so no three-level pointer is ever written, `entry` keeps its shipped two-level meaning, and the "second-order trap" (an older grim unable to uninstall a deep entry) and its `docs/src/stability.md` consequence both disappear. Adds `ExperimentalOptions`, and records that `grim config set` on the flag **refuses** with "run `grim install` to disarm" rather than running convergence itself. |
| C-013 (`clients.md` Hook cell) | **C**, **F** | The `✓` conjunct is scoped to tiers **valid at that event**, and F now rejects `gatekeeper` on a verdict-less event at `grim build` — without which the cell was unsatisfiable for every client. Whether a client reaches `✓` is *computed* from C-004, never asserted in prose. |

Two contracts are unchanged but worth flagging: **C-012** already carried the
redaction requirement that the audit revision above generalizes, and **C-014**
(`grim schema --kind hook`) gains the reserved-but-unparsed `policy` key from
decision Q.

The **Component diagram** above § Component contracts is also amended (it is not a
numbered contract, and the re-validation found it contradicting B/L, I and P): the
recorded `ClientOutput` is the shared payload directory, registrations are marked
derived, the table is the single machine-local `$GRIM_HOME/hooks/dispatch.json`
keyed by workspace root, and the runtime argv carries `--root`.


**C-001 — `hook.toml` manifest.** Decision F above, verbatim, plus: wire type
rides `adr_oci_empty_config_compat.md` unchanged (OCI empty config,
`com.grimoire.kind = "hook"`, no custom `artifactType`).
`ArtifactKind::Hook::subdir() == "hooks"`, `artifact_type()` and
`config_media_type()` gain arms, `is_dir_artifact() == true`.
`BundleMember.kind` already carries `ArtifactKind`, so bundles group hooks for
free — and a bundle-delivered hook must be visible at `grim add` time and named
in the approval prompt, never arrive as a side effect
(`adr_effective_set_mutations.md` is the mechanism).

**C-002 — canonical stdin envelope.** One JSON object on stdin. `raw` carries
the vendor payload **verbatim and unmodified**; everything above it is grim's
normalization, in Claude's spelling.

```json
{ "schema": 1,
  "event": "PreToolUse", "native_event": "PreToolUse",
  "client": "codex", "scope": "project",
  "hook": "shell-guard/deny-curl-pipe-sh", "tier": "gatekeeper",
  "cwd": "/repo", "session_id": "…", "correlation_id": "…",
  "tool": { "name": "Bash", "input": { "command": "curl x | sh" } },
  "raw": { "…": "the client's own payload, byte-for-byte" } }
```

Env vars carry flat scalars only, and **must never carry secret-bearing
content**: `GRIM_HOOK_SCHEMA`, `_EVENT`, `_CLIENT`, `_NAME`, `_TIER`, `_TOOL`,
`_CWD`, `_DIR` (the artifact's own install directory — Claude's
`$CLAUDE_PROJECT_DIR` is the precedent for why that is not optional), and
`_PAYLOAD` only when `payload = "file"`. Rationale is not stylistic: `argv` is
visible to any local user through `/proc/<pid>/cmdline`; env is readable through
`/proc/<pid>/environ`, **inherited by every grandchild**, and captured in crash
dumps and CI logs; OWASP's own guidance is that env vars are "not recommended
unless other methods are not possible". `post-tool` payloads can embed whole
diffs and would overflow `ARG_MAX` (and Windows' ~32 KiB per-variable cap).
Stdin is the only transport that avoids all of the always-on metadata vectors;
it does not solve core dumps, and neither does anything else.

**C-003 — canonical response.** Small and closed. Anything richer is native
passthrough for one declared vendor.

```json
{ "decision": "allow" | "deny" | "ask" | "none",
  "reason": "…", "context": "…", "user_message": "…",
  "stop": false,
  "updated_input": { "…": "mutator tier only, PreToolUse only" } }
```

**C-004 — per-`(vendor, event)` response projection.** The v1 table. `⊘` = the
canonical field has no equivalent ⇒ dropped with a one-time warning
(`Degraded`); a tier requiring it is `Declined`, never degraded.

| vendor · event | verdict field | context | mutation | forbidden — emitting fails the render |
|---|---|---|---|---|
| claude · PreToolUse | `hookSpecificOutput.permissionDecision` + `permissionDecisionReason`; `hookEventName` **required const** | `hookSpecificOutput.additionalContext` | `hookSpecificOutput.updatedInput` | — |
| claude · PostToolUse | top-level `decision: "block"` + `reason` | `hookSpecificOutput.additionalContext` | ⊘ | `updatedInput` |
| claude · SessionStart | ⊘ (cannot block ⇒ gatekeeper `Declined`) | `hookSpecificOutput.additionalContext` | ⊘ | `decision`, `updatedInput` |
| claude · Stop | top-level `decision: "block"` + `reason`, or `continue: false` + `stopReason` | ⊘ | ⊘ | `updatedInput` |
| codex · PreToolUse | top-level `decision: "approve"\|"block"` + `reason`, **and** `hookSpecificOutput.permissionDecision` | `hookSpecificOutput.additionalContext` | `hookSpecificOutput.updatedInput` | — |
| codex · PostToolUse | `decision: "block"` + `reason` | `hookSpecificOutput.additionalContext` | ⊘ (`updatedMCPToolOutput` is a *result* rewrite, not an input rewrite — native-only) | `updatedInput` |
| codex · SessionStart | ⊘ | `hookSpecificOutput.additionalContext` | ⊘ | `decision`, `updatedInput` |
| codex · Stop | `decision: "block"` + `reason` (**`reason` is required when blocking** — enforced in Codex's output parser, not its JSON schema) | ⊘ | ⊘ | `updatedInput` |
| codex · PermissionRequest *(native-only)* | `hookSpecificOutput.decision.behavior` + `message` | ⊘ | ⊘ | **`updatedInput`, `updatedPermissions`, `interrupt` — reserved upstream and fail closed if present** |
| copilot · PreToolUse | `hookSpecificOutput.permissionDecision` + `permissionDecisionReason` | `hookSpecificOutput.additionalContext` | **unverified — see OQ1 ⇒ mutator `Declined` at v1** | the unverified spelling |
| copilot · PostToolUse | ⊘ | `additionalContext` | ⊘ (`modifiedResult` is a result rewrite — native-only) | `updatedInput` |
| copilot · SessionStart | ⊘ | ⊘ (**NOT DOCUMENTED**) | ⊘ | `decision` |
| copilot · Stop | `decision: "block"\|"allow"` + `reason` (**runaway guard: 8 consecutive blocks and the CLI forces the turn to end**) | ⊘ | ⊘ | `updatedInput` |

Two registration-level rules follow from the table. Grim registers **PascalCase**
event names on Copilot, because Copilot's stdin payload shape *differs by the
casing used in config* and the PascalCase path is the Claude-shaped one — one
fewer dialect to normalize, at no cost. And `hookSpecificOutput.hookEventName`
must echo the firing event on Claude and Codex, so the projector needs the
native event name, not only the canonical one: hence `native_event` in C-002.

**C-005 — `Vendor` trait additions.** All defaulted, so 14 vendors need no edit
and a forgotten one declines:

```rust
fn hook_surface(&self) -> Option<HookSurface> { None }
fn hook_tier_support(&self, tier: HookTier, event: CanonicalEvent) -> KindSupport { KindSupport::Declined }
fn hook_event_name(&self, event: CanonicalEvent) -> Option<&'static str> { None }
fn hook_registration(&self, scope: ConfigScope, event: CanonicalEvent, launcher: &Path)
    -> Option<HookRegistration> { None }
```

`HookSurface` names the install shape (`OwnFile { path_for(scope) }`,
`SpliceConfig { pointer }`, `CodegenModule { template }`) — the third exists in
v1 as a variant with no v1 implementor, because the trait must not need
reshaping when opencode/kilo/amp/openclaw land. `hook_tier_support` is the
per-**hook** resolution the per-kind `kind_support` seam cannot express: v1's
three clients support nearly every tier, so a per-kind gate would look
sufficient and then have to be widened for kiro, goose, cline and antigravity.
`installer::client_supports_kind` (`src/install/installer.rs:1106-1108`)
special-cases `Hook` to `hook_surface().is_some()`;
`expected_outputs::expected_clients` (`src/install/expected_outputs.rs:38`)
inherits that unchanged.

**The projection rule, restated as the guard on `hook_event_name`.** A canonical
event may project onto a differently-named native event **iff it fires at the
same moment AND the native surface's power is ≥ what the hook's tier requires.**
Renaming is allowed (Gemini's `BeforeTool` *is* `PreToolUse`). Narrowing is
allowed (Cursor's `beforeShellExecution` is `PreToolUse` restricted to shell; a
hook declaring `event = "PreToolUse", matcher = "Bash"` is a *more precise* fit,
not a lossy one). **Moment substitution is forbidden** — never relocate a
`PreToolUse` guardrail onto `PostToolUse` because it is "similar"; that runs
after the damage, and silent moment-substitution is how a fidelity decision
becomes a security hole.

Three failure modes, three shipped verdicts: event absent on that client ⇒
`Declined` (warn, zero outputs, `grim status` shows it); event present but the
tier is not honourable ⇒ `Declined` — **never degrade a guardrail into a
logger**; one response *field* has no equivalent ⇒ `Degraded` (install, drop the
field, warn once), which is `Degraded`'s shipped meaning.

**C-006 — dispatch table.** *Amended in place by decisions P, N and E.1.* **One**
small JSON file for the whole machine, `$GRIM_HOME/hooks/dispatch.json` — never a
per-scope file inside a workspace, so there is nothing plantable in a repository
(decision P). Written by the `sync_config` convergence pass **atomically and
wholesale per root key** — `entries["<abs root>"]` is replaced entirely, sibling
root keys are untouched, and nothing within a key is ever patched incrementally
(E.1). A concurrent reader sees the old table or the new one, never a torn write.
Shape:
`{ "schema": 1, "entries": { "<abs root>|global": { "<client>/<Event>": [ {hook
id, payload argv, resolved payload path, approved digest, tier, precompiled
matcher, timeout, payload mode} ] } } }`, in declaration order. **The flag off ⇒
that root key holds no entries** (N), which the runtime already handles as
no-match. There is no `fail` field. Matchers are stored
**precompiled as exact or glob forms, never as regex source**: regex
*compilation* alone measures from microseconds to ~44 ms for Unicode-heavy
patterns (the `regex` crate's own `PERFORMANCE.md` cites `\pL{100}` at ~44 ms),
which would dwarf everything else on the no-match path. The launcher argv carries
**`--root <abs>`** (decision P), which selects the root key; a global-scope
registration carries the global marker instead. The root is **never** derived from
the envelope `cwd`, because that is client-supplied. Project- and global-scope
entries may both exist for one `(client, event)` — the clients union their hook
sources — and each invocation selects exactly one root key, so ordering stays
deterministic. Per decision I only Claude has a project-scope registration in v1,
so only Claude's table can carry a non-global root key.

**C-007 — `grim hook run`.** Exempt from `Printable` and the report module under
`subsystem-cli-api.md` § "Commands That Exec a Child Process" — it speaks the
vendor's stdout protocol instead. `grim mcp` and `grim tui` are the shipped
exempt precedents (`src/app.rs:147,153-161`), but only for the *report* half:
both still receive `&ctx`. `grim hook run` must be the **first** dispatch arm
that takes `&args` only, the way `grim schema` and `grim completions` do
(`src/app.rs:131,135`) — `Context::new()` itself is already cheap (env reads
plus `OnceLock::new()`, no I/O, `src/context.rs:167-183`); the cost to avoid is
**per-command scope resolution**: `walk_up_for_config`
(`src/config/project_config.rs:149`, the only walk-up in the tree) plus the lock
and install-state parses layered on it. Because this is the first command whose
hot path *depends* on that property rather than merely happening to have it, the
property needs a **test**, not a convention. No lock, no catalog, no registry,
no network, no tokio runtime, no advisory file lock, no per-invocation regex
compilation. Exit codes per decision G. `grim hook list` is an ordinary report
command on the normal path.

**C-008 — launcher.** Decision D. Generated at `$GRIM_HOME/hooks/bin/grim-hook`
by `sync_config`, `chmod 0o755` (grim generates it locally, so F1 never
applies), self-healing on re-install, byte-stable across grim upgrades. Body
resolves `grim`, `exec`s `grim hook run "$@"`, and exits 0 if `grim` is not
found. Registered as an absolute path in exec form.

**C-009 — approval store and digest verification.** Decision E.
*Amended in place by decisions E.4 and N.*

> ⚠ **WITHDRAWN by amendment A3 — the text below is historical.** The runtime
> hashes nothing on either branch. Integrity is **lock-time**; payload drift is
> **evidence, not prevention**, surfaced via `ClientOutput::content_hash` at the
> next `grim status` or install. The replacement C-009 lives in the plan. Do not
> build a store, a chain, or a `(digest, scope root)` key from this paragraph.

`$GRIM_HOME/state/hook_approvals.json`, global scope only, append-only,
hash-chained, keyed **`(artifact content digest, scope root)`** — the *absolute
resolved workspace root* for a project-scope approval, the global marker for a
global one. The earlier key was `(scope, artifact name, digest)` with `scope` as
`Project | Global`, which did not apply the `direnv` lesson it cited: every
project shared the literal value `Project`, so one approval armed every workspace
on the machine. The artifact name is dropped as redundant — it is inside the
digest.

CI escape per D5: `--allow-hooks`, `GRIM_ALLOW_HOOKS=1`, or an approved-digest
list — **honoured from global config or the invoking environment only, never from
a project `grimoire.toml`** (decision N), because that is the one input that
bypasses the prompt. The escape is itself recorded in the audit trail, never
silent.

**Where the digest re-check goes, stated so an implementer cannot "optimize" it
onto the wrong branch:** the **no-match path never executes anything, so it
needs no hash at all**; the re-check happens on the *matched* path only,
immediately before spawning that payload, which is already the expensive branch.
This closes the TOCTOU window between "approved" and "ran" without touching the
hot path. The repeated real-world failure across four unrelated ecosystems is an
approval keyed on the wrong identity, checked at the wrong time — GitHub Actions
tag mutation, Bun's name-based trust, Cursor's create-vs-edit gap
(CVE-2025-54135 / CVE-2025-54130) and its case-normalization bypass
(CVE-2025-59944). A digest is structurally immune to the mutable-tag and
name-collision variants **only if** grim hashes the artifact that will actually
run — not a bundle wrapper, not a declared version — and re-checks it against
the file on disk immediately before `exec`.

**C-010 — state / lock / config deltas.** All additive; mechanics in Migration.
`GrimoireLock { …, #[serde(default, skip_serializing_if = "Vec::is_empty")] hooks }`;
`DesiredSet.hooks`; `ConfigOptions` gains a sixth field
`ExperimentalOptions { hooks: bool }` following `VendorOptions`/`TuiOptions`
verbatim (`src/config/declaration.rs:111-176,184-223`) with `is_empty()` +
`skip_serializing_if` so an unconfigured table never serializes; a static
`options.experimental.hooks` entry in `ConfigKey::ALL`
(`src/command/config_keys.rs`), simpler than the dynamic
`options.vendors.<name>.shared_skills` pattern at `:314-333`.
**`ClientOutput.entry` is NOT widened — amended in place by decision L.** The
earlier draft widened it to a three-level pointer for Claude's
`hooks.PreToolUse[i].hooks[j]` and then documented a "second-order trap": an older
grim would parse the field (it is a `String`) but `split_pointer` would return
`None` (`src/install/json_splice.rs:28-32` rejects a deeper member), so it could
not uninstall the entry — a behaviour break for old binaries, to be recorded in
`docs/src/stability.md`.

Decision L removes the whole problem: **registrations are never recorded as
outputs at all.** They are recomputed from install state by `sync_config` on every
mutation, so no `entry` pointer for a hook registration is ever written, `entry`
keeps its shipped two-level meaning, and there is no old-binary behaviour break
and no `stability.md` consequence to document. The only recorded hook output is the
shared payload directory (decision B), which is an ordinary file output.

`grim config set options.experimental.hooks false` **refuses** with "run
`grim install` to disarm" rather than performing installer convergence itself — a
config-write command must not run the installer. That is new behaviour on
`src/command/config.rs` and is named in § Scope deltas.

**C-011 — mutator pipeline.** Mandatory, non-optional, all nine controls from
`research_hooks_trampoline.md` § "Mutator tier". The ones that shape code:
(1) capability gate — `Declined`, never degraded; (2) the per-`(vendor, event)`
permitted-field table of C-004, whose absence would ship the Codex fail-closed
bug on day one; (3) **serial execution in declaration order, threading each
mutator's output into the next as input, emitting exactly one final
`updated_input`** — Claude resolves competing `updatedInput` as
last-process-to-exit-wins and most other clients leave ordering
`NOT DOCUMENTED`, so grim converts a race into a reproducible pipeline;
(7) digest re-verified at execution time (C-009); (8) **amended in place by
decision K — `mutator` resolves `Native` only for tools whose input is
structured** (argv arrays, typed fields) and **`Declined` for tools whose input is
a shell-command string** (Bash and each vendor's equivalent), enforced in the same
render-time table as the permitted fields and pinned by a test. The earlier
wording said "prefer structured input … either round-trip it through the same
parser the executor uses or treat `mutator` as `Declined`" — the round-trip branch
is **deleted**, because grim does not have and cannot acquire the client's shell,
and offering it invites an implementer to hand-roll a tokenizer, which is
precisely the `sudo` CVE-2023-22809 defect. (3a) **Tier composition is decision
O's invariant**: all mutators first, serial, one final input to *every* gatekeeper,
`deny` absorbing, `ask` outranking `allow`, gatekeepers never seeing pre-mutation
input. (5) Where the client supports it,
a mutation also emits a `systemMessage` / `additionalContext` line describing
the rewrite, so the agent's own transcript records that its command was altered
— no vendor does this by default. (6) The approval prompt names the tier in
plain language for mutators specifically — "this hook can rewrite commands
before they run" — distinct from observer and gatekeeper wording.

**C-012 — audit record.** Mandatory; **unredacted** audit is not. Default level
is a **redacted metadata view**: hook id, event, client, tier, digest, which
fields changed, sizes, the decision verdict, a correlation id, outcome status.
Full before/after body capture sits behind a stricter, separately-enabled mode.
This mirrors Kubernetes' `Metadata`/`Request`/`RequestResponse` levels and
CloudTrail's field truncation, both designed exactly this way, and it is a
correction of the naive design: audit logging has **zero** documented
before/after evidence of reducing incidents anywhere surveyed — every mature
system treats it as forensics, not prevention — while logging a mutated tool
input, which may carry a secret, invites secret capture, log injection
(CWE-117, with ANSI-escape spoofing of whoever reviews the trail — see
CVE-2025-58160 in `tracing-subscriber`, grim's own stack) and unbounded growth
(CWE-400). So: control characters sanitized on the way in, per-record size cap,
rotation from day one, and a write failure fails **closed** for the audit
(refuse to run the hook) rather than silently proceeding unlogged.

**C-013 — `docs/src/clients.md` Hook cell + parity test.** Decision C. Fifth
column driven by one `hook_matrix_cell(client)` helper; the test at
`src/install/client_target.rs:748-784` grows a second per-kind special case
beside MCP's.

**C-014 — `grim schema --kind hook`.** A fifth `SchemaKind` variant plus one arm
in each of the three parallel matches (`src/command/schema.rs:46,71-74,82-85,110-113`)
and a `HookManifest` struct deriving `schemars::JsonSchema`, following
`McpDescriptor` verbatim — the least structurally surprising extension in the
whole change, precisely because `Mcp` already established "the schema is a
schemars struct, not the artifact frontmatter".

### NFR coverage

Only the NFRs this decision actually affects; silence on the rest.

| NFR | Effect and the contract that bounds it |
|---|---|
| **Latency** | The one genuinely new cost. `PreToolUse` fires on every tool call and the dispatcher adds one process spawn ahead of the payload's own. Bounded by C-006 (one small precompiled table, exact-key lookup) and C-007 (no scope resolution, no tokio, no regex compilation). The *number* is out of scope — measured, not invented. Platform reality: full program launch is ~10× cheaper on Linux than macOS and >20× cheaper than Windows `CreateProcess`, which is additionally sensitive to Defender; **WSL2 is a third platform, not a Linux proxy** (~2–5 ms fork vs ~30 µs native). |
| **Security** | Grim's risk class changes from "text an agent reads" to "code that executes automatically". Bounded by D10 (off by default — the single most load-bearing control), C-009 (digest-pinned approval, re-verified before exec), C-004 (closed permitted-field sets), C-011, C-012, and decision G. `/security-auditor` is a required gate before any implementation merge. |
| **Availability** | Grim becomes a runtime dependency of the agent loop. Bounded by decision G and C-008: every internal failure is exit 0 and one log line, so grim's absence or failure degrades to "hooks off", never "agent blocked" — which matters because Copilot's `preToolUse` fails closed. |
| **Operability** | New surfaces to inspect: `grim hook list`, `grim status` reporting gated and `Declined` hooks through the existing `Declined`-kind path rather than a new one, the audit trail, and per-client reload behaviour that already diverges in v1 (Claude hot-reloads; Codex is fine; **Copilot CLI loads hook config only at CLI start** and needs a restart). Reload requirements must be reported per client at install time. |
| **Scalability** | Install-time work is O(hooks × clients); **runtime work must not be O(installed artifacts)** — that is exactly what C-006 buys, and it is the same move `mise` made in eliminating `asdf`'s shim indirection (~120 ms → ~5–10 ms per command) by pushing resolution to install time. Adding hook #2 touches no vendor config **when it reuses an existing matcher on an existing event**; a new matcher costs one vendor-config write (decision J). |
| **Cost** | No new infrastructure, no service, no daemon, no new dependency. The real cost is maintenance: 15 hook-capable clients with independently drifting response contracts, on a watchlist with a **90-day** re-verification horizon (the vendor survey's own expiry is deliberately short — three clients shipped or reworked hooks within 90 days of it). |

## Migration / Rollout Plan

**Feature flag first.** `[options.experimental] hooks = false`. Gated off, hook
artifacts resolve and lock normally, `grim install` **skips them with a warning**
and `grim status` reports them gated — reusing the `Declined`-kind reporting
path, not a new one. This is not a launch-phase convenience: it is the control
with the only causal track record in the entire prior-art survey (Insight 4),
and the ADR says so in those terms deliberately.

**Phase 1 — the kind, plus one client.** `ArtifactKind::Hook`, `hook.toml`,
envelope, canonical response, dispatch table, launcher, `grim hook run`,
approval store, Claude's `SpliceConfig` registration (which forces the nested
object-in-array splice). Acceptance tests: install → fire → update → uninstall →
prune; user edits to `settings.json` preserved; a gated install skipped;
re-materialize leaves `status` not-modified.

**Phase 2 — the second and third clients, both `OwnFile`.** Codex
(`$CODEX_HOME/hooks.json` global, `.codex/hooks.json` project) and Copilot
(`~/.copilot/hooks/grim.json`, `.github/hooks/grim.json` — a **directory glob
accepting any filename**, which makes Copilot the only v1 client with no
collision risk at all). Two clients simultaneously is the point: a portability
layer validated against a single vendor is not validated. Copilot's cloud agent
stays out — it reads `.github/hooks/*.json` only from the default branch, in an
ephemeral firewalled sandbox where the `grim` binary is not present.

**Phase 3 — the other twelve, additively.** One `hook_surface()` +
`hook_tier_support()` + `hook_event_name()` implementation per vendor, one
pinned-set test line, one `docs/src/clients.md` cell moving `✗ → ◐/✓`. The
seams that make this additive rather than a redesign — install-shape
abstraction, per-hook capability resolution, the per-`(vendor, event)` response
table, filename-as-identity in the own-file shape (Cline's filename **is** the
event name, from a hardcoded allow-list), payload-delivery variance (Kiro's IDE
bug #7375 delivers `{}` on stdin and puts the payload in a `USER_PROMPT` env
var), restart/reload reporting, the native-event escape hatch, and non-command
handler kinds modelled as native-only — all exist in v1 by construction.

**Principle 9 mechanics.**

- **Emit hook fields only when non-empty.** `declaration_hash`
  (`src/config/hash.rs:44-79`) already wraps `agents`/`bundles`/`mcp` in
  `if !set.<field>.is_empty()`, with the verbatim comment "so an mcp-free config
  hash matches pre-mcp grim (no version bump)". `hooks` follows the identical
  pattern and **`DECLARATION_HASH_VERSION` stays 1** (`src/config/hash.rs:27`).
  Lock stays V1 with an optional `[[hook]]` array. State fields are
  `#[serde(default)]` *and* skipped when empty — an always-emitted new field
  makes every new state file unreadable by an older grim, "a breaking change,
  not an additive one" (`src/install/install_state.rs:106-111`, grim's own
  words). A user with no hooks keeps byte-identical files.
- **The struct-literal cost no enum count reveals.** `GrimoireLock`
  (`src/lock/grimoire_lock.rs:47-65`) and `DesiredSet`
  (`src/config/declaration.rs:373-388`) carry **one field per kind, not a map** —
  13 `GrimoireLock { … }` and 7 `DesiredSet { … }` explicit literal sites.
  Sites using `..Default::default()` compile silently; `command/install.rs:262-275`
  and `resolve/resolver.rs:957-967` construct every field by hand and are
  certainly compiler-forced.
- **Accepted asymmetry**, identical to how `[options.vendors]` and the agent
  kind landed: a hand-written config or lock using the new keys needs the new
  grim (exit 78 / `deny_unknown_fields` on an older one).

### Scope deltas, named

| Surface | Cost |
|---|---|
| Exhaustive `match kind { … }` with no wildcard | **~25–30 arms across ~20 files** — `oci/artifact_kind.rs` (`subdir`, `artifact_type`, `config_media_type`, `is_dir_artifact`, `Display`), `lock/effective_set.rs:191`, `install/client_target.rs:308,355`, `skill/local_pack.rs:51`, `command/{build,remove,add,install,update,publish}.rs`, `resolve/resolver.rs:944`, `mcp/render.rs:99-150`, `fetch.rs:484,531,922`, `install/installer.rs:1106,1972,2442-2464`, `tui/app.rs` (×5). Compiler-forced. |
| Signature change, not just a new arm | `command/add.rs:646-652` `single_entry_lock` builds a 4-tuple `(skills, rules, agents, mcp)` — a sixth kind makes it a 5-tuple. |
| **Not** compiler-forced, must be edited by hand | `artifact_kind.rs:117-128` `from_artifact_type` / `from_config_made_type` iterate a fixed array literal; the test module at `:157-263` iterates another. Adding `Hook` compiles silently without inclusion, and a hook artifact would simply never resolve by legacy artifactType. |
| `GrimoireLock` / `DesiredSet` struct literals | **13 + 7 sites** — invisible to any enum-match count, which is why the 2026-06-03 draft's "~12 sites" was wrong in both the count and the *kind* of cost. |
| `path_anchor::candidate_anchors` (`src/install/path_anchor.rs:700`) | Its own per-`(client, kind)` arms; likely the single largest file to touch. **No new `PathAnchor` variant** — `Workspace` and `GrimHome` already cover both scopes (decision B); adding a *kind* is the expensive direction, adding a vendor is not. |
| `Vendor` hook seams | 3 files at v1, 17 eventually — and the trait default declines, so an unvisited vendor is safe (decision A). |
| `json_splice` | **Genuinely new code**: `upsert_array_element` / `remove_array_element` (`src/install/json_splice.rs:184,230`) handle a **string** element at a **root** key only (they call `json_string(element)` and compare by value equality), and `split_pointer` (`:28-32`) is two-level. Claude's `hooks.PreToolUse[i].hooks[j]` needs an object-in-nested-array primitive. `toml_splice` needs **nothing** new, because D7a chose own-a-file for Codex — `toml_splice` has no array-element function at all (`src/install/toml_splice.rs:51,92,120`), so splicing `[hooks]` in `config.toml` would have meant a second brand-new splice engine for one client. |
| `grim schema --kind hook` | C-014, four small edits. |
| `src/command/config.rs` — **new behaviour on an existing command** | `grim config set options.experimental.hooks false` must **refuse** with "run `grim install` to disarm" rather than silently leaving hooks armed until the next convergence (decision N, C-010). A config-write command must not run the installer itself, so refusal is the chosen half. `.claude/rules/subsystem-cli-commands.md` needs the row, and `docs/src/commands.md` the sentence. |
| Docs — the complete list, not a sample | **`docs/src/artifacts.md:17` is `## The five kinds {#kinds}`** — wrong *and* a linked anchor other pages target, so renaming has cross-reference consequences; `docs/src/clients.md` fifth column + Known-gaps prose; `src/install/client_target.rs:748-784` parity test; `docs/src/commands.md` gains `grim hook`; **`docs/src/configuration.md`** gains an `[options.experimental]` subsection (it documents `[options.tui]` and `[options.vendors]` but would leave the design's most load-bearing control undocumented); **`docs/src/vendor-metadata.md:186`** — "`hooks` … a separate ADR governs that surface" — is the forward pointer *this* ADR closes; `docs/src/concepts.md` kind/client taxonomy prose; `docs/src/json-interface.md` report shape for `grim hook list`; `docs/src/publishing.md`; `docs/src/package-index.md`; `docs/src/stability.md`. |
| **`catalog/` drift review — mandatory, and wider than first stated** | `catalog/README.md:109` names **six** trigger pages — `docs/src/{artifacts,clients,publishing,vendor-metadata,commands,package-index}.md` — plus `src/command/**` and `src/mcp/**`. A `Hook` kind touches at least four of them, so **all three** shipped skills fire: `grim-usage` and `grim-authoring` always, **and `ai-config-authoring` specifically because `clients.md` and `vendor-metadata.md` change**. `task catalog:verify` gates CI (`taskfile.yml:96`). Concrete stale sites, not a generic "review the skill": `catalog/skills/grim-authoring/SKILL.md:19` is `## The Five Kinds`, referenced as a `[five-kinds]` link anchor from `references/bootstrap-existing-repo.md:17`; `grim-usage` carries "It distributes five artifact kinds" in its **body** at `SKILL.md:15` — *not* in its description, which instead enumerates kinds by name ("skills, rules, agents, and bundles", already omitting `mcp`) and would need `hooks` added there and in `metadata.keywords`; `ai-config-authoring` already treats hooks as a concept in **six** places (`SKILL.md:32,42,49`, `references/guardrails.md:21`, `references/choosing-types.md:30,40`) — and `choosing-types.md:40` is the one this ADR **adopts** rather than rewrites (Decision M). |

**One claimed scope delta does not exist.** The survey states that
`vendor_codex.rs:106` "records that Codex hooks were rejected upstream". It does
not. The comment reads *"Codex has no path-scoped instruction mechanism (no
globs/applyTo anywhere; hooks cannot supply file-aware context). Rules are
declined"* — a statement about **rules**, fully consistent with the module doc
at `vendor_codex.rs:20-22` ("upstream hooks now accept `additionalContext` —
openai/codex#20692 — but that still cannot express path-glob-scoped rules").
Both comments are current and agree with each other. There is nothing stale to
fix, and no contradiction inside that file. Verified 2026-08-14 at `03e59b0`.

### Threat model

From `research_hooks_autoexec_supply_chain.md` §7, ranked by realistic
likelihood, each row naming the grim control that closes it. Severity per
`.claude/rules/quality-security.md`.

| # | Attack path | Likelihood | Control | Severity if unclosed |
|---|---|---|---|---|
| 1 | A hook is silently updated to a new digest between approval and the next run and keeps executing (the single most-repeated pattern in the prior art: tj-actions, trivy-action, Bun, Cursor create-vs-edit) | **High** | C-009: digest computed and compared **at the moment of invocation, every matched invocation**; mismatch refuses the hook, never warn-and-continue | Critical |
| 2 | A hook rewrites the approval/permission store to grant its own future runs blanket trust (the PromptArmor Claude Code marketplace pattern) | **High** for mutators | Decision E: the runtime path reads only a derived table that any grim command regenerates from version-controlled truth; approvals are global-only, hash-chained, fail closed; digest agreement verified at convergence and re-checked on-disk at exec | Critical |
| 3 | A mutator rewrites a shell command as a string and the executor re-parses it differently (`sudo` CVE-2023-22809) | **Medium-High** — depends entirely on the mutator API shape | **Decision K**: `mutator` is `Declined` for tools whose input is a shell-command string, enforced in the render-time permitted-field table and pinned by a test. The earlier "round-trip through the executor's parser" alternative is **deleted** — grim does not have the client's shell, and offering that branch invited an implementer to hand-roll a tokenizer, which is the CVE itself | High |
| 4 | Prompt injection drives the agent to author or modify a hook-governing file the approval flow has not seen (Cursor CVE-2025-54135 / CVE-2025-54130 / CVE-2025-59944 / CVE-2026-31854) | **Medium-High** — Cursor alone has 4+ CVEs of this shape in 18 months | C-009 approval keyed on content digest with **no create-vs-edit asymmetry**; every hook execution re-derives its identity from the on-disk bytes, so a newly created or case-varied file is unapproved by construction | Critical |
| 5 | Hooks default on, or the flag is easy to leave enabled without understanding the blast radius | **Medium** — the most consistently punished choice across 8 ecosystems | D10: off by default; enabling is an explicit scoped decision, not a global trust-everything toggle; the CI escape is itself audited | High |
| 6 | The audit log is poisoned via CRLF/ANSI injection in the attacker-shaped payload it records (CWE-117) | **Medium** | C-012: output-neutralize before writing; never render raw untrusted bytes into a log a human reads in a terminal | Medium |
| 7 | Audit log or approval store grows unbounded, then fails open or gets disabled under pressure (CWE-400/779) | **Medium** | C-012: rotation and size caps from day one; write failure fails closed | Medium |
| 8 | A secret in a tool call's input leaks through the hook's environment, argv, or logging rather than its intended function | **Medium** | C-002: stdin-only payload, non-secret-bearing flat env vars only, `payload = "file"` opt-in never default; C-012 redact-by-default | High |
| 9 | A project-scope approval arrives via `git clone` (pre-existing file, `git add -f`, or a `.gitignore`-unaware copy) and pre-arms every clone | **Low-Medium** | Decision E: approvals are **global-scope only** and never live in project state — the path is closed structurally, not by relying on the `.gitignore` grim writes only after first run | Medium |
| 9a | A cloned repository re-opens row 9 through a *different* file: a committed `grimoire.toml` carrying `hooks = true` **plus a pre-approved digest list**, arming on `grim install` with no human in the loop | **Medium** — trivially committable, and the panel found row 9's control did not cover it | **Decision N**: the CI escape (`--allow-hooks`, `GRIM_ALLOW_HOOKS=1`, the approved-digest list) is honoured from **global config or the invoking environment only**, never from a project `grimoire.toml`. A repository cannot carry its own permission to skip approval. The feature flag itself stays scope-resolved like every other `[options]` key, because it is not the control | High |
| 9b | A repository plants its own **dispatch table** (`subdir/.grimoire/hooks/dispatch.json` plus payload), which a cwd-relative lookup would load and arm | **Medium** — committable precisely because grim's `.gitignore` exists only after grim has run there | **Decision P**: one machine-local table at `$GRIM_HOME/hooks/dispatch.json`, located by an explicit `--root` in the launcher argv; the runtime refuses a table at any other path, and no arming input is ever derived from the client-supplied envelope `cwd` | Critical |
| 10 | A hook approved as part of a bundle stays trusted after the bundle's *composition* changes | **Low-Medium** | C-009: approval recorded per artifact at the artifact's own content digest, never at the bundle wrapper's; bundle members visible at `add` time and in the prompt | High |
| 11 | A registry serves different content for the same tag and an unlocked reference picks up the swap | **Low** | Never resolve a hook payload outside the locked digest path at execution time — the dispatch table carries the approved digest, not a reference | Critical if introduced |
| 12 | An observer-tier hook — output ignored, "safe" by design — is used as a reconnaissance/exfiltration channel, since it still sees the full tool-call payload | **Low-Medium** | Observer clears the **same** approval gate as gatekeeper and mutator: "output ignored" is not "input restricted", and the tier name must not imply a lower bar than the privilege it holds | Medium |

## Validation

- [ ] **Security review — required handoff to `/security-auditor` before any
      implementation merge.** Scope: RCA distribution, the approval store's
      equal-privilege problem, the mutator tier, foreign-config writes.
- [ ] Latency measured per the methodology in
      `research_hooks_hotpath_cost.md` §Q7: two numbers (no-match vs
      match-and-dispatch), p50 **and** p99, cold and warm stated separately,
      Linux / macOS / Windows / WSL2-native as distinct rows, `hyperfine`
      (`--warmup`, `-N` for sub-5 ms commands) plus `strace -c` to attribute
      rather than estimate.
- [ ] A test asserts `grim hook run`'s dispatch arm performs no scope
      resolution (C-007) — the property, not the convention.
- [ ] A pinned-set test asserts the exact hook-capable vendor set and per-tier
      verdicts (decision A).
- [ ] A test asserts exactly one dispatcher registration per
      `(client, event, scope)` across render modes (decision H).
- [ ] Acceptance tests: registration reversible, idempotent, edit-preserving;
      unmanaged keys untouched across install/update/uninstall/prune;
      re-materialize leaves `status` not-modified.
- [ ] Forward-compat: a hook-free config, lock, and declaration hash are
      byte-identical to pre-hook grim.
- [ ] A cloned repository carrying a registration with no payload and no
      approval fails open, exit 0.
- [ ] `task catalog:verify` passes after the `catalog/` drift review.

Added by the design panel (each closes a Block; the first six are new tests, not
restatements):

- [ ] **A clone carrying its own `subdir/.grimoire/hooks/dispatch.json` plus
      payload executes nothing** — the plant attack, closed by decision P's
      `--root` pin and the refusal of any table at another path.
- [ ] **Flag off ⇒ zero armed hooks after convergence** — an empty dispatch table
      and no registration on disk, so "off" is observable by the runtime
      (decision N). Plus: `grim config set` on that key either converges or
      refuses with an instruction.
- [ ] **A `gatekeeper` never observes pre-mutation input** — the decision O
      pipeline invariant: mutators serial and first, one final input to every
      gatekeeper, `deny` absorbing, `ask` outranking `allow`.
- [ ] **`mutator` on a shell-command-string tool is refused at render time**, not
      at runtime (decision K), and the permitted-field table rejects an
      unpermitted field as an error rather than dropping it silently.
- [ ] **An approval granted in one workspace does not arm the same digest in
      another** — the `(digest, scope root)` key (decision E.4).
- [ ] **The CI escape is ignored from project config** — a committed
      `grimoire.toml` carrying `hooks = true` plus an approved-digest list arms
      nothing (threat row 9a).
- [ ] The registration target is machine-local per decision I, and grim **warns
      when that target is not gitignored** rather than assuming the convention.
- [ ] An older `grim` meeting a hooks-bearing lock fails with a **clean,
      explanatory error naming the version requirement**, not a bare TOML parse
      failure (asserted on the message).
- [ ] A current `grim` with the flag **off** meeting a declared hook warns, skips,
      and reports `gated` — never errors.
- [ ] Cross-repo, `grimoire-vscode`: verified 2026-08-14 that an unknown `kind`
      normalizes to `null` and degrades (`webview/model.ts:100-102`) and that no
      runtime schema validator exists, so added fields are ignored. **Remaining
      work is cosmetic and belongs to that repo**: add `'hook'` to `ArtifactKind`,
      `KINDS`, `KIND_ICONS` so the row gains an icon and matches a kind filter.

## Explicitly out of scope

- **The `/security-auditor` handoff itself** — a separate, required gate, not
  part of this record.
- **The latency budget *number*.** No target is invented here, deliberately.
  The comparable tools (lefthook, starship, direnv, mise) publish none — they
  benchmark against what they replace, not an absolute figure. An implementation
  task must measure it; the research supplies the methodology, not the target.
- Agent-scoped hooks in Claude agent frontmatter — a second, genuinely different
  registration surface (`docs/src/vendor-metadata.md:228`); purely additive
  later, and deliberately not designed out.
- Non-command handler kinds (Claude `http`/`prompt`/`agent`/`mcp_tool`, Cursor
  `prompt`, Kiro `agent`): the LLM call *is* the handler, so there is no process
  for a dispatcher to stand in for. Native-only.
- The codegen install shape (opencode, kilo, amp, openclaw) — the trait variant
  exists in v1; the templates do not.
- Copilot's cloud agent, and a resident daemon.

## Open Questions

**All three resolved at acceptance, 2026-08-16 — each took its recommendation.**
They are kept in full rather than deleted, because two of them resolve to
"decline and verify against a live client", and the *reason* the decline is
honest is the evidence recorded below. WP-B owns both verifications.

- **RESOLVED → decline + verify in WP-B.** Ship Copilot `mutator` as `Declined`
  (cell `◐`) and settle the field name against a live Copilot CLI.
- **RESOLVED → refuse to arm on Windows** until WP-B verifies `commandWindows`
  (codex) and the `powershell` field (copilot), with a clear message rather than
  an unverified registration.
- **RESOLVED → redacted metadata only in v1**, appended to
  `$GRIM_HOME/state/hook_audit.jsonl` with rotation and a per-record size cap.
  Full-body capture lands later as an additive, separately-named flag.

The original wording of each, with its evidence:

- **[RESOLVED: Copilot's mutator field name under PascalCase
  registration.]** The primary report documents `preToolUse` returning
  `modifiedArgs` (`hooks_vendor_reports/copilot.md` §7), while
  `research_hooks_trampoline.md` § "Mutator tier" lists Copilot's field as
  `updatedInput`, citing `github/copilot-cli#2013` — whose own title is
  "`updatedInput` ignored". The two inputs disagree, the VS Code surface
  documents no input-rewrite field at all, and the field name changes with
  config casing. *Recommended: ship Copilot `mutator` as `Declined` in v1 (cell
  `◐`) and verify against a live CLI before enabling — an honest decline beats a
  rewrite whose meaning grim cannot verify, and the projection rule already
  forbids guessing.*
- **[RESOLVED: the Windows launcher invocation form.]** How each of
  the three v1 clients invokes a non-`.exe` launcher on Windows is
  `NOT DOCUMENTED` in anything surveyed; Copilot carries separate
  `bash`/`powershell` command fields, Claude and Codex do not, and `CreateProcess`
  does not exec a `.cmd` directly. *Recommended: generate a `.cmd` beside the
  POSIX shim and register the `.cmd` path, verified per client on Windows CI
  before hooks are enabled there; until verified, the experimental flag refuses
  to arm on Windows with a clear message rather than registering something
  unverified.*
- **[RESOLVED: does the full-body audit mode ship in v1, and where
  does the trail live?]** C-012 makes the redacted view the default; the
  stricter mode's sink, retention and access model are unspecified.
  *Recommended: v1 ships redacted metadata only, appended to
  `$GRIM_HOME/state/hook_audit.jsonl` with rotation and a per-record size cap;
  full-body capture lands later as an additive, separately-named flag — every
  mature audit system surveyed treats unredacted capture as an opt-in level, and
  shipping it by default would create the secret-bearing log the research
  identifies as the weakest control in the original plan.*

## Links

- [`research_hooks_trampoline.md`](../research/research_hooks_trampoline.md) — grim-side findings F1–F13, decisions D1–D10, the projection rule, mutator controls
- [`research_hooks_vendor_survey.md`](../research/research_hooks_vendor_survey.md) — the 17-client survey; **supersedes [`research_ide_hooks.md`](../research/research_ide_hooks.md) for all vendor facts** (that survey covers Windsurf / Continue / Aider, which are not grim clients, and omits 9 of the 17 — its portability and security *analysis* is retained and carried forward here)
- [`research_hooks_codex_surface.md`](../research/research_hooks_codex_surface.md) — D7a evidence, source-verified against `openai/codex` `main` @ `8630bb3c`
- [`research_hooks_hotpath_cost.md`](../research/research_hooks_hotpath_cost.md) — the latency-budget *shape* and the measurement methodology
- [`research_hooks_autoexec_supply_chain.md`](../research/research_hooks_autoexec_supply_chain.md) — consent-model prior art and the threat-model checklist
- [`hooks_vendor_reports/`](../research/hooks_vendor_reports/) — the 17 per-client primary-source reports
- [`adr_render_layout_stability.md`](./adr_render_layout_stability.md) — **Accepted**: render layout outside the 1.0 contract (§1), plugin render mode (§2), `render` key reserved (§4)
- [`adr_agent_artifact_kind.md`](./adr_agent_artifact_kind.md) — the template for adding a kind additively
- [`adr_structured_vendor_metadata.md`](./adr_structured_vendor_metadata.md) — settles that hooks get a dedicated kind, not a `FieldType::Json` metadata field
- [`adr_vendor_config_and_selection.md`](./adr_vendor_config_and_selection.md) — the `sync_config` / detection / pool-refcount context a hook registration inherits
- [`adr_vendor_wave_expansion.md`](./adr_vendor_wave_expansion.md) — decline-first and the `KindSupport` tri-state
- [`adr_client_compat_matrix.md`](./adr_client_compat_matrix.md) — the matrix-plus-parity-test contract the Hook column joins
- [`adr_install_state_portability.md`](./adr_install_state_portability.md) — `PathAnchor`, containment, the single `persist` seam
- [`adr_multifile_rules.md`](./adr_multifile_rules.md) — index + sibling directory folded into one integrity hash
- [`adr_oci_empty_config_compat.md`](./adr_oci_empty_config_compat.md) — wire format for a new kind
- [`adr_effective_set_mutations.md`](./adr_effective_set_mutations.md) — how a bundle-delivered hook reaches the desired set
- [`plan_hooks_artifact_kind.md`](../plans/plan_hooks_artifact_kind.md) — the plan derived from this ADR; **normative for contracts C-015…C-026**, threat rows 13/14 and the row 1/2/4/10 re-controls
- [`adr_hook_workspace_consent.md`](./adr_hook_workspace_consent.md) — **partially reverses A2 and restores Decision E point 4's directory binding**; supersedes C-022, annotates Key insight 6, and records the I2 relationship
- [`arch-threat-model.md`](../../.claude/rules/arch-threat-model.md) — **the trust boundary this ADR's threat model was extracted into**: T1–T5 in scope, N1–N5 non-goals (N1 = insiders with commit access), invariants I1–I6. A1 is I1 applied; A3 is a recorded trade against I2, resolved as I5.
- `.claude/rules/quality-security.md` · `.claude/rules/arch-principles.md` · `.claude/rules/subsystem-file-structure.md` · `.claude/rules/subsystem-cli-api.md` · `.claude/rules/vendor-capability-watchlist.md`

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-08-28 | Michael Herwig + Claude (hex swarm) | **A2 partially reversed.** Its registry half — `trust_hooks`, the `[[registries]]` grant, the tap-trust precedent — is replaced by per-workspace consent ([`adr_hook_workspace_consent.md`](./adr_hook_workspace_consent.md)), which **keeps** A2's coarseness in full and restores the half of Decision E point 4 that A2 deleted as collateral: the directory binding, per [`direnv/direnv#83`](https://github.com/direnv/direnv/issues/83). Deleting both halves had left **T3** uncovered. Contract C-022 superseded outright with its B4/B5/B7/W8/S2-2 amendments; C-023 amended (subject registry → workspace). Key insight 6 annotated: it dismissed Claude's folder trust as "coarser" *before* A2 chose coarseness deliberately, so the folder axis is now the adopted precedent and Gemini stays the precedent for the content axis grim no longer walks. |
| 2026-06-03 | Architect (/architect) | Initial draft — Claude-first native registration (Option 2), evolving to translation (Option 3) |
| 2026-08-16 | Maintainer | **Accepted**, with five amendments folded in (§ "Amendments accepted 2026-08-16") and all three Open Questions resolved to their recommendations. Two are owner reversals of this ADR's own frozen inputs: **A2 reverses D5** from per-hook digest approval to registry-scoped trust — deleting `hook_approvals.json`, the hash chain and the per-artifact key — and **A3 drops the exec-time digest re-check**, leaving one in-scope residual covered as tamper-evidence rather than prevention. **A1 restores Decision I** (project scope Claude-only), reversing the widening the owner directed at the planning gate, on evidence the panel produced afterwards. A4 names `GRIM_EXPERIMENTAL_HOOKS` distinct from `GRIM_ALLOW_HOOKS`; A5 makes the shim `exec` a recorded absolute path. The general boundary these rest on was extracted to the shared rule [`arch-threat-model.md`](../../.claude/rules/arch-threat-model.md) (T1–T5 in scope, N1–N5 non-goals, invariants I1–I6). |
| 2026-08-14 | Orchestrator (/hex-architect, round 2) | **Re-validation findings applied** (owner-authorised extra round; plan-artifact scope normally allows one). The first fix pass amended *decisions* but left *contract* text, and `reviewer:spec` correctly rejected a precedence note as a substitute for an edit — a contract is what a test is generated from. Five Blocks closed: **F-1** Copilot moves to `~/.copilot/hooks/grim.json` (stays `OwnFile`; the earlier `settings.local.json` route cited a line that recommends against it, and would have made Copilot a second splice client); **F-2** project-scope hooks are **Claude-only in v1** — a user-level registration cannot carry a per-project `--root`, so Codex and Copilot are global-scope-only, stated as a scope reduction rather than left contradictory; **F-3** wholesale regeneration is **per root key**, and E.1's revert guarantee is rescoped to "the next mutating command *in that workspace*"; **F-4** the H invariant becomes per `(client, event, scope, matcher)` and the two "hook #2 touches no vendor config" claims are qualified; **F-5** all six amendments applied in place, C-010 and C-013 rows added, Component diagram redrawn. Item 9 closed by scoping `✓` to tiers *valid at that event* plus a new F rule rejecting `gatekeeper` on a verdict-less event, and by deleting the prose assertion that claude and codex earn `✓` (it is computed from C-004). Also: `--scope` argv → `--root`, `src/command/config.rs` scope-delta row (F-6), and the `grim-usage` "five artifact kinds" location corrected (F-7). |
| 2026-08-14 | Orchestrator (/hex-architect, panel fix pass) | **Design panel + owner decisions applied.** Five perspectives (spec, quality, security, SOTA gap check, docs) returned **14 Block** findings; ten were one coupled problem in three clusters — what the runtime may read, what travels with a repository, and how the tiers compose. Nine decisions added (**I–Q**): machine-local registration, matcher hybrid push-down, `mutator` declined per tool shape, registrations as `sync_config` projections, hooks-are-defence-in-depth, the flag needing no new precedence rule, the tier-composition invariant, one machine-local dispatch table located by `--root`, and a reserved `policy` key. Amended: decisions B, C, E.2, E.4, F, G, plus contracts C-004/006/007/009/011 (numbers preserved, overridden by named decision). `fail` removed from `hook.toml` as unimplementable. Threat model gains rows 9a/9b. Validation gains ten items. Two corrections to this run's own research: Copilot's rewrite field is `modifiedArgs` not `updatedInput` (so Copilot `mutator` ships `Declined`), and the survey's claim that `vendor_codex.rs:106` is stale is **retracted** — it is correct as written. `grimoire-vscode` verified forward-compatible; remaining work there is a three-line cosmetic addition. |
| 2026-08-14 | Architect (/hex-architect) | **Rewritten in place.** The 2026-06-03 draft was never Accepted and three of its premises no longer hold: its vendor table surveys Windsurf / Continue / Aider (not grim clients) and omits 9 of grim's 17; the reversible foreign-config registration engine it treats as unproven has since shipped (`Vendor::sync_config`, whose doc comment names the hooks ADR as the pattern's origin); and it considers no runtime dispatcher, which wins by 30 weighted points against its own chosen option. Its "activation, not placement" framing and its security analysis are carried forward. Rewritten rather than superseded because nothing depended on the draft and git history preserves it. New in this revision: the five-option trade-off matrix, C-001…C-014 contracts, the per-`(vendor, event)` response projection table with Codex's fail-closed reserved fields, the eight added decisions (hook-support opt-in seam, payload once per scope, the `clients.md` Hook cell, the fixed-path launcher, the approval-store answer, the `hook.toml` schema, the never-fail-through-exit-code policy, plugin-render-mode composition), the threat model, and the correction that `vendor_codex.rs:106` is **not** stale. |
