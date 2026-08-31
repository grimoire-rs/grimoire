# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Package-index browse-source acceptance tests (`[[registries]] index = …`).

A ``[[registries]]`` entry sets exactly one of ``url`` / ``index``. An
``index`` entry lists packages from a package index instead of the OCI
``_catalog`` endpoint, over two transports:

- HTTP(S): a compiled static index — ``<base>/all.json``
- git: a shallow clone walking ``index/**/metadata.json``

The index is a phone book: entries carry ``ref`` (registry/repository),
kind, and description — never versions. Search rows therefore surface with
no version data; installs resolve tags live from the registry.
"""
from __future__ import annotations

import json
import subprocess
import threading
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest


# ---------------------------------------------------------------------------
# Helpers / fixtures
# ---------------------------------------------------------------------------


def _package(
    name: str,
    kind: str,
    ref: str,
    description: str,
    keywords: list[str] | None = None,
    summary: str | None = None,
    deprecated: str | None = None,
    replaced_by: str | None = None,
) -> dict:
    pkg = {
        "schema": 1,
        "name": name,
        "kind": kind,
        "ref": ref,
        "description": description,
        "repository": "https://github.com/acme/skills",
        "owner": {"github": "acme", "id": 1},
    }
    # Omit-empty, mirroring the announce writer: pre-search-metadata
    # pointers carry neither key.
    if keywords:
        pkg["keywords"] = keywords
    if summary is not None:
        pkg["summary"] = summary
    if deprecated is not None:
        pkg["deprecated"] = deprecated
    if replaced_by is not None:
        pkg["replaced_by"] = replaced_by
    return pkg


@pytest.fixture()
def http_index(tmp_path: Path):
    """A local static webserver serving an index dist dir (all.json)."""
    root = tmp_path / "index-dist"
    root.mkdir()
    handler = partial(SimpleHTTPRequestHandler, directory=str(root))
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    yield root, f"http://127.0.0.1:{server.server_address[1]}"
    server.shutdown()


def _write_all_json(root: Path, packages: list[dict]) -> None:
    (root / "all.json").write_text(json.dumps(packages))


def _git_index_repo(tmp_path: Path, packages: list[dict]) -> Path:
    """A local git repository (path ends in ``.git`` so it classifies as a
    git index locator) holding ``index/github.com/<ns>/<pkg>/metadata.json``."""
    repo = tmp_path / "index-repo.git"
    for pkg in packages:
        d = repo / "index" / "github.com" / "acme" / pkg["name"]
        d.mkdir(parents=True)
        (d / "metadata.json").write_text(json.dumps(pkg))
    def git(*args: str) -> None:
        subprocess.run(
            ["git", "-c", "user.email=t@t", "-c", "user.name=t", *args],
            cwd=repo,
            check=True,
            capture_output=True,
        )
    subprocess.run(["git", "init", "-q", str(repo)], check=True, capture_output=True)
    git("add", "-A")
    git("commit", "-q", "-m", "seed")
    return repo


def _index_config(project_dir: Path, locator: str) -> None:
    # `locator` goes into a TOML basic string, so callers passing a local
    # path must forward-slash it (`Path.as_posix()`): a Windows `str(path)`
    # embeds backslashes, which are invalid TOML escapes. git clones a
    # forward-slash path fine on every platform.
    (project_dir / "grimoire.toml").write_text(
        f'[[registries]]\n'
        f'alias = "hub"\n'
        f'index = "{locator}"\n'
        f'default = true\n'
        f'\n[skills]\n\n[rules]\n'
    )


def _search_rows(runner) -> list[dict]:
    result = runner.run("--format", "json", "search", "--refresh", check=False)
    assert result.returncode == 0, (
        f"index-backed search must exit 0, got {result.returncode}; stderr: {result.stderr}"
    )
    rows = json.loads(result.stdout)["items"]
    assert isinstance(rows, list)
    return rows


# ---------------------------------------------------------------------------
# HTTP transport
# ---------------------------------------------------------------------------


def test_search_http_index_lists_packages(grim_at, project_dir: Path, http_index) -> None:
    """``grim search`` against an ``index = http://…`` source lists the
    packages from ``all.json`` — no OCI registry involved at all."""
    root, base = http_index
    _write_all_json(
        root,
        [
            _package("idx-skill", "skill", "ghcr.io/acme/skills/idx-skill", "From the index"),
            _package("idx-rule", "rule", "registry.example/acme/rules/idx-rule", "Rule pointer"),
        ],
    )
    _index_config(project_dir, base)

    rows = _search_rows(grim_at(project_dir))
    repos = [r.get("repo", "") for r in rows]
    assert "ghcr.io/acme/skills/idx-skill" in repos, f"got {repos}"
    assert "registry.example/acme/rules/idx-rule" in repos, f"got {repos}"

    skill = next(r for r in rows if r.get("repo") == "ghcr.io/acme/skills/idx-skill")
    assert skill.get("kind") == "skill"
    assert skill.get("description") == "From the index"
    # Phone-book contract: the index carries no version data.
    assert not skill.get("version"), f"index rows carry no version, got {skill!r}"


def test_search_http_index_filters_by_query(grim_at, project_dir: Path, http_index) -> None:
    root, base = http_index
    _write_all_json(
        root,
        [
            _package("alpha-skill", "skill", "ghcr.io/acme/skills/alpha-skill", "Alpha"),
            _package("beta-rule", "rule", "ghcr.io/acme/rules/beta-rule", "Beta"),
        ],
    )
    _index_config(project_dir, base)

    runner = grim_at(project_dir)
    result = runner.run("--format", "json", "search", "--refresh", "alpha", check=False)
    assert result.returncode == 0, result.stderr
    repos = [r.get("repo", "") for r in json.loads(result.stdout)["items"]]
    assert repos == ["ghcr.io/acme/skills/alpha-skill"], f"got {repos}"


def test_search_http_index_matches_by_keyword(grim_at, project_dir: Path, http_index) -> None:
    """A query term present only in an index pointer's ``keywords`` (never in
    the repo name or description) still matches — the index now carries the
    search metadata the phone book used to drop."""
    root, base = http_index
    _write_all_json(
        root,
        [
            _package(
                "grim",
                "mcp",
                "ghcr.io/grimoire-rs/mcp/grim",
                "The grimoire MCP server",
                keywords=["catalog", "fetch", "render"],
                summary="Drive grim from an agent",
            ),
            _package("other", "skill", "ghcr.io/acme/skills/other", "Unrelated"),
        ],
    )
    _index_config(project_dir, base)

    runner = grim_at(project_dir)
    result = runner.run("--format", "json", "search", "--refresh", "fetch", check=False)
    assert result.returncode == 0, result.stderr
    repos = [r.get("repo", "") for r in json.loads(result.stdout)["items"]]
    assert repos == ["ghcr.io/grimoire-rs/mcp/grim"], f"keyword-only match failed: {repos}"


def test_search_http_index_summary_reaches_search_row(grim_at, project_dir: Path, http_index) -> None:
    """A package's ``summary`` from the index ``all.json`` surfaces on the
    search row (``row["summary"]``) — the index now carries the catalog blurb
    the phone book used to drop."""
    root, base = http_index
    _write_all_json(
        root,
        [
            _package(
                "sum-skill",
                "skill",
                "ghcr.io/acme/skills/sum-skill",
                "A skill with a summary",
                summary="terse catalog blurb",
            ),
        ],
    )
    _index_config(project_dir, base)

    rows = _search_rows(grim_at(project_dir))
    row = next(r for r in rows if r.get("repo") == "ghcr.io/acme/skills/sum-skill")
    assert row.get("summary") == "terse catalog blurb", (
        f"index summary must reach the search row, got {row!r}"
    )


def test_search_http_index_hides_and_marks_deprecated(grim_at, project_dir: Path, http_index) -> None:
    """A deprecated, *not installed*, index-sourced package is hidden by
    default and marked once shown — the same contract a registry-catalog
    source already honours. The pointer is the only browse-time source, so
    dropping its ``deprecated`` field made the row render as verified-healthy
    (grimoire#58)."""
    root, base = http_index
    _write_all_json(
        root,
        [
            _package("fresh-skill", "skill", "ghcr.io/acme/skills/fresh-skill", "A current one"),
            _package(
                "old-skill",
                "skill",
                "ghcr.io/acme/skills/old-skill",
                "An old one",
                deprecated="use new-skill instead",
                replaced_by="ghcr.io/acme/skills/new-skill",
            ),
        ],
    )
    _index_config(project_dir, base)
    runner = grim_at(project_dir)

    repos = [r.get("repo", "") for r in _search_rows(runner)]
    assert repos == ["ghcr.io/acme/skills/fresh-skill"], (
        f"a deprecated, uninstalled index row must be hidden by default: {repos}"
    )

    result = runner.run("--format", "json", "search", "--refresh", "--show-deprecated", check=False)
    assert result.returncode == 0, result.stderr
    rows = json.loads(result.stdout)["items"]
    old = next(r for r in rows if r.get("repo") == "ghcr.io/acme/skills/old-skill")
    assert old.get("deprecated") == "use new-skill instead", f"got {old!r}"
    assert old.get("replaced_by") == "ghcr.io/acme/skills/new-skill", f"got {old!r}"
    assert old.get("status") == "not-installed", f"got {old!r}"

    # Plain output marks the row too — deprecation rides a comma-suffixed
    # Status cell, so it is greppable for an uninstalled row as well.
    plain = runner.run("search", "--refresh", "--show-deprecated", check=False)
    assert plain.returncode == 0, plain.stderr
    marked = [line for line in plain.stdout.splitlines() if "old-skill" in line]
    assert marked and "not-installed,deprecated" in marked[0], f"got {marked!r}"


def test_search_unreachable_http_index_degrades_to_empty(grim_at, project_dir: Path) -> None:
    """An unreachable index degrades that source to an empty group — the
    browse still exits 0 (same contract as an unreachable registry)."""
    _index_config(project_dir, "http://127.0.0.1:1/absent")
    rows = _search_rows(grim_at(project_dir))
    assert rows == [], f"unreachable index must yield no rows, got {rows}"


def test_search_json_names_the_failed_index_source(grim_at, project_dir: Path) -> None:
    """A failed ``index =`` source is named in the envelope, not just on stderr.

    Same contract as an unreachable OCI registry (issue #108): the browse
    degrades that source to an empty group and exits 0, but the ``sources``
    sibling says which source failed and why, so an empty ``items`` is never
    mistaken for an empty index.
    """
    locator = "http://127.0.0.1:1/absent"
    _index_config(project_dir, locator)
    runner = grim_at(project_dir)

    result = runner.run("--format", "json", "search", "--refresh", check=False)
    assert result.returncode == 0, result.stderr
    doc = json.loads(result.stdout)

    assert doc["items"] == [], f"unreachable index yields no rows, got {doc['items']!r}"
    assert doc["sources"] == [
        {"alias": "hub", "locator": locator, "ok": False, "error": doc["sources"][0]["error"]}
    ], f"the failed index source is named: {doc['sources']!r}"
    assert doc["sources"][0]["error"], "a failed source carries a non-null error"


# ---------------------------------------------------------------------------
# Git transport
# ---------------------------------------------------------------------------


def test_search_git_index_lists_packages(grim_at, project_dir: Path, tmp_path: Path) -> None:
    """``index = <repo>.git`` shallow-clones the index repository and walks
    ``index/**/metadata.json`` — works against GitHub, GitLab, or any plain
    git host; here a local repository stands in."""
    repo = _git_index_repo(
        tmp_path,
        [
            _package("git-skill", "skill", "ghcr.io/acme/skills/git-skill", "Cloned pointer"),
            _package("git-bundle", "bundle", "gitlab.example/acme/bundles/git-bundle", "Bundle pointer"),
        ],
    )
    _index_config(project_dir, repo.as_posix())

    rows = _search_rows(grim_at(project_dir))
    repos = [r.get("repo", "") for r in rows]
    assert "ghcr.io/acme/skills/git-skill" in repos, f"got {repos}"
    assert "gitlab.example/acme/bundles/git-bundle" in repos, f"got {repos}"


def test_git_index_refresh_picks_up_new_packages(grim_at, project_dir: Path, tmp_path: Path) -> None:
    """A second ``--refresh`` re-clones and surfaces newly announced packages."""
    repo = _git_index_repo(
        tmp_path,
        [_package("first", "skill", "ghcr.io/acme/skills/first", "First")],
    )
    _index_config(project_dir, repo.as_posix())
    runner = grim_at(project_dir)

    assert any("first" in r.get("repo", "") for r in _search_rows(runner))

    d = repo / "index" / "github.com" / "acme" / "second"
    d.mkdir(parents=True)
    (d / "metadata.json").write_text(
        json.dumps(_package("second", "rule", "ghcr.io/acme/rules/second", "Second"))
    )
    subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"],
        cwd=repo, check=True, capture_output=True,
    )
    subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "announce"],
        cwd=repo, check=True, capture_output=True,
    )

    repos = [r.get("repo", "") for r in _search_rows(runner)]
    assert any("second" in r for r in repos), f"got {repos}"


