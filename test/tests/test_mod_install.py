# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Installing a ``mod``: what the install pass refuses and what it writes.

Each refusal sits beside a skill declared in the same run, which is written:
the positive control that the run reached its writers at all and that the
mod's refusal did not stop the other rows. A "nothing was written" assertion
without that control would pass against a binary that never reached the mod
phase.
"""

from __future__ import annotations

import io
import json
import re
import sys
import tarfile
from pathlib import Path

import pytest

from src.helpers import make_artifact, make_mod, make_raw_mod, mod_sandbox, write_config
from src.registry import LAYER_MEDIA_TYPE
from src.runner import GrimRunner

SKILL = "---\nname: control\ndescription: d\n---\n# control\n"
MANIFEST = (
    'schema = 1\nname = "hostile"\ndescription = "d"\n\n[claude]\nentry = "{entry}"\n'
)

no_links = pytest.mark.skipif(
    sys.platform == "win32",
    reason="symbolic links need a privilege Windows runners lack",
)


def _tar(entries: list[tuple[str, bytes | tuple[str, str]]]) -> bytes:
    """A tar over ``(path, bytes)`` or ``(path, (link-kind, target))`` entries."""
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as tar:
        for path, content in entries:
            info = tarfile.TarInfo(name=path)
            info.mode = 0o644
            if isinstance(content, tuple):
                info.type = tarfile.SYMTYPE
                info.linkname = content[1]
                tar.addfile(info)
                continue
            info.size = len(content)
            tar.addfile(info, io.BytesIO(content))
    return buf.getvalue()


def _base(entry: str = "index.js") -> list[tuple[str, bytes | tuple[str, str]]]:
    return [
        ("hostile/mod.toml", MANIFEST.format(entry=entry).encode()),
        ("hostile/claude/index.js", b"export default {};\n"),
    ]


def _without_manifest() -> list[tuple[str, bytes | tuple[str, str]]]:
    return _base()[1:]


# class -> (tar entries, clients annotation, the rule the validator names)
HOSTILE: dict[str, tuple[list, str, str]] = {
    "entry-type": (
        _base() + [("hostile/claude/link.js", ("symlink", "index.js"))],
        "claude",
        "entry-type",
    ),
    "device-name": (
        _base() + [("hostile/claude/nul.js", b"1\n")],
        "claude",
        "segment",
    ),
    "backslash": (_base() + [("hostile/claude/a\\b.js", b"1\n")], "claude", "path"),
    "escape": (_base() + [("hostile/claude/../../x.js", b"1\n")], "claude", "path"),
    "case-collision": (
        _base() + [("hostile/claude/Index.js", b"1\n")],
        "claude",
        "duplicate",
    ),
    "extension": (
        _base() + [("hostile/claude/data.json", b"{}\n")],
        "claude",
        "extension",
    ),
    "no-manifest": (_without_manifest(), "claude", "manifest"),
    "two-roots": (_base() + [("other/x.js", b"1\n")], "claude", "layout"),
    "clients-mismatch": (_base(), "claude,opencode", "clients"),
    "entry-missing": (_base("gone.js"), "claude", "entry"),
}


def _project(
    project_dir: Path, tmp_path: Path, repo: str, mods: dict, grim_at, vendors=None
):
    """A flag-on project declaring ``mods`` and one control skill; returns the runner."""
    skill = make_artifact(f"{repo}/control", "skill", {"control/SKILL.md": SKILL}, "1")
    write_config(
        project_dir,
        skills={"control": skill.fq},
        mods={name: m.fq for name, m in mods.items()},
        experimental_mods=True,
        vendors=vendors,
    )
    runner = grim_at(project_dir)
    runner.env.update(mod_sandbox(tmp_path / "sandbox")[0])
    runner.run("lock")
    return runner


def _error(result) -> dict:
    return json.loads(result.stdout)["error"]


@pytest.mark.parametrize("hostile", sorted(HOSTILE))
def test_hostile_layer_refuses_per_class(
    hostile: str,
    grim_at,
    project_dir: Path,
    tmp_path: Path,
    unique_repo: str,
) -> None:
    entries, clients, rule = HOSTILE[hostile]
    mod = make_raw_mod(
        f"{unique_repo}/hostile",
        "1",
        [(LAYER_MEDIA_TYPE, _tar(entries))],
        clients=clients,
    )
    runner = _project(project_dir, tmp_path, unique_repo, {"hostile": mod}, grim_at)

    result = runner.run("install", "--trust-workspace", format="json", check=False)

    assert result.returncode == 65, result.stderr
    assert _error(result)["reason"] == "invalid-mod"
    assert f"invalid mod: {rule}:" in result.stderr, result.stderr
    assert not (project_dir / ".claude" / "mods").exists()
    assert (project_dir / ".claude" / "skills" / "control" / "SKILL.md").is_file(), (
        "positive control: the skill declared beside the refused mod is written"
    )


def test_hand_edited_lock_clients_refuse_at_install(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    good = make_mod(f"{unique_repo}/good", "1")
    edited = make_mod(f"{unique_repo}/edited", "1")
    runner = _project(
        project_dir, tmp_path, unique_repo, {"good": good, "edited": edited}, grim_at
    )
    lock = project_dir / "grimoire.lock"
    text = lock.read_bytes().decode("utf-8")
    block = re.search(r'\[\[mod\]\]\nname = "edited".*?(?=\n\[\[|\Z)', text, re.DOTALL)
    assert block, text
    narrowed = re.sub(r"clients = \[[^\]]*\]", 'clients = ["claude"]', block.group(0))
    assert narrowed != block.group(0), "the edit must change the row"
    lock.write_bytes(text.replace(block.group(0), narrowed).encode("utf-8"))

    result = runner.run("install", "--trust-workspace", format="json", check=False)

    assert result.returncode == 65, result.stderr
    assert _error(result)["reason"] == "invalid-mod"
    assert not (project_dir / ".claude" / "mods" / "edited").exists()
    assert (project_dir / ".claude" / "mods" / "good" / "src" / "index.js").is_file(), (
        "positive control: the untouched mod row converged in the same run"
    )
    assert (project_dir / ".claude" / "skills" / "control" / "SKILL.md").is_file()


@no_links
def test_symlinked_mods_dir_refuses_anchor_escape(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at)
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    link = project_dir / ".claude" / "mods"
    link.symlink_to(elsewhere, target_is_directory=True)

    result = runner.run("install", "--trust-workspace", format="json", check=False)

    assert result.returncode == 65, result.stderr
    assert _error(result)["reason"] == "anchor-escape"
    assert list(elsewhere.iterdir()) == [], "nothing was written through the link"
    assert (project_dir / ".claude" / "skills" / "control" / "SKILL.md").is_file()

    # Positive control: without the link the same run writes the plugin tree.
    link.unlink()
    runner.run("install", "--trust-workspace")
    plugin = project_dir / ".claude" / "mods" / "status-band"
    assert (plugin / "src" / "index.js").is_file()
    assert (plugin / ".claude-plugin" / "plugin.json").is_file()
    assert (plugin / "hooks" / "hooks.json").is_file()


MARKETPLACE = Path(".claude/mods/.claude-plugin/marketplace.json")
SETTINGS = Path(".claude/settings.local.json")


def _read_json(path: Path) -> dict:
    return json.loads(path.read_bytes().decode("utf-8"))


def _write_json(path: Path, value: dict) -> None:
    path.write_bytes((json.dumps(value, indent=2) + "\n").encode("utf-8"))


def _marketplace_name(project_dir: Path) -> str:
    (name,) = _read_json(project_dir / SETTINGS)["extraKnownMarketplaces"]
    assert re.fullmatch(r"grim-mods-[0-9a-f]{12}", name), name
    return name


def test_hand_edited_marketplace_refuses_and_force_restores(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at)
    runner.run("install", "--trust-workspace")
    market = project_dir / MARKETPLACE
    rendered = market.read_bytes()
    assert _read_json(market)["plugins"] == [
        {"name": "status-band", "source": "./status-band"}
    ], "positive control: the first install writes the marketplace file"

    market.write_bytes(b'{"hand": "edited"}\n')
    result = runner.run("install", "--trust-workspace", format="json", check=False)

    assert result.returncode == 65, result.stderr
    assert _error(result)["reason"] == "untracked-destination"
    assert market.read_bytes() == b'{"hand": "edited"}\n', (
        "the edit is never overwritten"
    )

    runner.run("install", "--trust-workspace", "--force")
    assert market.read_bytes() == rendered, "--force restores the render"
    runner.run("install", "--trust-workspace")
    assert market.read_bytes() == rendered, (
        "positive control: a clean re-run stays quiet"
    )


def test_foreign_marketplace_entry_refuses(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at)
    runner.run("install", "--trust-workspace")
    settings = project_dir / SETTINGS
    name = _marketplace_name(project_dir)
    ours = _read_json(settings)["extraKnownMarketplaces"][name]
    assert ours["source"] == {
        "path": str(project_dir / ".claude" / "mods"),
        "source": "directory",
    }

    data = _read_json(settings)
    foreign = {"source": {"path": "/somewhere/else", "source": "directory"}}
    data["extraKnownMarketplaces"][name] = foreign
    data["theme"] = "dark"
    _write_json(settings, data)
    before = settings.read_bytes()

    result = runner.run("install", "--trust-workspace", format="json", check=False)

    assert result.returncode == 65, result.stderr
    assert _error(result)["reason"] == "untracked-destination"
    assert settings.read_bytes() == before, (
        "the foreign entry and every other byte stay"
    )

    runner.run("install", "--trust-workspace", "--force")
    after = _read_json(settings)
    assert after["extraKnownMarketplaces"][name] == ours, "--force writes grim's value"
    assert after["theme"] == "dark", "members grim does not own are kept"


def test_enabled_plugins_value_table(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    runner = _project(project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at)
    runner.run("install", "--trust-workspace")
    settings = project_dir / SETTINGS
    key = f"status-band@{_marketplace_name(project_dir)}"
    plugin_src = project_dir / ".claude" / "mods" / "status-band" / "src" / "index.js"
    assert _read_json(settings)["enabledPlugins"][key] is True, (
        "absent -> true is written"
    )

    def set_value(value) -> None:
        data = _read_json(settings)
        if value is None:
            del data["enabledPlugins"][key]
        else:
            data["enabledPlugins"][key] = value
        _write_json(settings, data)

    # `true` is kept: nothing is rewritten.
    before = settings.read_bytes()
    runner.run("install", "--trust-workspace")
    assert settings.read_bytes() == before

    # Removed: the next run writes `true` again.
    set_value(None)
    runner.run("install", "--trust-workspace")
    assert _read_json(settings)["enabledPlugins"][key] is True

    # `false` is kept, the mod's files are still written, the cause is named.
    set_value(False)
    plugin_src.unlink()
    result = runner.run("install", "--trust-workspace", check=False)
    assert result.returncode == 0, result.stderr
    assert _read_json(settings)["enabledPlugins"][key] is False
    assert plugin_src.is_file(), (
        "the files are written even while the client has it off"
    )
    assert "mod-disabled-in-client" in result.stderr, result.stderr
    runner.run("install", "--trust-workspace", "--force")
    assert _read_json(settings)["enabledPlugins"][key] is True, "--force writes true"

    # Any other value refuses; --force replaces it.
    set_value("yes")
    result = runner.run("install", "--trust-workspace", format="json", check=False)
    assert result.returncode == 65, result.stderr
    assert _error(result)["reason"] == "untracked-destination"
    assert _read_json(settings)["enabledPlugins"][key] == "yes"
    runner.run("install", "--trust-workspace", "--force")
    assert _read_json(settings)["enabledPlugins"][key] is True


def test_two_mods_named_update_of_one_and_failure_of_the_other_keep_the_marketplace(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    first = make_mod(f"{unique_repo}/first-mod", "1")
    second = make_mod(f"{unique_repo}/second-mod", "1")
    runner = _project(
        project_dir,
        tmp_path,
        unique_repo,
        {"first-mod": first, "second-mod": second},
        grim_at,
    )
    runner.run("install", "--trust-workspace")
    market = project_dir / MARKETPLACE
    both = [
        {"name": "first-mod", "source": "./first-mod"},
        {"name": "second-mod", "source": "./second-mod"},
    ]
    assert _read_json(market)["plugins"] == both, "positive control: both are listed"
    rendered = market.read_bytes()

    # The second mod's file is hand-edited, so its row fails.
    (project_dir / ".claude" / "mods" / "second-mod" / "src" / "index.js").write_bytes(
        b"x\n"
    )
    result = runner.run("install", "--trust-workspace", format="json", check=False)
    assert result.returncode == 65, result.stderr
    assert _error(result)["reason"] == "modified"
    assert market.read_bytes() == rendered, "a failed row keeps the other mod listed"

    # A named update of the first mod leaves the second listed.
    runner.run("update", "first-mod", "--trust-workspace", check=False)
    assert _read_json(market)["plugins"] == both
    key = f"second-mod@{_marketplace_name(project_dir)}"
    assert _read_json(project_dir / SETTINGS)["enabledPlugins"][key] is True


# --- both clients: goldens, re-runs, locations, roots --------------------------

GOLDENS = (
    Path(__file__).resolve().parents[2] / "src" / "install" / "testdata" / "mod_render"
)
OPENCODE_V2 = {"opencode": ['plugin_api = "v2"']}
BOTH = ("--client", "claude", "--client", "opencode")
GOLDEN_TOML = (
    'schema = 1\nname = "status-band"\ndescription = "d"\n\n'
    '[claude]\nentry = "register.tsx"\n\n[opencode]\nentry = "status-band.ts"\n'
)
ENTRY_SRC = "export default {};\n"


def _golden_mod(repo: str):
    """A mod whose render inputs equal the renderer goldens' (entries and name)."""
    return make_mod(
        f"{repo}/status-band",
        "1",
        {"claude/register.tsx": ENTRY_SRC, "opencode/status-band.ts": ENTRY_SRC},
        mod_toml=GOLDEN_TOML,
    )


