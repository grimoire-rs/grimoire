# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Real Claude Code and Codex binaries read the settings files grim renders.

Opt-in: ``GRIM_REAL_CLAUDE_BIN`` and ``GRIM_REAL_CODEX_BIN`` name the binaries,
and each test skips with a reason while its variable is unset, so the ordinary
run skips them all. ``adr_settings_artifact_kind.md``, Decisions 3 and 10.

Credentials never reach a harness. Every child process gets an environment
built from scratch (never copied from ``os.environ``) with a scratch ``HOME``
and the vendor root redirected into the test's tmp dir. That holds on Linux
only: elsewhere a harness reaches the login through the OS credential store or
a home it resolves without ``HOME``, so off Linux every test skips and no
binary is spawned. Only offline read verbs
run: ``claude doctor``, ``codex mcp list``, ``codex debug prompt-input`` and
``codex execpolicy check``. Nothing calls a model, starts a session or logs in,
and what only a session can show is a skip with that reason, never a pass.

Each reading sits beside a broken copy of the same file that the same binary
must report, so a binary that says nothing never reads as a pass. Fixtures come
from ``make_settings``.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from collections.abc import Callable
from pathlib import Path

import pytest

from src.helpers import make_settings, settings_sandbox, write_config
from src.runner import GrimRunner

CLAUDE_VAR = "GRIM_REAL_CLAUDE_BIN"
CODEX_VAR = "GRIM_REAL_CODEX_BIN"

# Telemetry and update opt-outs, each a fixed literal. They are the only
# CLAUDE_* names the child environment may hold besides the config root.
_OPT_OUTS = {
    "DO_NOT_TRACK": "1",
    "DISABLE_TELEMETRY": "1",
    "DISABLE_ERROR_REPORTING": "1",
    "DISABLE_AUTOUPDATER": "1",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
}
_ROOT_KEYS = {"HOME", "TMPDIR", "CLAUDE_CONFIG_DIR", "CODEX_HOME"}
_ALLOWED_KEYS = {"PATH", *_ROOT_KEYS, *_OPT_OUTS}

# The only subcommands a test may hand a harness: none dials a model or logs in.
_OFFLINE_VERBS = {
    ("doctor",),
    ("mcp", "list"),
    ("debug", "prompt-input"),
    ("execpolicy", "check"),
}
_CODEX_FLAGS = [
    "-c",
    "analytics.enabled=false",
    "-c",
    "feedback.enabled=false",
    "-c",
    "check_for_update_on_startup=false",
]

CLAUDE_ENTRY = {"source": {"source": "github", "repo": "acme/tools"}}
CODEX_ENTRY = {"source": "https://example.com/acme", "source_type": "git"}
CODEX_RULES = [
    {"pattern": ["npm", "test"], "decision": "allow", "justification": "the suite"},
    {"pattern": ["rm"], "decision": "forbidden"},
]
BAD_RULES = 'prefix_rule(pattern = "ls", decision = "allow"\n'


def _isolation_gap(platform: str) -> str | None:
    """Why a scratch environment does not isolate a harness on `platform`."""
    if platform.startswith("linux"):
        return None
    return (
        f"{platform}: a harness can reach the real login there (the OS credential store, or a "
        "home resolved without HOME), so the real-binary checks run on Linux only"
    )


def _binary(variable: str) -> Path:
    value = os.environ.get(variable)
    if not value:
        pytest.skip(f"{variable} is unset: the real-binary checks are opt-in")
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
def codex_bin() -> Path:
    return _binary(CODEX_VAR)


def _harness_env(binary: Path, anchor: Path, vendor: dict[str, str]) -> dict[str, str]:
    """A child environment assembled from nothing, its home under `anchor`."""
    home = anchor / "harness-home"
    env = {
        "PATH": f"{binary.parent}:/usr/bin:/bin",
        "HOME": str(home),
        "TMPDIR": str(home / "tmp"),
        **_OPT_OUTS,
        **vendor,
    }
    (home / "tmp").mkdir(parents=True, exist_ok=True)
    _assert_no_credentials(env, anchor)
    return env


def _assert_no_credentials(env: dict[str, str], anchor: Path) -> None:
    """Every key allowlisted, every opt-out its literal, every root under `anchor`."""
    assert set(env) <= _ALLOWED_KEYS, sorted(set(env) - _ALLOWED_KEYS)
    altered = sorted(key for key, value in _OPT_OUTS.items() if env.get(key) != value)
    assert altered == [], f"opt-outs must hold their literals: {altered}"
    # A child without HOME resolves the real one from the password database.
    assert {"HOME", "TMPDIR"} <= set(env), "HOME and TMPDIR must be set"
    for key in sorted(_ROOT_KEYS & set(env)):
        root = Path(env[key])
        assert root.is_absolute(), f"{key} must be absolute"
        assert root.resolve().is_relative_to(anchor.resolve()), f"{key} must sit under the test's tmp dir"


