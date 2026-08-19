# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim rate` acceptance tests — the forge-backed vote write path.

Every case here is reachable **without a forge**: resolution, the
confirmation gate, the credential gate and the host gate all decide before
a single request is made. That is not a limitation of the harness, it is
the contract — plan C-022 exists so a client can learn where its credential
would go *before* it authenticates, and C-006's refusals all fire ahead of
the `Authorization` header.

The one thing no test here may do is reach a real forge. Two mechanisms
keep that true. Every gate test points `GRIM_RATING_HOST` at an unroutable
`.invalid` host, so a bug that skipped a gate fails loudly instead of
voting. The end-to-end vote tests point it at `_FakeForge`, a GraphQL
server bound to `127.0.0.1` on an ephemeral port: loopback is not a real
forge and cannot become one — the socket never leaves the machine, the
address is not resolvable from anywhere else, and `graphql_endpoint`
speaks plain HTTP to exactly the three loopback host forms and `https://`
to every other host, lookalikes included (D-1). That is what lets the
`--up` happy path run end to end without a test-only seam in production
code.
"""
from __future__ import annotations

import json
import subprocess
import threading
from functools import partial
from http.server import (
    BaseHTTPRequestHandler,
    SimpleHTTPRequestHandler,
    ThreadingHTTPServer,
)
from pathlib import Path

import pytest


# ---------------------------------------------------------------------------
# Helpers / fixtures
# ---------------------------------------------------------------------------

RATED = "ghcr.io/acme/skills/rated"
UNRATED = "ghcr.io/acme/skills/unrated"
# The vote subject the sidecar binds to RATED. A mutation that carries any
# other node id voted on something the resolution step did not resolve.
TARGET = "D_kwDOAbCdEf"

# Unroutable by RFC 6761: a request that escaped a gate cannot reach a real
# forge, and the test fails on the wrong exit code rather than casting a vote.
GHES_HOST = "ghes.corp.invalid"


@pytest.fixture()
def http_index(tmp_path: Path):
    """A local static webserver serving an index dist dir."""
    root = tmp_path / "index-dist"
    root.mkdir()
    handler = partial(SimpleHTTPRequestHandler, directory=str(root))
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    yield root, f"http://127.0.0.1:{server.server_address[1]}"
    server.shutdown()


def _package(name: str, ref: str) -> dict:
    return {
        "schema": 1,
        "name": name,
        "kind": "skill",
        "ref": ref,
        "description": f"{name} skill",
        "repository": "https://github.com/acme/skills",
        "owner": {"github": "acme", "id": 1},
    }


def _publish(root: Path, provider: str | None = "github", rated: bool = True) -> None:
    """Serve an index carrying one rated and one unrated pointer."""
    (root / "all.json").write_text(
        json.dumps([_package("rated", RATED), _package("unrated", UNRATED)])
    )
    stats: dict = {
        "schema_version": 1,
        "generated_at": "2026-08-18T00:00:00Z",
        "providers": {},
        "entries": {},
    }
    if provider is not None:
        stats["providers"]["rating"] = provider
    if rated:
        stats["entries"][RATED] = {
            "rating": {
                "up": 12,
                "target": TARGET,
                "url": "https://github.com/acme/index/discussions/7",
            }
        }
    (root / "stats.json").write_text(json.dumps(stats))


def _index_config(project_dir: Path, locator: str) -> None:
    (project_dir / "grimoire.toml").write_text(
        f'[[registries]]\nalias = "hub"\nindex = "{locator}"\ndefault = true\n\n[skills]\n\n[rules]\n'
    )


def _warm_catalog(runner) -> None:
    """Build the catalog cache once so `rate` resolves rows deterministically."""
    result = runner.run("--format", "json", "search", "--refresh", check=False)
    assert result.returncode == 0, f"catalog build failed: {result.stderr}"


def _run_with_stdin(runner, *args: str, stdin: str, env_extra: dict | None = None):
    """`grim` with a piped stdin — `GrimRunner.run` does not take input.

    Piping is the whole point of `--token-stdin`, and it is also what makes
    stdin a non-terminal, which is the non-interactive half of the
    confirmation contract.
    """
    env = dict(runner.env)
    env.update(env_extra or {})
    return subprocess.run(
        [str(runner.binary), *args],
        input=stdin,
        capture_output=True,
        text=True,
        env=env,
        cwd=str(runner.cwd) if runner.cwd else None,
    )


def _rate(runner, *args: str, env_extra: dict | None = None):
    """`grim rate` with an isolated env. stdin is not a terminal under
    pytest, so every run here is non-interactive by construction."""
    env = dict(runner.env)
    env.update(env_extra or {})
    return subprocess.run(
        [str(runner.binary), *args],
        capture_output=True,
        text=True,
        env=env,
        cwd=str(runner.cwd) if runner.cwd else None,
        stdin=subprocess.DEVNULL,
    )


class _FakeForge:
    """A GraphQL forge bound to `127.0.0.1` on an ephemeral port.

    Answers the three documents `grim rate` sends — the viewer-identity
    read, `addUpvote` and `removeUpvote` — and records the **whole request
    body** of each, as the sibling fake in `test_publish_announce.py` does.
    The body, not just the document: the vote subject travels in
    `variables`, so a fake that reads only `query` cannot tell a vote on the
    resolved artifact from a vote on some other node.

    `contrary = True` makes it answer the **opposite** of what was asked —
    `viewerHasUpvoted: false` to an `addUpvote`, `true` to a `removeUpvote`.
    No real forge does that; it is how a test tells "grim reported what the
    forge settled on" apart from "grim assumed the action worked", which is
    invariant R-3 and is otherwise unobservable.

    Loopback is the whole reason this is reachable at all: `graphql_endpoint`
    speaks plain HTTP to `localhost`, `127.0.0.1` and `::1` (bare or on any
    port) and `https://` to every other host, so no lookalike host can be
    substituted for this one (D-1).
    """

    # The account the credential belongs to. The id is what
    # `state/votes.json` keys on — never the login, which is renameable.
    ACCOUNT_ID = 4242
    LOGIN = "voter"
    # The counts the forge reports back, both deliberately different from
    # the sidecar's 12 so a report echoing the cached value fails.
    UP_COUNT = 13
    REMOVE_COUNT = 11

    def __init__(self, contrary: bool = False) -> None:
        self.contrary = contrary
        self.bodies: list[dict] = []
        self.auth_headers: list[str] = []
        self.paths: list[str] = []
        forge = self

        class Handler(BaseHTTPRequestHandler):
            def _reply(self, code: int, body: object) -> None:
                payload = json.dumps(body).encode()
                self.send_response(code)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            def do_POST(self) -> None:  # noqa: N802 (http.server API)
                raw = self.rfile.read(int(self.headers.get("Content-Length") or 0))
                body = json.loads(raw) if raw else {}
                document = body.get("query", "")
                forge.paths.append(self.path)
                forge.bodies.append(body)
                forge.auth_headers.append(self.headers.get("Authorization", ""))
                if "removeUpvote" in document:
                    self._reply(
                        200,
                        {
                            "data": {
                                "removeUpvote": {
                                    "subject": {
                                        "upvoteCount": _FakeForge.REMOVE_COUNT,
                                        "viewerHasUpvoted": forge.contrary,
                                    }
                                }
                            }
                        },
                    )
                elif "addUpvote" in document:
                    self._reply(
                        200,
                        {
                            "data": {
                                "addUpvote": {
                                    "subject": {
                                        "upvoteCount": _FakeForge.UP_COUNT,
                                        "viewerHasUpvoted": not forge.contrary,
                                    }
                                }
                            }
                        },
                    )
                elif "viewer" in document:
                    self._reply(
                        200,
                        {
                            "data": {
                                "viewer": {
                                    "login": _FakeForge.LOGIN,
                                    "databaseId": _FakeForge.ACCOUNT_ID,
                                }
                            }
                        },
                    )
                else:
                    self._reply(404, {})

            def log_message(self, *args: object) -> None:
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.host = f"127.0.0.1:{self.server.server_port}"

    @property
    def documents(self) -> list[str]:
        return [body.get("query", "") for body in self.bodies]

    def mutation(self) -> dict:
        """The one vote mutation this forge was asked for. Raises when none
        was issued, which is itself the assertion."""
        return next(body for body in self.bodies if "Upvote" in body.get("query", ""))

    def close(self) -> None:
        self.server.shutdown()


@pytest.fixture()
def fake_forge():
    forge = _FakeForge()
    yield forge
    forge.close()


@pytest.fixture()
def rated_project(grim_at, project_dir: Path, http_index):
    """A project browsing an index that publishes a github-provider rating."""
    root, base = http_index
    _publish(root)
    _index_config(project_dir, base)
    runner = grim_at(project_dir)
    _warm_catalog(runner)
    return runner


# ---------------------------------------------------------------------------
# The vote itself — argv → confirm → resolve → mutate → report (F-2)
# ---------------------------------------------------------------------------


def _votes_key(reference: str) -> str:
    """The `state/votes.json` key for a github vote by the fake forge's
    account: `<provider>:<account-id>`, a NUL, then the ref."""
    return f"github:{_FakeForge.ACCOUNT_ID}\x00{reference}"


def _stored_vote(runner, reference: str) -> dict:
    stored = json.loads((Path(runner.grim_home) / "state" / "votes.json").read_text())
    return stored["votes"][_votes_key(reference)]


def _vote(runner, forge, *args: str, stdin: str):
    """A real vote against the loopback forge, credential piped."""
    return _run_with_stdin(
        runner,
        "--format",
        "json",
        "rate",
        RATED,
        *args,
        "--yes",
        "--token-stdin",
        "--token-host",
        forge.host,
        stdin=stdin,
        env_extra={"GRIM_RATING_HOST": forge.host},
    )


def test_an_up_vote_succeeds_and_reports_the_forge_state(rated_project, fake_forge) -> None:
    """The whole write path in one run: argv, the confirmation gate, row
    resolution, the mutation, and the report. Every other test in this file
    stops at a gate, so without this one nothing proves the flag works."""
    secret = "ghp_thistokenreachesexactlyoneheader"
    result = _vote(rated_project, fake_forge, "--up", stdin=secret)

    assert result.returncode == 0, f"the vote must land: {result.stderr}"
    report = json.loads(result.stdout)
    assert report["ref"] == RATED
    assert report["action"] == "up"
    assert report["provider"] == "github"
    assert report["host"] == fake_forge.host
    assert report["up"] == _FakeForge.UP_COUNT, (
        "the forge's post-mutation count, never the sidecar's cached 12"
    )
    assert report["viewer_up"] is True, "the forge just said so, so the report says so"

    assert fake_forge.paths[0] == "/api/graphql", "the GHES/GitLab endpoint layout"
    mutation = fake_forge.mutation()
    assert "addUpvote" in mutation["query"], mutation["query"]
    assert mutation["variables"] == {"id": TARGET}, (
        "the vote lands on the node the sidecar bound to this ref — the resolve "
        "half of resolve→mutate, which a document-only assertion cannot see"
    )
    assert fake_forge.auth_headers[0] == f"Bearer {secret}", (
        "the piped credential reaches the Authorization header of the host it was declared for"
    )
    assert secret not in result.stdout + result.stderr, "and is never echoed"


def test_a_remove_retracts_and_reports_not_voted(rated_project, fake_forge) -> None:
    """S-009's other arm: `--remove` retracts this account's own upvote, and
    the report carries what the forge settled on, not what was asked for."""
    result = _vote(rated_project, fake_forge, "--remove", stdin="ghp_x")

    assert result.returncode == 0, f"the retraction must land: {result.stderr}"
    report = json.loads(result.stdout)
    assert report["action"] == "remove"
    assert report["up"] == _FakeForge.REMOVE_COUNT
    assert report["viewer_up"] is False
    mutation = fake_forge.mutation()
    assert "removeUpvote" in mutation["query"], mutation["query"]
    assert mutation["variables"] == {"id": TARGET}, "on the resolved node, not another"
    assert not any("addUpvote" in d for d in fake_forge.documents), (
        "a retraction must never issue an upvote"
    )
    assert _stored_vote(rated_project, RATED)["voted"] is False, (
        "the store's not-voted arm: R-3 keeps `false` distinct from unknown, so a "
        "retraction that recorded nothing would read back as `null` on the next dry run"
    )


def test_a_vote_records_the_account_id_in_votes_json(rated_project, fake_forge) -> None:
    """C-008: the tri-state store is written from the mutation's own report.
    The key is the forge's immutable account id — a login is renameable, and
    the next holder must not inherit this account's vote display."""
    votes_file = Path(rated_project.grim_home) / "state" / "votes.json"
    assert not votes_file.exists(), "nothing is recorded before a vote"

    result = _vote(rated_project, fake_forge, "--up", stdin="ghp_x")
    assert result.returncode == 0, result.stderr

    stored = json.loads(votes_file.read_text())
    assert stored["votes"][_votes_key(RATED)]["voted"] is True, stored
    assert _FakeForge.LOGIN not in json.dumps(stored), (
        "keyed on the immutable id, never on the renameable login"
    )


