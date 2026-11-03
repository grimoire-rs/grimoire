# ADR: Declared dependencies between artifacts, resolved recursively through the bundle machinery

## Metadata

**Status:** Accepted (2026-11-02, goal loop `.agents/goals/adr-artifact-dependencies.md`; both open questions closed on their recommendations)
**Date:** 2026-11-01 (revision 2, same day: review round plus owner decisions D1-D4)
**Deciders:** Michael Herwig (owner) + Claude (architect)
**Beads Issue:** N/A
**Related PRD:** N/A (pre-plan discussion: `.agents/discussions/artifact-dependencies.md`)
**Blast radius:** external contract — OCI wire layout, `grimoire.lock` schema, `--format json` report shapes, `com.grimoire.*` annotations, exit codes and `ErrorReason` slugs, CLI surface, published JSON schemas
**Reversibility:** mixed, labelled per decision below. The two-layer wire layout (Decision 1) and the lock field shapes (Decision 4) are one-way doors: published artifacts and committed locks outlive any release. The diamond merge rule (D1), the caps, and the cycle refusal are two-way doors in the widening direction only.
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md` (no new dependency; Tokio `JoinSet` + semaphore, serde, existing OCI client and `BlobStore`)
**Domain Tags:** data | api | integration | security
**Supersedes:** the "no recursion in v1" nested-bundle rejection (`src/oci/bundle.rs:46`, `src/resolve/resolver.rs:500-505`; origin commit `49944cd9`)
**Extends:** `.agents/adr/adr_effective_set_mutations.md` (effective set becomes closure-aware), `.agents/adr/adr_hook_workspace_consent.md` (consent seam and transport rung applied along edges; no trust decision reopened)
**Superseded By:** N/A

## Context

Today a bundle is the only transitive mechanism. Its expansion is one flat
pass: `build_work` → `expand_bundles` → `merge_bundle_members`
(`src/resolve/resolver.rs:303-550`). A bundle member that is itself a bundle
is rejected, and nothing records why beyond "no recursion in v1" plus the
CWE-770 caps added in `49944cd9`
(`.agents/research/research_artifact_dependencies_history.md`).

Users want a skill to bring what it needs. The dominant case is a skill
reading a sibling skill (`gws-drive` reads `../gws-shared/SKILL.md`, which a
selective install leaves dangling). The next is a skill that needs an MCP
server (`.agents/research/research_artifact_dependencies_demand.md`). Almost
every request is presence-only, with no version ranges.

The owner wants three things:

- Any artifact kind may declare what it requires.
- grim resolves those requirements transitively at `add`, `lock` and
  `install`.
- grim can say **why** every artifact is installed, as a full chain from a
  root declaration, in text and JSON. The TUI and `grimoire-vscode` render it
  as a "required by" view.

Today `grimoire-vscode` approximates one hop of the why view by
regex-parsing the prose `StatusItem.source` (`"bundle: a, b"`), per
`.agents/research/research_artifact_dependencies_ocx_why.md`.

The surface is frozen for 1.0 (`AGENTS.md` Principle 9,
`docs/src/content/docs/stability.md`). Every new wire, lock, report and
annotation shape must be additive. A lock with no dependency edge must stay
byte-identical, modulo the `[metadata] generated_by` line every release
already rewrites.

### Premise corrections

The pre-plan discussion's `## Decisions` are working positions. Four do not
survive the code and research.

1. **"Older grim degrades silently" is false for install.** grim ≤ 0.15.1
   resolves a two-layer manifest without complaint, because `resolve_one`
   pins the digest only (`src/resolve/resolver.rs:869-895`). Every
   materializing path then calls `OciManifest::single_layer()`
   (`src/oci/manifest.rs:79-85`) and refuses:
   - `install` exits 65 with
     `expected a single-layer artifact, manifest has 2 layers`
     (`src/install/installer.rs:2367-2375`);
   - `fetch` fails the same way (`src/fetch.rs:349-354`).

   The refusal is loud but carries no reason slug and no "update grim" text
   (`.agents/research/research_artifact_dependencies_claim_diff.md`). The
   silent part is narrower: `grim lock`, `grim update` and
   `grim add --no-install` on 0.15.1 write a lock that is missing the
   dependencies. That residual is in shipped binaries and cannot be repaired.
2. **The raw-tar fallback in `src/oci/layer.rs:92-105` is never reached** for
   a two-layer manifest, because the single-layer check runs first.
3. **A `com.grimoire.requires-features` annotation cannot gate shipped
   clients.** OCI consumers MUST NOT error on unknown annotations, and grim
   0.15.1 ignores them
   (`.agents/research/research_artifact_dependencies_forward_compat.md` §3).
   The annotation is still the home for the *future* gate (Decision 2).
4. **The research's config media type or `artifactType` bump is foreclosed.**
   `.agents/adr/adr_oci_empty_config_compat.md` moved every artifact to the
   OCI empty config and dropped `artifactType`, because GitLab rejects both
   (`400 MANIFEST_INVALID`). The same ADR records that GitLab accepts the two
   custom **layer** media types grim uses, including
   `application/vnd.grimoire.bundle.v1+json`.

The decision also challenges two dossier positions:

- **"Reuse the 512-member cap as a whole-graph cap" would break existing
  projects.** The cap is per bundle and the number of declared bundles is
  unbounded. A project declaring two 300-member bundles is valid today, so
  the new caps count only shapes the old model could not produce
  (Decision 3).
- **The dual-shape provenance template of `2673bb2d` is not reused
  literally.** Its single-pair branch only keeps pre-existing locks
  byte-identical. A brand-new field has no legacy form to preserve
  (Decision 4).

## Decision Drivers

- **Fail closed on shipped clients.** No grim may silently install an
  incomplete set where that is avoidable. Silent degradation is the dominant
  failure across ecosystems: npm `engines`, pre-1.21 Go, Cargo unknown keys
  (forward-compat research, "Decision-relevant observations" 2).
- **Integrity by construction.** The dependency list must be covered by the
  pinned manifest digest, so the lock's digest pin answers T2 for the edges
  as for the content (`.claude/rules/arch-threat-model.md` I2). No cache may
  be trusted that is not content-addressed.
