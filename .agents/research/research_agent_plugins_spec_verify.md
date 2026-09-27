# Research: Agent Plugins 1.0 claim verification

## Metadata

- **Date**: 2026-09-27
- **Domain**: packaging
- **Triggered by**: "/hex-discuss harness-native-marketplaces — verify Agent Plugins 1.0 claim"
- **Expires**: 2027-03-27

## Direct Answer

The claim is **substantially correct, with three corrections**:

1. The spec is real: **Agent Plugins Specification 1.0.0**, home `agent-plugins.org`, source `github.com/agentplugins/agent-plugins-spec`, published **2026-08-06**. "GA" is GitHub's word for its Copilot rollout (2026-08-12); the spec itself is a "1.0.0" release. A 1.1.0 working draft already exists.
2. Backers: the TSC's Core Maintainers come from Amazon/AWS, Cursor (Anysphere), Microsoft, OpenAI and Vercel, and Google joined as a Core Maintainer on launch day. The charter seats *individuals*, and no vendor may hold a majority of seats. It is **not** hosted by the Linux Foundation's AAIF: AAIF's own blog says it "is not an AAIF project and has not submitted a proposal to become one."
3. Claude Code: Anthropic is not a maintainer, and Claude Code's docs never mention the spec. That lines up with "does not implement." But **no Anthropic statement exists either way**, and third-party claims that Agent Plugins packages "install into Claude Code today" have no primary backing. From Claude Code's documented loader rules, a spec-shaped plugin would partly load (inference, not tested): `skills/` would load because the manifest is optional and the directory is on the default layout, but root `plugin.json` would be ignored and `mcp.json` (no dot) would not be read, since Claude Code reads `.mcp.json`.

Spec scope: a root `plugin.json` is required, with only two required fields, `$schema` (a const that selects the spec version) and `name`. `version` is optional release metadata; semver is recommended, not required. Components are `skills/` (Agent Skills) and `mcp.json`, and nothing else. Agents, commands, hooks, rules and LSP are explicitly deferred. Client-specific data goes under `extensions`, keyed by reverse-domain namespace. v1 defines no permissions, sandboxing, signing or secrets.

## Per-part verdicts

