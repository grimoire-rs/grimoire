# Discussion: Mirroring of artifacts into grimoire
State: handed-off → loop · Updated: 2026-11-24
Ratified: 2026-11-24 → loop
Confidence: ratified by the owner in session. Decisions rest on seven research files dated 2026-11-23 (`.agents/research/research_artifact_mirroring_*.md`). Decisions marked "assistant" were taken on delegation and not confirmed one by one.

## Intent

Owner's words: "mirroring of artifacts into grimoire".

Meaning chosen by the owner (2026-11-23): take content authored elsewhere
(git repositories of skills, harness plugin marketplaces) and republish it
as grim OCI artifacts, the shape `ocx-mirror` has for upstream releases.

Out of scope: copying existing grim artifacts from one OCI registry to
another (corporate or air-gapped mirror). `docs/src/content/docs/publishing.md`
documents that as "use skopeo or oras", and the lock embeds the registry host
(`src/lock/locked_source.rs`), which is a separate problem.

Outcome shape: the owner first chose an ADR through `/hex-architect`
(2026-11-23). At the drain (2026-11-24) the owner redirected to a goal loop
through `/hex-loop`. Every point below that names "the ADR" is unchanged: the
loop reaches that ADR as its first deliverable (the assistant's reading of the
redirect, not an owner statement).

## Requirements

Provisional, in the owner's words where quoted (2026-11-23):

- Import artifacts out of a monorepo: one upstream repository yields many
  artifacts ("import like artifacts of [a monorepo]").
- "Then we may want to bundle them": the mirror can publish bundles whose
  members are mirrored artifacts.
- "Mirror multiple rules and so forth": every non-skill kind an upstream
  carries is a candidate, not skills alone.
- Harness marketplaces as an input "maybe ... from the beginning"; the owner
  left that call to the research (see Decisions).

## Decisions

- `ocx-mirror` is the reference, held loosely. The owner: "lean on OCX
  Mirror, but not as strict". Its recorded decisions are the starting point;
  where grimoire's content differs (git repositories without releases, many
  units per repository, bundles) the design departs without ceremony.
- Operator model, chosen by the owner: a tool anyone can run, plus a
  hand-picked list the project runs under its own namespace. Not bulk ingest.
  Strongest remaining counter-argument, noted once: the project becomes the
  redistributor of record for every entry and the bottleneck for "why is X
  not mirrored".
- Marketplace input, decided by the assistant on the owner's delegation: a
  git repository path at a pinned commit is the one source primitive. A
  harness marketplace file is a second way to enumerate such paths, and each
  plugin it lists maps to one bundle. It is in the ADR's scope from the
  start. Reason: Claude Code marketplace entries are themselves pointers to
  git sources with `ref` and `sha`
  (`.agents/research/research_artifact_mirroring_vendor.md`), so the second
  input adds an enumerator, not a second fetch path. Marketplace entries
  whose source is `npm`, `archive` or `command` are declined in the first
  version. Alternative not taken: marketplace files as the only input, which
  would tie the mirror to one vendor's file format.
- Kinds, chosen by the owner: the mirror republishes skills, rules, agents,
  and bundles of them. Hooks, MCP servers and settings are refused in the
  first version, because grim has no signing and nothing would tell a user
  that mirrored executable content reached them unchanged from its author.
  Consequence to design for: a plugin that carries a refused member mirrors
  as a partial bundle, and the mirror must report each skipped member.
- Consent, chosen by the owner: an upstream needs a licence that permits
  redistribution, and an author opts out by asking. No author contact or
  marker file is required. Amended by the owner (2026-11-23): "not always
  the license is known. So we should at least be able for a repository to
  state its license if it cannot be derived." So the licence is detected
  where possible, and otherwise a mirror entry may declare it. The artifact
  records the licence and whether it was detected or declared by the
  operator. With neither, the unit is refused. Consequence to design for:
  monorepos often carry a licence per unit, so detection and declaration
  both work per unit as well as per repository, and "permits
  redistribution" needs a concrete allowlist.
- Translation into grim's general form, decided by the assistant after the
  owner asked how a Claude plugin becomes a harness-neutral artifact
  (2026-11-23). Situation: grim's canonical skill is an agentskills-spec
  `SKILL.md` whose vendor-only fields live under `metadata` as
  `<vendor>.<field>` keys (`docs/src/content/docs/vendor-metadata.md`), and
  the canonical agent has common `name`, `description`, `model`, `tools`
  plus the same namespaced keys (`docs/src/content/docs/agents.md`). A
  Claude-authored file carries Claude's fields at the top level. Ruling:
  each mirror entry names the dialect its upstream is written in; the
  mirror applies the inverse of grim's install-time lift, moving that
  vendor's top-level fields under `metadata.<vendor>.*` and leaving common
  fields in place. The published artifact is the general form and installs
  on every client grim supports. A plugin or marketplace file is read only
  as a table of contents and is never republished. First dialects: plain
  agentskills (no change) and Claude; others are additive. Limits: body
  text is copied verbatim, so prose that names one harness's tools or a
  variable such as `${CLAUDE_PLUGIN_ROOT}` is not rewritten, and `model`
  values pass through untranslated as they do for native artifacts.
  Alternative not taken: publishing the upstream file untouched, which
  would leave Claude-only keys at the top level and break the spec for
  other clients.
