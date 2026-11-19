"""Regression guards for the OCX signing identity.

The signing certificate names the workflow file that defines the signing job
(`release-ocx.yml`), and that holds only while `release.yml` stays pure
cargo-dist output and `id-token: write` stays where a certificate is minted.
Plain text checks: no YAML parser is a dependency of this suite.

Run:
    cd .claude/tests && uv run pytest test_workflows.py -v
"""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github" / "workflows"

# Every place `id-token: write` may appear. Each mints an OIDC token: a new one
# is a new signing identity and must be a decision, not a drive-by.
ALLOWED_ID_TOKEN = {
    ("backfill.yml", "backfill"),  # second signing identity
    ("docs.yml", "<workflow>"),  # GitHub Pages deploy
    ("release.yml", "build-local-artifacts"),  # cargo-dist build provenance
    ("release.yml", "custom-release-ocx"),  # generated grant for the signing workflow
    ("release-ocx.yml", "<workflow>"),  # the signing workflow itself
}


def test_dist_still_checks_release_yml() -> None:
    # Without `allow-dirty`, the release's own `dist` plan step fails on a hand-edited release.yml.
    assert "allow-dirty" not in (ROOT / "dist-workspace.toml").read_text()


def _id_token_sites() -> set[tuple[str, str]]:
    sites: set[tuple[str, str]] = set()
    files = [*WORKFLOWS.glob("*.y*ml"), *(ROOT / ".github" / "actions").glob("**/*.y*ml")]
    for path in files:
        job = "<workflow>"
        for line in path.read_text().splitlines():
            if line.lstrip().startswith("#"):
                continue
            if line.startswith("jobs:"):
                job = ""
            elif (m := re.match(r"^  ([\w-]+):\s*$", line)) and job != "<workflow>":
                job = m.group(1)
            if re.search(r"""["']?id-token["']?\s*:\s*["']?write""", line):
                sites.add((path.name, job or "<workflow>"))
    return sites


def test_id_token_write_only_where_a_certificate_is_minted() -> None:
    assert _id_token_sites() == ALLOWED_ID_TOKEN


def test_release_ocx_starts_with_the_main_only_guard_and_uses_the_action() -> None:
    text = (WORKFLOWS / "release-ocx.yml").read_text()
    steps = text.split("\n    steps:\n", 1)[1]
    # The first list item of the job; comments before it are not items.
    first = re.split(r"(?m)^      - ", steps, maxsplit=2)[1]
    assert "Require the main branch" in first
    assert "refs/heads/main" in first and "::error::" in first
    assert "uses: ./.github/actions/release-ocx" in text
