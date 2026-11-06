# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""An output its record's kind may not address does not survive the state load.

The rule is ``adr_settings_artifact_kind.md``, Decision 6: legality reads the
kind's anchor table and the vendor's own surfaces, never the path alone, and an
ordinary record keeps every output.
"""
from __future__ import annotations

import copy
import json
from pathlib import Path

import pytest

from src.helpers import make_artifact, write_config

_DIGEST = "sha256:" + "a" * 64
_PIN = f"localhost:5000/grim-test/state-load@{_DIGEST}"
_WARNING = "not a surface"
_GOLDEN = Path(__file__).resolve().parents[1] / "data" / "golden"
_SETTINGS = {"permissions": {"deny": ["Read(./.env)"]}, "theme": "dark"}
_MCP_DESCRIPTOR = """\
description = "Test MCP server."

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
"""
_ALL_CLIENTS = [
    "claude",
    "opencode",
    "copilot",
    "codex",
    "cursor",
    "kiro",
    "junie",
    "gemini",
    "zed",
    "amp",
    "agents",
    "antigravity",
    "cline",
    "droid",
    "goose",
    "warp",
    "openclaw",
    "kilo",
    "qoder",
]


def _output(
    relative: str,
    *,
    client: str = "claude",
    anchor: str = "workspace",
    entry: str | None = None,
) -> dict:
    output = {
        "client": client,
        "target": {"anchor": anchor, "relative": relative},
        "content_hash": _DIGEST,
    }
    if entry is not None:
        output["entry"] = entry
    return output


def _record(kind: str, name: str, output: dict) -> dict:
    return {"kind": kind, "name": name, "pinned": _PIN, "outputs": [output]}


def _write_state(path: Path, records: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"version": 2, "records": records}, indent=2) + "\n")


def _warnings(stderr: str) -> list[str]:
    return [line for line in stderr.splitlines() if _WARNING in line]


def _names(state: Path) -> set[str]:
    return {record["name"] for record in json.loads(state.read_text())["records"]}


@pytest.mark.parametrize(
    ("kind", "entry"),
    [
        pytest.param("skill", None, id="skill"),
        pytest.param("rule", None, id="rule"),
        pytest.param("agent", None, id="agent"),
        pytest.param("hook", None, id="hook"),
        pytest.param("bundle", None, id="bundle"),
        pytest.param("mcp", None, id="mcp-file"),
        pytest.param("mcp", "/permissions/deny", id="mcp-entry"),
    ],
)
def test_outputs_outside_their_kind_are_dropped(
    grim_at, project_dir: Path, kind: str, entry: str | None
) -> None:
    """No kind reaches a file outside its surfaces through a recorded output.

    The legitimate orphan beside it is the proof the run really pruned.
    """
    settings = project_dir / ".claude" / "settings.json"
    settings.write_text(json.dumps(_SETTINGS, indent=2))
    before = settings.read_bytes()
    control = project_dir / ".claude" / "rules" / "control.md"
    control.parent.mkdir(parents=True)
    control.write_text("# control\n")
    state = project_dir / ".grimoire" / "state.json"
    _write_state(
        state,
        [
            _record(kind, "outsider", _output(".claude/settings.json", entry=entry)),
            _record("rule", "control", _output(".claude/rules/control.md")),
        ],
    )
    write_config(project_dir)

    result = grim_at(project_dir).run("update", "--force")

    assert not control.exists(), "the control orphan must really be pruned"
    assert settings.read_bytes() == before
    warned = _warnings(result.stderr)
    assert warned, result.stderr
    assert all("outsider" in line for line in warned), warned
    assert _names(state) == set(), "both records are orphans of an empty declaration"


def test_the_warning_escapes_what_it_quotes(grim_at, project_dir: Path) -> None:
    """A record's text reaches a terminal, and a state file can arrive by clone."""
    _write_state(
        project_dir / ".grimoire" / "state.json",
        [_record("skill", "out\u001b[31msider", _output(".claude/settings.json"))],
    )
    write_config(project_dir)

    result = grim_at(project_dir).run("status")

    assert _warnings(result.stderr), result.stderr
    assert "\u001b[31m" not in result.stderr
    assert "\\u{1b}[31m" in result.stderr


