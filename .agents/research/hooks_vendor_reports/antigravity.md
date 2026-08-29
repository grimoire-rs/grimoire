# Antigravity (Google Antigravity IDE / Antigravity CLI) — hook / lifecycle-event research

Research date: 2026-08-14. All fetches below performed 2026-08-14 unless noted.

## 0. Executive summary

Google Antigravity **does** have a native, vendor-documented hook mechanism called, verbatim,
**"Hooks"**. It is a JSON-configured, shell-command-execution system — the closest sibling in
this research set is Claude Code's hooks, not a JS-plugin or notify-only model. It was
introduced as a headline feature of **Antigravity 2.0** (announced 2026-05-19) and has shipped,
dated, ongoing reliability fixes through **v1.1.11 / v2.6.0 (2026-08-07)** — 7 days before this
research date. It is **separate from and NOT identical to** Gemini CLI's own (older, more
mature) hooks system, despite Antigravity sharing the `~/.gemini` home directory with Gemini
CLI. There is also a **second, unrelated "hooks" concept**: an in-process Python hook framework
shipped in the official `antigravity-sdk-python` package, for developers embedding the
Antigravity agent harness in their own programs — this is a different product surface (SDK,
not IDE/CLI end-user config) and is out of scope for a grim-materialized artifact, but is
documented here to avoid confusion since Google uses the word "hooks" for both.

---

## 1. Existence & name

**Exists: yes.** Vendor's own name: **"Hooks"** (capital H used consistently in docs/blog).

- Official docs page titled "Hooks" exists at two URLs with near-identical content:
  - `https://antigravity.google/docs/hooks` (listed under the general "Antigravity 2.0" doc
    section)
  - `https://antigravity.google/docs/ide/hooks` (IDE-specific variant, same schema, same 5
    events, same example JSON)
- First public mention found: the Antigravity 2.0 announcement blog post
  (`https://antigravity.google/blog/introducing-google-antigravity-2`, published **2026-05-19**):
  > "You can now define hooks in a simple JSON format, allowing you to intercept and control the
  > Antigravity agent's behavior"
  listed alongside Dynamic Subagents and Scheduled Tasks as new Antigravity 2.0 primitives.
- The companion Google I/O 2026 deep-dive post
  (`https://antigravity.google/blog/google-io-2026-feature-deep-dive`, also **2026-05-19**)
  gives the fullest one-paragraph vendor description found anywhere:
  > "Hooks allow users to execute custom local shell scripts at critical stages of an
  > Antigravity agent's execution cycle, such as before a tool execution (can help customize
  > arguments), after a tool execution (useful for logging), before a model call (to inject
  > system instructions), after a model call (to override exit rules), or at agent loop stopping
  > conditions (to force checks or block termination)."
- **Stability label:** neither doc page nor either blog post applies an explicit "beta" /
  "experimental" / "GA" label to Hooks specifically. The surrounding product framing is not
  itself GA either: Wikipedia (`https://en.wikipedia.org/wiki/Google_Antigravity`, fetched
  2026-08-14) still lists Antigravity's license as **"Proprietary (free during preview)"** as of
  its latest tracked stable release **2.1.1 (2026-06-22)** — i.e. the whole product line,
  Hooks included, is best classified as **shipped-but-still-preview**, not a flagged
  experimental sub-feature within a GA product. `antigravity.google/docs/hooks` itself:
  **NOT DOCUMENTED** (no stability tag on the page).
- **Since which version:** blog-level "Antigravity 2.0" (2026-05-19) is the feature
  introduction. The separately-versioned **Antigravity CLI** (own version track, e.g. v1.1.x,
  distinct from the "Antigravity 2.x" IDE/product versioning) shows its earliest hooks-related
  changelog entry at **v1.0.8 (2026-06-12)**: "Fixed a bug where the `/hooks` command wrote
  configurations [to an incorrect directory]" (`https://antigravity.google/changelog`, fetched
  2026-08-14) — proving Hooks (and an interactive `/hooks` configuration command) already
  existed by June 12, roughly 3 weeks after the 2.0 announcement.
- **Deprecation:** none found. The feature is under active, dated iteration — see §8.
- **Current versions at research date (2026-08-14):** Antigravity CLI v1.1.13 (per
  `https://github.com/google-antigravity/antigravity-cli/releases`, "released 14 Aug" — i.e.
  today); Antigravity 2.x product line at v2.6.0 per the changelog (2026-08-07 entry).

---

## 2. Config location(s)

Google's own docs page states the general location only as a **directory**, not a fully
spelled-out filename, in this exact sentence (extracted verbatim from
`https://antigravity.google/docs/hooks` via targeted fetch, 2026-08-14):

> hooks are "located in your customization directory (e.g., `.agents/` in your workspace or
> `~/.gemini/config/`)"

That is the single most precise verbatim location sentence obtainable from the vendor page
itself — **the vendor page never spells out the literal filename `hooks.json` in one sentence**
(confirmed by a targeted re-fetch that explicitly asked for the exact path string and returned
"NOT ON PAGE" for the fully-qualified filename). The filename `hooks.json` is, however:

- shown as the implicit artifact throughout every JSON example on the same page (the examples
  are captioned "Schema and File Format" and are unambiguously the contents of a file the docs
  call `hooks.json` elsewhere in prose, per repeated independent fetches of the same page), and
