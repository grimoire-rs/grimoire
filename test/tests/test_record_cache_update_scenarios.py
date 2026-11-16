# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""``grim update`` proving dropped files against the store, the registry and the disk.

A bare update proves the previous lock's nodes before it saves the new lock. The
old pin's content comes from the store, then the registry; a host that fails is
tried once. Content the run cannot reach, a pin that is gone, a path source that
drifted, files that are not the render, and links leave the files on disk, named
once. Update halves of ``research_install_record_cache_test_matrix.md`` S30,
S32, S39, S40, S42, S48, S52, S53, S54, S57, E07 to E11, E14, E15, E22 and E23,
plus ADR validation cases 13 to 16 and 23.
"""

from __future__ import annotations

import json
import shutil
import socket
import sys
from collections.abc import Callable
from pathlib import Path

import pytest
import tomllib

from src.assertions import assert_not_exists, assert_symlink_exists
from src.helpers import make_artifact, make_bundle, write_config
from src.oci_proxy import OciProxy, serve_proxy
from src.registry import REGISTRY_HOST, fetch_manifest, retag, tag_digest
from src.runner import GrimRunner
from src.scenarios import (
    Team,
    allow_insecure,
    assert_unchanged,
    delete_manifest,
    fresh_runner,
    git,
    gone_pin,
    move_tag,
    state_snapshot,
)

EDIT = "# local edit\n"

no_links = pytest.mark.skipif(
    sys.platform == "win32",
    reason="symbolic links need a privilege Windows runners lack",
)


# ---------------------------------------------------------------------------
# Observation
# ---------------------------------------------------------------------------


def _go(runner: GrimRunner, *args: str):
    """Run ``grim --format json`` and return ``(exit, parsed stdout, stderr)``."""
    result = runner.run(*args, format="json", check=False)
    try:
        doc = json.loads(result.stdout)
    except json.JSONDecodeError:
        doc = {}
    return result.returncode, doc, result.stderr


def _actions(doc: dict) -> dict[str, str]:
    return {row["name"]: row["action"] for row in doc.get("items", [])}


def _digest(pinned: str) -> str:
    return pinned.rsplit("@", 1)[1]


def _records(state: Path) -> dict[str, dict]:
    if not state.exists():
        return {}
    doc = json.loads(state.read_text(encoding="utf-8"))
    return {r["name"]: r for r in doc["records"]}


def _record_pins(state: Path) -> dict[str, str]:
    return {name: _digest(r["pinned"]) for name, r in _records(state).items()}


def _lock_pins(lock: Path) -> dict[str, str]:
    doc = tomllib.loads(lock.read_text(encoding="utf-8"))
    rows = [*doc.get("skill", []), *doc.get("rule", [])]
    return {row["name"]: _digest(row["pinned"]) for row in rows}


def _state(work: Path) -> Path:
    return work / ".grimoire" / "state.json"


def _skill(work: Path, name: str, client: str = "claude") -> Path:
    folder = {"claude": ".claude", "copilot": ".github", "cursor": ".cursor"}[client]
    return work / folder / "skills" / name


def _rule(work: Path, name: str) -> Path:
    return work / ".claude" / "rules" / f"{name}.md"


def _text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def _posix(text: str) -> str:
    return text.replace("\\", "/")


def _left_lines(stderr: str, name: str) -> list[str]:
    """The stderr lines naming ``'name'`` that say files were left on disk."""
    return [
        line
        for line in stderr.splitlines()
        if f"'{name}'" in line and "on disk" in line
    ]


def _left_line(stderr: str, kind: str, name: str, *paths: str) -> str:
    """The one left-on-disk line for ``name``, checked for its kind and each path."""
    (line,) = _left_lines(stderr, name)
    assert line.startswith(kind) or f"{kind} '{name}'" in line, line
    for path in paths:
        assert path in _posix(line), (path, line)
    return line


def _tree(root: Path) -> dict[str, bytes]:
    return {
        p.relative_to(root).as_posix(): p.read_bytes()
        for p in sorted(root.rglob("*"))
        if p.is_file() and not p.is_symlink()
    }


# ---------------------------------------------------------------------------
# World: Alice declares, installs, commits and pushes; Bob clones with no state
# ---------------------------------------------------------------------------


def _shared(
    team: Callable[[str], Team],
    declare: Callable[[Path], None],
    model: str = "CO",
    insecure: tuple[str, ...] = (),
) -> Team:
    """Bob holds Alice's commit, the files it carries and no install state."""
    t = team(model)
    for member in (t.alice, t.bob):
        allow_insecure(member.runner, *insecure)
    declare(t.alice.work)
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: declare")
    t.alice.push()
    t.bob.pull()
    assert not _state(t.bob.work).exists()
    return t


