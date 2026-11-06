# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A dropped client never takes an adopted MCP entry with it."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

MCP_DESCRIPTOR = """\
description = "Test MCP server."

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
"""

ADOPTED = "hand-authored"
WRITTEN = "grim-written"
MEMBER = {"command": "grim", "args": ["mcp"]}
VSCODE_MEMBER = {"type": "stdio", "command": "grim", "args": ["mcp"]}


def _release_mcp(
    runner, project_dir: Path, registry: str, unique_repo: str, name: str
) -> str:
    src = project_dir / "src"
    src.mkdir(parents=True, exist_ok=True)
    descriptor = src / f"{name}.toml"
    descriptor.write_text(MCP_DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/{name}:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")
    return ref


def _write_config(project_dir: Path, refs: dict[str, str], clients: list[str]) -> None:
    clients_toml = ", ".join(f'"{c}"' for c in clients)
    members = "\n".join(f'{name} = "{ref}"' for name, ref in refs.items())
    (project_dir / "grimoire.toml").write_text(
        f"[options]\nclients = [{clients_toml}]\n\n[mcp]\n{members}\n"
    )


def _outputs(project_dir: Path, name: str) -> dict[str, dict]:
    state = json.loads((project_dir / ".grimoire/state.json").read_text())
    record = next(r for r in state["records"] if r["name"] == name)
    return {o["client"]: o for o in record["outputs"]}


def test_dropped_client_leaves_adopted_mcp_entry(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Narrowing ``[options].clients`` leaves a member grim never wrote.

    Claude's ``.mcp.json`` holds one member the user wrote and one grim wrote,
    both intact. Dropping Claude splices out only grim's. The other is named in
    ``abandoned_entries``, because the output leaving the record is the last
    moment anything points at it.
    """
    runner = grim_at(project_dir)
    refs = {
        name: _release_mcp(runner, project_dir, registry, unique_repo, name)
        for name in (ADOPTED, WRITTEN)
    }
    config = project_dir / ".mcp.json"
    config.write_text(json.dumps({"mcpServers": {ADOPTED: MEMBER}}, indent=2))
    _write_config(project_dir, refs, ["claude", "cursor"])
    runner.run("lock")
    runner.run("install")

    assert _outputs(project_dir, ADOPTED)["claude"]["adopted"] is True
    assert "adopted" not in _outputs(project_dir, WRITTEN)["claude"]
    assert json.loads(config.read_text())["mcpServers"] == {
        ADOPTED: MEMBER,
        WRITTEN: MEMBER,
    }

    _write_config(project_dir, refs, ["cursor"])
    rows = {r["name"]: r for r in runner.json("update")["items"]}

    assert json.loads(config.read_text())["mcpServers"] == {ADOPTED: MEMBER}, (
        "the reap must splice out grim's own member and leave the adopted one"
    )
    left = rows[ADOPTED]
    assert left["reaped_clients"] == ["claude"], left
    assert left["kept_modified_clients"] == [], left
    assert left["retained"] == [], left
    [entry] = left["abandoned_entries"]
    assert Path(entry["path"]).resolve() == config.resolve(), entry
    assert entry["pointer"] == f"/mcpServers/{ADOPTED}", entry

    removed = rows[WRITTEN]
    assert removed["reaped_clients"] == ["claude"], removed
    assert removed["abandoned_entries"] == [], removed

    for name in refs:
        assert set(_outputs(project_dir, name)) == {"cursor"}, (
            "the dropped client's output leaves the record either way"
        )


def _outputs_by_file(project_dir: Path, name: str, client: str) -> dict[str, dict]:
    state = json.loads((project_dir / ".grimoire/state.json").read_text())
    record = next(r for r in state["records"] if r["name"] == name)
    return {
        o["target"]["relative"]: o for o in record["outputs"] if o["client"] == client
    }


def _member(config: Path, name: str) -> dict | None:
    document = json.loads(config.read_text())
    for container in ("servers", "mcpServers"):
        if name in document.get(container, {}):
            return document[container][name]
    return None


@pytest.mark.parametrize("narrowed", [False, True], ids=["configured", "client-flag"])
@pytest.mark.parametrize("force", [False, True], ids=["plain", "force"])
def test_dropped_client_keeps_adopted_mcp_entry_across_a_pin_roll(
    grim_at,
    project_dir: Path,
    registry: str,
    unique_repo: str,
    force: bool,
    narrowed: bool,
) -> None:
    """The update that drops a client and moves the pin leaves its adopted entry.

    Copilot's ``.vscode/mcp.json`` holds a member the user wrote and grim
    adopted. One ``grim update`` drops copilot and moves the pin to a new value.
    The member stays byte-identical and is named in ``abandoned_entries``;
    ``--force`` removes it, as on every other reap path. ``--client copilot``
    names the dropped client, and ``[options].clients`` still decides.
    """
    runner = grim_at(project_dir)
    _release_mcp(runner, project_dir, registry, unique_repo, ADOPTED)
    repo = f"{registry}/{unique_repo}/mcp/{ADOPTED}"
    vscode = project_dir / ".vscode" / "mcp.json"
    vscode.parent.mkdir(parents=True)
    vscode.write_text(json.dumps({"servers": {ADOPTED: VSCODE_MEMBER}}, indent=2))
    _write_config(project_dir, {ADOPTED: f"{repo}:latest"}, ["copilot", "cursor"])
    runner.run("lock")
    runner.run("install")

    copilot = _outputs_by_file(project_dir, ADOPTED, "copilot")
    assert copilot[".vscode/mcp.json"]["adopted"] is True, copilot
    before = vscode.read_bytes()

    descriptor = project_dir / "src" / f"{ADOPTED}.toml"
    descriptor.write_text(
        MCP_DESCRIPTOR.replace('command = "grim"', 'command = "grim2"')
    )
    runner.json("release", str(descriptor), f"{repo}:2.0.0", "--kind", "mcp")
    _write_config(project_dir, {ADOPTED: f"{repo}:latest"}, ["cursor"])
    flags = [
        *(["--client", "copilot"] if narrowed else []),
        *(["--force"] if force else []),
    ]
    rows = runner.json("update", *flags)["items"]
    [row] = [r for r in rows if r["name"] == ADOPTED]

    assert row["action"] == "updated", row
    assert row["reaped_clients"] == ["copilot"], row
    assert row["kept_modified_clients"] == [], row
    assert set(_outputs_by_file(project_dir, ADOPTED, "copilot")) == set(), (
        "the dropped client's outputs leave the record either way"
    )
    assert _member(project_dir / ".github" / "mcp.json", ADOPTED) is None, (
        "the member grim wrote for the dropped client is reaped either way"
    )
    assert (
        _member(project_dir / ".cursor" / "mcp.json", ADOPTED)["command"] == "grim2"
    ), "the kept client rolls to the new pin"
    if force:
        assert _member(vscode, ADOPTED) is None, "--force removes the adopted entry"
        assert row["abandoned_entries"] == [], row
    else:
        assert vscode.read_bytes() == before, (
            "the pin roll must not rewrite or reap a member grim only adopted"
        )
        [entry] = row["abandoned_entries"]
        assert Path(entry["path"]).resolve() == vscode.resolve(), entry
        assert entry["pointer"] == f"/servers/{ADOPTED}", entry
        assert entry["element"] is None, entry


def test_install_keeps_adopted_mcp_entry_of_a_dropped_client_across_a_pin_roll(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """``grim install`` at a new pin leaves a dropped client's adopted member.

    The install reads ``[options].clients`` too, so it carries the member
    forward verbatim and still marked adopted. The ``grim update`` after it
    then leaves the member in the file instead of reaping it as grim's own.
    """
    runner = grim_at(project_dir)
    ref = _release_mcp(runner, project_dir, registry, unique_repo, ADOPTED)
    repo = ref.rsplit(":", 1)[0]
    vscode = project_dir / ".vscode" / "mcp.json"
    vscode.parent.mkdir(parents=True)
    vscode.write_text(json.dumps({"servers": {ADOPTED: VSCODE_MEMBER}}, indent=2))
    _write_config(project_dir, {ADOPTED: ref}, ["copilot", "cursor"])
    runner.run("lock")
    runner.run("install")
    before = vscode.read_bytes()

    descriptor = project_dir / "src" / f"{ADOPTED}.toml"
    descriptor.write_text(
        MCP_DESCRIPTOR.replace('command = "grim"', 'command = "grim2"')
    )
    runner.json("release", str(descriptor), f"{repo}:2.0.0", "--kind", "mcp")
    _write_config(project_dir, {ADOPTED: f"{repo}:2.0.0"}, ["cursor"])
    runner.run("lock")
    runner.run("install")

    assert vscode.read_bytes() == before, (
        "an install at a new pin must not roll a dropped client's adopted member"
    )
    copilot = _outputs_by_file(project_dir, ADOPTED, "copilot")
    assert copilot[".vscode/mcp.json"]["adopted"] is True, copilot

    [row] = [r for r in runner.json("update")["items"] if r["name"] == ADOPTED]
    assert vscode.read_bytes() == before, "the reap leaves the adopted member"
    assert row["reaped_clients"] == ["copilot"], row
    [entry] = row["abandoned_entries"]
    assert Path(entry["path"]).resolve() == vscode.resolve(), entry


def test_client_flag_update_at_the_same_pin_leaves_a_dropped_clients_adopted_entry(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """``update --client copilot`` at an unchanged pin leaves the adopted member.

    ``grim install`` rolled the pin after copilot left ``[options].clients``, so
    copilot's outputs were carried at the old value. A missing copilot file
    then makes the next ``update`` reconcile at the same pin, and
    ``--client copilot`` selects the dropped client. The member stays
    byte-identical and is named in ``abandoned_entries``.
    """
    runner = grim_at(project_dir)
    ref = _release_mcp(runner, project_dir, registry, unique_repo, ADOPTED)
    repo = ref.rsplit(":", 1)[0]
    vscode = project_dir / ".vscode" / "mcp.json"
    vscode.parent.mkdir(parents=True)
    vscode.write_text(json.dumps({"servers": {ADOPTED: VSCODE_MEMBER}}, indent=2))
    _write_config(project_dir, {ADOPTED: ref}, ["copilot", "cursor"])
    runner.run("lock")
    runner.run("install")
    before = vscode.read_bytes()

    descriptor = project_dir / "src" / f"{ADOPTED}.toml"
    descriptor.write_text(
        MCP_DESCRIPTOR.replace('command = "grim"', 'command = "grim2"')
    )
    runner.json("release", str(descriptor), f"{repo}:2.0.0", "--kind", "mcp")
    _write_config(project_dir, {ADOPTED: f"{repo}:2.0.0"}, ["cursor"])
    runner.run("lock")
    runner.run("install")
    assert vscode.read_bytes() == before, "the pin roll carries the member"
    (project_dir / ".github" / "mcp.json").unlink()

    rows = runner.json("update", "--client", "copilot")["items"]
    [row] = [r for r in rows if r["name"] == ADOPTED]

    assert vscode.read_bytes() == before, (
        "a reconciliation at the same pin must not rewrite or reap the member"
    )
    assert row["reaped_clients"] == ["copilot"], row
    [entry] = row["abandoned_entries"]
    assert Path(entry["path"]).resolve() == vscode.resolve(), entry
    assert entry["pointer"] == f"/servers/{ADOPTED}", entry


def _edit_member(config: Path, name: str) -> None:
    document = json.loads(config.read_text())
    for container in ("servers", "mcpServers"):
        if name in document.get(container, {}):
            document[container][name]["args"] = ["mcp", "--edited"]
    config.write_text(json.dumps(document, indent=2))


@pytest.mark.parametrize("roll", [False, True], ids=["same-pin", "pin-roll"])
def test_dropped_client_keeps_its_edited_file_and_reaps_the_other(
    grim_at, project_dir: Path, registry: str, unique_repo: str, roll: bool
) -> None:
    """Copilot writes two files; dropping it reaps the intact one only.

    The edited member stays recorded, ``grim status`` reads it as modified,
    and each list names copilot once. ``--force`` then reaps the edited one.
    A pin roll in the same update keeps the whole client out of the roll, so
    the edited file is never re-registered over.
    """
    runner = grim_at(project_dir)
    ref = _release_mcp(runner, project_dir, registry, unique_repo, WRITTEN)
    repo = ref.rsplit(":", 1)[0]
    ref = f"{repo}:latest"
    _write_config(project_dir, {WRITTEN: ref}, ["copilot", "cursor"])
    runner.run("lock")
    runner.run("install")
    vscode = project_dir / ".vscode" / "mcp.json"
    github = project_dir / ".github" / "mcp.json"
    assert set(_outputs_by_file(project_dir, WRITTEN, "copilot")) == {
        ".vscode/mcp.json",
        ".github/mcp.json",
    }
    _edit_member(github, WRITTEN)
    edited = github.read_bytes()
    if roll:
        descriptor = project_dir / "src" / f"{WRITTEN}.toml"
        descriptor.write_text(
            MCP_DESCRIPTOR.replace('command = "grim"', 'command = "grim2"')
        )
        runner.json("release", str(descriptor), f"{repo}:2.0.0", "--kind", "mcp")

    _write_config(project_dir, {WRITTEN: ref}, ["cursor"])
    [row] = [r for r in runner.json("update")["items"] if r["name"] == WRITTEN]

    if roll:
        assert row["action"] == "updated", row
    assert row["reaped_clients"] == ["copilot"], row
    assert row["kept_modified_clients"] == ["copilot"], row
    assert _member(vscode, WRITTEN) is None, "the intact member is reaped"
    assert github.read_bytes() == edited, "the edited member stays byte-identical"
    assert set(_outputs_by_file(project_dir, WRITTEN, "copilot")) == {
        ".github/mcp.json"
    }
    [status] = [r for r in runner.json("status")["items"] if r["name"] == WRITTEN]
    assert status["state"] == "modified", status

    [row] = [
        r for r in runner.json("update", "--force")["items"] if r["name"] == WRITTEN
    ]
    assert row["reaped_clients"] == ["copilot"], row
    assert row["kept_modified_clients"] == [], row
    assert _member(github, WRITTEN) is None, "--force reaps the edited member"
    assert set(_outputs_by_file(project_dir, WRITTEN, "copilot")) == set()
