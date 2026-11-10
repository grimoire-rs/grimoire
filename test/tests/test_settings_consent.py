# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Workspace consent for the ``settings`` kind: one record, one question, two kinds.

``settings`` keys share the hook consent record under a ``settings:`` prefix
(``adr_settings_artifact_kind.md``), so the record an older grim left is a
record that covers hooks and no package. These tests drive the question itself,
which needs a terminal on stdin and stderr, so ``_on_a_terminal`` gives the
child a pseudo-terminal. Every "no prompt" and "nothing was written" assertion
sits beside a run where the prompt does appear or the file does change.
"""

from __future__ import annotations

import hashlib
import json
import os
import select
import subprocess
import time
from pathlib import Path

import pytest

from src.helpers import make_artifact, make_bundle, make_dependent, make_settings, settings_sandbox, write_config
from src.otlp_fixture import telemetry_payload
from src.registry import REGISTRY_HOST

pytestmark = pytest.mark.skipif(
    os.name == "nt", reason="the hook launcher and its registered command are POSIX-only in v1"
)

pty = pytest.importorskip("pty")

DOCUMENT = {"description": "Team defaults", "claude": {"model": "sonnet"}}

# Where a project install writes DOCUMENT.
LOCAL = ".claude/settings.local.json"

HOOK_TOML = """\
schema = 1
name = "shell-guard"
description = "Observes Bash tool calls before they run."

[[hooks]]
id = "guard"
event = "PreToolUse"
tier = "observer"
matcher = "Bash"
command = "sh guard.sh"
timeout = 5
"""

GUARD_SH = "#!/bin/sh\ncat > /dev/null\necho '{}'\n"

# The one piece of the question its wording never moves.
PROMPT = "[y/N]"

HOOK_CLAUSE = "A hook is code your AI client runs automatically."
SETTINGS_CLAUSE = "Settings change how your AI client behaves and what it may do."
OTHER_CHECKOUTS = "It grants nothing to any other checkout."
DECLINING = "Declining arms nothing and changes no file."

ANSWERS = [
    (("--no-trust-workspace",), False),
    (("--trust-workspace",), True),
    (("--no-trust-hooks",), False),
    (("--trust-hooks",), True),
    (("--trust-hooks", "--no-trust-workspace"), False),
    (("--no-trust-workspace", "--trust-hooks"), True),
    (("--trust-workspace", "--no-trust-hooks"), False),
    (("--no-trust-hooks", "--trust-workspace"), True),
    (("--no-trust-hooks", "--no-trust-workspace"), False),
    (("--trust-hooks", "--trust-workspace"), True),
]

TERMINAL_DEADLINE_SECONDS = 60


def _publish_hook(unique_repo: str):
    return make_artifact(
        f"{unique_repo}/shell-guard",
        "hook",
        {"shell-guard/hook.toml": HOOK_TOML, "shell-guard/guard.sh": GUARD_SH},
        tag="1",
    )


def _rows(runner) -> list[dict]:
    """Every dispatch row across every root: the flat arming truth."""
    path = Path(runner.grim_home) / "hooks" / "dispatch.json"
    if not path.is_file():
        return []
    table = json.loads(path.read_text(encoding="utf-8"))
    return [row for root in table["roots"].values() for row in root["hooks"]]


def _applied(workspace: Path) -> str | None:
    """The setting a run wrote into ``workspace``'s Claude file, which hooks share."""
    target = workspace / LOCAL
    return json.loads(target.read_text(encoding="utf-8")).get("model") if target.exists() else None


def _consent_files(runner) -> list[Path]:
    """Every consent record on this machine."""
    return sorted((Path(runner.grim_home) / "hooks" / "consent").glob("*.json"))


def _seed_record(runner, workspace: Path, keys: list[str]) -> Path:
    """Write the record an earlier ``grim hook allow`` left for ``workspace``.

    Written by hand, never by the grim under test: the shape is the shipped
    on-disk contract, and a grim that wrote settings keys here could not show
    what an older one leaves.
    """
    key = hashlib.sha256(str(workspace).encode()).hexdigest()
    path = Path(runner.grim_home) / "hooks" / "consent" / f"{key}.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps({"v": 1, "workspace": str(workspace), "hooks": keys, "consented_at": "2026-08-28T10:00:00Z"}),
        encoding="utf-8",
        newline="\n",
    )
    return path


