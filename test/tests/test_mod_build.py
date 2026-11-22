# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""``grim build`` and ``grim release`` for the experimental `mod` kind: kind
inference, the default entry, the refusals the packer owns, and the packed
descriptor.

A mod is a directory holding ``mod.toml`` plus one directory per harness. The
build refuses at authoring time every layer a grim would refuse at install.
"""

from __future__ import annotations

import io
import os
import tarfile
from pathlib import Path

import pytest

from src.registry import fetch_blob, fetch_manifest

MOD_TOML = 'schema = 1\nname = "band"\ndescription = "Build status band"\n'


def _write(path: Path, body: str | bytes = "export default {};\n") -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    if isinstance(body, bytes):
        path.write_bytes(body)
    else:
        path.write_text(body, encoding="utf-8", newline="\n")
    return path


def _mod(root: Path, files: dict[str, str] | None = None, toml: str = MOD_TOML) -> Path:
    """A mod directory ``band`` under ``root`` with ``files`` beside ``mod.toml``."""
    directory = root / "band"
    _write(directory / "mod.toml", toml)
    for rel, body in (files or {"claude/band.tsx": "export default {};\n"}).items():
        _write(directory / rel, body)
    return directory


def _tar(raw: bytes) -> dict[str, str]:
    with tarfile.open(fileobj=io.BytesIO(raw)) as archive:
        return {
            m.name: archive.extractfile(m).read().decode()
            for m in archive.getmembers()
            if m.isfile()
        }


def test_build_infers_the_kind_and_resolves_the_default_entry(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    source = _mod(project_dir, {"claude/band.tsx": "a\n", "opencode/band.ts": "b\n"})
    runner = grim_at(project_dir)

    built = runner.json("build", str(source))
    assert built["kind"] == "mod"
    assert built["name"] == "band"

    repo_path = f"{unique_repo}/band"
    released = runner.json("release", str(source), f"{registry}/{repo_path}:1.0.0")
    assert released["pushed"] is True
    manifest = fetch_manifest(repo_path, "1.0.0")
    annotations = manifest["annotations"]
    assert annotations["com.grimoire.kind"] == "mod"
    assert annotations["com.grimoire.mod.clients"] == "claude,opencode"
    assert "com.grimoire.requires-features" not in annotations
    files = _tar(fetch_blob(repo_path, manifest["layers"][0]["digest"]))
    assert files["band/mod.toml"] == (
        'schema = 1\nname = "band"\ndescription = "Build status band"\n\n'
        '[claude]\nentry = "band.tsx"\n\n[opencode]\nentry = "band.ts"\n'
    )
    assert sorted(files) == [
        "band/claude/band.tsx",
        "band/mod.toml",
        "band/opencode/band.ts",
    ]
    # The author's file is untouched, and a second release is byte-identical.
    assert (source / "mod.toml").read_text(encoding="utf-8") == MOD_TOML
    again = runner.json("release", str(source), f"{registry}/{repo_path}:1.0.0")
    assert again["manifest_digest"] == released["manifest_digest"]

    # `--kind mod` is accepted, and an author's entry wins over the default.
    other = _mod(
        project_dir / "o",
        {"claude/main.ts": "x\n"},
        MOD_TOML + '[claude]\nentry = "main.ts"\n',
    )
    assert runner.json("build", str(other), "--kind", "mod")["kind"] == "mod"


@pytest.mark.skipif(os.name == "nt", reason="symlinks need privileges on Windows")
def test_symlink_refuses(grim_at, project_dir: Path) -> None:
    source = _mod(project_dir)
    runner = grim_at(project_dir)
    assert runner.json("build", str(source))["kind"] == "mod", (
        "positive control: the tree packs"
    )

    (source / "claude" / "link.ts").symlink_to(source / "claude" / "band.tsx")
    refused = runner.run("build", str(source), check=False)
    assert refused.returncode == 65, refused.stderr
    assert "symlink" in refused.stderr


@pytest.mark.parametrize(
    "bad",
    [
        pytest.param("claude/data.json", id="non-code-extension"),
        pytest.param("claude/+page.ts", id="plus-sign"),
        pytest.param("claude/[id].ts", id="brackets"),
        pytest.param("claude/my file.ts", id="space"),
        pytest.param("claude/con.ts", id="device-name"),
        pytest.param("README.md", id="top-level-file"),
        pytest.param("amp/x.ts", id="unknown-harness-directory"),
    ],
)
def test_non_code_file_refuses(grim_at, project_dir: Path, bad: str) -> None:
    source = _mod(project_dir)
    runner = grim_at(project_dir)
    assert runner.json("build", str(source))["kind"] == "mod", "positive control"

    _write(source / bad, "x\n")
    refused = runner.run("build", str(source), check=False)
    assert refused.returncode == 65, refused.stderr
    assert (
        bad.rsplit("/", 1)[-1].split(".")[0] in refused.stderr
        or "refused" in refused.stderr
    )


def test_oversized_mod_toml_refuses(grim_at, project_dir: Path) -> None:
    source = _mod(project_dir)
    runner = grim_at(project_dir)
    assert runner.json("build", str(source))["kind"] == "mod", "positive control"

    (source / "mod.toml").write_text(
        MOD_TOML + "# " + "x" * 70_000 + "\n", encoding="utf-8", newline="\n"
    )
    refused = runner.run("build", str(source), check=False)
    assert refused.returncode == 65, refused.stderr
    assert "mod.toml" in refused.stderr


def test_default_entry_needs_exactly_one_candidate(grim_at, project_dir: Path) -> None:
    runner = grim_at(project_dir)
    for files in (
        {"claude/other.ts": "x\n"},
        {"claude/band.ts": "x\n", "claude/band.tsx": "y\n"},
    ):
        refused = runner.run(
            "build", str(_mod(project_dir / str(len(files)), files)), check=False
        )
        assert refused.returncode == 65, refused.stderr
        assert "entry" in refused.stderr or "named band" in refused.stderr