def _run(binary: Path, args: list[str], env: dict[str, str], cwd: Path) -> subprocess.CompletedProcess[str]:
    gap = _isolation_gap(sys.platform)
    assert gap is None, gap
    verb = tuple(arg for arg in args if not arg.startswith("-") and "=" not in arg)[:2]
    assert any(verb[: len(allowed)] == allowed for allowed in _OFFLINE_VERBS), f"not an offline verb: {args}"
    assert "exec" not in args and "-p" not in args and "--print" not in args
    return subprocess.run(  # noqa: S603
        [str(binary), *args],
        env=env,
        cwd=cwd,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=120,
        check=False,
    )


def _output(result: subprocess.CompletedProcess[str]) -> str:
    return result.stdout + result.stderr


def _render(
    grim: GrimRunner, home: Path, sandbox: Path, repo: str, client: str, document: dict
) -> dict[str, Path]:
    """Install `document` for one client through grim and return the client roots."""
    team = make_settings(f"{repo}/team", "1", {"description": "team defaults", **document})
    write_config(
        home, settings={"team": team.fq}, options={"clients": json.dumps([client])}, experimental_settings=True
    )
    env, roots = settings_sandbox(sandbox, grim.home)
    grim.env.update(env)
    grim.run("--global", "lock")
    grim.run("--global", "install")
    return roots


def _copy_root(root: Path, name: str) -> Path:
    copy = root.parent / name
    shutil.copytree(root, copy)
    return copy


