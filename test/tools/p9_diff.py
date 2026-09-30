# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Principle 9 differential run: ``origin/main``'s grim against this branch's.

Contract C-151 (``.agents/specs/design_hooks_pr98_revival.md``). Both binaries
run one hook-free scenario list, sequentially, under the identical absolute
root (wiped between runs) with identical ``HOME``, ``GRIM_HOME`` and
environment, against one throwaway zot registry whose tag state is reset to
the same bytes before each run. After **every** command the whole root
(workspace, fake ``HOME``, ``GRIM_HOME``) is snapshotted; the two runs must
match byte for byte outside the named ``VOLATILE`` mask, and ``--format json``
stdout may differ only by the C-153 additive keys.

Usage (``task p9:diff`` wires the builds and puts zot on PATH)::

    uv run python tools/p9_diff.py --main-bin A --branch-bin B --root DIR
    uv run python tools/p9_diff.py --self-test --root DIR

Exit 0: no difference. Exit 1: differences (printed). Exit 2: a scenario
failed to run as expected (non-zero exit, missing expected output).
"""

from __future__ import annotations

import argparse
import fnmatch
import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

TEST_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(TEST_DIR))
from src.zot import start_zot  # noqa: E402

FIXTURES = Path(__file__).resolve().parent / "p9_fixtures"

# C-153's closed list, restricted to what a hook-free run can show: `status`
# items gain `arming: []`, install rows gain `armed: null`. Keyed by command
# and value; nothing else may appear. Per D-20, `add`/`update` rows keep
# main's exact key set — no `armed` — so they are deliberately absent here.
ADDITIVE_STDOUT_KEYS: dict[str, tuple[str, object]] = {
    "status": ("arming", []),
    "install": ("armed", None),
}

# Named stdout equivalences (orchestrator decision D-13). Each is one rule, not
# a class of rules.
#   c118-relative-config-target: under a relative `--config`, main prints a
#   `target` relative to the cwd and the branch (C-118, PR 2a436191) prints it
#   absolute. Equal iff cwd / main's value == the branch's value.
STDOUT_EQUIVALENCES = ("c118-relative-config-target",)

# Named volatile fields: (root-relative path glob, key). A `key = "…"` (TOML)
# or `"key": "…"` (JSON) string value is masked in matching files; nothing
# else is. Each entry was observed to vary between two runs of one binary,
# except `generated_by`, which carries the grim version string.
VOLATILE: tuple[tuple[str, str], ...] = (
    ("*grimoire.lock", "generated_at"),
    ("*grimoire.lock", "generated_by"),
)

# Project scope cannot detect antigravity/openclaw (no project marker exists).
PROJECT_CLIENTS = frozenset(
    {
        "claude",
        "opencode",
        "copilot",
        "codex",
        "cursor",
        "kiro",
        "junie",
        "gemini",
        "zed",
        "amp",
        "cline",
        "droid",
        "goose",
        "warp",
        "kilo",
        "qoder",
    }
)
GLOBAL_CLIENTS = PROJECT_CLIENTS | {"antigravity", "openclaw"}

WS_MARKERS = (
    ".claude",
    ".opencode",
    ".codex",
    ".cursor",
    ".kiro",
    ".junie",
    ".gemini",
    ".zed",
    ".amp",
    ".cline",
    ".factory",
    ".goose",
    ".warp",
    ".kilo",
    ".qoder",
    ".github/instructions",
)
HOME_MARKERS = (
    ".claude",
    ".copilot/skills",
    ".config/opencode/skills",
    ".codex",
    ".cursor",
    ".kiro",
    ".junie",
    ".gemini/config",
    ".config/zed",
    ".config/amp",
    ".cline",
    ".factory",
    ".config/goose",
    ".warp",
    ".openclaw",
    ".config/kilo",
    ".qoder",
)

# (fixture path under v1/v2, --kind, repo under <registry>/p9)
ARTIFACTS = (
    ("skills/p9-member", "skill", "skills/p9-member"),
    ("skills/p9-skill", "skill", "skills/p9-skill"),
    ("skills/p9-hookish", "skill", "skills/p9-hookish"),
    ("rules/p9-rule.md", "rule", "rules/p9-rule"),
    ("agents/p9-agent.md", "agent", "agents/p9-agent"),
    ("mcp/p9-mcp.toml", "mcp", "mcp/p9-mcp"),
    ("bundles/p9-bundle.toml", "bundle", "bundles/p9-bundle"),
)
# Registry-state sentinels in the step list: tags reset to exactly the v1 / v2 publish.
REGISTRY_V1, REGISTRY_V2 = "<registry v1>", "<registry v2>"


class ScenarioError(Exception):
    """A command did not behave as the scenario expects — the run is invalid."""


@dataclass
class Step:
    args: tuple[str, ...]
    expects: tuple[str, ...] = ()  # root-relative paths that must exist non-empty
    cwd: str = "ws"
    rc: int = 0
    marker: tuple[str, str] | None = None  # (root-relative path, text it must contain)


@dataclass
class Checkpoint:
    label: str
    rc: int
    stdout: str
    tree: dict[str, tuple] = field(repr=False)
    command: str = ""
    cwd: str = "/"
    warnings: frozenset[str] = frozenset()


def scenario(reg: str) -> list[Step | str]:
    """The hook-free command list (C-151). ``reg`` is ``<host>/p9``."""
    add = [
        f"{reg}/{repo}:1" for _, kind, repo in ARTIFACTS if repo != "skills/p9-member"
    ]
    steps: list[Step | str] = []
    for scope, cfg, base, mcp in (
        ((), "ws/grimoire.toml", "ws", "ws/.mcp.json"),
        (("--global",), "grim_home/grimoire.toml", "home", "home/.claude.json"),
    ):
        lock = cfg.replace(".toml", ".lock")
        out = f"{base}/.claude/skills/p9-skill/SKILL.md"
        # One rendered output per kind, so two binaries that both render
        # nothing for a kind cannot pass.
        rendered = (
            lock,
            out,
            f"{base}/.claude/skills/p9-hookish/SKILL.md",
            f"{base}/.claude/skills/p9-member/SKILL.md",
            f"{base}/.claude/rules/p9-rule.md",
            f"{base}/.claude/agents/p9-agent.md",
            mcp,
        )
        steps += [
            REGISTRY_V1,
            Step(("init", *scope), (cfg,)),
            Step(("context", *scope), (cfg,)),
            *(Step(("add", *scope, "--no-install", ref), (cfg, lock)) for ref in add),
            Step(("install", *scope), rendered, marker=(out, "(v1)")),
            Step(("status", *scope), rendered),
            REGISTRY_V2,
            Step(("update", *scope), rendered, marker=(out, "(v2)")),
            Step(("status", *scope), rendered),
        ]
        steps += [
            Step(("uninstall", *scope, "skill", "p9-skill"), (lock,)),
            Step(("remove", *scope, "rule", "p9-rule"), (lock,)),
            Step(("install", *scope), (lock,)),
            Step(("status", *scope), (lock,)),
        ]
    # Relative --config from outside the workspace (C-118 must not move a byte).
    steps += [
        Step(
            ("--config", "ws/grimoire.toml", "status"), ("ws/grimoire.lock",), cwd="."
        ),
        Step(
            ("--config", "ws/grimoire.toml", "install"), ("ws/grimoire.lock",), cwd="."
        ),
        Step(
            ("--config", "ws/grimoire.toml", "uninstall", "agent", "p9-agent"),
            ("ws/grimoire.lock",),
            cwd=".",
        ),
    ]
    return steps


# --------------------------------------------------------------------------
# Snapshots and comparison
# --------------------------------------------------------------------------


def snapshot(root: Path) -> dict[str, tuple]:
    """Every entry under ``root``, sorted: ``rel -> (kind, payload, exec bit)``."""
    tree: dict[str, tuple] = {}
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        base = Path(dirpath)
        for name in sorted(dirnames + filenames):
            p = base / name
            rel = p.relative_to(root).as_posix()
            if p.is_symlink():
                tree[rel] = ("link", os.readlink(p).encode(), False)
            elif p.is_dir():
                tree[rel] = ("dir", b"", False)
            else:
                tree[rel] = (
                    "file",
                    _mask_file(rel, p.read_bytes()),
                    os.access(p, os.X_OK),
                )
    return dict(sorted(tree.items()))


def _mask_file(rel: str, data: bytes) -> bytes:
    for glob, key in VOLATILE:
        if fnmatch.fnmatch(rel, glob):
            pattern = rb'^(\s*"?' + re.escape(key.encode()) + rb'"?\s*[=:]\s*)"[^"\n]*"'
            data = re.sub(pattern, rb'\1"<masked>"', data, flags=re.MULTILINE)
    return data


def diff_trees(label: str, a: dict[str, tuple], b: dict[str, tuple]) -> list[str]:
    out = []
    for rel in sorted(a.keys() | b.keys()):
        if rel not in b:
            out.append(f"{label}: {rel} only with main")
        elif rel not in a:
            out.append(f"{label}: {rel} only with branch")
        elif a[rel] != b[rel]:
            (ka, pa, xa), (kb, pb, xb) = a[rel], b[rel]
            if ka != kb or xa != xb:
                out.append(f"{label}: {rel} type/mode {ka},{xa} != {kb},{xb}")
            else:
                i = next(
                    (i for i, (x, y) in enumerate(zip(pa, pb, strict=False)) if x != y),
                    min(len(pa), len(pb)),
                )
                out.append(
                    f"{label}: {rel} differs at byte {i} ({len(pa)} vs {len(pb)} bytes)"
                )
    return out


def _c118_equal(key: str, a: object, b: object, cwd: str) -> bool:
    """``c118-relative-config-target``: see ``STDOUT_EQUIVALENCES``."""
    return (
        key == "target"
        and isinstance(a, str)
        and isinstance(b, str)
        and os.path.normpath(os.path.join(cwd, a)) == os.path.normpath(b)
    )


def diff_json(
    label: str, a, b, path: str = "$", command: str = "", cwd: str = "/"
) -> list[str]:
    """Structural diff: the branch may only add the command's C-153 key."""
    if isinstance(a, dict) and isinstance(b, dict):
        allowed = ADDITIVE_STDOUT_KEYS.get(command)
        out = []
        for k in sorted(a.keys() | b.keys()):
            if k not in b:
                out.append(f"{label}: stdout {path}.{k} dropped by branch")
            elif k not in a:
                if allowed is None or k != allowed[0]:
                    out.append(
                        f"{label}: stdout {path}.{k} added by branch, not a C-153 key"
                    )
                elif b[k] != allowed[1]:
                    out.append(
                        f"{label}: stdout {path}.{k} = {b[k]!r}, hook-free run "
                        f"must print {allowed[1]!r}"
                    )
            elif _c118_equal(k, a[k], b[k], cwd):
                continue
            else:
                out += diff_json(label, a[k], b[k], f"{path}.{k}", command, cwd)
        return out
    if isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            return [f"{label}: stdout {path} length {len(a)} != {len(b)}"]
        return [
            m
            for i, (x, y) in enumerate(zip(a, b, strict=True))
            for m in diff_json(label, x, y, f"{path}[{i}]", command, cwd)
        ]
    return [] if a == b else [f"{label}: stdout {path} {a!r} != {b!r}"]


