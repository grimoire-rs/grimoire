# ADR: anchor-escape recovery and the `forceable` error contract

- **Status**: Accepted (2026-07-21)
- **Context**: [grimoire-rs/grimoire#57](https://github.com/grimoire-rs/grimoire/issues/57)
- **Scope**: `grimoire` (Rust CLI) + `grimoire-vscode` (extension)
- **Supersedes**: nothing. **Amends**: the `# Residual TOCTOU` threat-model note in
  `src/install/path_anchor.rs:300-313`.

## Context

grim's anchor system stores install targets **lexically** (`from_target` →
`strip_prefix_relative`, `path_anchor.rs:254-280`, `:563-597` — zero filesystem
access) and validates them by **canonicalizing** at read time (`resolve` Layer 2,
`path_anchor.rs:356-370`). A symlink on any path component between the anchor root
and the artifact is therefore invisible at install and fatal at every read
afterwards.

Reproduced end to end on grim 0.10.0 (Linux, hermetic `$HOME`): `add` exits 0 and
writes a well-formed record; `status` reports `missing` with exit 0; `update`,
`install` and `uninstall` all exit 65 with
`resolved path escapes its anchor root (anchor: claude-root)`; `remove` undeclares
but orphans the record, so the next `update` fails again. `--force` is a no-op —
`installer.rs:857-861` returns on `is_present()`'s `Err` three lines before `force`
is read at `:865`. `resolve` is byte-identical at `v0.8.4` and HEAD.

The layouts that trigger this — GNU stow, yadm, iCloud/Dropbox-synced config dirs —
are legitimate and common.

## Decision 1 — a derived `forceable` boolean in the JSON error envelope

Emit `"forceable": true`, omit-when-absent, derived from a new
`ErrorReason::forceable()`. No remediation payload, no anchor/root keys.

**Why.** grim already emits exactly this shape for `retryable`
(`error.rs:129-138`, `main.rs:248-261`, `docs/src/json-interface.md:353-369`) — one
pattern, one doc paragraph, one test shape. The hard requirement it satisfies:
clients must stop keying on exit codes. `error.rs:130-135` documents the trap, and
this issue is a live instance of it — exit 65 covers both the forceable drift
refusal and the non-forceable containment refusal.

**Rejected.** A structured remediation command bakes argv into a frozen contract and
cannot be correct anyway: the extension prepends `--global` itself
(`grimoire-vscode/src/scopes.ts`), so grim does not know the caller's scope flags. A
client-side policy table over reason slugs is kept only as the conceptual fallback —
as the primary mechanism it silently loses the dialog when grim adds a forceable
reason, which is the drift `error.rs:321-324` was written to prevent.

## Decision 2 — asymmetric containment: relax reads, keep destructive callers strict

`AnchoredPath::resolve` gains an explicit caller-intent parameter.
`Containment::AllowRelocatedAncestor` permits an escape when the leaf is not itself
a symlink; `Containment::Strict` preserves today's behaviour. Read-only probes take
the former, anything that deletes or rewrites takes the latter.

**Why not a blanket relax.** `remove_output` calls `std::fs::remove_dir_all` on the
resolved path (`uninstall.rs:207`). Relaxing every caller would let a **stored record
direct a recursive delete outside the anchor root**, which is a materially larger
primitive than the write it was assumed to be. Read paths alone are sufficient to fix
#57.

*Corrected 2026-07-21 (Phase 3 architect review).* An earlier draft of this ADR
claimed the relax would give grim a delete-outside-the-root capability it did not
already have. That is false, and the looser claim "grim never deletes outside the
anchor root" was never an invariant: the install path already does it. `remove_path`
at `installer.rs:638-639` (and `:644` for the support dir) operates on `dest`, which
is built lexically by `target.path_for()` at `:603` and never passes through
`resolve()` — so for a relocated-ancestor user, grim already recursively removes a
tree outside the root, and arguably that is exactly what the user asked for by
symlinking. The precise, defensible claim this decision rests on is narrower:

> **A tampered or stale state record can never direct a delete outside the anchor
> root**, for escapes reachable by symlink — the mechanism `canonicalize` detects.

Scoped deliberately (2026-07-21, Phase 6 security review). `canonicalize` resolves
symlinks, not mount points: a bind mount at `<anchor_root>/skills` → `/etc` yields a
`canon_candidate` that `starts_with(canon_root)` accepts under **`Strict` as well as**
permissive, so a record naming `skills/x` would direct `remove_dir_all` at `/etc/x`.
That is pre-existing — `Strict` is byte-identical to the pre-change behaviour — and
setting it up needs privilege or a user namespace, i.e. the adversary this threat model
already excludes. It is named here because an unqualified "can never" invites the reading
that the guard covers every way a path can end up outside the root, and it does not.

That invariant is what `Containment::Strict` on every record-driven destructive
caller preserves, and it is the one worth having — the record is the untrusted input,
the user's own directory layout is not.

**Why the leaf stays strict.** Layer 1 (`path_anchor.rs:336-342`) already rejects
non-`Normal` components, so an escape with a non-symlink leaf can only originate
from a symlinked ancestor *inside* the anchor root — the user's own layout. A
symlinked leaf is the CWE-59 shape and is where the installer writes
(`installer.rs:649-659`).

