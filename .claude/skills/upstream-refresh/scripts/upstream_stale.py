#!/usr/bin/env python3
"""Upstream staleness report and depth ladder for the `upstream-refresh` skill.

`report` (default, C-014) lists every ledger entry past its freshness
threshold; it is warn-only and exits 0 whenever the scan completes.
`ladder` (C-015) computes each check-ledger target's pass depth (C-003).
Both exit 2 on a usage error. Standard library only.
"""

from __future__ import annotations

import argparse
import calendar
import re
import sys
from dataclasses import dataclass
from datetime import date, datetime, timezone
from pathlib import Path

THRESHOLD_DAYS = 183
DEEP_AFTER_DAYS = 30

WATCHLIST = ".claude/rules/vendor-capability-watchlist.md"
PRODUCT_CONTEXT = ".claude/rules/product-context.md"
CHECK_LEDGER = ".agents/upstream-checks.md"

DATE = r"\d{4}-\d{2}-\d{2}"
WATCHLIST_HEADER = re.compile(r"^\|\s*(Capability|Variable)\s*\|")
LEDGER_HEADER = re.compile(r"^\|\s*Target\s*\|")
SEPARATOR = re.compile(r"^\|[\s:|-]+\|?\s*$")
HEADING = re.compile(r"^#{1,6}\s")
INLINE_VERIFIED = re.compile(rf"verified ({DATE})")
SECTION_DEFAULT = re.compile(rf"All rows `verified ({DATE})(/\d{{1,2}})?`")
STAMP = re.compile(r"verified")
REVERIFY = re.compile(rf"re-verify after ({DATE})")
EXPIRES = re.compile(r"\*\*Expires:\*\*\s*(.*)")


@dataclass(slots=True)
class Finding:
    kind: str  # STALE | EXPIRED | UNCHECKED | MISSING
    path: str
    line: int
    when: date | str  # a date, or "—" / "?"
    past: int | None  # days past the threshold; None when undated
    text: str

    def render(self) -> str:
        when = self.when.isoformat() if isinstance(self.when, date) else self.when
        past = "-" if self.past is None else f"{self.past}d"
        return f"{self.kind} {self.path}:{self.line} {when} {past} {self.text.strip()[:80]}"


def _date(text: str) -> date | None:
    """A `YYYY-MM-DD` string (callers match DATE first) → date, None if impossible."""
    try:
        return date.fromisoformat(text)
    except ValueError:
        return None


def _table_rows(lines: list[str], header: re.Pattern[str]):
    """Yield (lineno, line) for data rows of every table whose header matches."""
    in_table = False
    for n, line in enumerate(lines, 1):
        if not line.startswith("|"):
            in_table = False
        elif header.match(line):
            in_table = True
        elif in_table and not SEPARATOR.match(line):
            yield n, line


def _cells(line: str) -> list[str]:
    return [c.strip() for c in line.strip().strip("|").split("|")]


def _aged(
    kind: str, rel: str, n: int, when: date, today: date, line: str, limit: int
) -> list[Finding]:
    """One finding when `when` is more than `limit` days before today."""
    past = (today - when).days - limit
    return [Finding(kind, rel, n, when, past, line)] if past > 0 else []


# ---------------------------------------------------------------- scanners


def scan_watchlist(rel: str, lines: list[str], today: date) -> list[Finding]:
    """C-014 (1): `| Capability |` / `| Variable |` rows older than the threshold."""
    default_for: dict[int, date | None] = {}
    default: date | None = None
    for n, line in enumerate(lines, 1):
        if HEADING.match(line):
            default = None
        elif m := SECTION_DEFAULT.search(line):
            d = _date(m.group(1))
            if d and m.group(2):  # a day range takes the later day
                try:
                    d = d.replace(day=int(m.group(2)[1:]))
                except ValueError:
                    d = None
            default = d
        default_for[n] = default
    out: list[Finding] = []
    for n, line in _table_rows(lines, WATCHLIST_HEADER):
        inline = [d for d in map(_date, INLINE_VERIFIED.findall(line)) if d]
        when = max(inline) if inline else default_for[n]
        if when is None:
            out.append(Finding("STALE", rel, n, "—", None, line))
        else:
            out += _aged("STALE", rel, n, when, today, line, THRESHOLD_DAYS)
    return out


def scan_vendor_stamps(rel: str, lines: list[str], today: date) -> list[Finding]:
    """C-014 (2): `//!` `(live-)?verified` stamps, date on the same or next `//!` line."""
    out: list[Finding] = []
    for i, line in enumerate(lines):
        if not line.lstrip().startswith("//!") or not (m := STAMP.search(line)):
            continue
        found = re.findall(DATE, line[m.end() :])
        if not found and i + 1 < len(lines) and lines[i + 1].lstrip().startswith("//!"):
            found = re.findall(DATE, lines[i + 1])
        dates = [d for d in map(_date, found) if d]
        if dates:
            out += _aged("STALE", rel, i + 1, max(dates), today, line, THRESHOLD_DAYS)
    return out


