# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim remove` and `grim uninstall` relock in the same run.

An edit meant to keep `grimoire.toml` and `grimoire.lock` in sync ends with a
lock that is not stale: every pin that still answers carries, nothing is
refreshed, and the lock is written once. A relock that fails changes nothing on
disk and names the `--no-relock` command, which edits in place as before and
leaves the lock stale. A lock that stays fresh reads back byte for byte from an
offline `grim lock`, which keeps `generated_at` when nothing changed.
"""

from __future__ import annotations

import json
import re
import subprocess
from pathlib import Path

import pytest
import tomllib

from src.helpers import make_hook, printed_argv, publish_release, write_config
from src.registry import REGISTRY_HOST, PublishedArtifact, retag
from src.runner import GrimRunner


def _files(name: str) -> dict[str, str]:
    return {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name}\n"}


def _ref(repo: str, tag: str | None = None) -> str:
    return f"{REGISTRY_HOST}/{repo}" + (f":{tag}" if tag else "")


def _at(unique_repo: str, target: str, tag: str) -> tuple[str, str]:
    """An edge onto skill `target` at `tag`."""
    return target, _ref(f"{unique_repo}/{target}", tag)


def _release(
    unique_repo: str, name: str, version: str, *edges: tuple[str, str]
) -> PublishedArtifact:
    """Release skill `name` at `version`, cascade included, requiring each `(target, ref)`."""
    requires = [("skill", target, ref) for target, ref in edges] or None
    return publish_release(f"{unique_repo}/{name}", version, _files(name), requires)


def _pins(project_dir: Path) -> dict[str, str]:
    text = (project_dir / "grimoire.lock").read_text(encoding="utf-8")
    return {e["name"]: e["pinned"] for e in tomllib.loads(text).get("skill", [])}


def _declared(project_dir: Path) -> dict[str, str]:
    text = (project_dir / "grimoire.toml").read_text(encoding="utf-8")
    return tomllib.loads(text).get("skills", {})


def _relock_offline(runner: GrimRunner, project_dir: Path) -> bool:
    """Run one offline `grim lock`; whether it left the lock's bytes as they were."""
    lock = project_dir / "grimoire.lock"
    before = lock.read_bytes()
    result = runner.run("--offline", "lock", format="json", check=False)
    assert result.returncode == 0, result.stderr
    return lock.read_bytes() == before


def _snapshot(project_dir: Path) -> dict[str, bytes]:
    """The bytes of every file a refused edit must leave alone."""
    files = ["grimoire.toml", "grimoire.lock", ".grimoire/state.json"]
    return {f: (project_dir / f).read_bytes() for f in files if (project_dir / f).is_file()}


def _refused(result: subprocess.CompletedProcess[str], exit_code: int) -> dict:
    assert result.returncode == exit_code, result.stderr
    return json.loads(result.stdout)["error"]


def _printed(pattern: str, message: str) -> re.Match[str]:
    printed = re.search(pattern, message)
    assert printed, message
    return printed


def _c_under_a_range_claim(
    grim_at, project_dir: Path, unique_repo: str
) -> tuple[GrimRunner, PublishedArtifact]:
    """`q` claims `c:2`; `c` is declared on `c:stable`, which names 2.3.0 while
    2.9.0 is out. Locked online, so the store holds every manifest."""
    c = f"{unique_repo}/c"
    v230 = _release(unique_repo, "c", "2.3.0")
    retag(c, "stable", v230.digest)
    _release(unique_repo, "c", "2.9.0")
    _release(unique_repo, "q", "1.0.0", _at(unique_repo, "c", "2"))
    write_config(
        project_dir, skills={"q": _ref(f"{unique_repo}/q", "1"), "c": _ref(c, "stable")}
    )
    runner = grim_at(project_dir)
    runner.run("lock")
    assert _pins(project_dir)["c"].endswith(f"@{v230.digest}")
    return runner, v230


def _two_majors_held_off(
    grim_at, project_dir: Path, unique_repo: str
) -> tuple[GrimRunner, dict[str, str]]:
    """`b` claims `x:1` and `c` claims `x:2`; the tagged line `x = x:2` wins."""
    _release(unique_repo, "x", "1.0.0")
    _release(unique_repo, "x", "2.0.0")
    _release(unique_repo, "b", "1.0.0", _at(unique_repo, "x", "1"))
    _release(unique_repo, "c", "1.0.0", _at(unique_repo, "x", "2"))
    skills = {
        "b": _ref(f"{unique_repo}/b", "1"),
        "c": _ref(f"{unique_repo}/c", "1"),
        "x": _ref(f"{unique_repo}/x", "2"),
    }
    write_config(project_dir, skills=skills)
    runner = grim_at(project_dir)
    runner.run("lock")
    return runner, skills


