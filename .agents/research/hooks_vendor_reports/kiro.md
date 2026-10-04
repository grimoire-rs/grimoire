# Kiro (IDE + Kiro CLI) — hook / lifecycle-event mechanism

Research date: 2026-08-14. All sources fetched 2026-08-14 unless noted.

## 1. Existence & name

Yes. Vendor calls it **"Agent Hooks"** (IDE product name, capitalized in headers) / **"Hooks"**
(doc section title, both IDE and CLI). Presented in the docs nav as a core, stable feature —
not listed on the CLI's dedicated `/docs/cli/experimental/` page, no "beta"/"preview" badge found
anywhere in the docs.

**Two incompatible generations exist, plus one internal split (IDE vs CLI heritage). This
versioning is the single most important fact for a portable schema design:**

| Date | Event | Source |
|---|---|---|
| pre-2025 | Amazon Q Developer CLI ships agent config with embedded `hooks` (`agentSpawn`, `userPromptSubmit`, `preToolUse`, `postToolUse`, `stop`) at `~/.aws/amazonq/cli-agents/*.json` / `.amazonq/cli-agents/*.json` | aws.github.io/amazon-q-developer-cli/agent-format.html |
| 2025-07-16 | Kiro IDE launches "Agent Hooks": file-event triggers only (`fileEdit`, `fileCreate`, `fileDelete`, `userTriggered`/manual), single action type `askAgent` (prompt only, **no shell command**). Files: `.kiro/hooks/*.kiro.hook`, git-shared. | kiro.dev/blog/automate-your-development-workflow-with-agent-hooks/ |
| 2025-11-17 | Amazon Q Developer CLI v1.20.0 rebrands to **Kiro CLI**; embedded agent-config hooks mechanism carries over verbatim, path becomes `.kiro/agents/*.json` / `~/.kiro/agents/` | docs.aws.amazon.com/amazonq/.../upgrade-to-kiro.html (rebrand notice, does not itself mention hooks); path confirmed via kiro.dev/docs/cli/2x-reference/ and live GitHub issue #8021 |
| 2026-02-05 | IDE v0.9: adds `PreToolUse`/`PostToolUse` hook triggers (tool interception), matcher gains category filters (`read`/`write`/`shell`/`web`) | kiro.dev/changelog/ide/0-9/ |
| ~2026-03/04 (undated in changelog; bracketed by evidence below) | IDE gains a deterministic `runCommand` action type (shell command, not just `askAgent`) | inferred from GitHub issues #7500 (2026-04-16) and #7375 (2026-04-11), both already discussing "IDE runCommand hooks" as existing |
| 2026-05-01 | Kiro CLI 2.2.0 still uses the legacy embedded-in-agent-config hooks format at `.kiro/agents/kiro_default.json` | GitHub issue #8021 (open) |
| **2026-06-25** | **IDE 1.0.0 / CLI 3.0 — breaking rewrite.** Hooks become standalone files, unified across IDE and CLI: `.kiro/hooks/*.json`, `"version": "v1"`, `hooks` array, PascalCase trigger names. Old embedded format explicitly **"no longer supported in 3.0"** — hard break, not soft-deprecated. Migration is either `kiro-cli agent migrate` or manual. Legacy 0.x IDE hook files show an "upgrade badge" in the Agent Hooks panel and **do not execute until migrated**. | kiro.dev/docs/cli/v3/hooks-migration/, kiro.dev/changelog/ide/ (1.0.0 entry) |
| 2026-07-02 | IDE 1.0.52: "hooks auto-migrate from the legacy format" (softer, automatic follow-up to the June hard cut) | kiro.dev/changelog/ide/ |
| 2026-07-09 | IDE 1.0.116: hooks extended to "agent-driven file changes" | kiro.dev/changelog/ide/ |
| 2026-07-17 | **CLI 2.13.0**: adds **global hooks** — `~/.kiro/hooks/` fire in every workspace; workspace-level hooks continue to work alongside global ones (additive, not override) | kiro.dev/changelog/cli/2-13/ |
| 2026-07-20 | **IDE 1.0.182**: "adds user-level global hooks" (IDE-side counterpart to the above) | kiro.dev/changelog/ide/ |
| 2026-08-13 | IDE 1.0.309: further hooks/MCP-connection reliability improvements (no schema change mentioned) | kiro.dev/changelog/ide/ |

Deprecation: **yes, hard** — the CLI 2.x / IDE-0.x embedded and per-file legacy formats are
removed (not merely discouraged) as of the June 2026 rewrite; only auto-migration or manual
rewrite carries a repo forward.

## 2. Config location(s)

