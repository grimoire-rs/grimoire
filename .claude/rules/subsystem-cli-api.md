---
paths:
  - src/**
---

# CLI API Data Layer Patterns

Standards for the API reporting layer: flat report modules
`src/api/{name}_report.rs`, re-exported from `src/api.rs`. Rules ensure
a consistent output format across all commands. No `Api` facade, no
`api/data/` subdirectory — commands return report values; the dispatch
arm in `src/app.rs` renders them via the `Printable` trait (plain or
`--format json`).

## Data Type Structure

Every report file in `src/api/` follow this structure:

1. **Doc comments** on all public types — describe purpose, plain format, JSON format:
   ```rust
   /// Short description of what this represents.
   ///
   /// Plain format: N-column table (Col1 | Col2 | Col3).
   ///
   /// JSON format: shape description (array of objects, keyed object, etc.).
   ```
2. **`new()` constructor** (or named constructors for polymorphic types like `without_tags` / `with_tags`)
3. **`Printable` impl** with single `print_table` call — no conditional empty-checks, no multiple tables
4. **Static `&str` headers** in `print_table` — never `format!()` for dynamic headers; add data columns instead

Reference impls: `src/api/install_report.rs` (multi-item, `items`
envelope), `src/api/release_report.rs` (single-item),
`src/api/publish_report.rs` (multi-item batch with typed status enum and
a sibling `announce` key).

## Single-Table Rule

Each `Printable::print_plain()` impl produce exactly one table. Multiple dimensions (e.g., type + path, status + content) → encode as columns, not separate tables with dynamic headers.

**Wrong:**
```rust
if !self.objects.is_empty() {
    let header = format!("Object{}", suffix);
    print_table(&[&header], &rows);
}
if !self.temp.is_empty() {
    print_table(&[&format!("Temp{}", suffix)], &rows);
}
```

**Right:**
```rust
print_table(&["Type", "Dry Run", "Path"], &rows);
```

## Report Actual Results

Commands report what happened, not echo input. Task methods return enough data for command to build accurate output.

- **Task return values drive report.** Task can be no-op (resource already absent) → return type must encode this (e.g., `Option<PathBuf>` where `None` = no-op).
- **Never build report data from `self.packages` (CLI args) alone.** Use task return value for status.
- **Preserve input order.** `_all` methods return results in same order as input `packages` slice, so caller zip with original identifiers.

## Typed Enums Over Strings

Status values, category tags, bounded sets = enums with `Display` and `Serialize` impls — never raw `String` fields.

```rust
#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum RemovedStatus {
    Removed,
    Absent,
}
```

## JSON Serialization

- **Multi-item reports use the uniform `{"items": [...]}` envelope.** Name
  the inner `Vec` field `items` and `#[derive(Serialize)]` on the report —
  no hand-written `Serialize` impl, no bare top-level array. Bare arrays
  can never grow a sibling field under the 1.0 additive-field policy; the
  envelope can. `publish_report.rs` is no longer an exception — it derives
  `{"items": [...], "announce": ...}` the same way.
- **Optional fields are always-present-null.** `skip_serializing_if` is
  banned in `src/api/` — an absent-key optional breaks consumers that
  distinguish "not applicable" from "older grim". MCP-only payloads
  (`src/mcp/fetch.rs`, `src/mcp/render.rs`) are exempt.
- Single-object reports serialize as one flat object (e.g.
  `init_report.rs`, `release_report.rs`).
- Polymorphic types use `#[serde(untagged)]` to produce different JSON shapes per variant.
- Consumer contract, error document, and per-command shapes:
  `docs/src/json-interface.md`.

## Adding a New Report Type

1. Create `src/api/{name}_report.rs` with struct + doc comments +
   `Printable` impl
2. Add `pub mod {name}_report;` (+ re-export) to `src/api.rs`
3. Return the report from `command/{name}.rs` built from operation
   results; render it in the matching dispatch arm in `src/app.rs`

## Payload-Plain Reports

Two exemption tiers from the table-plain norm:

- **Fully exempt** (`schema`, `tui`, `mcp`): no report module at all — the
  command emits a document / owns the terminal / speaks JSON-RPC, wired
  directly in `app.rs`.
- **Payload-plain, JSON-standard** (`fetch`): the report module exists and
  `print_json` is a normal report, but `print_plain` writes the raw
  content payload — exact bytes, no table, no added trailing newline — so
  the output pipes (`grim fetch ref --path X > file`). Anything
  human-facing (warnings) goes to stderr via `tracing`, never stdout.

## Commands That Exec a Child Process

A command whose job is to replace/spawn a child process is exempt from the
`Printable` / report-module path: it does not emit structured output
because execution diverges into the child. Keep any CLI-shaped error for
such a command local to its `command/{name}.rs` file rather than in a
report module — it carries CLI wording and exits before any structured
payload exists.