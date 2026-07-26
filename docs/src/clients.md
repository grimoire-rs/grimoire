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

<!--
  Client marks reused from docs/theme/index.hbs: inlined from
  @lobehub/icons-static-svg (MIT, Copyright (c) 2025 LobeHub) for every
  client except Zed, whose mark comes from Simple Icons (CC0-1.0).
-->

<div class="matrix-table">

| Client | Skill | Rule | Agent | MCP |
|--------|-------|------|-------|-----|
| [Claude] <svg viewBox="0 0 24 24" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M20.998 10.949H24v3.102h-3v3.028h-1.487V20H18v-2.921h-1.487V20H15v-2.921H9V20H7.488v-2.921H6V20H4.487v-2.921H3V14.05H0V10.95h3V5h17.998v5.949zM6 10.949h1.488V8.102H6v2.847zm10.51 0H18V8.102h-1.49v2.847z"></path></svg> | ✓ | ✓ | ✓ | ✓ |
| [OpenCode] <svg viewBox="0 0 24 24" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M16 6H8v12h8V6zm4 16H4V2h16v20z"></path></svg> | ✓ | ◐ | ✓ | ◐ |
| [Copilot] <svg viewBox="0 0 24 24" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M19.245 5.364c1.322 1.36 1.877 3.216 2.11 5.817.622 0 1.2.135 1.592.654l.73.964c.21.278.323.61.323.955v2.62c0 .339-.173.669-.453.868C20.239 19.602 16.157 21.5 12 21.5c-4.6 0-9.205-2.583-11.547-4.258-.28-.2-.452-.53-.453-.868v-2.62c0-.345.113-.679.321-.956l.73-.963c.392-.517.974-.654 1.593-.654l.029-.297c.25-2.446.81-4.213 2.082-5.52 2.461-2.54 5.71-2.851 7.146-2.864h.198c1.436.013 4.685.323 7.146 2.864zm-7.244 4.328c-.284 0-.613.016-.962.05-.123.447-.305.85-.57 1.108-1.05 1.023-2.316 1.18-2.994 1.18-.638 0-1.306-.13-1.851-.464-.516.165-1.012.403-1.044.996a65.882 65.882 0 00-.063 2.884l-.002.48c-.002.563-.005 1.126-.013 1.69.002.326.204.63.51.765 2.482 1.102 4.83 1.657 6.99 1.657 2.156 0 4.504-.555 6.985-1.657a.854.854 0 00.51-.766c.03-1.682.006-3.372-.076-5.053-.031-.596-.528-.83-1.046-.996-.546.333-1.212.464-1.85.464-.677 0-1.942-.157-2.993-1.18-.266-.258-.447-.661-.57-1.108-.32-.032-.64-.049-.96-.05zm-2.525 4.013c.539 0 .976.426.976.95v1.753c0 .525-.437.95-.976.95a.964.964 0 01-.976-.95v-1.752c0-.525.437-.951.976-.951zm5 0c.539 0 .976.426.976.95v1.753c0 .525-.437.95-.976.95a.964.964 0 01-.976-.95v-1.752c0-.525.437-.951.976-.951zM7.635 5.087c-1.05.102-1.935.438-2.385.906-.975 1.037-.765 3.668-.21 4.224.405.394 1.17.657 1.995.657h.09c.649-.013 1.785-.176 2.73-1.11.435-.41.705-1.433.675-2.47-.03-.834-.27-1.52-.63-1.813-.39-.336-1.275-.482-2.265-.394zm6.465.394c-.36.292-.6.98-.63 1.813-.03 1.037.24 2.06.675 2.47.968.957 2.136 1.104 2.776 1.11h.044c.825 0 1.59-.263 1.995-.657.555-.556.765-3.187-.21-4.224-.45-.468-1.335-.804-2.385-.906-.99-.088-1.875.058-2.265.394zM12 7.615c-.24 0-.525.015-.84.044.03.16.045.336.06.526l-.001.159a2.94 2.94 0 01-.014.25c.225-.022.425-.027.612-.028h.366c.187 0 .387.006.612.028-.015-.146-.015-.277-.015-.409.015-.19.03-.365.06-.526a9.29 9.29 0 00-.84-.044z"></path></svg> | ✓ | ✓ | ✓ | ◐ |
| [Codex] <svg viewBox="0 0 24 24" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M8.086.457a6.105 6.105 0 013.046-.415c1.333.153 2.521.72 3.564 1.7a.117.117 0 00.107.029c1.408-.346 2.762-.224 4.061.366l.063.03.154.076c1.357.703 2.33 1.77 2.918 3.198.278.679.418 1.388.421 2.126a5.655 5.655 0 01-.18 1.631.167.167 0 00.04.155 5.982 5.982 0 011.578 2.891c.385 1.901-.01 3.615-1.183 5.14l-.182.22a6.063 6.063 0 01-2.934 1.851.162.162 0 00-.108.102c-.255.736-.511 1.364-.987 1.992-1.199 1.582-2.962 2.462-4.948 2.451-1.583-.008-2.986-.587-4.21-1.736a.145.145 0 00-.14-.032c-.518.167-1.04.191-1.604.185a5.924 5.924 0 01-2.595-.622 6.058 6.058 0 01-2.146-1.781c-.203-.269-.404-.522-.551-.821a7.74 7.74 0 01-.495-1.283 6.11 6.11 0 01-.017-3.064.166.166 0 00.008-.074.115.115 0 00-.037-.064 5.958 5.958 0 01-1.38-2.202 5.196 5.196 0 01-.333-1.589 6.915 6.915 0 01.188-2.132c.45-1.484 1.309-2.648 2.577-3.493.282-.188.55-.334.802-.438.286-.12.573-.22.861-.304a.129.129 0 00.087-.087A6.016 6.016 0 015.635 2.31C6.315 1.464 7.132.846 8.086.457zm-.804 7.85a.848.848 0 00-1.473.842l1.694 2.965-1.688 2.848a.849.849 0 001.46.864l1.94-3.272a.849.849 0 00.007-.854l-1.94-3.393zm5.446 6.24a.849.849 0 000 1.695h4.848a.849.849 0 000-1.696h-4.848z"></path></svg> | ✓ | ✗ | ✓ | ◐ |
| [Cursor] <svg viewBox="0 0 24 24" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M22.106 5.68L12.5.135a.998.998 0 00-.998 0L1.893 5.68a.84.84 0 00-.419.726v11.186c0 .3.16.577.42.727l9.607 5.547a.999.999 0 00.998 0l9.608-5.547a.84.84 0 00.42-.727V6.407a.84.84 0 00-.42-.726zm-.603 1.176L12.228 22.92c-.063.108-.228.064-.228-.061V12.34a.59.59 0 00-.295-.51l-9.11-5.26c-.107-.062-.063-.228.062-.228h18.55c.264 0 .428.286.296.514z"></path></svg> | ✓ | ✓ | ✓ | ◐ |
| [Kiro] <svg viewBox="0 0 24 24" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M4.594 6.677C6.67-2.226 18.746-2.211 21.16 6.632c.353 1.297 1.725 7.582-1.673 13.747-1.545 2.797-5.841 5.49-6.99 1.883C8.6 25.477 3.315 24.1 5.789 18.609l-.318.143c-3.57 1.305-3.863-1.208-3.173-2.513.45-.84.727-1.335.937-1.897.353-.975.458-1.568.593-2.498.27-1.837.277-3.607.765-5.167zm8.37.01a.92.92 0 00-.81.428c-.217.323-.33.825-.33 1.462 0 .705.15 1.89 1.14 1.89h.008c.757 0 1.214-.705 1.214-1.89 0-.622-.127-1.125-.367-1.455a1.014 1.014 0 00-.855-.435zm4.08 0a.92.92 0 00-.81.428c-.217.323-.33.825-.33 1.462 0 .705.15 1.89 1.14 1.89h.008c.757 0 1.215-.705 1.215-1.89 0-.622-.128-1.125-.368-1.455a1.014 1.014 0 00-.855-.435z"></path></svg> | ✓ | ✓ | ✗ | ◐ |
| [Junie] <svg viewBox="0 0 24 24" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M24 9.333C24 18.666 20 24 9.333 24H8v-8h1.333C14 16 16 14 16 9.333V8h8v1.333zM8 16H0V8h8v8zM16 8H8V0h8v8z"></path></svg> | ✓ | ✗ | ✗ | ◐ |
| [Gemini] <svg viewBox="0 0 24 24" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M20.616 10.835a14.147 14.147 0 01-4.45-3.001 14.111 14.111 0 01-3.678-6.452.503.503 0 00-.975 0 14.134 14.134 0 01-3.679 6.452 14.155 14.155 0 01-4.45 3.001c-.65.28-1.318.505-2.002.678a.502.502 0 000 .975c.684.172 1.35.397 2.002.677a14.147 14.147 0 014.45 3.001 14.112 14.112 0 013.679 6.453.502.502 0 00.975 0c.172-.685.397-1.351.677-2.003a14.145 14.145 0 013.001-4.45 14.113 14.113 0 016.453-3.678.503.503 0 000-.975 13.245 13.245 0 01-2.003-.678z"></path></svg> | ✓ | ✗ | ✓ | ◐ |
| [Zed] <svg viewBox="0 0 24 24" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M2.25 1.5a.75.75 0 0 0-.75.75v16.5H0V2.25A2.25 2.25 0 0 1 2.25 0h20.095c1.002 0 1.504 1.212.795 1.92L10.764 14.298h3.486V12.75h1.5v1.922a1.125 1.125 0 0 1-1.125 1.125H9.264l-2.578 2.578h11.689V9h1.5v9.375a1.5 1.5 0 0 1-1.5 1.5H5.185L2.562 22.5H21.75a.75.75 0 0 0 .75-.75V5.25H24v16.5A2.25 2.25 0 0 1 21.75 24H1.655C.653 24 .151 22.788.86 22.08L13.19 9.75H9.75v1.5h-1.5V9.375A1.125 1.125 0 0 1 9.375 8.25h5.314l2.625-2.625H5.625V15h-1.5V5.625a1.5 1.5 0 0 1 1.5-1.5h13.19L21.438 1.5z"/></svg> | ✓ | ✗ | ✗ | ◐ |
| [Amp] <svg viewBox="0 0 24 24" fill-rule="evenodd" aria-hidden="true" width="16" height="16" fill="currentColor"><path d="M15.087 23.18L12.03 24l-2.097-7.823-5.738 5.738-2.251-2.251 5.718-5.719-7.769-2.082.82-3.057 11.294 3.08 3.08 11.295z"></path><path d="M19.505 18.762l-3.057.82-2.564-9.573-9.572-2.564.819-3.057 11.295 3.079 3.08 11.295z"></path><path d="M23.893 14.374l-3.057.82-2.565-9.572L8.7 3.057 9.52 0l11.295 3.08 3.079 11.294z"></path></svg> | ✓ | ✗ | ✗ | ◐ |
| agents | ✓ | ✗ | ✗ | ✗ |

