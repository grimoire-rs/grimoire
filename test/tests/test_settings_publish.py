# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Publishing the `settings` kind: build, release, batch publish, schema, bundle
membership, `requires` edges, export and discovery.

A settings package is one TOML document (`settings/<name>.toml`) that releases
as a single canonical-JSON layer with the kind riding on the
``com.grimoire.kind`` annotation. Consuming one is covered elsewhere. Fixtures a
consumer must read are pushed by ``make_settings``, never by the grim under test.
"""
from __future__ import annotations

import json
import tomllib
from pathlib import Path
from urllib.error import HTTPError

import pytest

from src.helpers import (
    SETTINGS_LAYER_MEDIA_TYPE,
    make_artifact,
    make_bundle,
    make_dependent,
    make_hook,
    make_settings,
    members_layer,
    settings_layer,
    write_config,
)
from src.registry import REGISTRY_HOST, fetch_blob, fetch_manifest, tag_digest

SOURCE_ROOT = Path(__file__).resolve().parents[2]

DOCUMENT = """\
description = "Team defaults for every harness."
summary = "team defaults"
keywords = "team,defaults"
license = "Apache-2.0"

[claude]
model = "sonnet"

[claude.permissions]
allow = ["Read"]

[codex]
model = "gpt-5"

[[codex.rules]]
pattern = ["git", "status"]
decision = "allow"
"""

# What DOCUMENT must become on the wire: descriptive keys and vendor tables
# side by side, keys sorted.
LAYER = {
    "claude": {"model": "sonnet", "permissions": {"allow": ["Read"]}},
    "codex": {"model": "gpt-5", "rules": [{"decision": "allow", "pattern": ["git", "status"]}]},
    "description": "Team defaults for every harness.",
    "keywords": "team,defaults",
    "license": "Apache-2.0",
    "summary": "team defaults",
}

# Planted in refused values: a message that repeats it has leaked a value.
MARKER = "fake-settings-value-0001"

TEAM = {"description": "Team defaults.", "claude": {"model": "sonnet"}}

REQUIRES_LAYER_MEDIA_TYPE = "application/vnd.grimoire.bundle.v1+json"


def _skill_tree(name: str) -> dict[str, str]:
    return {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: A skill.\n---\n# {name}\n"}


def _write(path: Path, body: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body)
    return path


def _source(project_dir: Path, body: str = DOCUMENT, name: str = "team-defaults") -> Path:
    return _write(project_dir / "settings" / f"{name}.toml", body)


def _assert_absent(repo_path: str, tag: str) -> None:
    with pytest.raises(HTTPError) as refused:
        tag_digest(repo_path, tag)
    assert refused.value.code == 404


def _consumer(grim_at, project_dir: Path, name: str):
    """A fresh project under ``project_dir`` and a runner rooted there."""
    root = project_dir / name
    root.mkdir()
    write_config(root)
    return root, grim_at(root)


def _lock(root: Path) -> dict:
    return tomllib.loads((root / "grimoire.lock").read_text())


def _pulled_in(runner, reference: str) -> list[tuple[str, str]]:
    report = json.loads(runner.run("--format", "json", "add", reference).stdout)
    return [(row["kind"], row["name"]) for row in report["pulled_in"]]


def test_build_release_publish(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    source = _source(project_dir)
    runner = grim_at(project_dir)

    built = runner.json("build", str(source), "--kind", "settings")
    assert built["kind"] == "settings"
    assert built["name"] == "team-defaults"

    repo_path = f"{unique_repo}/settings/team-defaults"
    ref = f"{registry}/{repo_path}:1.0.0"
    released = runner.json("release", str(source), ref, "--kind", "settings")
    assert released["pushed"] is True
    assert set(released["tags"]) == {"1.0.0", "1.0", "1", "latest"}

    manifest = fetch_manifest(repo_path, "1.0.0")
    assert manifest["config"]["mediaType"] == "application/vnd.oci.empty.v1+json"
    assert "artifactType" not in manifest
    assert [layer["mediaType"] for layer in manifest["layers"]] == [SETTINGS_LAYER_MEDIA_TYPE]
    annotations = manifest["annotations"]
    assert annotations["com.grimoire.kind"] == "settings"
    assert annotations["org.opencontainers.image.title"] == "team-defaults"
    assert annotations["org.opencontainers.image.version"] == "1.0.0"
    assert annotations["org.opencontainers.image.licenses"] == "Apache-2.0"
    assert annotations["com.grimoire.summary"] == "team defaults"

    # The bytes are the canonical form, and `build` previews the same digest.
    digest = manifest["layers"][0]["digest"]
    assert fetch_blob(repo_path, digest) == settings_layer(LAYER)
    assert built["layer_digest"] == digest

    again = runner.json("release", str(source), ref, "--kind", "settings")
    assert again["manifest_digest"] == released["manifest_digest"], "a release is idempotent"

    described = runner.json("describe", ref)
    assert described["kind"] == "settings"
    assert described["description"] == "Team defaults for every harness."
    assert described["keywords"] == ["team", "defaults"]

    fetched = runner.json("fetch", ref)
    assert json.loads(fetched["content"]) == LAYER

    # Positive control for the cap below: a pushed layer under it reads.
    small = make_settings(f"{unique_repo}/settings/small", "1.0.0", {"description": "small"})
    assert json.loads(runner.json("fetch", small.fq)["content"]) == {"description": "small"}
    large = make_settings(f"{unique_repo}/settings/large", "1.0.0", {"description": "x" * 70_000})
    capped = runner.run("fetch", large.fq, check=False)
    assert capped.returncode != 0, capped.stdout
    assert str(70_000) in capped.stderr or "65536" in capped.stderr, capped.stderr

    # A `.toml` still means a bundle without the flag.
    bundle = _write(project_dir / "b.toml", '[skills]\na = "ghcr.io/x/a:1"\n')
    assert runner.json("build", str(bundle))["kind"] == "bundle"


def test_publish_orders_settings_after_hooks_and_before_bundles(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    prefix = f"{unique_repo}/p"
    _write(project_dir / "skills/s/SKILL.md", "---\nname: s\ndescription: A skill.\n---\n# s\n")
    _write(
        project_dir / "mcp/m.toml",
        'description = "d"\n[server]\ntransport = "stdio"\ncommand = "grim"\n',
    )
    _write(
        project_dir / "hooks/h/hook.toml",
        'schema = 1\nname = "h"\ndescription = "d"\n'
        '[[hooks]]\nid = "g"\nevent = "PreToolUse"\ntier = "observer"\n'
        'argv = ["sh", "g.sh"]\ntimeout = 5\npayload = "stdin"\n',
    )
    _write(project_dir / "hooks/h/g.sh", "exit 0\n")
    _write(project_dir / "bundles/b.toml", f'[skills]\ns = "{registry}/{prefix}/s:0.1.0"\n')
    _source(project_dir)
    manifest = _write(
        project_dir / "publish.toml",
        f'registry = "{registry}"\nrepository_prefix = "{prefix}"\nversion = "0.1.0"\n\n'
        "[skills.s]\n[mcp.m]\n[hooks.h]\n[bundles.b]\n"
        "[settings.team-defaults]\ndescription = false\n"
        '[settings.team-defaults.metadata]\nvendor = "entry-vendor"\n',
    )

    rows = grim_at(project_dir).json("publish", "--manifest", str(manifest))["items"]

    assert [row["kind"] for row in rows] == ["skill", "mcp", "hook", "settings", "bundle"]
    assert all(row["status"] == "pushed" for row in rows), rows
    assert rows[3]["compression"] is None, "a JSON layer is never compressed"
    pushed = fetch_manifest(f"{prefix}/team-defaults", "0.1.0")
    assert pushed["annotations"]["com.grimoire.kind"] == "settings"
    assert pushed["annotations"]["org.opencontainers.image.vendor"] == "entry-vendor"
    assert [layer["mediaType"] for layer in pushed["layers"]] == [SETTINGS_LAYER_MEDIA_TYPE]


@pytest.mark.parametrize(
    ("body", "pointer"),
    [
        pytest.param(
            f'description = "d"\n[claude]\nnot_a_key = "{MARKER}"\n',
            "/not_a_key",
            id="unknown-field",
        ),
        pytest.param(
            f'description = "d"\n[future]\nkey = "{MARKER}"\n',
            "future",
            id="unknown-vendor",
        ),
        pytest.param(
            'description = "d"\n[claude.permissions]\nallow = "Read"\n',
            "/permissions/allow",
            id="wrong-type",
        ),
        pytest.param(
            f'description = "d"\n[opencode]\nmodel = "{{env:{MARKER}}}"\n',
            "/opencode/model",
            id="expansion-token",
        ),
        pytest.param(
            f'description = "d"\n[kilo]\nmodel = "{{file:{MARKER}}}"\n',
            "/kilo/model",
            id="expansion-token-kilo",
        ),
        pytest.param(
            f'description = "d"\n[gemini.model]\nname = "${MARKER}"\n',
            "/gemini/model/name",
            id="variable-reference-gemini",
        ),
        pytest.param(
            'description = "d"\n[[codex.rules]]\npattern = ["git"]\n'
            f'decision = "allow"\njustification = "{MARKER}\\u0007"\n',
            "rule 0",
            id="rule-control-character",
        ),
    ],
)
def test_invalid_document_exits_65(
    grim_at, project_dir: Path, registry: str, unique_repo: str, body: str, pointer: str
) -> None:
    source = _source(project_dir, body)
    runner = grim_at(project_dir)

    built = runner.run("build", str(source), "--kind", "settings", check=False)
    assert built.returncode == 65, built.stderr
    assert pointer in built.stderr
    assert str(source) in built.stderr, "the message names the file"
    assert MARKER not in built.stderr, "a value never reaches a message"

    repo_path = f"{unique_repo}/settings/team-defaults"
    ref = f"{registry}/{repo_path}:1.0.0"
    released = runner.run("release", str(source), ref, "--kind", "settings", check=False)
    assert released.returncode == 65, released.stderr
    assert pointer in released.stderr
    assert MARKER not in released.stderr
    _assert_absent(repo_path, "1.0.0")

    # Positive control: the same path and repository take a valid document.
    _source(project_dir)
    assert runner.json("release", str(source), ref, "--kind", "settings")["pushed"] is True


def test_an_unknown_setting_carries_the_unknown_key_hint(grim_at, project_dir: Path) -> None:
    runner = grim_at(project_dir)
    source = _source(project_dir, 'description = "d"\n[claude]\nfutureKey = 1\n')

    plain = runner.run("build", str(source), "--kind", "settings", check=False)
    assert plain.returncode == 65, plain.stderr
    assert "claude has no setting at /futureKey" in plain.stderr
    assert "upgrade grim" in plain.stderr

    error = json.loads(runner.run("build", str(source), "--kind", "settings", format="json", check=False).stdout)
    assert "upgrade grim" in error["error"]["hint"], error


def test_a_bare_dotted_key_is_told_to_quote_it(grim_at, project_dir: Path) -> None:
    runner = grim_at(project_dir)
    bare = _source(project_dir, 'description = "d"\n[kiro]\nchat.defaultModel = "m"\n')

    refused = runner.run("build", str(bare), "--kind", "settings", check=False)
    assert refused.returncode == 65, refused.stderr
    assert 'quote the whole key in TOML, as in "chat.defaultModel"' in refused.stderr

    # Positive control: the quoted key is the one Kiro reads.
    _source(project_dir, 'description = "d"\n[kiro]\n"chat.defaultModel" = "m"\n')
    assert runner.json("build", str(bare), "--kind", "settings")["name"] == "team-defaults"


def test_build_refuses_a_name_add_would_refuse(grim_at, project_dir: Path) -> None:
    runner = grim_at(project_dir)
    refused = runner.run(
        "build", str(_source(project_dir, name="Team_Defaults")), "--kind", "settings", check=False
    )
    assert refused.returncode == 65, refused.stderr
    assert "Team_Defaults" in refused.stderr

    good = _source(project_dir, name="team.defaults-2")
    assert runner.json("build", str(good), "--kind", "settings")["name"] == "team.defaults-2"


def test_schema_is_additive(grim_at, project_dir: Path) -> None:
    runner = grim_at(project_dir)

    settings = json.loads(runner.plain("schema", "--kind", "settings").stdout)
    assert settings["$id"] == "https://grimoire.rs/schemas/grim-settings.schema.json"
    assert settings["required"] == ["description"]
    assert settings["additionalProperties"] is False
    for client in (
        "amp",
        "antigravity",
        "claude",
        "codex",
        "copilot",
        "cursor",
        "droid",
        "gemini",
        "kilo",
        "kiro",
        "opencode",
        "qoder",
        "warp",
    ):
        assert settings["properties"][client]["type"] == "object", client
    assert "requires" in settings["properties"]
    assert "requires" not in settings["required"]

    # The batch manifest gains one optional table and requires nothing new.
    publish = json.loads(runner.plain("schema", "--kind", "publish").stdout)
    assert publish["required"] == ["registry"]
    assert publish["properties"]["settings"]["type"] == "object"

    # The shipped kinds keep their bytes.
    hook = (SOURCE_ROOT / "src/command/testdata/hook.schema.json").read_text()
    assert runner.plain("schema", "--kind", "hook").stdout == hook
    lock = (SOURCE_ROOT / "src/lock/testdata/lock.schema.json").read_text()
    assert runner.plain("schema", "--kind", "lock").stdout == lock


def test_bundle_member_and_requires_edge(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    team = make_settings(f"{unique_repo}/settings/team", "1.0.0", TEAM)
    skill = make_artifact(f"{unique_repo}/lint", "skill", _skill_tree("lint"), tag="1.0.0")
    hook = make_hook(f"{unique_repo}/guard", "guard", tag="1.0.0")
    runner = grim_at(project_dir)

    # Authored settings-first: the order comes from the sort, not from the file.
    bundle_source = _write(
        project_dir / "starter.toml",
        f'[settings]\nteam = "{team.fq}"\n\n'
        f'[hooks]\nguard = "{hook.fq}"\n\n'
        f'[skills]\nlint = "{skill.fq}"\n',
    )
    assert runner.json("build", str(bundle_source))["kind"] == "bundle"
    bundle_path = f"{unique_repo}/bundles/starter"
    runner.json("release", str(bundle_source), f"{registry}/{bundle_path}:1.0.0")
    members = fetch_blob(bundle_path, fetch_manifest(bundle_path, "1.0.0")["layers"][0]["digest"])
    assert [m["kind"] for m in json.loads(members)["members"]] == ["skill", "hook", "settings"]
    assert members == members_layer(
        [("skill", "lint", skill.fq), ("hook", "guard", hook.fq), ("settings", "team", team.fq)]
    )

    # A member of a hand-pushed bundle reaches the lock under the bundle.
    pushed = make_bundle(
        f"{unique_repo}/bundles/pushed",
        [("settings", "team", team.fq), ("skill", "lint", skill.fq)],
        tag="1.0.0",
    )
    root, consumer = _consumer(grim_at, project_dir, "through-a-bundle")
    consumer.run("add", "--kind", "bundle", pushed.fq)
    locked = _lock(root)["settings"]
    assert [row["name"] for row in locked] == ["team"]
    assert team.digest in locked[0]["pinned"], "the member keeps its own digest"
    assert locked[0]["bundle"] == f"{registry}/{unique_repo}/bundles/pushed"

    # Out of a settings package: it pulls in what it requires.
    with_lint = make_settings(
        f"{unique_repo}/settings/with-lint", "1.0.0", TEAM, requires=[("skill", "lint", skill.fq)]
    )
    root, consumer = _consumer(grim_at, project_dir, "out-of-settings")
    assert _pulled_in(consumer, with_lint.fq) == [("skill", "lint")]
    assert [row["name"] for row in _lock(root)["skill"]] == ["lint"]

    # Into a settings package: a skill requires it.
    needs_team = make_dependent(
        f"{unique_repo}/needs-team",
        "1.0.0",
        "skill",
        _skill_tree("needs-team"),
        [("settings", "team", team.fq)],
    )
    root, consumer = _consumer(grim_at, project_dir, "into-settings")
    assert _pulled_in(consumer, needs_team.fq) == [("settings", "team")]
    assert [row["name"] for row in _lock(root)["settings"]] == ["team"]

    # Authored: `requires` leaves the document and rides its own layer.
    source = _source(
        project_dir,
        DOCUMENT
        + f'\n[requires.skills]\nlint = "{skill.fq}"\n'
        + f'\n[requires.settings]\nteam = "{team.fq}"\n',
        name="with-requires",
    )
    repo_path = f"{unique_repo}/settings/with-requires"
    runner.json("release", str(source), f"{registry}/{repo_path}:1.0.0", "--kind", "settings")
    layers = fetch_manifest(repo_path, "1.0.0")["layers"]
    assert [layer["mediaType"] for layer in layers] == [
        SETTINGS_LAYER_MEDIA_TYPE,
        REQUIRES_LAYER_MEDIA_TYPE,
    ]
    assert fetch_blob(repo_path, layers[0]["digest"]) == settings_layer(LAYER)
    assert fetch_blob(repo_path, layers[1]["digest"]) == members_layer(
        [("skill", "lint", skill.fq), ("settings", "team", team.fq)]
    )


def test_export_declines(grim_at, tmp_path: Path, registry: str, unique_repo: str) -> None:
    team = make_settings(f"{unique_repo}/team", "1.0.0", TEAM)
    skill = make_artifact(f"{unique_repo}/lint", "skill", _skill_tree("lint"), tag="1.0.0")
    warning = "settings 'team' omitted from plugin 'starter'"

    def export(root: Path, skills: dict[str, str]):
        root.mkdir()
        config = write_config(root, skills=skills, settings={"team": team.fq})
        config.write_text(config.read_text() + '\n[plugin]\nname = "starter"\n')
        runner = grim_at(root)
        runner.run("lock")
        clients = ("--client", "claude", "--client", "codex")
        return runner.run(
            "export", "plugin", "--project", *clients, "-o", "dist", format="json", check=False
        )

    # Next to a skill: omitted per client, warned once, and the skill still ships.
    project = tmp_path / "mixed"
    result = export(project, {"lint": skill.fq})
    assert result.returncode == 0, result.stderr
    assert result.stderr.count(warning) == 1, result.stderr
    omitted = {"kind": "settings", "name": "team", "reason": "no-format-surface"}
    items = json.loads(result.stdout)["items"]
    assert {item["client"] for item in items} == {"claude", "codex"}
    for item in items:
        assert item["omitted"] == [omitted], item
        tree = project / "dist" / f"starter.{item['client']}"
        assert (tree / "skills" / "lint" / "SKILL.md").is_file()
        assert f"Omitted for {item['client']}: settings team." in (tree / "README.md").read_text()
        assert not (tree / "settings.json").exists(), "nothing of the document is staged"

    # Alone it leaves the plugin empty: refused, and the run above shows where a
    # tree would have landed.
    only = tmp_path / "only"
    refused = export(only, {})
    assert refused.returncode == 65, refused.stderr
    assert "has no member the 'claude' plugin format can carry" in refused.stderr
    assert not (only / "dist" / "starter.claude").exists()

    # A marketplace reaches the same gate through an `include` reference.
    work = tmp_path / "market"
    work.mkdir()
    _write(
        work / "marketplace.toml",
        '[marketplace]\nname = "acme"\n\n[marketplace.owner]\nname = "Acme"\n\n'
        f'[plugins.starter]\ninclude = ["{skill.fq}", "{team.fq}"]\n',
    )
    market = grim_at(work).run("export", "marketplace", format="json", check=False)
    assert market.returncode == 0, market.stderr
    assert market.stderr.count(warning) == 1, market.stderr
    rows = json.loads(market.stdout)["items"]
    assert rows, market.stdout
    for row in rows:
        assert row["omitted"] == [omitted], row
        assert [member["name"] for member in row["members"]] == ["lint"], row


def test_search_and_describe_show_the_kind(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    team = make_settings(f"{unique_repo}/team-defaults", "1.0.0", TEAM)
    # The positive control for the filter: its name holds the keyword, so only a
    # kind filter keeps it out.
    make_artifact(
        f"{unique_repo}/settings-helper", "skill", _skill_tree("settings-helper"), tag="1.0.0"
    )
    root, runner = _consumer(grim_at, project_dir, "consumer")
    scoped = ("--registry", f"{REGISTRY_HOST}/{unique_repo}")

    warm = runner.json("search", unique_repo, *scoped, "--refresh")["items"]
    assert {row["repo"].rsplit("/", 1)[-1]: row["kind"] for row in warm} == {
        "team-defaults": "settings",
        "settings-helper": "skill",
    }

    for keyword in ("settings", "setting", "SETTINGS"):
        rows = runner.json("--offline", "search", f"{unique_repo} {keyword}", *scoped)["items"]
        assert [row["repo"].rsplit("/", 1)[-1] for row in rows] == ["team-defaults"], keyword

    assert runner.json("describe", team.fq)["kind"] == "settings"

    # Added with no `--kind`, the manifest says what it is.
    report = json.loads(runner.run("--format", "json", "add", team.fq).stdout)
    assert report["kind"] == "settings"
    assert tomllib.loads((root / "grimoire.toml").read_text())["settings"]["team-defaults"]
    assert team.digest in _lock(root)["settings"][0]["pinned"]
