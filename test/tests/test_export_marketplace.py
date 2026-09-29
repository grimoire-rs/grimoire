# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim export marketplace` acceptance tests (plan contracts C-012, C-021; scenarios S-001–S-018, S-020, S-022, S-030).

`grim export marketplace` statelessly regenerates a marketplace repository from
the plugins `marketplace.toml` declares and its `[marketplace]` table: one
marketplace file per client at the harness's own path, one plugin tree per
plugin and client at `./<client>/<plugin>`, and `marketplace.lock` beside the
manifest. The repo root defaults to the manifest's directory, so `work` below is
the whole repository.

Fixture: skills `a` and `b` published as `<repo>/a:1` and `<repo>/b:1`; plugins
`alpha` (skill a) and `beta` (skill b) declared in `marketplace.toml`.
"""
from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest
import tomllib

from src.helpers import held_flock, make_artifact, tree_suffix
from src.runner import GrimRunner

unix_only = pytest.mark.skipif(sys.platform == "win32", reason="needs a Unix host (flock, symlinks, chmod)")
windows_only = pytest.mark.skipif(sys.platform != "win32", reason="needs a Windows host (directory junctions)")

DEFAULT_CLIENTS = ("claude", "copilot", "codex", "qoder")
# ADR D2: each harness's own marketplace file.
FILES = {
    "claude": ".claude-plugin/marketplace.json",
    "copilot": ".github/plugin/marketplace.json",
    "codex": ".agents/plugins/marketplace.json",
    "qoder": ".qoder-plugin/marketplace.json",
    "cursor": ".cursor-plugin/marketplace.json",
}
# The manifest a rendered tree carries its version in, per client.
TREE_MANIFEST = {
    "claude": ".claude-plugin/plugin.json",
    "qoder": ".qoder-plugin/plugin.json",
    "copilot": "plugin.json",
    "codex": "plugin.json",
    "cursor": "plugin.json",
}
ITEM_KEYS = ["plugin", "client", "family", "path", "version", "action", "members", "omitted"]
FILE_KEYS = ["client", "path", "action", "plugins"]
SVG = b'<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"/>\n'
AGENT_MD = "---\nname: helper\ndescription: Helps.\nmodel: sonnet\n---\n# Helper\nAgent body.\n"


# ── fixtures and helpers ──────────────────────────────────────────────────


@pytest.fixture()
def work(tmp_path: Path) -> Path:
    """The repository root and cwd — the manifest's directory."""
    d = tmp_path / "work"
    d.mkdir()
    return d


def _skill_md(name: str, heading: str = "Demo") -> str:
    return f"---\nname: {name}\ndescription: Demo skill.\n---\n# {heading}\n\nBody.\n"


def _skill(repo: str, name: str, heading: str = "Demo", extra: dict[str, str] | None = None) -> str:
    """Publish a skill at `<repo>:1`; returns its digest."""
    files = {f"{name}/SKILL.md": _skill_md(name, heading), **(extra or {})}
    return make_artifact(repo, "skill", files, tag="1").digest


def _refs(registry: str, unique_repo: str, *names: str) -> dict[str, str]:
    """Publish one skill per name; `name -> fully-qualified ref`."""
    out = {}
    for name in names:
        _skill(f"{unique_repo}/{name}", name)
        out[name] = f"{registry}/{unique_repo}/{name}:1"
    return out


def _toml(
    plugins: dict[str, dict],
    *,
    table: bool = True,
    name: str = "acme",
    clients: list[str] | None = None,
    table_extra: str = "",
    owner: str = 'name = "Acme"',
) -> str:
    """A manifest: `[marketplace]` (unless `table=False`) then `{plugin: {"include": [...], "description"?: str, ...}}`."""
    parts = []
    if table:
        head = f'[marketplace]\nname = "{name}"\n'
        if clients is not None:
            head += f"clients = {json.dumps(clients)}\n"
        parts.append(head + table_extra + f"\n[marketplace.owner]\n{owner}\n")
    for plugin, decl in plugins.items():
        lines = [f"[plugins.{plugin}]"]
        for key in ("include", "description", "version", "logo", "project"):
            if key in decl:
                lines.append(f"{key} = {json.dumps(decl[key])}")
        parts.append("\n".join(lines) + "\n")
    return "\n".join(parts)


def _write(work: Path, plugins: dict[str, dict], **kw) -> Path:
    path = work / "marketplace.toml"
    path.write_text(_toml(plugins, **kw))
    return path


def _two_plugins(work: Path, registry: str, unique_repo: str, **kw) -> dict[str, str]:
    refs = _refs(registry, unique_repo, "a", "b")
    _write(work, {"alpha": {"include": [refs["a"]]}, "beta": {"include": [refs["b"]]}}, **kw)
    return refs


def _run(runner: GrimRunner, *args: str, fmt: str | None = "json") -> subprocess.CompletedProcess[str]:
    return runner.run("export", "marketplace", *args, format=fmt, check=False)


def _ok(result: subprocess.CompletedProcess[str]) -> dict:
    assert result.returncode == 0, f"rc={result.returncode}\nstdout: {result.stdout}\nstderr: {result.stderr}"
    return json.loads(result.stdout)


def _error(result: subprocess.CompletedProcess[str]) -> dict:
    return json.loads(result.stdout)["error"]


