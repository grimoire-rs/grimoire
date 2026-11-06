# Research: Config-Writer History (repo archaeology)

## Metadata

**Date:** 2026-11-03
**Domain:** packaging
**Triggered by:** `.agents/discussions/config-artifact-type.md`
**Expires:** 2027-05-03

## Direct Answer

grim has written into harness-owned files since 2026-07-02 (MCP) and 2026-10-04 (hooks). Every failure mode below was found after first landing, not before. No `Revert`-prefixed commit touches config writes (`git log --all -i --grep='^revert'` returns only a8ed6e4, a TUI fix). Withdrawals happened inside ADR text, before release. Commit `24a14bb` is not in any local ref (`git cat-file -t` fails); it is cited only by `.agents/adr/adr_hook_workspace_consent.md:290`.

## 1. Fix commits for shared-file merging, by failure mode

Counts are of the commits listed. One commit can count in two groups.

| Failure mode | n | Commits |
|---|---|---|
| Format or comment loss | 4 | c63bac6 opencode `instructions` reserialised the file (comments dropped, keys alphabetised); 2369e1e Codex TOML bare `[mcp_servers]` header and its comment deleted; 0c31420 `grimoire.toml` rewrite dropped comments (grim's own file); 03e59b0 competing `opencode.json` written beside an existing `.jsonc` (also 06b0163 for Amp) |
| Symlinked dotfiles replaced by a regular file | 2 | 7df05e4 (all vendor config splices; Claude `settings.json`, `opencode.json`, `~/.claude.json`, `config.toml`); 34388d5 (lock guard refused symlinked config) |
| Reversal or uninstall failure | 5 | afd6cfe stale MCP entry left live after pin-change decline; 41fac57 uninstall deleted a user-modified or merely adopted entry; f0c565b adopted entry unreachable once the record dropped; f053de1 entry stranded at a relocated vendor root; 31625da partial splice across clients left an unrecorded entry |
| Wrong-file or path identity | 3 | 8e913ac decline splice edited a file recomputed from the current env (repointed `OPENCODE_CONFIG`); 06b0163 Zed `%APPDATA%` path; 8e913ac reaper followed a support-dir symlink alias |
| Injection into the spliced file | 1 | 2043a62 member keys not escaped in JSON splices |
| Clobbering user content | 1 | e86b911 untracked-destination gate (a feature, added 7 days after MCP landed) |
| Drift or status | 2 | 569dc68 introduced semantic entry drift; 3a6bb06 `outputs_pending` descriptor lookup |
| Unrepresentable value | 1 | e64cc4d case-insensitive duplicate headers silently overwrote one Codex slot |

Concurrent edits: `.agents/security_audit_hooks.md:432` (W1) flags that the dispatch write has no lock and a concurrent install silently un-arms another root. The native-dispatcher ADR later keeps an advisory lock (`adr_hook_native_dispatcher.md:299`, `dispatch-lock-held`). I found no bug commit for a user-vs-grim concurrent edit.

Since 2026-07-03, 16 commits touched the five splice and registration files; 8 are `fix`.

## 2. Withdrawn or reversed approaches

- **Registry-scoped hook trust (A2) replaced by workspace consent.** Tried: a `trust_hooks` field on `[[registries]]` plus a prompt. Dropped: a project config file was an input to an arming decision, and five amendments existed to patch that (`adr_hook_workspace_consent.md:36-45`). Replaced by `$GRIM_HOME/hooks/consent/<workspace-key>.json` and the `--trust-hooks` flag pair (`:276-290`). Not a release withdrawal, since hooks were unreleased (`:60`).
- **Committed registration (project scope for Codex and Copilot).** The owner asked for it at the planning gate. Withdrawn by amendment A1 (`adr_hooks_support.md:343-352`): the launcher path became environment-derived (`.envrc`, `.mise.toml`, devcontainer). Replaced by Claude-only project scope through `.claude/settings.local.json`. Codex and Copilot ship global-only. The restriction became I1 (`arch-threat-model.md:67`).
- **Per-hook digest approval store (Decision E).** Withdrawn by A2/A3 (`adr_hooks_support.md:611-617`); C-024 withdrawn outright.
- **`GRIM_ALLOW_HOOKS` and C-026.** The two security audits raised it as B6, "specified two contradictory ways" (`security_audit_hooks_formats.md:319`; W6 at `:492` is the repo-carried `GRIM_EXPERIMENTAL_HOOKS`). It was removed 2026-08-17 (`24a14bb`, per `adr_hook_workspace_consent.md:290`). `AGENTS.md` records it as "must not be re-added".
- **sh launcher shim replaced by a native `grim-hook`** (ef16d1a). The same ADR withdrew O6 (version-stamp and backward-compat machinery), the H3 option, and the directory-reap migration (`adr_hook_native_dispatcher.md:79,157,451`), because hooks are experimental.
- **Payload under the workspace.** Hook payloads were first materialised at `<workspace>/.grimoire/hooks/` (`arch-threat-model.md:69-77`). Execution test SEC-1 showed a repo carrying its own `state.json` and payload could arm offline. They moved to `$GRIM_HOME` (dated 2026-08-18).

## 3. Security findings and the invariants that resulted

- Hooks audit: 8 Blocks (B1-B8) and 9 Warns (W1-W9) in `.agents/security_audit_hooks_formats.md`. The attacks: B1, an environment-derived dispatch table path; B2, the launcher path expanded in a shell string; B3, a guessable root key so a repo selects another root's hooks; B4/B5/B7, trust scope and identity; B8, an unrecognised guard state fails closed in Copilot.
- Invariants: I1 nothing armable in a repository; I2 identity is content, with a recorded hook-consent exception; I3 grim never converts its own failure into a verdict; I4 default-deny for anything that executes; I5 tamper-evidence, not tamper-resistance; I6 secrets never in argv or env (`arch-threat-model.md:67-136`).
- Fix 456858f: a gatekeeper hook that crashed or timed out was read as "no opinion", so the tool call proceeded. It is now withheld (closes #250, #248).
- MCP-side hardening: 8e913ac (CWE-59 and wrong-file splice), 7df05e4, 2043a62, e86b911.

## 4. Scope decisions

- `adr_artifact_trust_model.md:110-118` (out of scope): "Anything the harness owns: which tools an agent may run, sandboxing, approval prompts at execution time. grim installs config; it is not the execution policy layer." The same ADR says "kind X is executable, therefore gate it" is not a finding. Hooks are the recorded exception.
- `adr_vendor_support_tiers.md:99-110`: Class 1, repair of grim's own output (always in scope); Class 2, a static compensating render (Tier 1 clients only); Class 3, runtime code (never). Worked example: registering `claudeMdExcludes` is Class 1 (691395b, issue #102). That commit is the one place grim edits a harness behaviour key in `settings.json`.
- Counter-evidence: agent and skill frontmatter already carries `permissionMode`, `allowed-tools` and `disallowed-tools` as tool-namespaced vendor metadata (`docs/src/content/docs/vendor-metadata.md:196,234`). So permission-adjacent fields are passed through, not forbidden.
- `adr_hooks_support.md:838-845`: a client with no declarative hook file (OpenCode) is out of scope because its surface is a plugin API (the `CodegenModule` shape).

## 5. Effort signal

- **MCP (2026-07-02).** Four landing commits: 68c564b (28 files, +807), 6f16d0b (the splice editor, +711), 569dc68 (13 files), a533436 (10 files, +821). Since then, 16 commits touched the splice and registration files, 8 of them fixes. Rework covered format fidelity, uninstall safety, symlinks and path identity.
- **Hooks.** Landing commit 82b6c9f: 115 files, +35,812 lines (src 28.6k, test 7.1k, plus ADRs and plans). The first branch version is 12e7ed2 (2026-08-29). Adjacent commits: 5187f20 (43 files, a hook-free golden, for Principle 9), 844c4ea and dbe0424 (export), 0867e43. The launcher rewrite ef16d1a is 94 files (+12,037, -5,333) three weeks later. 456858f is a fix after that. The largest rework was the arming and trust model (A1, A2, A3, the consent ADR), followed by the launcher.

## Negative and contradicting evidence

- No `Revert` commits exist for config writes. "Withdrawn" is ADR-level, and mostly pre-release.
- `24a14bb` cannot be inspected locally. The reason for removing `GRIM_ALLOW_HOOKS` is taken from the audit B6 and the consent ADR, not from the commit.
- The MCP and hook kinds were never run through a rollback that grim itself performed. Reversal bugs came from users or relocations, not from a withdrawn renderer.
- 2043a62 has no commit body; the attack is inferred only from its subject.