**Current (v1 schema, IDE 1.0+ / CLI 3.0+):**
- Project scope: `.kiro/hooks/*.json` (real-world repos also still use the `*.kiro.hook`
  extension — both are attested; the docs' own generic examples use `.json`, the community
  best-practices repo below uses `.kiro.hook`. Whichever extension, content is plain JSON.)
- Global/user scope: `~/.kiro/hooks/` — added 2026-07-17 (CLI)/2026-07-20 (IDE). **Additive**
  with workspace hooks, not override: "Hooks placed in `~/.kiro/hooks/` now fire in every
  workspace automatically... Workspace-level hooks continue to work alongside global ones."
  (kiro.dev/changelog/cli/2-13/, verbatim).
  - Before this shipped, the IDE had a confirmed discovery bug: it scanned
    `<workspace>/.kiro/hooks/` by literally appending that suffix to each open folder, so for
    the global folder itself (`~/.kiro`) the effective scanned path became
    `~/.kiro/.kiro/hooks/` — one level too deep, silently finding nothing at `~/.kiro/hooks/`.
    (GitHub issue #9075, closed as duplicate, filed 2026-06-02; original ask was GitHub issue
    #5440, filed 2026-02-05, closed.)

**Legacy (CLI 2.x, Amazon-Q-CLI heritage):** hooks lived **inside** agent config, not a
separate directory: `.kiro/agents/<agent-name>.json` (project) / `~/.kiro/agents/` (global),
under a `hooks` key. Confirmed still in production use in kiro-cli 2.2.0 as of 2026-05-01
(issue #8021).

**Original Amazon Q Developer CLI (pre-rebrand) ancestor:** `~/.aws/amazonq/cli-agents/*.json`
(global), `.amazonq/cli-agents/*.json` (project) — same embedded-`hooks` shape. Confirmed via
`aws.github.io/amazon-q-developer-cli/agent-format.html` and validated with
`q agent validate --path .amazonq/cli-agents/your-agent.json`. **This path does not survive**
into Kiro — Kiro CLI 2.x moved it to `.kiro/agents/`, and Kiro CLI 3.0 moved hooks again, out
of agent config entirely, into `.kiro/hooks/`.

**Kiro's own plugin system ("Powers")** installs to `~/.kiro/powers/installed/<name>/` and
copies a `hooks/` subdirectory from the power's source repo into that install location — **but
Kiro does not load hooks from there.** Verbatim, GitHub issue #9007 (open, filed 2026-05-31):
"A `hooks/` directory in the power's repo is copied to
`~/.kiro/powers/installed/<name>/hooks/` during installation, but Kiro does not load hooks
from that location." Current workaround documented in that issue: tell the user to hand-copy
files into their workspace `.kiro/hooks/`.

**Format:** JSON only. No JSONC/TOML/YAML/JS/TS variant found anywhere in the docs.

**Env var relocation:** `KIRO_HOME` relocates the **CLI's** entire `~/.kiro` root (agents,
prompts, skills, steering, settings, sessions — hooks not explicitly enumerated in any single
source, but hooks live under the same root so this plausibly extends to
`$KIRO_HOME/hooks/` by construction; **not independently confirmed** for hooks by name in any
primary source). The **IDE ignores `KIRO_HOME` outright** — confirmed directly, GitHub issue
#9148 (closed, filed 2026-06-04), verbatim: "Kiro IDE hardcodes `~/.kiro/` as the global
configuration directory for steering files, skills, and agents. The CLI supports `KIRO_HOME`
to relocate this root, but the IDE ignores it — even though the env var is visible in the
IDE's process (`process.env.KIRO_HOME` is set)." (Note: this issue's own enumeration of
affected config types — "steering files, skills, and agents" — does not name hooks; global
hooks didn't exist yet when this issue was filed, so it's silent on hooks by omission, not by
explicit exclusion.)

**Merge behavior:** within a scope, all files under `.kiro/hooks/` are loaded additively — "Each
file defines one or more hooks" (multiple hook objects per file, via the `hooks` array).
Across scopes (global vs workspace), also additive/union — not override.

## 3. Config schema — verbatim

**Current (v1, IDE 1.0+/CLI 3.0+)** — an **array** of hook entries (`"hooks": [...]`) inside a
file, not a map keyed by event name. Verbatim from `kiro.dev/docs/cli/v3/hooks-migration/`:

```json
{
  "version": "v1",
  "hooks": [
    {
      "name": "lint-on-save",
      "trigger": "PostFileSave",
      "matcher": "\\.ts$",
      "action": { "type": "command", "command": "npm run lint" },
      "timeout": 30,
      "enabled": true
    }
  ]
}
```

