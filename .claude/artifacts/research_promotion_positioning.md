# Research: 1.0 Promotion, Positioning, and Public-Surface Readiness

<!--
Filename: artifacts/research_promotion_positioning.md
Owner: Researcher (multi-agent workflow, 17 agents)
Handoff to: meta-plan_promotion_1_0.md
Purpose: Durable evidence base for the pre-1.0 promotion push. Every claim
here was verified against a primary source; refuted claims are recorded as
refuted so they are not re-derived.
-->

## Metadata

**Date:** 2026-07-26
**Domain:** positioning / packaging / ci-cd / security
**Triggered by:** Pre-1.0 decision — "is the project underselling itself, and
where should it be posted?"
**Expires:** 2027-01-26 (competitive landscape moves fast; re-verify star
counts, funding, and channel rules before citing them)
**Method:** 6 parallel research lanes → 10 adversarial refutation agents
(8 of 10 claims refuted or corrected) → synthesis. Load-bearing claims then
re-verified by hand against the local checkout and live APIs.

---

## Direct Answer

The implementation is ready; the public surface is not. But the gap is
**factual, not rhetorical** — the README describes a v0.1.0 product and its
first pasted command fails on the shipped binary. Fix the inventory and the
first-run path before touching framing. Do **not** rename the project, do
**not** adopt an enterprise reframe, do **not** migrate the docs site.

---

## Verified defects (re-checked by hand, 2026-07-26)

