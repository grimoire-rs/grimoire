# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim update --marketplace` acceptance tests (C-011–C-013, S-020–S-025, S-027).

`update --marketplace <M>` rolls the pins of `M`'s `marketplace.lock` forward
and installs nothing. The initial lock is produced by the command itself (an
absent lock resolves every selected plugin in full), so no export is needed.
"""
from __future__ import annotations

import hashlib
import json
import os
import sys
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

import pytest
import tomllib

from src.helpers import make_artifact, write_config
from src.registry import PublishedArtifact

unix_only = pytest.mark.skipif(
    sys.platform == "win32", reason="flock(2) contention needs a Unix host"
)

_KINDS = ("skill", "rule", "agent", "mcp")

# Every key `grim update --format json` emitted before C-013, in wire order.
_PRE_C013_KEYS = [
    "kind",
    "name",
    "old",
    "new",
    "action",
    "reaped_clients",
    "kept_modified_clients",
    "retained",
    "abandoned_entries",
    "refused",
]


def _skill(repo: str, name: str, body: str, tag: str = "stable") -> PublishedArtifact:
    return make_artifact(
        repo,
        "skill",
        {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: Demo {body}.\n---\n# {body}\n"},
        tag=tag,
    )


def _rule(repo: str, name: str, body: str, tag: str = "stable") -> PublishedArtifact:
    return make_artifact(repo, "rule", {f"{name}.md": f"# {body}\n"}, tag=tag)


def _write_marketplace(path: Path, plugins: dict[str, list[str]]) -> Path:
    body = "".join(
        f"[plugins.{name}]\ninclude = [{', '.join(json.dumps(i) for i in includes)}]\n\n"
        for name, includes in plugins.items()
    )
    path.write_text(body)
    return path


def _lock(path: Path) -> dict:
    return tomllib.loads(path.read_text())


def _pins(lock: dict, plugin: str) -> dict[tuple[str, str], str]:
    """`(kind, name) -> pinned` for one plugin's part of a marketplace lock."""
    return {
        (kind, e["name"]): e["pinned"]
        for kind in _KINDS
        for e in lock.get(kind, [])
        if e.get("plugin") == plugin
    }


def _part(lock: dict, plugin: str) -> tuple[list[dict], dict]:
    """One plugin's raw entries plus its `[[plugin]]` row."""
    entries = [e for kind in _KINDS for e in lock.get(kind, []) if e.get("plugin") == plugin]
    row = next(r for r in lock["plugin"] if r["name"] == plugin)
    return entries, row


def _snapshot(root: Path, exclude: set[Path]) -> dict[str, tuple | None]:
    """Every dir (presence) and file (content hash + mtime) under ``root``."""
    snap: dict[str, tuple | None] = {}
    for dirpath, dirnames, filenames in os.walk(root):
        here = Path(dirpath)
        dirnames[:] = [d for d in dirnames if here / d not in exclude]
        for d in dirnames:
            snap[str((here / d).relative_to(root))] = None
        for f in filenames:
            p = here / f
            if p in exclude:
                continue
            st = p.lstat()
            snap[str(p.relative_to(root))] = (
                hashlib.sha256(p.read_bytes()).hexdigest() if p.is_file() else "link",
                st.st_mtime_ns,
            )
    return snap


@contextmanager
def _held_flock(sidecar: Path) -> Iterator[None]:
    """Hold grim's advisory lock on ``sidecar`` (fs4 flock, same lock space
    as ``fcntl.flock``) for the block's duration."""
    import fcntl

    fd = os.open(sidecar, os.O_RDWR | os.O_CREAT, 0o644)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield
    finally:
        os.close(fd)
        sidecar.unlink(missing_ok=True)


def _error(result) -> dict:
    return json.loads(result.stdout)["error"]


# ── S-020 / C-011 / C-013 — rolls all, installs nothing ─────────────────


