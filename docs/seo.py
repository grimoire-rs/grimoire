#!/usr/bin/env python3
"""Post-process the built mdBook site: stamp the crate version, inject
canonical/OG/Twitter tags, and write a sitemap.xml.

mdBook 0.5.3's Handlebars context registers no string helpers, so a theme
partial has no way to turn `introduction.md` (the only path it sees) into
`introduction.html`. That rules out doing this at template-render time —
instead we run after `mdbook build` and read the tags back out of the
already-rendered HTML, where mdBook itself resolved everything correctly.
Wired in via taskfiles/docs.taskfile.yml (`task docs:build`).
"""
import re
from pathlib import Path
from xml.sax.saxutils import escape

# Canonical public origin (no trailing slash). Must match the Host used in
# docs/src/robots.txt.
SITE = "https://grimoire.rs"
OG_IMAGE = f"{SITE}/og-card.png"  # produced by a separate worker at docs/src/og-card.png

BOOK_DIR = Path(__file__).parent / "book"
# print.html already carries its own noindex and must not get a competing
# canonical; 404.html and toc.html are not indexable content pages either.
SKIP = {"404.html", "print.html", "toc.html"}

CARGO_TOML = Path(__file__).parent.parent / "Cargo.toml"
# The landing page used to carry the version as a literal, and it went stale
# the first time nobody remembered to edit it by hand. Any element tagged
# `data-grim-version` has its text replaced with the crate's version at build
# time, so a release bump reaches the site with no second edit.
VERSION_RE = re.compile(r"(<[^>]*\bdata-grim-version\b[^>]*>)[^<]*(</)")
CARGO_VERSION_RE = re.compile(r'^version\s*=\s*"([^"]+)"', re.MULTILINE)

# The chapter pages get their footer injected here rather than in the theme,
# because the `{{else}}` branch of theme/index.hbs is mdBook's stock template
# kept byte for byte so an upgrade can be diffed against `mdbook init --theme`.
# Editing it to add a footer would quietly end that. The landing page and the
# standalone pages carry their own footer in source and are skipped.
FOOTER_LINKS = (
    ("{root}index.html", "home"),
    ("{root}introduction.html", "documentation"),
    ("{root}stability.html", "stability"),
    ("{root}privacy.html", "privacy"),
    ("https://github.com/grimoire-rs/grimoire/blob/main/LICENSE", "Apache-2.0"),
)
FOOTER_STYLE = (
    "margin:3rem 0 1rem; padding-top:1.2rem; border-top:1px solid var(--table-border-color);"
    "display:flex; flex-wrap:wrap; gap:0.6rem 2rem; align-items:baseline;"
    "font-size:0.85em; opacity:0.75"
)

TITLE_RE = re.compile(r"<title>(.*?)</title>", re.DOTALL)
DESC_RE = re.compile(r'<meta name="description" content="(.*?)">', re.DOTALL)
MAIN_RE = re.compile(r"<main>(.*?)</main>", re.DOTALL)
PARA_RE = re.compile(r"<p>(.*?)</p>", re.DOTALL)
TAG_RE = re.compile(r"<[^>]+>")
FALLBACK_TITLE = "Grimoire"
FALLBACK_DESC = "An OCI-backed package manager for AI skills and rules"


def crate_version():
    """The `[package] version` from Cargo.toml.

    Read off the first `version = "…"` in the file, which is the package's own
    — dependency versions all live under `[dependencies]` further down, and
    `[package]` is the first table.
    """
    match = CARGO_VERSION_RE.search(CARGO_TOML.read_text(encoding="utf-8"))
    if not match:
        raise SystemExit(f"seo.py: no version found in {CARGO_TOML}")
    return match.group(1)


