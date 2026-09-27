# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim export plugin` acceptance tests (design record S-001–S-018, S-026–S-031).

`grim export plugin` renders locked artifacts into a harness plugin — a
Claude-family tree (`.claude-plugin/plugin.json`) or an Agent Plugins tree
(`plugin.json` with `$schema`) — per `--client`, as a directory or a zip.
Ad-hoc refs never touch a lock; declared plugins (`marketplace.toml`) keep
their pins in `marketplace.lock`, written after the outputs are placed.

Fixture bundle `team-stack` = skill `team-plan`, skill `team-review`, agent
`team-reviewer`, rule `team-style`, mcp `team-srv` (stdio). No fixture file
mentions a member name outside its own frontmatter `name`, so the rename
tests start from a tree the stale-reference scan (C-022) accepts.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import sys
import zipfile
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path

import pytest
import tomllib

from src.helpers import make_artifact, make_bundle, write_config
from src.registry import push_artifact
from src.runner import GrimRunner

unix_only = pytest.mark.skipif(sys.platform == "win32", reason="needs a Unix host (flock, umask, symlinks)")

ONRAMP = "Packaged by grim (https://grimoire.rs); install grim for pinned, updatable installs."
AGENT_PLUGINS_SCHEMA = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json"
# C-025: the Agent Plugins 1.0 name pattern, asserted on every emitted name.
PLUGIN_NAME = re.compile(r"^(?!.*(?:--|\.\.))[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?$")
ITEM_KEYS = {"plugin", "client", "family", "format", "path", "version", "members", "omitted"}
# Decision 24: members/omitted sort by ArtifactKind order, then name.
KIND_ORDER = {"skill": 0, "rule": 1, "agent": 2, "bundle": 3, "mcp": 4}
LOCK_KINDS = ("skill", "rule", "agent", "mcp")

STDIO_MCP = """\
description = "Team server."

[server]
transport = "stdio"
command = "npx"
args = ["-y", "srv"]
"""

OAUTH_MCP = """\
description = "Server behind OAuth."

[server]
transport = "http"
url = "https://example.com/auth"

[server.oauth]
client_id = "abc"
"""

# S-011: vendor-namespaced metadata makes rendering visible in the bytes —
# lifted to a native key for claude, dropped as foreign for every other client.
PLAN_MD = '---\nname: team-plan\ndescription: Demo skill.\nmetadata:\n  claude.user-invocable: "false"\n---\n# Plan\n\nBody.\n'
REVIEWER_MD = (
    '---\nname: team-reviewer\ndescription: Reviews code.\nmodel: sonnet\nmetadata:\n  claude.color: "blue"\n---\n'
    "# Reviewer\nAgent body.\n"
)

REVIEW_PLAIN = "Check every change twice."
# S-016: line 7 of team-review/SKILL.md points into a sibling member.
REVIEW_STALE = "See ../team-plan/notes.md for context."


# ── fixtures ──────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class Stack:
    """The published `team-stack` bundle and its members."""

    bundle: str  # fully-qualified floating ref of the bundle
    refs: dict[str, str]  # member name -> fully-qualified ref the bundle lists
    digests: dict[tuple[str, str], str]  # (kind, name) -> manifest digest


@pytest.fixture()
def work(tmp_path: Path) -> Path:
    """The cwd `grim export plugin` runs in — no `grimoire.toml`, no marker."""
    d = tmp_path / "work"
    d.mkdir()
    return d


def _skill_md(name: str, heading: str, line7: str = "Body.") -> str:
    return f"---\nname: {name}\ndescription: Demo skill.\n---\n# {heading}\n\n{line7}\n"


def _skill(repo: str, name: str, heading: str = "Demo", tag: str = "1", **kw) -> str:
    """Publish a one-file skill; returns its digest (the fq ref is `{reg}/{repo}:{tag}`)."""
    return make_artifact(repo, "skill", {f"{name}/SKILL.md": _skill_md(name, heading)}, tag=tag, **kw).digest


def _release_mcp(runner: GrimRunner, src: Path, fq: str, body: str) -> str:
    """Publish an MCP descriptor via `grim release`; returns the manifest digest."""
    path = src / f"{fq.rsplit('/', 1)[1].split(':')[0]}.toml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body)
    return runner.json("release", str(path), fq, "--kind", "mcp")["manifest_digest"]


def _publish_stack(
    runner: GrimRunner,
    tmp_path: Path,
    registry: str,
    unique_repo: str,
    *,
    review_line7: str = REVIEW_PLAIN,
    annotations: dict[str, str] | None = None,
) -> Stack:
    """Publish the five `team-stack` members and the bundle listing them."""
    reg = f"{registry}/{unique_repo}"
    digests: dict[tuple[str, str], str] = {}
    refs: dict[str, str] = {}

    def put(kind: str, name: str, files: dict[str, str]) -> None:
        digests[(kind, name)] = make_artifact(f"{unique_repo}/{name}", kind, files, tag="1").digest
        refs[name] = f"{reg}/{name}:1"

    put(
        "skill",
        "team-plan",
        {"team-plan/SKILL.md": PLAN_MD, "team-plan/notes.md": "Planning notes.\n"},
    )
    put("skill", "team-review", {"team-review/SKILL.md": _skill_md("team-review", "Review", review_line7)})
    put(
        "agent",
        "team-reviewer",
        {"team-reviewer.md": REVIEWER_MD},
    )
    put("rule", "team-style", {"team-style.md": "# Style\nUse tabs.\n"})
    refs["team-srv"] = f"{reg}/team-srv:1.0.0"
    digests[("mcp", "team-srv")] = _release_mcp(runner, tmp_path / "src", refs["team-srv"], STDIO_MCP)

    kinds = {name: kind for (kind, name) in digests}
    members = [{"kind": kinds[n], "name": n, "id": refs[n]} for n in refs]
    layer = json.dumps({"members": members}).encode()
    bundle = push_artifact(f"{unique_repo}/team-stack", "1", layer, "bundle", annotations)
    return Stack(bundle=f"{registry}/{bundle.repo}:1", refs=refs, digests=digests)


def _annotated_stack(runner: GrimRunner, tmp_path: Path, registry: str, unique_repo: str) -> Stack:
    return _publish_stack(
        runner,
        tmp_path,
        registry,
        unique_repo,
        annotations={
            "org.opencontainers.image.version": "v1.4.0",
            "org.opencontainers.image.description": "The team stack.",
        },
    )


def _suffix(members: dict[tuple[str, str], str], rename: dict[str, str] | None = None) -> str:
    """C-023: first 12 hex of SHA-256 over sorted `kind\\temitted\\tdigest\\n` lines."""
    rename = rename or {}
    lines = sorted(f"{k}\t{rename.get(n, n)}\t{d}\n" for (k, n), d in members.items())
    return hashlib.sha256("".join(lines).encode()).hexdigest()[:12]


def _export(runner: GrimRunner, *args: str, fmt: str | None = "json") -> subprocess.CompletedProcess[str]:
    return runner.run("export", "plugin", *args, format=fmt, check=False)


def _ok(result: subprocess.CompletedProcess[str]) -> dict:
    assert result.returncode == 0, f"rc={result.returncode}\nstderr: {result.stderr}"
    return json.loads(result.stdout)


def _error(result: subprocess.CompletedProcess[str]) -> dict:
    return json.loads(result.stdout)["error"]