- corroborated by **three independent, convergent sources**:
  1. `[unofficial]` Mete Atamel, "Where does Antigravity look for Hooks?"
     (`https://atamel.dev/posts/2026/07-16_where_agy_hooks/`, published 2026-07-16, last
     modified 2026-08-13 — i.e. updated yesterday): states plainly "Workspace-level: `.agents/
     hooks.json`" and "Global-level: `~/.gemini/config/hooks.json`", explicitly noting the
     global path applies "across all AGY flavors (AGY, AGY CLI, AGY IDE)".
  2. `[unofficial, third-party bug tracker]` a `manaflow-ai/cmux` GitHub issue thread (found via
     search, title referencing hook removal) states hooks need to be "removed from
     `~/.gemini/config/hooks.json`" with a caveat that the third-party tool `cmux` "re-installs
     hooks on upgrade" — an independent real-world confirmation of the exact same global path
     string.
  3. `[unofficial]` a third-party Antigravity CLI plugin repo, `ChernegaSergiy/antigravity-hooks`
     (`https://github.com/ChernegaSergiy/antigravity-hooks`), refers to `hooks.json` as "the
     configuration file that maps [hooks] to event handlers" and to a `/hooks` menu as the
     interactive alternative to hand-editing it.

**Working conclusion (convergent evidence, not a single verbatim vendor sentence):**
- Project/workspace scope: **`.agents/hooks.json`** (relative to workspace/git root — the same
  `.agents/` directory Antigravity uses for rules; see §rules-workflows note below).
- Global/user scope: **`~/.gemini/config/hooks.json`**.
- No env var to relocate either path was found anywhere (vendor docs, blog, changelog, or the
  unofficial sources). **NOT DOCUMENTED.**

**Format:** JSON (`.json` extension, no JSONC/TOML/YAML variant mentioned anywhere).

**Directory-convention auto-discovery:** the two fixed filenames above are the *only* discovery
mechanism found — no evidence of a `hooks/*.json` multi-file directory convention (contrast
Claude Code's single `settings.json` "hooks" key vs. e.g. a hypothetical hooks.d/ directory;
Antigravity does not appear to support the latter). **NOT DOCUMENTED** beyond the two files.

**Merge vs. override when both exist:** the vendor hooks page does not state this
(targeted re-fetch returned "NOT ON PAGE" for this exact question). Indirect evidence points
both ways and is **unresolved**:
- The WebSearch summary of an unindexed page fragment claimed: "Antigravity now supports both
  global hooks and workspace-specific hooks, **with the latter taking precedence if it
  exists**" — this reads like an override/precedence model, not a merge.
- `[unofficial]` the Medium walkthrough (§ Sources, Tanaike) instead claims "Both scopes
  execute; veto power applies if any hook denies" — i.e. a merge model where both run and a
  Decide-hook veto from either scope can block.
These two claims are **mutually inconsistent** and neither is a verbatim vendor quote captured
in this research. Classify as: **NOT DOCUMENTED with conflicting secondary claims** — do not
build the portable schema assuming either behavior without a live test against a real
Antigravity install.

---

## 3. Config schema — verbatim

**The hook collection is a NAMED MAP, keyed by a user-chosen hook name — NOT a bare array, and
NOT a bare `{"hooks": {...}}` wrapper either** (this is a real, load-bearing difference from
Claude Code's `{"hooks": {"<Event>": [...]}, ...}` shape, and from Gemini CLI's own
`{"hooks": {"<Event>": [...]}}` shape under `settings.json` — see §lineage). In Antigravity, the
**hook's own name is the top-level key**, and each named block can itself register handlers for
one or more events. Exact JSON reproduced character-for-character from
`https://antigravity.google/docs/hooks` (verified via a second, independent targeted fetch that
was instructed to reproduce code blocks with zero paraphrasing):

```json
{
  "my-linter-hook": {
    "PostToolUse": [
      {
        "matcher": "run_command",
        "hooks": [
          {
            "type": "command",
            "command": "./scripts/lint.sh",
            "timeout": 10
          }
        ]
      }
    ]
  },
  "safety-gate": {
    "enabled": false,
    "PreToolUse": [
      {
        "matcher": "run_command",
        "hooks": [
          {
            "command": "./scripts/safety-check.sh"
          }
        ]
      }
    ]
  },
  "reminder": {
    "PreInvocation": [
      {
        "type": "command",
        "command": "./scripts/reminder.sh"
      }
    ]
  }
}
```

Notable shape details directly visible in this example:
- Each named entry (`my-linter-hook`, `safety-gate`, `reminder`) is an object whose keys are
  **either** an `"enabled"` boolean **or** one of the 5 event names (§4). `"enabled": false` on
  `safety-gate` demonstrates a **per-named-hook kill switch** — this is a real, load-bearing
  identity/lifecycle field: an installer can add `"enabled": false` to disable a hook it owns
  without deleting the entry.
- Under each event name is an **array** of matcher groups: `[{"matcher": "...", "hooks": [...]}]`
  — so within one event, multiple matcher groups can exist, and each matcher group fans out to
  its own array of handler objects (`hooks: [...]`) — i.e. **two levels of array nesting** below
  the event key (matcher-groups array → handlers array), mirroring Claude Code's shape almost
  exactly at this inner level, but wrapped in the outer named-map that Claude Code does not have.