def add_footer(html, rel_path):
    """Append the site footer to a chapter page.

    A page that already has a footer in source (the landing page, start.html,
    privacy.html) is left alone, and so is anything without a `<main>` — that
    is the marker of the stock chapter template.
    """
    if "<footer" in html or "</main>" not in html:
        return html
    root = "../" * (len(rel_path.parts) - 1)
    links = "".join(
        f'<a href="{href.format(root=root)}">{label}</a>' for href, label in FOOTER_LINKS
    )
    footer = (
        f'</main>\n<footer style="{FOOTER_STYLE}">{links}'
        "<span>artifacts live in your registry, not ours</span></footer>"
    )
    return html.replace("</main>", footer, 1)


def page_url(rel_path):
    if rel_path.name == "index.html":
        return SITE + "/"
    return "/".join([SITE, *rel_path.parts])


def attr(text):
    """Make already-escaped rendered HTML safe inside a double-quoted attribute.

    The text comes straight out of mdBook's output, so `&`, `<` and `>` are
    already entities — re-escaping them would double them up. Only the quote
    that would close the attribute is left to handle.
    """
    return text.replace('"', "&quot;")


def chapter_summary(html):
    """First real paragraph of the chapter, for a per-page description.

    book.toml carries one description for the whole book, so without this every
    chapter would unfurl in Slack with identical text. Returns None when there
    is nothing usable and the caller should fall back to the book description.
    """
    main = MAIN_RE.search(html)
    if not main:
        return None
    for para in PARA_RE.finditer(main.group(1)):
        text = " ".join(TAG_RE.sub("", para.group(1)).split())
        # Short leading fragments are captions and admonition labels, not prose.
        if len(text) > 80:
            return text[:197].rsplit(" ", 1)[0] + "…" if len(text) > 200 else text
    return None


def process(path, version):
    """Stamp the version, inject tags if missing, return the canonical URL."""
    rel = path.relative_to(BOOK_DIR)
    url = page_url(rel)
    original = path.read_text(encoding="utf-8")
    # Runs before the canonical early-return below, so a re-run over an
    # already-tagged build still corrects a stale version.
    html = VERSION_RE.sub(rf"\g<1>{version}\g<2>", original)
    html = add_footer(html, rel)

    if 'rel="canonical"' in html:
        if html != original:
            path.write_text(html, encoding="utf-8")
        return url  # already tagged — idempotent otherwise

    title_match = TITLE_RE.search(html)
    title = title_match.group(1) if title_match else FALLBACK_TITLE
    desc_match = DESC_RE.search(html)
    description = desc_match.group(1) if desc_match else FALLBACK_DESC
    if not title_match:
        print(f"warning: {rel} has no <title>, using fallback")
    if not desc_match:
        print(f"warning: {rel} has no description meta, using fallback")

    og_type = "website" if url == SITE + "/" else "article"
    # The root is the product pitch, so the book description is the right blurb
    # for it; chapters describe themselves.
    if og_type == "article":
        description = chapter_summary(html) or description
    title, description = attr(title), attr(description)
    tags = f"""<link rel="canonical" href="{url}">
<meta property="og:type" content="{og_type}">
<meta property="og:title" content="{title}">
<meta property="og:description" content="{description}">
<meta property="og:url" content="{url}">
<meta property="og:image" content="{OG_IMAGE}">
<meta property="og:site_name" content="Grimoire">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="{title}">
<meta name="twitter:description" content="{description}">
<meta name="twitter:image" content="{OG_IMAGE}">
</head>"""
    path.write_text(html.replace("</head>", tags, 1), encoding="utf-8")
    return url


def write_sitemap(urls):
    urls = [SITE + "/"] + [u for u in urls if u != SITE + "/"]  # root first
    body = "\n".join(f"  <url><loc>{escape(u)}</loc></url>" for u in urls)
    xml = (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
        f"{body}\n</urlset>\n"
    )
    (BOOK_DIR / "sitemap.xml").write_text(xml, encoding="utf-8")


def main():
    version = crate_version()
    urls = [
        process(path, version)
        for path in sorted(BOOK_DIR.rglob("*.html"))
        if path.name not in SKIP
    ]
    write_sitemap(urls)
    print(f"seo.py: processed {len(urls)} pages at v{version}, wrote sitemap.xml")


if __name__ == "__main__":
    main()
