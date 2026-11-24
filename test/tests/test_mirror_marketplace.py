# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Done 4: a Claude plugin marketplace mirrors each plugin's members and one bundle per plugin."""
from __future__ import annotations

import json
from datetime import datetime
from pathlib import Path
from urllib.error import HTTPError

import pytest

from src.git_fixture import BASE_DATE, GitRepo, agent_md, marketplace, skill_md
from src.helpers import write_config
from src.registry import fetch_blob, fetch_manifest, list_tags

EXT_DATE = "2026-03-05T12:00:00+00:00"
GPL_TEXT = (
    Path(__file__).resolve().parents[2] / "crates" / "grim-mirror" / "tests" / "licenses" / "gpl-3.0.verbatim.txt"
).read_text(encoding="utf-8")

MIT_TEXT = (Path(__file__).resolve().parents[2] / "crates" / "grim-mirror" / "licenses" / "MIT.txt").read_text(encoding="utf-8")
MARKET_FILE = ".claude-plugin/marketplace.json"


def _mirror_file(tmp_path: Path, registry: str, unique_repo: str, repo_url: str, mirror_extra: str = "") -> Path:
    path = tmp_path / "mirror.toml"
    path.write_text(
        f'schema = 1\n[mirror]\nregistry = "{registry}/{unique_repo}"\n{mirror_extra}[[upstream]]\nname = "up"\n'
        f'repo = "{repo_url}"\nmarketplace = "{MARKET_FILE}"\nplugins = ["*"]\n',
        encoding="utf-8",
        newline="\n",
    )
    return path


def _sync(grim_mirror, mf: Path):
    r = grim_mirror.run("--file", str(mf), "--format", "json", "sync")
    return json.loads(r.stdout)


def _external(path: Path, extra: dict[str, str] | None = None) -> GitRepo:
    repo = GitRepo.init(path)
    repo.record(
        "external plugin",
        {"LICENSE": MIT_TEXT, "plugins/ext/skills/ext-skill/SKILL.md": skill_md("ext-skill"), **(extra or {})},
        date=EXT_DATE,
    )
    return repo


def _market_with(path: Path, plugins: list[dict], files: dict[str, str] | None = None) -> GitRepo:
    repo = GitRepo.init(path)
    document = {"name": "fixture-market", "owner": {"name": "Fixture Owner"}, "plugins": plugins}
    repo.record("marketplace", {MARKET_FILE: json.dumps(document), "LICENSE": MIT_TEXT, **(files or {})})
    return repo


def _market(tmp_path: Path, ext_extra: dict[str, str] | None = None) -> GitRepo:
    ext = _external(tmp_path / "ext", ext_extra)
    repo = marketplace(tmp_path / "market", ext)
    # A unit is read at its own last revision, so the relicensing must touch it too.
    repo.record(
        "recognisable licence text",
        {"LICENSE": MIT_TEXT, "plugins/local/skills/local-skill/SKILL.md": skill_md("local-skill", "Relicensed.")},
        date="2026-02-01T00:00:00+00:00",
    )
    return repo


