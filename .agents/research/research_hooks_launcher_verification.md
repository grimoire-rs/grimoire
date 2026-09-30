# WP-B — live vendor verification of the hook launcher, guard, and matcher dialects

**Date:** 2026-08-16 · **Author:** WP-B worker · **Plan:** `plan_hooks_artifact_kind.md` § WP-B
**Status:** complete for claude + codex + copilot **CLI** surfaces. Windows runtime and Copilot's
cloud agent remain unverifiable here and are marked as such.

Clients under test, exact versions:

| Client | Version | Auth state during the run |
|---|---|---|
| claude | `2.1.233 (Claude Code)` | authenticated (real Anthropic session) |
| codex | `codex-cli 0.147.0` | **not logged in** — driven through a local BYOK provider (see Method) |
| copilot | `GitHub Copilot CLI 1.0.80` | **not authenticated to the Copilot API** (corporate Zscaler proxy returns HTTP 403 for `api.*.githubcopilot.com`) — driven through a local BYOK provider |

Host: Linux 6.18.33.2-microsoft-standard-WSL2, `/bin/sh` → `dash`, user `$SHELL` → `/bin/zsh`.

---

## Method — and why almost nothing is BLOCKED-ON-LOGIN

The brief expected Codex to be blocked behind an interactive browser login. It is not, and neither
is Copilot. **Both CLIs accept a custom, OpenAI-compatible model provider and then require no vendor
authentication at all**:

- Copilot: `copilot help providers` states verbatim *"GitHub authentication is not required when
  using a custom provider."* Activated with `COPILOT_PROVIDER_BASE_URL` + `COPILOT_MODEL`.
- Codex: `[model_providers.<name>]` in `config.toml` with `base_url` + `wire_api = "responses"` +
  `env_key`. (`wire_api = "chat"` is rejected in 0.147.0: *"`wire_api = "chat"` is no longer
  supported"*.)

I therefore wrote two throwaway HTTP servers on loopback — one Chat-Completions
(`fakeoai.py`), one Responses (`fakeresp.py`) — that return a canned assistant message, or a canned
tool call, and log every request. Every "did the hook fire" result below is a **real hook process
spawned by the real client binary**, with the real payload on stdin. No mock of the client, no
network egress, no tokens spent on Codex/Copilot. Claude ran against its real authenticated session
(one-word prompts).

The probe launcher is a shell script at a path **containing a space**:

```
/…/scratchpad/wpb/grim home/hooks/bin/grim-hook
```

It appends `argv`, `pwd` and its stdin to a log, then exits 0 (optionally printing a canned JSON
response). This makes the "absolute path, no expansion, space survives quoting" question answerable
by inspecting `argv[0]`.

### Evidence tiers used in the tables

| Tier | Meaning |
|---|---|
| **PASS / FAIL** | I ran it against the real client binary and observed the outcome. Literal output quoted. |
| **PASS (schema)** | The real client *loaded and accepted* the config containing the field, and no other observable behaviour follows on this OS. Used only for Windows-only fields. |
| **UNVERIFIED** | Not executed. What would settle it is stated on the row. Never inferred from docs alone. |
| **N/A** | Out of scope for v1 by plan decision (e.g. Copilot cloud agent, which gets no grim registration). |

Nothing in this report is a PASS on documentation. Where I read the vendor's shipped binary (Rust
string tables in Copilot's `runtime.node` and Codex's `codex`), that is called out inline as
**shipped-source evidence** and never counted as a PASS by itself.

---

## 1. The headline results

1. **The corrected guard is not a style preference — on Copilot it is the difference between
   "hook absent" and "every tool call denied".** Copilot CLI's `preToolUse` is **fail-closed on a
   non-zero hook exit**. The first draft's `command -v grim && exec "<launcher>" … || exit 0`
   produced exit 127 and Copilot denied the tool call, verbatim:
   `Denied by preToolUse hook from "…/grim.json" (hook errored)`. The corrected
   `L="…"; [ -x "$L" ] || exit 0` form yields exit 0 and the tool call proceeds. Both executed.
2. **Grim's `matcher = "Bash"` is correct on claude and codex, and INERT on copilot unless the
   registration uses PascalCase event names.** Copilot has two matcher dialects selected by the
   *casing of the event key*, and they see different tool names (`bash` vs `Bash`). Under camelCase
   `preToolUse`, `matcher: "Bash"` never fires and a glob `*` is rejected as an invalid regex and the
   hook is **skipped** — the exact silent-guardrail failure C-025 exists to prevent, reproduced.
3. **Open Question 2 is settled and the answer is "both, in different dialects".** `modifiedArgs`
   and `updatedInput` are both real, both shipped, and each works in exactly one dialect.
   **Copilot `mutator` does not need to ship `Declined`.**
