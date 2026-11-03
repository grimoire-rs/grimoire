"""A dependency-free project upgrades unchanged.

The committed fixture under ``test/data/golden/pre_dependencies_cdeedcaa/`` was
produced by grim 0.15.1 (``cdeedcaa``), the last release before declared
artifact dependencies.  **This module is its consumer.**  What makes the
assertions non-vacuous is the *provenance* of the bytes, not the comparison:
asserting current-binary-equals-current-binary would pass however far the lock
format drifted.  Never regenerate the fixture from a tree that knows about
dependencies -- see its ``README.md``.

The fixture's refs are pinned to ``localhost:5000`` and that host is baked into
every ``pinned =`` and both declaration hashes, so it cannot be normalized at
seed time without rewriting the bytes under test.  Without a registry there this
module skips; the deep verify job runs it alone with ``GRIM_REQUIRE_GOLDEN=1``,
under which a skip is a failure.

The offline cases lock the **global** scope only: 0.15.1 cannot lock a registry
bundle offline (its bundle manifest fetch is online-only, exit 69), so the
project, which declares one, has no offline baseline to compare against.
"""

from __future__ import annotations

import difflib
import json
import os
import re
import shutil
import sys
from pathlib import Path

import pytest

from src import registry as reg

FIXTURE = (
    Path(__file__).resolve().parents[1] / "data" / "golden" / "pre_dependencies_cdeedcaa"
)
GOLDEN = FIXTURE / "golden"

_GOLDEN_HOST = "localhost:5000"

_HOW_TO_RUN = (
    f"the golden fixture's refs are pinned to {_GOLDEN_HOST}; this session's "
    f"registry is {reg.REGISTRY_HOST}. Start a registry on 5000 "
    "(`docker run -d --rm -p 5000:5000 --name grim-golden-registry registry:2`) "
    f"and set GRIM_TEST_REGISTRY_HOST={_GOLDEN_HOST} to exercise the baseline."
)

REQUIRED = os.environ.get("GRIM_REQUIRE_GOLDEN") == "1"
_GOLDEN_TESTS = frozenset(
    {
        "test_golden_locks_reserialize_byte_identically",
        "test_golden_status_sources_are_unchanged",
        "test_a_fresh_lock_matches_the_baseline_modulo_stamps",
        "test_offline_lock_matches_the_baseline[tag-cache]",
        "test_offline_lock_matches_the_baseline[tag-cache-no-manifests]",
        "test_offline_lock_without_a_cache_still_fails",
        "test_remove_bundle_drops_what_the_baseline_dropped",
    }
)

if REQUIRED:
    if reg.REGISTRY_HOST != _GOLDEN_HOST:
        pytest.fail(f"GRIM_REQUIRE_GOLDEN=1: {_HOW_TO_RUN}", pytrace=False)
    if os.environ.get("PYTEST_XDIST_WORKER"):
        # The ran-all check below is per process; xdist would split the set.
        pytest.fail(
            "GRIM_REQUIRE_GOLDEN=1: run this module alone with `-n 0`", pytrace=False
        )
else:
    pytestmark = pytest.mark.skipif(
        reg.REGISTRY_HOST != _GOLDEN_HOST, reason=_HOW_TO_RUN
    )

_completed: set[str] = set()


@pytest.fixture(scope="module", autouse=True)
def _every_golden_test_ran():
    """Under GRIM_REQUIRE_GOLDEN=1, fail unless every test ran to the end.

    Catches each skip route uniformly (the ``registry`` fixture, the host
    managed-settings guard in ``GrimRunner.run``, a ``-k`` that deselects one)
    because a test only records itself after its last assertion.
    """
    yield
    if REQUIRED:
        missing = sorted(_GOLDEN_TESTS - _completed)
        assert not missing, (
            f"GRIM_REQUIRE_GOLDEN=1 but these golden tests did not pass: {missing}"
        )


