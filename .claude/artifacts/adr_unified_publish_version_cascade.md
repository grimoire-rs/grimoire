# ADR: Unify the publish/release version+cascade interface

## Metadata

**Status:** Accepted
**Date:** 2026-07-10
**Deciders:** Michael Herwig (maintainer)
**Domain Tags:** api, cli
**Supersedes:** the `--tag` non-semver rule (publish ADR "D1") and the
channel-tag always-move amendment (publish ADR "D3") for `grim publish`

## Context

`grim release` and `grim publish` expressed "what tag(s) do I push" through
two different, partly-magic interfaces:

- **release** took the version from the `<ref>` positional and inferred the
  cascade purely from the string's *shape* — a full semver cascaded
  (`1.2.3` → `1.2.3`, `1.2`, `1`, `latest`), anything else published a single
  literal tag. No explicit control.
- **publish** split the concern across two flags: `--version <semver>`
  (top-level override, cascades) and `--tag <channel>` (a movable channel
  tag applied to every entry). Their overwrite semantics disagreed:
  `--version`/default skipped-existing and needed `--force` to move, while
  `--tag` *always moved* (force-on, skip-off) and rejected semver values.

Three problems: (1) `--tag`'s always-move was inconsistent with every other
publish path and could silently overwrite; (2) cascade-vs-not was decided by
implicit string-shape magic with no way to assert intent or catch a typo;
(3) the two commands used unrelated interfaces for the same idea.

## Decision Drivers

- Uniform, predictable overwrite semantics across every value and command.
- Make cascade explicit and typo-safe without losing the convenient default.
- Reduce the flag surface without breaking the batch command's real needs.

## Considered Options

### Option 1 — Single `--version` + explicit `--cascade` on both commands

`--version` the sole source everywhere; explicit `--cascade` requires semver.

| Pros | Cons |
|------|------|
| Fewest flags | `publish --version` scope depends on value shape (top-level vs uniform) — reintroduces magic |
| Matches the "one flag" instinct | Explicit-only cascade inverts the common default (semver almost always wants cascade) |

### Option 2 — Tri-state `--[no-]cascade`, keep the convenient default (CHOSEN)

Cascade stays automatic for semver; `--cascade`/`--no-cascade` make it
explicit. `release` keeps its ref-tag; `publish` collapses `--tag` into
`--version` (shape selects scope), with uniform skip/force.

| Pros | Cons |
|------|------|
| Convenient default preserved, no silent behavior change for existing semver releases | `publish --version` scope (top-level vs uniform) still depends on value shape |
| `--cascade` doubles as an assert-semver typo guard | Two flags for a tri-state bool |
| Uniform skip-existing/`--force` for every value | `grim publish --tag` removed (breaking) |

### Option 3 — Explicit `--cascade` only (no auto), like some sibling tools

| Pros | Cons |
|------|------|
| Zero shape-magic | Every real semver release must pass `--cascade`; forgetting it silently stops moving `latest` — a worse failure mode |

## Decision Outcome

**Chosen: Option 2.**

**Cascade is a tri-state** (`Option<bool>`), from the mutually-`overrides_with`
`--cascade` / `--no-cascade` flag pair, applied by
`oci::release::publish_tags(tag, cascade)`:

| Flag state | semver `tag` | non-semver `tag` |
|---|---|---|
| none (default) | cascade `X.Y.Z, X.Y, X, latest` | single literal tag |
| `--cascade` | cascade | **error 65** (`CascadeRequiresSemver`) |
| `--no-cascade` | single exact tag, no floats | single literal tag |

For `grim release`, a prerelease ref-tag (`1.2.3-rc.1`) is always
exact-only, even under `--cascade` (it parses as semver so `--cascade` is
allowed, but a release candidate never floats). `grim publish --version`
does **not** share this leniency: the manifest forbids prerelease/build
entry versions, so a prerelease or build-metadata `--version` value is
rejected outright (exit 65) rather than treated as a channel tag — see
`validate_channel_value` under Technical Details.

**Uniform overwrite semantics.** The exact tag is skip-existing by default;
`--force` moves it. This now holds for **every** value including channels —
the `--tag` always-move special case is gone. Re-publishing `--version
canary` is an idempotent no-op unless `--force`. Cascade floats
(`X.Y`/`X`/`latest`) still always move — that is their purpose.

