# Research: Lock pin stability — package-manager prior art

## Metadata

**Date:** 2026-11-03
**Domain:** packaging
**Triggered by:** /hex-discuss lock-pin-stability (.agents/discussions/lock-pin-stability.md)
**Expires:** 2027-05-03

Scope: when a lockfile is regenerated or a dependency is added without an explicit update command, is an existing lock entry kept as-is or resolved again. Findings only; no recommendation. Docs pages cited were fetched on 2026-11-03 and reflect "latest" at that date unless marked. Items older than ~18 months (before 2025-05) are flagged **[re-verify]**. Claims marked (unverified) come from a search snippet or general knowledge, not a fetched primary source.

## Direct answer

Five families exist.

1. **Lock is a preference; only a violated requirement moves an entry.** Cargo, uv, pip-tools, Bundler (conservative), Poetry (default since 2.0), pnpm, Terraform, SwiftPM. A requirement edit is judged per entry: does the locked version still satisfy it. Unrelated entries stay.
2. **Lock is a contract; whole-file staleness check, then refuse.** OCX (`declaration_hash`), Helm `build` (digest over the dependency list), `npm ci`, `uv --locked`, `cargo --locked`, `pnpm --frozen-lockfile`, `yarn --immutable`, `deno --frozen`. Mismatch is an error, not a re-resolve.
3. **Whole-file hash mismatch triggers whole-file re-resolve.** Helm `dependency update` and OCX `lock` on a drifted config re-resolve every entry. The recurring complaint is that adding one dependency also moves the others (helm/helm#11308).
4. **Per-entry "original vs locked" comparison.** Nix flakes compare each input's recorded `original` to the current declaration; only changed or new inputs are re-fetched. Kbld, ToolHive and Dagger also keep the requested reference beside the resolved digest.
5. **Mutable reference policy is separate from version policy.** Cargo/Nix/SwiftPM/Go pin a branch to a commit and move it only on an explicit update. Dagger makes the choice a per-entry policy (`pin` vs `float`). Helm and rules_oci tags have no digest in the lock at all (open proposal).

## Per-tool findings

Axes: (1) what the lock records about the request; (2) which commands may or never move an existing entry; (3) how a changed requirement is detected, and what happens to other entries; (4) transitive deps of an unchanged parent; (5) mutable references; (6) new vs changed vs unchanged shown in output; (7) incidents and design debates.

### OCX (ocx-sh/ocx) — closest OCI-native analog

- Public repo: https://github.com/ocx-sh/ocx. Its own `ocx.lock` (lock_version 3): https://github.com/ocx-sh/ocx/blob/main/ocx.lock
- Axis 1: records **no tag and no requirement**. Per tool: `name`, `group`, `repository`, and a `[tool.platforms]` table of platform to leaf manifest digest. The header carries one `declaration_hash = "sha256:..."` over the canonicalized `ocx.toml`, plus `generated_by` and `generated_at`. Digests are per-platform leaf manifests, never the index digest, because a publisher's index digest is garbage-collected on re-push: https://github.com/ocx-sh/ocx/blob/main/website/src/content/docs/explanation/dependencies.mdx
- Axis 2: only five mutators touch `ocx.lock`: `add`, `remove`, `lock`, `update`, `upgrade`. Read paths (`pull`, `exec`) never write and never re-resolve: https://github.com/ocx-sh/ocx/blob/main/website/src/content/docs/guides/lock-and-reproduce.mdx
  - `add` / `remove` are "partial mutators": every untouched entry is carried forward byte-identical with no registry contact; `add` is idempotent for an identical identifier and refuses a different identifier for an existing name (exit 64): https://github.com/ocx-sh/ocx/blob/main/website/src/content/docs/explanation/project-toolchain.md (section "Pin preservation")
  - `lock`: "A current lock is carried forward verbatim, never advancing a moving tag; a drifted one re-resolves every tag." Source: https://github.com/ocx-sh/ocx/blob/main/crates/ocx_cli/src/command/lock.rs
  - `update` (bare = every tag, even when the lock is current; `NAME` / `-g GROUP` scopes it) and `upgrade` (rewrites the tag in `ocx.toml` then re-locks only retagged bindings): https://github.com/ocx-sh/ocx/blob/main/website/src/content/docs/guides/update-tools.mdx
- Axis 3: **whole-file**. On hash mismatch every read path and the partial mutators exit 65 ("There is no implicit re-resolution"). The reconcile command, `ocx lock`, then re-resolves **all** declared tags, so a moving tag may advance. Their docs state this openly in the table row "A drifted config re-resolves every declared tag": https://github.com/ocx-sh/ocx/blob/main/website/src/content/docs/guides/update-tools.mdx
  - Cost: a hand edit to one binding followed by `ocx lock` moves every other moving-tag binding. The workaround offered is `ocx add` / `ocx remove` for edits, and pinned tags (`:3.28`) for stability.
  - `lock --check` is a no-network CI gate (65 stale, 78 missing).
- Axis 4: dependencies of a package are pinned by the publisher as manifest digests in the package's own metadata ("no version ranges, no resolution algorithm"); the project lock does not re-resolve them: https://github.com/ocx-sh/ocx/blob/main/website/src/content/docs/explanation/dependencies.mdx
- Axis 5: a tag is resolved at `lock`/`update` time and the digest recorded. "A lock records the exact digest it pinned and never reads the index back to confirm it." A moved tag is invisible to `lock` when current, and to every read path. Only `update` observes it.
- Axis 6: only `update` shows what moved. Report is `Binding | From | To`, with unchanged pins summarized behind a hint; JSON is `{changes, unchanged, metadata_changed}`; `--check` exits 65 when something would move. `lock` prints the full lock, not a diff. Same guide as above.
- Axis 7: the repo's own ADR records a design reversal. The original spec banned subset update on the grounds of "anti-laundering" (a silent full re-resolve reachable from `add`/`remove` "launders a stale lock under a fresh declaration hash") and "anti-drift"; the ADR later allowed scoped `update` while keeping both guarantees: https://github.com/ocx-sh/ocx/blob/main/.claude/artifacts/adr_scoped_upgrade.md (2026-07-09). No public issue about unexpected upgrades was found (GitHub search of the repo for "lock" returned only unrelated items).

### Cargo (Cargo.lock)

- Axis 1: exact version and source; for git, the source string carries the commit after `#`, e.g. `git+https://github.com/servo/angle?branch=servo#<sha>`; the `?branch=` query is part of the source (search snippet, https://internals.rust-lang.org/t/upcoming-changes-to-cargo-lock/14017, 2021 **[re-verify]**; unverified on whether editing the branch rewrites the entry). The manifest requirement is **not** stored.
- Axis 2: `cargo update` (all), `cargo update <spec>` (conservative: "only the dependency specified by SPEC will be updated. Its transitive dependencies will be updated only if SPEC cannot be updated without updating dependencies. All other dependencies will remain locked"), `--precise`, `--recursive`: https://doc.rust-lang.org/cargo/commands/cargo-update.html. Builds create a missing lock and do not otherwise advance entries: https://doc.rust-lang.org/cargo/guide/cargo-toml-vs-cargo-lock.html (page does not describe the Cargo.toml-edit case in detail).
- Axis 3: per entry, by re-resolving with the old lock as the starting point (maintainer quote in cargo#5529: "when Cargo reads a lock file it'll re-resolve ... conservative update ... try to not tamper with the rest of the dependency graph"). `--locked` turns any would-be change into an error: https://doc.rust-lang.org/cargo/commands/cargo-update.html
- Axis 4: kept unless the spec cannot move without them.
- Axis 5 (depth): "Cargo locks the commits of `git` dependencies in `Cargo.lock` file at the time of their addition and checks for updates only when you run `cargo update` command." A `branch = "next"` dependency does not float per build: https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html. `--precise` accepts a git SHA or tag.
- Axis 6: `cargo update` prints Updating / Adding / Removing / Locking lines; `--dry-run` previews: https://doc.rust-lang.org/cargo/commands/cargo-update.html
- Axis 7: rust-lang/cargo#5529 (2018, open) "`cargo update -p` can silently downgrade dependencies of other packages when multi-major ranges are used" **[re-verify]**: https://github.com/rust-lang/cargo/issues/5529. rust-lang/cargo#8468 (2020, closed) "`cargo build` updates the git commit" (regression, git dep re-resolved every build) **[re-verify]**: https://github.com/rust-lang/cargo/issues/8468. crates.io#1605: one Cargo.toml edit changed many entries because a locked version had been yanked (search snippet) **[re-verify]**: https://github.com/rust-lang/crates.io/pull/1605

### npm (package-lock.json)

- Axis 1: `resolved` (URL) and `integrity` (sha512/sha1 SRI) per package: https://docs.npmjs.com/cli/v10/configuring-npm/package-lock-json. Git commit-ish handling is documented for `package.json` only, not for the lock: https://docs.npmjs.com/cli/v10/commands/npm-install
- Axis 2: `npm install` is documented as writing the lock on any operation that modifies `node_modules` or `package.json`; docs do not say whether a still-satisfied locked version is kept (they say the lock exists so "subsequent installs are able to generate identical trees, regardless of intermediate dependency updates"). `npm ci` never writes.
- Axis 3: `npm ci` is whole-file: "`npm ci` will exit with an error, instead of updating the package lock": https://docs.npmjs.com/cli/v10/commands/npm-ci
- Axis 4, 5, 6: not described in the fetched pages.
- Axis 7: npm/cli#6787 (fresh `npm install` produced a lock `npm ci` rejects; second install changes it again): https://github.com/npm/cli/issues/6787. npm/npm#17979 (2017) "installs kept updating the lockfile to newer releases" under ranges **[re-verify]**: https://github.com/npm/npm/issues/17979. Both from a search snippet, not read in full.

### pnpm

- Axis 1/3: `--frozen-lockfile` (default true in CI when a lockfile exists): "pnpm doesn't generate a lockfile and fails to install if the lockfile is out of sync with the manifest". `--prefer-frozen-lockfile` reuses the lock without re-resolving even if the manifest may have changed; `--no-prefer-frozen-lockfile` always re-resolves against the registry: https://pnpm.io/cli/install
- Axis 2/4: in workspaces, unchanged direct deps in other projects keep locked versions if they still satisfy the ranges; `pnpm update` requests newer. The page does not say whether a non-frozen install re-resolves only changed deps or the whole graph.
- Axis 7: none found.

### Yarn Berry

- Axis 2/3: `--immutable` (default true on CI) aborts if the lockfile "was to be modified"; `--refresh-lockfile` keeps the same resolution for lock packages while refreshing metadata; `--mode update-lockfile` only fetches packages missing from the lockfile (used by Renovate/Dependabot): https://yarnpkg.com/cli/install
- Axis 2: `yarn up <pkg>` upgrades matching packages "to their latest available version across the whole project", ignoring manifest ranges unless `-R`: https://yarnpkg.com/cli/up. Whether unnamed packages' resolutions are kept is not stated there.
- Axis 5: the git protocol page documents `#tag=`, `#commit=`, `#head=` selectors but not how the resolved commit is stored or re-resolved: https://yarnpkg.com/protocol/git. Not verified. A 2017 Yarn 1 issue reports a branch-pinned git dep installing at a commit different from the lock (search snippet) **[re-verify]**: https://www.goodfirstissue.org/yarnpkg/yarn/issues/4722

### uv (uv.lock)

- Axis 1: exact versions, resolution markers, a schema `version` and `revision`; timestamp included when a cooldown is used: https://docs.astral.sh/uv/concepts/resolution/
- Axis 2: "uv will prefer the previously locked versions of packages when running `uv sync` and `uv lock`." A locked version changes only if constraints exclude it. `--upgrade` (all), `--upgrade-package <pkg>[==ver]` (one): https://docs.astral.sh/uv/concepts/projects/sync/
- Axis 3: staleness is checked against project metadata. Adding a dependency or excluding the locked version makes the lock outdated; if the locked version still satisfies a changed constraint the lock stays current; new upstream releases never make it outdated. `--locked` errors; `--frozen` skips the check; `uv lock --check`. Same sync page.
- Axis 5: the sync page states that for Git dependencies uv keeps the locked commit SHA unless `--upgrade` / `--upgrade-package` is used (reported by the fetch tool as a paraphrase; the exact sentence was not quoted). The dependencies page documents `tag`/`branch`/`rev` but does not say how branches are re-resolved: https://docs.astral.sh/uv/concepts/projects/dependencies/
- Axis 7: no specific incident found. uv#14466 (open, 2025-07) asks how to pin to the latest version in the lock: https://github.com/astral-sh/uv/issues/14466

### pip-tools (pip-compile)

- Axis 1: the output `requirements.txt` holds pins (plus `# via` annotations); no separate request record.
- Axis 2/3: "reads any existing output file ... and uses the versions specified there as constraints"; if it still satisfies the inputs nothing changes. `--upgrade` re-resolves all, `--upgrade-package` named only. On conflict it discards the conflicting pins ("Discarding foo=1.2.3 to proceed"): https://pip-tools.pypa.io/en/stable/
- Axis 7: none searched in depth.

### Poetry

- Axis 1: per package version, source and hashes; `[metadata] content-hash`, a hash of the relevant `pyproject.toml` fields only (dependencies, source, extras, dev-dependencies, group; for PEP 621 `requires-python`, `dependencies`, `optional-dependencies`): https://github.com/python-poetry/poetry/blob/main/src/poetry/packages/locker.py
- Axis 2: since **2.0.0 (2025-01-05)** `poetry lock` defaults to the old `--no-update`; `--regenerate` is the old default: "Change the default behavior of `poetry lock` to `--no-update` and introduce a `--regenerate` option for the old default behavior" (#9327): https://github.com/python-poetry/poetry/blob/main/CHANGELOG.md ; announcement: "to prevent accidental updates of the lock file": https://python-poetry.org/blog/announcing-poetry-2.0.0/ . Docs: "By default, packages that have already been added to the lock file before will not be updated": https://python-poetry.org/docs/cli/#lock
- Axis 3: whole-file hash over relevant keys; on mismatch `poetry install` warns/errors and `poetry lock` is the fix. A PR comment notes the message implies a whole-file hash though only relevant fields are hashed (python-poetry/poetry#8737, search snippet): https://github.com/python-poetry/poetry/pull/8737
- Axis 7: before 2.0 the default re-resolved everything and `--no-update` was the opt-out; repo history shows many `--no-update` bugs (dropped dependencies, dup hashes, extras updated; changelog lines for #6335, #6389, #4618). python-poetry/poetry#10164 (open PR): `--no-update` now errors for v1/v2 shared scripts. The 2021 issue #4288 "`lock --no-update` still updates dependencies" is a no-lockfile-yet case, not a bug **[re-verify]**: https://github.com/python-poetry/poetry/issues/4288

### Go modules (go.mod / go.sum)

- Axis 1: `go.mod` is both requirement and pin (minimum versions); `go.sum` holds content hashes only. There is no separate lock: https://go.dev/ref/mod
- Axis 2: build commands default to `-mod=readonly` and error rather than edit `go.mod`; `go get` is the command that changes versions; `-u` upgrades; "MVS is deterministic, and the build list doesn't change when new versions of dependencies are released": https://go.dev/ref/mod
- Axis 3/4: with MVS there is no re-resolution step; adding a module may raise others only as required by that module's own requirements.
- Axis 5: a branch or commit given on the command line is translated into a pseudo-version (`v0.0.0-<time>-<sha>`) and recorded; "a non-canonical version like `master`" may not appear in a `go.mod` outside the main module: https://go.dev/ref/mod
- Axis 7: not searched. Only the first ~100k characters of the reference were read.

### Nix flakes (flake.lock)

- Axis 1: **both** `original` ("the original input specification from `flake.nix`") and `locked` (including `narHash` and e.g. `lastModified`, `revCount`) are stored per input: https://nix.dev/manual/nix/latest/command-ref/new-cli/nix3-flake.html
- Axis 2: `nix flake lock` only fills missing entries and "will never update inputs already in the lock file"; `nix flake update` updates all inputs by default, or the named inputs; `--update-input` is deprecated: https://nix.dev/manual/nix/latest/command-ref/new-cli/nix3-flake-lock.html ; https://nix.dev/manual/nix/latest/command-ref/new-cli/nix3-flake-update
- Axis 3: **per entry**. In the lock computation, an old entry is kept if `oldLock->originalRef.canonicalize() == input.ref->canonicalize()` and the parent path matches and there is no CLI override; otherwise it is "creating new input" (fetched and relocked). Source: https://github.com/NixOS/nix/blob/master/src/libflake/flake.cc (`computeLocks`, around lines 604-700 on master at 2026-11-03).
- Axis 4: unchanged parent: the sub-inputs are taken from the old lock entry (the flake is not even fetched unless an update was requested under it); only new overrides are checked. Same source.
- Axis 5 (depth): branch/tag refs are in `original`, the resolved rev in `locked`; the entry is reused as long as `original` is unchanged, so a moved branch is ignored until `nix flake update <input>`. In pure mode an unlocked input cannot be relocked ("cannot update unlocked flake input"). Same source.
- Axis 6: when `newLockFile != oldLockFile` Nix prints a diff of the lock file (`LockFile::diff`); `--no-update-lock-file` makes a required change an error. Same source.
- Axis 7: NixOS/nix#5112 (2021, open) `nix build` still writes flake.lock for missing inputs; reporter wants every lock-writing operation explicit **[re-verify]**: https://github.com/NixOS/nix/issues/5112 . NixOS/nix#8637 (2023, open) asks for an offline "is the lock in sync with inputs" check; `--no-update-lock-file` still fetches **[re-verify]**: https://github.com/NixOS/nix/issues/8637

### Helm (Chart.lock)

- Axis 1: per dependency `name`, `repository`, resolved `version`; top-level `digest` = hash over the Chart.yaml dependency list plus the locked dependencies. The chart (OCI manifest) digest is **not** recorded. Source: https://github.com/helm/helm/blob/main/internal/resolver/resolver.go and https://github.com/helm/helm/blob/main/pkg/downloader/manager.go
- Axis 2: `helm dependency build` "rebuild[s] the charts/ directory based on the Chart.lock file" and "will not re-negotiate dependencies, as 'helm dependency update' does"; with no lock it behaves like `update`: https://helm.sh/docs/helm/helm_dependency_build/ . `update` re-resolves the whole list.
- Axis 3: **whole-file**. `build` recomputes the digest from Chart.yaml and, on mismatch, fails: "the lock file (Chart.lock) is out of sync with the dependencies file (Chart.yaml). Please update the dependencies with 'helm dependency update'". `update` then re-resolves every dependency (`Resolve(req)` runs constraint matching per dep against the repo/tag list) with no per-entry reuse. Source: manager.go above.
- Axis 5 (depth, OCI): for OCI repositories the resolver lists tags and picks the first that satisfies the semver constraint; the lock stores a version, i.e. a mutable tag. helm/community PR #441 (HIP, open draft, 2026-06) proposes a per-dependency `digest` field: "Chart dependencies in Chart.yaml can only be specified by version, which corresponds to a mutable tag"; maintainers note HIP-0012 limits new Chart.yaml fields and that embedding the digest in `version` would break compatibility: https://github.com/helm/community/pull/441
- Axis 7: helm/helm#11308 (opened 2022-09, still open) "`helm dependency update` should have an option to skip upgrades of existing dependencies": adding one dependency forces an upgrade of every ranged one **[re-verify]**: https://github.com/helm/helm/issues/11308 . helm/helm#11750 (open) `build` out-of-sync error: https://github.com/helm/helm/issues/11750 . helm/helm#32581 (open) `dep update` ignores semver ordering in OCI subcharts: https://github.com/helm/helm/issues/32581

### Bundler (Gemfile.lock)

- Axis 2/3: if the Gemfile is unchanged, `bundle install` uses the lock and does not resolve. If the Gemfile changed, Bundler "will use the dependencies in the `Gemfile.lock` for all gems that you did not update, but will re-resolve the dependencies of gems that you did update". "CONSERVATIVE UPDATING: ... Bundler will update only the gems that you modified": https://github.com/rubygems/rubygems/blob/master/lib/bundler/man/bundle-install.1.ronn
- Axis 2: `bundle update <gem>` updates the named gem; `--conservative` also blocks indirect dependency updates; `--patch/--minor/--major` bound the jump; `bundle update --source <name>` updates a `:git`/`:path` source; `bundle update --all` ignores the lock: https://github.com/rubygems/rubygems/blob/master/lib/bundler/man/bundle-update.1.ronn
- Axis 1: the `DEPENDENCIES` section of Gemfile.lock records the Gemfile requirement (this is the general-knowledge basis for the per-entry check; not confirmed from a fetched page).
- Axis 5, 6, 7: not covered by the fetched man pages.

### SwiftPM (Package.resolved)

- Axis 1: per pin, branch name (or null), exact revision, version when present (sample lockfile, search snippet): https://chromium.googlesource.com/external/github.com/material-components/material-components-ios/+/v55.5.0/scripts/github-comment/Package.resolved
- Axis 2/3: `resolve` "resolves to those versions as long as they are still eligible"; "In most cases, the resolve command performs no changes unless the `Package.swift` manifest or `Package.resolved` file has changed"; `update` moves to latest eligible; `--force-resolved-versions` uses the file as-is: https://github.com/swiftlang/swift-package-manager/blob/main/Sources/PackageManagerDocs/Documentation.docc/ResolvingPackageVersions.md
- Axis 4: the lock only pins the root (leaf) package; a dependency's own Package.resolved is ignored (same page).
- Axis 5: forum posts: branch deps are suitable mainly in development because pins are ignored by clients, and an old bug ignored the recorded SHA for branch deps on a from-scratch resolve **[re-verify]**: https://forums.swift.org/t/about-reproducible-builds/26957
- Axis 7: forum thread (2021) on Package.resolved reverting after switching git branches because pins no longer satisfying the manifest are discarded **[re-verify]**: https://forums.swift.org/t/package-resolved-being-reset-after-checking-out-a-branch-with-updates/48634 ; swift-package-manager#4709 `resolve` overwriting a newer Package.resolved **[re-verify]**: https://github.com/apple/swift-package-manager/issues/4709

### Terraform (.terraform.lock.hcl)

- Axis 1: `version` (exact), `constraints` (the requirement as last seen), `hashes`. "Terraform doesn't actually use [constraints] to make installation decisions" — it is informational: https://developer.hashicorp.com/terraform/language/files/dependency-lock
- Axis 2: `terraform init` with an existing entry "reselects the recorded version, even if a newer one exists"; adds entries for new providers; removes unused ones; `init -upgrade` ignores existing selections. Prints a notice when it changes the file. Same page.
- Axis 3: per provider: a recorded version that no longer meets a changed constraint is an error at init that tells you to run `-upgrade` (not read from the fetched page; unverified).
- Axis 5: module sources (git refs, registry modules) are not covered by the provider lock (general knowledge, unverified).

### Deno (deno.lock)

- Axis 1: the lock maps each specifier as written (key like `npm:chalk@^5.6.2`) to one exact version, with an integrity hash per package; additive by default (new deps recorded without complaint, removed ones may linger): https://docs.deno.com/examples/tutorials/dependency_lockfile.md (search snippet, not fetched in full).
- Axis 2/3: `--frozen` (also `"lock": {"frozen": true}`) fails with a would-be diff; `deno install --entrypoint` re-records; `deno install --frozen-lockfile` "Error out if lockfile is out of date": https://docs.deno.com/runtime/reference/cli/install/
- Axis 7: frozen failures such as `Cannot add new entry for "jsr:@scope/pkg@1.3.0"` when a transitive import was never recorded (search snippet).

### OCI-native tools with a lock or pin file

- **Carvel kbld / imgpkg:** `--imgpkg-lock-output` writes an ImagesLock; each entry has the digest as `image` and the original tag in annotation `kbld.carvel.dev/id`; `--lock-output` writes overrides mapping the original tag to `newImage` digest with `preresolved: true`. Feeding the lock back via `-f` "produces same resolved configuration": https://carvel.dev/kbld/docs/develop/resolving/ . Records the requested reference next to the digest.
- **ToolHive skills lockfile (`toolhive.lock.yaml`):** `Source` = reference as originally requested, "never rewritten; upgrade re-resolves this value to check for newer content"; `ResolvedReference`; `Digest` (OCI sha256 manifest digest or git commit); `ContentDigest`; `RequiredBy` and `Explicit` track dependencies; `Update` is atomic under a file lock: https://pkg.go.dev/github.com/stacklok/toolhive@v0.43.0/pkg/skills/lockfile . The page does not say what a plain install does when the lock entry exists.
- **Dagger (`.dagger/lock`):** lock-aware calls are `container.from` (image ref + platform), `git.head/branch/tag/ref`. Each entry stores a policy: `container.from` and `git.tag` default to `pin`; `git.head` and `git.branch` default to `float`. Modes: `disabled`, `live` (resolve live and rewrite), `pinned` (reuse pinned, resolve floating live, record), `frozen` (lock only, fail on miss). `dagger lock update` refreshes recorded entries: https://docs.dagger.io/next/reference/cli/lockfiles . Page does not say whether the requested ref is stored beside the digest.
- **rules_oci `oci_pull` / Bazel:** exactly one of `tag`/`digest`; a tag build prints a warning with recommended digest/platform values; `reproducible` silences it: https://github.com/bazel-contrib/rules_oci/blob/main/docs/pull.md . Whether a tag resolution is stored in `MODULE.bazel.lock` was not confirmed (Bazel lockfile docs: https://bazel.build/versions/6.5.0/external/lockfile, 2023 **[re-verify]**).
- **Third-party docker-lock / dockerfile-pin:** `generate` / `verify` / `rewrite` for Dockerfiles and Compose files (search snippet): https://pkg.go.dev/github.com/dockerlocker/docker-lock . Not examined.

## Cross-cutting patterns

| Tool | Lock records the request? | Edit detected | Other entries on a mismatch | Unchanged parent's transitive | Moved mutable ref, still requested | Explicit-only movers |
|---|---|---|---|---|---|---|
| OCX | No tag; repository + per-platform digests; whole-file `declaration_hash` | whole-file hash | refuse (65); `lock` then re-resolves **all** tags; `add`/`remove` carry the rest verbatim | package deps pinned by publisher metadata | invisible until `update` | `update`, `upgrade`, drifted `lock` |
| Cargo | Source (+`?branch=` for git); not the requirement | per entry, via re-resolve from old lock | stay unless the spec needs them | stay | commit locked at addition; moves on `cargo update` | `cargo update` |
| npm | `resolved` + `integrity` | `npm ci` whole-file refuse | `npm install` unspecified in docs | not documented | not documented | not documented |
| pnpm | not in fetched docs | frozen: whole-file refuse | prefer-frozen: reuse; no-prefer: re-resolve | workspace: unchanged keep | not documented | `pnpm update` |
| Yarn Berry | not in fetched docs | `--immutable` refuse | `update-lockfile`: fetch only missing | not documented | not documented | `yarn up` |
| uv | versions + markers; requirement compared against project metadata | per entry (locked version still satisfies?) | stay | stay | git SHA kept unless upgrade | `--upgrade`, `--upgrade-package` |
| pip-tools | pins in output | per entry | pins as constraints; conflicting ones discarded with warning | pinned | n/a | `-U`, `-P` |
| Poetry | version/hash; `content-hash` over relevant fields | whole-file hash | 2.x: existing pins kept by default; `--regenerate` for all | kept | not covered | `poetry update`, `--regenerate` |
| Go | `go.mod` is the pin | n/a (MVS) | n/a | n/a | branch becomes pseudo-version | `go get [-u]` |
| Nix | `original` **and** `locked` | per entry (`original` comparison) | kept | lazily taken from old entry | rev kept until `flake update <input>` | `nix flake update [input]` |
| Helm | resolved version; digest over request list + locked | whole-file hash | `build` refuses; `update` re-resolves all | direct deps only | tag has no digest (HIP #441 open) | `helm dependency update` |
| Bundler | `DEPENDENCIES` section (requirement) | per gem | kept ("conservative updating") | kept | `--source` | `bundle update [gem]` |
| SwiftPM | branch + revision + version | per pin eligibility | kept if still eligible | root only | revision kept | `package update` |
| Terraform | exact version + `constraints` (informational) | per provider | kept | n/a | n/a | `init -upgrade` |
| Deno | specifier verbatim as key to exact version | `--frozen` refuse | additive | recorded | not covered | re-run install |
| kbld / ImagesLock | original tag + digest | n/a (re-run) | n/a | n/a | digest from lock | re-run kbld |
| ToolHive | `Source` verbatim + digest | not documented | not documented | `RequiredBy` tracked | `upgrade` re-resolves `Source` | `upgrade` |
| Dagger | policy `pin`/`float` per entry | n/a | n/a | n/a | `pin`: reused; `float`: live under `pinned` | `lock update`, `--lock=live` |

Evidence for and against, by approach (neutral):

- *Keep entry while requirement satisfied (per entry).* For: no surprise movement when adding a dependency (Cargo, uv, Bundler docs; Helm users request exactly this in #11308). Against: "satisfied" must be definable. A tag has no ordering, so for a mutable tag the only decidable form is request equality (Nix `original`, kbld annotation, ToolHive `Source`) or whole-file hash (OCX, Helm). Cargo#5529 shows even the conservative path can move other entries when the graph forces it.
- *Whole-file hash plus refuse.* For: cheap, offline CI gate; no silent drift (OCX `lock --check`, Helm `build`, `uv --locked`). Against: any hash mismatch gives no information about which entry changed; the follow-up reconcile command (OCX `lock`, Helm `update`) re-resolves everything, so a one-line edit can advance every moving tag.
- *Record request beside resolution.* For: enables per-entry change detection and a readable diff (Nix, kbld, ToolHive); costs a larger lock and a canonicalization of the request. OCX and Helm do not store it and instead rely on a whole-file hash.
- *Digest-only for mutable refs.* Helm and `oci_pull` tags show the cost when the lock stores no digest: the reference can move without the lock noticing, and Helm has an open HIP to add one.

## negative:

- OCX GitHub issues: no public report of an unexpected upgrade or a stale-pin complaint was found; `gh search issues "lock"` returned only unrelated items. The OCX behaviours above are from its docs and source, not incident reports.
- npm, pnpm, Yarn Berry, Deno, Terraform: the fetched pages did not state how a still-satisfied lock entry is treated on a non-frozen install, or how git branch pins are re-resolved; those cells in the table are "not documented", not "no".
- Go, uv git-branch handling, Yarn git: only partial doc text; not verified from source code.
- Terraform module sources, Bundler `DEPENDENCIES` content, Cargo branch-edit behavior: general knowledge or search snippets only.
- Zarf, Docker Compose / Buildx lockfile proposals, Bazel `MODULE.bazel.lock` handling of `oci.pull` tags: a search found no Compose or Buildx lockfile proposal and nothing confirming Bazel records tag resolutions. docker-lock was only seen in a snippet. Not covered.
- A search for "poetry lock updates everything unexpectedly" returned mostly the no-lockfile-yet case (#4288) and Dependabot bugs; not counted as evidence.
- The WebFetch tool summarized pages with a small model: wording marked as quotes in some places (uv Git SHA retention, pnpm workspace behavior) was paraphrase and may differ from the primary text.

## leads:

- `ocx` source `crates/ocx_project/src/resolve.rs`: how `resolve_lock_touched` decides "touched" and what `is_current` hashes; useful to compare with a per-entry request record.
- Nix `LockFile::diff` output format: a concrete model for "added / changed / unchanged" lock reporting on a write.
- Helm HIP-0012 and community PR #441 discussion: the compatibility constraints on adding a digest field to an existing lock format.
- Dagger lock policy source (`pin` vs `float` defaults): how a per-entry policy is stored and reported, relevant to tags vs moving tags.
- Renovate / Dependabot lock-maintenance modes (Yarn `update-lockfile`, pnpm `--fix-lockfile`, Poetry lock-only): how automation expects a lock to behave after a manifest edit.
- Cargo `Cargo.lock` source-string format for `?branch=` / `?tag=` / `#sha`: confirm whether editing the selector rewrites the entry.

## Sources

- https://github.com/ocx-sh/ocx/blob/main/ocx.lock — OCX's own lockfile format (recorded fields) — read 2026-11-03
- https://github.com/ocx-sh/ocx/blob/main/crates/ocx_cli/src/command/lock.rs — `ocx lock` carry-forward vs re-resolve-all logic — 2026-11-03
- https://github.com/ocx-sh/ocx/blob/main/website/src/content/docs/guides/update-tools.mdx — OCX lock/update/upgrade semantics and reports — 2026-11-03
- https://github.com/ocx-sh/ocx/blob/main/website/src/content/docs/guides/lock-and-reproduce.mdx — read paths, `--frozen`, mutator list — 2026-11-03
- https://github.com/ocx-sh/ocx/blob/main/website/src/content/docs/explanation/project-toolchain.md — pin preservation — 2026-11-03
- https://github.com/ocx-sh/ocx/blob/main/website/src/content/docs/explanation/dependencies.mdx — manifest pins, publisher-pinned deps — 2026-11-03
- https://github.com/ocx-sh/ocx/blob/main/.claude/artifacts/adr_scoped_upgrade.md — ADR for scoped upgrade — 2026-07-09
- https://doc.rust-lang.org/cargo/commands/cargo-update.html — conservative update, `--locked`, `--precise` — 2026-11-03
- https://doc.rust-lang.org/cargo/guide/cargo-toml-vs-cargo-lock.html — lock creation, git commit pinning — 2026-11-03
- https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html — git branch lock statement — 2026-11-03
- https://github.com/rust-lang/cargo/issues/5529 — `cargo update -p` downgrades others — 2018 **[re-verify]**
- https://github.com/rust-lang/cargo/issues/8468 — `cargo build` updates git commit — 2020 **[re-verify]**
- https://github.com/rust-lang/crates.io/pull/1605 — yanked crate forces lock changes (snippet) — **[re-verify]**
- https://internals.rust-lang.org/t/upcoming-changes-to-cargo-lock/14017 — `?branch=` lock encoding — 2021 **[re-verify]**
- https://docs.npmjs.com/cli/v10/configuring-npm/package-lock-json — npm lock fields — 2026-11-03
- https://docs.npmjs.com/cli/v10/commands/npm-install — npm install, git commit-ish — 2026-11-03
- https://docs.npmjs.com/cli/v10/commands/npm-ci — `npm ci` refuses on mismatch — 2026-11-03
- https://github.com/npm/cli/issues/6787 — install/ci lock churn (snippet) — 2023
- https://github.com/npm/npm/issues/17979 — ranges keep updating lock (snippet) — 2017 **[re-verify]**
- https://pnpm.io/cli/install — frozen / prefer-frozen — 2026-11-03
- https://yarnpkg.com/cli/install — immutable, refresh-lockfile, update-lockfile — 2026-11-03
- https://yarnpkg.com/cli/up — `yarn up` scope — 2026-11-03
- https://yarnpkg.com/protocol/git — git selectors — 2026-11-03
- https://www.goodfirstissue.org/yarnpkg/yarn/issues/4722 — Yarn 1 branch pin issue (snippet) — 2017 **[re-verify]**
- https://docs.astral.sh/uv/concepts/projects/sync/ — uv lock preferences, upgrade, locked/frozen — 2026-11-03
- https://docs.astral.sh/uv/concepts/resolution/ — preferences, lock schema — 2026-11-03
- https://docs.astral.sh/uv/concepts/projects/dependencies/ — git source selectors — 2026-11-03
- https://github.com/astral-sh/uv/issues/14466 — pin to latest in uv.lock — 2025-07
- https://pip-tools.pypa.io/en/stable/ — pip-compile pin reuse — 2026-11-03
- https://python-poetry.org/docs/cli/#lock — `--regenerate`, default — 2026-11-03
- https://python-poetry.org/blog/announcing-poetry-2.0.0/ — lock defaults to no-update — 2025-01 (about 22 months) **[re-verify]**
- https://github.com/python-poetry/poetry/blob/main/CHANGELOG.md — 2.0.0 entry, historic `--no-update` fixes — 2026-11-03
- https://github.com/python-poetry/poetry/blob/main/src/poetry/packages/locker.py — content-hash relevant keys — 2026-11-03
- https://github.com/python-poetry/poetry/pull/8737 — content-hash message discussion (snippet) — **[re-verify]**
- https://github.com/python-poetry/poetry/issues/4288 — `--no-update` report — 2021 **[re-verify]**
- https://go.dev/ref/mod — Go MVS, `go get`, pseudo-versions (first 100k chars) — 2026-11-03
- https://nix.dev/manual/nix/latest/command-ref/new-cli/nix3-flake.html — `original` vs `locked` — 2026-11-03
- https://nix.dev/manual/nix/latest/command-ref/new-cli/nix3-flake-lock.html — `flake lock` fills only missing entries — 2026-11-03
- https://nix.dev/manual/nix/latest/command-ref/new-cli/nix3-flake-update — `flake update` — 2026-11-03
- https://github.com/NixOS/nix/blob/master/src/libflake/flake.cc — per-entry `original` comparison, diff on write — master, 2026-11-03
- https://github.com/NixOS/nix/issues/5112 — `nix build` writes lock — 2021 **[re-verify]**
- https://github.com/NixOS/nix/issues/8637 — offline lock-sync check — 2023 **[re-verify]**
- https://helm.sh/docs/helm/helm_dependency_build/ — build vs update — 2026-11-03
- https://github.com/helm/helm/blob/main/pkg/downloader/manager.go — digest check, out-of-sync error — main, 2026-11-03
- https://github.com/helm/helm/blob/main/internal/resolver/resolver.go — constraint resolution, OCI tag listing — main, 2026-11-03
- https://github.com/helm/community/pull/441 — HIP: digest in Chart.lock — 2026-06 (open)
- https://github.com/helm/helm/issues/11308 — update should skip existing deps — 2022 **[re-verify]**
- https://github.com/helm/helm/issues/11750 — build out-of-sync — 2025-04 (about 19 months) **[re-verify]**
- https://github.com/helm/helm/issues/32581 — OCI subchart semver ordering — 2026-08
- https://github.com/rubygems/rubygems/blob/master/lib/bundler/man/bundle-install.1.ronn — conservative updating — 2026-11-03
- https://github.com/rubygems/rubygems/blob/master/lib/bundler/man/bundle-update.1.ronn — update flags — 2026-11-03
- https://github.com/swiftlang/swift-package-manager/blob/main/Sources/PackageManagerDocs/Documentation.docc/ResolvingPackageVersions.md — resolve vs update — 2026-11-03
- https://forums.swift.org/t/about-reproducible-builds/26957 — branch deps — 2019 **[re-verify]**
- https://forums.swift.org/t/package-resolved-being-reset-after-checking-out-a-branch-with-updates/48634 — pin discarded when unsatisfied — 2021 **[re-verify]**
- https://github.com/apple/swift-package-manager/issues/4709 — resolve overwrites Package.resolved — **[re-verify]**
- https://developer.hashicorp.com/terraform/language/files/dependency-lock — Terraform lock semantics — 2026-11-03
- https://docs.deno.com/runtime/reference/cli/install/ — frozen-lockfile, lockfile-only — 2026-11-03
- https://docs.deno.com/examples/tutorials/dependency_lockfile.md — deno.lock recording (snippet) — 2026
- https://carvel.dev/kbld/docs/develop/resolving/ — kbld lock outputs — 2026-11-03
- https://pkg.go.dev/github.com/stacklok/toolhive@v0.43.0/pkg/skills/lockfile — ToolHive skills lockfile — 2026-11-03
- https://docs.dagger.io/next/reference/cli/lockfiles — Dagger lock policies/modes — 2026-11-03
- https://github.com/bazel-contrib/rules_oci/blob/main/docs/pull.md — `oci_pull` tag vs digest (snippet)
- https://bazel.build/versions/6.5.0/external/lockfile — Bazel lockfile — 2023 **[re-verify]**
- https://pkg.go.dev/github.com/dockerlocker/docker-lock — third-party docker-lock (snippet only)

## Addendum: OCX lock v4 (orchestrator, 2026-11-03)

The OCX section above describes `ocx.lock` v3, which is what the public sources showed. The local ocx checkout (`~/dev/ocx-sh/ocx`) has a newer commit that replaces it: `7378fe0b feat(lock)!: ocx lock re-resolves only the tools whose ocx.toml entry changed` (2026-11-02).

- `lock_version = 4`: each `[[tool]]` records `declared`, the full `ocx.toml` spelling the entry was resolved from (`crates/ocx_project/src/lock.rs`).
- `resolve_lock_reconcile` (`crates/ocx_project/src/resolve.rs`) treats a binding as **changed** when it has no predecessor entry, names another repository, or its `declared` spelling differs. Only changed bindings and the `forced` ones (`update NAME`) are resolved; every other entry is carried forward byte-identical, **even when its tag moved**.
- A clean v3 lock migrates to v4 without contacting the registry. A drifted v3 lock (no `declared` to compare) resolves everything once.
- An older OCX refuses a v4 lock (lock_version gate). Read-only commands never rewrite a v3 lock.
- Bare `ocx update` on a drifted lock re-resolves the default group plus the changed bindings.
- The docs table now reads: `ocx lock` — "Only the bindings whose `ocx.toml` entry is new or changed; every other pin carries forward, even a moved tag."

So OCX moved from family 2/3 (whole-file hash, whole-file re-resolve on drift) to family 4 (per-entry comparison of the recorded request), the Nix flake `original`/`locked` model.
