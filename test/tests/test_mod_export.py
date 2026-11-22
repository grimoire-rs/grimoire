# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim export plugin` with a mod member.

One admitted mod is staged: the author tree under ``src/`` and the install
``hooks/hooks.json`` bytes. Two or more admitted mods are all omitted (a Claude
plugin loads one hooks module). A plugin that stages a mod refuses an output
directory inside a client config root (64); a plugin of skills keeps exporting
there.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

from src.helpers import (
    make_artifact,
    make_mod,
    make_raw_mod,
    mod_sandbox,
    write_config,
)
from src.registry import LAYER_MEDIA_TYPE
from src.runner import GrimRunner

SKILL = "---\nname: a\ndescription: Demo skill.\n---\n# A\n"
TYPES = "export {};\n"


def _mod_toml(name: str) -> str:
    """A packed manifest: each entry is relative to its harness directory."""
    return (
        f'schema = 1\nname = "{name}"\ndescription = "test mod {name}"\n\n'
        '[claude]\nentry = "index.js"\n\n[opencode]\nentry = "index.ts"\n'
    )


def _mod(repo: str, files: dict[str, str | bytes] | None = None):
    name = repo.rsplit("/", 1)[-1]
    return make_mod(repo, "1", files, mod_toml=_mod_toml(name))


def _project(
    grim_at,
    root: Path,
    registry: str,
    unique_repo: str,
    *,
    mods: dict[str, dict[str, str | bytes]],
    skill: bool = True,
) -> GrimRunner:
    """A locked project declaring mods (name -> extra files) and, optionally, skill `a`."""
    declared = {}
    for name, files in mods.items():
        _mod(f"{unique_repo}/{name}", files)
        declared[name] = f"{registry}/{unique_repo}/{name}:1"
    skills = {}
    if skill:
        make_artifact(f"{unique_repo}/a", "skill", {"a/SKILL.md": SKILL}, tag="1")
        skills = {"a": f"{registry}/{unique_repo}/a:1"}
    root.mkdir(parents=True, exist_ok=True)
    write_config(root, skills=skills, mods=declared, experimental_mods=True)
    config = root / "grimoire.toml"
    config.write_text(
        config.read_text(encoding="utf-8") + '\n[plugin]\nname = "team"\n',
        encoding="utf-8",
        newline="\n",
    )
    runner = grim_at(root)
    runner.run("lock")
    return runner


def _export(
    runner: GrimRunner, out: str = "dist", client: str = "claude"
) -> subprocess.CompletedProcess[str]:
    return runner.run(
        "export",
        "plugin",
        "--project",
        "--client",
        client,
        "-o",
        out,
        format="json",
        check=False,
    )


