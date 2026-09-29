# Research: several harness marketplace files at one repo root

## Metadata
- Date: 2026-09-28
- Lane: two harness files one repo
- Question: can one git repo root carry every harness's marketplace file at once, each pointing at a different per-harness-rendered plugin tree (`claude/<plugin>/`, `agent-plugins/<plugin>/`), such that each harness reads only its own file? Also: marketplace `name` uniqueness, and whether `source` may point outside the manifest dir (`../claude/foo`).
- Extends: `research_marketplace_hosting_prior_art.md`, `research_plugin_support_matrix.md` (not redone).
- Sources (accessed 2026-09-28):
  - [S-codex] github.com/openai/codex @ `33a0f766a647208b471cfbcad889c67fd324ee04` (shallow clone), `codex-rs/core-plugins/src/marketplace.rs`, `marketplace_tests.rs`, `marketplace_add.rs`
  - [P-*] local probes, throwaway dirs under `/tmp/claude-1000/-home-mherwig-dev-grimoire-duo/e1fb891b-ae2f-44d6-90a3-1454608d98a8/scratchpad/probe/` (`HOME`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `COPILOT_HOME` all pointed there; local-path marketplaces only, no network, real configs re-checked unchanged). CLIs: claude 2.1.284, codex-cli 0.153.4, copilot 1.0.88 (`--no-auto-update`). Droid, Junie, Cursor not installed.
  - [W-*] WebFetch summaries: docs.factory.ai/cli/configuration/plugins, junie.jetbrains.com/docs/junie-cli-extensions.html, cursor.com/docs/reference/plugins
  - [R-*] real repos via `gh api repos/<r>/git/trees/HEAD` + contents; [I-*] GitHub issues
- Caveat: Copilot CLI is closed source; its lookup is established by probe (P-copilot) plus string table of `prebuilds/linux-x64/runtime.node` (`plugin/marketplace.json`, `.github/plugin/marketplace.json`, `.claude-plugin/marketplace.json`), not by source.

## Verdict on the question (findings only)

All three installed CLIs were probed against one scratch git repo carrying `.claude-plugin/marketplace.json`, `.github/plugin/marketplace.json` and `.agents/plugins/marketplace.json`, all declaring marketplace `mp` with plugin `foo`, each pointing at a different tree (`./claude/foo` vs `./agent-plugins/foo`).

| Harness | File actually read | Tree installed | Evidence |
|---|---|---|---|
| Claude Code | `.claude-plugin/` only | `claude/foo` (skill `foo-claude` in `plugins/cache/mp/foo/1.0.0/`) | P-claude |
| Copilot CLI | `.github/plugin/` (claude file ignored) | `agent-plugins/foo` (browse shows "agent tree (copilot file)"; installed live from repo path, nothing copied) | P-copilot |
| Codex | `.agents/plugins/` (claude file ignored) | `agent-plugins/foo` (skill `foo-agent` in `plugins/cache/mp/foo/local/`) | P-codex |

Answer: yes for these three. Each harness read exactly one file and never merged; the same marketplace `name` in all files caused no conflict.

## 1. Lookup logic per harness: first hit wins, no merge

**Codex (source).** `marketplace.rs:20-25`:
```rust
const MARKETPLACE_MANIFEST_RELATIVE_PATHS: &[&str] = &[
    ".agents/plugins/marketplace.json",
    ".agents/plugins/api_marketplace.json",
    ".claude-plugin/marketplace.json",
    ".cursor-plugin/marketplace.json",
];
```
`find_marketplace_manifest_path` (`marketplace.rs:324-333`) is `.iter().find_map(...)` on `path.is_file()`: first existing file wins. Test `list_marketplaces_prefers_first_supported_manifest_layout` (`marketplace_tests.rs:1121-1160`) writes `.agents/plugins` (`agents-marketplace`) and `.claude-plugin` (`alternate-marketplace`) and asserts `marketplaces.len() == 1`, name `agents-marketplace`. Note: `.github/plugin/` and `.factory-plugin/` are not in the list; `.cursor-plugin/` is, last. So a repo with `.cursor-plugin/` and `.claude-plugin/` but no `.agents/plugins/` is read by Codex through the Claude file.
- Existence check only (`is_file()`): a present-but-invalid primary is not skipped in favour of the fallback (the chosen path is loaded afterwards, `marketplace.rs:314-319`).

