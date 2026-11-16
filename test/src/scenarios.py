# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Multi-machine and multi-workspace scenario helpers for the acceptance suite.

A "machine" is a ``GrimRunner`` whose ``GRIM_HOME`` and ``HOME`` sit in their
own directory. ``GrimRunner`` puts ``HOME`` at ``<grim_home parent>/home``, so
two runners whose homes share a parent share ``HOME`` and the global skill
pool: ``fresh_runner`` gives each its own parent.
"""

from __future__ import annotations

import hashlib
import os
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from src.helpers import make_artifact, write_config
from src.registry import REGISTRY_BASE, PublishedArtifact, list_tags, retag, tag_digest
from src.runner import GrimRunner

_TEST_DIR = Path(__file__).resolve().parent.parent


def _binary() -> Path:
    if env := os.environ.get("GRIM_COMMAND"):
        return Path(env)
    path = _TEST_DIR / "bin" / "grim"
    return path.with_suffix(".exe") if sys.platform == "win32" else path


def allow_insecure(runner: GrimRunner, *hosts: str) -> None:
    """Add ``hosts`` to the runner's plain-HTTP set, keeping what it already holds."""
    current = [h for h in runner.env.get("GRIM_INSECURE_REGISTRIES", "").split(",") if h]
    runner.env["GRIM_INSECURE_REGISTRIES"] = ",".join([*current, *(h for h in hosts if h not in current)])


# ---------------------------------------------------------------------------
# Machines and workspaces
# ---------------------------------------------------------------------------


def fresh_runner(
    tmp: Path, name: str, *, cwd: Path | None = None, insecure: tuple[str, ...] = ()
) -> GrimRunner:
    """A runner with its own ``GRIM_HOME`` and ``HOME`` under ``tmp/name``."""
    root = tmp / name
    grim_home = root / "grim-home"
    grim_home.mkdir(parents=True)
    runner = GrimRunner(_binary(), grim_home, cwd=cwd)
    allow_insecure(runner, *insecure)
    return runner


def cold_runner(
    warm: GrimRunner, tmp: Path, name: str, *, global_scope: bool = False
) -> GrimRunner:
    """A teammate's machine: same workspace, empty store.

    Project scope reuses ``warm.cwd`` under a fresh ``GRIM_HOME``. Global scope
    keeps its config and lock in ``GRIM_HOME``, so the new one starts with
    copies of those two files and nothing else.
    """
    cold = fresh_runner(tmp, name, cwd=warm.cwd)
    cold.env["GRIM_INSECURE_REGISTRIES"] = warm.env.get("GRIM_INSECURE_REGISTRIES", "")
    if not cold.env["GRIM_INSECURE_REGISTRIES"]:
        del cold.env["GRIM_INSECURE_REGISTRIES"]
    if global_scope:
        for leaf in ("grimoire.toml", "grimoire.lock"):
            source = warm.grim_home / leaf
            if source.is_file():
                shutil.copy2(source, cold.grim_home / leaf)
    return cold


def clone_workspace(src: Path, dst: Path) -> Path:
    """Copy a workspace without its ``.grimoire/``."""
    shutil.copytree(src, dst, ignore=shutil.ignore_patterns(".grimoire"), symlinks=True)
    return dst


def pull_from(src: Path, dst: Path) -> None:
    """Make ``dst`` match ``src`` the way a checkout does, keeping ``dst/.grimoire/``.

    Lock, config and client outputs come over; ``.git`` and the local
    ``.grimoire/`` of both sides stay out of it.
    """
    keep = {".grimoire", ".git"}
    for entry in dst.iterdir():
        if entry.name in keep:
            continue
        if entry.is_dir() and not entry.is_symlink():
            shutil.rmtree(entry)
        else:
            entry.unlink()
    for entry in src.iterdir():
        if entry.name in keep:
            continue
        target = dst / entry.name
        if entry.is_dir() and not entry.is_symlink():
            shutil.copytree(entry, target, symlinks=True)
        else:
            shutil.copy2(entry, target, follow_symlinks=False)


# ---------------------------------------------------------------------------
# Git team
# ---------------------------------------------------------------------------

_CLIENT_IGNORES = (".claude/skills/", ".claude/rules/", ".claude/agents/", ".agents/", ".grimoire/")
_GIT_ENV = {"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull}


def git(repo: Path, *args: str) -> str:
    """Run git in ``repo`` with no system or global config; return trimmed stdout."""
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
        env={**os.environ, **_GIT_ENV},
    )
    return result.stdout.strip()


