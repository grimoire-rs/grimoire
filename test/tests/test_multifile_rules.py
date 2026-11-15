# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Multi-file rule acceptance tests — index `.md` + sibling support dir.

Since grim 0.16.0 a rule is the `.md` only: `grim build`, `grim release` and
`grim publish` refuse a rule whose sibling directory holds anything beyond
`README.md`, `logo.png` and `logo.svg`, and never pack those three. Rules
published before that still carry a support directory (`<name>.md` plus
`<name>/<rel>` in the layer); they install as before, with one deprecation
warning. The consume-side tests push such an old-format artifact with
`make_artifact`, because `grim release` can no longer produce one.
"""
from __future__ import annotations

import json
from pathlib import Path
from urllib.error import HTTPError

from src.helpers import make_artifact, write_config
from src.registry import tag_digest

REFUSAL = "a rule can no longer ship a support directory (since grim 0.16.0)"
UPGRADING = "https://grimoire.rs/upgrading.html#rule-support-dir"
DEPRECATION = "ships a support directory, which is deprecated"


def _write(p: Path, body: str) -> None:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(body, encoding="utf-8", newline="\n")


def _old_format_rule(repo: str, name: str = "my-rule", tag: str = "1.0.0"):
    """Push a rule the way grim < 0.16.0 packed one: index plus support tree."""
    return make_artifact(
        repo,
        "rule",
        {
            f"{name}.md": (
                "---\npaths: ['**/*.rs']\n---\n"
                f"# {name}\nSee [examples](./{name}/examples.md) and the schema.\n"
            ),
            f"{name}/examples.md": "# Examples\nworked example\n",
            f"{name}/schema.json": '{"version": 1}\n',
        },
        tag=tag,
    )


def _local_rule_with_support_file(project_dir: Path, name: str = "my-rule") -> Path:
    """A local rule whose directory holds a file a rule may no longer ship."""
    index = project_dir / "rules" / f"{name}.md"
    _write(index, f"---\npaths: ['**/*.rs']\n---\n# {name}\n")
    _write(project_dir / "rules" / name / "examples.md", "# Examples\n")
    return index


def _tag_exists(repo_path: str, tag: str) -> bool:
    try:
        tag_digest(repo_path, tag)
    except HTTPError as exc:
        assert exc.code == 404, exc
        return False
    return True


def _assert_refused(result) -> None:
    assert result.returncode == 65, f"got {result.returncode}; stderr: {result.stderr}"
    assert REFUSAL in result.stderr, result.stderr
    found = [ln.strip() for ln in result.stderr.splitlines() if ln.strip().startswith("found in")]
    assert len(found) == 1 and found[0].endswith(": examples.md"), result.stderr
    assert UPGRADING in result.stderr, result.stderr


def test_install_lands_index_and_support_dir(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    repo = f"{registry}/{unique_repo}/my-rule"
    _old_format_rule(f"{unique_repo}/my-rule")

    # Install the published rule into a clean workspace.
    write_dir = project_dir / "consumer"
    write_dir.mkdir()
    (write_dir / ".claude").mkdir()
    consumer = grim_at(write_dir)
    write_config(write_dir, rules={"my-rule": f"{repo}:1.0.0"})
    consumer.run("lock", check=False)
    rows = consumer.json("install")["items"]
    assert {r["status"] for r in rows} == {"installed"}

    # Both the index AND the support tree land beside each other.
    rules = write_dir / ".claude/rules"
    assert (rules / "my-rule.md").is_file()
    assert (rules / "my-rule/examples.md").is_file()
    assert (rules / "my-rule/schema.json").is_file()
    assert (rules / "my-rule/examples.md").read_text(encoding="utf-8") == "# Examples\nworked example\n"

    # A no-op reinstall: intact footprint ⇒ nothing to do.
    again = consumer.json("install")["items"]
    assert {r["status"] for r in again} == {"unchanged"}


def test_support_file_edit_is_detected_as_drift(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    repo = f"{registry}/{unique_repo}/my-rule"
    _old_format_rule(f"{unique_repo}/my-rule")
    runner = grim_at(project_dir)

    write_config(project_dir, rules={"my-rule": f"{repo}:1.0.0"})
    runner.run("lock", check=False)
    runner.json("install")

    # Hand-edit a *support* file (not the index): status must flag drift.
    (project_dir / ".claude/rules/my-rule/examples.md").write_text("tampered\n", encoding="utf-8", newline="\n")
    status = runner.json("status")["items"]
    by_name = {r["name"]: r for r in status}
    assert by_name["my-rule"]["state"] == "modified", (
        f"editing a support file must be detected as drift, got {status}"
    )


def test_uninstall_removes_index_and_support_dir(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    repo = f"{registry}/{unique_repo}/my-rule"
    _old_format_rule(f"{unique_repo}/my-rule")
    runner = grim_at(project_dir)

    write_config(project_dir, rules={"my-rule": f"{repo}:1.0.0"})
    runner.run("lock", check=False)
    runner.json("install")

    rules = project_dir / ".claude/rules"
    assert (rules / "my-rule.md").is_file()
    assert (rules / "my-rule").is_dir()

    out = runner.json("uninstall", "rule", "my-rule")
    assert out["status"] == "uninstalled"

    # The index file AND the support directory are both gone.
    assert not (rules / "my-rule.md").exists()
    assert not (rules / "my-rule").exists()

    # Idempotent: a second uninstall is a reported no-op.
    again = runner.json("uninstall", "rule", "my-rule")
    assert again["status"] == "not-installed"


def test_rerelease_is_idempotent(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    index = project_dir / "my-rule.md"
    _write(index, "---\npaths: ['**/*.rs']\n---\n# my-rule\nbody\n")
    repo = f"{registry}/{unique_repo}/my-rule"
    runner = grim_at(project_dir)

    first = runner.json("release", str(index), f"{repo}:1.0.0")
    second = runner.json("release", str(index), f"{repo}:1.0.0")
    assert first["manifest_digest"] == second["manifest_digest"], (
        "re-releasing an identical rule must yield the same digest"
    )


def test_support_dir_is_excluded_from_claude_auto_load(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The support tree lands, but never as unconditional Claude context.

    Claude Code discovers ``.claude/rules/`` recursively and loads any file
    without ``paths:`` frontmatter into every session, so grim's own copy of
    the support directory would drown the consumer's context. grim registers
    the directory in ``claudeMdExcludes`` instead — auto-load suppressed,
    files still on disk so the index's relative links resolve
    (grimoire-rs/grimoire#102).
    """
    repo = f"{registry}/{unique_repo}/my-rule"
    _old_format_rule(f"{unique_repo}/my-rule")

    # .claude/ pre-created to force Claude as the target client, matching
    # test_install_lands_index_and_support_dir's consumer/ pattern.
    write_dir = project_dir / "consumer"
    write_dir.mkdir()
    (write_dir / ".claude").mkdir()
    runner = grim_at(write_dir)
    write_config(write_dir, rules={"my-rule": f"{repo}:1.0.0"})
    runner.run("lock", check=False)
    runner.json("install")

    settings = write_dir / ".claude/settings.json"
    assert settings.is_file(), "installing a support-dir rule registers the exclusion"
    assert json.loads(settings.read_text(encoding="utf-8"))["claudeMdExcludes"] == [
        "**/.claude/rules/my-rule/**"
    ]
    # Suppressing auto-load must not remove anything from disk.
    assert (write_dir / ".claude/rules/my-rule/examples.md").is_file()

    runner.json("uninstall", "rule", "my-rule")
    assert "claudeMdExcludes" not in json.loads(settings.read_text(encoding="utf-8")), (
        "the emptied managed key is dropped, never left as []"
    )


