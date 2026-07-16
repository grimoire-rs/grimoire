# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim add` kind-inference and name-override acceptance tests.

`grim add <reference>` now requires only the reference.  When `--kind` is
omitted, the kind is inferred from the manifest's kind metadata (the
`com.grimoire.kind` annotation; legacy `artifactType`/config media type
fallbacks).  When `--name` is omitted, the binding name defaults to the
reference's last path segment.  Both flags remain overridable.  A reference
that cannot be resolved yields exit 65 (DataError / KindInferenceFailed).
"""
from __future__ import annotations

from pathlib import Path

from src.helpers import make_artifact, write_config
from src.registry import fetch_manifest


def test_add_infers_kind_and_name_from_manifest(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Kind and name are inferred from the published manifest when omitted."""
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# Rust Style\n"},
        tag="v1",
    )
    write_config(project_dir)
    runner = grim_at(project_dir)

    out = runner.json("add", ru.fq)
    assert out["kind"] == "rule", (
        f"kind must be inferred as 'rule' from the manifest annotation, got {out['kind']!r}"
    )
    assert out["name"] == "rust-style", (
        f"name must default to the last path segment 'rust-style', got {out['name']!r}"
    )
    assert out["status"] == "added"
    assert "@sha256:" in out["pinned"]


def test_legacy_shaped_manifest_types_kind_via_artifact_type(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Read tier 1 (`adr_oci_empty_config_compat.md`): a legacy-shaped manifest
    that still carries the custom `artifactType` resolves its kind from that
    tier. The harness (`registry.py push_artifact`) deliberately emits a richer
    manifest than grim's own output — it stamps `artifactType` AND the
    `com.grimoire.kind` annotation over the OCI empty config — so this exercises
    the backward-compat read path. grim's own writes carry only the annotation
    (see `test_release_wire_shape_*`)."""
    repo = f"{unique_repo}/rust-style"
    ru = make_artifact(
        repo,
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# Rust Style\n"},
        tag="v1",
    )

    manifest = fetch_manifest(repo, "v1")
    assert manifest["artifactType"] == "application/vnd.grimoire.rule.v1", (
        f"manifest must carry the Grimoire artifactType, got {manifest.get('artifactType')!r}"
    )
    assert manifest["config"]["mediaType"] == "application/vnd.oci.empty.v1+json", (
        f"config descriptor must be the OCI empty type, got {manifest['config']['mediaType']!r}"
    )
    assert manifest.get("annotations", {}).get("com.grimoire.kind") == "rule", (
        f"manifest must carry the com.grimoire.kind fallback annotation, "
        f"got {manifest.get('annotations', {})!r}"
    )

    # End-to-end: kind inference resolves at the artifactType read tier on this
    # harness-built legacy-shaped manifest.
    write_config(project_dir)
    out = grim_at(project_dir).json("add", ru.fq)
    assert out["kind"] == "rule"


def test_add_name_override_replaces_inferred_name(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """--name overrides the default segment-based name."""
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n"},
        tag="stable",
    )
    write_config(project_dir)
    runner = grim_at(project_dir)

    # `--no-install`: declaration-only check (the install path of a
    # rebound skill is covered by
    # test_add_name_override_installs_rebound_skill below).
    out = runner.json("add", "--no-install", sk.fq, "--name", "cr")
    assert out["name"] == "cr", (
        f"--name 'cr' must override the default segment name, got {out['name']!r}"
    )
    assert out["kind"] == "skill"

    # The config binding name must match the --name value.
    # The FQ reference in the value still contains "code-review" (that's the
    # repo path), but the KEY must be "cr", not "code-review".
    cfg = (project_dir / "grimoire.toml").read_text()
    skills_section = cfg.split("[skills]")[1].split("[rules]")[0]
    assert 'cr = "' in skills_section, (
        f"config skills section must have key 'cr', got:\n{skills_section}"
    )
    assert not skills_section.strip().startswith("code-review"), (
        "config skills key must be 'cr', not 'code-review'"
    )


def test_add_name_override_installs_rebound_skill(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A `--name` rebinding installs: the tree lands under the binding
    directory and the installed SKILL.md frontmatter `name` is rewritten
    to the binding (Agent Skills directory-equality rule)."""
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {
            "code-review/SKILL.md": (
                "---\nname: code-review\ndescription: d\n---\n# CR\n"
            ),
            "code-review/scripts/run.sh": "echo hi\n",
        },
        tag="stable",
    )
    write_config(project_dir)
    runner = grim_at(project_dir)

    out = runner.json("add", sk.fq, "--name", "cr")
    assert out["name"] == "cr"

    index = project_dir / ".claude/skills/cr/SKILL.md"
    assert index.is_file(), "rebound skill must materialize under the binding dir"
    doc = index.read_text()
    assert "name: cr" in doc, f"frontmatter name must be rewritten to the binding:\n{doc}"
    assert "name: code-review" not in doc, f"stale artifact name must be gone:\n{doc}"
    assert doc.endswith("# CR\n"), f"body must be preserved:\n{doc}"
    assert (
        project_dir / ".claude/skills/cr/scripts/run.sh"
    ).read_text() == "echo hi\n", "sibling files stay verbatim"


