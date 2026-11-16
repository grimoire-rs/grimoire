# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Self-tests for the scenario helpers (``src/scenarios.py``, ``src/oci_proxy.py``)."""
from __future__ import annotations

import os
import shutil
import urllib.error
import urllib.request
from collections.abc import Callable
from pathlib import Path

import pytest

from src.helpers import make_artifact, write_config
from src.oci_proxy import OciProxy
from src.registry import REGISTRY_BASE
from src.scenarios import (
    Team,
    allow_insecure,
    assert_unchanged,
    clone_workspace,
    fresh_runner,
    gone_pin,
    pull_from,
    state_snapshot,
    to_crlf,
)

pytestmark = pytest.mark.skipif(shutil.which("git") is None, reason="git not on PATH")


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8", newline="\n")


def _skill(unique_repo: str, name: str = "helper"):
    return make_artifact(
        f"{unique_repo}/{name}", "skill", {f"{name}/SKILL.md": f"---\nname: {name}\n---\n# {name}\n"}
    )


def test_clone_workspace_has_no_grimoire_dir(tmp_path: Path) -> None:
    src = tmp_path / "src"
    _write(src / ".grimoire" / "state.json", "{}")
    _write(src / "grimoire.toml", "[skills]\n")
    clone = clone_workspace(src, tmp_path / "clone")
    assert (clone / "grimoire.toml").is_file()
    assert not (clone / ".grimoire").exists()


def test_pull_from_keeps_destination_grimoire_dir(tmp_path: Path) -> None:
    src, dst = tmp_path / "src", tmp_path / "dst"
    _write(src / "grimoire.lock", "new\n")
    _write(src / ".claude" / "skills" / "a" / "SKILL.md", "a\n")
    _write(src / ".grimoire" / "state.json", "source\n")
    _write(dst / ".grimoire" / "state.json", "mine\n")
    _write(dst / "stale.txt", "gone upstream\n")
    pull_from(src, dst)
    assert (dst / ".grimoire" / "state.json").read_text(encoding="utf-8") == "mine\n"
    assert (dst / "grimoire.lock").read_text(encoding="utf-8") == "new\n"
    assert (dst / ".claude" / "skills" / "a" / "SKILL.md").is_file()
    assert not (dst / "stale.txt").exists()


def test_fresh_runners_do_not_share_home(tmp_path: Path) -> None:
    one, two = fresh_runner(tmp_path, "one"), fresh_runner(tmp_path, "two")
    assert one.home != two.home
    assert one.grim_home.parent != two.grim_home.parent


def test_proxy_counts_requests_of_a_real_pull(
    oci_proxy: Callable[..., OciProxy], tmp_path: Path, unique_repo: str
) -> None:
    art = _skill(unique_repo)
    proxy = oci_proxy("count")
    project = tmp_path / "project"
    (project / ".claude").mkdir(parents=True)
    write_config(project, skills={"helper": f"{proxy.host}/{art.repo}:{art.tag}"})
    runner = fresh_runner(tmp_path, "m", cwd=project, insecure=(proxy.host,))
    runner.run("lock")
    assert proxy.count(f"/v2/{art.repo}/manifests/{art.tag}") >= 1


def test_proxy_record_marks_no_authorization_and_stores_no_values(
    oci_proxy: Callable[..., OciProxy], tmp_path: Path, unique_repo: str
) -> None:
    art = _skill(unique_repo)
    proxy = oci_proxy("record")
    project = tmp_path / "project"
    (project / ".claude").mkdir(parents=True)
    write_config(project, skills={"helper": f"{proxy.host}/{art.repo}:{art.tag}"})
    fresh_runner(tmp_path, "m", cwd=project, insecure=(proxy.host,)).run("lock")
    assert proxy.requests
    assert not any(r.has_authorization for r in proxy.requests)

    secret = "s3cret-value-123"
    request = urllib.request.Request(
        f"http://{proxy.host}/v2/", headers={"Authorization": f"Bearer {secret}", "X-Probe": secret}
    )
    urllib.request.urlopen(request).close()
    last = proxy.requests[-1]
    assert last.has_authorization
    assert "x-probe" in last.header_names
    assert secret not in repr(proxy.requests)


