#!/usr/bin/env python3
"""URL and asset checks for the docs-quality rule set (C-010).

Checks covered (C-010 items 1-6, plus C-009 item 7):
  1. check_static_paths   DOC-URL-01  every declared static asset exists in the built dist
  2. check_schemas        DOC-URL-02  every published schema's canonical $id matches its dist path
  3. check_fragments      DOC-URL-03  every `{#custom-id}` in the source tree resolves to an
                                      `id=` in the built page
  4. check_catalog_links  DOC-URL-04  every `https://grimoire.rs/*.html` link in `catalog/**`
                                      and `README.md` resolves in the built tree
  5. check_no_mdx         DOC-URL-05  no stray .mdx source file (the Starlight source tree is
                                      Markdown only)
  6. check_sidebar_coverage
                          DOC-URL-06  every source page sits in exactly one sidebar group
                                      of `docs/astro.config.mjs`, and every sidebar slug
                                      resolves to a page on disk
  7. check_internal_hrefs DOC-URL-07  every internal `href` in the built tree is the site
                                      root, a `.html` page, or a known asset extension,
                                      and a `#frag` on one resolves to an `id=` there
  8. check_sitemap        DOC-URL-08  the sitemap names the site root and both
                                      hand-written pages under docs/public/ (C-009)

Usage:
  check_urls.py [--root DIR] [--dist DIR] [--format text|json]

Exit codes: 0 clean, 1 findings, 2 usage or missing input.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

RULE_PATH = "DOC-URL-01"
RULE_SCHEMA = "DOC-URL-02"
RULE_FRAGMENT = "DOC-URL-03"
RULE_CATALOG = "DOC-URL-04"
RULE_MDX = "DOC-URL-05"
RULE_SIDEBAR = "DOC-URL-06"
RULE_HREF = "DOC-URL-07"
RULE_SITEMAP = "DOC-URL-08"

SITE = "https://grimoire.rs"

# The 21 chapters of the site, in nav order. Not a runtime assert —
# see the module docstring's C-010 item list for what reads this.
CHAPTERS = (
    "introduction",
    "installation",
    "quickstart",
    "concepts",
    "clients",
    "commands",
    "configuration",
    "authentication",
    "package-index",
    "hosting-an-index",
    "ratings",
    "publishing",
    "ci",
    "self-hosted-gitlab",
    "agents",
    "mcp-servers",
    "artifacts",
    "vendor-metadata",
    "json-interface",
    "stability",
    "upgrading",
)  # 21 entries

# Relative to the dist root (leading slash stripped).
STATIC_PATHS = (
    "404.html",
    "og-card.png",
    "robots.txt",
    "sitemap.xml",
    "install.sh",
    "install.ps1",
    "favicon.png",
    "favicon.svg",
    "demo.cast",
    "start.html",
    "privacy.html",
)

# The sitemap entries no Astro route produces: the site root (the landing's
# canonical URL) and the two pages served straight out of docs/public/.
SITEMAP_URLS = (f"{SITE}/", f"{SITE}/start.html", f"{SITE}/privacy.html")

# Canonical $id is f"{SITE}/schemas/{name}.schema.json"; the file lives at
# <dist>/schemas/<name>.schema.json.
SCHEMAS = ("grimoire-config", "grim-mcp", "grim-publish", "grimoire-lock")


# A fenced block is a markdown sample, so a `{#id}` inside one never becomes a
# heading id in the build. The closing run is matched by backreference, which
# also swallows a longer closing fence.
FENCE_RE = re.compile(r"^ {0,3}(`{3,}|~{3,}).*?^ {0,3}\1", re.MULTILINE | re.DOTALL)
CUSTOM_ID_RE = re.compile(r"\{#([A-Za-z0-9_-]+)\}")
DOCS_URL_RE = re.compile(
    r"https://grimoire\.rs/([A-Za-z0-9._/-]+)\.html(?:#([A-Za-z0-9_.-]+))?"
)
SIDEBAR_RE = re.compile(r"\bsidebar:\s*\[")
SLUG_RE = re.compile(r"\bslug:\s*['\"]([^'\"]+)['\"]")
HREF_RE = re.compile(r'href="([^"]*)"')

# Every non-page extension the site serves, from `docs/public/` plus what the
# build emits under `_astro/` and `pagefind/`. Deliberately an allowlist and
# not "the path has a dot": `/commands.htm` has a dot, and is a dead link.
ASSET_SUFFIXES = frozenset(
    """cast css ico js json map png ps1 sh svg txt wasm webmanifest woff woff2 xml""".split()
)

# `is_file()` passes on the ~130-byte ASCII stub Git LFS leaves when the payload
# was never fetched, so a static image is checked by its first bytes instead.
LFS_POINTER = b"version https://git-lfs.github.com/spec/v1"
PNG_MAGIC = b"\x89PNG\r\n\x1a\n"
BOM = b"\xef\xbb\xbf"


def _finding(path: Path, rule: str, message: str, line: int = 1) -> dict:
    """One finding, in the shape the text and JSON renderers both read."""
    return {"page": str(path), "line": line, "rule": rule, "message": message}


def _read(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace")


def _has_id(html: str, fragment: str) -> bool:
    """Astro emits double quotes; accept single so a hand-written page passes.

    The lookbehind is what keeps `data-id="x"` out. `\\b` would not: it matches
    between the hyphen and the `i`, so the attribute this guard exists to
    reject would satisfy it.
    """
    return re.search(rf'(?<![-\w])id=["\']{re.escape(fragment)}["\']', html) is not None


def _asset_finding(path: Path) -> dict | None:
    """A static image that is not the image — an unfetched Git LFS pointer, or
    a file whose first bytes are not its format's."""
    with path.open("rb") as handle:
        head = handle.read(64)
    if head.startswith(LFS_POINTER):
        return _finding(path, RULE_PATH, "a Git LFS pointer, not the file itself")
    if path.suffix == ".png" and not head.startswith(PNG_MAGIC):
        return _finding(path, RULE_PATH, "not a PNG: no PNG signature")
    if path.suffix == ".svg" and not head.removeprefix(BOM).lstrip().startswith(b"<"):
        return _finding(path, RULE_PATH, "not SVG: does not open with '<'")
    return None