# The manifests are byte-verbatim, so the empty config blob they reference has
# to exist before the manifest PUT can succeed.
EMPTY_CONFIG = b"{}"


def _replay_registry() -> None:
    """Push the recorded artifacts back byte-for-byte.

    Independent of grim and of ``src.registry``'s packer: a change to how grim
    packs an artifact must not be able to move the digests in the golden locks.
    """
    recorded = json.loads((FIXTURE / "registry" / "pushed.json").read_text())
    for entry in recorded:
        repo = entry["repo"]
        reg._push_blob(repo, EMPTY_CONFIG)
        reg._push_blob(repo, bytes.fromhex(entry["layer"]))
        digest = reg._put(
            f"{reg.REGISTRY_BASE}/v2/{repo}/manifests/{entry['tag']}",
            entry["manifest"].encode(),
            "application/vnd.oci.image.manifest.v1+json",
        )
        assert digest == entry["manifest_digest"], (
            f"{repo}: the registry computed {digest} for the replayed manifest but the "
            f"fixture recorded {entry['manifest_digest']}. The manifest bytes are stored "
            "verbatim, so a mismatch means the replay altered them, not that the "
            "fixture is stale."
        )


# `generated_by` is stamped by every lock save, so it moves at every release,
# not with the format; `generated_at` is stamped on a fresh resolve. They are
# the only masks, and the line must still be present and shaped as before.
_GENERATED_BY = re.compile(rb'^generated_by = "grim \d+\.\d+\.\d+[^"]*"$', re.MULTILINE)
_GENERATED_AT = re.compile(rb"^generated_at = .*$", re.MULTILINE)
# The project hook payload path embeds the SHA-256 of the workspace directory.
_WORKSPACE_KEY = re.compile(rb"hooks/payload/[0-9a-f]{64}/")


def _normalize(name: str, data: bytes, *, mask_generated_at: bool) -> bytes:
    data = _WORKSPACE_KEY.sub(b"hooks/payload/<WORKSPACE-KEY>/", data)
    if name.endswith(".lock"):
        assert _GENERATED_BY.search(data), f"{name}: the `generated_by` line changed shape"
        data = _GENERATED_BY.sub(b'generated_by = "grim <version>"', data)
        if mask_generated_at:
            assert _GENERATED_AT.search(data), f"{name}: no `generated_at` line"
            data = _GENERATED_AT.sub(b'generated_at = "<NORMALIZED>"', data)
    return data


def _assert_identical(
    name: str, expected_path: Path, actual_path: Path, *, mask_generated_at: bool = False
) -> None:
    assert actual_path.exists(), f"{name}: grim did not write {actual_path}"
    expected = _normalize(
        name, expected_path.read_bytes(), mask_generated_at=mask_generated_at
    )
    actual = _normalize(
        name, actual_path.read_bytes(), mask_generated_at=mask_generated_at
    )
    if actual == expected:
        return
    diff = "\n".join(
        difflib.unified_diff(
            expected.decode().splitlines(),
            actual.decode().splitlines(),
            f"golden/{name} (built at cdeedcaa)",
            f"actual/{name} (this binary)",
            lineterm="",
            n=2,
        )
    )
    pytest.fail(
        f"{name} is not identical to the 0.15.1 baseline.\n\n"
        "The fixture is a record of the past, so a mismatch means THIS BINARY changed "
        "a dependency-free project's on-disk contract, which is a Principle 9 breaking "
        "change. Do not regenerate the fixture to make this pass.\n\n" + diff
    )