| # | Part | Verdict | Evidence |
|---|---|---|---|
| 1 | Canonical home, versioned spec, name, version, date | **CONFIRMED** (date from vendor posts; the spec site carries no date) | [agent-plugins.org](https://agent-plugins.org), [spec repo](https://github.com/agentplugins/agent-plugins-spec), [Google blog 2026-08-06](https://developers.googleblog.com/agent-plugins-package-your-skills-tools-and-more/), [AAIF blog 2026-08-06](https://aaif.io/blog/from-skills-and-tools-to-portable-agent-plugins) |
| 2a | Backers: Amazon, Cursor, Microsoft, OpenAI, Vercel, plus Google | **CONFIRMED** | [agent-plugins.org](https://agent-plugins.org) ("initial Technical Steering Committee includes Core Maintainers from Amazon, Cursor, Microsoft, OpenAI, and Vercel"), [Google blog](https://developers.googleblog.com/agent-plugins-package-your-skills-tools-and-more/) ("joining that group as a Core Maintainer"), [GOVERNANCE.md](https://github.com/agentplugins/agent-plugins-spec/blob/main/GOVERNANCE.md) (individual seats, no vendor majority) |
| 2b | Copilot implements it | **CONFIRMED** | [GitHub changelog 2026-08-12](https://github.blog/changelog/2026-08-12-agent-plugins-1-0-in-vs-code-copilot-cli-and-the-copilot-app/), [VS Code docs](https://code.visualstudio.com/docs/agent-customization/agent-plugins) |
| 2c | Codex implements it | **CONFIRMED** | [OpenAI Codex docs](https://developers.openai.com/codex/plugins/build) ("For a portable Agent Plugins package, add `plugin.json` at the plugin root and declare the Agent Plugins schema"; `.codex-plugin/plugin.json` kept "as a compatibility fallback") |
| 3 | Scope: `plugin.json` + `skills/` + `mcp.json` | **CONFIRMED**. Required fields are `$schema` + `name`; `version` is optional, with semver recommended only | [spec](https://agent-plugins.org/specification), [plugin.schema.json](https://agent-plugins.org/schemas/1.0.0/plugin.schema.json) (`additionalProperties: false`, `extensions` object) |
| 4 | Claude Code does not implement it | **CONFIRMED** from docs silence and non-membership. **UNCONFIRMED** for any Anthropic statement (none found) | [Claude Code plugin manifest reference](https://code.claude.com/docs/en/plugins-reference) (manifest at `.claude-plugin/plugin.json`, optional; `$schema` "Claude Code ignores it at load time"; MCP from `.mcp.json`) |
| 5 | GitHub "Agent Plugins 1.0" changelog: same spec or a Copilot product name? | **Same spec**. GitHub calls it "an open standard ... governed independently of any single vendor" and links agent-plugins.org; "GA" refers to the Copilot surfaces (VS Code, Copilot CLI, Copilot SDK, Copilot app) | [GitHub changelog](https://github.blog/changelog/2026-08-12-agent-plugins-1-0-in-vs-code-copilot-cli-and-the-copilot-app/) |
| 6a | AAIF exists under the Linux Foundation | **CONFIRMED** (formed 2025-12-09; MCP, goose, AGENTS.md) | [LF press release](https://www.linuxfoundation.org/press/linux-foundation-announces-the-formation-of-the-agentic-ai-foundation) |
| 6b | AAIF hosts or endorses Agent Plugins | **REFUTED** for hosting. AAIF blogged about it as a "complementary, independently governed effort" | [AAIF blog](https://aaif.io/blog/from-skills-and-tools-to-portable-agent-plugins) |
| 7a | Cursor implements it | **CONFIRMED**. It reads root `plugin.json` with the spec `$schema`, and its native `.cursor-plugin/plugin.json` still exists | [Cursor docs](https://cursor.com/docs/plugins) |
| 7b | Gemini CLI implements it | **UNCONFIRMED / no evidence**. Its docs require `gemini-extension.json` and never mention the spec. Google's own launch products are Agents CLI and Data Agent Kit, and Google says it is "starting to build support" | [Gemini CLI extension docs](https://geminicli.com/docs/extensions/writing-extensions/), [Google blog](https://developers.googleblog.com/agent-plugins-package-your-skills-tools-and-more/) |
| 7c | OpenCode implements it | **REFUTED** as of 2026-09-27. A feature request is open | [anomalyco/opencode#40993](https://github.com/anomalyco/opencode/issues/40993) (opened 2026-08-07, open) |

## negative:

- The spec site publishes no client or implementation list: `/clients` returns 404, and neither the homepage nor `/plugin-authors` lists clients. "Launch clients: ChatGPT, Codex, Cursor, GitHub Copilot, Kiro, VS Code" appears only in secondary blogs. Kiro and ChatGPT support were not verified against Kiro or OpenAI docs.
- No primary source (Anthropic docs, changelog or blog) mentions Agent Plugins. The widely repeated claim that Claude Code "installs Agent Plugins today" traces to [scienceshot.com](https://scienceshot.com/post/agent-plugins-1-0-claude-code), which cites nothing.
- The Google blog's mention of "Antigravity, Gemini CLI, Claude Code, or Cursor" describes where Google's Agents CLI *skills* can be used. It is not a statement that those clients implement the spec.
- The Codex version that shipped support ("0.147.0, 2026-08-07") appears only in the OpenCode issue text. It is unverified against Codex release notes.
- The spec site gives no publication date. The 2026-08-06 date comes from the Google, AAIF and GitHub posts.

## Sources

- https://agent-plugins.org
- https://agent-plugins.org/specification
- https://agent-plugins.org/schemas/1.0.0/plugin.schema.json
- https://github.com/agentplugins/agent-plugins-spec
- https://github.com/agentplugins/agent-plugins-spec/blob/main/GOVERNANCE.md
- https://developers.googleblog.com/agent-plugins-package-your-skills-tools-and-more/
- https://aaif.io/blog/from-skills-and-tools-to-portable-agent-plugins
- https://www.linuxfoundation.org/press/linux-foundation-announces-the-formation-of-the-agentic-ai-foundation
- https://github.blog/changelog/2026-08-12-agent-plugins-1-0-in-vs-code-copilot-cli-and-the-copilot-app/
- https://code.visualstudio.com/docs/agent-customization/agent-plugins
- https://developers.openai.com/codex/plugins/build
- https://cursor.com/docs/plugins
- https://code.claude.com/docs/en/plugins-reference
- https://geminicli.com/docs/extensions/writing-extensions/
- https://github.com/anomalyco/opencode/issues/40993