# ---------------------------------------------------------------------------
# No project scope (issue #41)
# ---------------------------------------------------------------------------


def test_search_outside_project_browses_global_index_source(
    grim_at, grim_home: Path, tmp_path: Path, http_index
) -> None:
    """``grim search`` run outside any project honors a GLOBAL config's
    ``[[registries]]`` index source — regression for issue #41, where the
    no-scope path browsed the push-side GHCR fallback (ignoring the global
    config tiers entirely) and mis-warned about ``_catalog`` gating."""
    root, base = http_index
    _write_all_json(
        root,
        [_package("outside-skill", "skill", "ghcr.io/acme/skills/outside-skill", "From the index")],
    )
    (grim_home / "grimoire.toml").write_text(
        f'[[registries]]\nalias = "hub"\nindex = "{base}"\ndefault = true\n'
    )
    outside = tmp_path / "outside"
    outside.mkdir()

    result = grim_at(outside).run("--format", "json", "search", "--refresh", check=False)
    assert result.returncode == 0, result.stderr
    repos = [r.get("repo", "") for r in json.loads(result.stdout)["items"]]
    assert "ghcr.io/acme/skills/outside-skill" in repos, f"got {repos}"
    assert "no catalog entries" not in result.stderr, (
        f"the _catalog-gating warn must not fire for an index-only browse: {result.stderr}"
    )


