# ADR: Tool-namespaced metadata keys in canonical SKILL.md frontmatter

## Metadata

**Status:** Accepted
**Date:** 2026-06-10
**Deciders:** Michael Herwig (maintainer)
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md`
      (OCI distribution substrate unchanged; canonical agentskills format
      untouched on the wire)
**Domain Tags:** integration, api
**Supersedes:** N/A

## Context

A published `SKILL.md` is a canonical [agentskills][agentskills-spec]
document: `name`, `description`, `license`, `compatibility`,
`allowed-tools`, and a `metadata` map of string-valued key/value pairs.
Those are the only top-level fields the specification defines.

Each client tool (Claude Code, OpenCode, GitHub Copilot) adds its own
capability fields on top — for example, [Claude Code][claude-skills-docs]
reads `disable-model-invocation`, `user-invocable`, `effort`, `context`,
`argument-hint`, `when_to_use`, and others. OpenCode reads none of these;
Copilot reads none of them either.

The problem is that a skill author writing for multiple clients had two
bad choices before this decision:

1. **Duplicate files**: maintain a `SKILL.md` per client, keeping them in
   sync manually.
2. **Spec contamination**: write Claude-specific fields at the top level of
   the canonical `SKILL.md`, making every client download an artifact
   that carries fields it does not understand and that violate the
   agentskills wire contract.

The install-time pipeline is the right place to resolve this: the canonical
artifact stays spec-pure on the wire, and grim projects the frontmatter
for each client when it writes the files to disk.

## Decision Drivers

- Keep the published artifact spec-compliant with [agentskills][agentskills-spec]
  — no tool-specific top-level keys in the OCI layer.
- Single source of truth: one `SKILL.md` in the repository, not one per
  client.
- At-install projection: grim transforms per client; the author authors once.
- Publish-time validation: bad literals (e.g. `claude.effort: "warp"`) fail
  the publish, never silently ship a broken artifact.
- The projection must be deterministic: same input → byte-identical output,
  so the integrity hash anchors on the expected rendered bytes and a user
  edit is detected as drift.

## Considered Options

### Option 1 — Typed top-level vendor fields — REJECTED

Add typed optional fields to the `SkillFrontmatter` struct for each
client's capabilities (`user_invocable: Option<bool>`, `effort:
Option<EffortEnum>`, …), parsed from the canonical document.

| Pros | Cons |
|------|------|
| Native Rust types — no string-to-bool conversion at install time | Every new Claude/OpenCode/Copilot release that adds a field requires a Grimoire release |
| Compile-time exhaustiveness on known fields | The struct encodes vendor knowledge; vendors own their own schemas |
| | Non-agentskills fields at the top level violate the spec — artifacts are no longer portable through other agentskills tooling |
| | As capabilities diverge (three clients × N fields), the struct balloons |

**Rejected.** Spec impurity is disqualifying. Struct churn per vendor release is a maintenance trap.

### Option 2 — Separate per-client files in the OCI artifact — REJECTED

Pack `SKILL.claude.md`, `SKILL.opencode.md`, `SKILL.copilot.md` into the
artifact layer, let the installer pick the right one.

| Pros | Cons |
|------|------|
| Each file carries exactly the fields its client reads | Artifact duplication: N files per client, each a near-copy |
| No runtime projection needed | Author still maintains near-duplicate source files |
| | Wire contract changes (layer shape, manifest shape) for every new client |
| | No single canonical file to show in `grim search` / TUI |

**Rejected.** Duplication is the exact problem this feature is meant to
solve. Wire-contract churn is unacceptable.

### Option 3 — Namespaced string keys inside `metadata` — CHOSEN

The canonical `SKILL.md` stays spec-pure. Tool capabilities are authored as
string-valued entries in the agentskills `metadata` map, namespaced by the
target client: `claude.<field>: "value"`. At install time grim reads the
known registry for the target and converts each matching key to its native
YAML type, lifting it to a top-level key in the written file.

| Pros | Cons |
|------|------|
| Canonical artifact is agentskills-compliant; `metadata` strings are in-spec | Authors must learn the `claude.*` prefix convention |
| Single source of truth; no per-client source files | The field registry in grim must track vendor-defined schemas |
| Publish-time validation catches typos and bad literals before they reach a registry | Legacy top-level keys parse into `extra` and install verbatim — migration nudge only, not hard error at install |
| Deterministic render → integrity-hashable generated files | JSONC comment loss on OpenCode config rewrite (documented caveat) |
| New field = one table row in the registry; no Rust type-model change needed | |
| Projection is a pure function; easy to test | |

**Chosen.**

## Decision Outcome

The canonical `SKILL.md` is authored with agentskills-standard fields only.
Tool-specific capabilities live in the `metadata` map under a
`<client>.<field>` namespace. At install time `src/install/render.rs`
projects the frontmatter for the target client:

- a **known** `<target>.<field>` key converts to its native YAML type
  (bool, string, enum) and is lifted to a top-level frontmatter key;
- an **unknown** `<target>.<field>` key warns and is dropped (typo guard);
- a known key with an **invalid literal** is a hard `RenderError` — publish
  fails (exit `DataError = 65`), install fails with `MaterializeFailed`;
- a **foreign-namespace** key (e.g. `opencode.*` when rendering Claude) is
  dropped silently;
- **plain metadata** keys (unknown prefixes like `vendor.x`) pass through
  unchanged.

A `SKILL.md` with no tool-namespaced keys takes the fast path: byte-identical
verbatim install, no render pass.

### Known namespaces

Three tool namespaces are recognized: `claude`, `opencode`, `copilot`.
Any key with a different prefix (e.g. `vendor.x`) is plain metadata.

### claude.* skill registry

The full mapping from namespaced key to native Claude Code frontmatter
field. This table is the implementation in `CLAUDE_SKILL_FIELDS` in
`src/install/vendor_claude.rs` — it is the single source of truth.

| Namespaced key | Native key | Type |
|---|---|---|
| `claude.disable-model-invocation` | `disable-model-invocation` | bool (`"true"`/`"false"`) |
| `claude.user-invocable` | `user-invocable` | bool |
| `claude.model` | `model` | string |
| `claude.effort` | `effort` | enum: `low`, `medium`, `high`, `xhigh`, `max` |
| `claude.context` | `context` | enum: `fork` |
| `claude.agent` | `agent` | string |
| `claude.argument-hint` | `argument-hint` | string |
| `claude.when-to-use` | `when_to_use` | string (note: native key uses underscore) |
| `claude.arguments` | `arguments` | string |
| `claude.disallowed-tools` | `disallowed-tools` | string |
| `claude.shell` | `shell` | enum: `bash`, `powershell` |
| `claude.paths` | `paths` | string (comma-separated globs) |

`hooks` is deliberately absent: it is an object-valued field that cannot
be expressed as a single string metadata value. A separate hooks ADR owns
that surface.

### opencode.* and copilot.* skill registries

Both are **empty**. OpenCode and Copilot read only the universal
agentskills fields from a `SKILL.md`; any `opencode.*` or `copilot.*`
skill key is unknown and will warn + drop.

### Rule-level metadata

Rule frontmatter follows the same common-vs-unique principle as skills:
`paths` is a top-level canonical field (common to multiple clients);
vendor-unique capabilities are authored inside a `metadata:` map under
their `<vendor>.<field>` namespace. The registry for rule keys is in
`src/install/vendor_copilot.rs`.

The only registered rule key in v1 is `copilot.exclude-agent`
(authored as `metadata.copilot.exclude-agent`; enum: `code-review`,
`cloud-agent`), which maps to `excludeAgent:` in the [Copilot][copilot-instructions-docs]
`.instructions.md`. A bad literal is a hard `RenderError`. Unknown
tool-namespaced rule keys warn and drop at publish time.

A vendor-namespaced key authored top-level in rule frontmatter is never
projected — publish emits a migration nudge ("author it inside 'metadata'").

### Legacy top-level key migration

A legacy `SKILL.md` that authors Claude-specific fields at the top level
(e.g. `user-invocable: true`) has those fields parsed into the `extra` map
and installed verbatim — no breakage. `grim build` / `grim release` emits
a migration-nudge warning:

```
top-level frontmatter key 'user-invocable' is not an agentskills field;
author it as metadata 'claude.user-invocable' instead
```

This is a warning, not a hard error, to give authors a migration window.

### Namespaced key wins on collision

When a `claude.<field>` key and a colliding top-level key both appear in
the same `SKILL.md`, the namespaced key takes precedence (the projection
overwrites the top-level value). A warning is emitted:

```
metadata key 'claude.model' overrides the top-level 'model' frontmatter key
```

### Generated file integrity

A rendered `SKILL.md` written to disk has `generated: true` in the install
record. The integrity hash anchors on the **expected rendered bytes** —
not the canonical input bytes. A user edit to the rendered file therefore
diverges from the expected hash and is detected as drift on the next
`grim update` / `grim status`.

### Publish-time validation

`grim build` and `grim release` call `validate_namespaced_metadata` in
`src/install/render.rs`, which runs the projection for every supported
client and unions the warnings. Any `RenderError::InvalidValue` fails the
publish (exit `DataError = 65`). The artifact never reaches the registry
with a broken projection.

### Consequences

**Positive:**
- The canonical artifact is agentskills-compliant and portable through
  third-party agentskills tooling.
- One source file; grim handles the per-client divergence at install.
- Publish-time validation prevents silent shipping of broken values.
- Adding a new field is one table row in the registry — no type-model
  change.

**Negative / Risks:**
- The `claude.*` namespace is a one-way-door authoring format: once a
  skill catalog grows, the field names are load-bearing for authors. A
  future rename requires a migration nudge analogous to the legacy key
  nudge.
- JSONC comment loss: when grim rewrites an OpenCode `opencode.jsonc` to
  register or deregister its managed glob, JSONC comments in that file are
  not preserved (grim writes plain JSON). This is a documented limitation;
  a warning is emitted when it occurs.
- `hooks` is excluded from v1. Object-valued frontmatter fields cannot be
  expressed as single metadata strings; the hooks surface will be addressed
  in a separate ADR.

## Validation

- Rust unit tests in `src/install/render.rs`: known fields lift to native
  types; unknown target keys warn + drop; bad bool/enum literals error;
  foreign namespaces drop silently; `when-to-use` maps to `when_to_use`;
  render is deterministic; collision warns + namespaced wins; legacy key
  emits migration nudge. Rule projection: plain rule with no tool-namespaced
  keys returns `None` (verbatim fast path); rule with foreign vendor key
  renders cleaned for Claude; empty-frontmatter rule after cleaning omits
  block; top-level vendor rule key emits migration nudge.
- Rust unit tests in `src/install/vendor_claude.rs`: doc/registry parity
  test — every `claude.*` key in `docs/src/vendor-metadata.md` matches the
  `CLAUDE_SKILL_FIELDS` registry exactly.
- Rust unit tests in `src/install/vendor_copilot.rs`: `copilot.exclude-agent`
  reads from `metadata` map; bad literal fails with `RenderError`; `paths`
  maps to `applyTo` (comma-joined); bare rule yields no frontmatter block;
  transforms are deterministic.
- Rust unit tests in `src/install/vendor_opencode.rs`: frontmatter stripped,
  provenance prepended; own-namespace rule key warns.
- Rust unit tests in `src/install/opencode_config.rs`: managed glob
  added/removed/unchanged; JSONC parsed and rewritten; unparseable config
  refused; global path resolution order.

## Refinements

Post-initial-acceptance refinements recorded here rather than in separate
amendment ADRs because the feature is unreleased.

### Vendor trait structure

The vendor field registries and rule-index transforms moved out of
`src/install/render.rs` into per-vendor structs that implement a `Vendor`
trait in `src/install/vendor.rs`. `ClientTarget` stays as the closed
identity enum (parse / display / paths); behavior dispatches through
`ClaudeVendor`, `OpenCodeVendor`, and `CopilotVendor` in
`src/install/vendor_claude.rs`, `src/install/vendor_opencode.rs`, and
`src/install/vendor_copilot.rs`. `render.rs` is now pure projection
mechanics. Source-of-truth file pointers:

- Claude skill registry: `CLAUDE_SKILL_FIELDS` in `src/install/vendor_claude.rs`
- Copilot rule registry: `COPILOT_RULE_FIELDS` in `src/install/vendor_copilot.rs`
- Projection engine: `src/install/render.rs`

### Common-vs-unique authoring principle

Codified as the governing rule for the metadata/top-level split: a
capability **common to several vendors** is authored once as a canonical
top-level frontmatter field and projected per vendor (e.g., `paths` →
Claude `paths:`, Copilot `applyTo:`); a capability **unique to one
vendor** is authored as a `<vendor>.<field>` string key inside `metadata`.
This principle is declared in `vendor.rs` module-level doc and governs
all future field placement decisions.

### Rule metadata map (symmetry with skills)

Rule vendor keys moved from top-level frontmatter into a `metadata:` map,
mirroring skill authoring. Canonical rule authoring is now:

```yaml
---
paths: ["**/*.rs"]
keywords: rust,style
metadata:
  copilot.exclude-agent: code-review