Fields observed across multiple independent fetches: `name` (plain string), `trigger`
(PascalCase enum, see §4), `matcher` (optional regex/category string, see below),
`action.type` (`"command"` | `"agent"`), `action.command` or `action.prompt`, `timeout`
(seconds), `enabled` (bool), and an opt-in `confirm` block (see below). **`name` is a plain,
apparently-unenforced-unique string — the closest thing to a stable identity for a third-party
installer.** No generated id/UUID field was found in any schema fetch.

**Real, currently-shipping example** (verbatim, from
`github.com/awsdataarchitect/kiro-best-practices` — `.kiro/hooks/lint-and-format-on-save.kiro.hook`,
note this repo uses the **older pre-v1 per-file shape** — `when`/`then`, not `trigger`/`action`
— demonstrating that both shapes are live in the wild simultaneously depending on when the repo
last touched its hooks):

```json
{
  "enabled": true,
  "name": "Lint and Format on Save",
  "description": "Automatically lint and format code when files are saved following project standards",
  "version": "1",
  "when": {
    "type": "fileEdited",
    "patterns": ["**/*.ts", "**/*.js", "**/*.tsx", "**/*.jsx", "**/*.py", "**/*.json"]
  },
  "then": {
    "type": "askAgent",
    "prompt": "A code file has been saved. Please:\n1. Run the appropriate linter (ESLint for JS/TS, flake8/pylint for Python)\n2. Run the appropriate formatter (Prettier for JS/TS, Black for Python)\n3. Fix any auto-fixable issues\n4. Report any remaining issues that need manual attention\n\nUse the project's existing configuration files (.eslintrc, .prettierrc, pyproject.toml, etc.) and follow the established coding standards."
  }
}
```
Note this generation's `then.type` is `askAgent` only — **no shell-command action existed in
this schema generation at all**; `version` here is the bare string `"1"`, distinct from the
current `"v1"`.

**Legacy CLI 2.x / Amazon-Q-CLI-heritage embedded format** — here it **is** a named map keyed
by camelCase event, values are arrays of `{command, matcher}`, **no name/id field per hook at
all** (only the containing agent file has a name). Verbatim ancestor example from
`aws.github.io/amazon-q-developer-cli/agent-format.html`:

```json
{
  "hooks": {
    "agentSpawn": [
      { "command": "git status" }
    ],
    "preToolUse": [
      {
        "matcher": "execute_bash",
        "command": "{ echo \"$(date) - Bash command:\"; cat; echo; } >> /tmp/bash_audit_log"
      }
    ],
    "postToolUse": [
      { "matcher": "fs_write", "command": "cargo fmt --all" }
    ]
  }
}
```
Kiro's own migration doc shows the same shape carried into CLI 2.x verbatim except tool-name
matchers evolved toward category strings:
```json
{
  "hooks": {
    "agentSpawn": [{"command": "echo 'starting'", "matcher": ".*"}],
    "preToolUse": [{"command": "npm run lint", "matcher": "Write|Edit"}],
    "fileEdited": [{"command": "prettier --write", "matcher": "\\.ts$"}]
  }
}
```
(kiro.dev/docs/cli/v3/hooks-migration/, labelled "Old Format (2.x) — Deprecated" there,
contrasted directly against the v1 array format as "New Format (3.0) — Current Standard.")
Migration steps given on that page: run `kiro-cli agent migrate` for automatic conversion, or
manually move hooks out of agent config into standalone `.kiro/hooks/*.json` files, rename
trigger keys per a provided mapping table, add `version: "v1"` plus required fields
(`name`, `trigger`, `action`). Stated verbatim: "The old embedded format is **no longer
supported in 3.0**" — and "Regex patterns from CLI 2.x transfer directly."

**Matcher/filter syntax** (current schema, from `kiro.dev/docs/hooks/types/`): for
`PreToolUse`/`PostToolUse`, matcher accepts: category filters `read`, `write`, `shell`, `web`,
`spec`, `*`; source prefixes `@mcp`, `@powers`, `@builtin`; and regex patterns combining both,
e.g. `@mcp.*sql.*`. For file-event triggers, matcher is a plain regex against the file path
(e.g. `\.ts$`, or glob-style `**/*.{js,ts,jsx,tsx}` per the examples page — both regex-looking
and glob-looking matcher strings appear across doc pages; **exact grammar (regex vs glob) is
NOT DOCUMENTED as a single unambiguous spec** — treat as regex per the schema page but expect
glob-style patterns to also appear in the wild/examples).

**`confirm` block** (opt-in, author-added, not a vendor-imposed gate — see §9). Reconstructed
from two independent WebSearch extractions of vendor docs (I could not get a single WebFetch to
surface the full JSON verbatim in one pass — flagging accordingly):
```
confirm: {
  question: string,
  options: [ { id: string, label: string, run: boolean }, ... ],
  confirmCommand: string   // optional; stdout parsed as JSON
}
```
`confirmCommand`'s stdout, if present, is parsed as JSON and can return `{"skip": true}`
(suppress the prompt, skip the hook this turn) or `{"question": "...", "options": [...]}`
(replace the static prompt dynamically). Documented as available at least for `command` hooks
on the `Stop` trigger; broader applicability **NOT DOCUMENTED** with full confidence.

