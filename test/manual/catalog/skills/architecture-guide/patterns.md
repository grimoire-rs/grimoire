# Worked Patterns

Reference content for the `architecture-guide` skill, loaded on demand from
`SKILL.md`. The `architecture-guide` rule requires this skill and tells the
agent to load it, so the patterns reach the agent without a support
directory under `rules/`.

## Command pattern

```
args -> typed identifiers -> operation -> report data -> output
```

## Option-based lookups

"Not found" is `Option::None` at the lookup layer, not an error.

Edit this file after installing to see `grim status` report the skill as
`modified` (drift detection).
