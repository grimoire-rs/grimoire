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
(`ocx package install --select ocx.sh/grim`); the curl-installer is the
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

`[package] version` in `Cargo.toml` is the single source of truth. The
version is compiled in via `env!("CARGO_PKG_VERSION")`.

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
task release:prepare    # run verify, bump Cargo.toml (git-cliff --bumped-version), regenerate CHANGELOG.md
# Human reviews the changes
git add -A && git commit -m "release: vX.Y.Z"
git tag vX.Y.Z
git push --atomic origin main vX.Y.Z   # human-run — never auto-push
```

The final push is always the human's call; `release:prepare` only prints
it. After the tag is pushed, CI takes over: build → test → GitHub Release,
then the post-announce jobs.

### Catalog Publish

`dist-workspace.toml` `post-announce-jobs` runs
`.github/workflows/publish-catalog.yml` after the GitHub Release: it
republishes every package in `catalog/publish.toml` to GHCR via `grim
publish` (skip-existing, so only version-bumped packages actually push).
Locally the same batch runs as `task catalog:release` (append flags after
`--`, e.g. `-- --dry-run`). Catalog version bumps are staged in
`catalog/publish.toml` during feature work (drift duty →
`catalog/README.md`) and ride out with the next release.

### Version Between Releases

Between releases, `Cargo.toml` retains the version from the last
`release:prepare` run. `release:prepare` computes the next version from
commit history via `git-cliff --bumped-version` at release time. There is
no automated post-release version bump — `release:prepare` is the single
versioning mechanism.
