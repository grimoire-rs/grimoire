# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""JSON interface contract tests — the 1.0 machine-readable surface.

Locks the cross-command invariants documented in
``docs/src/json-interface.md``: the uniform ``{"items": [...]}`` envelope
(incl. the empty case), the structured error document (two exit-code
classes), the "non-zero exit does not imply the error document" carve-out,
`search`'s nullable `kind` field, and MCP/CLI data parity without byte
identity.
"""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

from src.helpers import make_artifact, make_description, write_config
from src.registry import REGISTRY_HOST

_PROTOCOL = "2025-06-18"


def test_error_document_on_missing_config(
    grim_at, project_dir: Path
) -> None:
    """A failing run under --format json emits the structured error
    document on stdout; the human chain stays on stderr."""
    runner = grim_at(project_dir)
    missing = project_dir / "no-such-grimoire.toml"

    result = runner.run(
        "--format", "json", "--config", str(missing), "status", check=False
    )
    assert result.returncode == 79, result.stderr
    doc = json.loads(result.stdout)
    assert set(doc) == {"error"}, f"top-level error key marks the doc: {doc}"
    assert doc["error"]["code"] == "not-found"
    assert doc["error"]["exit"] == 79
    assert doc["error"]["message"], "message carries the rendered chain"
    assert "reason" not in doc["error"], (
        f"an unclassified error omits the optional reason subtype: {doc}"
    )
    assert "retryable" not in doc["error"], (
        f"no reason ⇒ no retryable key either: {doc}"
    )
    assert result.stderr.strip(), "human-readable chain still on stderr"


def test_error_document_on_usage_error(
    grim_at, project_dir: Path
) -> None:
    """A second exit-code class maps to its slug (unknown config key → 64)."""
    from src.helpers import write_config

    write_config(project_dir)
    runner = grim_at(project_dir)

    result = runner.run(
        "--format", "json", "config", "get", "optins.clients", check=False
    )
    assert result.returncode == 64, result.stderr
    doc = json.loads(result.stdout)
    assert doc["error"]["code"] == "usage"
    assert doc["error"]["exit"] == 64
    assert "reason" not in doc["error"], "a usage error carries no reason subtype"


def test_error_reason_marks_stale_lock(grim_at, project_dir: Path) -> None:
    """The error document carries a machine-readable ``reason`` for the
    stale-lock refusal, so a consumer detects it without scraping the
    non-frozen ``message``.

    The partial-resolve guard (``resolve_lock_partial``,
    ``src/resolve/resolver.rs``) runs BEFORE any registry I/O, so a
    hand-crafted lock whose ``declaration_hash`` deliberately mismatches the
    declared set triggers the refusal fully offline — no registry needed.
    """
    write_config(
        project_dir, skills={"code-review": "ghcr.io/acme/code-review:stable"}
    )
    # A valid lock whose all-zero declaration_hash cannot match the real
    # canonicalized hash of the declaration above.
    (project_dir / "grimoire.lock").write_text(
        "[metadata]\n"
        "lock_version = 1\n"
        "declaration_hash_version = 1\n"
        f'declaration_hash = "sha256:{"0" * 64}"\n'
        'generated_by = "grim test"\n'
        'generated_at = "2026-01-01T00:00:00Z"\n'
    )
    runner = grim_at(project_dir)

    result = runner.run(
        "--offline", "--format", "json", "update", "code-review", check=False
    )
    assert result.returncode == 65, result.stderr
    doc = json.loads(result.stdout)
    assert doc["error"]["code"] == "data"
    assert doc["error"]["exit"] == 65
    assert doc["error"]["reason"] == "stale-lock"


def test_modified_refusal_carries_reason(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The error document carries the machine-readable ``reason``
    ``modified`` for the local-modification integrity refusal — on both
    `grim install` and `grim add` (install-on-add shares the pipeline), so
    an extension retries the same command with `--force` without scraping
    the non-frozen ``message``."""
    repo = f"{unique_repo}/rust-style"
    make_artifact(
        repo,
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# canonical\n"},
        tag="v1",
    )
    write_config(project_dir, rules={"rust-style": f"{registry}/{repo}:v1"})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    (project_dir / ".claude/rules/rust-style.md").write_text("hand edited\n")

    for argv in (
        ("--format", "json", "install"),
        ("--format", "json", "add", f"{registry}/{repo}:v1"),
    ):
        result = runner.run(*argv, check=False)
        assert result.returncode == 65, f"{argv}: {result.stderr}"
        doc = json.loads(result.stdout)
        assert doc["error"]["code"] == "data", argv
        assert doc["error"]["exit"] == 65, argv
        assert doc["error"]["reason"] == "modified", f"{argv}: {doc}"


