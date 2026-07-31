# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim lock` acceptance tests."""
from __future__ import annotations

from pathlib import Path

from src.helpers import make_artifact, write_config


def test_lock_writes_sha256_pins(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\n---\n"},
        tag="stable",
    )
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# rust\n"},
        tag="v1",
    )
    write_config(
        project_dir,
        skills={"code-review": sk.fq},
        rules={"rust-style": ru.fq},
    )
    runner = grim_at(project_dir)

    rows = runner.json("lock")["items"]
    assert {r["name"] for r in rows} == {"code-review", "rust-style"}
    assert all(r["action"] == "locked" for r in rows)

    lock_text = (project_dir / "grimoire.lock").read_text()
    assert "@sha256:" in lock_text
    assert sk.digest in lock_text
    assert ru.digest in lock_text


def test_relock_is_byte_identical_and_preserves_generated_at(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    sk = make_artifact(
        f"{unique_repo}/s",
        "skill",
        {"s/SKILL.md": "---\nname: s\n---\n"},
        tag="stable",
    )
    write_config(project_dir, skills={"s": sk.fq})
    runner = grim_at(project_dir)

    runner.run("lock", check=False)
    first = (project_dir / "grimoire.lock").read_bytes()
    rows = runner.json("lock")["items"]
    second = (project_dir / "grimoire.lock").read_bytes()

    assert first == second, "a no-op relock must be byte-identical"
    assert all(r["action"] == "unchanged" for r in rows)


def test_lock_past_64_kib_stays_readable(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A lock larger than the old 64 KiB cap must still be readable.

    grim writes the lock itself: a bundle-heavy setup with deep registry
    paths crosses 64 KiB around ~140 artifacts, and the read-side cap then
    rejected a file grim had just generated — `status`, `install`, `update`
    and `lock` all exited 78, including the commands that would undo it.
    """
    sk = make_artifact(
        f"{unique_repo}/big",
        "skill",
        {"big/SKILL.md": "---\nname: big\n---\n"},
        tag="stable",
    )
    write_config(project_dir, skills={"big": sk.fq})
    runner = grim_at(project_dir)
    runner.run("lock")

    # TOML comments: parse-inert, so only the file size changes.
    lock_path = project_dir / "grimoire.lock"
    lock_path.write_text(lock_path.read_text() + "# pad pad pad pad pad\n" * 5000)
    assert lock_path.stat().st_size > 64 * 1024, "precondition: past the old cap"

    result = runner.run("status", check=False)
    assert result.returncode == 0, (
        f"an oversized-by-old-limits lock must still read, got "
        f"{result.returncode}; {result.stderr}"
    )


def test_tag_not_found_exits_79(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    write_config(
        project_dir,
        skills={"missing": f"{registry}/{unique_repo}/nope:absent"},
    )
    runner = grim_at(project_dir)
    result = runner.run("lock", check=False)
    assert result.returncode == 79, (
        f"unknown tag must exit 79, got {result.returncode}; "
        f"{result.stderr}"
    )


def test_bad_config_exits_78(grim_at, project_dir: Path) -> None:
    # `surprise` is an unknown top-level field ⇒ TOML parse / schema error.
    (project_dir / "grimoire.toml").write_text(
        'surprise = true\n[skills]\n'
    )
    runner = grim_at(project_dir)
    result = runner.run("lock", check=False)
    assert result.returncode == 78, (
        f"bad config must exit 78, got {result.returncode}; "
        f"{result.stderr}"
    )