def test_the_report_and_the_store_follow_the_forge_not_the_request(
    rated_project, fake_forge
) -> None:
    """R-3: a vote state is what the forge settled on, never what was asked
    for. This forge contradicts the request, so every assertion below fails
    if the state is inferred from `--up` instead of read from the payload —
    and with a forge that always agrees, the two are indistinguishable."""
    fake_forge.contrary = True

    result = _vote(rated_project, fake_forge, "--up", stdin="ghp_x")
    assert result.returncode == 0, f"a disagreeing forge is not an error: {result.stderr}"

    report = json.loads(result.stdout)
    assert report["action"] == "up", "the request is still reported as made"
    assert report["viewer_up"] is False, "but the state is the forge's answer, not the request"
    assert _stored_vote(rated_project, RATED)["voted"] is False, (
        "and the cache stores what the forge said, so the next reader is not told "
        "something the forge never said"
    )


# ---------------------------------------------------------------------------
# C-022 — `--dry-run` resolves without a credential or a request (S-020)
# ---------------------------------------------------------------------------


def test_dry_run_reports_the_resolved_host_without_a_credential(rated_project) -> None:
    """S-020: this is how a client learns which host a ref votes against —
    *before* it picks an auth provider, which a field on the post-vote
    report would be far too late to do."""
    result = _rate(rated_project, "--format", "json", "rate", RATED, "--dry-run")
    assert result.returncode == 0, f"a dry run needs no credential: {result.stderr}"
    report = json.loads(result.stdout)
    assert report["ref"] == RATED
    assert report["action"] == "up"
    assert report["provider"] == "github"
    assert report["host"] == "api.github.com"
    assert report["up"] == 12, "the sidecar's current count, not a mutated one"
    assert report["url"] == "https://github.com/acme/index/discussions/7"


