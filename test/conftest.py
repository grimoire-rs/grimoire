# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Shared fixtures for the Grimoire acceptance-test suite."""
from __future__ import annotations

import json
import os
import socket
import subprocess
import sys
import time
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

# Maximum number of repositories the acceptance suite tolerates in the shared
# registry before it declares the registry "polluted" and starts a fresh
# throwaway container on a dynamic port.
#
# Context: grim's bounded catalog walk fetches at most MAX_PAGES * PAGE_SIZE
# = 8 * 1 000 = 8 000 repos from the registry's /_catalog endpoint.  If the
# registry contains large numbers of repos that sort lexicographically before
# the "grim-test/" namespace (the prefix every acceptance-test repo uses),
# none of the test repos fall inside grim's browse window — making any test
# that calls `grim search --refresh` flaky (empty result).  A registry used
# exclusively for one test session never exceeds a few hundred repos, so 500
# is a comfortable threshold.
_CATALOG_REPO_LIMIT = 500

_REGISTRY_CONTAINER = "grim-acceptance-registry"


def _host_reachable(host: str, timeout: float = 2.0) -> bool:
    """Return True when the OCI ``/v2/`` endpoint on *host* answers."""
    try:
        with urllib.request.urlopen(
            f"http://{host}/v2/", timeout=timeout
        ) as resp:
            return resp.status in (200, 401)
    except Exception:
        return False


def _catalog_repo_count(host: str, timeout: float = 3.0) -> int:
    """Return the number of repos on the first catalog page (up to 1 000).

    Returns -1 when the registry is unreachable or the endpoint is gated.
    Checking only the first page is fast and sufficient: a polluted registry
    has thousands of repos, and we only care whether the count exceeds the
    threshold.
    """
    try:
        url = f"http://{host}/v2/_catalog?n=1000"
        with urllib.request.urlopen(url, timeout=timeout) as resp:
            data = json.loads(resp.read())
            return len(data.get("repositories", []))
    except Exception:
        return -1


def _free_port() -> int:
    """Return an unused TCP port on localhost."""
    with socket.socket() as s:
        s.bind(("", 0))
        return s.getsockname()[1]


def _start_registry_container(host_port: int, name: str) -> bool:
    """Start a ``registry:2`` container. Return True on success or name-in-use
    (a sibling xdist worker already won the race — treat as success)."""
    result = subprocess.run(
        [
            "docker", "run", "-d", "--rm",
            "--name", name,
            "-p", f"{host_port}:5000",
            "registry:2",
        ],
        capture_output=True,
        text=True,
    )
    name_in_use = "already in use" in (result.stderr or "").lower()
    return result.returncode == 0 or name_in_use


def _wait_registry_ready(host: str, timeout_s: float = 30.0) -> bool:
    """Block until the registry answers or the timeout expires."""
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        if _host_reachable(host):
            return True
        time.sleep(0.5)
    return False


# ---------------------------------------------------------------------------
# Pre-session hook: resolve the registry host BEFORE src.registry is imported
# ---------------------------------------------------------------------------


def pytest_configure(config: pytest.Config) -> None:  # noqa: ARG001
    """Resolve the registry host, then refuse to run without one.

    Split in two on purpose: ``_resolve_registry_host`` is best-effort (it
    may legitimately fail to start a container), ``_require_registry`` is
    the gate that turns that failure into a red run instead of a silently
    skipped suite.  The gate runs once per process tree — probing once per
    xdist worker would only add a way for the run to die on a transient blip.

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
    1. ``GRIM_TEST_REGISTRY_HOST`` already set → honour it, skip detection
       (also handles the xdist-worker case where the controller already set
       the variable before the worker process was spawned).
    2. ``localhost:5000`` unreachable → start a throwaway
       ``grim-acceptance-registry-<port>`` on a dynamic **non-5000** port and
       use it (the normal CI path).  The port is deliberately not 5000:
       that host is in grim's built-in plain-HTTP allowlist, which makes a
       config-declared ``insecure = true`` indistinguishable from the
       loopback default and forces every test of it to skip.
    3. ``localhost:5000`` reachable with ≤ ``_CATALOG_REPO_LIMIT`` repos →
       use it as-is (our own previous container or a freshly started CI
       registry).
    4. ``localhost:5000`` reachable but polluted (too many repos) → start a
       private throwaway container on a dynamic port and use that instead.
    """
    # Skip when env var is already set.  This covers:
    # - xdist workers (controller already set it before fork)
    # - manual override by developer or CI
    if os.environ.get("GRIM_TEST_REGISTRY_HOST"):
        return

    default_host = "localhost:5000"

    # Case 2: nothing on port 5000 — start a dedicated container on a
    # *dynamic* port. This is the normal CI path, and the port matters:
    # `localhost:5000` is in grim's built-in plain-HTTP allowlist, so a rig
    # on it cannot tell "grim reached the registry because the config
    # declared it insecure" from "grim reached it because the loopback
    # default let it through". Every test needing that distinction skipped
    # itself on the default port, which meant the plain-HTTP opt-in — the
    # whole `insecure = true` feature — was never once exercised in CI.
    #
    # A dynamic port also exercises `GrimRunner`'s `GRIM_INSECURE_REGISTRIES`
    # branch, dormant while the rig sat on the built-in allowlist.
    if not _host_reachable(default_host):
        fresh_port = _free_port()
        fresh_host = f"localhost:{fresh_port}"
        fresh_name = f"{_REGISTRY_CONTAINER}-{fresh_port}"
        ok = _start_registry_container(fresh_port, fresh_name)
        if ok and _wait_registry_ready(fresh_host):
            os.environ["GRIM_TEST_REGISTRY_HOST"] = fresh_host
            os.environ["_GRIM_FRESH_REGISTRY_NAME"] = fresh_name
        # If startup failed, leave env unset; `_require_registry` aborts.
        return

    # Case 3: reachable and small enough → reuse.
    count = _catalog_repo_count(default_host)
    if 0 <= count <= _CATALOG_REPO_LIMIT:
        os.environ["GRIM_TEST_REGISTRY_HOST"] = default_host
        return

    # Case 4: reachable but polluted → start a fresh throwaway container.
    fresh_port = _free_port()
    fresh_host = f"localhost:{fresh_port}"
    fresh_name = f"{_REGISTRY_CONTAINER}-{fresh_port}"

    started = _start_registry_container(fresh_port, fresh_name)
    if started and _wait_registry_ready(fresh_host):
        os.environ["GRIM_TEST_REGISTRY_HOST"] = fresh_host
        # Record name for teardown.
        os.environ["_GRIM_FRESH_REGISTRY_NAME"] = fresh_name
    else:
        # Fall back to the polluted registry and warn.
        _warn_polluted_registry(default_host, count)
        os.environ["GRIM_TEST_REGISTRY_HOST"] = default_host


