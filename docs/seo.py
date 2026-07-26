#!/usr/bin/env python3
"""Post-process the built mdBook site: inject canonical/OG/Twitter tags and
write a sitemap.xml.

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

TITLE_RE = re.compile(r"<title>(.*?)</title>", re.DOTALL)
DESC_RE = re.compile(r'<meta name="description" content="(.*?)">', re.DOTALL)
MAIN_RE = re.compile(r"<main>(.*?)</main>", re.DOTALL)
PARA_RE = re.compile(r"<p>(.*?)</p>", re.DOTALL)
TAG_RE = re.compile(r"<[^>]+>")
FALLBACK_TITLE = "Grimoire"
FALLBACK_DESC = "An OCI-backed package manager for AI skills and rules"


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


def process(path):
    """Inject tags if missing; always return this page's canonical URL."""
    rel = path.relative_to(BOOK_DIR)
    url = page_url(rel)
    html = path.read_text(encoding="utf-8")
    if 'rel="canonical"' in html:
        return url  # already tagged — idempotent, nothing to do

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
    urls = [
        process(path)
        for path in sorted(BOOK_DIR.rglob("*.html"))
        if path.name not in SKIP
    ]
    write_sitemap(urls)
    print(f"seo.py: processed {len(urls)} pages, wrote sitemap.xml")


if __name__ == "__main__":
    main()