**Why the signature change is deliberate.** Threading `Containment` forces every
existing call site to be visited and classified, and prevents a future caller from
silently inheriting the permissive mode — the same fail-closed discipline as the
no-wildcard match at `prune.rs:81-88`.

**Threat-model honesty.** An earlier draft justified this by citing
`path_anchor.rs:300-313` as already excluding the adversary. That citation is wrong:
the doc excludes only a *racing* adversary ("swapped between this call and the
caller's filesystem op"). A **pre-planted** symlink is precisely what Layer 2's own
comment at `:346-348` says the guard exists for. This ADR narrows that guard on read
paths as a deliberate, documented trade-off — not as something the existing doc
already sanctioned. The `# Residual TOCTOU` section must be amended to say so.

**Constraints carried by this decision.**

- The carve-out is `#[cfg(unix)]`. `is_symlink()` does not cover every Windows
  reparse tag (`LX_SYMLINK`, `APPEXECLINK`, WCI), and a security guard must not rest
  on that predicate. The layouts this exists for are Unix-only.
- Leaf metadata is taken **once**, before the `exists()`/`is_symlink()` branch —
  stat'ing after `canonicalize` widens the TOCTOU window in the exploitable
  direction.
- The permissive branch logs `warn!` with the resolved path, not `debug!`: the
  destructive callers refuse this path, so it is the only signal that grim is
  reading through a relocated ancestor.
- **Non-goal, permanently:** never cache a validated root or path prefix across
  `resolve()` calls. That is the shape of gitoxide GHSA-f89h-2fjh-2r9q /
  CVE-2026-44471 (symlink-prefix-reuse worktree escape). grim is safe today only
  because it re-canonicalizes fresh on every call; the per-artifact loop at
  `installer.rs:602-699` is the obvious place a future contributor would "optimize".
- grim must not be run elevated.

**What the guard does NOT cover, stated so nobody assumes otherwise.** Layer 2 detects
escapes that `canonicalize` can see — i.e. symlinks. Two mechanisms route a path outside
the anchor root *without ever triggering the check*, both before and after this ADR:

- **Hardlinks.** A hardlink is a second directory entry for the same inode, not a
  redirection. `symlink_metadata` reports a regular file and `canonicalize` returns the
  path as given, so `starts_with` passes and no escape is detected. Writing through it
  mutates the outside file's contents.
- **Bind mounts / mount points.** `realpath` does not resolve a bind mount back to its
  source, so the path likewise stays "inside" the root.

Neither is widened by the ancestor carve-out — the carve-out only changes what happens
*after* an escape is detected. Detecting them would require inode- or mount-aware
comparison, which is out of scope for the same reason `openat2` is: both require write
access to the user's own config dir to set up, which is the adversary the threat model
excludes. Recorded because "the containment guard" invites the reading that any path
landing outside the root is caught, and that is not what it promises.

**Accepted cost.** For relocated-ancestor users, `uninstall` no longer deletes the
files — it drops the record and reports what it left behind (`retained` on
`UninstallResult`). Silent divergence between state and filesystem is not
acceptable; reported divergence is.

**Rejected.** A blanket relax (the delete primitive above). An opt-in config knob —
broken by default for every affected user, and a global allow-switch is weaker than
the read/destructive split. `cap-std`/`openat2` component-wise resolution — defends
against the racing adversary the module excludes, and does not fix the bug.
Relaxing leaf symlinks too — removes the guard almost entirely.

## Decision 3 — two structurally different client surfaces

A forceable refusal gets a **modal** confirm with an `Overwrite` action, Cancel
default-focused and bound to dismiss, no "don't ask again", and grim's `message`
displayed verbatim. A containment refusal gets a **non-modal** notification with
**no override control of any kind** — offering one on a security refusal trains
click-through. Its remediation is uninstall + reinstall, which this ADR makes work
for the first time.

## Consequences

- Every install wedged by a symlinked **ancestor** recovers on upgrade with no user
  action and no state migration — records were always correct; only the read side
  rejected them.
- **The stated remediation holds only on the adoption branch.** "Uninstall + reinstall"
  works when the footprint hash still matches and the files are adopted
  (`installer.rs:596`). If the pin moved between the two, the hashes differ and the user
  gets a generic `RefusedUntracked` modal whose `--force` recursively removes a tree
  outside the anchor root via `remove_path`. That is invariant-safe — `dest` is
  layout-derived, not record-derived — but the dialog does not say so. Accepted and
  documented rather than fixed (2026-07-21): wording it specially would require the
  resolved out-of-root path on the wire, which this ADR deliberately keeps off it.
- A symlinked **leaf** stays refused. GNU stow tree-unfolding and chezmoi `symlink_`
  entries do produce these, so a real share of affected users recover via
  uninstall + reinstall rather than automatically. chezmoi hit this same case
  (twpayne/chezmoi#2758) and also chose refuse-over-heuristic.
- `forceable` and `anchor-escape` become permanent wire surface (frozen-additive).
- The extension's `MINIMUM_GRIM_VERSION` floor moves to the release carrying the
  contract; no compat shim, per project policy.
