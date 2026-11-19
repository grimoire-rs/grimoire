# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Load probe: do the real clients still load what grim installs for a ``mod``?

Opt-in. ``GRIM_REAL_CLAUDE_BIN`` and ``GRIM_REAL_OPENCODE_BIN`` name the client
binaries; each probe skips with its reason while its variable is unset, so the
ordinary run skips every one. Linux only: elsewhere a client can reach the real
login, so every probe skips there too.

A probe mod writes a marker file when its hook fires on ``session.start``. grim
installs it (mods flag on, workspace consented) into a scratch project or the
scratch global roots, a client runs one headless session, and the marker is
polled for up to 20 s. Marker present: pass. Marker absent after the client
exited by itself: fail, the signal that a client changed how it loads mods. A
hang (killed at the hard timeout) or an absent client: skip with the reason.
Neither client logs in or sends a model request: Claude stops at "Not logged
in" after its plugins load, and OpenCode fails on the unknown model ``none/none``.

Probe items (the numbers appear in the test names):

1. a name-only ``plugin.json`` loads, in both scopes
2. an absolute path in ``settings.local.json`` loads in the first session
3. an untrusted project does not load that registration; its trusted twin does
4. a user-scope registration loads in a global session
5. which name keys a directory marketplace, the settings key or the manifest
   ``name`` (observation, recorded as a junit property)
6. whether one marketplace name declared by two projects crosses over
   (observation, recorded as a junit property)
7. the extensionless re-export of a ``.js`` entry, OpenCode, both scopes
8. to 10. need an interactive session or another client: each skips with its reason

The resolved client versions go into the junit properties of every probe item.

The child environment is built from nothing: an allowlist of keys, every root
under the test's tmp dir, plus the clients' documented opt-outs. Names checked
against upstream docs on 2026-11-19:

- ``DO_NOT_TRACK``, ``DISABLE_TELEMETRY``, ``DISABLE_ERROR_REPORTING``,
  ``DISABLE_AUTOUPDATER``, ``DISABLE_UPDATES`` and
  ``CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC``:
  https://code.claude.com/docs/en/env-vars (Claude Code environment variables)
- OpenCode documents no environment opt-out for updates or sharing; its scratch
  config sets the ``autoupdate`` and ``share`` keys instead, per the published
  config schema https://opencode.ai/config.json (checked 2026-11-19):
  ``autoupdate: false`` stops update checks, ``share: "disabled"`` turns sharing off.