def _x_held_off_by_a_tagged_line(
    grim_at, project_dir: Path, unique_repo: str
) -> tuple[GrimRunner, PublishedArtifact, PublishedArtifact]:
    """`p` claims `x:2`; the tagged line `x = x:1` wins, so `x` is pinned on 1.0.0."""
    v100 = _release(unique_repo, "x", "1.0.0")
    v200 = _release(unique_repo, "x", "2.0.0")
    _release(unique_repo, "p", "1.0.0", _at(unique_repo, "x", "2"))
    write_config(
        project_dir,
        skills={
            "p": _ref(f"{unique_repo}/p", "1"),
            "x": _ref(f"{unique_repo}/x", "1"),
        },
    )
    runner = grim_at(project_dir)
    runner.run("lock")
    assert _pins(project_dir)["x"].endswith(f"@{v100.digest}")
    return runner, v100, v200


def _forget_offline(grim_home: Path, repo: str, tags: list[str], digest: str) -> None:
    """Drop `tags` of `repo` from the tag cache and `digest` from the store."""
    for path in (grim_home / "tags").rglob("tags.json"):
        cache = json.loads(path.read_text(encoding="utf-8"))
        if cache["repository"] == repo:
            for tag in tags:
                cache["tags"].pop(tag, None)
            path.write_text(json.dumps(cache), encoding="utf-8", newline="\n")
    algorithm, hex_digest = digest.split(":")
    (grim_home / "blobs" / algorithm / hex_digest[:2] / hex_digest).unlink(missing_ok=True)


# ── remove ────────────────────────────────────────────────────────────────


def test_remove_keeps_a_pin_a_remaining_range_claim_holds_and_leaves_the_lock_fresh(
    grim_at, grim_home: Path, project_dir: Path, unique_repo: str
) -> None:
    """Removing `c`'s line leaves `q`'s claim `c:2`, which 2.3.0 answers: the pin
    carries, 2.9.0 is not taken, and the lock states the edited declaration."""
    runner, v230 = _c_under_a_range_claim(grim_at, project_dir, unique_repo)

    result = runner.run("remove", "skill", "c", check=False)

    assert result.returncode == 0, result.stderr
    assert "stale" not in result.stderr, result.stderr
    assert "c" not in _declared(project_dir)
    assert _pins(project_dir)["c"].endswith(f"@{v230.digest}")
    assert _relock_offline(runner, project_dir), "the remove left the lock stale"
    assert not (grim_home / "hooks" / "consent").exists()


def test_an_offline_remove_with_a_complete_store_relocks_without_the_network(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    runner, v230 = _c_under_a_range_claim(grim_at, project_dir, unique_repo)

    result = runner.run("--offline", "remove", "skill", "c", format="json", check=False)

    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)["status"] == "removed"
    assert _pins(project_dir)["c"].endswith(f"@{v230.digest}")
    assert _relock_offline(runner, project_dir), "the remove left the lock stale"


