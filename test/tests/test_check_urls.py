# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Contract tests for ``docs/check_urls.py`` (C-010) and ``nav_depth.py``'s
Starlight branch (D-9).

``check_urls.py`` is the URL contract of the built docs site: every declared
static path, every schema ``$id``, every source fragment, every catalog link
back to a page, and the absence of MDX under the source tree.  Its **exit
code** is the contract — 0 clean, 1 findings, 2 usage or missing input — so
each case drives the script as a subprocess and asserts on the exit code
*and* on what the message names.  An exit code alone would pass against a
script that merely crashes.

The source tree ``docs/src/content/docs/`` does not exist in this repo yet
(a later work package creates it), so every case builds its own repo-shaped
tree under ``tmp_path`` and never points the script at the live worktree.

Expectations are hard-coded below instead of imported from the module under
test: a test that reads its expectations from the implementation asserts
nothing.
"""
from __future__ import annotations

import json
import shutil
import subprocess
import sys
from collections.abc import Iterator
from pathlib import Path
from types import ModuleType

import pytest

from src.helpers import PROJECT_ROOT

_CHECK_URLS = PROJECT_ROOT / "docs" / "check_urls.py"
_CHECKS = PROJECT_ROOT / ".claude" / "rules" / "docs-quality" / "checks"
_NAV_FIXTURE = _CHECKS / "fixtures" / "nav_depth" / "pass-nav-starlight"

# The 21 chapters of the site, in nav order (C-010 item 1).
_CHAPTERS = (
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
)

# Static assets served from the site root (C-010 item 1).
_STATIC_PATHS = (
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
    "casts.js",
    "asciinema-player.min.js",
    "asciinema-player.css",
)

# Published schemas; each `$id` is its own canonical URL (C-010 item 2).
_SCHEMAS = ("grimoire-config", "grim-mcp", "grim-publish", "grimoire-lock")

# C-009: the sitemap entries no Astro route produces. Mirrors
# `check_urls.SITEMAP_URLS`.
_SITEMAP_URLS = (
    "https://grimoire.rs/",
    "https://grimoire.rs/start.html",
    "https://grimoire.rs/privacy.html",
)

_SITE = "https://grimoire.rs"

# Real bytes, because a static image is checked by its magic bytes, not by
# `is_file()` — a text stub would fail the clean-tree case for the wrong reason.
_PNG_STUB = b"\x89PNG\r\n\x1a\nstub\n"
_SVG_STUB = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"/>\n'

# What Git LFS leaves in the worktree when the payload was never fetched.
_LFS_POINTER = (
    "version https://git-lfs.github.com/spec/v1\n"
    "oid sha256:0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0\n"
    "size 131\n"
)

# The four sidebar groups, covering all 21 chapters exactly once (C-010 item
# 6).  Hard-coded here, mirroring `docs/astro.config.mjs`: a fixture that
# derived its grouping from the module under test would assert nothing.
_SIDEBAR: tuple[tuple[str, tuple[str, ...]], ...] = (
    ("Getting started", ("introduction", "installation", "quickstart", "concepts")),
    ("Guides", ("clients",)),
    (
        "Teams and automation",
        (
            "publishing",
            "ci",
            "hosting-an-index",
            "self-hosted-gitlab",
            "authentication",
            "ratings",
        ),
    ),
    (
        "Reference",
        (
            "commands",
            "configuration",
            "artifacts",
            "agents",
            "mcp-servers",
            "vendor-metadata",
            "package-index",
            "json-interface",
            "stability",
            "upgrading",
        ),
    ),
)


def _astro_config(sidebar: tuple[tuple[str, tuple[str, ...]], ...] = _SIDEBAR) -> str:
    """Render a minimal but structurally faithful Starlight config.

    Item 6 reads the ``sidebar`` array out of a
    ``defineConfig({ integrations: [ starlight({ … }) ] })`` shell, so that
    shell is reproduced; the plugins, head tags and markdown chain of the real
    config are not, because no assertion here reads them.
    """
    groups = ",\n".join(
        "        {\n"
        f"          label: '{label}',\n"
        "          items: [\n"
        + "".join(
            f"            {{ slug: '{slug}', label: '{slug}' }},\n" for slug in slugs
        )
        + "          ],\n"
        "        }"
        for label, slugs in sidebar
    )
    return (
        "// @ts-check\n"
        "import { defineConfig } from 'astro/config';\n"
        "import starlight from '@astrojs/starlight';\n"
        "\n"
        "export default defineConfig({\n"
        "  site: 'https://grimoire.rs',\n"
        "  build: { format: 'file' },\n"
        "  trailingSlash: 'never',\n"
        "  integrations: [\n"
        "    starlight({\n"
        "      title: 'Grimoire',\n"
        "      sidebar: [\n"
        f"{groups},\n"
        "      ],\n"
        "    }),\n"
        "  ],\n"
        "});\n"
    )


# One href of every bucket item 7 classifies, on every built page.  A fixture
# whose pages carry no links at all would satisfy item 7 vacuously, so the
# clean tree has to exercise the classifier rather than starve it.
_NAV = (
    '<a href="/introduction.html">page</a>\n'
    # Points at an id `_clean_tree` really writes: item 7 resolves a cross-page
    # fragment, so a made-up one here would fail the clean tree.
    '<a href="/configuration.html#registry-compatibility">deep link</a>\n'
    '<a href="/og-card.png">asset</a>\n'
    '<a href="https://example.test/x">external</a>\n'
    '<a href="//cdn.example.test/lib">protocol-relative</a>\n'
    '<a href="#local">fragment only</a>\n'
    '<a href="/">root</a>'
)


def _html(title: str, body: str = "") -> str:
    """A minimal built page, carrying one href of every classified bucket."""
    return (
        '<!doctype html>\n<html lang="en">\n'
        f"<head><meta charset=\"utf-8\"><title>{title}</title></head>\n"
        f"<body>\n<h1>{title}</h1>\n{_NAV}\n{body}\n</body>\n</html>\n"
    )


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _clean_tree(root: Path) -> Path:
    """Build a complete, clean, repo-shaped tree and return its root.

    It satisfies every C-010 assertion: all 21 chapters and 11 static paths
    in ``docs/dist``, four schemas whose ``$id`` matches their own path, a
    source fragment that resolves in the built page, a catalog link and a
    README link that both hit real pages, no ``.mdx`` anywhere, and an
    ``astro.config.mjs`` whose four sidebar groups carry each of the 21
    chapters exactly once.
    """
    dist = root / "docs" / "dist"
    src = root / "docs" / "src" / "content" / "docs"

    # C-010 item 6: the nav side of the disk/nav pairing.  `main()` treats a
    # missing config as a missing input, so the clean tree has to carry one.
    _write(root / "docs" / "astro.config.mjs", _astro_config())

    for chapter in _CHAPTERS:
        _write(dist / f"{chapter}.html", _html(chapter))
        _write(
            src / f"{chapter}.md",
            "---\n"
            f"title: {chapter}\n"
            f"description: The {chapter} page.\n"
            "---\n"
            "<!-- doc_type: reference -->\n\n"
            f"# {chapter}\n",
        )

    for name in _STATIC_PATHS:
        if name.endswith(".png"):
            (dist / name).parent.mkdir(parents=True, exist_ok=True)
            (dist / name).write_bytes(_PNG_STUB)
        elif name.endswith(".svg"):
            _write(dist / name, _SVG_STUB)
        else:
            _write(dist / name, _html(name) if name.endswith(".html") else f"{name}\n")

    # C-010 item 7: three shapes no other fixture line carries, so the branches
    # that read them are exercised rather than starved — a recording mounted
    # through `data-cast`, its `<noscript>` href, and the relative links the two
    # pages copied verbatim out of `docs/public/` use throughout.
    _write(dist / "casts" / "quickstart.cast", '{"version": 2}\n')
    _write(
        dist / "quickstart.html",
        _html(
            "quickstart",
            '<div data-cast="/casts/quickstart.cast" data-cast-poster="npt:0:03"></div>\n'
            '<noscript><a href="/casts/quickstart.cast">Download the recording</a></noscript>',
        ),
    )
    _write(
        dist / "start.html",
        _html(
            "start",
            '<a href="introduction.html">relative page link</a>\n'
            '<a href="configuration.html#registry-compatibility">relative deep link</a>',
        ),
    )

    # C-010 item 4: the binary prints docs URLs of its own, so `src/` is a
    # checked input beside `catalog/`.
    _write(
        root / "src" / "catalog" / "registry_catalog.rs",
        "pub const REGISTRY_COMPAT_DOCS_URL: &str =\n"
        f'    "{_SITE}/configuration.html#registry-compatibility";\n',
    )

    # C-010 item 7 / C-009: overwrites the placeholder the static-path loop
    # wrote, with the three URLs no Astro route produces.
    _write(
        dist / "sitemap.xml",
        "<urlset>"
        + "".join(f"<url><loc>{url}</loc></url>" for url in _SITEMAP_URLS)
        + "</urlset>\n",
    )

    for name in _SCHEMAS:
        _write(
            dist / "schemas" / f"{name}.schema.json",
            json.dumps({"$id": f"{_SITE}/schemas/{name}.schema.json"}) + "\n",
        )

    # A source fragment (C-010 item 3) and the id it must resolve to.
    _write(
        src / "configuration.md",
        "---\n"
        "title: configuration\n"
        "description: The configuration page.\n"
        "---\n"
        "<!-- doc_type: reference -->\n\n"
        "# configuration\n\n"
        "## Registry compatibility {#registry-compatibility}\n\n"
        "Prose.\n",
    )
    _write(
        dist / "configuration.html",
        _html(
            "configuration",
            '<h2 id="registry-compatibility">Registry compatibility</h2>',
        ),
    )

    # A catalog link back to a real page and fragment (C-010 item 4).
    _write(
        root / "catalog" / "skills" / "x" / "SKILL.md",
        "# x\n\nSee [batch publishing]"
        f"({_SITE}/publishing.html#batch-publish).\n",
    )
    _write(
        dist / "publishing.html",
        _html("publishing", '<h2 id="batch-publish">Batch publish</h2>'),
    )

    _write(
        root / "README.md",
        f"# grimoire\n\nStart with the [introduction]({_SITE}/introduction.html).\n",
    )
    return root


def _run(root: Path, *extra: str) -> subprocess.CompletedProcess[str]:
    """Drive check_urls.py as a subprocess — the exit code is the contract."""
    return subprocess.run(
        [sys.executable, str(_CHECK_URLS), "--root", str(root), *extra],
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )


@pytest.fixture
def clean_root(tmp_path: Path) -> Path:
    return _clean_tree(tmp_path / "repo")


def test_clean_tree_exits_zero(clean_root: Path) -> None:
    """C-010 acceptance, S-012: a clean build reports nothing."""
    result = _run(clean_root)
    assert result.returncode == 0, (
        f"clean tree should exit 0, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert result.stdout == "", f"clean tree printed findings: {result.stdout}"


def test_missing_chapter_page_exits_one(clean_root: Path) -> None:
    """C-010 item 1, S-014: a deleted chapter page is named and fails."""
    (clean_root / "docs" / "dist" / "quickstart.html").unlink()
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"missing page should exit 1, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert "quickstart.html" in result.stdout, (
        f"finding must name the missing page, got: {result.stdout}"
    )


def test_sitemap_without_static_page_exits_one(clean_root: Path) -> None:
    """C-009: dropping `customPages` drops start.html from the sitemap.

    `@astrojs/sitemap` sees Astro routes only, so the two pages served out of
    `docs/public/` reach it through that key alone — and leave silently if it
    goes, with every other check still green.
    """
    sitemap = clean_root / "docs" / "dist" / "sitemap.xml"
    _write(sitemap, sitemap.read_text().replace(f"<loc>{_SITE}/start.html</loc>", ""))
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"sitemap missing a static page should exit 1, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert "start.html missing from the sitemap" in result.stdout, (
        f"finding must name the dropped URL, got: {result.stdout}"
    )


def test_unresolved_source_fragment_exits_one(clean_root: Path) -> None:
    """C-010 item 3, S-012 and S-014: a `{#id}` with no id in dist fails."""
    _write(
        clean_root / "docs" / "dist" / "configuration.html",
        _html("configuration", "<h2>Registry compatibility</h2>"),
    )
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"unresolved fragment should exit 1, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert "registry-compatibility" in result.stdout, (
        f"finding must name the fragment, got: {result.stdout}"
    )


def test_stray_mdx_source_exits_one(clean_root: Path) -> None:
    """C-010 item 5: an `.mdx` file under the source tree fails."""
    _write(
        clean_root / "docs" / "src" / "content" / "docs" / "stray.mdx",
        "# stray\n",
    )
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"stray .mdx should exit 1, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert "stray.mdx" in result.stdout, (
        f"finding must name the stray file, got: {result.stdout}"
    )


def test_schema_id_mismatch_exits_one(clean_root: Path) -> None:
    """C-010 item 2, S-013: a schema whose `$id` is not its own URL fails."""
    _write(
        clean_root / "docs" / "dist" / "schemas" / "grim-mcp.schema.json",
        json.dumps({"$id": f"{_SITE}/schemas/wrong.schema.json"}) + "\n",
    )
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"schema $id mismatch should exit 1, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert "grim-mcp" in result.stdout, (
        f"finding must name the schema, got: {result.stdout}"
    )


def test_catalog_link_to_missing_page_exits_one(clean_root: Path) -> None:
    """C-010 item 4: a catalog link to a page that is not built fails."""
    _write(
        clean_root / "catalog" / "skills" / "x" / "SKILL.md",
        f"# x\n\nSee [nope]({_SITE}/nope.html).\n",
    )
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"catalog link rot should exit 1, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert "SKILL.md" in result.stdout, (
        f"finding must name the catalog file, got: {result.stdout}"
    )


def test_missing_dist_exits_two(clean_root: Path) -> None:
    """C-010 exit-code contract: partial coverage is never reported as 0."""
    dist = clean_root / "docs" / "dist"
    shutil.rmtree(dist)
    result = _run(clean_root)
    assert result.returncode == 2, (
        f"missing docs/dist should exit 2, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert str(dist) in result.stderr, (
        f"stderr must name the missing path, got: {result.stderr}"
    )


def test_catalog_toml_link_rot_exits_one(clean_root: Path) -> None:
    """C-010 item 4: a dead link in `catalog/publish.toml` is link rot too.

    `publish.toml` carries `documentation = "https://grimoire.rs/…"`, which
    ships to the OCI registry as published package metadata — the exact thing
    item 4 exists to catch, and a `*.md` glob never sees it.
    """
    _write(
        clean_root / "catalog" / "publish.toml",
        f'[metadata]\ndocumentation = "{_SITE}/nope.html"\n',
    )
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"catalog non-markdown link rot should exit 1, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert "publish.toml" in result.stdout, (
        f"finding must name the catalog manifest, got: {result.stdout}"
    )


def test_lfs_pointer_image_exits_one(clean_root: Path) -> None:
    """C-007 acceptance: an unfetched LFS pointer is not the image.

    `is_file()` passes on the ~130-byte ASCII pointer Git LFS leaves behind,
    so the site would ship a broken card with the gate green.
    """
    _write(clean_root / "docs" / "dist" / "og-card.png", _LFS_POINTER)
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"an LFS pointer should exit 1, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert "og-card.png" in result.stdout, (
        f"finding must name the asset, got: {result.stdout}"
    )
    assert "LFS" in result.stdout, (
        f"finding must say it is an LFS pointer, got: {result.stdout}"
    )


def test_fragment_finding_carries_line_and_rule(clean_root: Path) -> None:
    """C-010 item 3: the rendered line names the source line and the rule.

    The expected line is derived from the fixture on disk, so the assertion
    stays honest if the fixture's front matter ever grows a line.
    """
    source = clean_root / "docs" / "src" / "content" / "docs" / "configuration.md"
    heading_line = next(
        n
        for n, line in enumerate(source.read_text(encoding="utf-8").splitlines(), 1)
        if "{#registry-compatibility}" in line
    )
    _write(
        clean_root / "docs" / "dist" / "configuration.html",
        _html("configuration", "<h2>Registry compatibility</h2>"),
    )
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"unresolved fragment should exit 1, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert f"configuration.md:{heading_line}: [DOC-URL-03]" in result.stdout, (
        f"finding must render as page:line: [rule], got: {result.stdout}"
    )


def test_data_id_does_not_resolve_a_fragment(clean_root: Path) -> None:
    """C-010 item 3: `data-id=` is not an `id=`.

    A substring test accepts `data-id="x"` and a commented-out `id="x"`, so a
    page that never emits the anchor passes.
    """
    _write(
        clean_root / "docs" / "dist" / "configuration.html",
        _html(
            "configuration",
            '<h2 data-id="registry-compatibility">Registry compatibility</h2>',
        ),
    )
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"a data-id attribute should not resolve a fragment, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert "registry-compatibility" in result.stdout, (
        f"finding must name the fragment, got: {result.stdout}"
    )


def test_missing_readme_exits_two(clean_root: Path) -> None:
    """C-010 exit-code contract: partial coverage is never reported as 0.

    The README is a checked input like the three directories, so its absence
    is a missing input, not silently dropped coverage.
    """
    readme = clean_root / "README.md"
    readme.unlink()
    result = _run(clean_root)
    assert result.returncode == 2, (
        f"missing README.md should exit 2, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert str(readme) in result.stderr, (
        f"stderr must name the missing path, got: {result.stderr}"
    )


def test_orphan_page_not_in_sidebar_exits_one(clean_root: Path) -> None:
    """E-4, C-010 item 6: a page in no sidebar group is an orphan and fails.

    The page is written to both the source tree and ``docs/dist``, so items 1
    and 3 stay clean and only item 6 can produce the finding.  WP-D deleted
    ``SUMMARY.md``, which was the only disk-to-nav check the site had.
    """
    _write(
        clean_root / "docs" / "src" / "content" / "docs" / "orphan.md",
        "---\ntitle: orphan\ndescription: The orphan page.\n---\n"
        "<!-- doc_type: reference -->\n\n# orphan\n",
    )
    _write(clean_root / "docs" / "dist" / "orphan.html", _html("orphan"))
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"a page in no sidebar group should exit 1, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert "orphan" in result.stdout, (
        f"finding must name the orphaned page, got: {result.stdout}"
    )


def test_sidebar_slug_without_page_exits_one(clean_root: Path) -> None:
    """E-4, C-010 item 6: a sidebar slug with no page on disk fails.

    This is the nav-to-disk half C-003 already asserted; item 6 keeps it once
    the sidebar becomes the only nav source.
    """
    _write(
        clean_root / "docs" / "astro.config.mjs",
        _astro_config((*_SIDEBAR[:1], ("Guides", ("clients", "ghost")), *_SIDEBAR[2:])),
    )
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"a sidebar slug with no page should exit 1, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert "ghost" in result.stdout, (
        f"finding must name the dangling slug, got: {result.stdout}"
    )


def test_page_in_two_groups_exits_one(clean_root: Path) -> None:
    """E-4, C-010 item 6: a page in two sidebar groups fails.

    "Exactly one" is the half a set-membership assertion would miss — a check
    that only asked whether each page appears somewhere passes this tree, so
    this case is what keeps item 6 from being vacuous.
    """
    _write(
        clean_root / "docs" / "astro.config.mjs",
        _astro_config(
            (*_SIDEBAR[:1], ("Guides", ("clients", "commands")), *_SIDEBAR[2:])
        ),
    )
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"a page in two groups should exit 1, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert "commands" in result.stdout, (
        f"finding must name the twice-listed page, got: {result.stdout}"
    )


def test_missing_astro_config_exits_two(clean_root: Path) -> None:
    """E-4, C-010 exit-code contract: no sidebar is a missing input, not a pass.

    Item 6 has nothing to read without the config, and silently dropping its
    coverage would report a green gate over an unchecked nav.
    """
    config = clean_root / "docs" / "astro.config.mjs"
    config.unlink()
    result = _run(clean_root)
    assert result.returncode == 2, (
        f"missing astro.config.mjs should exit 2, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert str(config) in result.stderr, (
        f"stderr must name the missing path, got: {result.stderr}"
    )


def test_extensionless_internal_href_exits_one(clean_root: Path) -> None:
    """E-7, C-010 item 7: an internal href with no `.html` suffix fails.

    ``astro-rehype-relative-markdown-links`` resolves against Astro's *route*
    manifest, and a route has no extension, so under ``build.format: 'file'``
    it emits ``/commands`` where the built file is ``/commands.html``.  The
    config stage that appends the suffix is one plugin bump away from being
    bypassed, and nothing else in the gate reads a built href.
    """
    page = clean_root / "docs" / "dist" / "concepts.html"
    _write(page, _html("concepts", '<a href="/commands">extensionless</a>'))
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"an extensionless internal href should exit 1, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert "/commands" in result.stdout, (
        f"finding must name the offending href, got: {result.stdout}"
    )


def test_href_fragment_is_not_read_as_an_extension(clean_root: Path) -> None:
    """E-7, C-010 item 7: the suffix is judged on the path, never the fragment.

    ``/commands#v1.2`` has a dot, but not in its path.  A classifier that
    tested the whole href would read the fragment as an extension and pass a
    link that resolves nowhere — the same mistake, one character over, as
    appending `.html` after the fragment.
    """
    page = clean_root / "docs" / "dist" / "concepts.html"
    _write(page, _html("concepts", '<a href="/commands#v1.2">fragment with a dot</a>'))
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"a dot in the fragment must not excuse the path, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )


def test_href_with_unknown_extension_exits_one(clean_root: Path) -> None:
    """E-7, C-010 item 7: an extension outside the asset set is a finding.

    ``/commands.htm`` is the typo this catches.  A check that accepted any
    trailing ``.ext`` would pass it, which is why the allowlist is explicit
    rather than "has a dot".
    """
    page = clean_root / "docs" / "dist" / "concepts.html"
    _write(page, _html("concepts", '<a href="/commands.htm">typo</a>'))
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"an unknown extension should exit 1, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert "/commands.htm" in result.stdout, (
        f"finding must name the offending href, got: {result.stdout}"
    )


def test_dangling_cross_page_fragment_exits_one(clean_root: Path) -> None:
    """E-7, C-010 item 7: a cross-page fragment must resolve in its target.

    The suffix stage blinds ``starlight-links-validator``: it keys its heading
    map on the route, so an href of ``commands.html`` matches nothing and the
    hash branch never runs.  C-011's build-time hash half is therefore gone
    for all 290 source ``./page.md#frag`` links, and no other item covers
    them — item 3 asserts a ``{#custom-id}`` *definition* renders, item 4 reads
    only ``https://grimoire.rs/`` URLs under ``catalog/``, and the shape check
    above never opens the target.
    """
    page = clean_root / "docs" / "dist" / "concepts.html"
    _write(page, _html("concepts", '<a href="/commands.html#no-such-anchor">dead</a>'))
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"a dangling cross-page fragment should exit 1, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert "no-such-anchor" in result.stdout, (
        f"finding must name the dangling fragment, got: {result.stdout}"
    )


def test_link_to_absent_page_exits_one(clean_root: Path) -> None:
    """C-010 item 7: a linked ``.html`` page must exist, fragment or not.

    Item 1 covers ``STATIC_PATHS`` plus the 21 chapters only — never
    ``index.html``, the nine ``guides/`` pages or ``tutorials/own-index.html``
    — so nothing else asserted the target of a link is on disk.  With a
    fragment the old shape was worse than silent: the missing page read as
    "nothing to compare the fragment against" and the link passed.
    """
    page = clean_root / "docs" / "dist" / "concepts.html"
    _write(
        page,
        _html(
            "concepts",
            '<a href="/guides/renamed-away.html#run">deep link, page gone</a>\n'
            '<a href="/guides/gone.html">plain link, page gone</a>',
        ),
    )
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"a link to an absent page should exit 1, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert "/guides/renamed-away.html#run" in result.stdout, (
        f"finding must name the dead deep link, got: {result.stdout}"
    )
    assert "/guides/gone.html" in result.stdout, (
        f"finding must name the dead plain link, got: {result.stdout}"
    )


def test_relative_href_to_absent_page_exits_one(clean_root: Path) -> None:
    """C-010 item 7: a relative href is resolved, not skipped.

    ``start.html`` and ``privacy.html`` are copied verbatim out of
    ``docs/public/`` and use relative hrefs throughout.  Being no Astro route
    they are invisible to ``starlight-links-validator`` too, so a gate that
    skipped every non-root-relative href left the lead adoption story's own
    links checked by nothing at all.
    """
    # A page that never existed, not a deleted chapter: deleting one would let
    # item 1 raise the finding and leave item 7 untested.
    _write(
        clean_root / "docs" / "dist" / "start.html",
        _html("start", '<a href="hosting-an-index-old.html">the index guide</a>'),
    )
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"a relative href to an absent page should exit 1, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert "hosting-an-index-old.html" in result.stdout, (
        f"finding must name the relative target, got: {result.stdout}"
    )
    assert "DOC-URL-07" in result.stdout, (
        f"the finding must come from item 7, got: {result.stdout}"
    )


def test_relative_href_dangling_fragment_exits_one(clean_root: Path) -> None:
    """C-010 item 7: a relative deep link resolves its fragment too.

    ``start.html`` carries six fragment links into ``hosting-an-index.html``.
    Renaming one of those anchors is the break this catches: the page still
    exists, so an existence-only check would pass it.
    """
    _write(
        clean_root / "docs" / "dist" / "start.html",
        _html(
            "start",
            '<a href="hosting-an-index.html#gate">the contribution gate</a>',
        ),
    )
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"a dangling relative fragment should exit 1, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert "#gate" in result.stdout, (
        f"finding must name the dangling fragment, got: {result.stdout}"
    )


def test_missing_cast_recording_exits_one(clean_root: Path) -> None:
    """C-010 item 7: a recording mounts through ``data-cast``, not ``href``.

    The eleven recordings are embedded by attribute, so an ``href``-only scan
    never saw them and no path list named them either.  A rename ships a dead
    player on eleven pages with the gate green.
    """
    (clean_root / "docs" / "dist" / "casts" / "quickstart.cast").unlink()
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"a missing recording should exit 1, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert "/casts/quickstart.cast" in result.stdout, (
        f"finding must name the missing recording, got: {result.stdout}"
    )


def test_src_link_rot_exits_one(clean_root: Path) -> None:
    """C-010 item 4: the binary's own printed docs links are link rot too.

    ``registry_catalog.rs`` and ``catalog_service.rs`` each print a
    ``https://grimoire.rs/configuration.html#…`` deep link in a user-facing
    error.  A ``catalog/**``-only scan passed them by coincidence, because a
    catalog reference file happened to mirror the same two anchors.
    """
    _write(
        clean_root / "src" / "catalog" / "registry_catalog.rs",
        "pub const REGISTRY_COMPAT_DOCS_URL: &str =\n"
        f'    "{_SITE}/configuration.html#renamed-away";\n',
    )
    result = _run(clean_root)
    assert result.returncode == 1, (
        f"a dead docs link in src/ should exit 1, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert "registry_catalog.rs" in result.stdout, (
        f"finding must name the source file, got: {result.stdout}"
    )
    assert "renamed-away" in result.stdout, (
        f"finding must name the dead anchor, got: {result.stdout}"
    )


def test_missing_src_exits_two(clean_root: Path) -> None:
    """C-010 exit-code contract: partial coverage is never reported as 0.

    ``src/`` is a checked input like ``catalog/`` now that item 4 reads it, so
    a run pointed at a tree without one must say so rather than quietly drop
    the coverage.
    """
    crate_src = clean_root / "src"
    shutil.rmtree(crate_src)
    result = _run(clean_root)
    assert result.returncode == 2, (
        f"missing src/ should exit 2, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}"
    )
    assert str(crate_src) in result.stderr, (
        f"stderr must name the missing path, got: {result.stderr}"
    )


@pytest.fixture(scope="module")
def nav_depth() -> Iterator[ModuleType]:
    """Import ``nav_depth`` with its checks directory on ``sys.path``.

    The module imports ``strip_prose`` as a sibling, so the path has to be
    present during the import itself, not only at call time.
    """
    sys.path.insert(0, str(_CHECKS))
    try:
        import nav_depth as module

        yield module
    finally:
        sys.path.remove(str(_CHECKS))


def test_starlight_nav_fixture_is_two_levels(nav_depth: ModuleType) -> None:
    """D-9: the Starlight branch reads a four-group sidebar as depth 2.

    ``check_nav`` returning no findings also proves ``find_generator`` routes
    an ``astro.config.mjs`` directory to the starlight branch.
    """
    text = (_NAV_FIXTURE / "astro.config.mjs").read_text(encoding="utf-8")
    assert nav_depth.starlight_nav(text) == (2, 0, False)
    assert nav_depth.check_nav(_NAV_FIXTURE) == []


def test_find_generator_accepts_any_astro_config(
    nav_depth: ModuleType, tmp_path: Path
) -> None:
    """D-9: an Astro config is `astro.config.*`, not only `.mjs`.

    A `.ts` config resolved to "not applicable" and the whole nav family
    exited 0 — the failure mode the rule exists to prevent, one extension over.
    """
    (tmp_path / "astro.config.ts").write_text(
        "export default { integrations: [] };\n", encoding="utf-8"
    )
    assert nav_depth.find_generator(tmp_path) == (
        "starlight",
        tmp_path / "astro.config.ts",
    )