# ---------------------------------------------------------------------------
# Mixed sources
# ---------------------------------------------------------------------------


def test_index_and_registry_sources_combine(
    grim_at, project_dir: Path, registry: str, http_index
) -> None:
    """A config declaring one registry source and one index source browses
    both — groups aggregate across source kinds."""
    import uuid

    from src.helpers import make_artifact
    from src.registry import REGISTRY_HOST

    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    make_artifact(
        f"{ns}/reg-skill",
        "skill",
        {"reg-skill/SKILL.md": "---\nname: reg-skill\ndescription: from registry\n---\n# S\n"},
        tag="latest",
    )

    root, base = http_index
    _write_all_json(
        root,
        [_package("hub-skill", "skill", "ghcr.io/acme/skills/hub-skill", "From the index")],
    )

    (project_dir / "grimoire.toml").write_text(
        f'[[registries]]\n'
        f'alias = "reg"\n'
        f'oci = "{REGISTRY_HOST}/{ns}"\n'
        f'default = true\n'
        f'\n'
        f'[[registries]]\n'
        f'alias = "hub"\n'
        f'index = "{base}"\n'
        f'\n[skills]\n\n[rules]\n'
    )

    repos = [r.get("repo", "") for r in _search_rows(grim_at(project_dir))]
    assert any("reg-skill" in r for r in repos), f"registry source missing: {repos}"
    assert any("hub-skill" in r for r in repos), f"index source missing: {repos}"


