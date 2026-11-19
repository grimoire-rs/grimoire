# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""What ``grim status`` says about a ``mod`` row.

A mod row reads the shared ladder (``installed``, ``missing``, ``modified``,
``outdated``), a gate or a refusal reads through ``arming`` per client, and
three advisory causes ride on ``arming`` without moving the row. Every "absent"
assertion sits beside a positive control in the same test.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
from pathlib import Path

import pytest

from src.helpers import make_artifact, make_mod, mod_sandbox, write_config

SKILL = "---\nname: control\ndescription: d\n---\n# control\n"
OPENCODE_V2 = {"opencode": ['plugin_api = "v2"']}
BOTH = ("--client", "claude", "--client", "opencode")
SETTINGS = Path(".claude/settings.local.json")
TREE = Path(".claude/mods/status-band")
TRUST = "--trust-workspace"

needs_git = pytest.mark.skipif(
    shutil.which("git") is None or os.name == "nt", reason="needs git and POSIX paths"
)


@pytest.fixture()
def git(tmp_path: Path):
    """Git blind to the host's own ignore files."""
    home = tmp_path / "git-home"
    home.mkdir()
    env = {
        **os.environ,
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(home),
        "GIT_CONFIG_NOSYSTEM": "1",
    }

    def run(cwd: Path, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["git", *args],
            cwd=cwd,
            env=env,
            check=True,
            capture_output=True,
            text=True,
            encoding="utf-8",
        )

    return run


def _project(project_dir, tmp_path, repo, grim_at, vendors=None, mods=None):
    """A flag-on project declaring one mod and one control skill, locked."""
    skill = make_artifact(f"{repo}/control", "skill", {"control/SKILL.md": SKILL}, "1")
    mods = mods or {"status-band": make_mod(f"{repo}/status-band", "1")}
    write_config(
        project_dir,
        skills={"control": skill.fq},
        mods={name: m.fq for name, m in mods.items()},
        experimental_mods=True,
        vendors=vendors,
    )
    runner = grim_at(project_dir)
    runner.env.update(mod_sandbox(tmp_path / "sandbox")[0])
    runner.run("lock")
    return runner


def _row(report: dict, name: str = "status-band") -> dict:
    [row] = [item for item in report["items"] if item["name"] == name]
    return row


def _causes(row: dict) -> list[tuple[str, str]]:
    return [(entry["client"], entry["cause"]) for entry in row["arming"]]


def _read_json(path: Path) -> dict:
    return json.loads(path.read_bytes().decode("utf-8"))


def _write_json(path: Path, value: dict) -> None:
    path.write_bytes((json.dumps(value, indent=2) + "\n").encode("utf-8"))


def _exclude_lines(project_dir: Path) -> list[str]:
    text = (project_dir / ".git" / "info" / "exclude").read_bytes().decode("utf-8")
    return [line.strip() for line in text.splitlines() if line.strip()]