def _options(clients: tuple[str, ...]) -> dict[str, str] | None:
    return {"clients": json.dumps(list(clients))} if clients else None


def _helper(version: str = "v1") -> dict[str, str]:
    return {"helper/SKILL.md": f"# helper {version}\n"}


def _extra(version: str = "v1") -> dict[str, str]:
    return {
        "extra/SKILL.md": f"# extra {version}\n",
        "extra/ref.md": f"ref {version}\n",
    }


Member = tuple[str, str, str]


def _bundle(
    repo: str, members: list[Member], *, host: str = REGISTRY_HOST, move: bool
) -> None:
    """Publish ``stack`` with ``(kind, name, artifact:tag)`` members; a move keeps its old manifest."""
    if move:
        old = tag_digest(f"{repo}/stack", "rolling")
        retag(f"{repo}/stack", f"keep-{old.removeprefix('sha256:')[:12]}", old)
    refs = [(kind, name, f"{host}/{repo}/{art}") for kind, name, art in members]
    make_bundle(f"{repo}/stack", refs, tag="rolling")


def _stack(
    repo: str,
    artifacts: dict[str, tuple[str, dict[str, str]]],
    members: list[Member],
    *,
    host: str = REGISTRY_HOST,
    clients: tuple[str, ...] = (),
) -> Callable[[Path], None]:
    """Declare bundle ``stack`` over ``artifacts`` (``art:tag`` to kind and files)."""

    def declare(work: Path) -> None:
        for art, (kind, files) in artifacts.items():
            name, tag = art.split(":")
            make_artifact(f"{repo}/{name}", kind, files, tag=tag)
        _bundle(repo, members, host=host, move=False)
        write_config(
            work,
            bundles={"stack": f"{host}/{repo}/stack:rolling"},
            options=_options(clients),
        )

    return declare


PAIR = {"helper:v1": ("skill", _helper()), "extra:v1": ("skill", _extra())}
BOTH: list[Member] = [("skill", "helper", "helper:v1"), ("skill", "extra", "extra:v1")]
HELPER_ONLY: list[Member] = [("skill", "helper", "helper:v1")]


def _drop_declaration(work: Path, *names: str) -> None:
    """Take ``names`` out of ``grimoire.toml`` by hand, as an edit before an update does."""
    config = work / "grimoire.toml"
    lines = [
        line
        for line in _text(config).splitlines()
        if not any(line.startswith(f"{name} = ") for name in names)
    ]
    config.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")


def _blob_reads(proxy: OciProxy, name: str) -> int:
    return sum(n for path, n in proxy.counts.items() if f"/{name}/blobs/" in path)


def _remove_blob(home: Path, digest: str) -> None:
    hex_digest = digest.removeprefix("sha256:")
    (home / "blobs" / "sha256" / hex_digest[:2] / hex_digest).unlink()


# ===========================================================================
# Store and network: where the old pin's content comes from
# ===========================================================================


def test_a_cold_clone_update_fetches_the_old_pin_and_removes_what_it_drops(
    team, unique_repo, oci_proxy
):
    # research_install_record_cache_test_matrix.md S52; ADR validation case 15
    proxy: OciProxy = oci_proxy("count")
    t = _shared(
        team, _stack(unique_repo, PAIR, BOTH, host=proxy.host), insecure=(proxy.host,)
    )
    work = t.bob.work
    _bundle(unique_repo, HELPER_ONLY, host=proxy.host, move=True)
    before = _blob_reads(proxy, "extra")

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert rc == 0, stderr
    assert _blob_reads(proxy, "extra") > before
    assert _actions(doc)["extra"] == "removed"
    assert_not_exists(_skill(work, "extra"))
    assert _left_lines(stderr, "extra") == []
    assert _record_pins(_state(work)) == _lock_pins(work / "grimoire.lock")