Claude trust is seeded only in the scratch ``$CLAUDE_CONFIG_DIR/.claude.json``
(documented key ``projects["<path>"].hasTrustDialogAccepted``).
"""

from __future__ import annotations

import json
import os
import re
import signal
import subprocess
import sys
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

import pytest

from src.helpers import make_mod, mod_sandbox, write_config

CLAUDE_VAR = "GRIM_REAL_CLAUDE_BIN"
OPENCODE_VAR = "GRIM_REAL_OPENCODE_BIN"

MARKER_POLL_SECONDS = 20.0
CLAUDE_TIMEOUT = 90.0
OPENCODE_TIMEOUT = 120.0

# Documented opt-outs, each a fixed literal (names and sources in the docstring).
_OPT_OUTS = {
    "DO_NOT_TRACK": "1",
    "DISABLE_TELEMETRY": "1",
    "DISABLE_ERROR_REPORTING": "1",
    "DISABLE_AUTOUPDATER": "1",
    "DISABLE_UPDATES": "1",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
}
_ROOT_KEYS = {
    "HOME",
    "TMPDIR",
    "CLAUDE_CONFIG_DIR",
    "OPENCODE_CONFIG_DIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
}
_ALLOWED_KEYS = {"PATH", *_ROOT_KEYS, *_OPT_OUTS}

_TRUST_PROMPT = re.compile(
    r"trust (this|the) (folder|directory|dialog)|do you trust", re.IGNORECASE
)

WORKFLOW = (
    Path(__file__).resolve().parents[2] / ".github" / "workflows" / "verify-deep.yml"
)


# ---------------------------------------------------------------------------
# Opt-in gate and scratch environment
# ---------------------------------------------------------------------------


def _isolation_gap(platform: str) -> str | None:
    """Why a scratch environment does not isolate a client on `platform`."""
    if platform.startswith("linux"):
        return None
    return (
        f"{platform}: a client can reach the real login there (the OS credential store, or a "
        "home resolved without HOME), so the load probe runs on Linux only"
    )


def _binary(variable: str) -> Path:
    value = os.environ.get(variable)
    if not value:
        pytest.skip(f"{variable} is unset: the real-client load probe is opt-in")
    if gap := _isolation_gap(sys.platform):
        pytest.skip(gap)
    path = Path(value)
    if not (path.is_file() and os.access(path, os.X_OK)):
        pytest.skip(f"{variable} does not name an executable file")
    return path


@pytest.fixture
def claude_bin() -> Path:
    return _binary(CLAUDE_VAR)


@pytest.fixture
def opencode_bin() -> Path:
    return _binary(OPENCODE_VAR)


def _child_env(binary: Path, home: Path, roots: dict[str, Path]) -> dict[str, str]:
    """A child environment assembled from nothing, every root under the test's tmp dir."""
    env = {
        "PATH": f"{binary.parent}:/usr/bin:/bin",
        "HOME": str(home),
        "TMPDIR": str(home / "tmp"),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "XDG_DATA_HOME": str(home / ".local" / "share"),
        "XDG_CACHE_HOME": str(home / ".cache"),
        "XDG_STATE_HOME": str(home / ".local" / "state"),
        "CLAUDE_CONFIG_DIR": str(roots["claude"]),
        "OPENCODE_CONFIG_DIR": str(roots["opencode"]),
        **_OPT_OUTS,
    }
    for key in (
        "TMPDIR",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
    ):
        Path(env[key]).mkdir(parents=True, exist_ok=True)
    _assert_isolated(env, home.parent)
    return env


def _assert_isolated(env: dict[str, str], anchor: Path) -> None:
    """Every key allowlisted, every opt-out its literal, every root under `anchor`."""
    assert set(env) <= _ALLOWED_KEYS, sorted(set(env) - _ALLOWED_KEYS)
    altered = sorted(key for key, value in _OPT_OUTS.items() if env.get(key) != value)
    assert altered == [], f"opt-outs must hold their literals: {altered}"
    # A child without HOME resolves the real one from the password database.
    assert {"HOME", "TMPDIR"} <= set(env), "HOME and TMPDIR must be set"
    for key in sorted(_ROOT_KEYS & set(env)):
        root = Path(env[key])
        assert root.is_absolute(), f"{key} must be absolute"
        assert root.resolve().is_relative_to(anchor.resolve()), (
            f"{key} must sit under the test's tmp dir"
        )