class _Env:
    """The baseline's environment: one project scope and one global scope."""

    def __init__(self, grim_at, tmp_path: Path, *, seed_locks: bool):
        home = tmp_path / "home"
        self.grim_home = tmp_path / "grim-home"
        self.project = tmp_path / "project"
        for directory in (home, self.grim_home):
            directory.mkdir(parents=True, exist_ok=True)
        shutil.copytree(FIXTURE / "project", self.project)
        shutil.copyfile(FIXTURE / "global-grimoire.toml", self.grim_home / "grimoire.toml")
        # Client detection is directory-based; the baseline had these present.
        for client_dir in (".claude", ".copilot", ".codex"):
            (home / client_dir).mkdir(parents=True, exist_ok=True)
        if seed_locks:
            # Seeding is the upgrading user's state (a lock 0.15.1 wrote), and it
            # makes `generated_at` preserved so no timestamp needs masking.
            shutil.copyfile(
                GOLDEN / "project.grimoire.lock", self.project / "grimoire.lock"
            )
            shutil.copyfile(
                GOLDEN / "global.grimoire.lock", self.grim_home / "grimoire.lock"
            )
        env = {
            "HOME": str(home),
            "USERPROFILE": str(home),
            "XDG_CONFIG_HOME": str(home / ".config"),
            "GRIM_HOME": str(self.grim_home),
        }
        self.project_runner = grim_at(self.project)
        self.project_runner.env.update(env)
        self.global_runner = grim_at(self.grim_home)
        self.global_runner.env.update(env)

    @property
    def project_lock(self) -> Path:
        return self.project / "grimoire.lock"

    @property
    def global_lock(self) -> Path:
        return self.grim_home / "grimoire.lock"

    @property
    def project_state(self) -> Path:
        return self.project / ".grimoire" / "state.json"

    @property
    def global_state(self) -> Path:
        return self.grim_home / "state" / "global.json"

    def lock_and_install(self) -> None:
        self.project_runner.run("lock")
        self.project_runner.run("install", "--client", "claude", "--trust-hooks")
        self.global_runner.run("lock", "--global")
        self.global_runner.run("install", "--global", "--client", "claude")


@pytest.mark.usefixtures("registry")
def test_golden_locks_reserialize_byte_identically(grim_binary, grim_at, tmp_path, request):
    """A lock 0.15.1 wrote is parsed and written back unchanged.

    Seeded, so the relock keeps `generated_at` and every other byte comes from
    parsing the golden lock and serializing the parsed model.
    """
    _replay_registry()
    env = _Env(grim_at, tmp_path, seed_locks=True)

    env.project_runner.run("lock")
    env.global_runner.run("lock", "--global")

    _assert_identical("project.grimoire.lock", GOLDEN / "project.grimoire.lock", env.project_lock)
    _assert_identical("global.grimoire.lock", GOLDEN / "global.grimoire.lock", env.global_lock)
    _completed.add(request.node.name)


@pytest.mark.usefixtures("registry")
def test_golden_status_sources_are_unchanged(grim_binary, grim_at, tmp_path, request):
    """`grim status` still reports every item with the `source` 0.15.1 reported.

    Only `source` is compared: later versions add fields to each row, and
    install paths embed the environment root.
    """
    _replay_registry()
    env = _Env(grim_at, tmp_path, seed_locks=True)
    env.lock_and_install()

    for scope, runner, args in (
        ("project", env.project_runner, ()),
        ("global", env.global_runner, ("--global",)),
    ):
        expected = json.loads((FIXTURE / "provenance" / f"status.{scope}.json").read_text())
        actual = runner.json("status", *args)
        want = {(i["kind"], i["name"]): i["source"] for i in expected["items"]}
        got = {(i["kind"], i["name"]): i["source"] for i in actual["items"]}
        assert want, f"{scope}: the recorded status carries no items (positive control)"
        assert got == want, f"{scope}: status `source` per item differs from 0.15.1"
    _completed.add(request.node.name)


