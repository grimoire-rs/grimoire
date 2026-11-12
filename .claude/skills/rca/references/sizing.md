# Fix size rubric

Shared by `/rca` and `/fix`. Size by blast radius, not hours. State the basis beside the letter.

| Size | Criteria |
|---|---|
| **S** | One function or file. No interface surface. An existing test pattern covers it. Zero siblings. |
| **M** | Up to 5 files in one crate, or the cause plus 1–3 sibling sites. New tests, no design choice. |
| **L** | Crosses crates or layers, **or** touches any interface surface below, **or** needs a choice between designs. |
| **XL** | Needs an ADR, the architectural assumption itself is wrong, a migration, or a concurrency/locking redesign. Split before building. |

**Bump +1** when the repro is flaky or Windows-only, or confidence is below medium.

## Interface surfaces (`docs/src/content/docs/stability.md` › Frozen at 1.0)

CLI subcommands, arguments and flags · exit codes · `--format json` reports and the error document · `grimoire.toml` / `grimoire.lock` · `marketplace.toml` / `marketplace.lock` · `publish.toml` · the bundle source manifest · the MCP descriptor · install state (`state.json`) · the OCI wire format · the package index transport · the MCP server tool surface · published schema URLs · `GRIM_*` environment variables and honored vendor overrides · `.grimignore` · `grim export` output · rendered client layouts.

Touching one is a decision, not a refactor. A break is prohibited (AGENTS.md › Principle 9): evolution is additive-only, a layout move ships state migration, an old-path reaper and an upgrade fixture, and a renderer change proves self-heal (`adr_render_layout_stability.md`).