def _project_both(project_dir, tmp_path, repo, mods, grim_at):
    return _project(project_dir, tmp_path, repo, mods, grim_at, vendors=OPENCODE_V2)


def _tree(root: Path) -> dict[str, tuple[int, bytes]]:
    return {
        str(p.relative_to(root)): (p.stat().st_mtime_ns, p.read_bytes())
        for p in sorted(root.rglob("*"))
        if p.is_file()
    }


def test_install_both_clients_matches_goldens(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = _golden_mod(unique_repo)
    runner = _project_both(
        project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at
    )

    runner.run("install", "--trust-workspace", *BOTH)

    plugin = project_dir / ".claude" / "mods" / "status-band"
    assert (plugin / ".claude-plugin" / "plugin.json").read_bytes() == (
        GOLDENS / "plugin_plain.json"
    ).read_bytes()
    assert (plugin / "hooks" / "hooks.json").read_bytes() == (
        GOLDENS / "hooks.json"
    ).read_bytes()
    # The golden's marketplace name is a fixed sample; the run's is derived, so the
    # expected bytes are the golden with that one value swapped.
    expected = (
        (GOLDENS / "marketplace.json")
        .read_bytes()
        .replace(b"grim-mods-3fa9c2d1e0b7", _marketplace_name(project_dir).encode())
    )
    assert (project_dir / MARKETPLACE).read_bytes() == expected
    assert (
        project_dir / ".opencode" / "plugins" / "status-band" / "index.ts"
    ).read_bytes() == (GOLDENS / "index.ts").read_bytes()


def test_rerun_writes_nothing(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = _golden_mod(unique_repo)
    runner = _project_both(
        project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at
    )
    runner.run("install", "--trust-workspace", *BOTH)
    watched = [project_dir / ".claude", project_dir / ".opencode"]
    before = [_tree(root) for root in watched]
    assert all(before), "positive control: the first run wrote both clients"

    again = runner.run("install", "--trust-workspace", *BOTH, format="json")

    assert [_tree(root) for root in watched] == before, "mtime and bytes unchanged"
    items = json.loads(again.stdout)["items"]
    assert not [
        i
        for i in items
        if i.get("name") == "status-band" and "installed" in json.dumps(i)
    ], items


def _expected_paths(claude: Path, opencode: Path, project: bool) -> list[Path]:
    plugin = claude / "mods" / "status-band"
    files = [
        plugin / "src" / "register.tsx",
        plugin / ".claude-plugin" / "plugin.json",
        plugin / "hooks" / "hooks.json",
        claude / "mods" / ".claude-plugin" / "marketplace.json",
        opencode / "plugins" / "status-band" / "index.ts",
        opencode / "plugins" / "status-band" / "src" / "status-band.ts",
    ]
    return files


def test_project_and_global_locations(
    grim_at,
    grim_binary: Path,
    grim_home: Path,
    project_dir: Path,
    tmp_path: Path,
    unique_repo: str,
) -> None:
    mod = _golden_mod(unique_repo)
    runner = _project_both(
        project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at
    )
    runner.run("install", "--trust-workspace", *BOTH)
    for path in _expected_paths(
        project_dir / ".claude", project_dir / ".opencode", True
    ):
        assert path.is_file(), path
    assert (project_dir / SETTINGS).is_file()

    # Global scope, scratch roots outside the repository the command runs in.
    sandbox, roots = mod_sandbox(tmp_path / "global-sandbox")
    write_config(
        grim_home,
        mods={"status-band": mod.fq},
        experimental_mods=True,
        vendors=OPENCODE_V2,
        options={"clients": '["claude", "opencode"]'},
    )
    glob = GrimRunner(grim_binary, grim_home, cwd=tmp_path)
    glob.env.update(sandbox)
    glob.run("--global", "lock")
    glob.run("--global", "install", "--trust-workspace")
    for path in _expected_paths(roots["claude"], roots["opencode"], False):
        assert path.is_file(), path
    settings = _read_json(roots["claude"] / "settings.json")
    assert any(
        k.startswith("status-band@grim-mods-") for k in settings["enabledPlugins"]
    )


def test_untracked_opencode_dir_refuses_and_force_adopts(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = _golden_mod(unique_repo)
    runner = _project_both(
        project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at
    )
    index = project_dir / ".opencode" / "plugins" / "status-band" / "index.ts"
    index.parent.mkdir(parents=True)
    index.write_bytes(b"// mine\n")

    result = runner.run(
        "install", "--trust-workspace", *BOTH, format="json", check=False
    )

    assert result.returncode == 65, result.stderr
    assert _error(result)["reason"] == "untracked-destination"
    assert index.read_bytes() == b"// mine\n", "the file is kept"

    # --force replaces a file that differs from the render ...
    runner.run("install", "--trust-workspace", *BOTH, "--force")
    assert index.read_bytes() == (GOLDENS / "index.ts").read_bytes()
    # ... and adopts one that equals it: nothing is rewritten.
    index.unlink()
    index.write_bytes((GOLDENS / "index.ts").read_bytes())
    stamp = index.stat().st_mtime_ns
    runner.run("install", "--trust-workspace", *BOTH, "--force")
    assert index.read_bytes() == (GOLDENS / "index.ts").read_bytes()
    assert index.stat().st_mtime_ns == stamp, "an equal file is adopted, not rewritten"


def _git_repo(path: Path) -> Path:
    import os
    import subprocess

    path.mkdir(parents=True, exist_ok=True)
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env |= {"HOME": str(path.parent), "GIT_CONFIG_NOSYSTEM": "1"}
    subprocess.run(
        ["git", "init", "-q"],
        cwd=path,
        env=env,
        check=True,
        capture_output=True,
        timeout=60,
    )
    return path


@pytest.mark.parametrize(
    ("variable", "client", "where"),
    [
        ("CLAUDE_CONFIG_DIR", "claude", "mods"),
        ("OPENCODE_CONFIG_DIR", "opencode", "plugins"),
    ],
)
def test_global_roots_outside_and_inside_the_repository(
    variable: str,
    client: str,
    where: str,
    grim_binary: Path,
    grim_home: Path,
    tmp_path: Path,
    unique_repo: str,
) -> None:
    repo = _git_repo(tmp_path / "repo")
    mod = _golden_mod(unique_repo)
    write_config(
        grim_home,
        mods={"status-band": mod.fq},
        experimental_mods=True,
        vendors=OPENCODE_V2,
        options={"clients": json.dumps([client])},
    )
    sandbox, roots = mod_sandbox(tmp_path / "sandbox")
    runner = GrimRunner(grim_binary, grim_home, cwd=repo)
    runner.env.update(sandbox)
    runner.run("--global", "lock")

    # A relative root and one inside the repository: exit 0, one warning, no mod file.
    inside = repo / "cfg"
    inside.mkdir()
    for value, where_files in (("rel-root", repo / "rel-root"), (str(inside), inside)):
        runner.env[variable] = value
        result = runner.run("--global", "install", "--trust-workspace", check=False)
        assert result.returncode == 0, result.stderr
        assert result.stderr.count("mods left unchanged") == 1, result.stderr
        assert not (where_files / where).exists(), where_files
        # status reads the fence: not-armed, with the cause naming why.
        cause = (
            "settings-root-relative"
            if value == "rel-root"
            else "settings-root-in-workspace"
        )
        [row] = runner.json("--global", "status")["items"]
        assert row["name"] == "status-band"
        assert row["state"] == "not-armed", row
        assert [(a["client"], a["cause"]) for a in row["arming"]] == [(client, cause)]
        plain = runner.plain("--global", "status")
        assert "status-band" in plain.stdout and "not-armed" in plain.stdout, (
            plain.stdout
        )
    assert not (repo / "rel-root").exists()

    # Positive control: an absolute root outside the repository gets the files.
    runner.env[variable] = str(roots[client])
    runner.run("--global", "install", "--trust-workspace")
    assert (roots[client] / where / "status-band").is_dir()
    [row] = runner.json("--global", "status")["items"]
    assert row["state"] == "installed", row
    assert row["arming"] == []


# --- hosting: which clients a row reaches ---------------------------------------


def _claude_only(repo: str, name: str = "only"):
    """A mod that serves Claude alone, by manifest and annotation."""
    manifest = f'schema = 1\nname = "{name}"\ndescription = "d"\n\n[claude]\nentry = "index.js"\n'
    return make_raw_mod(
        f"{repo}/{name}",
        "1",
        [
            (
                LAYER_MEDIA_TYPE,
                _tar(
                    [
                        (f"{name}/mod.toml", manifest.encode()),
                        (f"{name}/claude/index.js", b"export default {};\n"),
                    ]
                ),
            )
        ],
        clients="claude",
    )


def test_opencode_without_plugin_api_declines_with_one_warning(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = _golden_mod(unique_repo)
    runner = _project(project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at)

    result = runner.run("install", "--trust-workspace", *BOTH)

    lines = [l for l in result.stderr.splitlines() if "not installed for opencode" in l]
    assert len(lines) == 1, result.stderr
    assert "plugin_api" in lines[0], lines
    assert not (project_dir / ".opencode" / "plugins").exists()
    assert (project_dir / ".claude" / "mods" / "status-band" / "src").is_dir(), (
        "positive control: Claude is hosted in the same run"
    )

    # Positive control: declaring the key lets the next run write OpenCode's files.
    runner = _project_both(
        project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at
    )
    runner.run("install", "--trust-workspace", *BOTH)
    assert (
        project_dir / ".opencode" / "plugins" / "status-band" / "index.ts"
    ).is_file()


def test_only_claude_present_declines_opencode(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = _golden_mod(unique_repo)
    runner = _project_both(
        project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at
    )

    runner.run("install", "--trust-workspace", "--client", "claude")

    assert (project_dir / ".claude" / "mods" / "status-band" / "src").is_dir()
    assert not (project_dir / ".opencode" / "plugins").exists(), (
        "OpenCode was not a target"
    )
    runner.run("install", "--trust-workspace", *BOTH)
    assert (
        project_dir / ".opencode" / "plugins" / "status-band" / "index.ts"
    ).is_file(), "positive control: selecting OpenCode writes it"


def test_row_not_serving_a_client_writes_nothing_there(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    only = _claude_only(unique_repo)
    both = make_mod(f"{unique_repo}/both", "1")
    runner = _project_both(
        project_dir, tmp_path, unique_repo, {"only": only, "both": both}, grim_at
    )

    result = runner.run("install", "--trust-workspace", *BOTH)

    plugins = project_dir / ".opencode" / "plugins"
    assert not (plugins / "only").exists()
    assert "mod 'only' not installed" not in result.stderr, (
        "not serving is by design: no warning"
    )
    assert (project_dir / ".claude" / "mods" / "only" / "src").is_dir()
    assert (plugins / "both" / "index.ts").is_file(), (
        "positive control: the row that serves it"
    )


def test_unknown_harness_directory_installs_known_clients(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    manifest = GOLDEN_TOML + '\n[amp]\nentry = "a.ts"\n'
    mod = make_raw_mod(
        f"{unique_repo}/status-band",
        "1",
        [
            (
                LAYER_MEDIA_TYPE,
                _tar(
                    [
                        ("status-band/mod.toml", manifest.encode()),
                        ("status-band/claude/register.tsx", ENTRY_SRC.encode()),
                        ("status-band/opencode/status-band.ts", ENTRY_SRC.encode()),
                        ("status-band/amp/anything.bin", b"\x00\x01"),
                    ]
                ),
            )
        ],
        clients="claude,opencode,amp",
    )
    runner = _project_both(
        project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at
    )

    result = runner.run("install", "--trust-workspace", *BOTH)

    assert (
        project_dir / ".claude" / "mods" / "status-band" / "src" / "register.tsx"
    ).is_file()
    assert (
        project_dir / ".opencode" / "plugins" / "status-band" / "index.ts"
    ).is_file()
    assert not list(project_dir.rglob("anything.bin")), "nothing is written for amp"
    warnings = [l for l in result.stderr.splitlines() if "amp" in l]
    assert len(warnings) == 1, result.stderr


def test_hand_edited_path_source_in_the_lock_is_never_written(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    edited = _golden_mod(unique_repo)
    good = make_mod(f"{unique_repo}/good", "1")
    runner = _project_both(
        project_dir,
        tmp_path,
        unique_repo,
        {"status-band": edited, "good": good},
        grim_at,
    )
    lock = project_dir / "grimoire.lock"
    text = lock.read_bytes().decode("utf-8")
    block = re.search(
        r'\[\[mod\]\]\nname = "status-band".*?(?=\n\[\[|\Z)', text, re.DOTALL
    )
    assert block, text
    swapped = re.sub(
        r'pinned = "[^"]*"',
        'path = "./dev-mod"\nhash = "sha256:' + "0" * 64 + '"',
        block.group(0),
    )
    assert swapped != block.group(0), "the edit must change the row"
    lock.write_bytes(text.replace(block.group(0), swapped).encode("utf-8"))

    result = runner.run("install", "--trust-workspace", *BOTH, check=False)

    assert result.returncode == 0, result.stderr
    [row] = [i for i in runner.json("status")["items"] if i["name"] == "status-band"]
    assert row["outputs"] == [], "no output is recorded for a row with no pinned source"
    assert not (project_dir / ".claude" / "mods" / "status-band").exists()
    assert not (project_dir / ".opencode" / "plugins" / "status-band").exists()
    assert (project_dir / ".claude" / "mods" / "good" / "src").is_dir(), (
        "positive control: the pinned row converged in the same run"
    )


# --- state files never hold a mod record -----------------------------------------


def _record_kinds(state_file: Path) -> list[str]:
    """Every ``kind`` value in a state file, wherever the records sit."""
    kinds: list[str] = []

    def walk(node: object) -> None:
        if isinstance(node, dict):
            if isinstance(node.get("kind"), str):
                kinds.append(node["kind"])
            for value in node.values():
                walk(value)
        elif isinstance(node, list):
            for value in node:
                walk(value)

    walk(json.loads(state_file.read_text(encoding="utf-8")))
    return kinds


def test_no_state_file_holds_a_mod_record(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = _golden_mod(unique_repo)
    runner = _project(project_dir, tmp_path, unique_repo, {"status-band": mod}, grim_at)

    runner.run("install", "--trust-workspace")

    kinds = _record_kinds(project_dir / ".grimoire" / "state.json")
    assert "skill" in kinds, "positive control: the skill's record is written"
    assert "mod" not in kinds, kinds
    assert (project_dir / ".claude" / "mods" / "status-band" / "src").is_dir()
    status = runner.run("status")
    assert "discarded mod record" not in status.stderr, status.stderr


def test_no_global_state_file_holds_a_mod_record(
    grim_binary: Path, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    repo = _git_repo(tmp_path / "repo")
    mod = _golden_mod(unique_repo)
    skill = make_artifact(
        f"{unique_repo}/control", "skill", {"control/SKILL.md": SKILL}, "1"
    )
    write_config(
        grim_home,
        skills={"control": skill.fq},
        mods={"status-band": mod.fq},
        experimental_mods=True,
        vendors=OPENCODE_V2,
        options={"clients": json.dumps(["claude"])},
    )
    sandbox, roots = mod_sandbox(tmp_path / "sandbox")
    runner = GrimRunner(grim_binary, grim_home, cwd=repo)
    runner.env.update(sandbox)
    runner.run("--global", "lock")

    runner.run("--global", "install", "--trust-workspace")

    kinds = _record_kinds(grim_home / "state" / "global.json")
    assert "skill" in kinds, "positive control: the skill's record is written"
    assert "mod" not in kinds, kinds
    assert (roots["claude"] / "mods" / "status-band").is_dir()
    status = runner.run("--global", "status")
    assert "discarded mod record" not in status.stderr, status.stderr


# The mod directories `test_mod_build.py` builds: id -> (files, mod.toml tail,
# clients with the entry each writes).
BUILT_MODS = {
    "default-entry-both": (
        {"claude/band.tsx": "a\n", "opencode/band.ts": "b\n"},
        "",
        {"claude": "band.tsx", "opencode": "band.ts"},
    ),
    "author-entry": (
        {"claude/main.ts": "x\n"},
        '[claude]\nentry = "main.ts"\n',
        {"claude": "main.ts"},
    ),
    "claude-only": ({"claude/band.tsx": "x\n"}, "", {"claude": "band.tsx"}),
}


@pytest.mark.parametrize("built", sorted(BUILT_MODS))
def test_every_built_mod_passes_install_validation(
    built: str, grim_at, project_dir: Path, tmp_path: Path, unique_repo: str, registry
) -> None:
    files, toml_tail, written = BUILT_MODS[built]
    source = tmp_path / "src" / "band"
    toml = 'schema = 1\nname = "band"\ndescription = "Build status band"\n' + (
        "\n" + toml_tail if toml_tail else ""
    )
    (source / "mod.toml").parent.mkdir(parents=True, exist_ok=True)
    (source / "mod.toml").write_text(toml, encoding="utf-8", newline="\n")
    for rel, body in files.items():
        (source / rel).parent.mkdir(parents=True, exist_ok=True)
        (source / rel).write_text(body, encoding="utf-8", newline="\n")

    ref = f"{registry}/{unique_repo}/band:1"
    runner = grim_at(project_dir)
    assert runner.json("release", str(source), ref)["pushed"] is True

    write_config(
        project_dir,
        mods={"band": ref},
        experimental_mods=True,
        vendors=OPENCODE_V2,
    )
    runner.env.update(mod_sandbox(tmp_path / "sandbox")[0])
    runner.run("lock")
    runner.run("install", "--trust-workspace", *BOTH)

    for client, entry in written.items():
        base = (
            project_dir / ".claude" / "mods" / "band"
            if client == "claude"
            else project_dir / ".opencode" / "plugins" / "band"
        )
        assert (base / "src" / entry).read_text(encoding="utf-8") == files[
            f"{client}/{entry}"
        ], (client, sorted(str(p) for p in base.rglob("*")))
    if "opencode" not in written:
        assert not (project_dir / ".opencode" / "plugins").exists()


def test_add_then_install_keeps_the_mod_store_unchanged(
    grim_at, grim_home: Path, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """`grim add` hands the mod pass a lock whose pins carry their tag and
    `grim install` one without it. The store records the pin without the tag,
    so switching between the two commands rewrites nothing."""
    mod = make_mod(f"{unique_repo}/status-band", "1")
    write_config(project_dir, experimental_mods=True)
    (project_dir / ".claude").mkdir(exist_ok=True)
    runner = grim_at(project_dir)
    runner.env.update(mod_sandbox(tmp_path / "sandbox")[0])
    runner.run("add", "--kind", "mod", "--trust-workspace", mod.fq)
    [store] = sorted((grim_home / "mods").glob("*.json"))
    before = store.read_bytes()
    assert b"status-band" in before, "positive control: add recorded the mod"
    assert f"{mod.repo}:1@".encode() not in before, "the record keeps no tag"

    runner.run("install", "--trust-workspace")

    assert store.read_bytes() == before


def test_global_install_and_uninstall_keep_the_users_marketplace_and_drop_a_stale_one(
    grim_binary: Path,
    grim_home: Path,
    tmp_path: Path,
    unique_repo: str,
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    sandbox, roots = mod_sandbox(tmp_path / "global-sandbox")
    settings = roots["claude"] / "settings.json"
    mine = {"source": {"source": "github", "repo": "me/market"}}
    _write_json(
        settings,
        {
            "extraKnownMarketplaces": {
                "my-market": mine,
                "grim-mods-aaaaaaaaaaaa": {"source": {"source": "directory"}},
            },
            "theme": "dark",
        },
    )
    write_config(
        grim_home,
        mods={"status-band": mod.fq},
        experimental_mods=True,
        options={"clients": '["claude"]'},
    )
    runner = GrimRunner(grim_binary, grim_home, cwd=tmp_path)
    runner.env.update(sandbox)
    runner.run("--global", "lock")

    runner.run("--global", "install", "--trust-workspace")

    markets = _read_json(settings)["extraKnownMarketplaces"]
    (ours,) = [k for k in markets if k.startswith("grim-mods-")]
    assert ours != "grim-mods-aaaaaaaaaaaa", "the stale grim-mods key is removed"
    assert markets["my-market"] == mine
    assert _read_json(settings)["theme"] == "dark"

    runner.run("--global", "uninstall", "mod", "status-band")

    after = _read_json(settings)
    assert after["extraKnownMarketplaces"] == {"my-market": mine}
    assert after["theme"] == "dark"
    # Claude's own registry file is never created by a mod install.
    assert not (roots["claude"] / "plugins" / "known_marketplaces.json").exists()
