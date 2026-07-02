# ADR: Per-call MCP scope + `grim_fetch` / `grim_render` tools

**Status:** Accepted
**Date:** 2026-07-03
**Decision drivers:** `grim mcp` v2 — the AI consuming the MCP server, not
the process launcher, should decide per question which scope a tool call
acts on; and "agent finds a skill → installs it → uses it now" breaks at
the last step because harnesses scan skills at session start.

## Context

The `grim mcp` STDIO server (shipped 0.7.0, `adr_multi_registry_mcp.md`)
pins the install scope at launch: `--global` / `--config <path>` are
copied into `McpState` and every tool call operates within that one scope
for the server's lifetime. The `state.rs` module doc records this as a
deliberate invariant ("an agent cannot redirect a project-scoped session
into global writes").

Two problems surfaced:

1. **Launch-pinned scope is the wrong granularity.** An MCP server is
   wired once (per IDE, per machine) but the agent's questions span
   scopes: "what's installed globally?", "what does *this* project
   declare?", "what about the repo in `~/other`?" — one launch scope
   cannot answer all three. `catalog/mcp/grim.toml` carries a TODO noting
   that baking a scope flag into the descriptor pins every consumer.

2. **Install is the wrong operation for the AI consumer.** `grim install`
   mid-session does not make a skill usable: harnesses scan skills at
   session start, so a freshly installed skill is invisible until the
   next session. Key insight: a skill is markdown — return the content in
   the tool result and the agent has it in-context immediately. **Use ≠
   install.** Install is for persistence across sessions; use is
   satisfied by fetch-into-context.

At the command layer, scope is already per-call: `scope_resolution::
resolve(ctx, global, config)` runs inside each `command::*::run`, and
`Context` is scope-free. The launch-pinning lives only in `McpState` —
the MCP layer added the restriction; removing it is an MCP-layer change.

## Decision

### (a) Scope becomes a per-tool-call parameter; the fixed-scope invariant is deliberately reversed

`grim mcp` drops `--global` / `--config` (breaking, `feat(mcp)!:` — the
old flags exit 64 at parse). Every scope-sensitive tool takes an optional
flattened trio:

- `global: bool` — global scope (`$GRIM_HOME`); wins over everything.
- `config: path` — explicit project config file; wins over `workspace`.
- `workspace: path` — seed directory for the project-config walk-up.
- default (all omitted) — walk-up from the server's cwd, exactly the CLI
  default.

Precedence: `global` > `config` > `workspace`-seeded walk-up > cwd
walk-up. `resolve_in` re-reads scope per call — the server stays
stateless across calls; concurrent calls with different scopes never
share resolution state.

**Why reversing the invariant is safe:** the invariant never protected
anything concrete — no write tools existed when it was written; the
read-only surface can already be pointed anywhere by whoever launches the
process. The stance is **local trust**: scope/dest parameters supplied by
the model are equivalent to CLI flags of a user-launched local process
(the model can already run `grim --config X` through any shell tool in
the same harness). The *real* trust boundaries are:

- **Registry content** (the only genuinely untrusted input):
  `safe_relative_path` guards every tar unpack — kept verbatim in both
  the on-disk and the new in-memory unpack path, forged-header tests on
  both.
- **`--allow-writes`** stays launch-pinned: enabling mutation is a trust
  decision of whoever wires the server into a harness, not of the model.

### (b) SSRF stance retained: no registry parameter on any tool

No tool accepts a registry/host override (CWE-918). Tools resolve
references only against the registries the *resolved scope's config*
declares (`[[registries]]` + documented fallback chain). Scope params
select **which config file** is read — they do not let the model name an
arbitrary host directly. The existing `tool_args.rs` comment stays
verbatim. When scope resolution fails, fetch degrades to the
flag/env/fallback registry chain exactly like `grim search`'s browse-only
path.

### (c) `grim_fetch` — read tool, size caps with a truncate-vs-error split

`grim_fetch {ref, vendor?, path?, global?, config?, workspace?}` resolves
+ fetches + returns artifact **content** in the tool result — no install,
no state mutation, no harness reload. Default is the canonical
(as-authored) form; `vendor` (claude|opencode|copilot) returns that
client's projection via the existing pure in-memory transforms
(`render_skill_doc`, `rule_index`/`agent_index`, `Vendor::mcp_entry`);
`path` returns one support file; a `files` listing (path + size) is
always included.

Two ceilings, different failure modes:

| Cap | Value | Behavior | Rationale |
|---|---|---|---|
| Layer blob (`FETCH_BLOB_SIZE_LIMIT`) | 8 MiB | **Error**, checked against the manifest's layer-descriptor `size` *before* download | skill/rule/agent layers have no publish-side cap and `fetch_blob` streams unbounded; refusing pre-download bounds memory and network |
| Returned document (`FETCH_DOC_SIZE_LIMIT`) | 256 KiB per file | **Truncate** + `truncated: true` + marker line naming `grim_render` / `grim install` as the escape hatch | a truncated skill doc is still useful in-context; an error would make large-but-valid artifacts unusable |

Existing kind-level caps stand (mcp descriptor 64 KiB, bundle 512 KiB).
The blob is fetched via a ~30-line pipeline over `OciAccess` directly
(`resolve_digest → fetch_manifest → kind_from_manifest → single_layer →
size gate → fetch_blob → digest verify`) in `src/mcp/fetch.rs` —
`installer::fetch_verified_layer` is *not* lifted, because it discards
the manifest while fetch needs it twice (kind inference + pre-download
size gate). Installer untouched.

### (d) Write-tool gating = rmcp `disable_route` (hide + reject)

`grim_render {ref, vendor, dest_dir, …scope}` writes an artifact's
vendor-native files to an arbitrary `dest_dir` — the first real tool
behind `--allow-writes`. Gating uses rmcp 1.7.0 `ToolRouter::
disable_route`: the route is both hidden from `tools/list` **and**
rejected at `tools/call` (`invalid_params`) — one mechanism, no
drift between advertising and enforcement. The router is built once in
`serve()`; no runtime toggling, so no `list_changed` notifier needed.
Render reuses `fetch_artifact` (no blob cap — output goes to disk,
install parity) → staging tempdir (`DefaultMaterializer`) →
`ClientTarget::materialize` with `dest_dir`-derived dest. No install
state, no lock, no flock — render is not install.

### (e) Use ≠ install

`grim_fetch` (in-context use, zero side effects) and `grim_render`
(materialize files where the agent wants them) deliberately do **not**
touch declarations, lock, or install state. `grim install` remains the
only path that persists. This resolves the `catalog/mcp/grim.toml` TODO:
instead of baking a scope into the descriptor, the server emits rendered
artifacts for a requested vendor/scope on demand.

## Alternatives Considered

- **Keep launch-pinned scope, add a second server instance per scope.**
  Rejected: harness configs multiply, and "another directory" is
  unbounded — can't pre-wire a server per possible workspace.
- **`grim install` from the MCP + harness reload.** Rejected: no harness
  reload primitive exists mid-session; install has side effects the agent
  consumer doesn't want (state, lock, client config edits).
- **Zip/binary payloads for multi-file artifacts.** Rejected: a JSON file
  map (`files` listing + per-file fetch via `path`) is strictly better
  for an LLM consumer; agents can't unzip tool results.
- **Registry param on fetch (fetch from anywhere).** Rejected: SSRF
  (CWE-918); configured registries stay the boundary — see (b).
- **Error instead of truncation at the doc cap.** Rejected: large-but-
  valid skills would become unfetchable; truncation with an explicit
  marker keeps them useful and names the escape hatch.

## Consequences

- `grim mcp --global` / `--config` exit 64 at parse (breaking; migration:
  move scope into the tool calls).
- `grim_status` gains an all-optional args schema (was param-less);
  `grim_search` gains the scope trio. Empty-object calls stay valid
  (`#[serde(default)]` everywhere, no `deny_unknown_fields`).
- `scope_resolution::resolve_in` + seedable `walk_up_for_config(start)`
  land as delegating extensions — zero CLI caller churn.
- The in-memory tar walk (`unpack_tar_in_memory`) becomes a sibling of
  the on-disk unpack, sharing the file-private `safe_relative_path`.
- **Offline limitation (documented):** `CachedAccess` caches blobs but
  not manifests, so `GRIM_OFFLINE` fetch/render fail cleanly at
  `fetch_manifest` even with warm blobs. A manifest cache is deferred.
- `test_mcp.py`'s `--allow-writes` tool-surface change-detector now
  guards a real difference: read-only set {grim_search, grim_status,
  grim_fetch}; `--allow-writes` adds grim_render.

## Deferred

- **MCP roots** — rmcp has server-side `list_roots`; client support
  varies too much to build scope defaults on it today.
- **Hosted/remote MCP facade** on the index server.
- **MCP resources** (resource-style content addressing) — tools first.
- **Zip payloads** — JSON file map chosen; revisit only if a consumer
  demands binary fidelity.
- **Manifest cache** for true offline fetch.
- From the mcp-kind TODO: `${VAR:-default}` substitution, per-vendor
  override keys in descriptors, VS Code user-profile `mcp.json` surface,
  mcp bundle membership.

## References

- `adr_multi_registry_mcp.md` — v1 server, `load_catalog` seam, SSRF
  stance origin, `--allow-writes` gate.
- `src/command/scope_resolution.rs`, `src/config/project_config.rs` —
  per-call scope seams.
- `src/install/materializer.rs` (`safe_relative_path`),
  `src/install/client_target.rs` (`materialize`), `src/install/render.rs`
  (pure projections), `src/oci/annotations.rs` (`kind_from_manifest`).
- `catalog/mcp/grim.toml` TODO + `TODO.md` mcp follow-ups entry.
