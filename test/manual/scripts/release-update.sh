#!/usr/bin/env bash
# Outdated / rolling-release demo step: publish a NEW version (1.3.0) of
# `code-reviewer` — ABOVE the bootstrap matrix top (1.2.0) — so a project
# that already locked the floating `:1` tag (at 1.2.0) shows `↑ outdated`
# and can roll forward with `grim update`.
#
# This post-lock publish is the only way to produce a genuinely outdated
# lock without hand-editing grimoire.lock: bootstrap publishes ascending to
# 1.2.0, you `grim lock` (records 1.2.0), THEN run this to move :1 → 1.3.0.
#
# Run this AFTER `grim lock` in test/manual/project, then `grim update`.
set -euo pipefail
IFS=$'\n\t'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MANUAL_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$MANUAL_DIR/../.." && pwd)"
REGISTRY="localhost:5050" # manual-rig registry (see docker-compose.yml)
GRIM="$REPO_ROOT/test/bin/grim"

export GRIM_HOME="$MANUAL_DIR/.grim-home"
export GRIM_DEFAULT_REGISTRY="$REGISTRY"
export GRIM_INSECURE_REGISTRIES="$REGISTRY"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cp -r "$MANUAL_DIR/catalog/skills/code-reviewer" "$tmp/code-reviewer"
printf '\n## Changelog\n\n- 1.3.0: clarified the severity grouping.\n' \
    >>"$tmp/code-reviewer/SKILL.md"

printf '\033[1;34m==>\033[0m releasing code-reviewer:1.3.0 (moves :1, :latest)\n'
# --force so the demo is re-runnable after the catalog skill is edited (the
# rig's :5050 registry is throwaway — moving its tags is intended).
"$GRIM" release "$tmp/code-reviewer" "$REGISTRY/grimoire/skills/code-reviewer:1.3.0" --force

cat >&2 <<EOF

Now roll the project forward:
  cd test/manual/project
  grim status                 # code-reviewer -> 'outdated' (locked 1.2.0, :1 now 1.3.0)
  grim update                 # re-resolves :1 -> 1.3.0, re-materializes
  grep code-reviewer grimoire.lock
EOF
