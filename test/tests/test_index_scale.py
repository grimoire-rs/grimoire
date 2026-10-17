# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""An index with more than 500 entries is listed whole.

Index sources are no longer capped at 500 entries (the OCI ``_catalog``
walk keeps its cap). The search envelope reports ``total`` (every matching
row, counted before ``--limit``) and ``truncated`` (``items`` is not the
full answer), ``--limit <N>`` cuts the ranked or ``--sort``ed result, and
``--offset <N>`` skips rows first so a consumer can read the next page.
"""

from __future__ import annotations

import json
from pathlib import Path

from src.index_fixture import (
    filtered_index_config,
    index_config,
    package,
    search_envelope,
    write_all_json,
    write_stats,
)

COUNT = 600
# Row 550 lies past the old 500-entry cap.
LATE = "ghcr.io/acme/skills/pkg-550"


def _ref(i: int) -> str:
    return f"ghcr.io/acme/skills/pkg-{i:03d}"


def _big_packages() -> list[dict]:
    return [
        package(
            f"pkg-{i:03d}",
            "skill",
            _ref(i),
            "Zebra crossing helper" if i == 550 else "Generic helper",
        )
        for i in range(COUNT)
    ]


def _write_big_index(root: Path) -> None:
    write_all_json(root, _big_packages())


def _rating(up: int) -> dict:
    return {"rating": {"up": up, "target": "t", "url": "https://example.test/1"}}


def _repos(envelope: dict) -> list[str]:
    return [row["repo"] for row in envelope["items"]]


def test_the_envelope_always_carries_total_and_truncated(
    grim_at, project_dir: Path, http_index
) -> None:
    root, base = http_index
    write_all_json(
        root,
        [package("one", "skill", "ghcr.io/acme/skills/one", "First")],
    )
    index_config(project_dir, base)

    envelope = search_envelope(grim_at(project_dir))

    assert envelope["total"] == 1
    assert envelope["truncated"] is False


def test_an_index_past_500_entries_is_listed_whole(
    grim_at, project_dir: Path, http_index
) -> None:
    root, base = http_index
    _write_big_index(root)
    index_config(project_dir, base)

    envelope = search_envelope(grim_at(project_dir))

    assert sorted(_repos(envelope)) == [_ref(i) for i in range(COUNT)], (
        f"all {COUNT} rows must list; got {len(envelope['items'])}"
    )
    assert envelope["total"] == COUNT
    assert envelope["truncated"] is False


def test_a_query_reaches_a_row_past_the_old_cap(
    grim_at, project_dir: Path, http_index
) -> None:
    root, base = http_index
    _write_big_index(root)
    index_config(project_dir, base)

    envelope = search_envelope(grim_at(project_dir), "zebra")

    assert _repos(envelope) == [LATE]
    assert envelope["total"] == 1, "total counts the query's matches, not the catalog"
    assert envelope["truncated"] is False


def test_an_include_filter_reaches_rows_past_the_old_cap(
    grim_at, project_dir: Path, http_index
) -> None:
    """The browse filter runs after the build, so it only sees what the
    build kept: with the old cap, `pkg-55*` admitted nothing."""
    root, base = http_index
    _write_big_index(root)
    filtered_index_config(project_dir, base, include=("acme/skills/pkg-55*",))

    envelope = search_envelope(grim_at(project_dir))

    assert sorted(_repos(envelope)) == [_ref(i) for i in range(550, 560)]
    assert envelope["total"] == 10, "total counts the rows the filter admitted"
    assert envelope["truncated"] is False


def test_limit_on_a_filtered_browse_counts_the_filtered_rows(
    grim_at, project_dir: Path, http_index
) -> None:
    root, base = http_index
    _write_big_index(root)
    filtered_index_config(project_dir, base, include=("acme/skills/pkg-55*",))

    envelope = search_envelope(grim_at(project_dir), "--limit", "3")

    assert len(envelope["items"]) == 3
    assert envelope["total"] == 10, "total is the filtered count, not the catalog's 600"
    assert envelope["truncated"] is True


def test_rate_resolves_a_row_past_the_old_cap(
    grim_at, project_dir: Path, http_index
) -> None:
    root, base = http_index
    _write_big_index(root)
    write_stats(
        root,
        {
            LATE: {
                "rating": {
                    "up": 5,
                    "target": "D_kwDOAbCdEf",
                    "url": "https://github.com/acme/index/discussions/7",
                }
            }
        },
    )
    index_config(project_dir, base)
    runner = grim_at(project_dir)
    search_envelope(runner)  # warm the catalog `rate` resolves against

    result = runner.run("--format", "json", "rate", LATE, "--dry-run", check=False)

    assert result.returncode == 0, (
        f"a row past the old cap must resolve (79 = no such row); stderr: {result.stderr}"
    )
    assert json.loads(result.stdout)["ref"] == LATE


def test_limit_cuts_the_ranked_result_and_reports_the_full_count(
    grim_at, project_dir: Path, http_index
) -> None:
    root, base = http_index
    _write_big_index(root)
    index_config(project_dir, base)
    runner = grim_at(project_dir)

    limited = search_envelope(runner, "--limit", "5")
    full = search_envelope(runner)

    assert len(limited["items"]) == 5
    assert limited["total"] == COUNT
    assert limited["truncated"] is True
    assert _repos(limited) == _repos(full)[:5], "--limit applies after ranking"


def test_limit_applies_after_the_sort_order(
    grim_at, project_dir: Path, http_index
) -> None:
    """A limit applied before the sort would keep ``pkg-000``; the most-upvoted
    row sits past the old 500-entry cap."""
    root, base = http_index
    _write_big_index(root)
    write_stats(root, {LATE: _rating(9), _ref(10): _rating(2)})
    index_config(project_dir, base)
    runner = grim_at(project_dir)

    top = search_envelope(runner, "--sort", "rating", "--limit", "1")
    top_two = search_envelope(runner, "--sort", "rating", "--limit", "2")

    assert _repos(top) == [LATE]
    assert top["total"] == COUNT
    assert top["truncated"] is True
    assert _repos(top_two) == [LATE, _ref(10)]


def test_limit_applies_after_the_relevance_rank(
    grim_at, project_dir: Path, http_index
) -> None:
    """The weaker match is declared first, so a limit applied before ranking
    would keep ``pkg-000`` instead of the stronger late row."""
    root, base = http_index
    packages = _big_packages()
    packages[0]["description"] = "Helper for zebra patterns"
    packages[550]["keywords"] = ["zebra"]
    packages[550]["summary"] = "Zebra crossing"
    write_all_json(root, packages)
    index_config(project_dir, base)
    runner = grim_at(project_dir)
    # Both rows must survive the relevance cutoff for the limit to have a choice.
    runner.run("config", "set", "options.search_min_relevance", "0")

    full = search_envelope(runner, "zebra")
    limited = search_envelope(runner, "zebra", "--limit", "1")

    assert _repos(full) == [LATE, _ref(0)], "the late row outranks the earlier one"
    assert _repos(limited) == [LATE]
    assert limited["total"] == 2
    assert limited["truncated"] is True


def test_limit_zero_is_a_usage_error(grim_at, project_dir: Path, http_index) -> None:
    root, base = http_index
    write_all_json(root, [package("one", "skill", "ghcr.io/acme/skills/one", "First")])
    index_config(project_dir, base)
    runner = grim_at(project_dir)

    result = runner.run("search", "--refresh", "--limit", "0", check=False)

    assert result.returncode == 64, (
        f"--limit 0 must exit 64, got {result.returncode}; stderr: {result.stderr}"
    )
    assert "--limit" in result.stderr and "unexpected argument" not in result.stderr, (
        f"the usage error must name --limit as an invalid value; stderr: {result.stderr}"
    )
    # Positive control: a binary without the flag also exits 64 above, so the
    # flag must be accepted at its lower bound.
    assert len(search_envelope(runner, "--limit", "1")["items"]) == 1


def _pages(runner, size: int, *extra: str) -> list[dict]:
    """Read the whole result ``size`` rows at a time, the documented recipe:
    ``offset += len(items)`` while ``offset < total``."""
    pages: list[dict] = []
    offset = 0
    while True:
        page = search_envelope(
            runner, *extra, "--offset", str(offset), "--limit", str(size)
        )
        pages.append(page)
        offset += len(page["items"])
        if not page["items"] or offset >= page["total"]:
            return pages


def test_pages_of_100_cover_the_whole_index_disjointly_in_the_unpaged_order(
    grim_at, project_dir: Path, http_index
) -> None:
    root, base = http_index
    _write_big_index(root)
    index_config(project_dir, base)
    runner = grim_at(project_dir)

    full = search_envelope(runner)
    pages = _pages(runner, 100)

    assert len(pages) == COUNT // 100
    assert all(len(p["items"]) == 100 for p in pages)
    assert all(p["total"] == COUNT and p["truncated"] is True for p in pages)
    stitched = [repo for p in pages for repo in _repos(p)]
    assert len(set(stitched)) == COUNT, "no row may appear on two pages"
    assert stitched == _repos(full), "pages concatenate to the unpaged order"


def test_pages_are_sound_under_a_sort_and_under_a_query(
    grim_at, project_dir: Path, http_index
) -> None:
    root, base = http_index
    _write_big_index(root)
    write_stats(root, {_ref(i): _rating(i % 4) for i in range(0, COUNT, 3)})
    index_config(project_dir, base)
    runner = grim_at(project_dir)

    for extra in (("--sort", "rating"), ("--sort", "name"), ("generic",)):
        full = search_envelope(runner, *extra)
        stitched = [repo for p in _pages(runner, 70, *extra) for repo in _repos(p)]
        assert stitched == _repos(full), f"paging {extra} must match the unpaged list"


def test_offset_at_or_past_the_end_lists_nothing_and_exits_zero(
    grim_at, project_dir: Path, http_index
) -> None:
    root, base = http_index
    _write_big_index(root)
    index_config(project_dir, base)
    runner = grim_at(project_dir)

    for offset in (COUNT, 10000):
        envelope = search_envelope(runner, "--offset", str(offset))
        assert envelope["items"] == []
        assert envelope["total"] == COUNT
        assert envelope["truncated"] is True


def test_offset_without_limit_skips_rows_and_keeps_the_rest(
    grim_at, project_dir: Path, http_index
) -> None:
    root, base = http_index
    _write_big_index(root)
    index_config(project_dir, base)
    runner = grim_at(project_dir)

    full = search_envelope(runner)
    skipped = search_envelope(runner, "--offset", "595")

    assert _repos(skipped) == _repos(full)[595:]
    assert skipped["total"] == COUNT
    assert skipped["truncated"] is True
    zero = search_envelope(runner, "--offset", "0")
    assert zero == full, "--offset 0 is the unpaged answer"


def test_offset_rejects_negative_and_non_integer_values(
    grim_at, project_dir: Path, http_index
) -> None:
    root, base = http_index
    write_all_json(root, [package("one", "skill", "ghcr.io/acme/skills/one", "First")])
    index_config(project_dir, base)
    runner = grim_at(project_dir)

    for bad in ("-1", "abc", "1.5"):
        result = runner.run("search", "--refresh", "--offset", bad, check=False)
        assert result.returncode == 64, (
            f"--offset {bad} must exit 64, got {result.returncode}; stderr: {result.stderr}"
        )
    assert search_envelope(runner, "--offset", "0")["total"] == 1


def test_plain_output_hints_the_next_page_on_stderr_only_for_a_partial_page(
    grim_at, project_dir: Path, http_index
) -> None:
    root, base = http_index
    _write_big_index(root)
    index_config(project_dir, base)
    runner = grim_at(project_dir)

    middle = runner.run("search", "--refresh", "--offset", "100", "--limit", "100")
    assert "showing 101-200 of 600; next page: --offset 200" in middle.stderr
    assert "showing" not in middle.stdout

    last = runner.run("search", "--refresh", "--offset", "500", "--limit", "100")
    assert "showing 501-600 of 600" in last.stderr
    assert "next page" not in last.stderr

    past = runner.run("search", "--refresh", "--offset", "700")
    assert "no rows at --offset 700; 600 match in total" in past.stderr

    complete = runner.run("search", "--refresh", "--limit", "600")
    assert "showing" not in complete.stderr and "next page" not in complete.stderr
    unpaged = runner.run("search", "--refresh")
    assert "showing" not in unpaged.stderr and "next page" not in unpaged.stderr


def test_json_output_prints_no_paging_hint(
    grim_at, project_dir: Path, http_index
) -> None:
    root, base = http_index
    _write_big_index(root)
    index_config(project_dir, base)
    runner = grim_at(project_dir)

    result = runner.run(
        "--format", "json", "search", "--refresh", "--offset", "100", "--limit", "100"
    )

    assert "showing" not in result.stderr and "next page" not in result.stderr
    assert "offset" not in json.loads(result.stdout), "no new JSON field"


def test_a_multi_host_index_keeps_its_qualified_reference_order(
    grim_at, project_dir: Path, http_index
) -> None:
    """The unqueried browse and an equal-score query list rows by
    ``registry/repository``, as before paging existed. Host order and path order
    disagree here, so a tie-break on the bare repository path would flip it."""
    root, base = http_index
    write_all_json(
        root,
        [
            package("tool", "skill", "quay.io/aaa/tool", "Same words"),
            package("tool", "skill", "ghcr.io/zzz/tool", "Same words"),
        ],
    )
    index_config(project_dir, base)
    runner = grim_at(project_dir)

    expected = ["ghcr.io/zzz/tool", "quay.io/aaa/tool"]
    assert _repos(search_envelope(runner)) == expected
    assert _repos(search_envelope(runner, "tool")) == expected
    assert _repos(search_envelope(runner, "--offset", "1")) == expected[1:]


def test_later_pages_read_the_cache_with_offline_after_a_refreshing_first_call(
    grim_at, project_dir: Path, http_index
) -> None:
    root, base = http_index
    _write_big_index(root)
    index_config(project_dir, base)
    runner = grim_at(project_dir)

    first = search_envelope(runner, "--offset", "0", "--limit", "100")
    stitched = _repos(first)
    offset = len(first["items"])
    while offset < first["total"]:
        result = runner.run(
            "--format",
            "json",
            "--offline",
            "search",
            "--offset",
            str(offset),
            "--limit",
            "100",
        )
        page = json.loads(result.stdout)
        assert page["total"] == COUNT and page["items"], "an offline page is served"
        stitched += _repos(page)
        offset += len(page["items"])

    assert stitched == _repos(search_envelope(runner))
