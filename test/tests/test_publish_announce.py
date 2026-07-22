# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim publish --announce` acceptance tests.

--announce records published packages in a package-index git repository:
clone → write index/<host>/<ns>/<pkg>/metadata.json → commit on a
deterministic topic branch → push → open the PR/MR via the forge API
(GitHub/GitLab), via git push options (token-less GitLab), or leave the
pushed branch (plain git host).

Hermetic setup: the announce target is a real-looking
`https://git.example.test/acme/index.git` URL that a `GIT_CONFIG_*`
insteadOf rewrite points at a local bare repository — the host derivation
and forge project-path parsing see the URL, git clone/push see the local
repo. Forge APIs are a local HTTP server injected via `[announce] api_url`
(or the CI env's `CI_API_V4_URL`). Explicit `owner_id` keeps most tests
free of owner-lookup traffic.
"""
from __future__ import annotations

import json
import shutil
import subprocess
import sys
import threading
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import quote

import pytest

from src.registry import REGISTRY_HOST

INDEX_URL = "https://git.example.test/acme/index.git"
INDEX_HOST = "git.example.test"
MR_URL = "https://git.example.test/acme/index/-/merge_requests/7"
PR_URL = "https://git.example.test/acme/index/pull/7"
TOKEN = "glpat-test-secret-value"

# ── auto-fork fixtures ─────────────────────────────────────────────────────
# The upstream index project derived from INDEX_URL (the `.git` suffix is
# stripped by grim's project-path parsing).
UPSTREAM_PROJECT = "acme/index"
UPSTREAM_ENCODED = quote(UPSTREAM_PROJECT, safe="")  # "acme%2Findex"
UPSTREAM_PROJECT_ID = 100
FORK_PROJECT_ID = 200
FORK_OWNER = "forkuser"
# The fork clone/push URL — a distinct host path a second insteadOf rewrite
# points at a second local bare repo (the "fork").
FORK_URL = "https://git.example.test/forkuser/index.git"
# A renamed fork: the upstream repo is `index`, the fork is `grimoire-index`.
RENAMED_FORK_FULL_NAME = "forkuser/grimoire-index"
RENAMED_FORK_URL = "https://git.example.test/forkuser/grimoire-index.git"


def _write(p: Path, body: str) -> None:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(body)


def _make_skill_source(
    project_dir: Path,
    name: str,
    description: str,
    keywords: str | None = None,
    summary: str | None = None,
) -> None:
    meta = f"  repository: https://github.com/acme/{name}\n"
    if keywords is not None:
        meta += f"  keywords: {keywords}\n"
    if summary is not None:
        meta += f"  summary: {summary}\n"
    _write(
        project_dir / "skills" / name / "SKILL.md",
        f"---\nname: {name}\ndescription: {description}\n"
        f"metadata:\n{meta}---\n# {name}\n",
    )


def _git(cwd: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", *args],
        cwd=cwd,
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout


def _bare_index_repo(tmp_path: Path) -> Path:
    """A seeded bare repository standing in for a custom index host."""
    seed = tmp_path / "index-seed"
    seed.mkdir()
    (seed / "README.md").write_text("# index\n")
    subprocess.run(["git", "init", "-q", str(seed)], check=True, capture_output=True)
    _git(seed, "add", "-A")
    _git(seed, "commit", "-q", "-m", "seed")
    bare = tmp_path / "index.git"
    subprocess.run(
        ["git", "clone", "--bare", "-q", str(seed), str(bare)],
        check=True,
        capture_output=True,
    )
    return bare


def _index_remote(tmp_path: Path, runner) -> Path:
    """Rewrite INDEX_URL to a local bare repo for every git grim spawns."""
    bare = _bare_index_repo(tmp_path)
    runner.env.update(
        {
            "GIT_CONFIG_COUNT": "1",
            "GIT_CONFIG_KEY_0": f"url.{bare}.insteadOf",
            "GIT_CONFIG_VALUE_0": INDEX_URL,
        }
    )
    return bare


def _index_and_fork_remote(tmp_path: Path, runner, fork_url: str = FORK_URL) -> tuple[Path, Path]:
    """Two local bare repos — the upstream index and a fork of it — with a
    two-entry insteadOf rewrite so INDEX_URL clones/pushes hit the upstream
    bare and `fork_url` (the fork's clone URL) hits the fork bare."""
    upstream = _bare_index_repo(tmp_path)
    fork = tmp_path / "fork.git"
    subprocess.run(
        ["git", "clone", "--bare", "-q", str(upstream), str(fork)],
        check=True,
        capture_output=True,
    )
    runner.env.update(
        {
            "GIT_CONFIG_COUNT": "2",
            "GIT_CONFIG_KEY_0": f"url.{upstream}.insteadOf",
            "GIT_CONFIG_VALUE_0": INDEX_URL,
            "GIT_CONFIG_KEY_1": f"url.{fork}.insteadOf",
            "GIT_CONFIG_VALUE_1": fork_url,
        }
    )
    return upstream, fork


def _heads(bare: Path) -> str:
    return _git(bare, "for-each-ref", "--format=%(refname:short)", "refs/heads/")


def _manifest(
    project_dir: Path,
    ns: str,
    name: str,
    repository: str,
    *,
    owner_id: int | None = 42,
    host: str | None = None,
    forge: str | None = None,
    api_url: str | None = None,
    fork: bool | str | None = None,
) -> None:
    announce = [f'repository = "{repository}"', 'namespace = "acme"']
    if owner_id is not None:
        announce.append(f"owner_id = {owner_id}")
    if host is not None:
        announce.append(f'host = "{host}"')
    if forge is not None:
        announce.append(f'forge = "{forge}"')
    if api_url is not None:
        announce.append(f'api_url = "{api_url}"')
    if fork is not None:
        # A str is an explicit policy ("never"/"auto"/"always"); a bool is the
        # legacy toggle.
        value = f'"{fork}"' if isinstance(fork, str) else ("true" if fork else "false")
        announce.append(f"fork = {value}")
    _write(
        project_dir / "publish.toml",
        f'registry = "{REGISTRY_HOST}"\n'
        f'repository_prefix = "{ns}"\n'
        f"\n[announce]\n" + "\n".join(announce) + "\n"
        f"\n[skills.{name}]\n"
        f'version = "0.1.0"\n',
    )


class _ForgeApi:
    """Minimal fake forge API (GitHub + GitLab routes), recording requests.

    `namespaces` shapes the GitLab `/namespaces/<path>` reply: "group"
    (default), "user" (a visible user namespace whose namespace id differs
    from the user id), or "missing" (404 — how a foreign user namespace
    looks to a project bot token, since the endpoint is membership-scoped).

    Auto-fork knobs: `push_access` drives the upstream repo/project
    `permissions` (True ⇒ the token can push, so no fork). `fork_exists`
    makes the conventional-path fork already present (reuse, created=false);
    otherwise a POST creates it. `fork_full_name` / `fork_clone_url` /
    `parent_full_name` / `forked_from_id` populate the fork response body —
    a mismatched parent exercises the security guard, a differing full_name
    the renamed-fork path. `import_status` / `import_error` drive the
    GitLab fork-readiness poll body (`"failed"` fast-fails with the error).
    `import_status` accepts a plain string (constant reply) or a list —
    consumed one entry per GET request that returns a fork body, last
    value sticky once exhausted — to exercise a pending→ready poll
    sequence. `pr_fails` makes the PR/MR-creation POST always return 500
    (a fork resolves successfully but the change-request API call fails,
    exercising the BranchPushed-with-fork outcome).

    GitLab identity-based reuse (a 409 on fork-create): `GET
    /projects/<upstream_id>/forks` and the numeric-id readiness poll `GET
    /projects/<FORK_PROJECT_ID>` both reply with `_fork_json()`, so the same
    knobs above (`fork_full_name`, `forked_from_id`, ...) shape the
    enumerated fork too.
    """

    def __init__(
        self,
        conflict: bool = False,
        namespaces: str = "group",
        *,
        push_access: bool = True,
        fork_exists: bool = False,
        fork_conflict: bool = False,
        fork_owner: str = FORK_OWNER,
        fork_full_name: str = f"{FORK_OWNER}/index",
        fork_clone_url: str = FORK_URL,
        parent_full_name: str = UPSTREAM_PROJECT,
        forked_from_id: int = UPSTREAM_PROJECT_ID,
        import_status: str | list[str] = "finished",
        import_error: str | None = None,
        pr_fails: bool = False,
    ) -> None:
        api = self
        self.requests: list[tuple[str, str]] = []
        self.bodies: list[tuple[str, dict]] = []
        self.conflict = conflict
        self.namespaces = namespaces
        self.push_access = push_access
        self.fork_exists = fork_exists
        self.fork_conflict = fork_conflict
        self.forked = False
        self.fork_owner = fork_owner
        self.fork_full_name = fork_full_name
        self.fork_clone_url = fork_clone_url
        self.parent_full_name = parent_full_name
        self.forked_from_id = forked_from_id
        self.import_status = import_status
        self._import_status_idx = 0
        self.import_error = import_error
        self.pr_fails = pr_fails

        class Handler(BaseHTTPRequestHandler):
            def _reply(self, code: int, body: object) -> None:
                payload = json.dumps(body).encode()
                self.send_response(code)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            def do_GET(self) -> None:  # noqa: N802 (http.server API)
                api.requests.append(("GET", self.path))
                if "/users?" in self.path:
                    self._reply(200, [{"id": 44, "username": "acme"}])
                elif self.path == "/user":
                    self._reply(200, {"login": api.fork_owner, "username": api.fork_owner})
                elif "/namespaces/" in self.path:
                    if api.namespaces == "user":
                        self._reply(200, {"kind": "user", "id": 999, "full_path": "acme"})
                    elif api.namespaces == "missing":
                        self._reply(404, {})
                    else:
                        self._reply(200, {"kind": "group", "id": 44, "full_path": "acme"})
                elif "/merge_requests?" in self.path:
                    self._reply(200, [{"web_url": MR_URL}])
                elif "/pulls?" in self.path:
                    self._reply(200, [{"html_url": PR_URL}])
                elif self.path in (f"/repos/{UPSTREAM_PROJECT}", f"/projects/{UPSTREAM_ENCODED}"):
                    self._reply(
                        200,
                        {
                            "default_branch": "main",
                            "id": UPSTREAM_PROJECT_ID,
                            "permissions": api._permissions(),
                        },
                    )
                elif api._is_fork_get(self.path):
                    if api.fork_exists or api.forked:
                        self._reply(200, api._fork_json(advance=True))
                    else:
                        self._reply(404, {})
                elif self.path.split("?", 1)[0] == f"/projects/{UPSTREAM_PROJECT_ID}/forks":
                    # GitLab existing-fork enumeration (409 reuse path):
                    # grim selects by forked_from_project.id + namespace and
                    # requests `?owned=true&per_page=100&page=N`, so match the
                    # path with the query stripped.
                    self._reply(200, [api._fork_json(advance=True)])
                elif self.path == f"/projects/{FORK_PROJECT_ID}":
                    # Numeric-id readiness poll after enumeration selects a fork.
                    self._reply(200, api._fork_json(advance=True))
                elif "/projects/" in self.path or "/repos/" in self.path:
                    self._reply(200, {"default_branch": "main"})
                else:
                    self._reply(404, {})

            def do_POST(self) -> None:  # noqa: N802 (http.server API)
                api.requests.append(("POST", self.path))
                raw = self.rfile.read(int(self.headers.get("Content-Length") or 0))
                try:
                    body = json.loads(raw) if raw else {}
                except ValueError:
                    body = {}
                api.bodies.append((self.path, body))
                if self.path.endswith("/forks"):  # GitHub fork (idempotent 202)
                    api.forked = True
                    self._reply(202, api._fork_json())
                elif self.path.endswith("/fork"):  # GitLab fork (201, or 409 if it exists)
                    api.forked = True
                    if api.fork_conflict:
                        self._reply(409, {"message": "409 Conflict: already forked"})
                    else:
                        self._reply(201, api._fork_json())
                elif self.path.endswith("/merge_requests"):
                    if api.pr_fails:
                        self._reply(500, {"message": "internal error"})
                    else:
                        self._reply(409 if api.conflict else 201, {"web_url": MR_URL})
                elif self.path.endswith("/pulls"):
                    if api.pr_fails:
                        self._reply(500, {"message": "internal error"})
                    else:
                        self._reply(422 if api.conflict else 201, {"html_url": PR_URL})
                else:
                    self._reply(404, {})

            def log_message(self, *args: object) -> None:
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.url = f"http://127.0.0.1:{self.server.server_port}"

    def _permissions(self) -> dict:
        """Permissions readable by both forge readers: GitHub `push` (bool)
        and GitLab `project_access.access_level` (Developer 30 = can push)."""
        if self.push_access:
            return {"push": True, "project_access": {"access_level": 40}, "group_access": None}
        return {"push": False, "project_access": {"access_level": 20}, "group_access": None}

    def _current_import_status(self) -> str:
        """The `import_status` for the current call: a scalar is constant,
        a list is indexed by how many GET-triggered calls have advanced it
        (sticky at the last entry once exhausted)."""
        if isinstance(self.import_status, list):
            idx = min(self._import_status_idx, len(self.import_status) - 1)
            return self.import_status[idx]
        return self.import_status

    def _fork_json(self, *, advance: bool = False) -> dict:
        """One body carrying both GitHub and GitLab fork fields (grim reads
        only the relevant ones per forge kind). `advance=True` (GET-triggered
        reads — the readiness poll) consumes the next `import_status` entry
        when it's a list; POST-triggered bodies (fork creation) never
        advance it, so a poll sequence starts at the list's first entry."""
        status = self._current_import_status()
        if advance and isinstance(self.import_status, list):
            self._import_status_idx = min(self._import_status_idx + 1, len(self.import_status) - 1)
        body = {
            "full_name": self.fork_full_name,
            "clone_url": self.fork_clone_url,
            "owner": {"login": self.fork_owner},
            "parent": {"full_name": self.parent_full_name},
            "id": FORK_PROJECT_ID,
            "path_with_namespace": self.fork_full_name,
            "http_url_to_repo": self.fork_clone_url,
            "forked_from_project": {"id": self.forked_from_id},
            "import_status": status,
        }
        if self.import_error is not None:
            body["import_error"] = self.import_error
        return body

    def _is_fork_get(self, path: str) -> bool:
        return path in (f"/repos/{self.fork_full_name}", f"/projects/{quote(self.fork_full_name, safe='')}")

    def close(self) -> None:
        self.server.shutdown()


@pytest.fixture
def forge_api():
    apis: list[_ForgeApi] = []

    def make(conflict: bool = False, namespaces: str = "group", **kwargs) -> _ForgeApi:
        api = _ForgeApi(conflict, namespaces, **kwargs)
        apis.append(api)
        return api

    yield make
    for api in apis:
        api.close()


def _announce_branch(bare: Path) -> str:
    branches = _git(bare, "branch", "--list", "announce/*")
    assert "announce/acme-" in branches, f"topic branch missing: {branches!r}"
    return branches.strip().lstrip("* ").strip()


# ── plain git host (no forge API, no token) ────────────────────────────────


def test_publish_announce_pushes_branch_with_metadata(
    grim_at, project_dir: Path, registry: str, tmp_path: Path
) -> None:
    """Plain host: pointers land under the URL-derived index/<host>/ path
    with the generic owner.login key. The bare repo does not advertise push
    options, so this also exercises the plain-push retry after the
    merge_request.create options push is rejected."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-skill"
    _make_skill_source(project_dir, name, "Announce me.")
    _manifest(project_dir, ns, name, INDEX_URL)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    result = runner.run("publish", "--announce", check=False)
    assert result.returncode == 0, f"publish --announce failed: {result.stderr}"
    assert "announced:" in result.stderr, result.stderr

    branch = _announce_branch(bare)
    blob = _git(bare, "show", f"{branch}:index/{INDEX_HOST}/acme/{name}/metadata.json")
    meta = json.loads(blob)
    assert meta["schema"] == 1
    assert meta["name"] == name
    assert meta["kind"] == "skill"
    assert meta["ref"] == f"{REGISTRY_HOST}/{ns}/{name}", meta
    assert meta["description"] == "Announce me."
    assert meta["owner"] == {"login": "acme", "id": 42}
    assert meta["repository"] == f"https://github.com/acme/{name}"


def test_publish_announce_carries_keywords_and_summary(
    grim_at, project_dir: Path, registry: str, tmp_path: Path
) -> None:
    """A skill published with metadata.keywords / metadata.summary announces
    a pointer that carries them, so an index-backed `grim search` can match
    them (a skill without those fields writes neither key)."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-kw"
    _make_skill_source(project_dir, name, "Announce me.", keywords="review, quality", summary="Terse review")
    _manifest(project_dir, ns, name, INDEX_URL)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    result = runner.run("publish", "--announce", check=False)
    assert result.returncode == 0, f"publish --announce failed: {result.stderr}"

    branch = _announce_branch(bare)
    blob = _git(bare, "show", f"{branch}:index/{INDEX_HOST}/acme/{name}/metadata.json")
    meta = json.loads(blob)
    assert meta["keywords"] == ["review", "quality"], meta
    assert meta["summary"] == "Terse review", meta


def test_publish_announce_is_repeatable(
    grim_at, project_dir: Path, registry: str, tmp_path: Path
) -> None:
    """A re-run (packages already pushed → skipped) still announces cleanly
    onto the same deterministic topic branch."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-repeat"
    _make_skill_source(project_dir, name, "Repeatable.")
    _manifest(project_dir, ns, name, INDEX_URL)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    first = runner.run("publish", "--announce", check=False)
    assert first.returncode == 0, first.stderr
    second = runner.run("publish", "--announce", check=False)
    assert second.returncode == 0, second.stderr

    branches = [
        b.strip().lstrip("* ").strip()
        for b in _git(bare, "branch", "--list", "announce/*").splitlines()
    ]
    assert len(branches) == 1, f"identical content must reuse one branch: {branches}"


def test_publish_announce_dry_run_touches_nothing(
    grim_at, project_dir: Path, registry: str, tmp_path: Path
) -> None:
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-dry"
    _make_skill_source(project_dir, name, "Dry.")
    _manifest(project_dir, ns, name, INDEX_URL)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    result = runner.run("publish", "--announce", "--dry-run", check=False)
    assert result.returncode == 0, result.stderr
    assert "announce: skipped (dry run)" in result.stderr

    branches = _git(bare, "branch", "--list", "announce/*")
    assert branches.strip() == "", f"dry run must not push: {branches!r}"


@pytest.mark.skipif(sys.platform == "win32", reason="shell hook fixture")
def test_publish_announce_push_options_reach_the_server(
    grim_at, project_dir: Path, registry: str, tmp_path: Path
) -> None:
    """A server that advertises push options receives merge_request.create
    (the mechanism GitLab uses to open the MR server-side without a token)."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-opts"
    _make_skill_source(project_dir, name, "Options.")
    _manifest(project_dir, ns, name, INDEX_URL)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    _git(bare, "config", "receive.advertisePushOptions", "true")
    seen = bare / "push-options.txt"
    hook = bare / "hooks" / "post-receive"
    hook.write_text(f'#!/bin/sh\necho "${{GIT_PUSH_OPTION_0:-none}}" > "{seen}"\n')
    hook.chmod(0o755)

    result = runner.run("publish", "--announce", check=False)
    assert result.returncode == 0, result.stderr
    assert seen.read_text().strip() == "merge_request.create", (
        f"push option not received: {seen.read_text()!r}"
    )


# ── misconfiguration exits usage (64) ──────────────────────────────────────


def test_publish_announce_local_path_requires_host(
    grim_at, project_dir: Path, registry: str, tmp_path: Path
) -> None:
    """A locator without a derivable host (a local path) needs an explicit
    `[announce] host` — exit 64 names the key."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-nohost"
    _make_skill_source(project_dir, name, "No host.")
    bare = _bare_index_repo(tmp_path)
    _manifest(project_dir, ns, name, str(bare))

    runner = grim_at(project_dir)
    result = runner.run("publish", "--announce", check=False)
    assert result.returncode == 64, f"expected 64, got {result.returncode}: {result.stderr}"
    assert "[announce] host" in result.stderr


def test_publish_announce_plain_host_requires_owner_id(
    grim_at, project_dir: Path, registry: str, tmp_path: Path
) -> None:
    """Without a forge API to resolve it from, owner_id must be explicit."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-noowner"
    _make_skill_source(project_dir, name, "No owner.")
    _manifest(project_dir, ns, name, INDEX_URL, owner_id=None)

    runner = grim_at(project_dir)
    _index_remote(tmp_path, runner)
    result = runner.run("publish", "--announce", format="json", check=False)
    assert result.returncode == 64, f"expected 64, got {result.returncode}: {result.stderr}"
    assert "owner_id" in result.stderr
    # The publish succeeded before announce config failed: the JSON report
    # still renders the pushed entries with a null announce (exit 64, mirroring
    # the exit-69 failure path).
    data = json.loads(result.stdout)
    assert data["announce"] is None, data
    assert data["items"][0]["status"] == "pushed", data


def test_publish_announce_unreachable_index_exits_unavailable(
    grim_at, project_dir: Path, registry: str, tmp_path: Path
) -> None:
    """A failing announce after a successful publish exits 69 (the packages
    ARE published; only the announcement needs a retry)."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-fail"
    _make_skill_source(project_dir, name, "Unreachable index.")
    # host set explicitly so the failure is the clone, not host derivation
    _manifest(project_dir, ns, name, str(tmp_path / "no-such-repo.git"), host=INDEX_HOST)

    runner = grim_at(project_dir)
    result = runner.run("publish", "--announce", check=False)
    assert result.returncode == 69, f"expected 69, got {result.returncode}: {result.stderr}"
    assert "announce failed" in result.stderr


# ── forge APIs (fake server via api_url / CI env) ──────────────────────────


def test_publish_announce_gitlab_forge_opens_mr_via_api(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-mr"
    _make_skill_source(project_dir, name, "MR me.")
    api = forge_api()
    _manifest(project_dir, ns, name, INDEX_URL, forge="gitlab", api_url=api.url)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    result = runner.run("publish", "--announce", check=False)
    assert result.returncode == 0, result.stderr
    assert f"announced: {MR_URL}" in result.stderr, result.stderr
    assert ("POST", "/projects/acme%2Findex/merge_requests") in api.requests, api.requests
    _announce_branch(bare)  # the branch is pushed before the MR opens
    assert TOKEN not in result.stdout + result.stderr, "token must never be printed"


def test_publish_announce_gitlab_conflict_reuses_open_mr(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """A 409 (MR already open for the branch) reuses the existing MR URL."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-409"
    _make_skill_source(project_dir, name, "Conflict.")
    api = forge_api(conflict=True)
    _manifest(project_dir, ns, name, INDEX_URL, forge="gitlab", api_url=api.url)

    runner = grim_at(project_dir)
    _index_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    result = runner.run("publish", "--announce", check=False)
    assert result.returncode == 0, result.stderr
    assert f"announced: {MR_URL}" in result.stderr, result.stderr
    assert any(m == "GET" and "/merge_requests?" in p for m, p in api.requests), api.requests


def test_publish_announce_github_forge_opens_pr_via_api(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-pr"
    _make_skill_source(project_dir, name, "PR me.")
    api = forge_api()
    _manifest(project_dir, ns, name, INDEX_URL, forge="github", api_url=api.url)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    result = runner.run("publish", "--announce", check=False)
    assert result.returncode == 0, result.stderr
    assert f"announced: {PR_URL}" in result.stderr, result.stderr
    assert ("POST", "/repos/acme/index/pulls") in api.requests, api.requests
    branch = _announce_branch(bare)
    # GitHub-forge pointers keep the spec-v1 owner.github key.
    blob = _git(bare, "show", f"{branch}:index/{INDEX_HOST}/acme/{name}/metadata.json")
    assert json.loads(blob)["owner"] == {"github": "acme", "id": 42}


def test_publish_announce_owner_id_resolves_via_gitlab_api(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """Without an explicit owner_id, a tokened GitLab forge resolves the
    namespace id from /namespaces/<path>."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-ownerapi"
    _make_skill_source(project_dir, name, "Owner lookup.")
    api = forge_api()
    _manifest(project_dir, ns, name, INDEX_URL, owner_id=None, forge="gitlab", api_url=api.url)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    result = runner.run("publish", "--announce", check=False)
    assert result.returncode == 0, result.stderr
    assert ("GET", "/namespaces/acme") in api.requests, api.requests
    branch = _announce_branch(bare)
    blob = _git(bare, "show", f"{branch}:index/{INDEX_HOST}/acme/{name}/metadata.json")
    assert json.loads(blob)["owner"] == {"login": "acme", "id": 44}


def test_publish_announce_owner_id_user_namespace_uses_user_id(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """User namespaces resolve owner.id through the public /users lookup:
    /namespaces is membership-scoped (a project bot token 404s on foreign
    user namespaces), and owner.id carries the publicly verifiable user id."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-userns"
    _make_skill_source(project_dir, name, "User namespace.")
    api = forge_api(namespaces="missing")
    _manifest(project_dir, ns, name, INDEX_URL, owner_id=None, forge="gitlab", api_url=api.url)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    result = runner.run("publish", "--announce", check=False)
    assert result.returncode == 0, result.stderr
    assert any(m == "GET" and p.startswith("/users?username=acme") for m, p in api.requests), api.requests
    branch = _announce_branch(bare)
    blob = _git(bare, "show", f"{branch}:index/{INDEX_HOST}/acme/{name}/metadata.json")
    assert json.loads(blob)["owner"] == {"login": "acme", "id": 44}


def test_publish_announce_owner_id_visible_user_namespace_uses_user_id(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """Even when /namespaces succeeds for a user namespace (the publisher's
    own token sees it), owner.id is the user id — never the namespace id,
    which an index validator's bot token cannot verify."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-userns2"
    _make_skill_source(project_dir, name, "Visible user namespace.")
    api = forge_api(namespaces="user")
    _manifest(project_dir, ns, name, INDEX_URL, owner_id=None, forge="gitlab", api_url=api.url)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    result = runner.run("publish", "--announce", check=False)
    assert result.returncode == 0, result.stderr
    branch = _announce_branch(bare)
    blob = _git(bare, "show", f"{branch}:index/{INDEX_HOST}/acme/{name}/metadata.json")
    assert json.loads(blob)["owner"] == {"login": "acme", "id": 44}, blob


# ── CI environment auto-detection (host-match gated) ───────────────────────


def test_publish_announce_gitlab_ci_env_autoconfigures(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """In GitLab CI with a matching server host, forge/api/token come from
    the environment — zero `[announce]` forge config needed."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-cienv"
    _make_skill_source(project_dir, name, "CI env.")
    api = forge_api()
    _manifest(project_dir, ns, name, INDEX_URL)

    runner = grim_at(project_dir)
    _index_remote(tmp_path, runner)
    runner.env.update(
        {
            "GITLAB_CI": "true",
            "CI_SERVER_HOST": INDEX_HOST,
            "CI_API_V4_URL": api.url,
            "GITLAB_TOKEN": TOKEN,
        }
    )
    result = runner.run("publish", "--announce", check=False)
    assert result.returncode == 0, result.stderr
    assert f"announced: {MR_URL}" in result.stderr, result.stderr
    assert ("POST", "/projects/acme%2Findex/merge_requests") in api.requests, api.requests
    assert TOKEN not in result.stdout + result.stderr, "token must never be printed"


def test_publish_announce_ci_env_host_mismatch_is_ignored(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """A GitLab pipeline announcing to a foreign host must not inherit the
    GitLab CI credentials or API — the announce degrades to a plain push."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-mismatch"
    _make_skill_source(project_dir, name, "Mismatch.")
    api = forge_api()
    _manifest(project_dir, ns, name, INDEX_URL)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    runner.env.update(
        {
            "GITLAB_CI": "true",
            "CI_SERVER_HOST": "other.example.test",
            "CI_API_V4_URL": api.url,
            "GITLAB_TOKEN": TOKEN,
        }
    )
    result = runner.run("publish", "--announce", check=False)
    assert result.returncode == 0, result.stderr
    assert api.requests == [], f"mismatched CI host must not reach the API: {api.requests}"
    _announce_branch(bare)
    assert TOKEN not in result.stdout + result.stderr, "token must never be printed"


# ── machine-readable announce report (--format json) ───────────────────────


def test_publish_announce_json_reports_branch_pushed(
    grim_at, project_dir: Path, registry: str, tmp_path: Path
) -> None:
    """Plain host: the JSON wrapper carries announce.outcome and the
    deterministic topic branch — CI consumes it without grepping stderr."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-json"
    _make_skill_source(project_dir, name, "JSON me.")
    _manifest(project_dir, ns, name, INDEX_URL)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    data = runner.json("publish", "--announce")
    assert data["items"][0]["status"] == "pushed", data
    announce = data["announce"]
    assert announce["outcome"] == "branch-pushed", data
    assert announce["branch"] == _announce_branch(bare)
    # Always-present-null: the url key is present, explicit null off
    # the pull-request outcome.
    assert "url" in announce, f"url key must always be present: {announce}"
    assert announce["url"] is None, f"url must be null off pull-request: {announce}"
    # Same contract for fork: present as a key, null on an upstream push.
    assert "fork" in announce, f"fork key must always be present: {announce}"
    assert announce["fork"] is None, f"fork must be null on an upstream push: {announce}"


def test_publish_announce_json_pull_request_carries_url_and_branch(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-json-mr"
    _make_skill_source(project_dir, name, "JSON MR.")
    api = forge_api()
    _manifest(project_dir, ns, name, INDEX_URL, forge="gitlab", api_url=api.url)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    result = runner.run("publish", "--announce", format="json", check=False)
    assert result.returncode == 0, result.stderr
    # The JSON announce section is a distinct serialization path — assert the
    # forge token never leaks through it (parity with the plain-format sibling).
    assert TOKEN not in result.stdout + result.stderr, "announce token must never be printed"
    data = json.loads(result.stdout)
    announce = data["announce"]
    assert announce["outcome"] == "pull-request", data
    assert announce["url"] == MR_URL
    assert announce["branch"] == _announce_branch(bare)


def test_publish_announce_json_up_to_date_keeps_branch(
    grim_at, project_dir: Path, registry: str, tmp_path: Path
) -> None:
    """Once the index default branch carries the metadata (MR merged), a
    re-announce reports up-to-date WITH the branch."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-json-utd"
    _make_skill_source(project_dir, name, "Up to date.")
    _manifest(project_dir, ns, name, INDEX_URL)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    first = runner.run("publish", "--announce", check=False)
    assert first.returncode == 0, first.stderr
    # "Merge" the announce MR: fast-forward the default branch to the topic.
    branch = _announce_branch(bare)
    default = _git(bare, "symbolic-ref", "--short", "HEAD").strip()
    _git(bare, "update-ref", f"refs/heads/{default}", f"refs/heads/{branch}")

    data = runner.json("publish", "--announce")
    announce = data["announce"]
    assert announce["outcome"] == "up-to-date", data
    assert announce["branch"] == branch
    # Always-present-null: the url key is present and explicit null off
    # the pull-request outcome.
    assert "url" in announce, announce
    assert announce["url"] is None, announce


def test_publish_announce_json_dry_run_announce_is_null(
    grim_at, project_dir: Path, registry: str, tmp_path: Path
) -> None:
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-json-dry"
    _make_skill_source(project_dir, name, "Dry JSON.")
    _manifest(project_dir, ns, name, INDEX_URL)

    runner = grim_at(project_dir)
    _index_remote(tmp_path, runner)
    result = runner.run("publish", "--announce", "--dry-run", format="json", check=False)
    assert result.returncode == 0, result.stderr
    assert "announce: skipped (dry run)" in result.stderr
    data = json.loads(result.stdout)
    assert data["announce"] is None, data
    assert data["items"][0]["status"] == "dry-run"


def test_publish_announce_json_failure_keeps_entries(
    grim_at, project_dir: Path, registry: str, tmp_path: Path
) -> None:
    """announce failure after a successful publish: exit 69, the JSON report
    still renders with the pushed entries and announce null."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-json-fail"
    _make_skill_source(project_dir, name, "Fail JSON.")
    _manifest(project_dir, ns, name, str(tmp_path / "no-such-repo.git"), host=INDEX_HOST)

    runner = grim_at(project_dir)
    result = runner.run("publish", "--announce", format="json", check=False)
    assert result.returncode == 69, f"expected 69, got {result.returncode}: {result.stderr}"
    data = json.loads(result.stdout)
    assert data["announce"] is None, data
    assert data["items"][0]["status"] == "pushed", data


def test_publish_announce_json_fail_fast_keeps_announce_null(
    grim_at, project_dir: Path, registry: str, tmp_path: Path
) -> None:
    """A mid-batch publish failure stops before the announce step: exit
    non-zero, the JSON report's announce is null, and no topic branch is
    pushed to the index. Locks the fail-fast-before-announce ordering — a
    refactor that announced a partially-failed batch would break this."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    _make_skill_source(project_dir, "aaa-ok", "Valid first entry.")
    # An empty dir passes the upfront path check but fails at build time (no
    # SKILL.md), injecting a deterministic mid-batch failure after aaa-ok
    # pushes (kinds publish alphabetically: aaa-ok before zzz-broken).
    (project_dir / "skills" / "zzz-broken").mkdir(parents=True)
    _write(
        project_dir / "publish.toml",
        f'registry = "{REGISTRY_HOST}"\n'
        f'repository_prefix = "{ns}"\n'
        f'\n[announce]\nrepository = "{INDEX_URL}"\nnamespace = "acme"\nowner_id = 42\n'
        f'\n[skills.aaa-ok]\nversion = "0.1.0"\n'
        f'\n[skills.zzz-broken]\nversion = "0.1.0"\n',
    )

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    result = runner.run("publish", "--announce", format="json", check=False)
    assert result.returncode != 0, f"fail-fast must exit non-zero: {result.stderr}"
    data = json.loads(result.stdout)
    assert data["announce"] is None, f"announce must be null on fail-fast: {data}"
    heads = _git(bare, "for-each-ref", "--format=%(refname:short)", "refs/heads/")
    assert "announce/" not in heads, f"no announce branch may be pushed: {heads!r}"


# ── GitLab CI job-token transport fallback ─────────────────────────────────


def test_publish_announce_job_token_is_inert_and_never_leaks(
    grim_at, project_dir: Path, registry: str, tmp_path: Path
) -> None:
    """GITLAB_CI + CI_JOB_TOKEN + matching host injects the fallback
    credential helper; a transport that needs no credential (the local
    insteadOf rewrite) is unaffected, and the token never appears in
    output."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-jobtok"
    _make_skill_source(project_dir, name, "Job token.")
    _manifest(project_dir, ns, name, INDEX_URL)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    secret = "glcbt-job-token-secret"
    runner.env.update(
        {
            "GITLAB_CI": "true",
            "CI_SERVER_HOST": INDEX_HOST,
            "CI_JOB_TOKEN": secret,
        }
    )
    data = runner.json("publish", "--announce")
    assert data["announce"]["outcome"] == "branch-pushed", data
    _announce_branch(bare)
    result = runner.run("publish", "--announce", check=False)
    assert secret not in result.stdout + result.stderr, "job token must never be printed"


@pytest.mark.skipif(sys.platform == "win32", reason="shell shim fixture")
def test_publish_announce_job_token_reaches_git_argv(
    grim_at, project_dir: Path, registry: str, tmp_path: Path
) -> None:
    """The credential helper config reaches git's argv on clone and push —
    and only the literal ${CI_JOB_TOKEN} reference, never the token value."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-shim"
    _make_skill_source(project_dir, name, "Shim.")
    _manifest(project_dir, ns, name, INDEX_URL)

    runner = grim_at(project_dir)
    _index_remote(tmp_path, runner)
    real_git = shutil.which("git")
    assert real_git, "git required on PATH"
    log = tmp_path / "git-argv.log"
    shim_dir = tmp_path / "git-shim"
    shim_dir.mkdir()
    shim = shim_dir / "git"
    shim.write_text(f'#!/bin/sh\necho "$@" >> "{log}"\nexec "{real_git}" "$@"\n')
    shim.chmod(0o755)
    runner.env["PATH"] = f"{shim_dir}:{runner.env['PATH']}"

    secret = "glcbt-job-token-secret"
    runner.env.update(
        {
            "GITLAB_CI": "true",
            "CI_SERVER_HOST": INDEX_HOST,
            "CI_JOB_TOKEN": secret,
        }
    )
    result = runner.run("publish", "--announce", check=False)
    assert result.returncode == 0, result.stderr

    lines = log.read_text().splitlines()
    scoped_key = f"credential.https://{INDEX_HOST}.helper="
    clone_lines = [l for l in lines if "clone" in l.split() and scoped_key in l]
    push_lines = [l for l in lines if "push" in l.split() and scoped_key in l]
    assert clone_lines, f"clone must carry the host-scoped credential helper: {lines}"
    assert push_lines, f"push must carry the host-scoped credential helper: {lines}"
    argv_log = log.read_text()
    assert "gitlab-ci-token" in argv_log
    # Host-scoped key, never the bare global `credential.helper` — so git only
    # offers the token for the gated host (Clone2Leak / CVE-2024-53858 class).
    assert "credential.helper=" not in argv_log, "the helper key must be host-scoped"
    assert secret not in argv_log, "the token value must never enter git argv"


@pytest.mark.skipif(sys.platform == "win32", reason="shell shim fixture")
def test_publish_announce_job_token_foreign_host_not_injected(
    grim_at, project_dir: Path, registry: str, tmp_path: Path
) -> None:
    """A mismatched CI_SERVER_HOST must not inject the job-token helper."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-shim-foreign"
    _make_skill_source(project_dir, name, "Foreign.")
    _manifest(project_dir, ns, name, INDEX_URL)

    runner = grim_at(project_dir)
    _index_remote(tmp_path, runner)
    real_git = shutil.which("git")
    assert real_git, "git required on PATH"
    log = tmp_path / "git-argv.log"
    shim_dir = tmp_path / "git-shim"
    shim_dir.mkdir()
    shim = shim_dir / "git"
    shim.write_text(f'#!/bin/sh\necho "$@" >> "{log}"\nexec "{real_git}" "$@"\n')
    shim.chmod(0o755)
    runner.env["PATH"] = f"{shim_dir}:{runner.env['PATH']}"
    runner.env.update(
        {
            "GITLAB_CI": "true",
            "CI_SERVER_HOST": "other.example.test",
            "CI_JOB_TOKEN": "glcbt-job-token-secret",
        }
    )
    result = runner.run("publish", "--announce", check=False)
    assert result.returncode == 0, result.stderr
    text = log.read_text()
    assert "credential.helper=" not in text and "credential.https://" not in text, (
        "helper must not be injected for a foreign host"
    )


@pytest.mark.skipif(sys.platform == "win32", reason="shell shim fixture")
def test_publish_announce_job_token_empty_not_injected(
    grim_at, project_dir: Path, registry: str, tmp_path: Path
) -> None:
    """An empty CI_JOB_TOKEN (set but blank) must not inject the helper —
    locks the non-empty filter end to end, even on a matching host."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-shim-empty"
    _make_skill_source(project_dir, name, "Empty token.")
    _manifest(project_dir, ns, name, INDEX_URL)

    runner = grim_at(project_dir)
    _index_remote(tmp_path, runner)
    real_git = shutil.which("git")
    assert real_git, "git required on PATH"
    log = tmp_path / "git-argv.log"
    shim_dir = tmp_path / "git-shim"
    shim_dir.mkdir()
    shim = shim_dir / "git"
    shim.write_text(f'#!/bin/sh\necho "$@" >> "{log}"\nexec "{real_git}" "$@"\n')
    shim.chmod(0o755)
    runner.env["PATH"] = f"{shim_dir}:{runner.env['PATH']}"
    runner.env.update(
        {"GITLAB_CI": "true", "CI_SERVER_HOST": INDEX_HOST, "CI_JOB_TOKEN": ""}
    )
    result = runner.run("publish", "--announce", check=False)
    assert result.returncode == 0, result.stderr
    text = log.read_text()
    assert "credential.helper=" not in text and "credential.https://" not in text, (
        "an empty job token must not inject the helper"
    )


# ── HOME-less runners (GitLab step environments) ────────────────────────────


def test_publish_announce_succeeds_without_home(
    grim_at, project_dir: Path, registry: str, tmp_path: Path
) -> None:
    """GitLab step environments don't set HOME — announce must not need it
    (identity via -c, registry credentials degrade to anonymous)."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-nohome"
    _make_skill_source(project_dir, name, "No HOME.")
    _manifest(project_dir, ns, name, INDEX_URL)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    del runner.env["HOME"]
    result = runner.run("publish", "--announce", check=False)
    assert result.returncode == 0, f"announce must not need HOME: {result.stderr}"
    _announce_branch(bare)


# ── push/pull registry split (issue #39) ────────────────────────────────────


def test_announce_pointer_keeps_pull_reference(
    grim_at, project_dir: Path, registry: str, tmp_path: Path
) -> None:
    """Under a push/pull split the announce pointer `ref` keeps the PULL
    name while the metadata read-back succeeded via the push endpoint:
    the pointer carries the real published description (not the degraded
    fallback), even though the pull host is the reserved-unresolvable
    `pull.invalid`."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-split"
    description = "Announced via the push endpoint."
    _make_skill_source(project_dir, name, description)
    _write(
        project_dir / "publish.toml",
        f'registry = "pull.invalid"\n'
        f'push_registry = "{REGISTRY_HOST}"\n'
        f'repository_prefix = "{ns}"\n'
        f"\n[announce]\n"
        f'repository = "{INDEX_URL}"\n'
        f'namespace = "acme"\n'
        f"owner_id = 42\n"
        f"\n[skills.{name}]\n"
        f'version = "0.1.0"\n',
    )

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    result = runner.run("publish", "--announce", check=False)
    assert result.returncode == 0, f"publish --announce under the split failed: {result.stderr}"

    branch = _announce_branch(bare)
    blob = _git(bare, "show", f"{branch}:index/{INDEX_HOST}/acme/{name}/metadata.json")
    meta = json.loads(blob)
    assert meta["ref"] == f"pull.invalid/{ns}/{name}", (
        f"the pointer ref must keep the pull name, got {meta['ref']!r}"
    )
    assert meta["description"] == description, (
        "the metadata read-back must have succeeded via the push endpoint "
        f"(a pull-name lookup would degrade to the fallback), got {meta['description']!r}"
    )


# ── auto-fork on no push access (issue: announce-fork) ──────────────────────


def test_publish_announce_github_forks_when_no_push_access(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """No push access to the upstream index: grim forks it, pushes the branch
    to the fork, and opens a cross-repository PR whose head is fork-qualified."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-fork-new"
    _make_skill_source(project_dir, name, "Fork me.")
    api = forge_api(push_access=False)
    _manifest(project_dir, ns, name, INDEX_URL, forge="github", api_url=api.url)

    runner = grim_at(project_dir)
    upstream, fork = _index_and_fork_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    result = runner.run("publish", "--announce", format="json", check=False)
    assert result.returncode == 0, result.stderr
    # Fork-create disclosure: the stderr line names the fork and that it was
    # created in the publisher's own account (distinct from the reuse wording).
    assert "opened from fork forkuser/index, created in your account" in result.stderr, result.stderr
    data = json.loads(result.stdout)

    # The branch lands on the fork bare, never the upstream index.
    branch = _announce_branch(fork)
    assert "announce/" not in _heads(upstream), "upstream must not carry the branch"
    assert data["announce"]["outcome"] == "pull-request", data
    assert data["announce"]["fork"] == {"repo": "forkuser/index", "created": True}, data
    assert ("POST", "/repos/acme/index/forks") in api.requests, api.requests
    # The PR head is the fork owner, not a bare branch.
    pull_bodies = [b for p, b in api.bodies if p.endswith("/pulls")]
    assert pull_bodies and pull_bodies[0]["head"] == f"forkuser:{branch}", pull_bodies


def test_publish_announce_push_access_skips_fork(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """With push access to the upstream index, no fork is created — the
    branch goes straight to the index and `announce.fork` is null."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-nofork"
    _make_skill_source(project_dir, name, "No fork.")
    api = forge_api(push_access=True)
    _manifest(project_dir, ns, name, INDEX_URL, forge="github", api_url=api.url)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    data = runner.json("publish", "--announce")

    assert data["announce"]["fork"] is None, data
    assert not any(p.endswith("/forks") for _, p in api.requests), api.requests
    _announce_branch(bare)  # pushed to the upstream index as before


def test_publish_announce_reuses_existing_fork(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """An existing fork at the conventional path is reused (created=false) —
    no fork POST is issued."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-fork-reuse"
    _make_skill_source(project_dir, name, "Reuse fork.")
    api = forge_api(push_access=False, fork_exists=True)
    _manifest(project_dir, ns, name, INDEX_URL, forge="github", api_url=api.url)

    runner = grim_at(project_dir)
    upstream, fork = _index_and_fork_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    result = runner.run("publish", "--announce", format="json", check=False)
    assert result.returncode == 0, result.stderr
    # Fork-reuse disclosure: distinct wording from the create path — no
    # "created in your account".
    assert "opened from your existing fork forkuser/index" in result.stderr, result.stderr
    data = json.loads(result.stdout)

    _announce_branch(fork)
    assert data["announce"]["fork"] == {"repo": "forkuser/index", "created": False}, data
    assert not any(p.endswith("/forks") for _, p in api.requests), api.requests


def test_publish_announce_renamed_fork_uses_response_full_name(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """A renamed fork (upstream `index`, fork `grimoire-index`): the push URL
    and reported repo come from the fork response body, never a
    `{login}/{basename}` guess."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-fork-renamed"
    _make_skill_source(project_dir, name, "Renamed fork.")
    api = forge_api(
        push_access=False,
        fork_full_name=RENAMED_FORK_FULL_NAME,
        fork_clone_url=RENAMED_FORK_URL,
    )
    _manifest(project_dir, ns, name, INDEX_URL, forge="github", api_url=api.url)

    runner = grim_at(project_dir)
    upstream, fork = _index_and_fork_remote(tmp_path, runner, fork_url=RENAMED_FORK_URL)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    data = runner.json("publish", "--announce")

    branch = _announce_branch(fork)
    assert data["announce"]["fork"]["repo"] == RENAMED_FORK_FULL_NAME, data
    pull_bodies = [b for p, b in api.bodies if p.endswith("/pulls")]
    assert pull_bodies and pull_bodies[0]["head"] == f"forkuser:{branch}", pull_bodies


def test_publish_announce_gitlab_forks_cross_project_mr(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """GitLab: no push access forks the project and opens a cross-project MR
    posted from the fork's project with target_project_id set to the upstream
    (real GitLab has no source_project_id create attribute)."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-fork-gitlab"
    _make_skill_source(project_dir, name, "GitLab fork.")
    api = forge_api(push_access=False)
    _manifest(project_dir, ns, name, INDEX_URL, forge="gitlab", api_url=api.url)

    runner = grim_at(project_dir)
    upstream, fork = _index_and_fork_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    data = runner.json("publish", "--announce")

    _announce_branch(fork)
    assert data["announce"]["fork"] == {"repo": "forkuser/index", "created": True}, data
    assert ("POST", "/projects/acme%2Findex/fork") in api.requests, api.requests
    # The MR is created FROM the fork project, targeting the upstream.
    assert ("POST", f"/projects/{FORK_PROJECT_ID}/merge_requests") in api.requests, api.requests
    mr_bodies = [b for p, b in api.bodies if p.endswith("/merge_requests")]
    assert mr_bodies and mr_bodies[0].get("target_project_id") == UPSTREAM_PROJECT_ID, mr_bodies


def test_publish_announce_fork_disabled_pushes_upstream(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """`[announce] fork = false` forces the upstream push even without push
    access — no fork API call, `announce.fork` null (today's behavior)."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-fork-off"
    _make_skill_source(project_dir, name, "Fork off.")
    api = forge_api(push_access=False)
    _manifest(project_dir, ns, name, INDEX_URL, forge="github", api_url=api.url, fork=False)

    runner = grim_at(project_dir)
    upstream, fork = _index_and_fork_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    data = runner.json("publish", "--announce")

    _announce_branch(upstream)  # pushed straight to the index
    assert "announce/" not in _heads(fork), "fork must not be touched"
    assert data["announce"]["fork"] is None, data
    assert not any(p.endswith("/forks") for _, p in api.requests), api.requests


def test_publish_announce_fork_never_string_disables_fork(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """`[announce] fork = "never"` is the explicit spelling of the legacy
    `false`, proving the policy string round-trips through the real
    `[announce]` table (which denies unknown keys), not just the parser."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-fork-never"
    _make_skill_source(project_dir, name, "Never fork.")
    api = forge_api(push_access=False)
    _manifest(project_dir, ns, name, INDEX_URL, forge="github", api_url=api.url, fork="never")

    runner = grim_at(project_dir)
    upstream, fork = _index_and_fork_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    data = runner.json("publish", "--announce")

    _announce_branch(upstream)  # pushed straight to the index
    assert "announce/" not in _heads(fork), "fork must not be touched"
    assert data["announce"]["fork"] is None, data
    assert not any(p.endswith("/forks") for _, p in api.requests), api.requests


def test_publish_announce_fork_always_forks_despite_push_access(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """`[announce] fork = "always"` forks and opens a cross-repository PR even
    though the token can push to the index — the inverse of
    `test_publish_announce_push_access_skips_fork`, which shares this setup
    minus the policy."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-fork-always"
    _make_skill_source(project_dir, name, "Always fork.")
    api = forge_api(push_access=True)
    _manifest(project_dir, ns, name, INDEX_URL, forge="github", api_url=api.url, fork="always")

    runner = grim_at(project_dir)
    upstream, fork = _index_and_fork_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    data = runner.json("publish", "--announce")

    # The branch lands on the fork even though the upstream was writable.
    branch = _announce_branch(fork)
    assert "announce/" not in _heads(upstream), "upstream must not carry the branch"
    assert data["announce"]["outcome"] == "pull-request", data
    assert data["announce"]["fork"] == {"repo": "forkuser/index", "created": True}, data
    assert ("POST", "/repos/acme/index/forks") in api.requests, api.requests
    pull_bodies = [b for p, b in api.bodies if p.endswith("/pulls")]
    assert pull_bodies and pull_bodies[0]["head"] == f"forkuser:{branch}", pull_bodies


def test_publish_announce_gitlab_fork_always_forks_despite_push_access(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """GitLab counterpart to `test_publish_announce_fork_always_forks_despite_push_access`:
    `[announce] fork = "always"` forks and opens a cross-project MR even though
    the token can push to the upstream index. GitHub's and GitLab's permission
    parsing and fork plumbing are entirely separate code paths, so a bug that
    wires the force path for GitHub but not GitLab would still pass the whole
    suite without this. Shares its setup with
    `test_publish_announce_gitlab_forks_cross_project_mr` minus the policy
    (push access there is denied, not forced through)."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-fork-always-gitlab"
    _make_skill_source(project_dir, name, "Always fork GitLab.")
    api = forge_api(push_access=True)
    _manifest(project_dir, ns, name, INDEX_URL, forge="gitlab", api_url=api.url, fork="always")

    runner = grim_at(project_dir)
    upstream, fork = _index_and_fork_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    data = runner.json("publish", "--announce")

    # The branch lands on the fork even though the upstream was writable.
    _announce_branch(fork)
    assert "announce/" not in _heads(upstream), "upstream must not carry the branch"
    assert data["announce"]["fork"] == {"repo": "forkuser/index", "created": True}, data
    assert ("POST", "/projects/acme%2Findex/fork") in api.requests, api.requests
    # The MR is created FROM the fork project, targeting the upstream.
    mr_bodies = [b for p, b in api.bodies if p.endswith("/merge_requests")]
    assert mr_bodies and mr_bodies[0].get("target_project_id") == UPSTREAM_PROJECT_ID, mr_bodies


def test_publish_announce_gitlab_fork_always_self_owned_upstream_pushes_directly(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """The self-fork guard (forking your own repository is impossible) only
    had a raw unit test, not CLI-level coverage on either forge — exactly the
    scenario the ADR calls risky: a missing guard here turns a working push
    into exit 69 (GitLab 409s a self-fork, and the identity-based reuse hunt
    then finds nothing to adopt). Set the authenticated login to the upstream
    index's own namespace (`acme`) under `fork = "always"` and confirm the
    announce still degrades cleanly to the upstream push."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-fork-always-self-owned"
    _make_skill_source(project_dir, name, "Self-owned upstream.")
    api = forge_api(fork_owner="acme")
    _manifest(project_dir, ns, name, INDEX_URL, forge="gitlab", api_url=api.url, fork="always")

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    result = runner.run("publish", "--announce", format="json", check=False)
    assert result.returncode == 0, result.stderr
    data = json.loads(result.stdout)

    _announce_branch(bare)  # pushed straight to the index, no fork involved
    assert data["announce"]["fork"] is None, data
    assert not any(m == "POST" and p.endswith("/fork") for m, p in api.requests), api.requests


def test_publish_announce_fork_of_foreign_repo_rejected(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """Security guard: a fork whose parent does not match the upstream is
    rejected (exit 69) and nothing is pushed to the index."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-fork-foreign"
    _make_skill_source(project_dir, name, "Foreign fork.")
    api = forge_api(push_access=False, parent_full_name="stranger/index", forked_from_id=999)
    _manifest(project_dir, ns, name, INDEX_URL, forge="github", api_url=api.url)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    result = runner.run("publish", "--announce", format="json", check=False)

    assert result.returncode == 69, f"expected 69, got {result.returncode}: {result.stderr}"
    assert "announce failed" in result.stderr
    data = json.loads(result.stdout)
    # The publish itself succeeded; only the announce fork was rejected.
    assert data["announce"] is None, data
    assert data["items"][0]["status"] == "pushed", data
    assert "announce/" not in _heads(bare), "no branch may reach the index"


def test_publish_announce_github_hostile_fork_clone_url_rejected(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """Security guard: a hostile GitHub fork `clone_url` using git's `ext::`
    remote-helper transport must never reach `git push` — rejected (exit 69)
    before any push happens, so the embedded command never executes."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-fork-rce-gh"
    _make_skill_source(project_dir, name, "RCE guard GitHub.")
    sentinel = tmp_path / "pwned"
    api = forge_api(push_access=False, fork_clone_url=f'ext::sh -c "touch {sentinel}"')
    _manifest(project_dir, ns, name, INDEX_URL, forge="github", api_url=api.url)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    result = runner.run("publish", "--announce", format="json", check=False)

    assert result.returncode == 69, f"expected 69, got {result.returncode}: {result.stderr}"
    assert "announce failed" in result.stderr
    assert TOKEN not in result.stdout + result.stderr, "token must never be printed"
    data = json.loads(result.stdout)
    assert data["announce"] is None, data
    assert data["items"][0]["status"] == "pushed", data
    assert "announce/" not in _heads(bare), "no branch may reach the index"
    assert not sentinel.exists(), "hostile clone_url must never execute"


def test_publish_announce_gitlab_hostile_fork_url_rejected(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """Same guard, GitLab: a hostile `http_url_to_repo` using `ext::` is
    rejected before any push — no command execution."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-fork-rce-gl"
    _make_skill_source(project_dir, name, "RCE guard GitLab.")
    sentinel = tmp_path / "pwned"
    api = forge_api(push_access=False, fork_clone_url=f'ext::sh -c "touch {sentinel}"')
    _manifest(project_dir, ns, name, INDEX_URL, forge="gitlab", api_url=api.url)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    result = runner.run("publish", "--announce", format="json", check=False)

    assert result.returncode == 69, f"expected 69, got {result.returncode}: {result.stderr}"
    assert "announce failed" in result.stderr
    assert TOKEN not in result.stdout + result.stderr, "token must never be printed"
    data = json.loads(result.stdout)
    assert data["announce"] is None, data
    assert data["items"][0]["status"] == "pushed", data
    assert "announce/" not in _heads(bare), "no branch may reach the index"
    assert not sentinel.exists(), "hostile clone_url must never execute"


def test_publish_announce_fork_push_url_cross_repo_redirect_rejected(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """Security guard: a fork response can pass the scheme + parent/source
    checks yet still point `clone_url` at a different repository on the same
    trusted host (`git.example.test`, the index host — not a scheme
    violation). The identity-binding guard must reject that redirect before
    any push, distinct from a plain network failure (asserted via the
    specific guard message, not just the exit code)."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-fork-redirect"
    _make_skill_source(project_dir, name, "Cross-repo redirect guard.")
    api = forge_api(push_access=False, fork_clone_url=f"https://{INDEX_HOST}/victim/other-repo.git")
    _manifest(project_dir, ns, name, INDEX_URL, forge="github", api_url=api.url)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    result = runner.run("publish", "--announce", format="json", check=False)

    assert result.returncode == 69, f"expected 69, got {result.returncode}: {result.stderr}"
    assert "does not match the fork identity" in result.stderr, result.stderr
    assert TOKEN not in result.stdout + result.stderr, "token must never be printed"
    data = json.loads(result.stdout)
    assert data["announce"] is None, data
    assert data["items"][0]["status"] == "pushed", data
    assert "announce/" not in _heads(bare), "no branch may reach the index"


def test_publish_announce_fork_push_url_leading_dash_rejected(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """A fork push URL starting with `-` (could be parsed as a git flag,
    e.g. an SSH ProxyCommand injection) is rejected by the same https-only
    guard as the `ext::` case — exit 69, nothing pushed anywhere."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-fork-dash"
    _make_skill_source(project_dir, name, "Dash guard.")
    api = forge_api(push_access=False, fork_clone_url="-oProxyCommand=evil")
    _manifest(project_dir, ns, name, INDEX_URL, forge="gitlab", api_url=api.url)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    result = runner.run("publish", "--announce", format="json", check=False)

    assert result.returncode == 69, f"expected 69, got {result.returncode}: {result.stderr}"
    assert TOKEN not in result.stdout + result.stderr, "token must never be printed"
    data = json.loads(result.stdout)
    assert data["announce"] is None, data
    assert data["items"][0]["status"] == "pushed", data
    assert "announce/" not in _heads(bare), "no branch may reach the index"


def test_publish_announce_json_reports_fork_populated(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """The JSON announce section carries a populated fork object when the
    branch was pushed to a fork (the null case is covered by the
    branch-pushed test)."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-fork-json"
    _make_skill_source(project_dir, name, "Fork JSON.")
    api = forge_api(push_access=False)
    _manifest(project_dir, ns, name, INDEX_URL, forge="github", api_url=api.url)

    runner = grim_at(project_dir)
    _index_and_fork_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    data = runner.json("publish", "--announce")

    assert data["announce"]["fork"] == {"repo": "forkuser/index", "created": True}, data


# ── GitLab identity-based fork reuse via enumeration (Wave 2) ──────────────


def test_publish_announce_gitlab_reuses_fork_via_enumeration(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """Steady-state GitLab publisher case: a 409 on fork-create means the
    fork already exists, so grim enumerates `GET /projects/:id/forks` and
    adopts the entry whose `forked_from_project.id` and namespace match —
    the identity-based path that replaced the `{user}/{basename}` guess
    (ADR D6 / issue: announce-fork)."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-gl-reuse"
    _make_skill_source(project_dir, name, "GitLab reuse.")
    api = forge_api(push_access=False, fork_conflict=True)
    _manifest(project_dir, ns, name, INDEX_URL, forge="gitlab", api_url=api.url)

    runner = grim_at(project_dir)
    upstream, fork = _index_and_fork_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    result = runner.run("publish", "--announce", format="json", check=False)
    assert result.returncode == 0, result.stderr
    assert TOKEN not in result.stdout + result.stderr, "token must never be printed"
    # Fork-reuse disclosure, same wording as the GitHub reuse path.
    assert "opened from your existing fork forkuser/index" in result.stderr, result.stderr

    data = json.loads(result.stdout)
    assert data["announce"]["fork"] == {"repo": "forkuser/index", "created": False}, data
    _announce_branch(fork)
    assert "announce/" not in _heads(upstream), "upstream must not carry the branch"
    assert any(
        m == "GET" and p.split("?", 1)[0] == f"/projects/{UPSTREAM_PROJECT_ID}/forks"
        for m, p in api.requests
    ), api.requests


def test_publish_announce_gitlab_reuses_renamed_fork_via_enumeration(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """A renamed fork (upstream `index`, fork `grimoire-index`) is still
    found by enumeration and reused — proving the rename tolerance the old
    `{user}/{basename}` guess lacked."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-gl-renamed"
    _make_skill_source(project_dir, name, "GitLab renamed reuse.")
    api = forge_api(
        push_access=False,
        fork_conflict=True,
        fork_full_name=RENAMED_FORK_FULL_NAME,
        fork_clone_url=RENAMED_FORK_URL,
    )
    _manifest(project_dir, ns, name, INDEX_URL, forge="gitlab", api_url=api.url)

    runner = grim_at(project_dir)
    upstream, fork = _index_and_fork_remote(tmp_path, runner, fork_url=RENAMED_FORK_URL)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    data = runner.json("publish", "--announce")

    assert data["announce"]["fork"] == {"repo": RENAMED_FORK_FULL_NAME, "created": False}, data
    _announce_branch(fork)
    assert "announce/" not in _heads(upstream), "upstream must not carry the branch"


def test_publish_announce_gitlab_reuse_rejects_wrong_source_fork(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """The enumerated fork in the user's namespace was forked from a
    different upstream project — the source-lineage guard rejects it even
    though the namespace matches (exit 69, nothing pushed anywhere)."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-gl-wrongsource"
    _make_skill_source(project_dir, name, "GitLab wrong source.")
    api = forge_api(push_access=False, fork_conflict=True, forked_from_id=999)
    _manifest(project_dir, ns, name, INDEX_URL, forge="gitlab", api_url=api.url)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    result = runner.run("publish", "--announce", format="json", check=False)

    assert result.returncode == 69, f"expected 69, got {result.returncode}: {result.stderr}"
    assert "announce failed" in result.stderr
    assert TOKEN not in result.stdout + result.stderr, "token must never be printed"
    data = json.loads(result.stdout)
    # The publish itself succeeded; only the announce fork reuse was rejected.
    assert data["announce"] is None, data
    assert data["items"][0]["status"] == "pushed", data
    assert "announce/" not in _heads(bare), "no branch may reach the index"


def test_publish_announce_gitlab_fork_import_failed_fast_fails(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """A freshly-created fork whose import fails fast-fails the readiness
    poll instead of running to the wall-clock deadline. The classification
    itself is unit-covered (`gitlab_import_readiness_classifies_status`);
    this proves the failure reaches the CLI exit path end to end."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-gl-importfail"
    _make_skill_source(project_dir, name, "GitLab import failed.")
    api = forge_api(push_access=False, import_status="failed", import_error="disk full")
    _manifest(project_dir, ns, name, INDEX_URL, forge="gitlab", api_url=api.url)

    runner = grim_at(project_dir)
    bare = _index_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    result = runner.run("publish", "--announce", format="json", check=False)

    assert result.returncode == 69, f"expected 69, got {result.returncode}: {result.stderr}"
    assert "disk full" in result.stderr, result.stderr
    assert TOKEN not in result.stdout + result.stderr, "token must never be printed"
    data = json.loads(result.stdout)
    assert data["announce"] is None, data
    assert "announce/" not in _heads(bare), "no branch may reach the index"


def test_publish_announce_gitlab_fork_import_pending_then_ready(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """The readiness poll retries through `import_status="started"` twice
    before succeeding once GitLab reports `"finished"` — the Pending→Ready
    path of the poll loop, previously exercised only via the status
    classifier unit test and the immediate-`"failed"` fast-fail acceptance
    test above. Costs ~6s of real wall-clock backoff (2s + 4s): `PollBounds`
    are an internal Rust default with no CLI/env override, so this is real
    time, not simulated."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-gl-pending"
    _make_skill_source(project_dir, name, "GitLab pending poll.")
    api = forge_api(push_access=False, import_status=["started", "started", "finished"])
    _manifest(project_dir, ns, name, INDEX_URL, forge="gitlab", api_url=api.url)

    runner = grim_at(project_dir)
    upstream, fork = _index_and_fork_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    result = runner.run("publish", "--announce", format="json", check=False)
    assert result.returncode == 0, result.stderr
    assert TOKEN not in result.stdout + result.stderr, "token must never be printed"

    data = json.loads(result.stdout)
    assert data["announce"]["outcome"] == "pull-request", data
    assert data["announce"]["fork"] == {"repo": "forkuser/index", "created": True}, data
    _announce_branch(fork)
    assert "announce/" not in _heads(upstream), "upstream must not carry the branch"


def test_publish_announce_github_fork_branch_pushed_when_pr_creation_fails(
    grim_at, project_dir: Path, registry: str, tmp_path: Path, forge_api
) -> None:
    """The fork resolves and the branch pushes to it, but the PR-creation API
    call itself errors (a 500, distinct from the 422-reuse path) — the
    outcome degrades to BranchPushed while still carrying the fork, and the
    stderr disclosure uses the fork-aware branch-pushed wording (distinct
    from both the pull-request fork wording and the no-fork branch-pushed
    wording)."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    name = "ann-fork-branchpushed"
    _make_skill_source(project_dir, name, "Fork branch pushed.")
    api = forge_api(push_access=False, pr_fails=True)
    _manifest(project_dir, ns, name, INDEX_URL, forge="github", api_url=api.url)

    runner = grim_at(project_dir)
    upstream, fork = _index_and_fork_remote(tmp_path, runner)
    runner.env["GRIM_ANNOUNCE_TOKEN"] = TOKEN
    result = runner.run("publish", "--announce", format="json", check=False)
    assert result.returncode == 0, result.stderr

    branch = _announce_branch(fork)
    assert "announce/" not in _heads(upstream), "upstream must not carry the branch"
    assert (
        f"announced: pushed branch '{branch}' to your fork forkuser/index — "
        "open the pull/merge request from the fork's branch banner"
    ) in result.stderr, result.stderr

    data = json.loads(result.stdout)
    assert data["announce"]["outcome"] == "branch-pushed", data
    assert data["announce"]["fork"] == {"repo": "forkuser/index", "created": True}, data
