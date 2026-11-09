# Research: Lock pin stability — desired-state reconcile (adjacent fields)

## Metadata

**Date:** 2026-11-03
**Domain:** packaging
**Triggered by:** /hex-discuss lock-pin-stability (.agents/discussions/lock-pin-stability.md)
**Expires:** 2027-05-03

## Research: desired-state reconcile in adjacent fields

Method note: pages were read through a summarizing fetcher on 2026-11-03. Docs pages carry no visible publication date, so every "current docs" source below is dated by retrieval, not authorship; treat all as needing re-verification only if a claim is load-bearing. Quotes are the fetcher's extracts of the pages. Items marked UNVERIFIED were not confirmed from a primary page.

### Direct answer (short)

Every system studied splits the work into three steps: compute a target, observe the current state, derive mutations from the difference. They differ on where the subset filter sits.

- Package-manager lockfiles (cargo, uv) and OS transactions (dnf) restrict the target. A named update moves only the named entry plus what the solver must move. Everything else keeps its recorded value.
- Infrastructure planners (Terraform) compute over the whole graph and then filter by address. The filter widens to dependencies and is officially discouraged for routine use.
- Cluster tools (kubectl prune, Argo CD, Flux) scope deletion by labels, allowlists or an inventory. Scoping deletion is the documented source of accidents.
- Drift (state changed by someone else) is handled in four ways: reported only (Terraform refresh-only, Puppet noop, Ansible check), overwritten by default (Flux Override, Argo selfHeal), refused unless forced (SSA conflicts, home-manager collisions), or ignored by default (Argo automated sync without selfHeal).
- Reports come from the computed diff in the planners (plan, diff, check mode) and from execution results in the converging runtimes (Puppet reports, dnf history).

### Per-system findings (axes 1-6)

#### Terraform

