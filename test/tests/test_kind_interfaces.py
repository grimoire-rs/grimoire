# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Kind-by-interface acceptance matrix: every artifact kind against every interface.

One module-level list, ``KINDS``, mirrors ``ArtifactKind::ALL``; a test pins it to
the binary's own list. Each interface (column) is one test function
parametrized over ``KINDS``, reading a per-kind table. A kind absent from a
table raises ``KeyError``, so a new kind cannot pass this suite until every
column says what the kind does there. Nothing skips.

Columns: publish annotations, registry search, index search, describe, fetch
document, fetch file list, fetch one path, status, install report, uninstall
report, add and remove, MCP tools.

Uncovered by design: nothing here starts a real AI client, so "the client loaded
the artifact" is out of reach; the cells assert what grim reports.
"""

from __future__ import annotations

import json
import re
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from http.server import SimpleHTTPRequestHandler
from pathlib import Path
from uuid import uuid4

import pytest
import tomllib

from src.helpers import make_artifact, mod_sandbox, settings_sandbox
from src.index_fixture import (
    IndexHandler,
    conformance_tree,
    index_config,
    package,
    search_envelope,
    serve,
    write_all_json,
)
from src.registry import REGISTRY_HOST
from src.runner import GrimRunner
from src.stdio_drivers import drive_until_answered, initialize

KINDS = ["skill", "rule", "agent", "bundle", "mcp", "hook", "settings", "mod"]

SUMMARY = "Sum"
KEYWORDS = "kw1,kw2"
CLIENTS = ["claude", "opencode"]
TRUST = "--trust-workspace"
EXPERIMENTAL = "[options.experimental]\nhooks = true\nsettings = true\nmods = true\n"

# --- per-kind tables (every column indexes these; a missing kind is a KeyError) ---

# Catalog description each kind publishes (a rule derives it from its heading).
DESCRIPTION = {
    "skill": "A skill.",
    "rule": "Rule title",
    "agent": "An agent.",
    "bundle": "A bundle",
    "mcp": "An mcp",
    "hook": "A hook",
    "settings": "A settings",
    "mod": "A mod",
}

# Whether the kind authors summary + keywords. A hook has neither, by intent
# (publishing.md, hook metadata note).
CATALOG_METADATA = {
    "skill": True,
    "rule": True,
    "agent": True,
    "bundle": True,
    "mcp": True,
    "hook": False,
    "settings": True,
    "mod": True,
}

# `serves` on describe, search and status: the harness list for a mod, null otherwise.
SERVES = {
    "skill": None,
    "rule": None,
    "agent": None,
    "bundle": None,
    "mcp": None,
    "hook": None,
    "settings": None,
    "mod": CLIENTS,
}

# Extra manifest annotations beyond the common set.
EXTRA_ANNOTATIONS = {
    "skill": {},
    "rule": {},
    "agent": {},
    "bundle": {},
    "mcp": {},
    "hook": {},
    "settings": {},
    "mod": {"com.grimoire.mod.clients": "claude,opencode"},
}

# The fetch document, as a substring the kind's index document must carry.
FETCH_CONTENT = {
    "skill": "name: kx-skill",
    "rule": "# Rule title",
    "agent": "name: kx-agent",
    "bundle": '"members"',
    "mcp": '"server"',
    "hook": 'name = "kx-hook"',
    "settings": '"model": "sonnet"',
    "mod": 'name = "kx-mod"',
}

# The fetch file list (path -> unchanged check is by presence); None = the kind
# is a single layer document and the report carries no `files` key.
FETCH_FILES = {
    "skill": ["kx-skill/SKILL.md"],
    "rule": ["kx-rule.md"],
    "agent": ["kx-agent.md"],
    "bundle": None,
    "mcp": None,
    "hook": ["kx-hook/handler.sh", "kx-hook/hook.toml"],
    "settings": None,
    "mod": ["kx-mod/claude/index.js", "kx-mod/mod.toml", "kx-mod/opencode/index.ts"],
}

# `fetch --path <p>`: (path, expected content) for tar kinds, or the refusal.
FETCH_PATH = {
    "skill": ("kx-skill/SKILL.md", "name: kx-skill"),
    "rule": ("kx-rule.md", "# Rule title"),
    "agent": ("kx-agent.md", "name: kx-agent"),
    "bundle": ("x", "bundles carry no support files; omit 'path'"),
    "mcp": ("x", "mcp descriptors carry no support files; omit 'path'"),
    "hook": ("kx-hook/handler.sh", "exit 0"),
    "settings": ("x", "settings carry no support files; omit 'path'"),
    "mod": ("kx-mod/claude/index.js", "export default {};"),
}
FETCH_PATH_REFUSED = {"bundle", "mcp", "settings"}

# The `[table]` a kind is declared under in grimoire.toml.
TABLE = {
    "skill": "skills",
    "rule": "rules",
    "agent": "agents",
    "bundle": "bundles",
    "mcp": "mcp",
    "hook": "hooks",
    "settings": "settings",
    "mod": "mods",
}

# What a fresh `grim install` reports for a one-kind project: the (kind, status)
# of every item. A bundle reports its members, never itself.
INSTALL_ITEMS = {
    "skill": [("skill", "installed")],
    "rule": [("rule", "installed")],
    "agent": [("agent", "installed")],
    "bundle": [("skill", "installed")],
    "mcp": [("mcp", "installed")],
    "hook": [("hook", "installed")],
    "settings": [("settings", "installed")],
    "mod": [("mod", "installed")],
}

# `grim status` rows after the install: (kind, state) of the declared item.
STATUS_STATE = {
    "skill": "installed",
    "rule": "installed",
    "agent": "installed",
    "bundle": "installed",
    "mcp": "installed",
    "hook": "installed",
    "settings": "installed",
    "mod": "installed",
}

# `grim uninstall <kind> <name>`: exit code and message substring. A bundle is
# not an `uninstall` kind (clap refuses it); every other kind uninstalls.
UNINSTALL = {
    "skill": (0, None),
    "rule": (0, None),
    "agent": (0, None),
    "bundle": (64, "invalid value 'bundle'"),
    "mcp": (0, None),
    "hook": (0, None),
    "settings": (0, None),
    "mod": (0, None),
}


def name_of(kind: str) -> str:
    return f"kx-{kind}"


# --- the world: every kind published once for the module ---


@dataclass
class World:
    repo: str
    base: Path
    refs: dict[str, str]  # kind -> fully-qualified ref at 1.0.0
    registry_cfg: str  # `[[registries]]` entry on the session registry
    index_base: str  # URL of the served all.json

    def ref(self, kind: str) -> str:
        return self.refs[kind]

    def runner(self, cwd: Path, home_name: str) -> GrimRunner:
        runner = GrimRunner(_BINARY[0], self.base / home_name, cwd=cwd)
        runner.env.update(
            settings_sandbox(self.base / f"sb-{home_name}", runner.home)[0]
        )
        runner.env.update(mod_sandbox(self.base / f"sb-{home_name}")[0])
        return runner


_BINARY: list[Path] = []


def _write(path: Path, body: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding="utf-8", newline="\n")
    return path


def _sources(src: Path, member_ref: str) -> dict[str, Path]:
    """The author-side source tree for each kind, summary + keywords where supported."""
    meta = f'summary = "{SUMMARY}"\nkeywords = "{KEYWORDS}"\n'
    fm = f"  summary: {SUMMARY}\n  keywords: {KEYWORDS}\n"
    _write(
        src / "kx-skill/SKILL.md",
        f"---\nname: kx-skill\ndescription: A skill.\nmetadata:\n{fm}---\n# s\n",
    )
    _write(
        src / "kx-rule.md",
        f"---\nsummary: {SUMMARY}\nkeywords: {KEYWORDS}\n---\n# Rule title\nbody\n",
    )
    _write(
        src / "kx-agent.md",
        f"---\nname: kx-agent\ndescription: An agent.\nmetadata:\n{fm}---\nbody\n",
    )
    _write(
        src / "kx-bundle.toml",
        f'{meta}description = "A bundle"\n[skills]\nkx-member = "{member_ref}"\n',
    )
    _write(
        src / "kx-mcp.toml",
        f'description = "An mcp"\n{meta}\n[server]\ntransport = "stdio"\ncommand = "grim"\n',
    )
    _write(
        src / "kx-hook/hook.toml",
        'schema = 1\nname = "kx-hook"\ndescription = "A hook"\n\n[[hooks]]\n'
        'id = "h"\nevent = "PreToolUse"\ntier = "gatekeeper"\nmatcher = "Bash"\n'
        'argv = ["sh", "${GRIM_HOOK_DIR}/handler.sh"]\ntimeout = 30\npayload = "stdin"\n',
    )
    _write(src / "kx-hook/handler.sh", "#!/bin/sh\nexit 0\n")
    _write(
        src / "kx-settings.toml",
        f'description = "A settings"\n{meta}\n[claude]\nmodel = "sonnet"\n',
    )
    _write(
        src / "kx-mod/mod.toml",
        f'schema = 1\nname = "kx-mod"\ndescription = "A mod"\n{meta}\n'
        '[claude]\nentry = "index.js"\n\n[opencode]\nentry = "index.ts"\n',
    )
    _write(src / "kx-mod/claude/index.js", "export default {};\n")
    _write(src / "kx-mod/opencode/index.ts", "export default {};\n")
    return {
        "skill": src / "kx-skill",
        "rule": src / "kx-rule.md",
        "agent": src / "kx-agent.md",
        "bundle": src / "kx-bundle.toml",
        "mcp": src / "kx-mcp.toml",
        "hook": src / "kx-hook",
        "settings": src / "kx-settings.toml",
        "mod": src / "kx-mod",
    }


def _index_pointers(refs: dict[str, str]) -> list[dict]:
    pointers = []
    for kind in KINDS:
        meta = (
            {"summary": SUMMARY, "keywords": KEYWORDS.split(",")}
            if CATALOG_METADATA[kind]
            else {}
        )
        pointer = package(
            name_of(kind),
            kind,
            refs[kind].rsplit(":", 1)[0],
            DESCRIPTION[kind],
            **meta,
        )
        if SERVES[kind] is not None:
            pointer["serves"] = SERVES[kind]
        pointers.append(pointer)
    # `replacedBy` (camelCase) is accepted beside `replaced_by`; the snake row is the control.
    for name, key in (("camel", "replacedBy"), ("snake", "replaced_by")):
        pointer = package(
            f"kx-{name}", "skill", f"ghcr.io/acme/kx-{name}", "Old", deprecated="gone"
        )
        pointer[key] = f"ghcr.io/acme/successor-{name}"
        pointers.append(pointer)
    both = package("kx-both", "skill", "ghcr.io/acme/kx-both", "Old", deprecated="gone")
    both["replaced_by"] = "ghcr.io/acme/successor-snake"
    both["replacedBy"] = "ghcr.io/acme/successor-camel"
    pointers.append(both)
    return pointers


@pytest.fixture(scope="module")
def world(
    grim_binary: Path, registry: str, tmp_path_factory: pytest.TempPathFactory
) -> Iterator[World]:
    _BINARY[:] = [grim_binary]
    base = tmp_path_factory.mktemp("kinds")
    repo = f"grim-test/{uuid4().hex[:12]}"
    (base / "pub" / ".claude").mkdir(parents=True)
    member = make_artifact(
        f"{repo}/kx-member",
        "skill",
        {"kx-member/SKILL.md": "---\nname: kx-member\ndescription: m\n---\n# m\n"},
        tag="1.0.0",
    )
    registry_cfg = (
        f'[[registries]]\nalias = "r"\noci = "{REGISTRY_HOST}/{repo}"\ndefault = true\n'
    )
    _write(base / "pub" / "grimoire.toml", registry_cfg + "[skills]\n[rules]\n")
    publisher = GrimRunner(grim_binary, base / "pub-home", cwd=base / "pub")
    sources = _sources(base / "src", member.fq)
    refs = {kind: f"{REGISTRY_HOST}/{repo}/{name_of(kind)}:1.0.0" for kind in KINDS}
    for kind in KINDS:
        publisher.json("release", str(sources[kind]), refs[kind], "--kind", kind)

    index_root = base / "index-dist"
    index_root.mkdir()
    write_all_json(index_root, _index_pointers(refs))
    with serve(index_root, SimpleHTTPRequestHandler) as index_base:
        yield World(repo, base, refs, registry_cfg, index_base)


@pytest.fixture(scope="module")
def reader(world: World) -> GrimRunner:
    """A runner in a workspace whose only registry is the session registry's namespace."""
    ws = world.base / "reader"
    (ws / ".claude").mkdir(parents=True)
    _write(ws / "grimoire.toml", world.registry_cfg + "[skills]\n[rules]\n")
    return world.runner(ws, "reader-home")