## 4. Event catalogue

Current unified set (PascalCase = JSON `trigger` value; parenthetical = doc heading/display
name where it differs):

| Trigger (verbatim) | Fires when | Scope |
|---|---|---|
| `SessionStart` | Session begins | session lifecycle |
| `AgentSpawn` (camelCase `agentSpawn` in CLI-native docs) | Agent is first activated, no tool context yet | **CLI-only**; session lifecycle |
| `UserPromptSubmit` ("Prompt Submit") | User submits a prompt to the agent | prompt submit; **can block** |
| `PreToolUse` ("Pre Tool Use") | Before the agent invokes a tool | pre tool use; **can block** |
| `PostToolUse` ("Post Tool Use") | After a tool invocation, with access to its result | post tool use |
| `PostFileCreate` ("File Create") | Agent creates a new file matching `matcher` | file edit (post-hoc only) |
| `PostFileSave` ("File Save") | Agent saves/modifies a file matching `matcher` | file edit (post-hoc only) |
| `PostFileDelete` ("File Delete") | Agent deletes a file matching `matcher` | file edit (post-hoc only) |
| `PreTaskExec` ("Pre Task Execution") | Before a spec task starts (status → in_progress) | **IDE-only**; task/spec lifecycle; **can block** |
| `PostTaskExec` ("Post Task Execution") | After a spec task completes (status → completed) | **IDE-only**; task/spec lifecycle |
| `Stop` ("Agent Stop") | Agent has completed its turn / finished responding | stop/finish |
| Manual Trigger | On-demand, run from the Agent Hooks panel / quick-run | manual — **as of IDE 1.0, "Manual hooks have been replaced by manual steering files"** (kiro.dev changelog, verbatim) — i.e. this is no longer a hook trigger in the current generation, it moved to a different mechanism (steering) entirely. Flag for disambiguation per the brief. |

Blocking-capable set, stated together in one doc passage: **"PreToolUse, UserPromptSubmit, and
PreTaskExec can block — a command action exiting with code 2 stops the operation and returns
stderr to the agent."** All Post* events and `Stop`/`SessionStart` are after-the-fact/informational.

Groups requested by the brief with no match found — treated as **NOT DOCUMENTED / does not
appear to exist**: a dedicated **notification** event; a **compaction**/context-window event; a
**subagent** lifecycle hook (Kiro has "custom subagents" as a distinct feature with its own
config, but no `SubagentStart`/`SubagentStop` hook trigger was found in any source); an
**error**/failure event distinct from a hook's own non-zero exit.

Legacy CLI 2.x / Amazon-Q-CLI camelCase event names for comparison: `agentSpawn`,
`userPromptSubmit`, `preToolUse`, `postToolUse`, `fileEdited`, `fileCreated`, `stop`/`agentStop`.

## 5. Invocation