def _manifest(root: Path) -> dict:
    """Read a plugin's `plugin.json`, checking C-025's key order and name pattern."""
    claude = root / ".claude-plugin" / "plugin.json"
    if claude.exists():
        raw = claude.read_bytes()
        keys = ["name", "version", "description"]
    else:
        raw = (root / "plugin.json").read_bytes()
        keys = ["$schema", "name", "version", "description"]
    assert raw.endswith(b"}\n") and not raw.endswith(b"\n\n"), "plugin.json ends in exactly one newline"
    doc = json.loads(raw)
    assert list(doc) == keys, doc
    if "$schema" in doc:
        assert doc["$schema"] == AGENT_PLUGINS_SCHEMA
    assert PLUGIN_NAME.match(doc["name"]), doc["name"]
    return doc


def _tree(root: Path) -> list[tuple[str, str]]:
    """Sorted `(relpath, sha256)` of every regular file under ``root``."""
    out = []
    for dirpath, _dirs, files in os.walk(root):
        for f in files:
            p = Path(dirpath) / f
            out.append((p.relative_to(root).as_posix(), hashlib.sha256(p.read_bytes()).hexdigest()))
    return sorted(out)


def _entries(d: Path) -> list[str]:
    return sorted(p.name for p in d.iterdir()) if d.exists() else []


