# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A skill whose root carries a Claude plugin manifest directory is refused.

``grim build``, ``grim release`` and ``grim publish`` exit 65 with reason
``skill-plugin-manifest`` and push nothing. An entry the ignore set excludes
is not shipped, so it does not refuse; a manifest name below the root is not
refused either.
"""

from __future__ import annotations

import json
import shutil
from pathlib import Path
from urllib.error import HTTPError

from src.helpers import make_artifact, write_config
from src.registry import tag_digest

REASON = "skill-plugin-manifest"
UPGRADING = "https://grimoire.rs/upgrading.html#skill-plugin-manifest"


def _write(p: Path, body: str) -> None:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(body, encoding="utf-8", newline="\n")


def _skill(root: Path, name: str, *, manifest: str | None = None) -> Path:
    d = root / "skills" / name
    _write(
        d / "SKILL.md",
        f"---\nname: {name}\ndescription: A test skill.\n---\n# {name}\n",
    )
    if manifest is not None:
        _write(d / manifest / "plugin.json", "{}\n")
    return d


def _tag_exists(repo_path: str, tag: str) -> bool:
    try:
        tag_digest(repo_path, tag)
    except HTTPError as exc:
        assert exc.code == 404, exc
        return False
    return True


def _error(result) -> dict:
    return json.loads(result.stdout)["error"]


def test_build_and_release_refuse(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    runner = grim_at(project_dir)
    refused = _skill(project_dir, "refused", manifest=".Claude-Plugin")
    clean = _skill(project_dir, "clean")
    repo = f"{unique_repo}/refused"

    built = runner.run("build", str(refused), check=False)
    assert built.returncode == 65, built.stderr
    assert "since grim 0.17.0" in built.stderr and ".Claude-Plugin" in built.stderr, (
        built.stderr
    )
    assert UPGRADING in built.stderr, built.stderr
    doc = _error(runner.run("build", str(refused), format="json", check=False))
    assert (doc["code"], doc["exit"], doc["reason"]) == ("data", 65, REASON), doc

    released = runner.run(
        "release", str(refused), f"{registry}/{repo}:1.0.0", format="json", check=False
    )
    assert released.returncode == 65, released.stderr
    assert _error(released)["reason"] == REASON
    assert not _tag_exists(repo, "1.0.0"), "nothing is pushed"

    # Positive control: the same commands accept a skill without the entry.
    assert runner.run("build", str(clean), check=False).returncode == 0
    ok = runner.run(
        "release", str(clean), f"{registry}/{unique_repo}/clean:1.0.0", check=False
    )
    assert ok.returncode == 0, ok.stderr
    assert _tag_exists(f"{unique_repo}/clean", "1.0.0")


def _publish_manifest(project_dir: Path, registry: str, names: list[str]) -> Path:
    lines = [f'registry = "{registry}"', ""]
    for n in names:
        lines += [f"[skills.{n}]", 'version = "0.1.0"', ""]
    path = project_dir / "publish.toml"
    path.write_text("\n".join(lines), encoding="utf-8", newline="\n")
    return path


def _published(name: str) -> bool:
    return _tag_exists(f"skills/{name}", "0.1.0")


def test_publish_refuses_with_zero_pushes(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    runner = grim_at(project_dir)
    prefix = unique_repo.split("/")[-1]
    clean = f"{prefix}-m-clean"
    _skill(project_dir, clean)
    # The refused skill sorts before and after the clean one in the batch.
    refused = [f"{prefix}-a-refused", f"{prefix}-z-refused"]
    for name in refused:
        _skill(project_dir, name, manifest=".claude-plugin")
        manifest = _publish_manifest(project_dir, registry, [clean, name])
        for extra in ([], ["--dry-run"]):
            result = runner.run(
                "publish",
                "--manifest",
                str(manifest),
                *extra,
                format="json",
                check=False,
            )
            assert result.returncode == 65, result.stderr
            assert _error(result)["reason"] == REASON
        assert not _published(clean) and not _published(name), "zero pushes"

    # Positive control: with the entries removed the same batch publishes.
    for name in refused:
        shutil.rmtree(project_dir / "skills" / name / ".claude-plugin")
    manifest = _publish_manifest(project_dir, registry, [refused[0], clean, refused[1]])
    ok = runner.run("publish", "--manifest", str(manifest), format="json", check=False)
    assert ok.returncode == 0, ok.stderr
    assert {r["status"] for r in json.loads(ok.stdout)["items"]} == {"pushed"}
    assert _published(clean)


def test_grimignore_entry_publishes_plain_skill(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    runner = grim_at(project_dir)
    skill = _skill(project_dir, "ignored", manifest=".claude-plugin")
    _write(skill / ".grimignore", ".claude-plugin\n")
    out = runner.run(
        "release", str(skill), f"{registry}/{unique_repo}/ignored:1.0.0", check=False
    )
    assert out.returncode == 0, out.stderr
    assert _tag_exists(f"{unique_repo}/ignored", "1.0.0")


def test_nested_manifest_skill_builds_and_installs(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    runner = grim_at(project_dir)
    skill = _skill(project_dir, "nested")
    _write(skill / "templates/.claude-plugin/plugin.json", "{}\n")
    repo = f"{registry}/{unique_repo}/nested"
    assert runner.run("build", str(skill), check=False).returncode == 0
    rel = runner.run("release", str(skill), f"{repo}:1.0.0", check=False)
    assert rel.returncode == 0, rel.stderr

    consumer = project_dir / "consumer"
    (consumer / ".claude").mkdir(parents=True)
    write_config(consumer, skills={"nested": f"{repo}:1.0.0"})
    c = grim_at(consumer)
    c.run("lock", check=False)
    rows = c.json("install")["items"]
    assert {r["status"] for r in rows} == {"installed"}
    assert (
        consumer / ".claude/skills/nested/templates/.claude-plugin/plugin.json"
    ).is_file()


def _offline(runner):
    runner.env["GRIM_OFFLINE"] = "1"
    return runner


def test_path_source_refused_and_status_outdated(grim_at, project_dir: Path) -> None:
    _skill(project_dir, "p")
    _write(project_dir / "grimoire.toml", '[skills]\np = "./skills/p"\n')
    runner = _offline(grim_at(project_dir))
    runner.run("lock")
    runner.run("install", "--client", "claude")
    installed = project_dir / ".claude/skills/p/SKILL.md"
    assert installed.is_file(), "positive control: the clean skill installed"

    # The skill gains the entry after it was locked and installed.
    _write(project_dir / "skills/p/.claude-plugin/plugin.json", "{}\n")
    status = runner.run("status", format="json", check=False)
    assert status.returncode == 0, status.stderr
    assert [(i["name"], i["state"]) for i in json.loads(status.stdout)["items"]] == [
        ("p", "outdated")
    ]
    assert "since grim 0.17.0" in status.stderr, status.stderr

    # Dev install of the refused directory, from a project that declares nothing.
    dev = project_dir / "dev"
    (dev / ".claude").mkdir(parents=True)
    _write(dev / "grimoire.toml", "[skills]\n")
    dev_out = _offline(grim_at(dev)).run(
        "install", "../skills/p", "--client", "claude", format="json", check=False
    )
    assert dev_out.returncode == 65, dev_out.stderr
    assert _error(dev_out)["reason"] == REASON
    assert not (dev / ".claude/skills/p").exists()

    # With the outputs gone, a fresh install of the drifted lock row re-packs the source.
    shutil.rmtree(project_dir / ".claude/skills/p")
    assert not (project_dir / ".claude/skills/p").exists()
    for args in (
        ("lock",),
        ("update",),
        ("install", "--client", "claude"),
    ):
        out = runner.run(*args, format="json", check=False)
        assert out.returncode == 65, (args, out.returncode, out.stderr)
        assert _error(out)["reason"] == REASON, (args, out.stdout)
    assert not (project_dir / ".claude/skills/p").exists()

    # `add <path>` of a second refused skill fails the same way.
    _skill(project_dir, "q", manifest=".claude-plugin")
    added = runner.run("add", "./skills/q", "--no-install", format="json", check=False)
    assert added.returncode == 65, added.stderr
    assert _error(added)["reason"] == REASON
    assert "[skills.q]" not in (project_dir / "grimoire.toml").read_text(
        encoding="utf-8"
    ) and 'q = "' not in (project_dir / "grimoire.toml").read_text(encoding="utf-8")


def _published_skill(repo: str, name: str, *, manifest: str | None = None) -> None:
    files = {
        f"{name}/SKILL.md": f"---\nname: {name}\ndescription: A test skill.\n---\n# {name}\n"
    }
    if manifest is not None:
        files[f"{name}/{manifest}/plugin.json"] = "{}\n"
    make_artifact(repo, "skill", files, tag="1.0.0")


def test_install_refuses_published_skill(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # A tar pushed raw: `grim release` itself would refuse to pack it.
    _published_skill(f"{unique_repo}/bad", "bad", manifest=".Claude-Plugin")
    _published_skill(f"{unique_repo}/good", "good")
    bad_ref = f"{registry}/{unique_repo}/bad:1.0.0"
    good_ref = f"{registry}/{unique_repo}/good:1.0.0"

    runner = grim_at(project_dir)
    write_config(project_dir, skills={"bad": bad_ref, "good": good_ref})
    runner.run("lock")

    for extra in ([], ["--force"]):
        out = runner.run(
            "install", "--client", "claude", *extra, format="json", check=False
        )
        assert out.returncode == 65, (extra, out.stderr)
        assert "grim remove skill bad" in out.stderr, out.stderr
    skills = project_dir / ".claude/skills"
    assert (skills / "good/SKILL.md").is_file(), "the clean row still installs"
    assert not (skills / "bad").exists(), "nothing written for the refused row"

    err = _error(
        runner.run("install", "--client", "claude", format="json", check=False)
    )
    assert (err["code"], err["exit"], err["reason"]) == ("data", 65, REASON), err
    assert "retryable" not in err and "forceable" not in err, err

    # `grim add` keeps the declaration and the lock row; only a failed relock rolls back.
    fresh = project_dir / "fresh"
    (fresh / ".claude").mkdir(parents=True)
    write_config(fresh, skills={})
    added = grim_at(fresh).run("add", bad_ref, format="json", check=False)
    assert added.returncode == 65, added.stderr
    assert _error(added)["reason"] == REASON
    assert "bad" in (fresh / "grimoire.toml").read_text(encoding="utf-8")
    assert "bad" in (fresh / "grimoire.lock").read_text(encoding="utf-8")
    assert not (fresh / ".claude/skills/bad").exists()


def test_export_refuses_member(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    _published_skill(f"{unique_repo}/bad", "bad", manifest=".claude-plugin")
    _published_skill(f"{unique_repo}/good", "good")
    bad_ref = f"{registry}/{unique_repo}/bad:1.0.0"
    good_ref = f"{registry}/{unique_repo}/good:1.0.0"
    runner = grim_at(project_dir)

    # Positive control: the clean member exports, so a refusal below is the member's.
    out = project_dir / "good-out"
    ok = runner.run(
        "export", "plugin", good_ref, "--name", "p", "--client", "claude",
        "-o", str(out), format="json", check=False,
    )  # fmt: skip
    assert ok.returncode == 0, ok.stderr
    assert out.exists()

    refused = project_dir / "bad-out"
    result = runner.run(
        "export", "plugin", good_ref, bad_ref, "--name", "p", "--client", "claude",
        "-o", str(refused), format="json", check=False,
    )  # fmt: skip
    assert result.returncode == 65, result.stderr
    assert _error(result)["reason"] == REASON
    assert not refused.exists(), "no output path is created"

    (project_dir / "marketplace.toml").write_text(
        '[marketplace]\nname = "acme"\nclients = ["claude"]\n\n'
        '[marketplace.owner]\nname = "Acme"\n\n'
        f'[plugins.one]\ninclude = ["{bad_ref}"]\n',
        encoding="utf-8",
        newline="\n",
    )
    market = runner.run("export", "marketplace", format="json", check=False)
    assert market.returncode == 65, market.stderr
    assert _error(market)["reason"] == REASON
    assert not (project_dir / "claude").exists(), "no tree is created"


def test_installed_dev_skill_with_entry_refuses_install_and_update(
    grim_at, project_dir: Path
) -> None:
    # The state an older grim leaves: a dev install whose source and output both
    # carry the entry. Only the files are seeded; the records come from a real run.
    skill = _skill(project_dir, "devd")
    _write(project_dir / "grimoire.toml", "[skills]\n")
    (project_dir / ".claude").mkdir(parents=True, exist_ok=True)
    runner = _offline(grim_at(project_dir))
    runner.run("lock")
    runner.run("install", "./skills/devd", "--client", "claude")
    out_dir = project_dir / ".claude/skills/devd"
    assert (out_dir / "SKILL.md").is_file(), "positive control: the dev install ran"
    assert runner.run("install", "--client", "claude", check=False).returncode == 0

    for d in (skill, out_dir):
        _write(d / ".claude-plugin/plugin.json", "{}\n")

    status = runner.run("status", format="json", check=False)
    assert status.returncode == 0, status.stderr
    assert "no longer installs" in status.stderr, status.stderr

    for args in (("install", "--client", "claude"), ("update",)):
        out = runner.run(*args, format="json", check=False)
        assert out.returncode == 65, (args, out.returncode, out.stderr)
        assert _error(out)["reason"] == REASON, (args, out.stdout)
        assert "grim uninstall skill devd" in out.stderr, out.stderr
    assert (out_dir / ".claude-plugin/plugin.json").is_file(), "files are kept"

    removed = runner.run("uninstall", "skill", "devd", check=False)
    assert removed.returncode == 0, removed.stderr
    assert not out_dir.exists()
    assert runner.run("install", "--client", "claude", check=False).returncode == 0