# ---------------------------------------------------------------------------
# Browse filters — dual-candidate matching (S-001 … S-004)
# ---------------------------------------------------------------------------
#
# Every browse-filter pattern is tested against TWO strings: the bare
# repository path (``acme/tools``) and the fully-qualified reference
# (``ghcr.io/acme/tools``). A hit on either counts, so a bare pattern is
# host-agnostic and a host-qualified pattern selects one host.
#
# These live here rather than beside the other filter scenarios in
# ``test_registries.py`` for one reason: they need rows on TWO registry hosts,
# and the session's single OCI registry container cannot serve them. An index
# is a phone book of pointers — its ``ref`` values name hosts that need not
# exist — so the ``http_index`` fixture is the only rig in the tree that can
# express the case at all. The filter itself is source-kind-blind (C-008), so
# nothing about these scenarios is index-specific.


_TWO_HOST_PACKAGES = [
    _package("tools", "skill", "ghcr.io/acme/tools", "Tools on ghcr"),
    _package("tools", "skill", "quay.io/acme/tools", "Tools on quay"),
]


def _filtered_index_config(
    project_dir: Path,
    locator: str,
    include: tuple[str, ...] = (),
    exclude: tuple[str, ...] = (),
) -> None:
    """`_index_config` plus a browse filter on the same single entry."""
    lines = [
        "[[registries]]",
        'alias = "hub"',
        f'index = "{locator}"',
    ]
    for key, patterns in (("include", include), ("exclude", exclude)):
        if patterns:
            lines.append(f"{key} = [" + ", ".join(f'"{p}"' for p in patterns) + "]")
    lines += ["default = true", "", "[skills]", "", "[rules]", ""]
    (project_dir / "grimoire.toml").write_text("\n".join(lines))


@pytest.mark.parametrize(
    ("include", "exclude", "expected"),
    [
        pytest.param(
            ("acme/tools",),
            (),
            {"ghcr.io/acme/tools", "quay.io/acme/tools"},
            id="S-001-bare-pattern-is-host-agnostic",
        ),
        pytest.param(
            ("ghcr.io/acme/tools",),
            (),
            {"ghcr.io/acme/tools"},
            id="S-002-host-qualified-include-selects-one-host",
        ),
        pytest.param(
            ("acme/tools",),
            ("quay.io/acme/tools",),
            {"ghcr.io/acme/tools"},
            id="S-003-host-qualified-exclude-carves-out-one-host",
        ),
        pytest.param(
            (),
            ("quay.io/**",),
            {"ghcr.io/acme/tools"},
            id="S-004-whole-host-excluded",
        ),
    ],
)
def test_browse_filter_addresses_either_candidate(
    grim_at,
    project_dir: Path,
    http_index,
    include: tuple[str, ...],
    exclude: tuple[str, ...],
    expected: set[str],
) -> None:
    """`grim search` shows exactly the rows the two candidates admit.

    All four cases run against one index serving the same repository path on
    two hosts, so the expectations are directly comparable:

    - **S-001** a bare `acme/tools` hits both rows through the bare candidate
      — the regression half, and the reason a pattern survives a host move.
    - **S-002** `ghcr.io/acme/tools` hits one row through the qualified
      candidate. The superseded single-candidate rule matched **neither**
      row with this pattern and browsed empty.
    - **S-003** the discriminating case: the include hits both rows bare, the
      exclude removes one qualified. A naive `matches(bare) || matches(fq)`
      shows both rows here, because the bare-candidate verdict alone is
      "include hit and no exclude hit".
    - **S-004** a whole host excluded, no include list.

    Each case asserts the full visible set, so a filter that silently admits
    everything fails as loudly as one that admits nothing. Exit stays 0
    throughout — a filter is never an error.
    """
    root, base = http_index
    _write_all_json(root, _TWO_HOST_PACKAGES)
    _filtered_index_config(project_dir, base, include=include, exclude=exclude)

    rows = _search_rows(grim_at(project_dir))
    assert {r.get("repo", "") for r in rows} == expected


# ---------------------------------------------------------------------------
# Config surface + validation
# ---------------------------------------------------------------------------


