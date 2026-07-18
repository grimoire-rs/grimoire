# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim status` acceptance tests — state is data, always exit 0."""
from __future__ import annotations

from pathlib import Path

from src.helpers import make_artifact, write_config
from src.registry import REGISTRY_HOST, retag

DEPRECATED = "com.grimoire.deprecated"
REPLACED_BY = "com.grimoire.replaced-by"


def test_status_json_is_array_and_exit_0(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    repo = f"{unique_repo}/s"
    make_artifact(repo, "skill", {"s/SKILL.md": "v\n"}, tag="stable")
    write_config(project_dir, skills={"s": f"{registry}/{repo}:stable"})
    runner = grim_at(project_dir)

    result = runner.run("--format", "json", "status", check=False)
    assert result.returncode == 0
    import json

    arr = json.loads(result.stdout)["items"]
    assert isinstance(arr, list)
    assert arr[0]["name"] == "s"


def test_status_missing_then_installed(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    repo = f"{unique_repo}/s"
    make_artifact(repo, "skill", {"s/SKILL.md": "v1\n"}, tag="stable")
    write_config(project_dir, skills={"s": f"{registry}/{repo}:stable"})
    runner = grim_at(project_dir)

    runner.run("lock", check=False)
    rows = runner.json("status")["items"]
    assert rows[0]["state"] == "missing"

    runner.run("install", check=False)
    rows = runner.json("status")["items"]
    assert rows[0]["state"] == "installed"


def test_status_stale_when_config_changed(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    a_repo = f"{unique_repo}/a"
    make_artifact(a_repo, "rule", {"a.md": "a\n"}, tag="latest")
    write_config(project_dir, rules={"a": f"{registry}/{a_repo}:latest"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.run("install", check=False)

    # Add a rule without re-locking ⇒ lock declaration hash drifts.
    b_repo = f"{unique_repo}/b"
    make_artifact(b_repo, "rule", {"b.md": "b\n"}, tag="latest")
    write_config(
        project_dir,
        rules={
            "a": f"{registry}/{a_repo}:latest",
            "b": f"{registry}/{b_repo}:latest",
        },
    )
    result = runner.run("--format", "json", "status", check=False)
    assert result.returncode == 0
    import json

    rows = json.loads(result.stdout)["items"]
    assert all(r["state"] == "stale" for r in rows)


def test_status_json_includes_installed_outputs(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`outputs` carries per-client `{client, path}` for an installed
    artifact; a declared-but-not-installed artifact gets `outputs: []`."""
    repo = f"{unique_repo}/s"
    make_artifact(repo, "skill", {"s/SKILL.md": "v\n"}, tag="stable")
    write_config(project_dir, skills={"s": f"{registry}/{repo}:stable"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.run("install", check=False)

    # Declare a second skill and re-lock so the config matches the lock
    # (not stale) without installing it — declared-but-not-installed.
    repo2 = f"{unique_repo}/s2"
    make_artifact(repo2, "skill", {"s2/SKILL.md": "v\n"}, tag="stable")
    write_config(
        project_dir,
        skills={
            "s": f"{registry}/{repo}:stable",
            "s2": f"{registry}/{repo2}:stable",
        },
    )
    runner.run("lock", check=False)

    rows = runner.json("status")["items"]
    installed = next(r for r in rows if r["name"] == "s")
    not_installed = next(r for r in rows if r["name"] == "s2")

    assert installed["state"] == "installed"
    assert len(installed["outputs"]) > 0
    for output in installed["outputs"]:
        assert set(output.keys()) == {"client", "path"}
        assert Path(output["path"]).exists()

    assert not_installed["state"] == "missing"
    assert not_installed["outputs"] == []


def test_status_item_carries_client_drift_fields(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Every status item carries the always-present `clients_missing` /
    `clients_extra` arrays alongside the existing 6-key shape, plus the
    `--check`-shaped `deprecated` / `replaced_by` / `update_available` — the
    frozen key set is 11, and every new field defaults to empty/null
    without `--check`."""
    repo = f"{unique_repo}/s"
    make_artifact(repo, "skill", {"s/SKILL.md": "v\n"}, tag="stable")
    write_config(project_dir, skills={"s": f"{registry}/{repo}:stable"})
    runner = grim_at(project_dir)
    # A single-client config, installed, gives desired == recorded == {claude}
    # — the no-drift case (an undeclared/uninstalled row instead has a
    # nonempty `clients_missing`, exercised by the narrow/widen test below).
    runner.run("config", "set", "options.clients", "claude", check=False)
    runner.run("lock", check=False)
    runner.run("install", check=False)

    result = runner.json("status")
    assert result["checked"] is False, "no --check ⇒ checked is false"
    rows = result["items"]
    assert set(rows[0].keys()) == {
        "kind", "name", "source", "pinned", "state", "outputs",
        "clients_missing", "clients_extra",
        "deprecated", "replaced_by", "update_available",
    }, f"status item must carry exactly the 11 frozen fields; got: {sorted(rows[0].keys())}"
    assert rows[0]["clients_missing"] == []
    assert rows[0]["clients_extra"] == []
    assert rows[0]["deprecated"] is None
    assert rows[0]["replaced_by"] is None
    assert rows[0]["update_available"] is None


def test_status_client_drift_narrow_then_widen(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`clients_missing`/`clients_extra` report client-set drift entirely
    from local state (config + install record), no network: narrowing
    `options.clients` below what's installed names the dropped client in
    `clients_extra`; widening beyond what's installed names the new
    client in `clients_missing`."""
    repo = f"{unique_repo}/s"
    make_artifact(repo, "skill", {"s/SKILL.md": "v\n"}, tag="stable")
    write_config(project_dir, skills={"s": f"{registry}/{repo}:stable"})
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.clients", "claude,opencode", check=False)
    runner.run("lock", check=False)
    runner.run("install", check=False)

    # Narrow to just claude: opencode's recorded output is now extra.
    runner.run("config", "set", "options.clients", "claude", check=False)
    row = runner.json("--offline", "status")["items"][0]
    assert row["clients_missing"] == []
    assert row["clients_extra"] == ["opencode"]

    # Widen to include codex, never installed: it's now missing, and the
    # still-present opencode output is no longer extra.
    runner.run(
        "config", "set", "options.clients", "claude,opencode,codex", check=False
    )
    row = runner.json("--offline", "status")["items"][0]
    assert row["clients_missing"] == ["codex"]
    assert row["clients_extra"] == []


def test_status_outdated_when_lock_advances(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # Pin by explicit digest so re-locking observes the new pin without a
    # tag-cache round-trip: install v1, then point the config at v2's
    # digest and re-lock (without reinstalling). The lock pin now differs
    # from the install-state record ⇒ outdated.
    repo = f"{unique_repo}/s"
    v1 = make_artifact(repo, "skill", {"s/SKILL.md": "v1\n"}, tag="1.0.0")
    v2 = make_artifact(repo, "skill", {"s/SKILL.md": "v2\n"}, tag="2.0.0")
    assert v1.digest != v2.digest

    write_config(project_dir, skills={"s": v1.pinned})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.run("install", check=False)

    write_config(project_dir, skills={"s": v2.pinned})
    runner.run("lock", check=False)

    rows = runner.json("status")["items"]
    assert rows[0]["state"] == "outdated"


# ── --check: live catalog re-check (issue #43, C3) ─────────────────────


def _install_deprecated_skill(
    grim_at, project_dir: Path, unique_repo: str
) -> tuple:
    """Publish a deprecated, replaced skill and declare/lock/install it.

    Reuses the `com.grimoire.deprecated` / `com.grimoire.replaced-by`
    annotation fixtures from the search deprecation suite
    (`test_deprecation.py`). Returns the runner and the registry namespace
    the catalog check must be scoped to (`--registry`) so the browse set
    hits this test's throwaway repo instead of the global default.
    """
    repo = f"{unique_repo}/old-skill"
    make_artifact(
        repo,
        "skill",
        {"old-skill/SKILL.md": "---\nname: old-skill\n---\n# old\n"},
        tag="stable",
        annotations={
            DEPRECATED: "use new-skill instead",
            REPLACED_BY: "ghcr.io/acme/new-skill",
        },
    )
    write_config(project_dir, skills={"old-skill": f"{REGISTRY_HOST}/{repo}:stable"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.run("install", check=False)
    return runner


def test_status_check_populates_deprecation_fields(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`--check` runs one coordinated catalog load and fills in
    `deprecated` / `replaced_by` on the matching registry-sourced row;
    the top-level `checked` reports `true`."""
    runner = _install_deprecated_skill(grim_at, project_dir, unique_repo)

    result = runner.json(
        "--registry", f"{REGISTRY_HOST}/{unique_repo}", "status", "--check"
    )
    assert result["checked"] is True
    row = next(r for r in result["items"] if r["name"] == "old-skill")
    assert row["deprecated"] == "use new-skill instead"
    assert row["replaced_by"] == "ghcr.io/acme/new-skill"
    # Reserved for a future release — always null regardless of `checked`.
    assert row["update_available"] is None


def test_status_default_run_leaves_remote_fields_null(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Contract guard: without `--check` grim never touches the network, so
    `checked` is `false` and `deprecated`/`replaced_by` stay `null` even for
    an artifact the registry has since marked deprecated — proving the
    fields are not populated by accident from some other local source."""
    runner = _install_deprecated_skill(grim_at, project_dir, unique_repo)

    result = runner.json("status")
    assert result["checked"] is False
    row = next(r for r in result["items"] if r["name"] == "old-skill")
    assert row["deprecated"] is None
    assert row["replaced_by"] is None
    assert row["update_available"] is None


def test_status_check_offline_degrades(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`--check --offline` never touches the network: exits 0, `checked`
    stays `false`, remote fields stay `null`, and a warning explains why."""
    runner = _install_deprecated_skill(grim_at, project_dir, unique_repo)

    result = runner.run(
        "--offline",
        "--registry", f"{REGISTRY_HOST}/{unique_repo}",
        "status", "--check",
        format="json",
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert "requires network access" in result.stderr, result.stderr

    import json

    doc = json.loads(result.stdout)
    assert doc["checked"] is False
    row = next(r for r in doc["items"] if r["name"] == "old-skill")
    assert row["deprecated"] is None
    assert row["replaced_by"] is None