4. **The § Launcher security argument is empirically confirmed on all three clients**: the registered
   command string is passed to a shell and **environment variables in it expand from the client's
   inherited environment**. A `${GRIM_HOME:-…}` in any registration is attacker-selectable.
5. **Two load-bearing plan claims are wrong and need correcting** (§ 6): Claude's hook command is a
   **shell string run by `/bin/sh`**, not exec-form argv; and Copilot *does* have an exec-form field
   (`exec` + `args`) — which grim still must not use, because it cannot fail open.
6. **Codex hooks do not run at all until a human trusts them**, and the skip is **silent**. That is
   the per-hook human trust prompt the brief asked about, confirmed by execution.

---

## 2. Per-client verdict tables

### 2.1 claude (Claude Code 2.1.233)

Registration used: `<project>/.claude/settings.local.json` (the shape the plan ships for
claude · project). Global `~/.claude/settings.json` behaves identically for hook dispatch; it was
not re-tested separately because the hook loader is the same and the isolated-config run
(`CLAUDE_CONFIG_DIR`) fired `SessionStart` from a user-level `settings.json` before it hit the login
error.

| # | Question | Verdict | Evidence |
|---|---|---|---|
| 1 | Executes an **absolute** launcher path, no expansion of the path itself | **PASS** | `argv0=[/…/wpb/grim home/hooks/bin/grim-hook]` — the literal absolute path, unchanged. |
| 1b | …but the command **is** a shell string, and env vars in it *do* expand | **FAIL of the plan's claim** | Hook command `exec "$L" CL-ENVPROBE "${GRIMPROBE:-DEFAULT}"` with `GRIMPROBE=PWNED_FROM_CLIENT_ENV` in the client's env → `argv=[CL-ENVPROBE PWNED_FROM_CLIENT_ENV]`. See § 6.1. |
| 2 | Corrected guard yields exit 0 when launcher absent, `grim` present | **PASS** | Old form (127) ran, tool call still succeeded → claude is **fail-open**; corrected form is therefore safe *and* correct here. |
| 3 | Launcher path containing a space survives quoting | **PASS** | Same `argv0` as row 1 — the space is inside the path, the launcher executed. |
| 4 | Windows form | **N/A** | Claude has no `commandWindows`/`powershell` field; one `command` string. |
| 5 | **Matcher dialect (C-025)** | **PASS — start-anchored, case-sensitive regex; `*` and `""` mean match-all** | See § 3. `matcher = "Bash"` **matches** tool `Bash`. |
| 6 | `{{project_dir}}` | **N/A** | Copilot-only question. |
| 7 | Re-prompt on unchanged command / rewritten file | **N/A** | Claude has no hook trust hash. |
| 8 | Mutator field name | **N/A** | Copilot-only question. |
| **S1** | Per-command **human** trust prompt? | **No** — verified | A hand-written `.claude/settings.local.json` hook executed on the next non-interactive run with no prompt of any kind. |
| **S2** | exit 127 fail-open or fail-closed? | **Fail-open** — verified | With the 127 guard as the only `PreToolUse` hook, the Bash tool ran and returned `hook-probe`. |
| **S3** | Per-hook cloud-agent exclusion | **N/A** | No cloud surface in scope. |
| **S4** | Client itself gitignores `settings.local.json` | **UNVERIFIED** | I created the file by hand; no `.gitignore` was written by claude in the probe repo. The plan leans on this for I1. **What would settle it:** let Claude Code itself create `settings.local.json` (accept a permission rule interactively in a fresh git repo) and check whether `.gitignore` gains the entry. Until then treat "gitignored by the client" as an assumption, not a verified control. |

Shell used for hook commands, verified: `SHELL0=/bin/sh ZSH= BASH=` → **`/bin/sh`** (dash on this
host). A hook string must therefore be POSIX-`sh` compatible for claude.

Sample `PreToolUse` stdin (verbatim, trimmed): `{"session_id":…,"transcript_path":…,"cwd":…,
"hook_event_name":"SessionStart","source":"startup"}`.

### 2.2 codex (codex-cli 0.147.0) — **not** blocked on login