@pytest.mark.parametrize(
    ("client", "config", "pointer"),
    [
        pytest.param("gemini", ".gemini/settings.json", "/mcpServers/srv", id="gemini"),
        pytest.param("qoder", ".qoder/settings.json", "/mcpServers/srv", id="qoder"),
        pytest.param("zed", ".zed/settings.json", "/context_servers/srv", id="zed"),
        pytest.param("amp", ".amp/settings.json", "/amp.mcpServers/srv", id="amp"),
    ],
)
def test_ordinary_mcp_record_keeps_its_outputs(
    grim_at,
    project_dir: Path,
    registry: str,
    unique_repo: str,
    client: str,
    config: str,
    pointer: str,
) -> None:
    """An MCP member sharing a vendor settings file stays managed end to end.

    Uninstall splicing the member out, and only the member, is the proof the
    output survived the load rather than the rule never having run on it.
    """
    runner = grim_at(project_dir)
    descriptor = project_dir / "srv.toml"
    descriptor.write_text(_MCP_DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/srv:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")
    settings = project_dir / config
    settings.parent.mkdir(parents=True, exist_ok=True)
    settings.write_text(json.dumps({"userKey": True}, indent=2))
    (project_dir / "grimoire.toml").write_text(
        f'[options]\nclients = ["{client}"]\n\n[mcp]\nsrv = "{ref}"\n'
    )
    state = project_dir / ".grimoire" / "state.json"

    runner.run("lock")
    runner.run("install")
    recorded = state.read_bytes()
    [output] = next(
        r for r in json.loads(recorded)["records"] if r["name"] == "srv"
    )["outputs"]
    assert output["target"] == {"anchor": "workspace", "relative": config}
    assert output["entry"] == pointer

    again = runner.run("install")
    status = runner.run("status")

    assert state.read_bytes() == recorded
    assert _warnings(again.stderr) == [], again.stderr
    assert _warnings(status.stderr) == [], status.stderr

    # A dotted Amp key is one JSON member, so the pointer's parent is read whole.
    container = pointer.split("/")[1]
    assert "srv" in json.loads(settings.read_text())[container]

    runner.run("uninstall", "mcp", "srv")

    left = json.loads(settings.read_text())
    assert left["userKey"] is True
    assert "srv" not in left.get(container, {})


@pytest.mark.parametrize(
    ("fixture", "name"),
    [
        pytest.param("pre_hooks_03e59b0", "state.project.json", id="pre-hooks-project"),
        pytest.param("pre_hooks_03e59b0", "state.global.json", id="pre-hooks-global"),
        pytest.param(
            "pre_dependencies_cdeedcaa", "state.project.json", id="pre-deps-project"
        ),
        pytest.param(
            "pre_dependencies_cdeedcaa",
            "state.project.after-remove.json",
            id="pre-deps-after-remove",
        ),
        pytest.param(
            "pre_dependencies_cdeedcaa", "state.global.json", id="pre-deps-global"
        ),
    ],
)
def test_a_state_the_shipped_grim_wrote_loads_byte_identical(
    grim_at, project_dir: Path, grim_home: Path, fixture: str, name: str
) -> None:
    """A golden state round-trips through the real binary with nothing dropped.

    The seeded outsider is the proof the rule ran: it alone is warned about,
    and once both seeded records are gone the bytes are the golden ones.
    """
    golden = (_GOLDEN / fixture / "golden" / name).read_bytes()
    if "global" in name:
        scope = ["--global"]
        cwd = grim_home
        state = grim_home / "state" / "global.json"
        control = _output("rules/control.md", anchor="claude-root")
        outsider = _output("settings.json", anchor="claude-root")
    else:
        scope = []
        cwd = project_dir
        state = project_dir / ".grimoire" / "state.json"
        control = _output(".claude/rules/control.md")
        outsider = _output(".claude/settings.json")
    seeded = json.loads(golden)["records"]
    seeded.append(_record("rule", "control", control))
    seeded.append(_record("skill", "outsider", outsider))
    _write_state(state, seeded)
    write_config(cwd, options={"clients": '["claude"]'})
    runner = grim_at(cwd)

    first = runner.run("uninstall", *scope, "rule", "control")
    runner.run("uninstall", *scope, "skill", "outsider")

    warned = _warnings(first.stderr)
    assert warned, first.stderr
    assert all("outsider" in line for line in warned), warned
    assert state.read_bytes() == golden


@pytest.mark.parametrize("scope", [pytest.param([], id="project"), pytest.param(["--global"], id="global")])
def test_every_client_keeps_what_it_installed(
    grim_at,
    project_dir: Path,
    grim_home: Path,
    registry: str,
    unique_repo: str,
    scope: list[str],
) -> None:
    """Every vendor's real outputs reload whole, at both scopes.

    A hand-written outsider beside each real output, naming that output's own
    client and anchor, is the proof the rule ran for every one of them.
    """
    cwd = grim_home if scope else project_dir
    runner = grim_at(cwd)
    skill = make_artifact(
        f"{unique_repo}/plain-skill",
        "skill",
        {"plain-skill/SKILL.md": "---\nname: plain-skill\ndescription: d\n---\nbody\n"},
    )
    rule = make_artifact(f"{unique_repo}/plain-rule", "rule", {"plain-rule.md": "# rule\n"})
    agent = make_artifact(
        f"{unique_repo}/plain-agent",
        "agent",
        {"plain-agent.md": "---\nname: plain-agent\ndescription: d\n---\nbody\n"},
    )
    descriptor = cwd / "plain-mcp.toml"
    descriptor.write_text(_MCP_DESCRIPTOR)
    mcp = f"{registry}/{unique_repo}/mcp/plain-mcp:1.0.0"
    runner.json("release", str(descriptor), mcp, "--kind", "mcp")
    write_config(
        cwd,
        skills={"plain-skill": skill.fq},
        rules={"plain-rule": rule.fq},
        agents={"plain-agent": agent.fq},
    )
    with (cwd / "grimoire.toml").open("a", encoding="utf-8") as config:
        config.write(f'[mcp]\nplain-mcp = "{mcp}"\n')
    clients = ",".join(_ALL_CLIENTS)
    state = (
        grim_home / "state" / "global.json"
        if scope
        else project_dir / ".grimoire" / "state.json"
    )

    runner.run("lock", *scope)
    runner.run("install", *scope, "--client", clients)
    written = state.read_bytes()
    outputs = {
        output["client"]
        for record in json.loads(written)["records"]
        for output in record["outputs"]
    }
    assert len(outputs) >= 17, sorted(outputs)

    again = runner.run("install", *scope, "--client", clients)

    assert _warnings(again.stderr) == [], again.stderr
    assert state.read_bytes() == written

    records = json.loads(written)["records"]
    expected = set()
    for record in records:
        for output in list(record["outputs"]):
            outsider = copy.deepcopy(output)
            target = outsider["target"]
            target["relative"] += ".outsider"
            if "entry" in outsider:
                outsider["entry"] = "/outsider/member"
            record["outputs"].append(outsider)
            expected.add(f"{output['client']} at {target['anchor']}/{target['relative']}")
    _write_state(state, records)

    warned = _warnings(runner.run("status", *scope).stderr)

    assert {needle for needle in expected if any(needle in line for line in warned)} == expected
    assert all(".outsider" in line for line in warned), warned
