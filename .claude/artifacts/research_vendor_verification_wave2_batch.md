# Research: wave-2 skills-only batch surface verification

Live verification **2026-07-27** (six parallel `worker-researcher` passes,
first-party sources only) for the six clients added in
`feat/vendor-batch-tier-a`. Sibling artifacts:
`research_vendor_verification_cursor_kiro.md`,
`research_vendor_verification_junie_gemini.md`,
`research_vendor_verification_zed_amp.md`.

W-F shipped Antigravity without one of these and a reviewer flagged the absence
as that branch's weakest audit surface. This is the copy-out.

## Roster and the gate that closed

Six shipped: **Cline, Droid, Goose, Warp, OpenClaw, Kilo**. Nothing was
dropped. Kilo entered the wave gated on resolving `.kilo` vs `.kilocode`; the
gate closed on source evidence (below) and additionally moved the *client name*.

Deliberately out, unchanged from the plan: Windsurf, Trae, Hermes, Roo Code,
Devin.

## 🚨 Corrections to the brief

1. **Kilo ships as `kilo`, not `kilocode`.** The gate asked which directory;
   the answer moved the name too. `.kilo` is source-confirmed via `globalDirs()`
   and `skillDirectories()` in `Kilo-Org/kilocode`, the product has rebranded to
   "Kilo", and `kilocode.ai` 308-redirects to `kilo.ai`. Shipping `kilocode`
   would have frozen a name the vendor is actively retiring.