- **`command` action**: a shell command **string**, executed in the project root (cwd = project/
  workspace root, per docs "Command actions run a shell command in your project root"). Shell
  binary used, `$PATH` handling: **NOT DOCUMENTED**. On Windows, at least one confirmed bug
  report ran a `.ps1` under PowerShell 5.1 without complaint from Kiro (issue #7915),
  suggesting Kiro shells out to the OS default interpreter for the script rather than forcing a
  specific shell, but this is inferred from one bug report, not a doc statement.
- **`agent`/`askAgent` action**: not a subprocess — injects a prompt into an agent turn (new or
  current); consumes LLM credits; non-deterministic. For `UserPromptSubmit` specifically, the
  hook's prompt text is **appended to** the user's own submitted prompt, not a replacement.
- **Concurrency/ordering** across multiple hooks matching the same event: **NOT DOCUMENTED**.
- **Blocking vs fire-and-forget**: blocking-capable triggers (`PreToolUse`, `UserPromptSubmit`,
  `PreTaskExec`) make the agent wait for the hook's exit code before proceeding; all other
  triggers fire after the fact and cannot veto anything (though a non-zero exit still surfaces
  as a "failure warning" per one source — exact UI/agent-visible behavior for a failing
  non-blocking hook is **NOT DOCUMENTED** precisely).

## 6. Input payload — verbatim

**Documented/intended contract**: JSON on **stdin**. Confirmed independently for the CLI (CLI
2.x reference: "Hooks receive context via STDIN as JSON") and requested/expected for the IDE in
open issue #7500. Common keys across events, per the closest thing to an official shape
(reconstructed from GitHub issue #7500's requested payload plus issue #7375's description of
already-working CLI behavior):

```json
{
  "hook_event_name": "preToolUse",
  "tool_name": "read",
  "tool_input": { "operations": [{ "mode": "Line", "path": "/some/file.ts" }] },
  "cwd": "/home/user/project",
  "session_id": "abc123"
}
```
`hook_event_name`, `cwd`, `session_id` appear common to every event; tool events add
`tool_name` + `tool_input` (pre) and (requested, not independently confirmed shipped)
`tool_response` (post). `userPromptSubmit`'s exact key for the prompt text is **NOT DOCUMENTED
verbatim** (only described generically as "the user's prompt").

**Actual observed IDE behavior conflicts with the documented contract.** GitHub issue #7375
(closed/duplicate, filed 2026-04-11, reporter running Kiro v0.11.131) found the IDE's
`preToolUse` `runCommand` hook does **not** deliver JSON on stdin as documented — instead it
stringifies `toolArgs` into an environment variable named **`USER_PROMPT`**, and at the time of
the report this arrived empty (`{}`) rather than populated: "the plumbing exists — the args
just aren't being populated upstream." The CLI, by contrast, was confirmed working correctly
via stdin JSON in the same issue. **This is a live, filed discrepancy between docs and shipped
IDE behavior, not a hypothetical** — treat IDE tool-context delivery as unreliable until
independently re-verified against a current build.

Template-string interpolation (e.g. `{{file_path}}` inside the command string itself): one
search synthesis mentioned a `file_path` "template variable," but no doc page could be made to
produce a verbatim interpolation-syntax example under direct fetch. **NOT DOCUMENTED** with
enough confidence to state a syntax; the `USER_PROMPT` env-var precedent suggests env vars are
at least as likely a delivery channel as `{{}}` templating for the IDE specifically.

## 7. Output / response contract — verbatim

No structured JSON response object was found anywhere (nothing resembling Claude Code's
`hookSpecificOutput`/`permissionDecision`/`additionalContext`). The contract is exit code +
raw text:

- **Exit 0** → success. For `command` actions, **stdout is added to the agent's context**
  (i.e., the model sees it as injected text, not a human-facing message per se).
- **Exit 2** → for the three blocking-capable triggers only (`PreToolUse`, `UserPromptSubmit`,
  `PreTaskExec`), the operation is blocked and **stderr is returned to the agent** as the
  reason. (CLI 2.x reference doc, and the /docs/hooks/ synthesis, agree on this exact 0/2
  convention — same numbers Claude Code uses, though the response body is unstructured text
  here, not a JSON object.)
- **Other non-zero exit** → described only as a generic "failure warning"; exact per-trigger
  handling (does a non-blocking trigger's failure still surface anywhere visibly?) is **NOT
  DOCUMENTED**.
- The `agent`/`askAgent` action has no exit-code concept at all — its "output" is whatever the
  agent chooses to do with the injected prompt.
- The only place a hook's stdout **is** parsed as structured JSON is the opt-in
  `confirm.confirmCommand` sub-feature (§3) — scoped narrowly to customizing a pre-run
  confirmation dialog, not the hook's main result.
- Stderr routing on a *block*: to the agent (documented). Whether raw stdout/stderr is also
  shown to the human user in the IDE transcript on success or on a non-blocking failure: **NOT
  DOCUMENTED**.

## 8. Reliability & limits

- **Timeout**: documented default **60 seconds**; override via the `timeout` field (seconds);
  `0` disables it entirely. **Confirmed broken on Windows** — GitHub issue #7915 (closed, filed
  2026-04-28): a hook configured with `timeout: 300` running a 90-second PowerShell script was
  still killed at ~57–59 seconds; testing with `300`, `0`, and the field omitted "produced
  identical results, all terminating around the 60-second mark." No maintainer fix confirmed in
  what was retrievable.
- **Missing binary / malformed output**: **NOT DOCUMENTED**.
- **Parallel execution / ordering** across multiple hooks matching one event: **NOT
  DOCUMENTED**.
- **Blocking vs fire-and-forget**: see §5 — three named triggers block, the rest do not.

## 9. Security posture

No hooks-specific security section was found on `kiro.dev/docs/privacy-and-security/` or
`kiro.dev/docs/cli/chat/security/`. What that page does say, verbatim, about command execution
generally (chat-driven tool calls, not confirmed to apply to hook `command` actions — see
below): **"By default, Kiro requires approval before running any command."** A user can
maintain a "Trusted Commands" allowlist (**Kiro Agent: Trusted Commands** setting) that can
include a bare `*` to "trust all commands (use with extreme caution)"; the docs add: **"It does
not analyze command structure, chains, or special characters, putting full responsibility on
you to carefully configure trusted patterns."**

**Whether that same per-command approval gate applies to hook `command` actions is NOT stated
outright anywhere I could find** — I infer it does **not** apply by default, from: (a) hooks
are pitched throughout the docs and blog as unattended automation ("run tests after every file
save," "enforce standards," "gate dangerous operations" — value proposition depends on not
prompting every time); (b) the `confirm` block (§3) is opt-in and author-supplied, which only
makes sense as a feature if the default is silent, unprompted execution. This is my inference
from the schema's shape, not a directly quoted vendor claim — flagged accordingly.

No hook-specific CVE or security advisory was found. Two adjacent, **not hook-specific** CVEs
establish Kiro's track record on the same general risk class (agent-writable, auto-loaded,
project-relative config that can trigger code execution):

