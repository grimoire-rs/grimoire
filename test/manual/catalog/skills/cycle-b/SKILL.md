---
name: cycle-b
description: Half of a dependency cycle. Use only to see grim refuse a cycle (exit 65).
license: Apache-2.0
metadata:
  grim.requires.skills.cycle-a: "localhost:5050/grimoire/skills/cycle-a:1"
---

# Cycle b

Requires `cycle-a`, which requires this skill back. Never installable.
