#!/usr/bin/env python3
"""Generate the C-015 pre-hooks golden fixtures with a baseline `grim`.

Self-contained: stdlib only, no dependency on `test/src/*` so the fixture
bytes never move when the harness moves. Everything it pushes is recorded
byte-for-byte under <out>/registry/ so a consumer can replay the exact
manifests and get the exact digests without this script.
"""
from __future__ import annotations

import hashlib
import io
import json
import os
import shutil
import subprocess
import sys
import tarfile
import urllib.request
from pathlib import Path

REGISTRY_HOST = "localhost:5000"
REGISTRY_BASE = f"http://{REGISTRY_HOST}"

# Fixed, non-UUID repo namespace: the fixture must be reproducible, and a
# random namespace would put a fresh string in the golden lock every run.
NS = "grim-golden/pre-hooks-03e59b0"

MANIFEST_MT = "application/vnd.oci.image.manifest.v1+json"
LAYER_MT = "application/vnd.grimoire.artifact.layer.v1.tar"
EMPTY_CONFIG_MT = "application/vnd.oci.empty.v1+json"
KIND_ANNOTATION = "com.grimoire.kind"

GRIM = os.environ["GOLDEN_GRIM"]


def sha256(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def tar_of(files: dict[str, str]) -> bytes:
    """Deterministic uncompressed tar: sorted names, mtime 0, mode 0644."""
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as tar:
        for name in sorted(files):
            data = files[name].encode()
            info = tarfile.TarInfo(name=name)
            info.size = len(data)
            info.mode = 0o644
            info.mtime = 0
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            tar.addfile(info, io.BytesIO(data))
    return buf.getvalue()


def push_blob(repo: str, data: bytes) -> str:
    digest = sha256(data)
    start = urllib.request.Request(
        f"{REGISTRY_BASE}/v2/{repo}/blobs/uploads/", method="POST"
    )
    with urllib.request.urlopen(start) as resp:
        location = resp.headers["Location"]
    if location.startswith("/"):
        location = REGISTRY_BASE + location
    sep = "&" if "?" in location else "?"
    req = urllib.request.Request(
        f"{location}{sep}digest={digest}", data=data, method="PUT"
    )
    req.add_header("Content-Type", "application/octet-stream")
    with urllib.request.urlopen(req):
        pass
    return digest


PUSHED: list[dict] = []


def push(repo: str, tag: str, kind: str, layer: bytes) -> str:
    """Push one single-layer artifact; record its exact bytes. Returns digest."""
    config_blob = b"{}"
    config_digest = push_blob(repo, config_blob)
    layer_digest = push_blob(repo, layer)
    manifest = {
        "schemaVersion": 2,
        "mediaType": MANIFEST_MT,
        "artifactType": f"application/vnd.grimoire.{kind}.v1",
        "config": {
            "mediaType": EMPTY_CONFIG_MT,
            "digest": config_digest,
            "size": len(config_blob),
        },
        "layers": [
            {"mediaType": LAYER_MT, "digest": layer_digest, "size": len(layer)}
        ],
        "annotations": {KIND_ANNOTATION: kind},
    }
    manifest_bytes = json.dumps(manifest).encode()
    req = urllib.request.Request(
        f"{REGISTRY_BASE}/v2/{repo}/manifests/{tag}",
        data=manifest_bytes,
        method="PUT",
    )
    req.add_header("Content-Type", MANIFEST_MT)
    with urllib.request.urlopen(req):
        pass
    digest = sha256(manifest_bytes)
    PUSHED.append(
        {
            "repo": repo,
            "tag": tag,
            "kind": kind,
            "manifest_digest": digest,
            "manifest": manifest_bytes.decode(),
            "layer": layer.hex(),
        }
    )
    return digest


# --------------------------------------------------------------------------
# Fixture content — every byte fixed in this file.
# --------------------------------------------------------------------------

SKILL_FILES = {
    "reg-skill/SKILL.md": (
        "---\nname: reg-skill\ndescription: Registry-sourced golden skill.\n"
        "---\n# Registry skill\n"
    )
}
RULE_FILES = {"reg-rule.md": "# Registry rule\n\nGolden rule body.\n"}
AGENT_FILES = {
    "reg-agent.md": (
        "---\nname: reg-agent\ndescription: Registry-sourced golden agent.\n"
        "---\n# Registry agent\n"
    )
}
MCP_DESCRIPTOR_JSON = json.dumps(
    {
        "description": "Golden MCP descriptor.",
        "server": {"transport": "stdio", "command": "grim", "args": ["mcp"]},
    },
    sort_keys=True,
    separators=(",", ":"),
)

LOCAL_SKILL = {
    "SKILL.md": (
        "---\nname: local-skill\ndescription: Path-sourced golden skill.\n"
        "---\n# Local skill\n"
    )
}
LOCAL_RULE = "# Local rule\n\nPath-sourced golden rule body.\n"
LOCAL_AGENT = (
    "---\nname: local-agent\ndescription: Path-sourced golden agent.\n"
    "---\n# Local agent\n"
)


def write_project_sources(project: Path) -> None:
    """Write the path-sourced artifact tree a declaring project needs."""
    (project / "skills" / "local-skill").mkdir(parents=True, exist_ok=True)
    for name, body in LOCAL_SKILL.items():
        (project / "skills" / "local-skill" / name).write_text(body)
    (project / "rules").mkdir(exist_ok=True)
    (project / "rules" / "local-rule.md").write_text(LOCAL_RULE)
    (project / "agents").mkdir(exist_ok=True)
    (project / "agents" / "local-agent.md").write_text(LOCAL_AGENT)


def run(args: list[str], cwd: Path, env: dict, check=True):
    proc = subprocess.run(
        [GRIM, *args], cwd=cwd, env=env, capture_output=True, text=True
    )
    if check and proc.returncode != 0:
        raise SystemExit(
            f"grim {' '.join(args)} failed ({proc.returncode})\n"
            f"stdout:\n{proc.stdout}\nstderr:\n{proc.stderr}"
        )
    return proc


def generate(out: Path) -> None:
    # Guard: `out` is wiped, and this script must never be pointed at the
    # committed fixture. Regenerating from a post-hooks tree destroys the
    # fixture's whole value — see README.md "Why you must not regenerate".
    if (out / "README.md").exists():
        raise SystemExit(
            f"refusing to write into {out}: it looks like the committed "
            "fixture. Generate into a scratch directory instead."
        )
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)

    # ---- 1. registry content ------------------------------------------
    PUSHED.clear()
    sk = push(f"{NS}/reg-skill", "1.0.0", "skill", tar_of(SKILL_FILES))
    ru = push(f"{NS}/reg-rule", "1.0.0", "rule", tar_of(RULE_FILES))
    ag = push(f"{NS}/reg-agent", "1.0.0", "agent", tar_of(AGENT_FILES))
    mc = push(f"{NS}/reg-mcp", "1.0.0", "mcp", MCP_DESCRIPTOR_JSON.encode())
    bundle_doc = json.dumps(
        {
            "members": [
                {
                    "kind": "skill",
                    "name": "bundled-skill",
                    "id": f"{REGISTRY_HOST}/{NS}/reg-skill:1.0.0",
                },
                {
                    "kind": "rule",
                    "name": "bundled-rule",
                    "id": f"{REGISTRY_HOST}/{NS}/reg-rule:1.0.0",
                },
            ]
        }
    )
    bu = push(f"{NS}/reg-bundle", "1.0.0", "bundle", bundle_doc.encode())

    (out / "registry").mkdir()
    (out / "registry" / "pushed.json").write_text(
        json.dumps(PUSHED, indent=2, sort_keys=True) + "\n"
    )

    # ---- 2. isolated environment --------------------------------------
    env_root = out / "_env"
    home = env_root / "home"
    grim_home = env_root / "grim-home"
    project = env_root / "project"
    for d in (home, grim_home, project):
        d.mkdir(parents=True)

    env = {
        "PATH": "/usr/bin:/bin",
        "HOME": str(home),
        "GRIM_HOME": str(grim_home),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "GRIM_INSECURE_REGISTRIES": REGISTRY_HOST,
        "NO_COLOR": "1",
    }

    # ---- 3. project declaration (hook-free, all five kinds) -----------
    write_project_sources(project)

    r = f"{REGISTRY_HOST}/{NS}"
    config = f"""\
[bundles]
reg-bundle = "{r}/reg-bundle:1.0.0"

[skills]
local-skill = "./skills/local-skill"
reg-skill = "{r}/reg-skill:1.0.0"

[rules]
local-rule = "./rules/local-rule.md"
reg-rule = "{r}/reg-rule:1.0.0"

[agents]
local-agent = "./agents/local-agent.md"
reg-agent = "{r}/reg-agent:1.0.0"

[mcp]
reg-mcp = "{r}/reg-mcp:1.0.0"
"""
    (project / "grimoire.toml").write_text(config)

    # ---- 4. global declaration (drives state/global.json) -------------
    global_config = f"""\
[skills]
reg-skill = "{r}/reg-skill:1.0.0"

[rules]
reg-rule = "{r}/reg-rule:1.0.0"

[agents]
reg-agent = "{r}/reg-agent:1.0.0"

[mcp]
reg-mcp = "{r}/reg-mcp:1.0.0"
"""
    (grim_home / "grimoire.toml").write_text(global_config)
    # Global-scope client detection needs the native roots to exist.
    for d in (".claude", ".copilot", ".codex"):
        (home / d).mkdir(parents=True, exist_ok=True)

    # ---- 5. run the baseline binary -----------------------------------
    log: list[str] = []

    def step(args, cwd, note=""):
        proc = run(args, cwd, env)
        log.append(f"$ grim {' '.join(args)}   # cwd={cwd.name} {note}")
        return proc

    # Clients are pinned explicitly: auto-detection reads the ambient
    # filesystem, which is exactly the environment dependence a golden
    # fixture must not carry. Exactly ONE lock+install pass per scope —
    # what a consuming test does — so the golden describes a clean first
    # install (a second pass would flip `adopted` semantics).
    step(["lock"], project)
    step(["install", "--client", "claude"], project)
    step(["lock", "--global"], grim_home)
    step(["install", "--global", "--client", "claude"], grim_home)

    # ---- 6. capture ----------------------------------------------------
    g = out / "golden"
    g.mkdir()
    shutil.copyfile(project / "grimoire.lock", g / "project.grimoire.lock")
    shutil.copyfile(grim_home / "grimoire.lock", g / "global.grimoire.lock")
    shutil.copyfile(
        grim_home / "state" / "global.json", g / "state.global.json"
    )
    shutil.copyfile(
        project / ".grimoire" / "state.json", g / "state.project.json"
    )

    # declaration_hash, machine-readable, from the locks themselves
    def decl_hash(lock_path: Path) -> str:
        for line in lock_path.read_text().splitlines():
            if line.startswith("declaration_hash ="):
                return line.split("=", 1)[1].strip().strip('"')
        raise SystemExit(f"no declaration_hash in {lock_path}")

    (g / "declaration_hash.json").write_text(
        json.dumps(
            {
                "declaration_hash_version": 1,
                "project": decl_hash(project / "grimoire.lock"),
                "global": decl_hash(grim_home / "grimoire.lock"),
            },
            indent=2,
        )
        + "\n"
    )

    # status, JSON, for the record (paths normalized)
    for scope, cwd, extra in (
        ("project", project, []),
        ("global", grim_home, ["--global"]),
    ):
        proc = run(["status", *extra, "--format", "json"], cwd, env, check=False)
        text = proc.stdout.replace(str(env_root), "<ENVROOT>")
        (g / f"status.{scope}.json").write_text(text)

    proc = run(["context", "--format", "json"], project, env)
    (g / "context.project.json").write_text(
        proc.stdout.replace(str(env_root), "<ENVROOT>")
    )

    (out / "commands.txt").write_text("\n".join(log) + "\n")

    # The project sources, written explicitly rather than copied out of the
    # live project: copying would drag install outputs (`.mcp.json`,
    # `.claude/`, `.grimoire/state.json`) into the fixture *input*, and a
    # pre-existing splice target makes grim record `"adopted": true` instead
    # of `false` — the fixture would then no longer describe a clean install.
    proj_fixture = out / "project"
    write_project_sources(proj_fixture)
    (out / "global-grimoire.toml").write_text(global_config)
    (proj_fixture / "grimoire.toml").write_text(config)


if __name__ == "__main__":
    # Absolute, always: a relative GRIM_HOME makes grim resolve it against
    # the process CWD (see the plan's B1 finding) and leaks nested dirs.
    generate(Path(sys.argv[1]).resolve())
    print("ok")