- **CVE-2026-4295** (AWS security bulletin 2026-009): "improper trust boundary enforcement
  allowed arbitrary code execution when a user opened a maliciously crafted project directory,"
  affected versions < 0.8.0, fixed in 0.8.0. Predates tool-use hooks even existing (v0.9 shipped
  PreToolUse/PostToolUse a few months later). Hooks not mentioned.
- **CVE-2026-10591** (Intezer Research, published 2026-07-20/23; reported 2026-02-11; fixed
  2026-04-03; affected v0.9.2 and v0.10.16): root cause quoted verbatim — "the root cause is
  that `~/.kiro/settings/mcp.json` is not protected" and "Kiro can write to it on its own using
  the `fsWrite` tool without user approval." A prompt-injected web page could make Kiro rewrite
  its own MCP config to plant a malicious auto-executed server command. Verbatim on the trust
  model this defeated: "The user stays in the loop, reviews what the agent wants to do, and
  clicks 'allow.' That approval step is the security boundary" — bypassed entirely because the
  one file that decides what code later runs was itself writable without triggering that
  approval step. **This report does not mention `.kiro/hooks/` as an attack vector — only
  `mcp.json`.** I flag it only as a structural analogy: `.kiro/hooks/*.json` sits in the
  identical risk category (agent-writable via `fsWrite`, auto-loaded by trigger, capable of
  running arbitrary `command` actions), and no source I found describes any hook-specific
  mitigation (signing, first-use confirmation, diffing against a session-start snapshot) beyond
  the general "requires approval before running any command" statement whose applicability to
  hooks is itself unconfirmed (above).
- **Config snapshotting at session start**: **NOT DOCUMENTED for hooks specifically.** Adjacent
  evidence points toward live-reload, not snapshotting: general Kiro config docs state "When
  you edit an agent config, add a new agent file, or modify `mcp.json`, the changes take effect
  without restarting the session." If hooks follow the same model (unconfirmed for hooks by
  name), a hook planted or edited mid-session could become active without a client restart —
  the opposite of a safety net.

## 10. Third-party installability

**Direct file authorship is not just possible but the documented, intended workflow** — hooks
are explicitly designed to be hand-edited and git-shared. Verbatim from the original launch
blog: "Every new hook you create lives in the `.kiro/hooks` directory, ready to be shared. Once
you push the changes, your teammates can pull and start using your hooks instantly." Real
public repos commit hook files directly with no IDE-side registration step visible:
`github.com/awsdataarchitect/kiro-best-practices/.kiro/hooks/lint-and-format-on-save.kiro.hook`,
`github.com/viatoro/ecc/.kiro/hooks/`.

**Telling counter-evidence that direct file-write is the *only* reliable channel today**: Kiro
has its own first-party plugin/package system ("Powers," analogous in spirit to what `grim`
does) which installs to `~/.kiro/powers/installed/<name>/` — but it **cannot** deliver hooks
through that normal install path. Verbatim, GitHub issue #9007 (open, filed 2026-05-31): "A
`hooks/` directory in the power's repo is copied to
`~/.kiro/powers/installed/<name>/hooks/` during installation, but Kiro does not load hooks from
that location." The documented current workaround is telling power authors to instruct their
users to manually copy hook files into the workspace's own `.kiro/hooks/` — i.e., even Kiro's
own more-official distribution mechanism has to fall back to exactly the direct-file-write
approach a third-party tool like `grim` would use. There is no more-official channel to be
"missing out on" by writing files directly.

