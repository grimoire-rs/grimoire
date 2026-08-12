# Handover: re-pinning a declared artifact to a different tag

**Audience:** grim maintainers.
**Reported from:** grimoire-vscode (`feat/sidebar-view-modes`), 2026-08-12.
**Status:** blocked on a grim decision. No extension change can fix it; no
workaround wired (see "Why the extension can't route around it").
**Severity:** the extension's "Install Version" / "Switch Version" action
fails for **every artifact already declared in the target scope** — i.e. for
the entire case the action exists to serve.

## TL;DR

`grim add <repo>:<newtag>` refuses to re-pin an artifact that is already
declared, because the conflict guard compares the **full identifier**
(tag included) rather than the repository. A version switch is read as a
name collision:

```
$ grim add -- ghcr.io/grimoire-rs/mcp/grim:0.9.0
mcp 'grim' is already declared as ghcr.io/grimoire-rs/mcp/grim:latest;
pass --name to declare 'ghcr.io/grimoire-rs/mcp/grim:0.9.0' under a
different name                                                    (exit 64)
```

The hint is actively wrong for this case: `--name` would create a *second*
binding to the same artifact, which is not what the user asked for.

**grim's own TUI already performs this operation and is not subject to the
guard** (`src/tui/app.rs:2454` calls `declare()` directly, with the
user-picked version — see "The TUI/CLI asymmetry"). So the capability
exists and is intended; only the CLI path blocks it.

## Root cause

`src/command/add.rs:220`:

```rust
if let Some(existing) = declare(&mut set, kind, name.clone(), id.clone())
    && existing.identifier() != Some(&id)
{
    return Err(… CommandError::DeclareConflict { kind, name, existing, requested } …);
}
```

`Identifier` carries the tag, so `…/mcp/grim:latest` != `…/mcp/grim:0.9.0`
and any tag change trips the guard.

The guard's own documentation and test describe a different scenario — a
**different repository** claiming a taken binding name
(`src/command/command_error.rs:46-62`, and
`src/error.rs:576` uses `ghcr.io/acme/code-review:stable` vs
`ghcr.io/other/code-review:stable`). Protecting one publisher's binding
from another publisher is right. Refusing a re-pin of *the same artifact*
is collateral: nothing is being clobbered that the caller does not already
own and did not explicitly name.

Second site, same class: the path-source branch at `src/command/add.rs:511`
compares `DeclaredSource` values. It is a different keyspace (paths pin by
content hash, so there is no "same source, new version") and is out of
scope here — but any fix should say why it is left alone.

## The TUI/CLI asymmetry

`src/tui/app.rs:2437-2455` builds the identifier from the row's
`pinned_version` ("A user-pinned version (chosen in the picker) wins") and
declares it with no conflict check at all. Two consequences:

1. The operation the extension needs is already supported — via the TUI.
   Two front ends over one config disagree about whether it is legal.
2. The asymmetry cuts the other way too: the TUI *can* silently re-declare
   a binding against a genuinely different repository, which is exactly what
   the `add` guard exists to prevent. Whatever is decided below, the two
   paths should end up sharing one rule.

## Proposals

**1 — Narrow the guard to the repository (recommended).** Refuse only when
the existing declaration points at a different repo; same repo + different
tag/digest is a deliberate re-pin. No new CLI surface, no flag for callers
to discover, and the VS Code extension needs **zero** changes — its current
`grim add <repo>:<tag>` starts working the moment this lands.

Edge cases a fix should pin down explicitly:

- **Registry ↔ Path source change** must still refuse (different kind of
  declaration entirely, and the path branch pins by content hash).
- **Different registry host, same repo path** (`ghcr.io/acme/x` →
  `quay.io/acme/x`) — different repository, keep refusing.
- **Tag → digest and back** (`:latest` → `@sha256:…` of the same repo)
  should be allowed; it is the same artifact, pinned harder.
- The `--name` hint stays correct for the surviving cross-repo case and
  should stay in that message.
- Relock/re-materialize already follows, since `declare()` has inserted the
  new source before the guard runs and `write_config_and_relock` takes it
  from there — worth an integration test that the files on disk actually
  change version, not just the config.

**2 — Explicit opt-in flag** (`grim add --repin`, or `--allow-retag`),
keeping today's refusal as the default. Costs an extension change plus a
live release gate on the flag, and every other grim front end has to learn
it. Acceptable if the silent-overwrite risk is judged real, but note that
the TUI already overwrites silently today with no flag.

Either way: consider routing the TUI's declare through the same guard, so
one rule governs both.

## Why the extension can't route around it

- `grim update` has no `--to <version>` — it re-resolves floating tags, it
  cannot move a pin to a chosen tag.
- `--name` produces a second binding under a different name, not a switch.
- `grim remove <kind> <name>` then `grim add <ref>:<tag>` is two
  non-atomic steps: a failed `add` (network, auth, tag typo) leaves the
  artifact **undeclared**, having silently destroyed a working declaration.
  Not something a UI button should risk, so it was not wired.

Per the standing "no grim edits from the extension side, relay contract
issues" rule, nothing was changed in either repo for this.

## Extension call sites (for impact review)

Repo: `grimoire-vscode`, branch `feat/sidebar-view-modes`.

| Site | What it does |
|---|---|
| `src/views/pickVersion.ts:45` | `addArgs(`${refRepo(repo)}:${tag}`)` — the only argv builder involved |
| `src/views/pickVersion.ts:59` | runs it in the chosen scope |
| `src/grim.ts:561` `addArgs` | `['add', …, '--', reference]`; supports `--kind`, `--name`, `--no-install` |
| `src/webview/model.ts:971` | card menu entry "Install Version" |
| `src/webview/render.ts` `scopeInstallButton` / `scopeUninstallButton` / `scopeUpdateButton` | details split-button chevrons: "Install Version" / "Switch Version" |

All five funnel into the one `grim add` call. Under proposal 1 none of them
change; under proposal 2 only `addArgs` and `pickVersion` do.

## Reproduction

```sh
grim init --global
grim add --global -- ghcr.io/grimoire-rs/mcp/grim:latest   # ok
grim add --global -- ghcr.io/grimoire-rs/mcp/grim:0.9.0    # exit 64, DeclareConflict
```

Observed against `grim 0.12.1` (build at
`/home/mherwig/dev/grimoire/target/release/grim`).

## Open items

- Decide proposal 1 vs 2; if 2, the extension needs the flag name and a
  release-gate-able signal that the running grim supports it.
- Decide whether the TUI's unguarded `declare()` adopts the same rule.
- If the guard narrows, `json-interface.md` / `commands.md` should state
  that `grim add` on an existing binding re-pins it, and that a cross-repo
  rebind still refuses with `--name` as the remedy.
