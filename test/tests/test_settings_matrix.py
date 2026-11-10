# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""The settings rule, row by row, across every file format and merge class.

``adr_settings_artifact_kind.md`` Decision 5 holds the rule table; Decision 4
the merge classes. Each row of that table is run for a scalar, a set element, a
map entry and an object container, in a JSON file (Claude), a JSONC file
(Copilot) and a TOML file (Codex), plus the own-file class (Codex's rules
file). ``COVERAGE`` is the one table: the ADR's rows are read from the ADR, and
a test fails when a row has no entry here or an entry has no row there. A cell
that cannot exist carries a one-line reason instead of a case.

Everything runs at global scope with the vendor variables redirected, so no
real home is touched. Fixtures come from ``make_settings``, never from the grim
under test. Each "nothing was written" test also declares a sentinel piece in
the same package and asserts it was written, so an install that never ran
cannot pass for one that wrote nothing.
"""

from __future__ import annotations

import json
import re
import tomllib
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from subprocess import CompletedProcess

import pytest

from src.helpers import PROJECT_ROOT, make_settings, settings_sandbox, write_config
from src.registry import PublishedArtifact
from src.runner import GrimRunner

ADR = PROJECT_ROOT / ".agents" / "adr" / "adr_settings_artifact_kind.md"
MISSING = object()
USER = {"mine": "keep"}
GARBAGE = {"json": "{not json", "jsonc": "{not json", "toml": "= = not toml ["}


# ── The rows, read from the ADR ──────────────────────────────────────────


def adr_rows() -> list[str]:
    """The first column of the rule table, in order: `1`, `2a`, ... `8`."""
    text = ADR.read_text(encoding="utf-8")
    start = text.index("**The rule, per piece.**")
    end = text.index("A set element has no", start)
    rows = [m.group(1) for m in re.finditer(r"^\| (\d[a-d]?) \|", text[start:end], re.MULTILINE)]
    assert rows, "the rule table moved: update the matcher"
    return rows


# ── Formats ──────────────────────────────────────────────────────────────


def _toml_value(value: object) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, list):
        return "[" + ", ".join(_toml_value(item) for item in value) + "]"
    return json.dumps(value)


def toml_dumps(doc: dict, prefix: tuple[str, ...] = ()) -> str:
    out = []
    tables = []
    for key, value in doc.items():
        if isinstance(value, dict):
            tables.append((key, value))
        else:
            out.append(f"{json.dumps(key)} = {_toml_value(value)}")
    text = "\n".join(out) + ("\n" if out else "")
    for key, value in tables:
        path = (*prefix, key)
        text += "\n[" + ".".join(json.dumps(p) for p in path) + "]\n" + toml_dumps(value, path)
    return text


@dataclass(frozen=True)
class Fmt:
    name: str
    client: str
    filename: str

    def read(self, path: Path) -> dict:
        text = path.read_text(encoding="utf-8")
        if self.name == "toml":
            return tomllib.loads(text)
        lines = [line for line in text.splitlines() if not line.lstrip().startswith("//")]
        return json.loads("\n".join(lines))

    def write(self, path: Path, doc: dict) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        if self.name == "toml":
            path.write_text(toml_dumps(doc), encoding="utf-8")
        elif self.name == "jsonc":
            path.write_text("// mine\n" + json.dumps(doc, indent=2) + "\n", encoding="utf-8")
        else:
            path.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")


FORMATS = {
    "json": Fmt("json", "claude", "settings.json"),
    "jsonc": Fmt("jsonc", "copilot", "settings.json"),
    "toml": Fmt("toml", "codex", "config.toml"),
}
# One bool key per client that no cell uses: it rides in every package so a run
# that writes something is told apart from a run that did not happen.
SENTINEL = {"json": ("spinnerTipsEnabled",), "jsonc": ("askUser",), "toml": ("hide_agent_reasoning",)}
# What a package aimed at some other client declares, so it applies here to nothing.
ELSEWHERE = {"json": {"codex": {"model": "gpt"}}, "jsonc": {"codex": {"model": "gpt"}}, "toml": {"claude": {"model": "x"}}}


def _sorted(node: object) -> object:
    if isinstance(node, dict):
        return {key: _sorted(value) for key, value in node.items()}
    if isinstance(node, list) and all(isinstance(item, str) for item in node):
        return sorted(node)
    return node


def nest(path: tuple[str, ...], leaf: object) -> dict:
    for key in reversed(path):
        leaf = {key: leaf}
    return leaf


def merge(*docs: dict) -> dict:
    out: dict = {}
    for doc in docs:
        for key, value in doc.items():
            if isinstance(value, dict) and isinstance(out.get(key), dict):
                out[key] = merge(out[key], value)
            else:
                out[key] = value
    return out


# ── Cells: what one (format, class) varies ───────────────────────────────

SCALAR = {"json": ("model", "sonnet", "opus", "haiku"), "jsonc": ("model", "gpt-5", "gpt-4", "o3"), "toml": ("model", "gpt-5", "o4", "o3")}
SET = {
    "json": (("permissions", "deny"), "Bash(rm:*)", "Bash(ls:*)", "Bash(curl:*)"),
    "jsonc": (("deniedUrls",), "https://a.test", "https://b.test", "https://u.test"),
    "toml": (("sandbox_workspace_write", "writable_roots"), "/a", "/b", "/u"),
}
MAP = {
    "json": (
        ("extraKnownMarketplaces",),
        {"source": {"source": "github", "repo": "acme/tools"}},
        {"source": {"source": "github", "repo": "acme/other"}},
        {"source": {"source": "github", "repo": "mine/own"}},
    ),
    "jsonc": (
        ("extraKnownMarketplaces",),
        {"source": "https://example.com/a"},
        {"source": "https://example.com/b"},
        {"source": "https://example.com/u"},
    ),
    "toml": (
        ("marketplaces",),
        {"source": "https://example.com/a", "source_type": "git"},
        {"source": "https://example.com/b", "source_type": "git"},
        {"source": "https://example.com/u", "source_type": "git"},
    ),
}
OBJECT = {
    "json": (("sandbox", "enabled"), True, False, False),
    "jsonc": (("sandbox", "enabled"), True, False, False),
    "toml": (("sandbox_workspace_write", "network_access"), True, False, False),
}


@dataclass
class Cell:
    cls: str
    fmt: Fmt
    path: tuple[str, ...]
    a: object
    b: object
    u: object

    @property
    def container(self) -> tuple[str, ...]:
        return self.path[:-1] if self.cls in ("object",) else self.path

    def element(self, variant: str) -> str | None:
        return {"a": self.a, "b": self.b, "u": self.u}[variant] if self.cls == "set" else None  # type: ignore[return-value]

    @property
    def pointer(self) -> str:
        return "/" + "/".join(self.entry_path)

    @property
    def entry_path(self) -> tuple[str, ...]:
        return (*self.path, "acme") if self.cls == "map" else self.path

    def value(self, variant: str) -> object:
        return {"a": self.a, "b": self.b, "u": self.u}[variant]

    def leaf(self, variant: str) -> object:
        """What the file holds at the cell after `variant` was declared."""
        if self.cls == "set":
            return [self.value(v) for v in variant]
        return self.value(variant)

    def declared(self, variant: str) -> dict:
        """The package's vendor table for `variant`: `a`, `b`, `ab`, or `` for none."""
        if not variant:
            return {}
        return nest(self.entry_path, self.leaf(variant))

    def doc(self, *parts: str, extra: dict | None = None) -> dict:
        """The file after grim wrote `parts`, over the user's own key."""
        sentinel = nest(SENTINEL[self.fmt.name], True)
        return merge(USER, *(self.declared(p) for p in parts), sentinel, extra or {})

    def sibling(self) -> dict:
        """The user's own entry beside the cell, in the same container."""
        if self.cls == "set":
            return nest(self.path, [self.u])
        if self.cls == "map":
            return nest((*self.path, "mine"), self.u)
        return nest((*self.path[:-1], "mine"), 1)

    def blocker(self) -> dict:
        """A value of another type standing where the cell, or its container, goes."""
        if self.cls == "scalar":
            return nest(self.path, {"x": 1})
        if self.cls == "set":
            return nest(self.path, "x")
        return nest(self.path[:1], "x")

    @property
    def blocked_pointer(self) -> str:
        return "/" + self.blocker_path

    @property
    def blocker_path(self) -> str:
        return "/".join(self.path if self.cls == "scalar" else self.path[:1])


