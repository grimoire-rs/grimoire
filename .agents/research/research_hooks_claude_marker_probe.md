# Claude Code marker tolerance + `settings.local.json` gitignore probe

Executed 2026-08-17 against **Claude Code 2.1.233** (native build,
`/home/mherwig/.local/share/claude/versions/2.1.233`, commit `f8d57569aaf3`, linux-x64), real
authenticated session. Scratch tree (session-local, never committed, outside the repo):
`…/scratchpad/claude-marker-probe/` — probe launcher `hookbin/probe-hook`, three throwaway git
repos (`proj/`, `gi2/`, `gi3/`), a redirected `XDG_CONFIG_HOME` (`xdg/`), and `logs/`.

Harness shape reused from `research_hooks_launcher_verification.md` § Method / § 2.1: a shell hook
script that appends `argv`, `pwd` and its stdin to a log, touches a sentinel, exits 0; provoked with
a one-line `claude -p` prompt that must call `Bash`. Every "fired" claim below is a **real hook
process spawned by the real client binary** with the real payload on stdin. Shipped-source reads of
the Claude Code bundle are labelled as such and are never counted as a PASS on their own.

Evidence tiers: **PASS/FAIL** = executed, literal output quoted. **UNVERIFIED** = not executed, with
the settling experiment named. Nothing here is a PASS from documentation.

---

## 1. The headline

1. **The constant marker on the matcher group object is tolerated. The hook fires, and Claude emits
   no warning of any kind** — not on stdout, not on stderr, not in `--debug hooks`. Grim's ownership
   scheme can use `"com.grimoire.managed": "hook-dispatcher"` where WP-I designed it. **No fallback
   ladder is needed.**
2. The same marker **inside the handler object** is equally tolerated — the first fallback rung is
   also open, so grim has a spare.
3. **Claude Code does *not* write a repo `.gitignore` entry.** When the client creates
   `.claude/settings.local.json` itself it appends `**/.claude/settings.local.json` to the **user's
   global git excludes file** (`core.excludesfile`, default `~/.config/git/ignore`). The repo's
   `.gitignore` is untouched, so the ignore is **machine-local and user-local**: a colleague's clone,
   a second machine, or CI sees `?? .claude/`. Executed, both directions. **This weakens invariant I1
   as the plan states it** — see § 4.3.

---

## 2. Question 1 — marker on the matcher GROUP object

> `"com.grimoire.managed": "hook-dispatcher"` beside `matcher` and `hooks`, inside a
> `hooks.PreToolUse[]` entry.

**VERDICT: FIRED, NO WARNING.**

Registration used (`proj/.claude/settings.local.json`, paths elided):

```json
{
  "permissions": { "allow": ["Bash(echo:*)"] },
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "com.grimoire.managed": "hook-dispatcher",
        "hooks": [
          { "type": "command",
            "command": "PROBE_LOG='…/caseA.log' PROBE_SENTINEL='…/caseA.sentinel' exec '…/hookbin/probe-hook' CASE-A-GROUP-MARKER" }
        ]
      }
    ]
  }
}
```

Run: `claude -p "Run the bash command: echo hi" --allowedTools Bash --debug hooks --debug-file …`

| Observation | Literal evidence |
|---|---|
| Client exit | `EXIT=0` |
| Client stdout | ``Output: `hi` `` |
| Client stderr | *empty* (0 bytes) |
| Sentinel created | `-rw-r--r-- … 0 Aug 17 08:53 …/logs/caseA.sentinel` |
| Hook actually ran, with grim's argv | `=== fired at 2026-08-17T08:53:33+02:00 argv=[CASE-A-GROUP-MARKER] pwd=…/proj` |
| **The tool call really happened** (not a false negative) | hook stdin: `…"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"echo hi","description":"Echo hi"},"tool_use_id":"toolu_01TynzNFpNLFhjMV9QnnASiu"` |
| Claude consumed the hook's output | `[DEBUG] Hook output does not start with {, treating as plain text` |

