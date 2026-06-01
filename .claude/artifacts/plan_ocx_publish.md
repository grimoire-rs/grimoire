# Plan — Release grim with musl + publish to OCX registry

## Goal

1. Add musl Linux targets to the cargo-dist release (so the GitHub release
   ships musl archives alongside gnu/darwin/windows).
2. After the release is published, repackage the freshly built
   **musl + darwin + windows** archives as OCX packages, **test each on a
   native runner** with `ocx package test`, then **publish** the
   multi-platform package to the production registry `ocx.sh/grim`.
3. Ship a `CATALOG.md` + logo and push them via `ocx package describe`.
4. Replace `assets/logo.svg` with the new `assets/logo.png`; use the PNG for
   the README and the OCX catalog logo.

Reference (NOT a mirror — direct release): `ocx/.github/workflows/oci-publish.yml`
+ `post-release-oci-publish.yml`. The `ocx.mirror` skill is reference only.

## Key facts (verified)

- Archive names: `grimoire-<target>.tar.xz` (unix) / `grimoire-<target>.zip`
  (windows). Binary inside = `grim` (`grim.exe` on windows). Package = `grimoire`.
- `ocx package push` accepts `.tar.gz`/`.tar.xz` layers only — NOT `.zip`.
  So windows `.zip` is repacked via `ocx package create` into a `bin/`-layout
  `.tar.xz` on the native windows runner.
- Metadata (`packaging/grim/metadata.json`): bundle v1, single PATH env
  `${installPath}/bin` (public, required). No entrypoints (single binary).
- Auth: GitHub **environment `ocx.sh`** holds secrets `REGISTRY_USER` /
  `REGISTRY_TOKEN`. ocx reads `OCX_AUTH_ocx_sh_USER` / `OCX_AUTH_ocx_sh_TOKEN`
  (registry slug = non-alnum→`_`). Only the publish job sets `environment: ocx.sh`.
- `release:published` from GITHUB_TOKEN does NOT trigger a new workflow →
  must use cargo-dist `post-announce-jobs` hook (runs inside the release run).
- Frozen single version + concurrent index writes → **single consolidated
  publish job** (push all 6 platforms sequentially to `ocx.sh/grim:<ver>`).

## Local validation (DONE)

Downloaded v0.1.0 linux archive → repackage `bin/grim` → `ocx package create
-p linux/amd64` → `ocx package test -- grim --version` → `grim 0.1.0`, exit 0.
Mechanism + metadata proven before any CI change.

## Implementation

| Step | File | Action |
|------|------|--------|
| musl targets | `dist-workspace.toml` | add `x86_64`/`aarch64-unknown-linux-musl` |
| hook | `dist-workspace.toml` | `post-announce-jobs = ["./publish-ocx"]` |
| pipeline | `.github/workflows/publish-ocx.yml` | new (test matrix + publish) |
| metadata | `packaging/grim/metadata.json` | bundle v1 (DONE) |
| catalog | `CATALOG.md` | frontmatter + body |
| logo | `assets/logo.png` | new (keep); remove `assets/logo.svg` |
| readme | `README.md` | svg→png |
| regen | `.github/workflows/release.yml` | `dist generate` (musl + custom job) |
| version | `Cargo.toml` | 0.1.0 → 0.2.0 |
| changelog | `CHANGELOG.md` | git-cliff regen |

## publish-ocx.yml shape

- `workflow_call` (input `plan`, cargo-dist contract) + `workflow_dispatch`
  (input `tag`, manual re-publish escape hatch).
- Job `test` — matrix of 6 NATIVE runners (linux musl amd64/arm64, darwin
  amd64/arm64, windows amd64/arm64): setup-ocx → `gh release download` the
  platform archive → extract (`tar -xf`, bsdtar handles zip on windows) →
  `bin/` layout → `ocx package create` → `ocx package test -- grim --version`
  → upload `ocxpkg-<target>`.
- Job `publish` — single ubuntu, `needs: test`, `environment: ocx.sh`:
  download all `ocxpkg-*` → `ocx package push --cascade --new` each platform
  to `ocx.sh/grim:<ver>` → `ocx package describe ocx.sh/grim` with CATALOG +
  logo + title/description/keywords.

## Release

Branch `feat/ocx-publish` → verify → adversarial review of pipeline →
FF to main → tag `v0.2.0` → push tag → watch release run → confirm OCX
publish + `ocx package inspect ocx.sh/grim:0.2.0`.

## SHA-pins

- checkout v6.0.2 `de0fac2e4500dabe0009e67214ff5f5447ce83dd`
- ocx-sh/setup-ocx v1 `0a39e58f272e7b4e2e46cfa89cdbad2a3578d2cc`
- upload-artifact v7.0.1 `043fb46d1a93c77aae656e7c1c64a875d1fc6a0a`
- download-artifact v7.0.0 `37930b1c2abaa49bbe596cd826c3c89aef350131`
