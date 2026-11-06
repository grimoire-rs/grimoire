# Discussion: Harness configuration as a grim artifact type

State: handed-off → loop · Updated: 2026-11-03
Ratified: 2026-11-03 → loop
Confidence: ratified by the owner (explicit "yes" to the restate); decisions backed by 5 research artifacts dated 2026-11-03 (`.agents/research/research_config_artifact_*.md`), vendor docs read that day, and the code paths cited below; behaviour was read, not run, apart from the local `git pull` test.

## Intent

Could grimoire transfer harness configuration, as a new artifact type? Open
from the start: whether there should be one layer per harness, a generic form,
or both.

Requested outcome: a goal loop (`/hex-loop`), with this file as its source.
Precedent from `.agents/discussions/otel-usage-telemetry.md`: the loop's first
deliverable is an ADR, here also amending the trust records.

Out of scope for the first slice: whole harness profiles, including env and
endpoints; hook blocks in settings, which would bypass the hook dispatcher and
its feature flag; generic cross-harness keys; harnesses beyond the six named
below; re-asking consent on a version bump; exporting this kind into harness
plugins.

## Requirements

- The first slice covers behaviour defaults (model, output, tool and UI
  options), permission policy (allow and deny rules, sandbox and approval
  modes), and pre-configured marketplaces where a harness has a settings key.
- Public packages are in scope, as well as own machines and teams.
- First-slice harnesses: Claude, Codex, Copilot, Cursor, OpenCode, Qoder.
- A follow-up GitHub issue tracks support for every other harness. Filing it is
  a loop deliverable.
- The `upstream-refresh` skill gains a check of each vendor's settings keys, so
  key-list churn is reviewed: `.claude/skills/upstream-refresh/SKILL.md`,
  `.claude/skills/upstream-refresh/references/domains.md`,
  `.claude/skills/upstream-refresh/references/researcher-brief.md`,
  `.claude/skills/upstream-refresh/references/deep-pass.md`, and
  `.claude/rules/vendor-capability-watchlist.md`.
- Any run that changes applied settings lists the changed keys, including a
  `grim install` that follows a teammate's lock bump.
- Documentation is a first-class deliverable, written with the `docs` skill and
  held to `.claude/rules/docs-quality.md`, every example in the required gate:
  - A reference page, `docs/src/content/docs/settings.md`, on the document
    format, merge classes, ownership, `--force`, the error, and the per-harness
    field table, added to the sidebar in `docs/astro.config.mjs`.
  - A how-to guide, `docs/src/content/docs/guides/share-settings.md`, on sharing
    settings, linking each harness's own settings docs, with use cases: basic
    behaviour defaults, allow and deny rules, and a pre-configured marketplace.
  - Updates to `commands.md`, `json-interface.md`, `artifacts.md`, `clients.md`,
    `concepts.md`, `stability.md` and `upgrading.md` under
    `docs/src/content/docs/`, plus the catalog drift review in
    `catalog/README.md`.
  - Harness links for the guide: <https://code.claude.com/docs/en/settings>,
    <https://learn.chatgpt.com/docs/config-file/config-reference>,
    <https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference>,
    <https://cursor.com/docs/cli/reference/permissions>,
    <https://opencode.ai/docs/config/>, and the Qoder page at docs.qoder.com,
    still to be located.