@dataclass
class Member:
    name: str
    work: Path
    runner: GrimRunner

    def commit_all(self, message: str) -> None:
        git(self.work, "add", "-A")
        git(self.work, "commit", "-q", "--allow-empty", "-m", message)

    def push(self) -> None:
        git(self.work, "push", "-q", "origin", "main")

    def pull(self) -> None:
        git(self.work, "pull", "-q", "--ff-only", "origin", "main")

    def tracked(self) -> list[str]:
        return git(self.work, "ls-files").splitlines()


@dataclass
class Team:
    origin: Path
    model: str
    alice: Member
    bob: Member
    members: list[Member] = field(default_factory=list)


def git_team(tmp: Path, model: str, *, insecure: tuple[str, ...] = ()) -> Team:
    """A bare origin and two clones, Alice and Bob, each with its own machine.

    Model ``"CO"`` commits client outputs; ``"GO"`` commits a root ``.gitignore``
    for the client folders instead. Both keep ``.grimoire/`` out of git. Alice's
    first commit, with the ``.claude/`` marker that makes Claude the detected
    client, is already pushed when this returns.
    """
    if model not in ("CO", "GO"):
        raise ValueError(f"unknown team model {model!r}")
    origin = tmp / "origin.git"
    git(tmp, "init", "-q", "--bare", "-b", "main", str(origin))

    members: list[Member] = []
    for name in ("alice", "bob"):
        work = tmp / name / "work"
        work.parent.mkdir(parents=True)
        subprocess.run(
            ["git", "clone", "-q", str(origin), str(work)],
            check=True,
            capture_output=True,
            env={**os.environ, **_GIT_ENV},
        )
        git(work, "checkout", "-q", "-B", "main")
        git(work, "config", "user.email", f"{name}@example.invalid")
        git(work, "config", "user.name", name.title())
        git(work, "config", "core.autocrlf", "false")
        exclude = work / ".git" / "info" / "exclude"
        exclude.write_text(".grimoire/\n", encoding="utf-8", newline="\n")
        members.append(Member(name, work, fresh_runner(tmp, name, cwd=work, insecure=insecure)))
    alice, bob = members

    (alice.work / ".claude").mkdir()
    (alice.work / ".claude" / ".gitkeep").write_text("", encoding="utf-8", newline="\n")
    if model == "GO":
        (alice.work / ".gitignore").write_text(
            "\n".join(_CLIENT_IGNORES) + "\n", encoding="utf-8", newline="\n"
        )
    alice.commit_all("chore: workspace marker")
    alice.push()
    bob.pull()
    return Team(origin, model, alice, bob, members)


# ---------------------------------------------------------------------------
# Snapshots
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class _FileState:
    path: Path
    content: bytes | None
    mtime_ns: int | None
    inode: int | None


@dataclass(frozen=True)
class Snapshot:
    files: tuple[_FileState, ...]


def _file_state(path: Path, with_content: bool) -> _FileState:
    try:
        st = path.lstat()
    except FileNotFoundError:
        return _FileState(path, None, None, None)
    content = path.read_bytes() if with_content and path.is_file() else None
    return _FileState(path, content, st.st_mtime_ns, st.st_ino)


def state_snapshot(state: Path, *outputs: Path) -> Snapshot:
    """``state`` bytes, mtime and inode, plus mtime and inode of each output."""
    return Snapshot(
        (_file_state(state, True), *(_file_state(p, False) for p in outputs))
    )


def assert_unchanged(snapshot: Snapshot) -> None:
    """Fail naming the first path whose bytes, mtime or inode differ from ``snapshot``."""
    for before in snapshot.files:
        after = _file_state(before.path, before.content is not None)
        assert after == before, f"{before.path} changed: {before} -> {after}"


# ---------------------------------------------------------------------------
# Rendering a pin elsewhere
# ---------------------------------------------------------------------------

_KIND_TABLE = {"skill": "skills", "rule": "rules", "agent": "agents", "hook": "hooks"}


