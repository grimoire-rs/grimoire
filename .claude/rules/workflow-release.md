---
paths:
  - dist-workspace.toml
  - cliff.toml
  - CHANGELOG.md
  - .github/workflows/verify-deep.yml
  - .github/workflows/release.yml
  - .github/workflows/publish-catalog.yml
---

# Release Implementation Notes

Guidance for the release + versioning strategy. Read when working on
release infra, versioning, or the changelog.

The release has two halves: cargo-dist builds and publishes the `grim`
binary; the first-party catalog (`catalog/`) is republished to GHCR as OCI
artifacts via `grim publish` in a post-announce CI job.

## Key Decisions

### cargo-dist: Builds the Binary

cargo-dist handles binary builds, archives, checksums, the shell +
PowerShell installers (`grimoire-installer.sh` / `.ps1`), and GitHub
Release creation for the single `grim` binary. Configuration lives in
`dist-workspace.toml` (source of truth); the `installers` key drives which
installers are produced. The CI workflow is installer-agnostic — `dist
plan` reads the config at release time — so toggling `installers` needs no
`release.yml` change.

The docs site hosts version-less front doors at `docs/src/install.{sh,ps1}`
that fetch `releases/latest/download/grimoire-installer.{sh,ps1}` at run
time (the cargo-dist script bakes in a pinned version, so it cannot be
copied verbatim to the site). The recommended install path is [ocx][ocx]
(`ocx --global add grimoire/cli`); the curl-installer is the
no-ocx fallback.

[ocx]: https://ocx.sh

### Generated Workflows: Never Edit, Always Regenerate

cargo-dist generates the release workflow under `.github/workflows/` when
present. If it exists, **NEVER edit it directly** — changes are lost on the
next regeneration. To modify the release workflow:

1. Edit `dist-workspace.toml` (config source of truth)
2. Run `dist generate-ci` to regenerate the workflow
3. Commit both the config change and the regenerated workflow

### Version Source of Truth

`[workspace.package] version` in the root `Cargo.toml` is the single source of
truth; `grimoire` and the `crates/*` members inherit it with
`version.workspace = true`. The version is compiled in via
`env!("CARGO_PKG_VERSION")`.

### Commit Convention

