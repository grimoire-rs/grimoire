# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""What the opt-in usage telemetry records for install, update and add.

Observed through the single `telemetry payload <json>` debug line, so no
collector is involved: the line is built before any export is attempted.
`GrimRunner` hands the child a minimal environment, so no ambient
`OTEL_*` / `GRIM_TELEMETRY*` / `DO_NOT_TRACK` / `CI` leaks in.
"""
from __future__ import annotations

from pathlib import Path
from typing import Any

from src.helpers import make_artifact, make_bundle, write_config
from src.otlp_fixture import telemetry_payload
from src.registry import retag
from src.runner import GrimRunner

COMMAND = "grim.command"
ARTIFACT = "grim.artifact.changed"
TIMEOUT = 60


def _armed(runner: GrimRunner) -> GrimRunner:
    runner.env["GRIM_LOG"] = "debug"
    runner.env["GRIM_TELEMETRY"] = "1"
    return runner


def _events(result: Any) -> dict[str, list[dict[str, Any]]]:
    """The payload's events, grouped by name, attributes only, in emit order."""
    grouped: dict[str, list[dict[str, Any]]] = {}
    for event in telemetry_payload(result.stderr)["events"]:
        grouped.setdefault(event["name"], []).append(event["attributes"])
    return grouped


def _command(result: Any) -> dict[str, Any]:
    commands = _events(result)[COMMAND]
    assert len(commands) == 1, commands
    return commands[0]


def _skill(repo: str, body: str, tag: str):
    return make_artifact(repo, "skill", {"s/SKILL.md": body}, tag=tag)


def _two_skills(project_dir: Path, registry: str, unique_repo: str):
    a_repo, b_repo = f"{unique_repo}/alpha", f"{unique_repo}/beta"
    _skill(a_repo, "a1\n", "stable")
    _skill(b_repo, "b1\n", "1.0.0")
    write_config(
        project_dir,
        skills={
            "alpha": f"{registry}/{a_repo}:stable",
            "beta": f"{registry}/{b_repo}:1.0.0",
        },
    )
    return a_repo, b_repo


