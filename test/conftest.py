# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Shared fixtures for the Grimoire acceptance-test suite."""
from __future__ import annotations

import json
import os
import sys
import urllib.error
import urllib.request
import uuid
from pathlib import Path

import pytest

# ---------------------------------------------------------------------------
# Project root — computed without importing src.registry so that the registry
# host can be patched (via GRIM_TEST_REGISTRY_HOST) before any module in
# src/ reads it at import time.
# ---------------------------------------------------------------------------

# conftest.py → test/ → project root (3 parents)
_PROJECT_ROOT = Path(__file__).resolve().parent

# ---------------------------------------------------------------------------
# Registry health helpers
# ---------------------------------------------------------------------------

# The zot registry this process started, if any. Only the process holding it
# stops it: xdist workers and nested pytest runs inherit the host through
# ``GRIM_TEST_REGISTRY_HOST`` but never the handle.
_zot = None


def _host_reachable(host: str, timeout: float = 2.0) -> bool:
    """Return True when the OCI ``/v2/`` endpoint on *host* answers."""
    try:
        with urllib.request.urlopen(
            f"http://{host}/v2/", timeout=timeout
        ) as resp:
            return resp.status in (200, 401)
    except Exception:
        return False


# ---------------------------------------------------------------------------
# Pre-session hook: resolve the registry host BEFORE src.registry is imported
# ---------------------------------------------------------------------------


def pytest_configure(config: pytest.Config) -> None:  # noqa: ARG001
    """Resolve the registry host, then refuse to run without one.

    Split in two on purpose: ``_resolve_registry_host`` starts the registry,
    ``_require_registry`` is the gate that turns an unreachable host into a
    red run instead of a silently skipped suite.  The gate runs once per
    process tree — probing once per xdist worker would only add a way for
    the run to die on a transient blip.

    "Once" is keyed on ``_GRIM_REGISTRY_VERIFIED``, which **this hook sets
    for itself** after the gate passes; workers inherit it at fork exactly
    like ``GRIM_TEST_REGISTRY_HOST``.  It deliberately is *not* keyed on
    ``PYTEST_XDIST_WORKER``, which is an **inherited** variable no process
    here sets: with it merely present in the ambient environment the gate
    skipped and an unreachable registry went back to ``21 passed, 21
    skipped``, exit 0 — the exact silently-green signature this gate exists
    to remove, reachable without anyone opting out (round-3 review, T-5).
    A process that has not verified for itself now always checks.
    """
    _resolve_registry_host()
    if not os.environ.get("_GRIM_REGISTRY_VERIFIED"):
        _require_registry(os.environ.get("GRIM_TEST_REGISTRY_HOST", ""))
        # Only after it returns: a raise must not mark the tree verified.
        os.environ["_GRIM_REGISTRY_VERIFIED"] = "1"


def _resolve_registry_host() -> None:
    """Resolve the registry host before any test module imports ``src.registry``.

    ``pytest_configure`` runs before test collection, which is when test
    modules (e.g. ``test_mcp.py``) first import ``src.registry``.  By setting
    ``GRIM_TEST_REGISTRY_HOST`` here we guarantee that ``src.registry`` reads
    the correct host when it is imported at module level
    (``REGISTRY_HOST = os.environ.get("GRIM_TEST_REGISTRY_HOST", ...)``)
    in both the controller and xdist worker processes (which inherit the
    environment variable at fork time).

    **Prerequisite**: ``conftest.py`` must have NO top-level ``src.*`` imports
    so that ``src.registry`` is not imported before this hook fires.

    Decision tree
    -------------
    1. ``GRIM_TEST_REGISTRY_HOST`` already set → honour it (a registry run
       by hand, or an xdist worker / nested run inheriting the controller's).
    2. Otherwise → start a private zot on a random loopback port
       (``src/zot.py``). The port is never the built-in ``:5000``: that host
       is in grim's plain-HTTP allowlist, which would make a config-declared
       ``insecure = true`` indistinguishable from the loopback default and
       force every test of it to skip. It also exercises ``GrimRunner``'s
       ``GRIM_INSECURE_REGISTRIES`` branch.
    """
    global _zot
    if os.environ.get("GRIM_TEST_REGISTRY_HOST"):
        return

    from src.zot import start_zot  # not top-level: see Prerequisite

    try:
        _zot = start_zot()
    except RuntimeError as e:
        if os.environ.get("GRIM_ALLOW_NO_REGISTRY"):
            return
        raise pytest.UsageError(str(e)) from e
    os.environ["GRIM_TEST_REGISTRY_HOST"] = _zot.host


def _require_registry(host: str) -> None:
    """Abort the whole session when no registry answers at *host*.

    Skipping instead is what this replaces, and skipping was indistinguishable
    from passing: with an unreachable host ``tests/test_registries.py`` reported
    ``9 passed, 21 skipped`` and **exit 0** — a green Acceptance Tests job over
    a suite that exercised none of the registry-backed behaviour.

    Raised from ``pytest_configure``, so the failure is **one** message before
    collection (``ERROR: …``, exit 4) rather than one identical traceback per
    affected test — a session-scoped ``pytest.fail`` re-reports at every
    setup, which is 55 of the 66 test modules here.

    ``GRIM_ALLOW_NO_REGISTRY`` is the deliberate escape hatch for a machine
    without zot: it restores the old skip-and-pass behaviour, which is why
    the message says outright that it is not a valid gate.
    """
    if host and _host_reachable(host):
        return
    if os.environ.get("GRIM_ALLOW_NO_REGISTRY"):
        # Opt-out taken: the `registry` fixture skips, as it always did.
        return
    raise pytest.UsageError(
        f"no OCI registry answers at {host or '(unset)'}.\n"
        f"\n"
        f"Every registry-backed test would skip and the run would still exit "
        f"0 — a green gate over nothing. Refusing to start instead.\n"
        f"\n"
        f"  own registry:  unset GRIM_TEST_REGISTRY_HOST; the suite starts zot\n"
        f"  use another:   GRIM_TEST_REGISTRY_HOST=<host:port> uv run pytest\n"
        f"  skip them all: GRIM_ALLOW_NO_REGISTRY=1 uv run pytest   "
        f"(NOT a valid quality gate — most of the suite will not run)\n"
    )


