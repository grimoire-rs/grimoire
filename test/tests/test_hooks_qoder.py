# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Qoder hooks (ADR amendment A8), the C-121 pass branch.

C-121 passed on 2026-09-28 against `qodercli` 1.1.64
(`research_hooks_qoder_copilot_schemas.md` § 5), so Qoder arms hooks at
**global scope only** (D-2) by splicing marked elements into
`$QODER_CONFIG_DIR|~/.qoder/settings.json` — the same file grim's Qoder MCP
entries live in.

Scenarios: **S-103** (global arming across claude/codex/copilot/qoder, and an
unparseable surface reading `not-armed` with exit 0), **S-104** (project scope
skips qoder and points at `--global`), **S-105** / **C-123** (the user's own
Qoder hooks, keys and grim's MCP entry survive arm, re-arm and uninstall) and
**C-124** (a re-converge writes zero bytes to all four surfaces).
"""
from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from src.helpers import make_artifact, write_config
from src.runner import GrimRunner

pytestmark = pytest.mark.skipif(
    os.name == "nt", reason="the hook launcher and its registered command are POSIX-only in v1"
)

HOOK_TOML = """\
schema = 1
name = "shell-guard"
description = "Observes Bash tool calls before they run."

[[hooks]]
id = "guard"
event = "PreToolUse"
tier = "observer"
matcher = "Bash"
command = "sh guard.sh"
timeout = 5
"""

GUARD_SH = "#!/bin/sh\ncat > /dev/null\nprintf '{}'\n"

MCP_TOML = """\
description = "grim's own MCP server."

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
"""

MARKER_KEY = "com.grimoire.managed"
MARKER_VALUE = "hook-dispatcher"

# A user's own Qoder settings: their own hook on the same event and matcher
# grim registers on, and an unrelated key. Hand-formatted, so a rewrite that
# re-serialises the document is visible byte for byte.
USER_QODER_SETTINGS = """\
{
  "theme": "dark",
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "echo mine"
          }
        ]
      }
    ]
  }
}
"""


def _publish_hook(unique_repo: str):
    return make_artifact(
        f"{unique_repo}/shell-guard",
        "hook",
        {"shell-guard/hook.toml": HOOK_TOML, "shell-guard/guard.sh": GUARD_SH},
        tag="1",
    )


def _declare_globally(runner: GrimRunner, hooks: dict[str, str], mcp: dict[str, str] | None = None) -> None:
    lines = []
    if mcp:
        lines += ["[mcp]", *(f'{name} = "{ref}"' for name, ref in mcp.items()), ""]
    if hooks:
        lines += ["[hooks]", *(f'{name} = "{ref}"' for name, ref in hooks.items()), ""]
    lines += ["[options.experimental]", "hooks = true", ""]
    home = Path(runner.grim_home)
    home.mkdir(parents=True, exist_ok=True)
    (home / "grimoire.toml").write_text("\n".join(lines))


def _surfaces(home: Path) -> dict[str, Path]:
    """Each client's global hook surface under the isolated `$HOME`."""
    return {
        "claude": home / ".claude" / "settings.json",
        "codex": home / ".codex" / "hooks.json",
        "copilot": home / ".copilot" / "hooks" / "grim.json",
        "qoder": home / ".qoder" / "settings.json",
    }


def _detect(home: Path, clients: list[str]) -> None:
    """Make each client detected at global scope (copilot keys on its skills root)."""
    for client in clients:
        marker = home / ".copilot" / "skills" if client == "copilot" else home / f".{client}"
        marker.mkdir(parents=True, exist_ok=True)


def _rows(runner: GrimRunner) -> list[dict]:
    path = Path(runner.grim_home) / "hooks" / "dispatch.json"
    if not path.is_file():
        return []
    return [row for root in json.loads(path.read_text())["roots"].values() for row in root["hooks"]]


def _marked(settings: dict) -> list[dict]:
    return [
        element
        for groups in settings.get("hooks", {}).values()
        for group in groups
        for element in group.get("hooks", [])
        if element.get(MARKER_KEY) == MARKER_VALUE
    ]


