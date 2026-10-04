# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Port contracts of the hooks revival (`design_hooks_pr98_revival.md`) that
only the real binary can show: C-105 (global half), C-153 (`config list`),
C-154, C-155/S-120, S-110, S-111, S-112, S-114 and S-116.

Every "nothing armed" negative is paired with a positive control that arms the
same artifact, so a build that arms nothing at all cannot pass.
"""
from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from src.helpers import make_artifact, write_config

# Only the tests that actually arm and inspect the launcher/registration are
# POSIX-only in v1 — C-153, S-110 and S-112 in this module write or arm no
# hook surface at all, so the skip is per-test rather than module-wide
# (matches `test_bundle_hook_members.py`).
_POSIX_ONLY = pytest.mark.skipif(
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
command = "python3 guard.py"
timeout = 5
"""

GUARD_PY = "import sys\nsys.stdin.read()\nprint('{}')\n"

MARKER_VALUE = "hook-dispatcher"


def _publish_hook(unique_repo: str, extra: dict[str, str] | None = None):
    files = {"shell-guard/hook.toml": HOOK_TOML, "shell-guard/guard.py": GUARD_PY}
    files.update(extra or {})
    return make_artifact(f"{unique_repo}/shell-guard", "hook", files, tag="1")


def _rows(runner) -> list[dict]:
    path = Path(runner.grim_home) / "hooks" / "dispatch.json"
    if not path.is_file():
        return []
    return [row for root in json.loads(path.read_text())["roots"].values() for row in root["hooks"]]


def _claude_marked(project_dir: Path) -> bool:
    surface = project_dir / ".claude" / "settings.local.json"
    return surface.is_file() and MARKER_VALUE in surface.read_text()


def _hook_item(runner) -> dict:
    return next(i for i in runner.json("status")["items"] if i["kind"] == "hook")


def _armed_project(
    grim_at, project_dir: Path, unique_repo: str, extra: dict[str, str] | None = None, *, consent: bool = False
):
    """Arm `shell-guard`, by `--trust-hooks` or, with `consent`, by a recorded
    `grim hook allow` — the only one that persists to a later plain install."""
    hook = _publish_hook(unique_repo, extra)
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    if consent:
        runner.run("hook", "allow")
        runner.run("install")
    else:
        runner.run("install", "--trust-hooks")
    assert len(_rows(runner)) == 1 and _claude_marked(project_dir), "precondition: the hook must arm"
    return runner


def _snapshot(*roots: Path, skip: Path | None = None) -> list[tuple[str, int, bytes]]:
    out = []
    for root in roots:
        for path in sorted(root.rglob("*")):
            if skip is not None and path.is_relative_to(skip):
                continue
            stat = path.lstat()
            out.append((str(path), stat.st_mtime_ns, path.read_bytes() if path.is_file() else b""))
    return out


# ---------------------------------------------------------------------------
# C-105 — the hook-free short circuit, global half
# ---------------------------------------------------------------------------


@_POSIX_ONLY
def test_c105_hook_free_global_converge_writes_nothing(grim, registry: str, unique_repo: str) -> None:
    """**C-105 / S-101.** Global scope, feature on, `--trust-hooks`, every hook
    client detected under `$HOME`, nothing hook-shaped declared: a converging
    `grim install` writes no file anywhere under `$HOME` or `$GRIM_HOME`.

    The first install materializes the skill; the snapshot brackets the second,
    whose only possible writes are hook convergence (the skill is unchanged).
    `$GRIM_HOME/state` is left out: main's install already rewrites
    `global.json` on every run, which is not a hook write.
    """
    skill = make_artifact(f"{unique_repo}/demo", "skill", {"demo/SKILL.md": "---\nname: demo\n---\n"})
    home = Path(grim.home)
    for marker in (".claude", ".codex", ".copilot", ".qoder"):
        (home / marker).mkdir(parents=True, exist_ok=True)
    (home / ".claude" / "settings.json").write_text('{"permissions": {}}')
    grim_home = Path(grim.grim_home)
    (grim_home / "grimoire.toml").write_text(
        f'[skills]\ndemo = "{skill.fq}"\n\n[options.experimental]\nhooks = true\n'
    )
    grim.run("lock", "--global")
    grim.run("install", "--global", "--trust-hooks")
    before = _snapshot(home, grim_home, skip=grim_home / "state")

    grim.run("install", "--global", "--trust-hooks")

    assert _snapshot(home, grim_home, skip=grim_home / "state") == before, "a hook-free converge must not write a single file"
    assert not (grim_home / "hooks").exists(), "no launcher, no dispatch table, no token key"


