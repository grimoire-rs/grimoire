---
paths:
  - src/install/materializer.rs
  - src/install/expected_outputs.rs
  - src/install/render.rs
  - src/install/installer.rs
---

# Install Layout: Skills and Rules

Where a skill and a rule materialize per client. Part of the file-structure family; the index is [subsystem-file-structure.md](./subsystem-file-structure.md).

## Install Layout (client targets)

### Skills

A **skill** materializes as a directory tree under the client's `skills/`
dir. Every file in the tree is copied verbatim, **except** `SKILL.md` when
it carries tool-namespaced metadata keys (e.g. `claude.user-invocable` in
the `metadata` map). In that case `SKILL.md` is **rendered per client**:
known `<client>.<field>` keys are lifted to native typed top-level
frontmatter, foreign-namespace keys are dropped, and the written file is
marked `generated: true`. A plain skill with no tool-namespaced keys takes
the fast path and installs byte-identical. See `arch-principles.md` ADR
index → `adr_tool_namespaced_metadata_rendering.md`.

### Rules

A **rule** materializes as the index `<name>.md` under `rules/`, and —
when the artifact carries an optional sibling support directory — that
directory installs **beside** the index as `rules/<name>/…` so the index's
relative links resolve. The two on-disk roots (index file + sibling dir)
are one footprint: the integrity hash folds both, and uninstall removes
both. See `arch-principles.md` ADR index → `adr_multifile_rules.md`.

**Most clients decline rules** (`Vendor::kind_support(Rule) == Declined`) —
Codex, Gemini, Zed, Amp, the generic `agents` target, and five of the six
wave-2 clients (Goose, Warp, Droid, OpenClaw, Kilo; Droid and Warp also host MCP, and so does Cline at global scope only) lack an ownable
path-scoped instruction surface at all (AGENTS.md / GEMINI.md hierarchies, or
a UI-managed surface with no on-disk path). One decline is a grim
capability gap instead, not an upstream absence: Cline (the sixth
wave-2 client) documents a real `.clinerules/` surface with genuine
`paths:` scoping, declined only because this wave shipped skills-only. The
installer skips every one of them silently, writes no file, and records no output: a declined kind logs at
`debug` only (`installer.rs:558`), so nothing reaches stderr.
**Junie is Degraded, not Declined** — `.junie/rules/*.md` is a real
per-file directory grim can own; what it lacks is a per-file activation
key, so `paths` is dropped with a warning. **`docs/src/content/docs/clients.md` is the
enforced matrix** — a parity test reads it at test time, so trust it over
any prose list here. Background: `arch-principles.md` ADR index →
`adr_codex_vendor.md`.

**A kind can also be declined at one scope only** — `Vendor::kind_surface(kind,
scope) -> bool`, defaulted `true`, consulted from `client_supports_kind`
after `kind_support` has already passed. Two exceptions exist today, and
the complete set is pinned as `SCOPE_GAPS` in `vendor.rs`'s tests:
Junie rules at **global** scope (`.junie/rules/` exists; `~/.junie/rules/`
does not) and OpenClaw skills at **project** scope (its "project" path is a
fixed daemon home that does not track the repo). Add a scope gap here,
never by changing `kind_support`'s signature.

Per-client rule transforms:

- **Claude**: `paths:` is native Claude rule frontmatter. A plain rule
  carrying no tool-namespaced metadata keys installs verbatim, marked
  `generated: false` (fast path). A rule that carries any
  `<vendor>.<field>` entry inside its `metadata` map is re-rendered:
  own-namespace Claude keys lift per registry (empty today — unknown ones
  warn + drop), foreign vendor keys drop silently, plain keys survive.
  Written `generated: true`; if cleaned frontmatter is empty, the block
  is omitted entirely. A rule that ships a **support directory**
  additionally registers `**/.claude/rules/<name>/**` (absolute at global
  scope) in `claudeMdExcludes` — `<workspace>/.claude/settings.json` at
  project scope, `<claude_root>/settings.json` at global, where
  `<claude_root>` is the effective `CLAUDE_CONFIG_DIR` (shell or Claude's
  settings `env`, see the override table) else `$HOME/.claude` with **no
  workspace fallback**: unlike the rule-render path (which falls back to
  `<workspace>/.claude` when neither resolves), the sync is skipped
  entirely rather than writing a machine-absolute glob into a stray
  `settings.json` inside the repo. Claude discovers
  `rules/` recursively and auto-loads every unscoped file, so grim's own
  verbatim copy would otherwise be unconditional context. Managed like
  OpenCode's `instructions` glob and through the same driver
  (`managed_config.rs`) — added while the rule's output is recorded, removed
  when that output is **retired**, never a blanket `rules/*/**`. Removal is
  record-driven, never a filesystem probe for ownership: `sync_config` is handed the
  outputs the operation removed (`install_state::retired_outputs`), so grim
  can only ever remove an element it computed from a record it wrote — a
  consumer's own hand-written exclusion in the same git-tracked
  `settings.json` is untouchable, **except** one written in grim's exact
  spelling, which grim cannot tell from its own: it is adopted on install
  and removed when that rule is uninstalled (accepted, documented in the
  module doc). The filesystem is read once on the removal side, only to
  **decline**: a retired name whose support directory is still on disk keeps
  its element, so the three paths that drop a record while leaving the tree
  in place (an output resolving outside its anchor root, in `uninstall` or
  `reap_dropped_clients`; a shared footprint a surviving sibling still
  references) cannot strip the exclusion off a live tree. See
  `claude_config.rs`.
- **OpenCode**: frontmatter is stripped; the file written is a provenance
  comment followed by the rule body. Marked `generated: true`. Loading is
  wired through a managed glob entry in `opencode.json` (or `opencode.jsonc`
  when present). grim adds the entry when the first OpenCode rule installs
  and removes it when the last one uninstalls; the target file is
  `.opencode/rules/<name>.md`.
- **Copilot**: written to `.github/instructions/<name>.instructions.md`.
  Frontmatter maps `paths` → `applyTo` (comma-joined into a single string)
  and the optional `copilot.exclude-agent` key (authored in rule `metadata`)
  → `excludeAgent` (enum: `code-review` or `cloud-agent`). A rule with
  neither produces no frontmatter block at all. Marked `generated: true`.
- **Cursor**: written to `.cursor/rules/<name>.mdc` (native `.mdc`,
  `~/.cursor/rules/` at global). Frontmatter always present: `paths`
  comma-joins into the single `globs` STRING plus `alwaysApply: false`;
  unscoped → no `globs` and `alwaysApply: true`. Marked `generated: true`.
- **Kiro**: written to `.kiro/steering/<name>.md` (`~/.kiro/steering/` at
  global). Global steering ships inert (no per-file `fileMatch` scoping yet
  — watchlisted #9176) + warn. Marked `generated: true`.
- **Antigravity**: written to `.agents/rules/<name>.md`
  (`~/.gemini/config/rules/<name>.md` at global). Frontmatter always
  present — upstream silently discards a `rules/*.md` file without a valid
  `trigger`: `paths` comma-joins into the single `globs` STRING plus
  `trigger: glob`; unscoped → `trigger: always_on`, no `globs`. A
  `description` is emitted from `RuleFrontmatter::derive_description(body)`
  (rules have no `description` key). Upstream loads only the immediate `.md`
  children of `rules/`, so a support directory is inert unless the user lists
  it in `.agents/rules.json`.
  Marked `generated: true`.
- **Qoder**: written to `.qoder/rules/<name>.md` — Claude's shape (`paths:`
  native, verbatim fast path). Qoder loads `rules/**/*.md` recursively and
  documents **no usable** exclude key (`agentsMdExcludes` is named once, scope
  and format unstated), so a support directory over-loads exactly as
  Claude's did before `claudeMdExcludes` (#102) — a disclosed known gap
  (`clients.md#gap-qoder`), not repaired until upstream ships a key.

Support directory files are copied verbatim for every rule-supporting
client (Claude, OpenCode, Copilot, Cursor, Kiro, Antigravity, Qoder). Only the index is ever
transformed. The copy itself is never adjusted per client — where a client
would mis-read it, grim compensates in that client's own config (Claude, via
`claudeMdExcludes` above). Qoder is **known** to over-load it (recursive
`rules/` load, no usable exclude key); Antigravity does **not** by default
(it loads only the immediate `.md` children of `rules/`), but does once the
user lists the support directory in `.agents/rules.json`. **Whether the other four
do the same is unaudited**; Kiro `steering/` has no per-file scoping at all and is the
likeliest repeat.
