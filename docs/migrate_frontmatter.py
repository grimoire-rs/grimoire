#!/usr/bin/env python3
"""One-shot migration: convert each docs page's line-1 doc_type/doc_tier
HTML-comment declaration into Starlight-style YAML frontmatter, keeping the
declaration comment(s) intact immediately below the frontmatter block.

Before:
    <!-- doc_type: reference -->
    # Title
    ...

After:
    ---
    title: "Title"
    description: "..."
    ---
    <!-- doc_type: reference -->
    # Title
    ...

Idempotent: a page whose first line is already "---" is left untouched.
Titles and descriptions come from PAGES, drafted in
.agents/research/research_docs_migration_content_survey.md (section 2).

Usage:
  migrate_frontmatter.py [--root DIR]

Exit codes: 0 all pages migrated (or already migrated), 2 PAGES/disk mismatch.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

# file name -> (title, description). Verbatim from
# .agents/research/research_docs_migration_content_survey.md lines 33-102.
PAGES: dict[str, tuple[str, str]] = {
    "agents.md": (
        "Agent Artifacts",
        "Skills teach an agent a capability and rules constrain it — an agent bundles both into a reusable persona Grimoire can install.",
    ),
    "artifacts.md": (
        "Artifact Reference",
        "Grimoire ships five artifact kinds — skills, rules, agents, MCP servers, and bundles — each with its own file layout and install target.",
    ),
    "authentication.md": (
        "Authentication",
        "Most public skills and rules pull anonymously, but a private registry needs credentials — how grim resolves and stores them.",
    ),
    "ci.md": (
        "Publishing from CI",
        "Publishing by hand works until a second contributor bumps a version and conflicts arise — automate it from CI instead.",
    ),
    "clients.md": (
        "Client Compatibility",
        "grim installs one canonical artifact into many AI clients, and not every client supports every artifact kind equally.",
    ),
    "commands.md": (
        "Command Reference",
        "Every grim command follows the same shape: parse references into typed values, run the operation, then report the result.",
    ),
    "concepts.md": (
        "Concepts",
        "Grimoire borrows its mental model from package managers you already use, then adapts it for AI-agent configuration.",
    ),
    "configuration.md": (
        "Configuration",
        "Grimoire keeps configuration in two small files and a handful of environment variables, layered by scope.",
    ),
    "hosting-an-index.md": (
        "Host Your Own Index",
        "An index is the phone book grim browses — it answers what packages exist. Here is how to host your own.",
    ),
    "installation.md": (
        "Installation",
        "grim is a single self-contained binary. Once it is on your PATH there is nothing else to install.",
    ),
    "introduction.md": (
        "Introduction",
        "Grimoire is a package manager for AI-agent configuration, distributed through standard OCI registries.",
    ),
    "json-interface.md": (
        "The JSON Interface",
        "Every grim command that reports something offers --format json, forming a stable machine-readable interface.",
    ),
    "mcp-servers.md": (
        "MCP Server Artifacts",
        "Skills teach a capability, rules constrain behavior, and agents define a persona — MCP servers extend an agent with tools.",
    ),
    "package-index.md": (
        "The Package Index",
        "Most OCI registries cannot answer what packages exist — the package index is Grimoire's answer to that question.",
    ),
    "publishing.md": (
        "Publishing Skills and Rules",
        "Consuming artifacts is only half of Grimoire. The other half is producing and publishing them to a registry.",
    ),
    "quickstart.md": (
        "Quick Start",
        "This walkthrough declares a skill, installs it into a project, and shows the result end to end.",
    ),
    "ratings.md": (
        "Artifact Ratings",
        "An index lists what exists but says nothing about what is any good — ratings close that gap.",
    ),
    "self-hosted-gitlab.md": (
        "Self-Hosted GitLab Setup",
        "Everything grim does on github.com also works on a corporate GitLab instance, with a few setup differences.",
    ),
    "stability.md": (
        "Stability and Versioning",
        "Grimoire is pre-1.0 — this page documents which CLI, format, and pipeline contracts are frozen versus still evolving.",
    ),
    "upgrading.md": (
        "Upgrading",
        "CHANGELOG.md lists every change, one line per commit — this page covers upgrading grim itself version to version.",
    ),
    "vendor-metadata.md": (
        "Vendor-Specific Metadata",
        "Each AI client tool adds its own capability fields on top of the shared skill spec, namespaced under a metadata map.",
    ),
}

assert len(PAGES) == 21, f"expected 21 pages in PAGES, got {len(PAGES)}"

# A declaration line, e.g. "<!-- doc_type: reference -->". Bytes, because the
# whole migration is byte-level: the original file is re-emitted verbatim.
DECLARATION = re.compile(rb"^<!--\s*doc_(?:type|tier)\s*:")


def migrate(path: Path) -> bool:
    """Rewrite one page's leading doc_type/doc_tier comment(s) into frontmatter.

    - Read the file. If line 1 is exactly "---", the page is already
      migrated: return False and write nothing (this is what makes the
      script idempotent).
    - Otherwise take the leading contiguous run of lines matching
      ``^<!--\\s*doc_(type|tier)\\s*:`` (every page has 1 or 2 such lines,
      starting at line 1) as the declaration block.
    - Emit, in this order: "---", 'title: "<title>"',
      'description: "<description>"', "---", then the declaration lines
      verbatim, then every remaining line of the original file
      byte-for-byte unchanged.
    - Return True.
    """
    original = path.read_bytes()
    lines = original.splitlines(keepends=True)

    if lines and lines[0].rstrip(b"\r\n") == b"---":
        return False

    # The declaration block is the leading contiguous run of doc_type/doc_tier
    # comments, and it already sits at the head of the file — prefixing the
    # frontmatter keeps it exactly where it is, so only its presence needs
    # checking. A page without one is an error, never a silent pass.
    if not lines or not DECLARATION.match(lines[0]):
        raise ValueError(f"{path}: no doc_type/doc_tier declaration on line 1")

    title, description = PAGES[path.name]
    frontmatter = f'---\ntitle: "{title}"\ndescription: "{description}"\n---\n'
    path.write_bytes(frontmatter.encode("utf-8") + original)
    return True


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--root",
        type=Path,
        default=Path("docs/src/content/docs"),
        help="directory containing the page .md files (default: %(default)s)",
    )
    args = parser.parse_args(argv)

    pages_on_disk = sorted(args.root.glob("*.md"))
    on_disk_names = {p.name for p in pages_on_disk}
    known_names = set(PAGES)

    unknown = sorted(on_disk_names - known_names)
    missing = sorted(known_names - on_disk_names)
    if unknown or missing:
        for name in unknown:
            print(f"error: {name}: on disk but not in PAGES", file=sys.stderr)
        for name in missing:
            print(f"error: {name}: in PAGES but missing on disk", file=sys.stderr)
        return 2

    for page in pages_on_disk:
        if migrate(page):
            print(f"migrated {page}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