def diff_stdout(
    label: str, a: str, b: str, command: str = "", cwd: str = "/"
) -> list[str]:
    try:
        ja, jb = json.loads(a), json.loads(b)
    except ValueError:
        return [] if a == b else [f"{label}: stdout (non-JSON) differs"]
    return diff_json(label, ja, jb, "$", command, cwd)


def compare(main: list[Checkpoint], branch: list[Checkpoint]) -> list[str]:
    if [c.label for c in main] != [c.label for c in branch]:
        return ["checkpoint lists differ in shape"]
    out = []
    for a, b in zip(main, branch, strict=True):
        if a.rc != b.rc:
            out.append(f"{a.label}: exit {a.rc} != {b.rc}")
        out += diff_stdout(a.label, a.stdout, b.stdout, a.command, a.cwd)
        out += [
            f"{a.label}: branch-only warning: {w}"
            for w in sorted(b.warnings - a.warnings)
        ]
        out += diff_trees(a.label, a.tree, b.tree)
    return out


# --------------------------------------------------------------------------
# Registry
# --------------------------------------------------------------------------

_ACCEPT = (
    "application/vnd.oci.image.manifest.v1+json, "
    "application/vnd.oci.image.index.v1+json, "
    "application/vnd.docker.distribution.manifest.v2+json"
)