def _release_mcp(runner: GrimRunner, tmp_path: Path, registry: str, unique_repo: str) -> str:
    descriptor = tmp_path / "src" / "mcp" / "grim-mcp.toml"
    descriptor.parent.mkdir(parents=True, exist_ok=True)
    descriptor.write_text(MCP_TOML)
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")
    return ref


# ---------------------------------------------------------------------------
# S-103 + C-124 — one global install arms all four clients; a re-run writes nothing
# ---------------------------------------------------------------------------


def test_global_arming_across_clients(grim, registry: str, unique_repo: str) -> None:
    """**S-103.** With claude, codex, copilot and qoder detected, one global
    install arms a grim element per client — qoder's spliced into its own
    `settings.json` — and a dispatch row per client.

    **C-124.** A second install with nothing changed writes zero bytes to every
    one of the four surfaces (bytes and mtime), the renderer self-heal
    obligation of Principle 9.
    """
    home = Path(grim.home)
    hook = _publish_hook(unique_repo)
    _declare_globally(grim, {"shell-guard": hook.fq})
    _detect(home, ["claude", "codex", "copilot", "qoder"])
    grim.run("lock", "--global")

    report = grim.json("install", "--global", "--trust-hooks")
    row = json.dumps(next(item for item in report["items"] if item["kind"] == "hook"))
    for client in ("claude", "codex", "copilot", "qoder"):
        assert client in row, f"the install report never names {client}: {row}"
    assert sorted(r["client"] for r in _rows(grim)) == ["claude", "codex", "copilot", "qoder"], _rows(grim)

    surfaces = _surfaces(home)
    qoder = json.loads(surfaces["qoder"].read_text())
    group = qoder["hooks"]["PreToolUse"][0]
    assert group["matcher"] == "Bash", qoder
    [element] = _marked(qoder)
    assert element["type"] == "command" and "--client qoder" in element["command"], element
    assert element["timeout"] == 5, "qoder's timeout is seconds, like claude's"
    assert len(_marked(json.loads(surfaces["claude"].read_text()))) == 1
    for client in ("codex", "copilot"):
        assert "PreToolUse" in json.loads(surfaces[client].read_text())["hooks"], client

    before = {name: (path.read_bytes(), path.stat().st_mtime_ns) for name, path in surfaces.items()}
    grim.json("install", "--global", "--trust-hooks")
    after = {name: (path.read_bytes(), path.stat().st_mtime_ns) for name, path in surfaces.items()}
    for name in surfaces:
        assert after[name] == before[name], f"C-124: a re-converge rewrote {name}'s hook surface"


def test_unparseable_surface_not_armed(grim, registry: str, unique_repo: str) -> None:
    """**S-103, error case / C-017.** A Qoder `settings.json` grim cannot parse
    is never rewritten, and qoder is reported `not-armed`: one warning naming
    the client and the hook, no `armed` entry in the install report, and
    `not-registered` in `grim status` — the table no longer arms it. Claude
    still arms and the exit stays 0.
    """
    home = Path(grim.home)
    hook = _publish_hook(unique_repo)
    _declare_globally(grim, {"shell-guard": hook.fq})
    _detect(home, ["claude", "qoder"])
    broken = _surfaces(home)["qoder"]
    broken.write_text("{ this is not json")
    grim.run("lock", "--global")

    result = grim.run("install", "--global", "--trust-hooks", check=False)
    assert result.returncode == 0, result.stderr
    warnings = [line for line in result.stderr.splitlines() if "not armed for qoder" in line]
    assert len(warnings) == 1 and "shell-guard" in warnings[0], result.stderr
    assert broken.read_text() == "{ this is not json", "an unparseable user config must never be rewritten"
    assert [r["client"] for r in _rows(grim)] == ["claude"], _rows(grim)
    assert len(_marked(json.loads(_surfaces(home)["claude"].read_text()))) == 1, "POSITIVE CONTROL"

    report = grim.json("install", "--global", "--trust-hooks")
    row = next(item for item in report["items"] if item["kind"] == "hook")
    assert [a["client"] for a in row["armed"]] == ["claude"], row

    status = next(i for i in grim.json("status", "--global")["items"] if i["kind"] == "hook")
    assert [(a["client"], a["cause"]) for a in status["arming"]] == [("qoder", "not-registered")], status