- **Registry compatibility.** No new media type in a manifest.
- **Additive-only**, with each decision's reversibility named.
- **Reuse the hardened path.** The members-layer parse, its 512 KiB and
  512-member caps, `SkillName` validation (CWE-22) and `MemberRef` relative
  refs (mirror survival, issue #31) already survived an untrusted-registry
  review.
- **Offline stays offline.** Mutations stay network-free
  (`adr_effective_set_mutations.md`), and a project that locks offline today
  still locks offline after upgrading.
- **Do not reopen trust.** The declaration gesture grants trust that
  transitive pulls inherit (`adr_artifact_trust_model.md` decision 1). Hook
  consent records the resolved set and re-gates on drift
  (`adr_hook_workspace_consent.md` decision 4).
- **Bounded work from wire-controlled data:** `.claude/rules/rust-quality.md`
  ASYNC-04, ASYNC-05 and ASYNC-09; CWE-770; CWE-400.

## Industry Context & Research

**Research artifacts:** `.agents/research/research_artifact_dependencies_{claim_diff,codebase,forward_compat,history,prior_art,vendor,demand,ocx_why}.md`

**Trending approaches.** AI-config formats split into two camps:

- **No dependency field:** the Agent Skills spec, MCP `server.json`,
  Cursor, Codex and Copilot.
- **Two graphs:**
  - Claude Code plugins take `dependencies` with optional semver ranges
    over git tags. They intersect constraints, refuse cross-marketplace
    pulls unless allowlisted, keep no lockfile, and treat a
    dependencies-only plugin as a bundle.
  - APM resolves the full graph into `apm.lock.yaml` and gates transitive
    MCP servers.

Sources: `research_artifact_dependencies_prior_art.md`,
`research_artifact_dependencies_vendor.md`.

Presence-only flat models are common and well regarded:

- VS Code `extensionDependencies` takes IDs only, and closed a
  minimum-version request (vscode#202692).
- Homebrew has no ranges.
- Go MVS calls itself "understandable, predictable, boring".

**Removal.** Nix computes the closure from GC roots, so orphan removal is
exact. Homebrew `autoremove` and `claude plugin prune` remove only
auto-installed packages. APM silently keeps orphaned transitives (#3401) and
silently drops a transitive with exit 0 (#2815). The closure model below is
built against that failure.

**Forward compatibility.** Git `extensions.*` and JOSE `crit` are the
"producer lists what a reader must understand" pattern. Git notes that a
format version bump with no extension "provides no benefit", which is why
this feature does not list itself (Decision 2). Cargo shipped its lock error
("perhaps Cargo needs to be updated?") in the reader before a writer needed
it.

**Why views.** ocx returns every path root-first (`paths: PackageRef[][]`)
and recomputes paths from stored edges instead of persisting "installed
because". It renders a tree with a `repeated` flag. `npm explain`, `pnpm why`
and `yarn why` are the precedent for a dedicated subcommand.

**Key insight.** "Every artifact is a bundle" is literally implementable. A
bundle is an artifact whose only layer is the members document. A dependent
artifact is an artifact whose content layer is followed by that same
document. One media type, one parser, one cap, one resolver — and shipped
binaries already refuse to materialize it.

## Considered Options

### Carrier

| # | Option | Description |
|---|---|---|
| A | **Second members layer** | The artifact's own manifest carries `[content layer, members layer]`. The members layer is the existing `application/vnd.grimoire.bundle.v1+json` document |
| B | Referrer or companion manifest | An OCI 1.1 referrer (`subject` = the artifact) or a companion tag holding the members document |
| C | Manifest annotation | `com.grimoire.requires` holding the member list as a JSON string |
| D | Config blob field | Requirements in a non-empty config blob |
| E | Kind or `artifactType` bump | A new `artifactType` or a new `com.grimoire.kind` value for dependent artifacts |

Scores run from 1 (worst) to 5 (best). The weighted total is the sum of
weight × score, out of a maximum of 120.

| Criterion | Weight | A | B | C | D | E |
|---|---|---|---|---|---|---|
| Shipped grim fails closed (no silent incomplete install) | 5 | 4 | 1 | 1 | 1 | 3 |
| Edges covered by a pinned digest (T2) | 5 | 5 | 3 | 5 | 5 | 5 |
| Registry compatibility (GitLab allowlist, ECR, `registry:2`) | 4 | 5 | 2 | 4 | 1 | 1 |
| Reuses hardened fetch, caps and validation | 3 | 5 | 3 | 3 | 2 | 2 |
| Mirror survival (`skopeo`/`oras` copy, relative refs) | 3 | 5 | 2 | 5 | 5 | 5 |
| Resolve cost per node | 2 | 3 | 2 | 4 | 3 | 4 |
| No-dependency artifacts byte-identical | 2 | 4 | 5 | 4 | 3 | 2 |
| **Weighted total** (max 120) | | **109** | **57** | **86** | **67** | **77** |

B scores 3 on digest coverage, not 1. A referrer manifest is itself
digest-addressed and can be pinned by a consumer. The subject's digest does
not cover it, though, so a new referrer can appear without the artifact
changing.

**Option A risks:**

- Install on 0.15.1 refuses with an unhelpful message, and
  `lock`/`add --no-install` on 0.15.1 are silent. Both are shipped and
  unfixable.
- Resolve gains a manifest fetch per node.
- Layer order is a grim convention, not an OCI guarantee. The OCI image
  spec defines layer order only for filesystem application. grim therefore
  tells the two layers apart by media type in Rule F4, accepts either
  order, and refuses every other arrangement. The writer always emits
  content first.

**Option A reversibility:** one-way, because published two-layer artifacts
are permanent. The layout can still be extended (a third layer) under the
refusal rule of Decision 2.

**Rejected options:**

- **B:** shipped grim ignores it, so every old install is silently
  incomplete. A new referrer can appear without the subject changing, and
  support for the Referrers API and for mirroring referrers is uneven.
- **C:** shipped grim ignores it (silent). Registries limit annotation
  size, which `src/oci/bundle.rs` already cited when bundles chose a layer.
  It would also be a second encoding of the members document.
- **D:** the config must be the OCI empty `{}`
  (`adr_oci_empty_config_compat.md`). A non-empty config needs a custom
  config media type, which GitLab rejects.
- **E:** disqualified on Principle 9. GitLab rejects a custom
  `artifactType`, and a new kind value changes what `com.grimoire.kind`
  means for every index and catalog consumer.

### Forward-compatibility signal for future features

| # | Option | Verdict |
|---|---|---|
| F1 | **`com.grimoire.requires-features` manifest annotation**, a crit-style list a reader must understand | **Chosen.** Read on every interpreting path, covered by the manifest digest, accepted by every registry grim publishes to |
| F2 | A `features` field inside the members document | Rejected. A future feature may have no members layer |
| F3 | Config blob | Foreclosed (Option D) |
| F4 | Layer-shape refusal only | Kept as a second rule. It cannot express a feature that does not change the layout |

### Lock representation of edges

| # | Option | Verdict |
|---|---|---|
| L1 | **Forward edges on the parent, as published** (`[[<kind>.requires]]`, and the existing `[[bundle.member]]` list for bundles). `required_by` and paths recomputed | **Chosen.** One truth per edge, verified at install against the pinned manifest |
| L2 | Child-side `required_by` provenance in the dual shape of `2673bb2d` | Rejected. It duplicates the parent's pin and still needs L1 for an offline closure |
| L3 | `lock_version = 2` plus an `[[edge]]` section | Rejected. A second version mechanism the project has never used; old grim already refuses L1 (exit 78) |

### Why surface

| # | Option | Verdict |
|---|---|---|
| W1 | **`grim why <kind> <name>` subcommand, plus `direct` and `required_by` on every `grim status` item** | **Chosen.** One-hop data everywhere in linear size, full chains on demand |
| W2 | `grim status --why <name>` | Rejected. A flag that swaps a frozen report's shape |
| W3 | All paths embedded in every status item | Rejected. Path count is exponential in a layered diamond (CWE-400) |

## Decision Outcome

**Chosen:**

- carrier A;
- signal F1 + F4;
- lock L1;
- why surface W1;
- the resolution, lock, removal, consent, export and kind rules below.

Everything ships in **one release** (owner decision D3).

### Decision 1: the carrier (one-way door)

A **dependent artifact** is a non-bundle artifact whose manifest has exactly
two layers, in this order:

1. `layers[0]` is the kind's existing content layer, unchanged: tar,
   `tar+gzip`, or the MCP descriptor JSON.
2. `layers[1]` is the **requires layer**:
   - media type `application/vnd.grimoire.bundle.v1+json`;
   - bytes are the canonical `BundleManifest` document
     (`{"members": [...]}`, sorted by `(kind, name)`, pretty JSON with a
     trailing newline);
   - no descriptor annotations, never compressed.

A bundle stays a single-layer manifest whose one layer is the members
document. A non-dependent artifact stays single-layer. Both are
byte-identical to today.

The `__grimoire` description companion (`src/oci/description.rs:214`) is not
an artifact and keeps calling `single_layer()`.

### Decision 2: forward compatibility (one-way door for the annotation key; two-way for `KNOWN_FEATURES` membership)

**Rule F4 (layout).** `OciManifest::layout(kind)` accepts exactly three
shapes:

- one content layer;
- one content layer and one requires layer, in either order, for a
  non-bundle kind;
- one members layer, for a bundle.

The layers are classified by media type: a requires layer is one whose
type is `application/vnd.grimoire.bundle.v1+json`, and a content layer is
any other type. Any other shape refuses with exit **65**, reason
**`unsupported-artifact-feature`** (new, appended). That covers three
layers, a bundle with two layers, two content or two requires layers, and
a non-bundle whose only layer is a requires layer.

`layout` replaces `single_layer()` on every materializing path:

- `src/install/installer.rs:2367`
- `src/fetch.rs:349`
- `src/install/expected_outputs.rs:108`
- `src/resolve/resolver.rs:780`
- `src/oci/layer.rs:120` (`compression_of`): compression is read from the
  content layer only. A requires layer is never compressed.

**Rule F1 (declared features).** The manifest annotation
`com.grimoire.requires-features` lists feature names the reader must
understand.

- **Value format:** comma-separated names matching
  `[a-z0-9]+(-[a-z0-9]+)*`. A reader trims whitespace and drops empty
  segments. A writer emits names sorted and deduplicated.
- **Caps:** at most **1024 bytes** and **32 names**.
- **Refusal:** any of the following refuses with exit 65,
  `unsupported-artifact-feature`:
  - a name outside `KNOWN_FEATURES`;
  - a segment failing the grammar;
  - a value over either cap.
- **Absent:** an absent key, or one with no non-empty segment, means "this
  artifact requires nothing beyond the base format". This keeps
  `stability.md:64-66` ("absent means this artifact does not say"), which
  gains one sentence for the present case.

`KNOWN_FEATURES = ["dependencies"]`. The writer never emits the annotation
for dependencies: shipped grim ignores it, and every reader release knows
the name. The name is reserved so a third-party producer that lists it is
not refused.

**Where the gate fires:**

- resolve (every node's manifest);
- install (`fetch_verified_layer`);
- `grim fetch` and `grim export plugin`;
- the MCP tools `grim_fetch` and `grim_render`.

It does **not** fire on `describe`, `search`, the TUI catalog or index
browse. These read metadata and must keep showing an artifact this grim
cannot install.

**Messages.** Both are one line. Every registry-sourced string in them is
passed through the shared `registry_text` escaper (Decision 8).

- `{ref}: artifact requires features this grim does not support ({names}); update grim (this is grim {version})`
- `{ref}: manifest layer layout is not supported by this grim ({n} layers: {media types}); update grim (this is grim {version})`

**Why exit 65.** It is the closest precedent:

- the `+zstd` refusal is 65 with `unsupported-layer-media-type`
  (`adr_compressed_artifact_layers.md`);
- the MCP descriptor's newer-fields refusal is 65.

Exit 78 stays reserved for the user's own config and lock.

**Lock-side hint (staleness hint only, never a trust signal).** When
`grimoire.lock` fails to parse, grim re-reads `[metadata] generated_by`
leniently. The exit stays 78.

- The value is validated against `grim <semver>`, with `registry_text`
  applied before display.
- It is compared by semver precedence with the `semver` crate, already a
  direct dependency (`Cargo.toml:49`).
- If it names a newer grim, the error appends
  `; this lock was written by grim {x.y.z}; update grim`.

No resolve or install decision ever reads `generated_by`.

### Decision 3: resolution

**Graph model.**

- A node is `(kind, binding name)`.
- **Roots** are every direct declaration plus every declared bundle.
- A node's **outgoing edges** are its members document: a bundle's single
  layer, or a dependent artifact's requires layer. For a path-sourced node,
  they are its source declaration.
- Edges may target any kind, **including `bundle`**. Nested bundles are
  allowed, replacing the nested-bundle rejection.

**Expansion.** Breadth-first by level and iterative, with a settled set
keyed by `(kind, name)`. Each frontier node takes one of two paths.

**Root or newly requested node:**

1. Resolve its tag to a digest. This is the only tag resolve per node.
2. Fetch the manifest by that pinned digest.
3. Apply F1/F4.
4. If a requires or members layer is present, read it **by descriptor from
   that already-pinned manifest**, never through a second tag resolve.

**Fetch split.** Today's `fetch_bundle_layer` resolves the tag and then
fetches the layer. It splits into:

- `fetch_members_blob(repo, descriptor)`, which applies the 512 KiB
  descriptor-and-post-transfer cap and the 512-member cap;
- a thin tag-resolving wrapper kept for declared bundles.

`fetch_bundle_members` is re-plumbed onto the split, not left unchanged.

**Validation.** Every member name passes `SkillName::parse` (CWE-22).

**Relative ids.** These are resolved through `MemberRef::resolve` against
the **parent's current pinned registry/repository**. `./gws-shared:1` from
`ns/skills/gws-drive` names `ns/skills/gws-shared:1`, and after the consumer
switches to a mirror, the same edge names the mirror.

**Empty requires layer.** A requires layer with zero members is invalid
(65).

**Content-addressed cache (correct by construction).** A node's edges are a
pure function of its manifest digest. The manifest is stored by digest in
the existing content-addressed `BlobStore` under `$GRIM_HOME`
(`src/oci/access/cached_access.rs:93-125`, write-through today). The
requires blob is stored by its own descriptor digest.

- The reader makes `CachedAccess::fetch_manifest` read the store first, in
  both modes, as `fetch_blob` already does. `put` verifies the digest, so a
  hit is authoritative.
- A steady-state re-lock is then one tag resolve per artifact plus local
  store reads.
- **The lock's recorded `requires` is never a *trusted* cache.** It is used
  only on an offline store miss, unverified, and refused at install by the
  set-compare (Decision 4). Online, it never influences resolve. Locks
  travel with repositories and are attacker-editable (T3).

**Offline locking.** A project that locks offline today must still lock
offline after upgrading.

- Offline, the tag cache gives the digest, and the store gives the manifest
  and requires blob when present.
- On a store miss for a node whose previous lock entry pins the same
  digest, the previous entry's `requires` is carried forward **unverified**.
  It is empty for a pre-R lock, so the lock stays byte-identical modulo
  `generated_by`.
- `install`'s set-compare (Decision 4) is the guard: it re-reads the pinned
  manifest and refuses a mismatch before anything materializes.
- On a store miss for a node with no previous entry, edges are treated as
  empty unverified, under the same install guard.
- Online, a store miss always fetches.

**Bounded concurrency (ASYNC-05).**

- Each level resolves through one `JoinSet` behind a semaphore of
  `RESOLVE_CONCURRENCY = 32` permits.
- A level's results are fully collected, never more than the frontier, which
  is bounded by the remaining node budget. They are then sorted by
  `(kind, name)` (DATA-DET).
- The bound replaces today's unbounded direct fan-out
  (`src/resolve/resolver.rs:836-840`), an existing ASYNC-05 violation. It
  lands as its own `fix(resolve):` commit ahead of the feature.

**Caps.** Every breach refuses with exit **65**. Each cap is enforced
**before** the node or bytes are admitted, never after.

| Cap | Value | Counts |
|---|---|---|
| `MAX_BUNDLE_MEMBERS` (existing) | 512 per node | members of one members or requires layer |
| `BUNDLE_LAYER_SIZE_LIMIT` (existing) | 512 KiB per layer | one members or requires layer |
| `MAX_TRANSITIVE_NODES` | 2048 | nodes reached through a requires edge, or through a bundle that was itself reached through an edge. Depth-1 members of a declared bundle (today's only shape) are exempt |
| `MAX_REQUIRES_BYTES` | 32 MiB | sum of all requires and nested-bundle layer bytes fetched in one resolve. Declared-bundle layers are exempt |
| `MAX_DEPTH` | 32 levels | BFS levels below the roots. This bounds total wall time at `MAX_DEPTH × per_artifact_timeout × (1 + max_retries)` per serial chain |

Before a level's edges are inserted, grim computes how many new keys the
level would add. If that exceeds the remaining node budget, it refuses with
`GraphTooLarge { limit, via }`, where `via` names the parent whose edges
crossed the line.

**Diamond policy (owner decision D1, a two-way door: it widens what
succeeds).** Per `(kind, name)`:

- **Direct wins.** A direct declaration wins over every edge, and its own
  edges are expanded. When the direct declaration's registry/repository
  differs from a shadowed edge's, resolve emits a warning naming both. That
  is the "did you mean to switch publishers" case.
- **Equal digests merge.** Edges agree when their requested identifiers
  **resolve to the same manifest digest**. `B:1` and `B:1.2` pointing at one
  digest merge, and every parent is recorded.
- **Different digests conflict.** A conflicted node is never expanded.
  Resolution collects every conflict and then fails once with exit **78**
  (`BundleConflict`).

The conflict message names each contributor as `{id} from bundle {repo}:{tag}`
or `{id} required by {kind}/{name}`, and ends with
`declare <kind> <name> directly in grimoire.toml to choose one`.

**The flap.** Agreement is evaluated at resolve time. Two tags that agree
today can diverge when one moves, so a later `grim update` can turn a clean
lock into a 78. This is documented on `concepts.md#bundle-conflicts`, and the
fix is the direct declaration the message names.

**Reversibility of the merge rule.** Narrowing it back to "same requested
identifier" would refuse locks that succeed today, a break. Widening it
further, for example to repository-level identity, stays open.

**Determinism and error precedence.**

1. Within a level, every node result is collected. If any failed, the error
   of the smallest `(kind, name)` failing node is reported. Network
   failures are never raced.
2. Cap breaches (`GraphTooLarge`, bytes, depth) are reported at the level
   where they occur.
3. Conflicts (78) are reported after expansion completes.
4. Cycles (65) are checked only on a conflict-free graph.

**Cycles (a two-way door: refusing now, permitting later is additive).**
After expansion, the edge set is checked for a cycle, including a self-edge
and edges into a direct node. The reported cycle is chosen in two steps:

1. Take the first back edge found by an iterative DFS that starts from roots
   in `(kind, name)` order and visits children in `(kind, name)` order.
2. Rotate that cycle so its smallest key comes first.

The refusal is exit **65** (`DependencyCycle`), with this message:

```text
dependency cycle: skill/a -> skill/b -> skill/a; the edge skill/b -> skill/a is published by {repo of b}; contact that publisher
```

`grim build` and `grim release` refuse a self-reference offline.

**Floating by default; `--pin` freezes.**

- `grim release --pin` freezes the requires layer of any kind, as it
  freezes bundle members (`pin_members`, `src/command/release.rs:563`).
  Relative ids become absolute and digest-pinned.
- `publish.toml` accepts `pin = true` on any entry; it is a no-op without
  requires. This widens an input `src/command/publish.rs:1883` used to
  reject.
- `grim release` checks that every requires ref parses and **syntactically**
  resolves against the release target, exactly as bundle release does for
  members (`src/command/release.rs:392-407`). It does not check that the
  target exists in a registry; only `--pin` touches the network for members.

**Path-sourced artifacts.**

- A path-declared skill, rule, agent, mcp or hook contributes edges read
  from its source declaration (Decision 6).
- Every edge must be absolute. A relative edge refuses with 65, as for a
  local bundle (`validate_local_members`).
- A registry artifact can never require a path.

**Cross-registry edges are allowed**, as for bundle members:

- Each node is fetched with credentials scoped to its own host. A
  credential is never forwarded.
- Plain HTTP is reachable only through the existing insecure set:
  `GRIM_INSECURE_REGISTRIES`, `[[registries]] insecure = true`, and the
  always-on loopback forms. No new transport control is added (owner
  decision D4: the existing protection rules apply uniformly to direct and
  transitive artifacts).
- A security-review suggestion to refuse loopback edges from non-loopback
  parents is **declined by owner decision**. The existing config is the
  control.

**Partial resolve** (`grim update <names>`, `grim add`):

- Selected nodes, and any node absent from the previous lock, resolve
  fresh.
- An unselected child keeps its previous pin **only when its winning
  requested identifier is unchanged**. That identifier is the direct entry's
  ref, or the agreeing edge id resolved against its parent's current
  repository. Otherwise it resolves fresh, and the closure is recomputed from
  the new pins.
- Nodes no longer reachable are dropped.
- Declared bundles are re-fetched as today.

### Decision 4: lock and removal

**Lock shape (one-way door).** Both new fields are omitted when unused, so a
dependency-free lock is byte-identical modulo `generated_by`, and
`lock_version` stays `1`. The array-of-tables form follows the
`[[skill.bundles]]` precedent (`src/lock/grimoire_lock.rs:1109`).

```toml
[[skill]]
name = "gws-drive"
pinned = "ghcr.io/acme/skills/gws-drive@sha256:…"

[[skill.requires]]
kind = "mcp"
name = "gdrive"
id = "ghcr.io/acme/mcp/gdrive:2"

[[skill.requires]]
kind = "skill"
name = "gws-shared"
id = "./gws-shared:1"

[[skill]]
name = "gws-shared"
pinned = "ghcr.io/acme/skills/gws-shared@sha256:…"

[[bundle]]
name = "google-extras"
repo = "ghcr.io/acme/bundles/google-extras"
tag = "1"
pinned = "ghcr.io/acme/bundles/google-extras@sha256:…"
transitive = true

[[bundle.member]]
kind = "skill"
name = "gws-sheets"
id = "ghcr.io/acme/skills/gws-sheets:1"
```

**The `[[<kind>.requires]]` field.**

- It sits on `[[skill]]`, `[[rule]]`, `[[agent]]`, `[[mcp]]` or `[[hook]]`.
- Each element is the `BundleMember` type (`deny_unknown_fields`), sorted by
  `(kind, name)`.
- **`id` is stored exactly as published.** A relative id stays relative and
  is re-resolved against the parent's current pinned repository on every
  read, so a mirror switch needs no lock rewrite.
- Emitted only when non-empty, whatever the entry's own provenance.
- A byte-exact serialization test pins the rendering, including
  inline-vs-table form and key order.

**The `transitive = true` field (owner decision D2: kept).**

- It marks a `[[bundle]]` snapshot whose binding no `[bundles]` table
  declares, meaning a bundle reached through an edge. It is emitted only
  when true.
- The fact is derivable from `grimoire.toml`, so the field is redundant. It
  exists so that grim 0.15.1 refuses (exit 78) every lock that uses
  recursion.
- Without it, a lock whose only new structure is a nested bundle parses
  cleanly on 0.15.1. Old `grim remove` would then compute its effective set
  from the snapshots, find a transitive bundle's members attributed to no
  declared binding, and delete or restamp files the user still needs. That
  is data loss.
- Old `install` refusing is the accepted price.
- A reader that finds `transitive = true` on a declared binding, or a
  declared binding without its snapshot, treats the cache as stale.

**Other lock rules.**

- Members reached through a nested bundle keep today's bundle provenance
  fields, naming the nested bundle.
- Dependency provenance is never stored on the child. `required_by`,
  `direct` and every path are recomputed (L1).
- grim ≤ 0.15.1 reading a lock with either field exits 78
  (`deny_unknown_fields`). A lock that drops its last edge and re-locks
  downgrades cleanly.
- `docs/public/schemas/grimoire-lock.schema.json` gains both optional
  properties: an additive schema change, not a byte-identical one.

**Install set-compare (the edge integrity check).** `install` already
fetches each pinned manifest. For every node it compares two sets:

- the lock entry's `requires`, as a set of `(kind, name, id)`;
- the members of the requires layer read by descriptor from that pinned
  manifest.

Any difference refuses with exit **65**, reason **`stale-lock`**, before
anything materializes:

```text
{kind} '{name}': the lock's dependencies do not match the pinned artifact; run grim lock
```

This one check catches:

- locks written by 0.15.1 (no `requires`, layer present);
- hand-edited or attacker-edited locks;
- unverified offline carry-forward.

The pinned manifest is digest-verified, so the comparison is against
publisher bytes, not lock bytes.

**Path-source freshness.** A path entry's freshness check re-reads the
source and compares its edges with the recorded `requires`. A mismatch is
65 `stale-lock`. For MCP the declaration is not packed into the content
layer (Decision 6), so this comparison is load-bearing there.

**Closure-aware effective set (offline).**

1. `effective_set(set: &DesiredSet, lock: &GrimoireLock)` replaces
   `effective_set(set, cached: &[LockedBundle])`. Every caller passes the
   lock; this is an in-crate change.
2. Starting from the roots, it follows bundle snapshots and entry
   `requires`, recursively, with a visited set and the same merge policy.
3. **Direct precedence is applied first.** An edge to a directly declared
   `(kind, name)` resolves to the direct entry with no repository check.
4. Only for a non-direct target is the edge followed through that node's
   lock entry. That happens when the entry's registry/repository equals the
   edge id's, resolved against the parent's current repository (the
   `snapshot_matches` rule, generalized).
5. Digest agreement offline means the targets share the one lock entry. A
   conflicting pair cannot exist in a lock resolve accepted.
6. If any lookup fails, the closure is **incomputable** and returns `None`.

Mutations act on `E_before \ E_after`:

- `grim remove A` drops from config and lock A and every node that only A's
  subtree reached.
- `grim uninstall A` deletes the files and install records of
  `E_before \ E_after`, mirroring the TUI bundle delete
  (`bundle_uninstall_targets`).
- A node a remaining parent still reaches is protected
  (`declared_bundle_provides` generalizes to `declared_parent_provides`):
  - directly declared: uninstall degrades to remove and keeps the files;
  - dependency-only: nothing changes, and the uninstall reports
    **`kept-by-dependency`**;
  - held by a bundle: keeps `kept-by-bundle`.
- The id-mismatch rule of `adr_effective_set_mutations.md` is unchanged.

**Incomputable closure: never destructive.** When `effective_set` returns
`None`:

- `remove`/`uninstall` act on the **target only**. They drop its config
  entry, its lock entry and its own files.
- They never delete or restamp another node's files.
- The lock is written with every other entry untouched, and the closure
  becomes stale. The next `grim install` hits the existing stale guard, and
  `grim lock` recomputes and prunes.
- Plain output prints
  `kept N artifacts whose dependency graph could not be computed offline; run grim lock`.

This replaces, for dependency-bearing locks, the pre-cache fallback that
deleted members by provenance.

**Hook consent (seam narrowed; no trust decision reopened).** `grim add`
consents to exactly the hook nodes that meet all three conditions:

1. reachable from the **added target** through the target's own edges;
2. whose settled lock identifier equals the edge identifier that reached
   them (not shadowed by a direct entry at another id);
3. **not already present in `E_before`**, i.e. not settled by a pre-existing
   direct entry or reached by a pre-existing parent.

The `resolve_for_add` prohibition at `src/command/hook_consent.rs:202`
("adding `B` cannot consent to an unconsented `A` that was already sitting
in the workspace") holds along edges.

**When `E_before` is incomputable** (`effective_set` returns `None` on the
pre-add config and lock), condition 3 falls back to a conservative
exclusion. No hook node is consented if it is:

- declared in the pre-add config under any kind table; or
- an entry, or a `[[bundle.member]]`, of the previous lock.

Only hook nodes absent from both are consented. This closes a T3 path: a
cloned repository plants a direct `[hooks]` entry and also breaks the
closure, for example with a declared bundle that has no `[[bundle]]`
snapshot. Without the fallback, an "everything is new" reading of a `None`
closure would let `grim add` consent to the planted hook.

The single-entry projections in `install_added` (`src/command/add.rs:495`)
and in the TUI's add path are replaced by **reachability from the target**.
That reachability includes dependencies already locked, so they are
installed if missing, but only the nodes meeting condition 3 are consented.

**Narrowed bundle consent.** `grim add <bundle>` no longer consents a hook
member that was already present and unconsented in the workspace. Today's
projection consents every hook member of the added bundle. This narrows
shipped behavior. It is permitted because the hook kind is experimental
(`[options.experimental] hooks`), which carries no compatibility promise.
`upgrading.md` records it.

`install`, `update` and `lock` never write consent. A floating dependency
that later gains a hook drifts the set and re-gates as `consent-drifted`.

**Hook transport (owner decision D4).** A transitive hook is armed by
exactly the same rungs as a direct one; no edge-specific rule is added.
`ArmingQuery.insecure_transport` stays as today (`src/hook/trust.rs:229`):
true when the hook's own pinned host would be fetched over routable plain
HTTP. The transport of the parents that selected the hook is not consulted.
The rung order is unchanged: `--trust-hooks` (rung 3) still precedes
`insecure-transport` (rung 4), so the per-invocation flag can arm a
plain-HTTP hook, direct or transitive alike. Plain HTTP itself is reachable
only through the existing insecure set. The owner rejected both a
path-transport rule and moving the transport rung (2026-11-01): one
protection rule, applied uniformly.

**`--force` scope.** `grim add --force` overrides the untracked-clobber
guard for the **target's own destinations only**. A clobber on a transitive
destination still refuses with 65 `untracked-destination` and lists every
transitive destination. `grim install --force`, the explicit whole-set
gesture, is unchanged.

### Decision 5: the why system (ships in this release)

The owner explicitly asked for the why system, so `grim why` is not
deferred.

**`grim status` items** gain two always-present fields:

| Field | Type | Meaning |
|---|---|---|
| `direct` | `bool` | Declared in this scope's config under its kind table (`[bundles]` for a bundle row). Always computable from config |
| `required_by` | `[{kind: string, name: string}] \| null` | Immediate parents, bundles included as `kind: "bundle"`, sorted by `(kind, name)`, deduplicated. `[]` = no parent. `null` = the graph is incomputable offline (Decision 4) |

**The `source` field.**

- It keeps its exact current strings: `"direct"` when `direct`, else
  `"bundle: <repo>, …"` when any bundle contributes.
- It gains one appended literal family, `"dependency: <kind>/<name>, …"`,
  for a node reached only through requires edges.
- When the graph is incomputable, a non-bundle, non-direct row reads
  `"dependency"` with no list.
- Consumers should stop parsing `source`.

**Rows and notes.**

- Every lock entry that is not directly declared gets a row. The filter
  widens from `is_from_bundle()` (`src/command/status.rs:404-421`).
- When a direct declaration is also required **at the same digest**,
  meaning it overrides nothing, plain `status` prints one stderr note:
  `note: skill/x is declared directly and also required by skill/y at the same digest; the declaration can be removed`.
  JSON carries the same facts as `direct: true` plus a non-empty
  `required_by`.

**`grim why <kind> <name>`** is a new subcommand, additive on the frozen CLI
surface.

- It is offline: it reads the lock and config only.
- It honors `--global`, `--config` and `--format`.
- `--registry` is accepted as a global flag and has no effect, as on every
  lock-only command.
- `--help` reads:
  `Show why an artifact is installed: every chain from a declaration in grimoire.toml to it, root first. Reads the lock; never contacts a registry.`

It returns a single-object report:

```json
{
  "kind": "skill",
  "name": "gws-shared",
  "pinned": "ghcr.io/acme/skills/gws-shared@sha256:…",
  "direct": false,
  "paths": [
    [
      {"kind": "bundle", "name": "google", "pinned": "ghcr.io/acme/bundles/google@sha256:…"},
      {"kind": "skill", "name": "gws-drive", "pinned": "ghcr.io/acme/skills/gws-drive@sha256:…"},
      {"kind": "skill", "name": "gws-shared", "pinned": "ghcr.io/acme/skills/gws-shared@sha256:…"}
    ]
  ],
  "path_count": 1,
  "truncated": false,
  "overrides": []
}
```

**Paths.**

- `paths` holds simple paths from a root to the target, root first and
  target last. A direct target includes the one-node path `[target]`.
- Paths are sorted lexicographically by `(kind, name)` sequence and
  deduplicated, with at most `MAX_WHY_PATHS = 256`.
- `path_count` is the exact number of root-to-target paths. It is computed
  by a dynamic-programming count over the acyclic graph in O(V + E), and
  saturates at 9007199254740991 (2^53 - 1, the JSON safe-integer limit).
- `truncated` is `path_count > paths.length`.

**`overrides`** lists the edges into the target that a direct declaration
shadowed at a different digest, as `[{kind, name, id}]`:

- `kind`/`name` is the parent that requested the target;
- `id` is the identifier it asked for;
- sorted, `[]` when none.

**Node pins.** A node's `pinned` is its lock pin for an artifact, the
snapshot digest for a registry bundle, and `null` for a path source or local
bundle.

**Safety on hostile locks.** The offline graph is cycle-checked before
enumeration; a cycle in a lock is 65 `stale-lock`. Enumeration is an
iterative DFS with a **step budget of 100 000 edge visits**. Exhausting the
budget sets `truncated: true` and keeps the paths found so far, sorted.
`path_count` stays exact.

**Plain output.**

- One table with one column, `Path`, showing the first **10** paths, for
  example `bundle/google -> skill/gws-drive -> skill/gws-shared`.
- Then a line `… and {path_count - 10} more; grim why --format json lists up to 256`.
- A non-empty `overrides` adds the line
  `overrides: skill/gws-drive asked for {id}`.
- Direct-and-required-at-same-digest prints the note from `status`.

**Exit codes.**

| Condition | Exit |
|---|---|
| Success | 0 |
| Target not in the effective set | 79 |
| No lock | 79, `no-lock` |
| No project | 79, `no-config` |
| Closure incomputable, or cycle in the lock | 65, `stale-lock` |
| Unknown kind | 64 |

**`grim add`**'s report gains `pulled_in`, always present:

- `[{kind, name, pinned}]` = `E_after \ E_before`, minus the target, sorted;
  `[]` when none.
- When `E_before` is incomputable, it lists only nodes of `E_after` absent
  from both the pre-add config and the previous lock, minus the target. This
  is the same conservative set the consent fallback uses.
- Plain output adds one stderr line:
  `pulled in N dependencies: skill/gws-shared, mcp/gdrive`.

`install` and `update` do not gain `pulled_in`: their items already list
every materialized node.

**`grim describe`** and MCP `grim_describe` share one report, which gains
`requires`, always present:

- `[{kind, name, id}]` from the requires layer, with ids as published;
- `[]` when there is no requires layer;
- `null` for a foreign manifest.

It costs one bounded blob GET, only when the layer exists. **Failure
semantics:** a requires layer that is oversize, malformed or has an invalid
member fails `describe` with that error (65). An unreachable blob fails as
the manifest would (69, or 81 offline). `describe` never reports a partial
or guessed list. The contract wording moves from "without downloading its
content" to "without downloading its content layer".

**TUI.**

- The detail pane gains `Required by:` (one hop, `-` when none, `?` when
  incomputable) and `Why:` (the first path, `+N more`).
- Any row with requires expands like a bundle, with the member cache
  generalized to any node.
- Expansion is lazy and one level at a time. An ancestor repeated on the
  current branch renders as a non-expandable `(repeated)` leaf. This keeps
  the guard from `171c3290` for browse.

**grimoire-vscode** reads `direct` and `required_by` from `grim status
--format json`. It renders a "Required by" row in the details panel's Status
rail and runs `grim why --format json` on demand. It keeps `parseViaBundles`
only as a fallback when `required_by` is absent.

**A grimoire-vscode release that reads these fields ships with or before
this grim release**, because an older extension reading a `dependency: …`
source string or a `kept-by-dependency` status sees an unknown value.

**Stored or recomputed:** recomputed. Nothing "installed because" is
persisted.

### Decision 6: kinds and source-side declaration (one-way door for the key grammar)

Every installable kind may declare requires: skill, rule, agent, mcp and
hook. A bundle's members already are its requires. A bundle source gains a
`[bundles]` table (nested bundles).

**One grammar everywhere.** Table names are `skills`, `rules`, `agents`,
`mcp`, `hooks`, `bundles`. A binding name passes `SkillName::parse`. A
value is a `MemberRef`.

| Kind | Source declaration | Packed into the content layer? |
|---|---|---|
| skill, rule, agent | Frontmatter `metadata` keys `grim.requires.<table>.<name>: "<ref>"` | Yes, verbatim, as plain metadata the renderer keeps |
| mcp | Optional `[requires.<table>]` tables in `mcp/<name>.toml`; `docs/public/schemas/grim-mcp.schema.json` declares them | No. Split off before the canonical `McpDescriptor` JSON is built |
| hook | Optional `[requires.<table>]` tables in `hook.toml` | Yes, verbatim; the consumer's hook parser accepts and ignores the table |
| bundle | `[bundles]` table in the bundle source | n/a |

```yaml
---
name: gws-drive
description: Read and edit Google Drive files
metadata:
  grim.requires.skills.gws-shared: "./gws-shared:1"
  grim.requires.mcp.gdrive: "ghcr.io/acme/mcp/gdrive:2"
---
```

**Why flattened keys.**

- `metadata` is a string-to-string map, so a nested table cannot live
  there.
- One key per edge keeps every value a single `MemberRef` and makes
  duplicates impossible.
- `grim` is not a client namespace (`KNOWN_NAMESPACES`,
  `src/install/render.rs:67`), so these keys stay plain metadata.
- A `grim.requires.` key that does not parse is exit 65 at `grim build`.

`McpDescriptor` and `HookManifest` both use `deny_unknown_fields`, and serde
`flatten` does not combine with it. The MCP source therefore:

1. parses to a TOML table;
2. removes `requires`;
3. deserializes the remainder.

`HookManifest` gains an optional `requires` field. The hook kind is
experimental, so no shim is owed.

**Publishing.**

- `grim build` validates declarations offline.
- `grim release` checks the refs syntactically (Decision 3), writes the
  two-layer manifest, and freezes ids under `--pin`.
- `grim publish` orders entries topologically over same-manifest edges, with
  the fixed kind order as tie-break. A batch cycle is 65 before any push.
- **`same_content` is unchanged.** `classify` treats the JSON members
  media type as raw tar, so `content_key` already compares a requires layer
  by its raw digest (`src/oci/layer.rs:97-105`, `:257-268`).
- An identical re-release of a dependent tar-kind artifact is adopted (a
  test keeps it so).
- MCP and bundle releases keep their existing behavior: identical bytes are
  the same digest and succeed, while different bytes under an existing tag
  are `TagExists` (`src/command/release.rs:705-707`).

**Publisher guidance** (`publishing.md`). Adding `requires` to an artifact
under an existing floating tag breaks every grim 0.15.1 consumer of that tag
at their next install (exit 65). Publish the first dependency-bearing
version under a **new major tag**.

### Decision 7: export (orchestrator default)

`grim export plugin` and `grim export marketplace` **flatten the dependency
closure** into the exported plugin: every node reachable from the exported
artifact is written into the plugin as if it were a member. The export does
**not** emit Claude Code `dependencies`, because those take tag ranges over
git tags and grim cannot yet produce ranges.

Consequences:

- A cross-registry dependency's content is **copied** into the exported
  plugin. The plugin then redistributes another publisher's artifact, and
  that artifact's upstream updates do not reach plugin users until the
  plugin is re-exported.
- Export needs read credentials for every registry in the closure.
- Two exported plugins that share a dependency both carry it.

Exporting `requires` as plugin `dependencies` is a later, additive option.

### Decision 8: hardening carried by this change

- **`registry_text`.** It moves from a private helper in
  `src/oci/layer.rs:338` to a shared `src/oci` function. Every
  registry-sourced string that this feature prints passes through it:
  - member names and ids;
  - feature names;
  - media types;
  - repositories in conflict, cycle and cap messages.
- **`generated_by`** is validated before use (Decision 2).
- **Node-naming messages.** Every cycle, cap and conflict message names the
  publisher repository whose edge caused it, so the user knows whom to
  contact.

### Component contracts

Signature-level only; the plan owns placement.

```rust
// src/oci/manifest.rs
pub enum LayerLayout<'a> {
    Content(&'a Descriptor),
    ContentWithRequires { content: &'a Descriptor, requires: &'a Descriptor },
    BundleMembers(&'a Descriptor),
}
impl OciManifest {
    /// Rule F4. Replaces `single_layer()` on every materializing path and in `compression_of`.
    pub fn layout(&self, kind: ArtifactKind) -> Result<LayerLayout<'_>, ArtifactFeatureError>;
}

// src/oci/features.rs (new)
pub const REQUIRES_FEATURES_ANNOTATION: &str = "com.grimoire.requires-features";
pub const KNOWN_FEATURES: &[&str] = &["dependencies"];
pub const MAX_FEATURES_ANNOTATION_BYTES: usize = 1024;
pub const MAX_FEATURES: usize = 32;
#[derive(Debug, thiserror::Error)]
pub enum ArtifactFeatureError {      // exit 65, reason unsupported-artifact-feature
    UnknownFeatures { reference: String, features: Vec<String>, grim_version: &'static str },
    MalformedFeatures { reference: String, grim_version: &'static str },
    UnsupportedLayout { reference: String, media_types: Vec<String>, grim_version: &'static str },
}
pub fn check_required_features(reference: &str, manifest: &OciManifest) -> Result<(), ArtifactFeatureError>;

// src/oci/text.rs (moved) — shared escaper
pub fn registry_text(text: &str) -> String;

// src/oci/bundle.rs — document reused; fetch split
pub struct BundleMember { pub kind: ArtifactKind, pub name: String, pub id: String } // kind may be Bundle
pub const MAX_BUNDLE_MEMBERS: usize = 512;
pub const BUNDLE_LAYER_SIZE_LIMIT: u64 = 512 * 1024;
/// Bounded read of one members/requires layer by descriptor; no tag resolve.
pub async fn fetch_members_blob(access: &Arc<dyn OciAccess>, repo: &Identifier, layer: &Descriptor)
    -> Result<BundleManifest, ResolveErrorKind>;

// src/resolve/resolver.rs
pub const RESOLVE_CONCURRENCY: usize = 32;
pub const MAX_TRANSITIVE_NODES: usize = 2048;
pub const MAX_REQUIRES_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_DEPTH: usize = 32;
pub enum ResolveErrorKind {
    // existing variants unchanged; BundleConflict message generalized (78)
    DependencyCycle { cycle: Vec<(ArtifactKind, String)>, publisher: String }, // 65, reason null
    GraphTooLarge { limit: GraphLimit, via: String },                          // 65, reason null
    UnsupportedFeature(ArtifactFeatureError),                                  // 65, unsupported-artifact-feature
    // relative edge from a path source, zero-member requires layer, self-reference:
    //   BundleInvalid(..) (65, reason null), message names the parent
}
pub enum GraphLimit { Nodes(usize), RequiresBytes(u64), Depth(usize) }

// src/lock/locked_artifact.rs
pub struct LockedArtifact { /* … */ pub requires: Vec<BundleMember> }  // `[[<kind>.requires]]`, omitted when empty; ids as published
// src/lock/locked_bundle.rs
pub struct LockedBundle { /* … */ pub transitive: bool }               // `transitive = true`, omitted when false

// src/lock/effective_set.rs
pub type NodeKey = (ArtifactKind, String);
pub enum Origin {
    Direct(DeclaredSource),
    Implied { id: Identifier, contributors: Vec<BundleProvenance>, required_by: Vec<NodeKey> }, // replaces Bundles
    Conflicted,
}
pub struct EffectiveGraph {
    pub nodes: BTreeMap<NodeKey, Origin>,
    pub children: BTreeMap<NodeKey, BTreeSet<NodeKey>>,
}
pub fn effective_graph(set: &DesiredSet, lock: &GrimoireLock) -> Option<EffectiveGraph>;
pub fn effective_set(set: &DesiredSet, lock: &GrimoireLock) -> Option<BTreeMap<NodeKey, Origin>>; // = graph.nodes; was (set, &[LockedBundle])
pub fn declared_parent_provides(set: &DesiredSet, lock: &GrimoireLock, key: &NodeKey) -> bool;
pub const MAX_WHY_PATHS: usize = 256;
pub const WHY_STEP_BUDGET: usize = 100_000;
pub struct WhyPaths { pub paths: Vec<Vec<NodeKey>>, pub path_count: u64, pub truncated: bool }
/// Requires an acyclic graph (checked by the caller).
pub fn why_paths(graph: &EffectiveGraph, target: &NodeKey, limit: usize, step_budget: usize) -> WhyPaths;

// src/error.rs — ErrorReason, appended after `NoLock`
ErrorReason::UnsupportedArtifactFeature   // "unsupported-artifact-feature"

// src/api/uninstall_report.rs — UninstallStatus, appended
UninstallStatus::KeptByDependency         // "kept-by-dependency"
```

**JSON shapes.** Every field is always present; `null` only where stated.

| Report | Field | Type | Null / empty semantics |
|---|---|---|---|
| `status` item | `direct` | bool | never null |
| `status` item | `required_by` | `[{kind, name}]` \| null | `[]` = no parent; `null` = graph incomputable |
| `status` item | `source` | string | unchanged values; new literals `dependency: …` and `dependency` |
| `why` | `kind`, `name` | string | never null |
| `why` | `pinned` | string \| null | null for a path source or local bundle |
| `why` | `direct` | bool | never null |
| `why` | `paths` | `[[{kind, name, pinned}]]` | never empty on exit 0 |
| `why` | `path_count` | integer | exact, saturating at 2^53 - 1 |
| `why` | `truncated` | bool | never null |
| `why` | `overrides` | `[{kind, name, id}]` | `[]` = nothing shadowed |
| `add` | `pulled_in` | `[{kind, name, pinned}]` | `[]` = nothing pulled in |
| `describe`, MCP `grim_describe` | `requires` | `[{kind, name, id}]` \| null | `[]` = declares none; null = foreign manifest |
| `uninstall` item | `status` | string | new literal `kept-by-dependency` |
| error document | `reason` | string \| null | new literal `unsupported-artifact-feature` |

**Exit codes.** No new numeric code.

| Condition | Exit | Reason |
|---|---|---|
| Unknown or malformed required feature (F1), or unsupported layout (F4); at resolve, install, fetch or export | 65 | `unsupported-artifact-feature` |
| Dependency cycle | 65 | null |
| Node, requires-bytes or depth budget exceeded; requires list over 512; layer over 512 KiB; zero-member requires layer; invalid member name or id; relative edge from a path source; self-reference at build | 65 | null |
| Diamond conflict (different digests) | 78 | null |
| Edge target tag or repository not found | 79 | null |
| Registry unreachable / auth / offline miss | 69 / 80 / 81 | null |
| Install: lock `requires` differs from the pinned requires layer | 65 | `stale-lock` |
| Path source: re-read edges differ from recorded `requires` | 65 | `stale-lock` |
| `add --force` with a clobber on a transitive destination | 65 | `untracked-destination` |
| grim ≤ 0.15.1 reading a lock with `requires` or `transitive` | 78 | (shipped behavior) |
| `grim why`: as listed in Decision 5 | 79 / 79 `no-lock` / 79 `no-config` / 65 `stale-lock` / 64 | as listed |

**CLI flags.**

- **New:** `grim why <KIND> <NAME>`, with `KIND` one of skill, rule, agent,
  mcp, hook or bundle.
- **Widened:**
  - `grim release --pin` applies to any kind with requires.
  - `publish.toml` `pin = true` is accepted on any entry
    (`docs/public/schemas/grim-publish.schema.json` updated).
- **Narrowed in effect:** `grim add --force` covers the target's own
  destinations (Decision 4). This is a fresh surface: before this change no
  `add` had transitive destinations beyond bundle members, and for a bundle
  target, members are the target's own destinations.

### Quantified Impact

| Metric | Before | After | Notes |
|---|---|---|---|
| Registry requests per node, cold resolve | 1 (tag → digest) | 2-3 (+ manifest, + requires blob when present) | Install already fetches the manifest |
| Registry requests per unchanged node, warm re-lock | 1 | 1 | Manifest and blob served from `BlobStore` |
| Resolve wall time, 30 nodes, depth 2, 100 ms RTT, cold | ~0.1 s | ~0.6-0.9 s | Estimate, not measured |
| Resolve wall time, 200 direct artifacts, 100 ms RTT | ~0.1-0.3 s (unbounded) | ~0.7 s (7 waves of 32) | Cost of the ASYNC-05 bound |
| Worst-case nodes from one hostile declaration | 512 | 2048 transitive + 512 depth-1 | Plus 32 MiB requires bytes, 32 levels |
| Lock bytes per edge | 0 | ~70 B (`[[skill.requires]]` table) | Omitted without edges |

### Consequences

**Positive:**

- A skill brings its sibling skill and its MCP server.
- Bundles and dependencies are one mechanism, nested bundles work, and the
  bundle wire format is unchanged.
- Shipped grim refuses to materialize a dependent artifact. The new reader
  says "update grim" with a slug, and every later feature gets that gate.
- Edge integrity is checked at install against digest-verified bytes. No
  lock-carried cache is trusted.
- Removal is exact, offline, and never destructive when the graph is
  unknown.
- `grim status` exposes structured provenance, and `grim why` answers the
  owner's question with full chains and a path count.
- Resolve fan-out becomes bounded.

**Negative:**

- **Unfixable 0.15.1 residual.** Users who `lock`, `update` or
  `add --no-install` a dependent artifact get an incomplete lock with
  exit 0. Their next install refuses with exit 65 and an unhelpful message.
- **Silent transitive change.** A floating parent can gain a dependency,
  an MCP server or a hook included, without the consumer editing anything.
  That is trust decision 1 applied transitively. It is visible in the lock
  diff, `status` and `why`, and hooks still re-gate.
- **Tag flap.** A moving tag can turn a clean diamond into a conflict
  (exit 78) at a later update.
- **Slower large resolves.** Projects with more than 32 direct artifacts
  resolve somewhat slower.
- **Redundant field.** `transitive` exists only to make old grim refuse.
- **Refused cycles.** Some harmless cycles are refused.
- **Redistribution.** Export redistributes cross-registry dependencies.

**Risks:**

- **Registry acceptance.** Some registry may reject a two-layer manifest
  whose second layer is the bundle type. Each media type is GitLab-accepted
  alone, but the combination is untested. Mitigation: the release is gated
  on pushing and pulling the two-layer fixture on **GHCR, Docker Hub, ECR,
  Harbor and GitLab**, recorded in this ADR's changelog.
- **Online/offline policy drift.** The online expander and the offline
  closure could drift on the merge policy. Mitigation: one pure policy
  function with shared unit tests.
- **Exponential path enumeration in `why`.** Mitigation: step budget, path
  cap, and an exact DP count.
- **A publisher edge to a binding the consumer already declares.** Direct
  wins with a repository-mismatch warning, and the clobber gate still
  protects on-disk files.

## NFR coverage

**Scalability.** Expansion and closure are O(V + E) with a settled set.
Fan-out is bounded per node (512), per graph (2048 nodes, 32 MiB, 32
levels), and in `why` (256 paths, 100 000 steps). Lock growth is linear in
edges.

**Latency.** The cost is one manifest fetch per changed node plus one blob
GET per dependent node, behind 32 permits. Levels serialize, so latency
scales with depth, and real graphs are shallow. `remove`, `uninstall`,
`status` and `why` stay network-free.

**Security** (scoped to `.claude/rules/arch-threat-model.md`):

- **CWE-770 / CWE-400:**
  - per-node byte and member caps;
  - whole-graph node, byte and depth caps, enforced before admission;
  - per-node timeout over the whole fetch chain (ASYNC-04);
  - bounded `JoinSet` (ASYNC-05, ASYNC-09);
  - capped and step-budgeted path output.
- **CWE-22:** every member name passes `SkillName::parse`.
- **CWE-117 / terminal injection:** registry text is escaped through
  `registry_text`.
- **Cycle DoS:** a settled set guarantees termination. Cycles are refused at
  resolve and in an offline lock.
- **T1:** a malicious publisher can add transitive content. That is
  in-scope exposure already accepted for bundles by trust decision 1. No new
  gate, since out of scope by owner decision.
- **T2:** edges live in a digest-covered layer. The lock pins every node,
  and install compares recorded edges against the pinned bytes. The only
  cache is content-addressed.
- **T3:**
  - a cloned repository's declarations pull dependencies on install,
    exactly as bundles do;
  - `install` never writes consent;
  - `add` consents only to hooks new to the workspace through the target's
    own edges, so a repo-planted unconsented hook stays unarmed;
  - an attacker-edited lock `requires` is refused at install.
- **Plain HTTP:** transitive hooks pass the same `insecure-transport` rung as
  direct ones, on their own host (D4). Plain HTTP itself needs the existing
  insecure set; `--trust-hooks` keeps precedence, uniformly.
- **Cross-registry:** per-host credentials, never forwarded.
- No invariant I1-I6 is deviated from.

**Operability.**

- Every refusal names the node, its parent and the publisher to contact.
- Conflicts end with the fix.
- `unsupported-artifact-feature` names the running grim.
- `grim why` and `required_by` answer "why is this here" offline.
- Resolve logs one `debug` span per node with store hit or miss.

**Cost.** Registry egress is small: a manifest is a few KiB, and a requires
layer is bounded at 512 KiB, typically under 1 KiB.

## Migration and rollout

**One release (owner decision D3).** Reader and writer ship together in
release R (0.16.0). Shipped clients refuse at materialization, so no soak
period buys anything except the `lock`-is-silent window. That window exists
for any consumer that stays on 0.15.1, whatever the timing.

**Release gates, all required before tagging R:**

1. **Two-layer fixture on every registry.** A two-layer fixture artifact
   (skill + requires layer) is pushed and pulled back on GHCR, Docker Hub,
   ECR, Harbor and GitLab. The result is recorded in this ADR's changelog.
2. **grimoire-vscode release.** A release reading `direct`/`required_by`
   and knowing `kept-by-dependency` is published, or ships the same day.
3. **Old-grim check.** `test/tests/test_old_grim_dependencies.py` is run
   once locally with `GRIM_OLD_BIN` pointing to a v0.15.1 binary, and the
   result is recorded in the release notes. It stays local opt-in and is
   skipped when the variable is unset.
4. **The resolve fan-out bound** has landed as its own `fix(resolve):`
   commit.

**Old-grim behavior matrix:**

| Scenario | grim ≤ 0.15.1 | Release R |
|---|---|---|
| `grim add <dependent>` (default install) | lock written without edges; install exits 65 `expected a single-layer artifact, manifest has 2 layers` | closure locked and installed |
| `grim add --no-install` / `grim lock` / `grim update` with a dependent artifact | **exit 0, lock silently missing edges** | complete lock |
| `grim install` of a lock 0.15.1 wrote for a dependent artifact | exit 65 (two layers) | exit 65 `stale-lock`, "run grim lock" |
| `grim fetch <dependent>` | exit 65 | content layer printed |
| `grim describe <dependent>` | works; no requires info | `requires` populated |
| Bundle whose member is a dependent artifact | lock ok; install 65 | full closure |
| Bundle with a nested-bundle member | lock exits 65 `nested bundle member … is not supported` | expanded |
| Lock carrying `[[<kind>.requires]]` or `transitive` | exit 78 (`deny_unknown_fields`); old `remove` therefore cannot misread it | read |
| Dependency-free lock written by R | byte-identical modulo `generated_by`; reads fine | read |
| Dependency-free project locked offline after upgrade | — | locks offline; byte-identical modulo `generated_by` |
| Artifact listing an unknown `requires-features` name | ignored | exit 65 `unsupported-artifact-feature` |
| Three-layer manifest from a future grim | exit 65 (not single-layer) | exit 65 `unsupported-artifact-feature` |
| `grim build` of a source using `grim.requires.*` keys | packs them as plain metadata, emits no requires layer: **silent** (publisher-side) | requires layer emitted |
| `grim build` of `mcp/x.toml` with `[requires]` | exit 65 (`deny_unknown_fields`) | accepted |
| `publish.toml` `pin = true` on a skill | exit 65 | accepted |
| TUI, `search`, index browse | unaffected | requires shown on expand |

The silent publisher-side row is why `publishing.md` says "build with
grim ≥ R". A publisher on an old grim produces an artifact without its
requires layer: complete with no dependencies, never corrupt.

**Upgrade fixtures.** Both are produced by grim v0.15.1 (commit `cdeedcaa`)
and never regenerated from a later tree.

`test/data/golden/pre_dependencies_cdeedcaa/` follows the
`test/data/golden/pre_hooks_03e59b0/` layout:

```text
pre_dependencies_cdeedcaa/
  README.md                      # provenance and the never-regenerate warning
  commands.txt                   # the exact v0.15.1 commands that produced it
  global-grimoire.toml
  project/grimoire.toml          # direct skill, mcp, hook, registry bundle with a shared
                                 # member, single-provenance member, path skill, local bundle
  project/{skills,rules,agents}/ # path sources
  registry/pushed.json           # what the generator pushed (repos, tags, digests)
  golden/project.grimoire.lock
  golden/global.grimoire.lock
  golden/state.project.json
  golden/state.global.json
  golden/declaration_hash.json
  golden/tag-cache/              # warm tag cache + manifest blobs: offline store-hit case
  golden/tag-cache-no-manifests/ # same tag cache, no manifest blobs: offline store-miss case
  provenance/status.project.json
  provenance/status.global.json
  tools/{generate,replay,verify}.py
```

Its test is `test/tests/test_golden_pre_dependencies.py`, which asserts:

1. Each golden lock parses and re-serializes byte-identically.
2. `grim status --format json` works: `direct`/`required_by` are populated,
   and `source` strings equal `provenance/status.*.json`.
3. `grim lock` against the replayed registry writes a lock byte-identical
   modulo `generated_by`.
4. `grim lock --offline` writes the same bytes (modulo `generated_by`) in
   two variants, which together are the pre-R offline upgrade case:
   - **4a:** with only `golden/tag-cache/`, the store-hit branch;
   - **4b:** with only `golden/tag-cache-no-manifests/`, the store-miss
     carry-forward branch.
5. `grim remove` of the bundle drops exactly what 0.15.1 dropped.

**Docs to update:**

- `docs/src/content/docs/stability.md`:
  - Additive fields;
  - the `#frozen` OCI row (two-layer layout, a grim convention, plus the
    `com.grimoire.requires-features` annotation);
  - annotations at l.64-66;
  - Forward compatibility at l.413-462: new-lock → 78 on old grim, the
    install refusal on ≤ 0.15.1, the slug, and the silent-lock residual.
- `docs/src/content/docs/concepts.md`:
  - new `## Dependencies {#dependencies}`;
  - `#bundle-conflicts` generalizes, adds the digest-equal merge rule, the
    flap, the direct-declaration fix and the cycle rule;
  - `#bundle-pinning` extends to requires;
  - `#bundles` drops "never a nested bundle".
- `docs/src/content/docs/publishing.md`: declaring requires per kind,
  `--pin`, `publish.toml` `pin` and ordering, the new-major-tag guidance,
  and the minimum consumer grim.
- `docs/src/content/docs/artifacts.md`: the layer layout and the requires
  layer.
- `docs/src/content/docs/json-interface.md`:
  - status `direct`/`required_by` (including `null`);
  - the `why` report, `add.pulled_in` and `describe.requires`;
  - the new reason slug and literals.
- `docs/src/content/docs/commands.md`:
  - `grim why`;
  - status, remove and uninstall semantics, including `kept-by-dependency`;
  - `add --force` scope;
  - export flattening.
- `docs/src/content/docs/configuration.md`: the lock reference
  (`[[<kind>.requires]]`, `transitive`).
- `docs/src/content/docs/upgrading.md`:
  - the matrix notes;
  - the narrowed bundle-consent behavior: adding a bundle no longer consents
    a hook member that was already unconsented in the workspace (permitted
    because hooks are experimental).
- **Schemas:** `docs/public/schemas/grimoire-lock.schema.json`,
  `grim-mcp.schema.json` and `grim-publish.schema.json`.
- **AI config:**
  - `.claude/rules/subsystem-cli-commands.md` (the `why` row and status
    fields);
  - the `.claude/rules/arch-principles.md` ADR index row;
  - a catalog drift review of `grim-usage` and `grim-authoring`
    (`catalog/README.md`).

## Convention deviations, named

1. `transitive = true` is a redundant, derivable lock field kept only so old
   grim refuses, protecting old `remove` from data loss (D2).
2. Cycles are refused even where the closure would be harmless.
3. `RESOLVE_CONCURRENCY` slows resolve for projects with more than 32 direct
   artifacts. The change also fixes an existing ASYNC-05 violation and lands
   as its own commit.
4. The `ErrorReason` set gains a ninth slug, so two frozen-contract tests
   are extended by appending: `c031_4_no_new_error_reason_slug`
   (`FROZEN_REASON_SLUGS`, `src/error.rs`) and
   `c021_exit_codes_and_reasons_equal_main` (`src/command/export.rs:660`).
5. `grim.requires.*` is the first non-client prefix in frontmatter
   `metadata`.
6. The dual-shape template of `2673bb2d` is not applied to the new field.
7. The three published JSON schemas change additively, not
   byte-identically.
8. `grim describe`'s contract wording changes ("content layer").
9. The nested-bundle rejection and the TUI guard of `171c3290` are reversed
   for resolve and narrowed to a `(repeated)` render for browse.
10. The status `source` field gains the `dependency: …` and `dependency`
    literals. An existing grimoire-vscode that matches only `direct` and
    `bundle: ` sees an unknown value. Hence the paired extension release.
11. `UninstallStatus` gains `kept-by-dependency`, with the same consumer
    impact and the same mitigation.
12. `required_by` is nullable, which departs from the always-present
    non-null array norm in `src/api` because "unknown" must not read as
    "none".
13. The pre-cache destructive fallback in remove/uninstall is replaced by
    target-only behavior for incomputable graphs.

grim ≤ 0.15.1's silent incomplete lock cannot be fixed. It is documented,
not smoothed over.

## Open questions

None. Both questions were closed on 2026-11-02 by the goal loop
(`.agents/goals/adr-artifact-dependencies.md`, autonomy: no prompting), on
their recorded recommendations. The decision trail is in
`.agents/plans/plan_artifact_dependencies.md` § Decisions taken while
decomposing (PD-1, PD-2).

- **Whole-graph bounds: closed, kept as written.** `MAX_TRANSITIVE_NODES =
  2048`, `MAX_REQUIRES_BYTES = 32 MiB`, `MAX_DEPTH = 32`. Demand is shallow
  (skill → sibling, skill → MCP), and raising a cap later is additive while
  lowering one is a break.
- **`grim describe` fetches the requires layer: closed, yes.** `requires` is
  populated from the layer (one GET bounded at 512 KiB, only when the layer
  exists). `has_requires: bool` is not added. `grimoire-vscode` needs the
  names to show "this will also install …" before the user adds anything.

## Links

- `.agents/discussions/artifact-dependencies.md`
- `.agents/specs/design_artifact_dependencies.md`
- `.agents/adr/adr_artifact_trust_model.md`
- `.agents/adr/adr_effective_set_mutations.md`
- `.agents/adr/adr_hook_workspace_consent.md`
- `.agents/adr/adr_render_layout_stability.md`
- `.agents/adr/adr_oci_empty_config_compat.md`
- `.agents/adr/adr_compressed_artifact_layers.md`
- `docs/src/content/docs/stability.md#limitations-forward-compat`
- `docs/src/content/docs/concepts.md#bundle-pinning`

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-11-01 | Claude (architect) | Initial proposed draft |
| 2026-11-01 | Claude (architect) | Revision 2: owner decisions D1-D4; review round. Content-addressed edge cache and install set-compare; fetch by descriptor; consent narrowed to the target's new reach; non-destructive incomputable closure; partial-update pin rule; offline locking kept; node/bytes/depth caps; `same_content` change dropped; `[[<kind>.requires]]` shape; one release; export flattening; `why` gains `path_count` and `overrides`; per-decision reversibility |
| 2026-11-02 | Claude (goal loop, /hex-plan) | Status Proposed → Accepted. Open questions closed on their recommendations (caps 2048 / 32 MiB / 32 kept; `describe` fetches the requires layer). Decomposed in `plan_artifact_dependencies.md` |
| 2026-11-02 | Claude (/hex-execute) | Execution notes, decision text unchanged. Member and requires order is `ArtifactKind` enum order then name (skill, rule, agent, bundle, mcp, hook), the order `BundleManifest::new` already used; the D4 lock example's order is illustrative. Observed against grim 0.15.1: a lock carrying `[[<kind>.requires]]` or `transitive = true` exits 78; a lock with a bare nested-bundle member is read and the member ignored; only a nested-bundle manifest exits 65; `lock`/`update`/`add --no-install` of a dependent artifact silently write a lock without edges, which a newer grim refuses with 65 `stale-lock`. `stability.md` and `upgrading.md` carry the observed table, which supersedes the compatibility-matrix wording above. `lock` applies F1/F4 at resolve, so a forward-incompatible manifest reaches `install` only through a lock pinned earlier |
| 2026-11-03 | Claude (owner review) | Rule F4 classifies the two layers by media type instead of position, so a requires layer written first is read, not refused; a non-bundle whose only layer is a requires layer is now refused. Writer order (content first), the wire format and the bundle arm are unchanged; widening a reader is additive |
