---
paths:
  - src/**
---

# Grimoire CLI Commands — Quick Reference

Concise index of `grim` CLI commands. Implementation lives under
`src/command/` — read the source for return types, call sites, and report
column formats.

Index of shipped `grim` subcommands — keep in sync with `src/command/`
(one file per subcommand) and `docs/src/commands.md`.

## Command Surface

| Command | Purpose |
|---------|---------|
| `grim init` | Write a fresh `grimoire.toml` in the current directory; `--registry <ref>` seeds the default `[[registries]]` entry |
| `grim add [--kind …] [--name …] [--no-install] [--force] <ref>` | Declare a skill/rule/agent/mcp/bundle in `grimoire.toml`, pin it in the lock immediately, and (by default) materialize just that entry into the detected clients; `--no-install` stops at declare + lock; `--force` overwrites a locally modified artifact / untracked destination with `grim install --force` semantics (inert with `--no-install`) |
| `grim lock` | Resolve floating tags in `grimoire.toml` to digests and write `grimoire.lock` (after hand-edits; `add` locks what it declares) |
| `grim config get\|set\|unset\|list <key>` | Read and write `grimoire.toml` settings (`[options]`, `[options.tui]`) and registry fields via dotted keys; `list` dumps explicitly-set values for the active scope (never merged across scopes); `list [--all]` lists every supported key incl. unset, with JSON items carrying type/title/description/default metadata. Exit codes: unset `get` → 1, unknown key → 64, bad value → 65. |
| `grim config registry add\|rm\|use\|show\|list` | Registry lifecycle for `[[registries]]` entries: `add <alias> --url <url> [--default]`, `rm <alias>`, `use <alias>` (at-most-one-default, clears prior), `show <alias>`, `list`. Alias not found or dup on `add` → 64. |
| `grim install` | Materialize every locked artifact into the configured AI client(s); no positional — declare via `add`, scope clients via `--client`. Refuses to overwrite an untracked pre-existing destination (65) unless `--force`; adopts identical content into the record |
| `grim status` | Report each declared artifact's state (installed, outdated, modified, …) with bundle provenance |
| `grim context` | Read-only introspection of the resolved invocation context: scope, config/lock/state paths (+ existence), effective client set (names only), registry browse set, default registry, offline (+ source). Exits 79 outside a project without `--global` |
| `grim search [query]` | Substring search over the registry catalog (repo, summary, description, keywords); empty query lists all |
| `grim fetch <ref> [--vendor …] [--path …]` | Print an artifact's content without installing (CLI port of the MCP `grim_fetch` tool). Plain = raw payload (pipe-able, no trailing newline; warnings on stderr); JSON = full fetch report. Never truncates a printed payload (MCP keeps its 256 KiB doc cap). Two download ceilings instead: the declared layer size is checked against the 8 MiB limit before download, and that same declared size then bounds the actual streamed bytes — a registry serving more than it declared aborts mid-transfer into a data error (65) rather than a silent truncation |
| `grim tui` | Interactive catalog browser with live install state (flat list / tree toggle) |
| `grim update [names…]` | Re-resolve floating tags, roll the lock forward, re-materialize what changed; prunes clean orphans that dropped out of the lock. No names = everything; names are config binding names, not refs |
| `grim remove <kind> <name>` | Undeclare an artifact (config + lock only; files left on disk) |
| `grim uninstall <kind> <name>` | Full inverse of install: delete files, drop the install record, undeclare (config + lock). Shared seam reused by the TUI delete action. **Exception:** an artifact a declared bundle still provides keeps its files (a directly-declared one degrades to `remove`; a bundle-only member is a no-op — remove the bundle to remove it) |
| `grim build <path>` | Validate and pack a local skill/rule/agent/mcp/bundle without pushing (release dry run) |
| `grim release <path> <ref>` | Push a single artifact to a registry (validate, pack, push with cascade tags); `--push-registry <host[/prefix]>` pushes to a deviating endpoint while every baked/reported name keeps the reference's registry (the pull name; malformed value → 65), report gains always-present `pushed_to` (null when inactive) |
| `grim publish` | Batch-release all packages declared in a `publish.toml` manifest; validates whole manifest before any push; fixed kind order (skills → rules → agents → mcp → bundles), skip-existing by default. Optional manifest `push_registry` / `--push-registry` flag (flag wins) splits the push endpoint from the pull-named manifest `registry` (references, source fallback, pinned member ids, announce pointers, report `ref` all stay pull-named; per-entry `pushed_to` reports the push side) |
| `grim login [<registry>]` | Authenticate to a registry; store the credential via the docker-compatible credential store (helper or, with `--allow-insecure-store`, plaintext). Verifies the credential against the registry **before** storing by default (`/v2/` ping + challenge answer, scope-less): rejected → 80 nothing stored, unreachable/5xx/429 → 69 nothing stored, anonymous registry → 0 stored (`verification: no-auth-required`). `--no-verify` skips (store-only, `verification: skipped`); offline skips silently with a warning unless `--verify` is explicit → 81 |
| `grim logout [<registry>]` | Remove a stored registry credential (idempotent — exits 0 when nothing is stored) |
| `grim schema --kind <config\|publish\|lock>` | Print the JSON Schema for `grimoire.toml`, `publish.toml`, or `grimoire.lock` to stdout (generated from the real parse structs); emits a document, not a `Printable` report |
| `grim mcp [--allow-writes]` | Run a local STDIO Model Context Protocol server (rmcp SDK). Long-running, `Printable`-exempt (returns `ExitCode` directly, like `tui`/`schema`); stdout is the JSON-RPC channel. Read tools (`grim_search`, `grim_status`, `grim_fetch`) always on; the write tool `grim_render` gated behind `--allow-writes` (rmcp `disable_route`: hidden + rejected). Scope per tool call (`global`/`config`/`workspace` args; precedence in that order, default cwd walk-up) — launch scope flags removed, `--global`/`--config` exit 64 |
| `grim --version` | Print the compiled version (clap built-in; no subcommand) |

Global flags (`src/cli/options.rs` `GlobalOptions`): `--format`,
`--progress <auto|json|none>` (experimental; NDJSON events on stderr),
`--offline`, `--log-level`, `--config <path>`, `--global`,
`--registry <ref>` (repeatable / comma-separated).

`login`/`logout` resolve the registry from the positional argument, else
`--registry` / `GRIM_DEFAULT_REGISTRY` — the config `default_registry`
option is not consulted on this path (`Context::default_registry()`).
They read and write the docker config at `$DOCKER_CONFIG/config.json`
(default `~/.docker/config.json`) — the same file the credential read path
consults — so credentials round-trip with `docker login`.

## Conventions (apply as commands land)

- **One file per subcommand** under `src/command/`.
- **Typed identifiers**: parse user-supplied references into domain types
  early; the rest of the command works on typed values, not raw strings.
- **Report actual results**: a command reports what happened, not an echo
  of its input. Operations return enough data to build accurate output.
- **Exit codes**: follow `quality-rust-exit_codes.md` — usage errors,
  data errors, and I/O errors map to distinct, documented codes.
- **Output**: structured data goes through the shared output trait so
  `--format json` and the plain table render from one source.

## Cross-References

- `subsystem-cli.md` — CLI shell structure and clap usage
- `subsystem-cli-api.md` — output / report data layer patterns
- `quality-rust-exit_codes.md` — exit code design
