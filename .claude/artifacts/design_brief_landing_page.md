# Design Brief — grimoire.rs landing page

Self-contained handover. Everything a designer needs is in this file; no
repo access required. Copy verified against grim 0.11.1 on 2026-07-26.

---

## 1. The ask

`grimoire.rs` today renders a documentation chapter at its root — sidebar,
theme switcher, prev/next nav, no landing page. **Design a landing page for
the site root.**

Not a rebrand, not a docs redesign. One page: the thing a stranger sees when
someone links "grimoire.rs" in a comment thread.

## 2. What Grimoire is

A package manager for AI-agent configuration. The binary is `grim`.

Coding agents (Claude Code, Copilot, Cursor, and so on) are steered by config
files — *skills*, *rules*, *agents*, *MCP servers*. Today people copy those
between repos by hand. A rule written for one project gets pasted into the
next, then drifts: no version, no provenance, no upgrade path. **There is no
`npm install` for an agent skill.**

Grimoire makes each one a versioned, content-addressed artifact, distributed
through ordinary OCI registries — the same infrastructure that ships
container images. You declare what you want in `grimoire.toml`, exact digests
are pinned in `grimoire.lock`, and `grim install` materializes the files into
whichever agents you use. There is **no Grimoire service** — storage is a
registry you already have.

## 3. Audience and the bar

A developer who runs **three or more coding agents** and is tired of
copy-pasting the same rules between them. Technical, skeptical, has seen
twenty "npm for AI skills" projects this year.

**The bar: ten seconds.** They land, and either understand what this is and
why it is different, or they close the tab. The page's whole job is to win
that ten seconds honestly.

## 4. Verified copy — use as written

These strings are checked against actual CLI behaviour. Do not paraphrase the
technical ones.

**Headline**
> A package manager for AI-agent config — skills, rules, agents, and MCP
> servers, installed into every coding agent you use.

**Second line — must accompany the headline wherever it appears**
> Storage is any OCI registry — GHCR, Docker Hub, or your own. There is no
> Grimoire service to sign up for.

**Short hero / social-card line**
> One skill, ten coding agents, one lockfile.

**Install** (macOS / Linux)
```sh
curl --proto '=https' --tlsv1.2 -LsSf https://setup.grimoire.rs/sh | sh
```

**Quick start** — every command verified to exit 0 in this exact order
```sh
grim init                                        # create grimoire.toml
grim add ghcr.io/grimoire-rs/skills/grim-usage   # declare, lock, install
grim install                                     # re-materialize after a clone
grim tui                                         # browse the index
```

The long reference is deliberate — a bare `grim-usage` does **not** resolve.
Do not shorten it to make the block prettier.

**The ten clients**, in this order: Claude Code, Copilot, Cursor, Codex,
Gemini, Zed, Amp, Kiro, Junie, opencode.

## 5. Content blocks, in priority order

Ranked by how much each earns in the first ten seconds. The designer decides
how many survive — but not the order.

1. **One artifact → ten agents.** The strongest differentiator; nothing else
   surveyed does it. Each client gets its native format, and clients that
   genuinely cannot support something are *declined honestly* rather than
   faked. **Lead this on rules and agents, not skills** — see §6.
2. **Lockfile, digest pinning, `grim update`.** Reproducible, and upgrading
   is one command.
3. **No service to run.** Reassurance, never the lead — as a headline it
   reads as a barrier ("so I need a registry?"); as a follow-up it reads as
   freedom.
4. Supporting credibility, only if the layout has room: MCP servers as
   first-class artifacts · publish your own + self-hostable index · bundles ·
   a frozen stability contract with a JSON interface for scripting.

A visual that shows *one source artifact fanning out into many differently
shaped client files* would carry point 1 better than prose. That fan-out is
the product.

## 6. Do not say — each of these is a factual trap

