"""Contract C-015 — the hook kind did not disturb a hook-free project.

The committed fixture under ``test/data/golden/pre_hooks_03e59b0/`` was produced
by a ``grim`` built at ``03e59b0``, the last commit before any
``ArtifactKind::Hook`` work.  **This module is its consumer.**  Review finding
B6: the fixture, ~1,500 lines of baseline data plus a ``tools/verify.py``, was
referenced from no test, taskfile or workflow, so the Principle 9 evidence it
exists to provide was never actually checked.

What makes the assertion non-vacuous is the *provenance* of the bytes, not the
comparison itself: asserting current-binary-equals-current-binary would pass
however badly the lock format drifted.  Never regenerate the fixture from a tree
that knows about hooks -- see its ``README.md``, which explains why a refresh
silently deletes the only evidence while leaving a green test behind.

The lock is the sharpest target: the hook kind inserts ``[[hook]]`` between
``[[mcp]]`` and ``[[bundle]]``, and ``"hooks"`` between ``"bundles"`` and
``"mcp"`` in the JCS declaration document.  Both golden locks contain
``[[mcp]]`` *and* ``[[bundle]]``, so that adjacency is present in the data
rather than merely implied.
"""

from __future__ import annotations

import difflib
import json
import shutil
from pathlib import Path

import pytest

from src import registry as reg

FIXTURE = Path(__file__).resolve().parents[1] / "data" / "golden" / "pre_hooks_03e59b0"

# The host string is baked into every `pinned = …` in both golden locks and into
# both recorded `declaration_hash` values, so it cannot be normalized at seed
# time without rewriting the bytes under test — which is the whole assertion.
# The fixture's own README prescribes the answer: *skip rather than normalize*.
#
# This is a real gap rather than a formality. `test/conftest.py` deliberately
# puts the session registry on a **dynamic non-5000 port** whenever nothing
# answers on 5000, because 5000 sits in grim's built-in plain-HTTP allowlist and
# a rig there cannot distinguish "reached because `insecure = true` was
# declared" from "reached because loopback is always allowed". That is the
# normal CI path, so on a stock checkout this module skips, and the C-015
# evidence is only checked when a registry is already running on 5000.
#
# Making it unconditional needs the fixture regenerated against a host-neutral
# reference, which cannot be done from a tree that knows about hooks (see the
# README) — so it is a fixture-regeneration task, not a guard to tighten here.
_GOLDEN_HOST = "localhost:5000"

pytestmark = pytest.mark.skipif(
    reg.REGISTRY_HOST != _GOLDEN_HOST,
    reason=(
        f"the golden fixture's refs are pinned to {_GOLDEN_HOST}; this session's "
        f"registry is {reg.REGISTRY_HOST}. Start a registry on 5000 "
        "(`docker run -d --rm -p 5000:5000 --name grim-golden-registry registry:2`) "
        "to exercise the C-015 baseline."
    ),
)

# The empty OCI config blob every golden manifest points at.  Replayed
# explicitly: the manifests are byte-verbatim, so the blob they reference has to
# exist before the manifest PUT can succeed.
EMPTY_CONFIG = b"{}"