def test_dry_run_works_offline(rated_project) -> None:
    """S-020: a dry run makes no forge request at all, so `--offline` — which
    hard-refuses a real vote at 81 — must not block it."""
    result = _rate(rated_project, "--format", "json", "rate", RATED, "--dry-run", "--offline")
    assert result.returncode == 0, f"a dry run must work offline: {result.stderr}"
    report = json.loads(result.stdout)
    assert report["host"] == "api.github.com"
    assert report["viewer_up"] is None, "nothing was asked, so nothing is known"


def test_bare_dry_run_reads_no_credential_and_leaves_viewer_up_null(rated_project) -> None:
    """C-023: the bare `--dry-run` contract is unchanged. No credential is
    read — a sentinel in the ladder must not be consumed, and the run must
    not fail at 80 the way a credential-needing path would — and no request
    is made, so `viewer_up` is `null`."""
    sentinel = "ghp_thisladdertokenmustnotbeconsumed"
    result = _rate(
        rated_project,
        "--format",
        "json",
        "rate",
        RATED,
        "--dry-run",
        env_extra={"GRIM_RATE_TOKEN": sentinel, "GRIM_RATING_HOST": GHES_HOST},
    )
    assert result.returncode == 0, f"no credential is needed: {result.stderr}"
    report = json.loads(result.stdout)
    assert report["host"] == GHES_HOST, "resolution still happens"
    assert report["viewer_up"] is None, "no query was issued, so the state is unknown"
    assert sentinel not in result.stdout + result.stderr