def _by_kind(rows: list[dict]) -> dict[str, dict]:
    """Search rows keyed by the kind encoded in the `kx-<kind>` repo name."""
    out: dict[str, dict] = {}
    for row in rows:
        match = re.search(r"/kx-([a-z]+)$", row["repo"])
        if match:
            out[match.group(1)] = row
    return out


# --- a project per kind, declared, locked and installed ---


def _project_config(kind: str, world: World) -> str:
    return (
        f"{world.registry_cfg}\n[{TABLE[kind]}]\n"
        f'{name_of(kind)} = "{world.ref(kind)}"\n\n{EXPERIMENTAL}'
    )


@pytest.fixture()
def project(
    world: World, tmp_path: Path
) -> Callable[[str, bool], tuple[GrimRunner, Path, dict]]:
    """Factory ``(kind, install) -> (runner, dir, install report)``."""

    def build(kind: str, install: bool = True) -> tuple[GrimRunner, Path, dict]:
        ws = tmp_path / f"ws-{kind}"
        (ws / ".claude").mkdir(parents=True)
        _write(ws / "grimoire.toml", _project_config(kind, world))
        runner = world.runner(ws, f"home-{kind}-{uuid4().hex[:6]}")
        runner.run("lock")
        # --trust-workspace answers for one run only; the record is what `status` reads.
        runner.run("consent", "allow")
        report = runner.json("install", TRUST) if install else {}
        return runner, ws, report

    return build