def test_installed_row_serves_its_clients(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    runner = _project(project_dir, tmp_path, unique_repo, grim_at, OPENCODE_V2)
    first = runner.json("install", TRUST, *BOTH)
    lock = (project_dir / "grimoire.lock").read_bytes().decode("utf-8")
    [locked] = re.findall(
        r'name = "status-band".*?clients = \[([^\]]*)\]', lock, re.DOTALL
    )
    expected = re.findall(r'"([^"]+)"', locked)
    assert expected == ["claude", "opencode"], lock

    status = runner.json("status")

    row = _row(status)
    assert row["state"] == "installed"
    assert row["serves"] == expected
    assert row["arming"] == []
    assert {out["client"] for out in row["outputs"]} == {"claude", "opencode"}
    assert _row(first)["armed"] is None, "an install row carries no armed value"
    assert _row(status, "control")["serves"] is None, "positive control: a skill"


def test_flag_off_reads_feature_flag_off(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    runner = _project(project_dir, tmp_path, unique_repo, grim_at)
    runner.run("install", TRUST)
    written = (project_dir / TREE / "src" / "index.js").read_bytes()
    assert _row(runner.json("status"))["arming"] == []

    runner.run("config", "set", "options.experimental.mods", "false")
    row = _row(runner.json("status"))

    assert row["state"] == "gated"
    assert _causes(row) == [("claude", "feature-flag-off")]
    assert (project_dir / TREE / "src" / "index.js").read_bytes() == written, (
        "status writes nothing"
    )
    runner.run("config", "set", "options.experimental.mods", "true")
    row = _row(runner.json("status"))
    assert row["state"] == "installed"
    assert row["arming"] == []


@needs_git
def test_tracked_output_reads_mod_files_tracked(
    git, grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    git(project_dir, "init", "-q", "-b", "main")
    runner = _project(project_dir, tmp_path, unique_repo, grim_at)
    runner.run("install", TRUST)
    before = _row(runner.json("status"))
    assert before["state"] == "installed"
    assert before["arming"] == []

    git(project_dir, "add", "-f", str(TREE / "src" / "index.js"))
    row = _row(runner.json("status"))

    assert row["state"] == "installed", "an advisory cause never moves the row"
    assert _causes(row) == [("claude", "mod-files-tracked")]


def test_false_entry_reads_mod_disabled_in_client(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    runner = _project(project_dir, tmp_path, unique_repo, grim_at)
    runner.run("install", TRUST)
    settings = project_dir / SETTINGS
    data = _read_json(settings)
    [key] = [k for k in data["enabledPlugins"] if k.startswith("status-band@")]
    assert data["enabledPlugins"][key] is True
    assert _row(runner.json("status"))["arming"] == []

    data["enabledPlugins"][key] = False
    _write_json(settings, data)
    result = runner.run("status", format="json")
    row = _row(json.loads(result.stdout))

    assert result.returncode == 0
    assert row["state"] == "installed", "the row does not move"
    assert _causes(row) == [("claude", "mod-disabled-in-client")]
    assert (project_dir / TREE / "src" / "index.js").is_file(), "the files are kept"
    data["enabledPlugins"][key] = True
    _write_json(settings, data)
    assert _row(runner.json("status"))["arming"] == []


def test_fresh_clone_reads_workspace_not_consented(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    runner = _project(project_dir, tmp_path, unique_repo, grim_at)

    result = runner.run("install", check=False)

    assert result.returncode == 0, result.stderr
    assert not (project_dir / TREE).exists(), "nothing is written without consent"
    row = _row(runner.json("status"))
    assert row["state"] == "gated"
    assert _causes(row) == [("claude", "workspace-not-consented")]
    assert "grim consent allow" in row["arming"][0]["message"]
    assert (project_dir / ".claude" / "skills" / "control").is_dir(), (
        "positive control: the skill of the same run is written"
    )

    runner.run("consent", "allow")
    runner.run("install")
    row = _row(runner.json("status"))
    assert row["state"] == "installed"
    assert row["arming"] == []


@needs_git
def test_no_exclude_line_for_mod_outputs(
    git, grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    git(project_dir, "init", "-q", "-b", "main")
    runner = _project(project_dir, tmp_path, unique_repo, grim_at, OPENCODE_V2)

    runner.run("install", TRUST, *BOTH)

    listed = git(project_dir, "status", "--porcelain", "--untracked-files=all").stdout
    assert ".claude/mods/status-band/src/index.js" in listed, (
        "positive control: the output exists and git sees it"
    )
    assert ".opencode/plugins/status-band/index.ts" in listed
    assert not [
        line
        for line in _exclude_lines(project_dir)
        if "mods" in line or "plugins" in line or "status-band" in line
    ], _exclude_lines(project_dir)


@needs_git
def test_settings_local_exclude_line_stays_with_mods_only(
    git, grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    git(project_dir, "init", "-q", "-b", "main")
    exclude = project_dir / ".git" / "info" / "exclude"
    exclude.write_bytes(exclude.read_bytes() + f"{SETTINGS}\n".encode())
    runner = _project(project_dir, tmp_path, unique_repo, grim_at)
    runner.run("install", TRUST)
    assert (project_dir / SETTINGS).is_file(), "positive control: the mod uses the file"
    assert _exclude_lines(project_dir).count(str(SETTINGS)) == 1

    runner.run("uninstall", "mod", "status-band")

    assert not (project_dir / TREE).exists(), "the mod is gone"
    assert _exclude_lines(project_dir).count(str(SETTINGS)) == 1


def test_hand_edited_path_source_reads_not_armed(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    good = make_mod(f"{unique_repo}/good", "1")
    edited = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(
        project_dir,
        tmp_path,
        unique_repo,
        grim_at,
        OPENCODE_V2,
        {"status-band": edited, "good": good},
    )
    lock = project_dir / "grimoire.lock"
    text = lock.read_bytes().decode("utf-8")
    block = re.search(
        r'\[\[mod\]\]\nname = "status-band".*?(?=\n\[\[|\Z)', text, re.DOTALL
    )
    assert block, text
    swapped = re.sub(
        r'pinned = "[^"]*"',
        'path = "./dev-mod"\nhash = "sha256:' + "0" * 64 + '"',
        block.group(0),
    )
    assert swapped != block.group(0), "the edit must change the row"
    lock.write_bytes(text.replace(block.group(0), swapped).encode("utf-8"))
    runner.run("install", TRUST, *BOTH, check=False)

    status = runner.json("status")

    row = _row(status)
    assert row["state"] == "not-armed"
    assert _causes(row) == [
        ("claude", "not-registered"),
        ("opencode", "not-registered"),
    ]
    assert _row(status, "good")["state"] == "installed", "positive control"
    assert _row(status, "good")["arming"] == []


def test_unwritten_mod_with_files_reads_mod_files_present(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    runner = _project(project_dir, tmp_path, unique_repo, grim_at)
    runner.run("consent", "allow")
    before = _row(runner.json("status"))
    assert before["arming"] == [], "positive control: no directory, no cause"

    (project_dir / TREE / "src").mkdir(parents=True)
    (project_dir / TREE / "src" / "index.js").write_bytes(b"// hand made\n")
    row = _row(runner.json("status"))

    assert _causes(row) == [("claude", "mod-files-present")]
    assert row["state"] == before["state"], "an advisory cause never moves the row"
    assert row["outputs"] == [], "grim wrote nothing"


def test_revoked_mod_reads_installed_until_the_next_install(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    runner = _project(project_dir, tmp_path, unique_repo, grim_at)
    runner.run("consent", "allow")
    runner.run("install")
    assert (project_dir / TREE / "src" / "index.js").is_file(), "positive control"
    assert _row(runner.json("status"))["state"] == "installed"

    assert runner.json("consent", "revoke")["action"] == "revoked"
    row = _row(runner.json("status"))

    assert row["state"] == "installed", "a written output is the arming evidence"
    assert row["arming"] == []

    runner.run("install")
    row = _row(runner.json("status"))
    assert row["state"] == "gated"
    assert _causes(row) == [("claude", "workspace-not-consented")]
    assert not (project_dir / TREE).exists(), "the next run disarms"


def test_drifted_mod_reads_installed_until_the_next_install(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    drifting = make_mod(f"{unique_repo}/status-band", "1")
    covered = make_mod(f"{unique_repo}/good", "1")
    runner = _project(
        project_dir,
        tmp_path,
        unique_repo,
        grim_at,
        mods={"status-band": drifting, "good": covered},
    )
    runner.run("consent", "allow")
    runner.run("install")
    assert (project_dir / TREE / "src" / "index.js").is_file(), "positive control"
    assert _row(runner.json("status"))["state"] == "installed"

    # Drop only status-band's key: the record still covers `good`.
    [record] = sorted((Path(runner.grim_home) / "consent").glob("*.json"))
    data = _read_json(record)
    kept = [k for k in data["sources"] if not k.startswith("status-band@")]
    assert len(kept) == len(data["sources"]) - 1, data
    data["sources"] = kept
    _write_json(record, data)
    row = _row(runner.json("status"))

    assert row["state"] == "installed", "a written output is the arming evidence"
    assert row["arming"] == []

    runner.run("install")
    status = runner.json("status")
    row = _row(status)
    assert row["state"] == "gated"
    assert _causes(row) == [("claude", "consent-drifted")]
    assert not (project_dir / TREE).exists(), "the next run disarms"
    assert _row(status, "good")["state"] == "installed", (
        "a covered mod keeps converging"
    )
    assert (project_dir / ".claude/mods/good/src/index.js").is_file()


def _market_name(project_dir: Path) -> str:
    (name,) = _read_json(project_dir / SETTINGS)["extraKnownMarketplaces"]
    return name


def test_hand_edited_marketplace_file_reads_modified(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    runner = _project(project_dir, tmp_path, unique_repo, grim_at)
    runner.run("install", TRUST)
    market = project_dir / ".claude/mods/.claude-plugin/marketplace.json"
    assert _row(runner.json("status"))["state"] == "installed", "positive control"
    before = market.read_bytes()

    market.write_bytes(b'{"hand": "edited"}\n')
    row = _row(runner.json("status"))

    assert row["state"] == "modified"
    assert market.read_bytes() == b'{"hand": "edited"}\n', "status writes nothing"
    market.write_bytes(before)
    assert _row(runner.json("status"))["state"] == "installed"


def test_hand_edited_marketplace_entry_reads_modified(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    runner = _project(project_dir, tmp_path, unique_repo, grim_at)
    runner.run("install", TRUST)
    settings = project_dir / SETTINGS
    data = _read_json(settings)
    name = _market_name(project_dir)
    ours = data["extraKnownMarketplaces"][name]
    assert _row(runner.json("status"))["state"] == "installed", "positive control"

    data["extraKnownMarketplaces"][name] = {"source": {"source": "hand-made"}}
    _write_json(settings, data)
    assert _row(runner.json("status"))["state"] == "modified"

    del data["extraKnownMarketplaces"][name]
    _write_json(settings, data)
    assert _row(runner.json("status"))["state"] == "modified", "a missing entry drifts"

    data["extraKnownMarketplaces"][name] = ours
    _write_json(settings, data)
    assert _row(runner.json("status"))["state"] == "installed"


def test_disabled_plugin_value_is_not_shared_drift(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    runner = _project(project_dir, tmp_path, unique_repo, grim_at)
    runner.run("install", TRUST)
    settings = project_dir / SETTINGS
    data = _read_json(settings)
    [key] = [k for k in data["enabledPlugins"] if k.startswith("status-band@")]

    data["enabledPlugins"][key] = False
    _write_json(settings, data)
    row = _row(runner.json("status"))

    assert row["state"] == "installed"
    assert _causes(row) == [("claude", "mod-disabled-in-client")]