def _replay_registry() -> None:
    """Push the recorded artifacts back byte-for-byte.

    ``pushed.json`` holds each manifest's exact bytes and its layer as hex, so
    the replay reproduces every manifest digest in the golden locks **without
    depending on grim, on ``src.registry``'s packer, or on this repo's tar
    layout**.  That independence is the point: a change to how grim packs an
    artifact must not be able to move these digests.
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
            "verbatim, so a mismatch means the replay altered them -- not that the "
            "fixture is stale."
        )


def _assert_identical(name: str, expected: bytes, actual_path: Path) -> None:
    assert actual_path.exists(), f"{name}: grim did not write {actual_path}"
    actual = actual_path.read_bytes()
    if actual == expected:
        return
    diff = "\n".join(
        difflib.unified_diff(
            expected.decode().splitlines(),
            actual.decode().splitlines(),
            f"golden/{name} (built at 03e59b0)",
            f"actual/{name} (this binary)",
            lineterm="",
            n=2,
        )
    )
    pytest.fail(
        f"C-015 VIOLATED -- {name} is not byte-identical to the pre-hooks baseline.\n\n"
        "The fixture is a record of the past, so a mismatch means THIS BINARY changed a "
        "hook-free project's on-disk contract, which is a Principle 9 breaking change. "
        "Do not regenerate the fixture to make this pass.\n\n" + diff
    )


def _seed(grim_at, tmp_path, *, seed_locks: bool):
    """Lay out the baseline's environment and return its two runners.

    Shared by both tests because it is *setup*, not the assertion: the
    environment the baseline ran in is one fact, and two spellings of it would
    let the tests disagree about what they are comparing against.
    """
    home = tmp_path / "home"
    grim_home = tmp_path / "grim-home"
    project = tmp_path / "project"
    for directory in (home, grim_home):
        directory.mkdir(parents=True, exist_ok=True)
    shutil.copytree(FIXTURE / "project", project)
    shutil.copyfile(FIXTURE / "global-grimoire.toml", grim_home / "grimoire.toml")
    # The clients the baseline had present. Detection is directory-based, so
    # these have to exist or grim resolves a different client set and writes a
    # different state file for reasons unrelated to hooks.
    for client_dir in (".claude", ".copilot", ".codex"):
        (home / client_dir).mkdir(parents=True, exist_ok=True)

    if seed_locks:
        # Seeding both locks is what makes the `generated_at` preservation path
        # fire, which is why the comparison needs no normalization: an unseeded
        # run stamps a fresh timestamp, and the test would then have to blank the
        # one field most likely to reveal a real format change.
        golden = FIXTURE / "golden"
        shutil.copyfile(golden / "project.grimoire.lock", project / "grimoire.lock")
        shutil.copyfile(golden / "global.grimoire.lock", grim_home / "grimoire.lock")

    env = {
        "HOME": str(home),
        "USERPROFILE": str(home),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "GRIM_HOME": str(grim_home),
    }
    project_runner = grim_at(project)
    project_runner.env.update(env)
    global_runner = grim_at(grim_home)
    global_runner.env.update(env)
    return grim_home, project, project_runner, global_runner


@pytest.mark.usefixtures("registry")
def test_a_hook_free_project_matches_the_pre_hooks_baseline_c015(
    grim_binary, grim_at, tmp_path
):
    """Seed the golden locks, converge, and compare every assertion target."""
    _replay_registry()
    grim_home, project, runner, global_runner = _seed(grim_at, tmp_path, seed_locks=True)

    runner.run("lock")
    runner.run("install", "--client", "claude")
    global_runner.run("lock", "--global")
    global_runner.run("install", "--global", "--client", "claude")

    golden = FIXTURE / "golden"
    for name, actual in (
        ("project.grimoire.lock", project / "grimoire.lock"),
        ("global.grimoire.lock", grim_home / "grimoire.lock"),
        ("state.global.json", grim_home / "state" / "global.json"),
        ("state.project.json", project / ".grimoire" / "state.json"),
    ):
        _assert_identical(name, (golden / name).read_bytes(), actual)


@pytest.mark.usefixtures("registry")
def test_the_declaration_hash_is_unchanged_and_still_version_1_c015(
    grim_binary, grim_at, tmp_path
):
    """The JCS declaration document is the other half of C-015.

    Asserted separately from the lock bytes because it answers a different
    question: the lock could match while the hash algorithm or its field order
    changed, and a hash change silently re-resolves every declaration on every
    existing install. Run **unseeded** so the hash is recomputed from the
    declaration rather than preserved from the seeded lock -- seeding it here
    would make the assertion vacuous.
    """
    _replay_registry()
    grim_home, project, runner, global_runner = _seed(grim_at, tmp_path, seed_locks=False)

    runner.run("lock")
    global_runner.run("lock", "--global")

    expected = json.loads((FIXTURE / "golden" / "declaration_hash.json").read_text())

    def hash_field(lock: Path, field: str) -> str:
        for line in lock.read_text().splitlines():
            if line.startswith(f"{field} ="):
                return line.split("=", 1)[1].strip().strip('"')
        pytest.fail(f"{lock} carries no `{field}` -- the lock format changed shape")

    for scope, lock in (
        ("project", project / "grimoire.lock"),
        ("global", grim_home / "grimoire.lock"),
    ):
        assert hash_field(lock, "declaration_hash") == expected[scope], (
            f"C-015 VIOLATED -- the {scope} declaration hash moved for a hook-free "
            "project. The hook kind must contribute `hooks` to the JCS document only "
            "when non-empty; a hash change re-resolves every existing install."
        )
    assert hash_field(project / "grimoire.lock", "declaration_hash_version") == str(
        expected["declaration_hash_version"]
    ), "DECLARATION_HASH_VERSION must stay 1 -- bumping it invalidates every lock"