def test_config_registry_add_index_roundtrip(grim_at, project_dir: Path) -> None:
    (project_dir / "grimoire.toml").write_text("[skills]\n\n[rules]\n")
    runner = grim_at(project_dir)

    r = runner.run("config", "registry", "add", "hub", "--index", "https://index.grimoire.rs", check=False)
    assert r.returncode == 0, r.stderr

    r = runner.run("--format", "json", "config", "registry", "show", "hub", check=False)
    assert r.returncode == 0, r.stderr
    shown = json.loads(r.stdout)
    assert shown.get("index") == "https://index.grimoire.rs"
    assert "oci" not in shown or shown["oci"] is None

    r = runner.run("config", "get", "registry.hub.index", check=False)
    assert r.returncode == 0
    assert r.stdout.strip() == "https://index.grimoire.rs"


def test_config_registry_add_requires_exactly_one_source(grim_at, project_dir: Path) -> None:
    (project_dir / "grimoire.toml").write_text("[skills]\n\n[rules]\n")
    runner = grim_at(project_dir)

    # Neither --oci nor --index: usage error 64.
    r = runner.run("config", "registry", "add", "hub", check=False)
    assert r.returncode == 64, f"expected 64, got {r.returncode}: {r.stderr}"

    # Both: rejected at the clap layer (usage error 2 from clap → 64 mapping
    # or clap's own exit; accept any non-zero usage-shaped failure).
    r = runner.run(
        "config", "registry", "add", "hub",
        "--oci", "ghcr.io/acme", "--index", "https://idx", check=False,
    )
    assert r.returncode != 0


def test_config_registry_add_rejects_bad_index_locator(grim_at, project_dir: Path) -> None:
    (project_dir / "grimoire.toml").write_text("[skills]\n\n[rules]\n")
    runner = grim_at(project_dir)
    r = runner.run("config", "registry", "add", "hub", "--index", "ftp://nope", check=False)
    assert r.returncode == 65, f"expected 65, got {r.returncode}: {r.stderr}"


def test_config_set_index_on_oci_entry_rejected(grim_at, project_dir: Path) -> None:
    """oci and index are mutually exclusive — switching source type requires
    an explicit unset first (or rm/add).

    The entry is written with the legacy ``url`` key on purpose: it must
    keep parsing as ``oci`` (serde alias, 0.6.x back-compat).
    """
    (project_dir / "grimoire.toml").write_text(
        '[[registries]]\nalias = "acme"\nurl = "ghcr.io/acme"\n\n[skills]\n\n[rules]\n'
    )
    runner = grim_at(project_dir)
    r = runner.run("config", "set", "registry.acme.index", "https://idx.example", check=False)
    assert r.returncode == 65, f"expected 65, got {r.returncode}: {r.stderr}"

    # Unsetting the only source is refused (the entry would be sourceless).
    r = runner.run("config", "unset", "registry.acme.oci", check=False)
    assert r.returncode == 64, f"expected 64, got {r.returncode}: {r.stderr}"


def test_config_file_with_oci_and_index_rejected(grim_at, project_dir: Path) -> None:
    """A hand-edited entry setting both oci and index fails config parse (78)."""
    (project_dir / "grimoire.toml").write_text(
        '[[registries]]\n'
        'alias = "bad"\n'
        'oci = "ghcr.io/acme"\n'
        'index = "https://idx.example"\n'
        '\n[skills]\n\n[rules]\n'
    )
    runner = grim_at(project_dir)
    r = runner.run("config", "list", check=False)
    assert r.returncode == 78, f"expected 78, got {r.returncode}: {r.stderr}"


# ---------------------------------------------------------------------------
# Install from an index source (global scope, vendor-native destination)
# ---------------------------------------------------------------------------


def test_install_from_http_index_lands_in_claude_config_dir(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str, http_index
) -> None:
    """The whole reason an index exists: a user who knows only its URL can
    add it, find a package through it, and install that package.

    The index is a phone book, so the pointer carries a tagless
    ``registry/repository`` ref and no version — the install has to resolve
    the tag against the real registry. Global scope with
    ``$CLAUDE_CONFIG_DIR`` set is the shape a first-time user actually
    hits, and it is what proves the index never becomes the download
    source: the bytes come from the registry, only the *listing* from the
    index.
    """
    from src.helpers import make_artifact
    from src.runner import GrimRunner

    root, base = http_index
    art = make_artifact(
        f"{unique_repo}/index-installed",
        "skill",
        {
            "index-installed/SKILL.md": (
                "---\nname: index-installed\ndescription: Reached via an index.\n---\n\nBody.\n"
            )
        },
    )
    # Tagless: exactly what `grim publish --announce` writes into a pointer.
    _write_all_json(
        root,
        [
            _package(
                "index-installed",
                "skill",
                f"{registry}/{art.repo}",
                "Reached via an index.",
            )
        ],
    )

    runner = GrimRunner(grim_binary, grim_home)
    config_dir = grim_home.parent / "claude-cfg-index"
    config_dir.mkdir(parents=True, exist_ok=True)
    runner.env["CLAUDE_CONFIG_DIR"] = str(config_dir)

    runner.run("config", "registry", "add", "hub", "--index", base, "--default", "--global")

    rows = json.loads(runner.run("--format", "json", "search", "--refresh", "--global").stdout)["items"]
    repos = [r.get("repo", "") for r in rows]
    assert f"{registry}/{art.repo}" in repos, f"index must list the package; got {repos}"

    # Declare and install in the SAME scope — a project-scope `add` writes a
    # config `install --global` cannot see.
    runner.run("add", "--global", "--no-install", f"{registry}/{art.repo}")
    runner.run("install", "--global", "--client", "claude", format="json")

    skill = config_dir / "skills/index-installed/SKILL.md"
    assert skill.is_file(), (
        f"a package found through an index must install like any other; "
        f"nothing at {skill}"
    )
    assert "name: index-installed" in skill.read_text()
    # The vendor override is honoured, so nothing leaked into the default tree.
    assert not (runner.home / ".claude/skills/index-installed").exists(), (
        "global skill must NOT land in default ~/.claude when CLAUDE_CONFIG_DIR is set"
    )