Warning search over the whole 246-line debug log
(`grep -inE "warn|invalid|unknown|unrecogni|settings|grimoire|schema|ignor"`): **no hit mentions the
extra member.** The only settings-related lines are benign — `Broken symlink or missing file
encountered for settings.json at path: …/proj/.claude/settings.json` (there is no `settings.json`,
only `settings.local.json`) and `Watching for changes in setting files …/settings.local.json`.
The registration was loaded and honoured: `Replacing all allow rules for destination 'localSettings'
with 1 rule(s): ["Bash(echo:*)"]`.

`claude doctor` (which "Reads settings files in the current directory without a trust prompt") also
reported `No installation issues found.` with the marker in place — but note doctor covers the
*installation*, not settings validation, so that line is corroboration, not the proof. The proof is
the fired hook plus the silent debug log.

### Design consequence

**The WP-I registrar design stands as written.** A constant, self-describing member on the matcher
group is enough for grim to enumerate and remove exactly the registrations it owns. Grim does **not**
need an environment-derived structural predicate, so the orphaned-registration failure mode the
constant marker exists to prevent stays closed. One caveat worth stating in the plan: the tolerance
is *silence*, not *validation* — Claude neither rejects nor round-trips the member, so grim must
treat the marker as write-only truth it re-reads itself, and must preserve it whenever it rewrites a
group (Claude has no reason to keep it if the client ever rewrites the file — see § 4.2).

---

## 3. Question 2 — marker inside the HANDLER object

> `"com.grimoire.managed"` beside `type` / `command`, one level deeper.

**VERDICT: FIRED, NO WARNING.**

```json
"hooks": [
  { "type": "command",
    "com.grimoire.managed": "hook-dispatcher",
    "command": "… exec '…/hookbin/probe-hook' CASE-B-HANDLER-MARKER" }
]
```

| Observation | Literal evidence |
|---|---|
| Client exit / stdout / stderr | `EXIT=0` · ``Output: `hi` `` · stderr empty (0 bytes) |
| Sentinel created | `-rw-r--r-- … 0 Aug 17 08:54 …/logs/caseB.sentinel` |
| Hook ran | `=== fired at 2026-08-17T08:54:18+02:00 argv=[CASE-B-HANDLER-MARKER] pwd=…/proj` |
| Tool call confirmed | `"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"echo hi",…}` |
| Debug log | same benign set as § 2; no warning naming the member |

### Design consequence

The fallback rung is open, which is useful but not needed. Prefer the **group** placement (§ 2):
one marker per group is the granularity grim's remove step actually operates on, and the handler
placement would force grim to mark every handler in a group to describe the same ownership fact.

---

## 4. Question 3 — does Claude Code gitignore `settings.local.json` when *it* creates the file?

**VERDICT: FIRED — but into the WRONG FILE for I1's purposes. The repo `.gitignore` gets nothing;
the user's GLOBAL git excludes file gets the rule.**

### 4.1 How it was executed

Six real sessions across three fresh `git init` repos produced no `.gitignore` at all. The reason
the earlier verification never saw the behaviour is that the trigger is a *write* to `localSettings`
by the client — a hand-written file never triggers it, and neither does merely loading one
(the debug log distinguishes `Applying permission update:` = in-memory from
`Persisting permission update: … to source '…'` = the disk write; only the former ever appeared).

Attempts that did **not** trigger a write, and why (each executed, so the negative result is real):

| Route | Outcome |
|---|---|
| `claude -p …` with a hand-written `settings.local.json` (§ 2, § 3) | load only, no write |
| `claude mcp add -s local …` | writes `~/.claude.json`, not `settings.local.json`: `File modified: /home/mherwig/.claude.json [project: …/gi1]` |
| pty-driven interactive session, `Bash: echo hi` | auto-approved even under `--permission-mode manual` (low-risk classifier) — no dialog, no persistence |
| pty-driven, `Bash: touch probe.txt` → dialog option 2 | the offered option was `2. Yes, and always allow access to gi2/ from this project`, and it is **session-scoped**: `Applying permission update: Adding 1 directory with destination 'session'` + `Setting mode to 'acceptEdits'`. No file written. |

