# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A team pre-configures a plugin marketplace through a settings package.

``adr_settings_artifact_kind.md``, Decision 3. Claude and Copilot take the
marketplace under ``/extraKnownMarketplaces/<name>``, Codex under
``/marketplaces/<name>``. Each is one Map piece: the entry is grim's, a sibling
entry the user keeps is never compared, and uninstall removes the one entry.
Fixtures come from ``make_settings``; each "nothing was written" sits beside a
run that does write, in the same test.
"""

from __future__ import annotations

import json
import tomllib
from pathlib import Path
from subprocess import CompletedProcess

from src.helpers import make_settings, settings_sandbox, write_config
from src.registry import PublishedArtifact
from src.runner import GrimRunner

CLAUDE_ENTRY = {"source": {"source": "github", "repo": "acme/tools"}}
COPILOT_ENTRY = {"source": "https://example.com/acme"}
CODEX_ENTRY = {"source": "https://example.com/acme", "source_type": "git"}
MINE_CLAUDE = {"source": {"source": "github", "repo": "mine/own"}}
MINE_COPILOT = {"source": "https://example.com/mine"}
MINE_CODEX = {"source": "https://example.com/mine", "source_type": "git"}


def _package(repo: str, name: str, tables: dict, tag: str = "1") -> PublishedArtifact:
    return make_settings(f"{repo}/{name}", tag, {"description": f"{name} defaults", **tables})


def _global(
    grim: GrimRunner, home: Path, sandbox: Path, team: PublishedArtifact, clients: list[str]
) -> dict[str, Path]:
    write_config(
        home, settings={"team": team.fq}, options={"clients": json.dumps(clients)}, experimental_settings=True
    )
    env, roots = settings_sandbox(sandbox, grim.home)
    grim.env.update(env)
    grim.run("--global", "lock")
    return roots


def _changes(result: CompletedProcess[str]) -> list[tuple[str, str, str | None, str]]:
    rows = json.loads(result.stdout)["settings_changes"]
    return sorted((row["package"], row["key"], row["element"], row["action"]) for row in rows)


def _pieces(runner: GrimRunner) -> list[tuple[str, str, str, str | None]]:
    """Each recorded piece as `(package, client, pointer, element)`."""
    home = Path(runner.grim_home)
    stores = sorted((home / "settings").glob("*.json")) or [home / "state" / "global.json"]
    return sorted(
        (record["name"], output["client"], output["entry"], output.get("element"))
        for path in stores
        if path.exists()
        for record in json.loads(path.read_text(encoding="utf-8"))["records"]
        if record["kind"] == "settings"
        for output in record["outputs"]
    )


def test_claude_and_copilot_key(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(
        unique_repo,
        "team",
        {
            "claude": {"extraKnownMarketplaces": {"acme": CLAUDE_ENTRY}},
            "copilot": {"extraKnownMarketplaces": {"acme": COPILOT_ENTRY}},
        },
    )
    roots = _global(grim, grim_home, tmp_path / "s", team, ["claude", "copilot"])
    claude = roots["claude"] / "settings.json"
    copilot = roots["copilot"] / "settings.json"
    claude.write_text(json.dumps({"extraKnownMarketplaces": {"mine": MINE_CLAUDE}}), encoding="utf-8", newline="\n")
    copilot.write_text(json.dumps({"extraKnownMarketplaces": {"mine": MINE_COPILOT}}), encoding="utf-8", newline="\n")

    installed = grim.run("--global", "install", format="json")

    entry = "/extraKnownMarketplaces/acme"
    assert _changes(installed) == [("team", entry, None, "added")] * 2
    assert json.loads(claude.read_text(encoding="utf-8")) == {
        "extraKnownMarketplaces": {"mine": MINE_CLAUDE, "acme": CLAUDE_ENTRY}
    }
    assert json.loads(copilot.read_text(encoding="utf-8")) == {
        "extraKnownMarketplaces": {"mine": MINE_COPILOT, "acme": COPILOT_ENTRY}
    }
    # One Map piece per client: the entry, never the container or the sibling.
    assert _pieces(grim) == [("team", "claude", entry, None), ("team", "copilot", entry, None)]

    removed = grim.run("--global", "uninstall", "settings", "team", format="json")

    assert _changes(removed) == [("team", entry, None, "removed")] * 2
    assert json.loads(claude.read_text(encoding="utf-8")) == {"extraKnownMarketplaces": {"mine": MINE_CLAUDE}}
    assert json.loads(copilot.read_text(encoding="utf-8")) == {"extraKnownMarketplaces": {"mine": MINE_COPILOT}}
    assert _pieces(grim) == []


def test_codex_key(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"codex": {"marketplaces": {"acme": CODEX_ENTRY}}})
    roots = _global(grim, grim_home, tmp_path / "s", team, ["codex"])
    config = roots["codex"] / "config.toml"
    config.write_text(
        '[marketplaces.mine]\nsource = "https://example.com/mine"\nsource_type = "git"\n', encoding="utf-8", newline="\n"
    )

    installed = grim.run("--global", "install", format="json")

    entry = "/marketplaces/acme"
    assert _changes(installed) == [("team", entry, None, "added")]
    assert tomllib.loads(config.read_text(encoding="utf-8")) == {
        "marketplaces": {"mine": MINE_CODEX, "acme": CODEX_ENTRY}
    }
    assert _pieces(grim) == [("team", "codex", entry, None)]

    removed = grim.run("--global", "uninstall", "settings", "team", format="json")

    assert _changes(removed) == [("team", entry, None, "removed")]
    assert tomllib.loads(config.read_text(encoding="utf-8")) == {"marketplaces": {"mine": MINE_CODEX}}
    assert _pieces(grim) == []