# =========================== the tests ===========================


def test_kinds_list_matches_the_binary(grim_binary: Path, grim: GrimRunner) -> None:
    help_text = grim.run("search", "--help").stdout
    match = re.search(r"\[possible values: ([a-z, ]+)\]", help_text)
    assert match, help_text
    assert KINDS == match.group(1).split(", ")


@pytest.mark.parametrize("kind", KINDS)
def test_publish_annotations(kind: str, world: World, reader: GrimRunner) -> None:
    ann = reader.json("describe", world.ref(kind))["annotations"]
    assert ann["com.grimoire.kind"] == kind
    assert ann["org.opencontainers.image.title"] == name_of(kind)
    assert ann["org.opencontainers.image.description"] == DESCRIPTION[kind]
    if CATALOG_METADATA[kind]:
        assert ann["com.grimoire.summary"] == SUMMARY
        assert ann["com.grimoire.keywords"] == KEYWORDS
    else:
        assert "com.grimoire.summary" not in ann
        assert "com.grimoire.keywords" not in ann
    for key, value in EXTRA_ANNOTATIONS[kind].items():
        assert ann[key] == value


def _assert_search_row(kind: str, row: dict, keyword_hits: set[str]) -> None:
    assert row["kind"] == kind
    assert row["description"] == DESCRIPTION[kind]
    if CATALOG_METADATA[kind]:
        assert row["summary"] == SUMMARY
        assert kind in keyword_hits, f"keyword query did not find the {kind} row"
    else:
        assert row["summary"] is None
        assert kind not in keyword_hits
    assert row["serves"] == SERVES[kind]


