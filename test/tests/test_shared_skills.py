# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Shared ``.agents/skills`` pool refcount + Kiro global-scope inertness.

Wave-1 vendor expansion introduces two guards that need end-to-end proof:

1. **Refcount guard** (``prune::reap_dropped_clients``): Codex, Gemini, Zed,
   and Amp all target the same ``.agents/skills/<name>`` directory. Removing
   one client's record must NOT delete a directory another client's output
   still references — the shared dir survives until the LAST referencing
   client drops it (adr_vendor_wave_expansion.md §3).
2. **Kiro global scoped rule** is written correctly at global scope but is
   inert until upstream Kiro #9176 closes; grim emits a render-layer warning
   citing the issue (self-heals on the upstream fix, no grim change).
"""
from __future__ import annotations

import json
from pathlib import Path

from src.helpers import make_artifact
from src.runner import GrimRunner


# ---------------------------------------------------------------------------
# Shared-pool refcount: a dropped client's reap keeps the shared dir alive
# ---------------------------------------------------------------------------


def test_shared_agents_skills_survives_dropped_client_then_reaps_on_last(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Install one skill for codex+zed+amp → a single ``.agents/skills/<name>``
    dir recorded once per client. Narrowing ``[options].clients`` to drop zed
    and running ``update`` reaps zed's record but LEAVES the shared dir (codex
    and amp still reference it). A full uninstall finally removes the dir."""
    sk = make_artifact(
        f"{unique_repo}/shared-skill",
        "skill",
        {"shared-skill/SKILL.md": "---\nname: shared-skill\ndescription: d\n---\n# body\n"},
        tag="v1",
    )
    shared_dir = project_dir / ".agents/skills/shared-skill"

    # All three pool members select the same .agents/skills target.
    (project_dir / "grimoire.toml").write_text(
        '[options]\nclients = ["codex", "zed", "amp"]\n\n'
        f'[skills]\nshared-skill = "{sk.fq}"\n'
    )
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install")["items"]
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    # One physical directory, referenced once per client (3 outputs, 1 path).
    assert (shared_dir / "SKILL.md").is_file(), "the shared skill dir must exist after install"
    status = runner.json("status")["items"]
    item = next(r for r in status if r["name"] == "shared-skill")
    clients = {o["client"] for o in item["outputs"]}
    assert clients == {"codex", "zed", "amp"}, f"all three pool clients must record an output: {clients}"
    paths = {o["path"] for o in item["outputs"]}
    assert len(paths) == 1, f"all pool outputs must point at ONE shared dir: {paths}"

    # Drop zed from the client set; update reaps zed but the dir survives
    # because codex + amp still reference it.
    (project_dir / "grimoire.toml").write_text(
        '[options]\nclients = ["codex", "amp"]\n\n'
        f'[skills]\nshared-skill = "{sk.fq}"\n'
    )
    update_rows = runner.json("update")["items"]
    row = next(r for r in update_rows if r["name"] == "shared-skill")
    assert "zed" in row.get("reaped_clients", []), (
        f"zed must be reported reaped when dropped from the client set: {row}"
    )
    assert (shared_dir / "SKILL.md").is_file(), (
        "the shared dir MUST survive while codex+amp still reference it (refcount guard)"
    )
    status_after = runner.json("status")["items"]
    item_after = next(r for r in status_after if r["name"] == "shared-skill")
    clients_after = {o["client"] for o in item_after["outputs"]}
    assert clients_after == {"codex", "amp"}, f"zed's output must be gone from status: {clients_after}"

    # Full uninstall removes the last references and the shared dir with them.
    runner.json("uninstall", "skill", "shared-skill")
    assert not shared_dir.exists(), "uninstalling the last references must remove the shared dir"


# ---------------------------------------------------------------------------
# Kiro global scoped rule: written correctly, warned as upstream-inert
# ---------------------------------------------------------------------------


def test_kiro_global_scoped_rule_writes_file_and_warns_upstream_inertness(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """A scoped rule installed with ``--client kiro --global`` writes correct
    ``fileMatch`` steering to ``$HOME/.kiro/steering/<name>.md`` AND emits a
    render-layer warning citing upstream Kiro #9176 (global fileMatch is inert
    until that bug is fixed). The warning cites the issue number as a stable
    anchor, not exact prose."""
    ru = make_artifact(
        f"{unique_repo}/kiro-scoped",
        "rule",
        {"kiro-scoped.md": "---\npaths: ['**/*.rs']\n---\n# Kiro scoped\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[rules]\nkiro-scoped = "{ru.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    result = runner.run(
        "install", "--global", "--client", "kiro", format="json", log_level="warn"
    )
    rows = json.loads(result.stdout)["items"]
    assert rows[0]["status"] == "installed", rows

    # Correct output IS written at global scope (self-heals when #9176 closes).
    steering = runner.home / ".kiro/steering/kiro-scoped.md"
    assert steering.is_file(), "global Kiro scoped rule must still be written at $HOME/.kiro/steering/"
    assert "fileMatch" in steering.read_text(), "the global steering file must carry fileMatch scoping"

    # Honest render-layer warning cites the upstream issue as a stable anchor.
    assert "9176" in result.stderr, (
        f"global Kiro scoped rule must warn citing upstream #9176; got: {result.stderr!r}"
    )