# ---------------------------------------------------------------------------
# C-153 — the appended `config list` row
# ---------------------------------------------------------------------------


def test_c153_config_list_adds_the_experimental_hooks_row(grim_at, project_dir: Path) -> None:
    """**C-153.** `options.experimental.hooks` is row 11 of `grim config list
    --all` — appended to the fixed option keys, after
    `options.search_min_relevance` and before the `plugin.*` table keys — a
    boolean defaulting to false, with the design record's description
    verbatim. No other row is added, removed or reordered."""
    write_config(project_dir)
    runner = grim_at(project_dir)
    items = runner.json("config", "list", "--all")["items"]
    keys = [i["key"] for i in items]
    assert keys[:10] + keys[11:] == [
        "options.default_registry",
        "options.clients",
        "options.show_deprecated",
        "options.tui.default_view",
        "options.tui.group_by_type",
        "options.tui.tree_separators",
        "options.tui.expand_levels",
        "options.tui.sort",
        "options.tui.sort_order",
        "options.search_min_relevance",
        "plugin.name",
        "plugin.description",
        "plugin.version",
        "plugin.logo",
    ], keys
    row = items[10]
    assert row["key"] == "options.experimental.hooks"
    assert row["title"] == "Experimental hooks"
    assert row["type"] == "boolean"
    assert row["default"] == "false"
    assert row["description"] == (
        "Controls whether `grim install` arms declared hooks in your AI clients. When unset, install "
        "arms nothing and removes grim's existing hook registrations; no environment variable "
        "overrides this."
    )


# ---------------------------------------------------------------------------
# C-154 — no environment variable selects or consents to hooks
# ---------------------------------------------------------------------------


@_POSIX_ONLY
@pytest.mark.parametrize("flag", [False, True], ids=["flag-off", "flag-on-unconsented"])
def test_c154_hook_environment_variables_are_inert(
    grim_at, project_dir: Path, registry: str, unique_repo: str, flag: bool
) -> None:
    """**C-154.** `GRIM_EXPERIMENTAL_HOOKS` and `GRIM_ALLOW_HOOKS` do nothing:
    with the flag off the hook stays `feature-flag-off`; with the flag on in an
    unconsented workspace it stays `workspace-not-consented`. Positive control:
    `--trust-hooks` arms the same hook in the same workspace."""
    hook = _publish_hook(unique_repo)
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    if flag:
        runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    runner.env.update({"GRIM_EXPERIMENTAL_HOOKS": "1", "GRIM_ALLOW_HOOKS": "1"})

    result = runner.run("install", check=False)
    assert result.returncode == 0, result.stderr
    assert _rows(runner) == [] and not _claude_marked(project_dir), "an env var armed a hook"
    expected = "workspace-not-consented" if flag else "feature-flag-off"
    assert [a["cause"] for a in _hook_item(runner)["arming"]] == [expected]

    if flag:
        runner.run("install", "--trust-hooks")
        assert len(_rows(runner)) == 1, "POSITIVE CONTROL FAILED: --trust-hooks must arm"


# ---------------------------------------------------------------------------
# C-155 / S-120 — `grim remove` defers the disarm, and says so
# ---------------------------------------------------------------------------