def _refuse(result: subprocess.CompletedProcess[str], code: int = 65) -> dict:
    assert result.returncode == code, f"rc={result.returncode}\nstdout: {result.stdout}\nstderr: {result.stderr}"
    return _error(result)


def _actions(report: dict) -> dict[tuple[str, str], str]:
    return {(i["plugin"], i["client"]): i["action"] for i in report["items"]}


def _file_actions(report: dict) -> dict[str, str]:
    return {f["client"]: f["action"] for f in report["files"]}


def _doc(work: Path, client: str) -> dict:
    return json.loads((work / FILES[client]).read_text())


def _tree_manifest(work: Path, client: str, plugin: str) -> dict:
    return json.loads((work / client / plugin / TREE_MANIFEST[client]).read_text())


def _files(root: Path) -> dict[str, str]:
    """`relpath -> sha256` of every regular file under `root`."""
    out = {}
    for dirpath, _dirs, names in os.walk(root):
        for n in names:
            p = Path(dirpath) / n
            out[p.relative_to(root).as_posix()] = hashlib.sha256(p.read_bytes()).hexdigest()
    return out


def _snapshot(root: Path) -> dict[str, tuple[str, int]]:
    """`relpath -> (sha256, mtime_ns)` of everything a run may write, `.grim-export*` bookkeeping aside."""
    out = {}
    for dirpath, _dirs, names in os.walk(root):
        for n in names:
            p = Path(dirpath) / n
            rel = p.relative_to(root).as_posix()
            if rel.startswith(".grim-export"):
                continue
            out[rel] = (hashlib.sha256(p.read_bytes()).hexdigest(), p.stat().st_mtime_ns)
    return out


def _written_paths(work: Path) -> list[str]:
    """Every path a marketplace run creates: nothing but the manifest may exist after a refusal."""
    return sorted(p for p in _files(work) if p != "marketplace.toml" and not p.startswith(".grim-export"))


def _junction(link: Path, target: Path) -> None:
    """A directory junction `link -> target` (Windows; needs no privilege, unlike a symlink)."""
    subprocess.run(["cmd", "/c", "mklink", "/J", str(link), str(target)], check=True, capture_output=True)


def _export_plugin(runner: GrimRunner, plugin: str, client: str, out: str) -> subprocess.CompletedProcess[str]:
    return runner.run(
        "export", "plugin", "--marketplace", "marketplace.toml", "--plugin", plugin, "--client", client, "-o", out,
        format="json", check=False,
    )  # fmt: skip


# ── C-012 — the command line ──────────────────────────────────────────────


@pytest.mark.parametrize(
    "extra",
    [
        pytest.param(["ghcr.io/acme/a:1"], id="positional-ref"),
        pytest.param(["--name", "x"], id="name"),
        pytest.param(["--project"], id="project"),
        pytest.param(["--plugin", "alpha"], id="plugin"),
        pytest.param(["--zip"], id="zip"),
        pytest.param(["--version", "1.0.0"], id="version"),
        pytest.param(["--description", "d"], id="description"),
        pytest.param(["--logo", "l.svg"], id="logo"),
        pytest.param(["--client", "claude"], id="client"),
    ],
)
def test_c012_clap_rejects_the_export_plugin_inputs_with_64(grim_at, work: Path, extra: list[str]) -> None:
    result = _run(grim_at(work), *extra)
    assert result.returncode == 64, result.stderr
    assert _written_paths(work) == []


def test_c012_default_manifest_is_marketplace_toml_in_the_cwd(grim_at, work: Path) -> None:
    result = _run(grim_at(work))
    err = _refuse(result)
    assert "marketplace.toml" in err["message"] and "not found" in err["message"], err


