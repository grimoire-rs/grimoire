# Client Compatibility

grim installs one canonical artifact into many AI clients, and not every
client can host every artifact kind. A skill is universal, but a rule needs a
per-file scoping surface that some clients lack, and an agent needs a shipped
file format that fewer still provide.

Writing a rule into a client that silently drops its path scoping — or an agent
into one that never reads it — is worse than an honest refusal: the config
looks installed but does nothing. grim renders only what each client can
faithfully host, degrades with a warning where a surface exists but loses
fidelity, and declines (warn, skip, zero files) where no ownable surface exists
at all.

This page is the enforced source of truth. A table-parity test in
`src/install/client_target.rs` reads this matrix at build time and fails the
build if any cell drifts from the `Vendor` implementations, so the
documentation cannot silently lie about what is supported.

Legend:

- `✓` — supported: a native surface, or a faithful transform.
- `◐` — supported with a documented limitation (see [Known gaps](#known-gaps)).
- `✗` — declined: no ownable surface, so grim warns, skips, and writes nothing
  (see [Known gaps](#known-gaps)).

## Support matrix {#matrix}

| Client | Skill | Rule | Agent | MCP |
|--------|-------|------|-------|-----|
| Claude | ✓ | ✓ | ✓ | ✓ |
| OpenCode | ✓ | ◐ | ✓ | ◐ |
| Copilot | ✓ | ✓ | ✓ | ◐ |
| Codex | ✓ | ✗ | ✓ | ◐ |
| Cursor | ✓ | ✓ | ✓ | ◐ |
| Kiro | ✓ | ✓ | ✗ | ◐ |
| Junie | ✓ | ✗ | ✗ | ◐ |
| Gemini | ✓ | ✗ | ✓ | ◐ |
| Zed | ✓ | ✗ | ✗ | ◐ |
| Amp | ✓ | ✗ | ✗ | ◐ |

Bundles decompose into their member kinds and are not a column.

## Known gaps {#known-gaps}

Every ◐ and ✗ above traces to a specific, verified upstream limitation. The
internal working list is the vendor capability watchlist; the entries below are
its user-facing projection — the rationale and the upstream tracking pointer
for each.

### MCP: ws and oauth are Claude-only {#gap-mcp-ws-oauth}

Every MCP cell except Claude is ◐ because grim declines two descriptor shapes
for every non-Claude client: the WebSocket (`ws`) transport and the structured
`oauth` block. No surveyed non-Claude client documents a native config surface
for either, so grim skips a ws- or oauth-bearing server for that client with a
warning rather than writing an entry the client cannot honor. Every other
transport (stdio, sse, http) registers normally.

### Copilot: global MCP environment references {#gap-copilot-env}

At global scope, the GitHub Copilot CLI does not substitute `${VAR}`
environment references in its MCP config, so grim skips a descriptor that
carries one (project scope is unaffected). Upstream shipped substitution in
v0.0.406 and regressed it in v0.0.407 — grim will drop the skip once a fixed
release is confirmed.

### OpenCode: rules install without path scoping {#gap-opencode-rules}

OpenCode has a per-file rules surface but no `paths:` scoping. A rule installs
as body-plus-provenance with its `paths` dropped and a warning — Degraded, not
declined, because the instruction content still installs and loads.

### Codex: rules declined {#gap-codex-rules}

Codex has no path-scoped instruction mechanism — its `AGENTS.md` is always-on
and directory-granular, with no `paths`/`applyTo` equivalent. grim declines a
rule for Codex: warn, skip, and write no file.

### Kiro: global rules are inert until #9176 {#gap-kiro-rules}

Kiro steering rules are native at both scopes, but a global-scope scoped rule
is written correctly yet ignored by Kiro until upstream bug [kiro #9176] is
fixed. grim writes the correct `fileMatch` steering and emits a warning citing
the issue; the file self-heals (becomes active) when the bug closes, with no
grim change.

### Kiro: agents declined {#gap-kiro-agents}

A native Kiro IDE agent format exists, but the Kiro CLI expects an incompatible
JSON schema in the same `.kiro/agents/` directory (open bug [kiro #8040]).
Writing IDE-format files could break CLI users, so grim declines Kiro agents
pending a resolution.

### Junie: rules and agents declined {#gap-junie}

Junie has no grim-ownable per-file rules surface — its mechanism is a single
`.junie/AGENTS.md`, not a per-rule directory — so rules are declined. Junie's
`.junie/agents/` format exists but is early-access-preview only, not generally
available; agents are declined until it ships.

### Gemini: rules declined, agents gated by a setting {#gap-gemini}

Gemini's only rules surface is the `GEMINI.md` hierarchy, with no ownable
per-file target, so rules are declined. Gemini agents are native and are
installed, but Gemini only loads them when `experimental.enableAgents` is set —
which defaults on, so they work out of the box for most users.

### Shared skills pool visibility {#gap-shared-pool}

Codex, Gemini, Zed, and Amp all read the cross-vendor `.agents/skills`
directory. A skill installed for any one of them is physically the same file
every other pool member reads, so it is discoverable by all four even when only
one was selected. This is upstream scan behavior, not a grim choice; grim
refcounts the shared directory so removing one client never deletes a skill
another client still records.

### Zed: rules and agents declined, MCP env references {#gap-zed}

Zed has no rule scoping — instruction files follow a nine-name first-match
precedence with no per-file ownership — so rules are declined. Zed agents run
over ACP with no installable file format and are declined too. Zed's MCP config
has no environment-reference substitution, so grim skips a `${VAR}`-bearing
server with a warning.

### Amp: rules and agents declined {#gap-amp}

Amp's only instruction surface is `AGENTS.md` (falling back to `AGENT.md`, then
`CLAUDE.md`) with no per-file scoping, so rules are declined. Amp subagents are
spawned at runtime with no installable file format, so agents are declined.

## The `compatibility:` frontmatter field {#compatibility-disclaimer}

An artifact may carry a free-text `compatibility:` frontmatter field. It is an
editor and runtime *hint* only — a note for humans and tools that read the
source. It has **zero effect** on how grim renders or gates an artifact per
client. A `compatibility: codex` line does not make a rule install for Codex,
and it never overrides the matrix above. This matrix — enforced by the
build-time parity test — is the authoritative statement of what grim installs
where.

<!-- external -->
[kiro #9176]: https://github.com/kirodotdev/Kiro/issues/9176
[kiro #8040]: https://github.com/kirodotdev/Kiro/issues/8040
