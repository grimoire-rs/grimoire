# Research: Harness change feeds a sweep can diff against

## Metadata

**Date:** 2026-09-27
**Domain:** cli
**Triggered by:** `.agents/discussions/harness-capability-freshness.md` — vendor change-feeds lane
**Expires:** 2027-03-27

## Direct Answer

Gemini CLI is the only harness with an in-repo changelog, public doc sources
and a hosted settings schema. Claude Code, Codex, Copilot CLI, OpenCode, Zed,
Cline, Goose and Kilo expose GitHub Releases or an in-repo `CHANGELOG.md`.
Cursor, Kiro, Amp, Antigravity, Warp and Qoder expose only a hosted changelog
page, with no machine-readable feed found.

## Feeds

| Harness | Changelog feed | Docs source | Published schema | Cadence |
|---|---|---|---|---|
| Claude Code | [CHANGELOG.md](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md), GH Releases, [docs changelog](https://code.claude.com/docs/en/changelog) | not public | [SchemaStore claude-code-settings](https://json.schemastore.org/claude-code-settings.json), community, reported stale ([#5484](https://github.com/SchemaStore/schemastore/issues/5484)) | several releases/week |
| Codex CLI | CHANGELOG.md + [GH Releases](https://github.com/openai/codex/releases), [dev changelog](https://developers.openai.com/codex/changelog) | in-repo `docs/` | unconfirmed | near-daily |
| Copilot | CLI [GH Releases](https://github.com/github/copilot-cli/releases); [github.blog changelog](https://github.blog/changelog/label/copilot/); [VS Code updates](https://code.visualstudio.com/updates) | [github/docs](https://github.com/github/docs), [vscode-docs](https://github.com/microsoft/vscode-docs) | VS Code in-product schemas only | weekly + monthly |
| OpenCode | [opencode.ai/changelog](https://opencode.ai/changelog) + GH Releases (canonical org unconfirmed) | in-repo | [opencode.ai/config.json](https://opencode.ai/config.json), reported stale | daily patches |
| Cursor | [cursor.com/changelog](https://cursor.com/changelog) page only | closed | none | frequent |
| Kiro | [kiro.dev/changelog](https://kiro.dev/changelog/) page only | closed | unconfirmed | several/month |
| Junie | [whats-new](https://junie.jetbrains.com/whats-new) + [GH Releases](https://github.com/JetBrains/junie/releases) | partial ([junie-guidelines](https://github.com/JetBrains/junie-guidelines)) | unconfirmed | near-weekly |
| Gemini CLI | [docs/changelogs](https://github.com/google-gemini/gemini-cli/blob/main/docs/changelogs/index.md), 3 channels | public, [configuration.md](https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md) | [settings.schema.json](https://raw.githubusercontent.com/google-gemini/gemini-cli/main/schemas/settings.schema.json) | weekly preview |
| Zed | [GH Releases](https://github.com/zed-industries/zed/releases) | in-repo `docs/` | runtime-generated only | ~daily preview |
| Amp | [ampcode.com/chronicle](https://ampcode.com/chronicle) page only | closed | unconfirmed | unconfirmed |
| Antigravity | [antigravity.google/changelog](https://antigravity.google/changelog/) page only | closed | unconfirmed | frequent |
| Cline | CHANGELOG.md + [GH Releases](https://github.com/cline/cline/releases) | in-repo | unconfirmed | ~weekly |
| Factory Droid | [release notes](https://docs.factory.ai/changelog/release-notes) page | unconfirmed | unconfirmed | frequent |
| Goose | [GH Releases](https://github.com/block/goose/releases), [RELEASE.md](https://github.com/block/goose/blob/main/RELEASE.md) | monorepo | unconfirmed | ~weekly |
| Warp | [docs.warp.dev/changelog](https://docs.warp.dev/changelog/2026/) page only | closed | unconfirmed | weekly |
| OpenClaw | [docs.openclaw.ai/releases](https://docs.openclaw.ai/releases/2026.8.1/maintenance-changes-part-1) per-topic pages | unconfirmed | unconfirmed | frequent |
| Kilo Code | CHANGELOG.md + [GH Releases](https://github.com/Kilo-Org/kilocode/releases) | [Kilo-Org/docs](https://github.com/Kilo-Org/docs), migrating | `kilo.jsonc` schema ref, hosted file unconfirmed | frequent |
| Qoder | [qoder.com/changelog](https://qoder.com/changelog) page only | closed | unconfirmed | very frequent |

## Negative

No machine-readable feed for Cursor, Kiro, Amp, Antigravity, Warp or Qoder.
Published schemas (Claude Code via SchemaStore, OpenCode) are reported stale,
so a schema diff alone is not authoritative.

## Leads

- Confirm OpenCode's canonical repository.
- Whether Factory, OpenClaw and Kilo docs sites have public Markdown sources.
- Per-repo GH Releases tag conventions (some mix CLI/desktop/plugin streams).