def test_proxy_hang_never_answers(oci_proxy: Callable[..., OciProxy]) -> None:
    proxy = oci_proxy("hang")
    with pytest.raises((TimeoutError, urllib.error.URLError)):
        urllib.request.urlopen(f"http://{proxy.host}/v2/", timeout=0.5)


def test_git_team_co_commits_client_outputs(team: Callable[[str], Team], unique_repo: str) -> None:
    t = team("CO")
    art = _skill(unique_repo)
    write_config(t.alice.work, skills={"helper": art.fq})
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: add helper")
    tracked = t.alice.tracked()
    assert ".claude/skills/helper/SKILL.md" in tracked
    assert not any(p.startswith(".grimoire/") for p in tracked)


def test_git_team_go_ignores_client_outputs(team: Callable[[str], Team], unique_repo: str) -> None:
    t = team("GO")
    art = _skill(unique_repo)
    write_config(t.alice.work, skills={"helper": art.fq})
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: add helper")
    tracked = t.alice.tracked()
    assert ".gitignore" in tracked
    assert "grimoire.lock" in tracked
    assert not any(p.startswith(".claude/skills/") for p in tracked)
    assert (t.alice.work / ".claude" / "skills" / "helper" / "SKILL.md").is_file()


def test_state_snapshot_detects_a_rewrite(tmp_path: Path) -> None:
    state, out = tmp_path / "state.json", tmp_path / "out.md"
    _write(state, "{}")
    _write(out, "x")
    snap = state_snapshot(state, out)
    assert_unchanged(snap)
    _write(state, "{ }")
    with pytest.raises(AssertionError):
        assert_unchanged(snap)


def test_state_snapshot_detects_a_touched_output(tmp_path: Path) -> None:
    state, out = tmp_path / "state.json", tmp_path / "out.md"
    _write(state, "{}")
    _write(out, "x")
    snap = state_snapshot(state, out)
    stat = out.stat()
    os.utime(out, ns=(stat.st_atime_ns, stat.st_mtime_ns + 1_000_000_000))
    with pytest.raises(AssertionError):
        assert_unchanged(snap)


def test_gone_pin_makes_the_pin_unfetchable(tmp_path: Path, unique_repo: str) -> None:
    art = _skill(unique_repo)
    project = tmp_path / "project"
    (project / ".claude").mkdir(parents=True)
    write_config(project, skills={"helper": art.fq})
    runner = fresh_runner(tmp_path, "m", cwd=project)
    runner.run("lock")
    digest = gone_pin(project / "grimoire.lock", "helper")
    assert digest in (project / "grimoire.lock").read_text(encoding="utf-8")
    with pytest.raises(urllib.error.HTTPError):
        urllib.request.urlopen(f"{REGISTRY_BASE}/v2/{art.repo}/manifests/{digest}")
    result = runner.run("install", check=False)
    assert result.returncode != 0


def test_to_crlf_produces_crlf(tmp_path: Path) -> None:
    path = tmp_path / "f.md"
    path.write_bytes(b"a\r\nb\nc\n")
    to_crlf(path)
    assert path.read_bytes() == b"a\r\nb\r\nc\r\n"


def test_allow_insecure_keeps_existing_hosts(tmp_path: Path) -> None:
    runner = fresh_runner(tmp_path, "m", insecure=("127.0.0.1:1",))
    allow_insecure(runner, "127.0.0.1:2", "127.0.0.1:1")
    hosts = runner.env["GRIM_INSECURE_REGISTRIES"].split(",")
    assert hosts.count("127.0.0.1:1") == 1 and "127.0.0.1:2" in hosts
