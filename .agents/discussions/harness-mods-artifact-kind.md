# Discussion: Shipping harness mods through grimoire

State: handed-off → architect · Updated: 2026-11-18
Ratified: 2026-11-18 → architect
Confidence: ratified by the owner in one session on 2026-11-18 (restate confirmed with an explicit yes); every external claim rests on research fetched 2026-11-18; three load paths are unverified and listed under Open questions

## Intent

Owner, 2026-11-18: "Users are starting to use claude mods, which is awesome but
currently cannot be shipped with grimoire, i am sure there are other similar for
other harnesses as well, also not sure how to model that in grimoire itself, a
type that is only supported for one harness? Or a more general type that has
different layers for each harness?"

A Claude mod is a Claude Code plugin whose `hooks/hooks.json` names a `modules`
file: TypeScript or JavaScript that runs inside Claude Code and changes its
interface or behaviour. Mods launched on 2026-10-01 (Claude Code 2.1.287).
Other harnesses have their own in-process extension APIs with no shared core.

Why now: users already write and share Claude mods, and grim has no kind that
carries one behind its consent mechanism.

Outcome shape chosen at intake: an ADR (`/hex-architect`).

Out of scope:

- Translating a mod from one harness's API to another's. Grim carries each
  harness's code unchanged.
- A neutral schema of interface concepts (theme, pane, band as grim terms).
- Harnesses beyond Claude and OpenCode V2 in the first version. OpenCode V1
  and Kilo (V1 lineage) decline.
- Shipping a complete Claude plugin folder. A mod carries code files only;
  skills, agents, commands, MCP servers and command hooks keep going through
  their own kinds.
- Package dependencies. A mod is self-contained: Claude allows only relative
  imports and `claude-code`, and OpenCode V2 installs nothing for a local
  plugin.
- Reading or enforcing a payload's own version gate, and detecting a managed
  client setting that makes the client skip a mod. Both are documented gaps.
- Signing or publisher verification.
- The fix for the non-gated delivery path described under Threads. It is a
  separate bugfix that lands first.

## Requirements

Provisional, in prose. The ADR assigns identifiers.

- A new artifact kind `mod` carries native harness extension code: table
  `[mods]`, lock array `[[mod]]`, flag `[options.experimental] mods`. It is
  appended last in the kind order (`src/oci/artifact_kind.rs`), because bundle
  members sort by that order.
- One content layer. Inside it, one top-level directory per harness: `claude/`
  and `opencode/` in the first version. Each holds code files only. Grim
  copies those files unchanged for each present client and reads the set of
  supported clients from the directory names. An artifact may hold one
  directory or several.
- Each harness directory has one entry point. The descriptor may name it per
  harness; when it does not, the entry defaults to the mod's own name without
  its path namespace. `grim build` resolves the entry and fails when it is
  missing or ambiguous, so the published artifact always states it.
- No artifact of any kind carries a Claude plugin manifest. For Claude, grim
  generates the wrapper around the entry at install time: a `plugin.json` with
  the name (the documented minimum) and a `hooks/hooks.json` naming the one
  module. For OpenCode V2, grim places the files in a subfolder per mod and
  generates the `index` file OpenCode requires there. Generated files are
  rendered output under the renderer stability rules: re-materializing leaves
  `status` not-modified.