def test_each_plugin_publishes_its_members_and_one_bundle(
    grim_mirror, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    repo = _market(tmp_path)
    report = _sync(grim_mirror, _mirror_file(tmp_path, registry, unique_repo, repo.url))

    rows = {(i["plugin"], i["kind"], i["name"]): i for i in report["items"]}
    assert rows[("local", "skill", "local-skill")]["status"] == "published"
    assert rows[("ext", "skill", "ext-skill")]["status"] == "published"
    for plugin, member in (("local", "local-skill"), ("ext", "ext-skill")):
        bundle = rows[(plugin, "bundle", plugin)]
        assert bundle["status"] == "published", bundle
        assert bundle["members"] == [rows[(plugin, "skill", member)]["ref"]]
        assert f"/up/{plugin}/skills/{member}:" in bundle["members"][0]
    # The external plugin's units record the repository they come from.
    assert rows[("ext", "skill", "ext-skill")]["revision"] == GitRepo(tmp_path / "ext").head()
    assert rows[("local", "skill", "local-skill")]["revision"] == repo.head()


def test_unadmitted_members_are_listed_with_the_bundle_they_missed(
    grim_mirror, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    ext = _external(tmp_path / "ext", {"plugins/ext/hooks/hooks.json": json.dumps({"hooks": {}})})
    ext.write("plugins/ext/skills/linked/SKILL.md", skill_md("linked"))
    (ext.path / "plugins/ext/skills/linked/escape").symlink_to("/etc/passwd")
    ext.record("a skill carrying a symlink")
    repo = marketplace(tmp_path / "market", ext)
    repo.record(
        "recognisable licence text",
        {"LICENSE": MIT_TEXT, "plugins/local/skills/local-skill/SKILL.md": skill_md("local-skill", "Relicensed.")},
    )
    report = _sync(grim_mirror, _mirror_file(tmp_path, registry, unique_repo, repo.url))

    ext = [i for i in report["items"] if i["plugin"] == "ext"]
    bundle = next(i for i in ext if i["kind"] == "bundle")
    assert bundle["status"] == "published"
    assert len(bundle["members"]) == 1 and "ext-skill" in bundle["members"][0]
    broken = next(i for i in ext if i["name"] == "linked")
    hook = next(i for i in ext if i["kind"] == "hook")
    assert (broken["status"], broken["reason"], hook["status"]) == ("refused", "unsupported-entry", "skipped")
    assert broken["bundle"] == "ext"
    assert hook["bundle"] == "ext"


def test_unsupported_source_is_skipped_source_unsupported(
    grim_mirror, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    repo = _market_with(
        tmp_path / "market",
        [
            {"name": "fromnpm", "source": {"source": "npm", "package": "some-plugin"}},
            {"name": "ssh", "source": {"source": "url", "url": "git@example.invalid:o/r.git"}},
        ],
    )
    r = grim_mirror.run("--file", str(_mirror_file(tmp_path, registry, unique_repo, repo.url)), "--format", "json", "sync")

    assert r.returncode == 0
    rows = {i["plugin"]: i for i in json.loads(r.stdout)["items"]}
    for plugin in ("fromnpm", "ssh"):
        assert (rows[plugin]["status"], rows[plugin]["reason"]) == ("skipped", "source-unsupported")


def test_strict_false_with_a_manifest_and_entry_components_is_a_plugin_conflict(
    grim_mirror, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    repo = _market_with(
        tmp_path / "market",
        [
            {"name": "conf", "source": "./plugins/conf", "strict": False, "skills": ["./skills"]},
            {"name": "fine", "source": "./plugins/fine"},
        ],
        {
            "plugins/conf/.claude-plugin/plugin.json": json.dumps({"name": "conf"}),
            "plugins/conf/skills/c/SKILL.md": skill_md("c"),
            "plugins/fine/skills/f/SKILL.md": skill_md("f"),
        },
    )
    report = _sync(grim_mirror, _mirror_file(tmp_path, registry, unique_repo, repo.url))

    conf = [i for i in report["items"] if i["plugin"] == "conf"]
    assert len(conf) == 1, conf
    assert (conf[0]["status"], conf[0]["reason"]) == ("skipped", "plugin-conflict")
    assert not any(i["plugin"] == "conf" and i["status"] == "published" for i in report["items"])
    # A conflicting plugin does not stop its neighbours.
    assert any(i["plugin"] == "fine" and i["kind"] == "bundle" and i["status"] == "published" for i in report["items"])


def _members(repository_ref: str, registry: str) -> set[str]:
    """Member ids stored in a published bundle's layer."""
    repository, version = repository_ref.removeprefix(f"{registry}/").rsplit(":", 1)
    layer = fetch_manifest(repository, version)["layers"][0]["digest"]
    return {m["id"] for m in json.loads(fetch_blob(repository, layer))["members"]}


def test_registry_holds_each_member_with_its_own_origin(
    grim_mirror, grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    repo = _market(tmp_path)
    ext = GitRepo(tmp_path / "ext")
    report = _sync(grim_mirror, _mirror_file(tmp_path, registry, unique_repo, repo.url))
    rows = {(i["plugin"], i["kind"], i["name"]): i for i in report["items"]}
    runner = grim_at(tmp_path)

    local = runner.json("describe", rows[("local", "skill", "local-skill")]["ref"])
    assert local["annotations"]["com.grimoire.mirror.source"] == repo.url
    assert local["revision"] == repo.head()

    d = runner.json("describe", rows[("ext", "skill", "ext-skill")]["ref"])
    ann = d["annotations"]
    assert ann["com.grimoire.mirror.source"] == ext.url
    assert ann["org.opencontainers.image.revision"] == ext.head()
    assert ann["com.grimoire.mirror.path"] == "plugins/ext/skills/ext-skill"
    created = datetime.fromisoformat(ann["org.opencontainers.image.created"].replace("Z", "+00:00"))
    assert created == datetime.fromisoformat(EXT_DATE)
    assert created != datetime.fromisoformat(BASE_DATE)
    assert created.year == 2026


def test_plugin_bundle_layer_lists_exactly_its_member_refs(
    grim_mirror, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    repo = _market(tmp_path)
    report = _sync(grim_mirror, _mirror_file(tmp_path, registry, unique_repo, repo.url))
    rows = {(i["plugin"], i["kind"], i["name"]): i for i in report["items"]}

    for plugin, member in (("local", "local-skill"), ("ext", "ext-skill")):
        stored = _members(rows[(plugin, "bundle", plugin)]["ref"], registry)
        assert stored == {rows[(plugin, "skill", member)]["ref"]}


def test_grim_add_of_a_plugin_bundle_installs_its_members(
    grim_mirror, grim_at, project_dir: Path, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    repo = _market(tmp_path)
    report = _sync(grim_mirror, _mirror_file(tmp_path, registry, unique_repo, repo.url))
    bundle = next(i for i in report["items"] if i["kind"] == "bundle" and i["plugin"] == "local")

    write_config(project_dir)
    grim_at(project_dir).run("add", bundle["ref"])

    installed = project_dir / ".claude" / "skills" / "local-skill" / "SKILL.md"
    assert "Relicensed." in installed.read_text(encoding="utf-8")


def test_plugin_directory_without_manifest_is_licensed_by_its_own_file(
    grim_mirror, grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    repo = _market_with(
        tmp_path / "market",
        [{"name": "gpl", "source": "./plugins/gpl"}],
        {"plugins/gpl/LICENSE": GPL_TEXT, "plugins/gpl/skills/g/SKILL.md": skill_md("g")},
    )
    mf = _mirror_file(tmp_path, registry, unique_repo, repo.url, 'licenses = ["MIT"]\n')
    report = _sync(grim_mirror, mf)

    skill = next(i for i in report["items"] if i["kind"] == "skill" and i["name"] == "g")
    assert skill["status"] == "refused", skill
    assert skill["reason"] in ("license-unrecognised", "license-not-allowed"), skill
    repository = f"{unique_repo}/up/gpl/skills/g"
    with pytest.raises(HTTPError):
        list_tags(repository)
    assert grim_at(tmp_path).run("describe", f"{registry}/{repository}", check=False).returncode == 79


def test_unavailable_external_plugin_is_one_refused_row_and_its_neighbour_publishes(
    grim_mirror, grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    missing = (tmp_path / "no-such-repo").resolve().as_uri()
    repo = _market_with(
        tmp_path / "market",
        [
            {"name": "gone", "source": {"source": "git-subdir", "url": missing, "path": "plugins/gone"}},
            {"name": "fine", "source": "./plugins/fine"},
        ],
        {"plugins/fine/skills/f/SKILL.md": skill_md("f")},
    )
    report = _sync(grim_mirror, _mirror_file(tmp_path, registry, unique_repo, repo.url))

    assert report["upstreams"][0]["status"] == "ok", report["upstreams"]
    gone = [i for i in report["items"] if i["plugin"] == "gone"]
    assert len(gone) == 1, gone
    assert (gone[0]["kind"], gone[0]["name"], gone[0]["status"], gone[0]["reason"]) == (
        "bundle", "gone", "refused", "upstream-unavailable",
    )
    fine = next(i for i in report["items"] if i["plugin"] == "fine" and i["kind"] == "skill")
    assert fine["status"] == "published", fine
    assert grim_at(tmp_path).json("describe", fine["ref"])["kind"] == "skill"


def _unreadable_repo(path: Path) -> GitRepo:
    """A repository whose objects are all empty files: it exists, and no clone of it can complete."""
    repo = GitRepo.init(path)
    repo.record("content", {"plugins/broken/skills/b/SKILL.md": skill_md("b")})
    for obj in (path / ".git" / "objects").rglob("*"):
        if obj.is_file():
            obj.chmod(0o644)
            obj.write_bytes(b"")
    return repo


def test_unreadable_external_plugin_is_one_failed_row_and_sync_exits_infra(
    grim_mirror, grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    broken = _unreadable_repo(tmp_path / "broken")
    repo = _market_with(
        tmp_path / "market",
        [
            {"name": "broken", "source": {"source": "git-subdir", "url": broken.url, "path": "plugins/broken"}},
            {"name": "fine", "source": "./plugins/fine"},
        ],
        {"plugins/fine/skills/f/SKILL.md": skill_md("f")},
    )
    r = grim_mirror.run(
        "--file", str(_mirror_file(tmp_path, registry, unique_repo, repo.url)), "--format", "json", "sync", check=False
    )

    assert r.returncode == 69, r.stderr
    report = json.loads(r.stdout)
    assert report["upstreams"][0]["status"] == "ok", report["upstreams"]
    rows = [i for i in report["items"] if i["plugin"] == "broken"]
    assert [(i["kind"], i["name"], i["status"], i["reason"]) for i in rows] == [("bundle", "broken", "failed", None)]
    fine = next(i for i in report["items"] if i["plugin"] == "fine" and i["kind"] == "skill")
    assert fine["status"] == "published", fine
    assert grim_at(tmp_path).json("describe", fine["ref"])["kind"] == "skill"


def test_external_plugin_bundle_names_the_repository_its_revision_is_in(
    grim_mirror, grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    repo = _market(tmp_path)
    ext = GitRepo(tmp_path / "ext")
    report = _sync(grim_mirror, _mirror_file(tmp_path, registry, unique_repo, repo.url))
    rows = {(i["plugin"], i["kind"], i["name"]): i for i in report["items"]}

    ann = grim_at(tmp_path).json("describe", rows[("ext", "bundle", "ext")]["ref"])["annotations"]
    assert ann["com.grimoire.mirror.source"] == ext.url
    assert ann["org.opencontainers.image.revision"] == ext.head()
    assert ann["com.grimoire.mirror.path"] == "plugins/ext"
    local = grim_at(tmp_path).json("describe", rows[("local", "bundle", "local")]["ref"])["annotations"]
    assert local["com.grimoire.mirror.source"] == repo.url
    assert local["com.grimoire.mirror.path"] == "plugins/local"


def test_external_plugin_licence_notice_names_its_own_repository(
    grim_mirror, grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    ext = GitRepo.init(tmp_path / "ext")
    ext.record("plugin with a root licence", {"LICENSE": MIT_TEXT, "plugins/ext/agents/n.md": agent_md("n")})
    repo = _market_with(
        tmp_path / "market",
        [{"name": "ext", "source": {"source": "git-subdir", "url": ext.url, "path": "plugins/ext"}}],
    )
    report = _sync(grim_mirror, _mirror_file(tmp_path, registry, unique_repo, repo.url))
    agent = next(i for i in report["items"] if i["kind"] == "agent" and i["name"] == "n")
    assert agent["status"] == "published", agent

    out = tmp_path / "companion"
    grim_at(tmp_path).run("fetch", agent["ref"], "--description", "--out", str(out))
    readme = "\n".join(p.read_text(encoding="utf-8") for p in sorted(out.rglob("README*")))
    assert f"Mirrored from {ext.url} " in readme, sorted(p.name for p in out.rglob("*"))
    assert repo.url not in readme


def test_repository_root_plugin_is_licensed_by_its_plugin_manifest(
    grim_mirror, grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    ext = GitRepo.init(tmp_path / "ext")
    ext.record(
        "plugin at the repository root",
        {
            ".claude-plugin/plugin.json": json.dumps({"name": "rooted", "license": "MIT"}),
            "skills/r/SKILL.md": skill_md("r"),
        },
    )
    repo = _market_with(tmp_path / "market", [{"name": "rooted", "source": {"source": "url", "url": ext.url}}])
    report = _sync(grim_mirror, _mirror_file(tmp_path, registry, unique_repo, repo.url))

    skill = next(i for i in report["items"] if i["kind"] == "skill" and i["name"] == "r")
    assert skill["status"] == "published", skill
    ann = grim_at(tmp_path).json("describe", skill["ref"])["annotations"]
    assert ann["org.opencontainers.image.licenses"] == "MIT"
    assert ann["com.grimoire.mirror.source"] == ext.url


def test_repository_root_plugin_manifest_licence_never_outvotes_the_root_licence_file(
    grim_mirror, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    ext = GitRepo.init(tmp_path / "ext")
    ext.record(
        "plugin at the root whose manifest and licence file disagree",
        {
            ".claude-plugin/plugin.json": json.dumps({"name": "rooted", "license": "MIT"}),
            "LICENSE": GPL_TEXT,
            "skills/r/SKILL.md": skill_md("r"),
        },
    )
    repo = _market_with(tmp_path / "market", [{"name": "rooted", "source": {"source": "url", "url": ext.url}}])
    mf = _mirror_file(tmp_path, registry, unique_repo, repo.url, 'licenses = ["MIT"]\n')
    report = _sync(grim_mirror, mf)

    skill = next(i for i in report["items"] if i["kind"] == "skill" and i["name"] == "r")
    assert skill["status"] == "refused", skill
    with pytest.raises(HTTPError):
        list_tags(f"{unique_repo}/up/rooted/skills/r")