| # | Question | Verdict | Evidence |
|---|---|---|---|
| 1 | Executes an **absolute** launcher path, no expansion of the path itself | **PASS** | `argv=[X-SessionStart]` from `/…/grim home/hooks/bin/grim-hook`; the whole matcher matrix ran off that one absolute path. |
| 1b | Command is a shell string; env vars expand from the client's env | **PASS (i.e. the hazard is real)** | `argv=[CX-ENVPROBE PWNED_FROM_CLIENT_ENV]`. |
| 2 | Corrected guard yields exit 0 when launcher absent, `grim` present | **PASS** | `hook: PreToolUse Completed`, tool call proceeded. Old 127 form: `hook: PreToolUse Failed` — and the tool **still ran** (fail-open). |
| 3 | Launcher path containing a space survives quoting | **PASS** | Quoted `"$L"` executed correctly. Note: unquoted `$L` **also** worked here, because Codex runs hooks under `$SHELL -lc` and this host's `$SHELL` is zsh, which does not word-split unquoted expansions. On a bash/dash `$SHELL` it splits (§ 4). **Quoting stays mandatory** — the shell is the user's, not grim's. |
| 4 | `commandWindows` accepted | **PASS (schema)** | A `hooks.json` carrying `"commandWindows": "powershell -c echo win"` alongside `command`, `timeout` and `statusMessage` loaded without warning and the hook fired on Linux. Shipped-source corroboration: the field name is in the binary's serde table next to `HookHandlerConfig::Command with 6 elements`. **Windows runtime behaviour is UNVERIFIED — no Windows host.** |
| 5 | **Matcher dialect (C-025)** | **PASS — `*` means match-all; otherwise a regex over a Claude-style tool name** | See § 3. `matcher = "Bash"` **matches**, because Codex renames its own `exec_command` tool to `Bash` in the hook payload. |
| 6 | `{{project_dir}}` | **N/A** | Codex interpolates nothing into the command string; the only path in the payload is `cwd` (verified in the stdin sample below). |
| 7 | Re-prompt when the command is unchanged but the file is rewritten | **UNVERIFIED** | Trust cannot be granted non-interactively: there is no `codex hooks trust` verb (checked `codex --help`, `codex debug --help`, `codex plugin --help`, `codex features --help`), only the interactive `/hooks` TUI or `--dangerously-bypass-hook-trust`. I attempted the `app-server` `hooks/list` JSON-RPC route; it hung and produced no output. **What would settle it:** a human runs `codex` interactively, approves the hook in `/hooks`, then grim rewrites the same file byte-for-byte and the session is restarted — observe whether the hook still runs. Mechanism from the source-verified prior art (`research_hooks_codex_surface.md`): the trust *hash* is over normalized `(event, matcher, handler)` and is format-independent, while the trust *record key* is positional (`source_path:event:group_index:handler_index`) — so an unchanged rewrite should **not** re-prompt, but inserting a group above grim's silently un-trusts it. Grim should therefore emit exactly one matcher group per event. |
| 8 | Mutator field name | **N/A** | Copilot-only question. |
| **S1** | Per-command **human** trust prompt? | **YES — and the failure mode is silence.** Verified | Identical config, two runs: without the flag **no hook ran at all**, no warning, session looked normal; with `--dangerously-bypass-hook-trust` the same hooks fired. The flag prints `warning: '--dangerously-bypass-hook-trust' is enabled. Enabled hooks may run without review for this invocation.` |
| **S2** | exit 127 fail-open or fail-closed? | **Fail-open** — verified | `hook: PreToolUse Failed` then `/bin/zsh -lc 'echo hook-probe' … succeeded in 0ms: hook-probe`. |
| **S3** | Per-hook cloud-agent exclusion | **N/A** | No Codex cloud surface in scope. |

**Config-source discovery, executed** (four candidate paths, one distinct `touch` sentinel each,
one run):

| Path | Loaded? |
|---|---|
| `$CODEX_HOME/hooks.json` | **yes** |
| inline `[[hooks.SessionStart]]` in `$CODEX_HOME/config.toml` | **yes** |
| `<cwd>/.codex/hooks.json` (project scope) | **yes** |
| `$CODEX_HOME/hooks/hooks.json` | no (that path is the plugin layout) |

So Codex **does** have a working project-scope hook surface. It changes nothing for v1 — the plan
rejects a committed registration on the launcher-path argument, which § 5 confirms empirically — but
it is worth recording that the reason is the *executed path*, not the absence of a surface. Note the
mitigating detail: a cloned repo's `.codex/hooks.json` is discovered but **Untrusted**, hence
silently skipped, so T3 exposure through Codex project hooks needs a human approval first.

**Parse-failure behaviour, executed** (matters for C-017 and for "grim owns the whole file"):

| Input | Result |
|---|---|
| malformed JSON | `warning: failed to parse hooks config <path>: key must be a string at line 1 column 3` — session continues, all hooks from that file dropped |
| unknown **top-level** field | `warning: failed to parse hooks config <path>: unknown field 'bogusField', expected 'description' or 'hooks'` — whole file dropped |
| unknown **handler** field | silently accepted, hook ran |
| unknown **event name** | silently ignored, no warning |

One bad key anywhere in the file disables every hook in it, silently apart from a warning line —
another reason grim must own `hooks.json` exclusively and never splice into a user's.

Sample `PreToolUse` stdin (verbatim, trimmed):
`{"session_id":…,"turn_id":…,"transcript_path":…,"cwd":"/…/proj","hook_event_name":"PreToolUse",
"model":"fake-model","permission_mode":"bypassPermissions","tool_name":"Bash",
"tool_input":{"command":"echo hook-probe"},"tool_use_id":"call_1"}`

