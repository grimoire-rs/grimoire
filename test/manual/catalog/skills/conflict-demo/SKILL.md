---
name: conflict-demo
description: Requires commit-helper at :2. Use with triage-bot (which needs :1) to see a dependency conflict (exit 78).
license: Apache-2.0
metadata:
  grim.requires.skills.commit-helper: "localhost:5050/grimoire/skills/commit-helper:2"
---

# Conflict Demo

Wants `commit-helper:2`; `triage-bot` wants `commit-helper:1`. Declaring
both in one project cannot resolve.
