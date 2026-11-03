#!/usr/bin/env python3
"""Generate the pre-dependencies golden fixture with a baseline `grim`.

The baseline is grim 0.15.1 (commit cdeedcaa), the last release before
declared artifact dependencies. Never run this with a newer binary: the
fixture is a record of what 0.15.1 wrote, not an expectation to refresh.

Self-contained: stdlib only, no dependency on `test/src/*`, so the fixture
bytes never move when the harness moves. Everything pushed to the registry
is recorded byte-for-byte under <out>/registry/ so a consumer can replay
the exact manifests and get the exact digests without this script.

Usage: GOLDEN_GRIM=/path/to/grim-0.15.1 generate.py <scratch-out-dir>
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

# Fixed, non-UUID repo namespace: a random one would put a fresh string in
# the golden lock every run.
NS = "grim-golden/pre-dependencies-cdeedcaa"

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


def get_manifest_bytes(repo: str, tag: str) -> bytes:
    req = urllib.request.Request(f"{REGISTRY_BASE}/v2/{repo}/manifests/{tag}")
    req.add_header("Accept", MANIFEST_MT)
    with urllib.request.urlopen(req) as resp:
        return resp.read()


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
HOOK_FILES = {
    "reg-hook/hook.toml": (
        'schema = 1\nname = "reg-hook"\n'
        'description = "Golden observer hook."\n\n'
        '[[hooks]]\nid = "log-tool-call"\nevent = "PreToolUse"\n'
        'tier = "observer"\nmatcher = "*"\nargv = ["sh", "log.sh"]\n'
        'timeout = 5\npayload = "stdin"\n'
    ),
    "reg-hook/log.sh": "#!/bin/sh\ncat >/dev/null\n",
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

# A dependency-free skill, released with `grim release` from a directory
# outside any git tree. Its manifest bytes are the byte-identity baseline for
# a later grim releasing the same source.
BASELINE_SKILL = {
    "SKILL.md": (
        "---\nname: baseline-skill\n"
        "description: Dependency-free skill for the release byte-identity baseline.\n"
        "---\n# Baseline skill\n"
    )
}
BASELINE_REPO = f"{NS}/baseline-skill"
BASELINE_TAG = "1.0.0"


def local_bundle_toml(r: str) -> str:
    return f'[agents]\nlocal-bundled-agent = "{r}/reg-agent:1.0.0"\n'


def write_project_sources(project: Path, r: str) -> None:
    """Write the path-sourced tree a declaring project needs."""
    skill_dir = project / "skills" / "local-skill"
    skill_dir.mkdir(parents=True, exist_ok=True)
    for name, body in LOCAL_SKILL.items():
        (skill_dir / name).write_text(body)
    (project / "bundles").mkdir(exist_ok=True)
    (project / "bundles" / "local-bundle.toml").write_text(local_bundle_toml(r))


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
    # `out` is wiped, and this script must never be pointed at the committed
    # fixture: regenerating from a later tree destroys its whole value.
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
    push(f"{NS}/reg-skill", "1.0.0", "skill", tar_of(SKILL_FILES))
    push(f"{NS}/reg-rule", "1.0.0", "rule", tar_of(RULE_FILES))
    push(f"{NS}/reg-agent", "1.0.0", "agent", tar_of(AGENT_FILES))
    push(f"{NS}/reg-hook", "1.0.0", "hook", tar_of(HOOK_FILES))
    push(f"{NS}/reg-mcp", "1.0.0", "mcp", MCP_DESCRIPTOR_JSON.encode())
    # `reg-skill` is also declared directly (a member shared by two
    # provenances); `bundled-rule` only ever comes from this bundle.
    bundle_doc = json.dumps(
        {
            "members": [
                {
                    "kind": "skill",
                    "name": "reg-skill",
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
    push(f"{NS}/reg-bundle", "1.0.0", "bundle", bundle_doc.encode())

    (out / "registry").mkdir()
    (out / "registry" / "pushed.json").write_text(
        json.dumps(PUSHED, indent=2, sort_keys=True) + "\n"
    )
    manifest_digests = {rec["manifest_digest"] for rec in PUSHED}

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

    # ---- 3. project declaration ---------------------------------------
    r = f"{REGISTRY_HOST}/{NS}"
    write_project_sources(project, r)
    config = f"""\
[options.experimental]
hooks = true

[bundles]
local-bundle = "./bundles/local-bundle.toml"
reg-bundle = "{r}/reg-bundle:1.0.0"

[skills]
local-skill = "./skills/local-skill"
reg-skill = "{r}/reg-skill:1.0.0"

[hooks]
reg-hook = "{r}/reg-hook:1.0.0"

[mcp]
reg-mcp = "{r}/reg-mcp:1.0.0"
"""
    (project / "grimoire.toml").write_text(config)

    # ---- 4. global declaration ----------------------------------------
    # Direct artifacts only: 0.15.1 locks a registry bundle online only (the
    # bundle manifest fetch refuses offline), and the offline variants below
    # lock this scope.
    global_config = f"""\
