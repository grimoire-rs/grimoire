# Research: landing routers, screencasts and getting-started patterns

**Date:** 2026-09-06
**Run:** hex-plan xhigh, docs redesign (`.agents/plans/plan_docs_site_redesign.md`)
**Phase:** Research, patterns axis
**Consumers:** `.agents/specs/design_docs_site_redesign.md`, the plan.

# Research: docs design patterns for the grimoire docs-plan design draft

Scope: five topics requested by the docs-plan orchestrator, checked against
`.agents/discovery/use-cases.yaml` (entry_paths, router, screencasts, groups)
and `research_docs_pain_points.md` section B/C (which this research could not
re-read — file missing at the given path in the `docs-plan` worktree; findings
below rely on the use-cases.yaml router/entry_paths/screencasts blocks only).

---

## 1. Task-based landing routing ("ways in" + router)

| Site | Entry strip | Router below | Card wording |
|---|---|---|---|
| Docker docs | 4 top links: Get started / Guides / Manuals / Reference | "What's new" list + 5-question FAQ, not a card grid | Questions name pains ("How do I get started with Docker?") |
| Fly.io docs | 2-step linear "Install flyctl → fly launch" | 8-card grid | **Feature-named**, not pain-named (Machines, Volumes, Networking…) |
| Tailscale kb | 4-link "Get Started" strip (concept / install / scenarios / FAQ) | 3 grouped sections, 13 cards total (Manage / Expand / Reference) | Mixed: admin-task grouping, feature-named cards inside |
| Stripe get-started | CLI-first strip + "Start developing" list | "Common use cases" section, persona-framed | **Best pain/persona match**: "As a startup, accept simple payments," "As a SaaS startup, sell subscriptions," "Accept in-person payments as a retail store" — role + goal, not feature |
| GOV.UK design system | N/A (system, not one site) | "Task list" pattern for end-to-end journeys, "Navigate a service" links pattern for non-linear multi-task services | Doctrine, not a landing example: use Task list only when there's a clear end-to-end order; use nav links when tasks don't have one — directly answers whether grimoire's router (8 independent guide cards, no fixed order) is the right pattern (it is — non-linear, so nav-links/cards, not a step list) |

**No A/B or user-research write-up was found comparing task-based landings to
feature lists quantitatively** — this is asserted in blog posts (idratherbewriting.com,
LeadDev) as best practice but not backed by a published experiment. Cite as
industry consensus, not measured evidence.

**Strongest pattern**: Stripe's persona-framed "common use cases" is the
closest published analogue to the draft's "eight cards each naming a pain."
GOV.UK's task-list-vs-nav-links rule is the clearest *justification* for
using cards (nav links) over a step list, since grimoire's eight guides have
no fixed order.

**What the draft gets wrong, if anything**: none of the reference sites use
exactly three entry paths above a router — Docker uses four top links,
Tailscale four, Stripe a CLI-first single path. Three is not a violated
convention, just untested; no site benchmarks strip length. Not a defect,
just note it's an judgment call, not a copied pattern.

**Recommendation**: keep the three entry_paths (T02/T05+T32/T30) as a strip,
keep the router card pain-first wording already in use-cases.yaml (it already
matches the Stripe pattern better than Fly.io's feature-named cards) — no
change needed here.

---

## 2. Screencast-per-guide practice

- **asciinema**: headless mode + exit-status propagation makes it CI-safe;
  the embedding script auto-sizes an iframe; `idle-time-limit` trims dead
  air (recommended default 2s in most Sphinx/Hugo integrations found);
  `poster=npt:0:03`-style frame prevents an autoplay-looking blank box;
  `preload` is implied once a poster is set. No native accessibility
  transcript — sites that care ship a `<noscript>` fallback link to the raw
  `.cast` file or a hand-written transcript paragraph beside the player.
  Typical embedded length in the wild: 20–90s per guide-level cast.
- **VHS (Charm)**: `.tape` files are plain scripts (window size, theme,
  typing speed, `Sleep`, `Screenshot`/`Output` directives) — fully
  deterministic, and `charmbracelet/vhs-action` reruns them in CI so the
  rendered GIF/MP4 is regenerated and diffed in the PR alongside the tape
  edit. No built-in test-assertion story: VHS renders faithfully but does
  not itself verify the commands' output was correct — that has to come
  from a separate golden-file/integration-test pass wrapping the same tape.
- **Grimoire's current harness vs. both**: the existing approach (a Python
  test drives the real `grim` binary against a real registry to produce
  `docs/src/demo.cast`) is strictly stronger than either alternative on one
  axis neither tool gives for free: **the recording only exists if the
  commands actually succeeded**, because it's asserted inside a test, not
  merely replayed. VHS's CI regeneration proves the tape is reproducible,
  not that the output is correct, unless paired with its own assertions.
  Switching to VHS would trade that behavioral guarantee for nicer
  typography/theming and MP4/GIF output — not a net win unless video output
  (not asciicast) is specifically wanted. **Recommendation: keep the
  asciinema-via-pytest harness**, add `idle-time-limit` + a poster frame to
  the player config, and add a one-line `<noscript>` transcript link per
  cast for accessibility — do not migrate to VHS.
