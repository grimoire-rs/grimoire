# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Every settings host, end to end at global scope.

The host list comes from the shipped schema, so a new vendor table joins this
module without an edit. Each host builds one boolean key, installs it, keeps it
through a second run that reloads the state, reports it in ``status`` and
removes it on uninstall. Amp and Gemini keep grim's MCP entry in the same file,
which must survive every step. Every run has a scratch ``HOME``.
"""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

import pytest
import tomllib

from src.helpers import make_settings, settings_layer, settings_sandbox, write_config
from src.runner import GrimRunner

SCHEMA = Path(__file__).resolve().parents[2] / "src/command/testdata/settings.schema.json"


def _vendor_tables(schema: dict) -> dict[str, dict]:
    """The vendor tables: object properties that carry no description of their own."""
    return {
        name: node
        for name, node in schema["properties"].items()
        if node.get("type") == "object" and "description" not in node
    }


def _first_boolean(node: dict, path: tuple[str, ...] = ()) -> tuple[str, ...] | None:
    if node.get("type") == "boolean":
        return path
    for key, child in node.get("properties", {}).items():
        if (found := _first_boolean(child, (*path, key))) is not None:
            return found
    return None


TABLES = _vendor_tables(json.loads(SCHEMA.read_text(encoding="utf-8")))
HOSTS = sorted(TABLES)

# Hosts whose global settings file also carries grim's MCP entries, by the key
# that holds them (Amp's is one literal dotted key).
MCP_CONTAINER = {"amp": "amp.mcpServers", "gemini": "mcpServers"}

MCP_DESCRIPTOR = 'description = "d"\n\n[server]\ntransport = "stdio"\ncommand = "grim"\nargs = ["mcp"]\n'


def _nested(path: tuple[str, ...], value: object) -> dict:
    for key in reversed(path):
        value = {key: value}
    return value  # type: ignore[return-value]


def _source(root: Path, host: str, leaf: tuple[str, ...]) -> Path:
    """`settings/team.toml` setting `leaf` to true, each key quoted as written."""
    key = ".".join(json.dumps(part) for part in leaf)
    path = root / "settings" / "team.toml"
    path.parent.mkdir(parents=True)
    path.write_text(f'description = "every host"\n\n[{host}]\n{key} = true\n', encoding="utf-8", newline="\n")
    return path


def _read(file: Path) -> dict:
    if not file.exists():
        return {}
    text = file.read_text(encoding="utf-8")
    return tomllib.loads(text) if file.suffix == ".toml" else json.loads(text)


def _value(file: Path, leaf: tuple[str, ...]) -> object:
    node: object = _read(file)
    for key in leaf:
        if not isinstance(node, dict) or key not in node:
            return None
        node = node[key]
    return node


def _records(grim: GrimRunner) -> list[dict]:
    state = Path(grim.grim_home) / "state" / "global.json"
    records = json.loads(state.read_text(encoding="utf-8"))["records"] if state.exists() else []
    return [record for record in records if record["kind"] == "settings"]


def _on_disk(grim: GrimRunner, shown: str) -> Path:
    """A change row's `file`, which names the runner's home as `~`."""
    return grim.home / shown[2:] if shown.startswith("~/") else Path(shown)


def _mcp_servers(file: Path, host: str) -> list[str]:
    return sorted(_read(file).get(MCP_CONTAINER[host], {}))


def test_every_vendor_table_is_a_host_here(grim: GrimRunner, tmp_path: Path) -> None:
    shipped = json.loads(grim.plain("schema", "--kind", "settings").stdout)
    assert sorted(_vendor_tables(shipped)) == HOSTS
    _, roots = settings_sandbox(tmp_path / "s", tmp_path / "home")
    assert sorted(roots) == HOSTS
    for host in HOSTS:
        assert _first_boolean(TABLES[host]) is not None, host
    assert set(MCP_CONTAINER) <= set(HOSTS)


@pytest.mark.parametrize("host", HOSTS)
def test_a_global_value_round_trips_on_every_host(
    grim: GrimRunner, grim_home: Path, tmp_path: Path, registry: str, unique_repo: str, host: str
) -> None:
    leaf = _first_boolean(TABLES[host])
    assert leaf is not None
    document = {"description": "every host", host: _nested(leaf, True)}

    built = grim.json("build", str(_source(tmp_path / "src", host, leaf)), "--kind", "settings")
    assert built["layer_digest"] == "sha256:" + hashlib.sha256(settings_layer(document)).hexdigest()
    team = make_settings(f"{unique_repo}/team", "1", document)

    env, _ = settings_sandbox(tmp_path / "s", grim.home)
    grim.env.update(env)
    # Cursor writes only into a `cli-config.json` its CLI already created.
    (grim.home / ".cursor" / "cli-config.json").write_text("{}", encoding="utf-8", newline="\n")
    write_config(
        grim_home, settings={"team": team.fq}, options={"clients": json.dumps([host])}, experimental_settings=True
    )
    servers: list[str] = []
    if host in MCP_CONTAINER:
        descriptor = tmp_path / "src" / "mcp" / "shared.toml"
        descriptor.parent.mkdir(parents=True)
        descriptor.write_text(MCP_DESCRIPTOR, encoding="utf-8", newline="\n")
        ref = f"{registry}/{unique_repo}/mcp/shared:1.0.0"
        grim.json("release", str(descriptor), ref, "--kind", "mcp")
        with (grim_home / "grimoire.toml").open("a", encoding="utf-8") as config:
            config.write(f'[mcp]\nshared = "{ref}"\n')
        servers = ["shared"]
    grim.run("--global", "lock")

    if host == "warp" and sys.platform == "win32":
        # Warp has no settings file on Windows, so the package declines.
        declined = grim.run("--global", "install", format="json")
        assert json.loads(declined.stdout)["settings_changes"] == []
        assert "no-applicable-settings" in declined.stderr
        return

    installed = grim.json("--global", "install")
    [change] = installed["settings_changes"]
    file = _on_disk(grim, change["file"])
    assert (change["client"], change["action"]) == (host, "added"), change
    assert _value(file, leaf) is True
    if servers:
        assert _mcp_servers(file, host) == servers

    # A second run loads the record this one saved and finds nothing to do.
    again = grim.json("--global", "install")
    assert again["settings_changes"] == []
    assert _value(file, leaf) is True
    [record] = _records(grim)
    assert [output["client"] for output in record["outputs"]] == [host]
    if servers:
        assert _mcp_servers(file, host) == servers

    [row] = [item for item in grim.json("--global", "status")["items"] if item["name"] == "team"]
    assert row["state"] == "installed", row
    assert row["arming"] == []
    assert [(output["client"], Path(output["path"]).resolve()) for output in row["outputs"]] == [(host, file.resolve())]

    grim.run("--global", "uninstall", "settings", "team")
    assert _value(file, leaf) is None
    assert _records(grim) == []
    if servers:
        assert _mcp_servers(file, host) == servers
