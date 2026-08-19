---
paths:
  - "src/oci/**"
  - "src/install/**"
  - "src/command/hook*"
  - "src/command/login.rs"
  - "src/command/logout.rs"
  - "src/command/publish*"
  - "src/command/release*"
  - "catalog/**"
---

# Grimoire Threat Model

**The trust boundary, stated once.** Every security finding in this project is scoped by this
file. Without a written boundary, reviews churn: the same finding gets raised as a Block by one
reviewer and dismissed as a non-goal by the next, and neither is wrong because nobody said which
attacker we defend against.

> **Scope note:** `src/command/hook*` is now in `paths:` above — `src/command/hook.rs` and its
> runtime submodules exist, so the glob matches. It was deliberately absent until then, because a
> glob matching nothing is a rule that silently never fires and
> `test_all_rule_globs_match_files` enforces that. The glob covers the dispatcher runtime, whose
> whole design is this file's invariants: **I1** (nothing armable inside a repository), **I3** (grim
> degrades to "the feature is off", never to "the agent is blocked" — which is why every refusal on
> the dispatch path exits 0), and **I6** (secrets never in argv or the environment).

Read this **before** filing or triaging a security finding, and before designing anything that
executes code, writes into a file grim does not own, or reads content from a registry.
Companion: [`quality-security.md`](./quality-security.md) (the OWASP/STRIDE checklist and attack
surfaces). This file says *whom* we defend against; that one says *how*.

## What grim is

A package manager that fetches artifacts from OCI registries and materializes them into
AI-agent config on a developer's machine. As of the hook kind it also **arms code that a client
executes automatically**. So grim sits on a supply chain, and its risk class is the supply
chain's.

## In scope — attackers we defend against

| # | Attacker | Why it is in scope |
|---|---|---|
| **T1** | **A malicious or compromised artifact publisher.** Content arrives from a registry the user may not control; a legitimate package may be taken over. | This is grim's core exposure. Named incidents across eight ecosystems (npm, RubyGems, PyPI, Homebrew, VS Code, Cargo `build.rs`, GitHub Actions, Bun) are all this attacker. |
| **T2** | **A mutable-identity swap.** The same reference resolving to different bytes — a force-pushed tag, a re-pushed digest-less ref. | CVE-2025-30066 (tj-actions, 23,000+ repos), trivy-action (75 of 76 tags force-pushed), Bun's name-only trust (CVE-2026-24910). Content-digest resolution is grim's structural answer; anything that resolves outside it reopens the class. |
| **T3** | **An untrusted repository the user clones or opens.** Cloning a dependency, a fork, a contributor's branch, a sample repo, or opening any of them in an AI client. | The user is *not* vouching for a repo by cloning it. Cursor has 4+ CVEs of this exact shape in 18 months. Anything armable that travels in a repository is exposed to this attacker. |
| **T4** | **Prompt injection driving the agent.** Untrusted content in a file, a diff, a web page or a tool result steering the agent into authoring or modifying config. | PromptArmor's Claude Code marketplace attack is precisely this: an injected payload drove a hook that rewrote the permission store. The agent is a confused deputy holding the user's privilege. |
| **T5** | **Another local unprivileged process or user on a shared machine.** Reading `/proc/<pid>/cmdline`, `/proc/<pid>/environ`, crash dumps, CI logs, or a world-readable state file. | Cheap to defend, and it is why secrets never travel in argv or environment variables. |

## Out of scope — attackers we deliberately do **not** defend against

Naming these is as load-bearing as naming the ones above. A finding whose only attacker is one of
these is a **non-goal**, not an open defect — record it as such and move on.

| # | Non-goal | Why |
|---|---|---|
| **N1** | **An insider with commit access to the consumer's own repository.** A teammate who can land a commit or merge a PR. | They can already execute code on every machine in the fleet through CI, a `taskfile`, a test, a build script, or a devcontainer. Grim cannot be a meaningful boundary against someone who holds write access to the code the developer runs, and pretending otherwise buys friction without security. **Corollary:** "a malicious edit could be landed through a reviewed PR" is not a grim defect. Code review and branch protection own that. |
| **N2** | **The user's own machine compromised at or above grim's privilege.** | Hooks and grim run at user privilege. A process at that level can rewrite any file grim can, including grim's own state. Grim provides **tamper-evidence**, not tamper-resistance — see the invariant below. |
| **N3** | **A malicious AI client.** Grim registers into vendor config and trusts the vendor binary to honour its own documented contract. | If the client is hostile, it does not need grim. |
| **N4** | **A user deliberately bypassing a gate they were shown.** `--force` on an untracked destination, `--allow-hooks` in their own CI, approving a hook they did not read. | Consent means the user may consent. The obligation is that the prompt be **honest and legible**, not that it be unbypassable. |
| **N5** | **Denial of service against the developer's own tooling by their own configuration.** A hook the user installed that is merely slow. | Ordinary misconfiguration. Distinct from grim *itself* causing a denial — that is in scope (see I3). |

