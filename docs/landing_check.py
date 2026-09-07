#!/usr/bin/env python3
"""WP-R acceptance assertions for C-023 and C-020's footer.

Five checks, exactly as the plan's Specify bullet states them:

1. `docs/dist/index.html` carries eight anchors whose `href` set equals the
   eight `routerCards` hrefs in `docs/src/data/landing.ts`.
2. It carries three more whose `href` set equals the three `entryPaths` hrefs.
3. No `<a>` is nested inside any of those eleven anchors — the anchor *is* the
   card's bounding element, so the whole card is the link.
4. `index.html` and `commands.html` each carry all five footer hrefs
   `docs/seo.py` used to inject, *as links inside the `.site-links` block*
   (`SiteFooter.astro`, rendered beside `<main>` so it is a `contentinfo`
   landmark)
   (Principle 9: they must not go missing when the landing stops hand-rolling
   its own footer). Presence anywhere in the file is not enough — PR 1 already
   emitted them into a landing that hid the whole footer with CSS, and a check
   that only greps the file text goes green on exactly that.
5. Each `pain` string appears zero times in `docs/src/components/*.astro` —
   the copy lives in `landing.ts` and nowhere else (C-022, one module one
   source).

Needs `docs/dist`, so run `task docs:build` (or `task docs:check`, which builds
first) before it. Exit 0 clean, 1 findings, matching the docs-quality checks.
"""

from __future__ import annotations

import argparse
import re
import sys
from html.parser import HTMLParser
from pathlib import Path

# The five links `seo.py`'s FOOTER_LINKS injected into every chapter page, in
# its order. `Footer.astro` is the only thing rendering them now.
FOOTER_HREFS = (
    "/index.html",
    "/introduction.html",
    "/stability.html",
    "/privacy.html",
    "https://github.com/grimoire-rs/grimoire/blob/main/LICENSE",
)

# The two classes the components put on the card anchors themselves, and the
# block `Footer.astro` wraps its own additions in.
ROUTER_CLASS = "router-card"
WAY_IN_CLASS = "way-in"
FOOTER_CLASS = "grim-footer"
# The five Principle 9 links live in their own block inside the footer.
# Scoping to it matters: `FooterNav` repeats the whole sidebar in the same
# footer, and three of the five appear there too, so a set comparison against
# the outer block passes even when the site-links block loses one.
SITE_LINKS_CLASS = "site-links"

# `href: "…"` inside the two exported arrays. Splitting the module text on the
# second `export const` keeps the two lists apart without importing TypeScript.
HREF_RE = re.compile(r'href:\s*"([^"]+)"')
PAIN_RE = re.compile(r'pain:\s*"([^"]+)"')


class CardAnchors(HTMLParser):
    """Collects the card anchors and flags any `<a>` nested inside one."""

    def __init__(self, wanted: str) -> None:
        super().__init__(convert_charrefs=True)
        self.wanted = wanted
        self.hrefs: list[str] = []
        self.nested = 0
        # Depth of open `<a>` elements. `<a>` cannot nest in valid HTML, so a
        # depth above 1 while inside a card is the failure this counts.
        self._depth = 0
        self._inside_card = False

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag != "a":
            return
        attr = dict(attrs)
        classes = (attr.get("class") or "").split()
        if self._depth > 0 and self._inside_card:
            self.nested += 1
        elif self.wanted in classes:
            self._inside_card = True
            self.hrefs.append(attr.get("href") or "")
        self._depth += 1

    def handle_endtag(self, tag: str) -> None:
        if tag != "a":
            return
        self._depth = max(0, self._depth - 1)
        if self._depth == 0:
            self._inside_card = False


class FooterLinks(HTMLParser):
    """Collects the hrefs of every `<a>` inside the `.site-links` block.

    The block's element is not pinned here — the footer moved from a `<div>`
    inside `<main>` to a `<footer>` beside it when the site regained its
    `contentinfo` landmark, and this check is about the five links, not the
    tag. Whichever element carries the class opens the block and its matching
    end tag closes it.

    Scoped to `.site-links` rather than the whole footer on purpose. The
    footer also renders the site nav, which repeats three of the five, so
    collecting from the outer block makes the assertion pass on a footer that
    has lost one of them.
    """

    def __init__(self, css_class: str = SITE_LINKS_CLASS) -> None:
        super().__init__(convert_charrefs=True)
        self._class = css_class
        self.hrefs: list[str] = []
        # None until the block opens, then the nesting depth inside it.
        self._depth: int | None = None
        self._tag: str | None = None

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        attr = dict(attrs)
        if self._depth is None:
            if self._class in (attr.get("class") or "").split():
                self._depth = 1
                self._tag = tag
            return
        if tag == self._tag:
            self._depth += 1
        if tag == "a" and attr.get("href"):
            self.hrefs.append(attr["href"])

    def handle_endtag(self, tag: str) -> None:
        if self._depth is None or tag != self._tag:
            return
        self._depth -= 1
        if self._depth == 0:
            # Block closed; stop collecting, keep what was found.
            self._depth = None


