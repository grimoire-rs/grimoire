# Research: Lockfile scoping, build-metadata versioning, rename/reference-checking, and reproducible package-export prior art

## Metadata

**Date:** 2026-09-27
**Domain:** packaging
**Triggered by:** `/hex-plan` for harness-native marketplaces (`.agents/discussions/harness-native-marketplaces.md`) — `marketplace.toml`/`marketplace.lock`, `grim export plugin`, `[plugins.x.rename] strip_prefix`, and reproducible export output
**Expires:** 2027-03-27

## Direct Answer

Four independent questions, one answer each:

1. **Scoped locks**: pnpm's `importers:` map (one lockfile section per workspace project, same package pinned differently per importer) is the closest existing shape to `marketplace.lock`'s additive `plugin = "<name>"` scope on `[[skill]]`/`[[rule]]`/… entries — reuse the *idea* (scope key beside the entry, one lock, N scopes), not the YAML shape (grimoire's lock is already TOML arrays). Partial-update selectors converge on "unknown selector → hard error, no silent no-op" everywhere it was checked (`cargo update -p`, `uv lock --upgrade-package`, pnpm `--filter`) — the plan's `grim update --marketplace <plugin>[:<member>]` erroring on an unknown selector matches the field, not an outlier.
2. **Build-metadata versions**: Claude Code's own plugin-update algorithm is **confirmed as plain string equality** — "Updates are detected by comparing versions... users stay on their cached copy until the string changes" (`host-marketplace.md`, fetched 2026-09-27). That validates `<version>+<hash>` as the *only* correct way to force a re-pull on a member digest change under this consumer; semver-aware dedup (Cargo/npm) would be wasted sophistication here. Go's 12-hex-char pseudo-version suffix is the strongest precedent for hash length; recommend **12 hex chars** over 7.
3. **Rename + dangling-reference checking**: Claude Code ships a **directly analogous, shipped mechanism** for exactly this problem — a top-level `renames` map in `marketplace.json` that is append-only, chain-resolved, and **fails closed** (`claude plugin validate` rejects a cycle or a chain not ending in `null`/a live name, error `renames.<name>: chain does not resolve`). This is real, in-production prior art for the plan's `[plugins.x.rename]` fail-closed contract, from the very same target ecosystem — cite it directly, don't reach for npm/Helm as the primary analogy.
4. **Reproducible export**: `helm package` (Helm ≥4.0.0) and `vsce package` both had to retrofit bit-for-bit reproducibility years after shipping (fixed uid/gid/permissions, `SOURCE_DATE_EPOCH`, sorted archive entries) — build that in from day one rather than as a v2 patch. `npm pack --json`'s shape is **not stable across major versions** (array in npm 11, object-keyed-by-name in npm 12) — a cautionary example for `grim export plugin --format json`, which must pin its own shape as a frozen contract rather than mirror an upstream tool's.

## Technology Landscape

### Established (proven, widely accepted)

| Tool/Pattern | Status | Notes |
|---|---|---|
| pnpm `importers:` scoped lockfile | Standard for pnpm workspaces | One lockfile, one `importers` entry per workspace project + shared `packages`/`snapshots`; a package can resolve to different versions per importer. [pnpm.io/lockfile](https://pnpm.io/lockfile), [spec/lockfile/6.0.md](https://github.com/pnpm/spec/blob/master/lockfile/6.0.md) |
| Cargo workspace `Cargo.lock` | Standard | Single lock at workspace root; `cargo update -p <pkg> --precise <ver>` scopes a bump to one package. Pre-release precise-update UX has an open sharp edge ([rust-lang/cargo#12579](https://github.com/rust-lang/cargo/issues/12579)). |
| uv workspace `uv.lock` | Standard (uv 0.4+) | "the workspace shares a single lockfile"; `--package <name>` scopes `run`/`sync` to one member; `uv lock --upgrade-package <name>` scopes a resolve. [docs.astral.sh/uv/concepts/projects/workspaces](https://docs.astral.sh/uv/concepts/projects/workspaces/) |
| npm workspaces, single `package-lock.json` | Standard, known-fragile | Root lock covers all workspace packages; drift between nested and root lockfiles is a recurring, hard-to-detect-in-CI failure mode ([npm/cli#4460](https://github.com/npm/cli/issues/4460), [blamechris/chroxy#7324](https://github.com/blamechris/chroxy/issues/7324)). |
| SemVer 2.0.0 build metadata (`+build`) | Standard | Build metadata is explicitly precedence-inert: `1.0.0+a` and `1.0.0+b` compare equal. [semver.org](https://semver.org/) |
| Go module pseudo-versions | Standard | `vX.0.0-yyyymmddhhmmss-abcdefabcdef`; hash segment is a fixed **12-character prefix** of the commit hash. [go.dev/ref/mod](https://go.dev/ref/mod) |
| VS Code extension odd/even minor for pre-release | Standard workaround | Marketplace has no semver pre-release field, so parity of the minor version encodes channel; auto-update always prefers the highest version regardless of channel. [code.visualstudio.com/api/.../publishing-extension](https://code.visualstudio.com/api/working-with-extensions/publishing-extension) |
| `cargo package` / `cargo publish` tarball | Standard | Includes `Cargo.lock` automatically when the package has a binary/example target; `cargo install --locked` consumes it for reproducible builds. [doc.rust-lang.org/cargo/commands/cargo-package](https://doc.rust-lang.org/cargo/commands/cargo-package.html) |

### Emerging (early but promising)

| Tool/Pattern | Signal | Worth Watching Because |
|---|---|---|
| Bun's text-based `bun.lock` | Default since Bun 1.2 | Single root lockfile with a `workspaces` object keyed per member, chosen explicitly for clean PR diffs — same "flat map keyed by scope" shape `marketplace.lock`'s `plugin = "<name>"` field takes, arrived at independently. [bun.com/blog/bun-lock-text-lockfile](https://bun.com/blog/bun-lock-text-lockfile) |
| `helm package` reproducibility (Helm ≥4.0.0) | Shipped 2026 | Constant uid/gid, fixed permissions, deterministic dependency iteration order — a multi-year-late fix for exactly the bug class `grim export plugin`/`marketplace` must avoid from day one. [helm/helm#3612](https://github.com/helm/helm/issues/3612) |
| `vsce package` `SOURCE_DATE_EPOCH` + sorted entries | Shipped via PR | Same fix shape as Helm's, arrived at independently by a second ecosystem — converging evidence that "sort entries, pin timestamp" is the whole reproducibility recipe for a zip/tar export tool. [microsoft/vscode-vsce#1100](https://github.com/microsoft/vscode-vsce/pull/1100) |
| Claude Code marketplace `renames` map | Documented, versioned (requires Claude Code ≥2.1.193) | A shipped, fail-closed rename/migration mechanism in the exact target ecosystem `grim export plugin` renders for — the strongest possible prior art for `[plugins.x.rename]`. [code.claude.com/docs/en/plugins/host-marketplace](https://code.claude.com/docs/en/plugins/host-marketplace) |

### Declining (losing mindshare)

| Tool/Pattern | Signal | Avoid Because |
|---|---|---|
| `npm pack` un-versioned JSON shape as an integration contract | Breaking shape change npm 11→12 (array → object keyed by package name) | Confirms JSON output needs its own frozen schema/version, never "shaped like whatever the wrapped tool emits this month." |
| Poetry same-package-different-version-per-group | Poetry actively **rejects** this (post-2.0, collapses to lowest version, or hard errors) | Directly the opposite of what `marketplace.lock` needs (same artifact, two plugins, two digests) — cited as the negative case: don't follow Poetry's single-resolution-domain model. [python-poetry/poetry#7231](https://github.com/python-poetry/poetry/issues/7231) |

## Design Patterns Worth Considering

- **Scope key beside the entry, not a parallel lock** — pnpm's `importers:` and Bun's `workspaces:` both put the workspace-scope information in the *lockfile*, not a second file. `marketplace.lock`'s additive `plugin = "<name>"` field on the existing `grimoire_lock.rs` entry types follows this directly and is the right call — a parallel `marketplace-lock.rs` struct would be the "second lock path" the Decisions doc's Reuse Mandate explicitly forbids.
- **Fail closed on an unrecognized selector, everywhere partial-update exists** — `cargo update -p <unknown>`, `uv lock --upgrade-package <unknown>`, pnpm `--filter <unknown>` (with scope, "picks nothing" is the documented footgun when scope is *omitted*, not when the name plain doesn't exist) all treat "selector doesn't resolve" as an error, never a silent no-op. `grim update --marketplace <plugin>|<plugin>:<member>` erroring on an unknown selector is the field norm, not a design risk to flag.
- **Append-only rename ledger with closed-world validation** — Claude Code's `renames` map (chain must resolve to `null` or a live plugin name; a cycle is a validation error) is the pattern to point at directly when justifying `[plugins.x.rename]`'s fail-closed contract (empty name after strip, post-strip collision, stale reference). Grimoire's version is stricter (it also scans member *content* for stale mentions, which Claude Code's mechanism does not attempt — Claude Code only migrates the *identifier*, never checks prose/paths inside files), which is the right asymmetry: grim controls the rendered bytes, Claude Code only controls the installed-plugin name-to-name mapping.
- **Precedence-inert build metadata repurposed as a cache-buster, not a comparator** — SemVer's own spec says build metadata "MUST be ignored when determining version precedence." Every one of the harnesses in `research_plugin_support_matrix.md` that grim targets does **plain string equality**, not semver precedence, on the plugin version field (confirmed for Claude Code by direct doc fetch this session; undocumented-but-presumed-same for Copilot/Codex per the existing matrix). That means grim's `<version>+<hash>` suffix works by accident of every consumer's actual algorithm (string compare), not because of SemVer's formal precedence rule — worth stating explicitly in the ADR so nobody later "fixes" the comparison to be semver-aware and breaks the update signal.
- **Deterministic archive = sorted entries + pinned timestamp, nothing else** — Helm's and vsce's independently-arrived-at fixes are the same two moves. `grim export plugin --zip` needs both: stable JSON key order (already planned) plus zip entry order and a fixed mtime (`SOURCE_DATE_EPOCH`-style, or simply zero/epoch — no external env dependency needed since export is a single Rust process, not a multi-tool pipeline).
- **A wrapped tool's ad-hoc JSON output is not a contract** — npm's `pack --json` shape changed under a major bump with no deprecation path for consumers. `grim export plugin --format json` must define and freeze its *own* schema (per Principle 9) rather than mirroring `cargo package`/`npm pack`/`helm package` output verbatim, even though those tools are the closest functional analogues.

## Key Findings

1. Claude Code's plugin-update mechanism is **plain string equality on the version field** — "users stay on their cached copy until the string changes" and "Updates are detected by comparing versions, so a ref that moves without a version change leaves users on the cached copy" ([host-marketplace.md](https://code.claude.com/docs/en/plugins/host-marketplace)). This directly confirms the support-matrix research's inference and is now sourced from primary docs, not inference.
2. Claude Code ships `renames` — a top-level, append-only, chain-validated rename map in `marketplace.json`, with `claude plugin validate` refusing a cycle or a dangling chain (`renames.<name>: chain does not resolve`) — the single best piece of prior art for the plan's fail-closed rename contract, and it comes from the exact consumer ecosystem, not an adjacent one.
3. SemVer build metadata is formally precedence-inert (semver.org spec, §build metadata); the plan's `<declared-or-0.0.0>+<hash>` scheme relies on every real consumer doing string comparison, not semver-aware comparison — true for Claude Code (confirmed) and assumed but unverified for Copilot/Codex/Cursor (Agent Plugins 1.0's version-comparison algorithm is undocumented per `research_plugin_support_matrix.md`).
4. Go's pseudo-version format is the strongest existing precedent for a content-derived version suffix and uses a **12-character** commit-hash prefix with no published collision-risk rationale in the reference docs — but 12 hex chars (48 bits) gives a >50% collision chance only past roughly 16.7M distinct hashes (birthday bound ≈ 2^24), which is far beyond any plausible per-plugin digest-set count; 7 hex chars (28 bits, git's historical default) starts colliding at ~16K items, plausible for a large public marketplace's total plugin count over its lifetime. Recommend 12, matching Go's precedent, not git's 7-char default.
5. Every scoped-lock ecosystem checked (pnpm, Cargo workspaces, uv workspaces, Bun) puts the per-scope pin inside one shared lockfile rather than N separate lockfiles — validates `marketplace.lock`'s single-file-plus-`plugin`-field design over any per-plugin-lockfile alternative that was not seriously considered but is worth naming as the rejected alternative in the ADR.
6. `helm package` and `vsce package` reproducibility were both **retrofitted**, years after the tools shipped, once users noticed spurious diffs from non-deterministic archive ordering and timestamps ([helm/helm#3612](https://github.com/helm/helm/issues/3612), [microsoft/vscode-vsce#1100](https://github.com/microsoft/vscode-vsce/pull/1100)) — strong argument for treating "byte-reproducible" as a day-one test in `grim export plugin`/`marketplace`, not a follow-up fix.
7. `npm pack --json`'s output shape broke across a major version (array → object-keyed-by-package-name) with no compatibility shim — a concrete cautionary tale for freezing `grim export plugin --format json`'s own schema under Principle 9 rather than mirroring any single upstream tool.
8. Poetry's dependency-groups model is the negative case for "same artifact pinned differently per scope": Poetry actively collapses or rejects differing versions of the same package across groups post-2.0 ([python-poetry/poetry#7231](https://github.com/python-poetry/poetry/issues/7231)) — the opposite of what `marketplace.lock` needs (two plugins, two digests, no shared resolution domain), useful as the counter-example in an ADR's "alternatives considered."

## Recommendation

- **Scoping**: keep the plan as decided — one `marketplace.lock` in the existing grimoire-lock TOML shape, additive `plugin = "<name>"` per entry, reusing `src/lock/*`. This matches every scoped-lock ecosystem surveyed (pnpm, Cargo, uv, Bun); no ecosystem uses N separate lockfiles for N scopes.
- **Update selectors**: `grim update --marketplace <plugin>|<plugin>:<member>` should hard-error on an unknown selector (already the plan) — this is the universal behavior, not an outlier to second-guess.
- **Version hash**: use **12 hex characters**, following Go's pseudo-version precedent, not 7 (git's short-SHA default) — 7 chars is collision-plausible at marketplace scale; 12 is not, and the extra 5 characters cost nothing in a version string nobody hand-types.
- **Version comparison**: document explicitly, in the ADR, that `<version>+<hash>` works *because* every known consumer does string equality (confirmed for Claude Code; presumed for Agent Plugins 1.0 family pending upstream docs) — this is a load-bearing assumption, not an implementation detail, and should be called out so a future semver-aware consumer doesn't silently break the update signal.
- **Rename fail-closed checks**: the plan's design (empty name after strip, post-strip collision, stale in-file reference, reported file:line) is already stricter and more correct than Claude Code's own `renames` mechanism (which only migrates the identifier, never scans file contents) — cite Claude Code's shipped mechanism in the ADR as validating the *shape* (fail closed, table for future rule kinds) while noting grim's scope is deliberately wider (content, not just identifier).
- **Reproducibility**: build sorted-entry-order + fixed-timestamp into `grim export plugin --zip` from the first implementation, not as a follow-up — both Helm and vsce needed a dedicated fix release for exactly this, years in.
- **JSON output**: freeze `grim export plugin --format json`'s schema as grim's own contract (versioned like every other frozen CLI surface per Principle 9); do not structurally mirror `npm pack --json`, `cargo package`, or `helm package` output, since none of them are stable across their own tool's versions.

## Sources

| Source | Type | Date | Relevance |
|---|---|---|---|
| [pnpm.io/lockfile](https://pnpm.io/lockfile) | Docs | fetched 2026-09-27 | `importers:` scoped-lockfile structure |
| [github.com/pnpm/spec — lockfile/6.0.md](https://github.com/pnpm/spec/blob/master/lockfile/6.0.md) | Spec | 2026-09-27 | Lockfile v6 field layout |
| [pnpm.io/filtering](https://pnpm.io/filtering), [pnpm.io/cli/update](https://pnpm.io/cli/update) | Docs | 2026-09-27 | `--filter` selector semantics and unknown-package behavior |
| [doc.rust-lang.org/cargo/commands/cargo-update](https://doc.rust-lang.org/cargo/commands/cargo-update.html) | Docs | 2026-09-27 | `-p`/`--precise` scoped update |
| [rust-lang/cargo#12579](https://github.com/rust-lang/cargo/issues/12579) | Issue | 2026-09-27 | `--precise` pre-release error-message sharp edge |
| [doc.rust-lang.org/cargo/commands/cargo-package](https://doc.rust-lang.org/cargo/commands/cargo-package.html) | Docs | 2026-09-27 | `Cargo.lock` inclusion rule in packaged tarball |
| [docs.astral.sh/uv/concepts/projects/workspaces](https://docs.astral.sh/uv/concepts/projects/workspaces/) | Docs | fetched 2026-09-27 | Shared `uv.lock`, `--package` scoping |
| [astral-sh/uv#9449](https://github.com/astral-sh/uv/issues/9449) | Issue | 2026-09-27 | `--upgrade-package` documentation gap (optional deps) |
| [docs.npmjs.com/cli/deprecating-and-undeprecating-packages](https://docs.npmjs.com/deprecating-and-undeprecating-packages-or-package-versions/) | Docs | 2026-09-27 | npm deprecate as the closest npm rename/migration primitive |
| [semver.org](https://semver.org/) | Spec | 2026-09-27 | Build-metadata precedence-inert rule |
| [code.claude.com/docs/en/plugins/host-marketplace](https://code.claude.com/docs/en/plugins/host-marketplace) | Docs | fetched 2026-09-27 | Confirmed string-equality version comparison; `renames` map fail-closed contract; release-channel and command-source update mechanics |
| [code.claude.com/docs/en/plugin-marketplaces](https://code.claude.com/docs/en/plugin-marketplaces) | Docs | fetched 2026-09-27 | Plugin/skill namespacing (`<plugin>:<skill>`), marketplace validation error shapes |
| [go.dev/ref/mod](https://go.dev/ref/mod) | Spec | fetched 2026-09-27 | Pseudo-version format, 12-char commit-hash prefix |
| [github.com/oven-sh/bun — bun-lock-text-lockfile](https://bun.com/blog/bun-lock-text-lockfile) | Blog/announcement | 2026-09-27 | `bun.lock` `workspaces:` scoped structure |
| [python-poetry/poetry#7231](https://github.com/python-poetry/poetry/issues/7231) | Issue | 2026-09-27 | Poetry's rejection of same-package/different-version-per-group (negative case) |
| [helm.sh/docs/helm/helm_dependency](https://helm.sh/docs/helm/helm_dependency/), [helm/helm#3612](https://github.com/helm/helm/issues/3612) | Docs/Issue | 2026-09-27 | Chart alias/rename fragility; reproducible-tarball fix history |
| [github.com/npm/cli#5334](https://github.com/npm/cli/issues/5334), npm pack docs | Issue | 2026-09-27 | `npm pack --json` shape instability across npm 11/12 |
| [github.com/microsoft/vscode-vsce#1100](https://github.com/microsoft/vscode-vsce/pull/1100) | PR | 2026-09-27 | `SOURCE_DATE_EPOCH` + sorted-entry reproducibility fix |
| [code.visualstudio.com/api/working-with-extensions/publishing-extension](https://code.visualstudio.com/api/working-with-extensions/publishing-extension) | Docs | 2026-09-27 | Odd/even minor pre-release versioning convention |
| `.agents/research/research_plugin_support_matrix.md` (this repo) | Internal research | 2026-09-27 | Cross-harness plugin mechanism/version-semantics matrix, reused as baseline |