def test_c012_missing_marketplace_table_is_65_naming_the_table(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    _two_plugins(work, registry, unique_repo, table=False)

    err = _refuse(_run(grim_at(work)))

    assert "[marketplace]" in err["message"], err
    assert _written_paths(work) == []


def test_c012_default_output_root_is_the_manifest_directory(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str
) -> None:
    repo = work / "sub"
    repo.mkdir()
    refs = _refs(registry, unique_repo, "a")
    _write(repo, {"alpha": {"include": [refs["a"]]}})

    _ok(_run(grim_at(work), "--marketplace", "sub/marketplace.toml"))

    assert (repo / FILES["claude"]).is_file() and (repo / "claude" / "alpha").is_dir()
    assert (repo / "marketplace.lock").is_file()
    assert sorted(p.name for p in work.iterdir()) == ["sub"], "nothing lands in the cwd"


def test_c012_output_flag_moves_the_repo_root_but_not_the_lock(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    _two_plugins(work, registry, unique_repo)

    _ok(_run(grim_at(work), "-o", "repo"))

    assert (work / "repo" / FILES["claude"]).is_file() and (work / "repo" / "claude" / "alpha").is_dir()
    assert not (work / "claude").exists() and not (work / FILES["claude"]).exists()
    assert (work / "marketplace.lock").is_file(), "the lock stays beside the manifest"
    assert not (work / "repo" / "marketplace.lock").exists()


def test_c012_plain_output_is_one_table_and_one_stderr_line_per_file(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    _two_plugins(work, registry, unique_repo)
    runner = grim_at(work)

    first = _run(runner, fmt=None)

    assert first.returncode == 0, first.stderr
    assert first.stdout.split()[:5] == ["Plugin", "Client", "Version", "Action", "Omitted"], first.stdout
    assert len(first.stdout.strip().splitlines()) == 1 + 8, "header plus one row per (plugin, client)"
    lines = first.stderr.strip().splitlines()
    assert [ln for ln in lines if ln.startswith("wrote ")] == [
        f"wrote {(work / FILES[c]).resolve()} (2 plugins)" for c in DEFAULT_CLIENTS
    ], first.stderr

    second = _run(runner, fmt=None)
    assert [ln.split()[0] for ln in second.stderr.strip().splitlines()] == ["unchanged"] * 4, second.stderr


# ── S-001 — first export into an empty repo (C-013, C-019) ────────────────


def test_s001_first_export_writes_files_trees_and_lock(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    _two_plugins(work, registry, unique_repo)

    report = _ok(_run(grim_at(work)))

    assert list(report) == ["items", "files"]
    assert [(i["plugin"], i["client"]) for i in report["items"]] == [
        (p, c) for p in ("alpha", "beta") for c in DEFAULT_CLIENTS
    ], "plugins by name, then client selection order"
    assert all(list(i) == ITEM_KEYS for i in report["items"])
    assert all(i["action"] == "written" and i["members"] for i in report["items"])
    assert [(f["client"], f["action"], f["plugins"]) for f in report["files"]] == [
        (c, "written", ["alpha", "beta"]) for c in DEFAULT_CLIENTS
    ]
    assert all(list(f) == FILE_KEYS for f in report["files"])
    families = {i["client"]: i["family"] for i in report["items"]}
    assert families == {"claude": "claude", "qoder": "claude", "copilot": "agent-plugins", "codex": "agent-plugins"}

    for client in DEFAULT_CLIENTS:
        raw = (work / FILES[client]).read_bytes()
        assert raw.endswith(b"}\n") and not raw.endswith(b"\n\n")
        doc = json.loads(raw)
        assert list(doc) == ["name", "owner", "plugins"], "no metadata without a description"
        assert doc["name"] == "acme" and doc["owner"] == {"name": "Acme"}
        for entry, plugin in zip(doc["plugins"], ("alpha", "beta"), strict=True):
            assert list(entry) == ["name", "source", "version", "description"]
            assert entry["name"] == plugin
            assert entry["source"] == f"./{client}/{plugin}" and "../" not in entry["source"]
            tree = _tree_manifest(work, client, plugin)
            assert (entry["version"], entry["description"]) == (tree["version"], tree["description"]), client
            assert entry["version"] == f"0.0.0+{tree_suffix(work / client / plugin, '0.0.0')}"
        assert (work / client / "alpha" / "README.md").is_file()
    for item in report["items"]:
        assert Path(item["path"]) == (work / item["client"] / item["plugin"]).resolve()
    assert (work / "marketplace.lock").is_file()


def test_s001_table_metadata_reaches_every_document(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    refs = _refs(registry, unique_repo, "a")
    _write(
        work,
        {"alpha": {"include": [refs["a"]]}},
        table_extra='description = "Acme plugins."\n',
        owner='name = "Acme"\nemail = "team@acme.example"',
    )

    _ok(_run(grim_at(work)))

    for client in DEFAULT_CLIENTS:
        doc = _doc(work, client)
        assert list(doc) == ["name", "owner", "metadata", "plugins"]
        assert doc["owner"] == {"name": "Acme", "email": "team@acme.example"}
        assert doc["metadata"] == {"description": "Acme plugins."}


# ── S-002 — quiet re-run (C-020) ──────────────────────────────────────────


def test_s002_quiet_rerun_changes_nothing(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    _two_plugins(work, registry, unique_repo)
    runner = grim_at(work)
    _ok(_run(runner))
    before = _snapshot(work)

    report = _ok(_run(runner))

    assert set(_actions(report).values()) == {"unchanged"} and len(report["items"]) == 8
    assert set(_file_actions(report).values()) == {"unchanged"}
    assert _snapshot(work) == before, "no owned path's bytes or mtime changed, lock included"


# ── S-003 — member bump ───────────────────────────────────────────────────


def test_s003_member_bump_rewrites_only_that_plugin(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    _two_plugins(work, registry, unique_repo)
    runner = grim_at(work)
    _ok(_run(runner))
    versions = {(p, c): _tree_manifest(work, c, p)["version"] for p in ("alpha", "beta") for c in DEFAULT_CLIENTS}
    beta_before = _files(work / "claude" / "beta")

    _skill(f"{unique_repo}/a", "a", heading="Bumped")
    runner.run("update", "--marketplace", "marketplace.toml")
    report = _ok(_run(runner))

    actions = _actions(report)
    for c in DEFAULT_CLIENTS:
        assert actions[("alpha", c)] == "written" and actions[("beta", c)] == "unchanged", c
        assert _tree_manifest(work, c, "alpha")["version"] != versions[("alpha", c)]
        assert _tree_manifest(work, c, "beta")["version"] == versions[("beta", c)]
        assert [e["version"] for e in _doc(work, c)["plugins"]][0] == _tree_manifest(work, c, "alpha")["version"]
    assert set(_file_actions(report).values()) == {"written"}
    assert _files(work / "claude" / "beta") == beta_before


# ── S-004 — description edit ──────────────────────────────────────────────


def test_s004_description_edit_changes_every_client_version_and_keeps_the_lock(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    refs = _two_plugins(work, registry, unique_repo)
    runner = grim_at(work)
    _ok(_run(runner))
    lock = (work / "marketplace.lock").read_bytes()
    before = {c: _tree_manifest(work, c, "alpha")["version"] for c in DEFAULT_CLIENTS}

    _write(
        work,
        {"alpha": {"include": [refs["a"]], "description": "Alpha, reworded."}, "beta": {"include": [refs["b"]]}},
    )
    report = _ok(_run(runner))

    for c in DEFAULT_CLIENTS:
        assert _actions(report)[("alpha", c)] == "written" and _actions(report)[("beta", c)] == "unchanged"
        assert _tree_manifest(work, c, "alpha")["version"] != before[c]
        assert _doc(work, c)["plugins"][0]["description"].startswith("Alpha, reworded.")
    assert (work / "marketplace.lock").read_bytes() == lock, "a description edit re-pins nothing"


# ── S-005 — plugin dropped ────────────────────────────────────────────────


def test_s005_dropped_plugin_is_removed_from_trees_and_documents(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    refs = _two_plugins(work, registry, unique_repo)
    runner = grim_at(work)
    _ok(_run(runner))

    _write(work, {"alpha": {"include": [refs["a"]]}})
    report = _ok(_run(runner))

    assert [(i["plugin"], i["client"], i["action"]) for i in report["items"]] == [
        *(("alpha", c, "unchanged") for c in DEFAULT_CLIENTS),
        *(("beta", c, "removed") for c in sorted(DEFAULT_CLIENTS)),
    ], "live rows first, then removed rows by (client, plugin) bytes"
    for i in report["items"]:
        if i["action"] == "removed":
            assert i["version"] is None and i["members"] == [] and i["omitted"] == [] and list(i) == ITEM_KEYS
            assert Path(i["path"]) == (work / i["client"] / "beta").resolve()
    for c in DEFAULT_CLIENTS:
        assert not (work / c / "beta").exists()
        assert [e["name"] for e in _doc(work, c)["plugins"]] == ["alpha"]


# ── S-006 — client dropped ────────────────────────────────────────────────


@pytest.mark.parametrize("holds_another_file", [True, False], ids=["github-holds-a-file", "github-otherwise-empty"])
def test_s006_dropped_client_loses_its_file_and_directory_but_never_dot_github(
    grim_at, work: Path, registry: str, unique_repo: str, holds_another_file: bool
) -> None:
    refs = _two_plugins(work, registry, unique_repo)
    runner = grim_at(work)
    _ok(_run(runner))
    if holds_another_file:
        (work / ".github" / "CODEOWNERS").write_text("* @acme\n")
    kept = _files(work / "claude")

    _write(work, {"alpha": {"include": [refs["a"]]}, "beta": {"include": [refs["b"]]}}, clients=["claude", "codex", "qoder"])
    report = _ok(_run(runner))

    assert _file_actions(report) == {"claude": "unchanged", "codex": "unchanged", "qoder": "unchanged", "copilot": "removed"}
    assert report["files"][-1] == {
        "client": "copilot",
        "path": str((work / FILES["copilot"]).resolve()),
        "action": "removed",
        "plugins": [],
    }, "removed rows sort last"
    assert not (work / FILES["copilot"]).exists() and not (work / "copilot").exists()
    assert (work / ".github").is_dir(), ".github/ itself is never pruned"
    assert (work / ".github" / "CODEOWNERS").is_file() is holds_another_file
    assert _files(work / "claude") == kept


# ── S-007 — hand-edited tree ──────────────────────────────────────────────


@unix_only
def test_s007_hand_edited_tree_is_rewritten_and_the_stray_removed(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    _two_plugins(work, registry, unique_repo)
    runner = grim_at(work)
    _ok(_run(runner))
    tree = work / "claude" / "alpha"
    pristine = _files(tree)
    skill = tree / "skills" / "a" / "SKILL.md"
    skill.write_text(skill.read_text().replace("Body.", "Bodx."))
    (tree / "stray.txt").write_text("left behind\n")
    readme = tree / "README.md"
    readme.chmod(0o755)

    report = _ok(_run(runner))

    assert _actions(report)[("alpha", "claude")] == "written"
    assert {a for (p, c), a in _actions(report).items() if (p, c) != ("alpha", "claude")} == {"unchanged"}
    assert _files(tree) == pristine and not (tree / "stray.txt").exists()
    assert not readme.stat().st_mode & 0o111, "the exec bit is gone with the rest of the edit"


# ── S-008 — foreign ./claude/, no file ────────────────────────────────────


def test_s008_foreign_client_directory_without_a_file_is_refused_then_adopted(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    _two_plugins(work, registry, unique_repo)
    (work / "claude").mkdir()
    (work / "claude" / "notes.txt").write_text("mine\n")
    runner = grim_at(work)

    err = _refuse(_run(runner))

    assert err["reason"] == "untracked-destination" and err["forceable"] is True, err
    assert "output already exists" in err["message"] and str(work / "claude") in err["message"]
    assert _written_paths(work) == ["claude/notes.txt"], "nothing written"

    _ok(_run(runner, "--force"))
    assert (work / FILES["claude"]).is_file() and (work / "claude" / "alpha").is_dir()
    assert not (work / "claude" / "notes.txt").exists(), "adoption replaces what the render set does not name"


@unix_only
def test_s008_unselected_client_directory_without_a_file_survives(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    _two_plugins(work, registry, unique_repo, clients=["claude"])
    (work / "cursor").mkdir()
    (work / "cursor" / "mine.txt").write_text("mine\n")

    _ok(_run(grim_at(work)))

    assert (work / "cursor" / "mine.txt").read_text() == "mine\n"


# ── S-009 — renamed marketplace ───────────────────────────────────────────


def test_s009_renamed_marketplace_is_refused_naming_both_then_adopted(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    refs = _two_plugins(work, registry, unique_repo)
    runner = grim_at(work)
    _ok(_run(runner))
    before = _files(work)

    _write(work, {"alpha": {"include": [refs["a"]]}, "beta": {"include": [refs["b"]]}}, name="acme-two")
    err = _refuse(_run(runner))

    assert err["reason"] == "untracked-destination", err
    assert err["forceable"] is True
    assert _files(work) == {**before, "marketplace.toml": _files(work)["marketplace.toml"]}, "nothing written"

    _ok(_run(runner, "--force"))
    assert {_doc(work, c)["name"] for c in DEFAULT_CLIENTS} == {"acme-two"}


# ── S-010 — bad [marketplace] table, three commands alike ─────────────────

BAD_TABLES = [
    pytest.param({"name": "Bad_Name"}, "invalid", id="bad-name"),
    pytest.param({"name": "github"}, "reserved", id="reserved-name"),
    pytest.param({"name": "my-qoder-hub"}, "qoder", id="qoder-substring"),
    pytest.param({"owner": 'name = "Acme"\nemail = "not-an-email"'}, "email", id="bad-email"),
    pytest.param({"clients": ["junie"]}, "select claude", id="served-by-claude-hint"),
    pytest.param({"clients": []}, "select at least one client", id="empty-clients"),
    pytest.param({"clients": ["vim"]}, "unknown client", id="unknown-client"),
]


@pytest.mark.parametrize(("table_kw", "needle"), BAD_TABLES)
@pytest.mark.parametrize("command", ["export-marketplace", "export-plugin", "update-marketplace"])
def test_s010_bad_marketplace_table_is_65_from_every_command(
    grim_at, work: Path, registry: str, unique_repo: str, command: str, table_kw: dict, needle: str
) -> None:
    refs = _refs(registry, unique_repo, "a")
    _write(work, {"alpha": {"include": [refs["a"]]}}, **table_kw)
    runner = grim_at(work)

    if command == "export-marketplace":
        result = _run(runner)
    elif command == "export-plugin":
        result = _export_plugin(runner, "alpha", "claude", "dist")
    else:
        result = runner.run("update", "--marketplace", "marketplace.toml", format="json", check=False)

    err = _refuse(result)
    assert "[marketplace]" in err["message"] and needle in err["message"], err
    assert not (work / "marketplace.lock").exists() and not (work / "dist").exists()


# ── S-011 — phase-1 manifest, no table ────────────────────────────────────


def test_s011_manifest_without_the_table_behaves_as_before(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    refs = _refs(registry, unique_repo, "a")
    plugins = {"alpha": {"include": [refs["a"]]}}
    _write(work, plugins, table=False)
    runner = grim_at(work)

    plain = _ok(_export_plugin(runner, "alpha", "claude", "dist"))
    lock = (work / "marketplace.lock").read_bytes()
    tree = _files(work / "dist" / "alpha.claude")
    updated = runner.json("update", "--marketplace", "marketplace.toml")
    assert (work / "marketplace.lock").read_bytes() == lock, "update leaves an unchanged lock byte-identical"

    _write(work, plugins)  # adding [marketplace] must not change a phase-1 command
    with_table = _ok(_export_plugin(runner, "alpha", "claude", "dist2"))
    updated_with_table = runner.json("update", "--marketplace", "marketplace.toml")

    assert (work / "marketplace.lock").read_bytes() == lock, "adding [marketplace] never re-pins (C-007)"
    assert _files(work / "dist2" / "alpha.claude") == tree
    assert [i["version"] for i in with_table["items"]] == [i["version"] for i in plain["items"]]
    assert list(with_table["items"][0]) == list(plain["items"][0])
    assert updated_with_table == updated
    assert not (work / "claude").exists(), "export plugin never writes a marketplace layout"


# ── S-012 — symlinks on the way to an owned path ──────────────────────────


@unix_only
@pytest.mark.parametrize("where", ["client-dir", "github-plugin-dir", "tree-root"])
def test_s012_symlinked_owned_path_is_refused_and_nothing_is_written(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str, where: str
) -> None:
    _two_plugins(work, registry, unique_repo)
    runner = grim_at(work)
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    (elsewhere / "keep.txt").write_text("keep\n")
    if where == "client-dir":
        (work / "claude").symlink_to(elsewhere, target_is_directory=True)
    elif where == "github-plugin-dir":
        (work / ".github").mkdir()
        (work / ".github" / "plugin").symlink_to(elsewhere, target_is_directory=True)
    else:
        _ok(_run(runner))
        (work / "claude" / "alpha").rename(tmp_path / "moved-alpha")
        (work / "claude" / "alpha").symlink_to(elsewhere, target_is_directory=True)
    before = _snapshot(work)

    err = _refuse(_run(runner))

    assert "unsafe entry path" in err["message"], err
    assert _snapshot(work) == before, "nothing written"
    assert _files(elsewhere) == {"keep.txt": hashlib.sha256(b"keep\n").hexdigest()}
    if where != "tree-root":
        assert not (work / "marketplace.lock").exists()


@unix_only
@pytest.mark.parametrize("which", ["lock", "manifest"])
def test_s012_symlinked_manifest_or_lock_is_refused_and_the_target_is_untouched(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str, which: str
) -> None:
    _two_plugins(work, registry, unique_repo)
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    if which == "lock":
        target = elsewhere / "victim.lock"
        target.write_text("victim\n")
        (work / "marketplace.lock").symlink_to(target)
    else:
        target = elsewhere / "real.toml"
        (work / "marketplace.toml").replace(target)
        (work / "marketplace.toml").symlink_to(target)
    before = target.read_bytes()

    err = _refuse(_run(grim_at(work)))

    assert "symbolic link" in err["message"], err
    assert target.read_bytes() == before, "the link was not followed"
    assert [p for p in _written_paths(work) if p != "marketplace.lock"] == []


@windows_only
@pytest.mark.parametrize("where", ["client-dir", "github-plugin-dir", "tree-root"])
def test_s012_junction_on_an_owned_path_is_refused_and_nothing_is_written(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str, where: str
) -> None:
    """The Windows twin of the symlink case: a junction is a reparse point `check_chain` must refuse."""
    _two_plugins(work, registry, unique_repo)
    runner = grim_at(work)
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    (elsewhere / "keep.txt").write_bytes(b"keep\n")  # write_text would store CRLF on Windows
    if where == "client-dir":
        _junction(work / "claude", elsewhere)
    elif where == "github-plugin-dir":
        (work / ".github").mkdir()
        _junction(work / ".github" / "plugin", elsewhere)
    else:
        _ok(_run(runner))
        (work / "claude" / "alpha").rename(tmp_path / "moved-alpha")
        _junction(work / "claude" / "alpha", elsewhere)
    before = _snapshot(work)

    err = _refuse(_run(runner))

    assert "unsafe entry path" in err["message"], err
    assert _snapshot(work) == before, "nothing written"
    assert _files(elsewhere) == {"keep.txt": hashlib.sha256(b"keep\n").hexdigest()}
    if where != "tree-root":
        assert not (work / "marketplace.lock").exists()


@windows_only
def test_s012_stray_junction_under_an_owned_directory_is_unlinked_and_its_target_survives(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str
) -> None:
    _two_plugins(work, registry, unique_repo)
    runner = grim_at(work)
    _ok(_run(runner))
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    (elsewhere / "keep.txt").write_text("keep\n")
    _junction(work / "claude" / "stray", elsewhere)

    _ok(_run(runner))

    assert not (work / "claude" / "stray").exists(), "the junction itself is removed"
    assert (elsewhere / "keep.txt").read_text() == "keep\n", "removal never follows it into the target"


# ── S-013 — unsafe names in a rendered tree ───────────────────────────────


@pytest.mark.parametrize(
    "extra",
    [
        pytest.param({"a/aux.md": "reserved\n"}, id="windows-reserved-name"),
        pytest.param({"a/Notes.md": "upper\n", "a/notes.md": "lower\n"}, id="case-collision"),
    ],
)
def test_s013_unsafe_name_in_a_rendered_tree_is_65_and_writes_nothing(
    grim_at, work: Path, registry: str, unique_repo: str, extra: dict[str, str]
) -> None:
    _skill(f"{unique_repo}/a", "a", extra=extra)
    _write(work, {"alpha": {"include": [f"{registry}/{unique_repo}/a:1"]}})

    err = _refuse(_run(grim_at(work)))

    assert "unsafe entry path" in err["message"], err
    assert _written_paths(work) == []


# ── S-014 — escaping inputs, on the real binary ───────────────────────────


@unix_only
@pytest.mark.parametrize("case", ["logo-outside", "logo-symlink", "path-member-outside", "project-outside"])
def test_s014_inputs_outside_the_manifest_directory_or_symlinked_are_65(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str, case: str
) -> None:
    refs = _refs(registry, unique_repo, "a")
    decl: dict = {"include": [refs["a"]]}
    if case == "logo-outside":
        (tmp_path / "x.svg").write_bytes(SVG)
        decl["logo"] = "../x.svg"
    elif case == "logo-symlink":
        (work / "real.svg").write_bytes(SVG)
        (work / "logo.svg").symlink_to("real.svg")
        decl["logo"] = "logo.svg"
    elif case == "path-member-outside":
        skill = tmp_path / "outside" / "local-skill"
        skill.mkdir(parents=True)
        (skill / "SKILL.md").write_text(_skill_md("local-skill"))
        decl = {"include": ["../outside/local-skill"]}
    else:
        proj = tmp_path / "proj"
        proj.mkdir()
        (proj / "grimoire.toml").write_text(f'[skills]\na = "{refs["a"]}"\n')
        grim_at(proj).run("lock")
        decl = {"project": "../proj"}
    _write(work, {"alpha": decl})

    err = _refuse(_run(grim_at(work)))

    expected = "logo" if case.startswith("logo") else "outside"
    assert expected in err["message"], err
    assert [p for p in _written_paths(work) if p not in ("real.svg", "logo.svg")] == []
    assert not (work / "marketplace.lock").exists()


@pytest.mark.parametrize(
    "case",
    [
        "path-include-under-claude",
        "logo-under-cursor",
        "project-under-codex",
        "project-member-under-claude",
        "project-logo-under-cursor",
    ],
)
def test_s014_input_under_an_owned_directory_is_65_and_survives(
    grim_at, work: Path, registry: str, unique_repo: str, case: str
) -> None:
    """The run would delete the input as a stray in the same run (R2-22, C-012a)."""
    refs = _refs(registry, unique_repo, "a")
    decl: dict = {"include": [refs["a"]]}
    if case == "path-include-under-claude":
        skill = work / "claude" / "local-skill"
        skill.mkdir(parents=True)
        (skill / "SKILL.md").write_text(_skill_md("local-skill"))
        decl = {"include": ["./claude/local-skill"]}
        survivor = skill / "SKILL.md"
    elif case == "logo-under-cursor":
        (work / "cursor").mkdir()
        survivor = work / "cursor" / "logo.svg"
        survivor.write_bytes(SVG)
        decl["logo"] = "./cursor/logo.svg"
    elif case == "project-under-codex":
        proj = work / "codex" / "proj"
        proj.mkdir(parents=True)
        survivor = proj / "grimoire.toml"
        survivor.write_text(f'[skills]\na = "{refs["a"]}"\n')
        grim_at(proj).run("lock")
        decl = {"project": "./codex/proj"}
    else:
        # What the project's own lock and `[plugin]` table name, not the manifest entry.
        proj = work / "proj"
        (proj / "local").mkdir(parents=True)
        (proj / "local" / "SKILL.md").write_text(_skill_md("local"))
        if case == "project-member-under-claude":
            (work / "claude" / "local").mkdir(parents=True)
            survivor = work / "claude" / "local" / "SKILL.md"
            survivor.write_text(_skill_md("local"))
            (proj / "grimoire.toml").write_text('[skills]\nlocal = "../claude/local"\n')
        else:
            (work / "cursor").mkdir()
            survivor = work / "cursor" / "logo.svg"
            survivor.write_bytes(SVG)
            (proj / "grimoire.toml").write_text(
                '[skills]\nlocal = "./local"\n\n[plugin]\nlogo = "../cursor/logo.svg"\n'
            )
        grim_at(proj).run("lock")
        decl = {"project": "./proj"}
    _write(work, {"alpha": decl})
    before = survivor.read_bytes()

    err = _refuse(_run(grim_at(work), "--force"))

    assert "which `grim export marketplace` owns" in err["message"], err
    assert survivor.read_bytes() == before
    assert not (work / ".claude-plugin").exists() and not (work / "marketplace.lock").exists()


@windows_only
def test_s014_junction_as_a_logo_ancestor_is_65(
    grim_at, tmp_path: Path, work: Path, registry: str, unique_repo: str
) -> None:
    """The Windows twin of `logo-symlink`: `contain_input` must see a junction as a link."""
    refs = _refs(registry, unique_repo, "a")
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    (elsewhere / "logo.svg").write_bytes(SVG)
    _junction(work / "assets", elsewhere)
    _write(work, {"alpha": {"include": [refs["a"]], "logo": "./assets/logo.svg"}})

    err = _refuse(_run(grim_at(work)))

    assert "symbolic link" in err["message"], err
    assert (elsewhere / "logo.svg").read_bytes() == SVG
    assert not (work / "marketplace.lock").exists()


def test_c015_reserved_plugin_name_is_65_and_writes_nothing(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    refs = _refs(registry, unique_repo, "a")
    _write(work, {"con": {"include": [refs["a"]]}})

    err = _refuse(_run(grim_at(work)))

    assert "unsafe entry path" in err["message"] and "con" in err["message"], err
    assert _written_paths(work) == []


# ── S-015 — empty for one client ──────────────────────────────────────────


def test_s015_agent_only_plugin_is_empty_for_agent_plugins_clients(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    refs = _refs(registry, unique_repo, "a")
    make_artifact(f"{unique_repo}/helper", "agent", {"helper.md": AGENT_MD}, tag="1")
    _write(
        work,
        {"alpha": {"include": [refs["a"]]}, "bot": {"include": [f"{registry}/{unique_repo}/helper:1"]}},
    )

    result = _run(grim_at(work))
    report = _ok(result)

    actions = _actions(report)
    assert {c: actions[("bot", c)] for c in DEFAULT_CLIENTS} == {
        "claude": "written", "copilot": "empty", "codex": "empty", "qoder": "written",
    }  # fmt: skip
    for row in (i for i in report["items"] if i["action"] == "empty"):
        assert row["version"] is None and row["members"] == []
        assert [(o["kind"], o["name"]) for o in row["omitted"]] == [("agent", "helper")]
        assert Path(row["path"]) == (work / row["client"] / "bot").resolve(), "the would-be tree path"
        assert not Path(row["path"]).exists()
    assert "bot" in result.stderr, "one stderr warning per empty (plugin, client)"
    for c in ("claude", "qoder"):
        assert [e["name"] for e in _doc(work, c)["plugins"]] == ["alpha", "bot"]
        assert (work / c / "bot").is_dir()
    for c in ("copilot", "codex"):
        assert [e["name"] for e in _doc(work, c)["plugins"]] == ["alpha"]
    assert {f["client"]: f["plugins"] for f in report["files"]} == {
        "claude": ["alpha", "bot"], "qoder": ["alpha", "bot"], "copilot": ["alpha"], "codex": ["alpha"],
    }  # fmt: skip


def test_s015_rules_only_plugin_is_65_empty_plugin(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    make_artifact(f"{unique_repo}/style", "rule", {"style.md": "# Style\nUse tabs.\n"}, tag="1")
    _write(work, {"rules": {"include": [f"{registry}/{unique_repo}/style:1"]}})

    err = _refuse(_run(grim_at(work)))

    assert "plugin 'rules' has no member" in err["message"], err
    assert _written_paths(work) == []


# ── S-016 — zero plugins ──────────────────────────────────────────────────


def test_s016_zero_plugins_write_empty_documents_and_no_lock(grim_at, work: Path) -> None:
    _write(work, {})

    report = _ok(_run(grim_at(work)))

    assert report["items"] == []
    assert {f["client"]: f["action"] for f in report["files"]} == dict.fromkeys(DEFAULT_CLIENTS, "written")
    for c in DEFAULT_CLIENTS:
        assert _doc(work, c)["plugins"] == []
    assert not (work / "marketplace.lock").exists()


def test_s016_dropping_every_plugin_removes_the_prior_trees(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    _two_plugins(work, registry, unique_repo)
    runner = grim_at(work)
    _ok(_run(runner))

    _write(work, {})
    report = _ok(_run(runner))

    assert {a for a in _actions(report).values()} == {"removed"} and len(report["items"]) == 8
    for c in DEFAULT_CLIENTS:
        assert not (work / c).exists()
        assert _doc(work, c)["plugins"] == []


# ── S-017 — concurrent runs on one root ───────────────────────────────────


@unix_only
def test_s017_held_output_root_lock_exits_75(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    _two_plugins(work, registry, unique_repo)

    with held_flock(work / ".grim-export.lock"):
        err = _refuse(_run(grim_at(work)), 75)

    assert err["reason"] == "locked" and err["retryable"] is True, err
    assert _written_paths(work) == []


@unix_only
def test_s017_held_manifest_lock_exits_75(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    _two_plugins(work, registry, unique_repo)

    with held_flock(work / "marketplace.toml.lock"):
        err = _refuse(_run(grim_at(work)), 75)

    assert err["reason"] == "locked", err
    assert _written_paths(work) == []


# ── S-018 — byte equality with export plugin ──────────────────────────────


@pytest.mark.parametrize("client", DEFAULT_CLIENTS)
def test_s018_each_tree_equals_export_plugin_output(
    grim_at, work: Path, registry: str, unique_repo: str, client: str
) -> None:
    _two_plugins(work, registry, unique_repo)
    runner = grim_at(work)
    _ok(_run(runner))

    _ok(_export_plugin(runner, "alpha", client, "dist"))

    assert _files(work / client / "alpha") == _files(work / "dist" / f"alpha.{client}")


# ── S-020 (second half) — cursor is opt-in ────────────────────────────────


def test_s020_cursor_is_written_only_when_selected(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    refs = _refs(registry, unique_repo, "a")
    _write(work, {"alpha": {"include": [refs["a"]]}})
    runner = grim_at(work)
    _ok(_run(runner))
    assert not (work / FILES["cursor"]).exists() and not (work / "cursor").exists(), "not in the default set"

    _write(work, {"alpha": {"include": [refs["a"]]}}, clients=[*DEFAULT_CLIENTS, "cursor"])
    report = _ok(_run(runner))

    assert _file_actions(report)["cursor"] == "written"
    doc = _doc(work, "cursor")
    assert doc["name"] == "acme" and [e["source"] for e in doc["plugins"]] == ["./cursor/alpha"]
    tree = work / "cursor" / "alpha"
    second = json.loads((tree / ".cursor-plugin" / "plugin.json").read_text())
    assert list(second) == ["name", "version", "description"]
    assert second["version"] == doc["plugins"][0]["version"] == json.loads((tree / "plugin.json").read_text())["version"]


# ── S-022 — shadowing warnings (C-022) ────────────────────────────────────


def test_s022_factory_marketplace_file_warns_and_exits_0(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    _two_plugins(work, registry, unique_repo)
    (work / ".factory-plugin").mkdir()
    (work / ".factory-plugin" / "marketplace.json").write_text("{}\n")

    result = _run(grim_at(work))

    _ok(result)
    assert str(Path(".factory-plugin/marketplace.json")) in result.stderr, result.stderr
    assert (work / ".factory-plugin" / "marketplace.json").read_text() == "{}\n", "never touched"


def test_s022_unowned_qoder_file_with_qoder_unselected_warns(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    _two_plugins(work, registry, unique_repo, clients=["claude"])
    (work / ".qoder-plugin").mkdir()
    (work / ".qoder-plugin" / "marketplace.json").write_text('{"name": "other"}\n')

    result = _run(grim_at(work))

    _ok(result)
    assert str(Path(".qoder-plugin/marketplace.json")) in result.stderr, result.stderr


# ── S-030 — offline ───────────────────────────────────────────────────────


def test_s030_offline_with_an_absent_member_exits_81_and_writes_nothing(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    _two_plugins(work, registry, unique_repo)

    result = grim_at(work).run("--offline", "export", "marketplace", format="json", check=False)

    _refuse(result, 81)
    assert _written_paths(work) == []
