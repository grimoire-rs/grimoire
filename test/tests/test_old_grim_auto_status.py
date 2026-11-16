# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""grim 0.15.1 reads the records ``grim status`` wrote.

In a clone with no ``.grimoire/``, this grim's ``status`` fetches each lock pin,
proves the committed files against it and writes the records. grim 0.15.1, on
the same machine, reads that state file: ``status`` reports every artifact
``installed`` and ``install`` leaves every row ``unchanged``.
``research_install_record_cache_test_matrix.md`` S64 and
``adr_output_ownership_without_state.md`` validation case 36. The old binary
comes from ``GRIM_OLD_BIN`` (checked by ``src.old_grim``); the module skips
when it is unset.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from src.helpers import make_artifact, write_config
from src.old_grim import OLD_BIN, checked_old_grim
from src.registry import REGISTRY_HOST
from src.runner import GrimRunner

pytestmark = pytest.mark.skipif(
    not OLD_BIN, reason="GRIM_OLD_BIN names no grim 0.15.1 binary"
)

NAMES = ("moving", "style")


@pytest.fixture(scope="module")
def old_bin(tmp_path_factory: pytest.TempPathFactory) -> Path:
    return checked_old_grim(tmp_path_factory.mktemp("old-grim-identity"))


def _json(runner: GrimRunner, *args: str) -> tuple[int, dict, str]:
    result = runner.run(*args, format="json", check=False)
    try:
        return result.returncode, json.loads(result.stdout), result.stderr
    except json.JSONDecodeError:
        return result.returncode, {}, result.stderr


def test_old_grim_reads_the_records_status_wrote(team, unique_repo, old_bin):
    # research_install_record_cache_test_matrix.md S64
    t = team("CO")
    make_artifact(
        f"{unique_repo}/moving", "skill", {"moving/SKILL.md": "# moving\n"}, tag="1"
    )
    make_artifact(
        f"{unique_repo}/style",
        "rule",
        {"style.md": "---\npaths: ['**/*.rs']\n---\n# style\n"},
        tag="1",
    )
    write_config(
        t.alice.work,
        skills={"moving": f"{REGISTRY_HOST}/{unique_repo}/moving:1"},
        rules={"style": f"{REGISTRY_HOST}/{unique_repo}/style:1"},
    )
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: v1")
    t.alice.push()
    t.bob.pull()
    work = t.bob.work
    assert not (work / ".grimoire").exists()
    rc, doc, stderr = _json(t.bob.runner, "status")
    assert rc == 0, stderr
    assert {row["name"]: row["state"] for row in doc["items"]} == {
        n: "installed" for n in NAMES
    }
    records = json.loads(
        (work / ".grimoire" / "state.json").read_text(encoding="utf-8")
    )["records"]
    assert sorted(r["name"] for r in records) == sorted(NAMES)

    old = GrimRunner(old_bin, t.bob.runner.grim_home, cwd=work)
    old.env = dict(t.bob.runner.env)
    rc, doc, stderr = _json(old, "status")
    assert rc == 0, stderr
    assert {row["name"]: row["state"] for row in doc["items"]} == {
        n: "installed" for n in NAMES
    }
    rc, doc, stderr = _json(old, "install")
    assert rc == 0, stderr
    assert {row["name"]: row["status"] for row in doc["items"]} == {
        n: "unchanged" for n in NAMES
    }