## The invariants these produce

Design against these directly; they are what the boundary is *for*.

1. **I1 — Nothing armable lives inside a repository.** Anything that causes execution — a
   registration, a dispatch table, an approval record, a launcher, **and the payload that is
   executed** — lives in machine-local storage (`$GRIM_HOME`) or in a per-developer repo-resident
   surface the client treats as local (Claude's `.claude/settings.local.json`).

   > **The payload is named explicitly because omitting it was read as a licence.** Until
   > 2026-08-18 this list stopped at "a launcher", and hook payloads were materialized at
   > `<workspace>/.grimoire/hooks/<name>` on the stated reasoning that "nothing here is armable;
   > the registration is". WP-R falsified that by execution (SEC-1): a repository carrying its
   > **own committed** `state.json` *and* payload armed on a fresh machine **offline**, with no
   > fetch and no install history, because the integrity gate compares the *recorded* hash against
   > the on-disk payload — the attacker supplies both — and convergence then read the manifest out
   > of the directory the record named. Payloads now live under `$GRIM_HOME` at both scopes, and
   > the arming path derives that directory from `$GRIM_HOME` rather than from the record. **A
   > thing is armable if grim will execute it or read arming instructions out of it**; the absence
   > of a category from this list is not permission.

   This is T3's control, and
   it is why a "portable, committable" form of an executed path is not an acceptable trade: an
   executed path assembled from environment (`$GRIM_HOME`, `$PATH`, `$HOME`) in a committed file
   is CWE-426, and `.envrc` / `.mise.toml` / `devcontainer.json` `containerEnv` are ordinary repo
   files. **A gitignore rule is not part of this control** — it governs `git add` / `git status`, not
   reads, so an attacker's committed file is read whatever any ignore rule says. Ignoring a local
   registration is hygiene (it stops the *user* publishing their own arming); what actually holds T3
   is the absolute executed path, the ownership marker, and digest-pinned approval.
2. **I2 — Identity is content, and it is re-checked at the moment of use.** Approval and trust key
   on a content digest, never on a name, tag or version, and the digest is verified against the
   bytes on disk immediately before they are used — not only at install time. T2 and T4 are both
   "approved the right thing, checked at the wrong time".
3. **I3 — Grim fails in the direction that does not block the user.** An internal error, a missing
   binary, an unparsable file or an unknown schema version degrades to "the feature is off", never
   to "the agent is blocked". This is a genuine availability obligation, because some clients fail
   *closed* on a non-zero hook exit.
4. **I4 — Default-deny for anything that executes.** The only control with a causal track record
   across every ecosystem surveyed is a **default flip**; attestation and provenance layers added
   afterwards did not prevent named incidents. New execution capability ships off by default.
5. **I5 — Tamper-evidence, not tamper-resistance, and say which.** Under N2 grim cannot prevent a
   same-privilege process from editing its files. It can ensure that no such edit *arms* anything
   without a subsequent grim command whose inputs are version-controlled and visible in
   `git diff` / `grim status`. Never describe a control as prevention when it is evidence.
6. **I6 — Secrets never travel in argv or the environment.** T5. Payloads go on stdin; env carries
   non-secret scalars only.

## Applying it

- **Filing a finding:** name the attacker (T1–T5). If the only attacker is N1–N5, it is a
  non-goal — say so explicitly rather than filing it silently or dropping it silently.
- **Reviewing a design:** check each invariant that the change touches. A change that moves
  something armable into a repository, or that keys trust on a name, is a Block regardless of how
  it is mitigated.
- **Writing an ADR:** scope the threat table to this file rather than re-deriving a boundary, and
  record any deliberate deviation with the attacker it exposes you to.

## See also

- [`quality-security.md`](./quality-security.md) — attack surfaces, OWASP/STRIDE checklist
- [`arch-principles.md`](./arch-principles.md) — boundaries, invariants, glossary
- [`vendor-capability-watchlist.md`](./vendor-capability-watchlist.md) — dated re-verification of
  vendor declines, since a vendor's security posture is an input to these invariants
- `.agents/adr/adr_hooks_support.md` § Threat model — the worked example this file was extracted
  from; `.agents/research/research_hooks_autoexec_supply_chain.md` carries the incident evidence
  behind T1, T2 and I4