def cell_for(cls: str, fmt: str) -> Cell:
    table = {"scalar": SCALAR, "set": SET, "map": MAP, "object": OBJECT}[cls]
    head, a, b, u = table[fmt]
    path = (head,) if isinstance(head, str) else head
    return Cell(cls, FORMATS[fmt], tuple(path), a, b, u)


# ── The rig ──────────────────────────────────────────────────────────────


def _package(repo: str, name: str, tables: dict, tag: str = "1") -> PublishedArtifact:
    return make_settings(f"{repo}/{name}", tag, {"description": f"{name} defaults", **tables})


class Rig:
    """A global scope naming one client, whose settings file the test controls."""

    def __init__(self, grim: GrimRunner, home: Path, sandbox: Path, repo: str, cell: Cell) -> None:
        self.grim, self.home, self.repo, self.cell = grim, home, repo, cell
        env, roots = settings_sandbox(sandbox, grim.home)
        grim.env.update(env)
        self.root = roots[cell.fmt.client]
        self.path = self.root / cell.fmt.filename
        self.sentinel = "/" + "/".join(SENTINEL[cell.fmt.name])
        self.tags: dict[str, int] = {}

    def package(self, name: str, variant: str, tables: dict | None = None) -> PublishedArtifact:
        """One more version of `name`: `variant` of the cell, plus the sentinel."""
        self.tags[name] = self.tags.get(name, 0) + 1
        body = tables if tables is not None else {
            self.cell.fmt.client: merge(self.cell.declared(variant), nest(SENTINEL[self.cell.fmt.name], True))
        }
        return _package(self.repo, name, body, tag=str(self.tags[name]))

    def configure(self, packages: dict[str, PublishedArtifact]) -> None:
        write_config(
            self.home,
            settings={name: p.fq for name, p in packages.items()},
            options={"clients": json.dumps([self.cell.fmt.client])},
            experimental_settings=True,
        )
        self.grim.run("--global", "lock")
        if not self.path.exists():
            self.write(USER)

    def install(self, *flags: str, check: bool = True) -> CompletedProcess[str]:
        return self.grim.run("--global", "install", *flags, format="json", check=check)

    def repin(self, old: PublishedArtifact, new: PublishedArtifact) -> None:
        lock = self.home / "grimoire.lock"
        text = lock.read_text(encoding="utf-8")
        assert old.digest in text
        lock.write_text(text.replace(old.digest, new.digest), encoding="utf-8")

    def read(self) -> dict:
        """The file, parsed, with string lists sorted: a set has no order to assert."""
        return _sorted(self.cell.fmt.read(self.path))

    def write(self, doc: dict) -> bytes:
        self.cell.fmt.write(self.path, doc)
        return self.path.read_bytes()

    def records(self) -> dict[tuple[str, str, str | None], bool]:
        """Each recorded piece, `(package, pointer, element)`, and its `adopted`."""
        state = Path(self.grim.grim_home) / "state" / "global.json"
        if not state.exists():
            return {}
        return {
            (record["name"], output["entry"], output.get("element")): output.get("adopted", False)
            for record in json.loads(state.read_text(encoding="utf-8"))["records"]
            if record["kind"] == "settings"
            for output in record["outputs"]
            if output["entry"] != self.sentinel
        }

    def changes(self, result: CompletedProcess[str]) -> list[tuple[str, str, str | None, str]]:
        """What the run listed, sentinel apart: a test names what the cell did."""
        rows = json.loads(result.stdout)["settings_changes"]
        return sorted(
            (row["package"], row["key"], row["element"], row["action"])
            for row in rows
            if row["key"] != self.sentinel
        )

    def sentinel_rows(self, result: CompletedProcess[str]) -> list[str]:
        return [row["action"] for row in json.loads(result.stdout)["settings_changes"] if row["key"] == self.sentinel]

    def refusal(self, result: CompletedProcess[str], reason: str | None) -> dict:
        assert result.returncode == 65, result.stderr
        error = json.loads(result.stdout)["error"]
        if reason is None:
            assert "reason" not in error and "forceable" not in error, error
        else:
            assert (error["reason"], error["forceable"]) == (reason, True), error
        return error


