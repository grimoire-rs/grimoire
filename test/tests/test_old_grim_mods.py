"""What the grims released before the ``mod`` kind do with it.

Every row runs against grim 0.15.1 (``GRIM_OLD_BIN``) and grim 0.16.0
(``GRIM_OLD_BIN_0_16_0``), each checked by ``src.old_grim``; a leg skips when
its variable is unset.  The old binary reads what a newer grim declared,
locked or published, in the same project and ``GRIM_HOME``, as it would on one
machine.  Fixtures are written by hand or pushed through the helpers, never
built by the grim under test.

Most rows are refusals, so each one carries a positive control in the same
test: the project is first made to work with the old binary, then snapshotted,
and the refusal must leave that snapshot untouched.  The rows where an old
grim does *not* refuse (a silent lock rewrite, a feature annotation it
ignores) pin that behaviour too, since a newer grim cannot repair it.
"""

from __future__ import annotations

import hashlib
import json
from collections.abc import Callable
from pathlib import Path

import pytest

from src import registry as reg
from src.helpers import (
    _REQUIRES_LAYER_MEDIA_TYPE,
    make_bundle,
    make_dependent,
    make_hook,
    make_mod,
    members_layer,
    mod_sandbox,
    tar_of,
    write_config,
)
from src.old_grim import (
    OLD_BIN,
    OLD_BIN_0_16_0,
    checked_old_grim,
    checked_old_grim_0_16_0,
)
from src.registry import REGISTRY_HOST, push_artifact, push_manifest
from src.runner import GrimRunner

FEATURES_KEY = "com.grimoire.requires-features"
SKILL = {"s/SKILL.md": "---\nname: s\ndescription: a skill\n---\n# s\n"}
V15, V16 = "0.15.1", "0.16.0"


@pytest.fixture(scope="module", params=[V15, V16])
def old_bin(
    request: pytest.FixtureRequest, tmp_path_factory: pytest.TempPathFactory
) -> Path:
    root = tmp_path_factory.mktemp("old-grim-identity")
    if request.param == V15:
        if not OLD_BIN:
            pytest.skip("GRIM_OLD_BIN names no grim 0.15.1 binary")
        return checked_old_grim(root)
    if not OLD_BIN_0_16_0:
        pytest.skip("GRIM_OLD_BIN_0_16_0 names no grim 0.16.0 binary")
    return checked_old_grim_0_16_0(root)


@pytest.fixture()
def version(old_bin: Path, request: pytest.FixtureRequest) -> str:
    return request.node.callspec.params["old_bin"]


@pytest.fixture()
def current(grim_at, project_dir: Path, registry: str) -> GrimRunner:
    return grim_at(project_dir)


@pytest.fixture()
def old(old_bin: Path, current: GrimRunner, project_dir: Path) -> GrimRunner:
    return GrimRunner(old_bin, current.grim_home, cwd=project_dir)


def _bytes(path: Path) -> bytes | None:
    return path.read_bytes() if path.is_file() else None


def _snapshot(project: Path) -> dict[str, bytes | None]:
    """Every path under the project, files by content and directories as None."""
    return {str(p.relative_to(project)): _bytes(p) for p in sorted(project.rglob("*"))}


@pytest.fixture()
def skill(unique_repo: str) -> reg.PublishedArtifact:
    return push_artifact(f"{unique_repo}/s", "1", tar_of(SKILL), "skill")


@pytest.fixture()
def mod(unique_repo: str) -> reg.PublishedArtifact:
    return make_mod(f"{unique_repo}/m", "1")


@pytest.fixture()
def working(
    old: GrimRunner, project_dir: Path, skill: reg.PublishedArtifact
) -> reg.PublishedArtifact:
    """A project the old grim locked and installed; the control for every refusal."""
    write_config(project_dir, skills={"s": skill.fq})
    old.run("lock")
    old.run("install")
    assert (project_dir / "grimoire.lock").is_file()  # positive control
    assert (project_dir / ".claude" / "skills" / "s" / "SKILL.md").is_file()
    return skill


def _add_a_skill(skill: reg.PublishedArtifact) -> list[str]:
    return ["add", skill.fq, "--name", "s2"]