def _require_registry(host: str) -> None:
    """Abort the whole session when no registry answers at *host*.

    Skipping instead is what this replaces, and skipping was indistinguishable
    from passing: with an unreachable host ``tests/test_registries.py`` reported
    ``9 passed, 21 skipped`` and **exit 0**, and CI runs plain ``task test``
    with no ``services:`` block — so a runner where the best-effort
    ``docker run registry:2`` fails produced a green Acceptance Tests job over
    a suite that exercised none of the registry-backed behaviour.

    Raised from ``pytest_configure``, so the failure is **one** message before
    collection (``ERROR: …``, exit 4) rather than one identical traceback per
    affected test — a session-scoped ``pytest.fail`` re-reports at every
    setup, which is 55 of the 66 test modules here.

    ``GRIM_ALLOW_NO_REGISTRY`` is the deliberate escape hatch for a machine
    with no docker: it restores the old skip-and-pass behaviour, which is why
    the message says outright that it is not a valid gate.
    """
    if host and _host_reachable(host):
        return
    if os.environ.get("GRIM_ALLOW_NO_REGISTRY"):
        # Opt-out taken: the `registry` fixture skips, as it always did.
        return
    where = host or "localhost:5000"
    raise pytest.UsageError(
        f"no OCI registry answers at {where}, and this suite could not start "
        f"one itself (is docker running?).\n"
        f"\n"
        f"Every registry-backed test would skip and the run would still exit "
        f"0 — a green gate over nothing. Refusing to start instead.\n"
        f"\n"
        f"  start one:     docker run -d --rm -p 5000:5000 "
        f"--name {_REGISTRY_CONTAINER} registry:2\n"
        f"  use another:   GRIM_TEST_REGISTRY_HOST=<host:port> uv run pytest\n"
        f"  skip them all: GRIM_ALLOW_NO_REGISTRY=1 uv run pytest   "
        f"(NOT a valid quality gate — most of the suite will not run)\n"
    )


def _warn_polluted_registry(host: str, count: int) -> None:
    print(  # noqa: T201  (called before logging is available)
        f"\n[grim-test] WARNING: registry on {host} has {count} repos "
        f"(limit: {_CATALOG_REPO_LIMIT}). Could not start a fresh throwaway "
        f"container. Catalog-browse tests (grim search --refresh) may return "
        f"empty results because grim's bounded catalog walk does not reach "
        f"the grim-test/ namespace. "
        f"Fix: `docker rm -f {_REGISTRY_CONTAINER}` then re-run.",
        flush=True,
    )


def pytest_unconfigure(config: pytest.Config) -> None:  # noqa: ARG001
    """Stop a fresh throwaway registry started by this session, if any.

    Only the controller process tears down the container — xdist workers
    also inherit ``_GRIM_FRESH_REGISTRY_NAME`` from the controller environment
    but must not remove the container themselves (another worker might still
    be using it).  ``PYTEST_XDIST_WORKER`` is set on workers but absent on
    the controller, making it a reliable distinguisher.
    """
    if os.environ.get("PYTEST_XDIST_WORKER"):
        # We are a worker process; let the controller handle teardown.
        return
    name = os.environ.pop("_GRIM_FRESH_REGISTRY_NAME", None)
    if name:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True)


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
    registry depends on this fixture.  The host was already chosen (and the
    container started, if needed) before any test-module import occurred, so
    ``src.registry.REGISTRY_HOST`` reflects the correct value throughout the
    session.

    **xdist-safe**: ``pytest_load_initial_conftests`` runs in the controller
    before workers fork; workers inherit ``GRIM_TEST_REGISTRY_HOST`` and
    import ``src.registry`` with the correct value.

    **Teardown**: ``pytest_unconfigure`` handles container cleanup.

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