def test_command_event_status_project(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    _two_skills(project_dir, registry, unique_repo)
    runner = _armed(grim_at(project_dir))
    result = runner.run("status", timeout=TIMEOUT, check=False)

    attrs = _command(result)
    assert attrs["grim.command.name"] == "status"
    assert "grim.command.subcommand" not in attrs
    assert attrs["process.exit.code"] == result.returncode
    assert attrs["grim.scope"] == "project"
    assert attrs["grim.ci"] is False
    assert attrs["grim.command.duration"] >= 0
    assert "error.type" not in attrs
    # Only install/update/add carry client sets; no artifact events either.
    assert "grim.clients" not in attrs
    assert ARTIFACT not in _events(result)


def test_command_event_failure_sets_error_type(
    grim_at, project_dir: Path
) -> None:
    runner = _armed(grim_at(project_dir))
    # No config anywhere: install refuses with 79 exactly as when unarmed.
    result = runner.run("install", timeout=TIMEOUT, check=False)

    assert result.returncode == 79, result.stderr
    attrs = _command(result)
    assert attrs["grim.command.name"] == "install"
    assert attrs["process.exit.code"] == 79
    assert attrs["error.type"] == "no-config"
    assert ARTIFACT not in _events(result)


def test_command_event_global_scope(
    grim_at, project_dir: Path, grim_home: Path
) -> None:
    (grim_home / "grimoire.toml").write_text("[skills]\n")
    runner = _armed(grim_at(project_dir))
    result = runner.run("status", "--global", timeout=TIMEOUT, check=False)

    attrs = _command(result)
    assert attrs["grim.command.name"] == "status"
    assert attrs["grim.scope"] == "global"


def test_clients_recorded_on_install(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    _two_skills(project_dir, registry, unique_repo)
    runner = grim_at(project_dir)
    runner.run("lock", timeout=TIMEOUT)
    _armed(runner)
    result = runner.run("install", "--client", "claude", timeout=TIMEOUT)

    attrs = _command(result)
    assert attrs["grim.command.name"] == "install"
    assert attrs["grim.clients"] == ["claude"]
    # `.claude/` is the only marker the fixture workspace carries; detection
    # ran before the install wrote anything.
    assert attrs["grim.clients.detected"] == ["claude"]


def test_install_emits_one_event_per_changed_artifact(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    a_repo, b_repo = _two_skills(project_dir, registry, unique_repo)
    runner = grim_at(project_dir)
    runner.run("lock", timeout=TIMEOUT)
    _armed(runner)

    first = runner.run("install", timeout=TIMEOUT)
    events = _events(first)
    assert len(events[COMMAND]) == 1
    changed = events[ARTIFACT]
    assert len(changed) == 2
    assert {e["grim.artifact.ref"] for e in changed} == {
        f"{registry}/{a_repo}:stable",
        f"{registry}/{b_repo}:1.0.0",
    }
    assert {e["grim.artifact.kind"] for e in changed} == {"skill"}
    assert {e["grim.artifact.action"] for e in changed} == {"installed"}
    for event in changed:
        assert "sha256" not in event["grim.artifact.ref"]
        assert "@" not in event["grim.artifact.ref"]

    # Nothing changed the second time: the command event alone is sent.
    second = runner.run("install", timeout=TIMEOUT)
    again = _events(second)
    assert len(again[COMMAND]) == 1
    assert ARTIFACT not in again


def test_add_emits_added(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    repo = f"{unique_repo}/code-review"
    _skill(repo, "cr\n", "stable")
    write_config(project_dir)
    runner = _armed(grim_at(project_dir))
    result = runner.run("add", f"{registry}/{repo}:stable", timeout=TIMEOUT)

    events = _events(result)
    assert events[COMMAND][0]["grim.command.name"] == "add"
    assert events[COMMAND][0]["grim.clients"] == ["claude"]
    assert events[ARTIFACT] == [
        {
            "grim.artifact.ref": f"{registry}/{repo}:stable",
            "grim.artifact.kind": "skill",
            "grim.artifact.action": "added",
        }
    ]


def test_update_emits_updated(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    repo = f"{unique_repo}/code-review"
    other = f"{unique_repo}/other"
    _skill(repo, "v1\n", "stable")
    _skill(other, "o1\n", "stable")
    write_config(
        project_dir,
        skills={
            "code-review": f"{registry}/{repo}:stable",
            "other": f"{registry}/{other}:stable",
        },
    )
    runner = grim_at(project_dir)
    runner.run("lock", timeout=TIMEOUT)
    runner.run("install", timeout=TIMEOUT)

    v2 = _skill(repo, "v2\n", "2.0.0")
    retag(repo, "stable", v2.digest)
    _armed(runner)
    result = runner.run("update", timeout=TIMEOUT)

    events = _events(result)
    assert events[COMMAND][0]["grim.command.name"] == "update"
    assert events[COMMAND][0]["grim.clients"] == ["claude"]
    # The unchanged artifact stays silent.
    assert events[ARTIFACT] == [
        {
            "grim.artifact.ref": f"{registry}/{repo}:stable",
            "grim.artifact.kind": "skill",
            "grim.artifact.action": "updated",
        }
    ]


def test_update_emits_removed_for_a_dropped_artifact(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    a_repo, b_repo = _two_skills(project_dir, registry, unique_repo)
    runner = grim_at(project_dir)
    runner.run("lock", timeout=TIMEOUT)
    runner.run("install", timeout=TIMEOUT)

    write_config(project_dir, skills={"alpha": f"{registry}/{a_repo}:stable"})
    _armed(runner)
    result = runner.run("update", timeout=TIMEOUT, check=False)

    assert result.returncode == 0, result.stderr
    assert _events(result)[ARTIFACT] == [
        {
            "grim.artifact.ref": f"{registry}/{b_repo}",
            "grim.artifact.kind": "skill",
            "grim.artifact.action": "removed",
        }
    ]


def test_update_refused_for_a_modified_artifact_emits_no_updated_event(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    a_repo, b_repo = _two_skills(project_dir, registry, unique_repo)
    runner = grim_at(project_dir)
    runner.run("lock", timeout=TIMEOUT)
    runner.run("install", timeout=TIMEOUT)

    # Both skills have a newer pin; only the unedited one may be updated.
    retag(a_repo, "stable", _skill(a_repo, "a2\n", "2.0.0").digest)
    retag(b_repo, "1.0.0", _skill(b_repo, "b2\n", "1.0.1").digest)
    edited = project_dir / ".claude/skills/alpha/SKILL.md"
    edited.write_text(edited.read_text() + "\n# local edit\n")
    _armed(runner)
    result = runner.run("update", timeout=TIMEOUT, check=False)

    assert result.returncode == 65, result.stderr
    assert _command(result)["process.exit.code"] == 65
    assert _events(result)[ARTIFACT] == [
        {
            "grim.artifact.ref": f"{registry}/{b_repo}:1.0.0",
            "grim.artifact.kind": "skill",
            "grim.artifact.action": "updated",
        }
    ]


def test_add_of_a_bundle_emits_one_bundle_event_and_no_member_events(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    members = []
    for name in ("skill-a", "skill-b"):
        member = make_artifact(
            f"{unique_repo}/{name}",
            "skill",
            {f"{name}/SKILL.md": f"---\nname: {name}\n---\n# {name}\n"},
            tag="stable",
        )
        members.append(("skill", name, member.fq))
    bundle_repo = f"{unique_repo}/stack"
    make_bundle(bundle_repo, members, tag="1.0.0")
    write_config(project_dir)
    runner = _armed(grim_at(project_dir))
    result = runner.run("add", f"{registry}/{bundle_repo}:1.0.0", timeout=TIMEOUT)

    assert _events(result)[ARTIFACT] == [
        {
            "grim.artifact.ref": f"{registry}/{bundle_repo}:1.0.0",
            "grim.artifact.kind": "bundle",
            "grim.artifact.action": "added",
        }
    ]
    # The members really installed; they are just not reported.
    assert (project_dir / ".claude/skills/skill-a/SKILL.md").is_file()


def test_dev_install_emits_no_artifact_events(
    grim_at, project_dir: Path
) -> None:
    write_config(project_dir)
    skill = project_dir / "dev-skill"
    skill.mkdir()
    (skill / "SKILL.md").write_text(
        "---\nname: dev-skill\ndescription: Dev.\n---\n# Dev\n"
    )
    runner = grim_at(project_dir)
    runner.run("lock", timeout=TIMEOUT)
    _armed(runner)
    result = runner.run("install", "./dev-skill", "--client", "claude", timeout=TIMEOUT)

    assert (project_dir / ".claude/skills/dev-skill/SKILL.md").is_file()
    assert _command(result)["grim.command.name"] == "install"
    assert ARTIFACT not in _events(result)


def test_path_source_has_no_ref(
    grim_at, project_dir: Path, registry: str
) -> None:
    skill = project_dir / "skills" / "local-notes"
    skill.mkdir(parents=True)
    (skill / "SKILL.md").write_text(
        "---\nname: local-notes\ndescription: d\n---\n# notes\n"
    )
    write_config(project_dir)
    runner = _armed(grim_at(project_dir))
    result = runner.run("add", "./skills/local-notes", timeout=TIMEOUT)

    events = _events(result)
    assert events[ARTIFACT] == [
        {"grim.artifact.kind": "skill", "grim.artifact.action": "added"}
    ]
    # No local path leaks anywhere in the payload.
    assert "local-notes" not in str(telemetry_payload(result.stderr)["events"])
    assert str(project_dir) not in str(telemetry_payload(result.stderr))