def rig(grim: GrimRunner, grim_home: Path, tmp_path: Path, repo: str, cell: Cell) -> Rig:
    return Rig(grim, grim_home, tmp_path / "sandbox", repo, cell)


def _piece(cell: Cell, variant: str, package: str = "team") -> tuple[str, str, str | None]:
    return (package, cell.pointer if cell.cls != "set" else "/" + "/".join(cell.path), cell.element(variant))


def _listed(cell: Cell, variant: str, action: str, package: str = "team") -> tuple[str, str, str | None, str]:
    return (*_piece(cell, variant, package), action)


# ── Rows, scalar and map: one value, compared whole ──────────────────────


def row_1(r: Rig) -> None:
    cell = r.cell
    team = r.package("team", "a")
    r.configure({"team": team})
    r.write(USER)

    result = r.install()

    assert r.changes(result) == [_listed(cell, "a", "added")]
    assert r.sentinel_rows(result) == ["added"]
    assert r.read() == cell.doc("a")
    assert r.records()[_piece(cell, "a")] is False
    # The record reads back: a lost one would re-adopt on the next run.
    assert r.changes(r.install()) == []
    assert r.records()[_piece(cell, "a")] is False


def row_2a(r: Rig) -> None:
    cell = r.cell
    r.configure({"team": r.package("team", "a")})
    r.write(merge(USER, cell.declared("a")))

    result = r.install()

    assert r.sentinel_rows(result) == ["added"], "the run happened and wrote the sentinel"
    assert r.changes(result) == []
    assert r.read() == cell.doc("a")
    assert r.records()[_piece(cell, "a")] is True


def row_2b(r: Rig) -> None:
    cell = r.cell
    r.configure({"team": r.package("team", "a")})
    r.write(merge(USER, cell.declared("a")))
    assert r.changes(r.install()) == []
    held = r.path.read_bytes()

    result = r.install()

    assert result.returncode == 0
    assert json.loads(result.stdout)["settings_changes"] == []
    assert r.path.read_bytes() == held
    assert r.records()[_piece(cell, "a")] is True


def row_2c(r: Rig) -> None:
    cell = r.cell
    first, second = r.package("team", "a"), r.package("team", "b")
    r.configure({"team": first})
    r.install()
    assert r.records()[_piece(cell, "a")] is False
    # The user made the file match where the lock then moved.
    held = r.write(cell.doc("b"))
    r.repin(first, second)

    result = r.install()

    assert r.changes(result) == []
    assert r.path.read_bytes() == held
    assert r.records()[_piece(cell, "b")] is True
    # The record moved to hash(L): a stale one would refuse as modified here.
    r.repin(second, first)
    assert r.changes(r.install()) == [_listed(cell, "a", "changed")]
    assert r.read() == cell.doc("a")


def row_3a(r: Rig) -> None:
    cell = r.cell
    first, second = r.package("team", "a"), r.package("team", "b")
    r.configure({"team": first})
    r.install()
    r.repin(first, second)

    result = r.install()

    assert r.changes(result) == [_listed(cell, "b", "changed")]
    assert r.read() == cell.doc("b")
    assert r.records()[_piece(cell, "b")] is False


def row_3b(r: Rig) -> None:
    cell = r.cell
    first, second = r.package("team", "a"), r.package("team", "")
    r.configure({"team": first})
    r.write(merge(USER, cell.sibling()))
    r.install()
    assert r.records()[_piece(cell, "a")] is False
    r.repin(first, second)

    result = r.install()

    assert r.changes(result) == [_listed(cell, "a", "removed")]
    assert r.records() == {}
    assert r.read() == merge(USER, cell.sibling(), nest(SENTINEL[cell.fmt.name], True))


def row_3c(r: Rig) -> None:
    cell = r.cell
    first, second = r.package("team", "a"), r.package("team", "")
    r.configure({"team": first})
    r.write(merge(USER, cell.declared("a")))
    r.install()
    assert r.records()[_piece(cell, "a")] is True
    r.repin(first, second)

    result = r.install()

    assert r.changes(result) == []
    assert r.read() == cell.doc("a"), "an adopted value is the user's and stays"
    assert cell.pointer in result.stderr or "/" + "/".join(cell.path) in result.stderr
    assert r.records() == {}