# ---------------------------------------------------------------------------
# C-023 — the viewer-state read (S-022 / S-023)
# ---------------------------------------------------------------------------


def test_a_credentialed_dry_run_needs_no_yes_and_never_reports_not_voted(
    rated_project,
) -> None:
    """S-022 / S-023: `--dry-run --token-stdin` posts nothing, so it does not
    require `--yes` — a `64` here would mean the C-005 confirmation rule
    leaked onto a path with nothing to confirm.

    The host is unroutable, so the read itself fails; C-023 says that leaves
    `viewer_up` **null**, never `false`, and the resolution's own exit stays
    `0`. Reporting "you have not voted" because a query failed is the precise
    lie invariant R-3 exists to prevent."""
    secret = "ghp_thistokenisreadbutneverposted"
    result = _run_with_stdin(
        rated_project,
        "--format",
        "json",
        "rate",
        RATED,
        "--dry-run",
        "--token-stdin",
        stdin=secret,
        env_extra={"GRIM_RATING_HOST": GHES_HOST},
    )
    assert result.returncode == 0, f"a failed read still resolves: {result.stderr}"
    report = json.loads(result.stdout)
    assert report["viewer_up"] is None, "a failed query is unknown, never not-voted"
    assert report["viewer_up"] is not False, "never renders as not-voted"
    assert report["host"] == GHES_HOST
    assert secret not in result.stdout + result.stderr, "the credential must never be echoed"