def _closed_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def test_an_update_tries_a_host_that_is_down_once_and_names_both_drops(
    team, unique_repo
):
    # research_install_record_cache_test_matrix.md S54; ADR validation case 15
    def declare(work: Path) -> None:
        refs = {}
        for name in ("gone-a", "gone-b", "kept"):
            make_artifact(
                f"{unique_repo}/{name}",
                "skill",
                {f"{name}/SKILL.md": f"# {name}\n"},
                tag="v1",
            )
            refs[name] = f"{REGISTRY_HOST}/{unique_repo}/{name}:v1"
        write_config(work, skills=refs)

    t = _shared(team, declare)
    work = t.bob.work
    with serve_proxy(upstream=f"127.0.0.1:{_closed_port()}") as down:
        allow_insecure(t.bob.runner, down.host)
        # Both dropped rows now name a host that accepts and drops every request.
        lock = work / "grimoire.lock"
        lock.write_text(
            _text(lock).replace(
                f"{REGISTRY_HOST}/{unique_repo}/gone-",
                f"{down.host}/{unique_repo}/gone-",
            ),
            encoding="utf-8",
            newline="\n",
        )
        _drop_declaration(work, "gone-a", "gone-b")

        rc, doc, stderr = _go(t.bob.runner, "update")
        counts = down.counts

    assert rc == 0, stderr
    assert [path for path in counts if "/gone-a/" in path], counts
    assert not [path for path in counts if "/gone-b/" in path], counts
    for name in ("gone-a", "gone-b"):
        assert _skill(work, name).is_dir()
        _left_line(stderr, "skill", name, f".claude/skills/{name}")
    assert "failed earlier" in _left_lines(stderr, "gone-b")[0]
    assert _actions(doc)["kept"] == "unchanged"


def _gone_world(team: Callable[[str], Team], repo: str) -> Team:
    def declare(work: Path) -> None:
        refs = {}
        for name in ("gone", "kept"):
            make_artifact(
                f"{repo}/{name}", "skill", {f"{name}/SKILL.md": f"# {name}\n"}, tag="v1"
            )
            refs[name] = f"{REGISTRY_HOST}/{repo}/{name}:v1"
        write_config(work, skills=refs)

    return _shared(team, declare)


@pytest.mark.parametrize(
    ("command", "gone"),
    [("update", "rewritten"), ("update", "deleted"), ("uninstall", "rewritten")],
)
def test_an_old_pin_the_registry_lacks_leaves_its_files_named(
    team, unique_repo, command, gone
):
    # research_install_record_cache_test_matrix.md S53; ADR validation case 16
    t = _gone_world(team, unique_repo)
    work = t.bob.work
    lock = work / "grimoire.lock"
    if gone == "rewritten":
        gone_pin(lock, "gone")
    else:
        delete_manifest(f"{unique_repo}/gone", _lock_pins(lock)["gone"])
    files = _tree(_skill(work, "gone"))

    if command == "update":
        _drop_declaration(work, "gone")
        rc, doc, stderr = _go(t.bob.runner, "update")
        assert "gone" not in _actions(doc)
    else:
        rc, doc, stderr = _go(t.bob.runner, "uninstall", "skill", "gone")
        assert doc["status"] == "uninstalled"

    assert rc == 0, stderr
    assert _tree(_skill(work, "gone")) == files
    _left_line(stderr, "skill", "gone", ".claude/skills/gone")
    assert "gone" not in _records(_state(work))


def _local_doc(name: str, version: str) -> str:
    return f"---\nname: {name}\ndescription: Local.\n---\n# {name} {version}\n"


def test_an_update_names_the_files_of_a_path_source_that_drifted(team):
    # ADR validation case 16: a path source whose files no longer hash to the lock
    def declare(work: Path) -> None:
        for name in ("local", "kept"):
            source = work / "skills" / name / "SKILL.md"
            source.parent.mkdir(parents=True, exist_ok=True)
            source.write_text(_local_doc(name, "v1"), encoding="utf-8", newline="\n")
        write_config(work, skills={"local": "./skills/local", "kept": "./skills/kept"})

    t = _shared(team, declare)
    work = t.bob.work
    (work / "skills" / "local" / "SKILL.md").write_text(
        _local_doc("local", "v2"), encoding="utf-8", newline="\n"
    )
    _drop_declaration(work, "local")
    files = _tree(_skill(work, "local"))

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert rc == 0, stderr
    assert _tree(_skill(work, "local")) == files
    _left_line(stderr, "skill", "local", ".claude/skills/local")
    assert _actions(doc)["kept"] == "unchanged"


@pytest.mark.parametrize("missing", ["old", "new"])
def test_an_offline_update_names_old_files_it_cannot_read_and_needs_new_content(
    team, unique_repo, tmp_path, missing
):
    # research_install_record_cache_test_matrix.md S57; ADR validation case 16.
    # Bob's store holds the bundle's next version only, read by a project of his own.
    t = _shared(team, _stack(unique_repo, PAIR, BOTH))
    work = t.bob.work
    make_artifact(
        f"{unique_repo}/fresh", "skill", {"fresh/SKILL.md": "# fresh\n"}, tag="v1"
    )
    then = [
        *HELPER_ONLY,
        *([("skill", "fresh", "fresh:v1")] if missing == "new" else []),
    ]
    _bundle(unique_repo, then, move=True)
    scratch = tmp_path / "scratch"
    (scratch / ".claude").mkdir(parents=True)
    write_config(
        scratch, bundles={"stack": f"{REGISTRY_HOST}/{unique_repo}/stack:rolling"}
    )
    primer = GrimRunner(t.bob.runner.binary, t.bob.runner.grim_home, cwd=scratch)
    primer.run("lock")
    primer.run("install")
    if missing == "new":
        layer = fetch_manifest(f"{unique_repo}/fresh", "v1")["layers"][0]["digest"]
        _remove_blob(t.bob.runner.grim_home, layer)
    files = _tree(_skill(work, "extra"))

    rc, doc, stderr = _go(t.bob.runner, "--offline", "update")

    if missing == "new":
        assert rc == 81, stderr
        return
    assert rc == 0, stderr
    assert _tree(_skill(work, "extra")) == files
    _left_line(stderr, "skill", "extra", ".claude/skills/extra")
    assert _actions(doc)["helper"] == "unchanged"
    assert "extra" not in _records(_state(work))