**Version-targeting risk for an installer**: writing the current `v1` array schema targets
IDE 1.0+/CLI 3.0+ correctly. On an older client, the file would either be invisible or (per the
IDE's own "upgrade badge" behavior for stale files) visible-but-inert until the user manually
migrates — there is no single schema shape that "just works" across the full version range in
the wild today (confirmed still-in-production legacy usage as late as 2026-05-01, issue #8021).

**Restart requirement**: **not confirmed either way for hooks specifically** (see §9's
snapshotting discussion) — general config in Kiro appears to hot-reload, which would mean no
restart needed, but I found no source naming hooks explicitly in that reload behavior.

## 11. Trampoline viability

For the **`command` action type**, a generic trampoline looks genuinely viable: the contract is
about as thin as they come — spawn a process from a plain shell-command string, feed it JSON on
stdin (per docs), read an exit code, read stdout/stderr. No JS/TS module, no in-process
function, no HTTP endpoint, no typed response object to construct. A single
`grim hook run --client kiro --event <trigger>` command could plausibly serve as the `action.command`
value across every current trigger.

**Concrete blockers found, not hypothetical ones:**

1. **The documented stdin-JSON contract does not reliably match shipped IDE behavior.** Filed,
   reproducible bugs (#7375, #7500) show the IDE's `command`/`runCommand` hooks may receive no
   usable tool context at all (`{}` via a `USER_PROMPT` env var, not stdin JSON) for
   `PreToolUse`/`PostToolUse`, while the CLI's stdin-JSON delivery works as documented. A
   trampoline that branches on `tool_name`/`tool_input` would function on Kiro CLI today and
   may silently get nothing on Kiro IDE, depending on which build a user runs.
2. **Two live schema generations plus a fully separate legacy-per-file generation** a
   general-purpose installer must pick one of, or detect and branch on: CLI-2.x/Amazon-Q-CLI
   embedded map-of-arrays (`hooks: {camelCaseEvent: [{command, matcher}]}`, no per-hook
   identity), pre-1.0 IDE per-file `when`/`then`+`askAgent`-only (no shell command possible at
   all), and current unified `v1` array (`hooks: [{name, trigger, action, matcher, timeout,
   enabled, confirm}]`).
3. **The `agent`/`askAgent` action type is not trampolineable in the shell-command sense at
   all** — there is no subprocess, no exit code, no stdout to capture; it is "start/steer an
   LLM turn" with a prompt string. A portable `hook` artifact kind that wants to reach this half
   of Kiro's model needs to emit a **prompt string**, not a command — a fundamentally different
   payload shape from the trampoline binary, and non-deterministic/credit-consuming by nature.
4. **Windows timeout override is confirmed broken** (#7915) — a trampoline cannot safely rely
   on requesting more than the ~60s default on Windows today.
5. **No verified global-scope + `KIRO_HOME` interaction for hooks** — a portable global-scope
   installer should probe/verify at install time rather than assume `$KIRO_HOME/hooks/` is
   honored, and must remember the **IDE ignores `KIRO_HOME` unconditionally** (confirmed,
   issue #9148) — global-scope IDE hooks are only ever read from the literal `~/.kiro/hooks/`.
6. **No robust per-entry identity in either generation** — current schema's `name` is a
   freeform string with no confirmed uniqueness enforcement; the legacy embedded map has no
   per-hook identity field whatsoever. For idempotent third-party ownership, the safe pattern
   is **one grim-managed hook per whole file**, with the filename itself (not an in-file field)
   as the stable identity key grim owns, update, and deletes by — consistent with how hooks are
   already used in the wild (one `.kiro.hook` file per automation in every real repo example
   found).

## Sources

| URL | What it establishes | Fetched |
|---|---|---|
| https://kiro.dev/docs/hooks/ | Overview, "Agent Hooks" naming, current v1 schema shape, trigger table, IDE+CLI platform support | 2026-08-14 |
| https://kiro.dev/docs/hooks/types/ | Full current trigger catalogue incl. matcher category/prefix syntax (`read`/`write`/`shell`/`web`/`spec`/`*`, `@mcp`/`@powers`/`@builtin`) | 2026-08-14 |
| https://kiro.dev/docs/hooks/actions/ | `command` vs `agent` action types, exit-code/blocking semantics, 60s default timeout | 2026-08-14 |
| https://kiro.dev/docs/hooks/management/ | UI creation/edit/delete flows (confirms no file-path/schema info lives on this specific page) | 2026-08-14 |
| https://kiro.dev/docs/hooks/examples/ | Six documented use-cases (descriptive, not JSON) incl. security scanner, i18n helper, MCP-integrated hook | 2026-08-14 |
| https://kiro.dev/docs/cli/hooks/ | Redirect notice → confirms CLI hooks doc merged into the unified `/docs/hooks/` page | 2026-08-14 |
| https://kiro.dev/docs/cli/2x-reference/ | Legacy CLI 2.x embedded-hooks schema (`agentSpawn`/`preToolUse`/`fileEdited`/etc., camelCase, map-of-arrays) | 2026-08-14 |
| https://kiro.dev/docs/cli/v3/hooks/ | Redirect → confirms current CLI hooks doc is the migration page | 2026-08-14 |
| https://kiro.dev/docs/cli/v3/hooks-migration/ | **Verbatim old-vs-new schema comparison**, explicit "no longer supported in 3.0" breaking-change statement, `kiro-cli agent migrate` | 2026-08-14 |
| https://kiro.dev/docs/custom-agents/configuration-reference/ | Agent config schema still showing a `hooks` field; project (`<root>/.kiro/agents/`) vs global (`~/.kiro/agents/`) paths | 2026-08-14 |
| https://kiro.dev/blog/automate-your-development-workflow-with-agent-hooks/ | Original 2025-07-16 launch: file-event-only triggers, `askAgent`-only action, git-sharing quote | 2026-08-14 |
| https://kiro.dev/changelog/ide/ | Version-dated hooks entries: 1.0.0 (rewrite), 1.0.52 (auto-migrate), 1.0.116, 1.0.182 (global hooks), 1.0.309 | 2026-08-14 |
| https://kiro.dev/changelog/ide/0-9/ | v0.9 (2026-02-05) introduced Pre/Post Tool Use hook triggers | 2026-08-14 |
| https://kiro.dev/changelog/cli/2-13/ | CLI 2.13.0 (2026-07-17): global hooks (`~/.kiro/hooks/`) shipped, additive with workspace hooks | 2026-08-14 |
| https://kiro.dev/docs/privacy-and-security/ | General (non-hook-specific) command-approval stance: default-requires-approval, Trusted Commands allowlist, `*` wildcard warning | 2026-08-14 |
| https://github.com/awsdataarchitect/kiro-best-practices/blob/main/.kiro/hooks/lint-and-format-on-save.kiro.hook | Real, currently-committed legacy-shape (`when`/`then`, `askAgent`-only) hook file, full verbatim content | 2026-08-14 |
| https://github.com/kirodotdev/Kiro/issues/10320 | Closed (dup), filed 2026-07-19: confirms current CLI hook set (`AgentSpawn`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Stop`) and its status-visibility gaps | 2026-08-14 |
| https://github.com/kirodotdev/Kiro/issues/8021 | Open, filed 2026-05-01: kiro-cli 2.2.0 still using legacy `.kiro/agents/kiro_default.json` embedded hooks in production | 2026-08-14 |
| https://github.com/kirodotdev/Kiro/issues/7500 | Open, filed 2026-04-16: IDE `runCommand` hooks missing tool context; requested JSON payload shape | 2026-08-14 |
| https://github.com/kirodotdev/Kiro/issues/7375 | Closed (dup), filed 2026-04-11: **documented stdin-JSON contract vs actual `USER_PROMPT`-env-var/`{}` IDE behavior discrepancy** | 2026-08-14 |
| https://github.com/kirodotdev/Kiro/issues/7915 | Closed, filed 2026-04-28: `runCommand` hook `timeout` override confirmed broken on Windows, always enforces 60s | 2026-08-14 |
| https://github.com/kirodotdev/Kiro/issues/9148 | Closed, filed 2026-06-04: **confirms IDE ignores `KIRO_HOME`** while CLI honors it | 2026-08-14 |
| https://github.com/kirodotdev/Kiro/issues/5440 | Closed, filed 2026-02-05: original global-hooks feature request (predates the July 2026 ship) | 2026-08-14 |
| https://github.com/kirodotdev/Kiro/issues/9075 | Closed (dup), filed 2026-06-02: global-hook discovery bug, doubled `~/.kiro/.kiro/hooks/` path | 2026-08-14 |
| https://github.com/kirodotdev/Kiro/issues/9007 | Open, filed 2026-05-31: **Kiro's own "Powers" plugin system cannot deliver hooks** — direct file-write is the only working channel | 2026-08-14 |
| https://research.intezer.com/blog/2026/07/remote-code-execution-kiro/ | CVE-2026-10591 write-up (MCP config, **not hooks**) — used only as a structural security analogy, clearly labelled as such | 2026-08-14 |
| https://aws.amazon.com/security/security-bulletins/rss/2026-009-aws/ | CVE-2026-4295 (project-open RCE, <0.8.0, unrelated to hooks) | 2026-08-14 |
| https://docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/upgrade-to-kiro.html | Official AWS confirmation of the Amazon Q Developer CLI → Kiro rebrand (does not itself mention hooks/paths) | 2026-08-14 |
| https://aws.github.io/amazon-q-developer-cli/agent-format.html | **Verbatim ancestral hook schema** (`~/.aws/amazonq/cli-agents/`, embedded `hooks` map, `command`/`matcher` fields) that Kiro CLI 2.x inherited before the 3.0 rewrite | 2026-08-14 |

Not independently verified beyond WebSearch-synthesized snippets (flagged inline above where
used, treated as lower-confidence than a direct WebFetch quote): the exact `confirm` block
field list in §3; the `file_path` template-variable claim in §6.
