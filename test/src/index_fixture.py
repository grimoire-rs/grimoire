# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Builders for package-index (`[[registries]] index = …`) acceptance tests.

Plain helpers only; the ``http_index`` fixture that serves them lives in
``tests/conftest.py`` and delegates to ``serve``. ``IndexHandler`` is the
scriptable server for tests that need an ETag, a request log or injected
faults; ``write_root_json`` writes the anchored ``c/root.json`` and the ``c/f``
blobs it names (``.agents/specs/design_catalog_at_scale.md``, published files).
"""

from __future__ import annotations

import gzip
import hashlib
import json
import shutil
import threading
import time
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass, field
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Literal
from urllib.parse import urlsplit


def package(
    name: str,
    kind: str,
    ref: str,
    description: str,
    keywords: list[str] | None = None,
    summary: str | None = None,
    deprecated: str | None = None,
    replaced_by: str | None = None,
) -> dict:
    pkg = {
        "schema": 1,
        "name": name,
        "kind": kind,
        "ref": ref,
        "description": description,
        "repository": "https://github.com/acme/skills",
        "owner": {"github": "acme", "id": 1},
    }
    # Omit-empty, mirroring the announce writer: pre-search-metadata
    # pointers carry neither key.
    if keywords:
        pkg["keywords"] = keywords
    if summary is not None:
        pkg["summary"] = summary
    if deprecated is not None:
        pkg["deprecated"] = deprecated
    if replaced_by is not None:
        pkg["replaced_by"] = replaced_by
    return pkg


def write_all_json(root: Path, packages: list[dict]) -> None:
    (root / "all.json").write_text(json.dumps(packages))


def index_config(project_dir: Path, locator: str) -> None:
    # `locator` goes into a TOML basic string, so callers passing a local
    # path must forward-slash it (`Path.as_posix()`): a Windows `str(path)`
    # embeds backslashes, which are invalid TOML escapes. git clones a
    # forward-slash path fine on every platform.
    (project_dir / "grimoire.toml").write_text(
        f"[[registries]]\n"
        f'alias = "hub"\n'
        f'index = "{locator}"\n'
        f"default = true\n"
        f"\n[skills]\n\n[rules]\n"
    )


def filtered_index_config(
    project_dir: Path,
    locator: str,
    include: tuple[str, ...] = (),
    exclude: tuple[str, ...] = (),
) -> None:
    """`index_config` plus a browse filter on the same single entry."""
    lines = [
        "[[registries]]",
        'alias = "hub"',
        f'index = "{locator}"',
    ]
    for key, patterns in (("include", include), ("exclude", exclude)):
        if patterns:
            lines.append(f"{key} = [" + ", ".join(f'"{p}"' for p in patterns) + "]")
    lines += ["default = true", "", "[skills]", "", "[rules]", ""]
    (project_dir / "grimoire.toml").write_text("\n".join(lines))


def write_stats(
    root: Path,
    entries: dict,
    schema_version: int = 1,
    rating_host: str | None = None,
    downloads_provider: str | None = None,
) -> None:
    providers: dict = {"rating": "github"}
    if rating_host is not None:
        providers["rating_host"] = rating_host
    if downloads_provider is not None:
        providers["downloads"] = downloads_provider
    (root / "stats.json").write_text(
        json.dumps(
            {
                "schema_version": schema_version,
                "generated_at": "2026-08-18T00:00:00Z",
                "providers": providers,
                "entries": entries,
            }
        )
    )


def search_envelope(runner, *args: str) -> dict[str, Any]:
    """``grim --format json search --refresh <args>``, exit 0 asserted;
    returns the whole envelope (``items``, ``sources``, ``total``, …)."""
    result = runner.run("--format", "json", "search", "--refresh", *args, check=False)
    assert result.returncode == 0, (
        f"index-backed search must exit 0, got {result.returncode}; stderr: {result.stderr}"
    )
    return json.loads(result.stdout)


@contextmanager
def serve(root: Path, handler_cls: type[SimpleHTTPRequestHandler]) -> Iterator[str]:
    """Serve ``root`` through ``handler_cls``; yields the ``http://`` base URL."""
    server = ThreadingHTTPServer(
        ("127.0.0.1", 0), partial(handler_cls, directory=str(root))
    )
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield f"http://127.0.0.1:{server.server_address[1]}"
    finally:
        server.shutdown()
        server.server_close()


@dataclass(slots=True)
class Request:
    """One request an ``IndexHandler`` served. ``path`` is the raw request
    target, query included; ``headers`` has lower-cased names; ``length`` is
    the response body bytes the handler sent (0 for a HEAD, a bodiless status
    or a 404), filled in once the response is decided."""

    method: str
    path: str
    headers: dict[str, str]
    length: int = 0