def row_3d(r: Rig) -> None:
    cell = r.cell
    first, second = r.package("team", "a"), r.package("team", "")
    r.configure({"team": first})
    r.install()
    r.write(merge(USER, nest(SENTINEL[cell.fmt.name], True)))
    r.repin(first, second)

    result = r.install()

    assert r.changes(result) == []
    assert r.records() == {}
    assert r.read() == merge(USER, nest(SENTINEL[cell.fmt.name], True))
    assert "/" + "/".join(cell.path) not in result.stderr


def row_4a(r: Rig) -> None:
    cell = r.cell
    first, second = r.package("team", "a"), r.package("team", "b")
    r.configure({"team": first})
    held = r.write(merge(USER, cell.declared("u")))

    error = r.refusal(r.install(check=False), "untracked-destination")

    [piece] = error["pieces"]
    assert (piece["pointer"], piece["marked"], piece["state"]) == (cell.pointer, "overwrite", "untracked-destination")
    assert r.path.read_bytes() == held
    assert r.records() == {}
    forced = r.install("--force")
    assert r.changes(forced) == [_listed(cell, "a", "overwritten")]
    assert r.read() == cell.doc("a")
    # With a record: the user changes it, the lock moves.
    r.write(cell.doc("u"))
    r.repin(first, second)
    held = r.path.read_bytes()
    r.refusal(r.install(check=False), "modified")
    assert r.path.read_bytes() == held
    assert r.changes(r.install("--force")) == [_listed(cell, "b", "overwritten")]
    assert r.read() == cell.doc("b")


def row_4b(r: Rig) -> None:
    cell = r.cell
    first, second = r.package("team", "a"), r.package("team", "")
    r.configure({"team": first})
    r.install()
    held = r.write(cell.doc("u"))
    r.repin(first, second)

    error = r.refusal(r.install(check=False), "modified")

    [piece] = error["pieces"]
    assert (piece["pointer"], piece["wanted"]) == (cell.pointer, None)
    assert r.path.read_bytes() == held
    forced = r.install("--force")
    assert r.changes(forced) == [_listed(cell, "a", "removed")]
    assert r.read() == merge(USER, nest(SENTINEL[cell.fmt.name], True))
    assert r.records() == {}


def row_5(r: Rig) -> None:
    cell = r.cell
    r.configure({"team": r.package("team", "a")})
    held = r.write(merge(USER, cell.blocker()))

    error = r.refusal(r.install(check=False), "untracked-destination")

    assert cell.blocked_pointer in [piece["pointer"] for piece in error["pieces"]], error["pieces"]
    assert r.path.read_bytes() == held
    assert r.records() == {}
    forced = r.install("--force")
    assert r.changes(forced) == [_listed(cell, "a", "overwritten")]
    assert r.read() == cell.doc("a")


def row_6(r: Rig) -> None:
    cell = r.cell
    alpha, beta = r.package("alpha", "a"), r.package("beta", "a")
    r.configure({"alpha": alpha, "beta": beta})

    result = r.install()

    assert [row[1:] for row in r.changes(result)] == [_listed(cell, "a", "added")[1:]], "listed once"
    assert r.read() == cell.doc("a")
    both = {_piece(cell, "a", name): False for name in ("alpha", "beta")}
    assert {k: v for k, v in r.records().items() if k[1] == _piece(cell, "a")[1]} == both
    # One declarer leaves: the survivor's record decides, and no layer is read.
    r.configure({"alpha": alpha})
    assert r.changes(r.install()) == []
    assert r.read() == cell.doc("a")
    # The last one leaves: the piece goes.
    r.configure({})
    assert r.changes(r.install()) == [_listed(cell, "a", "removed", "alpha")]
    assert r.read() == merge(USER, nest(SENTINEL[cell.fmt.name], True)) or r.read() == USER


def row_7a(r: Rig) -> None:
    cell = r.cell
    r.configure({"team": r.package("team", "a")})
    r.path.write_text(GARBAGE[cell.fmt.name], encoding="utf-8")

    for flags in ((), ("--force",)):
        error = r.refusal(r.install(*flags, check=False), None)
        assert error["pieces"], error
        assert r.path.read_text(encoding="utf-8") == GARBAGE[cell.fmt.name]
        assert r.records() == {}
    # Positive control: the same run writes a file that parses.
    r.path.unlink()
    assert r.changes(r.install()) == [_listed(cell, "a", "added")]
    assert r.read() == merge(cell.declared("a"), nest(SENTINEL[cell.fmt.name], True))


def row_7b(r: Rig) -> None:
    cell = r.cell
    first = r.package("team", "a")
    second = r.package("team", "", tables=ELSEWHERE[cell.fmt.name])
    r.configure({"team": first})
    r.install()
    assert r.records()
    r.path.write_text(GARBAGE[cell.fmt.name], encoding="utf-8")
    r.repin(first, second)

    result = r.install()

    assert result.returncode == 0
    assert r.changes(result) == []
    assert r.path.read_text(encoding="utf-8") == GARBAGE[cell.fmt.name]
    assert r.records() == {}
    assert cell.pointer in result.stderr or "/" + "/".join(cell.path) in result.stderr


def row_8(r: Rig) -> None:
    cell = r.cell
    first, second = r.package("team", "a"), r.package("team", "")
    r.configure({"team": first})
    mine = merge(USER, cell.sibling())
    r.write(mine)

    assert r.changes(r.install()) == [_listed(cell, "a", "added")]
    assert r.read() == merge(mine, cell.declared("a"), nest(SENTINEL[cell.fmt.name], True))
    r.repin(first, second)
    assert r.changes(r.install()) == [_listed(cell, "a", "removed")]
    assert r.read() == merge(mine, nest(SENTINEL[cell.fmt.name], True))