@_POSIX_ONLY
def test_s120_remove_of_an_armed_hook_warns_and_defers_the_disarm(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """**C-155 / S-120.** `grim remove hook` drops the declaration and the lock
    entry, exits 0, and warns on stderr that the hook stays armed until
    `grim install` or `grim uninstall`. The claude element really is still
    there; the next `grim install` removes it. Consented, so the disarm is the
    lock's doing and not a missing grant's."""
    runner = _armed_project(grim_at, project_dir, unique_repo, consent=True)

    result = runner.run("remove", "hook", "shell-guard", check=False)
    assert result.returncode == 0, result.stderr
    assert "shell-guard" in result.stderr, result.stderr
    assert "stays armed until `grim install` or `grim uninstall`" in result.stderr, result.stderr
    assert "shell-guard" not in (project_dir / "grimoire.toml").read_text()
    assert _claude_marked(project_dir), "remove is not a converging seam: the element must still be there"
    assert len(_rows(runner)) == 1

    runner.run("install")
    assert not _claude_marked(project_dir), "the next install must reap the registration"
    assert _rows(runner) == []


# ---------------------------------------------------------------------------
# S-110 / S-111 — no path source, no reserved binding
# ---------------------------------------------------------------------------


def test_s110_a_path_source_hook_is_refused_and_writes_nothing(grim_at, project_dir: Path) -> None:
    """**S-110.** `grim add ./dir --kind hook` is the existing
    `UnsupportedPathKind` (64); without `--kind`, a directory holding only
    `hook.toml` is the existing `UninferablePathKind` (64). Nothing is written."""
    write_config(project_dir)
    config_before = (project_dir / "grimoire.toml").read_text()
    (project_dir / "my-hook").mkdir()
    (project_dir / "my-hook" / "hook.toml").write_text(HOOK_TOML)
    runner = grim_at(project_dir)

    for args in (("add", "./my-hook", "--kind", "hook"), ("add", "./my-hook")):
        result = runner.run(*args, check=False)
        assert result.returncode == 64, f"{args}: rc={result.returncode} {result.stderr}"
    assert (project_dir / "grimoire.toml").read_text() == config_before
    assert not (project_dir / "grimoire.lock").exists()


@_POSIX_ONLY
def test_s111_a_reserved_hook_binding_is_refused_by_add(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """**S-111.** `grim add <hook> --name bin` exits 64 with the config
    untouched; a hand-edited `[hooks] bin = …` is skipped by `install` with a
    warning (exit 0) and arms nothing. Positive control: the same artifact
    under a valid binding adds."""
    hook = _publish_hook(unique_repo)
    write_config(project_dir)
    config_before = (project_dir / "grimoire.toml").read_text()
    runner = grim_at(project_dir)

    refused = runner.run("add", hook.fq, "--name", "bin", check=False)
    assert refused.returncode == 64, refused.stderr
    assert (project_dir / "grimoire.toml").read_text() == config_before

    write_config(project_dir, hooks={"bin": hook.fq})
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    runner.run("hook", "allow")
    install = runner.run("install", "--trust-hooks", check=False)
    assert install.returncode == 0, install.stderr
    assert "reserved" in install.stderr, install.stderr
    assert _rows(runner) == [] and not _claude_marked(project_dir)
    assert _hook_item(runner)["state"] == "missing", "a reserved binding never materializes, so status reports it missing"

    write_config(project_dir)
    runner.run("add", hook.fq, "--name", "guard")
    assert "guard" in (project_dir / "grimoire.toml").read_text(), "POSITIVE CONTROL FAILED"


# ---------------------------------------------------------------------------
# S-112 — an older grim meets a hooks-bearing lock
# ---------------------------------------------------------------------------


def test_s112_an_unknown_lock_table_exits_78(grim_at, project_dir: Path, registry: str, unique_repo: str) -> None:
    """**S-112**, the half one binary can run. A pre-hooks grim reads
    `[[hook]]` as an unknown lock table, and every unknown lock table is
    refused with 78 (`EX_CONFIG`) — `RawLock` is `deny_unknown_fields`. Proven
    here by renaming this grim's `[[hook]]` to a table it does not know, which
    is exactly the position an older grim is in."""
    hook = _publish_hook(unique_repo)
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    lock = project_dir / "grimoire.lock"
    assert "[[hook]]" in lock.read_text(), "precondition: the lock carries the hook table"
    lock.write_text(lock.read_text().replace("[[hook]]", "[[hook-from-the-future]]"))

    result = runner.run("status", check=False)
    assert result.returncode == 78, f"rc={result.returncode} {result.stderr}"


# ---------------------------------------------------------------------------
# S-114 — a relative `--config` isolates repositories
# ---------------------------------------------------------------------------


@_POSIX_ONLY
def test_s114_a_relative_config_consent_does_not_arm_a_sibling_repo(
    two_projects, registry: str, unique_repo: str
) -> None:
    """**S-114 / C-118.** `grim --config grimoire.toml hook allow` in repo A,
    then `grim --config grimoire.toml install` in repo B (same `$GRIM_HOME`,
    same relative spelling): B stays unconsented and arms nothing, while A arms."""
    hook = _publish_hook(unique_repo)
    runner_a, runner_b = two_projects
    for runner in (runner_a, runner_b):
        write_config(runner.cwd, hooks={"shell-guard": hook.fq})
        runner.run("--config", "grimoire.toml", "config", "set", "options.experimental.hooks", "true")
        runner.run("--config", "grimoire.toml", "lock")

    runner_a.run("--config", "grimoire.toml", "hook", "allow")
    runner_a.run("--config", "grimoire.toml", "install")
    runner_b.run("--config", "grimoire.toml", "install")

    assert _claude_marked(runner_a.cwd), "POSITIVE CONTROL FAILED: repo A must arm after its own allow"
    assert not _claude_marked(runner_b.cwd), "repo A's consent armed repo B"
    b_item = next(
        i for i in runner_b.json("--config", "grimoire.toml", "status")["items"] if i["kind"] == "hook"
    )
    assert [a["cause"] for a in b_item["arming"]] == ["workspace-not-consented"], b_item


# ---------------------------------------------------------------------------
# S-116 / C-160 — runtime byproducts and hand edits in the payload
# ---------------------------------------------------------------------------


@_POSIX_ONLY
def test_s116_pycache_is_not_drift_but_a_hook_toml_edit_is(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """**S-116 / C-160.** The payload's `.grimignore` says `*.toml`, yet
    `hook.toml` is still hashed: a `__pycache__/` byproduct leaves the row
    `installed`, an edit to `hook.toml` flips it to `modified`, and `install`
    refuses without `--force` exactly as it does for a skill (the row is
    marked refused, the edit survives)."""
    runner = _armed_project(grim_at, project_dir, unique_repo, {"shell-guard/.grimignore": "*.toml\n"})
    payloads = list((Path(runner.grim_home) / "hooks").glob("payload/*/shell-guard"))
    assert len(payloads) == 1 and (payloads[0] / "hook.toml").is_file(), payloads
    payload = payloads[0]
    assert _hook_item(runner)["state"] == "installed", _hook_item(runner)

    (payload / "__pycache__").mkdir()
    (payload / "__pycache__" / "guard.cpython-313.pyc").write_bytes(b"\x00byproduct")
    assert _hook_item(runner)["state"] == "installed", _hook_item(runner)

    (payload / "hook.toml").write_text(HOOK_TOML + "# hand edit\n")
    assert _hook_item(runner)["state"] == "modified", _hook_item(runner)

    # `--trust-hooks`: the integrity gate sits after the hook gates (C-101), so
    # only an install that would arm reaches it.
    refused = runner.run("install", "--trust-hooks", check=False)
    assert refused.returncode == 65, f"as for skills: rc={refused.returncode} {refused.stderr}"
    assert "shell-guard" in refused.stderr, refused.stderr
    assert (payload / "hook.toml").read_text().endswith("# hand edit\n"), "the refusal must keep the edit"


@_POSIX_ONLY
def test_s116_a_real_python_run_leaves_pycache_that_is_neither_drift_nor_a_disarm(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """**S-116, end to end.** The dispatcher runs `python3 guard.py` from the
    payload directory; `guard.py` imports a sibling module, so CPython writes
    `__pycache__/helper.*.pyc` into the installed payload. Status, the install
    integrity gate and the arming re-hash all use the same filtered hash, so
    the row stays `installed` and a re-converging install keeps the hook armed.
    Negative control: an edit to the imported module is still drift."""
    runner = _armed_project(
        grim_at,
        project_dir,
        unique_repo,
        {"shell-guard/guard.py": "import sys\nimport helper\nsys.stdin.read()\nprint('{}')\n",
         "shell-guard/helper.py": "X = 1\n"},
    )
    grim_home = Path(runner.grim_home)
    table = grim_home / "hooks" / "dispatch.json"
    [token] = json.loads(table.read_text())["roots"]
    [payload] = list((grim_home / "hooks").glob("payload/*/shell-guard"))

    fired = runner.run(
        "hook", "run", "--client", "claude", "--event", "PreToolUse", "--table", str(table), "--root", token,
        stdin=json.dumps({"hook_event_name": "PreToolUse", "tool_name": "Bash",
                          "tool_input": {"command": "ls"}, "cwd": str(project_dir), "session_id": "s"}),
        check=False,
    )
    assert fired.returncode == 0, fired.stderr
    assert list(payload.glob("__pycache__/helper.*.pyc")), f"the handler must have run and imported helper: {fired.stderr}"

    item = _hook_item(runner)
    assert item["state"] == "installed" and item["arming"] == [], item
    again = runner.run("install", "--trust-hooks", check=False)
    assert again.returncode == 0, again.stderr
    assert "modified" not in again.stderr, again.stderr
    assert len(_rows(runner)) == 1 and _claude_marked(project_dir), "the byproduct must not disarm the hook"

    (payload / "helper.py").write_text("X = 2\n")
    assert _hook_item(runner)["state"] == "modified", "NEGATIVE CONTROL: a real source edit must still be drift"


# ---------------------------------------------------------------------------
# Rule 11 — a handler may not reference a file the pack leaves out
# ---------------------------------------------------------------------------


def _hook_dir(root: Path, files: dict[str, str]) -> Path:
    hook = root / "shell-guard"
    for rel, body in {"hook.toml": HOOK_TOML, **files}.items():
        (hook / rel).parent.mkdir(parents=True, exist_ok=True)
        (hook / rel).write_text(body)
    return hook


@pytest.mark.parametrize(
    "files,handler,path,reinclude",
    [
        ({"guard.py~": "x"}, 'command = "python3 guard.py~"', "guard.py~", "!guard.py~\n"),
        ({".venv/lib/dep.py": "x"}, 'argv = ["python3", "${GRIM_HOOK_DIR}/.venv/lib/dep.py"]', ".venv/lib/dep.py",
         "!.venv/\n"),
        ({"scripts/node_modules/dep/index.js": "x"}, 'argv = ["node", "scripts/node_modules/dep/index.js"]',
         "scripts/node_modules/dep/index.js", "!node_modules/\n"),
        ({"vendor/dep.js": "x", ".grimignore": "vendor/\n"}, 'argv = ["node", "$GRIM_HOOK_DIR/vendor/dep.js"]',
         "vendor/dep.js", ""),
    ],
    ids=["default-file", "default-dir", "nested-default-dir", "authored-grimignore"],
)
def test_build_refuses_a_handler_that_references_an_ignored_file(
    grim_at, project_dir: Path, files: dict[str, str], handler: str, path: str, reinclude: str
) -> None:
    """**Rule 11.** A handler naming a payload file the pack walk excludes is a
    65 at `grim build`, naming the path — never a published hook whose handler
    finds nothing at run time. Positive control: a `!` re-include builds."""
    hook = _hook_dir(project_dir, files)
    (hook / "hook.toml").write_text(HOOK_TOML.replace('command = "python3 guard.py"', handler))
    runner = grim_at(project_dir)

    refused = runner.run("build", str(hook), check=False)
    assert refused.returncode == 65, refused.stderr
    assert path in refused.stderr and ".grimignore" in refused.stderr, refused.stderr

    (hook / ".grimignore").write_text(reinclude)
    assert runner.json("build", str(hook))["kind"] == "hook", "POSITIVE CONTROL FAILED: a re-included file must build"


def test_build_warns_when_a_hook_leaves_a_dependency_dir_behind(grim_at, project_dir: Path) -> None:
    """An unreferenced `node_modules/` (or `.venv/`) is not provably needed, so
    it is a warning rather than a refusal: the build succeeds and names the
    directory. Negative control: the same tree built as a skill stays silent."""
    hook = _hook_dir(project_dir, {"guard.py": "print('{}')\n", "node_modules/dep/index.js": "x"})
    runner = grim_at(project_dir)

    built = runner.run("build", str(hook), format="json", check=False)
    assert built.returncode == 0, built.stderr
    assert "node_modules" in built.stderr, built.stderr

    (hook / "SKILL.md").write_text("---\nname: shell-guard\ndescription: d\n---\n")
    skill = runner.run("build", str(hook), format="json", check=False)
    assert skill.returncode == 0 and json.loads(skill.stdout)["kind"] == "skill", skill.stderr
    assert "node_modules" not in skill.stderr, "hook-free builds must stay byte-identical (Principle 9)"