### 2.3 copilot (GitHub Copilot CLI 1.0.80)

Registration used: `$COPILOT_HOME/hooks/<file>.json` (the global surface the plan ships), plus one
repo-level `.github/hooks/grim.json` run for question 6 only.

| # | Question | Verdict | Evidence |
|---|---|---|---|
| 1 | Executes an **absolute** launcher path, no expansion of the path itself | **PASS** | `argv0=[/…/wpb/grim home/hooks/bin/grim-hook]`, `argv=[run --client copilot --event SessionStart --root global]`, stdin `{"sessionId":…,"cwd":…,"source":"new","initialPrompt":"Reply with exactly: OK"}`. |
| 1b | Command is a shell string; env vars expand from the client's env | **PASS (hazard real)** | `argv=[ENVPROBE PWNED_FROM_CLIENT_ENV /home/mherwig]` — both `${GRIMPROBE:-…}` and `$HOME` expanded. |
| 2 | Corrected guard yields exit 0 when launcher absent, `grim` present | **PASS — and this row is the gate** | Old form → `Denied by preToolUse hook from "…" (hook errored)` + log line `Error in preToolUse hook … (fail-closed): Error: Hook command failed with code 127 / Stderr: bash: line 1: …: No such file or directory`. Corrected form → tool call ran normally. |
| 3 | Launcher path containing a space survives quoting | **PASS** | Same `argv0` as row 1. |
| 4 | `powershell` field accepted | **PASS (schema)** | An entry carrying both `command` and `powershell` loaded and fired on Linux. **Windows runtime UNVERIFIED.** |
| 5 | **Matcher dialect (C-025)** | **PASS — two dialects, selected by event-key casing; grim MUST use PascalCase** | See § 3. Under camelCase, `matcher = "Bash"` is **inert** and `matcher = "*"` is **rejected and skipped**. |
| 6 | `{{project_dir}}` in a plain non-plugin `.github/hooks/*.json` | **FAIL — no interpolation** | Repo-level hook command `exec "$L" R-repo "{{project_dir}}"` → `argv=[R-repo {{project_dir}}]`, the literal braces. Same result at user level. (The repo-level hook only loaded after the folder was listed in `trustedFolders`.) |
| 7 | Re-prompt on rewritten file | **N/A** | Copilot has no hook trust hash; see S1. |
| 8 | **Mutator field name (Open Question 2)** | **SETTLED — both names are real, one per dialect** | Executed matrix in § 3.3. |
| **S1** | Per-command **human** trust prompt? | **No — verified.** Folder trust only, and it is not per hook | User-level (`$COPILOT_HOME/hooks/`) hooks ran with zero prompts on the first run. Repo-level `.github/hooks/*.json` hooks did **not** load until the project dir was added to `trustedFolders` in `$COPILOT_HOME/settings.json`; after that they ran with no further prompt. So trusting a folder trusts every hook file in it, for good. |
| **S2** | exit 127 fail-open or fail-closed? | **Fail-CLOSED on `preToolUse`** — verified | Row 2. Copilot is the only one of the three that blocks. |
| **S3** | Per-hook cloud-agent exclusion | **None found — UNVERIFIED as an absence** | No surface/environment selector appears in the shipped hook-config vocabulary I could read out of `runtime.node` (`type`, `command`, `bash`, `powershell`, `exec`, `args`, `cwd`, `env`, `timeoutSec`/`timeout`, `matcher`, `_vsCodeCompat`, `allowedEnvVars`, `url`, `headers`, `prompt`, plus top-level `version`, `hooks`, `disableAllHooks`). I could not execute the cloud agent, so I report this as "no such field observed", not as a proof of absence. It does not gate v1: grim commits no registration, so the cloud agent never sees a grim hook. |

Shell used for `command`-type hooks, verified from the failure message: **bash**
(`bash: line 1: …`).

---

## 3. Matcher dialects — the C-025 answer, executed

One `PreToolUse`/`preToolUse` registration per candidate matcher, each invoking the launcher with a
distinct tag, one run per client, tool call forced by the fake provider.

### 3.1 The table

Tool actually invoked: claude `Bash`; codex `exec_command`, **renamed to `Bash` in the hook
payload**; copilot `bash` (camelCase dialect) / **`Bash` (PascalCase dialect)**.

