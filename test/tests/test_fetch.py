# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim fetch` acceptance tests — use != install, payload-plain stdout."""
from __future__ import annotations

import base64
import json
import subprocess
import uuid
from pathlib import Path

from src.helpers import make_artifact, make_bundle, make_description


SKILL_DOC = (
    "---\n"
    "name: fetch-demo\n"
    "description: Demo skill for fetch tests.\n"
    "---\n"
    "# Fetch Demo\n\nBody text.\n"
)
SUPPORT_DOC = "support file body\n"


def _publish_skill(registry: str) -> str:
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    repo = f"{ns}/skills/fetch-demo"
    make_artifact(
        repo,
        "skill",
        {
            "fetch-demo/SKILL.md": SKILL_DOC,
            "fetch-demo/references/notes.md": SUPPORT_DOC,
        },
        tag="latest",
    )
    return f"{registry}/{repo}:latest"


def test_fetch_plain_stdout_byte_equals_published_index(
    grim_at, project_dir: Path, registry: str
) -> None:
    ref = _publish_skill(registry)
    runner = grim_at(project_dir)
    result = runner.plain("fetch", ref)
    assert result.returncode == 0, result.stderr
    # Exact bytes: no table, no added trailing newline.
    assert result.stdout == SKILL_DOC


def test_fetch_path_returns_exact_support_file(
    grim_at, project_dir: Path, registry: str
) -> None:
    ref = _publish_skill(registry)
    runner = grim_at(project_dir)
    result = runner.plain("fetch", ref, "--path", "fetch-demo/references/notes.md")
    assert result.returncode == 0, result.stderr
    assert result.stdout == SUPPORT_DOC


def test_fetch_json_is_full_report(grim_at, project_dir: Path, registry: str) -> None:
    ref = _publish_skill(registry)
    runner = grim_at(project_dir)
    doc = runner.json("fetch", ref)
    assert doc["kind"] == "skill"
    assert doc["name"] == "fetch-demo"
    assert doc["vendor"] == "canonical"
    assert doc["content"] == SKILL_DOC
    assert doc["digest"].startswith("sha256:")
    paths = [f["path"] for f in doc["files"]]
    assert "fetch-demo/SKILL.md" in paths
    assert "fetch-demo/references/notes.md" in paths


def test_fetch_vendor_claude_projection(grim_at, project_dir: Path, registry: str) -> None:
    ref = _publish_skill(registry)
    runner = grim_at(project_dir)
    doc = runner.json("fetch", ref, "--vendor", "claude")
    assert doc["vendor"] == "claude"
    # A plain skill without tool-namespaced metadata projects verbatim.
    assert doc["content"] == SKILL_DOC


def test_fetch_bundle_vendor_and_unknown_vendor_error(
    grim_at, project_dir: Path, registry: str
) -> None:
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    repo = f"{ns}/bundles/stack"
    make_bundle(repo, [], tag="latest")
    runner = grim_at(project_dir)

    result = runner.plain("fetch", f"{registry}/{repo}:latest", "--vendor", "claude", check=False)
    assert result.returncode != 0
    assert "vendor projection" in result.stderr

    ref = _publish_skill(registry)
    result = runner.plain("fetch", ref, "--vendor", "nonesuch", check=False)
    assert result.returncode != 0


# A minimal but non-UTF-8 payload (PNG signature + a 0xFF byte).
LOGO_BYTES = bytes([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0xFF, 0xFE])


def _publish_skill_with_logo(registry: str) -> str:
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    repo = f"{ns}/skills/logo-demo"
    make_artifact(
        repo,
        "skill",
        {
            "logo-demo/SKILL.md": "---\nname: logo-demo\ndescription: d.\n---\n# Logo\n",
            "logo-demo/assets/logo.png": LOGO_BYTES,
        },
        tag="latest",
    )
    return f"{registry}/{repo}:latest"


