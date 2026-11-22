# Research: Security of the `mod` artifact kind

## Metadata

**Date:** 2026-11-18
**Domain:** security
**Triggered by:** `/hex-architect` run for the experimental `mod` kind (`.agents/discussions/harness-mods-artifact-kind.md`)
**Expires:** 2027-02-18 (Claude mods and OpenCode V2 plugins both change between releases)

Scope: the owner decisions are taken. This file gives the security facts, the controls that make them safe, and the residual risk each decision carries. Findings are judged against `.claude/rules/arch-threat-model.md` (T1 to T5, N1 to N5, I1 to I6). Existing mod research is not repeated; where it is relied on, it is named.

## Direct Answer

1. A Claude mod is not strictly more powerful than a grim hook at the OS level (both run arbitrary code as the user) but it is strictly more powerful inside the client, and grim cannot disarm it at run time.
2. The I1 exception changes little against T3 for what an attacker can do (a committed `.opencode/plugins/x.ts` loads with no grim at all) and a lot for what grim can promise: the lock pin and consent stop describing what runs once the payload sits in the repository. For Claude the exception can be narrowed at no cost (registration in the untracked local settings file).
3. Dropping the kind from the key widens consent in three ways, one of them a regression against a protection the code states in a comment (a second artifact from a consented repository). The wording that keeps "a new source is a new decision" true is "a source is a repository; consent covers everything that repository publishes".
4. The generated wrapper is safe only if every interpolated value is serialized by a JSON writer, never formatted into a string, and if the consumer-side install re-validates what the publisher-side build checked.
5. Every comparable system gates auto-loaded project code by a decision stored outside the project or tied to content; the repeated failure mode is the approval living inside the repository it governs.

## 1. Capability surface