def test_error_reason_marks_no_config(grim_at, project_dir: Path) -> None:
    """A project-scope command run in a directory with no discoverable
    ``grimoire.toml`` (walk-up finds nothing, ceiling'd at the isolated
    ``$HOME``) carries reason ``no-config`` — distinct from an explicit
    ``--config <path>`` that does not exist, which carries no reason at
    all (`test_error_document_on_missing_config`)."""
    runner = grim_at(project_dir)  # empty dir; no grimoire.toml anywhere up

    result = runner.run("--format", "json", "status", check=False)
    assert result.returncode == 79, result.stderr
    doc = json.loads(result.stdout)
    assert doc["error"]["code"] == "not-found"
    assert doc["error"]["exit"] == 79
    assert doc["error"]["reason"] == "no-config"
    assert "retryable" not in doc["error"], (
        f"no-config is not a retryable reason: {doc}"
    )


@pytest.mark.skipif(
    sys.platform == "win32", reason="POSIX fcntl.flock sidecar contention"
)
def test_error_reason_marks_locked_and_retryable(
    grim_at, project_dir: Path
) -> None:
    """A ``grim config set`` that loses the advisory-flock race on the
    ``grimoire.toml.lock`` sidecar exits 75 (TempFail) with reason
    ``locked`` and ``retryable: true`` — the one reason
    ``ErrorReason::retryable`` reports ``true`` for.

    Holds the sidecar's exclusive flock directly from the test process
    (POSIX ``fcntl.flock``, non-blocking) so the contention is deterministic
    rather than racing two subprocesses."""
    import fcntl
    import os

    write_config(project_dir)
    runner = grim_at(project_dir)
    sidecar = project_dir / "grimoire.toml.lock"

    fd = os.open(sidecar, os.O_CREAT | os.O_RDWR, 0o644)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)

        result = runner.run(
            "--format", "json", "config", "set", "options.clients", "claude",
            check=False,
        )
        assert result.returncode == 75, result.stderr
        doc = json.loads(result.stdout)
        assert doc["error"]["code"] == "temp-fail"
        assert doc["error"]["exit"] == 75
        assert doc["error"]["reason"] == "locked"
        assert doc["error"]["retryable"] is True
    finally:
        fcntl.flock(fd, fcntl.LOCK_UN)
        os.close(fd)


def test_plain_mode_failure_keeps_stdout_empty(
    grim_at, project_dir: Path
) -> None:
    """Without --format json, a failure writes nothing to stdout."""
    runner = grim_at(project_dir)
    missing = project_dir / "no-such-grimoire.toml"

    result = runner.plain("--config", str(missing), "status", check=False)
    assert result.returncode == 79, result.stderr
    assert result.stdout == "", f"plain failure must not write stdout: {result.stdout!r}"
    assert result.stderr.strip()