def footer_hrefs(html: str) -> list[str]:
    parser = FooterLinks()
    parser.feed(html)
    return parser.hrefs


def card_hrefs(html: str, css_class: str) -> tuple[list[str], int]:
    parser = CardAnchors(css_class)
    parser.feed(html)
    return parser.hrefs, parser.nested


def landing_lists(module: Path) -> tuple[list[str], list[str], list[str]]:
    """Return (entryPaths hrefs, routerCards hrefs, routerCards pain strings)."""
    text = module.read_text(encoding="utf-8")
    head, sep, tail = text.partition("export const routerCards")
    if not sep:
        raise SystemExit(f"landing.ts has no routerCards export: {module}")
    return HREF_RE.findall(head), HREF_RE.findall(tail), PAIN_RE.findall(tail)


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(add_help=True)
    ap.add_argument("--root", default=".")
    args = ap.parse_args(argv)

    root = Path(args.root).resolve()
    dist = root / "docs" / "dist"
    index = dist / "index.html"
    commands = dist / "commands.html"
    for page in (index, commands):
        if not page.is_file():
            print(f'FAIL missing {page} — run "task docs:build" first')
            return 1

    entry_hrefs, router_hrefs, pains = landing_lists(
        root / "docs" / "src" / "data" / "landing.ts"
    )
    index_html = index.read_text(encoding="utf-8")
    fails = 0

    def check(name: str, ok: bool, detail: str = "") -> None:
        nonlocal fails
        if ok:
            print(f"ok   {name}")
        else:
            fails += 1
            print(f"FAIL {name}{(' — ' + detail) if detail else ''}")

    # 1 + 2 — the anchor sets, compared against the module rather than a copy.
    found_router, nested_router = card_hrefs(index_html, ROUTER_CLASS)
    check(
        f"C-023 index.html has 8 .{ROUTER_CLASS} anchors",
        len(found_router) == 8,
        f"found {len(found_router)}",
    )
    check(
        f"C-023 .{ROUTER_CLASS} href set equals routerCards",
        set(found_router) == set(router_hrefs),
        f"only in page {sorted(set(found_router) - set(router_hrefs))}, "
        f"only in landing.ts {sorted(set(router_hrefs) - set(found_router))}",
    )

    found_entry, nested_entry = card_hrefs(index_html, WAY_IN_CLASS)
    check(
        f"C-023 index.html has 3 .{WAY_IN_CLASS} anchors",
        len(found_entry) == 3,
        f"found {len(found_entry)}",
    )
    check(
        f"C-023 .{WAY_IN_CLASS} href set equals entryPaths",
        set(found_entry) == set(entry_hrefs),
        f"only in page {sorted(set(found_entry) - set(entry_hrefs))}, "
        f"only in landing.ts {sorted(set(entry_hrefs) - set(found_entry))}",
    )

    # 3 — the whole card is the link: nothing anchor-shaped inside a card.
    check(
        "C-023 no <a> nested inside a card anchor",
        nested_router == 0 and nested_entry == 0,
        f"{nested_router + nested_entry} nested",
    )

    # 4 — the Principle 9 footer set, on the landing and on a chapter page.
    for page, html in ((index, index_html), (commands, commands.read_text("utf-8"))):
        rendered = set(footer_hrefs(html))
        missing = [h for h in FOOTER_HREFS if h not in rendered]
        check(
            f"C-020 {page.name} links all five footer hrefs from .{SITE_LINKS_CLASS}",
            not missing,
            f"missing {missing}",
        )

    # 5 — the pain copy never leaves the data module.
    components = sorted((root / "docs" / "src" / "components").glob("*.astro"))
    component_text = "\n".join(p.read_text(encoding="utf-8") for p in components)
    leaked = [p for p in pains if p in component_text]
    check(
        "C-022 no pain string appears in docs/src/components/*.astro",
        not leaked,
        f"leaked {leaked}",
    )

    print(f"\n{fails} failing check(s)")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