# ===========================================================================
# Global scope: the evidence lock and the state live in $GRIM_HOME
# ===========================================================================


def _global_world(grim: GrimRunner, repo: str, *, stow: bool) -> Path:
    """Declare skill ``solo`` and rule ``kept`` at global scope and install them.

    With ``stow`` the skills folder is a link into a dotfiles tree, as a stow
    or yadm user has it. Returns the global ``.claude`` root.
    """
    root = grim.home / ".claude"
    root.mkdir(parents=True, exist_ok=True)
    if stow:
        dotfiles = grim.home / "dotfiles" / "skills"
        dotfiles.mkdir(parents=True)
        (root / "skills").symlink_to(dotfiles, target_is_directory=True)
    make_artifact(f"{repo}/solo", "skill", {"solo/SKILL.md": "# solo\n"}, tag="v1")
    make_artifact(f"{repo}/kept", "rule", {"kept.md": "# kept\n"}, tag="v1")
    write_config(
        grim.grim_home,
        skills={"solo": f"{REGISTRY_HOST}/{repo}/solo:v1"},
        rules={"kept": f"{REGISTRY_HOST}/{repo}/kept:v1"},
    )
    grim.run("--global", "lock")
    grim.run("--global", "install")
    assert (root / "skills" / "solo" / "SKILL.md").is_file()
    return root


@pytest.mark.parametrize("stow", [False, True], ids=["plain", "stow"])
def test_a_global_update_without_its_state_removes_a_dropped_skill(
    grim, unique_repo, stow
):
    # ADR validation case 23; research_install_record_cache_test_matrix.md E11 (update half)
    if stow and sys.platform == "win32":
        pytest.skip("symbolic links need a privilege Windows runners lack")
    root = _global_world(grim, unique_repo, stow=stow)
    (grim.grim_home / "state" / "global.json").unlink()
    _drop_declaration(grim.grim_home, "solo")
    solo = root / "skills" / "solo"
    files = _tree(solo)

    rc, doc, stderr = _go(grim, "--global", "update")

    assert rc == 0, stderr
    assert _actions(doc)["kept"] == "unchanged"
    if not stow:
        assert _actions(doc)["solo"] == "removed"
        assert_not_exists(solo)
        assert _left_lines(stderr, "solo") == []
        return
    # A link on an ancestor below the global root is never proven: the files stay.
    assert "solo" not in _actions(doc)
    assert _tree(solo) == files
    line = _left_line(stderr, "skill", "solo", ".claude/skills/solo")
    assert "by hand" not in line
    assert "remove the link" not in line
    assert_symlink_exists(root / "skills")


@pytest.mark.parametrize("stow", [False, True], ids=["plain", "stow"])
def test_a_global_install_after_a_dotfiles_pull(grim, unique_repo, tmp_path, stow):
    # research_install_record_cache_test_matrix.md S48, E11 (install half)
    if stow and sys.platform == "win32":
        pytest.skip("symbolic links need a privilege Windows runners lack")
    root = _global_world(grim, unique_repo, stow=stow)
    move_tag(f"{unique_repo}/solo", {"solo/SKILL.md": "# solo v2\n"}, tag="v1")
    alice = fresh_runner(tmp_path, "alice")
    (alice.home / ".claude").mkdir(parents=True)
    shutil.copy2(grim.grim_home / "grimoire.toml", alice.grim_home / "grimoire.toml")
    alice.run("--global", "lock")
    alice.run("--global", "install")
    # The dotfiles pull: Alice's lock and files, Bob's own global.json.
    shutil.copy2(alice.grim_home / "grimoire.lock", grim.grim_home / "grimoire.lock")
    solo = root / "skills" / "solo"
    shutil.rmtree(solo)
    shutil.copytree(alice.home / ".claude" / "skills" / "solo", solo)
    state = grim.grim_home / "state" / "global.json"
    recorded = _record_pins(state)["solo"]
    assert recorded != _lock_pins(grim.grim_home / "grimoire.lock")["solo"]
    snap = state_snapshot(solo / "SKILL.md")

    rc, doc, stderr = _go(grim, "--global", "install")

    if stow:
        assert rc == 65, stderr
        assert doc["error"]["reason"] == "modified"
        assert _record_pins(state)["solo"] == recorded
        assert "solo v2" in _text(solo / "SKILL.md")
        return
    assert rc == 0, stderr
    assert {row["name"]: row["status"] for row in doc["items"]}["solo"] == "unchanged"
    assert (
        _record_pins(state)["solo"]
        == _lock_pins(grim.grim_home / "grimoire.lock")["solo"]
    )
    assert_unchanged(snap)


