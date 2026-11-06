---
paths:
  - src/hook/**
  - src/install/hook_*.rs
  - src/install/launcher.rs
  - crates/grim-hook/**
  - crates/grimoire-hook-core/**
---

# Install Layout: Hooks

Hook layout, the dispatch table, the launcher and consent. Index: [subsystem-file-structure.md](./subsystem-file-structure.md).

### Hooks {#install-layout-hooks}

A **hook** is a hybrid, and modelling it as either of its neighbours
mis-scopes half the work. It **materializes like Agent/Skill** (a payload
tree with a real recorded `ClientOutput`) and **registers like Mcp**
(foreign config, written through `sync_config`) — except more restrained,
because *the registration is never recorded at all*, plus a consent gate no
other kind has any precedent for — **workspace-scoped**, machine-local, and
written by exactly three seams (`grim hook allow`, `grim add`, an accepted
prompt). `grim install` deliberately writes none: it converges what is already
declared, and a cloned repository's `grimoire.toml` is not the user's gesture
(**T3**). Global scope is always consented and carries no record at all.

A hook payload is **machine-local at both scopes** (invariant I1):
`$GRIM_HOME/hooks/<name>/` globally, and
`$GRIM_HOME/hooks/payload/<workspace-key>/<name>/` for a workspace, where
`<workspace-key>` is the SHA-256 of the workspace path (hex) so two
workspaces sharing one `$GRIM_HOME` cannot collide. It is deliberately
**not** the dispatch `root_token`: a recorded install target is printed by
`grim status` and `grim install`, and a disclosed root token would let a
hostile repository's own registration fire the victim's already-armed hooks
(B3). A path key must be safe to print; a wire key must not be printable at
all. `payload` is therefore one of the `RESERVED_ARTIFACT_NAMES` entries — the
full set is `bin`, `consent`, `dispatch.json`, `dispatch.json.lock`, `grim-hook`, `grim-hook.exe`, `payload`,
`root-key`, and it is deliberately **not** restated as a count anywhere: this
list fell one behind the layout three times (`root-key`, the lock sidecar, then
`consent`), each time because a prose enumeration was cheaper to leave alone
than to check. The array in
`oci::hook` is the source of truth, the refusal message renders itself from it,
and `hook_dispatch`'s `every_grim_owned_name_under_hooks_is_a_reserved_binding_name`
fails the build for a layout constant that is missing from it.

**The reservation guards the *binding* name, and only since the P-2 fix.**
The check is `oci::hook::binding_name_refusal`, applied in three places, and the
split matters: `HookManifest::validate` checks the manifest's own `name` at
`grim build`, on the **publisher's** machine; `installer::install_one` checks the
**binding** name before materialization, which is the control, because the
payload directory is `payload_dir(grim_home, root, &record.name)` over that
binding; `grim add` checks it too, as the friendlier error at the moment a user
types the name. `add` cannot be the control — a **bundle** picks its members'
binding names and they never pass through that command.

`binding_name_refusal` is **two rules, not one**. The reserved-name equality
check is the narrow half; the load-bearing half is that the name must parse as a
`SkillName`, whose grammar (`[a-z0-9]+([.-][a-z0-9]+)*`) makes every traversal
spelling *unrepresentable* rather than blocklisted. A blocklist has to anticipate
the next escape; a grammar does not. That half closed a confirmed
arbitrary-file-overwrite: a binding name of `../../../../victim` from a cloned
repository's `grimoire.toml` was joined onto `$GRIM_HOME` and materialized
*before* anchor classification ran, so the write landed and only then failed to
classify. The same gap for the **other kinds'** declaration keys is tracked
separately (issue #90) and is not hook-specific — `SkillName::parse` reaches CLI
names and bundle members, never a committed config's keys.

Until wave 8 only the build-time check existed, so it guarded the wrong string:
a hook bound as `bin` did materialize `$GRIM_HOME/hooks/bin/hook.toml`. The
**reap** was the sharp edge — `grim uninstall --global hook grim-hook` would
delete the recorded output tree and take the launcher with it, silently
disarming every hook on the machine for every client and every workspace. A
hook bound under a reserved name reports `not-armed` with cause
`binding-name-reserved`.

**Convergence derives this directory from `$GRIM_HOME` and the resolved
scope, never from the install record** — that derivation, not the
relocation, is what closes SEC-1. A record is attacker-supplied in the
cloned-repository case, so a payload directory read out of one is a
directory the attacker chose.

Grim writes several things directly under **`$GRIM_HOME/hooks/`**, and that root
is load-bearing rather than incidental. The table below covers the dispatch-side
files install writes; the namespace also holds the `payload/` tree (written by
materialization, also install-side) and two runtime-side families — the rotating
`hook_audit.jsonl{,.1}` trail and transient `payload_<pid>_<slot>.json` envelopes.
The install/runtime split matters because only the install-side names are covered
by a test that walks the directory; the runtime-side ones are covered by a test
that lists them. (No count is stated: a count beside the list
it counts is how this section, and the authoring reference, each went stale once.) **Adding a file here means
adding its name to `RESERVED_ARTIFACT_NAMES`** unless it is unrepresentable as a
binding name — a hook bound to a name grim already writes lands a directory on
that path and stops every hook on the machine from arming:

| Path | What it is |
|---|---|
| `$GRIM_HOME/hooks/dispatch.json` | The dispatch table — schema-versioned, keyed by an opaque per-install root token, each root carrying its `root` path and a `hooks` array of `(artifact, id, client, event, tier, matcher, handler, timeout, payload, payload_dir, resolved_digest)` rows. Sorted on `(artifact, id, event, client)` so a no-change rewrite is byte-identical |
| `$GRIM_HOME/hooks/grim-hook` (`grim-hook.exe` on Windows) | The native launcher, the bytes embedded in grim, mode `0o755`. `launcher::ensure` rewrites it iff the bytes differ; a directory here refuses with `launcher-path-occupied` and nothing is written or reaped |
| `$GRIM_HOME/hooks/bin/grim-hook` | The retired `sh` shim. A convergence that arms something deletes it under the same guard (`launcher::retire_legacy_shim`, NotFound ignored); `bin` stays reserved. A registration an older grim wrote still points at it and exits 0 until `grim install` rewrites that workspace |
| `$GRIM_HOME/hooks/root-key` | The root-token map backing the opaque `<root>` operand — a `0o600` regular file; a directory here is unrecoverable, since the mint path is `create_new` on the same path |
| `$GRIM_HOME/hooks/dispatch.json.lock` | The table's advisory-lock sidecar (`AdvisoryFileLock` appends `.lock` to the **full** file name). Reserved: a directory here makes `try_acquire` fail `EISDIR`, so no table is written for as long as the offending artifact is installed |
| `$GRIM_HOME/hooks/owned_files/<path-key>` | The **own-file ownership record**: the hex SHA-256 of the bytes grim last wrote to one own-file hook surface (Codex `hooks.json`, Copilot `hooks/grim.json`), keyed by `hook_dispatch::workspace_key` of the surface path. A surface is grim's only while it is a regular file whose bytes match; otherwise it is never written or deleted. The underscore keeps it outside the binding grammar, so it needs no reservation |
| `$GRIM_HOME/hooks/consent/<workspace-key>.json` | The **workspace consent record** — `{v, workspace, hooks, consented_at}`, `deny_unknown_fields` with every field required, so a truncated file cannot deserialize into a valid-looking one. One file per workspace, never one shared file: a whole-file rewrite would let one workspace's operation disarm every other (Decision E's own reasoning about the dispatch table), and per-workspace files remove the read-modify-write entirely — no lock, no cross-workspace blast radius. `<workspace-key>` is `hook_dispatch::workspace_key` **verbatim**, so the record and the payload tree agree by construction rather than by a second spelling of the same identity. The `workspace` field is the identity; the filename is a lookup index only, and a record whose `workspace` ≠ the resolved one is not consent for it. An unknown `v`, a parse error or an I/O error ≡ **absent** — logged at debug, never warned, never an error (I3). Not a `ClientOutput`, so it is invisible to the anchor/remainder table and to reaping; it is never garbage-collected, and `grim hook revoke` is its only remover |

**`dispatch.json` is forward-compatible by contract.** Readers skip a root or row they cannot read (a newer tier, event or handler) and keep the rest. Writers splice into the stored JSON, so other roots and unknown keys survive verbatim. A table this grim cannot read at all (newer `schema`, unparseable, over `MAX_TABLE_BYTES`) is never overwritten: install warns, arms nothing, and status reports `not-registered`.

**The launcher is a native executable, not a script.** `crates/grim-hook` is built by `build.rs` under the `hook` profile and embedded in grim (`LAUNCHER_BYTES`); `launcher::ensure` writes it from `grim install`, under the same `dispatch.json.lock` guard that covers the table sync. It reads `dispatch.json` beside its own executable and needs no `grim` binary. The registration is one builder (`hook_command::render` over `core::argv::render`): exec form (`exec` + `args`) for Copilot, one shell string `<launcher> <client> <event> <root>` for Claude, Qoder and Codex. Copilot drops a Claude row's `args`, so Claude cannot use exec form. The three operands are never quoted; only the launcher path is, and only when it leaves the bare charset. Registration bytes are stable: Codex hashes the command text for its trust record, so a changed byte un-trusts an approved gatekeeper (`grim install` warns). The launcher exits `0/1/64/65/74/79`, never 2, and never converts a grim failure into a verdict (I3): the client's own policy decides. Argv grammar, exit table and the table's read rule are frozen-by-design; see [`adr_hook_native_dispatcher.md`](../../.agents/adr/adr_hook_native_dispatcher.md).

**Hooks arm on Linux and macOS, and on Windows only for the clients in `HOOKS_WINDOWS_VERIFIED`** (Claude only; Copilot and Codex passed only emulated launches, their CLIs are not installed on the test host, and Qoder is unobserved, so all three stay out until a live run of the client itself). A client outside the list gets cause `platform-unsupported`, `state: not-armed`, and no registration. On Windows `$GRIM_HOME` must also be a local path under `%USERPROFILE%` or `%LOCALAPPDATA%` (`grim-home-shared` otherwise), and `$GRIM_HOME/hooks/` holds no `.exe` but the launcher. A client joins the list only with a green live Windows run of its own CLI; there is no environment or flag override. `grim status` ranks the platform cause after the flag and consent gates.

**`GRIM_HOME` nesting is a refusal to arm here, not a caveat.** The
[project-state section](./subsystem-install-state.md#install-state-project) records "GRIM_HOME must not
be nested inside a workspace directory" as a *state-record* caveat — a
mis-classified anchor. For hooks the same condition makes an **armable** file
repo-resident, which the threat model forbids outright, so grim **declines to
arm** and reports it: cause `grim-home-in-workspace` for a nested root and
`grim-home-relative` for a relative one (a relative root resolves against the
process CWD, which for a client-spawned `grim-hook` *is* the workspace).
Both are `state: not-armed`. Do not downgrade either to a warning.

The client-side registration is a `sync_config` **projection, never a
recorded `ClientOutput`** — deliberately not built by copying
`install_mcp`'s record-a-`ClientOutput`-per-registration shape, because that
shape is the orphaned-registration bug behind issues #54/#55. Claude splices
`.claude/settings.local.json` at project scope; Codex (`hooks.json`) and
Copilot (`hooks/grim.json`) get a whole file grim writes and deletes — but
only while its bytes match the SHA-256 grim recorded when it last wrote them
(`$GRIM_HOME/hooks/owned_files/`). Any other file at that path is the user's:
never overwritten or deleted, and reported `not-armed` / `surface-user-owned`. **A splice surface that is a symlink is written *through* the link** (dotfiles layouts, issue #117), the link kept; at project scope a surface whose resolved path escapes the workspace — a linked file or a linked parent such as `.claude` — is refused (reported `not-registered`), because a cloned repo can plant one at an armable file (T3). This differs from MCP's `AllowRelocatedFile`, which permits any escape, on purpose. Only three of eighteen clients
arm at all, and only Claude at project scope — the reason is not scheduling
but that a committed registration makes the *executed binary path*
environment-derived (CWE-426), which every grim control is downstream of.
`docs/src/clients.md` `{#gap-hooks}` is the enforced matrix; trust it over
any prose here.

Payload-directory pruning goes through `prune::shared_by_surviving_sibling`,
the same N-outputs → one-destination refcount shared-pool skills already use
(hooks are **not** the first `entry: None` shared destination). The refcount
is record-only with no filesystem fallback, so a lost or partial record must
never delete a payload another client still references.