def _on_a_terminal(runner, *args: str, answer: str | None = None) -> tuple[int, str]:
    """Run grim with stdin and stderr on a pseudo-terminal.

    Returns the exit code and everything printed there. ``answer`` is typed
    once the question appears. With no answer, a question nobody expected hangs
    the child until the deadline, which fails the test.
    """
    master, slave = pty.openpty()
    child = subprocess.Popen(
        [str(runner.binary), *args],
        cwd=runner.cwd,
        env=runner.env,
        stdin=slave,
        stdout=subprocess.DEVNULL,
        stderr=slave,
    )
    os.close(slave)
    transcript = ""
    answered = answer is None
    deadline = time.monotonic() + TERMINAL_DEADLINE_SECONDS
    try:
        while True:
            remaining = deadline - time.monotonic()
            assert remaining > 0, f"grim never finished on its terminal:\n{transcript}"
            if not select.select([master], [], [], remaining)[0]:
                continue
            try:
                chunk = os.read(master, 4096)
            except OSError:  # Linux reports the closed far side as EIO.
                break
            if not chunk:
                break
            transcript += chunk.decode(errors="replace")
            if not answered and PROMPT in transcript:
                os.write(master, f"{answer}\n".encode())
                answered = True
    finally:
        os.close(master)
        if child.poll() is None:
            child.kill()
    return child.wait(timeout=TERMINAL_DEADLINE_SECONDS), transcript


def test_a_run_with_no_terminal_names_each_unconsented_package(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    team = make_settings(f"{unique_repo}/team", "1", DOCUMENT)
    other = make_settings(f"{unique_repo}/other", "1", {"description": "Other", "claude": {"language": "en"}})
    write_config(project_dir, settings={"team": team.fq, "other": other.fq}, experimental_settings=True)
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "s", runner.home)[0])
    runner.run("lock")

    result = runner.run("install", format="json")

    assert result.returncode == 0, result.stderr
    assert _applied(project_dir) is None
    for name in ("team", "other"):
        assert result.stderr.count(f"settings package '{name}' not applied: ") == 1, result.stderr
    assert "run `grim consent allow` or pass --trust-workspace" in result.stderr
    rows = {item["name"]: item["status"] for item in json.loads(result.stdout)["items"]}
    assert rows == {"team": "skipped", "other": "skipped"}

    # Positive control: the flag the warning names applies both, and names neither.
    trusted = runner.run("install", "--trust-workspace")
    assert _applied(project_dir) == "sonnet"
    assert "not applied" not in trusted.stderr, trusted.stderr


def test_record_without_settings_keys_re_prompts(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """Turning the settings flag on later drifts a hook-only record.

    The record stays good for the hook it names: a run that cannot ask leaves
    it armed and writes nothing. A run that can ask names the package, a decline
    changes no file, and accepting records both kinds once.
    """
    hook = _publish_hook(unique_repo)
    package = make_settings(f"{unique_repo}/team", "1", DOCUMENT)
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "sandbox", runner.home)[0])
    write_config(project_dir, hooks={"shell-guard": hook.fq}, settings={"team": package.fq})
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")

    hook_key = f"shell-guard@{REGISTRY_HOST}/{unique_repo}/shell-guard"
    settings_key = f"settings:team@{REGISTRY_HOST}/{unique_repo}/team"
    record = _seed_record(runner, project_dir, [hook_key])

    # Positive control: the seed is a real record, and flag off it covers all
    # that counts, so nothing is asked and the hook arms.
    code, shown = _on_a_terminal(runner, "install")
    assert code == 0, shown
    assert PROMPT not in shown
    assert len(_rows(runner)) == 1, shown

    runner.run("config", "set", "options.experimental.settings", "true")
    seeded = record.read_bytes()

    unattended = runner.run("install")
    assert unattended.returncode == 0, unattended.stderr
    assert PROMPT not in unattended.stderr
    assert record.read_bytes() == seeded, "a run that cannot ask records nothing"
    assert len(_rows(runner)) == 1, "the record still covers the hook"
    assert _applied(project_dir) is None, "a run that cannot ask writes no setting"

    code, shown = _on_a_terminal(runner, "install", answer="n")
    assert code == 0, shown
    assert PROMPT in shown, "the second flag turned on later is a drift"
    assert settings_key in shown, "the question names what changed"
    assert record.read_bytes() == seeded, "a decline changes no file"
    assert len(_rows(runner)) == 1, "a decline disarms nothing the record covers"
    assert _applied(project_dir) is None, "a decline writes no setting"

    code, shown = _on_a_terminal(runner, "install", answer="y")
    assert code == 0, shown
    assert PROMPT in shown
    assert set(json.loads(record.read_text(encoding="utf-8"))["hooks"]) == {hook_key, settings_key}
    assert len(_rows(runner)) == 1
    assert _applied(project_dir) == "sonnet", "an accepted answer applies"

    code, shown = _on_a_terminal(runner, "install")
    assert code == 0, shown
    assert PROMPT not in shown, "an accepted answer is not asked for again"