[options.experimental]
hooks = true

[skills]
reg-skill = "{r}/reg-skill:1.0.0"

[rules]
reg-rule = "{r}/reg-rule:1.0.0"

[agents]
reg-agent = "{r}/reg-agent:1.0.0"

[hooks]
reg-hook = "{r}/reg-hook:1.0.0"

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
    # filesystem. Exactly ONE lock+install pass per scope, so the golden
    # describes a clean first install (a second pass flips `adopted`).
    # `--trust-hooks` answers the project's hook consent for this run only
    # and writes no consent record.
    step(["lock"], project)
    step(["install", "--client", "claude", "--trust-hooks"], project)
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

    prov = out / "provenance"
    prov.mkdir()
    for scope, cwd, extra in (
        ("project", project, []),
        ("global", grim_home, ["--global"]),
    ):
        proc = run(["status", *extra, "--format", "json"], cwd, env, check=False)
        text = proc.stdout.replace(str(env_root), "<ENVROOT>")
        (prov / f"status.{scope}.json").write_text(text)

    # ---- 7. tag-cache variants -----------------------------------------
    # The warm tag cache grim wrote while resolving, with and without the
    # manifest blobs it mirrored into the content store. Layer blobs are
    # left out: only the manifests are the store-hit / store-miss axis.
    for variant, with_manifests in (
        ("tag-cache", True),
        ("tag-cache-no-manifests", False),
    ):
        dest = g / variant
        # A `host:port` directory name cannot be checked out on Windows,
        # where grim itself writes `host_port`; store that spelling. A Unix
        # consumer restores `:` before use (see the README).
        for registry_dir in sorted((grim_home / "tags").iterdir()):
            shutil.copytree(
                registry_dir, dest / "tags" / registry_dir.name.replace(":", "_")
            )
        if with_manifests:
            for digest in sorted(manifest_digests):
                hexpart = digest.split(":", 1)[1]
                src = grim_home / "blobs" / "sha256" / hexpart[:2] / hexpart
                if not src.is_file():
                    raise SystemExit(f"manifest blob {digest} not in the store")
                dst = dest / "blobs" / "sha256" / hexpart[:2] / hexpart
                dst.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(src, dst)

    # ---- 8. remove the registry bundle (last: it mutates the project) --
    step(["remove", "bundle", "reg-bundle"], project, "# after the captures above")
    shutil.copyfile(
        project / "grimoire.lock", g / "project.after-remove.grimoire.lock"
    )
    shutil.copyfile(
        project / ".grimoire" / "state.json",
        g / "state.project.after-remove.json",
    )

    # ---- 9. release baseline -------------------------------------------
    # A fresh GRIM_HOME and a source directory under `out` (not a git tree),
    # so the manifest carries no revision/created provenance annotations.
    rel_home = env_root / "release-grim-home"
    rel_home.mkdir()
    rel_env = {**env, "GRIM_HOME": str(rel_home)}
    src_dir = env_root / "release-src" / "baseline-skill"
    src_dir.mkdir(parents=True)
    for name, body in BASELINE_SKILL.items():
        (src_dir / name).write_text(body)
    reference = f"{REGISTRY_HOST}/{BASELINE_REPO}:{BASELINE_TAG}"
    release_args = ["release", str(src_dir), reference, "--no-cascade"]
    run(release_args, env_root, rel_env)
    manifest_bytes = get_manifest_bytes(BASELINE_REPO, BASELINE_TAG)
    manifest = json.loads(manifest_bytes)
    bad = [k for k in manifest.get("annotations", {}) if "revision" in k or "created" in k]
    if bad:
        raise SystemExit(f"baseline manifest carries provenance annotations: {bad}")
    (out / "registry" / "release_baseline.json").write_text(
        json.dumps(
            {
                "reference": reference,
                "command": "grim release <source-dir> <reference> --no-cascade",
                "source_dir_name": "baseline-skill",
                "source_files": BASELINE_SKILL,
                "manifest_digest": sha256(manifest_bytes),
                "manifest": manifest_bytes.decode(),
            },
            indent=2,
            sort_keys=True,
        )
        + "\n"
    )
    log.append(
        "$ grim release <baseline-skill> "
        f"{reference} --no-cascade   # cwd=env, fresh GRIM_HOME, no git tree"
    )

    (out / "commands.txt").write_text("\n".join(log) + "\n")

    # The inputs, written explicitly rather than copied out of the live
    # project: copying would drag install outputs into the fixture input, and
    # a pre-existing splice target makes grim record `"adopted": true`.
    proj_fixture = out / "project"
    write_project_sources(proj_fixture, r)
    (proj_fixture / "grimoire.toml").write_text(config)
    (out / "global-grimoire.toml").write_text(global_config)


if __name__ == "__main__":
    # Absolute, always: a relative GRIM_HOME resolves against the process CWD
    # and leaks nested directories.
    generate(Path(sys.argv[1]).resolve())
    print("ok")