def test_a_credentialed_dry_run_still_fails_closed_on_a_token_host_mismatch(
    rated_project,
) -> None:
    """C-023: `--token-host` applies to the read exactly as to the vote — 80
    naming both hosts, before the token reaches a header."""
    secret = "ghp_thistokenmustnevertravel"
    result = _run_with_stdin(
        rated_project,
        "rate",
        RATED,
        "--dry-run",
        "--token-stdin",
        "--token-host",
        "api.github.com",
        stdin=secret,
        env_extra={"GRIM_RATING_HOST": GHES_HOST},
    )
    assert result.returncode == 80, f"expected an auth refusal: {result.stderr}"
    assert "api.github.com" in result.stderr, "names the declared host"
    assert GHES_HOST in result.stderr, "names the resolved host"
    assert secret not in result.stdout + result.stderr


def test_a_credentialed_dry_run_refuses_offline(rated_project) -> None:
    """C-023: this variant wants a forge request, so `--offline` refuses it
    at 81 rather than answering a question it could not ask. The bare dry
    run's offline contract (S-020) is untouched."""
    result = _run_with_stdin(
        rated_project,
        "rate",
        RATED,
        "--dry-run",
        "--token-stdin",
        "--offline",
        stdin="ghp_x",
        env_extra={"GRIM_RATING_HOST": GHES_HOST},
    )
    assert result.returncode == 81, result.stderr


def test_a_credentialed_dry_run_still_refuses_an_empty_pipe(rated_project) -> None:
    """C-006 is unchanged on this path: a caller that stated it was supplying
    a credential must not silently read as somebody else."""
    result = _run_with_stdin(
        rated_project,
        "rate",
        RATED,
        "--dry-run",
        "--token-stdin",
        stdin="\n",
        env_extra={
            "GRIM_RATING_HOST": GHES_HOST,
            "GRIM_RATE_TOKEN": "ghp_a_different_credential",
        },
    )
    assert result.returncode == 80, result.stderr
    assert "ghp_a_different_credential" not in result.stdout + result.stderr


def test_a_credentialed_dry_run_on_an_unrated_provider_asks_nothing(
    grim_at, project_dir: Path, http_index
) -> None:
    """No host resolves, so there is nowhere to send the credential and no
    query to issue — `viewer_up` is `null` and the run still exits 0."""
    root, base = http_index
    _publish(root, provider="bitbucket")
    _index_config(project_dir, base)
    runner = grim_at(project_dir)
    _warm_catalog(runner)

    secret = "ghp_thistokenhasnowheretogo"
    result = _run_with_stdin(
        runner, "--format", "json", "rate", RATED, "--dry-run", "--token-stdin", stdin=secret
    )
    assert result.returncode == 0, result.stderr
    report = json.loads(result.stdout)
    assert report["host"] is None
    assert report["viewer_up"] is None
    assert secret not in result.stdout + result.stderr


def test_dry_run_honors_the_user_host_override(rated_project) -> None:
    """C-007: a GHES/self-managed host comes from the user's own environment.
    Nothing in `stats.json` can reach this — the sidecar carries a provider,
    a target and a url, and deliberately no host."""
    result = _rate(
        rated_project,
        "--format",
        "json",
        "rate",
        RATED,
        "--dry-run",
        env_extra={"GRIM_RATING_HOST": GHES_HOST},
    )
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)["host"] == GHES_HOST