- Project scope installs into the client's project folder, in a location of
  the mod kind's own, not the Claude skills directory. This is a recorded
  exception to invariant I1 in `.claude/rules/arch-threat-model.md` ("nothing
  armable lives inside a repository"), which the ADR must write down with its
  consequence: a committed mod loads for a cloner under the client's own
  rules, without grim's consent on that machine.
- A client with no directory in the artifact declines it visibly, with the
  existing decline behaviour (warn, skip, zero files), through
  `client_supports_kind` in `src/install/installer.rs`. Status, describe and
  the browse views show which clients a mod serves.
- Nothing the kind installs is written without workspace consent. Consent is
  the one workspace record and the existing `--trust-workspace` /
  `--no-trust-workspace` pair, with no environment form. The record holds the
  workspace and a plain list of sources (`registry/repository`), with no kind
  and no prefix. A hook, settings document or mod from a source not in the
  list asks again; a new version of a listed source does not. The same holds
  for a mod reached through a bundle or a dependency.
- `grim export plugin` carries a mod into an exported Claude-family plugin in
  the first version, reusing the generated wrapper.
- The wire format is frozen from the first release, experimental flag or not
  (`docs/src/content/docs/stability.md`). Older grim binaries must fail closed
  on the new kind; the ADR states what 0.15.x and 0.16.0 do with a lock, a
  bundle member and a dependency of this kind, from a run against the old
  binaries.
- The ADR amends `.agents/adr/adr_artifact_trust_model.md`,
  `.agents/adr/adr_hook_workspace_consent.md` and
  `.claude/rules/arch-threat-model.md`, and reopens the recorded decline of
  themes and output styles in `.agents/upstream-watchlist.md`, whose reopen
  condition (a user request) is now met.

## Decisions

Owner, 2026-11-18:

- **Installed kind.** `grim install` writes the mod itself. Not export-only,
  and not carried inside another kind.
- **The kind is named `mod`.** `plugin` and `extension` collide with existing
  grim vocabulary.
- **One consent mechanism: consent is given to a workspace.** Not per kind and
  not per artifact. A mod gets no mechanism of its own and no look-alike. The
  record stores no kind prefix.
- **The record holds the workspace plus a plain source list.** A source not in
  the list asks again. This keeps the rule in
  `.agents/adr/adr_hook_workspace_consent.md` that a new source is a new
  decision and a new version is not.
- **The consent record may break.** It is an experimental surface (shipped in
  0.15.x and 0.16.0, not frozen). It moves out of the hook-named folder to
  `$GRIM_HOME/consent/<workspace-key>.json`. No migration ceremony: existing
  records are asked again.
- **Hooks already install runtime code.** The sentence in
  `docs/src/content/docs/clients.md` ("grim will never install a plugin,
  extension, or any other runtime code into your client") is about grim's own
  shims for closing capability gaps, not about code a publisher ships. A mod
  kind needs at most a clarifying clause there.
- **First version serves Claude plus OpenCode**, so the format is proven with
  two real payloads before it freezes. OpenCode means the **V2** plugin API
  only.
- **Layout: harness directories.** One layer, one top-level directory per
  harness. Chosen over one harness per artifact with a bundle grouping them.
- **Entry point, not a plugin folder.** The author ships code files and an
  entry point; grim generates the client's wrapper files. The owner's reason:
  the Claude packaging makes a mod look like a hook, which it is not, and both
  Claude files are boilerplate for a mod.
- **The entry is optional** and defaults to the mod's name without its path
  namespace.
- **Project scope installs into the client's project folder**, not under
  `$GRIM_HOME`. **Not the Claude skills directory**: it is the wrong home for
  a mod and adds a name collision with skills.
- **Claude-family export in the first version.**
- **The non-gated delivery path is fixed first, as its own bugfix.** Non-mod
  kinds refuse a Claude plugin manifest with a named reason, after one
  logged-in run confirms the module executes.

## Threads

- **Kind shape (closed).** Single-harness kind versus one harness-neutral kind
  with a native payload per harness. Settled as a neutral kind with harness
  directories; see Decisions. Council, 2026-11-18 (three seats, blind to each
  other; each read little beyond the recon artifact, so their verdicts are not
  independent of it): the user-advocate seat picked the neutral kind, the
  premortem seat the single-harness kind, the simplicity seat no new kind. All
  three agreed on no vendor name in the kind literal, the experimental flag
  with hook-style consent, and that a designed neutral schema is premature.
- **Installed kind or export seam (closed).** Installed; see Decisions.
- **A non-gated kind can already deliver client-loaded code (closed as a
  decision, open as work).** Checked 2026-11-18 with grim 0.15.1 and Claude
  Code 2.1.327: the grim half is confirmed, the Claude half is confirmed up to
  "plugin loaded" at user scope; execution of the module itself was not
  observed, because the isolated run was not logged in. Evidence and the exact
  path are in `.agents/research/research_harness_mods_skill_dir_check.md`. It
  contradicts the rule that every such artifact passes the one consent
  mechanism. Bugfix first; see Decisions.
- **The no-runtime-code promise (closed).** See Decisions.

## Research

- `.agents/research/research_harness_mods_codebase_recon.md` (2026-11-18):
  the kind touch list (`settings` was 172 files, `hook` 115), how per-vendor
  payloads exist today (vendor-keyed tables in one layer, never one layer per
  harness), the recorded decline of themes and output styles with its reopen
  condition, the trust ADR's position on a third gated kind, what older grim
  binaries do with an unknown kind, and the local mod format (Claude Code
  2.1.327: `.claude-plugin/plugin.json`, `hooks/hooks.json` with `modules`, a
  TypeScript `hooks/register.tsx`).
- `.agents/research/research_harness_mods_prior_art.md` (2026-11-18, every
  claim with URL and fetch date): a mod is a plugin plus a `modules` key in
  `hooks/hooks.json`, with no manifest or install path of its own; introduced in
  Claude Code 2.1.287, the docs say events and methods can change between
  releases, and 2.1.323 to 2.1.327 each changed mod semantics; a version gate
  exists (`engines.claude-code`); no per-mod consent prompt, only the folder
  trust dialog and a managed-settings guard; a plugin directory under
  `~/.claude/skills/` or `<project>/.claude/skills/` also loads ("skills-dir"
  origin). Eight harnesses have separate in-process code extension APIs with no
  shared core (Claude, OpenCode, Kilo, Amp, pi, OpenClaw, Cline, Copilot CLI
  experimental). Agent Plugins 1.0 carries vendor content under a
  reverse-domain `extensions` namespace that other clients must ignore.
- `.agents/research/research_harness_mods_vendor_distribution.md`
  (2026-11-18): no vendor makes a mod a package type, each rides inside the
  vendor's own plugin unit; the closest single-harness precedent is Microsoft
  APM's Copilot-only canvas primitive (experimental, gated on one target,
  unsupported elsewhere, deployed verbatim, behind an executables approval);
  the closest multi-harness mechanism is the Agent Plugins 1.0 `extensions`
  namespace with one top-level directory per namespace; no third-party manager
  carries UI mods, and rulesync's generated OpenCode plugin broke on OpenCode
  V2; no product models a pane or band as neutral content.
- `.agents/research/research_harness_mods_community.md` (2026-11-18): mods are
  shared as GitHub repositories used as marketplaces; the loudest pains are
  team rollout, conflicts between mods and the unsandboxed trust model;
  cross-harness demand for UI mods is thin (4 of 278 comments in the design
  thread), while lockfile and signing requests are rare; Qwen Code is building
  a reader for a subset of the Claude mod contract.
- `.agents/research/research_harness_mods_skill_dir_check.md` (2026-11-18):
  the check behind the "non-gated kind" thread above.

- `.agents/research/research_harness_mods_open_questions_codebase.md`
  (2026-11-18): the consent record is `{v:1, workspace, hooks:[...],
  consented_at}` with unknown fields denied; a key is
  `binding@registry/repository`, prefixed `settings:` for settings, with no tag
  or digest, so a version or content change never re-asks; `split_by_kind`
  (`src/hook/consent.rs`) sends every key without the `settings:` prefix to the
  hook half, so a third prefix reads as hook drift until that is generalised;
  only grim reads the consent path (not the launcher, not the `grim-home-*`
  refusals), the path appears in `docs/src/content/docs/hooks.md` and four
  catalog pages, and no precedent keeps a permanent read-fallback; `plugin`
  collides heavily with the export vocabulary, `extension` collides with the
  frozen docs anchor `artifacts.md#vendor-extensions`, `mod` and `addon` have
  no hits; an export family omits a kind it cannot represent with exit 0 and an
  `omitted` entry; a hook's name comes from `hook.toml` and is stamped as the
  title annotation, while installed paths and consent keys use the consumer's
  binding; `!node_modules/` in `.grimignore` re-includes dependencies; no
  cross-artifact path collision detector exists.

- `.agents/research/research_harness_mods_claude_wrapper_facts.md`
  (2026-11-18, checked with `claude` 2.1.327 without login): a manifest needs
  only `name`; `modules` is relative to `hooks/`; one module per plugin, a
  second is a validate error; a module outside `hooks/` validates, its runtime
  load is unverified; no manifest or `hooks.json` change between 2.1.287 and
  2.1.327 breaks a minimal wrapper; a plugin under the skills directory loads
  in place with no `SKILL.md`, and loses to a marketplace plugin of the same
  name; a directory marketplace registered through `extraKnownMarketplaces`
  plus `enabledPlugins` also loads in place; a hand-written cache entry and a
  project `.claude/plugins/` do not load; managed settings can skip a
  skills-directory plugin silently, with nothing observable from outside;
  `engines` is advisory for a local folder; a module may import only relative
  files and `claude-code`, so a mod is self-contained by rule.
- `.agents/research/research_harness_mods_opencode_plugin_facts.md`
  (2026-11-18, from source, nothing executed): two incompatible OpenCode lines
  are live, V1 (1.19.x, `@opencode-ai/plugin`) and V2 (2.3.x,
  `@opencode/plugin`, npm `latest` since 2026-11-18), and a V1 plugin does not
  run in V2; V2 scans `plugin/` and `plugins/` one level deep, loads every
  top-level `.ts` or `.js` file as a plugin, and loads a subdirectory only
  through `index` or `server`; V1 loads top-level files only; neither asks
  before loading a project plugin, so it loads on clone; V2 installs no
  dependencies for a local plugin; a duplicate plugin id stops activation in
  V2; Kilo is the V1 lineage and does not load a V2 plugin.

## Related

- `.agents/adr/adr_settings_artifact_kind.md` (checklist for adding a kind;
  "one document, vendor objects, one layer" ratified over one layer per harness)
- `.agents/adr/adr_artifact_trust_model.md`, `.agents/adr/adr_hook_workspace_consent.md`
- `.agents/adr/adr_harness_plugin_export.md`, `.agents/adr/adr_harness_marketplace_export.md`
- `.agents/upstream-watchlist.md` (declined: output styles, themes, monitors)
- `.claude/rules/arch-threat-model.md`, `.claude/rules/subsystem-install-layout-hooks.md`
- `docs/src/content/docs/clients.md`, `docs/src/content/docs/stability.md`
- `src/oci/artifact_kind.rs`, `src/oci/manifest.rs`, `src/install/installer.rs`,
  `src/install/materializer.rs`, `src/skill/skill_package.rs`,
  `src/install/ignore_set.rs`, `src/hook/`, `src/export/family.rs`
- [grimoire-rs/grimoire#141](https://github.com/grimoire-rs/grimoire/issues/141) (Claude gap kinds)

## Open questions

- [NEEDS CLARIFICATION: Which project-folder location does Claude load a mod
  from, given the skills directory is ruled out?]
  Recommended: a directory marketplace inside the project (working name
  `.claude/mods/`) that grim registers through `extraKnownMarketplaces` and
  `enabledPlugins` in the project's Claude settings — the research found only
  two in-project routes that load, the skills directory and a registered
  directory marketplace; `.claude/plugins/` and a hand-written cache entry do
  not load. The ADR must verify the project-scope load with a relative path,
  what a cloner is shown, and how these two settings keys coexist with the
  `settings` kind, which may own the same keys.
- [NEEDS CLARIFICATION: Where do mods go at global scope?]
  Recommended: the same route under the user's Claude root (following
  `CLAUDE_CONFIG_DIR`), and OpenCode's global plugin folder (following
  `OPENCODE_CONFIG_DIR`, else the XDG `opencode` folder).