# ── Rows, set: one piece per element ─────────────────────────────────────


def _array(cell: Cell, *elements: str, user: bool = False) -> dict:
    items = sorted(([cell.u] if user else []) + [cell.value(e) for e in elements])
    return merge(USER, nest(cell.path, items), nest(SENTINEL[cell.fmt.name], True))


def set_1(r: Rig) -> None:
    cell = r.cell
    r.configure({"team": r.package("team", "ab")})
    r.write(merge(USER, nest(cell.path, [cell.u])))

    result = r.install()

    assert r.changes(result) == [_listed(cell, "a", "added"), _listed(cell, "b", "added")] or r.changes(result) == sorted(
        [_listed(cell, "a", "added"), _listed(cell, "b", "added")], key=lambda row: (row[1], row[2] or "")
    )
    assert r.read() == _array(cell, "a", "b", user=True)
    assert r.records() == {_piece(cell, "a"): False, _piece(cell, "b"): False}
    assert r.changes(r.install()) == []


def set_2a(r: Rig) -> None:
    cell = r.cell
    r.configure({"team": r.package("team", "a")})
    r.write(merge(USER, nest(cell.path, [cell.u, cell.a])))

    result = r.install()

    assert r.sentinel_rows(result) == ["added"]
    assert r.changes(result) == []
    assert r.read() == _array(cell, "a", user=True)
    assert r.records() == {_piece(cell, "a"): True}


def set_2b(r: Rig) -> None:
    cell = r.cell
    r.configure({"team": r.package("team", "a")})
    r.write(merge(USER, nest(cell.path, [cell.a])))
    r.install()
    held = r.path.read_bytes()

    result = r.install()

    assert json.loads(result.stdout)["settings_changes"] == []
    assert r.path.read_bytes() == held
    assert r.records() == {_piece(cell, "a"): True}


def set_3b(r: Rig) -> None:
    cell = r.cell
    both, one, none = r.package("team", "ab"), r.package("team", "a"), r.package("team", "")
    r.configure({"team": both})
    r.write(merge(USER, nest(cell.path, [cell.u])))
    r.install()
    r.repin(both, one)

    assert r.changes(r.install()) == [_listed(cell, "b", "removed")]
    assert r.read() == _array(cell, "a", user=True)
    # The last of ours goes: the user's element stays, and so does the array.
    r.repin(one, none)
    assert r.changes(r.install()) == [_listed(cell, "a", "removed")]
    assert r.read() == merge(USER, nest(cell.path, [cell.u]), nest(SENTINEL[cell.fmt.name], True))
    assert r.records() == {}


def set_3c(r: Rig) -> None:
    cell = r.cell
    both, one = r.package("team", "ab"), r.package("team", "a")
    r.configure({"team": both})
    r.write(merge(USER, nest(cell.path, [cell.b])))
    r.install()
    assert r.records()[_piece(cell, "b")] is True
    r.repin(both, one)

    result = r.install()

    assert r.changes(result) == []
    assert cell.b in r.read_leaf()
    assert "/" + "/".join(cell.path) in result.stderr
    assert (("team", "/" + "/".join(cell.path), cell.b)) not in r.records()


def set_3d(r: Rig) -> None:
    cell = r.cell
    both, one = r.package("team", "ab"), r.package("team", "a")
    r.configure({"team": both})
    r.install()
    r.write(merge(USER, nest(cell.path, [cell.a]), nest(SENTINEL[cell.fmt.name], True)))
    r.repin(both, one)

    result = r.install()

    assert r.changes(result) == []
    assert r.records() == {_piece(cell, "a"): False}
    assert r.read_leaf() == [cell.a]
    assert cell.b not in result.stderr


def set_5(r: Rig) -> None:
    cell = r.cell
    r.configure({"team": r.package("team", "a")})
    held = r.write(merge(USER, cell.blocker()))

    error = r.refusal(r.install(check=False), "untracked-destination")

    assert "/" + "/".join(cell.path) in [piece["pointer"] for piece in error["pieces"]]
    assert r.path.read_bytes() == held
    forced = r.install("--force")
    assert r.changes(forced) == [_listed(cell, "a", "overwritten")]
    assert r.read_leaf() == [cell.a]


def set_6(r: Rig) -> None:
    cell = r.cell
    alpha, beta = r.package("alpha", "a"), r.package("beta", "a")
    r.configure({"alpha": alpha, "beta": beta})

    result = r.install()

    assert len(r.changes(result)) == 1
    assert r.read_leaf() == [cell.a]
    assert set(r.records()) == {_piece(cell, "a", "alpha"), _piece(cell, "a", "beta")}
    r.configure({"alpha": alpha})
    assert r.changes(r.install()) == []
    assert r.read_leaf() == [cell.a]
    r.configure({})
    assert r.changes(r.install()) == [_listed(cell, "a", "removed", "alpha")]
    assert r.read_leaf() is MISSING


def set_7a(r: Rig) -> None:
    row_7a(r)


def set_7b(r: Rig) -> None:
    row_7b(r)


def set_8(r: Rig) -> None:
    cell = r.cell
    first, second = r.package("team", "a"), r.package("team", "")
    r.configure({"team": first})
    mine = merge(USER, nest(cell.path, [cell.u]))
    r.write(mine)

    assert r.changes(r.install()) == [_listed(cell, "a", "added")]
    assert r.read() == _array(cell, "a", user=True)
    r.repin(first, second)
    assert r.changes(r.install()) == [_listed(cell, "a", "removed")]
    assert r.read_leaf() == [cell.u]


