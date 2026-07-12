---
paths:
  - test/**
---

# Test Subsystem

Pytest acceptance tests run the compiled `grim` binary against a real OCI registry, at `test/`.

## Design Rationale

Pytest (not Rust integration tests) because acceptance tests exercise the real compiled binary against a real OCI registry — catches issues mocked unit tests miss. The registry host is resolved once per session in `pytest_configure` (before any test module imports `src.registry`), and a `registry:2` container is started via `docker run -d --rm` when nothing already answers on `localhost:5000`. UUID-prefixed repo names (`unique_repo`) give per-test isolation on the shared registry — no per-test cleanup needed. See `arch-principles.md` for the full pattern catalog.

## Structure

| Path | Purpose |
|------|---------|
| `test/conftest.py` | Session fixtures (`grim_binary`, `registry`) + `pytest_configure`/`pytest_unconfigure` registry lifecycle; function fixtures `grim_home`, `grim`, `unique_repo` |
| `test/tests/conftest.py` | Function fixtures local to `tests/`: `project_dir`, `grim_at` |
| `test/src/runner.py` | `GrimRunner` — subprocess wrapper with per-instance env isolation |
| `test/src/helpers.py` | `write_config()`, `make_bundle()`, `make_artifact()` — build + push test fixtures |
| `test/src/assertions.py` | Cross-platform path assertion helpers |
| `test/src/registry.py` | `PublishedArtifact` + minimal stdlib OCI client (push/fetch/retag) |
| `test/taskfile.yml` | Task runner: `build` (internal), `default`, `quick`, `parallel` |

## Key Fixtures

| Fixture | Scope | Defined in | Purpose |
|---------|-------|-----------|---------|
| `grim_binary` | session | `conftest.py` | Path to the `grim` binary — `$GRIM_COMMAND` env if set, else `test/bin/grim(.exe)` |
| `registry` | session | `conftest.py` | Registry host string (e.g. `"localhost:5000"`); skips the test if unreachable |
| `grim_home` | function | `conftest.py` | Isolated `tmp_path/grim-home` dir used as `GRIM_HOME` |
| `grim` | function | `conftest.py` | `GrimRunner(grim_binary, grim_home)` — no cwd set |
| `unique_repo` | function | `conftest.py` | UUID-prefixed repo name: `f"grim-test/{uuid4().hex[:12]}"` |
| `project_dir` | function | `tests/conftest.py` | Empty workspace dir (`tmp_path/project`) a project-scope `grim` runs inside |
| `grim_at` | function | `tests/conftest.py` | Factory `(cwd: Path) -> GrimRunner` — project-scope commands (`init`, `lock`, `install`) discover config by walking up from cwd |

## GrimRunner API

```python
runner = GrimRunner(binary, grim_home, cwd=None)
runner.run(*args, format=None, check=True, log_level=None)  # -> CompletedProcess
runner.json(*args, **kwargs)                                 # run with format="json", parse stdout
runner.plain(*args, **kwargs)                                # run without --format
runner.home                                                   # isolated $HOME (sibling of grim_home)
```

Per-instance `env` dict: `GRIM_HOME`, `PATH`, `HOME` + `USERPROFILE` (both point at the isolated home, sibling of `grim_home` — grim reads `USERPROFILE` on Windows, `HOME` elsewhere), `XDG_CONFIG_HOME` (`<home>/.config`), plus Windows spawn vars (`SYSTEMROOT`, `TEMP`, `TMP`, `PATHEXT`) when present. `GRIM_INSECURE_REGISTRIES` is set to the test registry host whenever it differs from grim's built-in HTTP allowlist (`localhost[:5000]`, `127.0.0.1[:5000]`).

## Test Data Helpers

`test/src/helpers.py` — no `make_package()`; the real helpers are:

- `write_config(project_dir, skills=None, rules=None, bundles=None, agents=None) -> Path` — writes `grimoire.toml` from `{name: fq_ref}` dicts
- `make_artifact(repo, kind, files, tag="latest", annotations=None) -> PublishedArtifact` — tars `{path: content}` and pushes a single-layer skill/rule artifact
- `make_bundle(repo, members, tag="latest") -> PublishedArtifact` — pushes a bundle whose layer lists `(kind, name, id)` member tuples

`test/src/registry.py` provides the lower-level push/fetch primitives these call: `push_artifact()`, `fetch_manifest()`, `fetch_blob()`, `retag()`, `tag_digest()`, `registry_reachable()`.

### PublishedArtifact

Returned by `make_artifact()`, `make_bundle()`, and the `registry.py` primitives:

| Field / property | Example |
|-------|---------|
| `repo` | `"grim-test/ab12cd34ef56/code-review"` |
| `tag` | `"stable"` |
| `digest` | `"sha256:…"` (manifest digest) |
| `kind` | `"skill"` |
| `.fq` | `"localhost:5000/grim-test/…/code-review:stable"` |
| `.pinned` | `"localhost:5000/grim-test/…/code-review@sha256:…"` |

## Assertion Helpers

- `assert_path_exists(path)` — exists (file, dir, or symlink)
- `assert_dir_exists(path)` — is directory
- `assert_symlink_exists(path)` — is symlink or Windows junction
- `assert_not_exists(path)` — not exist and not symlink

**Always use `assert_symlink_exists()` instead of `path.is_symlink()`** for Windows junction compat.

## Test Isolation

- **Per-test `GRIM_HOME`**: `grim_home` fixture gives each test an isolated `tmp_path` dir
- **Per-test `HOME`**: `GrimRunner` isolates `$HOME` too (sibling of `grim_home`), so global-scope installs never touch the developer's real home
- **UUID repo names**: `unique_repo` fixture prevents collisions on the shared registry
- **Shared registry**: session-scoped; all tests push/pull the same container (or a fresh throwaway one if the default is polluted with >500 repos — see `conftest.py`)
- **Minimal env**: `GrimRunner` strips ambient env; only `PATH`, `HOME`, `GRIM_*`, `XDG_CONFIG_HOME` (+ Windows spawn vars)

## Running Tests

```bash
task test              # build grim, copy to test/bin/grim, run full pytest suite
task test:quick        # skip rebuild (SKIP_BUILD=true), pytest -n auto
task test:parallel     # rebuild-aware (sources track src/tests), pytest -n auto

# Single test (binary must already exist at test/bin/grim, or set GRIM_COMMAND):
cd test && uv run pytest tests/test_install.py::test_lock_then_install_materializes_files -v
```

There is no `--no-build` pytest flag — build/rebuild is controlled entirely by the `task` layer (`SKIP_BUILD` var / `CI` env), not by a pytest CLI option.

## Adding a New Test

1. Add a function to the appropriate `test/tests/test_*.py` (or create a new file)
2. Use `grim_at`/`project_dir` for project-scope commands, or `grim`/`grim_home` for global-scope
3. Build fixture data with `write_config()` + `make_artifact()`/`make_bundle()` and `unique_repo`
4. Assert via `runner.json(...)`/`runner.run(...)` return values and `src/assertions.py` helpers
5. Run: `cd test && uv run pytest tests/test_file.py::test_name -v`

## Test Files

`test/tests/*.py` cover: install/uninstall/update/lock, config + registry management (incl. TUI options, default registry), add/remove (+ infer), init, release (+ non-version tags) and publish (+ announce), login, search (+ namespaced), status, targets, integrity, deprecation, metadata, multifile rules, bundles, agents, mcp (core, artifact, fetch), client rendering + desync + TUI multi-registry, global scope, git provenance, state portability, index source, docs, build, smoke, and cross-cutting workflows.

## Quality Gate

During review-fix loops, run `task test:parallel` (or `task test:quick` if the binary is already fresh) — not full `task verify`. Acceptance tests only; no need to re-run the Rust unit-test/lint gates.