</div>

Bundles decompose into their member kinds and are not a column.

`agents` is not a product — it is the vendor-neutral target. Selecting it
installs one copy of each skill into the cross-vendor `.agents/skills` pool
that Codex, Gemini, Zed, and Amp all scan, rather than into any one client's
directory. Rules, agents, and MCP have no vendor-neutral format, so it declines
all three. It is never detected, only selected: request it explicitly with
`--client agents` or in `[options].clients`.

## Known gaps {#known-gaps}

Every ◐ and ✗ above traces to a specific, verified upstream limitation. The
internal working list is the vendor capability watchlist; the entries below are
its user-facing projection — the rationale and the upstream tracking pointer
for each, plus a couple of authoring caveats (like [Cursor]'s comma-in-glob
split) worth calling out even where the surface is otherwise fully supported.

### MCP: ws and oauth are Claude-only {#gap-mcp-ws-oauth}

Every MCP cell except [Claude] is ◐ because grim declines two descriptor shapes
for every client other than [Claude]: the WebSocket (`ws`) transport and the
structured `oauth` block. No surveyed client other than [Claude] documents a
native config surface for either, so grim skips a ws- or oauth-bearing server
for that client with a warning rather than writing an entry the client cannot
honor. Every other transport (stdio, sse, http) registers normally.