def _read_leaf(self: Rig) -> object:
    node: object = self.read()
    for key in self.cell.path:
        if not isinstance(node, dict) or key not in node:
            return MISSING
        node = node[key]
    return node


Rig.read_leaf = _read_leaf  # type: ignore[attr-defined]


# ── Rows, object: recursion only ─────────────────────────────────────────


def obj_1(r: Rig) -> None:
    cell = r.cell
    r.configure({"team": r.package("team", "a")})
    r.write(USER)

    result = r.install()

    assert r.changes(result) == [_listed(cell, "a", "added")]
    assert r.read() == cell.doc("a")
    assert r.read_leaf() is True


def obj_3b(r: Rig) -> None:
    cell = r.cell
    first, second = r.package("team", "a"), r.package("team", "")
    r.configure({"team": first})
    r.write(USER)
    r.install()
    r.repin(first, second)

    result = r.install()

    assert r.changes(result) == [_listed(cell, "a", "removed")]
    assert r.read() == merge(USER, nest(SENTINEL[cell.fmt.name], True)), "the emptied container is pruned"
    # Positive control: a container with a user's key beside ours is kept.
    third = r.package("team", "a")
    r.repin(second, third)
    r.write(merge(USER, cell.sibling()))
    r.install()
    r.repin(third, r.package("team", ""))
    r.install()
    assert r.read() == merge(USER, cell.sibling(), nest(SENTINEL[cell.fmt.name], True))


def obj_5(r: Rig) -> None:
    cell = r.cell
    r.configure({"team": r.package("team", "a")})
    held = r.write(merge(USER, cell.blocker()))

    error = r.refusal(r.install(check=False), "untracked-destination")

    assert cell.blocked_pointer in [piece["pointer"] for piece in error["pieces"]], error["pieces"]
    assert r.path.read_bytes() == held
    forced = r.install("--force")
    assert r.changes(forced) == [_listed(cell, "a", "overwritten")]
    assert r.read() == cell.doc("a")


def obj_8(r: Rig) -> None:
    cell = r.cell
    first, second = r.package("team", "a"), r.package("team", "")
    r.configure({"team": first})
    mine = merge(USER, cell.sibling())
    r.write(mine)

    r.install()
    assert r.read() == merge(mine, cell.declared("a"), nest(SENTINEL[cell.fmt.name], True))
    r.repin(first, second)
    r.install()
    assert r.read() == merge(mine, nest(SENTINEL[cell.fmt.name], True))


# ── The coverage table ───────────────────────────────────────────────────

CLASSES = ("scalar", "set", "map", "object", "ownfile")
SET_NO_DIFFERENT_VALUE = "a set element is present or absent, so it has no 'different value' to overwrite"
OBJECT_NO_PIECE = "an object container is no piece: the leaf's cases run this row"
OWN_NO_PARSE = "an own file is judged by its bytes and never parsed, so it cannot be unparseable"

Case = Callable[[Rig], None] | str
COVERAGE: dict[tuple[str, str], Case] = {}


def _fill() -> None:
    valued = {
        "1": row_1, "2a": row_2a, "2b": row_2b, "2c": row_2c, "3a": row_3a, "3b": row_3b, "3c": row_3c,
        "3d": row_3d, "4a": row_4a, "4b": row_4b, "5": row_5, "6": row_6, "7a": row_7a, "7b": row_7b, "8": row_8,
    }
    sets: dict[str, Case] = {
        "1": set_1, "2a": set_2a, "2b": set_2b, "3b": set_3b, "3c": set_3c, "3d": set_3d, "5": set_5, "6": set_6,
        "7a": set_7a, "7b": set_7b, "8": set_8,
        "2c": "a present element always hashes as recorded, so no record can disagree with a matching file",
        "3a": "an element cannot change: a different element is one dropped and one added (rows 3b, 1)",
        "4a": SET_NO_DIFFERENT_VALUE,
        "4b": "a hand-edited element reads as missing, so a dropped one is row 3d, never a refusal",
    }
    objects: dict[str, Case] = {"1": obj_1, "3b": obj_3b, "5": obj_5, "8": obj_8}
    for row in adr_rows():
        COVERAGE[(row, "scalar")] = valued.get(row, "no case written")
        COVERAGE[(row, "map")] = valued.get(row, "no case written")
        COVERAGE[(row, "set")] = sets.get(row, "no case written")
        COVERAGE[(row, "object")] = objects.get(row, OBJECT_NO_PIECE)
        COVERAGE[(row, "ownfile")] = OWN.get(row, "no case written")


# ── Own file: Codex's rules, whole-file bytes ────────────────────────────


def _render(rules: list[dict]) -> str:
    out = ""
    for rule in rules:
        out += "prefix_rule(\n"
        out += f"    pattern = {json.dumps(rule['pattern'])},\n"
        out += f"    decision = {json.dumps(rule['decision'])},\n"
        out += ")\n"
    return out


RULES_A = [{"pattern": ["npm", "test"], "decision": "allow"}]
RULES_B = [{"pattern": ["rm"], "decision": "forbidden"}]
USER_RULES = b'prefix_rule(pattern = ["ls"], decision = "allow")\n# mine\n'
RENDER = {"a": _render(RULES_A), "b": _render(RULES_B)}