def _http(method: str, url: str, data: bytes | None = None, ctype: str | None = None):
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Accept", _ACCEPT)
    if ctype:
        req.add_header("Content-Type", ctype)
    with urllib.request.urlopen(req, timeout=30) as resp:
        return resp.read(), resp.headers


RegState = dict[
    str, dict[str, tuple[str, bytes, str]]
]  # repo -> tag -> (digest, bytes, ctype)


def reg_state(host: str, ns: str = "p9") -> RegState:
    state: RegState = {}
    for _, _, repo in ARTIFACTS:
        name = f"{ns}/{repo}"
        body, _ = _http("GET", f"http://{host}/v2/{name}/tags/list")
        tags = sorted(json.loads(body).get("tags") or [])
        state[name] = {}
        for tag in tags:
            data, headers = _http("GET", f"http://{host}/v2/{name}/manifests/{tag}")
            state[name][tag] = (
                headers["Docker-Content-Digest"],
                data,
                headers["Content-Type"],
            )
    return state


def reg_apply(host: str, target: RegState, full: RegState) -> None:
    """Set the registry's tags to exactly ``target``; ``full`` names every push."""
    for name, tags in full.items():
        keep = {d for d, _, _ in target[name].values()}
        for digest in sorted({d for d, _, _ in tags.values()} - keep):
            try:
                _http("DELETE", f"http://{host}/v2/{name}/manifests/{digest}")
            except urllib.error.HTTPError as e:
                if e.code != 404:
                    raise
        for tag, (_, data, ctype) in target[name].items():
            _http("PUT", f"http://{host}/v2/{name}/manifests/{tag}", data, ctype)
    got = {n: {t: v[0] for t, v in tags.items()} for n, tags in reg_state(host).items()}
    want = {n: {t: v[0] for t, v in tags.items()} for n, tags in target.items()}
    if got != want:
        raise ScenarioError(f"registry reset did not take: {got} != {want}")