2. **Goose renders INTO the shared pool; Warp does not.** Both scan
   `.agents/skills` at both scopes, but Goose's own `.goose/skills/` is labelled
   *backward-compatibility* by Goose's docs while `.agents/skills` is named the
   *recommended* location — so the owner principle ("prefer a vendor-specific
   directory") does not apply to a directory the vendor itself calls legacy.
   Warp's `.warp/skills/` is first-class, so Warp renders native and is merely
   pool-*eligible*.
3. **OpenClaw has no project scope at all.** Its docs use the word "project",
   but that path resolves to `~/.openclaw/workspace` — a fixed daemon home that
   does not track the repository. See the ruling below.
4. **Junie's rules decline was factually wrong** (task 7, not one of the six).
   `.junie/rules/*.md` is current, not legacy — step 4 of the discovery order,
   above the step-5 "Legacy guidelines file (still supported)". The surface is
   ownable; the blocker is scoping.

## Verified paths

| Client | Project skills | Global skills | Env override | Confidence |
|---|---|---|---|---|
| **cline** | `.cline/skills/` (first of `.cline/` → `.clinerules/` → `.claude/`) | `~/.cline/skills/` (`%USERPROFILE%\.cline\skills\`) | none honored | raw-text (`skills.mdx`) |
| **droid** | `.factory/skills/<name>/SKILL.md` | `~/.factory/skills/` | none found | raw-text — strongest in the batch |
| **goose** | `.agents/skills/` (shared pool) | `$HOME/.agents/skills/` (shared pool) | none for skills | raw-text, first-party repo |
| **warp** | `.warp/skills/` | `~/.warp/skills/` (cross-platform) | none found | first-party, quoted at both scopes |
| **openclaw** | *(none — no per-repo scope)* | `~/.openclaw/skills/` | `$OPENCLAW_HOME` **unconfirmed** | first-party repo + source |
| **kilo** | `.kilo/skills/` | `~/.kilo/skills/` | none found | **source** (`globalDirs()`, `skillDirectories()`) |

## Pool membership verdicts

The governing rule is scope-blind: a **partial** member must not join
`POOL_CAPABLE_VENDORS`, because `shared_skills = true` would then write global
skills where the client never scans **and nothing would fail**.

| Client | Reads pool? | On roster? | Why |
|---|---|---|---|
| **goose** | yes, both scopes | **YES** | full member; also renders into the pool |
| **warp** | yes, both scopes | **YES** | full member; renders native, pool via opt-in only |
| **cline** | **no** | no | *confirmed absence* — `.agents/skills` appears in neither its project nor global list. The only `.agents/` mention in Cline's docs is `~/.agents/AGENTS.md`, an unrelated rules file |
| **droid** | **no** | no | *confirmed absence*, checked against skills page, settings page and sitemap. Its compat dir is `.agent/skills/` — **singular**, a different convention from the `.agents` pool |
| **kilo** | project only | no | **partial** — project `.agents/skills` loads by default, no global support; nearest upstream is an open unmerged request (#10569). The Antigravity shape |
| **openclaw** | yes, priority 3 | no | **deliberate deferral**, not an evidence gap. Global-only scope model is unlike every roster member; adding later is additive, removing is breaking |

`POOL_ROSTER` in `render.rs` — "who *writes* the shared tree" — gains **Goose
only**. Warp reads the pool but renders natively, so it does not belong there.

**Three independent first-party confirmations of the pool convention now exist**
(Warp, Goose, plus the four incumbents). The `.agents/skills` standard is real
and vendor-documented, not inferred from `vercel-labs/skills`.

## Where the `agents.ts` lead was wrong or incomplete

Treated as a lead throughout, never as evidence. Confirmed wrong or unusable for
**Cline** and **Droid** — both are documented non-adopters of `.agents/skills`,
so any roster entry implying pool membership is false. Confirmed *incomplete*
for **Kilo**, which it lists under the retired `kilocode` name and the
deprecated `.kilocode` directory.

## Declines, with flip conditions

All six decline Rule, Agent and MCP. Only the non-uniform reasons are worth
recording:

- **Cline rules are a live candidate, not a capability gap.** `.clinerules/`
  documents genuine per-file `paths:` frontmatter scoping — the exact mechanism
  whose absence forces a decline everywhere else. Declined only because this
  wave shipped skills-only.
- **Goose MCP is blocked on grim's side, not upstream's.** Goose is
  extension/MCP-heavy, but its config is YAML (`config.yaml`) and grim splices
  JSON and TOML only.
- **OpenClaw MCP** needs JSON5 tolerance — `openclaw.json` mixes strict JSON
  and JSON5 (unquoted keys, a `--strict-json` flag).
- **Warp rules have no on-disk path at all** — global rules are UI/cloud-managed.
- **Kilo's MCP env-ref form is `{env:VAR}`**, *not* the `${VAR}` grim's renderer
  would assume. Recorded now so a future enablement does not write broken
  literals.

## Conflicts left deliberately unresolved

Both are **moot at this scope** — they touch only config files, which are MCP
and agent territory, and both kinds are declined for both clients. Nothing grim
writes depends on either.

- **Goose macOS config root**: docs and source disagree (`~/.config/goose/` vs
  an Application Support path). `$GOOSE_PATH_ROOT` relocates both.
- **Kilo `~/.config/kilo/` vs `~/.kilo/`**: docs and source disagree. Recheck
  after **2026-07-31**, when the legacy `.kilocode` codebase reaches EOL.

The distinction that made both safe to defer, and it generalizes: **a write path
must be exactly one location, so contested evidence means stop. Detection is a
boolean, so it may probe several candidate roots and OR them** — a false
negative costs a missed autodetect and carries no path-correctness risk, because
detection writes nothing.

## Implementation directives distilled

- Five `VENDOR_ROOTS` rows (`cline`, `droid`, `warp`, `openclaw`, `kilo`);
  **Goose gets none** — everything it writes anchors at `AgentsSkills`.
- `droid-root` resolves to `~/.factory`. The tag follows the client name, the
  directory follows the vendor's. Both frozen, both deliberate.
- **Never write `.kilocode`** — read-fallback only, EOL 2026-07-31. Accepted for
  *detection* so an existing install is recognized.
- **No vendor keys detection on `.agents/`.** That matters most for Goose,
  which renders into the pool: keying on it would make Goose detect itself after
  its own first install.
- OpenClaw's project scope is gated by `Vendor::kind_surface(Skill, Project) ==
  false` — the mirror of Junie's `(Rule, Global)` gap. One mechanism, opposite
  directions.

## Sources

Per-client first-party documentation and repositories, fetched 2026-07-27:
cline.bot / docs.cline.bot (`skills.mdx`, raw markdown), docs.factory.ai
(skills, settings, sitemap), block.github.io/goose + `block/goose` raw sources,
docs.warp.dev, the OpenClaw repository and its backward-compatibility source
(the `metadata.clawdbot` / `metadata.clawdis` legacy aliases are first-party
proof of the rename), and `Kilo-Org/kilocode` source (`globalDirs()`,
`skillDirectories()`) plus kilo.ai.

**Evidence caveat, recorded as W-F recorded it:** several pages were read
through a summarizing fetch tool rather than as raw text. Where a claim turned
on a single sentence, the additively-reversible side was taken — decline over
support, off-roster over on-roster, one write path over two.
