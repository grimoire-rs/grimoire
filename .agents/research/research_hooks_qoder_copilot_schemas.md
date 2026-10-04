# Research: Qoder and Copilot CLI hook schema primary-source verification

**Date:** 2026-09-28

**Question:** Fill four exact-schema gaps left by
[`research_hooks_upstream_2026-09.md`](./research_hooks_upstream_2026-09.md)
before PR [grimoire-rs/grimoire#98](https://github.com/grimoire-rs/grimoire/pull/98)
implementation resumes: (1) Qoder hooks JSON shape, matcher/timeout/exit-code
semantics, stdin/stdout fields, config paths, trust model, CLI-vs-IDE
differences; (2) Copilot CLI `modifiedArgs`, `SessionStart`
`additionalContext`, config shape, `version`, timeout field,
`requestSandboxBypass`; (3) Claude Code current `PostToolUse`/`Stop` response
shapes; (4) Gemini CLI trust-store filename and whether `trusted_hooks.json`
ever existed.

**Sources:** fetched 2026-09-28 via WebFetch/WebSearch against the live pages
cited inline. Fetches were summarized by an intermediate small model (the
WebFetch tool's stated behavior) — treat quoted JSON as high-confidence but
independently spot-check any field name before it drives a `serde` struct.

---

## 1. Qoder hooks — mostly new data confirmed, one real correction to the prior pass

Source: [docs.qoder.com/extensions/hooks](https://docs.qoder.com/extensions/hooks), fetched 2026-09-28.

**Registration shape — MATCH** (confirms the prior pass's summary, adds detail):

```json
{
  "hooks": {
    "EventName": [
      {
        "matcher": "match condition (optional)",
        "hooks": [
          {
            "type": "command",
            "command": "path/to/script",
            "timeout": 30,
            "async": false,
            "asyncRewake": false,
            "statusMessage": "custom description"
          }
        ]
      }
    ]
  }
}
```

Same `event -> [{matcher, hooks:[{type, command, timeout}]}]` shape as Claude.
Qoder adds three fields Claude does not document at top level: `async`,
`asyncRewake`, `statusMessage`.

**Matcher semantics — MATCH**: exact tool name, `|`-alternatives, regex, or
omit/`"*"` for all — same three-tier scheme as Claude's.

**Timeout — MATCH/new**: seconds, default **30s** per hook (the prior pass
didn't state a default).

**Handler types — DIVERGED (new)**: the page documents `command` **and**
`http` handler types, not command-only. Prior pass's implicit
command-only framing needs updating if grim's Hook artifact kind ever
targets Qoder's HTTP-handler variant.

**stdin payload fields — MATCH**, and richer than the prior pass:
`session_id`, `cwd`, `hook_event_name`, `transcript_path`, `tool_name`,
`tool_input`, plus a nested `extra` object: `extra.email`, `extra.repo`,
`extra.branch`, `extra.request_time`, `extra.response_time` (RFC3339). Also:
env vars are injected alongside stdin JSON — `QODER_SESSION_ID`,
`QODER_TOOL_NAME`, `QODER_CWD`, `QODER_TRANSCRIPT_PATH`,
`QODER_TOOL_INPUT_FILE_PATH` — a delivery mechanism not in the prior
research at all.

**PreToolUse stdout — MATCH** Claude's shape exactly:

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "allow|deny|ask",
    "permissionDecisionReason": "explanation string",
    "updatedInput": {"command": "modified command"},
    "additionalContext": "injected context"
  }
}
```

`updatedInput` for rewrite is confirmed present, same field name as Claude.

**SessionStart stdout — MATCH**: `hookSpecificOutput.additionalContext`,
same shape as PreToolUse's context field, event name `"SessionStart"`.

**Exit code semantics — MATCH the prior pass, adds detail**:

| Code | Behavior |
|---|---|
| `0` | Allow; parse stdout JSON |
| `2` | Block (blockable events only); stderr injected into the agent conversation |
| other | Non-blocking error; stderr shown to user; execution continues |

Confirmed **fail-open** on unexpected exit codes — matches the prior pass's
"same schema as Claude" framing and Claude's own fail-open model.

**Stop-loop guard — new, not in the prior pass**: stdin carries
`stop_hook_active`; a `Stop` hook script must `exit 0` when it is `true` or
it creates an infinite retry loop — same convention as Claude Code's
identically-named field.

**Config paths — MATCH, in explicit priority order** (lowest to highest):
`~/.qoder/settings.json` (user/global) → `.qoder/settings.json`
(project, shared) → `.qoder/settings.local.json` (project, gitignored).
**No `QODER_CONFIG_DIR` override is documented** on this page — the prior
pass and grim's own `AGENTS.md` env-var table both already assume
`QODER_CONFIG_DIR` works for hooks the same way it does for skills/rules;
this page does not confirm or deny that for the hooks *config file* itself
(it may still apply per grim's general vendor-config-dir convention, but
this specific page gives no citation for it — mark **UNKNOWN**, not
confirmed by this source).

> **Superseded by § 5 (live, 2026-09-28):** `QODER_CONFIG_DIR` relocates the
> user `settings.json` that holds `hooks` on `qodercli` 1.1.64.

**Trust/consent model — DIVERGED from "no model at all," and a real
correction to the prior pass**: no trust/approval/sandbox prompt before a
hook executes ("when the event fires, your script runs... no consent
prompts"), consistent with the prior pass's "no consent/trust model is
documented" framing (that part is a MATCH). But the page adds an important
scope caveat the prior pass did not have:

> "Hook capabilities differ per entry point: this page covers the Qoder IDE
> / JetBrains plugin (12 events, `command` and `http` handler only). Qoder
> CLI Hooks and QoderWork Hooks are documented separately."
>
> "The IDE and the CLI use different output structures for [PermissionRequest]
> — the CLI expects a nested `decision.behavior` object."

**This is new information not in the prior research at all**: Qoder CLI and
Qoder IDE hooks are **separately documented surfaces** with at least one
confirmed output-schema divergence (`PermissionRequest`'s `decision.behavior`
nesting on CLI vs. the IDE's flatter shape). The prior pass's "12 events... via
docs.qoder.com/extensions/hooks" citation was reading the **IDE/JetBrains**
page; a Qoder-CLI-hooks page exists separately and was not fetched by either
pass. **Flag for implementation**: if grim's Hook artifact kind targets Qoder
CLI specifically, re-fetch the CLI-specific hooks page before finalizing the
`PermissionRequest` verdict schema — this page is confirmed to not be it.

**Windows notes — UNKNOWN**, consistent with the prior pass: no Windows
guidance on this page; examples are Unix-shell only.

---

## 2 & 3. GitHub Copilot CLI hooks — closes the ADR's Open Question 1 with an exact field, one real divergence on config shape

Sources: [hooks-reference](https://docs.github.com/en/copilot/reference/hooks-reference),
[use-hooks](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/use-hooks),
both fetched 2026-09-28.

**`modifiedArgs` semantics — MATCH the prior pass's headline finding, with
the exact mechanics now confirmed**: `modifiedArgs` is a **full object
replacement** of the tool's arguments (not a merge/patch), delivered as a
JSON object (not a string). Docs phrase it as "substitute tool arguments to
use instead of the originals." **Confirmed valid only when the decision
permits execution** — i.e., it composes with `permissionDecision: "allow"`,
not with `"deny"`; the ADR's Open Question 1 (grim declined the mutator tier
pending field-name confirmation) can now cite this exact field + semantics.

**`preToolUse` output fields — MATCH/new detail**:
`permissionDecision` (`"allow"`/`"deny"`/`"ask"`), `permissionDecisionReason`
(required when denying), `modifiedArgs` (object, full replacement).

**`SessionStart`/`sessionStart` `additionalContext` — MATCH the prior pass's
"now documents an `additionalContext` field," but with a caveat this pass
adds**: the hooks-reference fetch found no output spec directly under
`sessionStart` itself; the `additionalContext` shape (`{"additionalContext":
"string"}`) is attested for **notification-type hooks fired during
`sessionStart`**, and a separate `prompt`-type hook entry auto-submits text
via a `"prompt"` string field. Net: `additionalContext` is real and
documented, but the exact hook-type/event pairing is more specific than a flat
"SessionStart outputs additionalContext" — **DIVERGED (nuance)**, not a
contradiction.

> **Superseded by § 6 (live, 2026-09-28):** on copilot 1.0.88 a PascalCase
> `SessionStart` hook's flat `additionalContext` reaches the session; the
> nested `hookSpecificOutput.additionalContext` does not.

**Config location/shape — DIVERGED from the prior pass's `.claude/settings.json`
compat framing, in a way worth flagging**: Copilot CLI's **own native**
user-level hooks live as **separate JSON files** under `~/.copilot/hooks/*.json`
(or `$COPILOT_HOME/hooks/`; Windows `%USERPROFILE%\.copilot\hooks\` /
`%COPILOT_HOME%\hooks\`) — a directory of hook-definition files, not a single
`settings.json` block. Each file requires this top-level shape:

```json
{
  "version": 1,
  "disableAllHooks": false,
  "hooks": { "eventName": [ /* entries */ ] }
}
```

`"version": 1` is **mandatory**. `disableAllHooks: true` disables that file's
hooks. This is Copilot's *native* format; the `.claude/settings.json` compat
path documented in the prior research is a separate ingestion route into the
same `hooks` event map, not the native storage location — the ADR/PR text
should distinguish "native Copilot hook file" from "Claude-settings-compat
hook file" if it doesn't already.

**Timeout field — DIVERGED (naming), confirms and sharpens the ADR's
"unverified" framing**: the field is **`timeoutSec`** (number, seconds,
default 30s), not `timeout`. A `timeout` alias exists but is **only used when
`timeoutSec` is absent**; `timeoutSec` wins when both are present.

**`requestSandboxBypass` — MATCH the prior pass's finding, mechanics
confirmed**: lives in `toolInput` during a `permissionRequest` event. Per
docs: a hook `allow` on a `requestSandboxBypass: true` request **does not
pre-approve it** — it falls through to normal interactive user confirmation
regardless of the hook's verdict. Only an explicit `deny` from the hook
takes effect at the hook layer. This is the "partial exception to no consent
step" the prior pass flagged — now backed by exact doc wording.

**Command hook shape — new, not previously captured**:

```json
{
  "type": "command",
  "bash": "string",
  "powershell": "string",
  "command": "string",
  "exec": "string",
  "args": ["array of strings"],
  "cwd": "string",
  "env": { "VAR": "VALUE" },
  "timeoutSec": 30
}
```

`bash`/`powershell` are OS-specific script fields (parallel structure to
Claude's single `command`); `exec` + `args` is a separate non-shell
invocation mode; docs say not to mix `exec` with `bash`/`powershell`/`command`.

**Event names — confirms the ADR's canonical four map cleanly**: `sessionStart`,
`sessionEnd`, `userPromptSubmitted`, `preToolUse`, `postToolUse`,
`errorOccurred` — camelCase, distinct from Claude's PascalCase (the PascalCase
*aliasing* the prior pass found is presumably for the `.claude/settings.json`
compat ingestion path, not native Copilot files, which are already camelCase).

---

## 3. Claude Code — PostToolUse/Stop shapes: UNKNOWN persists, summarizer risk flagged explicitly

> **Superseded by § 7 (live, 2026-09-28):** on Claude Code 2.1.283 both
> `PostToolUse` and `Stop` honour a top-level `decision: "block"` + `reason`.
> The summarized "no top-level decision" reading below is wrong.

Source: [code.claude.com/docs/en/hooks](https://code.claude.com/docs/en/hooks), fetched 2026-09-28.

The fetch's summary states, unambiguously:

- **PostToolUse does *not* accept a top-level `"decision"` field.** Supported
  fields are all under `hookSpecificOutput`: `hookEventName`,
  `additionalContext`, `systemMessage`, `terminalSequence`. No
  `updatedMCPToolOutput` field was reported present.
- **Stop does *not* accept a top-level `"decision": "block"` field either.**
  Stop can only block via **exit code 2**; its JSON response is limited to
  `hookSpecificOutput.{hookEventName, systemMessage, additionalContext,
  terminalSequence}`.

This **DIVERGES from the ADR's C-004 table**, which the prior pass already
flagged as UNKNOWN (ADR claims a top-level `decision:"block"`+`reason` shape
for both). This pass's fetch corroborates the prior pass's suspicion that the
ADR's shape is stale or was never accurate for these two events — but **keep
this at UNKNOWN, not DIVERGED-confirmed**, for one reason: this fetch, like
the prior one, went through WebFetch's summarization model rather than a raw
page read, and a summarizer asked "does X exist" can produce a confident "no"
from an incomplete rendering of a long docs page (this hooks page is known to
be long, with many per-event tables). **Recommendation for the plan/execute
phase**: before hard-coding grim's `Hook` response-schema types for
`PostToolUse`/`Stop`, do one raw fetch (curl the page, grep for
`"decision"` and `updatedMCPToolOutput` literal strings) rather than relying
on a second summarized pass — two summarizer passes agreeing is suggestive,
not primary-source-grade confirmation of a *negative* (absence of a field).

---

## 4. Gemini CLI trust store — MATCH, `trusted_hooks.json` confirmed never the real name

Sources: `github.com/google-gemini/gemini-cli/blob/HEAD/docs/cli/trusted-folders.md`,
`geminicli.com/docs/cli/trusted-folders/`, both surfaced 2026-09-28 via search
snippet (titles/summary only, not a full fetch of file content).

- **Trust store filename — MATCH the prior pass's correction**: confirmed
  `~/.gemini/trustedFolders.json`, referenced consistently across the
  official docs, a GitHub issue (#25032) about a restart loop keyed on that
  file, and a PR (#29423) patching trust persistence — all use
  `trustedFolders.json`, never `trusted_hooks.json`.
- **No occurrence of `trusted_hooks.json` found anywhere** in gemini-cli's
  repo, docs, issues, or PRs via search. This supports (does not
  definitively prove, since this was a search-snippet pass, not a full repo
  grep) the prior pass's conclusion that the ADRs' `trusted_hooks.json`
  citation is a paraphrase error, not a historical filename that was later
  renamed.
- **New detail beyond the prior pass**: the trust-discovery UI explicitly
  scans and lists "hooks that can intercept and modify CLI behavior" as one
  of the categories shown in the trust dialog before a user picks
  TRUST_FOLDER/TRUST_PARENT/DO_NOT_TRUST — i.e., hooks are a **named,
  surfaced** trust-discovery category, not merely covered incidentally by
  folder trust. Also: when IDE integration is active, the IDE's own trust
  answer takes priority over the local `trustedFolders.json` file — a
  precedence detail neither ADR captures.
- `GEMINI_CLI_TRUSTED_FOLDERS_PATH` override: not re-confirmed by this pass
  (not surfaced in the search snippets); treat as **carried over from the
  prior pass, not independently re-verified here**.

---

## Implications for the port

1. **Qoder CLI vs. IDE are documented on separate pages with at least one
   confirmed schema divergence** (`PermissionRequest`'s `decision.behavior`
   nesting on CLI). If the Hook artifact kind targets Qoder at all, decide
   which surface (CLI or IDE) v1 targets, and fetch the CLI-specific page —
   this research only confirms the IDE/JetBrains page's content.
2. **Copilot's `modifiedArgs` field is now fully specified** (object, full
   replacement, `allow`-only) — the ADR's Open Question 1 can close, and
   grim's `Declined` decision on the Copilot mutator tier is safe to revisit
   in a follow-up ADR/PR, not this one, per the stabilization-freeze
   discipline (`AGENTS.md` Principle 9) — don't fold a new mutator tier into
   PR #98's scope without a separate decision record.
3. **Copilot's timeout field is `timeoutSec`, not `timeout`** — if PR #98's
   schema or any code path currently emits/expects `timeout` for Copilot
   hook files, that's a bug against the real vendor shape, not a stylistic
   choice.
4. **Copilot's native user-level hook storage is a directory of per-file
   JSON documents (`~/.copilot/hooks/*.json`, each requiring `version: 1`),
   not a single settings block** — distinct from the `.claude/settings.json`
   compat-import path. If grim's install/render logic writes one merged
   Copilot hooks file today, confirm it targets the native shape (with
   `version`/`disableAllHooks`) rather than assuming settings.json parity.
5. **Claude Code's PostToolUse/Stop response-field UNKNOWN from the prior
   pass is not resolved, only reinforced** by a second summarized fetch. Before
   the schema lands in Rust types, do one non-summarized read (raw page fetch
   or `curl | grep`) for the literal strings `"decision"` and
   `"updatedMCPToolOutput"` on `code.claude.com/docs/en/hooks` — a false
   negative here would silently drop a real block-decision path for these two
   events.
6. **Gemini's `trusted_hooks.json"` naming error in both ADRs is confirmed
   wrong and should be corrected to `trustedFolders.json`** wherever the ADRs
   cite it as precedent — this is a plain text fix, not a design change, and
   safe to do in the same PR.

---

## 5. Qoder CLI — C-121 gate, live + source evidence (2026-09-28, WP-02)

**CLI:** `@qoder-ai/qodercli` **1.1.64** (npm, installed into a scratch prefix;
`qodercli --version` → `1.1.64`). Not logged in on this machine, so no model
turn ran; every probe below ends at the login check, *after* settings load.

**(a) Shape — MATCH.** `docs.qoder.com/cli/hooks-reference` (fetched
2026-09-28) shows `hooks.<Event>[{matcher, hooks:[{type:"command", command}]}]`
with optional `name`, `timeout`, `if`, `async`. The page is silent on file
locations, unknown keys and per-event output fields — (b) and (c) come from the
CLI itself.

**(b) Unknown-key tolerance — PASS.**

- *Live:* `qodercli --config-dir <tmp> -p hi` with a user `settings.json`
  whose `PreToolUse` handler carries `"com.grimoire.managed": true` printed only
  `Not logged in · Please run /login`. The control — same file with
  `"timeout": "thirty"` — printed
  `Ignored invalid setting "hooks.PreToolUse[0].hooks[0].timeout": Expected number, received string`
  before the login line. So the validator runs on this path, reports even a
  single bad field, and says nothing about the marker.
- *Live:* `QODER_CONFIG_DIR=<tmp-with-bad-file> qodercli -p hi` produced the
  same `Ignored invalid setting` line — `QODER_CONFIG_DIR` relocates the user
  `settings.json` that holds `hooks` (C-120's path).
- *Source (bundle `qoder-worker-runtime.mjs`, 1.1.64):* the settings schema's
  `HookDefinitionArray` handler item declares `name/type/command/timeout/if/
  shell/args/async/asyncRewake/rewakeMessage/rewakeSummary/statusMessage/once`
  with **no `additionalProperties: false`**, and the schema compiler
  (`UIA(A, "passthrough")`) turns such objects into `z.object(...).passthrough()`.
  The hook registry then reads `settings.user.hooks` raw; `validateHookConfig`
  checks only `type`, `command`, `args`, `url`, `prompt`, and a failing element
  is dropped **per element** (`processHookDefinition`), never the whole file —
  unlike Codex's `deny_unknown_fields`.
- *Not observed:* the marked handler actually executing (needs a login).
  The source path above registers the element unchanged, which is the property
  the marker needs.

**(c) Per-event response fields — from the CLI's own output schema**
(`hookSpecificOutput` is a `z.discriminatedUnion("hookEventName", …)`, so the
echo is **required**; top-level `decision` enum is `approve|block|allow|deny`):

| Event | `hookSpecificOutput` fields |
|---|---|
| PreToolUse | `permissionDecision` (`allow|deny|ask|defer`), `permissionDecisionReason`, `updatedInput`, `additionalContext` |
| PostToolUse | `additionalContext`, `updatedToolOutput`, `updatedMCPToolOutput` |
| SessionStart | `additionalContext` |
| Stop | `clearContext` (echo alone validates) |

**Residual, recorded 2026-09-28 (orchestrator decision D-19):** no Qoder hook
has been observed **firing** from a grim registration. The invocation-side facts
are therefore unobserved: the event and tool names (`Bash`), the stdin shape, and
that `permissionDecision: "deny"` blocks. D-19 accepts the evidence above — the
CLI's own passthrough handler schema plus the marker-versus-control diagnostic —
as the upstream statement criterion (b) asks for, because (b) guards against a
whole-file drop. A logged-in `qodercli` probe is tracked as follow-up issue 2.
The WP-07 watchlist row carries "handler invocation unobserved".

**Outcome:** C-121 passes → Qoder ships `SpliceConfig`, global only (D-2), rows
per C-122 with the mutator **declined** (no live proof the rewrite applies).
Handler types `http`, `prompt`, `agent` also exist on the CLI; grim writes
`command` only.

## 6. Copilot CLI — C-130 / C-131 live probes (2026-09-28, WP-02)

**CLI:** `copilot --version` → `GitHub Copilot CLI 1.0.88`. Isolated
`COPILOT_HOME=<scratch>` holding only `hooks/grim.json` (auth still resolved;
the user's `~/.copilot` was never touched). PascalCase event keys, the dialect
grim registers.

**C-130 — `hookSpecificOutput.updatedInput` APPLIES.** A `PreToolUse` hook (no
matcher) answered
`{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":{…,"command":"echo NONCE-updatedInput > …/nonce-updatedInput.txt"}}}`
for any call containing `ORIG`. Prompt: run `echo ORIG > orig.txt`. Result:
`nonce-updatedInput.txt` = `NONCE-updatedInput`, **no `orig.txt`**; the model
itself reported the command "was intercepted/altered … a hook rewrote it".
Stdin was Claude-shaped:
`{"hook_event_name":"PreToolUse","session_id":…,"cwd":…,"tool_name":"Bash","tool_input":{"command":"echo ORIG > orig.txt","description":…}}`.
Step 2 (`modifiedArgs`) was not needed and not run. → the copilot row keeps
`mutation: hookSpecificOutput.updatedInput`; the projector never emits
`modifiedArgs` (C-132). Contests
[github/copilot-cli#2013](https://github.com/github/copilot-cli/issues/2013)
for the PascalCase dialect on 1.0.88.

**C-131 — SessionStart context: FLAT form only.** Two runs, same prompt ("If
your context contains a session passphrase, reply with exactly that
passphrase. Otherwise reply NONE."):

| Hook output | Reply |
|---|---|
| `{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"…ZEBRA-7731."}}` | `NONE` |
| `{"additionalContext":"…OTTER-4419."}` | `OTTER-4419` |

Both hooks ran (stdin captured). → copilot SessionStart `context` =
`additionalContext`. Runtime note: only `observer` is valid at SessionStart and
`pipeline::assemble` drops observer context, so no v1 hook can reach this field
yet on any client — the row records the dialect, not a live path.

## 7. Claude Code — PostToolUse/Stop shapes and the marker (2026-09-28, WP-02)

**CLI:** `claude --version` → `2.1.283 (Claude Code)`; project-local
`.claude/settings.json` in a temp dir, `--model haiku`.

- **PostToolUse top-level `decision:"block"` + `reason` — WORKS.** The hook
  returned `{"decision":"block","reason":"Hook note: the audit word is MANGO-8812. …"}`
  after a `Bash` call; the model's next message was
  "Done. Output: `hello`. Audit word: MANGO-8812."
- **Stop top-level `decision:"block"` + `reason` — WORKS.** The hook (guarded
  by `stop_hook_active`) returned `{"decision":"block","reason":"Before
  stopping, output the line KIWI-5521 on its own."}`; the transcript shows a
  synthetic user turn `Stop hook feedback:\n…KIWI-5521…` and the final output
  `KIWI-5521`. The §3 summarizer "no top-level decision" reading was wrong;
  the shipped claude rows stand unchanged.
- **Marker — no warning, handler runs.** Both handlers above carried
  `"com.grimoire.managed": true` and executed. Interactive startup (temp
  `CLAUDE_CONFIG_DIR` with onboarding/trust pre-set, no credentials copied):
  the marker file showed the normal prompt; the control with
  `"timeout": "thirty"` showed `Settings Warning … hooks.Stop.0.hooks."command":
  Invalid command hook (timeout: Invalid input); entry ignored.` So Claude
  validates handler elements and ignores unknown keys silently.