def publish(
    binary: Path, host: str, version: str, scratch: Path, ns: str = "p9"
) -> None:
    """Release every fixture of ``v1``/``v2`` with ``binary`` under ``<host>/<ns>``."""
    shutil.rmtree(scratch, ignore_errors=True)
    env = base_env(scratch, host)
    tag = {"v1": "1.0.0", "v2": "1.1.0"}[version]
    for src, kind, repo in ARTIFACTS:
        path = FIXTURES / version / src
        run = subprocess.run(
            [
                str(binary),
                "release",
                "--kind",
                kind,
                str(path),
                f"{host}/{ns}/{repo}:{tag}",
            ],
            env=env,
            cwd=scratch,
            capture_output=True,
            text=True,
            encoding="utf-8",
            check=False,
        )
        if run.returncode != 0:
            raise ScenarioError(
                f"publish {version} {repo} failed: {run.stderr.strip()}"
            )


def _layers(host: str, name: str, manifest: bytes) -> list[tuple[str, list[str]]]:
    """``(digest, member names)`` of every layer of ``manifest``."""
    out = []
    for layer in json.loads(manifest).get("layers", []):
        if layer.get("mediaType", "").endswith("+json"):  # bundle/mcp: no files
            out.append((layer["digest"], []))
            continue
        blob, _ = _http("GET", f"http://{host}/v2/{name}/blobs/{layer['digest']}")
        with tarfile.open(fileobj=io.BytesIO(blob), mode="r:*") as tar:
            out.append((layer["digest"], sorted(tar.getnames())))
    return out


# Manifest annotations grim derives from the push namespace; the branch
# publishes under `p9b/`, so these are compared with the namespace normalized.
# Every other manifest byte (layers, config, other annotations) must match.
NAMESPACE_ANNOTATIONS = (
    "org.opencontainers.image.source",
    "org.opencontainers.image.vendor",
)


def _manifest(data: bytes, ns: str) -> str:
    doc = json.loads(data)
    notes = doc.get("annotations", {})
    for key in NAMESPACE_ANNOTATIONS:
        if key in notes:
            notes[key] = re.sub(rf"(^|/){ns}(/|$)", r"\1<ns>\2", notes[key])
    return json.dumps(doc, sort_keys=True)