COMMANDS: list[Callable[[reg.PublishedArtifact], list[str]]] = [
    lambda s: ["status"],
    lambda s: ["install"],
    lambda s: ["lock"],
    lambda s: ["update"],
    _add_a_skill,
]
COMMAND_IDS = ["status", "install", "lock", "update", "add"]


@pytest.mark.parametrize("command", COMMANDS, ids=COMMAND_IDS)
def test_a_mods_table_is_refused_by_every_command(
    old, project_dir: Path, working, mod, command
) -> None:
    write_config(project_dir, skills={"s": working.fq}, mods={"m": mod.fq})
    before = _snapshot(project_dir)

    result = old.run(*command(working), check=False)

    assert result.returncode == 78, result.stderr
    assert "unknown field `mods`" in result.stderr, result.stderr
    assert _snapshot(project_dir) == before


@pytest.mark.parametrize("command", COMMANDS, ids=COMMAND_IDS)
def test_the_mods_flag_alone_is_refused_by_every_command(
    old, project_dir: Path, working, version, command
) -> None:
    write_config(project_dir, skills={"s": working.fq}, experimental_mods=True)
    before = _snapshot(project_dir)

    result = old.run(*command(working), check=False)

    assert result.returncode == 78, result.stderr
    expected = (
        "expected `hooks`" if version == V15 else "expected `hooks` or `settings`"
    )
    assert "unknown field `mods`" in result.stderr and expected in result.stderr, (
        result.stderr
    )
    assert _snapshot(project_dir) == before


def _lock_with_a_mod_row(project: Path, mod: reg.PublishedArtifact) -> bytes:
    lock = project / "grimoire.lock"
    lock.write_text(
        lock.read_text(encoding="utf-8")
        + "\n[[mod]]\n"
        + 'name = "m"\n'
        + f'repo = "{REGISTRY_HOST}/{mod.repo}"\n'
        + 'tag = "1"\n'
        + f'pinned = "{REGISTRY_HOST}/{mod.repo}@{mod.digest}"\n',
        encoding="utf-8",
        newline="\n",
    )
    return lock.read_bytes()


@pytest.mark.parametrize("command", COMMANDS[:2], ids=COMMAND_IDS[:2])
def test_a_lock_with_a_mod_row_cannot_be_read_for_status_or_install(
    old, project_dir: Path, working, mod, version, command
) -> None:
    locked = _lock_with_a_mod_row(project_dir, mod)
    before = _snapshot(project_dir)

    result = old.run(*command(working), check=False)

    assert result.returncode == 78, result.stderr
    if version == V16:
        assert "the lock cannot be read; restore it from git" in result.stderr, (
            result.stderr
        )
    else:
        assert "unknown field `mod`" in result.stderr, result.stderr
    assert (project_dir / "grimoire.lock").read_bytes() == locked
    assert _snapshot(project_dir) == before


@pytest.mark.parametrize("command", COMMANDS[2:], ids=COMMAND_IDS[2:])
def test_a_lock_with_a_mod_row_under_lock_update_and_add(
    old, project_dir: Path, working, mod, version, command
) -> None:
    """0.15.1 rewrites the lock and drops the row with exit 0; 0.16.0 refuses.

    The 0.15.1 half is the residual that cannot be repaired from a newer grim:
    it is pinned so a change in what this grim writes cannot hide it.
    """
    locked = _lock_with_a_mod_row(project_dir, mod)
    assert b"[[mod]]" in locked  # positive control

    result = old.run(*command(working), check=False)

    lock = (project_dir / "grimoire.lock").read_bytes()
    if version == V16:
        assert result.returncode == 78, result.stderr
        assert "the lock cannot be read; restore it from git" in result.stderr
        assert lock == locked
    else:
        assert result.returncode == 0, result.stderr
        assert b"[[mod]]" not in lock
        assert lock != locked


def test_add_cannot_infer_the_kind_of_a_mod(
    old, project_dir: Path, mod, version
) -> None:
    write_config(project_dir)
    before = _snapshot(project_dir)

    result = old.run("add", mod.fq, "--name", "x", check=False)

    assert result.returncode == 65, result.stderr
    assert "could not infer the kind" in result.stderr, result.stderr
    assert _snapshot(project_dir) == before