def test_prompt_names_settings(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """One question, worded for what needs the answer.

    A hooks-only workspace reads as it always did: ``test_hook_arming.py`` and
    ``test_hooks_boundary.py`` pin these fragments absent after commands that
    must not ask, which proves nothing unless the real question carries them.
    """
    hook = _publish_hook(unique_repo)
    package = make_settings(f"{unique_repo}/team", "1", DOCUMENT)
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "sandbox", runner.home)[0])
    write_config(project_dir, hooks={"shell-guard": hook.fq}, settings={"team": package.fq})
    runner.run("lock")
    hook_key = f"shell-guard@{REGISTRY_HOST}/{unique_repo}/shell-guard"
    settings_key = f"settings:team@{REGISTRY_HOST}/{unique_repo}/team"

    runner.run("config", "set", "options.experimental.hooks", "true")
    code, hooks_only = _on_a_terminal(runner, "install", answer="n")
    assert code == 0, hooks_only
    for fragment in (PROMPT, "are not consented yet", "Allow hooks from", HOOK_CLAUSE, OTHER_CHECKOUTS, DECLINING):
        assert fragment in hooks_only
    assert "pass --trust-hooks for one run" in hooks_only
    assert "grim hook allow" in hooks_only
    assert SETTINGS_CLAUSE not in hooks_only
    assert hooks_only.count(PROMPT) == 1

    runner.run("config", "unset", "options.experimental.hooks")
    runner.run("config", "set", "options.experimental.settings", "true")
    code, settings_only = _on_a_terminal(runner, "install", answer="n")
    assert code == 0, settings_only
    for fragment in (PROMPT, "Allow settings from", SETTINGS_CLAUSE, OTHER_CHECKOUTS, DECLINING):
        assert fragment in settings_only
    assert "pass --trust-workspace for one run" in settings_only
    assert "grim consent allow" in settings_only
    assert HOOK_CLAUSE not in settings_only
    assert "settings in " in settings_only and "were declined" in settings_only
    assert settings_only.count(PROMPT) == 1

    runner.run("config", "set", "options.experimental.hooks", "true")
    code, both = _on_a_terminal(runner, "install", answer="n")
    assert code == 0, both
    for fragment in (PROMPT, "Allow hooks and settings from", HOOK_CLAUSE, SETTINGS_CLAUSE, OTHER_CHECKOUTS, DECLINING):
        assert fragment in both
    assert both.count(PROMPT) == 1
    assert _consent_files(runner) == [], "a decline records nothing"
    assert _applied(project_dir) is None, "a decline writes no setting"

    code, both = _on_a_terminal(runner, "install", answer="y")
    assert code == 0, both
    [record] = _consent_files(runner)
    assert set(json.loads(record.read_text(encoding="utf-8"))["hooks"]) == {hook_key, settings_key}
    assert _applied(project_dir) == "sonnet", "the answer that records applies"

    # A drift names the kind that moved, and only that kind.
    _seed_record(runner, project_dir, [hook_key])
    code, drifted = _on_a_terminal(runner, "install", answer="n")
    assert code == 0, drifted
    assert "declares settings it was not consented for" in drifted
    assert f"new: {settings_key}" in drifted
    assert SETTINGS_CLAUSE in drifted
    assert HOOK_CLAUSE not in drifted