| matcher | claude (`Bash`) | codex (`Bash`) | copilot camelCase (`bash`) | copilot PascalCase (`Bash`) |
|---|---|---|---|---|
| *(omitted)* | fires | fires | fires | fires |
| `"*"` | fires | fires | **skipped — invalid regex** | fires |
| `"**"` | not tested | not tested | not tested | fires |
| `""` (empty) | fires | not tested | **skipped — "matcher cannot be empty"** | **skipped — same** |
| `"Bash"` | **fires** | **fires** | **does not fire** | **fires** |
| `"bash"` | does not fire | not tested | fires | fires |
| `"^Bash$"` / `"^bash$"` | fires | does not fire (`^exec_command$` also no) | fires | does not fire |
| `"as"` / `"hel"` (substring) | does not fire | does not fire | does not fire | does not fire |
| `"Ba*"` / `"ba*"` (glob intent) | **fires** (as regex `B`,`a*`) | not tested | does not fire | does not fire |
| `"Bash\|Write"` / `"bash\|shell"` | fires | fires | fires | fires |
| `".*"` | fires | fires | fires | fires |

### 3.2 What each dialect actually is

- **claude — start-anchored, case-sensitive regex**, with `*` and `""` special-cased to match-all.
  Proof of anchoring: `Ba*` fires (matches at position 0) but `as` does not, even though `as` occurs
  inside `Bash`. `^Bash$` fires, so the end is not forced.
- **codex — `*` is match-all; otherwise a case-sensitive regex over a Claude-style tool name.**
  `^exec_command$` and `exec.*` both fail while `Bash` succeeds: the *name being matched* is `Bash`,
  not the wire tool name `exec_command`. `.*` and alternation work.
- **copilot — two dialects, chosen by the casing of the event key in the config file, and they do not
  see the same tool name.**
  - camelCase `preToolUse` → **anchored full-match regex** (`^(?:PATTERN)$`) against `toolName`
    = `bash`. `*` is not a glob: the CLI logs
    `[ERROR] Invalid matcher regex in preToolUse hook: '*' — hook will be skipped`.
  - PascalCase `PreToolUse` → Claude-compatible semantics against `tool_name` = `Bash`; `*`/`**`
    match all, literal names match case-insensitively (`bash` matches `Bash`), `|` alternation works,
    and a regex like `^bash$` does **not** match. The payload also switches to snake_case
    (`hook_event_name`, `session_id`, `tool_name`, `tool_input`).
  - `""` is rejected in both: `[ERROR] hooks.preToolUse[3].matcher: matcher cannot be empty — hook
    will be skipped` (note the message normalizes the PascalCase key to `preToolUse`).

### 3.3 Copilot mutator — Open Question 2, settled by execution

Hook returns a canned JSON body; the fake provider's next request carries the tool result, so the
*executed* command is observable.

| Config event casing | Response shape | Mutation applied? |
|---|---|---|
| camelCase `preToolUse` | top-level `modifiedArgs` | **YES** → tool result `MUTATED_BY_modifiedArgs` |
| camelCase `preToolUse` | top-level `updatedInput` | no → `hook-probe` |
| PascalCase `PreToolUse` | `hookSpecificOutput.updatedInput` | **YES** → `MUT_hso_updatedInput` |
| PascalCase `PreToolUse` | `hookSpecificOutput.modifiedArgs` | no → `hook-probe` |
| PascalCase `PreToolUse` | top-level `modifiedArgs` | **YES** → `MUT_top_modifiedArgs` |

Both `hooks_vendor_reports/copilot.md` §7 (`modifiedArgs`) and `research_hooks_trampoline.md`
(`updatedInput`) were right about different dialects. Shipped-source corroboration: Copilot's
`runtime.node` string table lists both names adjacently in the hook-output field set
(`…permissionDecision permissionDecisionReason modifiedArgs updatedInput modifiedResult…`).

**Consequence:** Copilot `mutator` should **not** ship `Declined`. Ship it against the PascalCase
dialect, using `hookSpecificOutput.updatedInput` (or top-level `modifiedArgs`, which works in both).

**One security detail worth a threat-model line:** with the mutation applied, the Copilot CLI's own
transcript still displayed the **original** command (`echo hook-probe`) while executing the mutated
one. The human sees the un-mutated text. This is precisely what mutator control 5 (S-016, surfacing
the rewrite to the model/user) exists to compensate for, and it is a real, observed vendor
behaviour, not a hypothetical.

---

## 4. The guard — executed exit-code matrix

Pure-shell, no client involved, `grim` present on `$PATH`, launcher absent:

| Shell | Form | Exit | Output |
|---|---|---|---|
| `/bin/sh` (dash) | `command -v grim … && exec "<absent>" … \|\| exit 0` | **127** | `exec: /nonexistent/path/grim-hook: not found` |
| `/bin/bash` | same | **127** | `No such file or directory` |
| `/bin/dash` | same | **127** | `exec: … not found` |
| `/bin/sh`, `/bin/bash`, `/bin/dash` | `L="<absent>"; [ -x "$L" ] \|\| exit 0; exec "$L" …` | **0** | *(none)* |

Additional executed cases for the corrected form:

| Case | Result |
|---|---|
| launcher present + executable, path contains a space, `"$L"` quoted | exit 0, launcher runs, argv intact |
| launcher present but mode `0644` (the OCI-fetched case, C-019) | `[ -x ]` false → **exit 0, nothing spawned** — the guard also covers the non-executable payload |
| launcher present, `$L` **unquoted**, path contains a space, under `sh`/`bash` | exit **127**, tried to exec `…/wpb/grim` |
| same, with an unrelated executable planted at the word-split prefix | **exit 0 and the wrong binary ran**: `!!! WRONG BINARY EXECUTED (word-split prefix) argv: home/hooks/bin/grim-hook run --client copilot` |
| same, unquoted, under `zsh` (Codex's `$SHELL -lc`) | works — zsh does not word-split expansions |

So quoting is a correctness *and* an execution-selection issue, and it cannot be waived on the
grounds that "our shell is zsh": Codex uses the **user's `$SHELL`** (verified: hook probe printed
`SHELL0=/bin/zsh ZSH=5.9 BASH=`), Copilot uses **bash**, Claude uses **`/bin/sh`**. A grim-generated
command string must be POSIX-`sh`-safe and fully quoted to be correct on all three.

> **New risk not in the plan:** because Codex runs hooks through `$SHELL -lc` — a **login** shell —
> a user whose `$SHELL` is `fish` or `nushell` cannot execute the guard at all (`L="…";` is not fish
> syntax). Grim's registration for codex is a single string with no shell selector, so this is a real
> "hook silently never fires" class. Suggest: keep the string in the POSIX subset *and* record this
> in the vendor-capability watchlist; a `fish`-shell user is a supported developer configuration.

---

## 5. The § Launcher security argument, re-verified against real clients

The plan asserts a committed `${GRIM_HOME:-$HOME/.grimoire}/hooks/bin/grim-hook` would let anyone
who can set an environment variable from a repo file choose the executed binary (CWE-426). This was
argued from documentation. It is now executed:

| Client | Hook command | argv observed |
|---|---|---|
| copilot | `exec "$L" ENVPROBE "${GRIMPROBE:-DEFAULT}" "$HOME"` | `[ENVPROBE PWNED_FROM_CLIENT_ENV /home/mherwig]` |
| codex | `exec "$L" CX-ENVPROBE "${GRIMPROBE:-DEFAULT}"` | `[CX-ENVPROBE PWNED_FROM_CLIENT_ENV]` |
| claude | `exec "$L" CL-ENVPROBE "${GRIMPROBE:-DEFAULT}"` | `[CL-ENVPROBE PWNED_FROM_CLIENT_ENV]` |

`GRIMPROBE` was set in the *client's* environment, exactly as `.envrc` / `.mise.toml` /
devcontainer `containerEnv` would set `GRIM_HOME`. **All three clients expand it.** The plan's
conclusion stands, and it now stands on execution: *the registered path must be the absolute path
grim resolved at install time, on every client, in every scope.*

The corollary is the correction in § 6.1: this applies to Claude too. Claude's project registration
is **not** immune "by construction" because of exec-form argv — it is immune because grim writes an
absolute literal into a file that is not committed. The control is the literal path plus the
non-committed location, not the absence of a shell.

---

## 6. Corrections the plan needs

### 6.1 C-008 / § "The registration table" — Claude does **not** use exec-form argv