def test_harness_env_is_built_from_scratch(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    """Ambient credentials and unrelated variables never reach a harness child."""
    secrets = {
        "ANTHROPIC_API_KEY": "canary-a",
        "CLAUDE_CODE_OAUTH_TOKEN": "canary-b",
        "OPENAI_API_KEY": "canary-c",
        "CODEX_API_KEY": "canary-d",
        "GH_TOKEN": "canary-e",
        "GITHUB_TOKEN": "canary-f",
        "UNRELATED_CANARY": "canary-g",
    }
    for key, value in secrets.items():
        monkeypatch.setenv(key, value)
    home = tmp_path / "harness-home"
    env = _harness_env(
        tmp_path / "bin" / "claude", tmp_path, {"CLAUDE_CONFIG_DIR": str(home / ".claude")}
    )

    assert not set(secrets) & set(env)
    assert not {value for value in env.values() if value.startswith("canary-")}
    assert env["HOME"] == str(home)


_PLANTED: dict[str, Callable[[dict[str, str], Path], dict[str, str]]] = {
    "leaked-key": lambda env, _: {**env, "ANTHROPIC_API_KEY": "x"},
    "altered-opt-out": lambda env, _: {**env, "DO_NOT_TRACK": "0"},
    "no-home": lambda env, _: {key: value for key, value in env.items() if key != "HOME"},
    "home-outside": lambda env, tmp: {**env, "HOME": str(tmp.parent / "elsewhere")},
    "tmpdir-escapes": lambda env, tmp: {**env, "TMPDIR": str(tmp / ".." / "elsewhere")},
    "vendor-root-outside": lambda env, tmp: {**env, "CLAUDE_CONFIG_DIR": str(tmp.parent / ".claude")},
    "relative-root": lambda env, _: {**env, "CODEX_HOME": ".codex"},
}


@pytest.mark.parametrize("plant", list(_PLANTED), ids=list(_PLANTED))
def test_a_planted_leak_fails_the_check(plant: str, tmp_path: Path) -> None:
    """Each way a child could reach a credential or the real home fails the check."""
    env = _harness_env(
        tmp_path / "bin" / "claude", tmp_path, {"CLAUDE_CONFIG_DIR": str(tmp_path / "claude-root")}
    )
    _assert_no_credentials(env, tmp_path)

    with pytest.raises(AssertionError):
        _assert_no_credentials(_PLANTED[plant](env, tmp_path), tmp_path)


@pytest.mark.parametrize("platform", ["darwin", "win32", "freebsd14"])
def test_harnesses_run_on_linux_only(
    platform: str, monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    """Off Linux a scratch HOME does not keep a harness from the real login, so
    the fixture skips and the runner refuses before anything is spawned."""
    marker = tmp_path / "ran"
    stand_in = tmp_path / "claude"
    stand_in.write_text(f"#!/bin/sh\ntouch '{marker}'\n", encoding="utf-8", newline="\n")
    stand_in.chmod(0o755)
    monkeypatch.setenv(CLAUDE_VAR, str(stand_in))
    env = _harness_env(stand_in, tmp_path, {"CLAUDE_CONFIG_DIR": str(tmp_path / "claude-root")})

    monkeypatch.setattr(sys, "platform", "linux")
    assert _binary(CLAUDE_VAR) == stand_in, "positive control: Linux hands the binary out"
    monkeypatch.setattr(sys, "platform", platform)
    with pytest.raises(pytest.skip.Exception, match="Linux only"):
        _binary(CLAUDE_VAR)
    with pytest.raises(AssertionError, match="Linux only"):
        _run(stand_in, ["doctor"], env, tmp_path)
    assert not marker.exists(), "a harness ran off Linux"


# ── Claude Code ──────────────────────────────────────────────────────────

CLAUDE_DOCUMENT = {
    "claude": {
        "model": "sonnet",
        "effortLevel": "high",
        "alwaysThinkingEnabled": False,
        "permissions": {"allow": ["Bash(npm test:*)"], "deny": ["Bash(rm:*)"], "defaultMode": "plan"},
        "sandbox": {"enabled": True},
        "extraKnownMarketplaces": {"acme": CLAUDE_ENTRY},
    }
}


def _claude_doctor(binary: Path, tmp_path: Path, root: Path) -> str:
    cwd = tmp_path / "harness-cwd"
    cwd.mkdir(exist_ok=True)
    env = _harness_env(binary, tmp_path, {"CLAUDE_CONFIG_DIR": str(root)})
    result = _run(binary, ["doctor"], env, cwd)
    assert result.returncode == 0, _output(result)
    assert "Claude Code doctor" in result.stdout, "the binary reported nothing at all"
    return result.stdout


def test_claude_doctor_accepts_the_rendered_settings(
    claude_bin: Path, grim, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    roots = _render(grim, grim_home, tmp_path / "s", unique_repo, "claude", CLAUDE_DOCUMENT)
    root = roots["claude"]
    rendered = json.loads((root / "settings.json").read_text(encoding="utf-8"))
    assert rendered["permissions"] == CLAUDE_DOCUMENT["claude"]["permissions"]
    assert rendered["extraKnownMarketplaces"] == {"acme": CLAUDE_ENTRY}

    # Controls first: the binary names an invalid value and a malformed file.
    bad_value = _copy_root(root, "bad-value")
    broken = {**rendered, "permissions": {**rendered["permissions"], "defaultMode": "bogus"}}
    (bad_value / "settings.json").write_text(json.dumps(broken), encoding="utf-8", newline="\n")
    report = _claude_doctor(claude_bin, tmp_path, bad_value)
    assert "Invalid settings" in report
    assert "permissions.defaultMode" in report

    malformed = _copy_root(root, "malformed")
    text = (malformed / "settings.json").read_text(encoding="utf-8")
    (malformed / "settings.json").write_text(text[: len(text) // 2], encoding="utf-8", newline="\n")
    assert "Invalid or malformed JSON" in _claude_doctor(claude_bin, tmp_path, malformed)

    report = _claude_doctor(claude_bin, tmp_path, root)
    assert "Invalid settings" not in report


def test_claude_effective_values_need_a_session(claude_bin: Path) -> None:
    """`claude doctor` validates files; which value wins is shown only in a session."""
    pytest.skip(
        "no credential-free Claude verb prints an effective setting: reading one needs a session, "
        "which needs a login"
    )


# ── Codex ────────────────────────────────────────────────────────────────

CODEX_DOCUMENT = {
    "codex": {
        "model": "gpt-5",
        "approval_policy": "never",
        "sandbox_mode": "workspace-write",
        "sandbox_workspace_write": {"network_access": True, "writable_roots": ["/tmp/grim-real-harness"]},
        "tui": {"animations": False},
        "history": {"persistence": "none"},
        "marketplaces": {"acme": CODEX_ENTRY},
        "rules": CODEX_RULES,
    }
}


def _codex(binary: Path, tmp_path: Path, root: Path, args: list[str], *, flags: bool = True) -> subprocess.CompletedProcess[str]:
    cwd = tmp_path / "harness-cwd"
    cwd.mkdir(exist_ok=True)
    env = _harness_env(binary, tmp_path, {"CODEX_HOME": str(root)})
    return _run(binary, [*(_CODEX_FLAGS if flags else []), *args], env, cwd)


def test_codex_loads_the_rendered_config(
    codex_bin: Path, grim, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    roots = _render(grim, grim_home, tmp_path / "s", unique_repo, "codex", CODEX_DOCUMENT)
    root = roots["codex"]
    config = (root / "config.toml").read_text(encoding="utf-8")
    assert 'approval_policy = "never"' in config

    # Controls first: a bad value and a malformed file both stop the same verbs.
    bad_value = _copy_root(root, "bad-value")
    (bad_value / "config.toml").write_text(
        config.replace('approval_policy = "never"', 'approval_policy = "bogus"'), encoding="utf-8", newline="\n"
    )
    refused = _codex(codex_bin, tmp_path, bad_value, ["mcp", "list"])
    assert refused.returncode != 0
    assert "approval_policy" in _output(refused)

    malformed = _copy_root(root, "malformed")
    (malformed / "config.toml").write_text(config + "[tui\n", encoding="utf-8", newline="\n")
    refused = _codex(codex_bin, tmp_path, malformed, ["mcp", "list"])
    assert refused.returncode != 0
    assert "failed to load bootstrap configuration" in _output(refused)

    listed = _codex(codex_bin, tmp_path, root, ["mcp", "list"])
    assert listed.returncode == 0, _output(listed)
    assert "failed to load" not in _output(listed)

    # The one read-back verb: it prints the sandbox and approval settings it loaded.
    shown = _codex(codex_bin, tmp_path, root, ["debug", "prompt-input"])
    assert shown.returncode == 0, _output(shown)
    assert "`sandbox_mode` is `workspace-write`" in shown.stdout
    assert "Approval policy is currently never" in shown.stdout

    other = _copy_root(root, "other-mode")
    (other / "config.toml").write_text(config.replace('"workspace-write"', '"read-only"'), encoding="utf-8", newline="\n")
    flipped = _codex(codex_bin, tmp_path, other, ["debug", "prompt-input"])
    assert "`sandbox_mode` is `read-only`" in flipped.stdout, "the read-back follows the file"


def test_codex_execpolicy_reads_the_rendered_rules(
    codex_bin: Path, grim, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    roots = _render(grim, grim_home, tmp_path / "s", unique_repo, "codex", CODEX_DOCUMENT)
    rules = roots["codex"] / "rules" / "grim-team.rules"
    assert rules.is_file()

    def check(*files: Path, argv: list[str]) -> subprocess.CompletedProcess[str]:
        flags = [part for file in files for part in ("--rules", str(file))]
        return _codex(codex_bin, tmp_path, roots["codex"], ["execpolicy", "check", *flags, *argv], flags=False)

    bad = tmp_path / "bad.rules"
    bad.write_text(BAD_RULES, encoding="utf-8", newline="\n")
    refused = check(bad, argv=["ls"])
    assert refused.returncode != 0
    assert "failed to parse policy" in _output(refused), "the binary reports a bad rules file"

    forbidden = check(rules, argv=["rm", "-rf", "x"])
    assert forbidden.returncode == 0, _output(forbidden)
    assert json.loads(forbidden.stdout)["decision"] == "forbidden"
    allowed = check(rules, argv=["npm", "test"])
    assert json.loads(allowed.stdout)["decision"] == "allow"
    unmatched = check(rules, argv=["ls"])
    assert "matchedRules" not in json.loads(unmatched.stdout) or json.loads(unmatched.stdout)["matchedRules"] == []


def test_codex_execpolicy_with_a_second_bad_rules_file(
    codex_bin: Path, grim, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    """Records how one malformed file beside grim's good one changes a check.

    The verb takes both files in one call. The assertions pin what the pinned
    Codex does, so a change upstream turns this red and the reference page is
    re-read.
    """
    roots = _render(grim, grim_home, tmp_path / "s", unique_repo, "codex", CODEX_DOCUMENT)
    rules = roots["codex"] / "rules" / "grim-team.rules"
    bad = tmp_path / "bad.rules"
    bad.write_text(BAD_RULES, encoding="utf-8", newline="\n")

    def check(*files: Path) -> subprocess.CompletedProcess[str]:
        flags = [part for file in files for part in ("--rules", str(file))]
        return _codex(
            codex_bin, tmp_path, roots["codex"], ["execpolicy", "check", *flags, "rm", "x"], flags=False
        )

    assert json.loads(check(rules).stdout)["decision"] == "forbidden", "the good file alone decides"
    both = check(rules, bad)
    assert both.returncode != 0
    assert "failed to parse policy" in _output(both)
    assert "decision" not in both.stdout, "the good file's rule is not applied when its neighbour is bad"


def test_codex_session_loading_of_rules_needs_a_session(codex_bin: Path) -> None:
    """How a session treats a bad file in `rules/` is not observable offline."""
    pytest.skip(
        "`codex debug prompt-input` and `codex mcp list` never read rules/, so only a session loads "
        "them, and a session needs a login"
    )