@pytest.fixture(scope="module")
def registry_search(reader: GrimRunner) -> tuple[dict[str, dict], set[str]]:
    rows = _by_kind(reader.json("search", "--refresh")["items"])
    hits = _by_kind(reader.json("search", "kw1", "--refresh")["items"])
    return rows, set(hits)


@pytest.mark.parametrize("kind", KINDS)
def test_search_from_a_registry(
    kind: str, registry_search: tuple[dict[str, dict], set[str]]
) -> None:
    rows, hits = registry_search
    assert kind in rows, f"{kind} missing from registry search: {sorted(rows)}"
    _assert_search_row(kind, rows[kind], hits)


@pytest.fixture(scope="module")
def index_search(world: World) -> tuple[dict[str, dict], set[str]]:
    ws = world.base / "index-ws"
    (ws / ".claude").mkdir(parents=True)
    _write(
        ws / "grimoire.toml",
        f'[[registries]]\nalias = "hub"\nindex = "{world.index_base}"\ndefault = true\n\n[skills]\n\n[rules]\n',
    )
    runner = world.runner(ws, "index-home")
    rows = _by_kind(search_envelope(runner)["items"])
    hits = _by_kind(search_envelope(runner, "kw1")["items"])
    return rows, set(hits)


@pytest.mark.parametrize("kind", KINDS)
def test_search_from_an_index(
    kind: str, index_search: tuple[dict[str, dict], set[str]]
) -> None:
    rows, hits = index_search
    assert kind in rows, f"{kind} missing from index search: {sorted(rows)}"
    _assert_search_row(kind, rows[kind], hits)


