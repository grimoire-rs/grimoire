# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""The disarm half of the experimental ``mod`` kind.

A mod's files and settings entries leave when the mod is gated (consent
revoked, flag off), pruned or no longer hosted, whatever ``--client`` selects
and whatever another row did. Every "removed" assertion sits beside a check, in
the same test, that the files existed before the disarming run.
"""

from __future__ import annotations

import json
import re
import shutil
from pathlib import Path

import pytest

from src.helpers import (
    MOD_CLIENTS_ANNOTATION,
    make_artifact,
    make_bundle,
    make_mod,
    mod_sandbox,
    tar_of,
    write_config,
)
from src.registry import push_artifact
from src.runner import GrimRunner
from src.stdio_drivers import drive_until_answered, initialize

SKILL = "---\nname: control\ndescription: d\n---\n# control\n"
OPENCODE_V2 = {"opencode": ['plugin_api = "v2"']}
BOTH = ("--client", "claude", "--client", "opencode")
SETTINGS = Path(".claude/settings.local.json")
MARKETPLACE = Path(".claude/mods/.claude-plugin/marketplace.json")
CLAUDE_TREE = Path(".claude/mods/status-band")
OPENCODE_TREE = Path(".opencode/plugins/status-band")


def _declare(project_dir: Path, repo: str, mods: dict, vendors=None, flag=True) -> None:
    skill = make_artifact(f"{repo}/control", "skill", {"control/SKILL.md": SKILL}, "1")
    write_config(
        project_dir,
        skills={"control": skill.fq},
        mods={name: m.fq for name, m in mods.items()},
        experimental_mods=flag,
        vendors=vendors,
    )


def _project(project_dir, tmp_path, repo, mods, grim_at, vendors=None):
    _declare(project_dir, repo, mods, vendors)
    runner = grim_at(project_dir)
    runner.env.update(mod_sandbox(tmp_path / "sandbox")[0])
    runner.run("lock")
    return runner


def _read_json(path: Path) -> dict:
    return json.loads(path.read_bytes().decode("utf-8"))


def _installed(project_dir: Path, both: bool = False) -> None:
    """Positive control: the files and entries a first install writes."""
    assert (project_dir / CLAUDE_TREE / "src" / "index.js").is_file()
    assert (project_dir / CLAUDE_TREE / "hooks" / "hooks.json").is_file()
    assert (project_dir / CLAUDE_TREE / ".claude-plugin" / "plugin.json").is_file()
    assert (project_dir / MARKETPLACE).is_file()
    assert "grim-mods-" in (project_dir / SETTINGS).read_text(encoding="utf-8")
    if both:
        assert (project_dir / OPENCODE_TREE / "index.ts").is_file()


def _gone(project_dir: Path, both: bool = False) -> None:
    assert not (project_dir / CLAUDE_TREE).exists()
    assert not (project_dir / MARKETPLACE).exists()
    text = (project_dir / SETTINGS).read_text(encoding="utf-8")
    assert "grim-mods-" not in text, text
    if both:
        assert not (project_dir / OPENCODE_TREE).exists()


def test_revoke_then_install_disarms(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at)
    runner.run("install", "--trust-workspace")
    _installed(project_dir)

    runner.run("consent", "revoke")
    runner.run("install", check=False)

    _gone(project_dir)
    assert (project_dir / ".claude" / "skills" / "control" / "SKILL.md").is_file(), (
        "the control skill of the same run is untouched"
    )


def test_flag_off_then_install_disarms(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at)
    runner.run("install", "--trust-workspace")
    _installed(project_dir)

    runner.run("config", "set", "options.experimental.mods", "false")
    runner.run("install", check=False)

    _gone(project_dir)


def test_edited_src_file_is_kept_with_mod_files_present(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at)
    runner.run("install", "--trust-workspace")
    _installed(project_dir)
    edited = project_dir / CLAUDE_TREE / "src" / "index.js"
    edited.write_bytes(b"// mine\n")

    runner.run("consent", "revoke")
    result = runner.run("install", check=False)

    assert edited.read_bytes() == b"// mine\n", "the edited file stays"
    assert not (project_dir / CLAUDE_TREE / "hooks" / "hooks.json").exists()
    assert not (project_dir / CLAUDE_TREE / ".claude-plugin" / "plugin.json").exists()
    assert "grim uninstall --force mod status-band" in result.stderr, result.stderr
    assert "grim-mods-" not in (project_dir / SETTINGS).read_text(encoding="utf-8")


def test_edited_index_ts_is_kept_and_warned(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    _declare(project_dir, unique_repo, {"status-band": mod}, vendors=OPENCODE_V2)
    runner = grim_at(project_dir)
    runner.env.update(mod_sandbox(tmp_path / "sandbox")[0])
    runner.run("lock")
    runner.run("install", "--trust-workspace", *BOTH)
    _installed(project_dir, both=True)
    entry = project_dir / OPENCODE_TREE / "index.ts"
    entry.write_bytes(b"// mine\n")

    runner.run("consent", "revoke")
    result = runner.run("install", *BOTH, check=False)

    assert entry.read_bytes() == b"// mine\n", "the edited entry stays"
    assert "keeps loading" in result.stderr, result.stderr
    assert "grim uninstall --force mod status-band" in result.stderr, result.stderr
    assert not (project_dir / CLAUDE_TREE).exists(), "Claude's tree went"


def test_disarm_while_another_row_fails_and_offline(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at)
    runner.run("install", "--trust-workspace")
    _installed(project_dir)

    # The mod leaves the declaration (pruned) while another mod's row fails.
    broken = make_mod(f"{unique_repo}/other-band", "1", clients="claude")
    _declare(project_dir, unique_repo, {"other-band": broken})
    runner.run("lock", check=False)
    runner.env["GRIM_OFFLINE"] = "1"
    result = runner.run("install", "--trust-workspace", check=False)

    assert result.returncode != 0, "positive control: the other row failed the run"
    assert not (project_dir / CLAUDE_TREE).exists(), "the pruned mod went offline"


def test_revoke_then_install_client_claude_disarms_opencode_too(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    _declare(project_dir, unique_repo, {"status-band": mod}, vendors=OPENCODE_V2)
    runner = grim_at(project_dir)
    runner.env.update(mod_sandbox(tmp_path / "sandbox")[0])
    runner.run("lock")
    runner.run("install", "--trust-workspace", *BOTH)
    _installed(project_dir, both=True)

    runner.run("consent", "revoke")
    runner.run("install", "--client", "claude", check=False)

    _gone(project_dir, both=True)


def test_moved_checkout_keeps_only_its_own_marketplace_key(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at)
    runner.run("install", "--trust-workspace")
    _installed(project_dir)
    settings = project_dir / SETTINGS
    value = _read_json(settings)
    (old,) = value["extraKnownMarketplaces"]
    value["extraKnownMarketplaces"]["mine"] = {"source": {"source": "directory"}}
    value["enabledPlugins"]["z@other"] = True
    settings.write_bytes((json.dumps(value, indent=2) + "\n").encode("utf-8"))

    moved = tmp_path / "moved"
    shutil.copytree(project_dir, moved, ignore=shutil.ignore_patterns(".git"))
    moved_runner = grim_at(moved)
    moved_runner.env.update(mod_sandbox(tmp_path / "sandbox")[0])
    # The copied marketplace file names the old marketplace, so the copy adopts it by force.
    moved_runner.run("install", "--trust-workspace", "--force")

    kept = _read_json(moved / SETTINGS)
    names = [k for k in kept["extraKnownMarketplaces"] if k != "mine"]
    assert len(names) == 1 and re.fullmatch(r"grim-mods-[0-9a-f]{12}", names[0])
    assert names[0] != old, "the copy minted its own marketplace name"
    assert "mine" in kept["extraKnownMarketplaces"]
    assert kept["enabledPlugins"]["z@other"] is True
    assert f"status-band@{old}" not in kept["enabledPlugins"]
    assert f"status-band@{names[0]}" in kept["enabledPlugins"]
    assert old in _read_json(settings)["extraKnownMarketplaces"], (
        "positive control: the original checkout's file is untouched"
    )


def _forget_hashes(grim_home: Path) -> int:
    """Corrupt every record's output hash in the mod store (not the shared files'); returns how many."""
    bogus = "sha256:" + "0" * 64
    changed = 0

    def walk(node) -> None:
        nonlocal changed
        if isinstance(node, dict):
            for key, value in node.items():
                if key == "content_hash":
                    node[key] = bogus
                    changed += 1
                else:
                    walk(value)
        elif isinstance(node, list):
            for item in node:
                walk(item)

    for record in (grim_home / "mods").glob("*.json"):
        data = _read_json(record)
        walk(data["records"])
        record.write_bytes(json.dumps(data).encode("utf-8"))
    return changed


def test_a_render_of_the_record_pin_proves_a_file_its_record_lost(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at)
    runner.run("install", "--trust-workspace")
    _installed(project_dir)
    assert _forget_hashes(Path(runner.env["GRIM_HOME"])) > 0, "the record held hashes"

    # The row leaves the config: only the record names the pin now.
    _declare(project_dir, unique_repo, {})
    runner.run("lock")
    runner.run("install", check=False)

    _gone(project_dir)


def _tree(name: str) -> Path:
    return Path(".claude/mods") / name


def _snapshot(project_dir: Path) -> dict[str, bytes]:
    """Every byte of the Claude settings file and the Claude and OpenCode mod trees."""
    files = {}
    for root in (project_dir / ".claude" / "mods", project_dir / ".opencode"):
        files.update({str(p): p.read_bytes() for p in root.rglob("*") if p.is_file()})
    files[str(project_dir / SETTINGS)] = (project_dir / SETTINGS).read_bytes()
    return files


def _marketplace_name(project_dir: Path) -> str:
    (name,) = _read_json(project_dir / SETTINGS)["extraKnownMarketplaces"]
    return name


def _two_mods(grim_at, project_dir: Path, tmp_path: Path, repo: str):
    mods = {n: make_mod(f"{repo}/{n}", "1") for n in ("status-band", "other-band")}
    runner = _project(project_dir, tmp_path, repo, mods, grim_at)
    runner.run("install", "--trust-workspace")
    runner.run(
        "consent", "allow"
    )  # the flag answers one run; the record answers the next
    for n in mods:
        assert (project_dir / _tree(n) / "src" / "index.js").is_file()
    return runner, mods


def test_why_names_the_declaring_path(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    direct = make_mod(f"{unique_repo}/status-band", "1")
    dep = make_mod(f"{unique_repo}/dep-band", "1")
    top = make_mod(
        f"{unique_repo}/top-band", "1", requires=[("mod", "dep-band", dep.fq)]
    )
    member = make_mod(f"{unique_repo}/member-band", "1")
    stack = make_bundle(
        f"{unique_repo}/stack", [("mod", "member-band", member.fq)], "1"
    )
    write_config(
        project_dir,
        mods={"status-band": direct.fq, "top-band": top.fq},
        bundles={"stack": stack.fq},
        experimental_mods=True,
    )
    runner = grim_at(project_dir)
    runner.env.update(mod_sandbox(tmp_path / "sandbox")[0])
    runner.run("lock")

    def chain(name: str) -> list[list[tuple[str, str]]]:
        doc = runner.json("why", "mod", name)
        return [[(n["kind"], n["name"]) for n in path] for path in doc["paths"]]

    assert chain("status-band") == [[("mod", "status-band")]]
    assert chain("dep-band") == [[("mod", "top-band"), ("mod", "dep-band")]]
    assert chain("member-band") == [[("bundle", "stack"), ("mod", "member-band")]]


def test_non_converging_commands_leave_mod_files_unchanged(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at)
    runner.run("install", "--trust-workspace")
    _installed(project_dir)
    runner.run("consent", "revoke")  # gated: a converging command would disarm now
    before = _snapshot(project_dir)

    for args in (
        ("status",),
        ("lock",),
        ("context",),
        ("consent", "list"),
        ("hook", "list"),
        ("export", "plugin", "-o", str(tmp_path / "plugin-out")),
        ("fetch", mod.fq),
        ("consent", "allow"),
        ("consent", "revoke"),
        ("remove", "mod", "status-band"),
    ):
        runner.run(*args, check=False)
        assert _snapshot(project_dir) == before, args

    # Offline: with no `[[registries]]` the browse fallback is the public index.
    search = drive_until_answered(
        runner,
        project_dir,
        [
            initialize(1),
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "grim_search",
                    "arguments": {"query": "status-band"},
                },
            },
        ],
        allow_writes=True,
    )[2]
    assert not search["result"]["isError"], search
    assert _snapshot(project_dir) == before, "grim_search changed mod files"

    calls = {
        "grim_status": {},
        "grim_status#check": {"check": True},
        "grim_fetch": {"ref": mod.fq},
        "grim_render": {
            "ref": mod.fq,
            "vendor": "claude",
            "dest_dir": str(tmp_path / "render-out"),
        },
    }
    requests: list[dict] = [
        initialize(1),
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
    ]
    requests += [
        {
            "jsonrpc": "2.0",
            "id": 2 + n,
            "method": "tools/call",
            "params": {"name": key.split("#")[0], "arguments": args},
        }
        for n, (key, args) in enumerate(calls.items())
    ]
    # Online, but only `grim_fetch` reaches a registry: the session test registry.
    responses = drive_until_answered(
        runner, project_dir, requests, allow_writes=True, offline=False
    )
    for n, key in enumerate(calls):
        # Render refuses a mod: the refusal proves the call ran and wrote nothing.
        want = "error" if key == "grim_render" else "result"
        assert want in responses[2 + n], (key, responses[2 + n])
    fetch = responses[2 + list(calls).index("grim_fetch")]
    assert not fetch["result"]["isError"], fetch
    assert not responses[2]["result"]["isError"], responses[2]  # control: tools ran
    assert _snapshot(project_dir) == before, "an MCP tool changed mod files"

    runner.run("install", check=False)
    _gone(project_dir)  # positive control: a converging command does disarm
    assert not list(tmp_path.rglob("known_marketplaces.json")), (
        "a mod command wrote Claude's own marketplace registry"
    )


def test_named_update_after_revoke_disarms_outside_the_closure(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at)
    runner.run("install", "--trust-workspace")
    _installed(project_dir)

    runner.run("consent", "revoke")
    result = runner.run("update", "control", check=False)

    _gone(project_dir)
    assert "status-band" in result.stderr, result.stderr


def test_add_prunes_only_the_previous_closure(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    runner, _ = _two_mods(grim_at, project_dir, tmp_path, unique_repo)
    runner.run("remove", "mod", "status-band")
    extra = make_artifact(
        f"{unique_repo}/extra",
        "skill",
        {"extra/SKILL.md": SKILL.replace("control", "extra")},
        "1",
    )

    added = runner.run("add", "--kind", "skill", extra.fq)
    assert "named closure" not in added.stderr, added.stderr
    assert (project_dir / _tree("status-band") / "src" / "index.js").is_file()
    assert (project_dir / _tree("other-band") / "src" / "index.js").is_file()

    final = runner.run("install")
    assert not (project_dir / _tree("status-band")).exists()
    assert (project_dir / _tree("other-band") / "src" / "index.js").is_file(), (
        final.stderr
    )


def test_uninstall_names_the_marketplace_remove_command(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    runner, _ = _two_mods(grim_at, project_dir, tmp_path, unique_repo)
    command = f"claude plugin marketplace remove {_marketplace_name(project_dir)}"

    first = runner.run("uninstall", "mod", "status-band")
    assert not (project_dir / _tree("status-band")).exists()
    assert "claude plugin marketplace remove" not in first.stderr, first.stderr

    last = runner.run("uninstall", "mod", "other-band")
    assert not (project_dir / _tree("other-band")).exists()
    assert command in last.stderr, last.stderr


def test_remove_leaves_files_until_the_next_install(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at)
    runner.run("install", "--trust-workspace")
    _installed(project_dir)

    result = runner.run("remove", "mod", "status-band")

    assert "status-band" not in (project_dir / "grimoire.toml").read_text(
        encoding="utf-8"
    )
    assert "grim uninstall mod status-band" in result.stderr, result.stderr
    _installed(project_dir)
    runner.run("install")
    _gone(project_dir)


def test_uninstall_removes_a_false_entry(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at)
    runner.run("install", "--trust-workspace")
    settings = project_dir / SETTINGS
    value = _read_json(settings)
    (key,) = value["enabledPlugins"]
    value["enabledPlugins"][key] = False
    settings.write_bytes((json.dumps(value, indent=2) + "\n").encode("utf-8"))
    assert _read_json(settings)["enabledPlugins"] == {key: False}

    runner.run("uninstall", "mod", "status-band")

    _gone(project_dir)


def test_uninstall_refuses_a_drifted_entry_and_force_removes(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at)
    runner.run("install", "--trust-workspace")
    settings = project_dir / SETTINGS
    value = _read_json(settings)
    (key,) = value["enabledPlugins"]
    value["enabledPlugins"][key] = "mine"
    settings.write_bytes((json.dumps(value, indent=2) + "\n").encode("utf-8"))
    drifted = settings.read_bytes()

    refused = runner.run("uninstall", "mod", "status-band", check=False)

    assert refused.returncode == 65, refused.stderr
    assert settings.read_bytes() == drifted
    assert (project_dir / _tree("status-band") / "src" / "index.js").is_file()

    runner.run("uninstall", "--force", "mod", "status-band")
    _gone(project_dir)


def test_remove_warns_of_a_dependency_mod_it_dropped(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """Removing a mod drops the dependency only it required. That dependency's
    files stay until the next install, as the removed mod's do, and stderr says
    so for both."""
    dep = make_mod(f"{unique_repo}/dep-band", "1")
    top = make_mod(
        f"{unique_repo}/top-band", "1", requires=[("mod", "dep-band", dep.fq)]
    )
    runner = _project(project_dir, tmp_path, unique_repo, {"top-band": top}, grim_at)
    runner.run("install", "--trust-workspace")
    assert (project_dir / _tree("dep-band") / "src" / "index.js").is_file(), (
        "positive control: the dependency was written"
    )

    result = runner.run("remove", "mod", "top-band")

    assert "grim uninstall mod top-band" in result.stderr, result.stderr
    assert (
        "mod 'dep-band' was undeclared but its files stay until `grim install` runs"
        in result.stderr
    ), result.stderr
    assert (project_dir / _tree("dep-band")).is_dir()
    runner.run("install")
    assert not (project_dir / _tree("dep-band")).exists()


@pytest.mark.parametrize("dropped", ["opencode", "claude"])
@pytest.mark.parametrize("scope", ["project", "global"])
def test_narrowed_clients_disarm_the_dropped_client_at_the_next_install(
    grim_at,
    grim_binary: Path,
    grim_home: Path,
    project_dir: Path,
    tmp_path: Path,
    unique_repo: str,
    scope: str,
    dropped: str,
) -> None:
    """A client taken out of `[options].clients` loses the mod's files at the
    next converging command; the client still listed keeps its own."""
    mod = make_mod(f"{unique_repo}/status-band", "1")
    kept = "claude" if dropped == "opencode" else "opencode"
    both = {"clients": '["claude", "opencode"]'}
    if scope == "project":
        write_config(
            project_dir,
            mods={"status-band": mod.fq},
            experimental_mods=True,
            vendors=OPENCODE_V2,
            options=both,
        )
        runner = grim_at(project_dir)
        runner.env.update(mod_sandbox(tmp_path / "sandbox")[0])
        flags: tuple[str, ...] = ()
        trees = {
            "claude": project_dir / CLAUDE_TREE,
            "opencode": project_dir / OPENCODE_TREE,
        }
    else:
        sandbox, roots = mod_sandbox(tmp_path / "global-sandbox")
        write_config(
            grim_home,
            mods={"status-band": mod.fq},
            experimental_mods=True,
            vendors=OPENCODE_V2,
            options=both,
        )
        runner = GrimRunner(grim_binary, grim_home, cwd=tmp_path)
        runner.env.update(sandbox)
        flags = ("--global",)
        trees = {
            "claude": roots["claude"] / "mods" / "status-band",
            "opencode": roots["opencode"] / "plugins" / "status-band",
        }
    runner.run(*flags, "lock")
    runner.run(*flags, "install", "--trust-workspace")
    assert trees["claude"].is_dir() and trees["opencode"].is_dir(), "positive control"

    # `--client` narrows the writes, never the disarm half.
    runner.run(*flags, "install", "--trust-workspace", "--client", kept)
    assert trees[dropped].is_dir(), f"--client {kept} disarmed {dropped}"

    runner.run(*flags, "config", "set", "options.clients", kept)
    runner.run(*flags, "install", "--trust-workspace")

    assert not trees[dropped].exists(), f"{dropped} keeps the mod it no longer serves"
    assert trees[kept].is_dir(), f"{kept} lost the mod it still serves"


def test_kept_edited_file_reads_gated_with_mod_files_present(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at)
    runner.run("install", "--trust-workspace")
    _installed(project_dir)
    edited = project_dir / CLAUDE_TREE / "src" / "index.js"
    edited.write_bytes(b"// mine\n")
    runner.run("consent", "revoke")
    runner.run("install", check=False)
    assert edited.read_bytes() == b"// mine\n", "positive control: the edit was kept"

    [row] = [i for i in runner.json("status")["items"] if i["name"] == "status-band"]

    assert row["state"] == "gated"
    assert ("claude", "mod-files-present") in [
        (a["client"], a["cause"]) for a in row["arming"]
    ]


def _scoped(scope, grim_at, grim_binary, grim_home, project_dir, tmp_path, mod):
    """Declare ``mod`` for Claude and OpenCode at ``scope``; return the runner,
    the scope flags, both client trees and the shared Claude files."""
    both = {"clients": '["claude", "opencode"]'}
    config_dir = project_dir if scope == "project" else grim_home
    write_config(
        config_dir,
        mods={"status-band": mod.fq},
        experimental_mods=True,
        vendors=OPENCODE_V2,
        options=both,
    )
    if scope == "project":
        runner = grim_at(project_dir)
        runner.env.update(mod_sandbox(tmp_path / "sandbox")[0])
        return (
            runner,
            (),
            {
                "claude": project_dir / CLAUDE_TREE,
                "opencode": project_dir / OPENCODE_TREE,
                "marketplace": project_dir / MARKETPLACE,
                "settings": project_dir / SETTINGS,
            },
        )
    sandbox, roots = mod_sandbox(tmp_path / "global-sandbox")
    runner = GrimRunner(grim_binary, grim_home, cwd=tmp_path)
    runner.env.update(sandbox)
    return (
        runner,
        ("--global",),
        {
            "claude": roots["claude"] / "mods" / "status-band",
            "opencode": roots["opencode"] / "plugins" / "status-band",
            "marketplace": roots["claude"]
            / "mods"
            / ".claude-plugin"
            / "marketplace.json",
            "settings": roots["claude"] / "settings.json",
        },
    )


# Global scope keeps no consent record, so revoke and `--no-trust-workspace`
# are triggers at project scope only.
@pytest.mark.parametrize(
    ("scope", "trigger"),
    [
        ("project", "revoke"),
        ("project", "no-trust"),
        ("project", "flag-off"),
        ("project", "removed"),
        ("project", "uninstall"),
        ("global", "flag-off"),
        ("global", "removed"),
        ("global", "uninstall"),
    ],
)
def test_every_disarm_trigger_clears_both_clients(
    grim_at,
    grim_binary: Path,
    grim_home: Path,
    project_dir: Path,
    tmp_path: Path,
    unique_repo: str,
    scope: str,
    trigger: str,
) -> None:
    """Each disarm trigger removes the mod's Claude and OpenCode files and its
    shared Claude entries at that scope, and no mod command writes Claude's own
    `known_marketplaces.json`."""
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner, flags, paths = _scoped(
        scope, grim_at, grim_binary, grim_home, project_dir, tmp_path, mod
    )
    runner.run(*flags, "lock")
    runner.run(*flags, "install", "--trust-workspace")
    assert paths["claude"].is_dir() and paths["opencode"].is_dir(), "positive control"
    assert paths["marketplace"].is_file()
    assert "status-band@grim-mods-" in paths["settings"].read_text(encoding="utf-8")

    if trigger == "revoke":
        runner.run("consent", "revoke")
        runner.run("install", check=False)
    elif trigger == "no-trust":
        runner.run("install", "--no-trust-workspace", check=False)
    elif trigger == "flag-off":
        runner.run(*flags, "config", "set", "options.experimental.mods", "false")
        runner.run(*flags, "install", check=False)
    elif trigger == "removed":
        runner.run(*flags, "remove", "mod", "status-band")
        runner.run(*flags, "install")
    else:
        runner.run(*flags, "uninstall", "mod", "status-band")

    assert not paths["claude"].exists(), "Claude keeps the disarmed mod"
    assert not paths["opencode"].exists(), "OpenCode keeps the disarmed mod"
    assert not paths["marketplace"].exists()
    text = paths["settings"].read_text(encoding="utf-8")
    assert "grim-mods-" not in text and "status-band" not in text, text
    assert not list(tmp_path.rglob("known_marketplaces.json"))


def test_a_client_the_mod_no_longer_lists_is_disarmed(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """A new version whose clients drop Claude takes the mod off Claude at the
    next install and keeps OpenCode's copy."""
    first = make_mod(f"{unique_repo}/status-band", "1")
    _declare(project_dir, unique_repo, {"status-band": first}, vendors=OPENCODE_V2)
    runner = grim_at(project_dir)
    runner.env.update(mod_sandbox(tmp_path / "sandbox")[0])
    runner.run("lock")
    runner.run("install", "--trust-workspace", *BOTH)
    _installed(project_dir, both=True)

    manifest = (
        'schema = 1\nname = "status-band"\ndescription = "d"\n\n'
        '[opencode]\nentry = "index.ts"\n'
    )
    layer = tar_of(
        {
            "status-band/mod.toml": manifest,
            "status-band/opencode/index.ts": "export default {};\n",
        }
    )
    second = push_artifact(
        f"{unique_repo}/status-band",
        "2",
        layer,
        "mod",
        {MOD_CLIENTS_ANNOTATION: "opencode"},
    )
    _declare(project_dir, unique_repo, {"status-band": second}, vendors=OPENCODE_V2)
    runner.run("lock")
    runner.run("install", "--trust-workspace", *BOTH)

    assert not (project_dir / CLAUDE_TREE).exists(), "Claude no longer hosts the mod"
    assert not (project_dir / MARKETPLACE).exists()
    text = (project_dir / SETTINGS).read_text(encoding="utf-8")
    assert "grim-mods-" not in text, text
    assert (project_dir / OPENCODE_TREE / "index.ts").is_file(), (
        "OpenCode still hosts it"
    )


def test_offline_install_of_an_installed_mod_changes_nothing(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at)
    runner.run("install", "--trust-workspace")
    runner.run("consent", "allow")
    _installed(project_dir)
    store = sorted((Path(runner.env["GRIM_HOME"]) / "mods").glob("*.json"))
    assert store, "positive control: the mod store holds the record"
    before = (_snapshot(project_dir), [p.read_bytes() for p in store])

    runner.env["GRIM_OFFLINE"] = "1"
    result = runner.run("install", check=False)

    assert result.returncode == 0, result.stderr
    assert (_snapshot(project_dir), [p.read_bytes() for p in store]) == before