# ---------------------------------------------------------------------------
# S-104 — qoder is global-only
# ---------------------------------------------------------------------------


def test_project_scope_skips_qoder_and_points_at_global(grim_at, project_dir: Path, registry: str, unique_repo: str) -> None:
    """**S-104.** A project install with qoder selected skips qoder with a
    warning that says hooks are global-only there and points at `--global`;
    `.qoder/settings.json` is a tracked repository file (A1). Claude arms.
    """
    hook = _publish_hook(unique_repo)
    write_config(project_dir, hooks={"shell-guard": hook.fq}, options={"clients": '["claude", "qoder"]'})
    (project_dir / ".qoder").mkdir()
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")

    result = runner.run("install", "--trust-hooks", check=False)
    assert result.returncode == 0, result.stderr
    warning = next((line for line in result.stderr.splitlines() if "qoder" in line), "")
    assert "shell-guard" in warning and "--global" in warning, result.stderr
    assert "global-only" in warning, result.stderr
    assert not (project_dir / ".qoder" / "settings.json").exists(), "nothing armable in a tracked file"
    assert [r["client"] for r in _rows(runner)] == ["claude"], "POSITIVE CONTROL: claude must arm"


# ---------------------------------------------------------------------------
# S-105 / C-123 — the user's own Qoder config survives
# ---------------------------------------------------------------------------


def test_user_hooks_and_the_mcp_entry_survive_arm_rearm_and_uninstall(
    grim, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """**S-105 / C-123.** Qoder's `settings.json` holds the user's own hook, an
    unrelated key and grim's MCP entry. Arming, re-arming and uninstalling the
    hook changes nothing but grim's element, and the MCP row stays `installed`
    at every step — its recorded hash never moves under a hook splice.
    """
    home = Path(grim.home)
    _detect(home, ["qoder"])
    settings = _surfaces(home)["qoder"]
    settings.write_text(USER_QODER_SETTINGS)
    mcp = _release_mcp(grim, tmp_path, registry, unique_repo)
    hook = _publish_hook(unique_repo)

    # Step 0: the MCP entry alone, the state a user has before hooks exist.
    _declare_globally(grim, {}, mcp={"grim-mcp": mcp})
    grim.run("lock", "--global")
    grim.json("install", "--global", "--client", "qoder")
    baseline = json.loads(settings.read_text())
    assert "grim-mcp" in baseline["mcpServers"], baseline

    baseline_text = settings.read_text()
    user_prefix = baseline_text[: baseline_text.index('"hooks"')]

    def assert_user_bytes_kept(step: str) -> None:
        text = settings.read_text()
        assert text.startswith(user_prefix), f"{step}: bytes outside `hooks` moved\n{text}"
        doc = json.loads(text)
        assert doc["theme"] == "dark", step
        assert doc["mcpServers"] == baseline["mcpServers"], f"{step}: {doc}"
        user = [e for g in doc["hooks"]["PreToolUse"] for e in g["hooks"] if MARKER_KEY not in e]
        assert user == [{"type": "command", "command": "echo mine"}], f"{step}: {doc}"
        mcp_row = next(i for i in grim.json("status", "--global")["items"] if i["name"] == "grim-mcp")
        assert mcp_row["state"] == "installed", f"{step}: {mcp_row}"

    _declare_globally(grim, {"shell-guard": hook.fq}, mcp={"grim-mcp": mcp})
    grim.run("lock", "--global")
    grim.json("install", "--global", "--client", "qoder", "--trust-hooks")
    assert len(_marked(json.loads(settings.read_text()))) == 1, "the hook did not arm"
    assert_user_bytes_kept("arm")

    armed = settings.read_bytes()
    grim.json("install", "--global", "--client", "qoder", "--trust-hooks")
    assert settings.read_bytes() == armed, "re-arm rewrote the file"
    assert_user_bytes_kept("re-arm")

    grim.json("uninstall", "--global", "hook", "shell-guard")
    assert _marked(json.loads(settings.read_text())) == [], "uninstall left grim's element behind"
    assert_user_bytes_kept("uninstall")
    assert settings.read_text() == baseline_text, "uninstall must restore the user's exact bytes"
