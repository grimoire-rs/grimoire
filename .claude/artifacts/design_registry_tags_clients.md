# Design: registry/tags/kind/clients fixes

Status: in progress. Branch `fix/registry-tags-types`. Consolidates six
user-reported issues. Scope is mostly bugfix + small CLI/config changes;
provisional project, so no on-disk migration shims (KISS/YAGNI).

## Issues & decisions

### 1. Non-version tags on push (`grim release`)
- `cascade_tags` in `src/oci/release.rs` is the only semver gate; the
  resolver does no cascade walk (cascade is publish-side only).
- Rename `cascade_tags` → `publish_tags`. Behaviour:
  - full semver `X.Y.Z` → `[X.Y.Z, X.Y, X, latest]` (unchanged)
  - prerelease → `[exact]` (unchanged)
  - **non-semver tag (e.g. `canary`, `1.2`) → `[tag]`** single tag, no cascade
  - empty/no tag → new `ReleaseErrorKind::MissingTag` (DataError 65)
- `move_tags`/guard already work for a one-element set.

### 2. Default registry is CLI-only convenience; lock+config stay FQ
- Persistence is already fully-qualified (Identifier::Display in config
  write, PinnedIdentifier::Display in lock). No leak.
- Bug: `add`/`release` only honour `--registry`/`GRIM_DEFAULT_REGISTRY`,
  not config `[options].default_registry` (search/tui already do).
- Fix: shared `command::effective_default_registry(config, ctx)` =
  config default first, else ctx default. `add` threads
  `scope.options.default_registry`; `release` best-effort discovers a
  project config. `parse_reference` takes `Option<&str>` default.

### 3. TUI/search empty under a namespaced default registry
- `RegistryClient::list_catalog` built `{scheme}://{registry}/v2/_catalog`
  verbatim; a namespaced `default_registry` (`ghcr.io/acme`) → malformed
  URL → degrades to empty → silent blank TUI.
- Fix: extract the registry **host** (first path segment) for the
  `_catalog` URL + scheme. In `Catalog::build`, split the configured
  registry into `(host, namespace)`; query the host, filter repos by the
  namespace prefix, and build each entry with `registry = host`,
  `repository = <full repo path>` so `repo()` stays consistent with how
  identifiers parse (`ghcr.io` + `acme/code-review`). Cache identity keeps
  the full configured string.
- UX: when online and the rebuilt catalog is empty, the TUI status hints
  that the registry may not support `_catalog` (no more silent blank).

### 4. Persist artifact kind in OCI; make `add` kind optional
- Kind is already stamped as the `com.grimoire.kind` annotation at push
  and read by the catalog — `add` just never used it.
- `ArtifactKind::from_annotation(&str) -> Option<Self>` +
  `pub const KIND_ANNOTATION` + `annotations::kind_from_manifest`.
- `add` infers the kind from the pulled manifest when `--kind` is absent;
  an absent/unparseable annotation (or offline miss) is a clear error
  asking for `--kind`.

### 5. Rename `editor` config → `client`, make it an array
- `ConfigOptions.editor: Option<String>` → `clients: Vec<String>` (serde
  key `clients`). `EditorTarget` → `ClientTarget`, `EditorRecord` →
  `ClientRecord` (`editor` field → `client`), `editor_outputs` →
  `client_outputs`, install-state `editors` → `clients`, TUI
  `editor_default` → `clients_default: Vec<String>`,
  `UnsupportedEditor` → `UnsupportedClient`, CLI `--target` → `--client`.
- Install already loops over all targets, so multi-client generation
  works once the config list is threaded through. Default stays `[claude]`.

### 6. `grim add` name optional
- New `add` CLI: `grim add [--kind K] [--name N] <reference>`; reference
  is the sole positional. Name defaults to `Identifier::name()` (the
  repository's last segment).

## CLI contract changes (provisional, tests updated)
- `grim add <kind> <name> <ref>` → `grim add [--kind] [--name] <ref>`
- `grim install/update --target` → `--client`

## Verification
- Rust unit tests updated/added per module.
- Acceptance tests: non-version release tag, add inference + name default,
  config default-registry expansion persisted FQ, namespaced search,
  multi-client install. Full `task verify`, then land on main + push.

## PROGRESS (resume point)
Branch `fix/registry-tags-types`. All 6 Rust impls DONE; `cargo test --bin
grim` = 488 pass; `cargo check --tests` clean.

Implemented (src):
- oci/artifact_kind.rs: `KIND_ANNOTATION` const, `from_annotation`.
- oci/annotations.rs: const + `kind_from_manifest`.
- oci/release.rs: `cascade_tags`→`publish_tags` (non-semver ⇒ single tag,
  empty ⇒ `ReleaseErrorKind::MissingTag`). error.rs classifies MissingTag.
- command/release.rs: `publish_tags`, `parse_reference(ref, Option<&str>)`,
  `release_default_registry` (config default first).
- command.rs: `effective_default_registry(config, ctx)` helper.
- command/add.rs: NEW CLI `add [--kind/-k] [--name/-n] <reference>`; kind
  inferred via `infer_kind` (resolve_digest Query → fetch_manifest →
  kind_from_manifest); name defaults to `id.name()`; default-registry
  threaded; write_config writes `clients = [..]`. command_error.rs:
  `KindInferenceFailed` (DataError).
- catalog/registry_catalog.rs: `split_host_namespace`; build lists host,
  filters by namespace, entries rooted at host.
- oci/access/registry_client.rs: `registry_host` for `_catalog` URL/scheme.
- tui/app.rs: empty-online-catalog UX hint.
- RENAME editor→client: config `editor:Option<String>`→`clients:Vec<String>`
  (serde key `clients`); `EditorTarget`→`ClientTarget` (file
  client_target.rs); `EditorRecord`→`ClientRecord`(field `client`);
  install-state `editors`→`clients`; `editor_outputs`→`client_outputs`;
  `UnsupportedEditor`→`UnsupportedClient`; TUI `editor_default`→
  `clients_default:Vec<String>`; CLI `--target`→`--client` (install,update);
  InstallTarget::parse(ws, flags:&[String], config_default:&[String]).

Tests updated (test/): test_targets.py (--client), test_add_remove.py +
test_bundles.py (new add CLI / inference).

REMAINING:
1. NEW acceptance tests (test/tests/) — delegated.
2. Docs (docs/src/{configuration,commands,concepts,quickstart,introduction}.md)
   editor→clients, --client, add CLI — delegated.
3. `task verify` (full gate), fix any clippy/fmt.
4. Finalize → fast-forward main → push → CI green.

Test harness facts: GrimRunner.env is a mutable dict — set
`runner.env["GRIM_DEFAULT_REGISTRY"]=registry` to test env default. Config
default tested via write_config + a manual `[options]` block. Fixtures:
grim_at(project_dir), registry, unique_repo, project_dir. make_artifact/
make_bundle/write_config in src/helpers.py. Registry is live localhost:5000;
inference works online (default). `task verify` is final gate.