def test_child_env_is_built_from_scratch(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    """Nothing of the parent environment reaches a client, and every root is scratch."""
    for key, value in {
        "ANTHROPIC_API_KEY": "sk-leak",
        "GH_TOKEN": "leak",
        "XDG_CONFIG_HOME": "/real",
    }.items():
        monkeypatch.setenv(key, value)
    base = tmp_path / "scratch"
    _, roots = mod_sandbox(base, home=base / "home")
    stand_in = tmp_path / "bin" / "claude"
    env = _child_env(stand_in, base / "home", roots)
    assert set(env) <= _ALLOWED_KEYS
    assert env["DO_NOT_TRACK"] == "1" and env["DISABLE_AUTOUPDATER"] == "1"
    assert env["CLAUDE_CONFIG_DIR"] == str(roots["claude"])
    assert env["XDG_CONFIG_HOME"] != "/real"


@pytest.mark.parametrize(
    "plant",
    [
        {"ANTHROPIC_API_KEY": "sk-leak"},
        {"DO_NOT_TRACK": "0"},
        {"CLAUDE_CONFIG_DIR": "/real/.claude"},
        {"OPENCODE_CONFIG_DIR": "relative/dir"},
    ],
    ids=["extra-key", "opt-out-altered", "root-outside-tmp", "root-relative"],
)
def test_a_planted_leak_fails_the_check(plant: dict[str, str], tmp_path: Path) -> None:
    base = tmp_path / "scratch"
    _, roots = mod_sandbox(base, home=base / "home")
    env = _child_env(tmp_path / "bin" / "claude", base / "home", roots)
    env.update(plant)
    with pytest.raises(AssertionError):
        _assert_isolated(env, base)


@pytest.mark.parametrize("platform", ["darwin", "win32", "freebsd14"])
def test_probe_runs_on_linux_only(
    platform: str, monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    stand_in = tmp_path / "client"
    stand_in.write_text("#!/bin/sh\n", encoding="utf-8", newline="\n")
    stand_in.chmod(0o755)
    monkeypatch.setenv(CLAUDE_VAR, str(stand_in))
    monkeypatch.setattr(sys, "platform", platform)
    with pytest.raises(pytest.skip.Exception, match="Linux only"):
        _binary(CLAUDE_VAR)
    assert _isolation_gap("linux") is None


# ---------------------------------------------------------------------------
# Running a client
# ---------------------------------------------------------------------------


@dataclass
class Session:
    """What one headless client session left behind."""

    marker: Path
    debug: Path | None
    output: str

    @property
    def loaded(self) -> bool:
        return self.marker.exists()


def _run_bounded(
    argv: list[str], env: dict[str, str], cwd: Path, timeout: float
) -> tuple[bool, str]:
    """Run `argv` in its own process group; kill the group on timeout or after exit.

    Returns ``(exited_by_itself, output)``. OpenCode can leave a background
    server behind, so the group is killed after a clean exit too.
    """
    proc = subprocess.Popen(
        argv,
        env=env,
        cwd=cwd,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        start_new_session=True,
    )
    exited = True
    try:
        output, _ = proc.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        exited = False
        os.killpg(proc.pid, signal.SIGKILL)
        output, _ = proc.communicate()
    finally:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    return exited and proc.returncode is not None and proc.returncode >= 0, output


def _poll(marker: Path) -> None:
    deadline = time.monotonic() + MARKER_POLL_SECONDS
    while not marker.exists() and time.monotonic() < deadline:
        time.sleep(0.5)


def _session(
    argv: list[str],
    env: dict[str, str],
    cwd: Path,
    marker: Path,
    timeout: float,
    debug: Path | None,
) -> Session:
    """One bounded session. A hang skips with its reason; otherwise the marker is polled."""
    exited, output = _run_bounded(argv, env, cwd, timeout)
    if not exited and not marker.exists():
        pytest.skip(
            f"{argv[0]} did not exit within {timeout:.0f} s and was killed: {output[-300:]!r}"
        )
    _poll(marker)
    return Session(marker=marker, debug=debug, output=output)


def _claude_session(
    binary: Path, env: dict[str, str], cwd: Path, marker: Path
) -> Session:
    debug = cwd.parent / f"debug-{cwd.name}-{marker.stem}.log"
    argv = [str(binary), "-p", "hi", "--debug-file", str(debug)]
    return _session(argv, env, cwd, marker, CLAUDE_TIMEOUT, debug)


def _opencode_session(
    binary: Path, env: dict[str, str], cwd: Path, marker: Path
) -> Session:
    argv = [
        str(binary),
        "run",
        "--standalone",
        "--print-logs",
        "--log-level",
        "debug",
        "-m",
        "none/none",
        "x",
    ]
    return _session(argv, env, cwd, marker, OPENCODE_TIMEOUT, None)


def _version(binary: Path, env: dict[str, str]) -> str:
    """The resolved client version, never a reason to fail."""
    try:
        done = subprocess.run(
            [str(binary), "--version"],
            env=env,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        return f"unresolved ({error.__class__.__name__})"
    return (done.stdout.strip() or done.stderr.strip() or "unresolved").splitlines()[0]


def _no_trust_prompt(session: Session) -> None:
    text = (
        session.debug.read_text(encoding="utf-8", errors="replace")
        if session.debug and session.debug.exists()
        else ""
    )
    hits = [line for line in text.splitlines() if _TRUST_PROMPT.search(line)]
    assert hits == [], f"a trusted scratch project showed a trust prompt: {hits[:3]}"


# ---------------------------------------------------------------------------
# The probe mod and where grim installs it
# ---------------------------------------------------------------------------


def _claude_entry(marker: Path) -> str:
    return (
        "export function register(on) {\n"
        '  on("session.start", async ($, e, next) => {\n'
        f'    await $.fs.write({json.dumps(str(marker))}, "loaded");\n'
        "    return next(e);\n"
        "  });\n"
        "}\n"
    )


def _opencode_entry(marker: Path) -> str:
    return (
        'import { Plugin } from "@opencode/plugin";\n'
        'import { writeFileSync } from "node:fs";\n'
        "export default Plugin.define({\n"
        '  id: "probe-id",\n'
        "  async setup(ctx) {\n"
        f'    writeFileSync({json.dumps(str(marker))}, "loaded");\n'
        "  },\n"
        "});\n"
    )


def _probe_mod(repo: str, marker: Path, *, opencode_ext: str = "ts"):
    """A published mod whose hook writes `marker`; its OpenCode entry takes `opencode_ext`."""
    name = repo.rsplit("/", 1)[-1]
    manifest = (
        "schema      = 1\n"
        f'name        = "{name}"\n'
        'description = "load probe"\n'
        "\n"
        "[claude]\n"
        'entry = "index.js"\n'
        "\n"
        "[opencode]\n"
        f'entry = "index.{opencode_ext}"\n'
    )
    files = {
        "opencode/index.ts": None,
        "claude/index.js": _claude_entry(marker),
        f"opencode/index.{opencode_ext}": _opencode_entry(marker),
    }
    return make_mod(repo, "1", files, mod_toml=manifest)


@dataclass
class Rig:
    """Scratch world shared by grim and the client: one tmp dir, one home."""

    base: Path
    home: Path
    roots: dict[str, Path]
    grim_env: dict[str, str]

    def project(self, name: str) -> Path:
        path = self.base / name
        path.mkdir(parents=True, exist_ok=True)
        (path / ".claude").mkdir(exist_ok=True)
        (path / ".opencode").mkdir(exist_ok=True)
        return path

    def trust(self, *projects: Path) -> None:
        """Seed folder trust in the scratch Claude config only."""
        record = {
            "projects": {
                str(p.resolve()): {"hasTrustDialogAccepted": True} for p in projects
            }
        }
        (self.roots["claude"] / ".claude.json").write_text(
            json.dumps(record), encoding="utf-8", newline="\n"
        )

    def marker(self, label: str) -> Path:
        return self.base / f"marker-{label}.txt"

    def opencode_config(self) -> None:
        """OpenCode's scratch config: updates and sharing off."""
        (self.roots["opencode"] / "opencode.json").write_text(
            json.dumps({"autoupdate": False, "share": "disabled"}),
            encoding="utf-8",
            newline="\n",
        )


@pytest.fixture
def rig(grim_at: Callable[[Path], object], tmp_path: Path) -> Rig:
    runner = grim_at(tmp_path)
    home = Path(runner.home)  # type: ignore[attr-defined]
    env, roots = mod_sandbox(tmp_path / "sandbox", home=home)
    # The client shares the runner's XDG root (under its home), so grim and the client agree.
    env.pop("XDG_CONFIG_HOME")
    base = tmp_path / "world"
    base.mkdir()
    built = Rig(base=base, home=home, roots=roots, grim_env=env)
    built.opencode_config()
    return built


def _install_project(grim_at, rig: Rig, project: Path, mod, *, clients: str) -> None:
    """Declare the mod in `project` with the flag on, then install it with consent given."""
    # OpenCode hosts a mod only once the scope declares the V2 plugin API.
    write_config(
        project,
        mods={"probe": mod.fq},
        experimental_mods=True,
        vendors={"opencode": ['plugin_api = "v2"']},
    )
    runner = grim_at(project)
    runner.env.update(rig.grim_env)
    runner.run("lock")
    runner.run("install", "--client", clients, "--trust-workspace")


def _install_global(grim_at, rig: Rig, mod, *, clients: str) -> None:
    """Add the mod at global scope; `add` takes no `--client`, so the global list names it."""
    runner = grim_at(rig.base)
    runner.env.update(rig.grim_env)
    runner.run("config", "set", "--global", "options.experimental.mods", "true")
    runner.run("config", "set", "--global", "options.vendors.opencode.plugin_api", "v2")
    runner.run("config", "set", "--global", "options.clients", clients)
    runner.run(
        "add",
        "--global",
        "--kind",
        "mod",
        "--trust-workspace",
        mod.fq,
    )


def _record(record_property, binary: Path, env: dict[str, str], client: str) -> None:
    record_property(f"{client}_version", _version(binary, env))


# ---------------------------------------------------------------------------
# Items 1 to 4: Claude
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("scope", ["project", "global"])
def test_item1_name_only_plugin_json_loads(
    scope: str, claude_bin: Path, grim_at, rig: Rig, unique_repo: str, record_property
) -> None:
    marker = rig.marker(f"item1-{scope}")
    mod = _probe_mod(f"{unique_repo}/probe", marker)
    project = rig.project("proj")
    rig.trust(project)
    if scope == "project":
        _install_project(grim_at, rig, project, mod, clients="claude")
        manifest = project / ".claude/mods/probe/.claude-plugin/plugin.json"
    else:
        _install_global(grim_at, rig, mod, clients="claude")
        manifest = rig.roots["claude"] / "mods/probe/.claude-plugin/plugin.json"
    assert set(json.loads(manifest.read_text(encoding="utf-8"))) == {"name"}, (
        "the manifest carries a name and nothing else"
    )
    env = _child_env(claude_bin, rig.home, rig.roots)
    _record(record_property, claude_bin, env, "claude")
    session = _claude_session(claude_bin, env, project, marker)
    _no_trust_prompt(session)
    assert session.loaded, (
        f"claude did not load a name-only plugin ({scope} scope): {session.output[-300:]!r}"
    )


def test_item2_absolute_path_in_settings_local_loads_in_the_first_session(
    claude_bin: Path, grim_at, rig: Rig, unique_repo: str, record_property
) -> None:
    marker = rig.marker("item2")
    mod = _probe_mod(f"{unique_repo}/probe", marker)
    project = rig.project("proj")
    rig.trust(project)
    _install_project(grim_at, rig, project, mod, clients="claude")
    settings = json.loads(
        (project / ".claude/settings.local.json").read_text(encoding="utf-8")
    )
    [entry] = settings["extraKnownMarketplaces"].values()
    registered = Path(entry["source"]["path"])
    assert registered.is_absolute() and registered == (project / ".claude/mods"), (
        registered
    )
    env = _child_env(claude_bin, rig.home, rig.roots)
    _record(record_property, claude_bin, env, "claude")
    assert not (rig.roots["claude"] / "plugins").exists(), (
        "this must be the first session of the scratch config"
    )
    session = _claude_session(claude_bin, env, project, marker)
    assert session.loaded, (
        f"claude did not load the absolute path in its first session: {session.output[-300:]!r}"
    )


def test_item3_an_untrusted_project_does_not_load_the_registration(
    claude_bin: Path, grim_at, rig: Rig, unique_repo: str, record_property
) -> None:
    trusted_marker, untrusted_marker = (
        rig.marker("item3-trusted"),
        rig.marker("item3-untrusted"),
    )
    trusted, untrusted = rig.project("trusted"), rig.project("untrusted")
    rig.trust(trusted)
    _install_project(
        grim_at,
        rig,
        trusted,
        _probe_mod(f"{unique_repo}/twin-a", trusted_marker),
        clients="claude",
    )
    _install_project(
        grim_at,
        rig,
        untrusted,
        _probe_mod(f"{unique_repo}/twin-b", untrusted_marker),
        clients="claude",
    )
    env = _child_env(claude_bin, rig.home, rig.roots)
    _record(record_property, claude_bin, env, "claude")
    # Positive control: the trusted twin writes its marker, so the silence below is trust's doing.
    twin = _claude_session(claude_bin, env, trusted, trusted_marker)
    assert twin.loaded, (
        f"the trusted twin did not load, so the control is void: {twin.output[-300:]!r}"
    )
    other = _claude_session(claude_bin, env, untrusted, untrusted_marker)
    assert not other.loaded, (
        "an untrusted project loaded a settings.local.json registration"
    )


def test_item4_a_user_scope_registration_loads_in_a_global_session(
    claude_bin: Path, grim_at, rig: Rig, unique_repo: str, record_property
) -> None:
    marker = rig.marker("item4")
    _install_global(
        grim_at, rig, _probe_mod(f"{unique_repo}/probe", marker), clients="claude"
    )
    session_dir = rig.project("elsewhere")
    rig.trust(session_dir)
    assert not (session_dir / ".claude/settings.local.json").exists(), (
        "nothing project-scoped may carry the load"
    )
    env = _child_env(claude_bin, rig.home, rig.roots)
    _record(record_property, claude_bin, env, "claude")
    session = _claude_session(claude_bin, env, session_dir, marker)
    assert session.loaded, (
        f"claude did not load a user-scope registration: {session.output[-300:]!r}"
    )


# ---------------------------------------------------------------------------
# Items 5 and 6: observations, failing only when the mod does not load at all
# ---------------------------------------------------------------------------


def _marketplace_name(project: Path) -> str:
    manifest = project / ".claude/mods/.claude-plugin/marketplace.json"
    return json.loads(manifest.read_text(encoding="utf-8"))["name"]


def _rename_in(path: Path, old: str, new: str) -> None:
    path.write_text(
        path.read_text(encoding="utf-8").replace(old, new),
        encoding="utf-8",
        newline="\n",
    )


def _debug_lines(session: Session, word: str) -> str:
    if session.debug is None or not session.debug.exists():
        return ""
    lines = [
        ln
        for ln in session.debug.read_text(
            encoding="utf-8", errors="replace"
        ).splitlines()
        if word in ln
    ]
    return " | ".join(lines[:3])[:400]


def test_item5_marketplace_keyed_by_settings_key_or_manifest_name(
    claude_bin: Path, grim_at, rig: Rig, unique_repo: str, record_property
) -> None:
    marker = rig.marker("item5")
    project = rig.project("proj")
    rig.trust(project)
    _install_project(
        grim_at,
        rig,
        project,
        _probe_mod(f"{unique_repo}/probe", marker),
        clients="claude",
    )
    env = _child_env(claude_bin, rig.home, rig.roots)
    _record(record_property, claude_bin, env, "claude")
    baseline = _claude_session(claude_bin, env, project, marker)
    assert baseline.loaded, f"the mod does not load at all: {baseline.output[-300:]!r}"

    marker.unlink()
    key = _marketplace_name(project)
    _rename_in(
        project / ".claude/mods/.claude-plugin/marketplace.json",
        key,
        "manifest-name-differs",
    )
    mismatched = _claude_session(claude_bin, env, project, marker)
    record_property(
        "claude_marketplace_key_matches_settings_key", str(mismatched.loaded)
    )
    record_property(
        "claude_marketplace_mismatch_debug", _debug_lines(mismatched, "marketplace")
    )


def test_item6_one_marketplace_name_declared_by_two_projects(
    claude_bin: Path, grim_at, rig: Rig, unique_repo: str, record_property
) -> None:
    marker_a, marker_b = rig.marker("item6-a"), rig.marker("item6-b")
    first, second = rig.project("first"), rig.project("second")
    rig.trust(first, second)
    _install_project(
        grim_at,
        rig,
        first,
        _probe_mod(f"{unique_repo}/probe-a", marker_a),
        clients="claude",
    )
    _install_project(
        grim_at,
        rig,
        second,
        _probe_mod(f"{unique_repo}/probe-b", marker_b),
        clients="claude",
    )
    env = _child_env(claude_bin, rig.home, rig.roots)
    _record(record_property, claude_bin, env, "claude")
    for project, marker in ((first, marker_a), (second, marker_b)):
        control = _claude_session(claude_bin, env, project, marker)
        assert control.loaded, (
            f"the mod does not load at all in {project.name}: {control.output[-300:]!r}"
        )
        marker.unlink()

    # Both projects now declare the first project's marketplace name.
    shared, own = _marketplace_name(first), _marketplace_name(second)
    _rename_in(second / ".claude/mods/.claude-plugin/marketplace.json", own, shared)
    _rename_in(second / ".claude/settings.local.json", own, shared)
    seen = {}
    for project, marker in ((first, marker_a), (second, marker_b)):
        seen[project.name] = _claude_session(claude_bin, env, project, marker).loaded
    record_property(
        "claude_shared_marketplace_name_markers", json.dumps(seen, sort_keys=True)
    )
    record_property(
        "claude_shared_marketplace_name_crosses_over", str(not all(seen.values()))
    )


# ---------------------------------------------------------------------------
# Item 7: OpenCode
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("scope", ["project", "global"])
def test_item7_extensionless_re_export_of_a_js_entry(
    scope: str, opencode_bin: Path, grim_at, rig: Rig, unique_repo: str, record_property
) -> None:
    marker = rig.marker(f"item7-{scope}")
    mod = _probe_mod(f"{unique_repo}/probe", marker, opencode_ext="js")
    project = rig.project("proj")
    if scope == "project":
        _install_project(grim_at, rig, project, mod, clients="opencode")
        entry = project / ".opencode/plugins/probe/index.ts"
    else:
        _install_global(grim_at, rig, mod, clients="opencode")
        entry = rig.roots["opencode"] / "plugins/probe/index.ts"
    assert 'from "./src/' in entry.read_text(encoding="utf-8")
    assert ".js" not in entry.read_text(encoding="utf-8"), (
        "the re-export names no extension"
    )
    env = _child_env(opencode_bin, rig.home, rig.roots)
    _record(record_property, opencode_bin, env, "opencode")
    session = _opencode_session(opencode_bin, env, project, marker)
    assert session.loaded, (
        f"opencode did not load a .js entry through its re-export ({scope}): {session.output[-300:]!r}"
    )


# ---------------------------------------------------------------------------
# Items 8 to 10: not answerable by a headless session
# ---------------------------------------------------------------------------

_NOT_HEADLESS = {
    8: "needs a mod that uses `$.state` and a session that runs it; no headless probe shows whether `types` is read at runtime",
    9: "needs an interactive `/plugin` disable; a headless run cannot drive it",
    10: "needs a Copilot client, which this probe does not run",
}


@pytest.mark.parametrize("item", sorted(_NOT_HEADLESS))
def test_item_needs_more_than_a_headless_session(item: int) -> None:
    pytest.skip(f"probe item {item}: {_NOT_HEADLESS[item]}")


# ---------------------------------------------------------------------------
# The CI job: dispatch only, never a gate (runs without any client)
# ---------------------------------------------------------------------------


def _job_block(text: str, job: str) -> str:
    match = re.search(
        rf"^  {re.escape(job)}:\n(?P<body>(?:(?:    .*|\s*)\n)+)", text, re.MULTILINE
    )
    assert match, f"job {job!r} not found in {WORKFLOW}"
    return match.group("body")


def test_deep_workflow_probe_job_is_dispatch_only_and_non_gating() -> None:
    text = WORKFLOW.read_text(encoding="utf-8")
    triggers = re.search(r"^on:\n(?P<body>(?:  .*\n)+)", text, re.MULTILINE)
    assert triggers, "the workflow has no `on:` block"
    names = re.findall(r"^  ([a-z_]+):", triggers.group("body"), re.MULTILINE)
    assert names == ["workflow_dispatch"], (
        f"the deep workflow must be dispatch only: {names}"
    )
    assert not re.search(r"^\s*schedule:", text, re.MULTILINE), "no schedule trigger"

    job = _job_block(text, "mod-load-probe")
    assert re.search(r"^    continue-on-error: true$", job, re.MULTILINE)
    assert re.search(r"^    needs: \[build\]$", job, re.MULTILINE)
    assert re.search(r"^    permissions:\n      contents: read\n", job, re.MULTILINE)
    assert "secrets." not in job, "the probe job must hold no secret"
    assert "name: deep-mod-probe" in job

    aggregate = _job_block(text, "test-results")
    needs = re.search(r"^    needs: \[(?P<jobs>[^\]]*)\]$", aggregate, re.MULTILINE)
    assert needs, "test-results has no `needs:` list"
    gated = [name.strip() for name in needs.group("jobs").split(",")]
    assert "mod-load-probe" not in gated, "the probe must gate nothing"
    assert "build" in gated, "positive control: the parser read the real needs list"