def test_add_refuses_kind_mod_as_an_invalid_value(old, project_dir: Path, mod) -> None:
    write_config(project_dir)
    before = _snapshot(project_dir)

    result = old.run("add", mod.fq, "--name", "x", "--kind", "mod", check=False)

    assert result.returncode == 64, result.stderr
    assert "invalid value 'mod'" in result.stderr, result.stderr
    assert _snapshot(project_dir) == before


def test_fetch_does_not_know_a_mod_as_an_artifact(old, project_dir: Path, mod) -> None:
    write_config(project_dir)
    before = _snapshot(project_dir)

    result = old.run("fetch", mod.fq, check=False)

    assert result.returncode == 1, result.stderr
    assert "not a Grimoire artifact" in result.stderr, result.stderr
    assert _snapshot(project_dir) == before


def test_a_bundle_with_a_mod_member_is_refused_and_changes_nothing(
    old, project_dir: Path, unique_repo: str, mod
) -> None:
    bundle = make_bundle(f"{unique_repo}/b", [("mod", "m", mod.fq)], tag="1")
    write_config(project_dir)
    before = _snapshot(project_dir)

    result = old.run("add", bundle.fq, "--name", "x", check=False)

    assert result.returncode == 65, result.stderr
    assert "unknown variant `mod`" in result.stderr, result.stderr
    assert _snapshot(project_dir) == before


def _dependent_on_a_mod(
    repo: str, mod: reg.PublishedArtifact, annotations: dict[str, str] | None
) -> reg.PublishedArtifact:
    return make_dependent(
        repo, "1", "skill", SKILL, [("mod", "m", mod.fq)], annotations=annotations
    )


def test_a_skill_requiring_a_mod_without_the_annotation(
    old, project_dir: Path, unique_repo: str, mod, version
) -> None:
    dependent = _dependent_on_a_mod(f"{unique_repo}/dep", mod, None)
    write_config(project_dir)
    before = _snapshot(project_dir)

    result = old.run("add", dependent.fq, "--name", "x", check=False)

    assert result.returncode == 65, result.stderr
    if version == V15:
        # The two-layer artifact is refused only after the edge was declared and locked.
        assert "expected a single-layer" in result.stderr, result.stderr
        assert (project_dir / "grimoire.lock").is_file()
        assert _bytes(project_dir / "grimoire.toml") != before["grimoire.toml"]
    else:
        assert "unknown variant `mod`" in result.stderr, result.stderr
        assert _snapshot(project_dir) == before


def test_a_skill_requiring_a_mod_with_the_features_annotation(
    old, project_dir: Path, unique_repo: str, mod, version
) -> None:
    dependent = _dependent_on_a_mod(
        f"{unique_repo}/depf", mod, {FEATURES_KEY: "dependencies,mods"}
    )
    write_config(project_dir)
    before = _snapshot(project_dir)

    result = old.run("add", dependent.fq, "--name", "x", check=False)

    assert result.returncode == 65, result.stderr
    if version == V15:
        assert "expected a single-layer" in result.stderr, result.stderr
        assert _bytes(project_dir / "grimoire.toml") != before["grimoire.toml"]
    else:
        assert "requires features this grim does not support (mods)" in result.stderr
        assert "update grim" in result.stderr
        assert _snapshot(project_dir) == before


def test_a_plain_skill_with_the_mods_feature_annotation(
    old, project_dir: Path, unique_repo: str, version
) -> None:
    plain = push_manifest(
        f"{unique_repo}/pf",
        "1",
        [(reg.LAYER_MEDIA_TYPE, tar_of(SKILL))],
        {"com.grimoire.kind": "skill", FEATURES_KEY: "mods"},
    )
    write_config(project_dir)
    before = _snapshot(project_dir)

    result = old.run(
        "add", f"{REGISTRY_HOST}/{unique_repo}/pf:1", "--name", "x", check=False
    )

    if version == V15:
        assert result.returncode == 0, result.stderr
        assert (project_dir / ".claude" / "skills" / "x" / "SKILL.md").is_file()
    else:
        assert plain
        assert result.returncode == 65, result.stderr
        assert "requires features this grim does not support (mods)" in result.stderr
        assert "update grim" in result.stderr
        assert _snapshot(project_dir) == before


