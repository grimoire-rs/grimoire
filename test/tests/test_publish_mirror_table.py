# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`publish.toml` per-entry `mirror` table: grammar, annotations, precedence,
and byte-identical output for a manifest that does not carry it."""
from __future__ import annotations

import json
import shutil
from pathlib import Path

import pytest

from src.registry import fetch_manifest

DATA = Path(__file__).resolve().parent.parent / "data" / "publish_mirror"
VERSION = "0.1.0"
COMMIT = "0123456789abcdef0123456789abcdef01234567"
GOOD = {
    "source": '"https://github.com/up/repo"',
    "commit": f'"{COMMIT}"',
    "path": '"skills/demo"',
    "license": '"MIT"',
    "license-source": '"detected"',
}


def _stage(project: Path) -> None:
    for sub in ("skills", "rules"):
        shutil.copytree(DATA / sub, project / sub)


def _manifest(project: Path, registry: str, repo: str, entries: str, head: str = "") -> Path:
    p = project / "publish.toml"
    p.write_text(
        f'registry = "{registry}"\nrepository_prefix = "{repo}"\n{head}\n{entries}',
        encoding="utf-8",
        newline="\n",
    )
    return p


def _table(**overrides: str | None) -> str:
    keys = {**GOOD, **overrides}
    body = "".join(f"{k} = {v}\n" for k, v in keys.items() if v is not None)
    return f'[skills.demo]\nversion = "{VERSION}"\n[skills.demo.mirror]\n{body}'


def _publish(runner, manifest: Path, check: bool = True):
    return runner.run("publish", "--no-git", "--manifest", str(manifest), format="json", check=check)


def test_mirror_annotations_win_over_authored_tiers(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    _stage(project_dir)
    m = _manifest(
        project_dir,
        registry,
        unique_repo,
        _table(**{"license": '"MIT"'}) + '[skills.demo.metadata]\nlicense = "BSD-3-Clause"\n',
        head='[metadata]\nlicense = "GPL-2.0"\nrepository = "https://top.example/r"\n',
    )
    runner = grim_at(project_dir)
    _publish(runner, m)
    repo = f"{unique_repo}/demo"
    ann = fetch_manifest(repo, VERSION)["annotations"]
    assert ann["com.grimoire.mirror.source"] == "https://github.com/up/repo"
    assert ann["com.grimoire.mirror.path"] == "skills/demo"
    assert ann["com.grimoire.mirror.license-source"] == "detected"
    assert ann["org.opencontainers.image.revision"] == COMMIT
    # SKILL.md frontmatter says Apache-2.0; the table outranks it.
    assert ann["org.opencontainers.image.licenses"] == "MIT"
    assert ann["org.opencontainers.image.source"] == "https://github.com/up/repo"
    # Not set by the table: the authored value stays.
    assert ann["com.grimoire.deprecated"] == "authored notice"

    d = runner.json("describe", f"{registry}/{repo}:{VERSION}")
    assert d["revision"] == COMMIT
    assert d["license"] == "MIT"
    assert d["repository"] == "https://github.com/up/repo"
    assert d["annotations"]["com.grimoire.mirror.path"] == "skills/demo"


def test_mirror_deprecated_and_replaced_by_win(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    _stage(project_dir)
    m = _manifest(
        project_dir,
        registry,
        unique_repo,
        _table(deprecated='"mirror notice"', **{"replaced-by": '"ghcr.io/o/new:1"'}),
    )
    _publish(grim_at(project_dir), m)
    ann = fetch_manifest(f"{unique_repo}/demo", VERSION)["annotations"]
    assert ann["com.grimoire.deprecated"] == "mirror notice"
    assert ann["com.grimoire.replaced-by"] == "ghcr.io/o/new:1"


def test_file_source_leaves_image_source_alone(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    _stage(project_dir)
    m = _manifest(project_dir, registry, unique_repo, _table(source='"file:///srv/up"'))
    _publish(grim_at(project_dir), m)
    ann = fetch_manifest(f"{unique_repo}/demo", VERSION)["annotations"]
    assert ann["com.grimoire.mirror.source"] == "file:///srv/up"
    assert ann["org.opencontainers.image.source"] == "https://authored.example/demo"


@pytest.mark.parametrize(
    ("key", "value"),
    [
        ("source", '"http://e.example/r"'),
        ("commit", '"ABCDEF0123456789abcdef0123456789abcdef01"'),
        ("path", '"../x"'),
        ("license", '""'),
        ("license-source", '"guessed"'),
        ("source", None),
        ("commit", None),
        ("path", None),
        ("license", None),
        ("license-source", None),
    ],
    ids=lambda v: str(v)[:20],
)
def test_bad_mirror_key_exits_65_naming_the_key(
    grim_at, project_dir: Path, registry: str, unique_repo: str, key: str, value: str | None
) -> None:
    _stage(project_dir)
    m = _manifest(project_dir, registry, unique_repo, _table(**{key: value}))
    r = _publish(grim_at(project_dir), m, check=False)
    assert r.returncode == 65, r.stderr
    assert f"mirror.{key}" in r.stderr, r.stderr


def test_unknown_mirror_key_is_a_parse_error(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    _stage(project_dir)
    m = _manifest(project_dir, registry, unique_repo, _table() + "bogus = 1\n")
    r = _publish(grim_at(project_dir), m, check=False)
    assert r.returncode == 65 and "bogus" in r.stderr, r.stderr


def capture_plain(runner, project: Path, registry: str, repo: str) -> dict:
    """Publish a manifest without the table; manifests with host and repo
    swapped for tokens, so they compare across runs."""
    _stage(project)
    m = _manifest(
        project,
        registry,
        repo,
        '[skills.demo]\nversion = "0.1.0"\n[rules.demo-rule]\nversion = "0.1.0"\n',
        head='[metadata]\nlicense = "MIT"\n',
    )
    _publish(runner, m)
    out = {}
    for key, path in (("skill", "demo"), ("rule", "demo-rule")):
        text = json.dumps(fetch_manifest(f"{repo}/{path}", "0.1.0"), sort_keys=True)
        out[key] = json.loads(text.replace(registry, "@REG@").replace(repo, "@REPO@"))
    return out


def test_manifest_without_table_is_unchanged(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    golden = json.loads((DATA / "golden_no_table.json").read_text(encoding="utf-8"))
    got = capture_plain(grim_at(project_dir), project_dir, registry, unique_repo)
    assert got == golden