def scan_reverify(rel: str, lines: list[str], today: date) -> list[Finding]:
    """C-014 (3): `re-verify after YYYY-MM-DD` once today is past it."""
    out: list[Finding] = []
    for n, line in enumerate(lines, 1):
        for d in filter(None, map(_date, REVERIFY.findall(line))):
            out += _aged("STALE", rel, n, d, today, line, 0)
    return out


def _expiry(value: str) -> date | None | str:
    """A research `Expires:` value → date, "skip" for n/a, None when unreadable."""
    if value.startswith("n/a"):
        return "skip"
    if m := re.match(rf"({DATE})\b", value):
        return _date(m.group(1))
    if m := re.match(r"(\d{4})-(\d{2})\b", value):
        year, month = int(m.group(1)), int(m.group(2))
        if 1 <= month <= 12:
            return date(year, month, calendar.monthrange(year, month)[1])
    return None


def scan_research(rel: str, lines: list[str], today: date) -> list[Finding]:
    """C-014 (4): research artifacts past their `**Expires:**` (D-7)."""
    for n, line in enumerate(lines, 1):
        if m := EXPIRES.search(line):
            when = _expiry(m.group(1).strip())
            if when == "skip":
                return []
            if when is None:
                return [Finding("EXPIRED", rel, n, "?", None, line)]
            return _aged("EXPIRED", rel, n, when, today, line, 0)
    return []  # a missing line is not reported: D-7 dates those


def scan_check_ledger(rel: str, lines: list[str], today: date) -> list[Finding]:
    """C-014 (5): check-ledger rows never checked or checked too long ago."""
    out: list[Finding] = []
    for n, line in _table_rows(lines, LEDGER_HEADER):
        cells = _cells(line)
        cell = cells[3] if len(cells) > 3 else ""
        last = _date(cell) if re.fullmatch(DATE, cell) else None
        if last is None:  # never checked, or an unreadable cell
            out.append(
                Finding("UNCHECKED", rel, n, "—" if cell == "—" else "?", None, line)
            )
        else:
            out += _aged("STALE", rel, n, last, today, line, THRESHOLD_DAYS)
    return out


# The single list of scanned sources (D-8); `references/domains.md` points here.
# A pattern that matches no file is reported MISSING.
LEDGERS = (
    (WATCHLIST, (scan_watchlist, scan_reverify)),
    ("src/install/vendor_*.rs", (scan_vendor_stamps,)),
    (PRODUCT_CONTEXT, (scan_reverify,)),
    (".agents/research/*.md", (scan_research,)),
    (CHECK_LEDGER, (scan_check_ledger,)),
)

KIND_ORDER = {"MISSING": 0, "UNCHECKED": 1}


def _sort_key(f: Finding) -> tuple:
    rank = KIND_ORDER.get(f.kind, 2)
    if rank < 2:  # MISSING in LEDGERS order, UNCHECKED in file order
        return (rank, 0, "", 0)
    past = float("inf") if f.past is None else f.past  # undated sorts as oldest
    return (rank, -past, f.path, f.line)


def report(root: Path, today: date) -> int:
    found: list[Finding] = []
    for pattern, scanners in LEDGERS:
        paths = sorted(root.glob(pattern))
        if not paths:
            found.append(Finding("MISSING", pattern, 0, "—", None, "no such file"))
        for path in paths:
            lines = path.read_text(encoding="utf-8").splitlines()
            rel = path.relative_to(root).as_posix()
            for scan in scanners:
                found += scan(rel, lines, today)
    found.sort(key=_sort_key)
    for f in found:
        print(f.render())
    counts = {
        k: sum(f.kind == k for f in found)
        for k in ("STALE", "EXPIRED", "UNCHECKED", "MISSING")
    }
    if not found:
        print("upstream:stale: nothing past threshold")
    else:
        print(
            f"upstream:stale: {counts['STALE']} stale, {counts['EXPIRED']} expired, "
            f"{counts['UNCHECKED']} unchecked, {counts['MISSING']} missing"
        )
    return 0


# ---------------------------------------------------------------- ladder


@dataclass(slots=True)
class Target:
    name: str
    kind: str  # harness | domain
    tier: str  # 1 | 2 | —
    last_check: date | None
    last_deep: date | None


def read_ledger(path: Path) -> list[Target]:
    """Parse the C-005 check ledger strictly; ValueError on any malformed row."""
    lines = path.read_text(encoding="utf-8").splitlines()
    if not any(LEDGER_HEADER.match(ln) for ln in lines):
        raise ValueError(f"{path}: no `| Target |` table")

    def cell_date(value: str, n: int) -> date | None:
        if value == "—":
            return None
        if not re.fullmatch(DATE, value) or (d := _date(value)) is None:
            raise ValueError(f"{path}:{n}: not a date or —: {value!r}")
        return d

    targets: list[Target] = []
    for n, line in _table_rows(lines, LEDGER_HEADER):
        cells = _cells(line)
        if len(cells) < 8:
            raise ValueError(f"{path}:{n}: expected 8 cells, got {len(cells)}")
        name, kind, tier, last, _depth, _cursor, deep = cells[:7]
        if kind not in ("harness", "domain") or tier not in ("1", "2", "—") or not name:
            raise ValueError(f"{path}:{n}: bad Target/Kind/Tier: {line.strip()!r}")
        targets.append(Target(name, kind, tier, cell_date(last, n), cell_date(deep, n)))
    if not targets:
        raise ValueError(f"{path}: the check ledger has no rows")
    return targets