def test_an_unrecorded_answer_names_what_it_asked(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """An accepted answer whose record cannot be written arms and applies nothing.

    The warning names what the question asked about: a hooks-only run keeps the
    line it shipped with, and a run that asked about settings names workspace
    consent and the setting it left unapplied.
    """
    hook = _publish_hook(unique_repo)
    package = make_settings(f"{unique_repo}/team", "1", DOCUMENT)
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "sandbox", runner.home)[0])
    write_config(project_dir, hooks={"shell-guard": hook.fq}, settings={"team": package.fq})
    runner.run("lock")
    # A directory at the record's path: the write's rename onto it fails, and a read finds no record.
    key = hashlib.sha256(str(project_dir).encode()).hexdigest()
    blocker = Path(runner.grim_home) / "hooks" / "consent" / f"{key}.json"
    (blocker / "occupied").mkdir(parents=True)

    runner.run("config", "set", "options.experimental.settings", "true")
    code, settings_only = _on_a_terminal(runner, "install", answer="y")
    assert code == 0, settings_only
    assert "workspace consent could not be recorded; nothing applied" in settings_only
    assert "hook consent" not in settings_only
    assert _applied(project_dir) is None, "an unrecorded answer applies nothing"

    runner.run("config", "unset", "options.experimental.settings")
    runner.run("config", "set", "options.experimental.hooks", "true")
    code, hooks_only = _on_a_terminal(runner, "install", answer="y")
    assert code == 0, hooks_only
    assert "hook consent could not be recorded; nothing armed" in hooks_only
    assert _rows(runner) == [], "an unrecorded answer arms nothing"

    runner.run("config", "set", "options.experimental.settings", "true")
    code, both = _on_a_terminal(runner, "install", answer="y")
    assert code == 0, both
    assert "workspace consent could not be recorded; nothing armed or applied" in both
    assert _rows(runner) == []
    assert _applied(project_dir) is None

    # Positive control: with the path free, the same answer records, arms and applies.
    for entry in blocker.iterdir():
        entry.rmdir()
    blocker.rmdir()
    code, shown = _on_a_terminal(runner, "install", answer="y")
    assert code == 0, shown
    assert "could not be recorded" not in shown
    assert len(_rows(runner)) == 1, shown
    assert _applied(project_dir) == "sonnet", shown


