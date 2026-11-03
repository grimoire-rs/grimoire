# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim build` validates a declared `requires` offline, for every kind.

Skill, rule and agent sources carry it as ``grim.requires.<table>.<name>``
frontmatter ``metadata`` keys, mcp and hook sources as ``[requires.<table>]``
tables, a bundle source as a ``[bundles]`` table. Needs no registry.
"""
from __future__ import annotations

from pathlib import Path

import pytest

SKILL_REQ = '  grim.requires.skills.gws-shared: "./gws-shared:1"\n'
MCP_REQ = "[requires.skills]\nhelper = \"ghcr.io/acme/helper:1\"\n"
HOOK_REQ = '\n[requires.skills]\nhelper = "ghcr.io/acme/helper:1"\n'


def _write(p: Path, body: str) -> Path:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_bytes(body.encode())  # no CRLF translation on Windows: digests pin exact bytes
    return p


def _skill(root: Path, extra_meta: str = "", name: str = "gws-drive") -> Path:
    meta = f"metadata:\n{extra_meta}" if extra_meta else ""
    _write(
        root / name / "SKILL.md",
        f"---\nname: {name}\ndescription: Drive helpers.\n{meta}---\n# Body\n",
    )
    return root / name


def _rule(root: Path, extra_meta: str = "") -> Path:
    meta = f"metadata:\n{extra_meta}" if extra_meta else ""
    return _write(root / "style.md", f"---\npaths: ['**/*.rs']\n{meta}---\n# Style\n")


def _agent(root: Path, extra_meta: str = "") -> Path:
    meta = f"metadata:\n{extra_meta}" if extra_meta else ""
    return _write(
        root / "helper.md",
        f"---\nname: helper\ndescription: Helps.\n{meta}---\nBody\n",
    )


MCP_BASE = 'description = "d"\n[server]\ntransport = "stdio"\ncommand = "grim"\n'


def _mcp(root: Path, extra: str = "") -> Path:
    return _write(root / "gdrive.toml", MCP_BASE + extra)


def _hook(root: Path, extra: str = "", name: str = "guard") -> Path:
    _write(
        root / name / "hook.toml",
        f'schema = 1\nname = "{name}"\ndescription = "d"\n{extra}\n'
        '[[hooks]]\nid = "g"\nevent = "PreToolUse"\ntier = "observer"\n'
        'argv = ["sh", "g.sh"]\ntimeout = 5\npayload = "stdin"\n',
    )
    _write(root / name / "g.sh", "exit 0\n")
    return root / name


def _bundle(root: Path, body: str, name: str = "stack") -> Path:
    return _write(root / f"{name}.toml", body)


# ---------------------------------------------------------------------------
# Valid declarations build, for every kind
# ---------------------------------------------------------------------------


def test_valid_declarations_build_for_every_kind(grim_at, project_dir: Path) -> None:
    runner = grim_at(project_dir)
    cases = {
        "skill": ["build", str(_skill(project_dir, SKILL_REQ))],
        "rule": ["build", str(_rule(project_dir, '  grim.requires.agents.helper: "./helper:1"\n'))],
        "agent": [
            "build",
            "--kind",
            "agent",
            str(_agent(project_dir, '  grim.requires.mcp.gdrive: "ghcr.io/acme/mcp/gdrive:2"\n')),
        ],
        "mcp": ["build", "--kind", "mcp", str(_mcp(project_dir, MCP_REQ))],
        "hook": ["build", str(_hook(project_dir, HOOK_REQ))],
        "bundle": [
            "build",
            str(_bundle(project_dir, '[bundles]\ninner = "ghcr.io/acme/bundles/inner:1"\n')),
        ],
    }
    for kind, argv in cases.items():
        out = runner.json(*argv)
        assert out["kind"] == kind, out
        assert out["status"] == "built", out


def test_declared_keys_stay_in_the_packed_skill(grim_at, project_dir: Path) -> None:
    """The keys ride in the packed content verbatim, so declaring one moves the layer."""
    runner = grim_at(project_dir)
    plain = runner.json("build", str(_skill(project_dir / "a")))["layer_digest"]
    declared = runner.json("build", str(_skill(project_dir / "b", SKILL_REQ)))["layer_digest"]
    assert plain != declared


# ---------------------------------------------------------------------------
# Bad declarations refuse 65, with a positive control in each test
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "bad_meta",
    [
        '  grim.requires.widgets.x: "./x:1"\n',  # unknown table
        '  grim.requires.skills: "./x:1"\n',  # no binding name
        '  grim.requires.skills.Bad_Name: "./x:1"\n',  # invalid binding name
        '  grim.requires.skills.x: "no-registry:1"\n',  # bare reference
        '  grim.requires.skills.x: "./a/../b:1"\n',  # misplaced dot segment
    ],
)
def test_bad_skill_declaration_refuses_65(grim_at, project_dir: Path, bad_meta: str) -> None:
    runner = grim_at(project_dir)
    assert runner.run("build", str(_skill(project_dir / "ok", SKILL_REQ)), check=False).returncode == 0
    result = runner.run("build", str(_skill(project_dir / "bad", bad_meta)), check=False)
    assert result.returncode == 65, result.stderr
    assert "grim.requires" in result.stderr or "requires" in result.stderr, result.stderr


def test_bad_rule_and_agent_declarations_refuse_65(grim_at, project_dir: Path) -> None:
    runner = grim_at(project_dir)
    bad = '  grim.requires.skills.x: "no-registry:1"\n'
    result = runner.run("build", str(_rule(project_dir, bad)), check=False)
    assert result.returncode == 65, result.stderr
    result = runner.run("build", "--kind", "agent", str(_agent(project_dir, bad)), check=False)
    assert result.returncode == 65, result.stderr


@pytest.mark.parametrize(
    "bad",
    [
        '[requires.widgets]\nx = "./x:1"\n',
        '[requires.skills]\nBad_Name = "./x:1"\n',
        '[requires.skills]\nx = "no-registry:1"\n',
        '[requires.skills]\nx = 1\n',
    ],
)
def test_bad_mcp_and_hook_declarations_refuse_65(grim_at, project_dir: Path, bad: str) -> None:
    runner = grim_at(project_dir)
    assert runner.run("build", "--kind", "mcp", str(_mcp(project_dir / "ok", MCP_REQ)), check=False).returncode == 0
    result = runner.run("build", "--kind", "mcp", str(_mcp(project_dir / "bad", bad)), check=False)
    assert result.returncode == 65, result.stderr
    assert runner.run("build", str(_hook(project_dir / "ok", HOOK_REQ)), check=False).returncode == 0
    result = runner.run("build", str(_hook(project_dir / "bad", bad)), check=False)
    assert result.returncode == 65, result.stderr


def test_bad_bundle_member_ref_is_refused(grim_at, project_dir: Path) -> None:
    runner = grim_at(project_dir)
    ok = _bundle(project_dir / "ok", '[bundles]\ninner = "ghcr.io/acme/bundles/inner:1"\n')
    assert runner.run("build", str(ok), check=False).returncode == 0
    bad = _bundle(project_dir / "bad", '[bundles]\ninner = "no-registry:1"\n')
    result = runner.run("build", str(bad), check=False)
    assert result.returncode != 0, result.stderr


# ---------------------------------------------------------------------------
# Self-reference refuses 65 offline
# ---------------------------------------------------------------------------


def test_self_reference_refuses_65_for_every_kind(grim_at, project_dir: Path) -> None:
    runner = grim_at(project_dir)

    def refused(*argv: str) -> None:
        result = runner.run(*argv, check=False)
        assert result.returncode == 65, (argv, result.stderr)
        assert "requires itself" in result.stderr, result.stderr

    refused("build", str(_skill(project_dir / "s", '  grim.requires.skills.gws-drive: "./gws-drive:1"\n')))
    refused("build", str(_rule(project_dir / "r", '  grim.requires.rules.style: "./style:1"\n')))
    refused(
        "build",
        "--kind",
        "agent",
        str(_agent(project_dir / "a", '  grim.requires.agents.helper: "./helper:1"\n')),
    )
    refused(
        "build",
        "--kind",
        "mcp",
        str(_mcp(project_dir / "m", '[requires.mcp]\ngdrive = "./gdrive:1"\n')),
    )
    refused("build", str(_hook(project_dir / "h", '\n[requires.hooks]\nguard = "./guard:1"\n')))
    refused("build", str(_bundle(project_dir / "b", '[bundles]\nstack = "./stack:1"\n')))

    # Positive control: the same shape naming a different artifact builds.
    other = _skill(project_dir / "ok", '  grim.requires.skills.other: "./other:1"\n')
    assert runner.run("build", str(other), check=False).returncode == 0


# ---------------------------------------------------------------------------
# Byte identity
# ---------------------------------------------------------------------------


def test_mcp_descriptor_bytes_ignore_a_requires_table(grim_at, project_dir: Path) -> None:
    runner = grim_at(project_dir)
    plain = runner.json("build", "--kind", "mcp", str(_mcp(project_dir / "a")))
    declared = runner.json("build", "--kind", "mcp", str(_mcp(project_dir / "b", MCP_REQ)))
    assert plain["layer_digest"] == declared["layer_digest"]
    assert plain["annotation_count"] == declared["annotation_count"]


def test_dependency_free_sources_build_byte_identical_to_before(grim_at, project_dir: Path) -> None:
    """Digests recorded from a build that predates the requires grammar."""
    runner = grim_at(project_dir)
    digests = {
        "skill": runner.json("build", str(_skill(project_dir / "s")))["layer_digest"],
        "rule": runner.json("build", str(_rule(project_dir / "r")))["layer_digest"],
        "agent": runner.json("build", "--kind", "agent", str(_agent(project_dir / "a")))["layer_digest"],
        "mcp": runner.json("build", "--kind", "mcp", str(_mcp(project_dir / "m")))["layer_digest"],
        "hook": runner.json("build", str(_hook(project_dir / "h")))["layer_digest"],
        "bundle": runner.json(
            "build",
            str(_bundle(project_dir / "b", '[skills]\ncr = "ghcr.io/acme/cr:1"\n')),
        )["layer_digest"],
    }
    assert digests == EXPECTED_DIGESTS


EXPECTED_DIGESTS = {
    "skill": "sha256:9b8886d71a44a362806fba3c6e4e5516cbddbd860cdc44be520276f1f3de8802",
    "rule": "sha256:b7591db7e54cc852df5babf2e9e6436a8b035978c35785c9b1150ec5f149ec0e",
    "agent": "sha256:b279f35f1afac541fd1038282119e49a99902c8a3afb8aefa32b112d11d13f5a",
    "mcp": "sha256:fc349f626941a92ed418ab9f216f3ffc96d703a37e665aab78bf5d8ddb509cef",
    "hook": "sha256:688b7fbef857e085563971ffebac36e8bb62d3324ff48d8bef263bc684da7419",
    "bundle": "sha256:31e904330ce35561a83094e4acfafa227811e051acbc60ff6a25d1bfc77d784d",
}


# ---------------------------------------------------------------------------
# Nested bundle authoring
# ---------------------------------------------------------------------------


def test_bundle_source_lists_a_nested_bundle(grim_at, project_dir: Path) -> None:
    runner = grim_at(project_dir)
    flat = runner.json("build", str(_bundle(project_dir / "a", '[skills]\ncr = "ghcr.io/acme/cr:1"\n')))
    nested = runner.json(
        "build",
        str(
            _bundle(
                project_dir / "b",
                '[skills]\ncr = "ghcr.io/acme/cr:1"\n[bundles]\ninner = "ghcr.io/acme/bundles/inner:1"\n',
            )
        ),
    )
    assert nested["kind"] == "bundle"
    assert nested["annotation_count"] == flat["annotation_count"] + 1, (flat, nested)
    assert nested["layer_digest"] != flat["layer_digest"]