# ===========================================================================
# Shapes and copies: what a proof accepts inside a dropped skill
# ===========================================================================

NAMESPACED = "---\nname: extra\ndescription: Extra.\nmetadata:\n  claude.model: opus\n---\n# extra\n"


@pytest.mark.parametrize("body", ["plain", "namespaced"])
def test_an_update_dropping_a_skill_copied_into_other_client_folders(
    team, unique_repo, body
):
    # research_install_record_cache_test_matrix.md S30
    files = {"extra/SKILL.md": NAMESPACED} if body == "namespaced" else _extra()
    artifacts = {"helper:v1": ("skill", _helper()), "extra:v1": ("skill", files)}
    t = _shared(
        team, _stack(unique_repo, artifacts, BOTH, clients=("claude", "copilot"))
    )
    work = t.bob.work
    claude, copilot, cursor = (
        _skill(work, "extra", c) for c in ("claude", "copilot", "cursor")
    )
    # Claude's copy copied by hand over Copilot's, and into a client not selected.
    shutil.rmtree(copilot)
    shutil.copytree(claude, copilot)
    shutil.copytree(claude, cursor)
    unselected = _tree(cursor)
    _bundle(unique_repo, HELPER_ONLY, move=True)

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert rc == 0, stderr
    assert _actions(doc)["extra"] == "removed"
    assert_not_exists(claude)
    assert _tree(cursor) == unselected
    assert ".cursor" not in stderr
    if body == "plain":
        assert_not_exists(copilot)
        assert _left_lines(stderr, "extra") == []
        return
    line = _left_line(stderr, "skill", "extra", ".github/skills/extra")
    assert ".claude/skills/extra" not in _posix(line)
    assert copilot.is_dir()


def _reshape(folder: Path, shape: str, outside: Path) -> None:
    if shape == "missing-file":
        (folder / "ref.md").unlink()
    elif shape == "extra-file":
        (folder / "notes.txt").write_text("mine\n", encoding="utf-8", newline="\n")
    elif shape == "empty-dir":
        (folder / "empty").mkdir()
    elif shape == "extra-link":
        outside.write_text("kept\n", encoding="utf-8", newline="\n")
        (folder / "link.txt").symlink_to(outside)
    else:
        junk = {
            "pycache": "__pycache__/mod.cpython-313.pyc",
            "ds-store": ".DS_Store",
            "node-modules": "node_modules/dep/index.js",
            "git": ".git/HEAD",
        }[shape]
        (folder / junk).parent.mkdir(parents=True, exist_ok=True)
        (folder / junk).write_bytes(b"junk\n")


@pytest.mark.parametrize(
    ("shape", "proven"),
    [
        ("missing-file", False),
        ("extra-file", False),
        ("empty-dir", True),
        pytest.param("extra-link", True, marks=no_links),
        ("pycache", True),
        ("ds-store", True),
        ("node-modules", True),
        ("git", True),
    ],
)
def test_an_update_dropping_a_skill_removes_its_folder_only_when_it_proves(
    team, unique_repo, tmp_path, shape, proven
):
    # research_install_record_cache_test_matrix.md S32, E14, E15; ADR validation case 13
    t = _shared(team, _stack(unique_repo, PAIR, BOTH))
    work = t.bob.work
    extra = _skill(work, "extra")
    outside = tmp_path / "outside.txt"
    _reshape(extra, shape, outside)
    files = _tree(extra)
    _bundle(unique_repo, HELPER_ONLY, move=True)

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert rc == 0, stderr
    if proven:
        assert _actions(doc)["extra"] == "removed"
        assert_not_exists(extra)
        assert _left_lines(stderr, "extra") == []
        if shape == "extra-link":
            assert _text(outside) == "kept\n"
        return
    assert "extra" not in _actions(doc)
    assert _tree(extra) == files
    _left_line(stderr, "skill", "extra", ".claude/skills/extra")