class Own:
    """Codex's `grim-team.rules` beside its own `default.rules`."""

    def __init__(self, grim: GrimRunner, home: Path, tmp_path: Path, repo: str) -> None:
        self.grim, self.home, self.repo = grim, home, repo
        env, roots = settings_sandbox(tmp_path / "sandbox", grim.home)
        grim.env.update(env)
        self.file = roots["codex"] / "rules" / "grim-team.rules"
        self.default = roots["codex"] / "rules" / "default.rules"
        self.default.parent.mkdir(parents=True, exist_ok=True)
        self.default.write_bytes(USER_RULES)
        self.tags = 0
        self.sentinel = "/model"

    def package(self, rules: list[dict] | None) -> PublishedArtifact:
        self.tags += 1
        table = {"model": "gpt-5", **({"rules": rules} if rules is not None else {})}
        return _package(self.repo, "team", {"codex": table}, tag=str(self.tags))

    def configure(self, packages: dict[str, PublishedArtifact]) -> None:
        write_config(
            self.home,
            settings={name: p.fq for name, p in packages.items()},
            options={"clients": json.dumps(["codex"])},
            experimental_settings=True,
        )
        self.grim.run("--global", "lock")

    install = Rig.install
    repin = Rig.repin
    refusal = Rig.refusal

    @property
    def cell(self) -> Cell:
        return cell_for("scalar", "toml")

    def records(self) -> dict[tuple[str, str, str | None], bool]:
        state = Path(self.grim.grim_home) / "state" / "global.json"
        if not state.exists():
            return {}
        return {
            (record["name"], output.get("entry", "/rules"), output.get("element")): output.get("adopted", False)
            for record in json.loads(state.read_text(encoding="utf-8"))["records"]
            if record["kind"] == "settings"
            for output in record["outputs"]
            if output.get("entry", "/rules") == "/rules"
        }

    def changes(self, result: CompletedProcess[str]) -> list[tuple[str, str, str | None, str]]:
        rows = json.loads(result.stdout)["settings_changes"]
        return sorted((r["package"], r["key"], r["element"], r["action"]) for r in rows if r["key"] == "/rules")

    def sentinel_rows(self, result: CompletedProcess[str]) -> list[str]:
        return [r["action"] for r in json.loads(result.stdout)["settings_changes"] if r["key"] == "/model"]


PIECE = ("team", "/rules", None)
LISTED = lambda action: [("team", "/rules", None, action)]  # noqa: E731


def own_start(o: Own, first: list[dict] | None = None) -> PublishedArtifact:
    team = o.package(first if first is not None else RULES_A)
    o.configure({"team": team})
    return team


def own_1(o: Own) -> None:
    own_start(o)
    result = o.install()
    assert o.changes(result) == LISTED("added")
    assert o.file.read_text(encoding="utf-8") == RENDER["a"]
    assert o.records() == {PIECE: False}
    assert o.changes(o.install()) == []
    assert o.default.read_bytes() == USER_RULES


def own_2a(o: Own) -> None:
    own_start(o)
    o.file.parent.mkdir(exist_ok=True)
    o.file.write_text(RENDER["a"], encoding="utf-8", newline="\n")
    result = o.install()
    assert o.sentinel_rows(result) == ["added"]
    assert o.changes(result) == []
    assert o.file.read_text(encoding="utf-8") == RENDER["a"]
    assert o.records() == {PIECE: True}


def own_2b(o: Own) -> None:
    own_2a(o)
    held = o.file.read_bytes()
    result = o.install()
    assert json.loads(result.stdout)["settings_changes"] == []
    assert o.file.read_bytes() == held
    assert o.records() == {PIECE: True}


def own_2c(o: Own) -> None:
    first = own_start(o)
    o.install()
    second = o.package(RULES_B)
    o.file.write_text(RENDER["b"], encoding="utf-8", newline="\n")
    o.repin(first, second)
    held = o.file.read_bytes()
    result = o.install()
    assert o.changes(result) == []
    assert o.file.read_bytes() == held
    assert o.records() == {PIECE: True}


def own_3a(o: Own) -> None:
    first = own_start(o)
    o.install()
    o.repin(first, o.package(RULES_B))
    result = o.install()
    assert o.changes(result) == LISTED("changed")
    assert o.file.read_text(encoding="utf-8") == RENDER["b"]
    assert o.records() == {PIECE: False}


def own_3b(o: Own) -> None:
    first = own_start(o)
    o.install()
    assert o.file.is_file()
    o.repin(first, o.package(None))
    result = o.install()
    assert o.changes(result) == LISTED("removed")
    assert not o.file.exists()
    assert o.records() == {}
    assert o.default.read_bytes() == USER_RULES


def own_3c(o: Own) -> None:
    first = own_start(o)
    o.file.parent.mkdir(exist_ok=True)
    o.file.write_text(RENDER["a"], encoding="utf-8", newline="\n")
    o.install()
    o.repin(first, o.package(None))
    result = o.install()
    assert o.changes(result) == []
    assert o.file.read_text(encoding="utf-8") == RENDER["a"], "an adopted file is the user's and stays"
    assert "rules" in result.stderr
    assert o.records() == {}


def own_3d(o: Own) -> None:
    first = own_start(o)
    o.install()
    o.file.unlink()
    o.repin(first, o.package(None))
    result = o.install()
    assert o.changes(result) == []
    assert o.records() == {}
    assert not o.file.exists()


