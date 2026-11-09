# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A catalog cache an older grim wrote, met by this one.

grim 0.15.1 cached a settings package as a kindless row: it read the manifest
before it knew the ``settings`` kind. The row's digest still matches, so a
refresh that reuses rows by digest alone keeps it kindless forever. These tests
seed that cache by hand, so no older grim is needed to write it.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pytest

from src.helpers import make_settings
from src.old_grim import OLD_BIN, checked_old_grim
from src.registry import LAYER_MEDIA_TYPE, REGISTRY_HOST, push_manifest
from src.runner import GrimRunner


def _seed_old_cache(grim_home: Path, locator: str, rows: dict[str, str]) -> Path:
    """Write the cache 0.15.1 leaves for ``rows`` (repository to digest): no kinds."""
    host = locator.split("/", 1)[0]
    entries = {
        repo: {
            "registry": host,
            "repository": repo,
            "oci": {"digest": digest},
            "latest_tag": "latest",
            "fetched_at": "2026-01-01T00:00:00Z",
        }
        for repo, digest in rows.items()
    }
    cache = {
        "version": 1,
        "registry": locator,
        "scope": "",
        "truncated": False,
        "built_at": "2026-01-01T00:00:00Z",
        "entries": entries,
    }
    key = hashlib.sha256(locator.encode()).hexdigest()[:16]
    path = grim_home / "catalog" / f"{key}.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(cache))
    return path


@pytest.fixture()
def upgraded(grim_at, project_dir: Path, registry: str, unique_repo: str):
    """A settings package and a foreign image, cached kindless as 0.15.1 left them."""
    settings = make_settings(
        f"{unique_repo}/team-defaults",
        "latest",
        {"description": "team defaults", "claude": {"model": "sonnet"}},
    )
    foreign = f"{unique_repo}/foreign-image"
    foreign_digest = push_manifest(foreign, "latest", [(LAYER_MEDIA_TYPE, b"not grim")])
    runner = grim_at(project_dir)
    locator = f"{REGISTRY_HOST}/{unique_repo}"
    cache = _seed_old_cache(
        runner.grim_home,
        locator,
        {settings.repo: settings.digest, foreign: foreign_digest},
    )
    return runner, locator, cache, settings.repo, foreign


def test_one_refresh_finds_a_settings_package_cached_kindless(upgraded) -> None:
    runner, locator, cache, settings_repo, foreign = upgraded

    # `settings` is a kind filter, so a row still cached kindless never matches.
    found = runner.json("search", "settings", "--registry", locator, "--refresh")

    repos = [row["repo"] for row in found["items"]]
    assert repos == [f"{REGISTRY_HOST}/{settings_repo}"], found["items"]
    assert found["items"][0]["kind"] == "settings"
    written = json.loads(cache.read_text())["entries"]
    assert written[settings_repo]["kind"] == "settings"
    assert "kind_read" not in written[settings_repo], (
        "a row with a kind needs no marker"
    )
    assert written[foreign]["kind_read"] is True, "the foreign row records its read"


@pytest.mark.skipif(not OLD_BIN, reason="GRIM_OLD_BIN names no grim 0.15.1 binary")
def test_grim_0_15_1_lists_the_rows_this_grim_cached(upgraded, tmp_path: Path) -> None:
    runner, locator, cache, _, foreign = upgraded
    runner.json("search", "--registry", locator, "--refresh")
    assert json.loads(cache.read_text())["entries"][foreign]["kind_read"] is True

    identity = tmp_path / "old-grim-identity"
    identity.mkdir()
    old = GrimRunner(checked_old_grim(identity), runner.grim_home, cwd=runner.cwd)
    listed_old = old.json("search", "--offline", "--registry", locator)
    listed_new = runner.json("search", "--offline", "--registry", locator)

    assert len(listed_new["items"]) == 2, listed_new["items"]
    assert listed_old["items"] == listed_new["items"]
    assert listed_old["sources"][0]["ok"] is True, listed_old["sources"]
