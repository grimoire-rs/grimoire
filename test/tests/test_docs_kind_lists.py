# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Every "sorted by kind (...)" list on a docs or catalog page, against the binary.

grim sorts a `requires`, `required_by` or `pulled_in` list by the artifact-kind
enum, so a page that spells that order out restates a code fact. The order is
read from the binary's own lock schema, whose `ArtifactKind` lists the variants
in declaration order, the order those sorts use. A kind added to the enum and
left off a page fails here, and so does a page saying a kind "sorts last" or
"sorts first" when the enum puts another kind there.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

from src.helpers import PROJECT_ROOT

PAGES = sorted(
    [
        *(PROJECT_ROOT / "docs" / "src" / "content" / "docs").rglob("*.md"),
        *(PROJECT_ROOT / "catalog" / "skills").rglob("*.md"),
    ]
)

# "sorted by kind (skill, rule, ...)", "in kind order (skill, ...)" and
# "sorted by kind, in the order skill, rule, ...", read over whitespace-joined text.
_KIND_LIST = re.compile(
    r"(?:sorted by kind|in kind order)(?:,? in the order |\s\()(?P<list>[a-z]+(?:, (?:and )?[a-z]+)+)"
)

# "a hook sorts last", "hooks sort first", "a settings member sorts last".
_KIND_EDGE = re.compile(
    r"\b(?P<kind>[a-z]+)(?: members?)? sorts? (?P<edge>last|first)\b"
)


def _enum_order(grim_at, project_dir: Path) -> list[str]:
    schema = json.loads(grim_at(project_dir).run("schema", "--kind", "lock").stdout)
    return [variant["const"] for variant in schema["$defs"]["ArtifactKind"]["oneOf"]]


def _kind_lists(kinds: list[str]) -> list[tuple[str, list[str]]]:
    """``(page, kinds)`` for every list, cut at the first word that is no kind."""
    found = []
    for page in PAGES:
        text = " ".join(page.read_text(encoding="utf-8").split())
        for m in _KIND_LIST.finditer(text):
            words = [w.removeprefix("and ") for w in m.group("list").split(", ")]
            listed = []
            for word in words:
                if word not in kinds:
                    break
                listed.append(word)
            found.append((page.relative_to(PROJECT_ROOT).as_posix(), listed))
    return found


def _edge_claims(kinds: list[str]) -> list[tuple[str, str, str]]:
    """``(page, kind, edge)`` for every "<kind> sorts last|first" claim."""
    found = []
    for page in PAGES:
        text = " ".join(page.read_text(encoding="utf-8").split())
        for m in _KIND_EDGE.finditer(text):
            word = m.group("kind")
            kind = word if word in kinds else word.removesuffix("s")
            if kind in kinds:
                found.append((page.relative_to(PROJECT_ROOT).as_posix(), kind, m.group("edge")))
    return found


def test_every_kind_order_list_names_every_kind_in_enum_order(
    grim_at, project_dir: Path
) -> None:
    kinds = _enum_order(grim_at, project_dir)
    assert kinds[:3] == ["skill", "rule", "agent"] and "settings" in kinds, kinds
    lists = _kind_lists(kinds)
    pages = [page for page, _ in lists]
    assert pages.count("docs/src/content/docs/json-interface.md") == 3, lists
    assert len(lists) >= 5, f"the pattern stopped finding the pages' lists: {lists}"
    drifted = [(page, listed) for page, listed in lists if listed != kinds]
    assert not drifted, (
        f"a kind-order list differs from the enum order {kinds}: {drifted}"
    )


def test_every_sorts_last_or_first_claim_names_the_enum_edge(
    grim_at, project_dir: Path
) -> None:
    kinds = _enum_order(grim_at, project_dir)
    edge = {"first": kinds[0], "last": kinds[-1]}
    claims = _edge_claims(kinds)
    assert claims, "the pattern stopped finding the pages' sorts-last claims"
    wrong = [(page, kind, at) for page, kind, at in claims if kind != edge[at]]
    assert not wrong, f"a page names the wrong kind at an edge of {kinds}: {wrong}"
