# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Records the grim landing-page demo cast: init -> add -> multi-client
fan-out -> status.

Not part of `task verify` -- this file sits outside the acceptance suite's
`testpaths` (`test/pyproject.toml`), so a bare `pytest`/`task test` never
collects it. Run explicitly via `task demo` (see `test/taskfile.yml`). Reuses
the acceptance suite's own fixtures (`registry`, `grim_binary`) and helpers
(`make_artifact`, `GrimRunner`) instead of standing up a separate harness --
see `.claude/artifacts/research_promotion_positioning.md` "Demo asset" and
the W6 sub-plan.
"""
from __future__ import annotations

from pathlib import Path

from src.helpers import make_artifact
from src.runner import GrimRunner

from recordings.cast_recorder import CastRecorder

# The real grim-usage skill, published at this repo path -- the same path
# the README / landing-page quick start already promise
# (`ghcr.io/grimoire-rs/skills/grim-usage`). Only the registry HOST is
# sanitized out of the recording afterward; the repo path is the real
# published path, typed verbatim.
_DEMO_REPO = "grimoire-rs/skills/grim-usage"

# grim-usage's real content, read straight off disk -- not synthetic
# fixture data. test/recordings -> test -> project root -> catalog/...
_GRIM_USAGE_DIR = Path(__file__).resolve().parent.parent.parent / "catalog" / "skills" / "grim-usage"

CAST_OUTPUT = Path(__file__).resolve().parent.parent.parent / "docs" / "src" / "demo.cast"

# The landing page's own dark palette (docs/theme/index.hbs `:root` vars).
# agg (1.7.0) auto-picks up a cast's embedded theme when no `--theme` flag
# is passed -- pass one explicitly and it rejects "custom" outright, so this
# is the *only* way to get the GIF background off agg's stock terminal
# themes and onto the page's own dark background. grim's plain output
# carries no ANSI color, so the 16-entry palette is cosmetically inert here;
# included because asciicast v2 wants fg/bg/palette together.
_THEME = {
    "fg": "#e9e9ed",
    "bg": "#161826",
    "palette": (
        "#161826:#c96b6b:#7fae6f:#c9a86b:#6f97c9:#9184d9:#6fb3ae:#e9e9ed:"
        "#595d6c:#dba0a0:#a8cf9c:#dbc79c:#9cbcdb:#d2cefd:#9cd4cf:#ffffff"
    ),
}


def test_record_landing_demo(registry: str, grim_binary: Path, tmp_path: Path) -> None:
    # Push the real skill content so the demo installs something real, not a
    # synthetic fixture.
    files = {
        f"grim-usage/{p.relative_to(_GRIM_USAGE_DIR).as_posix()}": p.read_bytes()
        for p in _GRIM_USAGE_DIR.rglob("*")
        if p.is_file()
    }
    make_artifact(_DEMO_REPO, "skill", files, tag="latest")

    # Project workspace with three client markers pre-created, so `grim add`
    # (which targets every *detected* client when nothing narrows it) fans
    # out into all three -- the "one declaration, many clients" story the
    # landing page already tells in prose, made real.
    project_dir = tmp_path / "project"
    for client_dir in (".claude", ".cursor", ".opencode"):
        (project_dir / client_dir).mkdir(parents=True)

    grim_home = tmp_path / "grim-home"
    grim_home.mkdir()
    runner = GrimRunner(grim_binary, grim_home, cwd=project_dir)
    env = dict(runner.env)
    # So the recording types the bare command name ("grim init") instead of
    # the absolute build path.
    env["PATH"] = f"{grim_binary.parent}:{env['PATH']}"

    recorder = CastRecorder(env=env, cwd=str(project_dir))
    recorder.open()
    recorder.run_command("grim init")
    recorder.run_command(f"grim add {registry}/{_DEMO_REPO}")
    recorder.run_command("find .claude .cursor .opencode -type f | sort")
    recorder.run_command("grim status")
    recorder.close()

    (
        recorder.build(title="grim: one declaration, many clients", theme=_THEME)
        .sanitize({
            f"{registry}/": "ghcr.io/",
            str(project_dir): "~/myproject",
            str(runner.home): "~",
        })
        .shorten_digests()
        .auto_height()
        .write(CAST_OUTPUT)
    )
