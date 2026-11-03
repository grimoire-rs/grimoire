#!/usr/bin/env python3
"""Prove the committed golden bytes are reproduced by the 0.15.1 baseline.

Three passes, each in a fresh environment, each compared byte-for-byte:

1. online `lock` + `install` (+ `remove bundle reg-bundle`) against the replayed
   registry;
2. `lock --global --offline` with only `golden/tag-cache/` (manifest blobs);
3. `lock --global --offline` with only `golden/tag-cache-no-manifests/`;
4. the same with an empty cache, which must fail (negative control).

Passes 2 and 3 seed the golden lock first, which is the state an upgrading
user is in: a lock written by 0.15.1 and a warm local cache.

By default the golden lock is seeded so `generated_at` is preserved and no
normalization is needed. `GOLDEN_SEED=0` starts from no lock and blanks the
one time-varying `generated_at` line on both sides.

Usage: GOLDEN_GRIM=<grim-0.15.1> verify.py <fixture-dir> <scratch-dir>
"""
from __future__ import annotations

import difflib
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

GRIM = os.environ["GOLDEN_GRIM"]
REGISTRY_HOST = "localhost:5000"
SEEDED = os.environ.get("GOLDEN_SEED", "1") == "1"


# The project hook payload path embeds the SHA-256 of the workspace path, so
# the project state differs per directory by exactly that component.
WORKSPACE_KEY = re.compile(rb"hooks/payload/[0-9a-f]{64}/")


def norm(b: bytes) -> bytes:
    b = WORKSPACE_KEY.sub(b"hooks/payload/<WORKSPACE-KEY>/", b)
    if SEEDED:
        return b
    return b"\n".join(
        b'generated_at = "<NORMALIZED>"' if line.startswith(b"generated_at =") else line
        for line in b.split(b"\n")
    )


def compare(label: str, want: Path, got: Path) -> bool:
    a, b = norm(want.read_bytes()), norm(got.read_bytes())
    ok = a == b
    print(f"{'IDENTICAL' if ok else 'DIFFERS  '}  {label}")
    if not ok:
        for line in difflib.unified_diff(
            a.decode().splitlines(), b.decode().splitlines(), "golden", "actual",
            lineterm="", n=1,
        ):
            print("   " + line)
    return ok


class Env:
    """A fresh HOME, GRIM_HOME and project seeded from the fixture."""

    def __init__(self, fixture: Path, work: Path, seed_locks: bool):
        if work.exists():
            shutil.rmtree(work)
        self.home = work / "home"
        self.grim_home = work / "grim-home"
        self.project = work / "project"
        for d in (self.home, self.grim_home):
            d.mkdir(parents=True)
        shutil.copytree(fixture / "project", self.project)
        shutil.copyfile(fixture / "global-grimoire.toml", self.grim_home / "grimoire.toml")
        for d in (".claude", ".copilot", ".codex"):
            (self.home / d).mkdir(parents=True, exist_ok=True)
        g = fixture / "golden"
        if seed_locks:
            shutil.copyfile(g / "project.grimoire.lock", self.project / "grimoire.lock")
            shutil.copyfile(g / "global.grimoire.lock", self.grim_home / "grimoire.lock")
        self.env = {
            "PATH": "/usr/bin:/bin",
            "HOME": str(self.home),
            "GRIM_HOME": str(self.grim_home),
            "XDG_CONFIG_HOME": str(self.home / ".config"),
            "GRIM_INSECURE_REGISTRIES": REGISTRY_HOST,
            "NO_COLOR": "1",
        }

    def run(self, args: list[str], cwd: Path) -> None:
        p = subprocess.run([GRIM, *args], cwd=cwd, env=self.env, capture_output=True, text=True)
        if p.returncode != 0:
            print(f"grim {' '.join(args)} -> {p.returncode}\n{p.stderr}")
            sys.exit(1)


def decl_hash(lock: Path) -> str:
    return next(
        l.split("=", 1)[1].strip().strip('"')
        for l in lock.read_text().splitlines()
        if l.startswith("declaration_hash =")
    )


def online_pass(fixture: Path, work: Path) -> bool:
    print("-- online lock + install, then remove bundle")
    g = fixture / "golden"
    e = Env(fixture, work / "online", SEEDED)
    e.run(["lock"], e.project)
    e.run(["install", "--client", "claude", "--trust-hooks"], e.project)
    e.run(["lock", "--global"], e.grim_home)
    e.run(["install", "--global", "--client", "claude"], e.grim_home)
    ok = True
    for name, actual in (
        ("project.grimoire.lock", e.project / "grimoire.lock"),
        ("global.grimoire.lock", e.grim_home / "grimoire.lock"),
        ("state.global.json", e.grim_home / "state" / "global.json"),
        ("state.project.json", e.project / ".grimoire" / "state.json"),
    ):
        ok &= compare(name, g / name, actual)
    import json

    want = json.loads((g / "declaration_hash.json").read_text())
    for scope, path in (("project", e.project / "grimoire.lock"), ("global", e.grim_home / "grimoire.lock")):
        same = decl_hash(path) == want[scope]
        print(f"{'IDENTICAL' if same else 'DIFFERS  '}  declaration_hash[{scope}]")
        ok &= same
    e.run(["remove", "bundle", "reg-bundle"], e.project)
    ok &= compare("project.after-remove.grimoire.lock", g / "project.after-remove.grimoire.lock", e.project / "grimoire.lock")
    ok &= compare("state.project.after-remove.json", g / "state.project.after-remove.json", e.project / ".grimoire" / "state.json")
    return ok


def offline_pass(fixture: Path, work: Path, variant: str | None) -> bool:
    """Lock the global scope offline. `variant=None` is the negative control:
    an empty cache must fail, which proves the variants are what carries it."""
    label = "empty cache (must fail)" if variant is None else f"only golden/{variant}/"
    print(f"-- lock --global --offline with {label}")
    g = fixture / "golden"
    e = Env(fixture, work / (variant or "empty"), True)
    if variant is not None:
        for registry_dir in sorted((g / variant / "tags").iterdir()):
            # The fixture stores `host_port`; Unix grim reads `host:port`.
            name = registry_dir.name if sys.platform == "win32" else registry_dir.name.replace("_", ":", 1)
            shutil.copytree(registry_dir, e.grim_home / "tags" / name)
        if (g / variant / "blobs").is_dir():
            shutil.copytree(g / variant / "blobs", e.grim_home / "blobs")
    p = subprocess.run([GRIM, "lock", "--global", "--offline"], cwd=e.grim_home, env=e.env, capture_output=True, text=True)
    if variant is None:
        ok = p.returncode != 0
        print(f"{'FAILS AS EXPECTED' if ok else 'UNEXPECTEDLY LOCKED'}  exit {p.returncode}")
        return ok
    if p.returncode != 0:
        print(f"grim lock --global --offline -> {p.returncode}\n{p.stderr}")
        return False
    return compare("global.grimoire.lock", g / "global.grimoire.lock", e.grim_home / "grimoire.lock")


def main(fixture: Path, work: Path) -> int:
    ok = online_pass(fixture, work)
    ok &= offline_pass(fixture, work, "tag-cache")
    ok &= offline_pass(fixture, work, "tag-cache-no-manifests")
    ok &= offline_pass(fixture, work, None)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve()))