- A how-to guide for declared dependencies,
  `docs/src/content/docs/guides/dependencies.md`, added to the sidebar in
  `docs/astro.config.mjs`. It is unrelated to the settings kind: the owner asked
  for it as a drive-by, so it takes its own `docs:` commit and can land first.
  No guide exists today; the material sits in
  `docs/src/content/docs/artifacts.md#requires`,
  `docs/src/content/docs/concepts.md#dependencies` and
  `docs/src/content/docs/commands.md#why`. It states when to use a bundle and
  when `requires`, and how to read `grim why`. Two worked cases:
  - A lifecycle package of several skills and agents, modelled on the six
    stages of the AI-native SDLC playbook
    (<https://claude.com/resources/articles/the-ai-native-sdlc-playbook>):
    plan, design, build, test, deploy and maintain, with shared templates as a
    required skill and review subagents as members.
  - A skill or agent that requires an MCP server, as
    `grim.requires.mcp.gdrive` does in the reference.

## Decisions

- **Ownership comes from the lock.** A piece of settings is managed by grim
  when a settings package in the lock declares it. Install, add and update
  converge files to the lock.
- **The file decides; a record never authorises an overwrite.** Grim changes or
  removes a value without `--force` only if the file still holds exactly what
  grim recorded there, so a value grim owns can always be updated. Anything else
  needs `--force`. The owner's objection set this: a committed settings file or
  a copied-in config changes behind grim's back, so a record cannot say what the
  file holds now. The file is read first, every time, with the check skills and
  rules already use (`src/install/installer.rs:1582`).
- **The error.** It lists every affected piece across all settings packages,
  marked `overwrite`, before any settings write. Other artifacts in the run
  still install, as for MCP's `RefusedUntracked` (`src/command/install.rs:524`).
  `--force` hands the piece to grim for good: uninstall removes it, and the old
  value appears only in the error.
- **A matching value is adopted, as for MCP.** A file that already holds the
  lock's value, with no record, gets no write and no `--force`, on any command.
  The value is recorded as `adopted` (`src/install/installer.rs:2785`), uninstall
  leaves it, and it follows the package until a version changes it, when grim
  writes it and owns it. This replaces the `ownership` error in the owner's lock
  proposal: grim no longer takes ownership of anything it did not write. The
  owner confirmed this at the restate.
- **Grim's existing indicators and refusals apply.** Each value is an `entry`
  output with a semantic hash, as for MCP
  (`docs/src/content/docs/mcp-servers.md`), so reordering a file never flags
  `modified`, and `grim status`, the TUI and the install gate share one check:
  - `installed`: every declared value is in place at the locked version.
  - `outdated`: values intact, and the lock has moved on.
  - `modified`: a recorded value now differs, by hand, copy or pull.
  - `missing`: no record on this machine, or a recorded value was deleted.
  - `stale`: the lock no longer matches `grimoire.toml`.
  - `gated`: this workspace has not consented. The token is documented as
    hook-only today (`src/api/artifact_status.rs:46`); widening it is additive.
  A refusal keeps the existing forceable reasons, exit 65: `modified` with a
  record, `untracked-destination` without one.
- **A missing hash grants nothing, and no hash goes in the lock.** `content_hash`
  is a required field, so no hash means no record, which reads as "grim did not
  write this": identical values are adopted and different ones refuse
  (`src/install/installer.rs:2819`). `grim status` reports `missing`. The lock
  already fixes the expected values by pinning the package digest
  (`src/lock/locked_artifact.rs:52`). The record holds a per-machine fact, and
  after a pull a hash in the lock would describe the new values, not what this
  machine wrote.
- **Consent is per kind, with no key logic.** Settings are always a serious
  change, so this kind always requires consent, whatever keys a package holds.
  Project scope uses the workspace consent record. Global scope is already
  always consented, as for hooks, and `grim add` is the gesture.
- **One consent mechanism, never per kind.** The owner rules out a second
  mechanism and rules out consenting to kinds individually: one record per
  workspace, one prompt per command, one answer. The owner's message read "to
  consent mechanisms"; this records it as "two", to be corrected if wrong.
- **Version bumps behave as for hooks.** No re-ask. The change list is the
  evidence (I5). Invariant I2's recorded hook exception extends to this kind,
  which needs amending `.claude/rules/arch-threat-model.md`.
- **File shape: one JSON document with vendor objects, in one layer.** The MCP
  model in `src/oci/mcp.rs`: authored as `settings/<name>.toml`, published as
  canonical JSON with its own layer media type and a size cap, never
  materialized as a file of its own. Vendor objects keep native key names.
- **Vendor keys only.** `<vendor>.<field>` keys from a typed list, no generic
  core. Generic keys can be added later.
- **Public packages too.** The owner chose this over the own-machines-only
  recommendation, knowing the demand evidence is thin.

## Threads

- **Where the record lives.** The record exists because `grim add` and
  `grim update` can compare the old and new locks, while `grim install` after
  `git pull` cannot: the old lock is gone. It is the existing record type, with
  one output per value, and a list element needs an element identity on top of
  today's `entry` pointer. Global scope already keeps it in
  `$GRIM_HOME/state/global.json`. Project scope needs one new file,
  `$GRIM_HOME/settings/<workspace-key>.json`, with the key from
  `workspace_key` in `src/install/hook_dispatch.rs:349`. It must not live in
  `<workspace>/.grimoire/state.json`: in a local test on git 2.43 a `git pull`
  silently replaced a gitignored state file with a committed one, exit 0, and a
  forged record could remove a user's `deny` rule, because removal needs no
  consent. Kubernetes Server-Side Apply has the same split: manifests live in
  git, while ownership is stored with the live object
  (<https://kubernetes.io/docs/reference/using-api/server-side-apply/>).
- **The rule, per value.**
  - File holds nothing: write it.
  - File holds the lock's value: nothing to write. With no record, adopt it.
  - File holds what grim recorded, and the lock differs or no longer declares
    it: change or remove it, and list it. A teammate's bump and a revoked rule
    reach every machine this way.
  - Anything else: `--force`, marked `overwrite`. A copied or pulled config
    lands here, as does a value grim recorded and the user then changed.
  A list element shared by two packages stays while any package in the lock
  declares it, with no owner tracking. Two packages setting different values for
  one scalar are refused before any write, with exit 78, as a dependency
  diamond, and that is computable from the lock alone.
- **Costs of this rule.** A hand edit or Claude's `/model` on a declared key is
  not reverted: it is `modified`, and `grim install` exits 65 until the user
  passes `--force` or changes the package. `--force` does not save the old
  value. Losing the record turns grim's earlier writes into adopted values, so
  they stay on uninstall, and a rule a package dropped in that gap stays in the
  file, unreported.
- **Merge classes.** Each key on the typed list declares one, matching
  Server-Side Apply's `set`, `map` and `atomic` types:
  - Object: never written whole. Grim recurses into each key the package sets;
    keys the package does not mention are untouched. A missing object is
    created with only the package's keys and removed on uninstall if empty.
  - Unordered list: a set union; identity is the element.
  - Keyed map, such as Claude's `extraKnownMarketplaces`: one piece per entry.
  - Scalar: compared whole.
  - Ordered list or an object meaningful only whole, such as Claude's
    `fallbackModel` and `modelPicker` or OpenCode's `permission.bash` patterns:
    treated as one scalar value.
  - List of objects: needs a declared identity key, as `upsert_nested_handler`
    in `src/install/json_splice.rs` does for hooks; otherwise one scalar value.
  - A different type at a path is an `overwrite`.
  A shape that fits none stays off the list. A user's sibling keys and elements
  are never in the comparison, so they cannot conflict.
- **Committed settings files.** The committed things are `grimoire.toml` and
  the lock; rendered settings stay per-developer. A repo's `.claude/settings.json`
  is never written: project scope writes `.claude/settings.local.json`, Claude
  merges lists across both, and the local value outranks the committed one for
  a scalar. Other harnesses are global-only, because `.codex/config.toml` is a
  tracked file (`src/install/vendor_codex.rs:226`, amendment A1 in
  `.agents/adr/adr_hooks_support.md`). Copilot's local
  `.github/copilot/settings.local.json` is a candidate to verify. A tracked
  local file or a dotfiles-synced global file needs no special rule: identical
  values are adopted, and different ones get the `overwrite` error.
  `ExcludeOutcome::AlreadyTracked` in `src/install/hook_registrar.rs` stays a
  `grim status` note.
- **Removal needs no consent, by the hook rule.** A teammate dropping a package
  from the lock removes its rules at the next `grim install` with no prompt.
  That is a reviewed commit (non-goal N1), and the change list shows it.
- **Pre-configured marketplaces.** Claude's `extraKnownMarketplaces` is a named
  map with a nested `source`, valid in any settings file, applied immediately
  from user settings and after workspace trust from a repository's files
  (<https://code.claude.com/docs/en/settings-reference>). The guide can pair it
  with `grim export marketplace`. Other harnesses document
  `plugin marketplace add` commands (`docs/src/content/docs/commands.md:2452`);
  a settings key there is unverified.
- **Merge engine, the remaining build cost.** Existing: span-preserving edits in
  `src/install/json_splice.rs`, `src/install/toml_splice.rs` and
  `src/install/managed_config.rs`. New: `split_pointer` admits only
  `/container/member`; `upsert_array_element` handles root-level arrays only;
  `toml_splice` has no element operations. History: 19 fix commits across eight
  failure modes in shared-file merging, all found after first landing, including
  comment loss and symlinked dotfiles replaced by regular files.
- **Own a file where the harness allows it.** Codex reads a `rules/` folder
  under each active layer, so grim can own those files outright and skip
  merging. Claude has no user-tier drop-in; `managed-settings.d` is managed
  tier only.
- **Consent wiring.** The prompt is in `src/hook/trust.rs:341` and the record
  in `src/hook/consent.rs:110`. `evaluate` is a set difference
  (`src/hook/consent.rs:192`), so settings bindings can sit in the existing
  record with no format change, and older grims ignore them. The kind filter
  in `src/command/hook_consent.rs` widens to this kind, and
  `src/command/hook/allow.rs` gains a kind-neutral alias. Trust ADR decision 2
  needs an amendment, which the owner accepts. Transitive delivery follows the
  hook rule, with no rule based on how an artifact was reached, plus the
  disclosure in `disclose_bundle_hooks`.
- **`GRIM_HOME` guard.** Any settings write must reuse the hook refusals
  `grim-home-relative` and `grim-home-in-workspace`, because a repository can
  carry its own `GRIM_HOME` through `.envrc` or a devcontainer, and both the
  consent record and the settings record live there.
- **Kind touch surface.** `src/oci/artifact_kind.rs`, `src/oci/layer.rs`
  (`is_tar_kind`), `src/install/path_anchor.rs`, `src/install/installer.rs`,
  `src/install/uninstall.rs`, `src/install/vendor.rs` and the six vendor files,
  `src/command/status.rs`, `src/api/artifact_status.rs`, `src/export/stage.rs`,
  `src/lock/grimoire_lock.rs`, plus JSON reports and schemas.
- **Public surface.** `grim export` declines this kind, since Claude plugins
  honour only `agent` and `subagentStatusLine`
  (<https://code.claude.com/docs/en/plugins-reference>). The index,
  `grim search` and the VS Code extension must learn the kind.
- **Vendor evidence.** Claude applies `allow` rules from an untracked
  `.claude/settings.local.json` without workspace trust, and `deny` and `ask`
  rules right away (<https://code.claude.com/docs/en/settings>). Gemini CLI
  ignores `allow` and `yolo` in extension policies
  (<https://geminicli.com/docs/extensions/reference/>).
- **Trust boundary, as read from the record.**
  `.agents/adr/adr_artifact_trust_model.md` holds all kinds equally trusted,
  says the hook exception is "not an invitation to gate the next kind", and
  says grim "is not the execution policy layer". The last clause sits in the
  threat table, so it scopes what grim defends against and does not forbid
  installing permission config.
- **Existing reach.** MCP descriptors name a spawned `command`
  (`src/oci/mcp.rs:132`). Claude fields `allowed-tools` and `permission-mode`,
  including `bypassPermissions`, already pass through
  (`src/install/vendor_claude.rs:85`, `src/install/vendor_claude.rs:137`).
- **Compatibility.** A new kind follows the `hook` precedent: older grims
  refuse only a lock that uses it, with exit 78, and a lock without it stays
  byte-identical (`docs/src/content/docs/stability.md:441`).
- **Case against building.** Permission-preset requests in Claude Code drew
  zero reactions and closed as not planned (anthropics/claude-code#48097,
  anthropics/claude-code#8172), and no package tool surveyed distributes
  permissions.

## Research

- `.agents/research/research_config_artifact_codebase.md`
- `.agents/research/research_config_artifact_prior_art.md`
- `.agents/research/research_config_artifact_vendor_semantics.md`
- `.agents/research/research_config_artifact_archaeology.md`
- `.agents/research/research_config_artifact_community.md`

## Related

- `.agents/adr/adr_artifact_trust_model.md`
- `.agents/adr/adr_hook_workspace_consent.md`
- `.agents/adr/adr_hooks_support.md`
- `.agents/adr/adr_agent_artifact_kind.md`
- `.agents/adr/adr_artifact_dependencies.md`
- `.agents/adr/adr_harness_marketplace_export.md`
- `.claude/rules/arch-threat-model.md`
- `.claude/rules/vendor-capability-watchlist.md`
- `docs/src/content/docs/stability.md`

## Open questions

- [NEEDS CLARIFICATION: Should this kind ship behind `[options.experimental]`,
  as hooks did?]
  Recommended: yes — vendor keys churn, and the flag keeps the format changeable
  before the 1.0 freeze.
- [NEEDS CLARIFICATION: Is the kind named `settings` or `config`?]
  Recommended: `settings` — "config" already means grim's own config and the OCI
  config blob.
- [NEEDS CLARIFICATION: Does `--trust-hooks` also answer for this kind?]
  Recommended: yes, as an alias of a new kind-neutral flag — one answer, one
  mechanism.
- [NEEDS CLARIFICATION: Which fields can each harness carry in the first
  slice?]
  Recommended: verify before listing — Qoder's permission keys were not
  researched, Copilot's tool grammar was not found, and Cursor's merge rule is
  not stated. An unverified field stays off the list and moves to the follow-up
  issue.
- [NEEDS CLARIFICATION: Does Claude's `enabledPlugins` join the key list?]
  Recommended: not in the first slice — turning a plugin on runs publisher code
  outside grim's hook gates, and `extraKnownMarketplaces` alone meets the
  marketplace use case.
- [NEEDS CLARIFICATION: Do the other five harnesses have a marketplace settings
  key?]
  Recommended: verify in the loop — only Claude's key is confirmed.
- [NEEDS CLARIFICATION: Which file does a linked worktree use on Claude?]
  Recommended: verify against a real binary — Claude's docs say it uses the main
  checkout's local file and still reads one in the starting directory, with the
  root's value winning on a shared key.
- [NEEDS CLARIFICATION: How does grim handle a harness writing between its read
  and write?]
  Recommended: re-read immediately before the write — no bug is on record, but
  Claude writes `.claude/settings.local.json` during a session.

## Verification

- `task verify`, `task docs:check` and `task catalog:verify` pass.
- Acceptance tests cover every row of the rule above for JSON and TOML and each
  merge class, and replay the failure modes in
  `.agents/research/research_config_artifact_archaeology.md`: comments survive
  and a symlinked dotfile stays a symlink.
- The error lists every affected piece across packages, marked `overwrite`, in
  text and JSON, and no setting is written.
- A config copied over the file, and a value changed by `git pull`, both end in
  that error, never in a silent overwrite. Reordering keys flags nothing.
- `grim status` reports each indicator as defined under Decisions, and a refusal
  carries `modified` or `untracked-destination` with `forceable`.
- A value the user already held at the same setting is adopted, with no write
  and no `--force`, and survives uninstall.
- A teammate's `--force` committed through the lock does not overwrite another
  machine's value: that machine gets the same error.
- A fresh clone declaring a settings package, under a non-interactive
  `grim install`, writes no setting, exits 0, and reports
  `workspace-not-consented` in `grim status`.
- A forged committed `.grimoire/state.json`, including one delivered by
  `git pull` over a real state file, neither applies nor removes a setting.
- A settings file arriving without a record, and a deleted record, adopt
  identical values and refuse different ones, with nothing clobbered or removed.
- An older grim refuses a lock using this kind with exit 78, and a lock without
  it stays byte-identical.
- Rendered settings load in real Claude and Codex binaries.
- `/security-auditor` passes on the trust and consent changes.
- The reference page and both guides pass the docs-quality gate, with each use
  case run by a test.
- The follow-up issue exists, and `/upstream-refresh` reports settings-key
  changes for a named harness.