**Claude mod.** Sources: [admin](https://code.claude.com/docs/en/plugins/mods/admin.md), [plugin security](https://code.claude.com/docs/en/plugins/security.md), both fetched 2026-11-18. Quotes: a mod is "a plugin that runs code inside Claude Code with the permissions of the user who installed it. Mods aren't sandboxed." A user's mod "can still read and write files, start processes, and make network requests. It can also rewrite tool calls and the user's prompts, deny a tool call, add or remove tools, and draw in the interface." The `calls:` table lists `$.fs.read/write` ("anywhere the user can"), `$.process.run/spawn`, `$.http.fetch`, `$.env.get/set`, `$.settings.read` ("can hold API keys"), `$.mcp.call` ("with no permission prompt"), `$.model.complete` ("Uses the user's plan or API key"), `$.prompt.submit` ("as the user's own words"), `$.session.send` (messages another session's Claude reads). `tool.call` and `prompt.submit` hooks "mean the mod sees every tool call and every prompt, and can change them"; `session.append` rewrites "each row of the conversation before it's stored"; `tool.check` approves or denies "before a permission prompt appears"; `ui.render{component=AskUserQuestion}` redraws the dialog Claude uses to ask the user. `$.fs`, `$.process`, `$.tool.call` and `$.mcp.call` "can skip permission rules and PreToolUse hooks". Another mod can be hooked by name through `plugin.register`, so a mod can see and change what other mods load. `$.env.set` changes "every command and MCP server it starts afterward". Where the built-in guard loads (managed settings, Team or Enterprise login) it blocks a user's mod from rewriting the system prompt, managed instructions and managed MCP tool lists, and a `deny` rule survives a mod's approval; on a personal machine with none of those, the guard does not load and a user's mod "can skip more checks".

**OpenCode V2 plugin.** Sources: [plugins](https://opencode.ai/docs/plugins/), [server part](https://opencode.ai/docs/build/plugins/server), fetched 2026-11-18, plus `research_harness_mods_opencode_plugin_facts.md`. "The server part of a plugin runs in the OpenCode server." Hooks: `tool.execute.before` can "inspect or replace input", `execute.after` changes results, `shell.create.before` changes command, cwd, timeout and env of a shell, the permission `evaluate` hook sets `allow`, `ask` or `deny` ("An explicit configured `deny` is final"), prompt and model hooks rewrite prompts, messages, tools, HTTP requests and WebSocket frames, and `experimental.model.resolve` swaps the model. The docs do not say a plugin has full Node or Bun access; a plugin is JS/TS in the server process and OpenCode's `SECURITY.md` says it "does **not** sandbox the agent" (V1 file, in the existing research). Treat reach as equal to Claude's until shown otherwise; this was not verified.

**Against a grim hook.** A hook is a spawned command at the user's privilege through the `grim-hook` launcher, with a tier (observer or gatekeeper) that is a declaration, not a sandbox. At the OS level a mod adds nothing: both read files, start processes, reach the network. A command hook already returns allow, deny or rewritten input on a tool event. What a mod adds, and a hook lacks:

- **Client-level reach.** It reads every prompt and transcript row in memory, holds the client's credentials and settings, calls tools and models as the user without a permission prompt, and redraws the permission and question dialogs (a spoofing surface for T4-shaped abuse).
- **No grim in the load path.** A hook runs only if the dispatcher finds consent, so drift disarms it at the next event (I3 keeps this fail-safe). A mod loads by the client's own rules; revoking consent or a drifted record cannot stop an already registered mod. Only a later grim command that removes the registration can. The ADR must say that revoke, drift and uninstall remove the registration and the files, and that between commands grim does not control the mod.
- **A different org switch.** Claude's `allowManagedModsOnly` is separate from hook policy, and may silently skip a mod (no observable status for a skills-dir load, see `research_harness_mods_claude_wrapper_facts.md`). The ADR's "documented gap" is right; `grim status` must not claim "armed" for a mod, only "written".

Verdict: **different, and strictly wider in the client; equal at the OS.** Attacker mapping: T1 (publisher) is the one that gains; the capability class (arbitrary code as the user) is the one the hook ADR already accepted.

## 2. The I1 exception

**What I1 says and why.** "Nothing armable lives inside a repository... **and the payload that is executed**." The clause was added because WP-R showed "a repository carrying its **own committed** `state.json` *and* payload armed on a fresh machine **offline**". It also says: "**A gitignore rule is not part of this control** — it governs `git add` / `git status`, not reads, so an attacker's committed file is read whatever any ignore rule says." The one recorded repo-resident exception is `.claude/settings.local.json`, tolerated "because the client treats it as per-developer local" (`src/install/hook_registrar.rs`). That quote also fixes how to judge mitigations: ignore entries are hygiene for the user, not a control against T3.

**What the exception changes, per attacker.**

- **Cloner of a repo with committed mod files (T3).** The attacker can already commit a mod or an OpenCode plugin file with no grim involved, so grim adds no new capability. What each client does alone:
  - *Claude*: needs a registration. A committed `.claude/settings.json` can register a directory marketplace and enable the plugin; the docs say repository `extraKnownMarketplaces` are honored "only after you accept the workspace trust dialog", and "in a folder you haven't trusted, including a `-p` run there, it ignores them without a message" ([loading](https://code.claude.com/docs/en/plugins/loading.md), via the existing research). Interactive mods also "load only after the user answers the trust prompt" ([admin](https://code.claude.com/docs/en/plugins/mods/admin.md)). So the gate is one folder-level dialog, and there is no per-mod prompt. Project plugin trust "isn't enough" via a parent folder or `-p` (loading).
  - *OpenCode*: a file in `.opencode/plugins/` "loads on start" with no prompt; no trust or allowlist was found for V1, V2 or Kilo (existing research). The exposure is total, and grim does not change it.
  What the exception changes for grim's promise: consent is a no-op for this attacker. The attacker's own committed files load whether or not grim consent exists on that machine. The decisions record this as "a committed mod loads for a cloner under the client's own rules"; it is accurate.
- **A pull that changes a committed mod (N1 for a teammate, T3 for a fork).** The new bytes load at the next session start (Claude loads a directory plugin "current source files at every session start"). Grim is not run, so the lock pin is not checked and I2's "verified immediately before use" cannot hold for mods. The hook design already took a comparable trade (A3 dropped the exec-time recheck, recorded as I5), but there the payload lived under `$GRIM_HOME` where a pull cannot change it. For a mod the evidence is `git diff` on a normal-looking source tree, which reviewers will read as application code, not as lock output. **New finding:** a pin that is not enforced at load time reads as a guarantee; the docs and `grim status` must say "the lock pins what grim writes, not what the client loads".
- **A fork PR checkout into an already trusted folder (T3).** The Claude trust dialog was answered for the folder, not for the contributor's change, so a new committed mod or `.claude/settings.json` registration loads after the checkout. This is the same posture Claude's own project hooks and project `.mcp.json` have, so it is Claude's residual, not a new grim class. OpenCode: loads with no dialog at all.
- **T1** is untouched by location (the lock pin still governs what grim writes). **T4** gains a path: an injected agent that writes `.opencode/plugins/x.ts` arms code at the next start, but it could do that without grim.

**Mitigations that keep the decision, with evidence.**

| Mitigation | For | Against | Verdict |
|---|---|---|---|
| Register Claude mods in `.claude/settings.local.json`, not `settings.json` | The file is the recorded I1-tolerated local surface; Claude's docs list `local` as a scope for `enabledPlugins` and `extraKnownMarketplaces`; a cloner of committed mod files gets no registration from grim, so the files are inert unless the repo also commits a registration (which is Claude's own gate) | Per-developer: a team cannot share a mod by committing it (the loudest community pain is team rollout, see `research_harness_mods_community.md`); `local`-scope load after trust is unverified | **Adopt.** Keeps the Claude half within I1's spirit. |
| Claude marketplace directory under `$GRIM_HOME` with an absolute path in `settings.local.json` | Keeps the payload out of the repo, so Claude mods need no I1 exception; the directory marketplace loads in place from an absolute path (verified at user scope) | Contradicts the owner's project-folder decision; project-scope load of this form is unverified | **Offer as the fallback if the local-settings route fails.** The ADR should say Claude does not need the exception. |
| Write `.git/info/exclude` entries for mod outputs | Reuse: grim already does this for `settings.local.json` and handles linked worktrees (`hook_registrar.rs`); avoids a diff for teammates | It is hygiene, not a control (the I1 quote); no effect on an already tracked file; `git add -f` overrides it | **Adopt as default hygiene**, never described as protection. |
| `grim status` warns when a mod output is tracked by git (`git ls-files` or `git check-ignore` on the output path) | Turns "silently committed" into evidence (I5); one status line | Informational; the attacker's own files are not grim outputs and are not seen | **Adopt.** Also warn on any file in the harness plugin directory that grim does not own, for OpenCode. |
| Refuse to arm when the output path is tracked | Stops a teammate's pull from turning a grim output into repo content | Breaks the legitimate team-commit case the exclude entry does not prevent; a refusal on a tracked file also strands a user who committed by mistake; fails open for the attacker who commits any other file | **Do not adopt as a refusal.** Warn only. |
| Document it | Costs nothing, required for I5 honesty | Not a control | **Adopt**, with the sentence above about pins. |

**Residual to state plainly.** OpenCode project-scope is a recorded I1 exception with no client gate. Every cloned repo that carries `.opencode/plugins/` runs code on `opencode` start; a grim consent record on the cloner's machine adds nothing, and a grim warning appears only if grim is run. The ADR should offer the owner the option that a project-scope OpenCode mod needs the user's own gesture each time (documented) but the owner's decision stands; this report only states the residual.

## 3. Consent by source list

Current key (`src/hook/consent.rs`): `binding@registry/repository`, settings prefixed `settings:`, no tag, no digest. The code comment gives the reason for the binding: "a hostile upstream adding a *second* hook from a repository the user already consented to would otherwise arm with no re-prompt."

Proposed: workspace plus a plain list of `registry/repository`, no kind, no binding.

**Can one repository carry several kinds? Yes.** The kind is read from the manifest of the pinned tag (`src/oci/artifact_kind.rs`), so one repository can hold a hook at one tag and a mod at another. A bundle can name members in its own repository.

What an attacker gains from dropping the kind and the binding:

1. **Cross-kind widening (T1).** A source consented for a hook later publishes a mod or settings tag; a declaration that names it arms with no prompt. Under the current key a new kind is a new key. The gain is one capability class change (hook shell command to in-process code, see section 1), small in power and large in visibility. The attacker needs the user to declare the new tag, or a bundle in that repository to gain a member.
2. **Bundle expansion (T1).** A bundle whose repository is consented gains a member from the same repository: today a new resolved key, so drift and a re-ask; after the change, silent. Members from other repositories still ask. This is the only widening reachable with no user gesture beyond the existing add.
3. **Transitive `requires` (T1).** A dependency edge into another repository asks (new source). An edge into the same repository is silent.
4. **A rebinding or a second binding (T3 fork PR, N1).** Today a changed or additional binding of a consented source is drift. After the change a pull that edits `grimoire.toml` to add `[mods] x = "<consented source>:<tag>"` arms with no prompt. For a fork PR checked out into a consented workspace this is the one case that reaches T3 (the attacker can only reference artifacts the consented publisher already published, so the gain is bounded by that repository's content). For a teammate it is N1.
5. **No gain** from the key dropping the prefix other than the above; the T1 residual "new version of a consented source, no re-prompt" already exists and is unchanged (a mod runs the same way as a hook there).

Also: the declared set must be computed from gated kinds only. If skill and rule sources entered it, `grim install` of an ungated skill would read as drift. A source consented through a hook also covers a skill from that repository, which is harmless (skills are ungated).

**Which wording keeps "a new source is a new decision"?** State it as: *a source is a repository; consent covers everything that repository publishes, under any kind, binding or tag; a repository not in the list is a new decision.* That keeps the promise true because it changes what a "source" means rather than pretending the promise is unchanged. Three things make it honest:

- The prompt names the effect: "mods, hooks and settings from `<registry>/<repository>` will run on this machine, including kinds that repository publishes later".
- The change list of every changing run (the settings kind already has one, I5) names each artifact newly armed from an already consented source, with its kind. This is the only way a user sees item 1, 2 or 4.
- `grim hook allow` and the add gesture list the consent by repository, so revoking a source is one line.

The alternative, keeping the kind or binding in the key, is cheaper to reason about and costs a re-ask when a publisher adds a second artifact; the owner has chosen against it. The residual is items 1, 2 and 4 above, all bounded by T1's existing residual plus visibility, not prevention.

Migration: old records fail closed (unknown shape reads as absent, I3). Records must still carry `deny_unknown_fields` and no field defaults so a truncated file never reads as granted (an empty source list is a subset of an empty list, as the existing doc warns).

## 4. Generated wrapper and export

Generated files: `plugin.json` (`name`), `hooks/hooks.json` (`modules` with one relative path), the OpenCode `index` file (an import specifier). Existing validation covers the *consumer* binding and the *publisher's* manifest separately: `RESERVED_ARTIFACT_NAMES`, `refuse_bad_binding_name` for the binding, and `SkillName` (`[a-z0-9]+([.-][a-z0-9]+)*`, at most 64) for names; `path_segment_refusal` and `contain` in `src/path_safety.rs` for declaration keys and containment. The `src/oci/hook.rs` comment is explicit that manifest-name validation runs "on the publisher's machine".

| Risk | Closed by |
|---|---|
| JSON injection through `name` or entry (quotes, backslash, newline, control characters) | Build the files with `serde_json` (never string formatting). Restrict the name to the `SkillName` grammar and run that grammar **before** any key or path is minted (the existing `settings:` prefix case shows the order matters). |
| Entry path with `..`, absolute path, drive prefix, NUL, empty or dot-only part, backslash | `path_segment_refusal` on the entry, then `contain(<harness dir>, entry)`. Re-run **at install**, not only at `grim build`: a hostile publisher (T1) hand-crafts the manifest, and build-time checks bind only honest tooling. |
| Symlink or hardlink in the layer pointing out of the harness directory | Refuse non-regular entries in the mod layer and canonicalize (`contain` Layer 2). Claude 2.1.325 also "skips some unsafe paths and links" itself, a second layer, not a replacement. |
| Character significant in an import specifier (quote, backtick, `${`, `?`, `#`, newline) in the OpenCode `index` | Allow only `[A-Za-z0-9._-]` and `/` in the entry, with no leading dot; emit the specifier through JSON string encoding. The index re-exports one fixed relative file; it never concatenates a name. |
| Payload file shadowing a generated file (`hooks.json`, `plugin.json`, `index`, `server`) | Reserve those names inside each harness directory and refuse on collision, including case-insensitive filesystems (macOS, Windows) and Windows trailing dot or space and alternate data stream forms. |
| Entry resolves to a file that does not exist or to several | `grim build` fails; install also fails closed, because an artifact built by another tool can omit it. |
| Binding differs from the published name, so an installed path, a plugin name and a consent key disagree | Use the consumer binding for installed paths and the key (as hooks do); derive the generated plugin name from the binding, not the manifest. |
| Two bindings produce the same Claude plugin name or OpenCode plugin id | Claude registers only one hooks module per plugin name, and OpenCode V2 stops activation on a duplicate id (existing research); detect the Claude name collision in grim, state the OpenCode one as unchecked. |

Generated files are rendered output: re-materialization must leave `status` not-modified, and the content hash must cover them so a tampered `hooks.json` is detected.

## 5. Prior art for the controls

**direnv `allow`.** `.envrc` is blocked until `direnv allow`; the record is `$XDG_DATA_HOME/direnv/allow` ([man](https://direnv.net/man/direnv.1.html)). Source ([rc.go](https://raw.githubusercontent.com/direnv/direnv/master/internal/cmd/rc.go), fetched 2026-11-18): the allow file name is a SHA-256 over the absolute path and the file contents, so an edit re-gates and a copy to another directory does not inherit. A deny is keyed on the path alone. `whitelist.prefix` allows a whole directory tree regardless of content. Failure modes: content-only trust executed a copied file (`direnv/direnv#83`, the origin of this repo's directory binding); a prefix whitelist turns a directory into a standing grant; habituation after many `allow` runs. Lesson for grim: key on the workspace path, keep the record outside the repo, and note that grim's source-list consent is closer to a prefix whitelist than to direnv's content hash.

**VS Code Workspace Trust.** New folders open in Restricted Mode, which disables or limits "AI agents, terminal, tasks, debugging, workspace settings, and extensions"; trusting a parent folder trusts all subfolders ([docs](https://code.visualstudio.com/docs/editing/workspaces/workspace-trust), fetched 2026-11-18). Extensions opt in by declaring support. Its own limit: "Workspace Trust can't prevent a malicious extension from executing code and ignoring Restricted Mode." Failure modes: parent-folder trust inherited by every clone beneath it (the reason Claude says a parent folder "isn't enough" for project plugins), empty windows trusted by default, extensions that declare full support and then ignore the mode. Lesson: a folder-level answer is one decision for everything beneath, which is what grim's workspace consent also is.

**npm `ignore-scripts` and script approval.** `ignore-scripts` stops "scripts specified in package.json files" from running, but explicit `npm run`, `start`, `test` still run ([config](https://docs.npmjs.com/cli/v11/using-npm/config#ignore-scripts)); the newer `allow-scripts` setting names which non-registry dependencies may run install scripts (same page). pnpm 10 blocks dependency lifecycle scripts by default and `pnpm approve-builds` records approvals in `pnpm-workspace.yaml` as `allowBuilds` ([docs](https://pnpm.io/cli/approve-builds), fetched 2026-11-18; the page does not state version keying and examples use names only). Failure modes: approval is a repo file, so a repository ships its own allow list (the same defect as a committed consent record); a package name is a name, not content (T2); a global off switch is disabled in the first CI config that needs a build. Default-off is the one control with a causal record (I4).

**Microsoft APM executable approval.** From the existing research (`research_harness_mods_vendor_distribution.md`, fetched 2026-11-18): APM "blocks unapproved executable primitives from dependency packages: hooks, `bin/` executables, self-defined MCP servers, LSP servers, and canvas extensions", the gate is "opt-in for backward compatibility; a project `executables:` block or org policy enables it", text primitives are never gated, and approval is `apm approve <pkg>`, keyed `name#version@sha256:<digest>` for bundles. My own search found no APM documentation for the command, so this rests on that earlier fetch. Failure modes: opt-in means most projects run ungated; a `project executables:` block is a repo file; version and digest keying re-prompts every bump. Lesson: grim's default-on gate for the experimental flag is stronger; the digest key is the trade grim declined on purpose.

**Claude Code's own project plugin trust.** Project-scope plugins load "only after the same trust check that applies to project allow rules... Trusting a parent folder or running with `-p` isn't enough"; repository `extraKnownMarketplaces` are ignored in an untrusted folder; interactive mods wait for the trust dialog; "Neither the trust check nor these component limits apply to a plugin under your own `~/.claude/skills/`" (loading, via the existing research). There is no per-mod prompt, and a mod is not sandboxed. Failure modes: one folder answer covers every later change to the repository (pull or fork checkout), the answer is per folder not per plugin, and the user-scope path skips the check entirely. This is the gate a cloner meets, and grim should not claim to add to it.

## Trends

- **Trending:** default-off for install-time code (pnpm 10), per-folder trust (VS Code, Claude), org guards (`allowManagedModsOnly`).
- **Established:** path-plus-content approval (direnv), an allow record outside the project.
- **Declining:** global "trust the registry" switches and approvals stored in the repository they govern.

## Controls the ADR should state

Ranked. Each is tied to an attacker or invariant.

1. **State the I1 exception as two, not one, and narrow it.** Claude mods: registration in `.claude/settings.local.json` only (the recorded local surface), never `settings.json`; payload files in the project folder are inert without it. OpenCode project scope: recorded as a full exception with no client gate. Name T3 for both, and say the consent record does not stop a cloner of committed files. (T3, I1)
2. **Say "the pin governs what grim writes, not what the client loads"** in the ADR, the docs and `grim status`; do not report a mod as armed, report it as written. State that I2's use-time check does not exist for mods and that this is the hook A3 trade extended, with a weaker evidence surface (source tree, not lock). (T1, T2, I2, I5)
3. **Re-validate every generated-file input at install**: the binding and name by the `SkillName` grammar before any key is minted, the entry by `path_segment_refusal` plus `contain`, regular files only, reserved names, JSON written through `serde_json`, import specifier charset allowlist. Build-time checks are an author convenience only. (T1)
4. **Consent wording and visibility.** The list is of repositories; the prompt and docs say "covers every kind and every later tag from this repository"; every changing run lists artifacts newly armed from an already consented source with their kind; the declared set is computed from gated kinds only. State the three widenings (section 3 items 1, 2, 4) as residual under T1 and as visibility, not prevention. (T1, T3 fork PR, I5)
5. **Disarm semantics.** Revoke, drift and uninstall remove the Claude registration and the written files, since no dispatcher can stop a loaded mod; state that between grim commands the client controls the mod. (I3, I5)
6. **`.git/info/exclude` entries and a tracked-output warning** as hygiene and evidence, reusing the settings-local precedent; no refusal, and no claim that they hold T3. (I5)
7. **Experimental flag default off, no environment form for consent**, as in the hook ADR (I4, CWE-426), and no per-mod consent mechanism. Keep `--trust-workspace` pair unchanged.
8. **Unchecked gaps stated once:** OpenCode duplicate plugin id; managed Claude settings that silently skip a mod; OpenCode V2 runtime reach not verified beyond docs.

## Sources

- https://code.claude.com/docs/en/plugins/mods/admin.md : capability list, guard, trust dialog, validate output (fetched 2026-11-18)
- https://code.claude.com/docs/en/plugins/security.md : what a plugin and a mod can do, review steps (fetched 2026-11-18)
- https://code.claude.com/docs/en/plugins/mods/reference.md : fetched, saved locally, not read in full; the admin page carries the call table used here
- https://opencode.ai/docs/plugins/ and https://opencode.ai/docs/build/plugins/server : load locations, server hooks (fetched 2026-11-18); `/docs/build/plugins` page said nothing on security
- https://code.visualstudio.com/docs/editing/workspaces/workspace-trust : Restricted Mode, parent folder trust, stated limit (fetched 2026-11-18)
- https://direnv.net/man/direnv.1.html and https://raw.githubusercontent.com/direnv/direnv/master/internal/cmd/rc.go : allow record, path-plus-content hash (fetched 2026-11-18)
- https://docs.npmjs.com/cli/v11/using-npm/config#ignore-scripts and https://pnpm.io/cli/approve-builds : script gating (fetched 2026-11-18)
- Local: `.agents/research/research_harness_mods_prior_art.md`, `research_harness_mods_claude_wrapper_facts.md`, `research_harness_mods_opencode_plugin_facts.md`, `research_harness_mods_vendor_distribution.md`, `research_harness_mods_community.md`; `src/hook/consent.rs`, `src/install/hook_registrar.rs`, `src/path_safety.rs`, `src/oci/hook.rs`, `src/skill/skill_name.rs`

## Not verified

- Claude project-scope load of a directory marketplace registered in `settings.local.json` (docs list the scope; no run, no login).
- Whether a Claude directory marketplace with an absolute path under `$GRIM_HOME` loads at project scope (user scope was verified).
- OpenCode V2 plugin reach beyond the docs; no OpenCode execution.
- The APM approval details beyond the earlier fetch; my search did not find the command.
- Whether a tracked output makes `git check-ignore` misreport; a local test is needed.