- **Page-weight practice**: one player per page (confirmed across every
  example found), lazy-instantiate via the async embed script rather than
  an eagerly-loaded `<iframe>`, and never autoplay — every example that
  specified autoplay behavior left it off by default and required an
  explicit click.

**What the draft gets wrong, if anything**: the plan already says "re-recorded
when commands change, never hand-edited" — correct and matches the harness's
integration-test framing. It doesn't yet specify idle-trim/poster/lazy-load
for the player; add those three as implementation details when the guide
pages are built, not a design-level gap.

---

## 3. Getting-started ending on a visible result / parallel paths

- **uv first-steps**: shortest example found — one verification command
  (`uv`), truncated help output, then "Next steps." Ends on a result in
  under one screen.
- **Deno get-started**: "drop-in JS runtime for Node devs" framing, ends on
  a runnable script producing visible output before linking to "Learn"
  content — same one-path-then-branch shape as uv.
- **Astro getting-started**: single recommended path (install → tutorial →
  explore → integrate); does **not** offer three parallel install paths on
  one page — it links out to the CLI wizard how-to and lets the tutorial
  carry the "first visible result" job. Avoids duplication by treating the
  landing page as a router, not a triplicated quickstart.
- **Stripe**: leads with a CLI-first single path (`stripe docs`, `stripe
  agent setup`) rather than three co-equal quickstarts; other entry modes
  (no-code, AI agents) are separate linked pages, not tabs on the same page.
- **Starlight's own `<Tabs>` component** (relevant since grimoire is
  weighing an Astro/Starlight migration) is the standard mechanism sites use
  to avoid tripling a quickstart: one page, one set of steps, a tab switch
  only at the single step that differs (package manager, OS). No example
  found of three *fully separate* getting-started quickstarts kept in sync
  by tabs for CLI vs TUI vs GUI specifically — every comparable (GitHub
  CLI/Desktop/web, Docker CLI/Desktop) documents each surface as its own
  page/product area rather than a tabbed single page, because the surfaces
  diverge too much past step one (a GUI walkthrough is screenshots, a CLI
  walkthrough is commands — they don't share a tab body).

**Recommendation**: grimoire's three entry_paths (quickstart.md CLI,
browse.md TUI+VS Code, first-skill.md author) match the observed norm —
*separate pages per surface*, not one tabbed page — because CLI/TUI/GUI
diverge immediately. Each should independently satisfy "ends on a visible
result" (uv/Deno pattern), not attempt to converge into one shared body.
`browse.md` covering both TUI and VS Code on one page is worth a second look:
it's two genuinely different UIs, closer to the CLI/GUI split that other
projects keep as separate pages, so if either surface's steps grow past a
screen, split it rather than force one narrative to fit both.

**What the draft gets wrong, if anything**: none of the three entry pages
were shown ending on a visible verifiable result in what's specified — that's
an authoring detail to enforce when writing, not a structural defect in the
plan.

---

## 4. Versioning explainer shape