# ---------------------------------------------------------------------------
# `stats.json` ratings sidecar (C-003, S-001, S-002)
# ---------------------------------------------------------------------------


def _write_stats(root: Path, entries: dict, schema_version: int = 1) -> None:
    (root / "stats.json").write_text(
        json.dumps(
            {
                "schema_version": schema_version,
                "generated_at": "2026-08-18T00:00:00Z",
                "providers": {"rating": "github"},
                "entries": entries,
            }
        )
    )


def _cached_entries(grim_home: Path) -> dict:
    """Every cached catalog entry across the per-registry cache files, keyed
    by ref — the read path's only observable surface until the display
    packages land."""
    entries: dict = {}
    for cache in (grim_home / "catalog").glob("*.json"):
        entries.update(json.loads(cache.read_text()).get("entries", {}))
    assert entries, f"no catalog cache was written under {grim_home / 'catalog'}"
    return entries


def test_http_index_joins_the_ratings_sidecar_by_ref(
    grim_at, grim_home: Path, project_dir: Path, http_index
) -> None:
    """S-001: an index publishing ``stats.json`` beside ``all.json`` has its
    ratings joined onto the catalog **by ref** — the rated pointer only."""
    root, base = http_index
    _write_all_json(
        root,
        [
            _package("rated", "skill", "ghcr.io/acme/skills/rated", "Rated"),
            _package("unrated", "skill", "ghcr.io/acme/skills/unrated", "Unrated"),
        ],
    )
    _write_stats(
        root,
        {
            "ghcr.io/acme/skills/rated": {
                "rating": {
                    "up": 12,
                    "target": "D_kwDOAbCdEf",
                    "url": "https://github.com/acme/index/discussions/7",
                }
            },
            # A ref the index does not list at all: joined onto nothing,
            # never invented as a row.
            "ghcr.io/acme/skills/ghost": {"rating": {"up": 99, "target": "t", "url": "u"}},
        },
    )
    _index_config(project_dir, base)

    rows = _search_rows(grim_at(project_dir))
    assert len(rows) == 2, f"both pointers still list; got {rows}"

    cached = _cached_entries(grim_home)
    rating = cached["ghcr.io/acme/skills/rated"]["rating"]
    assert rating["up"] == 12
    # Opaque: stored verbatim, never parsed or reconstructed by grim.
    assert rating["target"] == "D_kwDOAbCdEf"
    assert rating["url"] == "https://github.com/acme/index/discussions/7"
    assert "rating" not in cached["ghcr.io/acme/skills/unrated"], (
        "a ref absent from the sidecar is unrated, never a zero-vote record"
    )
    assert "ghcr.io/acme/skills/ghost" not in cached, (
        "the sidecar joins onto index rows; it never adds one"
    )


def test_http_index_without_a_sidecar_reads_unrated_and_warns_nothing(
    grim_at, grim_home: Path, project_dir: Path, http_index
) -> None:
    """S-002: no ``stats.json`` (404) is the normal case for every index that
    has not enabled ratings — the catalog builds, rows are unrated, and
    nothing is said above ``debug``."""
    root, base = http_index
    _write_all_json(root, [_package("plain", "skill", "ghcr.io/acme/skills/plain", "Plain")])
    _index_config(project_dir, base)

    runner = grim_at(project_dir)
    result = runner.run("--format", "json", "search", "--refresh", check=False)
    assert result.returncode == 0, f"a missing sidecar must not fail the build: {result.stderr}"
    assert len(json.loads(result.stdout)["items"]) == 1
    assert "rating" not in _cached_entries(grim_home)["ghcr.io/acme/skills/plain"]
    # Default filter is `warn`, so anything logged above `debug` shows here.
    assert "stats.json" not in result.stderr, f"404 is never surfaced: {result.stderr}"