- [NEEDS CLARIFICATION: Which name does the default entry use, and with which
  file extension?]
  Recommended: the name in the mod's own descriptor, resolved at `grim build`
  and recorded in the artifact — installed paths use the consumer's binding,
  which can differ, so a default resolved at install would miss the file.
  Accept exactly one file `<name>.<ext>` among the extensions the harness
  loads, and fail the build on none or several.
- [NEEDS CLARIFICATION: OpenCode V2 stops activating when two plugins share an
  id, and the id is set in the author's code. Does grim check it?]
  Recommended: no — grim does not parse the code; say so in the authoring docs.
- [NEEDS CLARIFICATION: Claude accepts one hooks module per plugin. What does
  `grim export plugin` do with a plugin that has several mods?]
  Recommended: carry the first, omit the rest with a named reason in the
  `omitted` list, as a hook is omitted today — a module that combines several
  would be grim-authored runtime code, which `clients.md` rules out.
- [NEEDS CLARIFICATION: What happens to consent records at the old path
  `$GRIM_HOME/hooks/consent/`?]
  Recommended: ignore them and delete the folder on the first write of a new
  record — no import, so a stale record can never restore a revoked consent.
- [NEEDS CLARIFICATION: Which Claude manifest fields may an author set?]
  Recommended: none in the first version — `engines` is advisory for a local
  folder and several other keys make older Claude versions reject the manifest.
- [NEEDS CLARIFICATION: Does a module placed outside `hooks/` load at runtime?
  `claude plugin validate` accepts it, the load itself was not observed.]
  Recommended: have the generated wrapper keep the entry inside `hooks/` unless
  a logged-in run shows the other layout loads.

## Verification

- The ADR passes `/hex-architect`'s review panel and the cross-model pass
  configured in `.agents/memory/hex.md`, with the stability perspective first.
- Its claim diff against the research artifacts above holds: every fact it
  restates about Claude mods or OpenCode plugins carries a source fetched on or
  after 2026-11-18, re-fetched if the ADR is written later than 2027-02.
- The eventual implementation is checked by acceptance tests under `test/`
  that: install one artifact with both directories into a workspace with both
  clients and find each code file byte-identical at its client's location and
  the generated Claude wrapper equal to a golden; build one artifact with no
  declared entry and find the default resolved; re-materialize and see `status`
  report not-modified;
  install it with one client and see the other decline with zero files; show
  that nothing is written before workspace consent and that
  `--no-trust-workspace` withholds it; show the same through a bundle member
  and a dependency; and run the old-binary golden against 0.15.x.
- The earlier bugfix is checked by its own failing-first regression test: a
  non-mod artifact carrying a plugin manifest is refused with the named reason.
- `task verify`, `task catalog:verify` and `task docs:check` pass.
