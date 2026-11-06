"""A project without the settings kind upgrades unchanged.

The committed fixture under ``test/data/golden/pre_settings_47da12f/`` was
produced by the binary built at ``47da12f``, the last commit before the
``settings`` artifact kind reached install.  **This module is its consumer.**
What makes the assertions non-vacuous is the *provenance* of the bytes, not the
comparison: asserting current-binary-equals-current-binary would pass however
far the lock, the install state or the consent record drifted.  Never
regenerate the fixture from a tree that knows about settings -- see its
``README.md``.

The project declares every pre-existing kind (skill, rule, agent, mcp, hook,
bundle) and carries the consent record ``grim hook allow`` wrote for its hook.

The fixture's refs are pinned to ``localhost:5000`` and that host is baked into
every ``pinned =``, every consent key and both declaration hashes, so it cannot
be normalized at seed time without rewriting the bytes under test.  Without a
registry there this module skips; the deep verify job runs it alone with
``GRIM_REQUIRE_GOLDEN=1``, under which a skip is a failure.

The offline lock cases lock the **global** scope only, which declares direct
artifacts and no bundle, so the cache the offline lock needs is the one the
baseline warmed. The offline install case installs both scopes from a store an
online install warmed, into a fresh project and home.
"""

from __future__ import annotations

import difflib
import hashlib
import json
import os
import re
import shutil
import sys
from pathlib import Path

import pytest

from src import registry as reg