def test_http_index_sidecar_server_error_reads_unrated(
    grim_at, grim_home: Path, project_dir: Path, tmp_path: Path
) -> None:
    """C-003: only the ``all.json`` fetch decides whether the build
    succeeded. A sidecar the server fails on degrades to *no ratings* — the
    same arm an outright transport fault takes."""

    class FailingStats(SimpleHTTPRequestHandler):
        def do_GET(self):  # noqa: N802 — http.server's own casing
            if self.path.endswith("stats.json"):
                self.send_error(500)
                return
            super().do_GET()

    root = tmp_path / "index-dist-500"
    root.mkdir()
    _write_all_json(root, [_package("plain", "skill", "ghcr.io/acme/skills/plain", "Plain")])
    handler = partial(FailingStats, directory=str(root))
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        _index_config(project_dir, f"http://127.0.0.1:{server.server_address[1]}")
        runner = grim_at(project_dir)
        result = runner.run("--format", "json", "search", "--refresh", check=False)
        assert result.returncode == 0, f"a failing sidecar must not fail the build: {result.stderr}"
        assert len(json.loads(result.stdout)["items"]) == 1
        assert "rating" not in _cached_entries(grim_home)["ghcr.io/acme/skills/plain"]
    finally:
        server.shutdown()


def test_sidecar_503_leaves_a_warm_cache_rated(
    grim_at, grim_home: Path, project_dir: Path, tmp_path: Path
) -> None:
    """F-1 / R-2: a sidecar the server fails on was *not observed* — it is not
    an observation that nothing is rated. One 503 with ``all.json`` still
    serving must not publish "nothing is rated" into the cache for a whole
    TTL, so the previous build's rating carries forward."""

    failing = threading.Event()

    class FlakyStats(SimpleHTTPRequestHandler):
        def do_GET(self):  # noqa: N802 — http.server's own casing
            if self.path.endswith("stats.json") and failing.is_set():
                self.send_error(503)
                return
            super().do_GET()

    root = tmp_path / "index-dist-flaky"
    root.mkdir()
    _write_all_json(root, [_package("rated", "skill", "ghcr.io/acme/skills/rated", "Rated")])
    _write_stats(
        root,
        {"ghcr.io/acme/skills/rated": {"rating": {"up": 12, "target": "t", "url": "u"}}},
    )
    handler = partial(FlakyStats, directory=str(root))
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        _index_config(project_dir, f"http://127.0.0.1:{server.server_address[1]}")
        runner = grim_at(project_dir)

        # A warm cache: one successful build that saw the rating.
        _search_rows(runner)
        assert _cached_entries(grim_home)["ghcr.io/acme/skills/rated"]["rating"]["up"] == 12

        # The sidecar starts failing; `all.json` keeps serving, so the
        # catalog still rebuilds — over a rating nothing observed.
        failing.set()
        rows = _search_rows(runner)
        rating = _cached_entries(grim_home)["ghcr.io/acme/skills/rated"].get("rating")
        assert rating is not None, (
            "a 503 on the sidecar is an unobserved rating, not an observation "
            "of none — the warm cache's rating must survive the rebuild"
        )
        assert rating["up"] == 12
        # And the shipped surface, not just the cache behind it: the JSON row
        # is the contract consumers read.
        row = next(r for r in rows if r["repo"] == "ghcr.io/acme/skills/rated")
        assert row["rating"]["up"] == 12, f"the served row keeps the rating too; got {row['rating']}"
    finally:
        server.shutdown()


def test_git_index_reads_unrated(grim_at, grim_home: Path, project_dir: Path, tmp_path: Path) -> None:
    """C-003: ratings ride the HTTP index only. A git-transport index is a
    tree of ``metadata.json`` files with no sidecar — unrated, not an
    error."""
    repo = _git_index_repo(
        tmp_path, [_package("gitpkg", "skill", "ghcr.io/acme/skills/gitpkg", "From git")]
    )
    _index_config(project_dir, repo.as_posix())

    rows = _search_rows(grim_at(project_dir))
    assert [r.get("repo") for r in rows] == ["ghcr.io/acme/skills/gitpkg"]
    assert "rating" not in _cached_entries(grim_home)["ghcr.io/acme/skills/gitpkg"]


def test_offline_browse_is_unchanged_by_the_sidecar(
    grim_at, grim_home: Path, project_dir: Path, http_index
) -> None:
    """C-003: ``GRIM_OFFLINE`` inherits catalog behaviour verbatim — the
    cached rating is served, and no sidecar request is made because no
    rebuild happens at all."""
    root, base = http_index
    _write_all_json(root, [_package("rated", "skill", "ghcr.io/acme/skills/rated", "Rated")])
    _write_stats(root, {"ghcr.io/acme/skills/rated": {"rating": {"up": 3, "target": "t", "url": "u"}}})
    _index_config(project_dir, base)

    runner = grim_at(project_dir)
    _search_rows(runner)
    assert _cached_entries(grim_home)["ghcr.io/acme/skills/rated"]["rating"]["up"] == 3

    # Take the index away entirely: offline must not reach for it.
    (root / "all.json").unlink()
    (root / "stats.json").unlink()
    runner.env["GRIM_OFFLINE"] = "1"
    result = runner.run("--format", "json", "search", check=False)
    assert result.returncode == 0, f"offline serves the cache: {result.stderr}"
    assert [r.get("repo") for r in json.loads(result.stdout)["items"]] == [
        "ghcr.io/acme/skills/rated"
    ]
    assert _cached_entries(grim_home)["ghcr.io/acme/skills/rated"]["rating"]["up"] == 3