def render_at(
    runner_factory: Callable[[Path], GrimRunner],
    pin_ref: PublishedArtifact,
    target: Path,
    *,
    clients: list[str] | None = None,
    partial: list[str] | None = None,
) -> list[str]:
    """Render ``pin_ref`` in a scratch project, then copy the output over ``target``.

    ``runner_factory`` builds a runner inside the scratch dir (``grim_at``).
    ``clients`` is passed as ``--client``; ``partial`` limits the copy to those
    relative paths. Config, lock and ``.grimoire/`` never travel. Returns the
    relative files copied.
    """
    scratch = Path(tempfile.mkdtemp(prefix="render-", dir=target.parent))
    (scratch / ".claude").mkdir()
    name = pin_ref.repo.rsplit("/", 1)[-1]
    write_config(scratch, **{_KIND_TABLE[pin_ref.kind]: {name: pin_ref.pinned}})
    runner = runner_factory(scratch)
    runner.run("lock")
    flags = [f for c in clients or [] for f in ("--client", c)]
    runner.run("install", *flags)

    skip = {"grimoire.toml", "grimoire.lock", ".grimoire"}
    if partial is None:
        sources = [p for p in scratch.iterdir() if p.name not in skip]
        files = [f for s in sources for f in ([s] if s.is_file() else sorted(s.rglob("*")))]
    else:
        files = [scratch / rel for rel in partial]
    copied: list[str] = []
    for file in files:
        if not file.is_file() and not file.is_symlink():
            continue
        rel = file.relative_to(scratch)
        (target / rel).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(file, target / rel)
        copied.append(rel.as_posix())
    shutil.rmtree(scratch)
    return copied


# ---------------------------------------------------------------------------
# Registry and lock edits
# ---------------------------------------------------------------------------

_GONE_DIGEST = "sha256:" + hashlib.sha256(b"grim-test-gone-pin").hexdigest()
_PINNED_DIGEST = re.compile(r'(?m)^(pinned\s*=\s*"[^"]*@)sha256:[0-9a-f]{64}')


def gone_pin(lock_path: Path, name: str) -> str:
    """Rewrite ``name``'s pin to a digest no registry holds; return that digest."""
    text = lock_path.read_text(encoding="utf-8")
    blocks = re.split(r"(?m)^(?=\[\[)", text)
    hits = 0
    for i, block in enumerate(blocks):
        if re.search(rf'(?m)^name\s*=\s*"{re.escape(name)}"', block) and _PINNED_DIGEST.search(block):
            blocks[i] = _PINNED_DIGEST.sub(lambda m: m.group(1) + _GONE_DIGEST, block, count=1)
            hits += 1
    assert hits == 1, f"expected one pinned row named {name!r}, found {hits}"
    lock_path.write_text("".join(blocks), encoding="utf-8", newline="\n")
    return _GONE_DIGEST


def move_tag(
    repo: str,
    files: dict[str, str | bytes],
    tag: str = "latest",
    kind: str = "skill",
) -> PublishedArtifact:
    """Publish new bytes under ``tag``, keeping the manifest it leaves reachable."""
    old = tag_digest(repo, tag) if tag in _tags_or_none(repo) else None
    if old is not None:
        retag(repo, f"keep-{old.removeprefix('sha256:')[:12]}", old)
    moved = make_artifact(repo, kind, files, tag=tag)
    assert moved.digest != old, "a moved tag must change the digest"
    return moved


def _tags_or_none(repo: str) -> list[str]:
    try:
        return list_tags(repo)
    except urllib.error.HTTPError:
        return []


def delete_manifest(repo: str, reference: str) -> None:
    """Delete a manifest by digest, or a tag by name."""
    request = urllib.request.Request(
        f"{REGISTRY_BASE}/v2/{repo}/manifests/{reference}", method="DELETE"
    )
    with urllib.request.urlopen(request):
        pass


def rows_by_name(report: dict[str, Any]) -> dict[str, dict[str, Any]]:
    """The ``items`` of a JSON report keyed by row name."""
    return {row["name"]: row for row in report["items"]}


def to_crlf(path: Path) -> None:
    """Rewrite a text file with CRLF line endings."""
    data = path.read_bytes().replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
    path.write_bytes(data)


__all__ = [
    "Member",
    "Snapshot",
    "Team",
    "allow_insecure",
    "assert_unchanged",
    "clone_workspace",
    "cold_runner",
    "delete_manifest",
    "fresh_runner",
    "git",
    "git_team",
    "gone_pin",
    "move_tag",
    "pull_from",
    "render_at",
    "rows_by_name",
    "state_snapshot",
    "to_crlf",
]