def diff_packing(host: str, main: RegState, branch: RegState) -> list[str]:
    """C-151/C-160: the branch packs every fixture to main's exact bytes.

    Both binaries release the same fixtures (main under ``p9/``, the branch
    under ``p9b/``); per repo and tag every layer digest and the manifest
    (namespace-derived annotations normalized) must match, and no layer of
    any package may carry ``hook.toml``.
    """
    out = []
    for name, tags in main.items():
        bname = "p9b/" + name.removeprefix("p9/")
        for tag in sorted(tags.keys() | branch[bname].keys()):
            if tag not in branch[bname] or tag not in tags:
                out.append(f"packing: {name}:{tag} tag set differs")
                continue
            ma, mb = tags[tag], branch[bname][tag]
            la, lb = _layers(host, name, ma[1]), _layers(host, bname, mb[1])
            if [d for d, _ in la] != [d for d, _ in lb]:
                out.append(f"packing: {name}:{tag} layer digests differ")
            elif _manifest(ma[1], "p9") != _manifest(mb[1], "p9b"):
                out.append(f"packing: {name}:{tag} manifests differ")
            for d, names in la + lb:
                if any(Path(n).name == "hook.toml" for n in names):
                    out.append(f"packing: {name}:{tag} layer {d} ships hook.toml")
    return out


# --------------------------------------------------------------------------
# Scenario runner
# --------------------------------------------------------------------------


def base_env(root: Path, host: str) -> dict[str, str]:
    home = root / "home"
    for d in (root / "ws", home, root / "grim_home"):
        d.mkdir(parents=True, exist_ok=True)
    return {
        "PATH": os.environ.get("PATH", ""),
        "HOME": str(home),
        "USERPROFILE": str(home),
        "APPDATA": str(home / "AppData/Roaming"),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "GRIM_HOME": str(root / "grim_home"),
        "GRIM_INSECURE_REGISTRIES": host,
        "GRIM_DEFAULT_REGISTRY": f"{host}/p9",
        "NO_COLOR": "1",
    }


def prepare_root(root: Path) -> None:
    shutil.rmtree(root, ignore_errors=True)
    for m in WS_MARKERS:
        (root / "ws" / m).mkdir(parents=True)
    for m in HOME_MARKERS:
        (root / "home" / m).mkdir(parents=True)


def run_scenario(
    binary: Path, root: Path, host: str, steps: list[Step | str], on_registry
) -> list[Checkpoint]:
    prepare_root(root)
    env = base_env(root, host)
    checkpoints: list[Checkpoint] = []
    for i, step in enumerate(steps):
        if isinstance(step, str):
            on_registry(step)
            continue
        label = f"#{i:02d} grim {' '.join(step.args)}"
        proc = subprocess.run(
            [str(binary), "--format", "json", *step.args],
            env=env,
            cwd=root / step.cwd,
            capture_output=True,
            text=True,
            encoding="utf-8",
            check=False,
        )
        if proc.returncode != step.rc:
            raise ScenarioError(
                f"{binary}: {label} exited {proc.returncode}, expected {step.rc}\n"
                f"{proc.stderr.strip()}"
            )
        for rel in step.expects:
            p = root / rel
            if not p.is_file() or p.stat().st_size == 0:
                raise ScenarioError(f"{binary}: {label} left no expected output {rel}")
        if step.marker and step.marker[1] not in (root / step.marker[0]).read_text(
            encoding="utf-8"
        ):
            raise ScenarioError(
                f"{binary}: {label} left no {step.marker[1]!r} in {step.marker[0]}"
            )
        try:
            json.loads(proc.stdout)
        except ValueError as e:
            raise ScenarioError(
                f"{binary}: {label} printed no JSON document on stdout"
            ) from e
        if step.args[0] == "context":
            _check_detection(label, proc.stdout, "--global" in step.args)
        command = next(
            a for a in step.args if not a.startswith("-") and not a.endswith(".toml")
        )
        checkpoints.append(
            Checkpoint(
                label,
                proc.returncode,
                proc.stdout,
                snapshot(root),
                command,
                str(root / step.cwd),
                _warnings(proc.stderr),
            )
        )
    return checkpoints


# The log line's leading RFC 3339 timestamp is the one volatile part of stderr.
_LOG_TIMESTAMP = re.compile(r"^\d{4}-\d\d-\d\dT[\d:.]+Z\s+")


def _warnings(stderr: str) -> frozenset[str]:
    """Warning lines on stderr (hook-free runs must not gain any, C-155)."""
    return frozenset(
        _LOG_TIMESTAMP.sub("", line.strip())
        for line in stderr.splitlines()
        if re.search(r"\bwarn", line, re.I)
    )