def test_fetch_binary_path_is_base64_and_round_trips(
    grim_at, project_dir: Path, registry: str
) -> None:
    """A binary --path support file comes back base64 (`encoding: "base64"`)
    in JSON, and plain output decodes back to the exact bytes so a stdout
    redirect round-trips byte-identical."""
    ref = _publish_skill_with_logo(registry)
    runner = grim_at(project_dir)

    doc = runner.json("fetch", ref, "--path", "logo-demo/assets/logo.png")
    assert doc["encoding"] == "base64"
    assert base64.b64decode(doc["content"]) == LOGO_BYTES
    assert doc.get("truncated") is not True

    # Plain redirect (binary): capture stdout to a file and compare bytes.
    out_file = project_dir / "logo.png"
    with out_file.open("wb") as fh:
        subprocess.run(
            [str(runner.binary), "fetch", ref, "--path", "logo-demo/assets/logo.png"],
            stdout=fh,
            env=runner.env,
            cwd=str(project_dir),
            check=True,
        )
    assert out_file.read_bytes() == LOGO_BYTES, "redirect round-trips byte-identical"

    # A UTF-8 support file is unchanged — no encoding field.
    text_doc = runner.json("fetch", ref, "--path", "logo-demo/SKILL.md")
    assert "encoding" not in text_doc


AGENT_DOC = (
    "---\n"
    "name: fetch-agent\n"
    "description: Demo agent for fetch tests.\n"
    "---\n"
    "You review code.\n"
)
AGENT_README = "# Fetch Agent\n\nWhat this agent does.\n"


def _publish_agent_with_readme(registry: str) -> str:
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    repo = f"{ns}/agents/fetch-agent"
    # An agent's well-known README rides the layer tree under `<name>/`,
    # exactly like a skill's — a non-skill kind shipping a README.
    make_artifact(
        repo,
        "agent",
        {
            "fetch-agent.md": AGENT_DOC,
            "fetch-agent/README.md": AGENT_README,
        },
        tag="latest",
    )
    return f"{registry}/{repo}:latest"


def test_fetch_agent_readme_listed_and_pullable(
    grim_at, project_dir: Path, registry: str
) -> None:
    """A non-skill (agent) kind can ship a README that rides the layer tree:
    it shows in `files[]` and pulls with `--path <name>/README.md`, the same
    path shape every tree-backed kind uses. This is the contract the VS Code
    extension's details tab relies on."""
    ref = _publish_agent_with_readme(registry)
    runner = grim_at(project_dir)

    doc = runner.json("fetch", ref)
    assert doc["kind"] == "agent"
    paths = [f["path"] for f in doc["files"]]
    assert "fetch-agent.md" in paths
    assert "fetch-agent/README.md" in paths

    result = runner.plain("fetch", ref, "--path", "fetch-agent/README.md")
    assert result.returncode == 0, result.stderr
    assert result.stdout == AGENT_README


def test_fetch_large_content_is_not_truncated(
    grim_at, project_dir: Path, registry: str
) -> None:
    """Pins the cap decision: the CLI never truncates below the 8 MiB
    layer gate — content past the MCP 256 KiB doc cap prints complete."""
    big_body = "x" * (300 * 1024)  # > 256 KiB MCP cap, < 8 MiB gate
    doc = f"---\nname: big\ndescription: d.\n---\n{big_body}\n"
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    repo = f"{ns}/skills/big"
    make_artifact(repo, "skill", {"big/SKILL.md": doc}, tag="latest")
    runner = grim_at(project_dir)

    result = runner.plain("fetch", f"{registry}/{repo}:latest")
    assert result.returncode == 0, result.stderr
    assert result.stdout == doc, "no truncation, no marker"
    assert "truncated" not in result.stdout

    parsed = json.loads(runner.run("fetch", f"{registry}/{repo}:latest", format="json").stdout)
    assert parsed.get("truncated") is not True


# --------------------------------------------------------------------------
# Description companion + digest probe (the VS Code details-tab surface)
# --------------------------------------------------------------------------

README_BYTES = b"# Repo\n\nWhat this repository ships.\n"


