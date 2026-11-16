# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""``grim_status`` proves from the local store and writes only under ``--allow-writes``.

The MCP tool applies the rule ``grim status`` applies: files the record does
not cover that match a render of the lock's pin read ``installed``. It reads
the local store only, so no call sends a registry request, and it writes the
proven records only when the server was started with ``--allow-writes``.
Matrix S50 and column b's ``grim_status --allow-writes`` form of
``research_install_record_cache_test_matrix.md``, and
``adr_output_ownership_without_state.md`` validation case 34.

World, on one machine: a skill ``moving`` and a rule ``style`` whose tags move
from v1 to v2, and a skill ``steady`` whose tag never moves.
"""

from __future__ import annotations

import json
import shutil
from dataclasses import dataclass
from pathlib import Path

import pytest
import tomllib

from src.helpers import make_artifact, write_config
from src.oci_proxy import OciProxy
from src.runner import GrimRunner
from src.scenarios import (
    allow_insecure,
    assert_unchanged,
    cold_runner,
    move_tag,
    state_snapshot,
)
from src.stdio_drivers import drive_until_answered, initialize

NAMES = ("moving", "style", "steady")
EDIT = "# local edit\n"


def _moving(version: str) -> dict[str, str]:
    return {
        "moving/SKILL.md": f"# moving {version}\n",
        "moving/ref.md": f"ref {version}\n",
    }


def _style(version: str) -> dict[str, str]:
    return {"style.md": f"---\npaths: ['**/*.rs']\n---\n# style {version}\n"}


@dataclass
class Machine:
    runner: GrimRunner
    work: Path
    proxy: OciProxy
    repo: str

    @property
    def state(self) -> Path:
        return self.work / ".grimoire" / "state.json"

    def client_files(self) -> list[Path]:
        return [p for p in sorted((self.work / ".claude").rglob("*")) if p.is_file()]

    def snapshot(self):
        return state_snapshot(self.state, *self.client_files())

    def requests(self) -> int:
        return sum(self.proxy.counts.values())

    def record_pins(self) -> dict[str, str]:
        doc = json.loads(self.state.read_text(encoding="utf-8"))
        return {r["name"]: r["pinned"].rsplit("@", 1)[1] for r in doc["records"]}

    def lock_pins(self) -> dict[str, str]:
        doc = tomllib.loads((self.work / "grimoire.lock").read_text(encoding="utf-8"))
        return {
            row["name"]: row["pinned"].rsplit("@", 1)[1]
            for kind in ("skill", "rule")
            for row in doc.get(kind, [])
        }

    def edit(self, *names: str) -> None:
        for name in names:
            path = self.work / ".claude" / "skills" / name / "SKILL.md"
            if not path.exists():
                path = self.work / ".claude" / "rules" / f"{name}.md"
            path.write_text(
                path.read_text(encoding="utf-8") + EDIT, encoding="utf-8", newline="\n"
            )

    def to_v2(self, *, keep_state: bool, keep_files: bool) -> None:
        """Lock and store at v2; the record and the files stay at v1 if asked."""
        aside = self.work.parent / "aside"
        shutil.copytree(self.work / ".grimoire", aside / "grimoire")
        shutil.copytree(self.work / ".claude", aside / "claude")
        move_tag(f"{self.repo}/moving", _moving("v2"), tag="rolling", kind="skill")
        move_tag(f"{self.repo}/style", _style("v2"), tag="rolling", kind="rule")
        self.runner.run("update")
        for keep, name, dest in (
            (keep_state, "grimoire", ".grimoire"),
            (keep_files, "claude", ".claude"),
        ):
            if keep:
                shutil.rmtree(self.work / dest)
                shutil.copytree(aside / name, self.work / dest)


def _machine(grim_at, project_dir: Path, unique_repo: str, proxy: OciProxy) -> Machine:
    make_artifact(f"{unique_repo}/moving", "skill", _moving("v1"), tag="rolling")
    make_artifact(f"{unique_repo}/style", "rule", _style("v1"), tag="rolling")
    make_artifact(
        f"{unique_repo}/steady",
        "skill",
        {"steady/SKILL.md": "# steady\n"},
        tag="stable",
    )
    host = proxy.host
    write_config(
        project_dir,
        skills={
            "moving": f"{host}/{unique_repo}/moving:rolling",
            "steady": f"{host}/{unique_repo}/steady:stable",
        },
        rules={"style": f"{host}/{unique_repo}/style:rolling"},
        options={"clients": json.dumps(["claude"])},
    )
    runner = grim_at(project_dir)
    allow_insecure(runner, host)
    runner.run("lock")
    runner.run("install")
    return Machine(runner, project_dir, proxy, unique_repo)


def _status_calls(
    runner: GrimRunner, cwd: Path, *, allow_writes: bool, calls: int = 1
) -> list[dict[str, str]]:
    """``calls`` ``grim_status`` calls to one online server; each call's states by name."""
    requests: list[dict] = [
        initialize(1),
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
    ]
    requests += [
        {
            "jsonrpc": "2.0",
            "id": 2 + n,
            "method": "tools/call",
            "params": {"name": "grim_status", "arguments": {}},
        }
        for n in range(calls)
    ]
    responses = drive_until_answered(
        runner, cwd, requests, allow_writes=allow_writes, offline=False
    )
    states = []
    for n in range(calls):
        result = responses[2 + n]["result"]
        assert result["isError"] is False, result
        items = json.loads(result["content"][0]["text"])["items"]
        states.append({row["name"]: row["state"] for row in items})
    return states