def test_list_reports_use_items_envelope(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Multi-item reports are `{"items": [...]}` objects, never bare arrays."""
    repo = f"{unique_repo}/s"
    make_artifact(repo, "skill", {"s/SKILL.md": "v\n"}, tag="stable")
    write_config(project_dir, skills={"s": f"{registry}/{repo}:stable"})
    runner = grim_at(project_dir)

    result = runner.run("--format", "json", "status", check=False)
    assert result.returncode == 0
    doc = json.loads(result.stdout)
    assert isinstance(doc, dict), "top-level JSON must be an object envelope"
    assert isinstance(doc["items"], list)
    assert doc["items"][0]["name"] == "s"


def test_empty_result_is_items_empty_array(grim_at, project_dir: Path) -> None:
    """A multi-item report with no rows is `{"items": []}`, never an
    absent key, `null`, or a bare `[]`. `status` also carries the
    always-present sibling envelope key `checked` (`false` without
    `--check`)."""
    write_config(project_dir)  # no skills/rules/bundles/agents declared
    runner = grim_at(project_dir)

    result = runner.run("--format", "json", "status", check=False)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == {"items": [], "checked": False}


def test_config_get_unset_key_reports_data_not_error(
    grim_at, project_dir: Path
) -> None:
    """A non-zero exit does not imply the error document: `config get` of a
    valid-but-unset key exits 1 and still prints the full data report, with
    no top-level `error` key — the "branch on error-key first, then exit
    code" contract."""
    write_config(project_dir)
    runner = grim_at(project_dir)

    result = runner.run(
        "--format", "json", "config", "get", "options.clients", check=False
    )
    assert result.returncode == 1, result.stderr
    doc = json.loads(result.stdout)
    assert "error" not in doc, f"a data report must not carry a top-level error key: {doc}"
    assert doc == {
        "key": "options.clients",
        "value": None,
        "set": False,
        "scope": "project",
    }


def test_config_list_items_carry_metadata_fields(
    grim_at, project_dir: Path
) -> None:
    """`config list --all --format json` items carry exactly the 9 frozen
    fields (`key`, `value`, `set`, `type`, `title`, `description`,
    `default`, `values`, `constraints`) — the always-present-null policy
    applies to `config list` entries too, not just single-object reports.

    `constraints` is non-null only for `options.tui.tree_separators`
    (advisory `item_pattern` regex + `item_width`, the width rule the
    pattern cannot express) — every other key, including the closed-set
    `options.clients`, carries `constraints: null`.
    """
    write_config(project_dir)
    runner = grim_at(project_dir)

    items = runner.json("config", "list", "--all")["items"]
    assert items, "config list --all on a fresh config must yield rows"
    item = items[0]
    assert set(item.keys()) == {
        "key", "value", "set", "type", "title", "description", "default",
        "values", "constraints",
    }, f"item must carry exactly the 9 frozen fields; got: {sorted(item.keys())}"

    by_key = {row["key"]: row for row in items}
    tree_separators = by_key["options.tui.tree_separators"]
    assert tree_separators["constraints"] == {
        "item_pattern": r"^[^\s\p{C}]$",
        "item_width": 1,
    }, f"tree_separators constraints must carry item_pattern/item_width; got: {tree_separators['constraints']}"

    other_constraints = {
        key: row["constraints"] for key, row in by_key.items() if key != "options.tui.tree_separators"
    }
    assert all(
        value is None for value in other_constraints.values()
    ), f"every non-tree-separators item must carry constraints: null; got: {other_constraints}"


def test_search_surfaces_null_kind_for_unrecognized_manifest(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`search` items carry `kind: null` (not an absent key) when the
    manifest declares no kind grim recognizes.

    ``kind_from_manifest`` (``src/oci/annotations.rs``) resolves three
    tiers — `artifactType`, the legacy config media type, then the
    `com.grimoire.kind` annotation — and returns `None` when none names a
    known `ArtifactKind` (`skill`/`rule`/`agent`/`bundle`/`mcp`). Pushing an
    artifact tagged with an unrecognized kind string puts a manifest on the
    wire whose `artifactType` and `com.grimoire.kind` annotation both carry
    that same unrecognized value — neither tier resolves — which is exactly
    the "foreign manifest" condition the Rust unit tests
    (`kind_from_manifest_none_for_foreign_image`) exercise directly. The
    catalog build never filters by kind (`build_entry` in
    `src/catalog/registry_catalog.rs`), so the row still surfaces here.
    """
    make_artifact(
        f"{unique_repo}/mystery",
        "widget",  # not a recognized ArtifactKind
        {"mystery.md": "opaque content\n"},
        tag="latest",
    )
    runner = grim_at(project_dir)

    rows = runner.json(
        "search", unique_repo, "--registry", f"{REGISTRY_HOST}/{unique_repo}", "--refresh"
    )["items"]
    entry = next(r for r in rows if r["repo"].endswith(f"{unique_repo}/mystery"))
    assert entry["kind"] is None


def test_describe_is_single_object_with_all_fields_present(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`describe` is a single flat object (no `items` envelope, no `error`
    key); every field is always present, `null` when absent (`revision` /
    `created` here), and `keywords`/`tags` are arrays, `annotations` a map —
    the single-object null policy."""
    repo = f"{unique_repo}/skills/desc"
    make_artifact(
        repo,
        "skill",
        {"desc/SKILL.md": "---\nname: desc\ndescription: d\n---\n# d\n"},
        tag="latest",
        annotations={"com.grimoire.replaced-by": "ghcr.io/acme/skills/desc-2"},
    )
    runner = grim_at(project_dir)

    doc = runner.json("describe", f"{registry}/{repo}:latest")
    assert isinstance(doc, dict)
    assert "items" not in doc and "error" not in doc
    for key in (
        "ref", "digest", "kind", "name", "title", "description",
        "has_description", "summary", "version", "license", "repository",
        "revision", "created", "keywords", "deprecated", "replaced_by",
        "tags", "annotations",
    ):
        assert key in doc, f"single-object report must always carry {key}"
    assert doc["revision"] is None and doc["created"] is None
    assert isinstance(doc["has_description"], bool), "companion presence is an always-present bool"
    assert isinstance(doc["keywords"], list)
    assert isinstance(doc["tags"], list)
    assert isinstance(doc["annotations"], dict)
    assert doc["replaced_by"] == "ghcr.io/acme/skills/desc-2"


def test_fetch_report_is_tri_shaped(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`fetch` JSON has three documented shapes: content (`{ref, digest, kind,
    …, content}`), a description bundle (`{ref, digest, kind: "desc",
    files[]}`), and a digest probe (`{ref, digest}` only). Each is a flat
    object, never an `items` envelope or `error` key."""
    repo = f"{unique_repo}/skills/tri"
    make_artifact(
        repo, "skill", {"tri/SKILL.md": "---\nname: tri\ndescription: d.\n---\n# t\n"}, tag="latest"
    )
    make_description(repo, {"README.md": b"# Repo\n"})
    # A resolved project scope keeps the digest probe to its minimal shape.
    (project_dir / "grimoire.toml").write_text("[skills]\n")
    runner = grim_at(project_dir)
    ref = f"{registry}/{repo}:latest"

    content = runner.json("fetch", ref)
    assert "items" not in content and "error" not in content
    assert content["kind"] == "skill" and "content" in content

    bundle = runner.json("fetch", ref, "--description")
    assert bundle["kind"] == "desc"
    assert isinstance(bundle["files"], list) and bundle["files"]
    assert {"path", "size", "content"} <= set(bundle["files"][0])

    probe = runner.json("fetch", ref, "--digest-only")
    assert set(probe) == {"ref", "digest"}, f"digest probe is exactly {{ref, digest}}: {probe}"
    assert probe["digest"] == content["digest"], "the probe digest equals the full fetch digest"


def test_mcp_and_cli_status_share_data_but_differ_in_bytes(
    grim_at, project_dir: Path
) -> None:
    """`grim_status` (MCP) and `grim status --format json` (CLI) carry
    identical data, envelope included, but are not byte-identical: the CLI
    pretty-prints (`serde_json::to_string_pretty`,
    `src/api/status_report.rs`), MCP emits compact JSON
    (`serde_json::to_string`, `src/mcp/server.rs`)."""
    write_config(project_dir)
    runner = grim_at(project_dir)

    cli_result = runner.run("--offline", "--format", "json", "status", check=False)
    assert cli_result.returncode == 0, cli_result.stderr
    cli_bytes = cli_result.stdout

    payload = "".join(
        json.dumps(r) + "\n"
        for r in [
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": _PROTOCOL,
                    "capabilities": {},
                    "clientInfo": {"name": "pytest", "version": "0"},
                },
            },
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "grim_status", "arguments": {}},
            },
        ]
    )
    mcp_result = subprocess.run(
        [str(runner.binary), "--offline", "mcp"],
        input=payload,
        capture_output=True,
        text=True,
        env=runner.env,
        cwd=str(project_dir),
        timeout=30,
    )
    responses: dict[int, dict] = {}
    for line in mcp_result.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        msg = json.loads(line)
        if isinstance(msg.get("id"), int):
            responses[msg["id"]] = msg
    assert 2 in responses, (
        f"grim mcp did not answer the tools/call request; got ids {sorted(responses)}"
    )
    mcp_text = responses[2]["result"]["content"][0]["text"]

    assert json.loads(mcp_text) == json.loads(cli_bytes), (
        "MCP and CLI must carry identical data (envelope included) for the same scope"
    )
    assert mcp_text != cli_bytes.strip(), (
        "MCP (compact) and CLI (pretty) must not be byte-identical"
    )
    assert "\n" in cli_bytes.strip(), "CLI JSON must be pretty-printed (multi-line)"
    assert "\n" not in mcp_text, "MCP JSON must be compact (single line)"