def test_a_bundle_manifest_carrying_the_mods_feature_itself(
    old, project_dir: Path, unique_repo: str, skill, version
) -> None:
    layer = members_layer([("skill", "s", skill.fq)])
    push_manifest(
        f"{unique_repo}/bf",
        "1",
        [(_REQUIRES_LAYER_MEDIA_TYPE, layer)],
        {"com.grimoire.kind": "bundle", FEATURES_KEY: "mods"},
    )
    write_config(project_dir)
    before = _snapshot(project_dir)

    result = old.run(
        "add", f"{REGISTRY_HOST}/{unique_repo}/bf:1", "--name", "x", check=False
    )

    if version == V15:
        assert result.returncode == 0, result.stderr
        assert (project_dir / ".claude" / "skills" / "s" / "SKILL.md").is_file()
    else:
        assert result.returncode == 65, result.stderr
        assert "requires features this grim does not support (mods)" in result.stderr
        assert _snapshot(project_dir) == before


def test_an_unknown_layer_media_type_on_a_known_kind_is_read_as_a_tar(
    old, project_dir: Path, unique_repo: str
) -> None:
    push_manifest(
        f"{unique_repo}/um",
        "1",
        [("application/vnd.grimoire.skill.v1.tar", tar_of(SKILL))],
        {"com.grimoire.kind": "skill"},
    )
    write_config(project_dir)

    result = old.run(
        "add", f"{REGISTRY_HOST}/{unique_repo}/um:1", "--name", "x", check=False
    )

    assert result.returncode == 0, result.stderr
    assert (project_dir / ".claude" / "skills" / "x" / "SKILL.md").is_file()


def test_a_global_config_naming_plugin_api_breaks_the_global_scope_only(
    old, current: GrimRunner, project_dir: Path, working
) -> None:
    """Global-scope commands exit 78; project-scope ones ignore the bad global options.

    The design expected 78 from every command that loads the global config. The
    real binaries only refuse in global scope, so a project with no mods keeps working.
    """
    assert (
        old.run("status", "--global", check=False).returncode == 0
    )  # positive control
    (current.grim_home / "grimoire.toml").write_text(
        '[options.vendors.opencode]\nplugin_api = "v2"\n',
        encoding="utf-8",
        newline="\n",
    )
    before = _snapshot(project_dir)

    refused = old.run("status", "--global", check=False)
    project = old.run("status", check=False)

    assert refused.returncode == 78, refused.stderr
    assert "plugin_api" in refused.stderr, refused.stderr
    assert project.returncode == 0, project.stderr
    assert _snapshot(project_dir) == before


def test_a_project_config_naming_plugin_api_is_refused(
    old, project_dir: Path, working
) -> None:
    write_config(
        project_dir,
        skills={"s": working.fq},
        vendors={"opencode": ['plugin_api = "v2"']},
    )
    before = _snapshot(project_dir)

    result = old.run("status", check=False)

    assert result.returncode == 78, result.stderr
    assert "plugin_api" in result.stderr, result.stderr
    assert _snapshot(project_dir) == before


# --- consent record locations -------------------------------------------------------


@pytest.fixture()
def hooked(
    old: GrimRunner, project_dir: Path, unique_repo: str
) -> reg.PublishedArtifact:
    """A project declaring one hook, locked and installed by the old grim and allowed."""
    hook = make_hook(f"{unique_repo}/hk", "hk", "1")
    write_config(project_dir, hooks={"hk": hook.fq})
    cfg = project_dir / "grimoire.toml"
    cfg.write_text(
        cfg.read_text(encoding="utf-8") + "\n[options.experimental]\nhooks = true\n",
        encoding="utf-8",
        newline="\n",
    )
    old.run("lock")
    old.run("hook", "allow")
    old.run("install")
    return hook


def _record_key(project: Path) -> str:
    return hashlib.sha256(str(project).encode()).hexdigest()


def _armed(grim_home: Path) -> list[dict]:
    table = grim_home / "hooks" / "dispatch.json"
    if not table.is_file():
        return []
    doc = json.loads(table.read_text(encoding="utf-8"))
    return [row for root in doc["roots"].values() for row in root["hooks"]]