def check_static_paths(dist: Path) -> list[dict]:
    """C-010 item 1 (DOC-URL-01): every STATIC_PATHS entry exists under dist."""
    out: list[dict] = []
    for name in (*STATIC_PATHS, *(f"{chapter}.html" for chapter in CHAPTERS)):
        path = dist / name
        if not path.is_file():
            out.append(_finding(path, RULE_PATH, "not found"))
        elif path.suffix in (".png", ".svg"):
            finding = _asset_finding(path)
            if finding:
                out.append(finding)
    return out


def check_sitemap(dist: Path) -> list[dict]:
    """C-009 (DOC-URL-08): the sitemap names the root and both static pages.

    `@astrojs/sitemap` only sees Astro routes, so `start.html` and
    `privacy.html` — hand-written, served from `docs/public/` — reach it only
    through the integration's `customPages`. Drop that key and they leave the
    sitemap with nothing else failing. The root is here because the landing's
    canonical must agree with it.
    """
    path = dist / "sitemap.xml"
    if not path.is_file():
        return [_finding(path, RULE_SITEMAP, "not found")]
    xml = _read(path)
    return [
        _finding(path, RULE_SITEMAP, f"{url} missing from the sitemap")
        for url in SITEMAP_URLS
        if f"<loc>{url}</loc>" not in xml
    ]


def check_schemas(dist: Path) -> list[dict]:
    """C-010 item 2 (DOC-URL-02): every schema's $id matches its dist path."""
    out: list[dict] = []
    for name in SCHEMAS:
        path = dist / "schemas" / f"{name}.schema.json"
        canonical = f"{SITE}/schemas/{name}.schema.json"
        if not path.is_file():
            out.append(_finding(path, RULE_SCHEMA, "not found"))
            continue
        try:
            data = json.loads(_read(path))
        except json.JSONDecodeError as exc:
            # A traceback here would exit 1 with no page named — the whole point
            # of the exit-code contract is that a finding says which file broke.
            out.append(_finding(path, RULE_SCHEMA, f"not valid JSON: {exc}"))
            continue
        found = data.get("$id") if isinstance(data, dict) else None
        if found != canonical:
            out.append(
                _finding(path, RULE_SCHEMA, f"$id is {found!r}, expected {canonical}")
            )
    return out


