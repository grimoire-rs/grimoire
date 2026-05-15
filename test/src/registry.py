# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Minimal OCI registry client for the acceptance suite.

The suite pushes single-layer OCI image artifacts to a local
``registry:2`` on ``localhost:5000`` over plain HTTP using only the
standard library (no extra test dependency). This mirrors what a real
publisher does: a config blob, one uncompressed-tar layer blob, and an
image manifest carrying the ``com.grimoire.kind`` annotation.
"""
from __future__ import annotations

import hashlib
import json
import urllib.error
import urllib.request
from dataclasses import dataclass

REGISTRY_HOST = "localhost:5000"
REGISTRY_BASE = f"http://{REGISTRY_HOST}"

_MANIFEST_MEDIA_TYPE = "application/vnd.oci.image.manifest.v1+json"
_CONFIG_MEDIA_TYPE = "application/vnd.oci.image.config.v1+json"
_LAYER_MEDIA_TYPE = "application/vnd.grimoire.artifact.layer.v1.tar"


def _sha256(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def registry_reachable(timeout: float = 2.0) -> bool:
    """Whether the ``/v2/`` API endpoint answers."""
    try:
        with urllib.request.urlopen(f"{REGISTRY_BASE}/v2/", timeout=timeout) as resp:
            return resp.status in (200, 401)
    except (urllib.error.URLError, OSError):
        return False


def _put(url: str, data: bytes, content_type: str) -> str:
    req = urllib.request.Request(url, data=data, method="PUT")
    req.add_header("Content-Type", content_type)
    with urllib.request.urlopen(req) as resp:
        return resp.headers.get("Docker-Content-Digest", "")


def _push_blob(repo: str, data: bytes) -> str:
    """Upload ``data`` as a blob via the two-step monolithic upload."""
    digest = _sha256(data)
    start = urllib.request.Request(
        f"{REGISTRY_BASE}/v2/{repo}/blobs/uploads/", method="POST"
    )
    with urllib.request.urlopen(start) as resp:
        location = resp.headers["Location"]
    if location.startswith("/"):
        location = REGISTRY_BASE + location
    sep = "&" if "?" in location else "?"
    put_url = f"{location}{sep}digest={digest}"
    req = urllib.request.Request(put_url, data=data, method="PUT")
    req.add_header("Content-Type", "application/octet-stream")
    with urllib.request.urlopen(req):
        pass
    return digest


@dataclass(frozen=True)
class PublishedArtifact:
    """A skill or rule pushed to the test registry."""

    repo: str
    tag: str
    digest: str
    kind: str

    @property
    def fq(self) -> str:
        """Fully-qualified ``registry/repo:tag`` reference."""
        return f"{REGISTRY_HOST}/{self.repo}:{self.tag}"

    @property
    def pinned(self) -> str:
        """Fully-qualified ``registry/repo@digest`` reference."""
        return f"{REGISTRY_HOST}/{self.repo}@{self.digest}"


def push_artifact(
    repo: str,
    tag: str,
    tar_bytes: bytes,
    kind: str,
    annotations: dict[str, str] | None = None,
) -> PublishedArtifact:
    """Push a single-layer OCI artifact and tag it.

    ``tar_bytes`` is the uncompressed artifact tar the materializer
    expects. The manifest carries ``com.grimoire.kind`` (plus any extra
    ``annotations``). Returns the published reference incl. the manifest
    digest, so callers can assert ``@sha256`` pins.
    """
    config_blob = json.dumps({"grimoire": {"kind": kind}}).encode()
    config_digest = _push_blob(repo, config_blob)
    layer_digest = _push_blob(repo, tar_bytes)

    manifest_annotations = {"com.grimoire.kind": kind}
    if annotations:
        manifest_annotations.update(annotations)

    manifest = {
        "schemaVersion": 2,
        "mediaType": _MANIFEST_MEDIA_TYPE,
        "config": {
            "mediaType": _CONFIG_MEDIA_TYPE,
            "digest": config_digest,
            "size": len(config_blob),
        },
        "layers": [
            {
                "mediaType": _LAYER_MEDIA_TYPE,
                "digest": layer_digest,
                "size": len(tar_bytes),
            }
        ],
        "annotations": manifest_annotations,
    }
    manifest_bytes = json.dumps(manifest).encode()
    manifest_digest = _sha256(manifest_bytes)
    _put(
        f"{REGISTRY_BASE}/v2/{repo}/manifests/{tag}",
        manifest_bytes,
        _MANIFEST_MEDIA_TYPE,
    )
    return PublishedArtifact(
        repo=repo, tag=tag, digest=manifest_digest, kind=kind
    )


def tag_digest(repo: str, tag: str) -> str:
    """Return the manifest digest a ``tag`` currently resolves to.

    Issues a manifest ``GET`` (``HEAD`` is not universally enabled on
    ``registry:2``) and reads the ``Docker-Content-Digest`` header — the
    authoritative answer to "what does this floating tag point at right
    now", used to assert a rolling release actually moved the cascade.
    """
    req = urllib.request.Request(
        f"{REGISTRY_BASE}/v2/{repo}/manifests/{tag}",
        headers={"Accept": _MANIFEST_MEDIA_TYPE},
    )
    with urllib.request.urlopen(req) as resp:
        header = resp.headers.get("Docker-Content-Digest")
        if header:
            return header
        return _sha256(resp.read())


def retag(repo: str, tag: str, target_digest: str) -> PublishedArtifact:
    """Re-point ``tag`` at an existing manifest ``target_digest``.

    Models a rolling release: the floating tag is moved to a manifest
    that is already in the registry. Returns the new published ref.
    """
    with urllib.request.urlopen(
        urllib.request.Request(
            f"{REGISTRY_BASE}/v2/{repo}/manifests/{target_digest}",
            headers={"Accept": _MANIFEST_MEDIA_TYPE},
        )
    ) as resp:
        manifest_bytes = resp.read()
    _put(
        f"{REGISTRY_BASE}/v2/{repo}/manifests/{tag}",
        manifest_bytes,
        _MANIFEST_MEDIA_TYPE,
    )
    manifest = json.loads(manifest_bytes)
    kind = manifest.get("annotations", {}).get("com.grimoire.kind", "skill")
    return PublishedArtifact(
        repo=repo, tag=tag, digest=_sha256(manifest_bytes), kind=kind
    )
