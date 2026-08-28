#!/usr/bin/env python3
"""Prove the zero-normalization consumption strategy.

Seed the committed golden lock into a fresh project, run `grim lock` +
`grim install`, and compare every golden artifact byte-for-byte. If
`generated_at` preservation works as documented, no normalization is needed.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

GRIM = os.environ["GOLDEN_GRIM"]
REGISTRY_HOST = "localhost:5000"


def main(fixture: Path, work: Path) -> int:
    if work.exists():
        shutil.rmtree(work)
    home = work / "home"
    grim_home = work / "grim-home"
    project = work / "project"
    for d in (home, grim_home):
        d.mkdir(parents=True)
    shutil.copytree(fixture / "project", project)
    shutil.copyfile(
        fixture / "global-grimoire.toml", grim_home / "grimoire.toml"
    )
    for d in (".claude", ".copilot", ".codex"):
        (home / d).mkdir(parents=True, exist_ok=True)

    g = fixture / "golden"
    seeded = os.environ.get("GOLDEN_SEED", "1") == "1"
    if seeded:
        # Seed both golden locks so the `generated_at` preservation path fires.
        shutil.copyfile(g / "project.grimoire.lock", project / "grimoire.lock")
        shutil.copyfile(g / "global.grimoire.lock", grim_home / "grimoire.lock")

    env = {
        "PATH": "/usr/bin:/bin",
        "HOME": str(home),
        "GRIM_HOME": str(grim_home),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "GRIM_INSECURE_REGISTRIES": REGISTRY_HOST,
        "NO_COLOR": "1",
    }

    def run(args, cwd):
        p = subprocess.run(
            [GRIM, *args], cwd=cwd, env=env, capture_output=True, text=True
        )
        if p.returncode != 0:
            print(f"grim {' '.join(args)} -> {p.returncode}\n{p.stderr}")
            sys.exit(1)
        return p

    run(["lock"], project)
    run(["install", "--client", "claude"], project)
    run(["lock", "--global"], grim_home)
    run(["install", "--global", "--client", "claude"], grim_home)

    checks = [
        ("project.grimoire.lock", project / "grimoire.lock"),
        ("global.grimoire.lock", grim_home / "grimoire.lock"),
        ("state.global.json", grim_home / "state" / "global.json"),
        ("state.project.json", project / ".grimoire" / "state.json"),
    ]
    def norm(b: bytes) -> bytes:
        """Unseeded mode: blank the one time-varying lock field."""
        if seeded:
            return b
        return b"\n".join(
            b'generated_at = "<NORMALIZED>"'
            if line.startswith(b"generated_at =")
            else line
            for line in b.split(b"\n")
        )

    rc = 0
    for name, actual in checks:
        want = norm((g / name).read_bytes())
        got = norm(actual.read_bytes())
        ok = want == got
        print(f"{'IDENTICAL' if ok else 'DIFFERS  '}  {name}")
        if not ok:
            rc = 1
            import difflib

            for line in difflib.unified_diff(
                want.decode().splitlines(),
                got.decode().splitlines(),
                "golden",
                "actual",
                lineterm="",
                n=1,
            ):
                print("   " + line)

    # declaration_hash, independently
    want_hash = json.loads((g / "declaration_hash.json").read_text())
    for scope, path in (
        ("project", project / "grimoire.lock"),
        ("global", grim_home / "grimoire.lock"),
    ):
        got = next(
            l.split("=", 1)[1].strip().strip('"')
            for l in path.read_text().splitlines()
            if l.startswith("declaration_hash =")
        )
        ok = got == want_hash[scope]
        print(f"{'IDENTICAL' if ok else 'DIFFERS  '}  declaration_hash[{scope}]")
        if not ok:
            rc = 1
            print(f"   golden={want_hash[scope]} actual={got}")
    return rc


if __name__ == "__main__":
    sys.exit(main(Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve()))