FIXTURE = (
    Path(__file__).resolve().parents[1] / "data" / "golden" / "pre_settings_47da12f"
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
        "test_the_install_state_matches_the_baseline",
        "test_an_offline_install_writes_the_baseline_state",
        "test_an_old_consent_record_still_arms_its_hook",
        "test_an_old_consent_record_still_gates_a_grown_declaration",
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
            f"golden/{name} (built at 47da12f)",
            f"actual/{name} (this binary)",
            lineterm="",
            n=2,
        )
    )
    pytest.fail(
        f"{name} is not identical to the baseline.\n\n"
        "The fixture is a record of the past, so a mismatch means THIS BINARY changed "
        "a project without settings's on-disk contract, which is a Principle 9 breaking "
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
            # Seeding is the upgrading user's state (a lock the baseline wrote), and it
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

    @property
    def dispatch_table(self) -> Path:
        return self.grim_home / "hooks" / "dispatch.json"

    def seed_consent(self, hooks: list[str] | None = None) -> Path:
        """Write the consent record `grim hook allow` wrote at the baseline.

        The golden record, with the two per-environment values filled in:
        `workspace` is the identity `evaluate` compares byte for byte, and the
        filename is its SHA-256. `hooks` overrides the consented set.
        """
        record = json.loads((GOLDEN / "consent.project.json").read_text())
        record["workspace"] = str(self.project)
        record["consented_at"] = "2026-01-01T00:00:00Z"
        if hooks is not None:
            record["hooks"] = hooks
        key = hashlib.sha256(str(self.project).encode()).hexdigest()
        path = self.grim_home / "hooks" / "consent" / f"{key}.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(record, indent=2) + "\n")
        return path

    def armed_rows(self) -> list[dict]:
        """Every row in the hook dispatch table, across every root."""
        if not self.dispatch_table.is_file():
            return []
        table = json.loads(self.dispatch_table.read_text())
        return [row for root in table["roots"].values() for row in root["hooks"]]

    def lock_and_install(self) -> None:
        # The upgrading user's project had consented through `grim hook allow`;
        # `--trust-hooks` would answer for this run only and prove nothing about
        # the record.
        self.seed_consent()
        self.project_runner.run("lock")
        self.project_runner.run("install", "--client", "claude")
        self.global_runner.run("lock", "--global")
        self.global_runner.run("install", "--global", "--client", "claude")


@pytest.mark.usefixtures("registry")
def test_golden_locks_reserialize_byte_identically(grim_binary, grim_at, tmp_path, request):
    """A lock the baseline wrote is parsed and written back unchanged.

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
    """`grim status` still reports every item with the `source` the baseline reported.

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
        assert got == want, f"{scope}: status `source` per item differs from the baseline"
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
            f"the {scope} declaration hash moved for a project without settings"
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
    """`lock --global --offline` from a warm baseline cache keeps the golden bytes.

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
    """`remove bundle reg-bundle` leaves the lock and state the baseline left."""
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


@pytest.mark.usefixtures("registry")
def test_the_install_state_matches_the_baseline(grim_binary, grim_at, tmp_path, request):
    """A first install writes the install state the baseline wrote, both scopes.

    Covers the pointers a registration-shaped kind could disturb: the mcp
    entry and its `adopted` flag, and the hook payload and registration.
    """
    _replay_registry()
    env = _Env(grim_at, tmp_path, seed_locks=True)
    env.lock_and_install()

    _assert_identical("state.project.json", GOLDEN / "state.project.json", env.project_state)
    _assert_identical("state.global.json", GOLDEN / "state.global.json", env.global_state)
    _completed.add(request.node.name)


@pytest.mark.usefixtures("registry")
def test_an_offline_install_writes_the_baseline_state(grim_binary, grim_at, tmp_path, request):
    """`install --offline` from a warm store writes the baseline's state, both scopes.

    One environment installs online to fill the store; a second, with a fresh
    project, home and install state, gets only that store's blobs and tags.
    Every layer it materializes therefore comes from the offline read path.
    """
    _replay_registry()
    warm = _Env(grim_at, tmp_path / "warm", seed_locks=True)
    warm.lock_and_install()
    env = _Env(grim_at, tmp_path / "offline", seed_locks=True)
    for store in ("blobs", "tags"):
        shutil.copytree(warm.grim_home / store, env.grim_home / store)
    env.seed_consent()

    env.project_runner.run("install", "--offline", "--client", "claude")
    env.global_runner.run("install", "--global", "--offline", "--client", "claude")

    _assert_identical("state.project.json", GOLDEN / "state.project.json", env.project_state)
    _assert_identical("state.global.json", GOLDEN / "state.global.json", env.global_state)
    _completed.add(request.node.name)


@pytest.mark.usefixtures("registry")
def test_an_old_consent_record_still_arms_its_hook(grim_binary, grim_at, tmp_path, request):
    """A record `grim hook allow` wrote, hook keys only, grants as it did.

    The control installs the same project with no record: nothing arms. Only
    then does arming with the record say anything about the record.
    """
    _replay_registry()
    control = _Env(grim_at, tmp_path / "control", seed_locks=True)
    control.project_runner.run("lock")
    control.project_runner.run("install", "--client", "claude")
    assert control.armed_rows() == [], "no record, no arming (positive control)"

    env = _Env(grim_at, tmp_path / "old-record", seed_locks=True)
    record = env.seed_consent()
    assert all(not key.startswith("settings:") for key in json.loads(record.read_text())["hooks"])
    env.project_runner.run("lock")
    env.project_runner.run("install", "--client", "claude")

    assert len(env.armed_rows()) == 1, "the old record arms the hook it names"
    status = env.project_runner.json("status")
    hook = [i for i in status["items"] if i["kind"] == "hook"]
    assert len(hook) == 1 and hook[0]["arming"] == [], hook
    _assert_identical("state.project.json", GOLDEN / "state.project.json", env.project_state)
    _completed.add(request.node.name)


@pytest.mark.usefixtures("registry")
def test_an_old_consent_record_still_gates_a_grown_declaration(
    grim_binary, grim_at, tmp_path, request
):
    """A record that does not cover the declared hook leaves it gated, as before."""
    _replay_registry()
    env = _Env(grim_at, tmp_path, seed_locks=True)
    env.seed_consent(hooks=[])
    env.project_runner.run("lock")

    result = env.project_runner.run("install", "--client", "claude")

    assert result.returncode == 0, "a withheld hook is not a failure"
    assert env.armed_rows() == [], "a record short of the declared hook arms nothing"
    status = env.project_runner.json("status")
    hook = [i for i in status["items"] if i["kind"] == "hook"]
    assert len(hook) == 1 and hook[0]["state"] == "gated", hook
    _completed.add(request.node.name)