All commits follow [Conventional Commits](https://www.conventionalcommits.org/).
git-cliff generates `CHANGELOG.md` from these.

### Existing Dependabot Config

`.github/dependabot.yml` uses dependency groups (`actions`, `rust-deps`).
When changing it, **preserve existing groups**.

## Cross-References

- **Documentation rules**: See `.claude/rules/docs-style.md` for doc
  writing guidelines
- **CI workflow patterns**: See `.claude/rules/subsystem-ci.md` for design
  principles, cost factors, and review checklist
- **CLI commands reference**: See `.claude/rules/subsystem-cli-commands.md`
  — update when adding new behavior

## Workflow: Release Ceremony

Release ceremony is a human-driven process with tooling support:

```bash
task release:prepare    # run verify, bump Cargo.toml and catalog/publish.toml (git-cliff --bumped-version), regenerate CHANGELOG.md
/changelog              # write the release up on the site changelog (docs/src/content/docs/changelog.md)
# Human reviews the changes
git add -A && git commit -s -m "release: vX.Y.Z"
git push origin main                                     # human-run — never auto-push
gh workflow run release.yml --ref main -f tag=vX.Y.Z     # human-run; never from another branch
```

`CHANGELOG.md` is the commit-level record; the site page `/changelog.html`
is the user-facing one — features by outcome, linked to their docs. The
`changelog` skill adds each release to it, and the page change rides in the
same `release: vX.Y.Z` commit. **An AI performing a release (rare) runs
`/changelog` before the release commit — the release is incomplete without
it.**

The push and the dispatch are always the human's call; `release:prepare` only
prints them. After the dispatch, CI takes over: build → test → GitHub Release
(and the git tag), then the post-announce jobs.

`/prepare-release <major|minor|patch>` covers the first two lines plus what
precedes them: an inventory of every change since the last tag, a drift review
of the shipped `catalog/` packages and of the docs pages, then the bump and the
changelog page. It stops before the release commit.

### Dispatch Release: Stable Signing Identity

`dist-workspace.toml` sets `dispatch-releases = true`: a release is
`release.yml` dispatched from `main` with input `tag` (`vX.Y.Z`; the default
`dry-run` plans and builds without publishing). Pushing a tag does not release.
Why: the OCX package is signed keylessly, and the Sigstore certificate names
the workflow ref. A tag-push run would sign as `…@refs/tags/vX.Y.Z`, a new
identity per release; a dispatch from `main` signs as
`https://github.com/grimoire-rs/grimoire/.github/workflows/release-ocx.yml@refs/heads/main`
(issuer `https://token.actions.githubusercontent.com`), one identity consumers
can pin.

- **The workflow creates the tag.** cargo-dist tags the dispatched commit
  together with the GitHub Release. Never create or push a tag by hand.
- **Dispatch only from `main`.** The generated `release.yml` does not block
  another ref: a real-tag dispatch from a branch can create the tag and the
  GitHub Release before `release-ocx.yml` refuses to sign (its first step
  requires `refs/heads/main`), leaving a half-released version. A generated
  guard (`plan-jobs`) was tried and dropped: cargo-dist wires a plan job only
  into `build-local-artifacts`' `needs`, and `host` runs with `always()` when
  builds are skipped, so it does not reliably stop the release. The gap stays.
  Mitigations: operator discipline (`--ref main`) or a repository ruleset that
  limits who may dispatch from which ref. Editing `release.yml` by hand is
  ruled out.
- **Each platform of `ocx.sh/grimoire/cli` is signed** and carries a signed
  CycloneDX SBOM attestation, verified before any floating tag moves. Only
  platform manifests are signed, not the image index: a manual
  `ocx package verify` needs `-p <platform>`.
- **Structure.** `release-ocx.yml` is a called workflow of the generated
  `release.yml` (`post-announce-jobs = ["./release-ocx", "./publish-catalog"]`,
  `id-token: write` granted via `github-custom-job-permissions`) and is also
  dispatchable with a `tag` input, so a failed publish is repaired from `main`
  under the same identity. It guards `refs/heads/main`, checks out, and runs
  the composite action `.github/actions/release-ocx` (repackage, sign and
  attest, verify, describe, cascade, announce). The two files' header comments
  are the source of truth; `.claude/tests/test_workflows.py` pins the guard.
- **Custom signing lives in a called workflow precisely so `release.yml` stays
  generated** (see "Generated Workflows" above): the certificate names the
  file that defines the signing job, and cargo-dist owns `release.yml`.

### Backfill and Publisher Declaration

`.github/workflows/backfill.yml` (dispatch-only, `scripts/backfill_release.py`)
signs and attests already-published versions (0.11.1 to 0.16.0 have a
package; v0.11.0 and older have none). It signs as a second identity,
`…/backfill.yml@refs/heads/main`. `dry_run` defaults to true, and the job sits
behind the owner-only `release-backfill` environment (required reviewer).

Order matters. A consumer's `ocx` verifies on install only when the index
entry in `ocx-sh/index` declares `publishers` (or the user sets a trust
policy); signing alone changes nothing for users. Once `publishers` is
declared, an unsigned version fails to install with exit 79. So:

1. signed release (the first dispatch release, with `release-ocx.yml` signing);
2. backfill every older version;
3. only then a `publishers` PR on `ocx-sh/index` naming both identities.

### Catalog Publish

`dist-workspace.toml` `post-announce-jobs` runs
`.github/workflows/publish-catalog.yml` after the GitHub Release: it
republishes every package in `catalog/publish.toml` to GHCR via `grim
publish` (skip-existing, so only packages with a new version actually push).
Locally the same batch runs as `task catalog:release` (append flags after
`--`, e.g. `-- --dry-run`). The catalog's `publish.toml`
version equals `[workspace.package] version`; `release:prepare` sets both,
so feature work never bumps it (drift duty → `catalog/README.md`) and
content changes ride out with the next release.

### Version Between Releases

Between releases, `Cargo.toml` retains the version from the last
`release:prepare` run. `release:prepare` computes the next version from
commit history via `git-cliff --bumped-version` at release time. There is
no automated post-release version bump — `release:prepare` is the single
versioning mechanism.