def test_add_rejects_invalid_binding_name(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A binding name outside the artifact-name charset refuses (exit 64):
    the binding becomes an install directory/file name, and mixed-case
    bindings collide on case-insensitive filesystems."""
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n"},
        tag="stable",
    )
    write_config(project_dir)
    runner = grim_at(project_dir)

    result = runner.run("add", sk.fq, "--name", "Code-Review", check=False)
    assert result.returncode == 64, (
        f"mixed-case binding must exit 64, got {result.returncode}; "
        f"stderr: {result.stderr}"
    )
    assert "lowercase" in result.stderr, (
        f"error must explain the charset; stderr:\n{result.stderr}"
    )
    # Nothing declared, nothing installed.
    cfg = (project_dir / "grimoire.toml").read_text()
    assert "Code-Review" not in cfg, "refused add must not write the config"


def test_add_kind_override_still_works(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Explicit --kind still overrides inference."""
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n"},
        tag="stable",
    )
    write_config(project_dir)
    runner = grim_at(project_dir)

    # Pass --kind explicitly (even if it matches what would be inferred).
    out = runner.json("add", sk.fq, "--kind", "skill")
    assert out["kind"] == "skill"
    assert out["name"] == "code-review"
    assert out["status"] == "added"


def test_add_missing_reference_kind_inference_fails_exit_65(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A reference that does not resolve fails kind inference: exit 65."""
    write_config(project_dir)
    runner = grim_at(project_dir)

    missing_ref = f"{registry}/{unique_repo}/missing:latest"
    result = runner.run("add", missing_ref, check=False)
    assert result.returncode == 65, (
        f"kind inference failure for an unresolvable reference must exit 65, "
        f"got {result.returncode}; stderr: {result.stderr}"
    )


def test_add_dotted_name_installs_end_to_end(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Issue #40: a dotted repo segment ('socket.io') derives a valid dotted
    binding, declares it under the dotted config key, and installs the tree
    under the dotted directory."""
    sk = make_artifact(
        f"{unique_repo}/socket.io",
        "skill",
        {"socket.io/SKILL.md": "---\nname: socket.io\ndescription: d\n---\n# S\n"},
        tag="stable",
    )
    write_config(project_dir)
    runner = grim_at(project_dir)

    out = runner.json("add", sk.fq)
    assert out["kind"] == "skill"
    assert out["name"] == "socket.io", (
        f"binding must default to the dotted segment, got {out['name']!r}"
    )
    assert out["status"] == "added"

    cfg = (project_dir / "grimoire.toml").read_text()
    skills_section = cfg.split("[skills]")[1].split("[rules]")[0]
    assert '"socket.io" = "' in skills_section, (
        f"config skills section must quote the dotted binding key "
        f"(a bare dotted key parses as a nested table), got:\n{skills_section}"
    )
    index = project_dir / ".claude/skills/socket.io/SKILL.md"
    assert index.is_file(), "dotted skill must materialize under the dotted dir"

    # A second invocation forces a re-read of the written grimoire.toml —
    # regression guard: a bare dotted key made every follow-up command fail
    # with 'invalid TOML ... invalid type: map, expected a string' (exit 78).
    rows = runner.json("status")["items"]
    assert any(row["name"] == "socket.io" for row in rows), (
        f"re-read config must surface the dotted binding, got: {rows}"
    )


def test_add_accepts_dotted_name_override(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Issue #40: an explicit dotted `--name` is a valid binding."""
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n"},
        tag="stable",
    )
    write_config(project_dir)
    runner = grim_at(project_dir)

    out = runner.json("add", "--no-install", sk.fq, "--name", "my.alias")
    assert out["name"] == "my.alias", (
        f"--name 'my.alias' must be accepted, got {out['name']!r}"
    )


def test_add_rejects_dotted_edge_binding_names(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Issue #40 guard rails: dotted names with separator-edge violations
    ('.hidden', 'a..b') stay refused with exit 64 — they would materialize
    hidden or traversal-adjacent install paths."""
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n"},
        tag="stable",
    )
    write_config(project_dir)
    runner = grim_at(project_dir)

    for bad in (".hidden", "a..b"):
        result = runner.run("add", sk.fq, "--name", bad, check=False)
        assert result.returncode == 64, (
            f"binding {bad!r} must exit 64, got {result.returncode}; "
            f"stderr: {result.stderr}"
        )
        cfg = (project_dir / "grimoire.toml").read_text()
        assert bad not in cfg, f"refused add must not write binding {bad!r}"