- **"We translate skills for every vendor."** Anthropic's Agent Skills format
  became a cross-vendor open standard in Dec 2025 (Microsoft, OpenAI,
  Atlassian, Figma, Cursor, GitHub). Claiming skill translation invites
  correction. **Rules and agents genuinely do still diverge** —
  `CLAUDE.md` vs `AGENTS.md` vs `.cursorrules` vs
  `copilot-instructions.md`. Make the cross-vendor claim there.
- **Anything about drift detection or CI gating.** `grim status` is a report,
  not a gate — it always exits 0. The claim becomes available later; not now.
- **Registry scale, catalog size, or ecosystem.** The public index holds 12
  packages, all first-party. Never imply a populated ecosystem.
- **Adoption, users, companies, headcount, logos, testimonials.** None. No
  anonymized "used at a large org" either.
- **Star counts, "trusted by", social proof of any kind.** There isn't any
  yet, and inventing it is the fastest way to lose this audience.
- **Marketing register generally.** House style is explicit: *"No sales pitch
  or marketing opener. Let examples make the case."* Match that.

## 7. Competitive context

Crowded, not blue ocean. Design should not look like the rest of the field.

| Project | Scale | Gap Grimoire fills |
|---|---|---|
| Vercel skills.sh | 27.2k★ | Documents having no dependency resolution, no cross-vendor rendering, no lockfile |
| Tessl | $125M raised, Snyk founder | Hosted registry, not OCI |
| ClawHub | 3,000+ skills | Hosted; no lockfile or digest model |
| skillctl | — | Sigstore signing, much narrower scope |

Grimoire is the **infrastructure-reuse** answer: no new registry, no new
trust boundary, no sign-up. Honest about being small.

## 8. Visual identity

- **Logo** — flat illustration: an open book in violet/indigo with pale
  lavender pages, three amber four-pointed sparkles rising above it. Rounded,
  friendly, not corporate. Available as SVG and PNG.
- **Palette** — violet/indigo primary, amber accent, taken from the logo.
- **Current site** — mdBook, `navy` theme (dark blue) as both the default and
  the preferred dark theme. The landing page does not have to inherit this,
  but must not clash with the docs one click away.
- **Tone** — a Rust systems tool. Precise, dense, unhurried. Think the
  reference documentation of a tool you already trust, not a startup splash.

## 9. Hard technical constraints

The page ships inside an mdBook static site. These are not negotiable:

- **Fully self-contained.** Inline CSS. No CDN, no external fonts, no
  external JS, no analytics. Assets are inlined or local files.
- **Static only.** No build step beyond mdBook, no framework, no React.
- **Must survive the theme switcher.** mdBook ships five themes (light, rust,
  coal, navy, ayu) and the reader can change it at any time. Either commit to
  one self-contained look that ignores the switcher, or handle light and dark
  — but say which, explicitly.
- **Responsive**, including the code blocks. They must scroll inside
  themselves rather than making the page scroll sideways.
- **Verified implementable:** mdBook 0.5.3 exposes an `is_index` flag in its
  template context (confirmed locally 2026-07-26 — the site root renders with
  it set, the same chapter's own page renders without). So the root page can
  branch to a completely different layout — full-bleed, no sidebar — while
  every docs page is untouched. **No site migration is needed.**

## 10. Deliverable

A **single self-contained HTML file** with inline CSS — the landing page as
it should appear at `grimoire.rs/`. That gets ported into the mdBook template
behind the `is_index` branch.

Include, if the design implies them:
- the light/dark decision from §9, stated
- any SVG or illustration inline
- a 1280×640 social-preview card, if the visual language suggests one
  (none exists today, and the repo has no OG image at all)

Wireframe or mockup first is fine — the HTML matters more than the polish of
the intermediate.

## 11. Known issues to avoid inheriting

`docs/src/introduction.md` carries a stale status block ("Grimoire is
young… moving toward 1.0"). Do **not** carry that wording onto the landing
page. Current framing: stabilizing toward 1.0, released surfaces are frozen
contracts.