def test_one_mod_is_staged_with_install_bytes(
    grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    proj = tmp_path / "proj"
    runner = _project(
        grim_at,
        proj,
        registry,
        unique_repo,
        mods={"m": {"claude/types/index.d.ts": TYPES}},
    )

    result = _export(runner)

    assert result.returncode == 0, result.stderr
    root = proj / "dist" / "team.claude"
    hooks = json.loads((root / "hooks" / "hooks.json").read_text(encoding="utf-8"))
    (module,) = hooks["modules"]
    assert module.startswith("../src/"), hooks
    assert (root / "hooks" / module).is_file(), "the hooks file names a staged file"
    assert (root / "src" / "types" / "index.d.ts").read_text(encoding="utf-8") == TYPES
    manifest = json.loads(
        (root / ".claude-plugin" / "plugin.json").read_text(encoding="utf-8")
    )
    assert manifest["name"] == "team", "the plugin keeps its own manifest"
    assert manifest["types"] == "./src/types/index.d.ts"
    assert list(manifest)[-1] == "types"
    # Positive control: the skill member of the same plugin is staged too.
    assert (root / "skills" / "a" / "SKILL.md").is_file()
    item = json.loads(result.stdout)["items"][0]
    assert any(m["kind"] == "mod" and m["name"] == "m" for m in item["members"])

    # The staged bytes are the install bytes: only plugin.json may differ.
    runner.run("install", "--trust-workspace", "--client", "claude")
    installed = proj / ".claude" / "mods" / "m"
    assert (root / "hooks" / "hooks.json").read_bytes() == (
        installed / "hooks" / "hooks.json"
    ).read_bytes()
    staged_src = sorted(
        p.relative_to(root / "src") for p in (root / "src").rglob("*") if p.is_file()
    )
    assert staged_src, "positive control: the staged tree holds files"
    assert staged_src == sorted(
        p.relative_to(installed / "src")
        for p in (installed / "src").rglob("*")
        if p.is_file()
    )
    for rel in staged_src:
        assert (root / "src" / rel).read_bytes() == (
            installed / "src" / rel
        ).read_bytes(), rel


def test_two_mods_are_both_omitted(
    grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    proj = tmp_path / "proj"
    runner = _project(grim_at, proj, registry, unique_repo, mods={"m1": {}, "m2": {}})

    result = _export(runner)

    assert result.returncode == 0, result.stderr
    root = proj / "dist" / "team.claude"
    item = json.loads(result.stdout)["items"][0]
    assert sorted((o["name"], o["reason"]) for o in item["omitted"]) == [
        ("m1", "not-representable"),
        ("m2", "not-representable"),
    ]
    assert result.stderr.count("one hooks module") == 1, result.stderr
    assert "one mod in each plugin" in result.stderr
    readme = (root / "README.md").read_text(encoding="utf-8")
    assert "Omitted for claude: mod m1, mod m2." in readme
    assert "loads one hooks module" in readme
    assert not (root / "src").exists()
    assert not (root / "hooks").exists()
    # Positive control: the skill member still ships.
    assert (root / "skills" / "a" / "SKILL.md").is_file()


def test_two_mods_and_nothing_else_is_an_empty_plugin(
    grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    proj = tmp_path / "proj"
    runner = _project(
        grim_at, proj, registry, unique_repo, mods={"m1": {}, "m2": {}}, skill=False
    )

    result = _export(runner)

    assert result.returncode == 65, result.stderr
    assert not (proj / "dist" / "team.claude").exists()


def test_mod_plugin_inside_claude_exits_64(
    grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    proj = tmp_path / "proj"
    runner = _project(grim_at, proj, registry, unique_repo, mods={"m": {}})
    env, roots = mod_sandbox(tmp_path / "sandbox")
    runner.env.update(env)

    # Positive control: the same plugin exports elsewhere.
    assert _export(runner, "dist").returncode == 0

    for out in (str(roots["claude"] / "out"), ".claude/out"):
        result = _export(runner, out)
        assert result.returncode == 64, (out, result.stderr)
    assert not (roots["claude"] / "out").exists()
    assert not (proj / ".claude" / "out").exists()


def test_mod_plugin_through_a_symlink_into_claude_exits_64(
    grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    proj = tmp_path / "proj"
    runner = _project(grim_at, proj, registry, unique_repo, mods={"m": {}})
    env, roots = mod_sandbox(tmp_path / "sandbox")
    runner.env.update(env)
    link = tmp_path / "alias"
    try:
        link.symlink_to(roots["claude"], target_is_directory=True)
    except OSError:
        import pytest

        pytest.skip("symlinks unavailable")

    result = _export(runner, str(link / "out"))

    assert result.returncode == 64, result.stderr
    assert not (roots["claude"] / "out").exists()


def test_skills_only_plugin_inside_claude_exits_0(
    grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    proj = tmp_path / "proj"
    runner = _project(grim_at, proj, registry, unique_repo, mods={})
    env, roots = mod_sandbox(tmp_path / "sandbox")
    runner.env.update(env)

    result = _export(runner, str(roots["claude"] / "out"))

    assert result.returncode == 0, result.stderr
    assert (roots["claude"] / "out" / "team.claude" / "skills" / "a").is_dir()


def test_grim_render_declines_mods(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    from src.stdio_drivers import drive_until_answered, initialize

    mod = _mod(f"{unique_repo}/m")
    skill = make_artifact(f"{unique_repo}/a", "skill", {"a/SKILL.md": SKILL}, tag="1")
    (project_dir / "grimoire.toml").write_text(
        "[skills]\n", encoding="utf-8", newline="\n"
    )
    runner = grim_at(project_dir)

    def call(req_id: int, ref: str, dest: Path) -> dict:
        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "tools/call",
            "params": {
                "name": "grim_render",
                "arguments": {"ref": ref, "vendor": "claude", "dest_dir": str(dest)},
            },
        }

    out_mod = project_dir / "out-mod"
    out_skill = project_dir / "out-skill"
    responses = drive_until_answered(
        runner,
        project_dir,
        [
            initialize(1),
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            call(2, mod.fq, out_mod),
            call(3, skill.fq, out_skill),
        ],
        allow_writes=True,
        offline=False,
    )

    declined = responses[2]
    text = json.dumps(declined)
    assert "error" in declined or declined["result"].get("isError") is True, declined
    assert "grim install" in text, text
    assert not out_mod.exists()
    # Positive control: the same server renders a skill.
    assert responses[3]["result"].get("isError") is not True, responses[3]
    assert (out_skill / "a" / "SKILL.md").is_file()


def test_two_mods_exported_into_claude_are_not_fenced(
    grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    proj = tmp_path / "proj"
    runner = _project(grim_at, proj, registry, unique_repo, mods={"m1": {}, "m2": {}})
    env, _roots = mod_sandbox(tmp_path / "sandbox")
    runner.env.update(env)

    result = _export(runner, ".claude/out")

    assert result.returncode == 0, result.stderr
    root = proj / ".claude" / "out" / "team.claude"
    item = json.loads(result.stdout)["items"][0]
    assert sorted(o["name"] for o in item["omitted"]) == ["m1", "m2"]
    assert not (root / "src").exists() and not (root / "hooks").exists()
    # Positive control: the skill member is staged in the same plugin.
    assert (root / "skills" / "a" / "SKILL.md").is_file()


def test_export_refuses_an_invalid_mod_and_writes_nothing(
    grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    import io
    import tarfile

    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as tar:
        body = b"export default {};\n"
        info = tarfile.TarInfo(name="bad/claude/index.js")
        info.size = len(body)
        # No mod.toml: the install validator refuses.
        tar.addfile(info, io.BytesIO(body))
    make_raw_mod(
        f"{unique_repo}/bad",
        "1",
        [(LAYER_MEDIA_TYPE, buf.getvalue())],
        clients="claude",
    )
    make_artifact(f"{unique_repo}/a", "skill", {"a/SKILL.md": SKILL}, tag="1")
    proj = tmp_path / "proj"
    proj.mkdir()
    write_config(
        proj,
        skills={"a": f"{registry}/{unique_repo}/a:1"},
        mods={"bad": f"{registry}/{unique_repo}/bad:1"},
        experimental_mods=True,
    )
    config = proj / "grimoire.toml"
    config.write_text(
        config.read_text(encoding="utf-8") + '\n[plugin]\nname = "team"\n',
        encoding="utf-8",
        newline="\n",
    )
    runner = grim_at(proj)
    runner.run("lock")

    result = _export(runner)

    assert result.returncode == 65, result.stderr
    assert json.loads(result.stdout)["error"]["reason"] == "invalid-mod"
    assert not (proj / "dist").exists()
