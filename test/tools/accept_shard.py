# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Print the acceptance modules of one shard, one path per line, relative to ``test/``.

    python3 tools/accept_shard.py --shard-count 4 --shard-index 1

Modules are ``tests/test_*.py`` sorted by stem; shard ``I`` of ``N`` takes
``modules[I::N]``, so every module lands in exactly one shard. Exit 2 is a shard that
cannot exist, exit 1 a shard that selects nothing.
"""

from __future__ import annotations

import argparse
import sys
from collections.abc import Sequence
from pathlib import Path

TESTS_DIR = Path(__file__).resolve().parent.parent / "tests"


def modules(tests_dir: Path = TESTS_DIR) -> list[str]:
    """Sorted stems of the acceptance modules under ``tests_dir``."""
    return sorted(path.stem for path in tests_dir.glob("test_*.py"))


def select(stems: Sequence[str], count: int, index: int) -> list[str]:
    """The stems of shard ``index`` of ``count``."""
    return list(stems[index::count])


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--shard-count", type=int, required=True, help="N, at least 1")
    parser.add_argument("--shard-index", type=int, required=True, help="I in 0..N-1")
    parser.add_argument("--tests-dir", type=Path, default=TESTS_DIR)
    args = parser.parse_args(argv)
    if not 0 <= args.shard_index < args.shard_count:
        parser.error(f"shard {args.shard_index} of {args.shard_count} cannot exist")

    chosen = select(modules(args.tests_dir), args.shard_count, args.shard_index)
    # An empty list would hand pytest no paths, and pytest would then run all of `testpaths`.
    if not chosen:
        print(
            f"accept shard: shard {args.shard_index} of {args.shard_count} "
            f"selects no module under {args.tests_dir}",
            file=sys.stderr,
        )
        return 1
    print("\n".join(f"tests/{stem}.py" for stem in chosen))
    return 0


if __name__ == "__main__":
    sys.exit(main())
