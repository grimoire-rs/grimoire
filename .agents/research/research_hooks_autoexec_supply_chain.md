# Research: Auto-exec supply-chain risk for grim hooks (observer / gatekeeper / mutator)

## Metadata

**Date:** 2026-08-14
**Domain:** security | packaging | integration
**Triggered by:** tier-high hex-architect run on the hooks feature — this axis feeds a mandatory
`reviewer:security` pass on the ADR that will decide the consent model, mutator-tier controls,
and audit design for `grim`'s three hook tiers.
**Expires:** 2026-11-14 (three months — the AI-coding-agent hook surface and its incident history
are both moving fast; re-verify against [`research_hooks_vendor_survey.md`](./research_hooks_vendor_survey.md)
and any new CVE/GHSA activity before reuse)
**Companion artifacts:** [`adr_hooks_support.md`](../adr/adr_hooks_support.md) (Proposed,
2026-06-03 — the shipping-shape decision this axis's findings feed into),
[`research_hooks_trampoline.md`](./research_hooks_trampoline.md) (grim-side machinery),
[`research_hooks_vendor_survey.md`](./research_hooks_vendor_survey.md) (17-client hook survey)
**Method:** Primary sources first (vendor docs, CVE/GHSA records, official postmortems, academic
papers). Blog/vendor-marketing sources labeled `[unofficial]`. Every claim carries a URL and a
fetch date (today: 2026-08-14). `NOT DOCUMENTED` recorded explicitly rather than guessed.
Research on Q1–Q4 and Q6 was gathered by four parallel research passes (package-manager incident
history, consent/trust-model evidence, mutator-tier/prompt-injection prior art, audit-logging and
subprocess-secrets guidance); Q5 and Q7 were derived directly from grim's own source
(`src/install/install_state.rs`, `src/install/path_anchor.rs`, `docs/src/configuration.md`) and
synthesized against Q1–Q6.

## Direct Answer

**Three controls I would refuse to ship the mutator tier without:**

1. **Default-deny execution, not default-deny-with-an-escape-hatch.** Every ecosystem surveyed
   that shipped "silent execute by default" (npm, RubyGems, PyPI, Homebrew third-party taps, VS
   Code extensions, Cargo `build.rs`) has since eaten at least one real, named incident traceable
   directly to that default — and the fix that *actually changed outcomes* was always a default
   flip (pnpm 10, npm v12, Homebrew 6.0 Tap Trust), never an attestation or logging layer bolted
   on after the fact (npm provenance/2FA did not stop the 2025–2026 Shai-Hulud worm or the 2026
   TanStack compromise). Grim's plan to gate hooks behind an experimental flag, off by default, is
   the right instinct — the ADR needs to say explicitly that this is not a launch-phase
   convenience but the single most load-bearing control in the whole design, based on the
   strongest pattern in this research.
2. **Approval bound to a content digest of the exact thing that executes, verified again at
   the moment of execution — not the moment of install.** The recurring, repeated real-world
   failure across completely unrelated systems (GitHub Actions tag mutation → CVE-2025-30066,
   `trivy-action` force-push, Bun's name-based trust bypass → CVE-2026-24910, Cursor's
   create-vs-edit approval gap → CVE-2025-54135/54130, its case-normalization bypass →
   CVE-2025-59944) is **an approval keyed on the wrong identity, checked at the wrong time**. A
   digest is structurally immune to the mutable-tag and name-collision variants of this bug, but
   only if grim (a) hashes the artifact that will actually run, not a bundle wrapper or a
   declared version, and (b) re-checks the digest against the file on disk immediately before
   `exec`, closing the TOCTOU gap between "approved" and "ran" — see Q5/Q7 below for the specific
   grim code paths this touches.
3. **The mutator's output is re-validated by the same command-line parser the eventual executor
   uses, not a second, hand-rolled one.** This is the direct, general lesson of `sudo`'s
   CVE-2023-22809: a trusted layer that inspects or rewrites a command line and then hands the
   result to a *different* parser than the one that finally executes it reliably develops exactly
   this bug class. A mutator that receives a shell command as a string, edits the string, and
   emits a new string for the shell to parse independently is not just risky in the abstract — it
   is the precise, historically-repeated shape of a real, patched CVE.

**The one planned control I think is weakest: mandatory before/after audit logging, as currently
scoped.** It is necessary but is being asked to do more work than logging can do. Concretely:
audit logging has *zero* documented before/after evidence of reducing incidents anywhere in this
research — it is forensics, not prevention, and every mature audit system surveyed
(Kubernetes, CloudTrail, `auditd`) treats it as exactly that, a *record* of a decision already
made elsewhere. Worse, the specific plan — log the tool call's mutated input, which may carry a
secret — creates a new secret-exposure and log-injection surface (CWE-117, CWE-400) unless the
log defaults to a redacted/metadata view and gates full-body capture behind a stricter, separately
audited mode, the way Kubernetes' `Metadata`/`Request`/`RequestResponse` levels and CloudTrail's
field truncation both do by design. As currently described ("mandatory before/after audit of
every mutation," full stop) the plan risks becoming exactly the control every mature system
avoids: an unbounded, unredacted, secret-bearing log a hostile mutator's own output could poison
via CRLF/ANSI injection.

---

## 1. Prior art: package managers shipping auto-executing code

### npm `install`/`postinstall` scripts — closest analogue

**Consent model.** Silent, default-on for every direct and transitive dependency; opt-out via
`--ignore-scripts`, not a default. About 2% of the registry ships a postinstall script.
[nodejs-security.com, npm ignore-scripts guide](https://www.nodejs-security.com/blog/npm-ignore-scripts-best-practices-as-security-mitigation-for-malicious-packages), fetched 2026-08-14 `[unofficial]`.

**Incidents (all primary-sourced):**
- **event-stream** (Sept–Nov 2018) — trusted-maintainer takeover via social engineering, not a
  fresh-published malicious script; a targeted Bitcoin-wallet payload shipped inside the Copay
  wallet app. npm removed the package and reclaimed the name.
  [npm Blog postmortem](https://blog.npmjs.org/post/180565383195/details-about-the-event-stream-incident), fetched 2026-08-14.
- **eslint-scope** (2018-07-12) — compromised npm account (reused password, no 2FA); malicious
  version fetched a Pastebin payload that exfiltrated `.npmrc` publish tokens.
  [ESLint official postmortem](https://eslint.org/blog/2018/07/postmortem-for-malicious-package-publishes/), fetched 2026-08-14.
  GHSA: [GHSA-hxxf-q3w9-4xgw](https://github.com/advisories/GHSA-hxxf-q3w9-4xgw).
- **ua-parser-js** (2021-10-22) — compromised account; install-time script dropped an XMRig
  miner plus (Windows) a credential-stealing trojan. CVE-2021-4229 /
  [GHSA-pjwm-rvh2-c87w](https://github.com/advisories/GHSA-pjwm-rvh2-c87w) (CVSS 8.8), fetched 2026-08-14.
- **coa and rc** (Nov 2021) — compromised account(s); postinstall dropped a DanaBot variant.
  [Rapid7](https://www.rapid7.com/blog/post/2021/11/05/new-npm-library-hijacks-coa-and-rc/) `[unofficial]`, fetched 2026-08-14.
- **node-ipc / peacenotwar** (March 2022) — **not** account compromise: the legitimate maintainer
  intentionally shipped a geo-IP-triggered destructive payload. CVE-2022-23812 /
  [GHSA-97m3-w2cp-4xx6](https://github.com/advisories/GHSA-97m3-w2cp-4xx6), fetched 2026-08-14.
  Load-bearing for the ADR: **a legitimate publisher turning malicious is a threat-model category
  that signing/provenance does not address at all.**
- **"Shai-Hulud" self-replicating worm** (Sept 2025, second wave Nov 2025) — phishing →
  compromised account → postinstall scripts that scanned for GitHub/AWS/GCP/Azure credentials and
  auto-republished trojanized versions of every other package reachable with the stolen token.
  500+ packages first wave, 600–800 packages / 25,000+ repos second wave.
  [CERT/CC VU#534320](https://www.kb.cert.org/vuls/id/534320),
  [Cybersecurity Dive](https://www.cybersecuritydive.com/news/cisa-dependency-checks--shai-hulud-compromise/761018/), fetched 2026-08-14.

**Ecosystem changes, in the order that actually changed the execution default:**
1. npm token revocation (2018, reactive, one-time) and `npm audit` (npm 6, 2018, detection only).
2. Mandatory 2FA rollout for top-package maintainers, phased Dec 2021 → Q3 2022 — addresses
   *account takeover*, not the "legitimate publisher turns malicious" or "worm re-publishes with a
   stolen but valid token" cases. [GitHub Blog](https://github.blog/security/supply-chain-security/top-100-npm-package-maintainers-require-2fa-additional-security/), fetched 2026-08-14.
3. **`npm publish --provenance`**, GA Oct 2023, Sigstore-backed. Attests build provenance; does
   **not** restrict what a published install script may do.
   [GitHub Blog](https://github.blog/security/supply-chain-security/introducing-npm-package-provenance/), fetched 2026-08-14.
4. **pnpm 10.0.0** (shipped 2025-01-10) — the first mainstream flip of the *execution default
   itself*: lifecycle scripts blocked unless explicitly approved (`pnpm approve-builds` /
   `onlyBuiltDependencies`). Named, dated trigger: a malicious postinstall script in `rspack`.
   [Socket.dev](https://socket.dev/blog/pnpm-10-0-0-blocks-lifecycle-scripts-by-default) `[unofficial]`, fetched 2026-08-14.
5. **npm v12** (announced June 2026, ~July 2026 target) — npm following pnpm's lead: install
   scripts and git/remote-URL dependencies become opt-in. GitHub's own framing: lifecycle scripts
   are "the single largest code-execution surface in the npm ecosystem."
   [TheHackerNews](https://thehackernews.com/2026/06/github-to-disable-npm-install-scripts.html), fetched 2026-08-14 — trade press, treat exact date as provisional pending an npm-authored confirmation.

**Reading for the ADR:** 8 years of attestation-and-account-hardening (2018–2023) did not stop
new incidents in 2021, 2022, or 2025 — only a 2024–2026 default-execution flip did. This is
strong evidence for shipping hooks default-deny from day one rather than defaulting open and
hardening later.

### Python `setup.py` / PEP 517-518

Silent, default-on, same as npm. **PEP 517/518 (2016–2017) did not close the arbitrary-execution
hole** — a build backend can still be, and very often still is, arbitrary Python (`setup.py`
itself). [Veracode](https://www.veracode.com/blog/python-package-installation-attacks/) `[unofficial]`, fetched 2026-08-14, states plainly that a declarative
manifest format only helps once no entry point still offers a `setup.py` fallback.
Incident: PyPI mass typosquatting campaign (March 2024) — 500+ packages with malicious
`setup.py` payloads (zgRAT malware family), executed automatically on `pip install`; PyPI
suspended new project/account creation registry-wide on 2026-03-28 as a circuit-breaker.
[Checkmarx](https://checkmarx.com/blog/pypi-is-under-attack-project-creation-and-user-registration-suspended/) `[unofficial]`, fetched 2026-08-14. No CVE/GHSA found for the campaign — **NOT DOCUMENTED** at
that level.
**Lesson**: a declarative-manifest reform alone does not eliminate arbitrary execution if any
backend can still shell out — directly relevant to grim's canonical hook-frontmatter design,
which must not let a `command:` field become an unbounded escape hatch around whatever gating the
frontmatter otherwise implies.

### RubyGems native extensions (`extconf.rb`)

Silent, default-on; no `--ignore-scripts` equivalent found (**NOT DOCUMENTED** / apparently absent).
[RubyGems Guides](https://guides.rubygems.org/gems-with-extensions/), fetched 2026-08-14.
Incident: ~760 typosquatted gems (reported April 2020) shipped a payload disguised as a `.png`
that the extension-build `Makefile` renamed to `.exe` and executed — a clean demonstration that
"native extension build step" is an unrestricted code-execution primitive.
[The Hacker News](https://thehackernews.com/2020/04/rubygem-typosquatting-malware.html) `[unofficial]`, fetched 2026-08-14.
Ecosystem response: mandatory MFA for top-gem maintainers (enforced from 2022-08-15) — again,
account-hardening only; **no evidence found of an execution-default change.**
[RubyGems official blog](https://blog.rubygems.org/2022/08/15/requiring-mfa-on-popular-gems), fetched 2026-08-14.

### Homebrew formulae — the single most directly relevant precedent

Formulae are Ruby DSL scripts executed at full user privilege with no per-install prompt, but
`homebrew/core`/`homebrew/cask` are centrally reviewed and mostly distributed as precompiled
CI-built "bottles" — most users never actually run the arbitrary upstream Ruby for core formulae.
**Third-party taps** (`brew tap`) were, until mid-2026, arbitrary unsandboxed Ruby with zero
central review — a structural analogue to grim installing hooks from an arbitrary OCI registry.
[Homebrew official docs](https://docs.brew.sh/Homebrew-Security-and-Supply-Chain), fetched 2026-08-14.

Incidents:
- **2021-04-18/19** — `homebrew-cask`'s `review-cask-pr` + `automerge` Actions auto-approved and
  auto-merged any PR touching only a cask's version string — an attacker could bump a cask to a
  URL/checksum they controlled with **zero human review**. Fixed within 24 hours by removing both
  Actions. [Homebrew official disclosure](https://brew.sh/2021/04/21/security-incident-disclosure/), fetched 2026-08-14.
- **2026-03-19** — Trivy scanner compromise: a compromised maintainer published a malicious Trivy
  release and simultaneously compromised a **custom (unofficial) Homebrew tap**; Homebrew's own
  official formula was unaffected. CVE-2026-33634 /
  [GHSA-69fq-xp46-6x23](https://github.com/advisories/GHSA-69fq-xp46-6x23), fetched 2026-08-14.

**Ecosystem change — the most on-point precedent in this entire survey:** **Homebrew 6.0.0
"Tap Trust" (released 2026-06-11)** requires third-party taps to be **explicitly trusted before
Homebrew will even evaluate their Ruby code**; official taps remain trusted by default, everything
else is opt-in. [Homebrew official release notes](https://brew.sh/2026/06/11/homebrew-6.0.0/), [Tap Trust docs](https://docs.brew.sh/Tap-Trust), fetched 2026-08-14. This is a
package manager that executed arbitrary interpreted code from third-party sources by default,
took a real incident hit, and pivoted to an explicit-trust-before-evaluation gate — the exact
decision shape grim faces now, pre-emptively rather than reactively.

### VS Code extensions

Silent activation-event execution, full editor-process privilege, no sandbox. Marketplace review
is automated malware scanning plus a "verified publisher" badge that proves *domain ownership*,
not code safety.
[VS Code official docs](https://code.visualstudio.com/docs/configure/extensions/extension-runtime-security), fetched 2026-08-14.
Multiple real, dated incidents (Dracula-theme clone trojan, June 2024, 100+ orgs infected before
takedown; "Material Theme" pulled Feb 2025 at ~9M combined installs; 9 malicious extensions /
300K+ installs in 3 days, April 2025, XMRig via PowerShell loader).
[BleepingComputer](https://www.bleepingcomputer.com/news/security/vscode-extensions-with-9-million-installs-pulled-over-security-risks/), [CSO Online](https://www.csoonline.com/article/3956464/warning-to-developers-stay-away-from-these-10-vscode-extensions.html), fetched 2026-08-14.
Independent research (ExtensionTotal) claims 1,283 extensions with known-malicious code totaling
229M installs slipped the review gate — `[unofficial, vendor-research-sourced, not independently
corroborated]`. **No evidence found of a structural default-execution change** analogous to
npm v12/pnpm 10/Homebrew Tap Trust — VS Code extensions still fully auto-activate with no
allowlist/opt-in gate as of this research. Flag as **NOT DOCUMENTED / apparently unaddressed** —
this is the weakest current security posture among ecosystems with a live registry/review model.

### `pre-commit` hook repos — the closest structural analogue to grim

This is the ecosystem most worth reading closely, because it is **literally "distribute hooks
from a registry into a repo."**

- **Pin-by-mutable-name, not content.** `.pre-commit-config.yaml`'s `rev:` field pins a git tag or
  SHA; the docs *assume* `rev` is immutable but this is a documentation convention, not an
  enforced guarantee — a git tag is force-pushable by the upstream owner (unlike a content
  digest). [pre-commit.com official docs](https://pre-commit.com/), fetched 2026-08-14.
- **No sandboxing whatsoever.** The per-language "isolated environment" (virtualenv, isolated
  `GOPATH`, etc.) is dependency-reproducibility isolation, not a security sandbox — hooks run at
  full user privilege with unrestricted filesystem/network access. Confirmed by direct fetch: the
  official docs contain **no discussion of security risk, sandboxing, or a trust model for
  third-party hook repos at all.**
- **The only mitigation is opt-in and reactive**: `pre-commit autoupdate --freeze` converts a
  tag-based `rev` into a full commit SHA — grim's OCI content-digest model is already strictly
  stronger than this, by design, from day one.
- **Incidents: NOT DOCUMENTED.** No pre-commit-specific GHSA/CVE, no reported hijack of a widely
  used hook repo, found in this research pass. State this as an absence of evidence, not evidence
  of safety: the framework's own docs show no security hardening has ever been forced by an
  incident — a *worse* posture than npm's (which has at least iterated because of repeated
  incidents), not a better one. **This is the "what not to imitate" case study for the ADR**: an
  unsandboxed, mutable-ref-pinned, no-registry-review model that simply has not yet had its
  event-stream moment.

### Nix

Sandbox by default via Linux namespaces (mount + network); ordinary derivations get **no network
access at all** during build; only explicitly-declared, hash-pinned fixed-output derivations may
reach the network. [nix.dev manual](https://nix.dev/manual/nix/2.23/command-ref/conf-file.html?highlight=sandbox), fetched 2026-08-14.
Important nuance: the sandbox is a **reproducibility/purity boundary**, not a security boundary
against a malicious builder — it constrains what a build can *reach*, not what it can do to its
own output within that reach; Nix has had its own sandbox-*escape* CVEs.
[NixOS Discourse, sandbox bypass advisory](https://discourse.nixos.org/t/security-fix-nix-fixed-output-derivation-sandbox-bypass/40972), fetched 2026-08-14.
No install/build-time supply-chain malware incident found for Nix specifically — **NOT
DOCUMENTED** whether this reflects a genuine security dividend or smaller adoption; don't imply
causation without more evidence.

### Cargo `build.rs` — the most explicit "by design, no sandbox" stance found

Silent, default-on, and **officially, explicitly unsandboxed**: *"Build scripts in Cargo can do
literally anything from network requests to executing arbitrary binaries. This isn't deemed a
security issue as it is 'by design'... this virtue relies on trust among developers within the
community. When trust is broken by some incidents, even just once, the community has no choice
but to intensively review build scripts in their dependencies."*
[Rust Project Goals, "Explore sandboxed build scripts" (2024H2)](https://rust-lang.github.io/goals/2024h2/sandboxed-build-script.html), fetched 2026-08-14.
Real incidents: crates.io's own official postmortem of a Aug 2023 typosquatting cluster (9 crates,
malicious `build.rs` exfiltrating OS/IP/geolocation to Telegram) —
[Rust Blog official postmortem](https://blog.rust-lang.org/inside-rust/2023/09/01/crates-io-malware-postmortem/), fetched 2026-08-14, and
[RUSTSEC-2022-0042](https://rustsec.org/advisories/RUSTSEC-2022-0042.html).
**Ecosystem change: none.** The Rust Project Goals sandboxing exploration is complete but the team
is "unlikely to continue this work" — Cargo/crates.io have explicitly **declined** to ship default
sandboxing for `build.rs` despite two documented exploitations of exactly this pattern.
[Rust Blog, Nov 2024 update](https://blog.rust-lang.org/2024/12/16/project-goals-nov-update/), fetched 2026-08-14.
**Lesson for the ADR**: this is the clearest evidence in the whole survey of how hard it is to
retrofit execution restrictions onto an ecosystem that started execution-permissive — a strong
argument for grim not starting there.

---

## 2. Consent and trust models that actually worked

### Digest/hash-pinned approval with re-prompt on change — grim's planned model

**The vendor that actually implements grim's exact design is Gemini CLI, not Claude Code.**
Gemini CLI fingerprints each hook by `name` + `command`; if either changes (e.g. via `git pull`),
the hook is treated as new and untrusted and the user is warned again before it executes.
[Gemini CLI Hooks](https://geminicli.com/docs/hooks/), [Hooks Best Practices](https://geminicli.com/docs/hooks/best-practices/), fetched 2026-08-14 — the
latter explicitly names the exact attack this defends against ("If the `command` string of a
project hook is changed... its identity changes... Gemini CLI will treat it as a new, untrusted
hook"). Even this mechanism had a real gap needing a fix in headless/CI contexts:
[GHSA-wpqr-6v78-jr5g](https://github.com/google-github-actions/run-gemini-cli/security/advisories/GHSA-wpqr-6v78-jr5g), fetched 2026-08-14.

Claude Code's actual mechanism is coarser: folder-level trust (a one-time dialog gating whether
project hooks/`.mcp.json`/`permissions.allow` load at all), plus a *separate* SHA-256 pin used for
**plugin-archive integrity** (download matches an expected digest) — not a per-hook-command
re-approval-on-change. [Claude Code docs: security](https://code.claude.com/docs/en/security), [hooks](https://code.claude.com/docs/en/hooks), fetched 2026-08-14.
**Citing Claude Code as the precedent for grim's plan would be citing the wrong vendor** — Gemini
CLI is the validating precedent. OpenAI Codex CLI has directory-based project trust and an OS
sandbox/approval-policy layer but **no equivalent per-hook content fingerprinting was found** —
**NOT DOCUMENTED**. [learn.chatgpt.com/docs/agent-approvals-security](https://learn.chatgpt.com/docs/agent-approvals-security), fetched 2026-08-14.

**Bottom line**: grim's design has one real precedent (Gemini CLI), is plausible and
structurally sound, but **no vendor anywhere publishes incident-reduction metrics for this
specific control** — describe it in the ADR as "best-available practice, validated by at least
one comparable vendor," not as proven.

### Allow-lists — empirically insufficient when keyed on a mutable identity

- **npm `trustedDependencies`** (npm 10.3+) — a committed, versioned allowlist.
  [npm/cli#9172](https://github.com/npm/cli/issues/9172), fetched 2026-08-14.
- **GitHub Actions org allow-lists** — hardened 2025-08-15 to add SHA-pinning *enforcement* and a
  `!`-prefixed blocklist that overrides everything else.
  [GitHub Changelog](https://github.blog/changelog/2025-08-15-github-actions-policy-now-supports-blocking-and-sha-pinning-actions/), fetched 2026-08-14.
  **This hardening exists because tag-keyed allow-listing was directly bypassed in a real,
  CVE-tracked incident**: `tj-actions/changed-files` (CVE-2025-30066, disclosed March 2025,
  23,000+ repos affected) — attackers retroactively repointed version tags to a malicious commit,
  so any allow-list keyed on `v45` still resolved to attacker content.
  [CISA Advisory](https://www.cisa.gov/news-events/alerts/2025/03/18/supply-chain-compromise-third-party-tj-actionschanged-files-cve-2025-30066-and-reviewdogaction), [Wiz](https://www.wiz.io/blog/github-action-tj-actions-changed-files-supply-chain-attack-cve-2025-30066), fetched 2026-08-14.
  A near-identical second incident: 75 of 76 `trivy-action` version tags force-pushed
  (March 2026, actor "TeamPCP"). [Wiz](https://www.wiz.io/blog/github-actions-security-guide), fetched 2026-08-14.
- **Bun's name-based trust check** — **CVE-2026-24910**: Bun's `trustedDependencies` matches by
  package **name only**, not source; a `file:`/`link:`/`git:`/`github:` dependency sharing a
  trusted package's name is silently treated as trusted.
  [SentinelOne](https://www.sentinelone.com/vulnerability-database/cve-2026-24910/), fetched 2026-08-14. Structurally the same lesson as the
  tag-mutation bugs: **a name/tag is not an identity you can safely pin trust to.**
- **VS Code enterprise `extensions.allowed`** has a documented, acknowledged bypass: sideloading a
  `.vsix` file entirely evades the marketplace allow-list.
  [microsoft/vscode#258775](https://github.com/microsoft/vscode/issues/258775), fetched 2026-08-14.

**Verdict**: allow-listing-by-name/tag is empirically insufficient — bypassed at least three
times across three unrelated ecosystems via exactly the same mechanism (mutate what the allow-list
keys on, not what it means). **Grim's OCI content-digest design is structurally immune to this
specific bypass class**, which is the strongest affirmative argument in this whole research set
for grim's chosen approach, provided the digest is bound to the artifact that executes (see Q5).

### Signing (Sigstore/cosign, npm provenance) — attests pipeline identity, not code honesty

npm's own docs concede this outright: *"When a package in the npm registry has established
provenance, it does not guarantee the package has no malicious code."*
[docs.npmjs.com](https://docs.npmjs.com/generating-provenance-statements/), fetched 2026-08-14.
The sharpest documented case of a valid signature not stopping an attack: the **TanStack npm worm
(disclosed 2026-05-12)** — 84 malicious versions across 42 `@tanstack/*` packages
(`@tanstack/react-router` alone ~12.7M weekly downloads) were published *from within TanStack's
own legitimate, compromised GitHub Actions pipeline* using valid OIDC tokens, so they carried
**valid npm provenance attestations** — "the first documented case of malicious npm packages
shipping with valid provenance attestation."
[TanStack official postmortem](https://tanstack.com/blog/npm-supply-chain-compromise-postmortem), [Orca Security](https://orca.security/resources/blog/tanstack-npm-supply-chain-worm/) `[unofficial]`, fetched 2026-08-14.
**Lesson**: signing/provenance and content-digest pinning are both *integrity* controls (detect
tampering after the trust decision) — neither is a *safety* control (vet whether the content was
ever safe to trust). Compromise the pipeline or the maintainer, and every downstream integrity
check faithfully certifies the compromised result. Do not let the ADR imply digest-pinning alone
solves the "is this hook safe" question — it only solves "is this the hook I already approved."

### TOFU (trust-on-first-use)

Canonical critique (SSH host keys): once users are trained to expect a "trust changed" prompt on
routine, benign changes, they habituate to clicking through it, and the prompt loses its signal
value by the time a real attack arrives.
[agwa.name, "Why TOFU Doesn't Work"](https://www.agwa.name/blog/post/why_tofu_doesnt_work) `[unofficial]`, fetched 2026-08-14. Foundational academic
treatment: Wendlandt et al., USENIX Security 2008, "Perspectives."
[usenix.org](https://www.usenix.org/legacy/events/usenix08/tech/full_papers/wendlandt/wendlandt_html/index.html), fetched 2026-08-14.
**Applied to grim**: digest-repin-on-change already defeats the classic "attacker present at
first install" TOFU failure by design — the residual risk is the *human-habituation* failure
mode, a UX/process risk (re-prompt fatigue on routine version bumps), not a cryptographic one.
No quantified evidence of TOFU's real-world effectiveness was found anywhere — **NOT DOCUMENTED**.

### Sandboxing

Only Nix does real OS-level sandboxing (Linux namespaces) by default, and only on Linux (off by
default on macOS). Deno and Bun's "sandboxing" is lighter-weight, portable permission-gating of
specific dangerous operations, not full process isolation — Deno never runs npm lifecycle scripts
by default and names two concrete 2025 incidents this closes off (`@ctrl/tinycolor`, September
2025, compromised via a hijacked postinstall script, 40+ packages/2M+ weekly downloads).
[Deno official blog](https://deno.com/blog/deno-protects-npm-exploits), fetched 2026-08-14 — vendor blog making a self-serving
comparison, treat as directionally real but not independently audited.
This is the closer model to what grim can realistically ship: permission-gating specific
operations (network, filesystem, tool-call rewriting) rather than full namespace sandboxing.

### `--ignore-scripts` defaults — the strongest "reduced real incidents" evidence in this research

| Ecosystem | Flip | Date | Named trigger |
|---|---|---|---|
| pnpm | scripts off by default, `pnpm approve-builds` to allow | 2025-01-10 (v10.0.0) | rspack cryptomining postinstall, per maintainer statement |
| Bun | allow-list default from the start | ongoing | axios postinstall incident cited in vendor's own incident-response messaging |
| Deno | never runs npm lifecycle scripts | design invariant | general hardening, not one incident |
| npm | planned flip, v12 | ~July 2026 (unconfirmed primary date) | accumulated ecosystem pressure |

[Socket.dev](https://socket.dev/blog/pnpm-10-0-0-blocks-lifecycle-scripts-by-default) `[unofficial]`, [pnpm.io/cli/approve-builds](https://pnpm.io/cli/approve-builds), fetched 2026-08-14.
This is the strongest causal chain in the whole research set (named incident → named maintainer
statement → shipped default flip → dated release), even without a formal transparency report —
directly supporting grim's default-deny, opt-in-flag plan.

---

## 3. The mutator tier specifically

### Prior art for rewriting middleware, and what made it acceptable where accepted

1. **`direnv`** — content-*and-path*-bound explicit approval (`direnv allow`), fail-closed for
   anything unapproved, disposable sub-shell so only the resulting environment diff is exported.
   An early version fingerprinted trust by content only, allowing a copied, already-approved
   `.envrc` to execute in a new, malicious directory — fixed by binding trust to path + content
   together. [direnv/direnv#83](https://github.com/direnv/direnv/issues/83), fetched 2026-08-14. **Direct lesson for grim**: a digest alone is
   not enough if the *context* the hook runs in (which client, which project, which event) can
   change independently of the digest — bind approval to the full tuple, not the content alone.
2. **`sudo` — CVE-2023-22809.** `sudoedit`'s front-end parsed a user-controlled editor invocation
   string differently than the eventual editor process would, letting an attacker smuggle a `--`
   to expand an authorized single-file edit into an arbitrary-file edit as root.
   [sudo.ws official advisory](https://www.sudo.ws/security/advisories/sudoedit_any/), fetched 2026-08-14. **General lesson, directly
   applicable to the mutator tier**: any layer that inspects/rewrites a command line and hands
   the result to a separate parser than the one that finally executes it is vulnerable to this
   exact bug class. A grim mutator must not re-serialize a shell command as a string for the shell
   to re-parse independently; it should operate on, and emit, the same structured representation
   the executor will consume.
3. **Envoy Lua/WASM filters rewriting HTTP requests** — Envoy's own threat model explicitly scopes
   trust: it hardens against untrusted *traffic*, but assumes filter *code* is trusted; **it never
   claims to defend against untrusted filter authorship**. [envoyproxy.io threat model](https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/security/threat_model), fetched
   2026-08-14. A real advisory (**GHSA-xrwg-mqj6-6m22**, Jan 2026, CVSS 8.8) came from RBAC-gated
   Lua being under-sandboxed inside the *controller pod's* validation step, not just the
   data-plane proxy, letting a malicious script read control-plane secrets. Fix: default-strict
   Lua validation, a security-hardening module blocking dangerous constructs by default, and an
   explicit `disableLua` kill switch with a searchably-named `InsecureSyntax` opt-out — never a
   silent relaxation. **Lesson**: gate *who may author* a mutator, separately and more strictly
   than gating what the mutator's output may do.
4. **Git server-side hooks** — GitHub/GitLab both restrict authorship of a mutating/gating
   server-side hook to instance administrators, not repo owners, and both hook models are
   fundamentally accept-or-reject, never silent rewrite-and-continue; a mutation that must happen
   is done client-side (`git filter-repo`) under the pusher's own visibility.
   [GitHub docs](https://docs.github.com/en/enterprise-server@3.16/admin/enforcing-policies/enforcing-policy-with-pre-receive-hooks/about-pre-receive-hooks), [GitLab docs](https://docs.gitlab.com/administration/server_hooks/), fetched 2026-08-14.
   **Lesson for grim**: prefer "hook can block, and a *separate*, visible mechanism can propose an
   edit for the model/user to see and accept" over "hook silently rewrites and the command just
   runs differently" wherever the choice exists.
5. **GitHub Actions `GITHUB_ENV`/`GITHUB_PATH`** — a real, named, patched vulnerability
   (CVE-2020-15228 / [GHSA-mfwh-5m23-j46w](https://github.com/advisories/GHSA-mfwh-5m23-j46w)) where the *predecessor* mechanism
   (`::set-env::` printed to stdout) let **any logged untrusted string** silently mutate a later,
   more-privileged step's environment. Fix: require an explicit **file write**, not a magic
   stdout token, closing the "ambient injection via mere log output" vector, plus documented
   guidance to route untrusted input through an intermediate variable and pin third-party actions
   to a commit SHA. [GitHub official changelog](https://github.blog/changelog/2020-10-01-github-actions-deprecating-set-env-and-add-path-commands/), [Security hardening guide](https://docs.github.com/en/actions/security-guides/security-hardening-for-github-actions), fetched 2026-08-14.

**Distilled: controls that make rewriting acceptable, across all five precedents**
- Explicit, scoped opt-in per trust boundary — never ambient interception.
- One parser, one truth — the mutator and the executor must share exactly one command-line
  interpretation, never two independently maintained ones.
- Fail closed, bounded blast radius (timeouts, default-strict validation, an explicit and
  searchable kill switch for any relaxation).
- Reject-or-pass beats silent mutate-in-place wherever a design choice exists.
- No shared execution context between untrusted input and the rewrite's effect (don't let a step
  processing untrusted content also hold the privilege to silently reconfigure a later step).
- The mutation is visible and versioned in its own right — never inferred only from its effects.

### Prompt injection reaching a hook/rewriting layer — the highest-value finding

**This is documented, and the primary evidence is Cursor's own vendor security advisories**,
which explicitly name "absence of hook/approval integrity checking" as root cause:

| Advisory | Date | Root cause |
|---|---|---|
| [GHSA-4cxx-hrm3-49rm](https://github.com/cursor/cursor/security/advisories/GHSA-4cxx-hrm3-49rm) (CVE-2025-54135) | 2025-08-02 | approval covered *editing* an existing dotfile but not *creating* one; indirect prompt injection had the agent write `.cursor/mcp.json` from scratch, registering a malicious MCP server |
| [GHSA-vqv7-vq92-x87f](https://github.com/cursor/cursor/security/advisories/GHSA-vqv7-vq92-x87f) (CVE-2025-54130) | 2025-08-02 | same create-vs-edit gap, targeting `.vscode/settings.json` (e.g. changing the default shell) |
| [GHSA-hf2x-r83r-qw5q](https://github.com/cursor/cursor/security/advisories/GHSA-hf2x-r83r-qw5q) (CVE-2026-31854) | 2026-03-09 | indirect prompt injection **combined with a bypass of the command allow-list mechanism itself** — commands executed automatically even under "Use AllowList" mode |
| [GHSA-rmj9-23rg-gr67](https://github.com/cursor/cursor/security/advisories/GHSA-rmj9-23rg-gr67) (CVE-2024-48919) | 2024-10-22 | poisoned webpage caused the model to emit a command + newline that auto-executed, bypassing the intended confirm-before-run step |
| [Lakera write-up](https://www.lakera.ai/blog/cursor-vulnerability-cve-2025-59944) (CVE-2025-59944) `[independent researcher]` | — | case-sensitive filename check vs. case-insensitive filesystem let `.cUrSoR/mcp.json` evade the exact approval gate the first two advisories introduced |

All four Cursor-authored advisories are explicit that the approval/integrity gate in the
automation-config layer itself was the thing that failed — not just "the agent ran a bad command."

**Claude Code**: Check Point's disclosure (**CVE-2025-59536 / CVE-2026-21852**, patched in Claude
Code v1.0.111) found hooks/MCP-init commands defined in a repo's `.claude/settings.json` could
execute *before* the trust dialog resolved, including one path that intercepted plaintext API
keys via a malicious `ANTHROPIC_BASE_URL`. [Check Point Research](https://research.checkpoint.com/2026/rce-and-api-token-exfiltration-through-claude-code-project-files-cve-2025-59536/), fetched 2026-08-14. **Caveat**: this
is a supply-chain/trust-dialog-timing bug (a static malicious file shipped in a cloned repo), not
prompt injection reaching a hook mid-session — a real hook-layer flaw, but a different mechanism
than "content the agent reads triggers a rewrite."

**The closer match for Claude Code specifically**: PromptArmor's "Hijacking Claude Code via
Injected Marketplace Plugins" (2025-10-16) — a malicious plugin marketplace (auto-indexed by
third-party registries within an hour, impersonating an Anthropic account) shipped a
`UserPromptSubmit` **hook** that overwrote Claude's own `settings.local.json` permissions, then a
separate prompt-injection payload drove Claude to run a now-pre-approved `curl` exfiltration.
[promptarmor.substack.com](https://promptarmor.substack.com/p/hijacking-claude-code-via-injected) `[independent researcher]`, fetched 2026-08-14. Hybrid: entry
is a user *installing* a malicious plugin (social engineering/supply chain), then injected content
exploits the now-weakened gate the hook itself created. **Directly on point for grim's mutator
tier**: a hook was able to mutate the very config that governs what future hooks/commands require
approval, with no integrity check on what a hook is allowed to touch.

**Adjacent research that does NOT match (checked and ruled out, so the ADR doesn't overstate)**:
Simon Willison's "lethal trifecta" is a conceptual framework (untrusted content + private data +
external communication), not a documented hook-layer incident.
[simonwillison.net](https://simonwillison.net/2025/Jun/16/the-lethal-trifecta/) `[independent researcher]`, fetched 2026-08-14. Invariant
Labs' "MCP tool poisoning" targets static tool-*description* text read once at registration, not
a runtime rewrite layer. [invariantlabs.ai](https://invariantlabs.ai/blog/mcp-security-notification-tool-poisoning-attacks), fetched 2026-08-14 — its root-cause language
("treating schemas as configuration rather than security-critical infrastructure... no integrity
verification, access controls, code review, or governance over schema changes") is reusable
verbatim as the general pattern grim's hook-integrity design must avoid, applied to hooks instead
of schemas. Embrace The Red's Claude Code DNS-exfiltration finding (CVE-2025-55284) bypasses an
overly permissive bash allowlist directly, with no hook/config involvement at all.
[embracethered.com](https://embracethered.com/blog/posts/2025/claude-code-exfiltration-via-dns-requests/) `[independent researcher]`, fetched 2026-08-14.

**Conclusion for Q3**: yes, prompt injection reaching a hook-like automation-config layer is
documented, repeatedly, in Cursor's own advisories, and the vendor's own stated root cause in
every case is exactly the shape of gap the ADR must close: an approval check keyed on the wrong
condition (edit-not-create, case-sensitivity, allow-list classification) rather than on the actual
identity and content of what is about to run.

---

## 4. Audit as a control

**Does mandatory audit logging demonstrably help?** No vendor or research source in this pass
provides before/after evidence that logging *by itself* reduced incidents — every mature system
surveyed (Kubernetes audit, `auditd`, sudo I/O logging, AWS CloudTrail) treats audit logging as
**forensics and detection, not prevention.** Its documented value is real but different: it makes
an incident *investigable* and a policy violation *detectable after the fact*, not blocked.

**What a useful record contains**, synthesized across systems:
- **Kubernetes** — `level` (`None`/`Metadata`/`Request`/`RequestResponse`, gating how much body
  content is captured), `stage`, timestamp, `user`, `sourceIPs`, `objectRef`, `responseStatus`,
  request/response bodies (level-gated), `annotations`.
  [kubernetes.io official docs](https://kubernetes.io/docs/tasks/debug/debug-cluster/audit/), fetched 2026-08-14. **The level split is the load-bearing
  idea for grim**: don't ship one fixed verbosity — default to metadata-plus-decision, and gate
  full mutated-payload capture behind an explicit, stricter mode.
- **AWS CloudTrail** — actor identity, `eventTime`, `eventName` (the decision), `requestParameters`
  (before-state), `responseElements` (after-state), a unique `eventID`, error/outcome status — and
  **CloudTrail caps and truncates oversized fields** (100 KB / 28 KB / 256 KB–1 MB ceilings) rather
  than logging unboundedly. [AWS official docs](https://docs.aws.amazon.com/awscloudtrail/latest/userguide/cloudtrail-event-reference-record-contents.html), fetched 2026-08-14.
- **sudo I/O logging + `sudoreplay`** — full before/after terminal I/O capture with replay,
  organized by session/user/command/timestamp. [sudo.ws docs](https://www.sudo.ws/docs/man/1.9.5/sudoers.man/), fetched 2026-08-14. The ceiling of
  what full-fidelity audit looks like — useful as a description of what a grim "verbose mutation
  audit mode" could offer on request, not as the unconditional default.

**Minimum record for grim's mutator audit**: actor/trigger identity (which hook, by digest),
timestamp, the decision (allow/deny/mutate), a correlation ID, before-state and after-state of the
mutated field (size-capped, CloudTrail-style), and outcome/error status — defaulting to a
redacted/structural view, not the raw payload.

**Audit logs as their own attack surface — real, documented risk, directly relevant given grim's
payload contains a tool call's raw (possibly attacker-influenced) input**:
- **Log injection — CWE-117** (aka log forging): unneutralized external input written to a log can
  forge entries or, in a terminal viewer, inject ANSI escape sequences to spoof what a reviewer
  sees. [cwe.mitre.org](https://cwe.mitre.org/data/definitions/117.html), fetched 2026-08-14. Real CVEs in the same class:
  CVE-2025-58160 (`tracing-subscriber`, Rust — directly relevant to grim's own stack),
  [gitlab.com advisory](https://advisories.gitlab.com/pkg/cargo/tracing-subscriber/CVE-2025-58160/), fetched 2026-08-14.
  **Because the audited payload is a tool call's raw input — which may be attacker-shaped —
  it must be output-neutralized before it reaches the log, or a hostile hook's own input becomes
  a log-forgery/terminal-hijack vector against whoever reviews the trail.**
- **Unbounded log growth (CWE-400 / CWE-779, Logging of Excessive Data)** — a real operational
  incident of exactly this shape: unrotated audit tables grew to ~65 GB on one Metabase instance,
  breaking backups and upgrades. [metabase/metabase#76625](https://github.com/metabase/metabase/issues/76625) `[unofficial]`, fetched 2026-08-14. Direct
  precedent for shipping rotation/size caps from day one, per Kubernetes' `--audit-log-max*` and
  CloudTrail's field truncation.
- **Secrets captured in logs**: GitHub's own docs concede masking is not exhaustive — "manual
  masking is required for anything not already a registered secret," and warn that even
  security-audit events can capture command-line-visible secrets. GitLab's own docs state
  outright: "Masking a CI/CD variable is not a guaranteed way to prevent malicious users from
  accessing variable values." [GitHub](https://docs.github.com/en/actions/security-guides/using-secrets-in-github-actions), [GitLab](https://docs.gitlab.com/ci/variables/), fetched 2026-08-14. Real
  precedent for a masking failure at scale: Twitter's 2018 self-disclosed incident of plaintext
  passwords landing in internal logs before hashing completed, undetected for months.
  [BleepingComputer](https://www.bleepingcomputer.com/news/security/twitter-admits-recording-plaintext-passwords-in-internal-logs-just-like-github/) `[unofficial]`, fetched 2026-08-14.

**Where should a tamper-evident log live?** Sigstore's Rekor transparency log is the clearest
model even though grim isn't doing signing: append-only, independently checkable via Merkle-tree
inclusion/consistency proofs, so "did someone edit yesterday's record" becomes detectable rather
than unanswerable. [docs.sigstore.dev](https://docs.sigstore.dev/logging/overview/), fetched 2026-08-14. Grim doesn't need a full
transparency-log server to take the transferable lesson: **the audit log should not be writable by
the same process/privilege level that runs the hook subprocess**, each record should be
append-only and ideally hash-chained to its predecessor, and (mirroring Linux `auditctl -e 2`'s
immutable-config lock) the logging configuration itself should not be disableable by anything less
privileged than the user who enabled hooks in the first place.
[auditctl man page](https://man7.org/linux/man-pages/man8/auditctl.8.html), fetched 2026-08-14.

---

## 5. Digest-pinned approval failure modes — grim-specific

Verified against grim's own source at `03e59b0` on 2026-08-14
(`src/install/install_state.rs`, `src/install/path_anchor.rs`, `docs/src/configuration.md`).

| Failure mode | Applies to grim? | What must be true for it to be safe |
|---|---|---|
| **Bundle/transitive member swap after approval** | **Real risk if not designed against.** A bundle-level approval (or a lock resolution keyed on the bundle's own digest) does not automatically constrain which *individual hook* ships under that bundle on the next resolve unless each member hook is independently digest-pinned and independently re-approved. | Approval must be recorded per-hook, at the hook's own content digest — never at the bundle's digest alone. `BundleMember.kind` already threads `ArtifactKind` through per the hooks ADR's own data-model section; the approval record must key off the same per-member identity, not the bundle wrapper. |
| **Mutable tag substituting for a digest** | Not a risk for OCI-resolved artifacts — grim already resolves and locks by content digest (`GrimoireLock`), which is the strictly-stronger-than-`pre-commit` design noted in §1. Risk resurfaces only if a future feature lets a hook `command:` reference something resolved by tag/version at *runtime* rather than by the locked digest. | Approval must be checked against the digest actually locked and about to execute, never re-resolved from a mutable reference at run time. |
| **Approval store the hook itself can edit** | **Real risk, must be closed explicitly.** If hook approvals are recorded in the same `state.json`/`global.json` a hook process (running at full user privilege) can write to, a malicious or compromised hook can grant its own future re-approval, or approve a sibling hook, with no separate privilege boundary. | The approval record must live somewhere the hook subprocess's own write access doesn't reach, or at minimum be integrity-checked (its own digest/hash) by grim before being trusted on the next run — the same "don't let the mutated thing also attest to its own trustworthiness" principle as Envoy's control-plane-vs-data-plane separation (§3). |
| **TOCTOU between approval and execution** | **Real risk.** Grim's existing model checks content hashes at *install* time (`ClientOutput.content_hash`, `footprint_hash`) to detect drift for materialized outputs — but the specific question for hooks is whether the digest is re-verified **immediately before `exec`**, not just at install/lock time. A window between "approved at install" and "executed this session" is exactly where a swapped file on disk would go undetected. | The digest check for a hook must happen at the moment of invocation, not only at install time — re-hash the file about to run, compare to the approved digest, refuse to run on mismatch, exactly mirroring the re-prompt-on-change behavior Gemini CLI documents (§2). |
| **World-writable / shared-home approval store** | **Plausible on shared machines.** `$GRIM_HOME/state/global.json` is global-scope, one file per `GRIM_HOME`. On a genuinely shared `$GRIM_HOME` (shared NFS home directory, a cluster login node, a container image that bakes in a pre-populated `global.json`), a different user or a previous session's compromise could plant an approval that a later, different user unknowingly inherits. | The approval record's validity should be scoped to the approving identity/machine where that's meaningful, and grim's install docs/threat model should name shared-`$GRIM_HOME` setups explicitly as a case requiring the operator to treat `global.json` as sensitive, matching how any credential store would be treated. |
| **Project-scope `state.json` committed to a shared repo, delivering a pre-baked approval to every clone** | **Currently mitigated, but the mitigation has known gaps.** Grim writes a self-managed `.grimoire/.gitignore` (`*`) the first time `.grimoire/` is created, keeping `state.json` out of version control by default — confirmed at `docs/src/configuration.md` and via the code comment at `src/install/path_anchor.rs:284` acknowledging *"the `state.json` a `git clone` can deliver (grim's own `.gitignore` exists only after grim has run)"*. The gap is explicit in grim's own source: the `.gitignore` is written **after** first run, so (a) a `state.json` created or committed *before* grim's first run in that workspace, (b) a user force-adding it (`git add -f`), or (c) a CI/vendoring system that copies a working tree without honoring `.gitignore` semantics, can all still deliver a committed `state.json` — and with it, any hook approval recorded inside — to every future clone. | If hook approvals are added to `state.json`'s schema, the ADR must treat "this file can, in edge cases, arrive via `git clone`" as a first-class threat, not an incidental one — e.g. by never treating a *project-scope* approval alone as sufficient to auto-run a hook on a machine/user that hasn't separately consented, or by binding project-scope approval to something that can't travel with the repo (a value derived at approval time from the approving user/machine, checked again before honoring the record). |
| **Multi-machine case (approve on machine A, hook runs on machine B)** | Same shape as the shared-home and git-clone cases above — grim's project/global scope split doesn't by itself prevent an approval recorded on one machine from being read as valid on another once the record file is copied or synced (dotfile syncing tools, home-directory backup/restore, shared network home). | Treat any approval record as data that can leave its original machine; the record's authority should not outlive verification that the thing it approves — the exact digest — is what's about to run *here, now* (restates the TOCTOU point above, but for the cross-machine case specifically). |

**Overall**: grim's content-digest foundation is genuinely stronger than every mutable-identity
scheme surveyed in §1–§2 (tags, names, branches). The failure modes that remain are not about the
digest itself but about **where the approval record lives relative to who can write it, and when
it's checked relative to when the hook actually runs** — exactly the two axes (`state.json`
writability, install-time-only verification) that grim's own code comments already flag as known
tensions in the current design, and that this research confirms are the load-bearing questions for
the ADR's threat model, not an edge case to defer.

---

## 6. Secrets exposure on the wire

**Comparative guidance, stdin vs env vs argv vs temp file:**

- **argv is worst.** Visible to any local user via `ps`/`/proc/<pid>/cmdline` unless the
  non-default `hidepid` mount option is set
  ([proc(5) man page](https://man7.org/linux/man-pages/man5/proc.5.html), fetched 2026-08-14); GitHub's own docs name `ps` as a leak channel
  ([GitHub Actions secrets docs](https://docs.github.com/en/actions/security-guides/using-secrets-in-github-actions), fetched 2026-08-14); and durably persisted to shell history by
  default unless a user manually opts out (`HISTCONTROL=ignorespace`).
- **Env vars are second-worst.** OWASP: *"environment variables are generally accessible to all
  processes and may be included in logs or system dumps... not recommended unless other methods
  are not possible."* [OWASP Secrets Management Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Secrets_Management_Cheat_Sheet.html), fetched 2026-08-14. Docker's own docs
  converge on the same point: *"can also be printed in logs when debugging errors without your
  knowledge."* [Docker docs](https://docs.docker.com/compose/how-tos/use-secrets/), fetched 2026-08-14. Readable via `/proc/<pid>/environ` by
  same-uid or root processes, subject to `ptrace_scope`/Yama LSM restrictions.
  [proc_pid_environ(5) man page](https://man7.org/linux/man-pages/man5/proc_pid_environ.5.html), fetched 2026-08-14. Captured wholesale in core dumps, and — critically
  for grim's design — **inherited by every grandchild process the hook subprocess spawns**, unless
  explicitly scrubbed, a much wider blast radius than a value read once from stdin.
- **A temp file is workable but adds its own surface**: permission-race (TOCTOU) between creation
  and read, guaranteed cleanup even on hook crash, and world-readable `/tmp` defaults on some
  distros without a careful `umask`. No source in this research recommends a temp file as *safer*
  than stdin for this purpose.
- **Stdin is the best fit of the four**: never enumerated by `ps`, never written to
  `/proc/<pid>/cmdline` or `/proc/<pid>/environ`, not automatically inherited by the subprocess's
  own children, leaves no shell-history trace. Its one unsolved weakness: whatever the subprocess
  reads from stdin into memory is just as present in a **core dump** as an env var would be
  ([core(5) man page](https://man7.org/linux/man-pages/man5/core.5.html), fetched 2026-08-14) — stdin does not solve the crash-dump vector,
  only the always-on-visible-metadata vectors (`ps`, `/proc/environ`, shell history, CI log echo
  of `env`/`printenv`).

**Recommendation directly supporting grim's already-planned design**: pass the JSON envelope via
stdin (not argv, not a bare env var) — matches the plan as described — and keep the accompanying
"few flat env vars" strictly non-secret-bearing (correlation ID, hook name, working directory,
tool name), precisely because env vars are grandchild-inherited and `/proc`/core-dump-readable in
a way stdin content is not. Separately worth considering: constraining or disabling core dumps for
the hook subprocess if crash-dump exposure of stdin-delivered secrets is judged a residual risk
worth closing, since neither transport solves that vector on its own.

---

## 7. Concrete threat-model checklist for the ADR

Ranked by realistic likelihood given the prior-art pattern above (not a generic OWASP pass —
each row names the specific grim mechanism that closes it). Severity labels follow
`.claude/rules/quality-security.md`'s Critical/High/Medium/Low scale.

| # | Attack path | Realistic likelihood | Control that closes it | Severity if unclosed |
|---|---|---|---|---|
| 1 | A hook is silently updated to a new digest between approval and the next run, and grim keeps executing it without re-prompting (the single most-repeated failure pattern in §1/§2: mutable tag/name substitution, generalized to "any un-reverified identity") | **High** — this exact bug shape recurred in at least 4 unrelated ecosystems (tj-actions, trivy-action, Bun, Cursor create-vs-edit) | Digest computed and compared at the moment of invocation, every invocation, not cached from install time; mismatch is a hard fail, never a warn-and-continue | Critical |
| 2 | A hook (or a plugin bundling a hook) rewrites the approval/permission store itself, granting its own future runs blanket trust — the PromptArmor Claude Code marketplace pattern (§3) | **High** for the mutator tier specifically, since mutators run with the ability to affect what happens next | Approval store is not writable by the hook subprocess's own privilege level, or is independently integrity-checked before being trusted on the next run | Critical |
| 3 | A mutator rewrites a shell command as a string, and the executor re-parses that string differently than the mutator assumed — the sudo CVE-2023-22809 pattern (§3) | **Medium-High** — depends entirely on grim's mutator API shape; likely if the API is "receive a string, return a string" | Mutator operates on/emits the same structured command representation the executor consumes; never a second, independently-parsed string | High |
| 4 | Prompt injection reads a tool result or fetched file, drives the agent to author or modify a hook definition (or a config the hook governs) that the digest-approval flow hasn't seen yet — the Cursor create-vs-edit and allow-list-bypass pattern (§3) | **Medium-High** — Cursor alone has 4+ CVEs of exactly this shape in the last 18 months | Any agent-authored write to a hook-governing file (not just edits — creation too, and case/path-normalization variants) requires the same approval gate as an install-time hook; no "create" vs "edit" asymmetry | Critical |
| 5 | Hooks default to on, or the experimental flag is easy to leave enabled without the operator understanding the blast radius, mirroring every ecosystem that shipped execute-by-default (§1) | **Medium** — a design/rollout risk, not a code bug, but the single most consistently-punished choice across 8 ecosystems | Hooks off by default behind the experimental flag; enabling requires an explicit, scoped decision (not a global "trust everything" toggle); CI escape hatch is itself audited, not silent | High |
| 6 | The mutation audit log itself is poisoned via CRLF/ANSI injection in the (attacker-shaped) tool-call payload it records, corrupting or spoofing what a reviewer sees (§4, CWE-117) | **Medium** — requires an attacker already able to influence tool-call content, which is the same precondition as row 4 | Output-neutralize any payload content before writing to the audit log; never render raw untrusted bytes into a terminal/log viewer | Medium |
| 7 | The audit log or approval store grows unbounded and either fails open (defeating the control) or becomes an ops liability that gets disabled under pressure (§4, CWE-400/779) | **Medium** | Rotation and size caps from day one (Kubernetes/CloudTrail pattern), fail-closed (never fail-open) on write failure | Medium |
| 8 | A secret embedded in a tool call's input leaks via the hook's own environment/argv/logging rather than via the hook's intended function (§6) | **Medium** | Stdin-only transport for the payload; non-secret-bearing flat env vars only; redact-by-default audit logging (ties to row 6) | High |
| 9 | Project-scope `state.json` (or a future approval record living there) is committed to a shared repo via a pre-existing file, a forced add, or a `.gitignore`-unaware copy, delivering a pre-baked approval to every clone (§5, grim-specific) | **Low-Medium** — grim already gitignores `.grimoire/` on first run, but the gap is in grim's own code comments | Never treat a project-scope approval alone as sufficient without re-verifying the exact digest at run time on the actual executing machine; document the git-clone delivery risk explicitly rather than relying on the `.gitignore` default | Medium |
| 10 | A hook approved as part of a bundle continues to be trusted after the bundle's *composition* changes (a different member hook shipping under an unchanged bundle-level identity) (§5) | **Low-Medium** — no incident found specific to grim's bundle model, but the general "wrapper digest doesn't constrain member identity" pattern is well established (§2, allow-list failures) | Approval recorded per-hook at the hook's own content digest, never at the bundle wrapper's digest alone | High |
| 11 | A malicious or compromised third-party OCI registry serves different content for the same tag across requests/regions, and a hook resolved by tag rather than locked digest picks up the swap | **Low** — grim already resolves to `GrimoireLock` digests; this only applies if the hook API allows unlocked/floating references at runtime | Never allow a hook `command:`/reference to be resolved outside the locked digest path at execution time | Critical if introduced |
| 12 | An observer-tier hook (output ignored, "safe" by design) is used as a reconnaissance/exfiltration channel since it still sees the full tool-call payload even though it can't act on it | **Low-Medium** — not found as a documented incident anywhere in this research, but is the direct extrapolation of "hooks run at full user privilege" (Gemini CLI's own words, §2) applied to the supposedly-safest tier | Observer tier still requires the same approval/trust gate as gatekeeper/mutator — "output ignored" is not "input restricted"; do not let the tier name imply a lower approval bar than the actual privilege it holds | Medium |

---

## Sources

| URL | What it establishes | Fetched |
|---|---|---|
| https://www.nodejs-security.com/blog/npm-ignore-scripts-best-practices-as-security-mitigation-for-malicious-packages | npm install-script consent model, ~2% prevalence `[unofficial]` | 2026-08-14 |
| https://blog.npmjs.org/post/180565383195/details-about-the-event-stream-incident | event-stream incident, official npm postmortem | 2026-08-14 |
| https://eslint.org/blog/2018/07/postmortem-for-malicious-package-publishes/ | eslint-scope compromised-account incident, official postmortem | 2026-08-14 |
| https://github.com/advisories/GHSA-hxxf-q3w9-4xgw | eslint-scope GHSA | 2026-08-14 |
| https://github.com/advisories/GHSA-pjwm-rvh2-c87w | ua-parser-js CVE-2021-4229 | 2026-08-14 |
| https://www.rapid7.com/blog/post/2021/11/05/new-npm-library-hijacks-coa-and-rc/ | coa/rc incident detail `[unofficial]` | 2026-08-14 |
| https://github.com/advisories/GHSA-97m3-w2cp-4xx6 | node-ipc/peacenotwar CVE-2022-23812 — legitimate-maintainer-sabotage class | 2026-08-14 |
| https://www.kb.cert.org/vuls/id/534320 | Shai-Hulud npm worm, CERT/CC | 2026-08-14 |
| https://www.cybersecuritydive.com/news/cisa-dependency-checks--shai-hulud-compromise/761018/ | Shai-Hulud second wave detail | 2026-08-14 |
| https://github.blog/security/supply-chain-security/top-100-npm-package-maintainers-require-2fa-additional-security/ | npm 2FA rollout timeline | 2026-08-14 |
| https://github.blog/security/supply-chain-security/introducing-npm-package-provenance/ | npm provenance GA, Sigstore-backed | 2026-08-14 |
| https://socket.dev/blog/pnpm-10-0-0-blocks-lifecycle-scripts-by-default | pnpm 10 default flip, named trigger incident `[unofficial]` | 2026-08-14 |
| https://thehackernews.com/2026/06/github-to-disable-npm-install-scripts.html | npm v12 planned default flip (trade press, date provisional) | 2026-08-14 |
| https://peps.python.org/pep-0517/ | PEP 517 build-backend interface | 2026-08-14 |
| https://www.veracode.com/blog/python-package-installation-attacks/ | PEP 517/518 does not close arbitrary-execution hole `[unofficial]` | 2026-08-14 |
| https://checkmarx.com/blog/pypi-is-under-attack-project-creation-and-user-registration-suspended/ | PyPI March 2024 typosquatting campaign, registry-wide suspension `[unofficial]` | 2026-08-14 |
| https://guides.rubygems.org/gems-with-extensions/ | RubyGems `extconf.rb` execution model | 2026-08-14 |
| https://thehackernews.com/2020/04/rubygem-typosquatting-malware.html | RubyGems typosquatting incident `[unofficial]` | 2026-08-14 |
| https://blog.rubygems.org/2022/08/15/requiring-mfa-on-popular-gems | RubyGems MFA rollout, official | 2026-08-14 |
| https://docs.brew.sh/Homebrew-Security-and-Supply-Chain | Homebrew trust model, bottles vs. taps | 2026-08-14 |
| https://brew.sh/2021/04/21/security-incident-disclosure/ | Homebrew Cask auto-merge incident, official disclosure | 2026-08-14 |
| https://github.com/advisories/GHSA-69fq-xp46-6x23 | Trivy/Homebrew custom-tap compromise, CVE-2026-33634 | 2026-08-14 |
| https://brew.sh/2026/06/11/homebrew-6.0.0/ | Homebrew 6.0.0 Tap Trust, official release notes | 2026-08-14 |
| https://docs.brew.sh/Tap-Trust | Tap Trust mechanism detail | 2026-08-14 |
| https://code.visualstudio.com/docs/configure/extensions/extension-runtime-security | VS Code extension execution model, official | 2026-08-14 |
| https://www.bleepingcomputer.com/news/security/vscode-extensions-with-9-million-installs-pulled-over-security-risks/ | Material Theme extension pull, Feb 2025 | 2026-08-14 |
| https://www.csoonline.com/article/3956464/warning-to-developers-stay-away-from-these-10-vscode-extensions.html | April 2025 malicious-extension wave | 2026-08-14 |
| https://github.com/microsoft/vscode/issues/258775 | VS Code allow-list `.vsix` sideload bypass, official issue tracker | 2026-08-14 |
| https://pre-commit.com/ | pre-commit `rev` pinning model, no security/sandboxing discussion (confirmed by fetch) | 2026-08-14 |
| https://nix.dev/manual/nix/2.23/command-ref/conf-file.html?highlight=sandbox | Nix sandbox default behavior, official | 2026-08-14 |
| https://discourse.nixos.org/t/security-fix-nix-fixed-output-derivation-sandbox-bypass/40972 | Nix sandbox-escape advisory | 2026-08-14 |
| https://doc.rust-lang.org/cargo/reference/build-scripts.html | Cargo build-script reference, official (no security warning present) | 2026-08-14 |
| https://rust-lang.github.io/goals/2024h2/sandboxed-build-script.html | Official "by design" no-sandbox stance for build.rs | 2026-08-14 |
| https://blog.rust-lang.org/inside-rust/2023/09/01/crates-io-malware-postmortem/ | crates.io malicious-crate postmortem, official | 2026-08-14 |
| https://rustsec.org/advisories/RUSTSEC-2022-0042.html | rustdecimal RUSTSEC advisory | 2026-08-14 |
| https://blog.rust-lang.org/2024/12/16/project-goals-nov-update/ | Rust Project Goals sandboxing exploration discontinued, official | 2026-08-14 |
| https://geminicli.com/docs/hooks/ | Gemini CLI hook fingerprinting, official | 2026-08-14 |
| https://geminicli.com/docs/hooks/best-practices/ | Gemini CLI re-prompt-on-change mechanism detail, official | 2026-08-14 |
| https://github.com/google-github-actions/run-gemini-cli/security/advisories/GHSA-wpqr-6v78-jr5g | Gemini CLI trust-model gap in headless/CI contexts, official | 2026-08-14 |
| https://code.claude.com/docs/en/security | Claude Code folder-level TOFU trust model, official | 2026-08-14 |
| https://code.claude.com/docs/en/hooks | Claude Code hooks reference, official | 2026-08-14 |
| https://learn.chatgpt.com/docs/agent-approvals-security | Codex CLI sandbox/approval-policy model, official | 2026-08-14 |
| https://github.com/npm/cli/issues/9172 | npm `trustedDependencies` allow-list model | 2026-08-14 |
| https://github.blog/changelog/2025-08-15-github-actions-policy-now-supports-blocking-and-sha-pinning-actions/ | GitHub Actions SHA-pinning/blocklist hardening, official | 2026-08-14 |
| https://www.cisa.gov/news-events/alerts/2025/03/18/supply-chain-compromise-third-party-tj-actionschanged-files-cve-2025-30066-and-reviewdogaction | tj-actions tag-mutation incident, CISA official advisory | 2026-08-14 |
| https://www.wiz.io/blog/github-action-tj-actions-changed-files-supply-chain-attack-cve-2025-30066 | tj-actions incident detail `[unofficial]` | 2026-08-14 |
| https://www.wiz.io/blog/github-actions-security-guide | trivy-action tag force-push incident `[unofficial]` | 2026-08-14 |
| https://www.sentinelone.com/vulnerability-database/cve-2026-24910 | Bun name-based trust bypass, CVE-2026-24910 | 2026-08-14 |
| https://docs.npmjs.com/generating-provenance-statements/ | npm provenance does not guarantee code safety, official | 2026-08-14 |
| https://tanstack.com/blog/npm-supply-chain-compromise-postmortem | TanStack worm, valid-provenance-on-malicious-content, official postmortem | 2026-08-14 |
| https://orca.security/resources/blog/tanstack-npm-supply-chain-worm/ | TanStack incident detail `[unofficial]` | 2026-08-14 |
| https://www.agwa.name/blog/post/why_tofu_doesnt_work | TOFU habituation critique `[unofficial]` | 2026-08-14 |
| https://www.usenix.org/legacy/events/usenix08/tech/full_papers/wendlandt/wendlandt_html/index.html | Perspectives (TOFU academic critique), USENIX Security 2008 | 2026-08-14 |
| https://deno.com/blog/deno-protects-npm-exploits | Deno permission-gating model vs. npm incidents `[unofficial, vendor blog]` | 2026-08-14 |
| https://github.com/direnv/direnv/issues/83 | direnv path+content trust-binding fix | 2026-08-14 |
| https://www.sudo.ws/security/advisories/sudoedit_any/ | sudoedit CVE-2023-22809, official advisory | 2026-08-14 |
| https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/security/threat_model | Envoy's explicit trusted-filter-code threat model, official | 2026-08-14 |
| https://github.com/envoyproxy/gateway/security/advisories/GHSA-xrwg-mqj6-6m22 | Envoy Gateway Lua sandboxing gap, official | 2026-08-14 |
| https://docs.github.com/en/enterprise-server@3.16/admin/enforcing-policies/enforcing-policy-with-pre-receive-hooks/about-pre-receive-hooks | GitHub server-hook privilege/timeout model, official | 2026-08-14 |
| https://docs.gitlab.com/administration/server_hooks/ | GitLab server-hook admin-only authorship, official | 2026-08-14 |
| https://github.com/advisories/GHSA-mfwh-5m23-j46w | GITHUB_ENV/set-env CVE-2020-15228 | 2026-08-14 |
| https://docs.github.com/en/actions/security-guides/security-hardening-for-github-actions | GitHub Actions script-injection hardening guidance, official | 2026-08-14 |
| https://github.com/cursor/cursor/security/advisories/GHSA-4cxx-hrm3-49rm | Cursor CVE-2025-54135, create-vs-edit approval gap, official | 2026-08-14 |
| https://github.com/cursor/cursor/security/advisories/GHSA-vqv7-vq92-x87f | Cursor CVE-2025-54130, official | 2026-08-14 |
| https://github.com/cursor/cursor/security/advisories/GHSA-hf2x-r83r-qw5q | Cursor CVE-2026-31854, allow-list bypass, official | 2026-08-14 |
| https://github.com/cursor/cursor/security/advisories/GHSA-rmj9-23rg-gr67 | Cursor CVE-2024-48919, official | 2026-08-14 |
| https://www.lakera.ai/blog/cursor-vulnerability-cve-2025-59944 | Cursor case-sensitivity bypass `[independent researcher]` | 2026-08-14 |
| https://research.checkpoint.com/2026/rce-and-api-token-exfiltration-through-claude-code-project-files-cve-2025-59536/ | Claude Code hook trust-dialog-timing CVEs `[independent researcher]` | 2026-08-14 |
| https://promptarmor.substack.com/p/hijacking-claude-code-via-injected | Claude Code marketplace-plugin hook hijack `[independent researcher]` | 2026-08-14 |
| https://simonwillison.net/2025/Jun/16/the-lethal-trifecta/ | Lethal trifecta framework (ruled out as non-match) `[independent researcher]` | 2026-08-14 |
| https://invariantlabs.ai/blog/mcp-security-notification-tool-poisoning-attacks | MCP tool poisoning (ruled out as non-match; root-cause language reused) `[independent researcher]` | 2026-08-14 |
| https://embracethered.com/blog/posts/2025/claude-code-exfiltration-via-dns-requests/ | Claude Code DNS exfiltration (ruled out as non-match) `[independent researcher]` | 2026-08-14 |
| https://kubernetes.io/docs/tasks/debug/debug-cluster/audit/ | Kubernetes audit levels/fields, official | 2026-08-14 |
| https://docs.aws.amazon.com/awscloudtrail/latest/userguide/cloudtrail-event-reference-record-contents.html | CloudTrail record fields + size caps, official | 2026-08-14 |
| https://www.sudo.ws/docs/man/1.9.5/sudoers.man/ | sudo I/O logging / sudoreplay, official | 2026-08-14 |
| https://cwe.mitre.org/data/definitions/117.html | CWE-117 log injection | 2026-08-14 |
| https://advisories.gitlab.com/pkg/cargo/tracing-subscriber/CVE-2025-58160/ | ANSI escape injection in tracing-subscriber (Rust) | 2026-08-14 |
| https://github.com/metabase/metabase/issues/76625 | Unbounded audit-log growth operational incident `[unofficial]` | 2026-08-14 |
| https://docs.github.com/en/actions/security-guides/using-secrets-in-github-actions | GitHub secret-masking limits, `ps`/audit-event leak channel named, official | 2026-08-14 |
| https://docs.gitlab.com/ci/variables/ | GitLab masking not a guaranteed control, official | 2026-08-14 |
| https://www.bleepingcomputer.com/news/security/twitter-admits-recording-plaintext-passwords-in-internal-logs-just-like-github/ | Twitter plaintext-password-in-logs incident `[unofficial]` | 2026-08-14 |
| https://docs.sigstore.dev/logging/overview/ | Rekor transparency-log tamper-evidence rationale, official | 2026-08-14 |
| https://man7.org/linux/man-pages/man8/auditctl.8.html | `auditctl -e 2` immutable-config lock, official man page | 2026-08-14 |
| https://cheatsheetseries.owasp.org/cheatsheets/Secrets_Management_Cheat_Sheet.html | OWASP env-var vs. file secret-injection guidance, official | 2026-08-14 |
| https://docs.docker.com/compose/how-tos/use-secrets/ | Docker env-var vs. mounted-secret guidance, official | 2026-08-14 |
| https://man7.org/linux/man-pages/man5/proc_pid_environ.5.html | `/proc/<pid>/environ` access control, official man page | 2026-08-14 |
| https://man7.org/linux/man-pages/man5/proc.5.html | `hidepid` mount option, cmdline/environ visibility, official man page | 2026-08-14 |
| https://man7.org/linux/man-pages/man5/core.5.html | Core dump captures process memory (env + stdin buffers), official man page | 2026-08-14 |
| `/mnt/wsl/share/dev/grimoire/grimoire/src/install/install_state.rs` (repo, `03e59b0`) | grim state.json/global.json schema, `ClientOutput.content_hash` drift detection | 2026-08-14 |
| `/mnt/wsl/share/dev/grimoire/grimoire/src/install/path_anchor.rs` (repo, `03e59b0`) | grim's own comment acknowledging `state.json` can arrive via `git clone` before `.gitignore` is written | 2026-08-14 |
| `/mnt/wsl/share/dev/grimoire/grimoire/docs/src/configuration.md` (repo, `03e59b0`) | project-scope `state.json` location and `.gitignore` self-management, official project docs | 2026-08-14 |
| `/mnt/wsl/share/dev/grimoire/grimoire/.agents/adr/adr_hooks_support.md` (repo, `03e59b0`) | grim's planned hook architecture, security mitigations already drafted (signed manifests, hash-change re-approval, observer/gatekeeper tiers) | 2026-08-14 |
| `/mnt/wsl/share/dev/grimoire/grimoire/.claude/rules/quality-security.md` (repo, `03e59b0`) | severity vocabulary (Critical/High/Medium/Low) used in §7 checklist | 2026-08-14 |