def test_dry_run_reports_a_null_host_for_an_unrecognised_provider(
    grim_at, project_dir: Path, http_index
) -> None:
    """C-022: `host` is always present and `null` when nothing resolves —
    the "grim cannot vote here" answer, which a client needs as data rather
    than as an error."""
    root, base = http_index
    _publish(root, provider="bitbucket")
    _index_config(project_dir, base)
    runner = grim_at(project_dir)
    _warm_catalog(runner)

    result = _rate(runner, "--format", "json", "rate", RATED, "--dry-run")
    assert result.returncode == 0, result.stderr
    report = json.loads(result.stdout)
    assert "host" in report, "the key is always present"
    assert report["host"] is None
    assert report["provider"] == "bitbucket", "the raw value the index published"


def test_an_unrecognised_provider_refuses_a_real_vote(
    grim_at, project_dir: Path, http_index
) -> None:
    """C-005: exit 65, with the raw provider value in the message."""
    root, base = http_index
    _publish(root, provider="bitbucket")
    _index_config(project_dir, base)
    runner = grim_at(project_dir)
    _warm_catalog(runner)

    result = _rate(runner, "rate", RATED, "--yes")
    assert result.returncode == 65, result.stderr
    assert "bitbucket" in result.stderr, result.stderr


# ---------------------------------------------------------------------------
# C-022 — `--token-host` fails closed before the header (S-021)
# ---------------------------------------------------------------------------


def test_token_host_mismatch_refuses_before_the_token_reaches_a_header(rated_project) -> None:
    """S-021: an extension that pipes a github.com token for a GHES ref is
    refused at 80 naming both hosts. This is the guarantee that does not
    depend on the client being correct."""
    secret = "ghp_thistokenmustnevertravel"
    result = _run_with_stdin(
        rated_project,
        "rate",
        RATED,
        "--yes",
        "--token-stdin",
        "--token-host",
        "api.github.com",
        stdin=secret,
        env_extra={"GRIM_RATING_HOST": GHES_HOST},
    )
    assert result.returncode == 80, f"expected an auth refusal: {result.stderr}"
    assert "api.github.com" in result.stderr, "names the declared host"
    assert GHES_HOST in result.stderr, "names the resolved host"
    assert secret not in result.stdout + result.stderr, "the credential must never be echoed"


def test_token_host_never_matches_a_lookalike(rated_project) -> None:
    """C-007: host comparison is exact — no prefix, suffix or subdomain
    matching, so `evil-github.com` and `api.github.com.evil.tld` are simply
    different hosts."""
    for hostile in ("evil-github.com", "api.github.com.evil.tld", "github.com"):
        result = _run_with_stdin(
            rated_project,
            "rate",
            RATED,
            "--yes",
            "--token-stdin",
            "--token-host",
            hostile,
            stdin="ghp_x",
        )
        assert result.returncode == 80, f"{hostile} must not match api.github.com: {result.stderr}"


def test_a_matching_token_host_passes_the_gate(rated_project) -> None:
    """A correct declaration proceeds past the host gate — it then fails at
    the unroutable forge (69), which is proof the gate let it through."""
    result = _run_with_stdin(
        rated_project,
        "rate",
        RATED,
        "--yes",
        "--token-stdin",
        "--token-host",
        GHES_HOST,
        stdin="ghp_x",
        env_extra={"GRIM_RATING_HOST": GHES_HOST},
    )
    assert result.returncode == 69, f"expected to get as far as the unreachable forge: {result.stderr}"


def test_token_host_without_token_stdin_is_a_usage_error(rated_project) -> None:
    result = _rate(rated_project, "rate", RATED, "--yes", "--token-host", "api.github.com")
    assert result.returncode == 64, result.stderr
    assert "--token-stdin" in result.stderr


# ---------------------------------------------------------------------------
# Confirmation — S-017 / S-018 / S-019
# ---------------------------------------------------------------------------