**Per-command shape:**

- **release** keeps the ref-tag (`repo:1.2.3`) as the version source and
  gains only `--cascade`/`--no-cascade`. No `--version` flag — the ref
  already carries it (no duplicate source, backward compatible).
- **publish** drops `--tag`; `--version` is the single source. A **semver**
  value overrides the manifest top-level version (per-entry pinned versions
  still win) and each entry cascades; a **non-semver** value is a movable
  channel tag applied to *every* entry uniformly, no cascade. `--cascade`
  combined with a channel value is a data error (65), rejected before any
  push. The `publish.toml` schema is unchanged.

**Rationale.** Keeping `--tag` as a separate flag was rejected because a
channel and a semver top-level override are the only two batch scopes, and
merging them into one `--version` (shape selects scope) is a smaller,
more teachable surface than a second flag — the residual shape-magic is
confined to *scope*, while the more dangerous shape-magic (cascade-or-not)
is now explicitly controllable. Explicit-only cascade (Option 3) was
rejected because it inverts the common default into a silent footgun.

### Consequences

**Positive:**
- One overwrite rule to learn; channels can no longer silently overwrite.
- `--cascade` is a CI typo guard (a non-semver `$GITHUB_REF_NAME` fails loud).
- `--no-cascade` enables a deliberate single-tag semver push.

**Negative / breaking (pre-1.0, acceptable):**
- `grim publish --tag <x>` is removed → clap rejects it (unexpected argument).
  Migration: `--tag canary` → `--version canary`.
- A channel publish that relied on `--tag`'s always-move now needs `--force`
  on re-publish.

**Risks:**
- `publish --version` scope is shape-dependent (semver→top-level,
  channel→uniform). Mitigated by docs, the `version_mode` classifier being
  the single, unit-tested decision point, and `validate_channel_value`
  rejecting anything shaped like a mistake before it becomes a channel: a
  prerelease/build value, a reserved cascade-float shape
  (`latest`/`X`/`X.Y`), or an illegal OCI-tag charset all fail cleanly at
  validation (65) instead of silently publishing under a wrong tag.

## Technical Details

- `oci::release::publish_tags(tag: &str, cascade: Option<bool>)` — tri-state
  tag-set computation; new `ReleaseErrorKind::CascadeRequiresSemver`
  (→ exit 65 via `classify_release`).
- `oci::release::resolve_cascade(cascade, no_cascade) -> Option<bool>` —
  shared flag-pair resolver, used by both commands.
- `command::publish::version_mode(version, prefix) -> VersionMode`
  (`Absent`/`Semver`/`Channel`) — the single scope-selection point; the
  `--cascade`-on-channel guard and `resolve_versions`/`plan_entries` routing
  both key off it. `resolve_force_skip(force) = (force, !force)` (channel
  special-case removed).
- `command::publish::validate_channel_value(channel, manifest_path) ->
  anyhow::Result<()>` — called only for `VersionMode::Channel`, before any
  push. Rejects: a prerelease/build semver (parses via `semver::Version`),
  a reserved cascade-float shape (`latest`, bare major, or `major.minor`),
  or a value that is not a legal OCI tag
  (`[A-Za-z0-9_][A-Za-z0-9._-]{0,127}`) — each a data error (65) attributed
  to the manifest.

## Validation

- Rust unit tests: `publish_tags` per tri-state cell; `resolve_cascade`;
  `version_mode`; publish `--cascade`+channel guard (65); channel
  skip-then-force-move.
- Acceptance tests (`test/tests/`): `release --no-cascade`/`--cascade`/
  `--cascade` on non-semver (65); `publish --version canary` channel;
  `publish --version canary --cascade` (65).
- Catalog drift: `grim-usage` skill + `docs/src/{commands,publishing}.md`.

## Links

- [adr_git_provenance_annotations.md](./adr_git_provenance_annotations.md) — the `--git` flag whose cascade phrasing this clarifies
- [`docs/src/publishing.md`](../../docs/src/publishing.md), [`docs/src/commands.md`](../../docs/src/commands.md)

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-07-10 | Michael Herwig | Initial record |
| 2026-07-10 | Michael Herwig | Amend: publish `--version` channel-value validation gate; clarify prerelease is release-only |
