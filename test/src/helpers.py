from __future__ import annotations

import io
import json
import tarfile
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
    hooks: dict[str, str] | None = None,
    options: dict[str, str] | None = None,
) -> Path:
    """Write a ``grimoire.toml`` with the given skill/rule/bundle/agent/hook refs.

    Each value is a fully-qualified ``registry/repo:tag`` (or ``@digest``)
    string, exactly as a user would write it. Returns the config path.

    ``[hooks]`` is emitted **only when non-empty**, and last — matching what
    ``grim`` itself writes (``command::add::write_config``). An always-present
    empty ``[hooks]`` table would change the declaration hash of every existing
    fixture, so the emptiness check is load-bearing, not cosmetic.

    ``options`` writes raw ``key = value`` lines into an ``[options]`` table
    (values are passed through verbatim, so a string needs its own quotes).
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
    if hooks:
        lines.append("[hooks]")
        for name, ref in hooks.items():
            lines.append(f'{name} = "{ref}"')
    if options:
        lines.append("[options]")
        for key, value in options.items():
            lines.append(f"{key} = {value}")
    cfg = project_dir / "grimoire.toml"
    cfg.write_text("\n".join(lines) + "\n")
    return cfg


def make_bundle(
    repo: str,
    members: list[tuple[str, str, str]],
    tag: str = "latest",
) -> PublishedArtifact:
    """Build and push a bundle artifact.

    ``members`` is a list of ``(kind, name, id)`` tuples, where ``kind`` is any
    installable kind — ``"skill"``, ``"rule"``, ``"agent"``, ``"mcp"``, or
    ``"hook"`` (never ``"bundle"``; grim rejects nesting at expansion) — ``name``
    is the binding name, and ``id`` is the fully-qualified member reference
    (floating tag or ``@digest``). The bundle's single layer is the JSON members
    document Grimoire reads on expansion.

    Members are written in the caller's order, deliberately: grim sorts them by
    ``(kind, name)`` when it authors a bundle, so passing an unsorted list here
    exercises the read path against a hand-authored layer rather than only
    against grim's own output.
    """
    doc = {"members": [{"kind": k, "name": n, "id": i} for (k, n, i) in members]}
    layer = json.dumps(doc).encode()
    return push_artifact(repo, tag, layer, "bundle")


def make_hook(
    repo: str,
    name: str,
    tag: str = "1.0.0",
    *,
    event: str = "PreToolUse",
    tier: str = "gatekeeper",
    matcher: str = "Bash",
) -> PublishedArtifact:
    """Build and push a minimal valid ``hook`` artifact.

    A hook is a **directory** artifact (``is_dir_artifact()``): the tar is rooted
    at ``<name>/`` and carries ``hook.toml`` plus the payload the handler runs.
    The payload here is inert (``exit 0``) — these tests assert declaration,
    locking, and provenance, never that a handler fired, so a payload with any
    observable effect would only muddy a failure.

    Note the manifest ``name`` must equal the directory stem or ``grim build``
    fails (65); the tar root and the ``name`` field are therefore both ``name``.
    """
    manifest = (
        "schema      = 1\n"
        f'name        = "{name}"\n'
        f'description = "test hook {name}"\n'
        "\n"
        "[[hooks]]\n"
        f'id      = "{name}-handler"\n'
        f'event   = "{event}"\n'
        f'tier    = "{tier}"\n'
        f'matcher = "{matcher}"\n'
        'argv    = ["sh", "${GRIM_HOOK_DIR}/handler.sh"]\n'
        "timeout = 30\n"
        'payload = "stdin"\n'
    )
    tar_bytes = _tar_of(
        {
            f"{name}/hook.toml": manifest,
            f"{name}/handler.sh": "#!/bin/sh\nexit 0\n",
        }
    )
    return push_artifact(repo, tag, tar_bytes, "hook")


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
