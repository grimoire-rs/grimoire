# Handover: Local path sources for grim (dev-install + path dependencies)

**Type:** Pre-ADR context / handover
**Status:** Exploration — no decision committed, no code written
**Date:** 2026-07-10
**Author:** conversation between user + Claude
**Next artifact:** ADR (`adr_local_path_sources.md`) → Plan → Implementation

---

## The idea (what the user wants)

Two related capabilities, both about **testing and maintaining a local artifact
without round-tripping through an OCI registry**:

1. **Quick local test loop** — take a skill/rule sitting in a local folder and
   render it straight into the vendor client dirs (`~/.claude/skills/…`,
   OpenCode, Copilot) to see it live. Edit → one command → installed.

2. **Local path dependencies declared in `grimoire.toml`** — a local directory
   becomes a first-class declared artifact (like a Cargo `path = "../foo"` or
   npm `file:` dependency). Then `status` / `update` detect that the directory
   changed and re-materialize it. Possibly with a `--watch` mode later so it
   updates automatically on save.

Driving use case (user's words): *"maintaining one skill and then render it for
multiple vendors used by co-workers or even one person."* A skill source lives
in a repo; co-workers clone it and `grim install` gets it rendered into whatever
AI client they use.

---

## Current state (evidence-backed — verified this session)

### The multi-vendor render engine already exists ✅

One canonical artifact is projected into multiple vendor formats **at install
time**. This is the expensive part, and it is done.

- Vendor seam: `src/install/vendor.rs` — the `Vendor` trait.
- Transform core: `src/install/render.rs` (`RenderedDoc`, `RenderError`,
  `validate_namespaced_metadata`).
- Per-vendor impls: `src/install/vendor_claude.rs` (`ClaudeVendor`),
  `vendor_opencode.rs` (`OpenCodeVendor`), `vendor_copilot.rs` (`CopilotVendor`).
- Vendor enum: `src/install/client_target.rs:43` — `ClientTarget { Claude, OpenCode, Copilot }`, `ALL` at `:102`.
- Fan-out loop: `src/install/installer.rs` — "each artifact materialized into
  every client"; loops at `:204`, `:324`, `:384`.
- **Vendors supported: `claude`, `opencode`, `copilot` (3).**

Note: `grim build` does **not** render per-vendor. It packs one canonical tar
for the registry (`src/command/build.rs:89-120`) and only *validates* namespaced
metadata as a publish gate. Per-vendor projection happens exclusively at install.

### Local source install / add / test — ABSENT ❌

Everything routes through OCI references. There is no path from a local folder
into installed client output.

- `grim add <REFERENCE>` accepts only `registry/repo:tag` or `@digest`
  (`src/command/add.rs:54`). No filesystem path, no `file://`.
- `grim install` has no positional path (`src/command/install.rs:29-38`); it
  materializes from `grimoire.lock`.
- `grim lock` has no path argument.
- Reference parsing actively rejects `./` and `../` in refs
  (`src/oci/identifier.rs:529,535`). The `./`/`../` handling in
  `src/resolve/resolver.rs:363-376` is bundle-member id resolution against the
  bundle's registry identifier — **not** disk paths.
- A local path **does** exist, but only on the publish side:
  `grim build <PATH>` and `grim release <PATH> <REF>`
  (`src/command/build.rs:30`). These validate + pack a local artifact into an
  OCI tar to push. They do not materialize into any client.

### Watch / file-watching / sync — ABSENT ❌

- No `--watch` flag anywhere. No `notify`/`inotify` crate in `Cargo.toml`.
- All `sync` hits are `Vendor::sync_config` (post-install config convergence),
  not filesystem watch. The only "live" surface is the TUI background registry
  poll (`src/tui/update_check.rs`), which watches the *registry*, not local files.

### The 20 subcommands (`src/main.rs:80-122`)

`config, context, init, lock, install, update, status, build, release, publish,
add, remove, uninstall, search, fetch, schema, tui, mcp, login, logout`.

### The existing model, in one line

`grimoire.toml` declares artifacts by registry ref + floating tag →
`grimoire.lock` pins tag → digest → `status` compares declared/locked/installed →
`update` re-resolves floating tag to a new digest and re-materializes if changed.

---

## Proposed design

Two complementary features. Feature B is the one the user is most interested in.

| | **A. Ad-hoc dev-install** | **B. Declared path dependency** |
|---|---|---|
| Command | `grim install <local-path>` (or `grim build --install`) | `path = "./skills/x"` entry in `grimoire.toml` |
| Lifetime | throwaway test | first-class, committed |
| Lockfile | stays **out** | goes **in** |
| Co-workers | no | yes — clone repo, `grim install`, rendered |
| Reuses | vendor render engine | vendor render engine + lock/status/update flow |

### Key insight — path deps FIT the digest model, they don't break it

The lockfile/reproducibility model keys everything off a digest. A local path has
no registry digest — but it has content. So:

- **Digest for a registry ref** = manifest digest from resolution (today).
- **Digest for a path dep** = **content hash of the canonical packed tar.**
  `build` already produces the exact tar via `pack_skill_dir` — hash it.

With that substitution, every existing flow keeps working, source-switched:

- `lock` / `update` → re-pack the path dep, hash it. Hash changed vs lockfile →
  re-materialize. **This is the user's "update sees the dir changed", for free.**
- `status` → current dir hash ≠ locked hash → shows `modified`.

**"Automatically"** — no daemon needed. Cargo doesn't run a background process
for path deps; it re-checks at command time. Same here: the change is picked up
on the next `grim update` / `grim install`. A real `--watch` is a separate
opt-in built on top of this (still YAGNI until the manual loop proves annoying).

### The one real tension — portability of a committed lockfile with a path

A committed `grimoire.toml` + lockfile that references a local path is only
reproducible if that path exists with matching content on the other machine.

- **Repo-relative path** (`./skills/my-skill`) in a monorepo → portable.
  Co-worker clones, path exists, hash matches. **This is exactly the user's
  "one skill, many vendors, shared with co-workers" case. It works.**
- **Absolute or out-of-repo path** → not reproducible. Forbid or warn.

Rule to adopt: **path deps must be repo-relative.** Mirrors Cargo (a crate with
`path` deps can't be `publish`ed; fine for local/workspace use).

### Earlier misstep corrected

In discussion I first said "keep local installs out of the lockfile." That is
correct for **Feature A** (ad-hoc/throwaway). For **Feature B** (declared path
dep) the opposite is right — you *want* it in the lock, with a content-hash
digest, so `status`/`update` work.

---

## Open decisions (settle in the ADR)

1. **Lockfile schema** — add a source discriminant per entry:
   `{ registry, digest }` (today) vs `{ path, content_hash }` (new). Exact TOML
   shape TBD. Check how bundle members and existing lock entries are shaped
   first (`src/lock/…`, `grimoire.lock` schema in `grim schema`).
2. **Content hash definition** — hash of the canonical packed tar (reuse
   `pack_skill_dir` / the same bytes `build` produces) so it's deterministic and
   matches what would be pushed. Confirm tar packing is byte-stable.
3. **`update` / `status` branching** — where the "re-resolve floating tag" logic
   lives, add a "re-hash local source" branch (`src/command/update.rs`,
   `status.rs`, `src/resolve/`).
4. **Path constraint** — repo-relative only; decide forbid (error) vs warn for
   absolute/out-of-repo. Decide what "repo root" means (workspace discovery
   root? config file dir?).
5. **`grimoire.toml` surface** — the declaration syntax for a path dep and how
   `grim add` might create one (e.g. `grim add --path ./skills/x`).
6. **Feature A vs B sequencing** — A is a strict subset of the render plumbing B
   needs. Ship A first (fast standalone win) or fold it in as a flag once B lands?
7. **`--watch`** — explicitly out of scope for v1. Note as a follow-up.

---

## Where to touch (module map)

| Concern | Files |
|---|---|
| New local-source input adapter | `src/command/install.rs`, new resolve branch under `src/resolve/` |
| Reuse render engine (no change expected) | `src/install/render.rs`, `vendor_*.rs`, `installer.rs` |
| Content-hash of packed tar | reuse `pack_skill_dir` (used by `src/command/build.rs`) |
| Lockfile schema + source discriminant | `src/lock/…`, `grimoire.lock` schema, `grim schema` output |
| `update` / `status` local-source branch | `src/command/update.rs`, `src/command/status.rs` |
| Ref parsing (must NOT mistake a path for a bad ref) | `src/oci/identifier.rs:529,535`, `src/resolve/resolver.rs:363` |
| Config schema (path dep declaration) | `src/config/…`, `src/command/add.rs` |

---

## Process reminders (from repo conventions)

- **Product direction** — this changes scope/positioning; consult
  `.claude/rules/product-context.md` when writing the ADR.
- **Catalog drift** — any CLI (`src/command/**`) change requires a drift review
  of the first-party catalog skills (`grim-usage`, `grim-authoring`); see
  `catalog/README.md`. `task catalog:verify` gates CI.
- **Acceptance tests** — new install/lock/update behavior needs pytest coverage
  under `test/tests/` (`subsystem-tests.md`).
- **ADR + Plan** — feature spans subsystems (config, lock, resolve, install) →
  Feature workflow: ADR → Design Spec → Plan (`workflow-feature.md`).

---

## Suggested next steps

1. Write `adr_local_path_sources.md` (template `.claude/templates/artifacts/adr.template.md`)
   deciding items 1–7 above. Lead with Feature B (path deps) + content-hash model;
   fold Feature A in as the simpler subset.
2. Prototype Feature A (`grim install <local-path>`) to de-risk the render-engine
   reuse — smallest thing that proves a local folder can reach vendor dirs.
3. Confirm tar packing is byte-stable (decision 2) before committing to
   content-hash as the digest primitive.

---

## Provenance

Findings gathered 2026-07-10 via CLI help inspection (`grim … --help`) and a
codebase scan (subagent "Find local-path and multi-vendor render support",
89k tokens, 20 tool uses). All file:line references above verified against the
working tree on branch `goat` at time of writing.