**Copilot CLI (probe).** Error text on a repo with no candidate lists the tried order: `marketplace.json` (repo root), `.plugin/marketplace.json`, `.github/plugin/marketplace.json`, `.claude-plugin/marketplace.json`. Confirmed by adding files one at a time (P-copilot):
- `.github/plugin` + `.claude-plugin` present: `.github/plugin` used.
- root `marketplace.json` + `.plugin/` + `.github/plugin/` + `.claude-plugin/`: root file used ("ROOT marketplace.json").
- `.plugin/` + `.github/plugin/` + `.claude-plugin/` (root file removed): `.plugin/` used.
- only `.claude-plugin` present: fallback used.
- `.github/plugin` present but invalid JSON, valid `.claude-plugin` present: **error** `Invalid marketplace.json: Invalid JSON syntax ...`, no fallback. Same for schema-invalid (`owner: Required, plugins: Required`).
- `.agents/plugins` only: error, `.agents/plugins` not among candidates.
- Drift: GitHub docs (prior art file, G1) name `.github/plugin` then `.claude-plugin`; the CLI additionally reads root `marketplace.json` and `.plugin/marketplace.json` ahead of both.
- Copilot requires `owner` (`owner.name`, `owner.email` strings in the binary's validation messages); Codex file needs none.

**Claude Code (probe).** Reads `.claude-plugin/marketplace.json` only. Repo with `.github/plugin` and `.agents/plugins` but no `.claude-plugin`: `Marketplace file not found at .../.claude-plugin/marketplace.json`. No fallback to other vendors' files.

**Droid ([W-factory], docs summary).** "`.factory-plugin/marketplace.json` (primary), `.claude-plugin/marketplace.json` (fallback only)". Fallback-only wording is stated for Droid here; not probed.

**Junie ([W-junie]).** Accepts `.junie-extension/marketplace.json` or `.claude-plugin/marketplace.json`; precedence when both exist "not specified". Not probed.

**Cursor ([W-cursor]).** "Place it at `.cursor-plugin/marketplace.json` in the repository root"; no fallback mentioned. Not probed.

## 2. `source` paths: relative to marketplace root, `..` rejected everywhere

Relative sources resolve against the repo (marketplace) root, not the manifest's own directory: `./claude/foo` in `.claude-plugin/marketplace.json` and `./agent-plugins/foo` in `.github/plugin/marketplace.json` both worked (P-*). So `../claude/foo` is never needed.

`../` probe (each file rewritten to `../<tree>/foo`, P-*):
- Claude: marketplace add ok, install fails `This plugin's marketplace entry is invalid: source: Invalid input`.
- Copilot: add and browse ok, install fails `Plugin path escapes marketplace directory: ../agent-plugins/foo`.
- Codex: add ok, `plugin foo was not found in marketplace mp`. Source: `resolve_local_plugin_source_path` (`marketplace.rs:651-700`) requires the `./` prefix (bare path allowed only for `.cursor-plugin/marketplace.json`) and rejects any non-`Normal` component: "local plugin source path must stay within the marketplace root"; relative git urls reject `..` too (`marketplace.rs:768`).
- Droid ([W-factory]): "Absolute paths and paths that escape the marketplace directory are rejected." Cursor ([W-cursor]): "no `..`, no absolute paths". Junie: not documented.

Entry shape still differs per file: Codex wants `{"source":"local","path":"./x"}` (string form also accepted in the `.claude-plugin` layout, `marketplace_tests.rs:99`); Cursor may use bare names with `metadata.pluginRoot`; Claude/Copilot accept `"./x"`.

## 3. Marketplace `name` uniqueness

- Scope is per harness, per user config store; files in one repo never meet inside one harness because only one is read. Same `name` in all files: no conflict (P-*).
- Collision only when a second *different source* registers the same name (P-*, two scratch repos both named `mp`):
  - Claude: silently re-points: `Marketplace 'mp' was already added from dir:<A> and now points at dir:<B>. Plugins already installed from it now update from the new source.`
  - Copilot: `Marketplace "mp" already registered` (also on re-adding the identical repo, so not idempotent).
  - Codex: `marketplace 'mp' is already added from a different source; remove it before adding this source` (`marketplace_add.rs`); same source is idempotent (`already_added: true`).
- Codex cache path `~/.codex/plugins/cache/<marketplace>/<plugin>/<version|local>/`; Claude `plugins/cache/<marketplace>/<plugin>/<version>/` (P-*).
- Droid/Junie/Cursor: no uniqueness rule stated in the fetched docs.

## 4. Real-world repos with several files at one root ([R-*], tree listing 2026-09-28)

- `stripe/ai`: `.claude-plugin`, `.codex-plugin`, `.cursor-plugin`, `.grok-plugin` marketplace files, one marketplace `name: stripe`, plugin `stripe` in each, **per-harness rendered trees**: `./providers/claude/plugin/`, `./providers/cursor/plugin/`, `./providers/grok/plugin/`, Codex file `git-subdir` with `path: providers/codex/plugin`. This is the closest match to the proposed layout. Caveat: `.codex-plugin/marketplace.json` is not in Codex's lookup list at HEAD `33a0f766`, so a Codex user adding this repo would read the `.claude-plugin` file (README instructs `codex plugin add stripe@openai-curated` instead).
- `grafana/ai-marketplace`: `.agents/plugins`, `.claude-plugin`, `.cursor-plugin`, `.grok-plugin`, `.kiro-power`; same name `grafana-ai-marketplace`, one shared `./plugins/<n>` tree (Cursor uses bare names, `.grok-plugin` `{"type":"local"}`, Codex file lists only 1 of 3 plugins).
- `github/copilot-plugins`: `.claude-plugin` + `.github/plugin`, identical content, same name.
- `obra/superpowers`: `.agents/plugins`, `.claude-plugin`, `.muse-plugin`. `addyosmani/agent-skills`: `.agents/plugins` + `.claude-plugin`. `ivankuznetsov/agent-plugins`: `.agents/plugins` + `.claude-plugin`.
- Other harness dirs seen in `gh search code --filename marketplace.json` (191 repos): `.grok-plugin`, `.omp-plugin`, `.codex-plugin`, `.kiro-power`, `.muse-plugin`.
- Single-file repos: `Factory-AI/factory-plugins` (`.factory-plugin` only), `JetBrains/junie-extensions` (`.junie-extension` only), `cursor/plugins`, `openai/plugins`, `anthropics/claude-plugins-official`, `github/awesome-copilot`.
- Not found: any repo with `.factory-plugin` or `.junie-extension` alongside another vendor's file; `gh search code` `path:` qualifier returned `[]` for dot-directories, so this is a tree scan of known repos, not exhaustive.

## 5. Issues found

- [openai/codex#19372](https://github.com/openai/codex/issues/19372) (open, v0.124.0, 2026-04-24): Codex scans `.claude-plugin/marketplace.json` files and auto-imports marketplaces into `~/.codex/.tmp/marketplaces/` although not declared in `config.toml`; Claude-only plugins (`${CLAUDE_PLUGIN_ROOT}` in `.mcp.json`) then fail MCP handshake. Matching code: `codex-rs/external-agent-migration/src/source_cla.rs:21-22` (`claude-plugins-official`). Relevance: a Claude-format tree can reach Codex through migration regardless of the `.agents/plugins` file (mechanism not probed).
- [openai/codex#17066](https://github.com/openai/codex/issues/17066) (closed): `./` local path could not reference repo root; source now special-cases `.`/`./`.
- [github/copilot-cli#2200](https://github.com/github/copilot-cli/issues/2200) (open): `pluginRoot` prepended to source path causing doubled directory segment.
- No issue found reporting a conflict between coexisting marketplace files (searched openai/codex, github/copilot-cli, anthropics/claude-code).

negative:
- No vendor doc, source or issue says "other vendors' files are ignored"; the evidence is code (Codex), probe (Claude, Copilot, Codex) and one docs line (Droid).
- Droid, Junie, Cursor behaviour with several files present is not tested and has no public source; Junie precedence unstated.
- Probes used local-directory marketplaces; git-URL adds not probed (lookup reads the same files after clone in Codex source, unverified for Claude/Copilot).
- Copilot lookup order is from a 1.0.88 probe; it disagrees with the docs and may change.

leads:
- Probe Droid/Junie/Cursor if a binary can be installed in a throwaway HOME; Cursor and Junie are the untested cells of the matrix.
- Check whether Codex's `.claude-plugin` migration/mirroring (#19372) fires for a repo-declared marketplace added by URL, and whether `.cursor-plugin` in Codex's list changes the result for a Cursor-only + Claude repo.
- Copilot: whether a root-level `marketplace.json` collision matters for repos that already ship a top-level file of that name (e.g. unrelated data files).
- Read `stripe/ai` `providers/*/plugin/` trees and its build script as a worked example of per-harness rendering from one source.
