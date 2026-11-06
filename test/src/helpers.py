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

from src.registry import (
    GRIMOIRE_GZIP_LAYER_MEDIA_TYPE,
    LAYER_MEDIA_TYPE,
    PublishedArtifact,
    push_artifact,
    push_gzip_artifact,
    push_manifest,
)

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------

# test/src/helpers.py -> test/src -> test -> project root
PROJECT_ROOT = Path(__file__).resolve().parent.parent.parent


# ---------------------------------------------------------------------------
# Artifact publishing
# ---------------------------------------------------------------------------


def tar_of(files: dict[str, str | bytes]) -> bytes:
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
    settings: dict[str, str] | None = None,
    experimental_settings: bool | None = None,
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

    ``settings`` is emitted like ``hooks``: only when non-empty, after
    ``[hooks]``, so a settings-free fixture keeps its declaration hash.
    ``experimental_settings`` writes ``settings = <bool>`` under
    ``[options.experimental]``; ``None`` writes nothing, which is the
    flag-unset state.
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
    if settings:
        lines.append("[settings]")
        for name, ref in settings.items():
            lines.append(f'{name} = "{ref}"')
    if options:
        lines.append("[options]")
        for key, value in options.items():
            lines.append(f"{key} = {value}")
    if experimental_settings is not None:
        lines.append("[options.experimental]")
        lines.append(f"settings = {str(experimental_settings).lower()}")
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


# The requires layer reuses the bundle members layer type
# (`BUNDLE_LAYER_MEDIA_TYPE`, src/oci/bundle.rs).
_REQUIRES_LAYER_MEDIA_TYPE = "application/vnd.grimoire.bundle.v1+json"
_MCP_LAYER_MEDIA_TYPE = "application/vnd.grimoire.mcp.v1+json"
# `ArtifactKind` declaration order (src/oci/artifact_kind.rs): grim sorts members
# by the enum, not by the kind's text; a text sort puts mcp before skill and
# changes the layer digest.
_KIND_ORDER = ("skill", "rule", "agent", "bundle", "mcp", "hook", "settings")


def members_layer(members: list[tuple[str, str, str]]) -> bytes:
    """The canonical members document: ``BundleManifest::new`` + ``to_layer_bytes``.

    Sorted by ``(kind, name)``, pretty JSON (2-space indent, struct field order
    ``kind``/``name``/``id``, non-ASCII unescaped like serde_json), trailing
    newline (src/oci/bundle.rs).
    """
    doc = {
        "members": [
            {"kind": k, "name": n, "id": i}
            for (k, n, i) in sorted(
                members, key=lambda m: (_KIND_ORDER.index(m[0]), m[1])
            )
        ]
    }
    return (json.dumps(doc, indent=2, ensure_ascii=False) + "\n").encode()


def make_dependent(
    repo: str,
    tag: str,
    kind: str,
    content: dict[str, str | bytes] | bytes,
    requires: list[tuple[str, str, str]],
    *,
    annotations: dict[str, str] | None = None,
) -> PublishedArtifact:
    """Push a dependent artifact: the kind's content layer, then a requires layer.

    ``content`` is the file tree for a tar kind (skill, rule, agent, hook) or the
    raw layer bytes (an MCP descriptor document). ``requires`` is a list of
    ``(kind, name, id)`` tuples, written in canonical order. The manifest has
    grim's own shape: empty config, no ``artifactType``, kind via the
    ``com.grimoire.kind`` annotation, no layer annotations.
    """
    blob = content if isinstance(content, bytes) else tar_of(content)
    content_type = _MCP_LAYER_MEDIA_TYPE if kind == "mcp" else LAYER_MEDIA_TYPE
    digest = push_manifest(
        repo,
        tag,
        [(content_type, blob), (_REQUIRES_LAYER_MEDIA_TYPE, members_layer(requires))],
        {"com.grimoire.kind": kind, **(annotations or {})},
    )
    return PublishedArtifact(repo=repo, tag=tag, digest=digest, kind=kind)


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
    tar_bytes = tar_of(
        {
            f"{name}/hook.toml": manifest,
            f"{name}/handler.sh": "#!/bin/sh\nexit 0\n",
        }
    )
    return push_artifact(repo, tag, tar_bytes, "hook")


# The settings layer type and kind annotation, as `src/oci/settings.rs` and
# `ArtifactKind::Settings` declare them.
SETTINGS_LAYER_MEDIA_TYPE = "application/vnd.grimoire.settings.v1+json"


def settings_layer(document: dict) -> bytes:
    """The canonical settings layer: pretty JSON, sorted keys, trailing newline."""
    return (json.dumps(document, indent=2, sort_keys=True, ensure_ascii=False) + "\n").encode()


def make_settings(
    repo: str,
    tag: str,
    document: dict,
    requires: list[tuple[str, str, str]] | None = None,
) -> PublishedArtifact:
    """Push a ``settings`` artifact straight through ``push_manifest``.

    ``document`` is the layer's JSON object: ``description`` plus one table per
    vendor (``{"description": "...", "claude": {"model": "sonnet"}}``). It is
    serialised as given, with no validation, so a test can publish a document
    the consumer must refuse. Never built by grim: a broken writer or builder
    cannot mask a broken reader. ``requires`` appends a requires layer.
    """
    layers = [(SETTINGS_LAYER_MEDIA_TYPE, settings_layer(document))]
    if requires:
        layers.append((_REQUIRES_LAYER_MEDIA_TYPE, members_layer(requires)))
    digest = push_manifest(repo, tag, layers, {"com.grimoire.kind": "settings"})
    return PublishedArtifact(repo=repo, tag=tag, digest=digest, kind="settings")


