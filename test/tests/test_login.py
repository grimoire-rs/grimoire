# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Acceptance tests for ``grim login`` / ``grim logout``.

Every test isolates the docker config into a per-test ``DOCKER_CONFIG``
tempdir — the user's real ``~/.docker`` is never touched. Helper-backed
tests drop a ``docker-credential-test`` Python script onto a tempdir,
prepend it to ``PATH``, and point ``credsStore`` at ``test``.

``grim login`` verifies the credential against the registry by default;
tests exercising only the store mechanics pass ``--no-verify`` so they
stay network-free. Verification tests use the anonymous session registry
and a module-local htpasswd-gated ``registry:2`` container.

Exit codes follow ``quality-rust-exit_codes.md`` (sysexits-aligned):
usage 64, unavailable 69, config 78, auth 80, offline-blocked 81,
success 0.
"""
from __future__ import annotations

import base64
import json
import os
import socket
import stat
import subprocess
import sys
import time
import urllib.error
import urllib.request
from collections.abc import Iterator
from pathlib import Path

import pytest

from src.runner import GrimRunner

# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture()
def docker_config(tmp_path: Path) -> Path:
    """An isolated ``$DOCKER_CONFIG`` directory."""
    d = tmp_path / "docker"
    d.mkdir()
    return d


_MOCK_HELPER = """\
#!/usr/bin/env python3
import json, os, sys

store = os.environ["DOCKER_CREDENTIAL_TEST_STORE"]


def load():
    try:
        with open(store) as fh:
            return json.load(fh)
    except FileNotFoundError:
        return {}


def save(data):
    with open(store, "w") as fh:
        json.dump(data, fh)


action = sys.argv[1] if len(sys.argv) > 1 else ""
data = load()

if action == "store":
    req = json.load(sys.stdin)
    data[req["ServerURL"]] = {"Username": req["Username"], "Secret": req["Secret"]}
    save(data)
elif action == "get":
    server = sys.stdin.read().strip()
    entry = data.get(server)
    if entry is None:
        print("credentials not found in native keychain")
        sys.exit(1)
    print(json.dumps({"ServerURL": server, "Username": entry["Username"], "Secret": entry["Secret"]}))
elif action == "erase":
    server = sys.stdin.read().strip()
    data.pop(server, None)
    save(data)
elif action == "list":
    print(json.dumps({k: v["Username"] for k, v in data.items()}))
else:
    sys.exit(2)