def test_a_clone_update_across_a_bundle_that_renames_adds_and_drops_members(
    team, unique_repo
):
    # research_install_record_cache_test_matrix.md S39
    artifacts = {
        **PAIR,
        "style:v1": ("rule", {"style.md": "# style\n"}),
        "fresh:v1": ("skill", {"fresh/SKILL.md": "# fresh\n"}),
        "found:v1": ("skill", {"found/SKILL.md": "# found\n"}),
    }
    first = [*BOTH, ("rule", "old-style", "style:v1")]
    then = [
        *HELPER_ONLY,
        ("rule", "new-style", "style:v1"),
        ("skill", "fresh", "fresh:v1"),
        ("skill", "found", "found:v1"),
    ]
    t = _shared(team, _stack(unique_repo, artifacts, first))
    work = t.bob.work
    # A teammate's commit already holds the new member's render.
    found = _skill(work, "found") / "SKILL.md"
    found.parent.mkdir(parents=True)
    found.write_text("# found\n", encoding="utf-8", newline="\n")
    _bundle(unique_repo, then, move=True)

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert rc == 0, stderr
    actions = _actions(doc)
    assert actions["extra"] == "removed"
    assert actions["old-style"] == "removed"
    assert actions["helper"] == "unchanged"
    assert_not_exists(_skill(work, "extra"))
    assert_not_exists(_rule(work, "old-style"))
    assert _rule(work, "new-style").is_file()
    assert (_skill(work, "fresh") / "SKILL.md").is_file()
    assert _text(found) == "# found\n"
    assert _record_pins(_state(work)) == _lock_pins(work / "grimoire.lock")


POOL = ("codex", "zed", "amp")


@pytest.mark.parametrize("change", ["carried", "dropped"])
def test_a_clone_update_proves_a_skill_three_pool_clients_share_once(
    team, unique_repo, oci_proxy, change
):
    # research_install_record_cache_test_matrix.md S40 (update half)
    proxy: OciProxy = oci_proxy("count")
    artifacts = {
        "helper:v1": ("skill", _helper()),
        "pooled:v1": ("skill", {"pooled/SKILL.md": "# pooled\n"}),
    }
    members = [*HELPER_ONLY, ("skill", "pooled", "pooled:v1")]
    t = _shared(
        team,
        _stack(unique_repo, artifacts, members, host=proxy.host, clients=POOL),
        insecure=(proxy.host,),
    )
    work = t.bob.work
    pool = work / ".agents" / "skills" / "pooled"
    assert pool.is_dir()
    if change == "dropped":
        _bundle(unique_repo, HELPER_ONLY, host=proxy.host, move=True)
    before = _blob_reads(proxy, "pooled")

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert rc == 0, stderr
    assert _blob_reads(proxy, "pooled") - before == 1
    assert _left_lines(stderr, "pooled") == []
    if change == "dropped":
        assert _actions(doc)["pooled"] == "removed"
        assert_not_exists(pool)
        return
    assert _actions(doc)["pooled"] == "unchanged"
    outputs = _records(_state(work))["pooled"]["outputs"]
    assert sorted(o["client"] for o in outputs) == sorted(POOL)
    assert len({json.dumps(o["target"], sort_keys=True) for o in outputs}) == 1


def test_a_clone_update_leaves_committed_dev_install_outputs_alone(team, unique_repo):
    # research_install_record_cache_test_matrix.md S42 (update half)
    t = team("CO")
    alice = t.alice
    make_artifact(
        f"{unique_repo}/kit", "skill", {"kit/SKILL.md": "# kit v1\n"}, tag="rolling"
    )
    write_config(
        alice.work, skills={"kit": f"{REGISTRY_HOST}/{unique_repo}/kit:rolling"}
    )
    source = alice.work / "dev" / "local" / "SKILL.md"
    source.parent.mkdir(parents=True)
    source.write_text(_local_doc("local", "v1"), encoding="utf-8", newline="\n")
    alice.runner.run("lock")
    alice.runner.run("install")
    alice.runner.run("install", "./dev/local")
    alice.commit_all("feat: declare, and a dev install")
    alice.push()
    t.bob.pull()
    work = t.bob.work
    local = _skill(work, "local")
    files = _tree(local)
    snap = state_snapshot(local / "SKILL.md")
    move_tag(f"{unique_repo}/kit", {"kit/SKILL.md": "# kit v2\n"}, tag="rolling")

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert rc == 0, stderr
    assert _actions(doc)["kit"] == "updated"
    assert "local" not in _actions(doc)
    assert "'local'" not in stderr
    assert _tree(local) == files
    assert_unchanged(snap)
    assert "local" not in _records(_state(work))