The route that worked, and cost **zero model tokens**: a project `.mcp.json` in a fresh repo. The
startup approval dialog persists `enabledMcpjsonServers` to `localSettings`, so the *client* creates
the file. Driven on a pty (`script -q -c "claude --debug-file …"`) with keystrokes fed on stdin —
Enter for the trust dialog, Enter for the MCP approval — and with `XDG_CONFIG_HOME` redirected into
the scratch tree so the global-ignore write would land there instead of in the user's home.

### 4.2 What appeared, verbatim

Claude Code created the file itself:

```
…/gi3/.claude/settings.local.json
{
  "enabledMcpjsonServers": [
    "probe-noop"
  ]
}
```

The repo `.gitignore`: **does not exist.**
`head: cannot open '…/gi3/.gitignore' for reading: No such file or directory`

The redirected global excludes file: **created, one line, exactly**

```
$ od -c …/xdg/git/ignore
0000000   *   *   /   .   c   l   a   u   d   e   /   s   e   t   t   i
0000020   n   g   s   .   l   o   c   a   l   .   j   s   o   n  \n
```

And the ignore is only effective while that machine-local file is in play:

```
# with the client-written global excludes file
$ XDG_CONFIG_HOME=…/xdg git check-ignore -v .claude/settings.local.json
…/xdg/git/ignore:1:**/.claude/settings.local.json    .claude/settings.local.json
exit=0

# a colleague's clone, a second machine, or CI — same repo, no such global file
$ git check-ignore -v .claude/settings.local.json
exit=1
$ git status --short
?? .claude/
?? .mcp.json
```

Shipped-source corroboration (read, not a PASS by itself — it is what told me where to look): the
bundle's ignore-writing routine resolves its target as `git config --global --get core.excludesfile`
→ else `$XDG_CONFIG_HOME/git/ignore` → else `~/.config/git/ignore`, prefixes the relative path with
`**/`, skips the write when `git check-ignore` already reports the path ignored, and is invoked as
`tHo(Jle("localSettings"), cwd)` on every `localSettings` write, gated only on "inside a git repo"
(`PHs = async (e) => Zc(e) !== null`). `Jle("localSettings")` is literally
`path.join(".claude","settings.local.json")`. A sibling call registers `.claude/<staging-dir>/` the
same way. Telemetry name: `gitignore_global_rule`. Nothing in the routine touches a repo `.gitignore`.
The same region also carries a *tracked*-detection helper that runs
`git ls-files --error-unmatch -- :(icase).claude/settings.local.json` and warns
`'<path>' is tracked in the index; gitignore rules do not apply to tracked files` — i.e. the client
knows the file can end up committed and only warns.

### 4.3 Design consequence — I1 needs restating

The plan's claim that the Claude project registration is safe "because the client gitignores the
file" is **half true, and the half that is false is the load-bearing half**:

- **True:** on the machine where Claude Code first wrote the file, the path is ignored, so a grim
  hook registration written there will not be accidentally `git add`-ed by that developer.
- **False:** the ignore does not travel with the repository. It is one line in a per-user, per-machine
  file. Consequences for I1:
  1. A **fresh clone** (colleague, second machine, CI, container, `gh codespace`) has no such rule.
     If grim registers a project-scope hook there before that machine's Claude Code has ever written
     `settings.local.json`, the file shows up as untracked and is a plausible accidental commit —
     which is exactly the "hook registration committed and executed on someone else's checkout"
     shape I1 is meant to exclude.
  2. Grim writing the file **first** actively prevents the client's rule from ever being written,
     because the routine only fires on a *client* write to `localSettings`. A grim-registered project
     hook therefore lands in a repo where nothing is ignored yet, and stays that way until the user
     accepts a permission/MCP dialog.
  3. If the file is already **tracked** in some repo, the rule is inert by git's own semantics, and
     Claude Code only logs a warning.