# ===========================================================================
# Matrix S50: with and without --allow-writes, warm and cold store
# ===========================================================================


def test_without_writes_status_reports_installed_and_writes_nothing(
    grim_at, project_dir, unique_repo, oci_proxy
):
    # research_install_record_cache_test_matrix.md S50;
    # adr_output_ownership_without_state.md validation case 34
    m = _machine(grim_at, project_dir, unique_repo, oci_proxy("count"))
    shutil.rmtree(m.work / ".grimoire")
    requests = m.requests()
    calls = _status_calls(m.runner, m.work, allow_writes=False, calls=2)
    assert calls == [{name: "installed" for name in NAMES}] * 2
    assert not (m.work / ".grimoire").exists()
    assert m.requests() == requests


def test_with_writes_status_records_what_it_proves_once(
    grim_at, project_dir, unique_repo, oci_proxy
):
    # research_install_record_cache_test_matrix.md S50;
    # adr_output_ownership_without_state.md validation case 34
    m = _machine(grim_at, project_dir, unique_repo, oci_proxy("count"))
    shutil.rmtree(m.work / ".grimoire")
    requests = m.requests()
    (states,) = _status_calls(m.runner, m.work, allow_writes=True)
    assert states == {name: "installed" for name in NAMES}
    assert m.record_pins() == m.lock_pins()
    snap = m.snapshot()
    (states,) = _status_calls(m.runner, m.work, allow_writes=True)
    assert states == {name: "installed" for name in NAMES}
    assert_unchanged(snap)
    assert m.requests() == requests


@pytest.mark.parametrize("allow_writes", [False, True])
def test_a_cold_store_sends_no_request_and_writes_nothing(
    grim_at, project_dir, unique_repo, oci_proxy, tmp_path, allow_writes
):
    # research_install_record_cache_test_matrix.md S50 (cold store);
    # adr_output_ownership_without_state.md validation case 34
    m = _machine(grim_at, project_dir, unique_repo, oci_proxy("count"))
    shutil.rmtree(m.work / ".grimoire")
    cold = cold_runner(m.runner, tmp_path, "empty")
    requests = m.requests()
    calls = _status_calls(cold, m.work, allow_writes=allow_writes, calls=2)
    assert calls == [{name: "missing" for name in NAMES}] * 2
    assert not (m.work / ".grimoire").exists()
    assert m.requests() == requests


# ===========================================================================
# Column b, `grim_status --allow-writes`: what each row proves and writes
# ===========================================================================

INSTALLED = dict.fromkeys(NAMES, "installed")
ROWS = {
    # line: (setup, states, written)
    "01-current": ("current", INSTALLED, False),
    "03-lagging": (
        "lagging",
        {**INSTALLED, "moving": "outdated", "style": "outdated"},
        False,
    ),
    "05-edited": (
        "edited",
        {**INSTALLED, "moving": "modified", "style": "modified"},
        False,
    ),
    "07-row4-match": ("row4", INSTALLED, True),
    "09-row4-no-match": (
        "row4-edited",
        {**INSTALLED, "moving": "modified", "style": "modified"},
        False,
    ),
    "11-row5-match": ("row5", INSTALLED, True),
    "13-row5-no-match": ("row5-edited", dict.fromkeys(NAMES, "missing"), False),
}


def _arrange(m: Machine, setup: str) -> None:
    if setup == "lagging":
        m.to_v2(keep_state=True, keep_files=True)
    elif setup == "edited":
        m.edit("moving", "style")
    elif setup.startswith("row4"):
        m.to_v2(keep_state=True, keep_files=False)
    elif setup.startswith("row5"):
        shutil.rmtree(m.work / ".grimoire")
    if setup == "row4-edited":
        m.edit("moving", "style")
    elif setup == "row5-edited":
        m.edit(*NAMES)


@pytest.mark.parametrize("line", list(ROWS))
def test_allow_writes_status_applies_column_b(
    grim_at, project_dir, unique_repo, oci_proxy, line
):
    # research_install_record_cache_test_matrix.md column b, CO lines 01 to 13
    setup, expected, written = ROWS[line]
    m = _machine(grim_at, project_dir, unique_repo, oci_proxy("count"))
    _arrange(m, setup)
    before = m.snapshot()
    requests = m.requests()
    (states,) = _status_calls(m.runner, m.work, allow_writes=True)
    assert states == expected
    if written:
        assert m.record_pins() == m.lock_pins()
        for output in before.files[1:]:
            assert output.path.stat().st_mtime_ns == output.mtime_ns, output.path
    else:
        assert_unchanged(before)
    again = m.snapshot()
    (states,) = _status_calls(m.runner, m.work, allow_writes=True)
    assert states == expected
    assert_unchanged(again)
    assert m.requests() == requests
