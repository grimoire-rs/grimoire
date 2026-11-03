---
name: cycle-a
description: Half of a dependency cycle. Use only to see grim refuse a cycle (exit 65).
license: Apache-2.0
metadata:
  grim.requires.skills.cycle-b: "localhost:5050/grimoire/skills/cycle-b:1"
---

# Cycle a

Requires `cycle-b`, which requires this skill back. Never installable.