1. Target computation. Plan "Reads the current state of any already-existing remote objects", "Compares the current configuration to the prior state", then "Proposes a set of change actions" (https://developer.hashicorp.com/terraform/cli/commands/plan). The inputs are configuration, state and the refreshed remote objects. Modes are exclusive: normal, `-destroy`, `-refresh-only`. `-refresh=false` skips the observation step and "causes Terraform to ignore external changes", which can give an incomplete plan (same page).
2. Subset. `-target=ADDRESS`: "it will also then extend the selection to include all other objects that those selections depend on" (same page). It is a filter on the whole graph. The tutorial says targeting "updates resources that the target depends on, but not resources that depend on it"; targeting one index of a multi-instance resource updated every instance (https://developer.hashicorp.com/terraform/tutorials/state/resource-targeting). Out-of-scope resources are left untouched and unreported; the only signal is a warning.
3. Deletion in scoped runs. Not stated in the pages read; a destroy of a targeted address plus its dependents is the documented use. UNVERIFIED for non-targeted drift deletions.
4. Drift. The normal plan refreshes in memory and shows "Objects have changed outside of Terraform" (https://developer.hashicorp.com/terraform/tutorials/state/refresh). `-refresh-only` records the observed values into state "without making modifications to your infrastructure", and says it "will not take any actions to undo these". The JSON plan has a separate `resource_drift` list (https://developer.hashicorp.com/terraform/internals/json-format). `-replace=ADDRESS` forces recreation of a degraded object (plan page).
5. Reporting. `resource_changes` actions are `no-op`, `create`, `read`, `update`, `delete`; replace is the pair `["delete","create"]` or `["create","delete"]` (json-format page). Reports come from the computed plan. A saved plan can go stale: "always re-check the final non-speculative plan before applying" (plan page).
6. Pitfalls. Warning text: "Resource targeting is in effect"; "The -target option is not for routine use, and is provided only for exceptional situations"; apply: "Applied changes may be incomplete ... some changes requested in the configuration may have been ignored and the output values may not be fully updated" (resource-targeting tutorial). The plan page: routine use "can lead to undetected configuration drift". The tutorial says to run a full plan or apply afterward.

#### Kubernetes

1. Controllers. A controller is a non-terminating loop that moves "current cluster state closer to the desired state" (https://kubernetes.io/docs/concepts/architecture/controller/). It reads the spec, acts, and reports current state back. `kubectl apply` is a client-side one-shot: it writes a `kubectl.kubernetes.io/last-applied-configuration` annotation and diffs against it (https://kubernetes.io/docs/tasks/manage-kubernetes-objects/declarative-config/). That page also says apply "retains writes made to live objects without merging the changes back into the object configuration files".
2. Subset. `apply -f` is scoped by the files passed; `--prune` adds a label selector `-l` or `--all` and a GVK allowlist (same page). The target is the manifest set, restricted by the caller. The selector bounds only the deletion candidates.
3. Prune scoping. "Allowlist-based pruning queries the API server for all objects of the allowlisted GVKs that match the given labels ... If an object matches the query, and it does not have a manifest in the directory, and it has a last-applied-configuration annotation, it is deleted." `--prune-allowlist` default is "a partial list ... which can lead to surprising results". "Apply with prune should only be run against the root directory ... Running against sub-directories can cause objects to be unintentionally deleted." Changing flag values between runs can delete or retain objects unexpectedly. Alpha since v1.5; ApplySet (alpha since v1.27, parent object tracks membership) "is designed to replace" it. The docs call `kubectl delete -f` the recommended approach. Same page.
4. Drift and ownership (server-side apply). The server tracks managed fields; an apply must name a field manager. "A conflict is a special status error ... This prevents an applier from unintentionally overwriting the value set by another user"; the request is rejected "unless the client forces an override". Force (`--force-conflicts`) "changes the value of the field, and removes the field from all other managers' entries in managedFields". Same-value appliers share ownership. A field dropped from the manifest is deleted or reset to its default only if no other manager owns it (https://kubernetes.io/docs/reference/using-api/server-side-apply/). The ownership is per field, not per object.
5. Reporting. `kubectl diff` runs a server-side apply dry-run and needs PATCH/CREATE/UPDATE permissions (declarative-config page). Apply prints per-object created/configured/unchanged/pruned; that last format is from memory, UNVERIFIED here.
6. Pitfalls. See prune above.

#### Argo CD

1. Target is the Git render; live state is compared to it; app status is Synced or OutOfSync. Automated sync "will only be performed if the application is OutOfSync" and will not retry the same commit-SHA after a failure unless `retry` is set (https://argo-cd.readthedocs.io/en/stable/user-guide/auto_sync/).
2. Subset. Selective sync applies only chosen resources, leaves no history entry, so it cannot be rolled back, and skips sync hooks (https://argo-cd.readthedocs.io/en/stable/user-guide/selective_sync/). The page does not say what happens to the unselected out-of-sync resources; the app stays OutOfSync by definition of the comparison (inference, UNVERIFIED). `ApplyOutOfSyncOnly=true` is a different optimization: only drifted resources are applied, hooks run, history is recorded (https://argo-cd.readthedocs.io/en/stable/user-guide/sync-options/).
3. Prune. Automated sync "will not delete resources when Argo CD detects the resource is no longer defined in Git" unless `prune: true`. An `allowEmpty` guard stops pruning down to an empty app (auto_sync page). Per-resource `Prune=false` overrides the app policy and the app is shown OutOfSync if a prune is expected; `PruneLast` runs prune as a final wave after the others are healthy (sync-options page).
4. Drift. "By default, changes that are made to the live cluster will not trigger automated sync"; `selfHeal` enables reverting (auto_sync page). `Replace=true` uses replace/create instead of apply and "can be destructive" (sync-options page).
5. Reporting comes from the diff (sync status) plus per-sync operation results.
6. Pitfalls. Selective sync not being recorded or reversible; prune expectations leave a permanent OutOfSync status.

#### Flux (Kustomize controller)

1. Source revision is rendered and server-side-applied on an interval; each interval it runs "a server-side apply dry-run to detect and correct drift" (https://fluxcd.io/flux/components/kustomize/kustomizations/).
2. Subset. Not a per-run concept; `suspend` pauses both new revisions and drift correction for the whole object (same page).
3. Prune. `.spec.prune` is required; garbage collection uses `.status.inventory` (the controller's record of what it applied). Objects previously applied but absent from the new revision are removed. A resource is protected with the label or annotation `kustomize.toolkit.fluxcd.io/prune: disabled`. Deleting the Kustomization also removes what it applied. The fetcher did not return the Inventory section, so the inventory mechanics are UNVERIFIED.
4. Drift. The default policy `Override` reverts manual edits each reconcile; `Merge` keeps other tools' fields but Flux's fields always win. `force` replaces resources when patching fails on immutable fields; the page says it should be temporary and may cause downtime; a per-resource `force: enabled` annotation is the narrower form.
5. Reporting. Status and events per reconcile.
6. Pitfalls. Force-replace downtime; manual edits silently reverted by default.

#### Helmfile

1. Target is the state file's release list. `helmfile diff` is a dry run; `apply` deploys. Per the Go package docs for v0.33.0 (old), `apply` starts with `diff` and runs `sync` only if there are changes, after confirmation (https://pkg.go.dev/github.com/helmfile/helmfile@v0.33.0, via search snippet; UNVERIFIED against current code, version is old).
2. Subset. Selectors match release labels, and `name`, `namespace`, `chart`: comma-separated terms AND together, repeated `--selector` flags OR together, `!=` inverts, and a nested `helmfiles:` entry with its own `selectors:` replaces the CLI selectors for its subtree (https://helmfile.readthedocs.io/en/latest/). The target is the filtered release set. Needs (dependencies) are ignored by default; `--include-needs` adds direct needs and `--include-transitive-needs` adds transitive ones, overriding selector exclusions (https://helmfile.readthedocs.io/en/latest/releases/, via search snippet; UNVERIFIED). The docs note cross-branch transitive `needs:` are not auto-resurrected when `dir=` prunes branches.
3. Deletion. Not covered in pages read. Releases are removed via `installed: false` or `destroy`; UNVERIFIED.
4. Drift. Diffing is via the helm-diff plugin, so drift outside Helm's own record is only seen as far as helm-diff sees it. The page read does not state this; treat as UNVERIFIED.
5. Reporting comes from the diff.
6. Pitfalls. A selector excludes dependencies silently unless the needs flags are passed.

#### Nix (profiles, home-manager)

1. A profile is "a symlink to path-N-link", each generation a store path and a GC root (https://nix.dev/manual/nix/stable/command-ref/new-cli/nix3-profile.html). `nix profile rollback` returns to a prior generation. The page does not describe how elements are selected for upgrade or remove, nor the atomic switch (UNVERIFIED). Target = build output; the new generation replaces the profile link as one unit.
2. Subset. Not covered by the pages read.
3. Deletion. Falls out of generations: a path not in the new generation is no longer linked. GC is separate.
4. Drift. home-manager checks for collisions with unmanaged files before activation: "Existing file '...' is in the way" aborts activation before changing anything. `backupFileExtension` (or `-b`) moves the colliding file aside, and still aborts if the backup name exists. `force = true` on a file entry skips the check and replaces it; the page warns it "can silently delete local changes" (https://nix-community.github.io/home-manager/usage/dotfiles.html). `overwriteBackup` can clobber backups. Force here is per file entry, not global.
5. Reporting. Not covered by the pages read.
6. Pitfalls. Collision errors on first adoption of an existing config.

#### Ansible check and diff mode

Evidence is thin because the official page returned HTTP 429 twice. From secondary sources (search snippets, https://oneuptime.com/blog/post/2026-02-21-ansible-playbook-check-mode-dry-run/view, https://engineering.nordeus.com/ansible-check-mode-tips/): modules that do not support check mode are skipped; `command`/`shell` are skipped unless `creates`/`removes` is given; a task that consumes a skipped task's output can fail or mislead; `--diff` output exists only for modules that support it. Check mode reports from a per-module prediction, not a computed whole-state diff, and its accuracy is module-dependent. Scoping (`--limit`, `--tags`) restricts which tasks run; the official page was not read. All UNVERIFIED against primary docs; re-fetch https://docs.ansible.com/ansible/latest/playbook_guide/playbooks_checkmode.html.

#### Puppet and Chef

Puppet noop: the report counts "resources that would have been changed if not run in no-op mode", nodes are labeled "Would have intentional changes" or "Would be unchanged", and resources with `noop => false` are enforced even during a noop run; such a node is still reported as a noop node (https://puppet.com/docs/pe/latest/infrastructure_reports.html, via search snippet). So the prediction comes from the real run's per-resource comparison of catalog vs actual state, with a per-resource override. Chef why-run: the docs URL returned 404; not covered. Catalog-vs-actual detail beyond the report page: UNVERIFIED.

#### OS package transactions (dnf; apt and pacman not verified)

1. dnf computes a transaction from the installed set plus the request. `install` "Makes sure that the given packages and their dependencies are installed" (https://dnf.readthedocs.io/en/latest/command_ref.html). `upgrade <pkg>` moves the named package and "updates dependencies as needed". Without arguments it updates every package to the latest version that is both available and resolvable; by default it skips updates that cannot be installed for dependency reasons, `--best` turns that into a failure with a reason, and `--allowerasing` lets the solver remove installed packages.
2. Subset. A named request restricts the target; the solver pulls in what it needs.
3. Deletion. `remove` removes the named packages and their dependents; with `clean_requirements_on_remove` (default on) it also removes dependencies no longer needed; `autoremove` removes dependency-installed packages no user-installed package needs; `mark install` shields a package. `--noautoremove` disables it per run.
4. Drift. Not covered in the page. The installed set (rpm database) is the observed state.
5. Reporting. The transaction summary comes from the solver before execution; `dnf history` records executed transactions with action types (Install, Downgrade, Obsolete, Upgrade, Remove, Reinstall, Reason change). `history undo` reverses a transaction; `history rollback` reverses all after a point, and "will not undo any transaction" if the rpm database state prevents it. `--assumeno` answers no to all prompts (a preview-by-refusal).
6. Pitfalls. Arch's "no partial upgrades" guidance could not be fetched (Anubis block): UNVERIFIED. apt: not researched.

#### Other systems with the same shape: cargo and uv

- cargo: `cargo update <spec>` is "a conservative update": only the named dependency changes; "Its transitive dependencies will be updated only if SPEC cannot be updated without updating dependencies. All other dependencies will remain locked". With no spec, all dependencies update. `--precise` pins a version, `--recursive` forces the spec's dependencies too, `--dry-run` shows without writing, `-w` updates workspace packages only and others "only if they don't already exist in the lockfile". `--locked` errors if the lock would change (https://doc.rust-lang.org/cargo/commands/cargo-update.html). Behavior of plain `cargo build` on a stale lock was not covered by this page (UNVERIFIED).
- uv: "uv will prefer the previously locked versions of packages when running `uv sync` and `uv lock`"; versions change only if constraints exclude the old pin. `--upgrade-package <pkg>` upgrades one package "while retaining the locked versions of all other packages". `uv lock --check`, `--locked` (error if stale), `--frozen` (no check). `uv sync` is exact by default and removes unselected packages; `uv run` syncs inexactly; `--inexact` opts out (https://docs.astral.sh/uv/concepts/projects/sync/).

### Cross-cutting patterns (neutral comparison table)

| System | Subset mechanism | Target restricted or filtered | Out-of-scope drift or missing | Scoped deletion | Local-modification handling | Report source |
|---|---|---|---|---|---|---|
| Terraform | `-target` address | Computed over all, then filtered; widened to dependencies | Untouched; warning only | Not documented in pages read | Refresh shows drift; `-refresh-only` records it; no overwrite without a plan | Computed plan |
| kubectl apply | files, `-l`, allowlist | Manifest set restricted by the caller | Live objects absent from files ignored unless pruned | Selector + allowlist + last-applied annotation; alpha; documented accidents | SSA: conflict refused; `--force-conflicts` takes the field | Diff (dry-run) and per-object outcome |
| Argo CD | selective sync | Chosen resources only | Stay OutOfSync (inference) | Off by default for auto sync; per-resource `Prune=false` | Ignored by default; `selfHeal` reverts | Sync status, operation result |
| Flux | none per run (`suspend`) | Whole source | n/a | Inventory-based; label opt-out | `Override` reverts; `Merge` keeps others' fields | Status and events |
| Helmfile | label selectors | Release set filtered; needs optional | Not touched | Not documented here | via helm-diff (UNVERIFIED) | Diff |
| home-manager | none | Whole generation | n/a | Via generation switch | Collision aborts before any change; backup or per-file force | Not covered |
| Ansible | `--limit`, `--tags` (UNVERIFIED) | Tasks filtered | Not run | Task-defined | Module-dependent | Predicted per module |
| Puppet | per-resource `noop` | Per resource | n/a | Catalog-defined | Reported as would-change; enforced unless noop | Run report |
| dnf | named packages | Request restricted; solver widens | Left as installed | `remove`/`autoremove` with dependency rules | rpm database state blocks undo | Pre-run summary, then history |
| cargo | `update <spec>` | Lock entries other than spec kept | Kept | n/a | `--locked` errors on change | Printed lock changes; `--dry-run` |
| uv | `--upgrade-package` | Previous pins preferred; one moved | Kept | `sync` exact removes extras | `--locked` / `--frozen` | Not covered |

### negative:

- Argo CD docs read do not say what happens to unselected out-of-sync resources after a selective sync.
- No page read states Terraform's behavior for deleting drifted non-target resources under `-target`.
- Ansible's official check-mode page, Chef why-run, Arch wiki on partial upgrades, and apt were not retrieved (429, 404, bot block, not attempted).
- Helmfile's drift model and deletion scoping, Flux inventory internals, and Nix element selection are not covered by retrieved text.
- No retrieved source gives dates; staleness cannot be assessed beyond "current docs".

### leads: (one line each)

- Terraform: how `-target` interacts with `-destroy` and drift in non-target resources; read the `-target` section of https://developer.hashicorp.com/terraform/cli/commands/plan.
- Kubernetes ApplySet KEP: how the parent object tracks membership versus label scoping.
- Flux `.status.inventory` schema: the record of applied objects used for pruning (the closest analogue to an install-state file).
- Helm `--reuse-values` / `helm upgrade` three-way merge: another drift-versus-desired model.
- Arch Linux "partial upgrades unsupported" and apt `--only-upgrade`/`apt-mark`: scoped upgrade rules in other solvers.
- npm `ci` vs `install`, pip-tools `--upgrade-package`, Poetry `lock` (previously keep pins; `--regenerate` flag): lockfile analogues.
- home-manager generations/rollback pages (`usage/rollbacks.html`, `internals/activation.html`).
- Ansible official check/diff page and `--limit`: re-fetch.
- Puppet `noop` per-resource documentation and Chef why-run: primary pages.

### Sources (URL — what — date)

All retrieved 2026-11-03; none shows a publication date (re-verify if load-bearing).

- https://developer.hashicorp.com/terraform/cli/commands/plan — Terraform plan modes, `-target`, `-refresh-only`
- https://developer.hashicorp.com/terraform/tutorials/state/resource-targeting — targeting warnings and behavior
- https://developer.hashicorp.com/terraform/tutorials/state/refresh — drift, refresh-only
- https://developer.hashicorp.com/terraform/internals/json-format — plan action values, `resource_drift`
- https://kubernetes.io/docs/reference/using-api/server-side-apply/ — field managers, conflicts, force (last ~25k chars unread)
- https://kubernetes.io/docs/tasks/manage-kubernetes-objects/declarative-config/ — apply, prune, ApplySet, diff
- https://kubernetes.io/docs/concepts/architecture/controller/ — control loops
- https://argo-cd.readthedocs.io/en/stable/user-guide/selective_sync/ — selective sync
- https://argo-cd.readthedocs.io/en/stable/user-guide/sync-options/ — prune, PruneLast, ApplyOutOfSyncOnly, Replace
- https://argo-cd.readthedocs.io/en/stable/user-guide/auto_sync/ — automated sync, prune, selfHeal
- https://fluxcd.io/flux/components/kustomize/kustomizations/ — prune, force, field manager, suspend (last ~21k chars unread)
- https://helmfile.readthedocs.io/en/latest/ — selector semantics
- https://helmfile.readthedocs.io/en/latest/releases/ and https://pkg.go.dev/github.com/helmfile/helmfile@v0.33.0 — needs flags, apply-runs-diff (search snippets only; the pkg.go.dev version is old, re-verify)
- https://nix.dev/manual/nix/stable/command-ref/new-cli/nix3-profile.html — profile generations
- https://nix-community.github.io/home-manager/usage/dotfiles.html — collisions, backup, force
- https://puppet.com/docs/pe/latest/infrastructure_reports.html — noop reporting (search snippet)
- https://oneuptime.com/blog/post/2026-02-21-ansible-playbook-check-mode-dry-run/view and https://engineering.nordeus.com/ansible-check-mode-tips/ — Ansible check mode (secondary, snippets; the Nordeus post date unknown, possibly old)
- https://dnf.readthedocs.io/en/latest/command_ref.html — dnf transactions, history
- https://doc.rust-lang.org/cargo/commands/cargo-update.html — cargo update spec semantics
- https://docs.astral.sh/uv/concepts/projects/sync/ — uv lock and sync
