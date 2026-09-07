# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Structural guards for the user documentation under ``docs/src/content/docs/``.

Doc pages cross-link heavily (command reference, concepts, vendor
registries), and a renamed page or dropped section breaks readers silently
— a static-site build renders a dangling anchor without failing. These
tests pin the structural invariants:

- the page set is non-empty (a mistargeted root cannot pass by collecting
  nothing),
- every internal link points at an existing page, or at one of the planned
  pages still to be written,
- every internal anchor (``./page.md#section``, ``../guides/x.md#section``
  or same-page ``#section``) resolves to an explicit ``{#anchor}`` or a
  heading on the target page.

Runnable doc *workflows* (the quickstart chain etc.) are exercised
against the real binary in ``test_workflows.py``; this module checks
structure only.
"""

from __future__ import annotations

import re
import unicodedata
from pathlib import Path

import pytest

from src.helpers import PROJECT_ROOT

_DOCS_DIR = PROJECT_ROOT / "docs" / "src" / "content" / "docs"

# Pages the docs plan commits to but that are not written yet (spec C-025).
# A link to one of these resolves to nothing on disk and still passes — the
# contract is that the sibling page is coming, not that the link is dead.
# Anything *not* listed here still fails, so a typo cannot hide behind the
# allowlist. Remove an entry the moment its page lands.
_PLANNED_PAGES = frozenset(
    {
        "browse",
        "first-skill",
        "guides/scopes-and-clients",
        "guides/shared-skills",
        "guides/lifecycle",
        "guides/inspect",
        "guides/versioning",
        "guides/mcp-everywhere",
        "guides/team-ci",
        "guides/registries",
        "guides/catalog-best-practices",
        "tutorials/own-index",
    }
)

# Inline links and reference-style definitions to a local page, with an
# optional anchor: `](./page.md#a)`, `](../guides/lifecycle.md)`, `]: ./page.md`, ...
_INTERNAL_LINK = re.compile(
    r"(?:\]\(|\]:\s*)(?P<page>\.{1,2}/[A-Za-z0-9._/-]+\.md)(?:#(?P<anchor>[A-Za-z0-9._-]+))?"
)
# Same-page anchor links: `](#section)`.
_LOCAL_LINK = re.compile(r"\]\(#(?P<anchor>[A-Za-z0-9._-]+)\)")
# Explicit `{#anchor}` markers on headings.
_EXPLICIT_ANCHOR = re.compile(r"\{#(?P<anchor>[A-Za-z0-9._-]+)\}")
_HEADING = re.compile(r"^#{1,6}\s+(?P<text>.+?)\s*$", re.MULTILINE)


def _pages() -> list[Path]:
    return sorted(_DOCS_DIR.rglob("*.md"))


def _page_id(page: Path) -> str:
    """Parametrize id: the path relative to the root (``.name`` collides)."""
    return page.relative_to(_DOCS_DIR).as_posix()


def _strip_code_blocks(text: str) -> str:
    """Drop fenced code blocks — links inside them are samples, not links."""
    return re.sub(r"```.*?```", "", text, flags=re.DOTALL)


def _slugify(heading: str) -> str:
    """Reproduce github-slugger, the slugger Astro and Starlight use.

    Its rule, in order: lowercase; delete every punctuation, symbol,
    control and format character except ``-`` and ``_``; turn each ASCII
    space into ``-``. So an em dash is *deleted* rather than becoming a
    hyphen, while digits and non-ASCII letters survive. Matching the real
    slugger is the point: an anchor that passes here cannot fail the build.

    The trailing ``strip()`` is ours, not the slugger's — it removes the
    whitespace left behind by an explicit ``{#anchor}`` marker so the
    heading slugs the way it renders once the marker is consumed.
    """
    text = _EXPLICIT_ANCHOR.sub("", heading).strip().lower()
    out: list[str] = []
    for ch in text:
        category = unicodedata.category(ch)
        if ch in "-_":
            out.append(ch)
        elif ch == " ":
            # Only U+0020 becomes a hyphen. Every other Zs separator (NBSP,
            # U+3000) is deleted, so this arm has to run before the delete.
            out.append("-")
        elif category[0] in "PSZ" or category in ("Cc", "Cf"):
            continue
        else:
            out.append(ch)
    return "".join(out)


def _anchors(page: Path) -> set[str]:
    body = _strip_code_blocks(page.read_text(encoding="utf-8"))
    anchors = {m.group("anchor") for m in _EXPLICIT_ANCHOR.finditer(body)}
    # github-slugger suffixes repeats in document order: the second `foo`
    # becomes `foo-1`, the third `foo-2`. Walk the headings, don't set-ify.
    seen: dict[str, int] = {}
    for m in _HEADING.finditer(body):
        slug = _slugify(m.group("text"))
        result = slug
        while result in seen:
            seen[slug] += 1
            result = f"{slug}-{seen[slug]}"
        seen[result] = 0
        anchors.add(result)
    return anchors


def test_pages_are_discovered() -> None:
    """A mistargeted root must fail loudly, not collect zero cases."""
    assert _pages(), f"no .md pages found under {_DOCS_DIR}"


def _link_problems(page: Path) -> list[str]:
    """Why each internal link on the page fails to resolve. Empty when clean."""
    body = _strip_code_blocks(page.read_text(encoding="utf-8"))
    problems: list[str] = []
    for m in _INTERNAL_LINK.finditer(body):
        # Links are relative to the linking page's own directory, not the root.
        target = (page.parent / m.group("page")).resolve()
        # Containment is checked before existence: a `../` link that lands on a
        # real repo file outside the docs root must fail, not pass because the
        # file happens to exist.
        try:
            slug = target.relative_to(_DOCS_DIR).with_suffix("").as_posix()
        except ValueError:
            problems.append(f"{m.group(0)!r}: resolves outside {_DOCS_DIR}")
            continue
        if not target.is_file():
            if slug not in _PLANNED_PAGES:
                problems.append(f"{m.group(0)!r}: page does not exist")
            # A planned page has no headings yet — nothing to check its anchor against.
            continue
        anchor = m.group("anchor")
        if anchor and anchor not in _anchors(target):
            problems.append(
                f"{m.group(0)!r}: no '{{#{anchor}}}' or matching heading in {target.name}"
            )
    for m in _LOCAL_LINK.finditer(body):
        anchor = m.group("anchor")
        if anchor not in _anchors(page):
            problems.append(f"(#{anchor}): no such anchor on this page")
    return problems


@pytest.mark.parametrize("page", _pages(), ids=_page_id)
def test_internal_links_resolve(page: Path) -> None:
    """Every internal link on the page hits an existing page and anchor."""
    problems = _link_problems(page)
    assert not problems, f"{_page_id(page)}: " + "; ".join(problems)


def test_link_escaping_the_docs_root_is_rejected(tmp_path: Path, monkeypatch) -> None:
    """An existing file outside the root does not license a link to it."""
    root = tmp_path / "docs"
    (root / "sub").mkdir(parents=True)
    (tmp_path / "OUTSIDE.md").write_text("# outside\n", encoding="utf-8")
    page = root / "sub" / "page.md"
    page.write_text("[a](../../OUTSIDE.md)\n", encoding="utf-8")

    monkeypatch.setitem(globals(), "_DOCS_DIR", root)
    assert _link_problems(page) == [f"'](../../OUTSIDE.md': resolves outside {root}"]