# ---------------------------------------------------------------------------
# `--sort` browse ordering (C-017, S-010, S-011)
# ---------------------------------------------------------------------------


def _sorted_index(root: Path, project_dir: Path, base: str) -> None:
    """A four-package index whose ratings, names, and relevance to the query
    ``tool`` all disagree — so any single ordering the CLI applies is
    distinguishable from the other two."""
    _write_all_json(
        root,
        [
            _package(name, "skill", f"ghcr.io/acme/skills/{name}", desc)
            for name, desc in [
                # Scores highest on `tool`: the term appears in the leaf name
                # AND the description.
                ("tool-tool", "tool tool tool"),
                ("apex", "a tool"),
                ("Zulu", "a tool"),
                ("unrated", "a tool"),
            ]
        ],
    )
    _write_stats(
        root,
        {
            "ghcr.io/acme/skills/apex": {"rating": {"up": 7, "target": "t1", "url": "u1"}},
            "ghcr.io/acme/skills/Zulu": {"rating": {"up": 40, "target": "t2", "url": "u2"}},
            "ghcr.io/acme/skills/tool-tool": {"rating": {"up": 1, "target": "t3", "url": "u3"}},
            # `unrated` is deliberately absent from the sidecar.
        },
    )
    _index_config(project_dir, base)


def _repos(runner, *args: str) -> list[str]:
    result = runner.run("--format", "json", "search", "--refresh", *args, check=False)
    assert result.returncode == 0, f"search must exit 0; stderr: {result.stderr}"
    return [r["repo"].rsplit("/", 1)[-1] for r in json.loads(result.stdout)["items"]]


def test_sort_rating_orders_the_browse_with_unrated_last(
    grim_at, project_dir: Path, http_index
) -> None:
    """S-010: ``--sort rating`` orders the whole browse by upvotes descending
    with the unrated artifact in a bucket of its own at the end — never folded
    to zero, which would have interleaved it with the low-rated rows."""
    root, base = http_index
    _sorted_index(root, project_dir, base)
    assert _repos(grim_at(project_dir), "--sort", "rating") == [
        "Zulu",  # 40
        "apex",  # 7
        "tool-tool",  # 1
        "unrated",  # no rating record at all
    ]


def test_sort_name_is_case_insensitive_ascending(grim_at, project_dir: Path, http_index) -> None:
    """C-017: ``--sort name`` is ascending and case-insensitive, so ``Zulu``
    sorts by its letters rather than ahead of every lowercase name on its
    capital (a byte-wise sort would put it first)."""
    root, base = http_index
    _sorted_index(root, project_dir, base)
    assert _repos(grim_at(project_dir), "--sort", "name") == ["apex", "tool-tool", "unrated", "Zulu"]


def test_sort_updated_is_deterministic_when_every_row_is_undated(
    grim_at, project_dir: Path, http_index
) -> None:
    """C-017: an index pointer carries no ``created`` at all, so every row
    falls into the undated bucket together — and the order must still be
    total, resolved by the name tiebreak rather than left to the sort
    implementation."""
    root, base = http_index
    _sorted_index(root, project_dir, base)
    runner = grim_at(project_dir)
    first = _repos(runner, "--sort", "updated")
    assert first == ["apex", "tool-tool", "unrated", "Zulu"]
    assert _repos(runner, "--sort", "updated") == first, "an undated browse is stable across runs"


def test_sort_overrides_relevance_on_a_query(grim_at, project_dir: Path, http_index) -> None:
    """S-011: with a query present ``--sort`` replaces relevance ranking.
    ``tool-tool`` is the strongest match for ``tool`` and heads the unsorted
    query; under ``--sort rating`` it drops to its rating position instead."""
    root, base = http_index
    _sorted_index(root, project_dir, base)
    runner = grim_at(project_dir)

    by_relevance = _repos(runner, "tool")
    assert by_relevance[0] == "tool-tool", f"relevance puts the best match first; got {by_relevance}"

    assert _repos(runner, "tool", "--sort", "rating") == ["Zulu", "apex", "tool-tool", "unrated"], (
        "--sort must replace the relevance ranking, not compose with it"
    )


def test_absent_sort_leaves_the_browse_exactly_as_it_was(
    grim_at, project_dir: Path, http_index
) -> None:
    """Principle 9: ``--sort`` is a new optional flag, so a run without it
    must be byte-identical to what it was before the flag existed — the
    unqueried browse in index order, the queried one by relevance."""
    root, base = http_index
    _sorted_index(root, project_dir, base)
    runner = grim_at(project_dir)

    # Unqueried: unchanged registry-group order — rows byte-sorted by
    # repository path within the group, which is why the capitalised `Zulu`
    # leads. That is *not* what `--sort name` produces (case-insensitive, so
    # `Zulu` lands last there), which is what makes this a real baseline
    # rather than a restatement of the new comparator.
    assert _repos(runner) == ["Zulu", "apex", "tool-tool", "unrated"]

    # Queried: relevance descending, with the strongest match first.
    assert _repos(runner, "tool")[0] == "tool-tool"