def own_4a(o: Own) -> None:
    first = own_start(o)
    o.file.parent.mkdir(exist_ok=True)
    o.file.write_text("# mine\n", encoding="utf-8")
    error = o.refusal(o.install(check=False), "untracked-destination")
    assert [piece["pointer"] for piece in error["pieces"]] == ["/rules"]
    assert o.file.read_text(encoding="utf-8") == "# mine\n"
    assert o.records() == {}
    assert o.changes(o.install("--force")) == LISTED("overwritten")
    assert o.file.read_text(encoding="utf-8") == RENDER["a"]
    # With a record: the user edits it and the lock moves.
    o.file.write_text("# edited\n", encoding="utf-8")
    o.repin(first, o.package(RULES_B))
    o.refusal(o.install(check=False), "modified")
    assert o.file.read_text(encoding="utf-8") == "# edited\n"
    assert o.changes(o.install("--force")) == LISTED("overwritten")
    assert o.file.read_text(encoding="utf-8") == RENDER["b"]


def own_4b(o: Own) -> None:
    first = own_start(o)
    o.install()
    o.file.write_text("# edited\n", encoding="utf-8")
    o.repin(first, o.package(None))
    o.refusal(o.install(check=False), "modified")
    assert o.file.read_text(encoding="utf-8") == "# edited\n"
    assert o.changes(o.install("--force")) == LISTED("removed")
    assert not o.file.exists()
    assert o.records() == {}


def own_5(o: Own) -> None:
    own_start(o)
    o.file.mkdir(parents=True)
    result = o.install(check=False)
    assert result.returncode == 65, result.stderr
    assert o.file.is_dir()
    assert o.records() == {}
    assert not o.file.parent.parent.joinpath("config.toml").exists(), "a refusal writes nothing"
    # Positive control: with the directory gone the same run writes.
    o.file.rmdir()
    assert o.changes(o.install()) == LISTED("added")


def own_8(o: Own) -> None:
    first = own_start(o)
    o.install()
    o.repin(first, o.package(None))
    o.install()
    assert not o.file.exists()
    assert o.default.read_bytes() == USER_RULES, "Codex's own rules file is never compared"


OWN: dict[str, Case] = {
    "1": own_1, "2a": own_2a, "2b": own_2b, "2c": own_2c, "3a": own_3a, "3b": own_3b, "3c": own_3c, "3d": own_3d,
    "4a": own_4a, "4b": own_4b, "5": own_5, "8": own_8,
    "6": "each binding owns its own file name, so two packages can never declare one file",
    "7a": OWN_NO_PARSE,
    "7b": OWN_NO_PARSE,
}
_fill()


# ── Tests ────────────────────────────────────────────────────────────────


def test_every_adr_row_has_an_entry_and_every_entry_a_row() -> None:
    rows = adr_rows()
    assert rows == ["1", "2a", "2b", "2c", "3a", "3b", "3c", "3d", "4a", "4b", "5", "6", "7a", "7b", "8"]
    assert {row for row, _ in COVERAGE} == set(rows)
    for row in rows:
        assert {cls for r, cls in COVERAGE if r == row} == set(CLASSES), row


def test_every_missing_case_states_why() -> None:
    gaps = {key: why for key, why in COVERAGE.items() if isinstance(why, str)}
    assert gaps, "the table lists no impossible cell: the matcher is broken"
    for key, why in gaps.items():
        assert why != "no case written", f"{key} has neither a case nor a reason"
        assert "\n" not in why and 10 < len(why) < 160, (key, why)


def _cases(classes: tuple[str, ...]) -> list:
    params = []
    for (row, cls), case in COVERAGE.items():
        if cls not in classes or isinstance(case, str):
            continue
        for fmt in FORMATS:
            params.append(pytest.param(row, cls, fmt, id=f"row{row}-{cls}-{fmt}"))
    return params


@pytest.mark.parametrize(("row", "cls", "fmt"), _cases(("scalar", "set", "map", "object")))
def test_rule_row(grim, grim_home: Path, tmp_path: Path, unique_repo: str, row: str, cls: str, fmt: str) -> None:
    case = COVERAGE[(row, cls)]
    assert callable(case)
    case(rig(grim, grim_home, tmp_path, unique_repo, cell_for(cls, fmt)))


@pytest.mark.parametrize("row", [row for (row, cls), case in COVERAGE.items() if cls == "ownfile" and callable(case)])
def test_rule_row_own_file(grim, grim_home: Path, tmp_path: Path, unique_repo: str, row: str) -> None:
    case = COVERAGE[(row, "ownfile")]
    assert callable(case)
    case(Own(grim, grim_home, tmp_path, unique_repo))


def test_force_replaces_only_an_empty_directory_at_an_own_file(
    grim, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    """`--force` takes an empty directory at a rules file and stops at one holding anything.

    Row 5 replaces a value. What a directory holds is the user's and no row names
    it, so a forced write stops before any write rather than deleting it.
    """
    o = Own(grim, grim_home, tmp_path, unique_repo)
    own_start(o)
    o.file.mkdir(parents=True)
    keep = o.file / "mine"
    keep.write_text("mine\n", encoding="utf-8")
    config = o.file.parent.parent / "config.toml"
    o.refusal(o.install(check=False), "untracked-destination")

    stopped = o.install("--force", check=False)

    assert stopped.returncode == 74, stopped.stderr
    assert "only an empty directory" in stopped.stdout + stopped.stderr
    assert str(o.file) in stopped.stdout + stopped.stderr, "the stop names what stands in the way"
    assert keep.read_text(encoding="utf-8") == "mine\n"
    assert o.records() == {}
    assert not config.exists(), "the stop comes before any write"

    # Positive control: emptied, the same forced run replaces the directory.
    keep.unlink()
    assert o.changes(o.install("--force")) == LISTED("overwritten")
    assert o.file.read_text(encoding="utf-8") == RENDER["a"]
    assert o.records() == {PIECE: False}