def _only_at_the_new_location(grim_home: Path, project: Path) -> bytes:
    """Move the old grim's record to `consent/`, leaving `hooks/consent/` absent."""
    old_dir = grim_home / "hooks" / "consent"
    record = old_dir / f"{_record_key(project)}.json"
    assert record.is_file()  # positive control: the old grim wrote its record there
    new = grim_home / "consent" / record.name
    new.parent.mkdir(parents=True)
    body = b'{"v": 2, "sources": ["mod:x"], "workspace": "w"}\n'
    new.write_bytes(body)
    record.unlink()
    old_dir.rmdir()
    return body


def test_a_record_only_at_the_new_location_leaves_hooks_unarmed_and_the_dir_untouched(
    old, current: GrimRunner, project_dir: Path, hooked
) -> None:
    assert _armed(current.grim_home), "the old grim armed the hook (positive control)"
    body = _only_at_the_new_location(current.grim_home, project_dir)
    (current.grim_home / "hooks" / "dispatch.json").unlink()

    result = old.run("install", check=False)

    assert result.returncode == 0, result.stderr
    assert _armed(current.grim_home) == [], (
        "no record the old grim reads, so nothing arms"
    )
    assert not (current.grim_home / "hooks" / "consent").exists()
    assert (
        current.grim_home / "consent" / f"{_record_key(project_dir)}.json"
    ).read_bytes() == body


def test_with_both_record_locations_the_old_grim_reads_its_own_only(
    old, current: GrimRunner, project_dir: Path, hooked
) -> None:
    new = current.grim_home / "consent" / f"{_record_key(project_dir)}.json"
    new.parent.mkdir(parents=True)
    body = b'{"v": 2, "sources": [], "workspace": "w"}\n'
    new.write_bytes(body)
    (current.grim_home / "hooks" / "dispatch.json").unlink()

    result = old.run("install", check=False)

    assert result.returncode == 0, result.stderr
    assert _armed(current.grim_home), "the old record still arms"
    assert new.read_bytes() == body


def test_an_old_revoke_leaves_a_newer_grims_v2_grant_armed(
    old, current: GrimRunner, project_dir: Path, hooked
) -> None:
    """In one `GRIM_HOME`: allow (old), a v2 grant (newer), revoke (old), arm (newer).

    The newer grim's grant moves the old grim's record into `consent/`, so the
    old revoke finds nothing of its own to remove and the grant survives it.
    """
    grim_home = current.grim_home
    assert _armed(grim_home), "the old grim armed the hook (positive control)"
    current.run("consent", "allow")
    v2 = grim_home / "consent" / f"{_record_key(project_dir)}.json"
    body = v2.read_bytes()
    assert json.loads(body)["v"] == 2  # positive control

    old.run("hook", "revoke", check=False)

    assert v2.read_bytes() == body, "the v2 grant is not the old grim's to remove"
    (grim_home / "hooks" / "dispatch.json").unlink(missing_ok=True)
    current.run("install")
    assert _armed(grim_home), "the newer grim still arms on the v2 grant"


def test_global_state_file_stays_readable_by_an_older_grim(
    old: GrimRunner,
    current: GrimRunner,
    skill: reg.PublishedArtifact,
    mod: reg.PublishedArtifact,
    tmp_path: Path,
) -> None:
    sandbox, _ = mod_sandbox(tmp_path / "sandbox")
    current.env.update(sandbox)
    old.env.update(sandbox)
    write_config(
        current.grim_home,
        skills={"s": skill.fq},
        mods={"m": mod.fq},
        experimental_mods=True,
        options={"clients": json.dumps(["claude"])},
    )
    current.run("--global", "lock")
    current.run("--global", "install", "--trust-workspace")
    state_file = current.grim_home / "state" / "global.json"
    before = state_file.read_bytes()
    text = before.decode("utf-8")
    assert '"s"' in text, "positive control: the global skill's record is written"
    assert '"mod"' not in text

    # An older grim refuses a global config naming mods (78), so the declaration
    # and its lock leave; what it must read is the state file this grim wrote beside it.
    write_config(
        current.grim_home,
        skills={"s": skill.fq},
        options={"clients": json.dumps(["claude"])},
    )
    (current.grim_home / "grimoire.lock").unlink()

    status = old.run("--global", "status", check=False)

    assert status.returncode == 0, status.stderr
    assert state_file.read_bytes() == before
