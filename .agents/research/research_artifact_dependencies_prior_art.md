# Research: Artifact dependencies — prior art

## Metadata

**Date:** 2026-11-01
**Domain:** packaging
**Triggered by:** `.agents/discussions/artifact-dependencies.md` — how comparable systems model package dependencies
**Expires:** 2027-05-01

## Direct Answer

Among AI-config formats, only Claude Code plugins and APM have a real dependency graph. The Agent Skills spec, MCP `server.json`, Cursor, Codex and Copilot have no dependency field. Classic and OCI ecosystems split two ways. Flat or unversioned systems: Homebrew, VS Code, Carvel and Go MVS. Range-solving systems: Cargo, uv/PubGrub, npm and Helm.

## Findings

**AI-config ecosystems**
- **Claude Code plugins** ([docs](https://code.claude.com/docs/en/plugin-dependencies)):
  - `plugin.json` takes `dependencies`: bare names, which float to the latest, or `{name, version, marketplace}` with a semver range over git tags `<plugin>--v<ver>`.
  - Several constraints on one dependency are intersected to the highest tag that satisfies them all. If none does, the install fails.
  - Cross-marketplace dependencies are refused unless the root lists the target in `allowCrossMarketplaceDependenciesOn`, to stop silent installs from unreviewed sources.
  - There is no lockfile.
  - `claude plugin prune` removes auto-installed dependencies no plugin needs, and never removes user-installed ones.
  - A plugin with only `name` and `dependencies` is a valid bundle.
- **Agent Skills spec** ([spec](https://agentskills.io/specification)): no dependency field. `compatibility` is free text.
- **APM (Microsoft)** ([docs](https://microsoft.github.io/apm/consumer/install-packages/)):
  - Resolves the full transitive graph.
  - `apm.lock.yaml` records ref plus content hash, and `--frozen` replays it.
  - A transitive MCP server is gated until the top-level manifest re-declares it, unless `--trust-transitive-mcp` is passed.
  - The docs I read don't state a diamond policy.
- **skills.sh:** has a lock but no restore. Third-party forks (skpm, skillrt) added dependency fields themselves (secondary sources).
- **Cursor, Copilot, Codex and MCP `server.json`:** no dependency field.

**OCI-native**
- **Helm** ([docs](https://helm.sh/docs/topics/charts/)):
  - `Chart.yaml` dependencies carry a semver constraint, with `Chart.lock` written by `helm dependency update` and replayed by `build`.
  - `alias` lets one chart appear several times, and `condition`/`tags` toggle subcharts.
  - Third-party sources (secondary) say there is no cross-tree dedup.
- **ORAS referrers:** attachment graph (signatures, SBOMs), not install-time dependencies.
- **Carvel imgpkg:** nested bundles are digest-only references with no depth limit and no solving.
- **Zarf:** an import chain of components (secondary).

**Classic**
- **Cargo:** unifies semver-compatible requirements and builds major-incompatible versions side by side. That causes type-mismatch hazards. Lock priority applies.
- **uv / PubGrub:** produces readable conflict traces. Allows one version per package and forks resolution by marker.
- **npm:** ranges with nested duplicates. A conflicting peer can make the tree unresolvable. `overrides` work from the root only.
- **Go MVS** ([Cox](https://research.swtch.com/vgo-mvs)):
  - Takes the minimum version that satisfies, with no SAT solver.
  - Exclusions are top-level only, because conflicting exclusions would reintroduce complexity.
  - Its own description: "understandable, predictable, boring".
- **Homebrew:** no ranges; versioned formulae are separate packages (`foo@N`). `brew autoremove` drops formulae installed only as dependencies.
- **Nix:** the closure is reachability from GC roots, so orphan removal is exact.
- **VS Code:** `extensionDependencies` and `extensionPack` take IDs only, with no versions. A request for minimum versions was closed ([vscode#202692](https://github.com/microsoft/vscode/issues/202692)).
- **Transitive-trust exposure:** [Zimmermann, USENIX 2019](https://usenix.org/system/files/sec19-zimmermann.pdf).

negative:
- No official statement of Helm's or APM's diamond behavior.
- PubGrub's original article was unreachable (HTTP 403).
- kapp-controller dependencies are only proposed.

leads:
- Claude Code's "no tag satisfies" fallback behavior.
- APM's rationale for gating transitive MCP servers.
- Helm OCI `Chart.lock` digest pinning.
- Nix `flake.lock` and Go `go.sum` as provenance without ranges.
