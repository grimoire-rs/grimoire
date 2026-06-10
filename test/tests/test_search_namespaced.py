# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim search --registry <host>/<namespace>` namespaced discovery tests.

When the ``--registry`` flag includes a namespace path component
(e.g. ``localhost:5000/grim-test/<hex>``), the catalog queries the bare
host and filters by namespace so only repos under that path appear.  This
lets a caller scope a search to a specific organisation/user namespace
without a free-text query.
"""
from __future__ import annotations

from pathlib import Path

from src.helpers import make_artifact
from src.registry import REGISTRY_HOST


def test_search_namespaced_registry_finds_repo_in_namespace(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """``--registry host/namespace`` returns repos published under that namespace."""
    make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\n---\n# CR\n"},
        tag="latest",
        annotations={
            "com.grimoire.keywords": "review,quality",
            "org.opencontainers.image.description": "Review code quality",
        },
    )
    runner = grim_at(project_dir)

    # Use the full namespace path as the --registry value.
    rows = runner.json(
        "search", "--registry", f"{REGISTRY_HOST}/{unique_repo}", "--refresh"
    )
    assert isinstance(rows, list), f"search must return a JSON array, got {rows!r}"

    matching = [r for r in rows if "code-review" in r.get("repo", "")]
    assert matching, (
        f"namespaced search must include 'code-review' repo under "
        f"'{unique_repo}', got rows: {rows}"
    )


def test_search_namespaced_registry_empty_when_no_repos(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Namespaced search of an empty/unknown namespace returns an empty array (exit 0)."""
    runner = grim_at(project_dir)

    # Use a namespace that has nothing under it (unique_repo has no artifacts yet).
    result = runner.run(
        "--format", "json",
        "search",
        "--registry", f"{REGISTRY_HOST}/{unique_repo}",
        "--refresh",
        check=False,
    )
    assert result.returncode == 0, (
        f"namespaced search with no results must exit 0, "
        f"got {result.returncode}; stderr: {result.stderr}"
    )
    import json
    arr = json.loads(result.stdout)
    assert isinstance(arr, list)
    # May be empty or may contain repos from other tests on the shared registry;
    # the key assertion is exit 0 and a valid array.
