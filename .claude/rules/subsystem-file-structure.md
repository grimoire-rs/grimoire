---
paths:
  - src/**
---

# File Structure Subsystem

How Grimoire lays out its on-disk data: where downloaded artifacts, the
local index, and install links live under the data root.

## Design Rationale

- **Single data root.** All Grimoire state lives under one directory
  (default `~/.grimoire`, overridable via `GRIM_HOME`). Keeping everything
  under one root makes atomic rename / hardlink operations possible because
  source and destination stay on one filesystem.
- **Content-addressed storage.** Downloaded artifacts are addressed by
  content hash so identical content is stored once and is immutable.
- **Mutable namespace on top.** Human-facing names (tags, "installed"
  links) are a thin mutable layer pointing at immutable content.
- **Cross-device safety.** Operations that rely on same-filesystem
  atomic rename must validate the data root sits on a single volume;
  cross-device hardlink/rename fails and must be handled explicitly.

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

### MCP servers {#install-layout-mcp}

An **mcp** artifact never materializes a file. Install registers a
vendor-native entry in each client's own MCP config file via a
span-preserving splice (every byte outside the managed member — key
order, formatting, comments — survives): JSON/JSONC for every client but
Codex (`src/install/json_splice.rs`), TOML for Codex
(`src/install/toml_splice.rs`, built on `toml_edit`). Uninstall removes
only that entry, never the file, through the same format-specific
splice engine (dispatched on `Vendor::mcp_config_format`). Integrity is
judged **semantically** on the entry value (canonical sorted-key JSON
hash — even for the TOML target, whose entry is converted to JSON before
hashing), so reformatting the config is not a modification. Keys a
vendor writes into grim's own member (`Vendor::mcp_entry_vendor_owned_keys`
— Cline's `autoApprove`, `oauth` tokens, …; empty for everyone else) are
left out of that hash and of the adopt/refuse comparison, and every rewrite
carries them over from the member on disk; uninstall still removes the
whole member. Per-client
config files:

| Client | Project | Global |
|--------|---------|--------|
| **Claude** | `<workspace>/.mcp.json` (`mcpServers`) | `~/.claude.json` — `$CLAUDE_CONFIG_DIR/.claude.json` when set (`mcpServers`) |
| **OpenCode** | `<workspace>/opencode.json`/`.jsonc` (`mcp`) | `$OPENCODE_CONFIG` else XDG `opencode.json` (`mcp`) |
| **Copilot** | **two files, two outputs**: `<workspace>/.vscode/mcp.json` (`servers`, `${env:VAR}`) for VS Code's Copilot Chat, then `<workspace>/.github/mcp.json` (`mcpServers`, the global shape, `${VAR}` verbatim) for the Copilot **CLI**, which never reads the VS Code file — `Vendor::mcp_config_paths` + `mcp_entry_for`; a workspace `.mcp.json` (Claude's) hides `.github/mcp.json` from the CLI entirely (live-verified 1.0.88). oauth: `client_id` only → `oauth.clientId` / `oauthClientId` | `$COPILOT_HOME`\|`~/.copilot`/`mcp-config.json` (`mcpServers`); `${VAR}` written verbatim — Copilot CLI expands it (skipped before 2026-09-27) |
| **Codex** | `<workspace>/.codex/config.toml` (`mcp_servers`); only honored by Codex for **trusted** projects — grim writes it regardless, an untrusted project simply won't have it read | `$CODEX_HOME`\|`~/.codex`/`config.toml` (`mcp_servers`); HTTP/SSE headers map to `http_headers`/`env_http_headers`/`bearer_token_env_var`; a header embedding an env ref in text is unrepresentable → descriptor skipped; `timeout` (ms) → `startup_timeout_ms` |
| **Cursor** | `<workspace>/.cursor/mcp.json` (`mcpServers`); stdio needs `type: "stdio"`, env refs `${env:VAR}` | `~/.cursor/mcp.json` (`mcpServers`) |
| **Kiro** | `<workspace>/.kiro/settings/mcp.json` (`mcpServers`); `${VAR}` env refs native | `~/.kiro/settings/mcp.json` (`mcpServers`) |
| **Junie** | `<workspace>/.junie/mcp/mcp.json` (`mcpServers`); env-ref descriptors skipped (interpolation undocumented) | `$JUNIE_HOME`\|`~/.junie`/`mcp/mcp.json` (`mcpServers`) |
| **Gemini** | `<workspace>/.gemini/settings.json` (`mcpServers`); sse → `url`, http → `httpUrl`, `${VAR}` native; `timeout` dropped + warned (Gemini bounds every tool call with it; projected before 2026-09-27) | `~/.gemini/settings.json` (`mcpServers`) |
| **Zed** | `<workspace>/.zed/settings.json` (`context_servers`, flat shape); env-ref descriptors skipped (no upstream support) | `$XDG_CONFIG_HOME`\|`~/.config/zed` (unix) or `%APPDATA%\Zed` (Windows) `/settings.json` (`context_servers`, JSONC) |
| **Amp** | `<workspace>/.amp/settings.json` (`amp.mcpServers`, literal dotted key); `${VAR}` refs | `$XDG_CONFIG_HOME`\|`~/.config/amp`/`settings.json` (`amp.mcpServers`) |
| **Warp** | `<workspace>/.warp/.mcp.json` (`mcpServers`); stdio `command`/`args`/`env`/`working_directory` (from `cwd`), http + sse `url`/`headers`; descriptors with an env ref or any oauth field skipped (expansion undocumented, no oauth keys). Warp also reads Claude's `.mcp.json`/`~/.claude.json` and `.agents/.mcp.json` — double registration when both clients are selected | `~/.warp/.mcp.json` (`mcpServers`) |
| **Droid** | `<workspace>/.factory/mcp.json` (`mcpServers`); `type` always written; `disabled`/`disabledTools` vendor-owned (unhashed, carried over); `${VAR}` verbatim in `env`/`headers`/`oauth.clientId` (Droid expands only there) — a ref in `command`/`args`/`url` skips; oauth `client_id` only, else skip | `~/.factory/mcp.json` (`mcpServers`); Droid's UI copies a toggled project server here, so a same-named global install meets the untracked gate (65) |
| **Qoder** | `<workspace>/.qoder/settings.json` (`mcpServers`) — **not** the shared `.mcp.json` Qoder also reads (Claude's grim-managed file; one member, two vendors, two state outputs); Claude shape + `cwd`, `timeout` ms; env-ref descriptors skipped (expansion undocumented) | `$QODER_CONFIG_DIR`\|`~/.qoder`/`settings.json` (`mcpServers`) |
| **Cline** | — none upstream; `mcp_config_path` is `None`, so a project install warns + skips | `$CLINE_MCP_SETTINGS_PATH`, else `$CLINE_DATA_DIR/settings/`, else `$CLINE_DIR`\|`~/.cline` + `/data/settings/`, file `cline_mcp_settings.json` (`mcpServers`, flat, `${env:VAR}`) — shared by Cline CLI and extension; every splice (install and removal) holds Cline's own `<file>.lock` directory lock (`cline_lock.rs`) and re-reads under it; a held lock past 12 s → exit 75; anchors at `cline-root`, so a file relocated outside `~/.cline` is unanchorable → skipped |

Every non-Claude client declines the `ws` transport (skip + warn). The
structured `oauth` block is lossless-or-skip (`McpOAuth::unmapped`,
`adr_mcp_oauth_projection.md`): OpenCode, Zed, Copilot and Droid write it when they
carry every field it sets (Copilot and Droid: `client_id` only), every other client
skips it (Qoder documents both, in shapes grim cannot carry) — see
`docs/src/content/docs/clients.md` "Known gaps".

### Hooks {#install-layout-hooks}

A **hook** is a hybrid, and modelling it as either of its neighbours
mis-scopes half the work. It **materializes like Agent/Skill** (a payload
tree with a real recorded `ClientOutput`) and **registers like Mcp**
(foreign config, written through `sync_config`) — except more restrained,
because *the registration is never recorded at all*, plus a consent gate no
other kind has any precedent for — **workspace-scoped**, machine-local, and
written by exactly three seams (`grim hook allow`, `grim add`, an accepted
prompt). `grim install` deliberately writes none: it converges what is already
declared, and a cloned repository's `grimoire.toml` is not the user's gesture
(**T3**). Global scope is always consented and carries no record at all.

A hook payload is **machine-local at both scopes** (invariant I1):
`$GRIM_HOME/hooks/<name>/` globally, and
`$GRIM_HOME/hooks/payload/<workspace-key>/<name>/` for a workspace, where
`<workspace-key>` is the SHA-256 of the workspace path (hex) so two
workspaces sharing one `$GRIM_HOME` cannot collide. It is deliberately
**not** the dispatch `root_token`: a recorded install target is printed by
`grim status` and `grim install`, and a disclosed root token would let a
hostile repository's own registration fire the victim's already-armed hooks
(B3). A path key must be safe to print; a wire key must not be printable at
all. `payload` is therefore one of the `RESERVED_ARTIFACT_NAMES` entries — the
full set is `bin`, `consent`, `dispatch.json`, `dispatch.json.lock`, `payload`,
`root-key`, and it is deliberately **not** restated as a count anywhere: this
list fell one behind the layout three times (`root-key`, the lock sidecar, then
`consent`), each time because a prose enumeration was cheaper to leave alone
than to check. The array in
`oci::hook` is the source of truth, the refusal message renders itself from it,
and `hook_dispatch`'s `every_grim_owned_name_under_hooks_is_a_reserved_binding_name`
fails the build for a layout constant that is missing from it.

**The reservation guards the *binding* name, and only since the P-2 fix.**
The check is `oci::hook::binding_name_refusal`, applied in three places, and the
split matters: `HookManifest::validate` checks the manifest's own `name` at
`grim build`, on the **publisher's** machine; `installer::install_one` checks the
**binding** name before materialization, which is the control, because the
payload directory is `payload_dir(grim_home, root, &record.name)` over that
binding; `grim add` checks it too, as the friendlier error at the moment a user
types the name. `add` cannot be the control — a **bundle** picks its members'
binding names and they never pass through that command.

`binding_name_refusal` is **two rules, not one**. The reserved-name equality
check is the narrow half; the load-bearing half is that the name must parse as a
`SkillName`, whose grammar (`[a-z0-9]+([.-][a-z0-9]+)*`) makes every traversal
spelling *unrepresentable* rather than blocklisted. A blocklist has to anticipate
the next escape; a grammar does not. That half closed a confirmed
arbitrary-file-overwrite: a binding name of `../../../../victim` from a cloned
repository's `grimoire.toml` was joined onto `$GRIM_HOME` and materialized
*before* anchor classification ran, so the write landed and only then failed to
classify. The same gap for the **other kinds'** declaration keys is tracked
separately (issue #90) and is not hook-specific — `SkillName::parse` reaches CLI
names and bundle members, never a committed config's keys.

Until wave 8 only the build-time check existed, so it guarded the wrong string:
a hook bound as `bin` did materialize `$GRIM_HOME/hooks/bin/hook.toml`. The
write collision was survivable (convergence regenerates `bin/grim-hook`
afterwards in the same command, so grim's shim wins); the **reap** was not —
`grim uninstall --global hook bin` deleted the recorded output tree and took the
launcher with it, silently disarming every hook on the machine for every client
and every workspace, because the registered command's own
`[ -f "$L" ] && [ -x "$L" ] || exit 0` guard degrades to exit 0.

**Convergence derives this directory from `$GRIM_HOME` and the resolved
scope, never from the install record** — that derivation, not the
relocation, is what closes SEC-1. A record is attacker-supplied in the
cloned-repository case, so a payload directory read out of one is a
directory the attacker chose.

Grim writes several things directly under **`$GRIM_HOME/hooks/`**, and that root
is load-bearing rather than incidental. The table below covers the dispatch-side
files install writes; the namespace also holds the `payload/` tree (written by
materialization, also install-side) and two runtime-side families — the rotating
`hook_audit.jsonl{,.1}` trail and transient `payload_<pid>_<slot>.json` envelopes.
The install/runtime split matters because only the install-side names are covered
by a test that walks the directory; the runtime-side ones are covered by a test
that lists them. (No count is stated: a count beside the list
it counts is how this section, and the authoring reference, each went stale once.) **Adding a file here means
adding its name to `RESERVED_ARTIFACT_NAMES`** unless it is unrepresentable as a
binding name — a hook bound to a name grim already writes lands a directory on
that path and stops every hook on the machine from arming:

| Path | What it is |
|---|---|
| `$GRIM_HOME/hooks/dispatch.json` | The dispatch table — schema-versioned, keyed by an opaque per-install root token, each root carrying its `root` path and a `hooks` array of `(artifact, id, client, event, tier, matcher, handler, timeout, payload, payload_dir, resolved_digest)` rows. Sorted on `(artifact, id, event, client)` so a no-change rewrite is byte-identical |
| `$GRIM_HOME/hooks/bin/grim-hook` | The generated launcher: a `sh` script holding the absolute path of the `grim` binary, an `[ -f ] && [ -x ] \|\| exit 0` guard, and `exec "$G" hook "$@"`. Regenerated by `grim install`, never hand-edited |
| `$GRIM_HOME/hooks/root-key` | The root-token map backing the opaque `--root` lookup — a `0o600` regular file; a directory here is unrecoverable, since the mint path is `create_new` on the same path |
| `$GRIM_HOME/hooks/dispatch.json.lock` | The table's advisory-lock sidecar (`AdvisoryFileLock` appends `.lock` to the **full** file name). Reserved: a directory here makes `try_acquire` fail `EISDIR`, so no table is written for as long as the offending artifact is installed |
| `$GRIM_HOME/hooks/owned_files/<path-key>` | The **own-file ownership record**: the hex SHA-256 of the bytes grim last wrote to one own-file hook surface (Codex `hooks.json`, Copilot `hooks/grim.json`), keyed by `hook_dispatch::workspace_key` of the surface path. A surface is grim's only while it is a regular file whose bytes match; otherwise it is never written or deleted. The underscore keeps it outside the binding grammar, so it needs no reservation |
| `$GRIM_HOME/hooks/consent/<workspace-key>.json` | The **workspace consent record** — `{v, workspace, hooks, consented_at}`, `deny_unknown_fields` with every field required, so a truncated file cannot deserialize into a valid-looking one. One file per workspace, never one shared file: a whole-file rewrite would let one workspace's operation disarm every other (Decision E's own reasoning about the dispatch table), and per-workspace files remove the read-modify-write entirely — no lock, no cross-workspace blast radius. `<workspace-key>` is `hook_dispatch::workspace_key` **verbatim**, so the record and the payload tree agree by construction rather than by a second spelling of the same identity. The `workspace` field is the identity; the filename is a lookup index only, and a record whose `workspace` ≠ the resolved one is not consent for it. An unknown `v`, a parse error or an I/O error ≡ **absent** — logged at debug, never warned, never an error (I3). Not a `ClientOutput`, so it is invisible to the anchor/remainder table and to reaping; it is never garbage-collected, and `grim hook revoke` is its only remover |

**`GRIM_HOME` nesting is a refusal to arm here, not a caveat.** The
[project-state section](#install-state-project) records "GRIM_HOME must not
be nested inside a workspace directory" as a *state-record* caveat — a
mis-classified anchor. For hooks the same condition makes an **armable** file
repo-resident, which the threat model forbids outright, so grim **declines to
arm** and reports it: cause `grim-home-in-workspace` for a nested root and
`grim-home-relative` for a relative one (a relative root resolves against the
process CWD, which for a client-spawned `grim hook run` *is* the workspace).
Both are `state: not-armed`. Do not downgrade either to a warning.

The client-side registration is a `sync_config` **projection, never a
recorded `ClientOutput`** — deliberately not built by copying
`install_mcp`'s record-a-`ClientOutput`-per-registration shape, because that
shape is the orphaned-registration bug behind issues #54/#55. Claude splices
`.claude/settings.local.json` at project scope; Codex (`hooks.json`) and
Copilot (`hooks/grim.json`) get a whole file grim writes and deletes — but
only while its bytes match the SHA-256 grim recorded when it last wrote them
(`$GRIM_HOME/hooks/owned_files/`). Any other file at that path is the user's:
never overwritten or deleted, and reported `not-armed` / `surface-user-owned`. Only three of eighteen clients
arm at all, and only Claude at project scope — the reason is not scheduling
but that a committed registration makes the *executed binary path*
environment-derived (CWE-426), which every grim control is downstream of.
`docs/src/clients.md` `{#gap-hooks}` is the enforced matrix; trust it over
any prose here.

Payload-directory pruning goes through `prune::shared_by_surviving_sibling`,
the same N-outputs → one-destination refcount shared-pool skills already use
(hooks are **not** the first `entry: None` shared destination). The refcount
is record-only with no filesystem fallback, so a lost or partial record must
never delete a payload another client still references.

### Agents {#install-layout-agents}

An **agent** materializes as a single file in the client's agents
directory (no support directory). For Claude/OpenCode/Copilot/Cursor/Gemini/Qoder
it is a Markdown file (Claude and Qoder install a plain agent verbatim; the others
project the frontmatter, each lifting its own `<vendor>.*` field registry —
e.g. `cursor.readonly`, `gemini.temperature`). For **Codex** it is a
**TOML** file (`<name>.toml`) — Codex is the only TOML-emitting vendor: the
canonical `name`/`description` plus the agent body (as
`developer_instructions`) and an optional `model` are serialized to TOML;
the `tools` field has no Codex equivalent and is dropped with a warning.
For **Antigravity** it is a Markdown file too, with `tools` emitted as a
YAML sequence (upstream types it `string[]`) and nothing lifted — the
`antigravity.*` registry is empty. **Goose** writes `name`/`description`/`model`
only (`tools` dropped + warning, empty `goose.*` registry) into its own
`.goose/agents/`, never the `.agents/agents/` Antigravity owns. **Kilo** takes
OpenCode's shape — filename is the identity, `name` and `tools` dropped,
`kilo.*` lifted — and the same install-time `color`/`steps` drop as OpenCode
(`drop_invalid_opencode_values`; Kilo skips just the invalid agent, OpenCode its
whole config). **Junie** is Markdown with `tools` as a
YAML sequence and `junie.*` lifted to camelCase keys; an agent name outside
Junie's `[a-z][a-z0-9_-]*` is skipped for Junie only. **Droid** writes a
custom droid at `.factory/droids/<name>.md`: `tools` as a YAML list, `model`
verbatim (`inherit` included), `droid.reasoning-effort` → `reasoningEffort`;
a name outside Droid's `[a-z0-9_-]+` (a `.`) is skipped for Droid only. Both
skips go through `Vendor::agent_name_grammar`, checked by
`installer::client_hosts`, so they never count as expected output.
**Every other client declines agents**
(`kind_support == Declined`): CLI/IDE schema collision (Kiro);
Cline's CLI surface documents an installable
format grim does not render yet; ACP/runtime-only (Zed, Amp, OpenClaw); or
no installable format at all (Warp and the generic `agents` target) —
installer skips silently and records no output, logging at
`debug` only.

Per-client agent paths:

| Client | Global agent path |
|--------|-------------------|
| **Claude** | `<claude_root>/agents/<name>.md` |
| **Copilot** | `<copilot_root>/agents/<name>.md` |
| **OpenCode** | `<opencode_root>/agents/<name>.md` |
| **Codex** | `<codex_root>/agents/<name>.toml` |
| **Cursor** | `~/.cursor/agents/<name>.md` (project `.cursor/agents/`) |
| **Gemini** | `<gemini_root>/agents/<name>.md` (project `.gemini/agents/`) |
| **Antigravity** | `~/.gemini/config/agents/<name>.md` (project `.agents/agents/`) |
| **Qoder** | `<qoder_root>/agents/<name>.md` (project `.qoder/agents/`) |
| **Goose** | `~/.goose/agents/<name>.md` (project `.goose/agents/`; `$GOOSE_PATH_ROOT` does not move it) |
| **Kilo** | `$XDG_CONFIG_HOME\|~/.config/kilo/agents/<name>.md` (project `.kilo/agents/`) — **not** the `~/.kilo` skills root |
| **Junie** | `~/.junie/agents/<name>.md` (project `.junie/agents/`, never the shared `.agents/`) |
| **Droid** | `~/.factory/droids/<name>.md` (project `.factory/droids/`) |
| **everyone else** | declined — no agent surface |

`opencode_root` is the parent of the OpenCode skills directory (i.e. the
directory one level above the `skills/` subdir resolved from
`$OPENCODE_CONFIG_DIR` or the XDG default). `claude_root`, `copilot_root`,
`codex_root`, `gemini_root` and `qoder_root` are the vendor roots in the
[`VendorRoot` table](#path-anchor-set) — `$CODEX_HOME` else `~/.codex`,
`$GEMINI_CLI_HOME/.gemini` else `~/.gemini`, and `$QODER_CONFIG_DIR` else
`~/.qoder`.

### Global-scope paths {#global-scope-paths}

For a **global-scope** install (`--global`), grim writes into each
client's **native** user-level discovery directory rather than under
`$GRIM_HOME`, so the files are found without extra configuration:

| Client | Skills root | Rules path | Agents path |
|--------|-------------|------------|-------------|
| **Claude** | `~/.claude/skills/<name>/` | `~/.claude/rules/<name>.md` | `~/.claude/agents/<name>.md` |
| **OpenCode** | `$XDG_CONFIG_HOME/opencode/skills/<name>/` | `$GRIM_HOME/.opencode/rules/<name>.md` (absolute glob registered in global `opencode.json`) | `$XDG_CONFIG_HOME/opencode/agents/<name>.md` |
| **Copilot** | `~/.copilot/skills/<name>/` | `~/.copilot/instructions/<name>.instructions.md` (native; workspace-layout fallback + warn only when no root resolves) | `~/.copilot/agents/<name>.md` |
| **Codex** | `$HOME/.agents/skills/<name>/` (cross-vendor standard; independent of `$CODEX_HOME`) | **unsupported** — Codex has no path-scoped rule mechanism; grim skips it silently, writes no file | `$CODEX_HOME`\|`~/.codex/agents/<name>.toml` (TOML) |
| **Cursor** | `~/.cursor/skills/<name>/` | `~/.cursor/rules/<name>.mdc` | `~/.cursor/agents/<name>.md` |
| **Kiro** | `~/.kiro/skills/<name>/` | `~/.kiro/steering/<name>.md` | declined |
| **Junie** | `<junie_root>/skills/<name>/` (`$JUNIE_HOME` else `~/.junie`) | declined | `<junie_root>/agents/<name>.md` |
| **Gemini** | `$HOME/.agents/skills/<name>/` (shared pool) | declined | `<gemini_root>/agents/<name>.md` |
| **Zed** | `$HOME/.agents/skills/<name>/` (shared pool) | declined | declined |
| **Amp** | `$HOME/.agents/skills/<name>/` (shared pool) | declined | declined |
| **agents** | `$HOME/.agents/skills/<name>/` (shared pool — its only surface) | declined | declined |
| **Antigravity** | `~/.gemini/config/skills/<name>/` — **not** the pool at global scope | `~/.gemini/config/rules/<name>.md` | `~/.gemini/config/agents/<name>.md` |
| **Goose** | `$HOME/.agents/skills/<name>/` (shared pool, both scopes) | declined | `~/.goose/agents/<name>.md` |
| **Cline** | `~/.cline/skills/<name>/` | declined | declined |
| **Droid** | `~/.factory/skills/<name>/` (native by default; the pool only via `shared_skills`) | declined | `~/.factory/droids/<name>.md` |
| **Warp** | `~/.warp/skills/<name>/` (native by default; the pool only via `shared_skills`) | declined | declined |
| **OpenClaw** | `<openclaw_root>/skills/<name>/` (`$OPENCLAW_STATE_DIR`, else `$OPENCLAW_HOME/.openclaw`, else `~/.openclaw`) — **global-only client**, project scope writes nothing | declined | declined |
| **Kilo** | `~/.kilo/skills/<name>/` (native by default; the pool only via `shared_skills`) | declined | `$XDG_CONFIG_HOME\|~/.config/kilo/agents/<name>.md` |
| **Qoder** | `<qoder_root>/skills/<name>/` | `<qoder_root>/rules/<name>.md` | `<qoder_root>/agents/<name>.md` |

`$XDG_CONFIG_HOME` falls back to `~/.config` when unset. A client whose
`[options.vendors.<name>].shared_skills` is set writes its skills to
`$HOME/.agents/skills/<name>/` instead of the root above; the opt-in is
accepted only for a **verified pool reader** (`POOL_CAPABLE_VENDORS` in
`src/install/vendor.rs`), and setting it on any other client is exit **65**
at `grim config set` / **78** at load.

**Vendor config-dir env vars that are NOT honored** (paths hardcode the
documented native root), each for its own reason:

- `CURSOR_CONFIG_DIR` — documented only as the location of the CLI's own
  `cli-config.json`, never tied to the directories grim writes (skills,
  rules, agents, `mcp.json`).
- the `JUNIE_*_LOCATIONS` family — additive, not untested: it only adds
  search paths, so grim's defaults stay read. (`JUNIE_HOME`, which replaces
  `~/.junie` outright, **is** honored — see the override table.)
- `GEMINI_CONFIG_DIR` — genuinely does not exist upstream (only FR #2815).
  The variable that *does* exist is `GEMINI_CLI_HOME`, which grim honors —
  see the override table below.
- `$AMP_SETTINGS_FILE` — **contested, not disproven.** Amp's official
  manual documents only `--settings-file`, but a third-party string
  extraction from Amp's real shipped bundle lists the variable among its
  env vars. No source addresses its precedence relative to
  `.amp/settings.json`, or whether it names a file or a directory.
  Shipping behaviour on that evidence would be a coin flip, so behaviour
  is unchanged — the reason is *unaddressed precedence*, not
  *nonexistence*. Do not "correct" this back to "no such variable exists".

All are watchlisted for re-verification — see
`vendor-capability-watchlist.md`.

**XDG resolution is platform-dependent for Zed.** Zed and Amp both root
their global settings under `$XDG_CONFIG_HOME` (falling back to
`~/.config`), **but Zed consults XDG on Linux and FreeBSD only**:
upstream's `config_dir()` reads XDG on that branch alone, falls to a
literal `~/.config/zed` on macOS, and uses `%APPDATA%\Zed` on Windows.
`vendor_zed::zed_root_from` mirrors that split through an explicit
`ZedRootKind`, so every arm is testable on any host. **Amp is a separate
question and it is unresolved:** source-tier verification is unachievable
(compiled binary, stub npm package, no public repo), and the full manual
greps to zero XDG hits while documenting `~/.config/amp/` identically for
macOS and Linux. The shared `xdg_config_dir()` helper is therefore left
alone — making it platform-aware would move Amp's macOS resolution on
zero evidence.

**Vendor env overrides** (each client's own variable; the directory
variables are honored read-only, `OPENCODE_CONFIG` names a file grim reads
**and** rewrites; empty value = unset):

| Variable | Effect on global paths |
|----------|------------------------|
| `CLAUDE_CONFIG_DIR` | Replaces the entire `~/.claude` tree — Claude skills, rules, and agents root there. Also relocates the global MCP registration file to `$CLAUDE_CONFIG_DIR/.claude.json`. **Also read from Claude's own settings `env`** (`vendor_claude::config_dir_override`, the one resolver every Claude path uses): managed `managed-settings.json` then `managed-settings.d/*.json` (name order, last wins) > user `settings.json` in the shell-resolved root > shell > `~/.claude` — settings beat the shell because Claude writes each `env` entry over the inherited value (code.claude.com/docs/en/env-vars "Precedence"). Project/local settings are never read (Claude ≥ 2.1.251 ignores them for this key). Values are untrusted input: absolute, no `..`, nothing expanded; any failure drops that layer at `debug`. Residual gaps: MDM/registry/server-managed policy (unreadable), `claude --settings` (flagSettings, per-session), a second hop (`<new root>/settings.json` setting it again — grim follows one), and a Windows rooted-without-drive `\x` (not `is_absolute`, ignored). Memoized per process (`OnceLock`) so one run never splits across roots. The managed dir is injected (`config_dir_from`) so tests never touch system paths |
| `COPILOT_HOME` | Replaces `~/.copilot` — Copilot skills and agents land under `$COPILOT_HOME/`. VS Code's embedded Copilot CLI honors it since VS Code 1.132.0 (microsoft/vscode#314917). **Caveat:** set to anything but `~/.copilot`, Copilot stops scanning `~/.agents/skills`, so `shared_skills` output goes unread (watchlist) |
| `OPENCODE_CONFIG_DIR` | OpenCode's additive scan dir — preferred over the XDG default for skills and agents when set |
| `OPENCODE_CONFIG` | Config **file** path only (global `opencode.json` edit target); no effect on skill/agent paths |
| `CODEX_HOME` | Replaces `~/.codex` — Codex **agents** root **and** the MCP `config.toml` there. Does **not** relocate Codex skills (those follow the `$HOME/.agents/skills` cross-vendor standard) |
| `KIRO_HOME` | Replaces `~/.kiro` **outright, no `.kiro` segment appended** — the `CODEX_HOME` shape. Kiro skills, `steering/` rules, and `settings/mcp.json` all follow it. grim follows the Kiro **CLI**; the Kiro **IDE** still hardcodes `~/.kiro` and ignores the variable (kirodotdev/Kiro#9148) — that is an upstream fact, not a limit on what grim honors. A user who sets it *and* uses the IDE gets output where the CLI reads it, not the IDE |
| `QODER_CONFIG_DIR` | Replaces `~/.qoder` **outright** — the `KIRO_HOME` shape. Qoder skills, rules, agents, and `settings.json` all follow it. Always honored since Qoder support landed, so **no** `relocated_vendor_roots` row |
| `JUNIE_HOME` | Replaces `~/.junie` **outright** — the `KIRO_HOME` shape. Junie skills, agents, and `mcp/mcp.json` follow it (`vendor_junie::junie_root`, the one resolver) |
| `OPENCLAW_STATE_DIR`, `OPENCLAW_HOME` | `OPENCLAW_STATE_DIR` names the state root itself and wins; else `OPENCLAW_HOME` replaces the home directory, so the root is `$OPENCLAW_HOME/.openclaw` (the `GEMINI_CLI_HOME` shape). Skills land in `<root>/skills`. A leading `~` expands to the real home (upstream does the same); any other relative value is ignored at `debug`, never CWD-resolved |
| `GEMINI_CLI_HOME` | **The opposite shape.** It replaces Node's `os.homedir()`, and Gemini then joins `.gemini` onto it — so the root is `$GEMINI_CLI_HOME/.gemini`, with the segment still appended. Relocates Gemini's `agents/` and `settings.json`. Deliberately does **not** relocate the shared `.agents/skills` pool, which stays keyed on the real `$HOME` (see below) |

**The two shapes are opposites — do not conflate them.** `KIRO_HOME` and
`CODEX_HOME` *replace* their directory; `GEMINI_CLI_HOME` replaces the
*home directory* and the vendor segment is still appended.

**The shared pool never follows a vendor override.** `AgentsSkills` is
keyed on the real `$HOME` for every member. Upstream Gemini *does* derive
its own pool from the overridden homedir, so this is a knowing divergence:
grim writes **one** physical pool tree, deduped to a single destination and
released by `prune.rs`'s refcount guard, and a per-vendor pool root would
fork that tree and break the one-path/N-outputs shape both the guard and
the installer's dest-dedup rest on. **Residual gap, open:** a user who sets
`GEMINI_CLI_HOME` gets pool skills at `$HOME/.agents/skills` while Gemini
reads `$GEMINI_CLI_HOME/.agents/skills`. Watchlisted.

**A newly honored override is a layout move.** `KIRO_HOME`,
`GEMINI_CLI_HOME`, `JUNIE_HOME`, `OPENCLAW_STATE_DIR`/`OPENCLAW_HOME`, the
settings-sourced `CLAUDE_CONFIG_DIR`, and the Zed macOS XDG correction all relocate a root
for users who already set the variable, so `installer.rs::reap_relocated_roots`
sweeps the pre-override root on install, update, and uninstall.
`relocated_vendor_roots` is the closed table of roots that actually moved —
**never add a row for a variable grim always honored** (the shell
`CLAUDE_CONFIG_DIR`, `COPILOT_HOME`, `CODEX_HOME`, `OPENCODE_CONFIG_DIR`): their pre-override
location is the *default* root, which grim itself very likely populated in
an earlier no-override session, and a row would delete those copies. The
Claude rows are minted only when the settings-derived value differs from the
shell-only one, and move **both** Claude roots (`claude-root` and
`claude-user-dir`, the latter via the `CLAUDE_USER_DIR_ROW` pseudo-row).

**Fallback**: env override → native default (`$HOME`-derived) → workspace
layout under `$GRIM_HOME` for the affected client.

## Install State {#install-state}

Grimoire records what it installed, where, and at what content hash. The
record location differs by scope.

### Project state {#install-state-project}

Project install state lives at `<workspace>/.grimoire/state.json` — inside
a `.grimoire/` directory co-located with `grimoire.toml`. The workspace
directory is the key; there is no content hash of the config path in the
filename. Each project has exactly one state file at this fixed location,
so two projects sharing a common ancestor (or a shared `GRIM_HOME` volume)
cannot collide.

**Self-managed `.gitignore`**: the first time grim creates the `.grimoire/`
directory it writes `.grimoire/.gitignore` with contents `*` (if absent —
never overwrites a user-edited one). The consumer's root `.gitignore` is
never touched. This mirrors the convention used by [uv] (`.venv/.gitignore`)
and [pixi] (`.pixi/.gitignore`): the tool owns its dot-dir and excludes
its own contents from version control.

**Devcontainer named-volume caution**: if a devcontainer mounts a named
Docker volume at `<workspace>/.grimoire`, that volume shadows the
bind-mounted workspace state. A `grim install` inside the container writes
to the named volume, which is invisible to the host and to other containers
that bind-mount the same workspace directory. Use a bind-mount (not a
named volume) at `<workspace>/.grimoire` if you need state to be shared.

**Non-UTF-8 path components**: any path component that is not valid UTF-8
is rejected at store time with `UnknownAnchor`. All anchor roots must be
representable as UTF-8.

**Reap window**: between the read-only `load()` in a first post-upgrade
`status` call and the mutating `save()` in the next mutating command, the
old legacy state file (`$GRIM_HOME/state/projects/<sha>.json`) still
exists. A concurrent observer looking at the legacy path during this window
may see the old file even though the in-memory view is already migrated.
This is transient; the next mutating command reaps the legacy file.

**Nesting constraint**: `GRIM_HOME` must not be nested inside a workspace
directory. If it is, `from_target` may match `GrimHome` as the anchor for
a path that should classify as `Workspace`, producing an incorrect record.
For **hooks** the same condition is not a record caveat but a hard
**refusal to arm** — see [Hooks](#install-layout-hooks).

### Global state {#install-state-global}

Global install state lives at `$GRIM_HOME/state/global.json`. This
location is unchanged from previous versions.

**Residual risk under a shared GRIM_HOME**: when two machines or containers
share the same `GRIM_HOME` volume, both read and write the same
`global.json`. Anchoring makes the stored *paths* portable (each machine
resolves anchor roots from its own environment), but the *record set* in
the file is shared. Concurrent or serial `grim install --global` calls
from different machines are last-writer-wins on the record set — the same
class of collision that project state now avoids. **v1 stance: single
writer at a time.** Per-host segmentation (keyed by a machine identity
such as `devcontainerId`) is a tracked follow-up, not part of this change.
`atomic_write` prevents partial-file corruption; only record-set
last-writer-wins is a residual risk.

### PathAnchor set {#path-anchor-set}

Stored paths are anchor-relative rather than absolute, so a state file
written on one machine resolves correctly on another (portable `$HOME`,
devcontainer portability). Every stored path carries an `anchor` tag and a
`relative` string (forward-slash UTF-8, Normal components only — no `.`,
`..`, leading `/`, or drive prefix).

The anchor set is **six fixed variants plus one parameterized
`VendorRoot(&'static str)`**. A vendor root serializes as
`<vendor-name>-root` — `cursor-root`, `kiro-root`, `antigravity-root` —
with **no `vendor:` prefix**. That is the same on-disk vocabulary every
shipped `state.json` already carries, because `Vendor::name()` equals each
former variant's tag prefix; collapsing the per-vendor variants into one
parameterized variant was a pure internal refactor with **zero on-disk
change**.

Fixed anchors:

| Anchor | Serde tag | Resolved root |
|--------|-----------|---------------|
| `Workspace` | `workspace` | The workspace directory passed to the CLI |
| `GrimHome` | `grim-home` | `$GRIM_HOME`. Also the universal fallback: appended to every pair's candidate list |
| `ClaudeUserDir` | `claude-user-dir` | the effective `CLAUDE_CONFIG_DIR` (shell or Claude settings `env`) else `$HOME` — the dir holding Claude's user config file `.claude.json`. A *second*, differently-shaped root for one vendor, so it cannot be a `VendorRoot` row: with the override set the file lives *inside* it, without it the file is a *sibling* of `~/.claude` |
| `AgentsSkills` | `agents-skills` | `$HOME/.agents/skills` — the cross-vendor shared skills pool. Belongs to no single vendor, the root already ends in `/skills` (so `relative` is the bare skill name), and it is **never** relocated by a vendor `*_HOME` |
| `OpenCodeSkills` | `open-code-skills` | `$OPENCODE_CONFIG_DIR/skills` else `$XDG_CONFIG_HOME/opencode/skills` — a skills dir one level *below* the config root |
| `OpenCodeRoot` | `open-code-root` | Parent of the `OpenCodeSkills` root. Derived at lookup time, so there is no stored root and **`opencode` must never get a `VENDOR_ROOTS` row** — that would be two spellings of one location in `state.json`, which the reaper and the prune refcount treat as distinct outputs |

Note the two OpenCode tags: `rename_all = "kebab-case"` split the variant
names on their internal capital, so the on-disk tags are `open-code-*`, not
`opencode-*`. `Display` was aligned onto serde (error text only; the tags
never moved).

`VendorRoot` rows live in `VENDOR_ROOTS` (`src/install/path_anchor.rs`),
one per vendor whose global root is a plain `Option<PathBuf>`. Each row is
`(name, fn(EnvLookup, Option<PathBuf>) -> Option<PathBuf>)` — a pure
function of injected env and home, so the wiring is hermetically testable;
**never call `env_dir`/`home_dir` inside a row.** The resolvers run exactly
once, inside `AnchorRoots::resolve`, which is the single place ambient env
is read.

| Vendor row | Tag | Resolved root |
|---|---|---|
| `claude` | `claude-root` | the effective `CLAUDE_CONFIG_DIR` (shell or Claude settings `env`) else `~/.claude` — `AnchorRoots::resolve` feeds the row the settings-aware value |
| `copilot` | `copilot-root` | `$COPILOT_HOME` else `~/.copilot` |
| `codex` | `codex-root` | `$CODEX_HOME` else `~/.codex` (hosts Codex `agents/` **and** the MCP `config.toml`) |
| `cursor` | `cursor-root` | `~/.cursor` (`CURSOR_CONFIG_DIR` not honored; hosts skills, `.mdc` rules, agents, `mcp.json`) |
| `kiro` | `kiro-root` | `$KIRO_HOME` else `~/.kiro` (hosts skills, `steering/` rules, `settings/mcp.json`) |
| `junie` | `junie-root` | `$JUNIE_HOME` else `~/.junie` (hosts skills, `agents/`, `mcp/mcp.json`) |
| `gemini` | `gemini-root` | `$GEMINI_CLI_HOME/.gemini` else `~/.gemini` — segment appended either way (hosts `agents/`, `settings.json` MCP; skills use `AgentsSkills`) |
| `zed` | `zed-root` | Linux/FreeBSD: `$XDG_CONFIG_HOME` else `~/.config`, then `/zed`; macOS: literal `~/.config/zed`; Windows: `%APPDATA%\Zed` (hosts `settings.json` MCP; skills use `AgentsSkills`) |
| `amp` | `amp-root` | `$XDG_CONFIG_HOME` else `~/.config`, then `/amp` (hosts `settings.json` MCP; skills use `AgentsSkills`) |
| `antigravity` | `antigravity-root` | `~/.gemini/config` — nested under, and distinct from, the `gemini` row. Each client's candidate set holds only its own root, so the nesting never cross-classifies |
| `cline` | `cline-root` | `~/.cline` |
| `droid` | `droid-root` | `~/.factory` — **the tag follows the CLIENT name, the directory follows the vendor's.** Both are frozen and they differ on purpose (hosts skills, `droids/` agents, `mcp.json`) |
| `warp` | `warp-root` | `~/.warp` (identical on macOS, Linux and Windows, deliberately so upstream; hosts skills, `.mcp.json` MCP) |
| `openclaw` | `openclaw-root` | `$OPENCLAW_STATE_DIR`, else `$OPENCLAW_HOME/.openclaw`, else `~/.openclaw` |
| `kilo` | `kilo-root` | `~/.kilo` (the legacy `.kilocode` is read for detection only, never written) |
| `kilo-config` | `kilo-config-root` | `$XDG_CONFIG_HOME` else `~/.config`, then `/kilo` — Kilo's **second** root, hosting its global `agents/`. The `opencode-config` precedent: one vendor, two roots, so the second row is not named after the vendor |
| `goose` | `goose-root` | `~/.goose` — hosts Goose's global `agents/` only; its skills anchor at `AgentsSkills` |
| `qoder` | `qoder-root` | `$QODER_CONFIG_DIR` else `~/.qoder` (hosts skills, rules, agents, `settings.json` MCP) |

`goose`'s skills render into the shared pool at both scopes and anchor at
`AgentsSkills`; its `goose-root` row exists for agents alone. The
vendor-neutral `agents` client has no row.

All roots are resolved once at scope-resolution time and passed as an
`AnchorRoots` struct — now a `BTreeMap<&'static str, PathBuf>` rather than
per-vendor fields — so every downstream operation is a pure table-lookup
with no ambient environment access. An absent vendor is an absent key.

**A row's name is an on-disk contract.** Rows may be appended; a name may
never be changed or removed (Principle 9). Adding a vendor costs one row
plus its `(client, kind)` arms in `candidate_anchors`, an entry in
`SHIPPED_ANCHOR_TAGS`, and a row in `hermetic_vendor_roots` — no enum
variant, no `AnchorRoots` field, no fixture churn. Never add a row whose
`<name>-root` collides with a fixed tag: the fixed arms of
`from_serde_tag` are matched first, so a colliding row would be written
under its own name and silently read back as the *other* anchor.

### Anchor root/remainder table {#anchor-remainder-table}

Authoritative mapping from `(scope, client, kind)` to `(anchor, stored relative)`:

Anchors are named below by their **serde tag** — what actually lands in
`state.json`.

| Scope · client · kind | Anchor | Stored `relative` |
|---|---|---|
| project · any · any **except `hook`** | `workspace` | `.claude/…`, `.opencode/…`, `.github/…`, `.agents/…`, `.codex/…` (full sub-path from workspace) |
| project · any · hook | `grim-home` | `hooks/payload/<sha256-of-workspace>/<name>` — **the one project triple that does not anchor at `workspace`** (I1). `workspace` stays in the candidate set, second, so pre-relocation records still classify for the layout reaper |
| global · any · hook | `grim-home` | `hooks/<name>` (client-independent — S-003) |
| global · claude · skill | `claude-root` | `skills/<name>` |
| global · claude · rule | `claude-root` | `rules/<name>.md` |
| global · claude · agent | `claude-root` | `agents/<name>.md` |
| global · copilot · skill | `copilot-root` | `skills/<name>` |
| global · copilot · agent | `copilot-root` | `agents/<name>.md` |
| global · opencode · skill | `open-code-skills` | `<name>` (root already ends `/skills`) |
| global · opencode · agent | `open-code-root` | `agents/<name>.md` |
| global · opencode · rule | `grim-home` | `.opencode/rules/<name>.md` |
| global · copilot · rule | `copilot-root` | `instructions/<name>.instructions.md` (the `grim-home` fallback classifies pre-move records for the layout-migration reaper) |
| global · codex · skill | `agents-skills` | `<name>` (root already ends `/skills`) |
| global · codex · agent | `codex-root` | `agents/<name>.toml` |
| · codex · rule | — | **not classified** — declined at the `kind_support` gate before anchoring; no output recorded |
| project · any · mcp | `workspace` | client-specific config path from the [MCP table](#install-layout-mcp) (`.mcp.json`, `.cursor/mcp.json`, `.kiro/settings/mcp.json`, …); entry-typed output — `entry` carries the managed member's JSON pointer |
| global · claude · mcp | `claude-user-dir` | `.claude.json` |
| global · opencode · mcp | `open-code-root` | `opencode.json` |
| global · copilot · mcp | `copilot-root` | `mcp-config.json` |
| global · codex · mcp | `codex-root` | `config.toml` (same root as the agent anchor — TOML splice, see [install-layout-mcp](#install-layout-mcp)) |

**Every other vendor** follows the same `(scope, client, kind)` →
`(anchor, relative)` shape, with `<name>-root` as its global anchor and the
`relative` remainder taken verbatim from the Install Layout and
[Global-scope](#global-scope-paths) tables above — e.g. global · cursor ·
rule → `cursor-root` + `rules/<name>.mdc`; global · kiro · mcp →
`kiro-root` + `settings/mcp.json`. Three exceptions worth stating:

- **Global pool skills anchor at `agents-skills`, never at a vendor
  root** — Codex, Gemini, Zed, Amp, Goose, the generic `agents` client,
  and any client whose `[options.vendors.<name>].shared_skills` opt-in is
  active. Goose's `goose-root` hosts only its agents. (At *project* scope every
  triple anchors at `workspace`, pool or not; the pool shows up only in
  the `relative` remainder, `.agents/skills/<name>`.)
- **Antigravity is a partial pool member.** Its *project* skills pool into
  `.agents/skills`; its *global* skills anchor at
  `antigravity-root` (`~/.gemini/config/skills/<name>`). Nothing in the
  installer or prune assumes scope-uniform pool membership — both key on
  resolved destination paths, not on a roster. **Pool *capability* is
  scope-blind**, though: a partial member must not join
  `POOL_CAPABLE_VENDORS`, or an opt-in would write global skills where the
  client never scans, and nothing would fail.
- **The opt-in classifies both layouts for one triple.**
  `candidate_anchors` cannot see the config, so a pool-capable client's
  skill triple returns `[native root, agents-skills, grim-home]` — native
  first, so a root tie keeps the client's own anchor rather than silently
  re-anchoring a user who never opted in.

Declined `(client, kind)` pairs are never classified — skipped at the
`kind_support` gate before anchoring. A pair declined only *at one scope*
(`Vendor::kind_surface(kind, scope) == false`, today Junie rules at global
and OpenClaw skills at project) is likewise never classified at that scope:
the installer warns, skips, and records zero outputs.

### Path containment guard {#path-containment-guard}

`AnchoredPath::resolve` enforces containment through two layers before any
filesystem operation runs on the joined path:

**Layer 1 (always, works for absent paths)**: every component of `relative`
must be `Normal`. Any `ParentDir` (`..`), `CurDir` (`.`), `RootDir`, or
`Prefix` component causes an immediate `TraversalAttempt` error without
touching the filesystem.

**Layer 2 (only when the candidate path exists)**: `dunce::canonicalize`
resolves both the candidate path and the anchor root, then asserts
`candidate.starts_with(anchor_root)` at the component boundary. A symlink
inside the anchor pointing outside it yields `EscapedAnchor`.

No consumer joins anchor + relative manually. Every filesystem operation
(read, hash, delete) receives the result of `resolve()`, never the raw
`relative` string.

The escape verdict is caller-typed (`Containment`): `Strict` for anything
that deletes, `AllowRelocatedAncestor` for read-only probes through a
symlinked ancestor (stow/yadm layouts, Unix only), and
`AllowRelocatedFile` — chosen by `ClientOutput::resolved_target` for every
**entry** output regardless of the caller — because a managed MCP member is
only ever read or spliced in place, never deleted, so a dotfiles-linked
vendor config (`~/.codex/config.toml → ~/dotfiles/…`) is the user's layout,
not an escape. The same reasoning is why the three user-authored files
grim rewrites (`grimoire.toml`, `grimoire.lock`, the docker credential
config) and every vendor-config splice go through
`atomic_write_through_symlink`, while grim-owned outputs keep the plain
rename that clobbers a planted link (issue #117).

See `quality-security.md` for the path-traversal and symlink-escape guard
principles that this two-layer pattern implements.

## Client Detection (default install targets) {#client-detection}

When neither `--client` nor the config `[options].clients` selects a
client, `install` / `update` / TUI target **all detected clients**. A
client is detected when its vendor directory / config marker is present
for the active scope:

| Client | Project signal | Global signal |
|--------|----------------|---------------|
| **Claude** | `<workspace>/.claude` **or** `<workspace>/.mcp.json` (a grim-managed MCP config is still a real Claude footprint, even without `.claude/`) | native root (`$CLAUDE_CONFIG_DIR` or `~/.claude`) exists **or** the sibling `.claude.json` MCP config exists |
| **OpenCode** | `<workspace>/.opencode` **or** the resolved project `opencode.json`/`.jsonc` exists (the same file grim manages for both rules and MCP entries) | native skills root (`$OPENCODE_CONFIG_DIR` or `$XDG_CONFIG_HOME/opencode/skills`) exists **or** the resolved global `opencode.json` (`$OPENCODE_CONFIG` / XDG default) exists |
| **Copilot** | a Copilot-specific marker — **not** bare `.github` (nearly every repo carries it for CI): `<workspace>/.github/copilot-instructions.md` or `<workspace>/.github/instructions/` — **or** `<workspace>/.vscode/mcp.json` or `<workspace>/.github/mcp.json` exists (the CLI's own file, as Copilot-specific as the VS Code one) | native skills root (`$COPILOT_HOME/skills` or `~/.copilot/skills`) exists — the `skills/` subdir, not the bare `~/.copilot` parent — **or** the global `mcp-config.json` exists |
| **Codex** | `<workspace>/.codex` — **not** the shared `.agents/skills` dir (a weak cross-vendor marker, like Copilot's bare `.github` caveat) | native config root (`$CODEX_HOME` or `~/.codex`) exists |
| **Cursor** | `<workspace>/.cursor` exists | `~/.cursor` exists |
| **Kiro** | `<workspace>/.kiro` exists | native root (`$KIRO_HOME` or `~/.kiro`) exists |
| **Junie** | `<workspace>/.junie` exists | native root (`$JUNIE_HOME` or `~/.junie`) exists |
| **Gemini** | `<workspace>/.gemini` exists — **not** the shared `.agents/skills` dir (weak cross-vendor marker, like Codex) | native root (`$GEMINI_CLI_HOME/.gemini` or `~/.gemini`) exists |
| **Zed** | `<workspace>/.zed` exists | the platform-resolved Zed config root exists (`$XDG_CONFIG_HOME`\|`~/.config`/`zed` on Linux/FreeBSD, `~/.config/zed` on macOS, `%APPDATA%\Zed` on Windows) |
| **Amp** | `<workspace>/.amp` exists | `$XDG_CONFIG_HOME`\|`~/.config`/`amp` exists |
| **agents** | **never** — `false` at both scopes, by design | **never** |
| **Antigravity** | **never** — `false` by design; all its project surfaces live under the shared `.agents/`, so keying on it would install Antigravity into every workspace that ever used a pool client. Upstream documents no product-specific project marker | `~/.gemini/config` exists |
| **Cline** | `<workspace>/.clinerules` **or** `<workspace>/.cline` exists | `~/.cline` exists |
| **Droid** | `<workspace>/.factory` exists | `~/.factory` exists |
| **Goose** | `<workspace>/.goose` exists — **never** `.agents/`, which is where Goose writes | any candidate config root exists: `$XDG_CONFIG_HOME/goose` or `~/Library/Application Support/goose`, OR-ed — **but `$GOOSE_PATH_ROOT`, when set, *replaces* that list rather than extending it.** `~/.goose` is not a global marker |
| **Warp** | `<workspace>/.warp` exists | `~/.warp` exists — deliberately the same path on all three platforms; the OS-specific app-data dirs are **not** consulted |
| **OpenClaw** | **never** — it has no project scope, and `kind_surface(Skill, Project)` refuses skills there too | native root (`$OPENCLAW_STATE_DIR`, `$OPENCLAW_HOME/.openclaw`, or `~/.openclaw`) exists |
| **Kilo** | `<workspace>/.kilo` **or** `<workspace>/.kilocode` exists — `.kilocode` counts for detection only and is **never written** | `~/.kilo` or `$XDG_CONFIG_HOME/kilo` exists (OR-ed) |
| **Qoder** | `<workspace>/.qoder` exists (its `settings.json` MCP file sits inside it, so no extra clause) | native root (`$QODER_CONFIG_DIR` or `~/.qoder`) exists |

**Never key `detect()` on `.agents/`.** It is a shared multi-client marker,
and for Goose — which *renders into* the pool — keying on it would make the
client detect itself after its own first install. Asserted per vendor.
Where several candidate markers are OR-ed (Cline, Kilo, Goose), that is
deliberate: detection writes nothing, so OR-ing risks only a missed
autodetect, never a wrong path. A *write* path would have to resolve to
exactly one location first.

**Known cross-client leak, disclosed not fixed.** Antigravity's global root
`~/.gemini/config` nests *inside* `~/.gemini`, which is Gemini CLI's global
marker — so a global Antigravity install creates a directory that makes
**gemini** detected on the next autodetected global command, and it
survives uninstall (grim removes files, not empty directories). The reverse
is clean: a Gemini install creates `~/.gemini/agents`, never `config/`.
Fixing it means narrowing `vendor_gemini`'s marker to a Gemini-CLI-exclusive
file — a shipped client's detection change under the freeze. Documented in
`docs/src/content/docs/clients.md` `{#gap-antigravity}` and watchlisted.

A client whose only footprint on a machine/workspace is a grim-installed
MCP entry still counts as detected — its config file lives outside the
vendor's skills/root marker for every client above except Codex (whose
`config.toml` sits inside the directory `detect()` already checks, so no
extra clause is needed there).

Detection lives on the [`Vendor`] trait (`Vendor::detect(workspace,
scope)`), driven by `install::target::detect_clients`, which iterates
`ClientTarget::ALL` so the set is deterministic and returns the **raw**
detected set (possibly empty).

Two callers, two answers, and the split is load-bearing:

- **`InstallTarget::parse`** — the seam every mutating command uses.
  Nothing detected ⇒ the single generic `agents` client (`.agents/skills`,
  skills only). *Not* all clients: that old fallback wrote one directory
  per known vendor, and those directories were exactly what made the next
  run "detect" every client — it was not idempotent with respect to its own
  input. `AgentsVendor::detect` returns `false` at both scopes **by
  design**; the generic client is selected, never detected, so writing the
  pool changes no future resolution. Do not "fix" that to return `true`.
  A recorded `agents` output is likewise treated as unconditionally active
  when reconciling state, since it can only ever have been selected.

  When the fallback is active, `install_and_persist` logs one `warn` naming
  the pool and both selection knobs (issue #113) — the fallback is the
  right default for pool readers but silent misplacement for Claude Code.
  **The surviving exit-78.** When that fallback is active **and** the
  artifact set holds nothing the generic client can install (only rules,
  agents, and/or MCP — all declined by it), the command exits **78**. The
  guard is `installer::refuse_uninstallable_fallback`, called as the first
  statement of `install_and_persist`, so it fires before any blob is
  fetched. It quantifies over `effective_supporting_clients` (target ∪
  recorded clients whose output still resolves), not over
  `target.clients()` alone — otherwise `install` would refuse a state that
  `update` re-materializes cleanly. The message names **both** `--client`
  and `[options].clients`, because `grim add` reaches the same seam and has
  no `--client` flag. `grim update` and `refresh_dev_installs` bypass it
  deliberately: they re-materialize recorded state, and guarding there
  would make an existing exit-0 path start erroring.
- **`detect_clients_or_all`** — the permissive wrapper for read-only
  consumers (`status`, `search`, the TUI badge sites), which reconcile
  *recorded* outputs against "which clients might be present". Nothing
  detected ⇒ all clients, unchanged from before. Also what
  `InstallTarget::new`'s empty-list branch still does — `new` is test-only
  (`parse` guarantees a non-empty list before delegating), but it is still
  `pub`, so a production caller passing `vec![]` would silently re-create
  the original bug.

**Zero detected is neither "all clients" nor an error.** Both readings were
true at different points and both are now wrong: the fallback is one
synthetic skills-only client writing the shared pool, and 78 survives only
in the narrow uninstallable case above.

An explicit `[options].clients` and the `--client` flag both override
detection. The detected set is **not** persisted to config — it is
recomputed each run, with one exception: `grim init` seeds
`[options].clients` from detection (writing no `[options]` table when
detection is empty, so the generic fallback is never persisted).
Detection reuses the same vendor env overrides documented in the table
above.

## Constraints

- Never assume a path operation crosses filesystems silently — check first.
- Treat the content store as append-only / immutable; mutate only the
  name → content mapping.
- Concurrent processes must coordinate via advisory file locks for any
  read-modify-write on shared metadata.

## Cross-References

- `arch-principles.md` — overall architecture and utility discipline
- `quality-security.md` — path traversal / symlink-escape guards (two-layer containment pattern)

<!-- external -->
[uv]: https://docs.astral.sh/uv/
[pixi]: https://pixi.sh/