def pytest_unconfigure(config: pytest.Config) -> None:  # noqa: ARG001
    """Stop the zot this process started, if any (see ``_zot``)."""
    global _zot
    if _zot is not None:
        _zot.stop()
        _zot = None


# ---------------------------------------------------------------------------
# Session-scoped fixtures
# ---------------------------------------------------------------------------


@pytest.fixture(scope="session")
def grim_binary() -> Path:
    if env_path := os.environ.get("GRIM_COMMAND"):
        p = Path(env_path)
    else:
        # _PROJECT_ROOT is the test/ directory; the binary lives at test/bin/grim.
        p = _PROJECT_ROOT / "bin" / "grim"
        if sys.platform == "win32" and not p.suffix:
            p = p.with_suffix(".exe")
    assert p.exists(), f"grim binary not found at {p}"
    return p


@pytest.fixture(scope="session")
def registry() -> str:  # type: ignore[return]
    """The acceptance-suite registry host, resolved by
    ``pytest_load_initial_conftests``.

    Every test that drives network-facing grim commands against a real OCI
    registry depends on this fixture.  The host was already chosen (and zot
    started, if needed) before any test-module import occurred, so
    ``src.registry.REGISTRY_HOST`` reflects the correct value throughout the
    session.

    **xdist-safe**: ``pytest_load_initial_conftests`` runs in the controller
    before workers fork; workers inherit ``GRIM_TEST_REGISTRY_HOST`` and
    import ``src.registry`` with the correct value.

    **Teardown**: ``pytest_unconfigure`` stops the zot this session started.

    **The skip below is only reachable under ``GRIM_ALLOW_NO_REGISTRY``.**
    ``_require_registry`` has already aborted the session otherwise, so a
    skipped registry test now means someone asked for it explicitly — it can
    no longer be the silent default that made a broken runner look green.
    """
    from src.registry import REGISTRY_HOST

    host = os.environ.get("GRIM_TEST_REGISTRY_HOST", REGISTRY_HOST)
    if not _host_reachable(host):
        pytest.skip(f"no registry reachable at {host} (GRIM_ALLOW_NO_REGISTRY)")
    yield host


# ---------------------------------------------------------------------------
# Function-scoped fixtures
# ---------------------------------------------------------------------------


@pytest.fixture()
def grim_home(tmp_path: Path) -> Path:
    home = tmp_path / "grim-home"
    home.mkdir()
    return home


@pytest.fixture()
def grim(grim_binary: Path, grim_home: Path) -> "GrimRunner":
    from src.runner import GrimRunner

    return GrimRunner(grim_binary, grim_home)


@pytest.fixture()
def unique_repo() -> str:
    """A UUID-prefixed repository name, isolated per test on the shared
    registry."""
    return f"grim-test/{uuid.uuid4().hex[:12]}"


# ---------------------------------------------------------------------------
# Host managed Claude settings
# ---------------------------------------------------------------------------

# Claude Code's system managed-settings directory per OS
# (code.claude.com/docs/en/managed-settings). grim reads it for
# `env.CLAUDE_CONFIG_DIR`, and nothing in the per-test environment can mask it.
_CLAUDE_MANAGED_DIR = {
    "darwin": Path("/Library/Application Support/ClaudeCode"),
    "win32": Path(r"C:\Program Files\ClaudeCode"),
}.get(sys.platform, Path("/etc/claude-code"))


def _host_managed_claude_config_dir(managed_dir: Path = _CLAUDE_MANAGED_DIR) -> str | None:
    """The ``env.CLAUDE_CONFIG_DIR`` the host's managed settings impose, if
    any: ``managed-settings.json`` then ``managed-settings.d/*.json`` in name
    order, the last value set winning — the order grim itself reads."""
    drop_in_dir = managed_dir / "managed-settings.d"
    drop_ins = sorted(
        p for p in (drop_in_dir.iterdir() if drop_in_dir.is_dir() else ())
        if p.suffix == ".json" and not p.name.startswith(".")
    )
    found = None
    for path in [managed_dir / "managed-settings.json", *drop_ins]:
        try:
            value = json.loads(path.read_text()).get("env", {}).get("CLAUDE_CONFIG_DIR")
        except (OSError, ValueError, AttributeError):
            continue
        if isinstance(value, str) and value:
            found = value
    return found


@pytest.fixture(scope="session", autouse=True)
def _guard_host_managed_claude_settings() -> None:
    """Skip every ``--global`` grim run on a host whose managed Claude settings
    set ``CLAUDE_CONFIG_DIR``.

    grim honors that value exactly as Claude Code does, so on such a host a
    global install — or an autodetected one that picks Claude up — would
    write into the machine's REAL Claude config directory, outside the
    per-test ``$HOME``. The skip happens in ``GrimRunner.run`` before the
    process starts, so nothing is written."""
    from src import runner

    runner.HOST_MANAGED_CLAUDE_CONFIG_DIR = _host_managed_claude_config_dir()