| # | Defect | Evidence | Severity |
|---|---|---|---|
| D1 | **README quick start exits 64.** `README.md:55` runs `grim add skill code-review ghcr.io/acme/code-review:1`; `grim add --help` reports `Usage: grim add [OPTIONS] <REFERENCE>` — one positional. `--kind`/`-k` and `--name`/`-n` are flags (`src/command/add.rs:55-82`). | `target/release/grim add --help` | **blocker** |
| D2 | **Short refs 404 for every first-party package.** `Identifier::parse_with_default_registry` → `prepend_domain(name, default_registry)` (`src/oci/identifier.rs:352`) — a plain prepend, no kind segment. The catalog publishes kind-segmented to `ghcr.io/grimoire-rs/skills/<name>` (`catalog/README.md:68`). `docs/src/quickstart.md:5-7` advertises the unsegmented expansion. | source + catalog README | **blocker** |
| D3 | `LICENSE:189` = `Copyright 2026 The OCX Authors`. 191 of 192 holder sites already say `The Grimoire Authors` (189 `.rs` headers, `.licenserc.toml:3`, `docs/book.toml:3`). Came in with the scaffold commit copied from `ocx/LICENSE`; `.licenserc.toml` was corrected, `LICENSE` was not. Ships inside every one of the 20+ release archives (cargo-dist auto-includes LICENSE) and propagated to `grimoire-vscode/LICENSE:189` and `grimoire-components/LICENSE:189`. | `LICENSE:189`, grep | high |
| D4 | `SECURITY.md` does not exist; `README.md:72` links it → 404. GitHub private vulnerability reporting **disabled**; `isSecurityPolicyEnabled=false`. | `gh repo view` | high |
| D5 | Repo topics = `[]`. No social preview. Discussions disabled while `.github/ISSUE_TEMPLATE/config.yml` links "Ask questions" → `/discussions` (dead link). | `gh api repos/grimoire-rs/grimoire --jq '.topics'` | medium |
| D6 | `grimoire-rs/index` has **no LICENSE at all** — nobody may legally fork or self-host it, contradicting the self-hosted-index pitch. `setup-grimoire/LICENSE` is a 15-line stub, not the Apache-2.0 text (§4(a) requires the full copy for a redistributed Action). | repo contents API | high |
| D7 | `CONTRIBUTING.md` states **no inbound-license term**. Only Apache-2.0 §5's inbound=outbound presumption operates. Issue #57 proves external participants already exist. | `CONTRIBUTING.md` | high (one-way door) |
| D8 | No CVE/advisory scanning anywhere: no Trivy (the project's own declared golden path in `product-tech-strategy.md`), no `cargo-audit`, no `[advisories]` in `deny.toml`, no CodeQL. Dependabot **security** updates disabled; secret scanning disabled. `taskfiles/rust.taskfile.yml:82` defines `license:check` but no gate runs it. | workflows + `deny.toml` | medium |
| D9 | `grimoire-vscode` repo `homepageUrl` = `https://gimoire.rs` (typo, missing `r`). Marketplace category = `Other`. Extension is live at v0.2.4 with **76 installs**, 0 ratings. Not confirmed on Open VSX → invisible to Cursor / VSCodium / Windsurf users. | `gh repo view`, Marketplace listing | medium |
| D10 | README status block (`README.md:21-25`) still says "provisional, pre-1.0" and lists 12 subcommands; `src/main.rs` defines 22. `CLAUDE.md` and `docs/src/stability.md` declare a stabilization freeze. `product-context.md:86` says "18 subcommands". | README vs source | medium |

### Refuted — do not re-raise

| Claim | Verdict |
|---|---|
| "No public feedback path / issues restricted" (ChatGPT blocker #3) | **False.** Issues enabled, 20+ issues, external reporter present (#57). `.github/ISSUE_TEMPLATE/{bug_report,feature_request,config}.yml` exist and work. GitHub's community-profile API reports `issue_template: null` because that legacy field only tracks a single old `ISSUE_TEMPLATE.md`, never the modern YAML forms directory. |
| "`grim status --check` gives you a CI gate" | **False.** `docs/src/commands.md:458`: "`grim status` is a report, not a gate" — always exits 0. Do not claim a CI story without `--exit-code`. |
| "The index shows ecosystem traction" | **False.** index.grimoire.rs = 12 packages, 100% first-party (5 `grimoire-rs`, 7 `michael-herwig`/`arcana`). Zero third-party publishers. |
| "awesome-claude-code is ~28.5k stars" | **Corrected:** 50,947 stars / 4,441 forks (2026-07-26). Submission is via the **web UI issue form**, not a PR — their CONTRIBUTING says so explicitly. |
| "Tessl is funded by Snyk's founder" | **Corrected:** Guy Podjarny *founded* Tessl (he is not merely a backer); the $125M came from boldstart, GV, and Index Ventures. |
| "jeffreytse/grimoire is a near-identical concept collision" | **Overstated.** It is git-backed with symlink installs, curated content, and a compliance linter — zero OCI involvement. Real overlap is the name, the pitch phrase, and the **identical `grimoire.toml` / `grimoire.lock` filenames**. Vendor overlap is 6, not 8. |

---

## Naming and distribution collisions

`grim` was created **2026-05-15**; `jeffreytse/grimoire` was created
**2026-06-03** — grim is the earlier project.

| Holder | What it is | What it blocks |
|---|---|---|
| [jeffreytse/grimoire](https://github.com/jeffreytse/grimoire) | Go; "package manager for best practices — 1000+ skills across 26 domains"; targets Claude Code, Copilot, Gemini CLI, Cursor, OpenCode. 21★ | GitHub search + the pitch phrase + **colliding config filenames** |
| emersion `grim` | Wayland screenshot utility | Debian/Ubuntu `apt`, Arch official repos |
| [Vaishnav-Sabari-Girish/grimoire](https://github.com/Vaishnav-Sabari-Girish/grimoire) | Rust task runner, "define sigils, cast workflows"; v0.4.0 published 2026-07-02, **active**; ships binaries named `grim` *and* `grimoire` | crates.io **`grim`** |
| jshrake/grimoire (archived 2020-02-15) | GLSL live-coding tool, v0.2.1, 5.2k downloads | crates.io **`grimoire`** |

**Decision: keep the name.** A rename breaks `ghcr.io/grimoire-rs/*` catalog
refs, `index.grimoire.rs`, `setup-grimoire@v1`, the `grimoire.rs` domain, the
VS Code extension publisher id, and the `grimoire.toml` / `grimoire.lock`
filenames — every one a frozen 1.0 contract under Principle 9. The collisions
bite only on channels not in use: the install path is the checksum-verifying
installer plus `ocx`, and `README.md:48` already correctly uses
`cargo install --git`.

**Consequences of that decision:** Homebrew, homebrew-core, AUR, nixpkgs,
winget, and crates.io are **permanently off the channel list**. Add
`publish = false` to `Cargo.toml` so an accidental `cargo publish` cannot
half-land. Optional mitigation: have the install script warn when `grim` is
already on `PATH`.

---

## Competitive landscape

The "npm for AI skills" niche is **crowded**, not blue ocean.

| Project | Signal | What it does *not* do |
|---|---|---|
| Vercel skills.sh | 27.2k★ | Documents having **no** dependency resolution, cross-vendor rendering, hosted registry, or lockfile |
| Tessl | Founded by Guy Podjarny (Snyk founder); $125M raised; Snyk security score on every public skill | Not OCI; hosted registry |
| ClawHub | 3,000+ skills, semantic search, serves the OpenClaw ecosystem | Hosted; no lockfile/digest model |
| skillctl | Sigstore/cosign signing | Narrower scope |
| jeffreytse/grimoire | 21★ | git-backed, no OCI, no lockfile |
| openskills / Paks / Askill / FastSkill / skill-get / Skilz | Show HN launches | assorted |

**Anthropic's Agent Skills format became a cross-vendor open standard in
Dec 2025** (Microsoft, OpenAI, Atlassian, Figma, Cursor, GitHub). Consequence:
"we translate *skills* per vendor" invites factual rebuttal. Lead the
cross-vendor claim on **rules and agents** — `CLAUDE.md` / `AGENTS.md` /
`.cursorrules` / `copilot-instructions.md` genuinely still diverge.

### Differentiator ranking (10-second stranger appeal)

1. **One artifact → 10 clients, with build-time-enforced honest declines.**
   Unique across everything surveyed. Backed by `docs/src/clients.md:29-40`
   and the parity test in `src/install/client_target.rs` that fails the build
   if the table drifts. Sell on rules + agents.
2. **Lockfile + digest pinning + `grim update`.**
3. **No service to run** (OCI reuse) — sentence two, never the headline. As a
   lead it is a barrier; as a follow-up it is reassurance.
4. MCP-server-as-artifact · 5. publish + self-hostable index + GitHub/GitLab
   parity · 6. bundles · 7. frozen stability contract + JSON interface.
   *(4–7 are the credibility payload for the announcement post, not the front
   page.)*
8. **Drift/CI — cut from positioning entirely** until `--exit-code` exists.

### Unclaimed narratives (both high value)

- **Thomas Vitale's OCI Agent Skills Artifacts spec** — draft v0.1.0,
  published 2026-04-02, GitHub Discussion **#292** in `agentskills/agentskills`
  (the org hosting the core Agent Skills Specification originally released by
  Anthropic). Proposes standardizing packaging, distribution, signing, and
  tracking of Agent Skills as OCI artifacts. Two implementations exist:
  Arconia CLI (Java, ORAS Java SDK) and salaboy's `skills-oci` (Go, ORAS Go
  client) — **both newer and thinner than grim**. "A shipping 22-command
  implementation of where the spec is heading" is a far stronger 1.0 story
  than "another skills package manager", and engaging hedges the risk that the
  spec commoditizes the differentiator. Cost: one comment.
- **Skill-registry supply-chain risk.** Andrew Nesbitt published a
  skills-registry threat-model taxonomy (2026-06-03) naming ClawHub, Tessl,
  and skills.sh. A Snyk audit found **13.4% of sampled skills across ClawHub
  and skills.sh (534 of 3,984) had at least one critical issue** — malware,
  prompt injection, or exposed secrets. grim is absent from this conversation.
  "Your own registry, your own auth boundary, digest-pinned, no third-party
  host" is an evidence-backed angle nobody is using — **claimable only once
  `SECURITY.md` exists and the no-signing limitation is stated honestly.**

---

## Positioning copy (proposed, verified against grim 0.11.1)

**Headline (recommended):**
> A package manager for AI-agent config — skills, rules, agents, and MCP
> servers, installed into every coding agent you use.

**Supporting sentence two, verbatim everywhere the headline appears:**
> Storage is any OCI registry — GHCR, Docker Hub, or your own. There is no
> Grimoire service to sign up for.

**Docs-site hero / social-card line:** *One skill, ten coding agents, one
lockfile.*

**Rejected:** ChatGPT's "Governed, reproducible distribution of AI-agent
configuration" — removes the only legible noun ("package manager"), adds a
word no individual developer searches for, targets an audience an unmonetized
6★ project cannot serve, and violates the project's own
`.claude/rules/docs-style.md:28` ("No sales pitch or marketing opener").

**Keep the package-manager noun.** `docs/src/introduction.md:14` already uses
it well ("There is no `npm install` for an agent skill"). The fix is to append
the differentiator, not delete the frame.

**GitHub About (299 chars, limit 350):**
> Package manager for AI-agent config. grim installs, updates, and publishes
> skills, rules, agents, MCP servers, and bundles into Claude Code, Copilot,
> Cursor, Codex, Gemini, Zed, Amp, Kiro, Junie, and opencode — pinned by
> digest in a lockfile. Storage is any OCI registry; there is no service to
> run.

**Topics (20 = GitHub's maximum):**
```
ai, ai-agents, agent-skills, agent-config, claude-code, copilot, cursor,
codex, mcp, model-context-protocol, oci, oci-registry, ghcr,
package-manager, developer-tools, devtools, cli, rust, skills,
prompt-engineering
```

**Verified quick start** (full refs deliberate — see D2):
```sh
grim init                                        # write grimoire.toml
grim search authoring                            # browse the public index
grim add ghcr.io/grimoire-rs/skills/grim-usage   # declare, pin, install
grim status                                      # what is installed
```

---

## Website

`grimoire.rs` is mdBook output with no landing content — the root URL renders
`introduction.md` verbatim inside book chrome (sidebar, theme switcher,
prev/next nav). `docs/book.toml` has no `[output.html.additional-*]` and no
home-page config.

**Recommendation: keep mdBook. Add one file.** mdBook injects `is_index: true`
into the Handlebars context when it re-renders the first chapter as the site
root (`hbs_renderer.rs:127-129`), exactly as it already does with `is_print`.
A single overridden `docs/theme/index.hbs` with
`{{#if is_index}}…{{else}}…{{/if}}` produces a real hero page with no sidebar
chrome — **no new toolchain, no build-step hack, no URL changes.** The site
deploys `mdbook build docs` → `docs/book` straight to the domain root, so all
~18 flat URLs stay put (Principle 9 — published doc URLs are a contract).

**Rejected:** VitePress migration to match ocx.sh (days of work + a permanent
Node toolchain for a solo maintainer, 18 pages of frontmatter rewrites, anchor
and `/schemas/` path re-verification). **Rejected:** post-build `cp` over
`index.html` (duplicates nav/theme markup, maintained twice, no gain over the
`is_index` branch).

**Also missing, all one-time and static:** OG / Twitter / canonical meta tags
via `theme/head.hbs` (a supported site-wide override), plus hand-written
`sitemap.xml` and `robots.txt` dropped into `docs/theme/` (mdBook copies
theme-dir files through). Today a grimoire.rs link posted to HN renders with
**no preview card at all**.

**Demo asset — highest leverage per hour, pipeline already owned.** OCX has a
working asciinema setup: pytest scripts driving the real binary → `.cast`,
embedded via `asciinema-player`, converted to GIF via `agg`
(`ocx/website/recordings.taskfile.yml`). Copy it. Record
`search → add → status → tui`. `.cast` files are kilobytes of text. Player on
the landing page, GIF in the README. Note OCX itself never closed the
GIF-in-README loop — grimoire would be doing this better than the reference
project. `assets/` currently holds only a logo; the TUI is invisible.

---

## Publish-from-CI

`docs/src/ci.md` (245 lines) is **good**: GitHub Actions + GitLab CI, fork
policy, `DOCKER_CONFIG`, `gh auth setup-git`, exit 69 semantics,
fine-grained-PAT caveats. Two real gaps:

1. It is page 11 of 18 in `SUMMARY.md` and linked from nothing. No path from
   README → "publish from CI".
2. No copy-paste starting point.

**`~/dev/arcana/.github/workflows/publish.yml` is that starting point** — a
real, working, tag-driven pipeline whose comments encode hard-won gotchas:

- `grimoire-rs/setup-grimoire@v1.2.0` pinned with the comment *"floating @v1
  tag is stale/broken"* — **the unretagged `v1` from the announce work is
  still open and must be fixed before any CI example ships**, or every
  copy-paster hits it.
- `DOCKER_CONFIG` isolated to `$RUNNER_TEMP/docker` (runner temp is only
  available inside steps, not job-level env).
- `workflow_dispatch` → `--dry-run` with a non-semver `canary` version so it
  never cascades or skip-exists. **`--dry-run` appears zero times in
  `ci.md`.**
- Announce needs a **classic** PAT — a fine-grained token scoped to the fork
  403s on `grimoire-rs/index` and cannot fork.
- Exit **69** = bytes published but announce failed → soft `::warning::`, not
  a failed build. Bytes-published is the hard gate.
- `grim logout` in an `if: always()` step.

---

## Licensing

**Recommendation: `LICENSE:189` → `   Copyright 2026 The Grimoire Authors`
(preserve the 3-space appendix indent). Not a personal name.**

Rationale against a personal name: 191/192 sites already say "The Grimoire
Authors"; maintainer identity is already public (`.github/CODEOWNERS:2`,
`CODE_OF_CONDUCT.md:46`, 524 authored commits); a personal line is a 192-file
diff instead of 1 and goes stale on the second contributor; it contradicts the
maintainer's own OCX convention (ocx.sh footer: "Copyright © 2026 The OCX
Authors"), where "The <Project> Authors" reads as a deliberate house
convention across both projects.

The Apache-2.0 appendix line sits below `END OF TERMS AND CONDITIONS`
(`LICENSE:175`) — it is instructional boilerplate and transfers nothing. What
a wrong name *does* do: create a false attribution in every distributed
artifact, weaken the notice's evidentiary function, and mislead every license
scanner (licensee, askalono, ClearlyDefined, FOSSA). The VS Code Marketplace
renders the license on the listing page.

**Do NOT create a NOTICE file.** Neither vendored fork
(`external/rust-oci-client`, `external/docker_credential`) ships one, so
Apache-2.0 §4(d) never triggers. Creating one imposes a permanent propagation
duty on every downstream redistributor. For dependency notices the right
artifact is `THIRD-PARTY.md` via `cargo about`.

**Two items that matter more than the string:**

1. **Inbound-license term.** Add a `## License` section to `CONTRIBUTING.md`:
   contributions are Apache-2.0, require `Signed-off-by` (DCO, link
   developercertificate.org), define "The Grimoire Authors" as everyone in
   `git shortlog -sne`. **No CLA** — disproportionate for an unmonetized
   project and it deters drive-by contributors. This is the only genuinely
   irreversible item with no external dependency.
2. **§69b UrhG employer rights.** *Not legal advice.* If the code was created
   "in Wahrnehmung seiner Aufgaben oder nach den Anweisungen seines
   Arbeitgebers", the exclusive economic rights sit with the employer and the
   public Apache-2.0 grant is defective at the root — affecting every
   downstream user, un-fixable retroactively. `src/store/paths.rs:6`,
   `src/store/atomic_write.rs:6`, and `src/resolve.rs:6` record code adapted
   from OCX, so **one conversation covers both projects**. Check first whether
   an Open-Source-Nebentätigkeit process already exists. The one question a
   lawyer would need to answer: *was this software created in the course of
   employment duties or on employer instruction?*

---

## Launch channels (ranked, mechanics verified)

| Order | Channel | Timing | Mechanics / gate |
|---|---|---|---|
| 0 | GitHub topics + About | today | Currently `[]`. Free, zero-risk, no dependencies. jeffreytse has 20 topics — plausibly why it out-surfaces grim on GitHub's own browse pages. |
| 1 | **Console.dev** — hello@console.dev | **before tagging 1.0** | Only hard deadline: selection criteria admit **pre-1.0 tools only**. Ineligible forever after the tag. |
| 2 | **Show HN** | launch day | Highest leverage, effectively one-shot. HN policy **forbids LLM-generated or LLM-edited post and comment text** — write by hand, block several hours to answer. Timing effects ~3%; readiness dominates. |
| 3 | Changelog News + DEV.to mirror | launch day | Changelog's bar (not a how-to, not commercial) is cleared by an unmonetized OSS 1.0. DEV.to is zero-gate. |
| 4 | users.rust-lang.org "announcements" | day 0/+1 | Zero gatekeeping, category literally exists for this. Safer than any subreddit. |
| 5 | This Week in Rust — Call for Participation PR + Crate of the Week self-nomination | launch week | CFP prereqs already met (OSI license, public tracker); needs one difficulty-tagged issue. CoW is nominated *and voted on* in thread 2704 with editorial curation — self-nomination allowed, not automatic. |
| 6 | Claude Developer Discord + MCP Discord | launch week | Literal renderer-target audience. One post each, not a sustained presence. |
| 7 | registry.modelcontextprotocol.io (`server.json`) | +2 weeks | Official, low-risk, distinct surface, matches a shipping capability. |
| 8 | awesome-claude-code (50,947★) | +2–4 weeks | No star gate. **Web UI issue form, not a PR** — their CONTRIBUTING says so. Needs the reworked README hook. |
| 9 | `agentskills/agentskills` Discussion #292 | opportunistic | Possibly the best ROI item on the list. |

**Skipped:** Reddit (reddit.com was unfetchable during research — *all*
subreddit rule claims are secondhand; r/programming and r/devops reportedly
carry real removal risk for cold self-promo; treat as opportunistic thread
participation and re-check sidebars by hand). awesome-rust (hard
50★/2000-download gate with an "equivalent popularity metrics" escape hatch —
arguing from 6 stars is weak). Lobste.rs (invite-only; the `vibecoding` tag is
a direct topical fit if an invite appears).

---

## Honest ceiling

A front-page Show HN for a Rust dev tool in a crowded 2026 niche produces
roughly **200–800 stars**, a handful of external issues, and one or two people
who actually try publishing. Not front page: 50–150 stars and a slow trickle.
Either way, near-zero external PRs — Rust dev tools get stars, not
contributors — and no ecosystem forms around a 12-package index in launch
week.

**Realistic 6-month best case** is not "the package manager for AI-agent
config" (Vercel has 27.2k stars; Tessl has $125M). It is: *the reference OCI
implementation people cite when the Agent Skills OCI spec discussion comes
up, used by people who run 3+ coding agents and got tired of copy-pasting
rules.*

**Failure signal worth stopping promotion over:** three months post-launch,
with `SECURITY.md`, the demo, and the reworked README all shipped — **zero
third-party publishers on the index and no external issues beyond the one
that already exists.** Star count is not the metric; *someone else publishing
an artifact* is.

**What would justify escalating:** a second maintainer appearing, or the
Vitale spec gaining traction with grim cited as prior art.

---

## Sources

| Source | Type | Date checked | Covers |
|---|---|---|---|
| local checkout `~/dev/grimoire` @ 0.11.1 | Repo | 2026-07-26 | D1–D10, all file:line claims |
| `~/dev/arcana/.github/workflows/publish.yml` | Repo | 2026-07-26 | Working CI publish reference |
| `gh api repos/grimoire-rs/*` | API | 2026-07-26 | Topics, issues, security toggles, index contents |
| crates.io API `/crates/grim`, `/crates/grimoire` | API | 2026-07-26 | Name collisions |
| marketplace.visualstudio.com listing | Web | 2026-07-26 | Extension live, 76 installs, category `Other` |
| github.com/jeffreytse/grimoire | Repo | 2026-07-26 | Category collision scope |
| github.com/Vaishnav-Sabari-Girish/grimoire | Repo | 2026-07-26 | crates.io `grim` holder |
| `agentskills/agentskills` Discussion #292 | Discussion | 2026-07-26 | OCI Agent Skills spec draft |
| rust-unofficial/awesome-rust CONTRIBUTING.md | Repo | 2026-07-26 | 50★/2000-download gate |
| hesreallyhim/awesome-claude-code | Repo | 2026-07-26 | 50,947★, issue-form submission |
| ChatGPT 5.6 SOL handover, 2026-07-19 | Doc | 2026-07-19 | Prior analysis; several claims refuted above |