def test_a_non_interactive_run_without_yes_refuses_naming_the_flag(rated_project) -> None:
    """S-018: never a hang, never an unconfirmed vote."""
    result = _rate(rated_project, "rate", RATED)
    assert result.returncode == 64, result.stderr
    assert "--yes" in result.stderr, result.stderr


def test_token_stdin_without_yes_is_a_usage_error(rated_project) -> None:
    """S-019: stdin carries the credential, so it cannot also carry a `y`,
    and the prompt is never rerouted to /dev/tty."""
    result = _run_with_stdin(rated_project, "rate", RATED, "--token-stdin", stdin="ghp_x")
    assert result.returncode == 64, result.stderr
    assert "--yes" in result.stderr, result.stderr


def test_up_and_remove_together_is_a_usage_error(rated_project) -> None:
    """S-009: `--remove` retracts your own upvote; it is not a downvote, and
    the two flags are not composable."""
    result = _rate(rated_project, "rate", RATED, "--up", "--remove", "--yes")
    assert result.returncode == 64, result.stderr


# ---------------------------------------------------------------------------
# C-006 — the piped credential
# ---------------------------------------------------------------------------


def test_empty_piped_input_is_an_auth_error_and_never_falls_through(rated_project) -> None:
    """C-006: the caller *stated* it was supplying a credential, so silently
    voting as somebody else is worse than failing."""
    for empty in ("", "\n", "   \n"):
        result = _run_with_stdin(
            rated_project,
            "rate",
            RATED,
            "--yes",
            "--token-stdin",
            stdin=empty,
            env_extra={"GRIM_RATE_TOKEN": "ghp_a_different_credential"},
        )
        assert result.returncode == 80, f"input {empty!r}: {result.stderr}"
        assert "ghp_a_different_credential" not in result.stdout + result.stderr, (
            "an empty pipe must not fall through to the standalone ladder"
        )


def test_multi_line_piped_input_is_a_usage_error(rated_project) -> None:
    secret = "ghp_firstline"
    result = _run_with_stdin(
        rated_project, "rate", RATED, "--yes", "--token-stdin", stdin=f"{secret}\nextra\n"
    )
    assert result.returncode == 64, result.stderr
    assert secret not in result.stdout + result.stderr, "the credential must never be echoed"


def test_there_is_no_token_value_flag(rated_project) -> None:
    """CWE-214: argv lands in world-readable /proc/<pid>/cmdline, so the flag
    must not exist at all — refused by construction, not by validation."""
    result = _rate(rated_project, "rate", RATED, "--yes", "--token", "ghp_x")
    assert result.returncode == 64, result.stderr


def test_no_credential_refuses_naming_the_ladder_never_a_token(rated_project) -> None:
    """S-004: exit 80 with a message that names the ladder. The host is a
    `.invalid` one, so no forge CLI on the developer's machine can satisfy
    it either."""
    result = _rate(
        rated_project,
        "rate",
        RATED,
        "--yes",
        env_extra={"GRIM_RATING_HOST": GHES_HOST},
    )
    assert result.returncode == 80, result.stderr
    assert "GRIM_RATE_TOKEN" in result.stderr, f"names the ladder: {result.stderr}"
    assert "gh auth login" in result.stderr, f"names the ladder: {result.stderr}"


# ---------------------------------------------------------------------------
# Resolution and policy — S-005 / S-006
# ---------------------------------------------------------------------------


def test_a_ref_with_no_rating_is_a_data_error(rated_project) -> None:
    """S-005: the row exists, the index publishes no vote target for it."""
    result = _rate(rated_project, "rate", UNRATED, "--yes")
    assert result.returncode == 65, result.stderr


def test_a_ref_in_no_catalog_is_not_found(rated_project) -> None:
    """S-005 edge: not in the catalog at all is 79, a different condition
    from a row that carries no rating."""
    result = _rate(rated_project, "rate", "ghcr.io/acme/skills/ghost", "--yes")
    assert result.returncode == 79, result.stderr


def test_a_malformed_ref_is_a_usage_error(rated_project) -> None:
    """C-005: a reference that does not parse is 64, not the identifier
    layer's usual 65 — it is a bad invocation, not bad data."""
    result = _rate(rated_project, "rate", "ghcr.io/acme/../escape", "--yes")
    assert result.returncode == 64, result.stderr