@pytest.fixture(scope="module")
def indexer_tree_rows(
    grim_binary: Path, tmp_path_factory: pytest.TempPathFactory
) -> Iterator[list[dict]]:
    """Search rows read from the checked-in indexer tree, no registry needed."""
    base = tmp_path_factory.mktemp("indexer-tree")
    root = conformance_tree(base / "dist", "c-plus-legacy")
    project, home = base / "project", base / "home"
    project.mkdir()
    home.mkdir()
    with serve(root, IndexHandler.new()) as url:
        index_config(project, url)
        runner = GrimRunner(grim_binary, home, cwd=project)
        yield search_envelope(runner, "--show-deprecated")["items"]


@pytest.mark.parametrize("kind", KINDS)
def test_search_from_the_indexer_tree(kind: str, indexer_tree_rows: list[dict]) -> None:
    # Unlike the hand-built index column, these records come from the indexer.
    rows = [row for row in indexer_tree_rows if row["kind"] == kind]
    assert rows, f"no {kind} row in the indexer tree"
    if kind == "mod":
        by_repo = {row["repo"]: row for row in rows}
        assert by_repo["ghcr.io/acme/mod-sample"]["serves"] == CLIENTS
    else:
        assert all(row["serves"] is None for row in rows)


@pytest.mark.usefixtures("index_search")
def test_index_accepts_the_camel_case_replaced_by(world: World) -> None:
    ws = world.base / "index-ws"
    runner = world.runner(ws, "index-home")
    rows = _by_kind(search_envelope(runner, "--show-deprecated")["items"])
    assert rows["camel"]["replaced_by"] == "ghcr.io/acme/successor-camel"
    assert rows["snake"]["replaced_by"] == "ghcr.io/acme/successor-snake"


@pytest.mark.usefixtures("index_search")
def test_index_keeps_a_record_with_both_replaced_by_spellings(world: World) -> None:
    ws = world.base / "index-ws"
    runner = world.runner(ws, "index-home")
    rows = _by_kind(search_envelope(runner, "--show-deprecated")["items"])
    assert rows["both"]["replaced_by"] == "ghcr.io/acme/successor-snake"


@pytest.mark.parametrize("kind", KINDS)
def test_describe(kind: str, world: World, reader: GrimRunner) -> None:
    doc = reader.json("describe", world.ref(kind))
    assert doc["kind"] == kind
    assert doc["name"] == name_of(kind)
    assert doc["serves"] == SERVES[kind]
    assert isinstance(doc["has_description"], bool)
    assert doc["description"] == DESCRIPTION[kind]


@pytest.mark.parametrize("kind", KINDS)
def test_fetch_document(kind: str, world: World, reader: GrimRunner) -> None:
    doc = reader.json("fetch", world.ref(kind))
    assert doc["kind"] == kind
    assert doc["name"] == name_of(kind)
    assert doc["content"], "empty fetch content"
    assert FETCH_CONTENT[kind] in doc["content"]


@pytest.mark.parametrize("kind", KINDS)
def test_fetch_file_list(kind: str, world: World, reader: GrimRunner) -> None:
    doc = reader.json("fetch", world.ref(kind))
    expected = FETCH_FILES[kind]
    if expected is None:
        assert not doc.get("files"), doc.get("files")
        return
    assert [f["path"] for f in doc["files"]] == expected
    assert all(f["size"] > 0 for f in doc["files"])


@pytest.mark.parametrize("kind", KINDS)
def test_fetch_one_file(kind: str, world: World, reader: GrimRunner) -> None:
    path, expected = FETCH_PATH[kind]
    result = reader.run(
        "--format", "json", "fetch", world.ref(kind), "--path", path, check=False
    )
    if kind in FETCH_PATH_REFUSED:
        assert result.returncode == 1, result.stdout
        assert expected in result.stderr
        return
    assert result.returncode == 0, result.stderr
    assert expected in json.loads(result.stdout)["content"]


