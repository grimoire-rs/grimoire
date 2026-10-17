# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Builders for package-index (`[[registries]] index = …`) acceptance tests.

Plain helpers only; the ``http_index`` fixture that serves them lives in
``tests/conftest.py`` and delegates to ``serve``.
"""

from __future__ import annotations

import json
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any


def package(
    name: str,
    kind: str,
    ref: str,
    description: str,
    keywords: list[str] | None = None,
    summary: str | None = None,
    deprecated: str | None = None,
    replaced_by: str | None = None,
) -> dict:
    pkg = {
        "schema": 1,
        "name": name,
        "kind": kind,
        "ref": ref,
        "description": description,
        "repository": "https://github.com/acme/skills",
        "owner": {"github": "acme", "id": 1},
    }
    # Omit-empty, mirroring the announce writer: pre-search-metadata
    # pointers carry neither key.
    if keywords:
        pkg["keywords"] = keywords
    if summary is not None:
        pkg["summary"] = summary
    if deprecated is not None:
        pkg["deprecated"] = deprecated
    if replaced_by is not None:
        pkg["replaced_by"] = replaced_by
    return pkg


def write_all_json(root: Path, packages: list[dict]) -> None:
    (root / "all.json").write_text(json.dumps(packages))


def index_config(project_dir: Path, locator: str) -> None:
    # `locator` goes into a TOML basic string, so callers passing a local
    # path must forward-slash it (`Path.as_posix()`): a Windows `str(path)`
    # embeds backslashes, which are invalid TOML escapes. git clones a
    # forward-slash path fine on every platform.
    (project_dir / "grimoire.toml").write_text(
        f"[[registries]]\n"
        f'alias = "hub"\n'
        f'index = "{locator}"\n'
        f"default = true\n"
        f"\n[skills]\n\n[rules]\n"
    )


def filtered_index_config(
    project_dir: Path,
    locator: str,
    include: tuple[str, ...] = (),
    exclude: tuple[str, ...] = (),
) -> None:
    """`index_config` plus a browse filter on the same single entry."""
    lines = [
        "[[registries]]",
        'alias = "hub"',
        f'index = "{locator}"',
    ]
    for key, patterns in (("include", include), ("exclude", exclude)):
        if patterns:
            lines.append(f"{key} = [" + ", ".join(f'"{p}"' for p in patterns) + "]")
    lines += ["default = true", "", "[skills]", "", "[rules]", ""]
    (project_dir / "grimoire.toml").write_text("\n".join(lines))


def write_stats(
    root: Path,
    entries: dict,
    schema_version: int = 1,
    rating_host: str | None = None,
    downloads_provider: str | None = None,
) -> None:
    providers: dict = {"rating": "github"}
    if rating_host is not None:
        providers["rating_host"] = rating_host
    if downloads_provider is not None:
        providers["downloads"] = downloads_provider
    (root / "stats.json").write_text(
        json.dumps(
            {
                "schema_version": schema_version,
                "generated_at": "2026-08-18T00:00:00Z",
                "providers": providers,
                "entries": entries,
            }
        )
    )


def search_envelope(runner, *args: str) -> dict[str, Any]:
    """``grim --format json search --refresh <args>``, exit 0 asserted;
    returns the whole envelope (``items``, ``sources``, ``total``, …)."""
    result = runner.run("--format", "json", "search", "--refresh", *args, check=False)
    assert result.returncode == 0, (
        f"index-backed search must exit 0, got {result.returncode}; stderr: {result.stderr}"
    )
    return json.loads(result.stdout)


@contextmanager
def serve(root: Path, handler_cls: type[SimpleHTTPRequestHandler]) -> Iterator[str]:
    """Serve ``root`` through ``handler_cls``; yields the ``http://`` base URL."""
    server = ThreadingHTTPServer(
        ("127.0.0.1", 0), partial(handler_cls, directory=str(root))
    )
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield f"http://127.0.0.1:{server.server_address[1]}"
    finally:
        server.shutdown()
        server.server_close()
