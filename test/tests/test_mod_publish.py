# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Publishing the experimental `mod` kind: the feature token on manifests that
name a mod, release and batch-publish order, bundle and `requires` members,
the schema, and the settings refusal of the mod marketplace namespace."""

from __future__ import annotations

import json
from pathlib import Path

from src.helpers import (
    make_artifact,
    make_dependent,
    make_mod,
    make_settings,
    members_layer,
    settings_sandbox,
    write_config,
)
from src.registry import fetch_blob, fetch_manifest

FEATURES = "com.grimoire.requires-features"
MOD_TOML = 'schema = 1\nname = "band"\ndescription = "Build status band"\n'
REQUIRES_LAYER_MEDIA_TYPE = "application/vnd.grimoire.bundle.v1+json"


def _write(path: Path, body: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding="utf-8", newline="\n")
    return path


def _skill_tree(name: str) -> dict[str, str]:
    return {
        f"{name}/SKILL.md": f"---\nname: {name}\ndescription: A skill.\n---\n# {name}\n"
    }


def _mod_source(root: Path, name: str = "band", extra: str = "") -> Path:
    directory = root / name
    _write(
        directory / "mod.toml",
        f'schema = 1\nname = "{name}"\ndescription = "A mod"\n{extra}',
    )
    _write(directory / "claude" / f"{name}.ts", "export default {};\n")
    return directory


def test_requires_features_follow_mod_edges_only(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    runner = grim_at(project_dir)
    mod = make_mod(f"{unique_repo}/dep", "1.0.0")
    skill = make_artifact(
        f"{unique_repo}/lint", "skill", _skill_tree("lint"), tag="1.0.0"
    )

    # A bundle with a mod member carries the token; one that only coexists does not.
    with_mod = _write(
        project_dir / "with-mod.toml",
        f'[mods]\ndep = "{mod.fq}"\n\n[skills]\nlint = "{skill.fq}"\n',
    )
    plain = _write(project_dir / "plain.toml", f'[skills]\nlint = "{skill.fq}"\n')
    for source, name, token in ((with_mod, "with-mod", "mods"), (plain, "plain", None)):
        repo_path = f"{unique_repo}/bundles/{name}"
        runner.json("release", str(source), f"{registry}/{repo_path}:1.0.0")
        annotations = fetch_manifest(repo_path, "1.0.0")["annotations"]
        assert annotations.get(FEATURES) == token, name
    members = fetch_manifest(f"{unique_repo}/bundles/with-mod", "1.0.0")["layers"][0][
        "digest"
    ]
    assert fetch_blob(f"{unique_repo}/bundles/with-mod", members) == members_layer(
        [("skill", "lint", skill.fq), ("mod", "dep", mod.fq)]
    )

    # A mod requiring a mod carries it; a mod with no mod edge does not.
    needing = _mod_source(
        project_dir / "a", "needing", f'[requires.mods]\ndep = "{mod.fq}"\n'
    )
    alone = _mod_source(project_dir / "b", "alone")
    for source, name, token in ((needing, "needing", "mods"), (alone, "alone", None)):
        repo_path = f"{unique_repo}/{name}"
        runner.json("release", str(source), f"{registry}/{repo_path}:1.0.0")
        annotations = fetch_manifest(repo_path, "1.0.0")["annotations"]
        assert annotations.get(FEATURES) == token, name
        assert annotations["com.grimoire.kind"] == "mod"

    # A plain skill requiring a skill never carries it.
    skill_source = project_dir / "needs-skill"
    _write(
        skill_source / "SKILL.md",
        "---\nname: needs-skill\ndescription: A skill.\nmetadata:\n"
        f"  grim.requires.skills.lint: {skill.fq}\n---\n# x\n",
    )
    repo_path = f"{unique_repo}/needs-skill"
    runner.json("release", str(skill_source), f"{registry}/{repo_path}:1.0.0")
    assert FEATURES not in fetch_manifest(repo_path, "1.0.0")["annotations"]


def test_a_manifest_stamped_mods_resolves(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    root = project_dir / "consumer"
    root.mkdir()
    write_config(root)
    runner = grim_at(root)

    stamped = make_artifact(
        f"{unique_repo}/stamped",
        "skill",
        _skill_tree("stamped"),
        tag="1.0.0",
        annotations={FEATURES: "mods"},
    )
    runner.run("add", "--kind", "skill", stamped.fq)

    # Positive control for the refusal: an unknown token still stops the add.
    future = make_artifact(
        f"{unique_repo}/future",
        "skill",
        _skill_tree("future"),
        tag="1.0.0",
        annotations={FEATURES: "time-travel"},
    )
    refused = runner.run("add", "--kind", "skill", future.fq, check=False)
    assert refused.returncode == 65, refused.stderr


def test_release_and_publish_order(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    prefix = f"{unique_repo}/p"
    runner = grim_at(project_dir)
    _write(
        project_dir / "skills/s/SKILL.md",
        "---\nname: s\ndescription: A skill.\n---\n# s\n",
    )
    _write(
        project_dir / "settings/team.toml",
        'description = "d"\n[claude]\nmodel = "sonnet"\n',
    )
    _mod_source(project_dir / "mods", "band")
    _write(
        project_dir / "bundles/b.toml", f'[skills]\ns = "{registry}/{prefix}/s:0.1.0"\n'
    )

    # `--compression gzip` applies to the mod layer.
    ref = f"{registry}/{unique_repo}/gz:1.0.0"
    released = runner.json(
        "release", str(project_dir / "mods/band"), ref, "--compression", "gzip"
    )
    assert released["pushed"] is True
    layer_type = fetch_manifest(f"{unique_repo}/gz", "1.0.0")["layers"][0]["mediaType"]
    assert layer_type.endswith("tar+gzip"), layer_type

    manifest = _write(
        project_dir / "publish.toml",
        f'registry = "{registry}"\nrepository_prefix = "{prefix}"\nversion = "0.1.0"\n\n'
        "[skills.s]\n[settings.team]\n[mods.band]\n[bundles.b]\n",
    )
    rows = runner.json("publish", "--manifest", str(manifest))["items"]
    assert [row["kind"] for row in rows] == ["skill", "settings", "mod", "bundle"]
    assert all(row["status"] == "pushed" for row in rows), rows
    pushed = fetch_manifest(f"{prefix}/band", "0.1.0")
    assert pushed["annotations"]["com.grimoire.kind"] == "mod"
    assert pushed["annotations"]["com.grimoire.mod.clients"] == "claude"


def test_schema_is_additive(grim_at, project_dir: Path) -> None:
    runner = grim_at(project_dir)
    mod = json.loads(runner.plain("schema", "--kind", "mod").stdout)
    assert mod["$id"] == "https://grimoire.rs/schemas/grim-mod.schema.json"
    assert mod["required"] == ["schema", "name", "description"]
    assert mod["additionalProperties"] is False
    assert "requires" in mod["properties"]

    publish = json.loads(runner.plain("schema", "--kind", "publish").stdout)
    assert publish["required"] == ["registry"]
    assert publish["properties"]["mods"]["type"] == "object"


def test_bundle_member_and_requires_edge_resolve(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/dep", "1.0.0")
    skill = make_artifact(
        f"{unique_repo}/lint", "skill", _skill_tree("lint"), tag="1.0.0"
    )
    runner = grim_at(project_dir)

    # Authored mods-first: the sort puts the mod after every other kind.
    source = _write(
        project_dir / "starter.toml",
        f'[mods]\ndep = "{mod.fq}"\n\n[skills]\nlint = "{skill.fq}"\n',
    )
    repo_path = f"{unique_repo}/bundles/starter"
    runner.json("release", str(source), f"{registry}/{repo_path}:1.0.0")
    layer = fetch_manifest(repo_path, "1.0.0")["layers"][0]["digest"]
    blob = fetch_blob(repo_path, layer)
    assert [m["kind"] for m in json.loads(blob)["members"]] == ["skill", "mod"]
    assert blob == members_layer([("skill", "lint", skill.fq), ("mod", "dep", mod.fq)])

    # A mod's `[requires.mods]` rides its requires layer.
    needing = _mod_source(
        project_dir / "n", "needing", f'[requires.mods]\ndep = "{mod.fq}"\n'
    )
    needing_path = f"{unique_repo}/needing"
    runner.json("release", str(needing), f"{registry}/{needing_path}:1.0.0")
    layers = fetch_manifest(needing_path, "1.0.0")["layers"]
    assert layers[1]["mediaType"] == REQUIRES_LAYER_MEDIA_TYPE
    assert fetch_blob(needing_path, layers[1]["digest"]) == members_layer(
        [("mod", "dep", mod.fq)]
    )

    # A hand-pushed dependent skill naming a mod keeps the edge in its layer.
    make_dependent(
        f"{unique_repo}/needs-mod",
        "1.0.0",
        "skill",
        _skill_tree("needs-mod"),
        [("mod", "dep", mod.fq)],
    )
    layers = fetch_manifest(f"{unique_repo}/needs-mod", "1.0.0")["layers"]
    assert fetch_blob(f"{unique_repo}/needs-mod", layers[1]["digest"]) == members_layer(
        [("mod", "dep", mod.fq)]
    )


def test_settings_marketplace_name_grim_mods_refuses(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    runner = grim_at(project_dir)
    market = (
        'description = "d"\n[claude.extraKnownMarketplaces.{name}]\n'
        'source = {{ source = "github", repo = "acme/plugins" }}\n'
    )
    good = _write(
        project_dir / "settings" / "good.toml", market.format(name="team-plugins")
    )
    assert runner.json("build", str(good), "--kind", "settings")["kind"] == "settings"

    bad = _write(
        project_dir / "settings" / "bad.toml",
        market.format(name="grim-mods-0123456789ab"),
    )
    refused = runner.run("build", str(bad), "--kind", "settings", check=False)
    assert refused.returncode == 65, refused.stderr
    assert "mod kind" in refused.stderr


def test_install_refuses_a_hand_pushed_grim_mods_marketplace(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    source = {"source": {"source": "github", "repo": "acme/plugins"}}

    def document(name: str) -> dict:
        return {
            "description": "d",
            "claude": {"extraKnownMarketplaces": {name: source}},
        }

    good = make_settings(f"{unique_repo}/good", "1", document("team-plugins"))
    bad = make_settings(f"{unique_repo}/bad", "1", document("grim-mods-0123456789ab"))

    outcomes = {}
    for label, package in (("good", good), ("bad", bad)):
        root = project_dir / label
        root.mkdir()
        write_config(root, settings={label: package.fq}, experimental_settings=True)
        runner = grim_at(root)
        runner.env.update(settings_sandbox(tmp_path / label, runner.home)[0])
        runner.run("lock", check=False)
        outcomes[label] = runner.run("install", "--trust-workspace", check=False)

    assert outcomes["good"].returncode == 0, outcomes["good"].stderr
    assert outcomes["bad"].returncode == 65, outcomes["bad"].stderr
    assert "mod kind" in outcomes["bad"].stderr


def test_requirer_of_a_mod_is_stamped_with_the_mods_feature(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/dep", "1.0.0")
    runner = grim_at(project_dir)
    source = project_dir / "needs-mod"
    _write(
        source / "SKILL.md",
        "---\nname: needs-mod\ndescription: A skill.\nmetadata:\n"
        f"  grim.requires.mods.dep: {mod.fq}\n---\n# x\n",
    )
    repo_path = f"{unique_repo}/needs-mod"

    runner.json("release", str(source), f"{registry}/{repo_path}:1.0.0")

    annotations = fetch_manifest(repo_path, "1.0.0")["annotations"]
    assert annotations.get(FEATURES) == "mods"