@pytest.mark.parametrize("kind", KINDS)
def test_status_after_install(kind: str, project) -> None:
    runner, _, _ = project(kind)
    rows = [r for r in runner.json("status")["items"] if r["name"] == name_of(kind)]
    assert len(rows) == 1, rows
    row = rows[0]
    assert row["kind"] == kind
    assert row["state"] == STATUS_STATE[kind]
    assert row["serves"] == SERVES[kind]


@pytest.mark.parametrize("kind", KINDS)
def test_install_report(kind: str, project) -> None:
    _, _, report = project(kind)
    got = [(item["kind"], item["status"]) for item in report["items"]]
    assert got == INSTALL_ITEMS[kind]
    # json-interface.md: `target` is null only when every client declined (nothing written).
    assert all(item["target"] for item in report["items"]), report["items"]
    assert "settings_changes" in report


@pytest.mark.parametrize("kind", KINDS)
def test_uninstall_report(kind: str, project) -> None:
    runner, ws, _ = project(kind)
    code, message = UNINSTALL[kind]
    result = runner.run(
        "--format", "json", "uninstall", kind, name_of(kind), check=False
    )
    assert result.returncode == code, result.stderr
    if message is not None:
        assert message in result.stderr
        return
    report = json.loads(result.stdout)
    assert report["kind"] == kind
    assert report["name"] == name_of(kind)
    assert report["status"] == "uninstalled"
    config = tomllib.loads((ws / "grimoire.toml").read_text(encoding="utf-8"))
    assert name_of(kind) not in config.get(TABLE[kind], {})


@pytest.mark.parametrize("kind", KINDS)
def test_add_and_remove(kind: str, world: World, tmp_path: Path) -> None:
    ws = tmp_path / "add-ws"
    (ws / ".claude").mkdir(parents=True)
    _write(
        ws / "grimoire.toml",
        f"{world.registry_cfg}\n[skills]\n[rules]\n\n{EXPERIMENTAL}",
    )
    runner = world.runner(ws, f"add-home-{kind}")

    added = runner.json("add", "--kind", kind, world.ref(kind), TRUST)
    assert added["kind"] == kind
    assert added["name"] == name_of(kind)
    assert added["status"] == "added"
    config = tomllib.loads((ws / "grimoire.toml").read_text(encoding="utf-8"))
    assert config[TABLE[kind]][name_of(kind)] == world.ref(kind)

    removed = runner.json("remove", kind, name_of(kind))
    assert removed["kind"] == kind
    assert removed["status"] == "removed"
    config = tomllib.loads((ws / "grimoire.toml").read_text(encoding="utf-8"))
    assert name_of(kind) not in config.get(TABLE[kind], {})


def _call(idx: int, tool: str, arguments: dict) -> dict:
    return {
        "jsonrpc": "2.0",
        "id": idx,
        "method": "tools/call",
        "params": {"name": tool, "arguments": arguments},
    }


@pytest.mark.parametrize("kind", KINDS)
def test_mcp_tools(kind: str, world: World, project) -> None:
    runner, ws, _ = project(kind)
    responses = drive_until_answered(
        runner,
        ws,
        [
            initialize(1),
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            _call(2, "grim_search", {"query": name_of(kind)}),
            _call(3, "grim_describe", {"ref": world.ref(kind)}),
            _call(4, "grim_fetch", {"ref": world.ref(kind)}),
            _call(5, "grim_status", {}),
        ],
        offline=False,
    )
    payloads = {}
    for idx in (2, 3, 4, 5):
        result = responses[idx]["result"]
        assert result["isError"] is False, result
        payloads[idx] = json.loads(result["content"][0]["text"])

    search = payloads[2]
    rows = search["items"] if isinstance(search, dict) else search
    assert kind in _by_kind(rows), rows
    assert payloads[3]["kind"] == kind
    assert payloads[4]["kind"] == kind
    assert payloads[4]["content"]
    status = payloads[5]
    items = status["items"] if isinstance(status, dict) else status
    assert kind in {item["kind"] for item in items}