def test_s020_update_rolls_all_pins_and_installs_nothing(
    grim_at, project_dir: Path, tmp_path: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    repo = f"{unique_repo}/team-plan"
    v1 = _skill(repo, "team-plan", "v1")
    m = _write_marketplace(project_dir / "marketplace.toml", {"team": [f"{registry}/{repo}:stable"]})
    lock_path = project_dir / "marketplace.lock"
    runner = grim_at(project_dir)
    # Workspace (a detected Claude project, no grimoire.toml) and HOME both
    # live under tmp_path; only GRIM_HOME (cache) and L may change.
    exclude = {grim_home, lock_path}
    before = _snapshot(tmp_path, exclude)

    first = runner.run("--format", "json", "update", "--marketplace", "marketplace.toml", check=False)
    assert first.returncode == 0, first.stderr
    rows = json.loads(first.stdout)["items"]
    assert rows, "the initial resolve reports every pinned member"
    for row in rows:
        assert list(row) == [*_PRE_C013_KEYS, "plugin"], f"C-013 key order: {row}"
        assert row["plugin"] == "team", row
    assert _pins(_lock(lock_path), "team") == {("skill", "team-plan"): f"{registry}/{repo}@{v1.digest}"}
    assert _snapshot(tmp_path, exclude) == before, "a marketplace update must install nothing"

    v2 = _skill(repo, "team-plan", "v2")  # floating tag moves onto v2
    assert v2.digest != v1.digest
    rows = runner.json("update", "--marketplace", str(m))["items"]
    row = next(r for r in rows if r["name"] == "team-plan")
    assert (row["plugin"], row["kind"], row["action"]) == ("team", "skill", "updated"), row
    assert (row["old"], row["new"]) == (v1.digest, v2.digest), row
    assert _pins(_lock(lock_path), "team") == {("skill", "team-plan"): f"{registry}/{repo}@{v2.digest}"}

    plain = runner.plain("update", "--marketplace", "marketplace.toml")
    assert "team:team-plan" in plain.stdout, f"C-013 plain prefix: {plain.stdout}"

    after = _snapshot(tmp_path, exclude)
    assert after == before, "no vendor dir, .grimoire/, state file or config may appear or change"
    assert not (project_dir / "grimoire.toml").exists()
    assert not (project_dir / ".grimoire").exists()
    # The store's layout creates an empty `state/` dir; no install state may land in it.
    state_dir = grim_home / "state"
    assert not state_dir.exists() or not any(state_dir.iterdir()), "no global install state either"


@pytest.mark.parametrize(
    "scope_flag",
    [["--global"], ["--config", "x.toml"]],
    ids=["global", "config"],
)
def test_s020_scope_flags_are_refused_before_the_manifest_is_read(
    grim_at, project_dir: Path, scope_flag: list[str]
) -> None:
    # Plan decision 30: refused (64) before M is loaded — a missing M would
    # otherwise be a 65 manifest error.
    runner = grim_at(project_dir)
    result = runner.run(
        "--format", "json", *scope_flag, "update", "--marketplace", "missing.toml", check=False
    )
    assert result.returncode == 64, result.stderr
    assert _error(result)["exit"] == 64
    assert not (project_dir / "missing.lock").exists()


# ── S-021 — one plugin ─────────────────────────────────────────────────


def test_s021_update_one_plugin_leaves_the_other_part_identical(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    s_repo, r_repo = f"{unique_repo}/shared", f"{unique_repo}/style"
    _skill(s_repo, "shared", "v1")
    _rule(r_repo, "style", "v1")
    includes = [f"{registry}/{s_repo}:stable", f"{registry}/{r_repo}:stable"]
    # C-010: the lock path derives from the manifest stem (`team.toml` → `team.lock`).
    _write_marketplace(project_dir / "team.toml", {"a": includes, "b": includes})
    lock_path = project_dir / "team.lock"
    runner = grim_at(project_dir)
    runner.run("update", "--marketplace", "team.toml")
    first = _lock(lock_path)
    assert _pins(first, "a") == _pins(first, "b"), "both plugins pin the same members"

    s2 = _skill(s_repo, "shared", "v2")
    r2 = _rule(r_repo, "style", "v2")
    rows = runner.json("update", "--marketplace", "team.toml", "a")["items"]
    assert {r["plugin"] for r in rows} <= {"a", "b"}, rows
    assert all(r["action"] == "updated" for r in rows if r["plugin"] == "a"), rows
    # Plan decision 32: the carried part reports its rows as `unchanged`.
    b_rows = {(r["kind"], r["name"], r["action"]) for r in rows if r["plugin"] == "b"}
    assert b_rows == {("skill", "shared", "unchanged"), ("rule", "style", "unchanged")}, rows
    assert len(rows) == 4, f"2 members x 2 plugins: {rows}"

    second = _lock(lock_path)
    assert _pins(second, "a") == {
        ("skill", "shared"): f"{registry}/{s_repo}@{s2.digest}",
        ("rule", "style"): f"{registry}/{r_repo}@{r2.digest}",
    }
    assert _part(second, "b") == _part(first, "b"), "b's part is carried verbatim"


def test_s021_plugin_dropped_from_m_yields_no_rows_and_leaves_l(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # C-009 step 6 / C-013: a plugin no longer in M loses its part and reports nothing.
    repo = f"{unique_repo}/shared"
    _skill(repo, "shared", "v1")
    include = [f"{registry}/{repo}:stable"]
    m = project_dir / "marketplace.toml"
    lock_path = project_dir / "marketplace.lock"
    _write_marketplace(m, {"a": include, "b": include})
    runner = grim_at(project_dir)
    runner.run("update", "--marketplace", "marketplace.toml")
    assert {r["name"] for r in _lock(lock_path)["plugin"]} == {"a", "b"}

    _write_marketplace(m, {"a": include})
    rows = runner.json("update", "--marketplace", "marketplace.toml")["items"]
    assert rows and all(r["plugin"] == "a" for r in rows), rows

    after = _lock(lock_path)
    assert [r["name"] for r in after["plugin"]] == ["a"], after
    assert _pins(after, "b") == {}, "b's entries leave L with its row"


# ── S-022 — one member, across kinds ────────────────────────────────────


def test_s022_member_selector_rolls_every_kind_of_that_name_only(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    skill_repo = f"{unique_repo}/skills/team-plan"
    rule_repo = f"{unique_repo}/rules/team-plan"
    other_repo = f"{unique_repo}/skills/other"
    _skill(skill_repo, "team-plan", "v1")
    _rule(rule_repo, "team-plan", "v1")
    other1 = _skill(other_repo, "other", "v1")
    _write_marketplace(
        project_dir / "marketplace.toml",
        {
            "a": [
                f"{registry}/{skill_repo}:stable",
                f"{registry}/{rule_repo}:stable",
                f"{registry}/{other_repo}:stable",
            ]
        },
    )
    lock_path = project_dir / "marketplace.lock"
    runner = grim_at(project_dir)
    runner.run("update", "--marketplace", "marketplace.toml")

    s2 = _skill(skill_repo, "team-plan", "v2")
    r2 = _rule(rule_repo, "team-plan", "v2")
    _skill(other_repo, "other", "v2")  # moves too, but is not selected
    rows = runner.json("update", "--marketplace", "marketplace.toml", "a:team-plan")["items"]
    updated = {(r["kind"], r["name"]) for r in rows if r["action"] == "updated"}
    assert updated == {("skill", "team-plan"), ("rule", "team-plan")}, rows
    assert all(r["plugin"] == "a" for r in rows), rows

    assert _pins(_lock(lock_path), "a") == {
        ("skill", "team-plan"): f"{registry}/{skill_repo}@{s2.digest}",
        ("rule", "team-plan"): f"{registry}/{rule_repo}@{r2.digest}",
        ("skill", "other"): f"{registry}/{other_repo}@{other1.digest}",
    }


# ── S-023 / C-012 — selector errors ─────────────────────────────────────


def _two_member_plugin(project_dir: Path, registry: str, unique_repo: str) -> tuple[str, str]:
    plan, other = f"{unique_repo}/team-plan", f"{unique_repo}/other"
    _skill(plan, "team-plan", "v1")
    _skill(other, "other", "v1")
    _write_marketplace(
        project_dir / "marketplace.toml",
        {"a": [f"{registry}/{plan}:stable", f"{registry}/{other}:stable"]},
    )
    return plan, other


def test_s023_selector_errors_leave_the_lock_untouched(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    _two_member_plugin(project_dir, registry, unique_repo)
    lock_path = project_dir / "marketplace.lock"
    runner = grim_at(project_dir)
    runner.run("update", "--marketplace", "marketplace.toml")
    pinned = lock_path.read_bytes()

    cases = [
        (["ghost"], 79),  # not in M
        (["a:ghost"], 79),  # a's part present: same SelectorNotFound
        (["a:"], 64),
        ([":x"], 64),
        (["a:b:c"], 64),
        ([""], 64),
        (["a", "b:"], 64),
    ]
    for selectors, code in cases:
        result = runner.run(
            "--format", "json", "update", "--marketplace", "marketplace.toml", *selectors, check=False
        )
        assert result.returncode == code, f"{selectors}: {result.stderr}"
        assert _error(result)["exit"] == code, selectors
        assert lock_path.read_bytes() == pinned, f"{selectors} must not write L"


def test_s023_exact_duplicate_selectors_are_accepted(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # Plan decision 30.
    _two_member_plugin(project_dir, registry, unique_repo)
    runner = grim_at(project_dir)
    runner.run("update", "--marketplace", "marketplace.toml")
    result = runner.run("update", "--marketplace", "marketplace.toml", "a", "a", "a:team-plan", check=False)
    assert result.returncode == 0, result.stderr


def test_s023_member_selector_on_a_stale_part_is_stale_lock(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    plan, other = _two_member_plugin(project_dir, registry, unique_repo)
    lock_path = project_dir / "marketplace.lock"
    runner = grim_at(project_dir)
    runner.run("update", "--marketplace", "marketplace.toml")

    extra = f"{unique_repo}/extra"
    extra1 = _skill(extra, "extra", "v1")
    # Editing a's include makes a's part stale (C-033).
    _write_marketplace(
        project_dir / "marketplace.toml",
        {"a": [f"{registry}/{plan}:stable", f"{registry}/{other}:stable", f"{registry}/{extra}:stable"]},
    )
    stale = lock_path.read_bytes()

    result = runner.run(
        "--format", "json", "update", "--marketplace", "marketplace.toml", "a:team-plan", check=False
    )
    assert result.returncode == 65, result.stderr
    assert _error(result)["reason"] == "stale-lock"
    assert lock_path.read_bytes() == stale

    # A whole-plugin selector re-resolves the stale plugin.
    runner.run("update", "--marketplace", "marketplace.toml", "a")
    assert _pins(_lock(lock_path), "a")[("skill", "extra")] == f"{registry}/{extra}@{extra1.digest}"


def test_s023_member_selector_without_a_lock_is_checked_after_resolve(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    _two_member_plugin(project_dir, registry, unique_repo)
    runner = grim_at(project_dir)
    result = runner.run(
        "--format", "json", "update", "--marketplace", "marketplace.toml", "a:ghost", check=False
    )
    assert result.returncode == 79, result.stderr
    assert not (project_dir / "marketplace.lock").exists(), "nothing is saved on a 79"


def test_s023_undeclared_member_with_an_existing_lock_matches_the_no_lock_message(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # Regression: with a fresh part present the resolver's undeclared-name
    # refusal leaked its placeholder identity (`invalid.localhost`).
    _two_member_plugin(project_dir, registry, unique_repo)
    runner = grim_at(project_dir)
    args = ("--format", "json", "update", "--marketplace", "marketplace.toml", "a:ghost")
    without_lock = runner.run(*args, check=False)
    runner.run("update", "--marketplace", "marketplace.toml")
    with_lock = runner.run(*args, check=False)

    assert with_lock.returncode == 79, with_lock.stderr
    for out in (with_lock.stdout, with_lock.stderr):
        assert "invalid.localhost" not in out, out
    assert _error(with_lock)["message"] == _error(without_lock)["message"]
    assert "a:ghost" in _error(with_lock)["message"]


def test_s023_stale_refusal_names_the_marketplace_update_command(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    plan, other = _two_member_plugin(project_dir, registry, unique_repo)
    runner = grim_at(project_dir)
    runner.run("update", "--marketplace", "marketplace.toml")
    extra = f"{unique_repo}/extra"
    _skill(extra, "extra", "v1")
    _write_marketplace(
        project_dir / "marketplace.toml",
        {"a": [f"{registry}/{plan}:stable", f"{registry}/{other}:stable", f"{registry}/{extra}:stable"]},
    )

    # An undeclared member on a stale part: still the stale refusal, no
    # placeholder identity, and the retry names a runnable command.
    result = runner.run(
        "--format", "json", "update", "--marketplace", "marketplace.toml", "a:ghost", check=False
    )
    assert result.returncode == 65, result.stderr
    message = _error(result)["message"]
    assert "invalid.localhost" not in result.stdout + result.stderr, message
    assert "grim update --marketplace" in message and message.rstrip().endswith("a`"), message


@pytest.mark.parametrize(
    "extra",
    [["--client", "claude"], ["--force"]],
    ids=["client", "force"],
)
def test_s023_install_only_flags_conflict_with_marketplace(
    grim_at, project_dir: Path, registry: str, unique_repo: str, extra: list[str]
) -> None:
    _two_member_plugin(project_dir, registry, unique_repo)
    runner = grim_at(project_dir)
    result = runner.run("update", "--marketplace", "marketplace.toml", *extra, check=False)
    assert result.returncode == 64, result.stderr
    assert not (project_dir / "marketplace.lock").exists()


# ── S-024 — lock flavors reject each other ──────────────────────────────

_GRIMOIRE_LOCK_METADATA = (
    "[metadata]\n"
    "lock_version = 1\n"
    "declaration_hash_version = 1\n"
    f'declaration_hash = "sha256:{"0" * 64}"\n'
    'generated_by = "grim test"\n'
    'generated_at = "2026-01-01T00:00:00Z"\n'
)


def _plugin_scoped_grimoire_locks(pinned: str) -> dict[str, str]:
    return {
        "entry-plugin": (
            f'{_GRIMOIRE_LOCK_METADATA}\n[[skill]]\nname = "code-review"\n'
            f'plugin = "x"\npinned = "{pinned}"\n'
        ),
        "plugin-table": (
            f'{_GRIMOIRE_LOCK_METADATA}\n[[plugin]]\nname = "x"\n'
            f'declaration_hash = "sha256:{"1" * 64}"\n'
        ),
    }


@pytest.mark.parametrize("flavor", ["entry-plugin", "plugin-table"])
def test_s024_grimoire_lock_with_plugin_scope_is_refused_by_readers(
    grim_at, project_dir: Path, registry: str, unique_repo: str, flavor: str
) -> None:
    repo = f"{unique_repo}/code-review"
    art = _skill(repo, "code-review", "v1")
    write_config(project_dir, skills={"code-review": f"{registry}/{repo}:stable"})
    bad = _plugin_scoped_grimoire_locks(art.pinned)[flavor]
    lock_path = project_dir / "grimoire.lock"
    runner = grim_at(project_dir)

    for argv in (["install"], ["status"]):
        lock_path.write_text(bad)
        result = runner.run("--format", "json", *argv, check=False)
        assert result.returncode == 78, f"{argv}: {result.stderr}"
        assert _error(result)["exit"] == 78, argv

    lock_path.write_text(bad)
    ctx = runner.run("--format", "json", "context", check=False)
    assert ctx.returncode == 0, ctx.stderr
    assert json.loads(ctx.stdout)["lock_error"], "context reports the flavor mismatch"


@pytest.mark.parametrize("flavor", ["entry-plugin", "plugin-table"])
def test_s024_lock_writers_treat_a_plugin_scoped_grimoire_lock_as_absent(
    grim_at, project_dir: Path, registry: str, unique_repo: str, flavor: str
) -> None:
    repo = f"{unique_repo}/code-review"
    art = _skill(repo, "code-review", "v1")
    add_repo = f"{unique_repo}/added"
    added = _skill(add_repo, "added", "v1")
    write_config(project_dir, skills={"code-review": f"{registry}/{repo}:stable"})
    bad = _plugin_scoped_grimoire_locks(art.pinned)[flavor]
    lock_path = project_dir / "grimoire.lock"
    runner = grim_at(project_dir)

    for argv in (["lock"], ["update"], ["add", added.fq]):
        lock_path.write_text(bad)
        result = runner.run(*argv, check=False)
        assert result.returncode == 0, f"{argv}: {result.stderr}"
        rewritten = _lock(lock_path)
        assert "plugin" not in rewritten, f"{argv} re-resolved into a plain grimoire.lock"
        assert all("plugin" not in e for e in rewritten.get("skill", [])), argv


_MKT_METADATA = (
    "[metadata]\n"
    "lock_version = 1\n"
    "declaration_hash_version = 1\n"
    f'declaration_hash = "sha256:{"0" * 64}"\n'
    'generated_by = "grim test"\n'
    'generated_at = "2026-01-01T00:00:00Z"\n'
    f'\n[[plugin]]\nname = "team"\ndeclaration_hash = "sha256:{"1" * 64}"\n'
)


@pytest.mark.parametrize("flavor", ["entry-without-plugin", "bundle-table"])
def test_s024_marketplace_lock_of_the_wrong_flavor_is_78(
    grim_at, project_dir: Path, registry: str, unique_repo: str, flavor: str
) -> None:
    repo = f"{unique_repo}/team-plan"
    art = _skill(repo, "team-plan", "v1")
    _write_marketplace(project_dir / "marketplace.toml", {"team": [f"{registry}/{repo}:stable"]})
    body = {
        "entry-without-plugin": f'{_MKT_METADATA}\n[[skill]]\nname = "team-plan"\npinned = "{art.pinned}"\n',
        "bundle-table": (
            f'{_MKT_METADATA}\n[[skill]]\nname = "team-plan"\nplugin = "team"\npinned = "{art.pinned}"\n'
            f'\n[[bundle]]\nname = "stack"\nrepo = "{registry}/{unique_repo}/stack"\ntag = "1"\n'
            f'pinned = "{registry}/{unique_repo}/stack@sha256:{"f" * 64}"\n'
            f'\n[[bundle.member]]\nkind = "skill"\nname = "team-plan"\nid = "{registry}/{repo}:stable"\n'
        ),
    }[flavor]
    lock_path = project_dir / "marketplace.lock"
    lock_path.write_text(body)
    runner = grim_at(project_dir)

    out = project_dir / "out"
    for argv in (
        ["update", "--marketplace", "marketplace.toml"],
        ["export", "plugin", "--plugin", "team", "--client", "claude", "-o", str(out)],
    ):
        result = runner.run("--format", "json", *argv, check=False)
        assert result.returncode == 78, f"{argv}: {result.stderr}"
        assert _error(result)["exit"] == 78, argv
        assert lock_path.read_text() == body, f"{argv}: a refused lock is never rewritten"
    assert not any(out.glob("team.*")), "a refused lock exports nothing"


# ── S-025 / C-031.3 — normal update report is additive ─────────────────


def test_s025_normal_update_rows_gain_only_a_null_plugin(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    s_repo, r_repo = f"{unique_repo}/code-review", f"{unique_repo}/rust-style"
    _skill(s_repo, "code-review", "v1")
    _rule(r_repo, "rust-style", "v1")
    write_config(
        project_dir,
        skills={"code-review": f"{registry}/{s_repo}:stable"},
        rules={"rust-style": f"{registry}/{r_repo}:stable"},
    )
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    _skill(s_repo, "code-review", "v2")

    rows = runner.json("update")["items"]
    assert len(rows) == 2, rows
    for row in rows:
        assert list(row) == [*_PRE_C013_KEYS, "plugin"], f"previous keys unchanged, plugin last: {row}"
        assert row["plugin"] is None, row


# ── S-027 — lock contention ─────────────────────────────────────────────


@unix_only
def test_s027_held_marketplace_lock_is_75_locked_retryable(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    repo = f"{unique_repo}/team-plan"
    _skill(repo, "team-plan", "v1")
    _write_marketplace(project_dir / "marketplace.toml", {"team": [f"{registry}/{repo}:stable"]})
    runner = grim_at(project_dir)

    # C-010: the advisory sidecar is `<M>.lock`, never L itself.
    with _held_flock(project_dir / "marketplace.toml.lock"):
        result = runner.run(
            "--format", "json", "update", "--marketplace", "marketplace.toml", check=False
        )
    assert result.returncode == 75, result.stderr
    err = _error(result)
    assert (err["exit"], err["reason"], err["retryable"]) == (75, "locked", True), err
    assert not (project_dir / "marketplace.lock").exists()