def _publish_with_companion(registry: str) -> str:
    """Publish an artifact and its description companion; return the artifact
    ref (grim retargets the reserved ``__grimoire`` tag itself)."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    repo = f"{ns}/skills/desc-demo"
    make_artifact(
        repo,
        "skill",
        {"desc-demo/SKILL.md": "---\nname: desc-demo\ndescription: d.\n---\n# d\n"},
        tag="latest",
    )
    make_description(repo, {"README.md": README_BYTES, "assets/logo.png": LOGO_BYTES})
    return f"{registry}/{repo}:latest"


def test_fetch_description_json_inlines_all_members(
    grim_at, project_dir: Path, registry: str
) -> None:
    """`fetch --description` returns `{ref, digest, kind: "desc", files: [...]}`
    with every member inline: text verbatim, binary as base64."""
    ref = _publish_with_companion(registry)
    runner = grim_at(project_dir)

    doc = runner.json("fetch", ref, "--description")
    assert doc["kind"] == "desc"
    assert doc["ref"].endswith(":__grimoire"), doc["ref"]
    assert doc["digest"].startswith("sha256:")
    members = {f["path"]: f for f in doc["files"]}

    readme = members["README.md"]
    assert readme["content"].encode() == README_BYTES
    assert "encoding" not in readme, "a text member carries no encoding field"

    logo = members["assets/logo.png"]
    assert logo["encoding"] == "base64"
    assert base64.b64decode(logo["content"]) == LOGO_BYTES


def test_fetch_description_out_round_trips_the_tree(
    grim_at, project_dir: Path, registry: str
) -> None:
    """`fetch --description --out <dir>` unpacks the companion tree to disk
    byte-identical; plain stdout stays empty (no single payload)."""
    ref = _publish_with_companion(registry)
    runner = grim_at(project_dir)

    out_dir = project_dir / "desc-out"
    result = runner.plain("fetch", ref, "--description", "--out", str(out_dir))
    assert result.returncode == 0, result.stderr
    assert result.stdout == "", "a multi-file bundle has no single plain payload"

    assert (out_dir / "README.md").read_bytes() == README_BYTES
    assert (out_dir / "assets" / "logo.png").read_bytes() == LOGO_BYTES


def test_fetch_digest_only_matches_full_fetch_for_artifact_and_companion(
    grim_at, project_dir: Path, registry: str
) -> None:
    """`--digest-only` resolves without downloading and returns `{ref, digest}`;
    the digest equals the full fetch's digest, for the artifact and (with
    `--description`) the companion."""
    ref = _publish_with_companion(registry)
    # A resolved project scope keeps the probe to its minimal shape (a degraded
    # scope would add a `warnings` note, like every fetch report).
    (project_dir / "grimoire.toml").write_text("[skills]\n")
    runner = grim_at(project_dir)

    # Artifact.
    full = runner.json("fetch", ref)
    probe = runner.json("fetch", ref, "--digest-only")
    assert set(probe) == {"ref", "digest"}, f"digest probe is just {{ref, digest}}: {probe}"
    assert probe["digest"] == full["digest"]

    # Companion.
    full_desc = runner.json("fetch", ref, "--description")
    probe_desc = runner.json("fetch", ref, "--description", "--digest-only")
    assert probe_desc["digest"] == full_desc["digest"]
    assert probe_desc["ref"].endswith(":__grimoire")
    assert probe["digest"] != probe_desc["digest"], "artifact and companion are distinct manifests"


def test_fetch_missing_companion_is_not_found_79(
    grim_at, project_dir: Path, registry: str
) -> None:
    """`--description` on a repo with no companion is a not-found (79) — the
    standard error envelope, parity with a missing `grim fetch`."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    repo = f"{ns}/skills/lonely"
    make_artifact(repo, "skill", {"lonely/SKILL.md": "---\nname: lonely\ndescription: d.\n---\n# x\n"}, tag="latest")
    runner = grim_at(project_dir)

    result = runner.run("--format", "json", "fetch", f"{registry}/{repo}:latest", "--description", check=False)
    assert result.returncode == 79, result.stderr
    doc = json.loads(result.stdout)
    assert doc["error"]["code"] == "not-found"
    assert doc["error"]["exit"] == 79


def test_fetch_flag_combination_usage_errors_exit_64(
    grim_at, project_dir: Path, registry: str
) -> None:
    """The contradictory flag combinations are usage errors (64) before any
    resolution: `--out` without `--description`, download flags with
    `--digest-only`, and plain `--description` without `--out`."""
    ref = _publish_with_companion(registry)
    runner = grim_at(project_dir)

    # --out requires --description.
    r1 = runner.plain("fetch", ref, "--out", str(project_dir / "o"), check=False)
    assert r1.returncode == 64, r1.stderr

    # --digest-only downloads nothing, so it takes no content flag.
    for extra in (("--path", "README.md"), ("--vendor", "claude"), ("--out", str(project_dir / "o2"))):
        r = runner.plain("fetch", ref, "--digest-only", *extra, check=False)
        assert r.returncode == 64, f"--digest-only {extra} must be a usage error; stderr: {r.stderr}"

    # Plain --description without --out has no single payload to print.
    r3 = runner.plain("fetch", ref, "--description", check=False)
    assert r3.returncode == 64, r3.stderr
    assert "--out" in r3.stderr or "json" in r3.stderr, r3.stderr

    # ...but --format json is fine without --out.
    ok = runner.run("--format", "json", "fetch", ref, "--description", check=False)
    assert ok.returncode == 0, ok.stderr
