---
paths:
  - src/command/config_keys.rs
  - src/config/declaration.rs
---

# Config Key Title/Description Style

`KeySpec` (`src/command/config_keys.rs`) is the single source of truth for
every dotted `grim config` key's `title` and `description`. It renders in
three places: the JSON envelope from `grim config list --format json`, the
published JSON Schema (`grim schema --kind config`), and any editor surface
that reads that schema (e.g. the VS Code extension's settings UI). The
plain-text `grim config list` table shows only `Key` and `Value` columns —
`title`/`description` never render there. Write for that reader — a CLI or
extension user deciding whether to set the key — not for a grim
contributor.

`title` is free-form and untested. `description` is checked by
`config_key_metadata_matches_published_schema`: the whitespace-normalized
text must be a **prefix** of the whitespace-normalized doc comment on the
matching field in `config::declaration.rs` (that doc comment is what
schemars turns into the schema's `description`). The two are independent
copies, not one source — editing either one requires re-checking the
other still forms a valid prefix pair. Run
`cargo nextest run config_key_metadata_matches_published_schema` after
touching either file.

## Rules

- **Describe the effect, not the mechanism.** Say what the user observes,
  never a Rust type, module path, or internal function name — nothing in
  `description` should look like a symbol the reader could go read source
  for. (`command::resolve_default_registry` is banned; the flag/env var it
  resolves is not.)
- **Say what happens when the key is unset.** A bare default (`null`,
  `false`) tells the reader nothing; spell out the fallback behavior.
- **Sentence-style capitalization, terminal period**, even for one-clause
  fragments.
- **Booleans: "Controls whether X", never "Controls if X" or literal
  `true`/`false`.** Describe the effect in plain language instead.
- **Backtick every code-literal**: flags (`` `--client` ``), env vars
  (`` `GRIM_DEFAULT_REGISTRY` ``), other config keys, literal values a user
  types. Never backtick prose words.
- **Name interacting sources and say who wins.** A key settable from a CLI
  flag, an env var, and project/global config needs its precedence stated
  — "why didn't my setting take effect" is the question this line answers.
- **One to two sentences by default, aim under ~160 characters.** A third
  sentence is allowed only when it states a user-critical interaction fact
  that would otherwise be lost — an override or precedence source, an
  unset-fallback value, or a mutual-exclusion constraint. Everything else
  goes into the `declaration.rs` doc-comment continuation, which schema
  consumers can still read.
- **Plain words, no jargon, no Latin abbreviations** (`for example`, not
  `e.g.`).
- **Never say "we"**, and never restate the key's own title as the opening
  words of the description.
- **Lead with the effect** ("Sets…", "Controls whether…", "Determines…") so
  the first sentence stands alone.
- **Never restate scope precedence per key.** Project config overrides
  global config uniformly, for every key — that rule is documented once, in
  the general configuration docs (`docs/src/configuration.md` /
  `concepts.md#scopes`). Don't add a "project overrides global" clause to an
  individual key's description; it would just repeat the same sentence 10
  times over.

## Anti-patterns

- Internal identifiers in prose: `command::resolve_default_registry`,
  schema `$defs` names, struct field names.
- `"Controls if X is enabled"` — use `"Controls whether"` + enabled/disabled
  language.
- A setting that interacts with a flag or env var, with that interaction
  left unstated.
- A third sentence that adds no interaction-critical fact — elaboration,
  examples, or restating the default a second time — instead of moving that
  content to the `declaration.rs` doc-comment continuation.
- Backtick-free flags/env vars/keys mixed unreadably into plain prose.

## Before/after, from this repo's history

**`options.default_registry`** — bad (leaked an internal function path,
buried the one fact a user actually needs):

> Default registry for short identifiers (lower priority than
> `GRIM_DEFAULT_REGISTRY`; see the registry-precedence chain in
> `command::resolve_default_registry`).

Good — states the effect first, then the `[[registries]]` exception, then
the override chain, then the built-in fallback (rule: say what happens when
the key is unset):

> Registry used when an artifact reference names no registry. Ignored when
> a `[[registries]]` entry is declared — the array's default entry expands
> short identifiers instead. Overridden by the `--registry` flag or
> `GRIM_DEFAULT_REGISTRY` environment variable when set. Falls back to
> `ghcr.io/grimoire-rs` when this key, `--registry`, and
> `GRIM_DEFAULT_REGISTRY` are all unset.

Four sentences here use the interaction-fact allowance: the `[[registries]]`
exception (second sentence), the override chain (third), and the unset
fallback (fourth) are all facts a user would otherwise lose.

**`options.show_deprecated`** — bad (boolean phrased via literal
`true`/`false` instead of "whether"):

> When false (default), deprecated artifacts are hidden from `grim search`
> and the TUI catalog unless installed; true shows them everywhere.

Good:

> Controls whether deprecated artifacts appear in `grim search` and the
> TUI catalog. Hidden by default unless already installed.