The plan states (registration table, and C-008's amendment) that claude · project and claude · global
carry "exec-form argv, absolute launcher", and contrasts codex/copilot as the ones that "have no
exec-form field, so they get a shell string". Claude Code 2.1.233's hook entry is
`{"type":"command","command":"<string>"}` and the string is executed by **`/bin/sh`** with full
expansion (§ 5). There is no argv array.

Impact:
- The "no shell, no expansion, no search path" clause of the immunity argument is wrong as written.
  The *conclusion* (claude · project is safe) survives, for the reason restated at the end of § 5.
- **C-018b becomes more important, not less**: since every client's registration is a shell string,
  the "no publisher-controlled value is ever interpolated into a generated shell string" contract now
  covers 3 of 3 clients rather than 2 of 3.
- Claude's registration should carry the same `[ -x "$L" ] || exit 0` guard. Claude is fail-open, so
  it is not a Block there, but it removes a spurious `Hook command failed with code 127` error in the
  user's transcript on every tool call when grim is not yet installed.

### 6.2 Copilot has an exec-form field — and grim still must not use it

Copilot CLI 1.0.80 accepts `{"type":"command","exec":"<absolute path>","args":["…"]}`. Verified: the
launcher was invoked with argv exactly `[X-execArgs, --root, global]`, no shell involved. `exec` must
be a **string** (`exec` as an array or object → `[ERROR] hooks.sessionStart[0].exec: Expected string
— hook will be skipped`), `args` is the argv tail, and `argv` is not a recognised key. The CLI also
enforces mutual exclusion: *"Specify either 'exec' (native executable) or 'bash'/'powershell'/
'command' (shell), but not both."*

**Do not use it.** With `exec` there is no shell, therefore no guard, and a missing launcher is a
spawn failure — which on `preToolUse` is fail-closed. Verified: `exec` pointing at an absent path
produced `Error in preToolUse hook … (fail-closed): Error: spawn /tmp/claude…` and the tool call was
**denied**. That breaks S-009 outright. The shell-string form with the guard is the only shape that
satisfies both "absolute path" and "grim absent ⇒ nothing blocks". Worth one sentence in C-008 so a
future reader does not "improve" it into `exec`.

### 6.3 Copilot registrations must use **PascalCase** event names

C-025's table for copilot has to key off this. Under camelCase event keys, grim's declared
`matcher = "Bash"` silently never matches and `matcher = "*"` is skipped as an invalid regex — the
guardrail reports as installed and does nothing. Under PascalCase, grim's Claude-style dialect
translates 1:1 (`*` → match-all, literal names, `|` alternation) and the payload arrives in the same
snake_case shape as Claude and Codex, which also simplifies WP-K's projector. Recommended per-client
translation:

| grim `matcher` | claude | codex | copilot (PascalCase) |
|---|---|---|---|
| `Bash` (exact name) | `Bash` | `Bash` | `Bash` |
| `*` (all) | `*` | `*` | `*` |
| `A\|B` (alternation) | `A\|B` | `A\|B` | `A\|B` |
| a **glob** with `*` inside (`Ba*`) | expressible only as regex; claude's regex is start-anchored, so `Ba*` means `B` + zero-or-more `a` — **not** the glob | same hazard | **Declined** — camelCase would skip it, PascalCase treats it as neither glob nor match-all |

The glob row is the C-025 "lossless-or-declined" case: **an interior `*` in a grim matcher must make
that `(hook, client)` pair `Declined` on all three clients**, because no client's field is a glob.
Only the exact-name, full-`*`, and alternation forms translate losslessly.

### 6.4 Codex project scope exists (it just isn't the reason for the decline)

`<cwd>/.codex/hooks.json` is loaded (§ 2.2). The plan's "codex · project — not registered in v1" row
should cite the executed-path argument (§ 5) rather than implying no surface exists. The trust gate
means a hostile clone's project hooks are Untrusted and silently skipped, which is a genuine
mitigation to record for T3 — and also the reason a *grim* project registration would be useless
until a human approves it.

---

## 7. Open Questions 2 and 3

**Question 2 — Copilot's mutator field name: RESOLVED, and the decline should be lifted.**
`modifiedArgs` (native / top-level) and `updatedInput` (Claude-compat, inside `hookSpecificOutput`)
are both real and both work, each in its own dialect (§ 3.3, executed). Since § 6.3 already pushes
grim to the PascalCase dialect, ship the mutator as `hookSpecificOutput.updatedInput`. Cell `◐`
(`Declined`) is no longer justified for Copilot CLI. Record the display mismatch (§ 3.3, last
paragraph) as an accepted, disclosed residual.

**Question 3 — Windows: PARTIALLY resolved; keep the refusal.**
Both fields exist and are accepted by the shipping binaries: codex `commandWindows` (loaded and
fired alongside `command`; also present in the binary's serde field table), copilot `powershell`
(loaded and fired alongside `command`). What remains unverified is the only thing that matters —
whether either client can actually invoke a non-`.exe` launcher on Windows, since `CreateProcess`
will not exec a `.cmd`/`.ps1` directly. **Recommendation: keep "the experimental flag refuses to arm
on Windows"** for v1, and narrow the reason in the message from "the fields are unverified" to
"launcher invocation on Windows is unverified". What would settle it: a Windows host with either CLI
installed, a `grim-hook.cmd` (or `.ps1`) launcher, and the same probe run.

---

## 8. Gate verdicts for the dependent work packages

| WP | Can it proceed on this evidence? | Conditions |
|---|---|---|
| **WP-F** (vendor hook seam, 18 vendors, C-025 table) | **YES** | Use the § 6.3 translation table. Two hard requirements it must encode: copilot registrations use **PascalCase** event names; an interior-`*` glob matcher is `Declined` on all three clients. Copilot `mutator` is **supported**, not `Declined` (§ 7). |
| **WP-I** (dispatch table, launcher, `sync_config`) | **YES** | Guard form `L="<abs>"; [ -x "$L" ] \|\| exit 0; exec "$L" …` is verified correct on all three, and mandatory on copilot. Emit it for **claude too** (§ 6.1). Keep the string POSIX-`sh`-safe and fully quoted (§ 4). Never `exec`-form on copilot (§ 6.2). Never an env-derived path anywhere (§ 5). |
| **WP-J1** (path/anchor resolution, matrix cell) | **YES** | Nothing in this spike contradicts the anchor model; global-scope surfaces confirmed as `~/.claude/settings.json`, `$CODEX_HOME/hooks.json`, `$COPILOT_HOME/hooks/<file>.json` (all three loaded and executed). |
| **WP-P0** (whatever consumes this verdict) | **YES, with two amendments to carry forward** | (a) C-008's exec-form claim for claude is wrong (§ 6.1); (b) codex's trust gate means an armed hook is **not running** until a human approves it in the `/hooks` TUI, silently — this is a first-class `not-armed` case for C-017 and needs UX text, not just a status token. |

Nothing here blocks any of the four.

---

## 9. What is still unverified, and exactly what would settle it

| Item | Why | Settling experiment |
|---|---|---|
| Windows invocation of a non-`.exe` launcher (codex `commandWindows`, copilot `powershell`) | No Windows host | Windows box + `grim-hook.cmd` + the same probe |
| Codex re-prompt on an unchanged-command file rewrite | Trust can only be granted through the interactive `/hooks` TUI; no scripted verb exists; `app-server` `hooks/list` probe hung | Human approves once in `/hooks`, grim rewrites the file identically, restart and observe |
| Claude Code writing the `.gitignore` entry for `.claude/settings.local.json` | I created the file by hand | Let Claude Code create it itself in a fresh git repo (accept a permission rule) and inspect `.gitignore` |
| Copilot cloud agent behaviour and any per-hook exclusion | No cloud agent reachable; also out of v1 scope | GitHub-hosted Copilot coding agent run with a repo hook file |
| Copilot behaviour under a real (non-BYOK) Copilot API session | Corporate proxy blocks `api.*.githubcopilot.com` with HTTP 403 | Re-run § 2.3 off-VPN or with proxy allowance. The hook engine is the same Rust `runtime.node` in both modes, so I expect no difference, and I am not claiming one. |
| `bypass_hook_trust` as a config.toml key | `-c bypass_hook_trust=true` had no effect; the working flag is `--dangerously-bypass-hook-trust` | Not needed for grim |

---

## 10. Reproduction

Scratch tree (session-local, not committed): `…/scratchpad/wpb/` containing `fakeoai.py`
(Chat Completions), `fakeresp.py` (Responses), `cph/` (`COPILOT_HOME`), `cxh/` (`CODEX_HOME`),
`proj/` (probe workspace, a git repo), and `grim home/hooks/bin/grim-hook` (the probe launcher, path
deliberately containing a space).

```sh
# Copilot, no GitHub auth needed
python3 fakeoai.py 8901 &   # MODE=tool TOOLNAME=bash FAKE_OUT=…
COPILOT_HOME=…/cph COPILOT_PROVIDER_BASE_URL=http://127.0.0.1:8901/v1 \
  COPILOT_MODEL=fake-model COPILOT_PROVIDER_API_KEY=none \
  copilot -p "run a probe" --allow-all-tools --no-color --log-level debug --log-dir …/logs

# Codex, no OpenAI auth needed  (config.toml: [model_providers.fake] base_url=…, wire_api="responses", env_key="FAKE_KEY")
python3 fakeresp.py 8905 &  # MODE=tool TOOLNAME=exec_command TOOLARGS='{"cmd":"echo hook-probe"}'
CODEX_HOME=…/cxh FAKE_KEY=dummy \
  codex exec --skip-git-repo-check --dangerously-bypass-hook-trust "run a probe" < /dev/null

# Claude, real session, project-scope hooks in <proj>/.claude/settings.local.json
claude -p "Run the bash command: echo hook-probe . Then reply OK." --allowedTools Bash < /dev/null
```

Guard matrix (no client needed):

```sh
for sh in /bin/sh /bin/bash /bin/dash; do
  "$sh" -c 'command -v grim >/dev/null 2>&1 && exec "/nonexistent/grim-hook" run || exit 0'; echo "$sh old=$?"
  "$sh" -c 'L="/nonexistent/grim-hook"; [ -x "$L" ] || exit 0; exec "$L" run';            echo "$sh new=$?"
done
```

## 11. Sources

Primary, all executed on 2026-08-16 against the installed binaries listed at the top. Secondary,
read (never counted as a PASS): the shipped Rust string tables in Copilot's
`@github/copilot-linux-x64/prebuilds/linux-x64/runtime.node` (hook engine paths
`src/runtime/src/hooks/{config,loading,declarative,shell_selection,command_executor,http_executor}.rs`,
hook-output field set, validation messages) and Codex's `codex` binary (`HookHandlerConfig::Command
with 6 elements`, `commandWindows`, `HookStateToml.trusted_hash`, `hooks/src/engine/command_runner.rs`
with `SHELL`/`-lc`); Copilot's bundled `schemas/api.schema.json` (`HookType` enum, 17 events);
`.agents/research/hooks_vendor_reports/{claude,codex,copilot}.md` and
`research_hooks_codex_surface.md` for the claims under test.