def _check_detection(label: str, stdout: str, is_global: bool) -> None:
    want = GLOBAL_CLIENTS if is_global else PROJECT_CLIENTS
    missing = want - set(json.loads(stdout)["clients"])
    if missing:
        raise ScenarioError(f"{label}: clients not detected: {sorted(missing)}")


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def identical_inputs(main_sha: str, main_bin: Path, branch_bin: Path) -> str | None:
    """Why the two sides are the same build, or ``None`` when they differ."""
    if sha256(main_bin) == sha256(branch_bin):
        return "main and branch binaries are byte-identical"
    same_src = subprocess.run(
        [
            "git",
            "diff",
            "--quiet",
            main_sha,
            "--",
            "src",
            "Cargo.toml",
            "Cargo.lock",
            "external",
        ],
        cwd=TEST_DIR.parent,
        check=False,
    )
    if same_src.returncode == 0:
        return f"the working tree's src/Cargo/external equal {main_sha}'s"
    return None


def differential(main_bin: Path, branch_bin: Path, root: Path) -> list[str]:
    root = root.resolve()
    zot = start_zot()
    try:
        host = zot.host
        scratch = root.parent / "p9-publish"
        publish(main_bin, host, "v1", scratch)
        v1 = reg_state(host)
        publish(main_bin, host, "v2", scratch)
        v2 = reg_state(host)
        for version in ("v1", "v2"):
            publish(branch_bin, host, version, scratch, ns="p9b")
        packing = diff_packing(host, v2, reg_state(host, "p9b"))
        steps = scenario(f"{host}/p9")
        states = {REGISTRY_V1: v1, REGISTRY_V2: v2}
        runs = [
            run_scenario(
                binary, root, host, steps, lambda s: reg_apply(host, states[s], v2)
            )
            for binary in (main_bin, branch_bin)
        ]
        print(f"p9: {len(runs[0])} checkpoints per binary")
        return packing + compare(*runs)
    finally:
        zot.stop()


# --------------------------------------------------------------------------
# Self-test: the differ must see what it claims to see
# --------------------------------------------------------------------------