@dataclass(frozen=True, slots=True)
class Fault:
    """What to do to a URL path instead of, or on top of, serving its file.

    ``status`` (with an empty body unless ``body`` is given) replaces the
    response; left ``None`` the file (or ``body``) is served as a 200.
    ``headers`` replace same-named defaults, ``Content-Length`` included, so a
    short body under a long declared length is a truncated response.
    ``gzipped`` gzip-encodes the body and sends ``Content-Encoding: gzip``.
    ``stall_before`` sleeps before the response starts; ``stall_mid`` sends
    half the body, then sleeps before the rest. A fault keyed ``"*"`` in
    ``IndexHandler.faults`` answers every path without a fault of its own.
    """

    status: int | None = None
    headers: dict[str, str] = field(default_factory=dict)
    body: bytes | None = None
    gzipped: bool = False
    stall_before: float = 0.0
    stall_mid: float = 0.0


_NO_FAULT = Fault()


def _content_type(path: str) -> str:
    if path.endswith(".gz"):
        return "application/gzip"
    if path.endswith(".json"):
        return "application/json"
    return "application/octet-stream"


class IndexHandler(SimpleHTTPRequestHandler):
    """Serves the ``serve`` root with a strong ETag per file (never honoured as
    a condition), a request log and a fault table keyed by URL path.

    Build the handler class per test with ``IndexHandler.new()`` and hand it to
    ``serve``: ``log`` and ``faults`` are class-level state, so a shared class
    would leak one test's requests and faults into the next. Edit ``faults``
    while serving to change behaviour between refreshes.
    """

    log: list[Request]
    faults: dict[str, Fault]

    @classmethod
    def new(cls, faults: dict[str, Fault] | None = None) -> type[IndexHandler]:
        return type(cls.__name__, (cls,), {"log": [], "faults": dict(faults or {})})

    def do_GET(self) -> None:
        self._answer(head=False)

    def do_HEAD(self) -> None:
        self._answer(head=True)

    def log_message(self, format: str, *args: Any) -> None:
        pass

    def _read(self, path: str) -> bytes | None:
        file = Path(self.translate_path(path))
        return file.read_bytes() if file.is_file() else None

    def _answer(self, *, head: bool) -> None:
        path = urlsplit(self.path).path
        request = Request(
            self.command, self.path, {k.lower(): v for k, v in self.headers.items()}
        )
        self.log.append(request)
        fault = self.faults.get(path, self.faults.get("*", _NO_FAULT))
        time.sleep(fault.stall_before)

        if fault.status is None:
            body = fault.body if fault.body is not None else self._read(path)
            if body is None:
                self.send_error(404)
                return
            status = 200
        else:
            status, body = fault.status, fault.body or b""

        headers = {"Content-Type": _content_type(path)}
        if fault.gzipped:
            body = gzip.compress(body, mtime=0)
            headers["Content-Encoding"] = "gzip"
        if status == 200:
            headers["ETag"] = f'"{hashlib.sha256(body).hexdigest()}"'
        bodiless = status in (204, 304)
        request.length = 0 if head or bodiless else len(body)
        if not bodiless:
            headers["Content-Length"] = str(len(body))
        for name, value in fault.headers.items():
            for same in [h for h in headers if h.lower() == name.lower()]:
                del headers[same]
            headers[name] = value

        try:
            self.send_response(status)
            for name, value in headers.items():
                self.send_header(name, value)
            self.end_headers()
            if head or bodiless:
                return
            half = len(body) // 2 if fault.stall_mid else len(body)
            self.wfile.write(body[:half])
            if half < len(body):
                self.wfile.flush()
                time.sleep(fault.stall_mid)
                self.wfile.write(body[half:])
        except (BrokenPipeError, ConnectionResetError):
            pass  # the client gave up, as a deadline test makes it do


DEFAULT_GENERATION = 1_792_560_000
DEFAULT_GENERATED_AT = "2026-10-21T08:00:00Z"


def _write_blob(root: Path, data: bytes | None) -> dict[str, Any] | None:
    if data is None:
        return None
    digest = hashlib.sha256(data).hexdigest()
    blob = root / "c" / "f" / "sha256" / f"{digest}.json.gz"
    blob.parent.mkdir(parents=True, exist_ok=True)
    blob.write_bytes(gzip.compress(data, compresslevel=9, mtime=0))
    return {"sha256": digest, "size": len(data)}


def write_root_json(
    root: Path, flat: bytes | None, stats: bytes | None, **overrides: Any
) -> dict[str, Any]:
    """Write ``<root>/c/root.json`` and return the root dict.

    ``flat`` and ``stats`` are ``None`` (JSON ``null``) or the raw bytes of an
    ``all.json`` / ``stats.json``. Bytes are written as the gzip blob
    ``c/f/sha256/<hex>.json.gz`` (mtime 0), and the root carries
    ``{"sha256", "size"}`` over the inflated bytes. ``overrides`` replace or add
    top-level members (``schema=2``, ``expires=…``). They cannot reach ``flat``
    and ``stats``, which are the parameters: to break a ref, corrupt the blob
    on disk.

    ``generation`` and ``generated_at`` default to fixed values, so rewriting
    the same inputs gives byte-identical bytes and an unchanged ETag.
    """
    doc: dict[str, Any] = {
        "schema": 1,
        "generation": DEFAULT_GENERATION,
        "generated_at": DEFAULT_GENERATED_AT,
        "flat": _write_blob(root, flat),
        "stats": _write_blob(root, stats),
        "planes": {"catalog": None, "enrich": None},
        "search": None,
    }
    doc.update(overrides)
    path = root / "c" / "root.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    # sort_keys orders by code point, which is UTF-8 byte order.
    path.write_bytes(
        json.dumps(
            doc, sort_keys=True, separators=(",", ":"), ensure_ascii=False
        ).encode()
        + b"\n"
    )
    return doc