def depth(t: Target, today: date, force: bool, named: bool) -> tuple[str, str]:
    """C-003: the pass depth and its reason. Only a named harness gets `deep`;
    an unnamed Tier 1 row due one is a `sweep` whose reason says `deep due`."""
    if named:
        return (
            ("deep", "named harness")
            if t.kind == "harness"
            else ("sweep", "named domain")
        )
    if t.last_check is None:
        reason = "never checked"
    else:
        age = (today - t.last_check).days
        if age <= 0:
            return (
                ("sweep", "checked today, --force")
                if force
                else ("noop", "checked today")
            )
        if age <= 2:
            return "feed", f"checked {age}d ago"
        reason = f"checked {age}d ago"
    if t.tier == "1":
        if t.last_deep is None:
            return "sweep", f"{reason}, Tier 1, never deep, deep due"
        deep_age = (today - t.last_deep).days
        if deep_age >= DEEP_AFTER_DAYS:
            return "sweep", f"{reason}, Tier 1, last deep {deep_age}d ago, deep due"
    return "sweep", reason


def ladder(
    root: Path, today: date, force: bool, names: list[str], domain: str | None
) -> int:
    path = root / CHECK_LEDGER
    try:
        targets = read_ledger(path)
    except (OSError, ValueError) as e:
        print(f"upstream_stale.py ladder: {e}", file=sys.stderr)
        return 2
    by_name = {t.name: t for t in targets}
    domains = ["vendors", *(t.name for t in targets if t.kind == "domain")]
    unknown = [n for n in names if n not in by_name]
    if unknown:
        print(
            f"unknown target {', '.join(unknown)}; valid: {', '.join(by_name)}",
            file=sys.stderr,
        )
        return 2
    if domain is not None and domain not in domains:
        print(f"unknown domain {domain}; valid: {', '.join(domains)}", file=sys.stderr)
        return 2

    if not names and domain is None:
        selected = targets
    else:
        selected = [
            t
            for t in targets
            if t.name in names
            or (domain == "vendors" and t.kind == "harness")
            or (t.kind == "domain" and t.name == domain)
        ]
    # never-checked first, then oldest check; sort is stable, so ties keep ledger order
    selected.sort(key=lambda t: (t.last_check is not None, t.last_check or date.min))
    for t in selected:
        d, reason = depth(t, today, force, t.name in names)
        print(f"{t.name} {d} {reason}")
    return 0


# ---------------------------------------------------------------- CLI


def _parse_today(value: str) -> date:
    d = _date(value) if re.fullmatch(DATE, value) else None
    if d is None:
        raise argparse.ArgumentTypeError(f"not a YYYY-MM-DD date: {value!r}")
    return d


def find_root(start: Path) -> Path | None:
    """Nearest ancestor holding both `taskfile.yml` and `AGENTS.md` (works in a worktree)."""
    for d in (start, *start.parents):
        if (d / "taskfile.yml").is_file() and (d / "AGENTS.md").is_file():
            return d
    return None


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="upstream_stale.py", description=__doc__.splitlines()[0]
    )
    sub = parser.add_subparsers(dest="cmd", required=True)
    commands = {
        "report": sub.add_parser("report", help="warn-only staleness report (default)"),
        "ladder": sub.add_parser("ladder", help="pass depth per check-ledger target"),
    }
    for p in commands.values():
        p.add_argument(
            "--root", type=Path, help="repository root (default: discovered from cwd)"
        )
        p.add_argument(
            "--today",
            type=_parse_today,
            default=datetime.now(timezone.utc).date(),
            help="YYYY-MM-DD (default: today, UTC)",
        )
    lad = commands["ladder"]
    lad.add_argument("--force", action="store_true", help="turn a noop into a sweep")
    lad.add_argument(
        "--name", action="append", default=[], help="target to run (repeatable)"
    )
    lad.add_argument(
        "--domain", help="`vendors` for every harness row, else one domain row"
    )

    if not argv or argv[0].startswith("-") and argv[0] not in ("-h", "--help"):
        argv = ["report", *argv]
    args = parser.parse_args(argv)

    root = args.root.resolve() if args.root else find_root(Path.cwd())
    if root is None or not root.is_dir():
        print(
            "upstream_stale.py: no repository root (taskfile.yml + AGENTS.md) found",
            file=sys.stderr,
        )
        return 2
    if args.cmd == "ladder":
        return ladder(root, args.today, args.force, args.name, args.domain)
    return report(root, args.today)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