def test_single_file_rule_registers_no_exclusion(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A rule with no support directory is the index Claude should load."""
    index = project_dir / "plain.md"
    _write(index, "---\npaths: ['**/*.rs']\n---\n# plain\nbody\n")
    repo = f"{registry}/{unique_repo}/plain"
    runner = grim_at(project_dir)
    runner.json("release", str(index), f"{repo}:1.0.0")

    write_config(project_dir, rules={"plain": f"{repo}:1.0.0"})
    runner.run("lock", check=False)
    runner.json("install")

    assert (project_dir / ".claude/rules/plain.md").is_file()
    assert not (project_dir / ".claude/settings.json").exists(), (
        "nothing to exclude, so no settings file is created"
    )


def test_release_refuses_a_rule_with_support_files(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    index = _local_rule_with_support_file(project_dir)
    repo = f"{unique_repo}/my-rule"

    result = grim_at(project_dir).run("release", str(index), f"{registry}/{repo}:1.0.0", check=False)

    _assert_refused(result)
    assert not _tag_exists(repo, "1.0.0"), "a refused release must push nothing"


def test_build_refuses_a_rule_with_support_files(grim_at, project_dir: Path) -> None:
    index = _local_rule_with_support_file(project_dir)

    _assert_refused(grim_at(project_dir).run("build", str(index), check=False))


def test_build_accepts_readme_and_logo_in_the_rule_directory(grim_at, project_dir: Path) -> None:
    index = project_dir / "rules" / "my-rule.md"
    _write(index, "---\npaths: ['**/*.rs']\n---\n# my-rule\n")
    _write(project_dir / "rules" / "my-rule" / "README.md", "# my-rule\n")
    (project_dir / "rules" / "my-rule" / "logo.png").write_bytes(b"\x89PNG\r\n\x1a\n")
    _write(project_dir / "rules" / "my-rule" / "logo.svg", "<svg/>\n")

    out = grim_at(project_dir).json("build", str(index))

    assert out["status"] == "built"


def test_rule_readme_in_its_directory_is_not_packed(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    index = project_dir / "rules" / "x.md"
    _write(index, "---\npaths: ['**/*.rs']\n---\n# x\n")
    _write(project_dir / "rules" / "x" / "README.md", "# x\n")
    repo = f"{registry}/{unique_repo}/x"
    runner = grim_at(project_dir)
    runner.json("release", str(index), f"{repo}:1.0.0")

    paths = [f["path"] for f in runner.json("fetch", f"{repo}:1.0.0")["files"]]
    assert paths == ["x.md"], paths
    result = runner.run("fetch", f"{repo}:1.0.0", "--path", "x/README.md", check=False)
    assert result.returncode != 0, "the README must not ride the package"
    assert "no file 'x/README.md' in this artifact" in result.stderr, result.stderr


def test_old_support_dir_rule_warns_once_on_install(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    repo = f"{registry}/{unique_repo}/my-rule"
    _old_format_rule(f"{unique_repo}/my-rule")
    (project_dir / ".claude").mkdir(exist_ok=True)
    runner = grim_at(project_dir)
    write_config(project_dir, rules={"my-rule": f"{repo}:1.0.0"})
    runner.run("lock", check=False)

    result = runner.run("install", "--client", "claude,opencode")

    assert result.stderr.count(DEPRECATION) == 1, result.stderr
    assert repo in result.stderr, result.stderr
    assert (project_dir / ".claude/rules/my-rule/examples.md").is_file()


def test_update_to_single_file_rule_removes_support_dir_and_exclusion(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    repo_path = f"{unique_repo}/my-rule"
    repo = f"{registry}/{repo_path}"
    _old_format_rule(repo_path, tag="stable")
    (project_dir / ".claude").mkdir(exist_ok=True)
    runner = grim_at(project_dir)
    write_config(project_dir, rules={"my-rule": f"{repo}:stable"})
    runner.run("lock", check=False)
    runner.json("install")
    rules = project_dir / ".claude/rules"
    settings = project_dir / ".claude/settings.json"
    assert (rules / "my-rule").is_dir()
    assert "claudeMdExcludes" in json.loads(settings.read_text(encoding="utf-8"))

    make_artifact(repo_path, "rule", {"my-rule.md": "---\npaths: ['**/*.rs']\n---\n# single file\n"}, tag="stable")
    runner.json("update")

    assert (rules / "my-rule.md").is_file()
    assert not (rules / "my-rule").exists()
    assert "claudeMdExcludes" not in json.loads(settings.read_text(encoding="utf-8"))
