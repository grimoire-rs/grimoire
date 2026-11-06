# Research: settings kind, scope of issue #270 (other harnesses and Appendix B keys)

## Metadata

**Date:** 2026-11-05
**Domain:** cli (vendor AI-config layouts)
**Triggered by:** `/hex-plan` for `.agents/plans/plan_settings_follow_ups.md`, issue [#270](https://github.com/grimoire-rs/grimoire/issues/270)
**Expires:** 2027-05-05

Scoping only. No key here is verified to the standard a row needs; the plan's
research steps do that, per `.claude/skills/upstream-refresh/references/deep-pass.md`.
Page text came through a summarising fetcher; no harness binary was installed or
run (`npm view` metadata only). Counts are estimates.

## Direct Answer

grim splices JSON, JSONC (`src/install/json_splice.rs`) and TOML
(`src/install/toml_splice.rs`) only. No YAML or JSON5 engine exists;
`src/install/vendor_goose.rs:31-40` records a YAML splice as a real piece of work.
Adding a JSON or TOML harness touches no settings engine file: every engine keys
off `Vendor::settings_fields()` (`src/install/vendor.rs:1261`) and
`known_fields()` (`src/oci/settings.rs:358-362`).

| Harness | File and format | Candidates verifiable / total | Credential-free real-binary check | Scoping verdict |
|---|---|---|---|---|
| agents (generic) | none, a directory convention | 0 / 0 | n/a | no rows: no settings file |
| Amp | `~/.config/amp/settings.json[c]`, `.amp/settings.json[c]`; JSONC | about 3 / 8, none in the three categories | npm `@sourcegraph/amp`, no verb found | no rows: no model or mode key, permissions moved to the plugin API, no marketplace key |
| Antigravity CLI | `~/.gemini/antigravity-cli/settings.json`; JSON; user scope only | about 18 / 30 | no npm package; IDE is a GUI | rows feasible (about 15-18 keys), global only; `permissions` stays off (action grammar unlisted). Check first that grim's `antigravity` client is this CLI |
| Cline | `~/.cline/data/settings/global-settings.json`; JSON | 0 / unknown | npm `cline`, `cline config <target> --json`, login unstated | no rows: no keys documented |
| Droid | `~/.factory/settings.json`, `.factory/settings.json`, `settings.local.json`; JSON | about 4 / 6 | npm `droid`, no validate verb | rows feasible (about 4 keys); array merge stated |
| Gemini CLI | `~/.gemini/settings.json`, `.gemini/settings.json`; JSON | about 5 / 10 | npm `@google/gemini-cli`, no settings verb | rows feasible only where merge is stated; consumer service ended 2026-06-18 |
| Goose | `~/.config/goose/config.yaml`; YAML | about 2 / 4 | not on npm | no rows: YAML, no splice engine |
| Junie | `~/.junie/config.json`, `.junie/config.json`; JSON | about 0 / 5 | npm `@jetbrains/junie`, no print verb | no rows: no value types; allowlist in a separate file |
| Kilo Code | `~/.config/kilo/kilo.json[c]`, `./kilo.json[c]`; JSONC | about 8 / 15 by Kilo's schema | npm `@kilocode/cli`, no verb | rows only if Kilo's own docs resolve values and merge |
| Kiro CLI | `~/.kiro/settings/cli.json`, `.kiro/settings/cli.json`; JSON | about 12 / 71 | `kiro-cli settings list --all`, downloaded binary | rows feasible (8-12 behaviour keys); two pages disagree on project scope |
| OpenClaw | `~/.openclaw/openclaw.json`; JSON5 | 0 / unknown | npm `openclaw`, `config schema` and `doctor` without login | no rows: JSON5, sub-pages unread |
| Warp | `~/.config/warp-terminal/settings.toml` (Linux); TOML; user only | about 12 / 52 | no, an app | rows feasible (agent profile keys); two pages disagree on hand-editing `execution_profiles` |
| Zed | JSONC `settings.json`, `.zed/settings.json` | about 4 / 10 | no, a GUI | no rows yet: path unstated on the pages read; per-tool lists are list-of-objects |

### Appendix B keys on the first six

| Key | Scoping verdict | Source |
|---|---|---|
| Claude `/theme`, `/editorMode`, `/tui`, `/attribution/*` | values found in schemastore (secondary); confirm against the official reference and `claude doctor` with a bad-value control | [schemastore](https://www.schemastore.org/claude-code-settings.json), [settings](https://code.claude.com/docs/en/settings.md) |
| Claude `/sandbox/filesystem/{allowWrite,denyWrite,denyRead,allowRead}` | string arrays; "merges them, combining the paths" (set class) | [sandboxing](https://code.claude.com/docs/en/sandboxing.md) |
| Codex `/tui/notification_condition`, `/tui/notification_method`, `/file_opener`, `/web_search` | enumerated | [config reference](https://learn.chatgpt.com/docs/config-file/config-reference) |
| Codex granular `/approval_policy` | a table of five bools; the same key is also a scalar, so a union type | same |
| Codex rules `match`, `not_match` | semantics stated; a failing example's effect is not; `codex execpolicy check` can show it credential-free | [rules](https://learn.chatgpt.com/docs/agent-configuration/rules) |
| Copilot tool rules | none exist as settings keys (CLI flags only); new keys `editorMode`, `sandbox.userPolicy.*`, `subagents.*` appear; about 40,000 characters of the page still unread | [config-dir reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference) |
| Cursor `/sandbox/mode`, `/sandbox/networkAccess`, `/model` | unresolved | [CLI configuration](https://cursor.com/docs/cli/reference/configuration) |
| OpenCode `/share`, `/username` | resolved in the schema | [schema](https://opencode.ai/config.json) |
| OpenCode `/compaction/reserved` | deprecated in the schema; stays off | same |
| OpenCode `tui.json` keys | named, types unstated | [config](https://opencode.ai/docs/config/) |
| OpenCode `config.json` file name | the docs list `opencode.json[c]` only; the recorded gap may not be one | same |
| Qoder `/model/reasoningEffort` | enumerated | [settings reference](https://docs.qoder.com/cli/settings-reference) |
| Qoder array merge, project vs user | unresolved; `on-limited` stays | [settings](https://docs.qoder.com/cli/settings), [permissions](https://docs.qoder.com/cli/permissions) |

## Key Findings

- **Drift on a first-slice row.** The OpenCode schema marks `permission` deprecated in favour of a `permissions` array of `{action, resource, effect}`; the docs page still shows the object form ([schema](https://opencode.ai/config.json), [permissions](https://opencode.ai/docs/permissions/)). The first-slice rows use `/permission/<tool>`. Re-check before building on it.
- **Two harnesses cannot be settled from one page.** Kiro's configuration page (2026-10-21) says `cli.json` is global only; its settings reference (2026-11-03) says workspace values override per key. Warp's two pages disagree on whether `agents.execution_profiles` is hand-editable.
- **Real-binary checks are mostly unavailable.** Only Claude (`claude doctor`) and Codex (`codex debug prompt-input`, `codex execpolicy check`) have recorded credential-free verbs. Among the 13, OpenClaw (`config schema`, `doctor`) and Kiro (`settings list`) are the only candidates.
- **Per-harness cost in grim** (architecture map, 2026-11-05): a JSON harness at global scope is about 250-350 lines, mostly tests and docs: the vendor const and `impl Vendor` methods, its unit tests (pattern `vendor_qoder.rs:381-500`), `SETTINGS_GOLDEN`, `SETTINGS_HOSTS` and the row-count pins in `src/install/vendor.rs` tests, the host lists in `path_anchor.rs:1288` and `installer.rs:7045` tests, `test/src/helpers.py:271-280`, `test_settings_helpers.py:83`, `test_settings_publish.py:279`, `clients.md`, `settings.md` `#fields` and `#files`, the generated schemas, and ADR Appendix A. TOML adds `settings_config_format()` only.

## Recommendation

Attempt rows for Droid, Antigravity CLI, Kiro and Warp, and evaluate Gemini and Kilo by
the verification rule. Record "no rows" with a reason for agents, Amp, Cline, Junie, Zed,
OpenClaw and Goose unless a primary source settles their keys. Fold the resolved Appendix B
keys into the first six. Re-check OpenCode's `permission` deprecation first. Do not start a
YAML or JSON5 splice engine in this run.

## Sources

All fetched 2026-11-05; none older than 18 months.

- Amp: [settings](https://ampcode.com/docs/markdown/cli/settings) (2026-11-02), [tool-level permissions](https://ampcode.com/news/tool-level-permissions)
- Antigravity: [CLI reference](https://antigravity.google/docs/cli/reference/), [settings](https://antigravity.google/docs/settings/)
- Cline: [CLI reference](https://docs.cline.bot/cli/cli-reference), [configuration](https://docs.cline.bot/cli/configuration)
- Droid: [settings](https://docs.factory.com/cli/configuration/settings)
- Gemini CLI: [configuration](https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md), [transition post](https://developers.googleblog.com/an-important-update-transitioning-gemini-cli-to-antigravity-cli/) (2026-05-19)
- Goose: [config files](https://goose-docs.ai/docs/guides/config-files)
- Junie: [config.json](https://junie.jetbrains.com/docs/junie-cli-configuration.html) (2026-10-30)
- Kilo: [CLI](https://kilo.ai/docs/code-with-ai/platforms/cli), [schema](https://app.kilo.ai/config.json)
- Kiro: [configuration](https://kiro.dev/docs/configuration/) (2026-10-21), [settings reference](https://kiro.dev/docs/reference/settings/) (2026-11-03)
- OpenClaw: [gateway configuration](https://docs.openclaw.ai/gateway/configuration), [reference](https://docs.openclaw.ai/gateway/configuration-reference)
- Warp: [settings file](https://docs.warp.dev/terminal/settings/), [all settings](https://docs.warp.dev/terminal/settings/all-settings/) (2026-11-03)
- Zed: [configuring](https://zed.dev/docs/configuring-zed), [agent settings](https://zed.dev/docs/ai/agent-settings), [tool permissions](https://zed.dev/docs/ai/tool-permissions)
- First six: [Claude settings](https://code.claude.com/docs/en/settings.md), [Claude sandboxing](https://code.claude.com/docs/en/sandboxing.md), [Codex config reference](https://learn.chatgpt.com/docs/config-file/config-reference), [Codex rules](https://learn.chatgpt.com/docs/agent-configuration/rules), [Copilot config-dir reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference), [Cursor CLI configuration](https://cursor.com/docs/cli/reference/configuration), [OpenCode config](https://opencode.ai/docs/config/), [Qoder settings reference](https://docs.qoder.com/cli/settings-reference)

Search terms: `<harness> settings.json reference`, `<harness> CLI configuration file`,
`<harness> permissions allow deny`, `schemastore <harness> settings`.
