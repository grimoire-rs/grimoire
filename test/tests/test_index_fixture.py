# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Self-test of ``IndexHandler``, ``write_root_json``, ``conformance_tree`` and
``KindedIndex``.

The index suites trust these to serve and to write exactly what they say;
this file proves it with plain HTTP and file reads, no ``grim`` involved.
"""

from __future__ import annotations

import gzip
import hashlib
import http.client
import json
import time
from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import urlsplit

import pytest

from src.index_fixture import (
    INDEX_KINDS,
    ConformanceKind,
    Fault,
    IndexHandler,
    KindedIndex,
    Request,
    conformance_tree,
    package,
    serve,
    serve_kind,
    write_all_json,
    write_root_json,
    write_stats,
)

BODY = b'{"hello": "index"}\n'


@dataclass(slots=True)
class Reply:
    status: int
    headers: dict[str, str]
    body: bytes


@dataclass(slots=True)
class Served:
    root: Path
    base: str
    handler: type[IndexHandler]

    def get(
        self, path: str, headers: dict[str, str] | None = None, method: str = "GET"
    ) -> Reply:
        """The raw reply: no redirect following, no decompression."""
        netloc = urlsplit(self.base).netloc
        conn = http.client.HTTPConnection(netloc, timeout=10)
        try:
            conn.request(method, path, headers=headers or {})
            resp = conn.getresponse()
            return Reply(
                resp.status, {k.lower(): v for k, v in resp.getheaders()}, resp.read()
            )
        finally:
            conn.close()


@pytest.fixture()
def served(tmp_path: Path) -> Iterator[Served]:
    root = tmp_path / "index-dist"
    root.mkdir()
    (root / "all.json").write_bytes(BODY)
    (root / "stats.json").write_bytes(b"{}")
    handler = IndexHandler.new()
    with serve(root, handler) as base:
        yield Served(root, base, handler)


def test_files_carry_a_strong_etag_and_if_none_match_is_not_honoured(
    served: Served,
) -> None:
    first = served.get("/all.json")
    etag = first.headers["etag"]
    assert (first.status, first.body) == (200, BODY)
    assert etag == f'"{hashlib.sha256(BODY).hexdigest()}"', "strong, and over the bytes"

    again = served.get("/all.json", {"If-None-Match": etag})
    assert (again.status, again.body) == (200, BODY), "a plain static host"

    (served.root / "all.json").write_bytes(b"[]")
    assert served.get("/all.json").headers["etag"] != etag, "new bytes, new ETag"


def test_a_missing_file_is_404_and_a_head_has_no_body(served: Served) -> None:
    assert served.get("/c/root.json").status == 404
    head = served.get("/all.json", method="HEAD")
    assert (head.status, head.body) == (200, b"")
    assert int(head.headers["content-length"]) == len(BODY)


def test_the_request_log_records_method_path_and_headers_in_order(
    served: Served,
) -> None:
    served.get("/all.json?x=1", {"X-Probe": "yes"})
    served.get("/c/root.json")
    served.get("/stats.json", method="HEAD")

    assert [(r.method, r.path) for r in served.handler.log] == [
        ("GET", "/all.json?x=1"),
        ("GET", "/c/root.json"),
        ("HEAD", "/stats.json"),
    ]
    assert served.handler.log[0].headers["x-probe"] == "yes", (
        "header names are lower-cased"
    )
    assert "host" in served.handler.log[1].headers


def test_the_request_log_records_the_response_body_bytes_sent(
    served: Served,
) -> None:
    served.handler.faults["/stats.json"] = Fault(gzipped=True, body=b"[1]")

    served.get("/all.json")
    served.get("/all.json", method="HEAD")
    served.get("/c/root.json")
    sent = served.get("/stats.json").body

    assert [r.length for r in served.handler.log] == [len(BODY), 0, 0, len(sent)], (
        "a body counts, a HEAD and a 404 send none, a gzipped body counts as sent"
    )


def test_each_new_handler_class_has_its_own_log_and_faults() -> None:
    one, two = IndexHandler.new(), IndexHandler.new({"/x": Fault(status=500)})
    one.log.append(Request("GET", "/", {}))
    assert two.log == [] and one.faults == {} and set(two.faults) == {"/x"}


def test_a_status_fault_replaces_the_response_for_its_path_only(
    served: Served,
) -> None:
    served.handler.faults["/all.json"] = Fault(status=503)

    broken = served.get("/all.json")
    assert (broken.status, broken.body) == (503, b"")
    assert served.get("/stats.json").status == 200

    del served.handler.faults["/all.json"]
    assert served.get("/all.json").status == 200, (
        "faults can be lifted between refreshes"
    )


def test_a_status_and_headers_fault_scripts_a_redirect(served: Served) -> None:
    served.handler.faults["/all.json"] = Fault(
        status=302, headers={"Location": "http://elsewhere.invalid/all.json"}
    )

    reply = served.get("/all.json")
    assert reply.status == 302
    assert reply.headers["location"] == "http://elsewhere.invalid/all.json"


def test_a_header_fault_adds_headers_and_replaces_same_named_defaults(
    served: Served,
) -> None:
    served.handler.faults["/all.json"] = Fault(
        headers={"X-Extra": "1", "content-type": "text/html"}
    )

    reply = served.get("/all.json")
    assert (reply.status, reply.body) == (200, BODY), "the file is still served"
    assert reply.headers["x-extra"] == "1"
    assert reply.headers["content-type"] == "text/html", "no duplicate Content-Type"


def test_a_body_fault_replaces_the_body(served: Served) -> None:
    served.handler.faults["/c/root.json"] = Fault(body=b"<html>spa fallback</html>")

    reply = served.get("/c/root.json")
    assert (reply.status, reply.body) == (200, b"<html>spa fallback</html>")


def test_a_gzipped_fault_sends_the_body_with_content_encoding_gzip(
    served: Served,
) -> None:
    served.handler.faults["/all.json"] = Fault(gzipped=True)
    served.handler.faults["/stats.json"] = Fault(gzipped=True, body=b"[1]")

    reply = served.get("/all.json")
    assert reply.headers["content-encoding"] == "gzip"
    assert reply.body[:2] == b"\x1f\x8b"
    assert gzip.decompress(reply.body) == BODY
    assert gzip.decompress(served.get("/stats.json").body) == b"[1]"


def test_a_truncated_body_is_a_content_length_header_fault(served: Served) -> None:
    served.handler.faults["/all.json"] = Fault(headers={"Content-Length": "5"})

    assert served.get("/all.json").body == BODY[:5]


def test_a_stall_before_delays_the_whole_response(served: Served) -> None:
    served.handler.faults["/all.json"] = Fault(stall_before=0.4)

    start = time.monotonic()
    reply = served.get("/all.json")
    assert time.monotonic() - start >= 0.35
    assert reply.body == BODY


def test_a_stall_mid_sends_half_the_body_then_waits(served: Served) -> None:
    served.handler.faults["/all.json"] = Fault(stall_mid=0.5)
    conn = http.client.HTTPConnection(urlsplit(served.base).netloc, timeout=10)
    try:
        conn.request("GET", "/all.json")
        resp = conn.getresponse()
        half = len(BODY) // 2
        assert resp.read(half) == BODY[:half], "the first half arrives at once"
        waited = time.monotonic()
        rest = resp.read()
        assert time.monotonic() - waited >= 0.4, "the rest waits out the stall"
        assert BODY[:half] + rest == BODY
    finally:
        conn.close()


def test_a_client_that_gives_up_mid_stall_does_not_break_the_server(
    served: Served,
) -> None:
    served.handler.faults["/all.json"] = Fault(stall_mid=0.3)
    conn = http.client.HTTPConnection(urlsplit(served.base).netloc, timeout=10)
    conn.request("GET", "/all.json")
    conn.getresponse().read(1)
    conn.close()
    time.sleep(0.5)

    assert served.get("/stats.json").status == 200


def test_the_default_root_is_the_deterministic_all_null_document(
    tmp_path: Path,
) -> None:
    doc = write_root_json(tmp_path, flat=None, stats=None)

    raw = (tmp_path / "c" / "root.json").read_bytes()
    assert raw == (
        b'{"flat":null,"generated_at":"2026-10-21T08:00:00Z","generation":1792560000,'
        b'"planes":{"catalog":null,"enrich":null},"schema":1,"search":null,"stats":null}\n'
    )
    assert json.loads(raw) == doc
    assert not (tmp_path / "c" / "f").exists(), "no blob without bytes"


def test_a_ref_covers_the_inflated_bytes_and_names_a_deterministic_blob(
    tmp_path: Path,
) -> None:
    flat = json.dumps([{"name": "a"}], indent=1).encode()
    stats = b'{"schema_version": 1}'

    doc = write_root_json(tmp_path, flat, stats)

    for member, data in (("flat", flat), ("stats", stats)):
        ref = doc[member]
        digest = hashlib.sha256(data).hexdigest()
        assert ref == {"sha256": digest, "size": len(data)}
        blob = (tmp_path / "c" / "f" / "sha256" / f"{digest}.json.gz").read_bytes()
        assert gzip.decompress(blob) == data
        assert blob[:2] == b"\x1f\x8b" and blob[3] == 0, "gzip with no name or comment"
        assert blob[4:8] == bytes(4), "mtime 0"
    on_disk = json.loads((tmp_path / "c" / "root.json").read_bytes())
    assert on_disk["flat"] == doc["flat"] and on_disk["stats"] == doc["stats"]


def test_the_same_inputs_give_the_same_bytes(tmp_path: Path) -> None:
    one, two = tmp_path / "one", tmp_path / "two"
    for root in (one, two):
        write_root_json(root, b"[1]", b"{}", note="x")

    assert (one / "c" / "root.json").read_bytes() == (
        two / "c" / "root.json"
    ).read_bytes()
    blobs = sorted(p.name for p in (one / "c" / "f" / "sha256").iterdir())
    assert blobs == sorted(p.name for p in (two / "c" / "f" / "sha256").iterdir())
    for name in blobs:
        assert (one / "c" / "f" / "sha256" / name).read_bytes() == (
            two / "c" / "f" / "sha256" / name
        ).read_bytes()


def test_overrides_replace_or_add_members_and_keys_come_out_in_byte_order(
    tmp_path: Path,
) -> None:
    doc = write_root_json(
        tmp_path,
        None,
        None,
        schema=2,
        expires="2027-01-01T00:00:00Z",
        **{"10": 1, "0a": 2, "é": 3},
    )

    raw = (tmp_path / "c" / "root.json").read_bytes()
    assert doc["schema"] == 2 and doc["expires"] == "2027-01-01T00:00:00Z"
    assert b'"schema":2' in raw and b'"expires":"2027-01-01T00:00:00Z"' in raw
    keys = list(json.loads(raw))
    assert keys == sorted(keys, key=str.encode), "keys in UTF-8 byte order"
    assert keys.index("0a") < keys.index("10") < keys.index("expires") < keys.index("é")
    assert raw.endswith(b"}\n") and not raw.endswith(b"\n\n"), "one trailing newline"
    assert b": " not in raw and b", " not in raw, "no insignificant whitespace"
    assert "é".encode() in raw, "UTF-8, not \\u escapes"


def test_a_written_root_is_served_with_an_etag(served: Served) -> None:
    write_root_json(served.root, b"[]", None)

    reply = served.get("/c/root.json")
    assert reply.body == (served.root / "c" / "root.json").read_bytes()
    assert reply.headers["etag"] == f'"{hashlib.sha256(reply.body).hexdigest()}"'


CONFORMANCE = Path(__file__).resolve().parent.parent / "data" / "conformance"


def _tree_files(root: Path) -> set[str]:
    return {p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file()}


@pytest.mark.parametrize(
    ("kind", "has_legacy", "has_c"),
    [
        ("legacy-only", True, False),
        ("c-plus-legacy", True, True),
        ("c-only", False, True),
    ],
)
def test_a_conformance_tree_is_shaped_by_its_kind(
    tmp_path: Path, kind: ConformanceKind, has_legacy: bool, has_c: bool
) -> None:
    dest = conformance_tree(tmp_path / "dist", kind)

    files = _tree_files(dest)
    assert dest == tmp_path / "dist"
    assert ("all.json" in files) is has_legacy
    assert ("stats.json" in files) is has_legacy
    assert any(f.startswith("c/") for f in files) is has_c
    assert not files & {"MANIFEST.json", "README.md"}, "a published tree has neither"


def test_a_conformance_tree_copy_matches_the_manifest(tmp_path: Path) -> None:
    manifest = json.loads((CONFORMANCE / "MANIFEST.json").read_bytes())["files"]

    dest = conformance_tree(tmp_path, "c-plus-legacy")

    assert _tree_files(dest) == set(manifest)
    for rel, want in manifest.items():
        data = (dest / rel).read_bytes()
        if rel.endswith(".gz"):
            data = gzip.decompress(data)
        assert hashlib.sha256(data).hexdigest() == want["sha256"], rel
        assert len(data) == want["size"], rel


def test_a_wildcard_fault_answers_every_path_without_its_own(served: Served) -> None:
    served.handler.faults["*"] = Fault(status=500)
    served.handler.faults["/stats.json"] = Fault(status=204)

    assert served.get("/all.json").status == 500
    assert served.get("/missing.json").status == 500
    assert served.get("/stats.json").status == 204, "a path's own fault wins"

    del served.handler.faults["*"]
    assert served.get("/all.json").status == 200


@pytest.fixture(params=INDEX_KINDS)
def kinded(request: pytest.FixtureRequest, tmp_path: Path) -> Iterator[KindedIndex]:
    root = tmp_path / "index-dist"
    root.mkdir()
    write_all_json(root, [package("one", "skill", "ghcr.io/acme/skills/one", "One")])
    write_stats(root, {})
    with serve_kind(root, request.param) as index:
        yield index


def _get(index: KindedIndex, path: str) -> Reply:
    return Served(index.root, index.base, index.handler).get(path)


def test_a_kinded_index_serves_each_document_where_its_kind_publishes_it(
    kinded: KindedIndex,
) -> None:
    flat = (kinded.root / "all.json").read_bytes()
    legacy = kinded.kind != "c-only"
    anchored = kinded.kind != "legacy-only"

    assert (_get(kinded, "/all.json").status == 200) is legacy
    assert (_get(kinded, "/stats.json").status == 200) is legacy
    assert (_get(kinded, "/c/root.json").status == 200) is anchored
    if anchored:
        root = json.loads(_get(kinded, "/c/root.json").body)
        blob = _get(kinded, f"/c/f/sha256/{root['flat']['sha256']}.json.gz")
        assert gzip.decompress(blob.body) == flat
    assert [p for p in kinded.document_paths("flat") if p.startswith("/c/")] == (
        [f"/c/f/sha256/{hashlib.sha256(flat).hexdigest()}.json.gz"] if anchored else []
    )


def test_a_kinded_index_follows_the_files_as_they_change(kinded: KindedIndex) -> None:
    if kinded.kind == "legacy-only":
        pytest.skip("a legacy-only index has no anchored tree to follow")
    before = _get(kinded, "/c/root.json").body

    write_all_json(
        kinded.root, [package("two", "skill", "ghcr.io/acme/skills/two", "Two")]
    )

    assert _get(kinded, "/c/root.json").body != before, "a new document, a new root"
    assert any(r.path == "/c/root.json" for r in kinded.log)


def test_a_document_fault_breaks_every_path_serving_it_and_heals(
    kinded: KindedIndex,
) -> None:
    paths = kinded.document_paths("flat")
    assert paths, "every kind serves the flat document somewhere"
    _get(kinded, "/c/root.json")  # builds the blob in the anchored kinds

    kinded.fault("flat", Fault(status=503))
    assert {_get(kinded, p).status for p in paths} == {503}

    kinded.fault("flat", None)
    assert {_get(kinded, p).status for p in paths} == {200}


def test_an_outage_fails_the_whole_server_until_lifted(kinded: KindedIndex) -> None:
    kinded.outage(True)
    assert _get(kinded, "/all.json").status == 500
    assert _get(kinded, "/c/root.json").status == 500

    kinded.outage(False)
    assert _get(kinded, "/c/root.json").status == (
        404 if kinded.kind == "legacy-only" else 200
    )