def test_old_and_new_flags_are_one_answer(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """``--trust-workspace`` and ``--trust-hooks`` are one argument under two names.

    The last flag typed wins, whichever spelling it uses, on every command that
    carries the pair. The hook kind proves it, because a dispatch row is what
    an answer changes today. Each ``ANSWERS`` row flips what the row before it
    left, so no answer can pass by leaving the state alone.
    """
    hook = _publish_hook(unique_repo)
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")

    for command in ("install", "update", "add"):
        shown = runner.run(command, "--help").stdout
        for spelling in ("--trust-workspace", "--no-trust-workspace", "--trust-hooks", "--no-trust-hooks"):
            assert spelling in shown, f"`grim {command} --help` must name {spelling}"

    assert runner.run("install").returncode == 0
    assert _rows(runner) == [], "an unconsented workspace arms nothing unasked"
    runner.run("install", "--trust-workspace")
    assert len(_rows(runner)) == 1, "the new spelling arms an unconsented workspace"
    assert _consent_files(runner) == [], "and writes no record, as the old one does"

    runner.run("hook", "allow")
    runner.run("install")
    assert len(_rows(runner)) == 1, "the record alone arms, so a refusal below has something to refuse"
    for flags, armed in ANSWERS:
        runner.run("install", *flags)
        assert bool(_rows(runner)) is armed, f"install {flags}"

    runner.run("update", "--trust-hooks", "--no-trust-workspace")
    assert _rows(runner) == [], "update takes the last spelling typed"
    runner.run("update", "--no-trust-hooks", "--trust-workspace")
    assert len(_rows(runner)) == 1, "in both directions"

    def add_with(name: str, *flags: str) -> int:
        workspace = tmp_path / name
        (workspace / ".claude").mkdir(parents=True)
        adder = grim_at(workspace)
        adder.run("init")
        adder.run("config", "set", "options.experimental.hooks", "true")
        before = len(_rows(adder))
        adder.run("add", *flags, hook.fq)
        return len(_rows(adder)) - before

    assert add_with("refused", "--trust-hooks", "--no-trust-workspace") == 0, "add takes the last spelling typed"
    assert add_with("armed", "--no-trust-hooks", "--trust-workspace") == 1, "in both directions"


def _telemetry_command(result) -> dict:
    """The attributes of the one ``grim.command`` event a run built."""
    commands = [
        event["attributes"] for event in telemetry_payload(result.stderr)["events"] if event["name"] == "grim.command"
    ]
    assert len(commands) == 1, commands
    return commands[0]


def _top_level_commands(shown: str) -> list[str]:
    """The command names ``grim --help`` lists, without clap's own ``help``."""
    section = shown.split("Commands:\n", 1)[1].split("\n\n", 1)[0]
    return [line.split()[0] for line in section.splitlines() if line.split()[0] != "help"]


def _enable(runner, *flags: str, scope: tuple[str, ...] = ()) -> None:
    for flag in flags:
        runner.run("config", "set", f"options.experimental.{flag}", "true", *scope)


def test_consent_alias_reports_hook_telemetry_name(grim_at, project_dir: Path) -> None:
    """``grim consent`` is ``grim hook`` under a second name, not a new command.

    The top-level count stays and telemetry keeps reporting ``hook``, so a
    dashboard keyed on the name sees one command whichever word was typed.
    """
    write_config(project_dir)
    runner = grim_at(project_dir)
    shown = runner.run("--help").stdout
    hook_row = next(line for line in shown.splitlines() if line.startswith("  hook "))
    assert "[alias: consent]" in hook_row, shown
    commands = _top_level_commands(shown)
    assert len(commands) == 26, commands
    assert "consent" not in commands, "the alias is a label on `hook`, never a row of its own"

    runner.env["GRIM_LOG"] = "debug"
    runner.env["GRIM_TELEMETRY"] = "1"
    for words, subcommand in (
        (("hook", "allow"), "allow"),
        (("consent", "allow"), "allow"),
        (("consent", "revoke"), "revoke"),
        (("consent", "deny"), "revoke"),
    ):
        attributes = _telemetry_command(runner.run(*words))
        assert attributes["grim.command.name"] == "hook", words
        assert attributes["grim.command.subcommand"] == subcommand, words


@pytest.mark.parametrize(
    "flags",
    [(), ("hooks",), ("settings",), ("hooks", "settings")],
    ids=["flags-off", "hooks-on", "settings-on", "both-on"],
)
def test_allow_records_both_kinds(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str, flags: tuple[str, ...]
) -> None:
    """``allow`` is the review, so it records every declared key whatever the flags say.

    A flag turned on later then asks nothing about what was already reviewed.
    ``revoke`` withdraws the one record, so both kinds go together.
    """
    hook = _publish_hook(unique_repo)
    team = make_settings(f"{unique_repo}/team", "1", DOCUMENT)
    write_config(project_dir, hooks={"shell-guard": hook.fq}, settings={"team": team.fq})
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "sandbox", runner.home)[0])
    _enable(runner, *flags)
    runner.run("lock")
    _seed_record(runner, project_dir, [f"gone@{REGISTRY_HOST}/{unique_repo}/gone"])

    keys = sorted([f"shell-guard@{REGISTRY_HOST}/{hook.repo}", f"settings:team@{REGISTRY_HOST}/{team.repo}"])
    report = runner.json("consent", "allow")
    assert report["action"] == "consented"
    assert report["hooks"] == keys, "the report keeps its shape and carries the prefixed entries"
    [record] = _consent_files(runner)
    assert json.loads(record.read_text(encoding="utf-8"))["hooks"] == keys, "a dropped key does not linger"

    assert runner.json("consent", "revoke")["action"] == "revoked"
    assert _consent_files(runner) == [], "one record, so both kinds go"
    assert runner.json("consent", "revoke")["action"] == "not-consented"