ConformanceKind = Literal["legacy-only", "c-plus-legacy", "c-only"]
_CONFORMANCE = Path(__file__).resolve().parent.parent / "data" / "conformance"
_CONFORMANCE_DROPPED = {
    "legacy-only": ("c",),
    "c-plus-legacy": (),
    "c-only": ("all.json", "stats.json"),
}


def conformance_tree(dest: Path, kind: ConformanceKind) -> Path:
    """Copy the checked-in indexer tree into ``dest`` as an index of ``kind``.

    ``legacy-only`` drops the anchored ``c/`` tree, ``c-only`` drops
    ``all.json`` and ``stats.json``, ``c-plus-legacy`` keeps both. The
    manifest and README stay behind: a published tree has neither.
    """
    shutil.copytree(
        _CONFORMANCE,
        dest,
        ignore=shutil.ignore_patterns("MANIFEST.json", "README.md"),
        dirs_exist_ok=True,
    )
    for name in _CONFORMANCE_DROPPED[kind]:
        target = dest / name
        if target.is_dir():
            shutil.rmtree(target)
        else:
            target.unlink()
    return dest


INDEX_KINDS: tuple[ConformanceKind, ...] = ("legacy-only", "c-plus-legacy", "c-only")


def _read(path: Path) -> bytes | None:
    return path.read_bytes() if path.is_file() else None


def _digest(data: bytes | None) -> str | None:
    return None if data is None else hashlib.sha256(data).hexdigest()


class KindedIndex:
    """A dist dir served as one of the ``INDEX_KINDS``.

    Tests write plain ``all.json`` and ``stats.json`` into ``root``. Before
    every request the anchored tree (``c/root.json`` and its blobs) is rebuilt
    from them, so the same test body runs against all three kinds. In a
    ``c-only`` index the two legacy files answer 404.

    A document ("flat" or "stats") is published at more than one path in some
    kinds; ``fault`` breaks all of them, so a test says what is unavailable
    and not where. ``outage`` makes the whole server fail.
    """

    def __init__(self, root: Path, kind: ConformanceKind) -> None:
        self.root = root
        self.kind = kind
        self.base = ""
        self._lock = threading.Lock()
        self._published: tuple[str | None, str | None] | None = None
        hidden = {"/all.json": Fault(status=404), "/stats.json": Fault(status=404)}
        index = self

        class Kinded(IndexHandler):
            def _answer(self, *, head: bool) -> None:
                index.publish()
                super()._answer(head=head)

        self.handler = Kinded.new(hidden if kind == "c-only" else None)

    @property
    def log(self) -> list[Request]:
        return self.handler.log

    def publish(self) -> None:
        """Write the anchored tree for the legacy files as they are now."""
        if self.kind == "legacy-only":
            return
        with self._lock:
            flat = _read(self.root / "all.json")
            stats = _read(self.root / "stats.json")
            published = (_digest(flat), _digest(stats))
            if published != self._published:
                write_root_json(self.root, flat, stats)
                self._published = published

    def document_paths(self, document: Literal["flat", "stats"]) -> list[str]:
        """Every URL path serving ``document`` in this kind, as it stands now."""
        name = "all.json" if document == "flat" else "stats.json"
        paths = [] if self.kind == "c-only" else [f"/{name}"]
        data = _read(self.root / name)
        if self.kind != "legacy-only" and data is not None:
            paths.append(f"/c/f/sha256/{_digest(data)}.json.gz")
        return paths

    def fault(self, document: Literal["flat", "stats"], fault: Fault | None) -> None:
        """Apply ``fault`` to every path serving ``document``; ``None`` heals."""
        for path in self.document_paths(document):
            if fault is None:
                self.handler.faults.pop(path, None)
            else:
                self.handler.faults[path] = fault

    def outage(self, down: bool) -> None:
        """Fail every path with a 500, the hidden legacy files included."""
        if down:
            self.handler.faults["*"] = Fault(status=500)
        else:
            self.handler.faults.pop("*", None)
        if self.kind == "c-only":
            for path in ("/all.json", "/stats.json"):
                self.handler.faults[path] = Fault(status=500 if down else 404)


@contextmanager
def serve_kind(root: Path, kind: ConformanceKind) -> Iterator[KindedIndex]:
    """Serve ``root`` as an index of ``kind``; ``base`` is set while serving."""
    index = KindedIndex(root, kind)
    with serve(root, index.handler) as base:
        index.base = base
        yield index