| Source | Mechanism | Verdict |
|---|---|---|
| Helm | Prose only: chart version = SemVer2 (soft-enforced), filename derived from version. No floating/pinned table. | Weakest of the four — no consumer-side pin/float story at all. |
| npm | Caret/tilde range table is the industry-standard reference shape, but doc fetch 404'd this pass — cite from memory-verified prior knowledge only if re-confirmed, otherwise drop the npm row rather than assert unverified. |
| Cargo | Prose + inline formula table: `^1.2.3 := >=1.2.3 <2.0.0`, with `Cargo.lock` framed as the pin. Clear, but framed around *ranges*, not named tag *kinds*. |
| **ocx.sh/docs/in-depth/versioning** (already the owner's named model) | A **named ladder** from most-floating to most-pinned: floating (`:latest`) → major-rolling (`:3`) → minor-rolling (`:3.28`) → patch-rolling (`:3.28.1`) → build-tagged (immutable by convention) → digest (`@sha256:...`, absolute). Explicitly separates "the local index snapshot as a secondary lock" from the primary digest lock. | **Clearest of all four** — it's the only one that (a) names every rung the guide's own T28 needs (exact, rolling major/minor, latest, channel) and (b) states what a resolver-level cache/lock adds on top of the tag mechanism, which is exactly grimoire's own lockfile relationship to registry tags. |

**Recommendation**: the plan's choice to model `guides/versioning.md` on
ocx.sh's ladder is confirmed as the strongest available shape — no better
external example surfaced. Present it as a single ladder table (floating →
rolling-major → rolling-minor → exact → digest) plus one sentence on what
`grim update` re-resolves and what the lock freezes, mirroring ocx's
tag-vs-index-snapshot-vs-digest three-layer split.

---

## 5. Diátaxis: tutorial vs. how-to boundary for the own-index golden path

Direct from diataxis.fr:

- Tutorial purpose is **acquisition of skill**, not task completion:
  "Its purpose is not to help the user get something done, but to help them
  learn." How-to guides assume competence and serve "action and only
  action."
- Tutorials must **"ruthlessly minimise explanation"** and defer it with an
  explicit link-out ("There is a place for extended discussion... but not
  now").
- Conflating the two is named as "the root of many difficulties" — a
  concrete risk if `tutorials/own-index.md` starts explaining *why* OCI
  works partway through the golden path instead of linking to `concepts.md`.

**Applied to `tutorials/own-index.md`** (skill on disk → dev-install →
publish → announce → colleague installs): this is correctly a tutorial per
Diátaxis's own criterion — it's the one place two-or-more interacting
concepts (publish + index + announce) must be learned in combination, which
is exactly the case Diátaxis reserves for tutorials over how-tos. The plan
should stop the tutorial the moment a *second* path branches (e.g. GitLab
self-hosting, or catalog-layout conventions for many artifacts) and link out
to `self-hosted-gitlab.md` / `guides/catalog-best-practices.md` rather than
absorbing them — which the plan already does (those are separate guide
pages, not tutorial branches). No correction needed; the boundary is already
drawn where Diátaxis would draw it.

**What the draft gets wrong, if anything**: nothing found. The one risk to
flag during authoring, not planning: the tutorial's connective step ("the
manifest registry vs. repository_prefix trap" that broke T10's persona) must
be handled as a single stated rule inline, not a link to configuration.md's
fuller explanation — Diátaxis's "ruthless minimisation" argues for restating
the one fact needed, not deferring something the whole path depends on.

---

## Sources

- https://diataxis.fr/tutorials/ , https://diataxis.fr/how-to-guides/
- https://docs.docker.com/ , https://fly.io/docs/ , https://tailscale.com/kb ,
  https://docs.stripe.com/get-started
- https://design-system.service.gov.uk/patterns/navigate-a-service ,
  https://design-system.service.gov.uk/patterns/step-by-step-navigation
- https://docs.asciinema.org/manual/player/options/ ,
  https://docs.asciinema.org/manual/server/embedding/
- https://github.com/charmbracelet/vhs , charmbracelet/vhs-action (CI re-render pattern)
- https://helm.sh/docs/topics/charts/ , https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html
- https://ocx.sh/docs/in-depth/versioning (owner-named model, re-fetched and confirmed)
- https://starlight.astro.build/components/tabs/ , https://docs.astro.build/en/getting-started/ ,
  https://docs.astral.sh/uv/getting-started/first-steps/

**Not verified this pass**: npm's caret/tilde/dist-tag doc page 404'd at the
URL tried; if the npm comparison matters for `guides/versioning.md`, re-fetch
`https://docs.npmjs.com/about-semantic-versioning` instead.