**Recommended plan change (WP-I / threat model):** do not rest I1 on client behaviour. Either
(a) grim ensures the ignore itself when it registers a project-scope Claude hook — appending
`.claude/settings.local.json` to the repo's own `.gitignore`, or to `.git/info/exclude` if grim
prefers not to touch a tracked file — or (b) I1 is restated to accept that a project-scope
`settings.local.json` may be committed, and the launcher/guard argument must hold on a foreign
checkout without it. Option (a) with `.git/info/exclude` is the smaller footprint: per-clone, never
committed, no diff for the user to review. Whichever is chosen, the sentence "gitignored by the
client" must leave the document.

---

## 5. What is still unverified

| Item | Why | Settling experiment |
|---|---|---|
| Whether a Claude Code **permission** dialog (as opposed to MCP approval) ever persists to `localSettings` in 2.1.233 | The only dialog I could provoke offered a *session*-scoped directory grant; `echo`-class commands are auto-approved even under `--permission-mode manual` | Provoke a command class the classifier will not auto-approve and that yields the `Yes, and don't ask again for <x> commands in <dir>` option, then select it and re-inspect. Irrelevant to the verdict — § 4.2 already proves the write path and the ignore target via the MCP route. |
| Whether Claude Code **preserves** an unknown group member when the client itself rewrites `hooks` in `settings.local.json` | No client-side hook-editing surface was exercised; every write observed touched `permissions`/`enabledMcpjsonServers`, not `hooks` | Register a marked group, then drive a client action that rewrites the hooks block (e.g. the `update-config` skill or a `/hooks`-style editor if one lands), and re-read the group. Until then grim must assume the marker can be dropped by a client rewrite and re-assert it on `grim install`. |
| Windows behaviour of either placement | No Windows host | Same two cases on a Windows box |

---

## 6. Reproduction

```sh
# Case A / Case B — marker tolerance (real session, project scope)
#   .claude/settings.local.json as in § 2 / § 3
claude -p "Run the bash command: echo hi" --allowedTools Bash \
       --debug hooks --debug-file ./caseX.debug < /dev/null
#   then: sentinel present? hook log shows argv + PreToolUse stdin with "tool_name":"Bash"?
#         stderr empty? debug log free of any line naming the extra member?

# Question 3 — client-created settings.local.json, zero model tokens
git init gi3 && printf '%s\n' '{"mcpServers":{"probe-noop":{"command":"/bin/true"}}}' > gi3/.mcp.json
{ sleep 10; printf '\r'; sleep 8; printf '\r'; sleep 10; printf '\r';
  sleep 12; printf '\003'; sleep 1; printf '\003'; sleep 3; } |
  env -u CLAUDECODE -u CLAUDE_CODE_CHILD_SESSION -u CLAUDE_CODE_ENTRYPOINT \
      -u CLAUDE_CODE_MESSAGING_SOCKET -u CLAUDE_CODE_MESSAGING_TOKEN -u CLAUDE_CODE_EXECPATH \
      -u CLAUDE_CODE_SESSION_ID -u CLAUDE_PID -u CLAUDE_EFFORT \
      XDG_CONFIG_HOME="$PWD/xdg" script -q -c "claude" /dev/null
#   then: gi3/.claude/settings.local.json exists, gi3/.gitignore does not,
#         xdg/git/ignore == "**/.claude/settings.local.json"
```

Stripping the inherited `CLAUDE_CODE_*` environment is **required** — without it the child session
inherits the parent agent's permission context and no dialog appears at all.

Cleanup performed: probe hook script and both sentinels removed; the hand-written
`proj/.claude/settings.local.json` removed; the four probe project entries purged from
`~/.claude.json` (`claude project purge -y <path>`, `none` remaining). The user's global git ignore
was **never** touched — `~/.config/git/ignore` is still absent and `core.excludesfile` still unset
(the probe redirected `XDG_CONFIG_HOME` for exactly this reason). `git -C <grimoire> status --short`
shows no file attributable to this probe other than this report.