def check_fragments(src: Path, dist: Path) -> list[dict]:
    """C-010 item 3 (DOC-URL-03): every in-repo link fragment resolves in dist."""
    out: list[dict] = []
    for source in sorted(src.rglob("*.md")):
        page = dist / source.relative_to(src).with_suffix(".html")
        if not page.is_file():
            out.append(_finding(source, RULE_FRAGMENT, f"built page not found: {page}"))
            continue
        # Each fenced block becomes its own newlines, so a fragment's line
        # number survives the strip. First occurrence of a repeated id wins.
        text = FENCE_RE.sub(lambda m: "\n" * m.group(0).count("\n"), _read(source))
        fragments: dict[str, int] = {}
        for match in CUSTOM_ID_RE.finditer(text):
            fragments.setdefault(match.group(1), text[: match.start()].count("\n") + 1)
        if not fragments:
            continue
        html = _read(page)
        out += [
            _finding(
                source,
                RULE_FRAGMENT,
                f"fragment #{fragment} has no id in {page}",
                line,
            )
            for fragment, line in fragments.items()
            if not _has_id(html, fragment)
        ]
    return out


def check_catalog_links(root: Path, dist: Path) -> list[dict]:
    """C-010 item 4 (DOC-URL-04): every catalog package links to a real docs page."""
    # Every file, not only *.md: publish.toml's `documentation = "…"` ships to
    # the registry as package metadata. The whole tree is text today, and _read
    # replaces undecodable bytes, so no format filter is needed.
    sources = sorted(p for p in (root / "catalog").rglob("*") if p.is_file())
    sources.append(root / "README.md")  # main() has already proved it exists

    out: list[dict] = []
    for source in sources:
        seen: set[str] = set()
        text = _read(source)
        # Fences are not stripped here: a live URL is a live URL wherever it is
        # printed, and a catalog sample that names a dead page is still link rot.
        for match in DOCS_URL_RE.finditer(text):
            url, name, fragment = match.group(0), match.group(1), match.group(2)
            if url in seen:
                continue
            seen.add(url)
            line = text[: match.start()].count("\n") + 1
            page = dist / f"{name}.html"
            if not page.is_file():
                out.append(
                    _finding(
                        source, RULE_CATALOG, f"{url} -> {page} not found", line
                    )
                )
            elif fragment and not _has_id(_read(page), fragment):
                out.append(
                    _finding(
                        source,
                        RULE_CATALOG,
                        f'{url} -> no id="{fragment}" in {page}',
                        line,
                    )
                )
    return out


def check_no_mdx(src: Path) -> list[dict]:
    """C-010 item 5 (DOC-URL-05): no stray .mdx source file (source is .md only)."""
    return [
        _finding(path, RULE_MDX, "the Starlight source tree is Markdown only")
        for path in sorted(src.rglob("*.mdx"))
    ]


def _balanced(text: str, start: int) -> str:
    """The bracket run opening at `start`, up to its matching close.

    Technique shared with `nav_depth.py`'s `starlight_nav()`: the Starlight
    config is JavaScript, so the sidebar is located by walking brackets rather
    than by parsing the module.
    """
    open_ch = text[start]
    close_ch = {"{": "}", "[": "]"}[open_ch]
    level = 0
    for i in range(start, len(text)):
        if text[i] == open_ch:
            level += 1
        elif text[i] == close_ch:
            level -= 1
            if level == 0:
                return text[start : i + 1]
    return text[start:]


def _sidebar_groups(text: str) -> list[list[str]] | None:
    """Each top-level sidebar group as its list of slugs, or None if no sidebar.

    Each group's own braces are skipped whole, so a nested `items:` array
    never reads as a second group.
    """
    match = SIDEBAR_RE.search(text)
    if not match:
        return None
    block = _balanced(text, match.end() - 1)
    groups: list[list[str]] = []
    cursor = 0
    while (start := block.find("{", cursor)) != -1:
        group = _balanced(block, start)
        groups.append(SLUG_RE.findall(group))
        cursor = start + len(group)
    return groups