def self_test(root: Path) -> None:
    root = root.resolve()
    shutil.rmtree(root, ignore_errors=True)
    (root / "d").mkdir(parents=True)
    (root / "d/f.txt").write_bytes(b"abcdef")
    (root / "d/gone.txt").write_bytes(b"x")
    base = snapshot(root)
    (root / "d/f.txt").write_bytes(b"abcXef")
    (root / "d/gone.txt").unlink()
    moved = snapshot(root)
    found = diff_trees("t", base, moved)
    assert any("d/f.txt differs at byte 3" in m for m in found), (
        found
    )  # (a) one-byte difference
    assert any("d/gone.txt only with main" in m for m in found), (
        found
    )  # (b) missing file
    assert diff_trees("t", base, base) == []
    assert any("d/gone.txt only with branch" in m for m in diff_trees("t", moved, base))

    # The mask hides only the named key, never its neighbours.
    lock = b'generated_at = "2026-01-01T00:00:00Z"\ngenerated_x = "a"\n'
    assert _mask_file("ws/grimoire.lock", lock) == _mask_file(
        "ws/grimoire.lock", lock.replace(b"2026-01-01", b"2027-02-02")
    )
    assert _mask_file("ws/grimoire.lock", lock) != _mask_file(
        "ws/grimoire.lock", lock.replace(b'"a"', b'"b"')
    )
    assert _mask_file("ws/other.toml", lock) == lock

    # Per-command checkpoints: a difference erased by a later command is still reported.
    early = [Checkpoint("#0", 0, "{}", base), Checkpoint("#1", 0, "{}", moved)]
    late = [Checkpoint("#0", 0, "{}", moved), Checkpoint("#1", 0, "{}", moved)]
    assert compare(early, late), "an intermediate difference must be reported"

    # Stdout: only the command's own C-153 key, with its hook-free value.
    row, armed = '{"a": [{"x": 1}]}', '{"a": [{"x": 1, "armed": null}]}'
    assert diff_stdout("t", row, armed, "install") == []
    assert diff_stdout("t", row, armed, "status"), "armed is not a status key"
    assert diff_stdout("t", row, '{"a": [{"x": 1, "armed": []}]}', "status")
    assert diff_stdout("t", row, '{"a": [{"x": 1, "armed": true}]}', "install")
    assert diff_stdout("t", row, '{"a": [{"x": 1, "arming": []}]}', "status") == []
    assert diff_stdout("t", row, '{"a": [{"x": 1, "arming": ["h"]}]}', "status")
    assert diff_stdout("t", '{"a": 1}', '{"a": 1, "extra": 2}', "install")
    assert diff_stdout("t", '{"a": 1, "armed": null}', '{"a": 1}', "install")
    assert diff_stdout("t", '{"a": [{"x": 1, "y": 2}]}', '{"a": [{"x": 1}]}')
    assert diff_stdout("t", '{"a": 1}', '{"a": 2}')

    # c118-relative-config-target: exactly cwd-joined relative == absolute.
    rel, ab = '{"i": [{"target": "ws/x"}]}', '{"i": [{"target": "/r/ws/x"}]}'
    assert diff_stdout("t", rel, ab, "install", "/r") == []
    assert diff_stdout("t", rel, '{"i": [{"target": "/r/ws/y"}]}', "install", "/r")
    assert diff_stdout("t", rel, ab, "install", "/other")
    assert diff_stdout("t", '{"path": "ws/x"}', '{"path": "/r/ws/x"}', "install", "/r")

    # Manifests: only the named namespace annotations are normalized.
    m = {"annotations": {NAMESPACE_ANNOTATIONS[0]: "h/p9/x", "k": "p9"}}
    mb = {"annotations": {NAMESPACE_ANNOTATIONS[0]: "h/p9b/x", "k": "p9"}}
    assert _manifest(json.dumps(m).encode(), "p9") == _manifest(
        json.dumps(mb).encode(), "p9b"
    )
    mb["annotations"]["k"] = "p9b"
    assert _manifest(json.dumps(m).encode(), "p9") != _manifest(
        json.dumps(mb).encode(), "p9b"
    )

    # Hook-free runs gain no warnings; timestamps alone are not a new warning.
    assert (
        _warnings("2026-01-01T00:00:00.1Z  WARN a\n")
        == _warnings("2027-02-02T11:11:11.9Z  WARN a\n")
        == frozenset({"WARN a"})
    )
    warned = Checkpoint("#0", 0, "{}", base, warnings=frozenset({"WARN x"}))
    assert compare([Checkpoint("#0", 0, "{}", base)], [warned])

    for n, (rc, expects, what) in enumerate(
        (
            (3, (), "(c) non-zero exit"),
            (0, ("ws/never.txt",), "(d) missing expected output"),
            (0, ("ws/empty.txt",), "(d) empty expected output"),
        )
    ):
        fake = root / f"fake-grim-{n}"
        body = f": > empty.txt\necho '{{}}'\nexit {rc}\n"
        fake.write_text(f"#!/bin/sh\n{body}", encoding="utf-8")
        fake.chmod(0o755)
        try:
            run_scenario(
                fake,
                root / "run",
                "127.0.0.1:1",
                [Step(("init",), expects)],
                lambda _s: None,
            )
        except ScenarioError:
            pass
        else:
            raise AssertionError(f"self-test: {what} was not detected")
    shutil.rmtree(root, ignore_errors=True)
    print("p9 self-test: ok")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--main-bin", type=Path)
    ap.add_argument("--branch-bin", type=Path)
    ap.add_argument(
        "--root", type=Path, required=True, help="absolute temp root, wiped per run"
    )
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--main-sha", help="the commit --main-bin was built from")
    ap.add_argument(
        "--allow-identical",
        action="store_true",
        help="run although both sides are the same build (baseline only)",
    )
    args = ap.parse_args()
    if args.self_test:
        self_test(args.root)
        return 0
    if not (args.main_bin and args.branch_bin and args.main_sha):
        ap.error("--main-bin, --branch-bin and --main-sha are required")
    print(f"p9: main {args.main_sha} bin sha256 {sha256(args.main_bin)[:16]}")
    print(f"p9: branch bin sha256 {sha256(args.branch_bin)[:16]}")
    same = identical_inputs(args.main_sha, args.main_bin, args.branch_bin)
    if same and not args.allow_identical:
        print(f"p9: refusing a vacuous comparison: {same}", file=sys.stderr)
        return 2
    if same:
        print(f"p9: --allow-identical: {same}")
    try:
        found = differential(
            args.main_bin.resolve(), args.branch_bin.resolve(), args.root
        )
    except ScenarioError as e:
        print(f"p9: scenario failed: {e}", file=sys.stderr)
        return 2
    for line in found:
        print(line)
    print(f"p9: {len(found)} difference(s)")
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