### Copilot: global MCP environment references {#gap-copilot-env}

At global scope, the [GitHub Copilot][copilot] CLI does not substitute `${VAR}`
environment references in its MCP config, so grim skips a descriptor that
carries one (project scope is unaffected). Upstream shipped substitution in
v0.0.406 and regressed it in v0.0.407 — grim will drop the skip once a fixed
release is confirmed.

### Cursor: a comma inside a glob splits the pattern {#gap-cursor-globs}

[Cursor] rules are fully supported (a `.mdc` file with a comma-joined `globs`
string), but Cursor splits that string on **every** comma — including a comma
inside a `{a,b}` brace alternation ([cursor forum #76648][cursor-glob-split]).
A single glob such as `src/**/*.{rs,toml}` is therefore read as two separate
patterns. grim writes the glob unchanged and emits a warning at install time so
you can split the rule into one pattern per glob.

### OpenCode: rules install without path scoping {#gap-opencode-rules}

[OpenCode] has a per-file rules surface but no `paths:` scoping. A rule installs
as body-plus-provenance with its `paths` dropped and a warning — Degraded, not
declined, because the instruction content still installs and loads.

### Codex: rules declined {#gap-codex-rules}

[Codex] has no path-scoped instruction mechanism — its `AGENTS.md` is always-on
and directory-granular, with no `paths`/`applyTo` equivalent. grim declines a
rule for [Codex]: warn, skip, and write no file.

### Kiro: global rules are inert until #9176 {#gap-kiro-rules}

[Kiro] steering rules are native at both scopes, but a global-scope scoped rule
is written correctly yet ignored by [Kiro] until upstream bug [kiro #9176] is
fixed. grim writes the correct `fileMatch` steering and emits a warning citing
the issue; the file self-heals (becomes active) when the bug closes, with no
grim change.

A manual workaround exists today: switching the steering block to
`inclusion: auto` makes [Kiro] load it heuristically at the global scope. grim
deliberately does not emit `auto` — it ships the deterministic, path-scoped
`fileMatch` the rule actually describes, which activates exactly where intended
once the upstream fix lands, instead of a fuzzy always-on heuristic.

### Kiro: agents declined {#gap-kiro-agents}

A native [Kiro] IDE agent format exists, but the [Kiro] CLI expects an
incompatible JSON schema in the same `.kiro/agents/` directory (open bug
[kiro #8040]). Writing IDE-format files could break CLI users, so grim declines
[Kiro] agents pending a resolution.

### Junie: rules and agents declined {#gap-junie}

[Junie] has no grim-ownable per-file rules surface — its mechanism is a single
`.junie/AGENTS.md`, not a per-rule directory — so rules are declined. [Junie]'s
`.junie/agents/` format exists but is early-access-preview only, not generally
available; agents are declined until it ships.

### Gemini: rules declined, agents gated by a setting {#gap-gemini}

[Gemini]'s only rules surface is the `GEMINI.md` hierarchy, with no ownable
per-file target, so rules are declined. [Gemini] agents are native and are
installed, but [Gemini] only loads them when `experimental.enableAgents` is set —
which defaults on, so they work out of the box for most users.

The individual-tier [Gemini] CLI (free/Pro/Ultra) stopped being served on
2026-06-18, [transitioning to the Antigravity CLI][gemini-antigravity] (which
reportedly carries Agent Skills and subagents forward — unverified). Enterprise
[Gemini] Code Assist licenses remain fully supported; grim's [Gemini] support
targets that surface, verified against the still-served enterprise docs.

### Shared skills pool visibility {#gap-shared-pool}

[Codex], [Gemini], [Zed], and [Amp] all read the cross-vendor `.agents/skills`
directory. A skill installed for any one of them is physically the same file
every other pool member reads, so it is discoverable by all four even when only
one was selected. This is upstream scan behavior, not a grim choice; grim
refcounts the shared directory so removing one client never deletes a skill
another client still records.

### Zed: rules and agents declined, MCP env references {#gap-zed}

[Zed] has no rule scoping — instruction files follow a nine-name first-match
precedence with no per-file ownership — so rules are declined. [Zed] agents run
over ACP with no installable file format and are declined too. [Zed]'s MCP config
has no environment-reference substitution, so grim skips a `${VAR}`-bearing
server with a warning.

### Amp: rules and agents declined {#gap-amp}

[Amp]'s only instruction surface is `AGENTS.md` (falling back to `AGENT.md`, then
`CLAUDE.md`) with no per-file scoping, so rules are declined. [Amp] subagents are
spawned at runtime with no installable file format, so agents are declined.

## The `compatibility:` frontmatter field {#compatibility-disclaimer}

An artifact may carry a free-text `compatibility:` frontmatter field. It is an
editor and runtime *hint* only — a note for humans and tools that read the
source. It has **zero effect** on how grim renders or gates an artifact per
client. A `compatibility: codex` line does not make a rule install for [Codex],
and it never overrides the matrix above. This matrix — enforced by the
build-time parity test — is the authoritative statement of what grim installs
where.

<!-- external -->
[claude]: https://code.claude.com
[opencode]: https://opencode.ai
[copilot]: https://github.com/features/copilot
[codex]: https://developers.openai.com/codex
[cursor]: https://cursor.com
[kiro]: https://kiro.dev
[junie]: https://www.jetbrains.com/junie/
[gemini]: https://geminicli.com
[zed]: https://zed.dev
[amp]: https://ampcode.com
[cursor-glob-split]: https://forum.cursor.com/t/76648
[kiro #9176]: https://github.com/kirodotdev/Kiro/issues/9176
[kiro #8040]: https://github.com/kirodotdev/Kiro/issues/8040
[gemini-antigravity]: https://developers.googleblog.com/an-important-update-transitioning-gemini-cli-to-antigravity-cli/