def _sha(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


def _write_marketplace(path: Path, plugins: dict[str, dict]) -> Path:
    """`{name: {"include": [...], "description"?: str, "version"?: str, "strip_prefix"?: str}}`."""
    chunks = []
    for name, decl in plugins.items():
        lines = [f"[plugins.{name}]", f"include = {json.dumps(decl['include'])}"]
        for key in ("description", "version"):
            if key in decl:
                lines.append(f"{key} = {json.dumps(decl[key])}")
        if "strip_prefix" in decl:
            lines.append(f"rename = {{ strip_prefix = {json.dumps(decl['strip_prefix'])} }}")
        chunks.append("\n".join(lines) + "\n")
    path.write_text("\n".join(chunks))
    return path


def _lock_part(lock_path: Path, plugin: str) -> tuple[list[dict], dict]:
    """One plugin's raw lock entries (all kinds) and its `[[plugin]]` row."""
    lock = tomllib.loads(lock_path.read_text())
    entries = [dict(e, kind=k) for k in LOCK_KINDS for e in lock.get(k, []) if e.get("plugin") == plugin]
    row = next(r for r in lock.get("plugin", []) if r["name"] == plugin)
    return entries, row


@contextmanager
def _held_flock(sidecar: Path) -> Iterator[None]:
    """Hold grim's advisory lock on ``sidecar`` (same lock space as fs4's flock)."""
    import fcntl

    fd = os.open(sidecar, os.O_RDWR | os.O_CREAT, 0o644)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield
    finally:
        os.close(fd)
        sidecar.unlink(missing_ok=True)


# ── S-001 — Claude-app zip from one bundle ref (C-014, C-016, C-023–C-026) ──


def test_s001_claude_zip_from_one_bundle_ref(grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    stack = _annotated_stack(runner, tmp_path, registry, unique_repo)

    out = _ok(_export(runner, stack.bundle, "--client", "claude", "--zip", "-o", "dist"))

    zpath = work / "dist" / "team-stack.claude.zip"
    assert _entries(work / "dist") == ["team-stack.claude.zip"]
    with zipfile.ZipFile(zpath) as z:
        names = z.namelist()
        manifest_bytes = z.read(".claude-plugin/plugin.json")
    for expected in (
        ".claude-plugin/plugin.json",
        "skills/team-plan/SKILL.md",
        "skills/team-plan/notes.md",
        "skills/team-review/SKILL.md",
        "agents/team-reviewer.md",
        ".mcp.json",
    ):
        assert expected in names, names
    assert not any("team-style" in n for n in names), names
    assert not any(n.endswith("/") for n in names), "C-026: no directory entries"
    assert names == sorted(names), "C-026: entries sorted bytewise"

    unpacked = tmp_path / "unpacked"
    unpacked.mkdir()
    (unpacked / ".claude-plugin").mkdir()
    (unpacked / ".claude-plugin" / "plugin.json").write_bytes(manifest_bytes)
    doc = _manifest(unpacked)
    assert doc["name"] == "team-stack"
    # C-023: the pinned bundle's version annotation, leading `v` stripped.
    assert doc["version"] == f"1.4.0+{_suffix(stack.digests)}"
    assert doc["description"] == f"The team stack. Omitted for this client: rule team-style. {ONRAMP}"

    (item,) = out["items"]
    assert item["family"] == "claude" and item["format"] == "zip"
    assert Path(item["path"]) == zpath
    assert not list(tmp_path.rglob("marketplace.lock")), "ad-hoc export writes no lock"
    assert not list(tmp_path.rglob("*.toml.lock")), "ad-hoc export takes no manifest lock"


def test_s001_registry_down_exits_69(grim_at, work: Path) -> None:
    runner = grim_at(work)
    host = "127.0.0.1:1"
    insecure = runner.env.get("GRIM_INSECURE_REGISTRIES")
    runner.env["GRIM_INSECURE_REGISTRIES"] = f"{insecure},{host}" if insecure else host

    result = _export(runner, f"{host}/grim-test/down/team-stack:1", "--client", "claude", "--zip", "-o", "dist")
    assert result.returncode == 69, result.stderr
    assert _entries(work / "dist") == []


def test_s001_absent_tag_exits_79(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    _skill(f"{unique_repo}/team-plan", "team-plan")

    result = _export(runner, f"{registry}/{unique_repo}/team-plan:9", "--client", "claude", "--zip", "-o", "dist")
    assert result.returncode == 79, result.stderr
    assert _entries(work / "dist") == []


# ── S-002 — Agent Plugins directory (C-016, C-020, C-025, C-036) ────────────


def test_s002_agent_plugins_directory_for_codex(grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    stack = _annotated_stack(runner, tmp_path, registry, unique_repo)

    out = _ok(_export(runner, stack.bundle, "--client", "codex", "-o", "out"))

    root = work / "out" / "team-stack.codex"
    doc = _manifest(root)
    assert doc["name"] == "team-stack"
    assert doc["version"] == f"1.4.0+{_suffix(stack.digests)}"
    assert doc["description"] == (
        f"The team stack. Omitted for this client: rule team-style, agent team-reviewer. {ONRAMP}"
    )
    assert (root / "skills" / "team-plan" / "SKILL.md").is_file()
    assert (root / "skills" / "team-review" / "SKILL.md").is_file()
    assert not (root / "agents").exists()
    assert not (root / ".claude-plugin").exists()
    assert not (root / ".mcp.json").exists()

    servers = json.loads((root / "mcp.json").read_text())["mcpServers"]
    assert servers == {"team-srv": {"args": ["-y", "srv"], "command": "npx", "type": "stdio"}}
    assert list(servers["team-srv"]) == sorted(servers["team-srv"]), "C-036: keys sorted"

    (item,) = out["items"]
    assert item["family"] == "agent-plugins" and item["format"] == "dir"
    assert Path(item["path"]) == root
    assert item["omitted"] == [
        {"kind": "rule", "name": "team-style", "reason": "no-format-surface"},
        {"kind": "agent", "name": "team-reviewer", "reason": "no-format-surface"},
    ]


# ── S-003 — Two refs need `--name` (C-002, C-014, C-023) ────────────────────


def test_s003_two_refs_without_name_exit_64_and_write_nothing(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    _skill(f"{unique_repo}/a", "a")
    _skill(f"{unique_repo}/b", "b")

    result = _export(runner, f"{registry}/{unique_repo}/a:1", f"{registry}/{unique_repo}/b:1", "--client", "claude")
    assert result.returncode == 64, result.stderr
    assert "--name" in result.stdout + result.stderr
    assert _entries(work) == []


def test_s003_two_refs_with_name_merge_into_one_plugin(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    # The annotation is ignored: only a single-ref export reads it (C-023).
    ann = {"org.opencontainers.image.version": "9.9.9"}
    da = _skill(f"{unique_repo}/a", "a", annotations=ann)
    db = _skill(f"{unique_repo}/b", "b", annotations=ann)

    out = _ok(
        _export(runner, f"{registry}/{unique_repo}/a:1", f"{registry}/{unique_repo}/b:1", "--name", "duo", "--client", "claude")
    )

    root = work / "duo.claude"
    assert (root / "skills" / "a" / "SKILL.md").is_file()
    assert (root / "skills" / "b" / "SKILL.md").is_file()
    doc = _manifest(root)
    assert doc["name"] == "duo"
    assert doc["version"] == f"0.0.0+{_suffix({('skill', 'a'): da, ('skill', 'b'): db})}"
    assert [(m["kind"], m["name"]) for m in out["items"][0]["members"]] == [("skill", "a"), ("skill", "b")]


@pytest.mark.parametrize("name", ["Duo", "a--b", "a:b", "a/b", "x" * 65])
def test_s003_invalid_name_flag_exits_64(grim_at, work: Path, registry: str, unique_repo: str, name: str) -> None:
    """C-002: `--name` must pass the plugin name rule (64, binding-name parity)."""
    runner = grim_at(work)
    _skill(f"{unique_repo}/a", "a")

    result = _export(runner, f"{registry}/{unique_repo}/a:1", "--name", name, "--client", "claude")
    assert result.returncode == 64, result.stderr
    assert _entries(work) == []


# ── S-004 — Member conflicts (C-003, C-009 step 5) ──────────────────────────


def test_s004_same_skill_name_from_two_repos_conflicts_78(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    _skill(f"{unique_repo}/acme/x", "x", heading="Acme")
    _skill(f"{unique_repo}/other/x", "x", heading="Other")
    first, second = f"{registry}/{unique_repo}/acme/x:1", f"{registry}/{unique_repo}/other/x:1"

    result = _export(runner, first, second, "--name", "d", "--client", "claude", fmt=None)
    assert result.returncode == 78, result.stderr
    assert first in result.stderr and second in result.stderr
    assert _entries(work) == []


def test_s004_same_ref_twice_is_deduplicated(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    _skill(f"{unique_repo}/acme/x", "x")
    ref = f"{registry}/{unique_repo}/acme/x:1"

    out = _ok(_export(runner, ref, ref, "--name", "d", "--client", "claude"))
    assert [m["name"] for m in out["items"][0]["members"]] == ["x"]


def test_s004_direct_ref_clashing_with_bundle_member_conflicts_78(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str
) -> None:
    runner = grim_at(work)
    stack = _publish_stack(runner, tmp_path, registry, unique_repo)
    _skill(f"{unique_repo}/other/team-plan", "team-plan", heading="Other")
    other = f"{registry}/{unique_repo}/other/team-plan:1"

    result = _export(runner, stack.bundle, other, "--name", "d", "--client", "claude", fmt=None)
    assert result.returncode == 78, result.stderr
    assert stack.bundle in result.stderr and other in result.stderr
    assert _entries(work) == []


def test_s004_direct_ref_equal_to_bundle_member_is_deduplicated(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str
) -> None:
    runner = grim_at(work)
    stack = _publish_stack(runner, tmp_path, registry, unique_repo)

    out = _ok(_export(runner, stack.bundle, stack.refs["team-plan"], "--name", "d", "--client", "claude"))
    names = [m["name"] for m in out["items"][0]["members"] if m["kind"] == "skill"]
    assert names == ["team-plan", "team-review"]


def test_s004_path_source_clashing_with_bundle_member_conflicts_78(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str
) -> None:
    runner = grim_at(work)
    stack = _publish_stack(runner, tmp_path, registry, unique_repo)
    local = work / "local" / "team-plan"
    local.mkdir(parents=True)
    (local / "SKILL.md").write_text(_skill_md("team-plan", "Local"))

    result = _export(runner, stack.bundle, "./local/team-plan", "--name", "d", "--client", "claude", "-o", "dist")
    assert result.returncode == 78, result.stderr
    assert _entries(work / "dist") == []


def test_s003_single_path_ref_is_named_by_its_binding_not_its_file_stem(grim_at, work: Path) -> None:
    """C-002 / decision 38: a path ref's plugin name is its binding (the packed
    skill's name, `team.plan`), never `Path::file_stem` (`team`)."""
    runner = grim_at(work)
    local = work / "skills" / "team.plan"
    local.mkdir(parents=True)
    (local / "SKILL.md").write_text(_skill_md("team.plan", "Local"))

    out = _ok(_export(runner, "./skills/team.plan", "--client", "claude", "-o", "dist"))

    assert _entries(work / "dist") == ["team.plan.claude"]
    root = work / "dist" / "team.plan.claude"
    assert _manifest(root)["name"] == "team.plan"
    assert (root / "skills" / "team.plan" / "SKILL.md").is_file()
    assert out["items"][0]["plugin"] == "team.plan"


# ── S-005 — Declared plugin, no lock (C-005–C-007, C-010, C-014, C-027) ─────


def test_s005_declared_plugin_writes_lock_and_is_stable_on_rerun(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str
) -> None:
    runner = grim_at(work)
    stack = _annotated_stack(runner, tmp_path, registry, unique_repo)
    _write_marketplace(work / "marketplace.toml", {"team": {"include": [stack.bundle]}})
    lock_path = work / "marketplace.lock"

    out = _ok(_export(runner, "--plugin", "team", "--client", "claude", "-o", "dist"))

    root = work / "dist" / "team.claude"
    doc = _manifest(root)
    assert doc["name"] == "team"
    # Declared mode reads no annotation: no declared version → 0.0.0.
    assert doc["version"] == f"0.0.0+{_suffix(stack.digests)}"
    assert out["items"][0]["version"] == doc["version"]

    lock = tomllib.loads(lock_path.read_text())
    entries = [e for k in LOCK_KINDS for e in lock.get(k, [])]
    assert {e["name"] for e in entries} == {"team-plan", "team-review", "team-reviewer", "team-style", "team-srv"}
    assert all(e["plugin"] == "team" for e in entries), entries
    assert [r["name"] for r in lock["plugin"]] == ["team"]
    assert not lock.get("bundle"), "a marketplace lock carries no [[bundle]]"
    first_lock, first_tree = lock_path.read_bytes(), _tree(root)

    _ok(_export(runner, "--plugin", "team", "--client", "claude", "-o", "dist", "--force"))
    assert lock_path.read_bytes() == first_lock, "a fresh lock is not rewritten (generated_at preserved)"
    assert _tree(root) == first_tree


def test_c007_update_without_upstream_change_keeps_the_lock_and_export_leaves_it(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    """C-007 across commands: export writes L; `update --marketplace` with no
    upstream change keeps L's bytes; a re-export touches neither bytes nor mtime."""
    runner = grim_at(work)
    _skill(f"{unique_repo}/a", "a")
    _write_marketplace(work / "marketplace.toml", {"team": {"include": [f"{registry}/{unique_repo}/a:1"]}})
    lock_path = work / "marketplace.lock"

    _ok(_export(runner, "--client", "claude", "-o", "dist"))
    first = lock_path.read_bytes()
    runner.run("update", "--marketplace", "marketplace.toml")
    assert lock_path.read_bytes() == first, "a no-op update leaves L byte-identical"

    mtime = lock_path.stat().st_mtime_ns
    _ok(_export(runner, "--client", "claude", "-o", "dist", "--force"))
    assert lock_path.read_bytes() == first
    assert lock_path.stat().st_mtime_ns == mtime, "a fresh lock is not rewritten"


def test_c010_changed_local_member_exits_65_until_update_marketplace(grim_at, work: Path) -> None:
    """C-010 / decision 42: a declared path member edited after the lock was
    written is 65 with the `grim update --marketplace <M> <P>` hint (not the
    grimoire.toml `grim update x` / `grim lock` one), and that command clears it."""
    runner = grim_at(work)
    local = work / "skills" / "team-plan"
    local.mkdir(parents=True)
    (local / "SKILL.md").write_text(_skill_md("team-plan", "Original"))
    _write_marketplace(work / "marketplace.toml", {"team": {"include": ["./skills/team-plan"]}})
    _ok(_export(runner, "--client", "claude", "-o", "dist"))

    (local / "SKILL.md").write_text(_skill_md("team-plan", "Edited"))
    result = _export(runner, "--client", "claude", "-o", "dist", "--force")
    assert result.returncode == 65, result.stderr
    err = _error(result)
    shown = f"{err['message']} {err.get('hint') or ''}"
    assert f"grim update --marketplace {work / 'marketplace.toml'} team" in shown, shown
    assert "grim lock" not in shown, shown

    runner.run("update", "--marketplace", "marketplace.toml", "team")
    _ok(_export(runner, "--client", "claude", "-o", "dist", "--force"))
    assert "# Edited" in (work / "dist" / "team.claude" / "skills" / "team-plan" / "SKILL.md").read_text()


# ── S-006 — Stale lock after an edit (C-004, C-033) ─────────────────────────


def test_s006_description_edit_keeps_lock_and_version(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str
) -> None:
    runner = grim_at(work)
    stack = _publish_stack(runner, tmp_path, registry, unique_repo)
    m = work / "marketplace.toml"
    lock_path = work / "marketplace.lock"
    _write_marketplace(m, {"team": {"include": [stack.bundle], "description": "First."}})
    _ok(_export(runner, "--plugin", "team", "--client", "claude", "-o", "dist"))
    lock_before = lock_path.read_bytes()
    version_before = _manifest(work / "dist" / "team.claude")["version"]

    _write_marketplace(m, {"team": {"include": [stack.bundle], "description": "Second."}})
    _ok(_export(runner, "--plugin", "team", "--client", "claude", "-o", "dist", "--force"))

    doc = _manifest(work / "dist" / "team.claude")
    assert lock_path.read_bytes() == lock_before
    assert doc["version"] == version_before
    assert doc["description"].startswith("Second. ")


def test_s006_include_edit_reresolves_and_rewrites_lock(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str
) -> None:
    runner = grim_at(work)
    stack = _publish_stack(runner, tmp_path, registry, unique_repo)
    extra_digest = _skill(f"{unique_repo}/extra", "extra")
    extra = f"{registry}/{unique_repo}/extra:1"
    m = work / "marketplace.toml"
    lock_path = work / "marketplace.lock"
    _write_marketplace(m, {"team": {"include": [stack.bundle]}})
    _ok(_export(runner, "--plugin", "team", "--client", "claude", "-o", "dist"))
    _, row_before = _lock_part(lock_path, "team")

    _write_marketplace(m, {"team": {"include": [stack.bundle, extra]}})
    _ok(_export(runner, "--plugin", "team", "--client", "claude", "-o", "dist", "--force"))

    entries, row_after = _lock_part(lock_path, "team")
    assert row_after["declaration_hash"] != row_before["declaration_hash"]
    assert any(e["name"] == "extra" and e["pinned"].endswith(extra_digest) for e in entries), entries
    assert (work / "dist" / "team.claude" / "skills" / "extra" / "SKILL.md").is_file()


# ── S-007 — Offline from a warm cache (C-017, C-030) ────────────────────────


@pytest.mark.xfail(
    strict=True,
    reason="offline export needs a manifest cache — deferred in adr_mcp_percall_scope_fetch_render.md; follow-up",
)
def test_s007_offline_export_from_warm_cache_is_byte_identical(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str
) -> None:
    runner = grim_at(work)
    stack = _publish_stack(runner, tmp_path, registry, unique_repo)
    _write_marketplace(work / "marketplace.toml", {"team": {"include": [stack.bundle]}})
    _ok(_export(runner, "--plugin", "team", "--client", "claude", "-o", "dist"))

    result = runner.run(
        "--offline", "export", "plugin", "--plugin", "team", "--client", "claude", "-o", "dist2", "--force",
        format="json", check=False,
    )
    _ok(result)
    assert _tree(work / "dist2" / "team.claude") == _tree(work / "dist" / "team.claude")


def test_s007_offline_with_cold_cache_exits_81(
    grim_at, grim_binary: Path, tmp_path: Path, work: Path, registry: str, unique_repo: str
) -> None:
    runner = grim_at(work)
    stack = _publish_stack(runner, tmp_path, registry, unique_repo)
    _write_marketplace(work / "marketplace.toml", {"team": {"include": [stack.bundle]}})
    _ok(_export(runner, "--plugin", "team", "--client", "claude", "-o", "dist"))
    cold_home = tmp_path / "cold" / "grim-home"
    cold_home.mkdir(parents=True)
    cold = GrimRunner(grim_binary, cold_home, cwd=work)

    declared = cold.run("--offline", "export", "plugin", "--plugin", "team", "--client", "claude", "-o", "dist2", check=False)
    assert declared.returncode == 81, declared.stderr
    adhoc = cold.run("--offline", "export", "plugin", stack.bundle, "--client", "claude", "-o", "dist3", check=False)
    assert adhoc.returncode == 81, adhoc.stderr
    assert _entries(work / "dist2") == [] and _entries(work / "dist3") == []


# ── S-008 — Selection errors (C-001, C-014, C-028) ──────────────────────────


def test_s008_unknown_plugin_exits_79(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    _skill(f"{unique_repo}/a", "a")
    _write_marketplace(work / "marketplace.toml", {"team": {"include": [f"{registry}/{unique_repo}/a:1"]}})

    result = _export(runner, "--plugin", "ghost", "--client", "claude", "-o", "dist")
    assert result.returncode == 79, result.stderr
    assert _entries(work / "dist") == []
    assert not (work / "marketplace.lock").exists()


def test_s008_no_declared_plugins_exits_65_with_hint(grim_at, work: Path) -> None:
    runner = grim_at(work)
    (work / "marketplace.toml").write_text("")

    result = _export(runner, "--client", "claude")
    assert result.returncode == 65, result.stderr
    err = _error(result)
    assert "--name" in f"{err['message']} {err.get('hint') or ''}"


def test_s008_missing_manifest_exits_65(grim_at, work: Path) -> None:
    runner = grim_at(work)

    result = _export(runner, "--plugin", "team", "--client", "claude", fmt=None)
    assert result.returncode == 65, result.stderr
    assert "not found" in result.stderr.lower()


@pytest.mark.parametrize("name", ["team.json", "grimoire.toml", "team.toml.toml"])
def test_s008_bad_manifest_path_exits_65(grim_at, work: Path, name: str) -> None:
    """C-001: the path name rule is checked before reading (the file exists)."""
    runner = grim_at(work)
    (work / name).write_text('[plugins.team]\ninclude = ["x"]\n')

    result = _export(runner, "--marketplace", name, "--plugin", "team", "--client", "claude")
    assert result.returncode == 65, result.stderr


@pytest.mark.parametrize(
    "args",
    [
        pytest.param(("--plugin", "team", "a:1"), id="plugin-with-refs"),
        pytest.param(("--marketplace", "marketplace.toml", "a:1"), id="marketplace-with-refs"),
        pytest.param(("--name", "x", "--plugin", "team"), id="name-with-plugin"),
        pytest.param(("--name", "x"), id="name-without-refs"),
    ],
)
def test_s008_usage_matrix_exits_64(grim_at, work: Path, args: tuple[str, ...]) -> None:
    runner = grim_at(work)
    (work / "marketplace.toml").write_text('[plugins.team]\ninclude = ["localhost:5000/x/a:1"]\n')

    result = _export(runner, *args, "--client", "claude", "-o", "dist")
    assert result.returncode == 64, result.stderr
    assert _entries(work / "dist") == []


def test_s008_reserved_top_level_key_exits_65(grim_at, work: Path) -> None:
    """C-001: `name`/`owner`/`description` are reserved at top level in phase 1."""
    runner = grim_at(work)
    (work / "marketplace.toml").write_text('owner = "me"\n[plugins.team]\ninclude = ["localhost:5000/x/a:1"]\n')

    result = _export(runner, "--plugin", "team", "--client", "claude")
    assert result.returncode == 65, result.stderr


# ── S-009 — Clients without a plugin format (C-015) ─────────────────────────


def _team_of_one_skill(work: Path, registry: str, unique_repo: str) -> None:
    _skill(f"{unique_repo}/a", "a")
    _write_marketplace(work / "marketplace.toml", {"team": {"include": [f"{registry}/{unique_repo}/a:1"]}})


def test_s009_explicit_client_without_plugin_format_exits_78(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    _team_of_one_skill(work, registry, unique_repo)

    result = _export(runner, "--plugin", "team", "--client", "opencode", "-o", "dist")
    assert result.returncode == 78, result.stderr
    assert _entries(work / "dist") == []


def test_s009_unknown_client_exits_78(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    _team_of_one_skill(work, registry, unique_repo)

    result = _export(runner, "--plugin", "team", "--client", "vscode", "-o", "dist")
    assert result.returncode == 78, result.stderr


@pytest.mark.parametrize(
    ("clients", "outputs", "note"),
    [
        pytest.param('["claude", "opencode"]', ["team.claude"], "opencode", id="drop-one"),
        pytest.param('["opencode"]', ["team.agents"], "opencode", id="fallback-to-agents"),
        pytest.param(None, ["team.agents"], None, id="no-config"),
    ],
)
def test_s009_config_clients_drop_formatless_ones(
    grim_at, work: Path, registry: str, unique_repo: str, clients: str | None, outputs: list[str], note: str | None
) -> None:
    runner = grim_at(work)
    _team_of_one_skill(work, registry, unique_repo)
    if clients is not None:
        (work / "grimoire.toml").write_text(f"[options]\nclients = {clients}\n")

    result = _export(runner, "--plugin", "team", "-o", "dist", fmt=None)
    assert result.returncode == 0, result.stderr
    assert _entries(work / "dist") == outputs
    if note:
        assert f"client '{note}' has no plugin format; skipped" in result.stderr


# ── S-010 — Per-client declines (C-016, C-020) ──────────────────────────────


def test_s010_per_client_declines(grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    stack = _publish_stack(runner, tmp_path, registry, unique_repo)

    _ok(_export(runner, stack.bundle, "--client", "droid,junie,claude", "-o", "dist"))

    dist = work / "dist"
    assert _entries(dist) == ["team-stack.claude", "team-stack.droid", "team-stack.junie"]
    for client in ("droid", "junie", "claude"):
        root = dist / f"team-stack.{client}"
        assert (root / "skills" / "team-plan" / "SKILL.md").is_file(), client
        assert not list(root.rglob("*team-style*")), f"{client}: rules have no plugin surface"
        _manifest(root)
    droid, junie, claude = (dist / f"team-stack.{c}" for c in ("droid", "junie", "claude"))
    assert not (droid / "agents").exists() and not (droid / ".mcp.json").exists()
    assert not (junie / "agents").exists()
    assert "team-srv" in json.loads((junie / ".mcp.json").read_text())["mcpServers"]
    assert (claude / "agents" / "team-reviewer.md").is_file()
    assert "team-srv" in json.loads((claude / ".mcp.json").read_text())["mcpServers"]
    assert "Omitted for this client: rule team-style, agent team-reviewer, mcp team-srv." in _manifest(droid)["description"]


# ── S-011 — Rendered like install (C-017, C-018, C-020) ─────────────────────


def _assert_same_bytes(exported_root: Path, installed_root: Path) -> None:
    files = [p for p in exported_root.rglob("*") if p.is_file()]
    assert files, f"nothing exported under {exported_root}"
    for p in files:
        rel = p.relative_to(exported_root)
        assert (installed_root / rel).read_bytes() == p.read_bytes(), f"{rel} differs from install"


@unix_only
@pytest.mark.parametrize("client", ["claude", "codex", "copilot", "junie", "openclaw"])
def test_s011_members_render_byte_equal_to_install(
    grim_at, grim_binary: Path, grim_home: Path, tmp_path: Path, work: Path, registry: str, unique_repo: str, client: str
) -> None:
    runner = grim_at(work)
    stack = _publish_stack(runner, tmp_path, registry, unique_repo)

    if client == "openclaw":
        # OpenClaw hosts skills only globally: install under the sandboxed HOME.
        # Skills only — an mcp no selected client can register fails install (65).
        write_config(grim_home, skills={n: stack.refs[n] for n in ("team-plan", "team-review")})
        installer = GrimRunner(grim_binary, grim_home)
        installer.run("lock", "--global")
        installer.json("install", "--global", "--client", client)
        skills_root = installer.home / ".openclaw" / "skills"
    else:
        project = tmp_path / "project"
        project.mkdir()
        write_config(project, bundles={"team-stack": stack.bundle})
        installer = grim_at(project)
        installer.run("lock")
        installer.json("install", "--client", client)
        skills_root = project / {"claude": ".claude", "copilot": ".github", "junie": ".junie"}.get(client, ".agents") / "skills"

    _ok(_export(runner, stack.bundle, "--client", client, "-o", "dist"))
    root = work / "dist" / f"team-stack.{client}"

    for name in ("team-plan", "team-review"):
        _assert_same_bytes(root / "skills" / name, skills_root / name)
        # Both directions: install writes nothing the export leaves out.
        assert _tree(root / "skills" / name) == _tree(skills_root / name), name
    # The renderer ran: the namespaced key never ships verbatim.
    plan = (root / "skills" / "team-plan" / "SKILL.md").read_text()
    assert plan != PLAN_MD and "claude.user-invocable" not in plan, plan
    assert ("user-invocable: false" in plan) == (client == "claude"), plan
    if client == "claude":
        agent = root / "agents" / "team-reviewer.md"
        assert "color: blue" in agent.read_text() and agent.read_text() != REVIEWER_MD
        assert agent.read_bytes() == (project / ".claude" / "agents" / "team-reviewer.md").read_bytes()
    # C-020: the Claude family's MCP value is the client's own install entry.
    if client in ("claude", "junie"):
        mcp_file = project / (".mcp.json" if client == "claude" else ".junie/mcp/mcp.json")
        exported = json.loads((root / ".mcp.json").read_text())["mcpServers"]
        installed = json.loads(mcp_file.read_text())["mcpServers"]
        assert set(exported) == {"team-srv"}
        assert exported["team-srv"] == installed["team-srv"]


# ── S-012 — Reproducible (C-026, C-030) ─────────────────────────────────────


def _run_under(runner: GrimRunner, cwd: Path, tz: str, umask: int, *args: str) -> None:
    env = {**runner.env, "TZ": tz}
    result = subprocess.run(
        [str(runner.binary), "export", "plugin", *args], cwd=cwd, env=env, umask=umask, capture_output=True, text=True, check=False
    )
    assert result.returncode == 0, result.stderr


@unix_only
def test_s012_outputs_are_reproducible_across_tz_umask_cwd_and_output_dir(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str
) -> None:
    runner = grim_at(work)
    stack = _annotated_stack(runner, tmp_path, registry, unique_repo)
    one, two = tmp_path / "one", tmp_path / "two"
    one.mkdir()
    two.mkdir()

    _run_under(runner, one, "UTC", 0o022, stack.bundle, "--client", "claude", "--zip", "-o", "out-a")
    _run_under(runner, two, "Pacific/Kiritimati", 0o077, stack.bundle, "--client", "claude", "--zip", "-o", str(tmp_path / "elsewhere"))
    assert _sha(one / "out-a" / "team-stack.claude.zip") == _sha(tmp_path / "elsewhere" / "team-stack.claude.zip")

    _run_under(runner, one, "UTC", 0o022, stack.bundle, "--client", "codex", "-o", "dir-a")
    _run_under(runner, two, "America/St_Johns", 0o077, stack.bundle, "--client", "codex", "-o", "nested/dir-b")
    first = _tree(one / "dir-a" / "team-stack.codex")
    assert first and first == _tree(two / "nested" / "dir-b" / "team-stack.codex")


# ── S-013 — Rename success (C-019, C-021, C-023) ────────────────────────────


RENAMED = {"team-plan": "plan", "team-review": "review", "team-reviewer": "reviewer", "team-style": "style", "team-srv": "srv"}


def test_s013_strip_prefix_renames_every_member(grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    stack = _publish_stack(runner, tmp_path, registry, unique_repo)
    _write_marketplace(work / "marketplace.toml", {"team": {"include": [stack.bundle], "strip_prefix": "team-"}})

    out = _ok(_export(runner, "--plugin", "team", "--client", "claude", "-o", "dist"))

    root = work / "dist" / "team.claude"
    assert sorted(p.name for p in (root / "skills").iterdir()) == ["plan", "review"]
    assert "name: plan\n" in (root / "skills" / "plan" / "SKILL.md").read_text()
    assert "name: reviewer\n" in (root / "agents" / "reviewer.md").read_text()
    assert not (root / "agents" / "team-reviewer.md").exists()
    assert set(json.loads((root / ".mcp.json").read_text())["mcpServers"]) == {"srv"}

    doc = _manifest(root)
    assert doc["version"] == f"0.0.0+{_suffix(stack.digests, RENAMED)}"
    assert _suffix(stack.digests, RENAMED) != _suffix(stack.digests)
    assert f"Omitted for this client: rule style. {ONRAMP}" in doc["description"]
    plan = next(m for m in out["items"][0]["members"] if m["name"] == "plan")
    assert (plan["kind"], plan["lock_name"]) == ("skill", "team-plan")


# ── S-014 — Rename empty/invalid (C-021) ────────────────────────────────────


@pytest.mark.parametrize("prefix", ["team-plan", "team"])
def test_s014_rename_to_empty_or_invalid_exits_65_and_writes_nothing(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str, prefix: str
) -> None:
    runner = grim_at(work)
    stack = _publish_stack(runner, tmp_path, registry, unique_repo)
    m = work / "marketplace.toml"
    _write_marketplace(m, {"team": {"include": [stack.bundle]}})
    _ok(_export(runner, "--plugin", "team", "--client", "claude", "-o", "dist"))
    lock_before = (work / "marketplace.lock").read_bytes()

    _write_marketplace(m, {"team": {"include": [stack.bundle], "strip_prefix": prefix}})
    result = _export(runner, "--plugin", "team", "--client", "claude", "-o", "dist2")
    assert result.returncode == 65, result.stderr
    assert _entries(work / "dist2") == []
    assert (work / "marketplace.lock").read_bytes() == lock_before


# ── S-015 — Rename collision (C-021) ────────────────────────────────────────


def test_s015_rename_collision_exits_65_naming_both(grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    stack = _publish_stack(runner, tmp_path, registry, unique_repo)
    _skill(f"{unique_repo}/plan", "plan")
    plan = f"{registry}/{unique_repo}/plan:1"
    _write_marketplace(work / "marketplace.toml", {"team": {"include": [stack.bundle, plan], "strip_prefix": "team-"}})

    result = _export(runner, "--plugin", "team", "--client", "claude", "-o", "dist", fmt=None)
    assert result.returncode == 65, result.stderr
    assert "team-plan" in result.stderr
    assert re.search(r"(?<![\w-])plan(?![\w-])", result.stderr), result.stderr
    assert _entries(work / "dist") == []
    assert not (work / "marketplace.lock").exists()


# ── S-016 — Stale reference (C-022) ─────────────────────────────────────────


def test_s016_stale_reference_after_rename_exits_65(grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    stack = _publish_stack(runner, tmp_path, registry, unique_repo, review_line7=REVIEW_STALE)
    _write_marketplace(work / "marketplace.toml", {"team": {"include": [stack.bundle], "strip_prefix": "team-"}})

    result = _export(runner, "--plugin", "team", "--client", "claude", "-o", "dist", fmt=None)
    assert result.returncode == 65, result.stderr
    assert "claude: skills/review/SKILL.md:7: 'team-plan'" in result.stderr
    assert _entries(work / "dist") == []
    assert not (work / "marketplace.lock").exists()


# ── S-017 — Overwrite policy (C-027) ────────────────────────────────────────


def test_s017_zip_overwrite_needs_force(grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    stack = _publish_stack(runner, tmp_path, registry, unique_repo)
    args = (stack.bundle, "--client", "claude", "--zip", "-o", "dist")
    _ok(_export(runner, *args))
    zpath = work / "dist" / "team-stack.claude.zip"
    real = _sha(zpath)
    zpath.write_bytes(b"old")

    refused = _export(runner, *args)
    assert refused.returncode == 65, refused.stderr
    err = _error(refused)
    assert err["reason"] == "untracked-destination" and err["forceable"] is True
    assert zpath.read_bytes() == b"old"
    assert _entries(work / "dist") == ["team-stack.claude.zip"], "no staging dir left behind"

    _ok(_export(runner, *args, "--force"))
    assert _sha(zpath) == real


def test_s017_directory_overwrite_needs_force(grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    stack = _publish_stack(runner, tmp_path, registry, unique_repo)
    root = work / "dist" / "team-stack.codex"
    root.mkdir(parents=True)
    (root / "stale.txt").write_text("old tree\n")
    args = (stack.bundle, "--client", "codex", "-o", "dist")

    refused = _export(runner, *args)
    assert refused.returncode == 65, refused.stderr
    assert _error(refused)["reason"] == "untracked-destination"
    assert (root / "stale.txt").read_text() == "old tree\n"
    assert _entries(work / "dist") == ["team-stack.codex"]

    _ok(_export(runner, *args, "--force"))
    assert not (root / "stale.txt").exists()
    _manifest(root)
    assert _entries(work / "dist") == ["team-stack.codex"]


@unix_only
@pytest.mark.parametrize(("client", "zip_flag"), [("claude", True), ("codex", False)], ids=["zip", "dir"])
def test_s017_symlink_at_output_is_replaced_and_target_untouched(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str, client: str, zip_flag: bool
) -> None:
    runner = grim_at(work)
    stack = _publish_stack(runner, tmp_path, registry, unique_repo)
    dist = work / "dist"
    dist.mkdir()
    if zip_flag:
        target = tmp_path / "target.zip"
        target.write_bytes(b"target")
        link = dist / f"team-stack.{client}.zip"
    else:
        target = tmp_path / "target-dir"
        target.mkdir()
        (target / "keep.txt").write_text("keep\n")
        link = dist / f"team-stack.{client}"
    link.symlink_to(target)
    args = [stack.bundle, "--client", client, "-o", "dist", *(["--zip"] if zip_flag else [])]

    refused = _export(runner, *args)
    assert refused.returncode == 65, refused.stderr
    assert link.is_symlink()

    _ok(_export(runner, *args, "--force"))
    assert not link.is_symlink()
    if zip_flag:
        assert link.is_file() and zipfile.is_zipfile(link)
        assert target.read_bytes() == b"target"
    else:
        assert (link / "plugin.json").is_file()
        assert _entries(target) == ["keep.txt"]


# ── S-018 — Empty plugin (C-027) ────────────────────────────────────────────


def test_s018_agent_only_plugin_for_agent_plugins_client_exits_65(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    make_artifact(
        f"{unique_repo}/solo",
        "agent",
        {"solo.md": "---\nname: solo\ndescription: Solo agent.\nmodel: sonnet\n---\n# Solo\nBody.\n"},
        tag="1",
    )

    result = _export(runner, f"{registry}/{unique_repo}/solo:1", "--client", "codex", "-o", "dist")
    assert result.returncode == 65, result.stderr
    assert _entries(work / "dist") == []


# ── S-026 — JSON shape (C-029, decision 24) ─────────────────────────────────


def test_s026_json_items_follow_client_order_with_sorted_members(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str
) -> None:
    runner = grim_at(work)
    stack = _publish_stack(runner, tmp_path, registry, unique_repo)

    items = _ok(_export(runner, stack.bundle, "--client", "droid,junie,claude", "-o", "dist"))["items"]

    assert [(i["plugin"], i["client"]) for i in items] == [
        ("team-stack", "droid"),
        ("team-stack", "junie"),
        ("team-stack", "claude"),
    ]
    def by_order(e: dict) -> tuple[int, str]:
        return KIND_ORDER[e["kind"]], e["name"]

    for item in items:
        assert set(item) == ITEM_KEYS, item
        assert item["family"] == "claude" and item["format"] == "dir"
        assert item["members"] == sorted(item["members"], key=by_order)
        assert item["omitted"] == sorted(item["omitted"], key=by_order)
        for m in item["members"]:
            assert set(m) == {"kind", "name", "lock_name", "pinned"}, m
            assert m["pinned"].endswith(stack.digests[(m["kind"], m["lock_name"])]), m
        for o in item["omitted"]:
            assert set(o) == {"kind", "name", "reason"}, o

    def shape(i: dict) -> tuple[list, list]:
        return [(m["kind"], m["name"]) for m in i["members"]], [(o["kind"], o["name"], o["reason"]) for o in i["omitted"]]

    skills = [("skill", "team-plan"), ("skill", "team-review")]
    assert shape(items[0]) == (
        skills,
        [
            ("rule", "team-style", "no-format-surface"),
            ("agent", "team-reviewer", "client-declined"),
            ("mcp", "team-srv", "client-declined"),
        ],
    )
    assert shape(items[1]) == (
        [*skills, ("mcp", "team-srv")],
        [("rule", "team-style", "no-format-surface"), ("agent", "team-reviewer", "client-declined")],
    )
    assert shape(items[2]) == (
        [*skills, ("agent", "team-reviewer"), ("mcp", "team-srv")],
        [("rule", "team-style", "no-format-surface")],
    )


# ── S-027 — Lock contention (C-010) ─────────────────────────────────────────


@unix_only
def test_s027_held_manifest_lock_exits_75(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    _team_of_one_skill(work, registry, unique_repo)

    with _held_flock(work / "marketplace.toml.lock"):
        result = _export(runner, "--plugin", "team", "--client", "claude", "-o", "dist")

    assert result.returncode == 75, result.stderr
    err = _error(result)
    assert err["reason"] == "locked" and err["retryable"] is True
    assert _entries(work / "dist") == []
    assert not (work / "marketplace.lock").exists()


# ── S-028 — Junie drops an OAuth MCP server (C-020) ─────────────────────────


def test_s028_junie_omits_oauth_mcp_as_not_representable(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str
) -> None:
    runner = grim_at(work)
    _skill(f"{unique_repo}/team-plan", "team-plan")
    authd = f"{registry}/{unique_repo}/authd:1.0.0"
    _release_mcp(runner, tmp_path / "src", authd, OAUTH_MCP)
    bundle = make_bundle(
        f"{unique_repo}/oauth-stack",
        [("skill", "team-plan", f"{registry}/{unique_repo}/team-plan:1"), ("mcp", "authd", authd)],
        tag="1",
    )

    item = _ok(_export(runner, bundle.fq, "--client", "junie", "-o", "dist"))["items"][0]

    root = work / "dist" / "oauth-stack.junie"
    mcp = root / ".mcp.json"
    # Decision 34: the only MCP member was declined, so no MCP file at all.
    assert not mcp.exists()
    assert {"kind": "mcp", "name": "authd", "reason": "not-representable"} in item["omitted"]
    assert "Omitted for this client: mcp authd." in _manifest(root)["description"]


# ── S-029 — Hand-edited lock cannot escape (C-006, C-035) ───────────────────


def test_s029_hand_edited_lock_name_is_refused_78(grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    stack = _publish_stack(runner, tmp_path, registry, unique_repo)
    _write_marketplace(work / "marketplace.toml", {"team": {"include": [stack.bundle]}})
    _ok(_export(runner, "--plugin", "team", "--client", "claude", "-o", "dist"))
    lock_path = work / "marketplace.lock"
    text = lock_path.read_text()
    assert 'name = "team-plan"' in text
    lock_path.write_text(text.replace('name = "team-plan"', 'name = "../evil"', 1))

    result = _export(runner, "--plugin", "team", "--client", "claude", "-o", "dist2")
    assert result.returncode == 78, result.stderr
    assert _entries(work / "dist2") == []
    assert not [p for p in tmp_path.rglob("*evil*") if p != lock_path]


# ── S-030 — Per-plugin staleness (C-033) ────────────────────────────────────


def test_s030_only_the_edited_plugin_is_reresolved(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    reg = f"{registry}/{unique_repo}"
    _skill(f"{unique_repo}/a-skill", "a-skill", heading="A1", tag="stable")
    _skill(f"{unique_repo}/b-skill", "b-skill", heading="B1", tag="stable")
    _skill(f"{unique_repo}/a2", "a2")
    m = work / "marketplace.toml"
    lock_path = work / "marketplace.lock"
    _write_marketplace(m, {"a": {"include": [f"{reg}/a-skill:stable"]}, "b": {"include": [f"{reg}/b-skill:stable"]}})
    _ok(_export(runner, "--client", "claude", "-o", "dist"))
    a_before, a_row_before = _lock_part(lock_path, "a")
    b_before = _lock_part(lock_path, "b")
    b_tree = _tree(work / "dist" / "b.claude")

    a_new = _skill(f"{unique_repo}/a-skill", "a-skill", heading="A2", tag="stable")
    _skill(f"{unique_repo}/b-skill", "b-skill", heading="B2", tag="stable")
    _write_marketplace(
        m, {"a": {"include": [f"{reg}/a-skill:stable", f"{reg}/a2:1"]}, "b": {"include": [f"{reg}/b-skill:stable"]}}
    )
    _ok(_export(runner, "--plugin", "a", "--client", "claude", "-o", "dist", "--force"))

    a_after, a_row_after = _lock_part(lock_path, "a")
    assert a_row_after["declaration_hash"] != a_row_before["declaration_hash"]
    assert {e["name"] for e in a_after} == {"a-skill", "a2"}
    assert next(e for e in a_after if e["name"] == "a-skill")["pinned"].endswith(a_new)
    assert a_after != a_before
    assert _lock_part(lock_path, "b") == b_before

    _ok(_export(runner, "--plugin", "b", "--client", "claude", "-o", "dist", "--force"))
    assert _tree(work / "dist" / "b.claude") == b_tree
    assert _lock_part(lock_path, "b") == b_before


def test_s030_plugin_removed_from_manifest_is_dropped_from_lock(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    """C-033: parts of plugins no longer in M are dropped on the next write,
    even when every exported plugin is fresh."""
    runner = grim_at(work)
    reg = f"{registry}/{unique_repo}"
    _skill(f"{unique_repo}/a-skill", "a-skill")
    _skill(f"{unique_repo}/b-skill", "b-skill")
    m = work / "marketplace.toml"
    lock_path = work / "marketplace.lock"
    _write_marketplace(m, {"a": {"include": [f"{reg}/a-skill:1"]}, "b": {"include": [f"{reg}/b-skill:1"]}})
    _ok(_export(runner, "--client", "claude", "-o", "dist"))
    a_before = _lock_part(lock_path, "a")

    _write_marketplace(m, {"a": {"include": [f"{reg}/a-skill:1"]}})
    _ok(_export(runner, "--plugin", "a", "--client", "claude", "-o", "dist", "--force"))

    lock = tomllib.loads(lock_path.read_text())
    assert [r["name"] for r in lock["plugin"]] == ["a"]
    assert not [e for k in LOCK_KINDS for e in lock.get(k, []) if e.get("plugin") == "b"]
    assert _lock_part(lock_path, "a") == a_before


# ── S-031 — Agent Plugins MCP projection (C-036) ────────────────────────────


NET_MCP = {
    "web": 'transport = "http"\nurl = "https://example.com/web"\ntimeout = 5000\n',
    "feed": 'transport = "sse"\nurl = "https://example.com/feed"\n',
    "sock": 'transport = "ws"\nurl = "wss://example.com/sock"\n',
    "authd": 'transport = "http"\nurl = "https://example.com/authd"\n\n[server.oauth]\nclient_id = "abc"\n',
    "hdr": 'transport = "http"\nurl = "https://example.com/hdr"\nheaders = { Authorization = "Bearer ${TOKEN}" }\n',
}


def test_s031_agent_plugins_mcp_projection(grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    refs = []
    for name, server in NET_MCP.items():
        ref = f"{registry}/{unique_repo}/{name}:1.0.0"
        _release_mcp(runner, tmp_path / "src", ref, f'description = "The {name} server."\n\n[server]\n{server}')
        refs.append(ref)

    items = _ok(_export(runner, *refs, "--name", "net", "--client", "agents,cursor", "-o", "dist"))["items"]

    assert [i["client"] for i in items] == ["agents", "cursor"], "agents is emitted though it declines MCP on install"
    for item in items:
        root = work / "dist" / f"net.{item['client']}"
        servers = json.loads((root / "mcp.json").read_text())["mcpServers"]
        assert servers == {
            "feed": {"type": "sse", "url": "https://example.com/feed"},
            "web": {"type": "streamable-http", "url": "https://example.com/web"},
        }
        assert item["omitted"] == [
            {"kind": "mcp", "name": n, "reason": "not-representable"} for n in ("authd", "hdr", "sock")
        ]
        assert "Omitted for this client: mcp authd, mcp hdr, mcp sock." in _manifest(root)["description"]


# ── S-032 — Description cap: 500 characters, on-ramp kept whole (C-024) ────

MAX_DESCRIPTION = 500
MAX_BASE = MAX_DESCRIPTION - len(ONRAMP) - 1


def test_s032_description_flag_overrides_the_annotation(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str
) -> None:
    runner = grim_at(work)
    stack = _annotated_stack(runner, tmp_path, registry, unique_repo)

    _ok(_export(runner, stack.bundle, "--client", "claude", "--description", "  Flag text. "))

    doc = _manifest(work / "team-stack.claude")
    assert doc["description"] == f"Flag text. Omitted for this client: rule team-style. {ONRAMP}"


@pytest.mark.parametrize("where", ["flag", "declared"])
def test_s032_authored_description_over_the_cap_exits_65_and_writes_nothing(
    grim_at, work: Path, registry: str, unique_repo: str, where: str
) -> None:
    runner = grim_at(work)
    _skill(f"{unique_repo}/a", "a")
    ref = f"{registry}/{unique_repo}/a:1"
    long = "x" * (MAX_BASE + 1)
    if where == "flag":
        result = _export(runner, ref, "--client", "claude", "--description", long)
    else:
        _write_marketplace(work / "marketplace.toml", {"team": {"include": [ref], "description": long}})
        result = _export(runner, "--client", "claude")

    assert result.returncode == 65, result.stderr
    assert f"at most {MAX_BASE} fit" in _error(result)["message"]
    assert _entries(work) == ([] if where == "flag" else ["marketplace.toml"])


def test_s032_authored_description_at_the_cap_fits_exactly(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    _skill(f"{unique_repo}/a", "a")
    base = "x" * MAX_BASE

    _ok(_export(runner, f"{registry}/{unique_repo}/a:1", "--client", "claude", "--description", base))

    assert _manifest(work / "a.claude")["description"] == f"{base} {ONRAMP}"


def test_s032_long_annotation_is_cut_with_ellipsis_and_warned(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str
) -> None:
    """A publisher's text is outside the exporter's control: cut, never refused."""
    runner = grim_at(work)
    long = "Everything a repository needs. " * 30
    stack = _publish_stack(
        runner, tmp_path, registry, unique_repo, annotations={"org.opencontainers.image.description": long}
    )

    result = _export(runner, stack.bundle, "--client", "claude")
    _ok(result)

    desc = _manifest(work / "team-stack.claude")["description"]
    assert len(desc.encode("utf-16-le")) // 2 <= MAX_DESCRIPTION
    assert desc.startswith("Everything a repository needs.")
    assert desc.endswith(f"… Omitted for this client: rule team-style. {ONRAMP}")
    assert "description cut to 500 characters" in result.stderr


# ── S-033 — `--progress` reports every member fetched (C-027) ───────────────


def test_s033_progress_json_counts_members_across_plugins(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    runner = grim_at(work)
    refs = []
    for name in ("a", "b", "c"):
        _skill(f"{unique_repo}/{name}", name)
        refs.append(f"{registry}/{unique_repo}/{name}:1")
    _write_marketplace(work / "marketplace.toml", {"one": {"include": refs[:1]}, "two": {"include": refs[1:]}})

    result = runner.run("--progress", "json", "export", "plugin", "--client", "claude", check=False)
    assert result.returncode == 0, result.stderr

    events = [json.loads(line) for line in result.stderr.splitlines() if line.startswith("{")]
    assert events[0] == {"event": "start", "total": 3}
    advances = [e for e in events if e["event"] == "advance"]
    assert [(e["position"], e["total"]) for e in advances] == [(1, 3), (2, 3), (3, 3)]
    assert [e["label"] for e in advances] == ["one: skill a", "two: skill b", "two: skill c"]
    assert events[-1] == {"event": "finish"}
