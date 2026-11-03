#!/usr/bin/env python3
"""Replay the recorded registry bytes of a golden fixture, verbatim.

Reads ``<fixture>/registry/pushed.json`` and re-uploads each artifact
byte-for-byte. Because the manifest bytes are replayed verbatim, every
manifest digest in the golden lock is reproduced exactly, independent of
grim, of the test harness, and of this file's own JSON conventions.

Usage: replay.py <fixture-dir> [registry-host]
"""
from __future__ import annotations

import hashlib
import json
import sys
import urllib.request
from pathlib import Path

MANIFEST_MT = "application/vnd.oci.image.manifest.v1+json"


def sha256(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def push_blob(base: str, repo: str, data: bytes) -> None:
    digest = sha256(data)
    start = urllib.request.Request(
        f"{base}/v2/{repo}/blobs/uploads/", method="POST"
    )
    with urllib.request.urlopen(start) as resp:
        location = resp.headers["Location"]
    if location.startswith("/"):
        location = base + location
    sep = "&" if "?" in location else "?"
    req = urllib.request.Request(
        f"{location}{sep}digest={digest}", data=data, method="PUT"
    )
    req.add_header("Content-Type", "application/octet-stream")
    with urllib.request.urlopen(req):
        pass


def replay(fixture: Path, host: str = "localhost:5000") -> None:
    base = f"http://{host}"
    records = json.loads((fixture / "registry" / "pushed.json").read_text())
    for rec in records:
        repo, tag = rec["repo"], rec["tag"]
        manifest_bytes = rec["manifest"].encode()
        layer = bytes.fromhex(rec["layer"])
        push_blob(base, repo, b"{}")
        push_blob(base, repo, layer)
        req = urllib.request.Request(
            f"{base}/v2/{repo}/manifests/{tag}",
            data=manifest_bytes,
            method="PUT",
        )
        req.add_header("Content-Type", MANIFEST_MT)
        with urllib.request.urlopen(req):
            pass
        got = sha256(manifest_bytes)
        if got != rec["manifest_digest"]:
            raise SystemExit(
                f"{repo}:{tag} digest drift: recorded {rec['manifest_digest']}, replayed {got}"
            )
        print(f"replayed {repo}:{tag} -> {got}")


if __name__ == "__main__":
    replay(
        Path(sys.argv[1]).resolve(),
        sys.argv[2] if len(sys.argv) > 2 else "localhost:5000",
    )
