# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Workspace consent for the experimental ``mod`` kind: what records an entry.

A mod's entry is the one record's unprefixed spelling, ``<binding>@<registry>/
<repository>``. ``grim add`` records it when the mods flag is on, and ``grim
consent allow`` records every declared mod whatever the flag says. Every
"nothing was recorded" assertion sits beside a run where the same thing is
recorded.

The disclosure and revoke lines have no wording in the decision record, so the
tests pin stable fragments only: the mod's binding name, the word ``mod``, the
source ``<registry>/<repository>``, and ``grim install`` on a revoke line.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from src.consent_driver import (
    PROMPT,
    old_record_path,
    on_a_terminal,
    seed_old_record,
    workspace_key,
)
from src.helpers import (
    make_artifact,
    make_bundle,
    make_hook,
    make_mod,
    mod_sandbox,
    write_config,
)
from src.registry import REGISTRY_HOST


def _records(runner) -> list[Path]:
    return sorted((Path(runner.grim_home) / "consent").glob("*.json"))


def _sources(record: Path) -> list[str]:
    return json.loads(record.read_text(encoding="utf-8"))["sources"]


def _enable_mods(runner) -> None:
    runner.run("config", "set", "options.experimental.mods", "true")


def test_add_records_the_mod_entry(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    other = make_mod(f"{unique_repo}/other-band", "1")
    write_config(project_dir)
    runner = grim_at(project_dir)
    runner.env.update(mod_sandbox(tmp_path / "sandbox")[0])
    _enable_mods(runner)

    runner.run("add", "--kind", "mod", "--no-trust-workspace", other.fq)
    assert _records(runner) == [], "a typed refusal is never persisted"

    runner.run("add", "--kind", "mod", mod.fq)
    [record] = _records(runner)
    assert _sources(record) == [f"status-band@{REGISTRY_HOST}/{mod.repo}"]

    # Positive control for the flag-off half: the same add records the entry above,
    # so an empty directory below is the flag's doing and not a stale binary's.
    off_dir = tmp_path / "flag-off"
    off_dir.mkdir()
    off = grim_at(off_dir)
    off.env.update(mod_sandbox(tmp_path / "sandbox-off")[0])
    write_config(off_dir)
    (off_dir / ".claude").mkdir()
    off.run("add", "--kind", "mod", other.fq)
    assert "other-band" in (off_dir / "grimoire.toml").read_text(encoding="utf-8"), (
        "the flag gates writing, not declaring"
    )
    assert _records(off) == [record], "with the flag off nothing new is recorded"


def test_allow_records_mods_with_the_flag_off(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    write_config(project_dir, mods={"status-band": mod.fq}, experimental_mods=False)
    runner = grim_at(project_dir)
    runner.env.update(mod_sandbox(tmp_path / "sandbox")[0])
    runner.run("lock")
    assert _records(runner) == [], "lock is no gesture"

    key = f"status-band@{REGISTRY_HOST}/{mod.repo}"
    report = runner.json("consent", "allow")
    assert report["action"] == "consented"
    assert report["hooks"] == [key]
    [record] = _records(runner)
    assert _sources(record) == [key], "recorded with the mods flag off"

    # Positive control: the record is read back as this workspace's one answer, and
    # a revoke removes it, so the file above was a live record and not a leftover.
    assert runner.json("consent", "revoke")["action"] == "revoked"
    assert _records(runner) == []


SKILL = "---\nname: control\ndescription: d\n---\n# control\n"
BAND = Path(".claude/mods/status-band/src/index.js")


def _declare(project_dir: Path, repo: str, mods=None, bundles=None, hooks=None) -> None:
    skill = make_artifact(f"{repo}/control", "skill", {"control/SKILL.md": SKILL}, "1")
    write_config(
        project_dir,
        skills={"control": skill.fq},
        mods=mods,
        bundles=bundles,
        hooks=hooks,
        experimental_mods=True,
    )


def _project(project_dir: Path, tmp_path: Path, repo: str, grim_at, **decl):
    _declare(project_dir, repo, **decl)
    runner = grim_at(project_dir)
    runner.env.update(mod_sandbox(tmp_path / "sandbox")[0])
    runner.run("lock")
    return runner


def _tree(root: Path) -> dict[str, bytes]:
    return {
        str(p.relative_to(root)): p.read_bytes()
        for p in sorted(root.rglob("*"))
        if p.is_file()
    }


def _disclosed(stderr: str, name: str, source: str) -> list[str]:
    """Stderr lines naming a mod, its kind word and its source."""
    return [
        line
        for line in stderr.splitlines()
        if name in line and source in line and "mod" in line
    ]


def test_revoke_names_written_mods(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mods = {
        n: make_mod(f"{unique_repo}/{n}", "1") for n in ("status-band", "other-band")
    }
    runner = _project(
        project_dir,
        tmp_path,
        unique_repo,
        grim_at,
        mods={n: m.fq for n, m in mods.items()},
    )
    runner.run("consent", "allow")
    runner.run("install")
    assert (project_dir / BAND).is_file(), "positive control: the mod was written"
    before = _tree(project_dir / ".claude") | _tree(project_dir / ".opencode")

    result = runner.run("consent", "revoke")

    for name in mods:
        assert [
            line
            for line in result.stderr.splitlines()
            if name in line and "grim install" in line
        ], f"no revoke line names {name} and `grim install`:\n{result.stderr}"
    after = _tree(project_dir / ".claude") | _tree(project_dir / ".opencode")
    assert after == before, "revoke itself changes no file"


def test_first_write_under_an_existing_entry_is_disclosed(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    other = make_mod(f"{unique_repo}/other-band", "1")
    runner = _project(
        project_dir, tmp_path, unique_repo, grim_at, mods={"status-band": mod.fq}
    )
    runner.run("consent", "allow")
    assert not (project_dir / BAND).exists(), "consent alone writes nothing"

    first = runner.run("install")
    assert (project_dir / BAND).is_file()
    source = f"{REGISTRY_HOST}/{mod.repo}"
    assert len(_disclosed(first.stderr, "status-band", source)) == 1, first.stderr

    again = runner.run("install")
    assert not _disclosed(again.stderr, "status-band", source), "only the first write"

    # The run whose own gesture adds the entry prints no line for it: `grim add`
    # records the entry and writes the mod in the same run.
    gesture = runner.run("add", "--kind", "mod", other.fq)
    [record] = _records(runner)
    assert f"other-band@{REGISTRY_HOST}/{other.repo}" in _sources(record)
    assert (project_dir / ".claude/mods/other-band/src/index.js").is_file()
    assert not _disclosed(
        gesture.stderr, "other-band", f"{REGISTRY_HOST}/{other.repo}"
    ), gesture.stderr


def test_add_under_an_entry_recorded_before_the_run_is_disclosed(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(
        project_dir, tmp_path, unique_repo, grim_at, mods={"status-band": mod.fq}
    )
    runner.run("consent", "allow")
    [record] = _records(runner)
    source = f"{REGISTRY_HOST}/{mod.repo}"
    assert f"status-band@{source}" in _sources(record), "the entry predates the add"
    assert not (project_dir / BAND).exists(), "consent alone writes nothing"

    result = runner.run("add", "--kind", "mod", mod.fq)

    assert (project_dir / BAND).is_file(), "positive control: the add wrote the mod"
    assert len(_disclosed(result.stderr, "status-band", source)) == 1, result.stderr


def test_bundle_member_and_requires_edge_are_disclosed(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    member = make_mod(f"{unique_repo}/member-band", "1")
    dep = make_mod(f"{unique_repo}/dep-band", "1")
    top = make_mod(
        f"{unique_repo}/top-band", "1", requires=[("mod", "dep-band", dep.fq)]
    )
    direct = {"member-band": member.fq, "dep-band": dep.fq, "top-band": top.fq}
    runner = _project(project_dir, tmp_path, unique_repo, grim_at, mods=direct)
    runner.run("consent", "allow")

    pack = make_bundle(f"{unique_repo}/pack", [("mod", "member-band", member.fq)])
    _declare(
        project_dir, unique_repo, mods={"top-band": top.fq}, bundles={"pack": pack.fq}
    )
    runner.run("lock")
    result = runner.run("install")

    for mod, why in ((member, "bundle member"), (dep, "requires edge")):
        name = mod.repo.rsplit("/", 1)[-1]
        assert (project_dir / f".claude/mods/{name}/src/index.js").is_file(), why
        assert _disclosed(result.stderr, name, f"{REGISTRY_HOST}/{mod.repo}"), (
            f"{why}: no disclosure line\n{result.stderr}"
        )


def test_nothing_written_before_consent(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(
        project_dir, tmp_path, unique_repo, grim_at, mods={"status-band": mod.fq}
    )
    runner.run("install", check=False)
    assert not (project_dir / ".claude/mods").exists()
    assert not (project_dir / ".opencode/plugins").exists()
    assert (project_dir / ".claude/skills/control/SKILL.md").is_file(), (
        "the run reached its writers: the control skill was written"
    )

    runner.run("install", "--trust-workspace")
    assert (project_dir / BAND).is_file(), "positive control: the flag writes"


def test_trust_workspace_writes_with_no_record(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(
        project_dir, tmp_path, unique_repo, grim_at, mods={"status-band": mod.fq}
    )
    assert _records(runner) == []
    runner.run("install", "--trust-workspace")
    assert (project_dir / BAND).is_file()


def test_no_trust_workspace_disarms_for_the_run(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(
        project_dir, tmp_path, unique_repo, grim_at, mods={"status-band": mod.fq}
    )
    runner.run("install", "--trust-workspace")
    runner.run("consent", "allow")
    assert (project_dir / BAND).is_file(), "positive control: written and consented"

    runner.run("install", "--no-trust-workspace", check=False)
    assert not (project_dir / ".claude/mods/status-band").exists()

    runner.run("install")
    assert (project_dir / BAND).is_file(), "the next consented run writes it again"


def test_same_binding_across_kinds_needs_no_new_answer(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    repo = f"{unique_repo}/shared"
    hook = make_hook(repo, "b", "1")
    mod = make_mod(repo, "2")
    runner = _project(project_dir, tmp_path, unique_repo, grim_at, hooks={"b": hook.fq})
    runner.run("consent", "allow")
    [record] = _records(runner)
    key = f"b@{REGISTRY_HOST}/{repo}"
    assert _sources(record) == [key], "positive control: the hook's entry is recorded"

    _declare(project_dir, unique_repo, mods={"b": mod.fq})
    runner.run("lock")
    result = runner.run("install")  # exit 0 with stdin closed: no prompt

    assert (project_dir / ".claude/mods/b/src/index.js").is_file()
    assert _disclosed(result.stderr, "b", f"{REGISTRY_HOST}/{repo}"), result.stderr
    assert _sources(record) == [key], "no new entry was recorded"


needs_terminal = pytest.mark.skipif(
    os.name == "nt", reason="the consent question needs a POSIX pseudo-terminal"
)
OLD_KEY = f"legacy@{REGISTRY_HOST}/some/legacy"


def _status_rows(runner) -> dict[str, dict]:
    items = runner.json("status")["items"]
    return {i["name"]: i for i in items if i["kind"] == "mod"}


def test_add_deletes_an_older_record_and_records_the_mod(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    write_config(project_dir)
    runner = grim_at(project_dir)
    runner.env.update(mod_sandbox(tmp_path / "sandbox")[0])
    _enable_mods(runner)
    old = old_record_path(runner, project_dir)
    seed_old_record(old, project_dir, [OLD_KEY])

    runner.run("add", "--kind", "mod", "--trust-workspace", mod.fq)

    assert (project_dir / BAND).is_file(), "positive control: the add wrote the mod"
    assert not old.exists(), "the older record is deleted by the write"
    [record] = _records(runner)
    assert _sources(record) == [f"status-band@{REGISTRY_HOST}/{mod.repo}"]


@needs_terminal
def test_install_asks_about_a_declared_mod_and_the_answer_decides(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(
        project_dir, tmp_path, unique_repo, grim_at, mods={"status-band": mod.fq}
    )
    old = old_record_path(runner, project_dir)
    seed_old_record(old, project_dir, [OLD_KEY])
    key = f"status-band@{REGISTRY_HOST}/{mod.repo}"

    code, declined = on_a_terminal(runner, "install", answer="n")
    assert code == 0, declined
    assert PROMPT in declined
    assert "Mods declared by" in declined and "Allow mods from" in declined, declined
    assert "A mod changes your AI client's own interface" in declined
    assert "are not consented yet" in declined, "an older record counts as no record"
    assert "Declared here:" not in declined, "one kind needs no per-kind list"
    assert not (project_dir / ".claude/mods/status-band").exists()
    assert _records(runner) == [], "a decline records nothing"
    assert old.exists(), "a decline leaves the older record alone"

    code, accepted = on_a_terminal(runner, "install", answer="y")
    assert code == 0, accepted
    assert (project_dir / BAND).is_file()
    [record] = _records(runner)
    assert _sources(record) == [key]
    assert not old.exists(), "a recorded answer deletes the older record"


@needs_terminal
def test_question_for_a_hook_and_a_mod_names_both_and_lists_each(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    hook = make_hook(f"{unique_repo}/shell-guard", "shell-guard", "1")
    runner = _project(
        project_dir,
        tmp_path,
        unique_repo,
        grim_at,
        mods={"status-band": mod.fq},
        hooks={"shell-guard": hook.fq},
    )
    runner.run("config", "set", "options.experimental.hooks", "true")
    mod_key = f"status-band@{REGISTRY_HOST}/{mod.repo}"
    hook_key = f"shell-guard@{REGISTRY_HOST}/{hook.repo}"

    code, shown = on_a_terminal(runner, "install", answer="n")

    assert code == 0, shown
    assert "Allow hooks and mods from" in shown, shown
    assert "Declared here:" in shown
    assert f"  mod: {mod_key}" in shown and f"  hook: {hook_key}" in shown
    assert _records(runner) == []


def test_consent_for_one_mod_does_not_cover_another_source(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    ok = make_mod(f"{unique_repo}/status-band", "1")
    other = make_mod(f"{unique_repo}/other-band", "1")
    runner = _project(
        project_dir,
        tmp_path,
        unique_repo,
        grim_at,
        mods={"status-band": ok.fq, "other-band": other.fq},
    )
    record = Path(runner.grim_home) / "consent" / f"{workspace_key(project_dir)}.json"
    record.parent.mkdir(parents=True, exist_ok=True)
    record.write_text(
        json.dumps(
            {
                "v": 2,
                "workspace": str(project_dir),
                "sources": [f"status-band@{REGISTRY_HOST}/{ok.repo}"],
                "consented_at": "2026-08-28T10:00:00Z",
            }
        ),
        encoding="utf-8",
        newline="\n",
    )

    runner.run("install")

    assert (project_dir / BAND).is_file(), "the consented mod is written"
    assert not (project_dir / ".claude/mods/other-band").exists()
    rows = _status_rows(runner)
    assert rows["status-band"]["state"] == "installed"
    assert rows["other-band"]["state"] == "gated", rows["other-band"]