def test_add_is_the_gesture(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """``grim add`` consents to the package it brought, and to nothing already there.

    A package a cloned repository declared stays unconsented when the user adds
    another one, a typed refusal is not consent, a flag that is off records
    nothing, and global scope is consented without a record.
    """
    planted = make_settings(f"{unique_repo}/planted", "1", DOCUMENT)
    team = make_settings(f"{unique_repo}/team", "1", DOCUMENT)
    second = make_settings(f"{unique_repo}/second", "1", DOCUMENT)
    write_config(project_dir, settings={"planted": planted.fq})
    runner = grim_at(project_dir)
    sandbox, roots = settings_sandbox(tmp_path / "sandbox", runner.home)
    runner.env.update(sandbox)
    _enable(runner, "settings")
    runner.run("lock")
    assert _consent_files(runner) == [], "lock is no gesture"

    runner.run("add", "--kind", "settings", "--no-trust-workspace", team.fq)
    assert _consent_files(runner) == [], "a typed refusal is never persisted"
    assert _applied(project_dir) is None, "a typed refusal writes no setting"

    runner.run("add", "--kind", "settings", second.fq)
    [record] = _consent_files(runner)
    assert json.loads(record.read_text(encoding="utf-8"))["hooks"] == [f"settings:second@{REGISTRY_HOST}/{second.repo}"]
    assert _applied(project_dir) == "sonnet", "the gesture applies what it brought"

    off_dir = tmp_path / "flag-off"
    off_dir.mkdir()
    off = grim_at(off_dir)
    off.env.update(sandbox)
    off.run("init")
    off.run("add", "--kind", "settings", team.fq)
    assert "team" in (off_dir / "grimoire.toml").read_text(encoding="utf-8"), "the flag gates writing, not declaring"
    assert _consent_files(off) == [record], "with the flag off nothing is consented"
    assert _applied(off_dir) is None, "with the flag off nothing is written"

    _enable(off, "settings", scope=("--global",))
    off.run("add", "--kind", "settings", second.fq, "--global")
    assert (Path(off.grim_home) / "grimoire.toml").read_text(encoding="utf-8").count(second.repo) == 1
    assert _consent_files(off) == [record], "the user's own toolchain carries no record"
    written = json.loads((roots["claude"] / "settings.json").read_text(encoding="utf-8"))
    assert written == {"model": "sonnet"}, "consented at global scope, so the add applies"


@pytest.mark.parametrize("carrier", ["bundle", "requires"])
def test_transitive_package_is_disclosed(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str, carrier: str
) -> None:
    """A package nobody typed is named at warn level, on the run that brings it and after.

    It inherits the gesture that reached it, so its key lands. With the flag
    off the feature is silent, which is the half hook output already pins.
    """
    team = make_settings(f"{unique_repo}/team", "1", DOCUMENT)
    if carrier == "bundle":
        parent = make_bundle(f"{unique_repo}/pack", [("settings", "team", team.fq)], tag="1")
        line = "settings package 'team' is delivered by bundle"
    else:
        parent = make_dependent(
            f"{unique_repo}/carrier",
            "1",
            "skill",
            {"carrier/SKILL.md": "---\nname: carrier\ndescription: d\n---\n# carrier\n"},
            [("settings", "team", "./team:1")],
        )
        line = "settings package 'team' is required by skill 'carrier'"

    write_config(project_dir)
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "sandbox", runner.home)[0])
    silent = runner.run("add", parent.fq).stderr
    assert line not in silent, silent
    assert _applied(project_dir) is None, "with the flag off nothing is written"

    workspace = tmp_path / "flag-on"
    (workspace / ".claude").mkdir(parents=True)
    armed = grim_at(workspace)
    armed.env.update(runner.env)
    write_config(workspace)
    _enable(armed, "settings")
    shown = armed.run("add", parent.fq).stderr
    assert line in shown, shown
    [record] = _consent_files(armed)
    assert json.loads(record.read_text(encoding="utf-8"))["hooks"] == [f"settings:team@{REGISTRY_HOST}/{team.repo}"]
    assert _applied(workspace) == "sonnet", "the gesture reaches what it pulled in"
    later = armed.run("install").stderr
    assert line in later, later