# Vendor config-dir variables for the settings clients that have one, each
# mapped to the root (relative to the sandbox) it is redirected to.
_SETTINGS_ROOTS = {
    "claude": ("CLAUDE_CONFIG_DIR", "claude"),
    "codex": ("CODEX_HOME", "codex"),
    "copilot": ("COPILOT_HOME", "copilot"),
    "kiro": ("KIRO_HOME", "kiro"),
    "opencode": ("OPENCODE_CONFIG_DIR", "opencode"),
    "qoder": ("QODER_CONFIG_DIR", "qoder"),
}


def settings_sandbox(base: Path, home: Path | None = None) -> tuple[dict[str, str], dict[str, Path]]:
    """Seed harness roots for every settings client under ``base`` and ``home``.

    Returns ``(env, roots)``: ``env`` holds the vendor variables to merge into
    a ``GrimRunner.env``; ``roots`` maps each client to its seeded directory.
    Clients with no variable root under ``home``, which must be the runner's
    isolated home: Cursor at ``.cursor`` (``CURSOR_CONFIG_DIR`` moves only its
    ``cli-config.json``), Amp at ``.config/amp`` (the runner's XDG dir),
    Antigravity at its CLI's ``.gemini/antigravity-cli``, Droid at
    ``.factory``, Kilo at ``.config/kilo``, Gemini at ``.gemini`` (no
    ``GEMINI_CLI_HOME``, which would move the home itself) and Warp at
    ``.warp`` on macOS, else ``.config/warp-terminal``. Every root exists, so a
    global-scope run detects the client, and nothing is written outside
    ``base`` and ``home``.
    """
    env: dict[str, str] = {}
    roots: dict[str, Path] = {}
    for client, (variable, name) in _SETTINGS_ROOTS.items():
        root = base / name
        root.mkdir(parents=True, exist_ok=True)
        env[variable] = str(root)
        roots[client] = root
    home = home if home is not None else base / "home"
    warp = (".warp",) if sys.platform == "darwin" else (".config", "warp-terminal")
    for client, relative in (
        ("cursor", (".cursor",)),
        ("amp", (".config", "amp")),
        ("antigravity", (".gemini", "antigravity-cli")),
        ("droid", (".factory",)),
        ("kilo", (".config", "kilo")),
        ("gemini", (".gemini",)),
        ("warp", warp),
    ):
        root = home.joinpath(*relative)
        root.mkdir(parents=True, exist_ok=True)
        roots[client] = root
    # Warp is detected by `~/.warp`, which holds its settings on macOS only.
    (home / ".warp").mkdir(exist_ok=True)
    return env, roots


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
    tar_bytes = tar_of(files)
    return push_artifact(repo, "__grimoire", tar_bytes, "desc")


def make_artifact(
    repo: str,
    kind: str,
    files: dict[str, str | bytes],
    tag: str = "latest",
    annotations: dict[str, str] | None = None,
    layer_media_type: str = LAYER_MEDIA_TYPE,
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
    tar_bytes = tar_of(files)
    return push_artifact(
        repo, tag, tar_bytes, kind, annotations, layer_media_type=layer_media_type
    )


def make_gzip_artifact(
    repo: str,
    kind: str,
    files: dict[str, str | bytes],
    tag: str = "latest",
    *,
    media_type: str = GRIMOIRE_GZIP_LAYER_MEDIA_TYPE,
    annotate: bool = True,
    uncompressed_digest: str | None = None,
    blob: bytes | None = None,
) -> PublishedArtifact:
    """:func:`make_artifact` with a gzip-compressed layer.

    The options pass through to :func:`src.registry.push_gzip_artifact`.
    """
    return push_gzip_artifact(
        repo,
        tag,
        tar_of(files),
        kind,
        media_type=media_type,
        annotate=annotate,
        uncompressed_digest=uncompressed_digest,
        blob=blob,
    )


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
    blob = json.dumps(
        [list(t) for t in inventory], separators=(",", ":"), ensure_ascii=False
    ).encode()
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
            data = data.replace(
                f'"version": "{version}"'.encode(), f'"version": "{base}"'.encode()
            )
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


def make_repos(
    namespace: str, count: int, description: str = "Generic helper"
) -> list[str]:
    """Push ``count`` single-skill repos ``<namespace>/pkg-000``… and return
    their repository paths (no registry host), in listing order.

    One tar is shared by every repo; each push still uploads its own blobs, as
    a registry scopes blobs per repository. ~4 ms a repo against zot.
    """
    tar_bytes = tar_of({"pkg/SKILL.md": "---\nname: pkg\n---\n# P\n"})
    annotations = {"org.opencontainers.image.description": description}
    repos = [f"{namespace}/pkg-{i:03d}" for i in range(count)]
    for repo in repos:
        push_artifact(repo, "latest", tar_bytes, "skill", annotations)
    return repos


def paged_namespace(count: int) -> str:
    """A namespace of ``count`` repos that every test and xdist worker shares.

    The name depends only on ``count`` and ``make_repos`` is deterministic, so
    concurrent pushes from several workers write the same repos again instead
    of adding their own copies. A per-worker copy grows the shared registry
    needlessly and slows every ``_catalog`` walk in the session.
    """
    namespace = f"grim-test/paged-{count}"
    make_repos(namespace, count)
    return namespace
