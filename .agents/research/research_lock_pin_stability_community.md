# Research: Lock pin stability — community threads

## Metadata

**Date:** 2026-11-03
**Domain:** packaging
**Triggered by:** /hex-discuss lock-pin-stability (.agents/discussions/lock-pin-stability.md)
**Expires:** 2027-05-03

Scope: practitioner reports (GitHub issues, forum threads, vendor incident write-ups) on lock regeneration, add, and scoped update, with mutable references. Complements `research_lock_pin_stability_prior_art.md` (official docs); items already there are only cross-referenced. Evidence is neutral; no recommendation. Items older than ~18 months (before 2025-05) are marked **[re-verify]**. Most thread content here comes from WebSearch result summaries; only helm#11308, cargo#8555 and npm/cli#8667 were fetched and read (via a summarizing fetch tool, so quotes may be paraphrase). Everything else is "(snippet)" and unverified against the primary page.

## Direct answer (short)

Practitioner threads show two opposite complaint families and one shared remedy request.
(a) Surprise movement: a command meant to add or tweak one entry moves others. Helm `dependency update` is the clearest (open since 2022); Poetry before 2.0 changed its default for this reason; Bundler's default `bundle update <gem>` also moves shared dependencies, which `--conservative` was added to stop.
(b) Surprise staleness: a moved branch or tag is not picked up. Cargo git-branch deps (#8555), Nix inputs, Terraform providers, and Renovate's digest lookups all produced reports. In each, maintainers answered that this is intended and only an explicit update command moves the pin; the fix was usually documentation or a clearer error.
The shared request is a separate, named operation for "move this one entry" and for "add without moving the rest". Several maintainers resisted changing existing command behavior because of compatibility (Helm HIP-0004).
Supply-chain incidents (tj-actions 2025, Trivy 2026, Codecov 2021) are cases where a mutable reference was silently re-resolved to attacker content. They are cited for pinning to an immutable identifier, not for lock semantics as such.

## Findings by theme

### Theme 1 — Commands that unexpectedly moved other entries

| Tool | Behavior | Expectation | Response / resolution | URL |
|---|---|---|---|---|
| Helm `dependency update` | Adding one dependency to Chart.yaml re-resolves all ranged ones (e.g. `1.x`) | Add without upgrading existing; also switch pinned to a range without upgrading | Opened 2022-09-08 on v3.9.4. Contributor joejulian: existing behavior can't change (breaks workflows; HIP 0004 governs); proposed opt-in `--only-new`; reporter proposed naming one dependency, PR #11362 opened 2022-09-19. Stale-bot 2022-12, relabeled `in progress`. Users still asking in 2024-05 and 2024-08. Open at last read; PR outcome not shown on page. **[re-verify]** | https://github.com/helm/helm/issues/11308 |
| Poetry `lock` (pre-2.0) | Default re-resolved everything; `--no-update` was the opt-out | Lock should not upgrade existing pins | 2.0.0 (2025-01) flipped the default ("to prevent accidental updates of the lock file", per prior-art lane); `--regenerate` is the old behavior. Downstream projects had to drop `--no-update` from CI (codeflare-sdk commit, snippet); users on old guides hit `The option "--no-update" does not exist` on 2.2.1 (AOSSIE-Org/Devr.AI#176, snippet). Note: it was a default change that broke scripts, accepted at a major version | https://github.com/python-poetry/poetry/releases ; https://github.com/AOSSIE-Org/Devr.AI/issues/176 |
| Poetry `add` / `update` / `remove` (2.x) | Wrong handling of packages in other groups; extra packages uninstalled; `add --optional` lock not updated | Scoped command touches only its target | Fixed in 2.1.2 (#10014, #10016, #10076) per changelog (snippet) | https://github.com/python-poetry/poetry/blob/main/CHANGELOG.md |
| Poetry + Dependabot | Dependabot bump of one package removed marker-bearing `[package.dependencies]` entries from poetry.lock | Single-package PR changes that package only | Discussion #7116; commenter: fix was in poetry-core and not yet in a released Poetry (snippet). Dated 2022-23 **[re-verify]** | https://github.com/python-poetry/poetry/discussions/7116 |
| Poetry `update` | Dropped an explicit package source from lock | Lock keeps source metadata | Issue #8928, flagged duplicate of #8614; fix for fresh lock did not cover this path (snippet) **[re-verify]** | https://github.com/python-poetry/poetry/issues/8928 |
| npm `install` | Removes packages from lockfile when updating unrelated deps | Lockfile entries are not removed by unrelated updates | npm/cli#8667 opened and closed 2025-10-10 as a duplicate of #8464 (peer optional dependency); maintainer said "similar if not a duplicate" of #8516/#8464. Cause was a peer-optional-dependency bug, not a general policy | https://github.com/npm/cli/issues/8667 |
| npm 5-era | `npm install` kept rewriting or reordering the lock; git-dependency name mismatch caused churn and removed transitives on every install; reinstall against the same lock gave different trees | Same lock gives the same tree | npm/npm#17155, #16839, #19124 (2017-18), all snippet **[re-verify]** | https://github.com/npm/npm/issues/17155 ; https://github.com/npm/npm/issues/16839 ; https://github.com/npm/npm/issues/19124 |
| Cargo `update -p X --precise` | Also moves a related crate (tokio-executor moved along with tokio-threadpool) | Only X moves | #7671 (2019): `-p` alone was a silent no-op, `--precise` moved both. The cause (resolution constraint) is inferred from the snippet, not confirmed from the thread **[re-verify]** | https://github.com/rust-lang/cargo/issues/7671 |
| Cargo `--precise` without `-p` | Succeeded without doing what the user intended | Error on misuse | #10919; author suggested it should be an error (snippet) **[re-verify]** | https://github.com/rust-lang/cargo/issues/10919 |
| Go `go get` | Adds an indirect requirement taken from go.sum; plain `go get -u` moves indirect deps past go.mod versions | Only the named module changes | #26474 reframed as a documentation gap, workaround `go mod tidy`; #28424 proposes separate direct/indirect upgrade; #47979 test locks in that an argument may upgrade another existing root (snippets) **[re-verify]** | https://github.com/golang/go/issues/26474 ; https://github.com/golang/go/issues/28424 |
| Bundler `bundle update <gem>` | Default also updates all of the gem's dependencies, including those shared with other gems | Only the named gem moves | Bundler position (guides, snippet): for gems other than Rails, updating the dependencies is recommended; `--conservative` added in 1.14. Earlier request bundler#2016; undocumented `--source` workaround. Another guide advises reviewing `git diff Gemfile.lock` and reverting unwanted lines | https://github.com/bundler/bundler/issues/2016 ; https://guides.rubygems.org/updating_gems/ |
| Dependabot (any ecosystem) | A "targeted" security PR for a transitive dep looked like a full lockfile refresh | Targeted change | Renovate discussion #22747 (snippet): Renovate's `lockFileMaintenance` is whole-lock by design; Renovate dev docs warn the package tool's own update command may pick a newer version than Renovate intended | https://github.com/renovatebot/renovate/discussions/22747 |

### Theme 2 — Stale pin when a moved tag/branch was expected to be picked up

| Tool | Behavior | Expectation | Response / resolution | URL |
|---|---|---|---|---|
| Cargo git branch dep | Branch deps stay on the locked commit, even after `cargo clean` | Docs said Cargo "assumes the latest commit on master", so reporter expected newer commits | Issue #8555, 2020-07-28. ehuss: intended; "Cargo will not automatically fetch a new version... You will need to run `cargo update`." Closed 2020-07-29 by docs-only PR #8561 (commit 21a1e15). Follow-up 2024-07: `cargo update` is the answer. 2024-11: a user reports the problem persists after deleting Cargo.lock and `~/.cargo/git` (not diagnosed in the thread) | https://github.com/rust-lang/cargo/issues/8555 |
| Cargo git (opposite) | `cargo build` re-resolved a locked git commit | Build never moves a locked commit | #8468 (2020), regression traced to PR #8364 by reporter; status unchecked. **[re-verify]** | https://github.com/rust-lang/cargo/issues/8468 |
| Terraform providers | Changing a constraint without `init -upgrade` errors ("locked provider ... does not match configured version constraint"); plain `init` keeps the locked version | Constraint bump picks up the new version | HashiCorp Discuss and a GitLab CI issue (snippet): message tells user to run `terraform init -upgrade`; `-upgrade=hashicorp/aws` scopes to one provider; `-lockfile=readonly` makes CI fail instead of changing the file (third-party guides) | https://discuss.hashicorp.com/t/error-failed-to-query-available-provider-packages-locked-provider/40422 ; https://gitlab.com/gitlab-org/terraform-images/-/issues/29 |
| Terraform checksums | Fresh checkout hit a checksum mismatch; neither `-reconfigure` nor `-upgrade` fixed it | Lock works on every platform | #32124: workaround `terraform providers lock -platform=...` for several platforms (snippet) **[re-verify]** | https://github.com/hashicorp/terraform/issues/32124 |
| Nix flakes | `flake.lock` entry not refreshed when the input's declaration-related state changes (removing `flake = false`); `follows` leaves old rev in the nested lock; transitive inputs not updated by `flake lock` | Lock follows the edit | NixOS/nix#8511, #5785, Lix#8 (all snippets, no maintainer outcome read) **[re-verify]**. Documented: `nix flake lock` never updates existing entries; `nix flake update <input>` is the mover; Lix commit "Overhaul `nix flake update` and `nix flake lock` UX" (2024-03, snippet) | https://github.com/NixOS/nix/issues/8511 ; https://github.com/NixOS/nix/issues/5785 ; https://git.lix.systems/lix-project/lix/issues/8 |
| Renovate + Docker digests | Digest lookup for a mutable tag returned a stale digest and proposed it as an "update" over a newer one | Updates never move a pin to older content | Renovate discussion #45598 (snippet); reporter suggests refusing digest updates that regress. Renovate docs model: `image:tag@sha256:...` with the tag kept beside the digest; PRs opened when the tag's image changes | https://github.com/renovatebot/renovate/discussions/45598 ; https://docs.renovatebot.com/docker/ |
| Docker `latest` | Upstream pushed a broken `latest`; no tag to return to | Rollback by tag | liquibase/docker#13 (maintainers re-released); docker-redis-cluster#154: all Docker Hub tags updated, no way back, one maintainer re-pointed to an older digest (snippets) **[re-verify]** | https://github.com/liquibase/docker/issues/13 ; https://github.com/grokzen/docker-redis-cluster/issues/154 |

How tools surfaced "update available": Renovate/Chainguard Digestabot open PRs when a tag's digest changes (https://docs.renovatebot.com/docker/ ; https://edu.chainguard.dev/chainguard/containers/how-to-use/digestabot_frizbee/); Cargo/Nix/Terraform surface nothing, the user must run the update command (see prior-art for output formats).

### Theme 3 — Scoped updates touching unrelated entries

- Cargo `-p`: designed to be conservative, yet #5529 (2018, in prior-art lane) and #7671 show `--precise`/multi-major ranges can move others. Users run `--dry-run` to preview.
- Bundler: default `bundle update <gem>` is non-conservative; `--conservative`, `--patch/--minor/--major`, and `--source` exist because of the complaint; one guide recommends manual review of `git diff Gemfile.lock`.
- uv `--upgrade-package`: no report of unrelated entries moving was found. uv#17747 (`--upgrade-package` set makes `uv lock` then `uv sync --locked` report out-of-date after adding `opencv-python`) and uv#13551 (`uv lock` after version change did nothing; `uv sync` reverted the package) are lock-consistency reports (snippets). A third-party guide notes a single-package upgrade does not guarantee one changed entry because other constraints can force extras.
- Poetry 2.1.2: group-scoping bugs in `add`/`update`/`remove` (Theme 1).
- Helm: the request in #11308 is exactly a scoped update (name one dependency) which does not exist.
- Yarn / Dependabot: no Yarn thread was gathered in this lane.

### Theme 4 — Stated expectations of "lock" vs "update" vs "install", and default changes

- Cargo (#8555): "the lock file governs; `cargo update` moves it." Docs changed, behavior not. The reporter's confusion came from docs implying latest-commit semantics.
- Poetry 2.0: users and maintainers concluded `lock` should not upgrade; default flipped at a major version, with `--regenerate` for the old behavior. Breakage cost: removed flag errors in old scripts and guides.
- Helm: maintainers refused to change `update` semantics for compatibility (HIP 0004) and proposed opt-in flags instead; `build` is the "use the lock" command and fails on drift.
- npm: `npm ci` is the "respect the lock or fail" command. npm/cli#2701 (npm 7): `npm ci` ran a full install after a package.json major bump without a lock update; reporter expected a refusal; maintainer commit message "validate any lockfile against the current install so mismatches throw an error" appears in a fork, upstream landing not confirmed (snippet) **[re-verify]**.
- Terraform: `init` = "use the lock", `init -upgrade` = "re-resolve"; the failure message names the fix.
- Nix: `flake lock` = fill missing only; `flake update` = move; Lix reworked this UX (2024).
- Renovate: separates "update this dependency" from "lock file maintenance (refresh all within ranges)".
- Go: the build never edits go.mod (`-mod=readonly`); `go get` is the editor; users are surprised by additions to go.mod that come from go.sum (#26474).

### Theme 5 — Supply-chain incidents with silently re-resolved mutable references

| Incident | What moved | Notes | URL |
|---|---|---|---|
| tj-actions/changed-files, 2025-03 (CVE-2025-30066) | Existing version tags repointed to a malicious commit (one source: v1 through v45.0.7 to commit 0e58ed8) | Workflows pinned by tag ran attacker code and leaked CI secrets; reports cite 23,000+ repos using the action, one analysis ~218 actually exposed secrets (sources differ); mitigation advised everywhere: full-SHA pins | https://www.theregister.com/2025/03/17/supply_chain_attack_github/ ; https://threats.wiz.io/all-incidents/tj-actionschanged-files-supply-chain-attack |
| Trivy / trivy-action / setup-trivy, 2026-03-19 (CVE-2026-33634, GHSA-69fq-xp46-6x23) | 76 of 77 `trivy-action` tags (one outlet: 75) force-pushed; all 7 `setup-trivy` tags replaced; Docker Hub tags `0.69.5`, `0.69.6`, `latest` later pushed with compromised images (per a Docker statement quoted in a secondary source) | Maintainers republished fixed releases with a `v` prefix because old tags could not be recreated. Counts differ across outlets; CVE record is the stronger source. All via search snippets, not read | https://cve.circl.lu/cve/CVE-2026-33634 |
| Codecov Bash Uploader, 2021-01-31 to 2021-04-01 | Script served at `codecov.io/bash` (a mutable URL) was altered | Found when a customer compared the script's checksum with the published one; Codecov said the checksum check existed but was poorly documented and committed to GPG signing. Older than 18 months **[re-verify]** | https://about.codecov.io/apr-2021-post-mortem/ |
| Mutable Docker tags (general) | Tag content replaced | nodejs/docker-node#2461 asks for digest pinning in base images to prevent tag-mutability attacks (snippet); Renovate and Chainguard docs recommend `tag@digest` | https://github.com/nodejs/docker-node/issues/2461 |

Observation (not a recommendation): in all three named incidents the victims had pinned a human-readable name that the publisher, or an attacker holding the publisher's credentials, could re-point. The only cited mitigation is recording an immutable identifier (commit SHA, digest, checksum). The incident write-ups say nothing about lock-file semantics.

## negative:

- No thread was found where a Cargo, uv, Nix or Terraform user complained that an explicit `add`/`lock` command moved unrelated entries; their docs promise otherwise and complaints are about staleness. Absence from a few searches, not proof.
- No practitioner thread was found on `pnpm`, Yarn Berry, SwiftPM or Deno specifically.
- No incident was found where a *lock file* itself (as opposed to a bare tag reference) pulled in compromised content through silent re-resolution. The incidents above were unpinned references.
- No OCI-tool (Helm OCI, ORAS, kbld, imgpkg) user thread on tag-moved-but-lock-stale was found beyond Helm HIP #441 (prior-art).
- Poetry 2.0 rationale thread (#9327 discussion) was not found; the reason comes from the release announcement quoted in the prior-art lane.
- Hacker News and Reddit threads were not surfaced by the search tool in this pass; they are not represented. No Stack Overflow data was gathered.
- Most items are search-result summaries, not read pages. Detail beyond the quoted line (e.g. cause attributions) is inference by the summarizer.

## leads:

- helm/helm PR #11362 (named-dependency update): confirm merged, closed or abandoned, and why.
- helm/community HIP-0004 text: the compatibility rule that blocked changing `update`.
- Poetry PR #9327 and its linked issues: stated reasons for flipping the default.
- Bundler `--conservative` introduction (1.14) PR/issue thread and the `bundle update` default rationale from maintainers.
- Cargo #8555 follow-up 2024-11 (persisting after deleting Cargo.lock): check whether a separate issue was filed.
- Renovate discussion #45598 and the `pinDigests` design issue: how a stale/regressing digest is detected.
- Trivy GHSA-69fq-xp46-6x23 and the Docker statement on `latest`: primary account of which Docker Hub tags were repointed.
- GitHub "immutable releases" / Actions tag-immutability announcements after tj-actions (2025): how a platform responded.
- npm/cli#2701 upstream resolution commit.
- Nix: Lix "Overhaul `nix flake update` and `nix flake lock` UX" commit message for the stated motivation.

## Sources

- https://github.com/helm/helm/issues/11308 — add forces upgrades; scoped update request; maintainer compatibility stance — opened 2022-09, comments to 2024-08 (read) **[re-verify]**
- https://github.com/rust-lang/cargo/issues/8555 — git branch stays on locked commit; docs-only fix — 2020-07 (read) **[re-verify]**
- https://github.com/npm/cli/issues/8667 — npm removes lock entries on unrelated update; closed duplicate — 2025-10-10 (read)
- https://github.com/rust-lang/cargo/issues/7671 — `-p` no-op, `--precise` moves related crate — 2019 (snippet) **[re-verify]**
- https://github.com/rust-lang/cargo/issues/10919 — `--precise` without `-p` — 2022 (snippet) **[re-verify]**
- https://github.com/rust-lang/cargo/issues/8468 — `cargo build` rewrites locked git commit — 2020 (snippet) **[re-verify]**
- https://github.com/python-poetry/poetry/discussions/7116 — Dependabot single bump removes lock metadata — ~2022 (snippet) **[re-verify]**
- https://github.com/python-poetry/poetry/issues/8928 — `poetry update` drops explicit source — ~2023-24 (snippet) **[re-verify]**
- https://github.com/python-poetry/poetry/blob/main/CHANGELOG.md — 2.0.0 default flip; 2.1.2 group-scope fixes — snippet
- https://github.com/AOSSIE-Org/Devr.AI/issues/176 — `--no-update` errors on Poetry 2.2.1 — 2025 (snippet)
- https://github.com/npm/npm/issues/17155 , /16839 , /19124 — npm 5 lockfile churn — 2017-18 (snippets) **[re-verify]**
- https://github.com/npm/cli/issues/2701 — `npm ci` ran despite stale lock — 2021 (snippet) **[re-verify]**
- https://github.com/golang/go/issues/26474 , /28424 , /47979 — `go get` adds/bumps indirect requirements — 2018-2021 (snippets) **[re-verify]**
- https://github.com/bundler/bundler/issues/2016 — request for non-updating shared deps — ~2013 (snippet) **[re-verify]**
- https://guides.rubygems.org/updating_gems/ — Bundler team position; `--conservative` — snippet
- https://github.com/astral-sh/uv/issues/17747 , /13551 — `--upgrade-package` lock consistency — 2025-26 (snippets)
- https://discuss.hashicorp.com/t/error-failed-to-query-available-provider-packages-locked-provider/40422 — constraint change needs `-upgrade` — 2022 (snippet) **[re-verify]**
- https://gitlab.com/gitlab-org/terraform-images/-/issues/29 — same error in CI (snippet) **[re-verify]**
- https://github.com/hashicorp/terraform/issues/32124 — checksum mismatch on fresh checkout (snippet) **[re-verify]**
- https://github.com/NixOS/nix/issues/8511 , /5785 ; https://git.lix.systems/lix-project/lix/issues/8 — flake lock not refreshing cases (snippets) **[re-verify]**
- https://github.com/renovatebot/renovate/discussions/45598 — stale digest for mutable tag (snippet)
- https://docs.renovatebot.com/docker/ — `tag@digest` model, PRs on digest change (snippet)
- https://github.com/renovatebot/renovate/discussions/22747 — Dependabot vs Renovate `lockFileMaintenance` (snippet)
- https://edu.chainguard.dev/chainguard/containers/how-to-use/digestabot_frizbee/ — Digestabot/Frizbee digest pinning (snippet)
- https://github.com/nodejs/docker-node/issues/2461 — request to pin base images by digest (snippet)
- https://github.com/liquibase/docker/issues/13 ; https://github.com/grokzen/docker-redis-cluster/issues/154 — broken `latest`, no rollback (snippets) **[re-verify]**
- https://www.theregister.com/2025/03/17/supply_chain_attack_github/ ; https://threats.wiz.io/all-incidents/tj-actionschanged-files-supply-chain-attack — tj-actions tag repoint — 2025-03 (snippets)
- https://cve.circl.lu/cve/CVE-2026-33634 — Trivy tag hijack — 2026-03 (snippet)
- https://about.codecov.io/apr-2021-post-mortem/ — Codecov uploader compromise — 2021 (snippet) **[re-verify]**