def check_sidebar_coverage(src: Path, config: Path) -> list[dict]:
    """C-010 item 6 (DOC-URL-06): pages and sidebar groups cover each other.

    Every page under the source tree sits in exactly one sidebar group, and
    every sidebar slug resolves to a page on disk.
    """
    groups = _sidebar_groups(_read(config))
    if groups is None:
        message = "no sidebar array in the Starlight config"
        return [_finding(config, RULE_SIDEBAR, message)]

    listed: dict[str, int] = {}
    for group in groups:
        for slug in group:
            listed[slug] = listed.get(slug, 0) + 1

    out: list[dict] = []
    for source in sorted(src.rglob("*.md")):
        slug = source.relative_to(src).with_suffix("").as_posix()
        count = listed.pop(slug, 0)
        if count == 0:
            out.append(_finding(source, RULE_SIDEBAR, f"{slug} is in no sidebar group"))
        elif count > 1:
            out.append(
                _finding(source, RULE_SIDEBAR, f"{slug} is in {count} sidebar groups")
            )
    # Whatever is left was never claimed by a page on disk.
    out += [
        _finding(config, RULE_SIDEBAR, f"sidebar slug {slug} has no page under {src}")
        for slug in sorted(listed)
    ]
    return out


def check_internal_hrefs(dist: Path) -> list[dict]:
    """C-010 item 7 (DOC-URL-07): every internal href resolves to a real file.

    Under `build.format: 'file'` the emitted page is `/page.html`, so an
    internal href of `/page` names nothing on disk. The rewrite plugin
    resolves against Astro's *route* manifest and cannot append the suffix
    itself, so `astro.config.mjs` carries a stage that does — and this is the
    gate proving that stage is still in the chain and still correct.
    """
    out: list[dict] = []
    # One read per target page, not per href: 735 hrefs resolve onto 24 pages.
    targets: dict[Path, str | None] = {}
    for page in sorted(dist.rglob("*.html")):
        seen: set[str] = set()
        for href in HREF_RE.findall(_read(page)):
            # A scheme (`https:`, `mailto:`), a protocol-relative `//host/x`
            # and a bare `#frag` are all somebody else's business.
            if not href.startswith("/") or href.startswith("//"):
                continue
            # Split on the path only: `/commands#v1.2` carries a dot in its
            # fragment, and reading that as an extension passes a dead link.
            path, _, fragment = href.partition("#")
            path = path.split("?", 1)[0]
            if path == "/" or path.endswith("/") or href in seen:
                continue
            seen.add(href)
            suffix = path.rsplit("/", 1)[-1].rpartition(".")[2].lower()
            if suffix != "html" and suffix not in ASSET_SUFFIXES:
                out.append(
                    _finding(
                        page,
                        RULE_HREF,
                        f"internal href {href} is neither a .html page nor a known asset",
                    )
                )
                continue
            if not fragment or suffix != "html":
                continue
            # The suffix stage costs `starlight-links-validator` its hash half:
            # it keys headings on the route, so an href of `page.html` matches
            # nothing and the hash branch never runs. C-011's build-time
            # guarantee for the 290 `./page.md#frag` links lives here instead.
            target = dist / path.lstrip("/")
            if target not in targets:
                targets[target] = _read(target) if target.is_file() else None
            html = targets[target]
            # A page this slice does not emit is item 1's finding, not this
            # one's — 22 pages link `/index.html`, which WP-G ships.
            if html is not None and not _has_id(html, fragment):
                out.append(
                    _finding(
                        page,
                        RULE_HREF,
                        f"internal href {href} has no id=\"{fragment}\" in {target}",
                    )
                )
    return out


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(add_help=True)
    ap.add_argument("--root", default=".")
    ap.add_argument("--dist", default=None)
    ap.add_argument("--format", choices=("text", "json"), default="text")
    args = ap.parse_args(argv)

    root = Path(args.root)
    dist = Path(args.dist) if args.dist else root / "docs" / "dist"

    src = root / "docs" / "src" / "content" / "docs"
    catalog = root / "catalog"
    readme = root / "README.md"
    config = root / "docs" / "astro.config.mjs"
    for path in (src, catalog, dist):
        if not path.is_dir():
            print(f"missing input: {path}", file=sys.stderr)
            return 2
    for path in (readme, config):
        if not path.is_file():
            print(f"missing input: {path}", file=sys.stderr)
            return 2

    findings: list[dict] = []
    findings += check_static_paths(dist)
    findings += check_sitemap(dist)
    findings += check_schemas(dist)
    findings += check_fragments(src, dist)
    findings += check_catalog_links(root, dist)
    findings += check_no_mdx(src)
    findings += check_sidebar_coverage(src, config)
    findings += check_internal_hrefs(dist)

    if args.format == "json":
        print(json.dumps(findings, indent=2))
    else:
        for f in findings:
            print(f"{f['page']}:{f['line']}: [{f['rule']}] {f['message']}")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