"""


@pytest.fixture()
def credential_helper(tmp_path: Path, docker_config: Path) -> dict[str, str]:
    """A ``docker-credential-test`` helper on PATH + a ``credsStore`` config.

    Returns the extra environment the runner needs and writes the seed
    config.json. The helper persists credentials to a JSON file named by
    ``DOCKER_CREDENTIAL_TEST_STORE``.
    """
    bin_dir = tmp_path / "helper-bin"
    bin_dir.mkdir()
    helper = bin_dir / "docker-credential-test"
    helper.write_text(_MOCK_HELPER)
    helper.chmod(helper.stat().st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)

    (docker_config / "config.json").write_text(json.dumps({"credsStore": "test"}))

    store_file = tmp_path / "helper-store.json"
    return {
        "PATH": f"{bin_dir}{os.pathsep}{os.environ.get('PATH', '')}",
        "DOCKER_CREDENTIAL_TEST_STORE": str(store_file),
        "_STORE_FILE": str(store_file),
    }


_HTPASSWD_USER = "testuser"
_HTPASSWD_PASSWORD = "testpass"
# bcrypt htpasswd line for testuser:testpass, committed as a constant so
# the suite needs no bcrypt dependency (generated once with
# ``htpasswd -Bbn testuser testpass``).
_HTPASSWD_LINE = "testuser:$2y$05$yR3/Pme3IBgbwaObz/q0g.3fpoX1FSKU3UeUDxvBQF.tijc89N85y"


def _free_port() -> int:
    with socket.socket() as s:
        s.bind(("", 0))
        return s.getsockname()[1]


def _wait_registry_up(host: str, timeout_s: float = 30.0) -> bool:
    """True once ``/v2/`` answers anything HTTP — a 401 from the auth gate
    counts as up."""
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(f"http://{host}/v2/", timeout=2):
                return True
        except urllib.error.HTTPError:
            return True
        except (urllib.error.URLError, OSError):
            time.sleep(0.5)
    return False


@pytest.fixture(scope="module")
def auth_registry() -> Iterator[str]:
    """An htpasswd-gated ``registry:2`` container on a free port.

    Yields the ``host:port`` string. Skips when docker is unavailable or
    the container cannot start — the same posture as the session registry
    fixture in ``test/conftest.py``. The htpasswd file is written inside
    the container (no volume mount) from the committed bcrypt line.
    """
    port = _free_port()
    host = f"127.0.0.1:{port}"
    name = f"grim-login-verify-{port}"
    try:
        run = subprocess.run(
            [
                "docker", "run", "-d", "--rm",
                "--name", name,
                "-p", f"{port}:5000",
                "-e", "REGISTRY_AUTH=htpasswd",
                "-e", "REGISTRY_AUTH_HTPASSWD_REALM=Registry Realm",
                "-e", "REGISTRY_AUTH_HTPASSWD_PATH=/auth/htpasswd",
                "--entrypoint", "sh",
                "registry:2",
                "-c",
                f"mkdir -p /auth && printf '%s\\n' '{_HTPASSWD_LINE}' > /auth/htpasswd"
                " && exec registry serve /etc/docker/registry/config.yml",
            ],
            capture_output=True,
            text=True,
        )
    except FileNotFoundError:
        pytest.skip("docker not available")
    if run.returncode != 0:
        pytest.skip(f"cannot start htpasswd registry container: {run.stderr.strip()}")
    if not _wait_registry_up(host):
        subprocess.run(["docker", "rm", "-f", name], capture_output=True)
        pytest.skip("htpasswd registry container did not become ready")
    yield host
    subprocess.run(["docker", "rm", "-f", name], capture_output=True)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _login(
    grim: GrimRunner,
    *args: str,
    docker_config: Path,
    stdin: str | None = None,
    extra_env: dict[str, str] | None = None,
    fmt: str | None = None,
) -> subprocess.CompletedProcess[str]:
    env = dict(grim.env)
    env["DOCKER_CONFIG"] = str(docker_config)
    if extra_env:
        env.update({k: v for k, v in extra_env.items() if not k.startswith("_")})
    cmd = [str(grim.binary)]
    if fmt:
        cmd += ["--format", fmt]
    cmd += ["login", *args]
    return subprocess.run(
        cmd,
        input=stdin,
        stdin=subprocess.DEVNULL if stdin is None else None,
        capture_output=True,
        text=True,
        env=env,
        cwd=str(grim.cwd) if grim.cwd else None,
    )


def _logout(
    grim: GrimRunner,
    *args: str,
    docker_config: Path,
    extra_env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    env = dict(grim.env)
    env["DOCKER_CONFIG"] = str(docker_config)
    if extra_env:
        env.update({k: v for k, v in extra_env.items() if not k.startswith("_")})
    return subprocess.run(
        [str(grim.binary), "logout", *args],
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        env=env,
        cwd=str(grim.cwd) if grim.cwd else None,
    )


def _read_config(docker_config: Path) -> dict:
    path = docker_config / "config.json"
    return json.loads(path.read_text()) if path.exists() else {}


# ---------------------------------------------------------------------------
# Plaintext-store path (no native helper)
# ---------------------------------------------------------------------------


def test_login_plaintext_writes_base64_entry(grim: GrimRunner, docker_config: Path) -> None:
    res = _login(
        grim,
        "-u", "alice", "--password-stdin", "--allow-insecure-store", "--no-verify", "ghcr.io",
        docker_config=docker_config,
        stdin="hunter2\n",
    )
    assert res.returncode == 0, res.stderr
    cfg = _read_config(docker_config)
    auth = cfg["auths"]["ghcr.io"]["auth"]
    assert base64.b64decode(auth).decode() == "alice:hunter2"


@pytest.mark.skipif(sys.platform == "win32", reason="POSIX file mode check")
def test_login_plaintext_config_is_owner_only(grim: GrimRunner, docker_config: Path) -> None:
    _login(
        grim,
        "-u", "u", "--password-stdin", "--allow-insecure-store", "--no-verify", "ghcr.io",
        docker_config=docker_config,
        stdin="p\n",
    )
    mode = (docker_config / "config.json").stat().st_mode & 0o777
    assert mode == 0o600, f"credentials file must be 0600, got {oct(mode)}"


def test_login_refused_without_helper_or_optin(grim: GrimRunner, docker_config: Path) -> None:
    res = _login(
        grim,
        "-u", "u", "--password-stdin", "--no-verify", "ghcr.io",
        docker_config=docker_config,
        stdin="p\n",
    )
    assert res.returncode == 78, res.stderr  # ConfigError
    assert "allow-insecure-store" in res.stderr or "credential helper" in res.stderr


def test_login_canonicalizes_registry_key(grim: GrimRunner, docker_config: Path) -> None:
    res = _login(
        grim,
        "-u", "u", "--password-stdin", "--allow-insecure-store", "--no-verify", "https://ghcr.io/v1/",
        docker_config=docker_config,
        stdin="p\n",
    )
    assert res.returncode == 0, res.stderr
    cfg = _read_config(docker_config)
    assert "ghcr.io" in cfg["auths"], cfg


# ---------------------------------------------------------------------------
# Usage / input errors
# ---------------------------------------------------------------------------


def test_login_noninteractive_requires_password_stdin(grim: GrimRunner, docker_config: Path) -> None:
    # No --password-stdin and stdin is /dev/null (not a TTY) → usage error.
    res = _login(grim, "-u", "u", "--allow-insecure-store", "ghcr.io", docker_config=docker_config)
    assert res.returncode == 64, res.stderr  # UsageError
    assert "--password-stdin" in res.stderr


def test_login_rejects_password_value_flag(grim: GrimRunner, docker_config: Path) -> None:
    # CWE-214: there is no --password VALUE flag; clap rejects it at parse.
    res = _login(grim, "--password", "secret", "ghcr.io", docker_config=docker_config)
    assert res.returncode == 64, res.stderr


def test_login_empty_password_stdin_is_usage_error(grim: GrimRunner, docker_config: Path) -> None:
    res = _login(
        grim,
        "-u", "u", "--password-stdin", "--allow-insecure-store", "ghcr.io",
        docker_config=docker_config,
        stdin="",
    )
    assert res.returncode == 64, res.stderr


# ---------------------------------------------------------------------------
# JSON output
# ---------------------------------------------------------------------------


def test_login_json_output(grim: GrimRunner, docker_config: Path) -> None:
    res = _login(
        grim,
        "-u", "alice", "--password-stdin", "--allow-insecure-store", "--no-verify", "ghcr.io",
        docker_config=docker_config,
        stdin="p\n",
        fmt="json",
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload == {"registry": "ghcr.io", "username": "alice", "verification": "skipped"}


# ---------------------------------------------------------------------------
# Logout
# ---------------------------------------------------------------------------


def test_logout_removes_plaintext_entry(grim: GrimRunner, docker_config: Path) -> None:
    _login(
        grim,
        "-u", "u", "--password-stdin", "--allow-insecure-store", "--no-verify", "ghcr.io",
        docker_config=docker_config,
        stdin="p\n",
    )
    res = _logout(grim, "ghcr.io", docker_config=docker_config)
    assert res.returncode == 0, res.stderr
    assert "ghcr.io" not in _read_config(docker_config).get("auths", {})


def test_logout_noop_when_nothing_stored(grim: GrimRunner, docker_config: Path) -> None:
    res = _logout(grim, "ghcr.io", docker_config=docker_config)
    assert res.returncode == 0, res.stderr


# ---------------------------------------------------------------------------
# Native-helper path (mock docker-credential-test)
# ---------------------------------------------------------------------------


@pytest.mark.skipif(sys.platform == "win32", reason="mock helper is a POSIX script")
def test_login_via_helper_stores_credential(grim: GrimRunner, docker_config: Path, credential_helper: dict) -> None:
    res = _login(
        grim,
        "-u", "alice", "--password-stdin", "--no-verify", "ghcr.io",
        docker_config=docker_config,
        stdin="s3cret\n",
        extra_env=credential_helper,
    )
    assert res.returncode == 0, res.stderr
    # Credential landed in the helper's backing store, NOT in plaintext auths.
    store = json.loads(Path(credential_helper["_STORE_FILE"]).read_text())
    assert store["ghcr.io"] == {"Username": "alice", "Secret": "s3cret"}
    assert "auths" not in _read_config(docker_config)


@pytest.mark.skipif(sys.platform == "win32", reason="mock helper is a POSIX script")
def test_logout_via_helper_erases_credential(grim: GrimRunner, docker_config: Path, credential_helper: dict) -> None:
    _login(
        grim,
        "-u", "alice", "--password-stdin", "--no-verify", "ghcr.io",
        docker_config=docker_config,
        stdin="s3cret\n",
        extra_env=credential_helper,
    )
    res = _logout(grim, "ghcr.io", docker_config=docker_config, extra_env=credential_helper)
    assert res.returncode == 0, res.stderr
    store = json.loads(Path(credential_helper["_STORE_FILE"]).read_text())
    assert "ghcr.io" not in store


# ---------------------------------------------------------------------------
# [[registries]] resolution (ADR G5) — login/logout must consult the
# configured alias and default, consistent with add/search/release.
# ---------------------------------------------------------------------------


def test_login_resolves_configured_default_registry(
    grim_at, project_dir: Path, docker_config: Path
) -> None:
    """No positional registry, no ``--registry`` flag, no env — ``grim login``
    must resolve the project's ``[[registries]]`` default instead of erroring
    with "no registry given"."""
    (project_dir / "grimoire.toml").write_text(
        '[[registries]]\noci = "registry.corp.example"\ndefault = true\n'
    )
    runner = grim_at(project_dir)
    runner.env.pop("GRIM_DEFAULT_REGISTRY", None)

    res = _login(
        runner,
        "-u", "alice", "--password-stdin", "--allow-insecure-store", "--no-verify",
        docker_config=docker_config,
        stdin="hunter2\n",
        fmt="json",
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["registry"] == "registry.corp.example", payload
    cfg = _read_config(docker_config)
    assert "registry.corp.example" in cfg["auths"], cfg


def test_login_resolves_registries_alias(
    grim_at, project_dir: Path, docker_config: Path
) -> None:
    """A positional argument matching a configured ``[[registries]]`` alias
    substitutes that entry's url — mirroring the `alias/repo` resolution
    `add`/`search` already apply."""
    (project_dir / "grimoire.toml").write_text(
        '[[registries]]\nalias = "corp"\noci = "registry.corp.example"\n'
    )
    runner = grim_at(project_dir)
    runner.env.pop("GRIM_DEFAULT_REGISTRY", None)

    res = _login(
        runner,
        "-u", "alice", "--password-stdin", "--allow-insecure-store", "--no-verify", "corp",
        docker_config=docker_config,
        stdin="hunter2\n",
        fmt="json",
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["registry"] == "registry.corp.example", (
        f"the 'corp' alias must substitute its configured url, got: {payload!r}"
    )


def test_logout_resolves_registries_alias(
    grim_at, project_dir: Path, docker_config: Path
) -> None:
    """``grim logout`` resolves a configured alias the same way `login` does,
    so the pair round-trips against the same credential key."""
    (project_dir / "grimoire.toml").write_text(
        '[[registries]]\nalias = "corp"\noci = "registry.corp.example"\ndefault = true\n'
    )
    runner = grim_at(project_dir)
    runner.env.pop("GRIM_DEFAULT_REGISTRY", None)

    _login(
        runner,
        "-u", "alice", "--password-stdin", "--allow-insecure-store", "--no-verify",
        docker_config=docker_config,
        stdin="hunter2\n",
    )
    assert "registry.corp.example" in _read_config(docker_config).get("auths", {})

    res = _logout(runner, "corp", docker_config=docker_config)
    assert res.returncode == 0, res.stderr
    assert "registry.corp.example" not in _read_config(docker_config).get("auths", {})


def test_login_no_registry_anywhere_is_config_error(
    grim_at, project_dir: Path, docker_config: Path
) -> None:
    """Unlike `add`/`release`, `login` must never silently substitute the
    built-in fallback registry — storing a credential the user never named
    would be a silent surprise. Nothing configured anywhere ⇒ exit 78."""
    (project_dir / "grimoire.toml").write_text("[skills]\n\n[rules]\n")
    runner = grim_at(project_dir)
    runner.env.pop("GRIM_DEFAULT_REGISTRY", None)

    res = _login(
        runner,
        "-u", "alice", "--password-stdin", "--allow-insecure-store",
        docker_config=docker_config,
        stdin="hunter2\n",
    )
    assert res.returncode == 78, res.stderr
    assert "no registry" in res.stderr.lower(), res.stderr


# ---------------------------------------------------------------------------
# Credential verification (issue #37) — default-on registry ping before store
# ---------------------------------------------------------------------------


def test_login_verify_anonymous_registry_reports_no_auth_required(
    grim: GrimRunner, docker_config: Path, registry: str
) -> None:
    """Default verification against an anonymous ``registry:2``: ``/v2/``
    answers 2xx without a challenge, so there is nothing to verify — the
    credential stores with ``verification: no-auth-required``."""
    res = _login(
        grim,
        "-u", "alice", "--password-stdin", "--allow-insecure-store", registry,
        docker_config=docker_config,
        stdin="hunter2\n",
        fmt="json",
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["verification"] == "no-auth-required", payload
    assert registry in _read_config(docker_config)["auths"]


def test_login_verify_unreachable_registry_exits_69_and_stores_nothing(
    grim: GrimRunner, docker_config: Path
) -> None:
    res = _login(
        grim,
        "-u", "alice", "--password-stdin", "--allow-insecure-store", "127.0.0.1:1",
        docker_config=docker_config,
        stdin="hunter2\n",
        extra_env={"GRIM_INSECURE_REGISTRIES": "127.0.0.1:1"},
    )
    assert res.returncode == 69, res.stderr
    assert "127.0.0.1:1" not in _read_config(docker_config).get("auths", {})


def test_login_verify_bad_credentials_exits_80_and_stores_nothing(
    grim: GrimRunner, docker_config: Path, auth_registry: str
) -> None:
    """The htpasswd registry answers ``/v2/`` with a Basic challenge; a
    wrong password is rejected at login time, nothing persisted."""
    res = _login(
        grim,
        "-u", _HTPASSWD_USER, "--password-stdin", "--allow-insecure-store", auth_registry,
        docker_config=docker_config,
        stdin="wrong-password\n",
        extra_env={"GRIM_INSECURE_REGISTRIES": auth_registry},
    )
    assert res.returncode == 80, res.stderr
    assert auth_registry not in _read_config(docker_config).get("auths", {})


def test_login_verify_good_credentials_verifies_and_stores(
    grim: GrimRunner, docker_config: Path, auth_registry: str
) -> None:
    res = _login(
        grim,
        "-u", _HTPASSWD_USER, "--password-stdin", "--allow-insecure-store", auth_registry,
        docker_config=docker_config,
        stdin=f"{_HTPASSWD_PASSWORD}\n",
        fmt="json",
        extra_env={"GRIM_INSECURE_REGISTRIES": auth_registry},
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["verification"] == "verified", payload
    assert auth_registry in _read_config(docker_config)["auths"]


def test_login_verify_offline_is_blocked_exit_81(grim: GrimRunner, docker_config: Path) -> None:
    """Explicit ``--verify`` under ``GRIM_OFFLINE`` is a policy conflict —
    exit 81, nothing stored."""
    res = _login(
        grim,
        "-u", "alice", "--password-stdin", "--allow-insecure-store", "--verify", "ghcr.io",
        docker_config=docker_config,
        stdin="hunter2\n",
        extra_env={"GRIM_OFFLINE": "1"},
    )
    assert res.returncode == 81, res.stderr
    assert "ghcr.io" not in _read_config(docker_config).get("auths", {})


def test_login_offline_default_skips_verification(grim: GrimRunner, docker_config: Path) -> None:
    """Without an explicit ``--verify``, offline mode downgrades to a
    silent skip (warning on stderr) — the credential still stores."""
    res = _login(
        grim,
        "-u", "alice", "--password-stdin", "--allow-insecure-store", "ghcr.io",
        docker_config=docker_config,
        stdin="hunter2\n",
        fmt="json",
        extra_env={"GRIM_OFFLINE": "1"},
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["verification"] == "skipped", payload
    assert "ghcr.io" in _read_config(docker_config)["auths"]


def test_login_no_verify_stores_without_network(grim: GrimRunner, docker_config: Path) -> None:
    """``--no-verify`` preserves the store-only path: no network contact,
    so even an unreachable registry stores fine."""
    res = _login(
        grim,
        "-u", "alice", "--password-stdin", "--allow-insecure-store", "--no-verify", "127.0.0.1:1",
        docker_config=docker_config,
        stdin="hunter2\n",
        fmt="json",
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["verification"] == "skipped", payload
    assert "127.0.0.1:1" in _read_config(docker_config)["auths"]
