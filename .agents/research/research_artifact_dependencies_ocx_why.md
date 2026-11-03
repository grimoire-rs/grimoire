# Research: Artifact dependencies — ocx "why" system and VS Code slot-in points

## Metadata

**Date:** 2026-11-01
**Domain:** packaging
**Triggered by:** `.agents/adr/adr_artifact_dependencies.md` (in design). The owner asked to surface "why was this installed" in the TUI and the VS Code extension.
**Expires:** 2027-05-01

## Findings

Sibling ocx repo at `/home/mherwig/dev/ocx`. Its ADRs are under `.claude/artifacts/`.

**Dependency model**
- `metadata.json` declares an ordered `dependencies` array. Each entry has registry, digest and an advisory tag, plus a `visibility` and an optional `name`.
- No ranges and no solver. Dependencies are pinned to platform-manifest digests, never to index digests, because cascade publishing GCs the old index (`adr_dependency_manifest_pinning.md`).
- Authors write tag-only dependencies. `package create` resolves the pins, and `push` exits 65 on an unpinned dependency.
- Transitive resolution uses Kahn topological order with a lexicographic tiebreak. A diamond reaching the same digest is stored once.
- The same repo at different digests in a consumer-visible position is a hard error at composition. `deps --flat` only warns.
- Cycles are checked at `package create` (`crates/ocx_cli/src/command/package.rs:91`).
- Rejected alternatives (`adr_package_dependencies.md:562-600`): tag ranges with a resolver (not reproducible), and digest-only pins (unreadable). Chosen: digest plus advisory tag.

**Why system**
- It is `ocx package deps --why <DEP> <PKG>...` (`crates/ocx_cli/src/command/deps.rs:20-60`). The other views are a default tree with `--depth` and `--flat` evaluation order.
- A DFS returns **all** paths from each root to the target, root first. The target matches on registry and repository, ignoring the tag.
- The path is recomputed from stored metadata. No "installed because" record is persisted.
- Text output is one chain per path: `myapp:1.0 -> cmake:3.28 -> gcc:13`. A path is rebuilt from stored dependency edges each time it is asked for. No match prints a hint and exits 1.
- JSON (`website/src/public/schemas/reports/v1.json:1293-1400`):
  - `DependenciesTrace {paths: PackageRef[][], message?}`
  - `DependencyTree {items: DependencyNode[]}`
  - `DependencyNode {identifier, repeated, visibility?, dependencies[]}`
  - A repeated node is not expanded again.
- The CLI is the only surface: no TUI, no editor view. A doc cast comes from `test/doc_scripts/deps-why.sh`.

**grimoire-vscode slot-in** (`/home/mherwig/dev/grimoire-vscode`)
- Today it shows one-hop provenance only.
  - grim `status` `StatusItem.source` is a prose string `"bundle: a, b"` (`src/grim.ts:151-154`), regex-parsed by `parseViaBundles` (`src/webview/model.ts:196-205`).
  - That feeds `InstallVM.viaBundles` (`src/webview/protocol.ts:71-72`) and a "Bundle" button (`src/webview/render.ts:~1561`).
- A "Required by" row fits the details panel's Status rail (`render.ts:~1298-1305`).
- The sidebar tree (`ViewMode 'tree'`) groups by registry or scope; it is not a dependency tree.
- Gap: a structured `required_by` / `via` field is needed in place of the prose `source`.

negative:
- No ocx ADR weighs shortest-path versus all-paths, or an inverted tree.
- The ocx ADR's checklist item for create-time cycle detection is unchecked.

leads:
- `crates/ocx_cli/src/api/data/deps.rs` (`DependenciesTrace` printer).
- `test/doc_scripts/deps-why.sh`.
