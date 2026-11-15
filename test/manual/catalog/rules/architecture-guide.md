---
paths:
  - "src/**/*.rs"
summary: Architecture guide with examples
keywords: architecture,patterns,design,examples
metadata:
  grim.requires.skills.architecture-guide: "localhost:5050/grimoire/skills/architecture-guide:1"
---

# Architecture Guide

A **rule that requires a skill**: the worked example of migrating a rule's
support directory. The rule used to ship `architecture-guide/patterns.md`
beside this index; grim 0.16.0 no longer packs a rule's support directory, so
that file now lives in the `architecture-guide` skill, which this rule
requires. Use it to exercise a rule-to-skill dependency: installing the rule
installs the skill, and removing the rule removes it again.

Load the `architecture-guide` skill for the worked patterns this rule
references.

- Facade at the CLI boundary; business logic in plain modules.
- Strategy via traits for swappable implementations.
- Three-layer errors so batch operations diagnose per item.