---
```

`keywords` and `summary` stay top-level (catalog fields, not
vendor-specific). A vendor-namespaced key authored top-level is not
projected — publish emits a migration warning.

### Claude rule symmetry

Claude rule install is now symmetric with skill install: a rule carrying
no tool-namespaced metadata installs verbatim (`generated: false`, fast
path — `paths:` is native). A rule that does carry tool-namespaced
metadata is re-rendered: own-namespace keys lift per registry (Claude rule
registry is empty today → unknown ones warn + drop), foreign vendor keys
drop, plain keys survive. Written `generated: true`. If cleaned frontmatter
is empty, the block is omitted entirely. No provenance comment is written
for Claude (mirrors rendered `SKILL.md` behavior).

### Unified universal render for skills

[OpenCode][opencode-skills-docs] and [GitHub Copilot][copilot-instructions-docs]
both have empty skill registries. Their rendered skill files are therefore
byte-identical (the *unified universal render*). A Claude-installed skill
is also discovered by both other tools, which ignore the lifted Claude
fields as unknown keys.

### Completed Vendor trait surface

The `Vendor` trait in `src/install/vendor.rs` is now fully implemented
across all three vendor structs. The complete method surface is:

- `name()` — the `--client` identifier and `metadata` namespace prefix.
- `root_dir()` — the project-relative root directory (`.claude`, `.opencode`,
  `.github`).
- `skill_fields()` / `rule_fields()` — the known `<vendor>.*` field
  registries; empty for OpenCode and Copilot skills.
- `skills_root(workspace, scope)` / `rule_path(workspace, scope, name)` —
  scope-aware output path resolution (see vendor-native global layout below).
- `skill_index(doc)` / `rule_index(parsed, pinned)` — per-vendor index
  transforms.
- `sync_config(state, workspace, scope)` — the reversible config-registration
  hook, called after every install/update/uninstall for all involved vendors.
  Default implementation is a no-op; [OpenCode][opencode-config-docs]
  overrides it to maintain the managed `instructions` glob in
  `opencode.json`. No vendor-specific branching lives outside the vendor
  structs: the command layer calls a generic per-vendor sync loop.

Orchestration (I/O, tree copy, integrity recording) stays shared on
`ClientTarget` (template method); only vendor-varying behavior lives in
the structs.

### Vendor-native global layout

Global-scope installs previously targeted `$GRIM_HOME/.<vendor>/` —
directories no agent scans. The layout was corrected to install directly
into each client's native user-level discovery directory:

| Client | Skills | Rules |
|--------|--------|-------|
| [Claude Code][claude-memory-docs] | `~/.claude/skills/<name>/` | `~/.claude/rules/<name>.md` |
| [OpenCode][opencode-skills-docs] | `$XDG_CONFIG_HOME/opencode/skills/<name>/` | `$GRIM_HOME/.opencode/rules/<name>.md` + absolute glob in the global `opencode.json` |
| [GitHub Copilot][copilot-instructions-docs] | `~/.copilot/skills/<name>/` | `$GRIM_HOME/.github/instructions/<name>.instructions.md` (inert — Copilot has no documented user-level instructions path; grim warns at install) |

The defaults above are overridden by each client's directory env variable
— see the "Vendor directory env overrides" refinement below.

**Rationale**: the previous `$GRIM_HOME` layout was unscanned by every
supported tool, making global installs functionally useless without manual
configuration. Vendor-native dirs are discovered out of the box.

**Fallback**: when `$HOME` cannot be resolved (certain CI environments),
grim falls back to the workspace layout under `$GRIM_HOME` for the
affected client. Uninstall and integrity checks are unaffected because the
recorded path is always absolute.

**OpenCode global rules**: rules stay under `$GRIM_HOME/.opencode/rules/`
and are registered as an absolute glob in the global `opencode.json`
(respecting `$OPENCODE_CONFIG`). This means OpenCode discovers them via
config rather than a scanned native directory — the same mechanism as
project scope, with an absolute path instead of a relative one.

**Copilot global rules caveat**: [GitHub Copilot][copilot-instructions-docs]
documents no user-level instructions directory. Global Copilot rule
installs write to `$GRIM_HOME/.github/instructions/` and are inert —
Copilot will not pick them up. grim emits a warning at install time. The
limitation is Copilot-rules-only; Copilot skills work correctly via
`~/.copilot/skills`.

### Vendor directory env overrides

The vendor-native global layout initially hardcoded the default user-level
directories, ignoring each client's own directory-override environment
variable. Corrected — global-scope path resolution now honors them
(verified against official docs, 2026-06-11):

| Variable | Verified semantics | grim behavior |
|----------|--------------------|---------------|
| `CLAUDE_CONFIG_DIR` | Replaces the **entire** `~/.claude` tree — "every ~/.claude path … lives under that directory instead" (code.claude.com/docs/en/claude-directory) | Global Claude skills **and** rules root at `$CLAUDE_CONFIG_DIR` when set |
| `COPILOT_HOME` | "Replaces the entire ~/.copilot path" (docs.github.com → Copilot CLI config-dir reference) | Global Copilot skills land in `$COPILOT_HOME/skills/` when set. `$XDG_CONFIG_HOME` interplay is undocumented and inconsistent upstream (github/copilot-cli#1750) — not honored |
| `OPENCODE_CONFIG_DIR` | **Additive** extra scan directory, searched with the `{skill,skills}/**/SKILL.md` pattern alongside the always-scanned global config dir (opencode.ai/docs/config) | Global OpenCode skills land in `$OPENCODE_CONFIG_DIR/skills/` when set (respects the user's explicit override; the XDG default stays scanned by OpenCode either way) |
| `OPENCODE_CONFIG` | Points to a config **file**, merged into the config chain; does **not** affect skill discovery (sst/opencode#3432) | Already honored for the global `opencode.json` edit target only — unchanged, deliberately plays no role in skill paths |

Empty-string values are treated as unset. Resolution stays in pure
functions taking `Option<PathBuf>` parameters (unit-testable without env
mutation); thin wrappers read the environment via `vendor::env_dir`.
The fallback chain per client is: env override → native default
(`$HOME`-derived) → workspace layout under `$GRIM_HOME`.

## Links

- [agentskills specification][agentskills-spec]
- [Claude Code skills docs][claude-skills-docs]
- [Claude Code memory / rules docs][claude-memory-docs]
- [GitHub Copilot custom instructions][copilot-instructions-docs]
- [OpenCode skills docs][opencode-skills-docs]
- [OpenCode rules docs][opencode-rules-docs]
- [OpenCode config docs][opencode-config-docs]
- Related ADR: [`adr_multifile_rules.md`](./adr_multifile_rules.md)
  (multi-file rule support directory — support files are copied verbatim
  for all clients; only the index is projected)

[agentskills-spec]: https://agentskills.io/specification
[claude-skills-docs]: https://code.claude.com/docs/en/skills
[claude-memory-docs]: https://code.claude.com/docs/en/memory
[copilot-instructions-docs]: https://docs.github.com/en/copilot/customizing-copilot/adding-custom-instructions-for-github-copilot
[opencode-skills-docs]: https://opencode.ai/docs/skills
[opencode-rules-docs]: https://opencode.ai/docs/rules
[opencode-config-docs]: https://opencode.ai/docs/config

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-06-10 | Michael Herwig | Initial draft, accepted |
| 2026-06-11 | Michael Herwig | Refinements: vendor trait structure, rule metadata map, common-vs-unique principle, Claude rule symmetry, unified universal render |
| 2026-06-11 | Michael Herwig | Refinements: completed Vendor trait surface (full method set + sync_config hook); vendor-native global layout decision (replaces GRIM_HOME global layout); Copilot global rules caveat; HOME-unresolvable fallback semantics |
| 2026-06-11 | Michael Herwig | Refinement: vendor directory env overrides — global-scope paths honor `CLAUDE_CONFIG_DIR` (full `~/.claude` replacement), `COPILOT_HOME` (full `~/.copilot` replacement), `OPENCODE_CONFIG_DIR` (additive scan dir, preferred when set); `OPENCODE_CONFIG` stays config-file-only |