# ===========================================================================
# Links: a linked root, a linked destination, a folder where a file belongs
# ===========================================================================


@no_links
@pytest.mark.parametrize("inside", [True, False], ids=["inside", "outside"])
def test_a_clone_update_through_a_linked_client_root(team, unique_repo, inside):
    # research_install_record_cache_test_matrix.md E09, E10 (update halves); ADR validation case 13
    t = _shared(team, _stack(unique_repo, PAIR, BOTH, clients=("claude",)))
    work = t.bob.work
    real = work / "config" / "claude" if inside else work.parent / "dotfiles" / "claude"
    real.parent.mkdir(parents=True)
    shutil.move(work / ".claude", real)
    (work / ".claude").symlink_to(real, target_is_directory=True)
    extra = real / "skills" / "extra"
    files = _tree(extra)
    _bundle(unique_repo, HELPER_ONLY, move=True)

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert rc == 0, stderr
    assert _actions(doc)["helper"] == "unchanged"
    assert_symlink_exists(work / ".claude")
    if inside:
        assert _actions(doc)["extra"] == "removed"
        assert_not_exists(extra)
        assert _left_lines(stderr, "extra") == []
        return
    # Rows 1 to 3 read through the link; a proof refuses it and names the link.
    assert _tree(extra) == files
    line = _left_line(stderr, "skill", "extra", ".claude/skills/extra")
    assert f"under {_posix(str(work / '.claude'))} " in _posix(line)
    assert "by hand" not in line
    assert "remove the link" not in line
    assert "extra" not in _actions(doc)


def test_a_folder_named_like_a_dropped_rule_file_is_not_proven(team, unique_repo):
    # research_install_record_cache_test_matrix.md E22; ADR validation case 13
    artifacts = {
        "helper:v1": ("skill", _helper()),
        "style:v1": ("rule", {"style.md": "# style\n"}),
    }
    t = _shared(
        team,
        _stack(unique_repo, artifacts, [*HELPER_ONLY, ("rule", "style", "style:v1")]),
    )
    work = t.bob.work
    rule = _rule(work, "style")
    body = rule.read_bytes()
    rule.unlink()
    rule.mkdir()
    (rule / "style.md").write_bytes(body)
    _bundle(unique_repo, HELPER_ONLY, move=True)

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert rc == 0, stderr
    assert "style" not in _actions(doc)
    assert (rule / "style.md").read_bytes() == body
    _left_line(stderr, "rule", "style", ".claude/rules/style.md")


def test_a_rule_whose_support_folder_outlives_its_index_is_named_on_a_drop(
    team, unique_repo
):
    # ADR validation case 14
    guide = {
        "guide.md": "---\npaths: ['**/*.md']\n---\n# guide\n",
        "guide/notes.md": "notes\n",
    }
    artifacts = {"helper:v1": ("skill", _helper()), "guide:v1": ("rule", guide)}
    t = _shared(
        team,
        _stack(unique_repo, artifacts, [*HELPER_ONLY, ("rule", "guide", "guide:v1")]),
    )
    work = t.bob.work
    _rule(work, "guide").unlink()
    support = work / ".claude" / "rules" / "guide"
    files = _tree(support)
    _bundle(unique_repo, HELPER_ONLY, move=True)

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert rc == 0, stderr
    assert "guide" not in _actions(doc)
    assert _tree(support) == files
    line = _left_line(stderr, "rule", "guide", ".claude/rules/guide")
    assert ".claude/rules/guide.md" not in _posix(line)


@no_links
def test_a_linked_rule_file_is_adopted_while_shipped_and_never_proven_on_a_drop(
    team, unique_repo
):
    # research_install_record_cache_test_matrix.md E07 (update half); ADR validation case 13
    artifacts = {
        "kept:v1": ("rule", {"kept.md": "# kept\n"}),
        "gone:v1": ("rule", {"gone.md": "# gone\n"}),
    }
    t = _shared(
        team,
        _stack(
            unique_repo,
            artifacts,
            [("rule", "kept", "kept:v1"), ("rule", "gone", "gone:v1")],
        ),
    )
    work = t.bob.work
    shared = work / "shared"
    shared.mkdir()
    bodies = {}
    for name in ("kept", "gone"):
        target = shared / f"{name}.md"
        bodies[name] = _rule(work, name).read_bytes()
        target.write_bytes(bodies[name])
        _rule(work, name).unlink()
        _rule(work, name).symlink_to(target)
    _bundle(unique_repo, [("rule", "kept", "kept:v1")], move=True)

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert rc == 0, stderr
    assert _actions(doc)["kept"] == "unchanged"
    assert (shared / "kept.md").read_bytes() == bodies["kept"]
    assert _left_lines(stderr, "kept") == []
    line = _left_line(stderr, "rule", "gone", ".claude/rules/gone.md", "shared/gone.md")
    assert "remove the link only" in line
    assert_symlink_exists(_rule(work, "gone"))
    assert (shared / "gone.md").read_bytes() == bodies["gone"]


