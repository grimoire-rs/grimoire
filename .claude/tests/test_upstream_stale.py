"""Tests for `.claude/skills/upstream-refresh/scripts/upstream_stale.py`.

Contracts: C-014 (report), C-015 (ladder, implementing C-003), S-010.
Every test builds a fake repository under `tmp_path` and runs the script as a
subprocess with a pinned `--today`, so the date arithmetic is deterministic.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

SCRIPT = (
    Path(__file__).resolve().parents[1]
    / "skills"
    / "upstream-refresh"
    / "scripts"
    / "upstream_stale.py"
)
TODAY = "2026-09-27"

WATCHLIST = ".claude/rules/vendor-capability-watchlist.md"
PRODUCT = ".claude/rules/product-context.md"
LEDGER = ".agents/upstream-checks.md"
LEDGER_HEADER = (
    "# Upstream checks\n\n"
    "| Target | Kind | Tier | Last check | Depth | Feed cursor | Last deep | Notes |\n"
    "|---|---|---|---|---|---|---|---|\n"
)

CLEAN = {
    WATCHLIST: (
        "# Watchlist\n\n## Watchlist\n\n"
        "All rows `verified 2026-07-17` unless noted.\n\n"
        "| Capability | Vendor | Upstream status |\n|---|---|---|\n"
        "| Skills | Foo | shipped |\n"
    ),
    PRODUCT: "# Product\n\nLandscape (re-verify after 2027-01-26).\n",
    "src/install/vendor_foo.rs": "//! Foo mapping, verified 2026-07-27 against docs.\n",
    ".agents/research/research_foo.md": "# Foo\n\n**Expires:** 2027-03-27\n",
    LEDGER: LEDGER_HEADER
    + "| claude | harness | 1 | 2026-09-20 | deep | v1 | 2026-09-20 | |\n",
}


def make_tree(root: Path) -> Path:
    """Write a clean repository tree: root markers plus every ledger, all fresh."""
    root.mkdir(parents=True, exist_ok=True)
    (root / "taskfile.yml").write_text("version: '3'\n")
    (root / "AGENTS.md").write_text("# AGENTS\n")
    for rel, text in CLEAN.items():
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    return root


def with_files(root: Path, files: dict[str, str | None]) -> Path:
    """Clean tree with overrides keyed by relative path; None removes the file."""
    make_tree(root)
    for rel, text in files.items():
        path = root / rel
        if text is None:
            path.unlink()
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
    return root


def run(
    root: Path | None, *args: str, today: str = TODAY, cwd: Path | None = None
) -> subprocess.CompletedProcess[str]:
    cmd = [sys.executable, str(SCRIPT), *args, "--today", today]
    if root is not None:
        cmd += ["--root", str(root)]
    return subprocess.run(
        cmd, capture_output=True, text=True, encoding="utf-8", cwd=cwd, check=False
    )


def lines(result: subprocess.CompletedProcess[str]) -> list[str]:
    return result.stdout.strip().splitlines()


def findings(result: subprocess.CompletedProcess[str]) -> list[str]:
    """Finding lines of a completed report; the report is warn-only, so exit 0."""
    assert result.returncode == 0, result.stderr
    return [ln for ln in lines(result) if not ln.startswith("upstream:stale:")]


def watchlist(body: str) -> str:
    return "# Watchlist\n\n" + body


# ---------------------------------------------------------------- C-014 (1)


def test_c014_watchlist_inline_verified_is_stale_by_27_days(tmp_path: Path) -> None:
    body = watchlist(
        "## A\n\nAll rows `verified 2026-07-17` unless noted.\n\n"
        "| Capability | Vendor |\n|---|---|\n"
        "| Old | X — verified 2026-03-01 |\n"
        "| Defaulted | Y |\n"
    )
    root = with_files(tmp_path, {WATCHLIST: body})
    out = findings(run(root, "report"))
    assert out == [
        f"STALE {WATCHLIST}:9 2026-03-01 27d | Old | X — verified 2026-03-01 |"
    ]


def test_c014_watchlist_newest_inline_date_wins_and_event_dates_ignored(
    tmp_path: Path,
) -> None:
    body = watchlist(
        "## A\n\nAll rows `verified 2026-07-17` unless noted.\n\n"
        "| Capability | Vendor |\n|---|---|\n"
        "| Two | verified 2026-01-01, re-verified 2026-07-20 |\n"
        "| Event | shipped 2025-01-01 in v1.0 |\n"
    )
    root = with_files(tmp_path, {WATCHLIST: body})
    assert findings(run(root, "report")) == []


def test_c014_watchlist_default_line_forms(tmp_path: Path) -> None:
    body = watchlist(
        "## Range\n\nAll rows `verified 2026-03-19/20` (Cursor, Kiro).\n\n"
        "| Capability | Vendor |\n|---|---|\n| R | a |\n\n"
        "## Trailing text\n\nAll rows `verified 2026-03-01` against docs.qoder.com\n\n"
        "| Capability | Vendor |\n|---|---|\n| T | b |\n\n"
        "## Trailing period\n\nAll rows `verified 2026-03-01`.\n\n"
        "| Capability | Vendor |\n|---|---|\n| P | c |\n\n"
        "## Plain\n\nAll rows `verified 2026-03-01`\n\n"
        "| Capability | Vendor |\n|---|---|\n| Q | d |\n"
    )
    root = with_files(tmp_path, {WATCHLIST: body})
    out = findings(run(root, "report"))
    # the day range takes the later day (20th -> 8d, not 19th -> 9d)
    assert f"STALE {WATCHLIST}:9 2026-03-20 8d | R | a |" in out
    assert any(ln.startswith(f"STALE {WATCHLIST}:17 2026-03-01 27d") for ln in out)
    assert any(ln.startswith(f"STALE {WATCHLIST}:25 2026-03-01 27d") for ln in out)
    assert any(ln.startswith(f"STALE {WATCHLIST}:33 2026-03-01 27d") for ln in out)
    assert len(out) == 4


def test_c014_default_line_does_not_cross_sections(tmp_path: Path) -> None:
    body = watchlist(
        "## A\n\nAll rows `verified 2026-07-17`.\n\n"
        "## B\n\n| Capability | Vendor |\n|---|---|\n| Undated | z |\n"
    )
    root = with_files(tmp_path, {WATCHLIST: body})
    assert findings(run(root, "report")) == [
        f"STALE {WATCHLIST}:11 — - | Undated | z |"
    ]


def test_c014_class_table_is_ignored(tmp_path: Path) -> None:
    body = watchlist(
        "## Classes\n\n| Class | What it is | Policy |\n|---|---|---|\n"
        "| 1 | cosmetic | fine |\n"
    )
    root = with_files(tmp_path, {WATCHLIST: body})
    assert findings(run(root, "report")) == []


def test_c014_capability_row_without_any_date_is_stale_dash(tmp_path: Path) -> None:
    body = watchlist("## A\n\n| Capability | Vendor |\n|---|---|\n| Hooks | Foo |\n")
    root = with_files(tmp_path, {WATCHLIST: body})
    assert findings(run(root, "report")) == [f"STALE {WATCHLIST}:7 — - | Hooks | Foo |"]


def test_c014_c018_variable_table_is_scanned(tmp_path: Path) -> None:
    body = watchlist(
        "## Config roots and env vars\n\n"
        "| Variable | Vendor | grim behavior | Upstream status |\n|---|---|---|---|\n"
        "| `FOO_HOME` | Foo | honored | verified 2026-03-01 |\n"
    )
    root = with_files(tmp_path, {WATCHLIST: body})
    out = findings(run(root, "report"))
    assert len(out) == 1
    assert out[0].startswith(f"STALE {WATCHLIST}:7 2026-03-01 27d ")


def test_c014_threshold_boundary_is_183_days(tmp_path: Path) -> None:
    body = watchlist(
        "## A\n\n| Capability | Vendor |\n|---|---|\n"
        "| Exact | verified 2026-03-28 |\n"
        "| Over | verified 2026-03-27 |\n"
    )
    root = with_files(tmp_path, {WATCHLIST: body})
    out = findings(run(root, "report"))
    assert out == [f"STALE {WATCHLIST}:8 2026-03-27 1d | Over | verified 2026-03-27 |"]


# ---------------------------------------------------------------- C-014 (2)


def test_c014_vendor_stamp_date_on_next_doc_line(tmp_path: Path) -> None:
    root = with_files(
        tmp_path,
        {
            "src/install/vendor_x.rs": "//! Mapping, live-verified\n//! 2026-01-02, see research.\n",
            "src/install/vendor_y.rs": "//! Mapping, verified\n//!\n//! 2026-01-02 too far.\n",
        },
    )
    out = findings(run(root, "report"))
    assert out == [
        "STALE src/install/vendor_x.rs:1 2026-01-02 85d //! Mapping, live-verified"
    ]


def test_c014_indented_vendor_stamp_is_scanned(tmp_path: Path) -> None:
    root = with_files(
        tmp_path,
        {
            "src/install/vendor_z.rs": "mod m {\n    //! verified\n    //! 2026-01-02\n}\n"
        },
    )
    out = findings(run(root, "report"))
    assert out == ["STALE src/install/vendor_z.rs:2 2026-01-02 85d //! verified"]


# ---------------------------------------------------------------- C-014 (3)


def test_c014_reverify_after(tmp_path: Path) -> None:
    root = with_files(
        tmp_path,
        {
            PRODUCT: "# P\n\nOld (re-verify after 2026-09-01).\nNew (re-verify after 2027-01-26).\n",
        },
    )
    out = findings(run(root, "report"))
    assert out == [
        f"STALE {PRODUCT}:3 2026-09-01 26d Old (re-verify after 2026-09-01)."
    ]


def test_c014_reverify_after_in_watchlist(tmp_path: Path) -> None:
    root = with_files(
        tmp_path, {WATCHLIST: CLEAN[WATCHLIST] + "\nre-verify after 2026-09-01\n"}
    )
    out = findings(run(root, "report"))
    assert out == [f"STALE {WATCHLIST}:11 2026-09-01 26d re-verify after 2026-09-01"]


def test_c014_reverify_after_today_is_not_past(tmp_path: Path) -> None:
    root = with_files(tmp_path, {PRODUCT: "re-verify after 2026-09-27\n"})
    assert findings(run(root, "report")) == []


# ---------------------------------------------------------------- C-014 (4)


def test_c014_research_expires(tmp_path: Path) -> None:
    r = ".agents/research/"
    root = with_files(
        tmp_path,
        {
            r + "a.md": "# A\n\n**Expires:** 2026-08 (vendor docs move fast)\n",
            r + "b.md": "# B\n\n**Expires:** 2026-09-30\n",
            r + "c.md": "# C\n\n**Expires:** n/a (historical — adr_x.md)\n",
            r + "d.md": "# D\n\nNo expiry line.\n",
            r + "e.md": "# E\n\n**Expires:** soon\n",
        },
    )
    out = findings(run(root, "report"))
    assert out == [
        f"EXPIRED {r}e.md:3 ? - **Expires:** soon",
        f"EXPIRED {r}a.md:3 2026-08-31 27d **Expires:** 2026-08 (vendor docs move fast)",
    ]


# ---------------------------------------------------------------- C-014 (5)


def test_c014_check_ledger_unchecked_and_stale(tmp_path: Path) -> None:
    ledger = (
        LEDGER_HEADER
        + "| claude | harness | 1 | — | — | — | — | |\n"
        + "| codex | harness | 1 | 2026-03-11 | deep | v1 | 2026-03-11 | |\n"
        + "| cursor | harness | 2 | 2026-09-26 | feed | v2 | — | |\n"
    )
    root = with_files(tmp_path, {LEDGER: ledger})
    out = findings(run(root, "report"))
    assert len(out) == 2
    assert out[0].startswith(f"UNCHECKED {LEDGER}:5 — - | claude |")
    assert out[1].startswith(f"STALE {LEDGER}:6 2026-03-11 17d | codex |")


def test_c014_check_ledger_unreadable_date_shows_question_mark(tmp_path: Path) -> None:
    ledger = LEDGER_HEADER + "| claude | harness | 1 | yesterday | — | — | — | |\n"
    root = with_files(tmp_path, {LEDGER: ledger})
    out = findings(run(root, "report"))
    assert len(out) == 1
    assert out[0].startswith(f"UNCHECKED {LEDGER}:5 ? - | claude |")


# ---------------------------------------------------------------- output, exit codes, S-010


def test_s010_clean_tree_prints_nothing_past_threshold(tmp_path: Path) -> None:
    result = run(make_tree(tmp_path), "report")
    assert result.returncode == 0
    assert lines(result) == ["upstream:stale: nothing past threshold"]


def test_s010_report_is_the_default_subcommand(tmp_path: Path) -> None:
    result = run(make_tree(tmp_path))
    assert result.returncode == 0
    assert lines(result) == ["upstream:stale: nothing past threshold"]


def test_s010_findings_still_exit_zero_with_summary(tmp_path: Path) -> None:
    root = with_files(tmp_path, {PRODUCT: "re-verify after 2026-09-01\n"})
    result = run(root, "report")
    assert result.returncode == 0
    assert (
        lines(result)[-1]
        == "upstream:stale: 1 stale, 0 expired, 0 unchecked, 0 missing"
    )


def test_s010_bad_today_exits_2(tmp_path: Path) -> None:
    result = run(make_tree(tmp_path), "report", today="2026-13-01")
    assert result.returncode == 2


def test_c014_missing_watchlist(tmp_path: Path) -> None:
    root = with_files(tmp_path, {WATCHLIST: None})
    result = run(root, "report")
    assert result.returncode == 0
    out = lines(result)
    assert out[0].startswith(f"MISSING {WATCHLIST}:0 — - ")
    assert out[-1] == "upstream:stale: 0 stale, 0 expired, 0 unchecked, 1 missing"


def test_c014_sort_order(tmp_path: Path) -> None:
    ledger = (
        LEDGER_HEADER
        + "| codex | harness | 1 | 2026-03-11 | deep | v1 | 2026-03-11 | |\n"
        + "| claude | harness | 1 | — | — | — | — | |\n"
        + "| cursor | harness | 1 | — | — | — | — | |\n"
    )
    root = with_files(
        tmp_path,
        {
            WATCHLIST: None,
            LEDGER: ledger,
            ".agents/research/a.md": "**Expires:** 2026-08\n",
        },
    )
    kinds = [ln.split()[0:2] for ln in findings(run(root, "report"))]
    assert kinds == [
        ["MISSING", f"{WATCHLIST}:0"],
        ["UNCHECKED", f"{LEDGER}:6"],
        ["UNCHECKED", f"{LEDGER}:7"],
        ["EXPIRED", ".agents/research/a.md:1"],  # 27d
        ["STALE", f"{LEDGER}:5"],  # 17d
    ]


def test_c014_root_found_from_subdirectory_of_a_worktree(tmp_path: Path) -> None:
    root = make_tree(tmp_path)
    (root / ".git").write_text("gitdir: /elsewhere/.git/worktrees/x\n")
    sub = root / "src" / "install"
    result = run(None, "report", cwd=sub)
    assert result.returncode == 0, result.stderr
    assert lines(result) == ["upstream:stale: nothing past threshold"]


def test_c014_no_root_exits_2(tmp_path: Path) -> None:
    result = run(None, "report", cwd=tmp_path)
    assert result.returncode == 2


def test_c014_snippet_is_capped_at_80_chars(tmp_path: Path) -> None:
    long = "re-verify after 2026-09-01 " + "x" * 200
    root = with_files(tmp_path, {PRODUCT: long + "\n"})
    (line,) = findings(run(root, "report"))
    snippet = line.split(" ", 4)[4]
    assert len(snippet) == 80


# ---------------------------------------------------------------- C-015 ladder (C-003)

LADDER_LEDGER = LEDGER_HEADER + (
    "| claude | harness | 2 | 2026-09-27 | sweep | v1 | — | |\n"
    "| a1 | harness | 2 | 2026-09-26 | feed | v1 | — | |\n"
    "| a2 | harness | 2 | 2026-09-25 | feed | v1 | — | |\n"
    "| a3 | harness | 2 | 2026-09-24 | sweep | v1 | — | |\n"
    "| t1d29 | harness | 1 | 2026-09-24 | sweep | v1 | 2026-08-29 | |\n"
    "| t1d30 | harness | 1 | 2026-09-24 | sweep | v1 | 2026-08-28 | |\n"
    "| t1never | harness | 1 | — | — | — | — | |\n"
    "| t2never | harness | 2 | — | — | — | — | |\n"
    "| forge | domain | — | 2026-09-27 | sweep | — | — | |\n"
    "| research | domain | — | — | — | — | — | |\n"
)


def ladder(tmp_path: Path, *args: str, **kw: str) -> subprocess.CompletedProcess[str]:
    root = with_files(tmp_path, {LEDGER: LADDER_LEDGER})
    return run(root, "ladder", *args, **kw)


def depths(result: subprocess.CompletedProcess[str]) -> dict[str, str]:
    assert result.returncode == 0, result.stderr
    return {ln.split()[0]: ln.split()[1] for ln in lines(result)}


def test_c015_c003_age_boundaries(tmp_path: Path) -> None:
    d = depths(ladder(tmp_path))
    assert d["claude"] == "noop"
    assert d["a1"] == "feed"
    assert d["a2"] == "feed"
    assert d["a3"] == "sweep"
    assert d["t1d29"] == "sweep"  # Tier 1, Last deep 29 d
    assert d["t1d30"] == "sweep"  # Tier 1, Last deep 30 d: due, but unnamed
    assert d["t1never"] == "sweep"
    assert d["t2never"] == "sweep"
    assert d["research"] == "sweep"
    assert d["forge"] == "noop"


def test_c015_deep_only_for_a_named_harness(tmp_path: Path) -> None:
    """A due Tier 1 row is `sweep … deep due` unless named; naming it makes it `deep`."""
    reasons = {ln.split()[0]: ln for ln in lines(ladder(tmp_path))}
    assert (
        reasons["t1d30"]
        == "t1d30 sweep checked 3d ago, Tier 1, last deep 30d ago, deep due"
    )
    assert (
        reasons["t1never"]
        == "t1never sweep never checked, Tier 1, never deep, deep due"
    )
    assert "deep due" not in reasons["t1d29"]
    for args in ((), ("--domain", "vendors"), ("--name", "research")):
        assert "deep" not in depths(ladder(tmp_path, *args)).values(), args
    named = depths(ladder(tmp_path, "--name", "t1d30", "--domain", "vendors"))
    assert named["t1d30"] == "deep"
    assert named["t1never"] == "sweep"


def test_c015_force_turns_noop_into_sweep(tmp_path: Path) -> None:
    d = depths(ladder(tmp_path, "--force"))
    assert d["claude"] == "sweep"
    assert d["forge"] == "sweep"
    assert d["a1"] == "feed"


def test_c015_order_never_checked_first_then_oldest(tmp_path: Path) -> None:
    order = [ln.split()[0] for ln in lines(ladder(tmp_path))]
    assert order == [
        "t1never", "t2never", "research",
        "a3", "t1d29", "t1d30",
        "a2", "a1", "claude", "forge",
    ]  # fmt: skip


def test_c015_named_rows_override_the_ladder(tmp_path: Path) -> None:
    result = ladder(tmp_path, "--name", "claude", "--name", "forge")
    d = depths(result)
    assert d == {
        "claude": "deep",
        "forge": "sweep",
    }  # Tier 2 harness at age 0; domain at age 0


def test_c015_domain_vendors_selects_harness_rows(tmp_path: Path) -> None:
    d = depths(ladder(tmp_path, "--domain", "vendors"))
    assert set(d) == {
        "claude",
        "a1",
        "a2",
        "a3",
        "t1d29",
        "t1d30",
        "t1never",
        "t2never",
    }


def test_c015_name_plus_domain_selects_the_union(tmp_path: Path) -> None:
    d = depths(ladder(tmp_path, "--name", "a1", "--domain", "research"))
    assert d == {"a1": "deep", "research": "sweep"}


def test_c015_other_domain_selects_its_row(tmp_path: Path) -> None:
    assert depths(ladder(tmp_path, "--domain", "research")) == {"research": "sweep"}


def test_c015_unknown_name_exits_2_listing_names(tmp_path: Path) -> None:
    result = ladder(tmp_path, "--name", "nope")
    assert result.returncode == 2
    assert "claude" in result.stderr and "forge" in result.stderr


def test_c015_unknown_domain_exits_2_listing_domains(tmp_path: Path) -> None:
    result = ladder(tmp_path, "--domain", "nope")
    assert result.returncode == 2
    assert "vendors" in result.stderr and "research" in result.stderr


def test_c015_missing_ledger_exits_2(tmp_path: Path) -> None:
    root = with_files(tmp_path, {LEDGER: None})
    assert run(root, "ladder").returncode == 2


def test_c015_unparseable_ledger_exits_2(tmp_path: Path) -> None:
    bad = LEDGER_HEADER + "| claude | harness | 1 | yesterday | — | — | — | |\n"
    root = with_files(tmp_path, {LEDGER: bad})
    assert run(root, "ladder").returncode == 2
    root2 = with_files(tmp_path / "x", {LEDGER: "# no table here\n"})
    assert run(root2, "ladder").returncode == 2


def test_c015_bad_today_exits_2(tmp_path: Path) -> None:
    assert ladder(tmp_path, today="2026-02-30").returncode == 2
