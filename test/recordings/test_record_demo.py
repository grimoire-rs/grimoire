# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Records the grim landing-page demo cast: init -> add -> multi-client
fan-out -> status.

Not part of `task verify` -- this file sits outside the acceptance suite's
`testpaths` (`test/pyproject.toml`), so a bare `pytest`/`task test` never
collects it. Run explicitly via `task demo` (see `test/taskfile.yml`). Uses
`grim_binary` (session fixture) and `GrimRunner` (helper) from the
acceptance suite instead of standing up a separate harness -- see
`.claude/artifacts/research_promotion_positioning.md` "Demo asset" and the
W6 sub-plan.

Records straight against the real `ghcr.io/grimoire-rs/skills/grim-usage`
package -- the same ref the landing page's quick start already promises --
instead of a local throwaway registry. Verified anonymously pullable before
relying on it: an unauthenticated GHCR token request for
`repository:grimoire-rs/skills/grim-usage:pull` returns a valid token,
where the same request against a private/nonexistent repo 403s (GHCR gates
token issuance itself on visibility, so a 200 there is real evidence, not
just "the endpoint responds"). `GrimRunner`'s per-instance env already
isolates `HOME`/`DOCKER_CONFIG` from the developer's real credentials, so
this genuinely exercises the anonymous-pull path grim ships. That removes
two of the three post-hoc rewrites the old recording needed (registry host,
digest) -- see `cast_recorder.py`'s module docstring for why rewriting after
the fact broke column alignment in the first place. The third (the
absolute cwd) is removed by using a short, flat temp directory instead of
pytest's deeply-nested `tmp_path`, so the real path grim prints is already
short enough to show as-is.
"""
from __future__ import annotations

import tempfile
from pathlib import Path

from src.runner import GrimRunner

from recordings.cast_recorder import CastRecorder, assert_tables_column_aligned

# The real, published grim-usage skill -- see the module docstring for how
# "anonymously pullable" was verified.
_DEMO_REF = "ghcr.io/grimoire-rs/skills/grim-usage"

CAST_OUTPUT = Path(__file__).resolve().parent.parent.parent / "docs" / "src" / "demo.cast"

# The landing page's own dark palette (docs/theme/index.hbs `:root` vars).
# asciinema-player reads a cast's embedded `theme` header automatically
# when the `AsciinemaPlayer.create()` call passes no `theme` option (see
# index.hbs's init script) -- this is the only way to get the player's
# background off its stock terminal themes and onto the page's own dark
# background. grim's plain output carries no ANSI color, so the 16-entry
# palette is cosmetically inert here; included because asciicast v2 wants
# fg/bg/palette together.
_THEME = {
    "fg": "#e9e9ed",
    "bg": "#161826",
    "palette": (
        "#161826:#c96b6b:#7fae6f:#c9a86b:#6f97c9:#9184d9:#6fb3ae:#e9e9ed:"
        "#595d6c:#dba0a0:#a8cf9c:#dbc79c:#9cbcdb:#d2cefd:#9cd4cf:#ffffff"
    ),
}


def test_record_landing_demo(grim_binary: Path) -> None:
    # A flat tempfile.TemporaryDirectory(), not pytest's tmp_path -- tmp_path
    # nests a `pytest-of-<user>/pytest-<n>/test_record_landing_demo0/` chain
    # 60-90 chars deep. grim never truncates a table column to fit the
    # terminal, so a long cwd inflates the "Path" column's computed width;
    # a short, flat root keeps the real (unmodified) path presentable.
    with tempfile.TemporaryDirectory(prefix="grim-demo-") as tmp:
        base = Path(tmp)

        # Project workspace with three client markers pre-created, so
        # `grim add` (which targets every *detected* client when nothing
        # narrows it) fans out into all three -- the "one declaration, many
        # clients" story the landing page already tells in prose, made real.
        project_dir = base / "myproject"
        for client_dir in (".claude", ".cursor", ".opencode"):
            (project_dir / client_dir).mkdir(parents=True)

        grim_home = base / "grim-home"
        grim_home.mkdir()
        runner = GrimRunner(grim_binary, grim_home, cwd=project_dir)
        env = dict(runner.env)
        # So the recording types the bare command name ("grim init") instead
        # of the absolute build path.
        env["PATH"] = f"{grim_binary.parent}:{env['PATH']}"

        recorder = CastRecorder(env=env, cwd=str(project_dir))
        recorder.open()
        recorder.run_command("grim init")
        recorder.run_command(f"grim add {_DEMO_REF}")
        recorder.run_command("find .claude .cursor .opencode -type f | sort")
        recorder.run_command("grim status")
        recorder.close()

        recorder.build(title="grim: one declaration, many clients", theme=_THEME).auto_height().write(CAST_OUTPUT)

    assert_tables_column_aligned(CAST_OUTPUT)