@no_links
@pytest.mark.parametrize("shape", ["folder", "dangling", "shipped-folder"])
def test_a_linked_skill_folder_is_never_proven(team, unique_repo, tmp_path, shape):
    # research_install_record_cache_test_matrix.md E08 (update half); ADR validation case 13
    t = _shared(team, _stack(unique_repo, PAIR, BOTH))
    work = t.bob.work
    name = "helper" if shape == "shipped-folder" else "extra"
    dest = _skill(work, name)
    target = tmp_path / "elsewhere" / name
    target.parent.mkdir()
    if shape == "dangling":
        shutil.rmtree(dest)
    else:
        shutil.move(dest, target)
    dest.symlink_to(target, target_is_directory=True)
    kept = None if shape == "dangling" else _tree(target)
    if shape != "shipped-folder":
        _bundle(unique_repo, HELPER_ONLY, move=True)

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert_symlink_exists(dest)
    if kept is None:
        assert_not_exists(target)
    else:
        assert _tree(target) == kept
    if shape == "shipped-folder":
        assert rc == 65, stderr
        assert {row["name"]: row["refused"] for row in doc["items"]}["helper"] is True
        return
    assert rc == 0, stderr
    line = _left_line(
        stderr, "skill", "extra", ".claude/skills/extra", "elsewhere/extra"
    )
    assert "remove the link only" in line
    assert "extra" not in _actions(doc)


# ===========================================================================
# A committed state file: the record alone grants no deletion
# ===========================================================================

BOGUS = "sha256:" + "0" * 64


def _members(version: str, *, extra: bool) -> list[Member]:
    members = [("skill", "helper", f"helper:{version}")]
    if extra:
        members.append(("skill", "extra", f"extra:{version}"))
    return members


@pytest.mark.parametrize(
    ("older", "edited"),
    [(False, False), (True, False), (True, True)],
    ids=["at-the-lock-pin", "at-an-older-pin", "at-an-older-pin-edited"],
)
def test_a_committed_record_whose_hashes_match_nothing_decides_nothing_on_update(
    team, unique_repo, older, edited
):
    # research_install_record_cache_test_matrix.md E23 (update half)
    t = team("CO")
    alice = t.alice
    for version in ("v1", "v2"):
        make_artifact(f"{unique_repo}/helper", "skill", _helper(version), tag=version)
        make_artifact(f"{unique_repo}/extra", "skill", _extra(version), tag=version)
    _bundle(unique_repo, _members("v1", extra=True), move=False)
    write_config(
        alice.work, bundles={"stack": f"{REGISTRY_HOST}/{unique_repo}/stack:rolling"}
    )
    alice.runner.run("lock")
    alice.runner.run("install")
    state = json.loads(_state(alice.work).read_bytes())
    version = "v2" if older else "v1"
    if older:
        _bundle(unique_repo, _members("v2", extra=True), move=True)
        alice.runner.run("update")
    for record in state["records"]:
        for output in record["outputs"]:
            output["content_hash"] = BOGUS
    _state(alice.work).write_text(
        json.dumps(state, indent=2), encoding="utf-8", newline="\n"
    )
    git(alice.work, "add", "-f", ".grimoire/state.json")
    alice.commit_all("feat: declare, with the state file")
    alice.push()
    t.bob.pull()
    work = t.bob.work
    extra = _skill(work, "extra")
    if edited:
        path = extra / "SKILL.md"
        path.write_text(_text(path) + EDIT, encoding="utf-8", newline="\n")
    files = _tree(extra)
    _bundle(unique_repo, _members(version, extra=False), move=True)

    rc, doc, stderr = _go(t.bob.runner, "update")

    rows = {row["name"]: row for row in doc.get("items", [])}
    if not older:
        # Record and lock name one pin: the drifted files are an edit, refused and kept.
        assert rc == 65, stderr
        assert rows["helper"]["refused"] is True
        assert rows["extra"]["action"] == "kept-modified"
        assert _tree(extra) == files
        return
    assert rc == 0, stderr
    assert rows["helper"]["action"] == "unchanged"
    if edited:
        assert rows["extra"]["action"] == "kept-modified"
        assert _tree(extra) == files
    else:
        assert rows["extra"]["action"] == "removed"
        assert_not_exists(extra)
    assert _left_lines(stderr, "extra") == []