def test_hook_binding_mints_no_settings_key(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """A hook bound under the settings prefix consents to no package of that name.

    A repository picks the binding, and the name grammar runs after the key is
    minted, so the key itself refuses the prefix. The valid binding beside it
    proves the review records hook keys at all, and the later accepted answer
    proves the package was asked about because it was unconsented.
    """
    hook = _publish_hook(unique_repo)
    package = make_settings(hook.repo, "2", DOCUMENT)
    bindings = {"guard": hook.fq, '"settings:shell-guard"': hook.fq}
    write_config(project_dir, hooks=bindings)
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "sandbox", runner.home)[0])
    _enable(runner, "hooks")
    runner.run("lock")
    guard_key = f"guard@{REGISTRY_HOST}/{hook.repo}"
    settings_key = f"settings:shell-guard@{REGISTRY_HOST}/{hook.repo}"

    assert runner.json("hook", "allow")["hooks"] == [guard_key]
    [record] = _consent_files(runner)
    assert json.loads(record.read_text(encoding="utf-8"))["hooks"] == [guard_key]

    write_config(project_dir, hooks=bindings, settings={"shell-guard": package.fq})
    _enable(runner, "hooks", "settings")
    runner.run("lock")
    code, shown = _on_a_terminal(runner, "install", answer="n")
    assert code == 0, shown
    assert PROMPT in shown, "the package was never consented, whatever the hook's binding spells"
    assert f"new: {settings_key}" in shown
    assert _applied(project_dir) is None, "a decline writes no setting"

    code, shown = _on_a_terminal(runner, "install", answer="y")
    assert code == 0, shown
    assert PROMPT in shown
    assert set(json.loads(record.read_text(encoding="utf-8"))["hooks"]) == {guard_key, settings_key}
    assert _applied(project_dir) == "sonnet", "the accepted answer applies"


@pytest.mark.parametrize(
    "words",
    [("--name", "Team-Defaults"), ("--kind", "settings", "--name", "Team-Defaults"), ("--name", "team/defaults")],
    ids=["inferred-kind", "named-kind", "separator"],
)
def test_add_refuses_a_bad_binding(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str, words: tuple[str, ...]
) -> None:
    """A settings binding reaches a file path, so ``add`` refuses one outside the name grammar.

    The refusal leaves the declaration, the lock and the record as they were,
    and the same reference under a valid name goes through.
    """
    team = make_settings(f"{unique_repo}/team", "1", DOCUMENT)
    write_config(project_dir)
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "sandbox", runner.home)[0])
    _enable(runner, "settings")
    config = project_dir / "grimoire.toml"
    before = config.read_text(encoding="utf-8")

    refused = runner.run("add", *words, team.fq, check=False)
    assert refused.returncode == 65, refused.stderr
    assert "invalid settings binding name" in refused.stderr
    assert config.read_text(encoding="utf-8") == before
    assert not (project_dir / "grimoire.lock").exists()
    assert _consent_files(runner) == []
    assert _applied(project_dir) is None

    runner.run("add", "--name", "team-defaults", team.fq)
    assert "team-defaults" in config.read_text(encoding="utf-8")
    assert (project_dir / "grimoire.lock").is_file()
    assert len(_consent_files(runner)) == 1
    assert _applied(project_dir) == "sonnet"


@pytest.mark.parametrize("binding", ["Team-Defaults", '"team/defaults"'], ids=["upper", "separator"])
def test_install_refuses_a_hand_edited_binding(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str, binding: str
) -> None:
    """``add`` never saw a name typed into ``grimoire.toml``, and it reaches a file path.

    So the phase holds the same grammar, past consent, and the run writes
    nothing. The same reference under a valid name goes through.
    """
    team = make_settings(f"{unique_repo}/team", "1", DOCUMENT)
    write_config(project_dir, settings={binding: team.fq})
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "sandbox", runner.home)[0])
    runner.run("lock")
    _enable(runner, "settings")

    refused = runner.run("install", "--trust-workspace", check=False)

    assert refused.returncode == 65, refused.stderr
    assert "invalid settings binding name" in refused.stderr
    assert _applied(project_dir) is None
    assert list((Path(runner.grim_home) / "settings").glob("*.json")) == []

    write_config(project_dir, settings={"team-defaults": team.fq}, experimental_settings=True)
    runner.run("lock")
    runner.run("install", "--trust-workspace")
    assert _applied(project_dir) == "sonnet"