def test_offline_hard_refuses_a_vote(rated_project) -> None:
    """S-006: a vote writes to a forge and has no cached path, so offline is
    a deliberate refusal (81) rather than a silent degrade."""
    result = _rate(rated_project, "rate", RATED, "--yes", "--offline")
    assert result.returncode == 81, result.stderr


# ---------------------------------------------------------------------------
# Report shape (WP-N and WP-X both build against this)
# ---------------------------------------------------------------------------


def test_the_json_report_always_carries_all_seven_keys(rated_project) -> None:
    """Principle 9: `skip_serializing_if` is banned in `src/api/`, so a
    consumer never has to tell "absent key" from "no value". `viewer_up`
    (C-023) is additive and always present — `null` on every path that did
    not ask."""
    report = json.loads(
        _rate(rated_project, "--format", "json", "rate", RATED, "--dry-run").stdout
    )
    assert sorted(report) == [
        "action",
        "host",
        "provider",
        "ref",
        "up",
        "url",
        "viewer_up",
    ]


def test_the_plain_report_is_a_single_row_table(rated_project) -> None:
    result = _rate(rated_project, "rate", RATED, "--dry-run")
    assert result.returncode == 0, result.stderr
    lines = result.stdout.strip().splitlines()
    assert len(lines) == 2, f"header plus exactly one row: {result.stdout}"
    assert lines[0].startswith("Ref")
    assert "api.github.com" in lines[1]


# ---------------------------------------------------------------------------
# `provider: None` is a real state, distinct from an unrecognised one
# ---------------------------------------------------------------------------


def test_a_sidecar_with_no_provider_block_never_defaults_to_github(
    grim_at, project_dir: Path, http_index
) -> None:
    """A sidecar that states no rating producer leaves `provider` absent —
    which is *not* the same as an unrecognised one, and above all must not
    fall through to a default of ``github``. Silently voting against the
    wrong forge is the bug this pins."""
    root, base = http_index
    _publish(root, provider=None)
    _index_config(project_dir, base)
    runner = grim_at(project_dir)
    _warm_catalog(runner)

    report = json.loads(
        _rate(runner, "--format", "json", "rate", RATED, "--dry-run").stdout
    )
    assert report["provider"] is None, "no provider stated is null, never a default"
    assert report["host"] is None, "nothing to vote against, so no host resolves"
    assert report["up"] == 12, "the rating itself is still readable"


def test_a_sidecar_with_no_provider_block_refuses_a_real_vote(
    grim_at, project_dir: Path, http_index
) -> None:
    """Readable, not votable: exit 65, alongside `UnsupportedProvider`."""
    root, base = http_index
    _publish(root, provider=None)
    _index_config(project_dir, base)
    runner = grim_at(project_dir)
    _warm_catalog(runner)

    result = _rate(runner, "rate", RATED, "--yes")
    assert result.returncode == 65, result.stderr
    assert "no rating provider" in result.stderr, result.stderr


def test_an_unknown_provider_value_never_breaks_the_catalog_build(
    grim_at, project_dir: Path, http_index
) -> None:
    """C-001: the wire read is lenient. An unrecognised ``providers.rating``
    and unknown top-level keys degrade the artifact to "readable, not
    writable" — they never take the whole catalog down."""
    root, base = http_index
    (root / "all.json").write_text(json.dumps([_package("rated", RATED)]))
    (root / "stats.json").write_text(
        json.dumps(
            {
                "schema_version": 1,
                "generated_at": "2026-08-18T00:00:00Z",
                "providers": {"rating": "bitbucket", "downloads": "registry"},
                "entries": {
                    RATED: {
                        "rating": {"up": 5, "target": "t", "url": "u", "score": 0.5},
                        "downloads": {"total": 99},
                    }
                },
                "future_top_level_key": True,
            }
        )
    )
    _index_config(project_dir, base)
    runner = grim_at(project_dir)

    rows = runner.json("search", "--refresh")["items"]
    assert len(rows) == 1, f"the catalog still builds: {rows}"

    report = json.loads(
        _rate(runner, "--format", "json", "rate", RATED, "--dry-run").stdout
    )
    assert report["provider"] == "bitbucket", "the raw value survives the lenient read"
    assert report["up"] == 5, "readable"
    assert report["host"] is None, "not writable"
