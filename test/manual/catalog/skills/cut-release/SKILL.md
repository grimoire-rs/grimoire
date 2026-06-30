---
name: cut-release
description: A release-cutting playbook — version bump, changelog, tag, publish checklist. Published ALONE under a deep nested repo path (playbooks/ci/release/) so the TUI tree folds the single-child chain into one node.
license: Apache-2.0
metadata:
  summary: Release-cutting playbook (tree-fold demo)
  keywords: release,ci,playbook,demo
  author: grimoire-manual-rig
  repository: https://github.com/grimoire-samples/cut-release
---

# Cut a Release

A small playbook for cutting a release: bump the version, regenerate the
changelog, tag, and run the publish checklist.

It exists in the manual rig for one structural reason: it is the **only**
package under the `playbooks/ci/release/` namespace chain. Each of those
segments has exactly one child, so the TUI tree view joins
`playbooks` → `ci` → `release` into a single folded node (the "longest empty
prefix" / VS Code "compact folders" behavior), with `cut-release` as its one
leaf below it.