def test_remove_no_relock_edits_in_place_and_leaves_the_lock_stale(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    runner, v230 = _c_under_a_range_claim(grim_at, project_dir, unique_repo)

    result = runner.run("remove", "--no-relock", "skill", "c", check=False)

    assert result.returncode == 0, result.stderr
    assert "The lock is marked stale" in result.stderr, result.stderr
    assert "c" not in _declared(project_dir)
    assert _pins(project_dir)["c"].endswith(f"@{v230.digest}")
    assert not _relock_offline(runner, project_dir), "the in-place edit restamped the lock"


def test_a_remove_that_uncovers_two_majors_exits_78_and_changes_nothing(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """Without the tagged line, `b`'s `x:1` and `c`'s `x:2` meet on one node.
    Both printed remedies work: the flag removes the line with the lock left
    stale, and a line with the other tag locks."""
    runner, skills = _two_majors_held_off(grim_at, project_dir, unique_repo)
    before = _snapshot(project_dir)

    error = _refused(runner.run("remove", "skill", "x", format="json", check=False), 78)

    assert (error["code"], error.get("reason")) == ("config", None), error
    assert _snapshot(project_dir) == before
    assert "declare skill x directly in grimoire.toml with a tag" in error["message"]
    flag = _printed(r"`(grim remove --no-relock skill x)`", error["message"])

    followed = runner.run(*printed_argv(flag.group(1))[1:], check=False)

    assert followed.returncode == 0, followed.stderr
    assert "x" not in _declared(project_dir)
    write_config(project_dir, skills={**skills, "x": _ref(f"{unique_repo}/x", "1")})
    locked = runner.run("lock", check=False)
    assert locked.returncode == 0, locked.stderr


def test_a_remove_that_forces_a_major_downgrade_names_the_flag_and_the_lock_flag(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """`a` claims `x:2`; the tagged line `x = x:3` held `x` on 3.1.0. Removing it
    would move `x` down a major, which only `grim lock --allow-downgrade` takes."""
    _release(unique_repo, "x", "2.6.0")
    v310 = _release(unique_repo, "x", "3.1.0")
    _release(unique_repo, "a", "1.0.0", _at(unique_repo, "x", "2"))
    write_config(
        project_dir,
        skills={"a": _ref(f"{unique_repo}/a", "1"), "x": _ref(f"{unique_repo}/x", "3")},
    )
    runner = grim_at(project_dir)
    runner.run("lock")
    assert _pins(project_dir)["x"].endswith(f"@{v310.digest}")
    before = _snapshot(project_dir)

    error = _refused(runner.run("remove", "skill", "x", format="json", check=False), 78)

    assert error.get("reason") == "major-downgrade", error
    assert _snapshot(project_dir) == before
    printed = _printed(
        r"run `(grim remove --no-relock skill x)`, then `(grim lock --allow-downgrade)`",
        error["message"],
    )
    for command in printed.groups():
        followed = runner.run(*printed_argv(command)[1:], check=False)
        assert followed.returncode == 0, (command, followed.stderr)
    assert "x" not in _declared(project_dir)
    assert not _pins(project_dir)["x"].endswith(f"@{v310.digest}")
    assert _relock_offline(runner, project_dir)


def test_a_remove_that_raises_a_claim_moves_the_pin_and_says_so(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    runner, _, v200 = _x_held_off_by_a_tagged_line(grim_at, project_dir, unique_repo)

    result = runner.run("remove", "skill", "x", check=False)

    assert result.returncode == 0, result.stderr
    assert "moved 1 pin: skill/x" in result.stderr, result.stderr
    assert _pins(project_dir)["x"].endswith(f"@{v200.digest}")
    assert _relock_offline(runner, project_dir), "the remove left the lock stale"


def test_an_offline_remove_missing_a_manifest_fails_and_names_the_flag(
    grim_at, grim_home: Path, project_dir: Path, unique_repo: str
) -> None:
    """Offline, `p`'s claim `x:2` needs a tag this machine never cached."""
    runner, _, v200 = _x_held_off_by_a_tagged_line(grim_at, project_dir, unique_repo)
    _forget_offline(grim_home, f"{unique_repo}/x", ["2", "2.0", "2.0.0", "latest"], v200.digest)
    before = _snapshot(project_dir)

    error = _refused(
        runner.run("--offline", "remove", "skill", "x", format="json", check=False), 69
    )

    assert "offline mode blocked a required network operation" in error["message"], error
    assert _snapshot(project_dir) == before
    flag = _printed(r"`(grim remove --no-relock skill x)`", error["message"])

    followed = runner.run("--offline", *printed_argv(flag.group(1))[1:], check=False)

    assert followed.returncode == 0, followed.stderr
    assert "x" not in _declared(project_dir)


# ── uninstall ─────────────────────────────────────────────────────────────


def test_uninstall_relocks_and_leaves_the_lock_fresh(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """`q` still needs `c`, so its files stay and only the line goes."""
    runner, v230 = _c_under_a_range_claim(grim_at, project_dir, unique_repo)
    runner.run("install")

    result = runner.run("uninstall", "skill", "c", check=False)

    assert result.returncode == 0, result.stderr
    assert "stale" not in result.stderr, result.stderr
    assert "c" not in _declared(project_dir)
    assert _pins(project_dir)["c"].endswith(f"@{v230.digest}")
    assert (project_dir / ".claude/skills/c/SKILL.md").is_file()
    assert _relock_offline(runner, project_dir), "the uninstall left the lock stale"


def test_an_uninstall_that_uncovers_two_majors_exits_78_before_touching_a_file(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    runner, _ = _two_majors_held_off(grim_at, project_dir, unique_repo)
    runner.run("install")
    before = _snapshot(project_dir)

    error = _refused(
        runner.run("uninstall", "--force", "skill", "x", format="json", check=False), 78
    )

    assert _snapshot(project_dir) == before
    assert (project_dir / ".claude/skills/x/SKILL.md").is_file()
    flag = _printed(r"`(grim uninstall --no-relock --force skill x)`", error["message"])

    followed = runner.run(*printed_argv(flag.group(1))[1:], check=False)

    assert followed.returncode == 0, followed.stderr
    assert "x" not in _declared(project_dir)


def test_uninstall_no_relock_edits_in_place_and_leaves_the_lock_stale(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    runner, _ = _c_under_a_range_claim(grim_at, project_dir, unique_repo)
    runner.run("install")

    result = runner.run("uninstall", "--no-relock", "skill", "c", check=False)

    assert result.returncode == 0, result.stderr
    assert "The lock is marked stale" in result.stderr, result.stderr
    assert "c" not in _declared(project_dir)
    assert not _relock_offline(runner, project_dir), "the in-place edit restamped the lock"


def test_uninstall_keeps_the_files_of_a_row_the_relock_keeps(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """`p`, declared by hand after the last lock, requires `m`. The lock on disk
    has no row for `p`, but the relock resolves it and keeps `m`'s pin, so `m`'s
    files stay with it."""
    _release(unique_repo, "m", "1.0.0")
    _release(unique_repo, "p", "1.0.0", _at(unique_repo, "m", "1"))
    m = _ref(f"{unique_repo}/m", "1")
    write_config(project_dir, skills={"m": m})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    write_config(project_dir, skills={"m": m, "p": _ref(f"{unique_repo}/p", "1")})

    result = runner.run("uninstall", "skill", "m", check=False)

    assert result.returncode == 0, result.stderr
    assert "m" not in _declared(project_dir)
    assert "m" in _pins(project_dir)
    assert (project_dir / ".claude/skills/m/SKILL.md").is_file(), "the relocked lock pins a deleted row"
    assert _relock_offline(runner, project_dir), "the uninstall left the lock stale"


def test_uninstall_deletes_the_files_of_a_row_the_relock_drops(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """The line `x = x:1` holds `p`'s claim `x:2` off, and only `x` 1.0.0 requires
    `d`. Uninstalling `x` relocks it onto 2.0.0 for `p`, which drops `d`, so `d`'s
    files go with its row while `x`'s stay for `p`."""
    _release(unique_repo, "d", "1.0.0")
    _release(unique_repo, "x", "1.0.0", _at(unique_repo, "d", "1"))
    v200 = _release(unique_repo, "x", "2.0.0")
    _release(unique_repo, "p", "1.0.0", _at(unique_repo, "x", "2"))
    write_config(
        project_dir,
        skills={"p": _ref(f"{unique_repo}/p", "1"), "x": _ref(f"{unique_repo}/x", "1")},
    )
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    assert (project_dir / ".claude/skills/d/SKILL.md").is_file()

    result = runner.run("uninstall", "skill", "x", check=False)

    assert result.returncode == 0, result.stderr
    assert "d" not in _pins(project_dir)
    assert _pins(project_dir)["x"].endswith(f"@{v200.digest}")
    assert not (project_dir / ".claude/skills/d").exists(), "the relocked lock dropped d's row"
    assert (project_dir / ".claude/skills/x/SKILL.md").is_file(), "p still needs x"
    assert _relock_offline(runner, project_dir), "the uninstall left the lock stale"


@pytest.mark.parametrize("no_relock", [False, True])
def test_an_unregistered_gatekeeper_remedy_names_grim_lock_only_on_a_stale_lock(
    grim_at, project_dir: Path, unique_repo: str, no_relock: bool
) -> None:
    """A gatekeeper lost its client registration, then another row is uninstalled.
    A relocked uninstall leaves the lock in line, so the remedy names the install
    alone; under `--no-relock` the lock is stale and `grim lock` comes first."""
    _release(unique_repo, "carrier", "1.0.0")
    gate = make_hook(f"{unique_repo}/gate", "gate", tag="1")
    write_config(
        project_dir, skills={"carrier": _ref(f"{unique_repo}/carrier", "1")}, hooks={"gate": gate.fq}
    )
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    runner.run("hook", "allow")
    runner.run("install")
    (project_dir / ".claude" / "settings.local.json").unlink()
    flag = ["--no-relock"] if no_relock else []

    result = runner.run("uninstall", *flag, "skill", "carrier", check=False)

    assert result.returncode == 0, result.stderr
    warning = [line for line in result.stderr.splitlines() if "no client registration" in line]
    assert len(warning) == 1, result.stderr
    assert "`grim install` restores it" in warning[0], warning[0]
    assert ("run `grim lock` first" in warning[0]) == no_relock, warning[0]
