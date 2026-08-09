# Design note — `grim status --exit-code` (W1-C)

**Status:** proposed, not implemented. Written 2026-07-26 by the meta-plan
orchestrator so W1-C can be delegated without re-deciding the contract.
**Blocks:** nothing for 1.0. **Unblocks:** the drift/CI claim that
`meta-plan_promotion_1_0.md` currently strikes from all positioning.

## Why this exists

Positioning wants to say "wire `grim status` into CI and it fails when your
agent config has drifted." Today it cannot: `grim status` exits `0` whether
every artifact is pristine or every one is modified. A CI step would have to
parse `--format json` and grep, which makes the claim a lie in the only form
anyone would actually use it.

`--check` already exists and is a *different* axis — it adds a network
re-resolution and populates `update_available` / `deprecated`. It says nothing
about exit status. The two compose; neither implies the other.

## The one-way door

The exit code is a frozen surface the moment it ships (Principle 9). A script
that reads `case $? in 65) ...` cannot be un-taught. So the decision that
matters is not the flag — it is **which states count as drift**, and **which
code carries it**.

## Proposed contract

`--exit-code` changes only the process exit status. Report content, JSON shape,
and stdout are byte-identical with and without it. Nothing new appears in the
JSON — a consumer that wants detail already has `items[]`.

Exit `0` when every declared artifact is installed and unmodified.
Exit **`65` (`DataError`)** when at least one artifact is in a drift state.
Every existing error path keeps its current code and wins over drift — an
unreachable registry is still `69`, a bad config still `78`. Drift is only
reported when the command otherwise succeeded.

### What counts as drift

| State | Drift? | Reasoning |
|---|---|---|
| `modified` | **yes** | The materialized file no longer matches the locked content. This is the case the claim is about. |
| missing / not installed | **yes** | Declared, locked, absent from the client. `grim install` would change the tree. |
| client drift (`clients_missing` / `clients_extra` non-empty) | **yes** | Already computed locally today, already in the JSON. A configured client with no output is the same failure in a different dimension. |
| `outdated` (floating tag has moved) | **no** | Not drift — it is *news*. Failing CI because upstream published a new version turns every unrelated pipeline red on someone else's release. This is what `--check` plus `update_available` is for. |
| `deprecated` / `replaced_by` | **no** | Same reasoning, and it needs the network. |

The split is the whole design: **drift is "this checkout does not match its own
lockfile", not "the world has moved on".** The first is the committer's fault
and is reproducible offline; the second is not.

### Why 65 and not a new code

`DataError` already means "input data is not in the state it must be in", and
the private range above 78 is worth spending only on a distinction a script
must act on differently. A script that must separate drift from a malformed
lockfile can read `--format json`. If a concrete need for a dedicated code
appears later, adding one is additive; taking 65 back would not be.

## Test obligations

- Acceptance: clean tree exits `0` with and without the flag; a modified file
  exits `65` with the flag and `0` without it (proves the flag is the only
  behavioural change).
- Acceptance: an `outdated` artifact exits `0` even with `--exit-code` — this
  is the assertion that stops the "news is drift" regression.
- Acceptance: `clients_missing` non-empty exits `65`.
- Acceptance: an unreachable registry under `--check --exit-code` exits `69`,
  not `65` — error codes outrank drift.
- Unit: the drift predicate, table-driven over the state enum, so a new state
  variant fails to compile until it is classified.

## Not in scope

No `--exit-code` on other commands. No new JSON field. No change to `--check`.