@pytest.mark.usefixtures("registry")
def test_a_fresh_lock_matches_the_baseline_modulo_stamps(
    grim_binary, grim_at, tmp_path, request
):
    """Resolving from the declaration alone writes the golden bytes.

    Unseeded, so the lock and the declaration hash are recomputed rather than
    preserved; only the two timestamp/version stamps are masked.
    """
    _replay_registry()
    env = _Env(grim_at, tmp_path, seed_locks=False)
    assert not env.project_lock.exists() and not env.global_lock.exists()

    env.project_runner.run("lock")
    env.global_runner.run("lock", "--global")

    _assert_identical(
        "project.grimoire.lock",
        GOLDEN / "project.grimoire.lock",
        env.project_lock,
        mask_generated_at=True,
    )
    _assert_identical(
        "global.grimoire.lock",
        GOLDEN / "global.grimoire.lock",
        env.global_lock,
        mask_generated_at=True,
    )
    # The declaration hash is asserted by name too: the byte compare above would
    # report a moved hash as an opaque diff, and a hash change silently
    # re-resolves every existing install.
    expected = json.loads((GOLDEN / "declaration_hash.json").read_text())
    for scope, lock in (("project", env.project_lock), ("global", env.global_lock)):
        assert f'declaration_hash = "{expected[scope]}"' in lock.read_text(), (
            f"the {scope} declaration hash moved for a dependency-free project"
        )
    assert (
        f"declaration_hash_version = {expected['declaration_hash_version']}"
        in env.project_lock.read_text()
    )
    _completed.add(request.node.name)


def _seed_tag_cache(grim_home: Path, variant: str) -> None:
    for registry_dir in sorted((GOLDEN / variant / "tags").iterdir()):
        # The fixture stores `host_port`; Unix grim reads `host:port`.
        name = (
            registry_dir.name
            if sys.platform == "win32"
            else registry_dir.name.replace("_", ":", 1)
        )
        shutil.copytree(registry_dir, grim_home / "tags" / name)
    if (GOLDEN / variant / "blobs").is_dir():
        shutil.copytree(GOLDEN / variant / "blobs", grim_home / "blobs")


@pytest.mark.usefixtures("registry")
@pytest.mark.parametrize("variant", ["tag-cache", "tag-cache-no-manifests"])
def test_offline_lock_matches_the_baseline(
    grim_binary, grim_at, tmp_path, request, variant
):
    """`lock --global --offline` from a warm 0.15.1 cache keeps the golden bytes.

    `tag-cache` also holds the manifest blobs (the store-hit branch);
    `tag-cache-no-manifests` holds only the tags (the carry-forward branch).
    """
    env = _Env(grim_at, tmp_path, seed_locks=True)
    _seed_tag_cache(env.grim_home, variant)

    env.global_runner.run("lock", "--global", "--offline")

    _assert_identical("global.grimoire.lock", GOLDEN / "global.grimoire.lock", env.global_lock)
    _completed.add(request.node.name)


@pytest.mark.usefixtures("registry")
def test_offline_lock_without_a_cache_still_fails(grim_binary, grim_at, tmp_path, request):
    """Positive control for the two offline cases: the cache is what carries them."""
    env = _Env(grim_at, tmp_path, seed_locks=True)

    result = env.global_runner.run("lock", "--global", "--offline", check=False)

    assert result.returncode == 69, result.stderr
    _completed.add(request.node.name)


@pytest.mark.usefixtures("registry")
def test_remove_bundle_drops_what_the_baseline_dropped(
    grim_binary, grim_at, tmp_path, request
):
    """`remove bundle reg-bundle` leaves the lock and state 0.15.1 left."""
    _replay_registry()
    env = _Env(grim_at, tmp_path, seed_locks=True)
    env.lock_and_install()
    # Positive control: the bundle and its single-provenance member are there
    # before the remove, so the compare below is not over an empty project.
    before = env.project_lock.read_text()
    assert "bundled-rule" in before and "reg-bundle" in before

    env.project_runner.run("remove", "bundle", "reg-bundle")

    _assert_identical(
        "project.after-remove.grimoire.lock",
        GOLDEN / "project.after-remove.grimoire.lock",
        env.project_lock,
    )
    _assert_identical(
        "state.project.after-remove.json",
        GOLDEN / "state.project.after-remove.json",
        env.project_state,
    )
    _completed.add(request.node.name)