- Handler object fields seen: `"type"` (only value ever shown: `"command"`), `"command"`
  (a shell command **string**, e.g. `"./scripts/lint.sh"` — relative paths are used throughout
  every vendor example), `"timeout"` (integer seconds, optional — `safety-gate`'s handler omits
  it entirely, `my-linter-hook`'s sets `10`).
- `"type"` is **omittable** — `safety-gate`'s handler has no `"type"` key at all, implying
  `"command"` is the default when absent. **NOT EXPLICITLY DOCUMENTED** that this is a formal
  default vs. simply the only type that has ever existed; treat as "only one type exists today."

**Stable identity for a third-party installer:** the **top-level map key itself** (e.g.
`"my-linter-hook"`) is the natural, and only, candidate for a stable id — there is no separate
`id`/`name`/`description` field nested inside an entry. A targeted re-fetch of the vendor page
asking specifically "is there a sentence about using the top-level key as a stable id for
updating/removing via a CLI command" returned **NOT ON PAGE** — the docs never states this
explicitly as a contract, it is an inference from the example naming convention plus the
existence of an `/hooks` interactive command (§10) that presumably lists/edits by that same key.
No `description` field was seen anywhere.

**Matcher syntax:** confirmed only for `PreToolUse`/`PostToolUse`. Verbatim vendor sentence:
"For `PreToolUse` and `PostToolUse` matchers, you can match against standard tool names" with
patterns including the literal empty string `""`, the wildcard `"*"`, plain tool names like
`"run_command"`, and **regex** such as `"browser_.*"`. A third-party plugin README
(`ChernegaSergiy/antigravity-hooks`, `[unofficial]`) independently shows alternation syntax
`"run_command|write_to_file"` and `".*_file.*"`, consistent with a regex (not glob) engine.
The vendor page states the `matcher` field **is ignored** for `PreInvocation`, `PostInvocation`,
and `Stop` (those fire unconditionally once registered under the event key, with no filtering).

---

## 4. Event catalogue

The vendor hooks page (both `/docs/hooks` and `/docs/ide/hooks`) documents **exactly 5 events**,
verbatim names and firing descriptions, confirmed by two independent targeted fetches that were
explicitly asked to check for and rule out any additional events (SessionStart, SessionEnd,
UserPromptSubmit, Notification, Compaction, SubagentStop, Error — **none of these appear** on
either Antigravity hooks page):

| Event (verbatim) | Fires | Group |
|---|---|---|
| `PreToolUse` | "before a tool is executed" | pre/post tool use |
| `PostToolUse` | "after a tool completes" | pre/post tool use |
| `PreInvocation` | "before the model is called" | prompt/model call |
| `PostInvocation` | "immediately after each model invocation completes" (one page) / "after tool calls finish" (the IDE-variant page's paraphrase) | prompt/model call |
| `Stop` | "when execution terminates" / "when the execution loop terminates" | stop/finish |

No session-lifecycle (start/end), no compaction, no subagent-specific, no notification, and no
generic error event exists in Antigravity's own hooks taxonomy — a materially **smaller**
catalogue than either Claude Code's or Gemini CLI's own (§lineage note below has Gemini CLI's
11-event list for contrast). If grim's portable schema needs `SessionStart`/`Compaction`/
`Notification`-class events, **Antigravity currently has no native slot for them** — this is a
real coverage gap, not a naming difference.

The changelog (`https://antigravity.google/changelog`, fetched 2026-08-14) independently
confirms these exact event name strings are the ones the implementation actually dispatches, via
dated bug-fix entries that name them literally: *"Fixed `PostToolUse` hooks firing on non-tool
steps such as user input and model responses"* (v1.1.9, 2026-07-31); *"lets `PostInvocation`
hooks observe the final invocation of a turn and lets `Stop` hooks run at all"* (v1.1.10,
2026-08-03).

---

## 5. Invocation

- **Mechanism:** shell command execution. The only handler `"type"` ever documented or observed
  is `"command"`; the `"command"` value is a **string** (e.g. `"./scripts/lint.sh"`), not an
  argv array — implying it is passed to a shell for interpretation (supporting relative paths,
  presumably resolved from some working directory — see below) rather than exec'd directly.
- **Working directory:** **NOT DOCUMENTED.** Every vendor example uses a bare relative path
  (`./scripts/lint.sh`) with no sentence anywhere stating what that path is relative to (repo
  root? `.agents/`? the directory containing `hooks.json`? the process's own cwd?). A targeted
  fetch asking specifically for this returned "NOT ON PAGE."
- **Shell used:** **NOT DOCUMENTED** (bash? sh? platform-dependent cmd.exe on Windows, given
  Antigravity ships on Windows too?). No mention found in vendor docs, blog, or changelog.
- **`$PATH` handling:** **NOT DOCUMENTED.**
- **Timeouts:** a per-handler `"timeout"` integer field exists (seen as `10` in the vendor
  example, unit implied to be seconds by every secondary source, though the vendor page itself
  never states the unit in prose in any fetch performed). Default when omitted: **implied `30`**
  by two independent secondary-source paraphrases of the vendor page (not a verbatim vendor
  quote captured directly) — treat the exact default value with **medium confidence** only.
  Reliability: the changelog's v2.6.0 (2026-08-07) entry, *"Hooks that call a model now stop at
  their configured timeout with a clear error, instead of stalling the agent indefinitely,"*
  confirms timeout enforcement is real and was previously broken for at least one hook subtype
  (hooks that themselves call a model) until this fix — i.e. timeout enforcement has a dated
  history of being unreliable in production as recently as 7 days before this research date.
- **Concurrency / ordering when multiple hooks match one event:** **NOT DOCUMENTED** on the
  vendor hooks page (explicitly checked and returned "NOT ON PAGE"). The v1.1.10 changelog entry
  ("improved hook ordering so hooks defined in `hooks.json` run before the built-in termination
  checks") proves *some* ordering rules exist internally (hook-vs-built-in-check ordering) but
  says nothing about hook-vs-hook ordering when several matcher groups or several named hooks
  target the same event.
- **Blocking vs. fire-and-forget:** Decide-category behavior (§7) is explicitly blocking — the
  agent loop waits for the `decision` field before proceeding on `PreToolUse` and `Stop`. Whether
  Inspect-style hooks (e.g. a `PostToolUse` hook whose output is discarded) block the loop while
  running, or run fire-and-forget in the background, is **NOT DOCUMENTED**.

---

## 6. Input payload — verbatim

**All hooks receive a JSON object on stdin.** Every event's payload was captured verbatim from
the vendor docs page via a targeted fetch instructed to reproduce code blocks exactly:

Common/base fields present in every event's payload: `conversationId` (string, UUID),
`workspacePaths` (array of absolute path strings), `transcriptPath` (absolute path string),
`artifactDirectoryPath` (absolute path string), `modelName` (string).

`PreToolUse` stdin, verbatim:
```json
{
  "toolCall": {
    "name": "run_command",
    "args": {
      "CommandLine": "npm test",
      "Cwd": "/workspace/project",
      "WaitMsBeforeAsync": 5000
    }
  },
  "stepIdx": 19,
  "conversationId": "ec33ebf9-0cba-4100-8142-c61503f6c587",
  "workspacePaths": ["/workspace/project"],
  "transcriptPath": "~/.gemini/antigravity/brain/ec33ebf9-0cba-4100-8142-c61503f6c587/.system_generated/logs/transcript.jsonl",
  "artifactDirectoryPath": "~/.gemini/antigravity/brain/ec33ebf9-0cba-4100-8142-c61503f6c587",
  "modelName": "gemini-3.6-flash-medium"
}
```

`PostToolUse` stdin adds an `error` field (string, present in the failing-case example, e.g.
`"exit status 1"`) alongside the same `toolCall`/`stepIdx`/common fields.

`PreInvocation` / `PostInvocation` stdin, verbatim shape: `invocationNum` (integer),
`initialNumSteps` (integer), plus the common fields (no `toolCall`).

`Stop` stdin, verbatim shape: `executionNum` (integer), `terminationReason` (string, e.g.
`"model_stop"`), `error` (string, empty when absent), `fullyIdle` (boolean), plus common fields.

Note the `transcriptPath`/`artifactDirectoryPath` example values above reveal the **runtime data
root** is `~/.gemini/antigravity/brain/<conversationId>/` — a different subtree from the
**config** root `~/.gemini/config/` (§2). Do not conflate the two when building a client
descriptor.

**Env vars / argv / template interpolation:** no evidence of any of these three alternate
delivery mechanisms anywhere in the vendor docs, blog, or changelog — stdin JSON is the only
documented delivery path. **NOT DOCUMENTED** whether any env vars are *additionally* set (e.g.
an Antigravity-equivalent of `$CLAUDE_TOOL_NAME`) — the docs describe only the stdin JSON.

---

## 7. Output / response contract — verbatim

**Hooks return JSON on stdout.** The exact response shape is event-specific (unlike the shared
input envelope), captured verbatim from the vendor docs:

`PreToolUse` response — the load-bearing `decision` field takes exactly these 5 values, quoted
verbatim: `"allow"` ("Automatically allows the tool execution"), `"deny"` ("Hard blocks execution
immediately"), `"ask"` ("Prompts the user, but respects 'Always Allow' settings"), `"force_ask"`
("Always prompts the user, ignoring cached permissions"), `"deny_unless_prior_grant"` ("Denies
execution unless the resource was previously approved"). Optional fields: `reason` (string),
`permissionOverrides` (array of strings, e.g. `["command(npm test)"]`). Full verbatim example:

```json
{
  "decision": "ask",
  "reason": "Requires confirmation for test execution.",
  "permissionOverrides": ["command(npm test)"]
}
```

`PostToolUse` response: **must be an empty JSON object, `{}`** — i.e. this event is
Inspect-only; there is no documented field that lets a `PostToolUse` hook alter anything.

`PreInvocation` response: optional `injectSteps` (array of step objects) — verbatim example:
```json
{ "injectSteps": [{ "ephemeralMessage": "Remember to lint" }] }
```
This is the mechanism matching the blog's "before a model call (to inject system instructions)"
description in §1.

`PostInvocation` response: `injectSteps` (array, can be empty) plus `terminationBehavior`, one of
`"force_continue"`, `"terminate"`, or `""` (documented default/no-op) — verbatim example:
```json
{ "injectSteps": [], "terminationBehavior": "" }
```

`Stop` response: `decision` field — verbatim example `{"decision": "continue", "reason": "Not
done yet"}` — any value other than `"continue"` allows termination to proceed. This is the
"agent loop stopping conditions (to force checks or block termination)" hook from the blog
description.

**Exit-code semantics for Antigravity specifically: NOT DOCUMENTED.** A targeted, explicit
re-fetch of the vendor hooks page asking only about exit codes 0/1/2/other returned "NOT ON
PAGE." This is a real, confirmed gap in Google's own Antigravity documentation.

**Conflicting unofficial claim (flagged, not trusted):** `[unofficial]` a Medium walkthrough
(Kanshi Tanaike, "A Developer's Guide to Agent Hooks in Antigravity CLI," 2026-06-26) asserts a
**different, boolean-shaped** response schema — `{"allow_tool": false, "deny_reason": "..."}` /
`{"allow_tool": true}` — and claims "Scripts must always return exit code 0, even when denying
operations." **This directly contradicts** the vendor's own `decision`-enum JSON example
reproduced above (character-for-character, from two independent fetches of the vendor page
itself). Since the vendor's own docs page is a primary source and shows `decision`/`"ask"`/
`"deny"` in an explicit code example, the `decision`-enum shape is treated as authoritative here
and the Medium post's `allow_tool`/`deny_reason` shape is treated as **either stale, describing
an unreleased/earlier iteration, or simply incorrect** — do not build the portable schema on it.
The same Medium post also states the global path as `~/.gemini/antigravity-cli/hooks.json`,
which conflicts with the `~/.gemini/config/hooks.json` path corroborated by three other
independent sources in §2 — a second, independent reason to discount this particular post's
specifics while still logging it as a "lead."

**stderr:** **NOT DOCUMENTED** for Antigravity specifically (contrast Gemini CLI, §lineage,
which explicitly reserves stderr "for logs and feedback"). Whether stdout/hook output is shown
to the user, the model, both, or neither: **NOT DOCUMENTED** anywhere found.

**Malformed output / missing binary:** **NOT DOCUMENTED** in prose, but changelog v2.6.0
(2026-08-07) — *"Hook configurations that could never run are now rejected at load time with a
clear error instead of being silently ignored"* — proves that before 2026-08-07, at least one
class of bad hook configuration (one that "could never run") was **silently ignored** rather
than erroring, and this was only just fixed 7 days before this research date. A third-party
integration's GitHub issue (`manaflow-ai/cmux` #5358, `[unofficial, third-party bug report]`,
title: *"Antigravity (agy): injected PreToolUse hook denies every tool call (invalid_args) — agy
unusable in cmux"*) is real-world evidence that a malformed/misbehaving `PreToolUse` Decide-hook
can brick the agent entirely (every tool call denied) rather than being caught and degraded
gracefully.

---

## 8. Reliability & limits

- **Timeout default:** implied **30 seconds** per handler (medium confidence — see §5; not a
  direct verbatim vendor quote).
- **Non-zero exit / malformed output:** exit-code meaning **NOT DOCUMENTED** for Antigravity
  (§7). Pre-2026-08-07 behavior for at least one bad-config class was silent-ignore (now fixed to
  a load-time rejection error per the v2.6.0 changelog entry quoted in §7).
- **Missing binary:** **NOT DOCUMENTED.**
- **Parallel vs. sequential execution:** **NOT DOCUMENTED** (§5).
- **Dated reliability history found in the official changelog** (`antigravity.google/changelog`,
  fetched 2026-08-14), all direct quotes:
  - v1.0.8 (2026-06-12): "Fixed a bug where the `/hooks` command wrote configurations" to the
    wrong directory — an early config-location bug.
  - v1.1.7 (2026-07-24): "Fixed disabled plugins still running their hooks and contributing
    other customizations" — confirms hooks can also be delivered/bundled via "plugins" (a
    related but distinct Antigravity surface — see `/docs/plugins` in §sources — and that a
    plugin's enabled/disabled state used to be a no-op for suppressing its hooks).
  - v1.1.9 (2026-07-31): "Fixed stop hooks that always block hanging the agent forever; after a
    configurable number of consecutive continuations, the hook can no longer block and the turn
    ends normally." — direct proof that a `Stop` hook returning `decision: "continue"`
    unconditionally used to be able to **hang the agent forever**, and the fix is a hard cap on
    consecutive continuations rather than a user-configurable timeout. Also same version: "Fixed
    `PostToolUse` hooks firing on non-tool steps such as user input and model responses."
  - v1.1.10 (2026-08-03): "Improved hook ordering so hooks defined in `hooks.json` run before the
    built-in termination checks, which lets `PostInvocation` hooks observe the final invocation
    of a turn and lets `Stop` hooks run at all instead of sitting unreachable behind the
    built-ins." — proves `Stop` hooks could previously be entirely unreachable in some turns.
  - v2.6.0 (2026-08-07): the load-time-rejection and model-call-timeout fixes quoted in §7.
- **Overall read:** this is a real but young and still-hardening mechanism — five distinct,
  dated reliability bugs (hang-forever, unreachable hooks, silent-ignore of bad config, timeout
  not enforced, disabled plugin hooks still firing) were fixed in the ~2 months before this
  research date, which is evidence of active vendor investment but also of a feature that has
  not yet fully stabilized operationally.

---

## 9. Security posture

**No hook-specific "arbitrary code execution" warning was found anywhere in Antigravity's own
docs, blog posts, or changelog** — this was checked explicitly via a targeted re-fetch of the
vendor hooks page asking only about a security/trust disclaimer, which returned "NOT ON PAGE."
`https://antigravity.google/docs/permissions` (fetched 2026-08-14) documents a general
tool-permission model (`Deny > Ask > Allow` precedence; "Ask: The Agent pauses and prompts for
your explicit approval before proceeding"; web browsing defaults to Ask) but its examples are
about ordinary agent tool calls (`command(*)`, `read_url`), not specifically about installing or
running a hooks.json entry, and it does not mention hooks at all.

This is a **notable, confirmed documentation gap**, especially by contrast: Gemini CLI — a
sibling Google product sharing Antigravity's `~/.gemini` home directory — has an explicit,
strongly-worded warning on its own hooks docs (`https://geminicli.com/docs/hooks/`, fetched
2026-08-14):
> "Hooks execute arbitrary code with your user privileges. By configuring hooks, you are
> allowing scripts to run shell commands on your machine."
plus a **fingerprinting-based trust mechanism**: "if a hook's name or command changes (such as
through a git update), it's treated as untrusted and requires fresh approval." Antigravity's own
hooks page has **no equivalent sentence and no equivalent fingerprinting mechanism documented**
— whether Antigravity silently inherited Gemini CLI's trust/fingerprint behavior at the
implementation level, or dropped it, is **NOT DOCUMENTED** and unverifiable from docs alone.

The one Antigravity-specific approval mechanism found is generic and reactive, not
hook-specific: the `PreToolUse` response's `"ask"`/`"force_ask"` decisions (§7) surface "an
interactive card" in the editor per `/docs/permissions`, but that is the *hook itself choosing*
to ask the user about the underlying tool call — it says nothing about whether the *hook script
itself* was vetted/trusted before Antigravity ran it in the first place.

No evidence of config-snapshotting-at-session-start was found for Antigravity specifically
(the SDK's unrelated in-process "Decide hooks... halting until the developer reviews and
approves the request" is a different, developer-facing mechanism — §0, §11 — not a session-start
snapshot of hooks.json).

---

## 10. Third-party installability

**Yes, realistically installable by editing files** — hooks.json is a plain JSON file at a
fixed, predictable path per scope (§2), with no evidence of any server-side, cloud-only, or
proprietary binary format gate. This matches the general pattern grim already uses for other
clients' native JSON configs.

Two caveats specific to Antigravity:

1. **A first-party, interactive `/hooks` command/menu exists** as the "normal" way to configure
   hooks (referenced in the v1.0.8 changelog entry, §1, and independently in the third-party
   `ChernegaSergiy/antigravity-hooks` README: *"If you previously configured these hooks manually
   via the `/hooks` menu, please remove those manual entries..."*). This raises the same class of
   "two writers, one file" risk noted for other clients in this research set: a user editing
   hooks.json through the `/hooks` UI while grim also owns entries in the same file needs the
   named-map key (§3) to be a stable, collision-safe id grim can claim without stepping on
   UI-authored entries.
2. **Plugins are a separate, overlapping delivery path** for hooks (§8, v1.1.7 changelog entry;
   also `ChernegaSergiy/antigravity-hooks` installs via `agy plugin install` rather than by
   hand-editing hooks.json, per its own README, though its README does not confirm whether the
   plugin installer edits hooks.json under the hood or uses an entirely separate registration
   path). This is a second, undocumented-in-detail avenue by which hook-like behavior can arrive
   on a system, alongside direct hooks.json edits.

**Restart / snapshot gotcha:** **NOT DOCUMENTED** whether hooks.json is read fresh per-event,
per-session, or snapshotted at Antigravity startup (no equivalent of Claude Code's documented
startup-snapshot behavior was found for Antigravity in any source checked). Given the `/hooks`
command is described as an in-app configuration path (implying live reads/writes during a
running session) it is plausible edits take effect without a full restart, but this is an
inference, not a documented fact.

---

## 11. Trampoline viability

A generic `grim hook run --client antigravity --event <E>` trampoline looks **plausible** for
the `PreToolUse`/`PostToolUse`/`PreInvocation`/`PostInvocation`/`Stop` event set, because:

- The handler is a **shell command string** (`"type": "command"`, `"command": "..."`) — grim can
  write `"command": "grim hook run --client antigravity --event PreToolUse"` directly into the
  named-map entry it owns, with no need for a JS/TS module or in-process function.
- Input arrives as **JSON on stdin** uniformly across all 5 events (§6) — a single trampoline
  binary can read stdin, branch on which fields are present (`toolCall` implies Pre/PostToolUse,
  `invocationNum` implies Pre/PostInvocation, `executionNum` implies Stop) or simply trust the
  `--event` argv flag it was invoked with (since grim itself wrote that flag into the command
  string at install time — it always knows statically which event a given entry is for).
- Output is **JSON on stdout**, with a per-event schema grim's trampoline would need to shape
  correctly (§7) — this is a real complexity (5 distinct output shapes, not 1 shared shape) but
  fully mechanical and known.
- The named-map wrapper (§3) gives grim a **natural stable identity** — its own top-level key —
  to own, update, and idempotently remove one entry, plus a free `"enabled": false` kill switch,
  without needing to invent an id scheme of its own.

**Named blockers found:**
- **Concurrency/ordering across multiple matching hooks is undocumented** (§5) — if grim ever
  needs to guarantee its trampoline runs before/after some other hook on the same event, there is
  currently no documented lever for that.
- **Working directory, shell, and `$PATH` are all undocumented** (§5) — grim's materializer
  cannot currently assume a specific cwd for relative-path commands with certainty; using an
  absolute path to the `grim` binary in the generated `"command"` string sidesteps this but
  should be treated as a defensive necessity, not a documented guarantee.
- **Exit-code semantics are undocumented for Antigravity** (§7) — unlike Gemini CLI's clearly
  specified 0/2/other contract, grim cannot rely on any particular exit code meaning something
  specific to Antigravity; the JSON on stdout is the only confirmed signal channel.
- **Two competing config-owners** (`/hooks` UI and, separately, `plugin install`, §10) mean a
  purely-file-editing trampoline installer could be silently overwritten or duplicated by a user
  or plugin acting through either UI path — this is a real but not fatal blocker (same class of
  risk exists for other clients' native JSON configs).
- **The exact global path (`~/.gemini/config/hooks.json`) is not a direct verbatim vendor
  quote** (§2) — it is corroborated by three convergent independent sources but grim should
  verify this against a live install before hard-coding it, given the documented June 2026
  history of Google's own `/hooks` command once writing to the wrong directory.

None of the blockers found are of the fatal kind seen elsewhere in this research set (e.g. "must
be an in-process JS function," "no stdin," "cloud-only UI"). This looks like one of the more
directly trampoline-able clients researched.

---

## Lineage note — relationship to Gemini CLI (answers "does it inherit Gemini CLI's config
surface?")

Antigravity **shares the `~/.gemini` home directory root** with Gemini CLI (confirmed by the
`transcriptPath`/`artifactDirectoryPath` values in §6 living under `~/.gemini/antigravity/...`,
and by the config root `~/.gemini/config/` in §2), and Antigravity CLI is described by Google's
own docs as succeeding Gemini CLI outright:
`https://antigravity.google/docs/cli/overview` (fetched 2026-08-14), verbatim: "the onboarding
process supports a one-time import to automatically migrate your existing Gemini CLI extensions,
skills, and settings."

**However, the Hooks mechanism itself is NOT the same mechanism, and NOT the same file:**

| | Gemini CLI (`https://geminicli.com/docs/hooks/`, `/reference/`) | Antigravity (`antigravity.google/docs/hooks`) |
|---|---|---|
| Config file | `settings.json` (`.gemini/settings.json` project, `~/.gemini/settings.json` user, `/etc/gemini-cli/settings.json` system) under a `"hooks"` key | separate file **`hooks.json`** (`.agents/hooks.json` project, `~/.gemini/config/hooks.json` global) |
| Top-level shape | `{"hooks": {"<Event>": [...]}}` | named map: `{"<hook-name>": {"<Event>": [...]}}` |
| Event names | `BeforeTool`, `AfterTool`, `BeforeAgent`, `AfterAgent`, `BeforeModel`, `BeforeToolSelection`, `AfterModel`, `SessionStart`, `SessionEnd`, `Notification`, `PreCompress` (11 total) | `PreToolUse`, `PostToolUse`, `PreInvocation`, `PostInvocation`, `Stop` (5 total) |
| `decision` values | `"allow"` / `"deny"` (alias `"block"`) | `"allow"` / `"deny"` / `"ask"` / `"force_ask"` / `"deny_unless_prior_grant"` |
| Exit codes | documented: `0`=success/parse stdout, `2`=system block (stderr=reason), other=non-fatal warning | **not documented** |
| Multi-scope behavior | explicitly **merges**, cascading priority project > user > system > extensions | **not documented** (conflicting secondary claims, §2) |
| Security warning | explicit: "Hooks execute arbitrary code with your user privileges..." + fingerprint-based re-approval on change | **none found** |
| Ordering control | explicit per-group `"sequential"` boolean | **not documented** |

Antigravity's event names (`PreToolUse`/`PostToolUse`) actually read as closer to **Claude
Code's** naming convention than to its own sibling Gemini CLI's (`BeforeTool`/`AfterTool`) — so
"inherits Gemini CLI's config surface" is **true only at the home-directory level** (`~/.gemini`
as shared root, and the migration-on-onboarding of "extensions, skills, and settings") and
**false at the hooks-schema level** — Antigravity built a parallel, differently-named,
differently-shaped, differently-pathed hook system rather than reusing Gemini CLI's
`settings.json` `"hooks"` key. Any grim schema design should **not** assume Gemini-CLI-observed
behaviors (merge semantics, exit codes, security fingerprinting) transfer to Antigravity without
independent verification, despite the shared home directory being real and confirmed.

---

## Adjacent, NOT hooks (disambiguation, per brief's scope boundary)

- **Rules** (`https://antigravity.google/docs/rules-workflows`, fetched 2026-08-14): plain
  Markdown files, global at `~/.gemini/GEMINI.md`, workspace at `.agents/rules/`, 12,000-char
  cap, four activation modes (Manual/@mention, Always On, Model Decision, Glob). Prompt-level
  guidance, not executable code — explicitly NOT a hook.
- **Workflows** (same page): also Markdown, invoked via slash commands (`/workflow-name`),
  described as "a structured sequence of steps or prompts at the trajectory level" — a scripted
  *prompt* sequence, not a lifecycle-event-triggered *code* execution — NOT a hook, and the page
  explicitly does not mention hooks in relation to workflows.
- **Skills**, **MCP**, **Plugins**, **Subagents**, **Scheduled Tasks** (cron-triggered agent
  invocations) all exist as separate documented surfaces (`/docs/skills`, `/docs/mcp`,
  `/docs/plugins`, `/docs/subagents`) per the docs-home nav (§sources) but were not the target of
  this research beyond the one plugins/hooks interaction noted in §8/§10.
- **The `antigravity-sdk-python` in-process hook framework** (§0): a genuinely different,
  vendor-named "hooks" system — Python decorator/class-based (`@post_tool_call`, `Hook` base
  classes, `HookContext`/`SessionContext`/`TurnContext`/`OperationContext` hierarchy, a
  declarative `policy` DSL like `policy.deny('run_command', when=lambda args: ...)`) — for
  developers building their *own* agent programs on top of the Antigravity harness via the SDK.
  This is **not** the IDE/CLI end-user hooks.json mechanism and is **out of scope** for a
  grim-materialized artifact aimed at IDE/CLI users, but is real, is also called "Hooks" by
  Google, and should not be confused with §1–§11 above. Source:
  `https://github.com/google-antigravity/antigravity-sdk-python/blob/main/google/antigravity/hooks/README.md`
  (fetched 2026-08-14) and `https://antigravity.google/blog/introducing-google-antigravity-sdk`
  (published 2026-05-19, status: "Research Preview").

---

## Sources

| URL | What it establishes | Fetched |
|---|---|---|
| https://antigravity.google/docs/hooks | Primary hooks doc: schema, verbatim JSON example (named map), 5 events, input/output payloads per event, matcher syntax; confirmed absence of docs on exit codes, cwd/shell/PATH, concurrency, security warning, merge-vs-override, stable-id contract | 2026-08-14 (fetched 4x with different targeted prompts) |
| https://antigravity.google/docs/ide/hooks | IDE-specific variant of the same doc; confirms identical schema/events/payloads | 2026-08-14 |
| https://antigravity.google/docs/home | Docs site nav map — lists `/docs/hooks`, `/docs/ide/hooks`, `/docs/rules-workflows`, `/docs/permissions`, `/docs/plugins`, `/docs/skills`, `/docs/subagents`, `/docs/mcp`, `/docs/cli/*`, `/docs/sdk/*` | 2026-08-14 |
| https://antigravity.google/docs/permissions | General tool-permission model (Deny>Ask>Allow), "Ask" = interactive card; no hook-specific trust warning found | 2026-08-14 |
| https://antigravity.google/docs/cli/overview | Antigravity CLI = "lightweight TUI surface," succeeds Gemini CLI with one-time settings/extensions/skills import, v1.1.12 at fetch time, shares agent core + config sync with IDE | 2026-08-14 |
| https://antigravity.google/docs/rules-workflows | Rules/Workflows are Markdown, not hooks; file paths `~/.gemini/GEMINI.md` (global rules), `.agents/rules/` (workspace rules); disambiguation only | 2026-08-14 |
| https://antigravity.google/blog/introducing-google-antigravity-sdk | SDK announcement, published 2026-05-19, status "Research Preview"; describes the separate in-process Python hooks (Inspect/Decide/Transform, `@post_tool_call` decorator, policy DSL) | 2026-08-14 |
| https://antigravity.google/blog/google-io-2026-feature-deep-dive | Fullest vendor prose description of Hooks ("execute custom local shell scripts at critical stages..."); published 2026-05-19 | 2026-08-14 |
| https://antigravity.google/blog/introducing-google-antigravity-2 | Antigravity 2.0 announcement, published 2026-05-19; Hooks listed as new alongside Dynamic Subagents and Scheduled Tasks; GA-style availability language ("available on macOS, Linux, and Windows") | 2026-08-14 |
| https://antigravity.google/changelog | Dated version history: v1.0.8 (2026-06-12) earliest `/hooks` command bug; v1.1.7 (2026-07-24), v1.1.9 (2026-07-31), v1.1.10 (2026-08-03), v1.1.11 & v2.6.0 (2026-08-07) hook reliability fixes | 2026-08-14 |
| https://github.com/google-antigravity/antigravity-cli/releases | Corroborates changelog dates/versions; latest v1.1.13 released 2026-08-14 (today) | 2026-08-14 |
| https://github.com/google-antigravity/antigravity-sdk-python | Official Python SDK repo (google-antigravity org); confirms SDK is a distinct developer-facing product from IDE/CLI | 2026-08-14 |
| https://github.com/google-antigravity/antigravity-sdk-python/blob/main/google/antigravity/hooks/README.md | SDK's in-process hooks module: Inspect/Decide/Transform categories, execution order (Decide before Inspect, TOCTOU-safe), Context hierarchy, `policy` DSL | 2026-08-14 |
| https://en.wikipedia.org/wiki/Google_Antigravity | Product timeline: announced 2025-11-18 alongside Gemini 3, public preview at launch; license "Proprietary (free during preview)" still as of stable 2.1.1 (2026-06-22) | 2026-08-14 |
| https://geminicli.com/docs/hooks/ | Gemini CLI's own hooks overview: settings.json merge across project/user/system/extension scopes; explicit "arbitrary code" security warning + fingerprint-based re-approval | 2026-08-14 |
| https://geminicli.com/docs/hooks/reference/ | Gemini CLI hooks reference: 11 event names, exit-code contract (0/2/other), base stdin schema, `sequential` flag, "Silence is Mandatory" stdout rule | 2026-08-14 |
| https://github.com/google-gemini/gemini-cli | Confirms Gemini CLI hooks doc lives in-repo at `docs/hooks/reference.md`; issue titles (#23123, #14449, #9070, #2779) show ongoing community hook feature requests/PRs | 2026-08-14 (search only, page itself not deep-fetched) |
| `[unofficial]` https://atamel.dev/posts/2026/07-16_where_agy_hooks/ | Community post (Mete Atamel), published 2026-07-16, updated 2026-08-13; states exact filenames `.agents/hooks.json` and `~/.gemini/config/hooks.json`, and a `PreInvocation`/`PreToolUse` example with `matcher`+`hooks` nesting | 2026-08-14 |
| `[unofficial]` https://medium.com/google-cloud/a-developers-guide-to-agent-hooks-in-antigravity-cli-4c1440febd11 | Community post (Kanshi Tanaike, "Google Cloud - Community" tag on Medium), published 2026-06-26; **conflicts** with vendor docs on response schema (`allow_tool`/`deny_reason` vs. `decision` enum), global path (`~/.gemini/antigravity-cli/hooks.json`), and "always exit 0" claim — logged as a lead only, not trusted as fact | 2026-08-14 |
| `[unofficial]` https://github.com/ChernegaSergiy/antigravity-hooks | Third-party Antigravity CLI plugin (sound-effect hooks); corroborates the 5 event names and matcher regex syntax; confirms `/hooks` menu and `agy plugin install` both exist as real, distinct configuration paths | 2026-08-14 |
| `[unofficial, third-party bug reports]` github.com/manaflow-ai/cmux issues (#5358, #5473) | Real-world evidence: a malformed `PreToolUse` Decide-hook can deny every tool call and brick the agent in a third-party integration; independently confirms the `~/.gemini/config/hooks.json` global path | 2026-08-14 (search snippet only) |
