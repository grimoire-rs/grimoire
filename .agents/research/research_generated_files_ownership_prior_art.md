# Research: Generated files in a git working tree (ownership, stale removal, commit vs ignore)

## Metadata

**Date:** 2026-11-15
**Domain:** packaging
**Triggered by:** `.agents/discussions/stale-outputs-without-state.md` (stale outputs after a clone with no install state; issue grimoire-rs/grimoire#257)
**Expires:** 2027-05-15

## Direct Answer

No surveyed tool removes stale files from a previous lockfile alone. Tools that
delete either regenerate a whole directory they own, or carry an explicit
ownership record or removal list. Tools that ignore outputs treat the output
tree as disposable and restore it from the lock.

## Key findings

Sources were fetched on 2026-11-15; none carried a date. Items marked
(unverified) rest on search snippets or prior knowledge.

- **Ruler** (<https://github.com/intellectronica/ruler>): ignores generated
  outputs by default through a managed block in the root `.gitignore`
  (`# START Ruler Generated Files` to `# END Ruler Generated Files`).
  `--no-gitignore` disables it, `--gitignore-local` writes `.git/info/exclude`.
  Its README says ignored files are absent from git worktrees and need a
  re-apply per session. A sidecar `.ruler-managed.json` exists for subagents
  only, sits in the ignored directory, and so does not travel with a clone.
  Orphaned native dirs are removed only with `[agents] cleanup_orphaned = true`.
- **vercel-labs/skills** (<https://github.com/vercel-labs/skills>): committed
  `skills-lock.json` records a `computedHash` per skill dir; restore is
  `experimental_install`. Issue 549 shows a repo that ignored `.agents/skills/`
  beside the committed lock, which broke clone-and-restore (snippet only).
- **go mod vendor / cargo vendor**: outputs are committed, the directory is
  wiped and regenerated (`cargo vendor` unless `--no-delete`), so no per-file
  ownership is needed. `vendor/modules.txt` travels as a consistency check.
  `.cargo-checksum.json` (unverified).
- **copier** (<https://copier.readthedocs.io/en/stable/updating/>): update
  regenerates from the recorded template and answers and diffs; a file the user
  deleted stays deleted unless `copier recopy`.
- **uv** (<https://docs.astral.sh/uv/concepts/projects/layout/>): writes a
  self-managed `.gitignore` inside `.venv`. pixi adds `.pixi/` to the root
  `.gitignore` on `init` (unverified).
- **npm**: the hidden lockfile `node_modules/.package-lock.json` is a cache that
  is discarded when it disagrees with disk.
- **chezmoi**: deletion is explicit through `.chezmoiremove` patterns and
  `exact_` directories.
- **home-manager**: an existing unmanaged file blocks activation unless a backup
  extension is set; per-file `force = true` can silently delete local changes.
- **kubectl apply --prune** (unverified): ownership is an annotation plus a
  label selector on the object itself.

## Negative

- A sidecar manifest inside an ignored directory does not survive a clone
  (Ruler). A travelling marker must sit in a committed path.
- Ignoring a path the client reads at startup leaves a first-session gap in a
  fresh worktree (Ruler README).
- A committed lock with ignored contents needs a working restore command
  (skills issue 549).
- No complaint thread about Ruler's automatic gitignore editing was found; the
  issue tracker itself was not read.

## Not covered

vendir (fetch failed), git-subrepo, subtree, Gazelle, peru, cruft, stow, yadm,
Terraform, Helm, Pulumi, pnpm `.modules.yaml`, Poetry, Bundler, rulesync beyond
its README.