- Namespace, decided by the assistant from the owner's operator choice:
  mirrored artifacts land under the namespace of whoever runs the mirror,
  never under a name that imitates the upstream author. Situation:
  `ocx-mirror` publishes under vendor-identity namespaces
  (`bazelbuild/bazelisk`) with a human-reviewed first claim, while
  grimoire's index namespace is the announcing forge identity, checked
  against a live `owner.id` (`docs/src/content/docs/package-index.md`).
  Ruling: keep grimoire's rule unchanged; the upstream origin travels as
  metadata. Alternative not taken: vendor namespaces, which would need a new
  claim-review process in the index. Example: a skill from
  `github.com/acme/skills` mirrors to `<operator>/…/<name>`, with `acme` in
  the `repository` field, not in the namespace.
- Upstream origin, decided by the assistant: carried in the existing
  `repository`, `homepage` and `license` fields, which already map to
  artifact annotations. Whether an explicit "this is a mirror" marker is
  also needed (a new additive `com.grimoire.*` annotation) is left to the
  ADR.
- Tool home, chosen by the owner: a separate repository and binary, plus
  grim gains PATH plugin dispatch so the tool runs as `grim mirror`, the
  shape `ocx mirror` has. The mirror fetches upstream, writes a local
  artifact tree and calls `grim publish`. Strongest remaining
  counter-argument, noted once: dispatching `grim-<name>` from `PATH` is a
  new entry in grim's CLI contract and lets whatever controls `PATH`
  (a repository's `.envrc`, a devcontainer) decide what `grim <name>` runs;
  the ADR must say which names dispatch and from where.
- Version discovery scripts, decided by the assistant on the owner's "not
  sure if this is required here": not in the first version. Situation:
  `ocx-mirror` lets an entry run a script to translate upstream versions.
  Ruling: a tag pattern per entry plus the subtree commit cover every
  surveyed repository, including the one whose tags are 68% automation
  noise. Alternative not taken: a script hook, which adds code execution to
  the mirror for no surveyed case.
- Version source, chosen by the owner: where the upstream repository has
  semver tags, the mirror publishes at each tag under that version; where it
  has none, the version derives from the commit that last changed the unit's
  subtree. An entry can force snapshot mode. Declared per-unit versions are
  not a version source. Consequences to design for: two schemes side by
  side; with tag mode every unit in a repository gets the repository's
  version; a repository that rarely tags stays stale until its entry is
  switched to snapshot mode.
- Build steps, chosen by the owner: the mirror handles only upstreams whose
  files are installable as committed. An upstream whose content exists only
  after its own installer renders it is refused and reported as unsupported.
  The mirror runs no upstream code and, in the first version, no
  operator-written transform script either.

## Threads

- Closed by evidence (owner's concern, 2026-11-23): do upstream repositories
  ship content that differs per harness, which grim's single generalized
  form could not hold? Survey of 17 highly starred repositories
  (`.agents/research/research_artifact_mirroring_repo_survey.md`): 9 keep
  one shape, 4 generate per-harness copies from one source, 4 are mixed, and
  none hand-maintains different instructions per harness as its pattern.
  Skill bodies differed only by path and frontmatter. Real variants
  concentrate in slash commands and hooks, which the mirror does not
  republish. Working position: the mirror imports the one canonical source
  tree an entry names and ignores rendered copies; committed generated trees
  (for example about 17 in one surveyed repository) are excluded by the
  entry's path, not detected.
- Closed by evidence (owner's question, 2026-11-23): which version schemes
  exist. Survey of 15 repositories
  (`.agents/research/research_artifact_mirroring_version_signals.md`): no
  repository tags individual skills or plugins; 6 have repository-level
  semver tags; 6 fill a per-unit version field for most units; 4 have
  nothing but the commit. Declared versions drift (one skill: 58 commits at
  `1.0.0`; one repository: 2132 commits since its last tag). The Agent
  Skills standard defines no version field. The commit that last changed a
  unit's subtree is the only signal every upstream has.

## Research

- `.agents/research/research_artifact_mirroring_recon.md` (2026-11-23):
  grimoire has no import path; content becomes an artifact only through
  `grim build`, `grim release` and `grim publish` over local directories.
  The trust model is integrity-only with no signing. The index namespace is
  the forge identity. `ocx-mirror` signs its own output, carries upstream
  signatures verbatim, and is never a trust point.
- `.agents/research/research_artifact_mirroring_prior_art.md` (2026-11-23):
  how other ecosystems handle third-party ingest. Many claims there are
  marked `[search]` and are leads, not verified facts.
- `.agents/research/research_artifact_mirroring_vendor.md` (2026-11-23):
  Open VSX stores a copy and lists by pull request, with an OSI-licence check
  by a human; Claude Code marketplaces store pointers with `ref`/`sha` and
  fall back to the commit SHA as the version; the MCP registry lets a
  non-author publish only under their own namespace. Smithery is unverified.
- `.agents/research/research_artifact_mirroring_adjacent.md` (2026-11-23):
  version schemes for upstreams without releases (Debian
  `0.0~git<date>.<hash>`, Go pseudo-versions, conda-forge
  `0.0.0.YYYY.MM.DD`), each with or without a separate packaging revision;
  distributions give every unit split from one source the same version; the
  Go proxy refuses removal requests but caches unlicensed modules only
  temporarily; Debian refuses to distribute software with no licence.
- `.agents/research/research_artifact_mirroring_archaeology.md`
  (2026-11-23): the full ocx-mirror decision inventory with a grimoire
  verdict each. Facts the design must respect: `grim publish` rejects
  `+build` metadata and any non-strict-semver version, so the ocx tag shape
  `X.Y.Z_<ts>` does not carry over; skip-when-tag-exists is already the
  `grim publish` default; ocx has no recorded decision on licence, consent
  or takedown, nor on upstream bytes changing under one version; ocx-mirror
  lives in its own repository and shells out to `ocx package push`; a commit
  on `main` (`a174a953`, not on this branch) makes grim refuse a skill whose
  root holds `.claude-plugin`.
- `.agents/research/research_artifact_mirroring_repo_survey.md`
  (2026-11-23) and
  `.agents/research/research_artifact_mirroring_version_signals.md`
  (2026-11-23): summarized under Threads. Also from the survey: only 3 of 17
  repositories carry a licence per unit; one official marketplace lists 317
  entries of which 261 point at other repositories.
  (the full read of the ocx-mirror decision records and grim's version and
  annotation contracts).

## Related

- Sibling repositories `ocx-sh/ocx-mirror` and `ocx-sh/ocx` (decision
  records under `.claude/artifacts/` in each).
- `.agents/adr/adr_artifact_trust_model.md`,
  `.agents/adr/adr_push_pull_registry_split.md`,
  `.agents/adr/adr_artifact_dependencies.md`.
- `.agents/discussions/marketplace-phase-2.md` and
  `.agents/discussions/harness-native-marketplaces.md`: the owner's earlier
  ruling that the marketplace is hand-curated, not a mirror of every index
  bundle.
- `docs/src/content/docs/package-index.md`, `docs/src/content/docs/stability.md`.

## Open questions

Carried to the ADR; none blocks the hand-off.

- [NEEDS CLARIFICATION: what is the exact grammar of a snapshot version?]
  Recommended: a strict-semver form with no `+build` part, since
  `grim publish` rejects both; it must order by commit date and order below
  a later upstream release of the same unit. Check first how grim's floating
  tags treat a prerelease, because that decides whether a `0.0.0-<date>.<n>`
  shape is usable.
- [NEEDS CLARIFICATION: which names does plugin dispatch run, and from
  where?] Recommended: only for a subcommand grim does not define, built-in
  names always win, the binary is `grim-<name>`, and the ADR states the
  search location with the `PATH` concern from Decisions answered against
  `.claude/rules/arch-threat-model.md`.
- [NEEDS CLARIFICATION: is an explicit "this is a mirror" marker needed on
  the artifact?] Recommended: yes, one additive `com.grimoire.*` annotation,
  so clients and the index can label mirrored content without parsing the
  `repository` field.
- [NEEDS CLARIFICATION: what happens when the author publishes natively or
  asks for removal?] Recommended: the entry leaves the mirror list and the
  published versions are marked with the existing `deprecated` and
  `replaced-by` fields; nothing is deleted, so existing locks keep
  resolving.
- [NEEDS CLARIFICATION: which licences permit mirroring, and where is the
  licence read?] Recommended: an allowlist of SPDX identifiers for
  OSI-approved licences; a licence file in the unit's folder wins over the
  repository root; no licence found means refused.
- [NEEDS CLARIFICATION: what if upstream content changes under a version
  already mirrored (a moved tag, a force-push)?] Recommended: refuse and
  report; the mirror records the full commit SHA it published from.
  `ocx-mirror` has no recorded decision here.
- [NEEDS CLARIFICATION: how does the mirror file express one repository
  yielding many units and bundles?] Recommended: one entry per upstream
  repository with a list of unit paths and optional bundle groupings; this
  departs from `ocx-mirror`'s one spec per package.
- [NEEDS CLARIFICATION: how are plugin trees published, given that grim on
  `main` (commit `a174a953`) refuses a skill whose root holds
  `.claude-plugin`?] Recommended: the mirror unpacks a plugin into its
  member artifacts plus one bundle, and never publishes the plugin tree
  itself.
- [NEEDS CLARIFICATION: how is the curated list run and kept fresh?]
  Recommended: a scheduled CI job, as `ocx-mirror` generates; a re-run with
  nothing new publishes nothing, which `grim publish` already guarantees by
  skipping an existing tag.
- [NEEDS CLARIFICATION: where does the dialect translation live?]
  Recommended: in grim, next to the vendor field registry it inverts, so
  there is one source of truth and a person converting their own Claude
  skill by hand gets the same result; the mirror calls it. The cost is one
  more additive entry in grim's CLI contract. The alternative, a copy of the
  registry in the mirror repository, drifts.
- [NEEDS CLARIFICATION: what happens to a body that references one
  harness's tools or plugin-root variable?] Recommended: publish and report
  a warning per occurrence; refusing would drop most Claude-authored skills.

## Illustration

Not a decided format; shown to the owner on 2026-11-23 to make the shape
concrete. A mirror file in a small curated-list repository:

```toml
[mirror]
registry = "ghcr.io/<operator>/mirror"
licenses = ["MIT", "Apache-2.0", "BSD-3-Clause"]

[[upstream]]                      # plain monorepo, no tags
repo    = "https://github.com/anthropics/skills"
ref     = "main"
dialect = "agentskills"
version = "snapshot"
skills  = ["skills/*"]

[[upstream]]                      # semver tags, bundled, licence declared
repo    = "https://github.com/obra/superpowers"
dialect = "claude"
version = { tags = 'v(\d+\.\d+\.\d+)' }
license = "MIT"                   # stated by the operator when not derivable
skills  = ["skills/*"]
[[upstream.bundle]]
name    = "superpowers"
members = ["skills/*"]

[[upstream]]                      # marketplace file as the list
repo        = "https://github.com/wshobson/agents"
dialect     = "claude"
version     = "snapshot"
marketplace = ".claude-plugin/marketplace.json"
plugins     = ["python-development", "backend-*"]
```

A scheduled CI job runs a check step (resolve, licence and kind gate, print
the plan, publish nothing) on pull requests and a sync step on the default
branch. Per unit a run reports one of: published, unchanged, refused (with
the reason: licence, kind, unsupported build step).

## Verification

Files and interfaces the work touches. In grimoire: subcommand dispatch
under `src/command/` and its index `.claude/rules/subsystem-cli-commands.md`;
the artifact annotation set under `src/oci/`;
`docs/src/content/docs/stability.md` (dispatch and any new annotation join
the contract, additive only); `docs/src/content/docs/publishing.md`;
`.agents/adr/adr_artifact_trust_model.md` and
`.claude/rules/arch-threat-model.md` (a statement on mirrored content and on
`PATH` dispatch); `.claude/rules/product-context.md` (the new sibling
repository). Outside grimoire: the new mirror repository itself, which calls
`grim publish` and reads `publish.toml` conventions unchanged.
`docs/src/content/docs/package-index.md` is expected to stay unchanged.

How the eventual work is checked:

- The ADR is reviewed against `docs/src/content/docs/stability.md`: every
  grim-side change is additive, and existing commands, exit codes and JSON
  output are byte-identical for inputs that worked before.
- Dispatch: a built-in subcommand name always wins; an unknown name with a
  matching binary dispatches; an unknown name with none keeps today's error
  and exit code.
- Mirror, against a real registry: a single-shape monorepo yields one
  artifact per named unit plus the declared bundle; a second run publishes
  nothing; a hook, MCP server or settings member is skipped and reported; a
  unit with neither a detected nor a declared allowed licence is refused,
  and a declared one is recorded as declared; a Claude-dialect skill
  publishes with its Claude-only fields under `metadata.claude.*` and passes
  grim's own validation; a template-only upstream is
  refused as unsupported.
- Versions: a tagged upstream publishes under its tag's version, an untagged
  one under a snapshot version, and `grim publish` accepts both.
- A mirrored skill installed with `grim install` renders to each client
  exactly as a natively published one does.
- `task verify` passes in grimoire; the mirror repository carries its own
  gate.
