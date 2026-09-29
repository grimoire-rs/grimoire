from __future__ import annotations

import hashlib
import io
import json
import os
import sys
import tarfile
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

from src.registry import PublishedArtifact, push_artifact

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------

# test/src/helpers.py -> test/src -> test -> project root
PROJECT_ROOT = Path(__file__).resolve().parent.parent.parent


# ---------------------------------------------------------------------------
# Artifact publishing
# ---------------------------------------------------------------------------


def _tar_of(files: dict[str, str | bytes]) -> bytes:
    """Build an uncompressed tar from a ``{path: content}`` mapping.

    Paths are written verbatim and entries are emitted in sorted order so
    the produced bytes (and the resulting manifest digest) are stable.
    """
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as tar:  # no compression
        for path in sorted(files):
            content = files[path]
            data = content.encode() if isinstance(content, str) else content
            info = tarfile.TarInfo(name=path)
            info.size = len(data)
            info.mode = 0o644
            tar.addfile(info, io.BytesIO(data))
    return buf.getvalue()


def write_config(
    project_dir: Path,
    skills: dict[str, str] | None = None,
    rules: dict[str, str] | None = None,
    bundles: dict[str, str] | None = None,
    agents: dict[str, str] | None = None,
) -> Path:
    """Write a ``grimoire.toml`` with the given skill/rule/bundle/agent refs.

    Each value is a fully-qualified ``registry/repo:tag`` (or ``@digest``)
    string, exactly as a user would write it. Returns the config path.
    """
    lines: list[str] = []
    if bundles:
        lines.append("[bundles]")
        for name, ref in bundles.items():
            lines.append(f'{name} = "{ref}"')
    lines.append("[skills]")
    for name, ref in (skills or {}).items():
        lines.append(f'{name} = "{ref}"')
    lines.append("[rules]")
    for name, ref in (rules or {}).items():
        lines.append(f'{name} = "{ref}"')
    lines.append("[agents]")
    for name, ref in (agents or {}).items():
        lines.append(f'{name} = "{ref}"')
    cfg = project_dir / "grimoire.toml"
    cfg.write_text("\n".join(lines) + "\n")
    return cfg


def make_bundle(
    repo: str,
    members: list[tuple[str, str, str]],
    tag: str = "latest",
) -> PublishedArtifact:
    """Build and push a bundle artifact.

    ``members`` is a list of ``(kind, name, id)`` tuples, where ``kind`` is
    ``"skill"`` or ``"rule"``, ``name`` is the binding name, and ``id`` is
    the fully-qualified member reference (floating tag or ``@digest``). The
    bundle's single layer is the JSON members document Grimoire reads on
    expansion.
    """
    doc = {"members": [{"kind": k, "name": n, "id": i} for (k, n, i) in members]}
    layer = json.dumps(doc).encode()
    return push_artifact(repo, tag, layer, "bundle")


def make_description(repo: str, files: dict[str, str | bytes]) -> PublishedArtifact:
    """Build and push a repository description companion at the reserved
    ``__grimoire`` tag.

    Mirrors :func:`make_artifact` but tags the reserved companion tag and
    marks the manifest ``com.grimoire.kind: desc`` (the sole discriminator —
    ``kind_from_manifest`` returns ``None`` for it, so the read path routes it
    to the description path, not an installable kind). ``files`` is the
    companion tree (``README.md``, ``logo.png``, ``CHANGELOG.md``, …). Pushes
    directly to the registry so the fetch read lane does not depend on
    ``grim publish``.
    """
    tar_bytes = _tar_of(files)
    return push_artifact(repo, "__grimoire", tar_bytes, "desc")


def make_artifact(
    repo: str,
    kind: str,
    files: dict[str, str | bytes],
    tag: str = "latest",
    annotations: dict[str, str] | None = None,
) -> PublishedArtifact:
    """Build and push a single-layer OCI skill/rule artifact.

    ``files`` is the artifact tree exactly as the ``DefaultMaterializer``
    expects it: a *skill* is a directory tree rooted at ``<name>/`` (e.g.
    ``{"code-review/SKILL.md": "..."}``); a *rule* is a single
    ``<name>.md`` file (e.g. ``{"rust-style.md": "..."}``). The caller
    constructs the keys; this helper only tars + pushes them, with the kind
    carried by the OCI ``artifactType``.

    Returns the published reference incl. the manifest digest so tests can
    assert ``@sha256`` pins and retag for rolling-release scenarios.
    """
    tar_bytes = _tar_of(files)
    return push_artifact(repo, tag, tar_bytes, kind, annotations)


# ---------------------------------------------------------------------------
# Export plugin trees (shared by the export acceptance suites)
# ---------------------------------------------------------------------------

# Every manifest a rendered plugin carries its `version` in.
VERSIONED_MANIFESTS = (
    ".claude-plugin/plugin.json",
    ".qoder-plugin/plugin.json",
    ".cursor-plugin/plugin.json",
    "plugin.json",
)


def inventory_suffix(inventory: list[tuple[str, bool, str]]) -> str:
    """C-002: 12 hex of SHA-256 over the compact JSON array of `[name, exec, sha256]` triples."""
    blob = json.dumps([list(t) for t in inventory], separators=(",", ":"), ensure_ascii=False).encode()
    return hashlib.sha256(blob).hexdigest()[:12]


def tree_suffix(root: Path, base: str) -> str:
    """C-003: the tree hash of a produced plugin root, every manifest's version put back to `base`."""
    inventory = []
    for path in root.rglob("*"):
        if not path.is_file():
            continue
        rel = path.relative_to(root).as_posix()
        data = path.read_bytes()
        if rel in VERSIONED_MANIFESTS:
            version = json.loads(data)["version"]
            data = data.replace(f'"version": "{version}"'.encode(), f'"version": "{base}"'.encode())
        exec_bit = sys.platform != "win32" and bool(path.stat().st_mode & 0o111)
        inventory.append((rel, exec_bit, hashlib.sha256(data).hexdigest()))
    inventory.sort(key=lambda entry: entry[0].encode())
    return inventory_suffix(inventory)


@contextmanager
def held_flock(sidecar: Path) -> Iterator[None]:
    """Hold grim's advisory lock on ``sidecar`` (same lock space as fs4's flock)."""
    import fcntl

    fd = os.open(sidecar, os.O_RDWR | os.O_CREAT, 0o644)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield
    finally:
        os.close(fd)
        sidecar.unlink(missing_ok=True)
